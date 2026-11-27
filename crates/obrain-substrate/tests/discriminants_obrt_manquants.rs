//! ── DISCRIMINANTS COMPORTEMENTAUX MANQUANTS (OBRT #334/#418/#429) ──────────
//!
//! Témoin EXTERNE des contrats de résolution transactionnelle de
//! [`ReferenceTentative`] : crate d'intégration, aucun accès privilégié — tout
//! passe par l'API publique (`resoudre_reference`, `effet(i)`, `verdict_effet(i)`,
//! `effets()`, `nb_effets()`), jamais par un champ rendu `pub`.
//!
//! Les discriminants DÉJÀ couverts par `contre_epreuves_v2.rs` et
//! `reference_tentative_externe.rs` ne sont pas redoublés ici :
//!   * référence étrangère  → `reference_etrangere_ne_produit_jamais_engage`
//!   * référence altérée    → `reference_alteree_ne_produit_jamais_engage` +
//!     `alteration_partielle_est_signalee_comme_partielle`
//!   * perte de réponse     → `reference_survit_a_la_perte_de_reponse_et_au_masquage`
//!
//! Ce fichier ajoute les SEPT discriminants manquants :
//!   1. origine attendue indépendante (pas d'origine unique pré-câblée)
//!   4. référence legacy v1 (décodée, `domaine == 0`, acceptée et résolue)
//!   5. compaction des preuves (plusieurs preuves d'un même effet, sans perte)
//!   6. intégrité multi-effets (ordre préservé, aucun croisement)
//!   7. recalcul (déterminisme : deux résolutions → même verdict)
//!   8. 10 000 conflits (résolus sans dégradation d'invariant, temps borné)
//!  10. `effet(i)` / `verdict_effet(i)` hors bornes → PANIQUE (contrat exact)

use obrain_common::types::{NodeId, Value};
use obrain_core::graph::GraphStoreMut as _;
use obrain_substrate::store::{
    EtatPropriete, IntegriteTentative, ModePublication, ResolutionEffet, VerdictEffet,
};
use obrain_substrate::{ReferenceTentative, SubstrateStore};

/// Encode une référence « à la main » au format v1 (sans domaine) — ce que
/// ferait un ancien client. Le décodage DOIT l'accepter ; la résolution ne doit
/// jamais en tirer un `Engage` faux.
fn encoder_v1(auteur: u64, horloge: u64, cible: NodeId, cles: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"OBRT");
    out.push(1);
    out.extend_from_slice(&auteur.to_le_bytes());
    out.extend_from_slice(&horloge.to_le_bytes());
    out.extend_from_slice(&cible.0.to_le_bytes());
    out.extend_from_slice(&(cles.len() as u32).to_le_bytes());
    for c in cles {
        out.extend_from_slice(&(c.len() as u32).to_le_bytes());
        out.extend_from_slice(c.as_bytes());
    }
    out
}

/// Encode une référence « à la main » au format v2 (avec domaine), comme le
/// produit [`ReferenceTentative::to_bytes`]. Sert à forger une référence qui
/// porte le DOMAINE du store mais une origine (auteur/horloge) arbitraire.
fn encoder_v2(auteur: u64, horloge: u64, cible: NodeId, domaine: u128, cles: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"OBRT");
    out.push(2);
    out.extend_from_slice(&auteur.to_le_bytes());
    out.extend_from_slice(&horloge.to_le_bytes());
    out.extend_from_slice(&cible.0.to_le_bytes());
    out.extend_from_slice(&domaine.to_be_bytes());
    out.extend_from_slice(&(cles.len() as u32).to_le_bytes());
    for c in cles {
        out.extend_from_slice(&(c.len() as u32).to_le_bytes());
        out.extend_from_slice(c.as_bytes());
    }
    out
}

/// ── 1. ORIGINE ATTENDUE INDÉPENDANTE ───────────────────────────────────────
///
/// Une résolution n'exige pas une origine unique pré-câblée : le verdict se
/// dérive de l'origine PORTÉE PAR la référence (auteur + horloge), jamais d'une
/// origine « attendue » codée en dur dans le store. Toute origine arbitraire
/// est acceptée et résolue (`Indetermine` si aucune entrée ne la porte, `Engage`
/// sinon) ; `auteur_resolvant()` rapporte l'identité du RÉSOLVEUR, indépendante
/// de l'origine de la référence.
#[test]
fn resolution_independante_de_toute_origine_pre_cablee() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(td.path().join("kb")).unwrap();
    let id = store.create_node(&["n"]);
    store.set_node_property(id, "a", Value::Int64(1));

    let pre = store.capturer_precondition(id, &["a"]).unwrap();
    let prep = store
        .preparer_transformation(id, &[("a", Value::Int64(2))], &pre, ModePublication::Strict)
        .unwrap();
    let legitime = prep.reference_tentative();
    store.publier_si_precondition_intacte(prep).unwrap();

    let auteur_reel = legitime.auteur();
    let horloge_reelle = legitime.horloge();
    let domaine = store.domaine_id();
    let autre_auteur = auteur_reel.wrapping_add(1);
    assert_ne!(autre_auteur, auteur_reel);

    // (1) une origine ARBITRAIRE (auteur jamais vu par ce store), même domaine,
    //     même horloge : Indetermine — jamais un Engage, jamais une erreur.
    let etrangere = ReferenceTentative::from_bytes(&encoder_v2(
        autre_auteur,
        horloge_reelle,
        id,
        domaine,
        &["a"],
    ))
    .unwrap();
    let res_etrangere = store.resoudre_reference(&etrangere).unwrap();
    assert_eq!(res_etrangere.integrite(), IntegriteTentative::Indeterminee);
    assert_eq!(res_etrangere.effet(0).etat(), ResolutionEffet::Indetermine);

    // (2) même auteur, horloge jamais utilisée : Indetermine — c'est l'origine
    //     COMPLÈTE (auteur, horloge) qui discrimine, pas un auteur seul.
    let autre_horloge = ReferenceTentative::from_bytes(&encoder_v2(
        auteur_reel,
        horloge_reelle.wrapping_add(1),
        id,
        domaine,
        &["a"],
    ))
    .unwrap();
    let res_autre = store.resoudre_reference(&autre_horloge).unwrap();
    assert_eq!(res_autre.integrite(), IntegriteTentative::Indeterminee);

    // (3) la référence LÉGITIME résout Engage, sans qu'aucune origine n'ait été
    //     pré-câblée : c'est la référence elle-même qui porte son origine.
    let res_legitime = store.resoudre_reference(&legitime).unwrap();
    assert_eq!(res_legitime.integrite(), IntegriteTentative::Integrale);
    assert_eq!(res_legitime.effet(0).etat(), ResolutionEffet::Engage);

    // (4) auteur_resolvant() est l'identité du RÉSOLVEUR (constante du store),
    //     identique pour toutes les résolutions, INDÉPENDANTE de l'origine de
    //     la référence (auteur arbitraire ou légitime).
    assert_eq!(res_etrangere.auteur_resolvant(), auteur_reel);
    assert_eq!(res_autre.auteur_resolvant(), auteur_reel);
    assert_eq!(res_legitime.auteur_resolvant(), auteur_reel);
}

/// ── 4. RÉFÉRENCE LEGACY v1 ─────────────────────────────────────────────────
///
/// Une `ReferenceTentative` v1 (sans domaine) est DÉCODÉE et ACCEPTÉE, porte
/// `domaine == 0` (« domaine non spécifié, legacy »), et la résolution procède
/// par (cible, clé, auteur, horloge) — l'entrée réellement présente est rendue
/// `Engage`. C'est la limite historique documentée, jamais une confirmation
/// fictive.
#[test]
fn reference_v1_legacy_est_decodee_acceptee_et_resolue() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(td.path().join("kb")).unwrap();
    let id = store.create_node(&["n"]);
    store.set_node_property(id, "a", Value::Int64(1));

    let pre = store.capturer_precondition(id, &["a"]).unwrap();
    let prep = store
        .preparer_transformation(id, &[("a", Value::Int64(2))], &pre, ModePublication::Strict)
        .unwrap();
    let v2 = prep.reference_tentative();
    store.publier_si_precondition_intacte(prep).unwrap();

    // la v2 courante porte un domaine non nul.
    assert_ne!(
        v2.domaine(),
        0,
        "une référence v2 porte toujours un domaine"
    );

    // ré-encode la MÊME tentative au format v1 (sans domaine) — ce que ferait
    // un ancien client qui n'a pas le champ domaine.
    let cles: Vec<&str> = v2.effets().iter().map(|s| s.as_str()).collect();
    let legacy =
        ReferenceTentative::from_bytes(&encoder_v1(v2.auteur(), v2.horloge(), v2.cible(), &cles))
            .unwrap();

    // décodée et acceptée, domaine == 0 (legacy), champs préservés.
    assert_eq!(
        legacy.domaine(),
        0,
        "une référence v1 décodée a domaine == 0 (legacy)"
    );
    assert_eq!(legacy.auteur(), v2.auteur());
    assert_eq!(legacy.horloge(), v2.horloge());
    assert_eq!(legacy.cible(), v2.cible());
    assert_eq!(legacy.effets(), v2.effets());

    // elle résout Engage : la garde de domaine ne la refuse pas (domaine == 0),
    // et l'entrée est réellement présente dans la chaîne canonique.
    let res = store.resoudre_reference(&legacy).unwrap();
    assert_eq!(res.integrite(), IntegriteTentative::Integrale);
    assert_eq!(res.effet(0).etat(), ResolutionEffet::Engage);
    assert_eq!(res.verdict_effet(0), VerdictEffet::EngageEtCourant);
}

/// ── 5. COMPACTION DES PREUVES ──────────────────────────────────────────────
///
/// Plusieurs preuves (entrées) d'un même effet coexistent dans la chaîne
/// canonique ; la résolution les « compacte » en UN verdict par référence sans
/// en perdre AUCUNE : chaque tentative reste résolvable (`Engage`), et le
/// verdict distingue la preuve courante des preuves supplantées.
#[test]
fn plusieurs_preuves_d_un_meme_effet_se_compactent_sans_perte_de_verdict() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(td.path().join("kb")).unwrap();
    let id = store.create_node(&["n"]);
    store.set_node_property(id, "a", Value::Int64(1));

    // trois tentatives successives sur la MÊME clé : trois preuves, trois
    // horloges distinctes, aucune n'écrase l'entrée de la précédente.
    let mut references = Vec::new();
    for valeur in [10i64, 20, 30] {
        let pre = store.capturer_precondition(id, &["a"]).unwrap();
        let prep = store
            .preparer_transformation(
                id,
                &[("a", Value::Int64(valeur))],
                &pre,
                ModePublication::Strict,
            )
            .unwrap();
        let reference = prep.reference_tentative();
        store.publier_si_precondition_intacte(prep).unwrap();
        references.push(reference);
    }
    assert!(
        references[0].horloge() < references[1].horloge(),
        "horloges strictement croissantes"
    );
    assert!(
        references[1].horloge() < references[2].horloge(),
        "horloges strictement croissantes"
    );

    // CHAQUE preuve résout Engage : aucune n'est perdue par la preuve suivante.
    for reference in &references {
        let res = store.resoudre_reference(reference).unwrap();
        assert_eq!(res.integrite(), IntegriteTentative::Integrale);
        assert_eq!(res.effet(0).etat(), ResolutionEffet::Engage);
    }

    // le verdict compacte les trois preuves sans perte : les deux premières
    // sont SUMPLANTÉES, la dernière est COURANTE.
    assert_eq!(
        store
            .resoudre_reference(&references[0])
            .unwrap()
            .verdict_effet(0),
        VerdictEffet::EngagePuisSupplante
    );
    assert_eq!(
        store
            .resoudre_reference(&references[1])
            .unwrap()
            .verdict_effet(0),
        VerdictEffet::EngagePuisSupplante
    );
    assert_eq!(
        store
            .resoudre_reference(&references[2])
            .unwrap()
            .verdict_effet(0),
        VerdictEffet::EngageEtCourant
    );

    // la valeur courante est celle de la DERNIÈRE preuve.
    match store.lire_etat_propriete(id, "a").unwrap() {
        EtatPropriete::Presente { valeur, .. } => assert_eq!(valeur, Value::Int64(30)),
        autre => panic!("état inattendu : {autre:?}"),
    }
}

/// ── 6. INTÉGRITÉ MULTI-EFFETS ──────────────────────────────────────────────
///
/// Un `ResolutionTentative` à N effets rend `nb_effets() == N`, chaque
/// `effet(i)` est cohérent avec la i-ème clé de la référence (l'ordre est
/// préservé, aucun croisement), et l'agrégat d'intégrité est `Integrale`.
#[test]
fn integrite_multi_effets_preserve_l_ordre_sans_croisement() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(td.path().join("kb")).unwrap();
    let id = store.create_node(&["n"]);
    store.set_node_property(id, "a", Value::Int64(1));
    store.set_node_property(id, "b", Value::Int64(2));
    store.set_node_property(id, "c", Value::Int64(3));

    let pre = store.capturer_precondition(id, &["a", "b", "c"]).unwrap();
    let prep = store
        .preparer_transformation(
            id,
            &[
                ("a", Value::Int64(10)),
                ("b", Value::Int64(20)),
                ("c", Value::Int64(30)),
            ],
            &pre,
            ModePublication::Strict,
        )
        .unwrap();
    let reference = prep.reference_tentative();
    store.publier_si_precondition_intacte(prep).unwrap();

    assert_eq!(reference.effets().len(), 3);
    let res = store.resoudre_reference(&reference).unwrap();
    assert_eq!(res.nb_effets(), 3, "N effets → nb_effets() == N");
    assert_eq!(res.integrite(), IntegriteTentative::Integrale);

    // ordre préservé, aucun croisement : effet(i) correspond à la i-ème clé.
    for i in 0..3 {
        let e = res.effet(i);
        assert_eq!(
            e.cle(),
            reference.effets()[i].as_str(),
            "effet({i}) = {i}-ème clé, aucun croisement"
        );
        assert_eq!(e.etat(), ResolutionEffet::Engage);
        assert_eq!(res.verdict_effet(i), VerdictEffet::EngageEtCourant);
        assert_eq!(
            e.estampille_courante(),
            Some((reference.auteur(), reference.horloge()))
        );
    }
}

/// ── 7. RECALCUL / DÉTERMINISME ─────────────────────────────────────────────
///
/// Résoudre deux fois la MÊME référence sur le même état persisté (y compris
/// après une réouverture) rend le MÊME verdict, à l'octet près de l'observable
/// publique : la résolution est déterministe et non destructive.
#[test]
fn recalcul_deux_fois_meme_verdict_deterministe() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");

    let (reference, id) = {
        let store = SubstrateStore::create(&chemin).unwrap();
        let id = store.create_node(&["n"]);
        store.set_node_property(id, "a", Value::Int64(1));
        let pre = store.capturer_precondition(id, &["a"]).unwrap();
        let prep = store
            .preparer_transformation(id, &[("a", Value::Int64(2))], &pre, ModePublication::Strict)
            .unwrap();
        let reference = prep.reference_tentative();
        store.publier_si_precondition_intacte(prep).unwrap();
        store.close().unwrap();
        (reference, id)
    };

    let store = SubstrateStore::open(&chemin).unwrap();
    let r1 = store.resoudre_reference(&reference).unwrap();
    let r2 = store.resoudre_reference(&reference).unwrap();

    // même verdict, déterminisme strict de l'observable publique.
    assert_eq!(r1.integrite(), r2.integrite());
    assert_eq!(r1.cible(), r2.cible());
    assert_eq!(r1.auteur_resolvant(), r2.auteur_resolvant());
    assert_eq!(r1.nb_effets(), r2.nb_effets());
    assert_eq!(r1.effet(0).etat(), r2.effet(0).etat());
    assert_eq!(r1.verdict_effet(0), r2.verdict_effet(0));
    assert_eq!(
        r1.effet(0).estampille_courante(),
        r2.effet(0).estampille_courante()
    );
    assert_eq!(r1.effet(0).valeur_courante(), r2.effet(0).valeur_courante());

    // et le verdict ATTENDU, stable après réouverture.
    assert_eq!(r1.integrite(), IntegriteTentative::Integrale);
    assert_eq!(r1.verdict_effet(0), VerdictEffet::EngageEtCourant);
    assert_eq!(r1.effet(0).etat(), ResolutionEffet::Engage);

    // non destructif : une troisième résolution ne change rien.
    let r3 = store.resoudre_reference(&reference).unwrap();
    assert_eq!(r3.verdict_effet(0), VerdictEffet::EngageEtCourant);
    match store.lire_etat_propriete(id, "a").unwrap() {
        EtatPropriete::Presente { valeur, .. } => assert_eq!(valeur, Value::Int64(2)),
        autre => panic!("état inattendu : {autre:?}"),
    }
}

/// ── 8. 10 000 CONFLITS ─────────────────────────────────────────────────────
///
/// Stress : 10 000 tentatives préparées mais JAMAIS publiées (en conflit /
/// abandonnées), chacune résolue sans dégradation d'invariant — jamais un
/// `Engage` faux, toujours `Indeterminee` — et borné en temps raisonnable.
#[test]
fn dix_mille_tentatives_en_conflit_resolues_sans_degradation() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(td.path().join("kb")).unwrap();
    let id = store.create_node(&["n"]);
    store.set_node_property(id, "a", Value::Int64(1));
    let pre = store.capturer_precondition(id, &["a"]).unwrap();

    const N: usize = 10_000;
    let mut references = Vec::with_capacity(N);
    for _ in 0..N {
        let prep = store
            .preparer_transformation(id, &[("a", Value::Int64(2))], &pre, ModePublication::Lww)
            .unwrap();
        references.push(prep.reference_tentative());
        // NON publiée : tentative en conflit / abandonnée.
        drop(prep);
    }
    // horloges HLC distinctes et croissantes : 10 000 origines distinctes.
    for w in references.windows(2) {
        assert!(
            w[0].horloge() < w[1].horloge(),
            "horloges strictement croissantes"
        );
    }

    let debut = std::time::Instant::now();
    for reference in &references {
        let res = store.resoudre_reference(reference).unwrap();
        assert_eq!(
            res.integrite(),
            IntegriteTentative::Indeterminee,
            "jamais un Engage faux"
        );
        assert_eq!(res.effet(0).etat(), ResolutionEffet::Indetermine);
        assert_eq!(res.verdict_effet(0), VerdictEffet::Indetermine);
    }
    let dt = debut.elapsed();

    // invariant non dégradé : l'état persisté n'a pas bougé (a == 1).
    match store.lire_etat_propriete(id, "a").unwrap() {
        EtatPropriete::Presente { valeur, .. } => assert_eq!(valeur, Value::Int64(1)),
        autre => panic!("état inattendu : {autre:?}"),
    }

    eprintln!(
        "10000_CONFLITS : {N} tentatives en conflit résolues en {dt:?} ({:.0} ns/résolution)",
        dt.as_nanos() as f64 / N as f64
    );
    assert!(
        dt.as_secs() < 60,
        "10 000 résolutions bornées en temps raisonnable (mesuré {dt:?})"
    );
}

/// ── 10. `effet(i)` / `verdict_effet(i)` HORS BORNES ────────────────────────
///
/// Contrat EXACT tel qu'il est codé : ces deux accesseurs indexent directement
/// le `Vec` interne et PANIQUENT sur un indice hors bornes — pas d'`Option`,
/// pas d'erreur. Documenté et verrouillé tel quel (ce n'est pas une faille :
/// c'est un contrat d'indexation nommé dans la doc de `effet`).
#[test]
fn effet_et_verdict_hors_bornes_paniquent() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(td.path().join("kb")).unwrap();
    let id = store.create_node(&["n"]);
    store.set_node_property(id, "a", Value::Int64(1));

    let pre = store.capturer_precondition(id, &["a"]).unwrap();
    let prep = store
        .preparer_transformation(id, &[("a", Value::Int64(2))], &pre, ModePublication::Strict)
        .unwrap();
    let reference = prep.reference_tentative();
    store.publier_si_precondition_intacte(prep).unwrap();
    let res = store.resoudre_reference(&reference).unwrap();
    assert_eq!(res.nb_effets(), 1);

    // indice valide : aucune panique.
    assert_eq!(res.effet(0).cle(), "a");
    assert_eq!(res.verdict_effet(0), VerdictEffet::EngageEtCourant);

    // indice hors bornes : PANIQUE (contrat exact, pas d'Option ni d'erreur).
    use std::panic::{AssertUnwindSafe, catch_unwind};
    let effet_panique = catch_unwind(AssertUnwindSafe(|| {
        let _ = res.effet(1);
    }))
    .is_err();
    let verdict_panique = catch_unwind(AssertUnwindSafe(|| {
        let _ = res.verdict_effet(1);
    }))
    .is_err();
    let effet_lointain = catch_unwind(AssertUnwindSafe(|| {
        let _ = res.effet(usize::MAX);
    }))
    .is_err();
    assert!(
        effet_panique,
        "effet(1) hors bornes DOIT paniquer (indexation Vec directe)"
    );
    assert!(
        verdict_panique,
        "verdict_effet(1) hors bornes DOIT paniquer"
    );
    assert!(effet_lointain, "effet(usize::MAX) DOIT paniquer aussi");
}
