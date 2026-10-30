//! Banc — substrat EN MÉMOIRE contre substrat FICHIER (temporaire + mmap).
//!
//! POURQUOI CE BANC EXISTE. Le 2026-09-17, le « mode mémoire » du moteur a cessé
//! d'être un substrat FICHIER dans un répertoire temporaire pour devenir un vrai
//! substrat en RAM (`SubstrateFile::open_memory`, voir
//! `docs/ADR-2026-09-17-SUBSTRAT-EN-MEMOIRE.md`). Ce changement était nécessaire
//! — `wasm32-unknown-unknown` n'a pas de système de fichiers, donc le binding
//! navigateur ne pouvait pas construire de base — mais il déplace le stockage de
//! `mmap` vers des `Vec<u8>` : la question « qu'est-ce que ça coûte, et
//! qu'est-ce que ça rapporte » n'avait AUCUNE mesure. Ce banc la pose.
//!
//! LIRE `BANCS.md` AVANT D'AJOUTER UNE MESURE ICI. En particulier : le **contrôle
//! à blanc** est obligatoire (le même bras enregistré deux fois, l'écart attendu
//! est zéro), et une mesure à charge machine élevée ne vaut rien. Vérifier
//! `/proc/loadavg` avant et après.
//!
//! CE QUI EST MESURÉ, ET POURQUOI CES BRAS-LÀ
//!
//! * `ouverture` — le coût fixe qui décide d'un `new Database()` dans un
//!   navigateur. Le bras fichier crée un répertoire, un en-tête meta et ~20
//!   fichiers de zone puis les mappe ; le bras mémoire alloue des tampons.
//! * `ecriture_noeuds`, `pose_propriete` — le chemin d'écriture. C'est là que
//!   `mmap` et `Vec<u8>` devraient le plus différer.
//! * `lecture_unitaire`, `lecture_complete` — le chemin de lecture, que le
//!   service emprunte réellement (cf. `read_path.rs`).
//! * `controle_brut` — **le contrôle à blanc** : le bras mémoire enregistré une
//!   SECONDE fois sous un autre identifiant. L'écart attendu entre `memoire` et
//!   `memoire_controle` est nul ; s'il ne l'est pas, ce banc mesure du bruit et
//!   aucun de ses verdicts ne doit être lu.
//!
//! Lancer :
//!
//! ```bash
//! export CARGO_TARGET_DIR=/tmp/bancs/target     # JAMAIS target/release
//! MEM_BENCH_N=2000 cargo bench -p obrain-substrate --bench memoire_vs_fichier
//! # contrôle à blanc seul, pour valider la machine avant de lire un écart :
//! MEM_BENCH_N=2000 cargo bench -p obrain-substrate --bench memoire_vs_fichier \
//!     -- --save-baseline temoin
//! MEM_BENCH_N=2000 cargo bench -p obrain-substrate --bench memoire_vs_fichier \
//!     -- --baseline temoin
//! ```

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;

/// Nombre de nœuds du terrain, et d'opérations par itération de mesure.
fn corpus() -> usize {
    std::env::var("MEM_BENCH_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2_000)
}

/// Les deux supports comparés. `Fichier` est le comportement natif de
/// `Config::in_memory()` ; `Memoire` est ce que wasm32 emprunte faute de
/// système de fichiers.
#[derive(Copy, Clone, Debug)]
enum Support {
    Fichier,
    Memoire,
}

impl Support {
    fn nom(self) -> &'static str {
        match self {
            Support::Fichier => "fichier_temp+mmap",
            Support::Memoire => "memoire",
        }
    }

    fn ouvrir(self) -> SubstrateStore {
        match self {
            Support::Fichier => {
                SubstrateStore::open_tempfile().expect("substrat fichier temporaire")
            }
            Support::Memoire => SubstrateStore::open_memory().expect("substrat en mémoire"),
        }
    }
}

/// Base montée UNE fois, HORS de toute boucle de mesure — voir `BANCS.md` : un
/// montage dans `b.iter` a produit des écarts de 39 % sur du code identique.
struct Terrain {
    store: SubstrateStore,
    ids: Vec<NodeId>,
}

impl Terrain {
    fn monter(support: Support, n: usize) -> Self {
        let store = support.ouvrir();
        let mut ids = Vec::with_capacity(n);
        for i in 0..n {
            let id = store.create_node(&["Banc"]);
            store.set_node_property(id, "rang", Value::Int64(i as i64));
            store.set_node_property(id, "nom", Value::String(format!("n{i}").into()));
            ids.push(id);
        }
        Self { store, ids }
    }
}

fn cle(k: &str) -> PropertyKey {
    PropertyKey::new(k)
}

/// Coût d'OUVERTURE d'un store — le bras qui décide d'un `new Database()`.
fn banc_ouverture(c: &mut Criterion) {
    let mut g = c.benchmark_group("ouverture");
    for support in [Support::Fichier, Support::Memoire] {
        g.bench_function(support.nom(), |b| b.iter(|| black_box(support.ouvrir())));
    }
    // CONTRÔLE À BLANC : le même bras, une seconde fois, sous un autre nom.
    g.bench_function("memoire_controle", |b| {
        b.iter(|| black_box(Support::Memoire.ouvrir()))
    });
    g.finish();
}

/// Écriture : création de nœuds et pose de propriétés.
fn banc_ecriture(c: &mut Criterion) {
    let n = corpus();
    let mut g = c.benchmark_group("ecriture");
    g.throughput(Throughput::Elements(n as u64));

    for support in [Support::Fichier, Support::Memoire] {
        let terrain = Terrain::monter(support, 16);
        g.bench_function(BenchmarkId::new("creation_noeuds", support.nom()), |b| {
            b.iter(|| {
                for _ in 0..n {
                    black_box(terrain.store.create_node(&["Banc"]));
                }
            })
        });
    }

    for support in [Support::Fichier, Support::Memoire] {
        let terrain = Terrain::monter(support, n);
        let ids = terrain.ids.clone();
        g.bench_function(BenchmarkId::new("pose_propriete", support.nom()), |b| {
            let mut i = 0usize;
            b.iter(|| {
                let id = ids[i % ids.len()];
                i += 1;
                terrain
                    .store
                    .set_node_property(id, "compteur", Value::Int64(i as i64));
                black_box(id)
            })
        });
    }
    g.finish();
}

/// Lecture : propriété unitaire et hydratation complète.
fn banc_lecture(c: &mut Criterion) {
    let n = corpus();
    let mut g = c.benchmark_group("lecture");
    g.throughput(Throughput::Elements(n as u64));

    for support in [Support::Fichier, Support::Memoire] {
        let terrain = Terrain::monter(support, n);
        let ids = terrain.ids.clone();
        let k = cle("rang");

        g.bench_function(BenchmarkId::new("propriete_unitaire", support.nom()), |b| {
            let mut i = 0usize;
            b.iter(|| {
                let id = ids[i % ids.len()];
                i += 1;
                black_box(terrain.store.get_node_property(id, &k))
            })
        });

        g.bench_function(BenchmarkId::new("hydratation_complete", support.nom()), |b| {
            let mut i = 0usize;
            b.iter(|| {
                let id = ids[i % ids.len()];
                i += 1;
                black_box(terrain.store.get_node(id))
            })
        });
    }

    // CONTRÔLE À BLANC, sur le bras de lecture le plus sensible.
    let terrain = Terrain::monter(Support::Memoire, n);
    let ids = terrain.ids.clone();
    let k = cle("rang");
    g.bench_function(BenchmarkId::new("propriete_unitaire_controle", "memoire"), |b| {
        let mut i = 0usize;
        b.iter(|| {
            let id = ids[i % ids.len()];
            i += 1;
            black_box(terrain.store.get_node_property(id, &k))
        })
    });
    g.finish();
}

criterion_group!(bancs, banc_ouverture, banc_ecriture, banc_lecture);
criterion_main!(bancs);
