//! Débit d'écriture de propriétés, mono-écrivain, sur une base FICHIER (MP-3).
//!
//! POURQUOI CETTE SONDE EXISTE. MP-3 fait passer les curseurs d'allocation des
//! pages de propriétés v2 et de tas d'un champ PAR PROCESSUS à un mot PARTAGÉ
//! (`substrate.alloc`, `fetch_add` / `fetch_max`). Le prix est une opération
//! atomique sur une page mappée `MAP_SHARED` à chaque réservation de page :
//! il faut savoir s'il se voit sur le chemin d'écriture ordinaire, celui d'un
//! seul écrivain. La condition d'acceptation est un écart <= 5 % avec la
//! référence, mesuré en ALTERNANCE sur la même machine (voir `BANCS.md`).
//!
//! CE QUI EST MESURÉ : `set_node_property` sur N nœuds préexistants, K clés
//! distinctes par nœud, donc plusieurs pages de propriétés et plusieurs pages
//! de tas réellement allouées — sinon la mesure ne toucherait jamais le
//! curseur changé. Le temps inclut la sérialisation de la propriété et le
//! WAL, exactement ce qu'un import paie.
//!
//! La sonde REFUSE de mesurer si la zone v2 n'est pas active : sur le chemin
//! hérité, le curseur partagé n'est pas emprunté et la mesure ne dirait rien.
//!
//! Lancer (jamais dans `target/release` partagé) :
//!
//! ```bash
//! MP3_N=20000 MP3_K=6 MP3_TOURS=5 \
//!   cargo run -q --release -p obrain-substrate --example mp3_debit_props
//! ```
//!
//! Sortie : une ligne par tour, puis la médiane en ns par écriture.

use std::time::Instant;

use obrain_common::types::{NodeId, Value};
use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::SubstrateStore;

fn env_usize(cle: &str, defaut: usize) -> usize {
    std::env::var(cle)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(defaut)
}

fn main() {
    let n = env_usize("MP3_N", 20_000);
    let k = env_usize("MP3_K", 6);
    let tours = env_usize("MP3_TOURS", 5);

    let store = SubstrateStore::open_tempfile().expect("substrat fichier temporaire");
    if !store.props_v2_enabled() {
        eprintln!(
            "REFUS : la zone de propriétés v2 n'est pas active sur cette base ; \
             le curseur partagé n'est pas emprunté, la mesure ne dirait rien."
        );
        std::process::exit(2);
    }

    let ids: Vec<NodeId> = (0..n).map(|_| store.create_node(&["MP3"])).collect();
    // Les valeurs sont assez longues pour dépasser une page de tas : c'est le
    // passage par le curseur du TAS qu'on veut voir, pas seulement celui des
    // pages de propriétés.
    let valeur = "valeur-de-propriete-assez-longue-pour-le-tas-mp3";

    let mut mesures = Vec::with_capacity(tours);
    for tour in 0..tours {
        let debut = Instant::now();
        let mut ecrites = 0usize;
        for (i, id) in ids.iter().enumerate() {
            for c in 0..k {
                store.set_node_property(
                    *id,
                    &format!("k{c}"),
                    Value::String(format!("{valeur}-{tour}-{i}-{c}").into()),
                );
                ecrites += 1;
            }
        }
        store.flush().expect("flush");
        let ns = debut.elapsed().as_nanos() as f64 / ecrites as f64;
        mesures.push(ns);
        println!("tour {tour} : {ecrites} écritures, {ns:.0} ns/écriture");
    }

    mesures.sort_by(|a, b| a.partial_cmp(b).expect("tri de flottants"));
    let mediane = mesures[mesures.len() / 2];
    println!("MEDIANE {mediane:.0} ns/écriture (N={n}, K={k}, tours={tours}, v2=actif)");
}
