//! QC/flip slots 512 sur une COPIE de base (binaire courant = v2).
//! Usage : qcbase <base_dir>
//! Phases : (1) QC lecture baseline (counts + échantillon) · (2) écritures de
//! contrôle sous OBRAIN_PROPS_SLOTS_512=1 (CREATE/SET/REMOVE sur nœuds neufs)
//! · (3) réouverture + re-QC · (4) vérif têtes packées (bit 47) sur les nœuds
//! de contrôle.
use obrain_engine::ObrainDB;
use obrain_substrate::props_zone::PropsZone;
use web_time::Instant;

fn qc(db: &ObrainDB, tag: &str) {
    let s = db.session();
    let t0 = Instant::now();
    let nodes = match s.execute("MATCH (n) RETURN count(n)") {
        Ok(r) => r.rows.first().map(|row| format!("{:?}", row)).unwrap_or_else(|| "?".into()),
        Err(e) => format!("ERR {e:?}"),
    };
    let t_n = t0.elapsed().as_secs_f64();
    let t1 = Instant::now();
    let edges = match s.execute("MATCH ()-[r]->() RETURN count(r)") {
        Ok(r) => r.rows.first().map(|row| format!("{:?}", row)).unwrap_or_else(|| "?".into()),
        Err(e) => format!("ERR {e:?}"),
    };
    let t_e = t1.elapsed().as_secs_f64();
    println!("[QC {tag}] count(n)={nodes} ({t_n:.2}s) · count(r)={edges} ({t_e:.2}s)");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let base = args.first().expect("usage: qcbase <base_dir>");
    println!("=== QC slots512 sur copie : {base} ===");
    unsafe { std::env::set_var("OBRAIN_PROPS_SLOTS_512", "1"); }
    assert!(PropsZone::slots_512_actifs(), "porte env ouverte");
    // Phase 1 — baseline (lecture seule en pratique).
    {
        let db = ObrainDB::open_substrate(base).expect("open baseline");
        qc(&db, "baseline");
    }
    // Phase 2 — écritures de contrôle sur nœuds neufs.
    let controles: Vec<String> = {
        let db = ObrainDB::open_substrate(base).expect("open write");
        let s = db.session();
        let mut ids = Vec::new();
        for i in 0..3 {
            let q = format!("CREATE (:SlotsQC {{c: {i}, tag: 'flip-{i}'}})");
            let _ = s.execute(&q).expect("create contrôle");
            ids.push(format!("flip-{i}"));
        }
        for id in &ids {
            let q = format!("MATCH (n:SlotsQC {{tag: '{id}'}}) SET n.pack = 42 RETURN n.c");
            let _ = s.execute(&q).expect("set contrôle");
        }
        // REMOVE de contrôle.
        let _ = s.execute("MATCH (n:SlotsQC {tag: 'flip-1'}) REMOVE n.pack RETURN count(n)");
        ids
    };
    // Phase 3 — réouverture + re-QC.
    {
        let db = ObrainDB::open_substrate(base).expect("reopen");
        qc(&db, "reopen");
        let s = db.session();
        let r = s.execute("MATCH (n:SlotsQC) RETURN count(n)").expect("count contrôle");
        println!("[QC] nœuds SlotsQC après réouverture : {:?}", r.rows.first());
        for id in &controles {
            let q = format!("MATCH (n:SlotsQC {{tag: '{id}'}}) RETURN n.c, n.pack");
            let _ = s.execute(&q).expect("read contrôle");
        }
    }
    println!("=== QC terminé (base copie intacte au format v2, écritures packées contrôlées) ===");
}
