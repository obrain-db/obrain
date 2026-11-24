//! Reproductions minimales, EN MÉMOIRE, des défauts relevés sur les 325 requêtes
//! réelles de Project Orchestrator (banc dgx du 2026-11-23, clone Neo4j de
//! 4 327 350 nœuds, obrain `bd9b3b4`). Un défaut qui ne se reproduit pas ici
//! n'est qu'une hypothèse du banc : c'est ce fichier qui décide.
//!
//! Chaque cas imprime la requête, puis `OK` si obrain fait comme Neo4j, ou `ÉCART`
//! avec ce qu'obrain a rendu. Rien n'est affirmé : on imprime.
//!
//! ```bash
//! cargo run -p obrain-engine --features cypher --example repro_po_20261123
//! ```

#[cfg(feature = "cypher")]
use obrain_engine::ObrainDB;

#[cfg(feature = "cypher")]
fn joue(
    db: &ObrainDB,
    q: &str,
) -> Result<(Vec<String>, Vec<Vec<obrain_common::types::Value>>), String> {
    let r = db.session().execute_cypher(q).map_err(|e| format!("{e}"))?;
    Ok((r.columns.clone(), r.rows.clone()))
}

/// Compare sans le bruit de formatage `Debug` (guillemets, enveloppe `String(..)`).
#[cfg(feature = "cypher")]
fn norm(s: &str) -> String {
    s.replace("String(", "").replace(['"', '\\', ')'], "")
}

#[cfg(feature = "cypher")]
fn cas(titre: &str, q: &str, attendu: &str, obtenu: Result<String, String>) {
    println!("\n### {titre}\n    {q}\n    attendu (Neo4j) : {attendu}");
    match obtenu {
        Ok(s) if norm(&s) == norm(attendu) => println!("    OK"),
        Ok(s) => println!("    ÉCART : {s}"),
        Err(e) => println!("    ÉCART (erreur) : {}", e.lines().next().unwrap_or("")),
    }
}

#[cfg(feature = "cypher")]
fn main() {
    // ── 1. Clé de tri fuitée dans les colonnes (PO chat_13 et 84 autres) ─────
    {
        let db = ObrainDB::new_in_memory();
        let s = db.session();
        s.execute_cypher(
            "CREATE (s:S {id: 'a'})-[:HAS_EVENT]->(:E {seq: 1}), (s)-[:HAS_EVENT]->(:E {seq: 2})",
        )
        .unwrap();
        let q = "MATCH (s:S {id: 'a'})-[:HAS_EVENT]->(e:E) WHERE e.seq > 0 RETURN e ORDER BY e.seq ASC LIMIT 5";
        cas(
            "1. RETURN e ORDER BY e.seq : colonnes rendues",
            q,
            "colonnes = [\"e\"]",
            joue(&db, q).map(|(c, _)| format!("colonnes = {c:?}")),
        );
        let q2 = "MATCH (e:E) RETURN e.seq AS v ORDER BY e.seq DESC";
        cas(
            "1b. RETURN e.seq AS v ORDER BY e.seq : colonnes rendues",
            q2,
            "colonnes = [\"v\"]",
            joue(&db, q2).map(|(c, _)| format!("colonnes = {c:?}")),
        );
    }

    // ── 2. PatternComprehension dans un WHERE (PO persona_10) ────────────────
    {
        let db = ObrainDB::new_in_memory();
        let s = db.session();
        s.execute_cypher("CREATE (a:F {p: 'x'})-[:I]->(:F {p: 'y'}), (a)-[:I]->(:F {p: 'z'})")
            .unwrap();
        let q = "MATCH (t:F {p: 'x'}) WHERE size([(t)-[:I]-(x:F) | x]) <= 5 RETURN t.p AS p";
        cas(
            "2. size([motif | x]) dans WHERE",
            q,
            "1 ligne(s) : [\"x\"]",
            joue(&db, q).map(|(_, r)| {
                format!(
                    "{} ligne(s) : {:?}",
                    r.len(),
                    r.iter().map(|l| format!("{:?}", l[0])).collect::<Vec<_>>()
                )
            }),
        );
    }

    // ── 3. PatternComprehension indexée [..][0] après un WITH agrégeant (PO chat_23) ──
    {
        let db = ObrainDB::new_in_memory();
        let s = db.session();
        s.execute_cypher(
            "CREATE (c:CS {id: 'c1'})-[:D]->(:X), (c)-[:D]->(:X), (c)-[:SP]->(:CS {id: 'parent'})",
        )
        .unwrap();
        let q = "MATCH (c:CS)-[d:D]->() WITH c, count(d) AS n WHERE n > 0 RETURN c.id AS id, [(c)-[:SP]->(p:CS) | p.id][0] AS parent ORDER BY c.id";
        cas(
            "3. [(c)-[:SP]->(p) | p.id][0] après WITH count()",
            q,
            "1 ligne, parent = \"parent\"",
            joue(&db, q).map(|(_, r)| {
                format!(
                    "{} ligne(s), parent = {:?}",
                    r.len(),
                    r.first().map(|l| &l[1])
                )
            }),
        );
    }

    // ── 4. Sous-motif corrélé par un nœud qui n'est pas le premier (PO nbh_01/02) ──
    {
        let db = ObrainDB::new_in_memory();
        let s = db.session();
        s.execute_cypher("CREATE (:P {id: 'proj'})-[:HP]->(:F {p: 'x'})")
            .unwrap();
        let q = "MATCH (m:F {p: 'x'}) RETURN head([(op:P)-[:HP]->(m) | op.id]) AS owner";
        cas(
            "4. [(op:P)-[:HP]->(m) | op.id] : m lié, en DERNIÈRE position du motif",
            q,
            "owner = \"proj\"",
            joue(&db, q).map(|(_, r)| format!("owner = {:?}", r.first().map(|l| &l[0]))),
        );
    }

    // ── 5. WITH *, expr AS alias (PO ana_11) ─────────────────────────────────
    {
        let db = ObrainDB::new_in_memory();
        let s = db.session();
        s.execute_cypher("CREATE (:F {v: 1.0}), (:F {v: 3.0})")
            .unwrap();
        let q = "MATCH (n:F) WITH collect(n.v) AS vs WITH *, size(vs) AS cnt RETURN cnt";
        cas(
            "5. WITH *, size(vs) AS cnt",
            q,
            "cnt = Int64(2)",
            joue(&db, q).map(|(_, r)| format!("cnt = {:?}", r.first().map(|l| &l[0]))),
        );
    }

    // ── 6. Identifiants nœud/arête qui coïncident dans une liste (PO ana_09, ws_14) ──
    {
        let db = ObrainDB::new_in_memory();
        let s = db.session();
        // Cycle a -> b -> a : nodes(path) est une liste de nœuds ; les ids internes des
        // premiers nœuds et des premières arêtes coïncident (0, 1, …) dans une base neuve.
        s.execute_cypher("CREATE (a:F {p: 'a'})-[:I]->(b:F {p: 'b'})-[:I]->(a)")
            .unwrap();
        let q = "MATCH path = (f:F)-[:I*2..3]->(f) WITH nodes(path) AS ns WITH [n IN ns | n.p] AS cycle RETURN DISTINCT cycle ORDER BY size(cycle) LIMIT 10";
        cas(
            "6a. [n IN nodes(path) | n.p] (PO ana_09)",
            q,
            "≥ 1 ligne, sans erreur",
            joue(&db, q).map(|(_, r)| {
                if r.is_empty() {
                    "0 ligne".to_string()
                } else {
                    "≥ 1 ligne, sans erreur".to_string()
                }
            }),
        );
        let db2 = ObrainDB::new_in_memory();
        let s2 = db2.session();
        s2.execute_cypher(
            "CREATE (w:W {id: 'w'})-[:IT]->(:T {status: 'C'}), (w)-[:IT]->(:T {status: 'P'})",
        )
        .unwrap();
        let q2 = "MATCH (w:W {id: 'w'}) OPTIONAL MATCH (w)-[:IT]->(t1:T) WITH [x IN collect(DISTINCT t1) WHERE x IS NOT NULL] AS tasks RETURN size(tasks) AS total, size([t IN tasks WHERE t.status = 'C']) AS c";
        cas(
            "6b. size([t IN collect(DISTINCT t1) WHERE t.status = 'C']) (PO ws_14)",
            q2,
            "total = Int64(2), c = Int64(1)",
            joue(&db2, q2).map(|(_, r)| {
                format!(
                    "total = {:?}, c = {:?}",
                    r.first().map(|l| &l[0]),
                    r.first().map(|l| &l[1])
                )
            }),
        );
    }

    // ── 7. Procédures de LECTURE en session lecture seule (PO schema_01/02, note_31…) ──
    {
        let dir = std::env::temp_dir().join("repro-po-20261123-ro");
        let _ = std::fs::remove_dir_all(&dir);
        {
            let db = ObrainDB::open(&dir).expect("création");
            let s = db.session();
            // Les embeddings sont des `Value::Vector` (comme dans la base importée de PO) :
            // une liste Cypher `[0.1, …]` n'est PAS indexée par l'index vectoriel.
            let a = db.create_node(&["N"]);
            let b = db.create_node(&["N"]);
            for (id, nom, v) in [
                (a, "a", vec![0.1f32, 0.2, 0.3, 0.4]),
                (b, "b", vec![0.4f32, 0.3, 0.2, 0.1]),
            ] {
                db.set_node_property(id, "name", obrain_common::types::Value::String(nom.into()));
                db.set_node_property(id, "emb", obrain_common::types::Value::Vector(v.into()));
            }
            s.execute_cypher("MATCH (a:N {name: 'a'}), (b:N {name: 'b'}) CREATE (a)-[:R]->(b)")
                .unwrap();
            let idx = db.create_vector_index("N", "emb", Some(4), Some("cosine"), None, None);
            #[cfg(feature = "vector-index")]
            let nom = idx.and_then(|()| db.name_vector_index("N_emb", "N", "emb"));
            #[cfg(not(feature = "vector-index"))]
            let nom = idx;
            println!(
                "\n    (préparation) create_vector_index + nom N_emb : {:?}",
                nom.map(|()| "ok")
            );
            let q = "CALL db.index.vector.queryNodes('N_emb', 2, [0.1, 0.2, 0.3, 0.4]) YIELD node AS n, score RETURN n.name AS name, score";
            println!(
                "    (témoin, session ÉCRITURE, même base) queryNodes : {:?}",
                joue(&db, q).map(|(_, r)| r.len())
            );
        }
        let db = ObrainDB::open_read_only(&dir).expect("ouverture lecture seule");
        for (titre, q, attendu) in [
            (
                "7a. CALL db.labels() en lecture seule",
                "CALL db.labels() YIELD label RETURN label ORDER BY label",
                "1 ligne : N",
            ),
            (
                "7b. CALL db.relationshipTypes() en lecture seule",
                "CALL db.relationshipTypes() YIELD relationshipType RETURN relationshipType",
                "1 ligne : R",
            ),
            (
                "7c. CALL db.index.vector.queryNodes(...) en lecture seule",
                "CALL db.index.vector.queryNodes('N_emb', 2, [0.1, 0.2, 0.3, 0.4]) YIELD node AS n, score RETURN n.name AS name, score ORDER BY score DESC",
                "2 lignes, sans erreur",
            ),
        ] {
            cas(
                titre,
                q,
                attendu,
                joue(&db, q).map(|(_, r)| match r.len() {
                    0 => "0 ligne".to_string(),
                    1 => format!(
                        "1 ligne : {}",
                        r[0].first().map_or("?".into(), |v| format!("{v:?}")
                            .replace("String(", "")
                            .replace(['"', ')'], ""))
                    ),
                    n => format!("{n} lignes, sans erreur"),
                }),
            );
        }
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(not(feature = "cypher"))]
fn main() {
    eprintln!("repro_po_20261123 : requiert --features cypher");
}
