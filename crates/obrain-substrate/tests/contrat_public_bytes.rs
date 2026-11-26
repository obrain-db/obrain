//! Contrat public consommable par un client EXTERNE au crate substrate : capture
//! cohérente, lecture depuis la capture (SANS seconde lecture indépendante),
//! mutation conditionnelle, recapture après conflit certain.

use std::sync::Arc;

use obrain_common::types::{PropertyKey, Value};
use obrain_core::graph::traits::{GraphStore, GraphStoreMut};
use obrain_substrate::{
    IssuePublication, ModePublication, Precondition, ReferenceTentative, ResolutionEffet,
    SubstrateStore,
};

fn incr_byte0(bytes: &[u8]) -> Vec<u8> {
    let mut v = bytes.to_vec();
    v[0] = v[0].wrapping_add(1);
    v
}

fn bytes_de(valeur: &Value) -> Vec<u8> {
    match valeur {
        Value::Bytes(b) => b.to_vec(),
        autre => panic!("attendu Bytes, lu {autre:?}"),
    }
}

#[test]
fn contrat_public_mutation_conditionnelle_bytes_32() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(td.path().join("kb")).unwrap();
    let id = store.create_node(&["ancre"]);
    store.set_node_property(id, "ancre", Value::Bytes(Arc::from([0u8; 32].to_vec())));

    // capture cohérente (contenu + version) — la valeur est lue DEPUIS la capture
    let pre: Precondition = store.capturer_precondition(id, &["ancre"]).unwrap();
    let v0 = bytes_de(pre.valeur_capturee("ancre").expect("ancre capturée présente"));

    // A : incrément calculé depuis la capture
    let a = incr_byte0(&v0);
    let prep_a = store
        .preparer_transformation(id, &[("ancre", Value::Bytes(Arc::from(a.clone())))], &pre, ModePublication::Strict)
        .unwrap();
    assert_eq!(store.publier_si_precondition_intacte(prep_a).unwrap(), IssuePublication::PublieIntegrale);

    // B : même incrément depuis la MÊME capture → précondition obsolète
    let prep_b = store
        .preparer_transformation(id, &[("ancre", Value::Bytes(Arc::from(a)))], &pre, ModePublication::Strict)
        .unwrap();
    assert_eq!(store.publier_si_precondition_intacte(prep_b).unwrap(), IssuePublication::ConflitPrecondition);

    // recapture + recalcul depuis l'état courant
    let pre2: Precondition = store.capturer_precondition(id, &["ancre"]).unwrap();
    let v1 = bytes_de(pre2.valeur_capturee("ancre").expect("ancre recapturée présente"));
    let b2 = incr_byte0(&v1);
    let prep_b2 = store
        .preparer_transformation(id, &[("ancre", Value::Bytes(Arc::from(b2)))], &pre2, ModePublication::Strict)
        .unwrap();
    assert_eq!(store.publier_si_precondition_intacte(prep_b2).unwrap(), IssuePublication::PublieIntegrale);

    let finale = store.get_node_property(id, &PropertyKey::new("ancre")).expect("ancre finale");
    assert_eq!(bytes_de(&finale)[0], 2, "deux incréments (0 → 1 → 2)");
    eprintln!("CONTRAT_PUBLIC_OK : Bytes 32 octets, capture→calcul→engagement→recapture→recalcul");
}


#[test]
fn contrat_public_resolution_apres_perte_reponse() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let (id, reference) = {
        let store = SubstrateStore::create(&chemin).unwrap();
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let pre = store.capturer_precondition(id, &["a"]).unwrap();
        let prep = store
            .preparer_transformation(id, &[("a", Value::Int64(10))], &pre, ModePublication::Strict)
            .unwrap();
        // la référence de tentative est conservée AVANT la publication
        let reference: ReferenceTentative = prep.reference_tentative();
        // publication réelle, réponse « perdue » (verdict non reçu)
        let _verdict = store.publier_si_precondition_intacte(prep).unwrap();
        store.close().unwrap();
        (id, reference)
    };
    let store2 = SubstrateStore::open(&chemin).unwrap();
    // résoudre SANS republier
    let res = store2.resoudre_effet_historique_par_reference(id, "a", reference).unwrap();
    assert_eq!(res, ResolutionEffet::Engage, "l'engagement est attesté après réouverture");
    // masquage ultérieur : l'événement historique reste attesté
    store2.set_node_property(id, "a", Value::Int64(99));
    assert_eq!(
        store2.resoudre_effet_historique_par_reference(id, "a", reference).unwrap(),
        ResolutionEffet::Engage,
        "masqué mais historiquement engagé"
    );
    eprintln!("REFERENCE_OK : référence conservée → publication → perte → résolution sans republier");
}
