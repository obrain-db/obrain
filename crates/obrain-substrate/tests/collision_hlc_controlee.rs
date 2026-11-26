//! ── COLLISION HLC CONTRÔLÉE — deux bases, même `writer.id`, même horloge ──
//!
//! Démonstration DÉTERMINISTE (sans temporisation) de l'ambiguïté d'une
//! [`ReferenceTentative`] quand deux bases INDÉPENDANTES partagent le même
//! `writer.id` (copie du fichier) et produisent la MÊME estampille `(auteur,
//! horloge)` via le seam [`SubstrateStore::forcer_horloge_pour_test`].
//!
//! Les cinq identités à distinguer (contrat d'identité backend) :
//!   * DOMAINE DE STOCKAGE  = la base (répertoire). N'apparaît NULLE PART dans
//!     la référence ; le consommateur doit le conserver HORS-BANDE.
//!   * ÉCRIVAIN             = `writer_id` persisté dans `writer.id` (auteur).
//!     N'est PAS une identité de base : deux bases peuvent le partager par
//!     copie du fichier, et il ne garantit PAS l'unicité inter-bases.
//!   * INCARNATION DE RESSOURCE = `cible` (NodeId, alloué monotone LOCALEMENT).
//!     Deux bases allouent chacune leur premier nœud à `NodeId(1)`.
//!   * OPÉRATION LOGIQUE    = l'intention du consommateur (la transformation
//!     voulue). ABSENTE de la référence : rien ne la nomme.
//!   * TENTATIVE PHYSIQUE   = l'entrée estampillée réellement posée dans la
//!     chaîne canonique d'UNE base. C'est ce que résout `resoudre_reference`.
//!
//! La collision prouve : deux TENTATIVES PHYSIQUES distinctes (valeurs
//! différentes) produisent des références BYTE-IDENTIQUES, qui résolvent
//! `Engage` dans les DEUX bases. Une référence n'identifie donc PAS une
//! opération logique unique : elle n'identifie qu'une estampille `(auteur,
//! horloge)` LOCALE à une base — et cette localité n'est pas portée par la
//! référence.

use obrain_common::types::{NodeId, Value};
use obrain_core::graph::GraphStoreMut as _;
use obrain_substrate::store::{
    EtatPropriete, IntegriteTentative, ModePublication, ResolutionEffet,
};
use obrain_substrate::{ReferenceTentative, SubstrateStore};

/// Estampille HLC forcée, identique dans les deux bases : `physique = 2^44`
/// (au-delà de toute horloge murale plausible ; le champ physique tient sur
/// 48 bits), `logique = 0x42` — arbitraire mais FIXE, pour une collision
/// déterministe sans temporisation.
const HORLOGE_FORCEE: u64 = (1u64 << 44) << 16 | 0x42;

/// Lit `writer.id` (8 octets LE) — observation directe du writer_id persisté.
fn lire_writer_id(base: &std::path::Path) -> u64 {
    let octets = std::fs::read(base.join(obrain_substrate::writer_id::FICHIER_WRITER_ID))
        .expect("writer.id présent dans la base");
    u64::from_le_bytes(octets.as_slice().try_into().expect("8 octets"))
}

/// Écrase le `writer.id` de `dst` par celui de `src` (partage d'identité).
fn copier_writer_id(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::copy(
        src.join(obrain_substrate::writer_id::FICHIER_WRITER_ID),
        dst.join(obrain_substrate::writer_id::FICHIER_WRITER_ID),
    )
    .unwrap();
}

/// Copie byte-à-byte du CONTENU d'un répertoire de base (tous les fichiers de
/// premier niveau, y compris `writer.id` et `substrate.meta`).
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

/// Prépare + publie une transformation sur `cle`, d'estampille HLC FORCÉE
/// (via le seam), et rend sa [`ReferenceTentative`].
fn publier_avec_horloge_forcee(
    store: &SubstrateStore,
    cible: NodeId,
    cle: &str,
    valeur: i64,
) -> ReferenceTentative {
    let pre = store.capturer_precondition(cible, &[cle]).unwrap();
    store.forcer_horloge_pour_test(HORLOGE_FORCEE);
    let prep = store
        .preparer_transformation(cible, &[(cle, Value::Int64(valeur))], &pre, ModePublication::Strict)
        .unwrap();
    let reference = prep.reference_tentative();
    store.publier_si_precondition_intacte(prep).unwrap();
    reference
}

// ── Scénario 1 : la collision contrôlée (cœur de la démonstration) ─────────
//
// Deux bases indépendantes, même `writer.id` (copie), même horloge FORCÉE :
// deux tentatives physiques distinctes (a=10 dans A, a=20 dans B) produisent
// des références BYTE-IDENTIQUES, qui résolvent `Engage` dans les DEUX bases.
// C'est le FAUX POSITIF CROISÉ : une base peut être confondue avec l'autre.
#[test]
fn collision_hlc_controlee_faux_positif_croise() {
    let td = tempfile::tempdir().unwrap();
    let base_a = td.path().join("a");
    let base_b = td.path().join("b");

    // base A : writer.id propre, premier nœud, propriété initiale.
    let cible_a = {
        let store = SubstrateStore::create(&base_a).unwrap();
        let id = store.create_node(&["n"]);
        store.set_node_property(id, "a", Value::Int64(1));
        store.close().unwrap();
        id
    };

    // base B : neuve, puis ÉCRASER son writer.id par celui de A (partage).
    {
        let store = SubstrateStore::create(&base_b).unwrap();
        store.close().unwrap();
    }
    copier_writer_id(&base_a, &base_b);
    assert_eq!(lire_writer_id(&base_a), lire_writer_id(&base_b), "writer.id partagé byte-à-byte");

    // Dans CHAQUE base : une écriture d'estampille HLC FORCÉE (même horloge).
    let ref_a = {
        let store = SubstrateStore::open(&base_a).unwrap();
        let r = publier_avec_horloge_forcee(&store, cible_a, "a", 10);
        store.close().unwrap();
        r
    };
    let (ref_b, cible_b) = {
        let store = SubstrateStore::open(&base_b).unwrap();
        let id = store.create_node(&["n"]);
        store.set_node_property(id, "a", Value::Int64(5));
        let r = publier_avec_horloge_forcee(&store, id, "a", 20);
        store.close().unwrap();
        (r, id)
    };

    // mêmes (auteur, horloge, cible, effets) → références STRUCTURELLEMENT
    // et BYTE-IDENTIQUES, pourtant issues de DEUX tentatives physiques.
    assert_eq!(ref_a.auteur(), ref_b.auteur(), "writer.id partagé");
    assert_eq!(ref_a.horloge(), ref_b.horloge(), "horloge forcée identique");
    assert_eq!(ref_a.horloge(), HORLOGE_FORCEE, "l'horloge forcée est celle de la référence");
    assert_eq!(ref_a.cible(), cible_b, "chaque base alloue son premier nœud à NodeId(1)");
    assert_eq!(ref_a.effets(), ref_b.effets());
    assert_eq!(ref_a.to_bytes(), ref_b.to_bytes(), "références BYTE-IDENTIQUES");

    // résolution dans chaque base
    let store_a = SubstrateStore::open(&base_a).unwrap();
    let store_b = SubstrateStore::open(&base_b).unwrap();
    let r_aa = store_a.resoudre_reference(&ref_a).unwrap();
    let r_ab = store_b.resoudre_reference(&ref_a).unwrap();

    // (1) la MÊME référence résout Engage dans les DEUX bases, indépendamment.
    assert_eq!(r_aa.integrite, IntegriteTentative::Integrale);
    assert_eq!(r_ab.integrite, IntegriteTentative::Integrale);
    assert_eq!(r_aa.par_cle[0].etat, ResolutionEffet::Engage);
    assert_eq!(r_ab.par_cle[0].etat, ResolutionEffet::Engage);

    // (2) FAUX POSITIF CROISÉ : la référence émise en A résout À TORT en B —
    //     et pourtant les deux tentatives écrivent des valeurs DIFFÉRENTES.
    match store_a.lire_etat_propriete(cible_a, "a").unwrap() {
        EtatPropriete::Presente { valeur, .. } => assert_eq!(valeur, Value::Int64(10)),
        autre => panic!("base A : attendu 10, lu {autre:?}"),
    }
    match store_b.lire_etat_propriete(cible_b, "a").unwrap() {
        EtatPropriete::Presente { valeur, .. } => assert_eq!(valeur, Value::Int64(20)),
        autre => panic!("base B : attendu 20, lu {autre:?}"),
    }

    // (3) auteur_resolvant identique : writer.id ne discrimine PAS les bases.
    assert_eq!(r_aa.auteur_resolvant, r_ab.auteur_resolvant);

    // (4) le seul écart observable entre les deux résolutions est la valeur
    //     courante — qui n'appartient PAS à l'identité de la référence.
    assert_eq!(r_aa.par_cle[0].valeur_courante, Some(Value::Int64(10)));
    assert_eq!(r_ab.par_cle[0].valeur_courante, Some(Value::Int64(20)));

    eprintln!(
        "COLLISION_OK : ref (auteur={:#x}, horloge={:#x}, cible={}) Engage dans A ET B ; \
         valeurs A=10 B=20 — la référence ne distingue PAS les deux bases",
        ref_a.auteur(),
        ref_a.horloge(),
        ref_a.cible().as_u64()
    );
}

// ── Scénario 2 : même (auteur, horloge, cible) mais CLÉ différente ─────────
//
// Le faux positif exige que l'autre base ait écrit la MÊME clé sous la même
// estampille. Même `writer.id`, même horloge forcée, même cible — mais la base
// B écrit `b` au lieu de `a` : la référence de A (clé `a`) reste Indetermine
// en B. La résolution est donc par (cible, clé, auteur, horloge) — pas par
// l'identité d'écrivain seule.
#[test]
fn collision_hlc_sans_ecriture_de_la_cle_reste_indeterminee() {
    let td = tempfile::tempdir().unwrap();
    let base_a = td.path().join("a");
    let base_b = td.path().join("b");

    let ref_a = {
        let store = SubstrateStore::create(&base_a).unwrap();
        let id = store.create_node(&["n"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let r = publier_avec_horloge_forcee(&store, id, "a", 10);
        store.close().unwrap();
        r
    };

    // base B : writer.id partagé, même horloge forcée, mais on écrit "b".
    {
        let store = SubstrateStore::create(&base_b).unwrap();
        store.close().unwrap();
    }
    copier_writer_id(&base_a, &base_b);
    {
        let store = SubstrateStore::open(&base_b).unwrap();
        let id = store.create_node(&["n"]);
        store.set_node_property(id, "b", Value::Int64(5));
        let _ = publier_avec_horloge_forcee(&store, id, "b", 50);
        store.close().unwrap();
    }

    let store_a = SubstrateStore::open(&base_a).unwrap();
    let store_b = SubstrateStore::open(&base_b).unwrap();

    let r_aa = store_a.resoudre_reference(&ref_a).unwrap();
    let r_ab = store_b.resoudre_reference(&ref_a).unwrap();
    assert_eq!(r_aa.integrite, IntegriteTentative::Integrale);
    assert_eq!(
        r_ab.integrite,
        IntegriteTentative::Indeterminee,
        "même (auteur, horloge, cible) mais clé non écrite en B → Indetermine"
    );
    assert_eq!(r_ab.par_cle[0].etat, ResolutionEffet::Indetermine);

    eprintln!("CONTRASTE_OK : collision sur (auteur, horloge, cible) mais clé différente → Indetermine en B");
}

// ── Scénario 3 : déplacement du répertoire ─────────────────────────────────
//
// `writer.id` est un FICHIER DANS le répertoire : déplacer le répertoire le
// déplace avec lui. L'auteur est donc stable, et une référence émise avant le
// déplacement résout après — le chemin n'appartient pas à l'identité.
#[test]
fn deplacement_du_repertoire_preserve_l_identite_et_la_resolution() {
    let td = tempfile::tempdir().unwrap();
    let origine = td.path().join("avant");
    let deplacee = td.path().join("apres");

    let (octets, auteur) = {
        let store = SubstrateStore::create(&origine).unwrap();
        let id = store.create_node(&["n"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let pre = store.capturer_precondition(id, &["a"]).unwrap();
        let prep = store
            .preparer_transformation(id, &[("a", Value::Int64(10))], &pre, ModePublication::Strict)
            .unwrap();
        let reference = prep.reference_tentative();
        store.publier_si_precondition_intacte(prep).unwrap();
        let t = (reference.to_bytes(), reference.auteur());
        store.close().unwrap();
        t
    };

    // déplacer le RÉPERTOIRE ENTIER
    std::fs::rename(&origine, &deplacee).unwrap();
    assert!(!origine.exists() && deplacee.exists());

    let store = SubstrateStore::open(&deplacee).unwrap();
    let reference = ReferenceTentative::from_bytes(&octets).unwrap();
    let res = store.resoudre_reference(&reference).unwrap();
    assert_eq!(res.integrite, IntegriteTentative::Integrale);
    assert_eq!(res.auteur_resolvant, auteur, "writer.id a suivi le répertoire");

    eprintln!("DEPLACEMENT_OK : auteur stable après rename du répertoire ; référence Engage");
}

// ── Scénario 4 : restauration depuis sauvegarde ────────────────────────────
//
// Une sauvegarde est une copie du répertoire (writer.id compris). Restaurer
// revient à écraser la base par la copie : l'écriture ANTÉRIEURE à la
// sauvegarde résout, celle POSTÉRIEURE est perdue (Indetermine). L'auteur
// reste identique : writer.id ne porte aucune notion de « fraîcheur ».
#[test]
fn restauration_depuis_sauvegarde_perd_les_ecritures_posterieures() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    let sauvegarde = td.path().join("sauvegarde");

    let (ref_pre, cible, auteur) = {
        let store = SubstrateStore::create(&base).unwrap();
        let id = store.create_node(&["n"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let pre = store.capturer_precondition(id, &["a"]).unwrap();
        let prep = store
            .preparer_transformation(id, &[("a", Value::Int64(10))], &pre, ModePublication::Strict)
            .unwrap();
        let reference = prep.reference_tentative();
        store.publier_si_precondition_intacte(prep).unwrap();
        let t = (reference.to_bytes(), id, reference.auteur());
        store.close().unwrap();
        t
    };

    // sauvegarde (copie du répertoire entier, writer.id compris)
    copier_base(&base, &sauvegarde);

    // écriture POSTÉRIEURE à la sauvegarde (même nœud, valeur 99)
    let ref_post = {
        let store = SubstrateStore::open(&base).unwrap();
        let pre = store.capturer_precondition(cible, &["a"]).unwrap();
        let prep = store
            .preparer_transformation(cible, &[("a", Value::Int64(99))], &pre, ModePublication::Strict)
            .unwrap();
        let reference = prep.reference_tentative();
        store.publier_si_precondition_intacte(prep).unwrap();
        let t = reference.to_bytes();
        store.close().unwrap();
        t
    };

    // restauration : écraser la base par la sauvegarde
    std::fs::remove_dir_all(&base).unwrap();
    copier_base(&sauvegarde, &base);

    let store = SubstrateStore::open(&base).unwrap();
    let r_pre = store
        .resoudre_reference(&ReferenceTentative::from_bytes(&ref_pre).unwrap())
        .unwrap();
    let r_post = store
        .resoudre_reference(&ReferenceTentative::from_bytes(&ref_post).unwrap())
        .unwrap();

    assert_eq!(
        r_pre.integrite,
        IntegriteTentative::Integrale,
        "l'écriture antérieure à la sauvegarde a été restaurée"
    );
    assert_eq!(
        r_post.integrite,
        IntegriteTentative::Indeterminee,
        "l'écriture postérieure à la sauvegarde a été perdue"
    );
    assert_eq!(r_pre.auteur_resolvant, auteur, "writer.id restauré est identique");

    eprintln!("RESTAURATION_OK : écriture pré-sauvegarde Engage, post-sauvegarde Indetermine ; auteur inchangé");
}
