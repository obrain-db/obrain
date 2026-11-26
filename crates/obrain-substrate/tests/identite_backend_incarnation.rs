//! ── IDENTITÉ BACKEND / INCARNATION — témoin externe (#334, lot C) ────────
//!
//! Cinq scénarios d'identité, éprouvés UNIQUEMENT via l'API publique (comme
//! `reference_tentative_externe.rs`) : c'est la condition pour que le résultat
//! vaille comme contrat pour un consommateur (this-harness).
//!
//! Les trois identités à distinguer :
//!   * ÉCRIVAIN           = `ReferenceTentative.auteur()` (writer_id persisté
//!     dans `writer.id`). N'est PAS une identité de base : deux bases peuvent
//!     le partager (copie du fichier).
//!   * DOMAINE DE STOCKAGE = la base (répertoire). N'apparaît NULLE PART dans
//!     la référence : le consommateur doit la conserver HORS-BANDE.
//!   * INCARNATION DE RESSOURCE = `ReferenceTentative.cible()` (NodeId,
//!     alloué monotone, jamais réutilisé). Un NodeId vivant ne prouve pas la
//!     même incarnation.

use obrain_common::types::Value;
use obrain_core::graph::GraphStoreMut as _;
use obrain_substrate::store::{
    EtatPropriete, IntegriteTentative, ModePublication, ResolutionEffet,
};
use obrain_substrate::{ReferenceTentative, SubstrateStore};

/// Copie byte-à-byte du CONTENU d'un répertoire de base (tous les fichiers,
/// y compris `writer.id` et `substrate.meta`).
fn copier_base(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entree in std::fs::read_dir(src).unwrap() {
        let e = entree.unwrap();
        let chemin = e.path();
        if chemin.is_file() {
            std::fs::copy(&chemin, dst.join(e.file_name())).unwrap();
        }
    }
}

/// Lit `writer.id` (8 octets LE) — observation directe du writer_id persisté.
fn lire_writer_id(base: &std::path::Path) -> u64 {
    let octets = std::fs::read(
        base.join(obrain_substrate::writer_id::FICHIER_WRITER_ID),
    )
    .expect("writer.id présent dans la base");
    u64::from_le_bytes(octets.as_slice().try_into().expect("8 octets"))
}

// ── Scénario 1 : copie puis DIVERGENCE ─────────────────────────────────────
//
// Copier le répertoire du store, puis écrire des valeurs DIFFÉRENTES dans
// l'original et la copie. Une référence (auteur, horloge) résout selon les
// données RÉELLES de chaque base — jamais une fausse confirmation croisée.
#[test]
fn copie_puis_divergence_resout_selon_les_donnees_reelles() {
    let td = tempfile::tempdir().unwrap();
    let base_a = td.path().join("a");
    let base_b = td.path().join("b");

    // ── base A : nœud, publication conditionnelle, référence H0 ──────────
    let (ref_orig, cible, auteur, horloge_orig) = {
        let store = SubstrateStore::create(&base_a).unwrap();
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let pre = store.capturer_precondition(id, &["a"]).unwrap();
        let prep = store
            .preparer_transformation(id, &[("a", Value::Int64(10))], &pre, ModePublication::Strict)
            .unwrap();
        let reference = prep.reference_tentative();
        store.publier_si_precondition_intacte(prep).unwrap();
        let t = (
            reference.to_bytes(),
            reference.cible(),
            reference.auteur(),
            reference.horloge(),
        );
        store.close().unwrap();
        t
    };

    // ── copie byte-à-byte de A vers B (inclut writer.id) ──────────────────
    copier_base(&base_a, &base_b);

    // ── divergence : valeurs DIFFÉRENTES dans l'original et la copie ─────
    let ref_div_a = {
        let store = SubstrateStore::open(&base_a).unwrap();
        let pre = store.capturer_precondition(cible, &["a"]).unwrap();
        let prep = store
            .preparer_transformation(cible, &[("a", Value::Int64(20))], &pre, ModePublication::Strict)
            .unwrap();
        let reference = prep.reference_tentative();
        store.publier_si_precondition_intacte(prep).unwrap();
        let bytes = reference.to_bytes();
        store.close().unwrap();
        bytes
    };
    // écarte les horloges HLC des deux bases (non-collision déterministe)
    std::thread::sleep(std::time::Duration::from_millis(20));
    let ref_div_b = {
        let store = SubstrateStore::open(&base_b).unwrap();
        let pre = store.capturer_precondition(cible, &["a"]).unwrap();
        let prep = store
            .preparer_transformation(cible, &[("a", Value::Int64(30))], &pre, ModePublication::Strict)
            .unwrap();
        let reference = prep.reference_tentative();
        store.publier_si_precondition_intacte(prep).unwrap();
        let bytes = reference.to_bytes();
        store.close().unwrap();
        bytes
    };

    // ── résolution dans chaque base ───────────────────────────────────────
    let store_a = SubstrateStore::open(&base_a).unwrap();
    let store_b = SubstrateStore::open(&base_b).unwrap();
    let r_orig_a = store_a.resoudre_reference(&ReferenceTentative::from_bytes(&ref_orig).unwrap()).unwrap();
    let r_orig_b = store_b.resoudre_reference(&ReferenceTentative::from_bytes(&ref_orig).unwrap()).unwrap();
    let r_diva_a = store_a.resoudre_reference(&ReferenceTentative::from_bytes(&ref_div_a).unwrap()).unwrap();
    let r_diva_b = store_b.resoudre_reference(&ReferenceTentative::from_bytes(&ref_div_a).unwrap()).unwrap();
    let r_divb_b = store_b.resoudre_reference(&ReferenceTentative::from_bytes(&ref_div_b).unwrap()).unwrap();
    let r_divb_a = store_a.resoudre_reference(&ReferenceTentative::from_bytes(&ref_div_b).unwrap()).unwrap();

    // (1) l'entrée historique H0 a été copiée : l'engagement est VRAI dans les deux.
    assert_eq!(r_orig_a.integrite, IntegriteTentative::Integrale);
    assert_eq!(r_orig_b.integrite, IntegriteTentative::Integrale);

    // (2) mais l'estampille COURANTE a divergé : chaque base a masqué H0 avec SA écriture.
    assert_ne!(r_orig_a.par_cle[0].estampille_courante, Some((auteur, horloge_orig)));
    assert_ne!(r_orig_b.par_cle[0].estampille_courante, Some((auteur, horloge_orig)));

    // (3) pas de fausse confirmation croisée : chaque divergence ne résout QUE chez elle.
    assert_eq!(r_diva_a.integrite, IntegriteTentative::Integrale);
    assert_eq!(r_diva_b.integrite, IntegriteTentative::Indeterminee);
    assert_eq!(r_divb_b.integrite, IntegriteTentative::Integrale);
    assert_eq!(r_divb_a.integrite, IntegriteTentative::Indeterminee);

    // (4) la valeur courante relue diffère : données RÉELLES de chaque base.
    match store_a.lire_etat_propriete(cible, "a").unwrap() {
        EtatPropriete::Presente { valeur, .. } => assert_eq!(valeur, Value::Int64(20)),
        autre => panic!("base A : attendu 20, lu {autre:?}"),
    }
    match store_b.lire_etat_propriete(cible, "a").unwrap() {
        EtatPropriete::Presente { valeur, .. } => assert_eq!(valeur, Value::Int64(30)),
        autre => panic!("base B : attendu 30, lu {autre:?}"),
    }

    eprintln!(
        "DIVERGENCE_OK : H0={horloge_orig} Engage dans A et B ; estampille_courante diverge ; \
         ref_div_A Engage en A / Indetermine en B ; ref_div_B Engage en B / Indetermine en A ; \
         valeurs A=20 B=30"
    );
}

// ── Scénario 2 : deux bases INDÉPENDANTES partageant writer.id ─────────────
//
// Copier le fichier writer.id d'une base neuve à l'autre : les deux bases ont
// le MÊME auteur. Après divergence, les estampilles HLC ne se rejoignent pas,
// et l'engagement n'est JAMAIS déduit d'une base à l'autre.
#[test]
fn deux_bases_independantes_partageant_writer_id() {
    let td = tempfile::tempdir().unwrap();
    let base_a = td.path().join("a");
    let base_b = td.path().join("b");

    // ── base A : nœud + publication a=10, référence ref_A ─────────────────
    let (ref_a, cible_a, auteur_a) = {
        let store = SubstrateStore::create(&base_a).unwrap();
        let id = store.create_node(&["n"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let pre = store.capturer_precondition(id, &["a"]).unwrap();
        let prep = store
            .preparer_transformation(id, &[("a", Value::Int64(10))], &pre, ModePublication::Strict)
            .unwrap();
        let reference = prep.reference_tentative();
        store.publier_si_precondition_intacte(prep).unwrap();
        let t = (reference.to_bytes(), reference.cible(), reference.auteur());
        store.close().unwrap();
        t
    };

    // ── base B neuve : on pose son writer.id, puis on l'ÉCRASE par celui de A ──
    {
        let store = SubstrateStore::create(&base_b).unwrap();
        store.close().unwrap();
    }
    std::fs::copy(
        base_a.join(obrain_substrate::writer_id::FICHIER_WRITER_ID),
        base_b.join(obrain_substrate::writer_id::FICHIER_WRITER_ID),
    )
    .unwrap();
    assert_eq!(
        lire_writer_id(&base_a),
        lire_writer_id(&base_b),
        "le fichier writer.id est partagé byte-à-byte"
    );

    // ── base B (writer.id = celui de A) : nœud + publication a=100 ────────
    std::thread::sleep(std::time::Duration::from_millis(20));
    let (ref_b, cible_b, auteur_b) = {
        let store = SubstrateStore::open(&base_b).unwrap();
        let id = store.create_node(&["n"]);
        store.set_node_property(id, "a", Value::Int64(50));
        let pre = store.capturer_precondition(id, &["a"]).unwrap();
        let prep = store
            .preparer_transformation(id, &[("a", Value::Int64(100))], &pre, ModePublication::Strict)
            .unwrap();
        let reference = prep.reference_tentative();
        store.publier_si_precondition_intacte(prep).unwrap();
        let t = (reference.to_bytes(), reference.cible(), reference.auteur());
        store.close().unwrap();
        t
    };

    // ── identité partagée, incarnation distincte par horloge ──────────────
    assert_eq!(auteur_a, auteur_b, "les deux bases partagent le writer.id copié");
    assert_eq!(cible_a, cible_b, "chaque base alloue son premier nœud à NodeId(1)");

    let ref_a = ReferenceTentative::from_bytes(&ref_a).unwrap();
    let ref_b = ReferenceTentative::from_bytes(&ref_b).unwrap();
    assert_ne!(
        ref_a.horloge(),
        ref_b.horloge(),
        "après divergence, les estampilles HLC diffèrent (non-collision)"
    );

    let store_a = SubstrateStore::open(&base_a).unwrap();
    let store_b = SubstrateStore::open(&base_b).unwrap();

    let res_aa = store_a.resoudre_reference(&ref_a).unwrap();
    let res_ab = store_b.resoudre_reference(&ref_a).unwrap();
    let res_bb = store_b.resoudre_reference(&ref_b).unwrap();
    let res_ba = store_a.resoudre_reference(&ref_b).unwrap();

    // engagement dans SA base, jamais déduit dans l'autre
    assert_eq!(res_aa.integrite, IntegriteTentative::Integrale);
    assert_eq!(res_bb.integrite, IntegriteTentative::Integrale);
    assert_eq!(
        res_ab.integrite,
        IntegriteTentative::Indeterminee,
        "l'engagement en A n'est pas déduit en B"
    );
    assert_eq!(
        res_ba.integrite,
        IntegriteTentative::Indeterminee,
        "l'engagement en B n'est pas déduit en A"
    );

    // l'auteur_resolvant ne discrimine PAS les bases : il vaut W dans les deux.
    assert_eq!(res_aa.auteur_resolvant, auteur_a);
    assert_eq!(res_bb.auteur_resolvant, auteur_b);
    assert_eq!(
        res_aa.auteur_resolvant, res_bb.auteur_resolvant,
        "writer.id partagé → auteur_resolvant identique : l'auteur n'est pas une identité de base"
    );

    eprintln!(
        "PARTAGE_OK : auteur identique (writer.id copié), cible identique (NodeId 1), \
         horloge différente → ref_A Indetermine en B, ref_B Indetermine en A ; \
         auteur_resolvant identique dans les deux bases"
    );
}

// ── Scénario 3 : référence engagée AVANT suppression de la cible ───────────
//
// Publier, puis supprimer le nœud, puis résoudre. La résolution vérifie la
// VIVACITÉ de la cible AVANT de consulter la chaîne : une cible non vivante
// rend Indetermine, même si l'entrée historique peut survivre dans la chaîne.
#[test]
fn reference_engagee_avant_suppression_est_indeterminee() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(&td.path().join("kb")).unwrap();
    let id = store.create_node(&["ressource"]);
    store.set_node_property(id, "a", Value::Int64(1));
    let pre = store.capturer_precondition(id, &["a"]).unwrap();
    let prep = store
        .preparer_transformation(id, &[("a", Value::Int64(10))], &pre, ModePublication::Strict)
        .unwrap();
    let reference = prep.reference_tentative();
    store.publier_si_precondition_intacte(prep).unwrap();

    // avant suppression : engagé
    let avant = store.resoudre_reference(&reference).unwrap();
    assert_eq!(avant.integrite, IntegriteTentative::Integrale);
    assert_eq!(avant.par_cle[0].etat, ResolutionEffet::Engage);

    // supprimer la cible, puis résoudre
    assert!(store.delete_node(id), "delete_node doit confirmer la vivacité");
    let apres = store.resoudre_reference(&reference).unwrap();
    assert_eq!(
        apres.integrite,
        IntegriteTentative::Indeterminee,
        "cible non vivante → Indetermine (pas Engage), même si l'entrée survit dans la chaîne"
    );
    assert_eq!(apres.par_cle[0].etat, ResolutionEffet::Indetermine);
    assert_eq!(
        apres.par_cle[0].estampille_courante, None,
        "nœud non vivant → aucune estampille courante"
    );
    // Distingue « nœud non vivant » de « entrée compactée sur un nœud vivant » :
    match store.lire_etat_propriete(id, "a").unwrap() {
        EtatPropriete::RessourceInexistante => {}
        autre => panic!("nœud supprimé : attendu RessourceInexistante, lu {autre:?}"),
    }

    eprintln!(
        "SUPPRESSION_OK : Engage AVANT suppression, Indetermine APRÈS — \
         la résolution vérifie la vivacité de la cible AVANT de consulter la chaîne"
    );
}

// ── Scénario 4 : recréation de cible + non-réutilisation du NodeId ─────────
//
// Un NodeId est alloué monotone et jamais réutilisé : la recréation reçoit un
// NodeId DIFFÉRENT. Un NodeId vivant ne prouve donc PAS une incarnation
// identique — la référence de l'incarnation morte reste Indeterminee.
#[test]
fn recreation_de_cible_renouvelle_le_node_id() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(&td.path().join("kb")).unwrap();

    let id_avant = store.create_node(&["ressource"]);
    store.set_node_property(id_avant, "a", Value::Int64(1));
    let pre = store.capturer_precondition(id_avant, &["a"]).unwrap();
    let prep = store
        .preparer_transformation(id_avant, &[("a", Value::Int64(10))], &pre, ModePublication::Strict)
        .unwrap();
    let reference = prep.reference_tentative();
    store.publier_si_precondition_intacte(prep).unwrap();

    // suppression puis recréation
    assert!(store.delete_node(id_avant));
    let id_apres = store.create_node(&["ressource"]);

    // identifiants avant / après : jamais réutilisés
    assert_ne!(
        id_avant, id_apres,
        "un NodeId supprimé n'est pas réutilisé à la recréation"
    );
    assert!(
        id_apres.as_u64() > id_avant.as_u64(),
        "allocation monotone : nouvelle incarnation = nouveau NodeId"
    );

    // la référence de l'incarnation morte est indéterminée
    let res = store.resoudre_reference(&reference).unwrap();
    assert_eq!(res.integrite, IntegriteTentative::Indeterminee);

    // un NodeId VIVANT ne prouve pas la même incarnation : id_apres est vivant,
    // mais la référence vise id_avant (mort). Une référence qui vise id_apres
    // s'engage — preuve que id_apres est une incarnation DISTINCTE et vivante.
    store.set_node_property(id_apres, "b", Value::Int64(2));
    let pre2 = store.capturer_precondition(id_apres, &["b"]).unwrap();
    let prep2 = store
        .preparer_transformation(id_apres, &[("b", Value::Int64(20))], &pre2, ModePublication::Strict)
        .unwrap();
    let reference2 = prep2.reference_tentative();
    store.publier_si_precondition_intacte(prep2).unwrap();
    let res2 = store.resoudre_reference(&reference2).unwrap();
    assert_eq!(res2.integrite, IntegriteTentative::Integrale);

    eprintln!(
        "RECREATION_OK : id_avant={} id_apres={} — NodeId monotone, jamais réutilisé ; \
         la référence de l'incarnation morte est Indeterminee, la nouvelle incarnation Engage",
        id_avant.as_u64(),
        id_apres.as_u64()
    );
}
