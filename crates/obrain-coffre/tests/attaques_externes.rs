//! # La porte adverse — ce qu'un attaquant qui détient les octets tente
//!
//! Ce fichier est **indépendant** de `porte_confidentialite.rs` : il ne vérifie
//! pas que la crate fait ce qu'elle annonce, il essaie de la faire céder. Il
//! n'emploie que l'API publique, comme le ferait quelqu'un qui a le fichier
//! scellé, le disque, le réseau — et pas la clé.
//!
//! ## Ce que ces attaques supposent de l'adversaire
//!
//! * il **détient tous les octets** : le coffre scellé, l'enveloppe de clé, le
//!   jeton, l'en-tête en clair ;
//! * il **peut les modifier** : c'est le cas de l'opérateur de l'engine, du
//!   disque, du réseau, d'un mandataire ;
//! * il **peut les réordonner, les tronquer, les greffer** d'un objet à l'autre ;
//! * il **peut présenter des octets hostiles** : longueurs absurdes, magic
//!   valide et champs extrêmes, octets au hasard ;
//! * il **n'a pas la clé** — et c'est la seule chose qu'il n'a pas.
//!
//! ## Les deux invariants que ce fichier tient
//!
//! **I1 — aucun octet scellé n'est hors de l'authentification.** Si une seule
//! position d'un objet scellé peut être modifiée sans que le déscellement
//! échoue, cette position est un canal : l'attaquant y écrit et l'engine lit.
//! Le balayage exhaustif (une position à la fois, trois masques) le vérifie
//! position par position, sans qu'aucune liste de « champs importants » ait été
//! écrite à la main — c'est justement une liste écrite à la main qui laisserait
//! un champ de côté.
//!
//! **I2 — aucune entrée hostile ne fait tomber le processus.** Une panique dans
//! un serveur est un déni de service, et un déni de service est pire qu'une
//! fuite dans beaucoup de modèles de menace. Les longueurs annoncées dans un
//! en-tête viennent de l'adversaire : elles ne doivent produire ni division par
//! zéro, ni débordement, ni allocation géante — seulement un refus.
//!
//! ## Ce que ce fichier ne prétend pas
//!
//! Il ne vérifie pas la solidité d'AES-256-GCM, d'Argon2id ni de HKDF : ces
//! primitives sont employées telles quelles, et les auditer n'est pas le rôle
//! d'un test. Il ne vérifie pas non plus la sûreté du générateur d'entropie du
//! noyau. Il vérifie ce qui est **de la responsabilité de cette crate** : que
//! ses en-têtes, ses AAD, ses domaines de clé et ses bornes ne laissent pas de
//! porte.

use std::collections::HashSet;

use obrain_coffre::cle::{CleEspace, EnveloppeCle, LONGUEUR_NONCE};
use obrain_coffre::coffre::{BLOC_MAXIMUM, BLOC_MINIMUM, Coffre, SCEAU_ENTETE, TETE_COFFRE};
use obrain_coffre::enveloppe::{ClasseTaille, Enveloppe, TETE_ENVELOPPE};
use obrain_coffre::erreur::CoffreError;
use obrain_coffre::jeton::{Jeton, TETE_JETON};
use obrain_coffre::{ParametresKdf, ecrire_enveloppe, lire_enveloppe};

// ---------------------------------------------------------------------------
// Outils
// ---------------------------------------------------------------------------

/// Le bloc de test : le plus petit accepté, pour que « plusieurs blocs » soit
/// atteignable sans écrire des mégaoctets. Les bornes du format sont ainsi
/// exercées par le bas, là où les incohérences de longueur se voient.
const BLOC: u32 = BLOC_MINIMUM;

fn cle(espace: &str) -> CleEspace {
    CleEspace::engendrer(espace).expect("l'entropie doit être disponible")
}

fn coffre(espace: &str) -> Coffre {
    Coffre::neuf(&cle(espace)).expect("HKDF doit réussir")
}

/// L'échec est la seule issue attendue : on ne regarde pas *quelle* erreur, on
/// vérifie qu'aucun clair ne sort. Le balayage ne doit pas dépendre du choix des
/// variantes d'erreur — sinon il testerait les messages, pas la garantie.
#[track_caller]
fn doit_refuser<T: core::fmt::Debug>(resultat: obrain_coffre::Resultat<T>, ou: &str) {
    match resultat {
        Ok(valeur) => panic!("ACCEPTÉ ce qui devait être refusé ({ou}) : {valeur:?}"),
        Err(_) => {}
    }
}

fn contient(foin: &[u8], aiguille: &[u8]) -> bool {
    foin.windows(aiguille.len()).any(|f| f == aiguille)
}

/// Un générateur déterministe : les attaques doivent être **reproductibles**.
/// Un test adverse qui change à chaque exécution ne se reproduit pas quand il
/// trouve quelque chose — et c'est précisément quand il trouve que la
/// reproductibilité compte.
struct Hasard(u64);

impl Hasard {
    fn neuf(graine: u64) -> Self {
        Self(graine | 1)
    }

    fn suivant(&mut self) -> u64 {
        // xorshift64* : trois décalages et une multiplication, sans dépendance.
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn octets(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| (self.suivant() >> 24) as u8).collect()
    }
}

/// La charge utile de test : un motif qui ne doit apparaître **nulle part** dans
/// les octets scellés, et qui est assez long pour n'être pas confondu avec du
/// bruit par hasard.
const SECRET_DU_CALCUL: &[u8] = b"graph:read MATCH (n:Client) WHERE n.iban = 'FR76-LE-SECRET-A-NE-PAS-LIRE'";

fn charge(multiplier: usize) -> Vec<u8> {
    let mut charge = Vec::new();
    for _ in 0..multiplier {
        charge.extend_from_slice(SECRET_DU_CALCUL);
        charge.push(b'|');
    }
    charge
}

/// Une charge qui occupe **exactement** `n` blocs pleins.
///
/// Nécessaire parce que la queue d'un coffre n'est pas bourrée : un contenu de
/// quelques dizaines d'octets tient dans **un** bloc, quel que soit le nombre de
/// répétitions du motif. Les tests qui parlent de « trois blocs » doivent donc
/// les remplir.
fn charge_blocs(n: usize) -> Vec<u8> {
    let mut charge = charge(1);
    charge.resize(n * BLOC as usize, b'.');
    charge
}

/// La taille fixe d'un coffre : en-tête + sceau d'en-tête.
const FIXE: usize = TETE_COFFRE + SCEAU_ENTETE;

/// Le coût d'un bloc scellé : nonce + étiquette, **sans** la charge.
const PAR_BLOC: usize = LONGUEUR_NONCE + 16;

/// La longueur scellée attendue pour une charge de `longueur` octets.
///
/// La queue n'étant pas bourrée, c'est une fonction **exacte** de la longueur —
/// et c'est précisément ce que le test de fuite de longueur mesure plus bas.
fn longueur_scellee(longueur: usize, bloc: usize) -> usize {
    let blocs = longueur.div_ceil(bloc);
    FIXE + blocs * PAR_BLOC + longueur
}

// ---------------------------------------------------------------------------
// I1 — le balayage exhaustif
// ---------------------------------------------------------------------------

/// **Le test central de ce fichier.** Une position à la fois, sur *tout* l'objet
/// scellé : en-tête, sceau d'en-tête, nonces, chiffrés, étiquettes.
///
/// Trois masques sont employés parce qu'un seul bit ne suffit pas à réveiller
/// certains chemins : `0x01` change un compteur d'un cran, `0x80` change le bit
/// de poids fort d'un octet, `0xFF` le complète. Une position dont les trois
/// mutations passent est une position qui n'est lue par aucune vérification.
#[test]
fn aucun_octet_du_coffre_n_echappe_a_l_authentification() {
    let c = coffre("e1");
    let clair = charge_blocs(3); // trois blocs pleins : les index entrent en jeu
    let scelle = c.sceller_avec_bloc(&clair, BLOC).expect("scellement");
    assert_eq!(
        scelle.len(),
        longueur_scellee(clair.len(), BLOC as usize),
        "la forme du scellé n'est pas celle annoncée par les constantes publiques"
    );

    for position in 0..scelle.len() {
        for masque in [0x01u8, 0x80, 0xFF] {
            let mut falsifie = scelle.clone();
            falsifie[position] ^= masque;
            doit_refuser(
                c.desceller(&falsifie),
                &format!("coffre, position {position}, masque {masque:#04x}"),
            );
        }
    }
}

/// L'en-tête est en clair : l'attaquant peut y écrire des longueurs absurdes.
/// Aucune ne doit produire de panique, de débordement ni d'allocation géante —
/// seulement un refus, et **vite**.
#[test]
fn en_tete_hostile_ne_panique_pas_et_se_refuse_sans_allouer() {
    let c = coffre("e1");
    let scelle = c.sceller_avec_bloc(&charge(1), BLOC).expect("scellement");
    let en_tete = &scelle[..TETE_COFFRE];

    // Les champs tels que le scellement les a produits, pour ne changer qu'un
    // champ à la fois.
    let lire_u32 = |o: usize| u32::from_le_bytes([en_tete[o], en_tete[o + 1], en_tete[o + 2], en_tete[o + 3]]);
    assert_eq!(lire_u32(6), BLOC);
    assert_eq!(lire_u32(10), 1);

    let mut cas: Vec<(&str, Vec<u8>)> = Vec::new();

    let ecrire_u32 = |base: &[u8], o: usize, v: u32| {
        let mut t = base.to_vec();
        t[o..o + 4].copy_from_slice(&v.to_le_bytes());
        t
    };
    let ecrire_u64 = |base: &[u8], o: usize, v: u64| {
        let mut t = base.to_vec();
        t[o..o + 8].copy_from_slice(&v.to_le_bytes());
        t
    };

    // Taille de bloc : zéro est le cas qui divise par zéro si le contrôle des
    // bornes manque ; les autres sont des débordements de calcul.
    for (nom, valeur) in [
        ("taille_bloc = 0", 0u32),
        ("taille_bloc = 1", 1),
        ("taille_bloc = BLOC_MINIMUM - 1", BLOC_MINIMUM - 1),
        ("taille_bloc = BLOC_MAXIMUM + 1", BLOC_MAXIMUM + 1),
        ("taille_bloc = u32::MAX", u32::MAX),
    ] {
        cas.push((nom, ecrire_u32(&scelle, 6, valeur)));
    }

    // Nombre de blocs : l'absurde, et l'incohérent avec la longueur.
    for (nom, valeur) in [
        ("nb_blocs = 0", 0u32),
        ("nb_blocs = u32::MAX", u32::MAX),
        ("nb_blocs = 2 (un seul scellé)", 2),
    ] {
        cas.push((nom, ecrire_u32(&scelle, 10, valeur)));
    }

    // Longueur : le très grand, celui qui ferait allouer un téraoctet.
    for (nom, valeur) in [
        ("longueur = 2^40", 1u64 << 40),
        ("longueur = 2^63", 1u64 << 63),
        ("longueur = u64::MAX", u64::MAX),
        ("longueur = 0 avec un bloc", 0),
        ("longueur = BLOC + 1 avec un bloc", u64::from(BLOC) + 1),
    ] {
        cas.push((nom, ecrire_u64(&scelle, 14, valeur)));
    }

    // Le drapeau : tout octet non nul doit être refusé, sinon un drapeau
    // inconnu pourrait modifier la lecture sans être vu.
    for valeur in [1u8, 2, 0xFF] {
        let mut t = scelle.clone();
        t[5] = valeur;
        cas.push(("drapeau inconnu", t));
    }

    for (nom, octets) in cas {
        doit_refuser(c.desceller(&octets), nom);
        // Et par le chemin qui exige un identifiant, qui lit le même en-tête.
        let id = Coffre::identifiant_lu(&scelle).expect("identifiant lisible");
        doit_refuser(c.desceller_avec_id(&id, &octets), nom);
    }
}

/// La troncature, à **chaque** longueur possible. Ce n'est pas « un octet en
/// moins » qui est testé, c'est l'absence de toute longueur qui passerait pour
/// un objet complet.
#[test]
fn troncature_a_chaque_longueur_refusee() {
    let c = coffre("e1");
    let scelle = c.sceller_avec_bloc(&charge(2), BLOC).expect("scellement");

    for longueur in 0..scelle.len() {
        doit_refuser(
            c.desceller(&scelle[..longueur]),
            &format!("troncature à {longueur} octets"),
        );
    }
}

/// L'ajout d'octets est refusé aussi : sinon un mandataire pourrait coller deux
/// objets et espérer que le premier soit lu.
#[test]
fn ajout_d_octets_refuse() {
    let c = coffre("e1");
    let scelle = c.sceller_avec_bloc(&charge(1), BLOC).expect("scellement");

    // Ajouter zéro octet n'est pas une attaque : le scellé est identique. Le
    // test ne commence donc qu'à un octet de plus.
    for ajout in [&b"\x00"[..], &[0u8; 28][..], &[0u8; 1052][..]] {
        let mut etendu = scelle.clone();
        etendu.extend_from_slice(ajout);
        doit_refuser(c.desceller(&etendu), "ajout d'octets");
    }
}

/// Réordonner les blocs d'un même coffre : l'index entre dans l'AAD, donc le
/// contenu doit changer de place sans que rien ne le détecte — et c'est
/// exactement ce qu'il ne doit pas faire.
#[test]
fn reordonnancement_des_blocs_refuse() {
    let c = coffre("e1");
    let clair = charge_blocs(3);
    let scelle = c.sceller_avec_bloc(&clair, BLOC).expect("scellement");

    let debut = TETE_COFFRE + SCEAU_ENTETE;
    let taille_bloc_scelle = LONGUEUR_NONCE + BLOC as usize + 16;
    assert_eq!(debut + 3 * taille_bloc_scelle, scelle.len());

    let bloc = |i: usize| -> Vec<u8> {
        scelle[debut + i * taille_bloc_scelle..debut + (i + 1) * taille_bloc_scelle].to_vec()
    };

    for (a, b) in [(0usize, 1usize), (1, 2), (0, 2)] {
        let mut permute = scelle.clone();
        permute[debut + a * taille_bloc_scelle..debut + (a + 1) * taille_bloc_scelle]
            .copy_from_slice(&bloc(b));
        permute[debut + b * taille_bloc_scelle..debut + (b + 1) * taille_bloc_scelle]
            .copy_from_slice(&bloc(a));
        doit_refuser(c.desceller(&permute), &format!("blocs {a} et {b} échangés"));
    }
}

/// La greffe : prendre un bloc **d'un autre coffre** du même espace et de la
/// même clé, et le glisser dans celui-ci. Sans l'identifiant de coffre dans
/// l'AAD, cette attaque passait.
#[test]
fn greffe_d_un_bloc_entre_deux_coffres_refusee() {
    let c = coffre("e1");
    let a = c.sceller_avec_bloc(&charge_blocs(2), BLOC).expect("scellement A");
    let b = c.sceller_avec_bloc(&charge_blocs(2), BLOC).expect("scellement B");

    // Les deux coffres ont la même forme : la greffe est donc invisible à
    // quiconque ne regarde que les longueurs.
    assert_eq!(a.len(), b.len());

    let debut = TETE_COFFRE + SCEAU_ENTETE;
    let taille_bloc_scelle = LONGUEUR_NONCE + BLOC as usize + 16;

    // Dernier bloc de B dans A.
    let mut greffe = a.clone();
    greffe[debut + taille_bloc_scelle..debut + 2 * taille_bloc_scelle]
        .copy_from_slice(&b[debut + taille_bloc_scelle..debut + 2 * taille_bloc_scelle]);
    doit_refuser(c.desceller(&greffe), "dernier bloc greffé");

    // Premier bloc de B dans A.
    let mut greffe = a.clone();
    greffe[debut..debut + taille_bloc_scelle]
        .copy_from_slice(&b[debut..debut + taille_bloc_scelle]);
    doit_refuser(c.desceller(&greffe), "premier bloc greffé");

    // En-têtes échangés : le sceau d'en-tête de A ne couvre pas l'en-tête de B.
    let mut greffe = a.clone();
    greffe[..TETE_COFFRE].copy_from_slice(&b[..TETE_COFFRE]);
    doit_refuser(c.desceller(&greffe), "en-tête greffé");
}

/// **Le trou que la crate nomme elle-même, vérifié.** Un coffre entier venu
/// d'ailleurs est parfaitement valide : `desceller` ne peut pas le voir. Seul
/// `desceller_avec_id`, qui exige un identifiant, le voit.
///
/// Ce test existe pour que la limite soit **mesurée** et non promise : si un
/// jour `desceller` se mettait à refuser, ce test tomberait, et l'appelant
/// saurait que sa documentation a changé.
#[test]
fn substitution_d_un_coffre_entier_n_est_vue_que_par_desceller_avec_id() {
    let c = coffre("e1");
    let legitime = c.sceller_avec_bloc(&charge(1), BLOC).expect("scellement");
    let substitue = c.sceller_avec_bloc(&charge(1), BLOC).expect("scellement");

    let id_attendu = Coffre::identifiant_lu(&legitime).expect("identifiant");

    // `desceller` accepte le substitut : c'est documenté, et c'est mesuré ici.
    let rendu = c
        .desceller(&substitue)
        .expect("un coffre valide est accepté par desceller — c'est la limite documentée");
    assert_eq!(rendu, charge(1));

    // `desceller_avec_id` le refuse, et nomme l'identifiant attendu et lu.
    match c.desceller_avec_id(&id_attendu, &substitue) {
        Err(CoffreError::CoffreInconnu { attendu, lu }) => {
            assert_ne!(attendu, lu, "les deux identifiants doivent différer");
        }
        autre => panic!("attendu CoffreInconnu sur substitution, obtenu {autre:?}"),
    }

    // Et l'identifiant lu dans le substitut n'est pas celui du légitime.
    assert_ne!(
        Coffre::identifiant_lu(&substitue).expect("identifiant"),
        id_attendu
    );
}

/// La clé d'un espace n'ouvre pas les données d'un autre — et le refus est
/// **nommé**, parce que l'en-tête est en clair et que le dire ne révèle rien.
///
/// C'est le test qui compte pour l'engine : il peut détenir une clé légitime et
/// néanmoins ne rien pouvoir lire de l'espace voisin.
#[test]
fn cle_d_un_autre_espace_n_ouvre_pas_et_le_dit() {
    let c1 = coffre("espace-un");
    let c2 = coffre("espace-deux");
    let scelle = c1.sceller_avec_bloc(&charge(1), BLOC).expect("scellement");

    match c2.desceller(&scelle) {
        Err(CoffreError::EspaceIncoherent { attendu, scelle }) => {
            assert_ne!(attendu, scelle, "les deux espaces doivent différer");
        }
        autre => panic!("attendu EspaceIncoherent, obtenu {autre:?}"),
    }
}

/// Aucun usage ne tient lieu d'un autre. Un scellé d'enveloppe a exactement la
/// forme d'un bloc de coffre quand la capacité de la classe égale la taille de
/// bloc : les deux font `nonce + clair + étiquette`. S'ils partageaient leur
/// clé, l'échange passerait.
#[test]
fn les_usages_ne_se_confondent_pas() {
    let c = coffre("e1");
    let jeton = Jeton::neuf(&cle("e1")).expect("jeton");
    let enveloppe = Enveloppe::neuf(&cle("e1")).expect("enveloppe");

    let scelle = c.sceller_avec_bloc(&charge_blocs(1), BLOC).expect("scellement");
    let jeton_octets = jeton.emettre("graph:read", u64::MAX).expect("émission");
    let env_octets = enveloppe
        .sceller_dans(
            &vec![7u8; ClasseTaille::Octets1024.capacite_utile()],
            ClasseTaille::Octets1024,
        )
        .expect("scellement d'enveloppe");

    // Un jeton n'est pas un coffre, ni une enveloppe, ni l'inverse.
    doit_refuser(c.desceller(&jeton_octets), "jeton présenté comme coffre");
    doit_refuser(c.desceller(&env_octets), "enveloppe présentée comme coffre");
    doit_refuser(enveloppe.desceller(&scelle), "coffre présenté comme enveloppe");
    doit_refuser(enveloppe.desceller(&jeton_octets), "jeton présenté comme enveloppe");
    doit_refuser(jeton.lire(&scelle), "coffre présenté comme jeton");
    doit_refuser(jeton.lire(&env_octets), "enveloppe présentée comme jeton");

    // La substitution de forme : le corps d'une enveloppe (nonce + chiffré +
    // étiquette = 1052 octets en classe 1024) prend exactement la place d'un
    // bloc de coffre (12 + 1024 + 16 = 1052). Même longueur, même clé d'espace,
    // sous-clés distinctes : c'est le seul endroit où la séparation de domaines
    // est observable, et c'est ici qu'on la mesure.
    let debut_bloc = TETE_COFFRE + SCEAU_ENTETE;
    let debut_env = TETE_ENVELOPPE;
    let corps = LONGUEUR_NONCE + BLOC as usize + 16;
    assert_eq!(
        scelle.len() - debut_bloc,
        corps,
        "un seul bloc : la substitution de forme est possible"
    );
    let mut echanges = scelle.clone();
    echanges[debut_bloc..debut_bloc + corps]
        .copy_from_slice(&env_octets[debut_env..debut_env + corps]);
    doit_refuser(c.desceller(&echanges), "corps d'enveloppe en place de bloc");
}

/// Aucun secret ne doit se retrouver dans ce qui circule : ni la charge utile,
/// ni la phrase qui ouvre l'enveloppe de clé.
#[test]
fn aucun_secret_dans_les_octets_scelles() {
    let phrase = "phrase-de-test-qui-ne-doit-jamais-circuler";
    let c = coffre("e1");
    let clair = charge(2);

    let scelle = c.sceller_avec_bloc(&clair, BLOC).expect("scellement");
    let enveloppe = cle("e1")
        .envelopper_avec_phrase(phrase, obrain_coffre::ParametresKdf::PLANCHER)
        .expect("enveloppe");
    let jeton = Jeton::neuf(&cle("e1"))
        .expect("jeton")
        .emettre("iam:admin", u64::MAX)
        .expect("émission");

    assert!(
        !contient(&scelle, SECRET_DU_CALCUL),
        "la charge utile apparaît dans le coffre scellé"
    );
    for (nom, octets) in [
        ("coffre", scelle.as_slice()),
        ("enveloppe de clé", enveloppe.octets().as_slice()),
        ("jeton", jeton.as_slice()),
    ] {
        assert!(
            !contient(octets, phrase.as_bytes()),
            "la phrase apparaît dans : {nom}"
        );
        assert!(
            !contient(octets, SECRET_DU_CALCUL),
            "la charge utile apparaît dans : {nom}"
        );
    }

    // Le nom de l'action ne traverse pas le réseau non plus : il est dans le
    // chiffré, pas dans l'en-tête.
    assert!(
        !contient(&jeton, b"iam:admin"),
        "le nom de l'action apparaît dans le jeton"
    );
}

/// **Le mode de défaillance le plus grave de tout AES-GCM : la réutilisation de
/// nonce.** Deux messages chiffrés sous la même clé avec le même nonce livrent
/// le XOR des clairs, et permettent de recalculer la clé d'authentification.
///
/// Le test scelle le même contenu plusieurs centaines de fois sous la même clé
/// et vérifie qu'aucun nonce ne revient, **séparément par domaine de clé** : un
/// nonce d'en-tête et un nonce de bloc ne partagent pas de clé, les confondre
/// n'apprendrait rien ; deux nonces de bloc, si.
#[test]
fn nonces_jamais_repetes_dans_un_meme_domaine() {
    let c = coffre("e1");
    let clair = charge_blocs(2);
    let taille_bloc_scelle = LONGUEUR_NONCE + BLOC as usize + 16;
    let debut_blocs = TETE_COFFRE + SCEAU_ENTETE;

    let mut nonces_entete: HashSet<[u8; LONGUEUR_NONCE]> = HashSet::new();
    let mut nonces_bloc0: HashSet<[u8; LONGUEUR_NONCE]> = HashSet::new();
    let mut nonces_bloc1: HashSet<[u8; LONGUEUR_NONCE]> = HashSet::new();

    const TOURS: usize = 300;
    for _ in 0..TOURS {
        let scelle = c.sceller_avec_bloc(&clair, BLOC).expect("scellement");

        let lire = |o: usize| -> [u8; LONGUEUR_NONCE] {
            scelle[o..o + LONGUEUR_NONCE].try_into().expect("nonce")
        };
        assert!(
            nonces_entete.insert(lire(TETE_COFFRE)),
            "nonce d'en-tête répété sous la même clé"
        );
        assert!(
            nonces_bloc0.insert(lire(debut_blocs)),
            "nonce du bloc 0 répété sous la même clé"
        );
        assert!(
            nonces_bloc1.insert(lire(debut_blocs + taille_bloc_scelle)),
            "nonce du bloc 1 répété sous la même clé"
        );
    }

    assert_eq!(nonces_entete.len(), TOURS);
    assert_eq!(nonces_bloc0.len(), TOURS);
    assert_eq!(nonces_bloc1.len(), TOURS);
}

// ---------------------------------------------------------------------------
// La même chose, sur l'enveloppe à taille fixe
// ---------------------------------------------------------------------------

/// Le balayage exhaustif de l'enveloppe. Elle est plus courte, donc le balayage
/// est complet sans réserve.
#[test]
fn enveloppe_aucun_octet_n_echappe_a_l_authentification() {
    let enveloppe = Enveloppe::neuf(&cle("e1")).expect("enveloppe");
    let charge = charge(2);
    let scelle = enveloppe
        .sceller_dans(&charge, ClasseTaille::Octets256)
        .expect("scellement");

    for position in 0..scelle.len() {
        for masque in [0x01u8, 0x80, 0xFF] {
            let mut falsifie = scelle.clone();
            falsifie[position] ^= masque;
            doit_refuser(
                enveloppe.desceller(&falsifie),
                &format!("enveloppe, position {position}, masque {masque:#04x}"),
            );
        }
    }
}

/// **G3, la partie qu'on peut tenir :** deux charges utiles de tailles
/// différentes, dans la même classe, sont indiscernables à la longueur près —
/// c'est-à-dire qu'elles ne le sont pas du tout.
///
/// Le test dit aussi ce qui reste visible : la classe. Il ne prétend pas que
/// l'enveloppe cache la taille ; il mesure ce qu'elle cache.
#[test]
fn enveloppe_deux_charges_differentes_ont_la_meme_taille() {
    let enveloppe = Enveloppe::neuf(&cle("e1")).expect("enveloppe");

    let petite = enveloppe
        .sceller_dans(b"lire", ClasseTaille::Octets256)
        .expect("scellement");
    let grosse = enveloppe
        .sceller_dans(&vec![0xAB; 240], ClasseTaille::Octets256)
        .expect("scellement");

    assert_eq!(
        petite.len(),
        grosse.len(),
        "deux charges de la même classe doivent avoir la même longueur"
    );
    assert_eq!(
        petite[5],
        grosse[5],
        "la classe doit être la même (indice en clair, position 5)"
    );
    assert_ne!(petite, grosse, "les octets, eux, doivent différer");

    // Et le déscellement rend bien chaque charge, sans confondre le remplissage
    // avec la charge : une enveloppe dont le remplissage serait relu comme du
    // clair gonflerait les charges.
    assert_eq!(enveloppe.desceller(&petite).expect("déscellement"), b"lire");
    assert_eq!(
        enveloppe.desceller(&grosse).expect("déscellement"),
        vec![0xAB; 240]
    );

    // Toutes les classes annoncent une longueur totale qui ne dépend que d'elles.
    for classe in ClasseTaille::TOUTES {
        let scelle = enveloppe
            .sceller_dans(&vec![0u8; classe.capacite_utile()], classe)
            .expect("scellement");
        assert_eq!(scelle.len(), classe.taille_totale());
        assert_eq!(scelle[5], classe.indice());
        assert_eq!(
            enveloppe.desceller(&scelle).expect("déscellement").len(),
            classe.capacite_utile()
        );
    }
}

/// Abaisser la classe déclarée, c'est essayer de faire croire qu'une grosse
/// charge était petite — et de faire lire au client autre chose que ce qui a été
/// scellé. La classe est dans l'AAD : la modifier casse l'authentification.
#[test]
fn enveloppe_classe_abaissée_ou_inconnue_refusee() {
    let enveloppe = Enveloppe::neuf(&cle("e1")).expect("enveloppe");
    let scelle = enveloppe
        .sceller_dans(
            &vec![0x5A; ClasseTaille::Octets4096.capacite_utile()],
            ClasseTaille::Octets4096,
        )
        .expect("scellement");

    assert_eq!(scelle[5], ClasseTaille::Octets4096.indice());

    // Vers une classe plus petite, une classe plus grande, et un indice inconnu.
    // L'indice de classe est en clair à la position 5 : c'est ce que l'engine,
    // qui n'a pas la clé, peut lire — et donc ce qu'un attaquant peut réécrire.
    for (nom, indice) in [
        ("classe abaissée", ClasseTaille::Octets256.indice()),
        ("classe montée", ClasseTaille::Octets65536.indice()),
        ("indice inconnu", 200u8),
    ] {
        let mut falsifie = scelle.clone();
        falsifie[5] = indice;
        doit_refuser(enveloppe.desceller(&falsifie), nom);
    }

    // Une charge plus grande que la plus grande classe est refusée, jamais
    // tronquée — une charge tronquée ferait exécuter un autre calcul.
    let trop = vec![0u8; ClasseTaille::maximum() + 1];
    doit_refuser(enveloppe.sceller(&trop), "charge au-delà de la dernière classe");
}

// ---------------------------------------------------------------------------
// Le jeton de capacité
// ---------------------------------------------------------------------------

#[test]
fn jeton_aucun_octet_n_echappe_a_l_authentification() {
    let jeton = Jeton::neuf(&cle("e1")).expect("jeton");
    let emis = jeton.emettre("graph:read", 2_000_000_000).expect("émission");
    assert_eq!(emis.len(), TETE_JETON + LONGUEUR_NONCE + 64 + 16);

    for position in 0..emis.len() {
        for masque in [0x01u8, 0x80, 0xFF] {
            let mut falsifie = emis.clone();
            falsifie[position] ^= masque;
            doit_refuser(
                jeton.verifier(&falsifie, "graph:read", 1_000_000_000),
                &format!("jeton, position {position}, masque {masque:#04x}"),
            );
            doit_refuser(
                jeton.lire(&falsifie),
                &format!("jeton (lire), position {position}, masque {masque:#04x}"),
            );
        }
    }
}

/// L'échéance : `verifier` et `valide_a` doivent **s'accorder**, quelle que soit
/// la convention retenue à la seconde exacte. Le test ne présume pas de la
/// convention — il exige la cohérence, parce que deux jugements différents sur
/// la même échéance sont une faille, pas un détail.
#[test]
fn jeton_echeance_et_verifier_s_accordent() {
    let jeton = Jeton::neuf(&cle("e1")).expect("jeton");
    let echeance = 1_000_000_000u64;
    let emis = jeton.emettre("graph:read", echeance).expect("émission");

    let portee = jeton.lire(&emis).expect("lire");
    assert_eq!(portee.expiration, echeance);
    assert_eq!(portee.action, "graph:read");

    for maintenant in [
        echeance - 2,
        echeance - 1,
        echeance,
        echeance + 1,
        echeance + 2,
    ] {
        let juge_valide = portee.valide_a(maintenant);
        let verdict = jeton.verifier(&emis, "graph:read", maintenant);
        assert_eq!(
            juge_valide,
            verdict.is_ok(),
            "verifier et valide_a ne s'accordent pas à l'instant {maintenant}"
        );
    }

    // Et le bon sens, pour que l'accord ne soit pas un accord sur du faux.
    assert!(jeton.verifier(&emis, "graph:read", echeance - 1).is_ok());
    doit_refuser(jeton.verifier(&emis, "graph:read", echeance + 1), "échéance passée");
}

#[test]
fn jeton_d_un_autre_espace_ou_d_une_autre_action_refuse() {
    let jeton_un = Jeton::neuf(&cle("espace-un")).expect("jeton");
    let jeton_deux = Jeton::neuf(&cle("espace-deux")).expect("jeton");
    let emis = jeton_un
        .emettre("graph:read", 2_000_000_000)
        .expect("émission");

    // Rejouer le jeton sur l'autre espace : refusé, et nommé.
    match jeton_deux.verifier(&emis, "graph:read", 1_000_000_000) {
        Err(CoffreError::EspaceIncoherent { .. }) => {}
        autre => panic!("attendu EspaceIncoherent, obtenu {autre:?}"),
    }

    // Un `Allow` sur une action n'est pas un `Allow` sur une autre.
    match jeton_un.verifier(&emis, "iam:admin", 1_000_000_000) {
        Err(CoffreError::ActionHorsPortee { portee, demandee }) => {
            assert_eq!(portee, "graph:read");
            assert_eq!(demandee, "iam:admin");
        }
        autre => panic!("attendu ActionHorsPortee, obtenu {autre:?}"),
    }

    // Réécrire l'action « en clair » est impossible : elle ne s'y trouve pas.
    // C'est la même propriété que `aucun_secret_dans_les_octets_scelles`, mais
    // mesurée ici sur l'objet qui en dépend : si l'action était lisible, on
    // pourrait la remplacer sans toucher au chiffré.
    assert!(
        !contient(&emis, b"graph:read"),
        "l'action demandée ne doit pas être lisible dans le jeton"
    );

    // Et une altération quelconque du jeton est refusée — de façon
    // déterministe, sur le dernier octet (l'étiquette), où il n'y a rien à
    // deviner.
    let mut falsifie = emis.clone();
    let dernier = falsifie.len() - 1;
    falsifie[dernier] ^= 0x01;
    doit_refuser(
        jeton_un.verifier(&falsifie, "graph:read", 1_000_000_000),
        "étiquette du jeton altérée",
    );

    // Un jeton émis sous une autre clé du même nom d'espace ne vérifie pas.
    let autre_cle_meme_espace = Jeton::neuf(&cle("espace-un")).expect("jeton");
    doit_refuser(
        autre_cle_meme_espace.verifier(&emis, "graph:read", 1_000_000_000),
        "jeton émis sous une autre clé",
    );
}

// ---------------------------------------------------------------------------
// I2 — les octets hostiles ne font pas tomber le processus
// ---------------------------------------------------------------------------

/// Des octets au hasard, de toutes les longueurs, plus des octets hostiles
/// *structurés* : magic valide, version valide, champs extrêmes. Aucun ne doit
/// paniquer, aucun ne doit rendre du clair.
///
/// Un panique ici n'est pas un détail de style : c'est un déni de service
/// déclenchable par un mandataire réseau, avec 70 octets.
#[test]
fn octets_au_hasard_et_entetes_forges_ne_paniquent_jamais() {
    let c = coffre("e1");
    let enveloppe = Enveloppe::neuf(&cle("e1")).expect("enveloppe");
    let jeton = Jeton::neuf(&cle("e1")).expect("jeton");

    let mut hasard = Hasard::neuf(0x5EED_2026_0915);

    // Au hasard, pur.
    for _ in 0..400 {
        let n = (hasard.suivant() % 400) as usize;
        let octets = hasard.octets(n);
        doit_refuser(c.desceller(&octets), "hasard → coffre");
        doit_refuser(enveloppe.desceller(&octets), "hasard → enveloppe");
        doit_refuser(jeton.lire(&octets), "hasard → jeton");
        doit_refuser(Coffre::identifiant_lu(&octets), "hasard → identifiant");
    }

    // Structuré : magic et version valides, donc les contrôles superficiels
    // passent, et ce sont les contrôles profonds qui travaillent.
    let mut en_tete = [0u8; TETE_COFFRE];
    en_tete[0..4].copy_from_slice(b"OBCF");
    en_tete[4] = 1;
    en_tete[5] = 0;

    for (taille_bloc, nb_blocs, longueur) in [
        (0u32, 0u32, 0u64),
        (0, u32::MAX, u64::MAX),
        (u32::MAX, u32::MAX, u64::MAX),
        (BLOC, u32::MAX, u64::MAX),
        (BLOC_MAXIMUM, u32::MAX, u64::MAX),
        (BLOC, 1, u64::MAX),
        (BLOC, 1, u64::from(BLOC) * u64::from(u32::MAX)),
        (1, 1, 1),
    ] {
        en_tete[6..10].copy_from_slice(&taille_bloc.to_le_bytes());
        en_tete[10..14].copy_from_slice(&nb_blocs.to_le_bytes());
        en_tete[14..22].copy_from_slice(&longueur.to_le_bytes());

        // Avec un corps au hasard, et avec un corps vide.
        for corps in [Vec::new(), hasard.octets(200), hasard.octets(5000)] {
            let mut octets = en_tete.to_vec();
            octets.extend_from_slice(&corps);
            doit_refuser(
                c.desceller(&octets),
                &format!("en-tête forgé bloc={taille_bloc} n={nb_blocs} l={longueur}"),
            );
            let id = [0u8; 16];
            doit_refuser(
                c.desceller_avec_id(&id, &octets),
                &format!("en-tête forgé + id, bloc={taille_bloc} n={nb_blocs} l={longueur}"),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Les propriétés de forme
// ---------------------------------------------------------------------------

/// La longueur scellée ne dépend que de la longueur claire et de la taille de
/// bloc : si elle dépendait du **contenu**, le contenu fuiterait par la taille.
///
/// Ce test vérifie les deux moitiés : la forme est déterministe, et deux
/// contenus de même longueur donnent la même longueur scellée.
#[test]
fn longueur_scellee_ne_depend_que_de_la_longueur_claire() {
    let c = coffre("e1");
    let bloc = BLOC as usize;

    for longueur in [0usize, 1, 2, bloc - 1, bloc, bloc + 1, 2 * bloc, 2 * bloc + 1] {
        let attendu = longueur_scellee(longueur, bloc);

        // Deux contenus différents de même longueur : même forme.
        let a = vec![0x00u8; longueur];
        let b = vec![0xFFu8; longueur];
        let scelle_a = c.sceller_avec_bloc(&a, BLOC).expect("scellement");
        let scelle_b = c.sceller_avec_bloc(&b, BLOC).expect("scellement");

        assert_eq!(scelle_a.len(), attendu, "longueur {longueur}");
        assert_eq!(scelle_b.len(), attendu, "longueur {longueur}");
        assert_ne!(scelle_a, scelle_b, "deux contenus différents, mêmes octets ?");

        // Et l'aller-retour.
        assert_eq!(c.desceller(&scelle_a).expect("déscellement"), a);
        assert_eq!(c.desceller(&scelle_b).expect("déscellement"), b);
    }

    // La charge vide fait un aller-retour, et reste refusable par altération.
    let vide = c.sceller_avec_bloc(&[], BLOC).expect("scellement vide");
    assert_eq!(vide.len(), FIXE);
    assert_eq!(c.desceller(&vide).expect("déscellement"), Vec::<u8>::new());
    let mut falsifie = vide.clone();
    falsifie[0] ^= 0x01;
    doit_refuser(c.desceller(&falsifie), "coffre vide altéré");
}

/// **CONSTAT, et il contredit la documentation du module.**
///
/// `coffre.rs` annonce que ce qui reste lisible est « la classe de longueur
/// (nombre de blocs), donc la longueur arrondie ». Or la queue du dernier bloc
/// n'est **pas** bourrée : l'étiquette d'un bloc fait `longueur du clair + 16`.
/// Un observateur qui connaît la taille de bloc — elle est **en clair dans
/// l'en-tête** — retrouve donc la longueur *exacte* du clair, pas sa valeur
/// arrondie.
///
/// Ce n'est pas une faille de confidentialité du contenu : rien du clair ne
/// fuit. C'est une fuite de **métadonnée** (G3), et elle est mesurable :
/// « le calcul a rendu 1 337 octets » n'est pas la même information que « le
/// calcul tient dans un bloc de 1 Kio ».
///
/// Le test fixe la mesure pour que la documentation ne puisse pas l'oublier :
/// si un jour la queue est bourrée, ce test tombera, et ce sera un progrès à
/// consigner — pas une régression.
#[test]
fn constat_la_longueur_exacte_du_clair_est_lisible_sur_le_scelle() {
    let c = coffre("e1");
    let bloc = BLOC as usize;

    for longueur in [1usize, 17, 512, bloc - 1, bloc, bloc + 1, 3 * bloc - 7] {
        let scelle = c
            .sceller_avec_bloc(&vec![0u8; longueur], BLOC)
            .expect("scellement");
        let blocs = longueur.div_ceil(bloc);
        let retrouvee = scelle.len() - FIXE - blocs * PAR_BLOC;

        assert_eq!(
            retrouvee, longueur,
            "la longueur exacte du clair doit être retrouvable depuis la seule longueur du scellé"
        );
    }
}

/// Le scellement refuse une taille de bloc hors bornes : une API qui
/// l'accepterait permettrait de fabriquer des objets que personne ne relit.
#[test]
fn taille_de_bloc_hors_bornes_refusee_au_scellement() {
    let c = coffre("e1");
    for taille in [0u32, 1, BLOC_MINIMUM - 1, BLOC_MAXIMUM + 1, u32::MAX] {
        doit_refuser(
            c.sceller_avec_bloc(b"x", taille),
            &format!("scellement avec bloc {taille}"),
        );
    }
    // Aux bornes, ça passe.
    assert!(c.sceller_avec_bloc(b"x", BLOC_MINIMUM).is_ok());
    assert!(c.sceller_avec_bloc(b"x", BLOC_MAXIMUM).is_ok());
}

// ---------------------------------------------------------------------------
// L'enveloppe de clé et le dépôt — le maillon dont tout dépend
// ---------------------------------------------------------------------------
//
// Le coffre ne vaut que ce que vaut la clé qui l'ouvre, et la clé ne vaut que ce
// que vaut l'enveloppe où elle dort. Attaquer le coffre en laissant l'enveloppe
// de côté reviendrait à vérifier la serrure en laissant la clé sous le paillasson.

const PHRASE: &str = "phrase de l'enveloppe adverse";

/// Le balayage exhaustif de l'enveloppe de clé — 128 octets, toutes les
/// positions.
///
/// Un octet de cette enveloppe qui ne serait pas couvert permettrait de changer
/// **quelle clé** est descellée : ce n'est plus une fuite, c'est une
/// substitution de clé.
#[test]
fn enveloppe_de_cle_aucun_octet_n_echappe_a_l_authentification() {
    let enveloppe = cle("e1")
        .envelopper_avec_phrase(PHRASE, ParametresKdf::PLANCHER)
        .expect("enveloppe");
    let octets = *enveloppe.octets();
    let kek = enveloppe.deriver_kek(PHRASE).expect("dérivation");

    for position in 0..octets.len() {
        for masque in [0x01u8, 0x80, 0xFF] {
            let mut falsifie = octets;
            falsifie[position] ^= masque;
            // Un refus à la relecture du format est un refus valable : ce qui
            // compte est qu'aucune clé ne sorte.
            match EnveloppeCle::depuis_octets(&falsifie) {
                Err(_) => {}
                Ok(falsifiee) => doit_refuser(
                    CleEspace::desceller(&falsifiee, &kek),
                    &format!("enveloppe de clé, position {position}, masque {masque:#04x}"),
                ),
            }
        }
    }
}

/// **L'abaissement des paramètres**, sur l'objet qui décide de la force de la
/// clé. Le refus doit avoir lieu **avant** toute dérivation : un serveur qui
/// dériverait d'abord pour authentifier ensuite offrirait un calcul à 8 Kio à
/// qui sait réécrire un en-tête.
#[test]
fn abaissement_des_parametres_de_cle_refuse_avant_derivation() {
    let enveloppe = cle("e1")
        .envelopper_avec_phrase(PHRASE, ParametresKdf::default())
        .expect("enveloppe");

    // Sous le plancher : refusé par la dérivation elle-même, sans rien calculer.
    for (nom, m_cost) in [("m = 0", 0u32), ("m = 8 Kio", 8), ("m = 1024 Kio", 1024)] {
        let mut octets = *enveloppe.octets();
        octets[8..12].copy_from_slice(&m_cost.to_le_bytes());
        let abaissee = EnveloppeCle::depuis_octets(&octets).expect("format");
        match abaissee.deriver_kek(PHRASE) {
            Err(CoffreError::KdfTropFaible(_)) => {}
            autre => panic!("{nom} : attendu KdfTropFaible, obtenu {autre:?}"),
        }
    }

    // Au-dessus du plancher : les paramètres sont dans l'AAD, donc les modifier
    // casse le sceau — et l'échec est nommé comme tel.
    let mut octets = *enveloppe.octets();
    octets[12..16].copy_from_slice(&3u32.to_le_bytes()); // t_cost 2 → 3
    let modifiee = EnveloppeCle::depuis_octets(&octets).expect("format");
    let kek = modifiee.deriver_kek(PHRASE).expect("dérivation");
    assert!(matches!(
        CleEspace::desceller(&modifiee, &kek),
        Err(CoffreError::CleMaitresseIncorrecte)
    ));

    // Et l'on ne peut pas fabriquer soi-même une enveloppe faible.
    assert!(matches!(
        cle("e1").envelopper_avec_phrase(
            PHRASE,
            ParametresKdf {
                m_cost_kio: 1024,
                t_cost: 1,
                p_cost: 1,
            }
        ),
        Err(CoffreError::KdfTropFaible(_))
    ));
}

/// La chaîne complète : phrase → enveloppe → clé → coffre. C'est le chemin que
/// l'utilisateur emprunte réellement, et c'est donc celui où une substitution
/// silencieuse serait possible.
#[test]
fn la_chaine_phrase_enveloppe_cle_coffre_ne_se_substitue_pas() {
    let cle_alpha = cle("espaces/alpha");
    let enveloppe = cle_alpha
        .envelopper_avec_phrase(PHRASE, ParametresKdf::PLANCHER)
        .expect("enveloppe");

    // La phrase redonne la même clé : ce qu'un coffre scellé par l'original se
    // relit par la clé descellée.
    let kek = enveloppe.deriver_kek(PHRASE).expect("dérivation");
    let rendue = CleEspace::desceller(&enveloppe, &kek).expect("déscellement");
    assert_eq!(rendue.espace(), cle_alpha.espace());

    let clair = charge_blocs(1);
    let scelle = Coffre::neuf(&cle_alpha)
        .expect("coffre")
        .sceller_avec_bloc(&clair, BLOC)
        .expect("scellement");
    assert_eq!(
        Coffre::neuf(&rendue)
            .expect("coffre")
            .desceller(&scelle)
            .expect("déscellement"),
        clair
    );

    // Une autre phrase n'ouvre pas l'enveloppe.
    let autre = enveloppe.deriver_kek("une autre phrase").expect("dérivation");
    assert!(matches!(
        CleEspace::desceller(&enveloppe, &autre),
        Err(CoffreError::CleMaitresseIncorrecte)
    ));

    // Une clé légitime d'un AUTRE espace — descellée par la même phrase, donc
    // parfaitement authentique — n'ouvre pas le coffre d'alpha. C'est la
    // garantie qui compte pour l'engine : détenir une clé ne donne pas les
    // espaces voisins.
    let enveloppe_beta = cle("espaces/beta")
        .envelopper_avec_phrase(PHRASE, ParametresKdf::PLANCHER)
        .expect("enveloppe");
    let kek_beta = enveloppe_beta.deriver_kek(PHRASE).expect("dérivation");
    let cle_beta = CleEspace::desceller(&enveloppe_beta, &kek_beta).expect("déscellement");

    match Coffre::neuf(&cle_beta)
        .expect("coffre")
        .desceller(&scelle)
    {
        Err(CoffreError::EspaceIncoherent { .. }) => {}
        autre => panic!("attendu EspaceIncoherent, obtenu {autre:?}"),
    }
}

/// Le dépôt : écrit en 0600, et **refuse de servir** une clé que d'autres
/// peuvent lire. Une clé lisible par le voisin n'est pas une clé ; la servir
/// quand même donnerait à l'utilisateur l'illusion d'une garantie tenue.
#[cfg(unix)]
#[test]
fn le_depot_ecrit_en_0600_et_refuse_une_cle_trop_lisible() {
    use std::os::unix::fs::PermissionsExt;

    let dossier = tempfile::tempdir().expect("dossier temporaire");
    let chemin = dossier.path().join("cle.obccle");
    let enveloppe = cle("e1")
        .envelopper_avec_phrase(PHRASE, ParametresKdf::PLANCHER)
        .expect("enveloppe");

    ecrire_enveloppe(&chemin, &enveloppe).expect("écriture");

    // Le mode posé est 0600, même si le fichier existait déjà.
    let mode = std::fs::metadata(&chemin)
        .expect("métadonnées")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "le fichier de clé doit être privé");

    // Relu sans difficulté, et identique à ce qui a été écrit.
    let relue = lire_enveloppe(&chemin).expect("lecture");
    assert_eq!(relue.octets(), enveloppe.octets());

    // Rendu lisible au-delà du propriétaire : refusé, et le mode est dit.
    std::fs::set_permissions(&chemin, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    match lire_enveloppe(&chemin) {
        Err(CoffreError::PermissionsTropLarges { mode, .. }) => {
            assert_eq!(mode & 0o077, 0o044, "le mode fautif doit être rapporté");
        }
        autre => panic!("attendu PermissionsTropLarges, obtenu {autre:?}"),
    }
}

/// **Une clé absente est une impossibilité, pas un refus.** C'est la distinction
/// qui porte toute la garantie : s'il n'y a pas de clé, il n'y a rien à
/// déchiffrer, et aucun droit, aucun administrateur, aucune politique ne le
/// change. Le test la vérifie sur le seul chemin où elle est observable.
#[test]
fn une_cle_absente_du_depot_est_une_impossibilite() {
    let dossier = tempfile::tempdir().expect("dossier temporaire");
    let chemin = dossier.path().join("jamais-ecrite.obccle");

    match lire_enveloppe(&chemin) {
        Err(e) => {
            assert!(
                matches!(e, CoffreError::CleAbsente(_)),
                "attendu CleAbsente, obtenu {e:?}"
            );
            assert!(
                e.est_une_impossibilite(),
                "une clé absente doit être classée impossibilité, pas refus"
            );
        }
        Ok(_) => panic!("un fichier absent ne doit jamais rendre de clé"),
    }
}

/// Le dépôt refuse un lien symbolique : la clé lue serait choisie par un tiers,
/// et l'utilisateur croirait lire la sienne.
#[cfg(unix)]
#[test]
fn le_depot_refuse_un_lien_symbolique() {
    let dossier = tempfile::tempdir().expect("dossier temporaire");
    let vraie = dossier.path().join("vraie.obccle");
    let lien = dossier.path().join("lien.obccle");

    let enveloppe = cle("e1")
        .envelopper_avec_phrase(PHRASE, ParametresKdf::PLANCHER)
        .expect("enveloppe");
    ecrire_enveloppe(&vraie, &enveloppe).expect("écriture");
    std::os::unix::fs::symlink(&vraie, &lien).expect("création du lien");

    match lire_enveloppe(&lien) {
        Err(CoffreError::Depot(message)) => {
            assert!(
                message.contains("lien symbolique"),
                "le motif du refus doit être nommé : {message}"
            );
        }
        autre => panic!("attendu Depot sur un lien symbolique, obtenu {autre:?}"),
    }
}

// ---------------------------------------------------------------------------
// Le balayage du domaine — et non de quelques points choisis
// ---------------------------------------------------------------------------
//
// Les tests qui précèdent fixent la taille de bloc à la plus petite et
// parcourent les longueurs autour de ses multiples. C'est un **point** du
// domaine, pas le domaine : la relation entre `taille_bloc`, `longueur` et la
// longueur scellée est une formule en forme close (`nb_blocs = ⌈L/B⌉`, puis
// `fixe + nb_blocs·(nonce+étiquette) + L`), et une formule fausse d'un octet ne
// se voit qu'à certains couples (B, L) — précisément ceux qu'on n'a pas pensé à
// écrire à la main.
//
// Ces deux tests balaient donc le domaine : au hasard sur des milliers de
// couples, puis exhaustivement sur les bornes exactes de plusieurs tailles de
// bloc, y compris la plus grande acceptée.

/// Le balayage aléatoire : 2 000 couples (taille de bloc, longueur), chacun
/// vérifié sur sa forme exacte, son en-tête, son aller-retour, son identifiant,
/// et une altération tirée au sort.
#[test]
fn balayage_aleatoire_du_domaine_bloc_longueur() {
    let c = coffre("e1");
    let mut hasard = Hasard::neuf(0x0C0F_2026_0915);
    let bloc_min = u64::from(BLOC_MINIMUM);
    let bloc_max = 4096u64; // au-delà, 2 000 cas coûteraient des minutes pour rien

    for cas in 0..2000u32 {
        let bloc = (bloc_min + hasard.suivant() % (bloc_max - bloc_min + 1)) as u32;
        let longueur = (hasard.suivant() % (3 * u64::from(bloc) + 1)) as usize;
        // Un motif déterministe, pour que les octets comparés ne soient pas tous
        // identiques : une erreur de décalage passerait inaperçue sur du zéro.
        let clair: Vec<u8> = (0..longueur).map(|i| (i % 251) as u8).collect();

        let contexte = format!("cas {cas} : bloc {bloc}, longueur {longueur}");

        let scelle = c
            .sceller_avec_bloc(&clair, bloc)
            .unwrap_or_else(|e| panic!("{contexte} : scellement refusé : {e}"));

        // 1. La forme, en forme close.
        assert_eq!(
            scelle.len(),
            longueur_scellee(longueur, bloc as usize),
            "{contexte} : longueur scellée inattendue"
        );

        // 2. L'en-tête annonce ce qu'il doit annoncer.
        let nb_annonce = u32::from_le_bytes([scelle[10], scelle[11], scelle[12], scelle[13]]);
        assert_eq!(
            nb_annonce as usize,
            longueur.div_ceil(bloc as usize),
            "{contexte} : nombre de blocs annoncé incohérent"
        );
        let longueur_annoncee = u64::from_le_bytes([
            scelle[14], scelle[15], scelle[16], scelle[17], scelle[18], scelle[19], scelle[20],
            scelle[21],
        ]);
        assert_eq!(
            longueur_annoncee as usize, longueur,
            "{contexte} : longueur annoncée dans l'en-tête"
        );

        // 3. L'aller-retour, à l'octet près.
        assert_eq!(
            c.desceller(&scelle)
                .unwrap_or_else(|e| panic!("{contexte} : {e}")),
            clair,
            "{contexte} : contenu rendu différent"
        );

        // 4. L'identifiant lu engage bien le déscellement exigeant.
        let id = Coffre::identifiant_lu(&scelle).expect("identifiant lisible");
        assert_eq!(
            c.desceller_avec_id(&id, &scelle)
                .unwrap_or_else(|e| panic!("{contexte} : identifiant refusé : {e}")),
            clair,
            "{contexte} : contenu rendu différent (avec identifiant)"
        );

        // 5. Une altération tirée au sort est refusée.
        let position = (hasard.suivant() % scelle.len() as u64) as usize;
        let bit = (hasard.suivant() % 8) as u8;
        let mut falsifie = scelle.clone();
        falsifie[position] ^= 1 << bit;
        doit_refuser(
            c.desceller(&falsifie),
            &format!("{contexte}, position {position}, bit {bit}"),
        );
    }
}

/// Le balayage **exhaustif des bornes**, sur plusieurs tailles de bloc — dont
/// la plus grande acceptée.
///
/// Les longueurs choisies sont celles où une formule en forme close se casse :
/// zéro octet, un octet, le bloc moins un, le bloc exactement, le bloc plus un,
/// puis les multiples. `B` et `B+1` sont tous deux parcourus parce qu'une erreur
/// d'arrondi se cache exactement entre les deux.
#[test]
fn aux_bornes_exactes_le_format_tient_pour_plusieurs_tailles_de_bloc() {
    let c = coffre("e1");

    for bloc in [
        BLOC_MINIMUM,
        BLOC_MINIMUM + 1,
        2048,
        4096,
        65_536,
        BLOC_MAXIMUM,
    ] {
        let b = bloc as usize;
        for longueur in [0usize, 1, b - 1, b, b + 1, 2 * b - 1, 2 * b, 2 * b + 1, 3 * b] {
            let contexte = format!("bloc {bloc}, longueur {longueur}");
            let clair: Vec<u8> = (0..longueur).map(|i| (i % 251) as u8).collect();

            let scelle = c
                .sceller_avec_bloc(&clair, bloc)
                .unwrap_or_else(|e| panic!("{contexte} : scellement refusé : {e}"));

            assert_eq!(
                scelle.len(),
                longueur_scellee(longueur, b),
                "{contexte} : longueur scellée inattendue"
            );
            assert_eq!(
                c.desceller(&scelle)
                    .unwrap_or_else(|e| panic!("{contexte} : {e}")),
                clair,
                "{contexte} : contenu rendu différent"
            );

            // La troncature d'un seul octet, à la borne exacte.
            doit_refuser(
                c.desceller(&scelle[..scelle.len() - 1]),
                &format!("{contexte} : troncature d'un octet"),
            );

            // Et l'ajout d'un seul octet.
            let mut etendu = scelle.clone();
            etendu.push(0);
            doit_refuser(c.desceller(&etendu), &format!("{contexte} : ajout d'un octet"));
        }
    }
}
