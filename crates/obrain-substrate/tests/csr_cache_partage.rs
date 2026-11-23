//! Le cache CSR doit être invalidé par une écriture d'un PAIR.
//!
//! `csr_cached` se clé sur `(feature_epoch, taille du WAL)`. L'époque est un
//! atome PAR PROCESSUS : elle ne voit pas ce qu'un autre écrivain fait dans la
//! même base (mesuré : `feature_epoch_ne_voit_pas_les_ecritures_d_un_pair`).
//! La taille du WAL, elle, est partagée — c'est elle qui rattrape le pair.
//!
//! Sans cette seconde moitié de clé, une base ouverte en écriture partagée
//! sert un CSR périmé en silence : HKS, Ricci, pas de chaleur, résistance
//! effective et géodésiques calculent sur une topologie qui n'existe plus.
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::store::SubstrateStore;
use std::sync::Arc;

fn base(td: &tempfile::TempDir) -> std::path::PathBuf {
    let chemin = td.path().join("kb");
    let s = SubstrateStore::create(&chemin).unwrap();
    let a = s.create_node(&["N"]);
    let b = s.create_node(&["N"]);
    s.create_edge(a, b, "LIEN");
    s.flush().unwrap();
    drop(s);
    chemin
}

#[test]
fn sans_ecriture_le_cache_sert_le_meme_arc() {
    let td = tempfile::tempdir().unwrap();
    let chemin = base(&td);
    let s = SubstrateStore::open(&chemin).unwrap();
    let c1 = s.csr_cached();
    let c2 = s.csr_cached();
    assert!(
        Arc::ptr_eq(&c1, &c2),
        "deux appels sans écriture doivent rendre le MÊME Arc : sinon le cache ne sert à rien"
    );
}

#[test]
fn une_ecriture_d_un_pair_invalide_le_cache() {
    let td = tempfile::tempdir().unwrap();
    let chemin = base(&td);
    let a = SubstrateStore::open_shared_write(&chemin).unwrap();
    let b = SubstrateStore::open_shared_write(&chemin).unwrap();

    let avant = a.csr_cached();
    let n1 = b.create_node(&["N"]);
    let n2 = b.create_node(&["N"]);
    b.create_edge(n1, n2, "LIEN");
    let apres = a.csr_cached();

    assert!(
        !Arc::ptr_eq(&avant, &apres),
        "l'écriture d'un PAIR n'a pas invalidé le cache de A : le CSR servi est périmé, \
         et rien ne le signale"
    );
}
