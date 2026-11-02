//! Le porteur — celui qui livre sans lire, et qui ne peut pas renier.
//!
//! ## Ce qu'il détient, et ce qu'il ne détient pas
//!
//! Il détient : des adresses, des enveloppes scellées, des horodatages, et une
//! clé de **signature** pour son propre registre de transitions. Il ne détient
//! **aucune** clé d'espace : c'est la propriété structurelle de cette crate, et
//! elle n'est pas une convention — il n'existe pas de champ, pas d'argument, pas
//! de méthode qui lui donne accès à une [`CleEspace`](obrain_coffre::CleEspace).
//!
//! ## Ce qu'il fait respecter sans rien savoir
//!
//! Un porteur aveugle n'est pas un porteur passif. Sans jamais lire un contenu,
//! il peut — et doit — faire respecter trois règles :
//!
//! 1. **la forme** : seules les tailles du protocole sont acceptées, sinon la
//!    longueur des dépôts raconterait le contenu ;
//! 2. **le non-rejeu** : la même enveloppe deux fois est refusée, sinon dupliquer
//!    des octets ferait exécuter deux fois un calcul ;
//! 3. **le quota** : une boîte pleine refuse, elle ne grandit pas — un porteur
//!    qui accumule sans borne devient un amplificateur de déni de service.
//!
//! ## Ce qu'il journalise, et pourquoi
//!
//! Chaque transition (expéditeur, destinataire, taille, instant) est inscrite
//! dans un journal **chaîné et signé**, et chaque dépôt rend un reçu. Le porteur
//! ne peut donc pas renier un routage : celui qui détient le reçu le lui
//! opposera. Il peut reconstruire son registre et le resigner — il en a la clé —
//! mais le reçu détenu par l'extrémité ne s'accordera plus, exactement comme pour
//! l'audit chaîné.

use std::collections::{HashMap, HashSet, VecDeque};

use obrain_journal::{CleJournal, Entree, Journal, empreinte_de};

use crate::adresse::Adresse;
use crate::message::taille_conforme;
use crate::{ErreurAveugle, Resultat};

/// Ce que le porteur peut légitimement dire d'un échange.
///
/// **Aucun champ ne vient du contenu scellé** : c'est la définition de
/// l'aveuglement, et le test `ce_que_le_porteur_voit_ne_contient_aucun_secret`
/// la mesure plutôt que de l'affirmer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CeQueJaiVu {
    /// Qui a déposé.
    pub expediteur: Adresse,
    /// À qui c'était adressé, **tel qu'inscrit sur l'enveloppe** — donc ce que
    /// le porteur croit, pas ce qui est scellé dedans.
    pub destinataire: Adresse,
    /// La taille des octets transportés.
    pub taille: usize,
    /// L'instant du dépôt, déclaré par l'appelant.
    pub quand: u64,
}

impl CeQueJaiVu {
    /// La forme canonique : ce qui entre dans l'empreinte journalisée.
    #[must_use]
    pub fn vers_octets(&self) -> Vec<u8> {
        let mut sortie = Vec::with_capacity(48);
        sortie.extend_from_slice(b"obrain-aveugle/v1/transition");
        sortie.extend_from_slice(self.expediteur.octets());
        sortie.extend_from_slice(self.destinataire.octets());
        sortie.extend_from_slice(&u64::try_from(self.taille).unwrap_or(u64::MAX).to_le_bytes());
        sortie.extend_from_slice(&self.quand.to_le_bytes());
        sortie
    }

    /// L'empreinte de cette transition — ce qui est inscrit au journal.
    #[must_use]
    pub fn empreinte(&self) -> [u8; obrain_journal::TAILLE_EMPREINTE] {
        empreinte_de(&self.vers_octets())
    }
}

/// Un objet en attente de retrait.
#[derive(Clone, PartialEq, Eq)]
pub struct EnveloppeTransportee {
    /// L'adresse de l'expéditeur, telle qu'inscrite sur l'enveloppe.
    pub expediteur: Adresse,
    /// Les octets scellés, **intacts** : le porteur ne les modifie jamais, sans
    /// quoi l'authentification échouerait chez le destinataire.
    pub octets: Vec<u8>,
}

impl core::fmt::Debug for EnveloppeTransportee {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EnveloppeTransportee")
            .field("expediteur", &self.expediteur)
            .field("octets", &format!("{} octets scellés", self.octets.len()))
            .finish()
    }
}

/// Le porteur aveugle : il livre, compte, refuse, et journalise.
pub struct Porteur {
    cle: CleJournal,
    journal: Journal,
    connus: HashSet<Adresse>,
    boites: HashMap<Adresse, VecDeque<EnveloppeTransportee>>,
    deja_vues: HashSet<[u8; obrain_journal::TAILLE_EMPREINTE]>,
    vues: Vec<CeQueJaiVu>,
    quota: usize,
}

impl Porteur {
    /// Ouvre un porteur, avec son quota de boîte.
    ///
    /// # Erreurs
    ///
    /// [`ErreurAveugle::Journal`] si l'identifiant du registre est refusé.
    pub fn neuf(identifiant: &str, cle: CleJournal, quota: usize) -> Resultat<Self> {
        Ok(Self {
            journal: Journal::neuf(identifiant)?,
            cle,
            connus: HashSet::new(),
            boites: HashMap::new(),
            deja_vues: HashSet::new(),
            vues: Vec::new(),
            quota,
        })
    }

    /// Déclare une extrémité. Le porteur n'accepte de dépôt que pour une
    /// destination qu'il connaît : livrer à l'inconnu serait un puits sans fond.
    pub fn enregistrer(&mut self, adresse: Adresse) {
        self.connus.insert(adresse);
        self.boites.entry(adresse).or_default();
    }

    /// Dépose une enveloppe scellée à destination d'une extrémité, et rend le
    /// reçu de la transition.
    ///
    /// C'est **le seul chemin d'entrée** : le porteur ne sait pas fabriquer une
    /// enveloppe (il n'a pas de clé), il ne peut que la transporter.
    ///
    /// # Erreurs
    ///
    /// * [`ErreurAveugle::TailleNonConforme`] si les octets n'ont pas la taille
    ///   d'une classe du protocole ;
    /// * [`ErreurAveugle::DestinataireInconnu`] si l'extrémité n'est pas
    ///   déclarée ;
    /// * [`ErreurAveugle::Rejeu`] si cette enveloppe a déjà été déposée ;
    /// * [`ErreurAveugle::BoitePleine`] si la boîte du destinataire est pleine.
    pub fn deposer(
        &mut self,
        expediteur: Adresse,
        destinataire: Adresse,
        octets: Vec<u8>,
        quand: u64,
    ) -> Resultat<Entree> {
        if !taille_conforme(&octets) {
            return Err(ErreurAveugle::TailleNonConforme {
                taille: octets.len(),
            });
        }
        if !self.connus.contains(&destinataire) {
            return Err(ErreurAveugle::DestinataireInconnu(format!(
                "{destinataire:?}"
            )));
        }

        let empreinte = empreinte_de(&octets);
        if !self.deja_vues.insert(empreinte) {
            let mut court = hex::encode(empreinte);
            court.truncate(12);
            return Err(ErreurAveugle::Rejeu {
                empreinte: format!("{court}…"),
            });
        }

        let vue = CeQueJaiVu {
            expediteur,
            destinataire,
            taille: octets.len(),
            quand,
        };

        let boite = self.boites.entry(destinataire).or_default();
        if boite.len() >= self.quota {
            return Err(ErreurAveugle::BoitePleine {
                destinataire: format!("{destinataire:?}"),
                quota: self.quota,
            });
        }

        boite.push_back(EnveloppeTransportee { expediteur, octets });
        self.vues.push(vue);

        // La transition est inscrite **après** que la livraison est certaine :
        // journaliser un routage qui n'a pas eu lieu ferait de ce registre une
        // fiction signée.
        let recu = self.journal.recevoir(&self.cle, vue.empreinte(), quand)?;
        Ok(recu)
    }

    /// Retire l'enveloppe la plus ancienne destinée à cette extrémité.
    #[must_use]
    pub fn retirer(&mut self, destinataire: &Adresse) -> Option<EnveloppeTransportee> {
        self.boites.get_mut(destinataire)?.pop_front()
    }

    /// Ce que le porteur peut dire de son trafic : des adresses, des tailles, des
    /// instants. **Rien du contenu.**
    #[must_use]
    pub fn ce_que_jai_vu(&self) -> &[CeQueJaiVu] {
        &self.vues
    }

    /// Le nombre d'objets en attente pour une extrémité.
    #[must_use]
    pub fn en_attente(&self, destinataire: &Adresse) -> usize {
        self.boites.get(destinataire).map_or(0, VecDeque::len)
    }

    /// La tête du registre des transitions — ce qu'une extrémité peut conserver
    /// pour détecter une réécriture ultérieure.
    #[must_use]
    pub fn tete(&self) -> [u8; obrain_journal::TAILLE_EMPREINTE] {
        self.journal.tete()
    }

    /// Vérifie le registre des transitions contre la clé publique du porteur.
    ///
    /// # Erreurs
    ///
    /// Celles de [`Journal::verifier_signatures`].
    pub fn verifier(&self, publique: &[u8; 32]) -> Resultat<()> {
        self.journal
            .verifier_signatures(publique)
            .map_err(Into::into)
    }

    /// Le registre lui-même, pour le persister ou le confronter.
    #[must_use]
    pub fn journal(&self) -> &Journal {
        &self.journal
    }
}

impl core::fmt::Debug for Porteur {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Porteur")
            .field("extrémités connues", &self.connus.len())
            .field("transitions vues", &self.vues.len())
            .field("journal", &self.journal)
            .finish()
    }
}
