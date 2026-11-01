//! Lot 10 — PAGINATION : la FENÊTRE (`LIMIT` / `SKIP`).
//!
//! Ce que ce fichier cerne : **ce qui est servi et ce qui ne l'est pas**.
//! Pas le tri en propre (lot 09), pas la nullité en propre (lot 04), pas les
//! agrégats en propre (lot 07), pas DISTINCT en propre (lot 08) — mais leur
//! INTERACTION avec la fenêtre, là où la fenêtre décide du contenu.
//!
//! ## Référence normative
//!
//! ISO/IEC 39075:2024 (GQL), clause 14.13 « limit clause » et 14.12 « offset
//! clause » : l'opérande est un *unsigned integer*, la fenêtre s'applique
//! APRÈS `ORDER BY`, et l'ordre canonique d'une requête composée est
//! `ORDER BY` → `OFFSET`/`SKIP` → `LIMIT`. Un opérande négatif n'est pas
//! représentable dans la grammaire : le moteur doit le refuser, jamais le
//! réinterpréter.
//!
//! ## Règle d'écriture
//!
//! Chaque assertion est écrite depuis la spécification, jamais depuis
//! l'observation. Lorsque le moteur diverge, l'assertion RESTE conforme au
//! texte normatif et le cas est placé derrière la feature `defauts_connus`,
//! précédé d'un commentaire `// DÉFAUT:` nommant fichier:ligne et d'un
//! `// SYMPTÔME:` décrivant ce que le moteur rend aujourd'hui.
//!
//! * `cargo test -p obrain-engine --test gql_10_pagination`
//!   → vert : les cas de défaut ne sont pas même compilés.
//! * `… --cfg defauts_connus (RUSTFLAGS)`
//!   → les cas de défaut tournent et ÉCHOUENT, légitimement : ils énoncent ce
//!     que la spécification exige et que le moteur ne fait pas encore.
#![cfg(feature = "gql")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use obrain_engine::database::QueryResult;
use std::collections::HashMap;

// ─────────────────────────────────────────────────────────────────────────
//  Outillage
// ─────────────────────────────────────────────────────────────────────────

fn as_i64(v: &Value) -> i64 {
    match v {
        Value::Int64(i) => *i,
        other => panic!("Int64 attendu, obtenu {other:?}"),
    }
}

/// Extrait la colonne 0 en `i64`. Panique sur un NULL — les cas qui
/// attendent des NULL utilisent [`col0_opt`].
fn col0(r: &QueryResult) -> Vec<i64> {
    r.rows.iter().map(|row| as_i64(&row[0])).collect()
}

/// Extrait la colonne 0 en `Option<i64>`, `None` pour un NULL.
fn col0_opt(r: &QueryResult) -> Vec<Option<i64>> {
    r.rows
        .iter()
        .map(|row| match &row[0] {
            Value::Null => None,
            v => Some(as_i64(v)),
        })
        .collect()
}

/// Sème `n` nœuds `:Page` portant `v = 0..n`, insérés dans un ordre de scan
/// volontairement différent de l'ordre trié, pour qu'un `LIMIT` qui
/// tronquerait avant de trier se trahisse.
fn semer_desordonne(n: i64) -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    // Parcours par pas de 7 modulo n : couvre 0..n exactement une fois dès que
    // pgcd(7, n) == 1, et ne présente jamais les valeurs dans l'ordre.
    let mut ordre: Vec<i64> = (0..n).map(|i| (i * 7) % n).collect();
    if ordre.iter().collect::<std::collections::HashSet<_>>().len() != n as usize {
        // n multiple de 7 : repli sur un simple renversement.
        ordre = (0..n).rev().collect();
    }
    for v in ordre {
        s.execute(&format!("INSERT (:Page {{v: {v}}})")).unwrap();
    }
    db
}

// ─────────────────────────────────────────────────────────────────────────
//  A. Bornes dégénérées de la fenêtre
// ─────────────────────────────────────────────────────────────────────────

/// `LIMIT 0` : la fenêtre est vide, mais la requête reste une requête — le
/// descripteur de colonnes doit survivre. GQL 14.13 : `LIMIT 0` est licite.
#[test]
fn limit_zero_rend_zero_ligne_en_gardant_les_colonnes() {
    let db = semer_desordonne(10);
    let s = db.session();

    let r = s.execute("MATCH (n:Page) RETURN n.v AS v LIMIT 0").unwrap();
    assert_eq!(r.rows.len(), 0, "LIMIT 0 ne peut servir aucune ligne");
    assert_eq!(
        r.columns,
        vec!["v".to_string()],
        "LIMIT 0 vide les lignes, pas l'en-tête : le descripteur de colonnes \
         est déterminé par RETURN, jamais par la cardinalité"
    );
}

/// `LIMIT 0` sous un `ORDER BY` : trier zéro ligne reste zéro ligne.
///
/// PORTÉE RÉELLE, établie par mutation (M5). On pourrait croire ce cas
/// couvrir `sort.rs:207` (`Some(0) => rows.clear()`). Il ne le couvre PAS :
/// neutraliser cette ligne laisse le test vert. La raison est en amont —
/// `plan_limit` garde ses descentes derrière `limit.count.value() > 0`
/// (`project.rs:495` et `:549`), si bien qu'un `LIMIT 0` n'emprunte JAMAIS la
/// descente dans le tri : le tri s'exécute sans borne et c'est le
/// `LimitOperator` de tête qui rend zéro ligne. Ce cas mesure donc le
/// CONTRAT de sortie de `LIMIT 0` sous tri, pas la branche `Some(0)` du tri
/// borné — laquelle reste sans couverture depuis le chemin GQL.
#[test]
fn limit_zero_sous_order_by_rend_zero_ligne() {
    let db = semer_desordonne(10);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC LIMIT 0")
        .unwrap();
    assert_eq!(
        r.rows.len(),
        0,
        "ORDER BY … LIMIT 0 ne peut servir aucune ligne"
    );
}

/// `SKIP 0` est l'identité : la fenêtre commence au premier rang.
#[test]
fn skip_zero_est_l_identite() {
    let db = semer_desordonne(10);
    let s = db.session();

    let sans = s
        .execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC")
        .unwrap();
    let avec = s
        .execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC SKIP 0")
        .unwrap();

    assert_eq!(
        col0(&avec),
        col0(&sans),
        "SKIP 0 ne retranche rien : la fenêtre doit être le résultat entier"
    );
    assert_eq!(col0(&avec), (0..10).collect::<Vec<_>>());
}

/// `SKIP 0 LIMIT 0` : les deux bornes dégénérées ensemble.
#[test]
fn skip_zero_limit_zero_rend_zero_ligne() {
    let db = semer_desordonne(10);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC SKIP 0 LIMIT 0")
        .unwrap();
    assert_eq!(r.rows.len(), 0, "LIMIT 0 prime, quel que soit le SKIP");
}

/// Un `LIMIT` au-delà du total ne peut pas inventer de lignes ni en perdre.
#[test]
fn limit_au_dela_du_total_rend_tout() {
    let db = semer_desordonne(13);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC LIMIT 1000")
        .unwrap();
    assert_eq!(
        col0(&r),
        (0..13).collect::<Vec<_>>(),
        "une fenêtre plus large que le résultat sert le résultat entier"
    );
}

/// `LIMIT` exactement égal au total : la frontière `k == n`, celle où
/// `sort.rs:213` bascule du tas borné vers le tri complet. Les deux chemins
/// doivent rendre la même chose.
#[test]
fn limit_egal_au_total_rend_tout() {
    let db = semer_desordonne(13);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC LIMIT 13")
        .unwrap();
    assert_eq!(col0(&r), (0..13).collect::<Vec<_>>());
}

/// `LIMIT n-1` : la première valeur de `k` qui fait réellement mordre le tas
/// borné (`sort.rs:216`). Elle doit coïncider avec le préfixe du tri complet.
#[test]
fn limit_juste_sous_le_total_est_le_prefixe_du_tri_complet() {
    let db = semer_desordonne(13);
    let s = db.session();

    let complet = s
        .execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC")
        .unwrap();
    let borne = s
        .execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC LIMIT 12")
        .unwrap();

    assert_eq!(
        col0(&borne),
        col0(&complet)[..12].to_vec(),
        "le chemin borné doit rendre le préfixe exact du tri complet"
    );
}

/// `SKIP` égal au total : la fenêtre commence juste après la dernière ligne.
#[test]
fn skip_egal_au_total_rend_zero_ligne() {
    let db = semer_desordonne(10);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC SKIP 10")
        .unwrap();
    assert_eq!(r.rows.len(), 0, "sauter tout le résultat ne laisse rien");
}

/// `SKIP` au-delà du total : idem, sans erreur ni débordement.
#[test]
fn skip_au_dela_du_total_rend_zero_ligne() {
    let db = semer_desordonne(10);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC SKIP 1000 LIMIT 5")
        .unwrap();
    assert_eq!(
        r.rows.len(),
        0,
        "un SKIP au-delà du total vide la fenêtre ; le LIMIT ne la repeuple pas"
    );
}

/// `SKIP total-1 LIMIT 5` : la fenêtre déborde par la droite et se fait
/// tronquer par le résultat, pas par le LIMIT.
#[test]
fn fenetre_a_cheval_sur_la_fin_est_tronquee_par_le_resultat() {
    let db = semer_desordonne(10);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC SKIP 9 LIMIT 5")
        .unwrap();
    assert_eq!(
        col0(&r),
        vec![9],
        "la fenêtre [9, 14) intersecte le résultat sur la seule ligne 9"
    );
}

/// Un `LIMIT` très grand — au bord de `i64` — doit se comporter comme
/// « aucune borne », sans déborder ni paniquer. `CountExpr::Literal` est un
/// `usize` (`plan.rs:15`) ; la conversion depuis `i64` doit rester saine.
#[test]
fn limit_tres_grand_ne_deborde_pas() {
    let db = semer_desordonne(10);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC LIMIT 9223372036854775807")
        .unwrap();
    assert_eq!(
        col0(&r),
        (0..10).collect::<Vec<_>>(),
        "un LIMIT immense est une absence de borne, pas un débordement"
    );
}

/// Un `SKIP` très grand vide la fenêtre, sans déborder ni paniquer.
#[test]
fn skip_tres_grand_ne_deborde_pas() {
    let db = semer_desordonne(10);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC SKIP 9223372036854775807")
        .unwrap();
    assert_eq!(r.rows.len(), 0, "un SKIP immense ne laisse aucune ligne");
}

// ─────────────────────────────────────────────────────────────────────────
//  B. Opérandes hors domaine — LA FENÊTRE QUI S'ÉVAPORE
//
//  GQL 14.12/14.13 : l'opérande est un *unsigned integer*. Un littéral
//  négatif est hors grammaire. Le moteur doit le REFUSER — le réinterpréter
//  silencieusement est la faute la plus grave possible sur une fenêtre, car
//  elle rend un résultat plausible et faux.
//
//  [CORRIGÉ] (fix/gql-divers) Le garde sans `else` est remplacé par
//  `translate_count_expr` aux deux sites : littéral non négatif appliqué,
//  paramètre porté par `CountExpr::Parameter` (donc APPLIQUÉ, résolu par
//  `resolve_count_param` ; manquant → erreur propre), toute autre forme —
//  négatif, arithmétique — REFUSÉE franchement. Les cinq cas du bloc sont
//  dégardés et verts. Le texte est conservé comme mémoire du mécanisme.
//
//  ## DÉFAUT DE FOND, commun aux quatre cas qui suivent
//
//  `crates/obrain-engine/src/query/translators/gql/mod.rs:1118-1128`
//  (et son jumeau exact `:480-491`) :
//
//  ```
//  if let Some(skip_expr) = &query.return_clause.skip
//      && let ast::Expression::Literal(ast::Literal::Integer(n)) = skip_expr
//  { plan = wrap_skip(plan, *n as usize); }
//  ```
//
//  Le garde n'a PAS de branche `else`. Le parseur, lui, accepte n'importe
//  quelle expression à cette place (`obrain-adapters/src/query/gql/parser.rs`
//  :2604-2618, `Some(self.parse_expression()?)` — aucune validation). Tout
//  opérande qui n'est pas un littéral entier est donc ACCEPTÉ par le parseur
//  puis JETÉ EN SILENCE par le traducteur : la clause disparaît du plan et le
//  moteur sert le résultat ENTIER.
//
//  Trois formes tombent dans ce trou :
//    * `-1` — l'unaire moins produit `Expression::Unary { op: Minus, … }`
//      (`ast.rs:1177`, `UnaryOp::Minus` `ast.rs:1374-1379`), jamais
//      `Literal::Integer(-1)` ; le garde échoue.
//    * `$k` — un paramètre est `Expression::Parameter`, alors même que le
//      plan sait le représenter (`CountExpr::Parameter`, `plan.rs:17`) : la
//      capacité existe, le traducteur ne s'en sert pas.
//    * `1 + 1` — toute arithmétique, pour la même raison.
//
//  Gravité : ce n'est pas une borne mal calculée, c'est une borne ABSENTE.
//  Une pagination applicative qui passe sa taille de page en paramètre
//  reçoit la table entière à chaque appel, sans le moindre signal.
// ─────────────────────────────────────────────────────────────────────────

/// `LIMIT -1` doit être refusé.
// DÉFAUT: `gql/mod.rs:1125-1128` — `-1` est un `Expression::Unary{Minus}`, le
// garde `Literal::Integer` échoue, `wrap_limit` n'est jamais appelé : la
// clause LIMIT s'évapore et les 10 lignes sont servies.
// SYMPTÔME: LIMIT -1 est accepté puis ignoré : la clause est jetée en silence et tout le résultat est servi
#[test]
fn limit_negatif_est_refuse() {
    let db = semer_desordonne(10);
    let s = db.session();

    let r = s.execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC LIMIT -1");
    assert!(
        r.is_err(),
        "LIMIT -1 est hors grammaire (GQL 14.13 : unsigned integer) et doit \
         être refusé ; obtenu {} ligne(s)",
        r.map(|r| r.rows.len()).unwrap_or(0)
    );
}

/// `SKIP -1` doit être refusé.
///
/// Ce cas DÉPARTAGE deux hypothèses qu'on pourrait confondre. Si le défaut
/// était la conversion `*n as usize` sur un `-1` bien parsé, le SKIP vaudrait
/// `usize::MAX` et le résultat serait VIDE. Il compte 10 lignes : la clause
/// n'a donc pas été mal convertie, elle n'a pas été traduite du tout.
// DÉFAUT: `gql/mod.rs:1118-1121` — même garde sans `else` que pour LIMIT.
// SYMPTÔME: SKIP -1 est accepté puis ignoré : la clause est jetée en silence et tout le résultat est servi
#[test]
fn skip_negatif_est_refuse() {
    let db = semer_desordonne(10);
    let s = db.session();

    let r = s.execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC SKIP -1");
    assert!(
        r.is_err(),
        "SKIP -1 est hors grammaire (GQL 14.12 : unsigned integer) et doit \
         être refusé ; obtenu {} ligne(s)",
        r.map(|r| r.rows.len()).unwrap_or(0)
    );
}

// ─────────────────────────────────────────────────────────────────────────
//  C. SKIP sans LIMIT, LIMIT sans SKIP
// ─────────────────────────────────────────────────────────────────────────

/// `SKIP` seul : suffixe du résultat trié, jusqu'au bout.
#[test]
fn skip_seul_rend_le_suffixe_entier() {
    let db = semer_desordonne(10);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC SKIP 4")
        .unwrap();
    assert_eq!(
        col0(&r),
        (4..10).collect::<Vec<_>>(),
        "SKIP sans LIMIT sert tout ce qui suit le décalage"
    );
}

/// `LIMIT` seul : préfixe du résultat trié.
#[test]
fn limit_seul_rend_le_prefixe() {
    let db = semer_desordonne(10);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC LIMIT 4")
        .unwrap();
    assert_eq!(col0(&r), (0..4).collect::<Vec<_>>());
}

/// `SKIP` et `LIMIT` composent : la fenêtre est `[skip, skip+limit)`, dans
/// cet ordre (GQL : OFFSET puis LIMIT, jamais l'inverse).
#[test]
fn skip_puis_limit_decoupe_la_fenetre_attendue() {
    let db = semer_desordonne(20);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC SKIP 5 LIMIT 6")
        .unwrap();
    assert_eq!(
        col0(&r),
        (5..11).collect::<Vec<_>>(),
        "la fenêtre est [5, 11) : SKIP décale, LIMIT compte à partir du décalage"
    );
}

// ─────────────────────────────────────────────────────────────────────────
//  E. La fenêtre sur un résultat vide
// ─────────────────────────────────────────────────────────────────────────

/// `LIMIT` sur une requête qui ne ramène rien : zéro ligne, colonnes intactes.
#[test]
fn limit_sur_resultat_vide_reste_vide_avec_ses_colonnes() {
    let db = semer_desordonne(5);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Absent) RETURN n.v AS v LIMIT 5")
        .unwrap();
    assert_eq!(r.rows.len(), 0);
    assert_eq!(
        r.columns,
        vec!["v".to_string()],
        "une fenêtre sur le vide garde l'en-tête que RETURN a déclaré"
    );
}

/// `SKIP` sur une requête vide : rien à sauter, rien à servir, pas d'erreur.
#[test]
fn skip_sur_resultat_vide_reste_vide() {
    let db = semer_desordonne(5);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Absent) RETURN n.v AS v SKIP 3 LIMIT 2")
        .unwrap();
    assert_eq!(r.rows.len(), 0);
}

/// Un filtre qui ne retient rien, puis une fenêtre : la fenêtre ne doit pas
/// « repêcher » des lignes que le filtre a écartées.
#[test]
fn limit_apres_filtre_vide_ne_repeche_rien() {
    let db = semer_desordonne(20);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Page) WHERE n.v > 999 RETURN n.v AS v LIMIT 5")
        .unwrap();
    assert_eq!(
        r.rows.len(),
        0,
        "le filtre s'applique avant la fenêtre : aucune ligne à servir"
    );
}

// ─────────────────────────────────────────────────────────────────────────
//  Boilerplate de paramètre, utilisé plus bas.
// ─────────────────────────────────────────────────────────────────────────

fn params(pairs: &[(&str, i64)]) -> HashMap<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), Value::Int64(*v)))
        .collect()
}

/// Un `LIMIT` porté par un paramètre doit borner la fenêtre. Le silence est
/// interdit : soit la borne s'applique, soit la requête est refusée. Rendre
/// TOUTES les lignes en ignorant la borne est la seule issue inacceptable.
///
/// L'assertion laisse délibérément les DEUX issues conformes ouvertes : elle
/// ne mesure donc pas « le paramètre est supporté », elle mesure « la borne
/// n'est pas trahie en silence ». C'est le contrat minimal.
// DÉFAUT: `gql/mod.rs:1125-1128` — `$k` est un `Expression::Parameter` ; le
// garde `Literal::Integer` échoue et la clause est jetée. `CountExpr::Parameter`
// (`plan.rs:17`) existe pourtant pour porter exactement ce cas : le plan sait
// le représenter, le traducteur ne le lui donne jamais.
// SYMPTÔME: LIMIT $k ignore le paramètre sans erreur : 20 lignes servies au lieu de 3
#[test]
fn limit_parametre_borne_ou_refuse_mais_ne_se_tait_pas() {
    let db = semer_desordonne(20);
    let s = db.session();

    let r = s.execute_with_params(
        "MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC LIMIT $k",
        params(&[("k", 3)]),
    );

    match r {
        Ok(res) => assert_eq!(
            res.rows.len(),
            3,
            "LIMIT $k avec k=3 doit servir 3 lignes ; en servir {} signifie \
             que la borne a été silencieusement abandonnée",
            res.rows.len()
        ),
        Err(_) => { /* refus explicite : acceptable, la borne n'est pas trahie */ }
    }
}

/// Idem pour `SKIP` : un paramètre non honoré décale la fenêtre de zéro et
/// sert un résultat plausible mais faux.
///
/// C'est ici que le défaut est le plus vicieux : le `LIMIT 4` littéral, lui,
/// est bien appliqué. Le résultat a donc la BONNE TAILLE (4 lignes) et la
/// mauvaise position — `[0,1,2,3]` au lieu de `[5,6,7,8]`. Un appelant qui
/// ne contrôle que la cardinalité de sa page ne verra jamais rien.
// DÉFAUT: `gql/mod.rs:1118-1121` — `$k` jeté ; seul le LIMIT littéral survit.
// SYMPTÔME: SKIP $k ignore le paramètre : la page a la bonne taille mais reste au rang 0
#[test]
fn skip_parametre_decale_ou_refuse_mais_ne_se_tait_pas() {
    let db = semer_desordonne(20);
    let s = db.session();

    let r = s.execute_with_params(
        "MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC SKIP $k LIMIT 4",
        params(&[("k", 5)]),
    );

    match r {
        Ok(res) => assert_eq!(
            col0(&res),
            (5..9).collect::<Vec<_>>(),
            "SKIP $k avec k=5 doit ouvrir la fenêtre au rang 5"
        ),
        Err(_) => {}
    }
}

/// Une expression arithmétique constante en position de `LIMIT` : troisième
/// manifestation du même trou. `2 + 1` est un entier non négatif parfaitement
/// déterminé ; la fenêtre doit valoir 3, ou la requête être refusée.
// DÉFAUT: `gql/mod.rs:1125-1128` — `2 + 1` est un `Expression::Binary`, le
// garde `Literal::Integer` échoue, la clause LIMIT est jetée en silence.
// SYMPTÔME: LIMIT 2+1 ignore la borne : tout le résultat est servi
#[test]
fn limit_expression_arithmetique_borne_ou_refuse_mais_ne_se_tait_pas() {
    let db = semer_desordonne(20);
    let s = db.session();

    let r = s.execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC LIMIT 2 + 1");

    match r {
        Ok(res) => assert_eq!(
            res.rows.len(),
            3,
            "LIMIT 2 + 1 vaut 3 ; en servir {} signifie que la borne a été \
             silencieusement abandonnée",
            res.rows.len()
        ),
        Err(_) => { /* refus explicite : acceptable */ }
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  D. LA FENÊTRE × L'ORDRE — l'éviction des lignes pleines
//
//  [CORRIGÉ] Le défaut décrit ci-dessous est corrigé (placement ABSOLU des
//  NULL dans les comparateurs, défaut par direction dans le planificateur) ;
//  les trois cas du bloc sont dégardés et verts. Le texte est conservé comme
//  mémoire du mécanisme.
//
//  C'est le cœur du lot 10, et c'est là que le défaut d'ordre des NULL
//  CHANGE DE NATURE.
//
//  Sans fenêtre, `ORDER BY … DESC NULLS LAST` qui place les NULL en tête est
//  un défaut de PLACEMENT : toutes les lignes sont là, mal rangées. Le lot 09
//  couvre cela.
//
//  Sous `LIMIT k`, les lignes vides OCCUPENT les k emplacements de la fenêtre
//  et ÉVINCENT les lignes pleines HORS DU RÉSULTAT. Le défaut devient un
//  défaut de SÉLECTION : des lignes ne sont plus mal rangées, elles ne sont
//  plus là. C'est ce que ce bloc cerne — la CLASSE des combinaisons touchées
//  et l'AMPLEUR de la perte.
//
//  Cause commune, mesurée une fois pour tout le bloc :
//  `crates/obrain-core/src/execution/operators/sort.rs:249-252` —
//  `compare_values_with_nulls(…, key.null_order)` (`:365-382`) encode DÉJÀ la
//  place des NULL, puis `SortDirection::Descending => cmp.reverse()` (`:251`)
//  retourne ce résultat NULL COMPRIS. Sous DESC, `NULLS LAST` produit donc
//  des NULL en tête et `NULLS FIRST` des NULL en queue : chacun fait
//  exactement l'inverse de ce qu'il annonce. Le tas borné `bounded_top_k`
//  (`:286`, appelé depuis `:216`) consomme ce comparateur tel quel — il
//  sélectionne fidèlement les k premiers d'un ordre déjà faux.
// ─────────────────────────────────────────────────────────────────────────

/// Sème `pleines` nœuds `:Rk` portant `rank = 1..=pleines` et `vides` nœuds
/// `:Rk` SANS propriété `rank` (donc `n.rank` vaut NULL). Les vides sont
/// insérés EN PREMIER, pour qu'un moteur qui se contenterait de l'ordre de
/// scan ne puisse pas passer par chance.
fn semer_avec_nulls(pleines: i64, vides: i64) -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for i in 0..vides {
        s.execute(&format!("INSERT (:Rk {{nom: 'vide{i}'}})"))
            .unwrap();
    }
    for r in 1..=pleines {
        s.execute(&format!("INSERT (:Rk {{nom: 'plein{r}', rank: {r}}})"))
            .unwrap();
    }
    db
}

/// Le cas canonique, hérité du lot 04 et ici GÉNÉRALISÉ plus bas.
///
/// 3 rangs pleins (1, 2, 3) et 2 vides. `ORDER BY rank DESC NULLS LAST`
/// ordonne `[3, 2, 1, NULL, NULL]` ; `LIMIT 2` sert donc `[3, 2]`.
// corrigé par ce commit (placement ABSOLU des NULL dans les comparateurs)
#[test]
fn desc_nulls_last_limit_sert_les_plus_grands_rangs() {
    let db = semer_avec_nulls(3, 2);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Rk) RETURN n.rank AS rank ORDER BY n.rank DESC NULLS LAST LIMIT 2")
        .unwrap();

    assert_eq!(
        col0_opt(&r),
        vec![Some(3), Some(2)],
        "NULLS LAST relègue les NULL en fin d'ordre : ils ne peuvent pas \
         occuper une fenêtre de tête. Obtenir des NULL ici signifie que des \
         lignes pleines ont été évincées du résultat."
    );
}

/// LA CLASSE, balayée. Pour `v` vides de 0 à 4 et `k` de 1 à 5, sur 5 rangs
/// pleins, `ORDER BY rank DESC NULLS LAST LIMIT k` doit servir les `min(k, 5)`
/// plus grands rangs — et JAMAIS un NULL tant que `k <= 5`.
///
/// Ce cas ne se contente pas de constater l'échec : il MESURE l'ampleur.
/// Le message d'échec rapporte, pour chaque combinaison fautive, combien de
/// lignes pleines la fenêtre a perdues.
// corrigé par ce commit (placement ABSOLU des NULL — la classe entière :
// plus aucune fenêtre de tête ne perd min(k, vides) lignes pleines)
#[test]
fn classe_desc_nulls_last_aucune_fenetre_de_tete_ne_perd_de_ligne_pleine() {
    const PLEINES: i64 = 5;
    let mut fautes: Vec<String> = Vec::new();

    for vides in 0..=4 {
        let db = semer_avec_nulls(PLEINES, vides);
        let s = db.session();

        for k in 1..=5usize {
            let r = s
                .execute(&format!(
                    "MATCH (n:Rk) RETURN n.rank AS rank \
                     ORDER BY n.rank DESC NULLS LAST LIMIT {k}"
                ))
                .unwrap();
            let obtenu = col0_opt(&r);

            // k <= PLEINES : la fenêtre de tête est entièrement pleine.
            let attendu: Vec<Option<i64>> = (0..k).map(|i| Some(PLEINES - i as i64)).collect();

            if obtenu != attendu {
                let nulls_servis = obtenu.iter().filter(|o| o.is_none()).count();
                fautes.push(format!(
                    "  vides={vides} k={k} : attendu {attendu:?}, obtenu {obtenu:?} \
                     — {nulls_servis} emplacement(s) sur {k} volé(s) par des NULL, \
                     donc {nulls_servis} ligne(s) pleine(s) évincée(s)"
                ));
            }
        }
    }

    assert!(
        fautes.is_empty(),
        "une fenêtre de tête de taille k <= {PLEINES} ne doit contenir que des \
         lignes pleines sous NULLS LAST.\n{}\n\
         Ampleur : la perte vaut min(k, vides) lignes pleines ; dès que \
         vides >= k la fenêtre ne contient AUCUNE donnée utile.",
        fautes.join("\n")
    );
}

/// Le miroir : `DESC NULLS FIRST LIMIT k` doit servir des NULL, puisque
/// NULLS FIRST les met en tête. Le même `.reverse()` les renvoie en queue,
/// si bien que cette requête-ci sert des lignes pleines — au détriment des
/// lignes vides que l'appelant demandait explicitement.
///
/// Ce cas prouve que le défaut n'est pas « les NULL sont toujours en tête »
/// mais bien « la clause NULLS est inversée sous DESC » : les deux requêtes
/// se trompent en sens contraire.
// corrigé par ce commit (placement ABSOLU des NULL dans les comparateurs)
#[test]
fn desc_nulls_first_limit_sert_les_nulls() {
    let db = semer_avec_nulls(3, 2);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Rk) RETURN n.rank AS rank ORDER BY n.rank DESC NULLS FIRST LIMIT 2")
        .unwrap();

    assert_eq!(
        col0_opt(&r),
        vec![None, None],
        "NULLS FIRST place les NULL en tête : une fenêtre de 2 sur 2 NULL ne \
         contient que des NULL"
    );
}

/// Le versant ASCENDANT, témoin de contrôle. `.reverse()` n'est PAS appliqué
/// sous ASC (`sort.rs:250`), donc l'ordre des NULL doit y être honoré et la
/// fenêtre de tête doit être correcte. Si ce cas était rouge lui aussi, la
/// cause ne serait pas `.reverse()` mais `compare_values_with_nulls`.
#[test]
fn asc_nulls_last_limit_sert_les_plus_petits_rangs() {
    let db = semer_avec_nulls(3, 2);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Rk) RETURN n.rank AS rank ORDER BY n.rank ASC NULLS LAST LIMIT 2")
        .unwrap();

    assert_eq!(
        col0_opt(&r),
        vec![Some(1), Some(2)],
        "sous ASC l'ordre des NULL est honoré : la fenêtre de tête est pleine"
    );
}

/// Second témoin ascendant : `ASC NULLS FIRST LIMIT 2` doit servir les NULL.
#[test]
fn asc_nulls_first_limit_sert_les_nulls() {
    let db = semer_avec_nulls(3, 2);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Rk) RETURN n.rank AS rank ORDER BY n.rank ASC NULLS FIRST LIMIT 2")
        .unwrap();

    assert_eq!(col0_opt(&r), vec![None, None]);
}

/// LA SÉPARATION QUI COMPTE POUR CE LOT.
///
/// L'ordre peut être faux (bloc D, lot 09) sans que la FENÊTRE soit en cause.
/// Ce cas isole le mécanisme de fenêtrage lui-même : en parcourant TOUTES les
/// pages, page par page, on doit retrouver chaque ligne exactement une fois —
/// quel que soit l'ordre dans lequel le moteur les range.
///
/// S'il est VERT alors que les cas ci-dessus sont rouges, le diagnostic est
/// net et actionnable : le découpage `SKIP`/`LIMIT` est SAIN, seule la
/// TRONCATURE d'une fenêtre partielle perd des lignes, parce qu'elle tronque
/// un ordre déjà faux. La perte n'est pas dans la pagination, elle est dans
/// ce que la pagination reçoit.
#[test]
fn parcourir_toutes_les_pages_ne_perd_aucune_ligne_meme_avec_des_nulls() {
    let db = semer_avec_nulls(5, 3);
    let s = db.session();
    let total = 8usize;

    for taille in 1..=total {
        let mut vues: Vec<Option<i64>> = Vec::new();
        let mut decalage = 0usize;
        loop {
            let r = s
                .execute(&format!(
                    "MATCH (n:Rk) RETURN n.rank AS rank \
                     ORDER BY n.rank DESC NULLS LAST SKIP {decalage} LIMIT {taille}"
                ))
                .unwrap();
            if r.rows.is_empty() {
                break;
            }
            assert!(
                r.rows.len() <= taille,
                "taille={taille} décalage={decalage} : une page ne peut excéder \
                 son LIMIT, obtenu {} lignes",
                r.rows.len()
            );
            vues.extend(col0_opt(&r));
            decalage += taille;
            assert!(decalage <= total * 2, "pagination qui ne se termine pas");
        }

        assert_eq!(
            vues.len(),
            total,
            "taille de page {taille} : le parcours complet doit rendre {total} \
             lignes, ni doublon ni omission — obtenu {}",
            vues.len()
        );

        let mut pleins: Vec<i64> = vues.iter().filter_map(|o| *o).collect();
        pleins.sort_unstable();
        assert_eq!(
            pleins,
            vec![1, 2, 3, 4, 5],
            "taille de page {taille} : les 5 rangs pleins doivent apparaître \
             chacun exactement une fois sur l'ensemble des pages"
        );
        assert_eq!(
            vues.iter().filter(|o| o.is_none()).count(),
            3,
            "taille de page {taille} : les 3 lignes vides doivent apparaître \
             chacune exactement une fois"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  F. La fenêtre et les ÉGALITÉS — stabilité de la troncature
//
//  `bounded_top_k` (`sort.rs:286`) est un TAS, pas le tri complet : c'est un
//  algorithme distinct, emprunté dès que `k < n` (`sort.rs:216`). Sa
//  documentation (`sort.rs:264-285`) affirme rendre « le préfixe exact du tri
//  complet ». Les cas qui suivent confrontent les deux chemins là où un tas
//  se trompe d'ordinaire : sur les ÉGALITÉS.
// ─────────────────────────────────────────────────────────────────────────

/// Beaucoup d'ex æquo, une fenêtre étroite. `LIMIT k` doit rendre le préfixe
/// EXACT du tri complet, ordre des ex æquo compris — c'est ce que le tri
/// stable garantit et ce que le tas borné prétend reproduire.
#[test]
fn limit_sur_ex_aequo_est_le_prefixe_exact_du_tri_complet() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    // 24 nœuds, 4 classes seulement : chaque clé est portée par 6 lignes.
    for i in 0..24i64 {
        s.execute(&format!("INSERT (:Eq {{cle: {}, id: {i}}})", i % 4))
            .unwrap();
    }

    let complet = s
        .execute("MATCH (n:Eq) RETURN n.id AS id ORDER BY n.cle ASC")
        .unwrap();
    let ids_complet = col0(&complet);

    for k in 1..24usize {
        let borne = s
            .execute(&format!(
                "MATCH (n:Eq) RETURN n.id AS id ORDER BY n.cle ASC LIMIT {k}"
            ))
            .unwrap();
        assert_eq!(
            col0(&borne),
            ids_complet[..k].to_vec(),
            "LIMIT {k} sur une clé à ex æquo doit rendre le préfixe exact du \
             tri complet ; une divergence signifie que le tas borné \
             (sort.rs:286) départage les égalités autrement que le tri stable"
        );
    }
}

/// Même exigence en descendant — le chemin où `.reverse()` intervient.
#[test]
fn limit_desc_sur_ex_aequo_est_le_prefixe_exact_du_tri_complet() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for i in 0..24i64 {
        s.execute(&format!("INSERT (:Eqd {{cle: {}, id: {i}}})", i % 4))
            .unwrap();
    }

    let complet = s
        .execute("MATCH (n:Eqd) RETURN n.id AS id ORDER BY n.cle DESC")
        .unwrap();
    let ids_complet = col0(&complet);

    for k in 1..24usize {
        let borne = s
            .execute(&format!(
                "MATCH (n:Eqd) RETURN n.id AS id ORDER BY n.cle DESC LIMIT {k}"
            ))
            .unwrap();
        assert_eq!(
            col0(&borne),
            ids_complet[..k].to_vec(),
            "LIMIT {k} descendant doit rendre le préfixe exact du tri complet"
        );
    }
}

/// `SKIP` + `LIMIT` sur des ex æquo : la fenêtre `[a, a+b)` doit être la
/// tranche exacte du tri complet. C'est la propriété dont dépend toute
/// pagination applicative sur une colonne non unique.
#[test]
fn fenetre_sur_ex_aequo_est_la_tranche_exacte_du_tri_complet() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for i in 0..24i64 {
        s.execute(&format!("INSERT (:Eqs {{cle: {}, id: {i}}})", i % 4))
            .unwrap();
    }

    let complet = s
        .execute("MATCH (n:Eqs) RETURN n.id AS id ORDER BY n.cle ASC")
        .unwrap();
    let ids = col0(&complet);

    for a in [0usize, 1, 5, 11, 23] {
        for b in [1usize, 3, 7] {
            let page = s
                .execute(&format!(
                    "MATCH (n:Eqs) RETURN n.id AS id ORDER BY n.cle ASC SKIP {a} LIMIT {b}"
                ))
                .unwrap();
            let fin = (a + b).min(ids.len());
            let attendu = if a >= ids.len() {
                Vec::new()
            } else {
                ids[a..fin].to_vec()
            };
            assert_eq!(
                col0(&page),
                attendu,
                "la fenêtre SKIP {a} LIMIT {b} doit être la tranche [{a}, {fin}) \
                 du tri complet"
            );
        }
    }
}

/// LA PROPRIÉTÉ DE PARTITION, sur un ordre total et sans NULL.
///
/// Parcourir toutes les pages doit rendre chaque ligne EXACTEMENT une fois :
/// ni doublon, ni omission, et dans l'ordre global. C'est la garantie que
/// `ORDER BY` + `SKIP`/`LIMIT` offre à une pagination applicative, et elle
/// doit tenir pour TOUTE taille de page, y compris celles qui ne divisent
/// pas le total.
#[test]
fn parcours_pagine_partitionne_exactement_le_resultat_trie() {
    let total = 17i64; // premier : aucune taille de page ne divise le total
    let db = semer_desordonne(total);
    let s = db.session();

    for taille in 1..=(total as usize + 2) {
        let mut vues: Vec<i64> = Vec::new();
        let mut decalage = 0usize;
        loop {
            let page = s
                .execute(&format!(
                    "MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC \
                     SKIP {decalage} LIMIT {taille}"
                ))
                .unwrap();
            if page.rows.is_empty() {
                break;
            }
            assert!(
                page.rows.len() <= taille,
                "taille={taille} décalage={decalage} : page de {} lignes au-delà \
                 de son LIMIT",
                page.rows.len()
            );
            vues.extend(col0(&page));
            decalage += taille;
            assert!(decalage <= total as usize * 2, "pagination sans fin");
        }

        assert_eq!(
            vues,
            (0..total).collect::<Vec<_>>(),
            "taille de page {taille} : le recollement des pages doit reproduire \
             le résultat trié entier, chaque ligne exactement une fois"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  G. La fenêtre et les AGRÉGATS
//
//  Point de vigilance : la fenêtre porte sur les LIGNES DU RÉSULTAT, donc sur
//  les GROUPES après agrégation — jamais sur les lignes d'entrée. Un `LIMIT`
//  descendu sous l'agrégat rendrait des comptes justes en apparence et faux
//  en valeur.
// ─────────────────────────────────────────────────────────────────────────

/// `count(*)` sans regroupement produit UNE ligne ; `LIMIT 1` la sert.
#[test]
fn limit_un_sur_agregat_scalaire_sert_la_ligne_d_agregat() {
    let db = semer_desordonne(30);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Page) RETURN count(n) AS c LIMIT 1")
        .unwrap();
    assert_eq!(r.rows.len(), 1);
    assert_eq!(
        as_i64(&r.rows[0][0]),
        30,
        "le LIMIT borne le nombre de LIGNES DU RÉSULTAT, il ne tronque pas \
         l'entrée de l'agrégat : le compte doit rester 30"
    );
}

/// `LIMIT 0` sur un agrégat scalaire : la ligne d'agrégat existe mais la
/// fenêtre ne la sert pas. Elle ne doit pas « échapper » à la fenêtre au
/// prétexte qu'elle est produite par un agrégat.
#[test]
fn limit_zero_sur_agregat_scalaire_ne_sert_rien() {
    let db = semer_desordonne(30);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Page) RETURN count(n) AS c LIMIT 0")
        .unwrap();
    assert_eq!(
        r.rows.len(),
        0,
        "LIMIT 0 vide la fenêtre, y compris sur la ligne d'un agrégat scalaire"
    );
}

/// `SKIP 1` sur un agrégat scalaire : il n'y a qu'une ligne, la sauter vide
/// le résultat.
#[test]
fn skip_un_sur_agregat_scalaire_ne_sert_rien() {
    let db = semer_desordonne(30);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Page) RETURN count(n) AS c SKIP 1")
        .unwrap();
    assert_eq!(
        r.rows.len(),
        0,
        "un agrégat scalaire n'a qu'une ligne à sauter"
    );
}

/// LE CAS QUI COMPTE : agrégat GROUPÉ sous fenêtre.
///
/// 40 nœuds répartis en 4 groupes de 10. `LIMIT 2` doit servir 2 GROUPES,
/// chacun portant son compte COMPLET de 10. Si la fenêtre était appliquée
/// avant l'agrégation, on obtiendrait 2 lignes d'entrée et des comptes de 1 :
/// la cardinalité serait juste et les valeurs fausses.
#[test]
fn limit_sur_agregat_groupe_borne_les_groupes_pas_les_lignes_d_entree() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for i in 0..40i64 {
        s.execute(&format!("INSERT (:Grp {{g: {}, v: {i}}})", i % 4))
            .unwrap();
    }

    let r = s
        .execute("MATCH (n:Grp) RETURN n.g AS g, count(n) AS c ORDER BY g ASC LIMIT 2")
        .unwrap();

    assert_eq!(r.rows.len(), 2, "LIMIT 2 doit servir 2 GROUPES");
    assert_eq!(as_i64(&r.rows[0][0]), 0, "premier groupe trié : g=0");
    assert_eq!(as_i64(&r.rows[1][0]), 1, "second groupe trié : g=1");
    for (i, row) in r.rows.iter().enumerate() {
        assert_eq!(
            as_i64(&row[1]),
            10,
            "groupe {i} : le compte doit porter sur les 10 lignes du groupe. \
             Un compte de 1 signifierait que la fenêtre a tronqué l'ENTRÉE de \
             l'agrégat au lieu de sa SORTIE"
        );
    }
}

/// La même exigence avec un décalage : `SKIP 1 LIMIT 2` sur des groupes.
#[test]
fn fenetre_sur_agregat_groupe_decoupe_les_groupes_avec_leurs_comptes_complets() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for i in 0..40i64 {
        s.execute(&format!("INSERT (:Grp2 {{g: {}, v: {i}}})", i % 4))
            .unwrap();
    }

    let r = s
        .execute("MATCH (n:Grp2) RETURN n.g AS g, count(n) AS c ORDER BY g ASC SKIP 1 LIMIT 2")
        .unwrap();

    assert_eq!(r.rows.len(), 2);
    assert_eq!(
        (as_i64(&r.rows[0][0]), as_i64(&r.rows[1][0])),
        (1, 2),
        "la fenêtre [1, 3) sur 4 groupes triés sert g=1 et g=2"
    );
    assert_eq!(as_i64(&r.rows[0][1]), 10);
    assert_eq!(as_i64(&r.rows[1][1]), 10);
}

/// `LIMIT` au-delà du nombre de GROUPES : tous les groupes, pas davantage.
/// Une fenêtre de 100 sur 4 groupes ne doit pas se rabattre sur les 40
/// lignes d'entrée.
#[test]
fn limit_au_dela_du_nombre_de_groupes_rend_tous_les_groupes() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for i in 0..40i64 {
        s.execute(&format!("INSERT (:Grp3 {{g: {}, v: {i}}})", i % 4))
            .unwrap();
    }

    let r = s
        .execute("MATCH (n:Grp3) RETURN n.g AS g, count(n) AS c ORDER BY g ASC LIMIT 100")
        .unwrap();
    assert_eq!(
        r.rows.len(),
        4,
        "le résultat compte 4 groupes ; la fenêtre ne peut en servir ni plus \
         ni moins, et surtout pas les 40 lignes d'entrée"
    );
}

// ─────────────────────────────────────────────────────────────────────────
//  H. La fenêtre et DISTINCT
//
//  Angle du lot 10 : la fenêtre compte les lignes DISTINCTES, c'est-à-dire la
//  sortie de la déduplication, jamais son entrée. Une fenêtre descendue sous
//  le DISTINCT rendrait un nombre de lignes plausible et un contenu faux.
//  (La sémantique de DISTINCT elle-même relève du lot 08.)
// ─────────────────────────────────────────────────────────────────────────

/// Sème 40 nœuds `:Ds` ne portant que 4 valeurs distinctes, GROUPÉES par
/// paquets de 10 : une fenêtre appliquée avant la déduplication tomberait
/// dans un seul paquet et se trahirait.
fn semer_peu_de_distincts() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for i in 0..40i64 {
        s.execute(&format!("INSERT (:Ds {{v: {}}})", i / 10))
            .unwrap();
    }
    db
}

/// `DISTINCT … SKIP 4` sur 4 valeurs distinctes : le décalage épuise le
/// résultat dédupliqué, donc zéro ligne.
///
/// Ce cas DISCRIMINE fortement. Si le `SKIP` était appliqué aux 40 lignes
/// d'entrée avant la déduplication, il en resterait 36 couvrant toujours les
/// 4 valeurs : on obtiendrait 4 lignes au lieu de 0.
#[test]
fn skip_sur_distinct_decale_les_valeurs_distinctes_pas_les_lignes_d_entree() {
    let db = semer_peu_de_distincts();
    let s = db.session();

    let r = s
        .execute("MATCH (n:Ds) RETURN DISTINCT n.v AS v SKIP 4")
        .unwrap();
    assert_eq!(
        r.rows.len(),
        0,
        "le résultat dédupliqué compte 4 lignes ; SKIP 4 les épuise. Obtenir \
         des lignes ici signifierait que le décalage a porté sur les 40 lignes \
         d'entrée au lieu des 4 lignes de sortie"
    );
}

/// `DISTINCT … SKIP 2` : il doit rester exactement 2 valeurs distinctes.
#[test]
fn skip_partiel_sur_distinct_laisse_le_reste_des_valeurs_distinctes() {
    let db = semer_peu_de_distincts();
    let s = db.session();

    let r = s
        .execute("MATCH (n:Ds) RETURN DISTINCT n.v AS v ORDER BY v ASC SKIP 2")
        .unwrap();
    assert_eq!(
        col0(&r),
        vec![2, 3],
        "sur les 4 valeurs distinctes triées [0,1,2,3], SKIP 2 laisse [2,3]"
    );
}

/// `DISTINCT … LIMIT 0` : la fenêtre vide prime sur la déduplication.
#[test]
fn limit_zero_sur_distinct_ne_sert_rien() {
    let db = semer_peu_de_distincts();
    let s = db.session();

    let r = s
        .execute("MATCH (n:Ds) RETURN DISTINCT n.v AS v LIMIT 0")
        .unwrap();
    assert_eq!(r.rows.len(), 0);
}

/// `DISTINCT … LIMIT` au-delà du nombre de valeurs distinctes : les 4
/// valeurs, pas les 40 lignes.
#[test]
fn limit_au_dela_du_nombre_de_distincts_rend_les_distincts() {
    let db = semer_peu_de_distincts();
    let s = db.session();

    let r = s
        .execute("MATCH (n:Ds) RETURN DISTINCT n.v AS v ORDER BY v ASC LIMIT 100")
        .unwrap();
    assert_eq!(
        col0(&r),
        vec![0, 1, 2, 3],
        "une fenêtre plus large que le résultat dédupliqué sert ce résultat, \
         jamais les lignes d'entrée"
    );
}

/// Partition d'un résultat DISTINCT trié : le parcours pagine les VALEURS
/// distinctes, chacune exactement une fois.
#[test]
fn parcours_pagine_sur_distinct_partitionne_les_valeurs_distinctes() {
    let db = semer_peu_de_distincts();
    let s = db.session();

    for taille in 1..=5usize {
        let mut vues: Vec<i64> = Vec::new();
        let mut decalage = 0usize;
        loop {
            let page = s
                .execute(&format!(
                    "MATCH (n:Ds) RETURN DISTINCT n.v AS v ORDER BY v ASC \
                     SKIP {decalage} LIMIT {taille}"
                ))
                .unwrap();
            if page.rows.is_empty() {
                break;
            }
            vues.extend(col0(&page));
            decalage += taille;
            assert!(decalage <= 40, "pagination sans fin sur DISTINCT");
        }
        assert_eq!(
            vues,
            vec![0, 1, 2, 3],
            "taille de page {taille} : le recollement doit rendre les 4 valeurs \
             distinctes, chacune une seule fois"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  I. CHEMIN RAPIDE contre CHEMIN NORMAL
//
//  Le planificateur court-circuite l'exécution pour un `LIMIT` posé sur un
//  `NodeScan` NU — sans `ORDER BY` ni filtre. Deux chemins qui répondent à la
//  même question doivent répondre la même chose : ces cas les confrontent.
//  Un raccourci qui rendrait des doublons, ou qui ignorerait le `SKIP`,
//  servirait une fenêtre fausse sans jamais échouer bruyamment.
// ─────────────────────────────────────────────────────────────────────────

/// Le raccourci doit servir exactement `k` lignes, toutes DIFFÉRENTES. Un
/// scan tronqué qui rejouerait un tampon rendrait `k` lignes dont certaines
/// en double — cardinalité juste, contenu faux.
#[test]
fn chemin_rapide_sert_k_lignes_deux_a_deux_distinctes() {
    let db = semer_desordonne(50);
    let s = db.session();

    let r = s.execute("MATCH (n:Page) RETURN n.v AS v LIMIT 5").unwrap();
    assert_eq!(r.rows.len(), 5, "LIMIT 5 sur un scan nu sert 5 lignes");

    let mut vals = col0(&r);
    let brut = vals.len();
    vals.sort_unstable();
    vals.dedup();
    assert_eq!(
        vals.len(),
        brut,
        "les 5 lignes doivent porter 5 valeurs distinctes : chaque nœud n'est \
         scanné qu'une fois"
    );
    for v in &vals {
        assert!((0..50).contains(v), "valeur {v} hors du semis");
    }
}

/// Le raccourci doit honorer le `SKIP`. Deux pages consécutives d'un scan nu
/// doivent être DISJOINTES : si le `SKIP` était perdu, la seconde page
/// répéterait la première.
#[test]
fn chemin_rapide_honore_le_skip_et_rend_des_pages_disjointes() {
    let db = semer_desordonne(50);
    let s = db.session();

    let p1 = s.execute("MATCH (n:Page) RETURN n.v AS v LIMIT 5").unwrap();
    let p2 = s
        .execute("MATCH (n:Page) RETURN n.v AS v SKIP 5 LIMIT 5")
        .unwrap();

    assert_eq!(p1.rows.len(), 5);
    assert_eq!(
        p2.rows.len(),
        5,
        "SKIP 5 LIMIT 5 sur 50 nœuds sert 5 lignes"
    );

    let a: std::collections::HashSet<i64> = col0(&p1).into_iter().collect();
    let b: std::collections::HashSet<i64> = col0(&p2).into_iter().collect();
    let commun: Vec<&i64> = a.intersection(&b).collect();
    assert!(
        commun.is_empty(),
        "les pages [0,5) et [5,10) d'un même scan doivent être disjointes ; \
         valeurs répétées : {commun:?} — le SKIP a été perdu sur le chemin rapide"
    );
}

/// Le raccourci et le chemin normal doivent s'accorder sur le CONTENU.
///
/// Le même `LIMIT` est posé une fois sur un scan nu (chemin rapide) et une
/// fois derrière un filtre que tout satisfait (chemin normal, le raccourci
/// est désarmé). Les deux fenêtres n'ont pas à être ordonnées de la même
/// façon — GQL ne fixe pas l'ordre sans `ORDER BY` — mais elles doivent
/// toutes deux servir `k` lignes distinctes prélevées dans le même semis.
#[test]
fn chemin_rapide_et_chemin_normal_servent_des_fenetres_de_meme_calibre() {
    let db = semer_desordonne(50);
    let s = db.session();

    let rapide = s.execute("MATCH (n:Page) RETURN n.v AS v LIMIT 7").unwrap();
    let normal = s
        .execute("MATCH (n:Page) WHERE n.v >= 0 RETURN n.v AS v LIMIT 7")
        .unwrap();

    assert_eq!(rapide.rows.len(), 7, "chemin rapide : 7 lignes");
    assert_eq!(
        normal.rows.len(),
        7,
        "chemin normal (filtre total) : 7 lignes — le raccourci et le chemin \
         générique doivent servir la même quantité"
    );

    for (nom, r) in [("rapide", &rapide), ("normal", &normal)] {
        let mut v = col0(r);
        let brut = v.len();
        v.sort_unstable();
        v.dedup();
        assert_eq!(
            v.len(),
            brut,
            "chemin {nom} : lignes en double dans la fenêtre"
        );
        for x in &v {
            assert!((0..50).contains(x), "chemin {nom} : valeur {x} hors semis");
        }
    }
}

/// Le raccourci ne doit ni perdre ni dupliquer de ligne quand la fenêtre est
/// parcourue en entier.
///
/// GQL laisse l'ordre indéterminé en l'absence d'`ORDER BY` ; ce cas ne
/// contraint donc PAS l'ordre. Il contraint ce qui reste exigible : sur un
/// instantané inchangé, le recollement de toutes les pages doit reproduire
/// le résultat complet, chaque ligne exactement une fois. Sans cela le
/// `SKIP`/`LIMIT` d'un scan nu serait inutilisable pour paginer.
#[test]
fn parcours_pagine_du_chemin_rapide_reconstitue_tout_le_semis() {
    let db = semer_desordonne(31);
    let s = db.session();

    for taille in [1usize, 2, 5, 7, 31, 40] {
        let mut vues: Vec<i64> = Vec::new();
        let mut decalage = 0usize;
        loop {
            let page = s
                .execute(&format!(
                    "MATCH (n:Page) RETURN n.v AS v SKIP {decalage} LIMIT {taille}"
                ))
                .unwrap();
            if page.rows.is_empty() {
                break;
            }
            assert!(page.rows.len() <= taille, "page au-delà de son LIMIT");
            vues.extend(col0(&page));
            decalage += taille;
            assert!(decalage <= 200, "pagination sans fin sur le chemin rapide");
        }

        vues.sort_unstable();
        assert_eq!(
            vues,
            (0..31).collect::<Vec<_>>(),
            "taille de page {taille} : le recollement doit rendre les 31 nœuds, \
             chacun exactement une fois (ni doublon, ni omission)"
        );
    }
}

/// Le raccourci ne doit pas court-circuiter le comptage : `LIMIT` sur un scan
/// nu ne change pas ce qu'un agrégat non fenêtré rapporte.
#[test]
fn le_raccourci_ne_contamine_pas_le_total() {
    let db = semer_desordonne(50);
    let s = db.session();

    s.execute("MATCH (n:Page) RETURN n.v AS v LIMIT 3").unwrap();
    let total = s.execute("MATCH (n:Page) RETURN count(n) AS c").unwrap();
    assert_eq!(
        as_i64(&total.rows[0][0]),
        50,
        "une requête fenêtrée ne doit laisser aucune trace sur la suivante"
    );
}

// ─────────────────────────────────────────────────────────────────────────
//  J. LES ORTHOGRAPHES NORMATIVES DE LA FENÊTRE
//
//  `SKIP` est l'orthographe héritée de Cypher. ISO/IEC 39075 écrit la clause
//  de décalage `OFFSET` (14.12) et admet `FETCH FIRST n ROWS ONLY` comme
//  forme longue de la clause de limite (14.13). Le parseur reconnaît les
//  trois (`parser.rs:2604` pour `Skip | Offset`, `:2613-2615` pour `Fetch`,
//  `parse_fetch_first` `:2721`).
//
//  Trois orthographes pour une seule fenêtre : elles doivent découper la même
//  chose. Une divergence ici serait invisible à l'usage — chaque forme prise
//  isolément a l'air de fonctionner.
// ─────────────────────────────────────────────────────────────────────────

/// `OFFSET` doit décaler la fenêtre exactement comme `SKIP`.
#[test]
fn offset_est_l_orthographe_normative_de_skip() {
    let db = semer_desordonne(20);
    let s = db.session();

    let par_skip = s
        .execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC SKIP 5 LIMIT 6")
        .unwrap();
    let par_offset = s
        .execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC OFFSET 5 LIMIT 6")
        .unwrap();

    assert_eq!(
        col0(&par_offset),
        (5..11).collect::<Vec<_>>(),
        "OFFSET 5 LIMIT 6 découpe la fenêtre [5, 11) — GQL 14.12"
    );
    assert_eq!(
        col0(&par_offset),
        col0(&par_skip),
        "OFFSET et SKIP nomment la même clause : elles doivent servir la même \
         fenêtre, à la ligne près"
    );
}

/// `OFFSET` seul, sans limite.
#[test]
fn offset_seul_rend_le_suffixe_entier() {
    let db = semer_desordonne(10);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC OFFSET 4")
        .unwrap();
    assert_eq!(col0(&r), (4..10).collect::<Vec<_>>());
}

/// `FETCH FIRST n ROWS ONLY` doit borner comme `LIMIT n`.
#[test]
fn fetch_first_rows_only_borne_comme_limit() {
    let db = semer_desordonne(20);
    let s = db.session();

    let par_limit = s
        .execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC LIMIT 4")
        .unwrap();
    let par_fetch = s
        .execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC FETCH FIRST 4 ROWS ONLY")
        .unwrap();

    assert_eq!(
        col0(&par_fetch),
        (0..4).collect::<Vec<_>>(),
        "FETCH FIRST 4 ROWS ONLY borne la fenêtre à 4 lignes — GQL 14.13"
    );
    assert_eq!(
        col0(&par_fetch),
        col0(&par_limit),
        "FETCH FIRST et LIMIT nomment la même clause"
    );
}

/// La composition normative complète : `ORDER BY` → `OFFSET` → `FETCH FIRST`.
#[test]
fn offset_puis_fetch_first_composent_la_fenetre_normative() {
    let db = semer_desordonne(20);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Page) RETURN n.v AS v ORDER BY n.v ASC OFFSET 7 FETCH FIRST 3 ROWS ONLY")
        .unwrap();
    assert_eq!(
        col0(&r),
        vec![7, 8, 9],
        "la forme entièrement normative doit découper la fenêtre [7, 10)"
    );
}

// ─────────────────────────────────────────────────────────────────────────
//  K. LES FORMES QUI ATTEIGNENT VRAIMENT LE GARDE
//
//  Bloc ajouté APRÈS la campagne de mutation : cinq mutations avaient
//  survécu, non parce que le moteur était juste, mais parce qu'aucun cas ne
//  passait par le code muté. Les cas ci-dessous visent ces branches-là.
// ─────────────────────────────────────────────────────────────────────────

/// `DISTINCT` + `LIMIT` SANS `ORDER BY` : la seule forme qui présente au
/// planificateur la silhouette `Limit → Return(distinct) → NodeScan`, celle
/// que garde `project.rs:497` (`&& !ret.distinct`).
///
/// Ce garde interdit de descendre la limite sous une projection
/// dédupliquante : plafonner le scan à N rendrait au plus N lignes BRUTES,
/// dont la déduplication tirerait MOINS de N valeurs. Avec 40 lignes
/// groupées par 10 et `LIMIT 4`, un scan plafonné à 4 ne verrait que la
/// première valeur et rendrait 1 ligne au lieu de 4.
///
/// (Le moindre `ORDER BY` change la silhouette et désarme le garde : c'est
/// pourquoi les autres cas DISTINCT de ce fichier ne l'atteignent pas.)
#[test]
fn distinct_avec_limit_sans_order_by_ne_descend_pas_la_limite_sous_la_deduplication() {
    let db = semer_peu_de_distincts();
    let s = db.session();

    let r = s
        .execute("MATCH (n:Ds) RETURN DISTINCT n.v AS v LIMIT 4")
        .unwrap();

    assert_eq!(
        r.rows.len(),
        4,
        "DISTINCT … LIMIT 4 doit trouver 4 VALEURS distinctes. En rendre moins \
         signifie que la limite a été descendue sous la déduplication et a \
         affamé la projection (garde project.rs:497)"
    );
    let mut v = col0(&r);
    v.sort_unstable();
    assert_eq!(v, vec![0, 1, 2, 3], "les 4 valeurs distinctes du semis");
}

/// Même exigence avec une fenêtre décalée et sans `ORDER BY`.
#[test]
fn distinct_avec_skip_et_limit_sans_order_by_reste_sur_les_valeurs_distinctes() {
    let db = semer_peu_de_distincts();
    let s = db.session();

    let r = s
        .execute("MATCH (n:Ds) RETURN DISTINCT n.v AS v SKIP 1 LIMIT 2")
        .unwrap();
    assert_eq!(
        r.rows.len(),
        2,
        "il reste 3 valeurs distinctes après SKIP 1 ; LIMIT 2 en sert 2"
    );
    let mut v = col0(&r);
    v.sort_unstable();
    v.dedup();
    assert_eq!(
        v.len(),
        2,
        "les deux lignes servies doivent être distinctes"
    );
}

/// Sème `n` nœuds `:Vrac` directement, sans passer par le planificateur.
fn semer_en_vrac(n: i64) -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for v in 0..n {
        s.create_node_with_props(&["Vrac"], [("v", Value::Int64(v))]);
    }
    db
}

/// LA BRANCHE DE DÉCOUPE DE LOT.
///
/// `LimitOperator` ne tranche un lot en deux que si le lot reçu excède la
/// fenêtre restante (`limit.rs`, branche « Return partial chunk »). Les
/// chemins rapides du planificateur plafonnent la source AVANT cet
/// opérateur — scan borné pour un `NodeScan` nu, tri borné sous un `Sort` —
/// si bien que le `LimitOperator` de tête reçoit d'ordinaire exactement `k`
/// lignes et n'a rien à trancher.
///
/// Un FILTRE désarme les deux descentes tout en laissant passer beaucoup de
/// lignes : le lot atteint alors la taille de lot du moteur
/// (`DEFAULT_CHUNK_SIZE = 2048`, `obrain-core/src/execution/chunk.rs:13`) et
/// la découpe a réellement lieu. C'est la seule forme de ce fichier qui
/// l'exerce.
#[test]
fn limit_tranche_un_lot_plus_grand_que_la_fenetre() {
    let db = semer_en_vrac(3000);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Vrac) WHERE n.v >= 0 RETURN n.v AS v LIMIT 5")
        .unwrap();
    assert_eq!(
        r.rows.len(),
        5,
        "un lot de 2048 lignes tranché à 5 doit rendre EXACTEMENT 5 lignes ; \
         en rendre davantage trahit une erreur d'un cran dans le calcul du \
         reste à servir"
    );

    let mut v = col0(&r);
    let brut = v.len();
    v.sort_unstable();
    v.dedup();
    assert_eq!(
        v.len(),
        brut,
        "aucune ligne servie deux fois dans le lot tranché"
    );
    for x in &v {
        assert!((0..3000).contains(x), "valeur {x} hors du semis");
    }
}

/// La même découpe, décalée : le `SKIP` doit tomber À L'INTÉRIEUR du premier
/// lot, puis le `LIMIT` trancher ce qui reste.
#[test]
fn fenetre_decalee_tranche_correctement_a_l_interieur_d_un_lot() {
    let db = semer_en_vrac(3000);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Vrac) WHERE n.v >= 0 RETURN n.v AS v SKIP 10 LIMIT 5")
        .unwrap();
    assert_eq!(
        r.rows.len(),
        5,
        "SKIP 10 LIMIT 5 à l'intérieur d'un lot de 2048 doit rendre 5 lignes"
    );

    let premiere = s
        .execute("MATCH (n:Vrac) WHERE n.v >= 0 RETURN n.v AS v LIMIT 10")
        .unwrap();
    let debut: std::collections::HashSet<i64> = col0(&premiere).into_iter().collect();
    let apres: std::collections::HashSet<i64> = col0(&r).into_iter().collect();
    let commun: Vec<&i64> = debut.intersection(&apres).collect();
    assert!(
        commun.is_empty(),
        "la fenêtre [10, 15) ne peut recouper la fenêtre [0, 10) ; \
         valeurs répétées : {commun:?} — le décalage a été perdu"
    );
}

/// Une fenêtre qui ENJAMBE la frontière de lot (2048). Le `SKIP` épuise le
/// premier lot entier, le `LIMIT` sert des lignes du second : le compte de
/// lignes sautées doit survivre au passage d'un lot à l'autre.
#[test]
fn fenetre_a_cheval_sur_deux_lots_sert_le_bon_nombre_de_lignes() {
    let db = semer_en_vrac(3000);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Vrac) WHERE n.v >= 0 RETURN n.v AS v SKIP 2040 LIMIT 20")
        .unwrap();
    assert_eq!(
        r.rows.len(),
        20,
        "la fenêtre [2040, 2060) enjambe la frontière de lot 2048 et doit \
         servir 20 lignes ; un compteur remis à zéro entre deux lots en \
         servirait un autre nombre"
    );
    let mut v = col0(&r);
    let brut = v.len();
    v.sort_unstable();
    v.dedup();
    assert_eq!(v.len(), brut, "aucun doublon à la jointure de deux lots");
}

/// Le total reste servi en entier quand aucune fenêtre ne borne : témoin de
/// cardinalité pour les cas ci-dessus.
#[test]
fn sans_fenetre_le_filtre_rend_tout_le_semis() {
    let db = semer_en_vrac(3000);
    let s = db.session();

    let r = s
        .execute("MATCH (n:Vrac) WHERE n.v >= 0 RETURN count(n) AS c")
        .unwrap();
    assert_eq!(as_i64(&r.rows[0][0]), 3000);
}

// ═════════════════════════════════════════════════════════════════════════
//  PORTÉE RÉELLE DE CE FICHIER — ce que la campagne de mutation a établi
// ═════════════════════════════════════════════════════════════════════════
//
//  Neuf mutations ont été posées dans le code que ce fichier prétend
//  couvrir, chacune confrontée à la SUITE ENTIÈRE. Cinq ont été tuées,
//  quatre ont survécu. Les survivantes ne disent pas « le moteur est juste » :
//  elles disent « aucun cas ne passe par là ». Elles sont consignées ici
//  plutôt que masquées, parce qu'elles délimitent ce que ce fichier NE
//  mesure pas.
//
//  TUÉES — la couverture est réelle :
//   • M1  `LimitOperator`, reste à servir décalé d'un cran
//         (`limit.rs`, branche de découpe de lot) → 15 cas rougissent.
//   • M2  `SkipOperator`, décalage décalé d'un cran → 6 cas.
//   • M6  `bounded_top_k` privé du départage par ordinal d'arrivée
//         (`sort.rs:288`) → les deux cas d'ex æquo.
//   • M7  frontière `k >= n` changée en `k > n` (`sort.rs:213`) →
//         `limit_egal_au_total_rend_tout`.
//   • M8  garde `!ret.distinct` neutralisé (`project.rs:497`) →
//         `distinct_avec_limit_sans_order_by_…`.
//
//  SURVIVANTES — et pourquoi, ce qui est en soi une observation sur le
//  moteur :
//
//   • M3 et M4 — `LimitSkipOperator` (`limit.rs`) : ni sa limite ni son
//     décalage ne peuvent être cassés de manière observable, parce que
//     l'opérateur n'est JAMAIS CONSTRUIT. `build_limit` et `build_skip`
//     (`planner/common.rs:24` et `:35`) montent deux opérateurs séparés ;
//     `LimitSkipOperator` n'apparaît nulle part ailleurs que dans son
//     propre `pub use` (`operators/mod.rs:83`) et ses tests unitaires. Un
//     `SKIP … LIMIT` GQL passe donc par `Skip` PUIS `Limit`, jamais par
//     l'opérateur fusionné. C'est du code mort qui se teste lui-même.
//
//   • M5 — `Some(0) => rows.clear()` (`sort.rs:207`) : inatteignable depuis
//     le chemin GQL, car les descentes de `plan_limit` sont gardées par
//     `limit.count.value() > 0` (`project.rs:495`, `:549`). Un `LIMIT 0`
//     n'arme jamais le tri borné ; c'est le `LimitOperator` de tête qui
//     rend zéro ligne. Voir la note de portée sur
//     `limit_zero_sous_order_by_rend_zero_ligne`.
//
//   • M9 — plafond du scan poussé à `k + 1` : invisible dans les résultats,
//     parce que `build_limit` réenveloppe le plan dans un `LimitOperator`
//     qui retronque à `k` (`project.rs:517-521`). Le plafond du chemin
//     rapide est donc une OPTIMISATION, pas le contrat de sortie : une
//     erreur sur ce plafond ne se verrait qu'au chronomètre, jamais dans
//     les lignes servies. Aucun cas de ce fichier — et aucun test
//     fonctionnel en général — ne peut la détecter.
