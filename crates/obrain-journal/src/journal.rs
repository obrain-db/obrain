//! Le journal — une suite d'entrées dont chacune cite l'empreinte de la
//! précédente, et que chaque client peut confronter aux reçus qu'il détient.
//!
//! ## Ce que la chaîne apporte, et ce qu'elle n'apporte pas
//!
//! Le chaînage seul rend **impossible d'oublier discrètement** une entrée :
//! retirer un maillon rompt toutes les empreintes suivantes, et la vérification
//! dit à quelle séquence. Mais un opérateur qui détient la clé de signature peut
//! **reconstruire** une autre histoire de bout en bout, chaîne valide et
//! signatures valides. Contre cela, le chaînage ne sert à rien, et la signature
//! seule non plus.
//!
//! Ce qui reste, et c'est le cœur de cette crate : **le reçu détenu par le
//! client**. L'opérateur ne peut pas produire deux entrées signées différentes
//! pour la même séquence sans que celui qui garde l'ancienne s'en aperçoive
//! ([`Journal::accorde_avec`]). La détection ne vient donc pas du journal, elle
//! vient de la **rencontre** entre le journal et les reçus — c'est-à-dire du
//! client, qui est précisément la partie à protéger.
//!
//! ## Ce qui n'est pas prouvé pour autant
//!
//! * **La complétude.** Un opérateur qui n'inscrit rien n'est pris par rien :
//!   un journal vide est parfaitement valide. Ce qui le prend, c'est le client
//!   qui **attend** un reçu — [`sequences_manquantes`] est là pour ça.
//! * **L'honnêteté du calcul.** Le journal prouve qu'une entrée existe à une
//!   place, pas que le calcul annoncé a été fait. Cette moitié demande la mesure
//!   de code attestée (étape 4 de l'ADR), pas un journal.
//! * **La confidentialité.** Le journal ne conserve que des empreintes, donc il
//!   ne dit rien du contenu — mais c'est une propriété du format, pas une preuve
//!   sur ce que l'appelant y met.

use crate::cle::CleJournal;
use crate::entree::{
    Entree, TAILLE_EMPREINTE, TAILLE_SIGNATURE, court, empreinte_du_journal, genese,
};
use crate::erreur::{ErreurJournal, Resultat};

/// Signature du format persisté.
const MAGIC: &[u8; 8] = b"OBJRNL01";

/// Taille d'une entrée sur le disque : tous les champs sont à taille fixe, donc
/// une entrée occupe toujours la même place et ne peut pas se décaler.
const TAILLE_ENTREE_DISQUE: usize = TAILLE_EMPREINTE + 8 + 8 + TAILLE_EMPREINTE + TAILLE_EMPREINTE + TAILLE_SIGNATURE;

/// Taille maximale d'un identifiant de journal, en octets.
pub const IDENTIFIANT_MAXIMUM: usize = 255;

/// Un journal de calcul : une suite d'entrées chaînées et signées.
pub struct Journal {
    identifiant: String,
    empreinte_identifiant: [u8; TAILLE_EMPREINTE],
    entrees: Vec<Entree>,
}

impl Journal {
    /// Ouvre un journal neuf, vide, dont la tête est sa genèse.
    ///
    /// # Erreurs
    ///
    /// [`ErreurJournal::Reference`] si l'identifiant est vide, ou plus long que
    /// [`IDENTIFIANT_MAXIMUM`] — refusé plutôt que tronqué, parce qu'un
    /// identifiant tronqué désignerait un autre journal.
    pub fn neuf(identifiant: &str) -> Resultat<Self> {
        let nom = identifiant.trim();
        if nom.is_empty() {
            return Err(ErreurJournal::Reference(
                "l'identifiant d'un journal ne peut pas être vide".to_owned(),
            ));
        }
        if nom.len() > IDENTIFIANT_MAXIMUM {
            return Err(ErreurJournal::Reference(format!(
                "identifiant de {} octets, maximum {IDENTIFIANT_MAXIMUM}",
                nom.len()
            )));
        }
        Ok(Self {
            identifiant: nom.to_owned(),
            empreinte_identifiant: empreinte_du_journal(nom),
            entrees: Vec::new(),
        })
    }

    /// L'identifiant du journal.
    #[must_use]
    pub fn identifiant(&self) -> &str {
        &self.identifiant
    }

    /// L'empreinte du journal, qui lie toutes ses entrées.
    #[must_use]
    pub fn empreinte(&self) -> [u8; TAILLE_EMPREINTE] {
        self.empreinte_identifiant
    }

    /// Le nombre d'entrées.
    #[must_use]
    pub fn longueur(&self) -> usize {
        self.entrees.len()
    }

    /// Les entrées, dans l'ordre.
    #[must_use]
    pub fn entrees(&self) -> &[Entree] {
        &self.entrees
    }

    /// L'entrée d'une séquence, si elle existe.
    #[must_use]
    pub fn entree(&self, sequence: u64) -> Option<&Entree> {
        usize::try_from(sequence)
            .ok()
            .and_then(|i| self.entrees.get(i))
    }

    /// La tête du journal : la genèse s'il est vide, l'empreinte de la dernière
    /// entrée sinon.
    ///
    /// **Recalculée** à chaque appel, jamais lue dans un champ mis en cache : une
    /// tête en cache pourrait survivre à une corruption des entrées, et une
    /// vérification qui compare un cache à lui-même ne vérifie rien.
    #[must_use]
    pub fn tete(&self) -> [u8; TAILLE_EMPREINTE] {
        self.entrees
            .last()
            .map_or_else(|| genese(&self.empreinte_identifiant), Entree::empreinte)
    }

    /// Inscrit un calcul, et rend le reçu que le client emportera.
    ///
    /// La chaîne est contrôlée **avant** d'écrire : inscrire une entrée par-dessus
    /// une queue déjà rompue légitimerait la suivante, et rendrait la rupture
    /// indétectable au premier coup d'œil.
    ///
    /// # Erreurs
    ///
    /// [`ErreurJournal::ChaineRompee`] ou [`ErreurJournal::JournalEtranger`] si
    /// la queue du journal ne se tient plus.
    pub fn recevoir(
        &mut self,
        cle: &CleJournal,
        quoi: [u8; TAILLE_EMPREINTE],
        horodatage: u64,
    ) -> Resultat<Entree> {
        self.controler_la_queue()?;

        let sequence = u64::try_from(self.entrees.len()).map_err(|_| {
            ErreurJournal::Reference("journal trop long pour être numéroté".to_owned())
        })?;
        let precedent = self.tete();

        // L'empreinte d'abord, la signature ensuite : c'est la signature *de
        // l'empreinte* qui rend le reçu opposable.
        let provisoire = Entree::neuve(
            self.empreinte_identifiant,
            sequence,
            horodatage,
            quoi,
            precedent,
            [0u8; TAILLE_SIGNATURE],
        );
        let signature = cle.signer(&provisoire.empreinte());
        let entree = Entree::neuve(
            self.empreinte_identifiant,
            sequence,
            horodatage,
            quoi,
            precedent,
            signature,
        );

        self.entrees.push(entree.clone());
        Ok(entree)
    }

    /// Vérifie la chaîne entière, et rend la séquence de la première rupture.
    ///
    /// # Erreurs
    ///
    /// [`ErreurJournal::ChaineRompee`] si une entrée ne cite pas la précédente,
    /// si les séquences ne se suivent pas, ou si une entrée vient d'un autre
    /// journal. [`ErreurJournal::JournalEtranger`] dans ce dernier cas.
    pub fn verifier(&self) -> Resultat<()> {
        let mut precedent = genese(&self.empreinte_identifiant);
        for (position, entree) in self.entrees.iter().enumerate() {
            let sequence = u64::try_from(position).unwrap_or(u64::MAX);
            if entree.sequence != sequence {
                return Err(ErreurJournal::ChaineRompee {
                    sequence,
                    motif: format!(
                        "l'entrée annonce la séquence {} pour la position {sequence}",
                        entree.sequence
                    ),
                });
            }
            if entree.journal != self.empreinte_identifiant {
                return Err(ErreurJournal::JournalEtranger(court(&entree.journal)));
            }
            if entree.precedent != precedent {
                // Le point de rupture est **cette** entrée : c'est elle qui ne
                // cite plus ce que la chaîne attend. Quand l'entrée précédente a
                // été modifiée, la rupture se signale donc **une séquence plus
                // loin** que l'entrée touchée — le message nomme les deux, pour
                // qu'un humain n'ait pas à refaire le raisonnement.
                return Err(ErreurJournal::ChaineRompee {
                    sequence,
                    motif: format!(
                        "le maillon cité ({}) n'est pas l'empreinte de l'entrée {} ({})",
                        court(&entree.precedent),
                        sequence.saturating_sub(1),
                        court(&precedent)
                    ),
                });
            }
            precedent = entree.empreinte();
        }
        Ok(())
    }

    /// Vérifie la chaîne **et** que la tête est bien celle qu'un client avait
    /// souvenue.
    ///
    /// C'est le contrôle qui attrape la troncature : un journal auquel on a
    /// retiré ses dernières entrées se vérifie parfaitement tout seul — sa
    /// chaîne est intacte, elle est simplement plus courte. Seule une tête
    /// conservée ailleurs le démasque.
    ///
    /// # Erreurs
    ///
    /// [`ErreurJournal::TeteDifferente`] si la tête lue n'est pas celle attendue,
    /// plus les erreurs de [`Journal::verifier`].
    pub fn verifier_jusqua(&self, tete_attendue: [u8; TAILLE_EMPREINTE]) -> Resultat<()> {
        self.verifier()?;
        let lue = self.tete();
        if lue != tete_attendue {
            return Err(ErreurJournal::TeteDifferente {
                attendue: court(&tete_attendue),
                lue: court(&lue),
            });
        }
        Ok(())
    }

    /// Vérifie la chaîne **et** toutes les signatures contre une clé publique.
    ///
    /// La clé publique doit venir d'ailleurs que du journal : une clé livrée avec
    /// l'histoire qu'elle authentifie ne prouve rien, puisqu'elle serait réécrite
    /// avec elle.
    ///
    /// # Erreurs
    ///
    /// [`ErreurJournal::SignatureInvalide`], plus les erreurs de
    /// [`Journal::verifier`].
    pub fn verifier_signatures(&self, publique: &[u8; 32]) -> Resultat<()> {
        self.verifier()?;
        for entree in &self.entrees {
            entree.verifier_signature(publique)?;
        }
        Ok(())
    }

    /// Confronte un reçu détenu par un client au journal d'aujourd'hui.
    ///
    /// **C'est le contrôle qui compte.** Il attrape ce que ni la chaîne ni les
    /// signatures ne voient : un opérateur qui a reconstruit une autre histoire
    /// de bout en bout, chaîne valide et signatures valides, mais dont la
    /// séquence citée par le client ne dit plus la même chose.
    ///
    /// # Erreurs
    ///
    /// * [`ErreurJournal::JournalEtranger`] si le reçu vient d'un autre journal ;
    /// * [`ErreurJournal::SequenceAbsente`] si le journal est plus court que le
    ///   reçu ;
    /// * [`ErreurJournal::ElementDifferent`] si l'entrée de cette séquence a
    ///   changé — c'est-à-dire si l'histoire a été réécrite.
    pub fn accorde_avec(&self, recu: &Entree) -> Resultat<()> {
        if recu.journal != self.empreinte_identifiant {
            return Err(ErreurJournal::JournalEtranger(court(&recu.journal)));
        }
        let entree = self
            .entree(recu.sequence)
            .ok_or(ErreurJournal::SequenceAbsente {
                sequence: recu.sequence,
            })?;
        if entree.empreinte() != recu.empreinte() {
            return Err(ErreurJournal::ElementDifferent {
                sequence: recu.sequence,
            });
        }
        Ok(())
    }

    /// Le contrôle de queue, en temps constant : ce qui précède immédiatement
    /// une écriture.
    fn controler_la_queue(&self) -> Resultat<()> {
        let Some(derniere) = self.entrees.last() else {
            return Ok(());
        };
        let position = u64::try_from(self.entrees.len() - 1).unwrap_or(u64::MAX);
        if derniere.sequence != position {
            return Err(ErreurJournal::ChaineRompee {
                sequence: position,
                motif: "la dernière entrée n'est pas à sa place".to_owned(),
            });
        }
        if derniere.journal != self.empreinte_identifiant {
            return Err(ErreurJournal::JournalEtranger(court(&derniere.journal)));
        }
        let attendu = if self.entrees.len() == 1 {
            genese(&self.empreinte_identifiant)
        } else {
            self.entrees[self.entrees.len() - 2].empreinte()
        };
        if derniere.precedent != attendu {
            return Err(ErreurJournal::ChaineRompee {
                sequence: position,
                motif: "la queue du journal ne cite plus le maillon précédent".to_owned(),
            });
        }
        Ok(())
    }

    // ── PERSISTANCE ─────────────────────────────────────────────────────────

    /// Sérialise le journal : en-tête, puis les entrées à taille fixe.
    #[must_use]
    pub fn vers_octets(&self) -> Vec<u8> {
        let nom = self.identifiant.as_bytes();
        let mut sortie =
            Vec::with_capacity(8 + 2 + nom.len() + 8 + self.entrees.len() * TAILLE_ENTREE_DISQUE);
        sortie.extend_from_slice(MAGIC);
        // L'identifiant tient sur deux octets, et sa longueur est bornée à
        // l'ouverture : la troncature silencieuse est impossible.
        sortie.extend_from_slice(&u16::try_from(nom.len()).unwrap_or(u16::MAX).to_le_bytes());
        sortie.extend_from_slice(nom);
        sortie.extend_from_slice(
            &u64::try_from(self.entrees.len())
                .unwrap_or(u64::MAX)
                .to_le_bytes(),
        );
        for entree in &self.entrees {
            sortie.extend_from_slice(&entree.journal);
            sortie.extend_from_slice(&entree.sequence.to_le_bytes());
            sortie.extend_from_slice(&entree.horodatage.to_le_bytes());
            sortie.extend_from_slice(&entree.quoi);
            sortie.extend_from_slice(&entree.precedent);
            sortie.extend_from_slice(&entree.signature);
        }
        sortie
    }

    /// Relit un journal sérialisé, **et vérifie sa chaîne**.
    ///
    /// La chaîne est vérifiée au chargement, pas plus tard : charger un journal
    /// rompu pour le découvrir à la première écriture reviendrait à écrire
    /// par-dessus. Les signatures, elles, demandent la clé publique : voir
    /// [`Journal::verifier_signatures`].
    ///
    /// # Erreurs
    ///
    /// [`ErreurJournal::Reference`] si les octets ne sont pas un journal (magic,
    /// longueurs incohérentes), plus les erreurs de [`Journal::verifier`].
    pub fn depuis_octets(octets: &[u8]) -> Resultat<Self> {
        let trop_court = || {
            ErreurJournal::Reference(format!(
                "journal de {} octets : plus court que son en-tête",
                octets.len()
            ))
        };

        if octets.get(..8) != Some(&MAGIC[..]) {
            return Err(ErreurJournal::Reference(
                "signature de format de journal absente ou inconnue".to_owned(),
            ));
        }
        let longueur_nom = usize::from(
            u16::from_le_bytes(
                octets
                    .get(8..10)
                    .ok_or_else(trop_court)?
                    .try_into()
                    .map_err(|_| trop_court())?,
            ),
        );
        let fin_nom = 10usize
            .checked_add(longueur_nom)
            .ok_or_else(trop_court)?;
        let nom = octets.get(10..fin_nom).ok_or_else(trop_court)?;
        let nom = core::str::from_utf8(nom)
            .map_err(|_| ErreurJournal::Reference("identifiant de journal non UTF-8".to_owned()))?;

        let fin_nombre = fin_nom.checked_add(8).ok_or_else(trop_court)?;
        let nb_entrees = u64::from_le_bytes(
            octets
                .get(fin_nom..fin_nombre)
                .ok_or_else(trop_court)?
                .try_into()
                .map_err(|_| trop_court())?,
        );

        // La taille annoncée doit correspondre **exactement** à ce qui est reçu :
        // c'est ainsi qu'une troncature est attrapée avant toute allocation.
        let attendu = u64::try_from(TAILLE_ENTREE_DISQUE)
            .unwrap_or(u64::MAX)
            .checked_mul(nb_entrees)
            .and_then(|t| t.checked_add(u64::try_from(fin_nombre).unwrap_or(u64::MAX)))
            .ok_or_else(|| {
                ErreurJournal::Reference("taille de journal annoncée délirante".to_owned())
            })?;
        if attendu != u64::try_from(octets.len()).unwrap_or(u64::MAX) {
            return Err(ErreurJournal::Reference(format!(
                "journal de {} octets, {attendu} attendus d'après son en-tête",
                octets.len()
            )));
        }

        let mut journal = Self::neuf(nom)?;
        let mut curseur = fin_nombre;
        for _ in 0..nb_entrees {
            let bloc = octets
                .get(curseur..curseur + TAILLE_ENTREE_DISQUE)
                .ok_or_else(trop_court)?;
            journal.entrees.push(lire_entree(bloc)?);
            curseur += TAILLE_ENTREE_DISQUE;
        }

        // Un journal rompu ne se charge pas.
        journal.verifier()?;
        Ok(journal)
    }
}

impl core::fmt::Debug for Journal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Journal")
            .field("identifiant", &self.identifiant)
            .field("entrees", &self.entrees.len())
            .field("tete", &court(&self.tete()))
            .finish()
    }
}

/// Lit une fenêtre bornée d'un bloc, ou refuse : aucune lecture d'octets reçus
/// n'a lieu sans contrôle de longueur.
fn borne(bloc: &[u8], debut: usize, taille: usize) -> Resultat<&[u8]> {
    bloc.get(debut..debut + taille)
        .ok_or_else(|| ErreurJournal::Reference(format!("entrée tronquée avant {debut}")))
}

/// Relit une entrée à taille fixe. Les champs se suivent sans ambiguïté, et
/// aucune lecture n'est faite sans que la fenêtre existe.
fn lire_entree(bloc: &[u8]) -> Resultat<Entree> {
    let journal = borne(bloc, 0, TAILLE_EMPREINTE)?
        .try_into()
        .map_err(|_| ErreurJournal::Reference("empreinte de journal illisible".to_owned()))?;

    let sequence: [u8; 8] = borne(bloc, TAILLE_EMPREINTE, 8)?
        .try_into()
        .map_err(|_| ErreurJournal::Reference("séquence illisible".to_owned()))?;
    let horodatage: [u8; 8] = borne(bloc, TAILLE_EMPREINTE + 8, 8)?
        .try_into()
        .map_err(|_| ErreurJournal::Reference("horodatage illisible".to_owned()))?;

    let quoi = borne(bloc, TAILLE_EMPREINTE + 16, TAILLE_EMPREINTE)?
        .try_into()
        .map_err(|_| ErreurJournal::Reference("charge illisible".to_owned()))?;
    let precedent = borne(bloc, TAILLE_EMPREINTE * 2 + 16, TAILLE_EMPREINTE)?
        .try_into()
        .map_err(|_| ErreurJournal::Reference("maillon précédent illisible".to_owned()))?;
    let signature = borne(bloc, TAILLE_EMPREINTE * 3 + 16, TAILLE_SIGNATURE)?
        .try_into()
        .map_err(|_| ErreurJournal::Reference("signature illisible".to_owned()))?;

    Ok(Entree::neuve(
        journal,
        u64::from_le_bytes(sequence),
        u64::from_le_bytes(horodatage),
        quoi,
        precedent,
        signature,
    ))
}

/// Les séquences manquantes dans un ensemble de reçus détenus.
///
/// **Le contrôle de complétude, et il est côté client.** Un opérateur qui
/// n'inscrit pas un calcul n'est attrapé par aucune chaîne — un journal est
/// toujours cohérent avec lui-même. Ce qui le prend, c'est un client qui sait
/// combien de calculs il a lancés et qui constate un trou dans la suite des
/// séquences qu'on lui a rendues.
///
/// La borne basse est la plus petite séquence détenue : ce qui précède le premier
/// reçu n'est pas « manquant », c'est antérieur à ce que le client a conservé.
#[must_use]
pub fn sequences_manquantes(recus: &[Entree]) -> Vec<u64> {
    let mut sequences: Vec<u64> = recus.iter().map(|r| r.sequence).collect();
    sequences.sort_unstable();
    sequences.dedup();

    let Some((&premier, &dernier)) = sequences.first().zip(sequences.last()) else {
        return Vec::new();
    };

    let mut manquantes = Vec::new();
    let mut attendue = premier;
    while attendue <= dernier {
        if sequences.binary_search(&attendue).is_err() {
            manquantes.push(attendue);
        }
        attendue += 1;
    }
    manquantes
}
