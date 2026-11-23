//! `EXISTS { MATCH (n)<-[:CONTAINS*1..2]-(p) }` quand `n` et `p` sont DÉJÀ liés par le
//! motif externe (obrain#189 : `code_45`, `code_40`, `code_24` de Project Orchestrator).
//!
//! Défaut : ce `EXISTS` passait par la semi-jointure par hachage, qui plan le
//! sous-motif comme non corrélé et l'énumère pour TOUT le graphe avant de joindre
//! sur `(n, p)`. Coût du graphe, pas du voisinage : mesuré 44 → 156 → 641 ms pour
//! 2 000 → 8 000 → 32 000 fichiers, résultat constant, alors que la traversée
//! seule coûte 0,1 ms.
//!
//! Le correctif réécrit le sous-motif en sous-requête corrélée. Ce fichier vérifie
//! deux choses distinctes :
//!   1. la RÉPONSE n'a pas changé (cas écrits à la main, puis comparaison avec la
//!      formulation équivalente par `MATCH`, qui n'emprunte pas ce chemin) ;
//!   2. le TRAVAIL ne dépend plus de la taille du graphe — mesuré en lignes traitées
//!      par opérateur (PROFILE), jamais en millisecondes : un seuil de durée serait
//!      le flaky qu'on vient de passer deux jours à traquer.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test exists_extremite_liee_189
//! ```

#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use std::collections::BTreeSet;

fn texte(valeur: &Value) -> String {
    match valeur {
        Value::String(s) => s.to_string(),
        autre => format!("{autre:?}"),
    }
}

/// Première colonne de chaque ligne, triée et dédoublonnée.
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

fn noeud(db: &ObrainDB, label: &str, cle: &str) -> obrain_common::types::NodeId {
    db.session()
        .create_node_with_props(&[label], [("k", Value::String(cle.into()))])
}

fn arete(
    db: &ObrainDB,
    de: obrain_common::types::NodeId,
    vers: obrain_common::types::NodeId,
    type_: &str,
) {
    db.session().create_edge(de, vers, type_);
}

/// P1 ⊃ F1 ⊃ Fn1 ; P1 ⊃ F2 ; P2 ⊃ F3 ⊃ Fn3 ; F4 sans projet. IMPORTS : F1→F2, F1→F3.
fn graphe_main() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let p1 = noeud(&db, "Project", "P1");
    let p2 = noeud(&db, "Project", "P2");
    let f1 = noeud(&db, "File", "F1");
    let f2 = noeud(&db, "File", "F2");
    let f3 = noeud(&db, "File", "F3");
    let _f4 = noeud(&db, "File", "F4");
    let fn1 = noeud(&db, "Function", "Fn1");
    let fn3 = noeud(&db, "Function", "Fn3");
    arete(&db, p1, f1, "CONTAINS");
    arete(&db, p1, f2, "CONTAINS");
    arete(&db, p2, f3, "CONTAINS");
    arete(&db, f1, fn1, "CONTAINS");
    arete(&db, f3, fn3, "CONTAINS");
    arete(&db, f1, f2, "IMPORTS");
    arete(&db, f1, f3, "IMPORTS");
    db
}

#[test]
fn les_noeuds_contenus_dans_le_projet_lie_sont_gardes_et_les_autres_ecartes() {
    let db = graphe_main();
    // Voisins IMPORTS de F1 gardés s'ils sont contenus (1..2 sauts) dans P1 :
    // F2 oui ; F3 appartient à P2, donc non.
    let gardes = ensemble(
        &db,
        "MATCH (p:Project {k: 'P1'}) MATCH (s:File {k: 'F1'}) \
         CALL { WITH s, p MATCH (s)-[:IMPORTS]->(n) \
                WHERE EXISTS { MATCH (n)<-[:CONTAINS*1..2]-(p) } RETURN n.k AS k } \
         RETURN k",
    );
    assert_eq!(gardes, BTreeSet::from(["F2".to_string()]));
}

#[test]
fn l_extremite_liee_compte_une_arete_vers_un_autre_projet_ne_suffit_pas() {
    let db = graphe_main();
    // F3 a bien une arête CONTAINS entrante (celle de P2) ; vers P1, non. C'est le
    // bug #95-2 : « une arête existe » n'est pas « une arête vers CE nœud ».
    let vers_p1 = ensemble(
        &db,
        "MATCH (p:Project {k: 'P1'}) MATCH (n:File) \
         WHERE EXISTS { MATCH (n)<-[:CONTAINS]-(p) } RETURN n.k",
    );
    assert_eq!(
        vers_p1,
        BTreeSet::from(["F1".to_string(), "F2".to_string()])
    );
    let vers_p2 = ensemble(
        &db,
        "MATCH (p:Project {k: 'P2'}) MATCH (n:File) \
         WHERE EXISTS { MATCH (n)<-[:CONTAINS]-(p) } RETURN n.k",
    );
    assert_eq!(vers_p2, BTreeSet::from(["F3".to_string()]));
}

#[test]
fn les_bornes_de_profondeur_sont_respectees() {
    let db = graphe_main();
    // Fn1 est à 2 sauts de P1 : trouvé par *1..2 et *2..2, pas par *1..1.
    let profondeur = |bornes: &str| {
        ensemble(
            &db,
            &format!(
                "MATCH (p:Project {{k: 'P1'}}) MATCH (n:Function) \
                 WHERE EXISTS {{ MATCH (n)<-[:CONTAINS{bornes}]-(p) }} RETURN n.k"
            ),
        )
    };
    assert_eq!(profondeur("*1..2"), BTreeSet::from(["Fn1".to_string()]));
    assert_eq!(profondeur("*2..2"), BTreeSet::from(["Fn1".to_string()]));
    assert!(profondeur("*1..1").is_empty());
    assert!(profondeur("").is_empty());
}

#[test]
fn not_exists_rend_le_complement() {
    let db = graphe_main();
    let hors_p1 = ensemble(
        &db,
        "MATCH (p:Project {k: 'P1'}) MATCH (n:File) \
         WHERE NOT EXISTS { MATCH (n)<-[:CONTAINS*1..2]-(p) } RETURN n.k",
    );
    assert_eq!(
        hors_p1,
        BTreeSet::from(["F3".to_string(), "F4".to_string()])
    );
}

#[test]
fn la_forme_or_de_labels_combinee_a_exists_donne_le_meme_resultat() {
    let db = graphe_main();
    // La forme exacte de `code_45` : `(n:File OR n:Function) AND EXISTS {…}`.
    let ou = ensemble(
        &db,
        "MATCH (p:Project {k: 'P1'}) MATCH (n) \
         WHERE (n:File OR n:Function) AND EXISTS { MATCH (n)<-[:CONTAINS*1..2]-(p) } \
         RETURN n.k",
    );
    assert_eq!(
        ou,
        BTreeSet::from(["F1".to_string(), "F2".to_string(), "Fn1".to_string()])
    );
}

#[test]
fn un_noeud_externe_nul_n_est_jamais_garde() {
    let db = graphe_main();
    // `n` NULL (OPTIONAL MATCH sans voisin) : ni EXISTS ni la semi-jointure ne le
    // retiennent — la réécriture corrélée ne doit pas le laisser passer.
    let gardes = ensemble(
        &db,
        "MATCH (p:Project {k: 'P1'}) MATCH (s:File {k: 'F2'}) \
         OPTIONAL MATCH (s)-[:IMPORTS]->(n) \
         WITH p, n WHERE EXISTS { MATCH (n)<-[:CONTAINS*1..2]-(p) } RETURN n.k",
    );
    assert!(gardes.is_empty(), "{gardes:?}");
}

/// Générateur congruentiel déterministe : aucune dépendance, aucun hasard réel.
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

/// Graphe pseudo-aléatoire : 4 projets, 60 fichiers répartis (certains orphelins),
/// 2 fonctions par fichier, IMPORTS aléatoires.
fn graphe_aleatoire(graine: u64) -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let mut alea = Lcg(graine);
    let projets: Vec<_> = (0..4)
        .map(|i| noeud(&db, "Project", &format!("P{i}")))
        .collect();
    let mut fichiers = Vec::new();
    for i in 0..60 {
        let f = noeud(&db, "File", &format!("F{i:02}"));
        // 1 fichier sur 6 reste sans projet ; un autre sur 10 est dans DEUX projets.
        if alea.suivant(6) != 0 {
            arete(&db, projets[alea.suivant(4) as usize], f, "CONTAINS");
        }
        if alea.suivant(10) == 0 {
            arete(&db, projets[alea.suivant(4) as usize], f, "CONTAINS");
        }
        for j in 0..2 {
            let g = noeud(&db, "Function", &format!("F{i:02}_fn{j}"));
            arete(&db, f, g, "CONTAINS");
        }
        fichiers.push(f);
    }
    for _ in 0..180 {
        let a = fichiers[alea.suivant(60) as usize];
        let b = fichiers[alea.suivant(60) as usize];
        if a != b {
            arete(&db, a, b, "IMPORTS");
        }
    }
    db
}

#[test]
fn exists_lie_donne_le_meme_ensemble_que_la_formulation_par_match() {
    for graine in [1, 7, 42, 2026] {
        let db = graphe_aleatoire(graine);
        for projet in 0..4 {
            for bornes in ["*1..1", "*1..2", "*2..2", "*1..3"] {
                let avec_exists = ensemble(
                    &db,
                    &format!(
                        "MATCH (p:Project {{k: 'P{projet}'}}) MATCH (n) \
                         WHERE (n:File OR n:Function) \
                           AND EXISTS {{ MATCH (n)<-[:CONTAINS{bornes}]-(p) }} \
                         RETURN n.k"
                    ),
                );
                let avec_match = ensemble(
                    &db,
                    &format!(
                        "MATCH (p:Project {{k: 'P{projet}'}})-[:CONTAINS{bornes}]->(n) \
                         WHERE n:File OR n:Function RETURN DISTINCT n.k"
                    ),
                );
                assert_eq!(
                    avec_exists, avec_match,
                    "graine {graine}, projet P{projet}, bornes {bornes}"
                );
            }
        }
    }
}

#[test]
fn not_exists_lie_est_le_complement_exact_de_exists_lie() {
    let db = graphe_aleatoire(99);
    for projet in 0..4 {
        let tous = ensemble(&db, "MATCH (n:File) RETURN n.k");
        let dedans = ensemble(
            &db,
            &format!(
                "MATCH (p:Project {{k: 'P{projet}'}}) MATCH (n:File) \
                 WHERE EXISTS {{ MATCH (n)<-[:CONTAINS*1..2]-(p) }} RETURN n.k"
            ),
        );
        let dehors = ensemble(
            &db,
            &format!(
                "MATCH (p:Project {{k: 'P{projet}'}}) MATCH (n:File) \
                 WHERE NOT EXISTS {{ MATCH (n)<-[:CONTAINS*1..2]-(p) }} RETURN n.k"
            ),
        );
        assert!(dedans.is_disjoint(&dehors), "projet P{projet}");
        let union: BTreeSet<String> = dedans.union(&dehors).cloned().collect();
        assert_eq!(union, tous, "projet P{projet}");
    }
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
    // Forme de `code_45` : voisins IMPORTS d'un fichier, gardés s'ils sont contenus
    // dans le projet lié. Le résultat (5 voisins) et le voisinage sont constants ;
    // seule la taille du graphe change.
    let requete = "MATCH (s:File {path: 'f0'})<-[:CONTAINS]-(p:Project {id: 'p1'}) \
        CALL { WITH s, p MATCH (s)-[:IMPORTS*1..1]-(n) \
               WHERE (n:File OR n:Function) AND EXISTS { MATCH (n)<-[:CONTAINS*1..2]-(p) } \
               RETURN collect(DISTINCT n) AS voisins } \
        RETURN size(voisins) AS c";
    let mut mesures = Vec::new();
    for fichiers in [300u64, 3_000] {
        let db = ObrainDB::new_in_memory();
        let session = db.session();
        let projet =
            session.create_node_with_props(&["Project"], [("id", Value::String("p1".into()))]);
        let ids: Vec<_> = (0..fichiers)
            .map(|i| {
                let f = session.create_node_with_props(
                    &["File"],
                    [("path", Value::String(format!("f{i}").into()))],
                );
                session.create_edge(projet, f, "CONTAINS");
                for j in 0..3 {
                    let fonction = session.create_node_with_props(
                        &["Function"],
                        [("name", Value::String(format!("fn{i}_{j}").into()))],
                    );
                    session.create_edge(f, fonction, "CONTAINS");
                }
                f
            })
            .collect();
        let n = ids.len();
        for i in 0..n {
            for cible in [(i + 1) % n, (i * 7 + 1) % n, (i * 13 + 5) % n] {
                if cible != i {
                    session.create_edge(ids[i], ids[cible], "IMPORTS");
                }
            }
        }
        let resultat = session.execute_cypher(requete).expect("requête");
        assert_eq!(
            resultat.rows.first().and_then(|l| l.first()),
            Some(&Value::Int64(5)),
            "{fichiers} fichiers"
        );
        mesures.push(plus_grand_nombre_de_lignes(&db, requete));
    }
    // Avant le correctif, le sous-motif était énuméré pour tout le graphe : le plus
    // grand opérateur traitait des milliers de lignes (≥ le nombre de fichiers) et
    // croissait avec lui. Désormais il reste borné par le voisinage de `s`.
    assert!(
        mesures.iter().all(|&m| m <= 50),
        "un opérateur traite encore {mesures:?} lignes : le sous-motif est de nouveau énuméré pour tout le graphe"
    );
}

/// Chaque forme du sous-motif que la réécriture touche doit rendre exactement la
/// formulation par `MATCH`. Couvre : un intermédiaire non lié, un filtre écrit à
/// l'intérieur du `EXISTS` sur la colonne vertébrale, et le `WITH` d'import (chemin
/// `ParameterScan` du traducteur, enveloppe `plan_correlated_exists`).
#[test]
fn chaque_forme_du_sous_motif_rend_la_formulation_par_match() {
    // (requête avec EXISTS, requête par MATCH équivalente) ; `{p}` = nom du projet.
    let cas: [(&str, &str); 2] = [
        (
            "MATCH (p:Project {k: '{p}'}) MATCH (n) WHERE (n:File OR n:Function) \
             AND EXISTS { MATCH (n)<-[:CONTAINS]-(m)<-[:CONTAINS]-(p) } RETURN n.k",
            "MATCH (p:Project {k: '{p}'})-[:CONTAINS]->(m)-[:CONTAINS]->(n) \
             WHERE n:File OR n:Function RETURN DISTINCT n.k",
        ),
        (
            "MATCH (p:Project {k: '{p}'}) MATCH (n) WHERE (n:File OR n:Function) \
             AND EXISTS { MATCH (n)<-[:CONTAINS*1..2]-(p) WHERE n.k <> 'zzz' } RETURN n.k",
            "MATCH (p:Project {k: '{p}'})-[:CONTAINS*1..2]->(n) \
             WHERE (n:File OR n:Function) AND n.k <> 'zzz' RETURN DISTINCT n.k",
        ),
    ];
    for graine in [3, 11] {
        let db = graphe_aleatoire(graine);
        for projet in 0..4 {
            let nom = format!("P{projet}");
            for (avec_exists, par_match) in cas {
                let obtenu = ensemble(&db, &avec_exists.replace("{p}", &nom));
                let attendu = ensemble(&db, &par_match.replace("{p}", &nom));
                assert_eq!(
                    obtenu, attendu,
                    "graine {graine}, projet {nom}, requête « {avec_exists} »"
                );
            }
        }
    }
}
