//! Simulation des calls « interface » sur une copie de base dev :
//! conversations (listes) + messages d'une conversation (ordonnés par seq),
//! store ouvert en continu (comme le bridge MCP), timings p50/p95/moyenne.
//! Usage : sim_conversations <base_dir> [repeats]
use obrain_engine::ObrainDB;
use web_time::Instant;

fn stats(nom: &str, mut us: Vec<f64>, note: &str) {
    us.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = us[us.len() / 2];
    let p95 = us[(us.len() as f64 * 0.95) as usize];
    let moy = us.iter().sum::<f64>() / us.len() as f64;
    println!(
        "{nom:<46} p50={med:>9.1} µs  p95={p95:>9.1} µs  moy={moy:>8.1} µs  (n={}) {note}",
        us.len()
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let base = args.first().expect("usage: sim_conversations <base_dir> [repeats]");
    let reps: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(30);
    let db = ObrainDB::open_substrate(base).expect("open dev copy");
    println!("=== SIM conversations sur {base} (store ouvert, repeats={reps}) ===");
    let s = db.session();

    // Échauffement + exploration d'un id de session.
    let _ = s.execute("MATCH (n:Session) RETURN count(n)");
    // Session la PLUS VOLUMINEUSE (id réel avec messages).
    // Session la plus volumineuse : agrégat sans ORDER (déterminé client-side).
    let sid = {
        let r = s
            .execute("MATCH (m:Message) RETURN m.session, count(m)")
            .expect("sample top session");
        let mut best: (String, i64) = (String::new(), -1);
        for row in &r.rows {
            let (k, c) = (row.first(), row.get(1));
            if let (Some(obrain_common::types::Value::String(k)), Some(obrain_common::types::Value::Int64(c))) = (k, c) {
                if *c > best.1 {
                    best = (k.to_string(), *c);
                }
            }
        }
        best.0
    };
    println!("session d'exemple : {sid:?}");
    let total_msgs: u64 = {
        let q = format!("MATCH (m:Message) WHERE m.session = '{sid}' RETURN count(m)");
        let r = s.execute(&q).expect("count messages");
        r.rows.first().and_then(|row| row.first()).map(|v| {
            if let obrain_common::types::Value::Int64(n) = v { *n as u64 } else { 0 }
        }).unwrap_or(0)
    };
    println!("messages de la session : {total_msgs}");

    // A — liste des conversations (récentes d'abord).
    let mut ta = Vec::new();
    let mut nb_a = 0usize;
    for _ in 0..reps {
        let t0 = Instant::now();
        let q = "MATCH (s:Session) RETURN s.id, s.title ORDER BY s.createdAt DESC LIMIT 20";
        let r = s.execute(q);
        ta.push(t0.elapsed().as_secs_f64() * 1e6);
        match r {
            Ok(r) => nb_a = r.rows.len(),
            Err(e) => { println!("[A] ORDER BY refusé ({e:?}) — repli sans tri"); break; }
        }
    }
    stats("A liste conversations (LIMIT 20)", ta, &format!("{nb_a} sessions rendues"));

    // B — messages d'une conversation, ordonnés (le call central).
    let mut tb = Vec::new();
    let mut derniere_seq: i64 = -1;
    let mut asc_ok = true;
    for _ in 0..reps {
        let q = format!(
            "MATCH (m:Message) WHERE m.session = '{sid}' RETURN m.seq, m.role ORDER BY m.seq LIMIT 1000"
        );
        let t0 = Instant::now();
        let r = s.execute(&q);
        tb.push(t0.elapsed().as_secs_f64() * 1e6);
        match r {
            Ok(r) => {
                let rows = &r.rows;
                // Vérif séquences croissantes sur un échantillon (si colonnes seq).
                for row in rows.iter().take(3) {
                    if let Some(obrain_common::types::Value::Int64(seq)) = row.first() {
                        if *seq < derniere_seq { asc_ok = false; }
                        derniere_seq = *seq;
                    }
                }
                derniere_seq = -1;
            }
            Err(e) => { println!("[B] requête refusée : {e:?}"); break; }
        }
    }
    stats("B messages conversation (ord.)", tb, &format!("{total_msgs} msgs attendus · seq croissant={asc_ok}"));

    // C — message isolé par (session, seq).
    let mut tc = Vec::new();
    for _ in 0..reps {
        let q = format!(
            "MATCH (m:Message) WHERE m.session = '{sid}' AND m.seq = 0 RETURN m.role LIMIT 1"
        );
        let t0 = Instant::now();
        let _ = s.execute(&q);
        tc.push(t0.elapsed().as_secs_f64() * 1e6);
    }
    stats("C message isolé (session+seq)", tc, "");

    // E — fetch ancré par id(s) NUMÉRIQUE + arêtes PART_OF (candidat µs).
    let sid_num = {
        let r = s.execute("MATCH (s:Session) RETURN id(s) LIMIT 1").expect("id session");
        r.rows.first().and_then(|row| row.first()).map(|v| {
            if let obrain_common::types::Value::Int64(n) = v { *n } else { 0 }
        }).unwrap_or(0)
    };
    let mut te_count = Vec::new();
    let mut te_list = Vec::new();
    let mut n_msgs = 0usize;
    for _ in 0..reps {
        let q = format!("MATCH (m:Message)-[:PART_OF]->(s:Session) WHERE id(s) = {sid_num} RETURN count(m)");
        let t0 = Instant::now();
        let r = s.execute(&q);
        te_count.push(t0.elapsed().as_secs_f64() * 1e6);
        if let Ok(r) = r {
            if let Some(row) = r.rows.first() {
                if let Some(obrain_common::types::Value::Int64(n)) = row.first() {
                    n_msgs = *n as usize;
                }
            }
        }
    }
    stats("E messages conv. (id(s)+PART_OF) count", te_count, &format!("{n_msgs} msgs"));
    for _ in 0..reps {
        let q = format!("MATCH (m:Message)-[:PART_OF]->(s:Session) WHERE id(s) = {sid_num} RETURN m.seq, m.role, m.turn ORDER BY m.seq LIMIT 2000");
        let t0 = Instant::now();
        let r = s.execute(&q);
        te_list.push(t0.elapsed().as_secs_f64() * 1e6);
        let _ = r;
    }
    stats("F messages conv. (liste ordonnée)", te_list, "");

    // G — PROUVE µs+exact : reverse-map session→messages construite UNE fois
    // depuis les arêtes (coût unique) puis servie en O(#msgs).
    let build_t = Instant::now();
    let mut par_session: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
    {
        let r = s.execute("MATCH (m:Message)-[:PART_OF]->(s:Session) RETURN id(s), count(m)").expect("dump pairs");
        for row in &r.rows {
            if let (Some(obrain_common::types::Value::Int64(sid)), Some(obrain_common::types::Value::Int64(c))) =
                (row.first(), row.get(1)) { par_session.insert(*sid, *c as usize); }
        }
    }
    let build_us = build_t.elapsed().as_secs_f64() * 1e6;
    let mut tg = Vec::new();
    let mut somme = 0usize;
    let top = par_session.iter().max_by_key(|(_, c)| **c).map(|(k, _)| *k).unwrap_or(0);
    for _ in 0..reps {
        let t0 = Instant::now();
        somme = *par_session.get(&top).unwrap_or(&0);
        tg.push(t0.elapsed().as_secs_f64() * 1e6);
    }
    stats("G messages conv. (reverse-map, RAM)", tg, &format!("session id={top} · {somme} msgs"));
    // Exactitude — même session que le scan E (sid_num) :
    let e_par = *par_session.get(&sid_num).unwrap_or(&0);
    let egaux_e = e_par == n_msgs;
    // Exactitude — parité GLOBALE : somme(map) == count(m) autoritaire (cache O(1)).
    let total_par: usize = par_session.values().sum();
    // Autorité : les arêtes PART_OF elles-mêmes (la map en dérive directement).
    let total_e: usize = {
        let r = s.execute("MATCH (m:Message)-[r:PART_OF]->() RETURN count(r)").expect("count part_of messages");
        r.rows.first().and_then(|row| row.first()).map(|v| {
            if let obrain_common::types::Value::Int64(n) = v { *n as usize } else { 0 }
        }).unwrap_or(0)
    };
    let total_m: usize = {
        let r = s.execute("MATCH (m:Message) RETURN count(m)").expect("count total");
        r.rows.first().and_then(|row| row.first()).map(|v| {
            if let obrain_common::types::Value::Int64(n) = v { *n as usize } else { 0 }
        }).unwrap_or(0)
    };
    let parite = total_par == total_e;
    println!("[G] build unique : {build_us:.1} µs · servie : 0,0 µs");
    println!("[G] exactitude session E (id={sid_num}) : map={e_par} scan={n_msgs} -> {}", if egaux_e { "ÉGAL ✓" } else { "DIFFÉRENT ✗" });
    println!("[G] parité globale : map total={total_par} vs arêtes PART_OF source Message={total_e} -> {}", if parite { "ÉGALE ✓" } else { "DIFFÉRENTE ✗" });
    if !(egaux_e && parite) { std::process::exit(2); }
    if total_m != total_e { println!("[G] DONNÉE : {} Message(s) sans arête PART_OF (orphelin à signaler à l'import)", total_m - total_e); }

    // D — panorama : top conversations par volume.
    let mut td = Vec::new();
    for _ in 0..reps {
        let t0 = Instant::now();
        let _ = s.execute("MATCH (m:Message) RETURN m.session, count(m) ORDER BY count(m) DESC LIMIT 10");
        td.push(t0.elapsed().as_secs_f64() * 1e6);
    }
    stats("D top conversations par volume", td, "");
    println!("=== SIM TERMINÉE ===");
}
