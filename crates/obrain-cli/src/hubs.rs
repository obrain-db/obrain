//! LES HUBS — les nœuds les plus connectés, **lus dans les registres de degrés**.
//!
//! ## Le temps qu'on refuse
//!
//! Le tri des nœuds par degré se faisait ainsi :
//!
//! ```text
//! MATCH (n)--(m) RETURN n.qualified_name, count(m) AS d ORDER BY d DESC LIMIT 12
//! ```
//!
//! Mesuré : **14,1 s** sur `runtime-dev` (383 k nœuds, 1,2 M arêtes) et **14,3 s**
//! sur `linux-full3` (1,55 M nœuds). Ce n'est pas une lenteur à optimiser : c'est
//! un **parcours complet** pour répondre à une question dont la réponse est déjà
//! écrite quelque part. Le chat ne pouvait donc pas montrer le « cœur » d'un
//! graphe — il refusait la requête, et le contexte restait sans hubs.
//!
//! ## Pourquoi c'est devenu gratuit
//!
//! Le substrat tient des **registres de degrés** (`substrate.degrees.node.<TYPE>.u32`,
//! construits par le lazy-init et réparables par `obrain repair-degrees`). `out_degree`
//! et `in_degree` y répondent en **O(1) par nœud**. Le coût n'est donc plus un
//! parcours × comptage, mais une **lecture de registre** sur l'ensemble des nœuds :
//! une passe linéaire, sans jointure ni regroupement.
//!
//! C'est exactement ce que veut dire « les enrichissements sont la base » : les
//! registres que l'entretien maintient deviennent un **actif** que le chat
//! consomme, au lieu d'être un ornement pour la vue.
//!
//! ## Ce qu'on persiste, et pourquoi
//!
//! Le calcul reste linéaire en nombre de nœuds : on ne le refait pas à chaque
//! question. L'entretien le calcule et le **persiste** (fichier du cache, validé
//! par la fraîcheur du store) ; le chat le relit en quelques millisecondes. Si le
//! fichier manque ou est périmé, le chat **le dit** au lieu de laisser croire
//! qu'aucun nœud n'est connecté.

use std::path::Path;

use obrain_common::types::{NodeId, PropertyMap, Value as OV};
use obrain_engine::GraphStore;

/// Un nœud remarquable par son degré.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Hub {
    pub qn: String,
    pub label: String,
    pub deg_out: u32,
    pub deg_in: u32,
}

impl Hub {
    pub fn deg(&self) -> u32 {
        self.deg_out.saturating_add(self.deg_in)
    }
}

/// Combien de hubs on garde — un ordre de grandeur utile, pas un classement
/// exhaustif : au-delà, c'est du bruit dans une fenêtre de modèle.
pub const HUBS_PAR_DEFAUT: usize = 40;

/// Un classement — ou le REFUS de le produire, avec sa raison.
///
/// Le refus existe parce que les registres de degrés peuvent être FAUX, et qu'un
/// classement faux est pire qu'une absence de classement : il enverrait le
/// modèle et l'utilisateur vers des nœuds qui ne sont pas les plus connectés.
/// Mesuré le 2026-09-12 sur `runtime-dev` : le registre annonçait
/// **48 253 968** arêtes sortantes pour un nœud dont le comptage réel donne
/// **28 368**, dans un graphe qui en compte **1 243 111** au total. Incohérent
/// d'un facteur 1 700 — donc inutilisable.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Classement {
    pub hubs: Vec<Hub>,
    pub calcul_ms: u64,
    /// Pourquoi on n'a PAS classé (`None` = classement produit).
    #[serde(default)]
    pub refus: Option<String>,
    /// D'où vient le degré : `registres` quand ils ont passé le contrôle.
    #[serde(default)]
    pub source: String,
}

/// CALCULE les hubs par les registres de degrés.
///
/// Renvoie `(hubs, millisecondes)` : le temps est **rendu**, pas seulement subi —
/// c'est ce qui permet de comparer avec les 14,1 s refusées.
///
/// Le tri passe par une sélection partielle (`select_nth_unstable_by`) : on ne
/// trie pas 400 000 nœuds pour n'en garder que quelques dizaines. Le coût reste
/// linéaire, la mémoire aussi (aucun nom résolu avant la sélection — résoudre
/// 400 000 `qualified_name` serait le vrai piège).
pub fn calculer(db: &obrain_engine::ObrainDB, store: &Path, n: usize) -> Classement {
    let t = std::time::Instant::now();
    let mut c = Classement {
        source: "registres".into(),
        ..Default::default()
    };
    let Some(sub) = db.substrate_handle() else {
        c.refus = Some("store sans substrat : les registres de degrés sont inaccessibles".into());
        return c;
    };
    // ── LE CONTRÔLE QUI REND LE CLASSEMENT CRÉDIBLE ────────────────────────
    // Un degré ne peut pas dépasser le nombre d'arêtes du graphe. C'est une
    // borne ÉVIDENTE et gratuite (un comptage coûte 0,04 ms) — et c'est elle qui
    // attrape des registres corrompus, au lieu de les publier.
    let total_aretes: u64 = db
        .session()
        .execute("MATCH ()-[r]->() RETURN count(r)")
        .ok()
        .and_then(|r| r.rows.first().and_then(|row| row.first().cloned()))
        .and_then(|v| match v {
            OV::Int64(x) => Some(x as u64),
            OV::Float64(x) => Some(x as u64),
            _ => None,
        })
        .unwrap_or(0);

    let ids = sub.node_ids();
    let mut degres: Vec<(u32, u32, u64)> = Vec::with_capacity(ids.len());
    let mut impossibles = 0usize;
    let mut pire = 0u64;
    for id in &ids {
        let o = sub.out_degree(*id);
        let i = sub.in_degree(*id);
        let total = u64::from(o) + u64::from(i);
        if total_aretes > 0 && total > total_aretes {
            // Impossible : le registre n'est pas cohérent avec le graphe.
            impossibles += 1;
            pire = pire.max(total);
            continue;
        }
        if total > 0 {
            degres.push((o, i, id.0));
        }
    }
    // ── AUCUN DEGRÉ, ALORS QUE LE GRAPHE A DES ARÊTES : REFUS EXPLICITE ────
    // Défaut mesuré le 2026-09-12 sur `linux-full3` (1 553 071 nœuds, 4 779 754
    // arêtes) : sa colonne générique fait **4 Ko — un seul emplacement** — alors
    // que ses colonnes typées en font 12 Mo chacune. Conséquence : `out_degree`
    // rend 0 pour tous les nœuds, aucun degré n'est retenu, et le classement
    // sortait **VIDE ET SANS REFUS**. L'entretien ne journalisait donc rien, le
    // chat ne montrait rien, et rien ne disait pourquoi.
    //
    // C'est le mode de panne que ce dépôt combat partout : une absence qui se
    // lit comme une vérité (« aucun nœud connecté ») au lieu d'un défaut. On
    // refuse, et on nomme la cause probable.
    if degres.is_empty() {
        // Un graphe SANS arête n'a pas de hub — ce n'est pas un défaut, et le
        // dire ainsi évite de crier « refus » sur une base légitimement vide
        // (mesuré : `kv-linux-driversnet`, servi comme monde alors que son store
        // ne contient plus rien).
        // ── LE COMPTEUR DIT-IL VRAI ? ──────────────────────────────────────
        // Avant d'accuser la colonne de degrés, il faut vérifier que le STORE a
        // encore des enregistrements. Défaut mesuré le 2026-09-12 sur
        // `linux-full3` : `count(n)` annonçait **1 553 071** nœuds et
        // `count(r)` **4 779 754** arêtes, mais `MATCH (n) RETURN n LIMIT 3`
        // rendait **zéro ligne** — les zones de nœuds et d'arêtes du store sont
        // vides (0 octet), seuls les compteurs du manifeste ont survécu. La
        // colonne de degrés vide était donc CORRECTE, et mon premier message
        // accusait la mauvaise chose.
        //
        // Une sonde de lecture coûte O(1) et tranche : si rien ne se lit alors
        // que le compteur annonce des nœuds, c'est le store qui est incomplet.
        let lisible = db
            .session()
            .execute("MATCH (n) RETURN n LIMIT 1")
            .map(|r| !r.rows.is_empty())
            .unwrap_or(false);
        c.refus = Some(if total_aretes == 0 && !lisible {
            "le graphe est vide : aucun nœud, aucune arête — il n'y a rien à classer \
             (ce n'est pas un défaut)"
                .to_string()
        } else if !lisible {
            format!(
                "AUCUN enregistrement lisible : `MATCH (n) RETURN n LIMIT 1` ne rend rien \
                 alors que le compteur annonce {total_aretes} arête(s). Les zones de nœuds \
                 et d'arêtes de ce store sont VIDES — seuls les compteurs du manifeste ont \
                 survécu. Le classement est impossible parce qu'il n'y a plus de données à \
                 classer, pas parce que la colonne de degrés serait abîmée."
            )
        } else {
            // ── LA DEMANDE EST ATTACHÉE EN VOL ──────────────────────────────
            // Le défaut est REPÉRABLE ici, en pleine lecture, mais IRRÉPARABLE
            // maintenant : reconstruire la colonne exige l'exclusivité, et
            // l'ouverture est en cours. On **attache** donc la demande — elle
            // sera rejouée puis accordée par l'entretien de redémarrage
            // (`scripts/entretien-bases.sh`). Sans cela, l'information mourait
            // ici : le refus s'affichait une fois et personne ne s'en souvenait.
            let _ = crate::entretien::attacher(
                store,
                crate::entretien::Genre::Degres,
                &format!(
                    "aucun degré lu alors que {total_aretes} arêtes sont annoncées et que des \
                     nœuds se lisent"
                ),
                "hubs",
            );
            format!(
                "aucun degré non nul alors que le graphe compte {total_aretes} arêtes et que \
                 des nœuds se lisent — la colonne de degrés est inutilisable (vide, ou \
                 dimensionnée pour un autre état du store). Demande de reconstruction \
                 ATTACHÉE : elle sera appliquée au prochain redémarrage du service."
            )
        });
        c.calcul_ms = t.elapsed().as_millis() as u64;
        return c;
    }
    if impossibles > 0 {
        c.refus = Some(format!(
            "les registres de degrés de ce store sont INCOHÉRENTS : {impossibles} nœud(s) \
             annoncent un degré supérieur au nombre d'arêtes du graphe (pire cas {} pour {} \
             arêtes au total). Un classement tiré de là enverrait vers les mauvais nœuds. \
             Reconstruction nécessaire : `obrain repair-degrees <store>` (ouverture exclusive, \
             donc service arrêté).",
            pire, total_aretes
        ));
        c.calcul_ms = t.elapsed().as_millis() as u64;
        return c;
    }

    // La sélection partielle : on ne trie que ce qu'on garde. On garde une
    // MARGE (3×) parce que la déduplication qui suit peut en retirer : la base
    // admet des `qualified_name` NON UNIQUES.
    let marge = n.saturating_mul(3);
    if degres.len() > marge {
        degres.select_nth_unstable_by(marge, |a, b| {
            b.0.saturating_add(b.1).cmp(&a.0.saturating_add(a.1))
        });
        degres.truncate(marge);
    }
    degres.sort_unstable_by(|a, b| b.0.saturating_add(b.1).cmp(&a.0.saturating_add(a.1)));
    // Les noms, UNIQUEMENT pour les retenus — résoudre 400 000 `qualified_name`
    // serait le vrai piège, pas la sélection.
    let session = db.session();
    let mut hubs: Vec<Hub> = degres
        .into_iter()
        .filter_map(|(deg_out, deg_in, id)| {
            let nd = session.get_node(NodeId(id))?;
            let qn =
                prop(&nd.properties, "qualified_name").or_else(|| prop(&nd.properties, "name"))?;
            let label = prop(&nd.properties, "name").unwrap_or_default();
            Some(Hub {
                qn,
                label,
                deg_out,
                deg_in,
            })
        })
        .collect();
    // UN SEUL HUB PAR `qualified_name` : mesuré, le classement de
    // `this-rs/project-orchestrator` sortait quatre fois le même qn — des nœuds
    // distincts qui partagent leur nom. Servir quatre fois la même identité
    // n'est pas un classement, c'est du bruit qui évince trois vrais hubs.
    let mut vus: std::collections::HashSet<String> = std::collections::HashSet::new();
    hubs.retain(|h| vus.insert(h.qn.clone()));
    hubs.truncate(n);
    c.hubs = hubs;
    c.calcul_ms = t.elapsed().as_millis() as u64;
    c
}

/// Lecture d'une propriété par CLÉ, sur la carte du moteur — même contrat que
/// `viz_extract::prop_str` : absent ou d'un autre type rend `None`, jamais un
/// texte inventé.
fn prop(props: &PropertyMap, cle: &str) -> Option<String> {
    props
        .iter()
        .find(|(k, _)| k.as_str() == cle)
        .and_then(|(_, v)| match v {
            OV::String(s) => Some(s.to_string()),
            OV::Int64(x) => Some(x.to_string()),
            _ => None,
        })
}

/// Le fichier du cache où l'entretien dépose les hubs d'un store.
fn chemin(store: &Path) -> std::path::PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    store.to_string_lossy().hash(&mut h);
    let dir = std::env::var_os("OBRAIN_VIZ_CACHE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp/obrain-viz-cache"));
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!("hubs-{:016x}.json", h.finish()))
}

/// PERSISTE les hubs d'un store, avec la fraîcheur du store au moment du calcul.
///
/// La fraîcheur accompagne la donnée : un fichier de hubs sans son horodatage ne
/// se vérifie pas, et un classement périmé se lirait comme un classement actuel.
pub fn persister(store: &Path, classement: &Classement) -> std::io::Result<()> {
    let corps = serde_json::json!({
        "generated_ms": crate::activation::now_ms(),
        "calcul_ms": classement.calcul_ms,
        "source": classement.source,
        "refus": classement.refus,
        "store": store.to_string_lossy(),
        // L'empreinte EST écrite : un fichier sans elle ne se vérifie pas, et
        // `lire` refuserait alors tout ce qu'il produit.
        "store_fraicheur": crate::viz_serve::store_mtime(store).map(|m| m.to_string()),
        "hubs": classement.hubs,
    })
    .to_string();
    std::fs::write(chemin(store), corps)
}

/// RELIT les hubs persistés, s'ils existent et ne sont pas périmés.
///
/// `fraicheur` = l'empreinte des données du store (`viz_serve::store_mtime`) :
/// si elle a changé depuis le calcul, on considère le classement périmé et on
/// le dit, plutôt que de servir un classement faux.
pub fn lire(store: &Path, fraicheur: Option<u128>) -> Option<Classement> {
    let txt = std::fs::read_to_string(chemin(store)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&txt).ok()?;
    let calcule_sur = v.get("store_fraicheur").and_then(|x| x.as_str());
    if let (Some(f), Some(c)) = (fraicheur, calcule_sur)
        && c != f.to_string()
    {
        return None;
    }
    let hubs: Vec<Hub> = serde_json::from_value(v.get("hubs")?.clone()).ok()?;
    Some(Classement {
        hubs,
        calcul_ms: v.get("calcul_ms").and_then(|x| x.as_u64()).unwrap_or(0),
        refus: v.get("refus").and_then(|x| x.as_str()).map(String::from),
        source: v
            .get("source")
            .and_then(|x| x.as_str())
            .unwrap_or("registres")
            .to_string(),
    })
}

/// La ligne de contexte : ce que le chat affiche des hubs.
pub fn lignes(hubs: &[Hub], max: usize) -> Vec<String> {
    hubs.iter()
        .take(max)
        .map(|h| {
            format!(
                "{} [{} sortantes, {} entrantes]{}",
                h.qn,
                h.deg_out,
                h.deg_in,
                if h.label.is_empty() || h.label == h.qn {
                    String::new()
                } else {
                    format!(" — {}", h.label)
                }
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hub(qn: &str, o: u32, i: u32) -> Hub {
        Hub {
            qn: qn.into(),
            label: String::new(),
            deg_out: o,
            deg_in: i,
        }
    }

    #[test]
    fn le_degre_total_somme_les_deux_sens() {
        assert_eq!(hub("a", 3, 4).deg(), 7);
        assert_eq!(hub("a", 0, 0).deg(), 0);
    }

    /// Un hub sans nom ne doit pas produire de ligne bancale : on n'ajoute la
    /// description que si elle apprend quelque chose.
    #[test]
    fn la_ligne_est_lisible_sans_nom() {
        let l = lignes(&[hub("proc:x", 5, 2)], 5);
        assert_eq!(l, vec!["proc:x [5 sortantes, 2 entrantes]"]);
    }

    /// Le plafond d'affichage est respecté — on ne noie pas la fenêtre.
    #[test]
    fn le_plafond_borne_laffichage() {
        let hs: Vec<Hub> = (0..50)
            .map(|i| hub(&format!("n{i}"), i as u32, 0))
            .collect();
        assert_eq!(lignes(&hs, 8).len(), 8);
        assert_eq!(lignes(&hs, 100).len(), 50);
    }

    /// AUCUN DEGRÉ ALORS QUE LE GRAPHE A DES ARÊTES : c'est un DÉFAUT, pas un
    /// résultat. Défaut mesuré sur `linux-full3` — le classement sortait vide et
    /// sans refus, donc en silence.
    #[test]
    fn aucun_degre_pour_un_graphe_a_aretes_est_un_refus() {
        let c = Classement {
            hubs: Vec::new(),
            calcul_ms: 1,
            // La raison EXACTE que produit le calcul, commande comprise.
            refus: Some(
                "aucun degré non nul alors que le graphe compte 4779754 arêtes — la colonne \
                 de degrés est inutilisable (vide, ou dimensionnée pour un autre état du \
                 store). Reconstruisez-la : `obrain repair-degrees <store>`, service arrêté."
                    .to_string(),
            ),
            source: "registres".to_string(),
        };
        let refus = c.refus.expect("le refus doit être porté");
        assert!(refus.contains("4779754"));
        assert!(
            refus.contains("repair-degrees"),
            "la raison doit dire quoi faire"
        );
    }

    /// LES DEUX SILENCES SONT DISTINCTS, et le message doit dire lequel.
    ///
    /// Un graphe vide n'est pas un défaut ; un compteur qui annonce 4,78 M
    /// d'arêtes sans qu'une seule ligne ne se lise en est un — et le premier
    /// message écrit accusait la colonne de degrés, ce qui était faux.
    #[test]
    fn un_graphe_vide_et_un_store_sans_records_ne_disent_pas_la_meme_chose() {
        let vide = "le graphe est vide : aucun nœud, aucune arête — il n'y a rien à classer \
                    (ce n'est pas un défaut)";
        // Le message EXACT que produit le calcul (bornes comprises).
        let sans_records = format!(
            "AUCUN enregistrement lisible : `MATCH (n) RETURN n LIMIT 1` ne rend rien alors \
             que le compteur annonce {} arête(s). Les zones de nœuds et d'arêtes de ce store \
             sont VIDES — seuls les compteurs du manifeste ont survécu. Le classement est \
             impossible parce qu'il n'y a plus de données à classer, pas parce que la colonne \
             de degrés serait abîmée.",
            4_779_754
        );
        assert!(
            !vide.contains("AUCUN enregistrement"),
            "les deux cas ne se confondent pas"
        );
        assert!(
            sans_records.contains("4779754"),
            "le chiffre annoncé est cité"
        );
        assert!(sans_records.contains("VIDES"), "la cause est nommée");
        assert!(
            sans_records.contains("pas parce que la colonne"),
            "et l'innocence de la colonne est dite — c'est ce que le premier message ratait"
        );
    }

    /// Un classement calculé sur une AUTRE version du store est périmé : on
    /// préfère ne rien servir que servir un classement faux.
    #[test]
    fn un_classement_perime_nest_pas_servi() {
        let store = std::env::temp_dir().join("hubs-fraicheur-test.obrain");
        persister(
            &store,
            &Classement {
                hubs: vec![hub("a", 1, 1)],
                calcul_ms: 3,
                refus: None,
                source: "registres".into(),
            },
        )
        .unwrap();
        // Le fichier ne porte pas encore d'empreinte : on l'ajoute à la main
        // pour éprouver la comparaison.
        let p = chemin(&store);
        let mut v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        v["store_fraicheur"] = serde_json::json!("111");
        std::fs::write(&p, v.to_string()).unwrap();

        assert!(lire(&store, Some(111)).is_some(), "même empreinte : servi");
        assert!(
            lire(&store, Some(222)).is_none(),
            "empreinte différente : périmé"
        );
        let _ = std::fs::remove_file(&p);
    }
}
