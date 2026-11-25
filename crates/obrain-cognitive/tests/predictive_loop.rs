//! Chantier C — boucle prédictive exécutée sur le graphe natif (engrammes +
//! substrate), AVEC métriques complètes et baselines.
//!
//! Contrôle positif conservé : le motif croissant {A,B} -> {A,B,C} -> {A,B,C,D}
//! voit son taux de manque décroître 1.0 -> 0.333 -> 0.25 -> 0.0.
//!
//! Critique traitée : la métrique |O\P|/|O| ne pénalise que les éléments
//! MANQUANTS. Un prédicteur qui énonce tout obtient 0. On ajoute précision,
//! rappel, Jaccard et « superflu » (proportion de prédictions hors observation),
//! et on compare à des baselines. Résultat honnête : sur les séquences
//! décroissante/alternée, la reconsolidation additive rejoint l'union cumulative
//! — aucun bénéfice cognitif propre n'est encore démontré.
//!
//! Contexte observable : un marqueur de contexte (nœud) est fourni AVANT la
//! prédiction, à toutes les stratégies. Il permet de distinguer A→B (contexte 1)
//! de A→C (contexte 2) ; l'ordre des contextes est varié (pas une simple
//! alternance temporelle).

#![cfg(all(feature = "engram", feature = "substrate"))]
// Noms à un caractère = notation A/B/C/D de l'expérience cognitive (lisible).
#![allow(clippy::many_single_char_names)]

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use obrain_cognitive::engram::{EngramStore, reconsolidate};
use obrain_common::types::NodeId;
use obrain_substrate::{SubstrateFile, SyncMode, Writer as SubstrateWriter};

fn make_writer() -> (Arc<SubstrateWriter>, tempfile::TempDir) {
    let td = tempfile::tempdir().unwrap();
    let sub = SubstrateFile::create(td.path().join("kb")).unwrap();
    let w = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
    (Arc::new(w), td)
}

type Ep = HashSet<NodeId>;

// ---------------------------------------------------------------------------
// Métriques
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
struct Metrics {
    precision: f64, // |P∩O| / |P|
    recall: f64,    // |P∩O| / |O|
    jaccard: f64,   // |P∩O| / |P∪O|
    miss: f64,      // |O\P| / |O|  (éléments manquants)
    superflu: f64,  // |P\O| / |P|  (proportion de prédictions superflues)
}

fn metrics(predicted: &Ep, observed: &Ep) -> Metrics {
    let inter = predicted.intersection(observed).count() as f64;
    let p = predicted.len() as f64;
    let o = observed.len() as f64;
    let union = predicted.union(observed).count() as f64;
    Metrics {
        precision: if p == 0.0 { 0.0 } else { inter / p },
        recall: if o == 0.0 { 1.0 } else { inter / o },
        jaccard: if union == 0.0 { 1.0 } else { inter / union },
        miss: if o == 0.0 { 0.0 } else { (o - inter) / o },
        superflu: if p == 0.0 { 0.0 } else { (p - inter) / p },
    }
}

// ---------------------------------------------------------------------------
// Baselines (non conditionnées par l'indice : elles ne voient que l'historique)
// ---------------------------------------------------------------------------

fn baseline_union(history: &[Ep]) -> Ep {
    history.iter().fold(Ep::new(), |mut acc, e| {
        acc.extend(e);
        acc
    })
}

fn baseline_last(history: &[Ep]) -> Ep {
    history.last().cloned().unwrap_or_default()
}

fn baseline_freq_topk(history: &[Ep], k: usize) -> Ep {
    let mut counts: HashMap<NodeId, usize> = HashMap::new();
    for e in history {
        for &n in e {
            *counts.entry(n).or_insert(0) += 1;
        }
    }
    let mut v: Vec<(NodeId, usize)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.0.cmp(&b.0.0)));
    v.into_iter().take(k).map(|(n, _)| n).collect()
}

fn baseline_fixed(_history: &[Ep]) -> Ep {
    Ep::new()
}

// ---------------------------------------------------------------------------
// Prédicteur adaptatif (engrammes + reconsolidation)
// ---------------------------------------------------------------------------

/// Prédit l'ensemble co-activé à partir d'un indice : l'ensemble du plus grand
/// engramme rappelé contenant l'indice. Aucune erreur de stockage n'est
/// masquée : un échec de rappel remonte (`SubstrateError`).
fn predict(store: &EngramStore, cue: NodeId) -> Result<Ep, obrain_substrate::SubstrateError> {
    let recalled = store.recall(cue)?;
    let mut best: Option<Vec<NodeId>> = None;
    for eid in recalled {
        if let Some(e) = store.get(eid) {
            let members: Vec<NodeId> = e.ensemble.iter().map(|(n, _)| *n).collect();
            if best.as_ref().map_or(true, |b| members.len() > b.len()) {
                best = Some(members);
            }
        }
    }
    Ok(best.map(|v| v.into_iter().collect()).unwrap_or_default())
}

/// Un pas du prédicteur adaptatif : prédit AVANT d'observer, mesure, et
/// reconsolide les nœuds manquants. Le seuil natif de reconsolidation (0.1)
/// s'applique tel quel : `m.miss` est passé tel quel, sans forçage.
#[allow(clippy::too_many_arguments)]
fn adaptive_step(
    store: &EngramStore,
    last_eid: &mut Option<obrain_cognitive::engram::EngramId>,
    cue: NodeId,
    observed: &Ep,
) -> Metrics {
    let predicted = predict(store, cue).expect("rappel engramme a échoué");
    let m = metrics(&predicted, observed);
    let missing: Vec<NodeId> = observed.difference(&predicted).copied().collect();
    if !missing.is_empty() {
        match *last_eid {
            Some(eid) => store.update(eid, |e| reconsolidate(e, &missing, m.miss)),
            None => {
                let all: Vec<NodeId> = observed.iter().copied().collect();
                *last_eid = Some(store.form(&all).expect("form engramme a échoué"));
            }
        }
    }
    m
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Contrôle positif (conservé) : le motif croissant voit le taux de manque
/// décroître puis s'annuler. Rappel par un membre NOUVELLEMENT AJOUTÉ (C, D),
/// pas seulement par l'indice A.
#[test]
fn boucle_predictive_reellement_executee() {
    let store = EngramStore::new(None);
    let (writer, _td) = make_writer();
    store.attach_substrate(writer);

    let (a, b, c, d) = (NodeId(1), NodeId(2), NodeId(3), NodeId(4));
    let episodes = [
        [a, b].into_iter().collect(),
        [a, b, c].into_iter().collect(),
        [a, b, c, d].into_iter().collect(),
        [a, b, c, d].into_iter().collect(),
    ];

    let mut last_eid = None;
    let mut misses = Vec::new();
    for ep in &episodes {
        let m = adaptive_step(&store, &mut last_eid, a, ep);
        misses.push(m.miss);
    }

    assert!(
        misses[0] > misses[1],
        "taux de manque décroît ({} -> {})",
        misses[0],
        misses[1]
    );
    assert!(
        misses[1] > misses[2],
        "taux de manque décroît ({} -> {})",
        misses[1],
        misses[2]
    );
    assert_eq!(misses[3], 0.0, "prédiction exacte une fois le motif appris");

    // Rappel par un membre NOUVELLEMENT AJOUTÉ, pas seulement par l'indice A.
    let eid = last_eid.expect("un engramme a dû être formé");
    for newly in [c, d] {
        let recalled = store.recall(newly).expect("rappel par membre ajouté");
        assert!(
            recalled.contains(&eid),
            "le membre {newly:?} ajouté rappelle l'engramme"
        );
    }
    let members = store.members(eid).expect("lecture des membres");
    assert!(
        members.contains(&c) && members.contains(&d),
        "membres {c:?},{d:?} persistés"
    );
}

/// Une prédiction « tout » obtiendrait miss=0 : la métrique miss seule est
/// aveugle aux prédictions superflues. Jaccard et `superflu` le révèlent.
#[test]
fn metrique_miss_seule_est_aveugle_aux_superflus() {
    let a = NodeId(1);
    let b = NodeId(2);
    let c = NodeId(3);
    let observed: Ep = [a, c].into_iter().collect();
    let tout: Ep = [a, b, c].into_iter().collect();

    let m = metrics(&tout, &observed);
    assert_eq!(m.miss, 0.0, "miss=0 : l'ancienne métrique dit « parfait »");
    assert!(
        (m.jaccard - 2.0 / 3.0).abs() < 1e-9,
        "Jaccard révèle le superflu (2/3)"
    );
    assert!(
        m.superflu > 0.0,
        "proportion de prédictions superflues non nulle"
    );
}

/// Comparaison adaptatif vs baselines sur des séquences non croissantes.
/// On distingue le démarrage à froid (sous-prédiction : miss > 0) de
/// l'après-apprentissage (sur-prédiction : superflu > 0). Une moyenne < 1
/// seule ne prouve rien : elle peut venir d'éléments manquants.
#[test]
fn baselines_et_sequences_difficiles() {
    let (a, b, c, d) = (NodeId(1), NodeId(2), NodeId(3), NodeId(4));
    let ab: Ep = [a, b].into_iter().collect();
    let abc: Ep = [a, b, c].into_iter().collect();
    let abcd: Ep = [a, b, c, d].into_iter().collect();
    let ac: Ep = [a, c].into_iter().collect();

    let decroissante = [ab.clone(), abc.clone(), abcd.clone(), ab.clone()];
    let alternee = [ab.clone(), ac.clone(), ab.clone(), ac.clone()];

    for (name, seq) in [
        ("décroissante", &decroissante[..]),
        ("alternée", &alternee[..]),
    ] {
        let mut h: Vec<Ep> = Vec::new();
        let mut union_m = Vec::new();
        let mut last_m = Vec::new();
        let mut freq_m = Vec::new();
        let mut fixed_m = Vec::new();
        for ep in seq {
            let u = baseline_union(&h);
            union_m.push(metrics(&u, ep));
            last_m.push(metrics(&baseline_last(&h), ep));
            freq_m.push(metrics(&baseline_freq_topk(&h, 2), ep));
            fixed_m.push(metrics(&baseline_fixed(&h), ep));
            h.push(ep.clone());
        }

        let store = EngramStore::new(None);
        let (writer, _td) = make_writer();
        store.attach_substrate(writer);
        let mut last_eid = None;
        let mut ad_m = Vec::new();
        for ep in seq {
            ad_m.push(adaptive_step(&store, &mut last_eid, a, ep));
        }

        // Démarrage à froid (pas 0) : sous-prédiction, tout est manquant.
        assert!(
            union_m[0].miss > 0.0,
            "[{name}] démarrage à froid : miss = {}",
            union_m[0].miss
        );
        assert_eq!(
            union_m[0].superflu, 0.0,
            "[{name}] pas de prédiction superflue au départ"
        );

        // Après apprentissage (dernier pas) : l'union cumulative SUR-prédit.
        let last_union = union_m.last().unwrap();
        assert!(
            last_union.superflu > 0.0,
            "[{name}] union cumulative sur-prédit après apprentissage : superflu = {}",
            last_union.superflu
        );
        assert!(
            last_union.jaccard < 1.0,
            "[{name}] Jaccard final = {} < 1",
            last_union.jaccard
        );

        // Honnêteté : l'adaptatif (ajout sans décroissance) rejoint l'union.
        eprintln!(
            "[{name}] Jaccard final — union={:.3} last={:.3} freq={:.3} fixed={:.3} adaptatif={:.3}",
            last_union.jaccard,
            last_m.last().unwrap().jaccard,
            freq_m.last().unwrap().jaccard,
            fixed_m.last().unwrap().jaccard,
            ad_m.last().unwrap().jaccard,
        );
    }
}

/// CONTRÔLE POSITIF de rappel contextuel — PAS une prédiction avant observation :
/// les engrammes {{ctx,A,B}} et {{ctx,A,C}} sont formés AVANT le rappel, donc ce
/// test mesure la restitution d'associations déjà apprises, pas la prédiction.
/// Il vérifie qu'un marqueur de contexte observable (nœud) permet de distinguer
/// les deux associations, là où une union aveugle sur-prédit. Six form() pour
/// deux structures : cas concret pour la future déduplication (non exercée ici).
#[test]
fn rappel_contextuel_controle_positif() {
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));
    let ctx1 = NodeId(101);
    let ctx2 = NodeId(102);

    let store = EngramStore::new(None);
    let (writer, _td) = make_writer();
    store.attach_substrate(writer);

    // Ordre varié des contextes (pas d'alternance stricte).
    let steps: [(NodeId, [NodeId; 2]); 6] = [
        (ctx1, [a, b]),
        (ctx1, [a, b]),
        (ctx2, [a, c]),
        (ctx1, [a, b]),
        (ctx2, [a, c]),
        (ctx2, [a, c]),
    ];

    // Formation par contexte : {ctx, A, B} et {ctx, A, C}. Le contexte est le
    // signal observable qui devient l'indice de rappel.
    // Formation AVANT le rappel : 6 form() pour 2 structures distinctes
    // (déduplication non exercée ici — c'est le contrat d'identité de la primitive).
    for (ctx, ep) in &steps {
        let mut full = vec![*ctx];
        full.extend(ep.iter());
        store.form(&full).expect("form engramme de contexte");
    }

    // Prédiction par contexte (signal observable AVANT l'observation) :
    // rappel du marqueur, retrait du marqueur lui-même.
    let predict_by_ctx = |ctx: NodeId| -> Ep {
        store
            .recall(ctx)
            .expect("rappel de contexte")
            .into_iter()
            .flat_map(|eid| store.members(eid).expect("membres"))
            .filter(|&n| n != ctx)
            .collect()
    };

    let obs1: Ep = [a, b].into_iter().collect();
    let obs2: Ep = [a, c].into_iter().collect();

    // L'adaptatif (conditionné par le contexte) distingue les deux contextes.
    assert_eq!(
        metrics(&predict_by_ctx(ctx1), &obs1).jaccard,
        1.0,
        "ctx1 → {{A,B}} exact"
    );
    assert_eq!(
        metrics(&predict_by_ctx(ctx2), &obs2).jaccard,
        1.0,
        "ctx2 → {{A,C}} exact"
    );

    // Un prédicteur aveugle au contexte (union des deux) ne distingue pas.
    let blind: Ep = [a, b, c].into_iter().collect();
    let m1 = metrics(&blind, &obs1);
    let m2 = metrics(&blind, &obs2);
    assert!(
        (m1.jaccard - 2.0 / 3.0).abs() < 1e-9,
        "union aveugle sur-prédit en ctx1"
    );
    assert!(
        (m2.jaccard - 2.0 / 3.0).abs() < 1e-9,
        "union aveugle sur-prédit en ctx2"
    );
}
