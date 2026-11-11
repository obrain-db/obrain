//! Bench d'unicité : coût par insertion selon la contrainte portée par le nœud.
//!
//! Mesure le débit d'insertion (`INSERT (:T {a, b})`) sur cinq configurations :
//! - `sans_contrainte`            : aucun contrôle d'unicité (plancher) ;
//! - `unique_simple`              : `UNIQUE (a)`, `a` distinct à chaque ligne ;
//! - `unique_composite_a_repete`  : `UNIQUE (a, b)`, `a` se répète (2 valeurs),
//!   `b` distingue le tuple — le cas qui coûtait ;
//! - `unique_composite_indexe`    : idem + `db.create_property_index("a")`
//!   (l'alternative « index » au balayage) ;
//! - `unique_composite_b_first`   : `UNIQUE (b, a)`, mêmes données — la colonne
//!   sélective (`b`) est déclarée en premier.
//!
//! 10 000 insertions par itération ; le temps rendu est celui des 10 000,
//! diviser par 10 000 pour obtenir le coût par insertion. Le nombre
//! d'insertions RÉUSSIES est imprimé une fois (il révèle le rejet du bug).
//!
//! Rejouer : `cargo bench -p obrain-engine --features cypher --bench contrainte_unique`

use std::hint::black_box;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use obrain_engine::ObrainDB;

const N: u64 = 10_000;

#[derive(Clone, Copy)]
enum Config {
    SansContrainte,
    UniqueSimple,
    UniqueCompositeARepete,
    UniqueCompositeIndexe,
    UniqueCompositeBFirst,
}

fn setup(config: Config) -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    match config {
        Config::SansContrainte => {}
        Config::UniqueSimple => {
            session
                .execute_cypher("CREATE CONSTRAINT uniq_a FOR (n:T) REQUIRE n.a IS UNIQUE")
                .expect("contrainte UNIQUE(a)");
        }
        Config::UniqueCompositeARepete | Config::UniqueCompositeIndexe => {
            session
                .execute_cypher("CREATE CONSTRAINT uniq_ab FOR (n:T) REQUIRE (n.a, n.b) IS UNIQUE")
                .expect("contrainte UNIQUE(a,b)");
            if matches!(config, Config::UniqueCompositeIndexe) {
                db.create_property_index("a");
            }
        }
        Config::UniqueCompositeBFirst => {
            session
                .execute_cypher("CREATE CONSTRAINT uniq_ba FOR (n:T) REQUIRE (n.b, n.a) IS UNIQUE")
                .expect("contrainte UNIQUE(b,a)");
        }
    }
    db
}

fn insert_query(config: Config, i: u64) -> String {
    match config {
        // `a` distinct : aucune collision possible.
        Config::SansContrainte | Config::UniqueSimple => {
            format!("INSERT (:T {{a: {}, b: 'v{}'}})", i, i)
        }
        // `a` se répète sur 2 valeurs ; `b` (unique) rend chaque tuple distinct.
        Config::UniqueCompositeARepete
        | Config::UniqueCompositeIndexe
        | Config::UniqueCompositeBFirst => {
            format!("INSERT (:T {{a: {}, b: 'v{}'}})", i % 2, i)
        }
    }
}

fn bench_config(c: &mut Criterion, name: &str, config: Config) {
    let printed = AtomicBool::new(false);
    c.bench_function(name, |b| {
        b.iter_batched(
            || setup(config),
            |db| {
                let session = db.session();
                let mut ok: usize = 0;
                for i in 0..N {
                    let q = insert_query(config, i);
                    if session.execute(&q).is_ok() {
                        ok += 1;
                    }
                }
                if !printed.swap(true, Ordering::Relaxed) {
                    eprintln!("[{}] insertions réussies: {}/{}", name, ok, N);
                }
                black_box(ok);
            },
            BatchSize::PerIteration,
        );
    });
}

fn bench_contrainte_unique(c: &mut Criterion) {
    bench_config(c, "sans_contrainte", Config::SansContrainte);
    bench_config(c, "unique_simple", Config::UniqueSimple);
    bench_config(
        c,
        "unique_composite_a_repete",
        Config::UniqueCompositeARepete,
    );
    bench_config(c, "unique_composite_indexe", Config::UniqueCompositeIndexe);
    bench_config(c, "unique_composite_b_first", Config::UniqueCompositeBFirst);
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(10)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3));
    targets = bench_contrainte_unique
}
criterion_main!(benches);
