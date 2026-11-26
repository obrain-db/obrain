//! Ledger d'observation DURABLE, adossé à l'autorité conditionnelle native du
//! substrat (`capturer_precondition` / `preparer_transformation` /
//! `publier_si_precondition_intacte` / `resoudre_reference`).
//!
//! # Autorité canonique GLOBALE (correction du défaut 52406858)
//!
//! L'`ObservationId` a une identité CANONIQUE dans son domaine d'autorité :
//! le même K ne doit JAMAIS autoriser deux définitions incompatibles, et la
//! ressource n'est PAS implicitement une partie de K. Le registre précédent
//! écrivait `obs:<hex>` comme propriété DU NŒUD ressource
//! (`capturer_precondition` mono-cible) : le même K appliqué sur A puis sur B
//! produisait DEUX enregistrements indépendants (portée PAR-RESSOURCE), alors
//! que le registre MÉMOIRE (`InMemoryAtomicEventLedger`) déduplique
//! GLOBALEMENT. Ce fichier corrige l'implémentation durable pour respecter la
//! portée GLOBALE.
//!
//! # L'autorité canonique est UN nœud substrat dédié
//!
//! Un nœud unique, découvert/créé sous le label réservé `__obs_autorite__`,
//! porte TOUS les enregistrements d'identité `obs:<hex>`. Comme
//! `capturer_precondition` est MONO-CIBLE, c'est ce nœud unique — et non la
//! ressource — qui est le domaine d'autorité de l'identité : la déduplication
//! y est GLOBALE par construction. Il n'y a PAS de second registre : ce nœud
//! est bâti SUR la même autorité transactionnelle native. Le nœud ressource,
//! lui, continue de porter la RÉVISION (`obs:revision`) et l'EFFET (`obs:effect`)
//! de la ressource — c'est la PROJECTION par ressource, pas l'autorité.
//!
//! # Frontière d'engagement (PAS d'atomicité multi-ressources)
//!
//! Une application de RÉVISION engage DEUX nœuds : le nœud autorité (identité)
//! et le nœud ressource (effet + révision). `capturer_precondition` étant
//! mono-cible, ces deux engagements NE SONT PAS atomiques. L'ordre est :
//!
//! 1. **Pré-vérification** (lecture seule) : la précondition de révision est
//!    contrôlée AVANT tout engagement — une révision périmée est refusée sans
//!    rien écrire.
//! 2. **Préparation de la matérialisation** (nœud ressource, invisible) : la
//!    transformation est préparée AVANT le commit d'identité pour produire sa
//!    `ReferenceTentative` — rien n'est publié (chaîne privée).
//! 3. **Commit d'identité** (nœud autorité, atomique) : `obs:<hex>` est écrit
//!    `APPLIQUE` (identité + définition + ressource + effet + révision + la
//!    RÉFÉRENCE de matérialisation PERSÉVÉRÉE). C'est le point de sérialisation
//!    GLOBAL de l'identité : deux tentatives concurrentes du même K n'en
//!    laissent qu'UNE.
//! 4. **Matérialisation de l'effet** (nœud ressource, atomique) : check-and-set
//!    de la précondition, écriture de `obs:revision` + `obs:effect`.
//!
//! Un arrêt entre 3 et 4 laisse l'identité COMMISE mais l'effet NON matérialisé
//! sur la ressource — état `ResolvedState::CommittedNotMaterialized` (référence
//! déjà durable dans l'enregistrement d'identité, donc récupérable). `resolve` /
//! `resolve_on` comparent l'enregistrement d'identité à la projection ressource
//! (`obs:revision` / `obs:effect`) : l'effet n'est déclaré `Applied` que s'il est
//! MATÉRIALISÉ tel quel ; une intention dont l'effet est devenu incompatible
//! n'est JAMAIS déclarée engagée. L'autorité canonique porte l'effet : un rejeu
//! (`AlreadyApplied`) répare la matérialisation via le manager. Une compensation
//! n'est PAS une transaction atomique : cette frontière est RÉELLE et qualifiée,
//! jamais devinée.
//!
//! # Formations (démarrage à froid)
//!
//! Une formation cible une ressource SYNTHÉTIQUE (`b"F:<16 octets>"`), PAS un
//! `NodeId` de 8 octets : aucune ressource cognitive n'existe encore. Le
//! registre la porte ENTIÈREMENT sur le nœud autorité (une seule écriture
//! canonique, atomique) : `APPLIQUE` + effet `FORMATION_PENDING` (0x03), que
//! `complete_formation` fait passer à `FORMATION_DONE` (0x02). Aucun nœud
//! ressource n'est exigé.
//!
//! # Contrat manquant (nommé, pas contourné)
//!
//! L'atomicité RÉELLE (une seule bascule couvrant les deux nœuds) n'existe pas
//! dans le substrat : `capturer_precondition`/`publier_si_precondition_intacte`
//! sont MONO-CIBLE. La primitive manquante est une **transaction multi-nœuds**
//! (capture + publication atomiques sur un ENSEMBLE de nœuds). Ce registre ne
//! la simule pas : il rend les frontières explicites et récupérables, et nomme
//! la primitive pour le secondaire.

use std::sync::{Arc, Mutex};

use obrain_common::types::{NodeId, Value};
use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::{
    EtatPropriete, IssuePublication, ModePublication, ReferenceTentative, SubstrateStore,
};

use super::observation::{
    AbandonOutcome, AtomicEventLedger, LedgerOutcome, ResolvedState, EFFECT_FORMATION_DONE,
    EFFECT_FORMATION_PENDING,
};

/// Clé de la révision courante d'une ressource.
const CLE_REVISION: &str = "obs:revision";
/// Clé du dernier effet appliqué (état résultant sérialisé, préfixé).
const CLE_EFFET: &str = "obs:effect";
/// Préfixe de clé d'un enregistrement par identité d'observation.
const CLE_EVENEMENT_PREFIXE: &str = "obs:";

/// Label RÉSERVÉ du nœud d'autorité canonique des observations.
const LABEL_AUTORITE: &str = "__obs_autorite__";

/// Préfixe d'enregistrement : APPLIQUÉE (révision confirmée OU formation).
const RECORD_APPLIQUE: u8 = 0x01;
/// Préfixe d'enregistrement : ABANDONNÉE (jamais appliquée, ne le sera pas).
const RECORD_ABANDONNE: u8 = 0x02;

/// Nombre maximal de ré-évaluations en cas de `ConflitPrecondition`.
const NB_REESSAIS: usize = 8;

/// Registre d'observations DURABLE : autorité canonique globale (un nœud) +
/// projection par ressource (révision + effet).
pub struct SubstrateAtomicEventLedger {
    store: Arc<SubstrateStore>,
    /// Cache du nœud d'autorité (découvert/créé paresseusement). Réinitialisé
    /// à la construction : après réouverture, le label est re-balayé.
    autorite: Mutex<Option<NodeId>>,
    /// Crochet de suspension d'engagement (test-only, PAR LEDGER) : suspend le
    /// fil APRÈS le commit d'identité et AVANT la matérialisation — la fenêtre
    /// exacte de la frontière d'engagement. Permet un entrelacement
    /// DÉTERMINISTE de deux K concurrents sur la même révision.
    /// État du crochet (arme / suspendu / reprendre).
    #[cfg(test)]
    suspension_etat: Mutex<EtatSuspension>,
    /// Condvar du crochet (test-only), HORS du mutex d'état.
    #[cfg(test)]
    suspension_cv: std::sync::Condvar,
}

/// Contenu d'un enregistrement d'identité (hors préfixe).
#[derive(Debug, Clone, PartialEq, Eq)]
struct PayloadEnregistrement {
    resource: Vec<u8>,
    definition: Vec<u8>,
    effet: Vec<u8>,
    revision: u64,
    /// Référence de matérialisation persévérée AVEC l'identité (produite à la
    /// préparation de la matérialisation, écrite au commit d'identité) : un
    /// arrêt entre les deux la laisse durable, `resolve` peut la retrouver.
    /// `None` pour une formation (aucune matérialisation sur nœud ressource).
    reference: Option<ReferenceTentative>,
}

/// Décode un `resource_id` opaque en `NodeId` : 8 octets big-endian. Toute
/// autre longueur est refusée par un nom précis (les formations, elles, portent
/// une ressource synthétique et ne passent PAS par ce décodeur).
fn decoder_cible(resource_id: &[u8]) -> Result<NodeId, String> {
    if resource_id.len() != 8 {
        return Err(format!(
            "resource_id de {} octet(s) : le registre durable exige un NodeId big-endian de 8 octets",
            resource_id.len()
        ));
    }
    let mut octets = [0u8; 8];
    octets.copy_from_slice(resource_id);
    Ok(NodeId(u64::from_be_bytes(octets)))
}

/// Clé de propriété d'un événement : `obs:<32 hex>`.
fn cle_evenement(event_id: &[u8; 16]) -> String {
    let mut s = String::with_capacity(CLE_EVENEMENT_PREFIXE.len() + 32);
    s.push_str(CLE_EVENEMENT_PREFIXE);
    for b in event_id {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn pousse_champ(out: &mut Vec<u8>, octets: &[u8]) {
    out.extend_from_slice(&(octets.len() as u32).to_le_bytes());
    out.extend_from_slice(octets);
}

/// Encode un enregistrement d'identité.
/// Format : préfixe | ressource(len+octets) | définition(len+octets) |
/// effet(len+octets) | révision u64 LE | référence(len+octets, vide = absente).
fn encode_record(
    prefixe: u8,
    resource: &[u8],
    definition: &[u8],
    effet: &[u8],
    revision: u64,
    reference: Option<&ReferenceTentative>,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        1 + 4 + resource.len() + 4 + definition.len() + 4 + effet.len() + 8 + 4
            + reference.map(|r| r.to_bytes().len()).unwrap_or(0),
    );
    out.push(prefixe);
    pousse_champ(&mut out, resource);
    pousse_champ(&mut out, definition);
    pousse_champ(&mut out, effet);
    out.extend_from_slice(&revision.to_le_bytes());
    match reference {
        Some(r) => pousse_champ(&mut out, &r.to_bytes()),
        None => pousse_champ(&mut out, b""),
    }
    out
}

/// Décode un enregistrement d'identité. Refus nommé, jamais de panique.
fn decode_record(record: &[u8]) -> Result<(u8, Option<PayloadEnregistrement>), String> {
    let Some((&prefixe, reste)) = record.split_first() else {
        return Err("enregistrement d'événement vide".to_string());
    };
    match prefixe {
        RECORD_ABANDONNE => return Ok((prefixe, None)),
        RECORD_APPLIQUE => {}
        _ => {
            return Err(format!(
                "enregistrement d'événement invalide (préfixe {prefixe:#04x})"
            ))
        }
    }
    let mut pos = 0usize;
    let mut prend_champ = |reste: &[u8], pos: &mut usize| -> Result<Vec<u8>, String> {
        if reste.len() < *pos + 4 {
            return Err("enregistrement d'événement tronqué (longueur)".to_string());
        }
        let len = u32::from_le_bytes(reste[*pos..*pos + 4].try_into().unwrap()) as usize;
        *pos += 4;
        if reste.len() < *pos + len {
            return Err("enregistrement d'événement tronqué (corps)".to_string());
        }
        let v = reste[*pos..*pos + len].to_vec();
        *pos += len;
        Ok(v)
    };
    let resource = prend_champ(reste, &mut pos)?;
    let definition = prend_champ(reste, &mut pos)?;
    let effet = prend_champ(reste, &mut pos)?;
    if reste.len() < pos + 8 {
        return Err("enregistrement d'événement tronqué (révision)".to_string());
    }
    let revision = u64::from_le_bytes(reste[pos..pos + 8].try_into().unwrap());
    pos += 8;
    // Référence de matérialisation optionnelle : absente sur les anciens
    // enregistrements et sur les formations (champ vide).
    let reference = if pos < reste.len() {
        let octets = prend_champ(reste, &mut pos)?;
        if octets.is_empty() {
            None
        } else {
            Some(
                ReferenceTentative::from_bytes(&octets)
                    .map_err(|e| format!("référence de tentative invalide : {e}"))?,
            )
        }
    } else {
        None
    };
    Ok((
        prefixe,
        Some(PayloadEnregistrement {
            resource,
            definition,
            effet,
            revision,
            reference,
        }),
    ))
}

fn err_substrat(e: obrain_substrate::SubstrateError) -> String {
    e.to_string()
}

/// Crochet de suspension d'engagement (test-only, PAR LEDGER — jamais global) :
/// une suspension sur un ledger n'affecte jamais un autre ledger ni un autre
/// test du même processus.
#[cfg(test)]
struct EtatSuspension {
    arme: bool,
    suspendu: bool,
    reprendre: bool,
}

#[cfg(test)]
impl EtatSuspension {
    fn nouvelle() -> Self {
        Self {
            arme: false,
            suspendu: false,
            reprendre: false,
        }
    }
}

impl SubstrateAtomicEventLedger {
    /// Construit le registre au-dessus d'un `SubstrateStore` partagé.
    pub fn new(store: Arc<SubstrateStore>) -> Self {
        Self {
            store,
            autorite: Mutex::new(None),
            #[cfg(test)]
            suspension_etat: Mutex::new(EtatSuspension::nouvelle()),
            #[cfg(test)]
            suspension_cv: std::sync::Condvar::new(),
        }
    }

    /// Accès au store sous-jacent (tests, diagnostics).
    pub fn store(&self) -> &SubstrateStore {
        &self.store
    }

    /// Le nœud d'autorité canonique : découvert par label, créé paresseusement.
    /// Ce nœud UNIQUE est le domaine d'autorité GLOBALE des identités.
    fn autorite_canonique(&self) -> Result<NodeId, String> {
        let mut cache = self.autorite.lock().unwrap();
        if let Some(id) = *cache {
            return Ok(id);
        }
        let mut noeuds = self.store.nodes_by_label_balayage(LABEL_AUTORITE);
        let id = match noeuds.first() {
            Some(&id) => id,
            None => self.store.create_node(&[LABEL_AUTORITE]),
        };
        *cache = Some(id);
        Ok(id)
    }

    /// Lit et décode l'enregistrement canonique d'une identité, s'il existe.
    fn lire_record(
        &self,
        event_id: &[u8; 16],
    ) -> Result<Option<(u8, Option<PayloadEnregistrement>)>, String> {
        let autorite = self.autorite_canonique()?;
        let cle_ev = cle_evenement(event_id);
        match self
            .store
            .lire_etat_propriete(autorite, &cle_ev)
            .map_err(err_substrat)?
        {
            EtatPropriete::Presente { valeur, .. } => {
                let record = valeur
                    .as_bytes()
                    .ok_or("enregistrement d'événement non-binaire")?;
                decode_record(record).map(Some)
            }
            _ => Ok(None),
        }
    }

    /// Applique un événement et rend, en plus de l'issue, la `ReferenceTentative`
    /// NATIVE de la tentative (si une publication a eu lieu). Pour une révision,
    /// c'est la référence de l'écriture sur le NŒUD RESSOURCE ; pour une
    /// formation, celle de l'écriture sur le nœud autorité.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_avec_reference(
        &self,
        event_id: &[u8; 16],
        resource_id: &[u8],
        definition: &[u8],
        precondition_revision: u64,
        resulting_state: &[u8],
        resulting_revision: u64,
    ) -> Result<(LedgerOutcome, Option<ReferenceTentative>), String> {
        // Routage par la FORME de la ressource : une révision vise un nœud
        // substrat (NodeId big-endian 8 octets) ; une formation vise une
        // ressource SYNTHÉTIQUE (b"F:<16 octets>"), sans nœud substrat.
        match decoder_cible(resource_id) {
            Ok(cible) => self.apply_revision_durable(
                cible,
                event_id,
                resource_id,
                definition,
                precondition_revision,
                resulting_state,
                resulting_revision,
            ),
            Err(_) => {
                self.apply_formation_durable(event_id, resource_id, definition, resulting_revision)
            }
        }
    }

    /// FORMATION : une seule écriture canonique (la ressource est synthétique,
    /// aucune ressource cognitive n'existe encore). Le préfixe d'enregistrement
    /// est APPLIQUE directement ; le statut pending/done est porté par l'effet.
    fn apply_formation_durable(
        &self,
        event_id: &[u8; 16],
        resource_id: &[u8],
        definition: &[u8],
        resulting_revision: u64,
    ) -> Result<(LedgerOutcome, Option<ReferenceTentative>), String> {
        let autorite = self.autorite_canonique()?;
        let cle_ev = cle_evenement(event_id);
        for _ in 0..NB_REESSAIS {
            let pre = self
                .store
                .capturer_precondition(autorite, &[cle_ev.as_str()])
                .map_err(err_substrat)?;
            if let Some(valeur) = pre.valeur_capturee(&cle_ev) {
                let record = valeur
                    .as_bytes()
                    .ok_or("enregistrement d'événement non-binaire")?;
                let (prefixe, payload) = decode_record(record)?;
                return match prefixe {
                    RECORD_APPLIQUE => {
                        let Some(p) = payload else {
                            return Err("enregistrement d'événement sans charge".to_string());
                        };
                        if p.definition == definition {
                            Ok((
                                LedgerOutcome::AlreadyApplied {
                                    state: p.effet,
                                    revision: p.revision,
                                },
                                None,
                            ))
                        } else {
                            Ok((
                                LedgerOutcome::IdentityConflict {
                                    recorded_definition: p.definition,
                                },
                                None,
                            ))
                        }
                    }
                    RECORD_ABANDONNE => Ok((LedgerOutcome::Abandoned, None)),
                    _ => Err("enregistrement d'événement invalide (préfixe inconnu)".to_string()),
                };
            }
            let record = encode_record(
                RECORD_APPLIQUE,
                resource_id,
                definition,
                &[EFFECT_FORMATION_PENDING],
                resulting_revision,
                None,
            );
            let nouvelles = [(cle_ev.as_str(), Value::from(record))];
            let prep = self
                .store
                .preparer_transformation(autorite, &nouvelles, &pre, ModePublication::Strict)
                .map_err(err_substrat)?;
            let reference = prep.reference_tentative();
            match self
                .store
                .publier_si_precondition_intacte(prep)
                .map_err(err_substrat)?
            {
                IssuePublication::PublieIntegrale => {
                    return Ok((LedgerOutcome::Applied, Some(reference)));
                }
                IssuePublication::PubliePartielle => {
                    return Ok((LedgerOutcome::Indeterminate, Some(reference)));
                }
                IssuePublication::ConflitPrecondition => continue,
            }
        }
        Ok((LedgerOutcome::Indeterminate, None))
    }

    /// RÉVISION : commit d'identité canonique (déduplication GLOBALE) puis
    /// matérialisation de l'effet sur le nœud ressource. Frontières explicites,
    /// jamais d'atomicité multi-nœuds prétendue.
    #[allow(clippy::too_many_arguments)]
    fn apply_revision_durable(
        &self,
        cible: NodeId,
        event_id: &[u8; 16],
        resource_id: &[u8],
        definition: &[u8],
        precondition_revision: u64,
        resulting_state: &[u8],
        resulting_revision: u64,
    ) -> Result<(LedgerOutcome, Option<ReferenceTentative>), String> {
        // Ressource inexistante : refus NOMÉ (le registre durable exige un nœud
        // substrat par ressource de RÉVISION), pas un état fantôme.
        if matches!(
            self.store
                .lire_etat_propriete(cible, CLE_REVISION)
                .map_err(err_substrat)?,
            EtatPropriete::RessourceInexistante
        ) {
            return Err(format!(
                "ressource {cible:?} inexistante : le registre durable exige un nœud substrat par ressource"
            ));
        }
        let autorite = self.autorite_canonique()?;
        let cle_ev = cle_evenement(event_id);

        for _ in 0..NB_REESSAIS {
            // ── Étape A : déduplication GLOBALE (nœud autorité). ──
            let pre_canon = self
                .store
                .capturer_precondition(autorite, &[cle_ev.as_str()])
                .map_err(err_substrat)?;
            if let Some(valeur) = pre_canon.valeur_capturee(&cle_ev) {
                let record = valeur
                    .as_bytes()
                    .ok_or("enregistrement d'événement non-binaire")?;
                let (prefixe, payload) = decode_record(record)?;
                return match prefixe {
                    RECORD_APPLIQUE => {
                        let Some(p) = payload else {
                            return Err("enregistrement d'événement sans charge".to_string());
                        };
                        if p.definition == definition {
                            Ok((
                                LedgerOutcome::AlreadyApplied {
                                    state: p.effet,
                                    revision: p.revision,
                                },
                                None,
                            ))
                        } else {
                            Ok((
                                LedgerOutcome::IdentityConflict {
                                    recorded_definition: p.definition,
                                },
                                None,
                            ))
                        }
                    }
                    RECORD_ABANDONNE => Ok((LedgerOutcome::Abandoned, None)),
                    _ => {
                        Err("enregistrement d'événement invalide (préfixe inconnu)".to_string())
                    }
                };
            }

            // ── Étape 0 : pré-vérification (lecture seule) — refuse une
            //    révision périmée SANS commettre d'identité. ──
            let courant = self.resource_revision(resource_id)?;
            if courant != precondition_revision {
                return Ok((
                    LedgerOutcome::PreconditionFailed {
                        current_revision: courant,
                    },
                    None,
                ));
            }

            // ── Étape 1 : PRÉPARATION de la matérialisation — la référence de
            //    l'écriture sur le nœud ressource est produite AVANT le commit
            //    d'identité pour pouvoir y être persévérée. Rien n'est publié
            //    ici (chaîne privée invisible). ──
            let pre_res = self
                .store
                .capturer_precondition(cible, &[CLE_REVISION, CLE_EFFET])
                .map_err(err_substrat)?;
            let courant = pre_res
                .valeur_capturee(CLE_REVISION)
                .and_then(|v| v.as_int64())
                .map(|v| v as u64)
                .unwrap_or(0);
            if courant != precondition_revision {
                // Course AVANT tout engagement : refus SANS commettre d'identité.
                return Ok((
                    LedgerOutcome::PreconditionFailed {
                        current_revision: courant,
                    },
                    None,
                ));
            }
            let nouvelles = [
                (CLE_REVISION, Value::from(resulting_revision as i64)),
                (CLE_EFFET, Value::from(resulting_state.to_vec())),
            ];
            let prep_res = self
                .store
                .preparer_transformation(cible, &nouvelles, &pre_res, ModePublication::Strict)
                .map_err(err_substrat)?;
            let reference = prep_res.reference_tentative();

            // ── Étape 2 : COMMIT d'identité (APPLIQUE) — point de sérialisation
            //    GLOBAL, avec la référence de matérialisation PERSÉVÉRÉE dans
            //    l'enregistrement d'identité (un arrêt entre 2 et 3 ne la perd
            //    pas : `resolve` la retrouve via l'enregistrement). ──
            let record = encode_record(
                RECORD_APPLIQUE,
                resource_id,
                definition,
                resulting_state,
                resulting_revision,
                Some(&reference),
            );
            let prep_ident = self
                .store
                .preparer_transformation(
                    autorite,
                    &[(cle_ev.as_str(), Value::from(record))],
                    &pre_canon,
                    ModePublication::Strict,
                )
                .map_err(err_substrat)?;
            match self
                .store
                .publier_si_precondition_intacte(prep_ident)
                .map_err(err_substrat)?
            {
                IssuePublication::PublieIntegrale => {}
                IssuePublication::PubliePartielle => {
                    return Ok((LedgerOutcome::Indeterminate, None))
                }
                IssuePublication::ConflitPrecondition => continue,
            }

            // ── Étape 3 : matérialisation de l'effet (publication de la
            //    transformation préparée à l'étape 1) — précondition re-vérifiée
            //    sous le verrou du nœud ressource. ──
            #[cfg(test)]
            self.suspendre_apres_commit_identite();
            match self
                .store
                .publier_si_precondition_intacte(prep_res)
                .map_err(err_substrat)?
            {
                IssuePublication::PublieIntegrale => {
                    return Ok((LedgerOutcome::Applied, Some(reference)));
                }
                IssuePublication::PubliePartielle => {
                    return Ok((LedgerOutcome::Indeterminate, Some(reference)));
                }
                IssuePublication::ConflitPrecondition => {
                    // Course : la révision a avancé entre la préparation et la
                    // publication. L'identité est DÉJÀ commise AVEC la référence :
                    // « engagé non matérialisé », récupérable. Un rejeu réparera.
                    let courant = self.resource_revision(resource_id)?;
                    return Ok((
                        LedgerOutcome::PreconditionFailed {
                            current_revision: courant,
                        },
                        None,
                    ));
                }
            }
        }
        Ok((LedgerOutcome::Indeterminate, None))
    }

    /// Résolution SCOPÉE par ressource : l'identité appartient-elle à CETTE
    /// ressource ? (substitution incompatible → NotApplied, jamais devinée).
    ///
    /// Compare l'enregistrement d'identité à la projection ressource
    /// (`CLE_REVISION` / `CLE_EFFET`) : `Applied` si l'effet est matérialisé
    /// tel quel, `CommittedNotMaterialized` si l'identité est commise mais que
    /// l'effet ne l'est pas (ou plus) — jamais « engagé » sur un effet devenu
    /// incompatible.
    pub fn resolve_on(
        &self,
        resource_id: &[u8],
        event_id: &[u8; 16],
    ) -> Result<ResolvedState, String> {
        decoder_cible(resource_id)?;
        let Some((prefixe, payload)) = self.lire_record(event_id)? else {
            return Ok(ResolvedState::NotApplied);
        };
        match prefixe {
            RECORD_ABANDONNE => Ok(ResolvedState::Abandoned),
            RECORD_APPLIQUE => {
                let Some(p) = payload else {
                    return Err("enregistrement d'événement sans charge".to_string());
                };
                if p.resource != resource_id {
                    // L'identité est enregistrée pour une AUTRE ressource : elle
                    // ne migre pas vers celle-ci.
                    return Ok(ResolvedState::NotApplied);
                }
                self.classifier_revision(&p, resource_id)
            }
            _ => Err("enregistrement d'événement invalide (préfixe inconnu)".to_string()),
        }
    }

    /// Compare un enregistrement d'identité de RÉVISION (ressource déjà décodée
    /// en NodeId) à la projection ressource (`CLE_REVISION` / `CLE_EFFET`) :
    /// `Applied` si l'effet est matérialisé tel quel, `CommittedNotMaterialized`
    /// sinon (identité engagée, effet non — ou plus — matérialisé, récupérable :
    /// l'effet enregistré est restitué). Une intention dont l'effet est devenu
    /// incompatible n'est JAMAIS déclarée engagée.
    fn classifier_revision(
        &self,
        payload: &PayloadEnregistrement,
        resource_id: &[u8],
    ) -> Result<ResolvedState, String> {
        let projection = self.resource_state(resource_id)?;
        let materialise = matches!(
            &projection,
            Some((effet, revision)) if effet == &payload.effet && *revision == payload.revision
        );
        if materialise {
            Ok(ResolvedState::Applied {
                definition: payload.definition.clone(),
                state: payload.effet.clone(),
                revision: payload.revision,
            })
        } else {
            Ok(ResolvedState::CommittedNotMaterialized {
                definition: payload.definition.clone(),
                state: payload.effet.clone(),
                revision: payload.revision,
            })
        }
    }

    /// Abandon conditionnel SCOPÉ par ressource (refusé si déjà appliquée).
    pub fn abandon_on(
        &self,
        resource_id: &[u8],
        event_id: &[u8; 16],
    ) -> Result<AbandonOutcome, String> {
        // La ressource doit être un NodeId (refus nommé sinon) ; l'abandon,
        // lui, s'opère sur le nœud autorité (l'identité y est globale).
        let _cible = decoder_cible(resource_id)?;
        let autorite = self.autorite_canonique()?;
        let cle_ev = cle_evenement(event_id);
        for _ in 0..NB_REESSAIS {
            let pre = self
                .store
                .capturer_precondition(autorite, &[cle_ev.as_str()])
                .map_err(err_substrat)?;
            match pre.valeur_capturee(&cle_ev) {
                Some(valeur) => {
                    let record = valeur
                        .as_bytes()
                        .ok_or("enregistrement d'événement non-binaire")?;
                    let (prefixe, payload) = decode_record(record)?;
                    match prefixe {
                        RECORD_APPLIQUE => {
                            let revision = payload.map(|p| p.revision).unwrap_or(0);
                            return Ok(AbandonOutcome::AlreadyApplied { revision });
                        }
                        RECORD_ABANDONNE => return Ok(AbandonOutcome::Abandoned),
                        _ => {
                            return Err(
                                "enregistrement d'événement invalide (préfixe inconnu)".to_string()
                            )
                        }
                    }
                }
                None => {
                    let nouvelles = [(cle_ev.as_str(), Value::from(vec![RECORD_ABANDONNE]))];
                    let prep = self
                        .store
                        .preparer_transformation(autorite, &nouvelles, &pre, ModePublication::Strict)
                        .map_err(err_substrat)?;
                    match self
                        .store
                        .publier_si_precondition_intacte(prep)
                        .map_err(err_substrat)?
                    {
                        IssuePublication::PublieIntegrale => return Ok(AbandonOutcome::Abandoned),
                        IssuePublication::PubliePartielle => {
                            return Err(
                                "abandon : publication partielle (inattendu en mode strict)"
                                    .to_string(),
                            )
                        }
                        IssuePublication::ConflitPrecondition => continue,
                    }
                }
            }
        }
        Err("abandon : conflit persistant (résoudre au lieu de rejouer)".to_string())
    }
}

#[cfg(test)]
impl SubstrateAtomicEventLedger {
    /// Arme la suspension : le PROCHAIN commit d'identité suspendra le fil
    /// juste après, avant la matérialisation (no-op si jamais armé).
    fn armer_suspension_engagement(&self) {
        let mut s = self.suspension_etat.lock().unwrap();
        s.arme = true;
        s.suspendu = false;
        s.reprendre = false;
    }

    /// Bloque jusqu'à ce qu'un fil soit suspendu sur la frontière d'engagement.
    fn attendre_suspension_engagement(&self) {
        let mut s = self.suspension_etat.lock().unwrap();
        while !s.suspendu {
            s = self.suspension_cv.wait(s).unwrap();
        }
    }

    /// Relâche le fil suspendu sur la frontière d'engagement.
    fn relacher_suspension_engagement(&self) {
        let mut s = self.suspension_etat.lock().unwrap();
        s.reprendre = true;
        self.suspension_cv.notify_all();
    }

    /// Point d'interleaving DÉTERMINISTE (test-only) : appelé APRÈS le commit
    /// d'identité et AVANT la publication de la matérialisation.
    fn suspendre_apres_commit_identite(&self) {
        let mut s = self.suspension_etat.lock().unwrap();
        if !s.arme {
            return; // non armé : no-op (application normale)
        }
        // One-shot : la suspension est CONSOMMÉE par la première application qui
        // atteint la frontière — les applications suivantes passent sans attendre.
        s.arme = false;
        s.suspendu = true;
        self.suspension_cv.notify_all();
        while !s.reprendre {
            s = self.suspension_cv.wait(s).unwrap();
        }
    }
}

impl AtomicEventLedger for SubstrateAtomicEventLedger {
    fn resource_revision(&self, resource_id: &[u8]) -> Result<u64, String> {
        let cible = decoder_cible(resource_id)?;
        match self
            .store
            .lire_etat_propriete(cible, CLE_REVISION)
            .map_err(err_substrat)?
        {
            EtatPropriete::Presente { valeur, .. } => {
                Ok(valeur.as_int64().map(|v| v as u64).unwrap_or(0))
            }
            _ => Ok(0),
        }
    }

    fn resource_state(&self, resource_id: &[u8]) -> Result<Option<(Vec<u8>, u64)>, String> {
        let cible = decoder_cible(resource_id)?;
        let effet = match self
            .store
            .lire_etat_propriete(cible, CLE_EFFET)
            .map_err(err_substrat)?
        {
            EtatPropriete::Presente { valeur, .. } => valeur.as_bytes().map(|b| b.to_vec()),
            _ => None,
        };
        let Some(effet) = effet else {
            return Ok(None);
        };
        let revision = match self
            .store
            .lire_etat_propriete(cible, CLE_REVISION)
            .map_err(err_substrat)?
        {
            EtatPropriete::Presente { valeur, .. } => {
                valeur.as_int64().map(|v| v as u64).unwrap_or(0)
            }
            _ => 0,
        };
        Ok(Some((effet, revision)))
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
        self.apply_avec_reference(
            event_id,
            resource_id,
            definition,
            precondition_revision,
            resulting_state,
            resulting_revision,
        )
        .map(|(issue, _)| issue)
    }

    /// Résolution depuis l'identité SEULE : l'enregistrement canonique porte le
    /// lien vers la ressource (projection reconstructible, pas un index séparé).
    ///
    /// Distingue la frontière d'engagement : une FORMATION (ressource
    /// synthétique) est `Applied` dès le commit — son effet est ENTIÈREMENT
    /// porté par l'identité (une seule écriture canonique) ; une RÉVISION est
    /// `Applied` seulement si la projection ressource porte l'effet enregistré,
    /// sinon `CommittedNotMaterialized`.
    fn resolve(&self, event_id: &[u8; 16]) -> Result<ResolvedState, String> {
        let Some((prefixe, payload)) = self.lire_record(event_id)? else {
            return Ok(ResolvedState::NotApplied);
        };
        match prefixe {
            RECORD_ABANDONNE => Ok(ResolvedState::Abandoned),
            RECORD_APPLIQUE => {
                let Some(p) = payload else {
                    return Err("enregistrement d'événement sans charge".to_string());
                };
                // Formation (ressource synthétique non-NodeId) : pas de
                // projection ressource à comparer — l'effet est entièrement
                // porté par l'identité (une seule écriture canonique).
                if decoder_cible(&p.resource).is_err() {
                    return Ok(ResolvedState::Applied {
                        definition: p.definition,
                        state: p.effet,
                        revision: p.revision,
                    });
                }
                self.classifier_revision(&p, &p.resource)
            }
            _ => Err("enregistrement d'événement invalide (préfixe inconnu)".to_string()),
        }
    }

    /// Abandon conditionnel depuis l'identité seule (refusé si déjà appliquée).
    fn abandon(&self, event_id: &[u8; 16]) -> Result<AbandonOutcome, String> {
        let autorite = self.autorite_canonique()?;
        let cle_ev = cle_evenement(event_id);
        for _ in 0..NB_REESSAIS {
            let pre = self
                .store
                .capturer_precondition(autorite, &[cle_ev.as_str()])
                .map_err(err_substrat)?;
            match pre.valeur_capturee(&cle_ev) {
                Some(valeur) => {
                    let record = valeur
                        .as_bytes()
                        .ok_or("enregistrement d'événement non-binaire")?;
                    let (prefixe, payload) = decode_record(record)?;
                    match prefixe {
                        RECORD_APPLIQUE => {
                            let revision = payload.map(|p| p.revision).unwrap_or(0);
                            return Ok(AbandonOutcome::AlreadyApplied { revision });
                        }
                        RECORD_ABANDONNE => return Ok(AbandonOutcome::Abandoned),
                        _ => {
                            return Err(
                                "enregistrement d'événement invalide (préfixe inconnu)".to_string()
                            )
                        }
                    }
                }
                None => {
                    let nouvelles = [(cle_ev.as_str(), Value::from(vec![RECORD_ABANDONNE]))];
                    let prep = self
                        .store
                        .preparer_transformation(autorite, &nouvelles, &pre, ModePublication::Strict)
                        .map_err(err_substrat)?;
                    match self
                        .store
                        .publier_si_precondition_intacte(prep)
                        .map_err(err_substrat)?
                    {
                        IssuePublication::PublieIntegrale => return Ok(AbandonOutcome::Abandoned),
                        IssuePublication::PubliePartielle => {
                            return Err(
                                "abandon : publication partielle (inattendu en mode strict)"
                                    .to_string(),
                            )
                        }
                        IssuePublication::ConflitPrecondition => continue,
                    }
                }
            }
        }
        Err("abandon : conflit persistant (résoudre au lieu de rejouer)".to_string())
    }

    /// Confirme une formation réservée (effet FORMATION_PENDING → DONE).
    fn complete_formation(&self, event_id: &[u8; 16]) -> Result<bool, String> {
        let autorite = self.autorite_canonique()?;
        let cle_ev = cle_evenement(event_id);
        for _ in 0..NB_REESSAIS {
            let pre = self
                .store
                .capturer_precondition(autorite, &[cle_ev.as_str()])
                .map_err(err_substrat)?;
            let Some(valeur) = pre.valeur_capturee(&cle_ev) else {
                return Ok(false);
            };
            let record = valeur
                .as_bytes()
                .ok_or("enregistrement d'événement non-binaire")?;
            let (prefixe, payload) = decode_record(record)?;
            if prefixe != RECORD_APPLIQUE {
                return Ok(false);
            }
            let Some(p) = payload else {
                return Ok(false);
            };
            if p.effet.first() != Some(&EFFECT_FORMATION_PENDING) {
                // Déjà DONE (0x02) ou révision (0x01) : pas de transition.
                return Ok(false);
            }
            let nouvel = encode_record(
                RECORD_APPLIQUE,
                &p.resource,
                &p.definition,
                &[EFFECT_FORMATION_DONE],
                p.revision,
                p.reference.as_ref(),
            );
            let nouvelles = [(cle_ev.as_str(), Value::from(nouvel))];
            let prep = self
                .store
                .preparer_transformation(autorite, &nouvelles, &pre, ModePublication::Strict)
                .map_err(err_substrat)?;
            match self
                .store
                .publier_si_precondition_intacte(prep)
                .map_err(err_substrat)?
            {
                IssuePublication::PublieIntegrale => return Ok(true),
                IssuePublication::PubliePartielle => {
                    return Err(
                        "complete_formation : publication partielle (inattendu en mode strict)"
                            .to_string(),
                    )
                }
                IssuePublication::ConflitPrecondition => continue,
            }
        }
        Err("complete_formation : conflit persistant".to_string())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use obrain_core::graph::traits::GraphStoreMut;
    use obrain_substrate::IntegriteTentative;

    fn ressource_bytes(id: NodeId) -> Vec<u8> {
        id.0.to_be_bytes().to_vec()
    }

    fn ledger_memoire() -> (SubstrateAtomicEventLedger, NodeId) {
        let store = Arc::new(SubstrateStore::open_memory().unwrap());
        let id = store.create_node(&["ressource"]);
        (SubstrateAtomicEventLedger::new(store), id)
    }

    fn oid(n: u128) -> [u8; 16] {
        n.to_be_bytes()
    }

    fn effet_revision(cible: NodeId, marqueur: u8) -> Vec<u8> {
        // EFFECT_REVISION(0x01) | cible u64 | octets de test
        let mut e = Vec::new();
        e.push(0x01);
        e.extend_from_slice(&cible.0.to_be_bytes());
        e.push(marqueur);
        e
    }

    /// Ressource SYNTHÉTIQUE d'une formation (pas un NodeId) : `b"F:" + 16 octets`.
    fn ressource_formation(n: u128) -> Vec<u8> {
        let mut r = b"F:".to_vec();
        r.extend_from_slice(&n.to_be_bytes());
        r
    }

    // 1) première application : Applied, révision 1, état enregistré.
    #[test]
    fn premiere_application() {
        let (ledger, id) = ledger_memoire();
        let rid = ressource_bytes(id);
        let def = b"def-1".to_vec();
        let eff = effet_revision(id, 7);
        let out = ledger
            .apply_event(&oid(1), &rid, &def, 0, &eff, 1)
            .unwrap();
        assert_eq!(out, LedgerOutcome::Applied);
        assert_eq!(ledger.resource_revision(&rid).unwrap(), 1);
        assert_eq!(ledger.resource_state(&rid).unwrap(), Some((eff.clone(), 1)));
    }

    // 2) rejeu identique : AlreadyApplied, AUCUNE re-mutation.
    #[test]
    fn rejeu_identique_ne_remute_pas() {
        let (ledger, id) = ledger_memoire();
        let rid = ressource_bytes(id);
        let def = b"def-1".to_vec();
        let eff = effet_revision(id, 7);
        ledger.apply_event(&oid(1), &rid, &def, 0, &eff, 1).unwrap();
        let replay = ledger.apply_event(&oid(1), &rid, &def, 0, &eff, 1).unwrap();
        assert_eq!(
            replay,
            LedgerOutcome::AlreadyApplied {
                state: eff.clone(),
                revision: 1
            }
        );
        assert_eq!(ledger.resource_revision(&rid).unwrap(), 1, "pas de double application");
    }

    // 3) même identité, contenu incompatible : refus nommé.
    #[test]
    fn meme_identite_contenu_incompatible() {
        let (ledger, id) = ledger_memoire();
        let rid = ressource_bytes(id);
        ledger.apply_event(&oid(1), &rid, b"def-a", 0, &effet_revision(id, 7), 1).unwrap();
        let out = ledger
            .apply_event(&oid(1), &rid, b"def-b", 0, &effet_revision(id, 7), 1)
            .unwrap();
        assert_eq!(
            out,
            LedgerOutcome::IdentityConflict {
                recorded_definition: b"def-a".to_vec()
            }
        );
        assert_eq!(ledger.resource_revision(&rid).unwrap(), 1, "conflit sans effet");
    }

    // 4) révision devenue obsolète : PreconditionFailed.
    #[test]
    fn revision_obsolete() {
        let (ledger, id) = ledger_memoire();
        let rid = ressource_bytes(id);
        ledger.apply_event(&oid(1), &rid, b"def-a", 0, &effet_revision(id, 7), 1).unwrap();
        let out = ledger
            .apply_event(&oid(2), &rid, b"def-b", 0, &effet_revision(id, 8), 2)
            .unwrap();
        assert_eq!(
            out,
            LedgerOutcome::PreconditionFailed { current_revision: 1 }
        );
        // Recalcul depuis la révision courante : applique.
        let recomputed = ledger
            .apply_event(&oid(2), &rid, b"def-b", 1, &effet_revision(id, 8), 2)
            .unwrap();
        assert_eq!(recomputed, LedgerOutcome::Applied);
        assert_eq!(ledger.resource_revision(&rid).unwrap(), 2);
    }

    // 5) réponse perdue : ré-application du même identifiant, idempotent.
    #[test]
    fn reponse_perdue_reapplication_idempotente() {
        let (ledger, id) = ledger_memoire();
        let rid = ressource_bytes(id);
        let def = b"def-1".to_vec();
        let eff = effet_revision(id, 7);
        let first = ledger.apply_event(&oid(1), &rid, &def, 0, &eff, 1).unwrap();
        assert_eq!(first, LedgerOutcome::Applied);
        // La réponse est perdue : on renvoie la MÊME tentative.
        let resend = ledger.apply_event(&oid(1), &rid, &def, 0, &eff, 1).unwrap();
        match resend {
            LedgerOutcome::AlreadyApplied { state, revision } => {
                assert_eq!(state, eff);
                assert_eq!(revision, 1);
            }
            other => panic!("attendu AlreadyApplied, reçu {other:?}"),
        }
        assert_eq!(ledger.resource_revision(&rid).unwrap(), 1);
    }

    // 6) interruption après engagement + récupération après redémarrage :
    //    l'engagement survit à une réouverture du store, sans double application.
    #[test]
    fn interruption_apres_engagement_et_redemarrage() {
        let td = tempfile::tempdir().unwrap();
        let chemin = td.path().join("kb");
        let id = {
            let store = Arc::new(SubstrateStore::create(&chemin).unwrap());
            let id = store.create_node(&["ressource"]);
            let ledger = SubstrateAtomicEventLedger::new(store.clone());
            let rid = ressource_bytes(id);
            let def = b"def-1".to_vec();
            let eff = effet_revision(id, 7);
            let out = ledger.apply_event(&oid(1), &rid, &def, 0, &eff, 1).unwrap();
            assert_eq!(out, LedgerOutcome::Applied);
            store.close().unwrap();
            id
        };
        // Réouverture : l'état est présent, et le rejeu n'applique pas deux fois.
        let store = Arc::new(SubstrateStore::open(&chemin).unwrap());
        let ledger = SubstrateAtomicEventLedger::new(store);
        let rid = ressource_bytes(id);
        assert_eq!(ledger.resource_revision(&rid).unwrap(), 1, "révision survivante");
        let eff_attendu = effet_revision(id, 7);
        assert_eq!(
            ledger.resource_state(&rid).unwrap(),
            Some((eff_attendu.clone(), 1)),
            "état survivant"
        );
        let replay = ledger.apply_event(&oid(1), &rid, b"def-1", 0, &eff_attendu, 1).unwrap();
        assert_eq!(
            replay,
            LedgerOutcome::AlreadyApplied {
                state: eff_attendu,
                revision: 1
            },
            "rejeu après redémarrage : aucune double application"
        );
    }

    // 7) résolution par identité après redémarrage (resolve_on).
    #[test]
    fn resolution_par_identite_apres_redemarrage() {
        let td = tempfile::tempdir().unwrap();
        let chemin = td.path().join("kb");
        let (id, def) = {
            let store = Arc::new(SubstrateStore::create(&chemin).unwrap());
            let id = store.create_node(&["ressource"]);
            let ledger = SubstrateAtomicEventLedger::new(store.clone());
            let def = b"def-1".to_vec();
            ledger
                .apply_event(&oid(9), &ressource_bytes(id), &def, 0, &effet_revision(id, 3), 1)
                .unwrap();
            store.close().unwrap();
            (id, def)
        };
        let store = Arc::new(SubstrateStore::open(&chemin).unwrap());
        let ledger = SubstrateAtomicEventLedger::new(store);
        match ledger.resolve_on(&ressource_bytes(id), &oid(9)).unwrap() {
            ResolvedState::Applied {
                definition,
                state,
                revision,
            } => {
                assert_eq!(definition, def);
                assert_eq!(state, effet_revision(id, 3));
                assert_eq!(revision, 1);
            }
            other => panic!("attendu Applied, reçu {other:?}"),
        }
    }

    // 8) réparation de projection : resource_state rend l'effet ENREGISTRÉ,
    //    qu'un store en retard peut re-matérialiser sans repartir de zéro.
    #[test]
    fn reparation_de_projection_depuis_etat_enregistre() {
        let (ledger, id) = ledger_memoire();
        let rid = ressource_bytes(id);
        let eff = effet_revision(id, 7);
        ledger.apply_event(&oid(1), &rid, b"def-1", 0, &eff, 1).unwrap();
        // Le store (projection) est supposé en retard : l'état enregistré au
        // registre porte l'effet canonique à re-matérialiser.
        let (etat, rev) = ledger.resource_state(&rid).unwrap().expect("état enregistré");
        assert_eq!(rev, 1);
        assert_eq!(etat.first(), Some(&0x01), "préfixe d'effet révision");
        assert_eq!(&etat[1..9], &id.0.to_be_bytes(), "cible dans l'effet");
        assert_eq!(etat[9], 7, "marqueur d'état résultant conservé");
    }

    // 9) absence de double application : deux identités distinctes avancent la
    //    révision, un rejeu ne l'avance pas.
    #[test]
    fn absence_de_double_application() {
        let (ledger, id) = ledger_memoire();
        let rid = ressource_bytes(id);
        ledger.apply_event(&oid(1), &rid, b"d1", 0, &effet_revision(id, 1), 1).unwrap();
        ledger.apply_event(&oid(2), &rid, b"d2", 1, &effet_revision(id, 2), 2).unwrap();
        assert_eq!(ledger.resource_revision(&rid).unwrap(), 2);
        // Rejeu des deux : la révision ne bouge pas.
        ledger.apply_event(&oid(1), &rid, b"d1", 0, &effet_revision(id, 1), 1).unwrap();
        ledger.apply_event(&oid(2), &rid, b"d2", 1, &effet_revision(id, 2), 2).unwrap();
        assert_eq!(ledger.resource_revision(&rid).unwrap(), 2, "aucune avancée au rejeu");
    }

    // 10) réponse perdue résolue par la RÉFÉRENCE native (resoudre_reference),
    //     sans rejouer : l'engagement est attesté après réouverture.
    #[test]
    fn reponse_perdue_resolue_par_reference_native() {
        let td = tempfile::tempdir().unwrap();
        let chemin = td.path().join("kb");
        let (id, reference) = {
            let store = Arc::new(SubstrateStore::create(&chemin).unwrap());
            let id = store.create_node(&["ressource"]);
            let ledger = SubstrateAtomicEventLedger::new(store.clone());
            let (out, reference) = ledger
                .apply_avec_reference(&oid(1), &ressource_bytes(id), b"def-1", 0, &effet_revision(id, 7), 1)
                .unwrap();
            assert_eq!(out, LedgerOutcome::Applied);
            let reference = reference.expect("référence de tentative présente");
            store.close().unwrap();
            (id, reference)
        };
        let store = Arc::new(SubstrateStore::open(&chemin).unwrap());
        let resolution = store.resoudre_reference(&reference).unwrap();
        assert_eq!(resolution.integrite, IntegriteTentative::Integrale);
        assert_eq!(resolution.cible, id);
    }

    // 11) abandon d'une appliquée : refusé (une appliquée reste appliquée).
    #[test]
    fn abandon_d_une_appliquee_refuse() {
        let (ledger, id) = ledger_memoire();
        let rid = ressource_bytes(id);
        ledger.apply_event(&oid(1), &rid, b"d1", 0, &effet_revision(id, 1), 1).unwrap();
        assert_eq!(
            ledger.abandon_on(&rid, &oid(1)).unwrap(),
            AbandonOutcome::AlreadyApplied { revision: 1 }
        );
    }

    // 12) abandon d'une identité neuve : accepté, le rejeu rend Abandoned.
    #[test]
    fn abandon_d_une_identite_neuve_accepte() {
        let (ledger, id) = ledger_memoire();
        let rid = ressource_bytes(id);
        assert_eq!(ledger.abandon_on(&rid, &oid(5)).unwrap(), AbandonOutcome::Abandoned);
        assert_eq!(
            ledger.apply_event(&oid(5), &rid, b"d5", 0, &effet_revision(id, 5), 1).unwrap(),
            LedgerOutcome::Abandoned,
            "une identité abandonnée n'est jamais appliquée"
        );
        assert_eq!(ledger.resource_revision(&rid).unwrap(), 0, "l'abandon n'a rien écrit");
    }

    // 13) resource_id de longueur non-8 : refus nommé.
    #[test]
    fn resource_id_non_octets_refuse() {
        let (ledger, _id) = ledger_memoire();
        let err = ledger.resource_revision(b"trop-court").unwrap_err();
        assert!(err.contains("NodeId big-endian de 8 octets"), "vu : {err}");
    }

    // 14) résolution depuis l'identité SEULE (sans ressource) : opérationnelle.
    #[test]
    fn resolution_depuis_identite_seule() {
        let (ledger, id) = ledger_memoire();
        let rid = ressource_bytes(id);
        let def = b"def-1".to_vec();
        let eff = effet_revision(id, 7);
        ledger.apply_event(&oid(1), &rid, &def, 0, &eff, 1).unwrap();
        match ledger.resolve(&oid(1)).unwrap() {
            ResolvedState::Applied {
                definition,
                state,
                revision,
            } => {
                assert_eq!(definition, def);
                assert_eq!(state, eff);
                assert_eq!(revision, 1);
            }
            other => panic!("attendu Applied, reçu {other:?}"),
        }
        // Identité inconnue : NotApplied (une absence n'est pas une erreur).
        assert_eq!(ledger.resolve(&oid(999)).unwrap(), ResolvedState::NotApplied);
    }

    // 15) abandon depuis l'identité SEULE (sans ressource) : opérationnel.
    #[test]
    fn abandon_depuis_identite_seule() {
        let (ledger, id) = ledger_memoire();
        let rid = ressource_bytes(id);
        // Abandon d'une identité neuve : accepté.
        assert_eq!(ledger.abandon(&oid(7)).unwrap(), AbandonOutcome::Abandoned);
        // Rejeu d'une identité abandonnée : Abandoned.
        assert_eq!(
            ledger
                .apply_event(&oid(7), &rid, b"d7", 0, &effet_revision(id, 7), 1)
                .unwrap(),
            LedgerOutcome::Abandoned
        );
        // Abandon d'une appliquée : refusé (une appliquée reste appliquée).
        ledger
            .apply_event(&oid(8), &rid, b"d8", 0, &effet_revision(id, 8), 1)
            .unwrap();
        assert_eq!(
            ledger.abandon(&oid(8)).unwrap(),
            AbandonOutcome::AlreadyApplied { revision: 1 }
        );
    }

    // 16) complete_formation opérationnelle : PENDING → DONE, puis idempotente.
    #[test]
    fn complete_formation_operationnelle() {
        let (ledger, _id) = ledger_memoire();
        let k = oid(100);
        let res = ressource_formation(100);
        let def = b"def-F".to_vec();
        let out = ledger
            .apply_event(&k, &res, &def, 0, &[EFFECT_FORMATION_PENDING], 1)
            .unwrap();
        assert_eq!(out, LedgerOutcome::Applied);
        assert!(ledger.complete_formation(&k).unwrap(), "PENDING → DONE");
        match ledger.resolve(&k).unwrap() {
            ResolvedState::Applied { state, .. } => {
                assert_eq!(state, vec![EFFECT_FORMATION_DONE])
            }
            other => panic!("attendu Applied, reçu {other:?}"),
        }
        assert!(!ledger.complete_formation(&k).unwrap(), "déjà DONE : pas de transition");
        assert!(!ledger.complete_formation(&oid(101)).unwrap(), "identité inconnue : pas de transition");
    }

    // 17) formation : rejeu idempotent + conflit d'identité, aucune création double.
    #[test]
    fn formation_rejeu_idempotent() {
        let (ledger, _id) = ledger_memoire();
        let k = oid(102);
        let res = ressource_formation(102);
        let def = b"def-F".to_vec();
        assert_eq!(
            ledger
                .apply_event(&k, &res, &def, 0, &[EFFECT_FORMATION_PENDING], 1)
                .unwrap(),
            LedgerOutcome::Applied
        );
        match ledger
            .apply_event(&k, &res, &def, 0, &[EFFECT_FORMATION_PENDING], 1)
            .unwrap()
        {
            LedgerOutcome::AlreadyApplied { state, .. } => {
                assert_eq!(state, vec![EFFECT_FORMATION_PENDING])
            }
            other => panic!("attendu AlreadyApplied, reçu {other:?}"),
        }
        match ledger
            .apply_event(&k, &res, b"def-AUTRE", 0, &[EFFECT_FORMATION_PENDING], 1)
            .unwrap()
        {
            LedgerOutcome::IdentityConflict {
                recorded_definition,
            } => assert_eq!(recorded_definition, def),
            other => panic!("attendu IdentityConflict, reçu {other:?}"),
        }
    }

    // 18) formation : redémarrage — la réservation canonique survit.
    #[test]
    fn formation_redemarrage() {
        let td = tempfile::tempdir().unwrap();
        let chemin = td.path().join("kb");
        let k = oid(103);
        let res = ressource_formation(103);
        let def = b"def-F".to_vec();
        {
            let store = Arc::new(SubstrateStore::create(&chemin).unwrap());
            let ledger = SubstrateAtomicEventLedger::new(store.clone());
            ledger
                .apply_event(&k, &res, &def, 0, &[EFFECT_FORMATION_PENDING], 1)
                .unwrap();
            ledger.complete_formation(&k).unwrap();
            store.close().unwrap();
        }
        let store = Arc::new(SubstrateStore::open(&chemin).unwrap());
        let ledger = SubstrateAtomicEventLedger::new(store);
        match ledger.resolve(&k).unwrap() {
            ResolvedState::Applied {
                definition,
                state,
                revision,
            } => {
                assert_eq!(definition, def);
                assert_eq!(state, vec![EFFECT_FORMATION_DONE]);
                assert_eq!(revision, 1);
            }
            other => panic!("attendu Applied, reçu {other:?}"),
        }
    }

    // 19) formation : interruption avant confirmation — reste PENDING, le rejeu
    //     réactive (jamais « déjà fait » alors que l'apprentissage est absent).
    #[test]
    fn formation_interruption_avant_confirmation() {
        let (ledger, _id) = ledger_memoire();
        let k = oid(104);
        let res = ressource_formation(104);
        let def = b"def-F".to_vec();
        ledger
            .apply_event(&k, &res, &def, 0, &[EFFECT_FORMATION_PENDING], 1)
            .unwrap();
        // complete_formation jamais appelé : resolve rend PENDING.
        match ledger.resolve(&k).unwrap() {
            ResolvedState::Applied { state, .. } => {
                assert_eq!(state, vec![EFFECT_FORMATION_PENDING])
            }
            other => panic!("attendu Applied, reçu {other:?}"),
        }
        match ledger
            .apply_event(&k, &res, &def, 0, &[EFFECT_FORMATION_PENDING], 1)
            .unwrap()
        {
            LedgerOutcome::AlreadyApplied { state, .. } => {
                assert_eq!(state, vec![EFFECT_FORMATION_PENDING])
            }
            other => panic!("attendu AlreadyApplied, reçu {other:?}"),
        }
    }

    // 20) formation : abandon d'une identité neuve bloque la formation.
    #[test]
    fn formation_abandon() {
        let (ledger, _id) = ledger_memoire();
        let k = oid(105);
        let res = ressource_formation(105);
        assert_eq!(ledger.abandon(&k).unwrap(), AbandonOutcome::Abandoned);
        assert_eq!(
            ledger
                .apply_event(&k, &res, b"def-F", 0, &[EFFECT_FORMATION_PENDING], 1)
                .unwrap(),
            LedgerOutcome::Abandoned
        );
    }

    // 21) formation : concurrence — une seule réservation canonique.
    #[test]
    fn formation_concurrente() {
        let (ledger, _id) = ledger_memoire();
        let ledger = Arc::new(ledger);
        let k = oid(106);
        let res = ressource_formation(106);
        let def = b"def-F".to_vec();
        let t1 = {
            let ledger = Arc::clone(&ledger);
            let res = res.clone();
            let def = def.clone();
            std::thread::spawn(move || {
                ledger
                    .apply_event(&k, &res, &def, 0, &[EFFECT_FORMATION_PENDING], 1)
                    .unwrap()
            })
        };
        let t2 = {
            let ledger = Arc::clone(&ledger);
            let res = res.clone();
            let def = def.clone();
            std::thread::spawn(move || {
                ledger
                    .apply_event(&k, &res, &def, 0, &[EFFECT_FORMATION_PENDING], 1)
                    .unwrap()
            })
        };
        let out1 = t1.join().unwrap();
        let out2 = t2.join().unwrap();
        let n_applied = [&out1, &out2]
            .iter()
            .filter(|o| matches!(o, LedgerOutcome::Applied))
            .count();
        let n_rejoues = [&out1, &out2]
            .iter()
            .filter(|o| matches!(o, LedgerOutcome::AlreadyApplied { .. }))
            .count();
        assert_eq!(n_applied, 1, "exactement une application (out1={out1:?} out2={out2:?})");
        assert_eq!(n_rejoues, 1, "l'autre est un rejeu");
    }

    // 22) déduplication GLOBALE inter-ressources (miroir unitaire du scénario S1).
    #[test]
    fn deduplication_globale_inter_ressources() {
        let store = Arc::new(SubstrateStore::open_memory().unwrap());
        let a = store.create_node(&["ressource"]);
        let b = store.create_node(&["ressource"]);
        let ledger = SubstrateAtomicEventLedger::new(store);
        let k = oid(107);
        let def = b"def-D".to_vec();
        let out_a = ledger
            .apply_event(&k, &ressource_bytes(a), &def, 0, b"eff-A", 1)
            .unwrap();
        let out_b = ledger
            .apply_event(&k, &ressource_bytes(b), &def, 0, b"eff-B", 1)
            .unwrap();
        assert_eq!(out_a, LedgerOutcome::Applied);
        // Même K + même définition sur une AUTRE ressource : rejeu (l'effet
        // enregistré est celui de A), PAS une seconde application.
        assert_eq!(
            out_b,
            LedgerOutcome::AlreadyApplied {
                state: b"eff-A".to_vec(),
                revision: 1
            }
        );
        assert_eq!(ledger.resource_revision(&ressource_bytes(a)).unwrap(), 1);
        assert_eq!(
            ledger.resource_revision(&ressource_bytes(b)).unwrap(),
            0,
            "B jamais écrite"
        );
    }

    // 23) FRONTIÈRE D'ENGAGEMENT — entrelacement DÉTERMINISTE réel : deux K
    //     distincts sur la MÊME révision. K2 commet son identité puis SUSPEND
    //     (crochet test-only) ; K1 matérialise pendant la suspension ; K2 bute
    //     sur la révision → resolve(K2) = CommittedNotMaterialized (jamais
    //     Applied), la ressource ne porte QUE l'effet de K1.
    #[test]
    fn entrelacement_deterministe_deux_k_meme_revision() {
        let (ledger, id) = ledger_memoire();
        let ledger = Arc::new(ledger);
        let rid = ressource_bytes(id);
        let def1 = b"def-K1".to_vec();
        let eff1 = effet_revision(id, 1);
        let def2 = b"def-K2".to_vec();
        let eff2 = effet_revision(id, 2);

        // K2 s'engage en arrière-plan : elle commettra son identité puis
        // SUSPENDRA juste avant la matérialisation.
        ledger.armer_suspension_engagement();
        let t2 = {
            let ledger = Arc::clone(&ledger);
            let rid = rid.clone();
            let def2 = def2.clone();
            let eff2 = eff2.clone();
            std::thread::spawn(move || {
                ledger
                    .apply_event(&oid(2), &rid, &def2, 0, &eff2, 1)
                    .unwrap()
            })
        };

        // Attendre que K2 ait COMMIT son identité (suspendue à la frontière).
        ledger.attendre_suspension_engagement();

        // K1 matérialise DÉTERMINISTEMENT pendant la suspension de K2.
        let out1 = ledger.apply_event(&oid(1), &rid, &def1, 0, &eff1, 1).unwrap();
        assert_eq!(out1, LedgerOutcome::Applied);

        // Relâcher K2 : sa matérialisation échoue (révision avancée), mais son
        // identité est déjà COMMISE.
        ledger.relacher_suspension_engagement();
        let out2 = t2.join().unwrap();
        assert_eq!(out2, LedgerOutcome::PreconditionFailed { current_revision: 1 });

        // NORME : K2 = engagée non matérialisée, JAMAIS Applied.
        match ledger.resolve(&oid(2)).unwrap() {
            ResolvedState::CommittedNotMaterialized {
                definition,
                state,
                revision,
            } => {
                assert_eq!(definition, def2);
                assert_eq!(state, eff2);
                assert_eq!(revision, 1);
            }
            other => panic!("K2 attendue CommittedNotMaterialized, reçu {other:?}"),
        }
        // resolve_on a la MÊME classification.
        assert!(matches!(
            ledger.resolve_on(&rid, &oid(2)).unwrap(),
            ResolvedState::CommittedNotMaterialized { .. }
        ));

        // La ressource ne porte QUE l'effet de K1.
        assert_eq!(ledger.resource_revision(&rid).unwrap(), 1);
        assert_eq!(ledger.resource_state(&rid).unwrap(), Some((eff1, 1)));

        // K1 reste Matérialisée.
        assert!(matches!(
            ledger.resolve(&oid(1)).unwrap(),
            ResolvedState::Applied { revision: 1, .. }
        ));
    }

    // 24) RÉFÉRENCE DE MATÉRIALISATION PERSÉVÉRÉE avec l'identité : un arrêt
    //     entre commit d'identité et matérialisation ne la perd pas — elle est
    //     relue dans l'enregistrement canonique.
    #[test]
    fn reference_materialisation_perseveree_avec_identite() {
        let (ledger, id) = ledger_memoire();
        let rid = ressource_bytes(id);
        // K1 matérialisée (révision 0 -> 1).
        ledger
            .apply_event(&oid(1), &rid, b"def-K1", 0, &effet_revision(id, 1), 1)
            .unwrap();
        // K2 : identité commise (révision résultante 2) — on simule l'arrêt
        // entre le commit d'identité et la matérialisation en remettant la
        // projection ressource à l'état post-K1.
        ledger
            .apply_event(&oid(2), &rid, b"def-K2", 1, &effet_revision(id, 2), 2)
            .unwrap();
        ledger
            .store()
            .set_node_property(id, "obs:revision", Value::from(1i64));
        ledger
            .store()
            .set_node_property(id, "obs:effect", Value::from(effet_revision(id, 1)));

        // L'enregistrement d'identité de K2 porte la référence de matérialisation.
        let (prefixe, payload) = ledger.lire_record(&oid(2)).unwrap().unwrap();
        assert_eq!(prefixe, RECORD_APPLIQUE);
        let reference = payload
            .expect("charge présente")
            .reference
            .expect("référence persévérée avec l'identité");
        // La référence pointe le nœud ressource et les deux clés de projection.
        assert_eq!(reference.cible(), id);
        assert_eq!(
            reference.effets(),
            &[CLE_REVISION.to_string(), CLE_EFFET.to_string()]
        );
    }

    // 25) RÉCUPÉRATION : `CommittedNotMaterialized` restitue l'effet enregistré ;
    //     une re-matérialisation (via le manager, simulée ici) fait repasser
    //     `resolve` à `Applied`, et le rejeu n'applique JAMAIS deux fois.
    #[test]
    fn engage_non_materialise_recuperation_sans_double_application() {
        let (ledger, id) = ledger_memoire();
        let rid = ressource_bytes(id);
        let def1 = b"def-K1".to_vec();
        let eff1 = effet_revision(id, 1);
        let def2 = b"def-K2".to_vec();
        let eff2 = effet_revision(id, 2);
        ledger.apply_event(&oid(1), &rid, &def1, 0, &eff1, 1).unwrap();
        ledger.apply_event(&oid(2), &rid, &def2, 1, &eff2, 2).unwrap();
        // Simule l'arrêt entre commit d'identité (K2) et matérialisation.
        ledger.store().set_node_property(id, "obs:revision", Value::from(1i64));
        ledger.store().set_node_property(id, "obs:effect", Value::from(eff1.clone()));

        // K2 = engagée non matérialisée, avec l'effet restitué.
        match ledger.resolve(&oid(2)).unwrap() {
            ResolvedState::CommittedNotMaterialized { state, revision, .. } => {
                assert_eq!(state, eff2);
                assert_eq!(revision, 2);
            }
            other => panic!("attendu CommittedNotMaterialized, reçu {other:?}"),
        }

        // Récupération (simulée : le manager re-matérialise l'effet enregistré).
        ledger.store().set_node_property(id, "obs:revision", Value::from(2i64));
        ledger.store().set_node_property(id, "obs:effect", Value::from(eff2.clone()));
        assert!(matches!(
            ledger.resolve(&oid(2)).unwrap(),
            ResolvedState::Applied { revision: 2, .. }
        ));

        // Le rejeu n'applique pas deux fois (l'effet est déjà là).
        let rejeu = ledger.apply_event(&oid(2), &rid, &def2, 1, &eff2, 2).unwrap();
        assert!(matches!(rejeu, LedgerOutcome::AlreadyApplied { revision: 2, .. }));
        assert_eq!(ledger.resource_revision(&rid).unwrap(), 2, "pas de double application");
    }
}
