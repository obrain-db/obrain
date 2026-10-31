//! Lot 4 — CONFORMITÉ GQL : LA NULLITÉ.
//!
//! `IS NULL` / `IS NOT NULL`, et la propagation du NULL à travers chaque
//! opérateur, chaque comparaison, chaque agrégat.
//!
//! # MÉTHODE — lire avant de modifier ce fichier
//!
//! Chaque assertion de ce fichier encode une INTENTION SÉMANTIQUE, jamais une
//! observation du moteur. Pour chaque cas on énonce d'abord ce que la
//! sémantique DOIT donner et pourquoi (ISO/IEC 39075 §21, logique ternaire
//! héritée de SQL), puis on vérifie.
//!
//! Ce n'est pas un scrupule de style. Ce dépôt en a déjà payé le prix :
//! `crates/obrain-substrate/tests/vector_top_k_exact.rs` s'est déclaré
//! « contrat de PARITÉ LEGACY » et a figé le défaut NULLS-FIRST en écartant
//! explicitement la lecture correcte du code (« vérifié EMPIRIQUEMENT [...]
//! contrairement à la lecture "NullsLast" de la revue »). Un test écrit en
//! observant le moteur rend le défaut intouchable. [Depuis la correction de
//! l'ordre des NULL, ce test-là a été réécrit : sa sortie était la bonne
//! (défaut DESC → NullsFirst), c'est sa justification « parité legacy » qui
//! était fausse.]
//!
//! Quand le moteur diverge de l'intention, le test porte `#[ignore]` avec un
//! message qui NOMME la divergence et son emplacement. Il reste rouge par
//! construction — jamais réécrit pour devenir vert. `cargo test -- --ignored`
//! les exécute : ce sont les tests à faire passer, pas à supprimer.
//!
//! Quand la sémantique est AMBIGUË (le standard ne tranche pas), le choix
//! retenu est écrit en toutes lettres avec sa raison. Un choix documenté est
//! réversible ; un choix implicite ne l'est pas.
//!
//! Run : cargo test -p obrain-engine --test gql_04_nullite
//!       cargo test -p obrain-engine --test gql_04_nullite -- --ignored

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

// ---------------------------------------------------------------------------
// Fabriques — l'état est construit par le SEUL code de production (INSERT GQL),
// jamais en écrivant dans les structures internes. Un état fabriqué à la main
// ne prouverait que le comportement de la fabrication.
// ---------------------------------------------------------------------------

/// Un unique nœud d'ancrage, pour évaluer une expression scalaire sur une ligne.
fn db_ancre() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let mut session = db.session();
    session.begin_transaction().unwrap();
    session.execute("INSERT (:Ancre {n: 1})").unwrap();
    session.commit().unwrap();
    db
}

/// Évalue une expression scalaire sur exactement une ligne et rend sa valeur.
fn scalaire(db: &ObrainDB, expr: &str) -> Value {
    let session = db.session();
    let requete = format!("MATCH (a:Ancre) RETURN {expr} AS v");
    let result = session
        .execute(&requete)
        .unwrap_or_else(|e| panic!("la requête `{requete}` doit s'exécuter, erreur : {e:?}"));
    assert_eq!(
        result.rows.len(),
        1,
        "l'ancre doit produire exactement une ligne pour `{requete}`"
    );
    result.rows[0][0].clone()
}

/// Trois items dont un SANS la propriété `rank` (propriété ABSENTE).
///
/// C'est la forme que prend un nœud « coquille » en production : la clé n'a
/// jamais été écrite.
fn db_items_rank_absent() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let mut session = db.session();
    session.begin_transaction().unwrap();
    session
        .execute("INSERT (:Item {name: 'A', rank: 1})")
        .unwrap();
    session.execute("INSERT (:Item {name: 'B'})").unwrap(); // rank JAMAIS écrit
    session
        .execute("INSERT (:Item {name: 'C', rank: 3})")
        .unwrap();
    session.commit().unwrap();
    db
}

/// Rend la colonne 0 de chaque ligne, dans l'ordre du résultat.
fn colonne0(db: &ObrainDB, requete: &str) -> Vec<Value> {
    let session = db.session();
    let result = session
        .execute(requete)
        .unwrap_or_else(|e| panic!("la requête `{requete}` doit s'exécuter, erreur : {e:?}"));
    result.rows.iter().map(|r| r[0].clone()).collect()
}

fn s(v: &str) -> Value {
    Value::String(v.into())
}

// ===========================================================================
// GROUPE 0 — PROPRIÉTÉ ABSENTE CONTRE PROPRIÉTÉ PRÉSENTE MAIS NULLE
//
// Question centrale du lot. La production contient les deux formes : des
// nœuds « coquilles » dont toutes les colonnes sont nulles, et des nœuds où
// seule une l'est. Ces deux états doivent être DISTINGUÉS ou EXPLICITEMENT
// CONFONDUS — mais pas par accident.
//
// LES DEUX ÉTATS EXISTENT RÉELLEMENT EN STOCKAGE. Ce n'est pas une vue de
// l'esprit : `PropertyMap::get` rend `None` pour une clé absente, tandis
// qu'un NULL écrit est encodé `PropertyValue::Null`, qui n'est PAS la pierre
// tombale (celle-ci est un DRAPEAU d'entrée, `ENTRY_FLAG_TOMBSTONE`). Le
// substrat sait donc distinguer « jamais écrit », « écrit à NULL » et
// « effacé ».
//
// CHOIX RETENU ICI, en deux temps :
//
//   (a) DANS L'ALGÈBRE DE VALEURS — comparaison, logique, agrégats, tri —
//       les deux états sont CONFONDUS et valent NULL. Raison : GQL, comme
//       SQL, n'a pas de notion de « propriété absente » dans son algèbre ;
//       les distinguer y introduirait une quatrième valeur logique non
//       prévue par ISO 39075 §21, et rendrait le résultat d'une requête
//       dépendant de l'histoire d'écriture d'un nœud.
//
//   (b) DANS L'INTROSPECTION — `keys(n)`, `properties(n)` — les deux états
//       sont LÉGITIMEMENT DISTINCTS, parce que ces fonctions décrivent la
//       FORME du nœud et non la valeur d'une propriété. Un nœud portant
//       `x = null` a bien la clé `x`.
//
// Ces deux règles sont cohérentes ENTRE ELLES à une condition : que `REMOVE`
// retire réellement la clé. Sinon (b) devient inobservable et (a) devient le
// seul régime, ce qui rend le stockage d'un NULL indistinguable d'un effacement.
// C'est exactement ce que vérifient les tests de fin de groupe.
// ===========================================================================

/// Peut-on seulement STOCKER un NULL explicite ? La réponse gouverne tous les
/// tests « absent contre nul » qui suivent, donc elle est établie ici plutôt
/// que supposée.
///
/// INTENTION : `INSERT (:T {p: null})` doit être accepté. Que la propriété
/// soit ensuite matérialisée ou effacée est un détail de stockage, mais la
/// LECTURE doit rendre NULL dans les deux cas.
#[test]
fn stocker_un_null_explicite_est_accepte_et_se_relit_null() {
    let db = ObrainDB::new_in_memory();
    let mut session = db.session();
    session.begin_transaction().unwrap();
    session
        .execute("INSERT (:Coquille {name: 'nul_explicite', rank: null})")
        .expect("INSERT avec une propriété littérale null doit être accepté");
    session.commit().unwrap();

    let vals = colonne0(&db, "MATCH (n:Coquille) RETURN n.rank");
    assert_eq!(vals.len(), 1, "un seul nœud coquille a été inséré");
    assert_eq!(
        vals[0],
        Value::Null,
        "une propriété écrite à null doit se relire NULL"
    );
}

/// INTENTION : lire une propriété JAMAIS ÉCRITE rend NULL, pas une erreur.
#[test]
fn lire_une_propriete_absente_rend_null() {
    let db = db_items_rank_absent();
    let vals = colonne0(&db, "MATCH (n:Item) WHERE n.name = 'B' RETURN n.rank");
    assert_eq!(vals.len(), 1, "un seul Item se nomme B");
    assert_eq!(
        vals[0],
        Value::Null,
        "une propriété jamais écrite doit se lire NULL"
    );
}

/// INTENTION : `IS NULL` est VRAI dans les deux états, `IS NOT NULL` FAUX dans
/// les deux. C'est le choix de confusion énoncé en tête de groupe.
#[test]
fn is_null_ne_distingue_pas_absent_de_nul_explicite() {
    let db = ObrainDB::new_in_memory();
    let mut session = db.session();
    session.begin_transaction().unwrap();
    session.execute("INSERT (:T {name: 'absent'})").unwrap();
    session
        .execute("INSERT (:T {name: 'nul', rank: null})")
        .unwrap();
    session
        .execute("INSERT (:T {name: 'plein', rank: 7})")
        .unwrap();
    session.commit().unwrap();

    let nuls = colonne0(
        &db,
        "MATCH (n:T) WHERE n.rank IS NULL RETURN n.name ORDER BY n.name ASC",
    );
    assert_eq!(
        nuls,
        vec![s("absent"), s("nul")],
        "IS NULL doit retenir AUTANT la propriété absente que la propriété écrite à null"
    );

    let non_nuls = colonne0(
        &db,
        "MATCH (n:T) WHERE n.rank IS NOT NULL RETURN n.name ORDER BY n.name ASC",
    );
    assert_eq!(
        non_nuls,
        vec![s("plein")],
        "IS NOT NULL ne doit retenir que la propriété réellement porteuse d'une valeur"
    );
}

/// LE TEST DÉCISIF DU LOT.
///
/// INTENTION : `true OR <quoi que ce soit>` vaut TRUE. En logique ternaire,
/// TRUE OR UNKNOWN = TRUE (ISO 39075 §21). Aucune ligne ne doit disparaître,
/// que `rank` soit absent, nul, ou porteur d'une valeur.
///
/// Ce test sépare les deux états s'ils empruntent des chemins d'évaluation
/// différents : une propriété ABSENTE remonte un `None` Rust, une propriété
/// écrite à NULL remonte `Some(Value::Null)`. Si le `?` du dispatch
/// court-circuite le premier avant d'atteindre la table de vérité, la ligne
/// « absent » est perdue et le OR ne vaut plus TRUE.
// DÉFAUT: le dispatch `Binary` de filter.rs fait `eval_expr(left)?` ; une
// propriété ABSENTE y remonte un `None` Rust (et non `Value::Null`), donc le
// `?` court-circuite TOUTE l'expression avant que la table de vérité ne soit
// consultée, et `ExpressionPredicate::evaluate` rend `false`. La ligne dont la
// propriété est absente est PERDUE. Cause : filter.rs:477-478 + :427-455.
// CORRIGÉ (vert par intégration E+F, revue vague 3 — le D7 du correctif E répare le court-circuit du `?`) — défaut d'origine : `true OR n.p = 1` perdait la ligne dont p est ABSENTE (filter.rs:477-478) ; voir atelier gql04-or-perd-la-ligne-propriete-absente
#[test]
fn or_avec_vrai_ne_perd_aucune_ligne_quel_que_soit_l_etat_de_la_propriete() {
    let db = ObrainDB::new_in_memory();
    let mut session = db.session();
    session.begin_transaction().unwrap();
    session.execute("INSERT (:T {name: 'absent'})").unwrap();
    session
        .execute("INSERT (:T {name: 'nul', rank: null})")
        .unwrap();
    session
        .execute("INSERT (:T {name: 'plein', rank: 7})")
        .unwrap();
    session.commit().unwrap();

    let temoin = colonne0(&db, "MATCH (n:T) RETURN n.name ORDER BY n.name ASC");
    assert_eq!(
        temoin,
        vec![s("absent"), s("nul"), s("plein")],
        "témoin : les trois nœuds existent bien avant tout filtrage"
    );

    let gardes = colonne0(
        &db,
        "MATCH (n:T) WHERE true OR n.rank = 1 RETURN n.name ORDER BY n.name ASC",
    );
    assert_eq!(
        gardes,
        vec![s("absent"), s("nul"), s("plein")],
        "TRUE OR <expr> vaut TRUE en logique ternaire : aucune ligne ne doit être écartée, \
         y compris celle dont la propriété est ABSENTE"
    );
}

/// Corollaire de la table de vérité : `false AND <expr>` vaut FALSE, donc
/// AUCUNE ligne ne survit — y compris celles dont la propriété est absente.
///
/// PORTÉE LIMITÉE, ÉTABLIE PAR MUTATION — à lire avant de s'y fier. Ce test
/// SURVIT à la mutation qui retire l'absorption de FALSE dans AND (la ligne
/// `(Some(false), _) | (_, Some(false)) => Bool(false)` supprimée de
/// filter.rs). Raison : au niveau d'un WHERE, FALSE et UNKNOWN produisent le
/// MÊME effet observable — la ligne est écartée dans les deux cas. Ce test ne
/// peut donc pas distinguer « FALSE absorbe » de « UNKNOWN se propage ».
///
/// C'est `faux_absorbe_le_unknown_dans_and` qui porte l'assertion réelle, en
/// observant la VALEUR rendue plutôt que l'effet de filtrage. Celui-ci ne
/// garde qu'un pouvoir résiduel : il rougirait si AND rendait TRUE. Il est
/// conservé à ce titre, pas comme preuve de l'absorption.
#[test]
fn and_avec_faux_ne_retient_aucune_ligne() {
    let db = ObrainDB::new_in_memory();
    let mut session = db.session();
    session.begin_transaction().unwrap();
    session.execute("INSERT (:T {name: 'absent'})").unwrap();
    session
        .execute("INSERT (:T {name: 'nul', rank: null})")
        .unwrap();
    session
        .execute("INSERT (:T {name: 'plein', rank: 7})")
        .unwrap();
    session.commit().unwrap();

    let gardes = colonne0(
        &db,
        "MATCH (n:T) WHERE false AND n.rank = 1 RETURN n.name ORDER BY n.name ASC",
    );
    assert!(
        gardes.is_empty(),
        "FALSE AND <expr> vaut FALSE : aucune ligne ne doit survivre, obtenu {gardes:?}"
    );
}

// ===========================================================================
// GROUPE 1 — COMPARAISON AVEC NULL
//
// ISO/IEC 39075 §21 : toute comparaison dont un opérande est NULL rend
// UNKNOWN, jamais TRUE ni FALSE. UNKNOWN se matérialise en NULL dans une
// clause RETURN.
// ===========================================================================

/// INTENTION : `NULL = NULL` rend UNKNOWN (NULL), PAS TRUE.
/// C'est le piège classique : l'égalité structurelle de deux NULL est vraie
/// en Rust (`Value::Null == Value::Null`), mais la comparaison SQL/GQL est
/// une comparaison de VALEURS INCONNUES, dont on ne peut affirmer l'égalité.
#[test]
fn null_egale_null_rend_unknown() {
    let db = db_ancre();
    assert_eq!(
        scalaire(&db, "null = null"),
        Value::Null,
        "NULL = NULL doit rendre UNKNOWN (NULL), jamais TRUE : on ne peut affirmer \
         l'égalité de deux valeurs inconnues"
    );
}

/// INTENTION : `NULL <> NULL` rend UNKNOWN, PAS FALSE.
#[test]
fn null_different_de_null_rend_unknown() {
    let db = db_ancre();
    assert_eq!(
        scalaire(&db, "null <> null"),
        Value::Null,
        "NULL <> NULL doit rendre UNKNOWN (NULL), jamais FALSE"
    );
}

/// INTENTION : une comparaison d'ordre avec NULL rend UNKNOWN.
#[test]
fn comparaisons_d_ordre_avec_null_rendent_unknown() {
    let db = db_ancre();
    for expr in ["null < 1", "null > 1", "null <= 1", "null >= 1", "1 < null"] {
        assert_eq!(
            scalaire(&db, expr),
            Value::Null,
            "`{expr}` doit rendre UNKNOWN (NULL) : aucun ordre n'est décidable \
             face à une valeur inconnue"
        );
    }
}

/// INTENTION : l'égalité avec NULL rend UNKNOWN même face à une valeur connue.
#[test]
fn egalite_d_un_null_et_d_une_valeur_connue_rend_unknown() {
    let db = db_ancre();
    assert_eq!(
        scalaire(&db, "null = 1"),
        Value::Null,
        "NULL = 1 doit rendre UNKNOWN (NULL)"
    );
    assert_eq!(
        scalaire(&db, "1 = null"),
        Value::Null,
        "1 = NULL doit rendre UNKNOWN (NULL) — la comparaison est symétrique"
    );
}

// ===========================================================================
// GROUPE 2 — LOGIQUE TERNAIRE
//
// Tables de vérité ISO/IEC 39075 §21. Les cas où UNKNOWN est ABSORBÉ par un
// opérande dominant sont les plus importants : ce sont eux qui distinguent
// une vraie logique ternaire d'une simple propagation du NULL.
// ===========================================================================

/// INTENTION : FALSE domine AND. `NULL AND false` = FALSE, pas NULL.
#[test]
fn faux_absorbe_le_unknown_dans_and() {
    let db = db_ancre();
    assert_eq!(
        scalaire(&db, "null AND false"),
        Value::Bool(false),
        "UNKNOWN AND FALSE doit valoir FALSE : FALSE est absorbant pour AND, \
         la valeur inconnue ne peut pas rendre la conjonction vraie"
    );
    assert_eq!(
        scalaire(&db, "false AND null"),
        Value::Bool(false),
        "FALSE AND UNKNOWN doit valoir FALSE (symétrie)"
    );
}

/// INTENTION : TRUE domine OR. `NULL OR true` = TRUE, pas NULL.
#[test]
fn vrai_absorbe_le_unknown_dans_or() {
    let db = db_ancre();
    assert_eq!(
        scalaire(&db, "null OR true"),
        Value::Bool(true),
        "UNKNOWN OR TRUE doit valoir TRUE : TRUE est absorbant pour OR"
    );
    assert_eq!(
        scalaire(&db, "true OR null"),
        Value::Bool(true),
        "TRUE OR UNKNOWN doit valoir TRUE (symétrie)"
    );
}

/// INTENTION : hors absorption, UNKNOWN se propage.
#[test]
fn unknown_se_propage_hors_absorption() {
    let db = db_ancre();
    assert_eq!(
        scalaire(&db, "null AND true"),
        Value::Null,
        "UNKNOWN AND TRUE doit valoir UNKNOWN : rien ne tranche"
    );
    assert_eq!(
        scalaire(&db, "null OR false"),
        Value::Null,
        "UNKNOWN OR FALSE doit valoir UNKNOWN : rien ne tranche"
    );
    assert_eq!(
        scalaire(&db, "null AND null"),
        Value::Null,
        "UNKNOWN AND UNKNOWN doit valoir UNKNOWN"
    );
    assert_eq!(
        scalaire(&db, "null OR null"),
        Value::Null,
        "UNKNOWN OR UNKNOWN doit valoir UNKNOWN"
    );
}

/// INTENTION : `NOT UNKNOWN` = UNKNOWN. La négation d'une valeur inconnue
/// reste inconnue — elle ne devient ni TRUE ni FALSE.
///
/// Il s'agit de l'OPÉRATEUR `NOT`, pas de la contrainte `NOT NULL`.
#[test]
fn not_de_unknown_reste_unknown() {
    let db = db_ancre();
    assert_eq!(
        scalaire(&db, "NOT null"),
        Value::Null,
        "NOT UNKNOWN doit valoir UNKNOWN, jamais TRUE ni FALSE"
    );
}

/// INTENTION : `IS NULL` et `IS NOT NULL` sont TOTAUX — ils rendent toujours
/// un booléen franc, jamais UNKNOWN. C'est leur raison d'être : ce sont les
/// seuls prédicats capables de trancher sur un NULL.
#[test]
fn is_null_et_is_not_null_sont_totaux() {
    let db = db_ancre();
    assert_eq!(
        scalaire(&db, "null IS NULL"),
        Value::Bool(true),
        "NULL IS NULL doit valoir TRUE franc"
    );
    assert_eq!(
        scalaire(&db, "null IS NOT NULL"),
        Value::Bool(false),
        "NULL IS NOT NULL doit valoir FALSE franc"
    );
    assert_eq!(
        scalaire(&db, "1 IS NULL"),
        Value::Bool(false),
        "1 IS NULL doit valoir FALSE franc"
    );
    assert_eq!(
        scalaire(&db, "1 IS NOT NULL"),
        Value::Bool(true),
        "1 IS NOT NULL doit valoir TRUE franc"
    );
}

/// INTENTION : `NOT` composé avec `IS NULL` reste total, et la double
/// négation revient au prédicat de départ.
#[test]
fn not_de_is_null_reste_total() {
    let db = db_ancre();
    assert_eq!(
        scalaire(&db, "NOT (null IS NULL)"),
        Value::Bool(false),
        "NOT (NULL IS NULL) doit valoir FALSE franc : IS NULL ne rend jamais UNKNOWN"
    );
    assert_eq!(
        scalaire(&db, "NOT (null IS NOT NULL)"),
        Value::Bool(true),
        "NOT (NULL IS NOT NULL) doit valoir TRUE franc"
    );
}

/// INTENTION : une comparaison UNKNOWN placée sous `NOT` reste UNKNOWN, et
/// n'est donc pas retenue par un WHERE. Le piège serait que `NOT` transforme
/// UNKNOWN en TRUE et fasse remonter des lignes qui ne satisfont rien.
#[test]
fn where_not_sur_comparaison_unknown_ne_retient_rien() {
    let db = ObrainDB::new_in_memory();
    let mut session = db.session();
    session.begin_transaction().unwrap();
    session.execute("INSERT (:T {name: 'absent'})").unwrap();
    session
        .execute("INSERT (:T {name: 'nul', rank: null})")
        .unwrap();
    session.commit().unwrap();

    let gardes = colonne0(
        &db,
        "MATCH (n:T) WHERE NOT (n.rank = 1) RETURN n.name ORDER BY n.name ASC",
    );
    assert!(
        gardes.is_empty(),
        "NOT UNKNOWN vaut UNKNOWN, donc le WHERE ne retient rien ; obtenu {gardes:?}"
    );
}

// ===========================================================================
// GROUPE 3 — ORDRE DES NULL
//
// Les quatre combinaisons ASC/DESC × NULLS FIRST/NULLS LAST, plus les deux
// défauts. La suite existante (gql_spec_compliance.rs:527-571) ne couvre que
// ASC — c'est-à-dire exactement la moitié qui fonctionne.
//
// SÉMANTIQUE : `NULLS FIRST` et `NULLS LAST` sont des instructions ABSOLUES
// de placement. Elles désignent une extrémité de la sortie, pas une position
// relative à la clé de tri. Elles ne doivent donc PAS être retournées par
// `DESC` : c'est tout l'intérêt de pouvoir les écrire indépendamment du sens
// du tri. C'est la lecture d'ISO/IEC 39075, celle de SQL:2003 §10.10, et
// celle de PostgreSQL.
// ===========================================================================

/// Témoin : le tri ASC place les valeurs connues dans le bon ordre.
/// Sans ce témoin, un test d'ordre des NULL qui échoue ne dirait pas si c'est
/// le placement des NULL ou le tri lui-même qui est en cause.
#[test]
fn temoin_le_tri_asc_ordonne_les_valeurs_connues() {
    let db = db_items_rank_absent();
    let noms = colonne0(
        &db,
        "MATCH (n:Item) WHERE n.rank IS NOT NULL RETURN n.name ORDER BY n.rank ASC",
    );
    assert_eq!(
        noms,
        vec![s("A"), s("C")],
        "témoin : rank 1 puis rank 3 sous ASC"
    );
}

/// Témoin : le tri DESC place les valeurs connues dans le bon ordre.
#[test]
fn temoin_le_tri_desc_ordonne_les_valeurs_connues() {
    let db = db_items_rank_absent();
    let noms = colonne0(
        &db,
        "MATCH (n:Item) WHERE n.rank IS NOT NULL RETURN n.name ORDER BY n.rank DESC",
    );
    assert_eq!(
        noms,
        vec![s("C"), s("A")],
        "témoin : rank 3 puis rank 1 sous DESC"
    );
}

/// 1/4 — ASC NULLS FIRST. Déjà couvert par gql_spec_compliance.rs:527 ;
/// répété ici pour que les quatre combinaisons se lisent ensemble.
#[test]
fn asc_nulls_first_place_le_null_en_tete() {
    let db = db_items_rank_absent();
    let noms = colonne0(
        &db,
        "MATCH (n:Item) RETURN n.name ORDER BY n.rank ASC NULLS FIRST",
    );
    assert_eq!(
        noms,
        vec![s("B"), s("A"), s("C")],
        "ASC NULLS FIRST : le NULL ouvre, puis 1 puis 3"
    );
}

/// 2/4 — ASC NULLS LAST.
#[test]
fn asc_nulls_last_place_le_null_en_queue() {
    let db = db_items_rank_absent();
    let noms = colonne0(
        &db,
        "MATCH (n:Item) RETURN n.name ORDER BY n.rank ASC NULLS LAST",
    );
    assert_eq!(
        noms,
        vec![s("A"), s("C"), s("B")],
        "ASC NULLS LAST : 1 puis 3, le NULL ferme"
    );
}

/// 3/4 — DESC NULLS LAST. LE CAS QUI A SAIGNÉ EN PRODUCTION.
///
/// INTENTION : `NULLS LAST` est une instruction absolue. Sous DESC, les
/// valeurs connues descendent (3 puis 1) et le NULL FERME la liste.
///
/// C'est la requête « mes éléments les plus récents » : `ORDER BY x DESC`
/// avec une garde. Si le NULL ouvre la liste, les premières lignes servies
/// sont VIDES — six claims entièrement vides ont ainsi occupé les six
/// premières lignes de chaque lecture, et l'entité a délibéré sur du vide.
// corrigé par ce commit (placement ABSOLU des NULL dans les comparateurs)
#[test]
fn desc_nulls_last_place_le_null_en_queue() {
    let db = db_items_rank_absent();
    let noms = colonne0(
        &db,
        "MATCH (n:Item) RETURN n.name ORDER BY n.rank DESC NULLS LAST",
    );
    assert_eq!(
        noms,
        vec![s("C"), s("A"), s("B")],
        "DESC NULLS LAST : 3 puis 1, le NULL FERME la liste. NULLS LAST désigne \
         une extrémité de la sortie, elle ne doit pas être retournée par DESC"
    );
}

/// 4/4 — DESC NULLS FIRST.
///
/// INTENTION : le NULL ouvre, puis les valeurs connues descendent.
// corrigé par ce commit (placement ABSOLU des NULL dans les comparateurs)
#[test]
fn desc_nulls_first_place_le_null_en_tete() {
    let db = db_items_rank_absent();
    let noms = colonne0(
        &db,
        "MATCH (n:Item) RETURN n.name ORDER BY n.rank DESC NULLS FIRST",
    );
    assert_eq!(
        noms,
        vec![s("B"), s("C"), s("A")],
        "DESC NULLS FIRST : le NULL ouvre, puis 3 puis 1"
    );
}

/// Le DÉFAUT, sans clause NULLS — RÉÉCRIT par le commit qui corrige l'ordre
/// des NULL, en invoquant la clause de réversibilité que la version
/// précédente de ce test avait elle-même posée : « si le produit veut la
/// convention PostgreSQL, il lui faut changer le défaut du planificateur ET
/// ce test, ensemble ». C'est fait, ensemble, ici.
///
/// CHOIX RETENU : la convention PostgreSQL/Neo4j — NULL est la plus GRANDE
/// valeur, donc défaut calculé PAR DIRECTION (ASC → queue, DESC → tête).
/// C'est le comportement que TOUTES les requêtes de production sans clause
/// NULLS observaient déjà (obtenu, avant, par la compensation entre le
/// `NullsLast` unique du planificateur et le `.reverse()` fautif du
/// comparateur) : le conserver, obtenu par le chemin juste, est la seule
/// correction qui ne bascule pas la production. La version précédente de ce
/// test exigeait « NULLS LAST dans les deux sens », c'est-à-dire un
/// CHANGEMENT du comportement par défaut en production — refusé.
///
/// Ce choix est celui que fige aussi gql_09_tri.rs (groupe B,
/// `defaut_desc_place_les_null_en_tete`) : les deux fichiers disent
/// désormais la même chose. Le remède au piège « classement par récence »
/// n'est pas le défaut, c'est la clause explicite `NULLS LAST` — qui,
/// depuis ce commit, fonctionne (tests 3/4 et 4/4 ci-dessus).
#[test]
fn defaut_sans_clause_nulls_suit_la_convention_null_plus_grand() {
    let db = db_items_rank_absent();

    let asc = colonne0(&db, "MATCH (n:Item) RETURN n.name ORDER BY n.rank ASC");
    assert_eq!(
        asc,
        vec![s("A"), s("C"), s("B")],
        "défaut ASC : NULL est la plus grande valeur, il ferme"
    );

    let desc = colonne0(&db, "MATCH (n:Item) RETURN n.name ORDER BY n.rank DESC");
    assert_eq!(
        desc,
        vec![s("B"), s("C"), s("A")],
        "défaut DESC : NULL est la plus grande valeur, il OUVRE — la convention \
         PostgreSQL/Neo4j, et le comportement de production préservé. Qui veut \
         les lignes vides en queue écrit NULLS LAST, qui fonctionne désormais"
    );
}

/// Le placement des NULL doit être STABLE quand une LIMIT borne le tri.
///
/// Le chemin borné (`bounded_top_k`) est un tas, distinct du tri complet.
/// Un défaut de placement des NULL y devient un défaut de SÉLECTION : les
/// lignes vides ne sont plus seulement mal placées, elles ÉVINCENT les
/// lignes pleines hors du résultat.
// corrigé par ce commit (placement ABSOLU des NULL — le tri borné sert le
// préfixe exact du tri complet, la fenêtre ne peut plus être volée)
#[test]
fn le_tri_borne_ne_laisse_pas_le_null_evincer_les_lignes_pleines() {
    let db = db_items_rank_absent();
    let noms = colonne0(
        &db,
        "MATCH (n:Item) RETURN n.name ORDER BY n.rank DESC NULLS LAST LIMIT 2",
    );
    assert_eq!(
        noms,
        vec![s("C"), s("A")],
        "sous DESC NULLS LAST LIMIT 2, les deux lignes servies doivent être les deux \
         lignes PLEINES (3 puis 1) — le NULL est en queue, donc hors de la fenêtre"
    );
}

/// Une propriété ABSENTE et une propriété écrite à NULL doivent occuper la
/// MÊME place sous ORDER BY : ce sont deux NULL, et le tri ne connaît que des
/// valeurs. Les distinguer ici produirait un ordre dépendant de l'histoire
/// d'écriture du nœud, donc irreproductible.
#[test]
fn absent_et_nul_explicite_partagent_la_meme_place_sous_order_by() {
    let db = ObrainDB::new_in_memory();
    let mut session = db.session();
    session.begin_transaction().unwrap();
    session
        .execute("INSERT (:T {name: 'plein', rank: 5})")
        .unwrap();
    session.execute("INSERT (:T {name: 'absent'})").unwrap();
    session
        .execute("INSERT (:T {name: 'nul', rank: null})")
        .unwrap();
    session.commit().unwrap();

    let noms = colonne0(
        &db,
        "MATCH (n:T) RETURN n.name ORDER BY n.rank ASC NULLS LAST, n.name ASC",
    );
    assert_eq!(
        noms,
        vec![s("plein"), s("absent"), s("nul")],
        "la ligne pleine d'abord, puis les DEUX formes de NULL groupées en queue et \
         départagées par le second critère : absent et nul explicite occupent la même \
         classe d'ordre"
    );
}

// ===========================================================================
// GROUPE 4 — AGRÉGATS
//
// Règle ISO/SQL : tout agrégat sauf `count(*)` IGNORE les entrées NULL. La
// conséquence la plus contre-intuitive, et la plus importante à figer, est
// que `sum` d'un ensemble VIDE rend NULL et non 0.
// ===========================================================================

/// Fabrique une colonne mêlant valeurs et NULL sous les DEUX formes.
fn db_mesures_mixtes() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let mut session = db.session();
    session.begin_transaction().unwrap();
    session.execute("INSERT (:M {name: 'a', v: 1})").unwrap();
    session.execute("INSERT (:M {name: 'b', v: 3})").unwrap();
    session.execute("INSERT (:M {name: 'c'})").unwrap(); // v ABSENT
    session.execute("INSERT (:M {name: 'd', v: null})").unwrap(); // v NUL EXPLICITE
    session.commit().unwrap();
    db
}

/// Fabrique une colonne ENTIÈREMENT nulle.
fn db_mesures_toutes_nulles() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let mut session = db.session();
    session.begin_transaction().unwrap();
    session.execute("INSERT (:Z {name: 'a'})").unwrap();
    session.execute("INSERT (:Z {name: 'b', v: null})").unwrap();
    session.commit().unwrap();
    db
}

/// INTENTION : `count(*)` compte les LIGNES, NULL compris.
#[test]
fn count_etoile_compte_les_lignes_null_comprises() {
    let db = db_mesures_mixtes();
    let vals = colonne0(&db, "MATCH (n:M) RETURN count(*)");
    assert_eq!(
        vals[0],
        Value::Int64(4),
        "count(*) compte les 4 lignes, quel que soit l'état de v"
    );
}

/// INTENTION : `count(expr)` compte les valeurs NON NULL, sous les deux formes.
#[test]
fn count_d_expression_ignore_les_null_des_deux_formes() {
    let db = db_mesures_mixtes();
    let vals = colonne0(&db, "MATCH (n:M) RETURN count(n.v)");
    assert_eq!(
        vals[0],
        Value::Int64(2),
        "count(n.v) ne compte que les 2 valeurs présentes : la propriété ABSENTE et la \
         propriété écrite à NULL sont l'une comme l'autre exclues"
    );
}

/// INTENTION : `count` d'une colonne entièrement nulle rend 0 — c'est le seul
/// agrégat qui rend une valeur franche plutôt que NULL sur un ensemble vide.
#[test]
fn count_d_une_colonne_entierement_nulle_rend_zero() {
    let db = db_mesures_toutes_nulles();
    let vals = colonne0(&db, "MATCH (n:Z) RETURN count(n.v)");
    assert_eq!(
        vals[0],
        Value::Int64(0),
        "count d'une colonne entièrement nulle rend 0, pas NULL : count est un \
         DÉNOMBREMENT, il vaut zéro quand il n'y a rien à dénombrer"
    );
}

/// INTENTION : `sum` ignore les NULL et somme le reste.
#[test]
fn sum_ignore_les_null() {
    let db = db_mesures_mixtes();
    let vals = colonne0(&db, "MATCH (n:M) RETURN sum(n.v)");
    assert_eq!(
        vals[0],
        Value::Int64(4),
        "sum(n.v) = 1 + 3 = 4 : les deux formes de NULL sont ignorées, pas traitées \
         comme des zéros"
    );
}

/// INTENTION : `sum` d'une colonne ENTIÈREMENT nulle rend NULL, PAS 0.
///
/// C'est la règle SQL la plus contre-intuitive et la plus utile : 0 est une
/// affirmation (« la somme vaut zéro »), NULL est un aveu (« il n'y avait
/// rien à sommer »). Confondre les deux fait passer un ensemble vide pour un
/// ensemble dont les éléments s'annulent.
#[test]
fn sum_d_une_colonne_entierement_nulle_rend_null() {
    let db = db_mesures_toutes_nulles();
    let vals = colonne0(&db, "MATCH (n:Z) RETURN sum(n.v)");
    assert_eq!(
        vals[0],
        Value::Null,
        "sum d'une colonne entièrement nulle doit rendre NULL et non 0 : il n'y a rien \
         à sommer, et 0 serait une affirmation fausse"
    );
}

/// INTENTION : `avg` ignore les NULL — le dénominateur est le nombre de
/// valeurs PRÉSENTES, pas le nombre de lignes.
#[test]
fn avg_ignore_les_null_au_denominateur() {
    let db = db_mesures_mixtes();
    let vals = colonne0(&db, "MATCH (n:M) RETURN avg(n.v)");
    let attendu = 2.0_f64; // (1 + 3) / 2, et non (1 + 3) / 4
    match &vals[0] {
        Value::Float64(f) => assert!(
            (f - attendu).abs() < 1e-9,
            "avg(n.v) doit valoir {attendu} — moyenne des 2 valeurs PRÉSENTES. \
             Obtenu {f}. Un dénominateur de 4 signifierait que les NULL comptent \
             comme des zéros"
        ),
        Value::Int64(i) => assert_eq!(
            *i as f64, attendu,
            "avg(n.v) doit valoir {attendu}, obtenu {i}"
        ),
        autre => panic!("avg doit rendre un nombre, obtenu {autre:?}"),
    }
}

/// INTENTION : `avg` d'une colonne entièrement nulle rend NULL — surtout pas
/// une division par zéro, un NaN, ou 0.
#[test]
fn avg_d_une_colonne_entierement_nulle_rend_null() {
    let db = db_mesures_toutes_nulles();
    let vals = colonne0(&db, "MATCH (n:Z) RETURN avg(n.v)");
    assert_eq!(
        vals[0],
        Value::Null,
        "avg d'une colonne entièrement nulle doit rendre NULL : le dénominateur est \
         zéro, la moyenne n'est pas définie. Ni 0, ni NaN, ni une erreur"
    );
}

/// INTENTION : `min` et `max` ignorent les NULL.
///
/// En particulier, le NULL ne doit PAS être vu comme la plus petite valeur
/// par `min`, ni comme la plus grande par `max` — ce serait faire du NULL une
/// valeur ordonnable, ce qu'il n'est pas.
#[test]
fn min_et_max_ignorent_les_null() {
    let db = db_mesures_mixtes();
    let mini = colonne0(&db, "MATCH (n:M) RETURN min(n.v)");
    assert_eq!(
        mini[0],
        Value::Int64(1),
        "min(n.v) = 1 : le NULL n'est pas une valeur plus petite que 1, il est ignoré"
    );
    let maxi = colonne0(&db, "MATCH (n:M) RETURN max(n.v)");
    assert_eq!(
        maxi[0],
        Value::Int64(3),
        "max(n.v) = 3 : le NULL n'est pas une valeur plus grande que 3, il est ignoré"
    );
}

/// INTENTION : `min`/`max` d'une colonne entièrement nulle rendent NULL.
#[test]
fn min_et_max_d_une_colonne_entierement_nulle_rendent_null() {
    let db = db_mesures_toutes_nulles();
    assert_eq!(
        colonne0(&db, "MATCH (n:Z) RETURN min(n.v)")[0],
        Value::Null,
        "min d'une colonne entièrement nulle doit rendre NULL"
    );
    assert_eq!(
        colonne0(&db, "MATCH (n:Z) RETURN max(n.v)")[0],
        Value::Null,
        "max d'une colonne entièrement nulle doit rendre NULL"
    );
}

// ===========================================================================
// GROUPE 5 — NULL DANS LES LISTES, `IN`, `DISTINCT`, ET COMME CLÉ DE GROUPE
// ===========================================================================

/// INTENTION : `x IN (liste contenant x)` vaut TRUE même si la liste contient
/// aussi un NULL — une correspondance CERTAINE l'emporte sur l'incertitude.
#[test]
fn in_trouve_la_valeur_malgre_un_null_dans_la_liste() {
    let db = db_ancre();
    assert_eq!(
        scalaire(&db, "1 IN [1, null, 3]"),
        Value::Bool(true),
        "1 IN [1, null, 3] doit valoir TRUE : la correspondance est certaine, \
         le NULL de la liste ne la rend pas douteuse"
    );
}

/// INTENTION : sans correspondance, un NULL dans la liste rend le résultat
/// UNKNOWN — la valeur cherchée POURRAIT être ce NULL.
#[test]
fn in_sans_correspondance_avec_un_null_rend_unknown() {
    let db = db_ancre();
    assert_eq!(
        scalaire(&db, "2 IN [1, null, 3]"),
        Value::Null,
        "2 IN [1, null, 3] doit valoir UNKNOWN : 2 n'a pas été trouvé, mais le NULL \
         de la liste pourrait être un 2. On ne peut pas conclure FALSE"
    );
}

/// INTENTION : sans NULL dans la liste et sans correspondance, le résultat
/// est un FALSE franc. Témoin qui isole l'effet du NULL du test précédent.
#[test]
fn temoin_in_sans_null_et_sans_correspondance_rend_faux() {
    let db = db_ancre();
    assert_eq!(
        scalaire(&db, "2 IN [1, 3]"),
        Value::Bool(false),
        "témoin : sans NULL dans la liste, l'absence de correspondance est un FALSE franc"
    );
}

/// INTENTION : `NULL IN <liste non vide>` vaut UNKNOWN.
///
/// On cherche une valeur inconnue : on ne peut affirmer ni qu'elle figure
/// dans la liste, ni qu'elle n'y figure pas.
#[test]
fn null_in_une_liste_rend_unknown() {
    let db = db_ancre();
    assert_eq!(
        scalaire(&db, "null IN [1, 2, 3]"),
        Value::Null,
        "NULL IN [1, 2, 3] doit valoir UNKNOWN : on ne peut pas décider de \
         l'appartenance d'une valeur inconnue"
    );
}

/// INTENTION : un NULL peut figurer dans une liste littérale sans la
/// dénaturer — la liste garde sa longueur et le NULL y est présent comme
/// élément.
#[test]
fn un_null_dans_une_liste_reste_un_element_de_la_liste() {
    let db = db_ancre();
    let v = scalaire(&db, "[1, null, 3]");
    match v {
        Value::List(items) => {
            assert_eq!(
                items.len(),
                3,
                "la liste doit garder ses 3 éléments : le NULL n'est pas élidé"
            );
            assert_eq!(
                items[1],
                Value::Null,
                "le deuxième élément est le NULL, à sa place"
            );
        }
        autre => panic!("une liste littérale doit rendre une List, obtenu {autre:?}"),
    }
}

/// INTENTION : `DISTINCT` regroupe TOUS les NULL en UNE seule ligne.
///
/// C'est le seul contexte où GQL/SQL traite deux NULL comme identiques : le
/// regroupement (`DISTINCT`, `GROUP BY`) utilise l'identité, pas l'égalité.
/// Deux NULL ne sont pas ÉGAUX (groupe 1) mais ils sont NON DISTINCTS.
/// Cette dissymétrie est voulue par le standard, et c'est elle qu'on fige.
#[test]
fn distinct_regroupe_tous_les_null_en_une_ligne() {
    let db = ObrainDB::new_in_memory();
    let mut session = db.session();
    session.begin_transaction().unwrap();
    session.execute("INSERT (:D {v: 1})").unwrap();
    session.execute("INSERT (:D {v: 1})").unwrap();
    session.execute("INSERT (:D {})").unwrap(); // v ABSENT
    session.execute("INSERT (:D {v: null})").unwrap(); // v NUL EXPLICITE
    session.commit().unwrap();

    let vals = colonne0(&db, "MATCH (n:D) RETURN DISTINCT n.v");
    assert_eq!(
        vals.len(),
        2,
        "DISTINCT doit rendre 2 lignes : la valeur 1, et UNE seule ligne NULL \
         regroupant la propriété absente ET la propriété écrite à NULL. Obtenu {vals:?}"
    );
    assert!(
        vals.contains(&Value::Null),
        "l'une des lignes distinctes doit être NULL, obtenu {vals:?}"
    );
    assert!(
        vals.contains(&Value::Int64(1)),
        "l'autre ligne distincte doit être 1, obtenu {vals:?}"
    );
}

/// INTENTION : NULL est une CLÉ DE GROUPE valide. Les lignes à clé nulle
/// forment un groupe, elles ne sont pas écartées.
///
/// L'enjeu est direct : si les clés nulles étaient écartées, un `GROUP BY`
/// sur une colonne partiellement nulle perdrait silencieusement des lignes,
/// et les totaux ne se recomposeraient plus.
#[test]
fn null_est_une_cle_de_groupe_valide() {
    let db = ObrainDB::new_in_memory();
    let mut session = db.session();
    session.begin_transaction().unwrap();
    session.execute("INSERT (:G {cle: 'x', v: 1})").unwrap();
    session.execute("INSERT (:G {cle: 'x', v: 2})").unwrap();
    session.execute("INSERT (:G {v: 10})").unwrap(); // cle ABSENTE
    session.execute("INSERT (:G {cle: null, v: 20})").unwrap(); // cle NULLE
    session.commit().unwrap();

    let session = db.session();
    let result = session
        .execute("MATCH (n:G) RETURN n.cle AS cle, count(*) AS c ORDER BY c ASC")
        .unwrap();

    assert_eq!(
        result.rows.len(),
        2,
        "deux groupes attendus : la clé 'x', et UN groupe pour toutes les clés nulles \
         (absente et nulle explicite confondues). Obtenu {} lignes",
        result.rows.len()
    );

    let total: i64 = result
        .rows
        .iter()
        .map(|r| match r[1] {
            Value::Int64(i) => i,
            _ => panic!("count(*) doit rendre un Int64, obtenu {:?}", r[1]),
        })
        .sum();
    assert_eq!(
        total, 4,
        "les groupes doivent recomposer les 4 lignes : aucune ligne à clé nulle n'a le \
         droit de disparaître du regroupement"
    );

    let groupe_nul = result
        .rows
        .iter()
        .find(|r| r[0] == Value::Null)
        .expect("un groupe de clé NULL doit exister");
    assert_eq!(
        groupe_nul[1],
        Value::Int64(2),
        "le groupe de clé NULL doit compter 2 lignes : la clé absente et la clé nulle \
         explicite tombent dans le MÊME groupe"
    );
}

/// INTENTION : un agrégat calculé PAR GROUPE ignore les NULL de la colonne
/// agrégée à l'intérieur de chaque groupe, indépendamment de la clé.
#[test]
fn l_agregat_par_groupe_ignore_les_null_de_la_colonne_agregee() {
    let db = ObrainDB::new_in_memory();
    let mut session = db.session();
    session.begin_transaction().unwrap();
    session.execute("INSERT (:H {cle: 'x', v: 5})").unwrap();
    session.execute("INSERT (:H {cle: 'x'})").unwrap(); // v ABSENT
    session.execute("INSERT (:H {cle: 'y', v: null})").unwrap(); // v NUL
    session.commit().unwrap();

    let session = db.session();
    let result = session
        .execute("MATCH (n:H) RETURN n.cle AS cle, sum(n.v) AS s, count(n.v) AS c ORDER BY cle ASC")
        .unwrap();
    assert_eq!(result.rows.len(), 2, "deux groupes : x et y");

    // groupe x
    assert_eq!(
        result.rows[0][1],
        Value::Int64(5),
        "groupe x : sum ignore le v absent, donc 5"
    );
    assert_eq!(
        result.rows[0][2],
        Value::Int64(1),
        "groupe x : count(n.v) ne compte que la valeur présente"
    );

    // groupe y — entièrement nul
    assert_eq!(
        result.rows[1][1],
        Value::Null,
        "groupe y : sum sur un groupe entièrement nul rend NULL, pas 0"
    );
    assert_eq!(
        result.rows[1][2],
        Value::Int64(0),
        "groupe y : count(n.v) rend 0 sur un groupe entièrement nul"
    );
}

// ===========================================================================
// GROUPE 6 — LE SEUL DISCRIMINANT VISIBLE : `keys()` / `properties()`
//
// Application de la règle (b) énoncée en tête du GROUPE 0. `IS NULL` confond
// les deux états (règle (a), et c'est voulu) ; l'introspection les sépare,
// parce qu'elle décrit la FORME du nœud et non la valeur d'une propriété.
//
// C'est ici que se joue la cohérence de l'ensemble : si `REMOVE` n'efface pas
// réellement la clé, alors « effacé » et « écrit à NULL » deviennent le même
// état, et le moteur perd la seule distinction qu'il savait encore faire.
// ===========================================================================

/// INTENTION : `keys(n)` liste les clés que le nœud PORTE. Un nœud qui porte
/// `x = null` en stockage porte la clé `x` ; un nœud sans `x` ne la porte pas.
///
/// C'est le pendant nécessaire du test `is_null_ne_distingue_pas_...` : les
/// deux états sont confondus dans l'algèbre de valeurs ET distinguables par
/// l'introspection. Sans ce test, la confusion de `IS NULL` pourrait passer
/// pour une perte d'information alors qu'elle est un choix.
///
/// [SEMIS AJUSTÉ par le correctif « remove réel » — la règle (b) est intacte.]
/// Ce test semait l'état « clé présente, valeur nulle » par
/// `INSERT (:K {x: null})`. Depuis le correctif, AUCUNE écriture du langage ne
/// matérialise une clé nulle : `INSERT (… x: null)`, `SET n.x = null` et
/// `REMOVE n.x` mènent tous à « clé absente » (gql_13, GROUPE 7 : la forme
/// d'un nœud ne mémorise pas la ROUTE d'écriture). L'état « clé nulle », lui,
/// existe toujours en stockage — `PropertyValue::Null` n'est pas la pierre
/// tombale — et seul le SEMIS DIRECT par l'API (`create_node_with_props`,
/// comme `une_cle_nulle_semee_est_portee_par_le_noeud` de gql_13) sait encore
/// le fabriquer. C'est donc lui qui sème ici : l'assertion — keys() distingue
/// les deux états de stockage — n'a pas bougé.
#[test]
fn keys_distingue_la_cle_nulle_de_la_cle_absente() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["K"], [("name", Value::String("absent".into()))]);
    db.create_node_with_props(
        &["K"],
        [("name", Value::String("nul".into())), ("x", Value::Null)],
    );

    let session = db.session();
    let result = session
        .execute("MATCH (n:K) RETURN n.name AS nom, keys(n) AS cles ORDER BY nom ASC")
        .unwrap();
    assert_eq!(result.rows.len(), 2, "deux nœuds K attendus");

    let cles = |ligne: &Vec<Value>| -> Vec<String> {
        match &ligne[1] {
            Value::List(items) => {
                let mut v: Vec<String> = items
                    .iter()
                    .map(|i| match i {
                        Value::String(s) => s.to_string(),
                        autre => panic!("une clé doit être une chaîne, obtenu {autre:?}"),
                    })
                    .collect();
                v.sort();
                v
            }
            autre => panic!("keys() doit rendre une liste, obtenu {autre:?}"),
        }
    };

    assert_eq!(
        cles(&result.rows[0]),
        vec!["name".to_string()],
        "le nœud dont la propriété x est ABSENTE ne doit PAS porter la clé x"
    );
    assert_eq!(
        cles(&result.rows[1]),
        vec!["name".to_string(), "x".to_string()],
        "le nœud dont la propriété x est écrite à NULL DOIT porter la clé x : \
         `keys()` décrit la forme du nœud, pas la valeur de ses propriétés"
    );
}

/// INTENTION : `REMOVE n.x` RETIRE la clé. Après un REMOVE, `keys(n)` ne doit
/// plus contenir `x` — le nœud revient à l'état « jamais écrit ».
///
/// C'est le sens même du mot. Un `REMOVE` qui écrit un NULL n'est pas un
/// REMOVE, c'est un `SET n.x = null` : il laisse une clé porteuse de NULL,
/// donc il rend « effacé » indistinguable de « écrit à NULL » et fait
/// s'effondrer la règle (b) du GROUPE 0.
///
/// La conséquence n'est pas cosmétique : la clé reste inscrite au registre
/// des clés de propriété, `keys()` et `properties()` continuent de l'exposer,
/// et le journal porte un `SetNodeProperty{value: Null}` au lieu d'un
/// `RemoveNodeProperty` — donc la rejouabilité du journal fige l'écart.
// corrigé par ce commit (remove réel : retrait en stockage + enregistrement WAL Remove*Property ; SET/INSERT à null ne matérialisent plus la clé)
#[test]
fn remove_retire_reellement_la_cle() {
    let db = ObrainDB::new_in_memory();
    let mut session = db.session();
    session.begin_transaction().unwrap();
    session
        .execute("INSERT (:R {name: 'cible', x: 1})")
        .unwrap();
    session.commit().unwrap();

    // Témoin : avant le REMOVE, la clé est bien là. Sans ce témoin, un REMOVE
    // qui échouerait silencieusement serait indistinguable d'un REMOVE correct.
    let avant = colonne0(&db, "MATCH (n:R) RETURN keys(n)");
    match &avant[0] {
        Value::List(items) => assert!(
            items.iter().any(|i| *i == s("x")),
            "témoin : la clé x doit exister AVANT le REMOVE, obtenu {items:?}"
        ),
        autre => panic!("keys() doit rendre une liste, obtenu {autre:?}"),
    }

    let mut session = db.session();
    session.begin_transaction().unwrap();
    session.execute("MATCH (n:R) REMOVE n.x").unwrap();
    session.commit().unwrap();

    let apres = colonne0(&db, "MATCH (n:R) RETURN keys(n)");
    match &apres[0] {
        Value::List(items) => assert!(
            !items.iter().any(|i| *i == s("x")),
            "APRÈS `REMOVE n.x`, la clé x ne doit plus figurer dans keys(n) : REMOVE \
             retire la clé, il ne la met pas à NULL. Obtenu {items:?}"
        ),
        autre => panic!("keys() doit rendre une liste, obtenu {autre:?}"),
    }
}

/// Corollaire mesurable du précédent, sans passer par `keys()` : après un
/// `REMOVE`, le nœud doit être INDISTINGUABLE d'un nœud qui n'a jamais porté
/// la propriété. On le vérifie par le comportement qui les sépare aujourd'hui
/// — la composition `OR` — plutôt que par l'introspection.
///
/// Si ce test et `remove_retire_reellement_la_cle` divergent, c'est que
/// l'effacement est partiel : visible ici, invisible là.
// corrigé par ce commit (remove réel : retrait en stockage + enregistrement WAL Remove*Property ; SET/INSERT à null ne matérialisent plus la clé)
#[test]
fn apres_remove_le_noeud_se_comporte_comme_un_noeud_sans_la_propriete() {
    let db = ObrainDB::new_in_memory();
    let mut session = db.session();
    session.begin_transaction().unwrap();
    session
        .execute("INSERT (:S {name: 'jamais_ecrite'})")
        .unwrap();
    session
        .execute("INSERT (:S {name: 'effacee', x: 1})")
        .unwrap();
    session.commit().unwrap();

    let mut session = db.session();
    session.begin_transaction().unwrap();
    session
        .execute("MATCH (n:S {name: 'effacee'}) REMOVE n.x")
        .unwrap();
    session.commit().unwrap();

    let noms = colonne0(
        &db,
        "MATCH (n:S) WHERE n.x IS NULL RETURN n.name ORDER BY n.name ASC",
    );
    assert_eq!(
        noms,
        vec![s("effacee"), s("jamais_ecrite")],
        "après REMOVE, les deux nœuds doivent être également NULL pour IS NULL"
    );

    let keys_egaux = {
        let session = db.session();
        let r = session
            .execute("MATCH (n:S) RETURN n.name AS nom, keys(n) AS cles ORDER BY nom ASC")
            .unwrap();
        let extrait = |l: &Vec<Value>| match &l[1] {
            Value::List(items) => {
                let mut v: Vec<String> = items
                    .iter()
                    .map(|i| format!("{i:?}"))
                    .filter(|k| k.contains('x'))
                    .collect();
                v.sort();
                v
            }
            autre => panic!("keys() doit rendre une liste, obtenu {autre:?}"),
        };
        (extrait(&r.rows[0]), extrait(&r.rows[1]))
    };
    assert_eq!(
        keys_egaux.0, keys_egaux.1,
        "le nœud dont x a été EFFACÉ et le nœud qui n'a JAMAIS porté x doivent avoir \
         la même forme : REMOVE doit ramener au même état, pas à un état intermédiaire"
    );
}
