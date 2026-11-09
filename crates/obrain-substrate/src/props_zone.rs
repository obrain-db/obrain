//! # Props zone — mmap'd PropertyPage / HeapPage storage (T17c Step 2)
//!
//! The props zone replaces the transitional `props_snapshot.rs` bincode
//! sidecar. It lives in two files inside the substrate directory:
//!
//! * `substrate.props.v2` — array of 4 KiB [`crate::page::PropertyPage`]
//!   slots, one page per `page_idx` (0-based, but `page_idx == 0` is a
//!   reserved dummy so `U48::ZERO` can be used as the "end of chain"
//!   sentinel in `next_page` links and on [`crate::record::NodeRecord::first_prop_off`]).
//! * `substrate.props.heap.v2` — array of 4 KiB [`crate::heap::HeapPage`]
//!   slots holding variable-width payloads referenced by
//!   [`crate::page::HeapRef`] (`StringRef`, `BytesRef`, `ArrStringRef`).
//!
//! ## Multi-page chains
//!
//! Each node's property entries live on a linked list of property pages.
//! The list head is [`crate::record::NodeRecord::first_prop_off`] encoded
//! as `U48::from_u64(page_idx)` (zero = no properties). When the current
//! head is full, [`PropsZone::append_entry`] allocates a **new** head,
//! sets its `next_page` field to the old head, writes the entry there,
//! and returns the new head index. The new-head-first strategy keeps
//! appends O(1) and preserves ordering by recency: the chain walks
//! newest → oldest.
//!
//! ## Heap
//!
//! [`PropsZone::intern_bytes`] atomically appends a `(u32 len, bytes)`
//! record to the current heap tail and returns a [`HeapRef`](crate::page::HeapRef). Entries are
//! never relocated, so refs remain stable for the life of the substrate.
//!
//! ## Read path
//!
//! [`PropsZone::walk_chain`] streams entries through a
//! [`crate::page::PropertyCursor`] on each page in the linked list. It
//! is the zero-copy read primitive substituted for the old
//! `DashMap<NodeId, ...>` hydration done by `props_snapshot.rs`.
//!
//! ## Crash safety
//!
//! The zone is WAL-shadowed — every append is journalled before the
//! mmap'd page is mutated. Recovery rolls the WAL forward.
//!
//! ### CRC32 : ECRIT A CHAQUE APPEND, VERIFIE NULLE PART SUR LE CHEMIN CHAUD
//!
//! Ce paragraphe affirmait que le CRC par page « attrape les ecritures
//! dechirees sur les lectures froides ». C'ETAIT FAUX, et faux de la
//! maniere la plus couteuse : `seal_crc32` est appele a chaque ecriture
//! (31 sites dans le crate), mais `verify_crc32` n'etait appele NULLE
//! PART en dehors des tests. Le CRC etait paye a l'ecriture et jamais
//! encaisse — et la documentation faisait croire a une protection qui
//! n'existait pas.
//!
//! La verification N'A PAS ete mise sur le chemin de lecture, et c'est
//! une decision MESUREE, pas un report :
//!
//! | operation                          | cout       |
//! |------------------------------------|------------|
//! | `read_at` (valeur de ~20 o)        |   1,4 ns   |
//! | `compute_crc32` sur la page 4 KiB  | 106,6 ns   |
//! | lecture + verification             | 103,9 ns   |
//!
//! (mesure n3, `--release`, 200 000 iterations, page de 150 entrees ;
//! debit CRC 35,8 GiB/s — le materiel n'est pas en cause, c'est le
//! rapport 4096 o hashes pour 20 o lus qui l'est.)
//!
//! Verifier a chaque lecture coute donc **x73** sur un chemin dont la
//! raison d'etre est precisement d'etre zero-copie
//! ([`PropsZone::read_heap_view`]). Verifier a l'ouverture, de son cote,
//! forcerait a lire l'INTEGRALITE des zones vives (83 Mo de heap et 2,1 Go
//! de props sur la base de production) et annulerait la paresse du mmap,
//! qui est ce qui rend l'ouverture instantanee.
//!
//! Le CRC est donc verifie LA OU SON COUT EST JUSTIFIE : dans
//! `obrain repair-props`, qui est un balayage integral explicite et
//! deliberement hors du chemin chaud. C'est le seul endroit qui le
//! verifie, et c'est desormais dit.

#![allow(unsafe_code)]

use crate::error::{SubstrateError, SubstrateResult};
use crate::file::{SubstrateFile, ZoneFile};
use crate::heap::{HEAP_PAGE_SIZE, HeapPage};
use crate::page::{
    OwnerKind, PAGE_SIZE, PROP_PAGE_MAGIC_EDGE, PROP_PAGE_MAGIC_NODE, PropertyCursor,
    PropertyEntry, PropertyPage, decode_entry,
};
use crate::record::U48;
use bytemuck;
use obrain_common::utils::hash::{FxHashMap, FxHashSet};

/// Canonical filename of the v2 property zone.
pub const PROPS_V2_FILENAME: &str = "substrate.props.v2";
/// Canonical filename of the v2 heap zone.
pub const PROPS_HEAP_V2_FILENAME: &str = "substrate.props.heap.v2";

/// ── (2026-08-19) MARQUEUR DE MIGRATION **ACHEVÉE** VERS v2 ──
///
/// La présence de `substrate.props.v2` ne dit RIEN de l'état d'une
/// migration : le fichier naît dès la première page écrite, c'est-à-dire
/// dès la PREMIÈRE paire drainée. Une zone v2 porteuse de pages est donc
/// aussi bien une base entièrement migrée qu'une migration coupée en son
/// milieu — deux états qui appellent des lectures OPPOSÉES (sidecar
/// périmé d'un côté, sidecar autoritaire de l'autre) et qu'aucun octet
/// existant ne permettait de distinguer.
///
/// Ce fichier est le signal qui manquait. Il n'est écrit qu'au moment où
/// le sidecar hérité cesse d'être autoritaire — c'est-à-dire par
/// `SubstrateStore::delete_legacy_props_sidecar`, dernière étape de
/// `obrain-migrate --finalize-v2`, et jamais avant. Son ABSENCE est donc
/// l'état par défaut, et l'état par défaut est le prudent : le sidecar
/// est cru sur parole.
///
/// Écrit par tmp + `rename`, avec un contenu NON VIDE : on éprouve la
/// TAILLE et pas seulement l'existence, parce qu'un fichier vide créé par
/// une coupure au mauvais moment mentirait exactement comme
/// `substrate.props` l'a fait dans `a4c1edaf`.
pub const PROPS_V2_MIGRATION_ACHEVEE_FILENAME: &str = "substrate.props.v2.acheve";

/// Initial file size = two 4 KiB slots (dummy sentinel + room for the
/// first real page). Growth is exponential (×1.5) once the first real
/// page overflows.
const INITIAL_BYTES: u64 = (PAGE_SIZE * 2) as u64;
const INITIAL_HEAP_BYTES: u64 = HEAP_PAGE_SIZE as u64;

/// Encode a page id as stored in `next_page` / `first_prop_off`.
///
/// We simply reserve `page_idx == 0` at the file start as a dummy
/// sentinel; real pages live at `page_idx ≥ 1`, which lets us use
/// `U48::ZERO` as the null marker.
#[inline]
pub fn encode_page_id(page_idx: u32) -> U48 {
    U48::from_u64(page_idx as u64)
}

#[inline]
pub fn decode_page_id(u: U48) -> Option<u32> {
    let v = u.to_u64();
    if v == 0 { None } else { Some(v as u32) }
}

// ---------------------------------------------------------------------------
// PropsZone — owner of the two mmap'd zone files.
// ---------------------------------------------------------------------------

/// (T27) Pages contiguës accumulées pour une écriture par blocs (`pwrite`).
#[derive(Default)]
pub(crate) struct TamponPages {
    debut: u32,
    octets: Vec<u8>,
}

/// Taille d'un bloc d'écriture : assez grand pour amortir l'appel système,
/// assez petit pour garder la mémoire bornée.
const BLOC_ECRITURE: usize = 64 << 20;

/// Banc A/B (T27) : `OBRAIN_LOT_MMAP=1` fait écrire les pages d'un lot par la
/// projection, comme avant, au lieu de blocs `pwrite`. Sert à MESURER l'écart
/// dans le même binaire ; le défaut est `pwrite`.
fn lot_par_projection() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("OBRAIN_LOT_MMAP").is_ok_and(|v| v == "1"))
}

/// Page de queue du tas tenue en mémoire pendant une écriture en lot
/// (voir [`PropsZone::intern_bytes_en_lot`]).
#[derive(Default)]
pub(crate) struct TasEnLot {
    page: Option<(u32, HeapPage)>,
    /// La page de queue REPRISE d'avant le lot (réécrite en place, par la
    /// projection) — les pages neuves, elles, partent par blocs.
    reprise: Option<u32>,
    neuves: TamponPages,
}

pub struct PropsZone {
    props: ZoneFile,
    heap: ZoneFile,
    /// Cached `next_page` allocator cursor (monotonic). We never reuse
    /// page_idx values — tombstone-and-rewrite is handled in-page via
    /// the `ENTRY_FLAG_TOMBSTONE` flag, and wholesale page compaction is
    /// a T17d concern.
    next_page_idx: u32,
    next_heap_page_idx: u32,
    /// (feat slots512-wiring, étape 5) Carrier packé « ouvert » :
    /// `(index du carrier, prochaine unité libre 1..=7)`. Réinitialisé à
    /// l'ouverture (None) — un carrier laissé à moitié plein par un arrêt
    /// n'est pas re-rempli après réouverture (correct, monotone ; densité
    /// pleine au sein d'une session d'écriture).
    carrier_pack_ouvert: Option<(u32, u8)>,
}

impl PropsZone {
    /// Open or create both zone files under the given substrate.
    pub fn open(sub: &SubstrateFile) -> SubstrateResult<Self> {
        let mut props = sub.open_named_zone(PROPS_V2_FILENAME)?;
        let mut heap = sub.open_named_zone(PROPS_HEAP_V2_FILENAME)?;
        if props.is_empty() {
            props.grow_to(INITIAL_BYTES)?;
        }
        if heap.is_empty() {
            heap.grow_to(INITIAL_HEAP_BYTES)?;
        }
        let next_page_idx = Self::compute_next_page_idx(&props);
        let next_heap_page_idx = Self::compute_next_heap_page_idx(&heap);
        Ok(Self {
            props,
            heap,
            next_page_idx,
            next_heap_page_idx,
            carrier_pack_ouvert: None,
        })
    }

    /// Find the highest live page index (next allocation target) via
    /// binary search on the page-magic boundary.
    ///
    /// **T17g T2**: formerly a linear scan from end. On large bases with
    /// exponential-growth pre-allocation padding (e.g. Megalaw 103 GB
    /// props.v2 with ~1M pages of trailing padding) the linear scan paged
    /// in the entire padding region, costing hundreds of ms on open.
    ///
    /// Binary search takes O(log N) reads instead of O(tail_padding). It
    /// works because the boundary is sharp by construction: pages
    /// `1..next_page_idx` have a valid magic (either NODE or EDGE), pages
    /// `next_page_idx..total` have none (zero-init padding or freshly
    /// grown allocation). `allocated_page_count` increments monotonically
    /// via `ensure_page_addressable` — no gaps.
    ///
    /// Fast-path: the initial two-slot file returns 1 without any read.
    ///
    /// (2026-08-19) Rendue `pub(crate)` pour l audit d ancres post-rejeu :
    /// c est la SEULE façon d obtenir la frontière « première page jamais
    /// allouée » sans ouvrir une `PropsZone` complète — laquelle CRÉE les
    /// deux fichiers v2 (`open_or_create` + `grow_to`), ce qu un audit en
    /// lecture pure sur une base héritée ne doit surtout pas faire.
    pub(crate) fn compute_next_page_idx(zf: &ZoneFile) -> u32 {
        let bytes = zf.as_slice();
        let total_slots = (bytes.len() / PAGE_SIZE) as u32;
        if total_slots <= 1 {
            return 1;
        }

        // Binary search for the boundary between live pages (magic present)
        // and padding (no magic). Invariant throughout the loop:
        //   - pages `1..=lo` are either known-live or not yet probed
        //     ⇒ the answer is `>= lo + 1` if we haven't seen an empty page
        //   - pages `>= hi` are either known-empty or not yet probed
        // On exit `lo + 1` is the answer (= `next_page_idx`).
        let mut lo: u32 = 0; // highest index known to be LIVE (or 0 if none)
        let mut hi: u32 = total_slots; // lowest index known to be EMPTY (or total if all live)
        while lo + 1 < hi {
            let mid = lo + (hi - lo) / 2;
            debug_assert!(mid >= 1 && mid < total_slots);
            let slot = &bytes[mid as usize * PAGE_SIZE..(mid as usize + 1) * PAGE_SIZE];
            let magic = u32::from_le_bytes(slot[0..4].try_into().unwrap());
            if magic == PROP_PAGE_MAGIC_NODE || magic == PROP_PAGE_MAGIC_EDGE {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        // If we never found a live page, lo stays 0 ⇒ next is 1 (sentinel slot).
        // Otherwise lo is the last live slot ⇒ next is lo + 1.
        lo.saturating_add(1).max(1)
    }

    /// Heap-zone counterpart of [`Self::compute_next_page_idx`]. Same
    /// binary-search logic; different magic + page size.
    ///
    /// **T17g T2**: O(log N) instead of O(tail_padding) linear scan.
    ///
    /// ── CE QUE CETTE RECHERCHE SUPPOSE, ET QUAND C'EST FAUX ──
    ///
    /// Une recherche binaire n'est correcte que sur un predicat MONOTONE.
    /// Le predicat est ici « la page porte le magic », et sa monotonie est
    /// un INVARIANT DE L'ALLOCATEUR, pas une propriete du format : les
    /// pages sont allouees dans l'ordre, donc les vivantes forment un
    /// prefixe contigu. Si un TROU apparait (une page sans magic sous une
    /// page vivante), la recherche rend une frontiere arbitrairement
    /// BASSE — prouve par
    /// `la_recherche_binaire_sous_estime_la_frontiere_si_le_heap_a_un_trou`,
    /// qui rend 4 au lieu de 8.
    ///
    /// Un trou est physiquement possible : la reecriture des pages sales
    /// d'un mmap n'est pas ordonnee, donc un `msync` interrompu peut avoir
    /// ecrit un sous-ensemble QUELCONQUE des pages.
    ///
    /// POURQUOI CE DEFAUT N'EST PAS, MALGRE TOUT, UNE VOIE DE CORRUPTION —
    /// A CONDITION QUE L'ORDRE DE FLUSH SOIT RESPECTE
    /// ([`Self::zones_ordre_durabilite`]) : le heap devient durable
    /// INTEGRALEMENT avant que les props ne le deviennent. Toute page
    /// referencee par une entree props durable appartient donc a une
    /// region deja fsync'ee, c'est-a-dire propre. Un trou ne peut naitre
    /// que dans la region encore sale au moment de la coupure, laquelle
    /// est par construction AU-DESSUS de la derniere region fsync'ee.
    /// La frontiere rabattue reste donc >= toute page durablement
    /// referencee : les pages reutilisees sont des ORPHELINES. On perd de
    /// la place, jamais une valeur.
    ///
    /// Ce raisonnement est la seule chose qui tient ce code : il ne
    /// survivrait PAS a une inversion de l'ordre de flush. La borne est
    /// donc a la fois documentee ici et verrouillee par le test de
    /// durabilite.
    fn compute_next_heap_page_idx(zf: &ZoneFile) -> u32 {
        let bytes = zf.as_slice();
        let total_slots = (bytes.len() / HEAP_PAGE_SIZE) as u32;
        if total_slots == 0 {
            return 0;
        }

        let mut lo: u32 = 0; // highest LIVE (0 if none)
        let mut hi: u32 = total_slots; // lowest EMPTY (total if all live)
        // Heap is 0-indexed (no sentinel slot), so probe mid ∈ [0, total_slots-1].
        // Special-case: lo=0 means "nothing known live yet" — we must probe 0 first.
        if total_slots >= 1 {
            let slot0 = &bytes[..HEAP_PAGE_SIZE];
            let magic0 = u32::from_le_bytes(slot0[0..4].try_into().unwrap());
            if magic0 != crate::heap::HEAP_PAGE_MAGIC {
                return 0; // zone is empty
            }
            lo = 0; // slot 0 is live; answer is >= 1
        }
        while lo + 1 < hi {
            let mid = lo + (hi - lo) / 2;
            debug_assert!(mid < total_slots);
            let slot = &bytes[mid as usize * HEAP_PAGE_SIZE..(mid as usize + 1) * HEAP_PAGE_SIZE];
            let magic = u32::from_le_bytes(slot[0..4].try_into().unwrap());
            if magic == crate::heap::HEAP_PAGE_MAGIC {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo.saturating_add(1)
    }

    /// Get the property zone's current length (test/observability).
    pub fn props_len(&self) -> u64 {
        self.props.len()
    }

    pub fn heap_len(&self) -> u64 {
        self.heap.len()
    }

    pub fn allocated_page_count(&self) -> u32 {
        self.next_page_idx.saturating_sub(1)
    }

    pub fn allocated_heap_page_count(&self) -> u32 {
        self.next_heap_page_idx
    }

    /// Shrink both zone files to their live extent (compact time).
    ///
    /// The exponential ×1.5 pre-allocation pads `substrate.props.v2` well
    /// beyond the last live page (measured 2026-08-31 : 167 MB file for
    /// 26 MB live on a 6,4k-node graph). The padding is zero-init and never
    /// addressed (`next_page_idx` is the first unallocated slot), so
    /// truncating to `next_page_idx * PAGE_SIZE` / `next_heap_page_idx *
    /// HEAP_PAGE_SIZE` reclaims disk and shortens the msync/fsync at
    /// checkpoint/close without touching live data.
    ///
    /// Caller contract : no concurrent reader/writer may hold a slice into
    /// the zones (same discipline as `grow_to`). Safe at `compact`, where
    /// the store is exclusively held.
    pub fn shrink_to_fit(&mut self) -> SubstrateResult<()> {
        let props_target = u64::from(self.next_page_idx) * PAGE_SIZE as u64;
        if props_target < self.props.len() {
            self.props.shrink_to(props_target)?;
        }
        let heap_target = u64::from(self.next_heap_page_idx) * HEAP_PAGE_SIZE as u64;
        if heap_target < self.heap.len() {
            self.heap.shrink_to(heap_target)?;
        }
        Ok(())
    }

    // ---- low-level page I/O -------------------------------------------

    /// Grow the props zone if needed so `page_idx` is addressable, then
    /// return a mutable slice of that page's 4 KiB.
    fn ensure_page_addressable(&mut self, page_idx: u32) -> SubstrateResult<()> {
        let needed = ((page_idx as u64) + 1) * PAGE_SIZE as u64;
        if needed > self.props.len() {
            // Grow exponentially (×1.5, rounded to page boundary).
            let mut new_len = self.props.len().max(INITIAL_BYTES);
            while new_len < needed {
                new_len = (new_len.saturating_mul(3)) / 2;
                new_len = new_len.div_ceil(PAGE_SIZE as u64) * PAGE_SIZE as u64;
            }
            self.props.grow_to(new_len)?;
        }
        Ok(())
    }

    fn ensure_heap_page_addressable(&mut self, page_idx: u32) -> SubstrateResult<()> {
        let needed = ((page_idx as u64) + 1) * HEAP_PAGE_SIZE as u64;
        if needed > self.heap.len() {
            let mut new_len = self.heap.len().max(INITIAL_HEAP_BYTES);
            while new_len < needed {
                new_len = (new_len.saturating_mul(3)) / 2;
                new_len = new_len.div_ceil(HEAP_PAGE_SIZE as u64) * HEAP_PAGE_SIZE as u64;
            }
            self.heap.grow_to(new_len)?;
        }
        Ok(())
    }

    fn page_slice(&self, page_idx: u32) -> Option<&[u8]> {
        let start = page_idx as usize * PAGE_SIZE;
        let end = start + PAGE_SIZE;
        self.props.as_slice().get(start..end)
    }

    fn page_slice_mut(&mut self, page_idx: u32) -> Option<&mut [u8]> {
        let start = page_idx as usize * PAGE_SIZE;
        let end = start + PAGE_SIZE;
        self.props.as_slice_mut().get_mut(start..end)
    }

    fn heap_page_slice(&self, page_idx: u32) -> Option<&[u8]> {
        let start = page_idx as usize * HEAP_PAGE_SIZE;
        let end = start + HEAP_PAGE_SIZE;
        self.heap.as_slice().get(start..end)
    }

    fn heap_page_slice_mut(&mut self, page_idx: u32) -> Option<&mut [u8]> {
        let start = page_idx as usize * HEAP_PAGE_SIZE;
        let end = start + HEAP_PAGE_SIZE;
        self.heap.as_slice_mut().get_mut(start..end)
    }

    /// Read-only accessor returning the [`OwnerKind`] recorded on the
    /// page at `page_idx`, or `None` when the page is out of range or
    /// carries an uninitialised / invalid magic. Introduced by T17f
    /// Step 4 so that tests and tooling can inspect per-page ownership
    /// without reaching into the private `read_page` path.
    pub fn owner_kind_at(&self, page_idx: u32) -> Option<OwnerKind> {
        self.read_page(page_idx).ok()?.owner_kind()
    }

    /// Read the raw [`PropertyPage`] at `page_idx` — test-only public
    /// accessor that allows integration tests to observe tombstones on
    /// the chain (which [`collect_entries`] transparently filters). The
    /// signature is owner-agnostic; callers interpret the page's magic
    /// via [`PropertyPage::owner_kind`] if they care about the owner.
    #[cfg(test)]
    pub fn read_page_for_test(&self, page_idx: u32) -> SubstrateResult<PropertyPage> {
        self.read_page(page_idx)
    }

    fn read_page(&self, page_idx: u32) -> SubstrateResult<PropertyPage> {
        let bytes = self.page_slice(page_idx).ok_or_else(|| {
            SubstrateError::WalBadFrame(format!(
                "props page {page_idx} out of range (props_len = {})",
                self.props.len()
            ))
        })?;
        Ok(*bytemuck::from_bytes::<PropertyPage>(bytes))
    }

    fn write_page(&mut self, page_idx: u32, page: &PropertyPage) -> SubstrateResult<()> {
        self.ensure_page_addressable(page_idx)?;
        let slot = self
            .page_slice_mut(page_idx)
            .expect("just ensured addressable");
        slot.copy_from_slice(bytemuck::bytes_of(page));
        Ok(())
    }

    // ── ÉCRITURE EN LOT (2026-09-16, T27) ─────────────────────────────────
    //
    // Le chemin unitaire (`append_entry` + `intern_bytes`) RELIT la page de
    // tête (4 Ko), l'augmente, RECALCULE son CRC et la RÉÉCRIT — à chaque
    // propriété, et la même chose pour la page de queue du tas à chaque chaîne.
    // Mesuré sur le noyau Linux (1 553 071 nœuds, ≈ 12 M propriétés) : 96 % des
    // 1 293 s d'indexation. Une page construite en mémoire, scellée et écrite
    // UNE fois, donne exactement la même disposition sur disque : c'est ce
    // que font les deux primitives ci-dessous.

    /// Les zones ont-elles grandi sur disque depuis notre projection ?
    pub(crate) fn a_grandi_sur_disque(&self) -> bool {
        self.props.longueur_disque() > self.props.len()
            || self.heap.longueur_disque() > self.heap.len()
    }

    /// Reprojette les deux zones si un autre processus les a agrandies (voir
    /// `ZoneFile::rafraichir`). Rend `true` si l'une a grandi.
    pub(crate) fn rafraichir_projection(&mut self) -> SubstrateResult<bool> {
        let a = self.props.rafraichir()?;
        let b = self.heap.rafraichir()?;
        Ok(a || b)
    }

    /// Fin courante des pages de propriétés et du tas (prochains index libres).
    pub(crate) fn fins(&self) -> (u32, u32) {
        (self.next_page_idx, self.next_heap_page_idx)
    }

    /// (T27) Écrit en PARALLÈLE des blocs de pages déjà construits, chacun à
    /// son index de départ, puis avance les fins de zone. Les blocs doivent
    /// couvrir EXACTEMENT `[fin_courante, fin_props)` et `[fin_tas_courante,
    /// fin_tas)`, sans trou : la fin de zone se retrouve à la réouverture par
    /// dichotomie sur les pages vivantes, qui suppose la contiguïté.
    pub(crate) fn ecrire_blocs_en_lot(
        &mut self,
        props: &[(u32, Vec<u8>)],
        tas: &[(u32, Vec<u8>)],
        fin_props: u32,
        fin_tas: u32,
    ) -> SubstrateResult<()> {
        use rayon::prelude::*;
        let contigu = |blocs: &[(u32, Vec<u8>)], debut: u32, fin: u32, taille: usize| {
            let mut attendu = debut;
            for (d, o) in blocs {
                if o.is_empty() {
                    continue;
                }
                if *d != attendu || o.len() % taille != 0 {
                    return false;
                }
                attendu += (o.len() / taille) as u32;
            }
            attendu == fin
        };
        if !contigu(props, self.next_page_idx, fin_props, PAGE_SIZE)
            || !contigu(tas, self.next_heap_page_idx, fin_tas, HEAP_PAGE_SIZE)
        {
            return Err(SubstrateError::Internal(
                "ecrire_blocs_en_lot : blocs non contigus — refus d'écrire une zone trouée".into(),
            ));
        }
        if fin_props > self.next_page_idx {
            self.ensure_page_addressable(fin_props - 1)?;
        }
        if fin_tas > self.next_heap_page_idx {
            self.ensure_heap_page_addressable(fin_tas - 1)?;
        }
        let (zp, zh) = (&self.props, &self.heap);
        let r1: SubstrateResult<()> = props
            .par_iter()
            .try_for_each(|(d, o)| zp.ecrire_a(u64::from(*d) * PAGE_SIZE as u64, o));
        r1?;
        let r2: SubstrateResult<()> = tas
            .par_iter()
            .try_for_each(|(d, o)| zh.ecrire_a(u64::from(*d) * HEAP_PAGE_SIZE as u64, o));
        r2?;
        self.next_page_idx = fin_props;
        self.next_heap_page_idx = fin_tas;
        Ok(())
    }

    /// Réserve d'un coup la place de `pages_props` pages de propriétés et de
    /// `pages_tas` pages de tas au-delà de la fin courante. Chaque croissance
    /// (`grow_to`) libère et reprojette TOUTE la zone ; à ×1,5 par pas, porter
    /// la zone à ≈ 9 Go en demandait des dizaines pendant un lot. Une
    /// réservation trop large ne coûte qu'une fin de fichier creuse — l'état
    /// normal après une croissance ×1,5 ; trop courte, la croissance à la
    /// demande prend le relais.
    pub(crate) fn reserver(&mut self, pages_props: u32, pages_tas: u32) -> SubstrateResult<()> {
        if pages_props > 0 {
            self.ensure_page_addressable(self.next_page_idx.saturating_add(pages_props - 1))?;
        }
        if pages_tas > 0 {
            self.ensure_heap_page_addressable(
                self.next_heap_page_idx.saturating_add(pages_tas - 1),
            )?;
        }
        Ok(())
    }

    /// Pose une page de propriétés DÉJÀ REMPLIE : alloue son index, la chaîne
    /// sur `suivante` (l'ancienne tête), la scelle et l'écrit. Rend son index.
    /// Même disposition que `allouer_page_neuve` : la page la plus récente est
    /// la tête, et elle pointe vers la précédente.
    pub(crate) fn poser_page_remplie(
        &mut self,
        page: &mut PropertyPage,
        suivante: Option<u32>,
        tampon: &mut TamponPages,
    ) -> SubstrateResult<u32> {
        let idx = self.next_page_idx;
        self.next_page_idx = self
            .next_page_idx
            .checked_add(1)
            .ok_or_else(|| SubstrateError::WalBadFrame("props zone page id overflow".into()))?;
        if let Some(prev) = suivante {
            page.header.next_page = encode_page_id(prev);
        }
        page.seal_crc32();
        self.ensure_page_addressable(idx)?;
        if lot_par_projection() {
            self.write_page(idx, page)?;
            return Ok(idx);
        }
        if tampon.octets.is_empty() {
            tampon.debut = idx;
        }
        debug_assert_eq!(
            tampon.debut as usize + tampon.octets.len() / PAGE_SIZE,
            idx as usize,
            "les pages d'un lot sont contiguës"
        );
        tampon.octets.extend_from_slice(bytemuck::bytes_of(page));
        if tampon.octets.len() >= BLOC_ECRITURE {
            self.vider_pages(tampon)?;
        }
        Ok(idx)
    }

    /// Écrit d'un bloc les pages de propriétés accumulées.
    pub(crate) fn vider_pages(&mut self, tampon: &mut TamponPages) -> SubstrateResult<()> {
        if !tampon.octets.is_empty() {
            self.props
                .ecrire_a(tampon.debut as u64 * PAGE_SIZE as u64, &tampon.octets)?;
            tampon.octets.clear();
        }
        Ok(())
    }

    /// Interne des octets dans le tas en gardant la page de queue EN MÉMOIRE
    /// (`lot`) : elle n'est scellée et écrite que lorsqu'elle est pleine, puis
    /// par [`Self::clore_tas_en_lot`]. Même contrat de taille et mêmes
    /// références que [`Self::intern_bytes`].
    pub(crate) fn intern_bytes_en_lot(
        &mut self,
        lot: &mut TasEnLot,
        bytes: &[u8],
    ) -> SubstrateResult<crate::page::HeapRef> {
        let max_entry = HEAP_PAGE_SIZE - core::mem::size_of::<crate::heap::HeapPageHeader>() - 4;
        if bytes.len() > max_entry {
            return Err(SubstrateError::WalBadFrame(format!(
                "heap intern: entry of {} B exceeds single-page payload ({} B)",
                bytes.len(),
                max_entry
            )));
        }
        // Première utilisation : reprendre la page de queue existante, comme
        // le ferait `intern_bytes`.
        if lot.page.is_none()
            && lot.reprise.is_none()
            && lot.neuves.octets.is_empty()
            && self.next_heap_page_idx > 0
        {
            let tail = self.next_heap_page_idx - 1;
            lot.page = Some((tail, self.read_heap_page(tail)?));
            lot.reprise = Some(tail);
        }
        if let Some((idx, page)) = lot.page.as_mut()
            && let Some(off) = page.append(bytes)
        {
            return Ok(crate::page::HeapRef {
                page_id: *idx,
                offset: off,
            });
        }
        // Page pleine (ou aucune) : on scelle et on écrit la précédente, puis
        // on en ouvre une neuve.
        if let Some((idx, mut page)) = lot.page.take() {
            page.seal_crc32();
            self.poser_page_tas(lot, idx, &page)?;
        }
        let new_idx = self.next_heap_page_idx;
        self.next_heap_page_idx = self
            .next_heap_page_idx
            .checked_add(1)
            .ok_or_else(|| SubstrateError::WalBadFrame("heap zone page id overflow".into()))?;
        let mut page = HeapPage::new(new_idx);
        let off = page
            .append(bytes)
            .expect("fresh heap page fits single entry");
        lot.page = Some((new_idx, page));
        Ok(crate::page::HeapRef {
            page_id: new_idx,
            offset: off,
        })
    }

    /// Scelle et écrit la page de queue tenue par `lot`. À appeler avant tout
    /// `intern_bytes` unitaire (qui relit la queue sur disque) et en fin de lot.
    pub(crate) fn clore_tas_en_lot(&mut self, lot: &mut TasEnLot) -> SubstrateResult<()> {
        if let Some((idx, mut page)) = lot.page.take() {
            page.seal_crc32();
            self.poser_page_tas(lot, idx, &page)?;
        }
        if !lot.neuves.octets.is_empty() {
            let debut = lot.neuves.debut as u64 * HEAP_PAGE_SIZE as u64;
            self.heap.ecrire_a(debut, &lot.neuves.octets)?;
            lot.neuves.octets.clear();
        }
        // Après clôture, la queue est sur disque : un nouveau lot la reprendra.
        lot.reprise = None;
        Ok(())
    }

    /// Une page de tas terminée : la page REPRISE est réécrite en place ; une
    /// page neuve rejoint le bloc, vidé par `pwrite` quand il est plein.
    fn poser_page_tas(
        &mut self,
        lot: &mut TasEnLot,
        idx: u32,
        page: &HeapPage,
    ) -> SubstrateResult<()> {
        if lot.reprise == Some(idx) || lot_par_projection() {
            return self.write_heap_page(idx, page);
        }
        self.ensure_heap_page_addressable(idx)?;
        if lot.neuves.octets.is_empty() {
            lot.neuves.debut = idx;
        }
        debug_assert_eq!(
            lot.neuves.debut as usize + lot.neuves.octets.len() / HEAP_PAGE_SIZE,
            idx as usize,
            "les pages de tas d'un lot sont contiguës"
        );
        lot.neuves
            .octets
            .extend_from_slice(bytemuck::bytes_of(page));
        if lot.neuves.octets.len() >= BLOC_ECRITURE {
            let debut = lot.neuves.debut as u64 * HEAP_PAGE_SIZE as u64;
            self.heap.ecrire_a(debut, &lot.neuves.octets)?;
            lot.neuves.octets.clear();
        }
        Ok(())
    }

    /// Porte de staging du chantier slots 512 (RFC propszone-pages-partagees,
    /// branche feat/slots512-wiring). **DÉFAUT : OFF** — le comportement de
    /// production est inchangé tant qu'elle n'est pas posée. `=1` autorise
    /// l'écriture de débuts de chaîne PACKÉS (carriers 4K à 7 slots de 512).
    /// Ne pas activer en prod sans la garde de version prévue au flip (les
    /// vieux binaires compteraient le magic packé comme padding).
    pub fn slots_512_actifs() -> bool {
        std::env::var("OBRAIN_PROPS_SLOTS_512")
            .map(|v| v == "1")
            .unwrap_or(false)
    }

    /// (feat/slots512-wiring, étape 2a) Écrit un DÉBUT de chaîne packé :
    /// crée un carrier 4K neuf à `carrier_idx` (unité 0 = `CarrierControl`
    /// scellé), réserve l'unité 1 pour `owner` et y écrit `entry` (slot
    /// scellé). Rend l'adresse `UnitéAddr` (u64 48 bits) du slot. Mécanique
    /// posée et testée ici ; le câblage store (début de chaîne quand
    /// `slots_512_actifs()`) l'appellera — non branché à ce stade.
    pub fn ecrire_debut_chainee_pack(
        &mut self,
        owner_id: u32,
        kind: OwnerKind,
        entry: &PropertyEntry,
        carrier_idx: u32,
    ) -> SubstrateResult<u64> {
        self.ensure_page_addressable(carrier_idx)?;
        // Unité 0 — contrôle (unités 1..=7 à réserver).
        let mut ctrl = crate::unite::CarrierControl::new();
        ctrl.reserver_unite()
            .expect("unité 1 réservable sur carrier neuf");
        ctrl.seal_crc32();
        {
            let page = self.page_slice_mut(carrier_idx).expect("adressable");
            page[..core::mem::size_of::<crate::unite::CarrierControl>()]
                .copy_from_slice(bytemuck::bytes_of(&ctrl));
        }
        // Unité 1 — slot de l'owner avec sa première entrée.
        let mut slot = crate::unite::SlotPage::with_owner(owner_id, kind);
        slot.append_entry(entry).map_err(|e| {
            SubstrateError::WalBadFrame(format!("append début de chaîne packé refusé : {e:?}"))
        })?;
        slot.seal_crc32();
        {
            let page = self.page_slice_mut(carrier_idx).expect("adressable");
            let debut = crate::unite::UNITE_SIZE;
            page[debut..debut + crate::unite::UNITE_SIZE]
                .copy_from_slice(bytemuck::bytes_of(&slot));
        }
        crate::unite::encode_slot(carrier_idx, 1)
            .ok_or_else(|| SubstrateError::WalBadFrame("encodage slot impossible".to_string()))
    }

    /// (2026-09-06, fix CONSTAT-REMAP-PROPS-DETACHEMENT) Rafraîchit les
    /// owner stamps (`header.node_id`) d'une chaîne déplacée par un remap de
    /// slots. Parcourt la chaîne depuis `head` : toute page dont le stamp est
    /// l'ANCIEN slot d'un nœud déplacé vers `proprietaire` (c.-à-d.
    /// `old_to_new[stamp] == proprietaire`) est re-stampée à `proprietaire` et
    /// re-scellée (CRC). Une page dont le stamp ne correspond à AUCUN
    /// déplacement (vraie tête parasite d'une autre chaîne, correctif 19/08)
    /// ARRÊTE le parcours sans rien toucher : la garde d'append continuera de
    /// détacher, comportement préservé.
    ///
    /// Rend le nombre de pages re-stampées. Garde anti-cycle (une chaîne
    /// corrompue ne doit jamais boucler) : au-delà de `1_000_000` pages
    /// parcourues, erreur (le remap échoue proprement, rien n'est à moitié
    /// réparé).
    pub fn rafraichir_owners_de_chaine(
        &mut self,
        head: u32,
        proprietaire: u32,
        old_to_new: &[u32],
    ) -> SubstrateResult<u32> {
        const GARDE_PAGES: u32 = 1_000_000;
        let mut cur = Some(head);
        let mut patched = 0u32;
        let mut pages = 0u32;
        while let Some(page_idx) = cur {
            pages += 1;
            if pages > GARDE_PAGES {
                return Err(SubstrateError::WalBadFrame(format!(
                    "rafraichir_owners_de_chaine : chaîne de plus de {GARDE_PAGES} pages \
                     depuis la tête {head} — cycle probable, remap abandonné"
                )));
            }
            let mut page = self.read_page(page_idx)?;
            let stamp = page.header.node_id;
            if stamp == proprietaire {
                cur = decode_page_id(page.header.next_page);
                continue;
            }
            let deplacee_vers_ce_proprietaire =
                (stamp as usize) < old_to_new.len() && old_to_new[stamp as usize] == proprietaire;
            if !deplacee_vers_ce_proprietaire {
                // Page étrangère / parasite : ne pas épisser, s'arrêter.
                break;
            }
            page.header.node_id = proprietaire;
            page.seal_crc32();
            self.write_page(page_idx, &page)?;
            patched += 1;
            cur = decode_page_id(page.header.next_page);
        }
        Ok(patched)
    }

    /// (2026-09-06, fix CONSTAT-REMAP-PROPS-DETACHEMENT) Variante « auto-
    /// guérison à l'ouverture » de [`Self::rafraichir_owners_de_chaine`],
    /// SANS mapping : re-stampe TOUTE la chaîne depuis `head` au slot courant
    /// `proprietaire`. Utilisée uniquement sur les bases FINALISÉES (marqueur
    /// `migration_v2_achevee` posé) après un rejeu qui contenait un record de
    /// permutation : sur ces bases les têtes parasites du régime hérité n'ont
    /// plus cours, tout stamp divergent est donc le résidu d'un remap.
    /// S'arrête (sans erreur) sur la première page illisible/hors zone —
    /// auto-guérison best-effort, jamais fatale à l'ouverture.
    pub fn restamper_apres_rejeu(&mut self, head: u32, proprietaire: u32) -> u32 {
        const GARDE_PAGES: u32 = 1_000_000;
        let mut cur = Some(head);
        let mut patched = 0u32;
        let mut pages = 0u32;
        while let Some(page_idx) = cur {
            pages += 1;
            if pages > GARDE_PAGES {
                break;
            }
            let mut page = match self.read_page(page_idx) {
                Ok(p) => p,
                Err(_) => break,
            };
            if page.header.node_id != proprietaire {
                page.header.node_id = proprietaire;
                page.seal_crc32();
                if self.write_page(page_idx, &page).is_err() {
                    break;
                }
                patched += 1;
            }
            cur = decode_page_id(page.header.next_page);
        }
        patched
    }

    /// (feat slots512-wiring, étape 4) Variante UNITE de
    /// [`Self::rafraichir_owners_de_chaine`] : `head` est l'adresse brute
    /// (page legacy OU slot packé). Parcourt les unités de la chaîne et
    /// re-stampe chaque unité dont `old_to_new[stamp] == proprietaire` (arrêt
    /// sur unité étrangère — sémantique identique au cas legacy).
    pub fn rafraichir_owners_de_chainee_brute(
        &mut self,
        head: u64,
        proprietaire: u32,
        old_to_new: &[u32],
    ) -> SubstrateResult<u32> {
        use crate::unite::{UNITE_SIZE, UniteAddr};
        const GARDE: u32 = 1_000_000;
        let mut cur = if head == 0 { None } else { Some(head) };
        let mut patched = 0u32;
        let mut pages = 0u32;
        while let Some(addr) = cur {
            pages += 1;
            if pages > GARDE {
                return Err(SubstrateError::WalBadFrame(format!(
                    "rafraichir_owners_de_chainee_brute : chaîne > {GARDE} unités depuis {head:#x}"
                )));
            }
            match crate::unite::decode_unite(addr) {
                UniteAddr::Aucune => break,
                UniteAddr::Legacy(idx) => {
                    let mut page = self.read_page(idx)?;
                    let stamp = page.header.node_id;
                    let deplacee = (stamp as usize) < old_to_new.len()
                        && old_to_new[stamp as usize] == proprietaire;
                    let suivante = page.header.next_page.to_u64();
                    if stamp == proprietaire {
                        cur = if suivante == 0 { None } else { Some(suivante) };
                        continue;
                    }
                    if !deplacee {
                        break;
                    }
                    page.header.node_id = proprietaire;
                    page.seal_crc32();
                    self.write_page(idx, &page)?;
                    patched += 1;
                    cur = if suivante == 0 { None } else { Some(suivante) };
                }
                UniteAddr::Slot { carrier, unite } => {
                    let debut = usize::from(unite) * UNITE_SIZE;
                    let mut slot = {
                        let base = self.page_slice(carrier).ok_or_else(|| {
                            SubstrateError::WalBadFrame(format!(
                                "rafraichir_owners_de_chainee_brute : carrier {carrier} hors zone"
                            ))
                        })?;
                        let s: &crate::unite::SlotPage =
                            bytemuck::from_bytes(&base[debut..debut + UNITE_SIZE]);
                        if crate::page::OwnerKind::from_magic(s.header.magic).is_none() {
                            break;
                        }
                        *s
                    };
                    let stamp = slot.header.node_id;
                    let suivante = slot.header.next_page.to_u64();
                    if stamp == proprietaire {
                        cur = if suivante == 0 { None } else { Some(suivante) };
                        continue;
                    }
                    let deplacee = (stamp as usize) < old_to_new.len()
                        && old_to_new[stamp as usize] == proprietaire;
                    if !deplacee {
                        break;
                    }
                    slot.header.node_id = proprietaire;
                    slot.seal_crc32();
                    let base = self.page_slice_mut(carrier).expect("adressable");
                    base[debut..debut + UNITE_SIZE].copy_from_slice(bytemuck::bytes_of(&slot));
                    patched += 1;
                    cur = if suivante == 0 { None } else { Some(suivante) };
                }
            }
        }
        Ok(patched)
    }

    /// (feat slots512-wiring, étape 4) Variante UNITE de
    /// [`Self::restamper_apres_rejeu`] (auto-guérison, sans mapping) — patch
    /// chaque unité (legacy ou slot) au slot courant ; best-effort.
    pub fn restamper_apres_rejeu_brute(&mut self, head: u64, proprietaire: u32) -> u32 {
        use crate::unite::{UNITE_SIZE, UniteAddr};
        const GARDE: u32 = 1_000_000;
        let mut cur = if head == 0 { None } else { Some(head) };
        let mut patched = 0u32;
        let mut pages = 0u32;
        while let Some(addr) = cur {
            pages += 1;
            if pages > GARDE {
                break;
            }
            match crate::unite::decode_unite(addr) {
                UniteAddr::Aucune => break,
                UniteAddr::Legacy(idx) => {
                    let mut page = match self.read_page(idx) {
                        Ok(p) => p,
                        Err(_) => break,
                    };
                    let suivante = page.header.next_page.to_u64();
                    if page.header.node_id != proprietaire {
                        page.header.node_id = proprietaire;
                        page.seal_crc32();
                        if self.write_page(idx, &page).is_err() {
                            break;
                        }
                        patched += 1;
                    }
                    cur = if suivante == 0 { None } else { Some(suivante) };
                }
                UniteAddr::Slot { carrier, unite } => {
                    let debut = usize::from(unite) * UNITE_SIZE;
                    let mut slot = {
                        let base = match self.page_slice(carrier) {
                            Some(b) => b,
                            None => break,
                        };
                        let s: &crate::unite::SlotPage =
                            bytemuck::from_bytes(&base[debut..debut + UNITE_SIZE]);
                        if crate::page::OwnerKind::from_magic(s.header.magic).is_none() {
                            break;
                        }
                        *s
                    };
                    let suivante = slot.header.next_page.to_u64();
                    if slot.header.node_id != proprietaire {
                        slot.header.node_id = proprietaire;
                        slot.seal_crc32();
                        let base = match self.page_slice_mut(carrier) {
                            Some(b) => b,
                            None => break,
                        };
                        base[debut..debut + UNITE_SIZE].copy_from_slice(bytemuck::bytes_of(&slot));
                        patched += 1;
                    }
                    cur = if suivante == 0 { None } else { Some(suivante) };
                }
            }
        }
        patched
    }

    fn read_heap_page(&self, page_idx: u32) -> SubstrateResult<HeapPage> {
        let bytes = self.heap_page_slice(page_idx).ok_or_else(|| {
            SubstrateError::WalBadFrame(format!(
                "heap page {page_idx} out of range (heap_len = {})",
                self.heap.len()
            ))
        })?;
        Ok(*bytemuck::from_bytes::<HeapPage>(bytes))
    }

    fn write_heap_page(&mut self, page_idx: u32, page: &HeapPage) -> SubstrateResult<()> {
        self.ensure_heap_page_addressable(page_idx)?;
        let slot = self
            .heap_page_slice_mut(page_idx)
            .expect("just ensured addressable");
        slot.copy_from_slice(bytemuck::bytes_of(page));
        Ok(())
    }

    // ---- public append / walk API -------------------------------------

    /// Append a property entry for a given owner (node OR edge),
    /// maintaining the multi-page chain rooted at `head`. Returns the
    /// new head page idx (may differ from the input if a fresh page
    /// was allocated).
    ///
    /// The caller stores the returned value into
    /// [`crate::record::NodeRecord::first_prop_off`] (node owner) or
    /// [`crate::record::EdgeRecord::first_prop_off`] (edge owner) via
    /// `encode_page_id(returned_head)`.
    ///
    /// Kind safety: when `head` points to an existing page, the kind
    /// recorded on that page's magic MUST match the `kind` argument.
    /// A mismatch means the caller is about to splice a chain across
    /// owner boundaries (a bug that would corrupt reads through
    /// `walk_chain` for the wrong kind) and is refused loudly.
    pub fn append_entry_inner(
        &mut self,
        owner_id: u32,
        kind: OwnerKind,
        head: Option<u32>,
        entry: &PropertyEntry,
    ) -> SubstrateResult<u32> {
        let need = entry.encoded_len();
        if need > PAGE_SIZE - core::mem::size_of::<crate::page::PropertyPageHeader>() {
            return Err(SubstrateError::WalBadFrame(format!(
                "property entry of {} B exceeds single-page payload",
                need
            )));
        }
        if let Some(h) = head {
            // ── (2026-08-19) UNE TÊTE INEXPLOITABLE VAUT UNE TÊTE ABSENTE ──
            //
            // `head` vient de `NodeRecord::first_prop_off`. Sur toute base
            // antérieure à T17f, ce champ n existait pas sous cette forme
            // (il a été inséré à l offset 24) : un enregistrement hérité rend
            // donc une valeur PARASITE, que l on interprète en index de page.
            //
            // Cette valeur parasite prend quatre visages, et un seul était
            // traité jusqu ici :
            //   · index au-delà de la zone      → `read_page` rendait Err ;
            //   · page en magic 0x0, jamais écrite → Err ;
            //   · page d un autre genre (arête) → Err ;
            //   · page d un AUTRE nœud          → corrigé le 19/08 au matin.
            //
            // MESURE (19/08, base `messages`, 2 890 017 scalaires) :
            //   1 501 040 refus « props page N out of range »
            //     112 280 refus « head page N has invalid magic 0x0 »
            //   soit 1 613 320 propriétés qui n ont JAMAIS pu être écrites en
            //   v2. `set_node_property` les traitait en best-effort (un warn,
            //   puis repli DashMap) — donc la migration se croyait complète
            //   alors qu il en manquait 56 %.
            //
            // Les quatre cas partagent la même vérité : cette tête ne désigne
            // rien qui appartienne à ce propriétaire. Elle ne porte AUCUNE
            // information récupérable pour lui. La seule suite correcte est
            // celle déjà retenue pour le quatrième cas — repartir sur une page
            // neuve, sans chaîner sur une tête qui ment. Rien n est écrit chez
            // le voisin, et la chaîne du nœud redevient saine en la traversant.
            //
            // Échouer, à l inverse, condamnerait toute base héritée à rester
            // en DashMap — ce qui est précisément ce qu on cherche à supprimer.
            let lecture = self.read_page(h);
            let tete = match &lecture {
                Err(raison) => Err(format!("{raison}")),
                Ok(p) => match p.owner_kind() {
                    None => Err(format!(
                        "page en magic {:#x} — jamais écrite",
                        p.header.magic
                    )),
                    Some(k) if k != kind => Err(format!("page de genre {k:?}, attendu {kind:?}")),
                    Some(_) if p.header.node_id != owner_id => Err(format!(
                        "page appartenant au propriétaire {}",
                        p.header.node_id
                    )),
                    Some(_) => Ok(*p),
                },
            };
            let mut page = match tete {
                Ok(p) => p,
                Err(raison) => {
                    tracing::debug!(
                        page = h,
                        demandeur = owner_id,
                        raison = %raison,
                        "tête de chaîne inexploitable — page neuve allouée"
                    );
                    return self.allouer_page_neuve(owner_id, kind, None, entry);
                }
            };
            // Try to append to the current head.
            match page.append_entry(entry) {
                Ok(_) => {
                    page.seal_crc32();
                    self.write_page(h, &page)?;
                    return Ok(h);
                }
                Err(crate::page::PropertyCodecError::PageFull { .. }) => {
                    // Fall through: allocate a new head.
                }
                Err(e) => {
                    return Err(SubstrateError::WalBadFrame(format!(
                        "append_entry encode error: {e}"
                    )));
                }
            }
        }
        self.allouer_page_neuve(owner_id, kind, head, entry)
    }

    /// Alloue une page neuve pour `owner_id`, chaînée sur `head`.
    ///
    /// Extraite d `append_entry_inner` (2026-08-19) pour que la garde de
    /// propriétaire puisse y sauter directement quand la tête fournie
    /// appartient à quelqu un d autre.
    fn allouer_page_neuve(
        &mut self,
        owner_id: u32,
        kind: OwnerKind,
        head: Option<u32>,
        entry: &PropertyEntry,
    ) -> SubstrateResult<u32> {
        // Allocate a fresh page with next_page = old head.
        let new_idx = self.next_page_idx;
        self.next_page_idx = self
            .next_page_idx
            .checked_add(1)
            .ok_or_else(|| SubstrateError::WalBadFrame("props zone page id overflow".into()))?;
        let mut page = PropertyPage::with_owner(owner_id, kind);
        if let Some(prev) = head {
            page.header.next_page = encode_page_id(prev);
        }
        page.append_entry(entry).map_err(|e| {
            SubstrateError::WalBadFrame(format!("append to fresh page failed: {e}"))
        })?;
        page.seal_crc32();
        self.write_page(new_idx, &page)?;
        Ok(new_idx)
    }

    /// Comme [`Self::allouer_page_neuve`] mais chaîne sur une adresse BRUTE
    /// (slot packé possible) — utilisée par le débordement d'une chaîne dont
    /// la tête est un slot (feat slots512-wiring, étape 2).
    fn allouer_page_neuve_sur(
        &mut self,
        owner_id: u32,
        kind: OwnerKind,
        next_brut: Option<u64>,
        entry: &PropertyEntry,
    ) -> SubstrateResult<u32> {
        let new_idx = self.next_page_idx;
        self.next_page_idx = self
            .next_page_idx
            .checked_add(1)
            .ok_or_else(|| SubstrateError::WalBadFrame("props zone page id overflow".into()))?;
        let mut page = PropertyPage::with_owner(owner_id, kind);
        if let Some(prev) = next_brut {
            page.header.next_page = U48::from_u64(prev);
        }
        page.append_entry(entry).map_err(|e| {
            SubstrateError::WalBadFrame(format!("append to fresh page failed: {e}"))
        })?;
        page.seal_crc32();
        self.write_page(new_idx, &page)?;
        Ok(new_idx)
    }

    /// Append a node-owned property entry. See [`append_entry_inner`](Self::append_entry_inner)
    /// for full semantics.
    #[inline]
    pub fn append_entry_node(
        &mut self,
        node_id: u32,
        head: Option<u32>,
        entry: &PropertyEntry,
    ) -> SubstrateResult<u32> {
        self.append_entry_inner(node_id, OwnerKind::Node, head, entry)
    }

    /// Append an edge-owned property entry. See [`append_entry_inner`](Self::append_entry_inner)
    /// for full semantics.
    #[inline]
    pub fn append_entry_edge(
        &mut self,
        edge_id: u32,
        head: Option<u32>,
        entry: &PropertyEntry,
    ) -> SubstrateResult<u32> {
        self.append_entry_inner(edge_id, OwnerKind::Edge, head, entry)
    }

    /// Back-compat alias for [`append_entry_node`](Self::append_entry_node) — preserves the
    /// pre-T17f single-owner-kind API used by existing `store.rs`
    /// callsites and tests. New code should prefer the kind-explicit
    /// variants.
    #[inline]
    pub fn append_entry(
        &mut self,
        node_id: u32,
        head: Option<u32>,
        entry: &PropertyEntry,
    ) -> SubstrateResult<u32> {
        self.append_entry_node(node_id, head, entry)
    }

    /// (feat slots512-wiring, étape 2 — utilisé quand `slots_512_actifs()`)
    /// Append d'une entrée sur une chaîne adressée en BRUT (page legacy 4K ou
    /// slot packé 512). Rend la nouvelle adresse de tête : IDENTIQUE quand
    /// l'entrée tient dans l'unité courante (l'appelant ne met à jour le
    /// record que si elle change) ; sinon :
    /// - aucune tête → début de chaîne packé (env) sinon page legacy ;
    /// - tête inexploitable (owner/kind/magic) → début de chaîne neuf
    ///   (sémantique 19/08 conservée) ;
    /// - unité pleine → page legacy 4K chaînée sur l'adresse précédente
    ///   (mécanique simple ; l'overflow slot→slot reste une étape suivante).
    pub fn append_entree_sur_chainee(
        &mut self,
        owner_id: u32,
        kind: OwnerKind,
        head: Option<u64>,
        entry: &PropertyEntry,
    ) -> SubstrateResult<u64> {
        use crate::unite::{UNITE_SIZE, UniteAddr};
        let need = entry.encoded_len();
        if need > PAGE_SIZE - core::mem::size_of::<crate::page::PropertyPageHeader>() {
            return Err(SubstrateError::WalBadFrame(format!(
                "property entry of {} B exceeds single-page payload",
                need
            )));
        }
        // (A) Début de chaîne.
        if head.is_none() {
            return self.demarrer_chainee(owner_id, kind, entry);
        }
        let addr = head.expect("Some");
        match crate::unite::decode_unite(addr) {
            UniteAddr::Aucune => self.demarrer_chainee(owner_id, kind, entry),
            UniteAddr::Legacy(idx) => {
                let nouveau = self.append_entry_inner(owner_id, kind, Some(idx), entry)?;
                Ok(u64::from(nouveau))
            }
            UniteAddr::Slot { carrier, unite } => {
                let debut = usize::from(unite) * UNITE_SIZE;
                // Lecture + gardes de la tête (sémantique 19/08).
                let copie = {
                    let base = self.page_slice(carrier).ok_or_else(|| {
                        SubstrateError::WalBadFrame(format!(
                            "props chain: carrier {carrier} out of range"
                        ))
                    })?;
                    let slot: &crate::unite::SlotPage =
                        bytemuck::from_bytes(&base[debut..debut + UNITE_SIZE]);
                    let exploitable = crate::page::OwnerKind::from_magic(slot.header.magic)
                        .map(|k| k == kind)
                        .unwrap_or(false)
                        && slot.header.node_id == owner_id;
                    if !exploitable {
                        return self.demarrer_chainee(owner_id, kind, entry);
                    }
                    *slot
                };
                let mut slot = copie;
                match slot.append_entry(entry) {
                    Ok(_) => {
                        slot.seal_crc32();
                        let base = self.page_slice_mut(carrier).expect("adressable");
                        base[debut..debut + UNITE_SIZE].copy_from_slice(bytemuck::bytes_of(&slot));
                        Ok(addr)
                    }
                    Err(crate::page::PropertyCodecError::PageFull { .. }) => {
                        // Débordement → page legacy chaînée sur ce slot.
                        let idx = self.allouer_page_neuve_sur(owner_id, kind, Some(addr), entry)?;
                        Ok(u64::from(idx))
                    }
                    Err(e) => Err(SubstrateError::WalBadFrame(format!(
                        "append packé encode error: {e}"
                    ))),
                }
            }
        }
    }

    /// Début de chaîne : carrier packé si la porte env est ouverte, sinon
    /// page legacy 4K. (Étape 5) Réutilise le carrier packé OUVERT tant qu'il
    /// reste une unité (densité : jusqu'à 7 chaînages par 4K) ; sinon crée un
    /// nouveau carrier.
    fn demarrer_chainee(
        &mut self,
        owner_id: u32,
        kind: OwnerKind,
        entry: &PropertyEntry,
    ) -> SubstrateResult<u64> {
        if Self::slots_512_actifs() {
            // Carrier ouvert avec une unité libre ?
            if let Some((carrier, unite)) = self.carrier_pack_ouvert
                && unite <= crate::unite::SLOTS_UTILISABLES_PAR_CARRIER as u8
            {
                self.ecrire_slot_unite(carrier, unite, owner_id, kind, entry)?;
                let suivant = unite + 1;
                self.maj_controle_carrier(carrier, suivant)?;
                self.carrier_pack_ouvert = Some((carrier, suivant));
                return crate::unite::encode_slot(carrier, unite).ok_or_else(|| {
                    SubstrateError::WalBadFrame("encodage slot impossible".to_string())
                });
            }
            // Nouveau carrier : unité 0 = contrôle, unité 1 = 1re chaîne.
            let carrier = self.next_page_idx;
            self.next_page_idx = self
                .next_page_idx
                .checked_add(1)
                .ok_or_else(|| SubstrateError::WalBadFrame("props zone page id overflow".into()))?;
            self.ensure_page_addressable(carrier)?;
            {
                let mut ctrl = crate::unite::CarrierControl::new();
                ctrl.seal_crc32();
                let base = self.page_slice_mut(carrier).expect("adressable");
                base[..core::mem::size_of::<crate::unite::CarrierControl>()]
                    .copy_from_slice(bytemuck::bytes_of(&ctrl));
            }
            self.ecrire_slot_unite(carrier, 1, owner_id, kind, entry)?;
            self.maj_controle_carrier(carrier, 2)?;
            self.carrier_pack_ouvert = Some((carrier, 2));
            return crate::unite::encode_slot(carrier, 1).ok_or_else(|| {
                SubstrateError::WalBadFrame("encodage slot impossible".to_string())
            });
        }
        let idx = self.allouer_page_neuve(owner_id, kind, None, entry)?;
        Ok(u64::from(idx))
    }

    /// Écrit l'entrée de DÉBUT d'une chaîne dans l'unité `unite` du carrier
    /// `carrier` (slot neuf dédié à l'owner, scellé). Ne touche pas au
    /// contrôle (l'appelant met à jour le compteur d'unités).
    fn ecrire_slot_unite(
        &mut self,
        carrier: u32,
        unite: u8,
        owner_id: u32,
        kind: OwnerKind,
        entry: &PropertyEntry,
    ) -> SubstrateResult<()> {
        use crate::unite::UNITE_SIZE;
        let debut = usize::from(unite) * UNITE_SIZE;
        let mut slot = crate::unite::SlotPage::with_owner(owner_id, kind);
        slot.append_entry(entry).map_err(|e| {
            SubstrateError::WalBadFrame(format!("append slot packé refusé : {e:?}"))
        })?;
        slot.seal_crc32();
        let base = self
            .page_slice_mut(carrier)
            .ok_or_else(|| SubstrateError::WalBadFrame(format!("carrier {carrier} hors zone")))?;
        base[debut..debut + UNITE_SIZE].copy_from_slice(bytemuck::bytes_of(&slot));
        Ok(())
    }

    /// Met à jour le compteur d'unités utilisées du carrier (contrôle scellé).
    fn maj_controle_carrier(&mut self, carrier: u32, nb: u8) -> SubstrateResult<()> {
        let mut ctrl: crate::unite::CarrierControl = {
            let base = self.page_slice(carrier).ok_or_else(|| {
                SubstrateError::WalBadFrame(format!("carrier {carrier} hors zone"))
            })?;
            *bytemuck::from_bytes(&base[..core::mem::size_of::<crate::unite::CarrierControl>()])
        };
        ctrl.nb_unites_utilisees = nb;
        ctrl.seal_crc32();
        let base = self.page_slice_mut(carrier).expect("adressable");
        base[..core::mem::size_of::<crate::unite::CarrierControl>()]
            .copy_from_slice(bytemuck::bytes_of(&ctrl));
        Ok(())
    }

    /// Décode une tête de chaîne LEGACY (index de page u32) en adresse brute
    /// d'unité (bit 47 = 0 → `UniteAddr::Legacy`).
    #[inline]
    fn tete_legacy_u64(head: Option<u32>) -> Option<u64> {
        head.map(u64::from)
    }

    /// (feat slots512-wiring, étape 1) Curseur unité-aware : lit l'unité
    /// pointée par `addr` (page 4K legacy OU slot packé 512) et rend le
    /// curseur sur ses entrées utilisées + l'adresse de l'unité plus ancienne
    /// (`next_page` brut). `Ok(None)` = fin de chaîne. Le chemin legacy est
    /// strictement identique à l'ancien (magic seul vérifié, zéro copie).
    fn unite_cursor<'a>(&'a self, addr: u64) -> SubstrateResult<Option<(PropertyCursor<'a>, u64)>> {
        use crate::unite::UNITE_SIZE;
        match crate::unite::decode_unite(addr) {
            crate::unite::UniteAddr::Aucune => Ok(None),
            crate::unite::UniteAddr::Legacy(idx) => {
                let bytes = self.page_slice(idx).ok_or_else(|| {
                    SubstrateError::WalBadFrame(format!(
                        "props chain: page {idx} out of range (addr {addr:#x})"
                    ))
                })?;
                let page: &PropertyPage = bytemuck::from_bytes(bytes);
                if !page.has_valid_prop_magic() {
                    return Err(SubstrateError::WalBadFrame(format!(
                        "props chain: page {idx} has bad magic {:#x}",
                        page.header.magic
                    )));
                }
                let next = page.header.next_page.to_u64();
                Ok(Some((page.cursor(), next)))
            }
            crate::unite::UniteAddr::Slot { carrier, unite } => {
                let base = self.page_slice(carrier).ok_or_else(|| {
                    SubstrateError::WalBadFrame(format!(
                        "props chain: carrier {carrier} out of range (addr {addr:#x})"
                    ))
                })?;
                let debut = usize::from(unite) * UNITE_SIZE;
                let slot: &crate::unite::SlotPage =
                    bytemuck::from_bytes(&base[debut..debut + UNITE_SIZE]);
                if crate::page::OwnerKind::from_magic(slot.header.magic).is_none() {
                    return Err(SubstrateError::WalBadFrame(format!(
                        "props chain: slot (carrier {carrier}, unité {unite}) bad magic {:#x}",
                        slot.header.magic
                    )));
                }
                let used = &slot.payload[..slot.header.free_offset as usize];
                let next = slot.header.next_page.to_u64();
                Ok(Some((PropertyCursor::au_dessus(used), next)))
            }
        }
    }

    /// Walk the full property chain rooted at `head`, invoking `visit`
    /// for every non-tombstoned entry in chain order (newest → oldest).
    /// Tombstones are skipped transparently.
    ///
    /// If `head` is `None`, the visitor is never called.
    ///
    /// (feat slots512-wiring) Délègue à la variante unité-aware ; les têtes
    /// legacy (index u32) sont converties en adresse d'unité legacy — chemin
    /// inchangé.
    pub fn walk_chain<F>(&self, head: Option<u32>, visit: F) -> SubstrateResult<()>
    where
        F: FnMut(&PropertyEntry),
    {
        self.walk_chain_unite(Self::tete_legacy_u64(head), visit)
    }

    /// Variante unité-aware de [`Self::walk_chain`] : `head` est l'adresse
    /// brute (U48) — page legacy ou slot packé.
    pub(crate) fn walk_chain_unite<F>(&self, head: Option<u64>, mut visit: F) -> SubstrateResult<()>
    where
        F: FnMut(&PropertyEntry),
    {
        let mut cur = head;
        while let Some(addr) = cur {
            let Some((cursor, next)) = self.unite_cursor(addr)? else {
                break;
            };
            for entry in cursor {
                let e = entry.map_err(|e| {
                    SubstrateError::WalBadFrame(format!(
                        "walk_chain: decode error on unit {addr:#x}: {e}"
                    ))
                })?;
                if !e.is_tombstone() {
                    visit(&e);
                }
            }
            cur = if next == 0 { None } else { Some(next) };
        }
        Ok(())
    }

    /// Collect every non-tombstoned entry on the chain into a Vec in
    /// chain order. Convenience helper for tests and migration code —
    /// hot paths should prefer `walk_chain` to avoid the allocation.
    pub fn collect_entries(&self, head: Option<u32>) -> SubstrateResult<Vec<PropertyEntry>> {
        let mut out = Vec::new();
        self.walk_chain(head, |e| out.push(e.clone()))?;
        Ok(out)
    }

    /// T17k — LWW-correct bulk collection: for every `prop_key` on the
    /// chain rooted at `head`, returns whichever entry the scalar
    /// [`Self::get_latest_for_key`] would return.
    ///
    /// Semantics:
    /// - Pages walked `head` (newest) → tail (oldest).
    /// - Within a page, append-order cursor; LAST match per key wins.
    /// - Once a key is decided by the first page that mentions it
    ///   (live or tombstone), older pages cannot overwrite it.
    /// - `live` receives keys whose latest entry is a live value.
    ///   `tombstoned` receives keys whose latest entry is a tombstone.
    /// - A key present in `tombstoned` is NOT in `live` (the two sets
    ///   are disjoint).
    ///
    /// Cost: `O(num_pages × entries_per_page)`. Typical chain on a
    /// well-populated node: 1-3 pages, ≤ 100 entries → ~5-10 μs.
    ///
    /// Unlike [`Self::walk_chain`], tombstones are surfaced (in
    /// `tombstoned`) because bulk callers need them to correctly
    /// shadow lower-priority stores (e.g. the in-memory DashMap).
    pub fn collect_live_props_by_key(
        &self,
        head: Option<u32>,
        live: &mut FxHashMap<u16, PropertyEntry>,
        tombstoned: &mut FxHashSet<u16>,
    ) -> SubstrateResult<()> {
        self.collect_live_props_by_key_unite(Self::tete_legacy_u64(head), live, tombstoned)
    }

    /// Variante unité-aware de [`Self::collect_live_props_by_key`].
    ///
    /// POURQUOI `FxHashMap`/`FxHashSet` et non `std::collections` : la clef
    /// est un `u16` (≤ 65 536 valeurs), le hachage SipHash de `std` est
    /// inutilement coûteux pour ce domaine, et ces cartes sont allouées une
    /// fois PAR NŒUD sur le chemin `get_node` (voir `read_all_properties_for_node`).
    /// Le hachage `foldhash` (Fx) divise ce coût fixe tout en restant
    /// insensible aux attaques DoS par collision — hors propos ici.
    pub(crate) fn collect_live_props_by_key_unite(
        &self,
        head: Option<u64>,
        live: &mut FxHashMap<u16, PropertyEntry>,
        tombstoned: &mut FxHashSet<u16>,
    ) -> SubstrateResult<()> {
        let mut cur = head;
        while let Some(addr) = cur {
            let Some((cursor, next)) = self.unite_cursor(addr)? else {
                break;
            };
            // Within this unit, build a "latest per key" map. Cursor
            // yields oldest → newest, so later writes overwrite.
            let mut latest_in_page: FxHashMap<u16, PropertyEntry> = FxHashMap::default();
            for entry in cursor {
                let e = entry.map_err(|e| {
                    SubstrateError::WalBadFrame(format!(
                        "collect_live_props_by_key: decode error on unit {addr:#x}: {e}"
                    ))
                })?;
                // Only adopt a key that has not been claimed by a
                // newer page already.
                if !live.contains_key(&e.prop_key) && !tombstoned.contains(&e.prop_key) {
                    latest_in_page.insert(e.prop_key, e);
                }
            }
            for (k, e) in latest_in_page {
                if e.is_tombstone() {
                    tombstoned.insert(k);
                } else {
                    live.insert(k, e);
                }
            }
            cur = if next == 0 { None } else { Some(next) };
        }
        Ok(())
    }

    /// T17c Step 3c — Resolve the latest entry (live or tombstone) for
    /// `prop_key` on the chain rooted at `head`, following **LWW
    /// semantics** over the chain ordering:
    ///
    /// - Pages are scanned from `head` (newest page) toward the tail
    ///   (oldest page).
    /// - Within each page, the cursor iterates entries in append order
    ///   (oldest → newest within the page); the last matching entry in
    ///   the page wins.
    /// - The first page that contains any match for `prop_key` decides
    ///   the result; older pages are not consulted (any write on a
    ///   newer page shadows older writes for the same key).
    ///
    /// Returns:
    /// - `Ok(Some(entry))` when a match is found. The caller inspects
    ///   `entry.is_tombstone()` to distinguish "deleted" from "live".
    /// - `Ok(None)` when no entry for `prop_key` exists anywhere on
    ///   the chain.
    /// - `Err(..)` on a bad magic or decode error mid-walk.
    ///
    /// Unlike [`walk_chain`](Self::walk_chain), this helper surfaces
    /// tombstones — they are a valid LWW outcome ("this key was
    /// deleted") that the read path translates into `None`.
    pub fn get_latest_for_key(
        &self,
        head: Option<u32>,
        prop_key: u16,
    ) -> SubstrateResult<Option<PropertyEntry>> {
        self.get_latest_for_key_unite(Self::tete_legacy_u64(head), prop_key)
    }

    /// Variante unité-aware de [`Self::get_latest_for_key`] — `head` est
    /// l'adresse brute (page legacy ou slot packé).
    pub(crate) fn get_latest_for_key_unite(
        &self,
        head: Option<u64>,
        prop_key: u16,
    ) -> SubstrateResult<Option<PropertyEntry>> {
        let mut cur = head;
        while let Some(addr) = cur {
            let Some((cursor, next)) = self.unite_cursor(addr)? else {
                break;
            };
            // ── PASSE 1 : LES CLEFS SEULES (2026-09-12) ──
            //
            // Cette boucle décodait chaque entrée avec `decode_entry` — donc
            // construisait un `PropertyEntry` complet, et allouait un `Vec` pour
            // chaque valeur TABLEAU — alors que seule la clef décide. Sur un
            // nœud à 57 propriétés (dont des listes), chercher UNE clef payait
            // des dizaines de décodages et plusieurs allocations : mesuré
            // 1 450 à 2 561 ns/cellule sur les nœuds `File`, contre 320 ns sur un
            // nœud moyen, pour la MÊME clef.
            //
            // Ici on ne lit que l'en-tête et la longueur : aucune allocation,
            // aucun `PropertyEntry`. On retient l'OFFSET de la DERNIÈRE entrée
            // portant la clef — la sémantique LWW (« la dernière gagne ») est
            // donc conservée à l'identique, et une entrée qui n'est pas la clef
            // cherchée n'est plus décodée du tout.
            let tranche = cursor.tranche();
            let mut off = 0usize;
            let mut dernier_off: Option<usize> = None;
            let mut balayage_sur = true;
            while off < tranche.len() {
                match crate::page::clef_et_taille_entree(&tranche[off..]) {
                    Some((clef, taille)) => {
                        if clef == prop_key {
                            dernier_off = Some(off);
                        }
                        off += taille;
                    }
                    // Entrée tronquée ou tag inconnu : on ne devine pas. La
                    // passe 2 ci-dessous reprend le décodage complet, qui rendra
                    // exactement l'erreur rendue avant ce correctif.
                    None => {
                        balayage_sur = false;
                        break;
                    }
                }
            }
            if balayage_sur {
                if let Some(o) = dernier_off {
                    let (entree, _) = decode_entry(&tranche[o..]).map_err(|e| {
                        SubstrateError::WalBadFrame(format!(
                            "get_latest_for_key: decode error on unit {addr:#x}: {e}"
                        ))
                    })?;
                    return Ok(Some(entree));
                }
            } else {
                // ── PASSE 2 : CHEMIN D'ORIGINE, INCHANGÉ ──
                let mut latest_in_page: Option<PropertyEntry> = None;
                for entry in cursor {
                    let e = entry.map_err(|e| {
                        SubstrateError::WalBadFrame(format!(
                            "get_latest_for_key: decode error on unit {addr:#x}: {e}"
                        ))
                    })?;
                    if e.prop_key == prop_key {
                        latest_in_page = Some(e);
                    }
                }
                if latest_in_page.is_some() {
                    return Ok(latest_in_page);
                }
            }
            cur = if next == 0 { None } else { Some(next) };
        }
        Ok(None)
    }

    // ---- heap API ------------------------------------------------------

    /// Intern arbitrary bytes into the heap zone and return a stable
    /// [`HeapRef`](crate::page::HeapRef). The returned ref is valid for the lifetime of the
    /// substrate (entries are never relocated — compaction is deferred).
    pub fn intern_bytes(&mut self, bytes: &[u8]) -> SubstrateResult<crate::page::HeapRef> {
        let max_entry = HEAP_PAGE_SIZE - core::mem::size_of::<crate::heap::HeapPageHeader>() - 4;
        if bytes.len() > max_entry {
            return Err(SubstrateError::WalBadFrame(format!(
                "heap intern: entry of {} B exceeds single-page payload ({} B)",
                bytes.len(),
                max_entry
            )));
        }
        // Try current tail page; otherwise allocate a new one.
        if self.next_heap_page_idx > 0 {
            let tail = self.next_heap_page_idx - 1;
            let mut page = self.read_heap_page(tail)?;
            if let Some(off) = page.append(bytes) {
                page.seal_crc32();
                self.write_heap_page(tail, &page)?;
                return Ok(crate::page::HeapRef {
                    page_id: tail,
                    offset: off,
                });
            }
        }
        // Allocate a new heap page.
        let new_idx = self.next_heap_page_idx;
        self.next_heap_page_idx = self
            .next_heap_page_idx
            .checked_add(1)
            .ok_or_else(|| SubstrateError::WalBadFrame("heap zone page id overflow".into()))?;
        let mut page = HeapPage::new(new_idx);
        let off = page
            .append(bytes)
            .expect("fresh heap page fits single entry");
        page.seal_crc32();
        self.write_heap_page(new_idx, &page)?;
        Ok(crate::page::HeapRef {
            page_id: new_idx,
            offset: off,
        })
    }

    /// Vue zero-copy : les octets derrière `href`, empruntés directement
    /// à la page heap mmap — aucun octet copié, là où [`Self::read_heap`]
    /// copiait la page entière (4 KiB) puis le payload.
    ///
    /// `None` si la réf sort de la zone allouée, si l'offset est invalide,
    /// ou si l'alignement de page ne se vérifie pas
    /// (`bytemuck::try_from_bytes` — vérifié à l'exécution, jamais de
    /// transmute). Le `&[u8]` vit aussi longtemps que l'emprunt de `self` :
    /// sous la garde de lecture du `RwLock<PropsZone>` du store, le mmap ne
    /// peut pas être remappé (toute croissance passe par `&mut self`).
    pub fn read_heap_view(&self, href: crate::page::HeapRef) -> Option<&[u8]> {
        let bytes = self.heap_page_slice(href.page_id)?;
        let page: &HeapPage = bytemuck::try_from_bytes(bytes).ok()?;
        page.read_at(href.offset)
    }

    /// Read the bytes behind a [`HeapRef`](crate::page::HeapRef). Returns `None` if the ref
    /// points outside the allocated region or the offset is invalid.
    ///
    /// (O1) Réécrit au-dessus de [`Self::read_heap_view`] : une seule copie
    /// (le payload), plus la page 4 KiB intermédiaire.
    pub fn read_heap(&self, href: crate::page::HeapRef) -> Option<Vec<u8>> {
        self.read_heap_view(href).map(|s| s.to_vec())
    }

    /// Compte les pages heap de l'extension VIVANTE dont le CRC32 ne se
    /// verifie pas. Rend `(pages_examinees, pages_en_echec)`.
    ///
    /// ══ POURQUOI LE CRC N'EST VERIFIE QU'ICI, ET NULLE PART AILLEURS ══
    ///
    /// Le chemin de lecture chaud ([`Self::read_heap_view`]) ne verifie PAS
    /// le CRC, et c'est voulu : il faudrait relire les 4 KiB de la page a
    /// chaque resolution de `HeapRef` pour hacher, la ou la lecture utile ne
    /// touche que quelques dizaines d'octets. Le cout serait paye par toutes
    /// les requetes, en permanence, pour attraper un evenement rare.
    ///
    /// Le diagnostic, lui, peut se le permettre : il passe une seule fois,
    /// hors du chemin de service, et l'operateur qui le lance a deja accepte
    /// d'attendre. C'est donc ICI que la verification vit — un seul endroit,
    /// explicitement nomme, plutot que dispersee dans le chemin chaud ou elle
    /// serait tot ou tard retiree « pour la perf ».
    ///
    /// Une page en echec de CRC n'est PAS traitee comme une reference
    /// pendante : ses octets peuvent parfaitement se resoudre (`read_at`
    /// rend `Some`) tout en etant faux. Retirer les entrees qui la
    /// referencent detruirait des donnees peut-etre recuperables. Le compte
    /// est donc RAPPORTE, jamais agi.
    ///
    /// La borne est `next_heap_page_idx` (l'extension allouee), pas la
    /// longueur du fichier : au-dela s'etend le rembourrage de
    /// pre-allocation, qui n'a jamais porte ni magic ni CRC.
    pub fn verifier_crc_pages_heap(&self) -> (u32, u32) {
        let mut examinees = 0u32;
        let mut en_echec = 0u32;
        for page_idx in 0..self.next_heap_page_idx {
            let Some(bytes) = self.heap_page_slice(page_idx) else {
                // Hors de la region mappee : ce n'est pas un echec de CRC
                // mais une extension incoherente, que le scan de references
                // rapportera sous forme de refs pendantes.
                continue;
            };
            let Ok(page) = bytemuck::try_from_bytes::<HeapPage>(bytes) else {
                continue;
            };
            examinees += 1;
            if !page.verify_crc32() {
                en_echec += 1;
            }
        }
        (examinees, en_echec)
    }

    /// Vue EMPRUNTÉE sur la charge d'un `HeapRef`, LUE SUR PLACE dans le mmap.
    ///
    /// POURQUOI (2026-09-12). `read_heap` passait par `read_heap_page`, qui fait
    /// `*bytemuck::from_bytes::<HeapPage>(bytes)` : **une copie de 4 Ko — la page
    /// HEAP entière — pour lire UNE chaîne**. Puis `.read_at(offset).to_vec()`
    /// recopiait la chaîne, avant que `decode_string_ref` n'alloue encore un
    /// `ArcStr`. Soit, par cellule lue : 1 copie de page (4 Ko), 1 `Vec`, 1
    /// `ArcStr`. Mesuré sur le snapshot figé : ~330 ns par cellule pour la seule
    /// phase de lecture du lot, et le décodage des entrées n'y était que pour
    /// ~5 % (A/B entrelacé) — le reste était donc ici.
    ///
    /// Cette vue-ci ne copie RIEN : elle rend la tranche de l'arène mmap que
    /// `HeapPage::read_at` aurait rendue, avec les MÊMES contrôles de bornes
    /// (en-tête, longueur, dépassement). Appelant : garder le verrou de zone
    /// pendant l'usage de la tranche — c'est le contrat de `read_heap_page`.
    #[must_use]
    pub fn read_heap_slice(&self, href: crate::page::HeapRef) -> Option<&[u8]> {
        let bytes = self.heap_page_slice(href.page_id)?;
        let page: &crate::heap::HeapPage = bytemuck::from_bytes(bytes);
        page.read_at(href.offset)
    }

    /// Flush both zones to disk (`msync` + `fsync`).
    ///
    /// ORDRE CRITIQUE — LE HEAP D'ABORD, LES PROPS ENSUITE.
    ///
    /// Les entrees de la zone props contiennent des `HeapRef {page_id,
    /// offset}` : le POINTEUR. Les octets pointes vivent dans la zone heap :
    /// le POINTE. Rendre le pointeur durable avant le pointe ouvre une
    /// fenetre ou un crash laisse sur disque des references pendantes : la
    /// props survit, le heap non, et a la reouverture `HeapPage::read_at`
    /// refuse l'offset (« StringRef {..} out of range ») — valeur perdue.
    ///
    /// Pire : `next_heap_page_idx` est recalcule a l'ouverture comme la
    /// frontiere des pages PORTEUSES DU MAGIC (voir
    /// `compute_next_heap_page_idx` — c'est une recherche de frontiere, PAS
    /// une division de la longueur du fichier). Une queue de heap qui n'a
    /// jamais atteint le disque fait donc repartir l'allocateur EN ARRIERE,
    /// et les ecritures suivantes ECRASENT les pages que les props
    /// survivantes referencent encore. La corruption devient alors
    /// irreversible : les octets ne manquent pas, ils sont REMPLACES.
    ///
    /// Aucune troncature de fichier n'est requise pour cela — il suffit que
    /// la queue soit sale et non synchronisee. C'est le mecanisme reproduit
    /// par
    /// `reouverture_apres_perte_de_la_queue_du_heap_ecrase_les_pages_vivantes`.
    ///
    /// L'ordre inverse (heap puis props) est sur : un crash peut laisser des
    /// octets heap orphelins — du gaspillage, jamais une reference pendante.
    pub fn flush(&self) -> SubstrateResult<()> {
        for zone in self.zones_ordre_durabilite() {
            zone.msync()?;
            zone.fsync()?;
        }
        Ok(())
    }

    /// Les deux zones dans l'ordre ou elles DOIVENT devenir durables :
    /// le POINTE (heap) avant le POINTEUR (props).
    ///
    /// L'ordre est defini ici et nulle part ailleurs : `flush()` comme le
    /// test de durabilite le lisent depuis cette fonction, si bien qu'aucun
    /// des deux ne peut deriver par rapport a l'autre.
    fn zones_ordre_durabilite(&self) -> [&ZoneFile; 2] {
        [&self.heap, &self.props]
    }

    /// (test) Ne rend durable que la PREMIERE zone de l'ordre de flush puis
    /// s'arrete : modelise un crash survenu AU MILIEU de [`Self::flush`].
    #[doc(hidden)]
    pub fn flush_partiel_pour_test(&self) -> SubstrateResult<()> {
        let [premiere, _seconde] = self.zones_ordre_durabilite();
        premiere.msync()?;
        premiere.fsync()?;
        Ok(())
    }

    /// (test) Chemin de la zone qui devient durable EN DERNIER — celle dont
    /// un crash au milieu de [`Self::flush`] fait perdre la queue.
    #[doc(hidden)]
    pub fn fichier_zone_la_moins_durable(&self) -> &std::path::Path {
        let [_premiere, seconde] = self.zones_ordre_durabilite();
        seconde.path()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::{HeapRef, PropertyValue};

    // =================================================================
    // ETAPE 1 — DURABILITE
    //
    // Invariant : apres un crash survenu au milieu de `PropsZone::flush()`,
    // AUCUNE entree props durable ne doit referencer des octets heap qui,
    // eux, ne le sont pas.
    //
    // Pourquoi ne pas simplement faire un SIGKILL ? Parce qu'un SIGKILL ne
    // perd RIEN d'un mmap MAP_SHARED : les pages sales appartiennent au
    // cache de pages du noyau, qui les ecrit meme si le processus meurt.
    // Seule une coupure de courant (ou un crash noyau) perd la queue non
    // fsync'ee. Le crash est donc modelise LA OU IL SE PRODUIT VRAIMENT :
    // au niveau du stockage, en restaurant la zone non encore synchronisee
    // a son dernier etat durable.
    // =================================================================

    fn copier_repertoire(src: &std::path::Path, dst: &std::path::Path) {
        std::fs::create_dir_all(dst).unwrap();
        for e in std::fs::read_dir(src).unwrap() {
            let e = e.unwrap();
            if e.file_type().unwrap().is_file() {
                std::fs::copy(e.path(), dst.join(e.file_name())).unwrap();
            }
        }
    }

    #[test]
    fn crash_au_milieu_du_flush_ne_laisse_aucune_reference_pendante() {
        const N: usize = 20;
        const TAILLE: usize = 200;

        let sf = SubstrateFile::open_tempfile().unwrap();
        let mut pz = PropsZone::open(&sf).unwrap();

        // ---- lot 1 : ecrit, puis rendu ENTIEREMENT durable.
        let mut head = None;
        for i in 0..N {
            let s = format!("lot1-{:03}-{}", i, "a".repeat(TAILLE));
            let href = pz.intern_bytes(s.as_bytes()).unwrap();
            let e = PropertyEntry::new(i as u16, PropertyValue::StringRef(href));
            head = Some(pz.append_entry(1, head, &e).unwrap());
        }
        pz.flush().unwrap();
        assert_eq!(
            head,
            Some(1),
            "le lot 1 doit tenir sur une seule page props"
        );

        // Image disque de l'etat PLEINEMENT durable.
        let dir_durable = tempfile::tempdir().unwrap();
        copier_repertoire(sf.path(), dir_durable.path());

        // ---- lot 2 : ecrit, puis CRASH au milieu de flush().
        for i in 0..N {
            let s = format!("lot2-{:03}-{}", i, "b".repeat(TAILLE));
            let href = pz.intern_bytes(s.as_bytes()).unwrap();
            let e = PropertyEntry::new((N + i) as u16, PropertyValue::StringRef(href));
            head = Some(pz.append_entry(1, head, &e).unwrap());
        }
        assert_eq!(head, Some(1), "le lot 2 doit rester sur la meme page props");

        // Seule la PREMIERE zone de l'ordre de flush atteint le disque.
        pz.flush_partiel_pour_test().unwrap();
        let nom_perdue = pz
            .fichier_zone_la_moins_durable()
            .file_name()
            .unwrap()
            .to_owned();

        // ---- image disque APRES le crash : l'etat courant pour tout, sauf
        // la zone qui n'a pas eu le temps d'etre synchronisee — restauree a
        // son dernier etat durable (celui du lot 1).
        let dir_crash = tempfile::tempdir().unwrap();
        copier_repertoire(sf.path(), dir_crash.path());
        std::fs::copy(
            dir_durable.path().join(&nom_perdue),
            dir_crash.path().join(&nom_perdue),
        )
        .unwrap();

        drop(pz);

        // ---- reouverture et audit de TOUTES les references durables.
        let sf2 = SubstrateFile::open(dir_crash.path()).unwrap();
        let pz2 = PropsZone::open(&sf2).unwrap();
        let entrees = pz2.collect_entries(Some(1)).unwrap();

        let mut pendantes: Vec<(u16, HeapRef)> = vec![];
        let mut resolues = 0usize;
        for e in &entrees {
            if let PropertyValue::StringRef(h) = e.value {
                match pz2.read_heap(h) {
                    Some(_) => resolues += 1,
                    None => pendantes.push((e.prop_key, h)),
                }
            }
        }

        assert!(
            pendantes.is_empty(),
            "{} reference(s) pendante(s) apres un crash au milieu de flush() \
             — zone perdue = {:?}, {} entrees durables, {} resolues : {:?}",
            pendantes.len(),
            nom_perdue,
            entrees.len(),
            resolues,
            &pendantes[..pendantes.len().min(5)]
        );
        assert!(
            resolues > 0,
            "l'audit doit avoir resolu au moins une reference (sinon il ne \
             prouve rien) — {} entrees durables",
            entrees.len()
        );
    }

    // =================================================================
    // ETAPE 2 — LA CAUSE PREMIERE : L'ALLOCATEUR QUI REPART EN ARRIERE
    //
    // Les deux tests qui suivent etablissent, par le code, le mecanisme
    // qui a detruit les 92 proprietes — et delimitent ce qui n'y a PAS
    // contribue.
    //
    // `reouverture_apres_perte_de_la_queue_du_heap_ecrase_les_pages_vivantes`
    //   reproduit la sequence reelle : queue du heap non durable, puis
    //   reallocation des memes index, puis ECRASEMENT. Aucune troncature
    //   n'est necessaire : il suffit que la queue n'ait jamais atteint le
    //   disque. C'est ce que corrige l'ordre de flush (heap avant props).
    //
    // `la_recherche_binaire_sous_estime_la_frontiere_si_le_heap_a_un_trou`
    //   etablit un DEFAUT LATENT distinct : `compute_next_heap_page_idx`
    //   fait une recherche binaire sur un predicat (« la page porte-t-elle
    //   le magic ? ») qui n'est monotone que tant que les pages vivantes
    //   forment un prefixe contigu. Un seul trou suffit a lui faire rendre
    //   une frontiere arbitrairement basse.
    // =================================================================

    /// Nombre d'octets utiles d'une page heap (payload).
    const HEAP_PAYLOAD: usize =
        HEAP_PAGE_SIZE - core::mem::size_of::<crate::heap::HeapPageHeader>();

    /// Remplit une page heap entiere avec un enregistrement unique, de
    /// sorte que chaque `intern_bytes` consomme exactement une page : les
    /// index de page deviennent previsibles, et le test peut raisonner sur
    /// « la page N » sans dependre du tassage.
    fn intern_une_page_pleine(pz: &mut PropsZone, motif: u8) -> crate::page::HeapRef {
        let charge = vec![motif; HEAP_PAYLOAD - 4];
        pz.intern_bytes(&charge).unwrap()
    }

    /// LA CAUSE PREMIERE, reproduite de bout en bout.
    ///
    /// Sequence exacte, sans aucune troncature de fichier ni appel a
    /// `compact` / `shrink_to_fit` :
    ///
    /// 1. lot 1 interne des pages heap, le tout rendu durable ;
    /// 2. lot 2 interne d'AUTRES pages heap et ecrit les entrees props qui
    ///    les referencent, mais seules les PROPS atteignent le disque —
    ///    c'est l'ancien ordre de flush (le pointeur avant le pointe) ;
    /// 3. a la reouverture, l'allocateur reprend a la derniere page heap
    ///    DURABLE, donc EN DESSOUS des pages que les props survivantes
    ///    referencent encore ;
    /// 4. les ecritures suivantes reutilisent ces index et ECRASENT les
    ///    octets referencees.
    ///
    /// L'etat final est celui observe sur la base de production : la
    /// reference resout, elle rend des octets VALIDES, et ce sont ceux
    /// d'une AUTRE donnee. C'est cette etape 4 qui rend la perte
    /// irreversible — sans elle il manquerait des octets, avec elle ils
    /// sont REMPLACES.
    #[test]
    fn reouverture_apres_perte_de_la_queue_du_heap_ecrase_les_pages_vivantes() {
        let sf = SubstrateFile::open_tempfile().unwrap();
        let mut pz = PropsZone::open(&sf).unwrap();

        // ---- lot 1 : 2 pages heap, pleinement durables.
        for _ in 0..2 {
            intern_une_page_pleine(&mut pz, b'1');
        }
        pz.flush().unwrap();
        let heap_durable = pz.allocated_heap_page_count();
        assert_eq!(
            heap_durable, 2,
            "lot 1 doit occuper exactement 2 pages heap"
        );

        // Image disque de l'etat pleinement durable.
        let dir_durable = tempfile::tempdir().unwrap();
        copier_repertoire(sf.path(), dir_durable.path());

        // ---- lot 2 : 3 pages heap de plus, referencees par des entrees props.
        let mut head = None;
        let mut refs_lot2 = vec![];
        for i in 0..3u16 {
            let href = intern_une_page_pleine(&mut pz, b'2');
            refs_lot2.push(href);
            let e = PropertyEntry::new(i, PropertyValue::StringRef(href));
            head = Some(pz.append_entry(1, head, &e).unwrap());
        }
        assert_eq!(pz.allocated_heap_page_count(), 5);
        pz.flush().unwrap();

        // ---- le crash de l'ANCIEN ordre : les props ont atteint le disque,
        // la queue du heap non. On le modelise la ou il se produit vraiment
        // (au niveau du stockage) en restaurant le SEUL fichier heap a son
        // dernier etat durable ; le fichier props, lui, garde le lot 2.
        let dir_crash = tempfile::tempdir().unwrap();
        copier_repertoire(sf.path(), dir_crash.path());
        std::fs::copy(
            dir_durable.path().join(PROPS_HEAP_V2_FILENAME),
            dir_crash.path().join(PROPS_HEAP_V2_FILENAME),
        )
        .unwrap();
        drop(pz);

        // ---- reouverture : l'allocateur repart EN ARRIERE.
        let sf2 = SubstrateFile::open(dir_crash.path()).unwrap();
        let mut pz2 = PropsZone::open(&sf2).unwrap();
        let reprise = pz2.allocated_heap_page_count();
        assert_eq!(
            reprise, heap_durable,
            "l'allocateur reprend a la derniere page heap DURABLE"
        );
        let max_reference = refs_lot2.iter().map(|h| h.page_id).max().unwrap();
        assert!(
            reprise <= max_reference,
            "PREUVE : l'allocateur reprend a {reprise}, or des entrees props \
             durables referencent encore la page {max_reference} — les \
             prochaines ecritures vont les reutiliser"
        );

        // A ce stade les references du lot 2 sont PENDANTES (pas encore
        // ecrasees) : le heap ne va plus assez loin.
        for h in &refs_lot2 {
            assert!(
                pz2.read_heap_view(*h).is_none(),
                "avant reecriture, la reference {h:?} doit etre pendante"
            );
        }

        // ---- lot 3 : des donnees SANS AUCUN RAPPORT reutilisent ces index.
        for _ in 0..3 {
            intern_une_page_pleine(&mut pz2, b'3');
        }
        pz2.flush().unwrap();

        // ---- l'etat de production : la reference resout, et elle ment.
        let mut ecrasees = 0usize;
        for h in &refs_lot2 {
            if let Some(octets) = pz2.read_heap_view(*h) {
                assert!(
                    octets.iter().all(|b| *b == b'3'),
                    "la page {} rend desormais les octets du lot 3",
                    h.page_id
                );
                ecrasees += 1;
            }
        }
        assert!(
            ecrasees > 0,
            "PREUVE MANQUANTE : aucune reference du lot 2 n'a ete ecrasee \
             par le lot 3 — le mecanisme d'ecrasement n'est pas reproduit"
        );
    }

    /// DEFAUT LATENT — distinct de la cause premiere ci-dessus.
    ///
    /// `compute_next_heap_page_idx` cherche la frontiere « derniere page
    /// vivante » par recherche binaire sur la presence du magic. Une
    /// recherche binaire n'est correcte que sur un predicat MONOTONE : ici,
    /// « les pages vivantes forment un prefixe contigu ». L'invariant tient
    /// tant que les pages sont ecrites dans l'ordre ET que la reecriture du
    /// mmap atteint le disque dans cet ordre — ce que le noyau ne garantit
    /// pas : un `msync` interrompu peut laisser un sous-ensemble ARBITRAIRE
    /// de pages sales ecrites, donc un TROU.
    ///
    /// Le test construit le cas minimal deterministe : 8 pages vivantes,
    /// aucun remplissage, la page 4 remise a zero. La recherche sonde 4 en
    /// premier, le croit vide, et rabat la frontiere a 4 — alors que les
    /// pages 5, 6 et 7 sont vivantes et referencables. L'allocateur repart
    /// alors 4 pages en arriere et les ecrase.
    #[test]
    fn la_recherche_binaire_sous_estime_la_frontiere_si_le_heap_a_un_trou() {
        const PAGES: usize = 8;
        const TROU: usize = 4;

        let sf = SubstrateFile::open_tempfile().unwrap();
        let mut pz = PropsZone::open(&sf).unwrap();
        let mut refs = vec![];
        for _ in 0..PAGES {
            refs.push(intern_une_page_pleine(&mut pz, b'v'));
        }
        assert_eq!(pz.allocated_heap_page_count(), PAGES as u32);
        pz.flush().unwrap();

        let dir = tempfile::tempdir().unwrap();
        copier_repertoire(sf.path(), dir.path());
        drop(pz);

        // Le fichier heap est ramene a exactement 8 pages (pas de
        // remplissage : la frontiere est alors non ambigue), puis la page
        // du TROU est remise a zero — exactement ce que laisse un msync
        // interrompu qui n'a pas ecrit cette page-la.
        let chemin_heap = dir.path().join(PROPS_HEAP_V2_FILENAME);
        let mut octets = std::fs::read(&chemin_heap).unwrap();
        octets.truncate(PAGES * HEAP_PAGE_SIZE);
        octets[TROU * HEAP_PAGE_SIZE..(TROU + 1) * HEAP_PAGE_SIZE].fill(0);
        std::fs::write(&chemin_heap, &octets).unwrap();

        // Temoin : les pages au-dessus du trou sont bel et bien vivantes.
        for p in (TROU + 1)..PAGES {
            let magic = u32::from_le_bytes(
                octets[p * HEAP_PAGE_SIZE..p * HEAP_PAGE_SIZE + 4]
                    .try_into()
                    .unwrap(),
            );
            assert_eq!(
                magic,
                crate::heap::HEAP_PAGE_MAGIC,
                "la page {p} doit rester vivante — sinon le test ne prouve rien"
            );
        }

        let sf2 = SubstrateFile::open(dir.path()).unwrap();
        let pz2 = PropsZone::open(&sf2).unwrap();
        let frontiere = pz2.allocated_heap_page_count();

        assert!(
            frontiere < PAGES as u32,
            "PREUVE MANQUANTE : la recherche binaire a rendu {frontiere} \
             (= la bonne reponse) malgre le trou en {TROU}"
        );
        assert_eq!(
            frontiere, TROU as u32,
            "la recherche sonde la page du trou et rabat la frontiere dessus"
        );

        // La consequence : des pages vivantes ET REFERENCEES sont hors de
        // l'etendue que l'allocateur croit occupee — il va les reutiliser.
        let encore_vivantes: Vec<_> = refs
            .iter()
            .filter(|h| h.page_id >= frontiere && pz2.read_heap_view(**h).is_some())
            .collect();
        assert!(
            !encore_vivantes.is_empty(),
            "PREUVE MANQUANTE : aucune reference vivante au-dela de la frontiere"
        );
    }

    fn open_zone() -> (SubstrateFile, PropsZone) {
        let sf = SubstrateFile::open_tempfile().unwrap();
        let pz = PropsZone::open(&sf).unwrap();
        (sf, pz)
    }

    #[test]
    fn encode_decode_page_id() {
        assert_eq!(decode_page_id(encode_page_id(1)), Some(1));
        assert_eq!(decode_page_id(encode_page_id(42)), Some(42));
        assert_eq!(decode_page_id(U48::ZERO), None);
    }

    #[test]
    fn empty_zone_starts_with_sentinel_slot() {
        let (_sf, pz) = open_zone();
        // INITIAL_BYTES = 2 pages (dummy sentinel + first slot).
        assert_eq!(pz.props_len(), INITIAL_BYTES);
        assert_eq!(pz.next_page_idx, 1);
    }

    #[test]
    fn append_single_entry_returns_fresh_head() {
        let (_sf, mut pz) = open_zone();
        let entry = PropertyEntry::new(10, PropertyValue::I64(42));
        let head = pz.append_entry(7, None, &entry).unwrap();
        assert_eq!(head, 1);
        let collected = pz.collect_entries(Some(head)).unwrap();
        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0], entry);
    }

    #[test]
    fn multiple_entries_fit_on_one_page() {
        let (_sf, mut pz) = open_zone();
        let entries: Vec<_> = (0..10)
            .map(|i| PropertyEntry::new(i as u16, PropertyValue::I64(i as i64)))
            .collect();
        let mut head = None;
        for e in &entries {
            head = Some(pz.append_entry(0, head, e).unwrap());
        }
        // All 10 small entries fit on one page — head index stays at 1.
        assert_eq!(head, Some(1));
        let got = pz.collect_entries(head).unwrap();
        assert_eq!(got.len(), entries.len());
        // chain walks newest→oldest but all on one page, so same order as writes.
        assert_eq!(got, entries);
    }

    #[test]
    fn overflow_triggers_chain_expansion() {
        let (_sf, mut pz) = open_zone();
        // Each ArrI64 entry of 128 elements = 4 + 128*8 = 1028 B payload
        // + 4 B header = 1032 B per entry. Three entries = 3096 B which
        // fits in a ~4072 B payload; a fourth overflows.
        let big = PropertyValue::ArrI64((0..128).collect());
        let mut head = None;
        let mut writes = vec![];
        for i in 0..8u16 {
            let e = PropertyEntry::new(i, big.clone());
            head = Some(pz.append_entry(1, head, &e).unwrap());
            writes.push(e);
        }
        assert!(
            pz.allocated_page_count() >= 2,
            "chain should have spilled, got {} pages",
            pz.allocated_page_count()
        );
        let got = pz.collect_entries(head).unwrap();
        assert_eq!(got.len(), writes.len());
        // Newest→oldest walk: chain head is newest page (which holds the
        // most recent entries), then links to older page. Within a page
        // the cursor is write-order. So the full chain order is
        // [newest_page_entries_in_write_order, older_page_entries_in_write_order, ...].
        // Verify the multiset matches.
        let mut sorted_got: Vec<_> = got.iter().map(|e| e.prop_key).collect();
        let mut sorted_want: Vec<_> = writes.iter().map(|e| e.prop_key).collect();
        sorted_got.sort();
        sorted_want.sort();
        assert_eq!(sorted_got, sorted_want);
    }

    #[test]
    fn reopen_recovers_state() {
        let sf = SubstrateFile::open_tempfile().unwrap();
        let path = sf.path().to_path_buf();
        let mut pz = PropsZone::open(&sf).unwrap();
        let e = PropertyEntry::new(3, PropertyValue::F64(1.25));
        let head = pz.append_entry(99, None, &e).unwrap();
        pz.flush().unwrap();
        drop(pz);
        // Keep the tempdir alive via the SubstrateFile handle.
        let pz2 = PropsZone::open(&sf).unwrap();
        assert_eq!(pz2.allocated_page_count(), 1);
        let got = pz2.collect_entries(Some(head)).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0], e);
        // Silence the "unused" warning on `path` — keep it live until here.
        let _ = path;
    }

    #[test]
    fn heap_intern_roundtrip() {
        let (_sf, mut pz) = open_zone();
        let a = pz.intern_bytes(b"hello").unwrap();
        let b = pz.intern_bytes(b"world").unwrap();
        assert_eq!(pz.read_heap(a).as_deref(), Some(&b"hello"[..]));
        assert_eq!(pz.read_heap(b).as_deref(), Some(&b"world"[..]));
    }

    #[test]
    fn heap_spills_across_pages() {
        let (_sf, mut pz) = open_zone();
        // Each entry = 4 B len prefix + 1024 B payload = 1028 B.
        // 4 KiB - 16 B header = 4080 B per page -> ~3 entries per page.
        let mut refs = Vec::new();
        for i in 0..32 {
            let payload = vec![i as u8; 1024];
            let r = pz.intern_bytes(&payload).unwrap();
            refs.push((r, payload));
        }
        assert!(pz.allocated_heap_page_count() >= 2);
        for (r, expected) in &refs {
            assert_eq!(pz.read_heap(*r).as_deref(), Some(expected.as_slice()));
        }
    }

    #[test]
    fn round_trip_10k_nodes_mixed_props() {
        // T17c Step 2 acceptance verification: 10 000 nodes with mixed
        // scalars, 80-dim f64 vectors, and short string refs.
        let (_sf, mut pz) = open_zone();
        let mut heads: Vec<(u32, Option<u32>)> = Vec::with_capacity(10_000);
        for nid in 0..10_000u32 {
            let mut head = None;
            // Scalar int property.
            head = Some(
                pz.append_entry(
                    nid,
                    head,
                    &PropertyEntry::new(1, PropertyValue::I64(nid as i64)),
                )
                .unwrap(),
            );
            // String ref via heap (emulating a label lookup).
            let label = format!("node-{nid:05}");
            let href = pz.intern_bytes(label.as_bytes()).unwrap();
            head = Some(
                pz.append_entry(
                    nid,
                    head,
                    &PropertyEntry::new(2, PropertyValue::StringRef(href)),
                )
                .unwrap(),
            );
            // 80-dim f64 embedding.
            let embed: Vec<f64> = (0..80).map(|i| (i as f64) * 0.125 + nid as f64).collect();
            head = Some(
                pz.append_entry(
                    nid,
                    head,
                    &PropertyEntry::new(3, PropertyValue::ArrF64(embed)),
                )
                .unwrap(),
            );
            heads.push((nid, head));
        }
        // Sanity: every chain walks back to three entries with the right
        // prop keys. Check 200 random-ish samples (every 50th node) for
        // speed.
        for &(nid, head) in heads.iter().step_by(50) {
            let got = pz.collect_entries(head).unwrap();
            assert_eq!(got.len(), 3, "node {nid}: expected 3 entries");
            // prop keys {1, 2, 3} independent of order
            let mut keys: Vec<u16> = got.iter().map(|e| e.prop_key).collect();
            keys.sort();
            assert_eq!(keys, vec![1, 2, 3]);
            // Verify the scalar via find
            let i64_entry = got
                .iter()
                .find(|e| e.prop_key == 1)
                .expect("scalar entry present");
            assert_eq!(i64_entry.value, PropertyValue::I64(nid as i64));
            // Verify the label via heap lookup
            let str_entry = got
                .iter()
                .find(|e| e.prop_key == 2)
                .expect("string entry present");
            if let PropertyValue::StringRef(href) = str_entry.value {
                let bytes = pz.read_heap(href).expect("heap entry resolvable");
                assert_eq!(
                    std::str::from_utf8(&bytes).unwrap(),
                    format!("node-{nid:05}")
                );
            } else {
                panic!("expected StringRef at key 2");
            }
        }
    }

    #[test]
    fn chain_ordering_newest_first() {
        let (_sf, mut pz) = open_zone();
        let big = PropertyValue::ArrI64((0..128).collect());
        let entries: Vec<_> = (0..8u16)
            .map(|i| PropertyEntry::new(i, big.clone()))
            .collect();
        let mut head = None;
        for e in &entries {
            head = Some(pz.append_entry(1, head, e).unwrap());
        }
        assert!(pz.allocated_page_count() >= 2);
        let got = pz.collect_entries(head).unwrap();
        // The very first entry in `got` should correspond to the most
        // recently written entry on the newest page. The last key
        // written is 7; it must be on the newest (head) page, and its
        // prop_key must be present in the earliest block of `got` (the
        // first page's cursor yields in write order).
        let first_page_len = got
            .iter()
            .position(|e| e.prop_key == entries[0].prop_key)
            .unwrap_or(got.len());
        // All the keys in got[..first_page_len] are newer than the keys
        // in got[first_page_len..].
        let newer: std::collections::HashSet<u16> =
            got[..first_page_len].iter().map(|e| e.prop_key).collect();
        let older: std::collections::HashSet<u16> =
            got[first_page_len..].iter().map(|e| e.prop_key).collect();
        for n in &newer {
            for o in &older {
                assert!(n > o, "newer key {n} should be > older key {o}");
            }
        }
    }

    #[test]
    fn collect_live_props_by_key_lww_semantics() {
        let (_sf, mut pz) = open_zone();
        // Write 3 props, update 1, tombstone 1.
        let e_a = PropertyEntry::new(1, PropertyValue::I64(10));
        let e_b = PropertyEntry::new(2, PropertyValue::I64(20));
        let e_c = PropertyEntry::new(3, PropertyValue::I64(30));
        let e_b_updated = PropertyEntry::new(2, PropertyValue::I64(200));
        let mut tomb_c = PropertyEntry::new(3, PropertyValue::Null);
        tomb_c.flags = crate::page::ENTRY_FLAG_TOMBSTONE;

        let mut head = None;
        head = Some(pz.append_entry(0, head, &e_a).unwrap());
        head = Some(pz.append_entry(0, head, &e_b).unwrap());
        head = Some(pz.append_entry(0, head, &e_c).unwrap());
        head = Some(pz.append_entry(0, head, &e_b_updated).unwrap());
        head = Some(pz.append_entry(0, head, &tomb_c).unwrap());

        let mut live = FxHashMap::default();
        let mut tombstoned = FxHashSet::default();
        pz.collect_live_props_by_key(head, &mut live, &mut tombstoned)
            .unwrap();

        // Key 1 : live with I64(10)
        assert!(live.contains_key(&1));
        assert_eq!(live[&1].value, PropertyValue::I64(10));
        // Key 2 : live with UPDATED I64(200)
        assert!(live.contains_key(&2));
        assert_eq!(live[&2].value, PropertyValue::I64(200));
        // Key 3 : tombstoned, not in live
        assert!(!live.contains_key(&3));
        assert!(tombstoned.contains(&3));
        // No other keys
        assert_eq!(live.len(), 2);
        assert_eq!(tombstoned.len(), 1);
    }

    #[test]
    fn collect_live_props_by_key_empty_head() {
        let (_sf, pz) = open_zone();
        let mut live = FxHashMap::default();
        let mut tombstoned = FxHashSet::default();
        pz.collect_live_props_by_key(None, &mut live, &mut tombstoned)
            .unwrap();
        assert_eq!(live.len(), 0);
        assert_eq!(tombstoned.len(), 0);
    }

    #[test]
    fn collect_live_props_multi_page_newest_wins() {
        let (_sf, mut pz) = open_zone();
        // Write enough to overflow to a second page: each ArrI64(128)
        // entry is ~1028 B payload, so 3 entries fill the 3072 B page.
        let big = |v: i64| PropertyEntry::new(1, PropertyValue::ArrI64(vec![v; 128]));
        let mut head = None;
        head = Some(pz.append_entry(0, head, &big(1)).unwrap());
        head = Some(pz.append_entry(0, head, &big(2)).unwrap());
        head = Some(pz.append_entry(0, head, &big(3)).unwrap());
        // This likely triggers a new page
        head = Some(pz.append_entry(0, head, &big(4)).unwrap());

        let mut live = FxHashMap::default();
        let mut tombstoned = FxHashSet::default();
        pz.collect_live_props_by_key(head, &mut live, &mut tombstoned)
            .unwrap();
        // Only 1 key exists (id=1), and its latest value is 4
        assert_eq!(live.len(), 1);
        assert!(live.contains_key(&1));
        if let PropertyValue::ArrI64(v) = &live[&1].value {
            assert_eq!(v[0], 4, "newest write should win, got {:?}", v[0]);
        } else {
            panic!("unexpected value variant");
        }
    }

    #[test]
    fn tombstones_skip_on_walk() {
        let (_sf, mut pz) = open_zone();
        let e = PropertyEntry::new(1, PropertyValue::I64(7));
        let mut tomb = PropertyEntry::new(1, PropertyValue::Null);
        tomb.flags = crate::page::ENTRY_FLAG_TOMBSTONE;
        let head = pz.append_entry(0, None, &e).unwrap();
        let head = pz.append_entry(0, Some(head), &tomb).unwrap();
        let got = pz.collect_entries(Some(head)).unwrap();
        // Tombstone is not surfaced by walk_chain; the live entry is.
        // (Note: the tombstone semantically supersedes the live entry
        // in the store layer, which will apply last-write-wins per key.
        // walk_chain simply reports all non-tombstoned entries; the
        // store-level compactor/get_property honors the tombstone.)
        assert_eq!(got.len(), 1);
        assert_eq!(got[0], e);
    }

    #[test]
    fn too_large_entry_rejected() {
        let (_sf, mut pz) = open_zone();
        let huge = PropertyValue::ArrI64((0..600).collect()); // ~4804 B payload
        let e = PropertyEntry::new(1, huge);
        let err = pz.append_entry(0, None, &e).unwrap_err();
        assert!(matches!(err, SubstrateError::WalBadFrame(_)));
    }

    #[test]
    fn heap_too_large_entry_rejected() {
        let (_sf, mut pz) = open_zone();
        let big = vec![0u8; HEAP_PAGE_SIZE]; // strictly larger than max entry
        let err = pz.intern_bytes(&big).unwrap_err();
        assert!(matches!(err, SubstrateError::WalBadFrame(_)));
    }

    // ---- T17f Step 2: mixed node + edge owners ---------------------------

    #[test]
    fn edge_append_allocates_distinct_magic() {
        let (_sf, mut pz) = open_zone();
        let entry = PropertyEntry::new(10, PropertyValue::I64(42));
        let head_e = pz.append_entry_edge(5, None, &entry).unwrap();
        // Read back through the internal reader to check the magic.
        let page = pz.read_page(head_e).unwrap();
        assert_eq!(page.owner_kind(), Some(OwnerKind::Edge));
        assert_eq!(page.header.node_id, 5, "owner_id stored verbatim");
    }

    #[test]
    fn node_and_edge_chains_coexist_on_same_zone() {
        let (_sf, mut pz) = open_zone();
        let node_entry = PropertyEntry::new(1, PropertyValue::I64(111));
        let edge_entry = PropertyEntry::new(2, PropertyValue::I64(222));

        let node_head = pz.append_entry_node(42, None, &node_entry).unwrap();
        let edge_head = pz.append_entry_edge(7, None, &edge_entry).unwrap();
        assert_ne!(node_head, edge_head, "fresh heads get distinct slots");

        let np = pz.read_page(node_head).unwrap();
        let ep = pz.read_page(edge_head).unwrap();
        assert_eq!(np.owner_kind(), Some(OwnerKind::Node));
        assert_eq!(ep.owner_kind(), Some(OwnerKind::Edge));

        // Walking either chain surfaces only its own entry (chains are
        // rooted in distinct records, so they never share pages).
        let got_node = pz.collect_entries(Some(node_head)).unwrap();
        let got_edge = pz.collect_entries(Some(edge_head)).unwrap();
        assert_eq!(got_node, vec![node_entry]);
        assert_eq!(got_edge, vec![edge_entry]);
    }

    #[test]
    fn tete_d_un_autre_genre_alloue_une_page_neuve_sans_toucher_l_autre() {
        // (2026-08-19) Ce test verrouillait l ancien contrat : « une tête de
        // genre étranger est refusée ». Le refus s est révélé inapplicable —
        // `set_node_property` avalait l erreur en `warn` puis repliait sur la
        // DashMap, si bien que 1,6 M de propriétés de la base `messages` ne
        // parvenaient jamais en v2, sans que rien n échoue.
        //
        // Le contrat utile n était pas « refuser » mais « NE PAS ÉCRIRE CHEZ
        // AUTRUI ». C est ce que l on vérifie désormais, et c est plus fort :
        // on exige en plus que la page étrangère ressorte octet pour octet
        // identique, ce que l ancienne version ne regardait pas.
        let (_sf, mut pz) = open_zone();
        let entry = PropertyEntry::new(1, PropertyValue::I64(0));
        let node_head = pz.append_entry_node(1, None, &entry).unwrap();
        let avant = pz.read_page(node_head).unwrap();

        // On présente la tête d un nœud comme si c était celle d une arête.
        let edge_head = pz
            .append_entry_edge(2, Some(node_head), &entry)
            .expect("une tête inexploitable doit repartir sur une page neuve");

        assert_ne!(
            edge_head, node_head,
            "l arête ne doit surtout pas réutiliser la page du nœud"
        );

        // La page du nœud est intacte — c est l invariant qui compte.
        let apres = pz.read_page(node_head).unwrap();
        assert_eq!(
            bytemuck::bytes_of(&avant),
            bytemuck::bytes_of(&apres),
            "la page du nœud a été modifiée par un append d arête"
        );

        // Et chaque chaîne reste close sur son propre propriétaire.
        assert_eq!(pz.read_page(node_head).unwrap().header.node_id, 1);
        assert_eq!(pz.read_page(edge_head).unwrap().header.node_id, 2);
        assert_eq!(
            pz.collect_entries(Some(node_head)).unwrap(),
            vec![entry.clone()]
        );
        assert_eq!(
            pz.collect_entries(Some(edge_head)).unwrap(),
            vec![entry.clone()]
        );
    }

    #[test]
    fn tete_hors_zone_ou_jamais_ecrite_alloue_une_page_neuve() {
        // (2026-08-19) Les deux formes qui ont bloqué la migration de
        // `messages` : 1 501 040 refus « props page N out of range » et
        // 112 280 refus « invalid magic 0x0 ». Toutes deux viennent d un
        // `first_prop_off` parasite lu sur un NodeRecord antérieur à T17f.
        let (_sf, mut pz) = open_zone();
        let entry = PropertyEntry::new(1, PropertyValue::I64(42));

        // (a) index très au-delà de la zone allouée
        let h = pz
            .append_entry_node(11, Some(9_999_999), &entry)
            .expect("une tête hors zone doit repartir sur une page neuve");
        assert_eq!(pz.read_page(h).unwrap().header.node_id, 11);
        assert_eq!(pz.collect_entries(Some(h)).unwrap(), vec![entry.clone()]);

        // (b) page adressable mais jamais écrite (magic 0x0)
        let vierge = pz.next_page_idx + 1;
        pz.ensure_page_addressable(vierge).unwrap();
        assert_eq!(pz.read_page(vierge).unwrap().header.magic, 0);
        let h2 = pz
            .append_entry_node(12, Some(vierge), &entry)
            .expect("une page jamais écrite doit repartir sur une page neuve");
        assert_eq!(pz.read_page(h2).unwrap().header.node_id, 12);
        assert_eq!(pz.collect_entries(Some(h2)).unwrap(), vec![entry.clone()]);
    }

    #[test]
    fn edge_chain_overflows_and_preserves_entries() {
        // Exactly the `overflow_triggers_chain_expansion` coverage, but
        // for an edge owner. Guards against an asymmetric bug where the
        // spill path only worked for one kind.
        let (_sf, mut pz) = open_zone();
        let big = PropertyValue::ArrI64((0..128).collect());
        let mut head = None;
        let mut writes = vec![];
        for i in 0..8u16 {
            let e = PropertyEntry::new(i, big.clone());
            head = Some(pz.append_entry_edge(99, head, &e).unwrap());
            writes.push(e);
        }
        assert!(
            pz.allocated_page_count() >= 2,
            "edge chain should have spilled, got {} pages",
            pz.allocated_page_count()
        );
        // Every allocated page belongs to the edge kind.
        for idx in 1..=pz.allocated_page_count() {
            let page = pz.read_page(idx).unwrap();
            assert_eq!(
                page.owner_kind(),
                Some(OwnerKind::Edge),
                "page {idx} on an edge chain unexpectedly decodes as {:?}",
                page.owner_kind()
            );
        }
        let got = pz.collect_entries(head).unwrap();
        assert_eq!(got.len(), writes.len());
    }

    #[test]
    fn round_trip_10k_mixed_nodes_and_edges() {
        // T17f Step 2 acceptance: 5 000 nodes + 5 000 edges interleaved,
        // each with the same three-property mix as the node-only Step 1
        // test. Verifies chains don't bleed across kinds and that the
        // heap side-car is kind-agnostic.
        let (_sf, mut pz) = open_zone();
        let mut node_heads: Vec<(u32, Option<u32>)> = Vec::with_capacity(5_000);
        let mut edge_heads: Vec<(u32, Option<u32>)> = Vec::with_capacity(5_000);

        for i in 0..5_000u32 {
            // -- node chain ------------------------------------------------
            let mut nh = None;
            nh = Some(
                pz.append_entry_node(i, nh, &PropertyEntry::new(1, PropertyValue::I64(i as i64)))
                    .unwrap(),
            );
            let label = format!("node-{i:05}");
            let href = pz.intern_bytes(label.as_bytes()).unwrap();
            nh = Some(
                pz.append_entry_node(
                    i,
                    nh,
                    &PropertyEntry::new(2, PropertyValue::StringRef(href)),
                )
                .unwrap(),
            );
            nh = Some(
                pz.append_entry_node(
                    i,
                    nh,
                    &PropertyEntry::new(3, PropertyValue::ArrF64((0..16).map(f64::from).collect())),
                )
                .unwrap(),
            );
            node_heads.push((i, nh));

            // -- edge chain (distinct owner id namespace) ------------------
            let eid = i + 1_000_000;
            let mut eh = None;
            eh = Some(
                pz.append_entry_edge(
                    eid,
                    eh,
                    &PropertyEntry::new(10, PropertyValue::I64(-(i as i64))),
                )
                .unwrap(),
            );
            let etag = format!("edge-{i:05}");
            let ehref = pz.intern_bytes(etag.as_bytes()).unwrap();
            eh = Some(
                pz.append_entry_edge(
                    eid,
                    eh,
                    &PropertyEntry::new(11, PropertyValue::BytesRef(ehref)),
                )
                .unwrap(),
            );
            eh = Some(
                pz.append_entry_edge(
                    eid,
                    eh,
                    &PropertyEntry::new(12, PropertyValue::F64(i as f64 * 0.5)),
                )
                .unwrap(),
            );
            edge_heads.push((eid, eh));
        }

        // Sample 100 nodes and 100 edges — each chain surfaces its own
        // three entries, and no kind leaks across.
        for &(nid, head) in node_heads.iter().step_by(50) {
            let got = pz.collect_entries(head).unwrap();
            assert_eq!(got.len(), 3, "node {nid}: expected 3 entries");
            let mut keys: Vec<u16> = got.iter().map(|e| e.prop_key).collect();
            keys.sort();
            assert_eq!(keys, vec![1, 2, 3]);
            // Every page on this chain is Node-kind.
            let head_u32 = head.unwrap();
            let page = pz.read_page(head_u32).unwrap();
            assert_eq!(page.owner_kind(), Some(OwnerKind::Node));
        }
        for &(eid, head) in edge_heads.iter().step_by(50) {
            let got = pz.collect_entries(head).unwrap();
            assert_eq!(got.len(), 3, "edge {eid}: expected 3 entries");
            let mut keys: Vec<u16> = got.iter().map(|e| e.prop_key).collect();
            keys.sort();
            assert_eq!(keys, vec![10, 11, 12]);
            let head_u32 = head.unwrap();
            let page = pz.read_page(head_u32).unwrap();
            assert_eq!(page.owner_kind(), Some(OwnerKind::Edge));
            // Validate the BytesRef resolves through the shared heap.
            let bytes_entry = got
                .iter()
                .find(|e| e.prop_key == 11)
                .expect("bytesref entry present");
            if let PropertyValue::BytesRef(href) = bytes_entry.value {
                let payload = pz.read_heap(href).expect("heap entry resolvable");
                // Owner id eid = i + 1_000_000 → i = eid - 1_000_000.
                let i = eid - 1_000_000;
                assert_eq!(
                    std::str::from_utf8(&payload).unwrap(),
                    format!("edge-{i:05}")
                );
            } else {
                panic!("expected BytesRef at key 11");
            }
        }
    }

    #[test]
    fn back_compat_append_entry_still_allocates_node_kind() {
        // The unqualified `append_entry` shim must continue to produce
        // node-kind pages — this is the regression anchor for every
        // pre-T17f call site (including `store.rs:append_scalar_..`).
        let (_sf, mut pz) = open_zone();
        let entry = PropertyEntry::new(1, PropertyValue::I64(7));
        let head = pz.append_entry(42, None, &entry).unwrap();
        let page = pz.read_page(head).unwrap();
        assert_eq!(page.owner_kind(), Some(OwnerKind::Node));
    }

    #[test]
    fn uses_heap_ref_equality_for_lookup() {
        // Guard against a regression where two interns of the same
        // bytes accidentally collapse to the same HeapRef (we do NOT
        // dedupe — the caller decides).
        let (_sf, mut pz) = open_zone();
        let a = pz.intern_bytes(b"same").unwrap();
        let b = pz.intern_bytes(b"same").unwrap();
        assert_ne!(a, b, "intern_bytes must NOT dedupe — caller's choice");
        assert_eq!(pz.read_heap(a).as_deref(), Some(&b"same"[..]));
        assert_eq!(pz.read_heap(b).as_deref(), Some(&b"same"[..]));
        // Make sure HeapRef fields are consistent.
        assert_eq!(core::mem::size_of::<HeapRef>(), 8);
    }

    #[test]
    fn debut_chainee_pack_ecrit_control_et_slot() {
        let (_sub, mut pz) = open_zone();
        let e = PropertyEntry::new(7, PropertyValue::I64(42));
        let addr = pz
            .ecrire_debut_chainee_pack(9, OwnerKind::Node, &e, 1)
            .expect("début de chaîne packé écrit");
        // L'adresse rendue est un slot (carrier 1, unité 1).
        match crate::unite::decode_unite(addr) {
            crate::unite::UniteAddr::Slot { carrier, unite } => {
                assert_eq!(carrier, 1);
                assert_eq!(unite, 1);
            }
            other => panic!("adresse inattendue : {other:?}"),
        }
        // Relecture depuis la zone : contrôle puis slot (owner, entrée, CRC).
        let page = pz.page_slice(1).expect("carrier 1 lisible");
        let ctrl: &crate::unite::CarrierControl =
            bytemuck::from_bytes(&page[..core::mem::size_of::<crate::unite::CarrierControl>()]);
        assert_eq!(ctrl.magic, crate::unite::CARRIER_PACKED_MAGIC);
        assert_eq!(ctrl.nb_unites_utilisees, 2, "contrôle + unité 1 réservés");
        assert_eq!(ctrl.crc32, ctrl.compute_crc32());
        let slot: &crate::unite::SlotPage =
            bytemuck::from_bytes(&page[crate::unite::UNITE_SIZE..2 * crate::unite::UNITE_SIZE]);
        assert_eq!(slot.header.node_id, 9);
        assert_eq!(slot.header.magic, crate::page::PROP_PAGE_MAGIC_NODE);
        assert!(slot.verify_crc32(), "CRC du slot valide");
        let (entree, _) = slot.iter().next().expect("une entrée").expect("décodable");
        assert_eq!(entree.prop_key, 7);
        assert_eq!(entree.value, PropertyValue::I64(42));
    }
}
