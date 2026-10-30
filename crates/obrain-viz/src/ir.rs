//! Représentation interne d'un sous-graphe rendable + statistiques + validation.
//!
//! L'IR est agnostique au store : les nœuds sont adressés par un `id` opaque et
//! **stable** (fourni par l'extracteur — en pratique dérivé du qn), les signaux
//! cognitifs sont des champs optionnels normalisés (voir la roadmap, overlays).
//! Les champs facultatifs absents sont omis à la sérialisation (`skip_serializing_if`),
//! ce qui garde le schéma json-graph stable et compact.

use serde::{Deserialize, Serialize};

use crate::{VizError, VizResult};

/// ── EMISSION DES SIGNAUX f32 : LA PLUS COURTE FORME QUI REJOUE LE MEME f32 ──
///
/// Les signaux `ricci` (aretes) et `curvature` (noeuds) NAISSENT en f32 :
///
///   · `record.rs` : `(self.ricci_u8 as f32 / 127.5) - 1.0`  → le sous-type
///     `ricci_u8` est un octet, la valeur est f32 par construction ;
///   · `viz_extract.rs` : `node_curvature(store, ..).map(f64::from)` → `f64::from`
///     d'un f32, donc un f32 elargi.
///
/// Les champs de l'IR sont des `f64`, ce qui les faisait serialiser en DOUBLE
/// precision : `-0.8352941274642944` — dix-sept chiffres, dont les dix derniers
/// ne portent AUCUNE information absente de la source (l'octet `ricci_u8`).
///
/// Mesure sur `/api/viz/subgraph?center=…&hops=1` (`runtime-dev`, 500 noeuds /
/// 24 502 aretes) : la charge fait 2 314 165 octets, dont **454 944 octets pour
/// `ricci` seul**, a 18,6 octets par arete. Emettre la forme courte du meme f32
/// la ramene a ~247 800 — **−207 Ko, soit −9 % de la charge**, transmise ET
/// PARSEE par le moteur de rendu a chaque vue.
///
/// PERTE NULLE, et ce n'est pas une approximation : `serialize_f32` ecrit la
/// representation la plus courte qui rejoue EXACTEMENT le meme f32 (ryu). Sur
/// les 24 502 valeurs reelles, `float32(forme courte) == float32(original)`
/// pour **24 502 sur 24 502** — aucune exception. Ce qui disparait est le
/// remplissage du a l'elargissement, pas de la donnee.
///
/// `energy` N'EST PAS touchee : elle vient d'un Q1.15 (voir sa doc) et son
/// origine f32 n'est pas etablie. On ne raccourcit que ce qu'on a verifie.
fn f32_court<S>(valeur: &Option<f64>, s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match valeur {
        Some(x) => s.serialize_f32(*x as f32),
        None => s.serialize_none(),
    }
}

/// Relit un signal écrit par [`f32_court`] : élargit le f32 retrouvé en f64.
///
/// ── LE DÉFAUT QUE CETTE FONCTION CORRIGE (2026-09-15) ─────────────────────
///
/// `f32_court` écrit `-0.8352941` ; la relecture, elle, faisait un `f64`
/// ordinaire, donc rendait `-0.8352941` — un troisième nombre, qui n'est NI
/// l'original en mémoire (`-0.8352941274642944`) NI sa forme élargie. Écrire en
/// f32 sans relire en f32 casse l'aller-retour.
///
/// Ce n'était pas théorique : `subgraph_for_stream` relit le corps v1 mis en
/// cache par `/api/viz/subgraph` pour en tirer le `Subgraph`. La voie WebSocket
/// servait donc, pour `ricci` et `curvature`, des valeurs DIFFÉRENTES de celles
/// que la voie HTTP venait de servir sur la MÊME requête — deux écritures du
/// même sous-graphe qui ne disaient pas la même chose.
///
/// Symétrique par construction : ce que `f32_court` a écrit est exactement ce
/// que cette fonction restaure. Le `default` est requis par serde dès qu'un
/// `deserialize_with` est posé sur un `Option<T>` : sans lui, un champ ABSENT
/// deviendrait une erreur au lieu d'un `None`.
///
/// ── CONTRAT, ET CE QU'IL IMPOSE (à savoir avant de réutiliser cette fonction) ──
///
/// Relire en f32, c'est AFFIRMER que le champ EST un f32 : toute valeur f64
/// écrite à la main dans un corps json-graph est tronquée à la relecture. C'est
/// exact pour les deux seuls champs qui portent ces attributs — `ricci` (décodé
/// d'un `ricci_u8`) et `curvature` (issue de `node_curvature`, un f32) : leur
/// source ne porte déjà que 8 bits pour l'un, 24 pour l'autre, donc rien
/// n'est perdu. Ne PAS poser ces attributs sur un champ de précision réelle
/// (`weight`, `energy`, `size_hint`) sans établir la même origine.
fn f32_depuis_json<'de, D>(d: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let lu = Option::<f64>::deserialize(d)?;
    Ok(lu.map(|x| f64::from(x as f32)))
}

/// Nœud du sous-graphe rendable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VizNode {
    /// Identifiant stable et unique dans le sous-graphe (non vide).
    pub id: String,
    /// Qualified name dans le store (ex. `crate::module::func`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qn: Option<String>,
    /// Libellé d'affichage (défaut à la sérialisation graphique : `qn` puis `id`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Chemin du fichier source quand le nœud est un symbole indexé.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    /// Énergie nodale décodée (Q1.15 → f64 en [0, 1]) — overlay `energy`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub energy: Option<f64>,
    /// Courbure nodale agrégée (moyenne pondérée du ricci incident) — overlay `roles`.
    /// Émise en forme courte f32 (voir `f32_court`) : la source est un f32.
    #[serde(
        serialize_with = "f32_court",
        deserialize_with = "f32_depuis_json",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub curvature: Option<f64>,
    /// Id de composante fortement connexe — overlay `roles`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scc_id: Option<u64>,
    /// Communauté (colonne substrate, détection Leiden/communautaire) —
    /// représentation « spectrale » des voisinages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub community_id: Option<u64>,
    /// État engram (ex. `stable`, `fresh`) quand le nœud porte un engram.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engram_state: Option<String>,
    /// Appartenance engram (id d'engram persistant) — overlay `engrams`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engram_id: Option<String>,
    /// Stabilité FSRS-5 (jours, ~[0, 20]) — overlay `fsrs`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fsrs_stability: Option<f64>,
    /// Difficulté FSRS-5 (~[1, 10]) — overlay `fsrs`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fsrs_difficulty: Option<f64>,
    /// TYPE D'ENTITÉ (label du graphe : `Function`, `Method`, `File`,
    /// `Session`, `ToolCall`…).
    ///
    /// Sans lui, un client ne peut pas ADAPTER sa vue à ce qu'il regarde : il
    /// reçoit des nœuds interchangeables (qn, courbure, communauté) sans savoir
    /// si l'entité est une fonction, une session ou un appel d'outil. C'est la
    /// donnée structurelle qui manquait pour représenter chaque entité selon ce
    /// qu'elle est — et elle est collectée pendant le parcours qui tourne déjà,
    /// donc sans requête supplémentaire.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Indice de couleur (`#rrggbb`) posé par un overlay — attribut standard
    /// consommé par les writers (DOT `fillcolor`, Mermaid `style fill`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_hint: Option<String>,
    /// Indice de taille (échelle positive normalisée) posé par un overlay —
    /// attribut standard (rendu édge `penwidth`/`stroke-width`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_hint: Option<f64>,
}

impl VizNode {
    /// Construit un nœud avec uniquement l'id (ids vides rejetés par les writers).
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            qn: None,
            label: None,
            file_path: None,
            energy: None,
            curvature: None,
            scc_id: None,
            community_id: None,
            engram_state: None,
            engram_id: None,
            fsrs_stability: None,
            fsrs_difficulty: None,
            kind: None,
            color_hint: None,
            size_hint: None,
        }
    }

    /// Chaîne d'affichage : `label` si présent, sinon `qn`, sinon `id`.
    #[must_use]
    pub fn display_label(&self) -> &str {
        self.label
            .as_deref()
            .or(self.qn.as_deref())
            .unwrap_or(&self.id)
    }
}

/// Arête du sous-graphe rendable. `src`/`dst` référencent des `VizNode::id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VizEdge {
    pub src: String,
    pub dst: String,
    /// Type de l'arête dans le store (ex. `CALLS`, `IMPORTS`).
    #[serde(rename = "type")]
    pub edge_type: String,
    /// Poids (synapse/energy edge, dépendances…) — optionnel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<f64>,
    /// Courbure Ricci-Ollivier de l'arête (persistée `ricci_u8` décodée) — overlay `ricci`.
    /// Émise en forme courte f32 (voir `f32_court`) : la source EST un f32.
    #[serde(
        serialize_with = "f32_court",
        deserialize_with = "f32_depuis_json",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub ricci: Option<f64>,
    /// L'arête est marquée `RICCI_STALE` (valeur ricci non fraîche) — overlay `ricci`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale: Option<bool>,
    /// Intensité phéromone (somme des trails stigmergy) — overlay `pheromone`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pheromone: Option<f64>,
    /// Indice de couleur (`#rrggbb`) posé par un overlay (ex. ricci) — attribut
    /// standard consommé par les writers (DOT `color`, Mermaid `linkStyle`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_hint: Option<String>,
    /// Indice de taille d'arête posé par un overlay (≥ 1, ex. `1 + 3·|κ|`) —
    /// attribut standard (DOT `penwidth`, Mermaid `stroke-width`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_hint: Option<f64>,
}

impl VizEdge {
    /// Construit une arête minimale.
    #[must_use]
    pub fn new(
        src: impl Into<String>,
        dst: impl Into<String>,
        edge_type: impl Into<String>,
    ) -> Self {
        Self {
            src: src.into(),
            dst: dst.into(),
            edge_type: edge_type.into(),
            weight: None,
            ricci: None,
            stale: None,
            pheromone: None,
            color_hint: None,
            size_hint: None,
        }
    }
}

/// Sous-graphe rendable : un appel `viz_subgraph` = une valeur de ce type.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Subgraph {
    pub nodes: Vec<VizNode>,
    pub edges: Vec<VizEdge>,
    /// Vrai si le parcours d'extraction a été borné (la sortie est un échantillon
    /// tronqué, jamais un sous-graphe complet). Les stats `dropped`/`visited`
    /// sont portées par l'extracteur.
    #[serde(default)]
    pub truncated: bool,
}

impl Subgraph {
    /// Sous-graphe vide, non tronqué.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Tri canonique (déterministe) : nœuds par `id`, arêtes par
    /// `(src, dst, type)`. Rend la sortie indépendante de l'ordre d'extraction.
    #[must_use]
    pub fn canonical(mut self) -> Self {
        self.nodes.sort_by(|a, b| a.id.cmp(&b.id));
        self.edges
            .sort_by(|a, b| (&a.src, &a.dst, &a.edge_type).cmp(&(&b.src, &b.dst, &b.edge_type)));
        self
    }

    /// Statistiques de l'en-tête d'export (recalculées depuis les vecteurs :
    /// toujours cohérentes avec le contenu).
    #[must_use]
    pub fn stats(&self) -> Stats {
        Stats {
            node_count: self.nodes.len(),
            edge_count: self.edges.len(),
            truncated: self.truncated,
        }
    }

    /// Vérifie le bien-formé : ids non vides et uniques, aucune arête pendante
    /// (chaque `src`/`dst` ∈ nodes). Retourne une erreur descriptive sinon.
    pub fn validate(&self) -> VizResult<()> {
        validate_ids(&self.nodes)?;
        validate_edges(&self.edges, &self.nodes)
    }
}

/// Statistiques d'export (`stats` de l'en-tête json-graph).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    pub node_count: usize,
    pub edge_count: usize,
    pub truncated: bool,
}

/// Valide l'unicité et la non-vacuité des ids de nœuds.
pub fn validate_ids(nodes: &[VizNode]) -> VizResult<()> {
    let mut seen = std::collections::HashSet::with_capacity(nodes.len());
    for node in nodes {
        if node.id.is_empty() {
            return Err(VizError::Validation("id de nœud vide".into()));
        }
        if !seen.insert(&node.id) {
            return Err(VizError::Validation(format!(
                "id de nœud dupliqué: {:?}",
                node.id
            )));
        }
    }
    Ok(())
}

/// Valide qu'aucune arête ne référence un nœud absent (arête pendante).
pub fn validate_edges(edges: &[VizEdge], nodes: &[VizNode]) -> VizResult<()> {
    let ids: std::collections::HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    for edge in edges {
        if !ids.contains(edge.src.as_str()) {
            return Err(VizError::Validation(format!(
                "arête pendante: src {:?} absent des nœuds (arête {} -> {} [{:?}])",
                edge.src, edge.src, edge.dst, edge.edge_type
            )));
        }
        if !ids.contains(edge.dst.as_str()) {
            return Err(VizError::Validation(format!(
                "arête pendante: dst {:?} absent des nœuds (arête {} -> {} [{:?}])",
                edge.dst, edge.src, edge.dst, edge.edge_type
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Subgraph, VizEdge, VizNode};

    fn sample() -> Subgraph {
        Subgraph {
            nodes: vec![VizNode::new("b"), VizNode::new("a"), VizNode::new("c")],
            edges: vec![
                VizEdge::new("c", "a", "CALLS"),
                VizEdge::new("a", "b", "IMPORTS"),
            ],
            truncated: false,
        }
    }

    /// ── L'ALLER-RETOUR D'UN SIGNAL f32 DOIT ÊTRE EXACT ───────────────────────
    ///
    /// `f32_court` écrit la forme courte du f32 (`-0.8352941`) et
    /// `f32_depuis_json` doit la RELIRE en f32. Si l'un des deux manque, la
    /// valeur qui revient n'est plus celle qui est partie : `-0.8352941` n'est
    /// ni l'original en mémoire (`-0.8352941274642944`) ni sa forme élargie.
    ///
    /// CE N'EST PAS DÉCORATIF. `subgraph_for_stream` (`viz_serve.rs`) relit le
    /// corps v1 mis en cache par `/api/viz/subgraph` pour en tirer le
    /// `Subgraph` et le comparer par ÉGALITÉ à un sous-graphe reconstruit
    /// (`compute_delta`). Avec une conversion asymétrique, les deux côtés
    /// portent des `ricci` différents alors que le store n'a pas bougé : le
    /// delta devient non vide et la voie WebSocket annonce des changements
    /// INEXISTANTS — deux écritures du même sous-graphe qui ne disent pas la
    /// même chose.
    ///
    /// La valeur de test est celle du store réel : `ricci_u8 = 199`, décodé
    /// `(199 as f32 / 127.5) - 1.0`, élargi en f64.
    #[test]
    fn aller_retour_des_signaux_f32_est_exact() {
        let ricci = f64::from((199u8 as f32 / 127.5) - 1.0);
        // Garde-fou sur la VALEUR DE TEST : elle doit porter des chiffres que la
        // forme courte du f32 ne dit pas — sinon le test serait vert pour de
        // mauvaises raisons (les deux formes seraient identiques).
        assert_ne!(
            format!("{ricci}"),
            format!("{}", ricci as f32),
            "valeur de test mal choisie : forme longue et forme courte coincident"
        );

        let mut e = VizEdge::new("a", "b", "CALLS");
        e.ricci = Some(ricci);
        let json = serde_json::to_string(&e).expect("sérialisation");
        let relu: VizEdge = serde_json::from_str(&json).expect("relecture");
        assert_eq!(
            relu.ricci,
            Some(ricci),
            "aller-retour rompu pour ricci — écrit {json}"
        );

        let mut n = VizNode::new("n");
        n.curvature = Some(ricci);
        let json = serde_json::to_string(&n).expect("sérialisation");
        let relu: VizNode = serde_json::from_str(&json).expect("relecture");
        assert_eq!(
            relu.curvature,
            Some(ricci),
            "aller-retour rompu pour curvature — écrit {json}"
        );

        // La forme courte est bien celle qu'on attend : sans cette assertion, le
        // test passerait aussi si `f32_court` n'était pas appliqué du tout.
        assert!(
            json.len() < 120,
            "charge inattendue, f32_court n'est peut-être plus appliqué : {json}"
        );

        // ── CHAMP ABSENT ET CHAMP NUL : `deserialize_with` NE DOIT PAS LES CASSER ──
        // La plupart des nœuds et des arêtes ne portent NI `ricci` NI `curvature`
        // (`skip_serializing_if` les omet). Si `deserialize_with` exigeait la clé,
        // TOUT corps mis en cache deviendrait illisible pour
        // `subgraph_for_stream` — une panne entière pour un champ facultatif.
        // C'est pour cela que l'attribut porte `default`.
        let sans: VizEdge = serde_json::from_str(r#"{"src":"a","dst":"b","type":"CALLS"}"#)
            .expect("un champ ABSENT doit rester un None, jamais une erreur");
        assert_eq!(sans.ricci, None);
        let nul: VizEdge = serde_json::from_str(r#"{"src":"a","dst":"b","type":"CALLS","ricci":null}"#)
            .expect("un champ NUL doit rester un None");
        assert_eq!(nul.ricci, None);
        let sans_noeud: VizNode = serde_json::from_str(r#"{"id":"n"}"#)
            .expect("idem pour un nœud sans curvature");
        assert_eq!(sans_noeud.curvature, None);
    }

    #[test]
    fn canonical_sorts_nodes_and_edges() {
        let canon = sample().canonical();
        let ids: Vec<&str> = canon.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        let keys: Vec<(&str, &str, &str)> = canon
            .edges
            .iter()
            .map(|e| (e.src.as_str(), e.dst.as_str(), e.edge_type.as_str()))
            .collect();
        assert_eq!(
            keys,
            [("a", "b", "IMPORTS"), ("c", "a", "CALLS")] // tri (src, dst, type)
        );
    }

    #[test]
    fn canonical_is_idempotent() {
        let once = sample().canonical();
        let twice = once.clone().canonical();
        assert_eq!(once, twice);
    }

    #[test]
    fn stats_are_recomputed() {
        let s = sample();
        let stats = s.stats();
        assert_eq!(stats.node_count, 3);
        assert_eq!(stats.edge_count, 2);
        assert!(!stats.truncated);
    }

    #[test]
    fn validate_rejects_dangling_edge() {
        let mut s = sample();
        s.edges.push(VizEdge::new("a", "ghost", "CALLS"));
        let err = s.validate().unwrap_err().to_string();
        assert!(err.contains("arête pendante"), "{err}");
        assert!(err.contains("ghost"), "{err}");
    }

    #[test]
    fn validate_rejects_duplicate_ids() {
        let mut s = sample();
        s.nodes.push(VizNode::new("a"));
        let err = s.validate().unwrap_err().to_string();
        assert!(err.contains("dupliqué"), "{err}");
    }

    #[test]
    fn validate_rejects_empty_id() {
        let mut s = sample();
        s.nodes.push(VizNode::new(""));
        assert!(s.validate().is_err());
    }

    #[test]
    fn valid_sample_passes() {
        assert!(sample().validate().is_ok());
    }

    #[test]
    fn display_label_falls_back_qn_then_id() {
        let mut n = VizNode::new("id1");
        assert_eq!(n.display_label(), "id1");
        n.qn = Some("crate::f".into());
        assert_eq!(n.display_label(), "crate::f");
        n.label = Some("affiché".into());
        assert_eq!(n.display_label(), "affiché");
    }
}
