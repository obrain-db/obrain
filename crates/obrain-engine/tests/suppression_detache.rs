//! Un nœud supprimé emporte ses arêtes (contrat documenté de `delete_node`).
//! Défaut mesuré sur le noyau : une arête CONTENT entrante survivait à la suppression
//! du fichier qui la portait, et pointait vers un Blob désormais orphelin en apparence.
use obrain_common::types::PropertyKey;
use obrain_engine::ObrainDB;

#[test]
fn supprimer_un_noeud_supprime_ses_aretes_entrantes_et_sortantes() {
    let db = ObrainDB::new_in_memory();
    let fichier = db.create_node(&["File"]);
    let blob = db.create_node(&["Blob"]);
    let autre = db.create_node(&["Function"]);
    db.create_edge(fichier, blob, "CONTENT");
    db.create_edge(autre, fichier, "DEFINES");
    db.create_edge(blob, autre, "CALLS");
    assert_eq!(db.edge_count(), 3);

    assert!(db.delete_node(fichier));

    assert_eq!(db.edge_count(), 1, "seules les arêtes sans lien avec le fichier restent");
    let restantes = db.store().edges_from(blob, obrain_core::graph::Direction::Incoming);
    assert!(
        restantes.is_empty(),
        "aucune arête CONTENT fantôme ne pointe plus vers le blob : {restantes:?}"
    );
    let _ = PropertyKey::new("unused");
}
