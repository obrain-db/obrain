//! # La porte de l'audit chaîné — ce qu'un opérateur peut réécrire en silence
//!
//! Ce fichier confronte le registre d'audit de l'IAM, tel qu'il existe
//! (`crate::store::IamStore`, ses `AuditEvent`), aux garanties que le chaînage
//! prétend apporter. Chaque test est une capacité de l'opérateur — il détient la
//! clé, c'est **son** journal — et une réponse à la question : qui s'en aperçoit ?
//!
//! | L'opérateur… | Attrapé par | Vu par |
//! |---|---|---|
//! | change un verdict dans le store | le reçu du client | **le client** |
//! | retire un événement | la chaîne | quiconque vérifie |
//! | reconstruit **et resigne** l'audit | le reçu du client | **le client** |
//! | tronque la fin | la tête conservée | qui a gardé la tête |
//! | n'inscrit pas un événement | rien, ici | le client qui attend un reçu |
//!
//! Le premier ligne est la plus importante, et c'est la plus simple : un journal
//! d'audit n'a pas besoin qu'on en retire des lignes pour mentir — `Deny` devenu
//! `Allow` est une ligne modifiée, une ligne qui reste, et une histoire qui dit
//! le contraire.

use obrain_iam::audit_chaine::{RegistreAudit, empreinte_evenement};
use obrain_iam::model::{AuditEvent, PolicyDecision};
use obrain_iam::orn::Orn;
use obrain_journal::{CleJournal, ErreurJournal};

const ESPACE: &str = "espace-alpha";

fn cle_de_test() -> CleJournal {
    CleJournal::depuis_octets([42u8; 32])
}

/// Un événement d'audit, tel que la garde en produit : un verdict rendu sur une
/// action, pour un principal, sur une ressource.
fn evenement(id: &str, action: &str, resultat: PolicyDecision) -> AuditEvent {
    AuditEvent {
        id: id.to_owned(),
        principal: Orn::user("obrain", "alice"),
        action: action.to_owned(),
        resource: Orn::node("obrain", "graph:espace-alpha"),
        result: resultat,
        timestamp: "2026-09-15T21:00:00Z".to_owned(),
    }
}

/// `n` événements inscrits, avec les reçus correspondants.
fn registre_de(n: usize) -> (RegistreAudit, Vec<AuditEvent>, Vec<obrain_journal::Entree>) {
    let mut registre = RegistreAudit::neuf(ESPACE, cle_de_test()).expect("registre");
    let mut evenements = Vec::new();
    let mut recus = Vec::new();
    for i in 0..n {
        let evenement = evenement(
            &format!("evt-{i}"),
            "graph:read",
            if i % 2 == 0 {
                PolicyDecision::Allow
            } else {
                PolicyDecision::Deny
            },
        );
        let recu = registre
            .enregistrer(&evenement, 1_700_000_000 + i as u64)
            .expect("inscription");
        evenements.push(evenement);
        recus.push(recu);
    }
    (registre, evenements, recus)
}

// ---------------------------------------------------------------------------
// Le tour nominal
// ---------------------------------------------------------------------------

#[test]
fn un_evenement_inscrit_se_retrouve_par_son_recu() {
    let (registre, evenements, recus) = registre_de(4);

    registre
        .verifier(&cle_de_test().publique())
        .expect("chaîne et signatures");
    assert_eq!(registre.longueur(), 4);

    for (evenement, recu) in evenements.iter().zip(&recus) {
        registre
            .contient(recu, evenement)
            .expect("l'événement montré est celui qui a été inscrit");
    }
}

/// Le registre survit au disque : les reçus détenus restent opposables après un
/// redémarrage, ce qui est la moitié de l'intérêt d'un journal.
#[test]
fn le_registre_survit_au_disque() {
    let (registre, evenements, recus) = registre_de(3);
    let octets = registre.vers_octets();
    let tete = registre.tete();

    let relu = RegistreAudit::depuis_octets(ESPACE, cle_de_test(), &octets).expect("relecture");
    assert_eq!(relu.tete(), tete);
    assert_eq!(relu.longueur(), 3);
    relu.verifier(&cle_de_test().publique()).expect("signatures");

    for (evenement, recu) in evenements.iter().zip(&recus) {
        relu.contient(recu, evenement).expect("toujours opposable");
    }
}

// ---------------------------------------------------------------------------
// L'attaque la plus simple : un verdict réécrit
// ---------------------------------------------------------------------------

/// **Le test qui porte ce module.**
///
/// L'opérateur ne retire rien, ne réordonne rien : il change un verdict dans le
/// store. Le journal, lui, reste parfaitement intact — chaîne valide, signatures
/// valides. Seul le client, qui détient le reçu du verdict réel, s'en aperçoit.
#[test]
fn un_verdict_reecrit_dans_le_store_est_demasque_par_le_recu() {
    let mut registre = RegistreAudit::neuf(ESPACE, cle_de_test()).expect("registre");

    // La garde a refusé, et le client a emporté le reçu.
    let refuse = evenement("evt-1", "iam:admin", PolicyDecision::Deny);
    let recu = registre.enregistrer(&refuse, 1_700_000_000).expect("inscription");

    // L'opérateur réécrit le verdict dans ce qu'il montre. Le journal n'est pas
    // touché.
    let mut montre = refuse.clone();
    montre.result = PolicyDecision::Allow;

    match registre.contient(&recu, &montre) {
        Err(ErreurJournal::ElementDifferent { sequence }) => assert_eq!(sequence, 0),
        autre => panic!("attendu ElementDifferent sur verdict réécrit, obtenu {autre:?}"),
    }

    // Et le journal, lui, ne signale rien : c'est bien le problème, et c'est
    // pourquoi le reçu existe.
    registre
        .verifier(&cle_de_test().publique())
        .expect("le journal reste cohérent : il n'a pas été touché");

    // L'événement réel, lui, passe toujours.
    registre
        .contient(&recu, &refuse)
        .expect("le verdict réel reste opposable");
}

/// L'opérateur reconstruit tout l'audit — autres verdicts, autres actions — et
/// resigne l'ensemble avec la clé qu'il détient. Chaîne valide, signatures
/// valides, et pourtant : le reçu détenu par le client ne s'accorde plus.
#[test]
fn un_audit_reconstruit_et_resigne_est_demasque_par_le_recu() {
    let (honnete, evenements, recus) = registre_de(3);
    let reçu_du_client = recus[1].clone();
    let evenement_reel = evenements[1].clone();

    // L'opérateur refait l'histoire : mêmes identifiants, verdicts inversés.
    let mut reecrit = RegistreAudit::neuf(ESPACE, cle_de_test()).expect("registre");
    for (i, evenement) in evenements.iter().enumerate() {
        let mut inverse = evenement.clone();
        inverse.result = match evenement.result {
            PolicyDecision::Allow => PolicyDecision::Deny,
            PolicyDecision::Deny | PolicyDecision::ImplicitDeny => PolicyDecision::Allow,
        };
        reecrit
            .enregistrer(&inverse, 1_700_000_000 + i as u64)
            .expect("inscription");
    }

    reecrit
        .verifier(&cle_de_test().publique())
        .expect("l'audit réécrit est cohérent et signé : c'est bien le problème");
    assert_eq!(reecrit.longueur(), honnete.longueur());

    match reecrit.contient(&reçu_du_client, &evenement_reel) {
        Err(ErreurJournal::ElementDifferent { sequence }) => assert_eq!(sequence, 1),
        autre => panic!("attendu ElementDifferent sur audit reconstruit, obtenu {autre:?}"),
    }
}

/// Tronguer l'audit : le registre amputé reste cohérent avec lui-même, et seule
/// la tête conservée le démasque.
#[test]
fn une_troncature_est_demasquee_par_la_tete_souvenue() {
    let (registre, _, _) = registre_de(4);
    let tete_souvenue = registre.tete();

    // Deux événements au lieu de quatre : l'opérateur coupe la fin.
    let mut ampute = RegistreAudit::neuf(ESPACE, cle_de_test()).expect("registre");
    for i in 0..2 {
        let evenement = evenement(&format!("evt-{i}"), "graph:read", PolicyDecision::Allow);
        ampute
            .enregistrer(&evenement, 1_700_000_000 + i as u64)
            .expect("inscription");
    }

    ampute
        .verifier(&cle_de_test().publique())
        .expect("l'audit tronqué est cohérent avec lui-même");
    assert!(ampute.journal().verifier_jusqua(tete_souvenue).is_err());
}

/// Un registre ne se charge pas sous le nom d'un autre espace : sinon l'audit
/// d'un espace avalerait les reçus d'un autre, et chaque confrontation
/// échouerait ensuite sans que la cause soit visible.
#[test]
fn un_registre_ne_se_charge_pas_sous_un_autre_nom() {
    let (registre, _, _) = registre_de(2);
    let octets = registre.vers_octets();

    RegistreAudit::depuis_octets(ESPACE, cle_de_test(), &octets)
        .expect("le bon nom charge le registre");

    match RegistreAudit::depuis_octets("espace-beta", cle_de_test(), &octets) {
        Err(ErreurJournal::Reference(motif)) => {
            assert!(motif.contains("espace-beta"), "le motif doit nommer : {motif}");
        }
        autre => panic!("attendu Reference sur mauvais nom d'espace, obtenu {autre:?}"),
    }
}

// ---------------------------------------------------------------------------
// L'empreinte elle-même
// ---------------------------------------------------------------------------

/// **Deux découpages différents ne doivent pas produire la même empreinte.**
///
/// Sans préfixe de longueur, `id = "ab"` avec `action = "c"` et `id = "a"` avec
/// `action = "bc"` s'écriraient tous deux `abc` : l'opérateur déplacerait une
/// frontière et changerait le sens d'une ligne **sans changer son empreinte**.
#[test]
fn deux_decoupages_differents_ont_deux_empreintes() {
    let mut premier = evenement("ab", "c", PolicyDecision::Allow);
    premier.timestamp = String::new();
    let mut second = evenement("a", "bc", PolicyDecision::Allow);
    second.timestamp = String::new();

    assert_ne!(
        empreinte_evenement(&premier),
        empreinte_evenement(&second),
        "« ab »+« c » et « a »+« bc » ne doivent pas avoir la même empreinte"
    );
}

/// Le verdict est le champ le plus important de l'empreinte : c'est celui qu'on
/// a le plus intérêt à réécrire.
#[test]
fn le_verdict_change_l_empreinte() {
    let permet = evenement("evt", "graph:read", PolicyDecision::Allow);
    let refuse = evenement("evt", "graph:read", PolicyDecision::Deny);
    let implicite = evenement("evt", "graph:read", PolicyDecision::ImplicitDeny);

    assert_ne!(empreinte_evenement(&permet), empreinte_evenement(&refuse));
    assert_ne!(empreinte_evenement(&refuse), empreinte_evenement(&implicite));
    assert_ne!(empreinte_evenement(&permet), empreinte_evenement(&implicite));
}

/// L'empreinte est stable : deux fois le même événement, deux fois la même
/// empreinte — sinon rien de tout ceci ne serait vérifiable.
#[test]
fn la_meme_ligne_a_la_meme_empreinte() {
    let a = evenement("evt-7", "graph:write", PolicyDecision::Deny);
    let b = evenement("evt-7", "graph:write", PolicyDecision::Deny);
    assert_eq!(empreinte_evenement(&a), empreinte_evenement(&b));
}

/// Deux espaces, deux registres : un reçu de l'un ne se verse pas dans l'autre.
#[test]
fn deux_espaces_n_echangent_pas_leurs_recus() {
    let mut alpha = RegistreAudit::neuf("espace-alpha", cle_de_test()).expect("registre");
    let mut beta = RegistreAudit::neuf("espace-beta", cle_de_test()).expect("registre");

    let evenement = evenement("evt", "graph:read", PolicyDecision::Allow);
    let recu_alpha = alpha.enregistrer(&evenement, 1_700_000_000).expect("inscription");
    beta.enregistrer(&evenement, 1_700_000_000).expect("inscription");

    assert_ne!(alpha.tete(), beta.tete());
    match beta.contient(&recu_alpha, &evenement) {
        Err(ErreurJournal::JournalEtranger(_)) => {}
        autre => panic!("attendu JournalEtranger, obtenu {autre:?}"),
    }
}
