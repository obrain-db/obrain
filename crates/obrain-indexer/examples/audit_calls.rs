//! # `audit_calls` — la justesse des arêtes CALLS, pas leur nombre
//!
//! `bench_index` compte les appels « résolus ». Ce comptage ne dit rien de
//! leur justesse : il dit que l'indexeur a trouvé une cible, pas qu'elle est
//! la bonne. Cet audit vérifie la **cohérence interne** de chaque résolution,
//! en confrontant la stratégie annoncée sur l'arête aux données du graphe.
//!
//! Trois invariants, un par stratégie :
//!
//! - `unique_name` : le nom court de la cible doit être porté par **un seul**
//!   nœud Function/Method du corpus. Si plusieurs le portent, la résolution
//!   n'aurait pas dû s'appeler « unique ».
//! - `same_module` : l'appelant et la cible doivent venir du **même fichier**
//!   (le module est le fichier dans ce corpus).
//! - `receiver_*` : l'appelant et la cible doivent partager au moins un
//!   répertoire — sinon un `self.method()` a été résolu dans un autre
//!   sous-système, ce qui est presque toujours faux.
//!
//! ## Usage
//! ```text
//! cargo run --release -p obrain-indexer --example audit_calls -- <depot>
//! ```

use std::collections::HashMap;
use std::path::PathBuf;

use obrain_indexer::index::{IndexOptions, index_repository};
use obrain_indexer::schema::{EdgeKind, NodeKind};

type BoxErr = Box<dyn std::error::Error + Send + Sync>;

fn main() -> Result<(), BoxErr> {
    let root: PathBuf = std::env::args()
        .nth(1)
        .ok_or_else(|| -> BoxErr { "usage: audit_calls <depot>".into() })?
        .into();
    eprintln!("audit_calls : indexation de {} …", root.display());
    let out = index_repository(&root, &IndexOptions::default());
    let g = &out.graph;

    // 1. Combien de nœuds portent chaque nom court ?
    let mut par_nom: HashMap<&str, usize> = HashMap::new();
    for n in g.nodes() {
        if matches!(n.kind, NodeKind::Function | NodeKind::Method)
            && let Some(nom) = n.props.name.as_deref()
        {
            *par_nom.entry(nom).or_insert(0) += 1;
        }
    }
    let noms_ambigus = par_nom.values().filter(|c| **c > 1).count();
    println!(
        "noms de fonction/méthode : {} distincts, dont {} portés par >1 nœud",
        par_nom.len(),
        noms_ambigus
    );

    let position: HashMap<_, _> = g
        .nodes()
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id, i))
        .collect();
    let mut total = 0usize;
    let mut par_strategie: HashMap<String, usize> = HashMap::new();
    let mut violations: HashMap<String, usize> = HashMap::new();
    let mut exemples: Vec<String> = Vec::new();

    for e in g.edges() {
        // SEULEMENT les arêtes d'appel : les ContainsFile/Defines/Imports
        // traversent légitimement les fichiers, les juger sur « même
        // fichier » produisait un taux de violations qui ne voulait rien dire.
        if !matches!(e.kind, EdgeKind::Calls) {
            continue;
        }
        let strat = e
            .props
            .extra
            .get("strategy")
            .and_then(|v| v.as_str())
            .unwrap_or("(aucune)");
        total += 1;

        let (Some(&si), Some(&ti)) = (position.get(&e.source), position.get(&e.target)) else {
            *par_strategie.entry(format!("{strat} [?]")).or_insert(0) += 1;
            continue;
        };
        let (src, dst) = (&g.nodes()[si], &g.nodes()[ti]);
        let (Some(nom_src), Some(nom_dst)) = (src.props.name.as_deref(), dst.props.name.as_deref())
        else {
            continue;
        };

        // Le langage change le sens de « module ». En C, un module est le
        // fichier ; en Python, `same_module` couvre aussi ce qui vient d'un
        // `import`, donc deux fichiers différents peuvent être légitimes.
        // On ne juge donc l'invariant « même fichier » que sur les langages
        // où il est vrai, et on rapporte les autres à part.
        let ext = dst
            .props
            .file_path
            .as_deref()
            .and_then(|p| p.rsplit('.').next())
            .unwrap_or("");
        let cle_stat = format!("{strat} [{ext}]");
        *par_strategie.entry(cle_stat.clone()).or_insert(0) += 1;
        let faute = match strat {
            "unique_name" => par_nom.get(nom_dst).copied().unwrap_or(0) > 1,
            "same_module" if matches!(ext, "c" | "h" | "cc" | "cpp" | "hpp" | "rs") => {
                src.props.file_path != dst.props.file_path
            }
            s if s.starts_with("receiver") => {
                let (a, b) = (
                    src.props.file_path.as_deref(),
                    dst.props.file_path.as_deref(),
                );
                match (a, b) {
                    (Some(a), Some(b)) => dossier(a) != dossier(b),
                    _ => false,
                }
            }
            _ => false,
        };
        if faute {
            *violations.entry(cle_stat).or_insert(0) += 1;
            if exemples.len() < 8 {
                exemples.push(format!(
                    "  [{strat}] {nom_src} ({}:{}) -> {nom_dst} ({}:{})",
                    src.props.file_path.as_deref().unwrap_or("?"),
                    src.props.start_line.unwrap_or(0),
                    dst.props.file_path.as_deref().unwrap_or("?"),
                    dst.props.start_line.unwrap_or(0),
                ));
            }
        }
    }

    // ── ÉCHANTILLON VÉRIFIÉ PAR LE SOURCE ────────────────────────────────
    // Les taux « violations » ci-dessus mesurent la cohérence entre la
    // stratégie annoncée et une propriété du graphe, pas la vérité du source.
    // Ici on va VÉRIFIER DANS LE FICHIER : pour une arête `same_module` en C,
    // le nom de la cible doit apparaître dans le fichier de l'appelant.
    //
    // Mesure CONSERVATRICE : si le nom est absent, l'arête est certainement
    // fausse. S'il est présent, l'arête est plausible — pas prouvée (un appel
    // peut viser un homonyme). Le taux d'absence est donc un MINORANT du taux
    // d'erreur réel.
    let n_ech: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(200);
    // ÉCHANTILLON RÉPARTI, PAS LES N PREMIERS : prendre les n premières
    // concentre la mesure sur le premier sous-système du parcours et rend le
    // taux non extrapolable. On collecte toutes les candidates, on en prend
    // une sur `pas` pour couvrir le corpus, et on compte les DOUBLONS
    // (même source et même cible), qui sont un défaut distinct.
    let toutes: Vec<&obrain_indexer::Edge> = g
        .edges()
        .iter()
        .filter(|e| matches!(e.kind, EdgeKind::Calls))
        .filter(|e| {
            e.props
                .extra
                .get("strategy")
                .and_then(|v| v.as_str())
                .map(|s| s == "same_module")
                .unwrap_or(false)
        })
        .collect();
    let mut vues: HashMap<(usize, usize), usize> = HashMap::new();
    for e in &toutes {
        *vues
            .entry((e.source.0 as usize, e.target.0 as usize))
            .or_insert(0) += 1;
    }
    let doublons = vues.values().filter(|c| **c > 1).count();
    println!(
        "\nsame_module : {} arêtes, {} couples (source,cible) distincts, {} DUPLIQUÉS",
        toutes.len(),
        vues.len(),
        doublons
    );
    let pas = (toutes.len() / n_ech.max(1)).max(1);
    let mut verif = 0usize;
    let mut absents = 0usize;
    let mut exemples_absents: Vec<String> = Vec::new();
    for (rang, e) in toutes.iter().enumerate() {
        if verif >= n_ech || rang % pas != 0 {
            continue;
        }
        let (Some(&si), Some(&ti)) = (position.get(&e.source), position.get(&e.target)) else {
            continue;
        };
        let (src, dst) = (&g.nodes()[si], &g.nodes()[ti]);
        let (Some(nom_dst), Some(chemin)) =
            (dst.props.name.as_deref(), src.props.file_path.as_deref())
        else {
            continue;
        };
        if !chemin.ends_with(".c") && !chemin.ends_with(".h") {
            continue;
        }
        verif += 1;
        let Ok(texte) = std::fs::read_to_string(root.join(chemin)) else {
            continue;
        };
        let present = texte.contains(nom_dst);
        if !present {
            absents += 1;
            if exemples_absents.len() < 10 {
                exemples_absents.push(format!(
                    "  {nom_dst} absent de {chemin} (appelant {}:{}, cible {}:{})",
                    src.props.name.as_deref().unwrap_or("?"),
                    src.props.start_line.unwrap_or(0),
                    dst.props.file_path.as_deref().unwrap_or("?"),
                    dst.props.start_line.unwrap_or(0),
                ));
            }
        }
    }
    if verif > 0 {
        println!(
            "\néchantillon vérifié par le source : {verif} arêtes, {absents} cibles ABSENTES du fichier appelant ({:.1} %)",
            100.0 * absents as f64 / verif as f64
        );
        for e in &exemples_absents {
            println!("{e}");
        }
    }

    println!("\narêtes : {total}");
    let mut cles: Vec<_> = par_strategie.iter().collect();
    cles.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    println!(
        "\n{:<22} {:>10} {:>10} {:>8}",
        "stratégie", "arêtes", "violations", "taux"
    );
    let mut total_fautes = 0usize;
    for (s, n) in &cles {
        let v = violations.get(*s).copied().unwrap_or(0);
        total_fautes += v;
        println!(
            "{s:<22} {n:>10} {v:>10} {:>7.2}%",
            100.0 * v as f64 / **n as f64
        );
    }
    println!(
        "\nviolations totales : {total_fautes} / {total} = {:.3}%",
        100.0 * total_fautes as f64 / total.max(1) as f64
    );
    if !exemples.is_empty() {
        println!("\nexemples :");
        for e in exemples {
            println!("{e}");
        }
    }
    Ok(())
}

/// Répertoire d'un chemin, comme approximation du module pour les
/// stratégies `receiver_*` (le module exact n'est pas exposé sur l'arête).
fn dossier(p: &str) -> &str {
    match p.rfind('/') {
        Some(i) => &p[..i],
        None => "",
    }
}
