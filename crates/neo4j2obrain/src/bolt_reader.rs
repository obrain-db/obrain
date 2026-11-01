//! # Stage 1 — Neo4j bolt streaming reader
//!
//! Compiled only with `--features neo4j-bolt`. Pulls nodes + edges off
//! a live Neo4j database via [`neo4rs`] and writes them into the
//! [`SubstrateStore`] one batch at a time. The intent is to keep peak
//! memory roughly bounded by `batch_size × (avg node payload)` — the
//! batch is flushed to substrate + WAL before the next one is fetched.
//!
//! ## Query shape
//!
//! The reader issues two straightforward `CALL apoc.periodic.iterate`
//! equivalents using `SKIP / LIMIT` windows:
//!
//! ```cypher
//! MATCH (n) RETURN id(n) AS nid, labels(n) AS labels, properties(n) AS props
//!   SKIP $skip LIMIT $limit
//! ```
//!
//! ```cypher
//! MATCH (a)-[r]->(b)
//!   RETURN id(a) AS src, id(b) AS dst, type(r) AS ty, properties(r) AS props
//!   SKIP $skip LIMIT $limit
//! ```
//!
//! Properties are translated 1:1 to [`obrain_common::types::Value`] via
//! [`value_from_bolt`]; unsupported kinds (Geo, Date types) are warned
//! once and dropped.
//!
//! ## Scope
//!
//! This is a structural reader — it does **not** preserve the Neo4j
//! internal node id (Neo4j ids are dense but not contractually stable
//! across DB restarts). Edges reference substrate [`NodeId`]s via a
//! `HashMap<i64, NodeId>` populated during the node pass.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use obrain_common::NodeId;
use obrain_common::types::{PropertyKey, Value};
use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::SubstrateStore;

use crate::pipeline::PipelineOptions;

/// Reader output — plain counters suitable for `PipelineStats`.
#[derive(Debug, Default, Clone, Copy)]
pub struct ReadCounts {
    pub nodes: u64,
    pub edges: u64,
}

/// Stream nodes + edges from Neo4j into the given substrate store.
///
/// Opens a single bolt connection, walks the full graph in
/// `opts.batch_size` windows, and commits each batch to substrate
/// before requesting the next.
pub async fn stream_into_substrate(
    opts: &PipelineOptions,
    substrate: &Arc<SubstrateStore>,
) -> Result<ReadCounts> {
    let graph = neo4rs::Graph::new(
        &opts.neo4j_url,
        opts.neo4j_user.clone(),
        opts.neo4j_password.clone(),
    )
    .await
    .with_context(|| format!("connect to Neo4j at {}", opts.neo4j_url))?;

    // ── (2026-08-19) COMPTER LA SOURCE D ABORD ──
    // Sans reference, un import tronque est indistinguable d un import
    // complet : on ne peut que constater qu il s est termine. Deux comptages
    // au demarrage coutent quelques secondes et donnent la verite a laquelle
    // confronter le resultat.
    let attendu_nœuds: i64 = {
        let mut r = graph
            .execute(neo4rs::query("MATCH (n) RETURN count(n) AS c"))
            .await
            .context("comptage des nœuds source")?;
        match r.next().await {
            Ok(Some(row)) => row.get("c").unwrap_or(-1),
            _ => -1,
        }
    };
    let attendu_aretes: i64 = {
        let mut r = graph
            .execute(neo4rs::query("MATCH ()-[r]->() RETURN count(r) AS c"))
            .await
            .context("comptage des arêtes source")?;
        match r.next().await {
            Ok(Some(row)) => row.get("c").unwrap_or(-1),
            _ => -1,
        }
    };
    tracing::info!(
        attendu_nœuds,
        attendu_aretes,
        "source comptée — référence pour la vérification finale"
    );

    let mut counts = ReadCounts::default();
    // ── (2026-08-19) PRE-DIMENSIONNE ──
    // `id_map` doit contenir une entree par nœud — 3,3 M pour le
    // project-orchestrator — pour que la passe des aretes resolve ses
    // extremites. Partir de zero impose une vingtaine de rehachages, chacun
    // recopiant l integralite de la table : le dernier deplace des centaines
    // de mega-octets. On reserve d emblee.
    //
    // La valeur vient de la mesure : 3 328 876 nœuds cote source. On prend
    // large — une reservation trop petite ne coute qu un rehachage, une trop
    // grande ne coute que de la memoire virtuelle.
    // Remplie par le thread ecrivain de la passe des nœuds, qui la rend au join.
    let mut id_map: HashMap<i64, NodeId> = HashMap::new();
    let lot: u64 = opts.batch_size.max(1) as u64;

    // ═══════════════════════════════════════════════════════════════════
    // (2026-08-19) LA PAGINATION `SKIP` COUTAIT UN FACTEUR 224
    //
    // Les deux passes paginaient par `SKIP $skip LIMIT $limit`. Sur un
    // balayage NON INDEXE — et `MATCH (n)` comme `MATCH ()-[r]->()` en sont —
    // Neo4j execute `SKIP n` en RE-PARCOURANT les n premieres lignes pour les
    // jeter. Avec des lots de 10 000 sur 4,4 M d aretes, cela fait 444
    // requetes dont la derniere reparcourt 4 440 000 lignes pour en garder
    // 10 000 : 987 MILLIONS de lignes parcourues au lieu de 4,4 millions.
    // Du O(N^2) exact.
    //
    // MESURE QUI L A ETABLI (nuit du 18 au 19/08) :
    //   · debit d ecriture observe pendant l import : ~2 500 aretes/s
    //   · debit du substrat mesure au banc            : 195 000 aretes/s
    //     (600 000 aretes + 2 proprietes en 3,07 s, cout PLAT sur 12
    //      tranches — le substrat etait innocent)
    //   · decroissance du debit, monotone et sans palier :
    //       967 k -> 799 k -> 433 k -> 150 k LSN/min
    //     signature exacte d un `SKIP` croissant.
    //   · reseau a 1 Mo/min pendant que le CPU tenait 93 % : Neo4j peinait a
    //     PRODUIRE les lignes, il n avait pas de mal a les transmettre.
    //
    // Et cela explique pourquoi personne ne l avait vu : sur un graphe dix
    // fois plus petit, le cout quadratique est cent fois moindre. L operateur
    // se souvenait d imports en 20 minutes — c etait vrai, a cette echelle-la.
    //
    // LE CORRECTIF n est pas une pagination par cle mais l ABSENCE de
    // pagination : bolt DIFFUSE ses resultats. Une seule requete, consommee
    // au fil de l eau, fait un seul balayage — O(N). La memoire reste bornee
    // puisqu on ecrit chaque ligne au lieu de les accumuler, et on continue a
    // vider (`flush`) tous les `batch_size` enregistrements.
    // ═══════════════════════════════════════════════════════════════════

    // ── Passe des nœuds ────────────────────────────────────────────────
    //
    // ── (2026-08-19) LE GOULOT N ETAIT NI NEO4J NI LE SUBSTRAT ──
    //
    // Mesures qui ont mene ici :
    //   · Neo4j, en local, sans reseau      : 37 400 nœuds/s
    //   · le substrat, au banc              : 195 000 aretes/s (cout PLAT)
    //   · cette boucle                      :   7 900 nœuds/s
    //
    // Les deux extremites peuvent donc encaisser 5 a 25 fois plus que ce
    // qu on obtenait. Le goulot etait la STRUCTURE de la boucle : elle est
    // `async`, mais `create_node_with_props` est SYNCHRONE et BLOQUANTE. A
    // chaque ligne on bloquait le worker tokio pour ecrire, pendant que le
    // flux bolt attendait ; puis on relisait pendant que le substrat
    // attendait. Lecture (≈50 %) et ecriture (≈15 %) s ALTERNAIENT au lieu
    // de se recouvrir, et le reste partait en suspensions de tache — un
    // point de reprise par ligne, 7 900 fois par seconde.
    //
    // On decouple : la tache async ne fait que lire et convertir, elle pousse
    // dans un canal borne ; un thread dedie consomme et ecrit. Les deux
    // avancent EN MEME TEMPS, et le canal borne empeche le lecteur de prendre
    // trop d avance (la memoire reste plafonnee par sa capacite).
    //
    // `id_map` appartient desormais au thread ecrivain — c est lui qui
    // connait l identifiant substrat rendu par l ecriture. Il le rend au
    // moment du join, pour la passe des aretes qui en depend.
    {
        let q = neo4rs::query(
            "MATCH (n) RETURN id(n) AS nid, labels(n) AS labels, properties(n) AS props",
        );
        let t0 = std::time::Instant::now();
        let mut res = graph.execute(q).await.context("exec node query")?;
        tracing::info!(
            phase = "nodes",
            requete_ms = t0.elapsed().as_millis() as u64,
            "Stage 1 : flux des nœuds ouvert (une seule requête, sans SKIP)"
        );

        // Capacite du canal : assez pour absorber une rafale du lecteur sans
        // laisser la memoire filer. 16 384 lignes converties tiennent
        // largement, et bornent l avance a une fraction de seconde.
        let (tx, rx) = std::sync::mpsc::sync_channel::<(i64, Vec<String>, Vec<(PropertyKey, Value)>)>(16_384);

        let sub = Arc::clone(substrate);
        let lot_flush = lot;
        let ecrivain = std::thread::spawn(move || {
            let mut map: HashMap<i64, NodeId> = HashMap::with_capacity(4_000_000);
            let mut n = 0u64;
            let mut t_ecriture = std::time::Duration::ZERO;
            // ── (2026-08-19) DECOMPOSER L ECRITURE, SANS LA CHANGER ──
            //
            // `envoi_pct` monte de 61 a 72 % : le lecteur attend sur un canal
            // plein, donc c est CE thread le frein. Et il ralentit — 1629 puis
            // 1259 nœuds/s. Reste a savoir OU : la creation du nœud, ou les
            // proprietes ? Les nœuds du PO en portent une cinquantaine, dont
            // un `embedding` de 768 flottants qui part sur un chemin different
            // (colonne vectorielle) des scalaires.
            //
            // On remplace `create_node_with_props` par ce qu il fait DEJA en
            // interne — l implementation par defaut du trait est exactement
            // `create_node` puis un `set_node_property` par propriete
            // (obrain-core/graph/traits.rs). Meme sequence, memes appels,
            // meme resultat : on ne fait qu y poser des chronometres. Aucun
            // changement de comportement, aucun risque pour les donnees.
            let mut t_creation = std::time::Duration::ZERO;
            let mut t_scalaires = std::time::Duration::ZERO;
            let mut t_vecteurs = std::time::Duration::ZERO;
            let mut n_props = 0u64;
            let mut n_vec = 0u64;
            let mut jalon_ecr = std::time::Instant::now();
            let mut n_au_jalon = 0u64;

            while let Ok((nid_raw, labels, props)) = rx.recv() {
                let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
                let b = std::time::Instant::now();

                let c = std::time::Instant::now();
                let new_id = sub.create_node(&refs);
                t_creation += c.elapsed();

                for (cle, val) in &props {
                    let est_vec = matches!(val, Value::Vector(_));
                    let d = std::time::Instant::now();
                    sub.set_node_property(new_id, cle.as_str(), val.clone());
                    if est_vec {
                        t_vecteurs += d.elapsed();
                        n_vec += 1;
                    } else {
                        t_scalaires += d.elapsed();
                    }
                    n_props += 1;
                }

                t_ecriture += b.elapsed();
                map.insert(nid_raw, new_id);
                n += 1;
                if n % lot_flush == 0 {
                    sub.flush().ok();
                }
                // Le thread rend compte de LUI-MEME : c est lui le frein, donc
                // c est sa decomposition qui compte, pas celle du lecteur.
                if n % PALIER == 0 {
                    let dj = jalon_ecr.elapsed().as_secs_f64().max(1e-9);
                    let te = t_ecriture.as_secs_f64().max(1e-9);
                    tracing::info!(
                        phase = "nodes.writer",
                        total = n,
                        debit_par_s = ((n - n_au_jalon) as f64 / dj) as u64,
                        props_par_nœud = n_props / n.max(1),
                        props_par_s = (n_props as f64 / te) as u64,
                        creation_pct = (100.0 * t_creation.as_secs_f64() / te) as u32,
                        scalaires_pct = (100.0 * t_scalaires.as_secs_f64() / te) as u32,
                        vecteurs_pct = (100.0 * t_vecteurs.as_secs_f64() / te) as u32,
                        vecteurs_ecrits = n_vec,
                        "écriture"
                    );
                    jalon_ecr = std::time::Instant::now();
                    n_au_jalon = n;
                }
            }
            sub.flush().ok();
            (map, n, t_ecriture)
        });

        let mut t_lecture = std::time::Duration::ZERO;
        let mut t_conversion = std::time::Duration::ZERO;
        let mut t_envoi = std::time::Duration::ZERO;
        let debut = std::time::Instant::now();
        let mut jalon = std::time::Instant::now();
        let mut lues = 0u64;
        let mut au_jalon = 0u64;

        loop {
            let a = std::time::Instant::now();
            let suivant = res.next().await;
            t_lecture += a.elapsed();
            let row = match suivant {
                Ok(Some(r)) => r,
                Ok(None) => break,
                Err(err) => {
                    drop(tx);
                    let _ = ecrivain.join();
                    return Err(anyhow::anyhow!(err)).context(format!(
                        "flux des nœuds INTERROMPU après {lues} lignes — import \
                         incomplet, ne pas considérer le substrat comme valide"
                    ));
                }
            };

            let c = std::time::Instant::now();
            let nid_raw: i64 = row.get("nid").context("row missing nid")?;
            let labels: Vec<String> = row.get("labels").unwrap_or_default();
            let props_raw: neo4rs::BoltMap = row.get("props").unwrap_or_default();
            let props = bolt_map_to_props(&props_raw);
            t_conversion += c.elapsed();

            let e = std::time::Instant::now();
            if tx.send((nid_raw, labels, props)).is_err() {
                anyhow::bail!("le thread d écriture des nœuds s est arrêté prématurément");
            }
            t_envoi += e.elapsed();

            lues += 1;
            if lues % PALIER == 0 {
                let d = jalon.elapsed().as_secs_f64().max(1e-9);
                let tot = debut.elapsed().as_secs_f64().max(1e-9);
                tracing::info!(
                    phase = "nodes",
                    total = lues,
                    debit_par_s = ((lues - au_jalon) as f64 / d) as u64,
                    moyenne_par_s = (lues as f64 / tot) as u64,
                    lecture_pct = (100.0 * t_lecture.as_secs_f64() / tot) as u32,
                    conversion_pct = (100.0 * t_conversion.as_secs_f64() / tot) as u32,
                    // `envoi_pct` eleve = le thread ecrivain est le frein
                    // (le canal est plein) ; bas = c est la lecture.
                    envoi_pct = (100.0 * t_envoi.as_secs_f64() / tot) as u32,
                    "nœuds"
                );
                jalon = std::time::Instant::now();
                au_jalon = lues;
            }
        }

        drop(tx);
        let (map, ecrits, t_ecriture) = ecrivain
            .join()
            .map_err(|_| anyhow::anyhow!("le thread d écriture des nœuds a paniqué"))?;
        id_map = map;
        counts.nodes = ecrits;

        let tot = debut.elapsed();
        tracing::info!(
            phase = "nodes",
            total = counts.nodes,
            secondes = tot.as_secs(),
            debit_par_s = (counts.nodes as f64 / tot.as_secs_f64().max(1e-9)) as u64,
            lecture_pct = (100.0 * t_lecture.as_secs_f64() / tot.as_secs_f64().max(1e-9)) as u32,
            conversion_pct = (100.0 * t_conversion.as_secs_f64() / tot.as_secs_f64().max(1e-9)) as u32,
            envoi_pct = (100.0 * t_envoi.as_secs_f64() / tot.as_secs_f64().max(1e-9)) as u32,
            ecriture_thread_pct = (100.0 * t_ecriture.as_secs_f64() / tot.as_secs_f64().max(1e-9)) as u32,
            "Stage 1 (nodes) TERMINE"
        );
    }

    // ── Passe des arêtes ───────────────────────────────────────────────
    let mut orphelines_totales = 0u64;
    {
        let q = neo4rs::query(
            "MATCH (a)-[r]->(b) RETURN id(a) AS src, id(b) AS dst, \
             type(r) AS ty, properties(r) AS props",
        );
        let t0 = std::time::Instant::now();
        let mut res = graph.execute(q).await.context("exec edge query")?;
        let t_requete = t0.elapsed();
        tracing::info!(
            phase = "edges",
            requete_ms = t_requete.as_millis() as u64,
            "Stage 1 : flux des arêtes ouvert (une seule requête, sans SKIP)"
        );

        let mut t_lecture = std::time::Duration::ZERO;
        let mut t_extraction = std::time::Duration::ZERO;
        let mut t_conversion = std::time::Duration::ZERO;
        let mut t_ecriture = std::time::Duration::ZERO;
        let mut orphelines = 0u64;
        let debut = std::time::Instant::now();
        let mut jalon = std::time::Instant::now();
        let mut au_jalon = 0u64;

        loop {
            let a = std::time::Instant::now();
            let suivant = res.next().await;
            t_lecture += a.elapsed();
            let row = match suivant {
                Ok(Some(r)) => r,
                Ok(None) => break, // fin normale du flux
                Err(err) => {
                    return Err(anyhow::anyhow!(err)).context(format!(
                        "flux des arêtes INTERROMPU après {} arêtes — import \
                         incomplet, ne pas considérer le substrat comme valide",
                        counts.edges
                    ));
                }
            };

            let e = std::time::Instant::now();
            let src_raw: i64 = row.get("src").context("row missing src")?;
            let dst_raw: i64 = row.get("dst").context("row missing dst")?;
            let ty: String = row.get("ty").unwrap_or_else(|_| "REL".into());
            let props_raw: neo4rs::BoltMap = row.get("props").unwrap_or_default();
            t_extraction += e.elapsed();

            let (Some(&src), Some(&dst)) = (id_map.get(&src_raw), id_map.get(&dst_raw)) else {
                // Extremite absente du balayage des nœuds — rare, mais
                // possible si la base bouge pendant l import. On COMPTE au
                // lieu de laisser filer en silence.
                orphelines += 1;
                continue;
            };

            let c = std::time::Instant::now();
            let props = bolt_map_to_props(&props_raw);
            t_conversion += c.elapsed();

            let b = std::time::Instant::now();
            let _ = substrate.create_edge_with_props(src, dst, &ty, &props);
            t_ecriture += b.elapsed();

            counts.edges += 1;
            if counts.edges % lot == 0 {
                substrate.flush().ok();
            }
            if counts.edges % PALIER == 0 {
                let d = jalon.elapsed().as_secs_f64().max(1e-9);
                let n = counts.edges - au_jalon;
                let tot = debut.elapsed().as_secs_f64().max(1e-9);
                tracing::info!(
                    phase = "edges",
                    total = counts.edges,
                    debit_par_s = (n as f64 / d) as u64,
                    moyenne_par_s = (counts.edges as f64 / tot) as u64,
                    lecture_pct = (100.0 * t_lecture.as_secs_f64() / tot) as u32,
                    extraction_pct = (100.0 * t_extraction.as_secs_f64() / tot) as u32,
                    conversion_pct = (100.0 * t_conversion.as_secs_f64() / tot) as u32,
                    ecriture_pct = (100.0 * t_ecriture.as_secs_f64() / tot) as u32,
                    orphelines,
                    "arêtes"
                );
                jalon = std::time::Instant::now();
                au_jalon = counts.edges;
            }
        }
        substrate.flush().ok();
        let tot = debut.elapsed();
        tracing::info!(
            phase = "edges",
            total = counts.edges,
            orphelines,
            secondes = tot.as_secs(),
            debit_par_s = (counts.edges as f64 / tot.as_secs_f64().max(1e-9)) as u64,
            lecture_pct = (100.0 * t_lecture.as_secs_f64() / tot.as_secs_f64().max(1e-9)) as u32,
            ecriture_pct = (100.0 * t_ecriture.as_secs_f64() / tot.as_secs_f64().max(1e-9)) as u32,
            "Stage 1 (edges) TERMINE"
        );
        orphelines_totales = orphelines;
    }

    // ── (2026-08-19) VERIFICATION : ce qu on a lu vaut-il ce qu il y avait ? ──
    // On ECHOUE plutot que d avertir. Un substrat incomplet qui se presente
    // comme complet contamine tout ce qui vient apres — les communautes, la
    // centralite et les embeddings seraient calcules sur un graphe tronque,
    // sans que rien ne le signale.
    //
    // Les arêtes orphelines sont soustraites : elles sont LEGITIMEMENT
    // absentes du substrat (leur extremite manquait au balayage des nœuds,
    // ce qui arrive si la base source bouge pendant l import) et elles sont
    // comptees a part, donc explicables.
    // ── (2026-08-19) LA SOURCE VIT PENDANT QU ON LA LIT ──
    //
    // Operateur : « tu auras plus de nœuds et d arêtes à la fin de l import,
    // cette discussion en génère ». C est exact et ce n est pas un detail :
    // le Neo4j du project-orchestrator est EN PRODUCTION, il enregistre des
    // ChatEvent pendant que nous parlons. Une egalite stricte entre le
    // comptage d avant et le resultat rejetterait donc un import parfaitement
    // valide — j avais ecrit exactement ce piege dix minutes plus tot.
    //
    // La regle juste est ASYMETRIQUE : un DEFICIT signale une troncature et
    // doit echouer ; un EXCEDENT signale seulement que la base a grossi entre
    // le comptage et la fin du flux, ce qui est normal et sans consequence —
    // le flux bolt travaille sur son propre instantane.
    //
    // On tolere aussi une marge en dessous : le comptage et l ouverture du
    // flux ne sont pas simultanes, donc quelques entites creees dans cet
    // intervalle peuvent manquer a l instantane. La marge est proportionnelle
    // (0,1 %) plutot que fixe, pour rester juste a toutes les echelles.
    let marge = |attendu: i64| -> i64 { (attendu / 1000).max(100) };

    if attendu_nœuds >= 0 {
        let deficit = attendu_nœuds - counts.nodes as i64;
        if deficit > marge(attendu_nœuds) {
            anyhow::bail!(
                "IMPORT TRONQUÉ : {} nœuds importés pour {} comptés à la source \
                 (déficit {}, marge tolérée {}). Le substrat est INCOMPLET — ne \
                 pas lancer les étapes cognitives dessus.",
                counts.nodes,
                attendu_nœuds,
                deficit,
                marge(attendu_nœuds)
            );
        }
        if deficit < 0 {
            tracing::info!(
                importes = counts.nodes,
                comptes_avant = attendu_nœuds,
                "la source a grossi pendant l import (normal — base en production)"
            );
        }
    }
    if attendu_aretes >= 0 {
        let comptees = counts.edges as i64 + orphelines_totales as i64;
        let deficit = attendu_aretes - comptees;
        if deficit > marge(attendu_aretes) {
            anyhow::bail!(
                "IMPORT TRONQUÉ : {} arêtes + {} orphelines = {} pour {} comptées \
                 à la source (déficit {}, marge tolérée {}). Substrat INCOMPLET.",
                counts.edges,
                orphelines_totales,
                comptees,
                attendu_aretes,
                deficit,
                marge(attendu_aretes)
            );
        }
    }
    tracing::info!(
        nœuds = counts.nodes,
        aretes = counts.edges,
        orphelines = orphelines_totales,
        "VÉRIFICATION OK — les comptes correspondent à la source"
    );

    Ok(counts)
}

/// Fréquence des lignes de progression, en entités.
///
/// 100 000 tient le journal lisible (33 lignes pour 3,3 M de nœuds, 44 pour
/// 4,4 M d arêtes) tout en donnant un point de mesure toutes les quelques
/// secondes au débit nominal. C est aussi ce que l interface du PO lira pour
/// afficher sa progression.
const PALIER: u64 = 100_000;

/// Translate a Neo4j bolt map into substrate [`Value`]s.
///
/// Supported kinds:
/// * scalars — Null, Boolean, Integer, Float, String, Bytes
/// * temporal — Date, DateTime, LocalDateTime, DateTimeZoneId
///   (collapsed to [`Value::Timestamp`] in seconds)
/// * containers — List (→ `Vector<f32>` if all-numeric, else recursive `List`)
///   and Map (recursive)
///
/// Dropped kinds (with a single warn-level trace per occurrence): Node,
/// Relation, UnboundedRelation, Path, Point2D, Point3D, Duration, Time,
/// LocalTime. These are either embedded-graph constructs that do not
/// translate 1:1 to properties, or geo / duration types the cognitive
/// pipeline does not consume.
fn bolt_map_to_props(map: &neo4rs::BoltMap) -> Vec<(PropertyKey, Value)> {
    let mut out = Vec::with_capacity(map.value.len());
    for (k, v) in map.value.iter() {
        if let Some(val) = value_from_bolt(v) {
            out.push((PropertyKey::new(k.value.as_str()), val));
        }
    }
    out
}

/// Best-effort translation of a single bolt value.
///
/// Returns `None` when the value is Null or of a kind we intentionally
/// drop. The first two cases are separated in the match so Null does
/// not trip the warn branch (Null is semantically a valid "absent"
/// marker, not an unsupported type).
fn value_from_bolt(v: &neo4rs::BoltType) -> Option<Value> {
    use neo4rs::BoltType::*;
    use obrain_common::types::Timestamp;

    Some(match v {
        Null(_) => return None,
        Boolean(b) => Value::Bool(b.value),
        Integer(i) => Value::Int64(i.value),
        Float(f) => Value::Float64(f.value),
        String(s) => Value::String(s.value.clone().into()),
        Bytes(b) => Value::Bytes(b.value.as_ref().into()),

        // -------- Lists --------------------------------------------------
        //
        // Neo4j embedding vectors are almost always `List<Float>` on the
        // bolt wire. Detect that shape and fast-path to `Value::Vector`
        // (Arc<[f32]>) so the cognitive pipeline sees them natively.
        // Fallback: recursive translation into `Value::List` for
        // heterogeneous or non-numeric lists.
        List(list) => translate_list(list)?,

        // -------- Maps ---------------------------------------------------
        Map(m) => {
            use std::collections::BTreeMap;
            use std::sync::Arc;
            let mut bt = BTreeMap::new();
            for (k, v) in m.value.iter() {
                if let Some(val) = value_from_bolt(v) {
                    bt.insert(PropertyKey::new(k.value.as_str()), val);
                }
            }
            if bt.is_empty() {
                return None;
            }
            Value::Map(Arc::new(bt))
        }

        // -------- Temporal ----------------------------------------------
        //
        // We collapse every time-zoned / local / date-only variant to
        // a single UTC-seconds `Timestamp`. The cognitive pipeline does
        // not distinguish; preserving the original kind would force us
        // to fan out into several obrain Value variants for no benefit.
        DateTime(dt) => {
            use chrono::DateTime as ChronoDT;
            use chrono::FixedOffset;
            match ChronoDT::<FixedOffset>::try_from(dt) {
                Ok(t) => Value::Timestamp(Timestamp::from_secs(t.timestamp())),
                Err(_) => return None,
            }
        }
        LocalDateTime(dt) => {
            use chrono::NaiveDateTime;
            match NaiveDateTime::try_from(dt) {
                Ok(t) => Value::Timestamp(Timestamp::from_secs(t.and_utc().timestamp())),
                Err(_) => return None,
            }
        }
        DateTimeZoneId(dt) => {
            use chrono::DateTime as ChronoDT;
            use chrono::FixedOffset;
            match ChronoDT::<FixedOffset>::try_from(dt) {
                Ok(t) => Value::Timestamp(Timestamp::from_secs(t.timestamp())),
                Err(_) => return None,
            }
        }
        Date(d) => {
            use chrono::NaiveDate;
            match NaiveDate::try_from(d) {
                Ok(nd) => {
                    // Midnight UTC on that date. Keeps ordering semantics
                    // without introducing a distinct Date variant.
                    let ts = nd
                        .and_hms_opt(0, 0, 0)
                        .map(|ndt| ndt.and_utc().timestamp())
                        .unwrap_or(0);
                    Value::Timestamp(Timestamp::from_secs(ts))
                }
                Err(_) => return None,
            }
        }

        // -------- Dropped kinds -----------------------------------------
        _ => {
            tracing::warn!(
                "bolt_reader: dropping unsupported Neo4j value kind ({:?})",
                std::mem::discriminant(v),
            );
            return None;
        }
    })
}

/// Translate a `BoltList` into either a dense `Value::Vector` (when
/// every element is numeric — the embedding case) or a heterogeneous
/// `Value::List`.
///
/// The numeric fast-path inspects every element: as soon as a non-numeric
/// one is seen we fall back to the recursive path. Integers are cast to
/// f32 so a `List<Integer>` that happens to encode a dense vector (rare,
/// but possible in legacy schemas) still lands as `Value::Vector`.
fn translate_list(list: &neo4rs::BoltList) -> Option<Value> {
    use neo4rs::BoltType::*;
    use std::sync::Arc;

    if list.value.is_empty() {
        return Some(Value::List(Arc::from(Vec::<Value>::new())));
    }

    // Numeric detection — Float and/or Integer only.
    let all_numeric = list
        .value
        .iter()
        .all(|e| matches!(e, Float(_) | Integer(_)));

    if all_numeric {
        let mut buf: Vec<f32> = Vec::with_capacity(list.value.len());
        for e in list.value.iter() {
            match e {
                Float(f) => buf.push(f.value as f32),
                Integer(i) => buf.push(i.value as f32),
                _ => unreachable!("numeric-only path already checked"),
            }
        }
        return Some(Value::Vector(Arc::from(buf)));
    }

    // Heterogeneous / non-numeric: recurse per element, skip Nulls and
    // drops cleanly. An all-dropped list collapses to None so we don't
    // write an empty `List` property — matches the Map handling above.
    let mut items: Vec<Value> = Vec::with_capacity(list.value.len());
    for e in list.value.iter() {
        if let Some(v) = value_from_bolt(e) {
            items.push(v);
        }
    }
    if items.is_empty() {
        return None;
    }
    Some(Value::List(Arc::from(items)))
}
