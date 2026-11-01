//! Conformité GQL — lot 06 : les NOMBRES.
//!
//! Domaine : l'ARITHMÉTIQUE et la REPRÉSENTATION. Entier contre flottant,
//! promotion implicite, division, modulo, débordement `i64`, précision,
//! très grands et très petits nombres, zéro négatif, NaN et infinis dans les
//! OPÉRATIONS. Les comparaisons numériques appartiennent au lot 02, les
//! conversions inter-types au lot 12, les agrégats au lot 07 — on ne les
//! rejoue pas ici, on ne teste que ce que l'arithmétique en fait.
//!
//! Principe de rédaction : **chaque assertion encode l'intention, jamais
//! l'observation.** Un test qui échoue est placé derrière `#[cfg(feature =
//! defauts_connus)]`, avec la cause en fichier:ligne juste au-dessus, et il
//! énonce toujours ce que la sémantique DOIT donner.
//!
//! Les HUIT défauts D1–D8 mesurés le 2026-08-24 ont été CORRIGÉS par la
//! campagne « arithmétique gardée » : leurs tests, dégardés sans qu'aucune
//! assertion ne soit touchée, tournent désormais dans la suite ordinaire et
//! sont VERTS. La table ci-dessous reste comme trace de la mesure d'origine.
//!
//! ## Divergences mesurées (2026-08-24, lot `gql-06-nombres`) — corrigées
//!
//! | # | Divergence | Cause |
//! |---|---|---|
//! | D1 | `1 / 0` fait PANIQUER le moteur | `filter.rs:1357` passe `\|a, b\| a / b` brut à `eval_arithmetic` (`filter.rs:1487`) ; `i64 / 0` panique dans tous les profils |
//! | D2 | idem par une PROPRIÉTÉ : le défaut atteint les données réelles | `filter.rs:1357` |
//! | D3 | `i64::MIN / -1` fait PANIQUER le moteur | `filter.rs:1357`, « divide with overflow » |
//! | D4 | `1 % 0` rend NULL en silence, indiscernable d'une propriété absente | `filter.rs:1503` `_ => None` |
//! | D5 | `i64::MAX + 1` déborde sans garde | `filter.rs:1314` ; **enroulement SILENCIEUX en release**, `[profile.release]` n'active pas `overflow-checks` (`Cargo.toml:186`) |
//! | D6 | idem pour `*` et `-` | `filter.rs:1351`, `filter.rs:1345` |
//! | D7 | **`sum` déborde sans aucune garde** — la lacune signalée par le lot 07 | `aggregate.rs:191` `*sum += v` ; enroulement silencieux en release |
//! | D8 | `-9223372036854775808` est refusé « Invalid integer » | `parser.rs:3404` analyse le littéral sans son moins unaire ; la borne basse de `i64` est inatteignable |
//!
//! ## La correction de D1, D3, D4, D5, D6 existe DÉJÀ dans ce dépôt
//!
//! `crates/obrain-engine/src/catalog/check_eval.rs:784-800` implémente la
//! MÊME arithmétique avec `checked_add` / `checked_sub` / `checked_mul` et un
//! refus explicite de la division et du modulo par zéro. Le chemin des
//! contraintes `CHECK` était donc gardé ; celui des requêtes ne l'était pas.
//! Il ne manquait pas un algorithme, il manquait un branchement — c'est ce
//! branchement (sémantique dupliquée avec commentaire croisé, la dépendance
//! des crates interdisant le partage direct) que la campagne a posé.
//!
//! ## Anti-vacuité : 11 mutations posées, 11 tuées, 0 survivante
//!
//! Chaque test vert de ce fichier a été vérifié en CASSANT le code qu'il
//! prétend couvrir, puis en contrôlant que c'est bien LUI qui rougit, et avec
//! son propre message. Les mutations : type entier du résultat, promotion
//! mixte, sens de la troncature de la division, signe du reste, `sum` routée
//! par le flottant, NaN escamoté par `value_to_f64`, `min`/`max` comparés en
//! flottant, classement lexical d'un littéral à exposant, addition décalée
//! d'un cran, valeur absolue appliquée aux flottants (qui détruit le zéro
//! négatif), et une réécriture fautive du PLANIFICATEUR.
//!
//! DEUX MUTATIONS POUR LES TESTS `WHERE`, UNE PAR CHEMIN — c'est la leçon
//! que d'autres lots ont payée cher, et elle est ici tranchée par la mesure :
//!
//! - côté ÉVALUATEUR, décaler `|a, b| a + b` d'une unité (`filter.rs:1314`)
//!   fait rougir `l_addition_dans_where_s_accorde_avec_l_addition_projetee` ;
//! - côté PLANIFICATEUR, élargir `extract_range_predicate`
//!   (`planner/lpg/filter.rs:2016`) pour qu'il réécrive `n.p + k <op> lit` en
//!   un balayage d'intervalle sur `n.p` — donc en JETANT l'arithmétique —
//!   fait rougir le MÊME test.
//!
//! Le test garde donc les deux chemins et non un seul. Et la mutation de la
//! division (`filter.rs:1357`) fait rougir d'un seul coup la projection ET le
//! filtre : pour l'arithmétique, `RETURN` et `WHERE` partagent aujourd'hui
//! un unique évaluateur, parce que `extract_range_predicate` exige une
//! PROPRIÉTÉ NUE d'un côté du comparateur et laisse donc toute expression
//! arithmétique au `FilterOperator` générique.
//!
//! ## Ce que la précision flottante interdit de tester
//!
//! Au-delà de `2^53`, l'écart entre deux `f64` consécutifs vaut 2 : les
//! entiers impairs n'y sont plus représentables. Toute exigence de la forme
//! « `avg` de N valeurs égales à `2^53+1` rend `2^53+1` » est donc
//! INEXPRIMABLE — le littéral Rust `9007199254740993.0` vaut déjà
//! `9007199254740992.0`, et le test passerait trivialement. C'est pourquoi
//! ce fichier ne contient AUCUN test de « perte de précision de `avg` » :
//! `avg` rend un flottant, et un flottant ne peut pas mieux faire. La seule
//! exigence tenable, et celle qu'on tient ici, est **de ne pas faire passer
//! par le flottant ce qui peut rester exact** — c'est le sens des tests du
//! groupe PRÉCISION.

#![cfg(feature = "gql")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

// ---------------------------------------------------------------------------
// Harnais
// ---------------------------------------------------------------------------

/// Issue d'une évaluation.
///
/// Un moteur de base de données a le droit de rendre une valeur et le droit
/// de refuser une requête. Il n'a jamais le droit de PANIQUER : un
/// débordement d'entier ou une division par zéro sont des données d'entrée
/// ordinaires, pas des bogues de programmation, et une panique emporte le
/// thread appelant — dans un moteur embarqué, celui de l'application.
/// `Issue::Panique` existe pour nommer cette troisième sortie au lieu de
/// laisser tomber le binaire de test entier.
#[derive(Debug)]
enum Issue {
    Valeur(Value),
    Erreur(String),
    Panique,
}

impl Issue {
    /// La valeur, ou un échec qui NOMME l'issue réellement obtenue.
    fn valeur(self, contexte: &str) -> Value {
        match self {
            Issue::Valeur(v) => v,
            Issue::Erreur(e) => panic!("{contexte} : le moteur a REFUSÉ la requête — {e}"),
            Issue::Panique => panic!("{contexte} : le moteur a PANIQUÉ"),
        }
    }

    fn est_erreur(&self) -> bool {
        matches!(self, Issue::Erreur(_))
    }
}

/// Base à une seule ligne d'ancrage, pour évaluer des expressions scalaires.
fn base_ancree() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    s.execute("INSERT (:Ancre {k: 'a'})").unwrap();
    s.commit().unwrap();
    db
}

/// Évalue `expr` sur la ligne d'ancrage, sans jamais laisser une panique du
/// moteur emporter le binaire de test.
fn evalue(db: &ObrainDB, expr: &str) -> Issue {
    let q = format!("MATCH (a:Ancre) RETURN {expr}");
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| db.session().execute(&q)));
    match r {
        Ok(Ok(res)) => match res.rows.first() {
            Some(ligne) if !ligne.is_empty() => Issue::Valeur(ligne[0].clone()),
            _ => Issue::Erreur(format!(
                "aucune ligne produite — l'ancre existe pourtant : {q}"
            )),
        },
        Ok(Err(e)) => Issue::Erreur(format!("{e}")),
        Err(_) => Issue::Panique,
    }
}

/// Raccourci : la valeur d'une expression dont on attend qu'elle aboutisse.
fn val(db: &ObrainDB, expr: &str) -> Value {
    evalue(db, expr).valeur(expr)
}

/// Le `f64` porté par une valeur, avec un échec qui nomme le type obtenu.
fn flottant(v: &Value, contexte: &str) -> f64 {
    match v {
        Value::Float64(f) => *f,
        autre => panic!("{contexte} : attendu un Float64, obtenu {autre:?}"),
    }
}

/// Exécute une requête et rend ses lignes, ou échoue en citant la requête.
fn lignes(db: &ObrainDB, q: &str) -> Vec<Vec<Value>> {
    db.session()
        .execute(q)
        .unwrap_or_else(|e| panic!("requête refusée par le moteur :\n  {q}\n  → {e}"))
        .rows
}

// ===========================================================================
// GROUPE 1 — le TYPE RÉSULTANT d'une opération
// ===========================================================================

/// Entier OP entier reste ENTIER. La promotion gratuite au flottant serait
/// une perte de précision silencieuse au-delà de `2^53` (cf. groupe
/// PRÉCISION) : le type doit être conservé, pas seulement la valeur.
#[test]
fn entier_operateur_entier_reste_entier() {
    let db = base_ancree();
    for (expr, attendu) in [
        ("1 + 1", 2_i64),
        ("2 - 5", -3),
        ("2 * 3", 6),
        ("6 / 3", 2),
        ("7 % 3", 1),
    ] {
        assert_eq!(
            val(&db, expr),
            Value::Int64(attendu),
            "{expr} porte sur deux entiers : le résultat est l'entier {attendu}, \
             du type Int64 et non Float64"
        );
    }
}

/// Flottant OP flottant reste FLOTTANT — y compris quand le résultat tombe
/// sur un entier mathématique. `2.0 + 2.0` ne doit pas se contracter en
/// `Int64(4)` : le type d'une expression se déduit de ses opérandes, pas de
/// la valeur qu'elle prend sur ce jeu de données.
#[test]
fn flottant_operateur_flottant_reste_flottant() {
    let db = base_ancree();
    for expr in ["2.0 + 2.0", "6.0 - 2.0", "2.0 * 2.0", "8.0 / 2.0"] {
        let v = val(&db, expr);
        assert!(
            matches!(v, Value::Float64(_)),
            "{expr} ne porte que des flottants : le résultat reste flottant \
             même s'il vaut un entier mathématique — obtenu {v:?}"
        );
        assert_eq!(flottant(&v, expr), 4.0, "{expr} vaut 4");
    }
}

/// La promotion entier → flottant est SYMÉTRIQUE : le mélange donne le même
/// résultat quel que soit le côté où se trouve le flottant. Une promotion
/// qui ne s'appliquerait que d'un côté rendrait `+` non commutatif.
#[test]
fn la_promotion_vers_le_flottant_est_symetrique() {
    let db = base_ancree();
    for (gauche, droite) in [
        ("1 + 2.5", "2.5 + 1"),
        ("1 * 2.5", "2.5 * 1"),
        ("10 - 2.5", "-2.5 + 10"),
    ] {
        let (a, b) = (val(&db, gauche), val(&db, droite));
        assert!(
            matches!(a, Value::Float64(_)),
            "{gauche} mélange entier et flottant : le résultat est promu au \
             flottant — obtenu {a:?}"
        );
        assert_eq!(
            a, b,
            "la promotion ne dépend pas du côté où se trouve le flottant : \
             {gauche} et {droite} donnent le même nombre"
        );
    }
}

// ===========================================================================
// GROUPE 2 — la DIVISION
// ===========================================================================

/// La division de deux entiers TRONQUE VERS ZÉRO, elle n'arrondit pas et ne
/// prend pas le plancher. C'est la règle de la division exacte à échelle
/// nulle : `-7 / 2` vaut -3 et non -4. Les valeurs sont lues dans des
/// propriétés et non écrites en littéraux, pour que le signe soit celui de
/// l'opérande et non l'effet d'un moins unaire replié par l'analyseur.
#[test]
fn la_division_entiere_tronque_vers_zero() {
    let db = ObrainDB::new_in_memory();
    {
        let mut s = db.session();
        s.begin_transaction().unwrap();
        // Semis DIRECT : les valeurs négatives entrent par l'API de
        // production et non par un littéral, pour que ce test mesure la
        // division et rien d'autre — la lecture d'un `-7` littéral relève du
        // lot des littéraux, plus bas.
        for (nom, a, b) in [
            ("pp", 7_i64, 2_i64),
            ("np", -7, 2),
            ("pn", 7, -2),
            ("nn", -7, -2),
        ] {
            let _ = s.create_node_with_props(
                &["D"],
                [
                    ("nom", Value::String(nom.into())),
                    ("a", Value::Int64(a)),
                    ("b", Value::Int64(b)),
                ],
            );
        }
        s.commit().unwrap();
    }
    for (nom, attendu) in [("pp", 3_i64), ("np", -3), ("pn", -3), ("nn", 3)] {
        let r = lignes(
            &db,
            &format!("MATCH (d:D {{nom: '{nom}'}}) RETURN d.a / d.b"),
        );
        assert_eq!(r.len(), 1, "une seule ligne pour {nom}");
        assert_eq!(
            r[0][0],
            Value::Int64(attendu),
            "la division entière tronque VERS ZÉRO : le quotient de {nom} vaut \
             {attendu}, pas son plancher"
        );
    }
}

/// Dès qu'un opérande est flottant, la division ne tronque plus : elle rend
/// le quotient exact. `7 / 2` vaut 3 mais `7 / 2.0` vaut 3.5 — c'est le TYPE
/// des opérandes qui décide, pas la valeur.
#[test]
fn la_division_mixte_ne_tronque_pas() {
    let db = base_ancree();
    for expr in ["7 / 2.0", "7.0 / 2"] {
        let v = val(&db, expr);
        assert_eq!(
            flottant(&v, expr),
            3.5,
            "{expr} mélange entier et flottant : le quotient exact est 3.5, \
             une troncature à 3 perdrait la moitié"
        );
    }
    assert_eq!(
        val(&db, "7 / 2"),
        Value::Int64(3),
        "le même couple d'opérandes écrit en entiers tronque, lui : \
         c'est le type qui décide"
    );
}

/// La division ENTIÈRE par zéro n'a pas de résultat : les entiers n'ont pas
/// d'infini. Le moteur doit REFUSER la requête. Il n'a le droit ni de rendre
/// une valeur inventée, ni — surtout — de paniquer : en Rust, `a / 0` sur
/// `i64` panique dans TOUS les profils de compilation, debug comme release,
/// et emporte le thread appelant.
// corrigé par ce commit — cause d'origine : crates/obrain-core/src/execution/operators/filter.rs:1357 passe
// l'opérateur BRUT `|a, b| a / b` à `eval_arithmetic`, dont la branche
// Int64/Int64 (filter.rs:1487) l'applique tel quel. En Rust, `i64 / 0`
// panique dans TOUS les profils de compilation. La correction existe déjà
// AILLEURS DANS LE MÊME DÉPÔT : crates/obrain-engine/src/catalog/check_eval.rs:791
// refuse la division par zéro par un `Err("division by zero")` propre — le
// chemin des contraintes CHECK est gardé, celui des requêtes ne l'est pas.
#[test]
fn la_division_entiere_par_zero_est_refusee_et_non_fatale() {
    let db = base_ancree();
    let issue = evalue(&db, "1 / 0");
    assert!(
        !matches!(&issue, Issue::Panique),
        "une division par zéro est une DONNÉE, pas un bogue : le moteur doit \
         la refuser proprement, jamais paniquer — obtenu {issue:?}"
    );
    assert!(
        issue.est_erreur(),
        "les entiers n'ont pas d'infini : `1 / 0` n'a pas de résultat et la \
         requête doit être refusée — obtenu {issue:?}"
    );
}

/// La division par zéro passée par une PROPRIÉTÉ suit la même règle que le
/// littéral. Le littéral peut être replié à la planification ; la propriété,
/// elle, force l'évaluation ligne à ligne. Les deux chemins doivent
/// s'accorder — un moteur qui refuse `1 / 0` mais panique sur `n.a / n.b`
/// n'a rien réglé.
// corrigé par ce commit — cause d'origine : même cause que le test précédent —
// crates/obrain-core/src/execution/operators/filter.rs:1357. Ce test montre
// en plus que le défaut n'est pas un artefact du repliement de constantes :
// il frappe aussi l'évaluation ligne à ligne, donc les données réelles.
#[test]
fn la_division_par_zero_lue_dans_une_propriete_suit_la_meme_regle() {
    let db = ObrainDB::new_in_memory();
    {
        let mut s = db.session();
        s.begin_transaction().unwrap();
        s.execute("INSERT (:Z {a: 1, b: 0})").unwrap();
        s.commit().unwrap();
    }
    let q = "MATCH (z:Z) RETURN z.a / z.b";
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| db.session().execute(q)));
    assert!(
        r.is_ok(),
        "diviser par une propriété nulle ne doit pas paniquer : {q}"
    );
    assert!(
        r.unwrap().is_err(),
        "les entiers n'ont pas d'infini : la requête doit être refusée — {q}"
    );
}

/// La division FLOTTANTE par zéro, elle, a un résultat : IEEE-754 le définit.
/// Contrairement aux entiers, le binaire64 possède ±∞ et NaN, et le moteur
/// stocke ses flottants dans ce format ; il doit donc en livrer la sémantique
/// au lieu d'inventer la sienne. Le SIGNE compte : `1.0 / 0.0` et
/// `-1.0 / 0.0` ne sont pas le même nombre.
#[test]
fn la_division_flottante_par_zero_suit_ieee() {
    let db = base_ancree();

    let plus = val(&db, "1.0 / 0.0");
    let f = flottant(&plus, "1.0 / 0.0");
    assert!(
        f.is_infinite() && f.is_sign_positive(),
        "1.0 / 0.0 vaut +∞ en IEEE-754 — obtenu {f}"
    );

    let moins = val(&db, "-1.0 / 0.0");
    let f = flottant(&moins, "-1.0 / 0.0");
    assert!(
        f.is_infinite() && f.is_sign_negative(),
        "-1.0 / 0.0 vaut -∞ : le signe du dividende survit — obtenu {f}"
    );

    let indetermine = val(&db, "0.0 / 0.0");
    let f = flottant(&indetermine, "0.0 / 0.0");
    assert!(
        f.is_nan(),
        "0.0 / 0.0 est indéterminé : IEEE-754 rend NaN, pas 0 ni ∞ — obtenu {f}"
    );
}

// ===========================================================================
// GROUPE 3 — le MODULO
// ===========================================================================

/// Le reste prend le SIGNE DU DIVIDENDE, jamais celui du diviseur. C'est
/// l'identité qui le commande : `a = (a / b) * b + a % b` doit tenir avec la
/// division tronquée vers zéro du groupe précédent. Un modulo à la Python
/// (signe du diviseur) casserait cette identité.
#[test]
fn le_reste_prend_le_signe_du_dividende() {
    let db = ObrainDB::new_in_memory();
    {
        let mut s = db.session();
        s.begin_transaction().unwrap();
        for (nom, a, b) in [
            ("pp", 7_i64, 3_i64),
            ("np", -7, 3),
            ("pn", 7, -3),
            ("nn", -7, -3),
        ] {
            let _ = s.create_node_with_props(
                &["M"],
                [
                    ("nom", Value::String(nom.into())),
                    ("a", Value::Int64(a)),
                    ("b", Value::Int64(b)),
                ],
            );
        }
        s.commit().unwrap();
    }
    for (nom, attendu) in [("pp", 1_i64), ("np", -1), ("pn", 1), ("nn", -1)] {
        let r = lignes(
            &db,
            &format!("MATCH (m:M {{nom: '{nom}'}}) RETURN m.a % m.b"),
        );
        assert_eq!(
            r[0][0],
            Value::Int64(attendu),
            "le reste de {nom} vaut {attendu} : son signe est celui du \
             DIVIDENDE, pas celui du diviseur"
        );
    }

    // Et l'identité qui rend ce choix obligatoire.
    for nom in ["pp", "np", "pn", "nn"] {
        let r = lignes(
            &db,
            &format!("MATCH (m:M {{nom: '{nom}'}}) RETURN (m.a / m.b) * m.b + m.a % m.b"),
        );
        let a = lignes(&db, &format!("MATCH (m:M {{nom: '{nom}'}}) RETURN m.a"));
        assert_eq!(
            r[0][0], a[0][0],
            "(a / b) * b + a % b doit reconstituer a — c'est ce qui lie la \
             troncature de la division au signe du reste ({nom})"
        );
    }
}

/// Le modulo par zéro est aussi indéfini que la division par zéro, et doit
/// être refusé de la même manière. Un NULL silencieux serait pire qu'une
/// erreur : il ferait disparaître la faute au lieu de la signaler, et se
/// confondrait avec une propriété absente.
// corrigé par ce commit — cause d'origine : crates/obrain-core/src/execution/operators/filter.rs:1503 —
// `eval_modulo` garde bien le diviseur nul (`if *b != 0`), mais retombe sur
// `_ => None`, que l'évaluateur traduit en NULL. Le modulo par zéro devient
// donc indiscernable d'une propriété absente : la faute est effacée au lieu
// d'être signalée. À comparer avec check_eval.rs:797, qui rend
// `Err("modulo by zero")`.
#[test]
fn le_modulo_par_zero_est_refuse_et_non_silencieux() {
    let db = base_ancree();
    let issue = evalue(&db, "1 % 0");
    assert!(
        !matches!(&issue, Issue::Panique),
        "le modulo par zéro ne doit pas paniquer — obtenu {issue:?}"
    );
    assert!(
        issue.est_erreur(),
        "`1 % 0` est indéfini : le moteur doit refuser la requête, et non \
         rendre un NULL qui se confondrait avec une valeur absente — \
         obtenu {issue:?}"
    );
}

/// Le modulo flottant est le reste IEEE : `7.5 % 2` vaut 1.5, il ne tronque
/// pas ses opérandes en entiers avant d'opérer.
#[test]
fn le_modulo_flottant_ne_tronque_pas_ses_operandes() {
    let db = base_ancree();
    let v = val(&db, "7.5 % 2");
    assert_eq!(
        flottant(&v, "7.5 % 2"),
        1.5,
        "7.5 % 2 vaut 1.5 : une troncature préalable à 7 % 2 rendrait 1"
    );
}

// ===========================================================================
// GROUPE 4 — le DÉBORDEMENT i64
// ===========================================================================

/// Un débordement d'addition doit être SIGNALÉ. L'enroulement silencieux est
/// la pire des issues : `9223372036854775807 + 1` deviendrait
/// `-9223372036854775808`, un nombre négatif rendu pour une somme de deux
/// positifs, et rien dans le résultat ne dirait que le calcul est faux.
// corrigé par ce commit — cause d'origine : crates/obrain-core/src/execution/operators/filter.rs:1314 passe
// `|a, b| a + b` brut à `eval_arithmetic` (filter.rs:1487), sans
// `checked_add`. ET LE PROFIL DE COMPILATION CHANGE LA NATURE DU DÉFAUT :
// `[profile.release]` (Cargo.toml:186) n'active pas `overflow-checks`, donc
// ce qui panique ici, sous le profil de test, ENROULE EN SILENCE en
// production — `i64::MAX + 1` y rend un nombre négatif sans rien signaler.
// La version gardée existe à check_eval.rs:788 (`checked_add`).
#[test]
fn l_addition_qui_deborde_i64_est_signalee_et_jamais_enroulee() {
    let db = base_ancree();
    let issue = evalue(&db, "9223372036854775807 + 1");
    if let Issue::Valeur(Value::Int64(n)) = &issue {
        assert!(
            *n > 0,
            "la somme de deux entiers positifs ne peut pas être négative : \
             l'enroulement a rendu {n}"
        );
    }
    assert!(
        !matches!(&issue, Issue::Panique),
        "un débordement est une donnée d'entrée, pas un bogue : il doit être \
         refusé proprement, jamais paniquer — obtenu {issue:?}"
    );
    assert!(
        issue.est_erreur(),
        "la somme dépasse la capacité d'un entier 64 bits : la requête doit \
         être refusée — obtenu {issue:?}"
    );
}

/// Même exigence pour la multiplication et pour la soustraction par le bas.
// corrigé par ce commit — cause d'origine : crates/obrain-core/src/execution/operators/filter.rs:1351 (`*`) et
// filter.rs:1345 (`-`), même schéma que l'addition ci-dessus : opérateur brut,
// aucun `checked_mul` / `checked_sub`, et enroulement silencieux sous
// `[profile.release]`.
#[test]
fn le_produit_et_la_difference_qui_debordent_i64_sont_signales() {
    let db = base_ancree();
    for (expr, pourquoi) in [
        (
            "9223372036854775807 * 2",
            "le produit de deux positifs ne peut pas être négatif",
        ),
        (
            "-9223372036854775807 - 2",
            "la différence de deux négatifs ne peut pas être positive",
        ),
    ] {
        let issue = evalue(&db, expr);
        assert!(
            !matches!(&issue, Issue::Panique),
            "{expr} : un débordement ne doit pas paniquer — obtenu {issue:?}"
        );
        assert!(
            issue.est_erreur(),
            "{expr} déborde la capacité d'un entier 64 bits et doit être \
             refusé ({pourquoi}) — obtenu {issue:?}"
        );
    }
}

/// LA LACUNE SIGNALÉE PAR LE LOT 07 : `sum` accumule sans aucune garde. Une
/// somme qui déborde doit être refusée, jamais enroulée. Le danger est ici
/// bien plus grand qu'en arithmétique d'expression : personne ne regarde
/// deux fois un `sum`, et une somme enroulée passe pour un résultat.
///
/// Le jeu est minimal — deux lignes — pour qu'aucune autre cause ne puisse
/// expliquer l'échec : `i64::MAX` puis 1.
// corrigé par ce commit — cause d'origine : crates/obrain-core/src/execution/operators/aggregate.rs:191 —
// `*sum += v`, sans la moindre garde. C'est la lacune que le lot 07 avait
// signalée sans la couvrir. Elle est PLUS GRAVE que celle de l'arithmétique
// d'expression : sous `[profile.release]` (Cargo.toml:186, pas
// d'`overflow-checks`), une somme qui déborde rend un nombre négatif que
// rien ne distingue d'un résultat légitime, et personne ne vérifie un `sum`
// à la main. Les variantes voisines sont touchées de la même façon :
// aggregate.rs:208 (`sum DISTINCT`), et `count` à aggregate.rs:180.
#[test]
fn sum_qui_deborde_i64_est_signalee_et_jamais_enroulee() {
    let db = ObrainDB::new_in_memory();
    {
        let mut s = db.session();
        s.begin_transaction().unwrap();
        s.execute("INSERT (:B {v: 9223372036854775807})").unwrap();
        s.execute("INSERT (:B {v: 1})").unwrap();
        s.commit().unwrap();
    }
    // Garde-fou : sans les deux lignes, tout ce qui suit ne mesurerait rien.
    assert_eq!(
        lignes(&db, "MATCH (b:B) RETURN count(*)")[0][0],
        Value::Int64(2),
        "les deux lignes doivent exister, sinon la somme ne déborde pas"
    );

    let q = "MATCH (b:B) RETURN sum(b.v)";
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| db.session().execute(q)));
    match r {
        Err(_) => panic!(
            "un débordement de `sum` ne doit pas PANIQUER : c'est une donnée \
             d'entrée ordinaire — {q}"
        ),
        Ok(Ok(res)) => {
            if let Some(Value::Int64(n)) = res.rows.first().map(|l| l[0].clone()) {
                panic!(
                    "`sum` a rendu {n} pour 9223372036854775807 + 1 : la somme \
                     de deux entiers positifs a été ENROULÉE en silence. Elle \
                     doit être refusée."
                );
            }
            panic!(
                "la somme déborde la capacité d'un entier 64 bits : la requête \
                 doit être refusée — obtenu {:?}",
                res.rows
            );
        }
        Ok(Err(_)) => { /* refusée : c'est l'issue attendue */ }
    }
}

/// La borne BASSE de `i64` doit être écrivable en littéral. `i64` couvre
/// -9223372036854775808..=9223372036854775807 ; refuser sa borne basse
/// rendrait une partie du domaine du type inatteignable depuis GQL, et
/// obligerait à la contourner par un calcul.
// corrigé par ce commit — cause d'origine : crates/obrain-adapters/src/query/gql/parser.rs:3404 analyse le
// littéral SANS son moins unaire — l'analyseur lexical n'intègre pas le signe
// au nombre (lexer.rs:557 `scan_number` démarre sur un chiffre). Le parseur
// voit donc `9223372036854775808`, qui dépasse `i64::MAX` d'une unité, et
// échoue en `Invalid integer` (parser.rs:3406) AVANT que le moins ne soit
// appliqué. Résultat : la borne basse du type entier est inatteignable en
// littéral, alors qu'elle appartient à son domaine et se stocke sans
// problème par l'API (cf. le test de division ci-dessus, qui la sème
// directement).
#[test]
fn la_borne_basse_de_i64_est_un_litteral_ecrivable() {
    let db = base_ancree();
    let issue = evalue(&db, "-9223372036854775808");
    assert!(
        !matches!(&issue, Issue::Panique),
        "-9223372036854775808 ne doit pas faire paniquer l'analyseur"
    );
    assert_eq!(
        issue.valeur("-9223372036854775808"),
        Value::Int64(i64::MIN),
        "la borne basse du type entier fait partie de son domaine et doit \
         donc s'écrire littéralement"
    );
}

/// Un littéral entier HORS des bornes de `i64` doit être refusé
/// explicitement. Le silence serait pire : une troncature ou une conversion
/// tacite en flottant changerait la valeur écrite par l'auteur de la requête
/// sans le lui dire.
#[test]
fn un_litteral_entier_hors_bornes_est_refuse_et_non_tronque() {
    let db = base_ancree();
    let issue = evalue(&db, "9223372036854775808");
    if let Issue::Valeur(v) = &issue {
        panic!(
            "9223372036854775808 dépasse i64 : le moteur devait refuser le \
             littéral, il a rendu {v:?} — la valeur écrite n'est pas celle lue"
        );
    }
    assert!(
        issue.est_erreur(),
        "un littéral hors bornes doit être refusé explicitement — obtenu {issue:?}"
    );
}

/// `i64::MIN / -1` est le seul quotient d'entiers qui déborde : son résultat
/// mathématique, 9223372036854775808, dépasse la borne haute. Il doit être
/// signalé comme tout autre débordement — en Rust, cette division panique
/// dans tous les profils.
// corrigé par ce commit — cause d'origine : crates/obrain-core/src/execution/operators/filter.rs:1357 —
// « attempt to divide with overflow ». C'est le seul quotient d'entiers qui
// déborde, et il n'est pas plus gardé que la division par zéro.
#[test]
fn la_division_de_la_borne_basse_par_moins_un_est_signalee() {
    let db = ObrainDB::new_in_memory();
    {
        let mut s = db.session();
        s.begin_transaction().unwrap();
        let _ = s.create_node_with_props(
            &["Q"],
            [("a", Value::Int64(i64::MIN)), ("b", Value::Int64(-1))],
        );
        s.commit().unwrap();
    }
    let q = "MATCH (q:Q) RETURN q.a / q.b";
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| db.session().execute(q)));
    assert!(
        r.is_ok(),
        "i64::MIN / -1 déborde : ce doit être une erreur, jamais une panique — {q}"
    );
    match r.unwrap() {
        Ok(res) => panic!(
            "i64::MIN / -1 vaut 9223372036854775808, hors du domaine entier : \
             la requête devait être refusée, elle a rendu {:?}",
            res.rows
        ),
        Err(_) => { /* refusée : c'est l'issue attendue */ }
    }
}

// ===========================================================================
// GROUPE 5 — la PRÉCISION
// ===========================================================================

/// Un entier au-delà de `2^53` doit survivre à l'aller-retour par le
/// stockage. `9007199254740993` est le premier entier que `f64` ne sait pas
/// représenter : s'il ressort égal à `9007199254740992`, c'est qu'il est
/// passé par un flottant quelque part sur le trajet.
#[test]
fn un_entier_au_dela_de_deux_puissance_cinquante_trois_survit_au_stockage() {
    let db = ObrainDB::new_in_memory();
    {
        let mut s = db.session();
        s.begin_transaction().unwrap();
        s.execute("INSERT (:P {v: 9007199254740993})").unwrap();
        s.commit().unwrap();
    }
    assert_eq!(
        lignes(&db, "MATCH (p:P) RETURN p.v")[0][0],
        Value::Int64(9007199254740993),
        "2^53+1 n'est pas représentable en f64 : le relire comme \
         9007199254740992 prouverait un passage par le flottant"
    );
}

/// L'arithmétique entière reste EXACTE au-delà de `2^53`. Le voisinage de
/// `2^53` est choisi exprès : c'est précisément là qu'un calcul routé par le
/// flottant se met à mentir, et nulle part avant.
#[test]
fn l_arithmetique_entiere_reste_exacte_au_dela_de_deux_puissance_cinquante_trois() {
    let db = base_ancree();
    for (expr, attendu) in [
        ("9007199254740992 + 1", 9007199254740993_i64),
        ("9007199254740993 + 1", 9007199254740994),
        ("9007199254740993 * 2", 18014398509481986),
        ("9007199254740995 - 2", 9007199254740993),
    ] {
        assert_eq!(
            val(&db, expr),
            Value::Int64(attendu),
            "{expr} vaut exactement {attendu} : un calcul passé par f64 \
             rendrait un voisin pair"
        );
    }
}

/// `sum` sur une colonne ENTIÈRE reste exacte au-delà de `2^53`.
///
/// Le lot 07 vérifie déjà que `sum` d'entiers reste entière, mais sur le jeu
/// `1, 2, 3` : une implémentation qui promouvrait tout en `f64` y passerait
/// sans broncher, puisque 6 est représentable. Le jeu ci-dessous est choisi
/// pour que la promotion soit DÉTECTABLE — `2^53+1` ne l'est pas.
#[test]
fn sum_sur_colonne_entiere_reste_exacte_au_dela_de_deux_puissance_cinquante_trois() {
    let db = ObrainDB::new_in_memory();
    {
        let mut s = db.session();
        s.begin_transaction().unwrap();
        s.execute("INSERT (:E {v: 9007199254740993})").unwrap();
        s.execute("INSERT (:E {v: 1})").unwrap();
        s.commit().unwrap();
    }
    assert_eq!(
        lignes(&db, "MATCH (e:E) RETURN sum(e.v)")[0][0],
        Value::Int64(9007199254740994),
        "2^53+1 plus 1 vaut 2^53+2, exactement : une somme accumulée en f64 \
         rendrait Float64 et perdrait l'unité"
    );
}

/// `min` et `max` restituent l'entier EXACT, sans passer par le flottant.
/// Deux entiers consécutifs au-delà de `2^53` sont indiscernables une fois
/// convertis en `f64` : si la comparaison interne y passe, `min` peut rendre
/// le mauvais des deux, ou rendre un flottant.
#[test]
fn min_et_max_restituent_l_entier_exact_au_dela_de_deux_puissance_cinquante_trois() {
    let db = ObrainDB::new_in_memory();
    {
        let mut s = db.session();
        s.begin_transaction().unwrap();
        s.execute("INSERT (:X {v: 9007199254740993})").unwrap();
        s.execute("INSERT (:X {v: 9007199254740992})").unwrap();
        s.commit().unwrap();
    }
    let r = lignes(&db, "MATCH (x:X) RETURN min(x.v), max(x.v)");
    assert_eq!(
        r[0][0],
        Value::Int64(9007199254740992),
        "le minimum est 2^53, restitué exactement et sous forme entière"
    );
    assert_eq!(
        r[0][1],
        Value::Int64(9007199254740993),
        "le maximum est 2^53+1 : les deux valeurs sont distinctes en entier, \
         et confondues seulement si on les fait passer par f64"
    );
}

/// L'addition flottante est celle d'IEEE-754, BIT POUR BIT. `0.1 + 0.2` vaut
/// `0.30000000000000004`, pas `0.3` : un moteur qui rendrait `0.3` aurait
/// arrondi le résultat en passant par le décimal, et deviendrait imprévisible
/// dès qu'on enchaîne les opérations.
#[test]
fn l_addition_flottante_est_celle_d_ieee_bit_pour_bit() {
    let db = base_ancree();
    let v = val(&db, "0.1 + 0.2");
    let f = flottant(&v, "0.1 + 0.2");
    assert_eq!(
        f.to_bits(),
        (0.1_f64 + 0.2_f64).to_bits(),
        "0.1 + 0.2 doit être la somme IEEE-754 exacte, bit pour bit — obtenu {f}"
    );
    assert_ne!(
        f, 0.3_f64,
        "0.1 + 0.2 ne vaut PAS 0.3 en binaire64 ; l'obtenir signalerait un \
         arrondi décimal intercalé"
    );
}

// ===========================================================================
// GROUPE 6 — très GRANDS et très PETITS nombres
// ===========================================================================

/// Le dépassement de capacité flottant se SIGNALE par l'infini, avec le bon
/// signe. Le format de stockage est le binaire64 : `1e308 * 10` n'a pas de
/// représentation finie, et IEEE-754 impose ±∞. Rendre un fini quelconque
/// serait un résultat faux présenté comme exact.
#[test]
fn le_depassement_flottant_se_signale_par_l_infini_signe() {
    let db = base_ancree();
    let v = val(&db, "1e308 * 10");
    let f = flottant(&v, "1e308 * 10");
    assert!(
        f.is_infinite() && f.is_sign_positive(),
        "1e308 * 10 dépasse la capacité du binaire64 : IEEE rend +∞ — obtenu {f}"
    );

    let v = val(&db, "-1e308 * 10");
    let f = flottant(&v, "-1e308 * 10");
    assert!(
        f.is_infinite() && f.is_sign_negative(),
        "le dépassement conserve le signe : -∞ — obtenu {f}"
    );
}

/// Un très petit nombre reste NON NUL tant qu'il est représentable. `1e-320`
/// est un sous-normal du binaire64 : le rabattre à zéro perdrait l'ordre de
/// grandeur, et ferait passer une valeur positive pour un zéro.
#[test]
fn un_sous_normal_reste_non_nul() {
    let db = base_ancree();
    let v = val(&db, "1e-320");
    let f = flottant(&v, "1e-320");
    assert!(
        f > 0.0,
        "1e-320 est représentable en binaire64 (sous-normal) et vaut donc \
         strictement plus que zéro — obtenu {f}"
    );
    assert_eq!(
        f.to_bits(),
        1e-320_f64.to_bits(),
        "et c'est le sous-normal exact, bit pour bit"
    );
}

/// Le souspassement va à zéro EN GARDANT LE SIGNE. La moitié du plus petit
/// sous-normal n'est pas représentable ; IEEE-754 l'arrondit au zéro du bon
/// signe, et ce signe reste observable par la division.
#[test]
fn le_souspassement_va_au_zero_du_bon_signe() {
    let db = ObrainDB::new_in_memory();
    {
        let mut s = db.session();
        s.begin_transaction().unwrap();
        let _ = s.create_node_with_props(&["S"], [("v", Value::Float64(-5e-324))]);
        s.commit().unwrap();
    }
    let v = lignes(&db, "MATCH (s:S) RETURN s.v / 2")[0][0].clone();
    let f = flottant(&v, "s.v / 2");
    assert_eq!(
        f, 0.0,
        "la moitié du plus petit sous-normal souspasse à zéro"
    );
    assert!(
        f.is_sign_negative(),
        "et c'est le zéro NÉGATIF : le signe survit au souspassement — \
         obtenu un zéro de signe positif"
    );
}

// ===========================================================================
// GROUPE 7 — le ZÉRO NÉGATIF
// ===========================================================================

/// Le zéro négatif survit au stockage et reste DISTINGUABLE du zéro positif.
/// Les deux sont égaux au sens de `=`, ce qui rend la division leur seul
/// témoin observable : `1.0 / -0.0` vaut -∞ et `1.0 / 0.0` vaut +∞. Perdre
/// le bit de signe à l'écriture ferait basculer un résultat ultérieur de
/// -∞ à +∞ sans qu'aucune comparaison ne puisse le détecter.
#[test]
fn le_zero_negatif_survit_au_stockage_et_reste_distinguable() {
    let db = ObrainDB::new_in_memory();
    {
        let mut s = db.session();
        s.begin_transaction().unwrap();
        let _ = s.create_node_with_props(&["N"], [("v", Value::Float64(-0.0))]);
        let _ = s.create_node_with_props(&["P"], [("v", Value::Float64(0.0))]);
        s.commit().unwrap();
    }
    let n = lignes(&db, "MATCH (n:N) RETURN 1.0 / n.v")[0][0].clone();
    let f = flottant(&n, "1.0 / (-0.0)");
    assert!(
        f.is_infinite() && f.is_sign_negative(),
        "le zéro stocké était NÉGATIF : 1.0 divisé par lui vaut -∞. Obtenir \
         +∞ prouverait que le bit de signe a été perdu à l'écriture — obtenu {f}"
    );

    let p = lignes(&db, "MATCH (p:P) RETURN 1.0 / p.v")[0][0].clone();
    let g = flottant(&p, "1.0 / 0.0");
    assert!(
        g.is_infinite() && g.is_sign_positive(),
        "et le zéro positif donne bien +∞ — sinon les deux zéros seraient \
         confondus dans l'autre sens"
    );
}

/// Un produit qui s'annule garde le signe du produit des signes : `0.0 * -1`
/// vaut -0.0. C'est la règle IEEE, et c'est ce qui rend le zéro négatif
/// atteignable par le calcul et pas seulement par l'écriture littérale.
#[test]
fn un_produit_nul_de_signes_opposes_donne_le_zero_negatif() {
    let db = base_ancree();
    let v = val(&db, "0.0 * -1");
    let f = flottant(&v, "0.0 * -1");
    assert_eq!(f, 0.0, "le produit est nul");
    assert!(
        f.is_sign_negative(),
        "et son signe est négatif : 0.0 * -1 vaut -0.0 en IEEE-754 — obtenu \
         un zéro de signe positif"
    );
}

// ===========================================================================
// GROUPE 8 — NaN et INFINIS dans l'ARITHMÉTIQUE
// ===========================================================================

/// NaN se PROPAGE : toute opération arithmétique qui en reçoit un en rend un.
/// C'est ce qui rend l'indétermination traçable jusqu'au résultat au lieu de
/// la voir disparaître au milieu d'un calcul.
#[test]
fn nan_se_propage_a_travers_l_arithmetique() {
    let db = ObrainDB::new_in_memory();
    {
        let mut s = db.session();
        s.begin_transaction().unwrap();
        let _ = s.create_node_with_props(&["W"], [("v", Value::Float64(f64::NAN))]);
        s.commit().unwrap();
    }
    for expr in ["w.v + 1", "w.v - 1", "w.v * 0", "w.v / 2", "1 / w.v"] {
        let v = lignes(&db, &format!("MATCH (w:W) RETURN {expr}"))[0][0].clone();
        let f = flottant(&v, expr);
        assert!(
            f.is_nan(),
            "{expr} : NaN contamine toute opération arithmétique — même \
             multiplié par zéro. Obtenu {f}"
        );
    }
}

/// Les infinis suivent l'arithmétique étendue d'IEEE : l'infini absorbe
/// l'addition d'un fini, mais `∞ - ∞` et `∞ * 0` sont indéterminés et rendent
/// NaN. Un moteur qui rendrait 0 pour `∞ - ∞` inventerait une réponse là où
/// il n'y en a pas.
#[test]
fn l_arithmetique_des_infinis_suit_ieee() {
    let db = ObrainDB::new_in_memory();
    {
        let mut s = db.session();
        s.begin_transaction().unwrap();
        let _ = s.create_node_with_props(
            &["I"],
            [
                ("p", Value::Float64(f64::INFINITY)),
                ("n", Value::Float64(f64::NEG_INFINITY)),
            ],
        );
        s.commit().unwrap();
    }
    let r = lignes(
        &db,
        "MATCH (i:I) RETURN i.p + 1, i.p + i.p, i.p - i.p, i.p * 0, i.p + i.n",
    );
    let l = &r[0];

    let f = flottant(&l[0], "∞ + 1");
    assert!(
        f.is_infinite() && f.is_sign_positive(),
        "∞ + 1 reste +∞ : l'infini absorbe l'addition d'un fini — obtenu {f}"
    );
    let f = flottant(&l[1], "∞ + ∞");
    assert!(
        f.is_infinite() && f.is_sign_positive(),
        "∞ + ∞ vaut +∞ — obtenu {f}"
    );
    let f = flottant(&l[2], "∞ - ∞");
    assert!(
        f.is_nan(),
        "∞ - ∞ est indéterminé : IEEE rend NaN, et surtout pas 0 — obtenu {f}"
    );
    let f = flottant(&l[3], "∞ * 0");
    assert!(
        f.is_nan(),
        "∞ * 0 est indéterminé : IEEE rend NaN, et surtout pas 0 — obtenu {f}"
    );
    let f = flottant(&l[4], "∞ + (-∞)");
    assert!(
        f.is_nan(),
        "l'addition de deux infinis opposés est indéterminée — obtenu {f}"
    );
}

/// `sum` ne doit pas ESCAMOTER un NaN. Une somme dont un terme est
/// indéterminé est indéterminée : la voir rendre un nombre fini ferait passer
/// une colonne corrompue pour une colonne saine.
#[test]
fn sum_d_une_colonne_contenant_nan_rend_nan() {
    let db = ObrainDB::new_in_memory();
    {
        let mut s = db.session();
        s.begin_transaction().unwrap();
        let _ = s.create_node_with_props(&["K"], [("v", Value::Float64(1.0))]);
        let _ = s.create_node_with_props(&["K"], [("v", Value::Float64(f64::NAN))]);
        let _ = s.create_node_with_props(&["K"], [("v", Value::Float64(2.0))]);
        s.commit().unwrap();
    }
    assert_eq!(
        lignes(&db, "MATCH (k:K) RETURN count(k.v)")[0][0],
        Value::Int64(3),
        "les trois valeurs doivent être comptées — un NaN n'est pas un NULL"
    );
    let v = lignes(&db, "MATCH (k:K) RETURN sum(k.v)")[0][0].clone();
    let f = flottant(&v, "sum avec NaN");
    assert!(
        f.is_nan(),
        "un seul terme indéterminé rend la somme indéterminée : `sum` doit \
         propager le NaN et non le sauter — obtenu {f}"
    );
}

// ===========================================================================
// GROUPE 9 — les LITTÉRAUX et leur type
// ===========================================================================

/// Un littéral à EXPOSANT est flottant, même quand sa valeur est entière.
/// `1e3` s'écrit sans point décimal mais la notation scientifique appartient
/// aux approximatifs : le type se lit dans la NOTATION, pas dans la valeur.
#[test]
fn un_litteral_a_exposant_est_flottant() {
    let db = base_ancree();
    for expr in ["1e3", "1E3", "1.5e2"] {
        let v = val(&db, expr);
        assert!(
            matches!(v, Value::Float64(_)),
            "{expr} est écrit en notation scientifique : son type est flottant, \
             quelle que soit sa valeur — obtenu {v:?}"
        );
    }
    assert_eq!(flottant(&val(&db, "1e3"), "1e3"), 1000.0);
    assert_eq!(flottant(&val(&db, "1.5e2"), "1.5e2"), 150.0);
}

/// Le TYPE du littéral survit à l'écriture puis à la relecture. `2` et `2.0`
/// désignent le même nombre mais pas la même chose : le premier est exact, le
/// second approximatif, et la différence commande le comportement de toute
/// l'arithmétique qui suivra.
#[test]
fn le_type_du_litteral_survit_a_l_ecriture() {
    let db = ObrainDB::new_in_memory();
    {
        let mut s = db.session();
        s.begin_transaction().unwrap();
        s.execute("INSERT (:L {entier: 2, flottant: 2.0})").unwrap();
        s.commit().unwrap();
    }
    let r = lignes(&db, "MATCH (l:L) RETURN l.entier, l.flottant");
    assert_eq!(
        r[0][0],
        Value::Int64(2),
        "`2` est un littéral entier et se relit entier"
    );
    assert_eq!(
        r[0][1],
        Value::Float64(2.0),
        "`2.0` est un littéral flottant et se relit flottant : le point \
         décimal n'est pas décoratif"
    );

    // Et la différence de type se voit à l'usage : la division tranche.
    let d = lignes(&db, "MATCH (l:L) RETURN l.entier / 4, l.flottant / 4");
    assert_eq!(d[0][0], Value::Int64(0), "2 / 4 sur des exacts tronque à 0");
    assert_eq!(
        flottant(&d[0][1], "2.0 / 4"),
        0.5,
        "2.0 / 4 sur un approximatif rend 0.5 — c'est bien le type stocké qui \
         a décidé, pas la valeur"
    );
}

// ===========================================================================
// GROUPE 10 — COHÉRENCE ENTRE LES CHEMINS D'EXÉCUTION
// ===========================================================================

/// L'arithmétique dans `WHERE` doit donner exactement les mêmes nombres que
/// l'arithmétique projetée dans `RETURN`.
///
/// CE TEST NE SE CONTENTE PAS DE COMPARER DEUX EXÉCUTIONS ENTRE ELLES : il
/// calcule d'abord la table de référence par projection, puis exige que
/// chaque filtre sélectionne EXACTEMENT le sous-ensemble annoncé par cette
/// table. Deux chemins tous les deux faux de la même façon échoueraient donc
/// quand même, à condition que la table de référence soit, elle, vérifiée —
/// c'est ce que fait l'assertion sur `quotients` ci-dessous.
///
/// L'intérêt porte sur les valeurs NÉGATIVES : c'est là qu'une réécriture de
/// `n.v / 2 = k` en intervalle sur `n.v` se tromperait, puisque la troncature
/// vers zéro rend le quotient non monotone au passage de zéro (les quotients
/// de -1 et de 1 valent tous les deux 0).
#[test]
fn l_arithmetique_dans_where_s_accorde_avec_l_arithmetique_projetee() {
    let db = ObrainDB::new_in_memory();
    {
        let mut s = db.session();
        s.begin_transaction().unwrap();
        for v in -5_i64..=5 {
            let _ = s.create_node_with_props(&["C"], [("v", Value::Int64(v))]);
        }
        s.commit().unwrap();
    }

    // Table de référence : le quotient projeté pour chaque v.
    let mut quotients: Vec<(i64, i64)> = lignes(&db, "MATCH (c:C) RETURN c.v, c.v / 2")
        .into_iter()
        .map(|l| match (&l[0], &l[1]) {
            (Value::Int64(v), Value::Int64(q)) => (*v, *q),
            autre => panic!("v et son quotient doivent être entiers : {autre:?}"),
        })
        .collect();
    quotients.sort_unstable();

    // La table de référence est elle-même vérifiée contre la spécification —
    // sans cela, la comparaison ci-dessous n'opposerait que deux erreurs.
    assert_eq!(
        quotients,
        vec![
            (-5, -2),
            (-4, -2),
            (-3, -1),
            (-2, -1),
            (-1, 0),
            (0, 0),
            (1, 0),
            (2, 1),
            (3, 1),
            (4, 2),
            (5, 2),
        ],
        "les quotients projetés doivent être ceux de la division tronquée \
         vers zéro : c'est la référence de tout ce qui suit"
    );

    for k in -3_i64..=3 {
        let mut attendu: Vec<i64> = quotients
            .iter()
            .filter(|(_, q)| *q == k)
            .map(|(v, _)| *v)
            .collect();
        attendu.sort_unstable();

        let mut obtenu: Vec<i64> =
            lignes(&db, &format!("MATCH (c:C) WHERE c.v / 2 = {k} RETURN c.v"))
                .into_iter()
                .map(|l| match l[0] {
                    Value::Int64(v) => v,
                    ref autre => panic!("v doit être entier : {autre:?}"),
                })
                .collect();
        obtenu.sort_unstable();

        assert_eq!(
            obtenu, attendu,
            "le filtre `c.v / 2 = {k}` doit sélectionner exactement les lignes \
             dont le quotient projeté vaut {k}"
        );
    }
}

/// Même exigence pour l'ADDITION, qui possède son propre chemin de
/// réécriture côté planificateur (`typed_degree_rewrite.rs` ne s'intéresse
/// qu'à `BinaryOp::Add`). Un décalage d'un cran introduit par une réécriture
/// de `n.v + 1 > s` en `n.v > s - 1` serait invisible sur des données
/// éloignées du seuil : les valeurs sont donc posées À LA BORNE EXACTE.
#[test]
fn l_addition_dans_where_s_accorde_avec_l_addition_projetee() {
    let db = ObrainDB::new_in_memory();
    {
        let mut s = db.session();
        s.begin_transaction().unwrap();
        for v in [1_i64, 2, 3] {
            s.execute(&format!("INSERT (:A {{v: {v}}})")).unwrap();
        }
        s.commit().unwrap();
    }

    // v + 1 vaut 2, 3, 4 : le seuil 3 tombe PILE sur la ligne v = 2.
    let mut sommes: Vec<i64> = lignes(&db, "MATCH (a:A) RETURN a.v + 1")
        .into_iter()
        .map(|l| match l[0] {
            Value::Int64(n) => n,
            ref autre => panic!("la somme doit être entière : {autre:?}"),
        })
        .collect();
    sommes.sort_unstable();
    assert_eq!(
        sommes,
        vec![2, 3, 4],
        "la projection de v + 1 est la référence du filtre ci-dessous"
    );

    let mut strict: Vec<i64> = lignes(&db, "MATCH (a:A) WHERE a.v + 1 > 3 RETURN a.v")
        .into_iter()
        .map(|l| match l[0] {
            Value::Int64(n) => n,
            ref autre => panic!("{autre:?}"),
        })
        .collect();
    strict.sort_unstable();
    assert_eq!(
        strict,
        vec![3],
        "`v + 1 > 3` exclut la ligne dont la somme vaut PILE 3 : seul v = 3 \
         (somme 4) passe"
    );

    let mut large: Vec<i64> = lignes(&db, "MATCH (a:A) WHERE a.v + 1 >= 3 RETURN a.v")
        .into_iter()
        .map(|l| match l[0] {
            Value::Int64(n) => n,
            ref autre => panic!("{autre:?}"),
        })
        .collect();
    large.sort_unstable();
    assert_eq!(
        large,
        vec![2, 3],
        "`v + 1 >= 3` inclut la ligne à la borne exacte : v = 2 rejoint v = 3"
    );
}
