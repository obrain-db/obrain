//! Banc d essai ORBITES : compare les formes de requete de decoupage.
//! Usage : banc_orbites <store> <variante>
use obrain_common::types::Value as OV;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let store = a.get(1).cloned().unwrap_or_default();
    let var = a.get(2).cloned().unwrap_or_else(|| "a".into());
    let db = obrain_engine::ObrainDB::open_read_only(std::path::Path::new(&store))?;
    let sess = db.session();
    let p = "'";
    let q = match var.as_str() {
        // a : la requete ACTUELLE (un rang)
        "a" => format!(
            "MATCH (n) WHERE n.qualified_name CONTAINS {p}.{p} RETURN split(n.qualified_name, {p}.{p})[2] AS m, count(n) AS c ORDER BY c DESC LIMIT 200"
        ),
        // b : multi-cles via WITH (tuple de segments)
        "b" => format!(
            "MATCH (n) WHERE n.qualified_name CONTAINS {p}.{p} WITH split(n.qualified_name, {p}.{p}) AS s, n RETURN s[0] AS a0, s[1] AS a1, s[2] AS a2, s[3] AS a3, s[4] AS a4, s[5] AS a5, s[6] AS a6, count(n) AS c"
        ),
        // c : projection brute du qualified_name
        "c" => format!(
            "MATCH (n) WHERE n.qualified_name CONTAINS {p}.{p} RETURN n.qualified_name AS q"
        ),
        // d : groupement sur le qualified_name entier
        "d" => format!(
            "MATCH (n) WHERE n.qualified_name CONTAINS {p}.{p} RETURN n.qualified_name AS q, count(n) AS c"
        ),
        // e : deux rangs en une requete (RETURN direct)
        "e" => format!(
            "MATCH (n) WHERE n.qualified_name CONTAINS {p}.{p} RETURN split(n.qualified_name, {p}.{p})[2] AS a2, split(n.qualified_name, {p}.{p})[3] AS a3, count(n) AS c"
        ),
        // f : cinq rangs en une requete (RETURN direct) - la cible
        "f" => format!(
            "MATCH (n) WHERE n.qualified_name CONTAINS {p}.{p} RETURN split(n.qualified_name, {p}.{p})[2] AS a2, split(n.qualified_name, {p}.{p})[3] AS a3, split(n.qualified_name, {p}.{p})[4] AS a4, split(n.qualified_name, {p}.{p})[5] AS a5, split(n.qualified_name, {p}.{p})[6] AS a6, count(n) AS c"
        ),
        // g : projection brute sans filtre CONTAINS
        "g" => format!("MATCH (n) RETURN n.qualified_name AS q"),
        // h : comptage nu
        "h" => format!("MATCH (n) RETURN count(n) AS c"),
        autre => autre.to_string(),
    };
    eprintln!("REQUETE: {q}");
    let t = Instant::now();
    let r = sess.execute(&q);
    let ms = t.elapsed().as_millis();
    match r {
        Ok(res) => {
            println!("OK variante={var} ms={ms} lignes={}", res.rows.len());
            for row in res.rows.iter().take(6) {
                let cells: Vec<String> = row
                    .iter()
                    .map(|v| match v {
                        OV::String(s) => format!("{s:?}"),
                        OV::Int64(i) => i.to_string(),
                        autre => format!("{autre:?}"),
                    })
                    .collect();
                println!("  {}", cells.join(" | "));
            }
        }
        Err(e) => println!("ERR variante={var} ms={ms} : {e}"),
    }
    Ok(())
}
