//! Tri borné — `ORDER BY … LIMIT k` de bout en bout, à travers le planificateur.
//!
//! POURQUOI CE BANC EXISTE. `SortOperator` matérialisait tout et triait tout,
//! quelle que soit la limite au-dessus. Mesuré sur la forme de production :
//! `LIMIT 8` coûtait 126,2 ms et `LIMIT 20000` en coûtait 129,4 — **un rapport
//! de 0,98**. Rendre huit lignes coûtait le prix de seize mille.
//!
//! CE QUE MESURE CHAQUE BRAS
//!
//! * `k_petit` / `k_grand` — la MÊME requête, seule la limite change. Sans
//!   descente de limite, les deux coûtent pareil ; avec, `k_petit` décroche.
//!   **C'est le rapport entre ces deux bras qui est le résultat**, pas leurs
//!   valeurs absolues : il est insensible à la charge de la machine, qui les
//!   affecte toutes deux également.
//! * `sans_tri` — même limite, sans `ORDER BY`. Plancher de référence : ce que
//!   coûterait la requête si le tri était gratuit.
//! * `charge_utile` — sept colonnes projetées au lieu d'une, à limite égale.
//!   Isole ce que coûte la matérialisation hâtive, que la descente de limite ne
//!   corrige PAS : le `Project` reste sous le `Sort`, donc toutes les colonnes
//!   sont hydratées pour toutes les lignes avant le tri.
//!
//! LIRE `obrain-substrate/benches/BANCS.md` AVANT D'AJOUTER UNE MESURE. En
//! particulier : le contrôle à blanc est obligatoire, et il échoue sur cette
//! machine à charge > 40 — deux mesures indépendantes ont annoncé −39,3 % et
//! +63,7 % (p = 0,00) sur du code identique.
//!
//! ```bash
//! cargo bench -p obrain-engine --bench tri_borne
//! cargo bench -p obrain-engine --bench tri_borne -- --save-baseline avant
//! cargo bench -p obrain-engine --bench tri_borne -- --baseline avant
//! TRI_BORNE_BENCH_N=2000 cargo bench -p obrain-engine --bench tri_borne
//! ```

use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use obrain_engine::ObrainDB;

fn nombre_de_noeuds() -> usize {
    std::env::var("TRI_BORNE_BENCH_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000)
}

/// Corpus déterministe — une comparaison de lignes de base n'a aucun sens si
/// les données changent entre deux exécutions.
///
/// La clé de tri (`declared_at`) est volontairement à FORTE cardinalité et
/// désordonnée : un corpus déjà trié flatterait n'importe quel algorithme.
/// `stmt` porte une charge utile de ~400 caractères, comme les `Claim` réels.
fn monter(n: usize) -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    let bourrage = "x".repeat(400);
    for i in 0..n {
        // Mélange multiplicatif : ordre d'insertion ≠ ordre de tri.
        let quand = (i * 7919) % n;
        let requete = format!(
            "INSERT (:Claim {{id: {i}, status: 'open', declared_at: {quand}, \
             evidence_count_for: {}, evidence_count_against: {}, \
             confidence: {}, stmt: '{bourrage}'}})",
            i % 5,
            i % 3,
            i % 100
        );
        session.execute(&requete).unwrap();
    }
    db
}

const K_PETIT: usize = 8;

fn bench_tri_borne(c: &mut Criterion) {
    let n = nombre_de_noeuds();
    let db = monter(n);
    let session = db.session();

    let mut g = c.benchmark_group("tri_borne");
    g.throughput(Throughput::Elements(n as u64));

    // La forme de production : une colonne, tri sur une autre, petite limite.
    let petit = format!(
        "MATCH (c:Claim) WHERE c.status = 'open' RETURN c.id \
         ORDER BY c.declared_at DESC LIMIT {K_PETIT}"
    );
    let grand = format!(
        "MATCH (c:Claim) WHERE c.status = 'open' RETURN c.id \
         ORDER BY c.declared_at DESC LIMIT {n}"
    );

    g.bench_function("k_petit", |b| {
        b.iter(|| black_box(session.execute(&petit).unwrap()));
    });
    g.bench_function("k_grand", |b| {
        b.iter(|| black_box(session.execute(&grand).unwrap()));
    });

    // Plancher : même limite, aucun tri.
    let sans_tri = format!(
        "MATCH (c:Claim) WHERE c.status = 'open' RETURN c.id LIMIT {K_PETIT}"
    );
    g.bench_function("sans_tri", |b| {
        b.iter(|| black_box(session.execute(&sans_tri).unwrap()));
    });

    // Matérialisation hâtive : sept colonnes au lieu d'une, à limite égale.
    // La descente de limite ne corrige PAS ce poste — le `Project` reste sous
    // le `Sort`. L'écart avec `k_petit` mesure ce qu'il reste à gagner.
    let charge = format!(
        "MATCH (c:Claim) WHERE c.status = 'open' \
         RETURN c.id, c.status, c.declared_at, c.evidence_count_for, \
         c.evidence_count_against, c.confidence, c.stmt \
         ORDER BY c.declared_at DESC LIMIT {K_PETIT}"
    );
    g.bench_function("charge_utile", |b| {
        b.iter(|| black_box(session.execute(&charge).unwrap()));
    });

    g.finish();
}

criterion_group!(benches, bench_tri_borne);
criterion_main!(benches);
