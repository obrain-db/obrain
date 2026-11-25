//! Contrat transactionnel d'identité d'observation (RFC d59af717).
//!
//! # Le défaut qu'il ferme
//!
//! `EngramManager::revise(engram_id, new_nodes, prediction_error)` n'a pas
//! d'identité : après un échec partiel (set_engram_state réussi, add_engram_bit
//! échoué), la reprise applique la reconsolidation UNE SECONDE FOIS. Le test
//! `injection_erreur_partielle_set_then_bit` le documente (« l'API n'a pas
//! d'identité d'observation »).
//!
//! # Le contrat — trois exigences
//!
//! 1. **Identité stable, distincte des tentatives.** Une observation porte un
//!    identifiant (jeton fourni par l'appelant). Réessayer la même observation
//!    (même identifiant) est un REJEU, pas une nouvelle observation.
//! 2. **Rejeu vs conflit.** Même identifiant + même contenu → rejeu (idempotent).
//!    Même identifiant + contenu DIFFÉRENT → conflit (refus nommé). Deux
//!    identifiants différents + contenu identique → PAS un conflit.
//! 3. **Deux observations indépendantes de contenu identique sont légitimes.**
//!    L'identité n'est PAS dérivée du contenu (pas de déduplication par
//!    empreinte de contenu) : deux observations distinctes du même contenu
//!    s'appliquent toutes deux.
//!
//! # La frontière substrat (coordination — ne PAS implémenter ici)
//!
//! Le trait [`ObservationLedger`] est l'interface de la primitive ATOMIQUE que
//! le substrat doit fournir : appliquer la transformation ET enregistrer
//! l'identité dans une MÊME transaction WAL. Deux pièges, signalés par le
//! pilotage, sont explicitement hors contrat :
//!
//! * une **réservation d'identité suivie d'une transformation indépendante ne
//!   suffit pas** — un crash entre les deux laisse l'identité réservée sans
//!   effet, et le rejeu serait refusé à tort (c'est l'état `Indeterminate` que
//!   l'implémentation de référence sait produire) ;
//! * la **convergence LWW d'une propriété n'est pas la fusion d'observations
//!   cognitives indépendantes** — l'enregistrement identité+transformation est
//!   une transaction, pas une propriété écrasable.
//!
//! Ce module est le MODÈLE PUR : types + machine à états + implémentation de
//! référence en mémoire. Il ne touche pas aux primitives transactionnelles du
//! substrat.

use std::collections::HashMap;
use std::sync::Mutex;

use obrain_common::types::NodeId;

use super::types::EngramId;

/// Identifiant stable d'une observation, distinct des tentatives.
///
/// CONTENU-INDÉPENDANT : deux observations de contenu identique portent des
/// identifiants DIFFÉRENTS (exigence 3). Opaque, 128 bits (UUID-like).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObservationId(pub [u8; 16]);

impl ObservationId {
    /// Construit un identifiant depuis un u128 (par ex. un UUID encodé).
    pub fn from_u128(v: u128) -> Self {
        Self(v.to_be_bytes())
    }

    /// La valeur u128 sous-jacente.
    pub fn as_u128(self) -> u128 {
        u128::from_be_bytes(self.0)
    }
}

impl std::fmt::Display for ObservationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "obs:{:032x}", self.as_u128())
    }
}

/// Empreinte du CONTENU d'une observation (jamais de l'identité).
///
/// Deux tentatives du même contenu produisent la même empreinte ; c'est elle
/// qui départage le rejeu (même empreinte) du conflit (empreinte différente).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObservationFingerprint(pub u128);

/// Contenu canonique d'une observation : ce qui la DÉFINIT, indépendamment de
/// l'identité. `new_nodes` est pris dans l'ordre fourni (le rejeu repasse le
/// même ordre ; l'appelant est responsable de la canonisation s'il la veut).
/// Empreinte FNV-1a 128 bits — déterministe et stable, pas de dépendance.
pub fn observation_fingerprint(
    engram_id: EngramId,
    new_nodes: &[NodeId],
    prediction_error: f64,
) -> ObservationFingerprint {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0001_0000_0000_0000_0000_013b; // 2^88 + 0x13B
    let mut h: u128 = OFFSET;
    let mut feed = |bytes: &[u8]| {
        for &b in bytes {
            h ^= b as u128;
            h = h.wrapping_mul(PRIME);
        }
    };
    feed(&engram_id.0.to_be_bytes());
    feed(&(new_nodes.len() as u64).to_be_bytes());
    for n in new_nodes {
        feed(&n.0.to_be_bytes());
    }
    feed(&prediction_error.to_bits().to_be_bytes());
    ObservationFingerprint(h)
}

/// Issue d'une tentative d'application d'une observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationOutcome {
    /// Première application : transformation appliquée, identité enregistrée.
    Applied,
    /// Même identifiant, même contenu : rejeu idempotent, rien de réappliqué.
    Replayed,
    /// Même identifiant, contenu DIFFÉRENT : conflit nommé, jamais appliqué.
    Conflict,
    /// L'observation a été explicitement abandonnée (tombe durable) : ne pas
    /// rejouer, l'effet n'a pas eu lieu et n'aura pas lieu.
    Abandoned,
    /// État du registre indéterminé (écriture partielle) : impossible de
    /// départager rejeu/conflit/absence. NE PAS rejouer en aveugle.
    Indeterminate,
}

/// Primitive ATOMIQUE que le substrat doit fournir (chantier « réservation
/// générique » du responsable substrat).
///
/// Applique la transformation (l'état candidat COMPLET `members`, déjà
/// reconsolidé par l'appelant) ET enregistre l'identité dans une MÊME
/// transaction WAL — check-and-set sur l'identité, jamais une réservation
/// suivie d'une transformation indépendante.
///
/// Ce trait est la FRONTIÈRE : ce module ne l'implémente que pour la référence
/// en mémoire ([`InMemoryObservationLedger`]), jamais pour la durabilité.
pub trait ObservationLedger: Send + Sync {
    /// Check-and-set atomique. Si l'identité est absente, applique et
    /// enregistre (Applied). Sinon compare l'empreinte : rejeu / conflit /
    /// abandon / indéterminé. `members` est l'état candidat COMPLET (déjà
    /// reconsolidé par l'appelant), pas une transformation à rejouer.
    fn apply_observation(
        &self,
        observation_id: ObservationId,
        fingerprint: ObservationFingerprint,
        engram_id: EngramId,
        members: &[(NodeId, f64)],
    ) -> Result<ObservationOutcome, String>;
}

/// Entrée du registre d'observations de référence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LedgerEntry {
    /// Appliquée : l'empreinte du contenu est conservée pour départager le rejeu
    /// du conflit.
    Applied { fingerprint: ObservationFingerprint },
    /// Explicitement abandonnée (tombe).
    Abandoned,
    /// Réservée mais non remplie : écriture partielle (indéterminée). C'est
    /// l'état que laisserait « réserver l'identité puis transformer
    /// indépendamment » après un crash entre les deux.
    Claimed,
}

/// Implémentation de RÉFÉRENCE du registre d'observations, en mémoire.
///
/// Sert de spécification exécutable du contrat pour le responsable substrat ET
/// de mock pour les tests. N'est PAS durable : ce n'est pas une implémentation
/// de production (la durabilité est la primitive substrat, pas ce module).
pub struct InMemoryObservationLedger {
    entries: Mutex<HashMap<ObservationId, LedgerEntry>>,
    /// Injection de faute (test) : la PROCHAINE application réserve l'identité
    /// puis « crashe » avant de la remplir, laissant un état `Claimed`.
    fail_partial_write: std::sync::atomic::AtomicBool,
}

impl Default for InMemoryObservationLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryObservationLedger {
    /// Registre de référence vide.
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            fail_partial_write: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Marque une observation comme explicitement abandonnée : toute tentative
    /// ultérieure (quel que soit le contenu) rend `Abandoned`.
    pub fn abandon(&self, observation_id: ObservationId) {
        self.entries
            .lock()
            .unwrap()
            .insert(observation_id, LedgerEntry::Abandoned);
    }

    /// Injection de faute (test) : la prochaine application réserve puis
    /// « crashe » (retourne une erreur) en laissant un `Claimed` — reproduit
    /// exactement « réservation d'identité suivie d'une transformation
    /// indépendante » interrompue.
    pub fn set_fail_partial_write(&self, fail: bool) {
        self.fail_partial_write
            .store(fail, std::sync::atomic::Ordering::Relaxed);
    }
}

impl ObservationLedger for InMemoryObservationLedger {
    fn apply_observation(
        &self,
        observation_id: ObservationId,
        fingerprint: ObservationFingerprint,
        _engram_id: EngramId,
        _members: &[(NodeId, f64)],
    ) -> Result<ObservationOutcome, String> {
        let mut entries = self.entries.lock().unwrap();
        // Écriture partielle simulée : réserve (Claimed) puis échoue, SANS
        // appliquer ni remplir. Le rejeu découvrira l'état Indeterminate.
        if self
            .fail_partial_write
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            entries.insert(observation_id, LedgerEntry::Claimed);
            return Err("crash simulé après réservation, avant remplissage".into());
        }
        match entries.get(&observation_id) {
            None => {
                entries.insert(
                    observation_id,
                    LedgerEntry::Applied { fingerprint },
                );
                Ok(ObservationOutcome::Applied)
            }
            Some(LedgerEntry::Applied { fingerprint: recorded })
                if *recorded == fingerprint =>
            {
                Ok(ObservationOutcome::Replayed)
            }
            Some(LedgerEntry::Applied { .. }) => Ok(ObservationOutcome::Conflict),
            Some(LedgerEntry::Abandoned) => Ok(ObservationOutcome::Abandoned),
            Some(LedgerEntry::Claimed) => Ok(ObservationOutcome::Indeterminate),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(n: u128) -> ObservationId {
        ObservationId::from_u128(n)
    }

    #[test]
    fn premiere_application_est_applied() {
        let ledger = InMemoryObservationLedger::new();
        let id = obs(1);
        let fp = observation_fingerprint(EngramId(10), &[NodeId(1)], 0.5);
        let out = ledger
            .apply_observation(id, fp, EngramId(10), &[(NodeId(1), 0.5)])
            .unwrap();
        assert_eq!(out, ObservationOutcome::Applied);
    }

    #[test]
    fn rejeu_meme_id_meme_contenu_est_idempotent() {
        let ledger = InMemoryObservationLedger::new();
        let id = obs(1);
        let fp = observation_fingerprint(EngramId(10), &[NodeId(1)], 0.5);
        let first = ledger
            .apply_observation(id, fp, EngramId(10), &[(NodeId(1), 0.5)])
            .unwrap();
        assert_eq!(first, ObservationOutcome::Applied);
        let replay = ledger
            .apply_observation(id, fp, EngramId(10), &[(NodeId(1), 0.5)])
            .unwrap();
        assert_eq!(replay, ObservationOutcome::Replayed, "le rejeu n'est pas réappliqué");
    }

    #[test]
    fn meme_id_contenu_different_est_conflit() {
        let ledger = InMemoryObservationLedger::new();
        let id = obs(1);
        let fp_a = observation_fingerprint(EngramId(10), &[NodeId(1)], 0.5);
        let fp_b = observation_fingerprint(EngramId(10), &[NodeId(2)], 0.5);
        assert_eq!(
            ledger.apply_observation(id, fp_a, EngramId(10), &[(NodeId(1), 0.5)]).unwrap(),
            ObservationOutcome::Applied
        );
        let conflict = ledger
            .apply_observation(id, fp_b, EngramId(10), &[(NodeId(2), 0.5)])
            .unwrap();
        assert_eq!(conflict, ObservationOutcome::Conflict);
    }

    #[test]
    fn deux_observations_identiques_de_contenu_identique_sont_legitimes() {
        let ledger = InMemoryObservationLedger::new();
        // Contenu IDENTIQUE, identifiants DIFFÉRENTS : pas de déduplication.
        let fp = observation_fingerprint(EngramId(10), &[NodeId(7)], 0.5);
        let a = ledger
            .apply_observation(obs(1), fp, EngramId(10), &[(NodeId(7), 0.5)])
            .unwrap();
        let b = ledger
            .apply_observation(obs(2), fp, EngramId(10), &[(NodeId(7), 0.5)])
            .unwrap();
        assert_eq!(a, ObservationOutcome::Applied);
        assert_eq!(b, ObservationOutcome::Applied, "deux observations distinctes de contenu identique");
    }

    #[test]
    fn abandon_est_stable() {
        let ledger = InMemoryObservationLedger::new();
        let id = obs(9);
        ledger.abandon(id);
        let fp = observation_fingerprint(EngramId(10), &[NodeId(1)], 0.5);
        assert_eq!(
            ledger.apply_observation(id, fp, EngramId(10), &[(NodeId(1), 0.5)]).unwrap(),
            ObservationOutcome::Abandoned
        );
        assert_eq!(
            ledger.apply_observation(id, fp, EngramId(10), &[(NodeId(1), 0.5)]).unwrap(),
            ObservationOutcome::Abandoned,
            "l'abandon est durable et idempotent"
        );
    }

    #[test]
    fn ecriture_partielle_est_indeterminee_pas_rejouee() {
        let ledger = InMemoryObservationLedger::new();
        let id = obs(1);
        let fp = observation_fingerprint(EngramId(10), &[NodeId(1)], 0.5);

        // Réservation puis « crash » avant remplissage.
        ledger.set_fail_partial_write(true);
        let crash = ledger.apply_observation(id, fp, EngramId(10), &[(NodeId(1), 0.5)]);
        assert!(crash.is_err(), "le crash simulé rend une erreur, pas un outcome");

        // Le rejeu découvre l'état INDÉTERMINÉ : ni Applied ni Replayed ni
        // Conflict — on refuse de deviner.
        let replay = ledger
            .apply_observation(id, fp, EngramId(10), &[(NodeId(1), 0.5)])
            .unwrap();
        assert_eq!(replay, ObservationOutcome::Indeterminate);
    }

    #[test]
    fn empreinte_depend_du_contenu_pas_de_l_identite() {
        let fp1 = observation_fingerprint(EngramId(10), &[NodeId(1)], 0.5);
        let fp2 = observation_fingerprint(EngramId(10), &[NodeId(1)], 0.6);
        let fp3 = observation_fingerprint(EngramId(10), &[NodeId(2)], 0.5);
        let fp4 = observation_fingerprint(EngramId(11), &[NodeId(1)], 0.5);
        assert_ne!(fp1, fp2, "erreur de prédiction différente");
        assert_ne!(fp1, fp3, "nœud différent");
        assert_ne!(fp1, fp4, "engramme cible différent");
    }
}
