//! # `mesure_cascade` — rappel réel de la cascade L0→L1→L2, par l'API
//!
//! Les zones tier sont écrites par SLOT d'index (`slot_to_offset`) puis
//! réordonnées par `load_from_zones` pour que `l0[i]`, `l1[i]`, `l2[i]`
//! décrivent le nœud d'offset `i`. Comparer les fichiers par indice est donc
//! faux ; cet exemple passe par l'API publique et ne peut pas se tromper.
//!
//! ## Protocole
//!
//! Pour chaque requête : prendre l'embedding `_st_embedding` d'un nœud réel,
//! puis deux recherches.
//!
//! - **Référence** : `k0 = k1 = n` — la cascade ne filtre rien, donc le
//!   résultat est le L2 exact sur tout le corpus.
//! - **Mesure** : les largeurs réelles (`k0`, `k1` donnés en argument).
//!
//! Le rappel est l'intersection des deux top-k.
//!
//! ## Usage
//! ```text
//! cargo run --release -p obrain-substrate --example mesure_cascade -- \
//!     <base> [requetes] [k] [k0] [k1]
//! ```

use std::path::PathBuf;

use obrain_common::{PropertyKey, Value};
use obrain_core::graph::traits::GraphStore;
use obrain_substrate::SubstrateStore;
use obrain_substrate::retrieval::{LargeursCascade, NodeOffset};

type BoxErr = Box<dyn std::error::Error + Send + Sync>;

fn main() -> Result<(), BoxErr> {
    let args: Vec<String> = std::env::args().collect();
    let base = args.get(1).ok_or_else(|| -> BoxErr {
        "usage: mesure_cascade <base> [requetes] [k] [k0] [k1]".into()
    })?;
    let n_req: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(100);
    let k: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(10);
    let k0: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(1000);
    let k1: usize = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(100);

    let store = SubstrateStore::open(PathBuf::from(base))?;
    let index = store
        .load_tier_index(384)?
        .ok_or("no tier index on this base (zones absentes ou incomplètes)")?;

    // Embeddings réels : on parcourt les nœuds et on garde ceux qui en ont un.
    let cle = PropertyKey::new("_st_embedding");
    let mut requetes: Vec<Vec<f32>> = Vec::new();
    let mut visite = 0usize;
    for id in store.node_ids() {
        visite += 1;
        if requetes.len() >= n_req {
            break;
        }
        if let Some(Value::Vector(v)) = store.get_node_property(id, &cle)
            && v.len() == 384
        {
            requetes.push(v.to_vec());
        }
    }
    if requetes.is_empty() {
        return Err("aucun embedding _st_embedding de 384 dimensions trouvé".into());
    }
    eprintln!(
        "mesure_cascade: {} requêtes sur {visite} nœuds visités, k={k}, k0={k0}, k1={k1}",
        requetes.len()
    );

    let n = store.node_count();
    let exact = LargeursCascade { k0: n, k1: n };
    let mesure = LargeursCascade { k0, k1 };

    let mut rappels: Vec<f64> = Vec::with_capacity(requetes.len());
    let mut lat_exact = 0f64;
    let mut lat_mesure = 0f64;
    for q in &requetes {
        let t = std::time::Instant::now();
        let reference: Vec<NodeOffset> = index
            .chercher(q, k, exact)
            .into_iter()
            .map(|(o, _)| o)
            .collect();
        lat_exact += t.elapsed().as_secs_f64() * 1e3;

        let t = std::time::Instant::now();
        let obtenu: Vec<NodeOffset> = index
            .chercher(q, k, mesure)
            .into_iter()
            .map(|(o, _)| o)
            .collect();
        lat_mesure += t.elapsed().as_secs_f64() * 1e3;

        let inter = obtenu.iter().filter(|o| reference.contains(o)).count();
        rappels.push(inter as f64 / k as f64);
    }

    rappels.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let moyenne: f64 = rappels.iter().sum::<f64>() / rappels.len() as f64;
    let p05 = rappels[(rappels.len() as f64 * 0.05) as usize];
    let nq = requetes.len() as f64;
    println!("corpus            : {n} nœuds");
    println!("requêtes          : {nq:.0}  k={k}  (k0={k0}, k1={k1})");
    println!("rappel@{k} moyen    : {moyenne:.4}");
    println!("rappel@{k} p05      : {p05:.4}");
    println!("latence cascade   : {:.3} ms/requête", lat_mesure / nq);
    println!("latence référence : {:.3} ms/requête", lat_exact / nq);
    Ok(())
}
