//! Banc du SURCOÛT des points de contrôle d'annulation.
//!
//! N'utilise QUE l'API publique commune aux deux arbres (`open_substrate` +
//! `Session::execute`) : le MÊME fichier se compile avant et après le
//! correctif, ce qui est la condition pour que la comparaison porte sur le
//! code et non sur le banc.
//!
//! Base partagée via `OBRAIN_BANC_BASE` pour que les deux mesures lisent
//! octet pour octet les mêmes données.
//!
//!   cargo test --release --test banc_surcout -- --nocapture --ignored
//!
//! ── CE QUE CE BANC PEUT ET NE PEUT PAS CONCLURE (2026-08-24) ─────────
//!
//! Il ne peut PAS conclure « le surcout est de zero ». Deux passes du MEME
//! binaire, sans aucun changement de code, ont donne 125,875 ms puis
//! 116,724 ms sur la requete scan+props+tri : neuf millisecondes d ecart
//! pour un code identique. Cet ecart depasse tout ce que ce banc a mesure
//! entre avant et apres le correctif.
//!
//! La seule conclusion honnete est donc : le surcout des points de controle
//! est SOUS LE PLANCHER DE BRUIT de cette machine. Annoncer une egalite
//! chiffree (« 128,8 avant, 128,8 apres ») pretendrait a une precision que
//! le dispositif n a pas. Pour trancher plus finement il faudrait une
//! machine au repos, l isolation de coeurs, et un nombre de repetitions
//! calibre sur la variance observee — aucun des trois n est reuni ici.

use std::time::Duration;
use web_time::Instant;

use obrain_engine::ObrainDB;

const REQUETES: &[(&str, &str)] = &[
    ("scan+props+tri", "MATCH (n:Grain) RETURN n.v ORDER BY n.v"),
    (
        "scan+filtre+agregat",
        "MATCH (n:Grain) WHERE n.v > 60000 RETURN count(n)",
    ),
    ("scan nu", "MATCH (n:Grain) RETURN n.v"),
];

const REPETITIONS: usize = 15;
const CHAUFFE: usize = 3;

#[test]
#[ignore = "banc — lance a la main"]
fn banc_surcout() {
    let base = std::env::var("OBRAIN_BANC_BASE").expect("OBRAIN_BANC_BASE non definie");
    // Le banc ne fait que lire, mais toute ouverture ecrit sur le disque : il
    // ouvre une copie privee, jamais le chemin recu.
    let copie = obrain_bases_banc::copie_de(std::path::Path::new(&base), "OBRAIN_BANC_BASE");
    let db = ObrainDB::open_substrate(copie.chemin()).expect("ouverture de la copie");

    for (nom, q) in REQUETES {
        let mut mesures: Vec<Duration> = Vec::with_capacity(REPETITIONS);
        for i in 0..REPETITIONS {
            let t = Instant::now();
            let r = db.session().execute(q).expect("execution");
            let d = t.elapsed();
            std::hint::black_box(r.rows.len());
            if i >= CHAUFFE {
                mesures.push(d);
            }
        }
        mesures.sort();
        let mediane = mesures[mesures.len() / 2];
        let mini = mesures[0];
        println!(
            "BANC {nom:<22} mediane {:>9.3} ms   min {:>9.3} ms   n={}",
            mediane.as_secs_f64() * 1000.0,
            mini.as_secs_f64() * 1000.0,
            mesures.len()
        );
    }
}
