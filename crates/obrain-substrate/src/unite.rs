//! Unités d'adressage 512 o de la zone PropsZone v2 — étape 1 du chantier
//! « slots » (RFC propszone-pages-partagees, design
//! DESIGN-PROPSZONE-SLOTS-512).
//!
//! Ce module est **STRICTEMENT ADDITIF** : aucun chemin existant du store ne
//! le consomme encore. Il pose les briques format (adressage, slot 512,
//! carrier 4K) et leurs tests, pour être câblé dans `PropsZone` aux étapes
//! suivantes (allocateur, walkers, remap, gouvernance de version).
//!
//! ## Adressage (UnitéAddr, 48 bits — dans les U48 existants)
//!
//! - bit 47 = 0 → **unité legacy** : l'index de page 4K actuel (valeur u32).
//!   Toutes les adresses existantes ont les bits hauts à 0 → compatibilité.
//! - bit 47 = 1 → **slot packé** : bits 46..4 = index du carrier 4K,
//!   bits 3..0 = numéro d'unité (1..=7 ; l'unité 0 est l'en-tête du carrier).
//! - `0` = pas de chaîne (inchangé).
//!
//! ## Géométrie
//!
//! Un carrier = 4096 o = 8 unités de 512 : unité 0 = `CarrierControl`,
//! unités 1..=7 = `SlotPage`. 7 petits chaînages partagent donc un 4K
//! (densité ×7 sur le motif CREATE).

use crate::page::{
    OwnerKind, PropertyCodecError, PropertyEntry, PropertyPageHeader, decode_entry, encode_entry,
};

/// Taille d'une unité (slot / contrôle).
pub const UNITE_SIZE: usize = 512;
/// Unités par carrier 4K.
pub const UNITES_PAR_CARRIER: usize = 4096 / UNITE_SIZE;
/// Unités utilisables par carrier (0 = contrôle).
pub const SLOTS_UTILISABLES_PAR_CARRIER: usize = UNITES_PAR_CARRIER - 1;
/// Taille du payload d'un slot (header 24 o).
pub const SLOT_PAYLOAD_SIZE: usize = UNITE_SIZE - core::mem::size_of::<PropertyPageHeader>();

/// Magic 4K du carrier packé (l'unité 0 porte ce magic en tête).
/// NOUVEAU magic : un vieux binaire le compterait comme padding — toute zone
/// écrite avec des carriers packés exige la garde de version (design §2/§6).
pub const CARRIER_PACKED_MAGIC: u32 = 0xC5A2_50C8;

const _: [(); 1] = [(); (SLOT_PAYLOAD_SIZE == 488) as usize];

// ---------------------------------------------------------------------------
// UnitéAddr — encodage/décodage 48 bits.
// ---------------------------------------------------------------------------

/// Marqueur « slot packé » (bit 47 d'un U48).
pub const UNITE_SLOT_FLAG: u64 = 1 << 47;

/// Encode une adresse de slot `(carrier, unite 1..=7)` en U48 (u64).
/// Rend `None` si `unite` est hors 1..=7 ou si le carrier dépasse la plage
/// adressable (u32 — la zone réelle est bien en dessous).
pub fn encode_slot(carrier: u32, unite: u8) -> Option<u64> {
    if unite == 0 || unite as usize > SLOTS_UTILISABLES_PAR_CARRIER {
        return None;
    }
    Some(UNITE_SLOT_FLAG | ((carrier as u64) << 4) | (unite as u64))
}

/// Découpe une adresse brute 48 bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UniteAddr {
    /// Aucune chaîne (`0`).
    Aucune,
    /// Carrier legacy 4K (bit 47 = 0).
    Legacy(u32),
    /// Slot packé (bit 47 = 1) : (carrier, unité 1..=7).
    Slot { carrier: u32, unite: u8 },
}

/// Décode une adresse 48 bits brute. Refuse (plutôt que tronquer en silence)
/// toute valeur mal formée — le cast muet « `v as u32` » de l'encodage actuel
/// est précisément le danger à ne pas reproduire pour les nouvelles adresses.
pub fn decode_unite(addr: u64) -> UniteAddr {
    if addr == 0 {
        return UniteAddr::Aucune;
    }
    if addr & UNITE_SLOT_FLAG != 0 {
        let sans_flag = addr & !UNITE_SLOT_FLAG;
        // Bits résiduels au-delà de la plage (carrier u32 << 4) → mal formé.
        if sans_flag >> 4 > u64::from(u32::MAX) {
            return UniteAddr::Aucune;
        }
        let carrier = (sans_flag >> 4) as u32;
        let unite = (sans_flag & 0xF) as u8;
        if unite == 0 || unite as usize > SLOTS_UTILISABLES_PAR_CARRIER {
            return UniteAddr::Aucune;
        }
        return UniteAddr::Slot { carrier, unite };
    }
    // Legacy : seuls les 32 bits bas sont légitimes.
    if addr > u64::from(u32::MAX) {
        return UniteAddr::Aucune;
    }
    UniteAddr::Legacy(addr as u32)
}

// ---------------------------------------------------------------------------
// SlotPage — 512 o : PropertyPageHeader (24 o) + payload 488 o.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct SlotPage {
    pub header: PropertyPageHeader,
    pub payload: [u8; SLOT_PAYLOAD_SIZE],
}

const _: [(); 1] = [(); (core::mem::size_of::<SlotPage>() == UNITE_SIZE) as usize];

impl Default for SlotPage {
    fn default() -> Self {
        Self {
            header: PropertyPageHeader::default(),
            payload: [0; SLOT_PAYLOAD_SIZE],
        }
    }
}

impl SlotPage {
    /// Slot neuf dédié à un owner.
    pub fn with_owner(owner_id: u32, kind: OwnerKind) -> Self {
        let mut s = Self::default();
        s.header.magic = kind.magic();
        s.header.node_id = owner_id;
        s
    }

    /// CRC32 (crc32fast) sur header (crc nul) + payload entier.
    pub fn compute_crc32(&self) -> u32 {
        let mut h = crc32fast::Hasher::new();
        let mut hdr = self.header;
        hdr.crc32 = 0;
        h.update(bytemuck::bytes_of(&hdr));
        h.update(&self.payload);
        h.finalize()
    }

    pub fn verify_crc32(&self) -> bool {
        self.compute_crc32() == self.header.crc32
    }

    pub fn seal_crc32(&mut self) {
        self.header.crc32 = self.compute_crc32();
    }

    /// Append séquentiel d'une entrée au `free_offset` (miroir de
    /// `PropertyPage::append_entry`, borné au payload 488 o).
    pub fn append_entry(&mut self, entry: &PropertyEntry) -> Result<u16, PropertyCodecError> {
        let free = self.header.free_offset as usize;
        let room = SLOT_PAYLOAD_SIZE.saturating_sub(free);
        let need = entry.encoded_len();
        if need > room {
            return Err(PropertyCodecError::PageFull {
                needed: need,
                available: room,
            });
        }
        encode_entry(entry, &mut self.payload[free..free + need])?;
        self.header.free_offset = (free + need) as u16;
        self.header.entry_count = self.header.entry_count.saturating_add(1);
        if entry.is_tombstone() {
            self.header.tombstones = self.header.tombstones.saturating_add(1);
        }
        Ok(free as u16)
    }

    /// Itère les entrées du slot (tombes incluses — filtrage par les walkers),
    /// dans l'ordre d'écriture.
    pub fn iter(
        &self,
    ) -> impl Iterator<Item = Result<(PropertyEntry, usize), PropertyCodecError>> + '_ {
        let used = &self.payload[..self.header.free_offset as usize];
        let mut pos = 0usize;
        core::iter::from_fn(move || {
            if pos >= used.len() {
                return None;
            }
            let slice = &used[pos..];
            match decode_entry(slice) {
                Ok((e, n)) => {
                    pos += n;
                    Some(Ok((e, n)))
                }
                Err(err) => {
                    pos = used.len();
                    Some(Err(err))
                }
            }
        })
    }
}

// ---------------------------------------------------------------------------
// CarrierControl — unité 0 du carrier packé.
// ---------------------------------------------------------------------------

/// Unité 0 (512 o) d'un carrier packé : magic + compteur d'unités utilisées +
/// CRC de contrôle (porte sur le header seul ; le contenu des slots est
/// couvert par le CRC de chaque slot).
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CarrierControl {
    pub magic: u32,
    pub nb_unites_utilisees: u8,
    pub version: u8,
    pub reserved: [u8; 6],
    pub crc32: u32,
}

const _: [(); 1] = [(); (core::mem::size_of::<CarrierControl>() == 16) as usize];

impl CarrierControl {
    pub fn new() -> Self {
        Self {
            magic: CARRIER_PACKED_MAGIC,
            nb_unites_utilisees: 1, // l'unité 0 est ce contrôle
            version: 1,
            ..Self::default()
        }
    }

    pub fn compute_crc32(&self) -> u32 {
        let mut h = crc32fast::Hasher::new();
        let mut c = *self;
        c.crc32 = 0;
        h.update(bytemuck::bytes_of(&c));
        h.finalize()
    }

    pub fn seal_crc32(&mut self) {
        self.crc32 = self.compute_crc32();
    }

    /// Enregistre l'allocation d'une nouvelle unité (1..=7). Rend `None` si le
    /// carrier est plein.
    pub fn reserver_unite(&mut self) -> Option<u8> {
        let u = self.nb_unites_utilisees;
        if u as usize >= UNITES_PAR_CARRIER {
            return None;
        }
        self.nb_unites_utilisees = u + 1;
        Some(u)
    }
}

// ---------------------------------------------------------------------------
// CarrierPack — gestion in-memory d'un carrier packé sur une région 4K.
// ---------------------------------------------------------------------------
//
// Prototype de l'étape 2 (mécanique pure, sans mmap ni fichier) : un carrier
// = région [u8; 4096] ; l'unité 0 porte le `CarrierControl`, les unités
// 1..=7 des `SlotPage`. Sert à figer le comportement de l'allocateur
// (réservation jusqu'à 7, écritures par slot isolées, CRC par slot) avant le
// câblage dans `PropsZone`.

#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(transparent)]
pub struct CarrierPack(pub [u8; 4096]);

const _: [(); 1] = [(); (core::mem::size_of::<CarrierPack>() == 4096) as usize];

impl Default for CarrierPack {
    fn default() -> Self {
        Self::new()
    }
}

impl CarrierPack {
    pub fn new() -> Self {
        let mut region = [0u8; 4096];
        // Unité 0 = contrôle.
        let mut ctrl = CarrierControl::new();
        ctrl.seal_crc32();
        region[..core::mem::size_of::<CarrierControl>()].copy_from_slice(bytemuck::bytes_of(&ctrl));
        Self(region)
    }

    fn unite_brute(&mut self, unite: u8) -> Option<&mut [u8; UNITE_SIZE]> {
        if unite == 0 || unite as usize > SLOTS_UTILISABLES_PAR_CARRIER {
            return None;
        }
        let debut = (unite as usize) * UNITE_SIZE;
        Some(bytemuck::from_bytes_mut(
            &mut self.0[debut..debut + UNITE_SIZE],
        ))
    }

    /// Le contrôle du carrier (unité 0).
    pub fn controle(&self) -> CarrierControl {
        *bytemuck::from_bytes(&self.0[..core::mem::size_of::<CarrierControl>()])
    }

    /// Réserve la prochaine unité libre (1..=7) et rend son `SlotPage` neuf
    /// pour `owner`. `None` = carrier plein.
    pub fn allouer_slot(&mut self, owner: u32, kind: OwnerKind) -> Option<u8> {
        let mut ctrl = self.controle();
        let unite = ctrl.reserver_unite()?;
        let brut = self
            .unite_brute(unite)
            .expect("unité réservée → adressable");
        let slot = SlotPage::with_owner(owner, kind);
        brut.copy_from_slice(bytemuck::bytes_of(&slot));
        ctrl.seal_crc32();
        self.0[..core::mem::size_of::<CarrierControl>()].copy_from_slice(bytemuck::bytes_of(&ctrl));
        Some(unite)
    }

    /// Accès en écriture au slot `unite` (1..=7).
    pub fn slot(&mut self, unite: u8) -> Option<&mut SlotPage> {
        self.unite_brute(unite).map(|b| bytemuck::from_bytes_mut(b))
    }

    /// Accès en lecture au slot `unite`.
    pub fn slot_ref(&self, unite: u8) -> Option<&SlotPage> {
        if unite == 0 || unite as usize > SLOTS_UTILISABLES_PAR_CARRIER {
            return None;
        }
        let debut = (unite as usize) * UNITE_SIZE;
        Some(bytemuck::from_bytes(&self.0[debut..debut + UNITE_SIZE]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::{PropertyValue, ValueTag};

    #[test]
    fn geometrie_slots() {
        assert_eq!(UNITE_SIZE, 512);
        assert_eq!(UNITES_PAR_CARRIER, 8);
        assert_eq!(SLOTS_UTILISABLES_PAR_CARRIER, 7);
        assert_eq!(SLOT_PAYLOAD_SIZE, 488);
        assert_eq!(core::mem::size_of::<SlotPage>(), 512);
        assert_eq!(core::mem::size_of::<CarrierControl>(), 16);
    }

    #[test]
    fn encode_decode_slot_roundtrip() {
        for carrier in [0u32, 1, 123_456, u32::MAX] {
            for unite in 1u8..=7 {
                let addr = encode_slot(carrier, unite).expect("adresse valide");
                assert_eq!(addr & UNITE_SLOT_FLAG, UNITE_SLOT_FLAG);
                match decode_unite(addr) {
                    UniteAddr::Slot {
                        carrier: c,
                        unite: u,
                    } => {
                        assert_eq!(c, carrier);
                        assert_eq!(u, unite);
                    }
                    other => panic!("décode en {other:?} pour ({carrier},{unite})"),
                }
            }
        }
    }

    #[test]
    fn adresses_invalides_refusees() {
        // Slot 0 (contrôle) et 8 (hors carrier) : refusés à l'encodage.
        assert!(encode_slot(0, 0).is_none());
        assert!(encode_slot(0, 8).is_none());
        // Legacy : 0 = aucune ; valeurs u32 = legacy.
        assert_eq!(decode_unite(0), UniteAddr::Aucune);
        assert_eq!(decode_unite(5), UniteAddr::Legacy(5));
        // Bits hauts sans drapeau (legacy corrompue) : refus net.
        assert_eq!(decode_unite(1 << 40), UniteAddr::Aucune);
        assert_eq!(decode_unite(1 << 32), UniteAddr::Aucune);
        // Drapeau posé, unité hors bornes : refus.
        assert_eq!(decode_unite(UNITE_SLOT_FLAG), UniteAddr::Aucune);
        assert_eq!(decode_unite(UNITE_SLOT_FLAG | 8), UniteAddr::Aucune);
    }

    #[test]
    fn slot_append_remplit_puis_plein() {
        let mut slot = SlotPage::with_owner(3, OwnerKind::Node);
        assert_eq!(slot.header.magic, crate::page::PROP_PAGE_MAGIC_NODE);
        // Remplit le slot d'entrées Int64 (12 o) : 488 / 12 ≈ 40.
        let mut n = 0u32;
        loop {
            let e = PropertyEntry::new((n % 100) as u16, PropertyValue::I64(i64::from(n)));
            match slot.append_entry(&e) {
                Ok(_) => n += 1,
                Err(PropertyCodecError::PageFull { .. }) => break,
                Err(other) => panic!("erreur inattendue : {other:?}"),
            }
        }
        assert!(
            (38..=41).contains(&n),
            "capacité d'un slot ~40 Int64, reçu {n}"
        );
        assert_eq!(slot.header.entry_count, n as u16);
        // CRC + itération AVANT toute corruption.
        slot.seal_crc32();
        assert!(slot.verify_crc32());
        let mut lus = 0u32;
        for item in slot.iter() {
            let (e, _n) = item.expect("entrée décodable");
            match e.value {
                PropertyValue::I64(v) => assert_eq!(v, i64::from(lus)),
                _ => panic!("valeur inattendue"),
            }
            lus += 1;
        }
        assert_eq!(lus, n);
        // Corruption d'un octet de payload → CRC détecte.
        slot.payload[10] ^= 0xFF;
        assert!(!slot.verify_crc32());
    }

    #[test]
    fn carrier_controle_reserve_7_unites() {
        let mut c = CarrierControl::new();
        assert_eq!(c.magic, CARRIER_PACKED_MAGIC);
        for attente in 1u8..=7 {
            let u = c.reserver_unite().expect("unité disponible");
            assert_eq!(u, attente);
        }
        assert!(c.reserver_unite().is_none(), "carrier plein après 7 unités");
        c.seal_crc32();
        assert_eq!(c.crc32, c.compute_crc32());
    }

    #[test]
    fn carrier_alloue_7_owners_et_isole_les_slots() {
        let mut cp = CarrierPack::new();
        assert_eq!(cp.controle().magic, CARRIER_PACKED_MAGIC);
        // 7 owners → slots 1..=7 ; le 8e est refusé (carrier plein).
        let mut unites = Vec::new();
        for owner in 0u32..7 {
            let u = cp.allouer_slot(owner, OwnerKind::Node).expect("slot libre");
            assert!(!unites.contains(&u));
            unites.push(u);
            let s = cp.slot_ref(u).expect("slot lisible");
            assert_eq!(s.header.node_id, owner, "owner stamp du slot {u}");
            assert_eq!(s.header.magic, crate::page::PROP_PAGE_MAGIC_NODE);
        }
        assert!(
            cp.allouer_slot(99, OwnerKind::Node).is_none(),
            "carrier plein"
        );
        assert_eq!(cp.controle().nb_unites_utilisees, 8);
        // Isolation : chaque slot écrit ses propres entrées + CRC, sans
        // affecter ses voisins.
        for (i, &u) in unites.iter().enumerate() {
            let s = cp.slot(u).expect("slot");
            s.append_entry(&PropertyEntry::new(1, PropertyValue::I64(i as i64)))
                .expect("append");
            s.seal_crc32();
        }
        for (i, &u) in unites.iter().enumerate() {
            let s = cp.slot_ref(u).expect("slot");
            assert!(s.verify_crc32(), "CRC du slot {u} valide");
            let (e, _) = s.iter().next().expect("une entrée").expect("décodable");
            assert_eq!(e.prop_key, 1);
            assert_eq!(e.value, PropertyValue::I64(i as i64));
            assert_eq!(s.header.entry_count, 1);
        }
        // Le slot d'un owner reste celui du bon owner après écritures voisines.
        assert_eq!(cp.slot_ref(unites[3]).expect("slot").header.node_id, 3);
    }

    // Prouve que le type de tag est référencé (import utilisé par les
    // futurs tests de tombes).
    #[allow(dead_code)]
    fn _tombstone_tag() -> ValueTag {
        ValueTag::Null
    }
}
