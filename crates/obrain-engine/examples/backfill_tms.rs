//! Backfill `t_ms` sur les nœuds Message/ToolCall/ToolResult d'un store runtime.
//!
//! Le harness a TOUJOURS enregistré `time` (ms) sur chaque événement, mais
//! l'import le jetait. Le journal brut est conservé en zone substrate : on le
//! relit ici et on pose `t_ms` sur les nœuds déjà importés (idempotent).
//!
//!   cargo run --release -p obrain-engine --example backfill_tms -- <store>

use obrain_common::types::{NodeId, Value};
use obrain_engine::ObrainDB;

fn call_id_of(data: &serde_json::Value) -> Option<String> {
    if let Some(msg) = data.get("message") {
        if let Some(content) = msg.get("content").and_then(|c| c.as_array()) {
            for block in content.iter().take(16) {
                if let Some(tcid) = block.get("toolCallId").and_then(|v| v.as_str()) {
                    return Some(tcid.to_string());
                }
            }
        }
    }
    data.get("toolCallId").and_then(|v| v.as_str()).map(String::from)
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: backfill_tms <store>");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    let session = db.session();

    let r = session
        .execute("MATCH (s:Session) RETURN s.qualified_name")
        .expect("liste des sessions");

    let mut noeuds = 0u64;
    let mut sessions = 0u64;
    for row in &r.rows {
        let Some(sqn) = row.first().map(|v| v.to_string()) else { continue };
        if sqn.is_empty() || sqn == "NULL" { continue; }

        let lines = db.read_session_journal(&sqn, -1, i64::MAX, 1_000_000);
        if lines.is_empty() { continue; }
        sessions += 1;

        for line in &lines {
            let Ok(ev) = serde_json::from_str::<serde_json::Value>(line) else { continue };
            let Some(ty) = ev.get("type").and_then(|v| v.as_str()) else { continue };
            let Some(t) = ev.get("time").and_then(|v| v.as_i64()) else { continue };

            let qn: Option<String> = match ty {
                "user/message" | "assistant/message" => {
                    ev.get("seq").and_then(|v| v.as_i64()).map(|s| format!("{sqn}:msg:{s}"))
                }
                "tool/call" => ev
                    .get("data")
                    .and_then(|d| d.get("callId"))
                    .and_then(|v| v.as_str())
                    .map(|c| format!("{sqn}:call:{c}")),
                "tool/result" => ev
                    .get("data")
                    .and_then(call_id_of)
                    .map(|c| format!("{sqn}:res:{c}")),
                _ => None,
            };
            let Some(qn) = qn else { continue };

            let fq = format!(
                "MATCH (n {{qualified_name: '{}'}}) RETURN id(n) LIMIT 1",
                qn.replace('\'', "\\'")
            );
            if let Ok(rr) = session.execute(&fq) {
                if let Some(row) = rr.rows.first() {
                    if let Some(id) = row.first().and_then(|v| v.as_int64()) {
                        db.set_node_property(NodeId(id as u64), "t_ms", Value::Int64(t));
                        noeuds += 1;
                    }
                }
            }
        }
    }

    db.checkpoint().expect("checkpoint");
    println!("backfill t_ms : {noeuds} propriétés posées sur {sessions} sessions");
}
