//! # Matérialisation de la décroissance sur la colonne d'énergie du substrat
//!
//! En mode substrat, l'énergie vit dans `NodeRecord.energy` (Q1.15) et l'état
//! temporel dans UNE SEULE propriété composite
//! `_cog_energy_anchor` = `(E_ref, t_ref, t_hi, H)` (32 octets `Bytes`). Ce
//! module est la seule porte entre les deux.
//!
//! ## ATOMICITÉ (point 1 de la revue)
//!
//! L'ancre n'est PLUS une propriété `f64` séparée écrite après la colonne :
//! c'est une propriété COMPOSITE qui porte `(E_ref, t_ref)` ensemble. Une
//! propriété s'écrit en UNE mutation (un enregistrement WAL, ou un append de
//! zone v2), donc l'état temporel est écrit atomiquement. La colonne n'est
//! plus la source de vérité : c'est un MIROIR (best-effort) pour les lecteurs
//! directs du moteur. Un `kill -9` entre la propriété composite et le miroir
//! laisse la propriété cohérente, et la lecture expose toujours 0,5 ou 0,75 —
//! jamais 0,375 ni 1,0.
//!
//! ## Lecture paresseuse, tique facultative
//!
//! La valeur rendue par `current_energy` est calculée depuis l'ancre, donc
//! elle est **exacte sans qu'aucune minuterie ne tourne**. La tique
//! périodique (`advance_all`) ne sert qu'à faire vieillir les nœuds que
//! PERSONNE NE LIT, pour `list_low_energy` et l'archivage.
//!
//! ## Contrat de monotonie
//!
//! Monotonie garantie au sein d'une session (via `session_hi`, en mémoire,
//! dans [`crate::energy::EnergyStore`]) ; à travers un redémarrage, reprise
//! sur la dernière borne durable `t_hi` (écrite seulement par `reinforce` /
//! `advance_all`). Une lecture seule d'un instant plus grand n'est PAS
//! persistée : après un crash suivi d'un recul d'horloge, l'énergie peut
//! remonter vers la valeur impliquée par le dernier `t_hi` durable —
//! limitation documentée, pas une corruption.

use std::sync::Arc;
use std::time::Duration;

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;

use crate::decay_law::{
    DecayLaw, EnergyAnchor, elapsed_monotonic, elapsed_since, epoch_secs_to_micros,
    micros_to_epoch_secs, unix_micros,
};
use crate::store_trait::{PROP_ENERGY_ANCHOR, PROP_ENERGY_LAST_ACTIVATED_EPOCH, load_node_f64};

/// Nombre de champs `f64` de l'ancre composite : `E_ref, t_ref, t_hi, H`.
const ANCHOR_FIELDS: usize = 4;
const ANCHOR_BYTES: usize = ANCHOR_FIELDS * 8;

fn cle_ancre() -> PropertyKey {
    PropertyKey::from(PROP_ENERGY_ANCHOR)
}

/// Encode l'ancre en une propriété `Bytes` de 32 octets (4 × `f64` LE).
fn encode_anchor(a: &EnergyAnchor) -> Value {
    let champs = [
        a.e_ref,
        micros_to_epoch_secs(a.t_ref_micros),
        micros_to_epoch_secs(a.t_hi_micros),
        a.half_life.as_secs_f64(),
    ];
    let mut bytes = [0u8; ANCHOR_BYTES];
    for (i, x) in champs.iter().enumerate() {
        bytes[i * 8..i * 8 + 8].copy_from_slice(&x.to_le_bytes());
    }
    Value::Bytes(Arc::from(bytes.as_slice()))
}

/// Décode la propriété composite. `None` si absente, mal formée ou aberrante.
fn decode_anchor(v: &Value) -> Option<EnergyAnchor> {
    let bytes = v.as_bytes()?;
    if bytes.len() != ANCHOR_BYTES {
        return None;
    }
    let mut champs = [0f64; ANCHOR_FIELDS];
    for (i, slot) in champs.iter_mut().enumerate() {
        *slot = f64::from_le_bytes(bytes[i * 8..i * 8 + 8].try_into().ok()?);
    }
    let e_ref = champs[0];
    let t_ref = epoch_secs_to_micros(champs[1]);
    let t_hi = epoch_secs_to_micros(champs[2]);
    // `try_from_secs_f64` rejette NaN, infinie, négative et hors capacité
    // SANS paniquer ; la validation se fait donc APRÈS une conversion sûre.
    let h = match Duration::try_from_secs_f64(champs[3]) {
        Ok(h) => h,
        Err(_) => return None,
    };
    if !e_ref.is_finite() || e_ref <= 0.0 || t_ref <= 0 {
        return None;
    }
    Some(EnergyAnchor {
        e_ref,
        t_ref_micros: t_ref,
        t_hi_micros: t_hi.max(t_ref),
        half_life: h,
    })
}

/// Lit l'ancre composite d'un nœud. `None` = jamais ancré (ou donnée héritée).
pub fn read_anchor(sub: &SubstrateStore, node_id: NodeId) -> Option<EnergyAnchor> {
    match read_anchor_state(sub, node_id) {
        AnchorState::Valid(a) => Some(a),
        _ => None,
    }
}

/// Lecture TYPÉE de l'ancre : la seule source d'interprétation. `read_anchor`
/// (Option) est conservé pour compatibilité, mais perd la distinction
/// absence/corruption — les chemins énergétiques utilisent `read_anchor_state`.
pub fn read_anchor_state(sub: &SubstrateStore, node_id: NodeId) -> AnchorState {
    decode_anchor_state(&sub.get_node_property(node_id, &cle_ancre()))
}

/// Lecture énergétique QUALIFIÉE : distingue les états que `get_energy` (f64)
/// confond. Contrat minimal consommé par les chemins décisionnels.
#[derive(Debug, Clone, PartialEq)]
pub enum EnergyRead {
    /// Ancre valide : énergie effective calculable, exploitable.
    Effective(f64),
    /// Ancre corrompue : énergie stockée consultable (diagnostic), NON exploitable.
    Corrupted { stored: f64 },
    /// Pas d'ancre : temporalité inconnue, colonne héritée (diagnostic).
    UnknownTemporal { stored: f64 },
    /// Pas d'ancre et colonne nulle (ou nœud absent — le substrat ne distingue
    /// pas encore ces deux causes : limite documentée).
    Absent,
}

/// Lecture qualifiée de l'énergie d'un nœud. `Absent` couvre « nœud absent » et
/// « sans ancre + colonne nulle » (l'API historique `Option` ne les distingue
/// pas) ; une lecture en erreur y est aussi rabattue — à qualifier avec les
/// primitives versionnées avant d'affirmer une absence certaine.
pub fn read_energy_qualified(
    sub: &SubstrateStore,
    node_id: NodeId,
    law: &DecayLaw,
    now_micros: i64,
) -> EnergyRead {
    match read_anchor_state(sub, node_id) {
        AnchorState::Valid(_) => match energy_at(sub, node_id, law, now_micros) {
            Some(e) => EnergyRead::Effective(e),
            None => EnergyRead::Absent,
        },
        AnchorState::Corrupted => {
            let stored = sub
                .get_node_energy_f32(node_id)
                .ok()
                .flatten()
                .unwrap_or(0.0) as f64;
            EnergyRead::Corrupted { stored }
        }
        AnchorState::Absent => {
            let stored = sub
                .get_node_energy_f32(node_id)
                .ok()
                .flatten()
                .unwrap_or(0.0) as f64;
            if stored > 0.0 {
                EnergyRead::UnknownTemporal { stored }
            } else {
                EnergyRead::Absent
            }
        }
    }
}

/// État typé de l'ancre composite : distingue ABSENCE légitime, VALIDITÉ et
/// CORRUPTION. Une corruption ne doit jamais être confondue avec une absence.
#[derive(Debug, Clone, PartialEq)]
pub enum AnchorState {
    Absent,
    Valid(EnergyAnchor),
    Corrupted,
}

fn decode_anchor_state(ancre: &Option<Value>) -> AnchorState {
    match ancre {
        None => AnchorState::Absent,
        Some(v) => match decode_anchor(v) {
            Some(a) => AnchorState::Valid(a),
            None => AnchorState::Corrupted,
        },
    }
}

/// Ancre héritée (propriété `f64` séparée, avant l'ancre composite) : l'instant
/// seul. L'énergie de référence est alors la colonne.
pub fn legacy_anchor(store: &dyn GraphStoreMut, node_id: NodeId) -> Option<i64> {
    load_node_f64(store, node_id, PROP_ENERGY_LAST_ACTIVATED_EPOCH)
        .filter(|s| s.is_finite() && *s > 0.0)
        .map(crate::decay_law::epoch_secs_to_micros)
}

/// Écrit l'ancre composite d'un nœud. `half_life == Duration::ZERO` signifie
/// « non figée » : la lecture utilisera la loi passée par l'appelant.
fn write_anchor(
    sub: &SubstrateStore,
    node_id: NodeId,
    e_ref: f64,
    t_ref_micros: i64,
    t_hi_micros: i64,
    half_life: Duration,
) {
    let a = EnergyAnchor {
        e_ref,
        t_ref_micros,
        t_hi_micros: t_hi_micros.max(t_ref_micros),
        half_life,
    };
    sub.set_node_property(node_id, PROP_ENERGY_ANCHOR, encode_anchor(&a));
}

/// Helper de test/migration : écrit une ancre composite EXPLICITE
/// `(e_ref, t_ref, H)`, high-water mark = t_ref.
#[doc(hidden)]
pub fn persist_anchor_explicit(
    sub: &SubstrateStore,
    node_id: NodeId,
    e_ref: f64,
    t_ref_micros: i64,
    half_life: Duration,
) {
    write_anchor(sub, node_id, e_ref, t_ref_micros, t_ref_micros, half_life);
}

/// Écrit l'ancre physique d'un nœud (micros Unix) en conservant la colonne
/// comme énergie de référence et SANS figer la demi-vie (`H = 0`). Réservé aux
/// chemins de test et de ré-ancrage où la loi est connue de l'appelant.
pub fn persist_anchor(sub: &SubstrateStore, node_id: NodeId, micros: i64) {
    let e_ref = sub
        .get_node_energy_f32(node_id)
        .ok()
        .flatten()
        .unwrap_or(0.0) as f64;
    write_anchor(sub, node_id, e_ref, micros, micros, Duration::ZERO);
}

/// Énergie d'un nœud à `now`, sans rien écrire.
///
/// `None` si le nœud n'est pas vivant (slot non alloué ou enterré).
pub fn energy_at(
    sub: &SubstrateStore,
    node_id: NodeId,
    law: &DecayLaw,
    now_micros: i64,
) -> Option<f64> {
    // L'ancre composite est la source de vérité ATOMIQUE.
    match read_anchor_state(sub, node_id) {
        AnchorState::Valid(a) => {
            return Some(if a.half_life.is_zero() {
                // H non figée (test / ré-ancrage) : loi de l'appelant, garde monotone.
                law.energy_at(
                    a.e_ref,
                    elapsed_monotonic(a.t_ref_micros, now_micros, a.t_hi_micros),
                )
            } else {
                a.energy_at(now_micros)
            });
        }
        AnchorState::Corrupted => {
            // Ancre présente mais corrompue : l'énergie CANONIQUE (colonne) est
            // préservée telle quelle — PAS de décroissance, PAS de repli
            // historique (jamais confondue avec une absence).
            let e_ref = sub.get_node_energy_f32(node_id).ok().flatten()? as f64;
            return Some(if e_ref <= 0.0 { 0.0 } else { e_ref });
        }
        AnchorState::Absent => {}
    }
    // Donnée héritée (absence légitime) : colonne = E_ref, propriété = t_ref.
    let e_ref = sub.get_node_energy_f32(node_id).ok().flatten()? as f64;
    if e_ref <= 0.0 {
        return Some(0.0);
    }
    match legacy_anchor(sub, node_id) {
        Some(t_ref) => Some(law.energy_at(e_ref, elapsed_since(t_ref, now_micros))),
        // Ancienne base SANS ancre : âge inconnu. On ne l'invente pas (pas de
        // remise à zéro silencieuse) : on rend E_ref tel quel, et l'ancrage de
        // migration est fait par `advance_all` au prochain passage.
        None => Some(e_ref),
    }
}

/// Énergie d'un nœud à l'instant courant.
pub fn current_energy(sub: &SubstrateStore, node_id: NodeId, law: &DecayLaw) -> Option<f64> {
    energy_at(sub, node_id, law, unix_micros())
}

/// Matérialise un nœud : applique la décroissance écoulée, ajoute `delta`,
/// écrit l'ancre composite (une seule mutation atomique), puis reflète la
/// colonne. Rend la valeur écrite.
pub fn reinforce(
    sub: &SubstrateStore,
    node_id: NodeId,
    delta: f64,
    law: &DecayLaw,
    max_energy: f64,
    now_micros: i64,
) -> Option<f64> {
    // Temps logique de la mutation : τ = max(now, t_hi, t_ref) lu depuis
    // l'ancre PERSISTÉE. L'autorité temporelle ne dépend ni du chemin d'accès
    // ni du session_hi volatil du wrapper.
    let (tau, h) = match read_anchor_state(sub, node_id) {
        AnchorState::Valid(a) => (
            now_micros.max(a.t_hi_micros).max(a.t_ref_micros),
            if a.half_life.is_zero() {
                law.half_life()
            } else {
                a.half_life
            },
        ),
        // Absence légitime : migration (ancrage à now).
        AnchorState::Absent => (now_micros, law.half_life()),
        // Corruption : PAS de restauration implicite. Le renforcement ne peut
        // pas réparer une ancre temporellement invalide.
        AnchorState::Corrupted => return None,
    };
    // L'énergie est calculée au MÊME temps logique τ auquel l'ancre est ancrée :
    // jamais de décroissance entre le calcul et l'ancrage.
    let e_courante = energy_at(sub, node_id, law, tau)?;
    let plafond = max_energy.min(1.0);
    let nouvelle = (e_courante + delta).clamp(0.0, plafond);

    // UNE mutation atomique porte tout l'état temporel, ancrée à τ.
    write_anchor(sub, node_id, nouvelle, tau, tau, h);
    // Point d'injection (test de durabilité) : un SIGKILL ICI laisse l'ancre
    // écrite mais pas le miroir — la lecture expose alors 0,5 ou 0,75, jamais
    // une ancre déchirée (0,375 / 1,0).
    obrain_common::utils::panne::point_entre_ancre_et_miroir();
    // Miroir de colonne, best-effort : la lecture ne dépend pas de lui.
    let _ = sub.set_node_energy_f32(node_id, nouvelle as f32);
    Some(nouvelle)
}

/// Comptes d'une avance de tique.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DecayAdvance {
    /// Slots vivants examinés.
    pub scanned: u64,
    /// Nœuds effectivement avancés (ancre composite + colonne réécrites).
    pub advanced: u64,
    /// Nœuds d'énergie nulle : rien à faire, aucune ancre écrite.
    pub dormant: u64,
    /// Nœuds non traités faute de budget — rattrapés à la tique suivante.
    pub deferred: u64,
    /// Nœuds SANS ancre (colonne posée hors de ce module) — ancrés maintenant.
    pub unanchored: u64,
    /// Nœuds dont l'ancre composite est PRÉSENTE mais corrompue : non migrés,
    /// non décrus, énergie canonique préservée (verdict de corruption).
    pub corrupted: u64,
}

/// Avance jusqu'à `max_nodes` nœuds le long de LEUR propre loi, depuis LEUR
/// propre ancre, jusqu'à `now`. `max_nodes == 0` = pas de borne.
///
/// Les nœuds sans ancre (ancienne base) sont ANCRÉS à `now` — migration
/// explicite, dans le chemin d'écriture — au lieu d'être remis à zéro
/// silencieusement à la lecture.
pub fn advance_all(
    sub: &SubstrateStore,
    law: &DecayLaw,
    max_nodes: usize,
    now_micros: i64,
) -> obrain_substrate::SubstrateResult<DecayAdvance> {
    let mut stats = DecayAdvance::default();
    let slots = sub.iter_live_node_energies()?;
    stats.scanned = slots.len() as u64;

    let ids: Vec<NodeId> = slots.iter().map(|(id, _)| *id).collect();
    let ancres = sub.get_node_property_batch(&ids, &cle_ancre());

    // Assemble les nœuds vivants avec leur clé de vieillesse `(t_hi, t_ref)`.
    // Les dormants (colonne ≤ 0) sont comptés et écartés du tri : la loi y est
    // un point fixe, aucune ancre à écrire.
    let mut candidats: Vec<((i64, i64, NodeId), f64, Option<EnergyAnchor>, Option<i64>)> =
        Vec::new();
    for ((node_id, e_ref_col), ancre) in slots.into_iter().zip(ancres) {
        let e_ref_col = e_ref_col as f64;
        if e_ref_col <= 0.0 {
            stats.dormant += 1;
            continue;
        }
        let ancre_etat = decode_anchor_state(&ancre);
        if matches!(ancre_etat, AnchorState::Corrupted) {
            // Corruption : PAS de migration, PAS de décroissance. L'énergie
            // canonique (colonne) est préservée, le nœud est signalé.
            stats.corrupted += 1;
            continue;
        }
        let ancre_dec = match ancre_etat {
            AnchorState::Valid(a) => Some(a),
            _ => None,
        };
        let legacy = match &ancre_dec {
            Some(_) => None,
            None => legacy_anchor(sub, node_id),
        };
        let (t_hi, t_ref) = match &ancre_dec {
            Some(a) => (a.t_hi_micros, a.t_ref_micros),
            None => match legacy {
                Some(t) => (t, t),
                // Non ancré : le plus ancien, migré en priorité.
                None => (i64::MIN, i64::MIN),
            },
        };
        candidats.push(((t_hi, t_ref, node_id), e_ref_col, ancre_dec, legacy));
    }

    // Tri par ancienneté : `t_hi` asc (le nœud le moins avancé d'abord), puis
    // `t_ref` asc, puis `node_id` — déterministe. La « position de reprise »
    // est implicite via le high-water mark : un nœud fraîchement avancé a un
    // `t_hi` plus grand et repart en fond de file ; pas de curseur de slot
    // fragile qui affamerait les nœuds de tête en permanence.
    candidats.sort_by_key(|(cle, _, _, _)| *cle);

    for ((_t_hi, _t_ref, node_id), e_ref_col, ancre_dec, legacy) in candidats {
        if max_nodes != 0 && stats.advanced as usize >= max_nodes {
            stats.deferred += 1;
            continue;
        }

        let (e_ref, t_ref, t_hi, h_frozen) = match ancre_dec {
            Some(a) => (a.e_ref, a.t_ref_micros, a.t_hi_micros, a.half_life),
            None => match legacy {
                // Hérité (propriété f64 séparée) : E_ref = colonne.
                Some(t_ref) => (e_ref_col, t_ref, t_ref, Duration::ZERO),
                None => {
                    // Aucune ancre : ancrage de migration à now (point 4).
                    write_anchor(
                        sub,
                        node_id,
                        e_ref_col,
                        now_micros,
                        now_micros,
                        law.half_life(),
                    );
                    stats.unanchored += 1;
                    continue;
                }
            },
        };
        let h = if h_frozen.is_zero() {
            law.half_life()
        } else {
            h_frozen
        };
        let dt = elapsed_monotonic(t_ref, now_micros, t_hi);
        if dt.is_zero() {
            continue;
        }
        let nouvelle = DecayLaw::new(h).energy_at(e_ref, dt);
        // Une valeur qui ne change pas au bit près ne mérite pas deux écritures.
        if (nouvelle as f32) == (e_ref as f32) {
            continue;
        }
        write_anchor(sub, node_id, nouvelle, now_micros, t_hi, h);
        if sub.set_node_energy_f32(node_id, nouvelle as f32).is_ok() {
            stats.advanced += 1;
        }
    }
    Ok(stats)
}

/// Énergie courante à partir des pièces déjà lues en lot (colonne, ancre
/// composite, ancre héritée). Facteur commun des lectures groupées
/// (`EnergyStore::snapshot`) : l'ancre composite est la source de vérité.
pub fn energy_from_parts(
    e_ref_col: f64,
    composite: Option<&Value>,
    legacy_t_micros: Option<i64>,
    law: &DecayLaw,
    now_micros: i64,
) -> f64 {
    if let Some(a) = composite.and_then(decode_anchor) {
        return if a.half_life.is_zero() {
            law.energy_at(
                a.e_ref,
                elapsed_monotonic(a.t_ref_micros, now_micros, a.t_hi_micros),
            )
        } else {
            a.energy_at(now_micros)
        };
    }
    if e_ref_col <= 0.0 {
        return 0.0;
    }
    match legacy_t_micros {
        Some(t) => law.energy_at(e_ref_col, elapsed_since(t, now_micros)),
        None => e_ref_col,
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn anchor_value(e: f64, t_ref: f64, t_hi: f64, h: f64) -> Value {
        let mut bytes = [0u8; ANCHOR_BYTES];
        for (i, x) in [e, t_ref, t_hi, h].iter().enumerate() {
            bytes[i * 8..i * 8 + 8].copy_from_slice(&x.to_le_bytes());
        }
        Value::Bytes(Arc::from(bytes.as_slice()))
    }

    #[test]
    fn decode_anchor_refuse_les_demi_vies_invalides_sans_paniquer() {
        for h in [f64::NEG_INFINITY, f64::NAN, f64::INFINITY, -1.0, 1e300] {
            let v = anchor_value(1.0, 1.0, 1.0, h);
            assert!(decode_anchor(&v).is_none(), "demi-vie invalide {h} → None (sans panic)");
        }
    }

    #[test]
    fn decode_anchor_refuse_e_ref_invalide_et_octets_mal_formes() {
        for e in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let v = anchor_value(e, 1.0, 1.0, 64.0);
            assert!(decode_anchor(&v).is_none(), "e_ref invalide {e} → None");
        }
        let bad = Value::Bytes(Arc::from(vec![1u8, 2, 3].as_slice()));
        assert!(decode_anchor(&bad).is_none(), "longueur ≠ ANCHOR_BYTES → None");
    }

    #[test]
    fn decode_anchor_accepte_une_ancre_valide() {
        let ok = anchor_value(1.0, 1.0, 1.0, 64.0);
        let a = decode_anchor(&ok).expect("ancre valide");
        assert!((a.e_ref - 1.0).abs() < 1e-9);
        assert_eq!(a.half_life, Duration::from_secs(64));
    }
}
