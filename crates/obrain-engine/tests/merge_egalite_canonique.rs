//! `MERGE` retrouve un porteur ÉGAL au sens du langage, pas seulement
//! structurellement identique : `MERGE (n:T {a: 5})` trouve `{a: 5.0}`, et une
//! date écrite dans un autre fuseau trouve le même instant. Sans ce passage à
//! `egalite_structurelle` dans la revérification des candidats (`merge.rs`,
//! `porteurs_du_motif`), le MERGE créait un doublon d'un nœud que `MATCH`
//! trouvait. Sans contrainte d'unicité : seul le MERGE est en jeu.

#![cfg(feature = "cypher")]

use obrain_engine::ObrainDB;

const DT_Z: &str = "2026-11-11T10:00:00Z";
const DT_PLUS2: &str = "2026-11-11T12:00:00+02:00";

fn nombre_de_t(db: &ObrainDB) -> usize {
    db.session()
        .execute("MATCH (n:T) RETURN n")
        .unwrap()
        .rows
        .len()
}

#[test]
fn merge_trouve_le_porteur_egal_entier_flottant() {
    for (existant, motif) in [("5.0", "5"), ("5", "5.0")] {
        let db = ObrainDB::new_in_memory();
        let s = db.session();
        for i in 0..30 {
            s.execute(&format!("INSERT (:T {{a: {}}})", 100 + i))
                .unwrap();
        }
        s.execute(&format!("INSERT (:T {{a: {existant}, vu: 0}})"))
            .unwrap();
        s.execute_cypher(&format!("MERGE (n:T {{a: {motif}}}) ON MATCH SET n.vu = 1"))
            .unwrap();
        assert_eq!(
            nombre_de_t(&db),
            31,
            "{existant} existe, MERGE sur {motif} ne crée rien"
        );
        assert_eq!(
            s.execute("MATCH (n:T {vu: 1}) RETURN n")
                .unwrap()
                .rows
                .len(),
            1,
            "ON MATCH a touché le porteur"
        );
    }
}

#[test]
fn merge_trouve_le_porteur_egal_entre_deux_fuseaux() {
    for (existant, motif) in [(DT_Z, DT_PLUS2), (DT_PLUS2, DT_Z)] {
        let db = ObrainDB::new_in_memory();
        let s = db.session();
        for i in 0..30 {
            s.execute(&format!("INSERT (:T {{b: 'r{i}'}})")).unwrap();
        }
        s.execute(&format!("INSERT (:T {{b: '{existant}', vu: 0}})"))
            .unwrap();
        s.execute_cypher(&format!(
            "MERGE (n:T {{b: '{motif}'}}) ON MATCH SET n.vu = 1"
        ))
        .unwrap();
        assert_eq!(nombre_de_t(&db), 31, "{existant} existe, MERGE sur {motif}");
        assert_eq!(
            s.execute("MATCH (n:T {vu: 1}) RETURN n")
                .unwrap()
                .rows
                .len(),
            1
        );
    }
}
