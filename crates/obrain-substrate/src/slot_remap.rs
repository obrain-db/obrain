//! # SlotRemappable — enrôlement des side-cars slot-indexés dans les
//! repermutations de slots nœuds (chantier remap, 2026-08).
//!
//! ## Pourquoi ce module existe
//!
//! [`crate::writer::Writer::bulk_sort_by_hilbert`] et
//! [`crate::writer::Writer::compact_community`] permutent les slots de la
//! zone Nodes et réécrivent `EdgeRecord.src/dst`, les side-colonnes Hilbert
//! et Community — mais **aucun autre side-car slot-indexé** :
//!
//! * `substrate.veccol.*` (colonnes vectorielles denses — dont
//!   `embedding_vec` en production) ;
//! * `substrate.blobcol.*` (directory slot-indexé de blobs) ;
//! * `substrate.degrees.node.u32` + `substrate.degrees.node.<type>.u32` ;
//! * `Zone::EngramMembers` / `Zone::EngramBitset` (valeurs = slots nœuds) ;
//! * `substrate.tier0` (table `slot_to_offset` dont les valeurs sont des
//!   slots nœuds).
//!
//! Après une repermutation, chacun de ces side-cars associerait silencieusement
//! la donnée du nœud A au nœud B. Le warden (`warden.rs`) déclencherait cette
//! corruption dès qu'un binaire l'instancie. Ce module fournit le point
//! d'accrochage obligatoire : tout side-car slot-indexé **doit** être enrôlé
//! ici avant tout câblage du warden.
//!
//! ## Contrat (invariants I1-I5)
//!
//! * **I1 — bijection totale.** `old_to_new` est une bijection de `[0, hw)`
//!   sur lui-même, sentinelle `0 → 0`. Vérifiée par
//!   [`validate_permutation`] avant toute mutation ; une permutation
//!   invalide est un refus (`Err`), jamais un remap partiel.
//! * **I2 — out-of-place + CRC + epoch.** Chaque side-car à en-tête
//!   (veccol, blobcol, degrees, tier0) est réécrit out-of-place (payload
//!   permuté en mémoire, memcpy retour) puis son en-tête est réécrit **en
//!   dernier** avec le CRC recalculé et le `remap_epoch` (stocké dans les
//!   mots réservés de l'en-tête). Rejeu : `epoch en-tête >= epoch WAL` ⇒
//!   déjà remappé ⇒ skip ; CRC invalide ⇒ payload non fiable ⇒ skip en
//!   laissant le side-car « absent » (fail-safe-absent, jamais un résultat
//!   faux). Les zones engram, sans en-tête, ne portent pas d'epoch : leur
//!   idempotence de rejeu passe par des enregistrements WAL **absolus**
//!   (`EngramMembersSet` / `EngramBitsetSet` post-remap) émis par le writer
//!   juste après le record de repermutation — le rejeu les ré-applique
//!   idempotemment quel que soit l'état intermédiaire.
//! * **I3 — ordre strict.** remaps → msync → flip du flag / epoch méta.
//!   Le writer applique : WAL fsync → mutations (nœuds, arêtes,
//!   side-colonnes, side-cars enrôlés) → msync zones → écriture méta
//!   (flag + `remap_epoch`) → commit.
//! * **I4 — ensure_room.** Toute croissance passe par
//!   [`crate::writer::ensure_room`] (ou l'équivalent du side-car).
//! * **I5 — aucune sémantique par-valeur.** Le remap déplace des octets de
//!   slot à slot (ou réécrit des valeurs-slots via `old_to_new`) sans
//!   jamais interpréter le contenu métier.
//!
//! ## Deux chemins d'appel
//!
//! * **Chemin vif** (writer, sous fenêtre calme) : les side-cars ouverts par
//!   le `SubstrateStore` (veccol, blobcol, degrees) sont remappés via le
//!   [`SlotRemapRegistry`] — leurs handles en mémoire portent l'état
//!   authoritaire (n_slots non encore synchronisés, compteurs atomiques).
//! * **Chemin rejeu** (`replay.rs`, processus neuf après crash) : les
//!   fichiers sur disque sont l'état authoritaire ; les side-cars sont
//!   retrouvés par leurs noms de fichiers (les specs veccol/blobcol sont
//!   encodées dans le nom) et remappés via les mêmes primitives,
//!   gardées par l'epoch d'en-tête.

//! ## Retrograder un binaire (regle d'exploitation)
//!
//! Les repermutations s'ecrivent avec les opcodes WAL `0x54` / `0x55`. Un
//! binaire ANTERIEUR a ce chantier ne les connait pas. Depuis le 2026-11-07,
//! il **refuse d'ouvrir la base** des qu'il rencontre une frame entiere, a
//! CRC valide, d'un opcode hors de sa table : `SubstrateError::WalOpcodeInconnu`
//! nomme l'opcode et l'offset, rien n'est rejoue ni tombstone, le WAL n'est
//! ni tronque ni modifie (`replay.rs`, `store.rs`, test
//! `tests/wal_opcode_inconnu.rs`). Avant cette date, le rejeu s'arretait
//! net en silence sur un prefixe, et le checkpoint suivant tronquait le WAL :
//! sur une base dont une repermutation etait en cours, cela laissait un etat
//! partiel — c'est ce que la garantie v0.0.1 interdit desormais.
//!
//! **REGLE D'EXPLOITATION : checkpoint AVANT tout downgrade.** Un checkpoint
//! (par le binaire recent) tronque le WAL ; l'ancien binaire ne rencontre
//! alors plus aucun opcode inconnu et peut ouvrir la base. Cette regle rend
//! la base LISIBLE par l'ancien binaire ; elle n'est plus ce qui protege les
//! donnees — le refus s'en charge.
//!
//! Outillage : `replay_from` remonte le refus en `ERROR`, avec la regle citee
//! dans le message. C'est le signal qu'un exploitant peut brancher sur une
//! alerte. Le diagnostic « base deja repermutee » du bras `Err` du rejeu ne
//! concerne plus que les arrets sur anomalie (CRC, decodage), pas l'opcode
//! inconnu.
//!
//! ## Memoire des passes de remap
//!
//! * colonnes vectorielles (`substrate.veccol.*`, jusqu'a plusieurs Go) —
//!   fichier temporaire + `rename` atomique, copie mmap vers mmap : la
//!   memoire consommee est du cache de pages, reclamable, et non du tas
//!   anonyme ;
//! * repertoire blob (16 o/slot) et colonnes de degres (8 o/slot) — copie
//!   en memoire via [`permute_dense_payload`]. Sur une base de 4,5 M de
//!   noeuds cela plafonne a ~72 Mo et ~36 Mo respectivement, bornes
//!   assumees : les basculer sur la mecanique par fichier ajouterait du
//!   code de bascule pour un gain sans commune mesure avec celui des
//!   colonnes vectorielles.

use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use crate::error::{SubstrateError, SubstrateResult};
use crate::file::{SubstrateFile, Zone};
use crate::writer::ensure_room;

/// Contexte d'exécution d'un remap — voir la doc du module.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RemapContext {
    /// Session vivante : les handles mmap ouverts sont l'état authoritaire ;
    /// les CRC d'en-tête peuvent être périmés (écritures non synchronisées)
    /// sans que cela n'invalide le payload.
    Live,
    /// Rejeu après crash : seul le disque fait foi. Un CRC d'en-tête
    /// invalide signifie « payload non fiable » ⇒ le side-car est laissé
    /// dans son état « absent » détectable, jamais remappé à l'aveugle.
    Replay,
}

/// Un side-car slot-indexé capable de suivre une repermutation des slots
/// nœuds. Implémenté par les registres/colonnes détenus par
/// `SubstrateStore` et enrôlés dans le [`SlotRemapRegistry`].
///
/// Écart assumé vs la spec initiale (`&mut self`) : les implémenteurs
/// vivent derrière `Arc<RwLock<…>>` / `DashMap` — la mutabilité est
/// intérieure, le trait prend `&self`.
pub trait SlotRemappable: Send + Sync {
    /// Nom stable pour les logs et les rapports d'erreur.
    fn remap_name(&self) -> &'static str;

    /// Applique `old_to_new` (bijection totale sur `[0, hw)`, déjà validée
    /// par l'appelant) aux données slot-indexées par nœud. `epoch` est le
    /// numéro de repermutation porté par le record WAL ; les implémenteurs
    /// à en-tête le persistent pour l'idempotence du rejeu.
    ///
    /// ── DISCIPLINE DE VERROUILLAGE (interblocage trouvé en test) ──
    ///
    /// On reçoit l'`Arc<Mutex<SubstrateFile>>`, PAS un garde déjà pris.
    /// Chaque implémenteur prend le verrou lui-même, sur la portée la
    /// plus courte possible. La première version passait un
    /// `&SubstrateFile` — donc l'appelant tenait le mutex pendant toute
    /// la passe — et `TypedDegreeRegistry::open_existing_columns` le
    /// reprend en interne : `parking_lot::Mutex` n'étant pas réentrant,
    /// les onze tests d'intégration se bloquaient net.
    fn remap_node_slots(
        &self,
        substrate: &Arc<Mutex<SubstrateFile>>,
        old_to_new: &[u32],
        epoch: u32,
        ctx: RemapContext,
    ) -> SubstrateResult<()>;

    /// Pendant « arêtes » du hook. Aucune opération du crate ne permute
    /// aujourd'hui les slots d'arêtes (les repermutations réécrivent
    /// `src`/`dst` en place, les slots d'arêtes ne bougent pas) — défaut
    /// no-op, présent pour que le jour où un compacteur d'arêtes existe,
    /// le point d'accrochage existe déjà.
    fn remap_edge_slots(
        &self,
        _old_to_new: &[u64],
        _epoch: u32,
        _ctx: RemapContext,
    ) -> SubstrateResult<()> {
        Ok(())
    }
}

/// Registre des side-cars enrôlés. Partagé entre `SubstrateStore` (qui
/// enrôle ses registres ouverts à l'open) et `Writer` (qui l'invoque sous
/// verrou writer pendant `bulk_sort_by_hilbert` / `compact_community`).
#[derive(Clone, Default)]
pub struct SlotRemapRegistry {
    inner: Arc<RwLock<Vec<Arc<dyn SlotRemappable>>>>,
}

impl SlotRemapRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enrôle un side-car. L'ordre d'enrôlement est l'ordre d'application.
    pub fn register(&self, side_car: Arc<dyn SlotRemappable>) {
        self.inner.write().push(side_car);
    }

    /// Nombre d'implémenteurs enrôlés (observabilité / tests).
    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.read().is_empty()
    }

    /// Applique la permutation à chaque side-car enrôlé, dans l'ordre
    /// d'enrôlement. La première erreur interrompt la passe : l'appelant
    /// (writer) NE flippe alors PAS le flag méta, et le rejeu — gardé par
    /// les epochs d'en-tête — reprendra les side-cars restants.
    pub fn remap_node_slots(
        &self,
        substrate: &Arc<Mutex<SubstrateFile>>,
        old_to_new: &[u32],
        epoch: u32,
        ctx: RemapContext,
    ) -> SubstrateResult<()> {
        let sidecars = self.inner.read().clone();
        for sc in &sidecars {
            sc.remap_node_slots(substrate, old_to_new, epoch, ctx)
                .map_err(|e| {
                    SubstrateError::Internal(format!(
                        "slot_remap: side-car '{}' a refusé le remap (epoch {}): {e}",
                        sc.remap_name(),
                        epoch
                    ))
                })?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Zone d'epoch — `substrate.remap.epoch`
// ---------------------------------------------------------------------------

/// Nom de la zone nommée qui porte l'epoch de repermutation durable de la
/// base. 16 octets : magic "REMP", version, epoch, CRC32 des 12 premiers
/// octets. Vit dans une zone nommée (et PAS dans le `MetaHeader`) parce
/// que le rejeu ne dispose que d'un `&SubstrateFile` — il peut ouvrir et
/// écrire une zone nommée, pas réécrire le méta.
pub const REMAP_EPOCH_ZONE: &str = "substrate.remap.epoch";

const REMAP_EPOCH_MAGIC: u32 = u32::from_le_bytes(*b"REMP");
const REMAP_EPOCH_VERSION: u32 = 2;
const REMAP_EPOCH_SIZE: usize = 20;

/// État durable de repermutation de la base.
///
/// Deux champs, et c'est tout l'intérêt :
///
/// * `committed` — la plus haute repermutation dont les zones nœuds et
///   arêtes sont DURABLES. C'est le seul gage qu'a le rejeu pour savoir
///   si la permutation est déjà dans les octets.
/// * `pending` — repermutation ANNONCÉE mais dont la durabilité des zones
///   n'est pas encore acquise. Écrit AVANT la première mutation, effacé
///   au moment où `committed` avance.
///
/// Un `pending` retrouvé au rejeu avec `committed` en retard signifie que
/// le processus est mort ENTRE l'annonce et la barrière : l'état des
/// zones est alors INDÉTERMINÉ (intact, à moitié permuté, ou entièrement
/// permuté — rien ne les distingue de l'extérieur). Ré-appliquer serait
/// jouer à pile ou face avec les données. Le rejeu REFUSE bruyamment, ce
/// qui est le seul comportement honnête : sans ces deux champs, cette
/// fenêtre corrompait EN SILENCE avec un CRC valide.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct RemapEpochState {
    pub committed: u32,
    pub pending: u32,
}

/// Lit l'etat durable de repermutation — **en echouant en mode FERME**.
///
/// ── LA DISTINCTION QUI PORTE TOUT (revue #3, 2026-08-24) ──
///
/// Cette fonction rendait `default()` des que magic, version ou CRC ne
/// collaient pas. Consequence prouvee par execution : un `xor 0x01` sur
/// l'octet 16 d'une base saine faisait relire `committed = 0`, le cas 3
/// du bras `HilbertRepermuteV2` re-appliquait la permutation sur des
/// zones deja permutees, et `open` rendait `Ok`. CRC valides, aucun
/// avertissement.
///
/// Le vecteur n'est pas une corruption de bit improbable :
/// [`REMAP_EPOCH_VERSION`] vaut **2**, donc une version 1 a existe. Toute
/// montee de version relirait TOUTE base comme neuve. C'est un evenement
/// PLANIFIE.
///
/// Trois etats, et un seul est legitime hors incident :
///
/// * **ABSENTE** — le fichier n'existe pas : base jamais repermutee.
///   Legitime, rend `default()`.
/// * **PRESENTE MAIS JAMAIS ECRITE** — fichier trop court, ou 20 octets
///   nuls. Legitime aussi : `open_named_zone` CREE le fichier, donc un
///   simple appel en lecture a pu le materialiser vide. Le magic etant
///   non nul, aucun etat reellement ecrit ne peut ressembler a cela.
/// * **PRESENTE ET ECRITE MAIS ILLISIBLE** — magic, version ou CRC ne
///   collent pas. **Jamais legitime.** REFUS.
fn lire_etat_epoch(sub: &SubstrateFile) -> SubstrateResult<RemapEpochState> {
    // `open_named_zone` CREE le fichier s'il manque. Ce court-circuit
    // evite de materialiser une zone d'epoch vide sur toute base jamais
    // repermutee — le predicat est consulte a CHAQUE ouverture de store.
    if !sub.path().join(REMAP_EPOCH_ZONE).exists() {
        return Ok(RemapEpochState::default());
    }
    let zf = sub.open_named_zone(REMAP_EPOCH_ZONE)?;
    let slice = zf.as_slice();
    // « Jamais ecrite » = TOUS les octets presents sont nuls. La version
    // precedente rendait `default()` sur un fichier trop court SANS
    // regarder son contenu (revue #4, reserve mineure) : un fichier court
    // mais NON NUL aurait ete accepte comme vierge, alors que la doc
    // ci-dessus affirme que c'est impossible. Non atteignable par le code
    // actuel — mais c'etait une garantie par convention, pas par
    // mecanisme, et ce module a passe quatre revues a apprendre ce que
    // coutent les garanties par convention.
    //
    // ── LIMITE ASSUMEE, ET DITE PLUTOT QUE TUE (revue #5) ──
    //
    // Une zone TRONQUEE A ZERO OCTET est indistinguable d'une zone
    // materialisee par une simple lecture : dans les deux cas il n'y a
    // aucun octet a examiner. Elle relit donc `committed = 0`, en
    // silence. C'est irreductible dans ce format — il faudrait un
    // marqueur hors de la zone pour distinguer « jamais ecrite » de
    // « videe » — et c'est le seul cas ou cet oracle reste muet. Toute
    // troncature PARTIELLE, elle, est refusee juste en dessous.
    let vus = &slice[..slice.len().min(REMAP_EPOCH_SIZE)];
    if vus.iter().all(|b| *b == 0) {
        // Une zone entierement nulle — complete ou tronquee — n'a jamais
        // ete ecrite : le magic « REMP » est non nul.
        return Ok(RemapEpochState::default());
    }
    if slice.len() < REMAP_EPOCH_SIZE {
        return Err(SubstrateError::EpoqueRemapIllisible {
            raison: format!(
                "zone tronquee a {} octets (il en faut {REMAP_EPOCH_SIZE}) et NON NULLE : \
                 ecriture interrompue, pas une zone vierge",
                slice.len()
            ),
        });
    }
    // ── A partir d'ici la zone A ETE ECRITE. Toute anomalie est un REFUS. ──
    let magic = u32::from_le_bytes(slice[0..4].try_into().unwrap());
    if magic != REMAP_EPOCH_MAGIC {
        return Err(SubstrateError::EpoqueRemapIllisible {
            raison: format!("magic {magic:#010x} au lieu de {REMAP_EPOCH_MAGIC:#010x}"),
        });
    }
    let version = u32::from_le_bytes(slice[4..8].try_into().unwrap());
    if version != REMAP_EPOCH_VERSION {
        return Err(SubstrateError::EpoqueRemapIllisible {
            raison: format!(
                "version {version}, ce binaire lit la version {REMAP_EPOCH_VERSION} — \
                 montee ou descente de version sans migration de la zone d'epoch"
            ),
        });
    }
    let crc = u32::from_le_bytes(slice[16..20].try_into().unwrap());
    let mut h = crc32fast::Hasher::new();
    h.update(&slice[0..16]);
    let calcule = h.finalize();
    if calcule != crc {
        return Err(SubstrateError::EpoqueRemapIllisible {
            raison: format!("CRC {crc:#010x} annonce, {calcule:#010x} calcule"),
        });
    }
    Ok(RemapEpochState {
        committed: u32::from_le_bytes(slice[8..12].try_into().unwrap()),
        pending: u32::from_le_bytes(slice[12..16].try_into().unwrap()),
    })
}

/// État de repermutation durable de la base (voir [`RemapEpochState`]).
///
/// `Err` si la zone d'epoch est presente mais illisible — voir
/// `lire_etat_epoch`. Ne JAMAIS rattraper cette erreur en rendant
/// `default()` : c'est precisement le defaut corrige en revue #3.
pub fn remap_epoch_state(sub: &SubstrateFile) -> SubstrateResult<RemapEpochState> {
    lire_etat_epoch(sub)
}

/// Epoch CONFIRMÉ de la base — celui dont les zones sont durables. 0 sur
/// une base jamais repermutée (zone absente ou jamais écrite). `Err` si
/// la zone existe mais ne se relit pas : allouer `epoch + 1` a partir
/// d'un `0` invente ferait re-permuter la base au rejeu suivant.
pub fn current_remap_epoch(sub: &SubstrateFile) -> SubstrateResult<u32> {
    Ok(lire_etat_epoch(sub)?.committed)
}

fn write_epoch_state(sub: &SubstrateFile, state: RemapEpochState) -> SubstrateResult<()> {
    let mut zf = sub.open_named_zone(REMAP_EPOCH_ZONE)?;
    ensure_room(&mut zf, REMAP_EPOCH_SIZE, 0)?;
    let mut buf = [0u8; REMAP_EPOCH_SIZE];
    buf[0..4].copy_from_slice(&REMAP_EPOCH_MAGIC.to_le_bytes());
    buf[4..8].copy_from_slice(&REMAP_EPOCH_VERSION.to_le_bytes());
    buf[8..12].copy_from_slice(&state.committed.to_le_bytes());
    buf[12..16].copy_from_slice(&state.pending.to_le_bytes());
    let crc = {
        let mut h = crc32fast::Hasher::new();
        h.update(&buf[0..16]);
        h.finalize()
    };
    buf[16..20].copy_from_slice(&crc.to_le_bytes());
    zf.as_slice_mut()[..REMAP_EPOCH_SIZE].copy_from_slice(&buf);
    zf.msync()?;
    zf.fsync()?;
    // (Revue #3, dette 1) Le fsync du FICHIER ne rend pas durable son
    // ENTREE DE REPERTOIRE quand le fichier vient d'etre cree. Sur la
    // premiere repermutation d'une base, l'annonce pouvait donc se
    // perdre : la garde devenait muette et la base s'ouvrait sur des
    // zones a moitie permutees. Exposition nulle sur ext4 (qui force la
    // dentry parente au fsync d'un fichier neuf), mais ce lot ajoutait
    // `fsync_repertoire` pour le chemin AUTO-REPARABLE (le rename) en
    // oubliant le chemin PORTEUR — celui dont depend toute la garde.
    fsync_repertoire(&sub.path().join(REMAP_EPOCH_ZONE));
    Ok(())
}

/// ANNONCE une repermutation : `pending = epoch`, `committed` inchangé.
///
/// À appeler APRÈS la durabilité du record WAL et AVANT la première
/// mutation de zone. Rend la fenêtre « zones touchées, durabilité pas
/// encore acquise » DÉTECTABLE au rejeu.
///
/// ── PRIVEE AU MODULE (revue #5, verrou structurel) ──
///
/// Elle etait `pub`, et trois chemins l'appelaient directement. Deux ont
/// recu la discipline d'effacement qui la rend sure ; le troisieme, ajoute
/// plus tard, ne l'a pas recue — et a reproduit a l'identique le defaut
/// que le premier avait deja coute. Partager une fonction n'oblige
/// personne a partager la regle qui l'entoure.
///
/// Le seul acces depuis l'exterieur du module est desormais
/// [`AnnonceRemap::annoncer`], qui rend une garde ne pouvant pas etre
/// laissee non resolue. Un quatrieme chemin ne peut plus annoncer sans
/// pouvoir effacer : ce n'est plus une convention, c'est le compilateur.
fn announce_remap_epoch(sub: &SubstrateFile, epoch: u32) -> SubstrateResult<()> {
    let mut state = lire_etat_epoch(sub)?;
    state.pending = epoch;
    write_epoch_state(sub, state)
}

/// CONFIRME la repermutation : `committed = epoch`, `pending` effacé.
///
/// À appeler juste APRÈS la barrière de durabilité des zones nœuds et
/// arêtes (msync + fsync), et AVANT l'estampillage des side-cars — de
/// sorte qu'un crash entre les deux laisse un `committed` correct et des
/// side-cars que le rejeu rattrape par leurs propres epochs d'en-tête.
pub fn commit_remap_epoch(sub: &SubstrateFile, epoch: u32) -> SubstrateResult<()> {
    let mut state = lire_etat_epoch(sub)?;
    state.committed = state.committed.max(epoch);
    if state.pending == epoch {
        state.pending = 0;
    }
    write_epoch_state(sub, state)
}

/// Où en est une passe de repermutation **par rapport à la BARRIÈRE de
/// durabilité** — la seule question dont dépend la bonne reprise.
///
/// ══ POURQUOI CET ÉTAT EST TERNAIRE ET NON BINAIRE (revue #3) ══
///
/// La revue #2 avait fait câbler `clear_pending_remap_epoch` sur les
/// chemins d'échec, avec un drapeau booléen « zones touchées ». Ce
/// booléen répondait à la bonne question mais pas jusqu'au bout : il
/// distinguait « rien touché » de « quelque chose touché », alors que
/// la troisième situation — **barrière franchie** — se comporte à
/// l'inverse des deux autres.
///
/// * [`Self::AucuneMutation`] — aucun octet de zone n'a bougé. L'état est
///   CONNU INTACT. Reprise : **effacer l'annonce**. La base s'ouvre, et
///   un rejeu ultérieur appliquera la permutation depuis le WAL.
/// * [`Self::AvantBarriere`] — au moins une écriture a eu lieu, la
///   barrière n'est pas franchie. Les zones peuvent être intactes, à
///   moitié permutées ou entièrement permutées, et RIEN de l'extérieur
///   ne les distingue. L'état est INDÉTERMINÉ. Reprise : **aucune**, on
///   refuse.
/// * [`Self::ApresBarriere`] — `msync_zones` a réussi : les zones sont
///   permutées ET durables. L'état est aussi DÉTERMINÉ que dans le
///   premier cas, mais avec la valeur OPPOSÉE. Reprise : **confirmer
///   l'epoch**. Effacer l'annonce ici serait une corruption : le rejeu
///   raterait ses cas 1 et 2, tomberait dans son cas 3, et
///   ré-appliquerait la permutation sur des zones déjà permutées.
///
/// ── CE QUE COÛTAIT L'ABSENCE DU TROISIÈME ÉTAT ──
///
/// Prouvé en revue #3 : `commit_remap_epoch` était appelée APRÈS la
/// région gardée, avec un `?` nu. Son échec immobilisait une base
/// parfaitement saine (l'ouverture refusait), sans rien journaliser qui
/// dise de quel côté de la barrière on se trouvait — puis la procédure
/// de reprise documentée, qui ne connaissait que l'hypothèse « zones
/// intactes », la CORROMPAIT. Le chemin de secours détruisait ce que la
/// panne avait préservé.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PhaseRepermutation {
    /// Aucun octet de zone n'a bougé — état CONNU INTACT.
    AucuneMutation,
    /// Zones mutées, barrière NON franchie — état INDÉTERMINÉ.
    AvantBarriere,
    /// Barrière franchie : zones permutées ET durables — état DÉTERMINÉ.
    ApresBarriere,
}

impl PhaseRepermutation {
    /// Encodage pour le partage par `AtomicU8` — le drapeau est posé
    /// depuis l'intérieur des closures de mutation.
    pub fn code(self) -> u8 {
        match self {
            Self::AucuneMutation => 0,
            Self::AvantBarriere => 1,
            Self::ApresBarriere => 2,
        }
    }

    /// Inverse de [`Self::code`]. Tout code inconnu est ramené au cas le
    /// plus PRUDENT (`AvantBarriere`, donc refus) et jamais au cas qui
    /// efface ou qui confirme : une valeur qu'on ne comprend pas ne doit
    /// jamais autoriser une action irréversible.
    pub fn depuis_code(code: u8) -> Self {
        match code {
            0 => Self::AucuneMutation,
            2 => Self::ApresBarriere,
            _ => Self::AvantBarriere,
        }
    }
}

/// Epoch ANNONCEE et jamais CONFIRMEE, s'il y en a une — le seul
/// predicat qui dit « l'etat des zones noeuds/aretes est indetermine ».
///
/// ── POURQUOI CE PREDICAT ET PAS LE CANAL `Err` DU REJEU ──
///
/// Le bras `HilbertRepermuteV2` du rejeu detecte le meme etat, mais
/// SEULEMENT tant que le record de repermutation est encore dans la
/// fenetre de rejeu. Le premier `flush()` pousse `dernier_offset_flush`
/// au-dela du record : il n'est alors plus jamais rejoue, `pending` reste
/// pose indefiniment, et la detection disparait. Ce predicat lit l'etat
/// DURABLE de la base — il survit a la troncature du WAL, donc c'est lui
/// qui garde l'ouverture (`SubstrateStore::from_substrate`).
pub fn epoque_indeterminee(sub: &SubstrateFile) -> SubstrateResult<Option<u32>> {
    let etat = lire_etat_epoch(sub)?;
    Ok((etat.pending != 0 && etat.pending > etat.committed).then_some(etat.pending))
}

/// `fsync` du REPERTOIRE parent d'un fichier — indispensable apres un
/// `rename` (M2, revue #2). Le `rename` est atomique du point de vue de
/// la visibilite, mais sa DURABILITE exige que l'entree de repertoire
/// soit elle-meme sur disque : sans ce fsync, un crash juste apres la
/// bascule peut laisser le repertoire pointer encore sur l'ancien inode.
///
/// Auto-repare dans le cas courant (les side-cars sont remappes APRES
/// `commit_remap_epoch`, un rejeu repasse par le cas 1 et refait le
/// travail) — mais PAS apres le flush final de la repermutation, ou le
/// record sort de la fenetre de rejeu : un crash a cet instant laisserait
/// la colonne en etat pre-permutation face a des noeuds permutes, sans
/// plus aucun rattrapage possible.
///
/// L'echec d'ouverture du repertoire est journalise et avale : le rename
/// a DEJA eu lieu, ce fsync est un durcissement de durabilite, pas une
/// precondition — echouer ici transformerait un remap reussi en erreur.
/// (Revue #3) Le retour est `()` et non `SubstrateResult<()>` : cette
/// fonction ne peut JAMAIS rendre `Err`, et les appelants ecrivaient un
/// `?` qui suggerait le contraire. Un type qui ment sur ce qu'il peut
/// faire finit par etre lu de travers.
pub fn fsync_repertoire(fichier: &std::path::Path) {
    let Some(parent) = fichier.parent() else {
        return;
    };
    match std::fs::File::open(parent) {
        Ok(d) => {
            if let Err(e) = d.sync_all() {
                tracing::warn!(repertoire = %parent.display(), erreur = %e,
                    "fsync du repertoire parent a echoue apres rename — bascule visible, durabilite non garantie");
            }
        }
        Err(e) => {
            tracing::warn!(repertoire = %parent.display(), erreur = %e,
                "ouverture du repertoire parent impossible apres rename — durabilite de la bascule non garantie");
        }
    }
}

// ---------------------------------------------------------------------------
// DIAGNOSTIC DE REPERMUTATION — source UNIQUE du dry-run et de l'apply
// ---------------------------------------------------------------------------

/// Ce que l'état durable de la base permet d'établir sur une
/// repermutation interrompue.
///
/// ══ UNE SEULE IMPLÉMENTATION, DEUX USAGES ══
///
/// `obrain repair --dry-run` AFFICHE ce diagnostic ; `obrain repair
/// --apply` AGIT dessus. Ce n'est pas une convention : c'est structurel —
/// [`appliquer_reparation`] appelle elle-même [`diagnostiquer`] et rend
/// le diagnostic qu'elle a utilisé, de sorte qu'un raccourci « on sait
/// déjà » soit impossible à écrire sans faire diverger la valeur rendue.
/// Le test `le_dry_run_et_l_apply_partagent_le_meme_diagnostic` verrouille
/// cette égalité.
///
/// Deux règles de détection pour la même décision est le défaut
/// structurel le plus fréquent de ce chantier — c'est toujours au point
/// où elles divergent que quelqu'un se perd.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticRemap {
    /// Aucune repermutation en suspens. Rien à réparer.
    Saine {
        /// Epoch confirmée de la base (0 = jamais repermutée).
        committed: u32,
    },
    /// **L'oracle lui-même est illisible.** On REFUSE de diagnostiquer.
    ///
    /// Un diagnostic « base saine » rendu sur une zone qu'on n'a pas su
    /// lire serait pire que l'absence de diagnostic, parce qu'il en
    /// porterait l'autorité. Voir `lire_etat_epoch`.
    OracleIllisible { raison: String },
    /// Une repermutation est ANNONCÉE et jamais CONFIRMÉE.
    RepermutationInterrompue {
        epoch: u32,
        committed: u32,
        observations: Observations,
    },
    /// **L'état durable se contredit.** Un témoin ou un side-car porte
    /// l'epoch annoncée (ou davantage) alors que `pending > committed`.
    ///
    /// C'est impossible par construction : tout ce qui porte l'epoch `E`
    /// est produit par la phase 9, qui s'exécute APRÈS
    /// `commit_remap_epoch(E)`. Rencontrer cet état signifie que les
    /// fichiers ont été manipulés à la main, ou qu'une invariante du
    /// module a été cassée par une modification ultérieure.
    ///
    /// Cette variante existe pour que l'impossible soit un REFUS BRUYANT
    /// et non une branche morte. Une branche « si un témoin de l'epoch
    /// courante existe alors la barrière a été franchie » serait
    /// inatteignable — et un drapeau mort finit toujours par mentir.
    EtatContradictoire {
        epoch: u32,
        committed: u32,
        details: String,
    },
}

/// Ce que l'outil RECOMMANDE — la règle que l'opérateur suit réellement.
///
/// ══ POURQUOI ELLE VIT ICI ET PLUS DANS LE CLI (revue #4) ══
///
/// Le lot précédent revendiquait l'unicité du diagnostic, et elle était
/// réelle — mais elle couvrait l'ÉTAT, pas la RECOMMANDATION. La règle
/// que l'opérateur suit vivait dans `obrain-cli`, hors du diagnostic
/// partagé, et n'avait aucun test. C'est le défaut que ce chantier nomme
/// comme le plus fréquent, reproduit un étage plus haut : le diagnostic
/// était unique, la consigne ne l'était pas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recommandation {
    /// Rien à faire.
    RienAFaire,
    /// On ne répare pas ce qu'on n'a pas su lire, ni ce qui se contredit.
    NePasToucher { raison: String },
    /// **Aucune reprise n'est sûre depuis l'état durable seul.**
    ///
    /// C'est le SEUL verdict que le format actuel permet sur une
    /// repermutation interrompue, et la démonstration tient en une
    /// phrase : la seule chose qui prouverait que la barrière a été
    /// franchie est la confirmation elle-même, et c'est précisément ce
    /// qui manque. Tout ce qui se trouve en aval d'elle (side-cars
    /// estampillés, témoins de phase 9) est en aval de ce qui manque.
    ///
    /// Le journal du writer, lui, tranche — quand l'interruption fut un
    /// retour d'erreur. Après un arrêt brutal (coupure, OOM-kill,
    /// redémarrage de conteneur) il n'y a rien, et les deux côtés
    /// restent possibles : la bonne réponse est alors la sauvegarde.
    AucuneReprisSureSansJournal { epoch: u32 },
}

/// Ce que la base laisse voir, sans interprétation.
///
/// ── CE QUE CES OBSERVATIONS NE DISENT PAS ──
///
/// Elles ne permettent **pas** de décider de quel côté de la barrière de
/// durabilité la passe s'est arrêtée. C'est une limite de fond, pas un
/// manque d'effort : les zones nœuds/arêtes sont permutées EN PLACE, donc
/// « intactes », « à moitié permutées » et « entièrement permutées » sont
/// trois contenus d'octets également plausibles, et rien dans le format
/// actuel ne les distingue. Le dry-run le DIT au lieu de deviner.
///
/// Deux sources externes tranchent en pratique :
/// * le journal du writer, champ `cote_barriere`, quand l'interruption
///   fut un retour d'erreur et non un crash — c'est le cas courant ;
/// * une sauvegarde, sinon.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Observations {
    /// Le record de repermutation de cette epoch est-il encore lisible
    /// dans le WAL ? S'il l'est, un rejeu saurait appliquer la
    /// permutation ; s'il ne l'est plus, seule la zone d'epoch témoigne.
    pub record_dans_le_wal: bool,
    /// Epochs des en-têtes de side-cars (nom de fichier, epoch).
    pub sidecars: Vec<(String, u32)>,
    /// Temporaires de repermutation **datés d'une epoch ANTÉRIEURE** à
    /// celle annoncée, ou non datés (format hérité) : `(nom, epoch, octets)`.
    ///
    /// Ce sont des DÉBRIS d'une passe précédente. Ils ne disent **rien**
    /// de la passe en cours — c'est exactement l'inférence fausse que la
    /// revue #4 a prouvée. Ils restent rapportés parce qu'ils occupent du
    /// disque et signalent une phase 9 antérieurement interrompue.
    pub temoins_anterieurs: Vec<(String, Option<u32>, u64)>,
    /// Temoins portant l'epoch ANNONCÉE, ou une epoch supérieure.
    ///
    /// ── CETTE LISTE EST VIDE PAR CONSTRUCTION ──
    ///
    /// ══ LA PREUVE, ET POURQUOI CE N'EST PAS CELLE QU'ON CROIT ══
    ///
    /// La version précédente de ce commentaire disait : « tout artefact
    /// portant l'epoch `E` est produit par la phase 9 ». **C'est faux** —
    /// [`crate::degree_column::DegreeColumn::create`] estampille
    /// `_reserved[0]` à la création d'une colonne fraîche, hors de toute
    /// phase 9. La conclusion tenait par accident, parce que la valeur
    /// posée là se trouve être `committed`.
    ///
    /// L'argument qui porte réellement la preuve est la MONOTONIE :
    ///
    /// 1. `committed` ne décroît jamais ([`commit_remap_epoch`] applique
    ///    un `max`) ;
    /// 2. toute epoch annoncée vaut `committed + 1` (les deux passes de
    ///    `Writer` allouent `current_remap_epoch() + 1`) ;
    /// 3. tout artefact estampillé `E` l'est avec une valeur qui ne
    ///    dépasse jamais `committed` — soit `committed` lui-même à la
    ///    création, soit `E` après un commit qui a porté `committed` à `E`.
    ///
    /// Donc `epoch_artefact <= committed < pending`, et la liste est vide.
    ///
    /// Cette formulation-ci a une propriété que l'autre n'avait pas : le
    /// jour où quelqu'un estampille un side-car avec autre chose que
    /// `committed`, c'est le point (3) qui casse, et il casse VISIBLEMENT.
    /// L'argument « tout vient de la phase 9 » ne l'aurait pas signalé.
    ///
    /// Trouver cette liste non vide signifie donc que l'état durable se
    /// contredit : [`diagnostiquer`] rend alors
    /// [`DiagnosticRemap::EtatContradictoire`] plutôt qu'une conclusion.
    /// Verrouillé par `aucun_temoin_de_l_epoch_annoncee_ne_peut_exister`
    /// et `un_sidecar_estampille_a_l_epoch_annoncee_est_contradictoire`.
    pub temoins_epoch_annoncee: Vec<(String, u32)>,
}

/// Verdict que l'opérateur atteste quand la base ne peut pas trancher
/// seule. Les deux valeurs commandent des actions OPPOSÉES : se tromper
/// de verdict corrompt.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VerdictOperateur {
    /// Les zones n'ont PAS été permutées — on efface l'annonce.
    ZonesIntactes,
    /// Les zones SONT permutées et durables — on confirme l'epoch.
    ZonesPermutees,
}

/// Action effectuée (ou qui le serait) par une réparation.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ActionReparation {
    /// Rien à faire — la base est saine.
    RienAFaire,
    /// L'annonce a été effacée : la base s'ouvre, données intactes, et un
    /// rejeu appliquera la permutation depuis le WAL.
    AnnonceEffacee { epoch: u32 },
    /// L'epoch a été confirmée : la base s'ouvre, sans repermutation ; le
    /// rejeu se contentera de rattraper les side-cars.
    EpochConfirmee { epoch: u32 },
}

/// LE diagnostic. Lecture seule, aucune mutation, aucun effet de bord.
pub fn diagnostiquer(sub: &SubstrateFile) -> DiagnosticRemap {
    let etat = match lire_etat_epoch(sub) {
        Ok(e) => e,
        Err(SubstrateError::EpoqueRemapIllisible { raison }) => {
            return DiagnosticRemap::OracleIllisible { raison };
        }
        Err(e) => {
            return DiagnosticRemap::OracleIllisible {
                raison: format!("zone d'epoch inaccessible : {e}"),
            };
        }
    };
    if etat.pending == 0 || etat.pending <= etat.committed {
        return DiagnosticRemap::Saine {
            committed: etat.committed,
        };
    }
    let observations = observer(sub, etat.pending);
    if !observations.temoins_epoch_annoncee.is_empty() {
        return DiagnosticRemap::EtatContradictoire {
            epoch: etat.pending,
            committed: etat.committed,
            details: format!(
                "pending={} > committed={}, mais ces temoins portent deja l'epoch annoncee : \
                 {:?}. Impossible : la phase 9 qui les produit s'execute APRES \
                 commit_remap_epoch, qui aurait donc porte committed a {}.",
                etat.pending, etat.committed, observations.temoins_epoch_annoncee, etat.pending
            ),
        };
    }
    DiagnosticRemap::RepermutationInterrompue {
        epoch: etat.pending,
        committed: etat.committed,
        observations,
    }
}

/// LA recommandation — une seule regle, ici, testee.
pub fn recommander(diag: &DiagnosticRemap) -> Recommandation {
    match diag {
        DiagnosticRemap::Saine { .. } => Recommandation::RienAFaire,
        DiagnosticRemap::OracleIllisible { raison } => Recommandation::NePasToucher {
            raison: raison.clone(),
        },
        DiagnosticRemap::EtatContradictoire { details, .. } => Recommandation::NePasToucher {
            raison: details.clone(),
        },
        DiagnosticRemap::RepermutationInterrompue { epoch, .. } => {
            Recommandation::AucuneReprisSureSansJournal { epoch: *epoch }
        }
    }
}

/// Epoch d'en-tete des side-cars dont le format n'a pas de parseur de nom
/// dedie : tier0 (`substrate.tier0`) et les colonnes de degres
/// (`substrate.degrees.node[.<type>].u32`).
///
/// Les deux portent leur epoch dans `_reserved[0]` d'un en-tete de 64
/// octets, valide par un magic. On lit ces 64 octets et rien de plus :
/// un CRC perime n'empeche pas de lire l'estampille, et c'est
/// l'estampille seule qui nous interesse ici.
fn epoch_de_side_car_a_entete(sub: &SubstrateFile, nom: &str) -> Option<u32> {
    let est_degres = nom.starts_with("substrate.degrees.node") && nom.ends_with(".u32");
    let est_tier0 = nom == crate::file::zone::TIER0;
    if !est_degres && !est_tier0 {
        return None;
    }
    let zf = sub.open_named_zone(nom).ok()?;
    let slice = zf.as_slice();
    if est_degres {
        let h: &crate::degree_column::DegreeHeader =
            bytemuck::try_from_bytes(slice.get(..crate::degree_column::HEADER_SIZE)?).ok()?;
        (h.magic == crate::degree_column::DEGREE_MAGIC).then_some(h._reserved[0])
    } else {
        let h: &crate::tier_persist::TierPersistHeader =
            bytemuck::try_from_bytes(slice.get(..crate::tier_persist::HEADER_SIZE)?).ok()?;
        // Tier0 porte le magic de sa famille ; on se contente d'exiger
        // qu'il soit non nul, la zone etant nommee sans ambiguite.
        (h.magic != 0).then_some(h._reserved[0])
    }
}

fn observer(sub: &SubstrateFile, epoch: u32) -> Observations {
    let mut obs = Observations::default();

    // (a) Le record de repermutation est-il encore dans le WAL ?
    if let Ok(reader) = crate::wal_io::WalReader::open(sub.wal_path()) {
        for item in reader.iter_from(0) {
            let Ok((rec, _, _)) = item else { break };
            let porte = match &rec.payload {
                crate::wal::WalPayload::HilbertRepermuteV2 { epoch: e, .. }
                | crate::wal::WalPayload::CompactCommunityV2 { epoch: e, .. } => *e == epoch,
                _ => false,
            };
            if porte {
                obs.record_dans_le_wal = true;
                break;
            }
        }
    }

    // (b) Side-cars et temporaires : une seule traversee du repertoire.
    let Ok(entrees) = std::fs::read_dir(sub.path()) else {
        return obs;
    };
    for entree in entrees.flatten() {
        let chemin = entree.path();
        let Some(nom) = chemin.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Some(epoch_tmp) = epoch_du_temporaire(nom) {
            let octets = entree.metadata().map(|m| m.len()).unwrap_or(0);
            // Un temoin ne prouve quelque chose sur la passe EN COURS que
            // s'il porte SON epoch. Un temoin herite ou non date est un
            // debris : il occupe du disque, il ne temoigne de rien.
            match epoch_tmp {
                Some(e) if e >= epoch => obs.temoins_epoch_annoncee.push((nom.to_string(), e)),
                autre => obs
                    .temoins_anterieurs
                    .push((nom.to_string(), autre, octets)),
            }
            continue;
        }
        if let Some(spec) = crate::vec_column::VecColSpec::parse_filename(nom)
            && let Ok(zf) = sub.open_named_zone(&spec.filename())
            && let Some(h) = crate::vec_column::VecColumnHeader::try_read(zf.as_slice())
        {
            obs.sidecars.push((nom.to_string(), h._reserved[0]));
        } else if let Some(spec) = crate::blob_column::BlobColSpec::parse_idx_filename(nom)
            && let Ok(zf) = sub.open_named_zone(&spec.idx_filename())
            && let Some(h) = crate::blob_column::BlobColumnHeader::try_read(zf.as_slice())
        {
            obs.sidecars.push((nom.to_string(), h._reserved[0]));
        } else if let Some(epoch_sc) = epoch_de_side_car_a_entete(sub, nom) {
            // (Revue #5) Le filet ne voyait que veccol et blobcol, alors
            // que le rejeu estampille AUSSI tier0 et les colonnes de
            // degres. Ces deux familles passaient au travers du controle
            // de coherence — donc `EtatContradictoire` ne pouvait pas les
            // detecter, et l'operateur ne les voyait pas dans le dry-run.
            obs.sidecars.push((nom.to_string(), epoch_sc));
        }
    }
    // Meme raisonnement que pour les temoins : un side-car estampille a
    // l'epoch annoncee impliquerait que la phase 9 a tourne, donc que la
    // confirmation a reussi, donc que `committed >= epoch`.
    for (nom, e) in &obs.sidecars {
        if *e >= epoch {
            obs.temoins_epoch_annoncee.push((nom.clone(), *e));
        }
    }
    obs.sidecars.sort();
    obs.temoins_anterieurs.sort();
    obs.temoins_epoch_annoncee.sort();
    obs
}

/// Applique une réparation — **en repassant par [`diagnostiquer`]**.
///
/// Rend le diagnostic effectivement utilisé, de sorte qu'un appelant
/// (et un test) puisse verifier qu'il n'existe pas de seconde regle de
/// decision. Refuse categoriquement :
///
/// * sur [`DiagnosticRemap::OracleIllisible`] — on ne repare pas ce qu'on
///   n'a pas su lire ;
/// * sur [`DiagnosticRemap::Saine`] — il n'y a rien a reparer, et agir
///   quand meme serait une mutation gratuite d'un etat correct.
pub fn appliquer_reparation(
    sub: &SubstrateFile,
    verdict: VerdictOperateur,
) -> SubstrateResult<(DiagnosticRemap, ActionReparation)> {
    let diag = diagnostiquer(sub);
    match &diag {
        DiagnosticRemap::OracleIllisible { raison } => Err(SubstrateError::EpoqueRemapIllisible {
            raison: format!("{raison} — reparation REFUSEE : rien ne peut etre decide"),
        }),
        // Meme refus, meme raison de fond : on n'agit pas sur un etat
        // dont on vient d'etablir qu'il est impossible.
        DiagnosticRemap::EtatContradictoire { details, .. } => {
            Err(SubstrateError::EpoqueRemapIllisible {
                raison: format!("etat durable contradictoire ({details}) — reparation REFUSEE"),
            })
        }
        DiagnosticRemap::Saine { .. } => Ok((diag, ActionReparation::RienAFaire)),
        DiagnosticRemap::RepermutationInterrompue { epoch, .. } => {
            let epoch = *epoch;
            let action = match verdict {
                VerdictOperateur::ZonesIntactes => {
                    clear_pending_remap_epoch(sub, epoch)?;
                    ActionReparation::AnnonceEffacee { epoch }
                }
                VerdictOperateur::ZonesPermutees => {
                    commit_remap_epoch(sub, epoch)?;
                    ActionReparation::EpochConfirmee { epoch }
                }
            };
            Ok((diag, action))
        }
    }
}

/// Ou vit le substrat que la garde doit rejoindre pour se resoudre.
///
/// Deux formes, parce que les deux chemins de production n'ont pas le
/// meme regime de verrou : le rejeu tient un `&SubstrateFile` pendant
/// toute la passe, le writer partage un `Mutex` qu'il ne doit surtout pas
/// garder verrouille pendant les phases (interblocage avec le registre de
/// side-cars — voir la doc du trait `SlotRemappable`).
enum CibleAnnonce<'a> {
    Directe(&'a SubstrateFile),
    /// ⚠ **DANGER : la resolution REPREND ce verrou.**
    ///
    /// `parking_lot::Mutex` n'est pas reentrant. Un chemin qui tient
    /// encore le verrou au moment ou la garde meurt figerait le fil,
    /// definitivement et en silence — la revue #6 l'a execute.
    ///
    /// Les deux sites actuels sont saufs : `confirmer` et `abandonner`
    /// sont appeles HORS de toute portee `substrate.lock()`, et les `?`
    /// comme les paniques relachent le garde interne avant que l'annonce
    /// ne meure. Mais « saufs aujourd'hui » est precisement ce qui a coute
    /// deux revues a ce module.
    ///
    /// D'ou [`DELAI_REPRISE_VERROU`] : la resolution attend BORNE, puis
    /// renonce en journalisant. On echange un gel silencieux contre une
    /// annonce laissee et un message explicite — ce qui est toujours
    /// preferable dans un destructeur.
    Partagee(std::sync::Arc<parking_lot::Mutex<SubstrateFile>>),
}

/// Temps maximal d'attente du verrou substrat lors de la resolution
/// IMPLICITE d'une annonce (`Drop` uniquement — voir
/// [`AnnonceRemap::avec_substrat_borne`]).
///
/// ── CE QUE CETTE CONSTANTE EST, ET CE QU'ELLE N'EST PAS ──
///
/// Elle n'est PAS calibree. Aucune mesure ne la fonde : c'est un ordre de
/// grandeur choisi comme genereux au regard du travail a faire (une
/// ecriture de 20 octets), pas un p99.9 derive d'une distribution
/// observee. La revue #7 a mesure le plus long teneur de `substrate.lock()`
/// qu'elle a su produire — `checkpoint()`, 32 ms a 10k nœuds, 44 ms a 200k
/// avec un WAL de 42 Mo — soit ~45x de marge. Elle n'a pas atteint le
/// regime que decrivent les commentaires de ce module (WAL de 452 Mo,
/// 626 845 nœuds). Donc : plausiblement sur aux tailles mesurees, PAS
/// montre sur.
///
/// Le risque a ete reduit autrement, et c'est ce qui compte : depuis la
/// revue #7 cette borne ne s'applique QU'AU destructeur. Les resolutions
/// explicites (`confirmer`, `abandonner`) attendent sans limite, donc un
/// depassement ne peut plus transformer une repermutation REUSSIE en base
/// non ouvrable. Ce qu'un depassement coute desormais, c'est le filet —
/// pas le chemin nominal.
const DELAI_REPRISE_VERROU: std::time::Duration = std::time::Duration::from_secs(2);

/// Garde d'annonce de repermutation — **l'annonce et sa resolution ne
/// peuvent plus etre separees**.
///
/// ══ POURQUOI UNE GARDE PLUTOT QU'UNE DISCIPLINE (revue #5) ══
///
/// L'histoire de ce module est celle d'une meme regle oubliee deux fois.
/// La revue #2 a fait cabler l'effacement dans `Writer`. La revue #4 a
/// fait poser une annonce dans le chemin de REJEU — sans la discipline,
/// et le defaut s'est reproduit a l'identique, en pire : le rejeu tourne
/// a l'ouverture, donc un ENOSPC passager y briquait la base. La revue #5
/// a extrait le gestionnaire pour que les deux chemins le partagent.
///
/// Partager ne suffit pas. Deux chemins qui appellent aujourd'hui la meme
/// fonction ne garantissent rien sur le troisieme : c'est exactement ce
/// qui venait de se produire. Tant qu'il faut SE SOUVENIR d'appeler le
/// gestionnaire, quelqu'un l'oubliera.
///
/// ══ CE QUE CHAQUE MECANISME PORTE REELLEMENT (rectifie en revue #6) ══
///
/// La version precedente de cette doc annoncait « trois mecanismes qui ne
/// se recouvrent pas ». C'etait faux, et la revue l'a montre en les
/// desarmant un par un :
///
/// * **[`Drop`] — PORTEUR.** Le neutraliser fait rougir un test. Si
///   personne n'a tranche (oubli, `?` qui saute par-dessus, panique), la
///   garde tranche elle-meme selon la phase atteinte, avec la logique
///   ternaire de [`abandonner_annonce`].
/// * **Privatisation de `announce_remap_epoch` — PORTEUSE, mais
///   partielle.** Elle ferme la route evidente ; elle ne ferme pas
///   `use ... as alias`, ce que la revue a compile. Le balayage de
///   `aucun_chemin_ne_peut_annoncer_sans_pouvoir_effacer` couvre
///   desormais l'alias, en cherchant l'IDENTIFIANT et plus l'appel.
///   `Drop` ne protege que ceux qui ont pris une garde ; c'est ce que la
///   privatisation ajoute.
/// * **`#[must_use]` — DECORATIF.** Il est strictement subsume par
///   `Drop` : le seul diagnostic qu'il ajoute porte sur
///   `annoncer(..).unwrap();` en position d'instruction, cas ou la garde
///   est detruite en fin d'instruction — donc ou `Drop` resout deja. Le
///   cas « `Result` jete tel quel » est couvert par le `#[must_use]` de
///   `Result`. Il est conserve pour la qualite du message, pas pour la
///   surete. Je ne l'avais pas vu parce que mes sept mutations visaient
///   toutes la RESOLUTION, jamais l'attribut.
///
/// Le filet ne remplace pas la discipline : il rend son oubli inoffensif.
#[must_use = "une annonce de repermutation non resolue laisse `pending` pose : \
              appelez `confirmer` ou `abandonner` (a defaut, `Drop` tranchera)"]
pub struct AnnonceRemap<'a> {
    cible: CibleAnnonce<'a>,
    epoch: u32,
    phase: std::sync::atomic::AtomicU8,
    resolue: bool,
}

impl<'a> AnnonceRemap<'a> {
    /// ANNONCE la repermutation et rend la garde qui la resoudra.
    ///
    /// A appeler APRES la durabilite du record WAL et AVANT la premiere
    /// mutation de zone.
    /// ── LE CONSTRUCTEUR S'APPLIQUE SA PROPRE DISCIPLINE (revue #6) ──
    ///
    /// Il portait un `?` nu. Or `write_epoch_state` ECRIT la page avant
    /// de demander sa durabilite : une erreur d'E/S a cette couture fait
    /// echouer l'annonce APRES que `pending` soit dans le fichier. Aucune
    /// garde n'est alors rendue — donc ni `Drop`, ni `#[must_use]`, ni la
    /// privatisation n'ont prise. Le constructeur etait exactement le
    /// « quatrieme chemin » que la garde pretendait rendre impossible.
    ///
    /// (Ce n'etait pas une regression : le meme trou existait avant la
    /// garde, `write_epoch_state` etant inchangee. C'etait un defaut de
    /// REVENDICATION, et il est corrige ici.)
    pub fn annoncer(sub: &'a SubstrateFile, epoch: u32) -> SubstrateResult<Self> {
        if let Err(e) = announce_remap_epoch(sub, epoch) {
            // Aucune zone n'a pu etre touchee : par definition l'annonce
            // precede toute mutation. C'est donc `AucuneMutation`, et la
            // bonne reprise est l'effacement.
            abandonner_annonce(sub, epoch, PhaseRepermutation::AucuneMutation, &e);
            return Err(e);
        }
        Ok(Self::neuve(CibleAnnonce::Directe(sub), epoch))
    }

    /// Variante pour les appelants qui partagent le substrat derriere un
    /// `Mutex` et ne doivent pas le tenir verrouille pendant les phases.
    /// Le verrou n'est pris que le temps d'ecrire l'annonce, puis a la
    /// resolution.
    pub fn annoncer_partagee(
        sub: std::sync::Arc<parking_lot::Mutex<SubstrateFile>>,
        epoch: u32,
    ) -> SubstrateResult<Self> {
        {
            let g = sub.lock();
            // Meme discipline que `annoncer` — voir sa doc.
            if let Err(e) = announce_remap_epoch(&g, epoch) {
                abandonner_annonce(&g, epoch, PhaseRepermutation::AucuneMutation, &e);
                return Err(e);
            }
        }
        Ok(Self::neuve(CibleAnnonce::Partagee(sub), epoch))
    }

    fn neuve(cible: CibleAnnonce<'a>, epoch: u32) -> Self {
        Self {
            cible,
            epoch,
            phase: std::sync::atomic::AtomicU8::new(PhaseRepermutation::AucuneMutation.code()),
            resolue: false,
        }
    }

    /// Rejoint le substrat pour une resolution EXPLICITE
    /// ([`Self::confirmer`], [`Self::abandonner`]) — **attente illimitee**.
    ///
    /// ══ POURQUOI CELLE-CI N'EST PAS BORNEE (revue #7) ══
    ///
    /// `confirmer` est appele APRES la barriere de durabilite : les zones
    /// sont permutees ET durables, la repermutation a REUSSI. Renoncer la
    /// pour cause de contention transformerait un succes en base qui
    /// refuse de se rouvrir — c'est-a-dire produirait exactement la panne
    /// que toute cette garde existe pour empecher. Un teneur legitime qui
    /// depasse le delai est rare, mais il ne doit pas etre puni.
    ///
    /// Et l'appelant controle ces deux points : ils ne sont pas dans un
    /// destructeur, ils peuvent attendre. Quatorze lignes plus loin,
    /// `writer.rs` reprend d'ailleurs le meme mutex par un `lock()` nu :
    /// si l'attente etait intolerable ici, elle le serait la aussi.
    fn avec_substrat<R>(&self, f: impl FnOnce(&SubstrateFile) -> R) -> R {
        match &self.cible {
            CibleAnnonce::Directe(s) => f(s),
            CibleAnnonce::Partagee(m) => {
                let g = m.lock();
                f(&g)
            }
        }
    }

    /// Rejoint le substrat pour la resolution IMPLICITE ([`Drop`]) —
    /// **attente bornee**, rend `None` si le verrou reste pris.
    ///
    /// Ici l'arbitrage s'inverse : un destructeur ne doit jamais pouvoir
    /// figer un fil, et il n'a pas d'appelant a qui rendre la main. On
    /// echange un gel silencieux contre une annonce laissee et un message
    /// explicite. Voir [`CibleAnnonce::Partagee`].
    fn avec_substrat_borne<R>(&self, f: impl FnOnce(&SubstrateFile) -> R) -> Option<R> {
        match &self.cible {
            CibleAnnonce::Directe(s) => Some(f(s)),
            CibleAnnonce::Partagee(m) => match m.try_lock_for(DELAI_REPRISE_VERROU) {
                Some(g) => Some(f(&g)),
                None => {
                    tracing::error!(
                        epoch = self.epoch,
                        delai_ms = DELAI_REPRISE_VERROU.as_millis() as u64,
                        phase = ?self.phase_courante(),
                        "RESOLUTION D'ANNONCE ABANDONNEE : le verrou du substrat est reste pris \
                         au-dela du delai. L'annonce est LAISSEE POSEE — la base refusera de se \
                         rouvrir. C'est un defaut de l'appelant : il laisse mourir une garde en \
                         tenant deja `substrate.lock()`. Renoncer vaut mieux que figer le fil."
                    );
                    None
                }
            },
        }
    }

    fn phase_courante(&self) -> PhaseRepermutation {
        PhaseRepermutation::depuis_code(self.phase.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// A appeler **sur la ligne meme** de la premiere ecriture d'octet.
    pub fn premiere_mutation(&self) {
        self.phase.store(
            PhaseRepermutation::AvantBarriere.code(),
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// A appeler juste apres le msync/fsync qui rend les zones durables.
    pub fn barriere_franchie(&self) {
        self.phase.store(
            PhaseRepermutation::ApresBarriere.code(),
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// CONFIRME la repermutation, et **gere son propre echec**.
    ///
    /// C'est ce qui ferme le bloquant de la revue #3 par le type plutot
    /// que par la vigilance : la confirmation ne peut plus etre appelee
    /// « a cote » de la region gardee, puisqu'elle EST la resolution.
    pub fn confirmer(mut self) -> SubstrateResult<()> {
        self.resolue = true;
        let epoch = self.epoch;
        let issue = self.avec_substrat(|s| commit_remap_epoch(s, epoch));
        if let Err(e) = &issue {
            let phase = self.phase_courante();
            self.avec_substrat(|s| abandonner_annonce(s, epoch, phase, e));
        }
        issue
    }

    /// ABANDONNE la passe : le ternaire tranche selon la phase atteinte.
    pub fn abandonner(mut self, cause: &SubstrateError) {
        self.resolue = true;
        let (epoch, phase) = (self.epoch, self.phase_courante());
        self.avec_substrat(|s| abandonner_annonce(s, epoch, phase, cause));
    }
}

impl Drop for AnnonceRemap<'_> {
    /// LE FILET. Une annonce qui arrive ici non resolue vient d'un chemin
    /// qui a oublie de trancher — ou d'un `?` qui a saute par-dessus.
    /// Plutot que de laisser `pending` pose (et donc une base non
    /// ouvrable), on applique le meme ternaire, en journalisant que
    /// personne n'avait tranche.
    fn drop(&mut self) {
        if self.resolue {
            return;
        }
        let (epoch, phase) = (self.epoch, self.phase_courante());
        let cause = SubstrateError::Internal(format!(
            "annonce de repermutation epoch {epoch} liberee sans resolution explicite : \
             un chemin a oublie d'appeler `confirmer` ou `abandonner`. La garde tranche \
             a sa place (phase {phase:?})"
        ));
        // BORNEE : un destructeur ne doit jamais pouvoir figer un fil.
        self.avec_substrat_borne(|s| abandonner_annonce(s, epoch, phase, &cause));
    }
}

/// Que faire quand une passe de repermutation ECHOUE apres l'ANNONCE —
/// **implementation unique**, partagee par le chemin VIF et le chemin de
/// REJEU.
///
/// ══ POURQUOI ELLE EST ICI ET PLUS DANS `Writer` (revue #5) ══
///
/// Elle etait une methode de `Writer`. Quand le chemin de REJEU s'est mis
/// a poser une annonce (revue #4), il n'a pas herite de la discipline qui
/// va avec : aucun `clear_pending_remap_epoch` nulle part dans
/// `replay.rs`. Un `ensure_room` en echec y laissait donc `pending` pose
/// sur des zones PROUVEES intactes — et comme le rejeu tourne A
/// L'OUVERTURE, un ENOSPC passager au demarrage briquait definitivement
/// une base qu'il suffisait avant de reessayer.
///
/// C'etait, mot pour mot, ce que la revue #2 avait fait ecrire dans ce
/// meme module : « corriger le refus sans cabler cet effacement, c'est
/// transformer une correction en panne ». La lecon etait juste ; elle
/// n'avait ete appliquee qu'a un seul des deux chemins. Une regle de
/// surete qui existe en deux exemplaires finit par n'exister qu'a moitie.
///
/// Voir [`PhaseRepermutation`] pour le raisonnement ternaire.
pub fn abandonner_annonce(
    sub: &SubstrateFile,
    epoch: u32,
    phase: PhaseRepermutation,
    cause: &SubstrateError,
) {
    use PhaseRepermutation as P;
    let base = sub.path().to_path_buf();
    match phase {
        // ── ÉTAT CONNU INTACT : on efface, la base reste ouvrable ──
        P::AucuneMutation => match clear_pending_remap_epoch(sub, epoch) {
            Ok(()) => tracing::warn!(
                epoch,
                base = %base.display(),
                cause = %cause,
                cote_barriere = "AUCUNE_MUTATION",
                "repermutation abandonnee AVANT toute mutation de zone — les donnees sont \
                 intactes, l'annonce est effacee, la base reste parfaitement ouvrable"
            ),
            Err(e) => tracing::error!(
                epoch,
                base = %base.display(),
                cause = %cause,
                erreur_effacement = %e,
                cote_barriere = "AUCUNE_MUTATION",
                reprise = %consigne_reparation(&base, "zones-intactes"),
                "repermutation abandonnee avant toute mutation, mais l'EFFACEMENT de l'annonce \
                 a echoue : la base refusera de se rouvrir alors que ses donnees sont INTACTES. \
                 Lancez la commande du champ `reprise`."
            ),
        },

        // ── ÉTAT INDÉTERMINÉ : on laisse, la base doit refuser ──
        P::AvantBarriere => tracing::error!(
            epoch,
            base = %base.display(),
            cause = %cause,
            cote_barriere = "AVANT",
            "repermutation interrompue APRES la premiere mutation de zone et AVANT la barriere \
             de durabilite : l'etat des zones noeuds/aretes est INDETERMINE. L'annonce est \
             LAISSEE EN PLACE — la base refusera de se rouvrir, ce qui est le comportement \
             voulu. N'APPLIQUEZ AUCUNE REPRISE : les deux hypotheses sont fausses la moitie du \
             temps. Restaurez une sauvegarde."
        ),

        // ── ÉTAT DÉTERMINÉ, VALEUR OPPOSÉE : on CONFIRME ──
        //
        // La barriere a reussi : les zones SONT permutees et durables. La
        // verite durable est `committed = epoch`, et le seul echec
        // possible ici est celui de l'ecriture de cette verite — donc on
        // la reessaie. Une passe de plus coute un write de 20 octets ; ne
        // pas la tenter coute une base saine immobilisee.
        P::ApresBarriere => match commit_remap_epoch(sub, epoch) {
            Ok(()) => tracing::error!(
                epoch,
                base = %base.display(),
                cause = %cause,
                cote_barriere = "APRES",
                "repermutation interrompue APRES la barriere de durabilite : les zones SONT \
                 permutees et durables. L'epoch a ete confirmee au second essai — la base reste \
                 OUVRABLE et COHERENTE. Les side-cars restes en retard seront rattrapes par le \
                 rejeu (chacun se garde par l'epoch de son en-tete)."
            ),
            Err(e2) => tracing::error!(
                epoch,
                base = %base.display(),
                cause = %cause,
                erreur_confirmation = %e2,
                cote_barriere = "APRES",
                reprise = %consigne_reparation(&base, "zones-permutees"),
                "repermutation interrompue APRES la barriere ET la confirmation a echoue deux \
                 fois. Les zones SONT permutees et durables — l'etat est DETERMINE, pas \
                 indetermine. La base va refuser de se rouvrir. Lancez la commande du champ \
                 `reprise`. SURTOUT PAS le verdict `zones-intactes` : il ferait RE-APPLIQUER la \
                 permutation sur des zones deja permutees."
            ),
        },
    }
}

/// Nom de la commande de reparation — **source unique** citee par le
/// refus d'ouverture, par les journaux du writer, et par la commande
/// elle-meme.
///
/// (Revue #4) Deux journaux du handler renvoyaient a
/// `OBRAIN_REMAP_REPRISE=<base>:<epoch>:intactes|permutees`, une variable
/// SUPPRIMEE par le lot precedent, avec un vocabulaire qui ne
/// correspondait meme pas aux verdicts du CLI. Une consigne morte est
/// pire qu'une absence de consigne : elle envoie l'operateur chercher un
/// dispositif qui n'existe pas, le jour ou il est presse. Les messages
/// sont desormais CONSTRUITS par les fonctions ci-dessous, donc ils ne
/// peuvent plus deriver du nom reel de la commande.
pub const COMMANDE_REPARATION: &str = "obrain repair";

/// Consigne de DIAGNOSTIC (ne modifie rien) pour cette base.
pub fn consigne_diagnostic(base: &std::path::Path) -> String {
    format!("{COMMANDE_REPARATION} {} --dry-run", base.display())
}

/// Consigne de REPARATION pour cette base et ce verdict.
///
/// `verdict` doit etre l'un des noms que le CLI accepte —
/// `zones-intactes` ou `zones-permutees`. Les deux commandent des
/// actions OPPOSEES.
pub fn consigne_reparation(base: &std::path::Path, verdict: &str) -> String {
    format!(
        "{COMMANDE_REPARATION} {} --apply --verdict {verdict}",
        base.display()
    )
}

/// Nom du temporaire de repermutation d'une colonne, **date par
/// l'epoch** — `<colonne>.remap.<epoch>.tmp`.
///
/// ══ POURQUOI LA DATE FAIT PARTIE DU NOM (revue #4, 2026-08-24) ══
///
/// Le temporaire etait nomme `<colonne>.remap.tmp`, sans epoch. Il
/// n'est efface que par le remap SUIVANT de la meme colonne, ou par
/// [`nettoyer_temporaires_remap`] lors d'une ouverture REUSSIE — or une
/// base `pending > committed` **ne s'ouvre pas**. Le temoin survivait
/// donc exactement la ou il allait etre lu, et un temoin HERITE d'une
/// passe precedente etait compte comme une preuve sur la passe EN COURS.
/// `obrain repair` en tirait « LA BARRIERE A ETE FRANCHIE » et
/// recommandait le verdict qui corrompt.
///
/// L'epoch est mise dans le NOM plutot que lue dans l'en-tete du
/// temporaire : l'en-tete est ecrit EN DERNIER, donc un temporaire
/// interrompu tot n'en a pas. Le nom, lui, existe des la creation.
pub fn nom_temporaire_remap(colonne: &str, epoch: u32) -> String {
    format!("{colonne}.remap.{epoch}.tmp")
}

/// Reconnait un temporaire de repermutation et rend son epoch.
///
/// * `Some(Some(e))` — temporaire date de l'epoch `e` ;
/// * `Some(None)` — temporaire au format HERITE, sans date. Il ne peut
///   servir de preuve pour aucune epoch : traite comme anterieur ;
/// * `None` — ce n'est pas un temporaire de repermutation.
pub fn epoch_du_temporaire(nom: &str) -> Option<Option<u32>> {
    if !nom.ends_with(".tmp") || !nom.contains(".remap.") {
        return None;
    }
    let sans_suffixe = nom.strip_suffix(".tmp")?;
    match sans_suffixe.rsplit_once(".remap.") {
        // `<colonne>.remap.<epoch>` — le seul format date.
        Some((_, reste)) if !reste.is_empty() => Some(reste.parse::<u32>().ok()),
        _ => Some(None),
    }
}

/// Efface TOUS les temporaires de repermutation d'une colonne, quelle
/// que soit leur epoch.
///
/// Appele avant de batir un nouveau temporaire. Effacer seulement celui
/// de l'epoch courante laisserait s'accumuler ceux des passes
/// precedentes sur un processus long-vivant dont le warden retente —
/// c'est exactement la situation qui a produit le faux diagnostic.
pub fn effacer_temporaires_de_colonne(sub: &SubstrateFile, colonne: &str) {
    let Ok(entrees) = std::fs::read_dir(sub.path()) else {
        return;
    };
    let prefixe = format!("{colonne}.remap.");
    for entree in entrees.flatten() {
        let chemin = entree.path();
        let Some(nom) = chemin.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // `starts_with("<colonne>.remap.")` couvre les DEUX formes : la
        // forme datee `<colonne>.remap.<epoch>.tmp` et la forme heritee
        // `<colonne>.remap.tmp`. Une seconde clause pour cette derniere
        // serait morte (revue #5, mutant equivalent).
        if nom.ends_with(".tmp") && nom.starts_with(prefixe.as_str()) {
            let _ = std::fs::remove_file(&chemin);
        }
    }
}

/// Efface les temporaires `<colonne>.remap.<epoch>.tmp` laisses par un remap
/// interrompu (revue #3, dette 2).
///
/// Ils sont SANS DANGER — la bascule se fait par `rename`, donc un
/// temporaire orphelin n'a jamais ete vu par personne, et l'enumeration
/// du rejeu les exclut deja. Mais ils sont loin d'etre SANS COUT : ils
/// n'etaient effaces qu'au remap SUIVANT de la MEME colonne, donc un
/// crash mi-remap laissait durablement un doublon de la colonne sur le
/// disque — 3,8 Go pour `embedding_vec` en production.
///
/// Appele a l'ouverture, quand aucun remap ne peut etre en cours. Tout
/// echec est journalise et avale : ne pas reussir a liberer de la place
/// ne doit pas empecher une base saine de s'ouvrir.
pub fn nettoyer_temporaires_remap(sub: &SubstrateFile) {
    let Ok(entrees) = std::fs::read_dir(sub.path()) else {
        return;
    };
    for entree in entrees.flatten() {
        let chemin = entree.path();
        let Some(nom) = chemin.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if epoch_du_temporaire(nom).is_none() {
            continue;
        }
        let taille = entree.metadata().map(|m| m.len()).unwrap_or(0);
        match std::fs::remove_file(&chemin) {
            Ok(()) => tracing::warn!(
                fichier = nom,
                octets = taille,
                "temporaire de repermutation orphelin efface a l'ouverture — trace d'un remap \
                 interrompu ; la colonne vivante, elle, est intacte (bascule par rename)"
            ),
            Err(e) => tracing::warn!(fichier = nom, erreur = %e,
                "temporaire de repermutation orphelin non effacable — occupe de l'espace disque"),
        }
    }
}

/// Efface une annonce sans rien confirmer — utilisé quand la passe
/// échoue AVANT d'avoir touché la moindre zone, cas où l'état est connu
/// intact et où laisser un `pending` ferait refuser le prochain rejeu.
pub fn clear_pending_remap_epoch(sub: &SubstrateFile, epoch: u32) -> SubstrateResult<()> {
    let mut state = lire_etat_epoch(sub)?;
    if state.pending == epoch {
        state.pending = 0;
        write_epoch_state(sub, state)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tier0 — remap de la table slot_to_offset (valeurs = slots nœuds)
// ---------------------------------------------------------------------------

/// Remappe la table `slot_to_offset` de la zone tier0 (T16.5). Les
/// VALEURS de la table sont des slots nœuds ; l'ordre dense des records
/// tier0/1/2 ne change pas — seul tier0 est réécrit. Out-of-place via
/// [`crate::tier_persist::write_tier_zone_with_remap_epoch`] (en-tête CRC
/// écrit en dernier), gardé par l'epoch d'en-tête (`_reserved[0]`).
///
/// Utilisé par les DEUX chemins (vif et rejeu) : la zone tier n'est
/// jamais tenue ouverte par le store — le disque fait foi dans les deux
/// cas, et `read_tier_zone` valide le CRC (invalide ⇒ « absent » ⇒ le
/// chemin de rebuild existant prend le relais).
pub fn remap_tier0_slot_table(
    sub: &SubstrateFile,
    old_to_new: &[u32],
    epoch: u32,
) -> SubstrateResult<crate::vec_column::VecRemapOutcome> {
    use crate::tier_persist::{HEADER_SIZE, TierMagic, TierPersistHeader};
    use crate::vec_column::VecRemapOutcome as Outcome;

    let mut zf = sub.open_zone(Zone::Tier0)?;
    let slice = zf.as_slice();
    if slice.len() < HEADER_SIZE {
        return Ok(Outcome::Absent);
    }
    let Ok(header) = bytemuck::try_pod_read_unaligned::<TierPersistHeader>(&slice[..HEADER_SIZE])
    else {
        return Ok(Outcome::Absent);
    };
    if header._reserved[0] >= epoch {
        return Ok(Outcome::AlreadyApplied);
    }
    let Some((n_slots, Some(s2o), records)) =
        crate::tier_persist::read_tier_zone::<crate::tiers::Tier0>(&zf, TierMagic::Tier0)?
    else {
        return Ok(Outcome::Absent);
    };
    let mapped = map_slot_values(&s2o, old_to_new);
    crate::tier_persist::write_tier_zone_with_remap_epoch(
        &mut zf,
        TierMagic::Tier0,
        n_slots,
        Some(&mapped),
        &records,
        epoch,
    )?;
    Ok(Outcome::Applied)
}

// ---------------------------------------------------------------------------
// Rejeu — remap fichier de tous les side-cars à en-tête
// ---------------------------------------------------------------------------

/// Chemin REJEU : remappe tous les side-cars slot-indexés à en-tête
/// retrouvables sur disque (tier0, colonnes de degrés, colonnes vec
/// nœuds, colonnes blob nœuds). Chaque side-car se garde par son propre
/// epoch d'en-tête (`>= epoch` ⇒ skip) et par son CRC (invalide ⇒ laissé
/// « absent » — fail-safe). Les zones engram ne sont PAS traitées ici :
/// leur remap passe par des enregistrements WAL absolus rejoués par leurs
/// propres bras (voir `crate::wal::WalPayload::HilbertRepermuteV2`).
pub fn remap_sidecar_files_for_replay(
    sub: &SubstrateFile,
    old_to_new: &[u32],
    epoch: u32,
) -> SubstrateResult<()> {
    use crate::vec_column::VecRemapOutcome as Outcome;

    let mut applied = 0usize;
    let mut skipped = 0usize;
    let mut absent = 0usize;
    let mut tally = |name: &str, outcome: Outcome| match outcome {
        Outcome::Applied => {
            applied += 1;
            tracing::debug!(target: "substrate::slot_remap", side_car = name, epoch, "remap applique au rejeu");
        }
        Outcome::AlreadyApplied => skipped += 1,
        Outcome::Absent => absent += 1,
    };

    tally("tier0", remap_tier0_slot_table(sub, old_to_new, epoch)?);

    let dir = sub.path().to_path_buf();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        // Ordre déterministe pour des logs et des rejeux reproductibles.
        let mut names: Vec<String> = rd
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_owned))
            .collect();
        names.sort();
        for name in &names {
            if name.starts_with("substrate.degrees.node") && name.ends_with(".u32") {
                tally(
                    name,
                    crate::degree_column::remap_node_file_for_replay(sub, name, old_to_new, epoch)?,
                );
            } else if let Some(spec) = crate::vec_column::VecColSpec::parse_filename(name)
                && spec.entity_kind == crate::vec_column::EntityKind::Node
            {
                tally(
                    name,
                    crate::vec_column::remap_node_file_for_replay(sub, spec, old_to_new, epoch)?,
                );
            } else if let Some(spec) = crate::blob_column::BlobColSpec::parse_idx_filename(name)
                && spec.entity_kind == crate::vec_column::EntityKind::Node
            {
                tally(
                    name,
                    crate::blob_column::remap_node_file_for_replay(sub, spec, old_to_new, epoch)?,
                );
            }
        }
    }

    tracing::info!(
        target: "substrate::slot_remap",
        epoch,
        applied,
        deja_appliques = skipped,
        absents = absent,
        "rejeu remap side-cars"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Validation de permutation (I1)
// ---------------------------------------------------------------------------

/// Vérifie que `old_to_new` est une bijection totale de `[0, n)` sur
/// lui-même avec `0 → 0` (slot sentinelle épinglé). O(n) temps, O(n) bits.
pub fn validate_permutation(old_to_new: &[u32]) -> SubstrateResult<()> {
    let n = old_to_new.len();
    if n == 0 {
        return Ok(());
    }
    if old_to_new[0] != 0 {
        return Err(SubstrateError::Internal(format!(
            "slot_remap: la sentinelle doit rester épinglée (0 → {}, attendu 0 → 0)",
            old_to_new[0]
        )));
    }
    let mut seen = vec![false; n];
    for (old, &new) in old_to_new.iter().enumerate() {
        let Some(cell) = seen.get_mut(new as usize) else {
            return Err(SubstrateError::Internal(format!(
                "slot_remap: image hors domaine ({old} → {new}, domaine [0, {n}))"
            )));
        };
        if *cell {
            return Err(SubstrateError::Internal(format!(
                "slot_remap: permutation non injective (slot cible {new} atteint deux fois)"
            )));
        }
        *cell = true;
    }
    // n images distinctes dans [0, n) ⇒ surjection acquise.
    Ok(())
}

/// Construit la permutation totale équivalente à une liste de relocations
/// partielle `(old, new)` (compact_community) : identité partout, puis
/// `old ↔ new` échangés. Les slots `new` sont, par contrat du compacteur,
/// une plage fraîche en queue (jamais occupée) : l'échange donne bien
/// « la donnée suit le nœud » (new reçoit la donnée de old, old reçoit le
/// contenu absent/zéro qu'avait new).
///
/// `domain` doit couvrir `max(new) + 1` (le `new_node_hw` du compacteur).
pub fn permutation_from_relocations(
    relocations: &[(u32, u32)],
    domain: u32,
) -> SubstrateResult<Vec<u32>> {
    let n = domain as usize;
    let mut perm: Vec<u32> = (0..domain).collect();
    for &(old, new) in relocations {
        let (o, nw) = (old as usize, new as usize);
        if o >= n || nw >= n {
            return Err(SubstrateError::Internal(format!(
                "slot_remap: relocation ({old} → {new}) hors domaine [0, {domain})"
            )));
        }
        perm.swap(o, nw);
    }
    validate_permutation(&perm)?;
    Ok(perm)
}

// ---------------------------------------------------------------------------
// Primitives de permutation out-of-place (I2, I5)
// ---------------------------------------------------------------------------

/// Nombre de slots que doit couvrir la région de destination pour ne
/// perdre aucune donnée : la plus haute image d'un slot source NON NUL,
/// sans jamais rétrécir la couverture existante.
pub fn required_slots_after_permute(payload: &[u8], stride: usize, old_to_new: &[u32]) -> usize {
    let src_slots = payload.len() / stride;
    let mut max_target: Option<u32> = None;
    for old in 0..src_slots.min(old_to_new.len()) {
        let chunk = &payload[old * stride..(old + 1) * stride];
        if chunk.iter().any(|b| *b != 0) {
            let t = old_to_new[old];
            max_target = Some(max_target.map_or(t, |m: u32| m.max(t)));
        }
    }
    match max_target {
        None => src_slots,
        Some(m) => (m as usize + 1).max(src_slots),
    }
}

/// Permute un payload dense slot-indexé (stride constant) out-of-place,
/// en allouant le résultat.
///
/// Réservé aux side-cars dont la taille rend l'allocation négligeable —
/// répertoire blob (16 o/slot) et colonnes de degrés (8 o/slot). Les
/// colonnes vectorielles, elles, passent par un fichier temporaire +
/// `rename` (voir `VecColumnWriter::remap_node_slots`) : sur plusieurs Go,
/// cette allocation serait un pic anonyme inacceptable.
///
/// Les sources au-delà du domaine de `old_to_new` restent à leur place ;
/// les cibles sans source restent à zéro.
pub fn permute_dense_payload(payload: &[u8], stride: usize, old_to_new: &[u32]) -> Vec<u8> {
    debug_assert!(stride > 0);
    let src_slots = payload.len() / stride;
    let n = old_to_new.len();

    // Plus haute cible portant une donnée non nulle → taille de sortie.
    let mut max_target: Option<u32> = None;
    for old in 0..src_slots.min(n) {
        let chunk = &payload[old * stride..(old + 1) * stride];
        if chunk.iter().any(|b| *b != 0) {
            let t = old_to_new[old];
            max_target = Some(max_target.map_or(t, |m: u32| m.max(t)));
        }
    }
    // La sortie couvre au moins tous les slots sources (les slots au-delà
    // du domaine de la permutation sont conservés à l'identique — jamais
    // perdus), et s'étend jusqu'à la plus haute cible non nulle.
    let out_slots = match max_target {
        None => src_slots,
        Some(m) => (m as usize + 1).max(src_slots),
    };

    let mut out = vec![0u8; out_slots * stride];
    for old in 0..src_slots {
        // Identité pour les slots hors domaine.
        let new = if old < n {
            old_to_new[old] as usize
        } else {
            old
        };
        if new >= out_slots {
            continue; // cible au-delà : source forcément nulle (cf. max_target)
        }
        out[new * stride..(new + 1) * stride]
            .copy_from_slice(&payload[old * stride..(old + 1) * stride]);
    }
    out
}

/// Réécrit des valeurs-slots (u32) via `old_to_new`, en place dans le Vec
/// rendu. Les valeurs hors domaine sont conservées telles quelles (elles
/// désignent des slots que la permutation ne couvre pas — p. ex. un
/// side-car plus ancien que la borne `hw` de la passe).
pub fn map_slot_values(values: &[u32], old_to_new: &[u32]) -> Vec<u32> {
    values
        .iter()
        .map(|&v| old_to_new.get(v as usize).copied().unwrap_or(v))
        .collect()
}

// ---------------------------------------------------------------------------
// Tests unitaires
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_identity_and_swaps() {
        validate_permutation(&[]).unwrap();
        validate_permutation(&[0]).unwrap();
        validate_permutation(&[0, 1, 2, 3]).unwrap();
        validate_permutation(&[0, 3, 1, 2]).unwrap();
    }

    #[test]
    fn validate_rejects_moved_sentinel() {
        assert!(validate_permutation(&[1, 0]).is_err());
    }

    #[test]
    fn validate_rejects_non_injective_and_out_of_domain() {
        assert!(validate_permutation(&[0, 1, 1]).is_err());
        assert!(validate_permutation(&[0, 5, 1]).is_err());
    }

    #[test]
    fn relocations_build_total_swap_permutation() {
        // hw=4, relocations 1→4, 2→5 dans un domaine étendu à 6.
        let perm = permutation_from_relocations(&[(1, 4), (2, 5)], 6).unwrap();
        assert_eq!(perm, vec![0, 4, 5, 3, 1, 2]);
    }

    #[test]
    fn permute_dense_moves_bytes_and_zeroes_sources() {
        // 3 slots de stride 2 : [aa][bb][cc], perm 0→0, 1→2, 2→1.
        let payload = [0xA, 0xA, 0xB, 0xB, 0xC, 0xC];
        let out = permute_dense_payload(&payload, 2, &[0, 2, 1]);
        assert_eq!(out, vec![0xA, 0xA, 0xC, 0xC, 0xB, 0xB]);
    }

    #[test]
    fn permute_dense_grows_for_targets_past_current_coverage() {
        // 2 slots écrits, permutation sur un domaine de 5 : slot 1 → 4.
        let payload = [0u8, 0, 0xB, 0xB];
        let out = permute_dense_payload(&payload, 2, &[0, 4, 2, 3, 1]);
        assert_eq!(out.len(), 10);
        assert_eq!(&out[8..10], &[0xB, 0xB]);
        assert!(out[..8].iter().all(|b| *b == 0));
    }

    #[test]
    fn permute_dense_zero_only_payload_keeps_coverage() {
        let payload = [0u8; 6];
        let out = permute_dense_payload(&payload, 2, &[0, 2, 1]);
        assert_eq!(out, vec![0u8; 6]);
    }

    #[test]
    fn map_slot_values_applies_permutation_to_values() {
        let mapped = map_slot_values(&[3, 0, 7, 1], &[0, 4, 2, 5]);
        // 3→5, 0→0, 7 hors domaine → conservé, 1→4.
        assert_eq!(mapped, vec![5, 0, 7, 4]);
    }

    #[test]
    fn permute_dense_identity_is_byte_noop() {
        let payload = [1u8, 2, 3, 4, 5, 6];
        let out = permute_dense_payload(&payload, 2, &[0, 1, 2]);
        assert_eq!(out, payload.to_vec());
    }

    // -----------------------------------------------------------------
    // T2 + convergence de rejeu (crash injecté entre deux remaps)
    // -----------------------------------------------------------------

    use crate::degree_column::DegreeColumn;
    use crate::record::{NodeRecord, PackedScarUtilAff, U48, f32_to_q1_15};
    use crate::tier_persist::{self, TierMagic};
    use crate::tiers::Tier0;
    use crate::vec_column::{
        EntityKind, VecColSpec, VecColumnReader, VecColumnWriter, VecDType, VecRemapOutcome,
    };
    use crate::wal::WalPayload;
    use crate::wal_io::SyncMode;
    use crate::writer::Writer;

    /// Collecteur de journal minimal — `tracing-subscriber` n'est pas une
    /// dependance de developpement de ce crate, et l'ajouter pour une
    /// assertion serait disproportionne. Une cinquantaine de lignes
    /// suffisent a capturer ce dont on a besoin : le niveau et le message.
    #[derive(Clone, Default)]
    struct JournalTest(std::sync::Arc<parking_lot::Mutex<Vec<(tracing::Level, String)>>>);

    impl JournalTest {
        fn contient(&self, niveau: tracing::Level, extrait: &str) -> bool {
            self.0
                .lock()
                .iter()
                .any(|(n, m)| *n == niveau && m.contains(extrait))
        }
        fn tout(&self) -> Vec<(tracing::Level, String)> {
            self.0.lock().clone()
        }
    }

    struct CapteurMessage<'a>(&'a mut Option<String>);
    impl tracing::field::Visit for CapteurMessage<'_> {
        fn record_debug(&mut self, champ: &tracing::field::Field, valeur: &dyn std::fmt::Debug) {
            if champ.name() == "message" {
                *self.0 = Some(format!("{valeur:?}"));
            }
        }
    }

    impl tracing::Subscriber for JournalTest {
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn event(&self, evenement: &tracing::Event<'_>) {
            let mut message = None;
            evenement.record(&mut CapteurMessage(&mut message));
            if let Some(m) = message {
                self.0.lock().push((*evenement.metadata().level(), m));
            }
        }
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
    }

    const N: u32 = 6;
    /// Bijection totale sur [0, 6), sentinelle épinglée : i → 6 - i.
    fn perm() -> Vec<u32> {
        vec![0, 5, 4, 3, 2, 1]
    }

    fn marked_node(marker: u16) -> NodeRecord {
        NodeRecord {
            label_bitset: 1,
            first_edge_off: U48::default(),
            first_prop_off: U48::default(),
            community_id: 1,
            energy: f32_to_q1_15(0.5),
            scar_util_affinity: PackedScarUtilAff::new(0, 0, 0, false).pack(),
            centrality_cached: marker,
            flags: 0,
            // (jalon 2) appartenance : fixture publique.
            tenant_id: 0,
            _reserve: 0,
        }
    }

    fn vec_spec() -> VecColSpec {
        VecColSpec {
            prop_key_id: 1,
            entity_kind: EntityKind::Node,
            dim: 4,
            dtype: VecDType::F32,
        }
    }

    /// Construit une base complète (nœuds marqués, veccol, degrés, tier0,
    /// engram), journalise la repermutation V2 (epoch 1) + les états
    /// absolus engram, fsync le WAL, puis « crashe » ENTRE deux remaps de
    /// zones : seul tier0 a été remappé.
    fn build_crashed_mid_remap(path: &std::path::Path) -> u64 {
        let sub = crate::file::SubstrateFile::create(path).unwrap();
        let writer = Writer::new(sub, SyncMode::EveryCommit).unwrap();
        for i in 1..N {
            writer.update_node(i, marked_node(i as u16)).unwrap();
        }
        let subh = writer.substrate();
        {
            let g = subh.lock();
            let mut w = VecColumnWriter::create(&g, vec_spec()).unwrap();
            for i in 1..N {
                let v = [i as f32; 4];
                w.write_slot(i, bytemuck::cast_slice(&v)).unwrap();
            }
            w.finalize().unwrap();

            let mut col = DegreeColumn::create(&g, N).unwrap();
            for i in 1..N {
                col.incr_out(i, i as i32);
            }
            col.persist_header_crc();
            col.msync().unwrap();

            let mut zf = g.open_zone(Zone::Tier0).unwrap();
            let s2o: Vec<u32> = (1..N).collect();
            let records: Vec<Tier0> = (1..N).map(|i| Tier0([i as u64, 0])).collect();
            tier_persist::write_tier_zone::<Tier0>(
                &mut zf,
                TierMagic::Tier0,
                N - 1,
                Some(&s2o),
                &records,
            )
            .unwrap();
        }
        writer.set_engram_members(1, vec![1, 2, 3]).unwrap();
        writer.commit().unwrap();
        writer.msync_zones().unwrap();

        // ── LE FLUSH « AVANT » ──
        //
        // C'est lui qui vide la fenetre de rejeu. En production il vient de
        // `SubstrateStore::repermuter_par_hilbert` ; ici on le simule, et on
        // RETIENT l'offset : c'est exactement `wal_offset_at_flush`, le point
        // d'ou le rejeu de production repartira.
        let baseline = writer.wal().offset();
        writer.note_durable_baseline(baseline);

        // ── Repermutation : journal V2 + états absolus, UN fsync ─────
        let p = perm();
        let mapped: Vec<u32> = [1u32, 2, 3].iter().map(|&m| p[m as usize]).collect();
        writer
            .append_payload(WalPayload::HilbertRepermuteV2 {
                permutation: p.clone(),
                epoch: 1,
                engram_members: vec![(1u16, mapped)],
                engram_bitsets: Vec::new(),
            })
            .unwrap();
        writer.fsync_wal().unwrap();

        // ── Crash injecté ENTRE deux remaps de zones : tier0 seulement ──
        {
            let g = subh.lock();
            let outcome = remap_tier0_slot_table(&g, &p, 1).unwrap();
            assert_eq!(outcome, crate::vec_column::VecRemapOutcome::Applied);
        }
        drop(writer); // crash : ni nœuds permutés, ni veccol/degrés/engram
        baseline
    }

    fn assert_converged(sub: &crate::file::SubstrateFile) {
        let p = perm();
        // Epoch durable écrit par le rejeu.
        assert_eq!(current_remap_epoch(sub).unwrap(), 1);
        // Nœuds permutés : marqueur i au slot p[i].
        let zf = sub.open_zone(Zone::Nodes).unwrap();
        for i in 1..N {
            let off = (p[i as usize] as usize) * NodeRecord::SIZE;
            let rec: &NodeRecord =
                bytemuck::from_bytes(&zf.as_slice()[off..off + NodeRecord::SIZE]);
            assert_eq!(rec.centrality_cached, i as u16, "nœud {i} au mauvais slot");
        }
        // Tier0 : PAS doublement appliqué (epoch 1 posé avant le crash).
        let zf0 = sub.open_zone(Zone::Tier0).unwrap();
        let (_, s2o, records) = tier_persist::read_tier_zone::<Tier0>(&zf0, TierMagic::Tier0)
            .unwrap()
            .expect("tier0 illisible");
        let s2o = s2o.unwrap();
        for (dense, i) in (1..N).enumerate() {
            assert_eq!(
                s2o[dense], p[i as usize],
                "tier0 divergent (double remap ?)"
            );
            assert_eq!(records[dense].0[0], i as u64, "données tier0 modifiées");
        }
        // Veccol : remappé par le rejeu, en-tête valide (CRC + epoch).
        let r = VecColumnReader::open(sub, vec_spec())
            .unwrap()
            .expect("veccol absent après rejeu");
        for i in 1..N {
            assert_eq!(
                r.read_slot_f32(p[i as usize]).unwrap(),
                &[i as f32; 4][..],
                "veccol : l'embedding du nœud {i} n'a pas suivi"
            );
        }
        assert_eq!(
            crate::vec_column::remap_node_file_for_replay(sub, vec_spec(), &p, 1).unwrap(),
            VecRemapOutcome::AlreadyApplied,
            "l'epoch d'en-tête veccol doit gater la ré-application"
        );
        // Degrés : suivis.
        let col = DegreeColumn::open(sub).unwrap().expect("degrés absents");
        for i in 1..N {
            assert_eq!(col.out_degree(p[i as usize]), i, "degré du nœud {i}");
        }
        // Engram : états absolus rejoués.
        let ezf = sub.open_zone(Zone::EngramMembers).unwrap();
        let members = crate::engram::EngramZone::members(&ezf, 1)
            .unwrap()
            .unwrap();
        let mut expected: Vec<u32> = [1u32, 2, 3].iter().map(|&m| p[m as usize]).collect();
        let mut got = members;
        expected.sort_unstable();
        got.sort_unstable();
        assert_eq!(got, expected, "membres engram non remappés");
    }

    /// T2 — crash entre deux remaps, puis rejeu DEPUIS L'OFFSET DE
    /// CHECKPOINT (le mode de production).
    ///
    /// La version precedente de ce test rejouait depuis 0. C'etait le
    /// defaut central releve en revue : la production rejoue depuis
    /// `wal_offset_at_flush`, et un rejeu partant d'AVANT un tri termine
    /// reecrit des identifiants pre-permutation dans une zone
    /// post-permutation. Le test part desormais de la baseline, et le
    /// writer REFUSE de trier quand cette fenetre n'est pas vide.
    #[test]
    fn t2_crash_entre_deux_remaps_puis_rejeu_converge() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("kb");
        let baseline = build_crashed_mid_remap(&path);
        assert!(baseline > 0, "la baseline de production n'est jamais nulle");

        // Rejeu 1 : applique la permutation + les side-cars manquants,
        // saute tier0 (deja a l'epoch 1), et confirme l'epoch.
        let sub = crate::file::SubstrateFile::open(&path).unwrap();
        crate::replay::replay_from(&sub, baseline).unwrap();
        assert_converged(&sub);
        drop(sub);

        // Rejeu 2 (repete, meme offset) : l'epoch confirme gate la
        // permutation ; les side-cars se re-verifient et ne bougent pas.
        // Etat STRICTEMENT identique — pas de double permutation.
        let sub = crate::file::SubstrateFile::open(&path).unwrap();
        crate::replay::replay_from(&sub, baseline).unwrap();
        assert_converged(&sub);
    }

    #[test]
    fn veccol_crash_mi_remap_reste_absent_jamais_faux() {
        // Simule un crash au MILIEU du remap veccol : payload partiellement
        // réécrit, en-tête encore ancien. Le rejeu doit constater le CRC
        // invalide, laisser la colonne « absente », et ne JAMAIS produire
        // un vecteur attribué à un autre nœud.
        let td = tempfile::tempdir().unwrap();
        let sub = crate::file::SubstrateFile::create(td.path()).unwrap();
        let spec = vec_spec();
        let mut w = VecColumnWriter::create(&sub, spec).unwrap();
        for i in 1..N {
            let v = [i as f32; 4];
            w.write_slot(i, bytemuck::cast_slice(&v)).unwrap();
        }
        w.finalize().unwrap();

        // Corruption mi-remap : un slot réécrit, en-tête inchangé.
        {
            let mut zf = sub.open_named_zone(&spec.filename()).unwrap();
            let stride = spec.slot_stride();
            let off = crate::vec_column::HEADER_SIZE + 2 * stride;
            let garbage = [9.9f32; 4];
            zf.as_slice_mut()[off..off + stride].copy_from_slice(bytemuck::cast_slice(&garbage));
            zf.msync().unwrap();
        }

        let outcome =
            crate::vec_column::remap_node_file_for_replay(&sub, spec, &perm(), 1).unwrap();
        assert_eq!(
            outcome,
            VecRemapOutcome::Absent,
            "un payload non vérifiable ne doit jamais être permuté"
        );
        assert!(
            VecColumnReader::open(&sub, spec).unwrap().is_none(),
            "colonne mi-remappée : absente détectable, jamais un faux résultat"
        );
    }

    #[test]
    fn remap_fichiers_double_application_est_gatee_par_epoch() {
        let td = tempfile::tempdir().unwrap();
        let sub = crate::file::SubstrateFile::create(td.path()).unwrap();
        let spec = vec_spec();
        let mut w = VecColumnWriter::create(&sub, spec).unwrap();
        for i in 1..N {
            let v = [i as f32; 4];
            w.write_slot(i, bytemuck::cast_slice(&v)).unwrap();
        }
        w.finalize().unwrap();
        let mut col = DegreeColumn::create(&sub, N).unwrap();
        col.incr_out(1, 7);
        col.persist_header_crc();
        col.msync().unwrap();
        drop(col);

        let p = perm();
        assert_eq!(
            crate::vec_column::remap_node_file_for_replay(&sub, spec, &p, 3).unwrap(),
            VecRemapOutcome::Applied
        );
        assert_eq!(
            crate::vec_column::remap_node_file_for_replay(&sub, spec, &p, 3).unwrap(),
            VecRemapOutcome::AlreadyApplied,
            "même epoch : skip"
        );
        assert_eq!(
            crate::vec_column::remap_node_file_for_replay(&sub, spec, &p, 2).unwrap(),
            VecRemapOutcome::AlreadyApplied,
            "epoch antérieur : skip"
        );
        assert_eq!(
            crate::degree_column::remap_node_file_for_replay(
                &sub,
                crate::degree_column::DEGREE_COLUMN_FILENAME,
                &p,
                3
            )
            .unwrap(),
            VecRemapOutcome::Applied
        );
        assert_eq!(
            crate::degree_column::remap_node_file_for_replay(
                &sub,
                crate::degree_column::DEGREE_COLUMN_FILENAME,
                &p,
                3
            )
            .unwrap(),
            VecRemapOutcome::AlreadyApplied
        );
        let col = DegreeColumn::open(&sub).unwrap().unwrap();
        assert_eq!(col.out_degree(p[1]), 7, "degré permuté une seule fois");
    }

    #[test]
    fn epoch_zone_roundtrip_et_corruption() {
        let td = tempfile::tempdir().unwrap();
        let sub = crate::file::SubstrateFile::create(td.path()).unwrap();
        assert_eq!(
            current_remap_epoch(&sub).unwrap(),
            0,
            "base neuve : epoch 0"
        );
        commit_remap_epoch(&sub, 7).unwrap();
        assert_eq!(current_remap_epoch(&sub).unwrap(), 7);

        // ── (2026-08-24, revue #3) CE TEST DISAIT L'INVERSE ──
        //
        // Il assertait « corruption => 0 (degradation sure, documentee) ».
        // Ce n'etait pas une degradation sure : c'etait un ORACLE QUI
        // ECHOUE EN MODE OUVERT. Relire `committed = 0` fait tomber le
        // bras `HilbertRepermuteV2` dans son cas 3 et RE-APPLIQUER la
        // permutation sur des zones deja permutees, avec des CRC valides
        // et sans un mot. Un seul octet suffisait.
        {
            let mut zf = sub.open_named_zone(REMAP_EPOCH_ZONE).unwrap();
            zf.as_slice_mut()[8] ^= 0xFF;
            zf.msync().unwrap();
        }
        let err = current_remap_epoch(&sub).expect_err(
            "une zone d'epoch presente et illisible ne doit JAMAIS se lire comme neuve",
        );
        assert!(
            matches!(err, SubstrateError::EpoqueRemapIllisible { .. }),
            "obtenu : {err:?}"
        );
    }

    /// PROPRIETE 1 — un echec APRES LA BARRIERE laisse la base OUVRABLE.
    ///
    /// ══ LE BLOQUANT DE LA REVUE #3, EN POSITIF ══
    ///
    /// `commit_remap_epoch` etait appelee APRES la region gardee, avec un
    /// `?` nu : son echec contournait `abandonner_annonce_remap`. Les
    /// zones etaient permutees ET durables — etat parfaitement DETERMINE
    /// — et pourtant l'ouverture refusait, sans rien journaliser qui dise
    /// de quel cote de la barriere on se trouvait. Puis la procedure de
    /// reprise documentee, qui ne connaissait que l'hypothese « zones
    /// intactes », effacait l'annonce : le rejeu ratait ses cas 1 et 2,
    /// tombait dans son cas 3, et RE-APPLIQUAIT la permutation.
    ///
    /// Le chemin de secours detruisait ce que la panne avait preserve.
    #[test]
    fn un_echec_apres_la_barriere_laisse_la_base_ouvrable_et_confirmee() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("kb");
        let writer = base_durable(&path);

        // Marqueurs AVANT : le nœud `i` porte `centrality_cached == i`.
        let avant = marqueurs(&writer);
        assert_eq!(
            avant,
            vec![0, 5000, 4000, 3000, 2000, 1000],
            "premisse : etat PRE-tri, marqueurs decroissants"
        );

        // Le premier `ensure_room` sur la zone d'epoch est celui de
        // l'ANNONCE : on le laisse passer (1 saut) pour viser celui de la
        // CONFIRMATION, apres la barriere.
        crate::writer::armer_echec_ensure_room(REMAP_EPOCH_ZONE, 1);
        let err = writer
            .bulk_sort_by_hilbert(N, 0, 8, 4)
            .expect_err("le failpoint doit faire echouer la confirmation");
        assert!(format!("{err}").contains("ENOSPC simule"), "{err}");
        assert!(
            crate::writer::echec_ensure_room_a_tire(),
            "le failpoint n'a jamais tire : la confirmation n'a pas ete atteinte"
        );

        // ── PREMISSE DECISIVE : LA BARRIERE A ETE FRANCHIE ──
        // Les marqueurs ont bouge : les zones SONT permutees. Sans cette
        // assertion, le test ne distinguerait pas ce cas de celui d'un
        // echec avant toute mutation, et serait vide.
        let apres = marqueurs(&writer);
        assert_eq!(
            apres,
            vec![0, 1000, 2000, 3000, 4000, 5000],
            "premisse DECISIVE : les marqueurs doivent avoir ete REORDONNES — sinon la \
             barriere n'a pas ete franchie et ce test ne couvre pas la fenetre POST-barriere"
        );
        drop(writer);

        // ── L'ASSERTION : la base s'ouvre, et elle est CONFIRMEE ──
        {
            let sub = crate::file::SubstrateFile::open(&path).unwrap();
            assert_eq!(
                epoque_indeterminee(&sub).unwrap(),
                None,
                "une base dont les zones sont permutees ET durables n'est pas indeterminee : \
                 la refuser immobilise une base saine, et la reprise documentee la corrompt"
            );
            assert_eq!(
                current_remap_epoch(&sub).unwrap(),
                1,
                "l'epoch doit avoir ete CONFIRMEE au second essai"
            );
        }
        let store = crate::store::SubstrateStore::open(&path)
            .expect("un echec APRES la barriere doit laisser la base parfaitement ouvrable");
        drop(store);

        // ── ET SURTOUT : la permutation n'a PAS ete appliquee deux fois ──
        let sub = crate::file::SubstrateFile::open(&path).unwrap();
        assert_eq!(
            marqueurs_de(&sub),
            apres,
            "la permutation a ete RE-APPLIQUEE a l'ouverture — c'est exactement la corruption \
             que la confirmation evite"
        );
    }

    /// PROPRIETE 3 — un echec PENDANT les mutations (avant la barriere)
    /// refuse, et le journal dit de quel cote de la barriere on est.
    #[test]
    fn un_echec_avant_la_barriere_refuse_et_nomme_le_cote() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("kb");
        let writer = base_durable(&path);

        // La zone Hilbert est agrandie en phase 6 : APRES l'ecriture des
        // nœuds (phase 4), AVANT le msync de la barriere (phase 7).
        crate::writer::armer_echec_ensure_room("substrate.hilbert", 0);
        let err = writer
            .bulk_sort_by_hilbert(N, 0, 8, 4)
            .expect_err("le failpoint doit faire echouer la phase 6");
        assert!(format!("{err}").contains("ENOSPC simule"), "{err}");
        assert!(
            crate::writer::echec_ensure_room_a_tire(),
            "le failpoint n'a jamais tire : la phase 6 n'a pas ete atteinte"
        );
        // Premisse : les zones ONT ete mutees (sinon on serait dans le cas
        // « aucune mutation », qui efface l'annonce).
        assert_eq!(
            marqueurs(&writer),
            vec![0, 1000, 2000, 3000, 4000, 5000],
            "premisse : la phase 4 doit avoir mute la zone nœuds (mais la barriere n'a PAS \
             ete franchie — c'est tout l'objet de ce test)"
        );
        drop(writer);

        let sub = crate::file::SubstrateFile::open(&path).unwrap();
        assert_eq!(
            epoque_indeterminee(&sub).unwrap(),
            Some(1),
            "un echec ENTRE la premiere mutation et la barriere laisse un etat INDETERMINE : \
             l'annonce doit rester posee"
        );
        drop(sub);
        let Err(err) = crate::store::SubstrateStore::open(&path) else {
            panic!("un etat indetermine doit faire REFUSER l'ouverture")
        };
        assert!(
            matches!(
                err,
                SubstrateError::RepermutationIndeterminee { epoch: 1, .. }
            ),
            "obtenu : {err:?}"
        );
        let msg = format!("{err}");
        assert!(
            msg.contains("INDETERMINE"),
            "le refus doit nommer la cause : {msg}"
        );
    }

    /// PROPRIETE 5 — une reparation appliquee a UNE base n'ouvre pas UNE
    /// AUTRE.
    ///
    /// La variable `OBRAIN_REMAP_OUVERTURE_FORCEE` portait l'epoch mais
    /// PAS la base. Or l'epoch est allouee PAR BASE et repart a 1 : `=1`
    /// pose pour reparer la base A ouvrait aussi, par ricochet, une base B
    /// jamais inspectee — et `from_substrate` monte les graphes nommes
    /// enfants dans le MEME processus, chacun avec son compteur reparti a
    /// 1 — la variable est SUPPRIMEE, et la reparation designe sa base par
    /// un chemin. Ce test verrouille l'absence de tout retour a une portee
    /// globale au processus.
    #[test]
    fn une_reparation_appliquee_a_une_base_n_ouvre_pas_une_autre() {
        let td = tempfile::tempdir().unwrap();
        let base_a = td.path().join("kb_a");
        let base_b = td.path().join("kb_b");
        for p in [&base_a, &base_b] {
            drop(base_durable(p));
            let sub = crate::file::SubstrateFile::open(p).unwrap();
            announce_remap_epoch(&sub, 1).unwrap();
        }

        // ── PREMISSE : SANS reparation, les DEUX refusent ──
        for p in [&base_a, &base_b] {
            assert!(
                crate::store::SubstrateStore::open(p).is_err(),
                "premisse : les deux bases doivent refuser sans reparation"
            );
        }

        // Reparation appliquee a A SEULEMENT.
        {
            let sub = crate::file::SubstrateFile::open(&base_a).unwrap();
            let (_, action) = appliquer_reparation(&sub, VerdictOperateur::ZonesIntactes).unwrap();
            assert_eq!(action, ActionReparation::AnnonceEffacee { epoch: 1 });
        }

        crate::store::SubstrateStore::open(&base_a).expect("la base REPAREE doit s'ouvrir");
        let Err(err_b) = crate::store::SubstrateStore::open(&base_b) else {
            panic!("une base JAMAIS DESIGNEE ne doit pas etre reparee par ricochet")
        };
        assert!(
            matches!(
                err_b,
                SubstrateError::RepermutationIndeterminee { epoch: 1, .. }
            ),
            "obtenu : {err_b:?}"
        );
    }

    /// PROPRIETE 6 — `--dry-run` et `--apply` partagent EXACTEMENT le
    /// meme diagnostic.
    ///
    /// Deux regles de detection pour la meme decision est le defaut
    /// structurel le plus frequent de ce chantier, et c'est toujours au
    /// point ou elles divergent que quelqu'un se perd. Ce test compare la
    /// valeur rendue par `diagnostiquer` a celle que `appliquer_reparation`
    /// declare avoir utilisee : quiconque ajoute un raccourci « on sait
    /// deja » dans le chemin d'application fait rougir cette assertion.
    #[test]
    fn le_dry_run_et_l_apply_partagent_le_meme_diagnostic() {
        let td = tempfile::tempdir().unwrap();

        // (a) base SAINE — l'apply ne doit rien faire, et voir le meme etat.
        let saine = td.path().join("kb_saine");
        drop(base_durable(&saine));
        {
            let sub = crate::file::SubstrateFile::open(&saine).unwrap();
            let vu_par_le_dry_run = diagnostiquer(&sub);
            assert!(matches!(vu_par_le_dry_run, DiagnosticRemap::Saine { .. }));
            let (vu_par_l_apply, action) =
                appliquer_reparation(&sub, VerdictOperateur::ZonesPermutees).unwrap();
            assert_eq!(
                vu_par_l_apply, vu_par_le_dry_run,
                "l'apply a decide sur un AUTRE diagnostic que le dry-run"
            );
            assert_eq!(
                action,
                ActionReparation::RienAFaire,
                "l'apply ne doit RIEN faire sur une base saine"
            );
        }

        // (b) base INTERROMPUE — meme diagnostic des deux cotes.
        let cassee = td.path().join("kb_cassee");
        drop(base_durable(&cassee));
        {
            let sub = crate::file::SubstrateFile::open(&cassee).unwrap();
            announce_remap_epoch(&sub, 1).unwrap();
            let vu_par_le_dry_run = diagnostiquer(&sub);
            assert!(matches!(
                vu_par_le_dry_run,
                DiagnosticRemap::RepermutationInterrompue { epoch: 1, .. }
            ));
            let (vu_par_l_apply, action) =
                appliquer_reparation(&sub, VerdictOperateur::ZonesPermutees).unwrap();
            assert_eq!(
                vu_par_l_apply, vu_par_le_dry_run,
                "l'apply a decide sur un AUTRE diagnostic que le dry-run"
            );
            assert_eq!(action, ActionReparation::EpochConfirmee { epoch: 1 });
        }

        // (c) ORACLE ILLISIBLE — les DEUX refusent, aucun ne devine.
        let illisible = td.path().join("kb_illisible");
        drop(base_durable(&illisible));
        {
            let sub = crate::file::SubstrateFile::open(&illisible).unwrap();
            announce_remap_epoch(&sub, 1).unwrap();
            let mut zf = sub.open_named_zone(REMAP_EPOCH_ZONE).unwrap();
            zf.as_slice_mut()[16] ^= 0x01;
            zf.msync().unwrap();
            zf.fsync().unwrap();
        }
        let sub = crate::file::SubstrateFile::open(&illisible).unwrap();
        assert!(
            matches!(diagnostiquer(&sub), DiagnosticRemap::OracleIllisible { .. }),
            "le dry-run doit REFUSER de diagnostiquer une zone qu'il n'a pas su lire — un \
             diagnostic « base saine » y serait pire que pas de diagnostic du tout"
        );
        for verdict in [
            VerdictOperateur::ZonesIntactes,
            VerdictOperateur::ZonesPermutees,
        ] {
            let err = appliquer_reparation(&sub, verdict)
                .expect_err("l'apply ne doit JAMAIS agir sur un oracle illisible");
            assert!(
                matches!(err, SubstrateError::EpoqueRemapIllisible { .. }),
                "obtenu : {err:?}"
            );
        }
    }

    /// PROPRIETE 7 — le refus NOMME la commande de diagnostic.
    ///
    /// Le message de refus fait partie du mecanisme de surete : c'est le
    /// canal qui achemine l'operateur vers le chemin sur. S'il derive —
    /// commande renommee, chemin oublie, formulation videe — plus rien ne
    /// le signale, et on retombe sur le motif de fond : un dispositif
    /// correct dont le seul lien avec son utilisateur s'est rompu en
    /// silence. Cette assertion transforme l'intention en propriete.
    #[test]
    fn le_refus_nomme_la_commande_de_diagnostic() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("kb");
        drop(base_durable(&path));
        {
            let sub = crate::file::SubstrateFile::open(&path).unwrap();
            announce_remap_epoch(&sub, 1).unwrap();
        }
        let Err(err) = crate::store::SubstrateStore::open(&path) else {
            panic!("premisse : cette base doit refuser de s'ouvrir")
        };
        let msg = format!("{err}");
        // (Revue #5) On compare a la CONSTANTE PARTAGEE, plus a un
        // litteral jumeau : cette assertion comparait un litteral a son
        // propre jumeau, donc ne verrouillait rien. Le lien avec la vraie
        // sous-commande clap est fait cote CLI par
        // `le_nom_imprime_est_celui_de_la_sous_commande_reelle`.
        assert!(
            msg.contains(COMMANDE_REPARATION) && msg.contains("--dry-run"),
            "le refus doit nommer la COMMANDE a lancer ({COMMANDE_REPARATION}), sinon \
             l'operateur ira chercher un instrument contondant. Obtenu : {msg}"
        );
        assert!(
            msg.contains(&path.display().to_string()),
            "le refus doit nommer LA BASE concernee — un processus monte plusieurs bases, et \
             les graphes nommes enfants ont chacun leur compteur d'epoch reparti a 1. \
             Obtenu : {msg}"
        );
        assert!(
            msg.contains("SANS RIEN MODIFIER"),
            "le refus doit dire que le diagnostic ne modifie rien, sinon personne ne l'ose. \
             Obtenu : {msg}"
        );
    }

    /// Marqueurs `centrality_cached` de la zone nœuds, dans l'ordre des
    /// slots — l'observable qui dit si une permutation a eu lieu.
    /// PROPRIETE 8 — un temoin HERITE d'une passe precedente ne fait plus
    /// conclure a la barriere franchie.
    ///
    /// ══ LE BLOQUANT DE LA REVUE #4, EN POSITIF ══
    ///
    /// Scenario a cause unique (disque plein, processus long-vivant) :
    /// passe 1 confirmee mais interrompue en phase 9 — un `.remap.tmp`
    /// reste sur le disque ; le warden retente ; passe 2 meurt en phase 6,
    /// donc AVANT la barriere. L'ancien `obrain repair` imprimait
    /// « INDICE DECISIF ... LA BARRIERE A ETE FRANCHIE » sur la foi de ce
    /// temoin herite, et recommandait `--verdict zones-permutees`. Suivre
    /// la consigne confirmait l'epoch 2 : le rejeu tombait dans son cas 1,
    /// `remap_sidecar_files_for_replay` ne refait ni Hilbert ni Community
    /// (seule la phase 6 du writer le fait, et elle n'avait pas eu lieu) —
    /// zone nœuds en passe 2, side-colonne Hilbert restee en passe 1,
    /// desynchronisation DEFINITIVE.
    ///
    /// Construit par le seul code de production, failpoint mis a part.
    #[test]
    fn un_temoin_herite_ne_fait_plus_conclure_a_la_barriere_franchie() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("kb");
        let writer = base_durable(&path);
        let subh = writer.substrate();
        {
            let g = subh.lock();
            let mut w = VecColumnWriter::create(&g, vec_spec()).unwrap();
            for i in 1..N {
                let v = [i as f32; 4];
                w.write_slot(i, bytemuck::cast_slice(&v)).unwrap();
            }
            w.finalize().unwrap();
        }

        // ── PASSE 1 : zones permutees et CONFIRMEES, puis phase 9 qui
        //    echoue sur le temporaire de la colonne vectorielle. La base
        //    reste ouvrable ; un temoin reste sur le disque.
        writer
            .bulk_sort_by_hilbert(N, 0, 8, 4)
            .expect("passe 1 : les zones doivent etre permutees et confirmees");
        {
            let g = subh.lock();
            assert_eq!(
                remap_epoch_state(&g).unwrap(),
                RemapEpochState {
                    committed: 1,
                    pending: 0
                },
                "premisse : passe 1 confirmee"
            );
            let mut colonne = VecColumnWriter::create(&g, vec_spec()).unwrap();
            crate::writer::armer_echec_ensure_room(".tmp", 0);
            let err = colonne
                .remap_node_slots(&g, &perm(), 1)
                .expect_err("le failpoint doit faire echouer la phase 9");
            assert!(format!("{err}").contains("ENOSPC simule"), "{err}");
            assert!(crate::writer::echec_ensure_room_a_tire());

            // ── LE TEMOIN LAISSE PAR LE CODE DE PRODUCTION EST DATE ──
            // Assertion posee AVANT que le test ne touche quoi que ce soit :
            // `open_named_zone` CREE le fichier, donc la verifier apres
            // l'avoir ouverte ne prouverait rien. C'est la vacuite que la
            // mutation « nom non date » a revelee dans la premiere version
            // de ce test.
            let nom_tmp = nom_temporaire_remap(&vec_spec().filename(), 1);
            assert!(
                path.join(&nom_tmp).exists(),
                "le temoin laisse par la phase 9 doit porter SON epoch dans son nom — sans \
                 date, il se fera passer pour une preuve sur la passe suivante"
            );
            assert!(
                !path
                    .join(format!("{}.remap.tmp", vec_spec().filename()))
                    .exists(),
                "aucun temoin NON DATE ne doit plus etre produit"
            );

            // Le temporaire porte des octets quand l'interruption survient
            // plus loin dans sa construction.
            let mut tmp = g.open_named_zone(&nom_tmp).unwrap();
            crate::writer::ensure_room(&mut tmp, 4096, 0).unwrap();
            tmp.msync().unwrap();
        }

        // ── Le processus VIT : il remue des nœuds, le warden retente.
        for i in 1..N {
            writer
                .update_node(i, marked_node(((N - i) * 1000) as u16))
                .unwrap();
        }
        writer.commit().unwrap();
        writer.vider_fenetre_de_rejeu().unwrap();
        {
            let mut g = subh.lock();
            let mut h = g.meta_header();
            h.flags &= !crate::meta::meta_flags::HILBERT_SORTED;
            g.write_meta_header(&h).unwrap();
        }
        let hilbert_avant = {
            let g = subh.lock();
            let zf = g.open_zone(Zone::Hilbert).unwrap();
            zf.as_slice()[..(N as usize) * 4].to_vec()
        };
        let marqueurs_avant = marqueurs(&writer);

        // ── PASSE 2 : meurt en phase 6 — APRES la premiere mutation,
        //    AVANT la barriere. Etat INDETERMINE.
        crate::writer::armer_echec_ensure_room("substrate.hilbert", 0);
        let err = writer
            .bulk_sort_by_hilbert(N, 0, 8, 4)
            .expect_err("passe 2 : le failpoint doit faire echouer la phase 6");
        assert!(format!("{err}").contains("ENOSPC simule"), "{err}");
        assert!(crate::writer::echec_ensure_room_a_tire());

        // ── PREMISSE : la barriere n'a PAS ete franchie ──
        assert_ne!(
            marqueurs_avant,
            marqueurs(&writer),
            "premisse : la phase 4 doit avoir permute la zone nœuds"
        );
        {
            let g = subh.lock();
            let zf = g.open_zone(Zone::Hilbert).unwrap();
            assert_eq!(
                &zf.as_slice()[..(N as usize) * 4],
                &hilbert_avant[..],
                "premisse : la phase 6 ne doit PAS avoir eu lieu"
            );
        }
        drop(writer);

        let sub = crate::file::SubstrateFile::open(&path).unwrap();
        assert_eq!(
            remap_epoch_state(&sub).unwrap(),
            RemapEpochState {
                committed: 1,
                pending: 2
            },
            "l'etat DURABLE dit lui-meme que la barriere n'a pas ete franchie : si elle \
             l'avait ete, `commit_remap_epoch` aurait tourne DANS la region gardee, et aurait \
             ete REESSAYEE — donc committed vaudrait 2"
        );

        // ── L'ASSERTION ──
        let diag = diagnostiquer(&sub);
        let DiagnosticRemap::RepermutationInterrompue {
            epoch,
            committed,
            observations,
        } = diag.clone()
        else {
            panic!("obtenu : {diag:?}")
        };
        assert_eq!((epoch, committed), (2, 1));
        assert!(
            !observations.temoins_anterieurs.is_empty(),
            "le debris de la passe 1 doit rester VISIBLE (il occupe du disque) : {observations:?}"
        );
        assert!(
            observations.temoins_epoch_annoncee.is_empty(),
            "aucun temoin ne peut porter l'epoch ANNONCEE : {observations:?}"
        );
        assert_eq!(
            recommander(&diag),
            Recommandation::AucuneReprisSureSansJournal { epoch: 2 },
            "un temoin HERITE ne prouve rien sur la passe en cours : recommander \
             `zones-permutees` ici confirme l'epoch 2, le rejeu tombe dans son cas 1, et la \
             side-colonne Hilbert reste DEFINITIVEMENT desynchronisee des nœuds"
        );
    }

    /// PROPRIETE 9 — aucun temoin ne peut porter l'epoch ANNONCEE.
    ///
    /// Demonstration, et c'est elle qui justifie qu'aucune branche
    /// « temoin de l'epoch courante => barriere franchie » n'existe : tout
    /// ce qui porte l'epoch `E` est produit par la phase 9, qui s'execute
    /// APRES `commit_remap_epoch(E)` — lequel porte `committed` a `E` et
    /// efface `pending`. Un tel temoin contredirait donc
    /// `pending == E > committed`.
    ///
    /// Le lot precedent avait exactement cette branche dans le CLI. Elle
    /// etait inatteignable par sa condition legitime, et ne se declenchait
    /// que sur des DEBRIS — donc toujours a tort. Une branche morte finit
    /// par mentir : ici elle a menti dans le sens qui corrompt.
    #[test]
    fn aucun_temoin_de_l_epoch_annoncee_ne_peut_exister() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("kb");
        drop(base_durable(&path));
        let sub = crate::file::SubstrateFile::open(&path).unwrap();

        // On FABRIQUE l'etat impossible : annonce de l'epoch 3 ET un
        // temoin date de l'epoch 3. Aucun chemin de production ne le
        // produit — d'ou le refus, et non une conclusion.
        announce_remap_epoch(&sub, 3).unwrap();
        let nom = nom_temporaire_remap("substrate.veccol.node.0001.f32.4", 3);
        std::fs::write(path.join(&nom), b"temoin fabrique").unwrap();

        let diag = diagnostiquer(&sub);
        let DiagnosticRemap::EtatContradictoire {
            epoch, committed, ..
        } = &diag
        else {
            panic!(
                "un temoin de l'epoch annoncee est IMPOSSIBLE : le diagnostic doit refuser, \
                 pas conclure. Obtenu : {diag:?}"
            )
        };
        assert_eq!((*epoch, *committed), (3, 0));
        assert!(matches!(
            recommander(&diag),
            Recommandation::NePasToucher { .. }
        ));
        // Et la reparation refuse, quel que soit le verdict.
        for verdict in [
            VerdictOperateur::ZonesIntactes,
            VerdictOperateur::ZonesPermutees,
        ] {
            appliquer_reparation(&sub, verdict).expect_err(
                "on n'agit pas sur un etat dont on vient d'etablir qu'il est impossible",
            );
        }
    }

    /// PROPRIETE 14 — **aucun chemin ne peut annoncer sans pouvoir
    /// effacer.**
    ///
    /// ══ LE TEST PORTE SUR LA RELATION, PAS SUR UN CHEMIN ══
    ///
    /// Deux chemins qui appellent aujourd'hui le meme gestionnaire ne
    /// garantissent rien sur le troisieme : c'est exactement ce qui s'est
    /// produit entre `writer.rs` (corrige en revue #2) et `replay.rs`
    /// (ajoute en revue #4, sans la discipline). Tester chaque chemin un
    /// par un ne peut pas attraper le chemin qui n'existe pas encore.
    ///
    /// ── DEUX TROUS FERMES EN REVUE #6 ──
    ///
    /// 1. Il cherchait le LITTERAL `announce_remap_epoch(`, donc le site
    ///    d'appel. Un `use crate::slot_remap::announce_remap_epoch as
    ///    poser_annonce_brute;` suivi d'un appel a l'alias compilait et
    ///    passait au VERT — la revue l'a construit. Le balayage cherche
    ///    desormais l'IDENTIFIANT, sous quelque forme qu'il apparaisse :
    ///    appel, `use`, alias, re-export. Le module qui le definit est le
    ///    seul a avoir le droit de le nommer.
    /// 2. `read_dir` n'etait pas recursif. `src/` est plat aujourd'hui,
    ///    mais un futur `src/foo/bar.rs` y aurait echappe.
    ///
    /// Ce test ne remplace pas la privatisation : il la GARDE. Elle ferme
    /// la route evidente, il ferme le detour.
    #[test]
    fn aucun_chemin_ne_peut_annoncer_sans_pouvoir_effacer() {
        fn balayer(racine: &std::path::Path, fautifs: &mut Vec<String>) {
            for entree in std::fs::read_dir(racine).unwrap().flatten() {
                let chemin = entree.path();
                if chemin.is_dir() {
                    balayer(&chemin, fautifs); // (2) recursif
                    continue;
                }
                if chemin.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                // Le module qui DEFINIT la garde a le droit de nommer
                // l'ecriture brute : c'est son role.
                if chemin.file_name().and_then(|n| n.to_str()) == Some("slot_remap.rs") {
                    continue;
                }
                // (1) l'IDENTIFIANT, pas le site d'appel : attrape aussi
                //     `use ... as alias` et les re-exports.
                if std::fs::read_to_string(&chemin)
                    .unwrap()
                    .contains("announce_remap_epoch")
                {
                    fautifs.push(chemin.display().to_string());
                }
            }
        }
        // ── CONTROLE POSITIF, D'ABORD ──
        //
        // Un balayage qui ne trouve rien parce qu'il ne CHERCHE rien rend
        // le meme vert qu'un balayage correct. La revue #7 l'a montre :
        // rendre le litteral introuvable, ou vider `balayer` d'un `return`,
        // laissait la suite verte. Sa panne serait silencieuse — le pire
        // mode pour un test qui garde une invariante.
        //
        // On lui donne donc d'abord une proie qu'il DOIT attraper, dans
        // un sous-repertoire pour exercer aussi la recursion.
        let temoin = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temoin.path().join("sous/dossier")).unwrap();
        std::fs::write(
            temoin.path().join("sous/dossier/appat.rs"),
            "fn detour() { announce_remap_epoch(sub, 1) }",
        )
        .unwrap();
        std::fs::write(temoin.path().join("sous/innocent.rs"), "fn rien() {}").unwrap();
        let mut proies = Vec::new();
        balayer(temoin.path(), &mut proies);
        assert_eq!(
            proies.len(),
            1,
            "CONTROLE POSITIF EN ECHEC : le balayage ne detecte plus l'identifiant, ou ne \
             descend plus dans les sous-repertoires. Il rendrait desormais un vert qui ne \
             signifie rien. Proies : {proies:?}"
        );
        assert!(proies[0].ends_with("appat.rs"), "proies : {proies:?}");

        // ── PUIS LA VRAIE ASSERTION ──
        let racine = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut fautifs = Vec::new();
        balayer(&racine, &mut fautifs);
        assert!(
            fautifs.is_empty(),
            "ces fichiers NOMMENT l'ecriture brute de l'annonce — appel direct, `use`, alias ou \
             re-export — donc peuvent annoncer sans garantie de pouvoir effacer sur leurs \
             chemins d'erreur : {fautifs:?}. C'est le defaut qui a coute deux revues : \
             `writer.rs` corrige, puis `replay.rs` le reproduisant un etage plus bas. Passez \
             par `AnnonceRemap::annoncer`."
        );
    }

    /// PROPRIETE 16 — le CONSTRUCTEUR de la garde s'applique sa propre
    /// discipline.
    ///
    /// ══ LE SEUL CHEMIN QUE LA GARDE NE POUVAIT PAS COUVRIR ══
    ///
    /// `write_epoch_state` ECRIT la page avant de demander sa durabilite.
    /// Une erreur d'E/S a cette couture fait echouer l'annonce APRES que
    /// `pending` soit dans le fichier — et aucune garde n'est rendue, donc
    /// ni `Drop`, ni `#[must_use]`, ni la privatisation n'ont prise. Le
    /// constructeur etait exactement le « quatrieme chemin » que la garde
    /// pretendait rendre impossible.
    #[test]
    fn un_echec_du_constructeur_n_abandonne_pas_une_annonce_posee() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("kb");
        drop(base_durable(&path));
        let sub = crate::file::SubstrateFile::open(&path).unwrap();

        // ── PREMISSE : la base s'ouvre, rien n'est annonce ──
        assert_eq!(epoque_indeterminee(&sub).unwrap(), None);

        // ── LE FAILPOINT DOIT VISER LA COUTURE, PAS AVANT ──
        //
        // `write_epoch_state` ECRIT la page puis demande sa durabilite.
        // C'est un echec ENTRE LES DEUX qui laisse `pending` sur le disque
        // pendant que l'annonce echoue. Une premiere version de ce test
        // armait `ensure_room`, donc AVANT l'ecriture de la page : la
        // mutation « constructeur avec un `?` nu » restait VERTE, parce
        // que rien n'avait ete ecrit. C'est la mutation qui l'a revele.
        crate::writer::armer_echec_msync(REMAP_EPOCH_ZONE);
        let err = AnnonceRemap::annoncer(&sub, 5)
            .err()
            .expect("le failpoint doit faire echouer l'annonce");
        assert!(format!("{err}").contains("EIO simule"), "{err}");
        assert!(
            crate::writer::echec_msync_a_tire(),
            "le failpoint n'a jamais tire : la couture visee n'a pas ete atteinte"
        );

        // ── L'ASSERTION : aucune annonce ne survit a un constructeur qui
        //    a echoue, quel que soit l'endroit ou il a echoue ──
        assert_eq!(
            epoque_indeterminee(&sub).unwrap(),
            None,
            "un constructeur en echec ne doit JAMAIS laisser `pending` pose : aucune garde \
             n'est rendue, donc ni Drop ni must_use ni la privatisation n'ont prise. C'est le \
             seul chemin que la garde ne peut pas couvrir — il doit se couvrir lui-meme."
        );
        drop(sub);
        crate::store::SubstrateStore::open(&path)
            .expect("la base doit rester ouvrable apres un constructeur en echec");
    }

    /// PROPRIETE 16bis — **le jumeau PRODUCTION du precedent.**
    ///
    /// ══ J'AVAIS TESTE LE CHEMIN QUI NE TOURNE PAS EN PRODUCTION ══
    ///
    /// `annoncer` n'a qu'un appelant : le chemin de REJEU. Les deux sites
    /// de production — `bulk_sort_by_hilbert` et `compact_community` —
    /// passent par `annoncer_partagee`, dont le correctif n'etait garde
    /// par RIEN : le ramener a un `?` nu laissait la suite entierement
    /// verte.
    ///
    /// La moitie production du correctif tenait donc par la vigilance
    /// seule — exactement ce que ce chantier a passe trois revues a
    /// demonter, et sur le seul chemin qui compte.
    ///
    /// Lecon de methode : quand une fonction a deux variantes, tester
    /// celle qui se teste facilement n'apprend rien sur celle qui tourne.
    #[test]
    fn un_echec_du_constructeur_partage_n_abandonne_pas_une_annonce_posee() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("kb");
        let writer = base_durable(&path);
        let substrat = writer.substrate();

        // ── PREMISSE : rien n'est annonce ──
        {
            let g = substrat.lock();
            assert_eq!(epoque_indeterminee(&g).unwrap(), None);
        }

        // Le failpoint frappe la COUTURE : page ecrite, durabilite pas
        // encore acquise (cf. PROPRIETE 16).
        crate::writer::armer_echec_msync(REMAP_EPOCH_ZONE);
        let err = AnnonceRemap::annoncer_partagee(substrat.clone(), 5)
            .err()
            .expect("le failpoint doit faire echouer l'annonce partagee");
        assert!(format!("{err}").contains("EIO simule"), "{err}");
        assert!(
            crate::writer::echec_msync_a_tire(),
            "le failpoint n'a jamais tire : la couture visee n'a pas ete atteinte"
        );

        // ── L'ASSERTION, sur le chemin qui tourne reellement ──
        {
            let g = substrat.lock();
            assert_eq!(
                epoque_indeterminee(&g).unwrap(),
                None,
                "`annoncer_partagee` est le constructeur des DEUX chemins de production : un \
                 echec ne doit pas plus y laisser `pending` pose que dans `annoncer`"
            );
        }
        drop(writer);
        crate::store::SubstrateStore::open(&path)
            .expect("la base doit rester ouvrable apres un constructeur partage en echec");
    }

    /// PROPRIETE 17 — resoudre une annonce en tenant deja le verrou NE
    /// FIGE PAS le fil.
    ///
    /// `parking_lot::Mutex` n'est pas reentrant, et la resolution REPREND
    /// le verrou. La revue #6 a execute le gel : un chemin qui tient
    /// encore `substrate.lock()` quand la garde meurt figeait le fil,
    /// definitivement et en silence.
    ///
    /// Les deux sites actuels sont saufs — mais « saufs aujourd'hui » est
    /// exactement ce qui a coute deux revues a ce module. La resolution
    /// attend desormais BORNE puis renonce en journalisant : on echange un
    /// gel silencieux contre une annonce laissee et un message explicite.
    /// Dans un destructeur, c'est toujours le bon echange.
    #[test]
    fn resoudre_en_tenant_le_verrou_renonce_au_lieu_de_figer() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("kb");
        let writer = base_durable(&path);
        let substrat = writer.substrate();

        let journal = JournalTest::default();
        let debut = web_time::Instant::now();
        tracing::subscriber::with_default(journal.clone(), || {
            let annonce = AnnonceRemap::annoncer_partagee(substrat.clone(), 1).unwrap();
            // Le chemin fautif : il tient le verrou pendant que la garde
            // meurt. Sans borne, ce `drop` ne rendrait JAMAIS la main.
            let _garde_verrou = substrat.lock();
            drop(annonce);
        });
        let ecoule = debut.elapsed();

        // ── BORNE ABSOLUE, PAS RELATIVE A SA CIBLE (revue #7) ──
        //
        // L'assertion etait `ecoule < DELAI_REPRISE_VERROU * 3` : ecrite
        // dans l'unite de la constante qu'elle etait censee borner, donc
        // SUIVANT sa cible et incapable de la refuter. La revue a fait
        // varier le delai de 50 ms a 30 s sans jamais faire rougir le
        // test. Une borne en secondes de mur, elle, refute.
        const PLAFOND_ABSOLU: std::time::Duration = std::time::Duration::from_secs(10);
        assert!(
            ecoule < PLAFOND_ABSOLU,
            "la resolution a mis {ecoule:?} : un destructeur doit RENONCER, jamais figer le \
             fil. (Plafond absolu {PLAFOND_ABSOLU:?}, independant de DELAI_REPRISE_VERROU.)"
        );
        assert!(
            ecoule >= DELAI_REPRISE_VERROU,
            "la resolution a rendu la main en {ecoule:?}, avant meme d'avoir tente d'attendre \
             : le test n'a pas exerce la contention qu'il pretend exercer"
        );

        // ── ET LE RENONCEMENT EST JOURNALISE ──
        //
        // Toute la doctrine est « on echange un gel silencieux contre une
        // annonce laissee ET UN MESSAGE EXPLICITE ». Supprimer le
        // `tracing::error!` laissait la suite verte : la doctrine n'etait
        // gardee par rien, et le renoncement pouvait redevenir silencieux.
        assert!(
            journal.contient(tracing::Level::ERROR, "RESOLUTION D'ANNONCE ABANDONNEE"),
            "le renoncement doit etre journalise en ERROR — c'est la moitie de la doctrine. \
             Journal capture : {:?}",
            journal.tout()
        );
        // Et elle renonce honnetement : l'annonce reste posee, la base
        // refusera de se rouvrir — c'est visible, pas silencieux.
        let sub = crate::file::SubstrateFile::open(&path).unwrap();
        assert_eq!(
            epoque_indeterminee(&sub).unwrap(),
            Some(1),
            "renoncer laisse l'annonce : c'est le prix, et il est journalise en ERROR"
        );
    }

    /// PROPRIETE 15 — une annonce liberee SANS resolution explicite est
    /// tranchee par la garde elle-meme.
    ///
    /// C'est le filet : si un futur chemin oublie d'appeler `confirmer` ou
    /// `abandonner` — ou si un `?` saute par-dessus — `Drop` applique le
    /// meme ternaire. L'oubli devient inoffensif au lieu d'etre fatal.
    #[test]
    fn une_annonce_liberee_sans_resolution_est_tranchee_par_la_garde() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("kb");
        drop(base_durable(&path));
        let sub = crate::file::SubstrateFile::open(&path).unwrap();

        // (a) AUCUNE MUTATION : la garde doit EFFACER, donc la base reste
        //     ouvrable — c'est le cas de l'ENOSPC avant la premiere
        //     ecriture d'octet.
        {
            let annonce = AnnonceRemap::annoncer(&sub, 1).unwrap();
            assert_eq!(
                epoque_indeterminee(&sub).unwrap(),
                Some(1),
                "premisse : l'annonce est bien posee tant que la garde vit"
            );
            drop(annonce); // personne n'a tranche
        }
        assert_eq!(
            epoque_indeterminee(&sub).unwrap(),
            None,
            "une annonce liberee sans resolution, AUCUNE zone touchee, doit etre effacee : \
             sinon un chemin distrait laisse une base intacte non ouvrable"
        );

        // (b) APRES LA PREMIERE MUTATION : la garde doit LAISSER
        //     l'annonce — l'etat est indetermine, et c'est le seul cas ou
        //     le refus est le bon comportement.
        {
            let annonce = AnnonceRemap::annoncer(&sub, 1).unwrap();
            annonce.premiere_mutation();
            drop(annonce);
        }
        assert_eq!(
            epoque_indeterminee(&sub).unwrap(),
            Some(1),
            "apres une mutation, l'etat est INDETERMINE : la garde ne doit surtout pas effacer"
        );

        // (c) BARRIERE FRANCHIE : la garde doit CONFIRMER.
        clear_pending_remap_epoch(&sub, 1).unwrap();
        {
            let annonce = AnnonceRemap::annoncer(&sub, 2).unwrap();
            annonce.premiere_mutation();
            annonce.barriere_franchie();
            drop(annonce);
        }
        assert_eq!(
            epoque_indeterminee(&sub).unwrap(),
            None,
            "barriere franchie : les zones sont permutees et durables, la garde CONFIRME"
        );
        assert_eq!(
            current_remap_epoch(&sub).unwrap(),
            2,
            "la confirmation implicite doit avoir porte `committed` a l'epoch annoncee"
        );
    }

    /// PROPRIETE 9b — un SIDE-CAR estampille a l'epoch annoncee est lui
    /// aussi contradictoire.
    ///
    /// `EtatContradictoire` a DEUX sources : les temoins `.tmp` et les
    /// en-tetes de side-cars. Seule la premiere etait couverte — supprimer
    /// entierement la branche side-car laissait la suite verte. Une moitie
    /// de refus bruyant qui n'est exercee par rien finit par etre
    /// supprimee par quelqu'un qui la croit morte.
    #[test]
    fn un_sidecar_estampille_a_l_epoch_annoncee_est_contradictoire() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("kb");
        drop(base_durable(&path));
        let sub = crate::file::SubstrateFile::open(&path).unwrap();
        {
            let mut w = VecColumnWriter::create(&sub, vec_spec()).unwrap();
            w.write_slot(1, bytemuck::cast_slice(&[1.0f32; 4])).unwrap();
            w.finalize().unwrap();
        }
        announce_remap_epoch(&sub, 1).unwrap();

        // ── PREMISSE : sans estampille contradictoire, le diagnostic
        //    conclut a une repermutation interrompue ordinaire ──
        assert!(
            matches!(
                diagnostiquer(&sub),
                DiagnosticRemap::RepermutationInterrompue { epoch: 1, .. }
            ),
            "premisse : l'etat doit d'abord etre un simple `interrompue`"
        );

        // On FABRIQUE l'impossible : l'en-tete du side-car porte l'epoch
        // ANNONCEE, alors que `committed` vaut 0.
        {
            let mut zf = sub.open_named_zone(&vec_spec().filename()).unwrap();
            // `_reserved[0]` suit magic(4) version(4) prop_key_id(2)
            // entity_kind(1) dtype(1) dim(4) n_slots(4) flags(4) crc32(4).
            let octets = zf.as_slice_mut();
            let avant = octets.to_vec();
            let mut pose = false;
            for decalage in (0..64 - 4).step_by(4) {
                octets.copy_from_slice(&avant);
                octets[decalage..decalage + 4].copy_from_slice(&1u32.to_le_bytes());
                if let Some(h) = crate::vec_column::VecColumnHeader::try_read(octets)
                    && h._reserved[0] == 1
                {
                    pose = true;
                    break;
                }
            }
            assert!(pose, "impossible de poser l'epoch dans `_reserved[0]`");
            zf.msync().unwrap();
        }

        let diag = diagnostiquer(&sub);
        let DiagnosticRemap::EtatContradictoire {
            epoch, committed, ..
        } = &diag
        else {
            panic!(
                "un side-car estampille a l'epoch ANNONCEE est impossible (l'estampille ne \
                 depasse jamais `committed`, et `pending` vaut `committed + 1`) : le \
                 diagnostic doit REFUSER. Obtenu : {diag:?}"
            )
        };
        assert_eq!((*epoch, *committed), (1, 0));
        assert!(matches!(
            recommander(&diag),
            Recommandation::NePasToucher { .. }
        ));
        for verdict in [
            VerdictOperateur::ZonesIntactes,
            VerdictOperateur::ZonesPermutees,
        ] {
            appliquer_reparation(&sub, verdict)
                .expect_err("la reparation doit refuser sur un etat contradictoire");
        }
    }

    /// PROPRIETE 10 — la recommandation est UNE regle, dans le module
    /// partage, et elle couvre les quatre diagnostics.
    #[test]
    fn la_recommandation_couvre_les_quatre_diagnostics() {
        assert_eq!(
            recommander(&DiagnosticRemap::Saine { committed: 4 }),
            Recommandation::RienAFaire
        );
        assert!(matches!(
            recommander(&DiagnosticRemap::OracleIllisible {
                raison: "crc".into()
            }),
            Recommandation::NePasToucher { .. }
        ));
        assert!(matches!(
            recommander(&DiagnosticRemap::EtatContradictoire {
                epoch: 2,
                committed: 1,
                details: "x".into()
            }),
            Recommandation::NePasToucher { .. }
        ));
        // Le cœur : une repermutation interrompue ne permet AUCUNE reprise
        // sure depuis l'etat durable, quels que soient les debris observes.
        for observations in [
            Observations::default(),
            Observations {
                record_dans_le_wal: true,
                sidecars: vec![("substrate.veccol.node.0001.f32.4".into(), 1)],
                temoins_anterieurs: vec![("c.remap.1.tmp".into(), Some(1), 4096)],
                temoins_epoch_annoncee: Vec::new(),
            },
        ] {
            assert_eq!(
                recommander(&DiagnosticRemap::RepermutationInterrompue {
                    epoch: 2,
                    committed: 1,
                    observations,
                }),
                Recommandation::AucuneReprisSureSansJournal { epoch: 2 },
                "aucun debris ne doit jamais autoriser un verdict : la seule chose qui \
                 prouverait la barriere franchie est la confirmation, et c'est elle qui manque"
            );
        }
    }

    /// PROPRIETE 11 — les temoins sont DATES, et un remap efface ceux des
    /// passes precedentes de la meme colonne.
    #[test]
    fn les_temoins_sont_dates_et_les_anciens_sont_effaces() {
        assert_eq!(
            nom_temporaire_remap("substrate.veccol.node.0001.f32.4", 7),
            "substrate.veccol.node.0001.f32.4.remap.7.tmp"
        );
        assert_eq!(epoch_du_temporaire("c.remap.7.tmp"), Some(Some(7)));
        // Format HERITE : reconnu comme temoin, mais sans date — donc
        // jamais utilisable comme preuve.
        assert_eq!(epoch_du_temporaire("c.remap.tmp"), Some(None));
        assert_eq!(epoch_du_temporaire("substrate.nodes"), None);

        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("kb");
        let writer = base_durable(&path);
        let subh = writer.substrate();
        let g = subh.lock();
        let mut w = VecColumnWriter::create(&g, vec_spec()).unwrap();
        for i in 1..N {
            let v = [i as f32; 4];
            w.write_slot(i, bytemuck::cast_slice(&v)).unwrap();
        }
        w.finalize().unwrap();

        // Debris d'une passe 1 (date) et d'un binaire anterieur (non date).
        let colonne = vec_spec().filename();
        std::fs::write(path.join(nom_temporaire_remap(&colonne, 1)), b"vieux").unwrap();
        std::fs::write(path.join(format!("{colonne}.remap.tmp")), b"tres vieux").unwrap();

        let mut w = VecColumnWriter::create(&g, vec_spec()).unwrap();
        w.remap_node_slots(&g, &perm(), 5).unwrap();

        assert!(
            !path.join(nom_temporaire_remap(&colonne, 1)).exists(),
            "le debris DATE de la passe precedente doit avoir ete efface"
        );
        assert!(
            !path.join(format!("{colonne}.remap.tmp")).exists(),
            "le debris au format HERITE doit avoir ete efface lui aussi"
        );
    }

    /// PROPRIETE 13 — une zone d'epoch TRONQUEE mais NON NULLE est une
    /// ecriture interrompue, pas une base vierge.
    ///
    /// La branche « trop courte » rendait `default()` SANS regarder le
    /// contenu, alors que la doc affirmait que seule une zone nulle peut
    /// etre consideree vierge. Non atteignable par le code actuel — mais
    /// c'etait une garantie par CONVENTION, et ce module a passe quatre
    /// revues a apprendre ce que coutent les garanties par convention.
    #[test]
    fn une_zone_d_epoch_tronquee_mais_non_nulle_est_un_refus() {
        let td = tempfile::tempdir().unwrap();
        let sub = crate::file::SubstrateFile::create(td.path()).unwrap();
        commit_remap_epoch(&sub, 9).unwrap();
        assert_eq!(
            current_remap_epoch(&sub).unwrap(),
            9,
            "premisse : la zone est lisible avant troncature"
        );

        // Troncature a 10 octets : le magic « REMP » survit, donc le
        // contenu n'est PAS nul.
        let chemin = td.path().join(REMAP_EPOCH_ZONE);
        std::fs::File::options()
            .write(true)
            .open(&chemin)
            .unwrap()
            .set_len(10)
            .unwrap();

        let err = current_remap_epoch(&sub)
            .expect_err("une zone tronquee et NON NULLE est une ecriture interrompue");
        let msg = format!("{err}");
        assert!(
            matches!(err, SubstrateError::EpoqueRemapIllisible { .. }) && msg.contains("tronquee"),
            "obtenu : {err:?}"
        );

        // Contre-epreuve : tronquee ET NULLE reste legitime — c'est le cas
        // reel d'une zone materialisee par une simple lecture.
        std::fs::write(&chemin, [0u8; 8]).unwrap();
        assert_eq!(
            current_remap_epoch(&sub).unwrap(),
            0,
            "une zone courte et NULLE reste une base jamais repermutee"
        );
    }

    /// Prepare une base dont le WAL porte un record de repermutation non
    /// encore applique, et rend l'offset de rejeu.
    fn base_avec_record_de_repermutation(path: &std::path::Path, avec_engram: bool) -> u64 {
        let writer = base_durable(path);
        let baseline = writer.wal().offset();
        writer
            .append_payload(WalPayload::HilbertRepermuteV2 {
                permutation: perm(),
                epoch: 1,
                engram_members: if avec_engram {
                    vec![(1u16, vec![1u32, 2, 3])]
                } else {
                    Vec::new()
                },
                engram_bitsets: Vec::new(),
            })
            .unwrap();
        writer.fsync_wal().unwrap();
        baseline
    }

    /// PROPRIETE 12a — un echec du rejeu AVANT toute mutation EFFACE
    /// l'annonce : la base reste ouvrable.
    ///
    /// ══ LA REGRESSION DE LA REVUE #5, EN POSITIF ══
    ///
    /// La revue #4 avait fait poser une annonce sur le chemin de rejeu,
    /// sans y cabler l'effacement qui va avec. Or `ensure_room` s'y
    /// execute AVANT la premiere ecriture d'octet : un ENOSPC y laissait
    /// les zones PROUVEES intactes et `pending` durable.
    ///
    /// Et le chemin de rejeu tourne A L'OUVERTURE. Un disque passagerement
    /// plein au demarrage briquait donc definitivement une base qu'il
    /// suffisait, avant, de reessayer — la pire aggravation possible pour
    /// ce defaut. C'etait mot pour mot ce que ce module avait fait ecrire
    /// en revue #2 : « corriger le refus sans cabler cet effacement, c'est
    /// transformer une correction en panne ».
    #[test]
    fn un_echec_du_rejeu_avant_mutation_efface_l_annonce_et_la_base_reste_ouvrable() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("kb");
        let baseline = base_avec_record_de_repermutation(&path, false);

        let sub = crate::file::SubstrateFile::open(&path).unwrap();
        let avant = marqueurs_de(&sub);

        // `ensure_room` sur la zone nœuds : le tout premier point
        // faillible du rejeu, AVANT la premiere ecriture d'octet.
        crate::writer::armer_echec_ensure_room("substrate.nodes", 0);
        let err = crate::replay::replay_from(&sub, baseline)
            .expect_err("le failpoint doit interrompre le rejeu");
        assert!(format!("{err}").contains("ENOSPC simule"), "{err}");
        assert!(
            crate::writer::echec_ensure_room_a_tire(),
            "le failpoint n'a jamais tire : la fenetre visee n'a pas ete atteinte"
        );

        // ── PREMISSE : les zones sont PROUVEES intactes ──
        assert_eq!(
            marqueurs_de(&sub),
            avant,
            "premisse : l'echec est survenu AVANT toute ecriture d'octet"
        );
        // ── L'ASSERTION : l'annonce a ete effacee ──
        assert_eq!(
            epoque_indeterminee(&sub).unwrap(),
            None,
            "un ENOSPC du rejeu avant toute mutation ne doit PAS laisser d'annonce : le rejeu \
             tourne a l'ouverture, donc un disque passagerement plein briquerait la base"
        );
        drop(sub);
        crate::store::SubstrateStore::open(&path)
            .expect("la base doit rester parfaitement ouvrable — ses donnees sont intactes");
    }

    /// PROPRIETE 12b — un echec du rejeu APRES la premiere mutation LAISSE
    /// l'annonce, et le rejeu suivant REFUSE au lieu de re-appliquer.
    ///
    /// C'est l'autre moitie du ternaire : le cas 3 permutait les zones puis
    /// confirmait, sans annonce. Un second arret entre les deux laissait
    /// `committed` inchange, le rejeu suivant retombait dans le meme cas 3
    /// et RE-APPLIQUAIT — nœuds a p², side-cars a p¹, CRC valides, aucun
    /// symptome.
    #[test]
    fn un_echec_du_rejeu_apres_mutation_laisse_l_annonce_et_le_suivant_refuse() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("kb");
        let baseline = base_avec_record_de_repermutation(&path, true);

        let sub = crate::file::SubstrateFile::open(&path).unwrap();
        let avant = marqueurs_de(&sub);

        // La zone engram est agrandie APRES l'ecriture des nœuds et AVANT
        // le msync de la barriere.
        crate::writer::armer_echec_ensure_room("substrate.engram_members", 0);
        let err = crate::replay::replay_from(&sub, baseline)
            .expect_err("le failpoint doit interrompre le rejeu");
        assert!(format!("{err}").contains("ENOSPC simule"), "{err}");
        assert!(crate::writer::echec_ensure_room_a_tire());

        // ── PREMISSE : les zones ONT ete mutees ──
        assert_ne!(
            marqueurs_de(&sub),
            avant,
            "premisse : la permutation des nœuds doit avoir eu lieu, sinon ce test couvre le \
             cas 12a et non celui-ci"
        );
        // ── L'ASSERTION : l'annonce RESTE ──
        assert_eq!(
            epoque_indeterminee(&sub).unwrap(),
            Some(1),
            "un echec APRES la premiere mutation laisse un etat INDETERMINE : l'annonce doit \
             rester posee"
        );
        let apres = marqueurs_de(&sub);
        drop(sub);

        let sub = crate::file::SubstrateFile::open(&path).unwrap();
        let err = crate::replay::replay_from(&sub, baseline).expect_err(
            "sans annonce, ce second rejeu retomberait dans le cas 3 et RE-APPLIQUERAIT la \
             permutation sur des zones deja permutees",
        );
        assert!(
            matches!(
                err,
                SubstrateError::RepermutationIndeterminee { epoch: 1, .. }
            ),
            "obtenu : {err:?}"
        );
        assert_eq!(
            marqueurs_de(&sub),
            apres,
            "le second rejeu ne doit avoir touche AUCUN octet de la zone nœuds"
        );
    }

    fn marqueurs(writer: &Writer) -> Vec<u16> {
        let sub = writer.substrate();
        let sub = sub.lock();
        marqueurs_de(&sub)
    }

    fn marqueurs_de(sub: &crate::file::SubstrateFile) -> Vec<u16> {
        let zf = sub.open_zone(Zone::Nodes).unwrap();
        let buf = zf.as_slice();
        (0..N as usize)
            .map(|i| {
                let rec: &NodeRecord =
                    bytemuck::from_bytes(&buf[i * NodeRecord::SIZE..(i + 1) * NodeRecord::SIZE]);
                rec.centrality_cached
            })
            .collect()
    }

    /// PROPRIETE 4a — une zone d'epoch PRESENTE mais illisible fait
    /// REFUSER `SubstrateStore::open`, au lieu de repermuter.
    #[test]
    fn une_zone_d_epoch_illisible_fait_refuser_l_ouverture() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("kb");
        drop(base_durable(&path));
        {
            let sub = crate::file::SubstrateFile::open(&path).unwrap();
            commit_remap_epoch(&sub, 1).unwrap();
        }

        // ── PREMISSE : sur zone SAINE, la base s'ouvre ──
        crate::store::SubstrateStore::open(&path)
            .expect("premisse : zone d'epoch saine => ouverture normale");

        // Un seul octet du CRC.
        {
            let sub = crate::file::SubstrateFile::open(&path).unwrap();
            let mut zf = sub.open_named_zone(REMAP_EPOCH_ZONE).unwrap();
            zf.as_slice_mut()[16] ^= 0x01;
            zf.msync().unwrap();
            zf.fsync().unwrap();
        }

        let Err(err) = crate::store::SubstrateStore::open(&path) else {
            panic!("un octet corrompu dans l'ORACLE ne doit pas ouvrir la base")
        };
        assert!(
            matches!(err, SubstrateError::EpoqueRemapIllisible { .. }),
            "obtenu : {err:?}"
        );
        assert!(
            format!("{err}").contains("re-permuter"),
            "le refus doit nommer la consequence evitee : {err}"
        );
    }

    /// PROPRIETE 4b — une VERSION INCONNUE n'est pas une base vierge.
    ///
    /// Ce n'est pas une corruption de bit improbable : `REMAP_EPOCH_VERSION`
    /// vaut deja 2, donc une version 1 a existe et une version 3 existera.
    /// Sans ce refus, toute montee de version relirait TOUTE base comme
    /// neuve et la repermuterait. Evenement PLANIFIE, pas accident.
    #[test]
    fn une_version_d_epoch_inconnue_fait_refuser_l_ouverture() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("kb");
        drop(base_durable(&path));
        {
            let sub = crate::file::SubstrateFile::open(&path).unwrap();
            commit_remap_epoch(&sub, 1).unwrap();
        }
        crate::store::SubstrateStore::open(&path)
            .expect("premisse : version courante => ouverture normale");

        // Version 3 (futur binaire) AVEC un CRC recalcule : la zone est
        // parfaitement integre, seulement ecrite par un autre binaire.
        // C'est ce qui distingue ce test du precedent.
        {
            let sub = crate::file::SubstrateFile::open(&path).unwrap();
            let mut zf = sub.open_named_zone(REMAP_EPOCH_ZONE).unwrap();
            let octets = zf.as_slice_mut();
            octets[4..8].copy_from_slice(&(REMAP_EPOCH_VERSION + 1).to_le_bytes());
            let crc = {
                let mut h = crc32fast::Hasher::new();
                h.update(&octets[0..16]);
                h.finalize()
            };
            octets[16..20].copy_from_slice(&crc.to_le_bytes());
            zf.msync().unwrap();
            zf.fsync().unwrap();
        }

        let Err(err) = crate::store::SubstrateStore::open(&path) else {
            panic!("une version d'epoch inconnue ne doit pas se lire comme une base vierge")
        };
        let msg = format!("{err}");
        assert!(
            matches!(err, SubstrateError::EpoqueRemapIllisible { .. }) && msg.contains("version"),
            "obtenu : {err:?}"
        );
    }
    /// CONDITION DE RE-REVUE — crash injecté ENTRE la barrière de
    /// durabilité des zones et l'estampillage des side-cars.
    ///
    /// C'est la fenêtre que la revue a désignée comme la plus large : sur
    /// une colonne de plusieurs Go, l'estampillage dure des secondes. Le
    /// rejeu doit rattraper les side-cars SANS re-permuter les zones —
    /// exactement ce que le découplage « epoch confirmé » / « epochs
    /// d'en-tête » rend possible.
    ///
    /// (Ma `build_crashed_mid_remap` injecte le crash AVANT la barrière,
    /// une fenêtre plus facile : celle-ci est la difficile.)
    #[test]
    fn crash_entre_la_barriere_et_l_estampillage_des_sidecars() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("kb");
        let p = perm();

        let baseline = {
            let sub = crate::file::SubstrateFile::create(&path).unwrap();
            let writer = Writer::new(sub, SyncMode::EveryCommit).unwrap();
            for i in 1..N {
                writer.update_node(i, marked_node(i as u16)).unwrap();
            }
            let subh = writer.substrate();
            {
                let g = subh.lock();
                let mut w = VecColumnWriter::create(&g, vec_spec()).unwrap();
                for i in 1..N {
                    let v = [i as f32; 4];
                    w.write_slot(i, bytemuck::cast_slice(&v)).unwrap();
                }
                w.finalize().unwrap();
                let mut col = DegreeColumn::create(&g, N).unwrap();
                for i in 1..N {
                    col.incr_out(i, i as i32);
                }
                col.persist_header_crc();
                col.msync().unwrap();
            }
            writer.commit().unwrap();
            writer.msync_zones().unwrap();
            let baseline = writer.wal().offset();
            writer.note_durable_baseline(baseline);

            // Le record, puis l'annonce.
            writer
                .append_payload(WalPayload::HilbertRepermuteV2 {
                    permutation: p.clone(),
                    epoch: 1,
                    engram_members: Vec::new(),
                    engram_bitsets: Vec::new(),
                })
                .unwrap();
            writer.fsync_wal().unwrap();
            {
                let g = subh.lock();
                announce_remap_epoch(&g, 1).unwrap();
            }

            // Zones permutées PUIS rendues durables PUIS epoch CONFIRMÉ.
            {
                let g = subh.lock();
                let mut nodes = None;
                let mut edges = None;
                crate::replay::apply_hilbert_permutation(&g, &mut nodes, &mut edges, &p, None)
                    .unwrap();
                for z in [nodes.as_ref(), edges.as_ref()].into_iter().flatten() {
                    z.msync().unwrap();
                    z.fsync().unwrap();
                }
                commit_remap_epoch(&g, 1).unwrap();
            }
            // ── CRASH ICI : aucun side-car n'a été estampillé ──
            drop(writer);
            baseline
        };

        let sub = crate::file::SubstrateFile::open(&path).unwrap();
        // Prémisse du test : les zones SONT permutées, les side-cars NON.
        assert_eq!(remap_epoch_state(&sub).unwrap().committed, 1);
        let r = VecColumnReader::open(&sub, vec_spec()).unwrap().unwrap();
        assert_eq!(
            r.read_slot_f32(1).unwrap(),
            &[1.0f32; 4][..],
            "premisse : la colonne est encore en numerotation PRE-permutation"
        );
        drop(r);

        // Le rejeu rattrape les side-cars, sans re-permuter les zones.
        crate::replay::replay_from(&sub, baseline).unwrap();

        for i in 1..N {
            let off = (p[i as usize] as usize) * NodeRecord::SIZE;
            let zf = sub.open_zone(Zone::Nodes).unwrap();
            let rec: &NodeRecord =
                bytemuck::from_bytes(&zf.as_slice()[off..off + NodeRecord::SIZE]);
            assert_eq!(
                rec.centrality_cached, i as u16,
                "les zones ont ete re-permutees alors qu'elles etaient deja a jour"
            );
        }
        let r = VecColumnReader::open(&sub, vec_spec()).unwrap().unwrap();
        for i in 1..N {
            assert_eq!(
                r.read_slot_f32(p[i as usize]).unwrap(),
                &[i as f32; 4][..],
                "side-car veccol non rattrape apres un crash post-barriere"
            );
        }
        let col = DegreeColumn::open(&sub).unwrap().unwrap();
        for i in 1..N {
            assert_eq!(col.out_degree(p[i as usize]), i, "degres non rattrapes");
        }
    }

    /// Une annonce SANS confirmation : le processus est mort entre les
    /// mutations et la barrière. L'état des zones est INDÉTERMINÉ — rien
    /// ne distingue « intactes », « à moitié permutées » et « permutées ».
    /// Le rejeu doit REFUSER bruyamment plutôt que deviner : deviner,
    /// c'est corrompre une fois sur deux, en silence.
    #[test]
    fn une_annonce_sans_confirmation_fait_refuser_le_rejeu() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("kb");
        let baseline = {
            let sub = crate::file::SubstrateFile::create(&path).unwrap();
            let writer = Writer::new(sub, SyncMode::EveryCommit).unwrap();
            for i in 1..N {
                writer.update_node(i, marked_node(i as u16)).unwrap();
            }
            writer.commit().unwrap();
            writer.msync_zones().unwrap();
            let baseline = writer.wal().offset();
            writer
                .append_payload(WalPayload::HilbertRepermuteV2 {
                    permutation: perm(),
                    epoch: 1,
                    engram_members: Vec::new(),
                    engram_bitsets: Vec::new(),
                })
                .unwrap();
            writer.fsync_wal().unwrap();
            {
                let g = writer.substrate();
                let g = g.lock();
                announce_remap_epoch(&g, 1).unwrap();
            }
            baseline
        };

        let sub = crate::file::SubstrateFile::open(&path).unwrap();
        let err = crate::replay::replay_from(&sub, baseline)
            .expect_err("un etat indetermine doit faire REFUSER le rejeu");
        assert!(
            matches!(
                err,
                crate::error::SubstrateError::RepermutationIndeterminee { epoch: 1, .. }
            ),
            "le refus doit porter la VARIANTE dediee (sinon l'appelant ne peut pas le \
             distinguer d'un WAL tronque et l'avale), obtenu : {err:?}"
        );
        let msg = format!("{err}");
        assert!(
            msg.contains("INDETERMINE"),
            "le refus doit nommer la cause, obtenu : {msg}"
        );
    }

    // =================================================================
    // Revue adversariale #2 (2026-08-24) — LE REFUS DOIT REMONTER
    // JUSQU'A `SubstrateStore::open`.
    //
    // Les quatre tests ci-dessous ferment le bloquant : le refus
    // `INDETERMINE` existait deja, mais etait avale un etage plus haut
    // par un `tracing::warn!`, et la base s'ouvrait quand meme sur des
    // zones dont le code venait d'etablir que l'etat est indetermine.
    // =================================================================

    /// Prepare une base de `N` noeuds marques, commitee et durable, avec
    /// une baseline de rejeu publiee (sans quoi
    /// `require_empty_replay_window` refuserait tout tri).
    fn base_durable(path: &std::path::Path) -> Writer {
        let sub = crate::file::SubstrateFile::create(path).unwrap();
        let writer = Writer::new(sub, SyncMode::EveryCommit).unwrap();
        // Marqueurs DECROISSANTS. Le tri Hilbert derive sa cle de
        // `centrality_cached` : avec des marqueurs croissants la
        // permutation calculee est l'IDENTITE, et tout test qui pretend
        // observer « la permutation a eu lieu » serait VIDE. C'est
        // exactement ce que l'assertion de premisse de
        // `un_echec_apres_la_barriere_...` a attrape sur la premiere
        // version de ce fixture.
        for i in 1..N {
            writer
                .update_node(i, marked_node(((N - i) * 1000) as u16))
                .unwrap();
        }
        writer.commit().unwrap();
        // ── (2026-08-24) CE FIXTURE FORGEAIT SA PROPRE DURABILITE ──
        //
        // Il appelait `note_durable_baseline(wal.offset())`, qui AFFIRME
        // que la fenetre de rejeu est vide sans la vider. Consequence
        // observee sur `un_echec_apres_la_barriere_...` : la base contient
        // encore ses `NodeInsert` PRE-permutation, l'ouverture les rejoue
        // depuis l'offset 0 (aucun dict n'existe sur une base pilotee par
        // un `Writer` nu) et RESTAURE la disposition d'avant le tri. La
        // permutation semblait « appliquee une seconde fois » alors que
        // c'etait le WAL qui defaisait le tri.
        //
        // `vider_fenetre_de_rejeu` ne l'affirme pas : elle msync les
        // zones, fsync le WAL, puis le TRONQUE. Il n'y a plus rien a
        // rejouer, donc plus rien a defaire.
        writer.vider_fenetre_de_rejeu().unwrap();
        writer
    }

    /// BLOQUANT, cas 1 — le record de repermutation est ENCORE dans la
    /// fenetre de rejeu.
    ///
    /// Le bras `HilbertRepermuteV2` (cas 2) rend une `Err` ; elle remonte
    /// par `apply(...)?`, sort de `replay_from`, et arrivait dans un
    /// `tracing::warn!` a l'ouverture — canal dont la semantique
    /// historique est « le WAL a ete tronque par une panne, on ouvre ».
    /// La base s'ouvrait donc et servait des requetes sur des zones
    /// INDETERMINEES.
    #[test]
    fn une_annonce_sans_confirmation_fait_refuser_l_ouverture_du_store() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("kb");

        // ── PREMISSE : sans annonce, cette base s'ouvre parfaitement ──
        // Sans cette assertion, un test qui verrait `open` echouer pour
        // n'importe quelle autre raison passerait au vert pour rien.
        drop(base_durable(&path));
        crate::store::SubstrateStore::open(&path)
            .expect("premisse : sans annonce, la base DOIT s'ouvrir normalement");

        // ── L'ACTE : record durable + ANNONCE, puis mort avant la
        //    barriere de durabilite des zones ──
        {
            let writer = Writer::new(
                crate::file::SubstrateFile::open(&path).unwrap(),
                SyncMode::EveryCommit,
            )
            .unwrap();
            writer
                .append_payload(WalPayload::HilbertRepermuteV2 {
                    permutation: perm(),
                    epoch: 1,
                    engram_members: Vec::new(),
                    engram_bitsets: Vec::new(),
                })
                .unwrap();
            writer.fsync_wal().unwrap();
            let subh = writer.substrate();
            let g = subh.lock();
            announce_remap_epoch(&g, 1).unwrap();
        }

        // ── PREMISSE 2 : l'etat sur DISQUE est bien indetermine ──
        {
            let sub = crate::file::SubstrateFile::open(&path).unwrap();
            assert_eq!(
                epoque_indeterminee(&sub).unwrap(),
                Some(1),
                "premisse : l'annonce doit etre durable et sans confirmation"
            );
        }

        // ── L'ASSERTION : le store REFUSE de s'ouvrir ──
        let Err(err) = crate::store::SubstrateStore::open(&path) else {
            panic!("OUVRIR une base a l'etat de repermutation INDETERMINE est le bug")
        };
        assert!(
            matches!(
                err,
                crate::error::SubstrateError::RepermutationIndeterminee { epoch: 1, .. }
            ),
            "obtenu : {err:?}"
        );
    }

    /// BLOQUANT, cas 2 — le record est SORTI de la fenetre de rejeu.
    ///
    /// C'est le cas que le refus au niveau `replay_from` ne peut PAS
    /// attraper, quoi qu'on fasse : le premier `flush()` pousse
    /// `wal_offset_at_flush` au-dela du record, qui n'est alors plus
    /// jamais rejoue. `pending` reste pose indefiniment, `committed`
    /// reste en retard, et la detection disparait. Seule une garde qui
    /// lit l'etat DURABLE de la base (et non le WAL) survit ici.
    #[test]
    fn une_annonce_hors_fenetre_de_rejeu_fait_quand_meme_refuser_l_ouverture() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("kb");
        drop(base_durable(&path));

        // Le RECORD de repermutation est ecrit, l'annonce est posee — puis
        // le processus survit assez longtemps pour faire un flush. C'est
        // ce flush qui pousse `wal_offset_at_flush` AU-DELA du record.
        {
            let store = crate::store::SubstrateStore::open(&path).unwrap();
            store
                .writer()
                .append_payload(WalPayload::HilbertRepermuteV2 {
                    permutation: perm(),
                    epoch: 1,
                    engram_members: Vec::new(),
                    engram_bitsets: Vec::new(),
                })
                .unwrap();
            store.writer().fsync_wal().unwrap();
            {
                let sub = store.writer().substrate();
                let sub = sub.lock();
                announce_remap_epoch(&sub, 1).unwrap();
            }
            store.flush().unwrap();
        }
        let offset_flush = crate::dict::DictSnapshot::load(&path.join("substrate.dict"))
            .unwrap()
            .wal_offset_at_flush;
        assert!(
            offset_flush > 0,
            "premisse : le flush doit avoir publie un offset qui DEPASSE le record"
        );

        let sub = crate::file::SubstrateFile::open(&path).unwrap();
        // ── PREMISSE, ET C'EST TOUT LE PROPOS DU TEST ──
        // Le rejeu, lance exactement comme le fait l'ouverture, ne voit
        // RIEN : le refus au niveau `replay_from` est structurellement
        // aveugle ici. Si cette assertion tombait, le test ne prouverait
        // plus que la garde de `from_substrate` est porteuse.
        crate::replay::replay_from(&sub, offset_flush)
            .expect("premisse : le rejeu depuis l'offset de flush ne voit plus le record");
        assert_eq!(
            epoque_indeterminee(&sub).unwrap(),
            Some(1),
            "premisse : l'annonce est toujours posee, et plus personne ne la verra par le WAL"
        );
        drop(sub);

        let Err(err) = crate::store::SubstrateStore::open(&path) else {
            panic!("la garde doit tenir MEME quand le rejeu ne voit plus le record")
        };
        assert!(
            matches!(
                err,
                crate::error::SubstrateError::RepermutationIndeterminee { epoch: 1, .. }
            ),
            "obtenu : {err:?}"
        );
    }

    /// POINT 2 — une annonce suivie d'un echec AVANT toute mutation doit
    /// laisser une base parfaitement ouvrable.
    ///
    /// Ce test est indissociable des deux precedents : sans lui, le refus
    /// d'ouverture transforme un ENOSPC transitoire pendant un tick de
    /// warden en base non ouvrable au redemarrage suivant, alors que ses
    /// donnees sont intactes. Le correctif creerait la panne.
    ///
    /// L'echec est injecte dans `ensure_room` par le failpoint
    /// `cfg(test)` — c'est la seule facon d'atteindre cette fenetre, qui
    /// n'est declenchable par aucune entree de l'API publique.
    #[test]
    fn une_annonce_suivie_d_un_echec_avant_mutation_laisse_la_base_ouvrable() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("kb");
        let writer = base_durable(&path);

        // ── PREMISSE 1 : cette base n'a JAMAIS ete repermutee ──
        assert!(
            !path.join(REMAP_EPOCH_ZONE).exists(),
            "premisse : la zone d'epoch ne doit pas exister avant la premiere annonce"
        );

        // Le prochain `ensure_room` sur la zone nœuds echouera : c'est
        // exactement le premier point faillible APRES l'annonce et AVANT
        // la premiere ecriture de zone.
        crate::writer::armer_echec_ensure_room("substrate.nodes", 0);
        let err = writer
            .bulk_sort_by_hilbert(N, 0, 8, 4)
            .expect_err("le failpoint doit faire echouer le tri");
        assert!(
            format!("{err}").contains("ENOSPC simule"),
            "l'echec doit venir du failpoint, pas d'autre chose : {err}"
        );

        // ── PREMISSE 2 : le failpoint a bien TIRE (il se desarme) ──
        assert!(
            crate::writer::echec_ensure_room_a_tire(),
            "le failpoint n'a jamais tire : le test n'a pas atteint la fenetre visee"
        );
        // ── PREMISSE 3, LA PLUS IMPORTANTE : l'annonce A ETE POSEE ──
        // La zone d'epoch n'existait pas avant ; elle n'a pu naitre que
        // de `announce_remap_epoch`. Sans cette assertion, un test qui
        // echouerait AVANT l'annonce passerait au vert sans rien couvrir.
        assert!(
            path.join(REMAP_EPOCH_ZONE).exists(),
            "l'annonce n'a jamais ete posee : le test ne couvre pas la fenetre visee"
        );
        drop(writer);

        // ── L'ASSERTION : l'annonce a ete EFFACEE, la base est ouvrable ──
        {
            let sub = crate::file::SubstrateFile::open(&path).unwrap();
            assert_eq!(
                epoque_indeterminee(&sub).unwrap(),
                None,
                "annonce non effacee apres un echec AVANT toute mutation : un ENOSPC \
                 transitoire vient de rendre une base intacte non ouvrable"
            );
        }
        let store = crate::store::SubstrateStore::open(&path)
            .expect("un echec avant toute mutation doit laisser la base parfaitement ouvrable");
        drop(store);
    }

    /// M1 — le repertoire blob bascule par `rename`, donc un echec en
    /// cours de remap laisse la colonne INTACTE, pas « absente ».
    ///
    /// Le blob etait la seule famille de side-car ou « absent » est
    /// DEFINITIF : l'arene survit, mais le repertoire qui l'adresse est
    /// perdu, et il n'existe aucun chemin de reconstruction. La version
    /// en place (`copy_from_slice` dans le mmap idx vivant avant `sync`)
    /// rendait cette perte atteignable par un simple rejet de pages
    /// sales.
    #[test]
    fn blob_echec_mi_remap_laisse_la_colonne_intacte_et_lisible() {
        use crate::blob_column::{BlobColSpec, BlobColumnReader, BlobColumnWriter};
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("kb");
        let sub = crate::file::SubstrateFile::create(&path).unwrap();
        let spec = BlobColSpec {
            prop_key_id: 7,
            entity_kind: EntityKind::Node,
        };
        let mut w = BlobColumnWriter::create(&sub, spec).unwrap();
        for i in 1..N {
            w.write_slot(i, &[i as u8; 32]).unwrap();
        }
        w.sync().unwrap();

        // ── PREMISSE : la colonne est lisible en numerotation d'origine ──
        {
            let r = BlobColumnReader::open(&sub, spec)
                .unwrap()
                .expect("premisse : la colonne doit etre lisible avant le remap");
            for i in 1..N {
                assert_eq!(r.read_slot(i).unwrap(), &[i as u8; 32][..]);
            }
        }

        // Echec pendant la construction du TEMPORAIRE.
        // Le temporaire est desormais DATE (<colonne>.remap.<epoch>.tmp) :
        // viser ".remap.tmp" ne matcherait plus rien.
        crate::writer::armer_echec_ensure_room(".tmp", 0);
        let err = w
            .remap_node_slots(&sub, &perm(), 1)
            .expect_err("le failpoint doit faire echouer le remap");
        assert!(format!("{err}").contains("ENOSPC simule"), "{err}");
        assert!(
            crate::writer::echec_ensure_room_a_tire(),
            "le failpoint n'a jamais tire : le remap n'est pas passe par un temporaire"
        );
        drop(w);

        // ── L'ASSERTION : la colonne VIVANTE n'a pas bouge d'un octet ──
        let sub = crate::file::SubstrateFile::open(&path).unwrap();
        let r = BlobColumnReader::open(&sub, spec)
            .unwrap()
            .expect("colonne blob PERDUE apres un echec mi-remap — perte DEFINITIVE");
        for i in 1..N {
            assert_eq!(
                r.read_slot(i).unwrap(),
                &[i as u8; 32][..],
                "le repertoire blob a ete touche avant la bascule atomique"
            );
        }
    }
}
