//! `EXISTS { MATCH (n)<-[:CONTAINS]-(:File)<-[:CONTAINS]-(p) }` quand `n` et `p` sont
//! DÉJÀ liés et que le nœud intermédiaire est ÉTIQUETÉ (obrain#360).
//!
//! Défaut : après #345, la forme sans intermédiaire (`*1..2`) était réparée mais pas
//! celle-ci, dont le coût restait proportionnel au graphe (4,98 / 20,2 / 90,1 ms pour
//! 2 000 / 8 000 / 32 000 fichiers). Le traducteur ne chaîne pas de `NodeScan`
//! étiqueté : il pose un `Filter(hasLabel)` sur la colonne vertébrale. La cause est
//! l'unicité des relations (#182), un `Filter(_anon_0 <> _anon_1)` qui lit les
//! variables d'ARÊTE du motif, que la réécriture ne comptait pas comme produites.
//!
//! Deux vérifications distinctes : la RÉPONSE reste celle de la formulation par
//! `MATCH` ; le TRAVAIL (lignes PROFILE, jamais des millisecondes) ne croît plus avec
//! le graphe.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test exists_intermediaire_etiquete_360
//! ```

#![cfg(feature = "cypher")]

use obrain_common::types::{NodeId, Value};
use obrain_engine::ObrainDB;
use std::collections::BTreeSet;

fn texte(valeur: &Value) -> String {
    match valeur {
        Value::String(s) => s.to_string(),
        autre => format!("{autre:?}"),
    }
}

fn ensemble(db: &ObrainDB, requete: &str) -> BTreeSet<String> {
    let resultat = db
        .session()
        .execute_cypher(requete)
        .unwrap_or_else(|e| panic!("requête refusée : {requete}\n{e}"));
    resultat
        .rows
        .iter()
        .filter_map(|ligne| ligne.first().map(texte))
        .collect()
}

fn noeud(db: &ObrainDB, label: &str, cle: &str) -> NodeId {
    db.session()
        .create_node_with_props(&[label], [("k", Value::String(cle.into()))])
}

fn arete(db: &ObrainDB, de: NodeId, vers: NodeId, type_: &str) {
    db.session().create_edge(de, vers, type_);
}

struct Lcg(u64);
impl Lcg {
    fn suivant(&mut self, borne: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % borne
    }
}

/// 4 projets, 60 fichiers (certains orphelins, certains dans deux projets), 2 fonctions
/// par fichier, un dossier `Dir` entre projet et quelques fichiers (intermédiaire d'une
/// AUTRE étiquette), IMPORTS aléatoires.
fn graphe_aleatoire(graine: u64) -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let mut alea = Lcg(graine);
    let projets: Vec<_> = (0..4)
        .map(|i| noeud(&db, "Project", &format!("P{i}")))
        .collect();
    for i in 0..60 {
        let f = noeud(&db, "File", &format!("F{i:02}"));
        let conteneur = if alea.suivant(5) == 0 {
            let d = noeud(&db, "Dir", &format!("D{i:02}"));
            arete(&db, projets[alea.suivant(4) as usize], d, "CONTAINS");
            d
        } else {
            projets[alea.suivant(4) as usize]
        };
        if alea.suivant(6) != 0 {
            arete(&db, conteneur, f, "CONTAINS");
        }
        if alea.suivant(10) == 0 {
            arete(&db, projets[alea.suivant(4) as usize], f, "CONTAINS");
        }
        for j in 0..2 {
            let g = noeud(&db, "Function", &format!("F{i:02}_fn{j}"));
            arete(&db, f, g, "CONTAINS");
        }
    }
    db
}

#[test]
fn exists_a_intermediaire_etiquete_donne_le_meme_ensemble_que_la_formulation_par_match() {
    for graine in [1, 7, 42, 2026] {
        let db = graphe_aleatoire(graine);
        for projet in 0..4 {
            for etiquette in ["File", "Dir", "Function"] {
                let avec_exists = ensemble(
                    &db,
                    &format!(
                        "MATCH (p:Project {{k: 'P{projet}'}}) MATCH (n:Function) \
                         WHERE EXISTS {{ MATCH (n)<-[:CONTAINS]-(:{etiquette})<-[:CONTAINS]-(p) }} \
                         RETURN n.k"
                    ),
                );
                let avec_match = ensemble(
                    &db,
                    &format!(
                        "MATCH (p:Project {{k: 'P{projet}'}})-[:CONTAINS]->(:{etiquette})\
                         -[:CONTAINS]->(n:Function) RETURN DISTINCT n.k"
                    ),
                );
                assert_eq!(
                    avec_exists, avec_match,
                    "graine {graine}, projet P{projet}, intermédiaire :{etiquette}"
                );
            }
        }
    }
}

#[test]
fn intermediaire_avec_propriete_et_not_exists() {
    let db = graphe_aleatoire(5);
    for projet in 0..4 {
        let tous = ensemble(&db, "MATCH (n:Function) RETURN n.k");
        let dedans = ensemble(
            &db,
            &format!(
                "MATCH (p:Project {{k: 'P{projet}'}}) MATCH (n:Function) \
                 WHERE EXISTS {{ MATCH (n)<-[:CONTAINS]-(f:File)<-[:CONTAINS]-(p) \
                                 WHERE f.k <> 'F00' }} RETURN n.k"
            ),
        );
        let attendu = ensemble(
            &db,
            &format!(
                "MATCH (p:Project {{k: 'P{projet}'}})-[:CONTAINS]->(f:File)-[:CONTAINS]->(n:Function) \
                 WHERE f.k <> 'F00' RETURN DISTINCT n.k"
            ),
        );
        assert_eq!(dedans, attendu, "projet P{projet}");
        let dehors = ensemble(
            &db,
            &format!(
                "MATCH (p:Project {{k: 'P{projet}'}}) MATCH (n:Function) \
                 WHERE NOT EXISTS {{ MATCH (n)<-[:CONTAINS]-(f:File)<-[:CONTAINS]-(p) \
                                     WHERE f.k <> 'F00' }} RETURN n.k"
            ),
        );
        assert!(dedans.is_disjoint(&dehors), "projet P{projet}");
        let union: BTreeSet<String> = dedans.union(&dehors).cloned().collect();
        assert_eq!(union, tous, "projet P{projet}");
    }
}

/// L'unicité des relations (#182) est ce que le filtre sur les variables d'arête porte :
/// avec UNE seule arête a-b, `(a)-[]-(:X)-[]-(a')` où a' est le même nœud que a ne doit
/// PAS être satisfait en réutilisant l'arête. La réécriture doit garder ce filtre.
#[test]
fn l_unicite_des_relations_est_conservee_par_la_reecriture() {
    let db = ObrainDB::new_in_memory();
    let a = noeud(&db, "A", "a");
    let b = noeud(&db, "B", "b");
    arete(&db, a, b, "R");
    let avec_exists = ensemble(
        &db,
        "MATCH (n:A) MATCH (p:A) WHERE EXISTS { MATCH (n)-[:R]-(:B)-[:R]-(p) } RETURN n.k",
    );
    let avec_match = ensemble(&db, "MATCH (n:A)-[:R]-(:B)-[:R]-(p:A) RETURN DISTINCT n.k");
    assert_eq!(avec_exists, avec_match);
    assert!(avec_exists.is_empty(), "{avec_exists:?}");
    // Avec une seconde arête, la même forme devient vraie : le test n'est pas vide de sens.
    arete(&db, a, b, "R");
    let avec_exists = ensemble(
        &db,
        "MATCH (n:A) MATCH (p:A) WHERE EXISTS { MATCH (n)-[:R]-(:B)-[:R]-(p) } RETURN n.k",
    );
    let avec_match = ensemble(&db, "MATCH (n:A)-[:R]-(:B)-[:R]-(p:A) RETURN DISTINCT n.k");
    assert_eq!(avec_exists, avec_match);
    assert_eq!(avec_exists.len(), 1);
}

/// Plus grand `rows=` rapporté par PROFILE, tous opérateurs confondus.
fn plus_grand_nombre_de_lignes(db: &ObrainDB, requete: &str) -> u64 {
    let resultat = db
        .session()
        .execute_cypher(&format!("PROFILE {requete}"))
        .expect("PROFILE");
    let mut maxi = 0u64;
    for ligne in &resultat.rows {
        for valeur in ligne {
            if let Value::String(t) = valeur {
                for morceau in t.split("rows=").skip(1) {
                    let chiffres: String =
                        morceau.chars().take_while(|c| c.is_ascii_digit()).collect();
                    if let Ok(n) = chiffres.parse::<u64>() {
                        maxi = maxi.max(n);
                    }
                }
            }
        }
    }
    maxi
}

#[test]
fn le_travail_ne_depend_plus_de_la_taille_du_graphe() {
    // Forme B de `mesure_exists_formes` : projet ⊃ fichiers ⊃ fonctions, CALLS entre
    // fonctions. Le résultat (3 appelants) et le voisinage sont constants ; seule la
    // taille du graphe change.
    let requete = "MATCH (p:Project {id: 'p1'}) MATCH (f:Function {name: 'fn0_0'}) \
        MATCH (caller:Function)-[:CALLS]->(f) \
        WHERE EXISTS { MATCH (caller)<-[:CONTAINS]-(:File)<-[:CONTAINS]-(p) } \
        RETURN count(caller) AS c";
    let mut mesures = Vec::new();
    for fichiers in [300u64, 3_000] {
        let db = ObrainDB::new_in_memory();
        let session = db.session();
        let projet =
            session.create_node_with_props(&["Project"], [("id", Value::String("p1".into()))]);
        let mut fonctions = Vec::new();
        for i in 0..fichiers {
            let f = session
                .create_node_with_props(&["File"], [("k", Value::String(format!("f{i}").into()))]);
            session.create_edge(projet, f, "CONTAINS");
            for j in 0..3 {
                let g = session.create_node_with_props(
                    &["Function"],
                    [("name", Value::String(format!("fn{i}_{j}").into()))],
                );
                session.create_edge(f, g, "CONTAINS");
                fonctions.push(g);
            }
        }
        // 3 appelants de fn0_0 (fonctions 3, 4, 5).
        for appelant in [3usize, 4, 5] {
            session.create_edge(fonctions[appelant], fonctions[0], "CALLS");
        }
        let resultat = session.execute_cypher(requete).expect("requête");
        assert_eq!(
            resultat.rows.first().and_then(|l| l.first()),
            Some(&Value::Int64(3)),
            "{fichiers} fichiers"
        );
        mesures.push(plus_grand_nombre_de_lignes(&db, requete));
    }
    // Avant le correctif, le sous-motif était énuméré pour tout le graphe : le plus grand
    // opérateur traitait des milliers de lignes et croissait avec lui. Désormais il reste
    // borné par le voisinage de `caller`.
    assert!(
        mesures.iter().all(|&m| m <= 50),
        "un opérateur traite encore {mesures:?} lignes : le sous-motif est de nouveau énuméré pour tout le graphe"
    );
}

/// Deux sauts consécutifs SANS nœud filtré entre eux sont planifiés en chaîne
/// factorisée, qui ne se rejoue pas : la réécriture doit s'en abstenir et la réponse
/// rester celle de la formulation par `MATCH`.
#[test]
fn intermediaire_non_etiquete_garde_la_reponse_de_la_formulation_par_match() {
    for graine in [3, 11] {
        let db = graphe_aleatoire(graine);
        for projet in 0..4 {
            let avec_exists = ensemble(
                &db,
                &format!(
                    "MATCH (p:Project {{k: 'P{projet}'}}) MATCH (n) \
                     WHERE (n:File OR n:Function) \
                       AND EXISTS {{ MATCH (n)<-[:CONTAINS]-(m)<-[:CONTAINS]-(p) }} RETURN n.k"
                ),
            );
            let avec_match = ensemble(
                &db,
                &format!(
                    "MATCH (p:Project {{k: 'P{projet}'}})-[:CONTAINS]->(m)-[:CONTAINS]->(n) \
                     WHERE n:File OR n:Function RETURN DISTINCT n.k"
                ),
            );
            assert_eq!(avec_exists, avec_match, "graine {graine}, projet P{projet}");
        }
    }
}
