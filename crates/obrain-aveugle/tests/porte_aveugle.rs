//! # La porte du porteur aveugle — le critère de l'étape 3, exécutable
//!
//! L'ADR demande, pour l'étape 3 :
//!
//! > l'engine reçoit une enveloppe de taille fixe et **ne peut en extraire ni la
//! > requête ni la cible**.
//!
//! Ce fichier le mesure au lieu de l'affirmer. La forme la plus forte d'un tel
//! test n'est pas « le porteur refuse de lire » — un refus est une politique, et
//! une politique peut avoir tort — mais : **ce que le porteur peut observer ne
//! contient aucun octet du contenu**. On prend donc tout ce qu'il détient, on le
//! sérialise, et on y cherche le secret avec `grep`-en-Rust.
//!
//! Les autres tests portent sur ce qu'un porteur *doit* faire respecter sans rien
//! savoir : la forme des dépôts, le non-rejeu, le quota — et sur ce qu'il ne peut
//! pas empêcher : le détournement, qu'il doit au moins rendre **détectable**.

use obrain_aveugle::{Adresse, Charge, ErreurAveugle, Porteur, ouvrir, sceller};
use obrain_coffre::{CleEspace, Jeton};
use obrain_journal::{CleJournal, ErreurJournal};

/// Ce qu'un utilisateur ne veut pas voir traverser un engine.
const REQUETE: &str = "graph:read MATCH (n:Client) WHERE n.iban = 'FR76-LE-SECRET-DU-CLIENT'";
/// Et ce que le calcul a rendu.
const RESULTAT: &str = "resultat-du-calcul-8c1d55aa-non-plus-jamais";

fn cle_espace() -> CleEspace {
    CleEspace::engendrer("espace-alpha").expect("clé d'espace")
}

fn adresses() -> (Adresse, Adresse, Adresse) {
    (
        Adresse::tirer().expect("adresse A"),
        Adresse::tirer().expect("adresse B"),
        Adresse::tirer().expect("adresse C"),
    )
}

/// Un porteur avec deux extrémités déclarées, et un quota large.
fn porteur(quota: usize) -> (Porteur, CleJournal, Adresse, Adresse) {
    let (a, b, _) = adresses();
    let cle = CleJournal::depuis_octets([7u8; 32]);
    let mut porteur = Porteur::neuf("porteur-alpha", cle, quota).expect("porteur");
    porteur.enregistrer(a);
    porteur.enregistrer(b);
    (porteur, CleJournal::depuis_octets([7u8; 32]), a, b)
}

fn contient(foin: &[u8], aiguille: &[u8]) -> bool {
    foin.windows(aiguille.len()).any(|f| f == aiguille)
}

// ---------------------------------------------------------------------------
// Le critère de l'ADR
// ---------------------------------------------------------------------------

/// Un message traverse le porteur et arrive à bon port — sans que le porteur
/// l'ait jamais lu.
#[test]
fn un_message_traverse_le_porteur_et_arrive_a_bon_port() {
    let (mut porteur, _, a, b) = porteur(8);
    let cle = cle_espace();

    let charge = Charge::nouvelle(a, b, REQUETE.as_bytes().to_vec());
    let enveloppe = sceller(&cle, &charge).expect("scellement");
    porteur
        .deposer(a, b, enveloppe, 1_700_000_000)
        .expect("dépôt");

    let transportee = porteur.retirer(&b).expect("une enveloppe attend");
    assert_eq!(transportee.expediteur, a);
    assert!(porteur.retirer(&b).is_none(), "la boîte est vidée");

    let recue = ouvrir(&cle, &b, &transportee.octets).expect("déscellement");
    assert_eq!(recue.expediteur, a);
    assert_eq!(recue.destinataire, b);
    assert_eq!(recue.quoi, REQUETE.as_bytes());
}

/// **Le test central : ce que le porteur détient ne contient aucun secret.**
///
/// On sérialise tout ce qu'il peut observer — sa vue de chaque transition — et on
/// y cherche la requête, le résultat, le nom de l'action, et les octets scellés.
#[test]
fn ce_que_le_porteur_voit_ne_contient_aucun_secret() {
    let (mut porteur, _, a, b) = porteur(8);
    let cle = cle_espace();

    // Un aller : la requête.
    let en_avant =
        sceller(&cle, &Charge::nouvelle(a, b, REQUETE.as_bytes().to_vec())).expect("scellement");
    porteur
        .deposer(a, b, en_avant.clone(), 1_700_000_000)
        .expect("dépôt");
    let _ = porteur.retirer(&b);

    // Un retour : le résultat, et un jeton de capacité.
    let mut charge = Charge::nouvelle(b, a, RESULTAT.as_bytes().to_vec());
    let jeton = Jeton::neuf(&cle).expect("jeton");
    charge.jeton = Some(jeton.emettre("iam:admin", 2_000_000_000).expect("émission"));
    let en_retour = sceller(&cle, &charge).expect("scellement");
    porteur
        .deposer(b, a, en_retour.clone(), 1_700_000_001)
        .expect("dépôt");

    // Tout ce que le porteur peut dire de son trafic.
    let mut vu = Vec::new();
    for transition in porteur.ce_que_jai_vu() {
        vu.extend_from_slice(&transition.vers_octets());
    }
    // Plus ce qu'il transporte, tel quel.
    let transporte = porteur.retirer(&a).expect("le retour attend");
    let mut tout = vu.clone();
    tout.extend_from_slice(&transporte.octets);

    for (nom, secret) in [
        ("la requête", REQUETE),
        ("le résultat", RESULTAT),
        ("le nom de l'action", "iam:admin"),
    ] {
        assert!(
            !contient(&tout, secret.as_bytes()),
            "{nom} est lisible dans ce que le porteur détient"
        );
        assert!(
            !contient(&en_avant, secret.as_bytes()),
            "{nom} est lisible dans les octets scellés"
        );
        assert!(
            !contient(&en_retour, secret.as_bytes()),
            "{nom} est lisible dans les octets scellés"
        );
    }

    // Et la vue ne porte que ce qu'elle a le droit de porter.
    let vues = porteur.ce_que_jai_vu();
    assert_eq!(vues.len(), 2);
    assert_eq!(vues[0].expediteur, a);
    assert_eq!(vues[0].destinataire, b);
    assert_eq!(vues[0].taille, en_avant.len());
    assert_eq!(vues[1].expediteur, b);
    assert_eq!(vues[1].destinataire, a);
}

/// Le porteur n'a pas la clé : ses octets ne s'ouvrent ni sans clé, ni avec
/// celle d'un autre espace. C'est la position réelle de l'engine.
#[test]
fn sans_la_cle_le_porteur_n_obtient_rien() {
    let (a, b, _) = adresses();
    let cle = cle_espace();
    let autre_espace = CleEspace::engendrer("espace-beta").expect("clé");

    let enveloppe =
        sceller(&cle, &Charge::nouvelle(a, b, REQUETE.as_bytes().to_vec())).expect("scellement");

    // Avec la clé d'un autre espace.
    match ouvrir(&autre_espace, &b, &enveloppe) {
        Err(ErreurAveugle::Coffre(_)) => {}
        autre => panic!("attendu un refus du coffre, obtenu {autre:?}"),
    }

    // Et le message reçu par un autre que le destinataire scellé est détourné.
    let (_, _, c) = adresses();
    match ouvrir(&cle, &c, &enveloppe) {
        Err(ErreurAveugle::Detourne { .. }) => {}
        autre => panic!("attendu Detourne, obtenu {autre:?}"),
    }
}

/// **G3 : deux calculs différents font la même taille sur le fil.** Le porteur ne
/// peut donc pas distinguer « une lecture » d'« une écriture » à la longueur.
#[test]
fn deux_calculs_differents_ont_la_meme_taille_transportee() {
    let (mut porteur, _, a, b) = porteur(8);
    let cle = cle_espace();

    // Deux charges de tailles différentes mais de même classe.
    let courte = Charge::nouvelle(a, b, b"RETURN 1".to_vec());
    let longue = Charge::nouvelle(a, b, vec![b'x'; 150]);

    let e1 = sceller(&cle, &courte).expect("scellement");
    let e2 = sceller(&cle, &longue).expect("scellement");
    assert_eq!(
        e1.len(),
        e2.len(),
        "deux charges de la même classe doivent avoir la même longueur"
    );

    porteur.deposer(a, b, e1, 1).expect("dépôt");
    porteur.deposer(a, b, e2, 2).expect("dépôt");

    let tailles: Vec<usize> = porteur.ce_que_jai_vu().iter().map(|v| v.taille).collect();
    assert_eq!(tailles[0], tailles[1], "le porteur ne voit qu'une taille");
}

// ---------------------------------------------------------------------------
// Ce que le porteur fait respecter sans rien savoir
// ---------------------------------------------------------------------------

/// Un dépôt dont la taille n'est pas une taille du protocole est refusé : sinon
/// le porteur deviendrait un canal où la longueur raconte le contenu.
#[test]
fn un_depot_hors_classe_de_taille_est_refuse() {
    let (mut porteur, _, a, b) = porteur(8);

    for taille in [0usize, 1, 100, 322, 324, 100_000] {
        match porteur.deposer(a, b, vec![0u8; taille], 1) {
            Err(ErreurAveugle::TailleNonConforme { taille: vue }) => assert_eq!(vue, taille),
            autre => panic!("attendu TailleNonConforme pour {taille} octets, obtenu {autre:?}"),
        }
    }

    // Une taille conforme, elle, passe.
    let cle = cle_espace();
    let enveloppe =
        sceller(&cle, &Charge::nouvelle(a, b, b"RETURN 1".to_vec())).expect("scellement");
    porteur.deposer(a, b, enveloppe, 1).expect("dépôt conforme");
}

/// **Le non-rejeu.** Dupliquer les octets d'une enveloppe ferait exécuter deux
/// fois le même calcul — un porteur, ou un intermédiaire, ne doit pas pouvoir le
/// faire par simple copie.
#[test]
fn le_meme_message_deux_fois_est_refuse() {
    let (mut porteur, _, a, b) = porteur(8);
    let cle = cle_espace();

    let enveloppe =
        sceller(&cle, &Charge::nouvelle(a, b, REQUETE.as_bytes().to_vec())).expect("scellement");
    porteur
        .deposer(a, b, enveloppe.clone(), 1)
        .expect("premier dépôt");

    match porteur.deposer(a, b, enveloppe, 2) {
        Err(ErreurAveugle::Rejeu { empreinte }) => assert!(empreinte.ends_with('…')),
        autre => panic!("attendu Rejeu, obtenu {autre:?}"),
    }
    assert_eq!(porteur.en_attente(&b), 1, "une seule copie livrée");
}

/// Une boîte pleine refuse : un porteur qui accumule sans borne devient un
/// amplificateur de déni de service.
#[test]
fn une_boite_pleine_refuse_au_lieu_de_grandir() {
    let (mut porteur, _, a, b) = porteur(2);
    let cle = cle_espace();

    for i in 0..2 {
        let enveloppe = sceller(
            &cle,
            &Charge::nouvelle(a, b, format!("calcul-{i}").into_bytes()),
        )
        .expect("scellement");
        porteur.deposer(a, b, enveloppe, i).expect("dépôt");
    }

    let enveloppe =
        sceller(&cle, &Charge::nouvelle(a, b, b"troisieme".to_vec())).expect("scellement");
    match porteur.deposer(a, b, enveloppe, 3) {
        Err(ErreurAveugle::BoitePleine { quota, .. }) => assert_eq!(quota, 2),
        autre => panic!("attendu BoitePleine, obtenu {autre:?}"),
    }
    assert_eq!(porteur.en_attente(&b), 2, "la boîte n'a pas grandi");
}

#[test]
fn deposer_vers_une_extremite_inconnue_est_refuse() {
    let (mut porteur, _, a, _) = porteur(8);
    let (_, _, inconnue) = adresses();
    let cle = cle_espace();

    let enveloppe =
        sceller(&cle, &Charge::nouvelle(a, inconnue, b"RETURN 1".to_vec())).expect("scellement");

    match porteur.deposer(a, inconnue, enveloppe, 1) {
        Err(ErreurAveugle::DestinataireInconnu(_)) => {}
        autre => panic!("attendu DestinataireInconnu, obtenu {autre:?}"),
    }
}

// ---------------------------------------------------------------------------
// Ce que le porteur ne peut pas empêcher, et ce que les extrémités voient
// ---------------------------------------------------------------------------

/// Un détournement : le porteur réétiquette l'enveloppe vers C. Il tient les
/// octets, donc on ne peut pas l'en empêcher — mais la destination scellée dedans
/// le démasque, et le vrai destinataire ne reçoit rien.
#[test]
fn un_detournement_est_detecte_par_le_destinataire() {
    let (mut porteur, _, a, b) = porteur(8);
    let (_, _, c) = adresses();
    porteur.enregistrer(c);
    let cle = cle_espace();

    let enveloppe =
        sceller(&cle, &Charge::nouvelle(a, b, REQUETE.as_bytes().to_vec())).expect("scellement");
    porteur.deposer(a, b, enveloppe, 1).expect("dépôt");

    // Le porteur détourne : il livre à C ce qui était scellé pour B.
    let transportee = porteur.retirer(&b).expect("l'enveloppe attend");
    match ouvrir(&cle, &c, &transportee.octets) {
        Err(ErreurAveugle::Detourne { scelle, .. }) => {
            assert!(scelle.contains("Adresse("), "l'adresse scellée est nommée");
        }
        autre => panic!("attendu Detourne chez l'intrus, obtenu {autre:?}"),
    }

    // Et le vrai destinataire n'a rien : le détournement coûte la livraison.
    assert_eq!(porteur.en_attente(&b), 0);
}

/// Un jeton de capacité traverse **scellé** et se vérifie à l'arrivée : le
/// porteur transporte une autorisation sans jamais savoir ce qu'elle autorise.
#[test]
fn un_jeton_de_capacite_traverse_sans_etre_lu() {
    let (mut porteur, _, a, b) = porteur(8);
    let cle = cle_espace();

    let jeton = Jeton::neuf(&cle).expect("jeton");
    let emis = jeton
        .emettre("graph:write", 2_000_000_000)
        .expect("émission");

    let mut charge = Charge::nouvelle(a, b, b"RETURN 1".to_vec());
    charge.jeton = Some(emis.clone());
    let enveloppe = sceller(&cle, &charge).expect("scellement");
    porteur.deposer(a, b, enveloppe, 1).expect("dépôt");

    let transportee = porteur.retirer(&b).expect("l'enveloppe attend");
    let recue = ouvrir(&cle, &b, &transportee.octets).expect("déscellement");
    let jeton_recu = recue.jeton.expect("le jeton a traversé");

    // Le destinataire, lui, le vérifie — et l'action est bien celle qui a été
    // émise.
    let portee = jeton
        .verifier(&jeton_recu, "graph:write", 1_700_000_000)
        .expect("le jeton est valide à l'arrivée");
    assert_eq!(portee.action, "graph:write");

    // Le porteur, lui, n'a jamais vu l'action.
    let mut vu = Vec::new();
    for transition in porteur.ce_que_jai_vu() {
        vu.extend_from_slice(&transition.vers_octets());
    }
    assert!(!contient(&vu, b"graph:write"));
    assert_eq!(jeton_recu, emis, "le jeton traverse sans être modifié");
}

/// **Le porteur ne peut pas renier un routage.** Chaque transition est inscrite
/// dans un registre chaîné et signé, et chaque dépôt rend un reçu : une extrémité
/// peut opposer au porteur ce qu'il a livré, même s'il reconstruit son registre
/// et le resigne avec la clé qu'il détient.
#[test]
fn ce_que_le_porteur_a_livre_ne_peut_pas_etre_renie() {
    let (mut porteur, _, a, b) = porteur(8);
    let cle = cle_espace();
    let publique = CleJournal::depuis_octets([7u8; 32]).publique();

    let enveloppe =
        sceller(&cle, &Charge::nouvelle(a, b, REQUETE.as_bytes().to_vec())).expect("scellement");
    let recu = porteur
        .deposer(a, b, enveloppe, 1_700_000_000)
        .expect("dépôt");

    porteur
        .verifier(&publique)
        .expect("registre cohérent et signé");
    porteur
        .journal()
        .accorde_avec(&recu)
        .expect("le reçu de l'extrémité s'accorde");

    // Le porteur reconstruit son registre : autres destinataires, resigné.
    let mut reecrit =
        Porteur::neuf("porteur-alpha", CleJournal::depuis_octets([7u8; 32]), 8).expect("porteur");
    reecrit.enregistrer(a);
    reecrit.enregistrer(b);
    let (_, _, c) = adresses();
    reecrit.enregistrer(c);
    let autre = sceller(&cle, &Charge::nouvelle(a, c, b"RETURN 1".to_vec())).expect("scellement");
    reecrit.deposer(a, c, autre, 1_700_000_000).expect("dépôt");

    reecrit
        .verifier(&publique)
        .expect("le registre réécrit est cohérent et signé");

    match reecrit.journal().accorde_avec(&recu) {
        Err(ErreurJournal::ElementDifferent { sequence }) => assert_eq!(sequence, 0),
        autre => panic!("attendu ElementDifferent sur routage renié, obtenu {autre:?}"),
    }
}

/// Le protocole refuse une charge au-delà de la dernière classe — refusée, jamais
/// tronquée : une charge tronquée ferait exécuter un autre calcul.
#[test]
fn une_charge_trop_grande_est_refusee() {
    let (a, b, _) = adresses();
    let cle = cle_espace();
    let charge = Charge::nouvelle(a, b, vec![0u8; 70_000]);
    match sceller(&cle, &charge) {
        Err(ErreurAveugle::TropGrand { taille }) => assert_eq!(taille, charge.vers_octets().len()),
        autre => panic!("attendu TropGrand, obtenu {autre:?}"),
    }
}
