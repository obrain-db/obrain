//! Le registre d'UDF cognitives (par processus) ne retient PAS la base qui
//! l'a alimenté.
//!
//! Défaut corrigé (2026-11-03) : chaque UDF tenait un `Arc` fort vers le
//! moteur cognitif de la dernière base ouverte ; le moteur tient le
//! `SubstrateStore`, donc ses mmap et sa part de verrou de répertoire
//! survivaient à `close()`/drop jusqu'à l'ouverture d'une AUTRE base dans le
//! processus. Symptôme mesuré (Linux, `--all-features`, binaire entier) :
//! « substrate … is already open read-only in this process — cannot upgrade
//! to an exclusive (write) lock » sur une base dont tous les ouvreurs étaient
//! fermés.
#![cfg(feature = "cognitive")]

use std::sync::Arc;

use obrain_adapters::plugins::PluginRegistry;
use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use obrain_engine::cognitive_udfs::register_cognitive_udfs;

/// Le moteur cognitif meurt avec sa base : rien dans le processus ne le
/// retient une fois la base fermée. Déterministe quel que soit le voisinage.
#[test]
fn le_moteur_cognitif_meurt_avec_sa_base() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("udf-retention");
    let db = ObrainDB::open_substrate(&chemin).expect("création");
    let faible = Arc::downgrade(db.cognitive_engine().expect("moteur cognitif construit"));
    drop(db);
    assert!(
        faible.upgrade().is_none(),
        "le moteur cognitif (et avec lui le store, ses mmap et son verrou) survit \
         à la fermeture de sa base : quelque chose dans le processus le retient"
    );
}

/// La forme exacte du rouge CI : un lecteur RO fermé, puis une réouverture
/// EXCLUSIVE de la même base. L'ouverture intermédiaire d'une autre base
/// reproduit ce que fait un test voisin : elle évince la base de création du
/// registre, pour que le lecteur crée sa propre part de verrou (partagée) —
/// sans cela le lecteur réutilisait la part exclusive de la création, jamais
/// libérée, et le test passait pour une mauvaise raison.
#[test]
fn une_base_lue_puis_fermee_se_rouvre_en_ecriture() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("udf-ro-puis-exclusif");
    let autre = td.path().join("udf-autre-base");
    {
        let _creation = ObrainDB::open_substrate(&chemin).expect("création");
    }
    {
        let _voisine = ObrainDB::open_substrate(&autre).expect("autre base");
    }
    {
        let lecteur = ObrainDB::open_read_only(&chemin).expect("lecteur RO");
        assert_eq!(lecteur.node_count(), 0);
    }
    let db = ObrainDB::open_substrate(&chemin)
        .expect("réouverture exclusive après la fermeture du dernier lecteur");
    assert_eq!(db.node_count(), 0);
}

/// Une UDF enregistrée reste résoluble (contrat du registre), mais quand sa
/// base est fermée elle répond une erreur nommée — jamais la valeur d'une
/// base morte, et sans la maintenir en vie.
#[test]
fn une_udf_dont_la_base_est_fermee_repond_une_erreur_nommee() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("udf-fermee");
    let registre = PluginRegistry::new();
    let db = ObrainDB::open_substrate(&chemin).expect("création");
    register_cognitive_udfs(
        &registre,
        Arc::clone(db.cognitive_engine().expect("moteur cognitif construit")),
    );
    let energie = registre.get_udf("obrain.energy").expect("UDF enregistrée");
    assert!(
        energie.evaluate(&[Value::Int64(1)]).is_ok(),
        "base ouverte : l'UDF répond"
    );
    drop(db);
    let erreur = energie
        .evaluate(&[Value::Int64(1)])
        .expect_err("base fermée : l'UDF ne répond pas une valeur");
    assert!(
        erreur.to_string().contains("closed"),
        "l'erreur nomme la cause (base fermée), reçu : {erreur}"
    );
    for nom in ["obrain.risk", "obrain.synapses"] {
        let udf = registre.get_udf(nom).expect("UDF enregistrée");
        assert!(
            udf.evaluate(&[Value::Int64(1)]).is_err(),
            "{nom} : base fermée"
        );
    }
}
