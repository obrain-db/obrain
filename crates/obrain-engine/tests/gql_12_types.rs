//! Conformité GQL — lot 12 : LE SYSTÈME DE TYPES LUI-MÊME.
//!
//! obrain-db est SANS SCHÉMA : rien n'empêche la clé `v` de porter un `Int64`
//! sur un nœud et une `String` sur son voisin. Ce fichier ne teste donc pas les
//! opérateurs (lot 02), l'arithmétique (06), le tri (09), les chaînes (05) ni
//! les agrégats (07) : il teste ce qui arrive quand les TYPES SE MÉLANGENT, et
//! la cohérence interne du système de types — l'ordre entre types différents,
//! où s'applique la coercition implicite, et si le type DÉCLARÉ d'une colonne
//! dit la vérité sur les valeurs qu'elle contient.
//!
//! # RÈGLE D'ÉCRITURE
//!
//! Chaque assertion encode l'INTENTION sémantique, jamais l'observation. Là où
//! le moteur diverge, l'assertion reste conforme à la spécification et le test
//! est placé derrière `#[cfg(feature = "defauts-connus")]`, précédé d'un
//! commentaire `// DÉFAUT:` nommant le code fautif en `fichier:ligne`.
//!
//! * `cargo test` (défaut) — les 22 tests conformes tournent, et ils passent ;
//!   les tests de défauts ne sont pas même compilés.
//! * `cargo test --features defauts-connus` — les 12 tests de défauts tournent
//!   en plus, et ils ÉCHOUENT. C'est leur rôle : ils deviendront verts le jour
//!   de la correction, sans qu'une ligne d'assertion ne bouge.
//!
//! Plier une assertion vers l'observation graverait le défaut en
//! spécification. Ce dépôt en a déjà payé le prix :
//! `crates/obrain-substrate/tests/vector_top_k_exact.rs` s'est déclaré
//! « PARITÉ LEGACY » et a figé un défaut en écartant explicitement la lecture
//! correcte du code.
//!
//! # LÀ OÙ LA SPEC EST MUETTE, ON EXIGE UNE PROPRIÉTÉ, PAS UNE VALEUR
//!
//! ISO GQL ne FIXE PAS l'ordre entre deux types différents : openCypher en
//! impose un, SQL rejette la comparaison. Ce fichier ne tranche donc pas —
//! il exige les propriétés qu'un ordre, QUEL QU'IL SOIT, doit tenir :
//!
//! * **le résultat appartient à l'entrée** — trier ne fabrique ni ne perd rien ;
//! * **antisymétrie** — `ASC` et `DESC` sont exactement inverses l'un de l'autre
//!   quand toutes les valeurs sont distinctes ;
//! * **indépendance de l'ordre d'insertion** — un ordre est une propriété des
//!   valeurs, pas de la date à laquelle on les a écrites ;
//! * **transitivité par restriction** — un ordre total restreint à un
//!   sous-ensemble homogène reste l'ordre de ce sous-ensemble ;
//! * **déterminisme de chemin** — la même paire, sous deux plans d'exécution,
//!   rend la même réponse.
//!
//! Ces propriétés sont plus solides qu'un ordre canonique arbitraire, et elles
//! ne figent aucun choix.
//!
//! # ANTI-VACUITÉ — REGISTRE DES MUTATIONS
//!
//! Chaque test vert de ce fichier a été éprouvé en CASSANT le code de
//! production qu'il prétend couvrir, et en vérifiant qu'il rougit AVEC LE BON
//! MESSAGE. Vingt et une mutations, chacune appliquée SEULE sur un arbre
//! propre, puis révertée avec vérification que l'arbre est bit à bit celui
//! d'avant. Les douze tests `defauts-connus` n'ont pas besoin de mutation :
//! ils sont rouges contre le code réel, c'est leur raison d'être.
//!
//! | # | mutation | tests qui rougissent |
//! |---|----------|----------------------|
//! | M1  | `value_utils.rs:60` — ordre des entiers inversé | `entiers_et_flottants_*`, `une_colonne_heterogene_*` |
//! | M2  | `value_utils.rs:63` — ordre des chaînes inversé | `des_chaines_s_ordonnent_*` |
//! | M4  | `planner/lpg/filter.rs` — chemin intervalle désactivé | `le_type_booleen_est_ordonne_*` |
//! | M5  | `filter.rs:3413` — `values_equal` toujours vrai | les 2 tests de listes |
//! | M6  | `filter.rs` `value_matches_type` — INTEGER accepte les chaînes | les 2 tests `IS TYPED` |
//! | M7a | `executor/mod.rs:158` — types de colonnes tous `Any` | `count_declare_*` |
//! | M7b | idem — types de colonnes tous `Int64` | `le_type_declare_couvre_*`, `une_colonne_heterogene_*` |
//! | M7c | idem — aucun type déclaré | 4 tests, dont `tout_resultat_declare_*` |
//! | M8  | `filter.rs` `evaluate` — une valeur non booléenne est un prédicat vrai | `une_valeur_non_booleenne_*`, `une_propriete_absente_*` |
//! | M9  | `filter.rs:3431` — l'égalité de listes ignore la longueur | `deux_listes_de_longueurs_*` |
//! | M10b| `distinct.rs:40` — la clé de déduplication perd le type | `distinct_ne_confond_pas_*` |
//! | M11 | LA FIXTURE — semis déjà groupé/trié et portant un doublon | `le_semis_heterogene_est_un_semis_valide`, et lui SEUL |
//! | M12b| `sort.rs:396` — le tri perd une ligne | 6 tests, dont les 2 de permutation |
//! | M13b| `planner/lpg/filter.rs:15` — l'égalité indexée confond bool et entier | `un_booleen_n_est_pas_un_entier` |
//! | M16 | `planner/lpg/filter.rs` — la borne stricte devient inclusive | `sur_colonne_homogene_*`, `le_type_booleen_*` |
//! | M17 | `filter.rs` `evaluate` — le prédicat ignore la valeur du booléen | `un_booleen_est_un_predicat` + 3 |
//! | M18 | `filter.rs:488` — la liste littérale uniformise ses éléments en chaîne | `une_liste_heterogene_conserve_*` |
//!
//! ## LES TROIS SURVIVANTES — ce sont elles qui ont appris le plus
//!
//! * **M10** — casser `GroupKey`
//!   (`obrain-core/src/execution/operators/aggregate.rs`), qui encode les
//!   flottants par `f64::to_bits`, ne fait rougir AUCUN test. GQL ne passe pas
//!   par là pour `DISTINCT` : il passe par `RowKey` (`distinct.rs:40`), qui
//!   porte le MÊME défaut. Sans cette mutation, le commentaire `// DÉFAUT:` de
//!   `distinct_est_coherent_avec_l_egalite_*` aurait envoyé son lecteur
//!   corriger du code que la requête n'exécute pas.
//! * **M13** — casser `values_equal`
//!   (`obrain-core/src/execution/operators/filter.rs:3416`) ne fait pas rougir
//!   `un_booleen_n_est_pas_un_entier`. Une égalité sur PROPRIÉTÉ ne passe pas
//!   par l'évaluateur d'expression : elle passe par `values_equal_coerced`
//!   (`obrain-engine/src/query/planner/lpg/filter.rs:15`). C'est la même
//!   bifurcation que celle mesurée par la section B.
//! * **M12** — décaler la garde de sortie de `SortOperator` (`sort.rs:390`) ne
//!   fait rien rougir, non parce que le tri ne serait pas emprunté, mais parce
//!   que cette garde n'est atteinte qu'au SECOND appel, quand tout a déjà été
//!   émis : la mutation était inopérante par construction. M12b, qui écourte la
//!   boucle d'émission (`sort.rs:396`), en tue six. Une mutation qui ne fait
//!   rien rougir doit d'abord être soupçonnée elle-même.
//!
//! ## LA MUTATION QUI PROUVE QUE LA SECTION B MESURE BIEN DEUX CHEMINS
//!
//! Un test de parité qui comparerait deux fois LE MÊME chemin serait vide, et
//! passerait pour vert le jour d'une correction. M4 le réfute dans les deux
//! sens à la fois. Avec `--features defauts-connus` et le chemin de balayage
//! par intervalle désactivé :
//!
//! * les TROIS tests de parité (`une_borne_stricte_*`,
//!   `une_chaine_non_numerique_*`, `l_ordre_des_booleens_*`) VIRENT AU VERT —
//!   les deux formes du prédicat empruntent alors le même chemin et s'accordent ;
//! * et le témoin `le_type_booleen_est_ordonne_false_avant_true` VIRE AU ROUGE
//!   — il était donc bien servi par le chemin qu'on vient de retirer.
//!
//! Douze échecs deviennent dix : trois disparaissent, un apparaît. C'est la
//! preuve que les deux bras de chaque test de parité sont bien deux plans
//! distincts, et que la divergence vient du balayage par intervalle.
//!
//! ## NON COUVERT PAR UNE MUTATION
//!
//! `litteraux_gql_produisent_les_types_annonces` : l'atteindre demanderait de
//! muter le parseur de littéraux dans `obrain-adapters`, hors du périmètre de
//! ce lot. Le test reste utile — il verrouille la prémisse de typage de tous
//! les autres — mais sa falsifiabilité n'est pas démontrée ici.
//!
//! ```bash
//! cargo test -p obrain-engine --test gql_12_types
//! cargo test -p obrain-engine --features defauts-connus --test gql_12_types
//! ```
#![cfg(feature = "gql")]

use obrain_common::types::{LogicalType, Value};
use obrain_engine::ObrainDB;
use obrain_engine::database::QueryResult;

// ===========================================================================
// Outillage
// ===========================================================================

/// Sème des nœuds `:T` portant `v` (la valeur à l'étude) et `id` (l'identité
/// de ligne, TOUJOURS un entier, pour pouvoir relire l'ordre d'arrivée).
///
/// Le semis est DIRECT — `create_node_with_props` — et non par littéral GQL :
/// sur un lot consacré aux types, il faut contrôler la variante exacte de
/// `Value`, pas la laisser au parseur. Le typage des littéraux est vérifié
/// séparément par `litteraux_gql_produisent_les_types_annonces`.
fn semer(valeurs: &[Value]) -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    {
        let s = db.session();
        for (i, v) in valeurs.iter().enumerate() {
            s.create_node_with_props(&["T"], [("id", Value::Int64(i as i64)), ("v", v.clone())]);
        }
        // Ancre : une ligne unique pour évaluer les expressions scalaires.
        s.create_node_with_props(&["Ancre"], [("k", Value::Int64(0))]);
    }
    db
}

fn lire(db: &ObrainDB, requete: &str) -> QueryResult {
    db.session()
        .execute(requete)
        .unwrap_or_else(|e| panic!("requête refusée par le moteur :\n  {requete}\n  → {e:?}"))
}

/// Première colonne d'un résultat, valeurs brutes.
fn colonne0(db: &ObrainDB, requete: &str) -> Vec<Value> {
    lire(db, requete)
        .rows
        .into_iter()
        .map(|l| l[0].clone())
        .collect()
}

/// Évalue une expression scalaire GQL sur la ligne d'ancrage.
fn scalaire(db: &ObrainDB, expr: &str) -> Value {
    let requete = format!("MATCH (a:Ancre) RETURN {expr}");
    let r = lire(db, &requete);
    assert_eq!(
        r.rows.len(),
        1,
        "l'ancre doit produire exactement 1 ligne, sinon l'expression n'est pas évaluée : {requete}"
    );
    r.rows[0][0].clone()
}

/// Ensemble des `id` rendus par une requête projetant `n.id`.
fn ids(db: &ObrainDB, requete: &str) -> Vec<i64> {
    let mut v: Vec<i64> = lire(db, requete)
        .rows
        .iter()
        .map(|l| match &l[0] {
            Value::Int64(i) => *i,
            autre => panic!("colonne `id` : Int64 attendu, obtenu {autre:?} ({requete})"),
        })
        .collect();
    v.sort_unstable();
    v
}

/// Rend une étiquette lisible et STABLE pour une valeur, type compris.
///
/// `Int64(1)` et `String("1")` doivent se distinguer dans les messages
/// d'erreur : sans le type, un désordre entre types serait invisible.
fn etiquette(v: &Value) -> String {
    match v {
        Value::Int64(i) => format!("i{i}"),
        Value::Float64(f) => format!("f{f}"),
        Value::String(s) => format!("s'{s}'"),
        Value::Bool(b) => format!("b{b}"),
        Value::Null => "NULL".to_string(),
        autre => format!("{autre:?}"),
    }
}

fn etiquettes(vs: &[Value]) -> Vec<String> {
    vs.iter().map(etiquette).collect()
}

/// Une valeur non nulle est-elle CONFORME au type déclaré de sa colonne ?
///
/// `Any` accepte tout — c'est l'aveu honnête « je ne sais pas », et il est
/// permis. `NULL` habite tous les types. Tout le reste doit correspondre :
/// une colonne déclarée `Int64` qui livre une chaîne ment sur son contenu.
fn valeur_conforme(t: &LogicalType, v: &Value) -> bool {
    if matches!(t, LogicalType::Any) || matches!(v, Value::Null) {
        return true;
    }
    match (t, v) {
        (LogicalType::Bool, Value::Bool(_))
        | (LogicalType::String, Value::String(_))
        | (LogicalType::Bytes, Value::Bytes(_))
        | (LogicalType::Date, Value::Date(_))
        | (LogicalType::Time, Value::Time(_))
        | (LogicalType::Timestamp, Value::Timestamp(_))
        | (LogicalType::Duration, Value::Duration(_))
        | (LogicalType::List(_), Value::List(_))
        | (LogicalType::Map { .. }, Value::Map(_))
        | (LogicalType::Path, Value::Path { .. }) => true,
        (t, Value::Int64(_)) if t.is_integer() => true,
        (LogicalType::Float32 | LogicalType::Float64, Value::Float64(_)) => true,
        // Un nœud, une arête ou un chemin transitent sous des formes internes
        // variées ; ce fichier ne prétend pas les arbitrer.
        (LogicalType::Node | LogicalType::Edge | LogicalType::Path, _) => true,
        (LogicalType::Null, _) => false,
        _ => false,
    }
}

/// Vérifie le CONTRAT d'un résultat : autant de types déclarés que de
/// colonnes, et aucune valeur en contradiction avec le type de sa colonne.
fn verifier_contrat_de_types(r: &QueryResult, requete: &str) {
    assert_eq!(
        r.column_types.len(),
        r.columns.len(),
        "contrat rompu : {} colonne(s) mais {} type(s) déclaré(s)\n  requête : {requete}\n  \
         colonnes : {:?}\n  types : {:?}\n  un consommateur qui dimensionne ses tampons sur \
         `column_types` lirait hors du vecteur",
        r.columns.len(),
        r.column_types.len(),
        r.columns,
        r.column_types
    );
    for (col, t) in r.column_types.iter().enumerate() {
        for (ligne, valeurs) in r.rows.iter().enumerate() {
            let v = &valeurs[col];
            assert!(
                valeur_conforme(t, v),
                "le type DÉCLARÉ de la colonne {col} (`{}`) ment sur son contenu : \
                 déclaré {t:?}, ligne {ligne} porte {v:?}\n  requête : {requete}\n  \
                 déclarer `Any` serait honnête ; déclarer un type scalaire faux ne l'est pas",
                r.columns[col]
            );
        }
    }
}

// ===========================================================================
// A. L'ORDRE ENTRE TYPES DIFFÉRENTS
//
// `compare_values_total` (obrain-core/src/execution/operators/value_utils.rs:57)
// se termine par `_ => Ordering::Equal` sous une docstring qui annonce
// « total ordering ». Ce n'est pas un ordre total : il déclare ÉGALES toutes
// les paires de types différents, ce qui détruit la transitivité dès qu'une
// colonne est hétérogène — le cas normal d'une base sans schéma.
//
// Le codebase possède DÉJÀ un ordre total inter-types correct :
// `OrderableValue` (obrain-common/src/types/value.rs:815) départage les types
// différents par ordinal (Bool < Int64 < Float64 < String < …). Le tri ne s'en
// sert pas ; le balayage par intervalle, si — d'où la section B.
// ===========================================================================

/// Semis hétérogène de référence. Volontairement PAS dans l'ordre de sortie
/// attendu, quel que soit l'ordre canonique retenu : un tri inopérant rendrait
/// l'ordre d'insertion et serait attrapé.
fn semis_heterogene() -> Vec<Value> {
    vec![
        Value::Int64(3),
        Value::String("bravo".into()),
        Value::Float64(1.5),
        Value::Bool(true),
        Value::Int64(-2),
        Value::String("alpha".into()),
        Value::Bool(false),
        Value::Float64(9.25),
    ]
}

/// Garde-fou du fichier — le semis doit pouvoir faire échouer les tests.
///
/// Vérification STRUCTURELLE, sans le moteur : elle reste valide même si le
/// tri ne fait rien, ce qui est précisément le cas mesuré ici. Un garde-fou
/// qui interrogerait le moteur pour savoir si la fixture est bien mêlée
/// deviendrait vert dès que le tri cesse d'agir — l'inverse de son rôle.
#[test]
fn le_semis_heterogene_est_un_semis_valide() {
    let semis = semis_heterogene();

    let mut vues = etiquettes(&semis);
    let avant = vues.len();
    vues.sort();
    vues.dedup();
    assert_eq!(
        vues.len(),
        avant,
        "SEMIS INVALIDE : les valeurs doivent être deux à deux distinctes, sinon \
         le test d'antisymétrie ASC/DESC ne prouve rien (des ex æquo peuvent \
         légitimement rester dans le même ordre)"
    );

    let entiers: Vec<i64> = semis
        .iter()
        .filter_map(|v| match v {
            Value::Int64(i) => Some(*i),
            _ => None,
        })
        .collect();
    assert!(
        entiers.len() >= 2,
        "SEMIS INVALIDE : il faut au moins deux entiers pour éprouver la \
         transitivité par restriction"
    );
    assert!(
        entiers.windows(2).any(|p| p[0] > p[1]),
        "SEMIS INVALIDE : les entiers sont déjà croissants dans l'ordre \
         d'insertion ; un tri inopérant passerait le test de restriction"
    );

    let types: Vec<&str> = semis
        .iter()
        .map(|v| match v {
            Value::Int64(_) => "int",
            Value::Float64(_) => "flo",
            Value::String(_) => "cha",
            Value::Bool(_) => "boo",
            _ => "autre",
        })
        .collect();
    assert!(
        types.windows(2).any(|p| p[0] != p[1]),
        "SEMIS INVALIDE : les types doivent être ENTRELACÉS, pas groupés — s'ils \
         étaient déjà groupés par type, un ordre par ordinal de type serait \
         indiscernable de l'ordre d'insertion"
    );
}

/// Le résultat appartient à l'entrée : trier est une PERMUTATION.
///
/// C'est la propriété la plus faible qu'un tri doive tenir, et elle vaut quel
/// que soit l'ordre canonique choisi entre types.
#[test]
fn trier_une_colonne_heterogene_est_une_permutation_de_l_entree() {
    let semis = semis_heterogene();
    let db = semer(&semis);
    let mut attendu = etiquettes(&semis);
    let mut obtenu = etiquettes(&colonne0(&db, "MATCH (n:T) RETURN n.v ORDER BY n.v"));
    attendu.sort();
    obtenu.sort();
    assert_eq!(
        attendu, obtenu,
        "ORDER BY sur une colonne hétérogène doit rendre EXACTEMENT les valeurs \
         semées — ni perdue, ni dupliquée, ni transformée"
    );
}

/// Un tri qui n'ordonne rien reste un tri : il ne doit ni paniquer ni perdre
/// de lignes, fût-ce sur quarante valeurs de quatre types mêlés.
///
/// Rust spécifie que `sort_by` avec un comparateur non transitif rend un ordre
/// NON SPÉCIFIÉ et *peut paniquer* (« user-provided comparison function does
/// not correctly implement a total order »). Une base sans schéma atteint ce
/// cas par une requête ordinaire ; le moteur ne doit pas s'y interrompre.
#[test]
fn trier_quarante_valeurs_melangees_ne_panique_pas_et_conserve_le_cardinal() {
    let mut valeurs = Vec::new();
    for i in 0..40i64 {
        valeurs.push(match i % 4 {
            0 => Value::Int64(40 - i),
            1 => Value::String(format!("s{:02}", 40 - i).into()),
            2 => Value::Float64((40 - i) as f64 + 0.5),
            _ => Value::Bool(i % 8 == 3),
        });
    }
    let db = semer(&valeurs);
    let r = lire(&db, "MATCH (n:T) RETURN n.v ORDER BY n.v");
    assert_eq!(
        r.rows.len(),
        40,
        "le tri d'une colonne hétérogène doit rendre les 40 lignes semées"
    );
    let mut attendu = etiquettes(&valeurs);
    let mut obtenu = etiquettes(&r.rows.iter().map(|l| l[0].clone()).collect::<Vec<_>>());
    attendu.sort();
    obtenu.sort();
    assert_eq!(
        attendu, obtenu,
        "aucune valeur ne doit être perdue ni inventée"
    );
}

/// Témoin. Entiers et flottants mêlés s'ordonnent NUMÉRIQUEMENT — le seul
/// couple de types que `compare_values_total` sait vraiment départager.
///
/// Sans ce témoin, les tests rouges de cette section pourraient l'être parce
/// que le tri ne fonctionne pas du tout ; il prouve qu'il fonctionne.
#[test]
fn entiers_et_flottants_melanges_s_ordonnent_numeriquement() {
    let db = semer(&[
        Value::Int64(3),
        Value::Float64(1.5),
        Value::Int64(-2),
        Value::Float64(2.5),
    ]);
    assert_eq!(
        etiquettes(&colonne0(&db, "MATCH (n:T) RETURN n.v ORDER BY n.v")),
        vec!["i-2", "f1.5", "f2.5", "i3"],
        "INTEGER et FLOAT sont un même domaine numérique : leur ordre est celui \
         des nombres, pas celui des types"
    );
}

/// Témoin. Des chaînes seules s'ordonnent LEXICOGRAPHIQUEMENT — `'10'` avant
/// `'9'`, parce que le type est STRING et non NUMBER.
#[test]
fn des_chaines_s_ordonnent_lexicographiquement_et_non_numeriquement() {
    let db = semer(&[
        Value::String("9".into()),
        Value::String("10".into()),
        Value::String("100".into()),
    ]);
    assert_eq!(
        etiquettes(&colonne0(&db, "MATCH (n:T) RETURN n.v ORDER BY n.v")),
        vec!["s'10'", "s'100'", "s'9'"],
        "l'ordre du type STRING est celui des points de code ; une chaîne qui \
         ressemble à un nombre reste une chaîne"
    );
}

/// ANTISYMÉTRIE. Sur des valeurs toutes distinctes, `DESC` est exactement
/// l'inverse d'`ASC`. Aucun ordre — canonique ou non — n'y échappe.
///
/// Le tri inverse le résultat du comparateur pour `DESC` ; si le comparateur
/// déclare `Equal` des valeurs distinctes, l'inversion ne les départage pas et
/// le tri stable les laisse dans l'ordre d'arrivée, IDENTIQUE dans les deux
/// sens. La symétrie est alors rompue au milieu du résultat.
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : compare_values_total rend Equal pour toute paire de types
//   differents (obrain-core/src/execution/operators/value_utils.rs:57) ; le
//   `.reverse()` du DESC
//   (obrain-core/src/execution/operators/sort.rs:249-252) n'a alors rien a
//   inverser.
#[test]
fn desc_est_exactement_l_inverse_d_asc_sur_valeurs_distinctes() {
    // DÉFAUT: obrain-core/src/execution/operators/value_utils.rs:57 —
    // `compare_values_total` se termine par `_ => Ordering::Equal`. Les paires
    // de types différents étant déclarées égales, `.reverse()`
    // (obrain-core/src/execution/operators/sort.rs:249-252) n'a rien à
    // inverser et le tri stable reconduit l'ordre d'arrivée à l'identique.
    // La correction existe déjà dans le dépôt : `OrderableValue`
    // (obrain-common/src/types/value.rs:799-826) départage les types
    // différents par ordinal.
    let db = semer(&semis_heterogene());
    let asc = etiquettes(&colonne0(&db, "MATCH (n:T) RETURN n.v ORDER BY n.v"));
    let mut desc = etiquettes(&colonne0(&db, "MATCH (n:T) RETURN n.v ORDER BY n.v DESC"));
    desc.reverse();
    assert_eq!(
        asc, desc,
        "toutes les valeurs semées sont distinctes : ORDER BY DESC renversé doit \
         redonner ORDER BY ASC, sinon l'ordre n'est pas antisymétrique"
    );
}

/// INDÉPENDANCE DE L'ORDRE D'INSERTION. Un ordre est une propriété des
/// valeurs. Deux bases portant les mêmes valeurs, semées dans deux ordres
/// différents, doivent rendre la même séquence sous `ORDER BY`.
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : compare_values_total rend Equal pour toute paire de types
//   differents (obrain-core/src/execution/operators/value_utils.rs:57) ; le
//   tri stable reconduit l'ordre d'ecriture.
#[test]
fn le_tri_ne_depend_pas_de_l_ordre_d_insertion() {
    // DÉFAUT: obrain-core/src/execution/operators/value_utils.rs:57 — toute
    // paire de types différents étant `Equal`, le tri stable ne réordonne que
    // les paires de même type et conserve l'ordre d'arrivée pour le reste.
    let semis = semis_heterogene();
    let mut inverse = semis.clone();
    inverse.reverse();

    let a = etiquettes(&colonne0(
        &semer(&semis),
        "MATCH (n:T) RETURN n.v ORDER BY n.v",
    ));
    let b = etiquettes(&colonne0(
        &semer(&inverse),
        "MATCH (n:T) RETURN n.v ORDER BY n.v",
    ));
    assert_eq!(
        a, b,
        "les deux bases portent exactement les mêmes valeurs ; seul l'ordre \
         d'écriture diffère. Un ORDER BY qui en dépend n'ordonne pas les \
         valeurs, il restitue l'historique des écritures"
    );
}

/// TRANSITIVITÉ PAR RESTRICTION. Un ordre total restreint à un sous-ensemble
/// homogène reste l'ordre de ce sous-ensemble : parmi les lignes triées, les
/// entiers doivent apparaître dans l'ordre croissant, même si des valeurs
/// d'autres types sont intercalées.
///
/// C'est la conséquence la plus directe de la transitivité, et elle est
/// indépendante de tout ordre canonique : où que le moteur place les chaînes,
/// il ne doit pas mettre `3` avant `-2`.
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : compare_values_total n'est pas transitif
//   (obrain-core/src/execution/operators/value_utils.rs:57).
#[test]
fn les_entiers_restent_ordonnes_entre_eux_malgre_une_chaine_intercalee() {
    // DÉFAUT: obrain-core/src/execution/operators/value_utils.rs:57 —
    // `compare_values_total(Int64(3), String("m")) == Equal` et
    // `compare_values_total(String("m"), Int64(-2)) == Equal`, mais
    // `compare_values_total(Int64(3), Int64(-2)) == Greater`. Un comparateur
    // non transitif rend, par contrat Rust, un ordre NON SPÉCIFIÉ.
    let db = semer(&[Value::Int64(3), Value::String("m".into()), Value::Int64(-2)]);
    let trie = colonne0(&db, "MATCH (n:T) RETURN n.v ORDER BY n.v");
    let entiers: Vec<i64> = trie
        .iter()
        .filter_map(|v| match v {
            Value::Int64(i) => Some(*i),
            _ => None,
        })
        .collect();
    assert_eq!(
        entiers,
        vec![-2, 3],
        "sous-suite des entiers dans le résultat trié : elle doit être croissante \
         quel que soit l'endroit où le moteur place la chaîne. Résultat complet : {:?}",
        etiquettes(&trie)
    );
}

// ===========================================================================
// B. OÙ S'APPLIQUE LA COERCITION ? — LA RÉPONSE DÉPEND DU PLAN
//
// Deux sémantiques inconciliables cohabitent pour la même comparaison :
//
// * l'ÉVALUATEUR (obrain-core/src/execution/operators/filter.rs:3527)
//   coerce STRING vers NUMBER en parsant la chaîne (hack RDF) : `'7' > 5` est
//   VRAI, `'abc' > 5` est UNKNOWN ;
// * le BALAYAGE PAR INTERVALLE (obrain-substrate/src/store.rs:9067
//   `valeur_dans_intervalle`, via `OrderableValue`) ne coerce RIEN et range
//   les types par ordinal : toute STRING est supérieure à tout INTEGER, donc
//   `'abc' > 5` est VRAI.
//
// Le planificateur choisit l'un ou l'autre selon la FORME du prédicat
// (obrain-engine/src/query/planner/lpg/filter.rs, `try_plan_filter_with_range_index`).
// Une même question reçoit donc deux réponses selon le plan retenu.
//
// Les tests de cette section n'exigent AUCUNE des deux sémantiques : ils
// exigent seulement qu'elles coïncident. Les deux formes comparées sont `P` et
// `P AND (n.id >= 0)` — la seconde conjonction est VRAIE pour toutes les lignes
// semées (les `id` sont 0, 1, 2, …), donc les deux formes posent la même
// question. Son seul rôle est de rendre le prédicat inextractible par
// `try_plan_filter_with_range_index`, qui n'accepte qu'un prédicat de borne
// SIMPLE, et donc de forcer l'évaluateur.
// ===========================================================================

/// Colonne hétérogène de référence : entiers, chaînes numériques, chaîne non
/// numérique, flottant. Chaque valeur est à portée de la borne `5`.
fn semis_borne() -> Vec<Value> {
    vec![
        Value::Int64(1),             // id 0 — sous la borne
        Value::Int64(7),             // id 1 — au-dessus
        Value::String("7".into()),   // id 2 — chaîne qui « ressemble » à 7
        Value::String("abc".into()), // id 3 — chaîne qui ne ressemble à rien
        Value::Float64(6.5),         // id 4 — au-dessus
    ]
}

/// Témoin de la section : sur une colonne HOMOGÈNE, les deux formes du même
/// prédicat s'accordent. Il prouve que `P OR P` n'altère pas par lui-même le
/// sens d'un prédicat — sans quoi les tests de parité ne mesureraient rien.
#[test]
fn sur_colonne_homogene_les_deux_formes_du_meme_predicat_s_accordent() {
    let db = semer(&[
        Value::Int64(1),
        Value::Int64(5),
        Value::Int64(7),
        Value::Int64(9),
    ]);
    let direct = ids(&db, "MATCH (n:T) WHERE n.v > 5 RETURN n.id");
    let conjoint = ids(&db, "MATCH (n:T) WHERE n.v > 5 AND n.id >= 0 RETURN n.id");
    assert_eq!(
        direct,
        vec![2, 3],
        "borne stricte : 5 exclu, 7 et 9 retenus"
    );
    assert_eq!(
        direct, conjoint,
        "`n.id >= 0` est vraie pour toutes les lignes : les deux formes posent \
         la même question, et sur une colonne homogène le moteur y répond pareil"
    );
}

/// IDEMPOTENCE DE `OR`. `P` et `P OR P` sont la même question : ils doivent
/// rendre le même ensemble, colonne hétérogène ou non.
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : deux semantiques pour une meme borne — OrderableValue par ordinal
//   de type (obrain-common/src/types/value.rs:815) contre la coercition RDF
//   de l'evaluateur (obrain-core/src/execution/operators/filter.rs:3527).
#[test]
fn une_borne_stricte_rend_le_meme_ensemble_sous_ses_deux_formes() {
    // DÉFAUT: deux sémantiques pour une même comparaison.
    //   chemin A — obrain-engine/src/query/planner/lpg/filter.rs
    //     `try_plan_filter_with_range_index` reconnaît `n.v > 5` seul et
    //     délègue à `find_nodes_in_range`
    //     (obrain-substrate/src/store.rs:10765), qui applique
    //     `valeur_dans_intervalle` (store.rs:9067) sur `OrderableValue`.
    //     `OrderableValue::cmp` (obrain-common/src/types/value.rs:815) range
    //     les types différents par ordinal : String(3) > Int64(1), donc
    //     `'abc' > 5` est VRAI.
    //   chemin B — la disjonction n'est pas extractible, l'évaluateur
    //     `filter.rs:3527` s'applique : il parse la chaîne en f64, donc
    //     `'7' > 5` est VRAI et `'abc' > 5` est UNKNOWN.
    let db = semer(&semis_borne());
    let direct = ids(&db, "MATCH (n:T) WHERE n.v > 5 RETURN n.id");
    let conjoint = ids(&db, "MATCH (n:T) WHERE n.v > 5 AND n.id >= 0 RETURN n.id");
    assert_eq!(
        direct, conjoint,
        "la même question posée sous deux formes équivalentes doit recevoir la \
         même réponse. Ce fichier ne tranche pas laquelle des deux sémantiques \
         est la bonne — il exige qu'il n'y en ait qu'une.\n  \
         `n.v > 5` seul → {direct:?}\n  `n.v > 5 AND n.id >= 0` → {conjoint:?}"
    );
}

/// L'ÉGALITÉ A UNE SEULE DÉFINITION. Le moteur affirme `'5' = 5` sur des
/// littéraux ; il doit alors retenir le nœud dont `v` porte la chaîne `'5'`
/// quand on demande `n.v = 5`.
///
/// Ce test ne réclame pas la coercition — il réclame la COHÉRENCE : soit
/// `'5' = 5` est vrai partout, soit il est faux partout. Deux définitions de
/// l'égalité dans un même moteur rendent tout raisonnement impossible.
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : deux notions d'egalite — values_equal coerce les chaines
//   (obrain-core/src/execution/operators/filter.rs:3424) contre
//   values_equal_coerced qui ne coerce que Int64/Float64
//   (obrain-engine/src/query/planner/lpg/filter.rs:15).
#[test]
fn l_egalite_a_la_meme_definition_sur_un_litteral_et_sur_une_propriete() {
    // DÉFAUT: deux notions d'égalité.
    //   littéraux — obrain-core/src/execution/operators/filter.rs:3424,
    //     `values_equal` traite `(String, Int64)` en parsant la chaîne en i64.
    //   propriété — obrain-engine/src/query/planner/lpg/filter.rs:15
    //     `values_equal_coerced` ne coerce QUE Int64↔Float64 ; le chemin
    //     substrat obrain-substrate/src/store.rs:10559
    //     (`find_nodes_by_property`) compare par `v == value`, strictement.
    // ── ASSERTIONS RÉÉCRITES PAR LE CORRECTIF E, VIA LA CLAUSE DE
    // RÉVERSIBILITÉ DU TEST ── Le texte d'origine gravait la branche
    // coercitive (« '5' = 5 est VRAI, donc WHERE n.v = 5 retient la chaîne »)
    // en prémisse, tout en déclarant lui-même l'autre branche acceptable :
    // « Si le moteur préfère refuser la coercition, c'est le littéral qui
    // doit rendre FAUX — mais pas l'un et pas l'autre ». La spec retenue par
    // la campagne est celle du lot 02 (gql_02, chaine_et_nombre_ne_sont_pas_
    // comparables) : classes disjointes → UNKNOWN aux SIX opérateurs, sans
    // exception — exigée par l'identité (a >= b AND a <= b) ≡ (a = b). La
    // prémisse et la conclusion sont donc réécrites dans la branche
    // non-coercitive ; l'INTENTION du test — une seule définition de
    // l'égalité, que le motif porte un label ou non — est inchangée et
    // vérifiée par la parité littéral/propriété ci-dessous.
    let db = semer(&[
        Value::Int64(5),             // id 0
        Value::String("5".into()),   // id 1
        Value::Float64(5.0),         // id 2
        Value::String("abc".into()), // id 3
    ]);
    assert_eq!(
        scalaire(&db, "'5' = 5"),
        Value::Null,
        "prémisse (branche non-coercitive de la clause de réversibilité) : \
         sur des littéraux, la chaîne '5' et l'entier 5 sont de classes \
         disjointes — l'égalité vaut UNKNOWN, jamais VRAI"
    );
    assert_eq!(
        ids(&db, "MATCH (n:T) WHERE n.v = 5 RETURN n.id"),
        vec![0, 2],
        "conclusion : la même égalité, appliquée à une propriété, retient \
         l'entier 5 et le flottant 5.0 (même nombre) et JAMAIS la chaîne '5' \
         ni 'abc' — le même verdict que sur littéraux, label ou pas"
    );
}

/// Une chaîne non numérique n'est PAS un nombre : elle ne satisfait AUCUNE
/// borne numérique.
///
/// Trois prédicats, trois réponses qui doivent s'accorder : `'abc'` n'est ni
/// au-dessus de 5, ni au-dessous, ni égal. Un moteur qui la place au-dessus de
/// tout entier ne compare pas des valeurs, il compare des ordinaux de type —
/// et la borne `> 5` cesse de vouloir dire « plus grand que cinq ».
///
/// Le lot 02 a mesuré la même racine sur des LITTÉRAUX, côté évaluateur
/// (`'inf' > 999999999` rend VRAI par parsage IEEE). Ce test-ci porte sur le
/// chemin de BALAYAGE PAR INTERVALLE, qui ne parse rien et se trompe pour une
/// raison différente : l'ordinal de type. Deux causes distinctes, deux
/// corrections distinctes.
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : OrderableValue::cmp compare par ordinal de type
//   (obrain-common/src/types/value.rs:815), servi par
//   valeur_dans_intervalle (obrain-substrate/src/store.rs:9067).
#[test]
fn une_chaine_non_numerique_ne_satisfait_aucune_borne_numerique() {
    // DÉFAUT: obrain-common/src/types/value.rs:815 — `OrderableValue::cmp`
    // termine par `self.type_ordinal().cmp(&other.type_ordinal())`. String a
    // l'ordinal 3, Int64 l'ordinal 1 : `'abc' > 5` est donc VRAI, et le serait
    // pour n'importe quel entier. Ce comparateur est celui qu'emprunte
    // `valeur_dans_intervalle` (obrain-substrate/src/store.rs:9067), servi par
    // `find_nodes_in_range` (store.rs:10765).
    //
    // Noter l'ASYMÉTRIE mesurée : `< 5` rend bien zéro ligne. Le défaut n'est
    // donc pas visible en regardant une seule borne — il faut les deux, et
    // c'est pourquoi ce test les interroge toutes les trois.
    let db = semer(&[Value::String("abc".into())]);
    let au_dessus = ids(&db, "MATCH (n:T) WHERE n.v > 5 RETURN n.id");
    let au_dessous = ids(&db, "MATCH (n:T) WHERE n.v < 5 RETURN n.id");
    let egal = ids(&db, "MATCH (n:T) WHERE n.v = 5 RETURN n.id");
    assert!(
        egal.is_empty(),
        "une chaîne non numérique ne peut pas être ÉGALE à un entier — obtenu {egal:?}"
    );
    assert!(
        au_dessous.is_empty(),
        "`'abc' < 5` doit être UNKNOWN, donc aucune ligne — obtenu {au_dessous:?}"
    );
    assert!(
        au_dessus.is_empty(),
        "`'abc' > 5` doit être UNKNOWN, donc aucune ligne — obtenu {au_dessus:?}. \
         Le moteur répond VRAI parce qu'il range STRING après INTEGER dans un \
         ordinal de types, ce qui n'est pas une comparaison de valeurs"
    );
}

// ===========================================================================
// C. LE TYPE DÉCLARÉ D'UNE COLONNE
//
// `QueryResult::column_types` est le contrat que le moteur passe à ses
// clients : « les valeurs de cette colonne sont de ce type ». Un pilote, un
// binding, un curseur typé s'y fient. Le contenu de ce vecteur n'était, avant
// ce fichier, asserté nulle part sur le résultat d'une requête réelle.
//
// Deux façons de mentir :
//   * déclarer un type scalaire pour une colonne qui n'en porte pas
//     (MIN/MAX câblent Int64 — obrain-engine/src/query/planner/lpg/aggregate.rs:227,235) ;
//   * ne déclarer aucun type du tout alors qu'il y a des colonnes.
// `Any` n'est pas un mensonge : c'est l'aveu honnête.
// ===========================================================================

/// Témoin. Sur des littéraux, le type déclaré doit couvrir les valeurs.
#[test]
fn le_type_declare_couvre_les_valeurs_sur_les_formes_simples() {
    let db = semer(&[Value::Int64(1), Value::Int64(2)]);
    for requete in [
        "MATCH (a:Ancre) RETURN 1 AS x",
        "MATCH (a:Ancre) RETURN 'texte' AS x",
        "MATCH (a:Ancre) RETURN true AS x",
        "MATCH (n:T) RETURN n.v AS x ORDER BY n.id",
        "MATCH (n:T) RETURN count(*) AS x",
        "MATCH (n:T) RETURN n.id AS a, n.v AS b ORDER BY n.id",
    ] {
        verifier_contrat_de_types(&lire(&db, requete), requete);
    }
}

/// `count` est un cardinal : son type déclaré doit être un entier, et la
/// valeur rendue doit y être conforme.
#[test]
fn count_declare_un_entier_et_rend_un_entier() {
    let db = semer(&[Value::Int64(1), Value::String("x".into())]);
    let r = lire(&db, "MATCH (n:T) RETURN count(*) AS c");
    assert_eq!(
        r.column_types,
        vec![LogicalType::Int64],
        "un cardinal est un entier ; le type déclaré doit le dire"
    );
    assert_eq!(r.rows[0][0], Value::Int64(2));
    verifier_contrat_de_types(&r, "count");
}

/// Une colonne HÉTÉROGÈNE ne peut pas être déclarée d'un type scalaire. Le
/// seul type honnête est `Any` — ou tout type qui accepte chacune de ses
/// valeurs.
#[test]
fn une_colonne_heterogene_ne_declare_pas_un_type_scalaire_faux() {
    let db = semer(&[
        Value::Int64(1),
        Value::String("x".into()),
        Value::Float64(2.5),
        Value::Bool(true),
    ]);
    let requete = "MATCH (n:T) RETURN n.v AS x ORDER BY n.id";
    let r = lire(&db, requete);
    verifier_contrat_de_types(&r, requete);
    assert_eq!(
        r.rows.len(),
        4,
        "les quatre valeurs doivent ressortir, quel que soit le type déclaré"
    );
    assert_eq!(
        etiquettes(&r.rows.iter().map(|l| l[0].clone()).collect::<Vec<_>>()),
        vec!["i1", "s'x'", "f2.5", "btrue"],
        "projeter une colonne hétérogène ne doit convertir aucune valeur : le \
         type déclaré décrit le contenu, il ne le remplace pas"
    );
}

/// MIN et MAX rendent une valeur PRÉSENTE EN ENTRÉE : leur type déclaré doit
/// donc être celui de leur entrée, jamais un type câblé.
///
/// Le lot 07 couvre la VALEUR rendue par MIN/MAX ; ce test-ci porte sur le
/// TYPE DÉCLARÉ de la colonne, qui est la cause : la valeur est écrasée parce
/// que le vecteur de sortie a été créé `Int64`.
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : MIN/MAX cablent LogicalType::Int64
//   (obrain-engine/src/query/planner/lpg/aggregate.rs:227 et :235) ; le
//   push d'une valeur d'un autre type ecrit 0 en silence
//   (obrain-core/src/execution/vector.rs:222).
#[test]
fn min_declare_le_type_de_son_entree_et_non_un_type_cable() {
    // DÉFAUT: obrain-engine/src/query/planner/lpg/aggregate.rs:227 et :235 —
    // `Min | Max => LogicalType::Int64`, sous un commentaire qui reconnaît que
    // « the aggregate can return any Value type ». Le vecteur de sortie est
    // alors un `VectorData::Int64` ; obrain-core/src/execution/vector.rs:222
    // pousse `0` en silence pour toute valeur d'un autre type (« Type mismatch
    // - push a default value to maintain vector alignment »).
    let db = semer(&[
        Value::String("banane".into()),
        Value::String("abricot".into()),
    ]);
    let r = lire(&db, "MATCH (n:T) RETURN min(n.v) AS m");
    assert_eq!(
        r.column_types,
        vec![LogicalType::String],
        "l'entrée de MIN est une colonne de chaînes : la sortie est une chaîne"
    );
    assert_eq!(
        r.rows[0][0],
        Value::String("abricot".into()),
        "MIN rend une valeur présente en entrée"
    );
}

/// Le contrat structurel : autant de types déclarés que de colonnes, sur
/// toutes les formes de requête qui produisent des colonnes.
#[test]
fn tout_resultat_declare_autant_de_types_que_de_colonnes() {
    let db = semer(&[Value::Int64(1), Value::String("x".into())]);
    for requete in [
        "MATCH (n:T) RETURN n.v AS x ORDER BY n.id",
        "MATCH (n:T) RETURN n.id AS a, n.v AS b ORDER BY n.id",
        "MATCH (n:T) RETURN count(*) AS c",
        "MATCH (n:T) RETURN DISTINCT n.v AS x",
        "MATCH (a:Ancre) RETURN 1 AS a, 'b' AS b, true AS c",
    ] {
        let r = lire(&db, requete);
        assert_eq!(
            r.column_types.len(),
            r.columns.len(),
            "contrat rompu sur `{requete}` : colonnes {:?}, types {:?}",
            r.columns,
            r.column_types
        );
    }
}

/// Les littéraux GQL produisent les types qu'ils annoncent : `1` est un
/// entier, `1.5` un flottant, `'1'` une chaîne, `true` un booléen.
///
/// C'est la prémisse de tout le reste : si le parseur typait `1.5` en entier,
/// aucune des lois de ce fichier ne mesurerait ce qu'elle croit mesurer.
#[test]
fn litteraux_gql_produisent_les_types_annonces() {
    let db = ObrainDB::new_in_memory();
    {
        let s = db.session();
        s.execute("INSERT (:Ancre {k: 0})").unwrap();
        s.execute("INSERT (:L {k: 'ent', v: 1})").unwrap();
        s.execute("INSERT (:L {k: 'flo', v: 1.5})").unwrap();
        s.execute("INSERT (:L {k: 'cha', v: '1'})").unwrap();
        s.execute("INSERT (:L {k: 'boo', v: true})").unwrap();
    }
    let lu = |k: &str| -> Value {
        let r = lire(&db, &format!("MATCH (n:L) WHERE n.k = '{k}' RETURN n.v"));
        assert_eq!(r.rows.len(), 1, "le nœud `{k}` doit être unique");
        r.rows[0][0].clone()
    };
    assert!(
        matches!(lu("ent"), Value::Int64(1)),
        "`1` est un INTEGER, obtenu {:?}",
        lu("ent")
    );
    assert!(
        matches!(lu("flo"), Value::Float64(_)),
        "`1.5` est un FLOAT, obtenu {:?}",
        lu("flo")
    );
    assert!(
        matches!(lu("cha"), Value::String(_)),
        "`'1'` est une STRING, obtenu {:?}",
        lu("cha")
    );
    assert!(
        matches!(lu("boo"), Value::Bool(true)),
        "`true` est un BOOLEAN, obtenu {:?}",
        lu("boo")
    );
}

// ===========================================================================
// D. LE TYPE EXPOSÉ — `IS TYPED`
//
// `IS TYPED` est la seule fenêtre que GQL ouvre sur le type RÉEL d'une valeur.
// Sur une base sans schéma c'est l'outil qui permet à l'utilisateur de se
// défendre contre l'hétérogénéité — encore faut-il qu'il dise vrai, et qu'il
// soit cohérent avec `=`.
// ===========================================================================

/// Semis à quatre types, un par ligne, `id` = rang.
fn semis_quatre_types() -> ObrainDB {
    semer(&[
        Value::Int64(1),           // id 0
        Value::String("1".into()), // id 1
        Value::Float64(1.0),       // id 2
        Value::Bool(true),         // id 3
    ])
}

/// `IS TYPED` PARTITIONNE une colonne hétérogène : les prédicats de types
/// distincts sont disjoints, et leur réunion couvre toutes les lignes.
#[test]
fn is_typed_partitionne_une_colonne_heterogene() {
    let db = semis_quatre_types();
    let ent = ids(&db, "MATCH (n:T) WHERE n.v IS TYPED INTEGER RETURN n.id");
    let cha = ids(&db, "MATCH (n:T) WHERE n.v IS TYPED STRING RETURN n.id");
    let flo = ids(&db, "MATCH (n:T) WHERE n.v IS TYPED FLOAT RETURN n.id");
    let boo = ids(&db, "MATCH (n:T) WHERE n.v IS TYPED BOOLEAN RETURN n.id");

    assert_eq!(ent, vec![0], "seul le nœud 0 porte un INTEGER");
    assert_eq!(
        cha,
        vec![1],
        "seul le nœud 1 porte une STRING — `'1'` n'est pas 1"
    );
    assert_eq!(flo, vec![2], "seul le nœud 2 porte un FLOAT");
    assert_eq!(boo, vec![3], "seul le nœud 3 porte un BOOLEAN");

    let mut reunion: Vec<i64> = [ent, cha, flo, boo].concat();
    reunion.sort_unstable();
    reunion.dedup();
    assert_eq!(
        reunion,
        vec![0, 1, 2, 3],
        "les quatre prédicats doivent couvrir la colonne : toute valeur a un type"
    );
}

/// Une chaîne qui ressemble à un entier n'est pas un entier. C'est l'énoncé
/// minimal d'un système de types qui existe.
#[test]
fn une_chaine_numerique_n_est_pas_un_entier() {
    let db = semis_quatre_types();
    assert_eq!(
        scalaire(&db, "'1' IS TYPED INTEGER"),
        Value::Bool(false),
        "`'1'` est une STRING : sa ressemblance avec un nombre n'est pas un type"
    );
    assert_eq!(
        scalaire(&db, "'1' IS TYPED STRING"),
        Value::Bool(true),
        "`'1'` est une STRING"
    );
}

/// COHÉRENCE ENTRE L'INTROSPECTION ET L'ÉGALITÉ. Deux valeurs dont `IS TYPED`
/// affirme qu'elles sont de types disjoints ne peuvent pas être ÉGALES.
///
/// Si `=` répond « oui » là où `IS TYPED` répond « types différents », alors
/// l'une des deux constructions ment, et l'utilisateur n'a plus aucun moyen de
/// se défendre contre l'hétérogénéité.
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : values_equal parse la chaine en i64
//   (obrain-core/src/execution/operators/filter.rs:3424) la ou
//   value_matches_type exige Value::Int64.
#[test]
fn deux_valeurs_de_types_disjoints_ne_sont_pas_egales() {
    // DÉFAUT: obrain-core/src/execution/operators/filter.rs:3424 —
    // `values_equal` traite `(String, Int64)` en parsant la chaîne en i64,
    // MÊME RACINE que la divergence D5 du lot 02 (la coercition RDF), mais loi
    // différente : le lot 02 oppose `=` à `<=`, celui-ci oppose `=` à
    // l'INTROSPECTION DE TYPE. Une seule correction les ferait passer tous deux.
    // alors que `value_matches_type` (filter.rs, branche "INTEGER") exige
    // `matches!(val, Value::Int64(_))`. Les deux constructions décrivent le
    // même couple de valeurs et se contredisent.
    let db = semis_quatre_types();
    assert_eq!(
        scalaire(&db, "'5' IS TYPED INTEGER"),
        Value::Bool(false),
        "prémisse : l'introspection dit que `'5'` n'est pas un entier"
    );
    // ── ASSERTION RÉÉCRITE PAR LE CORRECTIF E (Bool(false) → Null) ──
    // L'intention du test — l'introspection et l'égalité ne se contredisent
    // pas — est préservée : `'5' = 5` n'AFFIRME plus l'égalité. La valeur
    // exacte est UNKNOWN et non FALSE, par la spec du lot 02 (gql_02,
    // chaine_et_nombre_ne_sont_pas_comparables : classes disjointes → NULL
    // aux six opérateurs), exigée par (a >= b AND a <= b) ≡ (a = b) — un
    // FALSE ici, face à des `>=`/`<=` UNKNOWN, recréerait l'incohérence
    // inter-opérateurs que le lot 02 interdit.
    assert_eq!(
        scalaire(&db, "'5' = 5"),
        Value::Null,
        "conclusion : ce qui n'est pas un entier ne peut pas être DÉCLARÉ égal \
         à l'entier 5 — l'égalité entre classes disjointes vaut UNKNOWN"
    );
}

// ===========================================================================
// E. LE BOOLÉEN COMME TYPE
//
// BOOLEAN est le type le plus exposé aux confusions : c'est le seul dont GQL
// se sert pour décider, et c'est celui qu'un moteur laxiste confond le plus
// volontiers avec 0/1.
// ===========================================================================

/// Un booléen n'est pas un entier. `true` n'est pas `1`.
///
/// Ce test est servi par le chemin INDEXÉ, pas par l'évaluateur : la mutation
/// M13 du registre, qui fait confondre BOOLEAN et INTEGER à `values_equal`
/// (obrain-core/src/execution/operators/filter.rs:3416), ne l'a PAS fait
/// rougir ; il a fallu muter `values_equal_coerced`
/// (obrain-engine/src/query/planner/lpg/filter.rs:15) pour l'atteindre. Une
/// égalité sur PROPRIÉTÉ ne passe donc pas par l'évaluateur d'expression —
/// c'est la bifurcation de la section B, vue depuis l'autre bord.
#[test]
fn un_booleen_n_est_pas_un_entier() {
    let db = semer(&[Value::Bool(true), Value::Bool(false), Value::Int64(1)]);
    assert_eq!(
        ids(&db, "MATCH (n:T) WHERE n.v = 1 RETURN n.id"),
        vec![2],
        "`= 1` ne doit retenir que le nœud dont `v` est réellement l'entier 1 ; \
         `true` n'est pas 1, `false` n'est pas 0"
    );
    assert_eq!(
        ids(&db, "MATCH (n:T) WHERE n.v = 0 RETURN n.id"),
        Vec::<i64>::new(),
        "`false` n'est pas l'entier 0"
    );
}

/// Un booléen est un prédicat ; il décide.
#[test]
fn un_booleen_est_un_predicat() {
    let db = semer(&[Value::Bool(true), Value::Bool(false)]);
    assert_eq!(
        ids(&db, "MATCH (n:T) WHERE n.v RETURN n.id"),
        vec![0],
        "seule la ligne dont `v` vaut VRAI doit passer"
    );
}

/// Une valeur non booléenne n'est PAS un prédicat. GQL n'a pas de « vérité
/// approchée » : `1` ne vaut pas VRAI, `'texte'` non plus.
#[test]
fn une_valeur_non_booleenne_n_est_pas_un_predicat() {
    let db = semer(&[
        Value::Int64(1),
        Value::String("texte".into()),
        Value::Float64(1.0),
        Value::Bool(true),
    ]);
    assert_eq!(
        ids(&db, "MATCH (n:T) WHERE n.v RETURN n.id"),
        vec![3],
        "seule la valeur BOOLEAN VRAI est un prédicat vrai ; aucune coercition \
         de véracité n'existe en GQL"
    );
}

/// Témoin. Le type BOOLEAN est ORDONNÉ : `false` précède `true`, au tri comme
/// au filtre.
///
/// Ce test est servi par le balayage par intervalle
/// (`OrderableValue`, obrain-common/src/types/value.rs:805, qui a bien une
/// branche `(Bool, Bool)`). Il établit la prémisse des deux suivants : le
/// moteur SAIT ordonner deux booléens.
#[test]
fn le_type_booleen_est_ordonne_false_avant_true() {
    let db = semer(&[Value::Bool(true), Value::Bool(false)]);
    assert_eq!(
        etiquettes(&colonne0(&db, "MATCH (n:T) RETURN n.v ORDER BY n.v")),
        vec!["bfalse", "btrue"],
        "le tri place false avant true"
    );
    assert_eq!(
        ids(&db, "MATCH (n:T) WHERE n.v > false RETURN n.id"),
        vec![0],
        "le filtre aussi : true est strictement supérieur à false"
    );
    assert_eq!(
        ids(&db, "MATCH (n:T) WHERE n.v >= false RETURN n.id"),
        vec![0, 1],
        "et `>=` inclut la borne : les deux booléens satisfont `>= false`"
    );
}

/// RÉFLEXIVITÉ. Pour toute valeur NON NULLE `x`, `x <= x` est VRAI.
///
/// C'est la loi la plus élémentaire d'un ordre. Elle ne dépend d'aucun choix
/// canonique, d'aucune coercition, d'aucune convention : elle compare une
/// valeur à elle-même. Un type dont le moteur ne sait pas dire qu'il est égal
/// à lui-même n'est pas un type ordonné du système.
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : compare_values n'a aucune branche (Bool, Bool) et rend None
//   (obrain-core/src/execution/operators/filter.rs:3527).
#[test]
fn toute_valeur_non_nulle_est_inferieure_ou_egale_a_elle_meme() {
    // DÉFAUT: obrain-core/src/execution/operators/filter.rs:3527 —
    // `compare_values` n'a AUCUNE branche `(Value::Bool, Value::Bool)` et
    // tombe dans `_ => None` : `<`, `<=`, `>`, `>=` rendent UNKNOWN sur deux
    // booléens. Trois autres implémentations de la même comparaison en ont
    // une (`value_utils.rs:60`, `zone_map.rs:579`, `property.rs:1733`) ; celle
    // que l'évaluateur emprunte ne l'a pas.
    let db = semer(&[
        Value::Int64(1),             // id 0
        Value::Float64(2.5),         // id 1
        Value::String("abc".into()), // id 2
        Value::Bool(true),           // id 3
        Value::Bool(false),          // id 4
    ]);
    assert_eq!(
        ids(&db, "MATCH (n:T) WHERE n.v <= n.v RETURN n.id"),
        vec![0, 1, 2, 3, 4],
        "toute valeur non nulle est inférieure ou égale à elle-même, quel que \
         soit son type"
    );
    assert_eq!(
        ids(&db, "MATCH (n:T) WHERE n.v >= n.v RETURN n.id"),
        vec![0, 1, 2, 3, 4],
        "et supérieure ou égale à elle-même"
    );
}

/// L'ORDRE DES BOOLÉENS NE DÉPEND PAS DU PLAN. `n.v > false` et
/// `n.v > false AND n.id >= 0` posent la même question.
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : OrderableValue ordonne les booleens
//   (obrain-common/src/types/value.rs:805), l'evaluateur non
//   (obrain-core/src/execution/operators/filter.rs:3527).
#[test]
fn l_ordre_des_booleens_ne_depend_pas_du_plan_choisi() {
    // DÉFAUT: même couple de chemins que la section B, sur le type BOOLEAN.
    //   borne simple → obrain-substrate/src/store.rs:9067
    //     `valeur_dans_intervalle` sur `OrderableValue`, qui compare les
    //     booléens (obrain-common/src/types/value.rs:805).
    //   borne conjointe → obrain-core/src/execution/operators/filter.rs:3527
    //     `compare_values`, sans branche (Bool, Bool), rend UNKNOWN.
    let db = semer(&[Value::Bool(true), Value::Bool(false)]);
    let direct = ids(&db, "MATCH (n:T) WHERE n.v > false RETURN n.id");
    let conjoint = ids(
        &db,
        "MATCH (n:T) WHERE n.v > false AND n.id >= 0 RETURN n.id",
    );
    assert_eq!(
        direct, conjoint,
        "`n.id >= 0` est vraie pour les deux lignes : ajouter cette conjonction \
         ne peut pas changer la réponse.\n  `n.v > false` → {direct:?}\n  \
         `n.v > false AND n.id >= 0` → {conjoint:?}"
    );
}

// ===========================================================================
// F. LA LISTE COMME TYPE
// ===========================================================================

/// Une liste n'est pas son unique élément : `[1]` n'est pas `1`.
#[test]
fn une_liste_n_est_pas_son_unique_element() {
    let db = semer(&[Value::Int64(1)]);
    assert_eq!(
        scalaire(&db, "[1] = 1"),
        Value::Bool(false),
        "LIST et INTEGER sont deux types ; la longueur 1 ne les identifie pas"
    );
}

/// Deux listes de longueurs différentes ne sont pas égales.
#[test]
fn deux_listes_de_longueurs_differentes_ne_sont_pas_egales() {
    let db = semer(&[Value::Int64(1)]);
    assert_eq!(scalaire(&db, "[1, 2] = [1, 2]"), Value::Bool(true));
    assert_eq!(
        scalaire(&db, "[1, 2] = [1, 2, 3]"),
        Value::Bool(false),
        "l'égalité de listes commence par la longueur"
    );
}

/// Une liste hétérogène conserve le type de chacun de ses éléments.
#[test]
fn une_liste_heterogene_conserve_le_type_de_chaque_element() {
    let db = semer(&[Value::Int64(1)]);
    match scalaire(&db, "[1, 'a', true]") {
        Value::List(elems) => {
            assert_eq!(elems.len(), 3, "trois éléments attendus");
            assert!(
                matches!(elems[0], Value::Int64(1)),
                "élément 0 : INTEGER, obtenu {:?}",
                elems[0]
            );
            assert!(
                matches!(elems[1], Value::String(_)),
                "élément 1 : STRING, obtenu {:?}",
                elems[1]
            );
            assert!(
                matches!(elems[2], Value::Bool(true)),
                "élément 2 : BOOLEAN, obtenu {:?}",
                elems[2]
            );
        }
        autre => panic!("une liste littérale doit rendre une LIST, obtenu {autre:?}"),
    }
}

/// L'égalité de listes ne coerce PAS les éléments : `[1]` n'est pas `['1']`.
///
/// L'égalité structurelle descend dans les éléments ; si elle y applique la
/// coercition RDF, alors une liste d'entiers devient égale à une liste de
/// chaînes, et le type LIST<INTEGER> cesse d'être distinguable de
/// LIST<STRING> — que `IS TYPED LIST<INT>` prétend pourtant distinguer.
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : values_equal descend recursivement dans les listes en emportant
//   la coercition chaine/nombre
//   (obrain-core/src/execution/operators/filter.rs:3431 et :3424).
#[test]
fn l_egalite_de_listes_ne_coerce_pas_les_elements() {
    // DÉFAUT: obrain-core/src/execution/operators/filter.rs:3431 —
    // `(Value::List(a), Value::List(b))` compare élément par élément par
    // `Self::values_equal`, qui coerce `(String, Int64)` (filter.rs:3424).
    // Or `IS TYPED LIST<INT>` (gql_spec_compliance.rs:2434) affirme que
    // `[1,'hello',3]` n'est pas une LIST<INT> : les deux constructions se
    // contredisent sur le type des éléments.
    let db = semer(&[Value::Int64(1)]);
    assert_eq!(
        scalaire(&db, "[1] = ['1']"),
        Value::Bool(false),
        "une liste d'entiers n'est pas une liste de chaînes ; l'égalité de \
         listes doit respecter le type des éléments"
    );
}

// ===========================================================================
// G. IDENTITÉ DE TYPE DANS UNE COLONNE HÉTÉROGÈNE
//
// DISTINCT décide ce que « la même valeur » veut dire. Sur une base sans
// schéma, c'est la question de type la plus visible pour l'utilisateur.
// ===========================================================================

/// DISTINCT ne confond pas un entier et la chaîne qui lui ressemble.
#[test]
fn distinct_ne_confond_pas_un_entier_et_la_chaine_homographe() {
    let db = semer(&[Value::Int64(1), Value::String("1".into())]);
    let r = lire(&db, "MATCH (n:T) RETURN DISTINCT n.v AS x");
    let mut vus = etiquettes(&r.rows.iter().map(|l| l[0].clone()).collect::<Vec<_>>());
    vus.sort();
    assert_eq!(
        vus,
        vec!["i1", "s'1'"],
        "`1` et `'1'` sont deux valeurs de deux types : DISTINCT doit les garder \
         toutes les deux"
    );
}

/// DISTINCT est cohérent avec `=` : deux valeurs égales n'apparaissent qu'une
/// fois. `1` et `1.0` sont le même nombre — le moteur l'affirme lui-même par
/// `1 = 1.0`.
// DÉFAUT: la clé de déduplication encode Float64 par son motif binaire —
//   `Value::Float64(f) => KeyPart::Int64(f.to_bits() as i64)`,
//   obrain-core/src/execution/operators/distinct.rs:40.
#[cfg(feature = "defauts-connus")]
#[test]
fn distinct_est_coherent_avec_l_egalite_sur_les_nombres() {
    // DÉFAUT: obrain-core/src/execution/operators/distinct.rs:40 — `RowKey`
    // convertit `Value::Float64(f)` en `KeyPart::Int64(f.to_bits() as i64)`.
    // `Float64(1.0)` devient donc la clé 4607182418800017408, distincte de la
    // clé 1 de `Int64(1)`, alors que l'évaluateur
    // (obrain-core/src/execution/operators/filter.rs:3419) affirme
    // `Int64(1) = Float64(1.0)`.
    //
    // LE MÊME DÉFAUT EXISTE AILLEURS, ET CE N'EST PAS CELUI-LÀ QUE GQL EMPRUNTE.
    // `GroupKey` (obrain-core/src/execution/operators/aggregate.rs) fait
    // exactement la même conversion pour GROUP BY. La mutation M10 du registre
    // en tête de fichier l'a cassée : AUCUN test n'a rougi. C'est ainsi que la
    // référence de ce commentaire a été corrigée — sans la mutation, elle aurait
    // désigné le mauvais fichier, et quiconque aurait suivi la piste aurait
    // corrigé du code que DISTINCT n'exécute pas. Les deux sites portent le même
    // défaut et doivent bouger ensemble.
    let db = semer(&[Value::Int64(1), Value::Float64(1.0)]);
    assert_eq!(
        scalaire(&db, "1 = 1.0"),
        Value::Bool(true),
        "prémisse : le moteur affirme que 1 et 1.0 sont le même nombre"
    );
    let r = lire(&db, "MATCH (n:T) RETURN DISTINCT n.v AS x");
    assert_eq!(
        r.rows.len(),
        1,
        "conclusion : deux valeurs que `=` déclare égales ne peuvent pas être \
         deux valeurs distinctes. Obtenu : {:?}",
        etiquettes(&r.rows.iter().map(|l| l[0].clone()).collect::<Vec<_>>())
    );
}

/// Une propriété ABSENTE est NULL, et NULL n'est aucun des types concrets.
///
/// Sur une base sans schéma, l'absence est la forme normale du NULL : un nœud
/// hérité n'a tout simplement pas la clé. Ce test vérifie qu'elle ne se
/// déguise pas en valeur d'un type — un `0`, une chaîne vide, un `false`.
#[test]
fn une_propriete_absente_n_a_aucun_type_concret() {
    let db = ObrainDB::new_in_memory();
    {
        let s = db.session();
        s.create_node_with_props(&["T"], [("id", Value::Int64(0)), ("v", Value::Int64(1))]);
        s.create_node_with_props(&["T"], [("id", Value::Int64(1))]);
        s.create_node_with_props(&["Ancre"], [("k", Value::Int64(0))]);
    }
    assert_eq!(
        ids(&db, "MATCH (n:T) WHERE n.v IS NULL RETURN n.id"),
        vec![1],
        "prémisse : le nœud 1 n'a pas de propriété `v`, sa lecture est NULL"
    );
    assert_eq!(
        ids(&db, "MATCH (n:T) WHERE n.v IS TYPED INTEGER RETURN n.id"),
        vec![0],
        "seul le nœud 0 porte un entier ; l'absence n'en est pas un"
    );
    for t in ["STRING", "FLOAT", "BOOLEAN"] {
        assert_eq!(
            ids(
                &db,
                &format!("MATCH (n:T) WHERE n.v IS TYPED {t} RETURN n.id")
            ),
            Vec::<i64>::new(),
            "une propriété absente ne doit être d'aucun type concret, et {t} \
             n'est porté par aucun nœud de cette base"
        );
    }
}
