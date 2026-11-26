//! ── TÉMOIN EXTERNE DE LA RÉFÉRENCE DE TENTATIVE ────────────────────────────
//!
//! Un test d'intégration est compilé comme une crate EXTERNE : il ne voit ni les
//! champs privés du store, ni les accesseurs `#[cfg(test)]`. Tout ce qui suit
//! n'utilise donc que l'API publique — c'est la condition pour que le résultat
//! vaille comme preuve pour un consommateur (this-harness).
//!
//! Ce qui est éprouvé, dans l'ordre du contrat :
//!   1. préparer ; 2. obtenir la référence ; 3. la sérialiser ; 4. publier en
//!   perdant la réponse ; 5. fermer ; 6. rouvrir ; 7. désérialiser et résoudre ;
//!   8. masquer l'effet ; 9. résoudre de nouveau sans republier.
//! Plus les cas adverses : référence étrangère, référence altérée, effets
//! incompatibles, décodage d'octets invalides, refus du chemin différé.

use obrain_common::types::{NodeId, Value};
use obrain_core::graph::{GraphStore as _, GraphStoreMut as _};
use obrain_substrate::store::{
    EtatPropriete, IntegriteTentative, ModePublication, ResolutionEffet,
};
use obrain_substrate::{ReferenceTentative, SubstrateError, SubstrateStore};

/// Encode une référence « à la main », selon le format documenté. Un attaquant
/// ferait exactement cela : le décodage DOIT l'accepter, et la résolution ne
/// doit jamais en tirer un `Engage` faux.
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

/// 1 → 9 : la référence survit à la perte de réponse, à la fermeture, et au
/// masquage. Et `Engage` ne dit rien de la compatibilité du contenu.
#[test]
fn reference_survit_a_la_perte_de_reponse_et_au_masquage() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");

    let (octets, cible, auteur, horloge) = {
        let store = SubstrateStore::create(&chemin).unwrap();
        let id = store.create_node(&["ressource"]);
        store.set_node_property(id, "a", Value::Int64(1));
        store.set_node_property(id, "b", Value::Int64(2));
        let pre = store.capturer_precondition(id, &["a", "b"]).unwrap();
        // 1. préparer
        let prep = store
            .preparer_transformation(
                id,
                &[("a", Value::Int64(10)), ("b", Value::Int64(20))],
                &pre,
                ModePublication::Strict,
            )
            .unwrap();
        // 2. obtenir la référence AVANT publication
        let reference = prep.reference_tentative();
        // 3. sérialiser
        let octets = reference.to_bytes();
        let (auteur, horloge) = (reference.auteur(), reference.horloge());
        assert_eq!(reference.cible(), id);
        assert_eq!(reference.effets(), ["a".to_string(), "b".to_string()]);
        // 4. publier en perdant la réponse
        let _verdict_perdu = store.publier_si_precondition_intacte(prep).unwrap();
        // 5. fermer
        store.close().unwrap();
        (octets, id, auteur, horloge)
    };

    // 6. rouvrir — nouvelle incarnation, même répertoire
    let store = SubstrateStore::open(&chemin).unwrap();
    // 7. désérialiser et résoudre, sans republier
    let reference = ReferenceTentative::from_bytes(&octets).unwrap();
    let res = store.resoudre_reference(&reference).unwrap();
    assert_eq!(res.cible, cible);
    assert_eq!(
        res.auteur_resolvant, auteur,
        "l'auteur persistant du répertoire est stable à travers la réouverture"
    );
    assert_eq!(
        res.integrite,
        IntegriteTentative::Integrale,
        "les deux effets de la tentative sont engagés"
    );
    assert!(res
        .par_cle
        .iter()
        .all(|e| e.etat == ResolutionEffet::Engage));

    // 8. masquer par une mutation ultérieure, sans republier
    store.set_node_property(cible, "a", Value::Int64(99));
    // 9. résoudre de nouveau
    let res2 = store.resoudre_reference(&reference).unwrap();
    assert_eq!(
        res2.integrite,
        IntegriteTentative::Integrale,
        "masqué n'est pas absence : les deux effets restent engagés"
    );
    let a = res2.par_cle.iter().find(|e| e.cle == "a").unwrap();
    assert_eq!(a.etat, ResolutionEffet::Engage);
    assert_ne!(
        a.estampille_courante(),
        Some((auteur, horloge)),
        "l'estampille courante est celle du masqueur, pas celle de la tentative"
    );

    // Effet INCOMPATIBLE : le contenu courant ne correspond plus à celui que la
    // tentative écrivait — et `Engage` est toujours rendu. C'est la preuve que
    // `Engage` n'est pas `EffetConfirme`.
    match store.lire_etat_propriete(cible, "a").unwrap() {
        EtatPropriete::Presente { valeur, .. } => assert_eq!(
            valeur,
            Value::Int64(99),
            "la valeur courante est celle du masqueur : l'engagement ne la qualifie pas"
        ),
        autre => panic!("état inattendu : {autre:?}"),
    }
}

/// Référence venue d'une AUTRE base : jamais un `Engage`.
#[test]
fn reference_etrangere_ne_produit_jamais_engage() {
    let td = tempfile::tempdir().unwrap();
    let base_a = td.path().join("a");
    let base_b = td.path().join("b");

    let store_a = SubstrateStore::create(&base_a).unwrap();
    let id_a = store_a.create_node(&["n"]);
    store_a.set_node_property(id_a, "x", Value::Int64(1));
    let pre = store_a.capturer_precondition(id_a, &["x"]).unwrap();
    let prep = store_a
        .preparer_transformation(
            id_a,
            &[("x", Value::Int64(2))],
            &pre,
            ModePublication::Strict,
        )
        .unwrap();
    let etrangere = prep.reference_tentative();
    let _ = store_a.publier_si_precondition_intacte(prep).unwrap();

    let store_b = SubstrateStore::create(&base_b).unwrap();
    let id_b = store_b.create_node(&["n"]);
    store_b.set_node_property(id_b, "x", Value::Int64(1));

    let res = store_b.resoudre_reference(&etrangere).unwrap();
    assert_eq!(
        res.integrite,
        IntegriteTentative::Indeterminee,
        "une référence d'une autre base ne peut pas engager quoi que ce soit ici"
    );
    assert!(res
        .par_cle
        .iter()
        .all(|e| e.etat == ResolutionEffet::Indetermine));
    assert_ne!(
        res.auteur_resolvant,
        etrangere.auteur(),
        "les deux bases ont des identités persistées distinctes"
    );
}

/// Référence ALTÉRÉE, bien formée : décodée sans erreur, mais un `Engage` n'est
/// rendu que pour l'effet réellement engagé — et l'agrégat devient `Partielle`.
#[test]
fn reference_alteree_ne_produit_jamais_engage() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let store = SubstrateStore::create(&chemin).unwrap();
    let id = store.create_node(&["n"]);
    store.set_node_property(id, "a", Value::Int64(1));
    let pre = store.capturer_precondition(id, &["a"]).unwrap();
    let prep = store
        .preparer_transformation(id, &[("a", Value::Int64(2))], &pre, ModePublication::Strict)
        .unwrap();
    let legitime = prep.reference_tentative();
    let _ = store.publier_si_precondition_intacte(prep).unwrap();

    // forge : même auteur, même estampille, mais on ajoute une clé jamais écrite
    let forgee = ReferenceTentative::from_bytes(&encoder(
        legitime.auteur(),
        legitime.horloge(),
        legitime.cible(),
        &["a", "jamais_ecrite"],
    ))
    .unwrap();
    let res = store.resoudre_reference(&forgee).unwrap();
    let a = res.par_cle.iter().find(|e| e.cle == "a").unwrap();
    let j = res
        .par_cle
        .iter()
        .find(|e| e.cle == "jamais_ecrite")
        .unwrap();
    assert_eq!(a.etat, ResolutionEffet::Engage);
    assert_eq!(
        j.etat,
        ResolutionEffet::Indetermine,
        "une clé jamais écrite ne devient pas engagée parce qu'elle est déclarée"
    );
    assert_eq!(
        res.integrite,
        IntegriteTentative::Partielle,
        "un agrégat explicite : ni Integrale, ni Indeterminee"
    );
}

/// Décodage contrôlé : aucune entrée invalide ne panique, toutes sont refusées.
#[test]
fn decodage_controle_refuse_les_octets_invalides() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(&td.path().join("kb")).unwrap();
    let id = store.create_node(&["n"]);
    store.set_node_property(id, "a", Value::Int64(1));
    let pre = store.capturer_precondition(id, &["a"]).unwrap();
    let prep = store
        .preparer_transformation(id, &[("a", Value::Int64(2))], &pre, ModePublication::Strict)
        .unwrap();
    let valide = prep.reference_tentative().to_bytes();

    // magic
    let mut mauvais_magic = valide.clone();
    mauvais_magic[0] = b'X';
    assert!(ReferenceTentative::from_bytes(&mauvais_magic).is_err());
    // version
    let mut mauvaise_version = valide.clone();
    mauvaise_version[4] = 9;
    assert!(ReferenceTentative::from_bytes(&mauvaise_version).is_err());
    // troncature
    assert!(ReferenceTentative::from_bytes(&valide[..20]).is_err());
    // octets excédentaires
    let mut excedent = valide.clone();
    excedent.push(0);
    assert!(ReferenceTentative::from_bytes(&excedent).is_err());
    // décompte d'effets absurde (4 G effets annoncés) — n_effets est à
    // l'offset 29 : magic(0..4) version(4) auteur(5..13) horloge(13..21)
    // cible(21..29) puis le décompte (29..33).
    let mut absurde = valide.clone();
    absurde[29..33].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(ReferenceTentative::from_bytes(&absurde).is_err());
    // entrée vide
    assert!(ReferenceTentative::from_bytes(&[]).is_err());
    // et la valide passe
    assert!(ReferenceTentative::from_bytes(&valide).is_ok());
}

/// Chemin différé : refus PUBLIC et matchable, jamais une estampille inventée.
#[test]
fn preparation_differee_refuse_une_reference() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(&td.path().join("kb")).unwrap();
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
            assert!(
                raison.contains("matérialisation"),
                "le refus nomme la cause : {raison}"
            );
        }
        Err(autre) => panic!("variante inattendue : {autre:?}"),
        Ok(_) => panic!("une préparation différée ne doit PAS produire de référence"),
    }
}
