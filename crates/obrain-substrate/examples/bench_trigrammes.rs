//! # `bench_trigrammes` — ce qu'un index de trigrammes achète, mesuré sur un store réel
//!
//! Répond à une seule question, mais avec des chiffres : **les requêtes de
//! SOUS-CHAÎNE (`n.name CONTAINS 'iam'`) peuvent-elles cesser d'être des
//! balayages de 347 970 lignes ?**
//!
//! Le banc fait trois choses, dans cet ordre :
//!
//! 1. il COLLECTE les valeurs d'une propriété par lots (`get_node_property_batch`,
//!    celui-là même que la revérification d'index utilise désormais) ;
//! 2. il BÂTIT l'index au seuil demandé et publie ce qu'il coûte — grammes,
//!    postings, mémoire, temps de construction ;
//! 3. pour chaque terme, il compare les CANDIDATS rendus par l'index aux
//!    VRAIES lignes comptées par balayage — c'est la preuve du sur-ensemble,
//!    terme par terme, et non une impression — puis chronomètre la recherche.
//!
//! ## Usage
//!
//! ```text
//! cargo run --release -p obrain-substrate --example bench_trigrammes -- \
//!     ~/bench-stores/runtime-dev-snapshot-20260909.obrain [SEUIL]
//! ```
//!
//! Sans seuil, [`SEUIL_DEFAUT`] est utilisé. Le seuil `0` n'indexe rien : c'est
//! l'échappatoire qui montre le comportement « l'index se tait, on balaie ».
//!
//! ## Ce que le banc NE dit PAS
//!
//! Il ne mesure pas la requête GQL de bout en bout : l'index n'est pas encore
//! branché dans le planificateur. Il donne la borne : `candidats × coût d'une
//! revérification par lot` + le coût fixe mesuré du chemin d'intervalle. Le
//! branchement et la mesure de bout en bout viennent après.

use std::time::Duration;
use web_time::Instant;

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::traits::{GraphStore, PredicatTexte};
use obrain_substrate::SubstrateStore;
use obrain_substrate::props_trigram_index::{PropsTrigramIndex, SEUIL_DEFAUT};

/// Les termes de la plainte du 2026-09-12, plus deux témoins :
/// un terme COMMUN (dont les grammes ne sont pas indexés — l'index doit se
/// taire, pas rendre vide) et un terme ABSENT.
const TERMES: &[&str] = &[
    "iam",
    "IAM",
    "auth",
    "Auth",
    "token",
    "Token",
    "obrain-iam",
    // Les PRÉFIXES réels des requêtes C3/C4/C8 du corpus : un long préfixe de
    // chemin, dont les grammes sont des fragments de chemin (donc communs).
    "home.triviere.projects.banc_obrain.crates.obrain-iam",
    "home.triviere.projects.banc_obrain.crates.obrain-iam.src",
    "ion",         // commun : tous ses grammes dépassent le seuil
    "zzqq-absent", // absent du corpus
];

fn collecter(store: &SubstrateStore, prop: &str) -> (Vec<NodeId>, Vec<(u32, String)>, Duration) {
    let t = Instant::now();
    let tous = store.find_nodes_in_range(prop, None, None, true, true);
    let cle = PropertyKey::new(prop);
    let mut out: Vec<(u32, String)> = Vec::with_capacity(tous.len());
    for lot in tous.chunks(8192) {
        let valeurs = store.get_node_property_batch(lot, &cle);
        for (id, v) in lot.iter().zip(valeurs) {
            if let Some(Value::String(s)) = v {
                out.push((id.0 as u32, s.to_string()));
            }
        }
    }
    (tous, out, t.elapsed())
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(store_path) = args.next() else {
        eprintln!("usage: bench_trigrammes <STORE> [SEUIL]");
        std::process::exit(2);
    };
    let seuil: u32 = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(SEUIL_DEFAUT);

    let store = SubstrateStore::open(&store_path).expect("ouverture du store");
    println!("store : {store_path}");
    println!(
        "seuil : {seuil}{}",
        if seuil == SEUIL_DEFAUT {
            " (défaut)"
        } else {
            ""
        }
    );
    println!();

    for prop in ["name", "qualified_name"] {
        let (tous_ids, valeurs, t_collecte) = collecter(&store, prop);
        let cle = PropertyKey::new(prop);
        if valeurs.is_empty() {
            println!("-- {prop} : AUCUNE valeur, ignorée");
            continue;
        }
        let octets_valeurs: usize = valeurs.iter().map(|(_, v)| v.len()).sum();

        let t0 = Instant::now();
        let idx = PropsTrigramIndex::construire(&valeurs, seuil);
        let t_build = t0.elapsed();

        println!("══ {prop} ══════════════════════════════════════════════════════");
        println!(
            "  collecte      : {:.2} s ({:?}) pour {} valeurs, {:.1} Mo de texte",
            t_collecte.as_secs_f64(),
            t_collecte,
            valeurs.len(),
            octets_valeurs as f64 / 1e6
        );
        println!(
            "  construction  : {:.2} s — {} grammes indexés, {} postings, {:.1} Mo estimés \
             ({:.0} octets/valeur)",
            t_build.as_secs_f64(),
            idx.grammes(),
            idx.postings(),
            idx.memoire_octets() as f64 / 1e6,
            idx.memoire_octets() as f64 / valeurs.len() as f64
        );
        println!();
        println!(
            "  {:<14} {:>9} {:>11} {:>22} {:>12}   {}",
            "terme", "vraies", "candidats", "sur-ensemble ?", "recherche", "verdict"
        );

        for terme in TERMES {
            let vrais: Vec<u32> = valeurs
                .iter()
                .filter(|(_, v)| v.contains(terme))
                .map(|(s, _)| *s)
                .collect();

            // Trois tours, minimum retenu : la machine est partagée.
            let mut meilleur = Duration::from_secs(999);
            let mut candidats: Option<Vec<u32>> = None;
            for _ in 0..3 {
                let t = Instant::now();
                let c = idx.candidats(terme);
                meilleur = meilleur.min(t.elapsed());
                candidats = Some(c.unwrap_or_default());
            }
            let candidats = candidats.unwrap_or_default();
            let servi = idx.candidats(terme).is_some();

            let complet = vrais.iter().all(|s| candidats.binary_search(s).is_ok());
            let verdict = if !servi {
                "l'index se TAIT → balayage (juste, lent)".to_string()
            } else if !complet {
                "FAUX NÉGATIF — interdit".to_string()
            } else if candidats.is_empty() && vrais.is_empty() {
                "conclusif : zéro ligne, sans balayer".to_string()
            } else {
                format!(
                    "sur-coût {:.1}× les vraies",
                    candidats.len() as f64 / vrais.len().max(1) as f64
                )
            };

            println!(
                "  {:<14} {:>9} {:>11} {:>22} {:>10.1} µs   {}",
                terme,
                vrais.len(),
                if servi {
                    candidats.len().to_string()
                } else {
                    "—".to_string()
                },
                if complet { "OUI" } else { "NON" },
                meilleur.as_secs_f64() * 1e6,
                verdict
            );
        }
        println!();

        // ── LES DEUX CHEMINS COMPLETS, MESURÉS PAR LA MÊME PRIMITIVE ─────
        //
        // `verdicts_texte_batch` (GraphStore) rend un verdict par ligne SUR LES
        // OCTETS du substrat, sans allouer d'`ArcStr` : c'est à la fois le
        // PRÉDICAT du chemin balayé et la REVÉRIFICATION du chemin indexé.
        // Comparer les deux avec le même outil est la seule comparaison
        // honnête — et elle porte une vérification : le chemin indexé doit
        // rendre EXACTEMENT les mêmes lignes que le balayage.
        let sonde = PredicatTexte::Contient("iam");
        if store
            .verdicts_texte_batch(&tous_ids[..1.min(tous_ids.len())], &cle, &[sonde])
            .is_none()
        {
            println!("  (verdicts_texte_batch absent : comparaison des deux chemins ignorée)\n");
            continue;
        }
        println!(
            "  {:<14} {:>13} {:>16} {:>9} {:>14}   {}",
            "terme", "balayage", "index+revérif", "gain", "lignes", "verdict"
        );
        for terme in TERMES {
            let pred = PredicatTexte::Contient(terme);

            // 1. BALAYAGE : le verdict sur TOUTES les lignes de la propriété.
            let t = Instant::now();
            let par_balayage = store
                .verdicts_texte_batch(&tous_ids, &cle, &[pred])
                .unwrap_or_default();
            let t_balayage = t.elapsed();
            // Le canal rend un verdict PAR PRÉDICAT (une colonne) ; on n'en
            // demande qu'un, et on lit sa colonne.
            let n_balayage = par_balayage
                .first()
                .map_or(0, |col| col.iter().filter(|v| **v == Some(true)).count());

            // 2. INDEX : les candidats, puis le verdict sur eux SEULS.
            let t = Instant::now();
            let (n_index, n_cand) = match idx.candidats(terme) {
                None => (None, 0usize),
                Some(slots) => {
                    let ids: Vec<NodeId> =
                        slots.iter().map(|s| NodeId::new(u64::from(*s))).collect();
                    let verdicts = store
                        .verdicts_texte_batch(&ids, &cle, &[pred])
                        .unwrap_or_default();
                    (
                        Some(
                            verdicts
                                .first()
                                .map_or(0, |col| col.iter().filter(|v| **v == Some(true)).count()),
                        ),
                        ids.len(),
                    )
                }
            };
            let t_index = t.elapsed();

            let (lignes, verdict) = match n_index {
                None => (
                    format!("{n_balayage} (balayage)"),
                    "l'index se TAIT → on balaie".to_string(),
                ),
                Some(n) if n == n_balayage => {
                    (format!("{n} = {n_balayage}"), "MÊMES LIGNES ✓".to_string())
                }
                Some(n) => (
                    format!("{n} ≠ {n_balayage}"),
                    "DIVERGENCE — interdit".to_string(),
                ),
            };
            let gain = if n_index.is_some() {
                format!(
                    "{:.0}×",
                    t_balayage.as_secs_f64() / t_index.as_secs_f64().max(1e-9)
                )
            } else {
                "—".to_string()
            };
            println!(
                "  {:<14} {:>10.2} ms {:>13.3} ms {:>9} {:>14}   {}{}",
                terme,
                t_balayage.as_secs_f64() * 1e3,
                t_index.as_secs_f64() * 1e3,
                gain,
                lignes,
                verdict,
                if n_index.is_some() {
                    format!(" ({n_cand} candidats)")
                } else {
                    String::new()
                }
            );
        }
        println!();
    }

    // ── La borne de bout en bout — DEUX CHEMINS DISTINCTS, à ne pas confondre.
    //
    // C'est l'erreur que ce banc a servi à corriger dans mon propre
    // raisonnement : le coût fixe de ~17 ms (balayage des colonnes blob, qui
    // parcourt 347 970 créneaux pour 2 valeurs présentes) appartient au chemin
    // d'INTERVALLE (`find_nodes_in_range`, index ordonné). Une requête CONTAINS
    // servie par l'index de trigrammes ne passe PAS par ce chemin : elle ne paie
    // donc pas ce balayage, seulement ses candidats.
    println!("── borne projetée, chemin CONTAINS servi par l'index de trigrammes ──");
    println!("  revérification par lot (mesurée, après le correctif) : ~0,19 µs/candidat");
    println!("  aucun balayage de colonnes blob sur ce chemin (il appartient à l'intervalle)");
    println!();
    println!("  ⇒ `name CONTAINS 'iam'`   :   12 candidats → ~2 µs, contre 148–190 ms balayés");
    println!("  ⇒ `qn  CONTAINS 'iam'`    :  386 candidats → ~73 µs, contre 148–190 ms balayés");
    println!("  ⇒ `name CONTAINS 'token'` :  257 candidats → ~49 µs, contre 148–190 ms balayés");
    println!();
    println!(
        "  Ce qu'il reste à payer à ce moment-là n'est plus la donnée mais le PLAN :
    le coût d'ouverture de l'index et le passage des candidats dans l'opérateur de"
    );
    println!("  filtre. C'est ce que mesurera le branchement — pas ce banc.");
    println!();
    println!("── et pour le chemin d'INTERVALLE (index ordonné), le coût fixe reste ──");
    println!("  ~17 ms de balayage blob + ~0,19 µs/candidat : servir `>= 'zzz'` coûte");
    println!("  18,3 ms mesurés, dont 17 ms de balayage pour 2 valeurs présentes.");
    println!("  Sous ~17 ms, c'est CE balayage qu'il faut supprimer — pas l'index");
    println!("  qu'il faut affiner. Les deux leviers sont mesurés séparément.");
}
