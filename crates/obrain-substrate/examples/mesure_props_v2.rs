//! Mesure l'amplification de la zone de propriétés v2 (obrain#181).
//!
//! `substrate.props.v2` alloue une page de 4 Kio **par nœud** : les propriétés
//! d'un nœud vivent sur une liste chaînée de pages qui lui est propre, et aucune
//! page n'est partagée entre deux nœuds. Un nœud à six propriétés courtes occupe
//! donc 4 096 octets. Sur la copie PO (4,1 M nœuds) cela donne 30,4 Gio logiques
//! pour 5,9 Go utiles, soit un facteur 5,3.
//!
//! Cette sonde reproduit le phénomène en quelques secondes, sans la copie :
//!
//! ```bash
//! cargo run -p obrain-substrate --release --example mesure_props_v2 -- 10000
//! ```
//!
//! Elle n'affirme rien : elle imprime. Un réducteur qui échoue serait un test
//! rouge permanent, et le défaut est connu et documenté.

use obrain_common::types::Value;
use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::SubstrateStore;

fn main() {
    let n: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_000);
    let dir = std::env::temp_dir().join(format!("obrain-181-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("répertoire temporaire");

    // Six propriétés courtes, comme un nœud `:Task` de PO.
    let props = [
        ("id", "t000000"),
        ("status", "todo"),
        ("n", "1"),
        ("label", "x"),
        ("owner", "bob"),
        ("site", "n1"),
    ];
    let mut utiles = 0usize;
    {
        let s = SubstrateStore::create(&dir).expect("création de la base");
        for i in 0..n {
            let id = s.create_node(&["Task"]);
            for (k, v) in props {
                let v = if k == "id" { format!("t{i:07}") } else { v.to_string() };
                utiles += k.len() + v.len();
                s.set_node_property(id, k, Value::String(v.into()));
            }
        }
        s.checkpoint().expect("checkpoint");
    }

    let mut total = 0u64;
    println!("{:<34} {:>14}", "fichier", "octets");
    let mut lignes: Vec<(String, u64)> = Vec::new();
    for e in std::fs::read_dir(&dir).expect("lecture du répertoire") {
        let e = e.expect("entrée");
        let taille = e.metadata().map(|m| m.len()).unwrap_or(0);
        if taille > 0 {
            total += taille;
            lignes.push((e.file_name().to_string_lossy().into_owned(), taille));
        }
    }
    lignes.sort_by_key(|(_, t)| std::cmp::Reverse(*t));
    for (nom, t) in &lignes {
        println!("{:<34} {:>14}", nom, t);
    }

    let zone = lignes
        .iter()
        .filter(|(n, _)| n.starts_with("substrate.props.v2"))
        .map(|(_, t)| *t)
        .sum::<u64>();

    println!();
    println!("nœuds                          : {n}");
    println!("propriétés par nœud            : {}", props.len());
    println!("utiles (clé + valeur)          : {utiles} o");
    println!("substrate.props.v2             : {zone} o");
    println!("total des fichiers             : {total} o");
    println!(
        "amplification de la zone       : {:.2}×",
        zone as f64 / utiles as f64
    );
    println!(
        "octets de zone par nœud        : {:.0} o",
        zone as f64 / n as f64
    );

    let _ = std::fs::remove_dir_all(&dir);
}
