//! Lot GQL-09 — conformité du TRI : `ORDER BY`, placement des NULL, clés
//! multiples, stabilité, tri borné, interaction avec `SKIP`/`LIMIT`.
//!
//! # Le principe qui gouverne ce fichier
//!
//! Chaque assertion encode l'INTENTION sémantique, jamais l'observation. Là
//! où le moteur diverge, le test échoue et le message dit ce que la sémantique
//! exigeait — c'est une trouvaille consignée, pas un comportement entériné.
//! Les tests qui échouent aujourd'hui portent `#[ignore]` avec la trouvaille
//! écrite en toutes lettres : ils sont exécutables à la demande
//! (`cargo test -- --ignored`) et deviendront verts le jour de la correction,
//! sans qu'une ligne d'assertion ne bouge.
//!
//! # Ce que ce fichier ne refait pas
//!
//! - `gql_order_by_limit.rs` : le tri précède la pagination (Sort(Limit) vs
//!   Limit(Sort)). Couvert, non redupliqué.
//! - `gql_limit_pushdown.rs` : `LIMIT` SANS tri. Aucun recouvrement.
//! - `gql_spec_compliance.rs` : deux tests `NULLS` (`test_order_by_nulls_first`
//!   et `..._last`), tous deux en **ASC** — c'est-à-dire exactement les deux
//!   combinaisons que le moteur traite correctement. Les deux combinaisons
//!   DESC, celles qui sont cassées, n'y sont pas. Ces deux tests sont écrits
//!   par intention (« Null rank should come first »), ils ne figent donc PAS
//!   le défaut ; mais ils font passer la fonctionnalité GA03 pour éprouvée
//!   alors que sa moitié fautive n'est pas touchée.
//!
//! # LE DÉFAUT DES NULL EST RÉPLIQUÉ SUR TROIS SITES, PAS UN
//!
//! [CORRIGÉ] La divergence gql09-desc-nulls-inverse décrite ci-dessous est
//! corrigée : placement ABSOLU des NULL dans les QUATRE comparateurs
//! (operators/sort.rs, push/sort.rs, spill/external_sort.rs et
//! parallel/merge.rs — un quatrième site que cet inventaire n'avait pas) et
//! défaut calculé PAR DIRECTION dans les planificateurs LPG et RDF
//! (ASC → NullsLast, DESC → NullsFirst). Le texte qui suit est conservé
//! comme mémoire du défaut et carte des sites.
//!
//! Les tests de ce fichier n'atteignent qu'un seul comparateur — celui du tri
//! scalaire en mémoire, `operators/sort.rs:249-252`. Le MÊME enchaînement
//! fautif (placement des NULL arbitré, PUIS `.reverse()` pour la direction, qui
//! retourne aussi le placement) est recopié à l'identique dans deux autres
//! implémentations :
//!
//! * `obrain-core/src/execution/operators/push/sort.rs:104-121` — l'exécuteur
//!   en mode « push » ;
//! * `obrain-core/src/execution/spill/external_sort.rs:359-377` — le tri
//!   externe, celui qui prend le relais quand le tri DÉBORDE sur disque.
//!
//! Conséquence à ne pas manquer pour qui corrigera : ne réparer que le site
//! scalaire ferait dépendre le placement des NULL du VOLUME de la requête —
//! juste tant que le tri tient en mémoire, faux dès qu'il déborde — et du
//! MODE D'EXÉCUTION choisi par le planificateur. Un même `ORDER BY … DESC
//! NULLS LAST` donnerait deux réponses selon la taille des données. C'est plus
//! difficile à diagnostiquer que la faute actuelle, qui a au moins le mérite
//! d'être uniforme. Les trois sites doivent bouger ensemble.
//!
//! Ces deux sites-là ne sont PAS couverts par ce fichier : les requêtes GQL
//! qu'il exécute passent toutes par le tri scalaire en mémoire.
//!
//! # AVERTISSEMENT — il existe DEUX ordres des NULL dans ce moteur
//!
//! Ce fichier pose la sémantique de référence des NULL pour le TRI SCALAIRE
//! (`SortOperator`, `obrain-core/src/execution/operators/sort.rs`), où le
//! défaut est « NULL est la plus GRANDE valeur » (groupe B).
//!
//! Un SECOND site tranche la même question dans l'autre sens :
//! `value_less_than` (`obrain-core/src/execution/factorized_chunk.rs:870-877`)
//! classe `Null` sous tout le reste, sans inversion de direction —
//! `(Value::Null, _) => true`. Il sert `min_deepest` / `max_deepest` du chemin
//! FACTORISÉ. Là, « NULL est la plus PETITE valeur ».
//!
//! Les deux conventions sont incompatibles et aucune ne cite l'autre. QUI
//! CORRIGERA L'UN DOIT REGARDER L'AUTRE : aligner le tri scalaire sans toucher
//! au chemin factorisé — ou l'inverse — laisserait le même moteur répondre
//! différemment à la même question selon le plan choisi, ce qui est plus
//! difficile à diagnostiquer que la faute actuelle. La parité
//! factorisé/scalaire n'est pas traitée ici : elle appartient au lot 07.
//!
//! # Anti-vacuité — registre des mutations
//!
//! Chaque test vert de ce fichier a été éprouvé en CASSANT le code de
//! production qu'il prétend couvrir, et en vérifiant qu'il rougit AVEC LE BON
//! MESSAGE. Onze mutations, chacune appliquée seule sur un arbre propre puis
//! révertée avec vérification. Les six tests `#[ignore]` n'ont pas besoin de
//! mutation : ils sont rouges contre le code réel, c'est leur raison d'être.
//!
//! | # | mutation | tests qui rougissent |
//! |---|----------|----------------------|
//! | M1 | `sort.rs:251` — retirer `.reverse()` du DESC | défaut DESC, les 2 multi-clés |
//! | M2 | `project.rs:848` — défaut → `NullsFirst` | les 3 tests de défaut |
//! | M3 | `project.rs:846` — clause `NULLS FIRST` perdue | `asc_nulls_first`, `desc_la_clause_n_est_pas_ignoree` |
//! | M4 | `bounded_top_k` — départage par ordinal inversé | les 4 `borne_*`, `justesse_*` |
//! | M5 | tri complet — `rows.reverse()` avant le tri stable | les 3 `stabilite_*`, `propriete_absente_*`, `tri_sur_propriete_jamais_declaree` |
//! | M6 | `compare_rows` — n'honorer que la 1re clé | les 2 multi-clés |
//! | M7 | `plan_skip` — décalage de 1 sur l'offset | `skip_et_limit_*`, `pagination_complete_*` |
//! | M8 | `compare_values_total` — abandon Int64↔Float64 | `types_numeriques_mixtes_*` |
//! | M9 | `sort.rs:250` — inverser la direction ASC | 13 tests, dont `asc_nulls_last`, `tri_sur_alias`, `limit_superieur` |
//! | M10 | `build_limit`/`build_skip` — bornes dégénérées écrasées | `limit_zero`, `skip_au_dela`, `pagination_complete_*` |
//! | M11 | la FIXTURE semée dans l'ordre déjà trié | `semis_est_bien_faux` — et lui SEUL |
//!
//! M11 mérite d'être lue : sous une fixture pré-triée,
//! `asc_nulls_last_place_les_null_en_queue` PASSE — il passerait même si le
//! tri ne faisait rien. Seul `semis_est_bien_faux` attrape la fixture. C'est
//! précisément le mode de panne d'un test de tri, et la raison d'être de ce
//! garde-fou.
//!
//! Deux enseignements tirés des mutations, tous deux corrigés dans ce fichier :
//! `borne_avec_null_reste_le_prefixe_du_complet` a d'abord SURVÉCU à M4 —
//! il comparait des valeurs, dans lesquelles deux NULL sont indiscernables,
//! alors que ces deux NULL étaient les seules lignes ex æquo de la fixture,
//! donc tout son pouvoir de détection. Il lit maintenant les identités.
//!
//! # Ordre de semis
//!
//! Toutes les fixtures sont insérées dans un ordre DÉLIBÉRÉMENT FAUX : jamais
//! l'ordre attendu en sortie, ni son inverse exact. Un tri inopérant (opérateur
//! neutralisé, clé ignorée) rend l'ordre d'insertion et fait donc rougir le
//! test. `semis_est_bien_faux` verrouille cette propriété du fichier lui-même.
#![cfg(feature = "gql")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use obrain_engine::database::QueryResult;

// ---------------------------------------------------------------------------
// Outillage
// ---------------------------------------------------------------------------

/// Construit une base en mémoire et y insère les nœuds `:T` décrits par
/// `(nom, valeur)`. `None` = propriété `v` ABSENTE du nœud — c'est ainsi que
/// naissent les NULL en production (nœuds hérités sans la clé), et non par un
/// littéral `null`.
///
/// Les nœuds sont insérés dans l'ordre du tableau : cet ordre est l'ordre de
/// stockage, donc l'ordre d'arrivée que le tri stable doit préserver.
fn base_t(lignes: &[(&str, Option<i64>)]) -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    for (nom, v) in lignes {
        let req = match v {
            Some(v) => format!("INSERT (:T {{nom: '{nom}', v: {v}}})"),
            None => format!("INSERT (:T {{nom: '{nom}'}})"),
        };
        s.execute(&req).unwrap();
    }
    s.commit().unwrap();
    db
}

fn lire(db: &ObrainDB, req: &str) -> QueryResult {
    db.session()
        .execute(req)
        .unwrap_or_else(|e| panic!("la requête doit s'exécuter : {req}\n  erreur : {e:?}"))
}

/// Colonne de chaînes, pour lire les identités de ligne.
fn noms(r: &QueryResult, col: usize) -> Vec<String> {
    r.rows
        .iter()
        .map(|l| match &l[col] {
            Value::String(s) => s.to_string(),
            autre => panic!("colonne {col} : String attendue, obtenu {autre:?}"),
        })
        .collect()
}

/// Colonne d'entiers nullables. `None` marque la place d'un NULL.
fn entiers(r: &QueryResult, col: usize) -> Vec<Option<i64>> {
    r.rows
        .iter()
        .map(|l| match &l[col] {
            Value::Int64(i) => Some(*i),
            Value::Null => None,
            autre => panic!("colonne {col} : Int64 ou Null attendu, obtenu {autre:?}"),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Garde-fou du fichier : le semis doit être faux
// ---------------------------------------------------------------------------

/// Anti-vacuité structurelle. Un test de tri passe par accident si la fixture
/// est déjà dans l'ordre attendu : le moteur n'a alors rien à faire pour avoir
/// raison. Ce test verrouille la prémisse de tous les autres — l'ordre
/// d'insertion N'EST PAS l'ordre trié — en la vérifiant sur le moteur lui-même,
/// via une lecture SANS `ORDER BY` (donc en ordre de stockage).
#[test]
fn semis_est_bien_faux() {
    let db = base_t(FIXTURE_NULLS);
    let brut = noms(&lire(&db, "MATCH (n:T) RETURN n.nom"), 0);
    assert_eq!(
        brut,
        vec!["b20", "nulA", "a10", "nulB", "c30"],
        "la lecture sans ORDER BY doit rendre l'ordre d'insertion ; si ce n'est \
         plus le cas, les tests de stabilité de ce fichier n'ont plus de \
         référence d'ordre d'arrivée"
    );
    assert_ne!(
        brut,
        vec!["a10", "b20", "c30", "nulA", "nulB"],
        "SEMIS INVALIDE : la fixture est déjà dans l'ordre croissant attendu, \
         les tests ASC passeraient sans que le tri fasse quoi que ce soit"
    );
    assert_ne!(
        brut,
        vec!["c30", "b20", "a10", "nulA", "nulB"],
        "SEMIS INVALIDE : la fixture est déjà dans l'ordre décroissant attendu"
    );
}

// ---------------------------------------------------------------------------
// GROUPE A — Placement des NULL : les quatre combinaisons explicites
// ---------------------------------------------------------------------------
//
// SÉMANTIQUE VISÉE, énoncée AVANT toute exécution.
//
// `NULLS FIRST` et `NULLS LAST` sont des placements ABSOLUS. La clause dit où
// vont les NULL dans le résultat FINAL — en tête ou en queue — et cette
// désignation ne dépend PAS de la direction de tri. C'est le sens que lui
// donnent SQL:2023 (§10.10 <null ordering>), ISO/IEC 39075 GQL (GA03, dont ce
// dépôt se réclame en commentaire) et toutes les implémentations de référence
// (PostgreSQL, Neo4j, DuckDB). « LAST » veut dire dernier ; il ne veut pas dire
// « dernier avant que la direction n'en décide autrement ».
//
// Corollaire, et c'est le point de contrôle : sous `DESC`, `NULLS LAST` doit
// TOUJOURS placer les NULL en queue. Si les inverser suffisait à obtenir
// l'autre comportement, la clause serait indiscernable de son absence.

/// Semis délibérément faux : valeurs 20, ABSENT, 10, ABSENT, 30.
/// Ni croissant, ni décroissant ; les NULL sont au MILIEU, pas déjà groupés en
/// tête ou en queue — un moteur qui ne les déplacerait pas du tout échouerait
/// aux quatre tests.
const FIXTURE_NULLS: &[(&str, Option<i64>)] = &[
    ("b20", Some(20)),
    ("nulA", None),
    ("a10", Some(10)),
    ("nulB", None),
    ("c30", Some(30)),
];

#[test]
fn asc_nulls_last_place_les_null_en_queue() {
    let db = base_t(FIXTURE_NULLS);
    let r = lire(&db, "MATCH (n:T) RETURN n.nom, n.v ORDER BY n.v ASC NULLS LAST");
    assert_eq!(
        entiers(&r, 1),
        vec![Some(10), Some(20), Some(30), None, None],
        "ASC NULLS LAST : les non-NULL croissants puis les NULL en queue"
    );
}

#[test]
fn asc_nulls_first_place_les_null_en_tete() {
    let db = base_t(FIXTURE_NULLS);
    let r = lire(&db, "MATCH (n:T) RETURN n.nom, n.v ORDER BY n.v ASC NULLS FIRST");
    assert_eq!(
        entiers(&r, 1),
        vec![None, None, Some(10), Some(20), Some(30)],
        "ASC NULLS FIRST : les NULL en tête puis les non-NULL croissants"
    );
}

/// TROUVAILLE — DIVERGENCE CONFIRMÉE.
///
/// `NULLS FIRST` sous `DESC` produit les NULL en QUEUE : la clause fait
/// exactement l'inverse de ce qu'elle annonce.
///
/// Mécanisme, lu dans le code : `compare_rows`
/// (obrain-core/src/execution/operators/sort.rs:247-252) appelle
/// `compare_values_with_nulls(.., key.null_order)` — qui a DÉJÀ arbitré la
/// place des NULL — puis applique `cmp.reverse()` pour la direction
/// descendante. Le `reverse` retourne le résultat entier, placement des NULL
/// compris. La direction et le placement des NULL ne sont pas séparables dans
/// ce comparateur alors que la sémantique les veut indépendants.
///
/// Correction attendue : `compare_values_with_nulls` doit rendre son verdict
/// APRÈS l'inversion de direction, ou le `reverse` ne doit s'appliquer qu'à la
/// comparaison des valeurs non-NULL.
///
/// [CORRIGÉ] C'est la seconde forme qui a été appliquée, sur les quatre
/// comparateurs du moteur.
#[test]
// corrigé par ce commit (placement ABSOLU des NULL dans les comparateurs)
fn desc_nulls_first_place_les_null_en_tete() {
    let db = base_t(FIXTURE_NULLS);
    let r = lire(&db, "MATCH (n:T) RETURN n.nom, n.v ORDER BY n.v DESC NULLS FIRST");
    assert_eq!(
        entiers(&r, 1),
        vec![None, None, Some(30), Some(20), Some(10)],
        "DESC NULLS FIRST : « FIRST » désigne la tête du résultat, quelle que \
         soit la direction. Les NULL doivent précéder les non-NULL décroissants"
    );
}

/// TROUVAILLE — DIVERGENCE CONFIRMÉE, et c'est celle qui a coûté en production.
///
/// `NULLS LAST` sous `DESC` produit les NULL en TÊTE. C'est la clause que l'on
/// écrit précisément pour s'en protéger, et elle aggrave le symptôme.
///
/// Impact documenté à l'atelier (fiche `orchestrateur-nulls-first`, gravité
/// critique) : six claims sans `declared_at` occupaient la tête de chaque
/// lecture `ORDER BY declared_at DESC LIMIT 8` — six des huit lignes injectées
/// dans le prompt étaient vides. Le contournement retenu (fiche
/// `corr-nulls-first-integre`) a été d'ajouter `WHERE x IS NOT NULL` sur huit
/// sites d'appel, PAS de corriger le moteur : la faute est toujours là, et
/// tout nouvel appelant qui écrira `NULLS LAST` — le réflexe juste — retombera
/// dedans.
#[test]
// corrigé par ce commit (placement ABSOLU des NULL dans les comparateurs)
fn desc_nulls_last_place_les_null_en_queue() {
    let db = base_t(FIXTURE_NULLS);
    let r = lire(&db, "MATCH (n:T) RETURN n.nom, n.v ORDER BY n.v DESC NULLS LAST");
    assert_eq!(
        entiers(&r, 1),
        vec![Some(30), Some(20), Some(10), None, None],
        "DESC NULLS LAST : « LAST » désigne la queue du résultat, quelle que \
         soit la direction. C'est la clause écrite pour empêcher les lignes \
         vides de remonter en tête d'un classement par récence — si elle les y \
         met, elle est pire qu'absente"
    );
}

/// La clause explicite doit CHANGER quelque chose. Test différentiel : sous
/// `DESC`, `NULLS FIRST` et `NULLS LAST` ne peuvent pas rendre le même
/// résultat, sinon la clause est ignorée.
///
/// Ce test tient indépendamment du sens conventionnel des mots, et reste donc
/// valide même pour qui contesterait la sémantique retenue plus haut. Il passe
/// aujourd'hui : le moteur honore bien la clause, il l'honore à l'envers.
#[test]
fn desc_la_clause_nulls_n_est_pas_ignoree() {
    let db = base_t(FIXTURE_NULLS);
    let f = entiers(&lire(&db, "MATCH (n:T) RETURN n.v ORDER BY n.v DESC NULLS FIRST"), 0);
    let l = entiers(&lire(&db, "MATCH (n:T) RETURN n.v ORDER BY n.v DESC NULLS LAST"), 0);
    assert_ne!(
        f, l,
        "DESC NULLS FIRST et DESC NULLS LAST rendent le MÊME ordre : la clause \
         est purement décorative, elle est acceptée par l'analyseur puis \
         perdue avant l'exécution"
    );
    let (tete_f, tete_l) = (f[0].is_none(), l[0].is_none());
    assert!(
        tete_f != tete_l,
        "les deux clauses doivent placer les NULL de part et d'autre du \
         résultat ; obtenu FIRST={f:?} LAST={l:?}"
    );
}

// ---------------------------------------------------------------------------
// GROUPE B — Placement des NULL par défaut, sans clause explicite
// ---------------------------------------------------------------------------
//
// SÉMANTIQUE VISÉE. ISO GQL laisse le défaut à l'implémentation. Le choix
// conventionnel, partagé par PostgreSQL et Neo4j, traite NULL comme la plus
// GRANDE des valeurs : NULL en queue sous ASC, NULL en tête sous DESC. C'est
// aussi le comportement actuel du moteur.
//
// CHOIX DOCUMENTÉ, DONC RÉVERSIBLE : ces deux tests figent la convention
// « NULL est le plus grand ». Quiconque veut en changer devra les modifier
// sciemment — c'est le but.
//
// ET LE CHOIX EST DÉJÀ CONTREDIT AILLEURS DANS LE MOTEUR : le chemin factorisé
// (`factorized_chunk.rs:870-877`, `value_less_than`) retient la convention
// INVERSE, « NULL est le plus petit ». Voir l'avertissement en tête de fichier.
// Ces deux tests-ci ne prétendent donc pas dire la vérité du moteur entier :
// ils disent celle du tri scalaire, et rendent l'écart visible.
//
// [FAIT — la correction décrite ci-dessous est exactement celle appliquée :
// placement absolu + défaut par direction ; ces deux tests sont restés verts
// sans qu'une assertion ne bouge, preuve que le défaut n'a pas changé.]
// POINT D'ATTENTION POUR QUI CORRIGERA LE GROUPE A. Le défaut n'est juste
// aujourd'hui que par compensation de la faute : le planificateur pose
// `NullOrder::NullsLast` pour les DEUX directions
// (planner/lpg/project.rs:848) et c'est le `reverse()` fautif qui, sous DESC,
// le retourne en NullsFirst — le résultat conventionnel obtenu par un chemin
// faux. Retirer le `reverse()` sur le placement SANS changer ce défaut ferait
// donc basculer le défaut DESC vers NULLS LAST et changerait le comportement
// de toutes les requêtes de production existantes. La correction juste est
// double : placement absolu, ET défaut calculé par direction
// (ASC → Last, DESC → First). Ces deux tests-ci sont le filet qui le prouvera.

#[test]
fn defaut_asc_place_les_null_en_queue() {
    let db = base_t(FIXTURE_NULLS);
    let r = lire(&db, "MATCH (n:T) RETURN n.v ORDER BY n.v ASC");
    assert_eq!(
        entiers(&r, 0),
        vec![Some(10), Some(20), Some(30), None, None],
        "défaut ASC : NULL traité comme la plus grande valeur, donc en queue"
    );
}

#[test]
fn defaut_desc_place_les_null_en_tete() {
    let db = base_t(FIXTURE_NULLS);
    let r = lire(&db, "MATCH (n:T) RETURN n.v ORDER BY n.v DESC");
    assert_eq!(
        entiers(&r, 0),
        vec![None, None, Some(30), Some(20), Some(10)],
        "défaut DESC : NULL traité comme la plus grande valeur, donc en tête. \
         C'est le piège de production — un classement par récence commence par \
         les lignes SANS date — mais c'est la convention, et le remède est la \
         clause explicite NULLS LAST, qui elle doit fonctionner (groupe A)"
    );
}

/// Sans clause, la direction seule ne doit pas être l'unique différence : le
/// défaut ASC et le défaut DESC placent les NULL à des bouts opposés. Vérifie
/// que le défaut n'est pas « NULL toujours en queue » ni « toujours en tête ».
#[test]
fn defaut_les_deux_directions_ne_placent_pas_les_null_du_meme_cote() {
    let db = base_t(FIXTURE_NULLS);
    let a = entiers(&lire(&db, "MATCH (n:T) RETURN n.v ORDER BY n.v ASC"), 0);
    let d = entiers(&lire(&db, "MATCH (n:T) RETURN n.v ORDER BY n.v DESC"), 0);
    assert!(
        a[0].is_some() && a[4].is_none(),
        "ASC par défaut : NULL en queue attendu, obtenu {a:?}"
    );
    assert!(
        d[0].is_none() && d[4].is_some(),
        "DESC par défaut : NULL en tête attendu, obtenu {d:?}"
    );
}

// ---------------------------------------------------------------------------
// GROUPE C — Clés multiples : chaque clé garde SA direction
// ---------------------------------------------------------------------------

/// `ORDER BY a ASC, b DESC, c ASC` — trois clés, trois directions à honorer
/// indépendamment. La seconde ne départage que les ex æquo de la première, la
/// troisième que les ex æquo des deux premières.
///
/// Fixture : (a, b, c) avec des égalités FORCÉES sur a et sur (a,b), pour que
/// les clés 2 et 3 aient réellement quelque chose à départager. Semée dans le
/// désordre.
///
/// Attendu, dérivé de la sémantique et non d'une exécution :
///   a=1,b=9,c=1 → a=1,b=9,c=2 → a=1,b=5,c=1 → a=2,b=7,c=1 → a=2,b=3,c=1
///   (a croissant ; à a égal, b DÉCROISSANT ; à (a,b) égaux, c croissant)
#[test]
fn multi_cles_chaque_cle_garde_sa_direction() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    // Ordre de semis délibérément faux : ni l'ordre attendu ni son inverse.
    for (nom, a, b, c) in [
        ("r4", 2, 3, 1),
        ("r1", 1, 9, 1),
        ("r5", 1, 5, 1),
        ("r2", 1, 9, 2),
        ("r3", 2, 7, 1),
    ] {
        s.execute(&format!(
            "INSERT (:M {{nom: '{nom}', a: {a}, b: {b}, c: {c}}})"
        ))
        .unwrap();
    }
    s.commit().unwrap();

    let r = lire(
        &db,
        "MATCH (n:M) RETURN n.nom, n.a, n.b, n.c ORDER BY n.a ASC, n.b DESC, n.c ASC",
    );
    assert_eq!(
        noms(&r, 0),
        vec!["r1", "r2", "r5", "r3", "r4"],
        "a croissant ; à a égal, b DÉCROISSANT (9 avant 5, 7 avant 3) ; à \
         (a,b) égaux, c croissant (r1 c=1 avant r2 c=2). Si r5 précède r1, la \
         seconde clé a été triée en ASC ; si r4 précède r3, idem"
    );
}

/// La direction de la PREMIÈRE clé ne doit pas contaminer les suivantes.
/// `ORDER BY a DESC, b ASC` : b reste croissant même sous une première clé
/// descendante.
#[test]
fn multi_cles_premiere_desc_seconde_asc_reste_asc() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    for (nom, a, b) in [("q2", 1, 2), ("q4", 2, 1), ("q1", 1, 1), ("q3", 2, 2)] {
        s.execute(&format!("INSERT (:M2 {{nom: '{nom}', a: {a}, b: {b}}})"))
            .unwrap();
    }
    s.commit().unwrap();

    let r = lire(&db, "MATCH (n:M2) RETURN n.nom ORDER BY n.a DESC, n.b ASC");
    assert_eq!(
        noms(&r, 0),
        vec!["q4", "q3", "q1", "q2"],
        "a décroissant (les a=2 d'abord) ; À L'INTÉRIEUR de chaque groupe, b \
         CROISSANT. Un résultat q3,q4,q2,q1 signifierait que la direction DESC \
         de la première clé a été appliquée à la seconde"
    );
}

// ---------------------------------------------------------------------------
// GROUPE D — Stabilité
// ---------------------------------------------------------------------------
//
// CE QUE L'ON ÉPROUVE ICI. GQL n'impose pas la stabilité : deux lignes ex æquo
// sur toutes les clés peuvent sortir dans n'importe quel ordre. Mais CE moteur
// l'affirme, et l'affirme comme une PREUVE, pas comme un espoir :
// `bounded_top_k` (sort.rs:264-285) documente « the earlier row wins, which is
// precisely what the stable sort would do », et le planificateur s'appuie sur
// cette affirmation pour descendre la limite dans le tri
// (project.rs:526-556). L'affirmation est donc porteuse : elle est ce qui rend
// l'optimisation légitime. Ces tests l'éprouvent.
//
// Référence d'ordre d'arrivée : l'ordre d'insertion, verrouillé par
// `semis_est_bien_faux`.

/// Cinq lignes, TOUTES ex æquo sur la clé de tri. Rien à trier — donc tout à
/// préserver. Semées dans un ordre qui n'est ni alphabétique ni son inverse,
/// pour qu'un tri secondaire clandestin sur le nom se voie.
#[test]
fn stabilite_toutes_ex_aequo_conserve_l_ordre_d_arrivee() {
    let db = base_t(&[
        ("e_charlie", Some(7)),
        ("e_alpha", Some(7)),
        ("e_echo", Some(7)),
        ("e_bravo", Some(7)),
        ("e_delta", Some(7)),
    ]);
    let r = lire(&db, "MATCH (n:T) RETURN n.nom ORDER BY n.v ASC");
    assert_eq!(
        noms(&r, 0),
        vec!["e_charlie", "e_alpha", "e_echo", "e_bravo", "e_delta"],
        "toutes les lignes sont ex æquo sur v : un tri stable les rend dans \
         l'ordre d'arrivée. Un résultat alphabétique trahirait un départage \
         clandestin ; un ordre quelconque trahirait un tri instable, et \
         invaliderait la preuve sur laquelle repose le tri borné"
    );
}

/// Ex æquo par GROUPES, avec du vrai travail de tri autour. Le tri doit
/// déplacer les groupes et ne rien réordonner à l'intérieur.
#[test]
fn stabilite_par_groupes_avec_tri_effectif() {
    let db = base_t(&[
        ("g3_second", Some(3)),
        ("g1_premier", Some(1)),
        ("g3_premier", Some(3)),
        ("g1_second", Some(1)),
        ("g2_seul", Some(2)),
    ]);
    let r = lire(&db, "MATCH (n:T) RETURN n.nom, n.v ORDER BY n.v ASC");
    assert_eq!(
        noms(&r, 0),
        vec![
            "g1_premier",
            "g1_second",
            "g2_seul",
            "g3_second",
            "g3_premier"
        ],
        "les groupes sont ordonnés 1,2,3 ; DANS le groupe 3, g3_second précède \
         g3_premier car il est arrivé le premier (nommage volontairement \
         trompeur : le nom ne doit avoir aucune influence)"
    );
}

/// La stabilité doit tenir aussi entre NULL ex æquo entre eux.
#[test]
fn stabilite_entre_null_ex_aequo() {
    let db = base_t(&[
        ("n_troisieme", None),
        ("n_premier", None),
        ("v_dix", Some(10)),
        ("n_second", None),
    ]);
    let r = lire(&db, "MATCH (n:T) RETURN n.nom ORDER BY n.v ASC NULLS LAST");
    assert_eq!(
        noms(&r, 0),
        vec!["v_dix", "n_troisieme", "n_premier", "n_second"],
        "les trois NULL sont ex æquo entre eux : ils sortent groupés en queue, \
         dans leur ordre d'arrivée"
    );
}

// ---------------------------------------------------------------------------
// GROUPE E — Tri borné : `ORDER BY … LIMIT k`
// ---------------------------------------------------------------------------
//
// LE CONTRAT, tel que le code l'énonce lui-même (sort.rs:88-91) :
// « The output is identical, row for row, to sorting everything and keeping
// the first k. That is not a hope, it is a consequence. »
//
// C'est un contrat DIFFÉRENTIEL, donc éprouvable sans rien supposer de la
// sémantique : le borné doit être le préfixe exact du complet. La frontière
// est le seul endroit où un top-k peut se tromper — c'est là que l'on frappe.

/// Le cas dur : PLUS de `k` lignes ex æquo autour du rang `k`. Quatre lignes
/// portent la valeur 2 ; avec k=3, le top-3 est « la ligne v=1, puis DEUX des
/// quatre v=2 » — et le contrat impose que ce soient les deux ARRIVÉES LES
/// PREMIÈRES, pas deux quelconques. C'est exactement le choix qu'un tas non
/// départagé ferait arbitrairement.
#[test]
fn borne_frontiere_plus_de_k_ex_aequo() {
    let db = base_t(&[
        ("deux_A", Some(2)),
        ("trois", Some(3)),
        ("deux_B", Some(2)),
        ("un", Some(1)),
        ("deux_C", Some(2)),
        ("deux_D", Some(2)),
    ]);
    let r = lire(&db, "MATCH (n:T) RETURN n.nom, n.v ORDER BY n.v ASC LIMIT 3");
    assert_eq!(
        noms(&r, 0),
        vec!["un", "deux_A", "deux_B"],
        "quatre lignes ex æquo à la frontière du rang 3 : le top-3 doit \
         retenir les DEUX PREMIÈRES ARRIVÉES parmi elles (deux_A puis deux_B), \
         car c'est ce que rendrait le tri complet suivi de LIMIT 3. Tout autre \
         couple, ou tout autre ordre, contredit la preuve de stabilité portée \
         par bounded_top_k"
    );
}

/// Le contrat différentiel, balayé sur TOUS les k. Pour chaque k de 0 à n+2,
/// le résultat borné doit être le préfixe de longueur k du résultat complet.
/// Cette forme est immunisée contre toute contestation sémantique : elle
/// compare le moteur à lui-même, et c'est précisément l'affirmation que le
/// planificateur invoque pour descendre la limite.
#[test]
fn borne_est_le_prefixe_exact_du_complet_pour_tout_k() {
    let db = base_t(&[
        ("deux_A", Some(2)),
        ("trois", Some(3)),
        ("deux_B", Some(2)),
        ("un", Some(1)),
        ("deux_C", Some(2)),
        ("deux_D", Some(2)),
    ]);
    let complet = noms(&lire(&db, "MATCH (n:T) RETURN n.nom ORDER BY n.v ASC"), 0);
    assert_eq!(complet.len(), 6, "le tri complet doit rendre les six lignes");

    for k in 0..=8usize {
        let borne = noms(
            &lire(&db, &format!("MATCH (n:T) RETURN n.nom ORDER BY n.v ASC LIMIT {k}")),
            0,
        );
        let attendu: Vec<String> = complet.iter().take(k).cloned().collect();
        assert_eq!(
            borne, attendu,
            "k={k} : ORDER BY … LIMIT {k} doit rendre EXACTEMENT le préfixe de \
             longueur {k} du tri complet, ex æquo compris. Complet = {complet:?}"
        );
    }
}

/// Même contrat en DESC, où le comparateur passe par la branche `reverse()`.
#[test]
fn borne_est_le_prefixe_exact_du_complet_en_desc() {
    let db = base_t(&[
        ("cinq_A", Some(5)),
        ("neuf", Some(9)),
        ("cinq_B", Some(5)),
        ("un", Some(1)),
        ("cinq_C", Some(5)),
    ]);
    let complet = noms(&lire(&db, "MATCH (n:T) RETURN n.nom ORDER BY n.v DESC"), 0);
    for k in 0..=6usize {
        let borne = noms(
            &lire(&db, &format!("MATCH (n:T) RETURN n.nom ORDER BY n.v DESC LIMIT {k}")),
            0,
        );
        assert_eq!(
            borne,
            complet.iter().take(k).cloned().collect::<Vec<_>>(),
            "k={k} en DESC : le borné doit être le préfixe exact du complet. \
             Complet = {complet:?}"
        );
    }
}

/// Le contrat différentiel doit tenir aussi quand des NULL sont présents : la
/// descente de la limite ne doit pas changer QUELLES lignes sortent, même
/// lorsque le placement des NULL est celui — fautif — du groupe A. La justesse
/// éprouvée ici est RELATIVE : elle dit que l'optimisation ne dégrade rien,
/// pas que le placement est bon.
///
/// ON COMPARE LES NOMS, PAS LES VALEURS, et c'est la mutation M4 (départage par
/// ordinal d'arrivée inversé dans `bounded_top_k`) qui l'a imposé : dans une
/// colonne de valeurs, deux NULL sont INDISCERNABLES. Une première version
/// comparait la colonne `n.v` et survivait à M4 sans broncher — elle ne pouvait
/// pas voir que le borné avait retenu l'autre NULL. La fixture a précisément
/// deux lignes ex æquo sur la clé (`nulA` et `nulB`), et ce sont elles qui
/// portent tout le pouvoir de détection : il faut donc les lire par leur
/// identité.
#[test]
fn borne_avec_null_reste_le_prefixe_du_complet() {
    let db = base_t(FIXTURE_NULLS);
    for clause in [
        "ASC NULLS FIRST",
        "ASC NULLS LAST",
        "DESC NULLS FIRST",
        "DESC NULLS LAST",
        "ASC",
        "DESC",
    ] {
        let complet = noms(
            &lire(&db, &format!("MATCH (n:T) RETURN n.nom ORDER BY n.v {clause}")),
            0,
        );
        for k in 0..=6usize {
            let borne = noms(
                &lire(
                    &db,
                    &format!("MATCH (n:T) RETURN n.nom ORDER BY n.v {clause} LIMIT {k}"),
                ),
                0,
            );
            assert_eq!(
                borne,
                complet.iter().take(k).cloned().collect::<Vec<_>>(),
                "clause « {clause} », k={k} : la descente de la limite dans le \
                 tri doit être neutre sur la sortie, ex æquo compris — les deux \
                 lignes sans valeur (nulA, nulB) sont ex æquo et doivent sortir \
                 dans leur ordre d'arrivée. Complet = {complet:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// GROUPE F — `LIMIT` dégénérés et `SKIP` combiné
// ---------------------------------------------------------------------------

#[test]
fn limit_zero_sous_order_by_rend_zero_ligne() {
    let db = base_t(FIXTURE_NULLS);
    let r = lire(&db, "MATCH (n:T) RETURN n.nom ORDER BY n.v ASC LIMIT 0");
    assert!(
        r.rows.is_empty(),
        "LIMIT 0 doit rendre zéro ligne, obtenu {:?}",
        noms(&r, 0)
    );
}

#[test]
fn limit_superieur_au_total_rend_tout_dans_l_ordre() {
    let db = base_t(FIXTURE_NULLS);
    let r = lire(&db, "MATCH (n:T) RETURN n.v ORDER BY n.v ASC NULLS LAST LIMIT 999");
    assert_eq!(
        entiers(&r, 0),
        vec![Some(10), Some(20), Some(30), None, None],
        "une limite supérieure au cardinal ne tronque rien et ne change pas \
         l'ordre : le résultat doit être identique au tri non borné"
    );
}

/// `SKIP` combiné au tri. Le planificateur EXCLUT délibérément la forme
/// `Limit(Skip(Sort))` de la descente de limite (project.rs:541-544 : « un
/// Skip intercalé ferait de la limite utile s + k, pas k »). Ce test éprouve
/// que l'exclusion tient : la page demandée doit être la fenêtre [s, s+k) du
/// tri complet, et non les k premiers d'un top-k mal dimensionné.
///
/// Si la limite était descendue à k=2 sans tenir compte du SKIP 2, le tri ne
/// produirait que 2 lignes, le SKIP les consommerait toutes, et le résultat
/// serait VIDE — c'est le mode de panne exact que ce test attrape.
#[test]
fn skip_et_limit_rendent_la_fenetre_du_tri_complet() {
    let db = base_t(&[
        ("s50", Some(50)),
        ("s20", Some(20)),
        ("s40", Some(40)),
        ("s10", Some(10)),
        ("s30", Some(30)),
    ]);
    let r = lire(
        &db,
        "MATCH (n:T) RETURN n.nom, n.v ORDER BY n.v ASC SKIP 2 LIMIT 2",
    );
    assert_eq!(
        noms(&r, 0),
        vec!["s30", "s40"],
        "ORDER BY ASC SKIP 2 LIMIT 2 sur (10,20,30,40,50) doit rendre la \
         fenêtre [2,4) du tri COMPLET, soit 30 et 40. Un résultat vide \
         signifierait que la limite a été descendue dans le tri sans y ajouter \
         le SKIP ; un résultat (10,20) que le SKIP a été perdu"
    );
}

/// `SKIP` au-delà du cardinal : zéro ligne, pas d'erreur, pas de repli sur la
/// première page.
#[test]
fn skip_au_dela_du_total_rend_zero_ligne() {
    let db = base_t(FIXTURE_NULLS);
    let r = lire(&db, "MATCH (n:T) RETURN n.nom ORDER BY n.v ASC SKIP 99 LIMIT 3");
    assert!(
        r.rows.is_empty(),
        "SKIP au-delà du cardinal doit rendre zéro ligne, obtenu {:?}",
        noms(&r, 0)
    );
}

/// Pagination exhaustive : la concaténation de toutes les pages de taille 2
/// doit reconstituer EXACTEMENT le tri complet, sans trou ni doublon. C'est le
/// test qui attrape un décalage de fenêtre d'une ligne, qu'aucune page prise
/// isolément ne révèle.
#[test]
fn pagination_complete_reconstitue_le_tri_complet() {
    let db = base_t(&[
        ("p50", Some(50)),
        ("p20", Some(20)),
        ("p40", Some(40)),
        ("p10", Some(10)),
        ("p30", Some(30)),
    ]);
    let complet = noms(&lire(&db, "MATCH (n:T) RETURN n.nom ORDER BY n.v ASC"), 0);
    let mut recolle = Vec::new();
    for s in [0usize, 2, 4, 6] {
        recolle.extend(noms(
            &lire(
                &db,
                &format!("MATCH (n:T) RETURN n.nom ORDER BY n.v ASC SKIP {s} LIMIT 2"),
            ),
            0,
        ));
    }
    assert_eq!(
        recolle, complet,
        "les pages successives (SKIP 0,2,4,6 LIMIT 2) doivent se recoller en \
         le tri complet, sans trou ni doublon"
    );
}

// ---------------------------------------------------------------------------
// GROUPE G — Tri sur EXPRESSION, pas sur propriété nue
// ---------------------------------------------------------------------------

/// TROUVAILLE — DIVERGENCE : le tri sur EXPRESSION n'existe pas.
///
/// `ORDER BY 0 - n.v` n'est pas mal trié : il est REFUSÉ, et refusé par une
/// erreur INTERNE — `Internal("Unsupported ORDER BY expression: Binary { .. }")`.
///
/// Mécanisme : `resolve_sort_expression_with_properties`
/// (planner/lpg/project.rs:884-918) ne SAIT PAS évaluer une expression. Elle ne
/// fait que résoudre la clé de tri vers une colonne DÉJÀ PROJETÉE, et n'accepte
/// donc que trois formes : une variable nue, une propriété nue
/// (`variable_propriete`), et un appel de fonction dont la colonne a été
/// auto-projetée. Tout le reste tombe dans le bras `_ =>` et lève une erreur
/// interne. ISO/IEC 39075 admet une <value expression> quelconque en clé de tri.
///
/// DEUX DÉFAUTS DISTINCTS, à ne pas confondre :
///   1. la capacité manque ;
///   2. son absence se signale par `Error::Internal`, et non par un GQLSTATUS
///      d'inconformité. Une fonctionnalité non implémentée se présente au
///      client comme un dysfonctionnement du moteur.
///
/// CONTOURNEMENT EXISTANT, et il éclaire le coût du correctif : la même
/// expression passée par un alias de projection fonctionne parfaitement — voir
/// `tri_sur_alias_de_projection`, qui est VERT. La couche de projection sait
/// donc calculer l'expression ; seule la résolution de la clé de tri ne sait
/// pas la lui demander. Le correctif est de projeter l'expression de tri comme
/// colonne supplémentaire — mécanisme qui EXISTE DÉJÀ dans le même fichier
/// (`sort_extra_count`, project.rs:867-878, qui injecte puis retire les
/// colonnes de clé de tri) — et non d'écrire un évaluateur.
///
/// CORRIGÉ (fix/gql-divers) : exactement par le mécanisme prescrit — la clé
/// expression est projetée comme item supplémentaire du Return augmenté de
/// `plan_sort` puis retirée par le strip `sort_extra_count`.
#[test]
fn tri_sur_expression_arithmetique() {
    let db = base_t(&[
        ("x2", Some(2)),
        ("x5", Some(5)),
        ("x1", Some(1)),
        ("x4", Some(4)),
    ]);
    let r = lire(&db, "MATCH (n:T) RETURN n.nom, n.v ORDER BY 0 - n.v ASC");
    assert_eq!(
        noms(&r, 0),
        vec!["x5", "x4", "x2", "x1"],
        "ORDER BY (0 - n.v) ASC doit ordonner par v DÉCROISSANT. Le résultat \
         x1,x2,x4,x5 signifierait que l'expression a été réduite à la \
         propriété nue et l'opération perdue"
    );
}

/// Tri sur un alias de la clause `RETURN`, l'expression étant calculée dans la
/// projection. L'alias doit être visible de `ORDER BY`.
#[test]
fn tri_sur_alias_de_projection() {
    let db = base_t(&[
        ("y2", Some(2)),
        ("y5", Some(5)),
        ("y1", Some(1)),
        ("y4", Some(4)),
    ]);
    let r = lire(
        &db,
        "MATCH (n:T) RETURN n.nom AS nom, 0 - n.v AS oppose ORDER BY oppose ASC",
    );
    assert_eq!(
        noms(&r, 0),
        vec!["y5", "y4", "y2", "y1"],
        "l'alias `oppose` doit être une clé de tri valide et porter la valeur \
         calculée, pas la propriété d'origine"
    );
}

/// Tri sur une expression dont la valeur est NULL pour certaines lignes : le
/// placement des NULL doit obéir à la même règle que sur une propriété nue.
/// Une expression arithmétique sur une propriété absente rend NULL.
///
/// Bloqué par la MÊME divergence que `tri_sur_expression_arithmetique` : la
/// requête est refusée avant même d'être exécutée. Ce test est conservé parce
/// qu'il éprouve une chose que l'autre n'éprouve pas — l'INTERACTION entre le
/// tri sur expression et le placement des NULL — et qui devra être vérifiée le
/// jour où la capacité arrivera : une expression rendant NULL doit être traitée
/// comme une propriété absente, pas comme une valeur particulière.
///
/// CORRIGÉ (fix/gql-divers) : même mécanisme que
/// `tri_sur_expression_arithmetique` ; le NULL produit par l'expression suit
/// la clause NULLS comme n'importe quelle colonne.
#[test]
fn tri_sur_expression_produisant_des_null() {
    let db = base_t(FIXTURE_NULLS);
    let r = lire(&db, "MATCH (n:T) RETURN n.v ORDER BY n.v + 1 ASC NULLS LAST");
    assert_eq!(
        entiers(&r, 0),
        vec![Some(10), Some(20), Some(30), None, None],
        "v + 1 vaut NULL là où v est absent : ces lignes doivent aller en \
         queue sous NULLS LAST, exactement comme sur la propriété nue"
    );
}

// ---------------------------------------------------------------------------
// GROUPE H — Propriété absente de certains nœuds
// ---------------------------------------------------------------------------

/// Le cas de production. Une propriété présente sur une minorité de nœuds : la
/// majorité sans la propriété ne doit pas noyer le résultat en tête d'un
/// classement par récence.
///
/// Ce test est écrit sur la forme EXACTE qui a fait perdre son fil à l'entité
/// (`ORDER BY … DESC LIMIT k`, fiche `orchestrateur-nulls-first`), avec la
/// clause `NULLS LAST` qui est censée l'en protéger.
#[test]
// corrigé par ce commit (placement ABSOLU des NULL + défaut par direction)
fn recence_desc_nulls_last_ne_remonte_pas_les_lignes_vides() {
    let mut lignes: Vec<(String, Option<i64>)> = Vec::new();
    // Six nœuds SANS la propriété, insérés EN PREMIER — comme les claims
    // hérités de production.
    for i in 0..6 {
        lignes.push((format!("vide_{i}"), None));
    }
    for i in 0..4 {
        lignes.push((format!("date_{i}"), Some(100 + i)));
    }
    let refs: Vec<(&str, Option<i64>)> =
        lignes.iter().map(|(n, v)| (n.as_str(), *v)).collect();
    let db = base_t(&refs);

    let r = lire(
        &db,
        "MATCH (n:T) RETURN n.nom, n.v ORDER BY n.v DESC NULLS LAST LIMIT 4",
    );
    assert_eq!(
        noms(&r, 0),
        vec!["date_3", "date_2", "date_1", "date_0"],
        "« les 4 plus récents » avec NULLS LAST explicite doit rendre QUATRE \
         lignes datées, de la plus récente à la plus ancienne. Si les six \
         nœuds sans date occupent les premières places, la clause NULLS LAST a \
         produit l'inverse de ce qu'elle annonce, et tout consommateur de \
         cette page ne voit que du vide"
    );
}

/// Toutes les lignes sans la propriété : le tri ne doit ni échouer, ni perdre
/// de ligne, ni réordonner (tout est ex æquo).
#[test]
fn propriete_absente_partout_conserve_toutes_les_lignes() {
    let db = base_t(&[("z_c", None), ("z_a", None), ("z_b", None)]);
    for clause in ["ASC", "DESC", "ASC NULLS FIRST", "DESC NULLS LAST"] {
        let r = lire(
            &db,
            &format!("MATCH (n:T) RETURN n.nom ORDER BY n.v {clause}"),
        );
        assert_eq!(
            noms(&r, 0),
            vec!["z_c", "z_a", "z_b"],
            "clause « {clause} » : toutes les lignes sont NULL donc ex æquo — \
             aucune ne doit disparaître et leur ordre d'arrivée doit tenir"
        );
    }
}

/// Tri sur une propriété qui n'existe sur AUCUN nœud, et n'a donc jamais été
/// déclarée au schéma. Doit se comporter comme « tout NULL », pas échouer.
#[test]
fn tri_sur_propriete_jamais_declaree() {
    let db = base_t(&[("w_c", Some(3)), ("w_a", Some(1)), ("w_b", Some(2))]);
    let r = lire(&db, "MATCH (n:T) RETURN n.nom ORDER BY n.inexistante ASC");
    assert_eq!(
        noms(&r, 0),
        vec!["w_c", "w_a", "w_b"],
        "une clé de tri inconnue vaut NULL partout : toutes les lignes sont ex \
         æquo, l'ordre d'arrivée est conservé, et aucune ligne n'est perdue"
    );
}

// ---------------------------------------------------------------------------
// GROUPE I — Types mélangés dans la même colonne
// ---------------------------------------------------------------------------
//
// SÉMANTIQUE VISÉE. GQL, comme Cypher, définit une ORDONNABILITÉ totale entre
// types : les valeurs de types différents sont comparables et rangées selon une
// précédence de types fixée. Quelle que soit la précédence retenue, une
// propriété MINIMALE s'impose, indépendante de tout choix : la comparaison doit
// être un ORDRE FAIBLE STRICT — donc TRANSITIVE. Sans transitivité, `sort_by`
// ne peut rien garantir, pas même sur les valeurs de type homogène.
//
// Test formulé pour ne dépendre d'AUCUNE précédence : quel que soit l'endroit
// où le moteur range les chaînes par rapport aux entiers, les entiers ENTRE EUX
// doivent rester croissants, et les chaînes ENTRE ELLES aussi. C'est le plus
// petit engagement défendable.

/// TROUVAILLE — DIVERGENCE CONFIRMÉE.
///
/// `compare_values_total` (obrain-core/src/execution/operators/value_utils.rs:57-74)
/// rend `Ordering::Equal` pour TOUTE paire de types non prévue — le bras
/// fourre-tout `_ => Ordering::Equal`. Un entier et une chaîne sont donc
/// déclarés « égaux ».
///
/// La conséquence n'est pas seulement un rangement discutable : la relation
/// n'est plus TRANSITIVE. `Int64(3) ≡ String("b")` et `String("b") ≡ Int64(1)`,
/// mais `Int64(3) > Int64(1)`. `sort_by` reçoit un comparateur qui n'est pas un
/// ordre faible strict ; son résultat n'est spécifié par rien, et les entiers
/// peuvent ressortir désordonnés ENTRE EUX du seul fait qu'une chaîne s'est
/// intercalée. La contamination est le vrai défaut : une seule valeur de type
/// étranger dans une colonne corrompt l'ordre de toutes les autres.
///
/// MESURÉ, et le résultat est plus net que prévu : sur le semis
/// (3, "b", 1, "a", 2), `ORDER BY n.v ASC` rend `["i3","sb","i1","sa","i2"]`
/// — l'ORDRE D'INSERTION, INCHANGÉ. Le tri est totalement INERTE. La raison
/// est mécanique : les chaînes s'intercalent entre les entiers, chaque
/// comparaison de voisins traverse une frontière de type et rend `Equal`, si
/// bien que le tri stable, ne voyant que des égalités, ne déplace rien — et
/// les entiers ne sont JAMAIS comparés entre eux. Ce n'est donc pas « un
/// rangement inter-types discutable » : c'est la perte silencieuse du tri sur
/// les valeurs qui, elles, étaient parfaitement comparables. Une seule
/// propriété au type hétérogène — un `count` écrit tantôt `3` tantôt `"3"` —
/// suffit à rendre muet l'`ORDER BY` de toute la colonne, sans erreur ni
/// avertissement.
#[test]
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : DIVERGENCE gql09-types-melanges : compare_values_total rend Equal entre types, la relation n'est pas transitive
fn types_melanges_les_valeurs_de_meme_type_restent_ordonnees() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    // Semis délibérément faux, types alternés pour maximiser la contamination.
    for req in [
        "INSERT (:X {nom: 'i3', v: 3})",
        "INSERT (:X {nom: 'sb', v: 'b'})",
        "INSERT (:X {nom: 'i1', v: 1})",
        "INSERT (:X {nom: 'sa', v: 'a'})",
        "INSERT (:X {nom: 'i2', v: 2})",
    ] {
        s.execute(req).unwrap();
    }
    s.commit().unwrap();

    let ordre = noms(&lire(&db, "MATCH (n:X) RETURN n.nom ORDER BY n.v ASC"), 0);
    let pos = |cible: &str| {
        ordre
            .iter()
            .position(|n| n == cible)
            .unwrap_or_else(|| panic!("{cible} absent du résultat {ordre:?}"))
    };

    assert!(
        pos("i1") < pos("i2") && pos("i2") < pos("i3"),
        "les entiers doivent rester croissants ENTRE EUX quelle que soit la \
         place des chaînes ; obtenu {ordre:?}"
    );
    assert!(
        pos("sa") < pos("sb"),
        "les chaînes doivent rester croissantes ENTRE ELLES quelle que soit la \
         place des entiers ; obtenu {ordre:?}"
    );
}

/// Entiers et flottants dans la même colonne : ce couple-là EST prévu par
/// `compare_values_total` (conversion en f64), la comparaison numérique doit
/// donc traverser la frontière de type sans anicroche.
#[test]
fn types_numeriques_mixtes_sont_ordonnes_par_valeur() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    for req in [
        "INSERT (:Y {nom: 'e3', v: 3})",
        "INSERT (:Y {nom: 'f15', v: 1.5})",
        "INSERT (:Y {nom: 'e1', v: 1})",
        "INSERT (:Y {nom: 'f25', v: 2.5})",
    ] {
        s.execute(req).unwrap();
    }
    s.commit().unwrap();

    let r = lire(&db, "MATCH (n:Y) RETURN n.nom ORDER BY n.v ASC");
    assert_eq!(
        noms(&r, 0),
        vec!["e1", "f15", "f25", "e3"],
        "entiers et flottants doivent s'ordonner par VALEUR (1 < 1,5 < 2,5 < \
         3) et non par type. Un résultat groupant les entiers puis les \
         flottants signalerait une comparaison qui abandonne entre types"
    );
}

// ---------------------------------------------------------------------------
// GROUPE J — Recoupement de la mesure « ORDER BY … LIMIT k coûte le prix de
// LIMIT tout » (fiche atelier planificateur-d1c52d91)
// ---------------------------------------------------------------------------

/// Ce test ne mesure pas un coût : il vérifie la JUSTESSE sur la forme et
/// l'échelle qui ont servi à la mesure. Le tri borné a été introduit depuis
/// (`bounded_top_k`, descente de la limite dans `plan_sort`) ; ce qu'il faut
/// s'assurer, c'est que le gain de temps n'a pas été payé en exactitude sur un
/// volume où le chemin borné est réellement emprunté — plus de deux chunks, une
/// limite très petite devant le cardinal, et des ex æquo partout.
///
/// 3 000 nœuds, clé à 10 valeurs distinctes : 300 lignes ex æquo par valeur,
/// donc la frontière du top-8 tombe en plein milieu d'un groupe d'ex æquo. Le
/// résultat borné doit rester le préfixe exact du complet.
#[test]
fn justesse_du_tri_borne_a_l_echelle_de_la_mesure() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    // Semis en dents de scie : l'ordre d'insertion n'a aucun rapport avec
    // l'ordre trié, et chaque valeur de clé revient tous les 10 nœuds.
    for i in 0..3000u32 {
        let cle = (i * 7) % 10;
        s.execute(&format!("INSERT (:B {{nom: 'n{i}', k: {cle}}})"))
            .unwrap();
    }
    s.commit().unwrap();

    let complet = noms(&lire(&db, "MATCH (n:B) RETURN n.nom ORDER BY n.k DESC"), 0);
    assert_eq!(complet.len(), 3000, "le tri complet doit rendre les 3 000 lignes");

    for k in [1usize, 8, 299, 300, 301, 1500] {
        let borne = noms(
            &lire(&db, &format!("MATCH (n:B) RETURN n.nom ORDER BY n.k DESC LIMIT {k}")),
            0,
        );
        assert_eq!(
            borne.len(),
            k,
            "LIMIT {k} doit rendre exactement {k} lignes"
        );
        assert_eq!(
            borne,
            complet.iter().take(k).cloned().collect::<Vec<_>>(),
            "k={k} sur 3 000 lignes à 300 ex æquo par valeur de clé : le tri \
             borné doit rester le préfixe exact du tri complet. k=299/300/301 \
             encadrent la frontière d'un groupe d'ex æquo"
        );
    }
}
