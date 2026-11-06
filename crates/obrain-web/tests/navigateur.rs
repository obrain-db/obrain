//! Persistance réelle de la base `Base` dans le navigateur : un instantané publié
//! sur OPFS survit à la fermeture de la base et à sa réouverture. Exécuté dans
//! Chrome headless, en worker dédié (le seul contexte où `createSyncAccessHandle`
//! existe). Même schéma que `obrain-substrate/tests/navigateur_opfs.rs`.
#![cfg(target_arch = "wasm32")]

use obrain_web::Base;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_dedicated_worker);

/// Nom unique par exécution : OPFS garde ses fichiers d'un test à l'autre.
fn nom_unique(tag: &str) -> String {
    let t = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("essai-{tag}-{t}.journal")
}

fn creer_personnes(base: &Base, n: usize) {
    for i in 0..n {
        base.creer_noeud(
            vec!["Personne".to_string()],
            &format!(r#"{{"nom":"p{i}"}}"#),
        )
        .expect("création");
    }
}

#[wasm_bindgen_test]
async fn un_etat_publie_survit_a_une_reouverture() {
    let nom = nom_unique("reouverture");
    {
        let base = Base::ouvrir(nom.clone()).await.expect("ouverture");
        creer_personnes(&base, 4);
        base.persister().expect("publication");
    }
    // Le handle est libéré à la destruction de la base : on rouvre le même fichier.
    let base = Base::ouvrir(nom).await.expect("réouverture");
    assert_eq!(
        base.noeuds(),
        4,
        "les 4 nœuds publiés sont revenus depuis OPFS"
    );
    let noms = base
        .executer("MATCH (p:Personne) RETURN p.nom AS nom ORDER BY nom")
        .expect("requête");
    assert!(
        noms.contains("p0") && noms.contains("p3"),
        "propriétés relues : {noms}"
    );
}

#[wasm_bindgen_test]
async fn un_etat_non_publie_est_perdu_a_la_fermeture() {
    let nom = nom_unique("non-publie");
    {
        let base = Base::ouvrir(nom.clone()).await.expect("ouverture");
        creer_personnes(&base, 2);
        // Aucune publication : la base est simplement détruite.
    }
    let base = Base::ouvrir(nom).await.expect("réouverture");
    assert_eq!(base.noeuds(), 0, "rien n'a été publié : rien ne revient");
}

#[wasm_bindgen_test]
async fn un_second_ouvreur_du_meme_fichier_est_refuse() {
    let nom = nom_unique("exclusif");
    let premier = Base::ouvrir(nom.clone()).await.expect("premier");
    assert!(
        Base::ouvrir(nom.clone()).await.is_err(),
        "un second handle sur le même journal doit être refusé"
    );
    drop(premier);
    assert!(Base::ouvrir(nom).await.is_ok(), "libéré : on peut rouvrir");
}

#[wasm_bindgen_test]
fn persister_une_base_non_ouverte_est_une_erreur() {
    assert!(
        Base::nouvelle().persister().is_err(),
        "une base en mémoire n'a pas de journal à publier"
    );
}
