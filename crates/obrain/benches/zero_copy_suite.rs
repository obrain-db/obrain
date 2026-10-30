//! Bench de la SUITE du pont mémoire zero-copy (chantier VIEWS-suite).
//!
//! Store synthétique bâti par GQL puis rouvert en LECTURE SEULE :
//!   - 9 500 nœuds `:Fait {texte ~150 o}` (PropsZone) + 500 nœuds `:Fait`
//!     longs (~600 o, colonne blob) = 10 000 nœuds balayés ;
//!   - 1 000 arêtes `:CITE {contexte ~150 o}` + 200 longues (~340 o, blob).
//!
//! Groupes (criterion, sample_size = 100) :
//!   `balayage_10k` — le MÊME travail (lire le texte de chaque :Fait) par
//!     trois chemins : (a) `execute` GQL matérialisé (QueryResult possédé),
//!     (b) boucle `prop_str_view` par nœud (une prise de gardes PAR nœud),
//!     (c) `for_each_prop_view` (UNE prise de garde par zone).
//!   `resultbuf_10k` — le chemin qui DOIT copier : (a) allocations répétées
//!     (Vec<(u64, String)> neuf à chaque requête), (b) `ResultBuf` réutilisé
//!     (`clear()` sans dealloc, arène amortie).
//!   `arete` — (a) chemin possédé (`get_edge_property`, arête matérialisée),
//!     (b) vue `edge_prop_str_view` (&str emprunté).
//!
//! Jamais de store vivant : la base est créée par le bench dans un tempdir.

use std::hint::black_box;
use std::ops::ControlFlow;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use obrain::{EdgeId, NodeId, ObrainDB, ResultBuf, StrViewSource, Value};

const NOEUDS_ZONE: usize = 9_500;
const NOEUDS_BLOB: usize = 500;
const ARETES_ZONE: usize = 1_000;
const ARETES_BLOB: usize = 200;

fn construit_store() -> (ObrainDB, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let chemin = tmp.path().join("suite.obrain");
    {
        let db = ObrainDB::open_substrate(&chemin).expect("création store synthétique");
        let session = db.session();
        let court = "un fait court mais realiste pour le seeding speculatif du moteur \
                     — environ cent cinquante octets de texte francais typique du graphe.";
        let long = "segment de fait long destine a la colonne blob du substrate, \
                    au-dela du seuil de 256 octets. "
            .repeat(6); // ~570 o
        for i in 0..NOEUDS_ZONE {
            session
                .execute(&format!("INSERT (:Fait {{fid: {i}, texte: '{court}'}})"))
                .expect("insert court");
        }
        for i in NOEUDS_ZONE..NOEUDS_ZONE + NOEUDS_BLOB {
            session
                .execute(&format!("INSERT (:Fait {{fid: {i}, texte: '{long}'}})"))
                .expect("insert long");
        }
        let ctx_court = "contexte d arete court, cite pendant la generation du tour \
                         — environ cent cinquante octets de texte francais typique.";
        let ctx_long = "contexte d arete long destine a la colonne blob des aretes. ".repeat(6);
        for i in 0..ARETES_ZONE {
            session
                .execute(&format!(
                    "INSERT (:Src {{s: {i}}})-[:CITE {{contexte: '{ctx_court}'}}]->(:Dst {{d: {i}}})"
                ))
                .expect("insert arête courte");
        }
        for i in ARETES_ZONE..ARETES_ZONE + ARETES_BLOB {
            session
                .execute(&format!(
                    "INSERT (:Src {{s: {i}}})-[:CITE {{contexte: '{ctx_long}'}}]->(:Dst {{d: {i}}})"
                ))
                .expect("insert arête longue");
        }
    }
    let db = ObrainDB::open_read_only(&chemin).expect("réouverture RO");
    (db, tmp)
}

fn bench_suite(c: &mut Criterion) {
    let (db, _tmp) = construit_store();
    let session = db.session();

    // Cibles pré-résolues + honnêteté du bench : répartition des sources.
    let mut ids: Vec<NodeId> = Vec::new();
    let (mut zone, mut blob, mut part) = (0u64, 0u64, 0u64);
    for i in 0..200_000u64 {
        if let Some(v) = session.prop_str_view(NodeId(i), "texte") {
            match v.source() {
                StrViewSource::Zone => zone += 1,
                StrViewSource::Blob => blob += 1,
                StrViewSource::Shared => part += 1,
            }
            ids.push(NodeId(i));
        }
    }
    eprintln!(
        "nœuds cibles: {} — sources: zone={zone} blob={blob} shared={part}",
        ids.len()
    );
    assert_eq!(ids.len(), NOEUDS_ZONE + NOEUDS_BLOB, "10 000 nœuds :Fait attendus");

    let servies_scan = session
        .for_each_prop_view(Some("Fait"), "texte", |_, _| ControlFlow::Continue(()))
        .expect("balayage de contrôle");
    assert_eq!(servies_scan, ids.len() as u64);

    let requete = "MATCH (n:Fait) RETURN n.texte";
    let res0 = session.execute(requete).expect("execute de contrôle");
    assert_eq!(res0.rows.len(), ids.len(), "GQL et balayage voient les mêmes nœuds");

    // ── Groupe 1 : balayage 10k, rapporté PAR NŒUD ──────────────────────
    let mut g = c.benchmark_group("balayage_10k");
    g.sample_size(100);
    g.throughput(Throughput::Elements(ids.len() as u64));
    g.bench_function("a_execute_gql_materialise", |b| {
        b.iter(|| {
            let res = session.execute(black_box(requete)).expect("execute");
            black_box(res.rows.len())
        })
    });
    g.bench_function("b_boucle_prop_str_view_par_noeud", |b| {
        b.iter(|| {
            let mut octets = 0u64;
            for id in &ids {
                let vue = session.prop_str_view(black_box(*id), "texte").expect("vue");
                octets += vue.len() as u64;
            }
            black_box(octets)
        })
    });
    g.bench_function("c_for_each_prop_view_une_garde_par_zone", |b| {
        b.iter(|| {
            let mut octets = 0u64;
            let n = session
                .for_each_prop_view(Some("Fait"), "texte", |_, s| {
                    octets += s.len() as u64;
                    ControlFlow::Continue(())
                })
                .expect("balayage");
            black_box((n, octets))
        })
    });
    g.finish();

    // ── Groupe 2 : le chemin qui DOIT copier, alloc répétées vs arène ───
    let mut g = c.benchmark_group("resultbuf_10k");
    g.sample_size(100);
    g.throughput(Throughput::Elements(ids.len() as u64));
    g.bench_function("a_allocations_repetees", |b| {
        b.iter(|| {
            // Une « requête » naïve : Vec + Strings NEUFS à chaque fois.
            let mut sortie: Vec<(u64, String)> = Vec::new();
            session
                .for_each_prop_view(Some("Fait"), "texte", |id, s| {
                    sortie.push((id.0, s.to_owned()));
                    ControlFlow::Continue(())
                })
                .expect("balayage");
            black_box(sortie.len())
        })
    });
    g.bench_function("b_resultbuf_reutilise", |b| {
        let mut buf = ResultBuf::new();
        b.iter(|| {
            let n = session
                .collect_props_into(Some("Fait"), "texte", &mut buf)
                .expect("collecte");
            black_box((n, buf.octets()))
        })
    });
    g.finish();

    // ── Groupe 3 : arêtes — possédé vs vue ──────────────────────────────
    let mut edge_ids: Vec<EdgeId> = Vec::new();
    let (mut ez, mut eb, mut es) = (0u64, 0u64, 0u64);
    for i in 0..100_000u64 {
        if let Some(v) = session.edge_prop_str_view(EdgeId(i), "contexte") {
            match v.source() {
                StrViewSource::Zone => ez += 1,
                StrViewSource::Blob => eb += 1,
                StrViewSource::Shared => es += 1,
            }
            edge_ids.push(EdgeId(i));
        }
    }
    eprintln!(
        "arêtes cibles: {} — sources: zone={ez} blob={eb} shared={es}",
        edge_ids.len()
    );
    assert_eq!(edge_ids.len(), ARETES_ZONE + ARETES_BLOB, "1 200 arêtes :CITE attendues");

    let mut g = c.benchmark_group("arete");
    g.sample_size(100);
    g.bench_function("a_get_edge_property_possedee", |b| {
        let mut i = 0usize;
        b.iter(|| {
            let id = edge_ids[i % edge_ids.len()];
            i += 1;
            let v = session.get_edge_property(black_box(id), "contexte");
            black_box(match v {
                Some(Value::String(s)) => s.len(),
                _ => 0,
            })
        })
    });
    g.bench_function("b_edge_prop_str_view_zero_copy", |b| {
        let mut i = 0usize;
        b.iter(|| {
            let id = edge_ids[i % edge_ids.len()];
            i += 1;
            let vue = session
                .edge_prop_str_view(black_box(id), "contexte")
                .expect("vue");
            black_box(vue.len())
        })
    });
    g.bench_function("b_edge_prop_str_view_consommee", |b| {
        let mut i = 0usize;
        b.iter(|| {
            let id = edge_ids[i % edge_ids.len()];
            i += 1;
            let vue = session
                .edge_prop_str_view(black_box(id), "contexte")
                .expect("vue");
            black_box(vue.as_bytes().iter().map(|&b| b as u64).sum::<u64>())
        })
    });
    g.finish();
}

criterion_group!(suite, bench_suite);
criterion_main!(suite);
