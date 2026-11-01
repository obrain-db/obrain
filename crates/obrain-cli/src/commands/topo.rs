//! `obrain topo` — passes de matérialisation simpliciales (T5, plan
//! « Topologie simpliciale native »).
//!
//! * `topo materialize <db>` — configure (si demandé) puis exécute la
//!   PASSE COMPLÈTE : triangles closed-wedge, cliques K4/K5, hyperarêtes
//!   DISPATCH. Idempotente et réparatrice (contrat cbm « re-materializing
//!   replaces, never duplicates ») — c'est aussi la commande de REBUILD
//!   offline ; le chemin du commit reste incrémental.
//! * `topo stats <db>` — volumes vivants par kind, configuration,
//!   clustering global.
//!
//! La commande travaille sur le SubstrateStore sous-jacent : une base en
//! mode non-substrat est refusée avec un diagnostic, jamais dégradée.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;
use clap::Subcommand;
use obrain_substrate::{DispatchSpec, SimplexPassesSpec, simplex_kind};

use crate::OutputFormat;

/// Sous-commandes `topo`.
#[derive(Subcommand)]
pub enum TopoCommands {
    /// Materialize the simplicial layer (triangles, K4/K5 cliques,
    /// dispatch hyperedges). Idempotent; also the offline rebuild/repair.
    Materialize {
        /// Path to the database
        path: PathBuf,

        /// Edge type enumerated for triangles/cliques
        #[arg(long, default_value = "CALLS")]
        clique_type: String,

        /// Comma-separated node labels allowed at clique-edge endpoints
        /// (empty string = no filter)
        #[arg(long, default_value = "Function,Method")]
        clique_labels: String,

        /// Edge type grouped into dispatch hyperedges
        #[arg(long, default_value = "CALLS")]
        dispatch_type: String,

        /// Edge property carrying the call-site group key
        #[arg(long, default_value = "line")]
        dispatch_prop: String,

        /// Hyperedge type name for emitted dispatch hyperedges
        #[arg(long, default_value = "DISPATCH")]
        dispatch_he_type: String,

        /// Disable the dispatch hyperedge pass entirely
        #[arg(long)]
        no_dispatch: bool,

        /// Disable incremental maintenance at commit time
        #[arg(long)]
        no_maintenance: bool,
    },

    /// Show simplicial layer statistics (live counts, config, clustering)
    Stats {
        /// Path to the database
        path: PathBuf,

        /// Also print per-node stats (triangle count, clustering, incident
        /// simplices) for these node slots — cross-validation witnesses
        #[arg(long = "node")]
        nodes: Vec<u64>,
    },

    /// (2026-08-31) CASCADE HILBERT — repermutation physique des nœuds par
    /// courbe de Hilbert (localité topologique → préfetch madvise).
    ///
    /// Active `Writer::bulk_sort_by_hilbert` : les nœuds de chaque
    /// communauté deviennent physiquement contigus sur disque, et
    /// `on_node_activated` (le hook du spread cognitif) peut alors
    /// préfetch un range Hilbert contigu d'un `madvise(WILLNEED)`. C'est
    /// la pièce de T11 qui n'avait aucun chemin de production : le code
    /// existait, jamais déclenché (substrate.hilbert = 0 octets sur les
    /// bases réelles). Idempotent (second appel = no-op via HILBERT_SORTED).
    ///
    /// `order` ≤ 15 (grille 2^order par axe, défaut 10) ; `max_degree`
    /// borne l'échelle de centralité (défaut 1024).
    Hilbert {
        /// Path to the database
        path: PathBuf,

        /// Hilbert grid order (2^order per axis, ≤ 15)
        #[arg(long, default_value_t = 10)]
        order: u32,

        /// Max degree for centrality scaling
        #[arg(long, default_value_t = 1024)]
        max_degree: u32,
    },
}

/// Point d'entrée du groupe `topo`.
pub fn run(cmd: TopoCommands, format: OutputFormat, quiet: bool) -> Result<()> {
    match cmd {
        TopoCommands::Materialize {
            path,
            clique_type,
            clique_labels,
            dispatch_type,
            dispatch_prop,
            dispatch_he_type,
            no_dispatch,
            no_maintenance,
        } => materialize(
            &path,
            SimplexPassesSpec {
                clique_edge_type: clique_type,
                clique_labels: clique_labels
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect(),
                dispatch: if no_dispatch {
                    None
                } else {
                    Some(DispatchSpec {
                        edge_type: dispatch_type,
                        group_prop_key: dispatch_prop,
                        he_type: dispatch_he_type,
                    })
                },
                maintenance: !no_maintenance,
            },
            format,
            quiet,
        ),
        TopoCommands::Stats { path, nodes } => stats(&path, &nodes, format, quiet),
        TopoCommands::Hilbert {
            path,
            order,
            max_degree,
        } => hilbert(&path, order, max_degree, format, quiet),
    }
}

/// Cascade Hilbert : repermutation physique par courbe de Hilbert.
fn hilbert(
    path: &Path,
    order: u32,
    max_degree: u32,
    format: OutputFormat,
    quiet: bool,
) -> Result<()> {
    let db = super::open_existing(path)?;
    let Some(sub) = db.substrate_handle() else {
        anyhow::bail!(
            "obrain topo hilbert : base non-substrat refusée ({})",
            path.display()
        );
    };
    let t = Instant::now();
    let before = std::fs::metadata(path.join("substrate.hilbert"))
        .map(|m| m.len())
        .unwrap_or(0);

    // (2026-08-31) CHAÎNE COMPLÈTE : si la base n'a pas encore de communautés
    // (community_id == 0 partout — le cas de toute base importée), on détecte
    // d'abord les communautés (Louvain sur les arêtes du graphe) et on les
    // écrit dans le NodeRecord. Sans cette étape, la repermutation Hilbert
    // regroupe tout dans la communauté 0 et le préfetch (`on_node_activated`)
    // couvre toute la base — inutile. Avec de vraies communautés, les nœuds
    // d'une même communauté deviennent physiquement contigus et le
    // `madvise(WILLNEED)` de la cascade préfetch le bon voisinage.
    //
    // L'ancienne version ne faisait QUE `repermuter_par_hilbert` : le code
    // T11 existait mais aucune base réelle n'avait de communautés, donc la
    // cascade était fonctionnellement morte (substrate.community = 0 octets).
    let has_communities = {
        // Un nœud témoin suffit : soit les communautés sont posées (≠0),
        // soit rien ne l'est (base importée).
        let mut assigned = 0u64;
        let hw = sub.slot_high_water();
        let mut probe = 0u32;
        while probe < hw && assigned == 0 {
            if let Ok(Some(rec)) = sub.writer().read_node(probe) {
                assigned = u64::from(rec.community_id);
            }
            probe += 1;
        }
        assigned != 0
    };

    let (num_communities, modularity) = if has_communities {
        (None, None)
    } else {
        // Louvain sur le graphe (via l'engine — les algos sont dans
        // obrain-adapters, déjà dépendance de l'engine).
        let store = db.graph_store();
        let result = obrain_adapters::plugins::algorithms::louvain(store.as_ref(), 1.0);
        // (node_id → community_id), remapé en u32 et compacté.
        let mut updates: Vec<(u32, u32)> = result
            .communities
            .iter()
            .map(|(n, c)| (n.as_u64() as u32, *c as u32))
            .collect();
        updates.sort_by_key(|(slot, _)| *slot);
        sub.writer().update_community_batch(updates)?;
        (Some(result.num_communities), Some(result.modularity))
    };

    sub.repermuter_par_hilbert(order, max_degree)?;
    let ms = t.elapsed().as_millis();
    let after = std::fs::metadata(path.join("substrate.hilbert"))
        .map(|m| m.len())
        .unwrap_or(0);
    let msg = format!(
        "cascade Hilbert OK (order={order}, max_degree={max_degree}, communautés={}, modularité={:.3}) — {} ms, colonne hilbert {before} → {after} octets",
        num_communities
            .map(|n| n.to_string())
            .unwrap_or_else(|| "existantes".into()),
        modularity.unwrap_or(0.0),
        ms
    );
    match format {
        OutputFormat::Json => {
            use serde_json::json;
            if !quiet {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "status": "ok",
                        "order": order,
                        "max_degree": max_degree,
                        "elapsed_ms": ms,
                        "hilbert_bytes_before": before,
                        "hilbert_bytes_after": after,
                        "communities_detected": num_communities,
                        "modularity": modularity,
                    }))?
                );
            }
        }
        OutputFormat::Table | OutputFormat::Csv | OutputFormat::Auto => {
            crate::output::status(&msg, quiet)
        }
    }
    Ok(())
}

fn substrate(
    db: &obrain_engine::ObrainDB,
) -> Result<std::sync::Arc<obrain_substrate::SubstrateStore>> {
    db.substrate_handle().ok_or_else(|| {
        anyhow::anyhow!(
            "cette base n'expose pas de SubstrateStore : les passes simpliciales \
             exigent le mode substrat (aucune dégradation silencieuse)"
        )
    })
}

fn materialize(
    path: &Path,
    spec: SimplexPassesSpec,
    format: OutputFormat,
    quiet: bool,
) -> Result<()> {
    let db = super::open_existing(path)?;
    let store = substrate(&db)?;
    let cfg = store.configurer_passes_simpliciales(&spec)?;
    let debut = Instant::now();
    let s = store.materialiser_simplexes()?;
    let duree = debut.elapsed();
    store.flush()?;

    if matches!(format, OutputFormat::Json) {
        println!(
            "{}",
            serde_json::json!({
                "triangles": s.triangles,
                "triangles_cycliques": s.triangles_cycliques,
                "k4": s.k4,
                "k5": s.k5,
                "hyperedges": s.hyperedges,
                "membres_hyperedges": s.membres_hyperedges,
                "groupes_hors_bornes": s.groupes_hors_bornes,
                "crees": s.crees,
                "reecrits": s.reecrits,
                "inchanges": s.inchanges,
                "tombstones": s.tombstones,
                "maintenance": cfg.maintenance_active(),
                "duree_ms": duree.as_secs_f64() * 1e3,
            })
        );
    } else if !quiet {
        println!(
            "Passe simpliciale terminée en {:.1} ms",
            duree.as_secs_f64() * 1e3
        );
        println!(
            "  triangles   : {} (dont {} cycliques)",
            s.triangles, s.triangles_cycliques
        );
        println!("  cliques     : {} K4, {} K5", s.k4, s.k5);
        println!(
            "  hyperarêtes : {} ({} membres, {} groupes hors bornes)",
            s.hyperedges, s.membres_hyperedges, s.groupes_hors_bornes
        );
        println!(
            "  réconciliation : {} créés, {} réécrits, {} inchangés, {} tombstonés",
            s.crees, s.reecrits, s.inchanges, s.tombstones
        );
        println!(
            "  maintenance incrémentale au commit : {}",
            if cfg.maintenance_active() {
                "ACTIVE"
            } else {
                "inactive"
            }
        );
    }
    Ok(())
}

fn stats(path: &Path, temoins: &[u64], format: OutputFormat, quiet: bool) -> Result<()> {
    let db = super::open_existing(path)?;
    let store = substrate(&db)?;
    let cfg = store.passes_simpliciales_config()?;
    let triangles = store.iter_simplices_kind(simplex_kind::TRIANGLE)?.len();
    let k4 = store.iter_simplices_kind(simplex_kind::K4)?.len();
    let k5 = store.iter_simplices_kind(simplex_kind::K5)?.len();
    let hyperedges = store.iter_simplices_kind(simplex_kind::HYPEREDGE)?.len();
    let clustering = store.clustering_global()?;

    if matches!(format, OutputFormat::Json) {
        println!(
            "{}",
            serde_json::json!({
                "triangles": triangles,
                "k4": k4,
                "k5": k5,
                "hyperedges": hyperedges,
                "clustering_global": clustering,
                "configure": cfg.is_some(),
                "maintenance": cfg.is_some_and(|c| c.maintenance_active()),
            })
        );
    } else if !quiet {
        match cfg {
            None => println!("Couche simpliciale : non configurée (obrain topo materialize)"),
            Some(c) => println!(
                "Couche simpliciale configurée (maintenance {})",
                if c.maintenance_active() {
                    "ACTIVE"
                } else {
                    "inactive"
                }
            ),
        }
        println!("  triangles   : {triangles}");
        println!("  cliques     : {k4} K4, {k5} K5");
        println!("  hyperarêtes : {hyperedges}");
        match clustering {
            Some(c) => println!("  clustering global : {c:.6}"),
            None => println!("  clustering global : n/a"),
        }
    }
    for &slot in temoins {
        let node = obrain_common::types::NodeId(slot);
        let t = store.triangles_du_noeud(node)?;
        let c = store.clustering_noeud(node)?;
        let simplexes = store.simplices_of_node(node)?;
        if matches!(format, OutputFormat::Json) {
            println!(
                "{}",
                serde_json::json!({
                    "node": slot,
                    "triangles": t,
                    "clustering": c,
                    "simplexes_incidents": simplexes.len(),
                })
            );
        } else if !quiet {
            println!(
                "  nœud {slot} : {t} triangles, clustering {}, {} simplexes incidents",
                c.map_or("n/a".into(), |v| format!("{v:.6}")),
                simplexes.len()
            );
        }
    }
    Ok(())
}
