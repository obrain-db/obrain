//! Lot 05 — CHAÎNES. Conformité GQL (ISO/IEC 39075) du traitement des
//! chaînes de caractères : égalité, ordre lexicographique, `CONTAINS` /
//! `STARTS WITH` / `ENDS WITH`, `LIKE`, casse, chaîne vide, unicode,
//! échappements, concaténation, fonctions de chaînes, chaînes très longues.
//!
//! FRONTIÈRES — le lot 02 couvre les opérateurs de comparaison génériques,
//! le lot 09 le tri, le lot 12 le système de types. Ici : ce qui est
//! spécifique aux CHAÎNES.
//!
//! ── LES DEUX CHEMINS, ET POURQUOI CHAQUE TEST D'ORDRE EST DOUBLÉ ──
//!
//! Une comparaison `n.prop <op> littéral` sur un `MATCH (n:L)` simple NE
//! passe PAS par l'évaluateur d'expressions. Le planificateur la détourne
//! vers un balayage par intervalle :
//!
//!   plan_filter                      engine/src/query/planner/lpg/filter.rs:96
//!     └─ try_plan_filter_with_range_index → plan_range_filter        (l. 1932)
//!          └─ store.find_nodes_in_range        substrate/src/store.rs:10765
//!               └─ valeur_dans_intervalle      substrate/src/store.rs:9067
//!                    └─ OrderableValue::cmp    common/src/types/value.rs:799
//!
//! La MÊME comparaison écrite en PROJECTION (`RETURN n.prop <op> …`) passe,
//! elle, par l'évaluateur :
//!
//!   ExpressionPredicate::compare_values
//!                       core/src/execution/operators/filter.rs:3527
//!
//! Deux comparateurs, deux tables de vérité. Sur chaîne-contre-chaîne ils
//! s'accordent (point SAIN, verrouillé ici). Sur chaîne-contre-nombre ils
//! se contredisent — c'est la frontière que ce lot établit.
//!
//! De même l'ÉGALITÉ a deux chemins :
//!   · avec label   → values_equal_coerced   engine/…/planner/lpg/filter.rs:15
//!   · sans label   → values_equal (évaluateur)  core/…/operators/filter.rs
//!
//! ── DÉFAUTS CONNUS ──
//!
//! Un test dont l'assertion est CORRECTE mais que le moteur ne tient pas
//! aujourd'hui porte `#[cfg(feature = "defauts-connus")]` et un
//! commentaire `// DÉFAUT:` nommant la cause en fichier:ligne.
//! `cargo test -p obrain-engine --test gql_05_chaines` est donc vert ;
//! `cargo test -p obrain-engine --test gql_05_chaines --features
//! defauts-connus` fait tourner les défauts, qui ÉCHOUENT — c'est la
//! liste vivante. L'assertion reste écrite depuis la spec, jamais depuis
//! l'observation.
//!
//! ── PREUVE PAR MUTATION (2026-08-24, 17 mutations) ──
//!
//! 16 tuées, 1 survivante. Les trois comparateurs de chaînes ont été
//! mutés séparément et ont tué TROIS ensembles DISTINCTS de tests, ce
//! qui établit qu'aucun d'eux n'est couvert par les assertions d'un
//! autre :
//!   · OrderableValue::cmp (balayage par intervalle) → tue les 4 tests
//!     à `WHERE n.nom <op> 'litteral'` ;
//!   · ExpressionPredicate::compare_values (évaluateur) → tue les 4
//!     tests à comparaison en projection ;
//!   · compare_values_total (tri) → tue les 4 tests à `ORDER BY`.
//! Idem pour l'égalité : values_equal (sans label) et
//! values_equal_coerced (avec label) tuent des ensembles différents.
//! Les mutations qui se contentent d'INVERSER un ordre laissent vertes
//! les assertions de PROPRIÉTÉ (transitivité, inverse exact) — c'est
//! attendu, un ordre inversé reste un ordre ; ces assertions-là sont
//! tuées par la mutation qui APLATIT l'ordre (tout égal).
//! Unique survivante : la mutation de `size()` — son seul test est
//! derrière `defauts-connus`, donc absent de la suite par défaut. Les
//! trois RÉPARATIONS essayées (size en caractères, normalize en vraie
//! NFC, CHARACTER_LENGTH reconnu) font chacune passer AU VERT le test
//! de défaut correspondant, et lui seul : ces tests mesurent bien le
//! site qu'ils nomment.
//!
//! ── SIGNALEMENT (lot conformité, ne pas corriger ici) ──
//! `crates/obrain-engine/tests/gql_spec_compliance.rs` :
//!   · l. 1531 `test_aggregate_in_order_by` et l. 1548
//!     `test_aggregate_order_by_alias` sont JUMEAUX mot pour mot — même
//!     requête, même unique assertion `assert!(!result.rows.is_empty())`.
//!     Deux tests, une seule mesure, et cette mesure ne vérifie pas l'ordre.
//!   · l. 214 `test_like_no_match` n'assert que `rows.is_empty()`.

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

// ───────────────────────────── outillage ─────────────────────────────

/// Une base en mémoire portant un nœud `:S {nom: …}` par entrée.
///
/// Semis DIRECT (et non `INSERT`) : c'est la seule voie pour les chaînes
/// que GQL ne sait pas écrire littéralement — octet nul, très longues,
/// formes normalisées explicites.
fn db_avec(noms: &[&str]) -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for n in noms {
        s.create_node_with_props(&["S"], [("nom", Value::String((*n).into()))]);
    }
    db
}

/// Une base à un seul nœud, pour évaluer des expressions littérales.
fn db_unaire() -> ObrainDB {
    db_avec(&["x"])
}

fn lignes(db: &ObrainDB, q: &str) -> Vec<Vec<Value>> {
    db.session()
        .execute(q)
        .unwrap_or_else(|e| panic!("requête refusée : {q}\n{e}"))
        .rows
}

/// Première colonne de la première ligne.
fn valeur(db: &ObrainDB, q: &str) -> Value {
    let r = lignes(db, q);
    assert!(!r.is_empty(), "aucune ligne pour : {q}");
    r[0][0].clone()
}

/// Première colonne, en texte, dans l'ordre rendu.
fn textes(db: &ObrainDB, q: &str) -> Vec<String> {
    lignes(db, q)
        .into_iter()
        .map(|l| match &l[0] {
            Value::String(s) => s.to_string(),
            autre => panic!("valeur non-chaîne : {autre:?}"),
        })
        .collect()
}

const VRAI: Value = Value::Bool(true);
const FAUX: Value = Value::Bool(false);

/// Antislash construit par POINT DE CODE. Le source de ce fichier ne
/// contient aucun antislash littéral dans les requêtes d'échappement :
/// un transport de source qui décoderait `\uXXXX` fausserait le test
/// sans le faire rougir.
fn as_() -> char {
    char::from_u32(92).expect("92 est un point de code valide")
}

/// Un jeu ASCII sans apostrophe, sûr à interpoler dans une requête.
const JEU: [&str; 12] = [
    "10", "9", "2", "", "a", "A", "ab", "abc", "b", "Z", "z", "+",
];

// ═══════════════════ GROUPE 1 — ÉGALITÉ DE CHAÎNES ═══════════════════

#[test]
fn egalite_de_chaines_est_sensible_a_la_casse() {
    // ISO/IEC 39075 : l'égalité de chaînes est une comparaison de valeurs,
    // sans repliement de casse. 'alpha' et 'ALPHA' sont deux valeurs.
    let db = db_avec(&["alpha", "ALPHA", "Alpha"]);

    // chemin ÉVALUATEUR (projection)
    assert_eq!(
        lignes(
            &db,
            "MATCH (n:S) RETURN n.nom, n.nom = 'alpha' AS eq ORDER BY n.nom"
        )
        .iter()
        .filter(|l| l[1] == VRAI)
        .count(),
        1,
        "un seul des trois nœuds est égal à 'alpha'"
    );

    // chemin PLANIFICATEUR (WHERE avec label → values_equal_coerced)
    assert_eq!(
        textes(&db, "MATCH (n:S) WHERE n.nom = 'alpha' RETURN n.nom"),
        ["alpha"]
    );
    assert_eq!(
        textes(&db, "MATCH (n:S) WHERE n.nom = 'ALPHA' RETURN n.nom"),
        ["ALPHA"]
    );
}

#[test]
fn egalite_de_la_chaine_vide() {
    // La chaîne vide est une valeur comme une autre : elle est égale à
    // elle-même et se retrouve par filtre comme par motif.
    let db = db_avec(&["", "a"]);
    assert_eq!(valeur(&db, "MATCH (n:S) RETURN '' = '' AS r LIMIT 1"), VRAI);
    assert_eq!(lignes(&db, "MATCH (n:S {nom: ''}) RETURN n.nom").len(), 1);
    assert_eq!(
        lignes(&db, "MATCH (n:S) WHERE n.nom = '' RETURN n.nom").len(),
        1
    );
    assert_eq!(
        lignes(&db, "MATCH (n) WHERE n.nom = '' RETURN n.nom").len(),
        1
    );
}

#[test]
fn egalite_ne_normalise_pas_unicode() {
    // GQL ne fait AUCUNE normalisation implicite : 'café' précomposé
    // (U+00E9) et 'café' décomposé (e + U+0301) sont deux chaînes
    // distinctes. C'est précisément pourquoi la norme offre
    // `IS [NFC|NFD] NORMALIZED` — il n'y aurait rien à tester sinon.
    let precompose = "caf\u{e9}";
    let decompose = "cafe\u{301}";
    let db = db_avec(&[precompose, decompose]);

    assert_eq!(
        lignes(&db, "MATCH (n:S) WHERE n.nom = 'caf\u{e9}' RETURN n.nom").len(),
        1
    );
    assert_eq!(
        lignes(&db, "MATCH (n:S) WHERE n.nom = 'cafe\u{301}' RETURN n.nom").len(),
        1
    );
    assert_eq!(lignes(&db, "MATCH (n:S) RETURN n.nom").len(), 2);
}

// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : `n.nom = 5` est FAUX avec un label et VRAI sans : deux égalités, deux vérités
#[test]
fn egalite_chaine_contre_nombre_ne_depend_pas_de_la_presence_dun_label() {
    // INTENTION : l'égalité est une relation entre deux valeurs. Le fait
    // qu'un motif porte ou non un label est une information de PLAN, pas
    // de sémantique : `MATCH (n:S) WHERE n.nom = 5` et
    // `MATCH (n) WHERE n.nom = 5` doivent rendre le même verdict.
    //
    // Et ce verdict est FAUX : le lot 02 a établi que `'5.0' = 5` est
    // faux — l'égalité ne coerce pas. La chaîne '5' n'est donc pas égale
    // au nombre 5.
    //
    // DÉFAUT: deux implémentations de l'égalité.
    //   · avec label → values_equal_coerced, engine/src/query/planner/lpg/filter.rs:15
    //     (`_ => a == b` : String vs Int64 → faux)
    //   · sans label → values_equal de l'évaluateur,
    //     core/src/execution/operators/filter.rs:3413, dont le bras RDF
    //     l. 3424-3426 `(String, Int64) => s.parse::<i64>().is_ok_and(...)`
    //     rend VRAI pour '5' = 5.
    // MESURE : `MATCH (n:S) WHERE n.nom = 5` → 0 ligne ;
    //          `MATCH (n)   WHERE n.nom = 5` → 1 ligne ('5').
    let db = db_avec(&["5", "5.0", "alpha"]);
    let avec_label = lignes(&db, "MATCH (n:S) WHERE n.nom = 5 RETURN n.nom");
    let sans_label = lignes(&db, "MATCH (n) WHERE n.nom = 5 RETURN n.nom");
    assert_eq!(
        avec_label.len(),
        sans_label.len(),
        "le label ne doit pas changer le verdict d'une égalité"
    );
    assert!(
        avec_label.is_empty(),
        "'5' n'est pas égal au nombre 5 : l'égalité ne coerce pas"
    );
}

// ═══════════════ GROUPE 2 — ORDRE LEXICOGRAPHIQUE ════════════════════

#[test]
fn ordre_est_lexicographique_sans_derive_numerique_sur_les_deux_chemins() {
    // POINT SAIN, à verrouiller. '10' < '9' parce que '1' < '9' : une
    // chaîne de chiffres reste une chaîne. Les DEUX chemins doivent le
    // dire — celui du balayage par intervalle (WHERE) et celui de
    // l'évaluateur (projection).
    let db = db_avec(&["10", "9", "2", "1e5"]);

    // chemin PLANIFICATEUR — plan_range_filter
    let mut avant_9 = textes(
        &db,
        "MATCH (n:S) WHERE n.nom < '9' RETURN n.nom ORDER BY n.nom",
    );
    avant_9.sort();
    assert_eq!(
        avant_9,
        ["10", "1e5", "2"],
        "ordre par caractères, pas par valeur numérique"
    );

    // chemin ÉVALUATEUR — compare_values
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN '10' < '9' AS r LIMIT 1"),
        VRAI
    );
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN '1e5' < '2' AS r LIMIT 1"),
        VRAI
    );
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN '9' < '10' AS r LIMIT 1"),
        FAUX
    );
    // pas de dérive scientifique : '1e5' n'est pas 100000
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN '1e5' < '99' AS r LIMIT 1"),
        VRAI
    );
}

#[test]
fn un_prefixe_precede_ce_quil_prefixe() {
    let db = db_unaire();
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN 'abc' < 'abcd' AS r LIMIT 1"),
        VRAI
    );
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN '' < 'a' AS r LIMIT 1"),
        VRAI
    );
    assert_eq!(valeur(&db, "MATCH (n:S) RETURN '' < '' AS r LIMIT 1"), FAUX);
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN 'abcd' < 'abc' AS r LIMIT 1"),
        FAUX
    );
}

#[test]
fn ordre_est_total_transitif_et_antisymetrique() {
    // La spec ne fixe pas la collation. Elle exige en revanche que `<`
    // soit un ORDRE : irréflexif, antisymétrique, transitif, total sur
    // les chaînes. On le vérifie sur toutes les paires en une requête —
    // ce qui exerce aussi la comparaison colonne-contre-colonne.
    let db = db_avec(&JEU);
    let paires = lignes(
        &db,
        "MATCH (a:S), (b:S) RETURN a.nom AS x, b.nom AS y, a.nom < b.nom AS lt",
    );
    let lt = |x: &str, y: &str| -> bool {
        paires
            .iter()
            .find(|l| l[0] == Value::String(x.into()) && l[1] == Value::String(y.into()))
            .map(|l| l[2] == VRAI)
            .unwrap_or_else(|| panic!("paire absente : {x:?} {y:?}"))
    };

    for x in JEU {
        assert!(!lt(x, x), "irréflexivité violée sur {x:?}");
        for y in JEU {
            if x != y {
                assert!(
                    lt(x, y) ^ lt(y, x),
                    "antisymétrie/totalité violées sur {x:?} et {y:?}"
                );
            }
            for z in JEU {
                if lt(x, y) && lt(y, z) {
                    assert!(lt(x, z), "transitivité violée : {x:?} < {y:?} < {z:?}");
                }
            }
        }
    }
}

#[test]
fn where_inferieur_et_order_by_designent_le_meme_ordre() {
    // COHÉRENCE ENTRE LES DEUX CHEMINS. `ORDER BY` trie par l'opérateur
    // de tri ; `WHERE n.nom < 'k'` par le balayage par intervalle du
    // planificateur. L'ensemble sélectionné par le second doit être
    // exactement le préfixe désigné par le premier.
    let db = db_avec(&JEU);
    let ordre = textes(&db, "MATCH (n:S) RETURN n.nom ORDER BY n.nom");
    assert_eq!(ordre.len(), JEU.len());

    for (i, borne) in ordre.iter().enumerate() {
        let mut avant = textes(
            &db,
            &format!("MATCH (n:S) WHERE n.nom < '{borne}' RETURN n.nom"),
        );
        avant.sort();
        let mut attendu: Vec<String> = ordre[..i].to_vec();
        attendu.sort();
        assert_eq!(
            avant, attendu,
            "`< {borne:?}` doit sélectionner exactement le préfixe de ORDER BY"
        );
    }
}

#[test]
fn ordre_descendant_est_l_inverse_exact_de_l_ordre_ascendant() {
    let db = db_avec(&JEU);
    let asc = textes(&db, "MATCH (n:S) RETURN n.nom ORDER BY n.nom");
    let desc = textes(&db, "MATCH (n:S) RETURN n.nom ORDER BY n.nom DESC");
    let mut inverse = asc.clone();
    inverse.reverse();
    assert_eq!(desc, inverse);
}

#[test]
fn ordre_est_stable_dun_appel_a_lautre() {
    // La collation n'est pas fixée par la norme ; sa STABILITÉ, si.
    let db = db_avec(&["b", "a", "B", "A", "0", "\u{e9}", "e\u{301}", "z"]);
    let reference = textes(&db, "MATCH (n:S) RETURN n.nom ORDER BY n.nom");
    for essai in 0..5 {
        assert_eq!(
            textes(&db, "MATCH (n:S) RETURN n.nom ORDER BY n.nom"),
            reference,
            "l'ordre a changé à l'appel {essai}"
        );
    }
}

#[test]
fn ordre_suit_les_points_de_code_y_compris_hors_du_plan_multilingue_de_base() {
    // Sans collation déclarée, l'ordre est celui des points de code. Le
    // piège classique est l'ordre UTF-16 : U+FFFF y passerait APRÈS les
    // caractères hors BMP, qui s'y écrivent en paires de substituts
    // (0xD800-0xDFFF < 0xFFFF). Ici l'ordre doit être celui des points
    // de code — U+FFFF avant U+10000.
    let db = db_avec(&[
        "\u{7f}",
        "\u{80}",
        "\u{7ff}",
        "\u{800}",
        "\u{ffff}",
        "\u{10000}",
        "\u{1f600}",
        "\u{10ffff}",
    ]);
    assert_eq!(
        textes(&db, "MATCH (n:S) RETURN n.nom ORDER BY n.nom"),
        [
            "\u{7f}",
            "\u{80}",
            "\u{7ff}",
            "\u{800}",
            "\u{ffff}",
            "\u{10000}",
            "\u{1f600}",
            "\u{10ffff}"
        ],
        "ordre par points de code, pas par unités UTF-16"
    );
    // et le balayage par intervalle dit la même chose
    assert_eq!(
        lignes(&db, "MATCH (n:S) WHERE n.nom < '\u{10000}' RETURN n.nom").len(),
        5
    );
    assert_eq!(
        lignes(&db, "MATCH (n:S) WHERE n.nom > '\u{ffff}' RETURN n.nom").len(),
        3
    );
}

#[test]
fn ordre_mele_z_et_decalages_horaires_reste_lexicographique() {
    // Fait mesuré par le lot 02, verrouillé ici parce qu'il est un piège
    // de production : '+' (0x2B) < '.' (0x2E) < '0' (0x30) < 'Z' (0x5A).
    // Un horodatage suffixé 'Z' se range donc APRÈS un horodatage suffixé
    // '+01:00' à préfixe égal — l'ordre chronologique est cassé si la
    // colonne mêle les deux écritures.
    let db = db_unaire();
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN '+' < '.' AS r LIMIT 1"),
        VRAI
    );
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN '.' < '0' AS r LIMIT 1"),
        VRAI
    );
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN '0' < 'Z' AS r LIMIT 1"),
        VRAI
    );
    let db2 = db_avec(&["2026-01-01T00:00:00+01:00", "2026-01-01T00:00:00Z"]);
    assert_eq!(
        textes(&db2, "MATCH (n:S) RETURN n.nom ORDER BY n.nom")[0],
        "2026-01-01T00:00:00+01:00"
    );
}

// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : `WHERE n.nom < 9` rend 0 ligne alors que `RETURN n.nom < 9` rend VRAI sur les mêmes lignes
#[test]
fn comparaison_chaine_contre_nombre_rend_le_meme_verdict_dans_where_et_en_projection() {
    // INTENTION : `WHERE e` sélectionne exactement les lignes où `e`
    // s'évalue à VRAI. C'est la définition de WHERE, quel que soit le
    // plan choisi. Le filtre et la projection ne peuvent pas diverger.
    //
    // DÉFAUT: le balayage par intervalle ne compare pas les valeurs, il
    // compare les TYPES.
    //   OrderableValue::cmp, common/src/types/value.rs:799-833 — le bras
    //   final `_ => self.type_ordinal().cmp(&other.type_ordinal())` avec
    //   type_ordinal(String)=3 > type_ordinal(Int64)=1 : TOUTE chaîne est
    //   déclarée supérieure à TOUT nombre.
    //   Atteint par valeur_dans_intervalle, substrate/src/store.rs:9067,
    //   via find_nodes_in_range, substrate/src/store.rs:10765,
    //   depuis plan_range_filter, engine/src/query/planner/lpg/filter.rs:1932.
    // L'évaluateur, lui, PARSE la chaîne en f64 :
    //   ExpressionPredicate::compare_values,
    //   core/src/execution/operators/filter.rs:3527.
    // MESURE : `WHERE n.nom < 9` → 0 ligne ; `RETURN n.nom < 9` → VRAI
    // pour '2'.
    let db = db_avec(&["2", "9", "10"]);
    let par_projection: Vec<String> = lignes(
        &db,
        "MATCH (n:S) RETURN n.nom AS nom, n.nom < 9 AS lt ORDER BY n.nom",
    )
    .into_iter()
    .filter(|l| l[1] == VRAI)
    .map(|l| match &l[0] {
        Value::String(s) => s.to_string(),
        a => panic!("{a:?}"),
    })
    .collect();
    let mut par_filtre = textes(&db, "MATCH (n:S) WHERE n.nom < 9 RETURN n.nom");
    par_filtre.sort();
    let mut attendu = par_projection;
    attendu.sort();
    assert_eq!(
        par_filtre, attendu,
        "WHERE doit sélectionner ce que la projection déclare vrai"
    );
}

// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : `WHERE n.nom > 1` retient 'alpha' : une comparaison INCONNUE passe le filtre
#[test]
fn une_chaine_non_numerique_ne_passe_pas_un_filtre_numerique() {
    // INTENTION : 'alpha' n'a pas d'ordre commun avec le nombre 1. La
    // comparaison vaut UNKNOWN, et UNKNOWN n'est pas VRAI : la ligne est
    // écartée (logique trivaluée, ISO/IEC 39075 §21).
    //
    // DÉFAUT: même cause que le test voisin — type_ordinal, common/src/
    // types/value.rs:822-833 : String(3) > Int64(1), donc TOUTE chaîne
    // satisfait `> <nombre>`, y compris celles qui ne sont pas des
    // nombres. L'évaluateur, lui, rend bien Null pour 'alpha' < 9
    // (core/src/execution/operators/filter.rs:3527, `s.parse::<f64>()`
    // échoue → None).
    // MESURE : `WHERE n.nom > 1` → 'alpha' est retenu.
    let db = db_avec(&["2", "alpha"]);
    let retenus = textes(&db, "MATCH (n:S) WHERE n.nom > 1 RETURN n.nom");
    assert!(
        !retenus.iter().any(|s| s == "alpha"),
        "'alpha' > 1 vaut UNKNOWN et ne doit pas passer le filtre ; obtenu {retenus:?}"
    );
}

// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : min()/max() sur une colonne CHAÎNE rendent Int64(0) au lieu de la chaîne extrême
#[test]
fn min_et_max_dune_colonne_chaine_suivent_l_ordre_des_chaines() {
    // INTENTION : `min`/`max` rendent le plus petit / le plus grand
    // élément SELON L'ORDRE DU TYPE. Sur une colonne chaîne c'est l'ordre
    // lexicographique — le même que celui d'`ORDER BY` sur cette colonne.
    // Le cas des chiffres est le révélateur : min('10','9','2') est '10'.
    //
    // DÉFAUT: le type de sortie de MIN/MAX est CÂBLÉ à Int64 dans le
    // planificateur — engine/src/query/planner/lpg/aggregate.rs:231-236
    // (« MIN/MAX preserve input type; use Int64 as default »). La valeur
    // rendue est Int64(0) et la colonne est typée Int64.
    // MESURÉ : remplacer ce seul `LogicalType::Int64` par `Any` NE SUFFIT
    // PAS — le test reste rouge, la valeur rendue reste Int64(0). Le
    // câblage du type n'est donc que la moitié du défaut.
    // À NOTER, pour la correction : l'agrégat compare via
    // value_utils::compare_values, core/src/execution/operators/
    // value_utils.rs:27-38, qui pour deux CHAÎNES tente D'ABORD une
    // comparaison NUMÉRIQUE. C'est un TROISIÈME ordre des chaînes,
    // incompatible avec les deux autres : il ferait min('10','9') = '9'
    // là où ORDER BY et `<` disent '10'. Le commentaire de
    // engine/src/database/search.rs:25-29 affirme que ce comparateur est
    // « l'evaluateur canonique du moteur — le meme que les operateurs de
    // filtre » ; la divergence qu'il dit prévenir existe déjà.
    let db = db_avec(&["10", "9", "2"]);
    let ordre = textes(&db, "MATCH (n:S) RETURN n.nom ORDER BY n.nom");
    let r = lignes(&db, "MATCH (n:S) RETURN min(n.nom) AS mn, max(n.nom) AS mx");
    assert_eq!(
        r[0][0],
        Value::String(ordre.first().expect("jeu non vide").as_str().into()),
        "min doit être le premier d'ORDER BY"
    );
    assert_eq!(
        r[0][1],
        Value::String(ordre.last().expect("jeu non vide").as_str().into()),
        "max doit être le dernier d'ORDER BY"
    );
}

// ═════════ GROUPE 3 — CONTAINS / STARTS WITH / ENDS WITH ═════════════

#[test]
fn aiguille_vide_est_contenue_partout() {
    // La chaîne vide est préfixe, suffixe et sous-chaîne de toute chaîne,
    // y compris d'elle-même.
    let db = db_unaire();
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN 'abc' CONTAINS '' AS r LIMIT 1"),
        VRAI
    );
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN 'abc' STARTS WITH '' AS r LIMIT 1"),
        VRAI
    );
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN 'abc' ENDS WITH '' AS r LIMIT 1"),
        VRAI
    );
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN '' CONTAINS '' AS r LIMIT 1"),
        VRAI
    );
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN '' STARTS WITH '' AS r LIMIT 1"),
        VRAI
    );
}

#[test]
fn contains_starts_ends_sont_sensibles_a_la_casse() {
    let db = db_avec(&["abc", "ABC"]);
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN 'ABC' CONTAINS 'b' AS r LIMIT 1"),
        FAUX
    );
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN 'abc' CONTAINS 'B' AS r LIMIT 1"),
        FAUX
    );
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN 'ABC' STARTS WITH 'a' AS r LIMIT 1"),
        FAUX
    );
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN 'ABC' ENDS WITH 'c' AS r LIMIT 1"),
        FAUX
    );
    assert_eq!(
        textes(&db, "MATCH (n:S) WHERE n.nom STARTS WITH 'a' RETURN n.nom"),
        ["abc"]
    );
}

#[test]
fn une_aiguille_plus_longue_que_la_botte_ne_correspond_pas() {
    let db = db_unaire();
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN 'abc' CONTAINS 'abcd' AS r LIMIT 1"),
        FAUX
    );
    assert_eq!(
        valeur(
            &db,
            "MATCH (n:S) RETURN 'abc' STARTS WITH 'abcd' AS r LIMIT 1"
        ),
        FAUX
    );
    assert_eq!(
        valeur(
            &db,
            "MATCH (n:S) RETURN 'abc' ENDS WITH 'zabc' AS r LIMIT 1"
        ),
        FAUX
    );
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN '' CONTAINS 'a' AS r LIMIT 1"),
        FAUX
    );
}

#[test]
fn contains_travaille_sur_les_points_de_code_pas_sur_les_graphemes() {
    // Conséquence directe de l'absence de normalisation implicite :
    // chercher 'é' précomposé dans un 'café' DÉCOMPOSÉ ne trouve rien,
    // alors que les deux chaînes s'affichent à l'identique. Ce n'est pas
    // un défaut — c'est la sémantique de GQL sans collation — mais c'est
    // un piège qui doit être écrit noir sur blanc.
    let db = db_unaire();
    assert_eq!(
        valeur(
            &db,
            "MATCH (n:S) RETURN 'caf\u{e9}' CONTAINS '\u{e9}' AS r LIMIT 1"
        ),
        VRAI
    );
    assert_eq!(
        valeur(
            &db,
            "MATCH (n:S) RETURN 'cafe\u{301}' CONTAINS '\u{e9}' AS r LIMIT 1"
        ),
        FAUX,
        "la forme décomposée ne contient pas le caractère précomposé"
    );
    assert_eq!(
        valeur(
            &db,
            "MATCH (n:S) RETURN 'cafe\u{301}' CONTAINS '\u{301}' AS r LIMIT 1"
        ),
        VRAI
    );
    // et la sous-chaîne trouvée ne coupe jamais un caractère en deux
    assert_eq!(
        valeur(
            &db,
            "MATCH (n:S) RETURN 'caf\u{e9}' CONTAINS 'f\u{e9}' AS r LIMIT 1"
        ),
        VRAI
    );
}

#[test]
fn operateur_de_chaine_sur_operande_non_chaine_rend_inconnu() {
    // Un opérande qui n'est pas une chaîne ne peut pas produire VRAI ni
    // FAUX en silence : la comparaison est INCONNUE (ou refusée). Ce qui
    // serait inacceptable, c'est un booléen inventé.
    let db = db_unaire();
    for q in [
        "MATCH (n:S) RETURN 5 STARTS WITH '5' AS r LIMIT 1",
        "MATCH (n:S) RETURN '5' STARTS WITH 5 AS r LIMIT 1",
        "MATCH (n:S) RETURN 'abc' CONTAINS null AS r LIMIT 1",
        "MATCH (n:S) RETURN null CONTAINS 'a' AS r LIMIT 1",
        "MATCH (n:S) RETURN null STARTS WITH null AS r LIMIT 1",
    ] {
        assert!(
            matches!(valeur(&db, q), Value::Null),
            "doit valoir UNKNOWN, pas un booléen : {q}"
        );
    }
}

#[test]
fn contains_compare_deux_colonnes() {
    let db = db_avec(&["abcdef", "cde", "xyz"]);
    let mut r: Vec<(String, String)> = lignes(
        &db,
        "MATCH (a:S), (b:S) WHERE a.nom CONTAINS b.nom AND a.nom <> b.nom RETURN a.nom, b.nom",
    )
    .into_iter()
    .map(|l| match (&l[0], &l[1]) {
        (Value::String(x), Value::String(y)) => (x.to_string(), y.to_string()),
        _ => panic!("colonnes non-chaînes"),
    })
    .collect();
    r.sort();
    assert_eq!(r, [("abcdef".to_string(), "cde".to_string())]);
}

#[test]
fn starts_with_rend_le_meme_verdict_avec_et_sans_label() {
    // Les deux chemins de plan doivent s'accorder sur les opérateurs de
    // chaîne — contrairement à ce qui se passe sur chaîne-contre-nombre.
    let db = db_avec(&["alpha", "ALPHA", "beta"]);
    let avec = textes(&db, "MATCH (n:S) WHERE n.nom STARTS WITH 'al' RETURN n.nom");
    let sans = textes(&db, "MATCH (n) WHERE n.nom STARTS WITH 'al' RETURN n.nom");
    assert_eq!(avec, ["alpha"]);
    assert_eq!(avec, sans);
}

// ═══════════════════════ GROUPE 4 — LIKE ═════════════════════════════

#[test]
fn like_pourcent_et_souligne() {
    let db = db_avec(&["abc", "ac", "abbc", "b"]);
    let mut r = textes(&db, "MATCH (n:S) WHERE n.nom LIKE 'a%c' RETURN n.nom");
    r.sort();
    assert_eq!(r, ["abbc", "abc", "ac"], "% couvre zéro caractère ou plus");
    let mut r = textes(&db, "MATCH (n:S) WHERE n.nom LIKE 'a_c' RETURN n.nom");
    r.sort();
    assert_eq!(r, ["abc"], "_ couvre exactement un caractère");
}

#[test]
fn like_est_ancre_aux_deux_extremites() {
    // SQL/GQL : LIKE compare le motif à la chaîne ENTIÈRE, pas à une
    // sous-chaîne. C'est la différence avec CONTAINS.
    let db = db_avec(&["abc"]);
    assert!(lignes(&db, "MATCH (n:S) WHERE n.nom LIKE 'b' RETURN n.nom").is_empty());
    assert!(lignes(&db, "MATCH (n:S) WHERE n.nom LIKE 'ab' RETURN n.nom").is_empty());
    assert_eq!(
        textes(&db, "MATCH (n:S) WHERE n.nom LIKE '%b%' RETURN n.nom"),
        ["abc"]
    );
    assert_eq!(
        textes(&db, "MATCH (n:S) WHERE n.nom LIKE 'abc' RETURN n.nom"),
        ["abc"]
    );
}

#[test]
fn like_souligne_couvre_un_caractere_et_non_un_octet() {
    // 'é' pèse deux octets en UTF-8. Un `_` qui compterait en octets
    // exigerait deux jokers — et 'café' ne serait pas reconnu par 'caf_'.
    let db = db_avec(&["caf\u{e9}", "caf\u{1f600}"]);
    let mut r = textes(&db, "MATCH (n:S) WHERE n.nom LIKE 'caf_' RETURN n.nom");
    r.sort();
    assert_eq!(
        r,
        ["caf\u{e9}", "caf\u{1f600}"],
        "_ couvre un caractère, fût-il hors BMP"
    );
}

#[test]
fn like_reconnait_un_pourcent_litteral_echappe() {
    let bs = as_();
    let db = db_avec(&["a%c", "abc", "a_c"]);
    let r = textes(
        &db,
        &format!("MATCH (n:S) WHERE n.nom LIKE 'a{bs}%c' RETURN n.nom"),
    );
    assert_eq!(r, ["a%c"]);
    let r = textes(
        &db,
        &format!("MATCH (n:S) WHERE n.nom LIKE 'a{bs}_c' RETURN n.nom"),
    );
    assert_eq!(r, ["a_c"]);
}

#[test]
fn like_est_sensible_a_la_casse() {
    let db = db_avec(&["abc", "ABC"]);
    assert_eq!(
        textes(&db, "MATCH (n:S) WHERE n.nom LIKE 'a%' RETURN n.nom"),
        ["abc"]
    );
    assert_eq!(
        textes(&db, "MATCH (n:S) WHERE n.nom LIKE 'A%' RETURN n.nom"),
        ["ABC"]
    );
}

#[test]
fn like_motif_vide_ne_reconnait_que_la_chaine_vide() {
    let db = db_avec(&["", "a"]);
    let r = textes(&db, "MATCH (n:S) WHERE n.nom LIKE '' RETURN n.nom");
    assert_eq!(r, [""]);
    let mut r = textes(&db, "MATCH (n:S) WHERE n.nom LIKE '%' RETURN n.nom");
    r.sort();
    assert_eq!(r, ["", "a"], "'%' couvre aussi la chaîne vide");
}

#[test]
fn like_traite_les_metacaracteres_dexpression_reguliere_comme_litteraux() {
    // LIKE n'a que deux jokers, `%` et `_`. Un point, une étoile, un
    // crochet doivent se comparer littéralement — sans quoi le motif
    // 'a.c' reconnaîtrait 'abc', ce qui est le symptôme classique d'une
    // implémentation qui délègue à une expression régulière sans échapper.
    let db = db_avec(&["a.c", "abc", "a+c", "aXc", "a[b]c"]);
    assert_eq!(
        textes(&db, "MATCH (n:S) WHERE n.nom LIKE 'a.c' RETURN n.nom"),
        ["a.c"]
    );
    assert_eq!(
        textes(&db, "MATCH (n:S) WHERE n.nom LIKE 'a+c' RETURN n.nom"),
        ["a+c"]
    );
    assert_eq!(
        textes(&db, "MATCH (n:S) WHERE n.nom LIKE 'a[b]c' RETURN n.nom"),
        ["a[b]c"]
    );
}

#[test]
fn like_avec_null_rend_inconnu() {
    let db = db_avec(&["abc"]);
    assert!(matches!(
        valeur(&db, "MATCH (n:S) RETURN n.nom LIKE null AS r LIMIT 1"),
        Value::Null
    ));
    assert!(matches!(
        valeur(&db, "MATCH (n:S) RETURN null LIKE 'a%' AS r LIMIT 1"),
        Value::Null
    ));
}

#[test]
fn l_operateur_d_expression_reguliere_est_refuse_franchement() {
    // Le moteur n'offre PAS `=~`. Ce qui compte pour la conformité, c'est
    // qu'il le dise : une erreur de syntaxe, pas un NULL silencieux ni un
    // filtre qui ne filtre rien.
    let db = db_avec(&["abc"]);
    let e = db
        .session()
        .execute("MATCH (n:S) WHERE n.nom =~ 'a.c' RETURN n.nom")
        .expect_err("`=~` doit être refusé, pas ignoré");
    let msg = e.to_string();
    assert!(
        msg.contains("=~"),
        "le diagnostic doit nommer l'opérateur refusé ; obtenu : {msg}"
    );
}

// ═════════════ GROUPE 5 — ÉCHAPPEMENTS ET GUILLEMETS ═════════════════

#[test]
fn echappements_simples_dans_un_litteral() {
    let bs = as_();
    let db = db_unaire();
    let un = |suite: &str, attendu: char| {
        let q = format!(
            "MATCH (n:S) RETURN '{bs}{suite}' AS r, char_length('{bs}{suite}') AS l LIMIT 1"
        );
        let l = lignes(&db, &q);
        assert_eq!(l[0][0], Value::String(attendu.to_string().into()), "{q}");
        assert_eq!(l[0][1], Value::Int64(1), "{q}");
    };
    un("n", char::from_u32(10).expect("LF"));
    un("r", char::from_u32(13).expect("CR"));
    un("t", char::from_u32(9).expect("TAB"));
    un("\\", as_());
    un("'", char::from_u32(39).expect("apostrophe"));
}

// CORRIGÉ par ce commit (correctif H, vague 4a : unescape_string décode les échappements unicode à 4 et 8 hexadécimales, adapters parser.rs) — défaut d'origine : l'échappement unicode par point de code rendait six caractères au lieu du caractère désigné
#[test]
fn echappement_unicode_par_point_de_code() {
    // INTENTION : ISO/IEC 39075 admet l'échappement unicode `\uXXXX` (et
    // `\UXXXXXXXX`) dans un littéral chaîne. C'est la seule façon
    // d'écrire un caractère non saisissable au clavier dans une requête
    // transportée en ASCII.
    //
    // DÉFAUT: unescape_string ne connaît que n, r, t, antislash,
    // apostrophe et guillemet — adapters/src/query/gql/parser.rs:9-31.
    // Le bras `Some(other)` REMET l'antislash et le caractère, donc
    // 'A' devient la suite littérale de six caractères, sans
    // avertissement.
    // MESURE : char_length('A') = 6.
    let bs = as_();
    let db = db_unaire();
    let l = lignes(
        &db,
        &format!("MATCH (n:S) RETURN '{bs}u0041' AS r, char_length('{bs}u0041') AS l LIMIT 1"),
    );
    assert_eq!(l[0][0], Value::String("A".into()));
    assert_eq!(l[0][1], Value::Int64(1));
}

// corrigé par ce commit, via la fermeture de classe du refus de queue : la
// relecture de « s' » comme alias laisse désormais une queue non consommée,
// donc la requête est REFUSÉE — l'issue « erreur de syntaxe » que ce test
// accepte explicitement. Le doublement d'apostrophe reste non supporté
// (lexer.rs, scan_string) ; seul le silence a disparu.
#[test]
fn une_apostrophe_doublee_ne_tronque_pas_la_chaine_en_silence() {
    // INTENTION : un moteur a le droit de ne pas accepter le doublement
    // d'apostrophe comme échappement. Il n'a PAS le droit de rendre une
    // AUTRE chaîne sans rien signaler. Deux issues sont acceptables :
    // l'erreur de syntaxe, ou la chaîne « it's ». Une troisième, « it »,
    // ne l'est pas.
    //
    // DÉFAUT: scan_string, adapters/src/query/gql/lexer.rs:518-530, ferme
    // le littéral au premier guillemet non précédé d'un antislash. La
    // suite « s' » est alors relue comme un alias ; la requête reste
    // valide et rend 'it'.
    // MESURE : `RETURN 'it''s' AS dq` → String("it"), colonne nommée
    // `String("it")`.
    let db = db_unaire();
    match db
        .session()
        .execute("MATCH (n:S) RETURN 'it''s' AS dq LIMIT 1")
    {
        Err(_) => {} // refus franc : acceptable
        Ok(r) => assert_eq!(
            r.rows[0][0],
            Value::String("it's".into()),
            "ni erreur ni chaîne correcte : troncature silencieuse"
        ),
    }
}

#[test]
fn les_guillemets_doubles_delimitent_aussi_un_litteral_chaine() {
    let db = db_unaire();
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN \"une chaine\" AS r LIMIT 1"),
        Value::String("une chaine".into())
    );
    // et une apostrophe y vit sans échappement
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN \"l'apostrophe\" AS r LIMIT 1"),
        Value::String("l'apostrophe".into())
    );
}

#[test]
fn une_chaine_portant_un_octet_nul_traverse_le_moteur_intacte() {
    // GQL n'écrit pas l'octet nul littéralement : semis direct. Le point
    // à vérifier est qu'aucune couche ne traite la chaîne comme une
    // chaîne C — la longueur doit compter les caractères APRÈS le nul.
    let avec_nul = "nul\u{0}apres";
    let db = db_avec(&[avec_nul]);
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN char_length(n.nom) AS l"),
        Value::Int64(9)
    );
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN octet_length(n.nom) AS l"),
        Value::Int64(9)
    );
    assert_eq!(
        lignes(
            &db,
            "MATCH (n:S) WHERE n.nom STARTS WITH 'nul' RETURN n.nom"
        )
        .len(),
        1
    );
    assert_eq!(
        lignes(
            &db,
            "MATCH (n:S) WHERE n.nom ENDS WITH 'apres' RETURN n.nom"
        )
        .len(),
        1
    );
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN n.nom"),
        Value::String(avec_nul.into())
    );
}

// ══════ GROUPE 6 — LONGUEURS, UNICODE, NORMALISATION, CASSE ══════════

#[test]
fn char_length_compte_les_caracteres() {
    let db = db_unaire();
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN char_length('abc') AS l LIMIT 1"),
        Value::Int64(3)
    );
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN char_length('') AS l LIMIT 1"),
        Value::Int64(0)
    );
    assert_eq!(
        valeur(
            &db,
            "MATCH (n:S) RETURN char_length('caf\u{e9}') AS l LIMIT 1"
        ),
        Value::Int64(4),
        "un é précomposé est UN caractère"
    );
    assert_eq!(
        valeur(
            &db,
            "MATCH (n:S) RETURN char_length('cafe\u{301}') AS l LIMIT 1"
        ),
        Value::Int64(5),
        "un é décomposé est DEUX caractères"
    );
    assert_eq!(
        valeur(
            &db,
            "MATCH (n:S) RETURN char_length('\u{1f600}') AS l LIMIT 1"
        ),
        Value::Int64(1),
        "un caractère hors BMP est UN caractère, pas deux unités UTF-16"
    );
}

#[test]
fn octet_length_compte_les_octets_utf8() {
    let db = db_unaire();
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN octet_length('abc') AS l LIMIT 1"),
        Value::Int64(3)
    );
    assert_eq!(
        valeur(
            &db,
            "MATCH (n:S) RETURN octet_length('caf\u{e9}') AS l LIMIT 1"
        ),
        Value::Int64(5)
    );
    assert_eq!(
        valeur(
            &db,
            "MATCH (n:S) RETURN octet_length('\u{1f600}') AS l LIMIT 1"
        ),
        Value::Int64(4)
    );
}

// CORRIGÉ par ce commit (correctif H, vague 4a : le bras size/length/cardinality compte les chars() d'une chaîne) — défaut d'origine : size()/length() rendaient des OCTETS, en contradiction avec left()/substring() qui découpent en caractères
#[test]
fn size_dune_chaine_est_coherent_avec_le_decoupage_en_caracteres() {
    // INTENTION : quelle que soit l'unité choisie, `size` doit être
    // COHÉRENT avec les fonctions de découpage du même moteur. Si
    // `size(s)` vaut n, alors `left(s, n)` — qui prend n CARACTÈRES —
    // doit rendre s tout entier, et sa longueur en caractères doit
    // valoir n. C'est une contrainte interne : elle ne suppose aucune
    // collation ni aucune option de la norme.
    //
    // DÉFAUT: le bras `"size" | "length" | "cardinality"` rend
    // `s.len()`, c'est-à-dire des OCTETS —
    // core/src/execution/operators/filter.rs:1702-1712 — alors que
    // `left` (l. 2525) et `substring` (l. 2342) découpent en `chars()`.
    // Sur une liste, le même bras compte des ÉLÉMENTS : `size` change
    // donc d'unité selon le type sans que rien ne le signale.
    // MESURE : size('café') = 5, char_length(left('café', 5)) = 4.
    let db = db_unaire();
    let l = lignes(
        &db,
        "MATCH (n:S) RETURN size('caf\u{e9}') AS n, char_length(left('caf\u{e9}', size('caf\u{e9}'))) AS m LIMIT 1",
    );
    assert_eq!(
        l[0][0], l[0][1],
        "size(s) doit compter ce que left(s, size(s)) rend"
    );
    assert_eq!(l[0][0], Value::Int64(4));
}

// CORRIGÉ par ce commit (correctif H, vague 4a : le bras char_length reconnaît character_length/characterlength) — défaut d'origine : CHARACTER_LENGTH, synonyme ISO de CHAR_LENGTH, rendait NULL
#[test]
fn character_length_est_le_synonyme_iso_de_char_length() {
    // INTENTION : ISO/IEC 39075 nomme la fonction de longueur en
    // caractères `CHAR_LENGTH` ET `CHARACTER_LENGTH` ; les deux écritures
    // sont interchangeables.
    //
    // DÉFAUT: le bras ne reconnaît que "char_length" | "charlength" —
    // core/src/execution/operators/filter.rs:2405. « character_length »
    // n'y correspond pas et retombe sur le bras final l. 3342, qui rend
    // None, donc Null en projection.
    // MESURE : CHARACTER_LENGTH('abc') → Null.
    let db = db_unaire();
    let l = lignes(
        &db,
        "MATCH (n:S) RETURN CHAR_LENGTH('abc') AS a, CHARACTER_LENGTH('abc') AS b LIMIT 1",
    );
    assert_eq!(l[0][1], l[0][0]);
    assert_eq!(l[0][1], Value::Int64(3));
}

// CORRIGÉ par ce commit (correctif H, vague 4a : le bras final d'eval_function consigne une erreur via signale_erreur — le canal de la division par zéro, vague 1) — défaut d'origine : un nom de fonction inconnu rendait NULL, une faute de frappe devenait une colonne vide
#[test]
fn une_fonction_de_chaine_inconnue_est_signalee() {
    // INTENTION : appeler une fonction qui n'existe pas est une erreur de
    // requête (ISO/IEC 39075 §23, condition 42 000 / fonction inconnue).
    // Rendre NULL transforme une faute de frappe — `charlenght`,
    // `concat`, `CHARACTER_LENGTH` — en colonne silencieusement vide,
    // dont rien ne distingue le résultat d'une vraie valeur absente.
    //
    // DÉFAUT: la table de fonctions de l'évaluateur rend `None` pour tout
    // nom non reconnu — bras final de eval_function,
    // core/src/execution/operators/filter.rs:3342 (`_ => None, // Unknown
    // function`) — et `None` devient Null en projection. Aucun
    // diagnostic n'est produit.
    // MESURE : `RETURN fonction_qui_nexiste_pas('a')` → Ok, colonne Null.
    let db = db_unaire();
    assert!(
        db.session()
            .execute("MATCH (n:S) RETURN fonction_qui_nexiste_pas('a') AS r LIMIT 1")
            .is_err(),
        "une fonction inconnue doit lever, pas rendre NULL"
    );
}

#[test]
fn is_normalized_distingue_les_formes_nfc_et_nfd() {
    // Ce chemin-ci fait un VRAI travail unicode (crate
    // unicode-normalization) — c'est lui qui sert de témoin au test
    // suivant.
    let db = db_unaire();
    let l = lignes(
        &db,
        "MATCH (n:S) RETURN 'caf\u{e9}' IS NFC NORMALIZED AS a, 'caf\u{e9}' IS NFD NORMALIZED AS b, \
         'cafe\u{301}' IS NFC NORMALIZED AS c, 'cafe\u{301}' IS NFD NORMALIZED AS d LIMIT 1",
    );
    assert_eq!(l[0][0], VRAI);
    assert_eq!(l[0][1], FAUX);
    assert_eq!(l[0][2], FAUX);
    assert_eq!(l[0][3], VRAI);
    // une chaîne purement ASCII est dans toutes les formes à la fois
    let l = lignes(
        &db,
        "MATCH (n:S) RETURN 'cafz' IS NFC NORMALIZED AS a, 'cafz' IS NFD NORMALIZED AS b LIMIT 1",
    );
    assert_eq!(l[0][0], VRAI);
    assert_eq!(l[0][1], VRAI);
}

// CORRIGÉ par ce commit (correctif H, vague 4a : le bras normalize applique une vraie NFC, celle du juge IS NFC NORMALIZED voisin) — défaut d'origine : normalize() était un no-op, sa sortie n'était pas NFC et le moteur se contredisait lui-même
#[test]
fn normalize_produit_une_chaine_effectivement_normalisee() {
    // INTENTION : la sortie de `normalize(s)` est, par définition, dans
    // la forme normale visée. Le moteur porte déjà le juge de cette
    // propriété — `IS NFC NORMALIZED` — et ce juge doit acquitter la
    // sortie de `normalize`. Aucune référence externe n'est nécessaire :
    // c'est une contradiction interne au moteur.
    //
    // DÉFAUT: le bras "normalize" rend la chaîne TELLE QUELLE —
    // core/src/execution/operators/filter.rs:3142-3155, commentaire
    // « returns the string as-is (NFC normalization) […] deferred to
    // Phase 2 » — alors que le bras "isnormalized" voisin (l. 3157)
    // appelle réellement `s.nfc()`.
    // MESURE : normalize('cafe\u{301}') rend 'cafe\u{301}', et
    // `normalize(x) IS NFC NORMALIZED` vaut FAUX.
    let db = db_avec(&["cafe\u{301}"]);
    assert_eq!(
        valeur(
            &db,
            "MATCH (n:S) RETURN normalize(n.nom) IS NFC NORMALIZED AS r"
        ),
        VRAI,
        "la sortie de normalize doit satisfaire IS NFC NORMALIZED"
    );
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN normalize(n.nom) AS r"),
        Value::String("caf\u{e9}".into())
    );
}

#[test]
fn le_changement_de_casse_unicode_nest_ni_reversible_ni_conservateur_en_longueur() {
    // Propriété à connaître avant d'écrire `toLower(x) = toLower(y)` en
    // guise de comparaison insensible à la casse : le repliement de casse
    // unicode PERD de l'information. 'ß' monte en 'SS' (un caractère
    // devient deux) et ne redescend pas.
    let db = db_unaire();
    assert_eq!(
        valeur(
            &db,
            "MATCH (n:S) RETURN toupper('stra\u{df}e') AS r LIMIT 1"
        ),
        Value::String("STRASSE".into())
    );
    assert_eq!(
        valeur(
            &db,
            "MATCH (n:S) RETURN tolower(toupper('stra\u{df}e')) AS r LIMIT 1"
        ),
        Value::String("strasse".into()),
        "l'aller-retour de casse n'est pas l'identité"
    );
    assert_eq!(
        valeur(
            &db,
            "MATCH (n:S) RETURN char_length(toupper('stra\u{df}e')) AS l LIMIT 1"
        ),
        Value::Int64(7),
        "la longueur change : 6 caractères deviennent 7"
    );
    // le changement de casse ne normalise pas davantage
    assert_eq!(
        valeur(
            &db,
            "MATCH (n:S) RETURN toupper('cafe\u{301}') AS r LIMIT 1"
        ),
        Value::String("CAFE\u{301}".into())
    );
}

#[test]
fn substring_left_et_right_decoupent_en_caracteres() {
    let db = db_unaire();
    assert_eq!(
        valeur(
            &db,
            "MATCH (n:S) RETURN substring('\u{1f600}abc', 0, 2) AS r LIMIT 1"
        ),
        Value::String("\u{1f600}a".into()),
        "le découpage ne coupe pas un caractère hors BMP en deux"
    );
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN left('\u{e9}xyz', 2) AS r LIMIT 1"),
        Value::String("\u{e9}x".into())
    );
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN right('xy\u{e9}', 2) AS r LIMIT 1"),
        Value::String("y\u{e9}".into())
    );
    // au-delà de la fin : chaîne vide, jamais de panique ni d'octet coupé
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN substring('abc', 10) AS r LIMIT 1"),
        Value::String("".into())
    );
}

// ═══════ GROUPE 7 — CONCATÉNATION ET FONCTIONS DE CHAÎNES ════════════

#[test]
fn concatenation_par_barre_verticale_et_par_plus() {
    let db = db_unaire();
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN 'a' || 'b' AS r LIMIT 1"),
        Value::String("ab".into())
    );
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN 'a' + 'b' AS r LIMIT 1"),
        Value::String("ab".into())
    );
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN '' || 'b' || '' AS r LIMIT 1"),
        Value::String("b".into()),
        "la chaîne vide est le neutre de la concaténation"
    );
    // la concaténation préserve les longueurs
    assert_eq!(
        valeur(
            &db,
            "MATCH (n:S) RETURN char_length('caf\u{e9}' || '\u{1f600}') AS l LIMIT 1"
        ),
        Value::Int64(5)
    );
}

#[test]
fn concatenation_avec_null_rend_null() {
    let db = db_unaire();
    assert!(matches!(
        valeur(&db, "MATCH (n:S) RETURN 'a' || null AS r LIMIT 1"),
        Value::Null
    ));
    assert!(matches!(
        valeur(&db, "MATCH (n:S) RETURN null || 'a' AS r LIMIT 1"),
        Value::Null
    ));
}

#[test]
fn split_et_string_join_font_un_aller_retour_fidele() {
    // Loi que toute paire découpe/recolle cohérente doit satisfaire :
    // string_join(split(s, d), d) = s. Elle attrape en particulier les
    // séparateurs vides perdus aux extrémités.
    let db = db_unaire();
    for (s, d) in [("a,b,,c", ","), (",a,", ","), ("", ","), ("abc", "--")] {
        assert_eq!(
            valeur(
                &db,
                &format!("MATCH (n:S) RETURN string_join(split('{s}', '{d}'), '{d}') AS r LIMIT 1")
            ),
            Value::String(s.into()),
            "aller-retour rompu pour {s:?} découpé par {d:?}"
        );
    }
    assert_eq!(
        valeur(
            &db,
            "MATCH (n:S) RETURN size(split('a,b,,c', ',')) AS l LIMIT 1"
        ),
        Value::Int64(4),
        "un séparateur doublé produit un élément vide, il ne disparaît pas"
    );
}

/// ── `split(s, d)[k]` : LE CHEMIN RAPIDE DOIT RENDRE EXACTEMENT LA VOIE GÉNÉRALE ──
///
/// `ExpressionPredicate::split_indexe_direct` (`core/…/operators/filter.rs`)
/// évite de matérialiser la liste entière quand seule une position est lue.
/// C'est une optimisation de COÛT : elle ne doit rien changer au RÉSULTAT, y
/// compris là où `str::split` surprend — séparateur vide, séparateur doublé,
/// index négatif, index hors bornes, argument non textuel.
///
/// Ces assertions passent AVANT comme APRÈS le chemin rapide : c'est
/// exactement ce qu'on leur demande. Le gain, lui, ne se mesure pas ici mais
/// au banc d'agrégats — une suite verte ne prouve pas une accélération.
#[test]
fn split_indexe_suit_la_voie_generale_sur_les_cas_limites() {
    let db = db_unaire();
    let idx = |e: &str| valeur(&db, &format!("MATCH (n:S) RETURN {e} AS r LIMIT 1"));
    let s = |t: &str| Value::String(t.into());

    // Nominal.
    assert_eq!(idx("split('a,b,c', ',')[0]"), s("a"));
    assert_eq!(idx("split('a,b,c', ',')[1]"), s("b"));
    assert_eq!(idx("split('a,b,c', ',')[2]"), s("c"));

    // Hors bornes : la voie générale faisait `items.get(idx)` → `None` → NULL.
    assert!(matches!(idx("split('a,b', ',')[2]"), Value::Null));
    assert!(matches!(idx("split('a,b', ',')[99]"), Value::Null));

    // Index NÉGATIF : compté depuis la fin (branche `len + i`).
    assert_eq!(idx("split('a,b,c', ',')[-1]"), s("c"));
    assert_eq!(idx("split('a,b,c', ',')[-3]"), s("a"));
    // …et hors bornes par le bas : `len + i < 0` → NULL, jamais un panic.
    assert!(matches!(idx("split('a,b,c', ',')[-4]"), Value::Null));

    // SÉPARATEUR DOUBLÉ : un élément vide reste un élément, il ne disparaît pas.
    assert_eq!(idx("split('a,,c', ',')[1]"), s(""));
    assert_eq!(idx("split('a,,c', ',')[2]"), s("c"));
    assert_eq!(idx("split(',a,', ',')[0]"), s(""));
    assert_eq!(idx("split(',a,', ',')[2]"), s(""));

    // SÉPARATEUR VIDE : `"abc".split("")` rend CINQ éléments (« », a, b, c, « »).
    // C'est le cas qui interdit de raisonner « k tokens = k+1 positions ».
    assert_eq!(idx("split('abc', '')[0]"), s(""));
    assert_eq!(idx("split('abc', '')[1]"), s("a"));
    assert_eq!(idx("split('abc', '')[4]"), s(""));
    assert!(matches!(idx("split('abc', '')[5]"), Value::Null));

    // ARGUMENT NON TEXTUEL : `split` rend `None`, donc l'indexation aussi — et
    // l'index n'est même pas évalué, comme avant (l'ordre est préservé).
    assert!(matches!(idx("split(null, ',')[0]"), Value::Null));
    assert!(matches!(idx("split(1, ',')[0]"), Value::Null));
    assert!(matches!(idx("split('a,b', null)[0]"), Value::Null));

    // INDEX CALCULÉ (non littéral) : le chemin rapide doit le suivre aussi.
    assert_eq!(idx("split('a,b,c', ',')[1 + 1]"), s("c"));

    // INDEX NON ENTIER : ce n'est pas le bras « liste indexée par un entier »,
    // donc la voie générale tranche — ici NULL, sans plantage.
    assert!(matches!(idx("split('a,b', ',')['1']"), Value::Null));

    // Le chemin rapide ne détourne QUE les bases `split` : l'indexation de
    // LISTE ordinaire garde son comportement, négatif compris.
    assert_eq!(idx("[10, 20, 30][1]"), Value::Int64(20));
    assert_eq!(idx("[10, 20, 30][-1]"), Value::Int64(30));
    assert!(matches!(idx("[10, 20][5]"), Value::Null));
}

#[test]
fn string_join_ignore_les_null_de_la_liste() {
    let db = db_unaire();
    assert_eq!(
        valeur(
            &db,
            "MATCH (n:S) RETURN string_join(['a', null, 'b'], '-') AS r LIMIT 1"
        ),
        Value::String("a-b".into())
    );
    assert!(matches!(
        valeur(
            &db,
            "MATCH (n:S) RETURN string_join(null, '-') AS r LIMIT 1"
        ),
        Value::Null
    ));
}

#[test]
fn group_concat_restitue_toutes_les_valeurs_avec_le_separateur_demande() {
    // Agrégat de chaînes : il ne doit rien perdre ni rien inventer.
    let db = db_avec(&["aa", "bb", "cc"]);
    let r = valeur(&db, "MATCH (n:S) RETURN group_concat(n.nom, '|') AS g");
    let Value::String(g) = r else {
        panic!("group_concat doit rendre une chaîne, obtenu {r:?}")
    };
    let mut morceaux: Vec<&str> = g.split('|').collect();
    morceaux.sort_unstable();
    assert_eq!(morceaux, ["aa", "bb", "cc"]);
    // et il s'accorde avec le chemin collect + string_join
    let r2 = valeur(
        &db,
        "MATCH (n:S) RETURN string_join(collect(n.nom), '|') AS g",
    );
    let Value::String(g2) = r2 else {
        panic!("chaîne attendue")
    };
    let mut m2: Vec<&str> = g2.split('|').collect();
    m2.sort_unstable();
    assert_eq!(
        m2, morceaux,
        "group_concat et string_join(collect(...)) doivent concorder"
    );
}

#[test]
fn les_fonctions_de_chaine_propagent_null() {
    let db = db_unaire();
    for f in [
        "char_length",
        "octet_length",
        "size",
        "toupper",
        "tolower",
        "trim",
        "ltrim",
        "rtrim",
        "normalize",
    ] {
        assert!(
            matches!(
                valeur(&db, &format!("MATCH (n:S) RETURN {f}(null) AS r LIMIT 1")),
                Value::Null
            ),
            "{f}(null) doit valoir NULL"
        );
    }
}

#[test]
fn replace_et_trim_operent_sur_des_chaines_unicode() {
    let db = db_unaire();
    assert_eq!(
        valeur(
            &db,
            "MATCH (n:S) RETURN replace('caf\u{e9}', '\u{e9}', 'e') AS r LIMIT 1"
        ),
        Value::String("cafe".into())
    );
    assert_eq!(
        valeur(
            &db,
            "MATCH (n:S) RETURN replace('cafe\u{301}', '\u{e9}', 'e') AS r LIMIT 1"
        ),
        Value::String("cafe\u{301}".into()),
        "replace ne normalise pas davantage que l'égalité"
    );
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN trim('  caf\u{e9}  ') AS r LIMIT 1"),
        Value::String("caf\u{e9}".into())
    );
    assert_eq!(
        valeur(
            &db,
            "MATCH (n:S) RETURN TRIM(BOTH '\u{e9}' FROM '\u{e9}caf\u{e9}') AS r LIMIT 1"
        ),
        Value::String("caf".into())
    );
}

// ═══════════════ GROUPE 8 — CHAÎNES TRÈS LONGUES ═════════════════════

#[test]
fn une_chaine_de_deux_cent_mille_caracteres_traverse_le_moteur() {
    let longue = "a".repeat(200_000);
    let db = db_avec(&[&longue]);
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN char_length(n.nom) AS l"),
        Value::Int64(200_000)
    );
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN octet_length(n.nom) AS l"),
        Value::Int64(200_000)
    );
    let q = format!("MATCH (n:S) WHERE n.nom = '{longue}' RETURN char_length(n.nom) AS l");
    assert_eq!(
        valeur(&db, &q),
        Value::Int64(200_000),
        "égalité sur 200 000 caractères"
    );
    assert_eq!(
        valeur(&db, "MATCH (n:S) RETURN n.nom"),
        Value::String(longue.as_str().into())
    );
}

#[test]
fn l_ordre_de_deux_chaines_tres_longues_se_joue_sur_le_dernier_caractere() {
    // Le point de divergence est au rang 199 999 : un comparateur qui
    // tronquerait, hacherait ou n'examinerait qu'un préfixe les
    // déclarerait égales.
    let a = format!("{}a", "a".repeat(199_999));
    let b = format!("{}b", "a".repeat(199_999));
    let db = db_avec(&[&b, &a]);
    let ordre = textes(&db, "MATCH (n:S) RETURN n.nom ORDER BY n.nom");
    assert_eq!(
        ordre[0], a,
        "la divergence au dernier caractère doit décider de l'ordre"
    );
    assert_eq!(ordre[1], b);
    // même verdict par le chemin du balayage par intervalle
    assert_eq!(
        lignes(
            &db,
            &format!("MATCH (n:S) WHERE n.nom < '{b}' RETURN n.nom")
        )
        .len(),
        1
    );
    // et par l'évaluateur
    assert_eq!(
        valeur(
            &db,
            "MATCH (x:S), (y:S) WHERE x.nom ENDS WITH 'a' AND y.nom ENDS WITH 'b' \
             RETURN x.nom < y.nom AS r"
        ),
        VRAI
    );
}

#[test]
fn contains_et_ends_with_sur_une_chaine_tres_longue() {
    let longue = format!("{}aiguille{}", "x".repeat(100_000), "y".repeat(100_000));
    let db = db_avec(&[&longue, &"x".repeat(200_008)]);
    assert_eq!(
        lignes(
            &db,
            "MATCH (n:S) WHERE n.nom CONTAINS 'aiguille' RETURN n.nom"
        )
        .len(),
        1
    );
    assert_eq!(
        lignes(&db, "MATCH (n:S) WHERE n.nom ENDS WITH 'yyyy' RETURN n.nom").len(),
        1
    );
    assert_eq!(
        lignes(
            &db,
            "MATCH (n:S) WHERE n.nom STARTS WITH 'xxxx' RETURN n.nom"
        )
        .len(),
        2
    );
    // une aiguille elle-même très longue
    let aiguille = "x".repeat(99_999);
    assert_eq!(
        lignes(
            &db,
            &format!("MATCH (n:S) WHERE n.nom CONTAINS '{aiguille}' RETURN n.nom")
        )
        .len(),
        2
    );
}
