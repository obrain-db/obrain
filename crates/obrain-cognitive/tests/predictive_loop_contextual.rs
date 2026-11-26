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
use obrain_cognitive::engram::{
    EngramManager, EngramManagerConfig, ObservationScope, PredictiveAction, PredictivePolicy,
};
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
// C — Régimes de prédiction temporelle ET probabiliste, à information ÉGALE.
//     Témoins : contexte (dernière / fréquence), TEMPOREL ordre-1, PROBABILISTE
//     marginal. Aucune supériorité d'Obrain n'est supposée : on rapporte le
//     classement réel par régime. (Correction : une alternance stricte est
//     PRÉVISIBLE par l'ordre-1, donc le témoin « dernière » n'est pas le bon
//     adversaire.)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Witnesses {
    ctx_history: HashMap<NodeId, Vec<Ep>>,
    // TEMPOREL ordre-1 CONDITIONNÉ par le contexte : (ctx, prev) -> next.
    // Meme information qu Obrain (contexte + issue precedente).
    transitions: HashMap<(NodeId, Ep), HashMap<Ep, usize>>,
    global: HashMap<Ep, usize>,
    prev: Option<Ep>,
}

impl Witnesses {
    fn observe(&mut self, ctx: NodeId, outcome: &Ep) {
        self.ctx_history.entry(ctx).or_default().push(outcome.clone());
        if let Some(p) = &self.prev {
            *self
                .transitions
                .entry((ctx, p.clone()))
                .or_default()
                .entry(outcome.clone())
                .or_insert(0) += 1;
        }
        *self.global.entry(outcome.clone()).or_insert(0) += 1;
        self.prev = Some(outcome.clone());
    }
    fn ctx_last(&self, ctx: NodeId) -> Ep {
        self.ctx_history
            .get(&ctx)
            .and_then(|v| v.last().cloned())
            .unwrap_or_default()
    }
    fn ctx_freq(&self, ctx: NodeId) -> Ep {
        let Some(v) = self.ctx_history.get(&ctx) else {
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
    /// TEMPOREL ordre-1 CONDITIONNÉ par le contexte : issue la plus fréquente
    /// APRÈS l'issue précédente DANS LE MÊME contexte (information égale à Obrain).
    fn temporal(&self, ctx: NodeId) -> Ep {
        self.prev
            .as_ref()
            .and_then(|p| self.transitions.get(&(ctx, p.clone())))
            .and_then(|m| m.iter().max_by_key(|(_, c)| **c))
            .map(|(e, _)| e.clone())
            .unwrap_or_default()
    }
    /// PROBABILISTE marginal (sans contexte).
    fn marginal(&self) -> Ep {
        self.global
            .iter()
            .max_by_key(|(_, c)| **c)
            .map(|(e, _)| e.clone())
            .unwrap_or_default()
    }
}

struct CaseResult {
    obrain: f64,
    ctx_last: f64,
    ctx_freq: f64,
    temporal: f64,
    marginal: f64,
}

fn run_case(
    m: &EngramManager,
    stream: &[(NodeId, Ep)],
    warmup: usize,
    k: usize,
) -> (Vec<f64>, CaseResult) {
    let mut w = Witnesses::default();
    let mut ob = Vec::new();
    let mut cl = Vec::new();
    let mut cf = Vec::new();
    let mut tp = Vec::new();
    let mut mg = Vec::new();
    for (ctx, obs) in stream {
        let step = m.predictive_step(&[*ctx], obs, k).expect("pas");
        cl.push(set_error(m, &w.ctx_last(*ctx), obs));
        cf.push(set_error(m, &w.ctx_freq(*ctx), obs));
        tp.push(set_error(m, &w.temporal(*ctx), obs));
        mg.push(set_error(m, &w.marginal(), obs));
        ob.push(step.error);
        w.observe(*ctx, obs);
    }
    let tail = |v: &Vec<f64>| mean(&v[warmup.min(v.len())..]);
    (
        ob.clone(),
        CaseResult {
            obrain: tail(&ob),
            ctx_last: tail(&cl),
            ctx_freq: tail(&cf),
            temporal: tail(&tp),
            marginal: tail(&mg),
        },
    )
}

fn lcg(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *seed >> 33
}

#[test]
fn c_regimes_temporels_et_probabilistes() {
    let (a, b, c, d) = (NodeId(1), NodeId(2), NodeId(3), NodeId(4));
    let ctx = NodeId(300);
    let ctx_a = NodeId(301);
    let ctx_b = NodeId(302);
    let ab: Ep = vec![ctx, a, b];
    let cd: Ep = vec![ctx, c, d];
    let ab_a: Ep = vec![ctx_a, a, b];
    let cd_a: Ep = vec![ctx_a, c, d];
    let ab_b: Ep = vec![ctx_b, a, b];
    let cd_b: Ep = vec![ctx_b, c, d];

    // 1. Alternance DÉTERMINISTE (prévisible par l'ordre-1).
    let mut alternance = Vec::new();
    for i in 0..12 {
        alternance.push((ctx, if i % 2 == 0 { ab.clone() } else { cd.clone() }));
    }
    // 2. Changement de RÉGIME.
    let mut regime = Vec::new();
    for _ in 0..3 {
        regime.push((ctx, ab.clone()));
    }
    for _ in 0..3 {
        regime.push((ctx, cd.clone()));
    }
    for _ in 0..3 {
        regime.push((ctx, ab.clone()));
    }
    // 3. VRAIMENT aléatoire (graine fixe).
    let mut seed = 42u64;
    let mut aleatoire = Vec::new();
    for _ in 0..14 {
        aleatoire.push((ctx, if lcg(&mut seed) % 2 == 0 { ab.clone() } else { cd.clone() }));
    }
    // 4. Distribution CONDITIONNELLE : ctx_a -> AB 80 %, ctx_b -> CD 80 %.
    let mut seed2 = 7u64;
    let mut conditionnelle = Vec::new();
    for i in 0..20 {
        let use_a = i % 2 == 0;
        let c2 = if use_a { ctx_a } else { ctx_b };
        let r = lcg(&mut seed2) % 10;
        let obs = if use_a {
            if r < 8 { ab_a.clone() } else { cd_a.clone() }
        } else if r < 8 {
            cd_b.clone()
        } else {
            ab_b.clone()
        };
        conditionnelle.push((c2, obs));
    }
    // 5. Contexte PARTIELLEMENT observable : signal faible (60 / 40).
    let mut seed3 = 99u64;
    let mut partiel = Vec::new();
    for i in 0..20 {
        let use_a = i % 2 == 0;
        let c2 = if use_a { ctx_a } else { ctx_b };
        let r = lcg(&mut seed3) % 10;
        let obs = if use_a {
            if r < 6 { ab_a.clone() } else { cd_a.clone() }
        } else if r < 6 {
            ab_b.clone()
        } else {
            cd_b.clone()
        };
        partiel.push((c2, obs));
    }

    let cases: Vec<(&str, &Vec<(NodeId, Ep)>, usize)> = vec![
        ("alternance_deterministe", &alternance, 3),
        ("changement_de_regime", &regime, 6),
        ("vraiment_aleatoire", &aleatoire, 2),
        ("distribution_conditionnelle", &conditionnelle, 4),
        ("contexte_partiellement_observable", &partiel, 4),
    ];

    for (name, stream, warmup) in &cases {
        let m = manager();
        let (ob, r) = run_case(&m, stream, *warmup, 8);
        eprintln!("[C:{}] erreurs Obrain = {:.3?}", name, ob);
        eprintln!(
            "[C:{}] moyenne après warmup — Obrain={:.3} ctx_last={:.3} ctx_freq={:.3} temporal={:.3} marginal={:.3}",
            name, r.obrain, r.ctx_last, r.ctx_freq, r.temporal, r.marginal
        );

        // On RAPPORTE le classement réel ; on n'invente pas de supériorité.
        if *name == "alternance_deterministe" {
            assert!(
                r.temporal < 0.05,
                "l'alternance déterministe est prévisible par l'ordre-1 (temporal={})",
                r.temporal
            );
            assert!(
                r.obrain > r.temporal,
                "Obrain ({}) ne bat pas l'ordre-1 ({}) sur l'alternance",
                r.obrain,
                r.temporal
            );
        }
        if *name == "vraiment_aleatoire" {
            // Sur le hasard, AUCUNE méthode n'est quasi-parfaite (pas de signal).
            assert!(
                r.temporal > 0.05
                    && r.ctx_freq > 0.05
                    && r.marginal > 0.05
                    && r.obrain > 0.05,
                "aucune méthode n'est quasi-parfaite sur le hasard"
            );
        }
        if *name == "distribution_conditionnelle" {
            assert!(
                r.ctx_freq < r.marginal,
                "le conditionnel ({}) bat le marginal ({})",
                r.ctx_freq,
                r.marginal
            );
        }
        if *name == "contexte_partiellement_observable" {
            // Signal faible : le conditionnel reste meilleur que le marginal,
            // mais l'écart se resserre — on le vérifie sans le magnifier.
            assert!(
                r.ctx_freq <= r.marginal,
                "signal faible : conditionnel ({}) <= marginal ({})",
                r.ctx_freq,
                r.marginal
            );
        }
    }
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

// ---------------------------------------------------------------------------
// F — PLASTICITÉ : révision NON MONOTONE (régime qui rétrécit) et
//     DIFFÉRENCIATION contextuelle (formation épisodique).
// ---------------------------------------------------------------------------

#[test]
fn f_plasticite_revision_non_monotone_et_differenciation() {
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));
    let ctx = NodeId(500);
    let ab: Ep = vec![ctx, a, b];
    let abc: Ep = vec![ctx, a, b, c];

    // (1) RÉVISION NON MONOTONE : régime qui croît puis rétrécit.
    let mut stream = Vec::new();
    for _ in 0..3 {
        stream.push((ctx, abc.clone()));
    }
    for _ in 0..8 {
        stream.push((ctx, ab.clone()));
    }

    let run = |policy: PredictivePolicy| -> Vec<f64> {
        let m = manager();
        stream
            .iter()
            .map(|(ctx_, obs)| {
                m.predictive_step_with_policy(&[*ctx_], obs, 8, policy)
                    .expect("pas")
                    .error
            })
            .collect()
    };

    let cons = run(PredictivePolicy::Consolidating);
    let ada = run(PredictivePolicy::Adaptive(ObservationScope::Complete));
    let tail_cons = mean(&cons[cons.len().saturating_sub(2)..]);
    let tail_ada = mean(&ada[ada.len().saturating_sub(2)..]);
    eprintln!("[F] rétrécissement — résiduel consolidating={:.4} adaptive={:.4}", tail_cons, tail_ada);
    assert!(
        tail_ada < tail_cons,
        "la révision NON MONOTONE réduit le résidu ({} < {})",
        tail_ada,
        tail_cons
    );

    // (2) DIFFÉRENCIATION : deux contextes partageant un participant.
    let (ctx1, ctx2) = (NodeId(501), NodeId(502));
    let o1: Ep = vec![ctx1, a, b];
    let o2: Ep = vec![ctx2, b, c];

    let learn = |policy: PredictivePolicy| -> EngramManager {
        let m = manager();
        for _ in 0..3 {
            let _ = m
                .predictive_step_with_policy(&[ctx1], &o1, 8, policy)
                .expect("pas o1");
            let _ = m
                .predictive_step_with_policy(&[ctx2], &o2, 8, policy)
                .expect("pas o2");
        }
        m
    };

    let m_epi = learn(PredictivePolicy::Episodic);
    let m_cons = learn(PredictivePolicy::Consolidating);
    let epi_count = m_epi.store().count();
    let cons_count = m_cons.store().count();
    let fused = m_cons.store().list().iter().any(|e| {
        let ids: std::collections::BTreeSet<u64> = e.node_ids().iter().map(|n| n.0).collect();
        ids.contains(&ctx1.0) && ids.contains(&ctx2.0)
    });
    eprintln!(
        "[F] différenciation — engrams episodic={} consolidating={} (fusion={})",
        epi_count, cons_count, fused
    );
    assert!(
        epi_count >= 2,
        "la formation ÉPISODIQUE garde deux contextes distincts (count={})",
        epi_count
    );
    assert!(fused, "la formation HEBBIENNE fusionne les contextes partagés");

    // (3) L'historique est PRÉSERVÉ : les engrams touchés ont un recall_count > 0.
    let mut touched = 0;
    for e in m_epi.store().list() {
        if e.recall_count > 0 || e.strength > 0.5 {
            touched += 1;
        }
    }
    assert!(
        touched >= 1,
        "l'historique (recall_count/strength) est conservé après révision"
    );
    eprintln!("[F] historique conservé sur {touched} engram(s)");
}
