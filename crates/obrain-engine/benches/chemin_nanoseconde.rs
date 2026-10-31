//! Banc du CHEMIN NANOSECONDE : mesure, en processus, ce que coute
//! l'enveloppe GQL par rapport aux primitives qu'elle transporte.
//!
//! Store reel exige (lecture seule) :
//!   NANO_STORE=/chemin/store.obrain cargo bench -p obrain-engine --bench chemin_nanoseconde
//!
//! Sans `NANO_STORE`, le banc s'abstient plutot que de mesurer un jouet.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use obrain_common::types::{NodeId, Value};
use obrain_engine::ObrainDB;

fn store() -> Option<ObrainDB> {
    let p = std::env::var("NANO_STORE").ok()?;
    match ObrainDB::open_read_only(&p) {
        Ok(db) => Some(db),
        Err(e) => {
            eprintln!("NANO_STORE illisible ({p}) : {e}");
            None
        }
    }
}

/// Label le plus peuple, et un nœud porteur d'une propriete texte.
fn terrain(db: &ObrainDB) -> (String, NodeId, String) {
    let sess = db.session();
    let mut labels: Vec<(String, u64)> = match db.schema() {
        obrain_engine::admin::SchemaInfo::Lpg(l) => l
            .labels
            .into_iter()
            .map(|i| (i.name, i.count as u64))
            .collect(),
        _ => Vec::new(),
    };
    labels.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    let label = labels
        .first()
        .map(|(l, _)| l.clone())
        .expect("aucun label dans le store");

    let mut trouve: Option<(NodeId, String)> = None;
    let r = sess
        .execute(&format!("MATCH (n:{label}) RETURN id(n) LIMIT 32"))
        .expect("scan d'amorce");
    for row in &r.rows {
        if let Some(Value::Int64(i)) = row.first() {
            let id = NodeId(*i as u64);
            if let Some(n) = sess.get_node(id) {
                for (k, v) in &n.properties {
                    if matches!(v, Value::String(_)) {
                        trouve = Some((id, k.to_string()));
                        break;
                    }
                }
            }
            if trouve.is_some() {
                break;
            }
        }
    }
    let (id, cle) = trouve.expect("aucun nœud avec propriete String");
    (label, id, cle)
}

// ── 1. L'ENVELOPPE PURE ────────────────────────────────────────────────
// `RETURN 1` ne touche aucune donnee. Ce qui reste : cache de plan, clones
// du plan, planification PHYSIQUE, executeur, materialisation du resultat.
fn enveloppe(c: &mut Criterion) {
    let Some(db) = store() else { return };
    let sess = db.session();

    let mut g = c.benchmark_group("enveloppe");

    sess.execute("RETURN 1").unwrap();
    g.bench_function("return_1_plan_en_cache", |b| {
        b.iter(|| black_box(sess.execute(black_box("RETURN 1")).unwrap()));
    });

    g.bench_function("return_1_plan_a_froid", |b| {
        b.iter(|| {
            sess.clear_plan_cache();
            black_box(sess.execute(black_box("RETURN 1")).unwrap())
        });
    });

    g.finish();
}

// ── 2. LES COMPTEURS ───────────────────────────────────────────────────
fn compteurs(c: &mut Criterion) {
    let Some(db) = store() else { return };
    let sess = db.session();
    let (label, _, _) = terrain(&db);

    let mut g = c.benchmark_group("compteurs");
    g.sample_size(20);

    sess.execute("MATCH (n) RETURN count(n)").unwrap();
    g.bench_function("noeuds_gql", |b| {
        b.iter(|| {
            black_box(
                sess.execute(black_box("MATCH (n) RETURN count(n)"))
                    .unwrap(),
            )
        });
    });
    g.bench_function("noeuds_direct", |b| {
        b.iter(|| black_box(db.node_count()));
    });

    sess.execute("MATCH ()-[r]->() RETURN count(r)").unwrap();
    g.bench_function("aretes_gql", |b| {
        b.iter(|| {
            black_box(
                sess.execute(black_box("MATCH ()-[r]->() RETURN count(r)"))
                    .unwrap(),
            )
        });
    });
    g.bench_function("aretes_direct", |b| {
        b.iter(|| black_box(db.edge_count()));
    });

    let q = format!("MATCH (n:{label}) RETURN count(n)");
    sess.execute(&q).unwrap();
    g.bench_function("par_label_gql", |b| {
        b.iter(|| black_box(sess.execute(black_box(&q)).unwrap()));
    });
    g.bench_function("par_label_direct", |b| {
        b.iter(|| black_box(db.node_count_by_label(black_box(&label))));
    });

    g.finish();
}

// ── 3. L'HISTOGRAMME DE L'OVERVIEW ─────────────────────────────────────
// La requete EXACTE de `/api/viz/overview` : un balayage complet du store.
const HISTO_GQL: &str =
    "MATCH (n) RETURN labels(n)[0] AS t, count(n) AS nbr ORDER BY nbr DESC LIMIT 14";

fn histogramme(c: &mut Criterion) {
    let Some(db) = store() else { return };
    let sess = db.session();

    let mut g = c.benchmark_group("histogramme");
    g.sample_size(10);

    sess.execute(HISTO_GQL).unwrap();
    g.bench_function("gql_balayage", |b| {
        b.iter(|| black_box(sess.execute(black_box(HISTO_GQL)).unwrap()));
    });

    g.bench_function("schema_direct", |b| {
        b.iter(|| black_box(db.schema()));
    });

    g.finish();
}

// ── 4. LA LECTURE PONCTUELLE ───────────────────────────────────────────
fn lecture_prop(c: &mut Criterion) {
    let Some(db) = store() else { return };
    let sess = db.session();
    let (_, id, cle) = terrain(&db);

    let q = format!("MATCH (n) WHERE id(n) = {} RETURN n.{}", id.0, cle);
    let mut g = c.benchmark_group("lecture_prop");

    sess.execute(&q).unwrap();
    g.bench_function("gql", |b| {
        b.iter(|| black_box(sess.execute(black_box(&q)).unwrap()));
    });
    g.bench_function("get_node_property", |b| {
        b.iter(|| black_box(sess.get_node_property(black_box(id), black_box(&cle))));
    });
    g.bench_function("prop_str_view", |b| {
        b.iter(|| {
            let v = sess.prop_str_view(black_box(id), black_box(&cle));
            black_box(v.as_ref().map(|s| s.as_str().len()))
        });
    });

    g.finish();
}

criterion_group!(benches, enveloppe, compteurs, histogramme, lecture_prop);
criterion_main!(benches, complements);

// ── 5. L'HISTOGRAMME EN OPERATION NOMMEE ───────────────────────────────
// `schema()` reste lent parce qu'il balaie TOUTES les aretes
// (`edge_counts_by_type`). `label_counts()` ne rend que la moitie noeuds.
fn histogramme_nomme(c: &mut Criterion) {
    let Some(db) = store() else { return };
    let mut g = c.benchmark_group("histogramme_nomme");
    g.bench_function("label_counts_direct", |b| {
        b.iter(|| black_box(db.label_counts()));
    });
    g.finish();
}

// ── 6. L'ENVELOPPE EST-ELLE PLUS CHERE EN ECRITURE-AUTORISEE ? ─────────
// Hypothese a verifier : `with_auto_commit` sur une base OUVERTE EN
// ECRITURE monte une transaction par requete, ce que le mode lecture seule
// court-circuite. Meme store, meme requete, deux modes d'ouverture.
fn enveloppe_rw_contre_ro(c: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    let chemin = dir.path().join("env.obrain");

    let mut g = c.benchmark_group("enveloppe_mode");
    {
        let db = ObrainDB::open(&chemin).unwrap();
        let s = db.session();
        for i in 0..64 {
            s.execute(&format!("INSERT (:Echantillon {{i: {i}}})"))
                .unwrap();
        }
        s.execute("RETURN 1").unwrap();
        g.bench_function("return_1_ouvert_en_ecriture", |b| {
            b.iter(|| black_box(s.execute(black_box("RETURN 1")).unwrap()));
        });
    }
    {
        let db = ObrainDB::open_read_only(&chemin).unwrap();
        let s = db.session();
        s.execute("RETURN 1").unwrap();
        g.bench_function("return_1_ouvert_en_lecture_seule", |b| {
            b.iter(|| black_box(s.execute(black_box("RETURN 1")).unwrap()));
        });
    }
    g.finish();
}

criterion_group!(complements, histogramme_nomme, enveloppe_rw_contre_ro);
