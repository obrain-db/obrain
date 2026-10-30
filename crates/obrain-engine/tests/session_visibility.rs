// Diagnostic (2026-09-04) : le binding NAPI renvoie 0 ligne à MATCH tandis que
// le CLI fonctionne. Hypothèse : with_session (session NEUVE par appel) ne voit
// pas les écritures d'une autre session ; une session TENUE les voit.
use obrain_engine::ObrainDB;

#[test]
fn with_session_vs_held_session_visibility() {
    let dir = std::env::temp_dir().join(format!("obrain-sv-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.obrain");

    let db = ObrainDB::open_substrate(&path).unwrap();

    // A — écriture via with_session (db.execute), lecture via with_session
    db.execute("INSERT (:File {qualified_name: 'f1', path: 'a.js'})").unwrap();
    let ra = db.execute("MATCH (f:File) RETURN f.path AS path").unwrap();
    println!("A with_session: INSERT puis MATCH → {} ligne(s)", ra.rows.len());

    // B — session TENUE : INSERT puis MATCH sur la même session
    let s = db.session();
    s.execute("INSERT (:File {qualified_name: 'f2', path: 'b.js'})").unwrap();
    let rb = s.execute("MATCH (f:File) RETURN f.path AS path").unwrap();
    println!("B session tenue: INSERT puis MATCH → {} ligne(s)", rb.rows.len());

    // C — session NEUVE (comme with_session) APRÈS les écritures : voit-elle le fichier ?
    let s2 = db.session();
    let rc = s2.execute("MATCH (f:File) RETURN f.path AS path").unwrap();
    println!("C session neuve après écritures → {} ligne(s)", rc.rows.len());

    std::fs::remove_dir_all(&dir).ok();
    assert!(ra.rows.len() >= 1 || rb.rows.len() >= 1, "au moins un chemin doit voir ses écritures");
}
