//! Overlays **stables** (P0-v03) : attachent les signaux cognitifs persistés
//! comme **attributs standard** (`color_hint`, `size_hint`) sur l'IR, avec une
//! fonction de normalisation **documentée par overlay** — c'est la
//! différenciation d'obrain (ce que ni Neo4j ni Kuzu ne peuvent copier).
//!
//! Mapping v1 (déterministe, pur, testé) :
//! - **energy** (nœuds, Q1.15 décodé ∈ [0,1]) → `color_hint` sur rampe
//!   froide→chaude (`#3b4cc0` froid / `#f5d63d` médian / `#e03b2f` chaud) et
//!   `size_hint = 0.75 + 1.5·e` ;
//! - **roles** (nœuds, `scc_id`) → `color_hint` pastel déterministe dérivé du
//!   `scc_id` (FNV-1a → teinte) ;
//! - **ricci** (arêtes, κ ∈ [-1,1], `ricci_u8` décodé) → `color_hint` divergent
//!   autour de 0 (négatif = bord de communauté → rouge, positif = intérieur →
//!   bleu, gris si `stale`) et `size_hint = 1 + 3·|κ|` (épaisseur).
//!
//! Un seul overlay par appel pour l'instant.
//!
//! Overlays **volatils** (V1-A) : `fsrs`/`engrams`/`pheromone` portent des valeurs
//! **temporelles** (décroissance FSRS, évaporation des phéromones) → deux exports
//! espacés d'un même store peuvent différer. C'est assumé et documenté : les
//! overlays volatils posent un `generated_at` dans le rapport (voir
//! [`Overlay::is_volatile`]) et les appels répétés ne sont PAS censés être
//! octet-déterministes (le reste de la lib l'est).

impl Overlay {
    /// Les overlays volatils produisent des exports non déterministes dans le
    /// temps (FSRS decay / évaporation) — l'exporteur doit horodater.
    #[must_use]
    pub const fn is_volatile(self) -> bool {
        matches!(self, Self::Fsrs | Self::Engrams | Self::Pheromone)
    }
}

use crate::ir::Subgraph;
use crate::sanitize::fnv1a64;

/// Overlays stables disponibles en P0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    /// Courbure Ricci-Ollivier per-edge (persistée `ricci_u8`).
    Ricci,
    /// Énergie nodale persistée (Q1.15 décodée).
    Energy,
    /// Rôles structurels (composante `scc_id`).
    Roles,
    /// FSRS-5 (stabilité/difficulté d'engram, VOLATILE — décroissance temporelle).
    Fsrs,
    /// Appartenance à un engram (VOLATILE).
    Engrams,
    /// Phéromones stigmergy sur arêtes (VOLATILE — évaporation).
    Pheromone,
    /// Tiers mémoire (STABLE) : la masse comme politique de cache.
    /// masse = 0.7·énergie + 0.3·degré normalisé (sur le sous-graphe) ;
    /// quantiles → chaud (réflexe/RAM), tiède (mmap), froid (log seul).
    Tier,
}

impl Overlay {
    /// Les quatre overlays stables + les trois volatils (V1-A).
    pub const ALL: [Self; 7] = [
        Self::Ricci,
        Self::Energy,
        Self::Roles,
        Self::Tier,
        Self::Fsrs,
        Self::Engrams,
        Self::Pheromone,
    ];

    /// Nom stable de l'overlay (utilisé dans `metadata.overlay` et la CLI).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ricci => "ricci",
            Self::Energy => "energy",
            Self::Roles => "roles",
            Self::Fsrs => "fsrs",
            Self::Engrams => "engrams",
            Self::Pheromone => "pheromone",
            Self::Tier => "tier",
        }
    }
}

/// Compte-rendu d'application d'un overlay (compteurs + légende exploitable
/// par la CLI/MCP pour documenter l'export).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayReport {
    pub overlay: String,
    pub nodes_styled: usize,
    pub edges_styled: usize,
    /// Arêtes marquées `RICCI_STALE` (fraîcheur du signal exposée).
    pub stale_edges: usize,
    /// Légende humaine : une entrée par règle appliquée.
    pub legend: Vec<String>,
}

/// Applique un overlay sur une **copie** du sous-graphe (l'entrée n'est pas
/// mutée) et retourne la copie stylée + le rapport. Déterministe : les hints
/// ne dépendent que des champs signal du sous-graphe.
pub fn apply_overlay(subgraph: &Subgraph, overlay: Overlay) -> (Subgraph, OverlayReport) {
    let mut styled = subgraph.clone();
    let mut report = OverlayReport {
        overlay: overlay.as_str().to_string(),
        nodes_styled: 0,
        edges_styled: 0,
        stale_edges: 0,
        legend: Vec::new(),
    };
    match overlay {
        Overlay::Energy => {
            // ── REPLI SUR LE DEGRÉ QUAND L'ÉNERGIE EST VIDE ─────────────────
            //
            // Mesuré sur les stores de code : `energy = 0.0` pour TOUS les nœuds.
            // L'overlay posait donc la MÊME couleur et la MÊME taille partout
            // (300 nœuds à 0,75, une seule couleur) : il ne distinguait RIEN.
            // Un overlay qui ne distingue rien ne sert ni l'œil, ni le moteur
            // d'inférence qui lit cette projection pour s'orienter.
            //
            // Le DEGRÉ, lui, est toujours là : il se calcule sur les arêtes du
            // sous-graphe. C'est la centralité — le signal le plus utile pour
            // repérer un hub, et le seul qui reste quand l'énergie manque.
            let energie_vide = styled
                .nodes
                .iter()
                .all(|n| n.energy.unwrap_or(0.0) <= f64::EPSILON);
            let mut degre: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
            for e in &styled.edges {
                *degre.entry(e.src.as_str()).or_insert(0) += 1;
                *degre.entry(e.dst.as_str()).or_insert(0) += 1;
            }
            let max_d = degre.values().copied().max().unwrap_or(0);
            if energie_vide && max_d > 1 {
                // ── ÉCHELLE LOGARITHMIQUE, PAS LINÉAIRE ────────────────────
                // Mesuré : une normalisation LINÉAIRE est écrasée par
                // l'exception. Avec un hub à 11 077 arêtes et le reste sous 181,
                // un nœud de degré 181 retombait à 0,77 — invisible, alors qu'il
                // est 60 fois plus connecté que la médiane. Une distribution de
                // degrés est en loi de puissance : c'est le logarithme qui rend
                // sa queue lisible. On garde donc une correspondance STRICTEMENT
                // croissante (le degré reste lisible tel quel), mais répartie.
                let log_max = (1.0 + max_d as f64).ln().max(f64::EPSILON);
                for node in &mut styled.nodes {
                    let deg = *degre.get(node.id.as_str()).unwrap_or(&0) as f64;
                    let d = (1.0 + deg).ln() / log_max;
                    node.color_hint = Some(color_ramp(d));
                    node.size_hint = Some(0.75 + 1.5 * d);
                    report.nodes_styled += 1;
                }
                report.legend.push(
                    "energy vide → repli sur le DEGRÉ (centralité) : couleur froide→chaude, taille 0.75→2.25".into(),
                );
            } else {
                for node in &mut styled.nodes {
                    if let Some(e) = node.energy {
                        let e = e.clamp(0.0, 1.0);
                        node.color_hint = Some(color_ramp(e));
                        node.size_hint = Some(0.75 + 1.5 * e);
                        report.nodes_styled += 1;
                    }
                }
                report.legend.push(
                    "energy: couleur froide→chaude (#3b4cc0→#f5d63d→#e03b2f), taille 0.75→2.25".into(),
                );
            }
        }
        Overlay::Roles => {
            for node in &mut styled.nodes {
                if let Some(scc) = node.scc_id {
                    node.color_hint = Some(pastel_for_id(scc));
                    report.nodes_styled += 1;
                }
            }
            report
                .legend
                .push("roles: couleur pastel déterministe par scc_id (FNV-1a → teinte)".into());
        }
        Overlay::Ricci => {
            for edge in &mut styled.edges {
                if let Some(k) = edge.ricci {
                    if edge.stale.unwrap_or(false) {
                        edge.color_hint = Some("#9e9e9e".into()); // gris = signal non frais
                        report.stale_edges += 1;
                    } else {
                        edge.color_hint = Some(ricci_color(k.clamp(-1.0, 1.0)));
                    }
                    edge.size_hint = Some(1.0 + 3.0 * k.clamp(-1.0, 1.0).abs());
                    report.edges_styled += 1;
                }
            }
            report.legend.push(
                "ricci: négatif (bord de communauté) → rouge, positif (intérieur) → bleu, gris = stale"
                    .into(),
            );
            if report.stale_edges > 0 {
                report.legend.push(format!(
                    "ricci: {} arête(s) stale (valeur non fraîche)",
                    report.stale_edges
                ));
            }
        }
        Overlay::Tier => {
            // La masse EST la politique de cache : ce que la dynamique
            // désigne comme chaud cognitivement doit être chaud
            // matériellement (RAM/réflexe), le déclin descend en mmap,
            // l'oubli ne survit que dans le log. Déterministe par snapshot :
            // masse = 0.7·énergie + 0.3·degré normalisé sur le sous-graphe,
            // classement par quantiles (top 10 % chaud, 40 % tiède, reste froid).
            use std::collections::HashMap;
            let mut deg: HashMap<&str, u32> = HashMap::new();
            for e in &styled.edges {
                *deg.entry(e.src.as_str()).or_insert(0) += 1;
                *deg.entry(e.dst.as_str()).or_insert(0) += 1;
            }
            let deg_max = deg.values().copied().max().unwrap_or(0).max(1) as f64;
            let mut masses: Vec<f64> = styled
                .nodes
                .iter()
                .map(|n| {
                    let e = n.energy.unwrap_or(0.0).clamp(0.0, 1.0);
                    let d = f64::from(deg.get(n.id.as_str()).copied().unwrap_or(0)) / deg_max;
                    0.7 * e + 0.3 * d
                })
                .collect();
            let mut sorted = masses.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let q = |frac: f64| -> f64 {
                if sorted.is_empty() {
                    return 0.0;
                }
                let idx = ((sorted.len() as f64 - 1.0) * frac).round() as usize;
                sorted[idx]
            };
            // Seuils par quantiles : ≥ q90 chaud, ≥ q50 tiède, sinon froid.
            let (q_hot, q_warm) = (q(0.90), q(0.50));
            let (mut hot, mut warm, mut cold) = (0usize, 0usize, 0usize);
            for (node, m) in styled.nodes.iter_mut().zip(masses.drain(..)) {
                let (color, size) = if m >= q_hot && m > 0.0 {
                    hot += 1;
                    ("#ff5c4d", 2.1) // chaud : servi en RAM, latence réflexe
                } else if m >= q_warm && m > 0.0 {
                    warm += 1;
                    ("#f5d63d", 1.3) // tiède : mmap, faulté à la demande
                } else {
                    cold += 1;
                    ("#3b4cc0", 0.8) // froid : reconstructible depuis le log
                };
                node.color_hint = Some(color.into());
                node.size_hint = Some(size);
                report.nodes_styled += 1;
            }
            report.legend.push(
                "tier: masse = 0.7·énergie + 0.3·degré normalisé — la masse est la politique de cache"
                    .into(),
            );
            report.legend.push(format!(
                "tier: chaud #ff5c4d ≥q90 (RAM/réflexe, {hot}), tiède #f5d63d ≥q50 (mmap, {warm}), froid #3b4cc0 (log, {cold})"
            ));
        }
        Overlay::Fsrs => {
            // Difficulté → rampe froide→chaude (élevée = rouge) ; stabilité →
            // taille (jours, plafonnés à 20).
            for node in &mut styled.nodes {
                if let (Some(d), Some(st)) = (node.fsrs_difficulty, node.fsrs_stability) {
                    let d = ((d.clamp(1.0, 10.0) - 1.0) / 9.0).clamp(0.0, 1.0);
                    node.color_hint = Some(color_ramp(d));
                    node.size_hint = Some(0.8 + (st.clamp(0.0, 20.0) / 20.0) * 1.4);
                    report.nodes_styled += 1;
                }
            }
            report.legend.push(
                "fsrs: couleur = difficulté (1→9, froide→chaude), taille = stabilité (0→20 j)"
                    .into(),
            );
        }
        Overlay::Engrams => {
            for node in &mut styled.nodes {
                if let Some(id) = &node.engram_id {
                    let h = fnv1a64(&format!("engram:{id}"));
                    node.color_hint = Some(pastel_for_id(h));
                    node.size_hint = Some(1.15);
                    report.nodes_styled += 1;
                } else if node.engram_state.is_some() {
                    node.size_hint = Some(1.15);
                    report.nodes_styled += 1;
                }
            }
            report.legend.push(
                "engrams: couleur pastel déterministe par engram (id), taille 1.15 pour les membres"
                    .into(),
            );
        }
        Overlay::Pheromone => {
            // Normalisation relative au MAX du sous-graphe (déterministe pour un
            // graphe donné ; PAS comparable entre deux exports espacés — le max
            // décroît avec l'évaporation).
            let max_p = styled
                .edges
                .iter()
                .filter_map(|e| e.pheromone)
                .fold(0.0_f64, f64::max);
            if max_p > 0.0 {
                for edge in &mut styled.edges {
                    if let Some(p) = edge.pheromone {
                        let rel = (p / max_p).clamp(0.0, 1.0);
                        edge.color_hint = Some(pheromone_color(rel));
                        edge.size_hint = Some(1.0 + 3.0 * rel);
                        report.edges_styled += 1;
                    }
                }
            }
            report.legend.push(
                "pheromone: intensité relative au max du sous-graphe (gris → orange), épaisseur 1→4"
                    .into(),
            );
        }
    }
    (styled, report)
}

/// Interpole une couleur le long d'une rampe à 3 arrêts (bleu froid →
/// jaune → rouge chaud). `t` est clampé en [0,1].
#[must_use]
pub fn color_ramp(t: f64) -> String {
    const STOPS: [(f64, [u8; 3]); 3] = [
        (0.0, [59, 76, 192]),  // #3b4cc0 froid
        (0.5, [245, 214, 61]), // #f5d63d médian
        (1.0, [224, 59, 47]),  // #e03b2f chaud
    ];
    let t = t.clamp(0.0, 1.0);
    let mut lo = STOPS[0];
    let mut hi = STOPS[1];
    for pair in STOPS.windows(2) {
        if t <= pair[1].0 {
            lo = pair[0];
            hi = pair[1];
            break;
        }
    }
    let span = hi.0 - lo.0;
    let f = if span == 0.0 { 1.0 } else { (t - lo.0) / span };
    hex8([
        lerp_u8(lo.1[0], hi.1[0], f),
        lerp_u8(lo.1[1], hi.1[1], f),
        lerp_u8(lo.1[2], hi.1[2], f),
    ])
}

/// Couleur d'arête ricci : gris neutre en 0, rouge croissant pour κ < 0
/// (bord de communauté / goulot), bleu croissant pour κ > 0 (intérieur).
#[must_use]
pub fn ricci_color(kappa: f64) -> String {
    const GREY: [u8; 3] = [232, 232, 232];
    if kappa < 0.0 {
        // #e8e8e8 → #d62728
        hex8([
            lerp_u8(GREY[0], 214, -kappa),
            lerp_u8(GREY[1], 39, -kappa),
            lerp_u8(GREY[2], 40, -kappa),
        ])
    } else {
        // #e8e8e8 → #1f77b4
        hex8([
            lerp_u8(GREY[0], 31, kappa),
            lerp_u8(GREY[1], 119, kappa),
            lerp_u8(GREY[2], 180, kappa),
        ])
    }
}

/// Couleur d'arête phéromone : gris clair (0) → orange (#e6550d) à l'intensité
/// maximale du sous-graphe.
#[must_use]
pub fn pheromone_color(rel: f64) -> String {
    const GREY: [u8; 3] = [232, 232, 232];
    const ORANGE: [u8; 3] = [230, 85, 13];
    let t = rel.clamp(0.0, 1.0);
    hex8([
        lerp_u8(GREY[0], ORANGE[0], t),
        lerp_u8(GREY[1], ORANGE[1], t),
        lerp_u8(GREY[2], ORANGE[2], t),
    ])
}

/// Couleur pastel déterministe pour un `scc_id` (teinte = FNV-1a, saturation
/// 45 %, valeur 95 %).
#[must_use]
pub fn pastel_for_id(scc_id: u64) -> String {
    let h = (fnv1a64(&format!("scc:{scc_id}")) % 360) as f64 / 360.0;
    hex8(hsv_to_rgb(h, 0.45, 0.95))
}

fn lerp_u8(lo: u8, hi: u8, f: f64) -> u8 {
    let f = f.clamp(0.0, 1.0);
    (f64::from(lo) + (f64::from(hi) - f64::from(lo)) * f).round() as u8
}

fn hex8(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

fn hsv_to_rgb(h: f64, s: f64, v: f64) -> [u8; 3] {
    let i = (h * 6.0).floor() as i32;
    let f = h * 6.0 - i as f64;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let tt = v * (1.0 - (1.0 - f) * s);
    let (r, g, b) = match i.rem_euclid(6) {
        0 => (v, tt, p),
        1 => (q, v, p),
        2 => (p, v, tt),
        3 => (p, q, v),
        4 => (tt, p, v),
        _ => (v, p, q),
    };
    [
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::{Overlay, apply_overlay, color_ramp, pastel_for_id, pheromone_color, ricci_color};
    use crate::ir::{Subgraph, VizEdge, VizNode};

    fn sample() -> Subgraph {
        let mut nodes = vec![
            VizNode::new("a"),
            VizNode::new("b"),
            VizNode::new("c"),
            VizNode::new("no-signal"),
        ];
        nodes[0].energy = Some(0.0);
        nodes[1].energy = Some(1.0);
        nodes[2].energy = Some(0.5);
        nodes[0].scc_id = Some(7);
        nodes[1].scc_id = Some(7);
        nodes[2].scc_id = Some(9);

        let mut edges = vec![
            VizEdge::new("a", "b", "CALLS"),
            VizEdge::new("b", "c", "CALLS"),
            VizEdge::new("c", "no-signal", "CALLS"),
        ];
        edges[0].ricci = Some(-1.0);
        edges[1].ricci = Some(1.0);
        edges[2].ricci = Some(-0.1);
        edges[2].stale = Some(true);

        Subgraph {
            nodes,
            edges,
            truncated: false,
        }
    }

    fn is_hex(s: &str) -> bool {
        s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
    }

    #[test]
    fn ramp_endpoints_and_midpoint() {
        assert_eq!(color_ramp(0.0), "#3b4cc0");
        assert_eq!(color_ramp(0.5), "#f5d63d");
        assert_eq!(color_ramp(1.0), "#e03b2f");
    }

    #[test]
    fn ricci_sign_maps_to_divergent_hues() {
        let neg = ricci_color(-1.0);
        let pos = ricci_color(1.0);
        assert!(is_hex(&neg) && is_hex(&pos));
        assert_ne!(neg, pos);
        // Neutre (0) = gris très clair, loin des extrêmes.
        assert_ne!(ricci_color(0.0), neg);
        assert_ne!(ricci_color(0.0), pos);
    }

    #[test]
    fn pastel_is_deterministic_and_hex() {
        assert_eq!(pastel_for_id(7), pastel_for_id(7));
        assert!(is_hex(&pastel_for_id(42)));
    }

    #[test]
    fn energy_overlay_styles_only_signal_nodes() {
        let (styled, report) = apply_overlay(&sample(), Overlay::Energy);
        assert_eq!(report.nodes_styled, 3);
        assert_eq!(report.edges_styled, 0);
        assert_eq!(report.overlay, "energy");
        assert!(!report.legend.is_empty());
        for node in &styled.nodes {
            if node.energy.is_some() {
                assert!(is_hex(node.color_hint.as_deref().unwrap()));
                assert!(node.size_hint.is_some());
            } else {
                assert!(node.color_hint.is_none());
            }
        }
    }

    #[test]
    fn energy_low_high_colors_differ() {
        let (styled, _) = apply_overlay(&sample(), Overlay::Energy);
        let low = styled.nodes.iter().find(|n| n.id == "a").unwrap();
        let high = styled.nodes.iter().find(|n| n.id == "b").unwrap();
        assert_ne!(low.color_hint, high.color_hint);
        assert!(low.size_hint.unwrap() < high.size_hint.unwrap());
    }

    #[test]
    fn roles_overlay_groups_by_scc() {
        let (styled, report) = apply_overlay(&sample(), Overlay::Roles);
        assert_eq!(report.nodes_styled, 3);
        let a = styled.nodes.iter().find(|n| n.id == "a").unwrap();
        let b = styled.nodes.iter().find(|n| n.id == "b").unwrap();
        let c = styled.nodes.iter().find(|n| n.id == "c").unwrap();
        assert_eq!(a.color_hint, b.color_hint); // même scc → même couleur
        assert_ne!(a.color_hint, c.color_hint); // scc différent
    }

    #[test]
    fn ricci_overlay_counts_stale_and_sizes() {
        let (styled, report) = apply_overlay(&sample(), Overlay::Ricci);
        assert_eq!(report.edges_styled, 3);
        assert_eq!(report.stale_edges, 1);
        let stale = styled.edges.iter().find(|e| e.src == "c").unwrap();
        assert_eq!(stale.color_hint.as_deref(), Some("#9e9e9e"));
        let neg = styled.edges.iter().find(|e| e.src == "a").unwrap();
        let pos = styled.edges.iter().find(|e| e.src == "b").unwrap();
        assert_ne!(neg.color_hint, pos.color_hint);
        // Taille proportionnelle à |κ| : κ=±1 → 4.0, κ=-0.1 → 1.3.
        let weak = styled.edges.iter().find(|e| e.src == "c").unwrap();
        assert_eq!(neg.size_hint, Some(4.0));
        assert_eq!(pos.size_hint, Some(4.0));
        assert!((weak.size_hint.unwrap() - 1.3).abs() < 1e-9);
    }

    #[test]
    fn apply_does_not_mutate_input() {
        let input = sample();
        let original = input.clone();
        let (_, _) = apply_overlay(&input, Overlay::Energy);
        assert_eq!(input, original);
    }

    #[test]
    fn overlay_is_deterministic() {
        let (a, ra) = apply_overlay(&sample(), Overlay::Ricci);
        let (b, rb) = apply_overlay(&sample(), Overlay::Ricci);
        assert_eq!(a, b);
        assert_eq!(ra, rb);
    }

    fn volatile_sample() -> Subgraph {
        let mut n1 = VizNode::new("a");
        n1.fsrs_stability = Some(5.0);
        n1.fsrs_difficulty = Some(3.0);
        n1.engram_id = Some("e7".into());
        let mut n2 = VizNode::new("b");
        n2.fsrs_stability = Some(20.0);
        n2.fsrs_difficulty = Some(10.0);
        let mut e1 = VizEdge::new("a", "b", "CALLS");
        e1.pheromone = Some(4.0);
        let mut e2 = VizEdge::new("b", "a", "CALLS");
        e2.pheromone = Some(1.0);
        Subgraph {
            nodes: vec![n1, n2],
            edges: vec![e1, e2],
            truncated: false,
        }
    }

    #[test]
    fn volatile_flags() {
        assert!(Overlay::Fsrs.is_volatile());
        assert!(Overlay::Engrams.is_volatile());
        assert!(Overlay::Pheromone.is_volatile());
        assert!(!Overlay::Ricci.is_volatile());
        assert!(!Overlay::Energy.is_volatile());
    }

    #[test]
    fn fsrs_overlay_maps_difficulty_and_stability() {
        let (styled, report) = apply_overlay(&volatile_sample(), Overlay::Fsrs);
        assert_eq!(report.overlay, "fsrs");
        assert_eq!(report.nodes_styled, 2);
        let a = styled.nodes.iter().find(|n| n.id == "a").unwrap();
        let b = styled.nodes.iter().find(|n| n.id == "b").unwrap();
        assert!(a.color_hint.is_some() && b.color_hint.is_some());
        assert_ne!(a.color_hint, b.color_hint); // difficulté 3 vs 10
        assert!(b.size_hint.unwrap() > a.size_hint.unwrap()); // stabilité 20 vs 5
        assert!(is_hex(a.color_hint.as_deref().unwrap()));
        assert!(!report.legend.is_empty());
    }

    #[test]
    fn engrams_overlay_colors_by_group() {
        let (styled, report) = apply_overlay(&volatile_sample(), Overlay::Engrams);
        assert_eq!(report.nodes_styled, 1); // seul a porte engram_id
        let a = styled.nodes.iter().find(|n| n.id == "a").unwrap();
        let b = styled.nodes.iter().find(|n| n.id == "b").unwrap();
        assert!(a.color_hint.is_some());
        assert_eq!(b.color_hint, None);
        assert_eq!(
            apply_overlay(&volatile_sample(), Overlay::Engrams).0,
            styled
        );
    }

    #[test]
    fn pheromone_overlay_normalizes_to_subgraph_max() {
        let (styled, report) = apply_overlay(&volatile_sample(), Overlay::Pheromone);
        assert_eq!(report.edges_styled, 2);
        let max = styled.edges.iter().find(|e| e.src == "a").unwrap();
        let weak = styled.edges.iter().find(|e| e.src == "b").unwrap();
        assert_eq!(
            max.color_hint.as_deref(),
            Some(pheromone_color(1.0).as_str())
        );
        assert_eq!(
            weak.color_hint.as_deref(),
            Some(pheromone_color(0.25).as_str())
        );
        assert_eq!(max.size_hint, Some(4.0));
        // max absent → aucun style (pas de division par zéro).
        let (plain, rep) = apply_overlay(&sample(), Overlay::Pheromone);
        assert_eq!(rep.edges_styled, 0);
        assert!(plain.edges.iter().all(|e| e.color_hint.is_none()));
    }

    #[test]
    fn tier_overlay_classifies_by_mass_quantiles() {
        use crate::ir::{Subgraph, VizEdge, VizNode};
        let mut sg = Subgraph::default();
        for (i, e) in [0.95_f64, 0.6, 0.3, 0.05, 0.0].iter().enumerate() {
            let mut n = VizNode::new(format!("n{i}"));
            n.energy = Some(*e);
            sg.nodes.push(n);
        }
        // n0 fortement connecté → degré renforce la masse.
        sg.edges.push(VizEdge::new("n0", "n1", "calls"));
        sg.edges.push(VizEdge::new("n0", "n2", "calls"));
        let (styled, report) = apply_overlay(&sg, Overlay::Tier);
        assert_eq!(report.overlay, "tier");
        assert_eq!(report.nodes_styled, 5);
        // Le plus massif est chaud, le nul est froid.
        assert_eq!(styled.nodes[0].color_hint.as_deref(), Some("#ff5c4d"));
        assert_eq!(styled.nodes[4].color_hint.as_deref(), Some("#3b4cc0"));
        // Déterminisme : deux applications identiques.
        let (styled2, _) = apply_overlay(&sg, Overlay::Tier);
        assert_eq!(styled, styled2);
    }
}
