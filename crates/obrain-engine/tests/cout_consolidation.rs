//! SONDE — le coût d'une CONSOLIDATION sur un store réel, mesuré en processus.
//!
//! ── POURQUOI CETTE SONDE, ET CE QU'ELLE A ÉTABLI ────────────────────────────
//! Un `obrain bridge` a été observé quatre fois tournant à 85-90 % d'un cœur
//! pendant 3 à 24 minutes, avec 2,0-2,6 Go de RSS, en écrivant des centaines de
//! mégaoctets — sans qu'aucune ligne de journal ne le signale. L'enquête a
//! identifié le chemin : le harness écrit toutes les **500 ms**
//! (`GraphWriter.flushIntervalMs`), chaque `import_events` déclenche
//! `auto_consolider` → `tisser` → trois lectures COMPLÈTES de la fenêtre CDC
//! (`search_surprise`, `search_conflicts`, `search_relief`).
//!
//! Chacune de ces trois lectures passait par la déduplication de
//! `changes_since_timestamp`, qui comparait l'événement à tous ses prédécesseurs
//! du même groupe de clé — un groupe valant jusqu'à la fenêtre entière
//! (`CDC_RELOAD_CAP` = 10 000 par défaut, 100 000 au maximum). Coût QUADRATIQUE.
//!
//! Cette sonde mesure, EN PROCESSUS (une seule ouverture de store, donc sans
//! confondre le coût d'ouverture des 14 Go avec le coût de la consolidation) :
//!   * le temps d'ouverture ;
//!   * le nombre d'événements de la fenêtre et la taille du plus gros groupe
//!     de clé (c'est lui qui commandait le coût quadratique) ;
//!   * le temps des trois lectures.
//!
//! ── MODE D'EMPLOI ───────────────────────────────────────────────────────────
//! ```bash
//! # sur une COPIE du store (jamais le store vivant : il est tenu par un pont)
//! OBRAIN_SONDE_STORE=/tmp/copie/runtime-dev.obrain \
//!   cargo test -p obrain-engine --features cdc --test cout_consolidation -- \
//!   --ignored --nocapture
//! ```
#![cfg(feature = "cdc")]

use web_time::Instant;

use obrain_engine::ObrainDB;

#[test]
#[ignore = "sonde — exige OBRAIN_SONDE_STORE (chemin absolu d'un store ; la sonde en consolide une copie)"]
fn cout_d_une_consolidation_sur_store_reel() {
    let Some(chemin) = std::env::var_os("OBRAIN_SONDE_STORE").filter(|v| !v.is_empty()) else {
        eprintln!(
            "SAUT [cout_d_une_consolidation_sur_store_reel] : OBRAIN_SONDE_STORE n'est pas définie — \
             la sonde ne mesure RIEN (fabriquer une base : `cargo run -p obrain-engine --features cdc \
             --example fabrique_base_cdc -- <dir> <N>`)"
        );
        return;
    };

    // La sonde CONSOLIDE : elle travaille sur une copie privée, jamais sur le
    // chemin reçu (qui pourrait être l'original, quoi qu'en dise la consigne).
    let copie = obrain_bases_banc::copie_de(std::path::Path::new(&chemin), "OBRAIN_SONDE_STORE");

    let t0 = Instant::now();
    let db = ObrainDB::open(copie.chemin()).expect("ouverture de la copie du store");
    let ouverture = t0.elapsed();
    eprintln!("SONDE ouverture : {ouverture:?}");

    // La fenêtre CDC telle que la voit le moteur (même chemin que la
    // consolidation : `changes_since_timestamp`, sans filtre).
    let t1 = Instant::now();
    let evenements = db.cdc_log().changes_since_timestamp(0, |_| true);
    let lecture_fenetre = t1.elapsed();
    // Une fenêtre vide ne mesure rien : la sonde le dit en échouant, au lieu de
    // sortir verte sur des zéros.
    assert!(
        !evenements.is_empty(),
        "fenêtre CDC VIDE dans la copie de {} : la sonde n'a rien à mesurer (base sans CDC, ou \
         compactée)",
        std::path::Path::new(&chemin).display()
    );
    let mut par_cle: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for e in &evenements {
        let cle = format!("{:?}|{:?}|{}", e.entity_id, e.epoch, e.timestamp);
        *par_cle.entry(cle).or_insert(0) += 1;
    }
    let plus_gros = par_cle.values().copied().max().unwrap_or(0);
    eprintln!(
        "SONDE fenêtre CDC : {} événements, {} groupes de clé, plus gros groupe = {plus_gros}",
        evenements.len(),
        par_cle.len()
    );
    eprintln!("SONDE lecture de la fenêtre : {lecture_fenetre:?}");

    // Les trois lectures de la consolidation, comme `tisser` les appelle.
    let t2 = Instant::now();
    let surpris = db.search_surprise(0, 5, false);
    let d_surprise = t2.elapsed();
    let t3 = Instant::now();
    let conflits = db.search_conflicts(0, 5);
    let d_conflits = t3.elapsed();
    let t4 = Instant::now();
    let relief = db.search_relief(0, 5);
    let d_relief = t4.elapsed();

    eprintln!(
        "SONDE consolidation : surprise={d_surprise:?} ({} lignes), \
         conflicts={d_conflits:?} ({} lignes), relief={d_relief:?} ({} lignes)",
        surpris.as_ref().map_or(0, Vec::len),
        conflits.as_ref().map_or(0, Vec::len),
        relief.as_ref().map_or(0, Vec::len),
    );
    let total = d_surprise + d_conflits + d_relief;
    eprintln!(
        "SONDE TOTAL consolidation : {total:?} — le harness écrit toutes les 500 ms : \
         au-delà de 500 ms, le pont ne rattrape plus son retard"
    );
}
