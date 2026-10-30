//! Fenêtres 2D par communauté sur le plan `(centrality_cached × out-degree)`
//! — RFC zorder-superposition, capacité C1.
//!
//! Le substrat expose le scan EXACT de référence (`SubstrateStore::window_query_2d`),
//! baseline que les benches Z-order (B2) doivent battre. Ce module ne fait que
//! l'exposer au niveau moteur : délégation au store + élargissement des slots
//! `u32` en ids de nœuds `u64`.

use obrain_common::utils::error::{Error, Result};

impl super::ObrainDB {
    /// 2D window query over the `(centrality_cached × out-degree)` plane,
    /// scoped to one community (RFC zorder-superposition, capacité C1).
    ///
    /// Délègue au scan EXACT de
    /// [`SubstrateStore::window_query_2d`](obrain_substrate::SubstrateStore::window_query_2d) :
    /// renvoie les ids (en `u64`) des nœuds VIVANTS de `community_id` dont la
    /// centralité de PageRank en cache (`centrality_cached`, Q0.16) tombe dans
    /// `[c_lo, c_hi]` ET dont le degré sortant tombe dans `[d_lo, d_hi]`.
    ///
    /// Bornes inclusives et insensibles à l'ordre (normalisées par le store).
    /// Jamais dépendant des side-columns Hilbert/Z-order : c'est la baseline
    /// exacte que les benches B2 du RFC doivent battre.
    ///
    /// # Errors
    ///
    /// `Error::Internal` quand la base n'est pas adossée à un store substrate
    /// (`self.substrate_store` est `None`) — message explicite — ou quand le
    /// scan substrate échoue.
    pub fn window_query_2d(
        &self,
        community_id: u32,
        c_lo: u16,
        c_hi: u16,
        d_lo: u32,
        d_hi: u32,
    ) -> Result<Vec<u64>> {
        let Some(ref store) = self.substrate_store else {
            return Err(Error::Internal(
                "window_query_2d requires a substrate-backed database".to_string(),
            ));
        };
        let slots = store
            .window_query_2d(community_id, c_lo, c_hi, d_lo, d_hi)
            .map_err(|e| Error::Internal(format!("window_query_2d failed: {e}")))?;
        // Les slots substrate SONT des ids de nœuds u32 ; le moteur les
        // élargit en u64 pour coller à la convention NodeId du reste de l'API.
        Ok(slots.into_iter().map(u64::from).collect())
    }

    /// 2D window query ACCÉLÉRÉE (zruns) sur une communauté d'un store trié
    /// — RFC zorder-superposition D3b. Contrat « layout trié » : correct tant
    /// qu'aucune feature n'a changé depuis le dernier `repermuter_par_hilbert`
    /// (voir `SubstrateStore::window_query_2d_sorted` / `sorted_view_valid`).
    /// Repli scan exact automatique si non trié / grille absente / fenêtre
    /// large. Requiert une base substrate.
    pub fn window_query_2d_sorted(
        &self,
        community_id: u32,
        c_lo: u16,
        c_hi: u16,
        d_lo: u32,
        d_hi: u32,
    ) -> Result<Vec<u64>> {
        let Some(ref store) = self.substrate_store else {
            return Err(Error::Internal(
                "window_query_2d_sorted requires a substrate-backed database".to_string(),
            ));
        };
        let slots = store
            .window_query_2d_sorted(community_id, c_lo, c_hi, d_lo, d_hi)
            .map_err(|e| Error::Internal(format!("window_query_2d_sorted failed: {e}")))?;
        Ok(slots.into_iter().map(u64::from).collect())
    }
}
