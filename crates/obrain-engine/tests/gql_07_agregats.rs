//! Conformité GQL — lot 07 : les AGRÉGATS.
//!
//! `count`, `sum`, `avg`, `min`, `max` — avec et sans groupement, sur ensemble
//! vide, en présence de NULL.
//!
//! RÈGLE D'ÉCRITURE DE CE FICHIER
//! -----------------------------
//! Chaque assertion encode **l'intention** — ce que la sémantique GQL/SQL doit
//! donner — et jamais l'observation du moteur. Là où le moteur diverge de la
//! sémantique, l'assertion reste conforme à la spécification et le test porte
//! `#[ignore = "DÉFAUT — …"]` avec un commentaire `// DÉFAUT:` qui explique
//! l'écart et désigne le code fautif. Plier l'assertion vers l'observation
//! gèlerait le défaut en spécification : c'est précisément ce qu'on refuse.
//!
//! LE PIÈGE PROPRE AUX AGRÉGATS
//! ----------------------------
//! Un agrégat rendant `0` peut signifier « zéro élément » ou « somme nulle ».
//! Le test `somme_nulle_et_ensemble_vide_ne_se_confondent_pas` les oppose
//! explicitement : sans lui, toute assertion `== 0` ne prouve rien.
//!
//! ```bash
//! cargo test -p obrain-engine --test gql_07_agregats
//! ```

use obrain_common::types::Value;
use obrain_engine::{Config, ObrainDB};

// ---------------------------------------------------------------------------
// Outillage — les états sont bâtis par le seul code de production (INSERT GQL
// et create_node_with_props), jamais en écrivant dans le substrat à la main.
// ---------------------------------------------------------------------------

/// Exécute `q` et rend l'unique ligne attendue, en échouant si le moteur n'en
/// rend pas exactement une. Le nombre de lignes fait partie de la sémantique
/// d'un agrégat scalaire : on ne le laisse jamais implicite.
fn ligne_unique(db: &ObrainDB, q: &str) -> Vec<Value> {
    let s = db.session();
    let r = s
        .execute(q)
        .unwrap_or_else(|e| panic!("la requête a échoué — {q}\n  erreur : {e}"));
    assert_eq!(
        r.rows.len(),
        1,
        "un agrégat scalaire doit rendre exactement UNE ligne, même sur ensemble vide — {q}"
    );
    r.rows[0].clone()
}

/// Rend la valeur scalaire unique de `q`.
fn scalaire(db: &ObrainDB, q: &str) -> Value {
    let l = ligne_unique(db, q);
    assert_eq!(l.len(), 1, "une seule colonne attendue — {q}");
    l[0].clone()
}

/// Toutes les lignes de `q`, triées pour une comparaison stable.
fn lignes(db: &ObrainDB, q: &str) -> Vec<Vec<Value>> {
    let s = db.session();
    let r = s
        .execute(q)
        .unwrap_or_else(|e| panic!("la requête a échoué — {q}\n  erreur : {e}"));
    r.rows.clone()
}

/// Cinq nœuds `:T`. Trois portent `x`, deux ne le portent pas : c'est ce qui
/// oppose `count(*)` (5 lignes) à `count(t.x)` (3 valeurs non nulles).
/// Trois portent `g`, un ne le porte pas : c'est le groupe NULL.
fn base_avec_nulls() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    s.execute("INSERT (:T {nom: 'a', x: 1, g: 'G1'})").unwrap();
    s.execute("INSERT (:T {nom: 'b', x: 2, g: 'G1'})").unwrap();
    s.execute("INSERT (:T {nom: 'c', x: 3, g: 'G2'})").unwrap();
    s.execute("INSERT (:T {nom: 'd', g: 'G2'})").unwrap();
    s.execute("INSERT (:T {nom: 'e'})").unwrap();
    s.commit().unwrap();
    db
}

// ===========================================================================
// GROUPE A — `count(*)` contre `count(x)` en présence de NULL
// ===========================================================================

/// `count(*)` compte les LIGNES : les deux nœuds sans `x` comptent aussi.
#[test]
fn count_etoile_compte_les_lignes_y_compris_celles_a_valeur_nulle() {
    let db = base_avec_nulls();
    assert_eq!(
        scalaire(&db, "MATCH (t:T) RETURN count(*)"),
        Value::Int64(5),
        "count(*) compte les lignes de l'ensemble, indépendamment de toute valeur"
    );
}

/// `count(x)` compte les VALEURS NON NULLES : seuls trois nœuds portent `x`.
#[test]
fn count_expression_ne_compte_que_les_valeurs_non_nulles() {
    let db = base_avec_nulls();
    assert_eq!(
        scalaire(&db, "MATCH (t:T) RETURN count(t.x)"),
        Value::Int64(3),
        "count(expr) écarte les NULL : 3 nœuds sur 5 portent x"
    );
}

/// Le cœur de la distinction : dans une même projection, sur les mêmes lignes,
/// les deux formes doivent différer. Un moteur qui les confondrait rendrait
/// deux fois la même valeur — et ce test est le seul à l'attraper.
#[test]
fn count_etoile_et_count_expression_different_sur_la_meme_projection() {
    let db = base_avec_nulls();
    let l = ligne_unique(&db, "MATCH (t:T) RETURN count(*), count(t.x)");
    assert_eq!(l[0], Value::Int64(5), "count(*) = nombre de lignes");
    assert_eq!(l[1], Value::Int64(3), "count(t.x) = valeurs non nulles");
    assert_ne!(
        l[0], l[1],
        "la distinction lignes/valeurs doit être observable sur une donnée qui contient des NULL"
    );
}

/// `count(v)` sur une VARIABLE de nœud vaut `count(*)` : une variable liée par
/// un motif désigne toujours un nœud, elle n'est jamais nulle.
#[test]
fn count_sur_variable_de_noeud_equivaut_a_count_etoile() {
    let db = base_avec_nulls();
    let l = ligne_unique(&db, "MATCH (t:T) RETURN count(*), count(t)");
    assert_eq!(
        l[0], l[1],
        "une variable de nœud liée n'est jamais nulle : count(t) doit valoir count(*)"
    );
    assert_eq!(l[1], Value::Int64(5));
}

/// Un NULL écrit explicitement doit se comporter comme une propriété absente :
/// compté par `count(*)`, écarté par `count(expr)`, `sum` et `avg`.
#[test]
fn null_ecrit_explicitement_se_comporte_comme_propriete_absente() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    s.execute("INSERT (:N {v: null, k: 1})").unwrap();
    s.execute("INSERT (:N {v: 5, k: 2})").unwrap();
    s.commit().unwrap();

    let l = ligne_unique(
        &db,
        "MATCH (n:N) RETURN count(*), count(n.v), sum(n.v), avg(n.v)",
    );
    assert_eq!(l[0], Value::Int64(2), "deux lignes existent");
    assert_eq!(l[1], Value::Int64(1), "une seule valeur non nulle");
    assert_eq!(l[2], Value::Int64(5), "sum ignore le NULL");
    assert_eq!(
        l[3],
        Value::Float64(5.0),
        "avg divise par le nombre de NON NULS (1), pas par le nombre de lignes (2)"
    );
}

// ===========================================================================
// GROUPE B — L'ENSEMBLE VIDE
// C'est le cas le plus fautif de la famille : aucune donnée de production ne
// le produit spontanément, et chaque agrégat y a une réponse canonique
// DIFFÉRENTE. On les sépare donc un par un.
// ===========================================================================

/// Un agrégat scalaire sur ensemble vide rend UNE ligne — jamais zéro ligne.
/// C'est la propriété que les moteurs manquent le plus souvent : sans clause
/// de groupement, l'ensemble vide est un groupe, pas une absence de groupe.
#[test]
fn agregat_scalaire_sur_ensemble_vide_rend_une_ligne_et_non_zero_ligne() {
    let db = base_avec_nulls();
    // `ligne_unique` échoue si le moteur rend 0 ligne : c'est là l'assertion.
    let l = ligne_unique(
        &db,
        "MATCH (t:Inexistant) RETURN count(*), sum(t.x), avg(t.x), min(t.x), max(t.x)",
    );
    assert_eq!(l.len(), 5);
}

/// `count` sur ensemble vide vaut 0 — le seul agrégat de la famille à rendre
/// une valeur et non NULL.
#[test]
fn count_sur_ensemble_vide_vaut_zero() {
    let db = base_avec_nulls();
    assert_eq!(
        scalaire(&db, "MATCH (t:Inexistant) RETURN count(*)"),
        Value::Int64(0)
    );
    assert_eq!(
        scalaire(&db, "MATCH (t:Inexistant) RETURN count(t.x)"),
        Value::Int64(0)
    );
}

/// `sum` sur ensemble vide rend NULL, pas 0.
///
/// AMBIGUÏTÉ ASSUMÉE — les deux familles de spécifications divergent :
///   * SQL / ISO GQL : `SUM` sur ensemble vide vaut NULL ;
///   * openCypher : `sum()` sur ensemble vide vaut 0.
/// Ce fichier teste la conformité **GQL**, donc NULL. Si le projet tranchait
/// un jour pour la lecture openCypher, ce test devrait être retourné
/// sciemment — et non « corrigé » parce qu'il rougit.
#[test]
fn sum_sur_ensemble_vide_rend_null_et_non_zero() {
    let db = base_avec_nulls();
    assert_eq!(
        scalaire(&db, "MATCH (t:Inexistant) RETURN sum(t.x)"),
        Value::Null,
        "SUM sur ensemble vide vaut NULL en SQL/GQL — 0 serait la lecture openCypher"
    );
}

/// `avg` sur ensemble vide rend NULL : il n'y a pas de division par zéro, donc
/// ni NaN ni erreur.
#[test]
fn avg_sur_ensemble_vide_rend_null_sans_nan_ni_erreur() {
    let db = base_avec_nulls();
    let v = scalaire(&db, "MATCH (t:Inexistant) RETURN avg(t.x)");
    assert_eq!(v, Value::Null, "AVG sur ensemble vide vaut NULL");
    // Une division 0/0 non gardée produirait NaN : on le refuse explicitement.
    if let Value::Float64(f) = v {
        panic!(
            "avg a rendu un flottant {f} (NaN ? {}) au lieu de NULL",
            f.is_nan()
        );
    }
}

/// `min` et `max` sur ensemble vide rendent NULL.
#[test]
fn min_et_max_sur_ensemble_vide_rendent_null() {
    let db = base_avec_nulls();
    assert_eq!(
        scalaire(&db, "MATCH (t:Inexistant) RETURN min(t.x)"),
        Value::Null
    );
    assert_eq!(
        scalaire(&db, "MATCH (t:Inexistant) RETURN max(t.x)"),
        Value::Null
    );
}

/// `collect` sur ensemble vide rend la LISTE VIDE, pas NULL : c'est l'exception
/// de la famille, et l'opposer aux autres est le seul moyen de le prouver.
#[test]
fn collect_sur_ensemble_vide_rend_liste_vide_et_non_null() {
    let db = base_avec_nulls();
    let l = ligne_unique(
        &db,
        "MATCH (t:Inexistant) RETURN collect(t.x), sum(t.x), count(*)",
    );
    assert_eq!(
        l[0],
        Value::List(Vec::new().into()),
        "collect sur ensemble vide rend [] — l'élément neutre de la concaténation"
    );
    assert_eq!(l[1], Value::Null, "sum n'a pas d'élément neutre : NULL");
    assert_eq!(l[2], Value::Int64(0), "count a pour neutre 0");
}

/// LE PIÈGE DU LOT. `0` rendu par un agrégat peut vouloir dire « zéro élément »
/// ou « somme nulle ». Les deux situations doivent être distinguables, sinon
/// aucune assertion `== 0` de ce fichier ne prouve quoi que ce soit.
#[test]
fn somme_nulle_et_ensemble_vide_ne_se_confondent_pas() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    s.execute("INSERT (:Z {v: 5})").unwrap();
    s.execute("INSERT (:Z {v: -5})").unwrap();
    s.commit().unwrap();

    // Deux éléments dont la somme vaut zéro.
    let peuplee = ligne_unique(&db, "MATCH (z:Z) RETURN count(*), sum(z.v)");
    assert_eq!(peuplee[0], Value::Int64(2), "l'ensemble n'est PAS vide");
    assert_eq!(
        peuplee[1],
        Value::Int64(0),
        "la somme de 5 et -5 vaut zéro : un vrai zéro arithmétique"
    );

    // Aucun élément.
    let vide = ligne_unique(&db, "MATCH (z:Inexistant) RETURN count(*), sum(z.v)");
    assert_eq!(vide[0], Value::Int64(0), "l'ensemble est vide");
    assert_eq!(
        vide[1],
        Value::Null,
        "sum sur ensemble vide rend NULL — c'est ce qui le distingue d'une somme nulle"
    );

    // La distinction, énoncée : même `sum`, sens opposés.
    assert_ne!(
        peuplee[1], vide[1],
        "« somme nulle » et « aucun élément » DOIVENT être discernables au niveau de sum ; \
         si les deux rendaient 0, aucune assertion sum == 0 de ce fichier ne prouverait rien"
    );
    // Et count seul ne suffit pas à les séparer si sum les confond :
    assert_ne!(peuplee[0], vide[0]);
}

/// Un ensemble vidé par un FILTRE (et non par un label absent) suit exactement
/// la même sémantique : le chemin d'exécution diffère, la sémantique non.
#[test]
fn ensemble_vide_par_filtre_suit_la_meme_semantique_que_label_absent() {
    let db = base_avec_nulls();
    let l = ligne_unique(
        &db,
        "MATCH (t:T) WHERE t.x > 999 RETURN count(*), sum(t.x), avg(t.x), min(t.x)",
    );
    assert_eq!(l[0], Value::Int64(0));
    assert_eq!(l[1], Value::Null);
    assert_eq!(l[2], Value::Null);
    assert_eq!(l[3], Value::Null);
}

/// AVEC clause de groupement, l'ensemble vide rend ZÉRO ligne — et non une
/// ligne de NULL. C'est l'exact opposé du cas scalaire, et c'est voulu :
/// il n'y a aucun groupe à décrire.
#[test]
fn agregat_groupe_sur_ensemble_vide_rend_zero_ligne() {
    let db = base_avec_nulls();
    let r = lignes(&db, "MATCH (t:Inexistant) RETURN t.g, count(*)");
    assert_eq!(
        r.len(),
        0,
        "sans aucun groupe, un agrégat groupé ne décrit rien : zéro ligne, \
         là où l'agrégat scalaire en rend une"
    );
}

// ===========================================================================
// GROUPE C — TYPAGE DU RÉSULTAT
// ===========================================================================

/// `avg` d'ENTIERS rend un FLOTTANT, jamais un entier tronqué.
/// La moyenne de 1 et 2 vaut 1.5 ; un moteur qui tronquerait rendrait 1.
#[test]
fn avg_d_entiers_rend_un_flottant_non_tronque() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    s.execute("INSERT (:A {v: 1})").unwrap();
    s.execute("INSERT (:A {v: 2})").unwrap();
    s.commit().unwrap();

    let v = scalaire(&db, "MATCH (a:A) RETURN avg(a.v)");
    assert_eq!(
        v,
        Value::Float64(1.5),
        "avg(1, 2) vaut 1.5 : une troncature entière rendrait 1 et perdrait la moitié"
    );
    assert!(
        matches!(v, Value::Float64(_)),
        "le type de sortie d'AVG est flottant même sur entrée entière"
    );
}

/// `sum` d'entiers reste ENTIER : elle ne promeut pas gratuitement en flottant
/// (ce qui perdrait de la précision au-delà de 2^53).
#[test]
fn sum_d_entiers_reste_entiere() {
    let db = base_avec_nulls();
    let v = scalaire(&db, "MATCH (t:T) RETURN sum(t.x)");
    assert_eq!(v, Value::Int64(6), "1 + 2 + 3 = 6");
    assert!(
        matches!(v, Value::Int64(_)),
        "sum sur colonne entière doit rester entière"
    );
}

/// `sum` de flottants rend un FLOTTANT.
#[test]
fn sum_de_flottants_rend_un_flottant() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    s.execute("INSERT (:F {v: 1.5})").unwrap();
    s.execute("INSERT (:F {v: 2.25})").unwrap();
    s.commit().unwrap();
    assert_eq!(
        scalaire(&db, "MATCH (f:F) RETURN sum(f.v)"),
        Value::Float64(3.75)
    );
}

/// L'ordre d'accumulation d'une somme flottante EST observable : l'addition
/// IEEE-754 n'est pas associative. La spécification GQL ne prescrit pas
/// d'ordre ; ce qu'on peut donc exiger, et ce qu'on exige ici, c'est le
/// DÉTERMINISME — deux exécutions de la même requête sur la même donnée
/// doivent rendre le même flottant, bit pour bit.
///
/// AMBIGUÏTÉ ASSUMÉE : sur ce jeu, l'accumulation de gauche à droite donne 3.5
/// tandis que la somme exacte vaut 3.0. Aucune des deux n'est « fausse » au
/// regard de la spec ; on n'en fige donc aucune, on fige la reproductibilité.
#[test]
fn sum_flottante_est_deterministe_meme_si_non_associative() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    s.execute("INSERT (:F {i: 0, v: 1.0e16})").unwrap();
    s.execute("INSERT (:F {i: 1, v: 1.5})").unwrap();
    s.execute("INSERT (:F {i: 2, v: -1.0e16})").unwrap();
    s.execute("INSERT (:F {i: 3, v: 1.5})").unwrap();
    s.commit().unwrap();

    let q = "MATCH (f:F) RETURN sum(f.v)";
    let a = scalaire(&db, q);
    let b = scalaire(&db, q);
    let c = scalaire(&db, q);
    assert_eq!(a, b, "sum flottante doit être reproductible");
    assert_eq!(b, c, "sum flottante doit être reproductible");

    // Le résultat doit rester un flottant fini : ni NaN ni infini.
    match a {
        Value::Float64(f) => assert!(
            f.is_finite(),
            "l'accumulation ne doit produire ni NaN ni infini, obtenu {f}"
        ),
        autre => panic!("sum de flottants doit rendre un flottant, obtenu {autre:?}"),
    }
}

// ===========================================================================
// GROUPE D — `min` / `max` SUR LES TYPES
// ===========================================================================

/// `min`/`max` sur ENTIERS.
#[test]
fn min_et_max_sur_entiers() {
    let db = base_avec_nulls();
    let l = ligne_unique(&db, "MATCH (t:T) RETURN min(t.x), max(t.x)");
    assert_eq!(l[0], Value::Int64(1));
    assert_eq!(l[1], Value::Int64(3));
}

/// `min`/`max` sur CHAÎNES suivent l'ordre lexicographique et rendent une
/// chaîne.
///
/// DÉFAUT: le moteur rend `Int64(0)`. Le type de sortie de MIN/MAX est câblé à
/// `LogicalType::Int64` dans le planificateur
/// (obrain-engine/src/query/planner/lpg/aggregate.rs, branche `Min | Max`),
/// juste sous un commentaire qui affirme pourtant « MIN/MAX preserve input
/// type ». Toute valeur non entière est écrasée à 0 en silence à l'écriture
/// dans la colonne de sortie. `Sum` utilise `LogicalType::Any` au même endroit
/// et n'a pas le défaut.
#[test]
// CORRIGÉ (vert par intégration E+F, revue vague 3 — MIN/MAX LogicalType::Any du correctif E) — défaut d'origine : MIN/MAX câblaient leur type de sortie à Int64 et écrasaient chaîne et flottant à Int64(0) ; voir atelier gql07
fn min_et_max_sur_chaines_suivent_l_ordre_lexicographique() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    for n in ["poire", "abricot", "cerise"] {
        s.execute(&format!("INSERT (:S {{nom: '{n}'}})")).unwrap();
    }
    s.commit().unwrap();

    let l = ligne_unique(&db, "MATCH (x:S) RETURN min(x.nom), max(x.nom)");
    assert_eq!(
        l[0],
        Value::String("abricot".into()),
        "min lexicographique de {{poire, abricot, cerise}}"
    );
    assert_eq!(
        l[1],
        Value::String("poire".into()),
        "max lexicographique de {{poire, abricot, cerise}}"
    );
}

/// `min`/`max` sur FLOTTANTS préservent le flottant.
///
/// DÉFAUT: même cause que ci-dessus — la sortie est câblée en Int64, donc
/// `min(1.5, 2.5)` rend `Int64(0)`. C'est le visage le plus dommageable du
/// défaut : `min`/`max` sur une colonne flottante est une requête ordinaire,
/// et elle rend zéro sans lever la moindre erreur.
#[test]
// CORRIGÉ (vert par intégration E+F, revue vague 3 — MIN/MAX LogicalType::Any du correctif E) — défaut d'origine : MIN/MAX sur flottants rendaient Int64(0) au lieu du flottant ; voir atelier gql07
fn min_et_max_sur_flottants_preservent_le_flottant() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    s.execute("INSERT (:F {v: 1.5})").unwrap();
    s.execute("INSERT (:F {v: 2.5})").unwrap();
    s.commit().unwrap();

    let l = ligne_unique(&db, "MATCH (f:F) RETURN min(f.v), max(f.v)");
    assert_eq!(l[0], Value::Float64(1.5));
    assert_eq!(l[1], Value::Float64(2.5));
}

/// Propriété minimale, indépendante de tout ordre entre types : le résultat de
/// `min`/`max` doit être L'UNE DES VALEURS D'ENTRÉE. Un moteur peut légitimement
/// choisir son ordre entre types hétérogènes, il ne peut pas inventer une
/// valeur que personne n'a insérée.
///
/// AMBIGUÏTÉ ASSUMÉE : l'ordre entre types différents n'est pas fixé par ISO
/// GQL (openCypher en impose un, SQL rejette la comparaison). On n'exige donc
/// PAS une valeur précise — seulement l'appartenance à l'entrée.
///
/// DÉFAUT: le moteur rend `Int64(1)` pour min ET pour max sur {1, 'texte',
/// true}. La valeur appartient certes à l'entrée, mais min == max sur trois
/// valeurs distinctes est impossible.
///
/// CAUSE DISTINCTE DE CELLE DES DEUX TESTS PRÉCÉDENTS — ne pas les confondre.
/// Ici le type de sortie câblé en Int64 n'est PAS en cause. La cause est dans
/// obrain-core/src/execution/operators/value_utils.rs : `compare_values` se
/// termine par un bras `_ => None` pour toute paire de types incomparables, et
/// `is_less_than` / `is_greater_than` décident par
/// `compare_values(new, curr) == Some(Ordering::Less | Greater)`. Une
/// comparaison impossible rend donc `None`, le test est FAUX, et la valeur
/// déjà en place n'est JAMAIS délogée. Sur {1, 'texte', true}, `1` s'installe
/// en premier et plus rien ne peut le remplacer — d'où min == max == 1.
///
/// CONSÉQUENCE POUR QUI CORRIGERA : réparer le type de sortie (Int64 -> Any)
/// repassera au vert les deux tests précédents et laissera CELUI-CI rouge.
/// Deux défauts indépendants produisent ici la même apparence ; refermer le
/// dossier sur la seule correction du type serait une erreur.
#[test]
// CORRIGÉ (vert par intégration E+F, revue vague 3 — incumbent délogé via comparer_total du correctif E) — défaut d'origine : une valeur incomparable ne délogeait jamais l'occupant (compare_values rendait None) ; voir atelier gql07
fn min_et_max_sur_types_melanges_rendent_une_valeur_presente_en_entree() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    s.execute("INSERT (:M {v: 1})").unwrap();
    s.execute("INSERT (:M {v: 'texte'})").unwrap();
    s.execute("INSERT (:M {v: true})").unwrap();
    s.commit().unwrap();

    let entrees = [
        Value::Int64(1),
        Value::String("texte".into()),
        Value::Bool(true),
    ];
    let l = ligne_unique(&db, "MATCH (m:M) RETURN min(m.v), max(m.v)");
    assert!(
        entrees.contains(&l[0]),
        "min doit rendre une valeur réellement présente en entrée, obtenu {:?}",
        l[0]
    );
    assert!(
        entrees.contains(&l[1]),
        "max doit rendre une valeur réellement présente en entrée, obtenu {:?}",
        l[1]
    );
    assert_ne!(
        l[0], l[1],
        "sur trois valeurs distinctes, min et max ne peuvent pas coïncider"
    );
}

// ===========================================================================
// GROUPE E — COLONNE ENTIÈREMENT NULLE
// ===========================================================================

/// Une colonne entièrement nulle se comporte comme un ensemble vide POUR LES
/// AGRÉGATS DE VALEURS, alors que `count(*)` continue de compter les lignes.
/// C'est le seul endroit où les deux notions se séparent proprement.
#[test]
fn colonne_entierement_nulle_vide_les_agregats_de_valeurs_mais_pas_count_etoile() {
    let db = base_avec_nulls();
    let l = ligne_unique(
        &db,
        "MATCH (t:T) RETURN count(*), count(t.absente), sum(t.absente), avg(t.absente), min(t.absente)",
    );
    assert_eq!(
        l[0],
        Value::Int64(5),
        "les lignes existent : count(*) les compte"
    );
    assert_eq!(l[1], Value::Int64(0), "aucune valeur non nulle");
    assert_eq!(l[2], Value::Null, "sum sans valeur rend NULL");
    assert_eq!(l[3], Value::Null, "avg sans valeur rend NULL");
    assert_eq!(l[4], Value::Null, "min sans valeur rend NULL");
    assert_ne!(
        l[0], l[1],
        "count(*) et count(colonne toute nulle) doivent se séparer : \
         5 lignes existent, 0 valeur existe"
    );
}

// ===========================================================================
// GROUPE F — GROUPEMENT
// ===========================================================================

/// Un groupe de clé NULL existe de plein droit : les lignes dont la clé de
/// groupement est nulle ne sont pas écartées, elles forment leur propre groupe.
#[test]
fn le_groupement_par_cle_nulle_produit_un_groupe_null_de_plein_droit() {
    let db = base_avec_nulls();
    let r = lignes(&db, "MATCH (t:T) RETURN t.g AS g, count(*) AS n");
    assert_eq!(r.len(), 3, "trois groupes attendus : G1, G2, et NULL");

    let groupe_null = r
        .iter()
        .find(|l| l[0] == Value::Null)
        .expect("un groupe de clé NULL doit exister, les lignes sans clé ne sont pas écartées");
    assert_eq!(
        groupe_null[1],
        Value::Int64(1),
        "un seul nœud (« e ») n'a pas de g"
    );

    // Et l'ensemble des lignes est conservé : aucun groupement ne perd de ligne.
    let total: i64 = r
        .iter()
        .map(|l| match l[1] {
            Value::Int64(n) => n,
            _ => panic!("count doit rendre un entier"),
        })
        .sum();
    assert_eq!(
        total, 5,
        "la somme des effectifs de groupes doit égaler le nombre de lignes"
    );
}

/// `count(x)` par groupe respecte les NULL À L'INTÉRIEUR de chaque groupe :
/// le groupe NULL contient une ligne mais zéro valeur.
#[test]
fn count_expression_par_groupe_respecte_les_nulls_internes_au_groupe() {
    let db = base_avec_nulls();
    let r = lignes(
        &db,
        "MATCH (t:T) RETURN t.g AS g, count(*) AS lignes, count(t.x) AS valeurs",
    );
    let trouve = |g: Value| -> Vec<Value> {
        r.iter()
            .find(|l| l[0] == g)
            .unwrap_or_else(|| panic!("groupe {g:?} absent"))
            .clone()
    };

    let g1 = trouve(Value::String("G1".into()));
    assert_eq!(g1[1], Value::Int64(2), "G1 a deux lignes");
    assert_eq!(g1[2], Value::Int64(2), "les deux portent x");

    let g2 = trouve(Value::String("G2".into()));
    assert_eq!(g2[1], Value::Int64(2), "G2 a deux lignes");
    assert_eq!(g2[2], Value::Int64(1), "une seule porte x");
    assert_ne!(
        g2[1], g2[2],
        "la distinction lignes/valeurs vaut aussi par groupe"
    );

    let gnull = trouve(Value::Null);
    assert_eq!(gnull[1], Value::Int64(1), "le groupe NULL a une ligne");
    assert_eq!(gnull[2], Value::Int64(0), "et zéro valeur non nulle");
}

/// Propriété absente et NULL explicitement stocké s'effondrent dans LE MÊME
/// groupe : il n'existe qu'une seule notion de « pas de valeur ».
#[test]
fn propriete_absente_et_null_stocke_tombent_dans_le_meme_groupe() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    s.execute("INSERT (:U {g: null, i: 1})").unwrap();
    s.execute("INSERT (:U {i: 2})").unwrap();
    s.execute("INSERT (:U {g: 'reel', i: 3})").unwrap();
    s.commit().unwrap();

    let r = lignes(&db, "MATCH (u:U) RETURN u.g AS g, count(*) AS n");
    assert_eq!(
        r.len(),
        2,
        "deux groupes seulement : le groupe NULL (absent + null stocké) et « reel »"
    );
    let gnull = r
        .iter()
        .find(|l| l[0] == Value::Null)
        .expect("groupe NULL attendu");
    assert_eq!(
        gnull[1],
        Value::Int64(2),
        "absent et null stocké doivent se confondre en un seul groupe de 2"
    );
}

/// Grouper sur une clé FLOTTANTE doit rendre la clé telle qu'elle a été
/// stockée.
///
/// DÉFAUT: la clé ressort en `Int64` portant le MOTIF BINAIRE du flottant —
/// `1.5` devient `Int64(4609434218613702656)`, soit `f64::to_bits`. Voir
/// `GroupKey::from_row` dans
/// obrain-core/src/execution/operators/aggregate.rs : la clé encode
/// `Value::Float64(f)` en `GroupKeyPart::Int64(f.to_bits() as i64)` et la
/// reconstruction `to_values()` n'a aucune branche Float64 pour défaire
/// l'encodage. Le regroupement lui-même est correct (les effectifs sont
/// justes) ; c'est la restitution de la clé qui est corrompue. Requête
/// parfaitement ordinaire : atteignable en production.
///
/// CORRIGÉ (fix/gql-divers) : `GroupKeyPart::Float64Bits(u64)` conserve le
/// regroupement par bits (injectif) et `to_values()` restitue la valeur via
/// `f64::from_bits`. NaN identiques groupés ensemble ; ±0.0 distincts.
#[test]
fn le_groupement_par_cle_flottante_restitue_la_cle_flottante() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    s.execute("INSERT (:P {cle: 1.5, n: 1})").unwrap();
    s.execute("INSERT (:P {cle: 1.5, n: 2})").unwrap();
    s.execute("INSERT (:P {cle: 2.5, n: 3})").unwrap();
    s.commit().unwrap();

    let r = lignes(&db, "MATCH (x:P) RETURN x.cle AS c, count(*) AS n");
    assert_eq!(r.len(), 2, "deux clés distinctes : 1.5 et 2.5");

    let mut cles: Vec<Value> = r.iter().map(|l| l[0].clone()).collect();
    cles.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    assert!(
        cles.contains(&Value::Float64(1.5)),
        "la clé 1.5 doit ressortir en Float64(1.5), obtenu {cles:?}"
    );
    assert!(
        cles.contains(&Value::Float64(2.5)),
        "la clé 2.5 doit ressortir en Float64(2.5), obtenu {cles:?}"
    );
}

/// Une projection qui mêle une colonne NUE et un agrégat groupe implicitement
/// par la colonne nue (sémantique GQL/Cypher, à l'inverse de SQL strict qui
/// exigerait un GROUP BY explicite).
#[test]
fn colonne_nue_a_cote_d_un_agregat_groupe_implicitement_par_cette_colonne() {
    let db = base_avec_nulls();
    let r = lignes(&db, "MATCH (t:T) RETURN t.g AS g, count(*) AS n");
    assert_eq!(
        r.len(),
        3,
        "le groupement implicite se fait sur la seule colonne nue (t.g) : 3 groupes"
    );

    // Avec une colonne nue distincte par ligne, on retrouve une ligne par nœud.
    let r2 = lignes(&db, "MATCH (t:T) RETURN t.nom AS nom, count(*) AS n");
    assert_eq!(r2.len(), 5, "t.nom est unique : 5 groupes d'une ligne");
    for l in &r2 {
        assert_eq!(l[1], Value::Int64(1));
    }
}

// ===========================================================================
// GROUPE G — DISTINCT
// ===========================================================================

/// `count(DISTINCT x)` compte les valeurs distinctes NON NULLES.
#[test]
fn count_distinct_ignore_les_doublons_et_les_nulls() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    s.execute("INSERT (:D {v: 1})").unwrap();
    s.execute("INSERT (:D {v: 1})").unwrap();
    s.execute("INSERT (:D {v: 2})").unwrap();
    s.execute("INSERT (:D {})").unwrap();
    s.commit().unwrap();

    let l = ligne_unique(
        &db,
        "MATCH (d:D) RETURN count(*), count(d.v), count(DISTINCT d.v)",
    );
    assert_eq!(l[0], Value::Int64(4), "quatre lignes");
    assert_eq!(l[1], Value::Int64(3), "trois valeurs non nulles");
    assert_eq!(
        l[2],
        Value::Int64(2),
        "deux valeurs DISTINCTES non nulles : {{1, 2}} — le NULL ne compte pas comme une valeur"
    );
    // Les trois doivent se séparer deux à deux : c'est ce qui prouve que
    // DISTINCT et non-nullité sont deux filtres indépendants.
    assert_ne!(l[0], l[1]);
    assert_ne!(l[1], l[2]);
}

/// `sum(DISTINCT x)` et `avg(DISTINCT x)` agrègent l'ENSEMBLE des valeurs
/// distinctes.
#[test]
fn sum_et_avg_distinct_agregent_les_valeurs_distinctes() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    s.execute("INSERT (:D {v: 1})").unwrap();
    s.execute("INSERT (:D {v: 1})").unwrap();
    s.execute("INSERT (:D {v: 2})").unwrap();
    s.execute("INSERT (:D {})").unwrap();
    s.commit().unwrap();

    let l = ligne_unique(
        &db,
        "MATCH (d:D) RETURN sum(d.v), sum(DISTINCT d.v), avg(d.v), avg(DISTINCT d.v)",
    );
    assert_eq!(l[0], Value::Int64(4), "1 + 1 + 2");
    assert_eq!(l[1], Value::Int64(3), "1 + 2 sur les valeurs distinctes");
    assert_eq!(l[2], Value::Float64(4.0 / 3.0), "moyenne de 1, 1, 2");
    assert_eq!(l[3], Value::Float64(1.5), "moyenne de 1 et 2");
    assert_ne!(l[0], l[1], "DISTINCT doit changer le résultat de sum");
}

/// `count(DISTINCT x)` sur ensemble vide vaut 0.
#[test]
fn count_distinct_sur_ensemble_vide_vaut_zero() {
    let db = base_avec_nulls();
    assert_eq!(
        scalaire(&db, "MATCH (t:Inexistant) RETURN count(DISTINCT t.x)"),
        Value::Int64(0)
    );
}

// ===========================================================================
// GROUPE H — AGRÉGAT IMBRIQUÉ DANS UNE EXPRESSION, ET DANS `ORDER BY`
// ===========================================================================

/// Un agrégat s'imbrique dans une expression arithmétique.
#[test]
fn un_agregat_s_imbrique_dans_une_expression_arithmetique() {
    let db = base_avec_nulls();
    let l = ligne_unique(
        &db,
        "MATCH (t:T) RETURN count(*) * 2 AS double, sum(t.x) + 1 AS decale",
    );
    assert_eq!(l[0], Value::Int64(10), "5 lignes × 2");
    assert_eq!(l[1], Value::Int64(7), "sum(1,2,3) + 1");
}

/// DEUX agrégats doivent pouvoir se combiner dans une même expression :
/// `sum(x) / count(x)` est la définition même de la moyenne et doit donc
/// s'accorder avec `avg(x)`.
///
/// DÉFAUT: le moteur rend NULL pour toute expression combinant DEUX agrégats,
/// alors qu'un agrégat combiné à un littéral (test précédent) fonctionne. La
/// réécriture d'expression n'expose visiblement qu'un seul agrégat par
/// expression au projecteur.
///
/// CORRIGÉ (fix/gql-divers) : `extract_wrapped_aggregates_into` accumule
/// TOUS les agrégats d'une expression (aliases synthétiques distincts) au
/// lieu de n'extraire que le premier et de laisser l'autre s'évaluer en NULL.
#[test]
fn deux_agregats_se_combinent_dans_une_meme_expression() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    for v in [2, 4, 6] {
        s.execute(&format!("INSERT (:E {{v: {v}}})")).unwrap();
    }
    s.commit().unwrap();

    let l = ligne_unique(
        &db,
        "MATCH (e:E) RETURN sum(e.v) / count(e.v) AS moyenne_manuelle, avg(e.v) AS moyenne",
    );
    assert_eq!(
        l[0],
        Value::Int64(4),
        "sum(2,4,6)=12 divisé par count=3 vaut 4"
    );
    assert_eq!(l[1], Value::Float64(4.0));
}

/// `ORDER BY` sur l'ALIAS d'un agrégat trie bien par la valeur agrégée.
/// (Le test correspondant de `gql_spec_compliance.rs` n'assertait que
/// `!rows.is_empty()` et ne vérifiait donc aucun ordre.)
#[test]
fn order_by_sur_alias_d_agregat_trie_par_la_valeur_agregee() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    for (g, n) in [("A", 1), ("A", 2), ("B", 3), ("C", 4), ("C", 5), ("C", 6)] {
        s.execute(&format!("INSERT (:G {{g: '{g}', n: {n}}})"))
            .unwrap();
    }
    s.commit().unwrap();

    let r = lignes(
        &db,
        "MATCH (x:G) RETURN x.g AS g, count(*) AS n ORDER BY n DESC",
    );
    assert_eq!(r.len(), 3);
    assert_eq!(
        r.iter().map(|l| l[0].clone()).collect::<Vec<_>>(),
        vec![
            Value::String("C".into()),
            Value::String("A".into()),
            Value::String("B".into())
        ],
        "l'ordre doit être décroissant par effectif : C(3), A(2), B(1)"
    );
    assert_eq!(
        r.iter().map(|l| l[1].clone()).collect::<Vec<_>>(),
        vec![Value::Int64(3), Value::Int64(2), Value::Int64(1)],
        "les effectifs eux-mêmes doivent décroître — c'est ce que « trier » signifie"
    );
}

/// `ORDER BY count(*)` — l'agrégat écrit DIRECTEMENT dans la clause de tri,
/// sans passer par un alias — doit trier comme la forme aliasée.
///
/// DÉFAUT: le moteur lève une erreur interne
/// « OBRAIN-X001: Internal error: Function column 'count' not found for
/// ORDER BY (from count()) ». Seule la forme aliasée fonctionne. Même symptôme
/// avec `ORDER BY sum(x)`. Les deux tests `test_aggregate_in_order_by` et
/// `test_aggregate_order_by_alias` de `gql_spec_compliance.rs` prétendent
/// couvrir « GF20 : Aggregate functions in ORDER BY » : ils sont en réalité
/// mot pour mot IDENTIQUES l'un à l'autre, n'exercent que la forme aliasée, et
/// n'assertent que `!result.rows.is_empty()`. La forme directe — celle que
/// nomme GF20 — n'est couverte par aucun des deux.
///
/// CORRIGÉ (fix/gql-divers) : une clé de tri agrégat structurellement
/// identique à un item aliasé du RETURN est réécrite en Variable(alias) —
/// la forme directe emprunte exactement la résolution de la forme aliasée.
#[test]
fn order_by_sur_agregat_direct_trie_comme_la_forme_aliasee() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    for (g, n) in [("A", 1), ("A", 2), ("B", 3), ("C", 4), ("C", 5), ("C", 6)] {
        s.execute(&format!("INSERT (:G {{g: '{g}', n: {n}}})"))
            .unwrap();
    }
    s.commit().unwrap();

    let par_alias = lignes(
        &db,
        "MATCH (x:G) RETURN x.g AS g, count(*) AS n ORDER BY n DESC",
    );
    let direct = lignes(
        &db,
        "MATCH (x:G) RETURN x.g AS g, count(*) AS n ORDER BY count(*) DESC",
    );
    assert_eq!(
        direct, par_alias,
        "ORDER BY count(*) et ORDER BY <alias de count(*)> désignent la même valeur \
         et doivent produire le même ordre"
    );
}

// ===========================================================================
// GROUPE I — PARITÉ ENTRE CHEMINS D'EXÉCUTION
//
// Le moteur possède plusieurs chemins pour le même agrégat : un raccourci qui
// lit un compteur de métadonnées sans toucher une ligne, l'agrégat scalaire,
// l'agrégat par hachage, et l'agrégat factorisé. Ces tests confrontent ces
// sources de vérité entre elles. Ils sont verts aujourd'hui : leur rôle est de
// le RESTER, en verrouillant les gardes qui tiennent les chemins divergents
// hors d'atteinte.
// ===========================================================================

/// Le raccourci `count` (qui rend `node_count_by_label()` sans balayer) doit
/// s'accorder avec un balayage réel APRÈS SUPPRESSION : un compteur de
/// métadonnées et un parcours de lignes sont deux sources de vérité, et les
/// pierres tombales sont l'endroit classique où elles se séparent.
#[test]
fn le_raccourci_count_s_accorde_avec_le_balayage_apres_suppression() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    for i in 0..5 {
        s.execute(&format!("INSERT (:Q {{i: {i}}})")).unwrap();
    }
    s.commit().unwrap();

    let sd = db.session();
    sd.execute("MATCH (n:Q) WHERE n.i < 2 DELETE n").unwrap();

    // Trois sources indépendantes du même nombre.
    let raccourci = scalaire(&db, "MATCH (n:Q) RETURN count(*)");
    let balayage = scalaire(&db, "MATCH (n:Q) RETURN count(n.i)");
    let collecte = match scalaire(&db, "MATCH (n:Q) RETURN collect(n.i)") {
        Value::List(l) => Value::Int64(l.len() as i64),
        autre => panic!("collect doit rendre une liste, obtenu {autre:?}"),
    };

    assert_eq!(
        raccourci,
        Value::Int64(3),
        "5 insérés moins 2 supprimés : le compteur ne doit pas inclure les pierres tombales"
    );
    assert_eq!(
        raccourci, balayage,
        "le raccourci de comptage doit s'accorder avec un balayage réel"
    );
    assert_eq!(
        raccourci, collecte,
        "le raccourci de comptage doit s'accorder avec la taille de collect()"
    );
}

/// Le raccourci `count` doit voir les écritures NON COMMITÉES de la
/// transaction courante, exactement comme le balayage : un compteur global qui
/// ignorerait le tampon transactionnel divergerait ici.
#[test]
fn le_raccourci_count_voit_les_ecritures_non_commitees_comme_le_balayage() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    for i in 0..3 {
        s.execute(&format!("INSERT (:R {{i: {i}}})")).unwrap();
    }
    s.commit().unwrap();

    let mut st = db.session();
    st.begin_transaction().unwrap();
    st.execute("INSERT (:R {i: 99})").unwrap();

    let raccourci = st.execute("MATCH (n:R) RETURN count(n)").unwrap().rows[0][0].clone();
    let balayage = st.execute("MATCH (n:R) RETURN count(n.i)").unwrap().rows[0][0].clone();
    let collecte = match st.execute("MATCH (n:R) RETURN collect(n.i)").unwrap().rows[0][0].clone() {
        Value::List(l) => Value::Int64(l.len() as i64),
        autre => panic!("collect doit rendre une liste, obtenu {autre:?}"),
    };

    assert_eq!(
        raccourci,
        Value::Int64(4),
        "la transaction courante doit voir sa propre écriture non commitée"
    );
    assert_eq!(
        raccourci, balayage,
        "raccourci et balayage doivent s'accorder"
    );
    assert_eq!(
        raccourci, collecte,
        "raccourci et collect doivent s'accorder"
    );
    st.commit().unwrap();
}

/// PARITÉ FACTORISÉ / PLAT, sur un motif À DEUX SAUTS.
///
/// ATTENTION AU PIÈGE QUI A FAILLI RENDRE CE TEST VIDE. Le chemin factorisé
/// n'est retenu qu'à conditions réunies (obrain-engine planner/lpg/aggregate.rs,
/// garde du `if` en tête de fichier) : exécution factorisée active, AUCUNE
/// clause de groupement, et surtout une chaîne d'expansion d'AU MOINS DEUX
/// SAUTS (`count_expand_chain(...).0 >= 2`). Une première rédaction de ce test
/// comparait `MATCH (w:W) RETURN sum(w.v)` — un simple balayage de nœuds, qui
/// n'emprunte JAMAIS le chemin factorisé. Les deux moitiés de la comparaison
/// exécutaient donc le même code plat, et la mutation délibérée du garde ne
/// faisait pas rougir le test : il ne mesurait rien. Le motif ci-dessous
/// traverse deux arêtes pour engager réellement le chemin factorisé.
///
/// CE TEST VERROUILLE UN GARDE AUTANT QU'UN RÉSULTAT. Le chemin factorisé
/// possède ses propres variantes de `sum`/`avg`/`min`/`max`
/// (obrain-core/src/execution/factorized_chunk.rs) qui divergent du chemin
/// scalaire dès qu'un NULL est présent : la somme d'une colonne nulle y vaut
/// `0.0` et non `Null`, la moyenne y divise par TOUTES les lignes et non par
/// les seules non nulles, et le minimum s'y écrase à `Null` dès qu'un null
/// traîne. Ces variantes sont aujourd'hui inatteignables parce que le garde
/// n'admet que les agrégats portant sur une VARIABLE — un identifiant de nœud,
/// jamais nul — et refuse ceux portant sur une PROPRIÉTÉ. La propriété `v`
/// ci-dessous est délibérément partiellement nulle : si quelqu'un élargit le
/// garde aux propriétés, les trois divergences deviennent atteignables et ce
/// test rougit.
#[test]
fn parite_des_agregats_entre_execution_factorisee_et_execution_plate() {
    let db_fact = ObrainDB::new_in_memory();
    let db_plat = ObrainDB::with_config(Config::default().without_factorized_execution()).unwrap();

    // Chaîne à deux sauts : A -> {B1, B2} -> {C1(v=1), C2(v=3), C3(sans v)}.
    // Les trois chemins de deux sauts atteignent donc les valeurs 1, 3 et NULL.
    for db in [&db_fact, &db_plat] {
        let mut s = db.session();
        s.begin_transaction().unwrap();
        s.execute("INSERT (:W {nom: 'A'})").unwrap();
        s.execute("INSERT (:W {nom: 'B1'})").unwrap();
        s.execute("INSERT (:W {nom: 'B2'})").unwrap();
        s.execute("INSERT (:W {nom: 'C1', v: 1})").unwrap();
        s.execute("INSERT (:W {nom: 'C3'})").unwrap();
        s.execute("INSERT (:W {nom: 'C2', v: 3})").unwrap();
        s.commit().unwrap();

        s.begin_transaction().unwrap();
        for (de, vers) in [
            ("A", "B1"),
            ("A", "B2"),
            ("B1", "C1"),
            ("B1", "C2"),
            ("B2", "C3"),
        ] {
            s.execute(&format!(
                "MATCH (x:W {{nom: '{de}'}}), (y:W {{nom: '{vers}'}}) INSERT (x)-[:L]->(y)"
            ))
            .unwrap();
        }
        s.commit().unwrap();
    }

    let motif = "MATCH (a:W {nom: 'A'})-[:L]->(b)-[:L]->(c)";

    // Garde-fou : le motif doit bien produire trois chemins de deux sauts.
    // Sans cela, tout ce qui suit comparerait deux ensembles vides.
    assert_eq!(
        scalaire(&db_fact, &format!("{motif} RETURN count(c)")),
        Value::Int64(3),
        "le motif à deux sauts doit produire exactement trois chemins — \
         sinon la parité ci-dessous ne compare rien"
    );

    for q in [
        format!("{motif} RETURN count(*)"),
        format!("{motif} RETURN count(c)"),
        format!("{motif} RETURN count(c.v)"),
        format!("{motif} RETURN sum(c.v)"),
        format!("{motif} RETURN avg(c.v)"),
        format!("{motif} RETURN min(c.v)"),
        format!("{motif} RETURN max(c.v)"),
        format!("{motif} RETURN sum(c.absente)"),
        format!("{motif} RETURN avg(c.absente)"),
        format!("{motif} RETURN min(c.absente)"),
    ] {
        assert_eq!(
            scalaire(&db_fact, &q),
            scalaire(&db_plat, &q),
            "l'exécution factorisée et l'exécution plate doivent s'accorder — {q}"
        );
    }

    // Et les valeurs sont celles du chemin scalaire de référence : une parité
    // entre deux chemins tous deux faux ne prouverait rien.
    assert_eq!(
        scalaire(&db_fact, &format!("{motif} RETURN sum(c.v)")),
        Value::Int64(4),
        "1 + 3, le chemin menant au nœud sans v étant écarté — et un ENTIER, \
         là où la variante factorisée rendrait un flottant"
    );
    assert_eq!(
        scalaire(&db_fact, &format!("{motif} RETURN avg(c.v)")),
        Value::Float64(2.0),
        "moyenne sur les DEUX valeurs non nulles (4/2), et non sur les trois chemins (4/3)"
    );
    assert_eq!(
        scalaire(&db_fact, &format!("{motif} RETURN min(c.v)")),
        Value::Int64(1),
        "min doit sauter le NULL et non s'écraser à NULL"
    );
    assert_eq!(
        scalaire(&db_fact, &format!("{motif} RETURN sum(c.absente)")),
        Value::Null,
        "somme d'une colonne entièrement nulle : NULL, et non 0.0"
    );
}
