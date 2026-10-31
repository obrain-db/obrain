//! ── BANC — LE COÛT DE L'INTERNEMENT PARTAGÉ (Phase B, pièce 3) ──────────────
//!
//! ── LA QUESTION ────────────────────────────────────────────────────────────
//! La pièce 3 a remplacé, pour chaque NOM interné, une lecture de `HashMap` LOCALE
//! (`bit_to_name.len()` / `id_to_name.len()`, quelques ns) par une décision prise
//! dans une table mmapée PARTAGÉE : `Acquire` sur l'état d'une case, comparaison de
//! hash, comparaison d'octets — et, pour un nom NEUF, un `compare_exchange` plus un
//! `fetch_add` sur le compteur d'indices.
//!
//! Ce banc dit ce que cela coûte. La réponse attendue n'est pas « zéro » : c'est
//! « négligeable devant ce que l'internement accompagne » — une entrée de WAL et
//! une écriture de `NodeRecord` dans une zone mmap, qui se comptent en centaines de
//! ns. Un banc qui ne mesurerait que l'internement dirait un chiffre juste et
//! inutile ; d'où les deux TÉMOINS ci-dessous, pris dans la même exécution :
//!
//! * `intern_local_temoin` — l'`HashMap` locale d'avant la pièce : c'est à ELLE
//!   que se compare le coût ajouté, pas à zéro (même règle que
//!   `multi_ecrivain_store::bench_borne_allocation`, qui compare la borne partagée
//!   à l'atomique local et non au vide) ;
//! * `intern_service_entier` — `create_node` sur un store réel, c'est-à-dire
//!   l'internement PLUS l'allocation de slot, l'append WAL et l'écriture de zone.
//!   C'est le dénominateur qui donne son sens au numérateur.
//!
//! ── LES TROIS RÉGIMES ──────────────────────────────────────────────────────
//! * `nom_neuf` — un nom JAMAIS vu : `compare_exchange` + `fetch_add`. C'est le
//!   régime le plus cher, et il ne se produit qu'une fois par nom et par base.
//! * `nom_connu` — un nom DÉJÀ dans la table (le régime de tout appel réel : une
//!   requête qui crée 10 000 nœuds avec la même étiquette interne UNE fois) :
//!   aucune écriture, donc aucune invalidation de ligne de cache entre processus.
//! * `nom_absent` — la question négative (`indice_de`) : « un pair l'a-t-il déjà
//!   interné ? ». C'est le chemin chaud du multi-écrivains.
//!
//! Lancer :
//!
//! ```bash
//! export CARGO_TARGET_DIR=/tmp/bancs/target
//! cargo bench -p obrain-substrate --bench intern_registres -- --save-baseline avant
//! ```

use criterion::{Criterion, Throughput, criterion_group};
use obrain_core::graph::GraphStoreMut as _;
use obrain_substrate::SubstrateStore;
use obrain_substrate::registres_partages::{Espace, RegistresPartages};
use std::hint::black_box;
use std::path::PathBuf;

/// Store pré-chauffé : zones dimensionnées, dictionnaire posé. Sans lui, la
/// première itération paierait la croissance de zone et le chiffre décrirait la
/// création de la base.
fn store_prechauffe(nom: &str) -> PathBuf {
    let td = tempfile::tempdir().expect("tempdir");
    let dir = td.path().join(nom);
    {
        let s = SubstrateStore::create(&dir).expect("créer");
        let _ = s.create_node(&["Banc"]);
        s.flush().expect("flush initial");
    }
    // `TempDir` est volontairement LEAKÉ : le répertoire doit survivre au banc (le
    // supprimer sous les pieds d'un store ouvert ferait échouer les writes).
    std::mem::forget(td);
    dir
}

/// L'internement PARTAGÉ, dans ses trois régimes.
fn bench_intern_partage(c: &mut Criterion) {
    let mut groupe = c.benchmark_group("intern_partage");
    groupe.throughput(Throughput::Elements(1));
    let dir = store_prechauffe("intern");
    let r = RegistresPartages::ouvrir(&dir).expect("registres partagés");

    // Un nom DÉJÀ interné : le régime de tout appel réel (le même nom, N fois).
    r.interner(Espace::Labels, "DejaConnu")
        .expect("intern initial");

    groupe.bench_function("nom_connu", |b| {
        b.iter(|| black_box(r.interner(Espace::Labels, "DejaConnu")).ok())
    });

    // La question NÉGATIVE : « un pair l'a-t-il déjà interné ? ». C'est le chemin
    // chaud du multi-écrivains (consulter avant de décider).
    groupe.bench_function("nom_absent_question", |b| {
        b.iter(|| black_box(r.indice_de(Espace::Labels, "JamaisVu")))
    });

    // ── UN NOM NEUF, MESURÉ PAR LOTS ────────────────────────────────────────
    // Un seul nom neuf par `iter` mesurait un ARTEFACT : l'itération tombe sous le
    // plancher de mesure de criterion (~ns), et le temps publié restait figé à
    // `[40 ms, 160 ms, 320 ms]` — les bornes par défaut du harnais, pas une mesure
    // (constaté au premier passage de ce banc). On interne donc un LOT de noms
    // neufs par itération : le temps publié est le coût AMORTI par nom, ce qui est
    // la seule façon de chiffrer le régime rare (une fois par nom et par base).
    const LOT: u64 = 200;
    let mut n: u64 = 0;
    groupe.bench_function("nom_neuf_amorti", |b| {
        b.iter(|| {
            let mut vus = 0u32;
            for _ in 0..LOT {
                n += 1;
                if r.interner(Espace::Types, &format!("Neuf{n}"))
                    .ok()
                    .flatten()
                    .is_some()
                {
                    vus += 1;
                }
            }
            black_box(vus)
        })
    });

    groupe.finish();
}

/// Le TÉMOIN : ce que coûtait l'internement LOCAL d'avant la pièce — une `HashMap`
/// et la longueur d'un vecteur.
fn bench_intern_local_temoin(c: &mut Criterion) {
    let mut groupe = c.benchmark_group("intern_local_temoin");
    groupe.throughput(Throughput::Elements(1));
    let mut carte: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    carte.insert("DejaConnu".to_string(), 0);
    groupe.bench_function("nom_connu", |b| {
        b.iter(|| black_box(carte.get("DejaConnu").copied()))
    });
    groupe.bench_function("nom_absent", |b| {
        b.iter(|| black_box(carte.get("JamaisVu").copied()))
    });
    groupe.finish();
}

/// Le SERVICE ENTIER : `create_node` sur un store réel, internement compris.
/// C'est le dénominateur qui donne son sens au coût ajouté.
fn bench_service_entier(c: &mut Criterion) {
    let mut groupe = c.benchmark_group("service_entier");
    groupe.throughput(Throughput::Elements(1));
    let dir = store_prechauffe("service");
    let s = SubstrateStore::open_shared_write(&dir).expect("store");
    groupe.bench_function("create_node_avec_label_connu", |b| {
        b.iter(|| black_box(s.create_node(&["Banc"]).0))
    });
    groupe.finish();
}

criterion_group!(
    benches,
    bench_intern_partage,
    bench_intern_local_temoin,
    bench_service_entier
);

fn main() {
    let mut c = Criterion::default().configure_from_args();
    bench_intern_partage(&mut c);
    bench_intern_local_temoin(&mut c);
    bench_service_entier(&mut c);
    c.final_summary();
}
