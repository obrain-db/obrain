//! Banc de l'index spatial — de BOUT EN BOUT, pas l'index seul.
//!
//! La leçon de la PR #281 : les « 894× » mesurés portaient sur l'index nu,
//! jamais sur une requête. Un chiffre qui ne traverse pas le parseur, le
//! planificateur et l'exécuteur ne dit rien de ce qu'un utilisateur ressent.
//!
//! Trois familles, et elles se lisent ensemble :
//!
//! - `spatial_api_rayon` : `ObrainDB::spatial_search` — l'index, le vrai coût
//!   de la structure seule (microsecondes attendues) ;
//! - `spatial_gql_indexe` : la même question en GQL, servie par l'index (le
//!   planificateur route le prédicat de distance) — c'est ce qu'un utilisateur
//!   paie ;
//! - `spatial_gql_balayage` : la même question en GQL SANS index — le prédicat
//!   `distance()` évalué sur chaque nœud. C'est la référence ;
//! - `spatial_construction` : le coût unique de la construction de l'arbre.
//!
//! Le jeu de points est celui de la vraie donnée : une grille quantifiée à
//! 0,001° (≈ 73 m × 111 m à nos latitudes), donc pleine d'ex æquo de distance.
//!
//! Mesuré sur dgx (2026-11-21), médianes criterion, GQL `RETURN n` :
//! 10⁵ points, 300 m (33 lignes) : 31,9 µs indexé contre 28,56 ms au
//! balayage (895×) ; 1 600 m (~1 000 lignes) : 843 µs contre 29,63 ms (35×).
//! Au-delà de quelques centaines de lignes, c'est la matérialisation de
//! `RETURN n` (~0,8 µs par ligne) qui domine, plus l'index.
//!
//! PIÈGE QUI A FAUSSÉ UNE PREMIÈRE MESURE : tant que la poignée acceptait un
//! identifiant `label:propriété` non déclaré, le planificateur bâtissait un
//! index IMPLICITE sur la base « sans index » au premier appel — le balayage
//! semblait alors aussi rapide que l'index (32 µs). La référence n'est une
//! référence que si l'on a vérifié qu'elle balaie (cf. le test
//! `une_lecture_spatiale_ne_batit_aucun_index_implicite`).

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// Tailles mesurées. 10⁶ est volontairement absent : construire un million de
/// nœuds dans un banc prend des minutes et ne change pas la conclusion (le
/// coût par point est linéaire, mesuré sur les deux paliers ci-dessous).
const TAILLES: [usize; 2] = [10_000, 100_000];

/// Rayons : ~0,03 % et ~1 % des points de la grille.
const RAYONS: [f64; 2] = [300.0, 1_600.0];

fn point(lon: f64, lat: f64) -> Value {
    Value::List(vec![Value::Float64(lon), Value::Float64(lat)].into())
}

/// `n` lieux en grille à 0,001° autour de Paris ; index optionnel.
fn base(n: usize, avec_index: bool) -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let cote = (n as f64).sqrt().ceil() as u32;
    let mut poses = 0usize;
    'externe: for i in 0..cote {
        for j in 0..cote {
            if poses >= n {
                break 'externe;
            }
            let lon = (2_000.0 + f64::from(i)) / 1_000.0;
            let lat = (48_000.0 + f64::from(j)) / 1_000.0;
            db.create_node_with_props(&["Lieu"], [("pos", point(lon, lat))]);
            poses += 1;
        }
    }
    if avec_index {
        db.execute("CREATE INDEX lieux_pos FOR (n:Lieu) ON (n.pos) USING SPATIAL")
            .expect("DDL SPATIAL");
    }
    db
}

fn requete_gql(rayon: f64) -> String {
    format!("MATCH (n:Lieu) WHERE distance(n.pos, point(2.05, 48.05)) <= {rayon} RETURN n")
}

fn banc_spatial(c: &mut Criterion) {
    for n in TAILLES {
        let avec = base(n, true);
        let sans = base(n, false);
        let mut groupe = c.benchmark_group(format!("spatial_{n}"));
        groupe.sample_size(40);
        for rayon in RAYONS {
            groupe.bench_with_input(
                BenchmarkId::new("index_api_rayon", format!("r{rayon}")),
                &rayon,
                |b, &rayon| {
                    b.iter(|| {
                        black_box(
                            avec.spatial_search("lieux_pos", 2.05, 48.05, rayon)
                                .unwrap(),
                        )
                    });
                },
            );
            // LA comparaison qui compte : la MÊME requête GQL, servie par
            // l'index d'un côté et par un balayage de l'autre.
            groupe.bench_with_input(
                BenchmarkId::new("gql_indexe", format!("r{rayon}")),
                &rayon,
                |b, &rayon| {
                    let q = requete_gql(rayon);
                    b.iter(|| black_box(avec.execute(&q).unwrap().rows.len()));
                },
            );
            groupe.bench_with_input(
                BenchmarkId::new("gql_balayage", format!("r{rayon}")),
                &rayon,
                |b, &rayon| {
                    let q = requete_gql(rayon);
                    b.iter(|| black_box(sans.execute(&q).unwrap().rows.len()));
                },
            );
        }
        groupe.finish();
    }
}

fn banc_construction(c: &mut Criterion) {
    let mut groupe = c.benchmark_group("spatial_construction");
    // Une construction par échantillon : le banc mesure un coût unique, pas
    // une boucle chaude (sinon criterion mesurerait surtout le cache).
    groupe.sample_size(10);
    for n in TAILLES {
        let db = base(n, false);
        groupe.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                black_box(
                    db.execute("CREATE INDEX lieux_pos FOR (n:Lieu) ON (n.pos) USING SPATIAL")
                        .unwrap(),
                )
            });
        });
    }
    groupe.finish();
}

criterion_group!(bancs, banc_spatial, banc_construction);
criterion_main!(bancs);
