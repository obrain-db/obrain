//! Recuperation apres panne REELLE : on tue sans flush, on rouvre, on compte.
//!
//! Un test qui rejoue deux fois de suite (`replay_is_idempotent`) prouve
//! l'idempotence — pas la COMPLETUDE. Ces tests-ci simulent ce qui arrive
//! vraiment : des ecritures acceptees, un processus qui disparait sans
//! `flush()`, une reouverture.
//!
//! ETAT AU 14/08 : deux de ces trois tests etaient `#[ignore]` (ils
//! echouaient, et c'etait voulu : ils decrivaient une capacite que la base
//! n'avait pas encore). ETAT AU 01/11 : les trois tournent sans attribut et
//! passent — le rejeu des enregistrements de noeuds et d'aretes a ete cable
//! en production. Ce qu'ils ne couvrent PAS : la disparition du processus
//! (SIGKILL) avec des octets encore en tampon utilisateur ; c'est l'objet de
//! `obrain-engine/tests/durabilite_commit_kill9.rs` (D-DUR, v0.0.1).
//!
//! CE QU'ILS ONT ETABLI :
//!   - les proprietes etaient deja recuperees (`merge_wal_inline_props`,
//!     appelee a l'ouverture depuis toujours) ;
//!   - les ENREGISTREMENTS de noeuds et d'aretes, eux, ne l'etaient PAS —
//!     `replay_from` n'avait aucun appelant en production. On restaurait donc
//!     des proprietes sur des entites disparues.

use obrain_common::types::{PropertyKey, Value};
use obrain_core::graph::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;

/// Ecrit sans jamais flusher, puis « perd » le processus en laissant tomber le
/// store sans `flush()` — les zones mmappees gardent ce que le noyau a bien
/// voulu ecrire, le WAL garde tout.
fn ecrire_puis_perdre(dir: &std::path::Path, n: u64) {
    let store = SubstrateStore::create(dir).expect("creation");
    // Le checkpoint automatique ecrirait un etat propre : on le desactive
    // pour que le test porte bien sur le REJEU et non sur le flush.
    store.definir_seuil_checkpoint(0);
    for i in 0..n {
        let id = store.create_node(&["Doc"]);
        store.set_node_property(id, "titre", Value::String(format!("document {i}").into()));
    }
    // PAS de flush : c'est tout l'objet du test.
    drop(store);
}

#[test]
fn une_base_rouverte_retrouve_ses_noeuds_et_leurs_proprietes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    ecrire_puis_perdre(tmp.path(), 120);

    // Reouverture : `from_substrate` rejoue le WAL, puis les proprietes.
    let store = SubstrateStore::open(tmp.path()).expect("reouverture");

    let noeuds = store.nodes_by_label("Doc");
    assert_eq!(
        noeuds.len(),
        120,
        "les 120 noeuds ecrits avant la panne doivent etre retrouves"
    );

    // ── ET LEURS PROPRIETES ──────────────────────────────────────────────
    // C'est le couple qui compte : un noeud sans sa propriete est une
    // coquille, une propriete sans son noeud est une orpheline. Les deux
    // chemins de recuperation doivent converger.
    let cle = PropertyKey::new("titre");
    let avec_titre = noeuds
        .iter()
        .filter(|id| store.get_node_property(**id, &cle).is_some())
        .count();
    assert_eq!(
        avec_titre, 120,
        "chaque noeud retrouve doit porter son titre — sinon on restaure des \
         coquilles"
    );
}

/// Une base qui n'a JAMAIS ete checkpointee ne doit PAS rejouer : son
/// `last_wal_offset` vaut zero parce que personne ne l'a jamais avance, pas
/// parce que rien n'a ete ecrit. Rejouer depuis la ferait relire l'histoire
/// entiere — 12,8 Go sur la base de production.
///
/// La garde doit sauter le rejeu ET laisser la base parfaitement utilisable.
#[test]
fn base_jamais_checkpointee_reste_ouvrable() {
    let tmp = tempfile::tempdir().expect("tempdir");
    {
        let store = SubstrateStore::create(tmp.path()).expect("creation");
        store.definir_seuil_checkpoint(0);
        for i in 0..40u64 {
            let id = store.create_node(&["Doc"]);
            store.set_node_property(id, "n", Value::Int64(i as i64));
        }
        store.flush().expect("flush");
    }

    let store = SubstrateStore::open(tmp.path()).expect("reouverture");
    assert_eq!(
        store.nodes_by_label("Doc").len(),
        40,
        "le flush a rendu les zones durables : tout doit etre la, rejeu ou pas"
    );
}

/// Apres un checkpoint, la ligne de base est etablie : la reouverture rejoue
/// pour de bon, et les ecritures posterieures au checkpoint sont retrouvees.
#[test]
fn apres_checkpoint_le_rejeu_recupere_les_ecritures_suivantes() {
    let tmp = tempfile::tempdir().expect("tempdir");

    {
        let store = SubstrateStore::create(tmp.path()).expect("creation");
        store.definir_seuil_checkpoint(0);
        for i in 0..30u64 {
            let id = store.create_node(&["Avant"]);
            store.set_node_property(id, "i", Value::Int64(i as i64));
        }
        // Checkpoint EXPLICITE : etablit la ligne de base et tronque le WAL.
        store.checkpoint().expect("checkpoint");

        // Puis on ecrit encore, et on perd le processus sans flush.
        for i in 0..25u64 {
            let id = store.create_node(&["Apres"]);
            store.set_node_property(id, "i", Value::Int64(i as i64));
        }
        drop(store);
    }

    let store = SubstrateStore::open(tmp.path()).expect("reouverture");
    assert_eq!(
        store.nodes_by_label("Avant").len(),
        30,
        "les noeuds d'avant le checkpoint sont dans les zones"
    );
    assert_eq!(
        store.nodes_by_label("Apres").len(),
        25,
        "les noeuds ecrits APRES le checkpoint doivent revenir par le rejeu — \
         c'est exactement ce que le WAL est cense couvrir"
    );
}
