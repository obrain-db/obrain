//! Chemin de LECTURE du substrat — `get_node`, `get_node_property`, et les lots.
//!
//! POURQUOI CE BANC EXISTE. Le chemin d'écriture a reçu sa ligne de base le
//! 2026-08-21 (`vec_write.rs`). Le chemin de LECTURE n'en avait aucune, alors
//! que c'est lui que le service emprunte : 15 593 rappels journalisés, médiane
//! 14,9 s chacun. Sans repère, aucune régression n'y est détectable et aucune
//! optimisation n'y est démontrable.
//!
//! LIRE `BANCS.md` AVANT D'AJOUTER UNE MESURE ICI. En particulier : le contrôle
//! à blanc est obligatoire, et il ÉCHOUE sur cette machine à charge > 40 —
//! deux mesures indépendantes ont annoncé −39,3 % et +63,7 % (p = 0,00) sur du
//! code identique. Vérifier `/proc/loadavg` avant et après.
//!
//! CE QUI EST MESURÉ, ET POURQUOI CES BRAS-LÀ
//!
//! * `get_node` — l'hydratation complète. Croît sur-linéairement avec le
//!   nombre de propriétés : `PropertyMap` est un `SmallVec<[_; 4]>` à `insert`
//!   par balayage linéaire, et `read_all_properties_for_node` y insère P fois.
//! * `get_node_property` — la lecture unitaire d'une clef. Plancher fixe
//!   d'environ 312 ns, dont la moitié en pure résolution.
//! * `get_nodes_properties_batch` — le lot. Il devrait amortir la résolution ;
//!   il a été mesuré **plus lent** que la boucle qu'il enveloppe.
//! * `get_node_property_batch` — même question sur une clef unique.
//!
//! Le bras `effet_P` fait varier le nombre de propriétés par nœud : c'est lui
//! qui expose le terme quadratique, invisible à P petit.
//!
//! Lancer :
//!
//! ```bash
//! cargo bench -p obrain-substrate --bench read_path
//! # référence puis comparaison :
//! cargo bench -p obrain-substrate --bench read_path -- --save-baseline avant
//! cargo bench -p obrain-substrate --bench read_path -- --baseline avant
//! # corpus réduit :
//! READ_PATH_BENCH_N=500 cargo bench -p obrain-substrate --bench read_path
//! ```

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;

fn nombre_de_noeuds() -> usize {
    std::env::var("READ_PATH_BENCH_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2_000)
}

/// Nombre de propriétés par nœud dans les bras qui ne le font pas varier.
const P_DEFAUT: usize = 20;

/// Base montée UNE fois, hors de toute boucle de mesure — voir `BANCS.md` :
/// un montage dans `b.iter` a produit des écarts de 39 % sur du code identique.
struct Terrain {
    store: SubstrateStore,
    ids: Vec<NodeId>,
    /// Une clef qui EXISTE sur chaque nœud.
    clef_presente: PropertyKey,
    /// Une clef internée mais ABSENTE des nœuds. Mesurée plus chère que la
    /// clef présente : il faut épuiser toute la chaîne de pages pour conclure.
    clef_absente: PropertyKey,
}

impl Terrain {
    fn nouveau(n: usize, p: usize) -> Self {
        let store = SubstrateStore::open_tempfile().expect("base temporaire");
        let ids: Vec<NodeId> = (0..n).map(|_| store.create_node(&["Noeud"])).collect();
        for (i, id) in ids.iter().enumerate() {
            for k in 0..p {
                store.set_node_property(
                    *id,
                    &format!("p{k}"),
                    Value::String(format!("v{i}_{k}").into()),
                );
            }
        }
        // Internée sur UN nœud puis jamais sur les autres : la clef existe au
        // registre, mais aucun nœud du corpus ne la porte.
        if let Some(premier) = ids.first() {
            store.set_node_property(*premier, "absente_ailleurs", Value::Int64(1));
        }
        Self {
            store,
            ids,
            clef_presente: PropertyKey::new("p0"),
            clef_absente: PropertyKey::new("absente_ailleurs"),
        }
    }
}

fn bench_get_node(c: &mut Criterion) {
    let n = nombre_de_noeuds();
    let t = Terrain::nouveau(n, P_DEFAUT);
    let mut g = c.benchmark_group("lecture/get_node");
    g.throughput(Throughput::Elements(n as u64));
    g.bench_function(BenchmarkId::from_parameter(n), |b| {
        b.iter(|| {
            let mut vus = 0usize;
            for id in &t.ids {
                if t.store.get_node(*id).is_some() {
                    vus += 1;
                }
            }
            black_box(vus)
        });
    });
    g.finish();
}

fn bench_propriete_unitaire(c: &mut Criterion) {
    let n = nombre_de_noeuds();
    let t = Terrain::nouveau(n, P_DEFAUT);
    let mut g = c.benchmark_group("lecture/propriete");
    g.throughput(Throughput::Elements(n as u64));

    g.bench_function("presente", |b| {
        b.iter(|| {
            let mut vus = 0usize;
            for id in &t.ids {
                if t.store.get_node_property(*id, &t.clef_presente).is_some() {
                    vus += 1;
                }
            }
            black_box(vus)
        });
    });

    // Une clef internée mais absente coûte PLUS cher qu'une clef présente :
    // `get_latest_for_key` n'a pas de sortie anticipée et doit épuiser la
    // chaîne de pages avant de conclure à l'absence.
    g.bench_function("absente", |b| {
        b.iter(|| {
            let mut vus = 0usize;
            for id in t.ids.iter().skip(1) {
                if t.store.get_node_property(*id, &t.clef_absente).is_some() {
                    vus += 1;
                }
            }
            black_box(vus)
        });
    });
    g.finish();
}

/// LE DUEL QUI COMPTE. Un lot doit amortir la résolution. Celui-ci a été mesuré
/// PLUS LENT que la boucle de `get_node` qui fait le même travail — il
/// construit la `PropertyMap` puis la recopie intégralement dans une seconde
/// table, clef par clef et valeur par valeur.
///
/// Les deux bras rendent le même contenu : toute divergence de temps est du
/// pur surcoût de structure.
fn bench_lot_contre_boucle(c: &mut Criterion) {
    let n = nombre_de_noeuds();
    let t = Terrain::nouveau(n, P_DEFAUT);
    let mut g = c.benchmark_group("lecture/lot_contre_boucle");
    g.throughput(Throughput::Elements(n as u64));

    g.bench_function("lot", |b| {
        b.iter(|| {
            let cartes = t.store.get_nodes_properties_batch(&t.ids);
            black_box(cartes.len())
        });
    });

    g.bench_function("boucle_get_node", |b| {
        b.iter(|| {
            let mut total = 0usize;
            for id in &t.ids {
                if let Some(nd) = t.store.get_node(*id) {
                    total += nd.properties.len();
                }
            }
            black_box(total)
        });
    });
    g.finish();
}

/// Le terme quadratique : `PropertyMap` est un `SmallVec<[_; 4]>` dont
/// l'`insert` balaie linéairement, et le doc-commentaire annonce « the typical
/// case of 1-4 properties ». À P propriétés, l'hydratation y fait P(P−1)/2
/// comparaisons de clefs. Invisible à P petit, dominant à P grand.
fn bench_effet_p(c: &mut Criterion) {
    let n = (nombre_de_noeuds() / 4).max(100);
    let mut g = c.benchmark_group("lecture/effet_P");
    // P=128 étend la courbe pour exposer la fin du terme quadratique :
    // c'est le point où `get_node` passe de ~39,7 µs (O(P²)) à ~12 µs
    // (O(P log P)) une fois l'hydratation passée au build trié en masse.
    for p in [5usize, 20, 60, 128] {
        let t = Terrain::nouveau(n, p);
        g.throughput(Throughput::Elements(n as u64));
        g.bench_with_input(BenchmarkId::from_parameter(p), &p, |b, _| {
            b.iter(|| {
                let mut vus = 0usize;
                for id in &t.ids {
                    if t.store.get_node(*id).is_some() {
                        vus += 1;
                    }
                }
                black_box(vus)
            });
        });
    }
    g.finish();
}

criterion_group!(
    benches,
    bench_get_node,
    bench_propriete_unitaire,
    bench_lot_contre_boucle,
    bench_effet_p
);
criterion_main!(benches);
