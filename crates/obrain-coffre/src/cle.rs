//! La hiérarchie de clés — et pourquoi elle a cette forme.
//!
//! ```text
//! phrase de l'utilisateur            fichier de clé machine (32 octets)
//!        │ Argon2id, sel et params            │ HKDF, sel de l'enveloppe
//!        │ lus DANS l'enveloppe               │
//!        ▼                                    ▼
//!              clé maîtresse (KEK) ── jamais écrite, jamais transmise
//!                        │  AES-256-GCM
//!                        │  AAD = magic ‖ version ‖ kdf ‖ params ‖ sel ‖ espace
//!                        ▼
//!              enveloppe de clé (128 octets fixes, sur disque)
//!                        │
//!                        ▼
//!              clé d'espace (DEK, 32 octets, une par espace)
//!                        │  HKDF-SHA256, séparation de domaines par usage
//!                        ├──▶ « coffre »         → le contenu scellé
//!                        ├──▶ « entete-coffre »  → l'en-tête du coffre
//!                        ├──▶ « enveloppe »      → les charges utiles de calcul
//!                        └──▶ « jeton »          → les jetons de capacité
//! ```
//!
//! ## Trois décisions, et leur raison
//!
//! 1. **La KEK ne se stocke pas ; on la dérive.** Un secret écrit quelque part
//!    est un secret que l'opérateur lit. Une phrase ne se lit pas dans un
//!    fichier.
//! 2. **Argon2id, pas SHA-256.** Un attaquant qui copie l'enveloppe attaque la
//!    phrase hors ligne. Argon2id coûte de la mémoire *et* du temps par essai,
//!    ce qui retire tout rendement aux cartes graphiques.
//! 3. **L'empreinte de l'espace entre dans l'AAD.** Copier l'enveloppe d'un
//!    espace vers un autre ne donne pas accès à l'autre : le déscellement
//!    échoue, et l'appelant l'apprend.
//!
//! ## Le plancher, et l'attaque qu'il ferme
//!
//! Les paramètres Argon2id voyagent **dans l'enveloppe** — sinon un client qui
//! n'a que sa phrase ne pourrait pas dériver. Un attaquant qui peut réécrire ce
//! fichier ne casse donc pas la clé : il **abaisse les paramètres**, laisse la
//! victime dériver une clé faible, et attaque celle-ci. La parade a deux étages,
//! et le premier est placé exactement là où la dérivation a lieu :
//!
//! * [`EnveloppeCle::deriver_kek`] vérifie le plancher ([`ParametresKdf::PLANCHER`])
//!   **avant** de dériver — un fichier qui demande moins est refusé sans avoir
//!   rien calculé ;
//! * les paramètres sont couverts par l'AAD — les modifier, même au-dessus du
//!   plancher, fait échouer l'authentification.
//!
//! Le contrôle de l'AEAD ([`CleEspace::desceller`]) ne juge **pas** les
//! paramètres : il ne dérive rien, donc il n'a rien à en dire. Placer la
//! vérification ailleurs serait un rituel, pas une défense.

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use crate::aleatoire;
use crate::erreur::{CoffreError, Resultat};

/// Longueur d'une clé symétrique, en octets (AES-256).
pub const LONGUEUR_CLE: usize = 32;

/// Longueur d'un nonce AES-GCM, en octets (96 bits, la taille recommandée).
pub const LONGUEUR_NONCE: usize = 12;

/// Longueur d'une étiquette d'authentification AES-GCM, en octets.
pub const LONGUEUR_ETIQUETTE: usize = 16;

/// Longueur d'un sel Argon2id, en octets.
pub const LONGUEUR_SEL: usize = 16;

/// Longueur d'une empreinte d'espace, en octets (SHA-256).
pub const LONGUEUR_EMPREINTE: usize = 32;

/// Taille fixe d'une enveloppe de clé sur le disque, en octets.
///
/// Fixe, donc l'enveloppe ne dit rien de la clé qu'elle contient — pas même un
/// ordre de grandeur.
pub const TAILLE_ENVELOPPE_CLE: usize = 128;

/// Préfixe de domaine de la crate, pour HKDF et pour les empreintes.
pub(crate) const DOMAINE: &[u8] = b"obrain-coffre/v1/";

const MAGIC_CLE: &[u8; 6] = b"OBCCLE";
const VERSION_CLE: u8 = 1;
const KDF_ARGON2ID: u8 = 0;
const KDF_OCTETS_BRUTS: u8 = 1;

/// Taille de l'en-tête d'une enveloppe de clé, couvert par l'AAD.
const TETE_CLE: usize = 68;

// ---------------------------------------------------------------------------
// Usage — la séparation de domaines
// ---------------------------------------------------------------------------

/// L'usage d'une sous-clé dérivée de la clé d'espace.
///
/// Deux usages distincts ne peuvent pas être confondus : présenter un jeton
/// comme un bloc de coffre, ou un bloc de coffre comme une enveloppe de calcul,
/// échoue parce que la clé n'est pas la même. La séparation est
/// cryptographique, pas une étiquette vérifiée après coup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Usage {
    /// Le contenu scellé — le graphe, les données au repos.
    Coffre,
    /// L'en-tête du coffre, scellé à part pour être authentifié même à vide.
    EnteteCoffre,
    /// Les charges utiles de calcul, à taille fixe.
    Enveloppe,
    /// Les jetons de capacité.
    Jeton,
}

impl Usage {
    /// L'étiquette de domaine de l'usage, telle qu'elle entre dans HKDF.
    const fn etiquette(self) -> &'static [u8] {
        match self {
            Self::Coffre => b"coffre",
            Self::EnteteCoffre => b"entete-coffre",
            Self::Enveloppe => b"enveloppe",
            Self::Jeton => b"jeton",
        }
    }
}

// ---------------------------------------------------------------------------
// EmpreinteEspace — nommer un espace sans le révéler
// ---------------------------------------------------------------------------

/// Empreinte d'un nom d'espace : `SHA-256(DOMAINE ‖ "espace" ‖ nom)`.
///
/// Le nom de l'espace n'est **jamais** écrit dans les octets scellés, seulement
/// son empreinte. Deux espaces distincts ne peuvent pas produire la même
/// empreinte, et l'empreinte ne permet pas de retrouver le nom.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EmpreinteEspace([u8; LONGUEUR_EMPREINTE]);

impl EmpreinteEspace {
    /// Calcule l'empreinte d'un nom d'espace.
    ///
    /// Le nom est normalisé (espaces de début et de fin retirés) puis haché avec
    /// un préfixe de domaine, pour qu'une empreinte d'espace ne puisse pas
    /// coïncider avec le hachage d'autre chose.
    #[must_use]
    pub fn calculer(espace: &str) -> Self {
        let mut h = Sha256::new();
        h.update(DOMAINE);
        h.update(b"espace");
        h.update(espace.trim().as_bytes());
        let mut octets = [0u8; LONGUEUR_EMPREINTE];
        octets.copy_from_slice(&h.finalize());
        Self(octets)
    }

    /// Rend les octets de l'empreinte.
    #[must_use]
    pub const fn octets(&self) -> &[u8; LONGUEUR_EMPREINTE] {
        &self.0
    }

    /// Reconstruit une empreinte depuis ses octets (lecture d'un fichier scellé).
    #[must_use]
    pub const fn depuis_octets(octets: [u8; LONGUEUR_EMPREINTE]) -> Self {
        Self(octets)
    }

    /// Forme courte, pour les messages : douze caractères hexadécimaux.
    #[must_use]
    pub fn court(&self) -> String {
        hex::encode(&self.0[..6])
    }
}

impl core::fmt::Debug for EmpreinteEspace {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "EmpreinteEspace({}…)", self.court())
    }
}

impl core::fmt::Display for EmpreinteEspace {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}…", self.court())
    }
}

// ---------------------------------------------------------------------------
// Sel
// ---------------------------------------------------------------------------

/// Sel de dérivation : 16 octets tirés du système, jamais réutilisés.
///
/// Un sel n'est pas un secret ; il empêche que deux utilisateurs partageant la
/// même phrase dérivent la même clé, et rend les tables précalculées inutiles.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Sel([u8; LONGUEUR_SEL]);

impl Sel {
    /// Tire un sel neuf.
    ///
    /// # Erreurs
    ///
    /// [`CoffreError::Entropie`] si le système n'a pas rendu d'octets.
    pub fn neuf() -> Resultat<Self> {
        Ok(Self(aleatoire::octets::<LONGUEUR_SEL>()?))
    }

    /// Reconstruit un sel depuis ses octets.
    #[must_use]
    pub const fn depuis_octets(octets: [u8; LONGUEUR_SEL]) -> Self {
        Self(octets)
    }

    /// Rend les octets du sel.
    #[must_use]
    pub const fn octets(&self) -> &[u8; LONGUEUR_SEL] {
        &self.0
    }
}

impl core::fmt::Debug for Sel {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Sel({})", hex::encode(self.0))
    }
}

// ---------------------------------------------------------------------------
// ParametresKdf
// ---------------------------------------------------------------------------

/// Paramètres Argon2id : leur plancher, et leur défaut.
///
/// Le **plancher** est la valeur en dessous de laquelle une enveloppe est
/// refusée avant toute dérivation. Le **défaut** suit les recommandations OWASP
/// 2024 pour Argon2id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParametresKdf {
    /// Coût mémoire, en kibioctets.
    pub m_cost_kio: u32,
    /// Nombre de passes.
    pub t_cost: u32,
    /// Degré de parallélisme.
    pub p_cost: u32,
}

impl ParametresKdf {
    /// Plancher : 8 Mio, 2 passes, 1 fil. En dessous, refus **sans dérivation**.
    pub const PLANCHER: Self = Self {
        m_cost_kio: 8 * 1024,
        t_cost: 2,
        p_cost: 1,
    };

    /// Défaut : 19 Mio, 2 passes, 1 fil (recommandation OWASP 2024).
    pub const DEFAUT: Self = Self {
        m_cost_kio: 19 * 1024,
        t_cost: 2,
        p_cost: 1,
    };

    /// Vrai si ces paramètres atteignent le plancher sur les trois axes.
    #[must_use]
    pub const fn au_dessus_du_plancher(&self) -> bool {
        self.m_cost_kio >= Self::PLANCHER.m_cost_kio
            && self.t_cost >= Self::PLANCHER.t_cost
            && self.p_cost >= Self::PLANCHER.p_cost
    }

    /// Décrit l'écart au plancher, pour le message d'erreur.
    #[must_use]
    pub fn ecart(&self) -> String {
        let plancher = Self::PLANCHER;
        format!(
            "reçu m={} Kio t={} p={}, plancher m={} Kio t={} p={}",
            self.m_cost_kio,
            self.t_cost,
            self.p_cost,
            plancher.m_cost_kio,
            plancher.t_cost,
            plancher.p_cost
        )
    }
}

impl Default for ParametresKdf {
    fn default() -> Self {
        Self::DEFAUT
    }
}

// ---------------------------------------------------------------------------
// Origine de la clé maîtresse
// ---------------------------------------------------------------------------

/// D'où vient la clé maîtresse — l'information que l'enveloppe doit porter pour
/// qu'un tiers sache comment la desceller.
///
/// Sans elle, un client qui n'a que sa phrase dériverait la mauvaise KEK sur une
/// enveloppe faite pour un fichier de clé machine, et l'échec serait attribué à
/// une mauvaise phrase. Le format le dit donc, et [`EnveloppeCle::deriver_kek`]
/// refuse le cas qui ne le concerne pas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrigineKek {
    /// Dérivée d'une phrase par Argon2id.
    Phrase,
    /// Dérivée d'octets de clé déjà tirés au sort, par HKDF.
    OctetsBruts,
}

// ---------------------------------------------------------------------------
// CleMaitresse
// ---------------------------------------------------------------------------

/// La KEK : dérivée d'une phrase ou d'un fichier de clé, **jamais écrite**.
///
/// Zéroïsée à la destruction. Son `Debug` ne rend rien d'exploitable : un secret
/// qui fuit par un `{:?}` dans un journal est un secret perdu.
#[derive(Clone)]
pub struct CleMaitresse {
    octets: Zeroizing<[u8; LONGUEUR_CLE]>,
    origine: OrigineKek,
}

impl CleMaitresse {
    /// Dérive la clé maîtresse d'une phrase, avec Argon2id.
    ///
    /// **Le sel est celui de l'enveloppe**, et ne se choisit pas librement : un
    /// sel qui ne correspond pas à celui du scellement produit une enveloppe
    /// que personne ne descelle. C'est pourquoi cette fonction n'est pas
    /// publique — [`EnveloppeCle::deriver_kek`] est le seul chemin, et il lit le
    /// sel là où il a été écrit.
    ///
    /// # Erreurs
    ///
    /// * [`CoffreError::KdfTropFaible`] si `parametres` est sous le plancher —
    ///   refus **avant** toute dérivation ;
    /// * [`CoffreError::Format`] si Argon2 refuse les paramètres.
    pub(crate) fn depuis_phrase(
        phrase: &str,
        sel: &Sel,
        parametres: ParametresKdf,
    ) -> Resultat<Self> {
        if !parametres.au_dessus_du_plancher() {
            return Err(CoffreError::KdfTropFaible(parametres.ecart()));
        }
        let params = Params::new(
            parametres.m_cost_kio,
            parametres.t_cost,
            parametres.p_cost,
            Some(LONGUEUR_CLE),
        )
        .map_err(|e| CoffreError::Format(format!("paramètres Argon2id refusés : {e}")))?;
        let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

        let mut cle = Zeroizing::new([0u8; LONGUEUR_CLE]);
        argon
            .hash_password_into(phrase.as_bytes(), sel.octets(), &mut cle[..])
            .map_err(|e| CoffreError::Format(format!("dérivation Argon2id échouée : {e}")))?;
        Ok(Self {
            octets: cle,
            origine: OrigineKek::Phrase,
        })
    }

    /// Dérive la clé maîtresse d'octets déjà tirés au sort (clé machine).
    ///
    /// Les octets fournis ne servent **pas** directement de KEK : ils passent
    /// par HKDF avec le sel de l'enveloppe, de sorte que le même fichier de clé
    /// ne produise pas la même KEK pour deux espaces. Comme
    /// [`CleMaitresse::depuis_phrase`], cette fonction n'est pas publique : le
    /// sel appartient à l'enveloppe.
    ///
    /// # Erreurs
    ///
    /// [`CoffreError::Format`] si l'expansion HKDF échoue — impossible en
    /// pratique, la taille demandée étant celle du condensat.
    pub(crate) fn depuis_octets(octets: [u8; LONGUEUR_CLE], sel: &Sel) -> Resultat<Self> {
        let source = Zeroizing::new(octets);
        let hk = Hkdf::<Sha256>::new(Some(sel.octets()), &source[..]);
        let mut cle = Zeroizing::new([0u8; LONGUEUR_CLE]);
        hk.expand(b"obrain-coffre/v1/kek-octets-bruts", &mut cle[..])
            .map_err(|_| CoffreError::Format("expansion HKDF de la KEK échouée".to_owned()))?;
        Ok(Self {
            octets: cle,
            origine: OrigineKek::OctetsBruts,
        })
    }

    /// D'où cette clé maîtresse a été dérivée.
    #[must_use]
    pub const fn origine(&self) -> OrigineKek {
        self.origine
    }

    /// Les octets de la KEK, pour l'AEAD.
    ///
    /// Jamais public : le type de la clé *est* l'autorisation, et l'exposer la
    /// dissoudrait.
    pub(crate) fn octets(&self) -> &[u8; LONGUEUR_CLE] {
        &self.octets
    }
}

impl core::fmt::Debug for CleMaitresse {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "CleMaitresse({:?}, •••)", self.origine)
    }
}

// ---------------------------------------------------------------------------
// CleEspace
// ---------------------------------------------------------------------------

/// La DEK : la clé d'un espace, 32 octets tirés au sort, jamais partagée.
///
/// C'est le seul objet qui permet de lire ou d'écrire dans un espace. Il ne se
/// fabrique pas de l'extérieur : [`CleEspace::engendrer`] ou
/// [`CleEspace::desceller`], et rien d'autre. **Aucune** fonction de cette crate
/// ne prend un nom d'espace et rend du clair : il faut l'objet.
#[derive(Clone)]
pub struct CleEspace {
    octets: Zeroizing<[u8; LONGUEUR_CLE]>,
    espace: EmpreinteEspace,
}

impl CleEspace {
    /// Engendre une clé neuve pour un espace.
    ///
    /// # Erreurs
    ///
    /// [`CoffreError::Entropie`] si le système n'a pas rendu d'octets.
    pub fn engendrer(espace: &str) -> Resultat<Self> {
        Ok(Self {
            octets: Zeroizing::new(aleatoire::octets::<LONGUEUR_CLE>()?),
            espace: EmpreinteEspace::calculer(espace),
        })
    }

    /// L'empreinte de l'espace auquel cette clé appartient.
    #[must_use]
    pub const fn espace(&self) -> EmpreinteEspace {
        self.espace
    }

    /// Dérive une sous-clé pour un usage donné.
    ///
    /// # Erreurs
    ///
    /// [`CoffreError::Format`] si HKDF échoue — impossible en pratique.
    pub(crate) fn sous_cle(&self, usage: Usage) -> Resultat<Zeroizing<[u8; LONGUEUR_CLE]>> {
        let hk = Hkdf::<Sha256>::new(None, &self.octets[..]);
        let mut info = Vec::with_capacity(DOMAINE.len() + 16);
        info.extend_from_slice(DOMAINE);
        info.extend_from_slice(usage.etiquette());
        let mut sous_cle = Zeroizing::new([0u8; LONGUEUR_CLE]);
        hk.expand(&info, &mut sous_cle[..])
            .map_err(|_| CoffreError::Format("expansion HKDF de la sous-clé échouée".to_owned()))?;
        Ok(sous_cle)
    }

    /// Enveloppe la clé sous une phrase, pour la poser sur le disque.
    ///
    /// Le sel est tiré **ici** et écrit dans l'enveloppe : c'est celui qui a
    /// servi à dériver la KEK. Il n'existe pas de variante qui prenne une KEK
    /// déjà dérivée et un sel quelconque — c'est volontaire, et c'est la
    /// correction d'un défaut trouvé par les tests : une API qui acceptait les
    /// deux permettait de produire une enveloppe dont le sel ne correspondait
    /// pas à la clé qui avait scellé, donc une enveloppe **indéscellable par son
    /// propre client**, tout en étant parfaitement authentique.
    ///
    /// # Erreurs
    ///
    /// * [`CoffreError::KdfTropFaible`] si `parametres` est sous le plancher ;
    /// * [`CoffreError::Format`] si les paramètres sont refusés par Argon2 ;
    /// * [`CoffreError::Entropie`] si le sel ou le nonce ne peuvent être tirés.
    pub fn envelopper_avec_phrase(
        &self,
        phrase: &str,
        parametres: ParametresKdf,
    ) -> Resultat<EnveloppeCle> {
        let sel = Sel::neuf()?;
        let kek = CleMaitresse::depuis_phrase(phrase, &sel, parametres)?;
        self.envelopper_avec_kek(&kek, &sel, parametres)
    }

    /// Enveloppe la clé sous une clé machine (32 octets déjà tirés au sort).
    ///
    /// Comme [`CleEspace::envelopper_avec_phrase`], le sel est tiré ici et écrit
    /// dans l'enveloppe : les octets fournis passent par HKDF avec ce sel, et ne
    /// servent donc jamais directement de clé.
    ///
    /// # Erreurs
    ///
    /// * [`CoffreError::KdfTropFaible`] si `parametres` est sous le plancher ;
    /// * [`CoffreError::Entropie`] si le sel ou le nonce ne peuvent être tirés ;
    /// * [`CoffreError::Format`] si l'expansion HKDF échoue.
    pub fn envelopper_avec_cle(
        &self,
        octets: [u8; LONGUEUR_CLE],
        parametres: ParametresKdf,
    ) -> Resultat<EnveloppeCle> {
        let sel = Sel::neuf()?;
        let kek = CleMaitresse::depuis_octets(octets, &sel)?;
        self.envelopper_avec_kek(&kek, &sel, parametres)
    }

    /// Le scellement lui-même : la KEK a été dérivée **avec ce sel**, et le sel
    /// part dans l'en-tête pour que le client puisse la redériver.
    fn envelopper_avec_kek(
        &self,
        kek: &CleMaitresse,
        sel: &Sel,
        parametres: ParametresKdf,
    ) -> Resultat<EnveloppeCle> {
        if !parametres.au_dessus_du_plancher() {
            return Err(CoffreError::KdfTropFaible(parametres.ecart()));
        }

        let nonce: [u8; LONGUEUR_NONCE] = aleatoire::octets()?;

        let mut enveloppe = [0u8; TAILLE_ENVELOPPE_CLE];
        enveloppe[0..6].copy_from_slice(MAGIC_CLE);
        enveloppe[6] = VERSION_CLE;
        enveloppe[7] = match kek.origine() {
            OrigineKek::Phrase => KDF_ARGON2ID,
            OrigineKek::OctetsBruts => KDF_OCTETS_BRUTS,
        };
        enveloppe[8..12].copy_from_slice(&parametres.m_cost_kio.to_le_bytes());
        enveloppe[12..16].copy_from_slice(&parametres.t_cost.to_le_bytes());
        enveloppe[16..20].copy_from_slice(&parametres.p_cost.to_le_bytes());
        enveloppe[20..36].copy_from_slice(sel.octets());
        enveloppe[36..68].copy_from_slice(self.espace.octets());
        enveloppe[68..80].copy_from_slice(&nonce);

        let chiffre = sceller_sous(kek, &nonce, &enveloppe[0..TETE_CLE], &self.octets[..])?;
        enveloppe[80..].copy_from_slice(&chiffre);

        Ok(EnveloppeCle(enveloppe))
    }

    /// Descel le la clé d'espace depuis une enveloppe, avec une KEK déjà dérivée.
    ///
    /// # Erreurs
    ///
    /// [`CoffreError::CleMaitresseIncorrecte`] si l'AEAD rejette. Ce cas couvre
    /// **deux** causes que l'AEAD ne peut pas distinguer : la clé maîtresse
    /// n'est pas la bonne, ou l'enveloppe a été modifiée. Le message ne tranche
    /// pas, parce que rien ne permet de trancher.
    pub fn desceller(enveloppe: &EnveloppeCle, kek: &CleMaitresse) -> Resultat<Self> {
        let brut = enveloppe.octets();
        let nonce: [u8; LONGUEUR_NONCE] = brut[68..80]
            .try_into()
            .map_err(|_| CoffreError::Format("nonce illisible".to_owned()))?;
        let mut clair = Zeroizing::new([0u8; LONGUEUR_CLE]);
        ouvrir_sous(kek, &nonce, &brut[0..TETE_CLE], &brut[80..], &mut clair[..])
            .map_err(|_| CoffreError::CleMaitresseIncorrecte)?;

        let mut empreinte = [0u8; LONGUEUR_EMPREINTE];
        empreinte.copy_from_slice(&brut[36..68]);

        Ok(Self {
            octets: clair,
            espace: EmpreinteEspace::depuis_octets(empreinte),
        })
    }
}

impl core::fmt::Debug for CleEspace {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "CleEspace(espace={}, •••)", self.espace)
    }
}

// ---------------------------------------------------------------------------
// EnveloppeCle
// ---------------------------------------------------------------------------

/// Une clé d'espace enveloppée : 128 octets de taille fixe, posables sur disque.
#[derive(Clone, PartialEq, Eq)]
pub struct EnveloppeCle([u8; TAILLE_ENVELOPPE_CLE]);

impl EnveloppeCle {
    /// Les octets à écrire sur le disque.
    #[must_use]
    pub const fn octets(&self) -> &[u8; TAILLE_ENVELOPPE_CLE] {
        &self.0
    }

    /// Relit une enveloppe depuis ses octets.
    ///
    /// # Erreurs
    ///
    /// [`CoffreError::Format`] si le magic, la version ou la taille ne
    /// correspondent pas. La validité **cryptographique** n'est pas jugée ici :
    /// seul [`CleEspace::desceller`] le peut, et il le fait.
    pub fn depuis_octets(octets: &[u8]) -> Resultat<Self> {
        if octets.len() != TAILLE_ENVELOPPE_CLE {
            return Err(CoffreError::Format(format!(
                "enveloppe de clé de {} octets, attendu {TAILLE_ENVELOPPE_CLE}",
                octets.len()
            )));
        }
        if &octets[0..6] != MAGIC_CLE {
            return Err(CoffreError::Format(
                "magic d'enveloppe de clé inconnu".to_owned(),
            ));
        }
        if octets[6] != VERSION_CLE {
            return Err(CoffreError::Format(format!(
                "version d'enveloppe de clé {} inconnue (cette build lit la {VERSION_CLE})",
                octets[6]
            )));
        }
        let mut tampon = [0u8; TAILLE_ENVELOPPE_CLE];
        tampon.copy_from_slice(octets);
        Ok(Self(tampon))
    }

    /// Dérive la KEK depuis une phrase, **avec le sel et les paramètres inscrits
    /// dans l'enveloppe**.
    ///
    /// C'est le seul chemin correct pour un client qui n'a que sa phrase, et
    /// c'est ici que le plancher Argon2id est vérifié : un fichier dont les
    /// paramètres ont été abaissés est refusé **sans qu'aucune dérivation n'ait
    /// eu lieu**.
    ///
    /// # Erreurs
    ///
    /// * [`CoffreError::Format`] si l'enveloppe attend une clé machine et non
    ///   une phrase — le cas est nommé plutôt que confondu avec une mauvaise
    ///   phrase ;
    /// * [`CoffreError::KdfTropFaible`] si les paramètres sont sous le plancher ;
    /// * [`CoffreError::Format`] si Argon2 refuse les paramètres.
    pub fn deriver_kek(&self, phrase: &str) -> Resultat<CleMaitresse> {
        if !self.depuis_phrase() {
            return Err(CoffreError::Format(
                "cette enveloppe a été produite depuis un fichier de clé, pas depuis une phrase"
                    .to_owned(),
            ));
        }
        let parametres = self.parametres();
        if !parametres.au_dessus_du_plancher() {
            return Err(CoffreError::KdfTropFaible(parametres.ecart()));
        }
        CleMaitresse::depuis_phrase(phrase, &self.sel(), parametres)
    }

    /// Dérive la KEK depuis un fichier de clé machine, avec le sel de
    /// l'enveloppe.
    ///
    /// # Erreurs
    ///
    /// [`CoffreError::Format`] si l'enveloppe attend une phrase, ou si HKDF
    /// échoue.
    pub fn deriver_kek_octets(&self, octets: [u8; LONGUEUR_CLE]) -> Resultat<CleMaitresse> {
        if self.depuis_phrase() {
            return Err(CoffreError::Format(
                "cette enveloppe a été produite depuis une phrase, pas depuis un fichier de clé"
                    .to_owned(),
            ));
        }
        CleMaitresse::depuis_octets(octets, &self.sel())
    }

    /// L'empreinte de l'espace inscrite dans l'enveloppe.
    ///
    /// Elle n'est **authentifiée** qu'après [`CleEspace::desceller`] : la lire
    /// avant, c'est lire ce qu'un attaquant a bien voulu écrire.
    #[must_use]
    pub fn espace_affiche(&self) -> EmpreinteEspace {
        let mut empreinte = [0u8; LONGUEUR_EMPREINTE];
        empreinte.copy_from_slice(&self.0[36..68]);
        EmpreinteEspace::depuis_octets(empreinte)
    }

    /// Vrai si l'enveloppe attend une phrase (Argon2id) plutôt qu'un fichier de
    /// clé machine.
    #[must_use]
    pub const fn depuis_phrase(&self) -> bool {
        self.0[7] == KDF_ARGON2ID
    }

    /// Les paramètres de dérivation inscrits dans l'enveloppe.
    ///
    /// Rendus **tels quels**, sans jugement : un appelant qui veut les vérifier
    /// peut le faire, et [`EnveloppeCle::deriver_kek`] le fait.
    #[must_use]
    pub fn parametres(&self) -> ParametresKdf {
        ParametresKdf {
            m_cost_kio: u32::from_le_bytes(lire4(&self.0[8..12])),
            t_cost: u32::from_le_bytes(lire4(&self.0[12..16])),
            p_cost: u32::from_le_bytes(lire4(&self.0[16..20])),
        }
    }

    /// Le sel inscrit dans l'enveloppe, pour rejouer la dérivation.
    #[must_use]
    pub fn sel(&self) -> Sel {
        let mut sel = [0u8; LONGUEUR_SEL];
        sel.copy_from_slice(&self.0[20..36]);
        Sel::depuis_octets(sel)
    }
}

impl core::fmt::Debug for EnveloppeCle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "EnveloppeCle(espace={}, {}, {} octets)",
            self.espace_affiche(),
            if self.depuis_phrase() {
                "phrase"
            } else {
                "clé machine"
            },
            TAILLE_ENVELOPPE_CLE
        )
    }
}

// ---------------------------------------------------------------------------
// L'AEAD, en deux fonctions
// ---------------------------------------------------------------------------

fn lire4(tranche: &[u8]) -> [u8; 4] {
    let mut tampon = [0u8; 4];
    if let Some(source) = tranche.get(0..4) {
        tampon.copy_from_slice(source);
    }
    tampon
}

/// Scelle `clair` sous la KEK, avec `aad` en données authentifiées non chiffrées.
pub(crate) fn sceller_sous(
    kek: &CleMaitresse,
    nonce: &[u8; LONGUEUR_NONCE],
    aad: &[u8],
    clair: &[u8],
) -> Resultat<Vec<u8>> {
    let chiffreur = Aes256Gcm::new_from_slice(kek.octets())
        .map_err(|_| CoffreError::Format("clé AES-256 de taille invalide".to_owned()))?;
    chiffreur
        .encrypt(Nonce::from_slice(nonce), Payload { msg: clair, aad })
        .map_err(|_| CoffreError::Format("scellement AEAD refusé".to_owned()))
}

/// Descel le dans `sortie` sous la KEK. Rend une erreur si l'étiquette, la
/// longueur ou l'AAD ne correspondent pas — sans jamais dire lequel des trois.
pub(crate) fn ouvrir_sous(
    kek: &CleMaitresse,
    nonce: &[u8; LONGUEUR_NONCE],
    aad: &[u8],
    chiffre: &[u8],
    sortie: &mut [u8],
) -> Resultat<()> {
    let chiffreur = Aes256Gcm::new_from_slice(kek.octets())
        .map_err(|_| CoffreError::Format("clé AES-256 de taille invalide".to_owned()))?;
    let mut clair = chiffreur
        .decrypt(Nonce::from_slice(nonce), Payload { msg: chiffre, aad })
        .map_err(|_| CoffreError::Authentification)?;
    if clair.len() != sortie.len() {
        clair.zeroize();
        return Err(CoffreError::Authentification);
    }
    sortie.copy_from_slice(&clair);
    clair.zeroize();
    Ok(())
}

/// Cherche `aiguille` dans `foin` — pour les tests qui prouvent une absence.
#[cfg(test)]
pub(crate) fn contient(foin: &[u8], aiguille: &[u8]) -> bool {
    !aiguille.is_empty() && foin.windows(aiguille.len()).any(|f| f == aiguille)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const PARAMS: ParametresKdf = ParametresKdf {
        m_cost_kio: 8 * 1024,
        t_cost: 2,
        p_cost: 1,
    };
    const PHRASE: &str = "phrase de test";

    /// Une clé d'espace et son enveloppe, scellée sous `PHRASE`.
    fn enveloppe(espace: &str) -> (CleEspace, EnveloppeCle) {
        let cle = CleEspace::engendrer(espace).unwrap();
        let env = cle.envelopper_avec_phrase(PHRASE, PARAMS).unwrap();
        (cle, env)
    }

    /// La KEK du client, dérivée **depuis l'enveloppe** — le seul chemin.
    fn kek(env: &EnveloppeCle) -> CleMaitresse {
        env.deriver_kek(PHRASE).unwrap()
    }

    /// Le tour complet : engendrer, envelopper, desceller, retrouver la clé.
    #[test]
    fn tour_complet_de_la_hierarchie() {
        let (cle, env) = enveloppe("e1");
        let retrouvee = CleEspace::desceller(&env, &kek(&env)).unwrap();

        assert_eq!(retrouvee.espace(), cle.espace());
        assert_eq!(
            retrouvee.sous_cle(Usage::Coffre).unwrap()[..],
            cle.sous_cle(Usage::Coffre).unwrap()[..],
            "la clé descelée doit être identique à la clé scellée"
        );
    }

    /// **Le chemin d'un client qui n'a que sa phrase** : l'enveloppe porte le sel
    /// et les paramètres, donc la KEK se re-dérive et la clé se descelle.
    ///
    /// C'est le test qui a révélé le défaut : tant que l'API acceptait un sel
    /// quelconque, on pouvait fabriquer une enveloppe dont le sel ne
    /// correspondait pas à la clé qui avait scellé — authentique, et
    /// indéscellable par son propre client.
    #[test]
    fn un_client_avec_sa_phrase_retrouve_la_cle() {
        let (cle, env) = enveloppe("e1");

        // Ailleurs, sur une autre machine : plus rien que la phrase et le fichier.
        let kek_ailleurs = env.deriver_kek(PHRASE).unwrap();
        let retrouvee = CleEspace::desceller(&env, &kek_ailleurs).unwrap();
        assert_eq!(retrouvee.espace(), cle.espace());
        assert_eq!(
            retrouvee.sous_cle(Usage::Coffre).unwrap()[..],
            cle.sous_cle(Usage::Coffre).unwrap()[..]
        );
    }

    /// Le sel écrit dans l'enveloppe est bien celui qui a servi à dériver : on
    /// peut le rejouer à la main et retrouver la même KEK.
    #[test]
    fn le_sel_de_l_enveloppe_est_celui_du_scellement() {
        let (_, env) = enveloppe("e1");
        let rejouee = CleMaitresse::depuis_phrase(PHRASE, &env.sel(), env.parametres()).unwrap();
        assert!(
            CleEspace::desceller(&env, &rejouee).is_ok(),
            "rejouer la dérivation depuis le sel inscrit doit fonctionner"
        );
    }

    /// Une autre phrase ne descelle pas — et le message ne dit pas *pourquoi*.
    #[test]
    fn mauvaise_phrase_refusee() {
        let (_, env) = enveloppe("e1");
        assert!(matches!(
            CleEspace::desceller(&env, &env.deriver_kek("autre phrase").unwrap()),
            Err(CoffreError::CleMaitresseIncorrecte)
        ));
    }

    /// **Attaque par abaissement des paramètres.** L'attaquant réécrit `m_cost`
    /// sous le plancher : le refus a lieu **avant** toute dérivation.
    #[test]
    fn parametres_abaisses_refuses_avant_derivation() {
        let (_, env) = enveloppe("e1");
        let mut octets = *env.octets();
        octets[8..12].copy_from_slice(&8u32.to_le_bytes()); // 8 Kio au lieu de 8 Mio

        let modifiee = EnveloppeCle::depuis_octets(&octets).unwrap();
        match modifiee.deriver_kek(PHRASE) {
            Err(CoffreError::KdfTropFaible(ecart)) => {
                assert!(
                    ecart.contains("plancher"),
                    "le message doit nommer le plancher : {ecart}"
                );
            }
            autre => panic!("attendu KdfTropFaible, reçu {autre:?}"),
        }
    }

    /// **Le second étage** : des paramètres modifiés *au-dessus* du plancher ne
    /// sont pas refusés par le plancher — ils sont attrapés par l'AAD, parce
    /// qu'ils font partie de ce qui a été scellé.
    #[test]
    fn parametres_modifies_au_dessus_du_plancher_casses_par_l_aad() {
        let (_, env) = enveloppe("e1");
        let mut octets = *env.octets();
        octets[8..12].copy_from_slice(&(16 * 1024u32).to_le_bytes()); // 16 Mio : au-dessus

        let modifiee = EnveloppeCle::depuis_octets(&octets).unwrap();
        let kek_decalee = modifiee.deriver_kek(PHRASE).unwrap(); // le plancher laisse passer
        assert!(
            matches!(
                CleEspace::desceller(&modifiee, &kek_decalee),
                Err(CoffreError::CleMaitresseIncorrecte)
            ),
            "l'AAD doit couvrir les paramètres, sinon l'en-tête est modifiable"
        );
    }

    /// Un sel modifié produit une autre KEK, donc un refus — l'AAD couvre le sel.
    #[test]
    fn sel_modifie_casse_le_descellement() {
        let (_, env) = enveloppe("e1");
        let mut octets = *env.octets();
        octets[20] ^= 0x01;

        let modifiee = EnveloppeCle::depuis_octets(&octets).unwrap();
        assert!(matches!(
            CleEspace::desceller(&modifiee, &modifiee.deriver_kek(PHRASE).unwrap()),
            Err(CoffreError::CleMaitresseIncorrecte)
        ));
    }

    /// **Le plancher est vérifié aussi à l'émission** : on ne peut pas fabriquer
    /// une enveloppe faible, même en le demandant.
    #[test]
    fn plancher_verifie_a_l_emission() {
        let cle = CleEspace::engendrer("e1").unwrap();
        let faible = ParametresKdf {
            m_cost_kio: 64,
            t_cost: 1,
            p_cost: 1,
        };
        assert!(matches!(
            cle.envelopper_avec_phrase(PHRASE, faible),
            Err(CoffreError::KdfTropFaible(_))
        ));
        assert!(matches!(
            cle.envelopper_avec_cle([1u8; 32], faible),
            Err(CoffreError::KdfTropFaible(_))
        ));
        assert!(matches!(
            CleMaitresse::depuis_phrase(PHRASE, &Sel::neuf().unwrap(), faible),
            Err(CoffreError::KdfTropFaible(_))
        ));
    }

    /// **Le cas machine contre le cas phrase** : chaque enveloppe dit ce qu'elle
    /// attend, et refuser le mauvais chemin est nommé, pas confondu avec une
    /// mauvaise phrase.
    #[test]
    fn origine_de_la_kek_est_respectee() {
        let cle = CleEspace::engendrer("e1").unwrap();

        let par_phrase = cle.envelopper_avec_phrase(PHRASE, PARAMS).unwrap();
        assert!(par_phrase.depuis_phrase());
        assert!(matches!(
            par_phrase.deriver_kek_octets([3u8; 32]),
            Err(CoffreError::Format(_))
        ));

        let par_machine = cle.envelopper_avec_cle([3u8; 32], PARAMS).unwrap();
        assert!(!par_machine.depuis_phrase());
        assert!(matches!(
            par_machine.deriver_kek(PHRASE),
            Err(CoffreError::Format(_))
        ));

        // Et le bon chemin, lui, marche.
        let kek_machine = par_machine.deriver_kek_octets([3u8; 32]).unwrap();
        let retrouvee = CleEspace::desceller(&par_machine, &kek_machine).unwrap();
        assert_eq!(retrouvee.espace(), cle.espace());

        // Un autre fichier de clé machine ne descelle pas.
        let autre = par_machine.deriver_kek_octets([4u8; 32]).unwrap();
        assert!(matches!(
            CleEspace::desceller(&par_machine, &autre),
            Err(CoffreError::CleMaitresseIncorrecte)
        ));
    }

    /// Un seul octet modifié dans le chiffré fait échouer le déscellement.
    #[test]
    fn un_octet_modifie_est_detecte() {
        let (_, env) = enveloppe("e1");
        for position in [80usize, 100, 127] {
            let mut octets = *env.octets();
            octets[position] ^= 0x01;
            let modifiee = EnveloppeCle::depuis_octets(&octets).unwrap();
            assert!(
                matches!(
                    CleEspace::desceller(&modifiee, &kek(&env)),
                    Err(CoffreError::CleMaitresseIncorrecte)
                ),
                "octet {position}"
            );
        }
    }

    /// **Anti-échange d'espace** : l'empreinte de l'espace est couverte par
    /// l'AAD. La réécrire fait échouer le déscellement — on ne peut donc pas
    /// faire croire à une clé qu'elle appartient à un autre espace.
    #[test]
    fn empreinte_espace_non_substituable() {
        let (_, env) = enveloppe("e1");
        let mut octets = *env.octets();
        octets[36..68].copy_from_slice(EmpreinteEspace::calculer("e2").octets());
        let modifiee = EnveloppeCle::depuis_octets(&octets).unwrap();
        assert!(matches!(
            CleEspace::desceller(&modifiee, &kek(&env)),
            Err(CoffreError::CleMaitresseIncorrecte)
        ));
    }

    /// La clé d'espace **n'apparaît pas** dans l'enveloppe : c'est le critère
    /// qui compte vraiment pour G1.
    #[test]
    fn la_cle_n_apparait_pas_dans_l_enveloppe() {
        let (cle, env) = enveloppe("e1");
        assert!(
            !contient(env.octets(), &cle.octets[..]),
            "les 32 octets de la clé ne doivent apparaître nulle part dans l'enveloppe"
        );
        assert!(!contient(env.octets(), &kek(&env).octets()[..]));
        assert!(!contient(env.octets(), PHRASE.as_bytes()));
    }

    /// Deux enveloppes de la même clé sont différentes (sel et nonce neufs).
    #[test]
    fn deux_enveloppes_de_la_meme_cle_different() {
        let cle = CleEspace::engendrer("e1").unwrap();
        let a = cle.envelopper_avec_phrase(PHRASE, PARAMS).unwrap();
        let b = cle.envelopper_avec_phrase(PHRASE, PARAMS).unwrap();
        assert_ne!(a.octets(), b.octets());
        assert_eq!(a.octets().len(), TAILLE_ENVELOPPE_CLE);

        // Et les deux restent descellables — un sel neuf ne casse rien.
        assert!(CleEspace::desceller(&a, &a.deriver_kek(PHRASE).unwrap()).is_ok());
        assert!(CleEspace::desceller(&b, &b.deriver_kek(PHRASE).unwrap()).is_ok());
    }

    /// Un magic, une version ou une taille inconnus sont refusés à la lecture.
    #[test]
    fn format_refuse_a_la_lecture() {
        let (_, env) = enveloppe("e1");
        let bon = *env.octets();

        let mut mauvais_magic = bon;
        mauvais_magic[0] = b'X';
        assert!(matches!(
            EnveloppeCle::depuis_octets(&mauvais_magic),
            Err(CoffreError::Format(_))
        ));

        let mut mauvaise_version = bon;
        mauvaise_version[6] = 99;
        assert!(matches!(
            EnveloppeCle::depuis_octets(&mauvaise_version),
            Err(CoffreError::Format(_))
        ));

        assert!(matches!(
            EnveloppeCle::depuis_octets(&bon[..127]),
            Err(CoffreError::Format(_))
        ));
    }

    /// Les usages sont séparés : une sous-clé ne sert pas dans un autre domaine.
    #[test]
    fn separation_des_domaines() {
        let cle = CleEspace::engendrer("e1").unwrap();
        let coffre = cle.sous_cle(Usage::Coffre).unwrap();
        let entete = cle.sous_cle(Usage::EnteteCoffre).unwrap();
        let jeton = cle.sous_cle(Usage::Jeton).unwrap();
        let enveloppe = cle.sous_cle(Usage::Enveloppe).unwrap();
        assert_ne!(coffre[..], entete[..]);
        assert_ne!(coffre[..], jeton[..]);
        assert_ne!(coffre[..], enveloppe[..]);
        assert_ne!(jeton[..], enveloppe[..]);
        assert_ne!(entete[..], jeton[..]);
    }

    /// L'empreinte d'espace distingue les noms sans les révéler.
    #[test]
    fn empreintes_d_espaces_distinctes() {
        assert_ne!(
            EmpreinteEspace::calculer("e1"),
            EmpreinteEspace::calculer("e2")
        );
        assert_eq!(
            EmpreinteEspace::calculer(" e1 "),
            EmpreinteEspace::calculer("e1"),
            "la normalisation doit rendre la même empreinte"
        );
        assert!(!contient(EmpreinteEspace::calculer("e1").octets(), b"e1"));
    }

    /// Le `Debug` d'un secret ne rend pas le secret.
    #[test]
    fn debug_ne_fuit_pas() {
        let (cle, env) = enveloppe("e1");
        let kek = kek(&env);
        let texte = format!("{cle:?} {kek:?} {env:?}");
        assert!(texte.contains("•••"));
        assert!(!texte.contains(&hex::encode(&cle.octets[..])));
        assert!(!texte.contains(&hex::encode(kek.octets())));
    }

    /// Une impossibilité n'est pas une décision — et l'API le dit.
    #[test]
    fn cle_absente_est_une_impossibilite() {
        assert!(CoffreError::CleAbsente("rien".to_owned()).est_une_impossibilite());
        assert!(!CoffreError::Authentification.est_une_impossibilite());
        assert!(!CoffreError::AutorisationRefusee("non".to_owned()).est_une_impossibilite());
        assert!(!CoffreError::CleMaitresseIncorrecte.est_une_impossibilite());
    }
}
