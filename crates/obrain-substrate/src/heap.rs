//! String/bytes/array heap — 4 KiB append-only pages with tombstones.
//!
//! See `docs/rfc/substrate/format-spec.md` §6 for the layout.
//!
//! ## Le prefixe de longueur est surdimensionne — et on le laisse tel quel
//!
//! Chaque enregistrement porte un prefixe de longueur `u32`. MESURE sur le
//! heap de la base de production (`substrate.props.heap.v2`, 27 938 pages,
//! 20 268 vivantes) :
//!
//! | grandeur                                   | valeur        |
//! |--------------------------------------------|---------------|
//! | enregistrements                            | 1 731 752     |
//! | octets de payload                          | 74 906 540    |
//! | octets de prefixe (`u32`)                  |  6 927 008    |
//! | heap utile (payload + prefixe)             | 81 833 548    |
//! | prefixe en part du heap utile              | 8,46 %        |
//! | longueur maximale observee                 | 256 o         |
//! | enregistrements de 255 o ou plus           | 516 (0,030 %) |
//!
//! Un prefixe `u8` coute encore 1 octet par enregistrement : l'economie
//! REELLE est donc de 6 927 008 - 1 731 752 = **5 195 256 o**, soit
//! **6,35 %** du heap utile — et non 8,46 %, qui est le cout total du
//! prefixe et non ce qu'on peut en reprendre.
//!
//! A NOTER : il est FAUX que « 100 % des valeurs font moins de 256 o ».
//! 516 enregistrements atteignent ou depassent 255 o (maximum observe :
//! 256). Un `u8` nu ne suffit donc pas — il faut un echappement, par
//! exemple 0xFF signalant qu'un `u32` suit. Le cas rare existe pour de
//! vrai, il n'est pas hypothetique.
//!
//! CE CHANTIER NE LE FAIT PAS, ET C'EST DELIBERE. C'est un changement de
//! FORMAT SUR DISQUE : tout store existant (~1 500 bases) porte des
//! prefixes `u32`, et `read_at` n'a aucun moyen de distinguer un `u8`
//! echappe d'un `u32` en lisant les octets — les deux encodages sont
//! ambigus entre eux. Il faut donc un champ de version par zone, lu avant
//! toute interpretation, et un chemin de migration. Livrer cela au
//! milieu d'un chantier de durabilite, sur le SEUL chemin qui vient
//! d'etre prouve responsable de la perte des 92 proprietes, echangerait
//! 5 Mo contre un risque sans commune mesure.
//!
//! LE CHEMIN, pour qui le reprendra : (1) ajouter un numero de version a
//! l'en-tete de zone heap — il reste 0 octet libre dans
//! [`HeapPageHeader`], donc la version va dans un en-tete de ZONE (page 0
//! reservee) et non de page ; (2) `read_at` se branche sur cette version ;
//! (3) les pages neuves s'ecrivent au format 2, les anciennes se lisent au
//! format 1, sans reecriture — le heap est append-only, les deux formats
//! peuvent coexister par page si la version est par page plutot que par
//! zone ; (4) l'economie n'apparait qu'au fil des ecritures neuves, ce qui
//! est acceptable puisque rien ne presse. Mesurer a nouveau avant de
//! s'engager : 5 Mo sur une base de 9 Go, c'est 0,06 % du total.

use bytemuck::{Pod, Zeroable};

/// Magic marker for [`HeapPage`] (FNV-1a truncated for "heap").
pub const HEAP_PAGE_MAGIC: u32 = 0xF1EA_95FF;

pub const HEAP_PAGE_SIZE: usize = 4096;

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct HeapPageHeader {
    pub magic: u32,       // @0..4
    pub page_id: u32,     // @4..8
    pub crc32: u32,       // @8..12
    pub entry_count: u16, // @12..14
    pub free_offset: u16, // @14..16
}

const _: [(); 1] = [(); (core::mem::size_of::<HeapPageHeader>() == 16) as usize];

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct HeapPage {
    pub header: HeapPageHeader,
    pub payload: [u8; HEAP_PAGE_SIZE - core::mem::size_of::<HeapPageHeader>()],
}

const _: [(); 1] = [(); (core::mem::size_of::<HeapPage>() == HEAP_PAGE_SIZE) as usize];

impl Default for HeapPage {
    fn default() -> Self {
        Self {
            header: HeapPageHeader::default(),
            payload: [0; HEAP_PAGE_SIZE - core::mem::size_of::<HeapPageHeader>()],
        }
    }
}

impl core::fmt::Debug for HeapPage {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HeapPage")
            .field("header", &self.header)
            .field("payload_len", &self.payload.len())
            .finish()
    }
}

impl HeapPage {
    pub fn new(page_id: u32) -> Self {
        let mut p = Self::default();
        p.header.magic = HEAP_PAGE_MAGIC;
        p.header.page_id = page_id;
        p.header.free_offset = 0;
        p
    }

    /// Append a `&[u8]` entry. Returns the offset within `payload` where the entry starts,
    /// or `None` if the page is full.
    pub fn append(&mut self, bytes: &[u8]) -> Option<u32> {
        let entry_len = 4 + bytes.len(); // 4 B length prefix + payload
        let cursor = self.header.free_offset as usize;
        if cursor + entry_len > self.payload.len() {
            return None;
        }
        // Write u32 LE length prefix, then the bytes.
        let len_bytes = (bytes.len() as u32).to_le_bytes();
        self.payload[cursor..cursor + 4].copy_from_slice(&len_bytes);
        self.payload[cursor + 4..cursor + 4 + bytes.len()].copy_from_slice(bytes);
        let offset = cursor as u32;
        self.header.free_offset = (cursor + entry_len) as u16;
        self.header.entry_count = self.header.entry_count.saturating_add(1);
        Some(offset)
    }

    /// Read an entry at a given offset. Returns `None` if the offset is past the free region
    /// or the claimed length would run past the written region.
    ///
    /// LA BORNE EST `free_offset`, JAMAIS `payload.len()`.
    ///
    /// `free_offset` marque la fin des octets REELLEMENT ECRITS ; au-dela
    /// s'etend le reste de la page, qui n'a jamais ete initialise pour cet
    /// enregistrement (zeros a la creation, ou octets d'une vie anterieure
    /// de la page). Borner sur `payload.len()` (4080) laissait passer tout
    /// champ de longueur corrompu tant qu'il restait sous 4080 : la lecture
    /// rendait alors `Some(..)` sur des octets arbitraires de la page — de
    /// la corruption SERVIE SILENCIEUSEMENT, indiscernable d'une valeur
    /// legitime par l'appelant. Avec `free_offset`, le meme enregistrement
    /// corrompu est REFUSE.
    ///
    /// `free_offset` lui-meme vient de l'en-tete et peut etre corrompu : on
    /// le rabat donc d'abord sur `payload.len()`.
    pub fn read_at(&self, offset: u32) -> Option<&[u8]> {
        let off = offset as usize;
        // En-tete potentiellement corrompu : `free_offset` ne peut pas
        // designer plus loin que la page elle-meme.
        let ecrit = (self.header.free_offset as usize).min(self.payload.len());
        if off.checked_add(4)? > ecrit {
            return None;
        }
        let mut len_bytes = [0u8; 4];
        len_bytes.copy_from_slice(&self.payload[off..off + 4]);
        let len = u32::from_le_bytes(len_bytes) as usize;
        if off.checked_add(4)?.checked_add(len)? > ecrit {
            return None;
        }
        Some(&self.payload[off + 4..off + 4 + len])
    }

    pub fn compute_crc32(&self) -> u32 {
        let mut h = crc32fast::Hasher::new();
        let mut header_copy = self.header;
        header_copy.crc32 = 0;
        h.update(bytemuck::bytes_of(&header_copy));
        h.update(&self.payload);
        h.finalize()
    }

    pub fn verify_crc32(&self) -> bool {
        self.compute_crc32() == self.header.crc32
    }

    pub fn seal_crc32(&mut self) {
        self.header.crc32 = self.compute_crc32();
    }
}

// ---------------------------------------------------------------------------
// StringHeap — in-memory writer + reader over a Vec<HeapPage>.
// This is the "build path" used by migration; the runtime uses mmap'd heap pages.
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct StringHeap {
    pub pages: Vec<HeapPage>,
}

impl StringHeap {
    pub fn new() -> Self {
        Self { pages: vec![] }
    }

    /// Intern the given bytes. Returns `(page_id, offset)` suitable for a [`crate::page::HeapRef`].
    pub fn intern(&mut self, bytes: &[u8]) -> (u32, u32) {
        // Try the last page first.
        if let Some(last) = self.pages.last_mut()
            && let Some(off) = last.append(bytes)
        {
            return (last.header.page_id, off);
        }
        // Allocate a new page.
        let page_id = self.pages.len() as u32;
        let mut page = HeapPage::new(page_id);
        // If the payload is too big to fit in a single fresh page, we fail loudly.
        let off = page
            .append(bytes)
            .expect("string-heap: single entry larger than heap page payload");
        self.pages.push(page);
        (page_id, off)
    }

    pub fn get(&self, page_id: u32, offset: u32) -> Option<&[u8]> {
        self.pages
            .get(page_id as usize)
            .and_then(|p| p.read_at(offset))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heap_page_is_4kib() {
        assert_eq!(core::mem::size_of::<HeapPage>(), HEAP_PAGE_SIZE);
    }

    #[test]
    fn append_and_read() {
        let mut p = HeapPage::new(0);
        let off1 = p.append(b"hello").unwrap();
        let off2 = p.append(b"world").unwrap();
        assert_eq!(p.read_at(off1), Some(&b"hello"[..]));
        assert_eq!(p.read_at(off2), Some(&b"world"[..]));
        assert_eq!(p.header.entry_count, 2);
    }

    #[test]
    fn fills_and_rejects() {
        let mut p = HeapPage::new(0);
        let big = vec![0u8; HEAP_PAGE_SIZE - 16 - 4]; // exactly fills
        assert!(p.append(&big).is_some());
        // Next append must fail.
        assert!(p.append(b"x").is_none());
    }

    #[test]
    fn crc32_roundtrip() {
        let mut p = HeapPage::new(0);
        p.append(b"abc").unwrap();
        p.seal_crc32();
        assert!(p.verify_crc32());
        p.payload[0] = 0xFF;
        assert!(!p.verify_crc32());
    }

    #[test]
    fn string_heap_spills_to_new_page() {
        let mut h = StringHeap::new();
        // Force spill by interning many small entries.
        let mut refs = vec![];
        for i in 0..2000 {
            let s = format!("value-{i:05}");
            refs.push((h.intern(s.as_bytes()), s));
        }
        // Should have spilled to > 1 page.
        assert!(h.pages.len() > 1, "expected spill, got {}", h.pages.len());
        for ((page_id, off), s) in refs {
            let got = h.get(page_id, off).unwrap();
            assert_eq!(got, s.as_bytes());
        }
    }

    #[test]
    fn read_at_out_of_bounds_returns_none() {
        let p = HeapPage::new(0);
        assert!(p.read_at(0).is_none());
        assert!(p.read_at(99_999).is_none());
    }

    /// Une longueur corrompue mais INFERIEURE a `payload.len()` doit etre
    /// refusee, pas servie.
    ///
    /// C'est le cas dangereux : l'ancienne borne etait `payload.len()`
    /// (4080), si bien qu'un prefixe de longueur corrompu a, disons, 4000
    /// passait le controle et rendait `Some(&payload[4..4004])` — 4000
    /// octets de page arbitraires (zeros d'initialisation, ou residus
    /// d'une vie anterieure de la page) presentes a l'appelant comme une
    /// valeur legitime. La borne correcte est `free_offset`, qui marque la
    /// fin des octets reellement ecrits.
    #[test]
    fn longueur_corrompue_sous_la_taille_de_page_est_refusee() {
        let mut p = HeapPage::new(0);
        let off = p.append(b"court").unwrap();
        assert_eq!(p.read_at(off), Some(&b"court"[..]));
        let ecrit = p.header.free_offset as usize;

        // Longueur corrompue : au-dela des octets ecrits, mais tres en
        // dessous de la taille de page — exactement ce que l'ancienne
        // borne laissait passer.
        let corrompue: u32 = 4000;
        assert!(
            (corrompue as usize) < p.payload.len(),
            "le test doit porter sur une longueur SOUS payload.len()"
        );
        assert!((corrompue as usize) > ecrit, "et AU-DELA des octets ecrits");
        p.payload[off as usize..off as usize + 4].copy_from_slice(&corrompue.to_le_bytes());

        assert_eq!(
            p.read_at(off),
            None,
            "une longueur corrompue doit etre refusee, jamais servie comme \
             des octets de page arbitraires"
        );
    }

    /// Un `free_offset` corrompu au-dela de la page ne doit pas faire
    /// deborder la lecture : il est rabattu sur `payload.len()`.
    #[test]
    fn free_offset_corrompu_ne_deborde_pas() {
        let mut p = HeapPage::new(0);
        let off = p.append(b"x").unwrap();
        p.header.free_offset = u16::MAX;
        // Ne doit pas paniquer, et ne doit rien rendre au-dela de la page.
        let _ = p.read_at(off);
        assert!(p.read_at(u32::MAX).is_none());
        assert!(p.read_at(p.payload.len() as u32).is_none());
    }
}
