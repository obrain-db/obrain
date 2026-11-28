//! Banc : l'allocateur natif d'engram (`SubstrateStore::seed_engram` +
//! borne partagée + dict `next_engram_id`) est-il MONOTONE après réouverture ?
//! Séquence : allocation A, retrait (membres vidés), flush, fermeture,
//! réouverture, allocation B — A et B doivent être DISTINCTS.

use obrain_substrate::SubstrateStore;

#[test]
fn allocateur_natif_distinct_apres_retrait_et_reouverture() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");

    let (a, hw_apres_a) = {
        let store = SubstrateStore::create(&chemin).unwrap();
        let a = store.seed_engram(&[]).unwrap();
        let hw = store.next_engram_id();
        // Retrait : l'engramme A est vidé (slot effacé dans la table membres).
        store.set_engram_members(a, &[]).unwrap();
        store.flush().unwrap();
        store.close().unwrap();
        (a, hw)
    };

    let (b, hw_apres_b) = {
        let store = SubstrateStore::open(&chemin).unwrap();
        // Le plancher persisté survit à la réouverture.
        assert!(store.next_engram_id() > a);
        let b = store.seed_engram(&[]).unwrap();
        let hw = store.next_engram_id();
        store.close().unwrap();
        (b, hw)
    };

    assert_eq!(hw_apres_a, a + 1);
    assert_eq!(hw_apres_b, b + 1);
    assert_ne!(
        a, b,
        "l'allocateur natif réutilise l'id retiré après réouverture"
    );
    assert!(a < b, "l'allocateur natif doit être monotone (a={a} b={b})");
}

#[test]
fn le_dernier_id_n_est_pas_reattribue_apres_reouverture() {
    let dossier = tempfile::tempdir().unwrap();
    let chemin = dossier.path().join("base");
    let derniere_borne = u64::from(u16::MAX);

    let store = SubstrateStore::create(&chemin).unwrap();
    store.raise_engram_floor(derniere_borne);
    store.raise_engram_floor(derniere_borne - 1);
    assert_eq!(store.reserve_engram_id().unwrap(), u16::MAX);
    assert!(store.reserve_engram_id().is_err());
    store.flush().unwrap();
    store.close().unwrap();

    let store = SubstrateStore::open(&chemin).unwrap();
    assert!(
        store.reserve_engram_id().is_err(),
        "le dernier identifiant natif reste consommé après réouverture"
    );
}

#[test]
fn relever_un_plancher_epuise_ne_rouvre_pas_l_allocation() {
    let dossier = tempfile::tempdir().unwrap();
    let chemin = dossier.path().join("base");
    let store = SubstrateStore::create(&chemin).unwrap();

    store.raise_engram_floor(u64::from(u16::MAX) + 1);
    store.raise_engram_floor(u64::from(u16::MAX));
    assert!(store.reserve_engram_id().is_err());
}
