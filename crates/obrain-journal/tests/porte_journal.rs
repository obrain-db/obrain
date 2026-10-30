//! # La porte du journal — ce qu'un opérateur peut faire, et ce qu'il ne peut pas
//!
//! Chaque test de ce fichier correspond à une capacité de l'opérateur, prise une
//! à une. Elles sont toutes légitimes pour lui : c'est **son** journal, il en
//! détient la clé. La question n'est pas de l'en empêcher — c'est impossible —
//! mais de savoir laquelle de ses manipulations laisse une trace, et pour qui.
//!
//! | L'opérateur… | Attrapé par | Vu par |
//! |---|---|---|
//! | modifie une entrée | la chaîne | quiconque vérifie |
//! | retire une entrée | la chaîne | quiconque vérifie |
//! | réordonne | la chaîne | quiconque vérifie |
//! | tronque la queue | la tête souvenue | qui a gardé la tête |
//! | reconstruit **et resigne** l'histoire | le reçu du client | **le client seul** |
//! | n'inscrit pas un calcul | les séquences manquantes | **le client seul** |
//!
//! Les deux dernières lignes sont les seules qui tiennent contre lui, et c'est
//! pour elles que la crate existe.

use obrain_journal::{
    CleJournal, Entree, ErreurJournal, Journal, empreinte_de, sequences_manquantes,
};

/// Taille d'une entrée sérialisée — tous les champs sont à taille fixe.
const TAILLE_ENTREE: usize = 32 + 8 + 8 + 32 + 32 + 64;

/// Où commencent les entrées dans le flux : magic(8) + longueur(2) + nom + n(8).
fn debut_des_entrees(identifiant: &str) -> usize {
    8 + 2 + identifiant.len() + 8
}

const IDENTIFIANT: &str = "espace-alpha";

fn cle_de_test() -> CleJournal {
    CleJournal::depuis_octets([42u8; 32])
}

/// Un journal de `n` entrées, toutes signées, avec des charges distinctes.
fn journal_de(n: usize) -> (Journal, CleJournal) {
    let cle = cle_de_test();
    let mut journal = Journal::neuf(IDENTIFIANT).expect("journal");
    for i in 0..n {
        let quoi = empreinte_de(format!("calcul-{i}").as_bytes());
        journal
            .recevoir(&cle, quoi, 1_700_000_000 + i as u64)
            .expect("écriture");
    }
    (journal, cle)
}

// ---------------------------------------------------------------------------
// Ce que la chaîne attrape
// ---------------------------------------------------------------------------

#[test]
fn la_chaine_tient_et_la_tete_avance_a_chaque_ecriture() {
    let (mut journal, cle) = journal_de(0);
    let genese = journal.tete();
    let vide = journal.tete();

    let mut tetes = vec![genese];
    for i in 0..5 {
        let quoi = empreinte_de(format!("calcul-{i}").as_bytes());
        journal.recevoir(&cle, quoi, 1_700_000_000).expect("écriture");
        tetes.push(journal.tete());
    }

    journal.verifier().expect("chaîne intacte");
    journal
        .verifier_signatures(&cle.publique())
        .expect("signatures valides");
    assert_eq!(journal.longueur(), 5);

    // La tête change à chaque écriture, et aucune ne revient.
    let mut vues = tetes.clone();
    vues.sort_unstable();
    vues.dedup();
    assert_eq!(vues.len(), tetes.len(), "deux têtes identiques");

    // Et un journal vide a bien la tête de sa genèse.
    assert_eq!(vide, Journal::neuf(IDENTIFIANT).expect("journal").tete());
}

/// Modifier une entrée rompt la chaîne **une séquence plus loin** — l'entrée
/// touchée ne se dénonce pas elle-même, c'est la suivante qui ne la cite plus.
/// Le message nomme les deux, et c'est la séquence de la rupture que l'erreur
/// annonce : un « échec » sans localisation obligerait l'appelant à tout
/// revérifier lui-même.
#[test]
fn reecrire_une_entree_rompt_la_chaine() {
    let (journal, _) = journal_de(4);
    let octets = journal.vers_octets();

    // On modifie la charge de l'entrée 2, dans le flux persisté.
    let debut = debut_des_entrees(IDENTIFIANT) + 2 * TAILLE_ENTREE + 32 + 8 + 8;
    let mut falsifie = octets.clone();
    falsifie[debut] ^= 0x01;

    match Journal::depuis_octets(&falsifie) {
        Err(ErreurJournal::ChaineRompee { sequence, motif }) => {
            assert_eq!(
                sequence, 3,
                "la rupture se signale à l'entrée qui cite la modifiée"
            );
            assert!(
                motif.contains("l'entrée 2"),
                "le motif doit nommer l'entrée touchée : {motif}"
            );
        }
        autre => panic!("attendu ChaineRompee à la séquence 3, obtenu {autre:?}"),
    }
}

/// Retirer une entrée **et recoller l'en-tête** — ce que ferait un opérateur
/// soigneux — laisse la chaîne rompue.
#[test]
fn supprimer_une_entree_rompt_la_chaine() {
    let (journal, _) = journal_de(4);
    let octets = journal.vers_octets();
    let debut = debut_des_entrees(IDENTIFIANT);

    let mut recolle = Vec::new();
    recolle.extend_from_slice(&octets[..debut - 8]);
    recolle.extend_from_slice(&3u64.to_le_bytes()); // il en reste trois
    for i in [0usize, 1, 3] {
        let d = debut + i * TAILLE_ENTREE;
        recolle.extend_from_slice(&octets[d..d + TAILLE_ENTREE]);
    }

    assert_eq!(recolle.len(), debut - 8 + 8 + 3 * TAILLE_ENTREE);
    assert!(
        Journal::depuis_octets(&recolle).is_err(),
        "un journal amputé d'une entrée ne doit pas se charger"
    );
}

#[test]
fn reordonner_deux_entrees_rompt_la_chaine() {
    let (journal, _) = journal_de(4);
    let octets = journal.vers_octets();
    let debut = debut_des_entrees(IDENTIFIANT);

    let bloc = |i: usize| -> Vec<u8> {
        let d = debut + i * TAILLE_ENTREE;
        octets[d..d + TAILLE_ENTREE].to_vec()
    };

    let mut permute = Vec::new();
    permute.extend_from_slice(&octets[..debut]);
    for i in [0usize, 2, 1, 3] {
        permute.extend_from_slice(&bloc(i));
    }

    assert!(
        Journal::depuis_octets(&permute).is_err(),
        "deux entrées échangées ne doivent pas se charger"
    );
}

/// **La troncature, et pourquoi la chaîne seule ne suffit pas.** Un journal
/// amputé de sa queue est parfaitement cohérent : sa chaîne est intacte, elle est
/// simplement plus courte. Seule une tête conservée **ailleurs** le démasque.
#[test]
fn tronquer_est_detecte_par_la_tete_souvenue() {
    let (journal, _) = journal_de(4);
    let tete_souvenue = journal.tete();
    let octets = journal.vers_octets();
    let debut = debut_des_entrees(IDENTIFIANT);

    let mut tronque = Vec::new();
    tronque.extend_from_slice(&octets[..debut - 8]);
    tronque.extend_from_slice(&2u64.to_le_bytes()); // deux entrées sur quatre
    tronque.extend_from_slice(&octets[debut..debut + 2 * TAILLE_ENTREE]);

    let ampute = Journal::depuis_octets(&tronque)
        .expect("un journal tronqué est cohérent avec lui-même : c'est bien le problème");
    ampute.verifier().expect("sa chaîne est intacte");
    assert_eq!(ampute.longueur(), 2);

    match ampute.verifier_jusqua(tete_souvenue) {
        Err(ErreurJournal::TeteDifferente { .. }) => {}
        autre => panic!("attendu TeteDifferente, obtenu {autre:?}"),
    }

    // Et sans tête souvenue, la vérification de chaîne ne dit rien — c'est
    // exactement ce que la doc de la crate annonce.
    ampute.verifier().expect("la chaîne seule ne voit pas la troncature");
}

// ---------------------------------------------------------------------------
// Ce que seul le client attrape
// ---------------------------------------------------------------------------

/// **Le test qui porte toute la crate.**
///
/// L'opérateur détient la clé : il reconstruit une autre histoire de bout en
/// bout, chaîne valide **et** signatures valides. Ni `verifier` ni
/// `verifier_signatures` ne voient quoi que ce soit — et c'est normal, elles
/// vérifient une cohérence interne, pas une identité.
///
/// Ce qui le prend, c'est le reçu que le client a emporté. La signature de ce
/// reçu reste valide (elle n'a jamais cessé de l'être) : c'est la **rencontre**
/// entre le reçu et le journal qui révèle la réécriture.
#[test]
fn resigner_une_autre_histoire_est_detecte_par_le_recu_du_client() {
    let cle = cle_de_test();

    // L'histoire d'origine, et le reçu que le client emporte.
    let mut honnete = Journal::neuf(IDENTIFIANT).expect("journal");
    let recu_du_client = honnete
        .recevoir(&cle, empreinte_de(b"calcul honnete"), 1_700_000_000)
        .expect("écriture");
    honnete
        .recevoir(&cle, empreinte_de(b"calcul suivant"), 1_700_000_001)
        .expect("écriture");

    // L'opérateur reconstruit : même journal, mêmes séquences, autres charges,
    // et il resigne tout — il en a le droit, c'est sa clé.
    let mut reecrit = Journal::neuf(IDENTIFIANT).expect("journal");
    reecrit
        .recevoir(&cle, empreinte_de(b"calcul REECRIT"), 1_700_000_000)
        .expect("écriture");
    reecrit
        .recevoir(&cle, empreinte_de(b"calcul suivant"), 1_700_000_001)
        .expect("écriture");

    // Rien, dans le journal réécrit, ne signale l'anomalie.
    reecrit
        .verifier()
        .expect("la chaîne de l'histoire réécrite est valide");
    reecrit
        .verifier_signatures(&cle.publique())
        .expect("et ses signatures sont valides : c'est bien le problème");

    // Le client, lui, voit tout.
    match reecrit.accorde_avec(&recu_du_client) {
        Err(ErreurJournal::ElementDifferent { sequence }) => {
            assert_eq!(sequence, 0, "la séquence du reçu");
        }
        autre => panic!("attendu ElementDifferent sur réécriture, obtenu {autre:?}"),
    }

    // Et la signature du reçu n'a jamais cessé d'être valide : ce n'est pas elle
    // qui a cédé, c'est l'accord entre le reçu et le journal.
    recu_du_client
        .verifier_signature(&cle.publique())
        .expect("le reçu du client reste authentique");
}

/// **L'omission.** Un opérateur qui n'inscrit pas un calcul produit un journal
/// parfaitement cohérent : aucune chaîne, aucune signature ne peut le prendre.
/// Ce qui le prend, c'est un client qui sait combien de calculs il a lancés.
#[test]
fn omettre_un_calcul_est_detecte_par_les_recus_du_client() {
    let (mut journal, cle) = journal_de(0);

    // Le client lance cinq calculs et garde quatre reçus : le troisième ne lui a
    // jamais été rendu.
    let mut gardes: Vec<Entree> = Vec::new();
    for i in 0..5u64 {
        let quoi = empreinte_de(format!("calcul-{i}").as_bytes());
        let recu = journal.recevoir(&cle, quoi, 1_700_000_000 + i).expect("écriture");
        if i != 2 {
            gardes.push(recu);
        }
    }

    journal.verifier().expect("le journal est cohérent avec lui-même");
    assert_eq!(journal.longueur(), 5, "les cinq calculs sont bien inscrits");
    assert_eq!(
        sequences_manquantes(&gardes),
        vec![2],
        "le trou dans les reçus détenus est la seule trace de l'omission"
    );

    // Et aucun reçu détenu ne s'accorde mal : ce n'est pas une réécriture, c'est
    // un manque — deux signalements différents, et l'appelant doit pouvoir les
    // distinguer.
    for recu in &gardes {
        journal.accorde_avec(recu).expect("les reçus détenus s'accordent");
    }
}

#[test]
fn un_recu_d_un_autre_journal_ne_s_accorde_pas() {
    let cle = cle_de_test();
    let mut alpha = Journal::neuf("espace-alpha").expect("journal");
    let mut beta = Journal::neuf("espace-beta").expect("journal");

    let recu_alpha = alpha
        .recevoir(&cle, empreinte_de(b"calcul"), 1_700_000_000)
        .expect("écriture");
    beta.recevoir(&cle, empreinte_de(b"calcul"), 1_700_000_000)
        .expect("écriture");

    match beta.accorde_avec(&recu_alpha) {
        Err(ErreurJournal::JournalEtranger(_)) => {}
        autre => panic!("attendu JournalEtranger, obtenu {autre:?}"),
    }

    // Et la genèse est propre à chaque journal : deux journaux ne peuvent pas
    // s'échanger leurs entrées, même à la séquence zéro.
    assert_ne!(alpha.tete(), beta.tete());
}

#[test]
fn la_signature_d_une_autre_cle_est_refusee() {
    let (journal, _) = journal_de(3);
    let autre = CleJournal::depuis_octets([7u8; 32]);

    match journal.verifier_signatures(&autre.publique()) {
        Err(ErreurJournal::SignatureInvalide { sequence }) => assert_eq!(sequence, 0),
        autre => panic!("attendu SignatureInvalide, obtenu {autre:?}"),
    }

    // Une entrée seule se vérifie aussi hors ligne, avec la seule clé publique.
    let recu = &journal.entrees()[1];
    recu.verifier_signature(&cle_de_test().publique())
        .expect("signature du bon signataire");
    assert!(recu.verifier_signature(&autre.publique()).is_err());
}

// ---------------------------------------------------------------------------
// Persistance, et entrées hostiles
// ---------------------------------------------------------------------------

#[test]
fn le_tour_complet_par_les_octets_garde_tout() {
    let (journal, cle) = journal_de(6);
    let octets = journal.vers_octets();
    let relu = Journal::depuis_octets(&octets).expect("relecture");

    assert_eq!(relu.identifiant(), journal.identifiant());
    assert_eq!(relu.longueur(), journal.longueur());
    assert_eq!(relu.tete(), journal.tete());
    assert_eq!(relu.entrees(), journal.entrees());
    relu.verifier_signatures(&cle.publique())
        .expect("les signatures survivent au disque");
}

#[test]
fn des_octets_hostiles_ne_paniquent_jamais() {
    let (journal, _) = journal_de(2);
    let valide = journal.vers_octets();

    // Un générateur déterministe : une attaque doit se reproduire.
    let mut etat = 0x5EED_2026_0915u64;
    let mut suivant = move || {
        etat ^= etat >> 12;
        etat ^= etat << 25;
        etat ^= etat >> 27;
        etat.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };

    // Au hasard pur.
    for _ in 0..400 {
        let n = (suivant() % 400) as usize;
        let octets: Vec<u8> = (0..n).map(|_| (suivant() >> 24) as u8).collect();
        assert!(Journal::depuis_octets(&octets).is_err());
    }

    // Un en-tête valide, mais des longueurs absurdes : c'est là qu'une lecture
    // non bornée ferait tomber le processus.
    for (nom, valeur) in [
        ("nb = 2^40", 1u64 << 40),
        ("nb = u64::MAX", u64::MAX),
        ("nb = 0 avec un corps", 0),
    ] {
        let debut = debut_des_entrees(IDENTIFIANT);
        let mut forge = valide.clone();
        forge[debut - 8..debut].copy_from_slice(&valeur.to_le_bytes());
        assert!(
            Journal::depuis_octets(&forge).is_err(),
            "{nom} : un en-tête qui annonce l'absurde doit être refusé, pas alloué"
        );
    }

    // Un nom de journal annoncé plus long que ce qui est reçu.
    let mut forge = valide.clone();
    forge[8..10].copy_from_slice(&60000u16.to_le_bytes());
    assert!(Journal::depuis_octets(&forge).is_err());
}

#[test]
fn un_journal_rompu_ne_se_charge_pas() {
    let (journal, _) = journal_de(3);
    let octets = journal.vers_octets();

    // On casse le maillon précédent de la dernière entrée.
    let debut = debut_des_entrees(IDENTIFIANT) + 2 * TAILLE_ENTREE + 32 + 8 + 8 + 32;
    let mut rompu = octets.clone();
    rompu[debut] ^= 0xFF;

    assert!(
        Journal::depuis_octets(&rompu).is_err(),
        "charger un journal rompu reviendrait à écrire par-dessus"
    );
}

#[test]
fn un_identifiant_vide_ou_trop_long_est_refuse() {
    assert!(Journal::neuf("").is_err());
    assert!(Journal::neuf("   ").is_err());
    assert!(Journal::neuf(&"a".repeat(256)).is_err());
    assert!(Journal::neuf(&"a".repeat(255)).is_ok());
}
