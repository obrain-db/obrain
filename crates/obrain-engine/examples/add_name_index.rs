//! One-shot : pose l'index d'égalité natif substrate sur la propriété
//! `name` d'un store existant (et s'assure de `qualified_name`).
//!
//! Contexte : l'indexeur code (mcp `index_repository`) déclare désormais
//! `name` au même titre que `qualified_name`, mais les stores déjà indexés
//! (kernel complet, engine…) ne portent que `qualified_name` : la
//! résolution de centres par nom court (viz_extract palier 2) tombait sur
//! le scan `ENDS WITH` du qn (1,2–3,4 s sur 1,55 M nœuds). Cet outil
//! comble le trou SANS réindexer : la déclaration + le scan de construction
//! sont idempotents (`create_property_index` sur une clé déjà enregistrée
//! ne refait rien), puis checkpoint persiste l'index (projection eq) et le
//! catalogue.
//!
//! Fenêtre de maintenance requise pour un store vivant (même procédure que
//! `scripts/index-code-projects.sh` : UN SEUL writer à la fois — stopper
//! les webs/importers avant, redémarrer après, pour que les lecteurs RO
//! (viz serve) rouvrent le store avec le nouveau catalogue).
//!
//! Usage :
//!   cargo run --release -p obrain-engine --example add_name_index -- <store.obrain>
use web_time::Instant;

fn main() {
    let mut a = std::env::args().skip(1);
    let Some(store) = a.next() else {
        eprintln!("usage: add_name_index <store.obrain>");
        std::process::exit(2);
    };

    let t0 = Instant::now();
    let db = obrain_engine::ObrainDB::open_substrate(&store)
        .unwrap_or_else(|e| panic!("ouverture {store} : {e}"));
    let est = db
        .substrate_handle()
        .map_or(0, |s| s.node_count_live_estimate());
    println!("store {store} : ~{est} nœuds — déclaration des index d'égalité…");

    // Idempotent : `create_property_index` ne refait rien sur une clé déjà
    // enregistrée ; `qualified_name` est re-déclaré par sécurité (certaines
    // bases portent l'index sans déclaration durable).
    db.create_property_index("qualified_name");
    db.create_property_index("name");

    if let Some(sub) = db.substrate_handle() {
        let _ = sub.checkpoint();
    }
    println!(
        "fait en {:.1}s — lookups par `name` (et `qualified_name`) désormais indexés.",
        t0.elapsed().as_secs_f64()
    );
}
