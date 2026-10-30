//! Timeline d'exécution d'une session — messages, appels d'outils et résultats,
//! fusionnés en une séquence ordonnée par `seq`, avec l'identité et les tokens.
//!
//! La donnée est déjà dans le graphe (`Message -[:PART_OF]-> Session` ordonné
//! par `seq`, et `Session -[:HAS_TURN]-> Turn -[:HAS_STEP]-> Step -[:INVOKED]->
//! ToolCall -[:PRODUCED]-> ToolResult`) : cette route ne fait que la RELIRE en
//! une forme plate, triée, directement rendable par le dashboard.
//!
//! SÉCURITÉ : le paramètre `session` vient de la query string (entrée non
//! fiable) ; il est échappé par [`lit`] avant d'entrer dans le GQL — jamais
//! interpolé nu (une apostrophe fermerait le littéral et ouvrirait l'injection).

use std::net::TcpStream;

use anyhow::Result;
use obrain_common::types::Value as OV;

use crate::viz_serve::{cached_ro, parse_query, respond, ServeConfig, VIZ_JSON};

/// `GET /api/viz/timeline?session=…&store=…` — le fil d'une session, trié.
pub(crate) fn api_viz_timeline(
    stream: &mut TcpStream,
    query: Option<&str>,
    config: &ServeConfig,
) -> Result<()> {
    let params = parse_query(query);
    let Some(session) = params.get("session").cloned() else {
        return respond(stream, 400, VIZ_JSON, r#"{"error":"session requise"}"#);
    };
    let store = config.resolve(params.get("store").map(String::as_str));
    let Some(store) = store else {
        return respond(stream, 400, VIZ_JSON, r#"{"error":"store requis"}"#);
    };
    let db = match cached_ro(&store) {
        Ok(db) => db,
        Err(e) => {
            return respond(
                stream,
                400,
                VIZ_JSON,
                &format!("{{\"error\":\"ouverture RO : {e}\"}}"),
            );
        }
    };

    let compte = db
        .session()
        .execute(&format!(
            "MATCH (s:Session {{qualified_name: {}}})-[:OWNED_BY]->(a:Account) RETURN a.name LIMIT 1",
            lit(&session)
        ))
        .ok()
        .and_then(|r| r.rows.into_iter().next())
        .map(|row| texte(row.first()));

    let evenements = fil(&db, &session);

    // Sous-agents (forks `agent_fork`/`spawn`) : un nœud Subagent -[:CHILD_OF]->
    // cette session, chacun portant SA propre session (même UUID, préfixe
    // `session:`) — donc son propre fil, superposé dans le temps au fil parent.
    let sq = format!(
        "MATCH (sub:Subagent)-[:CHILD_OF]->(s:Session {{qualified_name: {}}}) RETURN sub.qualified_name, sub.name",
        lit(&session)
    );
    let mut agents: Vec<serde_json::Value> = Vec::new();
    if let Ok(r) = db.session().execute(&sq) {
        for row in &r.rows {
            let sub_qn = texte(row.first());
            let sous_session = sub_qn.replacen("subagent:", "session:", 1);
            let ev = fil(&db, &sous_session);
            agents.push(serde_json::json!({
                "qn": sub_qn,
                "name": texte(row.get(1)),
                "evenements": ev.into_iter().map(|(_, e)| e).collect::<Vec<_>>(),
            }));
        }
    }

    let body = serde_json::json!({
        "session": session,
        "compte": compte,
        "evenements": evenements.into_iter().map(|(_, e)| e).collect::<Vec<_>>(),
        "agents": agents,
    });

    respond(stream, 200, VIZ_JSON, &body.to_string())
}

/// Le fil d'une session (messages + appels d'outils + résultats), trié par `seq`.
fn fil(db: &obrain_engine::ObrainDB, session: &str) -> Vec<(i64, serde_json::Value)> {
    // messages : seq, role, text, inputTokens, outputTokens, cacheReadTokens
    let mq = format!(
        "MATCH (m:Message)-[:PART_OF]->(s:Session {{qualified_name: {}}}) \
         RETURN m.seq, m.role, m.text, m.inputTokens, m.outputTokens, m.cacheReadTokens, m.t_ms \
         ORDER BY m.seq LIMIT 5000",
        lit(session)
    );
    // appels : seq, name, args, turn, step, callId, r.seq, r.text
    let cq = format!(
        "MATCH (s:Session {{qualified_name: {}}})-[:HAS_TURN]->(:Turn)-[:HAS_STEP]->(:Step)-[:INVOKED]->(tc:ToolCall) \
         OPTIONAL MATCH (tc)-[:PRODUCED]->(tr:ToolResult) \
         RETURN tc.seq, tc.name, tc.text, tc.turn, tc.step, tc.callId, tr.seq, tr.text, tc.t_ms, tr.t_ms \
         ORDER BY tc.seq LIMIT 20000",
        lit(session)
    );

    let mut evenements: Vec<(i64, serde_json::Value)> = Vec::new();

    if let Ok(r) = db.session().execute(&mq) {
        for row in &r.rows {
            let seq = entier(row.first()).unwrap_or(0);
            evenements.push((
                seq,
                serde_json::json!({
                    "seq": seq,
                    "kind": "message",
                    "role": texte(row.get(1)),
                    "text": texte(row.get(2)),
                    "t_ms": entier(row.get(6)).unwrap_or(0),
                    "tokens": {
                        "in": entier(row.get(3)).unwrap_or(0),
                        "out": entier(row.get(4)).unwrap_or(0),
                        "cache": entier(row.get(5)).unwrap_or(0),
                    }
                }),
            ));
        }
    }

    if let Ok(r) = db.session().execute(&cq) {
        for row in &r.rows {
            let seq = entier(row.first()).unwrap_or(0);
            let call_id = texte(row.get(5));
            evenements.push((
                seq,
                serde_json::json!({
                    "seq": seq,
                    "kind": "appel",
                    "name": texte(row.get(1)),
                    "args": texte(row.get(2)),
                    "turn": entier(row.get(3)).unwrap_or(0),
                    "step": entier(row.get(4)).unwrap_or(0),
                    "callId": call_id,
                    "t_ms": entier(row.get(8)).unwrap_or(0),
                }),
            ));
            if let Some(rs) = entier(row.get(6)) {
                evenements.push((
                    rs,
                    serde_json::json!({
                        "seq": rs,
                        "kind": "resultat",
                        "callId": call_id,
                        "text": texte(row.get(7)),
                        "t_ms": entier(row.get(9)).unwrap_or(0),
                    }),
                ));
            }
        }
    }

    evenements.sort_by_key(|(s, _)| *s);
    evenements
}

/// `GET /api/viz/sessions?store=…&limit=…` — les sessions du store runtime,
/// les plus récentes d'abord, pour l'explorateur du dashboard.
pub(crate) fn api_viz_sessions(
    stream: &mut TcpStream,
    query: Option<&str>,
    config: &ServeConfig,
) -> Result<()> {
    let params = parse_query(query);
    let store = config.resolve(params.get("store").map(String::as_str));
    let Some(store) = store else {
        return respond(stream, 400, VIZ_JSON, r#"{"error":"store requis"}"#);
    };
    let db = match cached_ro(&store) {
        Ok(db) => db,
        Err(e) => {
            return respond(
                stream,
                400,
                VIZ_JSON,
                &format!("{{\"error\":\"ouverture RO : {e}\"}}"),
            );
        }
    };
    // `limit` vient de la query string : borné pour qu'un appelant ne puisse pas
    // forcer un balayage déraisonnable.
    let limit = params
        .get("limit")
        .and_then(|l| l.parse::<u64>().ok())
        .unwrap_or(200)
        .min(1000);

    let q = format!(
        "MATCH (s:Session) \
         RETURN s.qualified_name, s.title, s.createdAt, s.source_path, id(s) \
         ORDER BY id(s) DESC LIMIT {limit}"
    );
    let Ok(r) = db.session().execute(&q) else {
        return respond(stream, 200, VIZ_JSON, r#"{"sessions":[]}"#);
    };

    let sessions: Vec<serde_json::Value> = r
        .rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "qn": texte(row.first()),
                "title": texte(row.get(1)),
                "createdAt": texte(row.get(2)),
                "source_path": texte(row.get(3)),
            })
        })
        .collect();

    respond(
        stream,
        200,
        VIZ_JSON,
        &serde_json::json!({ "sessions": sessions }).to_string(),
    )
}

/// LITTÉRAL GQL ÉCHAPPÉ — `'` → `\'`, `\` → `\\`, `\n`/`\r` neutralisés.
/// Même convention que `memoire::lit` / `viz_weave::lit` (le moteur lit les
/// littéraux à simple quote).
fn lit(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('\'');
    out
}

fn texte(v: Option<&OV>) -> String {
    match v {
        Some(OV::String(s)) => s.to_string(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

fn entier(v: Option<&OV>) -> Option<i64> {
    match v {
        Some(OV::Int64(i)) => Some(*i),
        _ => None,
    }
}
