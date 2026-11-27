//! ── CONTRE-ÉPREUVES V2 — substitution, altération, complétude, attribution ───
//!
//! Contre `obrain@89d169d7`. Test d'INTÉGRATION : crate externe, aucun accès
//! privilégié. On n'éprouve pas le stockage seul : on éprouve ce qu'un
//! consommateur (this-harness) peut et ne peut pas conclure.
//!
//! ## Modèle de menace — écrit AVANT toute conclusion
//!
//! | adversaire | ce qu'il peut faire | ce qui le détecte |
//! |---|---|---|
//! | A1 corruption accidentelle | altérer les octets stockés par le consommateur | le codec (`from_bytes`) refuse ce qui est mal formé |
//! | A2 réécriture du medium du consommateur | écrire des octets BIEN formés | rien dans la bibliothèque : le consommateur doit authentifier son medium (clé hors de ce medium) |
//! | A3 erreur du consommateur | mélanger deux références entre deux décisions | **aucune cryptographie n'aide** : seule la liaison (référence + cible + effets attendus + K) et la comparaison à la résolution |
//! | A4 stockage menteur | rendre un `Engage` faux | hors de portée d'une signature locale |
//!
//! **Conclusion sur la signature cryptographique** : elle n'est *pas* établie comme
//! nécessaire par les cas ci-dessous. Elle ne devient nécessaire que si A2 est dans
//! le modèle ET que la clé peut être gardée hors du medium réécrivable. Le
//! nécessaire et suffisant à ce niveau est la **liaison + vérification de
//! correspondance**, ce que ces tests exercent.
//!
//! ## Ce que chaque test établit, et rien de plus

use obrain_common::types::{NodeId, Value};
use obrain_core::graph::GraphStoreMut as _;
use obrain_substrate::store::{
    EtatPropriete, IntegriteTentative, ModePublication, ResolutionEffet,
};
use obrain_substrate::{ReferenceTentative, SubstrateError, SubstrateStore};

/// Encode une référence « à la main » selon le format documenté. C'est ce que
/// ferait un adversaire A2 : le décodage DOIT l'accepter, la résolution ne doit
/// jamais en tirer un `Engage` faux.
fn encoder(auteur: u64, horloge: u64, cible: NodeId, cles: &[&str]) -> Vec<u8> {
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

/// L'enregistrement de décision du consommateur : c'est LUI la source de vérité
/// des effets attendus. La référence n'en est qu'un index.
struct Enregistrement {
    reference: ReferenceTentative,
    attendus: Vec<String>,
}

impl Enregistrement {
    /// Vérification de correspondance : le consommateur n'accepte un verdict que
    /// si la référence résolue est BIEN celle qu'il a enregistrée, sur les MÊMES
    /// effets attendus. C'est ce contrôle — et non une signature — qui rend une
    /// substitution inoffensive.
    fn correspond(&self, autre: &ReferenceTentative) -> bool {
        autre == &self.reference
            && autre.cible() == self.reference.cible()
            && autre.effets() == self.attendus.as_slice()
    }
}

fn valeur_courante(store: &SubstrateStore, id: NodeId, cle: &str) -> Option<Value> {
    match store.lire_etat_propriete(id, cle).ok()? {
        EtatPropriete::Presente { valeur, .. } => Some(valeur),
        _ => None,
    }
}

/// Prépare un nœud avec deux clés, publie, et rend (store, id, enregistrement).
fn banc_deux_cles(chemin: &std::path::Path) -> (SubstrateStore, NodeId, Enregistrement) {
    let store = SubstrateStore::create(chemin).unwrap();
    let id = store.create_node(&["ressource"]);
    store.set_node_property(id, "a", Value::Int64(1));
    store.set_node_property(id, "b", Value::Int64(2));
    let pre = store.capturer_precondition(id, &["a", "b"]).unwrap();
    let prep = store
        .preparer_transformation(
            id,
            &[("a", Value::Int64(10)), ("b", Value::Int64(20))],
            &pre,
            ModePublication::Strict,
        )
        .unwrap();
    let reference = prep.reference_tentative();
    let attendus = reference.effets().to_vec();
    let _ = store.publier_si_precondition_intacte(prep).unwrap();
    (
        store,
        id,
        Enregistrement {
            reference,
            attendus,
        },
    )
}

/// A1 — ALTÉRATION PARTIELLE : une clé renommée, encodage toujours bien formé.
#[test]
fn alteration_partielle_est_signalee_comme_partielle() {
    let td = tempfile::tempdir().unwrap();
    let (store, id, enr) = banc_deux_cles(&td.path().join("kb"));

    // la référence légitime est intégrale
    let legitime = store.resoudre_reference(&enr.reference).unwrap();
    assert_eq!(legitime.integrite(), IntegriteTentative::Integrale);

    // altération d'UNE clé ("b" -> "z"), même longueur, encodage valide
    let alteree = ReferenceTentative::from_bytes(&encoder(
        enr.reference.auteur(),
        enr.reference.horloge(),
        id,
        &["a", "z"],
    ))
    .unwrap();
    let res = store.resoudre_reference(&alteree).unwrap();
    let a = (0..res.nb_effets())
        .map(|i| res.effet(i))
        .find(|e| e.cle() == "a")
        .unwrap();
    let z = (0..res.nb_effets())
        .map(|i| res.effet(i))
        .find(|e| e.cle() == "z")
        .unwrap();
    assert_eq!(a.etat(), ResolutionEffet::Engage);
    assert_eq!(z.etat(), ResolutionEffet::Indetermine);
    assert_eq!(
        res.integrite(),
        IntegriteTentative::Partielle,
        "une altération partielle se voit comme incomplétude, jamais comme succès"
    );
    // et le consommateur, lui, la refuse par correspondance
    assert!(!enr.correspond(&alteree));
}

/// A2 — SUBSTITUTION COMPLÈTE par une autre référence AUTHENTIQUE.
#[test]
fn substitution_par_une_reference_authentique_est_valide_pour_le_store() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let store = SubstrateStore::create(&chemin).unwrap();
    let id = store.create_node(&["n"]);
    store.set_node_property(id, "a", Value::Int64(1));
    store.set_node_property(id, "b", Value::Int64(1));

    // tentative 1 : écrit "a"
    let pre1 = store.capturer_precondition(id, &["a"]).unwrap();
    let prep1 = store
        .preparer_transformation(
            id,
            &[("a", Value::Int64(2))],
            &pre1,
            ModePublication::Strict,
        )
        .unwrap();
    let enr1 = Enregistrement {
        reference: prep1.reference_tentative(),
        attendus: vec!["a".to_string()],
    };
    let _ = store.publier_si_precondition_intacte(prep1).unwrap();

    // tentative 2 : écrit "b" — autre tentative, elle aussi parfaitement réelle
    let pre2 = store.capturer_precondition(id, &["b"]).unwrap();
    let prep2 = store
        .preparer_transformation(
            id,
            &[("b", Value::Int64(2))],
            &pre2,
            ModePublication::Strict,
        )
        .unwrap();
    let ref2 = prep2.reference_tentative();
    let _ = store.publier_si_precondition_intacte(prep2).unwrap();

    // pour le STORE, la référence substituée est parfaitement valide
    let res = store.resoudre_reference(&ref2).unwrap();
    assert_eq!(res.integrite(), IntegriteTentative::Integrale);
    assert_eq!(res.effet(0).etat(), ResolutionEffet::Engage);
    // pour le CONSOMMATEUR, elle ne correspond pas à son enregistrement
    assert!(
        !enr1.correspond(&ref2),
        "la substitution est détectable — par le consommateur, pas par le stockage"
    );
    assert_ne!(enr1.reference.horloge(), ref2.horloge());
}

/// A3 — SUPPRESSION D'UNE CLÉ ATTENDUE : le cas dangereux.
#[test]
fn suppression_d_une_cle_rend_integrale_une_tentative_incomplete() {
    let td = tempfile::tempdir().unwrap();
    let (store, id, enr) = banc_deux_cles(&td.path().join("kb"));

    // référence réduite à UNE clé alors que la tentative en a écrit DEUX
    let reduite = ReferenceTentative::from_bytes(&encoder(
        enr.reference.auteur(),
        enr.reference.horloge(),
        id,
        &["a"],
    ))
    .unwrap();
    let res = store.resoudre_reference(&reduite).unwrap();
    assert_eq!(
        res.integrite(),
        IntegriteTentative::Integrale,
        "L'AGRÉGAT PORTE SUR LA LISTE DE LA RÉFÉRENCE : la réduire le rend « intégral »"
    );
    assert_eq!(res.nb_effets(), 1);
    // la seule protection : comparer à la liste attendue du consommateur
    assert!(
        !enr.correspond(&reduite),
        "c'est le consommateur qui rétablit la complétude, pas Integrale"
    );
    assert_eq!(enr.attendus.len(), 2);
}

/// A4 + A5 — INCOMPATIBILITÉ DU CONTENU, et effet engagé mais masqué.
#[test]
fn engage_est_historique_et_ne_qualifie_pas_le_contenu() {
    let td = tempfile::tempdir().unwrap();
    let (store, id, enr) = banc_deux_cles(&td.path().join("kb"));

    // avant masquage : canonique, l'estampille courante est celle de la tentative
    let avant = store.resoudre_reference(&enr.reference).unwrap();
    let a_avant = (0..avant.nb_effets())
        .map(|i| avant.effet(i))
        .find(|e| e.cle() == "a")
        .unwrap();
    assert_eq!(
        a_avant.estampille_courante(),
        Some((enr.reference.auteur(), enr.reference.horloge()))
    );

    // masquage par une écriture plus récente
    store.set_node_property(id, "a", Value::Int64(99));
    let apres = store.resoudre_reference(&enr.reference).unwrap();
    let a_apres = (0..apres.nb_effets())
        .map(|i| apres.effet(i))
        .find(|e| e.cle() == "a")
        .unwrap();
    assert_eq!(a_apres.etat(), ResolutionEffet::Engage, "toujours engagé");
    assert_ne!(
        a_apres.estampille_courante(),
        Some((enr.reference.auteur(), enr.reference.horloge())),
        "l'estampille courante est celle du masqueur"
    );
    assert_eq!(apres.integrite(), IntegriteTentative::Integrale);
    // et le contenu n'est plus celui de la tentative
    assert_ne!(valeur_courante(&store, id, "a"), Some(Value::Int64(10)));
    assert_eq!(valeur_courante(&store, id, "a"), Some(Value::Int64(99)));
}

/// B1 — la référence ne porte PAS l'identité logique K.
#[test]
fn la_reference_ne_porte_pas_l_identite_logique_k() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(td.path().join("kb")).unwrap();
    let id = store.create_node(&["n"]);
    store.set_node_property(id, "a", Value::Int64(1));
    let pre = store.capturer_precondition(id, &["a"]).unwrap();

    // DEUX tentatives de la MÊME transformation : mêmes dépendances, mêmes effets
    let p1 = store
        .preparer_transformation(id, &[("a", Value::Int64(2))], &pre, ModePublication::Strict)
        .unwrap();
    let p2 = store
        .preparer_transformation(id, &[("a", Value::Int64(2))], &pre, ModePublication::Strict)
        .unwrap();
    // K (empreinte) identifie la DÉCISION, et elle est stable entre tentatives
    assert_eq!(
        p1.empreinte(),
        p2.empreinte(),
        "même décision logique ⇒ même empreinte"
    );
    // mais les deux TENTATIVES sont physiquement distinctes
    let r1 = p1.reference_tentative();
    let r2 = p2.reference_tentative();
    assert_ne!(
        r1.horloge(),
        r2.horloge(),
        "deux tentatives de la même décision restent distinctes"
    );

    // publier la première ne rend pas la seconde engagée
    let _ = store.publier_si_precondition_intacte(p1).unwrap();
    assert_eq!(
        store.resoudre_reference(&r1).unwrap().integrite(),
        IntegriteTentative::Integrale
    );
    assert_eq!(
        store.resoudre_reference(&r2).unwrap().integrite(),
        IntegriteTentative::Indeterminee,
        "K égale ne vaut pas engagement : la tentative non publiée reste indéterminée"
    );
}

/// B2 — autorité backend : l'auteur persiste à travers la réouverture, et deux
/// bases ont des autorités distinctes.
#[test]
fn l_autorite_backend_est_l_auteur_persiste() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let (auteur_avant, cible) = {
        let (store, id, enr) = banc_deux_cles(&chemin);
        let a = enr.reference.auteur();
        store.close().unwrap();
        (a, id)
    };

    // deuxième base, même construction, autre autorité
    let (_, _, enr2) = banc_deux_cles(&td.path().join("kb2"));
    assert_ne!(
        auteur_avant,
        enr2.reference.auteur(),
        "deux bases ⇒ deux autorités persistées distinctes"
    );

    // après réouverture, l'autorité est la même et la résolution la rapporte
    let store = SubstrateStore::open(&chemin).unwrap();
    let pre = store.capturer_precondition(cible, &["a"]).unwrap();
    let prep = store
        .preparer_transformation(
            cible,
            &[("a", Value::Int64(3))],
            &pre,
            ModePublication::Strict,
        )
        .unwrap();
    let reference = prep.reference_tentative();
    assert_eq!(reference.auteur(), auteur_avant);
    let _ = store.publier_si_precondition_intacte(prep).unwrap();
    assert_eq!(
        store
            .resoudre_reference(&reference)
            .unwrap()
            .auteur_resolvant(),
        auteur_avant
    );
}

/// C — chemin différé : refus nommé, jamais d'estampille inventée.
#[test]
fn preparation_differee_refuse_une_reference() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(td.path().join("kb")).unwrap();
    let id = store.create_node(&["n"]);
    store.set_node_property(id, "a", Value::Int64(1));
    let pre = store.capturer_precondition(id, &["a"]).unwrap();
    let differee = store
        .preparer_transformation_differee(
            id,
            &[("a", Value::Int64(2))],
            &pre,
            ModePublication::Strict,
        )
        .unwrap();
    match differee.reference_tentative() {
        Err(SubstrateError::ReferenceNonAttribuable { raison }) => {
            assert!(raison.contains("matérialisation"), "{raison}")
        }
        Err(autre) => panic!("variante inattendue : {autre:?}"),
        Ok(_) => panic!("une préparation différée ne doit pas produire de référence"),
    }
}
