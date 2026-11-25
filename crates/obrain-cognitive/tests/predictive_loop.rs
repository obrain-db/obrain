//! Chantier C — première boucle prédictive RÉELLEMENT EXÉCUTÉE sur le graphe
//! natif (engrammes + substrate). Chaque pas :
//!   1. ENREGISTRE la prédiction AVANT l'observation ;
//!   2. OBSERVE l'épisode de co-activation ;
//!   3. MESURE l'erreur (fraction des nœuds observés absents de la prédiction) ;
//!   4. APPLIQUE la mise à jour (reconsolidation : les nœuds manquants entrent
//!      avec un poids proportionnel à l'erreur) ;
//!   5. VÉRIFIE que l'erreur décroît puis s'annule une fois le motif appris.
//!
//! Le corpus est le motif croissant {A,B} -> {A,B,C} -> {A,B,C,D} : un prédicteur
//! non appris échoue, puis apprend l'ensemble co-activé par reconsolidation.

#![cfg(all(feature = "engram", feature = "substrate"))]

use std::collections::HashSet;
use std::sync::Arc;

use obrain_cognitive::engram::{reconsolidate, EngramStore};
use obrain_common::types::NodeId;
use obrain_substrate::{SubstrateFile, SyncMode, Writer as SubstrateWriter};

fn make_writer() -> (Arc<SubstrateWriter>, tempfile::TempDir) {
    let td = tempfile::tempdir().unwrap();
    let sub = SubstrateFile::create(td.path().join("kb")).unwrap();
    let w = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
    (Arc::new(w), td)
}

/// Prédit l'ensemble co-activé à partir d'un indice : l'ensemble du plus grand
/// engramme rappelé contenant l'indice.
fn predict(store: &EngramStore, cue: NodeId) -> HashSet<NodeId> {
    let recalled = store.recall(cue).unwrap_or_default();
    let mut best: Option<Vec<NodeId>> = None;
    for eid in recalled {
        if let Some(e) = store.get(eid) {
            let members: Vec<NodeId> = e.ensemble.iter().map(|(n, _)| *n).collect();
            if best.as_ref().map_or(true, |b| members.len() > b.len()) {
                best = Some(members);
            }
        }
    }
    best.map(|v| v.into_iter().collect()).unwrap_or_default()
}

#[test]
fn boucle_predictive_reellement_executee() {
    let store = EngramStore::new(None);
    let (writer, _td) = make_writer();
    store.attach_substrate(writer);

    let a = NodeId(1);
    let b = NodeId(2);
    let c = NodeId(3);
    let d = NodeId(4);

    let episodes: Vec<Vec<NodeId>> = vec![
        vec![a, b],
        vec![a, b, c],
        vec![a, b, c, d],
        vec![a, b, c, d],
    ];

    let mut errors: Vec<f64> = Vec::new();
    let mut last_eid = None;

    for episode in &episodes {
        // 1. Prédiction AVANT observation.
        let predicted = predict(&store, a);

        // 2. Observation.
        let observed: HashSet<NodeId> = episode.iter().copied().collect();

        // 3. Erreur : nœuds observés absents de la prédiction.
        let missing: Vec<NodeId> = observed.difference(&predicted).copied().collect();
        let error = missing.len() as f64 / observed.len() as f64;
        errors.push(error);

        // 4. Mise à jour.
        if !missing.is_empty() {
            match last_eid {
                Some(eid) => store.update(eid, |e| reconsolidate(e, &missing, error)),
                None => {
                    let eid = store.form(episode).expect("form du premier épisode");
                    last_eid = Some(eid);
                }
            }
        }
    }

    // 5. Vérification : l'erreur décroît puis s'annule.
    assert!(
        errors[0] > errors[1],
        "erreur décroît après la 1re reconsolidation ({} -> {})",
        errors[0],
        errors[1]
    );
    assert!(
        errors[1] > errors[2],
        "erreur décroît après la 2e reconsolidation ({} -> {})",
        errors[1],
        errors[2]
    );
    assert_eq!(errors[3], 0.0, "prédiction exacte une fois le motif appris");
}
