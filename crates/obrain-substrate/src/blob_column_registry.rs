//! `BlobColumnRegistry` — runtime routing of oversized `Value::String`
//! and `Value::Bytes` properties to the two-file mmap blob-column
//! zones defined in [`crate::blob_column`].
//!
//! # Role in the stack
//!
//! ```text
//! SubstrateStore::set_node_property(id, key, Value::String(s))
//!     └──► if s.len() > BLOB_COLUMN_THRESHOLD_BYTES
//!            └──► BlobColumnRegistry::write(key, Node, id, s.as_bytes())
//!                   └──► get-or-create BlobColumnWriter for (key, Node)
//!                          └──► write_slot(id as u32, bytes)
//! ```
//!
//! The registry is the sole gatekeeper of the `substrate.blobcol.*`
//! zone family, mirroring [`crate::vec_column_registry::VecColumnRegistry`]:
//!
//! 1. **One writer per (key, entity)** — kept open for the store's
//!    lifetime so a migration loop does not pay per-write `mmap` costs.
//! 2. **Read consistency** — reads issued after a write see the new
//!    payload via the writer's own mmap, even before the header CRCs
//!    have been `sync`'d.
//! 3. **Durability at flush cadence** — [`Self::sync_all`] walks every
//!    writer and `sync`s it, computing both CRCs and `msync`+`fsync`'ing
//!    the two files of each column.
//!
//! # Crash safety
//!
//! Identical model to the vec-column registry: writes are visible in
//! process but not durable until the next `flush`; a crash between
//! flushes loses writes back to the last sync-point, and on reopen the
//! column's reader rejects the stale header via the CRC check and the
//! zone degrades to "absent".

use std::sync::Arc;

use dashmap::DashMap;
use obrain_common::types::{PropertyKey, Value};
use parking_lot::RwLock;

use crate::blob_column::{BlobColSpec, BlobColumnWriter};
use crate::error::{SubstrateError, SubstrateResult};
use crate::file::SubstrateFile;
use crate::vec_column::EntityKind;

/// Default routing threshold for scalar string / bytes payloads:
/// values shorter than this land in the in-memory property map (and
/// thus in `substrate.props` as before), values longer are routed to a
/// blob column. 256 B was picked to cover every chat/event `data`
/// payload on PO (avg 1.4 KiB) while leaving small identifiers
/// (`file_path` avg 97 B, short `name` / `id` keys) in the scalar path.
pub const BLOB_COLUMN_THRESHOLD_BYTES: usize = 256;

/// Leading tag byte marking a blob-column payload that originated from a
/// `Value::String`. The raw UTF-8 bytes follow the tag.
pub(crate) const BLOB_TAG_STRING: u8 = b'S';

/// Leading tag byte marking a blob-column payload that originated from a
/// `Value::Bytes`. The raw bytes follow the tag.
pub(crate) const BLOB_TAG_BYTES: u8 = b'B';
/// Charge utile encodée en BINCODE derrière l'étiquette — voir
/// `encode_blob_payload`. Posée le 19/08 pour les listes ; élargie le 20/08 à
/// toutes les variantes à repli bincode (`Map`, `Path`, listes hétérogènes).
///
/// LE NOM A CHANGÉ, PAS LA VALEUR. `BLOB_TAG_LIST` disait le seul usage qu'on
/// lui connaissait ; il en a désormais trois. Le décodeur, lui, n'a jamais eu
/// à changer : il décode un `Value` bincode QUELCONQUE, ce qui rend une carte
/// écrite aujourd'hui relisible par un binaire du 19/08 — argument décisif
/// contre l'introduction d'une étiquette neuve, qui aurait rendu ces charges
/// indécodables en cas de retour arrière.
// NE PAS « harmoniser » CETTE VALEUR EN bL (2026-08-19).
//
// Les étiquettes voisines sont des lettres ASCII lisibles en hexdump, et 3
// détonne — la tentation de l aligner est réelle. Elle serait destructrice :
// cette valeur est DÉJÀ SÉRIALISÉE sur disque, dans les colonnes blob de
// `agent/system` (16 669 embeddings) et `agent`, migrées le 19/08. Changer
// l étiquette rendrait ces charges indécodables — `decode_blob_payload`
// tomberait dans son cas par défaut et rendrait None, c est-à-dire une
// propriété silencieusement DISPARUE, exactement le défaut que ce correctif
// est venu réparer.
//
// Toute nouvelle étiquette doit simplement éviter 3, bS et bB.
const BLOB_TAG_BINCODE: u8 = 3;

/// Inspect a [`Value`] and decide whether it should be routed to a blob
/// column. Returns `Some(tagged_bytes)` if the value is a `String` or
/// `Bytes` whose payload exceeds [`BLOB_COLUMN_THRESHOLD_BYTES`], `None`
/// otherwise. The first byte of the returned vector is a type tag
/// ([`BLOB_TAG_STRING`] or [`BLOB_TAG_BYTES`]) so the read-back path can
/// reconstruct the original [`Value`] variant byte-exactly — without
/// this, a `Value::String` and a `Value::Bytes` with identical payloads
/// would be indistinguishable on reopen.
///
/// `Value::Vector` is intentionally NOT handled here — vectors live in
/// the dense `vec_columns` zones (fixed `dim × dtype`), not in the
/// variable-length blob arena.
pub(crate) fn encode_blob_payload(value: &Value) -> Option<Vec<u8>> {
    match value {
        Value::String(s) => {
            let bytes = s.as_bytes();
            if bytes.len() > BLOB_COLUMN_THRESHOLD_BYTES {
                let mut out = Vec::with_capacity(1 + bytes.len());
                out.push(BLOB_TAG_STRING);
                out.extend_from_slice(bytes);
                Some(out)
            } else {
                None
            }
        }
        Value::Bytes(b) => {
            if b.len() > BLOB_COLUMN_THRESHOLD_BYTES {
                let mut out = Vec::with_capacity(1 + b.len());
                out.push(BLOB_TAG_BYTES);
                out.extend_from_slice(b);
                Some(out)
            } else {
                None
            }
        }
        // ── (2026-08-19) LES GROSSES LISTES N AVAIENT AUCUNE ROUTE ──
        //
        // Une `Value::List` volumineuse — typiquement un embedding de 768
        // flottants stocke comme liste plutot que comme `Value::Vector` —
        // ne trouvait AUCUNE destination : pas un Vector (donc pas de
        // colonne dense), pas un String/Bytes (donc pas de blob), et bien
        // trop grosse pour tenir dans une page de proprietes. Elle partait
        // sur le chemin scalaire, ou l append echouait avec « property
        // entry of N B exceeds single-page payload » — un simple `warn`.
        //
        // CONSEQUENCE VECUE (19/08, migration de `agent/system`) : 16 669
        // proprietes DISPARUES, toutes des `embedding`. L echec etant
        // silencieux, `obrain-migrate --finalize-v2` supprimait le sidecar
        // herite, et l ouverture suivante ne rehydratait plus rien
        // (`load_properties` court-circuite des que v2 a des pages). La
        // valeur n existait plus nulle part.
        //
        // On leur donne la route qui leur manquait, avec le meme seuil que
        // les autres. La liste est encodee en bincode derriere son etiquette,
        // et `decode_blob_payload` la reconstruit a l identique.
        // ── (2026-08-20) LES CARTES, LES CHEMINS ET LES LISTES HÉTÉROGÈNES
        //    N'AVAIENT PAS DE ROUTE NON PLUS ──
        //
        // Le 19/08 n'a ouvert cette porte qu'à `Value::List`. Les autres
        // variantes à repli bincode — `Map`, `Path`, et par ricochet les
        // listes hétérogènes qui n'y arrivaient que par le premier bras —
        // restaient sur le chemin scalaire, donc sur `encode_bincode_fallback`,
        // donc sur `intern_bytes`, borné à UNE page de tas (4 076 o utiles).
        // Au-delà, l'ajout v2 est REFUSÉ, la valeur retombe sur la DashMap —
        // qui n'est plus persistée sur une base finalisée — et DISPARAÎT à la
        // réouverture. Écrite sans erreur, relue dans la session, absente au
        // redémarrage.
        //
        // MESURÉ (2026-08-20, `carto` figée par
        // `props_codec::tests::le_repli_bincode_deborde_bien_avant_la_page_de_tas`) :
        //
        //     famille              refus v2 à N     varint > 256 o à N   marge
        //     Map<str,Int64>          163                  32            5,1×
        //     Map imbriquée            89                  22            4,0×
        //     List hétérogène         295                  75            3,9×
        //     Path                    170                  64            2,7×
        //
        // C'est ce tableau qui justifie ce remède plutôt qu'un chaînage sur
        // plusieurs pages de tas : le seuil de routage blob (256 o, encodage
        // varint) mord entre 2,7 et 5,1 fois AVANT le plafond de la page de
        // tas (4 076 o, encodage fixint). Il ne reste donc AUCUN intervalle
        // où une valeur serait trop grosse pour v2 sans être déjà partie en
        // colonne blob. Ce n'est pas un raisonnement, c'est une mesure.
        //
        // TEMPORELS EXCLUS À DESSEIN : `Timestamp`, `Date`, `Time`,
        // `Duration`, `ZonedDatetime` sont des scalaires de largeur FIXE —
        // 9 à 29 octets encodés, mesurés. Ils ne peuvent atteindre ni 256 o
        // ni 4 076 o. Les faire passer par cet encodage coûterait une
        // allocation par écriture pour un test toujours faux.
        Value::List(_) | Value::Map(_) | Value::Path { .. } => {
            let encoded = bincode::serde::encode_to_vec(value, bincode::config::standard()).ok()?;
            if encoded.len() > BLOB_COLUMN_THRESHOLD_BYTES {
                let mut out = Vec::with_capacity(1 + encoded.len());
                out.push(BLOB_TAG_BINCODE);
                out.extend_from_slice(&encoded);
                Some(out)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Rebuild a [`Value`] from a tagged blob payload. `None` on an unknown
/// tag or a [`BLOB_TAG_STRING`] payload that is not valid UTF-8 — both
/// signal corruption or forward-compat drift and are treated as "absent"
/// by the read-side router rather than poisoning the caller's property
/// map.
pub(crate) fn decode_blob_payload(bytes: &[u8]) -> Option<Value> {
    let (tag, payload) = bytes.split_first()?;
    match *tag {
        BLOB_TAG_STRING => {
            let s = std::str::from_utf8(payload).ok()?;
            Some(Value::String(s.into()))
        }
        BLOB_TAG_BYTES => Some(Value::Bytes(Arc::from(payload))),
        // (2026-08-19) Pendant de l encodage ci-dessus. Une charge illisible
        // rend None comme les autres cas — meme contrat de tolerance.
        BLOB_TAG_BINCODE => bincode::serde::decode_from_slice(payload, bincode::config::standard())
            .ok()
            .map(|(v, _)| v),
        _ => None,
    }
}

/// Size hint matching the bytes [`encode_blob_payload`] would write,
/// without paying for the copy. The setter uses this to decide routing
/// before allocating the tagged buffer.
pub(crate) fn blob_payload_len(value: &Value) -> Option<usize> {
    match value {
        Value::String(s) => Some(s.as_bytes().len()),
        Value::Bytes(b) => Some(b.len()),
        _ => None,
    }
}

/// True when a [`Value`] qualifies for blob-column routing
/// (String or Bytes whose payload exceeds the threshold). Used by the
/// store's setter fast-path and its `persist_properties` defensive
/// filter — never instantiated directly for an inline boolean, because
/// we want a single place for the routing predicate.
#[cfg(test)]
pub(crate) fn should_route_to_blob(value: &Value) -> bool {
    blob_payload_len(value)
        .map(|n| n > BLOB_COLUMN_THRESHOLD_BYTES)
        .unwrap_or(false)
}

/// Routing table + open-writer cache for blob-typed properties.
pub(crate) struct BlobColumnRegistry {
    /// `(PropertyKey, EntityKind) -> BlobColSpec`. Populated on first
    /// oversized write for a given key, and pre-populated on open via
    /// [`Self::hydrate_from_dict`] from the persisted
    /// `DictSnapshot.blob_columns` list plus the prop-key names.
    by_key: DashMap<(PropertyKey, EntityKind), BlobColSpec>,
    /// `BlobColSpec -> Arc<RwLock<BlobColumnWriter>>`.
    ///
    /// (2026-08-08) Mutex → RwLock. L'argument historique du Mutex —
    /// « write_slot peut faire grandir la mmap via ensure_room, ce qui
    /// invalide les emprunts en cours » — est EXACTEMENT la garantie d'un
    /// RwLock : `write_slot(&mut self)` passe par `.write()`, exclusif de
    /// tout lecteur, donc aucun emprunt de lecture ne survit à un remap.
    /// Ce que le Mutex ajoutait en trop : il sérialisait AUSSI les
    /// lecteurs entre eux. Mesuré en production (agent/system, ~30
    /// requêtes en vol) : chaque hydratation de nœud prend le verrou de
    /// ~30 colonnes ; sur les colonnes chaudes (content, goal…) partagées
    /// par toutes les requêtes concurrentes, le convoi faisait passer des
    /// MATCH de 20 nœuds à 24 s et des MERGE unitaires à 24-40 s.
    /// `read_slot(&self)` sous `.read()` : les lecteurs concourent.
    writers: DashMap<BlobColSpec, Arc<RwLock<BlobColumnWriter>>>,
    /// T2 get_node perf (plan b30650e2, gotcha 7940daa7): per-EntityKind
    /// snapshot of the registered keys, maintained at registration time
    /// — same rationale as `VecColumnRegistry::keys_by_kind`:
    /// `iter_props_for_entity` runs on every `get_node`, and blob
    /// registries hold the most columns (~30 on PO), so the full DashMap
    /// iteration + key clones per node dominated bulk hydration.
    keys_by_kind: DashMap<EntityKind, Arc<Vec<PropertyKey>>>,
}

impl BlobColumnRegistry {
    pub(crate) fn new() -> Self {
        Self {
            by_key: DashMap::new(),
            writers: DashMap::new(),
            keys_by_kind: DashMap::new(),
        }
    }

    /// Record `key` in the per-kind key snapshot (idempotent). Must be
    /// called wherever `by_key` gains an entry.
    fn note_key(&self, key: &PropertyKey, ek: EntityKind) {
        let mut entry = self
            .keys_by_kind
            .entry(ek)
            .or_insert_with(|| Arc::new(Vec::new()));
        if !entry.iter().any(|k| k == key) {
            let mut v: Vec<PropertyKey> = (**entry).clone();
            v.push(key.clone());
            *entry = Arc::new(v);
        }
    }

    /// O(1) snapshot of the keys registered for `ek`.
    fn keys_for(&self, ek: EntityKind) -> Arc<Vec<PropertyKey>> {
        self.keys_by_kind
            .get(&ek)
            .map(|e| Arc::clone(&e))
            .unwrap_or_else(|| Arc::new(Vec::new()))
    }

    /// Rehydrate the registry from persisted specs. Takes the
    /// `blob_columns` list from [`crate::dict::DictSnapshot`] plus the
    /// property-key name table (indexed by `prop_key_id`). For each
    /// spec, opens a writer (the backing files are expected to already
    /// exist from a prior session; missing files are tolerated and
    /// start empty) and records the key → spec mapping.
    ///
    /// Mismatching on-disk headers are also tolerated — the writer
    /// inherits `n_slots=0` + `arena_bytes=0` and overwrites on next
    /// write, which is the same "degrade to absent then re-emit" policy
    /// as the vec-column registry.
    pub(crate) fn hydrate_from_dict(
        &self,
        sub: &SubstrateFile,
        prop_key_names: &[String],
        specs: &[BlobColSpec],
    ) -> SubstrateResult<()> {
        for spec in specs {
            let Some(name) = prop_key_names.get(spec.prop_key_id as usize) else {
                // Persisted spec references a prop_key_id past the
                // known table. Dict corruption or forward-compat drift
                // — skip with a log, same policy as vec_column_registry.
                tracing::warn!(
                    target: "substrate::blob_columns",
                    "hydrate: blob_column spec references unknown prop_key_id {} \
                     (only {} names known); skipping",
                    spec.prop_key_id,
                    prop_key_names.len()
                );
                continue;
            };
            let key = PropertyKey::new(name.as_str());
            let writer = BlobColumnWriter::create(sub, *spec)?;
            self.writers.insert(*spec, Arc::new(RwLock::new(writer)));
            self.note_key(&key, spec.entity_kind);
            self.by_key.insert((key, spec.entity_kind), *spec);
        }
        Ok(())
    }

    /// Look up the spec for a (key, entity_kind) pair, if any. This is
    /// the cheap fast-path used by the read-side router — on a miss
    /// the caller falls through to the in-memory property map.
    pub(crate) fn spec_for(&self, key: &PropertyKey, ek: EntityKind) -> Option<BlobColSpec> {
        self.by_key.get(&(key.clone(), ek)).map(|r| *r)
    }

    /// Route an oversized `Value::String` / `Value::Bytes` write for a
    /// node/edge slot. Creates the blob column on first use;
    /// `prop_key_id` must already be interned in the store's
    /// `PropKeyRegistry`. Empty payloads are rejected by the underlying
    /// writer (absence is encoded by `len == 0`, so a "present but
    /// empty" payload cannot be round-tripped).
    pub(crate) fn write(
        &self,
        sub: &SubstrateFile,
        key: &PropertyKey,
        ek: EntityKind,
        prop_key_id: u16,
        slot: u32,
        bytes: &[u8],
    ) -> SubstrateResult<()> {
        if bytes.is_empty() {
            return Err(SubstrateError::WalBadFrame(
                "blob_column: cannot store empty payload".into(),
            ));
        }
        let spec = BlobColSpec {
            prop_key_id,
            entity_kind: ek,
        };

        // Register the spec on first sighting. Unlike vec_columns
        // there is no dim/dtype to enforce — only the (key, ek) pair
        // identifies the column.
        self.note_key(key, ek);
        self.by_key.entry((key.clone(), ek)).or_insert(spec);

        // Get-or-create the writer.
        let writer = if let Some(w) = self.writers.get(&spec) {
            w.clone()
        } else {
            let w = Arc::new(RwLock::new(BlobColumnWriter::create(sub, spec)?));
            self.writers
                .entry(spec)
                .or_insert_with(|| w.clone())
                .clone()
        };

        writer.write().write_slot(slot, bytes)
    }

    /// Marque ABSENTE la valeur blob de `(key, ek)` pour `slot`.
    ///
    /// (2026-08-20) Pendant obligatoire de [`Self::write`] : sans lui, une
    /// valeur longue MASQUE indéfiniment toute réécriture plus courte, l'ordre
    /// de lecture consultant les colonnes blob avant la chaîne v2. Voir
    /// [`crate::blob_column::BlobColumnWriter::clear_slot`].
    ///
    /// GARDE À COÛT NUL : sans colonne enregistrée pour cette clé — le cas de
    /// l'écrasante majorité des propriétés, qui n'ont jamais vu de valeur de
    /// plus de 256 octets — on sort sur un seul `get` de DashMap, avant tout
    /// verrou de colonne.
    ///
    /// VERROUS : ne prend QUE le `RwLock` de la colonne, et seulement après
    /// avoir relâché les deux gardes de DashMap. Aucun verrou de zone, aucun
    /// `substrate.lock()` — contrairement à [`Self::write`], dont l'ordre de
    /// prise est donc un sur-ensemble strict. Aucun cycle nouveau.
    pub(crate) fn clear(
        &self,
        key: &PropertyKey,
        ek: EntityKind,
        slot: u32,
    ) -> SubstrateResult<()> {
        let Some(spec) = self.spec_for(key, ek) else {
            return Ok(());
        };
        // Le garde de DashMap est relâché à la fin de CETTE instruction, avant
        // que le verrou de colonne ne soit pris ci-dessous.
        let writer = match self.writers.get(&spec) {
            Some(w) => w.clone(),
            None => return Ok(()),
        };
        writer.write().clear_slot(slot)
    }

    /// Copy the payload for `slot` out of the column registered for
    /// `(key, ek)`. Returns `None` if the key is not a blob column, if
    /// the slot is past the writer's high-water mark, or if the slot
    /// is marked absent (`len == 0`).
    ///
    /// The returned `Arc<[u8]>` is an owned copy of the **raw bytes
    /// including the 1-byte type tag** — the writer's mmap borrow
    /// cannot escape the lock guard. Callers that want a fully
    /// reconstructed [`Value`] should use [`Self::read_value`] instead,
    /// which strips the tag.
    pub(crate) fn read(&self, key: &PropertyKey, ek: EntityKind, slot: u32) -> Option<Arc<[u8]>> {
        let spec = self.spec_for(key, ek)?;
        let writer = self.writers.get(&spec)?.clone();
        let guard = writer.read();
        let data: &[u8] = guard.read_slot(slot)?;
        Some(Arc::from(data))
    }

    /// Read-through that reconstructs the original [`Value`] variant
    /// (String or Bytes) from the tagged payload. Returns `None` when
    /// the slot is absent, the tag is unknown, or a `String` payload is
    /// not valid UTF-8 (treated as "absent" rather than a hard error —
    /// same degradation policy as [`Self::read`]).
    pub(crate) fn read_value(&self, key: &PropertyKey, ek: EntityKind, slot: u32) -> Option<Value> {
        let raw = self.read(key, ek, slot)?;
        decode_blob_payload(&raw)
    }

    /// (O1) Vue zero-copy sur un payload `String` : rend la garde de
    /// lecture de la colonne (comptée par `Arc`, feature `arc_lock`) et le
    /// slot, APRÈS avoir validé présence, tag [`BLOB_TAG_STRING`] et UTF-8.
    /// `None` sur colonne absente, slot absent, tag non-String ou UTF-8
    /// invalide — même politique de dégradation que [`Self::read`].
    ///
    /// VERROUS : mêmes prises que [`Self::read`] (gardes DashMap relâchées
    /// avant la garde de colonne) ; la garde de LECTURE reste tenue par la
    /// vue rendue — un écrivain de CETTE colonne attendra tant que la vue
    /// vit. Bail court exigé (contrat de [`crate::view`]).
    pub(crate) fn read_str_view(
        &self,
        key: &PropertyKey,
        ek: EntityKind,
        slot: u32,
    ) -> Option<crate::view::BlobStrView> {
        let spec = self.spec_for(key, ek)?;
        let writer = self.writers.get(&spec)?.clone();
        let guard = writer.read_arc();
        {
            let bytes = guard.read_slot(slot)?;
            let (tag, payload) = bytes.split_first()?;
            if *tag != BLOB_TAG_STRING {
                return None;
            }
            std::str::from_utf8(payload).ok()?;
        }
        Some(crate::view::BlobStrView::new(guard, slot))
    }

    /// (VWS) Garde de lecture `Arc` de la colonne String `(key, ek)` — UNE
    /// prise pour tout un balayage (contrat « une garde par zone » de
    /// `SubstrateStore::for_each_node_prop_str_view`). `None` si la clé n'a
    /// jamais débordé vers une colonne blob pour ce genre d'entité.
    ///
    /// VERROUS : mêmes prises que [`Self::read_str_view`] (gardes DashMap
    /// relâchées avant la prise de colonne). L'appelant tient la garde le
    /// temps du balayage — un écrivain de CETTE colonne attendrait d'ici
    /// là : bail de balayage, pas de rétention au-delà.
    pub(crate) fn str_col_read_guard(
        &self,
        key: &PropertyKey,
        ek: EntityKind,
    ) -> Option<parking_lot::lock_api::ArcRwLockReadGuard<parking_lot::RawRwLock, BlobColumnWriter>>
    {
        let spec = self.spec_for(key, ek)?;
        let writer = self.writers.get(&spec)?.clone();
        Some(writer.read_arc())
    }

    /// Lit les valeurs d'un LOT de slots d'UNE colonne, sous une seule prise du
    /// verrou de colonne, et décode DANS le garde.
    ///
    /// POURQUOI (2026-09-12). `read_value` refait, PAR CELLULE : un `spec_for`
    /// (DashMap), un `writers.get` (DashMap), une prise du `RwLock` de colonne,
    /// puis `Arc::from(data)` — une copie allouée dont personne ne veut, car le
    /// garde interdit d'emprunter l'arène — avant `decode_blob_payload`, qui
    /// alloue À NOUVEAU la `String`. Cinq gestes partagés par valeur, mesurés
    /// au total ~1 µs/cellule : un filtre qui touche 347 970 lignes payait
    /// 827 ms pour UNE propriété (`scripts/bench-filtre-propriete.sh`, mesure B).
    /// Ici : un seul `spec_for`, un seul `writers.get`, un seul verrou, et UNE
    /// allocation par valeur (celle du `Value` rendu) au lieu de deux.
    ///
    /// CONTRAT — identique à `read_value`, au lot près : `slots[i] == None`
    /// signifie « ne pas lire cette case » (nœud mort, hors borne, ou déjà servi
    /// par une zone antérieure) et rend `None`. Un slot ABSENT de la colonne
    /// (sentinelle `len == 0`) rend `None` lui aussi : c'est ce `None`-là qui
    /// autorise l'appelant à reprendre le fil dans la chaîne v2 puis la DashMap,
    /// exactement comme le faisait `read_value`.
    ///
    /// Le tableau rendu a TOUJOURS la longueur de `slots` : le lot ne peut pas
    /// désaligner les indices.
    pub(crate) fn read_values_avec_spec(
        &self,
        spec: BlobColSpec,
        slots: &[Option<u32>],
    ) -> Vec<Option<Value>> {
        // Le garde de `writers` est relâché à la fin de CETTE instruction (le
        // temporaire du `get`), avant que le verrou de colonne ne soit pris —
        // même ordre que `read`, et pas de verrou de registry tenu pendant la
        // lecture de l'arène.
        let writer = match self.writers.get(&spec) {
            Some(w) => w.clone(),
            // Colonne déclarée sans écrivain ouvert : même contrat que
            // `read_value` (qui sort par `?` sur le `get`) — tout est absent.
            None => return vec![None; slots.len()],
        };
        let guard = writer.read();
        slots
            .iter()
            .map(|s| {
                let s = (*s)?;
                let data = guard.read_slot(s)?;
                decode_blob_payload(data)
            })
            .collect()
    }

    /// Verdicts d'un prédicat TEXTE pour un LOT de slots, sous une seule prise
    /// du verrou de colonne, SANS allouer de `Value`.
    ///
    /// `None` (extérieur du tableau) = « pas de verdict pour cette case » :
    /// le slot est absent (sentinelle `len == 0`) **ou** sa charge n'est pas
    /// étiquetée `String`. L'appelant reprend alors la chaîne des zones — c'est
    /// indispensable pour les charges `BINCODE`, qui peuvent porter une chaîne
    /// et que seul `decode_blob_payload` sait trancher.
    pub(crate) fn verdicts_texte_avec_spec(
        &self,
        spec: BlobColSpec,
        slots: &[Option<u32>],
        predicats: &[obrain_core::graph::traits::PredicatTexte<'_>],
    ) -> Vec<Vec<Option<bool>>> {
        let mut out: Vec<Vec<Option<bool>>> = vec![vec![None; slots.len()]; predicats.len()];
        let Some(writer) = self.writers.get(&spec).map(|w| w.clone()) else {
            return out;
        };
        let guard = writer.read();
        for (i, s) in slots.iter().enumerate() {
            let Some(slot) = *s else { continue };
            let Some(data) = guard.read_slot(slot) else {
                continue;
            };
            // Charge étiquetée : 1 octet de tag, puis les octets. Une charge
            // non étiquetée `String` (BINCODE, qui peut porter une chaîne) reste
            // sans verdict : l'appelant reprend la chaîne des zones.
            let Some((tag, charge)) = data.split_first() else {
                continue;
            };
            if *tag != BLOB_TAG_STRING {
                continue;
            }
            for (k, predicat) in predicats.iter().enumerate() {
                out[k][i] = predicat.verdict(charge);
            }
        }
        out
    }
    /// T17k — bulk iteration of every registered blob column whose
    /// `EntityKind` matches `ek`, yielding the `(PropertyKey, Value)`
    /// pair when `slot` has a live payload on that column. Absent
    /// payloads (len == 0 sentinel) are skipped.
    ///
    /// Unlike vec_columns, blob columns HAVE a presence sentinel
    /// (idx entry with len==0 = absent), so this iterator is
    /// unambiguous: only truly-live payloads are surfaced.
    ///
    /// Cost: `O(num_matching_cols × (lock + arena read + decode))`.
    /// On PO Node typically ~30 blob cols → ~30 μs with UTF-8 decodes.
    pub(crate) fn iter_props_for_entity(
        &self,
        ek: EntityKind,
        slot: u32,
    ) -> Vec<(PropertyKey, Value)> {
        // T2 get_node perf: O(1) Arc-cloned key snapshot instead of a
        // full DashMap iteration + key clones per call. Absent slots
        // already short-circuit inside `read_slot` (len == 0) before
        // any copy, so no further presence optimisation is needed here.
        let keys = self.keys_for(ek);
        keys.iter()
            .filter_map(|k| Some((k.clone(), self.read_value(k, ek, slot)?)))
            .collect()
    }

    /// Iterate every currently-registered spec. Used by
    /// `SubstrateStore::build_dict_snapshot` to populate the v4
    /// `blob_columns` list.
    pub(crate) fn specs_snapshot(&self) -> Vec<BlobColSpec> {
        let mut out: Vec<BlobColSpec> = self.writers.iter().map(|e| *e.key()).collect();
        // Deterministic order: (entity_kind, prop_key_id) matches the
        // vec-column snapshot convention for easier diffing.
        out.sort_by_key(|s| (s.entity_kind as u8, s.prop_key_id));
        out
    }

    /// Durability barrier: walk every open writer and `sync` it
    /// (both CRCs + header + msync + fsync on idx **and** dat). Called
    /// from `SubstrateStore::flush` so blob columns become durable at
    /// the same cadence as the rest of the substrate.
    pub(crate) fn sync_all(&self) -> SubstrateResult<()> {
        for entry in self.writers.iter() {
            entry.value().write().sync()?;
        }
        Ok(())
    }
}

/// Chantier SlotRemappable — chemin VIF, pendant blob de l'impl sur
/// `VecColumnRegistry` (voir sa doc) : seul le répertoire idx est
/// permuté, l'arène adressée par offset ne bouge pas.
impl crate::slot_remap::SlotRemappable for BlobColumnRegistry {
    fn remap_name(&self) -> &'static str {
        "blob_columns"
    }

    fn remap_node_slots(
        &self,
        substrate: &Arc<parking_lot::Mutex<SubstrateFile>>,
        old_to_new: &[u32],
        epoch: u32,
        _ctx: crate::slot_remap::RemapContext,
    ) -> SubstrateResult<()> {
        for entry in self.writers.iter() {
            if entry.key().entity_kind != EntityKind::Node {
                continue;
            }
            // Verrou pris PAR COLONNE, jamais tenu par l'appelant (cf. la
            // doc du trait `SlotRemappable`) — meme discipline que le
            // registre veccol.
            let sub = substrate.lock();
            entry
                .value()
                .write()
                .remap_node_slots(&sub, old_to_new, epoch)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob_column::BlobColumnReader;
    use crate::file::SubstrateFile;

    fn pk(s: &str) -> PropertyKey {
        PropertyKey::new(s)
    }

    #[test]
    fn write_then_read_node_roundtrip() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg = BlobColumnRegistry::new();

        let payload = b"hello world, this is a chat payload blob".to_vec();
        reg.write(&sub, &pk("data"), EntityKind::Node, 0, 5, &payload)
            .unwrap();

        let got = reg.read(&pk("data"), EntityKind::Node, 5).unwrap();
        assert_eq!(got.as_ref(), &payload[..]);
    }

    #[test]
    fn read_missing_key_returns_none() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg = BlobColumnRegistry::new();
        let payload = b"x".repeat(512);
        reg.write(&sub, &pk("data"), EntityKind::Node, 0, 0, &payload)
            .unwrap();
        assert!(reg.read(&pk("unknown"), EntityKind::Node, 0).is_none());
    }

    #[test]
    fn iter_props_for_entity_round_trip() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg = BlobColumnRegistry::new();
        // Payloads must exceed BLOB_COLUMN_THRESHOLD_BYTES (256) to
        // be routed to blob_columns (otherwise encode_blob_payload
        // returns None, signalling "goes to PropsZone v2 instead").
        let long_path = format!("/CHEMIN/UTILISATEUR/projects/{}", "a/".repeat(200));
        let long_title = format!("Incendie du bar Le Constellation {}", "x".repeat(300));
        let long_fp = format!("[{}]", "0.86,0.81,0.79,".repeat(30));
        let s = encode_blob_payload(&Value::from(long_path.as_str())).unwrap();
        reg.write(&sub, &pk("path"), EntityKind::Node, 0, 7, &s)
            .unwrap();
        let s2 = encode_blob_payload(&Value::from(long_title.as_str())).unwrap();
        reg.write(&sub, &pk("title"), EntityKind::Node, 1, 7, &s2)
            .unwrap();
        let s3 = encode_blob_payload(&Value::from(long_fp.as_str())).unwrap();
        reg.write(&sub, &pk("cc_fingerprint"), EntityKind::Node, 2, 7, &s3)
            .unwrap();
        let long_edge = format!("CALLS {}", "x".repeat(300));
        let s4 = encode_blob_payload(&Value::from(long_edge.as_str())).unwrap();
        reg.write(&sub, &pk("edge_label"), EntityKind::Edge, 3, 7, &s4)
            .unwrap();

        let props = reg.iter_props_for_entity(EntityKind::Node, 7);
        assert_eq!(props.len(), 3, "expected 3 Node blob props, got {props:?}");
        let map: std::collections::HashMap<String, Value> = props
            .into_iter()
            .map(|(k, v)| (k.as_str().to_string(), v))
            .collect();
        assert!(map.contains_key("path"));
        assert!(map.contains_key("title"));
        assert!(map.contains_key("cc_fingerprint"));
        assert!(!map.contains_key("edge_label"));
        if let Some(Value::String(s)) = map.get("title") {
            assert!(s.contains("Constellation"));
        } else {
            panic!("title is not a String variant");
        }
    }

    #[test]
    fn iter_props_for_entity_skips_absent_slots() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg = BlobColumnRegistry::new();
        let big1 = format!("title-5 {}", "x".repeat(300));
        let big2 = format!("path-9 {}", "x".repeat(300));
        let s1 = encode_blob_payload(&Value::from(big1.as_str())).unwrap();
        let s2 = encode_blob_payload(&Value::from(big2.as_str())).unwrap();
        reg.write(&sub, &pk("title"), EntityKind::Node, 0, 5, &s1)
            .unwrap();
        reg.write(&sub, &pk("path"), EntityKind::Node, 1, 9, &s2)
            .unwrap();
        // Slot 7 has neither (blob uses len=0 sentinel for absent).
        let props_7 = reg.iter_props_for_entity(EntityKind::Node, 7);
        assert!(
            props_7.is_empty(),
            "slot 7 should be empty, got {props_7:?}"
        );
        let props_5 = reg.iter_props_for_entity(EntityKind::Node, 5);
        assert_eq!(props_5.len(), 1);
        assert_eq!(props_5[0].0.as_str(), "title");
    }

    #[test]
    fn iter_props_empty_registry_returns_empty() {
        let reg = BlobColumnRegistry::new();
        assert!(reg.iter_props_for_entity(EntityKind::Node, 42).is_empty());
    }

    #[test]
    fn read_missing_slot_returns_none() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg = BlobColumnRegistry::new();
        let payload = b"some bytes".to_vec();
        reg.write(&sub, &pk("data"), EntityKind::Node, 0, 0, &payload)
            .unwrap();
        assert!(reg.read(&pk("data"), EntityKind::Node, 99).is_none());
    }

    #[test]
    fn node_and_edge_keys_are_independent() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg = BlobColumnRegistry::new();

        let node_payload = b"node-side payload".to_vec();
        let edge_payload = b"edge-side much longer payload with different length".to_vec();
        reg.write(&sub, &pk("data"), EntityKind::Node, 0, 0, &node_payload)
            .unwrap();
        reg.write(&sub, &pk("data"), EntityKind::Edge, 0, 0, &edge_payload)
            .unwrap();

        let got_node = reg.read(&pk("data"), EntityKind::Node, 0).unwrap();
        let got_edge = reg.read(&pk("data"), EntityKind::Edge, 0).unwrap();
        assert_eq!(got_node.as_ref(), &node_payload[..]);
        assert_eq!(got_edge.as_ref(), &edge_payload[..]);
    }

    #[test]
    fn empty_payload_is_rejected() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg = BlobColumnRegistry::new();
        let err = reg
            .write(&sub, &pk("k"), EntityKind::Node, 0, 0, &[])
            .unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("empty"), "unexpected err: {msg}");
    }

    #[test]
    fn variable_length_writes_share_one_column() {
        // Two writes for the same (key, ek) with different payload
        // sizes land in the same column — unlike vec_columns, there is
        // no dim check to reject.
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg = BlobColumnRegistry::new();
        let short = b"tiny".to_vec();
        let long = b"x".repeat(4096);
        reg.write(&sub, &pk("data"), EntityKind::Node, 0, 0, &short)
            .unwrap();
        reg.write(&sub, &pk("data"), EntityKind::Node, 0, 1, &long)
            .unwrap();

        assert_eq!(
            reg.read(&pk("data"), EntityKind::Node, 0).unwrap().as_ref(),
            &short[..]
        );
        assert_eq!(
            reg.read(&pk("data"), EntityKind::Node, 1).unwrap().as_ref(),
            &long[..]
        );
        // Only one spec registered for the key.
        assert_eq!(reg.specs_snapshot().len(), 1);
    }

    #[test]
    fn overwrite_returns_new_value() {
        // Blob columns are append-only: overwriting a slot rewrites
        // the slot entry and appends new bytes; the read must reflect
        // the latest write.
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg = BlobColumnRegistry::new();
        reg.write(&sub, &pk("data"), EntityKind::Node, 0, 4, b"first-value")
            .unwrap();
        reg.write(
            &sub,
            &pk("data"),
            EntityKind::Node,
            0,
            4,
            b"second-and-rather-longer-value",
        )
        .unwrap();
        let got = reg.read(&pk("data"), EntityKind::Node, 4).unwrap();
        assert_eq!(got.as_ref(), b"second-and-rather-longer-value");
    }

    #[test]
    fn specs_snapshot_is_deterministic() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg = BlobColumnRegistry::new();
        let payload = b"some bytes to store".to_vec();
        // Insert in a scrambled order: edge key 2, node key 0, node key 1.
        reg.write(&sub, &pk("b"), EntityKind::Edge, 2, 0, &payload)
            .unwrap();
        reg.write(&sub, &pk("a"), EntityKind::Node, 0, 0, &payload)
            .unwrap();
        reg.write(&sub, &pk("c"), EntityKind::Node, 1, 0, &payload)
            .unwrap();

        let specs = reg.specs_snapshot();
        assert_eq!(specs.len(), 3);
        // Sorted by (entity_kind, prop_key_id): node first.
        assert_eq!(specs[0].entity_kind, EntityKind::Node);
        assert_eq!(specs[0].prop_key_id, 0);
        assert_eq!(specs[1].entity_kind, EntityKind::Node);
        assert_eq!(specs[1].prop_key_id, 1);
        assert_eq!(specs[2].entity_kind, EntityKind::Edge);
        assert_eq!(specs[2].prop_key_id, 2);
    }

    #[test]
    fn sync_all_makes_data_durable_to_a_fresh_reader() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg = BlobColumnRegistry::new();
        let payload = b"persist me across reopen".to_vec();
        reg.write(&sub, &pk("data"), EntityKind::Node, 0, 3, &payload)
            .unwrap();
        reg.sync_all().unwrap();

        let spec = reg
            .spec_for(&pk("data"), EntityKind::Node)
            .expect("spec should be registered");
        let r = BlobColumnReader::open(&sub, spec).unwrap().unwrap();
        assert_eq!(r.n_slots(), 4); // slots 0..=3 allocated
        assert_eq!(r.read_slot(3).unwrap(), &payload[..]);
    }

    #[test]
    fn hydrate_from_dict_recovers_registered_key() {
        // Write + sync a column, then simulate a reopen via a fresh
        // registry that re-hydrates from (spec, prop-key-name) as the
        // dict would carry.
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg1 = BlobColumnRegistry::new();
        let payload = b"content that must survive a reopen cycle".to_vec();
        reg1.write(&sub, &pk("data"), EntityKind::Node, 0, 7, &payload)
            .unwrap();
        reg1.sync_all().unwrap();
        let specs = reg1.specs_snapshot();
        drop(reg1);

        // Fresh registry. Hydrate from what the dict would carry.
        let reg2 = BlobColumnRegistry::new();
        reg2.hydrate_from_dict(&sub, &["data".to_string()], &specs)
            .unwrap();

        let got = reg2.read(&pk("data"), EntityKind::Node, 7).unwrap();
        assert_eq!(got.as_ref(), &payload[..]);
    }

    #[test]
    fn hydrate_tolerates_unknown_prop_key_id() {
        // A persisted spec references a prop_key_id past the name
        // table — should log + skip, not error.
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg = BlobColumnRegistry::new();
        let bogus_spec = BlobColSpec {
            prop_key_id: 42, // past the names table below
            entity_kind: EntityKind::Node,
        };
        // Only one name registered, id 0. Spec's id 42 must be skipped.
        reg.hydrate_from_dict(&sub, &["only_one".to_string()], &[bogus_spec])
            .unwrap();
        assert!(reg.spec_for(&pk("only_one"), EntityKind::Node).is_none());
        assert_eq!(reg.specs_snapshot().len(), 0);
    }

    #[test]
    fn threshold_constant_matches_spec() {
        // Guard against accidentally bumping the threshold without an
        // RFC / plan-level discussion — the value is part of the
        // public routing contract.
        assert_eq!(BLOB_COLUMN_THRESHOLD_BYTES, 256);
    }

    #[test]
    fn encode_blob_payload_routes_big_string() {
        let big = "x".repeat(BLOB_COLUMN_THRESHOLD_BYTES + 1);
        let v = Value::String(big.as_str().into());
        let encoded = encode_blob_payload(&v).expect("big string must route");
        assert_eq!(encoded[0], BLOB_TAG_STRING);
        assert_eq!(&encoded[1..], big.as_bytes());
        assert_eq!(encoded.len(), 1 + big.len());
    }

    #[test]
    fn encode_blob_payload_routes_big_bytes() {
        let big: Vec<u8> = (0..(BLOB_COLUMN_THRESHOLD_BYTES as u32 + 1))
            .map(|i| (i & 0xff) as u8)
            .collect();
        let v = Value::Bytes(Arc::from(big.clone()));
        let encoded = encode_blob_payload(&v).expect("big bytes must route");
        assert_eq!(encoded[0], BLOB_TAG_BYTES);
        assert_eq!(&encoded[1..], big.as_slice());
    }

    #[test]
    fn encode_blob_payload_rejects_below_threshold() {
        // Exactly at threshold → stays inline (strict `>` cutoff).
        let at = "a".repeat(BLOB_COLUMN_THRESHOLD_BYTES);
        assert!(encode_blob_payload(&Value::String(at.as_str().into())).is_none());
        let at_b: Vec<u8> = vec![7u8; BLOB_COLUMN_THRESHOLD_BYTES];
        assert!(encode_blob_payload(&Value::Bytes(Arc::from(at_b))).is_none());
    }

    #[test]
    fn encode_blob_payload_ignores_other_variants() {
        assert!(encode_blob_payload(&Value::Null).is_none());
        assert!(encode_blob_payload(&Value::Int64(12345)).is_none());
        assert!(encode_blob_payload(&Value::Bool(true)).is_none());
        // Vectors go via vec_columns, never here.
        let v: Vec<f32> = vec![0.0; 1024];
        assert!(encode_blob_payload(&Value::Vector(Arc::from(v))).is_none());
    }

    #[test]
    fn decode_blob_payload_roundtrips_string_and_bytes() {
        let s = "hello world — utf-8 éclair".to_string();
        let encoded_s = {
            let mut v = vec![BLOB_TAG_STRING];
            v.extend_from_slice(s.as_bytes());
            v
        };
        match decode_blob_payload(&encoded_s).expect("valid string roundtrip") {
            Value::String(a) => assert_eq!(a.as_str(), s),
            other => panic!("expected String, got {other:?}"),
        }

        let raw: Vec<u8> = (0u8..=255).collect();
        let encoded_b = {
            let mut v = vec![BLOB_TAG_BYTES];
            v.extend_from_slice(&raw);
            v
        };
        match decode_blob_payload(&encoded_b).expect("valid bytes roundtrip") {
            Value::Bytes(a) => assert_eq!(a.as_ref(), raw.as_slice()),
            other => panic!("expected Bytes, got {other:?}"),
        }
    }

    #[test]
    fn decode_blob_payload_rejects_unknown_tag_and_bad_utf8() {
        assert!(decode_blob_payload(&[]).is_none());
        assert!(decode_blob_payload(&[b'Z', 1, 2, 3]).is_none());
        // Invalid UTF-8 under a String tag → None, not a panic.
        assert!(decode_blob_payload(&[BLOB_TAG_STRING, 0xff, 0xfe]).is_none());
    }

    #[test]
    fn should_route_to_blob_matches_threshold() {
        assert!(!should_route_to_blob(&Value::Null));
        assert!(!should_route_to_blob(&Value::String("short".into())));
        let big = "x".repeat(BLOB_COLUMN_THRESHOLD_BYTES + 1);
        assert!(should_route_to_blob(&Value::String(big.as_str().into())));
        let big_b: Vec<u8> = vec![0u8; BLOB_COLUMN_THRESHOLD_BYTES + 1];
        assert!(should_route_to_blob(&Value::Bytes(Arc::from(big_b))));
    }

    #[test]
    fn read_value_reconstructs_variant() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let reg = BlobColumnRegistry::new();
        let s = "éclair ".repeat(128); // > 256 B once encoded
        let big = Value::String(s.as_str().into());
        let payload = encode_blob_payload(&big).unwrap();
        reg.write(&sub, &pk("greeting"), EntityKind::Node, 3, 7, &payload)
            .unwrap();
        let got = reg
            .read_value(&pk("greeting"), EntityKind::Node, 7)
            .expect("read_value must return");
        assert_eq!(got, big);

        // Same column can also store Bytes if the type tag says so.
        // Needs > 256 B to qualify for blob routing.
        let raw: Vec<u8> = (0..(BLOB_COLUMN_THRESHOLD_BYTES as u32 + 128))
            .map(|i| (i & 0xff) as u8)
            .collect();
        let big_b = Value::Bytes(Arc::from(raw.clone()));
        let payload_b = encode_blob_payload(&big_b).unwrap();
        reg.write(&sub, &pk("greeting"), EntityKind::Node, 3, 8, &payload_b)
            .unwrap();
        match reg
            .read_value(&pk("greeting"), EntityKind::Node, 8)
            .unwrap()
        {
            Value::Bytes(b) => assert_eq!(b.as_ref(), raw.as_slice()),
            other => panic!("expected Bytes, got {other:?}"),
        }
    }
}

/// (VWS) Le `&str` d'un slot blob quand le payload est un `String` UTF-8 —
/// tag [`BLOB_TAG_STRING`] vérifié puis validation UTF-8, même politique de
/// dégradation (`None`) que `BlobColumnRegistry::read_str_view`. Emprunte à
/// la colonne : à appeler SOUS sa garde de lecture
/// ([`BlobColumnRegistry::str_col_read_guard`]).
pub(crate) fn blob_slot_str(col: &BlobColumnWriter, slot: u32) -> Option<&str> {
    let bytes = col.read_slot(slot)?;
    let (tag, payload) = bytes.split_first()?;
    if *tag != BLOB_TAG_STRING {
        return None;
    }
    std::str::from_utf8(payload).ok()
}
