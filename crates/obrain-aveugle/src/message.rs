//! Le message — ce qui voyage dans une enveloppe à taille fixe.
//!
//! ## Ce que le porteur voit, et ce qu'il ne voit pas
//!
//! Le porteur reçoit des **octets de taille fixe** : une enveloppe scellée par
//! l'expéditeur, sous une clé que le porteur n'a pas. Tout ce module décrit ce
//! qui est *à l'intérieur*, donc tout ce que le porteur ne peut pas lire.
//!
//! ## La destination est **dans** le message, pas seulement sur l'enveloppe
//!
//! C'est la décision la plus importante de ce module, et elle mérite d'être
//! justifiée. Le porteur doit savoir où livrer : l'adresse du destinataire est
//! donc nécessairement **dehors**, visible. Rien n'empêche alors le porteur de
//! réétiqueter une enveloppe et de la livrer ailleurs.
//!
//! On ne peut pas l'en empêcher — il tient les octets. On peut le **rendre
//! détectable** : la destination est aussi scellée **dedans**, et
//! [`ouvrir`] refuse un message qui ne lui était pas destiné. Un détournement
//! coûte donc au porteur la seule chose qu'il ne peut pas rendre : la
//! livraison. Le destinataire réel ne reçoit rien, et celui qui reçoit le sait.
//!
//! **Détection, pas empêchement** — et c'est écrit ici pour que personne ne
//! vende l'inverse.

use obrain_coffre::cle::CleEspace;
use obrain_coffre::{ClasseTaille, Enveloppe};

use crate::adresse::{Adresse, TAILLE_ADRESSE};
use crate::{ErreurAveugle, Resultat};

/// Domaine de la charge utile d'un message.
const DOMAINE_MESSAGE: &[u8] = b"obrain-aveugle/v1/message";

/// Ce qu'une extrémité met dans une enveloppe.
///
/// Tous ces champs sont **scellés** : le porteur n'en voit aucun. C'est ici que
/// vivent la requête, le résultat, et le jeton de capacité qui borne l'action.
#[derive(Clone, PartialEq, Eq)]
pub struct Charge {
    /// Qui envoie — vérifié par le destinataire, pas par le porteur.
    pub expediteur: Adresse,
    /// À qui c'est destiné — le porteur ne le lit que dehors, en clair, pour
    /// livrer ; ici, c'est le contrôle qui rend un détournement détectable.
    pub destinataire: Adresse,
    /// Le contenu : une requête, un résultat, une mesure. **Jamais lu par le
    /// porteur.**
    pub quoi: Vec<u8>,
    /// Un jeton de capacité, s'il y en a un — opaque pour le porteur, vérifié
    /// par la seule partie qui détient la clé de l'espace.
    pub jeton: Option<Vec<u8>>,
}

impl Charge {
    /// Construit une charge sans jeton.
    #[must_use]
    pub fn nouvelle(expediteur: Adresse, destinataire: Adresse, quoi: Vec<u8>) -> Self {
        Self {
            expediteur,
            destinataire,
            quoi,
            jeton: None,
        }
    }

    /// La forme canonique de la charge : chaque champ préfixé par sa longueur.
    ///
    /// Sans les préfixes, deux charges différentes pourraient s'écrire des
    /// mêmes octets — `quoi = "ab"` avec un jeton `"c"` et `quoi = "a"` avec un
    /// jeton `"bc"` — et l'ambiguïté se paierait au déchiffrement, c'est-à-dire
    /// trop tard.
    #[must_use]
    pub fn vers_octets(&self) -> Vec<u8> {
        let mut sortie = Vec::with_capacity(DOMAINE_MESSAGE.len() + 64 + self.quoi.len());
        sortie.extend_from_slice(DOMAINE_MESSAGE);
        sortie.extend_from_slice(self.expediteur.octets());
        sortie.extend_from_slice(self.destinataire.octets());
        pousser(&mut sortie, &self.quoi);
        match &self.jeton {
            Some(jeton) => pousser(&mut sortie, jeton),
            // Un jeton absent et un jeton vide ne doivent pas se confondre : le
            // premier n'autorise rien, le second est un jeton de zéro octet.
            None => sortie.extend_from_slice(&u32::MAX.to_le_bytes()),
        }
        sortie
    }

    /// Relit une charge. Ne panique **jamais** sur des octets hostiles : tout
    /// est borné, et ce qui dépasse est refusé.
    ///
    /// # Erreurs
    ///
    /// [`ErreurAveugle::MessageIllisible`] si les octets ne sont pas une charge.
    pub fn depuis_octets(octets: &[u8]) -> Resultat<Self> {
        let illisible = |quoi: &str| ErreurAveugle::MessageIllisible(quoi.to_owned());

        let reste = octets
            .strip_prefix(DOMAINE_MESSAGE)
            .ok_or_else(|| illisible("domaine de message absent"))?;
        let (expediteur, reste) = couper(reste, TAILLE_ADRESSE).ok_or_else(|| illisible("expéditeur"))?;
        let (destinataire, reste) = couper(reste, TAILLE_ADRESSE).ok_or_else(|| illisible("destinataire"))?;
        let (quoi, reste) = lire_champ(reste).ok_or_else(|| illisible("charge"))?;
        let quoi = quoi.ok_or_else(|| illisible("charge absente"))?;
        let (jeton, reste) = lire_champ(reste).ok_or_else(|| illisible("jeton"))?;

        if !reste.is_empty() {
            return Err(illisible("octets en trop après la charge"));
        }

        Ok(Self {
            expediteur: Adresse::depuis_octets(expediteur.try_into().map_err(|_| illisible("expéditeur"))?),
            destinataire: Adresse::depuis_octets(
                destinataire.try_into().map_err(|_| illisible("destinataire"))?,
            ),
            quoi: quoi.to_vec(),
            jeton: jeton.map(<[u8]>::to_vec),
        })
    }

    /// La classe d'enveloppe la plus petite qui contient cette charge.
    ///
    /// # Erreurs
    ///
    /// [`ErreurAveugle::TropGrand`] au-delà de la dernière classe — refusé, et
    /// jamais tronqué.
    pub fn classe(&self) -> Resultat<ClasseTaille> {
        ClasseTaille::pour(self.vers_octets().len()).map_err(|_| ErreurAveugle::TropGrand {
            taille: self.vers_octets().len(),
        })
    }
}

impl core::fmt::Debug for Charge {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Charge")
            .field("expediteur", &self.expediteur)
            .field("destinataire", &self.destinataire)
            .field("quoi", &format!("{} octets", self.quoi.len()))
            .field(
                "jeton",
                &self.jeton.as_ref().map_or("aucun".to_owned(), |j| {
                    format!("{} octets", j.len())
                }),
            )
            .finish()
    }
}

/// Scelle une charge dans une enveloppe **à taille fixe**.
///
/// La classe est choisie automatiquement : deux charges de tailles voisines
/// tombent dans la même classe, et le porteur ne peut donc pas les distinguer
/// par la longueur. C'est le critère G3 appliqué au transport.
///
/// # Erreurs
///
/// * [`ErreurAveugle::TropGrand`] si la charge dépasse la dernière classe ;
/// * [`ErreurAveugle::Coffre`] si le scellement échoue.
pub fn sceller(cle: &CleEspace, charge: &Charge) -> Resultat<Vec<u8>> {
    let classe = charge.classe()?;
    let enveloppe = Enveloppe::neuf(cle).map_err(ErreurAveugle::Coffre)?;
    enveloppe
        .sceller_dans(&charge.vers_octets(), classe)
        .map_err(ErreurAveugle::Coffre)
}

/// Descelle une enveloppe et rend sa charge, **en refusant ce qui n'était pas
/// destiné à `moi`**.
///
/// C'est le contrôle qui rend un détournement détectable : le porteur peut
/// réétiqueter une enveloppe, il ne peut pas changer ce qui est scellé dedans.
///
/// # Erreurs
///
/// * [`ErreurAveugle::Detourne`] si la destination scellée n'est pas `moi` ;
/// * [`ErreurAveugle::Coffre`] si l'authentification échoue (mauvaise clé,
///   altération) ;
/// * [`ErreurAveugle::MessageIllisible`] si la charge est malformée.
pub fn ouvrir(cle: &CleEspace, moi: &Adresse, octets: &[u8]) -> Resultat<Charge> {
    let enveloppe = Enveloppe::neuf(cle).map_err(ErreurAveugle::Coffre)?;
    let clair = enveloppe.desceller(octets).map_err(ErreurAveugle::Coffre)?;
    let charge = Charge::depuis_octets(&clair)?;
    if &charge.destinataire != moi {
        return Err(ErreurAveugle::Detourne {
            attendu: format!("{moi:?}"),
            scelle: format!("{:?}", charge.destinataire),
        });
    }
    Ok(charge)
}

/// La longueur d'une enveloppe est-elle une longueur **du protocole** ?
///
/// Le porteur n'a pas de clé, mais il peut exiger que ce qu'on lui confie ait
/// l'une des formes prévues. C'est la seule règle qu'il puisse faire respecter
/// sans rien savoir — et elle lui évite de devenir un canal où la taille
/// elle-même raconte le contenu.
#[must_use]
pub fn taille_conforme(octets: &[u8]) -> bool {
    ClasseTaille::TOUTES
        .into_iter()
        .any(|classe| classe.taille_totale() == octets.len())
}

fn pousser(sortie: &mut Vec<u8>, champ: &[u8]) {
    let longueur = u32::try_from(champ.len()).unwrap_or(u32::MAX);
    sortie.extend_from_slice(&longueur.to_le_bytes());
    sortie.extend_from_slice(champ);
}

fn couper(octets: &[u8], n: usize) -> Option<(&[u8], &[u8])> {
    if octets.len() < n {
        return None;
    }
    octets.split_at(n).into()
}

fn lire_champ(octets: &[u8]) -> Option<(Option<&[u8]>, &[u8])> {
    let (longueur, reste) = couper(octets, 4)?;
    let longueur = u32::from_le_bytes(longueur.try_into().ok()?);
    if longueur == u32::MAX {
        return Some((None, reste));
    }
    let longueur = usize::try_from(longueur).ok()?;
    let (champ, reste) = couper(reste, longueur)?;
    Some((Some(champ), reste))
}
