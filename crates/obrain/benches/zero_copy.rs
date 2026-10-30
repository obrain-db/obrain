//! Bench du pont mémoire zero-copy (chantier O1).
//!
//! Compare, sur un store ouvert en lecture seule partagée :
//!   (a) le chemin actuel du PoC agent-rs : `execute(GQL)` → sérialisation
//!       JSON → re-parse (ce que consomme la boucle agentique aujourd'hui) ;
//!   (b) la lecture possédée : `get_node_property` (Value, ArcStr copiés) ;
//!   (c) la vue zero-copy : `prop_str_view` (&str emprunté aux pages mmap).
//!
//! Store réel : OBRAIN_ZC_STORE=<COPIE d'un store — jamais un store vivant>,
//! clé OBRAIN_ZC_KEY (défaut "msg"), requête (a) OBRAIN_ZC_QUERY
//! (défaut `MATCH (n) RETURN n.<cle>`). Sans variable : store synthétique
//! (1000 faits ~150 o en PropsZone + 200 faits ~600 o en colonne blob).

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use obrain::{NodeId, ObrainDB, StrViewSource, Value};
use std::hint::black_box;

fn ouvre_store() -> (ObrainDB, String, Option<tempfile::TempDir>) {
    if let Ok(p) = std::env::var("OBRAIN_ZC_STORE") {
        let cle = std::env::var("OBRAIN_ZC_KEY").unwrap_or_else(|_| "msg".into());
        let db = ObrainDB::open_read_only(&p).expect("open_read_only du store copié");
        return (db, cle, None);
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    let chemin = tmp.path().join("bench.obrain");
    {
        let db = ObrainDB::open_substrate(&chemin).expect("création store synthétique");
        let session = db.session();
        let court = "un fait court mais realiste pour le seeding speculatif du moteur \
                     — environ cent cinquante octets de texte francais typique du graphe.";
        let long = "segment de fait long destine a la colonne blob du substrate, \
                    au-dela du seuil de 256 octets. "
            .repeat(7); // ~660 o
        for i in 0..1000 {
            session
                .execute(&format!("INSERT (:Fait {{fid: {i}, texte: '{court}'}})"))
                .expect("insert court");
        }
        for i in 1000..1200 {
            session
                .execute(&format!("INSERT (:Fait {{fid: {i}, texte: '{long}'}})"))
                .expect("insert long");
        }
    }
    let db = ObrainDB::open_read_only(&chemin).expect("réouverture RO");
    (db, "texte".into(), Some(tmp))
}

/// Balaie les identifiants de nœuds et garde ceux qui portent la clé.
fn collecte_ids(session: &obrain::Session, cle: &str, max: usize, borne: u64) -> Vec<NodeId> {
    let mut ids = Vec::new();
    for i in 0..borne {
        let id = NodeId(i);
        if session.prop_str_view(id, cle).is_some() {
            ids.push(id);
            if ids.len() >= max {
                break;
            }
        }
    }
    ids
}

fn bench_pont(c: &mut Criterion) {
    let (db, cle, _tmp) = ouvre_store();
    let session = db.session();
    let ids = collecte_ids(&session, &cle, 1000, 500_000);
    assert!(!ids.is_empty(), "aucun nœud porteur de la clé {cle}");

    // Répartition des sources — honnêteté du bench : on dit d'où viennent
    // les octets réellement lus.
    let (mut zone, mut blob, mut part) = (0u32, 0u32, 0u32);
    for id in &ids {
        match session.prop_str_view(*id, &cle).map(|v| v.source()) {
            Some(StrViewSource::Zone) => zone += 1,
            Some(StrViewSource::Blob) => blob += 1,
            Some(StrViewSource::Shared) => part += 1,
            None => {}
        }
    }
    eprintln!(
        "cible: {} nœuds clé `{cle}` — sources: zone={zone} blob={blob} shared={part}",
        ids.len()
    );

    // (a) chemin actuel : execute + JSON + re-parse, rapporté PAR LIGNE.
    let requete =
        std::env::var("OBRAIN_ZC_QUERY").unwrap_or_else(|_| format!("MATCH (n) RETURN n.{cle}"));
    let res0 = session.execute(&requete).expect("execute de contrôle");
    let n_lignes = res0.rows.len() as u64;
    eprintln!("requête (a): {requete} → {n_lignes} lignes");
    let mut ga = c.benchmark_group("pont_memoire");
    ga.throughput(Throughput::Elements(n_lignes.max(1)));
    ga.bench_function("a_execute_json_parse_par_ligne", |b| {
        b.iter(|| {
            let res = session.execute(black_box(&requete)).expect("execute");
            let payload = serde_json::json!({"columns": res.columns, "rows": res.rows});
            let s = serde_json::to_string(&payload).expect("to_string");
            let v: serde_json::Value = serde_json::from_str(&s).expect("from_str");
            black_box(
                v.get("rows")
                    .and_then(|r| r.as_array())
                    .map_or(0, |a| a.len()),
            )
        })
    });
    ga.finish();

    // (b) lecture possédée par nœud (Value + ArcStr copiés).
    c.bench_function("b_get_node_property_possedee", |b| {
        let mut i = 0usize;
        b.iter(|| {
            let id = ids[i % ids.len()];
            i += 1;
            let v = session.get_node_property(black_box(id), &cle);
            black_box(match v {
                Some(Value::String(s)) => s.len(),
                _ => 0,
            })
        })
    });

    // (c) vue zero-copy par nœud (&str emprunté au mmap, garde relâchée au drop).
    c.bench_function("c_prop_str_view_zero_copy", |b| {
        let mut i = 0usize;
        b.iter(|| {
            let id = ids[i % ids.len()];
            i += 1;
            let vue = session.prop_str_view(black_box(id), &cle).expect("vue");
            black_box(vue.len())
        })
    });

    // (c') la même, en consommant le texte comme le ferait un tokenizer
    // (somme des octets — force la lecture réelle des pages).
    c.bench_function("c_prop_str_view_consommee", |b| {
        let mut i = 0usize;
        b.iter(|| {
            let id = ids[i % ids.len()];
            i += 1;
            let vue = session.prop_str_view(black_box(id), &cle).expect("vue");
            black_box(vue.as_bytes().iter().map(|&b| b as u64).sum::<u64>())
        })
    });
}

criterion_group!(pont, bench_pont);
criterion_main!(pont);
