//! Helper de FABRICATION de fixtures legacy — réservé aux tests d'intégration
//! de obrain-substrate.
//!
//! Réplique l'ancien `Writer::set_engram_weights_legacy` (retiré de l'API
//! publique de production) via l'API publique uniquement : append WAL du record
//! `EngramWeightsSet` (0x76) + écriture des poids BRUTS (non liés, sans
//! empreinte) dans la zone des poids.

use obrain_substrate::engram::EngramZone;
use obrain_substrate::wal::{WalPayload, WalRecord};
use obrain_substrate::{SubstrateResult, Writer, Zone};

/// Écrit des poids d'engramme NON LIÉS (legacy 0x76), comme le faisait l'API V3.
///
/// ⚠️ TEST UNIQUEMENT : sous V4 cette forme n'est pas lisible de façon
/// cohérente (classée `PoidsNonLies` / refusée). Elle ne sert qu'à fabriquer
/// un état legacy (migration, non-recertification au remap, rejeu).
pub fn set_engram_weights_legacy(
    w: &Writer,
    engram_id: u16,
    weights: Vec<f32>,
) -> SubstrateResult<()> {
    EngramZone::verifier_engram_id(engram_id)?;
    let bits: Vec<u32> = weights.iter().map(|w| w.to_bits()).collect();
    let rec = WalRecord {
        lsn: 0,
        timestamp: 0,
        flags: 0,
        payload: WalPayload::EngramWeightsSet {
            engram_id,
            weights: bits,
        },
    };
    w.wal().append(rec)?;
    let mut zf = w.substrate().lock().open_zone(Zone::EngramWeights)?;
    EngramZone::set_weights_raw(&mut zf, engram_id, &weights)
}
