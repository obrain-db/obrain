//! Banc de l'ouverture en LECTURE SEULE : durée d'ouverture, RSS, latence d'une
//! lecture par identifiant et d'une recherche indexée.
//!
//! ```text
//! banc_lecture_seule fabriquer <dossier> <noeuds> <fenetre> [pas]
//! banc_lecture_seule mesurer   <dossier> <ouvertures> [etiquette] [nommes]
//! banc_lecture_seule perimer   <dossier>
//! ```
//!
//! * `fabriquer` écrit `<noeuds>` nœuds (une propriété texte, une propriété
//!   entière, une arête vers le précédent), déclare l'index d'égalité, flushe,
//!   puis écrit `<fenetre>` nœuds de plus SANS flush et quitte par
//!   `process::exit` — aucun destructeur : c'est l'écrivain interrompu, avec sa
//!   fenêtre WAL. `<fenetre> = 0` : base fermée proprement.
//! * `perimer` inverse un octet du CRC de la colonne de degrés.
//! * `mesurer` ouvre `<ouvertures>` fois en lecture seule (durée de chaque
//!   ouverture), garde la dernière ouverte, relève le RSS, puis mesure la
//!   latence par lots de 1 000 opérations (l'horloge d'Apple Silicon bat à
//!   ~42 ns : une opération seule n'est pas mesurable) et sort UNE ligne TSV.
//!
//! N'utilise que des API présentes avant comme après le correctif de la
//! lecture seule : le même source se compile sur les deux arbres.

// Un seul bloc `unsafe` : `getrusage`, pour compter les défauts de page.
#![allow(unsafe_code)]

use std::path::Path;
use std::time::Instant;

use obrain_common::types::{NodeId, Value};
use obrain_core::graph::{GraphStore as _, GraphStoreMut as _};
use obrain_substrate::SubstrateStore;
use obrain_substrate::wal_io::SyncMode;

fn ecrire(s: &SubstrateStore, debut: u64, n: u64, pas: u64) {
    let mut precedent: Option<NodeId> = None;
    for i in debut..debut + n {
        let id = s.create_node(&["Personne"]);
        // Un nœud sur `pas` porte des propriétés (`nom` = p<rang>) : la zone
        // de propriétés v2 prend une page de 4 Kio par nœud qui en porte,
        // et c'est elle qui fixe la taille de la base.
        if i % pas == 0 {
            s.set_node_property(id, "nom", Value::String(format!("p{}", i / pas).into()));
            s.set_node_property(id, "age", Value::Int64((i % 97) as i64));
        }
        if let Some(p) = precedent {
            s.create_edge(id, p, "CONNAIT");
        }
        precedent = Some(id);
    }
}

fn fabriquer(dossier: &Path, noeuds: u64, fenetre: u64, pas: u64) {
    let t = Instant::now();
    let s = SubstrateStore::create_with_mode(dossier, SyncMode::Never).expect("création");
    ecrire(&s, 0, noeuds, pas);
    s.create_property_index("nom");
    s.flush().expect("flush");
    if fenetre == 0 {
        s.close().expect("close");
        drop(s);
    } else {
        ecrire(&s, noeuds, fenetre, pas);
    }
    eprintln!(
        "fabriqué : {noeuds} nœuds + fenêtre {fenetre} en {:.1} s",
        t.elapsed().as_secs_f64()
    );
    // Aucun destructeur : l'écrivain disparaît, sa fenêtre WAL reste.
    std::process::exit(0);
}

fn perimer(dossier: &Path) {
    let colonne = dossier.join("substrate.degrees.node.u32");
    let mut octets = std::fs::read(&colonne).expect("colonne de degrés");
    octets[16] ^= 0xFF;
    std::fs::write(&colonne, &octets).expect("écrire la colonne");
}

/// `(médiane, p99)` d'un échantillon.
fn mediane_p99(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(|a, b| a.partial_cmp(b).expect("pas de NaN"));
    let p = |q: f64| v[((v.len() as f64 - 1.0) * q).round() as usize];
    (p(0.5), p(0.99))
}

/// Défauts de page mineurs et majeurs du processus (`getrusage`).
// `ru_minflt` est un `i64` sous Linux et un `i32` sous macOS : le cast est
// nécessaire sur l'un, superflu sur l'autre.
#[allow(clippy::unnecessary_cast)]
fn defauts_de_page() -> (i64, i64) {
    // SAFETY: `rusage` est un POD rempli par l'appel ; aucun pointeur conservé.
    let mut u: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
    (u.ru_minflt as i64, u.ru_majflt as i64)
}

fn rss_ko() -> u64 {
    let sortie = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&sortie.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

struct Alea(u64);

impl Alea {
    fn suivant(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn mesurer(dossier: &Path, ouvertures: usize, etiquette: &str, nommes: Option<u64>) {
    // Ouvertures : chacune ouvre, puis ferme.
    let (min0, maj0) = defauts_de_page();
    let mut durees_ms = Vec::with_capacity(ouvertures);
    for _ in 0..ouvertures {
        let t = Instant::now();
        let s = SubstrateStore::open_read_only(dossier).expect("ouverture en lecture seule");
        durees_ms.push(t.elapsed().as_secs_f64() * 1e3);
        drop(s);
    }
    let (ouv_med, ouv_p99) = mediane_p99(durees_ms);
    let (min1, maj1) = defauts_de_page();
    let min_par_ouv = (min1 - min0) as f64 / ouvertures as f64;
    let maj_par_ouv = (maj1 - maj0) as f64 / ouvertures as f64;

    let s = SubstrateStore::open_read_only(dossier).expect("ouverture en lecture seule");
    let rss_ouvert_mo = rss_ko() as f64 / 1024.0;
    // Coût de la PREMIÈRE lecture de compteurs et de degrés : sur la branche qui
    // diffère les vérifications, c'est ici qu'elles se paient.
    let t = Instant::now();
    let n = s.node_count() as u64;
    let premier_compte_ms = t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    let degre_2 = s.out_degree(NodeId(2));
    let premier_degre_ms = t.elapsed().as_secs_f64() * 1e3;
    // Les arêtes de la fabrication pointent vers le PRÉCÉDENT : le nœud 1 n'a pas
    // d'arête sortante et une entrante, le nœud 2 une sortante.
    assert_eq!(s.out_degree(NodeId(1)), 0, "degré sortant du nœud 1");
    assert_eq!(s.in_degree(NodeId(1)), 1, "degré entrant du nœud 1");
    assert_eq!(degre_2, 1, "degré sortant du nœud 2");

    const LOT: usize = 1000;
    const LOTS: usize = 400;
    let mut alea = Alea(0x9E37_79B9_7F4A_7C15);

    // Lecture par identifiant : l'enregistrement du nœud, puis une propriété.
    let mut par_id = Vec::with_capacity(LOTS);
    let mut vus = 0u64;
    for _ in 0..LOTS {
        let ids: Vec<NodeId> = (0..LOT).map(|_| NodeId(1 + alea.suivant() % n)).collect();
        let t = Instant::now();
        for &id in &ids {
            if s.get_node(id).is_some() {
                vus += 1;
            }
        }
        par_id.push(t.elapsed().as_nanos() as f64 / LOT as f64);
    }
    let (id_med, id_p99) = mediane_p99(par_id);

    // Recherche indexée : égalité sur `nom`.
    let t = Instant::now();
    s.create_property_index("nom");
    let index_ms = t.elapsed().as_secs_f64() * 1e3;
    const LOT_IDX: usize = 200;
    let mut indexee = Vec::with_capacity(LOTS);
    let mut trouves = 0u64;
    for _ in 0..LOTS {
        let valeurs: Vec<Value> = (0..LOT_IDX)
            .map(|_| Value::String(format!("p{}", alea.suivant() % nommes.unwrap_or(n)).into()))
            .collect();
        let t = Instant::now();
        for v in &valeurs {
            trouves += s.find_nodes_by_property("nom", v).len() as u64;
        }
        indexee.push(t.elapsed().as_nanos() as f64 / LOT_IDX as f64);
    }
    let (idx_med, idx_p99) = mediane_p99(indexee);
    let rss_fin_mo = rss_ko() as f64 / 1024.0;

    assert_eq!(vus, (LOT * LOTS) as u64, "tous les nœuds tirés existent");
    assert_eq!(
        trouves,
        (LOT_IDX * LOTS) as u64,
        "chaque valeur cherchée désigne exactement un nœud"
    );
    println!(
        "{etiquette}\t{n}\t{ouv_med:.2}\t{ouv_p99:.2}\t{rss_ouvert_mo:.0}\t{rss_fin_mo:.0}\t\
         {id_med:.0}\t{id_p99:.0}\t{idx_med:.0}\t{idx_p99:.0}\t{index_ms:.1}\t{min_par_ouv:.0}\t{maj_par_ouv:.1}\t{premier_compte_ms:.2}\t{premier_degre_ms:.2}"
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let usage = "usage : banc_lecture_seule fabriquer <dossier> <noeuds> <fenetre> | \
                 mesurer <dossier> <ouvertures> [etiquette] | perimer <dossier>";
    match args.get(1).map(String::as_str) {
        Some("fabriquer") => fabriquer(
            Path::new(&args[2]),
            args[3].parse().expect("noeuds"),
            args[4].parse().expect("fenetre"),
            args.get(5).map_or(1, |p| p.parse().expect("pas")),
        ),
        Some("perimer") => perimer(Path::new(&args[2])),
        Some("mesurer") => mesurer(
            Path::new(&args[2]),
            args[3].parse().expect("ouvertures"),
            args.get(4).map_or("-", String::as_str),
            args.get(5).map(|m| m.parse().expect("nommes")),
        ),
        _ => {
            eprintln!("{usage}");
            std::process::exit(2);
        }
    }
}
