//! Le coffre — sceller du contenu de telle sorte que quiconque le détient sans
//! la clé n'en apprenne rien, et que toute altération se voie.
//!
//! ## La forme, et pourquoi
//!
//! ```text
//! ┌─ en-tête (70 octets, EN CLAIR) ────────────────────────────────┐
//! │ magic ‖ version ‖ drapeau ‖ taille_bloc ‖ nb_blocs ‖ longueur   │
//! │ ‖ empreinte de l'espace ‖ identifiant du coffre                 │
//! └─────────────────────────────────────────────────────────────────┘
//! ┌─ sceau d'en-tête (28 octets) ───────────────────────────────────┐
//! │ nonce ‖ étiquette — l'en-tête est authentifié MÊME À VIDE       │
//! └─────────────────────────────────────────────────────────────────┘
//! ┌─ bloc 0 : nonce ‖ chiffré+étiquette ────────────────────────────┐
//! ┌─ bloc 1 : …                                                     ┐
//! ```
//!
//! **L'en-tête est en clair, et c'est délibéré.** Il faut bien que le porteur
//! sache combien d'octets lire et à quel espace l'objet appartient. Ce qu'il ne
//! doit pas pouvoir faire, c'est le *modifier* : d'où le sceau d'en-tête, un
//! AEAD à message vide dont l'AAD *est* l'en-tête. Sans lui, un en-tête vide de
//! charge utile n'était couvert par rien, et `nb_blocs` devenait un conseil.
//!
//! **L'identifiant du coffre est tiré au sort à chaque scellement**, et il entre
//! dans l'AAD de chaque bloc — comme l'empreinte de l'espace.
//!
//! ## Le défaut que ce champ corrige, et il faut le raconter
//!
//! La première version ne liait que l'**espace**. Un test l'a prise en défaut :
//! deux coffres du même espace, de même taille de bloc et de même longueur —
//! c'est-à-dire deux scellements ordinaires du même client — avaient un en-tête
//! **identique octet pour octet**, donc un AAD identique. Conséquence : un
//! porteur sans la clé pouvait **greffer un bloc du coffre B dans le coffre A**,
//! et le bloc se descellait proprement. Le résultat reçu était un mélange des
//! deux contenus, et rien ne le signalait.
//!
//! C'est exactement ce que **G4** interdit — « personne ne substitue un résultat
//! sans être détecté » — et cela ne se voyait qu'en essayant. La leçon est
//! transférable : *lier l'identité d'un objet à son propriétaire ne suffit pas ;
//! il faut lier l'objet lui-même.*
//!
//! ## Ce que l'identifiant permet, et ce qu'il ne permet pas
//!
//! * **Greffe d'un bloc entre deux coffres** : détectée, les AAD diffèrent.
//! * **Substitution d'un coffre entier** par un autre, authentique et scellé par
//!   la même clé : **indétectable par [`Coffre::desceller`] seul**, parce que le
//!   coffre reçu est parfaitement valide. Il faut dire lequel on attend :
//!   [`Coffre::desceller_avec_id`]. C'est le début de l'étape 4 de l'ADR — les
//!   reçus signés — et le dire vaut mieux que de laisser croire que la
//!   substitution est couverte.
//! * **Rien n'est alloué avant que tout soit vérifié.** Un en-tête menteur — qui
//!   annonce 2⁶³ octets dans un fichier de 200 — est refusé par un calcul, pas
//!   par un `Vec::with_capacity`. Un déni de service par allocation est une
//!   panne, et une panne est un canal auxiliaire.
//!
//! ## Ce qui reste visible malgré tout
//!
//! * La **longueur exacte** du contenu — exacte, et **non arrondie**. La queue
//!   du dernier bloc n'est pas bourrée : l'étiquette d'un bloc fait `longueur du
//!   clair + 16`, et la taille de bloc est **en clair dans l'en-tête**. Un
//!   observateur retrouve donc la longueur au bit près, et le nombre de blocs
//!   avec. C'est mesuré, et non supposé, par
//!   `tests/attaques_externes.rs::constat_la_longueur_exacte_du_clair_est_lisible_sur_le_scelle`.
//!
//!   Pour masquer la taille, il faut [`crate::Enveloppe`], qui bourre à taille
//!   fixe par classes. Bourrer la queue du coffre fermerait cette fuite, au prix
//!   d'un format v2 et d'un gonflement pouvant atteindre une taille de bloc
//!   (64 Kio par défaut) **par objet** : c'est un arbitrage, pas un oubli.
//! * L'**empreinte de l'espace**, qui dit à quel espace l'objet appartient sans
//!   dire lequel des noms d'espace c'est.
//! * L'**identifiant du coffre**, qui distingue deux objets — c'est nécessaire,
//!   et c'est un identifiant stable qu'un observateur peut suivre.

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use zeroize::Zeroizing;

use crate::aleatoire;
use crate::cle::{
    CleEspace, EmpreinteEspace, LONGUEUR_EMPREINTE, LONGUEUR_ETIQUETTE, LONGUEUR_NONCE, Usage,
};
use crate::erreur::{CoffreError, Resultat};

/// Taille de l'en-tête d'un coffre, en octets.
pub const TETE_COFFRE: usize = 70;

/// Taille du sceau d'en-tête, en octets (nonce + étiquette).
pub const SCEAU_ENTETE: usize = LONGUEUR_NONCE + LONGUEUR_ETIQUETTE;

/// Longueur de l'identifiant d'un coffre, en octets.
pub const LONGUEUR_ID_COFFRE: usize = 16;

/// Taille du bloc par défaut, en octets.
pub const BLOC_DEFAUT: u32 = 64 * 1024;

/// Plus petit bloc accepté, en octets.
pub const BLOC_MINIMUM: u32 = 1024;

/// Plus grand bloc accepté, en octets (1 Mio).
pub const BLOC_MAXIMUM: u32 = 1024 * 1024;

/// L'identifiant d'un coffre : 16 octets tirés au sort à chaque scellement.
pub type IdCoffre = [u8; LONGUEUR_ID_COFFRE];

const MAGIC_COFFRE: &[u8; 4] = b"OBCF";
const VERSION_COFFRE: u8 = 1;

/// Position de l'empreinte d'espace dans l'en-tête.
const POSITION_ESPACE: usize = 22;
/// Position de l'identifiant de coffre dans l'en-tête.
const POSITION_ID: usize = 54;

/// Le coffre d'un espace : scelle et descelle le contenu.
///
/// Se construit **uniquement** à partir d'une [`CleEspace`]. Il n'existe aucune
/// variante qui prenne un nom d'espace et rende du clair : le type de la clé
/// *est* l'autorisation, et c'est là que la garantie cesse d'être une politique.
pub struct Coffre {
    sous_cle_contenu: Zeroizing<[u8; 32]>,
    sous_cle_entete: Zeroizing<[u8; 32]>,
    espace: EmpreinteEspace,
}

impl Coffre {
    /// Ouvre le coffre d'un espace.
    ///
    /// # Erreurs
    ///
    /// [`CoffreError::Format`] si HKDF échoue — impossible en pratique.
    pub fn neuf(cle: &CleEspace) -> Resultat<Self> {
        Ok(Self {
            sous_cle_contenu: cle.sous_cle(Usage::Coffre)?,
            sous_cle_entete: cle.sous_cle(Usage::EnteteCoffre)?,
            espace: cle.espace(),
        })
    }

    /// L'empreinte de l'espace de ce coffre.
    #[must_use]
    pub const fn espace(&self) -> EmpreinteEspace {
        self.espace
    }

    /// Scelle `clair` avec le bloc par défaut.
    ///
    /// # Erreurs
    ///
    /// * [`CoffreError::TropGrand`] si le contenu dépasse ce qu'un en-tête peut
    ///   décrire (2⁶⁴ octets — inatteignable en pratique) ;
    /// * [`CoffreError::Entropie`] si les nonces ou l'identifiant ne peuvent être
    ///   tirés ;
    /// * [`CoffreError::Format`] si l'AEAD refuse de sceller — impossible en
    ///   pratique.
    pub fn sceller(&self, clair: &[u8]) -> Resultat<Vec<u8>> {
        self.sceller_avec_bloc(clair, BLOC_DEFAUT)
    }

    /// Scelle `clair` avec une taille de bloc choisie.
    ///
    /// # Erreurs
    ///
    /// Comme [`Coffre::sceller`], plus [`CoffreError::Format`] si `taille_bloc`
    /// sort des bornes [`BLOC_MINIMUM`] / [`BLOC_MAXIMUM`].
    pub fn sceller_avec_bloc(&self, clair: &[u8], taille_bloc: u32) -> Resultat<Vec<u8>> {
        if !(BLOC_MINIMUM..=BLOC_MAXIMUM).contains(&taille_bloc) {
            return Err(CoffreError::Format(format!(
                "taille de bloc {taille_bloc} hors bornes [{BLOC_MINIMUM}, {BLOC_MAXIMUM}]"
            )));
        }

        let longueur = u64::try_from(clair.len()).map_err(|_| CoffreError::TropGrand {
            taille: clair.len(),
            maximum: usize::MAX,
        })?;
        let capacite = u64::from(taille_bloc);
        let nb_blocs =
            u32::try_from(longueur.div_ceil(capacite)).map_err(|_| CoffreError::TropGrand {
                taille: clair.len(),
                maximum: usize::MAX,
            })?;

        // L'identifiant est neuf à chaque scellement : deux coffres du même
        // espace, de même forme et de même clé ne partagent donc **aucun** AAD.
        let identifiant: IdCoffre = aleatoire::octets()?;
        let en_tete = self.en_tete(taille_bloc, nb_blocs, longueur, &identifiant);

        let mut sortie = Vec::with_capacity(
            TETE_COFFRE
                + SCEAU_ENTETE
                + usize::try_from(nb_blocs).unwrap_or(0)
                    * (LONGUEUR_NONCE + taille_bloc as usize + LONGUEUR_ETIQUETTE),
        );
        sortie.extend_from_slice(&en_tete);

        // Le sceau de l'en-tête : message vide, AAD = l'en-tête entier.
        let nonce_entete: [u8; LONGUEUR_NONCE] = aleatoire::octets()?;
        let sceau = sceller(&self.sous_cle_entete, &nonce_entete, &en_tete, &[])?;
        sortie.extend_from_slice(&nonce_entete);
        sortie.extend_from_slice(&sceau);

        for (index, morceau) in clair.chunks(taille_bloc as usize).enumerate() {
            let index = u32::try_from(index).map_err(|_| CoffreError::TropGrand {
                taille: clair.len(),
                maximum: usize::MAX,
            })?;
            let nonce: [u8; LONGUEUR_NONCE] = aleatoire::octets()?;
            let aad = aad_bloc(&en_tete, index, morceau.len());
            let chiffre = sceller(&self.sous_cle_contenu, &nonce, &aad, morceau)?;
            sortie.extend_from_slice(&nonce);
            sortie.extend_from_slice(&chiffre);
        }

        Ok(sortie)
    }

    /// Descelle un contenu scellé par [`Coffre::sceller`].
    ///
    /// # Erreurs
    ///
    /// * [`CoffreError::Format`] si la structure est incohérente — magic,
    ///   version, drapeau, taille de bloc hors bornes, nombre de blocs qui ne
    ///   correspond pas à la longueur annoncée, ou longueur totale qui ne
    ///   correspond pas aux octets reçus (c'est ainsi que la **troncature** et
    ///   l'**ajout** sont attrapés, avant toute allocation) ;
    /// * [`CoffreError::EspaceIncoherent`] si le coffre appartient à un autre
    ///   espace que celui de la clé — nommé, parce que l'en-tête est en clair et
    ///   que le dire ne révèle rien ;
    /// * [`CoffreError::Authentification`] si l'en-tête ou un bloc a été
    ///   modifié, greffé depuis un autre coffre, ou si la clé n'est pas la bonne.
    pub fn desceller(&self, scelle: &[u8]) -> Resultat<Vec<u8>> {
        Ok(self.desceller_interne(scelle)?.0)
    }

    /// Descelle en exigeant un identifiant de coffre précis.
    ///
    /// C'est la seule façon de détecter la **substitution d'un coffre entier** :
    /// un coffre venu d'ailleurs est parfaitement valide, donc invisible à
    /// [`Coffre::desceller`]. L'appelant qui sait quel objet il attend passe son
    /// identifiant — obtenu par [`Coffre::identifiant_lu`] au moment du
    /// scellement, ou par le canal qui a commandé le calcul.
    ///
    /// # Erreurs
    ///
    /// Comme [`Coffre::desceller`], plus [`CoffreError::CoffreInconnu`] si
    /// l'identifiant reçu n'est pas celui attendu. Le contrôle a lieu **après**
    /// l'authentification de l'en-tête : un identifiant lu dans des octets non
    /// authentifiés ne prouve rien.
    pub fn desceller_avec_id(&self, id_attendu: &IdCoffre, scelle: &[u8]) -> Resultat<Vec<u8>> {
        let (clair, identifiant) = self.desceller_interne(scelle)?;
        if &identifiant != id_attendu {
            return Err(CoffreError::CoffreInconnu {
                attendu: hex::encode(id_attendu),
                lu: hex::encode(identifiant),
            });
        }
        Ok(clair)
    }

    /// L'identifiant inscrit dans un coffre, sans le desceller.
    ///
    /// Ce que le porteur peut lire — et ce que l'appelant peut consigner pour
    /// exiger le même objet plus tard. **Il n'est digne de confiance qu'après**
    /// un déscellement réussi : avant, c'est ce que le porteur a bien voulu
    /// écrire.
    ///
    /// # Erreurs
    ///
    /// [`CoffreError::Format`] si l'en-tête est trop court ou incohérent.
    pub fn identifiant_lu(scelle: &[u8]) -> Resultat<IdCoffre> {
        let en_tete = lire_entete_seule(scelle)?;
        let mut identifiant = [0u8; LONGUEUR_ID_COFFRE];
        identifiant.copy_from_slice(&en_tete[POSITION_ID..TETE_COFFRE]);
        Ok(identifiant)
    }

    /// Le déscellement, qui rend aussi l'identifiant authentifié.
    fn desceller_interne(&self, scelle: &[u8]) -> Resultat<(Vec<u8>, IdCoffre)> {
        let (en_tete, taille_bloc, nb_blocs, longueur) = self.lire_entete(scelle)?;

        // L'en-tête est en clair : dire « ce coffre n'est pas du même espace »
        // n'apprend rien à qui que ce soit, et évite de faire passer un
        // malentendu d'espace pour une clé fausse.
        let mut empreinte = [0u8; LONGUEUR_EMPREINTE];
        empreinte.copy_from_slice(&en_tete[POSITION_ESPACE..POSITION_ID]);
        let espace_scelle = EmpreinteEspace::depuis_octets(empreinte);
        if espace_scelle != self.espace {
            return Err(CoffreError::EspaceIncoherent {
                attendu: self.espace,
                scelle: espace_scelle,
            });
        }

        let attendu = taille_totale(taille_bloc, nb_blocs, longueur)?;
        if attendu != scelle.len() as u64 {
            return Err(CoffreError::Format(format!(
                "coffre de {} octets, {} attendus d'après l'en-tête",
                scelle.len(),
                attendu
            )));
        }

        // Le sceau de l'en-tête, avant tout le reste : si l'en-tête a bougé,
        // rien de ce qu'il annonce ne vaut.
        let nonce_entete: [u8; LONGUEUR_NONCE] = scelle[TETE_COFFRE..TETE_COFFRE + LONGUEUR_NONCE]
            .try_into()
            .map_err(|_| CoffreError::Format("nonce d'en-tête illisible".to_owned()))?;
        ouvrir(
            &self.sous_cle_entete,
            &nonce_entete,
            &en_tete,
            &scelle[TETE_COFFRE + LONGUEUR_NONCE..TETE_COFFRE + SCEAU_ENTETE],
        )?;

        let mut identifiant = [0u8; LONGUEUR_ID_COFFRE];
        identifiant.copy_from_slice(&en_tete[POSITION_ID..TETE_COFFRE]);

        let mut clair = Vec::with_capacity(usize::try_from(longueur).unwrap_or(0));
        let mut curseur = TETE_COFFRE + SCEAU_ENTETE;
        let mut reste = longueur;

        for index in 0..nb_blocs {
            let longueur_bloc = reste.min(u64::from(taille_bloc));
            let longueur_bloc = usize::try_from(longueur_bloc).map_err(|_| {
                CoffreError::Format("longueur de bloc hors des tailles adressables".to_owned())
            })?;

            let nonce: [u8; LONGUEUR_NONCE] = scelle[curseur..curseur + LONGUEUR_NONCE]
                .try_into()
                .map_err(|_| CoffreError::Format("nonce de bloc illisible".to_owned()))?;
            curseur += LONGUEUR_NONCE;

            let fin = curseur + longueur_bloc + LONGUEUR_ETIQUETTE;
            let aad = aad_bloc(&en_tete, index, longueur_bloc);
            let mut tampon = Zeroizing::new(vec![0u8; longueur_bloc]);
            ouvrir(&self.sous_cle_contenu, &nonce, &aad, &scelle[curseur..fin])
                .map(|octets| tampon.copy_from_slice(&octets))?;
            clair.extend_from_slice(&tampon[..]);
            curseur = fin;
            reste -= longueur_bloc as u64;
        }

        Ok((clair, identifiant))
    }

    /// Lit et vérifie la cohérence de l'en-tête, sans rien allouer.
    fn lire_entete(&self, scelle: &[u8]) -> Resultat<([u8; TETE_COFFRE], u32, u32, u64)> {
        let en_tete = lire_entete_seule(scelle)?;

        let taille_bloc = u32::from_le_bytes(lire4(&en_tete[6..10]));
        let nb_blocs = u32::from_le_bytes(lire4(&en_tete[10..14]));
        let mut longueur_octets = [0u8; 8];
        longueur_octets.copy_from_slice(&en_tete[14..22]);
        let longueur = u64::from_le_bytes(longueur_octets);

        let attendu_blocs = longueur.div_ceil(u64::from(taille_bloc));
        if u64::from(nb_blocs) != attendu_blocs {
            return Err(CoffreError::Format(format!(
                "en-tête incohérent : {nb_blocs} blocs annoncés, {attendu_blocs} attendus pour {longueur} octets"
            )));
        }

        Ok((en_tete, taille_bloc, nb_blocs, longueur))
    }

    /// Construit l'en-tête en clair d'un coffre.
    fn en_tete(
        &self,
        taille_bloc: u32,
        nb_blocs: u32,
        longueur: u64,
        identifiant: &IdCoffre,
    ) -> [u8; TETE_COFFRE] {
        let mut en_tete = [0u8; TETE_COFFRE];
        en_tete[0..4].copy_from_slice(MAGIC_COFFRE);
        en_tete[4] = VERSION_COFFRE;
        en_tete[5] = 0;
        en_tete[6..10].copy_from_slice(&taille_bloc.to_le_bytes());
        en_tete[10..14].copy_from_slice(&nb_blocs.to_le_bytes());
        en_tete[14..22].copy_from_slice(&longueur.to_le_bytes());
        en_tete[POSITION_ESPACE..POSITION_ID].copy_from_slice(self.espace.octets());
        en_tete[POSITION_ID..TETE_COFFRE].copy_from_slice(identifiant);
        en_tete
    }
}

impl core::fmt::Debug for Coffre {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Coffre(espace={}, •••)", self.espace)
    }
}

/// Lit la partie structurelle de l'en-tête, sans juger les longueurs.
fn lire_entete_seule(scelle: &[u8]) -> Resultat<[u8; TETE_COFFRE]> {
    if scelle.len() < TETE_COFFRE + SCEAU_ENTETE {
        return Err(CoffreError::Format(format!(
            "coffre de {} octets, minimum {} (en-tête + sceau)",
            scelle.len(),
            TETE_COFFRE + SCEAU_ENTETE
        )));
    }
    if &scelle[0..4] != MAGIC_COFFRE {
        return Err(CoffreError::Format("magic de coffre inconnu".to_owned()));
    }
    if scelle[4] != VERSION_COFFRE {
        return Err(CoffreError::Format(format!(
            "version de coffre {} inconnue (cette build lit la {VERSION_COFFRE})",
            scelle[4]
        )));
    }
    if scelle[5] != 0 {
        return Err(CoffreError::Format(format!(
            "drapeau de coffre inconnu : {} (aucun drapeau n'est défini)",
            scelle[5]
        )));
    }

    let mut en_tete = [0u8; TETE_COFFRE];
    en_tete.copy_from_slice(&scelle[0..TETE_COFFRE]);

    let taille_bloc = u32::from_le_bytes(lire4(&en_tete[6..10]));
    if !(BLOC_MINIMUM..=BLOC_MAXIMUM).contains(&taille_bloc) {
        return Err(CoffreError::Format(format!(
            "taille de bloc annoncée {taille_bloc} hors bornes [{BLOC_MINIMUM}, {BLOC_MAXIMUM}]"
        )));
    }

    Ok(en_tete)
}

/// L'AAD d'un bloc : l'en-tête entier — empreinte d'espace **et identifiant du
/// coffre** compris — plus l'index, plus la longueur en clair du bloc.
///
/// C'est ce qui lie le bloc à *cet* objet précis, et non seulement à son espace :
/// sans l'identifiant, deux coffres du même espace partageaient leur AAD.
fn aad_bloc(en_tete: &[u8; TETE_COFFRE], index: u32, longueur_clair: usize) -> Vec<u8> {
    let mut aad = Vec::with_capacity(TETE_COFFRE + 8);
    aad.extend_from_slice(en_tete);
    aad.extend_from_slice(&index.to_le_bytes());
    aad.extend_from_slice(&(longueur_clair as u32).to_le_bytes());
    aad
}

/// Taille totale attendue d'un coffre, d'après son en-tête.
///
/// Calculée **en forme close et en arithmétique vérifiée** : un en-tête qui
/// annonce des tailles absurdement grandes doit être refusé par un calcul, pas
/// par une allocation.
fn taille_totale(taille_bloc: u32, nb_blocs: u32, longueur: u64) -> Resultat<u64> {
    let capacite = u64::from(taille_bloc);
    let fixe = (TETE_COFFRE + SCEAU_ENTETE) as u64;
    let par_bloc = LONGUEUR_NONCE as u64 + LONGUEUR_ETIQUETTE as u64;

    if nb_blocs == 0 {
        if longueur != 0 {
            return Err(CoffreError::Format(
                "en-tête incohérent : aucun bloc, mais une longueur non nulle".to_owned(),
            ));
        }
        return Ok(fixe);
    }

    let pleins = u64::from(nb_blocs) - 1;
    let reste = longueur
        .checked_sub(pleins.checked_mul(capacite).ok_or_else(|| {
            CoffreError::Format(
                "en-tête incohérent : longueur multipliée par les blocs déborde".to_owned(),
            )
        })?)
        .ok_or_else(|| {
            CoffreError::Format(
                "en-tête incohérent : moins d'octets que de blocs pleins annoncés".to_owned(),
            )
        })?;
    if reste == 0 || reste > capacite {
        return Err(CoffreError::Format(
            "en-tête incohérent : le dernier bloc n'a pas une longueur valide".to_owned(),
        ));
    }

    let taille = fixe
        .checked_add(pleins.checked_mul(capacite + par_bloc).ok_or_else(|| {
            CoffreError::Format("en-tête incohérent : taille débordante".to_owned())
        })?)
        .and_then(|t| t.checked_add(par_bloc + reste))
        .ok_or_else(|| CoffreError::Format("en-tête incohérent : taille débordante".to_owned()))?;
    Ok(taille)
}

fn lire4(tranche: &[u8]) -> [u8; 4] {
    let mut tampon = [0u8; 4];
    if let Some(source) = tranche.get(0..4) {
        tampon.copy_from_slice(source);
    }
    tampon
}

fn sceller(
    sous_cle: &[u8; 32],
    nonce: &[u8; LONGUEUR_NONCE],
    aad: &[u8],
    clair: &[u8],
) -> Resultat<Vec<u8>> {
    let chiffreur = Aes256Gcm::new_from_slice(sous_cle)
        .map_err(|_| CoffreError::Format("sous-clé AES-256 de taille invalide".to_owned()))?;
    chiffreur
        .encrypt(Nonce::from_slice(nonce), Payload { msg: clair, aad })
        .map_err(|_| CoffreError::Format("scellement AEAD refusé".to_owned()))
}

fn ouvrir(
    sous_cle: &[u8; 32],
    nonce: &[u8; LONGUEUR_NONCE],
    aad: &[u8],
    chiffre: &[u8],
) -> Resultat<Zeroizing<Vec<u8>>> {
    let chiffreur = Aes256Gcm::new_from_slice(sous_cle)
        .map_err(|_| CoffreError::Format("sous-clé AES-256 de taille invalide".to_owned()))?;
    let clair = chiffreur
        .decrypt(Nonce::from_slice(nonce), Payload { msg: chiffre, aad })
        .map_err(|_| CoffreError::Authentification)?;
    Ok(Zeroizing::new(clair))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::cle::contient;

    /// Un marqueur long et reconnaissable, pour prouver son absence ensuite.
    const MARQUEUR: &[u8] = b"RACINE-DU-GRAPHE-INTERDITE-A-LA-LECTURE-0001";

    fn cle(espace: &str) -> CleEspace {
        CleEspace::engendrer(espace).unwrap()
    }

    fn coffre(espace: &str) -> Coffre {
        Coffre::neuf(&cle(espace)).unwrap()
    }

    /// Le tour complet, sur toutes les tailles qui changent de régime : vide,
    /// un octet, un bloc pile, un bloc plus un, plusieurs blocs.
    #[test]
    fn tour_complet_sur_toutes_les_tailles() {
        let coffre = coffre("e1");
        let bloc = BLOC_DEFAUT as usize;
        for taille in [
            0,
            1,
            2,
            1023,
            bloc - 1,
            bloc,
            bloc + 1,
            3 * bloc,
            3 * bloc + 7,
        ] {
            let clair: Vec<u8> = (0..taille).map(|i| (i % 251) as u8).collect();
            let scelle = coffre.sceller(&clair).unwrap();
            let rendu = coffre.desceller(&scelle).unwrap();
            assert_eq!(rendu, clair, "taille {taille}");
        }
    }

    /// **Le critère de l'ADR, exécutable** : `hexdump` du scellé ne rend aucun
    /// littéral du clair.
    #[test]
    fn hexdump_ne_rend_aucun_litteral() {
        let coffre = coffre("e1");
        let clair = [MARQUEUR, b" et voici d'autres octets lisibles"].concat();
        let scelle = coffre.sceller(&clair).unwrap();

        assert!(
            !contient(&scelle, MARQUEUR),
            "le marqueur ne doit apparaître nulle part dans les octets scellés"
        );
        assert!(!contient(&scelle, b"d'autres octets lisibles"));
    }

    /// L'empreinte de l'espace et l'identifiant, eux, apparaissent — et c'est le
    /// seul prix payé : on sait à quel espace l'objet appartient et lequel c'est,
    /// pas ce qu'il contient.
    #[test]
    fn l_espace_est_lisible_le_contenu_non() {
        let espace = "espace-confidentiel";
        let coffre = coffre(espace);
        let scelle = coffre.sceller(MARQUEUR).unwrap();
        assert!(contient(&scelle, coffre.espace().octets()));
        assert!(contient(&scelle, &Coffre::identifiant_lu(&scelle).unwrap()));
        assert!(!contient(&scelle, espace.as_bytes()));
    }

    /// Deux scellements du même clair donnent des octets différents (nonces et
    /// identifiant neufs) — sans quoi l'égalité de deux objets scellés serait
    /// observable.
    #[test]
    fn deux_scellements_differents() {
        let coffre = coffre("e1");
        let a = coffre.sceller(MARQUEUR).unwrap();
        let b = coffre.sceller(MARQUEUR).unwrap();
        assert_ne!(a, b);
        assert_eq!(a.len(), b.len());
        assert_ne!(
            Coffre::identifiant_lu(&a).unwrap(),
            Coffre::identifiant_lu(&b).unwrap(),
            "deux coffres doivent porter deux identifiants"
        );
    }

    /// Chaque zone de l'en-tête est couverte : la modifier se voit.
    #[test]
    fn toute_zone_de_l_entete_est_couverte() {
        let coffre = coffre("e1");
        let bon = coffre.sceller(MARQUEUR).unwrap();

        for (nom, position) in [
            ("magic", 0usize),
            ("version", 4),
            ("drapeau", 5),
            ("taille de bloc", 6),
            ("nombre de blocs", 10),
            ("longueur", 14),
            ("identifiant de coffre", POSITION_ID),
            (
                "dernier octet de l'identifiant",
                POSITION_ID + LONGUEUR_ID_COFFRE - 1,
            ),
        ] {
            let mut faux = bon.clone();
            faux[position] ^= 0x01;
            assert!(
                coffre.desceller(&faux).is_err(),
                "modifier « {nom} » doit être détecté"
            );
        }
    }

    /// **Anti-échange d'espace** : même clé, autre espace → refus nommé.
    #[test]
    fn un_coffre_d_un_autre_espace_est_refuse() {
        let k = cle("e1");
        let scelle = Coffre::neuf(&k).unwrap().sceller(MARQUEUR).unwrap();

        let mut faux = scelle.clone();
        faux[POSITION_ESPACE..POSITION_ID]
            .copy_from_slice(EmpreinteEspace::calculer("e2").octets());

        assert!(matches!(
            Coffre::neuf(&k).unwrap().desceller(&faux),
            Err(CoffreError::EspaceIncoherent { .. })
        ));
    }

    /// **Réordonnancement** : échanger deux blocs pleins est détecté par l'index
    /// présent dans l'AAD.
    #[test]
    fn reordonnancement_detecte() {
        let coffre = coffre("e1");
        let taille_bloc = 1024u32;
        let clair: Vec<u8> = (0..3000).map(|i| (i % 251) as u8).collect();
        let scelle = coffre.sceller_avec_bloc(&clair, taille_bloc).unwrap();

        let bloc = LONGUEUR_NONCE + taille_bloc as usize + LONGUEUR_ETIQUETTE;
        let debut = TETE_COFFRE + SCEAU_ENTETE;
        let mut faux = scelle.clone();
        let premier = faux[debut..debut + bloc].to_vec();
        faux.copy_within(debut + bloc..debut + 2 * bloc, debut);
        faux[debut + bloc..debut + 2 * bloc].copy_from_slice(&premier);

        assert!(matches!(
            coffre.desceller(&faux),
            Err(CoffreError::Authentification)
        ));
    }

    /// **Greffe d'un bloc venu d'un autre coffre — la faille trouvée par ce
    /// test.** Deux coffres du même espace, de même clé, de même taille de bloc
    /// et de même longueur : avant l'identifiant de coffre, leurs en-têtes
    /// étaient identiques octet pour octet, donc leurs AAD aussi, et la greffe
    /// **passait inaperçue**. Elle doit être détectée.
    #[test]
    fn greffe_d_un_bloc_etranger_detectee() {
        let k = cle("e1");
        let coffre = Coffre::neuf(&k).unwrap();
        let taille_bloc = 1024u32;

        let a = coffre
            .sceller_avec_bloc(&vec![b'A'; 2048], taille_bloc)
            .unwrap();
        let b = coffre
            .sceller_avec_bloc(&vec![b'B'; 2048], taille_bloc)
            .unwrap();

        // Les deux coffres ont exactement la même forme. Avant l'identifiant,
        // **tout** l'en-tête était identique — c'est précisément cela qui
        // rendait la greffe invisible.
        assert_eq!(a.len(), b.len());
        assert_eq!(
            &a[0..POSITION_ID],
            &b[0..POSITION_ID],
            "deux coffres de même forme partagent tout l'en-tête jusqu'à l'identifiant"
        );
        assert_ne!(
            &a[POSITION_ID..TETE_COFFRE],
            &b[POSITION_ID..TETE_COFFRE],
            "seul l'identifiant les distingue — et c'est lui qui entre dans l'AAD"
        );

        let bloc = LONGUEUR_NONCE + taille_bloc as usize + LONGUEUR_ETIQUETTE;
        let debut = TETE_COFFRE + SCEAU_ENTETE;
        let mut faux = a.clone();
        faux[debut..debut + bloc].copy_from_slice(&b[debut..debut + bloc]);

        assert!(
            matches!(coffre.desceller(&faux), Err(CoffreError::Authentification)),
            "un bloc greffé depuis un autre coffre doit être refusé, pas descellé"
        );
    }

    /// **Substitution d'un coffre entier** : impossible à voir sans dire lequel
    /// on attend — et c'est pourquoi `desceller_avec_id` existe.
    #[test]
    fn substitution_d_un_coffre_entier_detectee_avec_l_identifiant() {
        let k = cle("e1");
        let coffre = Coffre::neuf(&k).unwrap();

        let attendu = coffre.sceller(b"solde: 1 000 000").unwrap();
        let substitue = coffre.sceller(b"solde: 0").unwrap();

        let id_attendu = Coffre::identifiant_lu(&attendu).unwrap();

        // Sans identifiant attendu, le coffre substitué est parfaitement valide :
        // la crate ne peut pas le deviner, et le dit.
        assert_eq!(
            coffre.desceller(&substitue).unwrap(),
            b"solde: 0",
            "un coffre entier valide n'est pas distinguable d'un autre sans identifiant"
        );

        // Avec l'identifiant attendu, la substitution est détectée.
        match coffre.desceller_avec_id(&id_attendu, &substitue) {
            Err(CoffreError::CoffreInconnu { attendu, lu }) => {
                assert_eq!(attendu, hex::encode(id_attendu));
                assert_eq!(lu, hex::encode(Coffre::identifiant_lu(&substitue).unwrap()));
            }
            autre => panic!("attendu CoffreInconnu, reçu {autre:?}"),
        }

        // Et le bon objet passe.
        assert_eq!(
            coffre.desceller_avec_id(&id_attendu, &attendu).unwrap(),
            b"solde: 1 000 000"
        );
    }

    /// **Troncature et ajout** : la taille totale attendue est recalculée depuis
    /// l'en-tête authentifié, donc tout écart est vu — y compris une coupe qui
    /// tombe juste après un bloc entier.
    #[test]
    fn troncature_et_ajout_detectes() {
        let coffre = coffre("e1");
        let scelle = coffre.sceller(&vec![7u8; 5000]).unwrap();

        for coupe in [1usize, 16, 64, LONGUEUR_NONCE + 16, 1000] {
            let faux = &scelle[..scelle.len() - coupe];
            assert!(
                coffre.desceller(faux).is_err(),
                "une troncature de {coupe} octets doit être détectée"
            );
        }

        let mut rallonge = scelle.clone();
        rallonge.push(0);
        assert!(coffre.desceller(&rallonge).is_err());
    }

    /// Une clé qui n'est pas la bonne ne descelle pas.
    #[test]
    fn mauvaise_cle_refusee() {
        let scelle = coffre("e1").sceller(MARQUEUR).unwrap();
        let autre = coffre("e1"); // même nom d'espace, clé tirée à part
        assert!(matches!(
            autre.desceller(&scelle),
            Err(CoffreError::Authentification)
        ));
    }

    /// **Un en-tête menteur est refusé par un calcul, pas par une allocation.**
    /// Il annonce 2⁶³ octets dans 200 octets reçus.
    #[test]
    fn en_tete_menteur_refuse_sans_allouer() {
        let coffre = coffre("e1");
        let mut faux = coffre.sceller(MARQUEUR).unwrap();
        faux[14..22].copy_from_slice(&(1u64 << 62).to_le_bytes());
        faux[10..14].copy_from_slice(&(1u32 << 20).to_le_bytes());

        match coffre.desceller(&faux) {
            Err(CoffreError::Format(message)) => {
                assert!(
                    message.contains("incohérent") || message.contains("attendus"),
                    "le refus doit venir de la cohérence : {message}"
                );
            }
            autre => panic!("attendu Format, reçu {autre:?}"),
        }
    }

    /// Une taille de bloc absurde est refusée avant tout travail.
    #[test]
    fn taille_de_bloc_hors_bornes_refusee() {
        let coffre = coffre("e1");
        assert!(matches!(
            coffre.sceller_avec_bloc(MARQUEUR, 16),
            Err(CoffreError::Format(_))
        ));
        assert!(matches!(
            coffre.sceller_avec_bloc(MARQUEUR, 8 * 1024 * 1024),
            Err(CoffreError::Format(_))
        ));

        let mut faux = coffre.sceller(MARQUEUR).unwrap();
        faux[6..10].copy_from_slice(&(4u32 << 30).to_le_bytes()); // 4 Gio
        assert!(matches!(
            coffre.desceller(&faux),
            Err(CoffreError::Format(_))
        ));
    }

    /// Un en-tête trop court est refusé sans lecture hors bornes.
    #[test]
    fn entete_tronque_refuse() {
        let coffre = coffre("e1");
        for taille in [
            0usize,
            1,
            TETE_COFFRE - 1,
            TETE_COFFRE,
            TETE_COFFRE + SCEAU_ENTETE - 1,
        ] {
            assert!(coffre.desceller(&vec![0u8; taille]).is_err());
        }
    }

    /// Le coffre vide est authentifié : l'en-tête porte un sceau même sans bloc.
    #[test]
    fn coffre_vide_authentifie() {
        let coffre = coffre("e1");
        let scelle = coffre.sceller(&[]).unwrap();
        assert_eq!(scelle.len(), TETE_COFFRE + SCEAU_ENTETE);
        assert_eq!(coffre.desceller(&scelle).unwrap(), Vec::<u8>::new());

        let mut faux = scelle.clone();
        faux[TETE_COFFRE + LONGUEUR_NONCE + 4] ^= 0x01;
        assert!(
            coffre.desceller(&faux).is_err(),
            "le sceau doit couvrir l'en-tête vide"
        );
    }

    /// Le calcul de taille totale refuse les en-têtes qui débordent, au lieu de
    /// paniquer.
    #[test]
    fn calcul_de_taille_ne_deborde_pas() {
        assert!(taille_totale(BLOC_DEFAUT, 0, 5_000).is_err());
        assert!(taille_totale(BLOC_DEFAUT, u32::MAX, u64::MAX).is_err());
        assert!(taille_totale(BLOC_DEFAUT, 1, 0).is_err());
        assert_eq!(
            taille_totale(BLOC_DEFAUT, 0, 0).unwrap(),
            (TETE_COFFRE + SCEAU_ENTETE) as u64
        );
    }
}
