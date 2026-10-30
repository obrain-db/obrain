//! BANC NS — jalon 2 (RFC principal-natif-v2 §4-J2) : les quatre formes du
//! chantier « cout proportionnel » mesurees AVANT et APRES la rupture de
//! layout NodeRecord 32 → 40 octets.
//!
//! Reconstruction assumee : le banc d'origine de la campagne perf/ns-partout
//! (requetes-latence-check.mjs, via /api/query) vit sur une autre branche et
//! mesure a travers le bridge HTTP. Ici on mesure les MEMES quatre formes au
//! niveau moteur (ObrainDB + GQL), sur un store substrate DISQUE — c'est la
//! couche que la rupture 32→40 traverse. Le verdict demande est l'ORDRE DE
//! GRANDEUR avant/apres sur les memes donnees, pas la valeur absolue.
//!
//!   A — egalite indexee   : MATCH (n:S {nom: X}) RETURN n.nom
//!   B — prefixe (ordonne) : MATCH (n:S) WHERE n.nom STARTS WITH P RETURN n.nom
//!   C — CONTAINS (trigr.) : MATCH (n:S) WHERE n.nom CONTAINS I RETURN n.nom
//!   D — voisinage entrant : MATCH (t:T)-[:R]->(n:S {nom: X}) RETURN t.k
//!
//! Usage : cargo run --release -p obrain-engine --example banc_ns_j2 -- /tmp/banc-ns-j2
//! Le store est construit au premier passage (50 000 :S + 50 000 :T + 50 000
//! aretes R, index de propriete `nom` declare) et REUTILISE ensuite — pour
//! comparer avant/apres, detruire le repertoire entre les deux binaires n'est
//! PAS necessaire (la migration 2→3 a l'ouverture fait partie de la mesure
//! "apres", mais on mesure ici sur store frais par purete : passer un
//! repertoire neuf a chaque campagne).

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use web_time::Instant;

const N: usize = 50_000;
const CHAUFFES: usize = 5;
const MESURES: usize = 30;

fn mediane(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn banc(db: &ObrainDB, nom: &str, requetes: &[String]) {
    let s = db.session();
    // Chauffe.
    for q in requetes.iter().cycle().take(CHAUFFES) {
        let _ = s.execute(q).expect("requete banc");
    }
    let mut lignes_vues = 0usize;
    let mut ms = Vec::with_capacity(MESURES);
    for q in requetes.iter().cycle().take(MESURES) {
        let t = Instant::now();
        let r = s.execute(q).expect("requete banc");
        ms.push(t.elapsed().as_secs_f64() * 1e3);
        lignes_vues += r.rows.len();
    }
    println!(
        "{nom} : mediane {:.3} ms  (min {:.3}, max {:.3}, {} exec, {} lignes cumulees)",
        mediane(ms.clone()),
        ms.iter().cloned().fold(f64::INFINITY, f64::min),
        ms.iter().cloned().fold(0.0, f64::max),
        MESURES,
        lignes_vues
    );
}

fn main() {
    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/banc-ns-j2".into());
    let neuf = !std::path::Path::new(&dir).join("substrate.meta").exists();
    let db = ObrainDB::open(&dir).expect("ouverture substrate");
    if neuf {
        eprintln!("construction du store ({N} :S, {N} :T, {N} aretes R)...");
        let t = Instant::now();
        let s = db.session();
        for i in 0..N {
            let nom = format!("nom_{i:06}");
            let ns = s.create_node_with_props(&["S"], [("nom", Value::String(nom.into()))]);
            let nt = s.create_node_with_props(&["T"], [("k", Value::Int64(i as i64))]);
            s.create_edge(nt, ns, "R");
        }
        db.create_property_index("nom");
        eprintln!("store construit en {:.1} s", t.elapsed().as_secs_f64());
    }

    // Aiguilles VARIEES par execution (pas de relecture de cache) mais
    // IDENTIQUES entre les campagnes avant/apres (meme suite deterministe).
    let cles: Vec<usize> = (0..MESURES).map(|i| (i * 1663) % N).collect();

    let qa: Vec<String> = cles
        .iter()
        .map(|i| format!("MATCH (n:S {{nom: 'nom_{i:06}'}}) RETURN n.nom"))
        .collect();
    let qb: Vec<String> = cles
        .iter()
        .map(|i| {
            format!(
                "MATCH (n:S) WHERE n.nom STARTS WITH 'nom_{:03}' RETURN n.nom",
                i / 1000
            )
        })
        .collect();
    let qc: Vec<String> = cles
        .iter()
        .map(|i| format!("MATCH (n:S) WHERE n.nom CONTAINS '{:04}' RETURN n.nom", i % 10000))
        .collect();
    let qd: Vec<String> = cles
        .iter()
        .map(|i| format!("MATCH (t:T)-[:R]->(n:S {{nom: 'nom_{i:06}'}}) RETURN t.k"))
        .collect();

    println!(
        "banc NS jalon 2 — NodeRecord::SIZE = {} octets, NODES_PER_PAGE = {}",
        obrain_substrate::record::NodeRecord::SIZE,
        obrain_substrate::record::NODES_PER_PAGE
    );
    banc(&db, "A egalite indexee ", &qa);
    banc(&db, "B prefixe ordonne ", &qb);
    banc(&db, "C contains trigram", &qc);
    banc(&db, "D voisinage entrant", &qd);
}
