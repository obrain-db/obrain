//! Invariants multi-writer — filet du chantier « multi-writer haute performance
//! sans perte des lectures » (RFC docs/rfc/substrate/multi-writer.md).
//!
//! Chaque test gèle UN invariant sensible du modèle d'écriture ACTUEL, pour que
//! les phases A/B (ParallelExecutor branché, group commit, WAL multi-têtes,
//! writer central) ne les cassent pas silencieusement :
//!
//! | Test | Invariant (RFC §4) |
//! |---|---|---|
//! | `shared_write_puis_lecture_ro_voit_les_commits` | I3 — shared-write + RO coexistent, le RO voit les commits |
//! | `reouverture_ro_idempotente_apres_rejeu` | I6 — rejeu à l'ouverture : idempotent, zéro perte, pas de doublon |
//! | `second_writer_exclusif_refuse_pendant_shared_write` | I4 — un 2ᵉ writer exclusif est refusé pendant un writer partagé |
//! | `conflit_write_write_sur_store_substrate` | I7 — 1ᵉʳ writer gagne, le 2ᵉ est rejouable, valeur persistée |
//! | `threads_independants_substrate_zero_perte` | I5 — sérialisation WAL sous threads : count exact après réouverture |
//! | `lecture_ro_pendant_writer_thread_sans_blocage` | I10 — lecture RO pendant écriture active : pas de blocage, résultats cohérents |
//! | `parallel_executor_batch_reel_substrate` | I9 — ParallelExecutor peut driver de vraies écritures substrate sans perte |

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use obrain_engine::ObrainDB;

/// Crée un store substrate neuf au chemin donné (open-or-create idempotent).
fn creer_store(chemin: &std::path::Path) -> ObrainDB {
    ObrainDB::open_substrate(chemin).expect("open_substrate (création)")
}

// ============================================================================
// I3 — shared-write + lecture RO coexistent ; le RO voit les commits
// ============================================================================

#[test]
fn shared_write_puis_lecture_ro_voit_les_commits() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("mw-shared-ro");

    // 1. Writer shared-write (le modèle GraphWriter de prod) sur store existant.
    {
        let _create = creer_store(&chemin); // open-or-create
    }
    let writer = ObrainDB::open_shared_write(&chemin).expect("open_shared_write");
    let s = writer.session();
    for i in 0..25 {
        s.execute(&format!("CREATE (:Mw {{id: {i}, tag: 'shared'}})"))
            .expect("create partagé");
    }
    writer.checkpoint().expect("checkpoint writer");
    assert_eq!(writer.node_count(), 25, "writer voit ses 25 nœuds");

    // 2. Lecteur RO sur le MÊME store, writer toujours vivant (même process).
    //    Le registre des verrous partage la part (shared-write = LOCK_SH).
    let lecteur = ObrainDB::open_read_only(&chemin).expect("open_read_only");
    assert_eq!(
        lecteur.node_count(),
        25,
        "le RO voit les 25 nœuds committés par le writer shared-write"
    );

    // 3. Le writer continue ; le RO (déjà ouvert) ne bloque pas et ne corrompt pas.
    s.execute("CREATE (:Mw {id: 99, tag: 'apres'})")
        .expect("écriture après ouverture RO");
    writer.checkpoint().expect("checkpoint final");
    assert_eq!(writer.node_count(), 26);

    // 4. Le RO rouvre (rejeu WAL) → voit l'état complet, sans doublon.
    drop(lecteur);
    let lecteur2 = ObrainDB::open_read_only(&chemin).expect("reopen RO");
    assert_eq!(
        lecteur2.node_count(),
        26,
        "le RO rouvré voit l'état complet après rejeu"
    );
}

// ============================================================================
// I6 — rejeu à l'ouverture : idempotent, zéro perte, pas de doublon
// ============================================================================

#[test]
fn reouverture_ro_idempotente_apres_rejeu() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("mw-rejeu");
    {
        let db = creer_store(&chemin);
        let s = db.session();
        for i in 0..40 {
            s.execute(&format!("CREATE (:Rejeu {{id: {i}}})"))
                .expect("create");
        }
        // Pas de checkpoint volontaire : le rejeu se fait depuis le WAL.
        db.flush_wal().expect("flush wal");
    } // drop writer — comme un arrêt process propre

    let r1 = ObrainDB::open_read_only(&chemin).expect("RO #1");
    assert_eq!(r1.node_count(), 40, "rejeu #1 : zéro perte");
    drop(r1);

    let r2 = ObrainDB::open_read_only(&chemin).expect("RO #2");
    assert_eq!(r2.node_count(), 40, "rejeu #2 : idempotent, pas de doublon");
}

// ============================================================================
// I4 — un 2ᵉ writer exclusif est refusé pendant un writer partagé
// ============================================================================

#[test]
fn second_writer_exclusif_refuse_pendant_shared_write() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("mw-second-writer");
    {
        let _create = creer_store(&chemin);
    }

    let writer = ObrainDB::open_shared_write(&chemin).expect("writer shared-write #1");
    let s = writer.session();
    s.execute("CREATE (:A {id: 1})").expect("create");

    // Un 2ᵉ writer EXCLUSIF sur le même store doit être refusé (registre des
    // verrous : une part partagée vivante refuse une demande exclusive).
    let second = ObrainDB::open(&chemin);
    assert!(
        second.is_err(),
        "un 2ᵉ writer exclusif pendant un shared-write doit être refusé"
    );

    // Le writer partagé continue de fonctionner (invariant de sûreté).
    s.execute("CREATE (:A {id: 2})")
        .expect("create après refus");
    writer.checkpoint().expect("checkpoint");
    assert_eq!(writer.node_count(), 2);
}

// ============================================================================
// I7 — write-write conflict sur STORE SUBSTRATE persistant (pas in-memory)
// ============================================================================

#[test]
fn conflit_write_write_sur_store_substrate() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("mw-conflict");
    let db = creer_store(&chemin);

    db.session()
        .execute("CREATE (:Account {name: 'shared', balance: 100})")
        .expect("seed");

    // Session 1 : lit puis écrit le nœud partagé.
    let mut s1 = db.session();
    s1.begin_transaction().expect("begin s1");
    s1.execute("MATCH (a:Account {name: 'shared'}) SET a.balance = 200")
        .expect("set s1");

    // Session 2 : écrit le MÊME nœud → conflit write-write (first-writer-wins).
    let mut s2 = db.session();
    s2.begin_transaction().expect("begin s2");
    let set2 = s2.execute("MATCH (a:Account {name: 'shared'}) SET a.balance = 300");
    assert!(
        set2.is_err(),
        "le 2ᵉ SET doit échouer en write-write conflict : {set2:?}"
    );

    // Le 1ᵉʳ writer commit ; le 2ᵉ rollback.
    s1.commit().expect("commit s1");
    let _ = s2.rollback();

    // Valeur finale persistée = celle du 1ᵉʳ writer, visible après réouverture.
    let res = db
        .session()
        .execute("MATCH (a:Account {name: 'shared'}) RETURN a.balance AS b")
        .expect("lecture");
    assert_eq!(res.rows.len(), 1);
    assert_eq!(res.rows[0][0], obrain_common::types::Value::Int64(200));

    drop(db);
    let db2 = creer_store(&chemin);
    let res2 = db2
        .session()
        .execute("MATCH (a:Account {name: 'shared'}) RETURN a.balance AS b")
        .expect("lecture après réouverture");
    assert_eq!(
        res2.rows[0][0],
        obrain_common::types::Value::Int64(200),
        "la valeur du 1ᵉʳ writer persiste après réouverture"
    );
}

// ============================================================================
// I11 — scope par graphe : PAS de faux conflit inter-graphes (RFC B1)
// ============================================================================
//
// Deux SubstrateStore distincts allouent chacun leurs NodeId à partir de 1.
// SANS scope, le write_set GLOBAL du TransactionManager croit que NodeId(1)
// du graphe g1 et NodeId(1) du graphe g2 sont la MÊME entité → faux conflit.
// Mesuré AVANT fix : 2999 faux conflits / 4000 CREATE sur 4 graphes distincts
// (75 % d'échec). Ce test synchronise DEUX transactions ACTIVES sur DEUX
// graphes différents, chacune écrivant ses premiers NodeId → aucune ne doit
// échouer (les données sont disjointes). Le scope = `graph_scope()` du store.
#[test]
fn deux_graphes_disjoints_sans_faux_conflit() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("mw-scope");
    let db = creer_store(&chemin);

    let s = db.session();
    s.execute("CREATE GRAPH g1").expect("graphe g1");
    s.execute("CREATE GRAPH g2").expect("graphe g2");
    drop(s);

    // Deux sessions ACTIVES simultanément, une par graphe. Chacune écrit SES
    // premiers NodeId (1, 2, …) — les deux stores allouent les MÊMES valeurs
    // numériques, mais dans des espaces distincts.
    let mut s1 = db.session();
    s1.use_graph("g1");
    s1.begin_transaction().expect("begin g1");
    s1.execute("CREATE (:S)").expect("g1 nœud 1 actif");
    s1.execute("CREATE (:S)").expect("g1 nœud 2 actif");

    let mut s2 = db.session();
    s2.use_graph("g2");
    s2.begin_transaction().expect("begin g2");
    // g2 écrit AUSSI ses NodeId 1 et 2 — même valeurs numériques que g1.
    let r1 = s2.execute("CREATE (:S)");
    let r2 = s2.execute("CREATE (:S)");

    assert!(
        r1.is_ok() && r2.is_ok(),
        "faux conflit inter-graphes : g2 doit écrire ses NodeId sans être \
         bloqué par g1 (données disjointes) — r1={r1:?} r2={r2:?}"
    );

    s1.commit().expect("commit g1");
    s2.commit().expect("commit g2");

    // Les deux graphes ont bien chacun 2 nœuds.
    drop(s1);
    drop(s2);
    let session = db.session();
    session.use_graph("g1");
    let v1 = session
        .execute("MATCH (n:S) RETURN count(n) AS c")
        .expect("count g1");
    session.use_graph("g2");
    let v2 = session
        .execute("MATCH (n:S) RETURN count(n) AS c")
        .expect("count g2");
    assert_eq!(v1.rows[0][0], obrain_common::types::Value::Int64(2));
    assert_eq!(v2.rows[0][0], obrain_common::types::Value::Int64(2));
}

// ============================================================================
// I5 — sérialisation WAL sous threads concurrents : zéro perte
// ============================================================================

#[test]
fn threads_independants_substrate_zero_perte() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("mw-threads");
    let db = Arc::new(creer_store(&chemin));

    // Écritures préalables (base) — le rejeu doit les conserver.
    db.session()
        .execute("CREATE (:Base {id: -1})")
        .expect("seed base");
    assert_eq!(db.node_count(), 1);

    let nb_threads = 4;
    let par_thread = 25;
    let mut handles = Vec::new();
    for t in 0..nb_threads {
        let db = Arc::clone(&db);
        handles.push(thread::spawn(move || {
            let s = db.session();
            for i in 0..par_thread {
                let etiquette = format!("Th{t}");
                s.execute(&format!("CREATE (:{etiquette} {{id: {i}}})"))
                    .unwrap_or_else(|e| panic!("thread {t} insert {i} : {e}"));
            }
        }));
    }
    for h in handles {
        h.join().expect("thread joint");
    }

    let attendu = 1 + nb_threads * par_thread;
    assert_eq!(
        db.node_count(),
        attendu,
        "toutes les écritures concurrentes sont présentes en mémoire"
    );

    // Réouverture → count exact (le WAL a tout sérialisé, zéro perte).
    drop(db);
    let db2 = creer_store(&chemin);
    assert_eq!(
        db2.node_count(),
        attendu,
        "zéro perte après réouverture : le WAL a sérialisé les threads"
    );
}

// ============================================================================
// I10 — lecture RO pendant écriture active : pas de blocage, cohérence
// ============================================================================

#[test]
fn lecture_ro_pendant_writer_thread_sans_blocage() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("mw-ro-pendant-write");
    {
        let _create = creer_store(&chemin);
    }
    let writer = Arc::new(ObrainDB::open_shared_write(&chemin).expect("writer"));

    // Écriture de fond en continu (writer thread), comme le bridge de prod.
    let w2 = Arc::clone(&writer);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop2 = Arc::clone(&stop);
    let h = thread::spawn(move || {
        let s = w2.session();
        let mut i = 0u64;
        while !stop2.load(std::sync::atomic::Ordering::Relaxed) {
            s.execute(&format!("CREATE (:Live {{n: {i}}})"))
                .expect("écriture live");
            i += 1;
            if i % 10 == 0 {
                w2.checkpoint().expect("checkpoint live");
            }
        }
    });

    // Pendant ce temps : un lecteur RO lit en boucle, sans jamais se bloquer
    // ni lire un état corrompu (count monotone non décroissant).
    let lecteur = ObrainDB::open_read_only(&chemin).expect("open RO");
    let mut dernier = 0usize;
    for _ in 0..20 {
        let c = lecteur.node_count();
        assert!(
            c >= dernier,
            "le count RO ne décroît jamais (lu {c} après {dernier})"
        );
        dernier = c;
        thread::sleep(Duration::from_millis(5));
    }
    assert!(dernier > 0, "le RO a vu au moins une écriture live");

    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    h.join().expect("writer joint");
    drop(lecteur);
    writer.checkpoint().expect("checkpoint final");
    // Le writer partagé doit être fermé avant une réouverture EXCLUSIVE
    // (invariant I4 : un writer vivant bloque l'ouverture exclusive).
    drop(writer);

    // Réouverture propre : count exact, cohérent.
    let db2 = creer_store(&chemin);
    assert!(db2.node_count() > 0, "store cohérent après writer + RO");
}

// ============================================================================
// I9 — ParallelExecutor peut driver de vraies écritures substrate sans perte
// ============================================================================

#[test]
fn parallel_executor_batch_reel_substrate() {
    use obrain_engine::transaction::EntityId;
    use obrain_engine::transaction::parallel::{BatchRequest, ParallelExecutor};

    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("mw-parallel");
    let db = Arc::new(creer_store(&chemin));

    // L'API crud d'ObrainDB est le chemin d'écriture réel du substrate ; on
    // l'utilise comme « vrai store » piloté par l'exécuteur. Chaque opération
    // du batch crée un nœud DISTINCT → aucun conflit attendu, exécution
    // réellement parallèle, commit ordonné, zéro perte.
    let ops: Vec<String> = (0..12).map(|i| format!("create node {i}")).collect();
    let batch = BatchRequest::new(ops);

    let executor = ParallelExecutor::new(4);
    let db2 = Arc::clone(&db);
    let result = executor.execute_batch(batch, |idx, _op, execution| {
        let id = db2.create_node(&["Parallèle"]);
        execution.record_write(EntityId::Node(id));
        let _ = idx;
    });

    assert!(
        result.all_succeeded(),
        "batch réel : toutes les ops réussissent"
    );
    assert_eq!(result.failure_count, 0);
    assert_eq!(
        db.node_count(),
        12,
        "les 12 créations réelles pilotées par l'exécuteur sont toutes là"
    );

    // Réouverture : le substrate a tout persisté (les écritures passent par le
    // même Writer/WAL que la voie séquentielle).
    drop(db);
    let db2 = ObrainDB::open_substrate(&chemin).expect("réouverture");
    assert_eq!(
        db2.node_count(),
        12,
        "zéro perte après batch parallèle réel"
    );
}

// ============================================================================
/// La porte de performance est-elle ARMÉE ?
///
/// Même contrat que `obrain-cognitive/tests` : déarmée (défaut), le test MESURE
/// et le DIT, sans faire tomber la porte ; armée (`OBRAIN_PERF_GATE=1`), il
/// exige le repère. Un repère de temps mural est une propriété de la MACHINE
/// autant que du code : l'exiger par défaut, c'est livrer un test qui échoue
/// selon la charge.
#[must_use]
pub fn porte_armee() -> bool {
    std::env::var("OBRAIN_PERF_GATE").is_ok_and(|v| v.trim() == "1")
}

// I12 — import bulk zone-direct non régressé (repère ≈ 0,26 s historique)
// ============================================================================
//
// Le multi-writer (scope B1, refactors B1-2) ne doit PAS ralentir l'import
// bulk (native_import → `bulk_import` → `bulk_import_labels_edges`, zone
// directe SANS WAL par op). Volume de référence : 400k nœuds, 1,6M arêtes,
// mesuré 0,220 s le 2026-09-06 (import 0,181 s + checkpoint 0,039 s).
// Le test borne le temps total (garde souple ×3 pour machines lentes/CI) et
// vérifie la persistance par réouverture.
#[test]
fn import_bulk_400k_1600k_non_regresse() {
    use web_time::Instant;

    let n = 400_000usize;
    let m = 1_600_000usize;
    let alphabet = ["A", "B", "C", "D", "E", "F", "G", "H"];

    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("bulk-400k");
    let db = creer_store(&chemin);

    // Labels : petit alphabet réparti (≤64, limite bitset substrate).
    let labels: Vec<String> = (0..n)
        .map(|i| alphabet[i % alphabet.len()].to_string())
        .collect();
    let mut src = Vec::with_capacity(m);
    let mut dst = Vec::with_capacity(m);
    let mut etypes = Vec::with_capacity(m);
    for i in 0..m {
        src.push(((i * 7) % n) as u32);
        dst.push(((i * 13 + 1) % n) as u32);
        etypes.push("E".to_string());
    }

    let t0 = Instant::now();
    let (nn, mm) = db
        .bulk_import(&labels, &src, &dst, &etypes)
        .expect("bulk zone-direct");
    db.checkpoint().expect("checkpoint");
    let total = t0.elapsed();
    eprintln!(
        "bulk 400k/1,6M : {:.3} s (repère ≈ 0,26 s)",
        total.as_secs_f64()
    );
    assert_eq!(
        (nn as usize, mm as usize),
        (n as usize, m as usize),
        "retour bulk exact"
    );
    // ── LE REPÈRE DE TEMPS EST OPT-IN (doctrine du 2026-09-17) ──────────────
    // Une garde de temps mural dans la porte PAR DÉFAUT est un test qui dépend
    // de l'environnement : mesuré sur cette machine (charge 7,6 pour 16 cœurs,
    // des processus d'autres sessions encore vivants) le MÊME import prend
    // 2,2 s au lieu de 0,26 s, sans qu'une ligne de code ait changé. Un test
    // qui échoue selon la charge n'apprend rien sur le produit — il apprend
    // sur la charge.
    //
    // Mesure toujours faite et TOUJOURS imprimée (ci-dessus) ; l'assertion ne
    // tombe que si la porte est armée (`OBRAIN_PERF_GATE=1`), comme les portes
    // de `obrain-cognitive` (`feedback_baseline`, `synapse_reinforce_micro_bench`).
    if porte_armee() {
        assert!(
            total.as_secs_f64() < 0.80,
            "import bulk 400k/1,6M trop lent : {:.3} s (repère ≈ 0,26 s)",
            total.as_secs_f64()
        );
    } else {
        eprintln!(
            "  ⚠ repère TEMPS non armé (OBRAIN_PERF_GATE=1 pour l'exiger) — mesuré {:.3} s",
            total.as_secs_f64()
        );
    }
    drop(db);

    // Persistance vérifiée par réouverture (comptage O(1) substrate).
    let db2 = creer_store(&chemin);
    assert_eq!(db2.node_count(), n, "tous les nœuds persistés");
    assert!(
        db2.edge_count() >= m - m / 100,
        "arêtes persistées (tolérance doublons du corpus synthétique)"
    );
}
