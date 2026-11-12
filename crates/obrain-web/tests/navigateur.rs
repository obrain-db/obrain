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

/// Contrat : une publication REMPLACE la précédente ; l'état relu est le dernier publié.
#[wasm_bindgen_test]
async fn une_seconde_publication_remplace_la_premiere() {
    let nom = nom_unique("remplace");
    {
        let base = Base::ouvrir(nom.clone()).await.expect("ouverture");
        creer_personnes(&base, 2);
        base.persister().expect("publication 1");
        creer_personnes(&base, 3);
        base.persister().expect("publication 2");
    }
    let base = Base::ouvrir(nom).await.expect("réouverture");
    assert_eq!(base.noeuds(), 5, "l'état de la dernière publication");
}

/// Contrat : les écritures faites APRÈS la dernière publication ne sont pas promises.
/// Elles ne touchent pas OPFS avant `persister()` : rien ne revient à la réouverture.
#[wasm_bindgen_test]
async fn les_ecritures_apres_la_derniere_publication_sont_perdues() {
    let nom = nom_unique("apres-publication");
    {
        let base = Base::ouvrir(nom.clone()).await.expect("ouverture");
        creer_personnes(&base, 3);
        base.persister().expect("publication");
        creer_personnes(&base, 4); // jamais publiées
    }
    let base = Base::ouvrir(nom).await.expect("réouverture");
    assert_eq!(base.noeuds(), 3, "seule la publication survit");
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

/// Un import de nœuds, publié, survit à la réouverture OPFS (même chemin que `persister`).
#[wasm_bindgen_test]
async fn un_import_publie_survit_a_une_reouverture() {
    let nom = nom_unique("import");
    {
        let base = Base::ouvrir(nom.clone()).await.expect("ouverture");
        let n = base
            .importer_noeuds("Personne", r#"[{"nom":"a"},{"nom":"b"},{"nom":"c"}]"#)
            .expect("import");
        assert_eq!(n, 3, "trois nœuds importés");
        base.persister().expect("publication");
    }
    let base = Base::ouvrir(nom).await.expect("réouverture");
    assert_eq!(
        base.noeuds(),
        3,
        "les nœuds importés sont revenus depuis OPFS"
    );
    let noms = base
        .executer("MATCH (p:Personne) RETURN p.nom AS nom ORDER BY nom")
        .expect("requête");
    assert!(
        noms.contains("\"b\"") && noms.contains("\"c\""),
        "propriétés relues : {noms}"
    );
}

/// Un import invalide n'écrit rien : la validation précède la première écriture.
#[wasm_bindgen_test]
fn un_import_invalide_n_ecrit_rien() {
    let base = Base::nouvelle();
    assert!(
        base.importer_noeuds("Personne", r#"[{"nom":"a"}, 3]"#)
            .is_err(),
        "une ligne non objet doit être refusée"
    );
    assert_eq!(base.noeuds(), 0, "aucune écriture partielle");
}
