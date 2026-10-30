//! poc_engrams.rs — POC : brancher l'EngramManager sur un store RÉEL et
//! alimenter les épisodes depuis les co-activations des sessions runtime.
//!
//! Démonstration du relief cognitif : chaque session runtime = un épisode ;
//! les nœuds qui co-activent (fichiers touchés d'une même session) forment
//! des candidats engram (Hebbian + surprise). Puis : rappel par indice
//! (`recall(cues, k)`) — le mécanisme d'injection contextuelle de CH6.
//!
//! (2026-08-31) Le manager est celui du MOTEUR (`db.cognitive_engine().
//! engram_manager()`) — la preuve du branchement `with_engrams` : les
//! procédures `obrain.engrams.*` deviennent réelles après alimentation.
//!
//! Usage :
//!   cargo run --release -p obrain-examples --bin poc-engrams \
//!     --features "storage cognitive-full engram" -- <store.obrain> [limite-épisodes]

use std::path::PathBuf;
use std::time::Instant;

use obrain_common::types::NodeId;
use obrain_engine::ObrainDB;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let store_path = PathBuf::from(&args[1]);
    let limit: usize = args.get(2).map(|s| s.parse().unwrap_or(30)).unwrap_or(30);

    let t_open = Instant::now();
    let db = ObrainDB::open(&store_path)?;
    let session = db.session();
    println!("open: {:.1} ms — store {}", t_open.elapsed().as_secs_f64() * 1000.0, store_path.display());

    // ── Le manager du MOTEUR (câblé par with_engrams au boot) ──
    let engine = db.cognitive_engine().expect("cognitive engine construit");
    let manager = engine
        .engram_manager()
        .expect("engram manager câblé (feature cognitive-engram)");
    println!("engram manager : câblé ✓ (store={} engrams)", manager.store().count());

    // ── Épisodes : les FICHIERS touchés par session (co-activation sémantique)
    //   Les ids de Message sont uniques par session (jamais co-occurrents) ;
    //   les FICHIERS reviennent d'une session à l'autre (14-18 sessions) — ce
    //   sont les vraies co-activations récurrentes. Chaque session = épisode :
    //   les fichiers qu'elle a touchés ensemble.
    let t_ep = Instant::now();
    let mut episodes = 0usize;
    let mut total_nodes = 0usize;

    // Récupère les ids de session (via le session GQL) puis leurs fichiers.
    let sid_rows = session.execute("MATCH (s:Session) RETURN s.qualified_name LIMIT 200")?;
    for row in sid_rows.rows.iter().take(limit) {
        let sid = row.first().map(|v| v.to_string()).unwrap_or_default();
        let q = format!(
            "MATCH (tc:ToolCall)-[:TOUCHES]->(f:File) WHERE tc.session = '{}' RETURN id(f) LIMIT 300",
            sid
        );
        let Ok(rows) = session.execute(&q) else { continue };
        let nodes: Vec<NodeId> = rows
            .rows
            .iter()
            .filter_map(|r| r.first())
            .filter_map(|v| {
                v.as_str()
                    .and_then(|s| s.parse::<u64>().ok())
                    .or_else(|| v.as_int64().map(|i| i as u64))
            })
            .map(NodeId)
            .collect();
        if nodes.len() >= 2 {
            // prediction_error faible : les fichiers récurrents = peu de surprise
            manager.record_activation(&nodes, 0.05);
            episodes += 1;
            total_nodes += nodes.len();
        }
    }
    println!("épisodes (sessions→fichiers) : {episodes} — {total_nodes} co-activations en {:.1} ms",
        t_ep.elapsed().as_secs_f64() * 1000.0);

    // ── Consolidation + rappel ──
    manager.tick(); // FSRS decay / homéostasie
    let n_engrams = manager.store().count();
    println!("engrams consolidés  : {n_engrams}");

    if n_engrams > 0 {
        // Rappel par indice : les 3 premiers nœuds du premier engram comme cues
        let first = manager.store().list().first().cloned().unwrap();
        let cues: Vec<NodeId> = first.ensemble.iter().take(3).map(|(id, _)| *id).collect();
        let t_recall = Instant::now();
        let results = manager.recall(&cues, 5);
        println!("rappel (cues={} nœuds) : {} résultats en {:.2} ms",
            cues.len(), results.len(), t_recall.elapsed().as_secs_f64() * 1000.0);
        for (i, r) in results.iter().take(3).enumerate() {
            println!("  rappel {i}: engram {} — confiance {:.3}, force {:.3}, valence {:.3}",
                r.engram_id.0, r.confidence, r.engram.strength, r.engram.valence);
        }

        // ── PREUVE : la procédure obrain.engrams.list() voit les engrams ──
        let t_proc = Instant::now();
        let r = session.execute("CALL obrain.engrams.list() YIELD id, strength, recall_count RETURN id, strength, recall_count LIMIT 5")?;
        println!("\nprocédure obrain.engrams.list() : {} lignes en {:.2} ms",
            r.rows.len(), t_proc.elapsed().as_secs_f64() * 1000.0);
        for row in r.rows.iter().take(3) {
            println!("  engram: {:?}", row.iter().map(|v| v.to_string()).collect::<Vec<_>>());
        }
    } else {
        println!("(0 engram consolidé — seuils de formation non atteints avec cette limite)");
    }

    println!("\nPOC engrams terminé — le relais vers CH6 : recall(cues,k) = indices d'injection.");
    Ok(())
}

