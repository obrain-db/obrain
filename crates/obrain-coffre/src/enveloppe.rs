//! L'enveloppe — ce que l'engine peut transporter sans pouvoir le lire.
//!
//! ## Le problème qu'elle traite
//!
//! Un coffre scellé cache le contenu, pas sa **taille**. Or la taille parle :
//! « 4 Mio » et « 300 octets » ne désignent pas le même genre de question, et un
//! adversaire patient reconstruit beaucoup d'un graphe social avec les seules
//! tailles et les seuls horaires. C'est l'invariant **G3** de l'ADR.
//!
//! L'enveloppe répond par le **bourrage à classes fixes** : toute charge utile
//! est rangée dans la plus petite classe qui la contient, et la classe est
//! complétée de zéros. Deux charges utiles de tailles différentes, dans la même
//! classe, produisent des octets de **longueur identique**.
//!
//! ## Ce qui reste visible, et il faut le dire
//!
//! * La **classe** — cinq valeurs possibles, pas la taille exacte. Une classe de
//!   64 Kio est franchement plus grosse qu'une classe de 256 octets : le
//!   bourrage réduit le canal, il ne l'annule pas.
//! * L'**instant** et la **fréquence** — hors du périmètre de cette crate.
//! * Le **destinataire** (l'empreinte de l'espace) — le routage l'exige.
//!
//! ## Le format
//!
//! ```text
//! magic ‖ version ‖ classe ‖ drapeau ‖ empreinte espace ‖ nonce ‖ chiffré+étiquette
//!   4   ‖    1    ‖   1    ‖    1    ‖        32        ‖  12   ‖  capacité + 16
//! ```
//!
//! Le clair, à l'intérieur, est `longueur(4) ‖ charge ‖ zéros` — la longueur
//! réelle est **dedans**, donc invisible. Le remplissage est vérifié nul au
//! déscellement : une enveloppe dont le remplissage n'est pas canonique est
//! refusée, ce qui interdit d'y cacher un canal.

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use zeroize::Zeroizing;

use crate::aleatoire;
use crate::cle::{
    CleEspace, EmpreinteEspace, LONGUEUR_EMPREINTE, LONGUEUR_ETIQUETTE, LONGUEUR_NONCE, Usage,
};
use crate::erreur::{CoffreError, Resultat};

/// Taille de l'en-tête d'une enveloppe, en octets.
pub const TETE_ENVELOPPE: usize = 39;

const MAGIC_ENVELOPPE: &[u8; 4] = b"OBEN";
const VERSION_ENVELOPPE: u8 = 1;

/// En-tête du clair : la longueur réelle, en octets.
const TETE_CLAIR: usize = 4;

// ---------------------------------------------------------------------------
// ClasseTaille
// ---------------------------------------------------------------------------

/// Les classes de taille d'une enveloppe.
///
/// Cinq classes, et rien entre : le nombre de tailles observables est donc
/// **borné à cinq**, quelle que soit la charge utile. C'est cela qui fait de la
/// taille une information grossière au lieu d'une information fine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClasseTaille {
    /// 256 octets — un accusé, un jeton, un identifiant.
    Octets256,
    /// 1 Kio — une requête, un petit plan.
    Octets1024,
    /// 4 Kio — un plan moyen, une page de résultat.
    Octets4096,
    /// 16 Kio — un lot.
    Octets16384,
    /// 64 Kio — le maximum.
    Octets65536,
}

impl ClasseTaille {
    /// Toutes les classes, de la plus petite à la plus grande.
    pub const TOUTES: [Self; 5] = [
        Self::Octets256,
        Self::Octets1024,
        Self::Octets4096,
        Self::Octets16384,
        Self::Octets65536,
    ];

    /// Capacité de charge utile de la classe, en octets.
    #[must_use]
    pub const fn capacite(self) -> usize {
        match self {
            Self::Octets256 => 256,
            Self::Octets1024 => 1024,
            Self::Octets4096 => 4096,
            Self::Octets16384 => 16_384,
            Self::Octets65536 => 65_536,
        }
    }

    /// Capacité de charge utile réelle de la classe, en octets — en-tête du
    /// clair déduit.
    ///
    /// C'est cette valeur, et non [`ClasseTaille::capacite`], qui décide si une
    /// charge entre : les quatre premiers octets de la classe portent la
    /// longueur réelle.
    #[must_use]
    pub const fn capacite_utile(self) -> usize {
        self.capacite() - TETE_CLAIR
    }

    /// L'indice inscrit dans l'en-tête.
    #[must_use]
    pub const fn indice(self) -> u8 {
        match self {
            Self::Octets256 => 0,
            Self::Octets1024 => 1,
            Self::Octets4096 => 2,
            Self::Octets16384 => 3,
            Self::Octets65536 => 4,
        }
    }

    /// La plus grande charge utile acceptée, en octets.
    #[must_use]
    pub const fn maximum() -> usize {
        Self::Octets65536.capacite_utile()
    }

    /// La classe d'un indice, tel qu'il apparaît dans un en-tête.
    ///
    /// # Erreurs
    ///
    /// [`CoffreError::Format`] si l'indice ne correspond à aucune classe.
    pub fn depuis_indice(indice: u8) -> Resultat<Self> {
        Self::TOUTES
            .into_iter()
            .find(|c| c.indice() == indice)
            .ok_or_else(|| CoffreError::Format(format!("classe de taille inconnue : {indice}")))
    }

    /// La plus petite classe capable de contenir une charge utile de `taille`
    /// octets.
    ///
    /// # Erreurs
    ///
    /// [`CoffreError::TropGrand`] au-delà de [`ClasseTaille::maximum`].
    pub fn pour(taille: usize) -> Resultat<Self> {
        Self::TOUTES
            .into_iter()
            .find(|c| c.capacite_utile() >= taille)
            .ok_or(CoffreError::TropGrand {
                taille,
                maximum: Self::maximum(),
            })
    }

    /// La classe annoncée par les octets d'une enveloppe, sans les déchiffrer.
    ///
    /// Ce qu'un routeur — l'engine, qui n'a pas la clé — peut légitimement
    /// apprendre : la classe, et rien d'autre.
    ///
    /// # Erreurs
    ///
    /// [`CoffreError::Format`] si l'en-tête est trop court, si le magic ou la
    /// version ne correspondent pas, ou si la classe est inconnue.
    pub fn lue(octets: &[u8]) -> Resultat<Self> {
        let en_tete = lire_tete(octets)?;
        Self::depuis_indice(en_tete[5])
    }

    /// Longueur totale d'une enveloppe de cette classe.
    #[must_use]
    pub const fn taille_totale(self) -> usize {
        TETE_ENVELOPPE + LONGUEUR_NONCE + self.capacite() + LONGUEUR_ETIQUETTE
    }
}

// ---------------------------------------------------------------------------
// Enveloppe
// ---------------------------------------------------------------------------

/// L'enveloppe d'un espace : scelle des charges utiles à taille fixe.
///
/// Se construit **uniquement** à partir d'une [`CleEspace`], comme
/// [`crate::Coffre`] — il n'existe pas de chemin qui rende du clair sans la clé.
pub struct Enveloppe {
    sous_cle: Zeroizing<[u8; 32]>,
    espace: EmpreinteEspace,
}

impl Enveloppe {
    /// Ouvre les enveloppes d'un espace.
    ///
    /// # Erreurs
    ///
    /// [`CoffreError::Format`] si HKDF échoue — impossible en pratique.
    pub fn neuf(cle: &CleEspace) -> Resultat<Self> {
        Ok(Self {
            sous_cle: cle.sous_cle(Usage::Enveloppe)?,
            espace: cle.espace(),
        })
    }

    /// L'empreinte de l'espace de ces enveloppes.
    #[must_use]
    pub const fn espace(&self) -> EmpreinteEspace {
        self.espace
    }

    /// Scelle `charge` dans la plus petite classe qui la contient.
    ///
    /// # Erreurs
    ///
    /// * [`CoffreError::TropGrand`] au-delà de
    ///   [`ClasseTaille::maximum`] octets ;
    /// * [`CoffreError::Entropie`] si le nonce ne peut être tiré ;
    /// * [`CoffreError::Format`] si l'AEAD refuse de sceller — impossible.
    pub fn sceller(&self, charge: &[u8]) -> Resultat<Vec<u8>> {
        self.sceller_dans(charge, ClasseTaille::pour(charge.len())?)
    }

    /// Scelle `charge` dans une classe imposée.
    ///
    /// Imposer la classe sert à qui veut une taille **constante sur une
    /// conversation entière** : toutes les réponses d'un même échange dans la
    /// même classe, et la variation de taille disparaît complètement.
    ///
    /// # Erreurs
    ///
    /// [`CoffreError::TropGrand`] si `charge` ne tient pas dans `classe`.
    pub fn sceller_dans(&self, charge: &[u8], classe: ClasseTaille) -> Resultat<Vec<u8>> {
        if charge.len() > classe.capacite_utile() {
            return Err(CoffreError::TropGrand {
                taille: charge.len(),
                maximum: classe.capacite_utile(),
            });
        }

        // Le clair est TOUJOURS de la taille de la classe : c'est le bourrage.
        let mut clair = Zeroizing::new(vec![0u8; classe.capacite()]);
        clair[0..4].copy_from_slice(&(charge.len() as u32).to_le_bytes());
        clair[4..4 + charge.len()].copy_from_slice(charge);

        let en_tete = self.en_tete(classe);
        let nonce: [u8; LONGUEUR_NONCE] = aleatoire::octets()?;
        let chiffre = sceller(&self.sous_cle, &nonce, &en_tete, &clair)?;

        let mut sortie = Vec::with_capacity(classe.taille_totale());
        sortie.extend_from_slice(&en_tete);
        sortie.extend_from_slice(&nonce);
        sortie.extend_from_slice(&chiffre);
        Ok(sortie)
    }

    /// Descelle une enveloppe.
    ///
    /// # Erreurs
    ///
    /// * [`CoffreError::Format`] si la structure est incohérente (magic,
    ///   version, drapeau, classe inconnue, longueur qui ne correspond pas à la
    ///   classe annoncée, remplissage non nul) ;
    /// * [`CoffreError::EspaceIncoherent`] si l'enveloppe appartient à un autre
    ///   espace que celui de la clé ;
    /// * [`CoffreError::Authentification`] si les octets ont été modifiés ou si
    ///   la clé n'est pas la bonne.
    pub fn desceller(&self, octets: &[u8]) -> Resultat<Vec<u8>> {
        let en_tete = lire_tete(octets)?;
        let classe = ClasseTaille::depuis_indice(en_tete[5])?;

        if octets.len() != classe.taille_totale() {
            return Err(CoffreError::Format(format!(
                "enveloppe de {} octets, {} attendus pour la classe {}",
                octets.len(),
                classe.taille_totale(),
                classe.capacite()
            )));
        }

        let mut empreinte = [0u8; LONGUEUR_EMPREINTE];
        empreinte.copy_from_slice(&en_tete[7..TETE_ENVELOPPE]);
        let espace_scelle = EmpreinteEspace::depuis_octets(empreinte);
        if espace_scelle != self.espace {
            return Err(CoffreError::EspaceIncoherent {
                attendu: self.espace,
                scelle: espace_scelle,
            });
        }

        let nonce: [u8; LONGUEUR_NONCE] = octets[TETE_ENVELOPPE..TETE_ENVELOPPE + LONGUEUR_NONCE]
            .try_into()
            .map_err(|_| CoffreError::Format("nonce d'enveloppe illisible".to_owned()))?;
        let clair = ouvrir(
            &self.sous_cle,
            &nonce,
            &en_tete,
            &octets[TETE_ENVELOPPE + LONGUEUR_NONCE..],
        )?;

        Ok(depouiller(&clair)?.to_vec())
    }

    /// Construit l'en-tête en clair d'une enveloppe.
    fn en_tete(&self, classe: ClasseTaille) -> [u8; TETE_ENVELOPPE] {
        let mut en_tete = [0u8; TETE_ENVELOPPE];
        en_tete[0..4].copy_from_slice(MAGIC_ENVELOPPE);
        en_tete[4] = VERSION_ENVELOPPE;
        en_tete[5] = classe.indice();
        en_tete[6] = 0;
        // L'empreinte commence après le drapeau : magic ‖ version ‖ classe ‖
        // drapeau ‖ empreinte, dans cet ordre — voir `lire_tete`.
        en_tete[7..TETE_ENVELOPPE].copy_from_slice(self.espace.octets());
        en_tete
    }
}

impl core::fmt::Debug for Enveloppe {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Enveloppe(espace={}, •••)", self.espace)
    }
}

/// Lit et vérifie l'en-tête en clair d'une enveloppe.
fn lire_tete(octets: &[u8]) -> Resultat<[u8; TETE_ENVELOPPE]> {
    if octets.len() < TETE_ENVELOPPE {
        return Err(CoffreError::Format(format!(
            "enveloppe de {} octets, en-tête de {TETE_ENVELOPPE} au minimum",
            octets.len()
        )));
    }
    if &octets[0..4] != MAGIC_ENVELOPPE {
        return Err(CoffreError::Format("magic d'enveloppe inconnu".to_owned()));
    }
    if octets[4] != VERSION_ENVELOPPE {
        return Err(CoffreError::Format(format!(
            "version d'enveloppe {} inconnue (cette build lit la {VERSION_ENVELOPPE})",
            octets[4]
        )));
    }
    if octets[6] != 0 {
        return Err(CoffreError::Format(format!(
            "drapeau d'enveloppe inconnu : {} (aucun drapeau n'est défini)",
            octets[6]
        )));
    }
    let mut en_tete = [0u8; TETE_ENVELOPPE];
    en_tete.copy_from_slice(&octets[0..TETE_ENVELOPPE]);
    Ok(en_tete)
}

/// Vérifie la longueur inscrite et le remplissage, puis rend la charge utile.
///
/// Le remplissage doit être **exactement nul** : sans cette vérification, une
/// enveloppe bien formée pourrait transporter un second message dans ses zéros,
/// et le bourrage deviendrait un canal au lieu d'un camouflage.
fn depouiller(clair: &[u8]) -> Resultat<&[u8]> {
    if clair.len() < TETE_CLAIR {
        return Err(CoffreError::Format(
            "enveloppe descellée trop courte pour porter sa longueur".to_owned(),
        ));
    }
    let mut longueur_octets = [0u8; 4];
    longueur_octets.copy_from_slice(&clair[0..TETE_CLAIR]);
    let longueur = u32::from_le_bytes(longueur_octets) as usize;

    let charge = clair
        .get(TETE_CLAIR..TETE_CLAIR + longueur)
        .ok_or_else(|| {
            CoffreError::Format(format!(
                "longueur inscrite {longueur} au-delà des {} octets de la classe",
                clair.len()
            ))
        })?;

    if clair[TETE_CLAIR + longueur..].iter().any(|o| *o != 0) {
        return Err(CoffreError::Format(
            "remplissage non nul : le bourrage ne doit porter aucun octet".to_owned(),
        ));
    }

    Ok(charge)
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

    fn cle(espace: &str) -> CleEspace {
        CleEspace::engendrer(espace).unwrap()
    }

    fn enveloppe(espace: &str) -> Enveloppe {
        Enveloppe::neuf(&cle(espace)).unwrap()
    }

    /// Le tour complet, dans chaque classe, aux bornes comprises.
    #[test]
    fn tour_complet_dans_chaque_classe() {
        let env = enveloppe("e1");
        for classe in ClasseTaille::TOUTES {
            let capacite = classe.capacite();
            for taille in [0, 1, capacite / 2, capacite - TETE_CLAIR] {
                let charge: Vec<u8> = (0..taille).map(|i| (i % 251) as u8).collect();
                let scellee = env.sceller_dans(&charge, classe).unwrap();
                assert_eq!(scellee.len(), classe.taille_totale(), "classe {classe:?}");
                assert_eq!(env.desceller(&scellee).unwrap(), charge);
            }
        }
    }

    /// **Le critère G3, exécutable** : deux charges utiles différentes, de
    /// tailles différentes, produisent des enveloppes de **longueur identique**.
    #[test]
    fn tailles_differentes_meme_longueur() {
        let env = enveloppe("e1");
        let courte = env.sceller(b"x").unwrap();
        let moyenne = env.sceller(&[b'y'; 200]).unwrap();
        let longue = env
            .sceller(b"une requete entierement differente et plus longue")
            .unwrap();

        assert_eq!(courte.len(), moyenne.len());
        assert_eq!(moyenne.len(), longue.len());
        assert_eq!(courte.len(), ClasseTaille::Octets256.taille_totale());

        // Et au-delà : même classe, même longueur, contenu sans rapport.
        let classe_1024_courte = env.sceller(&[0u8; 300]).unwrap();
        let classe_1024_longue = env.sceller(&[0xffu8; 1000]).unwrap();
        assert_eq!(classe_1024_courte.len(), classe_1024_longue.len());
        assert_eq!(
            classe_1024_courte.len(),
            ClasseTaille::Octets1024.taille_totale()
        );
    }

    /// Le nombre de longueurs observables est **borné par les classes** : c'est
    /// tout l'objet du bourrage.
    #[test]
    fn les_longueurs_observables_sont_bornees() {
        let env = enveloppe("e1");
        let mut longueurs: Vec<usize> = (1..900)
            .map(|taille| env.sceller(&vec![0u8; taille]).unwrap().len())
            .collect();
        longueurs.sort_unstable();
        longueurs.dedup();
        assert_eq!(
            longueurs,
            vec![
                ClasseTaille::Octets256.taille_totale(),
                ClasseTaille::Octets1024.taille_totale(),
            ],
            "900 charges utiles de tailles différentes ne doivent produire que deux longueurs"
        );
    }

    /// Le contenu ne se lit pas dans les octets scellés.
    #[test]
    fn le_contenu_n_apparait_pas() {
        let env = enveloppe("e1");
        let requete = b"MATCH (n:Racine) RETURN n.cle_privee";
        let scellee = env.sceller(requete).unwrap();
        assert!(!contient(&scellee, requete));
        assert!(!contient(&scellee, b"MATCH"));
        assert!(!contient(&scellee, b"cle_privee"));
    }

    /// Une charge utile au-delà de la plus grande classe est refusée, avec la
    /// limite nommée.
    #[test]
    fn charge_trop_grande_refusee() {
        let env = enveloppe("e1");
        let trop = vec![0u8; ClasseTaille::maximum() + 1];
        match env.sceller(&trop) {
            Err(CoffreError::TropGrand { maximum, .. }) => {
                assert_eq!(maximum, ClasseTaille::maximum());
            }
            autre => panic!("attendu TropGrand, reçu {autre:?}"),
        }
    }

    /// Imposer une classe trop petite est refusé, pas tronqué en silence.
    #[test]
    fn classe_imposee_trop_petite_refusee() {
        let env = enveloppe("e1");
        assert!(matches!(
            env.sceller_dans(&vec![0u8; 300], ClasseTaille::Octets256),
            Err(CoffreError::TropGrand { .. })
        ));
    }

    /// **Anti-échange d'espace** : une enveloppe scellée pour un espace n'est
    /// pas descellée par la clé d'un autre, même si l'octet de classe correspond.
    #[test]
    fn enveloppe_d_un_autre_espace_refusee() {
        let scellee = enveloppe("e1").sceller(b"charge").unwrap();
        let mut faux = scellee.clone();
        faux[7..TETE_ENVELOPPE].copy_from_slice(EmpreinteEspace::calculer("e2").octets());

        assert!(matches!(
            enveloppe("e1").desceller(&faux),
            Err(CoffreError::EspaceIncoherent { .. })
        ));
    }

    /// Toute zone de l'en-tête et du chiffré est couverte.
    #[test]
    fn toute_zone_est_couverte() {
        let env = enveloppe("e1");
        let bonne = env.sceller(b"charge").unwrap();

        for (nom, position) in [
            ("magic", 0usize),
            ("version", 4),
            ("classe", 5),
            ("drapeau", 6),
            ("empreinte d'espace", 20),
            ("nonce", TETE_ENVELOPPE),
            ("premier octet de chiffré", TETE_ENVELOPPE + LONGUEUR_NONCE),
            ("dernier octet", bonne.len() - 1),
        ] {
            let mut faux = bonne.clone();
            faux[position] ^= 0x01;
            assert!(
                env.desceller(&faux).is_err(),
                "modifier « {nom} » doit être détecté"
            );
        }
    }

    /// Le **remplissage** est vérifié : il ne peut pas porter un second message.
    #[test]
    fn remplissage_non_nul_refuse() {
        let mut clair = vec![0u8; 16];
        clair[0..4].copy_from_slice(&4u32.to_le_bytes());
        clair[4..8].copy_from_slice(b"abcd");
        assert_eq!(depouiller(&clair).unwrap(), b"abcd");

        // Un octet caché dans les zéros de remplissage.
        clair[12] = 0x2a;
        assert!(matches!(depouiller(&clair), Err(CoffreError::Format(_))));
    }

    /// Une longueur inscrite au-delà de la classe est refusée.
    #[test]
    fn longueur_inscrite_incoherente_refusee() {
        let mut clair = vec![0u8; 16];
        clair[0..4].copy_from_slice(&1000u32.to_le_bytes());
        assert!(matches!(depouiller(&clair), Err(CoffreError::Format(_))));

        assert!(matches!(depouiller(&[0u8; 2]), Err(CoffreError::Format(_))));
    }

    /// Une enveloppe tronquée ou rallongée est refusée : la longueur est
    /// déterminée par la classe, sans jeu.
    #[test]
    fn longueur_stricte() {
        let env = enveloppe("e1");
        let bonne = env.sceller(b"charge").unwrap();

        assert!(env.desceller(&bonne[..bonne.len() - 1]).is_err());
        let mut rallongee = bonne.clone();
        rallongee.push(0);
        assert!(env.desceller(&rallongee).is_err());
        assert!(env.desceller(&bonne[..TETE_ENVELOPPE - 1]).is_err());
    }

    /// La classe se lit **sans la clé** — c'est ce que l'engine peut savoir, et
    /// rien de plus.
    #[test]
    fn la_classe_se_lit_sans_la_cle() {
        let scellee = enveloppe("secret").sceller(&vec![0u8; 5000]).unwrap();
        assert_eq!(
            ClasseTaille::lue(&scellee).unwrap(),
            ClasseTaille::Octets16384
        );
        assert_eq!(
            ClasseTaille::lue(&scellee).unwrap().capacite(),
            16_384,
            "le porteur apprend la classe, jamais la taille exacte"
        );

        let mut faux = scellee.clone();
        faux[5] = 99;
        assert!(matches!(
            ClasseTaille::lue(&faux),
            Err(CoffreError::Format(_))
        ));
    }

    /// Les classes sont ordonnées et sans trou : `pour` rend la plus petite qui
    /// contient.
    #[test]
    fn choix_de_classe() {
        assert_eq!(ClasseTaille::pour(1).unwrap(), ClasseTaille::Octets256);
        assert_eq!(ClasseTaille::pour(252).unwrap(), ClasseTaille::Octets256);
        assert_eq!(ClasseTaille::pour(253).unwrap(), ClasseTaille::Octets1024);
        assert_eq!(ClasseTaille::pour(1020).unwrap(), ClasseTaille::Octets1024);
        assert_eq!(ClasseTaille::pour(1021).unwrap(), ClasseTaille::Octets4096);
        assert_eq!(
            ClasseTaille::pour(ClasseTaille::maximum()).unwrap(),
            ClasseTaille::Octets65536
        );
        assert!(ClasseTaille::pour(ClasseTaille::maximum() + 1).is_err());
    }

    /// Une clé qui n'est pas la bonne ne descelle pas.
    #[test]
    fn mauvaise_cle_refusee() {
        let scellee = enveloppe("e1").sceller(b"charge").unwrap();
        assert!(matches!(
            enveloppe("e1").desceller(&scellee),
            Err(CoffreError::Authentification)
        ));
    }
}
