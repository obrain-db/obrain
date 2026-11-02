//! Data export/import commands.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};

use anyhow::{Context, Result};

use obrain_common::types::Value;

use crate::output;
use crate::{DataCommands, OutputFormat};

/// Décode une valeur de propriété NDJSON en [`Value`] — sans perte silencieuse.
///
/// Le vocabulaire natif est la sérialisation serde de `Value`, celle que
/// `data dump` émet : `"Null"`, `{"Bool": b}`, `{"Int64": i}`,
/// `{"Float64": f}`, `{"String": s}`, etc. On accepte EN PLUS les alias
/// produits par des outils tiers (JS notamment, qui n'a qu'un type `number`) :
///
/// - `{"Number": n}` → `Int64(n)` si `n` est un entier JSON (représentable
///   en i64, sans point décimal), `Float64(n)` sinon — donc `{"Number": 42}`
///   ressort `{"Int64": 42}` au dump et `{"Number": 1.5}` ressort
///   `{"Float64": 1.5}` : c'est la normalisation choisie ;
/// - `{"Float": f}` → `Float64(f)` (l'enum s'appelle `Float64`, pas `Float`).
///
/// Toute variante inconnue est une erreur FRANCHE : l'ancien comportement
/// (`unwrap_or(Value::Null)`) transformait `{"Number": 42}` en `Null`
/// silencieux — la valeur était PERDUE sans le moindre diagnostic.
fn decode_prop_value(key: &str, val: &serde_json::Value, line_num: usize) -> Result<Value> {
    // 1) Vocabulaire natif : exactement ce que `data dump` sait émettre.
    if let Ok(v) = serde_json::from_value::<Value>(val.clone()) {
        return Ok(v);
    }

    // 2) Alias tiers sur objet à variante unique.
    if let Some(obj) = val.as_object() {
        if obj.len() == 1 {
            let (variant, inner) = obj.iter().next().expect("len == 1");
            match variant.as_str() {
                "Number" => {
                    if let Some(i) = inner.as_i64() {
                        return Ok(Value::Int64(i));
                    }
                    if let Some(f) = inner.as_f64() {
                        return Ok(Value::Float64(f));
                    }
                }
                "Float" => {
                    if let Some(f) = inner.as_f64() {
                        return Ok(Value::Float64(f));
                    }
                }
                _ => {}
            }
            anyhow::bail!(
                "Unknown or invalid property value variant {variant:?} for key {key:?} \
                 on line {line_num} (value: {inner}). Expected a Value variant such as \
                 Null, Bool, Int64, Float64, String, or the aliases Number/Float. \
                 Refusing to silently drop the value."
            );
        }
    }

    anyhow::bail!(
        "Invalid property value for key {key:?} on line {line_num}: {val}. \
         Expected an object with a single Value variant (e.g. {{\"Int64\": 42}}) \
         or the string \"Null\"."
    )
}

/// Run data commands.
pub fn run(cmd: DataCommands, _format: OutputFormat, quiet: bool) -> Result<()> {
    match cmd {
        DataCommands::Dump {
            path,
            output: out,
            export_format,
        } => {
            let format_name = export_format.as_deref().unwrap_or("json");
            if format_name != "json" && format_name != "jsonl" {
                anyhow::bail!(
                    "Only JSON Lines format is currently supported.\n\
                     Use `--export-format json` or omit for the default."
                );
            }

            let db = super::open_existing(&path)?;
            let store = db.graph_store();
            let file = std::fs::File::create(&out)
                .with_context(|| format!("Failed to create output file: {}", out.display()))?;
            let mut writer = std::io::BufWriter::new(file);

            // T17 final cutover (2026-04-23): `ObrainDB::iter_nodes` / `iter_edges`
            // were removed with the legacy persistence layer. Iterate through
            // the substrate-routed `GraphStore` trait handle instead: list IDs
            // via `node_ids()` / `edge_ids()`, then fetch each entity by ID.
            let mut node_count = 0usize;
            for id in store.node_ids() {
                let Some(node) = store.get_node(id) else {
                    continue;
                };
                let labels: Vec<String> = node.labels.iter().map(|s| s.to_string()).collect();
                let properties: HashMap<String, Value> = node
                    .properties
                    .to_btree_map()
                    .into_iter()
                    .map(|(k, v)| (k.as_str().to_string(), v))
                    .collect();
                let record = serde_json::json!({
                    "type": "node",
                    "id": node.id.0,
                    "labels": labels,
                    "properties": properties,
                });
                serde_json::to_writer(&mut writer, &record)?;
                writeln!(writer)?;
                node_count += 1;
            }

            // Enumerate all edges by walking outgoing edges from each node
            // (avoids duplicates — edges_from returns only one direction).
            let mut edge_count = 0usize;
            let mut edge_ids: Vec<obrain_common::types::EdgeId> = Vec::new();
            for nid in store.node_ids() {
                for (_dst, eid) in store.edges_from(nid, obrain_core::graph::Direction::Outgoing) {
                    edge_ids.push(eid);
                }
            }
            for id in edge_ids {
                let Some(edge) = store.get_edge(id) else {
                    continue;
                };
                let properties: HashMap<String, Value> = edge
                    .properties
                    .to_btree_map()
                    .into_iter()
                    .map(|(k, v)| (k.as_str().to_string(), v))
                    .collect();
                let record = serde_json::json!({
                    "type": "edge",
                    "id": edge.id.0,
                    "source": edge.src.0,
                    "target": edge.dst.0,
                    "edge_type": edge.edge_type.as_str(),
                    "properties": properties,
                });
                serde_json::to_writer(&mut writer, &record)?;
                writeln!(writer)?;
                edge_count += 1;
            }

            writer.flush()?;
            output::status(
                &format!(
                    "Exported {} nodes and {} edges to {}",
                    node_count,
                    edge_count,
                    out.display()
                ),
                quiet,
            );
        }
        DataCommands::Load {
            input,
            path,
            no_index,
        } => {
            let file = std::fs::File::open(&input)
                .with_context(|| format!("Failed to open input file: {}", input.display()))?;
            let reader = BufReader::new(file);

            // (2026-09-09) BULK LOAD EN GROUP-COMMIT — `ObrainDB::open` ouvre
            // le WAL en `SyncMode::EveryCommit` : un fsync PAR record, donc un
            // fsync par nœud, par arête ET par propriété pendant l'import.
            // `open_substrate_with_mode` (T5 batch-WAL, plan b30650e2) est
            // l'entrée prévue exactement pour ce cas : `SyncMode::Group`
            // n'fsync jamais dans le writer, l'appelant paie UN `flush_wal()`
            // en fin de lot (voir plus bas, avant le checkpoint). La fenêtre
            // de durabilité couvre tout le load : acceptable, l'import est
            // rejouable depuis le fichier d'entrée s'il est interrompu.
            // L'ouverture crée le store s'il n'existe pas encore — mêmes
            // chemins que `open`/`open_existing`, seul le mode fsync change,
            // et il ne vaut que pour CETTE ouverture (les ouvertures
            // suivantes repartent en EveryCommit par défaut).
            let db = obrain_engine::ObrainDB::open_substrate_with_mode(
                &path,
                obrain_substrate::wal_io::SyncMode::Group,
            )
            .with_context(|| format!("Failed to open database at {}", path.display()))?;

            let mut node_count = 0usize;
            let mut edge_count = 0usize;

            // Correctif fantômes (T2, preuve PO 563e670f) : les ids du dump
            // peuvent être troués (nœuds supprimés avant l'export) alors que
            // `create_node` alloue des ids internes denses. Câbler les arêtes
            // avec les ids du dump matérialisait chaque trou référencé en
            // nœud fantôme (sans label ni propriété) via les colonnes de
            // degrés/adjacence — 7 133 fantômes sur un dump de 37 678 nœuds.
            // On maintient donc une table id-dump → id-interne pendant le
            // load ; les arêtes se câblent exclusivement via cette table.
            // Coût mémoire : deux u64 par nœud (~16 octets + surcoût
            // HashMap), soit ~1 Mo pour un dump de 37k nœuds.
            let mut correspondance: HashMap<u64, obrain_common::types::NodeId> = HashMap::new();

            // (NS-PARTOUT, defaut n°3) Un store restaure doit etre un store
            // COMPLET : `index_repository` indexe qualified_name + name a
            // l'import, `data load` rendait un store NU (constate sur
            // kv-linux-driversnet-v2 : seek 260 ms au lieu de 0,13 ms). On
            // note ici quelles proprietes par defaut apparaissent dans le
            // dump, pour ne creer a la fin QUE des index qui portent des
            // valeurs — jamais d'index vide, il ressemblerait a une reponse.
            const PROPRIETES_PAR_DEFAUT: [&str; 2] = ["qualified_name", "name"];
            let mut proprietes_vues: [bool; 2] = [false; 2];

            for (line_num, line) in reader.lines().enumerate() {
                let line = line.with_context(|| format!("Failed to read line {}", line_num + 1))?;
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                let record: serde_json::Value = serde_json::from_str(trimmed)
                    .with_context(|| format!("Invalid JSON on line {}", line_num + 1))?;

                match record.get("type").and_then(|t| t.as_str()) {
                    Some("node") => {
                        let labels: Vec<&str> = record
                            .get("labels")
                            .and_then(|l| l.as_array())
                            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                            .unwrap_or_default();

                        // Un seul appel nœud+propriétés (au lieu de
                        // `create_node` + un `set_node_property` PAR
                        // propriété) : une seule traversée du store par
                        // entité, et sous `SyncMode::Group` plus aucun fsync
                        // au fil de l'eau.
                        let props: Vec<(&str, Value)> = record
                            .get("properties")
                            .and_then(|p| p.as_object())
                            .map(|props| {
                                props
                                    .iter()
                                    .map(|(key, val)| {
                                        decode_prop_value(key, val, line_num + 1)
                                            .map(|value| (key.as_str(), value))
                                    })
                                    .collect::<Result<Vec<_>>>()
                            })
                            .transpose()?
                            .unwrap_or_default();

                        for (i, p) in PROPRIETES_PAR_DEFAUT.iter().enumerate() {
                            if !proprietes_vues[i] && props.iter().any(|(k, _)| k == p) {
                                proprietes_vues[i] = true;
                            }
                        }
                        let id = db.create_node_with_props(&labels, props);
                        if let Some(dump_id) = record.get("id").and_then(|v| v.as_u64()) {
                            correspondance.insert(dump_id, id);
                        }
                        node_count += 1;
                    }
                    Some("edge") => {
                        let source_dump = record
                            .get("source")
                            .and_then(|v| v.as_u64())
                            .ok_or_else(|| {
                                anyhow::anyhow!("Missing 'source' on line {}", line_num + 1)
                            })?;
                        let target_dump = record
                            .get("target")
                            .and_then(|v| v.as_u64())
                            .ok_or_else(|| {
                                anyhow::anyhow!("Missing 'target' on line {}", line_num + 1)
                            })?;
                        // Câblage via la table de correspondance : une arête
                        // ne peut référencer qu'un nœud déclaré PLUS HAUT
                        // dans le dump (data dump écrit tous les nœuds avant
                        // les arêtes). Référence inconnue = erreur franche,
                        // jamais de nœud fantôme.
                        let source = *correspondance.get(&source_dump).ok_or_else(|| {
                            anyhow::anyhow!(
                                "Edge on line {} references node id {} which is not declared \
                                 in this dump (nodes must precede their edges)",
                                line_num + 1,
                                source_dump
                            )
                        })?;
                        let target = *correspondance.get(&target_dump).ok_or_else(|| {
                            anyhow::anyhow!(
                                "Edge on line {} references node id {} which is not declared \
                                 in this dump (nodes must precede their edges)",
                                line_num + 1,
                                target_dump
                            )
                        })?;
                        let edge_type = record
                            .get("edge_type")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| {
                                anyhow::anyhow!("Missing 'edge_type' on line {}", line_num + 1)
                            })?;

                        // Même geste que pour les nœuds : arête+propriétés en
                        // un seul appel.
                        let props: Vec<(&str, Value)> = record
                            .get("properties")
                            .and_then(|p| p.as_object())
                            .map(|props| {
                                props
                                    .iter()
                                    .map(|(key, val)| {
                                        decode_prop_value(key, val, line_num + 1)
                                            .map(|value| (key.as_str(), value))
                                    })
                                    .collect::<Result<Vec<_>>>()
                            })
                            .transpose()?
                            .unwrap_or_default();

                        db.create_edge_with_props(source, target, edge_type, props);
                        edge_count += 1;
                    }
                    other => {
                        anyhow::bail!("Unknown record type {:?} on line {}", other, line_num + 1);
                    }
                }
            }

            output::status(
                &format!(
                    "Loaded {} nodes and {} edges into {}",
                    node_count,
                    edge_count,
                    path.display()
                ),
                quiet,
            );

            // (2026-09-09) FSYNC DE FIN DE LOT — contrepartie obligatoire de
            // `SyncMode::Group` : le writer WAL n'a rien fsyncé pendant le
            // load, on paie ici l'UNIQUE fsync qui rend tout l'import durable
            // avant le checkpoint (si le checkpoint échoue ensuite, le WAL
            // fsyncé suffit au replay). Erreur franche : un load annoncé
            // réussi doit être sur disque.
            db.flush_wal()
                .context("Failed to fsync WAL at end of load (Group sync mode)")?;

            // (NS-PARTOUT, defaut n°3) INDEX PAR DEFAUT — les memes que
            // `index_repository` (qualified_name, name), crees AVANT le
            // checkpoint pour que sa fusion re-ancre des zones deja pleines.
            // `--no-index` les debranche ; une propriete absente du dump
            // n'est pas indexee (jamais d'index vide).
            if !no_index {
                for (i, p) in PROPRIETES_PAR_DEFAUT.iter().enumerate() {
                    if proprietes_vues[i] {
                        let t = std::time::Instant::now();
                        db.create_property_index(p);
                        output::status(
                            &format!(
                                "Index de propriete `{}` construit et persiste ({:.1}s)",
                                p,
                                t.elapsed().as_secs_f64()
                            ),
                            quiet,
                        );
                    }
                }
            }

            // (2026-08-31) CHECKPOINT À LA FIN DU LOAD — la base née compacte.
            //
            // Sans cela, le WAL substrate accumule TOUTES les écritures de
            // l'import (EveryCommit) et le replay relit le fichier entier à
            // CHAQUE ouverture : mesuré 17 Mo (aletheia) / 122 Mo (harness)
            // pour des bases de 6-66k nœuds, soit open = 55-520 ms. Le
            // checkpoint (flush + troncature WAL + re-ancrage des index) est
            // sûr ici : le load est terminé, aucune écriture concurrente, et
            // le geste est identique à celui de `compact` (le chantier
            // « meilleure en tout »).
            if let Some(sub) = db.substrate_handle() {
                match sub.checkpoint() {
                    Ok(stats) => output::status(
                        &format!(
                            "WAL checkpointed after load ({} octets de WAL tronqués)",
                            stats.wal_offset_before
                        ),
                        quiet,
                    ),
                    Err(e) => {
                        output::status(&format!("Warning: WAL checkpoint failed: {e}"), false);
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_db(dir: &std::path::Path) -> obrain_engine::ObrainDB {
        let db = obrain_engine::ObrainDB::open(dir).expect("create db");
        let n1 = db.create_node(&["Person"]);
        let n2 = db.create_node(&["Company"]);
        db.set_node_property(n1, "name", Value::from("Alix"));
        db.set_node_property(n1, "age", Value::Int64(30));
        db.set_node_property(n2, "name", Value::from("Acme"));
        let e = db.create_edge(n1, n2, "WORKS_AT");
        db.set_edge_property(e, "since", Value::Int64(2020));
        db
    }

    #[test]
    fn test_dump_and_load_roundtrip() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("source.obrain");
        let dump_path = temp.path().join("dump.jsonl");
        let target_path = temp.path().join("target.obrain");

        let db = create_test_db(&db_path);
        drop(db);

        // Dump
        run(
            DataCommands::Dump {
                path: db_path,
                output: dump_path.clone(),
                export_format: None,
            },
            OutputFormat::Json,
            true, // quiet
        )
        .expect("dump should succeed");

        // Verify dump file exists and has content
        let content = std::fs::read_to_string(&dump_path).unwrap();
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(lines.len(), 3); // 2 nodes + 1 edge

        // Load into new database
        run(
            DataCommands::Load {
                input: dump_path,
                path: target_path.clone(),
                no_index: false,
            },
            OutputFormat::Json,
            true,
        )
        .expect("load should succeed");

        // Verify loaded data
        let loaded = obrain_engine::ObrainDB::open(&target_path).unwrap();
        let info = loaded.info();
        assert_eq!(info.node_count, 2);
        assert_eq!(info.edge_count, 1);
    }

    #[test]
    fn test_dump_explicit_json_format() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.obrain");
        let dump_path = temp.path().join("dump.json");

        // Create and drop the database so the CLI can reopen it
        drop(create_test_db(&db_path));

        run(
            DataCommands::Dump {
                path: db_path,
                output: dump_path.clone(),
                export_format: Some("json".to_string()),
            },
            OutputFormat::Json,
            true,
        )
        .expect("dump with explicit json format should succeed");

        let content = std::fs::read_to_string(&dump_path).unwrap();
        assert!(!content.is_empty());
    }

    #[test]
    fn test_dump_invalid_format_rejected() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.obrain");
        let dump_path = temp.path().join("dump.parquet");

        let _db = create_test_db(&db_path);

        let result = run(
            DataCommands::Dump {
                path: db_path,
                output: dump_path,
                export_format: Some("parquet".to_string()),
            },
            OutputFormat::Json,
            true,
        );

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("JSON Lines"));
    }

    #[test]
    fn test_load_invalid_json_fails() {
        let temp = TempDir::new().unwrap();
        let input_path = temp.path().join("bad.jsonl");
        let db_path = temp.path().join("target.obrain");

        std::fs::write(&input_path, "not valid json\n").unwrap();

        let result = run(
            DataCommands::Load {
                input: input_path,
                path: db_path,
                no_index: false,
            },
            OutputFormat::Json,
            true,
        );

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("line 1"));
    }

    #[test]
    fn test_load_unknown_type_fails() {
        let temp = TempDir::new().unwrap();
        let input_path = temp.path().join("unknown.jsonl");
        let db_path = temp.path().join("target.obrain");

        std::fs::write(&input_path, "{\"type\": \"widget\"}\n").unwrap();

        let result = run(
            DataCommands::Load {
                input: input_path,
                path: db_path,
                no_index: false,
            },
            OutputFormat::Json,
            true,
        );

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Unknown record type"));
    }

    #[test]
    fn test_load_skips_empty_lines() {
        let temp = TempDir::new().unwrap();
        let input_path = temp.path().join("sparse.jsonl");
        let db_path = temp.path().join("target.obrain");

        let content = "\n{\"type\":\"node\",\"labels\":[\"A\"],\"properties\":{}}\n\n";
        std::fs::write(&input_path, content).unwrap();

        run(
            DataCommands::Load {
                input: input_path,
                path: db_path.clone(),
                no_index: false,
            },
            OutputFormat::Json,
            true,
        )
        .expect("should handle empty lines");

        let db = obrain_engine::ObrainDB::open(&db_path).unwrap();
        assert_eq!(db.info().node_count, 1);
    }

    /// Compte les nœuds comme la preuve du PO : MATCH (n) RETURN count(n).
    fn compter_noeuds(db: &obrain_engine::ObrainDB) -> i64 {
        let session = db.session();
        let res = session
            .execute("MATCH (n) RETURN count(n)")
            .expect("count query");
        match &res.rows[0][0] {
            Value::Int64(n) => *n,
            other => panic!("valeur de count inattendue : {other:?}"),
        }
    }

    /// Forme canonique d'un dump JSONL : nœuds triés par id (ids conservés),
    /// arêtes sans leur champ `id` propre (jamais lu au load), triées.
    fn canoniser_dump(content: &str) -> (Vec<serde_json::Value>, Vec<String>) {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        for line in content.trim().lines() {
            let mut v: serde_json::Value = serde_json::from_str(line).unwrap();
            match v["type"].as_str().unwrap() {
                "node" => nodes.push(v),
                "edge" => {
                    v.as_object_mut().unwrap().remove("id");
                    edges.push(v.to_string());
                }
                other => panic!("type inconnu dans le dump : {other}"),
            }
        }
        nodes.sort_by_key(|v| v["id"].as_u64());
        edges.sort();
        (nodes, edges)
    }

    /// Reproduction preuve PO 563e670f : un dump à ids troués (1, 5, 9) ne
    /// doit PAS matérialiser les trous en nœuds fantômes, et les arêtes
    /// doivent être câblées vers les nœuds déclarés dans le dump.
    #[test]
    fn test_load_ids_troues_sans_noeuds_fantomes() {
        let temp = TempDir::new().unwrap();
        let input_path = temp.path().join("troue.jsonl");
        let db_path = temp.path().join("target.obrain");

        let content = concat!(
            "{\"type\":\"node\",\"id\":1,\"labels\":[\"A\"],\"properties\":{\"k\":{\"String\":\"n1\"}}}\n",
            "{\"type\":\"node\",\"id\":5,\"labels\":[\"A\"],\"properties\":{\"k\":{\"String\":\"n5\"}}}\n",
            "{\"type\":\"node\",\"id\":9,\"labels\":[\"A\"],\"properties\":{\"k\":{\"String\":\"n9\"}}}\n",
            "{\"type\":\"edge\",\"id\":1,\"source\":1,\"target\":5,\"edge_type\":\"T\",\"properties\":{}}\n",
            "{\"type\":\"edge\",\"id\":2,\"source\":5,\"target\":9,\"edge_type\":\"T\",\"properties\":{}}\n",
        );
        std::fs::write(&input_path, content).unwrap();

        run(
            DataCommands::Load {
                input: input_path,
                path: db_path.clone(),
                no_index: false,
            },
            OutputFormat::Json,
            true,
        )
        .expect("load should succeed");

        let db = obrain_engine::ObrainDB::open(&db_path).unwrap();
        // 3 nœuds déclarés → exactement 3 nœuds visibles, aucun fantôme.
        assert_eq!(
            compter_noeuds(&db),
            3,
            "les trous d'ids du dump ne doivent pas devenir des nœuds fantômes"
        );
        assert_eq!(db.info().node_count, 3);
        assert_eq!(db.info().edge_count, 2);

        // Câblage : n1 -T-> n5 et n5 -T-> n9, via la correspondance d'ids.
        let session = db.session();
        let res = session
            .execute("MATCH (a)-[:T]->(b) RETURN a.k, b.k")
            .expect("traversal query");
        let mut paires: Vec<(String, String)> = res
            .rows
            .iter()
            .map(|r| {
                (
                    r[0].as_str().expect("a.k doit être une chaîne").to_string(),
                    r[1].as_str().expect("b.k doit être une chaîne").to_string(),
                )
            })
            .collect();
        paires.sort();
        assert_eq!(
            paires,
            vec![
                ("n1".to_string(), "n5".to_string()),
                ("n5".to_string(), "n9".to_string())
            ],
            "les arêtes doivent être câblées via la correspondance id-dump → id-interne"
        );
    }

    /// Round-trip : un dump PRODUIT par `data dump` doit se recharger à
    /// l'identique (dump → load → dump : même graphe, ids stables).
    #[test]
    fn test_round_trip_dump_load_dump_identique() {
        let temp = TempDir::new().unwrap();
        let input_path = temp.path().join("troue.jsonl");
        let db1 = temp.path().join("db1.obrain");
        let db2 = temp.path().join("db2.obrain");
        let dump1 = temp.path().join("dump1.jsonl");
        let dump2 = temp.path().join("dump2.jsonl");

        let content = concat!(
            "{\"type\":\"node\",\"id\":2,\"labels\":[\"A\"],\"properties\":{\"k\":{\"String\":\"n2\"}}}\n",
            "{\"type\":\"node\",\"id\":7,\"labels\":[\"B\"],\"properties\":{\"k\":{\"String\":\"n7\"}}}\n",
            "{\"type\":\"node\",\"id\":11,\"labels\":[],\"properties\":{}}\n",
            "{\"type\":\"edge\",\"id\":3,\"source\":7,\"target\":2,\"edge_type\":\"R\",\"properties\":{\"w\":{\"Int64\":1}}}\n",
        );
        std::fs::write(&input_path, content).unwrap();

        run(
            DataCommands::Load {
                input: input_path,
                path: db1.clone(),
                no_index: false,
            },
            OutputFormat::Json,
            true,
        )
        .expect("load 1");
        run(
            DataCommands::Dump {
                path: db1,
                output: dump1.clone(),
                export_format: None,
            },
            OutputFormat::Json,
            true,
        )
        .expect("dump 1");
        run(
            DataCommands::Load {
                input: dump1.clone(),
                path: db2.clone(),
                no_index: false,
            },
            OutputFormat::Json,
            true,
        )
        .expect("load 2");
        run(
            DataCommands::Dump {
                path: db2,
                output: dump2.clone(),
                export_format: None,
            },
            OutputFormat::Json,
            true,
        )
        .expect("dump 2");

        let c1 = std::fs::read_to_string(&dump1).unwrap();
        let c2 = std::fs::read_to_string(&dump2).unwrap();
        assert_eq!(
            canoniser_dump(&c1),
            canoniser_dump(&c2),
            "dump → load → dump doit être un point fixe"
        );
    }

    /// Round-trip du vocabulaire de valeurs : un NDJSON mêlant les alias
    /// tiers ({"Number": ...}, {"Float": ...}) et les variantes natives
    /// (Int64, Float64, Bool, String) doit ressortir du dump SANS PERTE.
    ///
    /// Normalisation documentée : `{"Number": n}` entier → `{"Int64": n}`,
    /// `{"Number": x}` non entier et `{"Float": x}` → `{"Float64": x}`.
    /// L'ancien code perdait Number en `"Null"` silencieux (bug D-number-bug).
    #[test]
    fn test_round_trip_number_alias_valeurs_preservees() {
        let temp = TempDir::new().unwrap();
        let input_path = temp.path().join("mix.jsonl");
        let db_path = temp.path().join("target.obrain");
        let dump_path = temp.path().join("dump.jsonl");

        let content = concat!(
            "{\"type\":\"node\",\"id\":1,\"labels\":[\"T\"],\"properties\":{",
            "\"size\":{\"Number\":42},\"ratio\":{\"Number\":1.5}}}\n",
            "{\"type\":\"node\",\"id\":2,\"labels\":[\"T\"],\"properties\":{",
            "\"count\":{\"Int64\":7},\"name\":{\"String\":\"x\"},",
            "\"ok\":{\"Bool\":true},\"f\":{\"Float\":2.5},\"g\":{\"Float64\":3.5}}}\n",
        );
        std::fs::write(&input_path, content).unwrap();

        run(
            DataCommands::Load {
                input: input_path,
                path: db_path.clone(),
                no_index: false,
            },
            OutputFormat::Json,
            true,
        )
        .expect("load should succeed");
        run(
            DataCommands::Dump {
                path: db_path,
                output: dump_path.clone(),
                export_format: None,
            },
            OutputFormat::Json,
            true,
        )
        .expect("dump should succeed");

        let dumped = std::fs::read_to_string(&dump_path).unwrap();
        let (nodes, _edges) = canoniser_dump(&dumped);
        assert_eq!(nodes.len(), 2);
        let p1 = &nodes[0]["properties"];
        let p2 = &nodes[1]["properties"];
        assert_eq!(p1["size"], serde_json::json!({"Int64": 42}));
        assert_eq!(p1["ratio"], serde_json::json!({"Float64": 1.5}));
        assert_eq!(p2["count"], serde_json::json!({"Int64": 7}));
        assert_eq!(p2["name"], serde_json::json!({"String": "x"}));
        assert_eq!(p2["ok"], serde_json::json!({"Bool": true}));
        assert_eq!(p2["f"], serde_json::json!({"Float64": 2.5}));
        assert_eq!(p2["g"], serde_json::json!({"Float64": 3.5}));
        // Garde-fou : plus jamais de "Null" silencieux à la place d'une valeur.
        assert!(
            !dumped.contains("\"Null\""),
            "aucune propriété ne doit avoir été perdue en Null : {dumped}"
        );
    }

    /// Variante inconnue = erreur FRANCHE au load (jamais de Null silencieux).
    #[test]
    fn test_load_unknown_value_variant_fails() {
        let temp = TempDir::new().unwrap();
        let input_path = temp.path().join("widget.jsonl");
        let db_path = temp.path().join("target.obrain");

        let content = "{\"type\":\"node\",\"id\":1,\"labels\":[\"T\"],\"properties\":{\"size\":{\"Widget\":1}}}\n";
        std::fs::write(&input_path, content).unwrap();

        let result = run(
            DataCommands::Load {
                input: input_path,
                path: db_path,
                no_index: false,
            },
            OutputFormat::Json,
            true,
        );

        assert!(
            result.is_err(),
            "une variante inconnue doit faire échouer le load"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Widget"), "message: {err_msg}");
        assert!(err_msg.contains("line 1"), "message: {err_msg}");
    }

    #[test]
    fn test_load_edge_missing_source_fails() {
        let temp = TempDir::new().unwrap();
        let input_path = temp.path().join("bad_edge.jsonl");
        let db_path = temp.path().join("target.obrain");

        let content = "{\"type\":\"edge\",\"target\":1,\"edge_type\":\"KNOWS\"}\n";
        std::fs::write(&input_path, content).unwrap();

        let result = run(
            DataCommands::Load {
                input: input_path,
                path: db_path,
                no_index: false,
            },
            OutputFormat::Json,
            true,
        );

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("source"));
    }
}
