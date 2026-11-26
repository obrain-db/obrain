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
use obrain_substrate::{
    IssuePublication, ModePublication, Precondition, SubstrateResult, SubstrateStore,
};

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
    // e_ref == 0 est une énergie légitime (décroissance complète) : seul
    // e_ref < 0 ou non fini est aberrant.
    if !e_ref.is_finite() || e_ref < 0.0 || t_ref <= 0 {
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
    /// Ancre corrompue : miroir consultable (diagnostic), NON exploitable.
    /// `stored: None` = miroir lui-même indisponible (erreur de lecture).
    Corrupted { stored: Option<f64> },
    /// Pas d'ancre (temporalité inconnue) : colonne héritée. `stored: None` =
    /// miroir indisponible. Un nœud vivant sans ancre et colonne nulle est ici,
    /// PAS dans `Absent`.
    Unanchored { stored: Option<f64> },
    /// Nœud réellement absent (ou tombstoné) : `get_node_energy_f32` rend `Ok(None)`.
    Absent,
    /// Lecture en erreur : `get_node_energy_f32` rend `Err` — PAS une absence
    /// certaine, PAS un zéro.
    Unqualified,
}

/// Lecture qualifiée. Préserve les distinctions disponibles dans
/// `get_node_energy_f32 -> SubstrateResult<Option<f32>>` : `Err` (lecture en
/// erreur) n'est pas rabattue en absence ni en zéro ; `Ok(None)` (nœud absent
/// ou tombstoné) n'est pas confondu avec un nœud vivant sans ancre.
pub fn read_energy_qualified(
    sub: &SubstrateStore,
    node_id: NodeId,
    law: &DecayLaw,
    now_micros: i64,
) -> EnergyRead {
    match read_anchor_state(sub, node_id) {
        AnchorState::Valid(_) => match energy_at(sub, node_id, law, now_micros) {
            Some(e) => EnergyRead::Effective(e),
            None => EnergyRead::Unqualified,
        },
        AnchorState::Corrupted => EnergyRead::Corrupted {
            stored: match sub.get_node_energy_f32(node_id) {
                Ok(Some(v)) => Some(v as f64),
                Ok(None) => None,
                Err(_) => None,
            },
        },
        AnchorState::Absent => match sub.get_node_energy_f32(node_id) {
            Ok(Some(v)) => EnergyRead::Unanchored { stored: Some(v as f64) },
            Ok(None) => EnergyRead::Absent,
            Err(_) => EnergyRead::Unqualified,
        },
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

/// Écrit l'ancre composite d'un nœud SOUS L'AUTORITÉ CONDITIONNELLE NATIVE.
///
/// `pre` est la précondition CAPTURÉE qui a servi au calcul (même lecture, même
/// estampille HLC) — l'ordre est : capturer → calculer → préparer → publier.
/// `half_life == Duration::ZERO` signifie « non figée » : la lecture utilisera
/// la loi passée par l'appelant.
///
/// Cycle `Strict` : [`SubstrateStore::preparer_transformation`] puis
/// [`SubstrateStore::publier_si_precondition_intacte`]. Rend
/// `ConflitPrecondition` si l'ancre a changé depuis la capture — l'appelant
/// doit alors re-capturer et RECALCULER `e_courante` depuis la nouvelle ancre,
/// JAMAIS re-publier un delta additif aveugle.
fn write_anchor(
    sub: &SubstrateStore,
    node_id: NodeId,
    a: &EnergyAnchor,
    pre: &Precondition,
) -> SubstrateResult<(IssuePublication, (u64, u64))> {
    let prep = sub.preparer_transformation(
        node_id,
        &[(PROP_ENERGY_ANCHOR, encode_anchor(a))],
        pre,
        ModePublication::Strict,
    )?;
    // Estampille HLC attribuée aux écritures de la transformation : elle permet
    // de conditionner l'écriture du miroir de colonne à la canonicité de CETTE
    // ancre — jamais un miroir obsolète (fenêtre ancre→miroir fermée).
    let estampille = prep.estampille_effet();
    let issue = sub.publier_si_precondition_intacte(prep)?;
    Ok((issue, estampille))
}

/// Ancre composite CAPTURÉE sous l'autorité conditionnelle : distingue validité,
/// absence et corruption depuis la VALEUR capturée (jamais une seconde lecture).
#[derive(Debug, Clone, PartialEq)]
enum AncreCapturee {
    Valid(EnergyAnchor),
    Absente,
    Corrompue,
}

/// Capture l'ancre composite (valeur + estampille HLC) liée au store et au
/// nœud, prête à servir au calcul PUIS à la publication conditionnelle.
fn capturer_ancre(
    sub: &SubstrateStore,
    node_id: NodeId,
) -> SubstrateResult<(Precondition, AncreCapturee)> {
    let pre = sub.capturer_precondition(node_id, &[PROP_ENERGY_ANCHOR])?;
    let etat = match pre.valeur_capturee(PROP_ENERGY_ANCHOR) {
        Some(v) => match decode_anchor(v) {
            Some(a) => AncreCapturee::Valid(a),
            None => AncreCapturee::Corrompue,
        },
        None => AncreCapturee::Absente,
    };
    Ok((pre, etat))
}

/// Construit une ancre composite en bornant le high-water mark (jamais en-deçà
/// de `t_ref`).
fn construire_ancre(
    e_ref: f64,
    t_ref_micros: i64,
    t_hi_micros: i64,
    half_life: Duration,
) -> EnergyAnchor {
    EnergyAnchor {
        e_ref,
        t_ref_micros,
        t_hi_micros: t_hi_micros.max(t_ref_micros),
        half_life,
    }
}

/// Helper de test/migration : écrit une ancre composite EXPLICITE
/// `(e_ref, t_ref, H)`, high-water mark = t_ref. Publie sous l'autorité
/// conditionnelle ; en UNE SEULE tentative : sur conflit (ancre concurrente
/// plus recente), on abandonne SANS re-publier des valeurs fixes.
#[doc(hidden)]
pub fn persist_anchor_explicit(
    sub: &SubstrateStore,
    node_id: NodeId,
    e_ref: f64,
    t_ref_micros: i64,
    half_life: Duration,
) {
    let a = construire_ancre(e_ref, t_ref_micros, t_ref_micros, half_life);
    // VALEURS FIXES : une SEULE tentative. Un conflit signifie qu'une mutation
    // CONCURRENTE a pose une ancre plus recente ; re-publier les memes valeurs
    // fixes l'ECRASERAIT (dominance HLC strictement croissante). On abandonne,
    // sans jamais ecraser une mutation concurrente. Un Err (effet indetermine)
    // n'est pas converti en resolution : ce helper best-effort l'ignore.
    let Ok((pre, _)) = capturer_ancre(sub, node_id) else {
        return;
    };
    let _ = write_anchor(sub, node_id, &a, &pre);
}

/// Écrit l'ancre physique d'un nœud (micros Unix) en conservant la colonne
/// comme énergie de référence et SANS figer la demi-vie (`H = 0`). Réservé aux
/// chemins de test et de ré-ancrage où la loi est connue de l'appelant.
pub fn persist_anchor(sub: &SubstrateStore, node_id: NodeId, micros: i64) {
    loop {
        let Ok((pre, _)) = capturer_ancre(sub, node_id) else {
            return;
        };
        let e_ref = sub
            .get_node_energy_f32(node_id)
            .ok()
            .flatten()
            .unwrap_or(0.0) as f64;
        let a = construire_ancre(e_ref, micros, micros, Duration::ZERO);
        match write_anchor(sub, node_id, &a, &pre) {
            Ok((IssuePublication::PublieIntegrale, _)) => return,
            Ok((IssuePublication::ConflitPrecondition, _)) => continue,
            Ok((IssuePublication::PubliePartielle, _)) | Err(_) => return,
        }
    }
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
/// publie l'ancre composite SOUS l'autorité conditionnelle (Strict), puis
/// reflète la colonne miroir. Rend la valeur écrite.
///
/// Ordre : capturer → calculer sur la valeur CAPTURÉE (avec son estampille HLC)
/// → préparer → publier. Sur `ConflitPrecondition`, re-capture et RECALCULE
/// `e_courante` depuis la nouvelle ancre — JAMAIS de re-publication aveugle du
/// delta additif (absence de double renforcement).
pub fn reinforce(
    sub: &SubstrateStore,
    node_id: NodeId,
    delta: f64,
    law: &DecayLaw,
    max_energy: f64,
    now_micros: i64,
) -> Option<f64> {
    loop {
        let (pre, capture) = capturer_ancre(sub, node_id).ok()?;
        // Temps logique de la mutation : τ = max(now, t_hi, t_ref) lu depuis
        // l'ancre CAPTURÉE (valeur + estampille HLC). L'autorité temporelle ne
        // dépend ni du chemin d'accès ni du session_hi volatil du wrapper.
        let (tau, h) = match &capture {
            AncreCapturee::Valid(a) => (
                now_micros.max(a.t_hi_micros).max(a.t_ref_micros),
                if a.half_life.is_zero() {
                    law.half_life()
                } else {
                    a.half_life
                },
            ),
            // Absence légitime : migration (ancrage à now).
            AncreCapturee::Absente => (now_micros, law.half_life()),
            // Corruption : PAS de restauration implicite. Le renforcement ne peut
            // pas réparer une ancre temporellement invalide.
            AncreCapturee::Corrompue => return None,
        };
        // L'énergie est calculée au MÊME temps logique τ auquel l'ancre est
        // ancrée, DEPUIS la valeur capturée : jamais de décroissance entre le
        // calcul et l'ancrage, jamais une seconde lecture indépendante.
        let e_courante = match e_courante_depuis_capture(sub, node_id, &capture, law, tau) {
            Some(e) => e,
            None => return None,
        };
        let plafond = max_energy.min(1.0);
        let nouvelle = (e_courante + delta).clamp(0.0, plafond);

        let a = construire_ancre(nouvelle, tau, tau, h);
        match write_anchor(sub, node_id, &a, &pre) {
            Ok((IssuePublication::PublieIntegrale, (auteur, horloge))) => {
                // Point d'injection (test de durabilité) : un SIGKILL ICI laisse
                // l'ancre publiée mais pas le miroir — la lecture expose alors
                // 0,5 ou 0,75, jamais une ancre déchirée (0,375 / 1,0).
                obrain_common::utils::panne::point_entre_ancre_et_miroir();
                // Miroir de colonne, best-effort : la lecture ne dépend pas de
                // lui. Écrit APRÈS l'ancre (autorité), jamais avant.
                // Miroir de colonne CONDITIONNEL : ecrit APRES l'ancre (autorite),
                // et seulement si notre ancre est ENCORE canonique. Un ecrivain
                // concurrent (B) peut avoir publie une ancre plus recente entre
                // notre publication et ce point : on n'ecrit alors PAS un miroir
                // obsolete (la reconciliation a la prochaine tique le derive de
                // l'ancre canonique).
                let _ = sub.set_node_energy_f32_si_propriete_intacte(
                    node_id,
                    nouvelle as f32,
                    PROP_ENERGY_ANCHOR,
                    Some((auteur, horloge)),
                );
                return Some(nouvelle);
            }
            // Conflit : une ancre PLUS RÉCENTE existe. Re-capturer et RECALCULER
            // depuis elle — surtout ne PAS re-publier `delta` aveuglément (le
            // chemin boost est le plus exposé au double renforcement).
            Ok((IssuePublication::ConflitPrecondition, _)) => continue,
            Ok((IssuePublication::PubliePartielle, _)) | Err(_) => return None,
        }
    }
}

/// Énergie d'un nœud calculée DEPUIS l'ancre CAPTURÉE (pas une relecture).
/// `Absente` : colonne héritée (migration) — E_ref = colonne, t_ref = ancre
/// héritée.
fn e_courante_depuis_capture(
    sub: &SubstrateStore,
    node_id: NodeId,
    capture: &AncreCapturee,
    law: &DecayLaw,
    tau: i64,
) -> Option<f64> {
    match capture {
        AncreCapturee::Valid(a) => Some(if a.half_life.is_zero() {
            law.energy_at(
                a.e_ref,
                elapsed_monotonic(a.t_ref_micros, tau, a.t_hi_micros),
            )
        } else {
            a.energy_at(tau)
        }),
        AncreCapturee::Corrompue => None,
        AncreCapturee::Absente => {
            // Donnée héritée (absence légitime) : colonne = E_ref.
            let e_ref = sub.get_node_energy_f32(node_id).ok().flatten()? as f64;
            if e_ref <= 0.0 {
                return Some(0.0);
            }
            match legacy_anchor(sub, node_id) {
                Some(t_ref) => Some(law.energy_at(e_ref, elapsed_since(t_ref, tau))),
                None => Some(e_ref),
            }
        }
    }
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
    /// Miroirs (projections Q1.15) réconciliés SANS matérialisation canonique :
    /// l'ancre n'a pas changé, seule la projection obsolète a été réparée.
    pub reconciled: u64,
}

/// Avance jusqu'à `max_nodes` nœuds le long de LEUR propre loi, depuis LEUR
/// propre ancre, jusqu'à `now`. `max_nodes == 0` = pas de borne.
///
/// Les nœuds sans ancre (ancienne base) sont ANCRÉS à `now` — migration
/// explicite, dans le chemin d'écriture — au lieu d'être remis à zéro
/// silencieusement à la lecture. Toute écriture d'ancre passe par l'autorité
/// conditionnelle native (Strict) ; un conflit (ancre changée depuis la capture)
/// reporte le nœud à la tique suivante (décroissance re-capturable, jamais un
/// delta additif à re-publier).
pub fn advance_all(
    sub: &SubstrateStore,
    law: &DecayLaw,
    max_nodes: usize,
    now_micros: i64,
) -> SubstrateResult<DecayAdvance> {
    let mut stats = DecayAdvance::default();
    let slots = sub.iter_live_node_energies()?;
    stats.scanned = slots.len() as u64;

    let ids: Vec<NodeId> = slots.iter().map(|(id, _)| *id).collect();
    let ancres = sub.get_node_property_batch(&ids, &cle_ancre());

    // Assemble les nœuds vivants avec leur clé de vieillesse `(t_hi, t_ref)`.
    // Les dormants (colonne ≤ 0) et les ancres corrompues sont comptés et
    // écartés du tri. Le calcul et l'écriture, eux, re-capturent l'ancre sous
    // l'autorité conditionnelle au moment de la mutation.
    let mut candidats: Vec<((i64, i64, NodeId), f64)> = Vec::new();
    for ((node_id, e_ref_col), ancre) in slots.into_iter().zip(ancres) {
        let e_ref_col = e_ref_col as f64;
        let ancre_etat = decode_anchor_state(&ancre);
        if matches!(ancre_etat, AnchorState::Corrupted) {
            // Corruption : PAS de migration, PAS de décroissance. L'énergie
            // canonique (colonne) est préservée, le nœud est signalé.
            stats.corrupted += 1;
            continue;
        }
        if matches!(ancre_etat, AnchorState::Absent) && e_ref_col <= 0.0 {
            // Dormance EXPLICITE : pas d'ancre ET colonne nulle (politique de
            // migration), pas un effet accidentel de la précision du miroir.
            stats.dormant += 1;
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
        candidats.push(((t_hi, t_ref, node_id), e_ref_col));
    }

    // Tri par ancienneté : `t_hi` asc (le nœud le moins avancé d'abord), puis
    // `t_ref` asc, puis `node_id` — déterministe.
    candidats.sort_by_key(|(cle, _)| *cle);

    for ((_t_hi, _t_ref, node_id), _e_ref_col) in candidats {
        if max_nodes != 0 && stats.advanced as usize >= max_nodes {
            stats.deferred += 1;
            continue;
        }
        avancer_un_noeud(sub, node_id, law, now_micros, &mut stats)?;
    }
    Ok(stats)
}

/// Avance UN nœud sous l'autorité conditionnelle. Mutations d'ancre en `Strict` :
/// capture → calcul → préparation → publication. Un conflit re-capture et
/// recalcule (décroissance re-capturable) — jamais de re-publication additive.
fn avancer_un_noeud(
    sub: &SubstrateStore,
    node_id: NodeId,
    law: &DecayLaw,
    now_micros: i64,
    stats: &mut DecayAdvance,
) -> SubstrateResult<()> {
    loop {
        let (pre, capture) = capturer_ancre(sub, node_id)?;
        let estampille_ancre_capturee = pre
            .estampille_capturee(PROP_ENERGY_ANCHOR)
            .map(|e| (e.auteur, e.horloge));
        match capture {
            AncreCapturee::Corrompue => {
                stats.corrupted += 1;
                return Ok(());
            }
            AncreCapturee::Absente => {
                let e_ref_col = sub.get_node_energy_f32(node_id)?.unwrap_or(0.0) as f64;
                if e_ref_col <= 0.0 {
                    stats.dormant += 1;
                    return Ok(());
                }
                match legacy_anchor(sub, node_id) {
                    // Hérité (propriété f64 séparée) : E_ref = colonne, t_ref
                    // hérité, H non figée — on décroît depuis l'ancre héritée.
                    Some(t_ref) => {
                        let now_eff = now_micros.max(t_ref);
                        let dt = elapsed_monotonic(t_ref, now_eff, t_ref);
                        let nouvelle = law.energy_at(e_ref_col, dt);
                        let changement_canonique = (nouvelle as f32) != (e_ref_col as f32);
                        let miroir_attendu = obrain_substrate::record::q1_15_to_f32(
                            obrain_substrate::record::f32_to_q1_15(nouvelle as f32),
                        );
                        let miroir_stocke = match sub.get_node_energy_f32(node_id) {
                            Ok(Some(v)) => Some(v),
                            Ok(None) | Err(_) => None,
                        };
                        let miroir_obsolete =
                            miroir_stocke.is_some() && miroir_stocke != Some(miroir_attendu);
                        if !changement_canonique && !miroir_obsolete {
                            return Ok(());
                        }
                        // Estampille attendue pour la garde du miroir : ancre
                        // ABSENTE ici (None), sauf si l'on vient d'en publier une.
                        let mut estampille_attendu = estampille_ancre_capturee;
                        if changement_canonique {
                            let a = construire_ancre(nouvelle, now_eff, t_ref, law.half_life());
                            match write_anchor(sub, node_id, &a, &pre)? {
                                (IssuePublication::PublieIntegrale, stamp) => {
                                    stats.advanced += 1;
                                    estampille_attendu = Some(stamp);
                                }
                                (IssuePublication::ConflitPrecondition, _) => continue,
                                (IssuePublication::PubliePartielle, _) => {}
                            }
                        }
                        if miroir_obsolete {
                            match sub.set_node_energy_f32_si_propriete_intacte(
                                node_id,
                                nouvelle as f32,
                                PROP_ENERGY_ANCHOR,
                                estampille_attendu,
                            ) {
                                Ok(true) => stats.reconciled += 1,
                                Ok(false) | Err(_) => {}
                            }
                        }
                        return Ok(());
                    }
                    None => {
                        // Aucune ancre : ancrage de migration à now (point 4).
                        let a = construire_ancre(e_ref_col, now_micros, now_micros, law.half_life());
                        match write_anchor(sub, node_id, &a, &pre)? {
                            (IssuePublication::PublieIntegrale, _) => {
                                stats.unanchored += 1;
                                return Ok(());
                            }
                            (IssuePublication::ConflitPrecondition, _) => continue,
                            (IssuePublication::PubliePartielle, _) => return Ok(()),
                        }
                    }
                }
            }
            AncreCapturee::Valid(a) => {
                let h = if a.half_life.is_zero() {
                    law.half_life()
                } else {
                    a.half_life
                };
                // Le calcul et l'ancrage utilisent le MÊME temps logique (high-water).
                let now_eff = now_micros.max(a.t_hi_micros);
                let dt = elapsed_monotonic(a.t_ref_micros, now_eff, a.t_hi_micros);
                let nouvelle = DecayLaw::new(h).energy_at(a.e_ref, dt);
                // Deux décisions SÉPARÉES : (1) matérialisation canonique ; (2)
                // réconciliation du miroir.
                let changement_canonique = (nouvelle as f32) != (a.e_ref as f32);
                let miroir_attendu = obrain_substrate::record::q1_15_to_f32(
                    obrain_substrate::record::f32_to_q1_15(nouvelle as f32),
                );
                // Une erreur de LECTURE du miroir n'est pas une divergence.
                let miroir_stocke = match sub.get_node_energy_f32(node_id) {
                    Ok(Some(v)) => Some(v),
                    Ok(None) | Err(_) => None,
                };
                let miroir_obsolete =
                    miroir_stocke.is_some() && miroir_stocke != Some(miroir_attendu);
                if !changement_canonique && !miroir_obsolete {
                    return Ok(());
                }
                // Estampille attendue pour la garde du miroir : celle de l'ancre
                // CAPTUREE (Some), sauf si l'on vient d'en publier une nouvelle.
                let mut estampille_attendu = estampille_ancre_capturee;
                if changement_canonique {
                    let a_nouvelle = construire_ancre(nouvelle, now_eff, a.t_hi_micros, h);
                    match write_anchor(sub, node_id, &a_nouvelle, &pre)? {
                        (IssuePublication::PublieIntegrale, stamp) => {
                            stats.advanced += 1;
                            estampille_attendu = Some(stamp);
                        }
                        (IssuePublication::ConflitPrecondition, _) => continue,
                        (IssuePublication::PubliePartielle, _) => {}
                    }
                }
                if miroir_obsolete {
                    match sub.set_node_energy_f32_si_propriete_intacte(
                        node_id,
                        nouvelle as f32,
                        PROP_ENERGY_ANCHOR,
                        estampille_attendu,
                    ) {
                        Ok(true) => stats.reconciled += 1,
                        Ok(false) | Err(_) => {}
                    }
                }
                return Ok(());
            }
        }
    }
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
        // e_ref == 0 est une décroissance complète VALIDE (51bd7e5) : seuls
        // négatif / non fini sont refusés.
        for e in [-1.0, f64::NAN, f64::INFINITY] {
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


#[cfg(test)]
mod tests_encodeur {
    use super::*;
    use std::sync::Arc;

    fn rt(e: f64) -> Option<f64> {
        let a = EnergyAnchor { e_ref: e, t_ref_micros: 1_000_000, t_hi_micros: 1_000_000, half_life: Duration::from_secs(64) };
        decode_anchor(&encode_anchor(&a)).map(|a| a.e_ref)
    }

    #[test]
    fn round_trip_accepte_les_energies_legitimes() {
        for e in [1.0, 0.5, 2f64.powi(-16), 2f64.powi(-160), f64::MIN_POSITIVE, 0.0] {
            let d = rt(e);
            assert!(d.is_some(), "E={e} doit décoder (état valide), obtenu None");
        }
    }

    #[test]
    fn round_trip_rejette_les_aberrants() {
        for e in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let a = EnergyAnchor { e_ref: e, t_ref_micros: 1_000_000, t_hi_micros: 1_000_000, half_life: Duration::from_secs(64) };
            assert!(decode_anchor(&encode_anchor(&a)).is_none(), "E={e} doit être rejeté");
        }
    }

    #[test]
    fn une_decroissance_complete_n_est_pas_une_corruption() {
        // E=1, dt=86400 s, H=64 s -> 2^-1350 -> 0 en f64. L'ancre doit rester
        // VALIDE (e_ref=0), pas Corrupted.
        let e = 1.0f64 * 2f64.powf(-86400.0 / 64.0);
        assert_eq!(e, 0.0, "2^-1350 sous-dépasse en f64");
        assert_eq!(rt(e), Some(0.0), "une énergie nulle est un état valide");
    }
}
