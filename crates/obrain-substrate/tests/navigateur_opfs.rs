//! Persistance réelle dans le navigateur : un store publié sur OPFS survit à la
//! fermeture de son handle et à sa réouverture. Exécuté dans Chrome headless, en
//! worker dédié (le seul contexte où `createSyncAccessHandle` existe).
#![cfg(target_arch = "wasm32")]

use obrain_common::types::Value;
use obrain_core::graph::{GraphStore as _, GraphStoreMut as _};
use obrain_substrate::opfs::JournalOpfs;
use obrain_substrate::store::SubstrateStore;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_dedicated_worker);

/// Nom unique par exécution : OPFS garde ses fichiers d'un test à l'autre.
fn nom_unique(tag: &str) -> String {
    format!("essai-{tag}-{}.journal", js_sys_temps())
}

fn js_sys_temps() -> u64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[wasm_bindgen_test]
async fn un_store_publie_survit_a_une_reouverture() {
    let nom = nom_unique("reouverture");
    {
        let (store, miroir) = SubstrateStore::ouvrir_navigateur(&nom)
            .await
            .expect("ouverture");
        for i in 0..4 {
            let id = store.create_node(&["Personne"]);
            store.set_node_property(id, "nom", Value::String(format!("p{i}").into()));
        }
        store.persister_sur_miroir(miroir).expect("publication");
    }
    // Le handle est libéré à la destruction du journal : on rouvre le même fichier.
    let (store, _miroir) = SubstrateStore::ouvrir_navigateur(&nom)
        .await
        .expect("réouverture");
    assert_eq!(
        store.node_count(),
        4,
        "les 4 nœuds publiés sont revenus depuis OPFS"
    );
}

#[wasm_bindgen_test]
async fn un_handle_opfs_est_exclusif() {
    let nom = nom_unique("exclusif");
    let premier = JournalOpfs::ouvrir(&nom).await.expect("premier handle");
    let second = JournalOpfs::ouvrir(&nom).await;
    assert!(
        second.is_err(),
        "un second handle sur le même fichier doit être refusé"
    );
    drop(premier);
    assert!(
        JournalOpfs::ouvrir(&nom).await.is_ok(),
        "libéré : on peut rouvrir"
    );
}

#[wasm_bindgen_test]
async fn un_etat_non_publie_est_perdu_a_la_fermeture() {
    let nom = nom_unique("non-publie");
    {
        let (store, miroir) = SubstrateStore::ouvrir_navigateur(&nom)
            .await
            .expect("ouverture");
        let id = store.create_node(&["Personne"]);
        store.set_node_property(id, "nom", Value::String("jamais publié".into()));
        drop(miroir);
        drop(store);
    }
    let (store, _miroir) = SubstrateStore::ouvrir_navigateur(&nom)
        .await
        .expect("réouverture");
    assert_eq!(
        store.node_count(),
        0,
        "rien n'a été publié : rien ne revient"
    );
}
