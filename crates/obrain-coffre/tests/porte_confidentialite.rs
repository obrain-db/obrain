//! LA PORTE DE CONFIDENTIALITÉ — les critères de l'ADR, exécutables.
//!
//! Chaque test de ce fichier correspond à un critère **falsifiable** de
//! `docs/ADR-2026-09-15-GARANTIE-CONFIDENTIALITE-CALCUL.md`. Un critère qu'on ne
//! peut pas exécuter est une intention ; ce fichier existe pour qu'il n'y en ait
//! pas.
//!
//! | Test | Critère de l'ADR |
//! |---|---|
//! | [`hexdump_du_coffre_ne_rend_aucun_litteral_du_graphe`] | étape 2 : « `hexdump` du `.obrain` ne rend aucun littéral du graphe » |
//! | [`sans_la_cle_le_contenu_est_illisible_meme_avec_tous_les_octets`] | étape 1 : « retirer la clé rend les données illisibles » — ici, **sans dépendre d'aucun droit** |
//! | [`l_engine_transporte_sans_pouvoir_lire`] | étape 3 : « l'engine reçoit une enveloppe de taille fixe et ne peut en extraire ni la requête ni la cible » |
//! | [`deux_calculs_differents_ont_la_meme_taille`] | G3 : métadonnées de taille |
//! | [`le_nom_de_l_action_ne_traverse_pas_le_reseau`] | G3 : métadonnées d'action |
//! | [`substituer_un_resultat_est_detecte_par_le_client`] | G4 : intégrité |
//! | [`le_depot_ne_garde_aucune_cle_en_clair`] | G1 au repos |
//! | [`la_cle_ne_se_retrouve_pas_par_une_phrase_devinee`] | §9 : aucune porte de récupération |
//! | [`le_noyau_ne_contient_aucun_code_non_sur`] | §2 : la garantie ne repose pas sur du code non vérifiable |
//!
//! Ce que cette porte **ne** prouve pas, et qui est écrit ici pour ne pas être
//! vendu : que le clair ne soit pas lisible *pendant* le calcul sur la machine de
//! l'opérateur. Aucun test logiciel ne peut le prouver — c'est l'objet des §5 et
//! §7 de l'ADR, et la raison pour laquelle le calcul doit avoir lieu chez qui a
//! le droit de lire.

use std::path::Path;

use obrain_coffre::cle::{CleEspace, ParametresKdf};
use obrain_coffre::coffre::BLOC_DEFAUT;
use obrain_coffre::{
    ClasseTaille, Coffre, CoffreError, Enveloppe, EnveloppeCle, Jeton, ecrire_enveloppe,
    lire_enveloppe,
};

/// Paramètres au plancher : les tests paient le minimum autorisé, pas plus.
const PARAMS: ParametresKdf = ParametresKdf {
    m_cost_kio: 8 * 1024,
    t_cost: 2,
    p_cost: 1,
};

const ESPACE: &str = "espace-du-client";
const PHRASE: &str = "phrase-du-client-que-l-engine-ne-connait-pas";

/// Un graphe de test, écrit comme le ferait obrain : des littéraux
/// reconnaissables, et de quoi les chercher ensuite.
fn graphe_de_test() -> Vec<u8> {
    let mut graphe = Vec::new();
    graphe.extend_from_slice(b"RACINE-BANQUE-ALPHA\n");
    for index in 0..200 {
        graphe.extend_from_slice(
            format!(
                "  (compte-{index}:Compte {{iban: \"FR76-SECRET-{index:04}\", solde: {index}}}),\n"
            )
            .as_bytes(),
        );
    }
    graphe.extend_from_slice(b"FIN-GRAPHE\n");
    graphe
}

fn cle_du_client() -> CleEspace {
    CleEspace::engendrer(ESPACE).expect("entropie disponible")
}

/// L'enveloppe de clé du client : la clé scellée sous sa phrase.
fn enveloppe_de_cle(cle: &CleEspace) -> EnveloppeCle {
    cle.envelopper_avec_phrase(PHRASE, PARAMS)
        .expect("enveloppe de clé")
}

/// Cherche `aiguille` dans `foin` — la version de test, sans dépendance.
fn contient(foin: &[u8], aiguille: &[u8]) -> bool {
    !aiguille.is_empty() && foin.windows(aiguille.len()).any(|f| f == aiguille)
}

// ---------------------------------------------------------------------------
// Étape 2 — le disque ne rend aucun littéral
// ---------------------------------------------------------------------------

/// **Critère de l'étape 2** : `hexdump` du magasin ne rend aucun littéral du
/// graphe.
#[test]
fn hexdump_du_coffre_ne_rend_aucun_litteral_du_graphe() {
    let graphe = graphe_de_test();
    let coffre = Coffre::neuf(&cle_du_client()).expect("coffre");
    let scelle = coffre.sceller(&graphe).expect("scellement");

    for litteral in [
        &b"RACINE-BANQUE-ALPHA"[..],
        b"compte-0",
        b"FR76-SECRET-0000",
        b"FR76-SECRET-0199",
        b"FIN-GRAPHE",
        b"iban",
        b"solde",
    ] {
        assert!(
            !contient(&scelle, litteral),
            "le littéral {:?} ne doit pas apparaître dans les octets scellés",
            String::from_utf8_lossy(litteral)
        );
    }

    // Et sur le disque, la même chose — c'est le `.obrain` du critère.
    let dossier = tempfile::tempdir().expect("dossier temporaire");
    let chemin = dossier.path().join("espace.obrain");
    std::fs::write(&chemin, &scelle).expect("écriture");
    let relu = std::fs::read(&chemin).expect("relecture");
    assert!(!contient(&relu, b"RACINE-BANQUE-ALPHA"));
    assert!(!contient(&relu, b"FR76-SECRET-0123"));
}

// ---------------------------------------------------------------------------
// Étape 1 — sans la clé, rien, et sans dépendre d'un droit
// ---------------------------------------------------------------------------

/// **Critère de l'étape 1**, dans sa version la plus forte : la clé manquante est
/// une *impossibilité*, pas un refus. Il n'y a aucun droit à accorder, aucun
/// administrateur à convaincre, aucune politique à corriger.
#[test]
fn sans_la_cle_le_contenu_est_illisible_meme_avec_tous_les_octets() {
    let graphe = graphe_de_test();
    let cle = cle_du_client();
    let coffre = Coffre::neuf(&cle).expect("coffre");

    let scelle = coffre.sceller(&graphe).expect("scellement");
    let enveloppe = enveloppe_de_cle(&cle);

    // — Vue de l'engine : tous les octets, aucune clé. —
    let octets_de_l_engine: Vec<u8> = [scelle.clone(), enveloppe.octets().to_vec()].concat();

    // 1. Sans phrase, aucune clé d'espace ne se construit : une phrase fausse
    //    dérive une KEK qui ne descelle rien.
    let kek_devine = enveloppe
        .deriver_kek("phrase-du-client-que-l-engine-ne-connait-pa")
        .expect("dérivation");
    match CleEspace::desceller(&enveloppe, &kek_devine) {
        Err(CoffreError::CleMaitresseIncorrecte) => {}
        autre => panic!("une phrase fausse ne doit rien desceller, reçu {autre:?}"),
    }

    // 2. Même en fabriquant une clé pour le bon espace, le contenu résiste :
    //    la clé d'espace n'est pas déductible de l'espace.
    let cle_improvisee = CleEspace::engendrer(ESPACE).expect("entropie");
    let coffre_improvise = Coffre::neuf(&cle_improvisee).expect("coffre");
    assert!(
        matches!(
            coffre_improvise.desceller(&scelle),
            Err(CoffreError::Authentification)
        ),
        "tous les octets ne suffisent pas : il faut la clé"
    );

    // 3. Et l'impossibilité est nommée comme telle, jamais comme un refus.
    let dossier = tempfile::tempdir().expect("dossier");
    let absent = dossier.path().join("cle-que-l-engine-n-a-pas.cle");
    match lire_enveloppe(&absent) {
        Err(erreur) => assert!(
            erreur.est_une_impossibilite(),
            "une clé absente est une impossibilité, pas un refus : {erreur}"
        ),
        Ok(_) => panic!("aucune clé ne doit se lire là où il n'y en a pas"),
    }

    // 4. Et le client, lui, lit — parce qu'il a la phrase.
    let kek = enveloppe.deriver_kek(PHRASE).expect("dérivation du client");
    let cle_retrouvee = CleEspace::desceller(&enveloppe, &kek).expect("déscellement");
    let coffre_client = Coffre::neuf(&cle_retrouvee).expect("coffre du client");
    assert_eq!(
        coffre_client.desceller(&scelle).expect("lecture légitime"),
        graphe
    );

    // L'engine a bien eu tous les octets — la preuve qu'il ne lui manquait que
    // la clé.
    assert_eq!(octets_de_l_engine.len(), scelle.len() + 128);
}

// ---------------------------------------------------------------------------
// Étape 3 — l'engine transporte sans comprendre
// ---------------------------------------------------------------------------

/// **Critère de l'étape 3** : l'engine reçoit une enveloppe de taille fixe et ne
/// peut en extraire ni la requête, ni la cible, ni l'action déléguée.
#[test]
fn l_engine_transporte_sans_pouvoir_lire() {
    let cle = cle_du_client();
    let enveloppe = Enveloppe::neuf(&cle).expect("enveloppes");
    let jeton = Jeton::neuf(&cle).expect("jetons");

    let requete = b"MATCH (n:Compte {iban: 'FR76-SECRET-0042'}) RETURN n.solde";
    let paquet = enveloppe.sceller(requete).expect("scellement");
    let capacite = jeton
        .emettre("graph:read:compte-prive", 2_000_000_000)
        .expect("émission");

    // — Ce que l'engine reçoit, et ce qu'il peut en dire —
    for litteral in [
        &b"MATCH"[..],
        b"Compte",
        b"FR76-SECRET-0042",
        b"solde",
        b"graph:read",
        b"compte-prive",
    ] {
        assert!(
            !contient(&paquet, litteral) && !contient(&capacite, litteral),
            "l'engine ne doit pas pouvoir lire {:?}",
            String::from_utf8_lossy(litteral)
        );
    }

    // Il apprend la classe de l'enveloppe — et rien de plus fin que la classe.
    assert_eq!(
        ClasseTaille::lue(&paquet).expect("classe lisible"),
        ClasseTaille::Octets256
    );
    assert_eq!(paquet.len(), ClasseTaille::Octets256.taille_totale());

    // Il ne peut ni vérifier ni forger le jeton : les deux exigent la clé, qu'il
    // n'a pas. Une clé improvisée pour le même espace ne suffit pas.
    let cle_improvisee = CleEspace::engendrer(ESPACE).expect("entropie");
    assert!(matches!(
        Jeton::neuf(&cle_improvisee).expect("jetons").verifier(
            &capacite,
            "graph:read:compte-prive",
            1
        ),
        Err(CoffreError::Authentification)
    ));

    // Le client, lui, vérifie et lit.
    assert_eq!(
        jeton
            .verifier(&capacite, "graph:read:compte-prive", 1)
            .expect("vérification légitime")
            .action,
        "graph:read:compte-prive"
    );
    assert_eq!(
        enveloppe.desceller(&paquet).expect("déscellement légitime"),
        requete
    );
}

// ---------------------------------------------------------------------------
// G3 — les métadonnées
// ---------------------------------------------------------------------------

/// **G3** : deux calculs différents, de tailles très différentes, produisent des
/// enveloppes de longueur identique. L'observateur ne peut pas les distinguer par
/// leur taille.
#[test]
fn deux_calculs_differents_ont_la_meme_taille() {
    let enveloppe = Enveloppe::neuf(&cle_du_client()).expect("enveloppes");

    let court = enveloppe.sceller(b"RETURN 1").expect("scellement");
    let moyen = enveloppe.sceller(&[b'x'; 200]).expect("scellement");
    let autre = enveloppe
        .sceller(b"MATCH (n) WHERE n.secret = 'oui' RETURN count(n)")
        .expect("scellement");

    assert_eq!(court.len(), moyen.len());
    assert_eq!(moyen.len(), autre.len());

    // Et pour une conversation entière, la classe peut être imposée : plus aucune
    // variation de taille, même entre classes.
    let classe = ClasseTaille::Octets4096;
    let a = enveloppe.sceller_dans(b"a", classe).expect("scellement");
    let b = enveloppe
        .sceller_dans(&vec![b'z'; 4000], classe)
        .expect("scellement");
    assert_eq!(a.len(), b.len());
    assert_eq!(a.len(), classe.taille_totale());
}

/// **G3** : le nom de l'action ne traverse pas le réseau — l'engine route une
/// capacité sans savoir ce qu'elle autorise.
#[test]
fn le_nom_de_l_action_ne_traverse_pas_le_reseau() {
    let jeton = Jeton::neuf(&cle_du_client()).expect("jetons");
    let capacite = jeton
        .emettre("iam:admin:espace-tres-sensible", 2_000_000_000)
        .expect("émission");

    assert!(!contient(&capacite, b"iam:admin"));
    assert!(!contient(&capacite, b"espace-tres-sensible"));
    assert!(!contient(&capacite, b"sensible"));
    assert_eq!(
        capacite.len(),
        obrain_coffre::jeton::TAILLE_JETON,
        "la taille du jeton ne dépend pas de la longueur de l'action"
    );
}

// ---------------------------------------------------------------------------
// G4 — l'intégrité, y compris contre l'engine
// ---------------------------------------------------------------------------

/// **G4** : l'engine qui substitue un résultat — ou qui en modifie un octet — est
/// détecté par le client. Chaque zone est essayée, y compris la toute dernière.
#[test]
fn substituer_un_resultat_est_detecte_par_le_client() {
    let cle = cle_du_client();
    let coffre = Coffre::neuf(&cle).expect("coffre");
    let resultat = b"solde: 1 000 000";

    let scelle = coffre.sceller(resultat).expect("scellement");
    assert_eq!(coffre.desceller(&scelle).expect("lecture"), resultat);

    // L'engine modifie un octet, n'importe où.
    for position in [0, 4, 5, 10, 30, 40, 60, scelle.len() / 2, scelle.len() - 1] {
        let mut faux = scelle.clone();
        faux[position] ^= 0x01;
        assert!(
            coffre.desceller(&faux).is_err(),
            "modifier l'octet {position} doit être détecté"
        );
    }

    // L'engine tronque, rallonge, ou remplace le contenu par celui d'un autre
    // coffre scellé par la même clé.
    assert!(coffre.desceller(&scelle[..scelle.len() - 1]).is_err());

    let autre = coffre.sceller(b"solde: 0").expect("scellement");
    let mut substitue = scelle.clone();
    let debut = obrain_coffre::coffre::TETE_COFFRE + obrain_coffre::coffre::SCEAU_ENTETE;
    let longueur = autre.len() - debut;
    substitue[debut..debut + longueur.min(scelle.len() - debut)]
        .copy_from_slice(&autre[debut..debut + longueur.min(scelle.len() - debut)]);
    assert!(coffre.desceller(&substitue).is_err());
}

// ---------------------------------------------------------------------------
// G1 — le dépôt
// ---------------------------------------------------------------------------

/// **G1** : ce qui est posé sur le disque ne contient aucune clé en clair, et le
/// fichier de clé lui-même est refusé s'il est lisible par d'autres.
#[test]
fn le_depot_ne_garde_aucune_cle_en_clair() {
    let cle = cle_du_client();
    let enveloppe = enveloppe_de_cle(&cle);

    let dossier = tempfile::tempdir().expect("dossier");
    let chemin = dossier.path().join("espace.cle");
    ecrire_enveloppe(&chemin, &enveloppe).expect("écriture");

    let octets = std::fs::read(&chemin).expect("relecture");
    assert_eq!(octets.len(), 128);
    assert!(
        !contient(&octets, PHRASE.as_bytes()),
        "la phrase ne doit pas être sur le disque"
    );
    // La clé d'espace elle-même n'est pas dans le fichier : on la cherche par son
    // effet, faute de pouvoir la lire.
    let kek_relu = lire_enveloppe(&chemin)
        .expect("lecture")
        .deriver_kek(PHRASE)
        .expect("dérivation");
    let cle_relue = CleEspace::desceller(&enveloppe, &kek_relu).expect("déscellement");
    assert_eq!(cle_relue.espace(), cle.espace());

    let marqueur = b"TEMOIN-DE-CLE";
    let coffre = Coffre::neuf(&cle_relue).expect("coffre");
    assert!(!contient(
        &coffre.sceller(marqueur).expect("scellement"),
        marqueur
    ));
}

/// **§9 de l'ADR** : il n'y a aucune porte de récupération. Une phrase devinée ne
/// rend rien, et la deviner coûte le prix d'Argon2id à chaque essai.
#[test]
fn la_cle_ne_se_retrouve_pas_par_une_phrase_devinee() {
    let cle = cle_du_client();
    let enveloppe = enveloppe_de_cle(&cle);

    let candidats = [
        "",
        "motdepasse",
        "phrase-du-client-que-l-engine-ne-connait-pa",
        "phrase-du-client-que-l-engine-ne-connait-pas ",
        "PHRASE-DU-CLIENT-QUE-L-ENGINE-NE-CONNAIT-PAS",
    ];
    for candidat in candidats {
        let kek = enveloppe.deriver_kek(candidat).expect("dérivation");
        assert!(
            CleEspace::desceller(&enveloppe, &kek).is_err(),
            "la phrase {candidat:?} ne doit pas desceller"
        );
    }

    let kek = enveloppe.deriver_kek(PHRASE).expect("dérivation");
    assert!(CleEspace::desceller(&enveloppe, &kek).is_ok());
}

// ---------------------------------------------------------------------------
// La force du noyau lui-même
// ---------------------------------------------------------------------------

/// **§2** : la garantie ne doit pas reposer sur du code non vérifiable. Le noyau
/// déclare `forbid(unsafe_code)` et n'en contient aucun.
///
/// Le vrai garant est le compilateur — cet attribut fait échouer la compilation.
/// Ce test vérifie que **l'attribut est toujours là** : le retirer passerait
/// inaperçu, et c'est exactement le genre de silence que ce dépôt combat.
#[test]
fn le_noyau_ne_contient_aucun_code_non_sur() {
    let racine = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

    let lib = std::fs::read_to_string(racine.join("lib.rs")).expect("lib.rs lisible");
    assert!(
        lib.contains("forbid(unsafe_code)"),
        "lib.rs doit déclarer forbid(unsafe_code) : c'est le garant, pas une convention"
    );

    let mut fichiers = 0usize;
    for entree in std::fs::read_dir(&racine).expect("lecture de src") {
        let chemin = entree.expect("entrée").path();
        if chemin.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        fichiers += 1;
        let source = std::fs::read_to_string(&chemin).expect("source lisible");
        for (numero, ligne) in source.lines().enumerate() {
            let sans_espaces: String = ligne.split_whitespace().collect::<Vec<_>>().join(" ");
            for motif in [
                "unsafe fn",
                "unsafe impl",
                "unsafe trait",
                "unsafe {",
                "unsafe{",
            ] {
                assert!(
                    !sans_espaces.contains(motif),
                    "{}:{} contient « {motif} » — le noyau doit rester sans code non sûr",
                    chemin.display(),
                    numero + 1
                );
            }
        }
    }
    assert!(
        fichiers >= 7,
        "la porte doit lire tous les modules du noyau"
    );
}

/// Le format d'enveloppe de clé porte une version, et une version inconnue est
/// refusée plutôt que mal interprétée.
#[test]
fn une_version_inconnue_est_refusee() {
    let mut octets = *enveloppe_de_cle(&cle_du_client()).octets();
    octets[6] = 42;
    assert!(matches!(
        EnveloppeCle::depuis_octets(&octets),
        Err(CoffreError::Format(_))
    ));
}

/// Le coffre par défaut a bien la taille de bloc annoncée — la constante publique
/// et le comportement ne peuvent pas diverger en silence.
#[test]
fn la_taille_de_bloc_par_defaut_est_celle_annoncee() {
    let coffre = Coffre::neuf(&cle_du_client()).expect("coffre");
    let clair = vec![0u8; BLOC_DEFAUT as usize];
    let scelle = coffre.sceller(&clair).expect("scellement");

    let attendu = obrain_coffre::coffre::TETE_COFFRE
        + obrain_coffre::coffre::SCEAU_ENTETE
        + obrain_coffre::cle::LONGUEUR_NONCE
        + clair.len()
        + obrain_coffre::cle::LONGUEUR_ETIQUETTE;
    assert_eq!(scelle.len(), attendu);
    assert_eq!(coffre.desceller(&scelle).expect("lecture"), clair);
}
