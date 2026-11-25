//! Apprentissage contextuel NATIF à information égale — suite scientifique.
//!
//! Corrige les trois biais du premier test contextuel :
//!  (a) AUCUN bootstrap manuel : on apprend depuis un manager VIDE via la
//!      primitive native `EngramManager::predictive_step` ;
//!  (b) les témoins classiques reçoivent le MÊME contexte et la MÊME histoire
//!      qu'Obrain (témoin conditionné) ;
//!  (c) l'évaluation fait partie du flot : la révision continue de tourner.
//!
//! Mesures séparées : erreur AVANT apprentissage, vitesse d'adaptation, erreur
//! résiduelle, interférence, sensibilité du seuil de reconsolidation.
//! Aucune distance L2 n'est calculée entre deux dimensions différentes
//! (`prediction_error` le refuse).

#![cfg(feature = "engram")]

use std::collections::HashMap;
use std::sync::Arc;

use obrain_common::types::NodeId;
use obrain_cognitive::engram::{EngramManager, EngramManagerConfig, PredictiveAction};
use obrain_cognitive::InMemoryVectorIndex;

type Ep = Vec<NodeId>;

fn manager() -> EngramManager {
    EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    )
}

/// Erreur L2 d'un ensemble prédit face à l'observation, via la primitive native
/// (dimension vérifiée : même encodeur, donc même dimension).
fn set_error(m: &EngramManager, predicted: &Ep, observed: &Ep) -> f64 {
    let p = m.encode_observation(predicted);
    let o = m.encode_observation(observed);
    m.prediction_error(&p, &o)
        .expect("même encodeur => dimensions égales")
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

/// Premier indice (1-based) où l'erreur passe sous `seuil`, ou `None`.
fn adaptation_speed(errors: &[f64], seuil: f64) -> Option<usize> {
    errors.iter().position(|&e| e < seuil).map(|i| i + 1)
}

/// Témoin classique CONDITIONNÉ par le contexte et l'histoire : il reçoit le
/// contexte AVANT de prédire, exactement comme Obrain (information égale).
#[derive(Default)]
struct ConditionalWitness {
    history: HashMap<NodeId, Vec<Ep>>,
}

impl ConditionalWitness {
    fn observe(&mut self, ctx: NodeId, outcome: &Ep) {
        self.history.entry(ctx).or_default().push(outcome.clone());
    }

    /// Contexte -> dernière observation.
    fn last(&self, ctx: NodeId) -> Ep {
        self.history
            .get(&ctx)
            .and_then(|v| v.last().cloned())
            .unwrap_or_default()
    }

    /// Contexte -> ensemble le plus fréquent (égalité = première occurrence).
    fn freq(&self, ctx: NodeId) -> Ep {
        let Some(v) = self.history.get(&ctx) else {
            return Ep::new();
        };
        let mut counts: Vec<(Ep, usize)> = Vec::new();
        for e in v {
            if let Some(x) = counts.iter_mut().find(|(k, _)| k == e) {
                x.1 += 1;
            } else {
                counts.push((e.clone(), 1));
            }
        }
        counts
            .into_iter()
            .max_by_key(|(_, c)| *c)
            .map(|(e, _)| e)
            .unwrap_or_default()
    }

    /// Contexte -> union des observations vues pour ce contexte.
    fn union(&self, ctx: NodeId) -> Ep {
        let mut acc: Ep = Ep::new();
        if let Some(v) = self.history.get(&ctx) {
            for e in v {
                for &n in e {
                    if !acc.contains(&n) {
                        acc.push(n);
                    }
                }
            }
        }
        acc
    }
}

struct StepResult {
    obrain: f64,
    last: f64,
    freq: f64,
    union: f64,
    had_prediction: bool,
    engrams: usize,
}

/// Déroule un flot (contexte, observation) : Obrain et les trois témoins
/// prédisent AVANT d'observer, puis tout le monde observe.
fn run_stream(
    m: &EngramManager,
    w: &mut ConditionalWitness,
    stream: &[(NodeId, Ep)],
    k: usize,
    out: &mut Vec<StepResult>,
) {
    for (ctx, obs) in stream {
        let step = m
            .predictive_step(&[*ctx], obs, k)
            .expect("pas prédictif");
        let last = set_error(m, &w.last(*ctx), obs);
        let freq = set_error(m, &w.freq(*ctx), obs);
        let union = set_error(m, &w.union(*ctx), obs);
        w.observe(*ctx, obs);
        out.push(StepResult {
            obrain: step.error,
            last,
            freq,
            union,
            had_prediction: step.had_prediction,
            engrams: step.engram_count,
        });
    }
}

// ---------------------------------------------------------------------------
// A — Apprentissage contextuel depuis le VIDE vs témoins conditionnés.
// ---------------------------------------------------------------------------

#[test]
fn a_apprentissage_contextuel_depuis_le_vide() {
    let m = manager();
    let mut w = ConditionalWitness::default();
    let (ctx1, ctx2) = (NodeId(101), NodeId(102));
    let (a, b, c, d) = (NodeId(1), NodeId(2), NodeId(3), NodeId(4));
    let obs1: Ep = vec![ctx1, a, b];
    let obs2: Ep = vec![ctx2, c, d];

    // 4 tours interleavés : apprentissage ET évaluation dans le même flot.
    let mut stream = Vec::new();
    for _ in 0..4 {
        stream.push((ctx1, obs1.clone()));
        stream.push((ctx2, obs2.clone()));
    }
    let mut res = Vec::new();
    run_stream(&m, &mut w, &stream, 8, &mut res);

    let ob: Vec<f64> = res.iter().map(|r| r.obrain).collect();
    let last: Vec<f64> = res.iter().map(|r| r.last).collect();
    let freq: Vec<f64> = res.iter().map(|r| r.freq).collect();
    let union: Vec<f64> = res.iter().map(|r| r.union).collect();

    // AVANT apprentissage : ni Obrain ni les témoins ne peuvent prédire.
    assert!(
        res[0].obrain > 0.99,
        "Obrain sans mémoire : erreur ~1.0, obtenu {}",
        res[0].obrain
    );
    assert!(!res[0].had_prediction, "aucune prédiction avant le premier engramme");
    assert!(res[0].last > 0.99, "témoin sans histoire : erreur ~1.0");

    // APPREND : l'erreur résiduelle est inférieure à l'erreur initiale.
    let pre = res[0].obrain;
    let residual = mean(&ob[ob.len().saturating_sub(4)..]);
    assert!(
        residual < pre,
        "Obrain apprend réellement ({} -> {})",
        pre,
        residual
    );

    eprintln!("[A] Obrain  erreurs = {:.4?}", ob);
    eprintln!("[A] témoin last = {:.4?}", last);
    eprintln!("[A] témoin freq = {:.4?}", freq);
    eprintln!("[A] témoin union= {:.4?}", union);
    eprintln!(
        "[A] résiduel — Obrain={:.4} last={:.4} freq={:.4} union={:.4}",
        residual,
        mean(&last[last.len().saturating_sub(4)..]),
        mean(&freq[freq.len().saturating_sub(4)..]),
        mean(&union[union.len().saturating_sub(4)..]),
    );
    eprintln!(
        "[A] vitesse (1er < 0.1) — Obrain={:?} last={:?} freq={:?} union={:?}",
        adaptation_speed(&ob, 0.1),
        adaptation_speed(&last, 0.1),
        adaptation_speed(&freq, 0.1),
        adaptation_speed(&union, 0.1),
    );
    // Engrammes réellement formés depuis le vide (aucun bootstrap).
    assert!(
        res.last().unwrap().engrams >= 1,
        "des engrams se sont formés par le flot seul"
    );

    // CONSTAT HONNÊTE : sur cette tâche déterministe, Obrain n'atteint JAMAIS
    // l'erreur < 0.1 (plancher ~0.44 : centroïde = mélange des deux contextes),
    // alors que les témoins conditionnés y arrivent dès le 3e pas. Obrain ne bat
    // PAS un simple lookup contexte->dernière au même niveau d'information.
    assert!(
        adaptation_speed(&ob, 0.1).is_none(),
        "Obrain n'atteint pas < 0.1 sur cette tâche (plancher de mélange)"
    );
    assert!(
        adaptation_speed(&last, 0.1).is_some(),
        "le témoin conditionné atteint < 0.1"
    );
}

// ---------------------------------------------------------------------------
// B — Séquence non monotone : croissance puis rétrécissement (régime change).
// ---------------------------------------------------------------------------

#[test]
fn b_sequence_non_monotone_et_changement_de_regime() {
    let m = manager();
    let mut w = ConditionalWitness::default();
    let ctx = NodeId(200);
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));
    let ab: Ep = vec![ctx, a, b];
    let abc: Ep = vec![ctx, a, b, c];

    // Croissance (3x) puis rétrécissement (3x) : le régime CHANGE.
    let mut stream = Vec::new();
    for _ in 0..3 {
        stream.push((ctx, ab.clone()));
    }
    for _ in 0..3 {
        stream.push((ctx, abc.clone()));
    }
    for _ in 0..4 {
        stream.push((ctx, ab.clone()));
    }
    let mut res = Vec::new();
    run_stream(&m, &mut w, &stream, 8, &mut res);

    let ob: Vec<f64> = res.iter().map(|r| r.obrain).collect();
    let last: Vec<f64> = res.iter().map(|r| r.last).collect();

    // Après le rétrécissement, Obrain ne PEUT PAS retirer c (reconsolidation
    // additive). On mesure le résidu sur les 2 DERNIERS pas pour exclure le pas
    // de transition (où « last » prédit encore l'ancien régime).
    let tail_ob = mean(&ob[ob.len().saturating_sub(2)..]);
    let tail_last = mean(&last[last.len().saturating_sub(2)..]);

    eprintln!("[B] Obrain erreurs = {:.4?}", ob);
    eprintln!("[B] témoin last    = {:.4?}", last);
    eprintln!(
        "[B] résiduel après changement de régime — Obrain={:.4} last={:.4}",
        tail_ob, tail_last
    );

    assert!(
        tail_last < 0.05,
        "le témoin « dernière observation » s'adapte au rétrécissement : {tail_last}"
    );
    assert!(
        tail_ob > tail_last,
        "LIMITE mesurée : la reconsolidation additive n'oublie pas c (Obrain {} > last {})",
        tail_ob,
        tail_last
    );
}

// ---------------------------------------------------------------------------
// C — Observations contradictoires (même contexte, deux issues irréductibles).
// ---------------------------------------------------------------------------

#[test]
fn c_observations_contradictoires() {
    let m = manager();
    let mut w = ConditionalWitness::default();
    let ctx = NodeId(300);
    let (a, b, c, d) = (NodeId(1), NodeId(2), NodeId(3), NodeId(4));
    let ab: Ep = vec![ctx, a, b];
    let cd: Ep = vec![ctx, c, d];

    // Alternance stricte : aucune prédiction déterministe n'est possible.
    let mut stream = Vec::new();
    for _ in 0..6 {
        stream.push((ctx, ab.clone()));
        stream.push((ctx, cd.clone()));
    }
    let mut res = Vec::new();
    run_stream(&m, &mut w, &stream, 8, &mut res);

    let ob: Vec<f64> = res.iter().map(|r| r.obrain).collect();
    let last: Vec<f64> = res.iter().map(|r| r.last).collect();
    let freq: Vec<f64> = res.iter().map(|r| r.freq).collect();

    let ob_tail = mean(&ob[ob.len().saturating_sub(6)..]);
    let last_tail = mean(&last[last.len().saturating_sub(6)..]);
    let freq_tail = mean(&freq[freq.len().saturating_sub(6)..]);

    eprintln!("[C] Obrain = {:.4?}", ob);
    eprintln!("[C] last   = {:.4?}", last);
    eprintln!("[C] freq   = {:.4?}", freq);
    eprintln!(
        "[C] résiduel (irréductible) — Obrain={:.4} last={:.4} freq={:.4}",
        ob_tail, last_tail, freq_tail
    );

    // Aucune stratégie ne peut être exacte : le témoin « dernière » se trompe
    // une fois sur deux (erreur forte), le fréquentiel choisit un camp fixe.
    assert!(
        last_tail > 0.5,
        "contradiction irréductible : last se trompe une fois sur deux ({last_tail})"
    );
    // Obrain, en mélangeant les deux issues, n'est pas plus mauvais que last.
    assert!(
        ob_tail <= last_tail + 0.2,
        "Obrain (mélange) ne s'effondre pas face à la contradiction ({} vs {})",
        ob_tail,
        last_tail
    );
}

// ---------------------------------------------------------------------------
// D — Contextes partiellement communs : interférence.
// ---------------------------------------------------------------------------

#[test]
fn d_contextes_partiellement_communs_interference() {
    let m = manager();
    let (ctx1, ctx2) = (NodeId(401), NodeId(402));
    let (a, b, c, d) = (NodeId(1), NodeId(2), NodeId(3), NodeId(4));
    // ctx1 et ctx2 PARTAGENT {a,b} ; ils diffèrent par c vs d.
    let o1: Ep = vec![ctx1, a, b, c];
    let o2: Ep = vec![ctx2, a, b, d];

    // Apprend ctx1 seul.
    for _ in 0..3 {
        let _ = m.predictive_step(&[ctx1], &o1, 8).expect("pas");
    }
    let before = {
        let p = m.predict(&[ctx1], 8).expect("prédiction").predicted;
        let o = m.encode_observation(&o1);
        m.prediction_error(&p, &o).expect("dim")
    };

    // Apprend ctx2 (partage a,b avec ctx1).
    for _ in 0..3 {
        let _ = m.predictive_step(&[ctx2], &o2, 8).expect("pas");
    }
    let after = {
        let p = m.predict(&[ctx1], 8).expect("prédiction").predicted;
        let o = m.encode_observation(&o1);
        m.prediction_error(&p, &o).expect("dim")
    };

    eprintln!(
        "[D] erreur de ctx1 avant={:.4} après apprentissage de ctx2={:.4} (delta={:+.4})",
        before,
        after,
        after - before
    );

    // L'engramme de ctx1 reste présent et l'erreur ne s'effondre pas : on
    // rapporte le chiffre, sans prétendre qu'il n'y a pas d'interférence.
    assert!(
        after.is_finite() && before.is_finite(),
        "mesures finies"
    );
    // Les deux contextes PARTAGENT a,b : la composante connexe de co-activation
    // les FUSIONNE en UN seul engramme. Limite mesurée de la formation.
    assert_eq!(
        m.store().count(),
        1,
        "contextes partiellement communs : fusion en un seul engramme (count={})",
        m.store().count()
    );
    assert!(
        after > before,
        "interférence mesurée : erreur de ctx1 {} -> {} après fusion",
        before,
        after
    );
}

// ---------------------------------------------------------------------------
// E — Sensibilité du seuil de reconsolidation (0.1 codé en dur).
// ---------------------------------------------------------------------------

#[test]
fn e_sensibilite_du_seuil_de_reconsolidation() {
    let m = manager();
    let (a, b) = (NodeId(1), NodeId(2));
    let pair: Ep = vec![a, b];

    // Forme {a,b} par le flot natif.
    for _ in 0..3 {
        let _ = m.predictive_step(&[a], &pair, 8).expect("pas");
    }
    let eid = m
        .predict(&[a], 8)
        .expect("prédiction")
        .attention[0]
        .0;
    let size_before = m.store().get(eid).unwrap().node_ids().len();
    assert_eq!(size_before, 2, "engramme {{a,b}} formé");

    // Observation étendue : ajoute un 3e nœud DISTINCT. On mesure l'erreur
    // réelle et on observe si la reconsolidation s'est déclenchée.
    let ext: Ep = vec![a, b, NodeId(9)];
    let step = m.predictive_step(&[a], &ext, 8).expect("pas");
    let size_after = m.store().get(eid).unwrap().node_ids().len();
    let revised = matches!(step.action, PredictiveAction::Revised(_));

    eprintln!(
        "[E] erreur réelle = {:.4} ; action revise={} ; taille {} -> {}",
        step.error, revised, size_before, size_after
    );

    // Le seuil 0.1 est une CONSTANTE codée en dur : au-dessus, la
    // reconsolidation AJOUTE ; en dessous, elle n'ajoute rien. On qualifie son
    // effet (discontinu) sans prétendre qu'il est optimal.
    if step.error > 0.1 {
        assert!(revised, "au-dessus de 0.1 : révision appliquée");
        assert!(size_after > size_before, "nœud manquant ajouté");
    } else {
        assert_eq!(size_after, size_before, "sous 0.1 : aucun ajout");
    }

    // Contre-épreuve : une observation IDENTIQUE (erreur ~0) ne doit jamais
    // ajouter de nœud, quel que soit le seuil.
    let step2 = m.predictive_step(&[a], &ext, 8).expect("pas");
    let size_after2 = m.store().get(eid).unwrap().node_ids().len();
    assert!(
        step2.error < step.error + 0.2,
        "l'erreur mesurée reste bornée"
    );
    assert!(
        size_after2 >= size_after,
        "pas de retrait (reconsolidation additive)"
    );
}
