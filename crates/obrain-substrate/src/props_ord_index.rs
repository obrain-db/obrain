//! # Index ORDONNE PAR VALEUR et persiste des proprietes (« PropsOrdIndex »)
//!
//! ## Le besoin, distinct de `props_index`
//!
//! [`crate::props_index`] (`substrate.props.idx.v2`) trie ses series par
//! `(cle, slot)` ou `cle` est un encodage MONOTONE mais LOSSY (chaines
//! tronquees a 11 octets). C'est parfait pour un RANGE (sur-ensemble
//! re-verifie), mais insuffisant pour rendre `ORDER BY ... LIMIT k` en O(k) :
//! a l'interieur d'un groupe de meme cle, l'ordre `(cle, slot)` ne reproduit
//! PAS l'ordre des valeurs, et de nombreux `qualified_name` partagent leur
//! prefixe de 12 octets. Rendre exactement les k premiers exigerait de lire
//! tout le groupe-frontiere.
//!
//! Cette zone-ci trie chaque serie par la **VRAIE valeur** (puis slot), avec
//! EXACTEMENT le comparateur du SORT du moteur (`OrderableValue`, partage entre
//! substrat et moteur via `obrain-common`). L'iteration de la serie rend donc
//! l'ordre exact des valeurs. Consequences directes :
//!
//! * `min(prop)` = premiere entree, `max(prop)` = derniere entree — O(1) ;
//! * `ORDER BY prop [ASC|DESC] LIMIT k` = k premieres/dernieres entrees — O(k).
//!
//! ## Ce qu'elle reutilise de `props_index`
//!
//! TOUT le format sur disque : [`EntreeIndex`] (16 o), [`EnteteSerie`],
//! [`EnteteRacine`], [`DescSerie`], [`ENTREES_PAR_PAGE`], [`SERIES_MAX`],
//! l'ecriture atomique, le CRC par page, la generation, les ancres de
//! couverture. On n'en duplique rien : on importe. La cle 12 octets est
//! CONSERVEE dans l'entree (accelerateur de recherche binaire ; elle reste
//! monotone non-decroissante dans l'ordre de valeur, donc `candidats` reste
//! juste), mais l'iteration ordonnee (premiers/derniers) l'IGNORE.
//!
//! ## La difference de fusion, et POURQUOI aucune verification d'ordre
//!
//! [`PropsIndexZone::fusionner`](crate::props_index::PropsIndexZone::fusionner) refuse une serie dont `(cle, slot)` n'est pas
//! croissant, parce que sa recherche binaire par intervalle en depend. Ici, la
//! serie est triee par l'appelant selon `comparer_total` (le comparateur EXACT
//! du SORT du moteur), qui pour deux chaines RFC3339 compare PAR INSTANT et non
//! par octets, et donne a une chaine datetime le rang d'un Timestamp. Cet ordre
//! N'EST DONC PAS monotone en `cle` (encodage octets-bruts, sans instant) : une
//! verification `cle` croissante REFUSERAIT a tort des series correctes.
//!
//! Cette zone renonce donc a la recherche par intervalle (le range reste servi
//! par [`crate::props_index`]) et ne verifie AUCUN ordre a la fusion : elle
//! persiste la liste VERBATIM dans l'ordre fourni. La correction de l'ordre est
//! la responsabilite de l'appelant (le store, qui trie par `comparer_total`) et
//! elle est prouvee au niveau moteur par l'IDENTITE des resultats. Le champ
//! `cle` de l'entree est conserve (format identique a `props_index`) mais n'est
//! plus qu'un temoin ; l'iteration ordonnee (premiers/derniers) l'ignore.
//!
//! ## Meme doctrine que `props_index`
//!
//! Les chaines de pages sont la source de verite ; cet index n'en est qu'une
//! projection. Corrompu / perime / d'une autre version : on le JETTE (zone vide
//! + non fiable) et l'appelant retombe sur le balayage. Fiable UNIQUEMENT apres
//!   confrontation des ancres.

#![allow(unsafe_code)]

use crate::error::{SubstrateError, SubstrateResult};
use crate::file::{SubstrateFile, ZoneFile};
use crate::page::PAGE_SIZE;

/// Octets d'une entrée de série — DÉRIVÉ du type partagé
/// [`EntreeIndex`], jamais écrit en dur : la largeur de la clé d'ordre
/// (`props_index::LARGEUR_CLE`) a changé une fois (12 → 48 octets), et un
/// `16` figé ici faisait alors échouer toute fusion (« source slice length
/// (4056) does not match destination slice length (1248) ») — six tests du
/// module le disent.
const OCTETS_ENTREE: usize = core::mem::size_of::<EntreeIndex>();
use crate::props_index::{
    DescSerie, ENTREES_PAR_PAGE, EnteteRacine, EnteteSerie, EntreeIndex, SERIES_MAX,
};
use std::path::{Path, PathBuf};

/// Nom canonique du fichier d'index ordonne par valeur.
pub const PROPS_ORD_INDEX_V2_FILENAME: &str = "substrate.props.ord.idx.v2";

/// Magic de la page racine (plage distincte de `props_index` = 0x..1D00 et de
/// `props_eq_index` = 0x..1D10/0x..1D20).
pub const ORD_MAGIC_RACINE: u32 = 0xC5DE_1D30;
/// Magic d'une page de serie.
pub const ORD_MAGIC_SERIE: u32 = 0xC5DE_1D31;

/// Version du format. Un index d'une autre version est JETE.
pub const ORD_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// La zone
// ---------------------------------------------------------------------------

/// Zone d'index ordonne PAR VALEUR, projetee en memoire. Aucune RAM anonyme :
/// les series sont lues DANS la projection.
pub struct PropsOrdIndexZone {
    zone: Option<ZoneFile>,
    chemin: PathBuf,
    racine: EnteteRacine,
    series: Vec<DescSerie>,
    /// `false` tant que les ancres n'ont pas ete confrontees, ou des qu'elles
    /// ont diverge. Tant que ce champ est faux, l'index ne repond pas.
    fiable: bool,
}

impl PropsOrdIndexZone {
    /// Ouvre l'index d'un substrat. Absence de fichier, magic/version
    /// inattendus, CRC faux : zone VIDE et NON FIABLE, jamais d'erreur.
    pub fn ouvrir(sub: &SubstrateFile) -> Self {
        let chemin = sub.path().join(PROPS_ORD_INDEX_V2_FILENAME);
        Self::ouvrir_chemin(&chemin)
    }

    fn ouvrir_chemin(chemin: &Path) -> Self {
        let vide = Self {
            zone: None,
            chemin: chemin.to_path_buf(),
            racine: EnteteRacine::default(),
            series: Vec::new(),
            fiable: false,
        };
        if !chemin.exists() {
            return vide;
        }
        let Ok(zone) = ZoneFile::open_or_create(chemin) else {
            tracing::warn!(
                ?chemin,
                "index ordonne-valeur illisible — ignore (balayage)"
            );
            return vide;
        };
        let octets = zone.as_slice();
        if octets.len() < PAGE_SIZE {
            return vide;
        }
        let racine: EnteteRacine = *bytemuck::from_bytes(&octets[..48]);
        if racine.magic != ORD_MAGIC_RACINE || racine.version != ORD_VERSION {
            tracing::warn!(
                magic = format!("{:#x}", racine.magic),
                version = racine.version,
                "index ordonne-valeur d'un autre format — jete"
            );
            return vide;
        }
        if racine.nb_series as usize > SERIES_MAX {
            return vide;
        }
        let mut copie = [0u8; PAGE_SIZE];
        copie.copy_from_slice(&octets[..PAGE_SIZE]);
        copie[8..12].copy_from_slice(&0u32.to_le_bytes());
        if crc32fast::hash(&copie) != racine.crc32 {
            tracing::warn!("racine d'index ordonne-valeur : CRC faux — index jete");
            return vide;
        }
        let mut series = Vec::with_capacity(racine.nb_series as usize);
        for i in 0..racine.nb_series as usize {
            let d = 48 + i * 16;
            series.push(*bytemuck::from_bytes::<DescSerie>(&octets[d..d + 16]));
        }
        let pages_totales = (octets.len() / PAGE_SIZE) as u32;
        for s in &series {
            if s.page_debut == 0 || s.page_debut.saturating_add(s.nb_pages) > pages_totales {
                tracing::warn!("serie d'index ordonne-valeur hors zone — index jete");
                return vide;
            }
        }
        Self {
            zone: Some(zone),
            chemin: chemin.to_path_buf(),
            racine,
            series,
            fiable: false,
        }
    }

    /// Confronte les ancres de couverture a l'etat COURANT du substrat. C'est
    /// ici, et seulement ici, qu'un index devient fiable.
    pub fn confronter_ancres(&mut self, offset_wal: u64, pages_v2: u32) {
        let ok = self.zone.is_some()
            && self.racine.ancre_wal == offset_wal
            && self.racine.ancre_pages == pages_v2;
        if !ok && self.zone.is_some() {
            tracing::info!(
                ancre_wal = self.racine.ancre_wal,
                offset_wal,
                ancre_pages = self.racine.ancre_pages,
                pages_v2,
                "index ordonne-valeur : ancres divergentes — NON FIABLE, on balaiera"
            );
        }
        self.fiable = ok;
    }

    /// L'index repond-il ? `false` => l'appelant DOIT balayer.
    #[must_use]
    pub fn fiable(&self) -> bool {
        self.fiable
    }

    #[must_use]
    pub fn generation(&self) -> u32 {
        self.racine.generation
    }

    #[must_use]
    pub fn ancre_wal(&self) -> u64 {
        self.racine.ancre_wal
    }

    #[must_use]
    pub fn ancre_pages(&self) -> u32 {
        self.racine.ancre_pages
    }

    #[must_use]
    pub fn nb_entrees_totales(&self) -> u64 {
        self.series.iter().map(|s| u64::from(s.nb_entrees)).sum()
    }

    /// Nombre d'entrees (valeurs non-null) indexees pour une propriete, ou
    /// `None` si l'index n'est pas fiable ou si la propriete n'y figure pas.
    #[must_use]
    pub fn nb_entrees(&self, prop_key_id: u16) -> Option<u32> {
        if !self.fiable {
            return None;
        }
        self.desc(prop_key_id).map(|d| d.nb_entrees)
    }

    #[must_use]
    pub fn proprietes(&self) -> Vec<u16> {
        self.series.iter().map(|s| s.prop_key_id).collect()
    }

    #[must_use]
    pub fn chemin(&self) -> &Path {
        &self.chemin
    }

    fn desc(&self, prop_key_id: u16) -> Option<&DescSerie> {
        self.series.iter().find(|s| s.prop_key_id == prop_key_id)
    }

    /// Entrees de la page de rang `p` d'une serie, LUES DANS LA PROJECTION.
    fn entrees_page(&self, d: &DescSerie, p: u32) -> Option<&[EntreeIndex]> {
        let zone = self.zone.as_ref()?;
        let octets = zone.as_slice();
        let debut = (d.page_debut + p) as usize * PAGE_SIZE;
        let page = octets.get(debut..debut + PAGE_SIZE)?;
        let e: &EnteteSerie = bytemuck::from_bytes(&page[..24]);
        if e.magic != ORD_MAGIC_SERIE || e.prop_key_id != d.prop_key_id {
            return None;
        }
        let n = (e.nb_entrees as usize).min(ENTREES_PAR_PAGE);
        Some(bytemuck::cast_slice(&page[24..24 + n * OCTETS_ENTREE]))
    }

    /// Les `k` PREMIERS slots dans l'ordre de VALEUR (ascendant). Rend `None`
    /// (jamais `Some(vec![])`) si l'index n'est pas fiable ou si la propriete
    /// n'y figure pas / serie vide. Lit O(k / ENTREES_PAR_PAGE) pages.
    pub fn premiers_slots(&self, prop_key_id: u16, k: usize) -> Option<Vec<u32>> {
        if !self.fiable || k == 0 {
            return None;
        }
        let d = *self.desc(prop_key_id)?;
        if d.nb_entrees == 0 {
            return None;
        }
        let mut out = Vec::with_capacity(k.min(d.nb_entrees as usize));
        for p in 0..d.nb_pages {
            let entrees = self.entrees_page(&d, p)?;
            for e in entrees {
                out.push(e.slot);
                if out.len() >= k {
                    return Some(out);
                }
            }
        }
        Some(out)
    }

    /// Les `k` DERNIERS slots, rendus dans l'ordre de VALEUR ascendant (les k
    /// plus grandes valeurs). L'appelant (DESC) les re-trie par le vrai
    /// comparateur. Rend `None` si non fiable / propriete absente / serie vide.
    /// Lit O(k / ENTREES_PAR_PAGE) pages depuis la fin.
    pub fn derniers_slots(&self, prop_key_id: u16, k: usize) -> Option<Vec<u32>> {
        if !self.fiable || k == 0 {
            return None;
        }
        let d = *self.desc(prop_key_id)?;
        if d.nb_entrees == 0 {
            return None;
        }
        let total = d.nb_entrees as usize;
        let a_sauter = total.saturating_sub(k); // index de la premiere entree gardee
        let page_debut = (a_sauter / ENTREES_PAR_PAGE) as u32;
        let mut reste_a_sauter = a_sauter % ENTREES_PAR_PAGE;
        let mut out = Vec::with_capacity(k.min(total));
        for p in page_debut..d.nb_pages {
            let entrees = self.entrees_page(&d, p)?;
            let debut = reste_a_sauter.min(entrees.len());
            reste_a_sauter = 0;
            for e in &entrees[debut..] {
                out.push(e.slot);
            }
        }
        Some(out)
    }

    /// Copie les entrees d'une serie existante. Seule sortie de donnees vers le
    /// tas, uniquement au moment d'une FUSION. Vide si non fiable / absente :
    /// on ne reporte JAMAIS dans un index frais le contenu d'un index dont on
    /// doute (les ancres neuves le beniraient).
    #[must_use]
    pub fn entrees_serie(&self, prop_key_id: u16) -> Vec<EntreeIndex> {
        if !self.fiable {
            return Vec::new();
        }
        let Some(d) = self.desc(prop_key_id).copied() else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(d.nb_entrees as usize);
        for p in 0..d.nb_pages {
            match self.entrees_page(&d, p) {
                Some(e) => out.extend_from_slice(e),
                None => return Vec::new(),
            }
        }
        out
    }

    /// Reecrit INTEGRALEMENT l'index : fichier temporaire, `fsync`, `rename`,
    /// `fsync` du repertoire, puis reprojection.
    ///
    /// `series` est persistee VERBATIM dans l'ordre fourni : c'est l'appelant
    /// (le store) qui garantit l'ordre `comparer_total`. Aucune verification
    /// d'ordre ici — voir le pave de tete (« POURQUOI aucune verification »).
    /// La correction est prouvee au niveau moteur par l'identite des resultats.
    pub fn fusionner(
        &mut self,
        series: &[(u16, Vec<EntreeIndex>)],
        ancre_wal: u64,
        ancre_pages: u32,
    ) -> SubstrateResult<()> {
        if series.len() > SERIES_MAX {
            return Err(SubstrateError::Internal(format!(
                "index ordonne-valeur : {} series demandees, {SERIES_MAX} au plus",
                series.len()
            )));
        }

        let mut descs: Vec<DescSerie> = Vec::with_capacity(series.len());
        let mut pages: Vec<[u8; PAGE_SIZE]> = Vec::new();
        let mut page_courante = 1u32; // page 0 = racine

        for (pk, entrees) in series {
            let nb_pages = entrees.len().div_ceil(ENTREES_PAR_PAGE).max(1) as u32;
            descs.push(DescSerie {
                prop_key_id: *pk,
                _pad: 0,
                page_debut: page_courante,
                nb_pages,
                nb_entrees: u32::try_from(entrees.len()).map_err(|_| {
                    SubstrateError::Internal("index ordonne-valeur : serie > 2^32 entrees".into())
                })?,
            });
            for p in 0..nb_pages {
                let mut page = [0u8; PAGE_SIZE];
                let d = p as usize * ENTREES_PAR_PAGE;
                let f = (d + ENTREES_PAR_PAGE).min(entrees.len());
                let n = f.saturating_sub(d);
                let entete = EnteteSerie {
                    magic: ORD_MAGIC_SERIE,
                    crc32: 0,
                    prop_key_id: *pk,
                    nb_entrees: n as u16,
                    index_page: p,
                    nb_pages_serie: nb_pages,
                    _reserve: 0,
                };
                page[..24].copy_from_slice(bytemuck::bytes_of(&entete));
                if n > 0 {
                    page[24..24 + n * OCTETS_ENTREE]
                        .copy_from_slice(bytemuck::cast_slice(&entrees[d..f]));
                }
                let crc = crc32fast::hash(&page);
                page[4..8].copy_from_slice(&crc.to_le_bytes());
                pages.push(page);
            }
            page_courante += nb_pages;
        }

        let mut racine = [0u8; PAGE_SIZE];
        let entete = EnteteRacine {
            magic: ORD_MAGIC_RACINE,
            version: ORD_VERSION,
            crc32: 0,
            nb_series: descs.len() as u32,
            ancre_wal,
            ancre_pages,
            generation: self.racine.generation.wrapping_add(1),
            _reserve: [0; 4],
        };
        racine[..48].copy_from_slice(bytemuck::bytes_of(&entete));
        for (i, d) in descs.iter().enumerate() {
            racine[48 + i * 16..48 + (i + 1) * 16].copy_from_slice(bytemuck::bytes_of(d));
        }
        let crc = crc32fast::hash(&racine);
        racine[8..12].copy_from_slice(&crc.to_le_bytes());

        let tmp = self
            .chemin
            .with_extension(format!("tmp.{}", crate::wal_io::pid_portable()));
        {
            use std::io::Write as _;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&racine)?;
            for p in &pages {
                f.write_all(p)?;
            }
            f.sync_all()?;
        }
        self.zone = None;
        std::fs::rename(&tmp, &self.chemin)?;
        if let Some(rep) = self.chemin.parent()
            && let Ok(d) = std::fs::File::open(rep)
        {
            let _ = d.sync_all();
        }

        let rouvert = Self::ouvrir_chemin(&self.chemin);
        self.zone = rouvert.zone;
        self.racine = rouvert.racine;
        self.series = rouvert.series;
        self.fiable = self.zone.is_some();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::props_index::encoder_cle;
    use obrain_common::types::Value;

    fn zone_temporaire() -> (SubstrateFile, PropsOrdIndexZone) {
        let sf = SubstrateFile::open_tempfile().unwrap();
        let z = PropsOrdIndexZone::ouvrir(&sf);
        (sf, z)
    }

    /// Serie triee par VALEUR chaine (puis slot). Les slots ne sont PAS dans
    /// l'ordre croissant : c'est tout l'interet (l'ordre est celui des valeurs).
    fn serie_chaines(paires: &[(&str, u32)]) -> Vec<EntreeIndex> {
        let mut v: Vec<(String, u32)> = paires
            .iter()
            .map(|(s, id)| ((*s).to_string(), *id))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        v.into_iter()
            .map(|(s, id)| EntreeIndex::new(encoder_cle(&Value::String(s.into())).unwrap(), id))
            .collect()
    }

    #[test]
    fn un_index_absent_n_est_pas_fiable_et_ne_repond_pas() {
        let (_d, mut z) = zone_temporaire();
        z.confronter_ancres(0, 0);
        assert!(!z.fiable());
        assert_eq!(z.premiers_slots(1, 5), None);
        assert_eq!(z.derniers_slots(1, 5), None);
        assert_eq!(z.nb_entrees(1), None);
    }

    #[test]
    fn premiere_et_derniere_entree_sont_min_et_max() {
        let (_d, mut z) = zone_temporaire();
        // Valeurs volontairement dans le desordre de slot pour prouver que
        // l'ordre stocke est celui de la VALEUR, pas du slot.
        let s = serie_chaines(&[("mango", 3), ("apple", 42), ("zebra", 1), ("banana", 7)]);
        z.fusionner(&[(5u16, s)], 10, 2).unwrap();
        assert!(z.fiable());
        // min = premiere entree = "apple" (slot 42), max = "zebra" (slot 1).
        assert_eq!(z.premiers_slots(5, 1), Some(vec![42]));
        assert_eq!(z.derniers_slots(5, 1), Some(vec![1]));
        assert_eq!(z.nb_entrees(5), Some(4));
        // Ordre complet ascendant : apple(42), banana(7), mango(3), zebra(1).
        assert_eq!(z.premiers_slots(5, 4), Some(vec![42, 7, 3, 1]));
        // Les 2 plus grandes (mango, zebra) rendues en ordre ascendant.
        assert_eq!(z.derniers_slots(5, 2), Some(vec![3, 1]));
        // Propriete inconnue : None, jamais Some(vec![]).
        assert_eq!(z.premiers_slots(9, 1), None);
    }

    #[test]
    fn premiers_slots_bornes_a_k_meme_sur_grande_serie() {
        let (_d, mut z) = zone_temporaire();
        // 5 pages et quelques : cle STRING partagee sur les 11 octets => cle
        // egale, l'ordre stocke doit rester valeur puis slot.
        let n = ENTREES_PAR_PAGE * 5 + 3;
        let paires: Vec<(String, u32)> = (0..n)
            .map(|i| (format!("valeur-{i:08}"), (n - i) as u32))
            .collect();
        let refp: Vec<(&str, u32)> = paires.iter().map(|(s, id)| (s.as_str(), *id)).collect();
        let s = serie_chaines(&refp);
        z.fusionner(&[(1u16, s)], 0, 0).unwrap();
        let p20 = z.premiers_slots(1, 20).unwrap();
        assert_eq!(p20.len(), 20, "borne a k, ne lit pas toute la serie");
        // "valeur-00000000" est le plus petit -> slot n, etc.
        assert_eq!(p20[0], n as u32);
        assert_eq!(p20[1], (n - 1) as u32);
        assert_eq!(z.nb_entrees(1), Some(n as u32));
    }

    #[test]
    fn les_ancres_qui_divergent_rendent_l_index_non_fiable() {
        let (_d, mut z) = zone_temporaire();
        z.fusionner(&[(1u16, serie_chaines(&[("a", 1), ("b", 2)]))], 500, 12)
            .unwrap();
        assert!(z.fiable());
        z.confronter_ancres(500, 12);
        assert!(z.fiable(), "ancres identiques : fiable");
        z.confronter_ancres(501, 12);
        assert!(!z.fiable(), "le WAL a avance : NON fiable");
        assert_eq!(z.premiers_slots(1, 1), None);
    }

    #[test]
    fn persiste_verbatim_l_ordre_du_caller_sans_verifier_la_cle() {
        // La zone ne verifie AUCUN ordre : elle persiste la liste telle quelle.
        // Ici la cle serait "non monotone" (zzz avant aaa) mais c'est admis,
        // car pour comparer_total une chaine datetime passe AVANT une chaine
        // ordinaire (rang Timestamp) alors que sa cle octets-bruts serait plus
        // grande. La zone ne doit pas s'en meler.
        let (_d, mut z) = zone_temporaire();
        let s = vec![
            EntreeIndex::new(encoder_cle(&Value::String("zzz".into())).unwrap(), 1),
            EntreeIndex::new(encoder_cle(&Value::String("aaa".into())).unwrap(), 2),
        ];
        assert!(z.fusionner(&[(1u16, s)], 0, 0).is_ok());
        // Iteration = ordre fourni, verbatim.
        assert_eq!(z.premiers_slots(1, 2), Some(vec![1, 2]));
        assert_eq!(z.derniers_slots(1, 1), Some(vec![2]));
    }

    #[test]
    fn une_fusion_survit_a_la_reouverture_sans_reconstruction() {
        let sf = SubstrateFile::open_tempfile().unwrap();
        let mut z = PropsOrdIndexZone::ouvrir(&sf);
        let paires: Vec<(String, u32)> = (0..700).map(|i| (format!("v{i:05}"), i as u32)).collect();
        let refp: Vec<(&str, u32)> = paires.iter().map(|(s, id)| (s.as_str(), *id)).collect();
        z.fusionner(&[(3u16, serie_chaines(&refp))], 77, 9).unwrap();
        let g = z.generation();
        drop(z);
        let mut z2 = PropsOrdIndexZone::ouvrir(&sf);
        z2.confronter_ancres(77, 9);
        assert!(z2.fiable());
        assert_eq!(z2.generation(), g);
        assert_eq!(z2.nb_entrees(3), Some(700));
        assert_eq!(z2.premiers_slots(3, 1), Some(vec![0]));
        assert_eq!(z2.derniers_slots(3, 1), Some(vec![699]));
    }

    #[test]
    fn deux_fusions_du_meme_contenu_sont_identiques_octet_a_octet() {
        let paires: Vec<(String, u32)> = (0..300).map(|i| (format!("q{i:05}"), i as u32)).collect();
        let refp: Vec<(&str, u32)> = paires.iter().map(|(s, id)| (s.as_str(), *id)).collect();
        let lire = || {
            let sf = SubstrateFile::open_tempfile().unwrap();
            let mut z = PropsOrdIndexZone::ouvrir(&sf);
            z.fusionner(&[(2u16, serie_chaines(&refp))], 1, 1).unwrap();
            std::fs::read(z.chemin()).unwrap()
        };
        // Note: la generation part de 0 dans les deux (zone fraiche) -> egaux.
        assert_eq!(lire(), lire());
    }
}
