//! Écritures de propriétés vectorielles — le chemin `route_vector_write`.
//!
//! POURQUOI CE BANC EXISTE. Jusqu'au 2026-08-21, aucun banc du dépôt ne
//! mesurait une ÉCRITURE : `bench/substrate/crud.rs`, que son README présente
//! comme « criterion micro-bench for CRUD », a pour corps entier
//! `b.iter(|| black_box(0u64))` — un stub. Les dix bancs d'`obrain-substrate`
//! mesurent du géométrique et du cognitif (décay, Hopfield, Ricci, chaleur,
//! géodésiques). Le chemin d'écriture n'avait aucune ligne de base, donc
//! aucune régression n'y était détectable.
//!
//! CE QU'IL MESURE. `set_node_property` avec une `Value::Vector`, qui route
//! vers `route_vector_write` → colonne vectorielle dense mmappée. C'est le
//! chemin qu'emprunterait la persistance de `_hilbert_features` : un vecteur
//! par nœud, 626 845 nœuds sur la base `agent`.
//!
//! LES TROIS FORMES, ET POURQUOI LA TROISIÈME EST LA SEULE QUI COMPTE.
//!
//! * `serie` — un seul fil. Mesure le coût nu de l'écriture.
//! * `concurrent/{2,4,8}` — N fils écrivant la même colonne. Le `RwLock` par
//!   colonne les sérialise légitimement ; c'est la borne haute du débit.
//! * `lecture_pendant_ecriture` — **la vraie question**. Un fil écrit en
//!   continu pendant que des lecteurs interrogent la base. C'est la forme
//!   exacte d'un enrichissement Hilbert tournant pendant que le service sert
//!   des rappels. Avant le 2026-08-21, `route_vector_write` prenait le
//!   `Mutex<SubstrateFile>` GLOBAL une fois par vecteur — pour protéger une
//!   ressource utilisée seulement à la création de la colonne. Les lecteurs
//!   contendaient donc sur le verrou de la base entière, une fois par écriture.
//!   C'est cette forme-là qui doit montrer le gain, et une régression ici
//!   signifie que le verrou global est revenu dans le chemin chaud.
//!
//! LECTURE DES CHIFFRES. Chaque groupe déclare `Throughput::Elements`, donc
//! criterion publie un débit par ÉLÉMENT. C'est délibéré : plusieurs bancs du
//! dépôt (`hash_index_lookup`, `bench_adjacency_lookup`…) bouclent 100 ou
//! 1 000 fois dans leur `b.iter` sans déclarer de throughput, et publient donc
//! un temps de LOT que rien ne signale comme tel — un facteur 1 000 invisible.
//!
//! Lancer :
//!
//! ```bash
//! cargo bench -p obrain-substrate --bench vec_write
//! ```
//!
//! Détecter une régression (c'est l'usage principal) :
//!
//! ```bash
//! # une fois, sur la référence :
//! cargo bench -p obrain-substrate --bench vec_write -- --save-baseline avant
//! # après le changement :
//! cargo bench -p obrain-substrate --bench vec_write -- --baseline avant
//! ```
//!
//! Criterion imprime alors l'écart en pourcentage et un test de significativité
//! par mesure. Corpus réduit pour itérer vite :
//!
//! ```bash
//! VEC_WRITE_BENCH_N=2000 cargo bench -p obrain-substrate --bench vec_write
//! ```

use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use obrain_common::types::{NodeId, Value};
use obrain_core::graph::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;

/// Dimension des vecteurs écrits. 72 = 8 niveaux × 9 facettes, ce que produit
/// aujourd'hui `obrain-tmn` avec la facette temporelle (`enable_temporal`).
const DIM: usize = 72;

/// Nombre de nœuds. Assez pour sortir du bruit, assez peu pour que le banc
/// tienne en quelques secondes.
fn nombre_de_noeuds() -> usize {
    std::env::var("VEC_WRITE_BENCH_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5_000)
}

/// xorshift64 déterministe — pas de dépendance `rand` au moment du banc, et
/// surtout un corpus IDENTIQUE d'une exécution à l'autre. Une comparaison de
/// lignes de base n'a aucun sens si les données changent entre les deux.
struct Xs64(u64);
impl Xs64 {
    fn new(graine: u64) -> Self {
        Self(graine.max(1))
    }
    fn suivant(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn f32_unite(&mut self) -> f32 {
        (self.suivant() >> 40) as f32 / (1u64 << 24) as f32
    }
}

fn vecteur(rng: &mut Xs64) -> Arc<[f32]> {
    (0..DIM).map(|_| rng.f32_unite()).collect::<Vec<_>>().into()
}

/// Base temporaire peuplée de `n` nœuds, sans aucune propriété vectorielle.
///
/// La colonne vectorielle n'existe donc pas encore : la PREMIÈRE écriture la
/// crée, ce qui est le seul cas où `route_vector_write` doit prendre le verrou
/// global. On l'amorce explicitement dans les formes qui mesurent le régime
/// établi, pour ne pas mélanger le coût de création et celui d'écriture.
fn base_peuplee(n: usize) -> (SubstrateStore, Vec<NodeId>) {
    let store = SubstrateStore::open_tempfile().expect("base temporaire");
    let ids: Vec<NodeId> = (0..n).map(|_| store.create_node(&["Noeud"])).collect();
    (store, ids)
}

// ───────────────────────────────────────────────────────────────────────────
//  MONTAGE HORS BOUCLE — pourquoi
// ───────────────────────────────────────────────────────────────────────────
//
// La première version de ce banc utilisait `iter_batched(PerIteration)` et
// reconstruisait une base de N nœuds à CHAQUE itération. Criterion exclut bien
// le montage du chronomètre, mais pas du cache de pages ni de l'ordonnanceur :
// sur un hôte chargé, le bruit résiduel dépassait le signal.
//
// CONSTATÉ, pas supposé : en comparant le code à SA PROPRE ligne de base —
// donc un écart attendu de zéro — le harnais annonçait `serie −39,3 % (p=0,00)`,
// `concurrent/2 +43,7 % (p=0,02)` et `lecture/4 +14,5 % (p=0,00)`. Trois
// mesures sur six inventaient un changement significatif sur du code
// identique. Une régression de 39 % avait failli être attribuée à un correctif.
//
// D'où deux règles, appliquées ici et à rappeler à quiconque ajoute une mesure :
//   1. Le montage vit HORS de `b.iter`. On mesure le régime établi : la colonne
//      existe, les slots sont déjà alloués, il ne reste que l'écriture.
//   2. TOUT changement doit d'abord passer un CONTRÔLE À BLANC — comparer le
//      code à sa propre ligne de base et exiger « No change » partout. Un banc
//      qui échoue à ce contrôle ne mesure pas, il devine.

/// Base montée une fois : N nœuds, colonne amorcée, vecteurs pré-générés.
struct Terrain {
    store: SubstrateStore,
    ids: Vec<NodeId>,
    vecteurs: Vec<Arc<[f32]>>,
}

impl Terrain {
    fn nouveau(n: usize, graine: u64) -> Self {
        let (store, ids) = base_peuplee(n);
        let mut rng = Xs64::new(graine);
        let vecteurs: Vec<Arc<[f32]>> = (0..n).map(|_| vecteur(&mut rng)).collect();
        // Amorçage : la colonne est créée, et TOUS les slots sont écrits une
        // première fois. Les mesures qui suivent ne paient donc jamais
        // `ensure_room` — c'est le régime d'un enrichissement en cours, pas
        // celui de son premier vecteur.
        for (id, v) in ids.iter().zip(&vecteurs) {
            store.set_node_property(*id, "_bench_vec", Value::Vector(Arc::clone(v)));
        }
        Self {
            store,
            ids,
            vecteurs,
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
//  1. Écriture en série
// ───────────────────────────────────────────────────────────────────────────

fn bench_serie(c: &mut Criterion) {
    let n = nombre_de_noeuds();
    let t = Terrain::nouveau(n, 42);
    let mut g = c.benchmark_group("vec_write/serie");
    g.throughput(Throughput::Elements(n as u64));
    g.bench_function(BenchmarkId::from_parameter(n), |b| {
        b.iter(|| {
            for (id, v) in t.ids.iter().zip(&t.vecteurs) {
                t.store
                    .set_node_property(*id, "_bench_vec", Value::Vector(Arc::clone(v)));
            }
            black_box(t.ids.len())
        });
    });
    g.finish();
}

// ───────────────────────────────────────────────────────────────────────────
//  1 bis. Écriture en LOT — la même charge, résolue une fois
// ───────────────────────────────────────────────────────────────────────────
//
// Mesure `ecrire_vecteurs_en_lot` contre `vec_write/serie`, qui écrit
// exactement les mêmes N vecteurs par le chemin unitaire. L'écart est le coût
// de la résolution répétée : allocation `PropertyKey`, verrou du registre de
// clés, deux recherches DashMap, à chaque vecteur d'un lot où la clé et la
// colonne ne changent jamais.
//
// Le `RwLock` de la colonne reste pris PAR VECTEUR dans les deux cas : ce banc
// ne mesure donc PAS un gain obtenu en tenant un verrou plus longtemps.

fn bench_lot(c: &mut Criterion) {
    let n = nombre_de_noeuds();
    let t = Terrain::nouveau(n, 42);
    let entrees: Vec<(NodeId, Arc<[f32]>)> = t
        .ids
        .iter()
        .zip(&t.vecteurs)
        .map(|(id, v)| (*id, Arc::clone(v)))
        .collect();

    let mut g = c.benchmark_group("vec_write/lot");
    g.throughput(Throughput::Elements(n as u64));
    g.bench_function(BenchmarkId::from_parameter(n), |b| {
        b.iter(|| black_box(t.store.ecrire_vecteurs_en_lot("_bench_vec", &entrees)));
    });
    g.finish();
}

// ───────────────────────────────────────────────────────────────────────────
//  2. Écriture concurrente sur la même colonne
// ───────────────────────────────────────────────────────────────────────────

fn bench_concurrent(c: &mut Criterion) {
    let n = nombre_de_noeuds();
    let t = Arc::new(Terrain::nouveau(n, 7));
    let mut g = c.benchmark_group("vec_write/concurrent");
    g.throughput(Throughput::Elements(n as u64));
    for fils in [2usize, 4, 8] {
        g.bench_with_input(BenchmarkId::from_parameter(fils), &fils, |b, &fils| {
            b.iter(|| {
                let par_fil = n.div_ceil(fils);
                std::thread::scope(|portee| {
                    for f in 0..fils {
                        let t = Arc::clone(&t);
                        portee.spawn(move || {
                            let debut = f * par_fil;
                            let fin = ((f + 1) * par_fil).min(n);
                            for i in debut..fin {
                                t.store.set_node_property(
                                    t.ids[i],
                                    "_bench_vec",
                                    Value::Vector(Arc::clone(&t.vecteurs[i])),
                                );
                            }
                        });
                    }
                });
                black_box(n)
            });
        });
    }
    g.finish();
}

// ───────────────────────────────────────────────────────────────────────────
//  3. Lecture PENDANT écriture — la forme qui compte
// ───────────────────────────────────────────────────────────────────────────

/// Débit de LECTURE pendant qu'un fil écrit en continu — la forme d'un
/// enrichissement Hilbert tournant sous un service qui sert des rappels.
///
/// UNE RÉGRESSION ICI EST GRAVE : elle signifie qu'un verrou de portée base est
/// revenu dans le chemin d'écriture unitaire.
fn bench_lecture_pendant_ecriture(c: &mut Criterion) {
    let n = nombre_de_noeuds();
    let t = Arc::new(Terrain::nouveau(n, 99));
    let n_lectures = n;
    let mut g = c.benchmark_group("vec_write/lecture_pendant_ecriture");
    g.throughput(Throughput::Elements(n_lectures as u64));
    for lecteurs in [1usize, 4] {
        g.bench_with_input(
            BenchmarkId::from_parameter(lecteurs),
            &lecteurs,
            |b, &lecteurs| {
                b.iter(|| {
                    let stop = Arc::new(AtomicBool::new(false));
                    let ecrites = Arc::new(AtomicU64::new(0));
                    let t_ecr = Arc::clone(&t);
                    let stop_ecr = Arc::clone(&stop);
                    let cpt = Arc::clone(&ecrites);
                    let ecrivain = std::thread::spawn(move || {
                        let mut i = 0usize;
                        while !stop_ecr.load(Ordering::Relaxed) {
                            t_ecr.store.set_node_property(
                                t_ecr.ids[i % n],
                                "_bench_vec",
                                Value::Vector(Arc::clone(&t_ecr.vecteurs[i % n])),
                            );
                            cpt.fetch_add(1, Ordering::Relaxed);
                            i += 1;
                        }
                    });

                    let par_lecteur = n_lectures / lecteurs;
                    std::thread::scope(|portee| {
                        for _ in 0..lecteurs {
                            let t = Arc::clone(&t);
                            portee.spawn(move || {
                                let mut vus = 0usize;
                                for i in 0..par_lecteur {
                                    if t.store.get_node(t.ids[i % n]).is_some() {
                                        vus += 1;
                                    }
                                }
                                black_box(vus)
                            });
                        }
                    });

                    stop.store(true, Ordering::Relaxed);
                    ecrivain.join().expect("fil d'ecriture");
                    black_box(ecrites.load(Ordering::Relaxed))
                });
            },
        );
    }
    g.finish();
}

criterion_group!(
    benches,
    bench_serie,
    bench_lot,
    bench_concurrent,
    bench_lecture_pendant_ecriture
);
criterion_main!(benches);
