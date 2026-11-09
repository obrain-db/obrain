//! Persistance du journal CDC (Change Data Capture) — prod-readiness 2026-08-31.
//!
//! Le `CdcLog` est un journal en mémoire (`RwLock<HashMap<EntityId,
//! Vec<ChangeEvent>>>`). Sans persistance, l'historique (audit trail,
//! `nodeHistory`, `changesBetween`) disparaît au redémarrage — bloquant pour
//! la prod (audit §9.5 point 1).
//!
//! Ce module attache le log à une **zone substrate dédiée**
//! (`substrate.cdc_journal`) : chaque mutation est sérialisée (bincode) et
//! appendée à la zone (append-only, checkpointée par le compact comme les
//! autres zones). Au boot, le journal est rechargé depuis la zone.
//!
//! ## Format de la zone
//!
//! ```text
//!   header (16 B) : magic "CDCJ" + version u32 + count u64
//!   payload       : frames bincode concaténées (len u32 + bytes ChangeEvent)
//! ```
//!
//! ## Index de queue (2026-09-06, chantier B « cdc_tail »)
//!
//! La zone ne porte AUCUN index temporel : retrouver les événements depuis un
//! `since` récent exige de marcher toutes les frames (le rechargement lazy
//! borne le coût par la fenêtre mémoire ~20-30 ms au premier appel d'un
//! process neuf — toujours payé même quand « rien de neuf »). Un index de
//! queue dérivé (`substrate.cdc_tail`, magic `CDCT`) est donc tenu à chaque
//! append : une frame FIXE de 20 B par événement (`ts u64`, `payload_off u64`
//! = offset de la charge utile bincode dans la zone journal, `frame_len u32`),
//! en anneau borné (50 k → ~1 Mo, le plus récent écrase le plus ancien).
//!
//! ```text
//!   substrate.cdc_tail
//!     header (24 B)    : magic "CDCT" + version u32 + head u64 + count u64
//!     frames (20 B)    : anneau de 50 000 × (ts u64, payload_off u64, frame_len u32)
//! ```
//!
//! L'index est un **cache dérivé, jamais autoritaire** : écrit sous le même
//! verrou que l'append journal (avant la mise à jour de `used`, si bien
//! qu'aucun lecteur ne voit un `used` englobant une frame non indexée), mais
//! il n'est pas une source de vérité. Les lectures (`cold_zone_delta`)
//! vérifient à chaque appel sa COHÉRENCE contre l'extent de la zone journal
//! (dernière entrée de queue = `used`) et sa COUVERTURE (`since` ≥ ts de la
//! plus ancienne entrée) avant de s'en servir ; toute incohérence (compaction
//! étrangère, append en vol, crash partiel, stores antérieurs au déploiement)
//! retombe sur le chemin historique. Les frames hors bornes / indécodables
//! sont sautées à la lecture, jamais fatales. Hypothèse temporelle : les `ts`
//! sont croissants à l'append (horloge murale) — la même hypothèse que
//! l'index résident `by_ts` du log ; un retour d'horloge borne seulement la
//! couverture de la voie froide, jamais sa sûreté (filtre ts au décodage).
//!
//! ## Durabilité
//!
//! La zone est mmap-écrite (les mutations passent déjà par le WAL substrate —
//! NodeInsert/PropSetInline…), donc le contenu survit au crash via le WAL et
//! le compact checkpoint fige la zone. Le journal mémoire reste la vue chaude ;
//! la zone est la vérité froide.

use obrain_substrate::{SubstrateError, SubstrateResult};

use crate::cdc::ChangeEvent;

/// Nom de la zone (fichier `substrate.cdc_journal`).
pub const CDC_JOURNAL_FILENAME: &str = "substrate.cdc_journal";
/// Magic d'en-tête.
const MAGIC: &[u8; 4] = b"CDCJ";
/// Version du format.
const VERSION: u32 = 1;
/// Taille de l'en-tête (magic 4 + version 4 + count 8 + used 8 + first_off 8).
///   - `used` = offset du prochain append (la zone grandit par sauts ; les
///     données réelles sont [first_off..used]).
///   - `first_off` = offset du premier événement CONSERVÉ (fenêtre) — la
///     compaction avance ce pointeur, les événements de tête deviennent
///     garbage réclamé par le compact/checkpoint (échelle multi-PB : le
///     journal n'est jamais l'historique complet).
const HEADER_LEN: usize = 32;
/// Croissance initiale de la zone.
const INITIAL_GROW: u64 = 64 * 1024;

// ---------------------------------------------------------------------------
// Index de queue (2026-09-06, chantier B « cdc_tail ») — voir doc module.
// ---------------------------------------------------------------------------

/// Nom de la zone index de queue (fichier `substrate.cdc_tail`).
pub const CDC_TAIL_FILENAME: &str = "substrate.cdc_tail";
/// Magic de l'en-tête de l'index de queue.
const TAIL_MAGIC: &[u8; 4] = b"CDCT";
/// Version du format de l'index de queue.
const TAIL_VERSION: u32 = 1;
/// En-tête de l'index : magic 4 + version u32 4 + head u64 (8..16) + count u64 (16..24).
const TAIL_HEADER_LEN: usize = 24;
/// Frame fixe de l'index : ts u64 + payload_off u64 + frame_len u32 = 20 B.
const TAIL_FRAME_LEN: usize = 20;
/// Fenêtre de l'index (nb d'entrées) — 50 k × 20 B ≈ 1 Mo.
const TAIL_CAP: usize = 50_000;
/// Taille totale du fichier index (fixe dès la première écriture).
const TAIL_FILE_LEN: usize = TAIL_HEADER_LEN + TAIL_CAP * TAIL_FRAME_LEN;

/// Une entrée de l'index de queue : repère une frame de la zone journal.
pub struct TailEntry {
    /// Timestamp wall-clock (ms Unix) de l'événement.
    pub ts: u64,
    /// Offset de la CHARGE UTILE bincode dans la zone journal (frame + 4).
    pub payload_off: u64,
    /// Longueur de la charge utile (le préfixe `len u32` de la frame).
    pub frame_len: u32,
}

/// (2026-08-31) Plafond du rechargement au boot (fenêtre CDC). Le journal
/// n'est PAS l'historique complet — l'audit récent suffit à `nodeHistory`/
/// `changesBetween` ; recharger des millions d'événements en RAM au boot
/// serait fatal sur une base multi-PB.
pub const CDC_RELOAD_CAP: usize = 100_000;

/// (2026-09-05) Fenêtre de rechargement EFFECTIVE : par défaut 10 k événements
/// (décoder 100 k payloads CDC lourds ≈ 1 Go / ~0,8 s par process sur stores
/// runtime — mesuré) ; surchargeable par `OBRAIN_CDC_RELOAD_CAP` (≤
/// [`CDC_RELOAD_CAP`]) pour les contextes d'audit qui exigent une fenêtre plus
/// profonde. La zone garde TOUT (append-only) — le plafond ne borne que la vue
/// mémoire rechargée au premier usage d'une lecture CDC.
pub fn cdc_reload_cap() -> usize {
    std::env::var("OBRAIN_CDC_RELOAD_CAP")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map_or(20_000, |v| v.clamp(1, CDC_RELOAD_CAP))
}

/// Persistor du journal CDC, attaché à un store substrate.
pub struct CdcPersist {
    /// Le store substrate (zone `cdc_journal`).
    store: std::sync::Arc<obrain_substrate::SubstrateStore>,
}

impl std::fmt::Debug for CdcPersist {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CdcPersist")
            .field("events_persisted", &self.count())
            .finish()
    }
}

impl CdcPersist {
    /// Ouvre le persistor sur le store substrate.
    pub fn open(store: std::sync::Arc<obrain_substrate::SubstrateStore>) -> Self {
        Self { store }
    }

    /// Recharge le journal depuis la zone (au boot), PL AFONNÉ à
    /// `max_events` — le CDC est une FENÊTRE, pas l'historique complet
    /// (échelle multi-PB : recharger des millions d'événements en RAM au
    /// démarrage serait fatal). `0` = recharger tout (usage test).
    pub fn load(&self, max_events: usize) -> SubstrateResult<Vec<(u64, ChangeEvent)>> {
        let sub = self.store.writer().substrate();
        let guard = sub.lock();
        // (2026-09-05) LECTURE FICHIER PAR BLOCS, PAS mmap : la passe de
        // repérage sur la zone mmap touchait TOUTES ses pages (RSS ≈ taille
        // de la zone : ~200-250 Mo sur stores runtime). Une lecture
        // `pread`/buffered garde les pages en cache noyau, hors RSS process.
        //
        // Fenêtre glissante d'offsets (et PAS le `count` d'en-tête, qui peut
        // être PÉRIMÉ après compaction — first_off avancé sans décrément →
        // on aurait décodé zéro frame sur les zones réelles) : on parcourt
        // les préfixes [len u32], on ne garde que les offsets de charge utile
        // des `max_events` DERNIÈRES frames, puis on décode cette seule
        // fenêtre (seconde lecture ciblée, seek par frame). `0` = tout.
        use std::io::{BufReader, Read, Seek, SeekFrom};
        let path = guard.path().join(CDC_JOURNAL_FILENAME);
        let Ok(mut file) = std::fs::File::open(&path) else {
            return Ok(Vec::new());
        };
        let mut hdr = [0u8; HEADER_LEN];
        if file.read_exact(&mut hdr).is_err() || &hdr[..4] != MAGIC {
            return Ok(Vec::new());
        }
        let used = u64::from_le_bytes(hdr[16..24].try_into().unwrap_or([0; 8])) as usize;
        let first_off = u64::from_le_bytes(hdr[24..32].try_into().unwrap_or([0; 8])) as usize;
        let start = first_off.max(HEADER_LEN);
        let keep = max_events;
        // Passe 1 : marcher les préfixes, mémoriser les offsets des dernières
        // `keep` charges utiles (filet glissant, aucune allocation par frame).
        let mut win: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
        {
            file.seek(SeekFrom::Start(start as u64))?;
            let mut r = BufReader::with_capacity(1 << 20, file);
            let mut off = start;
            loop {
                let mut lb = [0u8; 4];
                if r.read(&mut lb)? < 4 {
                    break;
                }
                let len = u32::from_le_bytes(lb) as usize;
                off += 4;
                if len == 0 || off + len > used {
                    break;
                }
                if keep == 0 || win.len() < keep {
                    win.push_back(off);
                } else {
                    win.pop_front();
                    win.push_back(off);
                }
                off += len;
                if r.seek_relative(len as i64).is_err() {
                    break;
                }
            }
        }
        // Passe 2 : décoder la fenêtre glissante.
        let mut out: Vec<(u64, ChangeEvent)> = Vec::new();
        {
            let mut file = std::fs::File::open(&path)?;
            for payload_off in win {
                file.seek(SeekFrom::Start(payload_off as u64 - 4))?;
                let mut lb = [0u8; 4];
                if file.read_exact(&mut lb).is_err() {
                    break;
                }
                let len = u32::from_le_bytes(lb) as usize;
                let mut buf = vec![0u8; len];
                if file.read_exact(&mut buf).is_err() {
                    break;
                }
                match bincode::serde::decode_from_slice::<ChangeEvent, _>(
                    &buf,
                    bincode::config::standard(),
                ) {
                    Ok((ev, _)) => out.push((ev.timestamp, ev)),
                    Err(_) => break,
                }
            }
        }
        Ok(out)
    }

    /// Append un événement dans la zone (frame bincode).
    pub fn append(&self, event: &ChangeEvent) -> SubstrateResult<()> {
        let frame = match bincode::serde::encode_to_vec(event, bincode::config::standard()) {
            Ok(f) => f,
            Err(e) => {
                return Err(SubstrateError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("cdc: bincode serialize failed: {e}"),
                )));
            }
        };

        let sub = self.store.writer().substrate();
        let guard = sub.lock();
        let mut zf = guard.open_named_zone(CDC_JOURNAL_FILENAME)?;
        // Zone vide → en-tête.
        if zf.len() < HEADER_LEN as u64 {
            zf.grow_to(HEADER_LEN as u64)?;
            zf.as_slice_mut()[..4].copy_from_slice(MAGIC);
            zf.as_slice_mut()[4..8].copy_from_slice(&VERSION.to_le_bytes());
            zf.as_slice_mut()[8..16].copy_from_slice(&0u64.to_le_bytes());
            zf.as_slice_mut()[16..24].copy_from_slice(&(HEADER_LEN as u64).to_le_bytes());
        }
        let used = u64::from_le_bytes(zf.as_slice()[16..24].try_into().unwrap_or([0; 8])) as usize;
        if used < HEADER_LEN {
            // zone initialisée sans offset (ancien format) → répare
            zf.as_slice_mut()[16..24].copy_from_slice(&(HEADER_LEN as u64).to_le_bytes());
        }
        let needed = used + 4 + frame.len();
        if needed as u64 > zf.len() {
            // Croissance exponentielle ×1.5 (comme les autres zones).
            let mut new_len = (zf.len() * 3 / 2).max(INITIAL_GROW);
            while new_len < needed as u64 {
                new_len = new_len * 3 / 2;
            }
            zf.grow_to(new_len)?;
        }
        // (2026-09-06, cdc_tail) Index de queue — l'entrée est écrite AVANT la
        // frame et la mise à jour de `used` : un lecteur concurrent ne peut
        // jamais voir un `used` qui englobe une frame non indexée (la
        // vérification d'extent exige la cohérence exacte dernière-entrée ==
        // used). Échec ici → append entier en échec (fail-soft amont), journal
        // intact et cohérent.
        let off = used;
        self.tail_push(
            &guard,
            event.timestamp,
            (off + 4) as u64,
            frame.len() as u32,
        )?;
        // Écrit la frame.
        zf.as_slice_mut()[off..off + 4].copy_from_slice(&(frame.len() as u32).to_le_bytes());
        zf.as_slice_mut()[off + 4..off + 4 + frame.len()].copy_from_slice(&frame);
        // Met à jour count + used dans l'en-tête.
        let count = u64::from_le_bytes(zf.as_slice()[8..16].try_into().unwrap()) + 1;
        zf.as_slice_mut()[8..16].copy_from_slice(&count.to_le_bytes());
        let new_used = used + 4 + frame.len();
        zf.as_slice_mut()[16..24].copy_from_slice(&(new_used as u64).to_le_bytes());
        Ok(())
    }

    /// (2026-09-06, cdc_tail) Insère une entrée dans l'anneau de queue
    /// (`substrate.cdc_tail`), SOUS le verrou déjà tenu par l'append journal.
    /// Le fichier est dimensionné une fois pour toutes (`TAIL_FILE_LEN`) ; le
    /// slot d'écriture est `(head + count) % CAP`, et quand l'anneau est plein
    /// le plus ancien est écrasé (`head` avance). L'écriture de la frame
    /// précède la publication de `head`/`count` (lecteur cohérent à tout
    /// instant).
    fn tail_push(
        &self,
        sub: &obrain_substrate::file::SubstrateFile,
        ts: u64,
        payload_off: u64,
        frame_len: u32,
    ) -> SubstrateResult<()> {
        let mut zf = sub.open_named_zone(CDC_TAIL_FILENAME)?;
        if zf.len() < TAIL_FILE_LEN as u64 {
            zf.grow_to(TAIL_FILE_LEN as u64)?;
        }
        let s = zf.as_slice_mut();
        if s.len() < TAIL_HEADER_LEN || &s[..4] != TAIL_MAGIC {
            // Fichier neuf ou résiduel (crash pendant l'init) → en-tête propre.
            s[..4].copy_from_slice(TAIL_MAGIC);
            s[4..8].copy_from_slice(&TAIL_VERSION.to_le_bytes());
            s[8..16].copy_from_slice(&0u64.to_le_bytes());
            s[16..24].copy_from_slice(&0u64.to_le_bytes());
        }
        let head = u64::from_le_bytes(s[8..16].try_into().unwrap_or([0; 8])) as usize % TAIL_CAP;
        let count =
            (u64::from_le_bytes(s[16..24].try_into().unwrap_or([0; 8])) as usize).min(TAIL_CAP);
        let (slot, new_head, new_count) = tail_ring_slot(TAIL_CAP, head, count);
        let base = TAIL_HEADER_LEN + slot * TAIL_FRAME_LEN;
        s[base..base + 8].copy_from_slice(&ts.to_le_bytes());
        s[base + 8..base + 16].copy_from_slice(&payload_off.to_le_bytes());
        s[base + 16..base + 20].copy_from_slice(&frame_len.to_le_bytes());
        // Publication (frame écrite avant head/count).
        s[8..16].copy_from_slice(&(new_head as u64).to_le_bytes());
        s[16..24].copy_from_slice(&(new_count as u64).to_le_bytes());
        Ok(())
    }

    /// (2026-08-31) COMPACTION DE TÊTE — échelle multi-PB : le journal
    /// append-only grandirait sans borne. `compact` avance la fenêtre
    /// (`first_off`) de `skip` événements : les frames de tête deviennent
    /// garbage, réclamées au prochain compact/checkpoint (qui tronque la
    /// zone). Les `count` / `load` n'y voient plus rien. Idempotent.
    ///
    /// `skip` = nombre d'événements à retirer de la TÊTE (les plus anciens).
    pub fn compact(&self, skip: u64) -> SubstrateResult<()> {
        if skip == 0 {
            return Ok(());
        }
        let sub = self.store.writer().substrate();
        let guard = sub.lock();
        let mut zf = guard.open_named_zone(CDC_JOURNAL_FILENAME)?;
        if zf.len() < HEADER_LEN as u64 || &zf.as_slice()[..4] != MAGIC {
            return Ok(());
        }
        let first_off =
            u64::from_le_bytes(zf.as_slice()[24..32].try_into().unwrap_or([0; 8])) as usize;
        let used = u64::from_le_bytes(zf.as_slice()[16..24].try_into().unwrap_or([0; 8])) as usize;
        let end = used.min(zf.len() as usize);
        // Avance first_off de `skip` frames (borné par la fin de la fenêtre).
        let mut skipped = 0u64;
        let mut off = first_off.max(HEADER_LEN);
        while off + 4 <= end && skipped < skip {
            let len = u32::from_le_bytes(zf.as_slice()[off..off + 4].try_into().unwrap()) as usize;
            off += 4 + len;
            if off > end {
                break;
            }
            skipped += 1;
        }
        if skipped == 0 {
            return Ok(());
        }
        let new_first = off.min(end);
        zf.as_slice_mut()[24..32].copy_from_slice(&(new_first as u64).to_le_bytes());
        // Met à jour le count (événements conservés).
        let count = u64::from_le_bytes(zf.as_slice()[8..16].try_into().unwrap_or([0; 8]));
        let new_count = count.saturating_sub(skipped);
        zf.as_slice_mut()[8..16].copy_from_slice(&new_count.to_le_bytes());
        // (2026-09-06, cdc_tail) La fenêtre a avancé : les entrées de queue
        // pointant avant le nouveau `first_off` référencent des frames
        // compactées (garbage) → trim de tête sous le même verrou (sinon la
        // couverture « première entrée » resterait trop ancienne et la voie
        // froide décoderait des entrées mortes).
        self.tail_trim(&guard, new_first as u64 + 4)?;
        Ok(())
    }

    /// Nombre d'événements persistés.
    pub fn count(&self) -> u64 {
        let sub = self.store.writer().substrate();
        let guard = sub.lock();
        match guard.open_named_zone(CDC_JOURNAL_FILENAME) {
            Ok(zf) if zf.len() >= HEADER_LEN as u64 => {
                u64::from_le_bytes(zf.as_slice()[8..16].try_into().unwrap_or([0; 8]))
            }
            _ => 0,
        }
    }

    // -----------------------------------------------------------------------
    // Index de queue — lectures (2026-09-06, chantier B « cdc_tail »)
    // -----------------------------------------------------------------------

    /// (2026-09-06, cdc_tail) Entrées de l'index de queue en ordre
    /// chronologique (plus ancienne → plus récente). Fichier absent, mal
    /// formé ou tronqué → ce qui est lisible (voire vide) : l'index n'est
    /// qu'un cache — la cohérence est tranchée par l'appelant contre l'extent
    /// de la zone journal.
    fn tail_entries(&self) -> Vec<TailEntry> {
        let sub = self.store.writer().substrate();
        let guard = sub.lock();
        let path = guard.path().join(CDC_TAIL_FILENAME);
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut file) = std::fs::File::open(&path) else {
            return Vec::new();
        };
        let mut hdr = [0u8; TAIL_HEADER_LEN];
        if file.read_exact(&mut hdr).is_err() || &hdr[..4] != TAIL_MAGIC {
            return Vec::new();
        }
        let head = u64::from_le_bytes(hdr[8..16].try_into().unwrap_or([0; 8])) as usize;
        let count =
            (u64::from_le_bytes(hdr[16..24].try_into().unwrap_or([0; 8])) as usize).min(TAIL_CAP);
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let base = TAIL_HEADER_LEN + ((head + i) % TAIL_CAP) * TAIL_FRAME_LEN;
            let mut fr = [0u8; TAIL_FRAME_LEN];
            if file.seek(SeekFrom::Start(base as u64)).is_err() || file.read_exact(&mut fr).is_err()
            {
                break; // fichier tronqué → on garde ce qui est lisible
            }
            out.push(TailEntry {
                ts: u64::from_le_bytes(fr[0..8].try_into().unwrap_or([0; 8])),
                payload_off: u64::from_le_bytes(fr[8..16].try_into().unwrap_or([0; 8])),
                frame_len: u32::from_le_bytes(fr[16..20].try_into().unwrap_or([0; 4])),
            });
        }
        out
    }

    /// (2026-09-06, cdc_tail) Extent courant de la zone journal :
    /// `(used, first_off)`. `(0, 0)` si la zone est absente ou mal formée.
    fn journal_extent(&self) -> (u64, u64) {
        let sub = self.store.writer().substrate();
        let guard = sub.lock();
        let path = guard.path().join(CDC_JOURNAL_FILENAME);
        use std::io::Read;
        let Ok(mut file) = std::fs::File::open(&path) else {
            return (0, 0);
        };
        let mut hdr = [0u8; HEADER_LEN];
        if file.read_exact(&mut hdr).is_err() || &hdr[..4] != MAGIC {
            return (0, 0);
        }
        let used = u64::from_le_bytes(hdr[16..24].try_into().unwrap_or([0; 8]));
        let first = u64::from_le_bytes(hdr[24..32].try_into().unwrap_or([0; 8]));
        (used, first)
    }

    /// (2026-09-06, cdc_tail) Décode les frames de la zone journal désignées
    /// par `frames` (même patron de lecture que [`Self::load`] : `File` +
    /// seek, hors RSS process). Toute frame hors bornes, au préfixe
    /// incohérent ou indécodable est SAUTÉE — jamais une erreur fatale :
    /// l'index est un cache dérivé, et un saut correspond à une frame déjà
    /// compactée/tronquée ou à une écriture concurrente transitoire (la
    /// forme delta se répare à l'appel suivant).
    fn read_frames(&self, frames: &[TailEntry]) -> Vec<ChangeEvent> {
        if frames.is_empty() {
            return Vec::new();
        }
        let sub = self.store.writer().substrate();
        let guard = sub.lock();
        let path = guard.path().join(CDC_JOURNAL_FILENAME);
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut file) = std::fs::File::open(&path) else {
            return Vec::new();
        };
        let mut hdr = [0u8; HEADER_LEN];
        if file.read_exact(&mut hdr).is_err() || &hdr[..4] != MAGIC {
            return Vec::new();
        }
        let used = u64::from_le_bytes(hdr[16..24].try_into().unwrap_or([0; 8])) as usize;
        let first_off = u64::from_le_bytes(hdr[24..32].try_into().unwrap_or([0; 8])) as usize;
        let start = first_off.max(HEADER_LEN);
        let mut out = Vec::with_capacity(frames.len());
        for fr in frames {
            let po = fr.payload_off as usize;
            let len = fr.frame_len as usize;
            // Garde : charge utile dans la fenêtre vivante [start..used] et
            // préfixe `len` cohérent (frame ≠ garbage réutilisé).
            if po < start + 4 || po + len > used || len == 0 {
                continue;
            }
            if file.seek(SeekFrom::Start((po - 4) as u64)).is_err() {
                break;
            }
            let mut lb = [0u8; 4];
            if file.read_exact(&mut lb).is_err() {
                break;
            }
            if u32::from_le_bytes(lb) as usize != len {
                continue;
            }
            let mut buf = vec![0u8; len];
            if file.read_exact(&mut buf).is_err() {
                break;
            }
            if let Ok((ev, _)) = bincode::serde::decode_from_slice::<ChangeEvent, _>(
                &buf,
                bincode::config::standard(),
            ) {
                out.push(ev);
            }
        }
        out
    }

    /// (2026-09-06, cdc_tail) VOIE FROIDE ZONE — événements ≥ `since_ms` sans
    /// marcher/recharger la zone, UNIQUEMENT quand l'index de queue peut
    /// répondre sans risque :
    ///
    /// 1. **Cohérence** : la dernière entrée de queue couvre exactement
    ///    `used` du journal (et queue vide ⇔ fenêtre journal vide) — sinon
    ///    l'index est périmé/incomplet (crash partiel, append en vol, store
    ///    antérieur au déploiement) → `None`.
    /// 2. **Couverture** : `since_ms` ≥ ts de la plus ancienne entrée de queue
    ///    — sinon des événements ≥ `since` hors de la fenêtre de l'index
    ///    (tête évincée/compactée) manqueraient → `None`.
    ///
    /// `Some(events)` = réponse fiable (vide = « rien de neuf », µs) ; les
    /// événements sont BRUTS (bincode de la zone) — l'appelant applique sa
    /// rétention légère pour rester symétrique du chemin chaud.
    pub fn cold_zone_delta(&self, since_ms: u64) -> Option<Vec<ChangeEvent>> {
        let tail = self.tail_entries();
        let (used, first_off) = self.journal_extent();
        let start = first_off.max(HEADER_LEN as u64);
        match tail.last() {
            // Queue vide : légitime uniquement si le journal n'a pas d'événements.
            None if start >= used => return Some(Vec::new()),
            None => return None,
            Some(last) if last.payload_off + u64::from(last.frame_len) != used => return None,
            Some(_) => {}
        }
        // Couverture : le plus ancien événement de la queue doit être ≤ since.
        if let Some(first) = tail.first()
            && since_ms < first.ts
        {
            return None;
        }
        let sel: Vec<TailEntry> = tail.into_iter().filter(|e| e.ts >= since_ms).collect();
        let mut events = self.read_frames(&sel);
        // Filet de sûreté (lectures concurrentes/déchirées possibles en
        // RO-vs-writer) : on ne rend JAMAIS un événement < since — l'index
        // n'est qu'un cache ; un saut se répare à l'appel suivant.
        events.retain(|e| e.timestamp >= since_ms);
        Some(events)
    }

    /// (2026-09-06, cdc_tail) Trim de tête de l'anneau : retire les entrées
    /// dont la charge utile est antérieure à `min_payload_off` (frames
    /// compactées). À appeler SOUS le verrou de la compaction du journal.
    fn tail_trim(
        &self,
        sub: &obrain_substrate::file::SubstrateFile,
        min_payload_off: u64,
    ) -> SubstrateResult<()> {
        let mut zf = sub.open_named_zone(CDC_TAIL_FILENAME)?;
        if zf.len() < TAIL_HEADER_LEN as u64 || &zf.as_slice()[..4] != TAIL_MAGIC {
            // Pas d'index (jamais écrit) → rien à trimer. Le fichier peut être
            // créé vide ici — inoffensif (lectures → vide, cohérence tranchée
            // par l'extent du journal).
            return Ok(());
        }
        let s = zf.as_slice_mut();
        let mut head =
            u64::from_le_bytes(s[8..16].try_into().unwrap_or([0; 8])) as usize % TAIL_CAP;
        let mut count =
            (u64::from_le_bytes(s[16..24].try_into().unwrap_or([0; 8])) as usize).min(TAIL_CAP);
        while count > 0 {
            let base = TAIL_HEADER_LEN + head * TAIL_FRAME_LEN;
            let payload_off =
                u64::from_le_bytes(s[base + 8..base + 16].try_into().unwrap_or([0; 8]));
            if payload_off >= min_payload_off {
                break;
            }
            head = (head + 1) % TAIL_CAP;
            count -= 1;
        }
        s[8..16].copy_from_slice(&(head as u64).to_le_bytes());
        s[16..24].copy_from_slice(&(count as u64).to_le_bytes());
        Ok(())
    }
}

/// (2026-09-06, cdc_tail) Math de l'anneau : slot d'écriture + nouvel
/// état `(head, count)` après insertion d'une entrée. `cap` = taille de
/// l'anneau (testée unitairement avec un petit cap).
fn tail_ring_slot(cap: usize, head: usize, count: usize) -> (usize, usize, usize) {
    if count < cap {
        ((head + count) % cap, head, count + 1)
    } else {
        // Anneau plein : écrase le plus ancien (slot = head) puis head avance.
        (head, (head + 1) % cap, count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::{ChangeKind, EntityId};
    use obrain_common::types::{EdgeId, NodeId, Value};
    use std::collections::HashMap;

    fn mk_event(ts: u64) -> ChangeEvent {
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::from("Alix"));
        ChangeEvent {
            entity_id: EntityId::Node(NodeId::new(1)),
            kind: ChangeKind::Create,
            epoch: obrain_common::types::EpochId(3),
            timestamp: ts,
            before: None,
            after: Some(props),
        }
    }

    #[test]
    fn append_then_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = obrain_substrate::SubstrateStore::create(dir.path().join("cdc")).unwrap();
        let p = CdcPersist::open(std::sync::Arc::new(store));
        let ev = mk_event(12345);
        p.append(&ev).unwrap();
        assert_eq!(p.count(), 1);

        let loaded = p.load(0).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].1, ev);
    }

    #[test]
    fn compact_advance_fenetre() {
        let dir = tempfile::tempdir().unwrap();
        let store = obrain_substrate::SubstrateStore::create(dir.path().join("cdc3")).unwrap();
        let p = CdcPersist::open(std::sync::Arc::new(store));
        for i in 0..10u64 {
            p.append(&mk_event(i)).unwrap();
        }
        assert_eq!(p.count(), 10);
        // Compacte 4 de tête → 6 conservés, les 4 premiers invisibles.
        p.compact(4).unwrap();
        assert_eq!(p.count(), 6);
        let loaded = p.load(0).unwrap();
        assert_eq!(loaded.len(), 6);
        // Les timestamps restants sont 4..=9 (la tête 0..3 est partie).
        let ts: Vec<u64> = loaded.iter().map(|(t, _)| *t).collect();
        assert_eq!(ts, vec![4, 5, 6, 7, 8, 9]);
        // Compact au-delà de la fenêtre → tout est retiré.
        p.compact(100).unwrap();
        assert_eq!(p.count(), 0);
        assert!(p.load(0).unwrap().is_empty(), "résultat attendu vide");
    }

    #[test]
    fn load_plafond_fenetre() {
        let dir = tempfile::tempdir().unwrap();
        let store = obrain_substrate::SubstrateStore::create(dir.path().join("cdc4")).unwrap();
        let p = CdcPersist::open(std::sync::Arc::new(store));
        for i in 0..100u64 {
            p.append(&mk_event(i)).unwrap();
        }
        // Plafond 10 → les 10 DERNIERS (90..=99).
        let loaded = p.load(10).unwrap();
        assert_eq!(loaded.len(), 10);
        let ts: Vec<u64> = loaded.iter().map(|(t, _)| *t).collect();
        assert_eq!(ts, (90..100).collect::<Vec<u64>>());
    }

    #[test]
    fn reopen_survives() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cdc2");
        {
            let store = obrain_substrate::SubstrateStore::create(&path).unwrap();
            let p = CdcPersist::open(std::sync::Arc::new(store));
            p.append(&mk_event(1)).unwrap();
            p.append(&mk_event(2)).unwrap();
            assert_eq!(p.count(), 2);
        } // fermé
        // Réouverture : le journal est rechargé depuis la zone.
        let store = obrain_substrate::SubstrateStore::open(&path).unwrap();
        let p = CdcPersist::open(std::sync::Arc::new(store));
        assert_eq!(p.count(), 2);
        let loaded = p.load(0).unwrap();
        assert_eq!(loaded.len(), 2);
        // EdgeId aussi
        let mut props = HashMap::new();
        props.insert("w".to_string(), Value::from(42));
        let ev = ChangeEvent {
            entity_id: EntityId::Edge(EdgeId::new(7)),
            kind: ChangeKind::Update,
            epoch: obrain_common::types::EpochId(9),
            timestamp: 999,
            before: None,
            after: Some(props),
        };
        p.append(&ev).unwrap();
        assert_eq!(p.count(), 3);
    }

    // -----------------------------------------------------------------------
    // Index de queue (cdc_tail, 2026-09-06)
    // -----------------------------------------------------------------------

    #[test]
    fn tail_ring_slot_math() {
        // Anneau de cap 3 : remplit, puis écrase le plus ancien.
        assert_eq!(tail_ring_slot(3, 0, 0), (0, 0, 1));
        assert_eq!(tail_ring_slot(3, 0, 1), (1, 0, 2));
        assert_eq!(tail_ring_slot(3, 0, 2), (2, 0, 3));
        // Plein : écrase le slot head (0), head avance, count inchangé.
        assert_eq!(tail_ring_slot(3, 0, 3), (0, 1, 3));
        assert_eq!(tail_ring_slot(3, 1, 3), (1, 2, 3));
        // Après trim (head 2, count 1) → le prochain append re-suit l'ordre.
        assert_eq!(tail_ring_slot(3, 2, 1), (0, 2, 2));
    }

    #[test]
    fn cold_zone_delta_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = obrain_substrate::SubstrateStore::create(dir.path().join("cdc_tail0")).unwrap();
        let p = CdcPersist::open(std::sync::Arc::new(store));
        // Aucune zone → used (0,0) ; fenêtre vide → réponse vide fiable, µs.
        assert!(
            p.cold_zone_delta(0).unwrap().is_empty(),
            "résultat attendu vide"
        );
        assert!(
            p.cold_zone_delta(u64::MAX).unwrap().is_empty(),
            "résultat attendu vide"
        );
    }

    #[test]
    fn cold_zone_delta_via_queue() {
        let dir = tempfile::tempdir().unwrap();
        let store = obrain_substrate::SubstrateStore::create(dir.path().join("cdc_tail1")).unwrap();
        let p = CdcPersist::open(std::sync::Arc::new(store));
        for ts in 1..=10u64 {
            p.append(&mk_event(ts)).unwrap();
        }
        // `since` antérieur à la couverture de la queue → pas de voie froide
        // (des événements hors index pourraient manquer).
        assert!(p.cold_zone_delta(0).is_none());
        // Nouveauté récente : SEULES les frames ≥ since sont décodées.
        let got = p.cold_zone_delta(5).unwrap();
        let ts: Vec<u64> = got.iter().map(|e| e.timestamp).collect();
        assert_eq!(ts, (5..=10).collect::<Vec<u64>>());
        // Rien de neuf → Some(vide), sans décodage.
        assert!(
            p.cold_zone_delta(11).unwrap().is_empty(),
            "résultat attendu vide"
        );
    }

    #[test]
    fn cold_zone_delta_coherent_after_compact() {
        let dir = tempfile::tempdir().unwrap();
        let store =
            obrain_substrate::SubstrateStore::create(dir.path().join("cdc_tail_c")).unwrap();
        let p = CdcPersist::open(std::sync::Arc::new(store));
        for ts in 0..10u64 {
            p.append(&mk_event(ts)).unwrap();
        }
        // Compacte 4 de tête → fenêtre vivante ts 4..=9 ; la queue est trimée
        // de même sous le verrou (entrées pointant avant le nouveau first_off).
        p.compact(4).unwrap();
        assert_eq!(p.count(), 6);
        assert!(p.cold_zone_delta(3).is_none());
        let got = p.cold_zone_delta(4).unwrap();
        let ts: Vec<u64> = got.iter().map(|e| e.timestamp).collect();
        assert_eq!(ts, (4..=9).collect::<Vec<u64>>());
        // Compact au-delà de la fenêtre → journal vidé (used == first_off) et
        // queue vidée → cohérent → Some(vide) sans rechargement.
        p.compact(100).unwrap();
        assert_eq!(p.count(), 0);
        assert!(
            p.cold_zone_delta(0).unwrap().is_empty(),
            "résultat attendu vide"
        );
    }

    #[test]
    fn cdc_tail_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cdc_tail_re");
        {
            let store = obrain_substrate::SubstrateStore::create(&path).unwrap();
            let p = CdcPersist::open(std::sync::Arc::new(store));
            for ts in 1..=5u64 {
                p.append(&mk_event(ts)).unwrap();
            }
        }
        // Réouverture : l'index de queue persiste (cache dérivé, fichier à part).
        let store = obrain_substrate::SubstrateStore::open(&path).unwrap();
        let p = CdcPersist::open(std::sync::Arc::new(store));
        let got = p.cold_zone_delta(3).unwrap();
        let ts: Vec<u64> = got.iter().map(|e| e.timestamp).collect();
        assert_eq!(ts, vec![3, 4, 5]);
    }

    #[test]
    fn cdc_tail_eviction_at_cap() {
        let dir = tempfile::tempdir().unwrap();
        let store =
            obrain_substrate::SubstrateStore::create(dir.path().join("cdc_tail_ev")).unwrap();
        let p = CdcPersist::open(std::sync::Arc::new(store));
        let light = |ts: u64| ChangeEvent {
            entity_id: EntityId::Node(NodeId::new((ts % 7) + 1)),
            kind: ChangeKind::Create,
            epoch: obrain_common::types::EpochId(ts),
            timestamp: ts,
            before: None,
            after: None,
        };
        let n = TAIL_CAP + 3; // 50_003 appends : l'anneau doit évincer les 3 plus vieux.
        for ts in 1..=(n as u64) {
            p.append(&light(ts)).unwrap();
        }
        let first_kept = (n - TAIL_CAP + 1) as u64; // ts du premier conservé (4)
        // Avant la couverture de la queue → pas de voie froide.
        assert!(p.cold_zone_delta(first_kept - 1).is_none());
        // Depuis le premier conservé : toute la fenêtre de l'anneau, en ordre.
        let got = p.cold_zone_delta(first_kept).unwrap();
        assert_eq!(got.len(), TAIL_CAP);
        assert_eq!(got.first().unwrap().timestamp, first_kept);
        assert_eq!(got.last().unwrap().timestamp, n as u64);
    }
}
