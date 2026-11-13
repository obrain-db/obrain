//! ── INDEX DE LABELS : label → slots porteurs (obrain#120, point 5) ─────────
//!
//! ## Le défaut qu'il ferme
//!
//! `nodes_by_label` balayait TOUTE la zone des nœuds (`1..next_node_id`) en
//! testant un bit de `label_bitset` : un `MATCH (n:Rare)` coûtait O(slots), pas
//! O(porteurs). Ce module tient, par bit de label, la liste des slots
//! candidats ; la requête ne lit plus que ceux-là.
//!
//! ## Le contrat : un SUR-ENSEMBLE, vérifié à la lecture
//!
//! La vérité reste la zone (`NodeRecord.label_bitset` + drapeaux + tenant).
//! L'index ne la remplace pas, il la DÉSIGNE. Son seul invariant :
//!
//! > pour tout slot de la zone dont le record porte le bit `b` et n'est PAS
//! > tombstoné, le slot figure dans la liste `b`.
//!
//! Les faux positifs (label retiré, nœud supprimé) sont permis : la requête
//! relit chaque candidat et applique EXACTEMENT le prédicat du balayage
//! (`est_visible_pour(portee)` puis le bit, borne `hw`). Le résultat est donc
//! celui du balayage, slot pour slot, dans le même ordre — l'index ne peut
//! qu'ignorer des slots qui n'auraient pas été rendus. Un retrait n'a rien à
//! maintenir ; seul un AJOUT de bit (ou une résurrection) doit être noté.
//!
//! ## Qui le tient à jour
//!
//! Le [`crate::writer::Writer`] : toute écriture de record de nœud d'un
//! processus passe par lui (`write_node`, `update_node`, `revive_node`,
//! `bulk_write`, et les deux permutations de slots). Les crochets sont donc
//! au point d'écriture, pas chez les appelants : création, `add_label`,
//! `remove_label`, suppression, rembobinage d'une transaction (qui rappelle
//! `add_label` / `remove_label` / `revive_node`) sont couverts sans qu'aucun
//! d'eux ne sache que l'index existe. Les permutations (`bulk_sort_by_hilbert`,
//! `compact_community`) et l'import en bloc (`bulk_write`) INVALIDENT l'index :
//! la requête suivante le reconstruit.
//!
//! ## Pourquoi en RAM, reconstruit paresseusement
//!
//! L'index n'est jamais persisté. Il naît au PREMIER `nodes_by_label` d'un
//! store, d'un seul passage sur la zone — donc APRÈS le rejeu du WAL, la
//! migration et tout ce que fait l'ouverture. Après un `kill -9`, il est
//! dérivé de la zone que le rejeu a restaurée : il ne peut pas la contredire,
//! et il n'ajoute ni format, ni fichier, ni ligne au rejeu.
//!
//! ## Où il est INACTIF (le balayage reste le chemin)
//!
//! En écriture partagée (`open_shared_write`) et en lecture seule
//! (`open_read_only`), d'AUTRES processus écrivent dans la zone mappée : un
//! pair qui pose un label le pose dans les pages que ce processus lit, sans
//! passer par SON `Writer`. Un index local manquerait ce slot. Ces deux modes
//! gardent donc le balayage, exact par construction.
//!
//! ## Verrous
//!
//! Un `RwLock` FEUILLE pour les crochets d'écriture : ils le prennent APRÈS
//! avoir relâché le verrou de zone. La construction et la compaction tiennent
//! l'index en écriture PUIS lisent la zone ; la requête tient l'index en
//! lecture PUIS lit la zone. Un seul ordre (index → zone), jamais l'inverse.
//! La construction tient l'index en écriture pendant tout son passage : un
//! crochet concurrent attend, et note son slot APRÈS — rien ne se perd entre
//! la lecture de la zone et la publication de l'index.

use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::{RwLock, RwLockReadGuard, RwLockUpgradableReadGuard, RwLockWriteGuard};

/// Un `label_bitset` porte au plus 64 labels.
pub(crate) const NB_BITS: usize = 64;

/// Au-delà de ce nombre d'ajouts hors ordre en attente, la liste est fusionnée
/// dès l'écriture (borne la mémoire d'un flot d'`add_label` jamais interrogé).
const SEUIL_FUSION_ECRITURE: usize = 1 << 16;

/// Une requête complète qui a lu plus de ce nombre de candidats périmés (et
/// plus de périmés que de porteurs) déclenche la compaction de la liste.
pub(crate) const SEUIL_COMPACTION: usize = 1024;

/// Slots candidats d'un bit de label.
#[derive(Default)]
struct Liste {
    /// Triée, sans doublon.
    tries: Vec<u32>,
    /// Ajouts arrivés hors ordre (slot ≤ dernier de `tries`), non triés.
    ajouts: Vec<u32>,
}

impl Liste {
    fn ajouter(&mut self, slot: u32) {
        match self.tries.last() {
            Some(&d) if d == slot => {}
            Some(&d) if d > slot => self.ajouts.push(slot),
            _ if !self.ajouts.is_empty() => self.ajouts.push(slot),
            _ => self.tries.push(slot),
        }
        if self.ajouts.len() > SEUIL_FUSION_ECRITURE && self.ajouts.len() * 4 > self.tries.len() {
            self.fusionner();
        }
    }

    fn fusionner(&mut self) {
        if self.ajouts.is_empty() {
            return;
        }
        let mut ajouts = std::mem::take(&mut self.ajouts);
        ajouts.sort_unstable();
        ajouts.dedup();
        let tries = std::mem::take(&mut self.tries);
        let mut out = Vec::with_capacity(tries.len() + ajouts.len());
        let (mut i, mut j) = (0, 0);
        while i < tries.len() && j < ajouts.len() {
            let (a, b) = (tries[i], ajouts[j]);
            if a < b {
                out.push(a);
                i += 1;
            } else if b < a {
                out.push(b);
                j += 1;
            } else {
                out.push(a);
                i += 1;
                j += 1;
            }
        }
        out.extend_from_slice(&tries[i..]);
        out.extend_from_slice(&ajouts[j..]);
        self.tries = out;
    }

    fn octets(&self) -> usize {
        (self.tries.capacity() + self.ajouts.capacity()) * std::mem::size_of::<u32>()
    }
}

/// L'index construit : une liste par bit.
pub(crate) struct Etat {
    listes: Vec<Liste>,
}

impl Etat {
    fn vide() -> Self {
        Self {
            listes: (0..NB_BITS).map(|_| Liste::default()).collect(),
        }
    }

    fn noter(&mut self, slot: u32, mut bits: u64) {
        while bits != 0 {
            let b = bits.trailing_zeros() as usize;
            self.listes[b].ajouter(slot);
            bits &= bits - 1;
        }
    }

    /// Candidats triés du bit `bit`. Appelé seulement après
    /// [`IndexLabels::lecture_prete`], qui a fusionné la liste.
    pub(crate) fn candidats(&self, bit: usize) -> &[u32] {
        debug_assert!(self.listes[bit].ajouts.is_empty());
        &self.listes[bit].tries
    }
}

/// Instantané de diagnostic de l'index (tests, mesures).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiagnosticIndexLabels {
    /// L'index sert-il ce store ? (`false` : lecture seule, écriture partagée.)
    pub actif: bool,
    /// Est-il construit en ce moment ?
    pub construit: bool,
    /// Constructions depuis l'ouverture (1re requête + une par invalidation).
    pub constructions: u64,
    /// Requêtes servies par l'index.
    pub requetes: u64,
    /// Compactions de listes.
    pub compactions: u64,
    /// Entrées tenues (somme des listes, ajouts compris).
    pub entrees: u64,
    /// Octets réservés par les listes.
    pub octets: u64,
}

/// L'index label → slots d'un `Writer`.
pub(crate) struct IndexLabels {
    actif: bool,
    etat: RwLock<Option<Etat>>,
    constructions: AtomicU64,
    requetes: AtomicU64,
    compactions: AtomicU64,
}

impl IndexLabels {
    pub(crate) fn new(actif: bool) -> Self {
        Self {
            actif,
            etat: RwLock::new(None),
            constructions: AtomicU64::new(0),
            requetes: AtomicU64::new(0),
            compactions: AtomicU64::new(0),
        }
    }

    pub(crate) fn actif(&self) -> bool {
        self.actif
    }

    /// Crochet d'écriture : `bits` vient d'apparaître (ou de revivre) au slot
    /// `slot`. À appeler APRÈS l'écriture de la zone, verrou de zone relâché.
    /// Sans effet tant que l'index n'est pas construit : la construction lira
    /// la zone, écriture comprise.
    pub(crate) fn noter(&self, slot: u32, bits: u64) {
        if !self.actif || bits == 0 {
            return;
        }
        if let Some(e) = self.etat.write().as_mut() {
            e.noter(slot, bits);
        }
    }

    /// Oublie l'index : la prochaine requête le reconstruit depuis la zone.
    pub(crate) fn invalider(&self) {
        if self.actif {
            *self.etat.write() = None;
        }
    }

    /// Garde qui invalide l'index à sa destruction (y compris sur `?`).
    pub(crate) fn garde_invalidation(&self) -> GardeInvalidation<'_> {
        self.invalider();
        GardeInvalidation(self)
    }

    /// Rend l'index en LECTURE, construit et la liste `bit` fusionnée.
    ///
    /// `batir` reçoit un collecteur `(slot, label_bitset)` et doit lui passer
    /// chaque record NON tombstoné de toute la zone ; il rend `false` en cas
    /// d'échec (l'index reste alors non construit et l'appelant balaie). Il est
    /// appelé index tenu en ÉCRITURE : il peut lire la zone, pas reprendre
    /// l'index.
    pub(crate) fn lecture_prete(
        &self,
        bit: usize,
        batir: impl FnOnce(&mut dyn FnMut(u32, u64)) -> bool,
    ) -> Option<RwLockReadGuard<'_, Option<Etat>>> {
        if !self.actif || bit >= NB_BITS {
            return None;
        }
        let g = self.etat.upgradable_read();
        if g.as_ref().is_some_and(|e| e.listes[bit].ajouts.is_empty()) {
            self.requetes.fetch_add(1, Ordering::Relaxed);
            return Some(RwLockUpgradableReadGuard::downgrade(g));
        }
        let mut w = RwLockUpgradableReadGuard::upgrade(g);
        if w.is_none() {
            let mut e = Etat::vide();
            if !batir(&mut |slot, bits| e.noter(slot, bits)) {
                return None;
            }
            // La croissance par doublement réserve jusqu'à 2× : 32 Mo pour
            // 5 M d'entrées mesurées, 20 Mo utiles.
            for l in &mut e.listes {
                l.tries.shrink_to_fit();
            }
            *w = Some(e);
            self.constructions.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(e) = w.as_mut() {
            e.listes[bit].fusionner();
        }
        self.requetes.fetch_add(1, Ordering::Relaxed);
        Some(RwLockWriteGuard::downgrade(w))
    }

    /// Compacte la liste `bit` : ne garde que les slots pour lesquels
    /// `garder(slot)` est vrai. `filtrer` reçoit la liste et rend les slots à
    /// garder ; il est appelé index tenu en ÉCRITURE (même ordre de verrous que
    /// la construction).
    pub(crate) fn compacter(&self, bit: usize, filtrer: impl FnOnce(&[u32]) -> Option<Vec<u32>>) {
        if !self.actif || bit >= NB_BITS {
            return;
        }
        let mut w = self.etat.write();
        let Some(e) = w.as_mut() else {
            return;
        };
        let liste = &mut e.listes[bit];
        liste.fusionner();
        if let Some(gardes) = filtrer(&liste.tries) {
            liste.tries = gardes;
            self.compactions.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn diagnostic(&self) -> DiagnosticIndexLabels {
        let g = self.etat.read();
        let (construit, entrees, octets) = match g.as_ref() {
            Some(e) => (
                true,
                e.listes
                    .iter()
                    .map(|l| (l.tries.len() + l.ajouts.len()) as u64)
                    .sum(),
                e.listes.iter().map(|l| l.octets() as u64).sum(),
            ),
            None => (false, 0, 0),
        };
        DiagnosticIndexLabels {
            actif: self.actif,
            construit,
            constructions: self.constructions.load(Ordering::Relaxed),
            requetes: self.requetes.load(Ordering::Relaxed),
            compactions: self.compactions.load(Ordering::Relaxed),
            entrees,
            octets,
        }
    }
}

/// Voir [`IndexLabels::garde_invalidation`].
pub(crate) struct GardeInvalidation<'a>(&'a IndexLabels);

impl Drop for GardeInvalidation<'_> {
    fn drop(&mut self) {
        self.0.invalider();
    }
}

/// Bits que l'écriture de `nouveau` par-dessus `ancien` doit noter.
///
/// Un record vivant (non tombstoné) qui portait déjà un bit figure déjà dans sa
/// liste (invariant) : seuls les bits NOUVEAUX sont à noter. Un record
/// tombstoné a pu être compacté hors de toutes ses listes : s'il revit, tous
/// ses bits sont à noter.
pub(crate) fn bits_a_noter(ancien_bits: u64, ancien_tombstone: bool, nouveau_bits: u64) -> u64 {
    if ancien_tombstone {
        nouveau_bits
    } else {
        nouveau_bits & !ancien_bits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn liste_fusion_triee_sans_doublon() {
        let mut l = Liste::default();
        for s in [1, 3, 5, 5, 2, 9, 3, 4, 10] {
            l.ajouter(s);
        }
        l.fusionner();
        assert_eq!(l.tries, vec![1, 2, 3, 4, 5, 9, 10]);
        assert!(l.ajouts.is_empty());
    }

    #[test]
    fn bits_a_noter_vivant_et_revenant() {
        assert_eq!(bits_a_noter(0b011, false, 0b110), 0b100);
        assert_eq!(bits_a_noter(0b011, true, 0b011), 0b011);
        assert_eq!(bits_a_noter(0b011, false, 0b001), 0);
    }
}
