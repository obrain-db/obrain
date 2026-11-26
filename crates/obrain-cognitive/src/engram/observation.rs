//! Contrat transactionnel d'identité d'observation (RFC d59af717, corrigé A-G).
//!
//! # Le défaut qu'il ferme
//!
//! `EngramManager::revise(engram_id, new_nodes, prediction_error)` n'a pas
//! d'identité NI de précondition de révision : après un échec partiel, la
//! reprise applique la reconsolidation DEUX fois ; et deux observations
//! calculées depuis la MÊME révision s'écrasent l'une l'autre (mise à jour
//! perdue). Le test `injection_erreur_partielle_set_then_bit` documente le
//! premier défaut (« l'API n'a pas d'identité d'observation »).
//!
//! # Le contrat — quatre exigences (A, B, C, E)
//!
//! 1. **Identité logique stable, distincte des tentatives** (identifiant opaque
//!    fourni par l'appelant). Un rejeu technique (même identifiant) applique la
//!    transformation AU PLUS UNE FOIS.
//! 2. **Précondition de révision atomique** (point A). La TENTATIVE porte la
//!    révision attendue de la connaissance ; `apply` vérifie identité ET
//!    révision sous la même unité atomique. Deux observations calculées depuis
//!    la même révision ne peuvent PAS toutes deux s'appliquer : la seconde
//!    reçoit un CONFLIT DE RÉVISION (à recalculer depuis l'état courant, SANS
//!    inventer une nouvelle identité). Le conflit d'identité (même id, contenu
//!    ≠) et le conflit de révision sont DISTINCTS.
//! 3. **Deux observations indépendantes de contenu identique = deux
//!    applications réelles** (pas deux entrées Applied vides). L'identité n'est
//!    PAS dérivée du contenu.
//! 4. **État canonique complet conservé atomiquement avec l'identité**
//!    (points B, E) : les champs PERSISTANTS modifiés par la transformation
//!    (`ensemble`, `strength`, `recall_count`) sont enregistrés avec l'identité
//!    dans la même unité. Les PROJECTIONS DÉRIVÉES (`spectral_signature`) n'en
//!    font pas partie. Si le substrat ne conserve qu'une partie de l'état, la
//!    promesse d'atomicité ne porte QUE sur cette partie (voir plus bas).
//!
//! # L'empreinte (point D)
//!
//! [`ObservationDefinition`] est la DÉFINITION CANONIQUE ET VERSIONNÉE du
//! contenu — c'est elle qui fait preuve d'égalité (comparaison d'octets
//! exacts). [`ObservationDefinition::fingerprint`] n'est qu'un ACCÉLÉRATEUR de
//! recherche : une collision d'empreinte ne vaut jamais égalité. La
//! canonisation fixe la sémantique : ordre des membres (trié), doublons
//! (retirés), non-finis (refusés), représentation numérique (bits exacts).
//!
//! # La frontière (point F)
//!
//! Le substrat fournit une primitive **GÉNÉRIQUE** ([`AtomicEventLedger`],
//! octets opaques : identité d'événement, définition, précondition de révision,
//! état résultant, révision résultante) qui ne dépend PAS du crate cognitif.
//! La couche cognitive fournit l'**[`CognitiveObservationAdapter`]** qui
//! sérialise ses concepts et interprète les issues. Aucune réservation suivie
//! d'une mutation indépendante ; aucun registre en mémoire présenté comme
//! durable (l'implémentation de référence [`InMemoryAtomicEventLedger`] n'est
//! PAS une implémentation de production).
//!
//! # Ce que l'atomicité couvre RÉELLEMENT (point E)
//!
//! Champs modifiés par `reconsolidate`/`revise` :
//! * `ensemble` (poids décru + nouveaux nœuds) — PERSISTANT, miroir substrat actuel ;
//! * `strength`, `recall_count` — PERSISTANTS côté cognitif, PAS encore mirés
//!   au substrat (le miroir actuel n'écrit que membres+poids) ;
//! * `spectral_signature` — PROJECTION DÉRIVÉE (recalculable), hors état atomique.
//!
//! L'atomicité identité+état ne peut donc promettre que la partie effectivement
//! conservée par la primitive durable. Le jour où le substrat conserve
//! `strength`/`recall_count`, [`ResultingState`] les transporte déjà.

use std::collections::HashMap;
use std::sync::Mutex;

use obrain_common::types::NodeId;

use super::types::EngramId;

/// Version de la représentation canonique : l'incrémenter invalide les
/// définitions déjà enregistrées (une empreinte d'une autre version ne vaut
/// pas comparaison).
pub const OBSERVATION_DEFINITION_VERSION: u32 = 2;

/// Identité LOGIQUE d'une observation (l'événement), distincte des tentatives.
///
/// CONTENU-INDÉPENDANT : deux observations de contenu identique portent des
/// identifiants DIFFÉRENTS (deux expériences distinctes, exigence 3).
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

/// Définition CANONIQUE et VERSIONNÉE du contenu d'une observation.
///
/// C'est la preuve d'égalité (comparaison d'octets exacts), pas l'empreinte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationDefinition {
    /// Version de la représentation canonique.
    version: u32,
    /// Nœuds de l'observation, CANONISÉS : triés par id, doublons retirés.
    ///
    /// C'est le CONTENU de l'observation, INDÉPENDANT de la transformation
    /// sélectionnée (révision d'un engramme ou formation) : la cible appartient
    /// à la transformation, pas à l'observation.
    new_nodes: Vec<NodeId>,
    /// Les bits EXACTS du f64 de l'erreur de prédiction (comparaison exacte).
    prediction_error_bits: u64,
}

impl ObservationDefinition {
    /// Construit une définition canonique du CONTENU de l'observation.
    /// Refuse une erreur non finie (NaN, infini).
    pub fn new(new_nodes: &[NodeId], prediction_error: f64) -> Result<Self, String> {
        if !prediction_error.is_finite() {
            return Err(format!(
                "erreur de prédiction non finie : {prediction_error}"
            ));
        }
        let mut nodes: Vec<NodeId> = new_nodes.to_vec();
        nodes.sort_unstable_by_key(|n| n.0);
        nodes.dedup();
        Ok(Self {
            version: OBSERVATION_DEFINITION_VERSION,
            new_nodes: nodes,
            prediction_error_bits: prediction_error.to_bits(),
        })
    }

    /// La version de la représentation.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Les nœuds CANONISÉS (triés, dédupliqués). La transformation DOIT
    /// recevoir exactement ces nœuds.
    pub fn canonical_nodes(&self) -> &[NodeId] {
        &self.new_nodes
    }

    /// Sérialisation canonique (big-endian, déterministe).
    /// Format : version u32 | len u64 | (node u64)* | prediction_error_bits u64.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + 8 + 8 * self.new_nodes.len() + 8);
        out.extend_from_slice(&self.version.to_be_bytes());
        out.extend_from_slice(&(self.new_nodes.len() as u64).to_be_bytes());
        for n in &self.new_nodes {
            out.extend_from_slice(&n.0.to_be_bytes());
        }
        out.extend_from_slice(&self.prediction_error_bits.to_be_bytes());
        out
    }

    /// ACCÉLÉRATEUR de recherche (FNV-1a 64 bits) — JAMAIS une preuve
    /// d'égalité (comparer [`canonical_bytes`](Self::canonical_bytes)).
    pub fn fingerprint(&self) -> u64 {
        const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;
        let mut h = OFFSET;
        for b in self.canonical_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(PRIME);
        }
        h
    }
}

/// Révision attendue de la connaissance source. Appartient à la TENTATIVE :
/// elle ne modifie jamais l'identité logique de l'observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceRevision(pub u64);

/// État canonique PERSISTANT résultant de la transformation.
///
/// Champs modifiés par `reconsolidate` (point E). `spectral_signature` est une
/// projection dérivée : elle n'est PAS ici.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultingState {
    /// Ensemble pondéré (poids décru + nouveaux nœuds ajoutés).
    pub ensemble: Vec<(NodeId, f64)>,
    /// Force de consolidation après reconsolidation.
    pub strength: f64,
    /// Compteur de rappels après reconsolidation.
    pub recall_count: u32,
}

impl ResultingState {
    /// Sérialisation canonique : n u32 | (node u64, weight_bits u64)* |
    /// strength_bits u64 | recall_count u32.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + 16 * self.ensemble.len() + 12);
        out.extend_from_slice(&(self.ensemble.len() as u32).to_be_bytes());
        for (n, w) in &self.ensemble {
            out.extend_from_slice(&n.0.to_be_bytes());
            out.extend_from_slice(&w.to_bits().to_be_bytes());
        }
        out.extend_from_slice(&self.strength.to_bits().to_be_bytes());
        out.extend_from_slice(&self.recall_count.to_be_bytes());
        out
    }

    /// Décode une sérialisation produite par [`canonical_bytes`](Self::canonical_bytes).
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, String> {
        let mut cur = 0usize;
        let take = |cur: &mut usize, n: usize| -> Result<&[u8], String> {
            let end = cur.checked_add(n).ok_or("débordement")?;
            let slice = bytes.get(*cur..end).ok_or("tronqué")?;
            *cur = end;
            Ok(slice)
        };
        let n = u32::from_be_bytes(take(&mut cur, 4)?.try_into().unwrap()) as usize;
        let mut ensemble = Vec::with_capacity(n);
        for _ in 0..n {
            let node = u64::from_be_bytes(take(&mut cur, 8)?.try_into().unwrap());
            let wb = u64::from_be_bytes(take(&mut cur, 8)?.try_into().unwrap());
            ensemble.push((NodeId(node), f64::from_bits(wb)));
        }
        let strength = f64::from_bits(u64::from_be_bytes(take(&mut cur, 8)?.try_into().unwrap()));
        let recall_count = u32::from_be_bytes(take(&mut cur, 4)?.try_into().unwrap());
        Ok(Self {
            ensemble,
            strength,
            recall_count,
        })
    }
}

/// Issue d'une tentative, vue COGNITIVE.
#[derive(Debug, Clone, PartialEq)]
pub enum ObservationOutcome {
    /// Première application : état + identité enregistrés atomiquement.
    Applied {
        /// État canonique résultant, effectivement enregistré.
        state: ResultingState,
        /// Révision résultante enregistrée.
        revision: u64,
    },
    /// Même identifiant + même définition : rejeu, AUCUNE transformation
    /// supplémentaire. `state`/`revision` sont ceux ENREGISTRÉS à l'application
    /// (informationnels : ne JAMAIS les réappliquer par-dessus un état plus
    /// récent).
    Replayed {
        /// État ENREGISTRÉ à l'application (informationnel).
        state: ResultingState,
        /// Révision ENREGISTRÉE à l'application (informationnelle).
        revision: u64,
        /// Connaissance sur laquelle l'observation a EFFECTIVEMENT été appliquée.
        target: EngramId,
    },
    /// Même identifiant, DÉFINITION différente : conflit nommé.
    IdentityConflict {
        /// Définition EFFECTIVEMENT enregistrée pour cette identité.
        recorded: ObservationDefinition,
    },
    /// Révision attendue dépassée : recalculer depuis l'état COURANT, avec la
    /// MÊME identité logique (ne pas en inventer une nouvelle).
    RevisionConflict {
        /// Révision EFFECTIVEMENT courante de la connaissance.
        current_revision: u64,
    },
    /// L'effet a été ENREGISTRÉ au registre mais la matérialisation a trouvé la
    /// connaissance AVANCÉE (hors verrou). DISTINCT d'un conflit certain avant
    /// application : un rejeu réparera ; on ne présente pas un effet enregistré
    /// comme un refus sans effet.
    RecordedNotMaterialized {
        /// Révision EFFECTIVEMENT courante de la connaissance.
        current_revision: u64,
    },
    /// Identité enregistrée comme FORMATION (aucune connaissance visée) : un
    /// rejeu ne réactive pas, et une apparition ultérieure de connaissance ne
    /// transforme PAS rétroactivement cette observation en révision.
    Formed,
    /// Identité déjà enregistrée comme FORMATION, activation CONFIRMÉE.
    AlreadyFormed,
    /// Identité de FORMATION réservée mais activation NON confirmée (échec ou
    /// interruption entre la réservation et l'activation) : le rejeu doit
    /// RÉACTIVER, jamais déclarer l'apprentissage fait alors qu'il est absent.
    FormationPending,
    /// Identité explicitement abandonnée (jamais appliquée, ne le sera pas).
    Abandoned,
    /// État transactionnel indéterminé : APPELER [`CognitiveObservationAdapter::resolve`],
    /// ne pas rejouer en aveugle.
    Indeterminate,
}

// ===================================================================
// Frontière : primitive GÉNÉRIQUE fournie par le substrat (octets seuls)
// ===================================================================

/// Issue de la primitive générique.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerOutcome {
    /// Effet enregistré (identité + état).
    Applied,
    /// Identité déjà enregistrée : effet ENREGISTRÉ rendu tel quel.
    AlreadyApplied {
        /// Effet enregistré (préfixé : 0x01 révision, 0x02 formation).
        state: Vec<u8>,
        /// Révision enregistrée.
        revision: u64,
    },
    /// Même identité, définition différente.
    IdentityConflict {
        /// Définition enregistrée (octets canoniques).
        recorded_definition: Vec<u8>,
    },
    /// Précondition de révision non satisfaite.
    PreconditionFailed {
        /// Révision EFFECTIVEMENT courante de la ressource.
        current_revision: u64,
    },
    /// Identité abandonnée.
    Abandoned,
    /// État transactionnel indéterminé.
    Indeterminate,
}

/// Issue d'un abandon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbandonOutcome {
    /// Marquée abandonnée (elle n'était pas appliquée).
    Abandoned,
    /// REFUSÉ : déjà appliquée. Une appliquée reste appliquée ; l'annulation est
    /// une NOUVELLE transformation identifiable, pas un abandon rétroactif.
    AlreadyApplied {
        /// Révision de l'application existante (refus de l'abandon).
        revision: u64,
    },
}

/// État d'une identité après interrogation (résolution d'un Indeterminate).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedState {
    /// La transaction n'a PAS abouti : aucune application. Le rejeu peut
    /// appliquer (l'état courant n'a pas bougé).
    NotApplied,
    /// Appliquée : identité + état résultant sont présents.
    Applied {
        /// Définition enregistrée (octets canoniques).
        definition: Vec<u8>,
        /// Effet enregistré (préfixé : 0x01 révision, 0x02 formation).
        state: Vec<u8>,
        /// Révision enregistrée.
        revision: u64,
    },
    /// Identité COMMISE (APPLIQUE) mais effet NON matérialisé sur la ressource :
    /// la projection ressource (CLE_REVISION / CLE_EFFET) ne porte PAS l'effet
    /// enregistré — arrêt entre le commit d'identité et la matérialisation, ou
    /// effet devenu incompatible (masqué par une écriture ultérieure).
    /// RÉCUPÉRABLE : l'effet enregistré est restitué ici, un rejeu (via le
    /// manager) répare la matérialisation. JAMAIS confondu avec `Applied`
    /// (qui exige effet engagé ET matérialisé).
    CommittedNotMaterialized {
        /// Définition enregistrée (octets canoniques).
        definition: Vec<u8>,
        /// Effet enregistré (préfixé : 0x01 révision, 0x02 formation).
        state: Vec<u8>,
        /// Révision ENREGISTRÉE (celle que l'identité revendique).
        revision: u64,
    },
    /// Explicitement abandonnée.
    Abandoned,
}

/// Primitive ATOMIQUE GÉNÉRIQUE attendue du substrat (point F).
///
/// Le substrat ne connaît AUCUN type cognitif : tout est octets opaques.
///
/// ── CE QUI EST EXIGÉ (architectural), PAS COMMENT (implémentation) ─────────
///
/// Le besoin est : appliquer ATOMIQUEMENT un événement identifiable, avec une
/// PRÉCONDITION de révision, et CONSERVER son effet. Le substrat est LIBRE de
/// réutiliser ou d'enrichir GÉNÉRIQUEMENT ses mécanismes existants (WAL-first,
/// FLAG_COMMIT, mutation multi-zones sous un verrou). Aucune colonne physique
/// « observations appliquées » ni trame WAL particulière n'est exigée : ce sont
/// des OPTIONS d'implémentation, pas des exigences.
///
/// Cinq propriétés à vérifier SÉPARÉMENT (elles ne s'impliquent pas l'une
/// l'autre) :
/// 1. ATOMICITÉ — l'application (identité + effet) est tout-ou-rien.
/// 2. ISOLATION — des événements concurrents ne s'entrelacent pas partiellement.
/// 3. VISIBILITÉ — l'effet est visible après le retour `Applied`, et jamais
///    avant en cas d'échec.
/// 4. DURABILITÉ — après `Applied`, l'effet survit à un crash.
/// 5. RÉCUPÉRATION — après un crash, l'état est le préfixe COMMITÉ (aucun
///    événement déchiré) ; [`AtomicEventLedger::resolve`] tranche un état
///    indéterminé.
///
/// Une RÉSERVATION d'identité seule ne suffit PAS : réserver puis transformer
/// indépendamment laisse, après crash, une identité prise sans effet — état que
/// `resolve` doit QUALIFIER, jamais deviner.
pub trait AtomicEventLedger: Send + Sync {
    /// Révision COURANTE d'une ressource (la connaissance visée).
    fn resource_revision(&self, resource_id: &[u8]) -> Result<u64, String>;

    /// État COURANT d'une ressource tel qu'ENREGISTRÉ (dernier effet appliqué :
    /// octets de l'effet, révision). Sert à rattraper un store en retard sans
    /// perdre l'effet déjà accepté par le registre.
    fn resource_state(&self, resource_id: &[u8]) -> Result<Option<(Vec<u8>, u64)>, String>;

    /// Check-and-set atomique sur (identité, précondition de révision), puis
    /// application de l'état résultant — le tout dans une MÊME unité.
    fn apply_event(
        &self,
        event_id: &[u8; 16],
        resource_id: &[u8],
        definition: &[u8],
        precondition_revision: u64,
        resulting_state: &[u8],
        resulting_revision: u64,
    ) -> Result<LedgerOutcome, String>;

    /// Interroge l'état transactionnel réel d'une identité (résolution d'un
    /// Indeterminate : ni terminal arbitraire, ni autorisation de rejouer).
    fn resolve(&self, event_id: &[u8; 16]) -> Result<ResolvedState, String>;

    /// Abandon conditionnel (refusé si déjà appliquée).
    fn abandon(&self, event_id: &[u8; 16]) -> Result<AbandonOutcome, String>;

    /// Confirme qu'une FORMATION réservée a bien produit son effet (passe
    /// l'effet de PENDING à DONE). Rend `true` si la transition a eu lieu.
    fn complete_formation(&self, event_id: &[u8; 16]) -> Result<bool, String>;
}

// ===================================================================
// Adaptateur cognitif
// ===================================================================

/// Délégation `Arc<T>` : permet de détenir `Arc<dyn AtomicEventLedger>` (le
/// manager) et d'en construire un adaptateur sans connaître le type concret.
impl<T: AtomicEventLedger + ?Sized> AtomicEventLedger for std::sync::Arc<T> {
    fn resource_revision(&self, resource_id: &[u8]) -> Result<u64, String> {
        (**self).resource_revision(resource_id)
    }

    fn resource_state(&self, resource_id: &[u8]) -> Result<Option<(Vec<u8>, u64)>, String> {
        (**self).resource_state(resource_id)
    }

    fn apply_event(
        &self,
        event_id: &[u8; 16],
        resource_id: &[u8],
        definition: &[u8],
        precondition_revision: u64,
        resulting_state: &[u8],
        resulting_revision: u64,
    ) -> Result<LedgerOutcome, String> {
        (**self).apply_event(
            event_id,
            resource_id,
            definition,
            precondition_revision,
            resulting_state,
            resulting_revision,
        )
    }

    fn resolve(&self, event_id: &[u8; 16]) -> Result<ResolvedState, String> {
        (**self).resolve(event_id)
    }

    fn abandon(&self, event_id: &[u8; 16]) -> Result<AbandonOutcome, String> {
        (**self).abandon(event_id)
    }

    fn complete_formation(&self, event_id: &[u8; 16]) -> Result<bool, String> {
        (**self).complete_formation(event_id)
    }
}

/// Adaptateur cognitif : sérialise les concepts (`ObservationId`,
/// `ObservationDefinition`, `ResultingState`) vers la primitive générique et
/// interprète les issues.
pub struct CognitiveObservationAdapter<L: AtomicEventLedger> {
    ledger: L,
}

/// Préfixe d'effet : révision. Suit : cible u64 (engramme) puis
/// `ResultingState` sérialisé. La CIBLE est conservée dans la PREUVE de
/// transformation, même hors de la définition brute.
pub(crate) const EFFECT_REVISION: u8 = 0x01;
/// Préfixe d'effet : formation TERMINÉE (activation effectuée).
pub(crate) const EFFECT_FORMATION_DONE: u8 = 0x02;
/// Préfixe d'effet : formation RÉSERVÉE, activation pas encore confirmée.
pub(crate) const EFFECT_FORMATION_PENDING: u8 = 0x03;

impl<L: AtomicEventLedger> CognitiveObservationAdapter<L> {
    /// Construit un adaptateur au-dessus d'une primitive d'observation.
    pub fn new(ledger: L) -> Self {
        Self { ledger }
    }

    /// Enregistre une observation de FORMATION (identité seule, aucun état).
    /// Un rejeu rend `AlreadyFormed` sans réactiver.
    pub fn apply_formation(
        &self,
        observation_id: ObservationId,
        resource: &[u8],
        definition: &ObservationDefinition,
    ) -> Result<ObservationOutcome, String> {
        let outcome = self.ledger.apply_event(
            &observation_id.0,
            resource,
            &definition.canonical_bytes(),
            0,
            &[EFFECT_FORMATION_PENDING],
            1,
        )?;
        Ok(match outcome {
            LedgerOutcome::Applied => ObservationOutcome::Formed,
            LedgerOutcome::AlreadyApplied { state, .. } => match state.first() {
                Some(&EFFECT_FORMATION_DONE) => ObservationOutcome::AlreadyFormed,
                Some(&EFFECT_FORMATION_PENDING) => ObservationOutcome::FormationPending,
                _ => ObservationOutcome::AlreadyFormed,
            },
            LedgerOutcome::Abandoned => ObservationOutcome::Abandoned,
            LedgerOutcome::Indeterminate => ObservationOutcome::Indeterminate,
            LedgerOutcome::IdentityConflict {
                recorded_definition,
            } => ObservationOutcome::IdentityConflict {
                recorded: decode_definition(&recorded_definition)?,
            },
            LedgerOutcome::PreconditionFailed { current_revision } => {
                ObservationOutcome::RevisionConflict { current_revision }
            }
        })
    }

    /// Accès à la primitive sous-jacente (tests, diagnostics).
    pub fn ledger(&self) -> &L {
        &self.ledger
    }

    /// Applique une observation (check-and-set identité + révision, état
    /// résultant atomique).
    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        &self,
        observation_id: ObservationId,
        target: EngramId,
        resource: &[u8],
        definition: &ObservationDefinition,
        source_revision: SourceRevision,
        resulting_state: &ResultingState,
        resulting_revision: u64,
    ) -> Result<ObservationOutcome, String> {
        let mut effect = Vec::with_capacity(9 + 16 * resulting_state.ensemble.len() + 13);
        effect.push(EFFECT_REVISION);
        effect.extend_from_slice(&target.0.to_be_bytes());
        effect.extend_from_slice(&resulting_state.canonical_bytes());
        let outcome = self.ledger.apply_event(
            &observation_id.0,
            resource,
            &definition.canonical_bytes(),
            source_revision.0,
            &effect,
            resulting_revision,
        )?;
        Ok(match outcome {
            LedgerOutcome::Applied => ObservationOutcome::Applied {
                state: resulting_state.clone(),
                revision: resulting_revision,
            },
            LedgerOutcome::AlreadyApplied { state, revision } => match state.first() {
                Some(&EFFECT_REVISION) => {
                    if state.len() < 9 {
                        return Err("effet révision tronqué".into());
                    }
                    let recorded_target =
                        EngramId(u64::from_be_bytes(state[1..9].try_into().unwrap()));
                    ObservationOutcome::Replayed {
                        state: ResultingState::from_canonical_bytes(&state[9..])?,
                        revision,
                        target: recorded_target,
                    }
                }
                Some(&EFFECT_FORMATION_DONE) => ObservationOutcome::AlreadyFormed,
                Some(&EFFECT_FORMATION_PENDING) => ObservationOutcome::FormationPending,
                _ => return Err("effet enregistré invalide (préfixe inconnu)".into()),
            },
            LedgerOutcome::IdentityConflict {
                recorded_definition,
            } => {
                // Reconstruire la définition enregistrée depuis ses octets.
                let recorded = decode_definition(&recorded_definition)?;
                ObservationOutcome::IdentityConflict { recorded }
            }
            LedgerOutcome::PreconditionFailed { current_revision } => {
                ObservationOutcome::RevisionConflict { current_revision }
            }
            LedgerOutcome::Abandoned => ObservationOutcome::Abandoned,
            LedgerOutcome::Indeterminate => ObservationOutcome::Indeterminate,
        })
    }

    /// Confirme la fin d'une formation (PENDING → DONE).
    pub fn complete_formation(&self, observation_id: ObservationId) -> Result<bool, String> {
        self.ledger.complete_formation(&observation_id.0)
    }

    /// Résout un état indéterminé : interroge l'état transactionnel réel.
    pub fn resolve(&self, observation_id: ObservationId) -> Result<ResolvedState, String> {
        self.ledger.resolve(&observation_id.0)
    }

    /// Abandon conditionnel : rend l'issue, ou `Err` si déjà appliquée n'est
    /// pas un abandon.
    pub fn abandon(&self, observation_id: ObservationId) -> Result<AbandonOutcome, String> {
        self.ledger.abandon(&observation_id.0)
    }
}

/// Décode une définition canonique (utilisé pour rapporter la définition
/// enregistrée lors d'un conflit d'identité).
fn decode_definition(bytes: &[u8]) -> Result<ObservationDefinition, String> {
    let mut cur = 0usize;
    let take = |cur: &mut usize, n: usize| -> Result<&[u8], String> {
        let end = cur.checked_add(n).ok_or("débordement")?;
        let slice = bytes.get(*cur..end).ok_or("tronqué")?;
        *cur = end;
        Ok(slice)
    };
    let version = u32::from_be_bytes(take(&mut cur, 4)?.try_into().unwrap());
    if version != OBSERVATION_DEFINITION_VERSION {
        return Err(format!("version de définition inconnue : {version}"));
    }
    let n = u64::from_be_bytes(take(&mut cur, 8)?.try_into().unwrap());
    let mut new_nodes = Vec::with_capacity(n as usize);
    for _ in 0..n {
        new_nodes.push(NodeId(u64::from_be_bytes(
            take(&mut cur, 8)?.try_into().unwrap(),
        )));
    }
    let prediction_error_bits = u64::from_be_bytes(take(&mut cur, 8)?.try_into().unwrap());
    // Les octets sont canoniques par construction : on reconstruit sans
    // re-canoniser (new() refuserait un NaN déjà refusé à la source).
    Ok(ObservationDefinition {
        version,
        new_nodes,
        prediction_error_bits,
    })
}

// ===================================================================
// Implémentation de RÉFÉRENCE en mémoire (mock + spécification exécutable)
// ===================================================================

#[derive(Debug, Clone)]
enum EventEntry {
    Applied {
        definition: Vec<u8>,
        state: Vec<u8>,
        revision: u64,
    },
    Abandoned,
    /// Réservée mais non remplie : transaction interrompue (indéterminé).
    Claimed,
}

/// Registre d'événements de RÉFÉRENCE, en mémoire.
///
/// Sert de spécification exécutable du contrat ET de mock de test.
///
/// Propriétés fournies : ATOMICITÉ et ISOLATION (un mutex par registre),
/// VISIBILITÉ immédiate. Propriétés ABSENTES : DURABILITÉ et RÉCUPÉRATION
/// réelles (état volatil) — ce n'est PAS une implémentation de production. Le
/// chemin `Claimed` modélise seulement l'état qu'un crash laisserait.
pub struct InMemoryAtomicEventLedger {
    events: Mutex<HashMap<[u8; 16], EventEntry>>,
    resources: Mutex<HashMap<Vec<u8>, (Vec<u8>, u64)>>,
    fail_partial_write: std::sync::atomic::AtomicBool,
    /// Injection (test) : la prochaine application RÉUSSIT réellement mais
    /// rapporte `Indeterminate` — exerce la résolution qui doit restituer
    /// l'état EFFECTIVEMENT enregistré.
    indeterminate_but_applied: std::sync::atomic::AtomicBool,
    /// Injection (test) : `complete_formation` échoue (interruption avant
    /// confirmation de l'activation).
    fail_complete_formation: std::sync::atomic::AtomicBool,
}

impl Default for InMemoryAtomicEventLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryAtomicEventLedger {
    /// Registre de référence vide (volatil, non durable).
    pub fn new() -> Self {
        Self {
            events: Mutex::new(HashMap::new()),
            resources: Mutex::new(HashMap::new()),
            fail_partial_write: std::sync::atomic::AtomicBool::new(false),
            indeterminate_but_applied: std::sync::atomic::AtomicBool::new(false),
            fail_complete_formation: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Injection (test) : la prochaine application réussit mais rapporte
    /// `Indeterminate` (l'effet EST enregistré, la confirmation ne l'est pas).
    pub fn set_indeterminate_but_applied(&self, enabled: bool) {
        self.indeterminate_but_applied
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }

    /// Injection (test) : `complete_formation` ÉCHOUE — reproduit une
    /// interruption entre l'activation effective et la confirmation, et permet
    /// de MESURER la fenêtre de double application.
    pub fn set_fail_complete_formation(&self, fail: bool) {
        self.fail_complete_formation
            .store(fail, std::sync::atomic::Ordering::Relaxed);
    }

    /// Test : REMPLACE l'effet enregistré d'une observation déjà appliquée par
    /// un état DIFFÉRENT — permet de distinguer « autorité transactionnelle »
    /// de « candidat local » dans la résolution.
    pub fn set_recorded_effect(
        &self,
        observation_id: ObservationId,
        effect: Vec<u8>,
        revision: u64,
    ) {
        let mut events = self.events.lock().unwrap();
        if let Some(EventEntry::Applied { definition, .. }) = events.get(&observation_id.0) {
            let definition = definition.clone();
            events.insert(
                observation_id.0,
                EventEntry::Applied {
                    definition,
                    state: effect,
                    revision,
                },
            );
        }
    }

    /// Injection de faute (test) : la PROCHAINE application simule un crash
    /// AU DÉBUT de la transaction. Si l'identité est ABSENTE, elle est laissée
    /// `Claimed` (réservée non remplie) ; si elle est DÉJÀ APPLIQUÉE, elle n'est
    /// PAS touchée (le rejeu échoue sans corrompre l'application existante).
    pub fn set_fail_partial_write(&self, fail: bool) {
        self.fail_partial_write
            .store(fail, std::sync::atomic::Ordering::Relaxed);
    }
}

impl AtomicEventLedger for InMemoryAtomicEventLedger {
    fn resource_revision(&self, resource_id: &[u8]) -> Result<u64, String> {
        Ok(self
            .resources
            .lock()
            .unwrap()
            .get(resource_id)
            .map_or(0, |(_, r)| *r))
    }

    fn resource_state(&self, resource_id: &[u8]) -> Result<Option<(Vec<u8>, u64)>, String> {
        Ok(self.resources.lock().unwrap().get(resource_id).cloned())
    }

    fn apply_event(
        &self,
        event_id: &[u8; 16],
        resource_id: &[u8],
        definition: &[u8],
        precondition_revision: u64,
        resulting_state: &[u8],
        resulting_revision: u64,
    ) -> Result<LedgerOutcome, String> {
        let mut events = self.events.lock().unwrap();
        let mut resources = self.resources.lock().unwrap();

        // Crash simulé AU DÉBUT de la transaction.
        if self
            .fail_partial_write
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            // Une identité déjà APPLIQUÉE n'est jamais corrompue par l'échec :
            // seule une réservation neuve est laissée `Claimed`.
            if !events.contains_key(event_id) {
                events.insert(*event_id, EventEntry::Claimed);
            }
            return Err("crash simulé pendant l'application".into());
        }

        // 1) Verrou d'identité.
        match events.get(event_id) {
            Some(EventEntry::Applied {
                definition: recorded,
                state,
                revision,
            }) => {
                return if recorded.as_slice() == definition {
                    Ok(LedgerOutcome::AlreadyApplied {
                        state: state.clone(),
                        revision: *revision,
                    })
                } else {
                    Ok(LedgerOutcome::IdentityConflict {
                        recorded_definition: recorded.clone(),
                    })
                };
            }
            Some(EventEntry::Abandoned) => return Ok(LedgerOutcome::Abandoned),
            Some(EventEntry::Claimed) => return Ok(LedgerOutcome::Indeterminate),
            None => {}
        }

        // 2) Précondition de révision (point A) : l'observation a été calculée
        //    depuis `precondition_revision`. Si la ressource a avancé, c'est un
        //    conflit de révision (recalculer, même identité).
        let current = resources.get(resource_id).map_or(0, |(_, r)| *r);
        if precondition_revision != current {
            return Ok(LedgerOutcome::PreconditionFailed {
                current_revision: current,
            });
        }

        // 3) Application atomique : identité + état + révision ensemble.
        events.insert(
            *event_id,
            EventEntry::Applied {
                definition: definition.to_vec(),
                state: resulting_state.to_vec(),
                revision: resulting_revision,
            },
        );
        resources.insert(
            resource_id.to_vec(),
            (resulting_state.to_vec(), resulting_revision),
        );
        // Mode test : l'effet est enregistré, mais la confirmation est
        // indéterminée — resolve() devra restituer l'état ENREGISTRÉ.
        if self
            .indeterminate_but_applied
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            return Ok(LedgerOutcome::Indeterminate);
        }
        Ok(LedgerOutcome::Applied)
    }

    fn resolve(&self, event_id: &[u8; 16]) -> Result<ResolvedState, String> {
        let mut events = self.events.lock().unwrap();
        match events.get(event_id) {
            Some(EventEntry::Applied {
                definition,
                state,
                revision,
            }) => Ok(ResolvedState::Applied {
                definition: definition.clone(),
                state: state.clone(),
                revision: *revision,
            }),
            Some(EventEntry::Abandoned) => Ok(ResolvedState::Abandoned),
            Some(EventEntry::Claimed) => {
                // Transaction jamais aboutie : aucune application. On nettoie
                // la réservation, un rejeu pourra appliquer.
                events.remove(event_id);
                Ok(ResolvedState::NotApplied)
            }
            None => Ok(ResolvedState::NotApplied),
        }
    }

    fn complete_formation(&self, event_id: &[u8; 16]) -> Result<bool, String> {
        if self
            .fail_complete_formation
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return Err("injected complete_formation failure".into());
        }
        let mut events = self.events.lock().unwrap();
        match events.get(event_id) {
            Some(EventEntry::Applied {
                definition, state, ..
            }) if state.first() == Some(&EFFECT_FORMATION_PENDING) => {
                let definition = definition.clone();
                events.insert(
                    *event_id,
                    EventEntry::Applied {
                        definition,
                        state: vec![EFFECT_FORMATION_DONE],
                        revision: 1,
                    },
                );
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn abandon(&self, event_id: &[u8; 16]) -> Result<AbandonOutcome, String> {
        let mut events = self.events.lock().unwrap();
        match events.get(event_id) {
            Some(EventEntry::Applied { revision, .. }) => Ok(AbandonOutcome::AlreadyApplied {
                revision: *revision,
            }),
            Some(EventEntry::Abandoned) => Ok(AbandonOutcome::Abandoned),
            Some(EventEntry::Claimed) | None => {
                events.insert(*event_id, EventEntry::Abandoned);
                Ok(AbandonOutcome::Abandoned)
            }
        }
    }
}

// ===================================================================
// Contre-épreuves (point G) — 10 scénarios + canonisation/version
// ===================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(n: u128) -> ObservationId {
        ObservationId::from_u128(n)
    }

    fn def(_engram: u64, nodes: &[u64], err: f64) -> ObservationDefinition {
        let nids: Vec<NodeId> = nodes.iter().map(|n| NodeId(*n)).collect();
        ObservationDefinition::new(&nids, err).unwrap()
    }

    fn state(nodes: &[(u64, f64)], strength: f64, rc: u32) -> ResultingState {
        ResultingState {
            ensemble: nodes.iter().map(|(n, w)| (NodeId(*n), *w)).collect(),
            strength,
            recall_count: rc,
        }
    }

    fn adapter() -> CognitiveObservationAdapter<InMemoryAtomicEventLedger> {
        CognitiveObservationAdapter::new(InMemoryAtomicEventLedger::new())
    }

    // 1) application + rejeu identique : aucune transformation supplémentaire.
    #[test]
    fn application_puis_rejeu_identique() {
        let a = adapter();
        let d = def(10, &[1, 2], 0.5);
        let s = state(&[(1, 0.5), (2, 0.25)], 0.6, 1);
        let first = a
            .apply(
                oid(1),
                EngramId(10),
                &10u64.to_be_bytes(),
                &d,
                SourceRevision(0),
                &s,
                1,
            )
            .unwrap();
        assert_eq!(
            first,
            ObservationOutcome::Applied {
                state: s.clone(),
                revision: 1
            }
        );
        let replay = a
            .apply(
                oid(1),
                EngramId(10),
                &10u64.to_be_bytes(),
                &d,
                SourceRevision(0),
                &s,
                1,
            )
            .unwrap();
        assert_eq!(
            replay,
            ObservationOutcome::Replayed {
                state: s,
                revision: 1,
                target: EngramId(10)
            }
        );
        // La ressource n'a pas bougé au rejeu.
        assert_eq!(
            a.ledger().resource_revision(&10u64.to_be_bytes()).unwrap(),
            1
        );
    }

    // 2) même identité, définition incompatible → conflit d'identité nommé.
    #[test]
    fn meme_identite_contenu_incompatible() {
        let a = adapter();
        let d1 = def(10, &[1], 0.5);
        let d2 = def(10, &[2], 0.5);
        a.apply(
            oid(1),
            EngramId(10),
            &10u64.to_be_bytes(),
            &d1,
            SourceRevision(0),
            &state(&[(1, 0.5)], 0.5, 1),
            1,
        )
        .unwrap();
        let out = a
            .apply(
                oid(1),
                EngramId(10),
                &10u64.to_be_bytes(),
                &d2,
                SourceRevision(0),
                &state(&[(2, 0.5)], 0.5, 1),
                1,
            )
            .unwrap();
        match out {
            ObservationOutcome::IdentityConflict { recorded } => {
                assert_eq!(recorded.canonical_bytes(), d1.canonical_bytes());
            }
            other => panic!("attendu IdentityConflict, reçu {other:?}"),
        }
    }

    // 3) deux observations de contenu identique = DEUX applications réelles.
    #[test]
    fn deux_observations_identiques_deux_applications() {
        let a = adapter();
        let d = def(10, &[7], 0.5);
        let s = state(&[(7, 0.25)], 0.6, 1);
        let o1 = a
            .apply(
                oid(1),
                EngramId(10),
                &10u64.to_be_bytes(),
                &d,
                SourceRevision(0),
                &s,
                1,
            )
            .unwrap();
        // La seconde est calculée depuis la révision COURANTE (1), sinon elle
        // serait en conflit de révision.
        let o2 = a
            .apply(
                oid(2),
                EngramId(10),
                &10u64.to_be_bytes(),
                &d,
                SourceRevision(1),
                &s,
                2,
            )
            .unwrap();
        assert!(matches!(o1, ObservationOutcome::Applied { .. }));
        assert!(
            matches!(o2, ObservationOutcome::Applied { .. }),
            "deux observations distinctes de contenu identique sont deux applications"
        );
        assert_eq!(
            a.ledger().resource_revision(&10u64.to_be_bytes()).unwrap(),
            2
        );
    }

    // 4) deux concurrentes depuis la MÊME révision : la seconde est en conflit
    //    de révision (pas de mise à jour perdue).
    #[test]
    fn deux_concurrentes_meme_revision() {
        let a = adapter();
        let s1 = state(&[(1, 0.5)], 0.6, 1);
        let s2 = state(&[(2, 0.5)], 0.6, 1);
        let o1 = a
            .apply(
                oid(1),
                EngramId(10),
                &10u64.to_be_bytes(),
                &def(10, &[1], 0.5),
                SourceRevision(0),
                &s1,
                1,
            )
            .unwrap();
        let o2 = a
            .apply(
                oid(2),
                EngramId(10),
                &10u64.to_be_bytes(),
                &def(10, &[2], 0.5),
                SourceRevision(0),
                &s2,
                1,
            )
            .unwrap();
        assert!(matches!(o1, ObservationOutcome::Applied { .. }));
        assert_eq!(
            o2,
            ObservationOutcome::RevisionConflict {
                current_revision: 1
            },
            "la seconde observation calculée depuis la révision 0 est périmée"
        );
    }

    // 5) conflit de révision puis recalcul depuis l'état courant, MÊME identité.
    #[test]
    fn conflit_de_revision_puis_recalcul() {
        let a = adapter();
        a.apply(
            oid(1),
            EngramId(10),
            &10u64.to_be_bytes(),
            &def(10, &[1], 0.5),
            SourceRevision(0),
            &state(&[(1, 0.5)], 0.6, 1),
            1,
        )
        .unwrap();
        let d2 = def(10, &[2], 0.5);
        let conflict = a
            .apply(
                oid(2),
                EngramId(10),
                &10u64.to_be_bytes(),
                &d2,
                SourceRevision(0),
                &state(&[(2, 0.5)], 0.6, 1),
                1,
            )
            .unwrap();
        assert_eq!(
            conflict,
            ObservationOutcome::RevisionConflict {
                current_revision: 1
            }
        );
        // Recalcul depuis la révision courante (1) : même identité logique,
        // candidat légitimement différent.
        let recomputed = a
            .apply(
                oid(2),
                EngramId(10),
                &10u64.to_be_bytes(),
                &d2,
                SourceRevision(1),
                &state(&[(2, 0.25)], 0.7, 1),
                2,
            )
            .unwrap();
        assert!(
            matches!(recomputed, ObservationOutcome::Applied { .. }),
            "le recalcul depuis l'état courant applique, même identité"
        );
        assert_eq!(
            a.ledger().resource_revision(&10u64.to_be_bytes()).unwrap(),
            2
        );
    }

    // 6) rejeu d'une ancienne observation après révisions : NE restaure JAMAIS
    //    son ancien candidat par-dessus une connaissance plus récente.
    #[test]
    fn rejeu_ancienne_observation_ne_restaure_pas() {
        let a = adapter();
        let d1 = def(10, &[1], 0.5);
        let s1 = state(&[(1, 0.5)], 0.6, 1);
        a.apply(
            oid(1),
            EngramId(10),
            &10u64.to_be_bytes(),
            &d1,
            SourceRevision(0),
            &s1,
            1,
        )
        .unwrap();
        a.apply(
            oid(2),
            EngramId(10),
            &10u64.to_be_bytes(),
            &def(10, &[2], 0.5),
            SourceRevision(1),
            &state(&[(1, 0.25), (2, 0.25)], 0.7, 2),
            2,
        )
        .unwrap();
        // Rejeu de l'observation ancienne.
        let replay = a
            .apply(
                oid(1),
                EngramId(10),
                &10u64.to_be_bytes(),
                &d1,
                SourceRevision(0),
                &s1,
                1,
            )
            .unwrap();
        match replay {
            ObservationOutcome::Replayed { revision, .. } => {
                assert_eq!(
                    revision, 1,
                    "l'issue rend la révision ENREGISTRÉE (informationnelle)"
                );
            }
            other => panic!("attendu Replayed, reçu {other:?}"),
        }
        // La connaissance n'a PAS été restaurée à l'ancien candidat.
        assert_eq!(
            a.ledger().resource_revision(&10u64.to_be_bytes()).unwrap(),
            2,
            "le rejeu ne réécrit pas la ressource (elle reste à la révision 2)"
        );
    }

    // 7) abandon d'une appliquée : REFUSÉ (une appliquée reste appliquée).
    #[test]
    fn abandon_d_une_appliquee_refuse() {
        let a = adapter();
        a.apply(
            oid(1),
            EngramId(10),
            &10u64.to_be_bytes(),
            &def(10, &[1], 0.5),
            SourceRevision(0),
            &state(&[(1, 0.5)], 0.6, 1),
            1,
        )
        .unwrap();
        let ab = a.abandon(oid(1)).unwrap();
        assert_eq!(ab, AbandonOutcome::AlreadyApplied { revision: 1 });
        // Toujours appliquée.
        assert!(matches!(
            a.resolve(oid(1)).unwrap(),
            ResolvedState::Applied { .. }
        ));
    }

    // 8) injection de faute lors du rejeu d'une appliquée : l'application
    //    existante n'est PAS corrompue.
    #[test]
    fn injection_faute_lors_du_rejeu_preserve_l_appliquee() {
        let a = adapter();
        let d = def(10, &[1], 0.5);
        a.apply(
            oid(1),
            EngramId(10),
            &10u64.to_be_bytes(),
            &d,
            SourceRevision(0),
            &state(&[(1, 0.5)], 0.6, 1),
            1,
        )
        .unwrap();
        a.ledger().set_fail_partial_write(true);
        let crashed = a.apply(
            oid(1),
            EngramId(10),
            &10u64.to_be_bytes(),
            &d,
            SourceRevision(0),
            &state(&[(1, 0.5)], 0.6, 1),
            1,
        );
        assert!(crashed.is_err(), "le crash simulé rend une erreur");
        // L'application existante est intacte.
        match a.resolve(oid(1)).unwrap() {
            ResolvedState::Applied { revision, .. } => assert_eq!(revision, 1),
            other => panic!("attendu Applied intact, reçu {other:?}"),
        }
    }

    // 9) indéterminé puis résolution : on interroge l'état transactionnel, on
    //    ne devine pas et on ne rejoue pas à l'aveugle.
    #[test]
    fn indetermine_puis_resolution() {
        let a = adapter();
        let d = def(10, &[1], 0.5);
        let s = state(&[(1, 0.5)], 0.6, 1);
        a.ledger().set_fail_partial_write(true);
        let crashed = a.apply(
            oid(1),
            EngramId(10),
            &10u64.to_be_bytes(),
            &d,
            SourceRevision(0),
            &s,
            1,
        );
        assert!(crashed.is_err(), "transaction interrompue");
        // Résolution : la transaction n'a pas abouti → aucune application.
        assert_eq!(a.resolve(oid(1)).unwrap(), ResolvedState::NotApplied);
        // Le rejeu peut alors appliquer proprement.
        let retry = a
            .apply(
                oid(1),
                EngramId(10),
                &10u64.to_be_bytes(),
                &d,
                SourceRevision(0),
                &s,
                1,
            )
            .unwrap();
        assert!(matches!(retry, ObservationOutcome::Applied { .. }));
    }

    // 10) conservation ATOMIQUE identité + état résultant (les deux ensemble).
    #[test]
    fn conservation_atomique_identite_et_etat() {
        let a = adapter();
        let d = def(10, &[1, 2], 0.5);
        let s = state(&[(1, 0.5), (2, 0.25)], 0.6, 1);
        a.apply(
            oid(1),
            EngramId(10),
            &10u64.to_be_bytes(),
            &d,
            SourceRevision(0),
            &s,
            1,
        )
        .unwrap();
        match a.resolve(oid(1)).unwrap() {
            ResolvedState::Applied {
                definition,
                state,
                revision,
            } => {
                assert_eq!(definition, d.canonical_bytes(), "définition conservée");
                assert_eq!(
                    state.first(),
                    Some(&EFFECT_REVISION),
                    "préfixe d'effet révision"
                );
                assert_eq!(
                    ResultingState::from_canonical_bytes(&state[9..]).unwrap(),
                    s,
                    "état résultant conservé"
                );
                assert_eq!(revision, 1);
            }
            other => panic!("attendu Applied, reçu {other:?}"),
        }
    }

    // Canonisation : ordre et doublons ne changent PAS la définition.
    #[test]
    fn definition_canonique_insensible_ordre_et_doublons() {
        let a = def(10, &[2, 1, 2, 1], 0.5);
        let b = def(10, &[1, 2], 0.5);
        assert_eq!(a.canonical_bytes(), b.canonical_bytes());
        assert_eq!(a, b);
    }

    // A. Deux observations INDÉPENDANTES de même contenu renforcent chacune :
    // une identité de CONTENU ne supprime pas une occurrence indépendante.
    #[test]
    fn deux_formations_independantes_de_meme_contenu_renforcent() {
        let a = adapter();
        let d = def(10, &[1, 2], 0.5);
        let o1 = a.apply_formation(oid(1), b"F:obs1", &d).unwrap();
        let o2 = a.apply_formation(oid(2), b"F:obs2", &d).unwrap();
        assert_eq!(o1, ObservationOutcome::Formed, "première formation");
        assert_eq!(
            o2,
            ObservationOutcome::Formed,
            "seconde indépendante, même contenu"
        );
        assert!(a.complete_formation(oid(1)).unwrap());
        assert!(a.complete_formation(oid(2)).unwrap());
    }

    // B. Une formation RÉSERVÉE mais non confirmée n'est PAS déclarée faite :
    // le rejeu doit réactiver (Pending), jamais AlreadyFormed.
    #[test]
    fn formation_pending_non_confirmee_n_est_pas_declaree_faite() {
        let a = adapter();
        let d = def(10, &[1, 2], 0.5);
        assert_eq!(
            a.apply_formation(oid(1), b"F:obs1", &d).unwrap(),
            ObservationOutcome::Formed
        );
        // Rejeu AVANT confirmation : PENDING (activation pas confirmée).
        assert_eq!(
            a.apply_formation(oid(1), b"F:obs1", &d).unwrap(),
            ObservationOutcome::FormationPending
        );
        // Confirmation explicite, puis rejeu : DONE.
        assert!(a.complete_formation(oid(1)).unwrap());
        assert_eq!(
            a.apply_formation(oid(1), b"F:obs1", &d).unwrap(),
            ObservationOutcome::AlreadyFormed
        );
    }

    // D. Une ancienne définition (v1, avec cible) est explicitement REJETÉE :
    // le format a changé et la version le dit.
    #[test]
    fn ancienne_version_de_definition_rejetee() {
        let mut bytes = def(10, &[1], 0.5).canonical_bytes();
        bytes[3] = 1; // force la version 1 (ancienne, format différent)
        assert!(decode_definition(&bytes).is_err(), "v1 rejetée");
    }

    // Une erreur non finie est refusée AVANT toute persistance.
    #[test]
    fn definition_refuse_non_fini() {
        let nids = [NodeId(1)];
        assert!(ObservationDefinition::new(&nids, f64::NAN).is_err());
        assert!(ObservationDefinition::new(&nids, f64::INFINITY).is_err());
    }

    // La version fait partie de la définition (une version différente n'est pas
    // la même observation).
    #[test]
    fn definition_versionnee() {
        let d = def(10, &[1], 0.5);
        assert_eq!(d.version(), OBSERVATION_DEFINITION_VERSION);
        let mut bytes = d.canonical_bytes();
        bytes[3] = bytes[3].wrapping_add(1); // altère la version
        assert_ne!(bytes, d.canonical_bytes());
    }

    // ---- Branches limites du registre de référence ----

    // `Default` doit valoir `new()` : un registre par défaut ne porte AUCUN état
    // fantôme (ni identité connue, ni révision de ressource).
    #[test]
    fn registre_par_defaut_est_vide() {
        let registre = InMemoryAtomicEventLedger::default();
        assert_eq!(
            registre.resource_revision(b"ressource").unwrap(),
            0,
            "aucune ressource → révision 0"
        );
        assert_eq!(
            registre.resource_state(b"ressource").unwrap(),
            None,
            "aucune ressource → aucun état enregistré"
        );
        assert_eq!(
            registre.resolve(&oid(1).0).unwrap(),
            ResolvedState::NotApplied,
            "identité inconnue → jamais appliquée"
        );
    }

    // Une identité ABANDONNÉE avant toute application le reste : le rejeu rend
    // `Abandoned` et n'écrit rien (l'abandon est une décision de la tentative).
    #[test]
    fn identite_abandonnee_n_est_jamais_appliquee_au_rejeu() {
        let registre = InMemoryAtomicEventLedger::new();
        let observation = oid(7);
        assert_eq!(
            registre.abandon(&observation.0).unwrap(),
            AbandonOutcome::Abandoned,
            "abandon d'une identité non appliquée : accepté"
        );
        let issue = registre
            .apply_event(
                &observation.0,
                b"F",
                b"definition",
                0,
                &[EFFECT_REVISION],
                1,
            )
            .unwrap();
        assert_eq!(
            issue,
            LedgerOutcome::Abandoned,
            "une identité abandonnée n'est jamais appliquée"
        );
        assert_eq!(
            registre.resource_revision(b"F").unwrap(),
            0,
            "l'abandon n'a rien écrit sur la ressource"
        );
    }

    // Un effet enregistré dont le préfixe n'est ni révision ni formation est une
    // donnée INVALIDE pour `apply` : refusée, jamais réinterprétée.
    #[test]
    fn effet_enregistre_de_prefixe_inconnu_refuse_par_apply() {
        let adaptateur = adapter();
        let observation = oid(11);
        let d = def(1, &[1, 2], 0.5);
        let etat = state(&[(1, 1.0)], 0.5, 0);
        let _ = adaptateur
            .apply(
                observation,
                EngramId(1),
                b"R",
                &d,
                SourceRevision(0),
                &etat,
                1,
            )
            .unwrap();
        // L'effet enregistré est REMPLACÉ par un préfixe inconnu (0x09).
        adaptateur
            .ledger()
            .set_recorded_effect(observation, vec![0x09], 1);
        let erreur = adaptateur
            .apply(
                observation,
                EngramId(1),
                b"R",
                &d,
                SourceRevision(0),
                &etat,
                1,
            )
            .expect_err("un préfixe d'effet inconnu doit être refusé");
        assert!(
            erreur.contains("préfixe inconnu"),
            "message attendu, vu : {erreur}"
        );
    }

    // Le même effet invalide, vu par la voie FORMATION, ne fabrique pas un
    // « rejeu » silencieux : l'identité a été traitée, rien n'est réactivé.
    #[test]
    fn effet_enregistre_de_prefixe_inconnu_reste_une_formation_deja_faite() {
        let adaptateur = adapter();
        let observation = oid(12);
        let d = def(1, &[1, 2], 0.5);
        assert_eq!(
            adaptateur.apply_formation(observation, b"F", &d).unwrap(),
            ObservationOutcome::Formed,
            "première formation : appliquée"
        );
        adaptateur
            .ledger()
            .set_recorded_effect(observation, vec![0x09], 1);
        assert_eq!(
            adaptateur.apply_formation(observation, b"F", &d).unwrap(),
            ObservationOutcome::AlreadyFormed,
            "identité déjà traitée : aucune réactivation"
        );
    }
}
