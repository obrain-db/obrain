//! Requêtes « agent » sur le graphe enrichi M6 — INDEXÉES (timing par requête).
//!
//! Usage : cargo run --release -p obrain-indexer --example queries -- <repo>
//!
//! Utilise by_kind (positions par type) + top_by_metric (BinaryHeap partiel)
//! — pas de scan des 1,5 M nœuds ni de tri complet.

use obrain_indexer::index::{IndexOptions, index_repository};
use obrain_indexer::schema::{EdgeKind, NodeKind};
use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    let root = std::path::Path::new(&path);

    let t_idx = Instant::now();
    let out = index_repository(root, &IndexOptions::default());
    let g = &out.graph;
    println!(
        "=== {} : {} fichiers | {} nœuds | {} arêtes ===",
        path,
        out.files_scanned,
        g.node_count(),
        g.edge_count()
    );
    println!("  ⏱ indexation: {:.2} s", t_idx.elapsed().as_secs_f64());

    // 1. Hotspots : top 5 cognitive parmi les fonctions (BinaryHeap partiel).
    let t = Instant::now();
    let hot = g.top_k_cached(NodeKind::Function, "cognitive", 5);
    println!("\n— TOP HOTSPOTS (cognitive, index by_kind) —");
    for (cog, pos) in &hot {
        let n = &g.nodes()[*pos];
        let cx = n
            .props
            .extra
            .get("complexity")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let name = n.props.name.clone().unwrap_or_default();
        let file = n.props.file_path.clone().unwrap_or_default();
        let base = file.rsplit('/').next().unwrap_or(&file);
        println!("  cog={cog} cx={cx}  {name}  [{base}]");
    }
    println!(
        "  ⏱ query hotspots: {:.1} ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // 2. Récursives : count sur les seules fonctions (index kind).
    let t = Instant::now();
    let rec = g.count_bool_metric(NodeKind::Function, "recursive");
    println!("\n— RÉCURSION : {rec} fonctions récursives —");
    println!(
        "  ⏱ query récursion: {:.1} ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // 3. Boucles transitives : top 5 tld.
    let t = Instant::now();
    let tld_top = g.top_k_cached(NodeKind::Function, "transitive_loop_depth", 5);
    println!("\n— BOUCLES TRANSITIVES (tld desc) —");
    for (tv, pos) in &tld_top {
        let n = &g.nodes()[*pos];
        let ld = n
            .props
            .extra
            .get("loop_depth")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let name = n.props.name.clone().unwrap_or_default();
        println!("  tld={tv} (ld={ld}) {name}");
    }
    println!(
        "  ⏱ query boucles transitives: {:.1} ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // 4. Tests : edges TESTS (comptés à l'indexation — scan edges OK, moins
    // nombreux que les nœuds).
    let t = Instant::now();
    let tests = g.edge_count_kind(EdgeKind::Tests);
    let test_fns = g.count_bool_metric(NodeKind::Function, "is_test");
    println!("\n— TESTS : {test_fns} fns de test → {tests} edges TESTS —");
    println!(
        "  ⏱ query tests: {:.1} ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // 5. Co-évolution (edges FILE_CHANGES_WITH).
    let t = Instant::now();
    let mut cw: Vec<_> = g
        .edge_positions(EdgeKind::FileChangesWith)
        .iter()
        .filter_map(|&epos| {
            let e = &g.edges()[epos];
            let score = e.props.extra.get("coupling_score")?.as_f64()?;
            let co = e
                .props
                .extra
                .get("co_changes")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let src = g
                .nodes()
                .iter()
                .find(|n| n.id == e.source)?
                .props
                .file_path
                .clone()?;
            let dst = g
                .nodes()
                .iter()
                .find(|n| n.id == e.target)?
                .props
                .file_path
                .clone()?;
            Some((score, co, src, dst))
        })
        .collect();
    cw.sort_by_key(|e| std::cmp::Reverse(e.1));
    println!("\n— CO-ÉVOLUTION (top FILE_CHANGES_WITH) —");
    for (score, co, a, b) in cw.iter().take(5) {
        let (a, b) = (
            a.rsplit('/').next().unwrap_or(a),
            b.rsplit('/').next().unwrap_or(b),
        );
        println!("  {a} ↔ {b} : {co} co-chg (score {score:.2})");
    }
    println!(
        "  ⏱ query co-évolution: {:.1} ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // 6. Fichiers les plus modifiés (index kind File).
    let t = Instant::now();
    let chg_top = g.top_k_cached(NodeKind::File, "change_count", 5);
    println!("\n— FICHIERS LES PLUS MODIFIÉS —");
    for (c, pos) in &chg_top {
        let p = g.nodes()[*pos].props.file_path.clone().unwrap_or_default();
        let base = p.rsplit('/').next().unwrap_or(&p);
        println!("  {c}× {base}");
    }
    println!(
        "  ⏱ query fichiers modifiés: {:.1} ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // 7. Docstrings : count par présence.
    let t = Instant::now();
    let with_doc = g.count_bool_metric(NodeKind::Function, "has_docstring")
        + g.count_bool_metric(NodeKind::Method, "has_docstring")
        + g.count_bool_metric(NodeKind::Class, "has_docstring");
    println!("\n— {with_doc} symboles avec docstring —");
    println!(
        "  ⏱ query docstrings: {:.1} ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // 8. Branch.
    let t = Instant::now();
    let mut nb = 0;
    for &pos in g.positions_of_kind(NodeKind::Branch) {
        let n = &g.nodes()[pos];
        nb += 1;
        let b = n
            .props
            .extra
            .get("branch")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let head = n
            .props
            .extra
            .get("head_sha")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        println!("\n— BRANCH indexée : {b} @ {:.8}", head);
    }
    if nb == 0 {
        println!("\n— pas de git (extraction tar) —");
    }
    println!(
        "  ⏱ query branch: {:.1} ms",
        t.elapsed().as_secs_f64() * 1000.0
    );
}
