//! Ledger d'observation DURABLE, adossé à l'autorité conditionnelle native du
//! substrat (`capturer_precondition` / `preparer_transformation` /
//! `publier_si_precondition_intacte` / `resoudre_reference`).
//!
//! # Ce qu'il remplace
//!
//! [`super::observation::InMemoryAtomicEventLedger`] est un mock VOLATIL : il
//! prouve le contrat (atomicité/visibilité sous un mutex) mais aucune
//! DURABILITÉ. [`SubstrateAtomicEventLedger`] porte le même trait
//! [`super::observation::AtomicEventLedger`] sur l'autorité transactionnelle
//! NATIVE du substrat, sans second registre : tout est des propriétés d'UN
//! nœud substrat (la ressource), et chaque application est UNE tentative
//! `capturer → préparer → publier_si_precondition_intacte` en mode STRICT.
//!
//! # Lien atomique exigé (une même tentative)
//!
//! Sur le nœud de la ressource, une application écrit ATOMIQUEMENT (même
//! bascule de tête) :
//! * `obs:revision` → `Int64` : la révision résultante (précondition = la
//!   révision ATTENDUE relue au même instant) ;
//! * `obs:effect` → `Bytes` : l'effet (état résultant) du dernier événement —
//!   c'est ce que `resource_state` rend pour rattraper un store en retard ;
//! * `obs:<ObservationId en hex>` → `Bytes` : l'enregistrement par identité
//!   (définition + effet + révision), qui fait preuve d'égalité au rejeu et
//!   de résolution par identité.
//!
//! L'`ObservationId` EST la clé de propriété ; la `ObservationDefinition`, la
//! ressource, la révision attendue, l'état résultant et les effets sont DANS
//! l'enregistrement et les propriétés écrites ; le « résultat de publication »
//! est la garantie du mode STRICT (`PublieIntegrale` : tous les effets écrits
//! sont canoniques, aucun effet partiel masqué).
//!
//! # Limites déclarées (à qualifier, pas à deviner)
//!
//! 1. **UNE ressource = UN nœud.** `publier_si_precondition_intacte` couvre un
//!    nœud + sa PropsZone. Ce registre n'offre PAS d'atomicité multi-ressources :
//!    une mutation cognitive qui touche plusieurs ressources devra définir un
//!    contrat multi-nœuds (hors du présent fichier).
//! 2. **`resolve` / `abandon` / `complete_formation` du trait sont SANS
//!    ressource**, alors que l'autorité native est PAR-RESSOURCE (par nœud).
//!    Le registre fournit les variantes SCOPÉES (`resolve_on`, `abandon_on`) ;
//!    les méthodes du trait rendent une erreur NOMMÉE (jamais un `None`
//!    silencieux, jamais une ressource devinée). C'est le contrat manquant à
//!    définir avec le secondaire (adressage ressource-scopé de la résolution).
//! 3. **Formations hors portée.** `apply_formation` cible des ressources
//!    arbitraires (ex. `b"F:obs1"`), pas un `NodeId` de 8 octets. Le registre
//!    durable ne couvre QUE le chemin de RÉVISION (ressource = nœud substrat).
//! 4. **La durabilité suit le contrat de la publication native** : la bascule
//!    de tête est WAL-journalisée (`NodePropHeadUpdate`), le contenu des pages
//!    privées suit le flush/checkpoint de la PropsZone (même contrat que
//!    `set_node_property` v2 — la publication n'est pas un fsync).

use std::sync::Arc;

use obrain_common::types::{NodeId, Value};
use obrain_substrate::{
    EtatPropriete, IssuePublication, ModePublication, ReferenceTentative, SubstrateStore,
};

use super::observation::{AbandonOutcome, AtomicEventLedger, LedgerOutcome, ResolvedState};

/// Clé de la révision courante d'une ressource.
const CLE_REVISION: &str = "obs:revision";
/// Clé du dernier effet appliqué (état résultant sérialisé, préfixé).
const CLE_EFFET: &str = "obs:effect";
/// Préfixe de clé d'un enregistrement par identité d'observation.
const CLE_EVENEMENT_PREFIXE: &str = "obs:";

/// Préfixe d'enregistrement : appliquée (définition + effet + révision).
const RECORD_APPLIQUE: u8 = 0x01;
/// Préfixe d'enregistrement : abandonnée (jamais appliquée, ne le sera pas).
const RECORD_ABANDONNE: u8 = 0x02;

/// Nombre maximal de ré-évaluations en cas de `ConflitPrecondition` (course
/// capture→publication). Au-delà, on rend `Indeterminate` : l'appelant résout.
const NB_REESSAIS: usize = 8;

/// Registre d'observations DURABLE, un nœud substrat par ressource.
pub struct SubstrateAtomicEventLedger {
    store: Arc<SubstrateStore>,
}

/// Décode un `resource_id` opaque en `NodeId` : 8 octets big-endian (le format
/// produit par `engram_id.0.to_be_bytes()`). Toute autre longueur est refusée
/// par un nom précis, jamais interprétée.
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

fn encode_record_applique(definition: &[u8], effet: &[u8], revision: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + 4 + definition.len() + 4 + effet.len() + 8);
    out.push(RECORD_APPLIQUE);
    out.extend_from_slice(&(definition.len() as u32).to_le_bytes());
    out.extend_from_slice(definition);
    out.extend_from_slice(&(effet.len() as u32).to_le_bytes());
    out.extend_from_slice(effet);
    out.extend_from_slice(&revision.to_le_bytes());
    out
}

/// Décode un enregistrement « appliquée ». Refus nommé, jamais de panique.
fn decode_record_applique(record: &[u8]) -> Result<(Vec<u8>, Vec<u8>, u64), String> {
    if record.first() != Some(&RECORD_APPLIQUE) {
        return Err("enregistrement d'événement sans préfixe « appliquée »".to_string());
    }
    let mut pos = 1usize;
    let mut take = |n: usize| -> Result<&[u8], String> {
        let fin = pos.checked_add(n).ok_or("débordement de taille")?;
        let tranche = record
            .get(pos..fin)
            .ok_or("enregistrement d'événement tronqué")?;
        pos = fin;
        Ok(tranche)
    };
    let def_len = u32::from_le_bytes(take(4)?.try_into().unwrap()) as usize;
    let definition = take(def_len)?.to_vec();
    let eff_len = u32::from_le_bytes(take(4)?.try_into().unwrap()) as usize;
    let effet = take(eff_len)?.to_vec();
    let revision = u64::from_le_bytes(take(8)?.try_into().unwrap());
    Ok((definition, effet, revision))
}

fn err_substrat(e: obrain_substrate::SubstrateError) -> String {
    e.to_string()
}

impl SubstrateAtomicEventLedger {
    /// Construit le registre au-dessus d'un [`SubstrateStore`] partagé.
    pub fn new(store: Arc<SubstrateStore>) -> Self {
        Self { store }
    }

    /// Accès au store sous-jacent (tests, diagnostics, résolution par
    /// [`SubstrateStore::resoudre_reference`]).
    pub fn store(&self) -> &SubstrateStore {
        &self.store
    }

    /// Applique un événement et rend, en plus de l'issue, la
    /// [`ReferenceTentative`] NATIVE de la tentative (si une publication a eu
    /// lieu). Cette référence permet de résoudre une réponse PERDUE via
    /// [`SubstrateStore::resoudre_reference`] (consultation historique), sans
    /// rejouer.
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
        let cible = decoder_cible(resource_id)?;
        // Ressource inexistante : refus NOMÉ (le registre durable exige un
        // nœud substrat par ressource), pas un état fantôme.
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
        let cle_ev = cle_evenement(event_id);
        // Ordre des clés : les trois sont ÉCRITES, donc toutes déclarées en
        // dépendances (mode STRICT : W ⊆ R).
        let cles = [CLE_REVISION, CLE_EFFET, cle_ev.as_str()];

        for _ in 0..NB_REESSAIS {
            let pre = self
                .store
                .capturer_precondition(cible, &cles)
                .map_err(err_substrat)?;

            // 1) Déduplication par identité (AVANT la précondition, comme le
            //    registre de référence) : rejeu / conflit d'identité / abandon.
            if let Some(valeur) = pre.valeur_capturee(&cle_ev) {
                let record = valeur
                    .as_bytes()
                    .ok_or("enregistrement d'événement non-binaire")?;
                return match record.first() {
                    Some(&RECORD_APPLIQUE) => {
                        let (def_enregistree, effet, rev) = decode_record_applique(record)?;
                        if def_enregistree == definition {
                            Ok((
                                LedgerOutcome::AlreadyApplied {
                                    state: effet,
                                    revision: rev,
                                },
                                None,
                            ))
                        } else {
                            Ok((
                                LedgerOutcome::IdentityConflict {
                                    recorded_definition: def_enregistree,
                                },
                                None,
                            ))
                        }
                    }
                    Some(&RECORD_ABANDONNE) => Ok((LedgerOutcome::Abandoned, None)),
                    _ => Err("enregistrement d'événement invalide (préfixe inconnu)".to_string()),
                };
            }

            // 2) Précondition de révision.
            let courant = pre
                .valeur_capturee(CLE_REVISION)
                .and_then(|v| v.as_int64())
                .map(|v| v as u64)
                .unwrap_or(0);
            if courant != precondition_revision {
                return Ok((
                    LedgerOutcome::PreconditionFailed {
                        current_revision: courant,
                    },
                    None,
                ));
            }

            // 3) Transformation atomique : identité + définition + état + effet
            //    + révision, dans une même bascule (mode STRICT).
            let record = encode_record_applique(definition, resulting_state, resulting_revision);
            let nouvelles = [
                (CLE_REVISION, Value::from(resulting_revision as i64)),
                (CLE_EFFET, Value::from(resulting_state.to_vec())),
                (cle_ev.as_str(), Value::from(record)),
            ];
            let prep = self
                .store
                .preparer_transformation(cible, &nouvelles, &pre, ModePublication::Strict)
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
                // Le mode STRICT établit la canonicité AVANT la bascule : un
                // effet partiel y est impossible. On le signale, jamais avalé.
                IssuePublication::PubliePartielle => {
                    return Ok((LedgerOutcome::Indeterminate, Some(reference)));
                }
                IssuePublication::ConflitPrecondition => {
                    // Écriture concurrente intercalée : re-capturer et
                    // ré-évaluer (boucle bornée).
                    continue;
                }
            }
        }
        // Conflit persistant : indéterminé, l'appelant résout au lieu de rejouer.
        Ok((LedgerOutcome::Indeterminate, None))
    }

    /// Résolution SCOPÉE par ressource : interroge l'enregistrement durable de
    /// l'identité, sans deviner ni rejouer.
    pub fn resolve_on(&self, resource_id: &[u8], event_id: &[u8; 16]) -> Result<ResolvedState, String> {
        let cible = decoder_cible(resource_id)?;
        let cle_ev = cle_evenement(event_id);
        match self
            .store
            .lire_etat_propriete(cible, &cle_ev)
            .map_err(err_substrat)?
        {
            EtatPropriete::Presente { valeur, .. } => {
                let record = valeur
                    .as_bytes()
                    .ok_or("enregistrement d'événement non-binaire")?;
                match record.first() {
                    Some(&RECORD_APPLIQUE) => {
                        let (definition, state, revision) = decode_record_applique(record)?;
                        Ok(ResolvedState::Applied {
                            definition,
                            state,
                            revision,
                        })
                    }
                    Some(&RECORD_ABANDONNE) => Ok(ResolvedState::Abandoned),
                    _ => Err("enregistrement d'événement invalide (préfixe inconnu)".to_string()),
                }
            }
            // Absente / supprimée / ressource inexistante : jamais appliquée.
            _ => Ok(ResolvedState::NotApplied),
        }
    }

    /// Abandon conditionnel SCOPÉ par ressource (refusé si déjà appliquée).
    pub fn abandon_on(&self, resource_id: &[u8], event_id: &[u8; 16]) -> Result<AbandonOutcome, String> {
        let cible = decoder_cible(resource_id)?;
        let cle_ev = cle_evenement(event_id);
        for _ in 0..NB_REESSAIS {
            let pre = self
                .store
                .capturer_precondition(cible, &[cle_ev.as_str()])
                .map_err(err_substrat)?;
            match pre.valeur_capturee(&cle_ev) {
                Some(valeur) => {
                    let record = valeur
                        .as_bytes()
                        .ok_or("enregistrement d'événement non-binaire")?;
                    return match record.first() {
                        Some(&RECORD_APPLIQUE) => {
                            let (_, _, revision) = decode_record_applique(record)?;
                            Ok(AbandonOutcome::AlreadyApplied { revision })
                        }
                        Some(&RECORD_ABANDONNE) => Ok(AbandonOutcome::Abandoned),
                        _ => Err("enregistrement d'événement invalide (préfixe inconnu)".to_string()),
                    };
                }
                None => {
                    let nouvelles = [(cle_ev.as_str(), Value::from(vec![RECORD_ABANDONNE]))];
                    let prep = self
                        .store
                        .preparer_transformation(cible, &nouvelles, &pre, ModePublication::Strict)
                        .map_err(err_substrat)?;
                    match self
                        .store
                        .publier_si_precondition_intacte(prep)
                        .map_err(err_substrat)?
                    {
                        IssuePublication::PublieIntegrale => return Ok(AbandonOutcome::Abandoned),
                        IssuePublication::PubliePartielle => {
                            return Err("abandon : publication partielle (inattendu en mode strict)".to_string());
                        }
                        IssuePublication::ConflitPrecondition => continue,
                    }
                }
            }
        }
        Err("abandon : conflit persistant (résoudre au lieu de rejouer)".to_string())
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
            EtatPropriete::Presente { valeur, .. } => Ok(valeur.as_int64().map(|v| v as u64).unwrap_or(0)),
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
            EtatPropriete::Presente { valeur, .. } => valeur.as_int64().map(|v| v as u64).unwrap_or(0),
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

    fn resolve(&self, _event_id: &[u8; 16]) -> Result<ResolvedState, String> {
        Err(
            "resolve() SANS ressource : l'autorité native est PAR-RESSOURCE (un nœud substrat). \
             Utilisez SubstrateAtomicEventLedger::resolve_on(resource_id, event_id)."
                .to_string(),
        )
    }

    fn abandon(&self, _event_id: &[u8; 16]) -> Result<AbandonOutcome, String> {
        Err(
            "abandon() SANS ressource : l'autorité native est PAR-RESSOURCE (un nœud substrat). \
             Utilisez SubstrateAtomicEventLedger::abandon_on(resource_id, event_id)."
                .to_string(),
        )
    }

    fn complete_formation(&self, _event_id: &[u8; 16]) -> Result<bool, String> {
        Err(
            "complete_formation() : les FORMATIONS ciblent des ressources arbitraires \
             (non-NodeId) et ne sont pas couvertes par le registre durable par-ressource. \
             Contrat manquant à définir (ressource-nœud pour la formation)."
                .to_string(),
        )
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

    // 14) les méthodes du trait SANS ressource rendent une erreur NOMMÉE
    //     (le contrat ressource-scopé manque), jamais un état deviné.
    #[test]
    fn methodes_sans_ressource_nomment_le_contrat_manquant() {
        let (ledger, _id) = ledger_memoire();
        assert!(ledger.resolve(&oid(1)).unwrap_err().contains("PAR-RESSOURCE"));
        assert!(ledger.abandon(&oid(1)).unwrap_err().contains("PAR-RESSOURCE"));
        assert!(ledger.complete_formation(&oid(1)).unwrap_err().contains("FORMATION"));
    }
}
