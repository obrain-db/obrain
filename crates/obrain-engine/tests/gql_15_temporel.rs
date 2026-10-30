//! Conformité GQL — lot 15 : le TEMPOREL.
//!
//! Dates RFC 3339 à **précisions variables** (0, 1, 3, 6 ou 9 décimales),
//! fuseaux (`Z`, `+00:00`, `+02:00`), franchissements de rang, avant 1970,
//! et l'écart entre ordre **lexicographique** et ordre **chronologique**.
//!
//! # Le principe qui gouverne ce fichier
//!
//! Chaque assertion encode l'**intention** sémantique — l'ordre lexicographique
//! doit coïncider avec l'ordre chronologique, deux écritures du même instant
//! doivent se comparer égales — jamais l'observation. Là où le moteur diverge,
//! le test porte `#[ignore]`, énonce ce que la sémantique EXIGE, et échoue
//! aujourd'hui avec le message de la trouvaille. Les figer sur l'observé
//! rendrait les défauts intouchables. `cargo test … -- --ignored` les rejoue ;
//! ils deviendront verts le jour de la correction, sans qu'une ligne
//! d'assertion ne bouge.
//!
//! # Le piège propre à ce lot, et sa parade
//!
//! Un test temporel dont toutes les dates ont la MÊME précision passe
//! trivialement et ne prouve rien. Chaque fixture passe donc par
//! [`exiger_precisions_melangees`], qui fait échouer le test si le jeu de
//! données ne porte pas au moins trois longueurs de fraction distinctes.
//! Ce garde-fou est dirigé contre l'auteur du test, pas contre le moteur.
//!
//! # Ce que ce fichier corrige d'un relevé antérieur
//!
//! La fiche d'atelier `revue2_forme_date_non_exacte` (agent
//! `revue-adverse-cadence`, 2026-08-24T14:40) conclut que « la comparaison
//! lexicographique reste correcte dans tous les cas sauf l'égalité exacte, car
//! le séparateur plus vaut 0x2B et se classe avant le point 0x2E et avant tout
//! chiffre ». **Cette garantie ne tient que pour les décalages NUMÉRIQUES.**
//! Le désignateur UTC `Z` vaut 0x5A : il se classe APRÈS le point ET après
//! tout chiffre. Or `ZonedDatetime::Display` — code de PRODUCTION de ce dépôt,
//! `crates/obrain-common/src/types/zoned_datetime.rs:186` — écrit `Z` pour
//! l'offset zéro *et* rogne les zéros de fin de la fraction. Le moteur produit
//! donc lui-même, sans qu'on écrive une seule chaîne à la main, le couple qui
//! casse la coïncidence. Voir [`divergence_le_z_casse_la_coincidence_lexicographique`].
//!
//! # Divergences mesurées (2026-08-24, agent `gql-15-temporel`)
//!
//! | # | Divergence | Cause |
//! |---|---|---|
//! | D1 | `=` sur un temporel est TOUJOURS faux, `<>` toujours vrai — même valeur contre elle-même | `filter.rs:3413` `values_equal` n'a aucune branche temporelle et retombe sur `_ => false` |
//! | D2 | Le même prédicat rend deux réponses selon qu'il est poussé dans le scan ou évalué en expression | `factorized_filter.rs:170` traite `Timestamp` ; `filter.rs:3413` non |
//! | D3 | `ORDER BY` sur `ZONED DATETIME` est inerte : ASC et DESC rendent l'ordre d'insertion | `value_utils.rs:57` `compare_values_total` n'a pas de branche `ZonedDatetime` → `Ordering::Equal` |
//! | D4 | `min()`/`max()` sur `ZONED DATETIME` rendent `Int64(0)` | même chemin `value_utils.rs` (classe déjà relevée par gql-02 D4 sur les chaînes ; ici sur un type TEMPOREL) |
//! | D5 | Colonne mixte `Timestamp`+chaîne : `ORDER BY` rend l'ordre d'insertion, les deux `Timestamp` compris | `compare_values_total` rend `Equal` pour les paires croisées → comparateur non transitif |
//! | D6 | **Cas décisif** — l'ordre lexicographique et l'ordre chronologique DIVERGENT dès que l'offset zéro s'écrit `Z` | `'Z'` = 0x5A > `'.'` = 0x2E, alors que `'+'` = 0x2B < 0x2E |
//! | D7 | Une borne basse TRONQUÉE à la seconde EXCLUT des lignes qu'une borne à neuf décimales inclut — le mauvais sens | conséquence de D6 sur `>=` |
//! | D8 | Deux instants distincts d'1 ns deviennent ÉGAUX | `Timestamp` est en microsecondes (`timestamp.rs:17`) ; `Time::parse` tronque |
//! | D9 | Des dates MALFORMÉES sont acceptées comme instants réels (`.+5`, `T+10:00:00`, `2026-+1-01`) | `time.rs`/`date.rs` délèguent à `str::parse::<u32>`, qui accepte un `+` de tête |
//! | D10 | Une date malformée n'est pas REJETÉE : elle dégrade silencieusement en chaîne, puis se compare faux contre tout instant | `expression.rs:279` `map_or_else(… Value::String …)` |
//! | D11 | Les minuscules `t`/`z`, explicitement permises par RFC 3339 §4.2, sont refusées côté `z` | `time.rs` `parse_offset_suffix` ne teste que `'Z'` |
//! | D12 | Stockées en chaîne, deux écritures du même instant à des fuseaux différents ne sont ni égales ni ordonnées chronologiquement | aucun code — la comparaison lexicographique ne peut pas normaliser un offset |
//!
//! # Campagne anti-vacuité (19 mutations)
//!
//! Chaque test vert ci-dessous a été prouvé falsifiable : une mutation du code
//! de PRODUCTION le fait rougir, et le message qui sort est bien celui de
//! l'intention violée. Les treize tests `#[ignore]` sont falsifiables par
//! construction — ils échouent aujourd'hui, chacun sur son propre témoin.
//!
//! | Mutation | Verdict | Test qui rougit |
//! |---|---|---|
//! | M1 branche `Timestamp` retirée de `compare_values_total` | ROUGIT | ordre des instants (3 tests) |
//! | M2 branche `Date` inversée dans `compare_values_total` | ROUGIT | années courtes et négatives |
//! | M3 chaînes comparées par LONGUEUR | ROUGIT | garantie de l'offset numérique |
//! | M4 décalage non retranché à la conversion UTC | ROUGIT | même instant, six écritures |
//! | M5 fractions de moins de 9 chiffres ignorées | ROUGIT | 3 tests |
//! | M6 `Z` interprété comme `+01:00` | ROUGIT | même instant, six écritures |
//! | M7 signe de l'année perdu | ROUGIT | années courtes et négatives |
//! | M8 jour du mois non validé | ROUGIT | date impossible ; franchissement de jour |
//! | M10 zéros de fin non rognés (précision uniforme) | ROUGIT | **le garde-fou `exiger_precisions_melangees`** |
//! | M17 champ SECONDES ignoré | ROUGIT | bornes tronquées ; ORDER BY ; date impossible |
//! | M19 `Ord` de `Timestamp` inversé à la source | ROUGIT | 5 tests |
//! | M9, M11–M16 six autres copies de la comparaison temporelle | **SURVIVENT** | aucun |
//!
//! ## Ce que les survivants révèlent
//!
//! `obrain-core` porte **onze** implémentations distinctes de la comparaison
//! de deux `Value`. Inverser ou supprimer la branche `Timestamp` de six
//! d'entre elles — `filter.rs:3560`, les deux copies de
//! `factorized_filter.rs`, `graph/lpg/property.rs:1737`, `index/zone_map.rs:583`,
//! `execution/parallel/merge.rs:309` — ne change RIEN d'observable à travers
//! GQL. Forcer `ExpressionPredicate::compare_values` à rendre `None` en toute
//! circonstance ne change rien non plus (M18).
//!
//! Le chemin réellement emprunté par `WHERE n.at >= DATETIME '…'` est l'index
//! ORDONNÉ sur propriétés : `OrderableValue`
//! (`obrain-common/src/types/value.rs:700`), encodé en clé binaire par
//! `obrain-substrate/src/props_index.rs:294`. C'est une **septième** écriture
//! de l'ordre temporel, et c'est la seule qui compte pour les prédicats
//! d'intervalle. Le tri, lui, passe par `compare_values_total`.
//!
//! Deux ordres temporels, écrits séparément, pour deux clauses de la même
//! requête : c'est la configuration exacte qui produit D1, D2 et D3. Et
//! `props_index.rs` documente déjà, en commentaire, que `Time` n'a pas d'ordre
//! total cohérent — le même genre de dette, déjà constaté ailleurs.

#![cfg(feature = "gql")]

use obrain_common::types::{Timestamp, Value, ZonedDatetime};
use obrain_engine::{ObrainDB, Session};

// ---------------------------------------------------------------------------
// Harnais — les états sont construits par le seul code de production.
// ---------------------------------------------------------------------------

/// Exécute `query`, ou fait échouer le test en citant la requête fautive.
///
/// Un test qui avale une erreur de syntaxe est un test vide.
fn exec(s: &Session, query: &str) -> Vec<Vec<Value>> {
    s.execute(query)
        .unwrap_or_else(|e| panic!("requête refusée par le moteur :\n  {query}\n  → {e}"))
        .rows
}

/// Première colonne, **dans l'ordre rendu** (pour tout ce qui touche au tri).
fn cles(s: &Session, query: &str) -> Vec<String> {
    exec(s, query)
        .iter()
        .map(|row| match &row[0] {
            Value::String(v) => v.to_string(),
            autre => format!("{autre:?}"),
        })
        .collect()
}

/// Première colonne triée par clé (pour tout ce qui touche à l'appartenance).
fn cles_triees(s: &Session, query: &str) -> Vec<String> {
    let mut v = cles(s, query);
    v.sort();
    v
}

/// Évalue une expression scalaire GQL.
fn scalaire(s: &Session, expr: &str) -> Value {
    let query = format!("RETURN {expr} AS v");
    let rows = exec(s, &query);
    assert_eq!(
        rows.len(),
        1,
        "l'expression doit produire exactement 1 ligne, sinon elle n'est pas évaluée : {query}"
    );
    rows[0][0].clone()
}

/// Nombre de décimales de la fraction de seconde d'un rendu RFC 3339.
fn nb_decimales(rendu: &str) -> usize {
    match rendu.find('.') {
        None => 0,
        Some(p) => rendu[p + 1..]
            .chars()
            .take_while(char::is_ascii_digit)
            .count(),
    }
}

/// **Garde-fou anti-vacuité dirigé contre l'auteur du test.**
///
/// Si toutes les dates d'une fixture portent la même précision, le test qui
/// s'appuie dessus passe trivialement et ne prouve rien. On exige au moins
/// trois longueurs de fraction distinctes.
fn exiger_precisions_melangees(ecritures: &[String]) {
    let mut vues: Vec<usize> = ecritures.iter().map(|e| nb_decimales(e)).collect();
    vues.sort_unstable();
    vues.dedup();
    assert!(
        vues.len() >= 3,
        "FIXTURE VIDE DE SENS : les {} dates ne portent que {} précision(s) distincte(s) \
         ({vues:?}). Un test temporel à précision uniforme passe trivialement. \
         Écritures : {ecritures:?}",
        ecritures.len(),
        vues.len(),
    );
}

/// Instant de référence : `2026-01-01T10:00:00Z`, en microsecondes.
const ANCRE: i64 = 1_767_261_600_000_000;

/// Corpus d'instants **rendu par le code de production** (`ZonedDatetime::Display`),
/// au décalage demandé. Les micros sont choisis pour EXERCER les précisions :
/// 0, 1, 3 et 6 décimales, de part et d'autre d'un franchissement de seconde.
///
/// Rend `(micros_utc, écriture)`, du plus ancien au plus récent.
fn corpus_production(offset_seconds: i32) -> Vec<(i64, String)> {
    let micros = [
        ANCRE - 1,         // 09:59:59.999999 — 6 décimales
        ANCRE,             // 10:00:00        — 0 décimale
        ANCRE + 1,         // 10:00:00.000001 — 6 décimales
        ANCRE + 123_000,   // 10:00:00.123    — 3 décimales
        ANCRE + 500_000,   // 10:00:00.5      — 1 décimale
        ANCRE + 999_999,   // 10:00:00.999999 — 6 décimales
        ANCRE + 1_000_000, // 10:00:01        — 0 décimale
    ];
    let corpus: Vec<(i64, String)> = micros
        .iter()
        .map(|&m| {
            let z = ZonedDatetime::from_timestamp_offset(Timestamp::from_micros(m), offset_seconds);
            (m, z.to_string())
        })
        .collect();
    exiger_precisions_melangees(&corpus.iter().map(|(_, e)| e.clone()).collect::<Vec<_>>());
    corpus
}

/// Première paire du corpus dont l'ordre lexicographique contredit l'ordre
/// chronologique, s'il en existe une.
fn premiere_inversion(corpus: &[(i64, String)]) -> Option<(String, String)> {
    for (mi, si) in corpus {
        for (mj, sj) in corpus {
            if mi.cmp(mj) != si.as_str().cmp(sj.as_str()) {
                return Some((si.clone(), sj.clone()));
            }
        }
    }
    None
}

/// Charge un corpus comme propriété CHAÎNE `at`, clé `k` = rang chronologique.
fn charger_chaines(s: &Session, label: &str, corpus: &[(i64, String)]) {
    for (rang, (_, ecriture)) in corpus.iter().enumerate() {
        exec(
            s,
            &format!("INSERT (:{label} {{k:'r{rang}', at:'{ecriture}'}})"),
        );
    }
}

/// Rangs chronologiques attendus, dans l'ordre.
fn rangs(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("r{i}")).collect()
}

// ===========================================================================
// G1 — Un même instant écrit à des précisions et des fuseaux différents
// ===========================================================================

/// Les six écritures RFC 3339 du MÊME instant — 0, 3, 6, 9 décimales, `Z`,
/// `+00:00`, `+02:00` — doivent être une seule et même valeur : elles se
/// comparent égales, et elles se trient ensemble.
#[test]
fn typé_meme_instant_toutes_precisions_est_une_seule_valeur() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();

    let ecritures = [
        "2026-01-01T10:00:00Z".to_string(),
        "2026-01-01T10:00:00.000Z".to_string(),
        "2026-01-01T10:00:00.000000Z".to_string(),
        "2026-01-01T10:00:00.000000000Z".to_string(),
        "2026-01-01T10:00:00+00:00".to_string(),
        "2026-01-01T12:00:00+02:00".to_string(),
    ];
    exiger_precisions_melangees(&ecritures);

    for (i, e) in ecritures.iter().enumerate() {
        exec(
            &s,
            &format!("INSERT (:M1 {{k:'e{i}', at: DATETIME '{e}'}})"),
        );
    }
    // Deux témoins hors de l'instant, pour qu'aucune requête ne puisse passer
    // en ramassant simplement toute la table.
    exec(
        &s,
        "INSERT (:M1 {k:'avant', at: DATETIME '2026-01-01T09:59:59.999999Z'})",
    );
    exec(
        &s,
        "INSERT (:M1 {k:'apres', at: DATETIME '2026-01-01T10:00:00.000001Z'})",
    );

    let attendu: Vec<String> = (0..ecritures.len()).map(|i| format!("e{i}")).collect();

    // Toute écriture de l'instant retrouve TOUTES les autres, quelle que soit
    // celle qui sert de sonde.
    for e in &ecritures {
        assert_eq!(
            cles_triees(
                &s,
                &format!("MATCH (n:M1) WHERE n.at = DATETIME '{e}' RETURN n.k")
            ),
            attendu,
            "la sonde « {e} » doit retrouver les {} écritures du même instant \
             — et elles seules : ni le témoin « avant » ni le témoin « apres »",
            ecritures.len()
        );
    }

    // Et elles se trient ensemble : un bloc contigu, encadré par les témoins.
    let ordre = cles(&s, "MATCH (n:M1) RETURN n.k ORDER BY n.at ASC");
    assert_eq!(
        ordre.first().map(String::as_str),
        Some("avant"),
        "le témoin antérieur doit ouvrir le tri, ordre obtenu : {ordre:?}"
    );
    assert_eq!(
        ordre.last().map(String::as_str),
        Some("apres"),
        "le témoin postérieur doit fermer le tri, ordre obtenu : {ordre:?}"
    );
    let mut milieu: Vec<String> = ordre[1..ordre.len() - 1].to_vec();
    milieu.sort();
    assert_eq!(
        milieu, attendu,
        "les écritures du même instant doivent former un bloc CONTIGU entre \
         les deux témoins, ordre obtenu : {ordre:?}"
    );
}

// ===========================================================================
// G2 — Franchissements de rang
// ===========================================================================

/// Un franchissement de rang (seconde, minute, heure, jour, siècle, millénaire)
/// écrit avec PLUS de décimales du côté antérieur reste chronologique.
#[test]
fn typé_franchissement_de_rang_avec_precisions_opposees() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();

    // Chaque couple : (antérieur RICHE en décimales, postérieur PAUVRE).
    // C'est la disposition qui piège l'ordre lexicographique naïf. La longueur
    // de fraction change d'un couple à l'autre — 9, 3, 1, 6, 2, 9 — pour que la
    // colonne globale ne soit jamais à précision uniforme.
    let couples = [
        ("seconde", "2026-01-01T09:59:59.999999999Z", "2026-01-01T10:00:00Z"),
        ("minute", "2026-01-01T10:00:59.999Z", "2026-01-01T10:01:00Z"),
        ("heure", "2026-01-01T10:59:59.9Z", "2026-01-01T11:00:00Z"),
        ("jour", "2026-01-01T23:59:59.999999Z", "2026-01-02T00:00:00Z"),
        ("millénaire", "1999-12-31T23:59:59.99Z", "2000-01-01T00:00:00Z"),
        ("époque", "1969-12-31T23:59:59.999999999Z", "1970-01-01T00:00:00Z"),
    ];
    let toutes: Vec<String> = couples
        .iter()
        .flat_map(|(_, a, b)| [(*a).to_string(), (*b).to_string()])
        .collect();
    exiger_precisions_melangees(&toutes);

    for (rang, avant, apres) in couples {
        let label = format!("F{}", rang.chars().filter(char::is_ascii_alphanumeric).collect::<String>());
        exec(&s, &format!("INSERT (:{label} {{k:'avant', at: DATETIME '{avant}'}})"));
        exec(&s, &format!("INSERT (:{label} {{k:'apres', at: DATETIME '{apres}'}})"));

        assert_eq!(
            cles(&s, &format!("MATCH (n:{label}) RETURN n.k ORDER BY n.at ASC")),
            vec!["avant".to_string(), "apres".to_string()],
            "franchissement de {rang} : « {avant} » ({} décimales) précède \
             « {apres} » (0 décimale) — le tri doit être chronologique, pas lexicographique",
            nb_decimales(avant)
        );
        assert_eq!(
            cles(
                &s,
                &format!("MATCH (n:{label}) WHERE n.at < DATETIME '{apres}' RETURN n.k")
            ),
            vec!["avant".to_string()],
            "franchissement de {rang} : « {avant} » doit être STRICTEMENT antérieur à « {apres} »"
        );
    }
}

// ===========================================================================
// G3 — Avant 1970, années courtes, années négatives
// ===========================================================================

/// Les instants antérieurs à l'époque Unix — y compris sous-seconde, là où le
/// signe du compteur interne change — s'ordonnent chronologiquement.
#[test]
fn typé_avant_1970_y_compris_sous_seconde() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();

    // Ordre chronologique voulu, précisions délibérément mélangées.
    let chronologie = [
        ("r0", "1900-01-01T00:00:00Z"),
        ("r1", "1969-07-20T20:17:40.123Z"),
        ("r2", "1969-12-31T23:59:59Z"),
        ("r3", "1969-12-31T23:59:59.5Z"),
        ("r4", "1969-12-31T23:59:59.999999Z"),
        ("r5", "1970-01-01T00:00:00Z"),
        ("r6", "1970-01-01T00:00:00.000001Z"),
    ];
    exiger_precisions_melangees(
        &chronologie
            .iter()
            .map(|(_, e)| (*e).to_string())
            .collect::<Vec<_>>(),
    );
    // Inséré à l'envers : l'ordre de scan ne doit pas pouvoir sauver le test.
    for (k, e) in chronologie.iter().rev() {
        exec(&s, &format!("INSERT (:AV {{k:'{k}', at: DATETIME '{e}'}})"));
    }

    assert_eq!(
        cles(&s, "MATCH (n:AV) RETURN n.k ORDER BY n.at ASC"),
        rangs(chronologie.len()),
        "les instants antérieurs à 1970, dont quatre à moins d'une seconde de \
         l'époque et de part et d'autre du changement de signe, doivent \
         s'ordonner chronologiquement"
    );
}

/// Les années à moins de quatre chiffres et les années négatives, acceptées par
/// le moteur, s'ordonnent chronologiquement.
#[test]
fn typé_annees_courtes_et_negatives_sordonnent() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();

    let chronologie = [
        ("r0", "-0044-03-15"),
        ("r1", "0001-01-01"),
        ("r2", "0999-12-31"),
        ("r3", "1969-07-20"),
        ("r4", "2026-01-01"),
    ];
    for (k, e) in chronologie.iter().rev() {
        let v = scalaire(&s, &format!("DATE '{e}'"));
        assert!(
            matches!(v, Value::Date(_)),
            "« {e} » doit être reconnue comme une DATE, pas dégradée en {v:?}"
        );
        exec(&s, &format!("INSERT (:AN {{k:'{k}', at: DATE '{e}'}})"));
    }

    assert_eq!(
        cles(&s, "MATCH (n:AN) RETURN n.k ORDER BY n.at ASC"),
        rangs(chronologie.len()),
        "les années à un, trois et quatre chiffres, plus une année négative, \
         doivent s'ordonner chronologiquement — l'ordre lexicographique des \
         écritures placerait « -0044-03-15 » en tête pour la mauvaise raison \
         et « 0999 » avant « 0001 » n'aurait aucun sens"
    );
}

// ===========================================================================
// G4 — Bornes d'intervalle tronquées
// ===========================================================================

/// Une borne basse TRONQUÉE à la seconde est plus ancienne — ou égale — à la
/// même borne écrite à neuf décimales. Elle ne peut donc qu'INCLURE au moins
/// autant de lignes. Jamais moins.
#[test]
fn typé_borne_tronquee_ne_perd_aucune_ligne() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();

    let jeu = [
        ("r0", "2026-01-01T09:59:59.999999Z"),
        ("r1", "2026-01-01T10:00:00Z"),
        ("r2", "2026-01-01T10:00:00.000001Z"),
        ("r3", "2026-01-01T10:00:00.5Z"),
        ("r4", "2026-01-01T10:00:00.999999Z"),
        ("r5", "2026-01-01T10:00:01Z"),
    ];
    exiger_precisions_melangees(&jeu.iter().map(|(_, e)| (*e).to_string()).collect::<Vec<_>>());
    for (k, e) in jeu {
        exec(&s, &format!("INSERT (:BO {{k:'{k}', at: DATETIME '{e}'}})"));
    }

    let tronquee = cles_triees(
        &s,
        "MATCH (n:BO) WHERE n.at >= DATETIME '2026-01-01T10:00:00Z' RETURN n.k",
    );
    let neuf_decimales = cles_triees(
        &s,
        "MATCH (n:BO) WHERE n.at >= DATETIME '2026-01-01T10:00:00.000000000Z' RETURN n.k",
    );

    assert_eq!(
        tronquee,
        vec!["r1", "r2", "r3", "r4", "r5"],
        "la borne tronquée à la seconde doit retenir tout ce qui est à ou après \
         10:00:00 — et seulement cela : r0 est 1 µs avant"
    );
    assert_eq!(
        tronquee, neuf_decimales,
        "« >= 10:00:00Z » et « >= 10:00:00.000000000Z » désignent le MÊME \
         instant : les deux bornes doivent rendre exactement les mêmes lignes. \
         Tronquée : {tronquee:?} ; neuf décimales : {neuf_decimales:?}"
    );

    // Et la borne haute, symétriquement : elle doit exclure la seconde suivante.
    assert_eq!(
        cles_triees(
            &s,
            "MATCH (n:BO) WHERE n.at >= DATETIME '2026-01-01T10:00:00Z' \
             AND n.at < DATETIME '2026-01-01T10:00:01Z' RETURN n.k"
        ),
        vec!["r1", "r2", "r3", "r4"],
        "l'intervalle [10:00:00, 10:00:01) doit contenir la seconde entière, \
         fractions comprises, et exclure r5 qui est PILE sur la borne haute"
    );
}

// ===========================================================================
// G5 — ORDER BY sur colonne à précisions mélangées
// ===========================================================================

/// `ORDER BY` sur une colonne dont les valeurs ont été écrites à 0, 1, 3, 6 et
/// 9 décimales rend l'ordre chronologique, et son inverse en DESC.
#[test]
fn typé_order_by_precisions_melangees() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();

    let chronologie = [
        ("r0", "2026-01-01T09:59:59.999999999Z"),
        ("r1", "2026-01-01T10:00:00Z"),
        ("r2", "2026-01-01T10:00:00.000001Z"),
        ("r3", "2026-01-01T10:00:00.123Z"),
        ("r4", "2026-01-01T10:00:00.5Z"),
        ("r5", "2026-01-01T10:00:00.999999Z"),
        ("r6", "2026-01-01T10:00:01Z"),
    ];
    exiger_precisions_melangees(
        &chronologie
            .iter()
            .map(|(_, e)| (*e).to_string())
            .collect::<Vec<_>>(),
    );
    // Insertion dans un ordre ni chronologique ni lexicographique.
    for i in [3usize, 0, 6, 1, 5, 2, 4] {
        let (k, e) = chronologie[i];
        exec(&s, &format!("INSERT (:OB {{k:'{k}', at: DATETIME '{e}'}})"));
    }

    let asc = cles(&s, "MATCH (n:OB) RETURN n.k ORDER BY n.at ASC");
    let desc = cles(&s, "MATCH (n:OB) RETURN n.k ORDER BY n.at DESC");
    assert_eq!(
        asc,
        rangs(chronologie.len()),
        "ASC doit rendre l'ordre chronologique sur une colonne à cinq \
         précisions différentes"
    );
    let mut inverse = desc.clone();
    inverse.reverse();
    assert_eq!(
        inverse, asc,
        "DESC doit être l'exact renversement de ASC — s'ils coïncident, c'est \
         que la clé de tri n'est pas comparée du tout. ASC : {asc:?} ; DESC : {desc:?}"
    );
}

// ===========================================================================
// G6 — Chaînes : la garantie qui TIENT
// ===========================================================================

/// **La garantie forte.** Pour un décalage NUMÉRIQUE fixe (`+02:00`), et
/// malgré des fractions rognées à des longueurs différentes par le code de
/// production, l'ordre lexicographique COÏNCIDE avec l'ordre chronologique.
///
/// Raison : quand les deux écritures ont la même seconde, le caractère qui
/// décide est `'.'` (0x2E) d'un côté contre le signe du décalage de l'autre —
/// `'+'` (0x2B) ou `'-'` (0x2D), tous deux INFÉRIEURS. Et une fraction plus
/// courte est un préfixe de la plus longue dont le caractère suivant est un
/// chiffre (≥ 0x30), lui aussi supérieur au signe.
///
/// C'est cette garantie, et elle seule, que le relevé antérieur
/// `revue2_forme_date_non_exacte` avait établie.
#[test]
fn chaines_offset_numerique_lordre_lexicographique_est_chronologique() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();

    let corpus = corpus_production(2 * 3600);
    assert!(
        corpus.iter().all(|(_, e)| e.ends_with("+02:00")),
        "le code de production doit rendre un décalage numérique ici : {corpus:?}"
    );

    assert_eq!(
        premiere_inversion(&corpus),
        None,
        "à décalage numérique fixe, AUCUNE paire ne doit inverser l'ordre : \
         l'ordre lexicographique est chronologique. Corpus : {corpus:?}"
    );

    // La même garantie doit se voir de bout en bout dans le moteur.
    charger_chaines(&s, "SP", &corpus);
    assert_eq!(
        cles(&s, "MATCH (n:SP) RETURN n.k ORDER BY n.at ASC"),
        rangs(corpus.len()),
        "trié comme des chaînes, le corpus à décalage numérique doit sortir \
         dans l'ordre chronologique"
    );

    // Et une borne tronquée à la seconde reste conservatrice dans le bon sens :
    // elle ne peut rien perdre par rapport à la borne à neuf décimales.
    let tronquee = cles_triees(
        &s,
        "MATCH (n:SP) WHERE n.at >= '2026-01-01T12:00:00+02:00' RETURN n.k",
    );
    let neuf = cles_triees(
        &s,
        "MATCH (n:SP) WHERE n.at >= '2026-01-01T12:00:00.000000000+02:00' RETURN n.k",
    );
    assert!(
        neuf.iter().all(|k| tronquee.contains(k)),
        "la borne tronquée doit être au moins aussi INCLUSIVE que celle à neuf \
         décimales. Tronquée : {tronquee:?} ; neuf décimales : {neuf:?}"
    );
    assert_eq!(
        tronquee,
        vec!["r1", "r2", "r3", "r4", "r5", "r6"],
        "la borne tronquée doit retenir l'instant d'ancrage et tout ce qui suit"
    );
}

// ===========================================================================
// G7 — Dates malformées
// ===========================================================================

/// Une date impossible ne doit jamais devenir un instant. Elle est rejetée, ou
/// à tout le moins elle n'est PAS un `Timestamp`.
#[test]
fn date_impossible_nest_jamais_un_instant() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();

    for ecriture in [
        "2026-13-45T99:99:99Z",  // mois 13, jour 45, heure 99
        "2026-02-30T10:00:00Z",  // 30 février
        "2026-01-01T24:00:00Z",  // heure 24
        "2026-01-01T10:00:60Z",  // seconde 60
        "pas-une-date",
        "2026-01-01T10:00:00.-1Z", // fraction négative
    ] {
        let v = scalaire(&s, &format!("DATETIME '{ecriture}'"));
        assert!(
            !matches!(v, Value::Timestamp(_)),
            "« {ecriture} » ne désigne aucun instant : le moteur ne doit pas \
             en fabriquer un. Obtenu : {v:?}"
        );
    }
}

// ===========================================================================
// DIVERGENCES — ce que la sémantique exige, et que le moteur ne rend pas.
// Ces tests échouent aujourd'hui. Les ajuster à l'observé figerait les défauts.
// ===========================================================================

/// **D6 — LE CAS DÉCISIF.** L'ordre lexicographique et l'ordre chronologique
/// DIVERGENT, et le jeu qui le prouve est produit intégralement par le code de
/// production : `ZonedDatetime::Display` rogne les zéros de fin de fraction ET
/// écrit `Z` pour l'offset zéro.
///
/// `'Z'` vaut 0x5A ; `'.'` vaut 0x2E. `…T10:00:00Z` se classe donc APRÈS
/// `…T10:00:00.000001Z`, alors qu'il lui est antérieur d'une microseconde.
///
/// C'est la limite exacte du relevé `revue2_forme_date_non_exacte` : sa
/// conclusion vaut pour `'+'` (0x2B) et `'-'` (0x2D), pas pour `'Z'`.
#[test]
// CORRIGÉ par ce commit (correctif H, vague 4a : ZonedDatetime::Display écrit +00:00 pour l'offset nul) — défaut d'origine : D6 — 'Z' (0x5A) se classait après '.' (0x2E), l'ordre lexicographique cessait d'être chronologique
fn divergence_le_z_casse_la_coincidence_lexicographique() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();

    let corpus = corpus_production(0);
    // PRÉCONDITION DE FIXTURE RÉÉCRITE (vague 4a, correctif H) — l'original
    // exigeait `ends_with('Z')` (« le code de production doit rendre le
    // désignateur Z ici ») : une précondition qui FIGEAIT l'observation du
    // défaut D6, contre le principe du fichier. Elle rendait le test
    // INSATISFIABLE : le rognage des zéros (exigé par
    // exiger_precisions_melangees — mutation M10) place dans la même seconde
    // une écriture SANS fraction et une à six décimales ; suffixées 'Z'
    // (0x5A > '.' 0x2E > chiffres), cette paire s'inverse TOUJOURS. Aucun
    // Display ne satisfait à la fois « Z », « ≥3 précisions » et « aucune
    // inversion » — hormis le rendu pathologique '.000000' pour la seule
    // fraction nulle, qui passerait CE corpus sans rétablir la garantie
    // générale ('.5Z' vs '.51Z' s'inverse encore). La correction de D6 est
    // donc l'écriture '+00:00' de l'offset nul (note 0007e004, preuve
    // exhaustive ; alignée production obrain-tmn, to_rfc3339 → '+00:00').
    // Seule cette précondition change ; les assertions d'INTENTION du test
    // sont intactes. À valider en revue adversariale (précédent : les 3
    // assertions gql_12 réécrites en vague 3).
    assert!(
        corpus.iter().all(|(_, e)| e.ends_with("+00:00")),
        "le code de production doit rendre l'offset nul en +00:00 ici : {corpus:?}"
    );

    assert_eq!(
        premiere_inversion(&corpus),
        None,
        "l'ordre lexicographique doit coïncider avec l'ordre chronologique. \
         Il ne le fait pas : le désignateur UTC 'Z' vaut 0x5A et se classe \
         APRÈS le point 0x2E ainsi qu'après tout chiffre, si bien qu'un instant \
         ANTÉRIEUR sans fraction se classe après un instant POSTÉRIEUR qui en \
         porte une. Corpus rendu par ZonedDatetime::Display : {corpus:?}"
    );

    charger_chaines(&s, "SZ", &corpus);
    assert_eq!(
        cles(&s, "MATCH (n:SZ) RETURN n.k ORDER BY n.at ASC"),
        rangs(corpus.len()),
        "trié comme des chaînes, le corpus en Z doit malgré tout sortir dans \
         l'ordre chronologique"
    );
}

/// **D6 bis — le couple minimal**, réduit à deux instants distants d'1 µs, et
/// la démonstration que seule l'écriture de l'offset décide.
#[test]
// CORRIGÉ par ce commit (correctif H, vague 4a : plongement temporel des chaînes datetime aux prédicats, value_cmp::comparer) — défaut d'origine : D6 — deux écritures du même couple d'instants donnaient des verdicts OPPOSÉS selon que l'offset zéro s'écrit 'Z' ou '+00:00'
fn divergence_le_verdict_depend_de_lecriture_de_loffset_zero() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();

    let en_z = scalaire(
        &s,
        "'2026-01-01T10:00:00Z' < '2026-01-01T10:00:00.000001Z'",
    );
    let en_plus = scalaire(
        &s,
        "'2026-01-01T10:00:00+00:00' < '2026-01-01T10:00:00.000001+00:00'",
    );

    assert_eq!(
        en_plus,
        Value::Bool(true),
        "10:00:00 précède 10:00:00.000001 — écrit avec +00:00, le moteur le dit"
    );
    assert_eq!(
        en_z, en_plus,
        "MÊME couple d'instants, MÊME écart d'une microseconde, SEULE change \
         l'orthographe du décalage nul — pourtant le verdict s'inverse. \
         En Z : {en_z:?} ; en +00:00 : {en_plus:?}. La garantie ne repose sur \
         rien de sémantique : elle repose sur le fait que '+' vaut 0x2B."
    );
}

/// **D7.** Une borne basse TRONQUÉE à la seconde désigne un instant antérieur
/// ou égal : elle ne peut qu'inclure au moins autant de lignes. Sur des dates
/// stockées en chaîne et écrites en `Z`, elle en EXCLUT — le mauvais sens.
#[test]
// CORRIGÉ par ce commit (correctif H, vague 4a : plongement temporel au balayage par intervalle, comparer_ordonnable + désistement de l'index ordonné sur borne datetime) — défaut d'origine : D7 — borne tronquée sur chaînes en Z, la comparaison n'était pas conservatrice
fn divergence_borne_tronquee_sur_chaines_exclut_des_lignes() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();

    let corpus = corpus_production(0);
    charger_chaines(&s, "BT", &corpus);

    let tronquee = cles_triees(&s, "MATCH (n:BT) WHERE n.at >= '2026-01-01T10:00:00Z' RETURN n.k");
    let neuf = cles_triees(
        &s,
        "MATCH (n:BT) WHERE n.at >= '2026-01-01T10:00:00.000000000Z' RETURN n.k",
    );

    let perdues: Vec<&String> = neuf.iter().filter(|k| !tronquee.contains(k)).collect();
    assert!(
        perdues.is_empty(),
        "une borne basse tronquée à la seconde désigne un instant ANTÉRIEUR ou \
         égal à la même borne écrite à neuf décimales : elle ne peut PERDRE \
         aucune ligne. Elle en perd {} : {perdues:?}. \
         Tronquée : {tronquee:?} ; neuf décimales : {neuf:?}",
        perdues.len()
    );
}

/// **D12.** Stockées en chaîne, deux écritures du même instant à des fuseaux
/// différents doivent se comparer égales, et un corpus à fuseaux mélangés doit
/// se trier chronologiquement.
#[test]
// CORRIGÉ par ce commit (correctif H, vague 4a : égalité par instant de deux chaînes datetime, value_cmp::egaler) — défaut d'origine : D12 — la comparaison lexicographique ne normalisait aucun décalage
fn divergence_chaines_fuseaux_melanges() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();

    assert_eq!(
        scalaire(&s, "'2026-01-01T10:00:00Z' = '2026-01-01T12:00:00+02:00'"),
        Value::Bool(true),
        "10:00:00Z et 12:00:00+02:00 sont le MÊME instant : ils doivent se \
         comparer égaux"
    );

    // Corpus produit par le code de production, à trois décalages, précisions
    // mélangées ; l'ordre chronologique est celui de r0 à r5.
    let instants = [
        (ANCRE - 1_000_000, 0),        // r0  09:59:59Z
        (ANCRE - 500_000, 2 * 3600),   // r1  11:59:59.5+02:00
        (ANCRE, -5 * 3600),            // r2  05:00:00-05:00
        (ANCRE + 1, 2 * 3600),         // r3  12:00:00.000001+02:00
        (ANCRE + 123_000, 0),          // r4  10:00:00.123Z
        (ANCRE + 1_000_000, -5 * 3600),// r5  05:00:01-05:00
    ];
    let corpus: Vec<(i64, String)> = instants
        .iter()
        .map(|&(m, off)| {
            (
                m,
                ZonedDatetime::from_timestamp_offset(Timestamp::from_micros(m), off).to_string(),
            )
        })
        .collect();
    exiger_precisions_melangees(&corpus.iter().map(|(_, e)| e.clone()).collect::<Vec<_>>());
    charger_chaines(&s, "FM", &corpus);

    assert_eq!(
        cles(&s, "MATCH (n:FM) RETURN n.k ORDER BY n.at ASC"),
        rangs(corpus.len()),
        "un corpus à fuseaux mélangés, trié comme des chaînes, doit sortir dans \
         l'ordre chronologique. Corpus : {corpus:?}"
    );
}

/// **D1.** L'égalité est réflexive : toute valeur est égale à elle-même. Sur
/// les types temporels, `=` est toujours faux et `<>` toujours vrai — y compris
/// une valeur contre elle-même — alors que `<=` sur les mêmes opérandes est vrai.
#[test]
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : D1 — divergence mesurée : values_equal (filter.rs:3413) n'a aucune branche temporelle
fn divergence_egalite_temporelle_nest_pas_reflexive() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();

    for litteral in [
        "DATETIME '2026-01-01T10:00:00Z'",
        "DATETIME '2026-01-01T10:00:00.000001Z'",
        "DATE '2026-01-01'",
        "TIME '10:00:00.5'",
        "ZONED DATETIME '2026-01-01T10:00:00Z'",
    ] {
        assert_eq!(
            scalaire(&s, &format!("{litteral} = {litteral}")),
            Value::Bool(true),
            "toute valeur est égale à elle-même : {litteral}"
        );
        assert_eq!(
            scalaire(&s, &format!("{litteral} <> {litteral}")),
            Value::Bool(false),
            "aucune valeur n'est différente d'elle-même : {litteral}"
        );
        // Le témoin de cohérence interne : `<=` est vrai sur ce même couple.
        // `a <= a` vrai ET `a = a` faux ET `a <> a` vrai est une contradiction.
        assert_eq!(
            scalaire(&s, &format!("{litteral} <= {litteral}")),
            Value::Bool(true),
            "cohérence : `a <= a` doit être vrai — s'il l'est pendant que \
             `a = a` est faux, deux chemins de comparaison se contredisent : {litteral}"
        );
    }

    // Et sur une valeur STOCKÉE, comparée à elle-même.
    exec(
        &s,
        "INSERT (:RE {k:'x', at: DATETIME '2026-01-01T10:00:00.5Z'})",
    );
    assert_eq!(
        cles(&s, "MATCH (n:RE) WHERE n.at = n.at RETURN n.k"),
        vec!["x".to_string()],
        "une ligne dont l'horodatage n'est pas égal à lui-même est introuvable \
         par toute jointure ou tout regroupement sur cette colonne"
    );
}

/// **D2.** Le même prédicat doit rendre la même réponse, qu'il soit poussé dans
/// le scan (`WHERE`) ou évalué comme expression (`RETURN`).
#[test]
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : D2 — divergence mesurée : factorized_filter.rs:170 traite Timestamp, filter.rs:3413 non
fn divergence_meme_predicat_deux_reponses_selon_le_chemin() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();

    exec(
        &s,
        "INSERT (:CH {k:'x', at: DATETIME '2026-01-01T10:00:00.000001Z'})",
    );
    let predicat = "n.at = DATETIME '2026-01-01T10:00:00.000001Z'";

    let par_where = cles(&s, &format!("MATCH (n:CH) WHERE {predicat} RETURN n.k"));
    let par_projection = exec(&s, &format!("MATCH (n:CH) RETURN {predicat} AS eq"));

    assert_eq!(
        par_where,
        vec!["x".to_string()],
        "le prédicat filtre bien la ligne"
    );
    assert_eq!(
        par_projection[0][0],
        Value::Bool(true),
        "le MÊME prédicat, projeté au lieu d'être filtré, doit rendre vrai. \
         Il rend {:?} pendant que le WHERE retient la ligne : deux chemins \
         d'évaluation se contredisent sur les types temporels.",
        par_projection[0][0]
    );
}

/// **D3.** `ORDER BY` sur `ZONED DATETIME` doit ordonner par instant. Il est
/// inerte : ASC et DESC rendent la même séquence, l'ordre d'insertion.
#[test]
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : D3 — divergence mesurée : compare_values_total (value_utils.rs:57) n'a pas de branche ZonedDatetime
fn divergence_order_by_zoned_datetime_est_inerte() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();

    // Précisions ET fuseaux mélangés ; ordre chronologique = r0..r3.
    let chronologie = [
        ("r0", "2026-01-01T01:00:00Z"),
        ("r1", "2026-01-01T12:00:00.5+02:00"), // 10:00:00.5 UTC
        ("r2", "2026-01-01T12:00:00.000001Z"),
        ("r3", "2026-01-01T23:00:00+00:00"),
    ];
    exiger_precisions_melangees(
        &chronologie
            .iter()
            .map(|(_, e)| (*e).to_string())
            .collect::<Vec<_>>(),
    );
    // Inséré à l'envers : si le tri est inerte, ASC rendra r3..r0.
    for (k, e) in chronologie.iter().rev() {
        exec(&s, &format!("INSERT (:ZO {{k:'{k}', at: ZONED DATETIME '{e}'}})"));
    }

    let asc = cles(&s, "MATCH (n:ZO) RETURN n.k ORDER BY n.at ASC");
    let desc = cles(&s, "MATCH (n:ZO) RETURN n.k ORDER BY n.at DESC");
    assert_ne!(
        asc, desc,
        "ASC et DESC ne peuvent pas rendre la même séquence sur quatre instants \
         DISTINCTS : si c'est le cas, la clé de tri n'est jamais comparée. \
         Séquence commune : {asc:?}"
    );
    assert_eq!(
        asc,
        rangs(chronologie.len()),
        "ORDER BY sur ZONED DATETIME doit ordonner par INSTANT, décalages \
         normalisés — pas laisser passer l'ordre d'insertion"
    );
}

/// **D4.** `min()`/`max()` sur une colonne `ZONED DATETIME` doivent rendre des
/// instants. Ils rendent `Int64(0)`.
///
/// Même chemin `value_utils.rs` que la divergence D4 du lot gql-02 (relevée là
/// sur les colonnes CHAÎNE) ; consigné ici parce qu'il touche un type TEMPOREL,
/// et qu'un entier nul rendu pour une date est un résultat silencieusement faux.
#[test]
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : D4 — divergence mesurée : min/max sur ZONED DATETIME rendent Int64(0)
fn divergence_min_max_zoned_datetime_rend_un_entier() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();

    let jeu = [
        "2026-01-01T12:00:00.5+02:00", // 10:00:00.5 UTC — le minimum
        "2026-01-01T12:00:00.000001Z",
        "2026-01-01T23:00:00+00:00", // le maximum
    ];
    exiger_precisions_melangees(&jeu.iter().map(|e| (*e).to_string()).collect::<Vec<_>>());
    for (i, e) in jeu.iter().enumerate() {
        exec(&s, &format!("INSERT (:MM {{k:'r{i}', at: ZONED DATETIME '{e}'}})"));
    }

    let rows = exec(&s, "MATCH (n:MM) RETURN min(n.at) AS mn, max(n.at) AS mx");
    let (mn, mx) = (&rows[0][0], &rows[0][1]);
    assert!(
        matches!(mn, Value::ZonedDatetime(_)) && matches!(mx, Value::ZonedDatetime(_)),
        "min/max d'une colonne de dates doivent être des DATES. Obtenus : \
         min={mn:?}, max={mx:?}"
    );
    let attendu_mn = ZonedDatetime::parse(jeu[0]).expect("écriture de fixture valide");
    let attendu_mx = ZonedDatetime::parse(jeu[2]).expect("écriture de fixture valide");
    assert_eq!(mn, &Value::ZonedDatetime(attendu_mn), "min = l'instant le plus ancien");
    assert_eq!(mx, &Value::ZonedDatetime(attendu_mx), "max = l'instant le plus récent");
}

/// **D5.** Une colonne mélangeant horodatages typés et dates en chaîne doit
/// rester triée — au minimum, les valeurs comparables entre elles doivent
/// l'être. Le tri rend l'ordre d'insertion, les deux horodatages compris.
#[test]
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : D5 — divergence mesurée : compare_values_total rend Equal pour les paires croisées, le comparateur cesse d'être transitif
fn divergence_colonne_mixte_horodatage_et_chaine_nest_pas_triee() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();

    // r0 < r1 < r2 chronologiquement ; r1 est une CHAÎNE, r0 et r2 des instants.
    exec(&s, "INSERT (:MI {k:'r2', at: DATETIME '2026-01-01T10:00:00.5Z'})");
    exec(&s, "INSERT (:MI {k:'r1', at: '2026-01-01T09:00:00Z'})");
    exec(&s, "INSERT (:MI {k:'r0', at: DATETIME '2026-01-01T08:00:00.000001Z'})");

    let asc = cles(&s, "MATCH (n:MI) RETURN n.k ORDER BY n.at ASC");
    // Contrainte minimale : les deux valeurs de MÊME type doivent être ordonnées
    // entre elles, quoi que le moteur décide de la chaîne intercalée.
    let pos = |k: &str| asc.iter().position(|x| x == k);
    assert!(
        pos("r0") < pos("r2"),
        "r0 (08:00:00.000001) précède r2 (10:00:00.5) : deux horodatages du \
         MÊME type doivent être ordonnés entre eux même si une chaîne s'est \
         glissée dans la colonne. Ordre obtenu : {asc:?}"
    );
    assert_eq!(
        asc,
        vec!["r0", "r1", "r2"],
        "l'intention complète : la colonne doit s'ordonner chronologiquement. \
         Ordre obtenu : {asc:?}"
    );
}

/// **D8.** Deux instants distants d'une nanoseconde sont DISTINCTS. Le moteur
/// les confond : `Timestamp` compte en microsecondes, et `Time::parse` tronque.
/// Une date RFC 3339 à neuf décimales — la forme même qu'écrit
/// `to_rfc3339_opts(Nanos, …)` — perd donc ses trois derniers chiffres.
#[test]
// CORRIGÉ par ce commit (correctif H, vague 4a : Timestamp compte en NANOsecondes, from_date_time ne divise plus la fraction par 1000 ; API microsecondes conservée, plage 1677-2262 saturée) — défaut d origine : D8 — Timestamp était en microsecondes, les nanosecondes étaient tronquées sans avertissement
fn divergence_nanosecondes_tronquees_deux_instants_deviennent_egaux() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();

    let jeu = [
        ("r0", "2026-01-01T10:00:00Z"),
        ("r1", "2026-01-01T10:00:00.000000001Z"), // 1 ns après r0
        ("r2", "2026-01-01T10:00:00.000000999Z"),
        ("r3", "2026-01-01T10:00:00.000001Z"),
    ];
    exiger_precisions_melangees(&jeu.iter().map(|(_, e)| (*e).to_string()).collect::<Vec<_>>());
    for (k, e) in jeu {
        exec(&s, &format!("INSERT (:NS {{k:'{k}', at: DATETIME '{e}'}})"));
    }

    assert_eq!(
        cles_triees(
            &s,
            "MATCH (n:NS) WHERE n.at > DATETIME '2026-01-01T10:00:00Z' RETURN n.k"
        ),
        vec!["r1", "r2", "r3"],
        "r1, r2 et r3 sont STRICTEMENT postérieurs à r0 : une nanoseconde est \
         un écart, pas un arrondi. Les confondre rend `>` et `>=` \
         indiscernables sur toute la classe des dates à neuf décimales — \
         celles qu'écrit to_rfc3339_opts(Nanos, …)"
    );
    assert_eq!(
        cles(&s, "MATCH (n:NS) RETURN n.k ORDER BY n.at ASC"),
        rangs(4),
        "les quatre instants sont deux à deux distincts et doivent s'ordonner"
    );
}

/// **D9.** Des écritures MALFORMÉES sont acceptées comme instants réels : la
/// cause est que `str::parse::<u32>` accepte un `+` de tête et que les champs
/// ne sont pas contraints à des chiffres.
#[test]
// CORRIGÉ par ce commit (correctif H, vague 4a : champs RFC3339 en chiffres stricts — champ_numerique dans time.rs/date.rs — et secondes obligatoires) — défaut d'origine : D9 — time.rs et date.rs déléguaient à str::parse, qui accepte un signe de tête
fn divergence_ecritures_malformees_acceptees_comme_instants() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();

    // Aucune de ces écritures n'est du RFC 3339.
    for (ecriture, pourquoi) in [
        ("2026-01-01T10:00:00.+5Z", "une fraction ne commence pas par un signe"),
        ("2026-01-01T+10:00:00Z", "un champ heure ne commence pas par un signe"),
        ("+2026-01-01T10:00:00Z", "un millésime ne commence pas par un signe"),
        ("2026-01-01T10:00Z", "les secondes ne sont pas facultatives en RFC 3339"),
    ] {
        let v = scalaire(&s, &format!("DATETIME '{ecriture}'"));
        assert!(
            !matches!(v, Value::Timestamp(_)),
            "« {ecriture} » n'est pas une date : {pourquoi}. Le moteur en \
             fabrique pourtant un instant : {v:?}"
        );
    }
    for (ecriture, pourquoi) in [
        ("+2026-01-01", "un millésime ne commence pas par un signe"),
        ("2026-+1-01", "un mois ne commence pas par un signe"),
    ] {
        let v = scalaire(&s, &format!("DATE '{ecriture}'"));
        assert!(
            !matches!(v, Value::Date(_)),
            "« {ecriture} » n'est pas une date : {pourquoi}. Obtenu : {v:?}"
        );
    }
}

/// **D10.** Une date illisible doit être REJETÉE, ou au moins signalée de façon
/// uniforme. Le littéral typé la dégrade silencieusement en chaîne — qui se
/// compare ensuite faux contre tout instant — pendant que la fonction
/// équivalente rend `NULL` : deux régimes d'échec pour la même faute.
#[test]
// CORRIGÉ par ce commit (correctif H, vague 4a : le littéral DATETIME illisible rend NULL, comme toDatetime — un seul régime d'échec) — défaut d'origine : D10 — DATETIME 'illisible' dégradait en String sans erreur pendant que toDatetime rendait NULL
fn divergence_date_illisible_degradee_en_chaine_sans_erreur() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();

    let illisible = "2026-13-45T99:99:99Z";

    let par_litteral = scalaire(&s, &format!("DATETIME '{illisible}'"));
    let par_fonction = scalaire(&s, &format!("toDatetime('{illisible}')"));
    assert_eq!(
        par_litteral, par_fonction,
        "la même écriture illisible doit échouer de la même façon par le \
         littéral typé et par la fonction. Littéral : {par_litteral:?} ; \
         fonction : {par_fonction:?}"
    );

    // Et la conséquence : dégradée en chaîne, elle traverse tous les filtres
    // temporels sans jamais lever d'alerte.
    exec(&s, &format!("INSERT (:IL {{k:'illisible', at: DATETIME '{illisible}'}})"));
    exec(&s, "INSERT (:IL {k:'bon', at: DATETIME '2026-01-01T10:00:00.5Z'})");
    let hors_borne = cles_triees(
        &s,
        "MATCH (n:IL) WHERE n.at >= DATETIME '1970-01-01T00:00:00Z' RETURN n.k",
    );
    assert!(
        hors_borne.contains(&"illisible".to_string()) || hors_borne.len() == 1,
        "soit la ligne illisible n'existe pas (rejet à l'écriture), soit elle \
         participe aux comparaisons ; ce qu'elle ne doit pas faire, c'est \
         exister ET disparaître silencieusement de tout filtre temporel. \
         Lignes retenues : {hors_borne:?}"
    );
}

/// **D11.** RFC 3339 §4.2 permet explicitement les minuscules `t` et `z`. Le
/// moteur accepte `t` mais refuse `z`.
#[test]
// CORRIGÉ par ce commit (correctif H, vague 4a : parse_offset_suffix accepte 'z', le littéral DATETIME accepte 't' — RFC 3339 §4.2) — défaut d'origine : D11 — parse_offset_suffix ne testait que 'Z' majuscule
fn divergence_minuscules_rfc3339_refusees() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();

    let majuscules = scalaire(&s, "DATETIME '2026-01-01T10:00:00.5Z'");
    let minuscules = scalaire(&s, "DATETIME '2026-01-01t10:00:00.5z'");
    assert_eq!(
        minuscules, majuscules,
        "RFC 3339 §4.2 permet 't' et 'z' en minuscules : les deux écritures \
         désignent le même instant. Majuscules : {majuscules:?} ; \
         minuscules : {minuscules:?}"
    );
}
