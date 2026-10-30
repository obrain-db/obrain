//! LA CLÉ PRIME SUR LE DROIT — la frontière, exécutable.
//!
//! Ce fichier est le critère de l'**étape 1** de l'ADR du 2026-09-15 :
//!
//! > *retirer la clé d'un espace rend ses données illisibles **même avec tous
//! > les droits IAM***
//!
//! Il le démontre dans les deux sens, et c'est ce qui le rend utile :
//!
//! 1. **Le droit est réellement accordé.** On ne triche pas en donnant une
//!    politique faible : l'utilisateur reçoit `graph:read`, `graph:write`,
//!    `iam:audit` **et** `iam:admin` sur l'espace, la garde est armée, et le
//!    verdict de la garde est bien `Allow`. On le vérifie avant de continuer.
//! 2. **Et cela ne suffit pas à lire.** Le contenu est scellé ; la garde peut
//!    autoriser tout ce qu'elle veut, elle ne fabrique pas de clé.
//!
//! C'est la distinction que ce dépôt a payée cher à confondre : un `Allow` est
//! une **décision**, et une décision peut être fausse. Une clé absente est une
//! **impossibilité**, et il n'y a rien à corriger.
//!
//! ## Ce que ce test ne prouve pas
//!
//! Que l'opérateur ne lise pas le clair **pendant** le calcul sur sa propre
//! machine. Aucun test logiciel ne peut le prouver. Ce que ce test établit est
//! plus étroit et plus solide : *le droit d'accès ne donne pas le contenu*.

use std::sync::Arc;

use obrain_coffre::cle::{CleEspace, ParametresKdf};
use obrain_coffre::{Coffre, CoffreError};
use obrain_core::graph::traits::GraphStoreMut;
use obrain_iam::garde::Garde;
use obrain_iam::model::PolicyEffect;
use obrain_iam::orn::Orn;
use obrain_iam::provider::{AuthProvider, AuthRequest, ObrainIamProvider};
use obrain_iam::store::IamStore;
use obrain_substrate::SubstrateStore;

const COMPTE: &str = "obrain";
const ESPACE: &str = "espace-du-client";
const PHRASE: &str = "phrase-que-l-engine-ne-connait-pas";
const PARAMS: ParametresKdf = ParametresKdf {
    m_cost_kio: 8 * 1024,
    t_cost: 2,
    p_cost: 1,
};
const GRAPHE: &[u8] = b"RACINE-BANQUE-ALPHA compte-0000 FR76-SECRET-0000";

/// Monte une garde armée dont l'utilisateur détient **tous** les droits sur
/// l'espace : lecture, écriture, audit et administration.
fn garde_avec_tous_les_droits() -> (Garde, String) {
    let magasin: Arc<dyn GraphStoreMut> = Arc::new(SubstrateStore::open_tempfile().unwrap());
    let store = Arc::new(IamStore::new(Arc::clone(&magasin)));

    store.create_user("u1", "client", None).unwrap();
    store.create_role("r1", "tout", None).unwrap();
    store
        .create_policy(
            "p1",
            "tous les droits sur l'espace",
            PolicyEffect::Allow,
            &["graph:read", "graph:write", "iam:audit", "iam:admin"],
            &[Orn::tenant(COMPTE, ESPACE)],
        )
        .unwrap();
    store.attach_role("u1", "r1").unwrap();
    store.attach_policy_to_role("r1", "p1").unwrap();

    let provider = ObrainIamProvider::new(Arc::clone(&store), COMPTE);
    let identite = provider
        .authenticate(&AuthRequest::Internal {
            username: "client".to_string(),
        })
        .unwrap();
    let jeton = provider.create_session(&identite, 3600).unwrap().token;

    (Garde::nouvelle(magasin, true, COMPTE), jeton)
}

/// **Le cœur de l'étape 1.** Tous les droits, aucune clé : rien à lire.
#[test]
fn tous_les_droits_ne_donnent_pas_le_contenu() {
    let (garde, jeton) = garde_avec_tous_les_droits();
    let session = garde
        .authentifier(Some(&jeton))
        .expect("le jeton du client doit être lu")
        .expect("une session doit s'ouvrir");

    // 1. Le droit est bien accordé — on le prouve, sinon le test ne prouve rien.
    for action in ["graph:read", "graph:write", "iam:audit", "iam:admin"] {
        garde
            .controler_action(Some(&session), action, ESPACE)
            .unwrap_or_else(|e| panic!("{action} doit être autorisé, or : {e}"));
    }

    // 2. Le contenu est scellé par le client, avec une clé que l'engine n'a pas.
    let cle = CleEspace::engendrer(ESPACE).unwrap();
    let enveloppe = cle.envelopper_avec_phrase(PHRASE, PARAMS).unwrap();
    let coffre = Coffre::neuf(&cle).unwrap();
    let scelle = coffre.sceller(GRAPHE).unwrap();

    // 3. La garde dit `Allow` — et ne rend pas le contenu pour autant.
    garde
        .controler_action(Some(&session), "graph:read", ESPACE)
        .expect("la garde autorise");

    let cle_improvisee = CleEspace::engendrer(ESPACE).unwrap();
    assert!(
        matches!(
            Coffre::neuf(&cle_improvisee).unwrap().desceller(&scelle),
            Err(CoffreError::Authentification)
        ),
        "un `Allow` de la garde ne fabrique pas de clé : le contenu reste illisible"
    );

    // 4. Et l'engine ne peut pas davantage se donner la clé depuis l'enveloppe :
    //    une phrase devinée produit une KEK qui ne descelle rien.
    let kek_devinee = enveloppe.deriver_kek("phrase-que-l-engine-ne-connait-pa").unwrap();
    assert!(matches!(
        CleEspace::desceller(&enveloppe, &kek_devinee),
        Err(CoffreError::CleMaitresseIncorrecte)
    ));

    // 5. Le client, lui, lit — parce qu'il a la phrase, pas parce qu'il a un droit.
    let kek = enveloppe.deriver_kek(PHRASE).unwrap();
    let cle_du_client = CleEspace::desceller(&enveloppe, &kek).unwrap();
    assert_eq!(
        Coffre::neuf(&cle_du_client).unwrap().desceller(&scelle).unwrap(),
        GRAPHE
    );
}

/// **L'autre moitié, et elle compte autant** : les droits restent nécessaires.
/// La confidentialité ne remplace pas le contrôle d'accès — l'ADR le dit pour
/// G5, et l'inverse serait un trou : un jeton de capacité mal borné, ou un
/// espace élargi, doivent être refusés par la garde comme avant.
#[test]
fn les_droits_restent_necessaires() {
    let (garde, jeton) = garde_avec_tous_les_droits();
    let session = garde.authentifier(Some(&jeton)).unwrap().unwrap();

    // Hors de l'espace accordé, la garde refuse — la clé n'y change rien.
    assert!(
        garde
            .controler_action(Some(&session), "graph:read", "espace-d-un-autre")
            .is_err(),
        "un espace non détenu doit être refusé"
    );

    // Sans jeton, la garde armée refuse.
    assert!(
        garde.authentifier(None).is_err(),
        "une garde armée sans identité doit refuser"
    );

    // Et le verdict est journalisé : un `Allow` et un refus laissent une trace.
    let piste = garde.audit(50);
    assert!(
        !piste.is_empty(),
        "les décisions de sécurité doivent être écrites dans la piste d'audit"
    );
}

/// Un jeton de capacité ne remplace pas un droit sur le mauvais espace : les
/// deux mécanismes se composent, ils ne se substituent pas.
#[test]
fn un_jeton_de_capacite_reste_borne_a_son_espace() {
    use obrain_coffre::Jeton;

    let cle = CleEspace::engendrer(ESPACE).unwrap();
    let jeton = Jeton::neuf(&cle).unwrap();
    let capacite = jeton.emettre("graph:read", 2_000_000_000).unwrap();

    // Le nom de l'action ne traverse pas le réseau, et l'engine ne peut pas
    // vérifier le jeton sans la clé — donc il ne peut pas non plus l'élargir.
    assert!(
        !capacite
            .windows(b"graph:read".len())
            .any(|f| f == b"graph:read"),
        "le porteur ne doit pas pouvoir lire ce que la capacité autorise"
    );

    let cle_improvisee = CleEspace::engendrer(ESPACE).unwrap();
    assert!(
        Jeton::neuf(&cle_improvisee)
            .unwrap()
            .verifier(&capacite, "graph:read", 1)
            .is_err(),
        "un jeton non vérifiable sans la clé ne peut pas être élargi par l'engine"
    );

    // Et la garde, elle, continue de trancher sur l'espace — indépendamment.
    let (garde, jeton_session) = garde_avec_tous_les_droits();
    let session = garde.authentifier(Some(&jeton_session)).unwrap().unwrap();
    garde
        .controler_action(Some(&session), "graph:read", ESPACE)
        .expect("dans l'espace détenu, la garde autorise");
}
