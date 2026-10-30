//! Métriques structurelles par fonction/méthode — équivalent Rust de
//! `cbm_compute_complexity` (codebase-memory-mcp internal/cbm/helpers.c).
//!
//! Un seul walk itératif de l'AST produit :
//! - `complexity` : complexité cyclomatique = nombre de nœuds de branchement
//!   (types `branch` de la spec : if/for/while/switch/case…).
//! - `cognitive` : complexité cognitive Campbell — chaque branche ajoute
//!   1 + profondeur d'imbrication de branches (pénalité d'imbrication).
//! - `loop_count` / `loop_depth` : boucles (nœuds *named* seulement — les
//!   grammaires où `for`/`while` est un jeton anonyme ne comptent qu'une fois)
//!   et profondeur maximale d'imbrication de boucles (proxy degré polynomial).
//! - `max_access_depth` : plus longue chaîne d'accès membre/subscript
//!   (a.b.c.d) — smell structurel (le C : is_member_access_node).

use tree_sitter::Node;

/// Métriques d'une fonction/méthode (0 = non calculé / spec sans branch).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Metrics {
    pub complexity: u32,
    pub cognitive: u32,
    pub loop_count: u32,
    pub loop_depth: u32,
    pub max_access_depth: u32,
}

/// Types de nœuds de boucle (liste générique multi-langages du C).
const LOOP_KINDS: &[&str] = &[
    "for_statement",
    "while_statement",
    "do_statement",
    "do_while_statement",
    "for_in_statement",
    "for_of_statement",
    "for_each_statement",
    "foreach_statement",
    "enhanced_for_statement",
    "for_range_loop",
    "c_style_for_statement",
    "for_expression",
    "while_expression",
    "loop_expression",
    "while_let_expression",
    "repeat_statement",
    "repeat_while_statement",
    "until",
    "while_modifier",
    "until_modifier",
    "for",
    "while",
];

/// Types de nœuds d'accès membre/subscript chaînés (a.b.c.d).
const ACCESS_KINDS: &[&str] = &[
    "member_expression",
    "field_expression",
    "selector_expression",
    "field_access",
    "member_access_expression",
    "navigation_expression",
    "attribute",
    "subscript_expression",
    "subscript",
    "index_expression",
    "call_expression",
];

fn kind_in(kind: &str, list: &[&str]) -> bool {
    list.contains(&kind)
}

/// Calcule les métriques sur un nœud de définition (fonction/méthode) dont
/// le sous-arbre contient le corps. `branch_types` : les types de nœuds de
/// branchement de la spec du langage (`LangSpec.branch`).
pub fn compute(node: Node<'_>, branch_types: &[&str]) -> Metrics {
    let mut m = Metrics::default();
    if branch_types.is_empty() {
        return m;
    }
    // Pile de frames {node, bdepth, ldepth, adepth} — même structure que le C.
    struct Frame<'a> {
        node: Node<'a>,
        bdepth: u32,
        ldepth: u32,
        adepth: u32,
    }
    let mut stack: Vec<Frame> = vec![Frame {
        node,
        bdepth: 0,
        ldepth: 0,
        adepth: 0,
    }];
    while let Some(f) = stack.pop() {
        let kind = f.node.kind();
        let is_branch = kind_in(kind, branch_types);
        let mut child_b = f.bdepth;
        let mut child_l = f.ldepth;
        let mut child_a = 0u32;
        // Accès chaînés : chaque nœud d'accès consécutif approfondit ; les
        // autres nœuds réinitialisent.
        if f.node.is_named() && kind_in(kind, ACCESS_KINDS) {
            child_a = f.adepth + 1;
            if child_a > m.max_access_depth {
                m.max_access_depth = child_a;
            }
        }
        if is_branch {
            m.complexity += 1;
            m.cognitive += 1 + f.bdepth; // +1 + pénalité d'imbrication
            child_b = f.bdepth + 1;
        }
        // Boucles : seulement les nœuds *named* (le mot-clé for/while anonyme
        // dans C/Go ne doit pas doubler le compte ni gonfler la profondeur).
        if f.node.is_named() && kind_in(kind, LOOP_KINDS) {
            m.loop_count += 1;
            let d = f.ldepth + 1;
            if d > m.loop_depth {
                m.loop_depth = d;
            }
            child_l = d;
        }
        let mut cur = f.node.walk();
        let children: Vec<Node> = f.node.children(&mut cur).collect();
        for c in children.into_iter().rev() {
            stack.push(Frame {
                node: c,
                bdepth: child_b,
                ldepth: child_l,
                adepth: child_a,
            });
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> tree_sitter::Tree {
        let mut p = tree_sitter::Parser::new();
        p.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
        p.parse(src, None).unwrap()
    }

    fn first_func<'a>(n: Node<'a>, func_kinds: &[&str]) -> Option<Node<'a>> {
        if func_kinds.contains(&n.kind()) {
            return Some(n);
        }
        let mut cur = n.walk();
        for c in n.children(&mut cur) {
            if let Some(f) = first_func(c, func_kinds) {
                return Some(f);
            }
        }
        None
    }

    #[test]
    fn rust_if_else_compte_branches() {
        let src = "fn f(a: i32) -> i32 {\n    if a > 0 { 1 } else { 0 }\n}\n";
        let root = parse(src);
        let f = first_func(root.root_node(), &["function_item"]).unwrap();
        // rust branch: if_expression, match_expression... le if_expression est
        // un seul nœud (pas if_statement séparé) → complexity 1.
        let spec = crate::lang_specs::LANG_SPECS
            .iter()
            .find(|s| s.name == "rust")
            .unwrap();
        let m = compute(f, spec.branch);
        assert_eq!(m.complexity, 1, "if_expression = 1 branche");
        assert!(m.cognitive >= 1);
    }

    #[test]
    fn rust_boucle_for_compte_loop() {
        let src = "fn f() {\n    for i in 0..10 {}\n}\n";
        let root = parse(src);
        let f = first_func(root.root_node(), &["function_item"]).unwrap();
        let spec = crate::lang_specs::LANG_SPECS
            .iter()
            .find(|s| s.name == "rust")
            .unwrap();
        let m = compute(f, spec.branch);
        assert_eq!(m.loop_count, 1, "for_expression = 1 boucle");
        assert_eq!(m.loop_depth, 1);
    }

    #[test]
    fn rust_boucles_imbriquees_profondeur_2() {
        let src = "fn f() {\n    for i in 0..10 {\n        for j in 0..10 {}\n    }\n}\n";
        let root = parse(src);
        let f = first_func(root.root_node(), &["function_item"]).unwrap();
        let spec = crate::lang_specs::LANG_SPECS
            .iter()
            .find(|s| s.name == "rust")
            .unwrap();
        let m = compute(f, spec.branch);
        assert_eq!(m.loop_count, 2);
        assert_eq!(m.loop_depth, 2, "2 boucles imbriquées");
    }
}

/// Types de nœuds de branchement de la spec d'un langage (pour les langages
/// manuels qui n'ont pas la spec sous la main).
pub fn branch_types(lang_name: &str) -> &'static [&'static str] {
    crate::lang_specs::LANG_SPECS
        .iter()
        .find(|s| s.name == lang_name)
        .map(|s| s.branch)
        .unwrap_or(&[])
}

/// Calcule les métriques d'un nœud de fonction pour un langage manuel —
/// retourne None si la spec n'a pas de types de branchement.
pub fn for_function<'a>(node: Node<'a>, lang_name: &str) -> Option<Metrics> {
    let branch = branch_types(lang_name);
    if branch.is_empty() {
        return None;
    }
    Some(compute(node, branch))
}

#[cfg(test)]
mod tests_manual_langs {
    use super::*;

    #[test]
    fn c_spec_a_des_branches() {
        let b = branch_types("c");
        assert!(!b.is_empty(), "spec C doit avoir des branch types");
        assert!(b.contains(&"if_statement"));
    }
}

/// Passe interprocédurale post-résolution (équivalent pass_complexity du C) :
/// pour chaque Function/Method, calcule `transitive_loop_depth` (loop_depth +
/// max sur les callees transitifs le long des CALLS) et `recursive` (le nœud
/// participe à un cycle du call graph). Écrit les 2 propriétés dans
/// props.extra du nœud.
pub fn complexity_pass(graph: &mut crate::graph::CodeGraph) {
    // (T27) Tableaux DENSES au lieu de `HashMap<NodeId, _>` : les identifiants
    // sont attribués séquentiellement et jamais retirés, donc « position =
    // id − premier id ». Sur le noyau Linux (1,5 M nœuds, 3,8 M CALLS), ce sont
    // des millions d'opérations de hachage SipHash en moins.
    //
    // Même parcours en profondeur, même règle : un nœud revisité sur la pile
    // marque TOUTE la pile récursive et contribue 0. L'ordre des racines était
    // celui d'une `HashMap` (donc aléatoire, et les résultats sur les cycles en
    // dépendaient) ; c'est désormais l'ordre des positions — l'un des résultats
    // que l'ancien code pouvait rendre, mais reproductible.
    use crate::schema::{EdgeKind, NodeKind};
    let nodes = graph.nodes();
    let n = nodes.len();
    let premier = nodes.first().map(|x| x.id.0).unwrap_or(0);
    let dense = nodes.iter().enumerate().all(|(i, x)| x.id.0 == premier + i as u64);
    if !dense {
        return complexity_pass_generique(graph);
    }
    let pos = |id: crate::graph::NodeId| -> Option<usize> {
        let p = id.0.checked_sub(premier)? as usize;
        (p < n).then_some(p)
    };
    let mut loop_depth: Vec<Option<u32>> = vec![None; n];
    for (i, x) in nodes.iter().enumerate() {
        if x.kind == NodeKind::Function || x.kind == NodeKind::Method {
            let ld = x.props.extra.get("loop_depth").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            loop_depth[i] = Some(ld);
        }
    }
    // Adjacence CALLS en CSR, dans l'ordre d'insertion des arêtes.
    let mut degre = vec![0u32; n + 1];
    let mut appels: Vec<(usize, usize)> = Vec::new();
    for e in graph.edges() {
        if e.kind == EdgeKind::Calls
            && let (Some(a), Some(b)) = (pos(e.source), pos(e.target))
        {
            degre[a + 1] += 1;
            appels.push((a, b));
        }
    }
    for i in 0..n {
        degre[i + 1] += degre[i];
    }
    let mut cibles = vec![0u32; appels.len()];
    let mut curseur = degre.clone();
    for (a, b) in appels {
        cibles[curseur[a] as usize] = b as u32;
        curseur[a] += 1;
    }
    let mut state = vec![0u8; n];
    let mut tld = vec![0u32; n];
    let mut rec = vec![false; n];

    #[allow(clippy::too_many_arguments)]
    fn dfs(
        id: usize,
        loop_depth: &[Option<u32>],
        degre: &[u32],
        cibles: &[u32],
        state: &mut [u8],
        tld: &mut [u32],
        rec: &mut [bool],
        stack: &mut Vec<usize>,
    ) -> u32 {
        match state[id] {
            2 => return tld[id],
            1 => {
                for s in stack.iter() {
                    rec[*s] = true;
                }
                rec[id] = true;
                return 0;
            }
            _ => {}
        }
        state[id] = 1;
        stack.push(id);
        let ld = loop_depth[id].unwrap_or(0);
        let mut best = 0u32;
        for k in degre[id]..degre[id + 1] {
            let r = dfs(cibles[k as usize] as usize, loop_depth, degre, cibles, state, tld, rec, stack);
            if r > best {
                best = r;
            }
        }
        stack.pop();
        state[id] = 2;
        let total = ld + best;
        tld[id] = total;
        total
    }

    let mut stack = Vec::new();
    for id in 0..n {
        if loop_depth[id].is_some() {
            dfs(id, &loop_depth, &degre, &cibles, &mut state, &mut tld, &mut rec, &mut stack);
        }
    }
    for (i, x) in graph.nodes_mut().iter_mut().enumerate() {
        if state[i] == 2 {
            x.props.extra.insert("transitive_loop_depth".into(), serde_json::Value::from(tld[i]));
            x.props.extra.insert("recursive".into(), serde_json::Value::Bool(rec[i]));
        }
    }
}

/// Repli si les identifiants ne sont pas denses (ne se produit pas avec
/// `CodeGraph::upsert_node`, mais la passe ne doit jamais se tromper en silence).
fn complexity_pass_generique(graph: &mut crate::graph::CodeGraph) {
    // 1. loop_depth local par id de nœud (fonctions/méthodes).
    let mut loop_depth: std::collections::HashMap<crate::graph::NodeId, u32> = Default::default();
    for n in graph.nodes() {
        if n.kind == crate::schema::NodeKind::Function || n.kind == crate::schema::NodeKind::Method
        {
            let ld = n
                .props
                .extra
                .get("loop_depth")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            loop_depth.insert(n.id, ld);
        }
    }
    // 2. Call graph : callees par caller.
    let mut callees: std::collections::HashMap<crate::graph::NodeId, Vec<crate::graph::NodeId>> =
        Default::default();
    for e in graph.edges() {
        if e.kind == crate::schema::EdgeKind::Calls {
            callees.entry(e.source).or_default().push(e.target);
        }
    }
    // 3. DFS mémoïsé : tld(id) = loop_depth(id) + max(tld(callee)).
    // États : 0=visite, 1=en cours (cycle), 2=fait.
    let mut state: std::collections::HashMap<crate::graph::NodeId, u8> = Default::default();
    let mut tld: std::collections::HashMap<crate::graph::NodeId, u32> = Default::default();
    let mut rec: std::collections::HashMap<crate::graph::NodeId, bool> = Default::default();

    fn dfs(
        id: crate::graph::NodeId,
        loop_depth: &std::collections::HashMap<crate::graph::NodeId, u32>,
        callees: &std::collections::HashMap<crate::graph::NodeId, Vec<crate::graph::NodeId>>,
        state: &mut std::collections::HashMap<crate::graph::NodeId, u8>,
        tld: &mut std::collections::HashMap<crate::graph::NodeId, u32>,
        rec: &mut std::collections::HashMap<crate::graph::NodeId, bool>,
        stack: &mut Vec<crate::graph::NodeId>,
    ) -> u32 {
        match state.get(&id) {
            Some(2) => return *tld.get(&id).unwrap_or(&0),
            Some(1) => {
                // back-edge → cycle : tous les nœuds du stack courant sont
                // récursifs.
                for s in stack.iter() {
                    rec.insert(*s, true);
                }
                rec.insert(id, true);
                return 0;
            }
            _ => {}
        }
        state.insert(id, 1);
        stack.push(id);
        let ld = *loop_depth.get(&id).unwrap_or(&0);
        let mut best = 0u32;
        if let Some(cs) = callees.get(&id) {
            for c in cs {
                let r = dfs(*c, loop_depth, callees, state, tld, rec, stack);
                if r > best {
                    best = r;
                }
            }
        }
        stack.pop();
        state.insert(id, 2);
        let total = ld + best;
        tld.insert(id, total);
        total
    }

    let ids: Vec<crate::graph::NodeId> = loop_depth.keys().copied().collect();
    let mut stack = Vec::new();
    for id in ids {
        dfs(
            id,
            &loop_depth,
            &callees,
            &mut state,
            &mut tld,
            &mut rec,
            &mut stack,
        );
    }

    // 4. Écriture dans props.extra.
    for n in graph.nodes_mut() {
        if let Some(t) = tld.get(&n.id) {
            n.props
                .extra
                .insert("transitive_loop_depth".into(), serde_json::Value::from(*t));
            let r = rec.get(&n.id).copied().unwrap_or(false);
            n.props
                .extra
                .insert("recursive".into(), serde_json::Value::Bool(r));
        }
    }
}

#[cfg(test)]
mod tests_complexity_pass {
    use super::*;
    use crate::graph::{CodeGraph, EdgeProps, NodeId, NodeProps};
    use crate::schema::{EdgeKind, NodeKind};

    fn node(g: &mut CodeGraph, kind: NodeKind, name: &str, ld: u32) -> NodeId {
        let mut props = NodeProps::default();
        if ld > 0 {
            props
                .extra
                .insert("loop_depth".into(), serde_json::Value::from(ld));
        }
        props.name = Some(name.to_string());
        g.upsert_node(kind, name.into(), props)
    }

    #[test]
    fn tld_se_propage_le_long_des_calls() {
        let mut g = CodeGraph::new();
        let f1 = node(&mut g, NodeKind::Function, "mod.boucle", 1);
        let f2 = node(&mut g, NodeKind::Function, "mod.appelle", 0);
        g.insert_edge(EdgeKind::Calls, f2, f1, EdgeProps::default());
        complexity_pass(&mut g);
        let get = |qn: &str| -> (u32, bool) {
            let n = g.node_by_qn(qn).unwrap();
            let tld = n
                .props
                .extra
                .get("transitive_loop_depth")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let rec = n
                .props
                .extra
                .get("recursive")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            (tld, rec)
        };
        assert_eq!(get("mod.boucle"), (1, false), "boucle: tld=1");
        assert_eq!(get("mod.appelle"), (1, false), "appelle: tld hérité=1");
    }

    #[test]
    fn recursion_detectee() {
        let mut g = CodeGraph::new();
        let f = node(&mut g, NodeKind::Function, "mod.rec", 0);
        g.insert_edge(EdgeKind::Calls, f, f, EdgeProps::default());
        complexity_pass(&mut g);
        let n = g.node_by_qn("mod.rec").unwrap();
        assert_eq!(
            n.props.extra.get("recursive").and_then(|v| v.as_bool()),
            Some(true)
        );
    }
}

/// Passe TESTS (équivalent pass_tests du C) : scanne les CALLS — quand le
/// caller est un test (is_test=true) et le callee ne l'est pas, crée un edge
/// TESTS caller→callee. Les agents peuvent ainsi voir ce qu'un test couvre.
pub fn tests_pass(graph: &mut crate::graph::CodeGraph) {
    // is_test par id (colonne si déjà construite, sinon hash lookup).
    let use_col = graph.has_metric_column("is_test");
    let is_test_of: std::collections::HashMap<crate::graph::NodeId, bool> = graph
        .nodes()
        .iter()
        .map(|n| {
            let t = if use_col {
                graph.metric_value(n.id, "is_test") != 0
            } else {
                n.props
                    .extra
                    .get("is_test")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            };
            (n.id, t)
        })
        .collect();

    // CALLS où source=test et target≠test → TESTS. Itère les CALLS indexés
    // (by_edge_kind) — pas les 4,8 M edges du kernel.
    let mut edges: Vec<(crate::graph::NodeId, crate::graph::NodeId)> = Vec::new();
    for &epos in graph.edge_positions(crate::schema::EdgeKind::Calls) {
        let e = &graph.edges()[epos];
        let src_test = is_test_of.get(&e.source).copied().unwrap_or(false);
        let dst_test = is_test_of.get(&e.target).copied().unwrap_or(false);
        if src_test && !dst_test {
            edges.push((e.source, e.target));
        }
    }

    for (src, tgt) in edges {
        graph.insert_edge(
            crate::schema::EdgeKind::Tests,
            src,
            tgt,
            crate::graph::EdgeProps::default(),
        );
    }
}

#[cfg(test)]
mod tests_tests_pass {
    use super::*;
    use crate::graph::{CodeGraph, EdgeProps, NodeProps};
    use crate::schema::{EdgeKind, NodeKind};

    #[test]
    fn test_fonction_cree_edge_tests() {
        let mut g = CodeGraph::new();
        // test_prod appelle prod (test → pas test) → TESTS.
        let mut p1 = NodeProps {
            name: Some("mod.test_foo".into()),
            ..Default::default()
        };
        p1.extra
            .insert("is_test".into(), serde_json::Value::Bool(true));
        let f1 = g.upsert_node(NodeKind::Function, "mod.test_foo".into(), p1);
        let mut p2 = NodeProps {
            name: Some("mod.foo".into()),
            ..Default::default()
        };
        p2.extra
            .insert("is_test".into(), serde_json::Value::Bool(false));
        let f2 = g.upsert_node(NodeKind::Function, "mod.foo".into(), p2);
        g.insert_edge(EdgeKind::Calls, f1, f2, EdgeProps::default());
        tests_pass(&mut g);
        let n_tests = g
            .edges()
            .iter()
            .filter(|e| e.kind == EdgeKind::Tests)
            .count();
        assert_eq!(n_tests, 1, "test→prod doit créer 1 edge TESTS");
    }

    #[test]
    fn pas_de_tests_entre_prods() {
        let mut g = CodeGraph::new();
        let f1 = g.upsert_node(NodeKind::Function, "mod.a".into(), NodeProps::default());
        let f2 = g.upsert_node(NodeKind::Function, "mod.b".into(), NodeProps::default());
        g.insert_edge(EdgeKind::Calls, f1, f2, EdgeProps::default());
        tests_pass(&mut g);
        let n_tests = g
            .edges()
            .iter()
            .filter(|e| e.kind == EdgeKind::Tests)
            .count();
        assert_eq!(n_tests, 0, "prod→prod ne crée pas TESTS");
    }
}
