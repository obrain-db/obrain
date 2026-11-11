//! `duration.between` et ses variantes, et l'accès à un composant du résultat
//! (`duration.between(a, b).days`).
//!
//! Chaque forme est jouée en Cypher et en GQL ; les deux requêtes réelles de
//! l'application cliente (Project Orchestrator : `update_staleness_scores` et
//! la fédération MCP) sont rejouées TELLES QUELLES, avec des valeurs attendues
//! calculées à la main.
//!
//! Sémantique de Neo4j, reprise telle quelle : `between` rend des mois
//! ENTIERS, puis des jours, puis le reste ; chaque composant est le total de
//! son groupe dans son unité. En particulier `.days` ne compte PAS les mois :
//! de février à mai, `.days` vaut le reste en jours, pas l'écart total.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test duration_between
//! ```

use obrain_common::types::{Date, Time, Timestamp, Value};
use obrain_engine::ObrainDB;

#[derive(Clone, Copy, Debug)]
enum Langage {
    Cypher,
    Gql,
}

fn executer(db: &ObrainDB, langage: Langage, requete: &str) -> Vec<Vec<Value>> {
    let session = db.session();
    let resultat = match langage {
        Langage::Cypher => session.execute_cypher(requete),
        Langage::Gql => session.execute(requete),
    };
    match resultat {
        Ok(r) => r.rows,
        Err(e) => panic!("[{langage:?}] {requete}\n  -> {e}"),
    }
}

/// Une seule valeur, en une ligne.
fn valeur(db: &ObrainDB, langage: Langage, requete: &str) -> Value {
    let lignes = executer(db, langage, requete);
    assert_eq!(lignes.len(), 1, "[{langage:?}] {requete}");
    lignes[0][0].clone()
}

fn entier(db: &ObrainDB, langage: Langage, requete: &str) -> i64 {
    match valeur(db, langage, requete) {
        Value::Int64(i) => i,
        autre => panic!("[{langage:?}] {requete} : entier attendu, reçu {autre:?}"),
    }
}

fn horodatage(date: (i32, u32, u32), heure: (u32, u32, u32), nano: u32) -> Timestamp {
    Timestamp::from_date_time(
        Date::from_ymd(date.0, date.1, date.2).unwrap(),
        Time::from_hms_nano(heure.0, heure.1, heure.2, nano).unwrap(),
    )
}

fn vide() -> ObrainDB {
    ObrainDB::new_in_memory()
}

// ============================================================================
// between : mois entiers, puis jours, puis reste
// ============================================================================

/// 90 minutes : `PT1H30M`.
fn entre_deux_datetimes_vaut_une_duree_litterale(langage: Langage) {
    let db = vide();
    assert_eq!(
        valeur(
            &db,
            langage,
            "RETURN duration.between(datetime('2024-01-01T00:00:00Z'), \
             datetime('2024-01-01T01:30:00Z')) = duration('PT1H30M')",
        ),
        Value::Bool(true),
        "{langage:?}"
    );
}

/// Du 10 janvier 00:00 au 24 avril 06:00 (2024) : 3 mois entiers (jusqu'au
/// 10 avril), puis 14 jours (jusqu'au 24), puis 6 heures. `.days` vaut 14 et
/// non 105 : il ne compte pas les mois.
fn composants_d_une_duree_calendaire(langage: Langage) {
    let db = vide();
    let d = "duration.between(datetime('2024-01-10T00:00:00Z'), datetime('2024-04-24T06:00:00Z'))";
    for (composant, attendu) in [
        ("months", 3),
        ("days", 14),
        ("weeks", 2),
        ("hours", 6),
        ("minutes", 360),
        ("seconds", 21_600),
        ("milliseconds", 21_600_000),
    ] {
        assert_eq!(
            entier(&db, langage, &format!("RETURN {d}.{composant}")),
            attendu,
            "{langage:?} .{composant}"
        );
    }
}

/// Fin de mois : du 31 janvier au 1er mars 2024 (année bissextile) : 1 mois
/// (jusqu'au 29 février, fin de mois ramenée) et 1 jour.
fn fin_de_mois(langage: Langage) {
    let db = vide();
    let d = "duration.between(datetime('2024-01-31T00:00:00Z'), datetime('2024-03-01T00:00:00Z'))";
    assert_eq!(entier(&db, langage, &format!("RETURN {d}.months")), 1);
    assert_eq!(entier(&db, langage, &format!("RETURN {d}.days")), 1);
}

/// Dates : du 20 novembre 2023 au 5 février 2025 : 14 mois (jusqu'au 20
/// janvier 2025), puis 16 jours. `years` = 1.
fn entre_deux_dates(langage: Langage) {
    let db = vide();
    let d = "duration.between(date('2023-11-20'), date('2025-02-05'))";
    assert_eq!(entier(&db, langage, &format!("RETURN {d}.months")), 14);
    assert_eq!(entier(&db, langage, &format!("RETURN {d}.years")), 1);
    assert_eq!(entier(&db, langage, &format!("RETURN {d}.days")), 16);
}

/// Dans l'ordre inverse, la durée est l'opposé.
fn sens_inverse(langage: Langage) {
    let db = vide();
    let d = "duration.between(datetime('2024-04-24T06:00:00Z'), datetime('2024-01-10T00:00:00Z'))";
    assert_eq!(entier(&db, langage, &format!("RETURN {d}.months")), -3);
    assert_eq!(entier(&db, langage, &format!("RETURN {d}.days")), -14);
    assert_eq!(entier(&db, langage, &format!("RETURN {d}.hours")), -6);
}

/// Deux instants identiques : durée nulle.
fn instants_identiques(langage: Langage) {
    let db = vide();
    assert_eq!(
        entier(
            &db,
            langage,
            "RETURN duration.between(datetime('2024-01-01T00:00:00Z'), \
             datetime('2024-01-01T00:00:00Z')).seconds",
        ),
        0,
        "{langage:?}"
    );
}

// ============================================================================
// inMonths, inDays, inSeconds, inMilliseconds
// ============================================================================

/// Du 10 janvier au 24 avril 06:00 : 3 mois ; 105 jours entiers (jusqu'au 24
/// avril 00:00) ; 9 072 000 + 21 600 secondes.
fn variantes_in(langage: Langage) {
    let db = vide();
    let (a, b) = (
        "datetime('2024-01-10T00:00:00Z')",
        "datetime('2024-04-24T06:00:00Z')",
    );
    assert_eq!(
        entier(
            &db,
            langage,
            &format!("RETURN duration.inMonths({a}, {b}).months")
        ),
        3
    );
    assert_eq!(
        entier(
            &db,
            langage,
            &format!("RETURN duration.inDays({a}, {b}).days")
        ),
        105
    );
    // 105 jours = 9 072 000 s, plus 6 h = 21 600 s.
    assert_eq!(
        entier(
            &db,
            langage,
            &format!("RETURN duration.inSeconds({a}, {b}).seconds")
        ),
        9_093_600
    );
    assert_eq!(
        entier(
            &db,
            langage,
            &format!("RETURN duration.inMilliseconds({a}, {b}).milliseconds"),
        ),
        9_093_600_000
    );
}

/// La forme à un argument de l'application cliente : la durée d'un `between`
/// reconvertie en millisecondes (2,5 s = 2500 ms).
fn in_milliseconds_d_une_duree(langage: Langage) {
    let db = vide();
    assert_eq!(
        entier(
            &db,
            langage,
            "RETURN duration.inMilliseconds(duration.between(\
             datetime('2024-05-01T10:00:00Z'), datetime('2024-05-01T10:00:02.500Z'))).milliseconds",
        ),
        2500,
        "{langage:?}"
    );
}

// ============================================================================
// NULL, types incompatibles, usage dans un WITH
// ============================================================================

/// Un opérande NULL rend NULL ; une heure et un datetime ne se comparent pas ;
/// une durée qui porte des mois n'est pas convertible en secondes.
fn null_et_types_incompatibles(langage: Langage) {
    let db = vide();
    for requete in [
        "RETURN duration.between(null, datetime('2024-01-01T00:00:00Z'))",
        "RETURN duration.between(datetime('2024-01-01T00:00:00Z'), null)",
        "RETURN duration.between(time('10:00:00'), datetime('2024-01-01T00:00:00Z'))",
        "RETURN duration.inSeconds(duration('P1M'))",
    ] {
        assert_eq!(
            valeur(&db, langage, requete),
            Value::Null,
            "{langage:?} {requete}"
        );
    }
}

/// Dans un `WITH`, sur des propriétés : 45 secondes.
fn dans_un_with_sur_des_proprietes(langage: Langage) {
    let db = vide();
    let session = db.session();
    session.create_node_with_props(
        &["Run"],
        [
            (
                "started_at",
                Value::Timestamp(horodatage((2024, 3, 10), (8, 0, 0), 0)),
            ),
            (
                "ended_at",
                Value::Timestamp(horodatage((2024, 3, 10), (8, 0, 45), 0)),
            ),
        ],
    );
    assert_eq!(
        entier(
            &db,
            langage,
            "MATCH (r:Run) \
             WITH duration.between(r.started_at, r.ended_at).seconds AS s \
             RETURN s",
        ),
        45,
        "{langage:?}"
    );
}

/// Un `CASE` autour de l'appel, comme dans `update_staleness_scores`.
fn dans_un_case(langage: Langage) {
    let db = vide();
    assert_eq!(
        entier(
            &db,
            langage,
            "RETURN CASE WHEN true \
             THEN duration.between(datetime('2024-01-01T00:00:00Z'), \
                                   datetime('2024-01-04T00:00:00Z')).days \
             ELSE -1 END",
        ),
        3,
        "{langage:?}"
    );
}

// ============================================================================
// Fuseaux : le calendrier se lit dans le fuseau du premier opérande
// ============================================================================

/// `2024-01-31T00:30+02:00` est le 31 janvier à Paris (le 30, 22:30 en UTC).
/// Jusqu'au 1er mars 00:30+02:00 : un mois (fin de mois ramenée au 29
/// février) et un jour — en UTC on aurait lu du 30 janvier au 29 février,
/// soit un mois tout rond.
fn fuseau_non_utc_calendrier_local(langage: Langage) {
    let db = vide();
    let d = "duration.between(datetime('2024-01-31T00:30:00+02:00'), \
             datetime('2024-03-01T00:30:00+02:00'))";
    assert_eq!(
        entier(&db, langage, &format!("RETURN {d}.months")),
        1,
        "{langage:?}"
    );
    assert_eq!(
        entier(&db, langage, &format!("RETURN {d}.days")),
        1,
        "{langage:?}"
    );
    assert_eq!(
        entier(&db, langage, &format!("RETURN {d}.hours")),
        0,
        "{langage:?}"
    );
}

/// Deux fuseaux différents : le second instant est lu dans le fuseau du
/// premier. `2024-03-01T00:30Z` est `02:30+02:00` ; depuis le 31 janvier
/// 23:30+02:00 : un mois (29 février 23:30), puis 3 heures.
fn fuseaux_differents_lus_dans_celui_du_premier(langage: Langage) {
    let db = vide();
    let d = "duration.between(datetime('2024-01-31T23:30:00+02:00'), \
             datetime('2024-03-01T00:30:00Z'))";
    assert_eq!(
        entier(&db, langage, &format!("RETURN {d}.months")),
        1,
        "{langage:?}"
    );
    assert_eq!(
        entier(&db, langage, &format!("RETURN {d}.days")),
        0,
        "{langage:?}"
    );
    assert_eq!(
        entier(&db, langage, &format!("RETURN {d}.hours")),
        3,
        "{langage:?}"
    );
    // Les secondes ne dépendent d'aucun fuseau : 00:00+02:00 est 22:00Z la
    // veille, soit 7200 s avant 00:00Z.
    assert_eq!(
        entier(
            &db,
            langage,
            "RETURN duration.inSeconds(datetime('2024-01-01T00:00:00+02:00'), \
             datetime('2024-01-01T00:00:00Z')).seconds",
        ),
        7200,
        "{langage:?}"
    );
}

// ============================================================================
// Composant d'autre chose qu'une durée
// ============================================================================

/// Un nœud sorti d'une liste (`collect(n)[0]`, `head(collect(n))`) : sa
/// propriété se lit (il voyage comme son identifiant, résolu par le store).
fn composant_d_un_noeud_sorti_d_une_liste(langage: Langage) {
    let db = vide();
    db.session()
        .create_node_with_props(&["Commit"], [("sha", Value::String("c1".into()))]);
    for requete in [
        "MATCH (n:Commit) WITH collect(n) AS ns RETURN ns[0].sha",
        "MATCH (n:Commit) RETURN head(collect(n)).sha",
        "MATCH (n:Commit) WITH collect(n) AS ns RETURN ns[0].absente",
    ] {
        let attendu = if requete.ends_with("absente") {
            Value::Null
        } else {
            Value::String("c1".into())
        };
        assert_eq!(
            valeur(&db, langage, requete),
            attendu,
            "{langage:?} {requete}"
        );
    }
}

/// Une valeur qui n'a pas de composants (un texte, un entier qui n'est l'id
/// d'aucune entité) : une ERREUR, pas un NULL silencieux.
fn composant_d_une_valeur_sans_composants_est_une_erreur(langage: Langage) {
    let db = vide();
    let session = db.session();
    for requete in ["RETURN 'abc'.len", "RETURN (12345678).sha"] {
        let resultat = match langage {
            Langage::Cypher => session.execute_cypher(requete),
            Langage::Gql => session.execute(requete),
        };
        assert!(resultat.is_err(), "[{langage:?}] {requete} devait échouer");
    }
}

// ============================================================================
// WITH … MERGE … SET : la forme d'écriture de la requête de fédération
// ============================================================================

/// Un agrégat flottant traverse un `MERGE` puis un `SET` : 2000.0, pas 0
/// (le vecteur de passage déclaré « nœud » perdait la valeur).
fn with_agrege_puis_merge_set(langage: Langage) {
    let db = vide();
    let session = db.session();
    for fqn in ["x::alpha", "x::beta"] {
        session.create_node_with_props(&["McpTool"], [("fqn", Value::String(fqn.into()))]);
    }
    executer(
        &db,
        langage,
        "UNWIND [2500, 1500] AS ms \
         MATCH (a:McpTool {fqn: 'x::alpha'}), (b:McpTool {fqn: 'x::beta'}) \
         WITH a, b, count(*) AS follow_count, avg(ms) AS avg_delta \
         MERGE (a)-[r:OFTEN_FOLLOWS]->(b) \
         SET r.count = follow_count, r.avg_delta_ms = avg_delta \
         RETURN count(r) AS total",
    );
    assert_eq!(
        executer(
            &db,
            langage,
            "MATCH (:McpTool)-[r:OFTEN_FOLLOWS]->(:McpTool) RETURN r.count, r.avg_delta_ms",
        ),
        vec![vec![Value::Int64(2), Value::Float64(2000.0)]],
        "{langage:?}"
    );
}

/// `SET` puis `RETURN` dans la MÊME requête relit la valeur écrite, sur une
/// arête comme sur un nœud (la colonne d'arête traversant le `SET` était
/// retypée « nœud » : `r.p` relisait le nœud n° id-de-l'arête, NULL).
fn set_puis_return_dans_la_meme_requete(langage: Langage) {
    let db = vide();
    let session = db.session();
    let a = session.create_node_with_props(&["T"], [("k", Value::String("a".into()))]);
    let b = session.create_node_with_props(&["T"], [("k", Value::String("b".into()))]);
    session.create_edge(a, b, "R");
    assert_eq!(
        executer(
            &db,
            langage,
            "MATCH (:T)-[r:R]->(:T) SET r.poids = 1.5, r.n = 7 RETURN r.poids, r.n",
        ),
        vec![vec![Value::Float64(1.5), Value::Int64(7)]],
        "{langage:?} arête"
    );
    assert_eq!(
        executer(
            &db,
            langage,
            "MATCH (t:T {k: 'a'}) SET t.poids = 2.5 RETURN t.poids, t.k",
        ),
        vec![vec![Value::Float64(2.5), Value::String("a".into())]],
        "{langage:?} nœud"
    );
}

/// Déclare un module de tests : chaque cas listé y devient un `#[test]` joué
/// dans le langage donné.
macro_rules! cas_en {
    ($module:ident, $langage:expr, [$($cas:ident),+ $(,)?]) => {
        mod $module {
            $(#[test] fn $cas() { super::$cas($langage); })+
        }
    };
}

macro_rules! les_deux_langages {
    ($($cas:ident),+ $(,)?) => {
        cas_en!(cypher, super::Langage::Cypher, [$($cas),+]);
        cas_en!(gql, super::Langage::Gql, [$($cas),+]);
    };
}

les_deux_langages!(
    entre_deux_datetimes_vaut_une_duree_litterale,
    composants_d_une_duree_calendaire,
    fin_de_mois,
    entre_deux_dates,
    sens_inverse,
    instants_identiques,
    variantes_in,
    in_milliseconds_d_une_duree,
    null_et_types_incompatibles,
    dans_un_with_sur_des_proprietes,
    dans_un_case,
    fuseau_non_utc_calendrier_local,
    fuseaux_differents_lus_dans_celui_du_premier,
    composant_d_un_noeud_sorti_d_une_liste,
    composant_d_une_valeur_sans_composants_est_une_erreur,
    set_puis_return_dans_la_meme_requete,
);

// En GQL, les clauses d'écriture après un `WITH` sont lues et ordonnées par
// la PR « un WITH s'exécute à sa position » ; ici, Cypher seulement.
#[test]
fn cypher_with_agrege_puis_merge_set() {
    with_agrege_puis_merge_set(Langage::Cypher);
}

// ============================================================================
// Les requêtes réelles de l'application cliente, telles quelles
// ============================================================================

/// `mcp_federation.rs` : paires d'événements consécutifs d'une session, durée
/// entre eux en millisecondes, puis `OFTEN_FOLLOWS` avec le compte et la
/// moyenne.
///
/// Deux sessions, chacune avec deux événements `tool_use` consécutifs
/// (alpha puis beta) : écarts de 2,5 s et de 1,5 s. Attendu : une arête
/// alpha -> beta, `count` = 2, `avg_delta_ms` = (2500 + 1500) / 2 = 2000.
#[test]
fn requete_reelle_federation_mcp() {
    let db = vide();
    let session = db.session();
    for (id, debut, fin) in [
        (
            "s1",
            horodatage((2024, 5, 1), (10, 0, 0), 0),
            horodatage((2024, 5, 1), (10, 0, 2), 500_000_000),
        ),
        (
            "s2",
            horodatage((2024, 5, 2), (9, 0, 0), 0),
            horodatage((2024, 5, 2), (9, 0, 1), 500_000_000),
        ),
    ] {
        for (seq, data, quand) in [(1, "x::alpha", debut), (2, "x::beta", fin)] {
            session.create_node_with_props(
                &["ChatEventRecord"],
                [
                    ("session_id", Value::String(id.into())),
                    ("event_type", Value::String("tool_use".into())),
                    ("data", Value::String(data.into())),
                    ("seq", Value::Int64(seq)),
                    ("created_at", Value::Timestamp(quand)),
                ],
            );
        }
    }
    for fqn in ["x::alpha", "x::beta"] {
        session.create_node_with_props(&["McpTool"], [("fqn", Value::String(fqn.into()))]);
    }

    let lignes = executer(
        &db,
        Langage::Cypher,
        r#"
            MATCH (e1:ChatEventRecord), (e2:ChatEventRecord)
            WHERE e1.session_id = e2.session_id
              AND e1.event_type = 'tool_use' AND e1.data CONTAINS '::'
              AND e2.event_type = 'tool_use' AND e2.data CONTAINS '::'
              AND e2.seq = e1.seq + 1
            WITH e1.data AS data1, e2.data AS data2,
                 duration.inMilliseconds(
                     duration.between(e1.created_at, e2.created_at)
                 ).milliseconds AS delta_ms
            MATCH (a:McpTool) WHERE data1 CONTAINS a.fqn
            MATCH (b:McpTool) WHERE data2 CONTAINS b.fqn
            WITH a, b, count(*) AS follow_count, avg(delta_ms) AS avg_delta
            MERGE (a)-[r:OFTEN_FOLLOWS]->(b)
            SET r.count = follow_count, r.avg_delta_ms = avg_delta, r.updated_at = datetime()
            RETURN count(r) AS total
        "#,
    );
    assert_eq!(lignes, vec![vec![Value::Int64(1)]]);

    let arete = executer(
        &db,
        Langage::Cypher,
        "MATCH (a:McpTool)-[r:OFTEN_FOLLOWS]->(b:McpTool) \
         RETURN a.fqn, b.fqn, r.count, r.avg_delta_ms",
    );
    assert_eq!(
        arete,
        vec![vec![
            Value::String("x::alpha".into()),
            Value::String("x::beta".into()),
            Value::Int64(2),
            Value::Float64(2000.0),
        ]]
    );
}

/// `note.rs` : `update_staleness_scores`.
///
/// Trois notes, confirmées il y a 20 jours (moins d'un mois : `.days` = 20) :
///
/// - A, `tip` (90 jours de demi-vie), `medium` (facteur 1,0), jamais « pingée »
///   (`days_since_ping` = -1, remise 0) : staleness = 1 - e^(-20/90) ≈ 0,1993,
///   écart avec 0,0 supérieur à 0,01 : mise à jour ;
/// - B, `tip`, `critical` (facteur 0,5), pingée il y a 10 jours : remise
///   e^(-10/30) x 0,5 ≈ 0,3583 ; brute (1 - e^(-20/90)) x 0,5 ≈ 0,0996 ;
///   brute - remise < 0 : bornée à 0,0 ; l'ancien score 0,5 s'en écarte de
///   plus de 0,01 : mise à jour ;
/// - C, `assertion` : exclue.
///
/// Attendu : 2 notes mises à jour, A = 0,1993, B = 0,0, C inchangée.
#[test]
fn requete_reelle_note_staleness() {
    let db = vide();
    let session = db.session();
    let il_y_a = |jours: i64| {
        Value::String(
            Timestamp::from_nanos(Timestamp::now().as_nanos() - jours * 86_400_000_000_000)
                .to_string()
                .into(),
        )
    };
    let note = |id: &str,
                genre: &str,
                importance: &str,
                confirmee: Value,
                pingee: Option<Value>,
                score: f64| {
        let mut proprietes = vec![
            ("id", Value::String(id.into())),
            ("status", Value::String("active".into())),
            ("note_type", Value::String(genre.into())),
            ("importance", Value::String(importance.into())),
            ("last_confirmed_at", confirmee),
            ("staleness_score", Value::Float64(score)),
        ];
        if let Some(p) = pingee {
            proprietes.push(("freshness_pinged_at", p));
        }
        session.create_node_with_props(&["Note"], proprietes);
    };
    note("A", "tip", "medium", il_y_a(20), None, 0.0);
    note("B", "tip", "critical", il_y_a(20), Some(il_y_a(10)), 0.5);
    note("C", "assertion", "medium", il_y_a(20), None, 0.0);

    let lignes = executer(
        &db,
        Langage::Cypher,
        r#"
            MATCH (n:Note)
            WHERE n.status = 'active' AND n.note_type <> 'assertion'
            WITH n,
                 duration.between(
                     datetime(n.last_confirmed_at),
                     datetime()
                 ).days AS days_since_confirmed,
                 CASE n.note_type
                     WHEN 'context' THEN 30.0
                     WHEN 'tip' THEN 90.0
                     WHEN 'observation' THEN 90.0
                     WHEN 'gotcha' THEN 180.0
                     WHEN 'guideline' THEN 365.0
                     WHEN 'pattern' THEN 365.0
                     WHEN 'rfc' THEN 365.0
                     ELSE 90.0
                 END AS base_decay_days,
                 CASE n.importance
                     WHEN 'critical' THEN 0.5
                     WHEN 'high' THEN 0.7
                     WHEN 'medium' THEN 1.0
                     ELSE 1.3
                 END AS decay_factor,
                 CASE
                     WHEN n.freshness_pinged_at IS NOT NULL
                     THEN duration.between(datetime(n.freshness_pinged_at), datetime()).days
                     ELSE -1
                 END AS days_since_ping
            WITH n, days_since_confirmed, base_decay_days, decay_factor,
                 CASE
                     WHEN days_since_ping >= 0
                     THEN exp(-1.0 * days_since_ping / 30.0) * 0.5
                     ELSE 0.0
                 END AS freshness_discount
            WITH n,
                 CASE
                     WHEN base_decay_days = 0 THEN 0
                     ELSE (1.0 - exp(-1.0 * days_since_confirmed / base_decay_days)) * decay_factor
                 END AS raw_staleness,
                 freshness_discount
            WITH n,
                 CASE WHEN (raw_staleness - freshness_discount) > 1.0 THEN 1.0
                      WHEN (raw_staleness - freshness_discount) < 0.0 THEN 0.0
                      ELSE (raw_staleness - freshness_discount) END AS clamped_staleness
            WHERE abs(n.staleness_score - clamped_staleness) > 0.01
            SET n.staleness_score = clamped_staleness,
                n.status = CASE WHEN clamped_staleness > 0.8 THEN 'stale' ELSE n.status END
            RETURN count(n) AS updated
        "#,
    );
    assert_eq!(lignes, vec![vec![Value::Int64(2)]]);

    let scores = executer(
        &db,
        Langage::Cypher,
        "MATCH (n:Note) RETURN n.id, n.staleness_score, n.status ORDER BY n.id",
    );
    let score = |ligne: &Vec<Value>| match ligne[1] {
        Value::Float64(f) => f,
        ref autre => panic!("score attendu, reçu {autre:?}"),
    };
    // A : 1 - e^(-20/90) = 0,19926...
    assert!((score(&scores[0]) - 0.199_262).abs() < 1e-5, "{scores:?}");
    // B : borné à 0.
    assert_eq!(score(&scores[1]), 0.0, "{scores:?}");
    // C : exclue, inchangée.
    assert_eq!(score(&scores[2]), 0.0, "{scores:?}");
    assert!(
        scores
            .iter()
            .all(|l| l[2] == Value::String("active".into()))
    );
}
