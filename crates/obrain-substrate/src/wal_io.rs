//! WAL file I/O — writer and reader over the framed [`WalRecord`] format
//! defined in [`crate::wal`].
//!
//! ## Design
//!
//! The WAL is a single append-only file. Every mutation of the substrate
//! produces exactly one [`WalRecord`] written here, and the `fsync`ed byte
//! offset in this file is the durability line — everything before it
//! survives crashes, everything after does not.
//!
//! Three fsync policies are exposed via [`SyncMode`]:
//!
//! * [`SyncMode::EveryCommit`] — **le défaut** (`ObrainDB::open`). Un `fsync`
//!   par lot contenant [`WalRecord::FLAG_COMMIT`]. Depuis le 2026-11-01
//!   (contrat D-DUR, v0.0.1), `Session::commit()` pose ce marqueur à chaque
//!   transaction commitée, explicite ou implicite : un commit acquitté
//!   survit à un `kill -9` (test `durabilite_commit_kill9`). Avant cette
//!   date, seul `flush()` (checkpoint / close) le posait — un commit rendait
//!   `Ok` avec ses octets encore en tampon.
//! * [`SyncMode::Group`] — opt-in (`ObrainDB::open_substrate_with_mode`).
//!   Le marqueur est posé mais AUCUN `fsync` n'est émis par le writer : il
//!   n'y a pas de minuterie. L'appelant borne lui-même sa fenêtre de perte
//!   en appelant [`WalWriter::fsync`] (via `ObrainDB::flush_wal`) tous les N
//!   commits ou en fin de lot. Pour les imports en masse.
//! * [`SyncMode::Never`] — never fsync. Tests and benches only.
//!
//! ## Group commit à needle de durabilité (2026-10-06)
//!
//! Le `fsync` peut aussi être indexé sur une **NEEDLE** — 4 096 records, 4 Mio ou
//! 2 ms — au lieu du LOT. C'est le levier mesuré (`Writer::commit` : 626 rec/s →
//! 1 692 573 rec/s, ×2 704), et c'est un changement de SÉMANTIQUE, pas un réglage :
//! `append` rend alors la main dès la mise en tampon, la durabilité étant acquise
//! dans la fenêtre [`SEUIL_TEMPS`]. Il est donc EXPLICITE
//! ([`WalWriter::activer_group_commit`]) et ÉTEINT par défaut : tant qu'il ne l'est
//! pas, le chemin d'écriture est celui d'avant, bit pour bit.
//!
//! Ce qui NE change PAS : l'ordre physique, les LSN (`lsn = ancre + position`), le
//! format des frames, le CRC, le rejeu et le [`WalReader`]. Un tampon posé d'un
//! bloc produit exactement les octets de N appends successifs — c'est testé
//! octet par octet (`tests/wal_group_commit_needle.rs`).
//!
//! Un record peut toujours exiger la durabilité AVANT le retour, avec
//! [`WalRecord::FLAG_SYNC_ATTENDU`] (jamais écrit sur disque : c'est une demande
//! adressée à l'écrivain) — voir [`WalWriter::commit_point`] et, côté store,
//! `Writer::commit_durable`.
//!
//! ## FIFO multi-écrivains — les six pièces de l'axe APPEND (2026-10-06)
//!
//! Le WAL est une FIFO : N écrivains (N threads, et N PROCESSUS) doivent
//! pouvoir y ajouter sans qu'aucune frame ne disparaisse et sans qu'aucun LSN
//! ne soit réémis. Six mécanismes, tous mesurés par les tests
//! `mw_interprocess_append.rs` / `wal_append_unite.rs` :
//!
//! 1. **Ancre de LSN durable** — un side-car `<wal>.lsn` porte `(ancre,
//!    plancher)` et remplace le compteur `next_lsn` PAR PROCESSUS (qui faisait
//!    que deux processus émettaient tous deux le LSN 1). Elle est avancée par
//!    le checkpoint ÉLU, pas par chaque append.
//! 2. **LSN = ancre + position** — la position est l'offset octet RÉEL de la
//!    frame, lu sous le mutex juste avant l'écriture. L'ordre physique devient
//!    l'ordre LSN par construction (là où `fetch_add` les faisait diverger).
//! 3. **Réconciliation d'ouverture** — trois cas nommés
//!    ([`Reconciliation`]) : ancre adoptée, ré-ancrage en avant (side-car perdu
//!    sur un fichier non vide), troncature étrangère.
//! 4. **Checkpoint ordonné** — l'ancre est publiée AVANT la troncature
//!    ([`WalWriter::truncate`] le fait lui-même : un appelant ne peut pas
//!    inverser l'ordre). Tronquer d'abord ferait RECOUVRIR les LSN du nouveau
//!    segment sur ceux du précédent.
//! 5. **Garde d'append noyau** — la taille RÉELLE du fichier est relue sous le
//!    mutex à chaque append : si elle a diminué, c'est une troncature étrangère
//!    et l'ancre est relue du side-car, jamais d'écriture à l'aveugle.
//! 6. **Lecteur qui refuse les trous** — une plage de zéros SUIVIE DE DONNÉES
//!    est un [`crate::SubstrateError::WalTrou`] NOMMÉ, pas une fin de fichier ni
//!    une frontière de crash silencieuse.
//!
//! L'atomicité inter-processus ne vient PAS du `Mutex<WriterInner>` (qui ne
//! sérialise que les threads d'un même processus) : elle vient de `O_APPEND`,
//! qui fait que le noyau place chaque `write` à la fin du fichier sans qu'aucun
//! écrivain ne puisse écraser un autre. Le Mutex est donc DÉGRADÉ de
//! « garantie » à « ordonnancement » — voir [`WalWriter::append`].
//!
//! ## 7. **Registre d'append mmapé partagé** — `substrate.wal.reg` (2026-10-06)
//!
//! Les six pièces ci-dessus garantissent la FIFO, mais elles la font PAYER : en
//! coordination partagée, la garde d'append relit la taille RÉELLE du fichier
//! (`statx`, 802 ns mesurés) avant chaque `write`, puis l'observe après — **trois
//! appels système par record**, et un LSN qui n'est qu'une BORNE INFÉRIEURE de la
//! position. Le registre remplace la MESURE par une RÉSERVATION : une page
//! `MAP_SHARED` porte quatre mots atomiques (`tete_reservee`, `fin_publiee`,
//! `generation`, `ancre_partagee`), un `compare_exchange` y rend l'offset EXACT, et
//! un spin sur `fin_publiee` attend que le prédécesseur ait PUBLIÉ sa plage — tout
//! cela **sans un seul appel système**. Le chemin chaud devient **[CAS, `write`]**,
//! et `lsn = ancre_partagee + position` cesse d'être une espérance pour devenir une
//! propriété de la réservation.
//!
//! Ce qui NE change PAS : le format des frames, le CRC, le rejeu, l'ancre durable,
//! l'ordre physique, et le chemin mono-écrivain prouvé (`Aucune`), qui reste
//! bit-identique (aucun registre n'est même créé pour lui). Ce qui change :
//! `VerrouExact` ne prend plus de `flock` par record (l'exactitude vient de la
//! réservation, plus sûre qu'un verrou chaud), l'exactitude du LSN devient une
//! garantie de CONSTRUCTION au lieu d'un espoir, et le `statx` ne survit que sur
//! les chemins d'ANOMALIE (troncature, écrivain mort) — soit une fois par
//! checkpoint ou par crash, jamais par record. Voir [`crate::wal_registre`].
//!
//! ## Replay
//!
//! [`WalReader::iter_from(offset)`] yields decoded `(record, offset, length)`
//! tuples starting at `offset`. Decoding stops at the first frame that fails
//! CRC or short-reads — that's the crash boundary. A TROU (zero run followed by
//! data) is a different thing entirely and is reported as an error, not as a
//! boundary.

use crate::error::{SubstrateError, SubstrateResult};
use crate::wal::{WAL_HEADER_SIZE, WalRecord, crc_over};
use crate::wal_registre::RegistreAppend;
use parking_lot::{Condvar, Mutex};
use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use web_time::Instant;

/// WAL durability policy.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum SyncMode {
    /// Fsync the WAL after every record whose `FLAG_COMMIT` bit is set.
    #[default]
    EveryCommit,
    /// Coalesce fsyncs — batching is handled by the caller (e.g. a group-commit
    /// timer in the store). The writer itself does not fsync; callers must
    /// invoke [`WalWriter::fsync`] explicitly.
    Group,
    /// Never fsync. For tests and micro-benchmarks only.
    Never,
}

// ---------------------------------------------------------------------------
// Group commit à needle de durabilité (2026-10-06)
// ---------------------------------------------------------------------------

/// ── POURQUOI CES TROIS SEUILS, ET PAS UN SEUIL DE LOT ───────────────────────
///
/// MESURÉ (12 agents de bench, 2026-10-06). Le chemin de production
/// (`Writer::commit()` → `WalWriter::append`, lot de 1, `EveryCommit`) tient
/// **644 rec/s** sur ce noyau : 2,65 ms par `fsync`, et UN `fsync` PAR LOT parce
/// que le déclencheur était `sync_mode == EveryCommit && commit_dans_le_lot`
/// (voir `poser_sous_verrou`) — donc un `fsync` par `commit()`, quelle que soit
/// la taille du lot. Le même chemin, mesuré à lot constant, donne 38 379 rec/s à
/// 64 rec/flush et 1 554 539 rec/s à 4096 rec/flush (×40,5) : **c'est la TAILLE
/// DU LOT qui décide, pas le nombre de lots**. D'où la needle : on accumule
/// jusqu'à ce qu'un seuil soit franchi, et on ne `fsync` qu'à ce moment-là.
///
/// Les trois seuils ferment les trois façons de rester en attente :
///   · [`SEUIL_RECORDS`] — borne le DÉBIT d'un flux dense (4 096 records) ;
///   · [`SEUIL_OCTETS`] — borne la MÉMOIRE du tampon (4 Mio) quels que soient
///     les records : des payloads de 1 Mio atteindraient le seuil de records
///     après 4 Gio de RAM, ce qui n'est pas un tampon, c'est une fuite ;
///   · [`SEUIL_TEMPS`] — borne la FENÊTRE DE PERTE (2 ms) : sans lui, un flux
///     qui n'atteint jamais les deux autres seuils garderait ses records en
///     mémoire indéfiniment, et un crash les perdrait tous.
///
/// Un `fsync` n'est fait QUE sur franchissement (ou sur exigence explicite,
/// [`WalRecord::FLAG_SYNC_ATTENDU`]) : c'est ce qui rend le nombre de `fsync`
/// proportionnel aux OCTETS/needle et non aux LOTS. Voir
/// `tests/wal_group_commit_needle.rs`, qui compte les `fsync` réels.
pub const SEUIL_RECORDS: usize = 4096;
/// Seuil d'octets du tampon de group commit (4 Mio).
pub const SEUIL_OCTETS: usize = 4 * 1024 * 1024;
/// Seuil de temps du group commit : la fenêtre de perte MAXIMALE assumée.
pub const SEUIL_TEMPS: Duration = Duration::from_millis(2);

// ---------------------------------------------------------------------------
// Registre d'append mmapé partagé (2026-10-06) — voir `crate::wal_registre`
// ---------------------------------------------------------------------------

/// ── COMBIEN DE TEMPS AVANT DE DÉCLARER UNE RÉSERVATION ABANDONNÉE ───────────
///
/// La chaîne de réservation BLOQUE la tête quand un écrivain meurt entre son
/// `compare_exchange` et son `write` : sa plage reste réservée, et les suivants
/// attendent que la taille réelle l'atteigne. Ce délai est ce qui distingue
/// « il est mort » de « il est préempté » — et il est donc, par construction, un
/// COMPROMIS qu'aucune observation ne peut trancher :
///
/// * **trop court** : on ré-ancre la tête SOUS la réservation d'un pair vivant
///   mais désordonnancé. Sa frame part alors à un offset inférieur à celui qu'il
///   croit, donc son LSN n'est plus sa position — la propriété que tout ce
///   chantier existe pour obtenir ;
/// * **trop long** : un écrivain tué fige le WAL pour tout le monde pendant ce
///   délai (une récupération de crash dure ce temps-là).
///
/// 25 ms est choisi du côté de l'EXACTITUDE : un écrivain vivant tient sa
/// réservation pendant UN `write` (≈1 µs), ou pendant un quantum d'ordonnanceur
/// s'il est préempté (quelques ms sous charge) — 25 ms couvrent plusieurs quanta,
/// alors qu'un écrivain MORT, lui, ne se libère jamais. Le test
/// `mw_chaine_cas::*` (tête bloquée) réduit ce délai par le seam
/// [`WalWriter::forcer_delai_tete_bloquee`].
pub const DELAI_TETE_BLOQUEE: Duration = Duration::from_millis(25);

/// Attente ACTIVE (rotation pure sur un atome partagé, sans aucun appel système)
/// avant de rendre le processeur. L'attente d'un prédécesseur dure le temps d'un
/// `write`, soit ~1 µs : un `yield_now` dès le premier tour coûterait un changement
/// de contexte (des milliers de nanosecondes) pour attendre cinquante fois moins.
/// Au-delà, on rend la main — un prédécesseur préempté ne doit pas transformer les
/// autres écrivains en brûleurs de cœur.
const ATTENTE_ACTIVE: Duration = Duration::from_micros(50);

/// Combien de fois on tente de créer le registre d'append d'un WAL avant de
/// considérer qu'il est INDISPONIBLE. Il ne l'est légitimement que dans des cas
/// durables (média sans fichier, répertoire en lecture seule, `/dev/full`) — mais
/// une cause TRANSITOIRE existe (quota disque atteint : `set_len` échoue en
/// `EFBIG`), et une dégradation définitive pour un quota momentané serait un
/// mauvais échange. Après ces essais, on reste sur le chemin d'avant (deux
/// `statx`), qui est correct : le registre est une optimisation de COÛT.
const MAX_TENTATIVES_REGISTRE: u64 = 16;

/// ── LE TAMPON DU GROUP COMMIT, ET CE QU'IL DOIT AUX LSN ─────────────────────
///
/// L'ORDRE PHYSIQUE ET LES LSN NE CHANGENT PAS. Une frame déposée dans `tampon`
/// est estampillée `lsn = ancre + base + offset_dans_le_tampon`, où `base` est la
/// position FICHIER où le tampon sera posé. Écrire le tampon d'un bloc produit
/// donc exactement les mêmes octets que N appends successifs : le rejeu, le
/// `WalReader` et le format n'ont STRICTEMENT rien à savoir du group commit.
/// C'est la propriété qui rend la pièce sûre — et elle est testée
/// (`needle_le_group_commit_ecrit_les_memes_octets_que_n_appends`).
///
/// `base` est une PRÉDICTION tant que le tampon n'est pas posé (comme le LSN
/// l'est déjà hors verrou aujourd'hui) : à la pose, si la position réelle du
/// fichier a bougé (un pair a écrit), le tampon est RÉ-ESTAMPILLÉ avant le
/// `write` — voir `restampfer_lot`. L'invariant `lsn = ancre + position` tient
/// donc dans tous les régimes, y compris multi-écrivains.
#[derive(Debug)]
struct GroupCommit {
    /// Frames encodées en attente de `write`. Capacité réservée UNE fois à
    /// l'armement : un flush n'alloue rien, et le `resize` de l'encodage non plus.
    tampon: Vec<u8>,
    /// Records dans `tampon` (compteur de needle).
    records: usize,
    /// Octets dans `tampon` (compteur de needle, et borne mémoire).
    octets: usize,
    /// Instant du PREMIER record du tampon courant. C'est lui qui borne la
    /// fenêtre de perte : prendre l'instant du DERNIER ferait qu'un flux continu
    /// (un record toutes les 100 µs) ne franchirait jamais `SEUIL_TEMPS` et
    /// garderait tout en mémoire.
    debut: Instant,
    /// Position fichier contre laquelle les frames du tampon sont estampillées.
    base: u64,
    /// Le tampon contient-il une FRONTIÈRE DE DURABILITÉ (un `FLAG_COMMIT` /
    /// `FLAG_CHECKPOINT`, ou une exigence de synchronisation) ?
    ///
    /// POURQUOI CE BOOLÉEN DÉCIDE DE TOUT, ET POURQUOI IL EST ICI. Le group
    /// commit a été armé pour que `commit()` cesse de payer un `fsync` PAR
    /// COMMIT : la barrière est indexée sur la needle. Mais un `commit()` est
    /// une FRONTIÈRE : l'appelant a déclaré « la transaction est fermée ». En
    /// différer la durabilité au-delà de [`SEUIL_TEMPS`] serait un changement de
    /// contrat, pas une optimisation — et c'est exactement ce que le test
    /// `needle_la_fenetre_de_perte_est_bornee_par_le_seuil_de_temps` interdit
    /// (un seul `FLAG_COMMIT` poussé, puis silence : il DOIT être durable à
    /// l'échéance).
    ///
    /// Une écriture qui n'est PAS une frontière (`PropSetInline` d'un flux
    /// d'ingestion, `flags == 0`) n'a, elle, aucune raison de payer une barrière
    /// toutes les 2 ms : c'est le régime mesuré (557 poses × 1,66 ms = 925 ms
    /// pour 460 octets de moyenne). Pour celle-là, attendre que le tampon vaille
    /// un `fsync` est un GAIN, pas un relâchement — et la borne de temps reste
    /// tenue par [`DELAI_REPORT_MAX`].
    ///
    /// Le booléen est mémorisé au PREMIER record du tampon (comme `ancre` et
    /// `base`), et remis à `false` à chaque pose : à ce moment-là le tampon
    /// repart vide, donc sans frontière.
    contient_frontiere: bool,
    /// Instant du DERNIER `push` ayant mis des octets dans le tampon.
    ///
    /// POURQUOI CE CHAMP EXISTE : c'est la mesure qui distingue « un flux qui
    /// remplit le tampon » (le prochain `push` le rendra rentable, on peut
    /// différer la barrière) de « un flux qui s'est ARRÊTÉ » (plus rien ne
    /// viendra, il faut rendre durable ce qu'on a). Voir
    /// [`GroupCommit::pose_d_echeance_rentable`] et le report de l'échéance.
    dernier_push: Instant,
    /// ── ANCRE AVEC LAQUELLE LES FRAMES DU TAMPON ONT ÉTÉ ESTAMPILLÉES ──────
    /// Mémorisée au PREMIER record du tampon. Elle sert à la POSE : si l'ancre a
    /// changé entre-temps (un checkpoint d'un pair a publié une ancre neuve), les
    /// frames du tampon portent les LSN du segment PRÉCÉDENT — c'est-à-dire des
    /// LSN SOUS la base du segment courant — et il faut les RÉ-ESTAMPILLER même si
    /// la position, elle, n'a pas bougé. Sans ce champ, la condition de
    /// ré-estampillage (`base != position`) laisserait passer ce cas : un tampon
    /// de base 0 posé à la position 0 après une troncature garderait les LSN de
    /// l'ancien segment.
    ancre: u64,
    /// Le group commit est-il ARMÉ ? `false` → le chemin d'aujourd'hui,
    /// bit-pour-bit, lot de 1, `fsync` selon `SyncMode`.
    actif: bool,
    /// Le seuil de TEMPS en vigueur ([`SEUIL_TEMPS`] par défaut). Champ et non
    /// constante : c'est le seul des trois seuils qu'un test doit pouvoir
    /// ÉCARTER pour prouver que c'est bien le seuil de records (ou d'octets) qui
    /// a déclenché le flush. Sans cette possibilité, un test qui pousse 12 288
    /// records mettrait ~25 ms, l'échéance de 2 ms se déclencherait une dizaine de
    /// fois, et le compte de `fsync` ne prouverait plus rien d'exact.
    seuil_temps: Duration,
    /// En dessous de quoi une pose d'ÉCHÉANCE ne mérite pas son `fsync`
    /// ([`SEUIL_FSYNC_RENTABLE`] par défaut). Champ et non constante, pour la
    /// même raison que `seuil_temps` : c'est le seul moyen de MESURER l'effet du
    /// seuil (le mettre à 0 restaure la barrière à chaque pose d'échéance ;
    /// l'augmenter montre où le coût disparaît). Voir
    /// [`GroupCommit::pose_d_echeance_rentable`].
    seuil_fsync_rentable: usize,
}

/// ── TÉLÉMÉTRIE DU GROUP COMMIT (instrumentation de banc, 2026-10-06) ────────
///
/// POURQUOI ELLE EXISTE, ET POURQUOI ELLE EST BRANCHÉE SUR UNE VARIABLE
/// D'ENVIRONNEMENT. Un A/B mesuré (`ingestion_unitaire`, store déjà peuplé,
/// armé vs éteint) donnait 1,39 s → 2,49 s (×0,56) **avec un profil SYSCALL
/// IDENTIQUE** (mêmes `write`, mêmes `fsync`) et seulement +20 % de CPU. Le
/// processus ATTENDAIT ou refaisait un travail invisible. Aucun compteur du
/// WAL ne distinguait ces deux mondes : `poses`, `fsyncs`, `octets_en_tampon`
/// et `records_ecrits` sont identiques dans les deux cas.
///
/// Sans ces compteurs, la seule issue est de DEVINER le mécanisme (le fil de
/// veille ? le condvar ? un verrou ?), et une hypothèse non mesurée sur un
/// chemin d'écriture est exactement ce qu'un WAL ne peut pas se permettre.
/// Chaque champ répond à une hypothèse PRÉCISE, et le tout ne coûte rien quand
/// `OBRAIN_GC_DIAG` n'est pas posé (un `load` d'atome par point).
#[derive(Debug, Default)]
struct DiagGroupe {
    /// H1 — « le fil de veille prend un verrou à chaque réveil ». Combien de
    /// fois le veilleur est entré dans `commit_point`, et combien de temps il a
    /// passé à ATTENDRE le mutex `inner` (le temps qu'il passe verrou en main
    /// est mesuré séparément : c'est du travail, pas de l'attente).
    veille_reveils: AtomicU64,
    veille_attente_inner_ns: AtomicU64,
    veille_tenue_inner_ns: AtomicU64,
    /// Réveils où le tampon était VIDE (rien à poser) : c'est la mesure qui dit
    /// si la needle de TEMPS déclenche des poses inutiles ou seulement des
    /// prises de verrou à vide.
    veille_reveils_vides: AtomicU64,
    /// H2 — « le chemin d'ingestion attend le mutex `inner` ». Temps total
    /// passé à ATTENDRE `self.inner.lock()` dans `push_group_encode`, et temps
    /// total verrou EN MAIN (travail réel : memcpy, ré-estampillage, écriture,
    /// `fsync`).
    push_attente_inner_ns: AtomicU64,
    push_tenue_inner_ns: AtomicU64,
    /// H3 — « la chaîne de ré-estampillage est quadratique ». Nombre d'appels à
    /// `restampfer_lot` et OCTETS recalculés (CRC compris) : c'est la mesure du
    /// travail caché. Un compte d'octets proche de N × tampon pour N
    /// enregistrements prouve une passe par enregistrement sur un tampon qui
    /// grandit.
    restamps: AtomicU64,
    restamps_octets: AtomicU64,
    /// Nombre de FOIS où la prédiction de position hors verrou a été trouvée
    /// fausse (`prediction_juste == false`) : la cause déclenchante.
    predictions_fausses: AtomicU64,
    /// Temps passé DANS la pose (réservation de chaîne + `write` + `fsync`) :
    /// c'est la mesure qui sépare le coût du VERROU de celui du MÉDIA.
    pose_ns: AtomicU64,
    /// Nombre de poses du tampon (écritures groupées réelles) et octets posés.
    poses_groupe: AtomicU64,
    poses_groupe_octets: AtomicU64,
    /// Attente du mutex `inner` par `armer_la_veille` (l'appel par
    /// enregistrement) : c'est le seul point de contention ajouté au chemin
    /// chaud par le group commit.
    armer_attente_inner_ns: AtomicU64,
    armer_appels: AtomicU64,
}

impl DiagGroupe {
    /// Le diagnostic est-il demandé ? (un `load` détendu hors du chemin chaud)
    fn actif() -> bool {
        static ACTIF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ACTIF.get_or_init(|| std::env::var("OBRAIN_GC_DIAG").is_ok_and(|v| v == "1"))
    }

    fn ajouter(c: &AtomicU64, d: Duration) {
        c.fetch_add(d.as_nanos() as u64, Ordering::Relaxed);
    }

    /// Rendu BRUT, pour le banc.
    pub(crate) fn lignes(&self) -> String {
        let ms = |n: u64| n as f64 / 1e6;
        format!(
            "gc-diag veille_reveils={} veille_reveils_vides={} veille_attente_inner_ms={:.2} \
             veille_tenue_inner_ms={:.2} push_attente_inner_ms={:.2} push_tenue_inner_ms={:.2} \
             armer_appels={} armer_attente_inner_ms={:.2} pose_ms={:.2} predictions_fausses={} restamps={} \
             restamps_Mio={:.2} poses_groupe={} poses_groupe_Mio={:.2}",
            self.veille_reveils.load(Ordering::Relaxed),
            self.veille_reveils_vides.load(Ordering::Relaxed),
            ms(self.veille_attente_inner_ns.load(Ordering::Relaxed)),
            ms(self.veille_tenue_inner_ns.load(Ordering::Relaxed)),
            ms(self.push_attente_inner_ns.load(Ordering::Relaxed)),
            ms(self.push_tenue_inner_ns.load(Ordering::Relaxed)),
            self.armer_appels.load(Ordering::Relaxed),
            ms(self.armer_attente_inner_ns.load(Ordering::Relaxed)),
            ms(self.pose_ns.load(Ordering::Relaxed)),
            self.predictions_fausses.load(Ordering::Relaxed),
            self.restamps.load(Ordering::Relaxed),
            self.restamps_octets.load(Ordering::Relaxed) as f64 / (1024.0 * 1024.0),
            self.poses_groupe.load(Ordering::Relaxed),
            self.poses_groupe_octets.load(Ordering::Relaxed) as f64 / (1024.0 * 1024.0),
        )
    }
}

impl GroupCommit {
    fn neuf() -> Self {
        Self {
            tampon: Vec::new(),
            records: 0,
            octets: 0,
            debut: Instant::now(),
            base: 0,
            contient_frontiere: false,
            dernier_push: Instant::now(),
            ancre: 0,
            actif: false,
            seuil_temps: SEUIL_TEMPS,
            // Le seuil est lu à l'ARMEMENT (voir `activer_group_commit`) : la
            // surcharge par variable d'environnement est un OUTIL DE MESURE
            // (banc group commit), pas un réglage de service.
            seuil_fsync_rentable: SEUIL_FSYNC_RENTABLE,
        }
    }

    /// Le tampon est-il vide ? (aucun `write` n'est dû)
    fn vide(&self) -> bool {
        self.tampon.is_empty()
    }

    /// Un seuil est-il franchi ? Si oui, le prochain `commit_point` fait le
    /// `fsync` — c'est LA règle « le fsync est indexé sur la needle ».
    fn needle_franchie(&self) -> bool {
        self.records >= SEUIL_RECORDS || self.octets >= SEUIL_OCTETS
    }

    /// ── LA POSE D'ÉCHÉANCE EST-ELLE ASSEZ PLEINE POUR PAYER UN `fsync` ? ────
    ///
    /// POURQUOI CE PRÉDICAT EXISTE (défaut mesuré le 2026-10-06, ×0,42-0,60).
    ///
    /// Un `fsync` n'est pas un `memcpy` : il COÛTE, et son coût est FIXE (une
    /// barrière de média). Le diviser par le nombre d'octets qu'il rend durables
    /// est donc l'unique façon de savoir s'il est rentable. L'échéance de TEMPS,
    /// elle, ne regarde pas la taille : elle pose ce qu'il y a, même 40 octets.
    ///
    /// MESURÉ (ingestion incrémentale, store déjà peuplé, e1000e : 9 302
    /// enregistrements de 52 octets en ~2 s, soit ~244 o/ms) : l'échéance de 2 ms
    /// tombait sur des tampons de ~460 octets, 557 fois. Chaque pose payait alors
    /// un `fsync` de 1,66 ms **sous le verrou `inner`** — 925 ms de mur sur 2,33 s,
    /// pendant lesquelles le fil d'ingestion attendait. Le disque, lui, n'y est
    /// pour rien : le même `fsync` mesuré seul sur le même média coûte 0-8 µs
    /// (`examples/perf_fsync_wal.rs`). Ces 1,66 ms sont le prix de 557 poses
    /// minuscules, pas celui du média.
    ///
    /// La règle ajoutée est donc : **une pose d'ÉCHÉANCE ne `fsync` que si elle
    /// vaut un `fsync`**. En dessous, elle POSE quand même (le `write` reste
    /// groupé, les octets atteignent le noyau) mais sans barrière — et le
    /// prochain enregistrement, qui grossit le tampon, transforme cette pose
    /// minuscule en pose rentable.
    ///
    /// ── LA FENÊTRE DE PERTE RESTE BORNÉE, ET ELLE EST MÊME PLUS PETITE ──────
    /// La borne de [`SEUIL_TEMPS`] n'est pas relâchée : on POSE toujours à
    /// l'échéance (donc les octets ne peuvent pas rester indéfiniment en
    /// mémoire), et le `fsync` est simplement reporté à la première des
    /// occasions qui le justifie — seuil de records/octets franchi, exigence
    /// `FLAG_SYNC_ATTENDU` d'un enregistrement, `fsync()` explicite de
    /// l'appelant, pose suivante assez pleine, ou `Drop`. Le pire cas passe donc
    /// de « jusqu'à 2 ms de records non durables » à « le premier de : 2 ms, ou
    /// le franchissement d'un seuil, ou le prochain enregistrement qui rend la
    /// pose rentable » — c'est-à-dire une fenêtre PLUS COURTE en octets non
    /// durables, jamais plus longue en temps.
    fn pose_d_echeance_rentable(&self) -> bool {
        self.octets >= self.seuil_fsync_rentable
    }

    /// ── L'ÉCHÉANCE EST-ELLE UN POINT DE DURABILITÉ, OU UNE SIMPLE OCCASION ? ─
    ///
    /// C'EST LA QUESTION QUE CE CHANTIER A DÛ TRANCHER, et elle se répond avec le
    /// contenu du tampon, pas avec une préférence de performance :
    ///
    /// * **oui** si le tampon porte une FRONTIÈRE (`FLAG_COMMIT` /
    ///   `FLAG_CHECKPOINT` / exigence de synchronisation) — un appelant a déclaré
    ///   une transaction fermée, et l'échéance est le moment où le group commit a
    ///   promis de la rendre durable (fenêtre ≤ [`SEUIL_TEMPS`]) ;
    /// * **oui** si le tampon vaut déjà une barrière — payer maintenant est le
    ///   meilleur usage possible de cette barrière ;
    /// * **non** sinon — personne n'attend ces octets. L'échéance n'est alors
    ///   qu'une OCCASION : on pose (les octets quittent la mémoire) et on garde
    ///   la barrière pour une pose qui la rentabilise, sous la borne dure
    ///   [`DELAI_REPORT_MAX`].
    ///
    /// Un `flush_wal()` explicite, un `commit_point()` appelé par l'appelant et
    /// le `Drop` restent, eux, des points de durabilité INCONDITIONNELS : ils ne
    /// passent pas par cette décision.
    fn echeance_est_un_point_de_durabilite(&self) -> bool {
        self.contient_frontiere || self.pose_d_echeance_rentable()
    }

    /// ── PEUT-ON REPORTÉ LA BARRIÈRE D'ÉCHÉANCE À LA POSE SUIVANTE ? ────────
    ///
    /// Oui si, ET SEULEMENT SI, trois conditions tiennent ensemble :
    ///
    /// 1. le tampon n'est pas encore rentable (`pose_d_echeance_rentable`) :
    ///    payer une barrière pour quelques centaines d'octets est le gaspillage
    ///    mesuré ;
    /// 2. un `push` a eu lieu il y a MOINS de [`SEUIL_TEMPS`] : le flux est
    ///    VIVANT, le prochain enregistrement est imminent, et le report ne fait
    ///    donc pas durer la fenêtre de perte — il laisse simplement le temps au
    ///    tampon d'atteindre une taille qui vaut une barrière ;
    /// 3. le tampon est JEUNE (`debut + DELAI_REPORT_MAX` pas dépassé) : borne
    ///    dure, pour qu'un flux lent mais régulier (un enregistrement par
    ///    seconde) ne diffère pas indéfiniment. C'est ce qui garde la fenêtre de
    ///    perte bornée en TEMPS, et pas seulement en octets.
    ///
    /// Le cas « flux mort » (condition 2 fausse) est celui du test
    /// `needle_la_fenetre_de_perte_est_bornee_par_le_seuil_de_temps` : un seul
    /// enregistrement poussé, puis plus rien. Là, différer serait une PERTE de
    /// durabilité — on fsynce donc à l'échéance, ce qui est le comportement
    /// d'origine, intact.
    fn peut_reporter_la_barriere(&self) -> bool {
        // 1. LE TAMPON PORTE-T-IL UNE FRONTIÈRE DE DURABILITÉ ? Si oui, on ne
        //    reporte JAMAIS : c'est le contrat de `commit()`, et il n'est pas
        //    négociable pour gagner des barrières. C'est ce cas — et lui seul —
        //    que garde le test de la fenêtre de perte.
        if self.contient_frontiere {
            return false;
        }
        // 2. Le tampon vaut-il déjà une barrière ? Alors il n'y a rien à
        //    reporter : la payer maintenant est le meilleur usage possible.
        if self.pose_d_echeance_rentable() {
            return false;
        }
        // 3. Le flux est-il VIVANT ? Un `push` récent veut dire que le prochain
        //    enregistrement est imminent : le tampon va grossir, et la barrière
        //    couvrira alors beaucoup plus d'octets pour le même prix. Si le flux
        //    est mort, plus rien ne viendra : il faut rendre durable ce qu'on a.
        let maintenant = Instant::now();
        if maintenant.saturating_duration_since(self.dernier_push) >= SEUIL_TEMPS {
            return false;
        }
        // 4. BORNE DURE : un flux lent mais régulier ne doit pas différer
        //    indéfiniment (voir `DELAI_REPORT_MAX`).
        maintenant.saturating_duration_since(self.debut) < DELAI_REPORT_MAX
    }
}

/// ── EN DESSOUS DE QUOI UN `fsync` D'ÉCHÉANCE N'EST PAS RENTABLE ─────────────
///
/// 64 Kio : quatre pages. Le raisonnement n'est pas « un chiffre rond » mais
/// « le coût d'une barrière divisé par un débit utile ». Un `fsync` de barrière
/// coûte, sur un média lent réellement synchrone (pas la page cache d'un
/// `/tmp`), de l'ordre de la milliseconde ; le débit d'un `write` séquentiel de
/// WAL est de l'ordre du Gio/s. Rendre durable moins de 64 Kio à chaque
/// milliseconde revient donc à payer une barrière pour 6 % de ce qu'elle
/// pourrait couvrir — c'est exactement le régime mesuré (460 octets par pose,
/// 557 poses), et c'est ce que ce seuil interdit.
///
/// Pourquoi pas plus gros : le seuil ne doit pas retarder la durabilité d'un
/// flux LENT (un store qui écrit un record par seconde atteindrait 64 Kio en
/// ~4 minutes). C'est [`SEUIL_TEMPS`] qui garde cette borne-là : à l'échéance on
/// POSE toujours, et `Drop`/`fsync()`/`commit_point()` restent les points de
/// durabilité explicites. Le seuil ne fait que décider si une pose d'échéance
/// mérite SA barrière.
pub const SEUIL_FSYNC_RENTABLE: usize = 64 * 1024;

/// ── COMBIEN DE TEMPS ON PEUT REPORTÉ UNE BARRIÈRE D'ÉCHÉANCE ───────────────
///
/// [`SEUIL_TEMPS`] × 32 = 64 ms. POURQUOI UNE SECONDE BORNE, alors que le seuil
/// d'octets suffit presque toujours : parce qu'un flux LENT mais RÉGULIER (un
/// enregistrement toutes les 2 ms, soit 26 o/ms) n'atteindrait 64 Kio qu'au bout
/// de ~2,5 s. Sans cette borne, la fenêtre de perte d'un tel flux serait de 2,5 s
/// — c'est-à-dire que `SEUIL_TEMPS` ne serait plus une borne du tout. Avec elle,
/// la barrière tombe au plus tard 64 ms après le premier octet non durable, ce
/// qui est 32 fois la borne nominale mais reste un ordre de grandeur raisonnable
/// pour une fenêtre de perte, et ne coûte que ~15 barrières par seconde dans le
/// pire cas.
///
/// La borne est exprimée en multiples de [`SEUIL_TEMPS`] parce que c'est la
/// grandeur qui a un sens : « 32 échéances ratées, puis on paie ».
pub const DELAI_REPORT_MAX: Duration = Duration::from_millis(64);

/// ── LA VEILLE PARTAGÉE (mutex + condvar), HORS DU `WalWriter` ──────────────
///
/// POURQUOI CE TYPE EXISTE SÉPARÉMENT — c'est un défaut trouvé par le test du
/// Drop, pas une commodité. Un `MutexGuard` EMPRUNTE le mutex qu'il garde : si
/// l'état de veille vivait dans le `WalWriter`, le veilleur devrait tenir un
/// `Arc<WalWriter>` pendant TOUTE son attente (qui dure, par construction,
/// jusqu'à l'échéance). Le compte de références fortes ne tomberait alors jamais
/// à zéro, le `Drop` du `WalWriter` — qui est précisément ce qui POSE le tampon
/// restant — ne s'exécuterait jamais, et les records que `append` a acceptés
/// seraient perdus en silence (mesuré : fichier vide après 100 records, écrivain
/// jamais détruit).
///
/// Ici le veilleur tient `Arc<VeillePartagee>` — un mutex, un condvar, AUCUN état
/// d'écriture — et n'amorce son `Weak<WalWriter>` que le temps d'une pose.
#[derive(Debug)]
struct VeillePartagee {
    etat: Mutex<Veille>,
    reveil: Condvar,
}

impl VeillePartagee {
    fn neuve() -> Self {
        Self {
            etat: Mutex::new(Veille::Repos),
            reveil: Condvar::new(),
        }
    }
}

/// ── L'ÉTAT DE LA VEILLE DE TEMPS (`SEUIL_TEMPS`) ────────────────────────────
///
/// C'est un petit automate à trois états plutôt qu'un `bool` + un `Instant` :
/// le veilleur doit pouvoir distinguer « rien à faire » (dormir SANS échéance —
/// coût nul sur un WAL au repos) de « un tampon attend » (réveil à l'échéance).
/// Un sondage périodique (un réveil toutes les 500 µs pour vérifier) coûterait
/// 2 000 réveils/s par store pour ne rien faire 99,99 % du temps.
#[derive(Debug)]
enum Veille {
    /// Rien en attente : le veilleur dort jusqu'à ce qu'un push le réveille.
    Repos,
    /// Un tampon attend d'être posé : réveil à `deadline`.
    Arme { deadline: Instant },
    /// Arrêt demandé (Drop du `WalWriter`) : le veilleur sort.
    Arrete,
}

// ── LE BUFFER D'ENCODAGE, PAR THREAD ────────────────────────────────────────
//
// Le buffer dans lequel un lot est encodé HORS du mutex (les threads encodent en
// parallèle ; seul le `write` est sous le verrou). Réutilisé d'un record à
// l'autre : c'est ce qui a fait tomber l'encodage de 0,108 µs à 0,005 µs par
// record (2026-09-06). Sorti de `append_batch` en 2026-10-06 : les trois chemins
// d'écriture (lot, push du group commit, pose du tampon) le partagent.
thread_local! {
    static ENCODE_BUF: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Ce qui déclenche la pose du tampon du group commit.
///
/// Les trois cas ne diffèrent QUE par le `fsync`, et les confondre coûterait soit
/// une durabilité perdue (oublier le `fsync` d'une échéance), soit un `fsync`
/// inutile par appel (`fsync()` explicite sur un tampon vide), soit un `fsync`
/// absurde (avant une troncature qui va jeter les octets).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum PoseTampon {
    /// Needle franchie ou record exigeant la synchronisation : écrire, puis
    /// fsync. Sur un tampon vide, RIEN (il n'y a rien de neuf à rendre durable —
    /// et compter un `fsync` fantôme fausserait la mesure de la needle).
    Durable,
    /// `fsync()` explicite : fsyncer MÊME sur un tampon vide. C'est le contrat du
    /// mode `Group`, où les frames sont écrites sans `fsync` et où c'est
    /// l'appelant qui décide quand rendre durable.
    DurableToujours,
    /// Vidage avant troncature : écrire pour que l'ancre compte juste, sans
    /// `fsync` — le contenu du segment va être jeté.
    VidageSansFsync,
}

/// Refuse une frame qu'un seul `write` ne peut pas poser (voir
/// [`MAX_FRAME_BYTES`]) — contrôle fait APRÈS l'encodage, AVANT tout `write` :
/// rien n'a été posé quand on refuse.
#[inline]
fn verifier_borne(taille_frame: usize, borne: usize) -> SubstrateResult<()> {
    if taille_frame > borne {
        return Err(SubstrateError::WalFrameTropGrande {
            taille: taille_frame,
            max: borne,
        });
    }
    Ok(())
}

/// ── RÉ-ESTAMPILLER UN LOT DÉJÀ ENCODÉ (et non le ré-encoder) ────────────────
///
/// Le LSN est estampillé HORS verrou, sur une position PRÉDITE ; si la position
/// réelle diffère (WAL partagé, ou tampon du group commit qu'un pair a décalé),
/// les frames doivent porter la BONNE. Le code d'avant gardait les
/// `Vec<WalRecord>` du lot pour les RÉ-ENCODER ; la pièce 2 supprime justement ce
/// `Vec`. Or une frame est AUTO-DESCRIPTIVE (longueur `u32` en tête, voir
/// `encoder_frame`) : on peut donc reparcourir le lot encodé, patcher les 8 octets
/// de LSN — et, parce que le CRC couvre le LSN, RECALCULER le CRC de la frame
/// concernée. Aucun record à garder en vie, aucune allocation.
///
/// C'est un chemin d'ANOMALIE (il ne tourne que quand la prédiction a échoué) :
/// il peut donc se permettre deux tranches de CRC par frame, là où l'encodage
/// nominal fait tout en une passe.
fn restampfer_lot(buf: &mut [u8], ancre: u64, position: u64) -> SubstrateResult<()> {
    let mut i = 0usize;
    while i < buf.len() {
        if buf.len() - i < WAL_HEADER_SIZE {
            return Err(SubstrateError::WalBadFrame(
                "ré-estampillage : reste de tampon trop court pour un en-tête de frame".into(),
            ));
        }
        let length = u32::from_le_bytes(buf[i..i + 4].try_into().unwrap()) as usize;
        let fin = i + WAL_HEADER_SIZE + length;
        if fin > buf.len() {
            return Err(SubstrateError::WalBadFrame(
                "ré-estampillage : la longueur déclarée par une frame dépasse le tampon".into(),
            ));
        }
        // Le LSN de la frame i est `ancre + position + décalage_dans_le_tampon` :
        // exactement la valeur qu'aurait eue un append immédiat à cette position.
        buf[i + 8..i + 16].copy_from_slice(&(ancre + position + i as u64).to_le_bytes());
        let crc = crc_over(&buf[i..i + 24], &buf[i + WAL_HEADER_SIZE..fin]);
        buf[i + 24..i + 28].copy_from_slice(&crc.to_le_bytes());
        i = fin;
    }
    Ok(())
}

/// ── LA POSE D'ÉCHÉANCE VAUT-ELLE SON `fsync` ? (décision du VEILLEUR) ──────
///
/// POURQUOI CE N'EST PAS DANS `push_group_encode`. Le `push` a une autre
/// question à trancher (« la needle est-elle franchie ? ») et sa réponse est
/// OUI dès 4 096 records ou 4 Mio : il ne paie donc jamais de barrière pour
/// quelques octets. C'est l'ÉCHÉANCE DE TEMPS qui pose des tampons minuscules,
/// parce qu'elle est la seule à ne pas regarder la taille — d'où la décision
/// ici, au seul endroit qui en a besoin.
///
/// Le seuil est lu SOUS le verrou d'écriture (comme partout) : c'est un champ,
/// pas une constante, pour que le banc puisse le balayer.
/// Ce que le veilleur doit faire à l'échéance (voir le corps du veilleur).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum DecisionVeilleur {
    /// Ne rien poser : repousser l'échéance d'un `SEUIL_TEMPS`.
    Reporter,
    /// Poser AVEC barrière (le tampon vaut un `fsync`, ou le flux est mort).
    Durable,
    /// Poser SANS barrière (les octets quittent la mémoire, sans `fsync`).
    Sec,
}

fn decision_du_veilleur(w: &WalWriter) -> DecisionVeilleur {
    let g = w.inner.lock();
    if g.groupe.echeance_est_un_point_de_durabilite() {
        // Contrat tenu : `write` + `fsync` à l'échéance.
        DecisionVeilleur::Durable
    } else if g.groupe.peut_reporter_la_barriere() {
        // Personne n'attend ces octets et le flux est vivant : la barrière est
        // reportée à une pose qui la rentabilise (c'est le gain mesuré).
        DecisionVeilleur::Reporter
    } else {
        // On ne reporte plus (flux mort, ou borne dure atteinte) mais la pose
        // reste trop petite pour une barrière : on ÉCRIT sans `fsync`.
        DecisionVeilleur::Sec
    }
}

/// ── LE VEILLEUR DE LA NEEDLE DE TEMPS ───────────────────────────────────────
///
/// UN seul thread par écrivain groupé, qui ne fait rien tant qu'aucun tampon
/// n'attend : il dort sur le condvar SANS échéance (`Veille::Repos`), et c'est un
/// push qui l'arme. À l'échéance (`debut + SEUIL_TEMPS`) il pose le tampon —
/// `write` + `fsync` — ce qui BORNE la fenêtre de perte sans jamais bloquer
/// l'appelant : le push, lui, n'a fait qu'un `memcpy`.
///
/// Il tient un `Weak` : il ne maintient pas l'écrivain en vie, et il sort dès que
/// celui-ci disparaît (le `Drop` pose `Veille::Arrete` et notifie, puis pose le
/// tampon restant).
fn veiller_la_needle(veille: Arc<VeillePartagee>, faible: std::sync::Weak<WalWriter>) {
    loop {
        // ── ON ATTEND SANS TENIR L'ÉCRIVAIN (voir `VeillePartagee`) ──────────
        let mut etat = veille.etat.lock();
        match *etat {
            Veille::Arrete => return,
            Veille::Repos => {
                // Coût nul au repos : aucune échéance, aucun réveil périodique.
                veille.reveil.wait(&mut etat);
                continue;
            }
            Veille::Arme { deadline } => {
                let reste = deadline.saturating_duration_since(Instant::now());
                if !reste.is_zero() {
                    // Réveil à l'échéance, ou plus tôt si un push plus ancien
                    // réarme (on reboucle alors et on recalcule le reste).
                    veille.reveil.wait_for(&mut etat, reste);
                    continue;
                }
                // `Repos` est posé SOUS le verrou, puis on le RELÂCHE : la suite
                // (décision, pose) peut prendre `inner` sans risquer l'inversion
                // d'ordre décrite dans la branche `Reporter`.
                *etat = Veille::Repos;
                drop(etat);
                // Amorçage COURT : l'écrivain n'est tenu que le temps de la pose.
                let Some(w) = faible.upgrade() else {
                    return;
                };
                // ── INSTRUMENTATION (OBRAIN_GC_DIAG=1) : OÙ LE VEILLEUR ATTEND ─
                // `commit_point` commence par `self.inner.lock()` : c'est le SEUL
                // point par lequel le fil de veille peut entrer en conflit avec le
                // fil d'ingestion. On chronomètre donc séparément l'ATTENTE du
                // verrou et sa TENUE (l'écriture + le `fsync` sous verrou, qui
                // bloquent l'ingestion pour toute la durée de la pose).
                let diag_actif = DiagGroupe::actif();
                let t_avant = diag_actif.then(Instant::now);
                if diag_actif {
                    w.diag.veille_reveils.fetch_add(1, Ordering::Relaxed);
                }
                let vide = w.octets_en_tampon.load(Ordering::Acquire) == 0;
                // ── RIEN EN TAMPON : ON NE PREND MÊME PAS LE VERROU ───────────
                // C'est le cas du chemin SANS WAL (ou d'un WAL déjà vidé). Le
                // veilleur ne doit alors coûter NI verrou, NI `write`, NI `fsync` :
                // c'est la moitié « la needle de temps ne coûte rien quand le WAL
                // est inactif » du contrat. `octets_en_tampon` est un atome publié
                // sous le verrou d'écriture : le lire ici est exact, et un tampon
                // qui se remplit juste après réarmera la veille (un `push` arme
                // dès qu'il laisse des octets).
                if vide {
                    drop(w);
                    continue;
                }
                // ── DÉCISION D'ÉCHÉANCE : REPORT, POSE DURABLE, OU POSE SÈCHE ──
                // Trois cas, et la ligne entre eux est mesurée (voir
                // `GroupCommit::peut_reporter_la_barriere`) :
                //
                //   · REPORT — le flux est VIVANT et le tampon est encore trop
                //     petit pour qu'une barrière soit rentable : on ne pose RIEN,
                //     on repousse l'échéance. Le `write` groupé de la prochaine
                //     échéance couvrira plus d'octets avec la MÊME barrière : c'est
                //     là, exactement, que les 557 barrières de 1,66 ms mesurées
                //     cessent d'être payées.
                //   · POSE DURABLE — tampon rentable, ou flux mort, ou report
                //     épuisé : `write` + `fsync`. C'est le cas du contrat
                //     « un record seul est durable après l'échéance ».
                //   · POSE SÈCHE — tampon non rentable mais report refusé pour une
                //     autre raison : on écrit (les octets quittent la mémoire), on
                //     ne paie pas la barrière maintenant.
                let decision = decision_du_veilleur(&w);
                if decision == DecisionVeilleur::Reporter {
                    // ── RÉARMEMENT (SANS PRENDRE NI `etat` NI `inner` ICI) ─────
                    // L'ÉTAT EST DÉJÀ À `Repos` : la ligne `*etat = Veille::Repos`
                    // ci-dessus l'a posé, et on tient ce verrou en le relâchant.
                    // On ne le reprend donc PAS pour reprogrammer l'échéance : le
                    // faire obligerait à lire `seuil_temps` SOUS `etat` (puisqu'il
                    // faut la valeur pour calculer la deadline), c'est-à-dire à
                    // prendre `inner` après `etat` — l'ordre INVERSE de
                    // `armer_la_veille` et de `Drop`, donc un interblocage à deux
                    // fils (l'ingestion tient `inner` et attend `etat`, le Drop
                    // tient `etat` et attend `inner`).
                    //
                    // On laisse donc simplement l'échéance là où elle est : le
                    // prochain `push` réarme (il arme à chaque fois qu'il laisse
                    // des octets), et si aucun `push` ne vient, c'est le `Drop` qui
                    // pose. Aucun octet ne peut donc rester en mémoire sans que
                    // quelqu'un soit chargé de le poser — c'est la seule propriété
                    // dont ce chemin a besoin. (`etat` a déjà été relâché plus
                    // haut : on ne le reprend pas.)
                    continue;
                }
                let pose = if decision == DecisionVeilleur::Durable {
                    PoseTampon::Durable
                } else {
                    PoseTampon::VidageSansFsync
                };
                if let Err(e) = w.pose_du_veilleur_diag(diag_actif, t_avant, pose) {
                    // Le tampon est GARDÉ (voir `poser_le_tampon`) : le prochain
                    // réveil, ou `fsync()`, ou le Drop réessaiera. On le dit,
                    // parce qu'un échec d'E/S qui ne remonte à personne est
                    // exactement ce qu'un WAL ne doit pas faire.
                    tracing::warn!(
                        error = %e,
                        "group commit : la pose du tampon a échoué à l'échéance de la needle \
                         de temps — le tampon est conservé et sera reposé"
                    );
                }
                drop(w);
            }
        }
    }
}

/// ── CE QU'UN SEUL `write` PEUT POSER D'UN BLOC ──────────────────────────────
///
/// `MAX_RW_COUNT` vaut 0x7ffff000 sur Linux (~2 Gio − 4 Kio) : au-delà, le noyau
/// DÉCOUPE l'écriture et rend un compte partiel. Une frame plus grosse qu'un
/// `write` ne peut donc pas être posée atomiquement, et un `write_all` rejoué
/// après coup fabriquerait une frame HYBRIDE. On borne à 1 Gio : la marge est
/// confortable, et aucune frame réelle n'approche cette taille (le plus gros
/// payload du format porte un vecteur de propriété).
pub const MAX_FRAME_BYTES: usize = 1 << 30;

/// Suffixe du side-car d'ancre, ajouté au chemin du WAL :
/// `substrate.wal` → `substrate.wal.lsn`.
pub const SUFFIXE_ANCRE_LSN: &str = ".lsn";

/// Magie du side-car d'ancre — « Obrain WAL Lsn Anchor ».
const MAGIE_ANCRE: [u8; 4] = *b"OBLA";
/// Version du side-car. Toute autre valeur = side-car illisible (traité comme
/// ABSENT, jamais comme zéro : un side-car d'une version future ne doit pas
/// faire croire qu'aucune ancre n'a jamais été publiée).
const VERSION_ANCRE: u16 = 1;
/// 32 octets : magie(4) + version(2) + réservé(2) + ancre(8) + plancher(8) +
/// crc(4) + réservé(4).
const TAILLE_ANCRE: usize = 32;

/// ── L'ANCRE INITIALE (et pourquoi 1, pas 0) ─────────────────────────────────
///
/// `lsn = ancre + position`, et un WAL neuf a sa première frame à la position 0.
/// L'ancre vaut donc 1 pour que cette première frame porte le LSN 1 — la valeur
/// que le format avait déjà (l'ancien amorçage était `last_wal_offset + 1` avec
/// `last_wal_offset = 0`). Garder 1 évite aussi que le LSN 0, qui sert de
/// sentinelle « non assigné » chez tous les appelants (`lsn: 0, // assigned by
/// append`), devienne une valeur réelle sur disque.
pub const ANCRE_INITIALE: u64 = 1;

// ---------------------------------------------------------------------------
// Side-car d'ancre de LSN
// ---------------------------------------------------------------------------

/// Ancre durable lue dans le side-car `<wal>.lsn`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AncreDurable {
    /// Base du segment courant : `lsn = ancre + position`.
    pub ancre: u64,
    /// Plancher du segment : taille du WAL au moment où l'ancre a été scellée.
    /// Un fichier PLUS COURT que ce plancher a été tronqué hors de notre dos.
    pub plancher: u64,
}

/// Chemin du side-car d'ancre pour un WAL donné.
#[must_use]
pub fn chemin_ancre(wal: &Path) -> PathBuf {
    let mut s = wal.as_os_str().to_os_string();
    s.push(SUFFIXE_ANCRE_LSN);
    PathBuf::from(s)
}

fn crc_ancre(octets: &[u8]) -> u32 {
    let mut h = crc32fast::Hasher::new();
    h.update(octets);
    h.finalize()
}

/// Lit l'ancre durable d'un WAL. `None` = ABSENTE, illisible, d'une autre
/// version, ou CRC faux — dans tous ces cas l'appelant doit se comporter comme
/// si aucune ancre n'avait jamais été publiée (et non comme si elle valait zéro :
/// zéro est une ancre valide et parfaitement ambiguë).
#[must_use]
pub fn lire_ancre_durable(wal: &Path) -> Option<AncreDurable> {
    lire_ancre_sur(&chemin_ancre(wal))
}

/// [`lire_ancre_durable`] sur un chemin de side-car explicite.
#[must_use]
pub fn lire_ancre_sur(chemin: &Path) -> Option<AncreDurable> {
    let octets = crate::es::espace().lire_tout(chemin).ok()?;
    if octets.len() != TAILLE_ANCRE || octets[0..4] != MAGIE_ANCRE {
        return None;
    }
    if u16::from_le_bytes([octets[4], octets[5]]) != VERSION_ANCRE {
        return None;
    }
    let crc_attendu = u32::from_le_bytes([octets[24], octets[25], octets[26], octets[27]]);
    if crc_ancre(&octets[0..24]) != crc_attendu {
        return None;
    }
    Some(AncreDurable {
        ancre: u64::from_le_bytes(octets[8..16].try_into().ok()?),
        plancher: u64::from_le_bytes(octets[16..24].try_into().ok()?),
    })
}

/// Publie l'ancre durable : écriture ATOMIQUE (tmp + rename + fsync du
/// répertoire), comme l'en-tête meta du checkpoint.
///
/// ── POURQUOI ATOMIQUE, ALORS QUE LE FICHIER FAIT 32 OCTETS ────────────────
/// Un `write` de 32 octets à l'offset 0 n'est pas atomique vis-à-vis d'un crash :
/// une coupure au milieu laisse 16 octets neufs et 16 octets anciens. Le rename,
/// lui, est atomique sur POSIX : l'ancre est soit l'ancienne, soit la nouvelle,
/// jamais un mélange — ce qui compte, parce qu'une ancre MÉLANGÉE serait
/// silencieusement plausible.
fn publier_ancre_sur(chemin: &Path, ancre: u64, plancher: u64) -> SubstrateResult<()> {
    let mut buf = [0u8; TAILLE_ANCRE];
    buf[0..4].copy_from_slice(&MAGIE_ANCRE);
    buf[4..6].copy_from_slice(&VERSION_ANCRE.to_le_bytes());
    buf[8..16].copy_from_slice(&ancre.to_le_bytes());
    buf[16..24].copy_from_slice(&plancher.to_le_bytes());
    let crc = crc_ancre(&buf[0..24]);
    buf[24..28].copy_from_slice(&crc.to_le_bytes());

    let es = crate::es::espace();
    let tmp = chemin_tmp_ancre(chemin);
    if let Err(e) = es.ecrire_synchronise(&tmp, &buf) {
        let _ = es.supprimer(&tmp);
        return Err(e.into());
    }
    if let Err(e) = es.renommer(&tmp, chemin) {
        // ── LA COURSE À LA PUBLICATION N'A PAS DE PERDANT ────────────────────
        // MESURÉ : six processus ouvrant un WAL VIDE en même temps publient tous
        // la même ancre. Avec un nom de temporaire PARTAGÉ (`<ancre>.tmp`), le
        // second `rename` échouait en ENOENT — son temporaire avait déjà été
        // emporté par le premier — et l'OUVERTURE ÉCHOUAIT. Le nom est donc
        // unique par publication ; et si le rename échoue quand même, on relit :
        // une ancre LISIBLE vaut publication faite.
        let _ = es.supprimer(&tmp);
        if lire_ancre_sur(chemin).is_none() {
            return Err(e.into());
        }
    }
    // Fsync du RÉPERTOIRE : sans lui, le rename peut survivre au crash sans que
    // l'entrée qu'il crée soit elle-même durable.
    let dir = chemin.parent().filter(|p| !p.as_os_str().is_empty());
    let dir = dir.unwrap_or_else(|| Path::new("."));
    let _ = es.synchroniser_repertoire(dir);
    Ok(())
}

/// Nom du temporaire d'ancre : UNIQUE par publication (processus + compteur).
///
/// Un nom partagé (`<ancre>.tmp`) fait échouer le `rename` du second publieur en
/// ENOENT — le premier a déjà emporté le fichier. Un temporaire unique par appel
/// ne peut être emporté que par nous-mêmes.
/// Pendant de [`chemin_tmp_ancre`] pour les AUTRES publications atomiques
/// (dictionnaire, propriétés) : tout fichier écrit « tmp + rename » par plusieurs
/// processus doit porter un temporaire UNIQUE PAR APPEL, sans quoi le `rename` du
/// perdant échoue en `ENOENT` sur le fichier du gagnant.
pub fn chemin_tmp_unique(chemin: &Path) -> PathBuf {
    chemin_tmp_ancre(chemin)
}

/// Identifiant de processus PORTABLE.
///
/// `std::process::id()` PANIQUE sur wasm32 (« no pids on this platform »). Il n'y a
/// pas de multi-processus là-bas : zéro suffit pour nommer un fichier temporaire ou
/// signer un message. En natif, c'est exactement `std::process::id()`.
#[inline]
pub(crate) fn pid_portable() -> u32 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::process::id()
    }
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
}

fn chemin_tmp_ancre(chemin: &Path) -> PathBuf {
    static COMPTEUR_TMP: AtomicU64 = AtomicU64::new(0);
    let n = COMPTEUR_TMP.fetch_add(1, Ordering::Relaxed);
    let mut s = chemin.as_os_str().to_os_string();
    // `std::process::id()` PANIQUE sur wasm32 (« no pids on this platform ») : il
    // n'y a pas de multi-processus là-bas, un zéro suffit.
    let pid = u64::from(pid_portable());
    s.push(format!(".tmp.{pid}.{n}"));
    PathBuf::from(s)
}

// ---------------------------------------------------------------------------
// Élection du checkpointeur (Phase B, pièce 1)
// ---------------------------------------------------------------------------

/// Suffixe du FENCE d'élection du checkpointeur : `substrate.wal` →
/// `substrate.wal.chk`.
pub const SUFFIXE_FENCE_CHECKPOINT: &str = ".chk";

/// Chemin du fence d'élection pour un WAL donné.
#[must_use]
pub fn chemin_fence_checkpoint(wal: &Path) -> PathBuf {
    let mut s = wal.as_os_str().to_os_string();
    s.push(SUFFIXE_FENCE_CHECKPOINT);
    PathBuf::from(s)
}

/// ── QUI A LE DROIT DE CHECKPOINTER ? (élection SANS coordination lourde) ────
///
/// ── POURQUOI UNE ÉLECTION, ET PAS SEULEMENT UNE ANCRE ATOMIQUE ──────────────
/// La publication de l'ancre est atomique (`publier_ancre_sur`, tmp+rename+
/// fsync), mais un checkpoint n'est PAS une seule opération atomique : c'est
/// [avancer l'ancre] PUIS [tronquer]. Entre les deux, le fichier et l'ancre sont
/// désaccordés — c'est précisément la fenêtre pendant laquelle une troncature
/// concurrente ferait recouvrir les LSN du nouveau segment sur ceux du précédent.
/// Deux processus qui checkpointent en même temps avancent donc chacun l'ancre de
/// SA mesure, et la seconde avance est calculée sur une taille qui n'existe plus.
///
/// ── POURQUOI UN `flock` SUR UN FICHIER DÉDIÉ, ET PAS « LE PLUS ANCIEN OUVERT » ──
/// « Le plus ancien ouvert » demande de SAVOIR qui est ouvert : il faut soit
/// sonder `/proc` (non portable, et un processus peut être ouvert sans y être
/// lisible), soit tenir des cœurs de battement et décider de leur péremption —
/// c'est-à-dire de la coordination lourde, avec une fenêtre de désaccord où DEUX
/// processus se croient élus. Le noyau, lui, arbitre déjà parfaitement :
/// `flock(LOCK_EX)` sur ce fence est pris par EXACTEMENT un détenteur, et il est
/// rendu par le noyau à la mort du processus (y compris un `kill -9`) — aucune
/// élection à reprendre, aucun fichier de verrou résiduel à nettoyer.
///
/// ── POURQUOI CE N'EST PAS UN VERROU CHAUD ───────────────────────────────────
/// Le fence n'est pris QUE pendant un checkpoint (chemin froid : un par 8 Mio de
/// WAL) et par le checkpointeur SEUL. Les appends n'y touchent JAMAIS. Le seul
/// autre lecteur du fence est un pair qui veut checkpoint en même temps : il
/// échoue en `WouldBlock` et passe son chemin — c'est l'élection.
///
/// ── LE RÔLE EXPLICITE ──────────────────────────────────────────────────────
/// [`ElectionCheckpoint::ceder`] rend le fence sans rien faire : un processus qui
/// sait qu'il ne doit PAS checkpointer (rôle explicite d'un opérateur, un
/// écrivain de fond qui ne veut pas payer les msync) renonce avant toute E/S.
#[derive(Debug)]
pub struct ElectionCheckpoint {
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fichier: Option<File>,
    chemin: PathBuf,
}

impl ElectionCheckpoint {
    /// Tente de devenir LE checkpointeur. `Ok(None)` = un pair le détient :
    /// l'appelant doit RENONCER (et non attendre — un checkpoint concurrent
    /// rendrait le nôtre redondant, pas nécessaire).
    ///
    /// Un échec de `open` (répertoire en lecture seule, chemin sur un média
    /// exotique) rend `Ok(None)` plutôt qu'une erreur : ne pas pouvoir
    /// checkpoint n'est pas une panne du substrat, et faire échouer l'appel
    /// transformerait une gêne en panne sur le chemin de `flush`.
    #[must_use]
    pub fn tenter(wal: &Path) -> Option<Self> {
        let chemin = chemin_fence_checkpoint(wal);
        #[cfg(target_arch = "wasm32")]
        {
            // Aucun multi-processus : l'élection est toujours gagnée, et il n'y a
            // pas de `flock` à prendre (`fs2` n'y compile pas).
            let _ = &chemin;
            Some(Self {
                fichier: None,
                chemin,
            })
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            use fs2::FileExt as _;
            let fichier = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&chemin)
                .ok()?;
            match fichier.try_lock_exclusive() {
                Ok(()) => Some(Self {
                    fichier: Some(fichier),
                    chemin,
                }),
                // `WouldBlock` = un pair checkpointe. Toute autre erreur (système
                // de fichiers sans `flock`) est traitée pareil : on renonce.
                Err(_) => None,
            }
        }
    }

    /// Renonce au fence (rôle explicite « je ne checkpointe pas »).
    /// Le `Drop` rend le verrou de toute façon ; cette porte existe pour que
    /// l'intention soit LISIBLE dans le code appelant.
    #[must_use]
    pub fn ceder(self) -> Option<Self> {
        None
    }

    /// Chemin du fence détenu.
    #[must_use]
    pub fn chemin(&self) -> &Path {
        &self.chemin
    }

    /// Sommes-nous élu ? (toujours vrai pour un objet construit par
    /// [`ElectionCheckpoint::tenter`] et rendu `Some` — la question se pose
    /// après [`ElectionCheckpoint::ceder`].)
    #[must_use]
    pub fn elu(&self) -> bool {
        true
    }
}

impl Drop for ElectionCheckpoint {
    fn drop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(f) = self.fichier.take() {
            // Rend le verrou explicitement : le noyau le rendrait à la fermeture
            // du descripteur, mais un `Drop` qui dit ce qu'il fait se relit.
            // (`File::unlock` de la bibliothèque standard fait le même
            // `flock(LOCK_UN)` que celui qui a été pris par `fs2`.)
            let _ = f.unlock();
        }
    }
}

/// ── LES TROIS CAS DE RÉCONCILIATION D'OUVERTURE ─────────────────────────────
///
/// Le fichier WAL et son ancre sont DEUX objets : une panne peut les désunir. Au
/// lieu de deviner, `open` NOMME le cas qu'il a trouvé — et chaque cas a une
/// conséquence différente, sur le LSN comme sur l'offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reconciliation {
    /// **(1) Ancre adoptée** — le side-car décrit ce fichier (et le fichier
    /// n'est pas plus court que son plancher). C'est le cas nominal : rien à
    /// faire, aucune écriture, aucun coût. Il couvre aussi le WAL VIDE sans
    /// side-car (base neuve, ancre initiale), qui n'a aucune histoire à
    /// concilier.
    AncreAdoptee { ancre: u64 },
    /// **(2) Ré-ancrage en avant** — le fichier contient déjà des frames, mais
    /// AUCUNE ancre ne les décrit (side-car absent : base écrite par une version
    /// antérieure, fichier restauré seul, side-car perdu). On ne peut pas savoir
    /// quels LSN sont pris, donc on RÉ-ANCRE EN AVANT, au-delà du plus grand LSN
    /// présent. Sans cela, le premier append repartirait d'un LSN déjà écrit, et
    /// deux frames distinctes porteraient le même LSN.
    ReancrageEnAvant {
        ancre_avant: u64,
        ancre_apres: u64,
        lsn_max_fichier: u64,
    },
    /// **(3) Troncature étrangère** — le side-car décrit un segment plus LONG
    /// que le fichier : quelqu'un a tronqué le WAL hors de notre dos (ou nous
    /// avons rouvert après une troncature dont l'ancre n'a gardé trace). On garde
    /// l'ancre et on republie le plancher à la taille RÉELLE : écrire à partir
    /// d'un offset imaginaire est exactement ce qui produit des trous.
    TroncatureEtrangere {
        ancre: u64,
        plancher_attendu: u64,
        taille_reelle: u64,
    },
}

/// Plus grand LSN présent dans le fichier (0 si le fichier est vide ou si la
/// queue est déchirée avant toute frame).
///
/// ── POURQUOI UNE LECTURE COMPLÈTE, ET SEULEMENT DANS LE CAS (2) ────────────
/// C'est le SEUL cas où l'information n'existe nulle part ailleurs : aucune
/// ancre ne décrit ces frames, et deviner leur LSN serait pire que les mesurer.
/// Le coût (une passe sur le WAL) est payé UNE fois, à l'ouverture d'une base
/// écrite par une version antérieure, et jamais ensuite.
fn lsn_max_du_fichier(wal: &Path) -> u64 {
    match WalReader::open(wal) {
        Ok(r) => r
            .iter_from(0)
            .filter_map(Result::ok)
            .map(|(rec, _, _)| rec.lsn)
            .max()
            .unwrap_or(0),
        Err(_) => 0,
    }
}

/// Décide l'ancre d'ouverture. Rend `(ancre, cas, faut-il publier le side-car)`.
fn reconcilier_ouverture(
    ancre_chemin: &Path,
    wal: &Path,
    taille: u64,
    ancre_repli: u64,
) -> (u64, Reconciliation, bool) {
    match lire_ancre_sur(ancre_chemin) {
        Some(a) if taille >= a.plancher => (
            a.ancre,
            Reconciliation::AncreAdoptee { ancre: a.ancre },
            false,
        ),
        Some(a) => (
            a.ancre,
            Reconciliation::TroncatureEtrangere {
                ancre: a.ancre,
                plancher_attendu: a.plancher,
                taille_reelle: taille,
            },
            true,
        ),
        None if taille == 0 => (
            ancre_repli,
            Reconciliation::AncreAdoptee { ancre: ancre_repli },
            true,
        ),
        None => {
            let lsn_max = lsn_max_du_fichier(wal);
            let apres = core::cmp::max(lsn_max.saturating_add(1), ancre_repli);
            (
                apres,
                Reconciliation::ReancrageEnAvant {
                    ancre_avant: ancre_repli,
                    ancre_apres: apres,
                    lsn_max_fichier: lsn_max,
                },
                true,
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

/// Append-only writer over a WAL file.
///
/// Construction opens (or creates) the file at `path`, se cale sur sa taille
/// réelle et réconcilie son ancre de LSN avec le side-car `<path>.lsn` (voir
/// [`Reconciliation`]). Every call to [`WalWriter::append`] writes a complete
/// framed record and updates the internal offset + anchor.
///
/// The writer owns its own `Mutex<WriterInner>` so it is `Send + Sync`; multiple
/// producers can share a `&WalWriter` safely.
#[derive(Debug)]
pub struct WalWriter {
    path: PathBuf,
    /// Chemin du side-car d'ancre (normalement `<wal>.lsn`).
    ancre_path: PathBuf,
    inner: Mutex<WriterInner>,
    /// Byte offset of the next record to be written (monotonic).
    offset: AtomicU64,
    /// Base du segment courant : `lsn = ancre + position`.
    ancre: AtomicU64,
    sync_mode: SyncMode,
    /// Nombre de fois où le fichier s'est révélé PLUS LONG que notre compteur
    /// (un autre écrivain a ajouté entre deux appends) — recalage normal.
    recalages: AtomicU64,
    /// Nombre de fois où le fichier avait GRANDI entre notre lecture de la fin et
    /// la vérification qui suit notre écriture : le LSN estampillé sur cette frame
    /// n'est alors peut-être pas sa position.
    ///
    /// ── CE QUE CE COMPTEUR EST, EXACTEMENT (et ce qu'il n'est pas) ──────────
    /// C'est une **borne supérieure** du nombre de LSN non exacts, et pas le
    /// nombre lui-même : le fichier peut avoir grandi APRÈS notre écriture (un
    /// autre écrivain est passé ensuite, ce qui ne change RIEN à notre position)
    /// ou ENTRE notre lecture et notre écriture (là, notre LSN est une borne
    /// inférieure). La taille seule ne permet pas de trancher ; le test
    /// inter-processus, lui, tranche par RELECTURE (`lsn == ancre + position`),
    /// frame par frame. Ce compteur dit s'il y a lieu de regarder.
    courses_suspectees: AtomicU64,
    /// Nombre d'enregistrements APPENDÉS par cet écrivain depuis son ouverture.
    ///
    /// ── POURQUOI CE COMPTEUR EXISTE (2026-10-06) ─────────────────────────────
    /// `admin::WalStatus::record_count` le lisait sur `peek_lsn()` — « un
    /// incrément par enregistrement appendé, le seul compteur monotone que le
    /// substrat tient ». Ce n'est plus vrai : `peek_lsn` vaut `ancre + offset`,
    /// donc ≈ la TAILLE EN OCTETS du WAL, et `obrain wal status` aurait annoncé
    /// « Records: 1048576 » pour 500 enregistrements. Un compteur explicite (un
    /// `fetch_add` détendu par record, ~5 ns pour un write qui en coûte ~1000)
    /// rend le champ vrai sans rien coûter de mesurable.
    records_ecrits: AtomicU64,
    /// ── PHASE B, PIÈCE 1 : COMBIEN DE FOIS UNE TRONCATURE NOUS A EMPORTÉS ───
    /// Nombre de frames ÉCRITES PUIS DÉTRUITES par la troncature d'un autre
    /// processus avant d'avoir pu être relues (le checkpoint élu avance l'ancre
    /// puis tronque : une frame posée dans cette fenêtre disparaît). Chaque
    /// occurrence est REPRISE (ancre relue, position recalée, réécriture) — ce
    /// compteur dit à quelle fréquence le chemin de reprise sert réellement, ce
    /// qu'aucun test ne peut mesurer en production.
    troncatures_subies: AtomicU64,
    /// Nombre total de reprises d'append réussies après une troncature
    /// concurrente (voir [`WalWriter::troncatures_subies`]).
    reprises_apres_troncature: AtomicU64,
    // ── GROUP COMMIT À NEEDLE DE DURABILITÉ (2026-10-06) ────────────────────
    /// Le group commit est-il armé ? Lu SANS verrou pour choisir le chemin
    /// d'append (et donc pour savoir s'il faut encoder tout de suite).
    group_actif: AtomicBool,
    /// Miroir SANS VERROU de `groupe.octets` : c'est lui qui permet d'estampiller
    /// un LSN (prédiction `offset + octets_en_tampon`) hors du mutex, comme le
    /// chemin chaud le fait déjà avec `offset`. Une valeur périmée n'est jamais
    /// une corruption : elle déclenche le ré-estampillage sous verrou.
    octets_en_tampon: AtomicU64,
    /// Nombre de `fsync` RÉELLEMENT exécutés par cet écrivain (tous chemins :
    /// needle, exigence `FLAG_SYNC_ATTENDU`, `fsync()` explicite, Drop).
    ///
    /// C'est la mesure qui prouve que le `fsync` est indexé sur la NEEDLE et non
    /// sur le lot : `tests/wal_group_commit_needle.rs` compare ce compteur au
    /// nombre d'`append` (lots de 1) et au nombre de seuils franchis.
    fsyncs: AtomicU64,
    /// Nombre de `write` réellement posés dans le média (un par flush de needle).
    poses: AtomicU64,
    /// État et condvar de la veille de temps, dans un `Arc` SÉPARÉ (voir
    /// [`VeillePartagee`] : c'est ce qui permet au veilleur d'attendre sans
    /// retenir l'écrivain, donc au Drop de s'exécuter).
    veille: Arc<VeillePartagee>,
    // ── REGISTRE D'APPEND MMAPÉ PARTAGÉ (pièce 7, 2026-10-06) ───────────────
    /// Borne d'une frame (voir [`MAX_FRAME_BYTES`]), lisible SANS le mutex :
    /// l'encodage se fait hors verrou, il lui faut donc la borne sans le prendre.
    ///
    /// ── POURQUOI ELLE EST ICI ET PLUS DANS `WriterInner` ────────────────────
    /// Elle y était, et chaque append la lisait par `self.inner.lock()` PUIS
    /// reprenait le même verrou pour écrire : **deux paires de verrous par
    /// record** pour lire un entier que personne ne modifie jamais en production.
    /// Une copie atomique sur l'écrivain supprime la première paire, sans changer
    /// une seule garantie (le seam [`WalWriter::forcer_borne_frame`] publie les
    /// deux vues sous le verrou, comme avant).
    borne_frame_atomique: AtomicU64,
    /// Registre d'append partagé (`<wal>.reg`), créé PARESSEUSEMENT.
    ///
    /// ── POURQUOI PARESSEUSEMENT ─────────────────────────────────────────────
    /// Un store mono-écrivain prouvé (`Aucune`) n'en a aucun besoin : le créer à
    /// l'ouverture ferait apparaître un fichier dans TOUTE base existante, y
    /// compris celles qui n'auront jamais deux écrivains. Il n'est donc créé
    /// qu'au premier append en coordination PARTAGÉE, et jamais pour un WAL en
    /// mémoire. Quand il est indisponible (`/dev/full`, quota disque, répertoire
    /// en lecture seule, wasm), l'append reprend le chemin d'AVANT la pièce 7 —
    /// deux `statx` — au lieu d'échouer : le registre est une optimisation de
    /// COÛT, la correction vient de `O_APPEND` et du noyau.
    registre: OnceLock<Option<RegistreAppend>>,
    /// Nombre de tentatives de création du registre (voir
    /// [`MAX_TENTATIVES_REGISTRE`]).
    tentatives_registre: AtomicU64,
    /// ── LA VALEUR DE CHAÎNE QUE CET ÉCRIVAIN SAIT ÊTRE LA FIN RÉELLE ────────
    /// Sa propre publication (après un `write` complet), ou une valeur qu'il a
    /// VÉRIFIÉE par une mesure. C'est ce mot qui répond, avec deux lectures
    /// d'atomes, à la question « puis-je écrire sans rien demander au noyau ? » :
    /// la chaîne est prête quand `tete_reservee == fin_publiee == reg_fin_connue`
    /// — aucune réservation en attente, et une fin publiée dont on sait qu'elle est
    /// la taille du fichier. Sinon, on attend en mémoire partagée (aucun appel
    /// système non plus), ou on mesure (chemins d'anomalie).
    ///
    /// `u64::MAX` est la sentinelle « je ne sais plus » (voir
    /// [`WalWriter::oublier_la_tete`]) : elle ne peut égaler aucune taille de
    /// fichier, donc elle force le chemin prudent.
    reg_fin_connue: AtomicU64,
    /// Génération du registre que cet écrivain a réconciliée. Un changement
    /// signifie « un checkpoint a tronqué et publié une ancre neuve » : c'est
    /// l'ANNONCE qui dispense d'attendre pour se ré-ancrer (un pair qui a tronqué
    /// ne va pas finir d'écrire une frame que le fichier ne contient plus).
    reg_generation_vue: AtomicU64,
    /// Reprises du `compare_exchange` de réservation : un pair a réservé entre
    /// notre lecture de la tête et notre CAS. C'est de la CONTENTION, pas un coût :
    /// aucune de ces reprises ne paie d'appel système (l'ancien chemin payait, lui,
    /// deux `statx` par record).
    reprises_de_reservation: AtomicU64,
    /// Combien de fois un append a dû ATTENDRE que son prédécesseur publie sa
    /// plage (spin sur `fin_publiee`, **zéro appel système**). C'est la mesure qui
    /// remplace l'ancien « deux `statx` par record » : le mono-écrivain partagé
    /// reste à 0, le WAL réellement concurrent attend — en mémoire partagée.
    attentes_de_publication: AtomicU64,
    /// Têtes de chaîne RÉ-ANCRÉES sur la taille réelle : un écrivain mort entre
    /// sa réservation et son `write` (ou un pair écrivant hors chaîne) a laissé la
    /// tête en avance sur le fichier. C'est le compteur de production du chemin
    /// froid — sa croissance dit qu'un store récupère des écrivains tués.
    tetes_reancrees: AtomicU64,
    /// Troncatures étrangères vues par le registre (`generation` changée).
    /// L'équivalent partagé de [`WalWriter::troncatures_subies`], mais connu SANS
    /// `statx` : c'est ce qui rend la ré-ancrage immédiat au lieu d'attendu.
    generations_vues: AtomicU64,
    /// Délai avant de déclarer une réservation abandonnée, en nanosecondes.
    /// Champ (et non constante) pour que les tests puissent le réduire : le défaut
    /// ([`DELAI_TETE_BLOQUEE`], 25 ms) est choisi pour l'EXACTITUDE en production,
    /// et le prouver sans attendre 25 ms exige un seam.
    delai_tete_bloquee_ns: AtomicU64,
    /// Miroir ATOMIQUE de `groupe.seuil_temps` : permet au veilleur de recalculer
    /// une échéance sans prendre le verrou d'écriture (donc sans inverser l'ordre
    /// des verrous avec `armer_la_veille`). Tenu à jour par `forcer_seuil_temps`.
    seuil_temps_ns: AtomicU64,
    /// Télémétrie du group commit, inerte sans `OBRAIN_GC_DIAG=1` (2026-10-06) :
    /// c'est l'instrument qui a localisé la régression ×0,42-0,60, et il reste
    /// branché pour qu'une régression future soit MESURÉE au lieu d'être devinée.
    diag: DiagGroupe,
}

/// ── COMBIEN DE FOIS ON REPREND UN APPEND EMPORTÉ PAR UNE TRONCATURE ─────────
///
/// Une reprise est une E/S complète (relire l'ancre sur le disque, recaler,
/// réécrire) : elle coûte des milliers de fois un append nominal. La borner
/// n'est pas une optimisation, c'est une garantie de PROGRÈS : sur un store où
/// un pair checkpoint à chaque append (pathologique, mais atteignable en
/// réglant `OBRAIN_WAL_CHECKPOINT_BYTES` très bas), une boucle non bornée ne
/// terminerait jamais et l'appelant croirait à un blocage. Huit reprises
/// couvrent le cas réel (une troncature par fenêtre de 8 Mio) avec une marge
/// que seule une configuration aberrante peut épuiser — et cet épuisement est
/// NOMMÉ ([`crate::SubstrateError::WalTroncatureConcurrente`]), jamais silencieux.
const MAX_REPRISES_TRONCATURE: u32 = 8;

#[derive(Debug)]
struct WriterInner {
    medium: WalMedium,
    /// Dernier cas de réconciliation observé (ouverture, ou troncature étrangère
    /// détectée par la garde d'append).
    reconciliation: Option<Reconciliation>,
    /// Queue déchirée dont la réparation a échoué : TOUT append est refusé tant
    /// qu'elle n'est pas réparée. Écrire à la suite d'une déchirure produirait
    /// des frames qu'aucun lecteur ne verrait (le rejeu s'arrête dessus).
    queue_a_repairer: Option<String>,
    /// Faut-il vérifier, APRÈS écriture, que personne ne s'est glissé entre la
    /// lecture de la fin et le `write` ? Allumé au premier signe d'un second
    /// écrivain (le mono-écrivain ne paie donc JAMAIS ce fstat supplémentaire).
    surveiller_les_courses: bool,
    /// Politique de coordination de l'append. Voir [`CoordinationAppend`].
    coordination: CoordinationAppend,
    /// ── SEAM DE TEST : PAUSE ENTRE LE `write` ET SON OBSERVATION ───────────
    ///
    /// La fenêtre que la pièce 1 doit rendre récupérable — « une frame écrite,
    /// puis emportée par la troncature d'un pair » — dure quelques centaines de
    /// nanosecondes : aucun test ne peut l'ouvrir par la seule chance, et un test
    /// qui dépendrait d'une course serait un test qui échoue une fois sur vingt.
    /// Cette pause force l'entrelacement : elle ouvre la fenêtre POUR DE BON, de
    /// sorte que le test prouve le chemin (détection `apres < fin`, relecture de
    /// l'ancre, réécriture) au lieu de l'espérer.
    ///
    /// `None` partout ailleurs : le chemin chaud ne paie pas même une lecture.
    pause_apres_ecriture: Option<std::time::Duration>,
    /// ── SEAM DE TEST : PAUSE ENTRE LA RÉSERVATION ET LE `write` (pièce 7) ───
    ///
    /// C'est la fenêtre que la chaîne de réservation doit rendre RÉCUPÉRABLE :
    /// un écrivain qui a réservé sa plage et MEURT avant d'écrire laisse la tête
    /// bloquée au-delà de la taille réelle. Aucun test ne peut ouvrir cette
    /// fenêtre par chance (elle dure quelques dizaines de nanosecondes) : cette
    /// pause la rend DÉTERMINISTE, de sorte que le test prouve le chemin froid
    /// (attente bornée, ré-ancrage, aucun trou de zéros) au lieu de l'espérer.
    pause_apres_reservation: Option<std::time::Duration>,
    /// Le tampon du group commit à needle de durabilité (voir [`GroupCommit`]).
    groupe: GroupCommit,
}

/// Où le WAL écrit : un fichier (mode natif) ou un tampon en mémoire.
///
/// POURQUOI. Le mode « en mémoire » du substrat existait déjà comme une base
/// FICHIER dans un répertoire temporaire ; sur wasm32 il n'y a pas de système de
/// fichiers, donc le WAL ne peut pas être un fichier. Il n'a d'ailleurs aucune
/// raison de l'être : un WAL sert au rejeu APRÈS RÉOUVERTURE, et un store en
/// mémoire ne se rouvre jamais.
#[derive(Debug)]
enum WalMedium {
    File(Box<dyn crate::es::FichierJournal>),
    /// Les octets du WAL. L'écriture se fait toujours à la FIN (le fichier natif
    /// est ouvert en ajout, sans `seek`), donc `extend_from_slice` suffit.
    Memoire(Vec<u8>),
}

impl WriterInner {
    /// UN SEUL appel système d'écriture, et son compte RÉEL.
    ///
    /// ── POURQUOI PAS `write_all` (c'est LE correctif de déchirure) ──────────
    /// `write_all` BOUCLE : si le noyau n'accepte que 100 octets sur 400 (quota
    /// atteint), il rappelle `write` pour les 300 restants — et si l'appel
    /// suivant échoue, la frame reste à moitié posée. Pire : le noyau peut
    /// insérer les octets d'un AUTRE écrivain entre les deux appels, et la frame
    /// devient HYBRIDE (en-tête d'un record, corps d'un autre). Un seul `write`
    /// sous `O_APPEND` est atomique ; au-delà, on veut savoir qu'on a échoué,
    /// avec le compte exact, pour pouvoir ANNULER.
    fn ecrire_atomique(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match &mut self.medium {
            WalMedium::File(f) => f.ajouter(buf),
            WalMedium::Memoire(v) => {
                v.extend_from_slice(buf);
                Ok(buf.len())
            }
        }
    }

    /// Taille RÉELLE du média (le noyau est l'arbitre, pas notre compteur).
    fn taille_reelle(&mut self) -> std::io::Result<u64> {
        match &mut self.medium {
            WalMedium::File(f) => f.taille(),
            WalMedium::Memoire(v) => Ok(v.len() as u64),
        }
    }

    /// Annule tout ce qui a été écrit au-delà de `position` : c'est la RÉPARATION
    /// d'une écriture courte (la frame partielle disparaît, la queue redevient un
    /// bord de frame).
    ///
    /// ── ELLE N'ALLONGE JAMAIS (Phase B, pièce 1 — LE TROU DE ZÉROS) ─────────
    /// `set_len` fait DEUX choses selon la cible : il tronque si la cible est plus
    /// petite, et il **ALLONGE EN ZÉRO-REMPLISSANT** si elle est plus grande. Ce
    /// second comportement était le chemin exact du « trou de zéros » de la Phase
    /// B : un checkpoint d'un autre processus tronque le WAL pendant notre
    /// écriture ; notre `write` est court (quota, disque plein) ; `position` est
    /// l'offset d'AVANT la troncature, donc SUPÉRIEUR à la taille courante ; on
    /// appelait `set_len(position)` et le noyau remplissait [courante, position)
    /// de zéros. Le WAL repartait alors avec une plage de zéros SUIVIE DE DONNÉES
    /// — le `WalTrou` nommé de la pièce 6 — et le curseur repartait au-delà de
    /// l'EOF.
    ///
    /// La règle est donc : on ne retire QUE ce qui existe. Si la cible dépasse la
    /// taille courante, il n'y a rien à annuler au-delà d'elle — l'appelant, lui,
    /// recalera son curseur sur la taille RÉELLE (voir `poser_sous_verrou`).
    fn annuler_jusqua(&mut self, position: u64) -> std::io::Result<()> {
        match &mut self.medium {
            // `set_len` (ftruncate) ignore `O_APPEND` : on peut raccourcir.
            WalMedium::File(f) => {
                let courante = f.taille()?;
                if position >= courante {
                    // JAMAIS d'allongement : c'est ce chemin qui fabriquait le trou.
                    return Ok(());
                }
                f.tronquer(position)
            }
            WalMedium::Memoire(v) => {
                if position as usize >= v.len() {
                    return Ok(());
                }
                v.truncate(position as usize);
                Ok(())
            }
        }
    }

    /// ── LE VERROU D'APPEND INTER-PROCESSUS A ÉTÉ SUPPRIMÉ (pièce 7) ─────────
    ///
    /// Il vivait ici : `flock(LOCK_EX)` sur le descripteur du WAL, tenu le temps de
    /// la paire [lire la fin, écrire], pour que le LSN de `VerrouExact` soit
    /// exactement la position. Mesuré, il coûtait deux appels système par record
    /// et faisait BAISSER le débit cumulé quand N montait (×0,64 de N=1 à N=8) —
    /// plus cher que le `write` qu'il protégeait, et un réveil en troupeau des
    /// attentes par-dessus.
    ///
    /// La chaîne de réservation (`reserver_dans_la_chaine`) donne la MÊME garantie
    /// sans verrou chaud : l'offset n'est pas lu puis verrouillé, il est RÉSERVÉ
    /// atomiquement. Il n'y a donc plus rien à verrouiller ici — voir
    /// [`crate::wal_registre`].
    ///
    /// `tout = true` → `sync_all` (métadonnées + données), sinon `sync_data`.
    fn synchroniser(&mut self, tout: bool) -> std::io::Result<()> {
        match &mut self.medium {
            WalMedium::File(f) => f.synchroniser(tout),
            // En mémoire, il n'y a rien à pousser : les octets SONT l'état.
            WalMedium::Memoire(_) => Ok(()),
        }
    }

    /// ── NOS OCTETS SONT-ILS ENCORE DANS LE FICHIER ? (chemin d'anomalie) ────
    ///
    /// ── POURQUOI CETTE VÉRIFICATION EXISTE (défaut trouvé par le test T7) ────
    /// La détection d'une frame emportée est `taille_apres < fin` — c'est-à-dire
    /// « le fichier est plus court que ce que MON écriture aurait produit ». Or
    /// DEUX situations produisent cette inégalité, et une seule est une perte :
    ///
    /// 1. **la frame a été emportée** (un pair a tronqué APRÈS notre `write`) —
    ///    il faut la réécrire ;
    /// 2. **la frame a survécu, mais PLUS BAS que prévu** (un pair a tronqué
    ///    ENTRE notre lecture de la fin et notre `write` : `O_APPEND` nous a
    ///    alors posés au début du nouveau segment, à un offset très inférieur à
    ///    `position`). La frame est intacte, et la réécrire la DUPLIQUERAIT.
    ///
    /// MESURÉ (T7, 80 checkpoints concurrents sur ~11 000 appends) : le cas 2 se
    /// produit, et la version qui ne le distinguait pas du cas 1 produisait une
    /// frame en double. Le rejeu est idempotent, donc la donnée restait juste —
    /// mais un journal qui écrit deux fois le même enregistrement n'est plus le
    /// journal qu'on croit, et le compte de frames devient faux.
    ///
    /// La vérification est une RECHERCHE DE NOS PROPRES OCTETS dans la queue du
    /// fichier : le contenu d'une frame (en-tête + payload + CRC) n'est pas
    /// reproductible par un pair, donc le trouver EST la preuve qu'elle a
    /// survécu. Elle ne tourne QUE sur le chemin d'anomalie — jamais sur le
    /// chemin nominal, qui ne paie donc rien.
    fn nos_octets_sont_dans_la_queue(&mut self, buf: &[u8], taille: u64) -> std::io::Result<bool> {
        /// Marge au-delà de notre frame : ce qu'un pair a pu poser APRÈS nous
        /// dans la fenêtre de quelques microsecondes qui sépare notre `write` de
        /// cette vérification (le marqueur de checkpoint, dont l'écriture suit
        /// immédiatement la troncature, et les frames d'un pair). 64 Kio couvrent
        /// très largement ce qu'un écrivain peut poser en quelques microsecondes ;
        /// au-delà, on préfère RÉÉCRIRE (au pire une frame en double, inoffensive
        /// puisque le rejeu est idempotent) plutôt que de croire à une perte qui
        /// n'a pas eu lieu. Cette lecture n'a lieu QUE sur le chemin d'anomalie.
        const MARGE: u64 = 64 * 1024;

        if buf.is_empty() || taille < buf.len() as u64 {
            return Ok(false);
        }
        let fenetre = (buf.len() as u64 + MARGE).min(taille);
        let debut = taille - fenetre;
        let mut zone = vec![0u8; fenetre as usize];
        match &mut self.medium {
            WalMedium::File(f) => {
                #[cfg(unix)]
                {
                    f.lire_a(debut, &mut zone)?;
                }
                #[cfg(not(unix))]
                {
                    // Pas de `pread` sur cette cible : on NE réécrit pas. C'est
                    // le sens sûr — une frame intacte jamais dupliquée, au prix
                    // d'un LSN qui reste une prédiction (déjà documenté).
                    let _ = f;
                    return Ok(true);
                }
            }
            WalMedium::Memoire(v) => {
                zone.copy_from_slice(&v[debut as usize..]);
            }
        }
        Ok(zone.windows(buf.len()).any(|w| w == buf))
    }

    fn tronquer(&mut self) -> std::io::Result<()> {
        match &mut self.medium {
            WalMedium::File(f) => {
                f.tronquer(0)?;
                f.rembobiner()?;
                f.synchroniser(true)
            }
            WalMedium::Memoire(v) => {
                v.clear();
                Ok(())
            }
        }
    }
}

/// ── QUI COORDONNE LES APPENDS ENTRE PROCESSUS ? ─────────────────────────────
///
/// Les régimes sont MESURÉS (`tests/mw_interprocess_append.rs`, et le comptage de
/// syscalls de `tests/wal_append_syscalls.rs`) :
///
/// | régime | non-perte | LSN | syscalls/record | débit cumulé à 8 processus |
/// |---|---|---|---|---|
/// | [`Aucune`](Self::Aucune) | `O_APPEND` | exact (le compteur est autoritaire) | 1 (chemin chaud inchangé) | — |
/// | [`OAppend`](Self::OAppend) | `O_APPEND` + chaîne de réservation | **exact par construction** | **1** | **×5,1** |
/// | [`VerrouExact`](Self::VerrouExact) | idem `OAppend` (le `flock` a disparu) | **exact** | **1** | idem `OAppend` |
///
/// ── CE QUE LA MESURE DU 2026-10-06 A CHANGÉ ─────────────────────────────────
/// Avant la pièce 7, `OAppend` payait 3 syscalls (1 `write` + 2 `statx`) et son
/// LSN n'était qu'une BORNE INFÉRIEURE ; `VerrouExact` en payait 5 (+2 `flock`)
/// pour obtenir l'exactitude, au prix d'un débit qui BAISSE quand N monte (×0,64).
/// La réservation en chaîne rend l'exactitude GRATUITE et supprime le verrou
/// chaud : le LSN n'est plus prédit, il est RÉSERVÉ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CoordinationAppend {
    /// L'appelant a PROUVÉ qu'il est le seul écrivain (verrou de répertoire
    /// EXCLUSIF tenu) : aucune coordination n'est due, et l'append se réduit à
    /// [estamper, écrire]. C'est le chemin des stores ouverts par
    /// `SubstrateFile::open` — le chemin CHAUD, rendu INCHANGÉ par ce chantier
    /// (aucun registre n'est même créé pour lui).
    Aucune,
    /// **Défaut.** `O_APPEND` + **réservation en chaîne** dans le registre partagé
    /// `<wal>.reg` : la position d'écriture n'est plus MESURÉE (`statx`) mais
    /// RÉSERVÉE (`compare_exchange`), donc le LSN est `ancre + position` par
    /// construction, sans qu'aucun appel système ne soit payé pour le savoir — et
    /// `O_APPEND` continue de garantir qu'aucune écriture n'en recouvre une autre.
    /// C'est le régime qui SCALE (mesuré ×5,1 de N=1 à N=8).
    #[default]
    OAppend,
    /// `O_APPEND` + LSN EXACTEMENT égal à la position.
    ///
    /// ── CE N'EST PLUS UN VERROU, ET C'EST MESURÉ ────────────────────────────
    /// Ce régime prenait un `flock` sur le WAL autour de la paire [lire la fin,
    /// écrire] : deux appels système de plus par record, et un réveil en troupeau
    /// des attentes qui faisait BAISSER le débit cumulé quand N montait (mesuré :
    /// N=1 : 201 k/s → N=8 : 130 k/s, soit ×0,64). La réservation en chaîne
    /// (pièce 7) donne la MÊME garantie — l'exactitude — **sans verrou chaud et
    /// sans syscall** : ce variant est donc conservé pour ce qu'il DEMANDE (un
    /// LSN exact, vérifié par les tests), pas pour ce qu'il coûtait. Il est
    /// désormais indiscernable de [`OAppend`](Self::OAppend), qui l'offre aussi.
    VerrouExact,
}

impl WalWriter {
    /// Open or create the WAL file at `path`.
    ///
    /// `ancre_repli` n'est PLUS un « prochain LSN » : c'est l'ancre utilisée
    /// seulement quand le side-car `<path>.lsn` n'existe pas (base neuve, ou base
    /// écrite par une version antérieure à l'ancre durable). Dans le cas nominal
    /// — side-car présent — c'est LUI qui fait foi, et `ancre_repli` est ignoré.
    /// Le side-car vit à côté du WAL : `substrate.wal` → `substrate.wal.lsn`.
    pub fn open(
        path: impl AsRef<Path>,
        sync_mode: SyncMode,
        ancre_repli: u64,
    ) -> SubstrateResult<Self> {
        let path = path.as_ref().to_path_buf();
        // ── O_APPEND, POUR DE VRAI (2026-09-16) ─────────────────────────────
        // Le fichier était ouvert en `write` simple, alors que `append` affirmait
        // (« Writes to an append-opened file are always at end ») le contraire.
        // Sans O_APPEND, chaque processus écrit à SA position de curseur. Qu'un
        // autre processus tronque le WAL (checkpoint), et celui-ci continue
        // d'écrire à son ancienne position : le noyau comble l'écart par des
        // ZÉROS. Mesuré sur runtime-dev : 7 964 288 octets nuls après la première
        // trame, puis ~200 Ko d'écritures — que tout replay à froid ignorait,
        // arrêté au premier en-tête nul. Avec O_APPEND, le noyau place chaque
        // écriture, atomiquement, à la fin RÉELLE du fichier : un troncage par
        // autrui ne laisse plus aucun trou.
        //
        // ── RÉUNIFICATION (2026-10-06) ──────────────────────────────────────
        // Le correctif O_APPEND de la lignée servie et l'ouverture paramétrable du
        // multi-écrivain disent la MÊME chose (les deux ouvrent en `.append(true)`),
        // et l'explication ci-dessus reste la référence du POURQUOI. On garde donc
        // la structure du multi-écrivain — délégation à `open_avec_ancre`, seule
        // forme qui sache aussi viser un side-car de test (`/dev/full`).
        Self::open_avec_ancre(path.clone(), chemin_ancre(&path), sync_mode, ancre_repli)
    }

    /// [`WalWriter::open`] avec un chemin de side-car explicite — le seul moyen
    /// de tester l'écriture courte / le disque plein sur un média réel
    /// (`/dev/full`) tout en gardant le side-car dans un répertoire inscriptible.
    pub fn open_avec_ancre(
        path: impl AsRef<Path>,
        ancre_path: impl AsRef<Path>,
        sync_mode: SyncMode,
        ancre_repli: u64,
    ) -> SubstrateResult<Self> {
        let path = path.as_ref().to_path_buf();
        let ancre_path = ancre_path.as_ref().to_path_buf();
        // ── `append` ET NON `write` : C'EST L'ATOMICITÉ INTER-PROCESSUS ──────
        // Sans `O_APPEND`, chaque descripteur écrit à SA position courante :
        // deux processus ouverts sur un WAL vide écrivent tous deux à l'offset 0,
        // et le second écrase le premier frame par frame, avec des CRC tous
        // valides (74 % de perte silencieuse, MESURÉ — sonde
        // `mw_interprocess_probe::p1`). Avec `O_APPEND`, le noyau place chaque
        // écriture à la fin du fichier, atomiquement : aucune écriture ne peut en
        // recouvrir une autre.
        let file = crate::es::espace().ouvrir_journal(&path)?;
        let taille = file.taille()?;
        let (ancre, reconciliation, publier) =
            reconcilier_ouverture(&ancre_path, &path, taille, ancre_repli);
        // Le side-car n'est écrit QUE s'il n'est pas déjà juste : une ouverture
        // nominale ne touche donc pas au disque (hors le `open` lui-même), et
        // deux processus qui s'ouvrent en parallèle ne se battent pas pour le
        // réécrire.
        if publier {
            publier_ancre_sur(&ancre_path, ancre, taille)?;
        }
        Ok(Self {
            path,
            ancre_path,
            inner: Mutex::new(WriterInner {
                medium: WalMedium::File(file),
                reconciliation: Some(reconciliation),
                queue_a_repairer: None,
                surveiller_les_courses: false,
                coordination: CoordinationAppend::OAppend,
                pause_apres_ecriture: None,
                pause_apres_reservation: None,
                groupe: GroupCommit::neuf(),
            }),
            offset: AtomicU64::new(taille),
            ancre: AtomicU64::new(ancre),
            sync_mode,
            recalages: AtomicU64::new(0),
            courses_suspectees: AtomicU64::new(0),
            records_ecrits: AtomicU64::new(0),
            troncatures_subies: AtomicU64::new(0),
            reprises_apres_troncature: AtomicU64::new(0),
            // ── GROUP COMMIT : ÉTEINT PAR DÉFAUT, ET C'EST LA NON-RÉGRESSION ─
            // Tant que `activer_group_commit` n'est pas appelé, `append` suit le
            // chemin d'aujourd'hui bit-pour-bit (lot de 1, `fsync` par
            // `SyncMode`). Un store existant ne peut donc PAS changer de
            // comportement de durabilité sans le demander explicitement.
            group_actif: AtomicBool::new(false),
            octets_en_tampon: AtomicU64::new(0),
            fsyncs: AtomicU64::new(0),
            poses: AtomicU64::new(0),
            veille: Arc::new(VeillePartagee::neuve()),
            borne_frame_atomique: AtomicU64::new(MAX_FRAME_BYTES as u64),
            registre: OnceLock::new(),
            tentatives_registre: AtomicU64::new(0),
            // ── LA VUE DE DÉPART DE LA CHAÎNE EST LA TAILLE RÉELLE MESURÉE ───
            // À l'ouverture, `taille` est une mesure (`file.metadata()`) : la tête
            // de chaîne peut donc lui être comparée sans rien devoir au noyau. Elle
            // sera re-synchronisée à la création du registre, qui relit la taille.
            reg_fin_connue: AtomicU64::new(taille),
            reg_generation_vue: AtomicU64::new(0),
            reprises_de_reservation: AtomicU64::new(0),
            attentes_de_publication: AtomicU64::new(0),
            tetes_reancrees: AtomicU64::new(0),
            generations_vues: AtomicU64::new(0),
            delai_tete_bloquee_ns: AtomicU64::new(DELAI_TETE_BLOQUEE.as_nanos() as u64),
            seuil_temps_ns: AtomicU64::new(SEUIL_TEMPS.as_nanos() as u64),
            diag: DiagGroupe::default(),
        })
    }

    /// Ouvre un WAL EN MÉMOIRE : aucun fichier, donc aucune E/S.
    ///
    /// Le pendant de [`open`](Self::open) là où il n'y a pas de système de
    /// fichiers (wasm32). Le WAL démarre vide — il n'y a rien à rejouer, un store
    /// en mémoire n'ayant pas d'existence antérieure. `ancre_repli` est l'ancre
    /// de départ (défaut : [`ANCRE_INITIALE`]).
    pub fn open_memory(sync_mode: SyncMode, ancre_repli: u64) -> SubstrateResult<Self> {
        Ok(Self {
            path: PathBuf::from("<memoire:obrain>").join(crate::zone::WAL),
            ancre_path: PathBuf::from("<memoire:obrain>").join("substrate.wal.lsn"),
            inner: Mutex::new(WriterInner {
                medium: WalMedium::Memoire(Vec::new()),
                reconciliation: Some(Reconciliation::AncreAdoptee { ancre: ancre_repli }),
                queue_a_repairer: None,
                surveiller_les_courses: false,
                coordination: CoordinationAppend::OAppend,
                pause_apres_ecriture: None,
                pause_apres_reservation: None,
                groupe: GroupCommit::neuf(),
            }),
            offset: AtomicU64::new(0),
            ancre: AtomicU64::new(ancre_repli),
            sync_mode,
            recalages: AtomicU64::new(0),
            courses_suspectees: AtomicU64::new(0),
            records_ecrits: AtomicU64::new(0),
            troncatures_subies: AtomicU64::new(0),
            reprises_apres_troncature: AtomicU64::new(0),
            // ── GROUP COMMIT : ÉTEINT PAR DÉFAUT, ET C'EST LA NON-RÉGRESSION ─
            // Tant que `activer_group_commit` n'est pas appelé, `append` suit le
            // chemin d'aujourd'hui bit-pour-bit (lot de 1, `fsync` par
            // `SyncMode`). Un store existant ne peut donc PAS changer de
            // comportement de durabilité sans le demander explicitement.
            group_actif: AtomicBool::new(false),
            octets_en_tampon: AtomicU64::new(0),
            fsyncs: AtomicU64::new(0),
            poses: AtomicU64::new(0),
            veille: Arc::new(VeillePartagee::neuve()),
            borne_frame_atomique: AtomicU64::new(MAX_FRAME_BYTES as u64),
            registre: OnceLock::new(),
            tentatives_registre: AtomicU64::new(0),
            // Un WAL en mémoire n'a NI fichier NI registre : la chaîne de
            // réservation ne concerne qu'un média fichier partagé (aucun autre
            // processus ne peut l'écrire, et `registre` refusera de se créer).
            reg_fin_connue: AtomicU64::new(0),
            reg_generation_vue: AtomicU64::new(0),
            reprises_de_reservation: AtomicU64::new(0),
            attentes_de_publication: AtomicU64::new(0),
            tetes_reancrees: AtomicU64::new(0),
            generations_vues: AtomicU64::new(0),
            delai_tete_bloquee_ns: AtomicU64::new(DELAI_TETE_BLOQUEE.as_nanos() as u64),
            seuil_temps_ns: AtomicU64::new(SEUIL_TEMPS.as_nanos() as u64),
            diag: DiagGroupe::default(),
        })
    }

    /// Current byte offset of the next record.
    ///
    /// ── INVARIANT MONO-PROCESSUS (T6, `wal_append_unite.rs`) ────────────────
    /// C'est l'offset OCTET de la prochaine frame, bit-identique à la taille du
    /// fichier. Il est publié sous le mutex après chaque écriture, jamais dérivé
    /// du LSN : `store.rs` (baselines de flush, ancres d'index) et
    /// `props_eq_index.rs` comparent cet offset à des valeurs persistées, et tout
    /// changement de sa sémantique les rendrait menteurs.
    pub fn offset(&self) -> u64 {
        self.offset.load(Ordering::Acquire)
    }

    /// Ancre du segment courant : la base des LSN (`lsn = ancre + position`).
    pub fn ancre(&self) -> u64 {
        self.ancre.load(Ordering::Acquire)
    }

    /// Nombre d'enregistrements appendés par CET écrivain (voir le champ).
    pub fn records_ecrits(&self) -> u64 {
        self.records_ecrits.load(Ordering::Acquire)
    }

    /// Le LSN que porterait la PROCHAINE frame (`ancre + offset`).
    ///
    /// N'est plus un compteur : deux appels successifs rendent la même valeur
    /// tant que rien n'a été écrit. C'est une PROJECTION, pas une réservation —
    /// d'où la disparition de l'ancien `next_lsn()`, dont le `fetch_add` par
    /// processus fabriquait des doublons entre processus.
    pub fn peek_lsn(&self) -> u64 {
        self.ancre.load(Ordering::Acquire) + self.offset.load(Ordering::Acquire)
    }

    /// Cas de réconciliation retenu à l'ouverture (ou dernière troncature
    /// étrangère détectée). La réconciliation est TOUJOURS nommée : un écrivain
    /// neuf ne peut pas rendre `None`.
    pub fn reconciliation(&self) -> Option<Reconciliation> {
        self.inner.lock().reconciliation
    }

    /// Nombre de recalages sur la taille réelle (un autre écrivain avait avancé
    /// le fichier). Normal en mode partagé, TOUJOURS 0 en mono-écrivain.
    pub fn recalages(&self) -> u64 {
        self.recalages.load(Ordering::Acquire)
    }

    /// Nombre de LSN estampillés alors qu'un autre écrivain s'était glissé entre
    /// la lecture de la fin et notre `write` : sur ces frames, le LSN est une
    /// PRÉDICTION et non la position. Voir le champ : c'est une BORNE SUPÉRIEURE,
    /// et c'est le test inter-processus qui mesure le taux vrai par relecture.
    pub fn courses_suspectees(&self) -> u64 {
        self.courses_suspectees.load(Ordering::Acquire)
    }

    /// ── COMBIEN DE FRAMES ONT ÉTÉ EMPORTÉES PAR UNE TRONCATURE CONCURRENTE ──
    ///
    /// Un checkpoint d'un autre processus avance l'ancre puis tronque. Une frame
    /// posée entre ces deux instants (ou juste avant la troncature) disparaît :
    /// ce compteur dit combien de fois c'est arrivé à CET écrivain. Chaque
    /// occurrence a été RÉPARÉE par une reprise (voir
    /// [`WalWriter::reprises_apres_troncature`]) — mais le fait mérite d'être
    /// mesuré, parce qu'un store qui répare en boucle n'est pas un store sain :
    /// il dit que la fenêtre de checkpoint est trop large ou trop fréquente.
    pub fn troncatures_subies(&self) -> u64 {
        self.troncatures_subies.load(Ordering::Acquire)
    }

    /// Nombre total de reprises d'append réussies après une troncature
    /// concurrente. Toujours ≤ `troncatures_subies` (une troncature peut épuiser
    /// les reprises et rendre [`crate::SubstrateError::WalTroncatureConcurrente`]).
    pub fn reprises_apres_troncature(&self) -> u64 {
        self.reprises_apres_troncature.load(Ordering::Acquire)
    }

    // ── DIAGNOSTIC DU REGISTRE D'APPEND (pièce 7) ────────────────────────────

    /// Le registre d'append partagé est-il EN SERVICE pour cet écrivain ?
    ///
    /// `false` ne veut pas dire « dégradé » : cela veut dire que le WAL écrit par
    /// le chemin d'AVANT la pièce 7 (deux `statx`), ce qui est le cas d'un média
    /// sans registre (`/dev/full`), d'un répertoire en lecture seule, d'un quota
    /// disque, de wasm32 — ou d'un écrivain qui n'a jamais appendé (le registre
    /// n'est créé qu'au premier append PARTAGÉ).
    pub fn registre_en_service(&self) -> bool {
        self.registre.get().is_some_and(Option::is_some)
    }

    /// Reprises du CAS de réservation (contention entre écrivains). Coût : quelques
    /// nanosecondes, AUCUN appel système — l'ancien chemin payait deux `statx` par
    /// record partagé.
    pub fn reprises_de_reservation(&self) -> u64 {
        self.reprises_de_reservation.load(Ordering::Acquire)
    }

    /// Nombre d'appends qui ont ATTENDU la publication de leur prédécesseur
    /// (spin sur `fin_publiee`, en mémoire partagée, sans appel système).
    ///
    /// C'est la garantie qui rend `lsn = ancre + position` vrai par construction :
    /// on n'écrit que quand la fin réelle vaut déjà notre début de plage. Sur un WAL
    /// où un seul processus écrit, ce compteur reste à **0** — et l'append ne paie
    /// alors qu'un CAS et un `write`.
    pub fn attentes_de_publication(&self) -> u64 {
        self.attentes_de_publication.load(Ordering::Acquire)
    }

    /// Têtes de chaîne RÉ-ANCRÉES sur la taille réelle : un écrivain est mort
    /// entre sa réservation et son `write`, ou un pair a écrit hors chaîne. Chaque
    /// occurrence a coûté le délai [`DELAI_TETE_BLOQUEE`] avant d'être récupérée.
    pub fn tetes_reancrees(&self) -> u64 {
        self.tetes_reancrees.load(Ordering::Acquire)
    }

    /// Troncatures étrangères vues par le REGISTRE (`generation` changée). Le
    /// pendant sans `statx` de [`WalWriter::troncatures_subies`] : celles-ci sont
    /// connues par un atome, donc réparées immédiatement au lieu d'être attendues.
    pub fn generations_vues(&self) -> u64 {
        self.generations_vues.load(Ordering::Acquire)
    }

    /// Queue de chaîne connue de cet écrivain (voir [`WalWriter`] : c'est le mot
    /// qui rend le chemin chaud gratuit). Diagnostic et tests.
    pub fn fin_de_chaine_connue(&self) -> u64 {
        self.reg_fin_connue.load(Ordering::Acquire)
    }

    /// Force la vérification post-écriture « un autre écrivain s'est-il glissé
    /// entre la lecture de la fin et le write ? » dès le premier append.
    ///
    /// Diagnostic et tests inter-processus : en production elle s'allume d'elle
    /// même au premier recalage, ce qui laisse le chemin MONO-ÉCRIVAIN sans le
    /// fstat supplémentaire.
    pub fn forcer_surveillance_courses(&self) {
        self.inner.lock().surveiller_les_courses = true;
    }

    /// Choisit la politique de coordination de l'append (voir
    /// [`CoordinationAppend`]). `OAppend` par défaut : non-perte garantie par le
    /// noyau, et c'est le régime qui scale.
    pub fn coordination(&self, c: CoordinationAppend) {
        self.inner.lock().coordination = c;
    }

    /// Politique de coordination courante.
    pub fn coordination_courante(&self) -> CoordinationAppend {
        self.inner.lock().coordination
    }

    /// La queue est-elle déchirée au point d'interdire d'écrire ?
    pub fn queue_a_repairer(&self) -> Option<String> {
        self.inner.lock().queue_a_repairer.clone()
    }

    /// Force la borne d'une frame (voir [`MAX_FRAME_BYTES`]).
    ///
    /// Seam de test/diagnostic : sans lui, prouver le refus d'une frame trop
    /// grosse exigerait d'allouer 1 Gio. Le refus, lui, est le même code.
    ///
    /// La borne vit désormais dans un ATOMIQUE de l'écrivain (et non plus dans
    /// `WriterInner`) : l'encodage se fait HORS verrou, et le lire demandait donc
    /// une paire de verrous par record — pour un entier que ce seam est le seul à
    /// jamais modifier.
    #[doc(hidden)]
    pub fn forcer_borne_frame(&self, borne: usize) {
        self.borne_frame_atomique
            .store(borne as u64, Ordering::Release);
    }

    /// ── SEAM DE TEST : OUVRIR LA FENÊTRE « ÉCRITE PUIS EMPORTÉE » ──────────
    ///
    /// Endort l'append entre le `write` et la lecture de la taille qui suit. Un
    /// test inter-processus s'en sert pour faire tronquer le WAL par un PAIR
    /// pendant que la frame est posée — l'entrelacement exact que la pièce 1 rend
    /// récupérable, et qu'aucune course réelle ne rend reproductible.
    ///
    /// La pause est prise SOUS le mutex d'append : c'est délibéré (elle doit
    /// suspendre l'append lui-même, pas à côté), et sans effet sur tout autre
    /// chemin puisque le champ est `None` partout en production.
    #[doc(hidden)]
    pub fn forcer_pause_apres_ecriture(&self, duree: std::time::Duration) {
        self.inner.lock().pause_apres_ecriture = Some(duree);
    }

    /// ── SEAM DE TEST : OUVRIR LA FENÊTRE « RÉSERVÉ PUIS MORT » (pièce 7) ────
    ///
    /// Endort l'append entre la RÉSERVATION (le CAS dans le registre) et le
    /// `write`. La plage est alors réservée dans `tete_reservee` sans qu'un seul
    /// octet existe dans le fichier — exactement l'état que laisse un écrivain tué
    /// par `SIGKILL` au mauvais moment. Un test inter-processus s'en sert pour
    /// tuer un enfant DANS cette fenêtre et prouver que les autres écrivains
    /// récupèrent par le chemin froid (attente bornée, ré-ancrage, aucun trou).
    ///
    /// La pause est prise SOUS le mutex d'append (elle doit suspendre l'append
    /// lui-même, pas à côté), et `None` partout en production : le chemin chaud ne
    /// paie pas même une lecture.
    #[doc(hidden)]
    pub fn forcer_pause_apres_reservation(&self, duree: std::time::Duration) {
        self.inner.lock().pause_apres_reservation = Some(duree);
    }

    /// ── SEAM DE TEST : RÉGLER LE DÉLAI DE RÉCUPÉRATION D'UNE TÊTE BLOQUÉE ────
    ///
    /// Le défaut ([`DELAI_TETE_BLOQUEE`], 25 ms) est choisi pour l'EXACTITUDE en
    /// production : il doit couvrir plusieurs quanta d'ordonnanceur, pour ne
    /// jamais prendre un pair VIVANT mais préempté pour un pair mort. Le prouver
    /// sans attendre 25 ms par essai exige ce seam — le chemin exercé, lui, est le
    /// chemin de production (le champ est lu par le chemin froid).
    #[doc(hidden)]
    pub fn forcer_delai_tete_bloquee(&self, duree: std::time::Duration) {
        self.delai_tete_bloquee_ns
            .store(duree.as_nanos() as u64, Ordering::Release);
    }

    /// Marque la queue comme « à réparer » — seam de test/diagnostic.
    ///
    /// Le chemin qu'il ouvre (refus de TOUT append tant que la queue n'est pas
    /// réparée) n'est atteignable en production que si l'ANNULATION d'une écriture
    /// courte échoue elle-même, ce qu'aucun système de fichiers sain ne permet de
    /// provoquer depuis un test. Le contrat testé — refus NOMMÉ, puis réparation
    /// qui le lève — est bien celui du code de production.
    #[doc(hidden)]
    pub fn forcer_queue_a_repairer(&self, raison: &str) {
        self.inner.lock().queue_a_repairer = Some(raison.to_string());
    }

    /// Append a record. The record's `lsn` field is overwritten with
    /// `ancre + position` (voir [`WalWriter`]). Returns the byte offset at which
    /// the record was written and its length.
    ///
    /// ── LE MUTEX EST DÉGRADÉ DE « GARANTIE » À « ORDONNANCEMENT » ───────────
    /// Il ne garantit plus l'unicité des LSN ni l'absence d'écrasement : il
    /// sérialise seulement les threads de CE processus. L'atomicité vient de
    /// `O_APPEND` (le noyau), l'unicité des LSN de la position lue sous ce mutex.
    /// Ce qui a changé par rapport à l'ancien `append` :
    ///   · le LSN n'est plus réservé par `fetch_add` HORS du mutex (deux
    ///     processus émettaient alors le MÊME LSN — sonde `p2`) ;
    ///   · la fin du fichier n'est plus RELUE à chaque append : elle est RÉSERVÉE
    ///     dans le registre partagé (`substrate.wal.reg`), ce qui rend le LSN exact
    ///     par construction au lieu de borné par une mesure ;
    ///   · l'encode reste HORS du mutex (buffer thread-local) — seul le `write`
    ///     et la lecture de fin sont sous le verrou.
    ///
    /// ── SOUS GROUP COMMIT : CE QUE « ÉCRIT » VEUT DIRE (2026-10-06) ─────────
    /// Le retour `(offset, len)` reste l'offset de la POSITION que la frame
    /// occupera — mais, group commit armé, elle est D'ABORD mise en tampon. Les
    /// octets, l'ordre et le LSN sont ceux d'un append immédiat (c'est ce qui
    /// rend le group commit invisible au rejeu) ; seule la DATE à laquelle ils
    /// atteignent le disque change, et elle est bornée par [`SEUIL_TEMPS`]. Un
    /// échec d'E/S, lui, ne peut plus être rapporté par CET appel (l'écriture n'a
    /// pas encore eu lieu) : il l'est au point de flush suivant — le push qui
    /// franchit la needle, le veilleur, `commit_point()`, `fsync()`, ou le Drop.
    pub fn append(&self, rec: WalRecord) -> SubstrateResult<(u64, usize)> {
        self.append_encoded(&rec)
    }

    /// ── APPEND D'UN RECORD EMPRUNTÉ — ZÉRO ALLOCATION PAR RECORD (pièce 2) ──
    ///
    /// C'EST le chemin que `Writer::commit()` appelle. Avant cette pièce, il
    /// passait par `append_batch(core::iter::once(rec))`, qui faisait
    /// `into_iter().collect::<Vec<WalRecord>>()` : une allocation + un `memcpy`
    /// de `WalRecord` (payload compris) PAR ENREGISTREMENT, pour un lot d'UN
    /// élément. Mesuré au banc : C1 = 46,4 ns/record sur ce seul poste.
    ///
    /// Ici le record est emprunté jusqu'au bout : il est encodé directement dans
    /// le buffer du lot, puis posé. Aucun `Vec<WalRecord>`, et le payload
    /// possédé n'est pas cloné.
    pub fn append_encoded(&self, rec: &WalRecord) -> SubstrateResult<(u64, usize)> {
        self.append_frame(
            rec.is_commit() || rec.is_checkpoint() || rec.exige_sync(),
            rec.exige_sync(),
            |out, lsn| rec.encode_append_with_lsn_into(out, lsn, rec.flags),
        )
    }

    /// ── APPEND D'UN PAYLOAD EMPRUNTÉ : `PropSetInlineRef` (pièce 2) ────────
    ///
    /// LE chemin d'écriture de propriété scalaire du store. Il encode la frame
    /// depuis la clé (`&str`) et les octets de valeur (`&[u8]`) que l'appelant
    /// possède DÉJÀ : plus de `String` ni de `Vec<u8>` par record (46,4 ns/record
    /// mesurés). Les octets produits sont ceux de `WalPayload::PropSetInline`
    /// (test `payload_emprunte_encode_les_memes_octets`) : le rejeu ne voit
    /// aucune différence.
    pub fn append_prop_set_inline(
        &self,
        node_id: u32,
        key: &str,
        value_bincode: &[u8],
        timestamp: i64,
    ) -> SubstrateResult<(u64, usize)> {
        let emprunte = crate::wal::PropSetInlineRef {
            node_id,
            key,
            value_bincode,
        };
        self.append_frame(false, false, |out, lsn| {
            emprunte.encode_append_with_lsn_into(out, lsn, timestamp, 0)
        })
    }

    /// ── LE TRONC COMMUN DES APPENDS D'UNE FRAME ─────────────────────────────
    /// Tout ce qui distingue deux appels, c'est la façon d'écrire le PAYLOAD —
    /// le reste (prédiction de LSN, buffer thread-local, mutex, needle, pose) est
    /// identique et ne doit exister qu'une fois.
    ///
    /// `encodeur(out, lsn)` écrit la frame complète (en-tête + payload + CRC) à
    /// la fin de `out` ; il n'est appelé QU'UNE FOIS (le ré-estampillage d'un lot
    /// dont la prédiction a échoué se fait en place, sur les octets, sans
    /// revenir aux données).
    fn append_frame(
        &self,
        commit: bool,
        exige_sync: bool,
        encodeur: impl FnOnce(&mut Vec<u8>, u64) -> SubstrateResult<()>,
    ) -> SubstrateResult<(u64, usize)> {
        if self.group_actif.load(Ordering::Acquire) {
            // `commit || exige_sync` : c'est LA définition d'une frontière de
            // durabilité pour le group commit (voir
            // `GroupCommit::contient_frontiere`). Un simple `PropSetInline` du
            // flux d'ingestion n'en est pas une.
            self.push_group_encode(commit || exige_sync, exige_sync, encodeur)
        } else {
            self.poser_un_encode(commit, encodeur)
        }
    }

    /// ── GROUP COMMIT (T7) : N records, UN SEUL `write` ──────────────────────
    ///
    /// L'ORDRE PHYSIQUE EST CELUI DU LOT, et les LSN suivent les positions :
    /// `lsn_i = ancre + position + offset_de_la_frame_i_dans_le_lot`. Un lot est
    /// donc, sur le disque, indiscernable de N appends successifs — le rejeu n'a
    /// rien à savoir du group commit.
    ///
    /// Aucune frame n'est écrite si l'encodage échoue (le buffer est construit
    /// entièrement avant le `write`), et une écriture COURTE est ANNULÉE par
    /// troncature à la position de départ : un lot est tout ou rien.
    ///
    /// ── SANS `Vec<WalRecord>` (pièce 2) ────────────────────────────────────
    /// Le lot est encodé À MESURE qu'on itère les records, dans le buffer
    /// thread-local : plus de `collect()`, donc plus d'allocation proportionnelle
    /// à la taille du lot. Si la position prédite se révèle fausse sous le verrou,
    /// le lot est RÉ-ESTAMPILLÉ en place (`restampfer_lot`) au lieu d'être
    /// ré-encodé à partir des records — c'est possible parce qu'une frame est
    /// auto-descriptive, et c'est ce qui autorise à ne plus les garder en vie.
    ///
    /// ── SOUS GROUP COMMIT, LA FRONTIÈRE DU LOT N'EST PLUS CELLE DU `write` ──
    /// La needle décide : un lot de 1 peut n'être posé qu'avec les 4 095 suivants,
    /// et un lot de 10 000 peut être posé en 3 `write`. Le retour reste celui du
    /// LOT (offset de sa première frame, somme de ses longueurs), pour ne rien
    /// changer aux appelants existants.
    pub fn append_batch<I>(&self, recs: I) -> SubstrateResult<(u64, usize)>
    where
        I: IntoIterator<Item = WalRecord>,
    {
        let it = recs.into_iter();
        if self.group_actif.load(Ordering::Acquire) {
            let mut premier: Option<u64> = None;
            let mut total = 0usize;
            for rec in it {
                // `append_encoded` porte déjà `commit`/`exige_sync` au push :
                // l'information de frontière ne se perd pas dans ce chemin.
                let (off, len) = self.append_encoded(&rec)?;
                if premier.is_none() {
                    premier = Some(off);
                }
                total += len;
            }
            return Ok((premier.unwrap_or_else(|| self.offset()), total));
        }
        ENCODE_BUF.with(|cell| {
            let mut buf = cell.borrow_mut();
            // ── ENCODAGE HORS MUTEX ──────────────────────────────────────────
            // Les threads encodent en parallèle dans leur propre buffer ; le
            // verrou ne couvre que la lecture de fin + le write syscall.
            // Encodage 2026-09-06 : 0,108 µs → 0,005 µs par record (buffer
            // réutilisé, aucun changement d'octets sur disque).
            //
            // ── COMMENT LE LSN EST ESTAMPILLÉ SANS LA POSITION ? ────────────
            // La position est PRÉDITE ici (`self.offset`, un atomique : aucun
            // syscall), puis VÉRIFIÉE sous le verrou. En mono-écrivain prouvé la
            // prédiction est toujours juste (personne d'autre ne peut avancer le
            // fichier) : le lot part tel quel, sans ré-estampillage — c'est ce qui
            // garde le chemin chaud identique à celui d'avant le fix. En WAL
            // partagé, si la fin a bougé, le lot est RÉ-ESTAMPILLÉ sous le verrou
            // avec la position vraie (le régime qui a déjà un second écrivain est
            // le seul à payer cette seconde passe).
            buf.clear();
            // Sans verrou : la borne est un atomique de l'écrivain (pièce 7).
            let borne = self.borne_frame_atomique.load(Ordering::Acquire) as usize;
            let prediction = self.offset.load(Ordering::Acquire);
            let ancre = self.ancre.load(Ordering::Acquire);
            let mut commit_dans_le_lot = false;
            let mut n = 0usize;
            for rec in it {
                let debut = buf.len();
                rec.encode_append_with_lsn_into(
                    &mut buf,
                    ancre + prediction + debut as u64,
                    rec.flags,
                )?;
                verifier_borne(buf.len() - debut, borne)?;
                // Une exigence de synchronisation vaut un `FLAG_COMMIT` : dans ce
                // chemin (group commit ÉTEINT) la durabilité est celle de
                // `SyncMode`, et ce bit ne fait que la RÉCLAMER là où elle est
                // disponible (`EveryCommit`).
                if rec.is_commit() || rec.is_checkpoint() || rec.exige_sync() {
                    commit_dans_le_lot = true;
                }
                n += 1;
            }
            if n == 0 {
                return Ok((prediction, 0));
            }

            let mut guard = self.inner.lock();
            self.poser_sous_verrou_avec_garde(
                &mut guard,
                &mut buf,
                n,
                commit_dans_le_lot,
                prediction,
                // L'ancre AVEC LAQUELLE le lot vient d'être estampillé : elle est
                // comparée à l'ancre courante sous le verrou, parce qu'un
                // checkpoint (du pair, ou d'un thread du même processus) a pu la
                // faire avancer entre l'encodage et la pose — et des frames
                // estampillées sous la base du segment courant sont des LSN
                // menteurs, même quand leur position, elle, est juste.
                ancre,
                false,
            )
        })
    }

    /// Le chemin NON groupé d'UN record : encoder, puis poser. Identique au
    /// chemin d'avant la pièce 1 — c'est la non-régression par construction.
    fn poser_un_encode(
        &self,
        commit: bool,
        encodeur: impl FnOnce(&mut Vec<u8>, u64) -> SubstrateResult<()>,
    ) -> SubstrateResult<(u64, usize)> {
        ENCODE_BUF.with(|cell| {
            let mut buf = cell.borrow_mut();
            buf.clear();
            // Sans verrou : la borne est un atomique de l'écrivain (pièce 7).
            let borne = self.borne_frame_atomique.load(Ordering::Acquire) as usize;
            let prediction = self.offset.load(Ordering::Acquire);
            let ancre = self.ancre.load(Ordering::Acquire);
            encodeur(&mut buf, ancre + prediction)?;
            verifier_borne(buf.len(), borne)?;
            let mut guard = self.inner.lock();
            self.poser_sous_verrou_avec_garde(
                &mut guard, &mut buf, 1, commit, prediction, ancre, false,
            )
        })
    }

    /// ── LE PUSH DU GROUP COMMIT : METTRE EN TAMPON, OU POSER ────────────────
    ///
    /// Un `push` ne fait qu'UN `write` quand la needle est franchie (ou quand le
    /// record l'exige) ; sinon il ne fait qu'un `memcpy` dans le tampon partagé.
    /// C'est ce qui rend le nombre de `fsync` proportionnel aux OCTETS et non aux
    /// LOTS : 4 096 records poussés un par un donnent UN `fsync`, pas 4 096.
    fn push_group_encode(
        &self,
        frontiere: bool,
        exige_sync: bool,
        encodeur: impl FnOnce(&mut Vec<u8>, u64) -> SubstrateResult<()>,
    ) -> SubstrateResult<(u64, usize)> {
        ENCODE_BUF.with(|cell| {
            let mut buf = cell.borrow_mut();
            buf.clear();
            // Sans verrou : la borne est un atomique de l'écrivain (pièce 7).
            let borne = self.borne_frame_atomique.load(Ordering::Acquire) as usize;
            // ── PRÉDICTION DE POSITION, SANS VERROU ─────────────────────────
            // `offset` (fin de fichier publiée) + `octets_en_tampon` (miroir du
            // tampon) : deux atomiques, AUCUN syscall. C'est la même ruse que le
            // chemin chaud d'aujourd'hui, étendue au tampon. Une valeur périmée
            // n'est jamais une corruption : elle déclenche le ré-estampillage
            // sous verrou, juste en dessous.
            let base_predite =
                self.offset.load(Ordering::Acquire) + self.octets_en_tampon.load(Ordering::Acquire);
            let ancre = self.ancre.load(Ordering::Acquire);
            encodeur(&mut buf, ancre + base_predite)?;
            verifier_borne(buf.len(), borne)?;
            let len = buf.len();

            let mut guard = self.inner.lock();
            if let Some(raison) = guard.queue_a_repairer.clone() {
                return Err(SubstrateError::WalQueueAPreparer { raison });
            }
            // ── INSTRUMENTATION (OBRAIN_GC_DIAG=1) : COMBIEN DE TEMPS ON TIENT ─
            // Une tenue proche du mur total signifie que le chemin d'ingestion
            // TRAVAILLE sous verrou (mémoire, ré-estampillage, et surtout l'écriture
            // et le `fsync` qu'un veilleur y fait) ; une tenue négligeable avec un
            // mur qui double signifie qu'il ATTEND ailleurs — deux diagnostics
            // OPPOSÉS, et les confondre est exactement l'erreur que ce chantier
            // devait éviter.
            //
            // ── L'HORLOGE EST SOUS LE DRAPEAU, ET CE N'EST PAS DE LA COQUETTERIE ─
            // Deux `Instant::now()` par enregistrement coûtaient ~3 % du débit du
            // chemin `Writer::commit` (MESURÉ : 1 386 000 → 1 340 000 rec/s, moyennes
            // de 3 passes alternées) — soit précisément le genre de régression que
            // ce chantier existe pour supprimer. Le drapeau se lit UNE fois et
            // l'horloge ne se lit que s'il la demande (`Option` : sans diagnostic,
            // aucune lecture, et la mesure est simplement absente).
            let diag_actif = DiagGroupe::actif();
            let t_verrou = diag_actif.then(Instant::now);
            // `self.offset` est PUBLIÉ sous ce verrou : c'est la position où le
            // tampon commence. On ne relit PAS la taille du fichier ici (aucun
            // fstat par push) : un pair qui aurait avancé le fichier est détecté
            // à la POSE, qui ré-estampille alors tout le tampon. Le prix de cette
            // économie est exactement celui, déjà documenté, des LSN « prédits »
            // en WAL partagé : ils sont une borne inférieure, jamais un doublon.
            let base = self.offset.load(Ordering::Acquire);
            let tampon_avant = guard.groupe.tampon.len();
            // La prédiction hors verrou tombe-t-elle juste ? (un autre thread a pu
            // pousser entre notre lecture des atomiques et ce verrou)
            let prediction_juste = base_predite == base + tampon_avant as u64;

            guard.groupe.tampon.extend_from_slice(&buf);
            let ancre_maintenant = self.ancre.load(Ordering::Acquire);
            if tampon_avant == 0 {
                // L'ancre du PREMIER record du tampon : c'est elle qui permettra à
                // la POSE de savoir si les frames du tampon portent encore la base
                // du segment courant (voir `GroupCommit::ancre`).
                guard.groupe.ancre = ancre;
            }
            if !prediction_juste
                || guard.groupe.base != base
                || guard.groupe.ancre != ancre_maintenant
            {
                if diag_actif {
                    if !prediction_juste {
                        self.diag
                            .predictions_fausses
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    self.diag.restamps.fetch_add(1, Ordering::Relaxed);
                    self.diag
                        .restamps_octets
                        .fetch_add(guard.groupe.tampon.len() as u64, Ordering::Relaxed);
                }
                restampfer_lot(&mut guard.groupe.tampon, ancre_maintenant, base)?;
                guard.groupe.base = base;
                guard.groupe.ancre = ancre_maintenant;
            }
            guard.groupe.records += 1;
            guard.groupe.octets += len;
            guard.groupe.dernier_push = Instant::now();
            // La frontière de durabilité est MÉMORISÉE pour tout le tampon (et
            // jamais effacée avant une pose) : dès qu'un `commit()` entre dans le
            // tampon, l'échéance redevient un point de durabilité — c'est ce qui
            // garde le contrat de `commit()` intact, sans repayer un `fsync` par
            // enregistrement.
            guard.groupe.contient_frontiere |= frontiere;
            if tampon_avant == 0 {
                // L'instant du PREMIER record du tampon : c'est lui qui borne la
                // fenêtre de perte (voir le champ).
                guard.groupe.debut = guard.groupe.dernier_push;
            }
            self.octets_en_tampon
                .store(guard.groupe.octets as u64, Ordering::Release);
            let position = base + tampon_avant as u64;

            if guard.groupe.needle_franchie() || exige_sync {
                self.poser_le_tampon(&mut guard, PoseTampon::Durable)?;
            } else {
                let debut = guard.groupe.debut;
                drop(guard);
                self.armer_la_veille(debut);
            }
            if let Some(t) = t_verrou {
                DiagGroupe::ajouter(&self.diag.push_tenue_inner_ns, t.elapsed());
            }
            Ok((position, len))
        })
    }

    /// ── POSER LE TAMPON DU GROUP COMMIT : UN `write` ────────────────────────
    ///
    /// Prend le tampon (sans le vider logiquement : voir la remise en place), le
    /// confie à la MÊME fonction d'écriture que tous les autres chemins — donc
    /// mêmes garanties de tout-ou-rien, d'annulation de frame courte, de
    /// détection de troncature concurrente et de ré-estampillage — puis remet le
    /// tampon en place et remet les compteurs de needle à zéro.
    ///
    /// ── EN CAS D'ÉCHEC, LE TAMPON EST GARDÉ ────────────────────────────────
    /// Une écriture courte a déjà ANNULÉ ses octets partiels (`annuler_jusqua`) :
    /// le prochain flush réessaiera les mêmes frames au même offset. Vider le
    /// tampon sur un échec les perdrait DÉFINITIVEMENT — et sans que personne ne
    /// puisse le dire, puisque `append` les a déjà acceptés.
    fn poser_le_tampon(&self, guard: &mut WriterInner, pose: PoseTampon) -> SubstrateResult<()> {
        let fsync = matches!(pose, PoseTampon::Durable | PoseTampon::DurableToujours);
        if guard.groupe.vide() {
            // Rien de neuf à poser. Un `fsync()` EXPLICITE fsynce quand même
            // (mode `Group` : les frames sont écrites sans fsync, c'est l'appelant
            // qui décide quand) ; une échéance de needle, elle, n'a rien à rendre
            // durable et ne compte donc pas un `fsync` de plus.
            if pose == PoseTampon::DurableToujours {
                guard.synchroniser(false)?;
                self.fsyncs.fetch_add(1, Ordering::Relaxed);
            }
            return Ok(());
        }
        // ── LA GARDE DE QUEUE S'APPLIQUE AUSSI ICI (défaut trouvé en relecture) ─
        // `poser_le_tampon` est le seul chemin d'écriture qui n'appelait pas
        // `poser_sous_verrou_avec_garde`, donc le seul qui ÉCRIVAIT malgré une
        // queue déchirée : un append court non réparable pose `queue_a_repairer`,
        // et la pose suivante (needle, veilleur, `commit_point`, `fsync`, Drop)
        // repartait écrire derrière la déchirure — exactement ce que le contrat
        // T4 interdit (« tout append est refusé jusqu'à réparation »), et ce que
        // `reparer_queue` répare en tranchant au dernier bord de frame valide.
        // Le contrôle est APRÈS le cas « tampon vide » : sans group commit armé le
        // tampon est toujours vide, et `fsync()` garde donc son comportement
        // d'avant (il fsynce, il ne juge pas la queue).
        if let Some(raison) = guard.queue_a_repairer.clone() {
            return Err(SubstrateError::WalQueueAPreparer { raison });
        }
        let records = guard.groupe.records;
        let prediction = guard.groupe.base;
        // L'ancre avec laquelle les frames du tampon ont été estampillées : si
        // elle a changé depuis, la pose RÉ-ESTAMPILLE le tampon, même si sa
        // position n'a pas bougé (voir `GroupCommit::ancre`).
        let ancre_predite = guard.groupe.ancre;
        let mut tampon = core::mem::take(&mut guard.groupe.tampon);
        // Chronomètre de la POSE : c'est ce temps-là qui, mesuré à 1,65 ms par
        // pose × 526 poses, faisait le surcoût. Le séparer de l'attente du verrou
        // évite de rendre le veilleur coupable d'un coût qui est celui du `fsync`.
        let t_pose = DiagGroupe::actif().then(Instant::now);
        let resultat = self.poser_sous_verrou(
            guard,
            &mut tampon,
            records,
            // Le mode `EveryCommit` ne fsynce PAS ici : sous group commit, c'est
            // la NEEDLE (ou l'exigence portée par un record) qui décide, et
            // `forcer_fsync` le dit.
            false,
            prediction,
            ancre_predite,
            fsync,
        );
        // Longueur du tampon POSÉ, relevée AVANT la remise en place (la
        // télémétrie en a besoin, et `tampon` est ensuite rendu au groupe).
        let octets_poses = tampon.len() as u64;
        if DiagGroupe::actif() {
            if let Some(t) = t_pose {
                DiagGroupe::ajouter(&self.diag.pose_ns, t.elapsed());
            }
        }
        guard.groupe.tampon = tampon;
        match resultat {
            Ok(_) => {
                if DiagGroupe::actif() {
                    self.diag.poses_groupe.fetch_add(1, Ordering::Relaxed);
                    self.diag
                        .poses_groupe_octets
                        .fetch_add(octets_poses, Ordering::Relaxed);
                }
                guard.groupe.tampon.clear();
                guard.groupe.records = 0;
                guard.groupe.octets = 0;
                guard.groupe.debut = Instant::now();
                // Le tampon repart VIDE : plus aucune frontière en attente.
                guard.groupe.contient_frontiere = false;
                self.octets_en_tampon.store(0, Ordering::Release);
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// ── COMMIT POINT EXPLICITE : poser le tampon maintenant, durablement ────
    ///
    /// C'est ce que le veilleur appelle à l'échéance de [`SEUIL_TEMPS`], et ce
    /// qu'un appelant peut appeler lui-même pour fermer la fenêtre de perte sans
    /// attendre l'échéance. Sans group commit armé c'est un no-op (le record est
    /// déjà parti au moment de l'`append`).
    pub fn commit_point(&self) -> SubstrateResult<()> {
        if !self.group_actif.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut guard = self.inner.lock();
        self.poser_le_tampon(&mut guard, PoseTampon::Durable)
    }

    /// Le même `commit_point`, instrumenté : c'est le chemin du VEILLEUR, et
    /// c'est donc ici que se mesure la seule porte par laquelle le fil de veille
    /// peut contrarier le fil d'ingestion (l'attente de `inner`, puis sa tenue
    /// pendant le `write` + `fsync`). Voir [`DiagGroupe`].
    fn pose_du_veilleur_diag(
        &self,
        diag_actif: bool,
        t_avant: Option<Instant>,
        pose: PoseTampon,
    ) -> SubstrateResult<()> {
        if !self.group_actif.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut guard = self.inner.lock();
        if diag_actif {
            let t_verrou = Instant::now();
            if let Some(t0) = t_avant {
                DiagGroupe::ajouter(&self.diag.veille_attente_inner_ns, t_verrou - t0);
            }
            let r = self.poser_le_tampon(&mut guard, pose);
            DiagGroupe::ajouter(&self.diag.veille_tenue_inner_ns, t_verrou.elapsed());
            return r;
        }
        self.poser_le_tampon(&mut guard, pose)
    }

    /// ── ARMER LE GROUP COMMIT À NEEDLE DE DURABILITÉ ────────────────────────
    ///
    /// POURQUOI C'EST EXPLICITE, ET PAS LE DÉFAUT. Armer le group commit CHANGE
    /// la sémantique de durabilité d'`append` : la frame n'atteint plus le disque
    /// avant le retour de l'appel (elle l'atteint dans la fenêtre
    /// [`SEUIL_TEMPS`], ou avant si la needle est franchie). Aucun store existant
    /// ne doit hériter de ce changement sans le demander : tant que cette fonction
    /// n'est pas appelée, `append` suit le chemin d'aujourd'hui **bit pour bit**.
    ///
    /// Prend `&Arc<Self>` parce que le veilleur de la needle de TEMPS doit tenir
    /// une référence FAIBLE sur l'écrivain : il ne doit ni le maintenir en vie, ni
    /// survivre à son Drop (le Weak ne s'upgrade plus, la boucle sort).
    pub fn activer_group_commit(self: &Arc<Self>) {
        if self.group_actif.swap(true, Ordering::AcqRel) {
            return;
        }
        {
            let mut guard = self.inner.lock();
            guard.groupe.actif = true;
            // Capacité réservée UNE fois : un flush ne doit pas réallouer 4 Mio,
            // et les `resize` de l'encodage ne doivent pas recopier le tampon.
            if guard.groupe.tampon.capacity() < SEUIL_OCTETS + WAL_HEADER_SIZE {
                guard.groupe.tampon.reserve(SEUIL_OCTETS + WAL_HEADER_SIZE);
            }
            // ── LE SEUIL DE RENTABILITÉ DU `fsync` D'ÉCHÉANCE ───────────────
            // Surchargeable PAR MESURE seulement (banc group commit) : c'est ce
            // qui permet de rejouer le régime d'avant le correctif (`=0`, la
            // barrière à chaque échéance) et de balayer la valeur. Le défaut
            // ([`SEUIL_FSYNC_RENTABLE`]) est celui du service.
            if let Ok(v) = std::env::var("OBRAIN_GC_SEUIL_FSYNC_RENTABLE")
                && let Ok(n) = v.parse::<usize>()
            {
                guard.groupe.seuil_fsync_rentable = n;
            }
        }
        let faible = Arc::downgrade(self);
        let veille = Arc::clone(&self.veille);
        // Le thread de veille n'est PAS joint : il sort de lui-même dès que
        // l'écrivain disparaît (`Weak::upgrade` échoue) ou que `Veille::Arrete`
        // est posé par le Drop. Un `join` au Drop exigerait de garder un `JoinHandle`,
        // c'est-à-dire de faire dépendre la destruction du WAL d'un ordonnanceur.
        let _ = std::thread::Builder::new()
            .name("wal-needle-temps".to_string())
            .spawn(move || veiller_la_needle(veille, faible));
    }

    /// Le seuil de temps en vigueur, SANS prendre le verrou d'écriture : c'est
    /// le miroir atomique [`SEUIL_TEMPS`] publié à l'armement et par le seam
    /// `forcer_seuil_temps`. Le veilleur en a besoin pour recalculer une échéance
    /// sans inverser l'ordre des verrous (voir le corps de `veiller_la_needle`).
    fn seuil_temps_du_veilleur(&self) -> Duration {
        Duration::from_nanos(self.seuil_temps_ns.load(Ordering::Acquire))
    }

    /// Le group commit est-il armé ?
    pub fn group_commit_actif(&self) -> bool {
        self.group_actif.load(Ordering::Acquire)
    }

    /// ── LA TÉLÉMÉTRIE DU GROUP COMMIT, RENDUE AU BANC ───────────────────────
    ///
    /// Renvoie une ligne unique et stable (`gc-diag ...`) : c'est ce qui a permis
    /// de localiser la régression ×0,42-0,60 de 2026-10-06 sans deviner (voir
    /// [`DiagGroupe`]). Zéros si `OBRAIN_GC_DIAG` n'est pas posé — la télémétrie
    /// est INERTE par défaut, et le chemin d'écriture ne paie qu'un `load`.
    pub fn diagnostic_group_commit(&self) -> String {
        self.diag.lignes()
    }

    /// Seuil de temps en vigueur (fenêtre de perte maximale du group commit).
    pub fn seuil_temps(&self) -> Duration {
        self.inner.lock().groupe.seuil_temps
    }

    /// ── SEAM DE TEST : CHANGER LE SEUIL DE TEMPS ────────────────────────────
    ///
    /// Régler le seuil de temps sur une valeur très grande ÉCARTE la needle de
    /// temps : les flush observés ne peuvent alors venir que des seuils de
    /// RECORDS ou d'OCTETS, et leur compte devient exact et reproductible. Le
    /// chemin exercé est le chemin de production (le veilleur lit ce champ), pas
    /// une variante de test.
    #[doc(hidden)]
    pub fn forcer_seuil_temps(&self, d: Duration) {
        self.seuil_temps_ns
            .store(d.as_nanos() as u64, Ordering::Release);
        let mut guard = self.inner.lock();
        guard.groupe.seuil_temps = d;
        // Une échéance déjà armée doit être recalculée : sans cela, le veilleur
        // garderait l'ancienne, et le test croirait avoir écarté la needle de
        // temps alors qu'un réveil est déjà programmé.
        let debut = guard.groupe.debut;
        let vide = guard.groupe.vide();
        drop(guard);
        if !vide {
            let mut v = self.veille.etat.lock();
            if matches!(*v, Veille::Arme { .. }) {
                *v = Veille::Arme {
                    deadline: debut + d,
                };
                self.veille.reveil.notify_one();
            }
        }
    }

    /// ── ARMER LA VEILLE DE TEMPS (fenêtre de perte ≤ SEUIL_TEMPS) ────────────
    ///
    /// Appelée à chaque push qui laisse le tampon non posé. Le veilleur dort
    /// SANS échéance quand rien n'attend (coût nul sur un WAL au repos) et se
    /// réveille à `debut + SEUIL_TEMPS` sinon — d'où l'automate [`Veille`].
    fn armer_la_veille(&self, debut: Instant) {
        // Le SEUL point de contention ajouté par le group commit au chemin chaud,
        // et il est ici pour LIRE UN CHAMP IMMUABLE EN PRATIQUE (`seuil_temps`,
        // modifié seulement par le seam de test). Instrumenté pour cette raison :
        // si ce verrou devient l'attente dominante, c'est ce point qu'il faut
        // corriger — pas le veilleur.
        let diag_actif = DiagGroupe::actif();
        let t0 = diag_actif.then(Instant::now);
        let seuil = self.inner.lock().groupe.seuil_temps;
        if let Some(t) = t0 {
            self.diag.armer_appels.fetch_add(1, Ordering::Relaxed);
            DiagGroupe::ajouter(&self.diag.armer_attente_inner_ns, t.elapsed());
        }
        // ── UNE ÉCHÉANCE DÉJÀ PASSÉE SE REPROGRAME (jamais du passé) ────────
        // POURQUOI (défaut mesuré) : `debut` est l'instant du PREMIER record du
        // tampon. Tant qu'une pose n'a pas eu lieu, `debut` NE BOUGE PAS — donc
        // `debut + seuil` reste une échéance de plus en plus périmée. Quand le
        // veilleur REPORTE une pose (voir `peut_reporter_la_barriere`), il
        // recalcule l'échéance à partir de `debut`, retombe sur un instant passé,
        // et se réveille AUSSITÔT : la boucle de report devient un sondage actif.
        // MESURÉ sur `query` (20 000 CREATE) : 20 652 réveils pour ~750 échéances
        // réelles, soit 55 ms de veilleur — un coût qui n'existe pas.
        //
        // La règle est donc : l'échéance ne peut pas être dans le passé. Elle vaut
        // `max(debut + seuil, maintenant + seuil)` dès que le tampon porte des
        // octets. Le cas « rien n'attend » (`tampon vide`) garde, lui, l'échéance
        // NOMINALE `debut + seuil` : c'est ce qui laisse la fenêtre de perte
        // bornée à [`SEUIL_TEMPS`] après une pose, et c'est le contrat du test de
        // la fenêtre de perte.
        let maintenant = Instant::now();
        let deadline = if self.octets_en_tampon.load(Ordering::Acquire) == 0 {
            debut + seuil
        } else {
            (debut + seuil).max(maintenant + seuil)
        };
        let mut v = self.veille.etat.lock();
        match *v {
            // Après un Drop, plus personne ne posera le tampon : ne pas se
            // réarmer évite un réveil stérile sur un écrivain mort.
            Veille::Arrete => {}
            // ── `Repos` N'EST PAS L'ABSENCE D'ÉCHÉANCE ──────────────────────
            // Le veilleur pose `Repos` AVANT de décider de reporter : un `push`
            // qui arrive ensuite DOIT donc pouvoir réarmer, même si l'échéance
            // calculée est plus tardive que l'ancienne. Le comparer comme dans le
            // cas `Arme` (n'armer que si c'est plus tôt) ferait perdre le réveil
            // et le tampon resterait en mémoire jusqu'au `Drop`.
            Veille::Repos => {
                *v = Veille::Arme { deadline };
                self.veille.reveil.notify_one();
            }
            Veille::Arme { deadline: deja } if deadline < deja => {
                *v = Veille::Arme { deadline };
            }
            Veille::Arme { .. } => {}
        }
    }

    /// ── COMBIEN DE `fsync` ONT RÉELLEMENT ÉTÉ EXÉCUTÉS ──────────────────────
    ///
    /// La mesure qui prouve que le `fsync` est indexé sur la NEEDLE : pour 12 288
    /// records poussés un par un (donc 12 288 LOTS), ce compteur vaut 3 — pas
    /// 12 288. Voir `tests/wal_group_commit_needle.rs`.
    pub fn fsyncs(&self) -> u64 {
        self.fsyncs.load(Ordering::Acquire)
    }

    /// Nombre de `write` réellement posés dans le média (un par flush de needle).
    pub fn poses(&self) -> u64 {
        self.poses.load(Ordering::Acquire)
    }

    /// Octets actuellement en attente de pose (group commit éteint : toujours 0).
    pub fn octets_en_tampon(&self) -> u64 {
        self.octets_en_tampon.load(Ordering::Acquire)
    }

    /// Records actuellement en attente de pose (group commit éteint : toujours 0).
    pub fn records_en_tampon(&self) -> usize {
        self.inner.lock().groupe.records
    }

    /// Le chemin commun à TOUS les appels : vérifier la queue à réparer, poser,
    /// rendre la main. Le mutex intra-processus est déjà tenu par l'appelant.
    ///
    /// ── LE VERROU INTER-PROCESSUS A DISPARU (pièce 7, 2026-10-06) ───────────
    /// `CoordinationAppend::VerrouExact` prenait ici un `flock(LOCK_EX)` sur le
    /// descripteur du WAL autour de la paire [lire la fin, écrire] : deux appels
    /// système de plus par record, et un réveil en troupeau des attentes. Ce que
    /// ce verrou achetait — un LSN EXACTEMENT égal à la position — la réservation
    /// en chaîne le donne maintenant par construction, sans verrou chaud : la
    /// position n'est plus lue (elle serait périmée dès la lecture), elle est
    /// RÉSERVÉE par un `compare_exchange`. La garantie est donc PLUS FORTE (elle
    /// ne dépend plus de la discipline de l'appelant ni de la portée d'un
    /// verrou), et elle ne coûte rien.
    fn poser_sous_verrou_avec_garde(
        &self,
        guard: &mut WriterInner,
        buf: &mut Vec<u8>,
        records: usize,
        commit_dans_le_tampon: bool,
        prediction: u64,
        ancre_predite: u64,
        forcer_fsync: bool,
    ) -> SubstrateResult<(u64, usize)> {
        if let Some(raison) = guard.queue_a_repairer.clone() {
            return Err(SubstrateError::WalQueueAPreparer { raison });
        }
        self.poser_sous_verrou(
            guard,
            buf,
            records,
            commit_dans_le_tampon,
            prediction,
            ancre_predite,
            forcer_fsync,
        )
    }

    /// Le corps de l'append, exécuté sous [mutex intra-processus + verrou
    /// inter-processus] : lire la fin, estampiller, écrire, vérifier.
    ///
    /// ── PHASE B, PIÈCE 1 : LA TRONCATURE CONCURRENTE DEVIENT RÉCUPÉRABLE ────
    /// Jusqu'ici la garde d'append DÉTECTAIT la troncature étrangère (la taille
    /// réelle avait diminué) mais ne pouvait rien en faire de mieux que recaler le
    /// curseur : une frame posée dans la fenêtre [lecture de la fin, `write`] d'un
    /// checkpoint ÉLU était DÉTRUITE sans que personne ne la réécrive. C'est la
    /// perte que la Phase B devait fermer, et le signal qui la révèle est exact :
    /// après notre `write` de `len` octets à partir de `position`, la taille du
    /// fichier DOIT valoir au moins `position + len` — sauf si ce que nous venons
    /// d'écrire n'existe plus.
    ///
    /// D'où la règle, et elle est stricte :
    ///
    /// * `apres == fin` — cas nominal (et cas mono-écrivain) ;
    /// * `apres > fin` — un autre écrivain s'est glissé entre notre lecture de la
    ///   fin et notre `write` ; notre frame est intacte, plus loin que prévu, et
    ///   son LSN est une borne inférieure (voir `courses_suspectees`) ;
    /// * `apres < fin` — **notre frame a été emportée par une troncature**. On
    ///   relit l'ancre publiée par le checkpointeur qui nous a emportés, on recale
    ///   la position sur la taille RÉELLE, on ré-encode et on réécrit. La perte
    ///   est ainsi RÉPARÉE par l'écrivain qui l'a subie, sans verrou chaud et sans
    ///   coordination : c'est le seul endroit du système qui SAIT qu'une frame a
    ///   disparu, parce que c'est le seul qui l'ait vue exister.
    /// ── UNE SEULE FONCTION D'ÉCRITURE POUR TOUS LES CHEMINS (2026-10-06) ────
    /// Le lot d'un `append`, le lot d'un `append_batch` et le tampon du group
    /// commit passent par ICI : les garantir tout-ou-rien, annuler une frame
    /// courte, détecter une troncature concurrente et ré-estampiller un lot ne
    /// sont donc écrits qu'une fois. `records` et `commit_dans_le_tampon` sont
    /// fournis par l'appelant parce que le tampon peut contenir N frames sans que
    /// cette fonction ait à les décoder.
    fn poser_sous_verrou(
        &self,
        guard: &mut WriterInner,
        buf: &mut Vec<u8>,
        records: usize,
        commit_dans_le_tampon: bool,
        // Position PRÉDITE hors verrou, sur laquelle le lot a déjà été encodé.
        prediction: u64,
        // L'ancre avec laquelle le lot a été ESTAMPILLÉ (voir `GroupCommit::ancre`
        // et les chemins d'encodage) : si elle a changé à l'arrivée ici, les LSN
        // du lot désignent le segment PRÉCÉDENT, et il faut les refaire.
        ancre_predite: u64,
        // Le fsync est-il DÛ indépendamment de `SyncMode` ? (group commit : la
        // needle ou l'exigence d'un record l'ont décidé)
        forcer_fsync: bool,
    ) -> SubstrateResult<(u64, usize)> {
        let commit_dans_le_lot = commit_dans_le_tampon;
        // Nombre de frames emportées par une troncature concurrente DANS CET
        // append. Zéro dans tous les cas nominaux — y compris tous les cas
        // multi-écrivains sans checkpoint.
        let mut emportees: u32 = 0;
        loop {
            // ── D'OÙ VIENT LA POSITION ? ─────────────────────────────────────
            // SEUL ÉCRIVAIN PROUVÉ (`Aucune`) : notre compteur EST la fin du
            // fichier — personne d'autre ne peut écrire, aucune vérification
            // n'est due, et le chemin chaud reste EXACTEMENT celui d'avant le fix
            // (un `write`, rien d'autre). C'est la décision qui empêche ce
            // chantier de coûter quoi que ce soit au cas normal.
            //
            // WAL PARTAGÉ : la position vient d'une RÉSERVATION dans le registre
            // d'append (`<wal>.reg`), pas d'une mesure. Un seul
            // `compare_exchange` rend l'offset exact — au lieu du `statx` d'avant,
            // et sans le `flock` que `VerrouExact` payait pour la même garantie.
            // Quand le registre est indisponible, on retombe sur le chemin
            // d'AVANT : une mesure (`statx`) sous le verrou, et un second `statx`
            // après (voir `observe`). C'est la non-régression par repli.
            let reg = match guard.coordination {
                CoordinationAppend::Aucune => None,
                _ => self.registre(guard),
            };
            let (position, chemin_froid) = match reg {
                None if guard.coordination == CoordinationAppend::Aucune => {
                    (self.offset.load(Ordering::Acquire), false)
                }
                None => {
                    let taille = guard.taille_reelle()?;
                    (self.caler_sur_la_fin(guard, taille)?, true)
                }
                Some(reg) => self.reserver_dans_la_chaine(guard, reg, buf.len() as u64)?,
            };
            // La prédiction faite HORS verrou est-elle tombée juste ? Sinon (WAL
            // partagé où un autre écrivain est passé), le lot est RÉ-ESTAMPILLÉ
            // avec la position VRAIE : l'invariant `lsn = ancre + position` tient
            // dans tous les régimes.
            //
            // Après une reprise, on RÉ-ESTAMPILLE TOUJOURS : l'ancre a pu avancer
            // (c'est même la cause de la troncature), et le lot porte les LSN de
            // l'ancien segment.
            // ── RÉ-ESTAMPILLAGE EN PLACE, PLUS DE RÉ-ENCODAGE ───────────────
            // Le lot est déjà encodé : lui redonner ses LSN coûte un parcours de
            // frames et un CRC par frame, là où ré-encoder exigerait de garder les
            // `Vec<WalRecord>` (l'allocation que la pièce 2 supprime) et de
            // refaire la sérialisation bincode complète. Le contrôle de borne a
            // déjà eu lieu à l'encodage : la taille des frames ne change pas ici.
            //
            // ── L'ANCRE FAIT PARTIE DE LA CONDITION (pièce 7) ───────────────
            // Une position juste avec une ancre périmée donne des LSN SOUS la base
            // du segment courant — le cas exact que le test inter-processus
            // `t2_checkpoint_d_un_processus_ne_perd_pas_l_append_de_l_autre`
            // refuse (`lsn_min >= ancre_apres`). La comparer coûte UNE lecture
            // d'atome.
            if position != prediction
                || emportees > 0
                || ancre_predite != self.ancre.load(Ordering::Acquire)
            {
                restampfer_lot(buf, self.ancre.load(Ordering::Acquire), position)?;
            }

            let len = buf.len();
            // ── SEAM DE TEST : LA FENÊTRE « RÉSERVÉ PUIS MORT » (pièce 7) ────
            // Placé EXACTEMENT là où la fenêtre est réelle : la plage est
            // réservée dans `tete_reservee` (et RIEN n'est publié dans
            // `fin_publiee`), donc aucun octet n'existe encore dans le fichier. Un enfant tué ici reproduit à volonté l'écrivain mort entre
            // son CAS et son `write` que le chemin froid doit récupérer.
            if let Some(duree) = guard.pause_apres_reservation {
                std::thread::sleep(duree);
            }

            // ── LA GÉNÉRATION EST LUE AVANT LE `write` ───────────────────────
            // Elle sert, juste après, à savoir si un checkpoint a tronqué PENDANT
            // notre écriture : c'est le seul signal qui reste disponible sans
            // `statx` (voir `observe`).
            let generation_avant = reg.map(|r| r.generation().load(Ordering::Acquire));
            // ── UN SEUL write, ET SON COMPTE RÉEL ────────────────────────────
            let ecrits = guard.ecrire_atomique(buf)?;
            if ecrits == 0 {
                // ── 0 OCTET = PROPRE : le disque a refusé la frame ENTIÈRE ───
                // Rien n'a été posé, rien n'est déchiré : la queue est intacte
                // et aucune réparation n'est due. C'est le cas OPPOSÉ de
                // l'écriture courte ci-dessous, et les confondre ferait
                // tronquer un fichier sain.
                //
                // La RÉSERVATION, elle, doit être RELÂCHÉE : notre plage n'existe
                // pas dans le fichier, et la laisser dans la tête ferait payer aux
                // appends suivants le délai de récupération d'une tête qu'ils
                // prendraient pour un écrivain mort.
                if let Some(reg) = reg {
                    self.relacher_la_reservation(reg, position + len as u64, position);
                }
                return Err(SubstrateError::Io(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "WAL : le disque a refusé la frame ENTIÈRE (0 octet écrit) — \
                     queue intacte, rien à réparer",
                )));
            }
            if ecrits < len {
                // ── ÉCRITURE COURTE : LA FRAME EST DÉCHIRÉE, ON L'ANNULE ─────
                let courante = guard.taille_reelle()?;
                if position > courante {
                    // ── TAILLE DIMINUÉE SOUS NOUS : ON NE DEVINE PAS OÙ TRANCHER ─
                    //
                    // `position` date d'AVANT la troncature d'un pair. Deux
                    // maux possibles, et aucun ne se répare en devinant :
                    //   · `set_len(position)` ALLONGERAIT le fichier en le
                    //     zéro-remplissant — le « trou de zéros » exact que cette
                    //     phase existe pour fermer ;
                    //   · `set_len(courante - ecrits)` suppose que personne n'a
                    //     écrit APRÈS nos octets partiels. Or un pair peut l'avoir
                    //     fait entre notre `write` et cette lecture : trancher là
                    //     couperait SA frame, c'est-à-dire détruire le travail d'un
                    //     autre écrivain pour réparer le nôtre.
                    //
                    // On refuse donc, en le disant : `offset` est recalé sur la
                    // taille RÉELLE (jamais au-delà de l'EOF), et la queue reste à
                    // réparer par `reparer_queue()`, qui lit le WAL et tranche au
                    // dernier BORD DE FRAME valide — la seule cible qui ne devine
                    // rien. Chemin atteignable seulement par DISQUE PLEIN **et**
                    // checkpoint concurrent : rare, et réparable.
                    let raison = format!(
                        "écriture courte ({ecrits}/{len}) à l'offset {position}, et la \
                         taille réelle ({courante}) est INFÉRIEURE : une troncature \
                         concurrente est passée pendant l'écriture — l'annulation ne peut \
                         pas deviner où trancher sans risquer d'emporter la frame d'un pair"
                    );
                    self.offset.store(courante, Ordering::Release);
                    self.reg_fin_connue.store(courante, Ordering::Release);
                    guard.queue_a_repairer = Some(raison.clone());
                    return Err(SubstrateError::WalQueueAPreparer { raison });
                }
                // La RÉSERVATION est relâchée : la plage n'a jamais été écrite
                // EN ENTIER, et la laisser dans la tête ferait passer notre échec
                // pour un écrivain mort (les suivants paieraient le délai de
                // récupération pour rien).
                if let Some(reg) = reg {
                    self.relacher_la_reservation(reg, position + len as u64, position);
                }
                return match guard.annuler_jusqua(position) {
                    Ok(()) => {
                        self.offset.store(position, Ordering::Release);
                        Err(SubstrateError::WalEcritureCourte {
                            ecrits,
                            demandes: len,
                            position,
                        })
                    }
                    Err(e) => {
                        // ── REFUS TANT QUE LA QUEUE N'EST PAS RÉPARÉE ─────────
                        let raison = format!(
                            "écriture courte ({ecrits}/{len}) à l'offset {position}, et \
                             l'annulation a échoué : {e}"
                        );
                        guard.queue_a_repairer = Some(raison.clone());
                        Err(SubstrateError::WalQueueAPreparer { raison })
                    }
                };
            }

            // ── SEAM DE TEST (voir le champ) ─────────────────────────────────
            // Placé EXACTEMENT là où la fenêtre est réelle : la frame est posée,
            // sa taille n'a pas encore été observée. Un pair qui tronque ici
            // reproduit la perte que la pièce 1 doit réparer.
            if let Some(duree) = guard.pause_apres_ecriture {
                std::thread::sleep(duree);
            }
            let fin = position + len as u64;
            // ── PUBLIER LA FIN ÉCRITE (l'autre moitié du protocole) ───────────
            // `fin_publiee` est ce qui autorise le SUIVANT à écrire : tant qu'il
            // vaut moins que le début de sa plage, il attend (spin atomique). On ne
            // le publie donc QUE si notre `write` a été COMPLET — c'est la règle du
            // design, et c'est elle qui interdit à quiconque d'écrire par-dessus des
            // octets qui n'existent pas.
            //
            // ── POURQUOI ON NE PUBLIE QUE SI LA CHAÎNE NOUS ATTEND ────────────
            // Notre plage commence à `position` : la publier n'a de sens que si
            // `fin_publiee` vaut ENCORE `position`, c'est-à-dire si notre prédécesseur
            // a publié et que personne ne s'est glissé avant nous. Sinon un pair a
            // été plus vite (ou une récupération a ré-ancré la chaîne) : publier
            // `position + len` ferait sauter le marqueur par-dessus SA plage, et le
            // suivant écrirait trop haut. On le COMPTE alors comme anomalie et on
            // rend l'observation obligatoire — la mesure, elle, dira où nous sommes.
            let mut chaine_intacte = true;
            if let Some(reg) = reg {
                if reg.fin_publiee().load(Ordering::Acquire) == position {
                    reg.publier_la_fin(fin);
                } else {
                    chaine_intacte = false;
                    self.courses_suspectees.fetch_add(1, Ordering::Relaxed);
                    guard.surveiller_les_courses = true;
                    self.oublier_la_tete();
                    tracing::warn!(
                        position,
                        fin,
                        fin_publiee = reg.fin_publiee().load(Ordering::Acquire),
                        "WAL partagé : la chaîne n'attendait plus notre plage à la fin de notre \
                         write (un pair a publié avant nous, ou une récupération a ré-ancré la \
                         chaîne) — la position est vérifiée par mesure"
                    );
                }
            }
            // ── L'OBSERVATION APRÈS ÉCRITURE DOIT-ELLE ÊTRE PAYÉE ? ──────────
            // ── CE QU'ELLE EXISTE POUR VOIR, ET RIEN D'AUTRE ────────────────
            // Un seul événement justifie ce `statx` : une TRONCATURE d'un pair
            // pendant notre `write`, qui emporte la frame qu'on vient de poser
            // (pièce 1 : la perte doit être réparée par celui qui l'a subie).
            // Avant la pièce 7, la condition était « coordination partagée » :
            // DEUX `statx` par record, dont un qui ne pouvait rien voir dans
            // l'immense majorité des cas. Aujourd'hui la question est posée à des
            // ATOMES, qui coûtent quelques nanosecondes :
            //   · la GÉNÉRATION a-t-elle changé pendant notre `write` ? C'est
            //     l'annonce explicite d'une troncature (elle est publiée `Release`
            //     APRÈS la troncature et l'ancre devenue neuve) ;
            //   · la CHAÎNE nous attendait-elle pour publier notre fin ? (voir la
            //     publication juste au-dessus : la mesurer, c'est savoir qu'un pair
            //     ou une récupération a bougé la tête sous nous) ;
            //   · le CHEMIN FROID a-t-il servi ? (un pair a réservé depuis notre
            //     dernière publication : un checkpoint de ce pair est possible) ;
            //   · le drapeau `surveiller_les_courses` est-il allumé ? (il l'est dès
            //     qu'un pair s'est manifesté — comportement d'avant, inchangé) ;
            //   · le seam de pause est-il posé ? (les tests qui ouvrent la fenêtre
            //     à la main en dépendent : le chemin doit rester celui de
            //     production, donc l'observation doit rester due).
            // En `Aucune` (seul écrivain prouvé) et en l'absence de drapeau, rien
            // de tout cela ne peut arriver — et on ne paie rien, exactement comme
            // avant. Le mono-écrivain partagé, lui, passe de deux `statx` à ZÉRO.
            let generation_apres = reg.map(|r| r.generation().load(Ordering::Acquire));
            let observe = if guard.coordination == CoordinationAppend::Aucune
                && !guard.surveiller_les_courses
            {
                None
            } else if reg.is_none() {
                // Repli (pas de registre) : le comportement d'AVANT la pièce 7,
                // à la lettre — c'est la non-régression quand `<wal>.reg` est
                // impossible à créer (`/dev/full`, quota, répertoire en lecture
                // seule).
                Some(guard.taille_reelle()?)
            } else if chemin_froid
                || !chaine_intacte
                || guard.surveiller_les_courses
                || guard.pause_apres_ecriture.is_some()
                || generation_apres != generation_avant
            {
                Some(guard.taille_reelle()?)
            } else {
                None
            };
            match observe {
                // ── FRAME EMPORTÉE, OU FRAME SURVIVANTE PLUS BAS ? ──────────
                // Voir la doc de `nos_octets_sont_dans_la_queue` : `apres < fin`
                // recouvre LES DEUX cas, et les confondre dupliquait la frame.
                Some(apres)
                    if apres < fin && guard.nos_octets_sont_dans_la_queue(buf, apres)? =>
                {
                    // Notre frame est intacte, simplement plus bas que prévu (le
                    // pair a tronqué entre notre lecture de la fin et notre
                    // write). Rien à réécrire : on recale et on le compte — le
                    // LSN qu'elle porte est une prédiction, comme pour toute
                    // frame écrite pendant qu'un pair avançait le fichier.
                    self.courses_suspectees.fetch_add(1, Ordering::Relaxed);
                    self.offset.store(apres, Ordering::Release);
                    // L'ancre a changé (c'est la troncature du pair qui l'a
                    // avancée) : la relire évite d'estampiller le prochain append
                    // avec l'ancre du segment précédent.
                    self.rafraichir_ancre_durable();
                    // ── LA CHAÎNE N'EST PLUS NOTRE VUE ──────────────────────
                    // Notre réservation désignait un offset d'AVANT la troncature :
                    // elle ne vaut plus rien. On OUBLIE la tête (voir
                    // `oublier_la_tete`) : le prochain tour repassera par le chemin
                    // froid, qui VÉRIFIE la tête — et attend, borné, qu'un pair qui
                    // a réservé dans le nouveau segment ait publié. Écrire ici la
                    // taille observée écraserait cette réservation-là.
                    if let Some(reg) = reg {
                        self.oublier_la_tete();
                        self.reg_generation_vue
                            .store(reg.generation().load(Ordering::Acquire), Ordering::Release);
                    }
                    tracing::warn!(
                        position,
                        taille_apres = apres,
                        longueur = len,
                        "WAL partagé : la frame est intacte mais PLUS BAS que prédit (un pair                          a tronqué entre notre lecture de la fin et notre write) — aucune                          réécriture, aucun doublon ; son LSN est une borne inférieure"
                    );
                }
                // ── LA FRAME A ÉTÉ EMPORTÉE : ON LA RÉÉCRIT ─────────────────
                Some(apres) if apres < fin => {
                    self.troncatures_subies.fetch_add(1, Ordering::Relaxed);
                    emportees += 1;
                    // Le curseur dit la VÉRITÉ avant tout retour : même en échec
                    // par épuisement, `offset()` ne désigne jamais un offset
                    // au-delà de l'EOF.
                    self.offset.store(apres, Ordering::Release);
                    if emportees > MAX_REPRISES_TRONCATURE {
                        return Err(SubstrateError::WalTroncatureConcurrente {
                            essais: emportees,
                            position,
                        });
                    }
                    // Le checkpoint qui nous a emportés a publié son ancre AVANT
                    // de tronquer (ordre invariant, voir `truncate`) : elle est
                    // donc déjà lisible, et c'est elle qui fonde les LSN du
                    // nouveau segment.
                    self.rafraichir_ancre_durable();
                    guard.surveiller_les_courses = true;
                    // ── LA CHAÎNE AUSSI DOIT ÊTRE OUBLIÉE (et non ré-ancrée) ──
                    // La troncature a emporté notre frame : notre vue de la tête est
                    // fausse (le checkpointer a remis la sienne à 0, et des pairs ont
                    // pu réserver dans le nouveau segment). On l'OUBLIE sans toucher
                    // au registre : la reprise qui suit (le `continue`) repasse par le
                    // chemin froid, qui VÉRIFIE la tête partagée et attend le pair en
                    // vol. Écrire ici la taille observée écraserait sa réservation.
                    if let Some(reg) = reg {
                        self.oublier_la_tete();
                        self.reg_generation_vue
                            .store(reg.generation().load(Ordering::Acquire), Ordering::Release);
                    }
                    guard.reconciliation = Some(Reconciliation::TroncatureEtrangere {
                        ancre: self.ancre.load(Ordering::Acquire),
                        plancher_attendu: fin,
                        taille_reelle: apres,
                    });
                    self.reprises_apres_troncature
                        .fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(
                        position,
                        taille_apres = apres,
                        longueur = len,
                        ancre = self.ancre.load(Ordering::Acquire),
                        "WAL partagé : la frame vient d'être EMPORTÉE par une troncature \
                         concurrente (checkpoint d'un autre processus) — ancre relue, \
                         position recalée, frame RÉÉCRITE (aucune perte)"
                    );
                    continue;
                }
                Some(apres) if apres != fin => {
                    // Le fichier a GRANDI au-delà de notre frame.
                    //
                    // ── SOUS LA CHAÎNE, CE N'EST PAS UNE PRÉDICTION ───────────
                    // Un pair a réservé APRÈS nous et a publié : notre frame est bien
                    // à `position` (la discipline de vérification l'impose), et ce
                    // n'est donc PAS le cas « un écrivain s'est glissé entre notre
                    // lecture de la fin et notre write » du régime d'avant. Le
                    // compter comme LSN suspect ferait mentir un compteur que le test
                    // d'exactitude (`t2b`) exige à zéro.
                    //
                    // ── SANS REGISTRE (repli), LA SÉMANTIQUE D'AVANT SUBSISTE ─
                    // Là, la position venait d'un `statx` : un pair a pu lire la même
                    // fin que nous et écrire avant nous, donc notre LSN redevient une
                    // borne inférieure — et le compteur doit le dire.
                    if reg.is_none() {
                        self.courses_suspectees.fetch_add(1, Ordering::Relaxed);
                    }
                    self.offset.store(apres, Ordering::Release);
                    // ── ON N'ÉCRASE PAS LA TÊTE ICI (défaut trouvé en vérifiant) ──
                    // Le fichier a GRANDI : c'est un pair qui a écrit. La tête, elle,
                    // peut être en avance sur le fichier parce qu'un AUTRE pair a
                    // réservé sans avoir encore écrit — la recaler sur `apres`
                    // détruirait cette réservation, et la frame de ce pair partirait
                    // sous son LSN. On oublie donc notre vue, sans toucher au
                    // registre : la vérification (et l'avance, si le fichier dépasse
                    // vraiment la tête) aura lieu au prochain append, dans
                    // `attendre_la_tete`.
                    if reg.is_some() {
                        self.oublier_la_tete();
                    }
                    tracing::warn!(
                        position,
                        taille_apres = apres,
                        "WAL partagé : le fichier a grandi entre notre lecture de la fin et la \
                         vérification qui suit notre write — le LSN de cette frame n'est \
                         peut-être pas sa position (borne supérieure, voir le compteur)"
                    );
                }
                Some(_) | None => {
                    self.offset.store(fin, Ordering::Release);
                    // La chaîne est vérifiée : `fin` EST la taille réelle (c'est ce
                    // que l'observation vient de constater, ou ce que le chemin
                    // chaud tient pour acquis). C'est ce mot qui rend le prochain
                    // append GRATUIT.
                    self.reg_fin_connue.store(fin, Ordering::Release);
                }
            }

            self.records_ecrits
                .fetch_add(records as u64, Ordering::Relaxed);
            self.poses.fetch_add(1, Ordering::Relaxed);
            // ── LE FSYNC EST INDEXÉ SUR LA NEEDLE, PLUS SUR LE LOT ──────────
            // `forcer_fsync` vient du group commit (needle franchie, exigence
            // d'un record, échéance de temps, `fsync()` explicite). Le mode
            // `EveryCommit` garde exactement sa règle d'avant pour tous les
            // appelants qui n'arment PAS le group commit : un `fsync` par lot
            // contenant un `FLAG_COMMIT` — donc un par `commit()`, ce qui est la
            // durabilité demandée et le 644 rec/s mesuré.
            let should_fsync = forcer_fsync
                || match self.sync_mode {
                    SyncMode::EveryCommit => commit_dans_le_lot,
                    SyncMode::Group => false,
                    SyncMode::Never => false,
                };
            if should_fsync {
                guard.synchroniser(false)?;
                self.fsyncs.fetch_add(1, Ordering::Relaxed);
            }
            return Ok((position, len));
        }
    }

    /// Relit l'ancre durable du side-car et l'adopte si elle a AVANCÉ.
    ///
    /// ── POURQUOI ELLE N'ÉCRIT JAMAIS EN ARRIÈRE (`if >`) ─────────────────────
    /// Le side-car est publié par le checkpoint ÉLU ; un lecteur peut le lire à
    /// tout moment. Une ancre plus PETITE que la nôtre ne peut venir que d'un
    /// état antérieur (une publication en cours de rename, ou un side-car
    /// restauré) : la reculer ferait estampiller des LSN déjà utilisés — deux
    /// frames distinctes porteraient le même LSN, exactement ce que l'ancre
    /// existe pour empêcher. Le sens de la correction est donc strictement
    /// croissant.
    fn rafraichir_ancre_durable(&self) {
        if let Some(a) = lire_ancre_sur(&self.ancre_path)
            && a.ancre > self.ancre.load(Ordering::Acquire)
        {
            self.ancre.store(a.ancre, Ordering::Release);
        }
    }

    // -----------------------------------------------------------------------
    // Registre d'append mmapé partagé (pièce 7) — la chaîne de réservation
    // -----------------------------------------------------------------------

    /// Le registre d'append de ce WAL, CRÉÉ à la première demande.
    ///
    /// ── POURQUOI PARESSEUSEMENT (et pourquoi ce n'est pas un `open` raté) ───
    /// Un store mono-écrivain prouvé (`Aucune`) n'a aucun besoin de ce fichier :
    /// le créer à l'ouverture ferait apparaître `substrate.wal.reg` dans toute
    /// base — y compris celles qui n'auront jamais deux écrivains — et ferait
    /// payer une E/S à une ouverture que ce chantier laisse bit-identique. Le
    /// registre n'apparaît donc qu'au PREMIER append partagé.
    ///
    /// Il peut légitimement être IMPOSSIBLE à créer : `open_avec_ancre` accepte un
    /// WAL sur `/dev/full` (tests d'écriture courte), un quota disque peut faire
    /// échouer le `set_len` en `EFBIG`, un répertoire peut être en lecture seule,
    /// et wasm32 n'a pas de multi-processus. Dans TOUS ces cas on rend `None` et
    /// l'append reprend le chemin d'avant (mesure `statx` + vérification) : le
    /// registre est une optimisation de COÛT, jamais une condition de correction.
    fn registre(&self, guard: &mut WriterInner) -> Option<&RegistreAppend> {
        if let Some(deja) = self.registre.get() {
            return deja.as_ref();
        }
        // Un WAL en mémoire n'a pas de fichier : rien à partager.
        if !matches!(guard.medium, WalMedium::File(_)) {
            let _ = self.registre.set(None);
            return None;
        }
        // Borné : une cause TRANSITOIRE (quota) ne doit pas dégrader
        // définitivement, mais une cause DURABLE ne doit pas coûter une tentative
        // d'E/S par append.
        if self.tentatives_registre.fetch_add(1, Ordering::Relaxed) >= MAX_TENTATIVES_REGISTRE {
            return None;
        }
        let Ok(taille) = guard.taille_reelle() else {
            return None;
        };
        let cree =
            RegistreAppend::ouvrir(&self.path, taille, self.ancre.load(Ordering::Acquire)).ok();
        match &cree {
            Some(reg) => {
                // Notre vue de départ : la tête vaut la taille RÉELLE, et la
                // génération courante est TENUE POUR VUE (une génération non nulle
                // dit qu'un checkpoint a déjà tronqué ce fichier — ne pas la
                // réconcilier ferait prendre sa troncature pour la nôtre).
                self.reg_fin_connue.store(taille, Ordering::Release);
                self.reg_generation_vue
                    .store(reg.generation().load(Ordering::Acquire), Ordering::Release);
                self.adopter_l_ancre_partagee(reg);
                // ── LE REGISTRE PEUT ÊTRE EN AVANCE SUR LE FICHIER ──────────
                // Cas vécu (`wal_append_unite::cas_3`) : le WAL a été tronqué
                // HORS CHAÎNE (un `set_len` extérieur, un fichier restauré) pendant
                // qu'aucun écrivain ne tournait. La création du registre est le seul
                // moment où l'écrivain paie une mesure : c'est là qu'on peut le voir
                // sans un `statx` par record.
                let _ = self.reancrer_si_troncature(guard, reg).ok()?;
            }
            None => tracing::warn!(
                chemin = %self.path.display(),
                "WAL : registre d'append partagé INDISPONIBLE (création impossible) — \
                 le chemin d'avant la pièce 7 est repris : deux `statx` par append. \
                 La correction est inchangée (`O_APPEND` + noyau), seul le coût l'est."
            ),
        }
        let _ = self.registre.set(cree);
        self.registre.get().and_then(Option::as_ref)
    }

    /// Le registre DÉJÀ créé, s'il existe (sans le créer) — pour les chemins
    /// froids (`truncate`, `reparer_queue`) qui doivent ANNONCER une troncature
    /// sans pour autant imposer un fichier à un store mono-écrivain.
    fn registre_existant(&self) -> Option<&RegistreAppend> {
        self.registre.get().and_then(Option::as_ref)
    }

    /// ── LA RÉSERVATION EN CHAÎNE : LE CAS QUI REMPLACE LES DEUX `statx` ─────
    ///
    /// Rend `(offset réservé, chemin_froid)`. L'offset est EXACT **par
    /// construction** : c'est la valeur observée de `tete_reservee`, et le
    /// `compare_exchange` a échoué si elle avait bougé entre la lecture et la
    /// réservation. Rien n'est mesuré, rien n'est prédit.
    ///
    /// ── LES QUATRE ÉTAPES, ET CE QUE CHACUNE COÛTE ──────────────────────────
    /// 1. **Génération** — un checkpoint (le pair ÉLU, ou nous) a publié une ancre
    ///    neuve et tronqué : on relit l'ancre (side-car + registre) et on OUBLIE
    ///    notre vue du fichier. C'est le chemin froid « une fois par checkpoint ».
    /// 2. **Quiescence** — la chaîne est-elle SANS réservation en attente
    ///    (`tete_reservee == fin_publiee`) et cette valeur est-elle celle que NOUS
    ///    savons être la fin réelle (notre propre publication, ou une mesure) ?
    ///    Deux lectures d'atomes. Si oui, la taille réelle EST cette valeur :
    ///    **zéro appel système**.
    /// 3. **Réservation** — le `compare_exchange` qui attribue la plage. S'il
    ///    échoue, un pair a réservé : on reprend AU-DESSUS de sa plage (on ne
    ///    saute jamais la réservation de quelqu'un).
    /// 4. **Attente de publication** — on n'écrit QUE lorsque tout ce qui est
    ///    au-dessous a été publié (`fin_publiee == mon_debut`). C'est ce qui rend
    ///    `lsn = ancre + position` vrai PAR CONSTRUCTION, et l'attente est un spin
    ///    sur un ATOMIQUE partagé : **zéro appel système**, là où l'ancien chemin
    ///    payait un `statx` (802 ns) par record pour la même information.
    ///
    /// ── CE QUI RESTE PAYANT, ET POURQUOI C'EST RARE ─────────────────────────
    /// Un `statx` n'est plus payé que par la RÉCUPÉRATION : un prédécesseur mort
    /// (tête bloquée, voir [`DELAI_TETE_BLOQUEE`]) ou des octets publiés au-delà
    /// de notre début (écriture hors chaîne). Deux événements par crash et par
    /// checkpoint — jamais par record.
    fn reserver_dans_la_chaine(
        &self,
        guard: &mut WriterInner,
        reg: &RegistreAppend,
        len: u64,
    ) -> SubstrateResult<(u64, bool)> {
        let mut chemin_froid = false;
        loop {
            // ── 1. LA GÉNÉRATION : L'ANNONCE D'UNE TRONCATURE ───────────────
            // Le checkpoint publie l'ancre (side-car ET registre), met les deux
            // têtes à 0, PUIS incrémente la génération — c'est elle qui publie les
            // autres (`Release`). La voir changer suffit donc à savoir que le
            // fichier a été tronqué : aucun `statx` n'est dû pour le croire.
            let generation = reg.generation().load(Ordering::Acquire);
            if generation != self.reg_generation_vue.load(Ordering::Acquire) {
                self.generations_vues.fetch_add(1, Ordering::Relaxed);
                self.rafraichir_ancre_durable();
                self.adopter_l_ancre_partagee(reg);
                self.reg_generation_vue.store(generation, Ordering::Release);
                // Notre vue du fichier est périmée : plus AUCUNE supposition.
                self.oublier_la_tete();
                chemin_froid = true;
            }
            // ── 2. LA CHAÎNE EST-ELLE QUIESCENTE, ET LA TÊTE CONNUE ? ───────
            // `tete_reservee == fin_publiee` : aucune réservation n'attend d'être
            // écrite, donc tout ce qui est réservé existe. Et si cette valeur est
            // celle que NOUS savons égale à la fin réelle (notre publication, ou une
            // mesure), alors la taille réelle EST la tête — sans le demander au
            // noyau.
            let tete = reg.tete_reservee().load(Ordering::Acquire);
            let publiee = reg.fin_publiee().load(Ordering::Acquire);
            if tete != publiee || publiee != self.reg_fin_connue.load(Ordering::Acquire) {
                chemin_froid = true;
            }
            // ── 3. RÉSERVER (le CAS rend l'offset) ──────────────────────────
            let debut = self.reserver_depuis(reg, tete, len);
            // ── 4. ATTENDRE QUE LE PRÉDÉCESSEUR AIT PUBLIÉ ──────────────────
            if self.attendre_la_publication(guard, reg, debut)? {
                // Notre `write` va publier `debut + len` : c'est donc cette valeur
                // qui sera « la fin réelle connue » pour notre prochain append — et
                // elle ne l'est qu'après un `write` COMPLET (voir la publication
                // sous le verrou, et la libération en cas d'échec).
                self.reg_fin_connue.store(debut + len, Ordering::Release);
                return Ok((debut, chemin_froid));
            }
            // La récupération a ré-ancré la chaîne : on réserve à nouveau (la
            // boucle reprend par la vérification de génération, qui est gratuite).
            chemin_froid = true;
        }
    }

    /// Le CAS de réservation, et sa boucle de reprise.
    ///
    /// ── POURQUOI LA BOUCLE N'EST PAS UNE ATTENTE ACTIVE ─────────────────────
    /// Un CAS qui échoue signifie qu'un pair a réservé entre notre lecture et
    /// nous : c'est un PROGRÈS (la chaîne a avancé), et la reprise repart de sa
    /// valeur — au-dessus de sa plage, jamais dedans. Le nombre de tours est borné
    /// par le nombre d'écrivains actifs, pas par une durée, et il ne coûte aucun
    /// appel système.
    fn reserver_depuis(&self, reg: &RegistreAppend, mut tete: u64, len: u64) -> u64 {
        loop {
            match reg.tete_reservee().compare_exchange(
                tete,
                tete + len,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return tete,
                Err(courant) => {
                    self.reprises_de_reservation.fetch_add(1, Ordering::Relaxed);
                    tete = courant;
                }
            }
        }
    }

    /// ── ATTENDRE QUE TOUT CE QUI EST AU-DESSOUS SOIT PUBLIÉ ─────────────────
    ///
    /// `O_APPEND` place notre `write` à la fin RÉELLE du fichier : pour que notre
    /// frame occupe EXACTEMENT la plage que notre LSN annonce, il faut que la fin
    /// réelle vaille déjà `debut`. C'est exactement `fin_publiee == debut` — un
    /// ATOMIQUE partagé, donc une attente qui ne coûte **aucun appel système** :
    /// c'est tout l'apport du mot `tete_reservee`/`fin_publiee` par rapport à un
    /// mot unique (mesuré : 24,55 % de LSN inexacts sans lui).
    ///
    /// Rend `true` si la publication est acquise (on peut écrire), `false` si la
    /// chaîne a été ré-ancrée (il faut réserver à nouveau).
    ///
    /// ── LES DEUX SEules ISSUES ANORMALES ────────────────────────────────────
    /// * `fin_publiee > debut` : des octets publiés AU-DELÀ de notre début signifient
    ///   qu'un pair a écrit hors chaîne (version antérieure, ou chaîne ré-ancrée) :
    ///   la position n'est plus certaine, on MESURE et on répare ;
    /// * le délai [`DELAI_TETE_BLOQUEE`] expire : le prédécesseur est mort entre sa
    ///   réservation et son `write`. On MESURE (un `statx`) et on ré-ancre les deux
    ///   têtes sur la taille réelle — sa plage devient un TROU DANS LES LSN, jamais
    ///   un trou de zéros dans le fichier.
    fn attendre_la_publication(
        &self,
        guard: &mut WriterInner,
        reg: &RegistreAppend,
        debut: u64,
    ) -> SubstrateResult<bool> {
        let depart = Instant::now();
        let delai = Duration::from_nanos(self.delai_tete_bloquee_ns.load(Ordering::Acquire));
        let generation_vue = self.reg_generation_vue.load(Ordering::Acquire);
        let mut premier_tour = true;
        loop {
            let publiee = reg.fin_publiee().load(Ordering::Acquire);
            if publiee == debut {
                return Ok(true);
            }
            // ── LA TRONCATURE EST ANNONCÉE : ON NE L'ATTEND PAS ─────────────
            // `generation` change quand un checkpoint publie une ancre neuve et
            // tronque. Notre réservation appartient alors au segment PRÉCÉDENT :
            // personne ne publiera jamais sa fin, et attendre le délai de
            // récupération serait attendre pour rien (mesuré : la rafale de
            // `mw_interprocess_phase_b::t7` passait de quelques centaines de
            // millisecondes à un blocage, un checkpoint par append payant 25 ms).
            // Le délai, lui, garde tout son sens pour ce qu'il sait distinguer : un
            // prédécesseur MORT d'un prédécesseur PRÉEMPTÉ.
            if reg.generation().load(Ordering::Acquire) != generation_vue {
                let taille = guard.taille_reelle()?;
                tracing::warn!(
                    debut_reserve = debut,
                    fin_publiee = publiee,
                    taille_reelle = taille,
                    "WAL partagé : un checkpoint a tronqué le WAL pendant notre attente (génération                      changée) — la chaîne est ré-ancrée sur la taille réelle SANS délai, et la                      réservation est reprise dans le nouveau segment"
                );
                self.reancrer_la_chaine(reg, taille);
                self.rafraichir_ancre_durable();
                self.adopter_l_ancre_partagee(reg);
                self.reg_generation_vue
                    .store(reg.generation().load(Ordering::Acquire), Ordering::Release);
                return Ok(false);
            }
            if publiee > debut {
                // ── DES OCTETS EXISTENT AU-DELÀ DE NOTRE DÉBUT ───────────────
                // Un écrivain a publié une plage que la chaîne ne lui avait pas
                // donnée (version antérieure, ou tête ré-ancrée par une
                // récupération). La chaîne ne dit plus la vérité : c'est le FICHIER
                // qui fait foi, et il faut le MESURER (un `statx` — chemin
                // d'anomalie, donc rare).
                self.tetes_reancrees.fetch_add(1, Ordering::Relaxed);
                let taille = guard.taille_reelle()?;
                tracing::warn!(
                    debut_reserve = debut,
                    fin_publiee = publiee,
                    taille_reelle = taille,
                    "WAL partagé : des octets sont PUBLIÉS au-delà de notre réservation (écriture                      hors chaîne) — les deux têtes sont ré-ancrées sur la taille réelle, et la                      réservation est reprise"
                );
                self.reancrer_la_chaine(reg, taille);
                let _ = self.reancrer_si_troncature(guard, reg)?;
                return Ok(false);
            }
            // ── LE PRÉDÉCESSEUR N'A PAS ENCORE PUBLIÉ ───────────────────────
            if depart.elapsed() >= delai {
                // Tête ABANDONNÉE : personne ne publiera jamais cette plage (son
                // détenteur est mort), donc attendre plus longtemps serait attendre
                // indéfiniment. On mesure, et on ré-ancre.
                self.tetes_reancrees.fetch_add(1, Ordering::Relaxed);
                let taille = guard.taille_reelle()?;
                tracing::warn!(
                    debut_reserve = debut,
                    fin_publiee = publiee,
                    taille_reelle = taille,
                    delai_ms = delai.as_millis(),
                    "WAL partagé : une réservation est restée ABANDONNÉE (un écrivain est mort                      entre sa réservation et son write) — les deux têtes sont ré-ancrées sur la                      taille réelle ; sa plage devient un TROU DANS LES LSN, jamais un trou de                      zéros dans le fichier"
                );
                self.reancrer_la_chaine(reg, taille);
                if self.reancrer_si_troncature(guard, reg)?.is_none() {
                    // Pas une troncature : l'ancre a pu être publiée par un checkpoint
                    // de pair (side-car) sans que le fichier ait été tronqué — on la
                    // relit avant d'estampiller quoi que ce soit.
                    self.rafraichir_ancre_durable();
                    self.adopter_l_ancre_partagee(reg);
                }
                return Ok(false);
            }
            if premier_tour {
                self.attentes_de_publication.fetch_add(1, Ordering::Relaxed);
                premier_tour = false;
            }
            if depart.elapsed() < ATTENTE_ACTIVE {
                // Attente ACTIVE : le prédécesseur publie en ~1 µs (le temps d'un
                // `write`), donc la quasi-totalité des attentes se terminent ici.
                std::hint::spin_loop();
            } else {
                // Au-delà, on rend le processeur : un prédécesseur préempté ne doit
                // pas transformer les autres écrivains en brûleurs de cœur.
                std::thread::yield_now();
            }
        }
    }

    /// ── UN RÉ-ANCRAGE EST UN ÉVÉNEMENT DE CHAÎNE : ON L'ANNONCE ─────────────
    ///
    /// La génération est ce que les pairs lisent sans `statx` pour savoir que le
    /// monde a changé. Un ré-ancrage local qui ne l'incrémenterait pas laisserait
    /// leurs vues locales (`reg_fin_connue`) périmées — chacun d'eux devrait alors
    /// découvrir l'anomalie par sa propre mesure, c'est-à-dire par une tempête de
    /// `statx`. L'annonce, elle, coûte un `fetch_add`.
    fn annoncer_reancrage(&self, reg: &RegistreAppend) {
        let generation = reg.generation().fetch_add(1, Ordering::AcqRel) + 1;
        self.reg_generation_vue.store(generation, Ordering::Release);
    }

    /// ── LA SEULE PREUVE DE TRONCATURE QUE LA CHAÎNE PUISSE OBTENIR ──────────
    /// `fin_publiee > taille_réelle` : des octets ont été PUBLIÉS — donc ils
    /// existaient — et le fichier est maintenant plus court. Aucun écrivain conforme
    /// ne publie des octets qui n'existent pas : c'est une TRONCATURE, et elle n'a
    /// pas été annoncée par la génération (sinon on ne serait pas en train de la
    /// découvrir).
    ///
    /// ── POURQUOI LA MESURE EST REPRISE PLUSIEURS FOIS (défaut MESURÉ) ───────
    /// Une taille mesurée est PÉRIMÉE dès qu'un pair publie : mesurer 4 000 octets
    /// puis constater que la fin publiée vaut 4 500 ne prouve RIEN — le pair a
    /// simplement écrit entre les deux. La première version concluait pourtant à la
    /// troncature, et RECULAIT la chaîne : mesuré sur
    /// `wal_chaine_cas::t1_quatre_processus_lsn_exact_et_ordre_physique`, **5 966
    /// frames sur 6 000 portaient un LSN faux**, décalées de la longueur des plages
    /// écrasées, et l'ordre physique cessait de suivre l'ordre des LSN. Une
    /// troncature, elle, est STABLE : la taille ne rattrape jamais la fin publiée.
    /// On redemande donc la taille tant qu'elle progresse, et on ne conclut que si
    /// elle reste en dessous.
    ///
    /// Rend `Some(taille)` si la troncature est AVÉRÉE (la chaîne a été ré-ancrée et
    /// l'événement annoncé), `None` sinon.
    fn reancrer_si_troncature(
        &self,
        guard: &mut WriterInner,
        reg: &RegistreAppend,
    ) -> SubstrateResult<Option<u64>> {
        /// Trois mesures : la première peut être en retard d'une écriture de pair, la
        /// deuxième de quelques-unes ; au-delà, seule une troncature explique qu'elles
        /// restent TOUTES sous la fin publiée.
        const MESURES: u32 = 3;
        let mut taille = guard.taille_reelle()?;
        let mut tours = 0u32;
        while reg.fin_publiee().load(Ordering::Acquire) > taille && tours < MESURES {
            std::thread::yield_now();
            taille = guard.taille_reelle()?;
            tours += 1;
        }
        if reg.fin_publiee().load(Ordering::Acquire) <= taille {
            return Ok(None);
        }
        tracing::warn!(
            fin_publiee = reg.fin_publiee().load(Ordering::Acquire),
            taille_reelle = taille,
            mesures = tours + 1,
            "WAL partagé : des octets PUBLIÉS ont disparu du fichier (troncature NON annoncée \
             par la génération) — les deux têtes sont ré-ancrées sur la taille réelle et \
             l'événement est annoncé aux pairs"
        );
        self.reancrer_la_chaine(reg, taille);
        self.annoncer_reancrage(reg);
        Ok(Some(taille))
    }

    /// ── RE-ANCRAGE DE LA CHAÎNE SUR UNE TAILLE MESURÉE ──────────────────────
    ///
    /// Deux usages, et un invariant :
    /// * une TRONCATURE avérée ([`WalWriter::reancrer_si_troncature`]) : la fin
    ///   publiée est ramenée à la taille réelle, et la tête avec elle ;
    /// * une RÉSERVATION ABANDONNÉE ou une écriture hors chaîne (voir
    ///   [`WalWriter::attendre_la_publication`]) : la tête est ramenée à la taille
    ///   réelle — ce qui LIBÈRE la plage du détenteur mort — tandis que la fin
    ///   publiée, elle, ne bouge que si elle la dépasse.
    ///
    /// L'INVARIANT est `tete_reservee >= fin_publiee`, et il est vrai par
    /// construction : la fin publiée ne décrit que des octets ÉCRITS, la tête décrit
    /// la fin des octets écrits **ou réservés**. Une tête plus basse que la fin
    /// publiée donnerait à un pair une plage qui contient déjà des octets.
    ///
    /// ── POURQUOI UN CAS, ET PAS LE `store` NU DU DESIGN ─────────────────────
    /// Un store NU écraserait une réservation qu'un pair vient de prendre ENTRE
    /// notre mesure et cette ligne — c'est-à-dire exactement la plage que le
    /// ré-ancrage prétend récupérer (c'est le défaut mesuré plus haut : 5 966 frames
    /// sur 6 000). On tente donc un CAS DEPUIS LA VALEUR OBSERVÉE : si un pair a
    /// réservé, le CAS échoue, on relit, on recommence — sa réservation survit.
    ///
    /// ── ET POURQUOI LE `store` RESTE, EN DERNIER RECOURS ────────────────────
    /// Une tête abandonnée ne se libère JAMAIS toute seule : personne ne publiera la
    /// plage d'un mort. Un CAS qui échouerait indéfiniment (parce que des pairs
    /// réservent en boucle depuis la tête périmée) transformerait la récupération en
    /// blocage définitif. Après `ESSAIS_REANCRAGE` tours, on FORCE donc le store :
    /// c'est le seul chemin du chantier qui écrase une réservation, il est borné,
    /// compté (`tetes_reancrees`) et TRACÉ.
    fn reancrer_la_chaine(&self, reg: &RegistreAppend, taille: u64) {
        const ESSAIS_REANCRAGE: u32 = 64;
        // ── LA FIN PUBLIÉE EST REPOSÉE SUR LA MESURE, DANS LES DEUX SENS ────────
        // C'est ce que dit le design (`fin_publiee.store(taille_reelle)`), et le
        // premier jet ne le faisait qu'à la BAISSE — ce qui a produit un BLOCAGE
        // DÉFINITIF, observé sur `mw_interprocess_phase_b::t7` (80 checkpoints
        // concurrents) : état du registre au moment du blocage, relevé sur le
        // fichier — `fin_publiee = 0`, `tete_reservee = 416 776`, WAL = 416 664
        // octets. La fin publiée, restée à 0, ne pouvait plus JAMAIS rattraper la
        // taille réelle (les écrivains qui auraient dû la faire avancer voyaient
        // `fin_publiee != position` à la publication et la SAUTAIENT, par prudence),
        // donc tous les appends attendaient 25 ms puis récupéraient en boucle.
        //
        // Relever la fin sur la taille MESURÉE est vrai, et c'est ce qui débloque :
        // ces octets EXISTENT (on vient de les compter). La chaîne repart donc de la
        // taille réelle, et tout ce qui était réservé au-dessus est libéré — c'est
        // le prix, assumé, d'un chemin d'ANOMALIE.
        reg.fin_publiee().store(taille, Ordering::Release);
        let cible = taille;
        for _ in 0..ESSAIS_REANCRAGE {
            let courant = reg.tete_reservee().load(Ordering::Acquire);
            if courant == cible {
                break;
            }
            if reg
                .tete_reservee()
                .compare_exchange(courant, cible, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break;
            }
        }
        if reg.tete_reservee().load(Ordering::Acquire) != cible {
            tracing::warn!(
                tete_viser = cible,
                "WAL partagé : des pairs réservaient SANS ARRÊT depuis une tête périmée — la tête \
                 est forcée sur la taille réelle (dernier recours borné du ré-ancrage)"
            );
            reg.tete_reservee().store(cible, Ordering::Release);
        }
        self.reg_fin_connue.store(cible, Ordering::Release);
    }

    /// Relâche NOTRE réservation quand l'écriture a ÉCHOUÉ (0 octet, ou écriture
    /// courte annulée).
    ///
    /// POURQUOI C'EST NÉCESSAIRE, ET PAS SEULEMENT PROPRE : une plage réservée dont
    /// les octets n'existent pas est indistinguable, pour les autres écrivains, de
    /// la plage d'un écrivain MORT — ils attendraient donc le délai de récupération
    /// complet avant de pouvoir écrire, à chaque disque plein. Le CAS n'écrase QUE
    /// notre propre réservation (si un pair est passé au-dessus, sa valeur reste).
    fn relacher_la_reservation(&self, reg: &RegistreAppend, nôtre: u64, position: u64) {
        let _ = reg.tete_reservee().compare_exchange(
            nôtre,
            position,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        self.reg_fin_connue.store(position, Ordering::Release);
    }

    /// Oublie la tête de chaîne : notre vue locale n'est plus fiable (une
    /// troncature est passée, ou notre frame a survécu ailleurs que prévu).
    ///
    /// ── POURQUOI OUBLIER PLUTÔT QUE RÉ-ANCRER (défaut MESURÉ) ───────────────
    /// Une première version appelait ici `reancrer_la_chaine(reg, taille_observée)`.
    /// Or `taille_observée` est la taille du FICHIER, pas la fin des réservations :
    /// dans la fenêtre où un pair a réservé sans avoir encore écrit, la tête est
    /// LÉGITIMEMENT en avance sur le fichier — et l'écraser « au nom de la mesure »
    /// détruisait la réservation du pair. Sa frame partait alors à un offset qui
    /// n'était pas celui de son LSN : exactement l'inexactitude que la chaîne
    /// existe pour interdire.
    ///
    /// Oublier est STRICTEMENT plus sûr : le prochain append repasse par la
    /// vérification de quiescence et par l'attente de publication, qui VÉRIFIENT la
    /// chaîne partagée au lieu de la remplacer. La chaîne n'est reculée que par
    /// [`WalWriter::reancrer_la_chaine`], sur une mesure prise dans un chemin
    /// d'anomalie.
    fn oublier_la_tete(&self) {
        // `u64::MAX` est une sentinelle IMPOSSIBLE : elle ne peut égaler aucune
        // taille de fichier, donc la vérification de quiescence échoue à coup sûr et
        // le prochain append ne suppose rien.
        self.reg_fin_connue.store(u64::MAX, Ordering::Release);
    }

    /// Adopte l'ancre partagée : publie la nôtre si elle est plus avancée, et lit
    /// l'ancre des pairs sinon.
    ///
    /// Les deux sens en UN `fetch_max` : une ancre ne recule JAMAIS (c'est la
    /// règle qui interdit de réémettre un LSN), donc `max` est la seule
    /// combinaison correcte — et un checkpoint d'un pair qui avance l'ancre est
    /// ainsi vu par tous les écrivains, même ceux qui ne lisent jamais le
    /// side-car.
    fn adopter_l_ancre_partagee(&self, reg: &RegistreAppend) {
        let nôtre = self.ancre.load(Ordering::Acquire);
        let publiee = reg.ancre_partagee().fetch_max(nôtre, Ordering::AcqRel);
        if publiee > nôtre {
            self.ancre.store(publiee, Ordering::Release);
        }
    }

    /// Recale le compteur d'offset sur la taille RÉELLE du fichier, et traite les
    /// deux écarts possibles.
    ///
    /// ── C'EST LE CHEMIN DE REPLI, ET C'EST LA NON-RÉGRESSION ────────────────
    /// Depuis la pièce 7, la position ne vient plus d'ici mais d'une RÉSERVATION
    /// dans le registre partagé. Il ne reste appelé que quand le registre est
    /// INDISPONIBLE (`/dev/full`, quota disque, répertoire en lecture seule,
    /// wasm32) : on reprend alors le comportement d'avant, à la lettre — deux
    /// `statx` par record et un LSN qui peut n'être qu'une borne inférieure. Le
    /// registre est une optimisation de COÛT, jamais une condition de correction.
    fn caler_sur_la_fin(&self, guard: &mut WriterInner, taille: u64) -> SubstrateResult<u64> {
        let attendu = self.offset.load(Ordering::Acquire);
        if taille == attendu {
            return Ok(taille);
        }
        if taille > attendu {
            // Un second écrivain a avancé le fichier : rien d'anormal, on se
            // recale — et on allume la surveillance des courses, parce qu'un
            // écrivain qui en voit un autre peut désormais le croiser ENTRE notre
            // lecture et notre write.
            self.recalages.fetch_add(1, Ordering::Relaxed);
            guard.surveiller_les_courses = true;
        } else {
            // ── TRONCATURE ÉTRANGÈRE ─────────────────────────────────────────
            // Le fichier a DIMINUÉ. Écrire à `attendu` produirait un LSN calculé
            // sur un offset qui n'existe plus (et `O_APPEND` poserait la frame
            // ailleurs) : la suite du fichier porterait des LSN en désordre. On
            // relit l'ancre, car un checkpoint ÉLU a pu l'avancer AVANT de
            // tronquer — et c'est cette ancre-là qui est désormais la bonne.
            self.rafraichir_ancre_durable();
            guard.reconciliation = Some(Reconciliation::TroncatureEtrangere {
                ancre: self.ancre.load(Ordering::Acquire),
                plancher_attendu: attendu,
                taille_reelle: taille,
            });
            tracing::warn!(
                attendu,
                taille_reelle = taille,
                ancre = self.ancre.load(Ordering::Acquire),
                "WAL : la taille réelle du fichier a DIMINUÉ (troncature étrangère ou \
                 checkpoint d'un autre écrivain) — offset recalé sur la fin réelle, ancre \
                 relue du side-car ; rien n'est écrit à l'aveugle"
            );
        }
        self.offset.store(taille, Ordering::Release);
        // Le repli n'a pas de chaîne, donc pas de tête à tenir : on garde la vue
        // locale cohérente avec la mesure, au cas où le registre deviendrait
        // disponible plus tard (les tentatives de création sont bornées).
        self.reg_fin_connue.store(taille, Ordering::Release);
        Ok(taille)
    }

    /// Force a flush + fsync now, regardless of sync mode. Returns after the
    /// fsync completes; callers that set [`SyncMode::Group`] should invoke this
    /// from a background timer or at explicit commit points.
    pub fn fsync(&self) -> SubstrateResult<()> {
        let mut guard = self.inner.lock();
        // ── SOUS GROUP COMMIT, UN `fsync` DOIT D'ABORD POSER LE TAMPON ───────
        // Sinon les frames en attente ne seraient pas dans le fichier, et le
        // `fsync` qui suit ne rendrait durable que ce qui y est déjà : l'appelant
        // croirait avoir fermé la fenêtre de perte et ne l'aurait pas fermée.
        // `DurableToujours` fsynce même sur un tampon vide : c'est le contrat du
        // mode `Group` (les frames partent sans fsync, l'appelant décide quand).
        self.poser_le_tampon(&mut guard, PoseTampon::DurableToujours)
    }

    /// ── TRONCATURE ORDONNÉE (pièce 4) : L'ANCRE AVANT LA TRONCATURE ─────────
    ///
    /// Truncate the WAL to zero bytes, after having durably ADVANCED the LSN
    /// anchor by the size of the segment being dropped. Returns the new anchor.
    ///
    /// ── POURQUOI L'ORDRE EST DANS CETTE FONCTION ET PAS CHEZ L'APPELANT ─────
    /// Tronquer d'abord ferait repartir les positions à 0 avec l'ANCIENNE ancre :
    /// les LSN du nouveau segment RECOUVRERAIENT ceux du précédent, et deux
    /// enregistrements distincts porteraient le même LSN — exactement ce que
    /// l'ancre existe pour empêcher. En publiant l'ancre ICI, avant `set_len`,
    /// aucun appelant ne peut inverser l'ordre ni l'oublier.
    ///
    /// Le plancher est publié à 0 : il décrit un segment qui n'existe pas encore
    /// et que rien ne peut avoir tronqué. [`WalWriter::sceller_segment`] le
    /// relève une fois le marqueur de checkpoint posé.
    /// ── PHASE B, PIÈCE 1 : LA BASE DE DÉPART EST L'ANCRE DURABLE, PAS LA NÔTRE ─
    ///
    /// ── LE DÉFAUT MESURÉ (2026-10-…) : L'ANCRE RECULAIT ────────────────────
    /// Cette fonction calculait `nouvelle = notre_ancre + taille`. « Notre ancre »
    /// est celle de CE `WalWriter` : chargée à l'OUVERTURE, rafraîchie seulement
    /// quand une anomalie le force. Un second processus dont le `WalWriter` est
    /// ouvert depuis un moment — le cas NORMAL d'un store multi-écrivains — porte
    /// donc une ancre ANTÉRIEURE au dernier checkpoint d'un pair. Son `truncate`
    /// publiait `ancre_périmée + petite_taille`, c'est-à-dire une valeur PLUS
    /// PETITE que celle déjà publiée : **l'ancre RECULAIT**.
    ///
    /// La conséquence est exactement celle que l'ancre existe pour empêcher : les
    /// LSN du nouveau segment RECOUVRENT ceux du segment abandonné. Mesuré (test
    /// `t6_deux_checkpoints_de_deux_processus_ne_font_pas_reculer_l_ancre`) :
    /// WAL de 100 trames → premier checkpoint à ancre 3401 → second checkpoint
    /// (autre processus) replubliant l'ancre 41. Toute reprise qui trie ou compare
    /// des LSN devenait alors menteuse, et `at_lsn` — le LSN gravé dans le
    /// marqueur — désignait une époque déjà révolue.
    ///
    /// ── POURQUOI LE MAXIMUM, ET PAS SIMPLEMENT « RELIRE » ──────────────────
    /// Le côté froid (un checkpoint) peut payer une lecture du side-car. Mais une
    /// lecture seule ne suffit pas : pendant un `rename` de publication, le
    /// side-car peut être illisible un instant, et retomber sur notre valeur
    /// périmée reproduirait le défaut. On prend donc le MAXIMUM de ce que le
    /// disque publie et de ce que nous savons — jamais en arrière. C'est la même
    /// règle que `fetch_max` sur les filigranes d'allocation : une borne ne recule
    /// pas.
    pub fn truncate(&self) -> SubstrateResult<u64> {
        let mut guard = self.inner.lock();
        // ── L'ANCRE COMPTE LA TAILLE RÉELLE : LE TAMPON DOIT Y ÊTRE ──────────
        // `nouvelle = base + taille`. Si des frames attendaient dans le tampon du
        // group commit, `taille` ne les compterait pas et l'ancre avancerait trop
        // PEU : les LSN de ces frames seraient RÉÉMIS dans le segment suivant,
        // c'est-à-dire exactement le doublon que l'ancre existe pour empêcher. On
        // les pose donc d'abord — sans `fsync`, puisque le segment part à la
        // troncature juste après.
        self.poser_le_tampon(&mut guard, PoseTampon::VidageSansFsync)?;
        let taille = guard.taille_reelle()?;
        let notre = self.ancre.load(Ordering::Acquire);
        let durable = lire_ancre_sur(&self.ancre_path).map_or(0, |a| a.ancre);
        let base = notre.max(durable);
        let nouvelle = base.saturating_add(taille);
        if matches!(guard.medium, WalMedium::File(_)) {
            publier_ancre_sur(&self.ancre_path, nouvelle, 0)?;
        }
        guard.tronquer()?;
        self.ancre.store(nouvelle, Ordering::Release);
        self.offset.store(0, Ordering::Release);
        // ── LA TRONCATURE EST ANNONCÉE (pièce 7) ────────────────────────────
        // Les pairs doivent savoir, SANS `statx`, que le fichier a été tronqué et
        // que l'ancre a changé : c'est exactement ce que porte `generation`. Trois
        // écritures d'atomes, dans CET ordre :
        //   1. la TÊTE à 0 (un pair qui la voit au moment où il allait réserver
        //      repart du début, sans attendre le délai de récupération) ;
        //   2. l'ANCRE partagée (le nouveau segment a une base neuve) ;
        //   3. la GÉNÉRATION, en DERNIER et en `Release` : c'est elle qui PUBLIE
        //      les deux autres — un pair qui la voit changée (Acquire) voit la
        //      tête et l'ancre qui vont avec.
        //
        // ── POURQUOI ON CRÉE LE REGISTRE ICI SI BESOIN ──────────────────────
        // Un checkpointeur qui n'a encore jamais appendé (le cas du processus qui ne
        // fait que ça) doit quand même annoncer sa troncature : sans registre, ses
        // pairs garderaient une tête périmée et écriraient à un offset qui n'existe
        // plus — la frame serait estampillée sous la base du segment courant. On le
        // crée donc au premier checkpoint d'un écrivain PARTAGÉ (`Aucune` n'a pas de
        // pair, et ne doit pas imposer ce fichier). C'est un chemin FROID : un
        // checkpoint par 8 Mio, et une seule création.
        if guard.coordination != CoordinationAppend::Aucune
            && matches!(guard.medium, WalMedium::File(_))
        {
            if let Some(reg) = self.registre(&mut guard) {
                reg.fin_publiee().store(0, Ordering::Release);
                reg.ancre_partagee().fetch_max(nouvelle, Ordering::AcqRel);
                let generation = reg.generation().fetch_add(1, Ordering::AcqRel) + 1;
                self.reg_generation_vue.store(generation, Ordering::Release);
                self.reg_fin_connue.store(0, Ordering::Release);
            }
        }
        Ok(nouvelle)
    }

    /// Relève le PLANCHER du segment courant à sa taille réelle.
    ///
    /// À appeler une fois le marqueur de checkpoint écrit : c'est ce plancher qui
    /// rend une troncature étrangère DÉTECTABLE à la réouverture (un fichier plus
    /// court que son plancher a été tronqué par quelqu'un d'autre).
    pub fn sceller_segment(&self) -> SubstrateResult<()> {
        let mut guard = self.inner.lock();
        if !matches!(guard.medium, WalMedium::File(_)) {
            return Ok(());
        }
        let taille = guard.taille_reelle()?;
        publier_ancre_sur(&self.ancre_path, self.ancre.load(Ordering::Acquire), taille)
    }

    /// ── RÉPARATION DE QUEUE (T4) ────────────────────────────────────────────
    ///
    /// Ramène le fichier au dernier BORD DE FRAME valide quand la queue est
    /// déchirée (troncature au milieu d'une frame, écriture en vol, arrêt
    /// brutal). Rend `None` si rien n'était à réparer.
    ///
    /// ── CE QU'ELLE REFUSE DE FAIRE, ET POURQUOI ────────────────────────────
    /// Elle ne tronque JAMAIS sur un TROU de zéros ni sur une corruption : dans
    /// les deux cas il peut y avoir des frames VALIDES après le défaut, et les
    /// effacer serait une perte silencieuse — le contraire exact du contrat. Un
    /// trou remonte donc en [`SubstrateError::WalTrou`], une corruption en erreur
    /// de décodage, et l'appelant décide.
    pub fn reparer_queue(&self) -> SubstrateResult<Option<ReparationQueue>> {
        let mut guard = self.inner.lock();
        let taille = guard.taille_reelle()?;
        if taille == 0 || !matches!(guard.medium, WalMedium::File(_)) {
            guard.queue_a_repairer = None;
            return Ok(None);
        }
        let reader = WalReader::open(&self.path)?;
        let mut dernier_bon: u64 = 0;
        let mut iter = reader.iter_from(0);
        let mut trou = None;
        for item in iter.by_ref() {
            match item {
                Ok((_, off, len)) => dernier_bon = off + len as u64,
                Err(e @ SubstrateError::WalTrou { .. }) => {
                    trou = Some(e);
                    break;
                }
                // Corruption franche : on s'arrête et on refuse de trancher.
                Err(e) => return Err(e),
            }
        }
        if let Some(e) = trou {
            return Err(e);
        }
        if !iter.queue_incomplete() {
            // Queue propre (ou zéros de pré-allocation) : rien à faire.
            guard.queue_a_repairer = None;
            return Ok(None);
        }
        let avant = taille;
        guard.annuler_jusqua(dernier_bon)?;
        self.offset.store(dernier_bon, Ordering::Release);
        // ── LA RÉPARATION EST UNE TRONCATURE : ELLE S'ANNONCE ────────────────
        // Elle retire des octets au fichier, donc toute réservation au-delà de
        // `dernier_bon` désigne un offset qui n'existe plus. Les pairs doivent le
        // savoir : `generation` + tête recalée, exactement comme `truncate` (on ne
        // crée pas le registre ici : une réparation de queue est un chemin
        // d'anomalie, elle n'a pas à imposer un fichier à un store mono-écrivain).
        if let Some(reg) = self.registre_existant() {
            reg.fin_publiee().store(dernier_bon, Ordering::Release);
            let generation = reg.generation().fetch_add(1, Ordering::AcqRel) + 1;
            self.reg_generation_vue.store(generation, Ordering::Release);
            self.reg_fin_connue.store(dernier_bon, Ordering::Release);
        }
        publier_ancre_sur(
            &self.ancre_path,
            self.ancre.load(Ordering::Acquire),
            dernier_bon,
        )?;
        guard.queue_a_repairer = None;
        Ok(Some(ReparationQueue {
            avant,
            apres: dernier_bon,
            octets_retires: avant - dernier_bon,
        }))
    }

    /// Path of the WAL file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Chemin du side-car d'ancre de LSN (`<wal>.lsn`).
    pub fn ancre_path(&self) -> PathBuf {
        self.ancre_path.clone()
    }

    /// Current sync mode.
    pub fn sync_mode(&self) -> SyncMode {
        self.sync_mode
    }
}

/// ── DROP : POSER CE QUI ATTEND (le dernier maillon de la fenêtre de perte) ───
///
/// Un `WalWriter` détruit avec un tampon non vide perdrait ses records SANS que
/// personne ne l'ait dit — et `append` les a pourtant acceptés. On les pose donc,
/// durablement, comme le ferait l'échéance de la needle. C'est aussi ce qui rend
/// les tests déterministes : un `WalWriter` lâché à la fin d'un test a bien écrit
/// tout ce qu'il a accepté.
///
/// Le veilleur est arrêté AVANT de prendre le mutex : sans cela il pourrait se
/// réarmer sur un état en cours de démontage (et, plus tard, poser un tampon sur
/// un écrivain à demi détruit). Il tient un `Weak`, donc il ne peut de toute
/// façon pas s'exécuter PENDANT ce `Drop` (le compte fort est déjà à zéro).
impl Drop for WalWriter {
    fn drop(&mut self) {
        {
            let mut v = self.veille.etat.lock();
            *v = Veille::Arrete;
            self.veille.reveil.notify_all();
        }
        let mut guard = self.inner.lock();
        if guard.groupe.vide() {
            return;
        }
        if let Err(e) = self.poser_le_tampon(&mut guard, PoseTampon::Durable) {
            // Un Drop ne peut pas rendre d'erreur : on la TRACE, au lieu de la
            // perdre. Le tampon, lui, n'est plus réessayable (l'écrivain
            // disparaît) — c'est la seule perte que ce chemin assume.
            tracing::error!(
                error = %e,
                "group commit : la pose du tampon au Drop a échoué — les records en attente                  sont perdus (ils avaient été acceptés par `append`)"
            );
        }
    }
}

/// Queue déchirée réparée : bornes AVANT/APRÈS et nombre d'octets retirés.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReparationQueue {
    pub avant: u64,
    pub apres: u64,
    pub octets_retires: u64,
}

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

/// Sequential reader over a WAL file.
///
/// The reader opens the file in read-only mode, reads the full content into a
/// buffer (WALs are expected to stay small between checkpoints — tens of MiB
/// max under Group commit policy), and exposes an iterator starting at a
/// given byte offset.
#[derive(Debug)]
pub struct WalReader {
    bytes: Vec<u8>,
}

impl WalReader {
    /// Open and read the WAL file at `path`. Returns an empty reader if the
    /// file does not exist.
    pub fn open(path: impl AsRef<Path>) -> SubstrateResult<Self> {
        let path = path.as_ref();
        let es = crate::es::espace();
        let bytes = if es.existe(path) {
            es.lire_tout(path)?
        } else {
            Vec::new()
        };
        Ok(Self { bytes })
    }

    /// Total byte length of the loaded WAL.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Iterate over the records starting at `offset`. Stops at the first
    /// decode error — this yields the crash boundary.
    ///
    /// Each item is `(record, offset, length)` where `offset` is where the
    /// record starts and `length` is its total encoded size (header + payload).
    pub fn iter_from(&self, offset: u64) -> WalIter<'_> {
        // (le drapeau `queue_incomplete` de l'itérateur rendu se lit avec
        // [`WalIter::queue_incomplete`] une fois l'itération terminée)
        WalIter {
            bytes: &self.bytes,
            cursor: offset as usize,
            stop_on_error: true,
            queue_incomplete: false,
            trou: None,
        }
    }
}

impl WalIter<'_> {
    /// L'itération s'est-elle arrêtée sur une queue incomplète ?
    ///
    /// À lire APRÈS l'itération : `true` = le fichier s'arrête au milieu d'un
    /// enregistrement (écriture en vol, ou crash). Ce n'est pas une corruption —
    /// c'est une frontière de crash, et elle doit se dire.
    #[must_use]
    pub fn queue_incomplete(&self) -> bool {
        self.queue_incomplete
    }

    /// Bornes du TROU rencontré, s'il y en a un : `(offset, longueur)`.
    /// Un trou n'est PAS une frontière de crash — c'est un défaut de contenu.
    #[must_use]
    pub fn trou(&self) -> Option<(u64, u64)> {
        self.trou
    }
}

/// Iterator over WAL records. Yields `(record, offset, length)` tuples.
pub struct WalIter<'a> {
    bytes: &'a [u8],
    cursor: usize,
    stop_on_error: bool,
    /// L'itération s'est-elle arrêtée sur une queue INCOMPLÈTE (écriture en vol
    /// ou arrêt brutal) plutôt que sur la fin propre du fichier ?
    ///
    /// ── POURQUOI CE DRAPEAU, ET PAS SEULEMENT UN `None` ─────────────────────
    /// Une queue incomplète n'est pas une corruption (voir [`Iterator::next`]) :
    /// l'itérateur s'arrête donc proprement. Mais l'INFORMATION, elle, ne doit
    /// pas disparaître pour autant : `ReplayStats::decode_errors` documente
    /// exactement « 1 si la queue du WAL a été déchirée par un crash », et
    /// `stopped_at` « pointe sur la frontière de crash ». Rendre la queue
    /// silencieuse aurait transformé une frontière de crash en fin de fichier
    /// ordinaire — une régression d'OBSERVABILITÉ, exactement ce que ce dépôt
    /// refuse (un défaut qui ne se dit nulle part est indiscernable d'une
    /// absence).
    queue_incomplete: bool,
    /// TROU rencontré : plage de zéros SUIVIE DE DONNÉES. Distinct de la queue
    /// incomplète — un trou n'est pas une frontière de crash, c'est un contenu
    /// qui ment sur ce qui suit.
    trou: Option<(u64, u64)>,
}

/// Un en-tête de frame entièrement nul n'est JAMAIS une frame valide : le champ
/// `length` d'un payload vaut au moins 4 (bincode en fixint écrit l'indice de
/// variante sur 32 bits) et le kind 0x00 n'existe pas (voir
/// [`crate::wal::WalKind`], qui commence à 0x01). C'est ce qui permet de
/// distinguer un TROU d'une frame.
fn debut_de_trou(octets: &[u8]) -> bool {
    octets.len() >= WAL_HEADER_SIZE
        && octets[0..4] == [0, 0, 0, 0]
        && octets[4] == 0
        && octets[24..28] == [0, 0, 0, 0]
}

impl WalIter<'_> {
    /// Saute un trou de zéros et reprend à la première trame VALIDE.
    ///
    /// La longueur est le premier champ de l'en-tête, en petit-boutiste : une
    /// trame dont la longueur a des octets de poids faible nuls (256, 65 536…)
    /// commence donc par 1 à 3 octets nuls. Le premier octet non nul peut ainsi
    /// être jusqu'à 3 octets APRÈS le vrai début : on essaie ces 4 positions,
    /// et on ne retient qu'un décodage dont le CRC tient.
    ///
    /// Zéros jusqu'à la fin : fin normale (fichier pré-alloué). Des octets non
    /// nuls sans aucune trame valide : on rend l'erreur — c'est une vraie
    /// frontière de crash, pas un trou.
    ///
    /// ── ELLE EST DE NOUVEAU APPELÉE (réunification 2026-10-06) ───────────────
    /// Voir la longue note de [`WalIter::next`] : la détection du trou vient de
    /// la pièce 6 (`debut_de_trou`), mais la REPRISE reste celle de la lignée
    /// servie — parce que `un_wal_deja_troue_se_relit_en_entier` la teste, et
    /// parce que les stores déjà troués par l'ère sans O_APPEND doivent se
    /// relire EN ENTIER. Les bornes du trou sont publiées séparément par
    /// [`WalIter::trou`].
    fn reprendre_apres_trou(&mut self) -> Option<SubstrateResult<(WalRecord, u64, usize)>> {
        let debut_trou = self.cursor;
        let Some(rel) = self.bytes[debut_trou..].iter().position(|b| *b != 0) else {
            self.cursor = self.bytes.len();
            return None;
        };
        let non_nul = debut_trou + rel;
        let mut derniere_erreur = None;
        for recul in (0..=3usize).rev() {
            let Some(cand) = non_nul.checked_sub(recul) else {
                continue;
            };
            if cand < debut_trou || self.bytes.len() - cand < WAL_HEADER_SIZE {
                continue;
            }
            match WalRecord::decode(&self.bytes[cand..]) {
                Ok((rec, used)) => {
                    tracing::warn!(
                        trou_debut = debut_trou,
                        trou_octets = cand - debut_trou,
                        reprise = cand,
                        "wal : trou de zéros sauté, lecture reprise à la trame valide suivante"
                    );
                    self.cursor = cand + used;
                    return Some(Ok((rec, cand as u64, used)));
                }
                Err(e) => derniere_erreur = Some(e),
            }
        }
        if self.stop_on_error {
            self.cursor = self.bytes.len();
        }
        derniere_erreur.map(Err)
    }
}

impl<'a> Iterator for WalIter<'a> {
    type Item = SubstrateResult<(WalRecord, u64, usize)>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor >= self.bytes.len() {
            return None;
        }
        // ── RESTE COURT : PRÉ-ALLOCATION OU ENREGISTREMENT COUPÉ ? ──────────
        // Un reste plus court qu'un en-tête existe dans DEUX cas opposés :
        //  · le fichier a été PRÉ-ALLOCÉ (zone de zéros en fin de fichier) :
        //    c'est une fin normale, et le rejeu ne doit rien signaler ;
        //  · un enregistrement a été COUPÉ (écriture en vol, arrêt brutal,
        //    troncature) : c'est une frontière de crash, et elle doit se dire.
        //
        // Le contenu tranche : des zéros = pré-allocation, autre chose = un
        // enregistrement interrompu. Sans cette distinction, la troncature était
        // SILENCIEUSE (le drapeau n'était posé que par le chemin `WalShortRead`,
        // jamais atteint quand il reste moins qu'un en-tête) — et
        // `ReplayStats::decode_errors`, qui documente « 1 si la queue a été
        // déchirée », restait à 0 sur une panne.
        if self.bytes.len() - self.cursor < WAL_HEADER_SIZE {
            let reste = &self.bytes[self.cursor..];
            if reste.iter().any(|o| *o != 0) {
                self.queue_incomplete = true;
            }
            return None;
        }
        let reste = &self.bytes[self.cursor..];
        // ── RÉUNIFICATION (2026-10-06) : DÉTECTER COMME LA PIÈCE 6, REPRENDRE
        //    COMME LA LIGNEÉ SERVIE ─────────────────────────────────────────────
        //
        // DEUX LIGNÉES, DEUX CONSTATS, ET ILS SONT COMPLÉMENTAIRES.
        //
        // · La lignée servie (2026-09-16, correctif d'incident) SAUTE le trou et
        //   reprend à la première trame dont le CRC tient
        //   (`reprendre_apres_trou`). C'est une GARANTIE DE PRODUCTION, et elle
        //   est PROUVÉE par `un_wal_deja_troue_se_relit_en_entier` : un WAL déjà
        //   troué — écrit par une version sans O_APPEND, cas RÉELLEMENT observé
        //   sur runtime-dev (7 964 288 octets nuls) — doit se relire EN ENTIER,
        //   les trames d'après le trou comprises. Les stores existants en
        //   dépendent : refuser de rejouer après un trou, c'est perdre du
        //   contenu qu'on sait lire.
        //
        // · Le multi-écrivain (pièce 6) a raison sur un point que la lignée
        //   servie ne nommait pas : un trou n'est PAS une frontière de crash, et
        //   le lecteur doit le DIRE au lieu de le confondre avec une fin de
        //   fichier. `debut_de_trou` distingue le trou de la pré-allocation, et
        //   `self.trou` / `WalIter::trou()` publient ses bornes.
        //
        // On garde donc LES DEUX : la détection et la publication du trou
        // (pièce 6), ET la reprise transparente (lignée servie). Le seul point
        // où la pièce 6 est écartée est celui où elle S'OPPOSE au test canonique
        // ci-dessus : elle rendait un `WalTrou` qui ARRÊTAIT la lecture, donc
        // laissait les trames d'après le trou non rejouées. `WalTrou` reste
        // défini et reste consommé par `reparer_queue` (qui refuse, à raison, de
        // tronquer sur un trou).
        //
        // Note : ce choix est ARBITRABLE. La pièce 6 est plus prudente (après
        // une troncature, les octets qui suivent le trou peuvent être d'ANCIENNES
        // trames à CRC valide). Il a été tranché ici en faveur de la lignée
        // SERVIE parce que sa garantie est testée, déployée, et porte sur des
        // données déjà sur disque.
        if debut_de_trou(reste) {
            let longueur_zero = reste.iter().position(|o| *o != 0).unwrap_or(reste.len());
            if self.cursor + longueur_zero >= self.bytes.len() {
                // Zéros JUSQU'À LA FIN : pré-allocation, fin normale — les deux
                // lignées disent exactement la même chose ici. (Ce cas couvre
                // aussi un en-tête nul de 32 octets et plus, que l'ancien code
                // rendait en ERREUR DE CRC — un fichier pré-alloué se signalait
                // donc comme corrompu.)
                self.cursor = self.bytes.len();
                return None;
            }
            // Le trou est NOMMÉ (bornes publiées par `WalIter::trou()`) — puis on
            // reprend à la première trame valide APRÈS lui.
            let offset = self.cursor as u64;
            let longueur = longueur_zero as u64;
            self.trou = Some((offset, longueur));
            return self.reprendre_apres_trou();
        }
        match WalRecord::decode(reste) {
            Ok((rec, used)) => {
                let at = self.cursor as u64;
                self.cursor += used;
                Some(Ok((rec, at, used)))
            }
            // ── UNE QUEUE INCOMPLÈTE N'EST PAS UNE CORRUPTION (2026-09-17) ──
            // `decode` reçoit `&bytes[cursor..]` : « il manque des octets »
            // signifie donc, PAR CONSTRUCTION, que le fichier s'arrête avant la
            // fin de l'enregistrement — une écriture en vol, ou un arrêt brutal.
            // C'est la frontière de crash, et le contrat de cet itérateur est de
            // s'arrêter PROPREMENT dessus.
            //
            // Défaut mesuré : il rendait `Some(Err(WalShortRead))`, si bien que
            // `multi_writer_wal::append_multithread_contre_reader_concurrent` —
            // dont le commentaire dit mot pour mot « une queue partielle, que
            // l'itérateur arrête proprement (comportement crash-boundary) » —
            // échouait 2 fois sur 12, en isolation, dès qu'un lecteur tombait au
            // milieu d'un `write` de l'écrivain. Le TEST avait raison ; c'est le
            // code qui ne tenait pas son contrat.
            //
            // Une VRAIE corruption (CRC invalide, octets réservés non nuls, type
            // inconnu) reste une erreur : elle vient d'un fichier complet dont le
            // contenu est faux, pas d'un fichier tronqué.
            Err(crate::SubstrateError::WalShortRead { .. }) => {
                self.cursor = self.bytes.len();
                // On s'arrête proprement, ET on le retient : l'appelant qui a
                // besoin de savoir (rejeu, audit, réparation) le lira.
                self.queue_incomplete = true;
                None
            }
            Err(e) => {
                if self.stop_on_error {
                    self.cursor = self.bytes.len(); // halt the iterator
                }
                Some(Err(e))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wal::WalPayload;
    use tempfile::tempdir;

    /// Le scénario MESURÉ sur runtime-dev : un processus tronque le WAL
    /// (checkpoint) pendant qu'un autre le tient ouvert ; ce dernier écrit
    /// ensuite. Avec O_APPEND, aucune zone de zéros ne doit apparaître, et
    /// tout ce qui a été écrit doit se relire.
    #[test]
    fn un_troncage_par_autrui_ne_laisse_aucun_trou() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("wal");
        let a = WalWriter::open(&p, SyncMode::Never, 1).unwrap();
        let b = WalWriter::open(&p, SyncMode::Never, 100).unwrap();
        for _ in 0..50 {
            a.append(sample(0, true)).unwrap();
        }
        // b a ouvert AVANT les écritures de a : sans O_APPEND, son curseur
        // resterait à 0 ; on fait avancer le sien aussi.
        for _ in 0..50 {
            b.append(sample(0, true)).unwrap();
        }
        // Le « checkpoint » d'un tiers : troncage à zéro.
        a.truncate().unwrap();
        // b, qui ne sait rien du troncage, écrit.
        let (off, _) = b.append(sample(0, true)).unwrap();
        assert_eq!(
            off, 0,
            "l'écriture après troncage doit tomber au DÉBUT, pas à l'ancien curseur"
        );
        let octets = std::fs::read(&p).unwrap();
        assert!(
            !octets[..WAL_HEADER_SIZE].iter().all(|x| *x == 0),
            "aucun en-tête nul"
        );
        let lus: Vec<_> = WalReader::open(&p).unwrap().iter_from(0).collect();
        assert_eq!(lus.len(), 1);
        assert!(lus[0].is_ok());
    }

    /// Un WAL DÉJÀ troué (écrit par une version sans O_APPEND) doit se relire
    /// en entier : le trou est sauté, les trames qui suivent sont rejouées.
    #[test]
    fn un_wal_deja_troue_se_relit_en_entier() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("wal");
        let w = WalWriter::open(&p, SyncMode::Never, 1).unwrap();
        w.append(sample(0, true)).unwrap();
        let tete = std::fs::read(&p).unwrap();
        let mut apres = Vec::new();
        for _ in 0..3 {
            let mut buf = Vec::new();
            sample(7, true).encode_into(&mut buf).unwrap();
            apres.extend(buf);
        }
        // La forme exacte observée : une trame, 1 Mo de zéros, puis des trames.
        let mut fichier = tete.clone();
        fichier.extend(std::iter::repeat_n(0u8, 1 << 20));
        fichier.extend(&apres);
        std::fs::write(&p, &fichier).unwrap();

        let lus: Vec<_> = WalReader::open(&p).unwrap().iter_from(0).collect();
        assert!(
            lus.iter().all(|r| r.is_ok()),
            "aucune erreur : un trou n'est pas un crash"
        );
        assert_eq!(lus.len(), 4, "la trame d'avant ET les trois d'après");
        let reprise = lus[1].as_ref().unwrap().1 as usize;
        assert_eq!(
            reprise,
            tete.len() + (1 << 20),
            "reprise exactement après le trou"
        );
    }

    /// Des zéros jusqu'à la fin : fin normale, pas une erreur.
    #[test]
    fn des_zeros_jusqu_a_la_fin_sont_une_fin_normale() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("wal");
        let w = WalWriter::open(&p, SyncMode::Never, 1).unwrap();
        w.append(sample(0, true)).unwrap();
        let mut f = std::fs::read(&p).unwrap();
        f.extend(std::iter::repeat_n(0u8, 4096));
        std::fs::write(&p, &f).unwrap();
        let lus: Vec<_> = WalReader::open(&p).unwrap().iter_from(0).collect();
        assert_eq!(lus.len(), 1);
        assert!(lus[0].is_ok());
    }

    /// Une trame ABÎMÉE (non nulle) reste une frontière de crash : le
    /// correctif ne doit rien changer à ce cas.
    #[test]
    fn une_trame_abimee_reste_une_frontiere() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("wal");
        let w = WalWriter::open(&p, SyncMode::Never, 1).unwrap();
        w.append(sample(0, true)).unwrap();
        w.append(sample(0, true)).unwrap();
        let mut f = std::fs::read(&p).unwrap();
        let n = f.len();
        f[n - 1] ^= 0xFF; // corrompt la charge utile de la deuxième trame
        std::fs::write(&p, &f).unwrap();
        let lus: Vec<_> = WalReader::open(&p).unwrap().iter_from(0).collect();
        assert_eq!(lus.len(), 2);
        assert!(lus[0].is_ok());
        assert!(lus[1].is_err(), "la trame abîmée est rapportée comme avant");
    }

    fn sample(lsn: u64, commit: bool) -> WalRecord {
        WalRecord {
            lsn,
            timestamp: 0,
            flags: if commit { WalRecord::FLAG_COMMIT } else { 0 },
            payload: WalPayload::NodeInsert {
                node_id: lsn as u32,
                label_bitset: lsn,
            },
        }
    }

    #[test]
    fn writer_appends_and_tracks_offset() {
        let dir = tempdir().unwrap();
        let wal = dir.path().join("w.wal");
        let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
        let (o1, l1) = w.append(sample(0, true)).unwrap();
        let (o2, l2) = w.append(sample(0, false)).unwrap();
        assert_eq!(o1, 0);
        assert_eq!(o2, l1 as u64);
        assert_eq!(w.offset(), (l1 + l2) as u64);
    }

    /// ── L'INVARIANT DE LA PIÈCE 2 : `lsn == ancre + position` ───────────────
    ///
    /// Ce test remplace l'ancienne assertion `rec.lsn == count`. Le LSN n'est
    /// PLUS un numéro d'ordre contigu : c'est l'offset physique de la frame,
    /// décalé par l'ancre. Ce qui compte — et ce qui est vérifié ici — c'est que
    /// l'ORDRE PHYSIQUE et l'ORDRE LSN coïncident, ce que l'ancien `fetch_add`
    /// ne garantissait pas (RFC multi-writer §4.1, constat mesuré).
    #[test]
    fn reader_iterates_writer_output() {
        let dir = tempdir().unwrap();
        let wal = dir.path().join("w.wal");
        let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
        for i in 1..=100u64 {
            w.append(sample(i, i % 10 == 0)).unwrap();
        }
        drop(w);

        let r = WalReader::open(&wal).unwrap();
        let mut count = 0u64;
        let mut last_offset = 0u64;
        let mut dernier_lsn = 0u64;
        for item in r.iter_from(0) {
            let (rec, off, len) = item.unwrap();
            assert!(off >= last_offset);
            assert!(len >= WAL_HEADER_SIZE);
            // `lsn = ancre + position` : la première frame d'un WAL neuf porte
            // le LSN 1 (ancre initiale 1, position 0).
            assert_eq!(rec.lsn, ANCRE_INITIALE + off, "lsn = ancre + position");
            assert!(rec.lsn > dernier_lsn, "l'ordre physique EST l'ordre LSN");
            dernier_lsn = rec.lsn;
            last_offset = off + len as u64;
            count += 1;
        }
        assert_eq!(count, 100);
    }

    #[test]
    fn reader_stops_at_crc_corruption() {
        let dir = tempdir().unwrap();
        let wal = dir.path().join("w.wal");
        let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
        for i in 1..=10u64 {
            w.append(sample(i, true)).unwrap();
        }
        drop(w);

        // ── ON CORROMPT LE CRC, PAS UN OCTET QUELCONQUE (2026-09-17) ──────
        // Ce test s'appelle « stops_at_crc_corruption » et flippait un octet au
        // MILIEU du fichier. Si cet octet tombe dans le champ LONGUEUR d'un
        // enregistrement, celui-ci réclame alors plus d'octets qu'il n'en existe,
        // et le lecteur ne peut PAS distinguer ce cas d'une queue tronquée : le
        // format lui-même ne le permet pas (la longueur est le seul repère, et on
        // ne peut pas vérifier un CRC sur des octets absents). Le test mesurait
        // donc autre chose que son nom, et il est tombé dès que l'itérateur a
        // cessé de crier « corruption » sur une queue incomplète — le
        // comportement que `multi_writer_wal` exige par ailleurs.
        //
        // On corrompt donc le CRC d'un enregistrement du milieu : là, tous les
        // octets sont présents, le décodage aboutit, et c'est bien la SOMME DE
        // CONTRÔLE qui doit faire échouer la lecture.
        let mut bytes = std::fs::read(&wal).unwrap();
        let idx = bytes.len() / 2;
        // Un enregistrement commence à son offset, avec le CRC en 24..28.
        // On cherche la frame qui contient `idx` : on repart du premier
        // enregistrement complet (`iter_from(0)`) pour trouver ses bornes.
        let bornes: Vec<(u64, usize)> = WalReader::open(&wal)
            .unwrap()
            .iter_from(0)
            .filter_map(|it| it.ok().map(|(_, at, used)| (at, used)))
            .collect();
        let (at, used) = bornes
            .iter()
            .copied()
            .find(|(at, used)| idx >= *at as usize && idx < *at as usize + used)
            .expect("l'octet choisi doit tomber dans un enregistrement");
        let crc_off = at as usize + 24;
        bytes[crc_off] ^= 0xFF;
        std::fs::write(&wal, bytes).unwrap();

        let r = WalReader::open(&wal).unwrap();
        let mut ok_count = 0;
        let mut saw_err = false;
        for item in r.iter_from(0) {
            match item {
                Ok(_) => ok_count += 1,
                Err(_) => {
                    saw_err = true;
                    break;
                }
            }
        }
        assert!(
            saw_err,
            "une somme de contrôle fausse doit être DITE (CRC corrompu)"
        );
        assert!(ok_count > 0 && ok_count < 10);
    }

    /// ── LE CAS QUE LE FORMAT NE SAIT PAS DISTINGUER, ET QUI SE DIT ──────────
    ///
    /// Corrompre le champ LONGUEUR produit exactement ce qu'une écriture en vol
    /// produit : un enregistrement qui réclame plus d'octets qu'il n'en existe.
    /// Le format n'a aucun moyen de trancher (le CRC ne vérifie que des octets
    /// présents). Le contrat retenu est donc celui de la frontière de crash :
    /// **on s'arrête proprement**, sans crier « corruption » — et c'est ce que ce
    /// test fixe, pour que la décision soit explicite plutôt que tacite.
    #[test]
    fn reader_stops_cleanly_when_a_length_field_is_absurd() {
        let dir = tempdir().unwrap();
        let wal = dir.path().join("w.wal");
        let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
        for i in 1..=10u64 {
            w.append(sample(i, true)).unwrap();
        }
        drop(w);

        let mut bytes = std::fs::read(&wal).unwrap();
        // Le premier enregistrement commence à l'offset 0 : on gonfle sa longueur.
        bytes[0..4].copy_from_slice(&u32::MAX.to_le_bytes());
        std::fs::write(&wal, bytes).unwrap();

        let r = WalReader::open(&wal).unwrap();
        let items: Vec<_> = r.iter_from(0).collect();
        assert_eq!(
            items.len(),
            0,
            "le premier enregistrement réclame plus que le fichier : la lecture \
             s'arrête là, proprement — elle ne rend ni erreur ni record"
        );
    }

    #[test]
    fn fsync_every_commit_sets_durability() {
        let dir = tempdir().unwrap();
        let wal = dir.path().join("w.wal");
        let w = WalWriter::open(&wal, SyncMode::EveryCommit, ANCRE_INITIALE).unwrap();
        // Commit-flagged record triggers fsync internally — hard to observe
        // directly, so we just verify no panic / no error.
        for i in 1..=20u64 {
            w.append(sample(i, i % 3 == 0)).unwrap();
        }
        w.fsync().unwrap();
    }

    /// ── UNE QUEUE DE ZÉROS N'EST PAS UNE COUPURE ────────────────────────────
    ///
    /// Le chemin « il reste moins qu'un en-tête » sert DEUX cas opposés : un
    /// fichier pré-alloué (zone de zéros = fin normale) et un enregistrement
    /// coupé (frontière de crash). Le contenu tranche — et ce test fixe la
    /// décision : des zéros ne lèvent PAS le drapeau, sans quoi toute base
    /// pré-allouée signalerait une panne à chaque rejeu.
    ///
    /// PORTÉE EXACTE : ce test couvre le reste PLUS COURT qu'un en-tête. Un reste
    /// de zéros PLUS LONG (≥ 32 octets) ne prend pas ce chemin : il tombe sur la
    /// détection de TROU, qui rend « fin normale » quand les zéros vont jusqu'à
    /// la fin du fichier (voir `un_entete_nul_jusqu_a_la_fin_est_une_preallocation`).
    #[test]
    fn une_queue_de_zeros_ne_leve_pas_le_drapeau() {
        let dir = tempdir().unwrap();
        let wal = dir.path().join("w.wal");
        let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
        for i in 1..=5u64 {
            w.append(sample(i, true)).unwrap();
        }
        drop(w);
        // Pré-allocation : on prolonge le fichier de zéros (ce que fait une
        // réservation de pages, et ce que ferait un `set_len` généreux).
        let len = std::fs::metadata(&wal).unwrap().len();
        let f = std::fs::OpenOptions::new().write(true).open(&wal).unwrap();
        // 8 octets : moins qu'un en-tête (28), donc le chemin « reste court ».
        f.set_len(len + 8).unwrap();
        drop(f);

        let r = WalReader::open(&wal).unwrap();
        let mut it = r.iter_from(0);
        let mut n = 0;
        while let Some(item) = it.next() {
            item.expect("aucun enregistrement corrompu");
            n += 1;
        }
        assert_eq!(n, 5, "les cinq enregistrements complets sont lus");
        assert!(
            !it.queue_incomplete(),
            "une queue de ZÉROS est une pré-allocation, pas une coupure : le \
             drapeau de frontière de crash ne doit pas être levé"
        );
    }

    /// Un en-tête NUL de 32 octets et plus, jusqu'à la fin : pré-allocation. Le
    /// code ANTÉRIEUR y décodait une « frame » de longueur 0 dont le CRC ne
    /// pouvait pas correspondre, et rendait donc une ERREUR DE CRC : un fichier
    /// pré-alloué se signalait comme corrompu.
    #[test]
    fn un_entete_nul_jusqu_a_la_fin_est_une_preallocation() {
        let dir = tempdir().unwrap();
        let wal = dir.path().join("w.wal");
        let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
        for i in 1..=5u64 {
            w.append(sample(i, true)).unwrap();
        }
        drop(w);
        let len = std::fs::metadata(&wal).unwrap().len();
        let f = std::fs::OpenOptions::new().write(true).open(&wal).unwrap();
        f.set_len(len + 4096).unwrap();
        drop(f);

        let r = WalReader::open(&wal).unwrap();
        let items: Vec<_> = r.iter_from(0).collect();
        assert_eq!(items.len(), 5, "les cinq frames sont lues, sans erreur");
        assert!(items.iter().all(Result::is_ok));
    }

    /// ── T4 : LA RÉPARATION DE QUEUE ─────────────────────────────────────────
    ///
    /// Une troncature au MILIEU d'une frame (ce que laisse un `kill -9` en pleine
    /// écriture, ou un disque plein) doit pouvoir être réparée : on revient au
    /// dernier bord de frame, et les appends suivants sont LUS — ce qui n'était
    /// pas le cas avant, puisque le rejeu s'arrête sur la déchirure et que tout
    /// ce qui suit était invisible.
    #[test]
    fn t4_reparation_de_queue_puis_reprise_propre() {
        let dir = tempdir().unwrap();
        let wal = dir.path().join("w.wal");
        let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
        for i in 1..=10u64 {
            w.append(sample(i, true)).unwrap();
        }
        let taille_saine = w.offset();
        drop(w);

        // Déchirure : on coupe 5 octets au milieu de la dernière frame.
        let f = std::fs::OpenOptions::new().write(true).open(&wal).unwrap();
        f.set_len(taille_saine - 5).unwrap();
        drop(f);

        // Le lecteur DIT la déchirure (frontière de crash), il ne la tait pas.
        let r = WalReader::open(&wal).unwrap();
        let mut it = r.iter_from(0);
        let mut fin_du_9e = 0u64;
        let mut lus = 0usize;
        while let Some(item) = it.next() {
            if let Ok((_, off, len)) = item {
                fin_du_9e = off + len as u64;
                lus += 1;
            }
        }
        assert_eq!(lus, 9, "la dixième frame est incomplète");
        assert!(it.queue_incomplete());

        // Réparation. Elle retire la frame DÉCHIRÉE EN ENTIER (ses 34 octets
        // complets plus les 5 octets coupés) : un bord de frame est un bord de
        // frame, il n'existe pas de « moitié de frame valide ».
        let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
        let rep = w.reparer_queue().unwrap().expect("une queue à réparer");
        assert_eq!(rep.avant, taille_saine - 5);
        assert_eq!(
            rep.apres, fin_du_9e,
            "la réparation ramène au dernier BORD DE FRAME valide"
        );
        assert_eq!(rep.octets_retires, rep.avant - rep.apres);
        assert!(rep.octets_retires > 5, "la frame coupée part en entier");
        assert_eq!(w.offset(), rep.apres);

        // 100 appends suivants : LUS, contigus au sens fort (ordre physique ==
        // ordre LSN, aucun doublon), et le drapeau de frontière de crash retombe.
        for i in 0..100u64 {
            w.append(sample(i, true)).unwrap();
        }
        let attendu_fin = w.offset();
        drop(w);

        let r = WalReader::open(&wal).unwrap();
        let mut it = r.iter_from(0);
        let mut n = 0usize;
        let mut last = 0u64;
        let mut dernier_offset = 0u64;
        while let Some(item) = it.next() {
            let (rec, off, len) = item.expect("aucune erreur après réparation");
            assert!(rec.lsn > last, "les LSN sont strictement croissants");
            assert_eq!(rec.lsn, w_lsn_ref(off), "lsn = ancre + position");
            last = rec.lsn;
            dernier_offset = off + len as u64;
            n += 1;
        }
        assert_eq!(n, 9 + 100, "les 9 frames saines + les 100 appends suivants");
        assert!(!it.queue_incomplete(), "plus aucune déchirure");
        assert!(it.trou().is_none(), "et aucun trou");
        assert_eq!(dernier_offset, attendu_fin, "EOF == taille exacte");
    }

    /// Le LSN attendu d'une frame d'un WAL neuf, à l'offset `off` (ancre = 1).
    fn w_lsn_ref(off: u64) -> u64 {
        ANCRE_INITIALE + off
    }

    #[test]
    fn truncate_resets_offset() {
        let dir = tempdir().unwrap();
        let wal = dir.path().join("w.wal");
        let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
        for i in 1..=5u64 {
            w.append(sample(i, true)).unwrap();
        }
        assert!(w.offset() > 0);
        let taille = w.offset();
        let nouvelle_ancre = w.truncate().unwrap();
        assert_eq!(w.offset(), 0);
        // ── PIÈCE 4 : L'ANCRE A AVANCÉ AVANT LA TRONCATURE ─────────────────
        assert_eq!(
            nouvelle_ancre,
            ANCRE_INITIALE + taille,
            "l'ancre avance de la taille du segment abandonné"
        );
        assert_eq!(w.ancre(), nouvelle_ancre);
        assert_eq!(w.peek_lsn(), nouvelle_ancre);
        // Writing again after truncate starts at offset 0.
        let (off, _) = w.append(sample(42, true)).unwrap();
        assert_eq!(off, 0);
    }

    #[test]
    fn reader_starts_from_nonzero_offset() {
        let dir = tempdir().unwrap();
        let wal = dir.path().join("w.wal");
        let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
        let (o1, _) = w.append(sample(1, true)).unwrap();
        let (o2, _) = w.append(sample(2, true)).unwrap();
        let _ = w.append(sample(3, true)).unwrap();
        drop(w);

        assert_eq!(o1, 0);
        let r = WalReader::open(&wal).unwrap();
        let items: Vec<_> = r.iter_from(o2).collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0].0.lsn,
            ANCRE_INITIALE + o2,
            "lsn = ancre + position"
        );
        assert!(items[1].0.lsn > items[0].0.lsn);
    }

    #[test]
    fn empty_wal_reads_nothing() {
        let dir = tempdir().unwrap();
        let wal = dir.path().join("empty.wal");
        let r = WalReader::open(&wal).unwrap();
        assert!(r.is_empty());
        assert!(r.iter_from(0).next().is_none());
    }
}
