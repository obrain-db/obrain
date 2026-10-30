//! ── LE COÛT DE LA DÉDUPLICATION CDC, MESURÉ ────────────────────────────────
//!
//! Le 2026-09-17, la déduplication de `changes_since_*` a été corrigée : sa clé
//! `(entité, epoch, timestamp)` effondrait cinq écritures d'une même milliseconde
//! en une seule (défaut réel : `search_conflicts` ne voyait plus les
//! contradictions). La nouvelle clé compare l'événement ENTIER — mais la
//! comparaison se fait par un parcours LINÉAIRE des événements déjà vus sous la
//! même clé.
//!
//! Ce parcours est quadratique dans la taille du groupe. Or un groupe est
//! exactement ce qui se forme quand on importe une session : des milliers
//! d'événements, la MÊME entité (le nœud de session), la MÊME milliseconde (une
//! rafale d'écritures). Si le coût quadratique est réel, la correction a déplacé
//! un défaut de justesse vers un défaut de coût — et un pont qui brûle un cœur
//! pendant des minutes est exactement ce qu'on observe par ailleurs.
//!
//! Ce fichier MESURE ce coût, sans seuil : il imprime les durées pour 1k, 2k, 4k
//! et 8k événements de même clé. Un facteur ≈ 4 par doublement = quadratique.
#![cfg(feature = "cdc")]

use std::collections::HashMap;
use web_time::Instant;

use obrain_common::types::{EpochId, NodeId, Value};
use obrain_engine::ObrainDB;

/// N événements sur la MÊME entité, dans la MÊME milliseconde (une rafale).
fn mesurer(n: usize) -> std::time::Duration {
    let db = ObrainDB::new_in_memory();
    let log = db.cdc_log();
    let ent = NodeId(7);
    // Une rafale : toutes les écritures de la même milliseconde, comme un import.
    let debut = Instant::now();
    for i in 0..n {
        let mut props = HashMap::new();
        props.insert("i".to_string(), Value::Int64(i as i64));
        log.record(obrain_engine::cdc::ChangeEvent {
            entity_id: obrain_engine::cdc::EntityId::Node(ent),
            kind: obrain_engine::cdc::ChangeKind::Update,
            epoch: EpochId(1),
            timestamp: 1_789_000_000_000,
            before: Some(props.clone()),
            after: Some(props),
        });
    }
    let ecriture = debut.elapsed();
    let t0 = Instant::now();
    let evenements: Vec<obrain_engine::cdc::ChangeEvent> =
        log.changes_since_timestamp(0, |_| true);
    let lecture = t0.elapsed();
    eprintln!(
        "CDC n={n:>5} : écriture={ecriture:>9.1?} lecture={lecture:>9.1?} \
         evenements={:?}",
        evenements.len()
    );
    lecture
}

/// ── LE CHEMIN FROID : CELUI QUI PORTE LA DÉDUPLICATION ──────────────────────
///
/// Le chemin chaud (tous les événements en RAM) rend la liste sans dédupliquer —
/// mesuré linéaire, donc la déduplication n'y est PAS en cause. Mais un pont est
/// un processus NEUF qui lit un store FROID : c'est `changes_since_cold` qui
/// tourne alors, et c'est lui qui porte le parcours linéaire du groupe de clé.
/// C'est le scénario de l'emballement (2 Go de RSS, un cœur, aucune E/S) qu'on
/// veut trancher ici.
fn mesurer_froid(n: usize) -> std::time::Duration {
    use obrain_common::types::{EpochId, NodeId, Value};

    let td = tempfile::tempdir().expect("tempdir");
    let chemin = td.path().join("cold");
    let ent = NodeId(7);
    {
        let db = ObrainDB::open(&chemin).expect("open");
        let log = db.cdc_log();
        for i in 0..n {
            let mut props = HashMap::new();
            props.insert("i".to_string(), Value::Int64(i as i64));
            log.record(obrain_engine::cdc::ChangeEvent {
                entity_id: obrain_engine::cdc::EntityId::Node(ent),
                kind: obrain_engine::cdc::ChangeKind::Update,
                epoch: EpochId(1),
                timestamp: 1_789_000_000_000,
                before: Some(props.clone()),
                after: Some(props),
            });
        }
        db.close().expect("close");
    }
    // Processus neuf : rien en RAM, tout vient de la zone.
    let db = ObrainDB::open(&chemin).expect("reopen");
    let t0 = Instant::now();
    let evenements = db.cdc_log().changes_since_timestamp(0, |_| true);
    let duree = t0.elapsed();
    eprintln!("CDC FROID n={n:>5} : lecture={duree:>10.1?} evenements={}", evenements.len());
    // ── LE COMPTE PROUVE LES DEUX CONTRATS À LA FOIS ────────────────────────
    // Chaque événement enregistré diffère des autres (champ `i`) ET n'existe
    // qu'une fois dans la zone : le compte doit donc valoir exactement `n`.
    // Plus grand = la déduplication n'a plus dédupliqué (doublons rendus) ;
    // plus petit = elle a effacé des événements distincts (le défaut d'origine,
    // celui qu'on a corrigé en passant de la clé grossière à l'événement entier).
    assert_eq!(
        evenements.len(),
        n,
        "le chemin froid doit rendre chaque événement UNE fois, sans en perdre"
    );
    duree
}

/// ── GARDE DE COMPLEXITÉ (et non de temps mural) ─────────────────────────────
///
/// Le seuil est choisi pour SÉPARER LES COMPLEXITÉS, pas pour mesurer la machine :
/// à 16 000 événements de même clé, la version quadratique prenait ≈ 3,6 s
/// (extrapolé des ×3,9 mesurés) et la version linéaire ≈ 70 ms. Un plafond d'une
/// seconde est donc 14× au-dessus du linéaire (large, stable même sur une machine
/// chargée) et 3,6× sous le quadratique (il tombe s'il revient). C'est la seule
/// assertion de ce fichier, et elle ne dépend pas finement de la charge.
#[test]
fn le_cout_du_chemin_froid_est_mesure_et_borne() {
    let mut precedent: Option<(usize, std::time::Duration)> = None;
    let mut dernier = std::time::Duration::ZERO;
    for n in [1_000usize, 2_000, 4_000, 16_000] {
        let duree = mesurer_froid(n);
        if let Some((n_prec, duree_prec)) = precedent {
            eprintln!(
                "  FROID n x{:.0} → temps x{:.1} (quadratique ≈ 4, linéaire ≈ 2)",
                n as f64 / n_prec as f64,
                duree.as_secs_f64() / duree_prec.as_secs_f64().max(1e-9)
            );
        }
        precedent = Some((n, duree));
        dernier = duree;
    }
    assert!(
        dernier < std::time::Duration::from_secs(1),
        "le chemin froid de la déduplication CDC redevient probablement QUADRATIQUE : \
         16 000 événements de même clé en {dernier:?} (linéaire attendu ≈ 70 ms, \
         quadratique ≈ 3,6 s). La cause mesurée le 2026-09-17 était un parcours \
         linéaire du groupe — corrigé par un index de hachage (voir `DedupCdc`)."
    );
}

#[test]
fn le_cout_de_la_deduplication_cdc_est_mesure() {
    let mut precedent: Option<(usize, std::time::Duration)> = None;
    for n in [1_000usize, 2_000, 4_000] {
        let duree = mesurer(n);
        if let Some((n_prec, duree_prec)) = precedent {
            let facteur_n = n as f64 / n_prec as f64;
            let facteur_t = duree.as_secs_f64() / duree_prec.as_secs_f64().max(1e-9);
            eprintln!(
                "  n x{facteur_n:.0} → temps x{facteur_t:.1} (quadratique ≈ 4, linéaire ≈ 2)"
            );
        }
        precedent = Some((n, duree));
    }
    let _ = HashMap::<String, Value>::new();
}
