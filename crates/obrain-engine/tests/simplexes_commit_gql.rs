//! La maintenance simpliciale est-elle branchée sur le VRAI chemin d'écriture ?
//!
//! ── POURQUOI CE TEST EXISTE ──────────────────────────────────────────
//!
//! Les tests de `obrain-substrate` prouvent que `Store::create_edge` et
//! consorts entretiennent la couche simpliciale. Ils ne prouvent PAS que
//! le chemin qu'emprunte un utilisateur — une session GQL, un `INSERT`
//! d'arête, un `COMMIT` — passe bien par ces fonctions. Entre le
//! `CreateEdgeOperator` du planificateur et le substrat il y a le trait
//! `GraphStoreMut`, des variantes versionnées, un journal d'annulation et
//! une couche de transaction : n'importe laquelle de ces indirections
//! pourrait écrire l'arête sans réveiller les hooks, et TOUTE la suite
//! substrat resterait verte pendant que la fonctionnalité serait morte en
//! production.
//!
//! On mesure donc la couche simpliciale APRÈS des requêtes GQL, et jamais
//! après un appel direct à l'API de passe.
//!
//! ── CE QUE CHAQUE TEST PROUVE ────────────────────────────────────────
//!
//!   · `insert_gql_...`    — l'`INSERT` d'une arête qui FERME un triangle
//!                           matérialise ce triangle AVANT le `COMMIT` de
//!                           la session (donc dans la même transaction),
//!                           et le triangle SURVIT à la fermeture et à la
//!                           réouverture de la base (donc il est dans le
//!                           WAL de cette transaction, pas seulement en
//!                           mémoire).
//!   · `delete_gql_...`    — le `DELETE` d'une arête tombstone les
//!                           simplexes qui la contenaient, même chemin.
//!   · `dispatch_gql_...`  — les hyperarêtes typées suivent aussi les
//!                           propriétés posées par GQL (`SET` / `REMOVE`),
//!                           pas seulement les arêtes.
//!   · `anti_vacuite_...`  — LE test qui donne leur valeur aux trois
//!                           autres : le MÊME script GQL, sur une base
//!                           dont les passes ne sont pas configurées, ne
//!                           matérialise RIEN. Sans lui, un `assert` sur
//!                           « le triangle est là » pourrait être satisfait
//!                           par n'importe quel autre mécanisme.
//!
//! ```bash
//! cargo test -p obrain-engine --test simplexes_commit_gql
//! ```

use std::collections::BTreeSet;
use std::sync::Arc;

use obrain_common::types::Value;
use obrain_engine::{ObrainDB, Session};
use obrain_substrate::{
    DispatchSpec, SimplexPassesSpec, SubstrateStore, simplex_flags, simplex_kind,
};

// ── Fixture ──────────────────────────────────────────────────────────

/// Spec de portage cbm, maintenance ACTIVE : arêtes `CALLS`, hyperarêtes
/// `DISPATCH` groupées par la propriété `line`.
fn spec_maintenue() -> SimplexPassesSpec {
    SimplexPassesSpec {
        clique_edge_type: "CALLS".into(),
        clique_labels: vec![],
        dispatch: Some(DispatchSpec {
            edge_type: "CALLS".into(),
            group_prop_key: "line".into(),
            he_type: "DISPATCH".into(),
        }),
        maintenance: true,
    }
}

/// Ouvre une base sur DISQUE (le mode mémoire n'expose pas de poignée
/// substrat typée) et rend la base + sa poignée. `configurer` dit si la
/// couche simpliciale est armée — c'est le seul paramètre qui sépare le
/// cas nominal du contrôle d'anti-vacuité.
fn base(chemin: &std::path::Path, configurer: bool) -> (ObrainDB, Arc<SubstrateStore>) {
    let db = ObrainDB::open(chemin).expect("ouverture de la base");
    let sub = db
        .substrate_handle()
        .expect("base adossée au substrat : poignée typée attendue");
    if configurer {
        sub.configurer_passes_simpliciales(&spec_maintenue())
            .expect("configuration des passes");
        // Passe sur la couche VIDE : elle ne crée rien, elle ouvre la vie
        // au commit (la config est écrite, les zones existent).
        sub.materialiser_simplexes().expect("passe initiale");
    }
    (db, sub)
}

/// Membres des simplexes vivants d'un genre donné, triés — la forme
/// canonique sur laquelle porteront toutes les assertions.
fn membres(sub: &SubstrateStore, kind: u8) -> BTreeSet<Vec<u32>> {
    sub.iter_simplices_kind(kind)
        .unwrap()
        .into_iter()
        .map(|id| sub.get_simplex(id).unwrap().unwrap().members)
        .collect()
}

/// Drapeaux du simplexe vivant de membres donnés (`None` s'il n'existe pas).
fn drapeaux(sub: &SubstrateStore, kind: u8, cherches: &[u32]) -> Option<u8> {
    sub.iter_simplices_kind(kind)
        .unwrap()
        .into_iter()
        .filter_map(|id| sub.get_simplex(id).unwrap())
        .find(|v| v.members == cherches)
        .map(|v| v.flags)
}

/// Nombre total de simplexes vivants, tous genres confondus.
fn total_vivants(sub: &SubstrateStore) -> usize {
    [
        simplex_kind::TRIANGLE,
        simplex_kind::K4,
        simplex_kind::K5,
        simplex_kind::HYPEREDGE,
    ]
    .into_iter()
    .map(|k| sub.iter_simplices_kind(k).unwrap().len())
    .sum()
}

/// Ids substrat (slots) des nœuds `:F`, indexés par leur propriété `i`,
/// LUS PAR GQL — on ne suppose pas l'allocation des slots.
fn slots(db: &ObrainDB, n: usize) -> Vec<u32> {
    let s = db.session();
    (0..n)
        .map(|i| {
            let r = s
                .execute(&format!("MATCH (n:F {{i: {i}}}) RETURN id(n) AS id"))
                .expect("lecture d'id");
            assert_eq!(r.rows.len(), 1, "nœud i={i} : un seul attendu");
            match &r.rows[0][0] {
                Value::Int64(v) => u32::try_from(*v).expect("slot"),
                autre => panic!("id(n) : entier attendu, reçu {autre:?}"),
            }
        })
        .collect()
}

/// Crée `n` nœuds `:F {i: …}` PAR GQL.
fn semer(db: &ObrainDB, n: usize) {
    let s = db.session();
    for i in 0..n {
        s.execute(&format!("INSERT (:F {{i: {i}}})")).unwrap();
    }
}

/// L'arête `CALLS` de `i` vers `j`, insérée PAR GQL **dans la session
/// donnée** — donc dans sa transaction, quand elle en a une ouverte.
/// C'est ce point qui fait la différence entre « le hook existe » et « le
/// hook s'exécute dans la transaction de l'utilisateur ».
fn arete(session: &Session, i: usize, j: usize) {
    session
        .execute(&format!(
            "MATCH (x:F {{i: {i}}}), (y:F {{i: {j}}}) INSERT (x)-[:CALLS]->(y)"
        ))
        .unwrap_or_else(|e| panic!("INSERT arête {i}->{j} : {e}"));
}

// ── 1. INSERT : la matérialisation est dans la transaction ───────────

#[test]
fn insert_gql_materialise_le_triangle_dans_la_meme_transaction() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let attendu: Vec<u32>;
    {
        let (db, sub) = base(&chemin, true);
        semer(&db, 3);
        let n = slots(&db, 3);
        attendu = {
            let mut v = n.clone();
            v.sort_unstable();
            v
        };

        // Une SEULE session, une SEULE transaction GQL explicite : les
        // trois `INSERT` d'arête sont exécutés PAR elle, entre son BEGIN
        // et son COMMIT.
        let mut session = db.session();
        session.begin_transaction().expect("BEGIN");

        arete(&session, 0, 1);
        arete(&session, 0, 2);
        assert!(
            membres(&sub, simplex_kind::TRIANGLE).is_empty(),
            "deux arêtes : le wedge n'est pas encore fermé"
        );

        // L'arête qui FERME le wedge.
        arete(&session, 1, 2);

        // ── LE POINT DU TEST ──
        // Aucun COMMIT n'a eu lieu, aucune passe n'a été rappelée : le
        // triangle est déjà là. La maintenance s'est donc exécutée DANS
        // la transaction qui a écrit l'arête.
        assert_eq!(
            membres(&sub, simplex_kind::TRIANGLE),
            BTreeSet::from([attendu.clone()]),
            "le triangle doit exister AVANT le COMMIT de la session"
        );
        assert_eq!(
            drapeaux(&sub, simplex_kind::TRIANGLE, &attendu),
            Some(0),
            "wedge fermé non cyclique : pas de bit CYCLIC"
        );

        session.commit().expect("COMMIT");
        assert_eq!(
            membres(&sub, simplex_kind::TRIANGLE),
            BTreeSet::from([attendu.clone()]),
            "le COMMIT ne défait rien"
        );

        // La fermeture c→a d'un 3-cycle pose le bit d'orientation, elle
        // aussi par GQL (hors transaction explicite cette fois : le
        // chemin d'auto-commit doit se comporter pareil).
        arete(&db.session(), 2, 0);
        assert_eq!(
            drapeaux(&sub, simplex_kind::TRIANGLE, &attendu),
            Some(simplex_flags::CYCLIC),
            "la fermeture arrière pose CYCLIC, en auto-commit aussi"
        );

        db.flush_wal().expect("flush WAL");
    }

    // ── DURABILITÉ : la couche était bien dans le WAL de ces
    //    transactions, pas seulement dans le mmap du processus. ──
    let sub = SubstrateStore::open(&chemin).expect("réouverture");
    assert_eq!(
        membres(&sub, simplex_kind::TRIANGLE),
        BTreeSet::from([attendu.clone()]),
        "le triangle survit à la fermeture/réouverture de la base"
    );
    assert_eq!(
        drapeaux(&sub, simplex_kind::TRIANGLE, &attendu),
        Some(simplex_flags::CYCLIC),
        "le bit CYCLIC aussi"
    );
}

// ── 2. DELETE : le tombstone suit le même chemin ─────────────────────

#[test]
fn delete_gql_tombstone_les_simplexes_touches() {
    let td = tempfile::tempdir().unwrap();
    let (db, sub) = base(&td.path().join("kb"), true);
    semer(&db, 4);
    let n = slots(&db, 4);

    // K4 dirigé complet sur les quatre nœuds : 4 triangles + 1 K4.
    let mut session = db.session();
    for i in 0..4 {
        for j in (i + 1)..4 {
            arete(&session, i, j);
        }
    }
    assert_eq!(membres(&sub, simplex_kind::TRIANGLE).len(), 4);
    assert_eq!(membres(&sub, simplex_kind::K4).len(), 1);

    // DELETE GQL d'UNE arête, dans une transaction explicite de la même
    // session.
    session.begin_transaction().expect("BEGIN");
    session
        .execute("MATCH (x:F {i: 0})-[r:CALLS]->(y:F {i: 1}) DELETE r")
        .expect("DELETE");

    // Avant même le COMMIT : le K4 est mort et les deux triangles qui
    // portaient l'arête (0,1,·) aussi. Les deux autres survivent.
    assert!(
        membres(&sub, simplex_kind::K4).is_empty(),
        "le K4 tombe avec son arête, dans la transaction"
    );
    let restants = membres(&sub, simplex_kind::TRIANGLE);
    assert_eq!(
        restants.len(),
        2,
        "seuls les triangles sans l'arête (0,1) survivent — restants : {restants:?}"
    );
    let (a, b) = (n[0].min(n[1]), n[0].max(n[1]));
    for t in &restants {
        assert!(
            !(t.contains(&a) && t.contains(&b)),
            "le triangle {t:?} contient encore l'arête supprimée"
        );
    }
    session.commit().expect("COMMIT");
    assert_eq!(membres(&sub, simplex_kind::TRIANGLE).len(), 2);

    // DETACH DELETE d'un nœud : la cascade I5 passe elle aussi par le
    // GQL. Les deux triangles restants portent tous deux le nœud 3
    // (ce sont (0,2,3) et (1,2,3) à des ids près), donc il ne doit plus
    // rien rester.
    session
        .execute("MATCH (n:F {i: 3}) DETACH DELETE n")
        .expect("DETACH DELETE");
    assert!(
        membres(&sub, simplex_kind::TRIANGLE).is_empty(),
        "DETACH DELETE : la cascade I5 emporte les simplexes incidents"
    );

    // Le rebuild de contrôle ne trouve RIEN à réparer : ce que la
    // maintenance a laissé est exactement ce que la passe complète
    // aurait produit.
    let stats = sub.materialiser_simplexes().unwrap();
    assert_eq!(
        (stats.crees, stats.reecrits, stats.tombstones),
        (0, 0, 0),
        "après le DELETE GQL, la passe complète est un no-op intégral"
    );
}

// ── 3. Hyperarêtes DISPATCH : les propriétés posées par GQL ──────────

#[test]
fn dispatch_gql_suit_les_proprietes_de_la_requete() {
    let td = tempfile::tempdir().unwrap();
    let (db, sub) = base(&td.path().join("kb"), true);
    semer(&db, 4);
    let n = slots(&db, 4);
    let session = db.session();

    // Site d'appel (n0) vers deux cibles, LIGNE POSÉE DANS L'INSERT.
    for j in [1usize, 2] {
        session
            .execute(&format!(
                "MATCH (x:F {{i: 0}}), (y:F {{i: {j}}}) INSERT (x)-[:CALLS {{line: 42}}]->(y)"
            ))
            .unwrap_or_else(|e| panic!("INSERT arête avec propriété : {e}"));
    }
    let groupe: Vec<u32> = {
        let mut v = vec![n[0], n[1], n[2]];
        v.sort_unstable();
        v
    };
    assert_eq!(
        membres(&sub, simplex_kind::HYPEREDGE),
        BTreeSet::from([groupe]),
        "deux cibles distinctes à la même ligne ⇒ une hyperarête DISPATCH"
    );

    // Une troisième cible, dont la ligne est posée APRÈS coup par SET.
    arete(&session, 0, 3);
    assert_eq!(
        membres(&sub, simplex_kind::HYPEREDGE).len(),
        1,
        "sans ligne, la troisième arête n'entre pas dans le groupe"
    );
    session
        .execute("MATCH (x:F {i: 0})-[r:CALLS]->(y:F {i: 3}) SET r.line = 42")
        .expect("SET propriété");
    let large: Vec<u32> = {
        let mut v = vec![n[0], n[1], n[2], n[3]];
        v.sort_unstable();
        v
    };
    assert_eq!(
        membres(&sub, simplex_kind::HYPEREDGE),
        BTreeSet::from([large]),
        "le SET GQL fait entrer la cible dans le groupe (ancien tombstoné)"
    );

    // REMOVE : la cible ressort, le groupe rétrécit.
    session
        .execute("MATCH (x:F {i: 0})-[r:CALLS]->(y:F {i: 3}) REMOVE r.line")
        .expect("REMOVE propriété");
    let etroit: Vec<u32> = {
        let mut v = vec![n[0], n[1], n[2]];
        v.sort_unstable();
        v
    };
    assert_eq!(
        membres(&sub, simplex_kind::HYPEREDGE),
        BTreeSet::from([etroit]),
        "le REMOVE GQL retire la cible du groupe"
    );

    let stats = sub.materialiser_simplexes().unwrap();
    assert_eq!(
        (stats.crees, stats.reecrits, stats.tombstones),
        (0, 0, 0),
        "la passe complète confirme l'état laissé par les requêtes"
    );
}

// ── 4. ROLLBACK : la couche suit l'annulation ────────────────────────

#[test]
fn rollback_gql_ne_laisse_pas_de_simplexe_orphelin() {
    // La contrepartie du test n°1 : si la matérialisation a lieu DANS la
    // transaction, alors l'annulation de cette transaction doit la
    // défaire — sinon la couche affirmerait un triangle que le graphe ne
    // porte pas, ce qui est pire qu'un triangle manquant.
    let td = tempfile::tempdir().unwrap();
    let (db, sub) = base(&td.path().join("kb"), true);
    semer(&db, 3);

    let mut session = db.session();
    arete(&session, 0, 1);
    arete(&session, 0, 2);

    session.begin_transaction().expect("BEGIN");
    arete(&session, 1, 2); // ferme le wedge…
    assert_eq!(
        membres(&sub, simplex_kind::TRIANGLE).len(),
        1,
        "le triangle est matérialisé dans la transaction"
    );
    session.rollback().expect("ROLLBACK");

    // …et l'annulation le reprend, parce qu'elle repasse par le chemin
    // de suppression d'arête, qui porte le même hook.
    assert_eq!(
        db.session()
            .execute("MATCH (x:F {i: 1})-[r:CALLS]->(y:F {i: 2}) RETURN count(r)")
            .unwrap()
            .rows[0][0],
        Value::Int64(0),
        "prémisse : le ROLLBACK a bien défait l'arête"
    );
    assert!(
        membres(&sub, simplex_kind::TRIANGLE).is_empty(),
        "aucun triangle orphelin ne survit au ROLLBACK"
    );
    let stats = sub.materialiser_simplexes().unwrap();
    assert_eq!(
        (stats.crees, stats.reecrits, stats.tombstones),
        (0, 0, 0),
        "la passe complète confirme l'état d'après annulation"
    );
}

// ── 5. Anti-vacuité ──────────────────────────────────────────────────

#[test]
fn anti_vacuite_sans_configuration_le_meme_gql_ne_materialise_rien() {
    // MÊME script GQL que les trois tests ci-dessus — K4 complet,
    // propriétés `line`, 3-cycle — sur une base dont les passes ne sont
    // PAS configurées. Si les assertions précédentes étaient satisfaites
    // par autre chose que les hooks (une passe implicite, un effet de
    // bord d'ouverture), ce test le verrait : il compterait des
    // simplexes là où il ne doit y en avoir aucun.
    let td = tempfile::tempdir().unwrap();
    let (db, sub) = base(&td.path().join("kb"), false);
    semer(&db, 4);
    let session = db.session();
    for i in 0..4 {
        for j in (i + 1)..4 {
            arete(&session, i, j);
        }
    }
    session
        .execute("MATCH (x:F {i: 0})-[r:CALLS]->(y:F {i: 1}) SET r.line = 42")
        .expect("SET propriété");
    arete(&session, 3, 0);
    session
        .execute("MATCH (x:F {i: 0})-[r:CALLS]->(y:F {i: 2}) DELETE r")
        .expect("DELETE");

    assert_eq!(
        total_vivants(&sub),
        0,
        "couche non configurée : le commit GQL ne matérialise RIEN"
    );

    // Et le mécanisme n'est pas cassé pour autant : configurée puis
    // matérialisée à la main, la MÊME base rend les simplexes attendus.
    sub.configurer_passes_simpliciales(&spec_maintenue())
        .unwrap();
    let stats = sub.materialiser_simplexes().unwrap();
    assert!(
        stats.triangles > 0,
        "le graphe portait bien des triangles : {stats:?}"
    );
}
