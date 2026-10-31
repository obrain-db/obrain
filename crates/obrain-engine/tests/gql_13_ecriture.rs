//! Lot 13 — CONFORMITÉ GQL : L'ÉCRITURE ET L'ÉTAT QU'ELLE LAISSE.
//!
//! `INSERT`, `MERGE`, `SET`, `REMOVE`, `DELETE` / `DETACH DELETE`.
//!
//! # L'ANGLE DE CE FICHIER
//!
//! Les autres lots interrogent la LECTURE : ce qu'une expression rend. Celui-ci
//! interroge l'ÉTAT : ce qu'une écriture a réellement changé dans le graphe, et
//! ce qu'elle aurait dû changer.
//!
//! Conséquence de méthode, et elle est contraignante : une assertion posée sur
//! un EFFET (« la ligne n'apparaît plus », « la requête ne renvoie rien ») peut
//! être verte pour une raison sans rapport avec la cause qu'on croit tester —
//! un MATCH qui ne matche pas, une clause avalée sans erreur, un filtre qui
//! écarte la ligne. On assertionne donc, chaque fois que c'est possible,
//! l'ÉTAT OBSERVABLE PRÉCIS : l'ensemble exact des clés (`keys`), la carte
//! exacte des propriétés (`properties`), la valeur exacte, le compte exact —
//! jamais la seule absence.
//!
//! # MÉTHODE — lire avant de modifier ce fichier
//!
//! Chaque assertion encode une INTENTION SÉMANTIQUE, jamais une observation du
//! moteur. On énonce d'abord ce que l'écriture DOIT laisser et pourquoi, puis
//! on vérifie.
//!
//! Ce n'est pas un scrupule de style. Ce dépôt en a déjà payé le prix :
//! `crates/obrain-substrate/tests/vector_top_k_exact.rs` s'est déclaré
//! « contrat de PARITÉ LEGACY » et a figé un défaut en écartant explicitement
//! la lecture correcte du code. Un test écrit en observant le moteur rend le
//! défaut intouchable.
//!
//! Quand le moteur diverge de l'intention, le test est placé derrière
//! `#[cfg(feature = "defauts-connus")]` et précédé d'un commentaire `// DÉFAUT:`
//! qui NOMME la divergence et son emplacement en fichier:ligne. Il reste rouge
//! par construction — jamais réécrit pour devenir vert.
//!
//!   `cargo test -p obrain-engine --test gql_13_ecriture`
//!       la suite par défaut : verte, les défauts ne sont pas compilés.
//!   `cargo test -p obrain-engine --test gql_13_ecriture --features defauts-connus`
//!       les défauts tournent et ÉCHOUENT — légitimement. Ce sont les tests à
//!       faire passer, pas à supprimer.
//!
//! # LES TROIS ÉTATS D'UNE PROPRIÉTÉ
//!
//! Le stockage distingue TROIS états (`obrain-substrate/src/store.rs:6892`,
//! `lookup_node_property_v2` rend `Some(Some(v))` vivant / `Some(None)` pierre
//! tombale / `None` rien). Le langage n'en voit que deux : `IS NULL` est vrai
//! pour les trois, et le SEUL discriminant offert à GQL est `keys(n)` /
//! `properties(n)`. Tout ce fichier s'appuie sur ce discriminant : c'est le
//! seul instrument qui sépare « la clé a disparu » de « la clé porte un NULL ».
//!
//! Run : cargo test -p obrain-engine --test gql_13_ecriture

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Outils d'observation.
//
// L'état est construit par le SEUL code de production (INSERT/MERGE/SET GQL),
// sauf mention explicite : un état fabriqué à la main ne prouverait que le
// comportement de la fabrication.
// ---------------------------------------------------------------------------

/// Exécute une suite d'écritures dans UNE transaction et la commite.
fn ecrire(db: &ObrainDB, requetes: &[&str]) {
    let mut session = db.session();
    session.begin_transaction().unwrap();
    for r in requetes {
        session
            .execute(r)
            .unwrap_or_else(|e| panic!("l'écriture `{r}` doit s'exécuter, erreur : {e:?}"));
    }
    session.commit().unwrap();
}

/// Lit dans une session NEUVE — donc après commit, jamais depuis le cache
/// d'écriture d'une transaction ouverte.
fn lire(db: &ObrainDB, requete: &str) -> Vec<Vec<Value>> {
    let session = db.session();
    session
        .execute(requete)
        .unwrap_or_else(|e| panic!("la lecture `{requete}` doit s'exécuter, erreur : {e:?}"))
        .rows
}

/// Une seule ligne, une seule colonne.
fn scalaire(db: &ObrainDB, requete: &str) -> Value {
    let rows = lire(db, requete);
    assert_eq!(
        rows.len(),
        1,
        "`{requete}` doit rendre EXACTEMENT une ligne, obtenu {}",
        rows.len()
    );
    rows[0][0].clone()
}

/// Le nombre de nœuds portant l'étiquette `label`.
fn compte(db: &ObrainDB, label: &str) -> i64 {
    match scalaire(db, &format!("MATCH (n:{label}) RETURN count(n) AS c")) {
        Value::Int64(n) => n,
        autre => panic!("count doit rendre un Int64, obtenu {autre:?}"),
    }
}

/// Traduit une `Value::List` de chaînes en vecteur trié — `keys()` n'ordonne
/// rien, seul l'ENSEMBLE des clés a un sens.
fn en_ensemble(v: &Value) -> Vec<String> {
    match v {
        Value::List(items) => {
            let mut cles: Vec<String> = items
                .iter()
                .map(|i| match i {
                    Value::String(s) => s.to_string(),
                    autre => panic!("une clé doit être une chaîne, obtenu {autre:?}"),
                })
                .collect();
            cles.sort();
            cles
        }
        autre => panic!("keys() doit rendre une liste, obtenu {autre:?}"),
    }
}

/// L'ensemble EXACT des clés portées par l'unique nœud que `motif` sélectionne.
///
/// `motif` est un motif MATCH complet liant la variable `n`, p. ex.
/// `"(n:T {id: 'a'})"`.
fn cles(db: &ObrainDB, motif: &str) -> Vec<String> {
    en_ensemble(&scalaire(db, &format!("MATCH {motif} RETURN keys(n) AS c")))
}

/// La carte EXACTE des propriétés de l'unique nœud que `motif` sélectionne.
fn proprietes(db: &ObrainDB, motif: &str) -> BTreeMap<String, Value> {
    match scalaire(db, &format!("MATCH {motif} RETURN properties(n) AS p")) {
        Value::Map(m) => m
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.clone()))
            .collect(),
        autre => panic!("properties() doit rendre une carte, obtenu {autre:?}"),
    }
}

/// Les identifiants des nœuds portant `label`, LUS PAR BALAYAGE.
///
/// Voie délibérément distincte de [`compte`] : `count(...)` et la projection
/// des lignes n'empruntent pas le même chemin dans ce moteur (le GROUPE 8
/// l'établit), donc un test d'unicité qui ne consulterait que l'agrégat
/// pourrait être vert sans qu'aucune ligne ne le justifie.
fn identifiants(db: &ObrainDB, label: &str) -> Vec<String> {
    let v = scalaire(db, &format!("MATCH (n:{label}) RETURN collect(n.id) AS l"));
    match v {
        Value::List(items) => {
            let mut ids: Vec<String> = items
                .iter()
                .map(|i| match i {
                    Value::String(s) => s.to_string(),
                    autre => format!("{autre:?}"),
                })
                .collect();
            ids.sort();
            ids
        }
        autre => panic!("collect() doit rendre une liste, obtenu {autre:?}"),
    }
}

/// Raccourci de lisibilité pour les attentes d'ensembles de clés.
fn ens(cles: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = cles.iter().map(|s| s.to_string()).collect();
    v.sort();
    v
}

// ===========================================================================
// GROUPE 1 — INSERT : CE QUE L'INSERTION DÉPOSE, EXACTEMENT
//
// Le premier devoir d'une insertion est de ne déposer NI PLUS NI MOINS que ce
// qui est écrit dans le motif. « Ni moins » se vérifie en lisant les valeurs ;
// « ni plus » exige de comparer l'ENSEMBLE des clés, pas de piocher dedans.
// ===========================================================================

/// INTENTION : `INSERT` dépose exactement les clés citées dans le motif — pas
/// une de plus, pas une de moins, et avec les valeurs citées.
///
/// L'assertion porte sur l'ensemble complet des clés parce qu'une assertion
/// par propriété serait aveugle à la faute inverse : une clé parasite ajoutée
/// par le moteur (compteur interne, marqueur de version) passerait inaperçue.
#[test]
fn insert_depose_exactement_les_cles_citees() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a', n: 1, s: 'deux'})"]);

    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["id", "n", "s"]),
        "INSERT dépose exactement les trois clés du motif"
    );

    let p = proprietes(&db, "(n:T {id: 'a'})");
    assert_eq!(p.get("id"), Some(&Value::String("a".into())));
    assert_eq!(p.get("n"), Some(&Value::Int64(1)));
    assert_eq!(p.get("s"), Some(&Value::String("deux".into())));
    assert_eq!(p.len(), 3, "aucune propriété parasite, obtenu {p:?}");
}

/// INTENTION : `INSERT` n'est PAS idempotent. Deux insertions du même motif
/// laissent DEUX nœuds distincts.
///
/// C'est le contraste qui donne son sens à `MERGE` : si `INSERT` dédupliquait,
/// `MERGE` n'aurait pas de raison d'être. Le test ancre la différence.
#[test]
fn insert_repete_laisse_deux_noeuds_distincts() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a'})", "INSERT (:T {id: 'a'})"]);

    assert_eq!(
        compte(&db, "T"),
        2,
        "INSERT n'est pas idempotent : deux insertions du même motif laissent \
         deux nœuds — c'est précisément ce que MERGE existe pour éviter"
    );
}

/// INTENTION : une insertion faite dans une transaction est visible par la
/// LECTURE de cette même transaction, avant tout commit.
///
/// La règle GQL est celle de SQL : une transaction voit ses propres écritures.
/// Ce test est la référence à laquelle se compare le GROUPE 2 : c'est
/// exactement cette visibilité-là qui y donne une fausse assurance.
#[test]
fn insert_est_visible_dans_sa_propre_transaction() {
    let db = ObrainDB::new_in_memory();
    let mut session = db.session();
    session.begin_transaction().unwrap();
    session.execute("INSERT (:T {id: 'a', v: 7})").unwrap();

    let r = session
        .execute("MATCH (n:T {id: 'a'}) RETURN n.v AS v")
        .unwrap();
    assert_eq!(
        r.rows.len(),
        1,
        "la transaction doit voir sa propre insertion"
    );
    assert_eq!(r.rows[0][0], Value::Int64(7));

    session.commit().unwrap();
}

/// INTENTION : un `rollback` ne laisse RIEN de l'insertion.
///
/// L'assertion porte sur le compte de nœuds portant l'étiquette, pas sur
/// l'échec d'un MATCH : un MATCH vide serait vert même si le nœud existait
/// sous une forme que le motif ne sait plus atteindre.
#[test]
fn insert_annule_ne_laisse_aucun_noeud() {
    let db = ObrainDB::new_in_memory();
    let mut session = db.session();
    session.begin_transaction().unwrap();
    session.execute("INSERT (:T {id: 'a'})").unwrap();
    session.rollback().unwrap();

    assert_eq!(
        compte(&db, "T"),
        0,
        "après rollback, AUCUN nœud T ne doit subsister"
    );
}

/// INTENTION : `INSERT` d'un chemin crée les deux nœuds ET l'arête qui les
/// joint, en une seule écriture.
#[test]
fn insert_de_chemin_depose_les_deux_noeuds_et_l_arete() {
    let db = ObrainDB::new_in_memory();
    ecrire(
        &db,
        &["INSERT (:A {id: 'g'})-[:VERS {poids: 3}]->(:B {id: 'd'})"],
    );

    assert_eq!(compte(&db, "A"), 1, "le nœud source est déposé");
    assert_eq!(compte(&db, "B"), 1, "le nœud cible est déposé");

    let r = lire(
        &db,
        "MATCH (a:A)-[r:VERS]->(b:B) RETURN a.id AS g, r.poids AS p, b.id AS d",
    );
    assert_eq!(r.len(), 1, "exactement une arête VERS relie A à B");
    assert_eq!(r[0][0], Value::String("g".into()));
    assert_eq!(
        r[0][1],
        Value::Int64(3),
        "l'arête porte la propriété citée dans le motif"
    );
    assert_eq!(r[0][2], Value::String("d".into()));
}

/// INTENTION : `INSERT` d'un nœud à plusieurs étiquettes les dépose toutes ;
/// le nœud est atteignable par chacune, et c'est LE MÊME nœud.
///
/// L'identité est vérifiée par le compte total (`MATCH (n:A:B)` doit rendre
/// une ligne et l'ensemble des nœuds A doit être de taille 1) : deux nœuds
/// jumeaux passeraient un test qui se contenterait d'interroger chaque
/// étiquette séparément.
#[test]
fn insert_multi_etiquettes_depose_un_seul_noeud() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:A:B {id: 'x'})"]);

    assert_eq!(compte(&db, "A"), 1, "un seul nœud porte l'étiquette A");
    assert_eq!(compte(&db, "B"), 1, "un seul nœud porte l'étiquette B");
    assert_eq!(
        compte(&db, "A:B"),
        1,
        "c'est LE MÊME nœud qui porte les deux étiquettes, pas deux jumeaux"
    );
}

/// INTENTION : une insertion commitée est visible d'une session OUVERTE APRÈS
/// le commit — la durabilité ne dépend pas de la session qui a écrit.
#[test]
fn insert_commite_est_visible_d_une_autre_session() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a', v: 9})"]);

    let autre = db.session();
    let r = autre
        .execute("MATCH (n:T {id: 'a'}) RETURN n.v AS v")
        .unwrap();
    assert_eq!(
        r.rows.len(),
        1,
        "une session neuve voit l'écriture commitée"
    );
    assert_eq!(r.rows[0][0], Value::Int64(9));
}

// ===========================================================================
// GROUPE 2 — `INSERT (...) ... SET ...` : LE MOTIF QU'UN SERVEUR DE PRODUCTION
// REFUSE D'ÉMETTRE
//
// Ce groupe n'est pas une exploration : il documente un défaut déjà payé. Le
// serveur `obrain-tmn` porte un garde TEXTUEL dédié, `create_then_set_detected`
// (`obrain-tmn/src/obrain_db.rs:692`), qui REFUSE toute requête mêlant un
// `CREATE (...)` et un `SET` séparé, avec ce commentaire : « the
// confirmed-broken pattern on this engine ». Le message d'erreur qu'il rend
// (ibid. :1162) décrit précisément le symptôme :
//
//     « silently DROPS every property assigned by that SET [...] ok:true
//       returned, properties even visible in the immediate response, but never
//       persisted to disk »
//
// Un garde côté appelant qui protège d'un défaut moteur jamais corrigé est une
// dette qui ne se voit nulle part dans le moteur. Ce groupe l'y inscrit.
//
// La méthode a payé : en SÉPARANT la visibilité immédiate de la persistance,
// ce groupe établit que le symptôme décrit par le garde de production est FAUX
// sur cette version. La propriété n'est PAS « visible dans la réponse
// immédiate puis perdue au disque » : elle n'est écrite À AUCUN MOMENT, pas
// même dans la transaction émettrice. La perte est totale, jamais différée.
//
// LA CAUSE, établie en remontant jusqu'au parseur :
//
//   `parse_single_statement` (`obrain-adapters/src/query/gql/parser.rs:326`)
//   aiguille une requête dont le PREMIER jeton est `INSERT` vers `parse_insert()`
//   seul — un `Statement::DataModification(Insert)` autonome. Idem pour
//   `CREATE (` (:337, `parse_create_as_insert`). Or `SET`, `REMOVE` et `RETURN`
//   ne sont analysés QUE par `parse_query()`, l'autre branche de l'aiguillage
//   (:320-325). Et `parse()` (:241) ne vérifie JAMAIS que l'entrée a été
//   consommée jusqu'au bout : la queue non lue est jetée en silence, `Ok`.
//
// Deux conséquences que ce groupe met à l'épreuve, et qui vont plus loin que
// ce que le garde de production savait :
//
//   (a) `MERGE` figure dans la branche `parse_query()` (:320-325) — c'est la
//       raison exacte, et la seule, pour laquelle `MERGE … SET` est durable.
//       Le contournement de production est fondé, mais par accident d'aiguillage.
//   (b) Le garde ne détecte que `CREATE (` (`obrain_db.rs:694`). L'orthographe
//       GQL du même motif, `INSERT (…) SET …`, passe le garde sans être vue et
//       perd ses propriétés exactement de la même manière.
// ===========================================================================

/// INTENTION : `INSERT (n:T {...}) SET n.x = 1` écrit `x` sur le nœud créé.
/// Après commit, `n.x` vaut 1.
///
/// La clause `SET` s'applique à la variable liée par l'`INSERT` qui la précède ;
/// c'est le sens de la liaison. Rien dans GQL ne permet à une clause d'écriture
/// d'être acceptée sans erreur puis ignorée.
///
/// C'est le cas central du groupe : celui que le garde de production refuse.
// corrigé par ce commit : INSERT est routé par `parse_query` (les clauses
// suivantes sont enchaînées) et `parse()` refuse toute entrée non consommée.
#[test]
fn insert_puis_set_persiste_la_propriete_apres_commit() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (n:T {id: 'a'}) SET n.x = 1"]);

    assert_eq!(
        scalaire(&db, "MATCH (n:T {id: 'a'}) RETURN n.x AS x"),
        Value::Int64(1),
        "après commit, la propriété assignée par le SET qui suit l'INSERT doit \
         valoir 1 : une clause d'écriture acceptée sans erreur ne peut pas être \
         silencieusement perdue"
    );
}

/// INTENTION : la même écriture laisse la clé `x` sur le nœud.
///
/// Ce test double le précédent SUR L'ÉTAT plutôt que sur la valeur, et ce n'est
/// pas une redite : `n.x` rend `NULL` aussi bien pour une clé absente que pour
/// une clé écrite à NULL (les deux états que `IS NULL` confond). Seul `keys(n)`
/// dit si le SET a laissé une trace quelconque — donc si la perte est totale
/// (la clé n'existe pas) ou partielle (la clé existe, vidée).
// corrigé par ce commit : même cause, même correction (routage d'INSERT par
// `parse_query`).
#[test]
fn insert_puis_set_laisse_la_cle_sur_le_noeud() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (n:T {id: 'a'}) SET n.x = 1"]);

    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["id", "x"]),
        "le nœud doit porter la clé posée par le SET, en plus de celle du motif"
    );
}

/// INTENTION : l'écriture est visible DANS la transaction qui l'a émise, avant
/// commit — comme n'importe quelle écriture (cf.
/// `insert_est_visible_dans_sa_propre_transaction`).
///
/// PORTÉE RÉELLE de ce test : il ne sert pas à constater une écriture, il sert
/// à LOCALISER la perte. Le garde de production décrit un symptôme précis —
/// « properties even visible in the immediate response, but never persisted to
/// disk » (`obrain-tmn/src/obrain_db.rs:1164`) — c'est-à-dire une écriture qui
/// a lieu puis se perd au commit. Si ce test est vert et que son jumeau
/// « après commit » est rouge, ce diagnostic est le bon. S'il est ROUGE, le SET
/// n'a jamais été appliqué et la perte se joue AVANT l'exécution.
///
/// Il est rouge. Le diagnostic écrit dans le serveur de production est donc
/// inexact sur cette version : rien n'est écrit, à aucun moment. C'est
/// cohérent avec la cause trouvée — la clause n'est même pas ANALYSÉE, donc
/// jamais planifiée, donc jamais exécutée. Ne remplacez pas ce test par le
/// seul cas « après commit » : c'est lui qui écarte l'hypothèse du commit
/// fautif, et donc qui désigne le parseur.
// corrigé par ce commit : la clause est analysée, donc visible dans sa propre
// transaction. (Le diagnostic du garde obrain-tmn/src/obrain_db.rs:1164 reste
// infirmé : la perte se jouait AVANT l'exécution, dans le parseur.)
#[test]
fn insert_puis_set_est_visible_avant_le_commit() {
    let db = ObrainDB::new_in_memory();
    let mut session = db.session();
    session.begin_transaction().unwrap();
    session
        .execute("INSERT (n:T {id: 'a'}) SET n.x = 1")
        .unwrap();

    let r = session
        .execute("MATCH (n:T {id: 'a'}) RETURN n.x AS x")
        .unwrap();
    assert_eq!(
        r.rows.len(),
        1,
        "le nœud créé est visible dans sa transaction"
    );
    assert_eq!(
        r.rows[0][0],
        Value::Int64(1),
        "dans sa propre transaction, la valeur assignée par SET doit être lue"
    );

    session.commit().unwrap();
}

/// INTENTION : `SET n += {…}` après un `INSERT` fusionne la carte dans le nœud
/// créé. La forme cartographique du SET n'a pas de privilège sur la forme
/// scalaire : si l'une est perdue, l'autre doit l'être aussi, et si l'une
/// tient, l'autre aussi.
///
/// Ce test décide entre « le SET n'est pas appliqué » (défaut de liaison, donc
/// les deux formes tombent) et « seule l'assignation scalaire est perdue »
/// (défaut de traduction, donc une seule tombe).
// corrigé par ce commit : même cause, même correction — les deux formes (carte
// et scalaire) sont enchaînées par le même chemin.
#[test]
fn insert_puis_set_carte_fusionne_persiste_apres_commit() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (n:T {id: 'a'}) SET n += {x: 1, y: 2}"]);

    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["id", "x", "y"]),
        "SET n += {{…}} après INSERT doit ajouter les deux clés de la carte"
    );
}

/// INTENTION : `MERGE (n:T {id}) SET n.x = 1` persiste — c'est le motif de
/// contournement que le garde de production RECOMMANDE (« Use MERGE instead »,
/// `obrain-tmn/src/obrain_db.rs:1165`) et qu'il décrit comme « confirmed
/// durable ».
///
/// Ce test est le TÉMOIN du groupe. Sans lui, un `INSERT … SET` rouge ne dirait
/// rien : peut-être aucun `SET` après une clause d'écriture ne fonctionne. S'il
/// est vert alors que son jumeau `INSERT` est rouge, la faute est propre au
/// chemin de création, et le contournement de production est fondé.
#[test]
fn merge_puis_set_persiste_la_propriete_apres_commit() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["MERGE (n:T {id: 'a'}) SET n.x = 1"]);

    assert_eq!(
        scalaire(&db, "MATCH (n:T {id: 'a'}) RETURN n.x AS x"),
        Value::Int64(1),
        "le motif recommandé par le garde de production (MERGE + SET) doit être \
         durable — c'est la prémisse de tous les appelants de ce dépôt"
    );
    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["id", "x"]),
        "MERGE + SET laisse la clé du SET sur le nœud"
    );
}

/// INTENTION : `MERGE (n:T {id}) SET n.x = …` sur un nœud DÉJÀ existant écrit
/// aussi — le motif de contournement doit tenir dans les deux branches de
/// MERGE, création comme appariement, sans quoi il ne protège qu'à moitié.
#[test]
fn merge_puis_set_ecrit_aussi_sur_un_noeud_deja_present() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a', x: 0})"]);
    ecrire(&db, &["MERGE (n:T {id: 'a'}) SET n.x = 5"]);

    assert_eq!(
        compte(&db, "T"),
        1,
        "le MERGE apparie le nœud existant au lieu d'en créer un second"
    );
    assert_eq!(
        scalaire(&db, "MATCH (n:T {id: 'a'}) RETURN n.x AS x"),
        Value::Int64(5),
        "sur la branche APPARIEMENT aussi, le SET qui suit le MERGE doit écrire"
    );
}

/// INTENTION : le même `INSERT (…) SET …`, cette fois PRÉCÉDÉ d'un `MATCH`,
/// écrit la propriété. Le nœud d'ancrage ne sert qu'à faire commencer la
/// requête par `MATCH`.
///
/// PORTÉE RÉELLE — c'est le test qui DÉSIGNE la cause. Le motif d'écriture est
/// identique au cas central, mot pour mot ; seul le PREMIER JETON de la
/// requête change. Si celui-ci est vert alors que le cas central est rouge,
/// alors le défaut n'est ni dans `SET`, ni dans `INSERT`, ni dans leur
/// composition : il est dans l'AIGUILLAGE de la requête d'après son premier
/// jeton (`obrain-adapters/src/query/gql/parser.rs:326`). Aucun autre couple
/// de tests ne peut réduire l'espace des causes aussi étroitement.
#[test]
fn insert_puis_set_precede_d_un_match_ecrit_bien_la_propriete() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:Ancre {i: 1})"]);
    ecrire(&db, &["MATCH (a:Ancre) INSERT (n:T {id: 'a'}) SET n.x = 1"]);

    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["id", "x"]),
        "précédé d'un MATCH, le même INSERT + SET écrit bien la clé : seul le \
         premier jeton de la requête a changé"
    );
    assert_eq!(
        scalaire(&db, "MATCH (n:T {id: 'a'}) RETURN n.x AS x"),
        Value::Int64(1),
        "et il écrit bien la VALEUR, pas seulement la clé"
    );
}

/// INTENTION : `INSERT (n:T {…}) RETURN n.id` rend la projection demandée —
/// une colonne `i` portant `'a'`.
///
/// Ce test n'interroge pas l'état du graphe mais il appartient au groupe :
/// c'est la MÊME queue de requête jetée par la MÊME cause. Un appelant qui
/// écrit `INSERT … RETURN n.id` pour récupérer la clé du nœud qu'il vient de
/// créer reçoit autre chose que ce qu'il a demandé, sans erreur — la faute est
/// la même, mais elle se paie ici en lecture au lieu de se payer en écriture.
// corrigé par ce commit : le RETURN qui suit un INSERT est analysé et la
// projection demandée est rendue.
#[test]
fn insert_puis_return_rend_la_projection_demandee() {
    let db = ObrainDB::new_in_memory();
    let mut session = db.session();
    session.begin_transaction().unwrap();
    let r = session
        .execute("INSERT (n:T {id: 'a'}) RETURN n.id AS i")
        .unwrap();
    session.commit().unwrap();

    assert_eq!(
        r.columns,
        vec!["i".to_string()],
        "la projection demandée nomme sa colonne `i`"
    );
    assert_eq!(r.rows.len(), 1, "un nœud créé, une ligne projetée");
    assert_eq!(
        r.rows[0][0],
        Value::String("a".into()),
        "et la ligne porte la valeur projetée, pas le nœud entier"
    );
}

/// INTENTION : une requête dont la fin est du charabia est REFUSÉE. Le moteur
/// doit rendre une erreur, pas exécuter la moitié qu'il a comprise.
///
/// PORTÉE RÉELLE : ce test ne parle pas d'écriture, il parle de la GARANTIE qui
/// rend toute écriture vérifiable. Un parseur qui jette sa queue en silence ne
/// perd pas seulement les `SET` : il rend indistinguables « la requête a fait
/// ce que j'ai écrit » et « la requête a fait ce que le parseur a bien voulu
/// lire ». C'est la faute STRUCTURELLE dont les quatre pertes de propriétés du
/// groupe ne sont que des cas particuliers — et la seule dont la correction les
/// convertirait toutes en erreurs franches plutôt qu'en pertes muettes.
// corrigé par ce commit : `parse()` refuse toute entrée non consommée — c'est
// la fermeture de CLASSE, qui couvre tous les aiguillages et convertit chaque
// perte muette en erreur franche. (Seuls les `;` terminaux restent tolérés.)
#[test]
fn une_queue_de_requete_incomprise_doit_etre_refusee() {
    let db = ObrainDB::new_in_memory();
    let mut session = db.session();
    session.begin_transaction().unwrap();
    let r = session.execute("INSERT (n:T {id: 'a'}) CECI EST DU CHARABIA");
    let refusee = r.is_err();
    session.rollback().unwrap();

    assert!(
        refusee,
        "une requête dont la fin n'a pas été analysée doit être REFUSÉE : \
         l'accepter rend toute écriture invérifiable, puisque `ok` ne dit plus \
         que la requête émise est celle qui a été exécutée"
    );
}

// ===========================================================================
// GROUPE 3 — MERGE : L'IDEMPOTENCE, ET CE QU'ELLE NE COUVRE PAS
//
// `MERGE` est le seul verbe d'écriture dont le contrat porte sur l'état
// RÉSULTANT plutôt que sur l'action : après `MERGE (n:T {id:'a'})`, il existe
// EXACTEMENT un nœud T d'identifiant `a` — que la requête ait créé ou apparié.
// C'est ce contrat que ce dépôt fait porter à tous ses appelants
// (`obrain-tmn/src/obrain_db.rs:1165`), donc c'est lui qu'il faut vérifier.
//
// Une idempotence ne se teste jamais sur une seule exécution : on exécute deux
// fois et on compte. Et on compte le TOTAL, pas les lignes d'un motif — un
// second nœud jumeau resterait invisible à un MATCH qui rend la première ligne
// venue.
// ===========================================================================

/// INTENTION : deux `MERGE` du même motif dans la MÊME transaction laissent un
/// seul nœud. Le second doit voir ce que le premier a créé.
///
/// C'est le cas le plus exigeant de l'idempotence : il oblige `MERGE` à lire
/// les écritures non encore commitées de sa propre transaction. Un moteur qui
/// n'apparie que ce qui est au disque crée un doublon ici et nulle part
/// ailleurs.
///
/// PORTÉE RÉELLE, MESURÉE — à lire avant de s'appuyer sur ce test pour
/// modifier `MergeOperator`. Neutraliser l'appariement
/// (`obrain-core/src/execution/operators/merge.rs:233`, `find_matching_node`
/// forcé à ne rien trouver) NE REND PAS ce test rouge : il est resté vert, et
/// avec l'assertion sur le balayage comme avec celle sur le compte. La raison
/// est dans le code : `merge_node_for_row` porte une SECONDE garantie
/// d'unicité, indépendante de la première — après avoir créé, elle rebalaie
/// les porteurs du motif, et si un porteur de plus petit `NodeId` existe elle
/// EFFACE le nœud qu'elle vient de créer et adopte le gagnant (ibid.
/// :263-284, règle de course ajoutée en 2026-08-17 contre les doublons de
/// tempête de conflits). L'invariant observable est donc défendu deux fois.
///
/// Neutraliser les DEUX ensemble tue bien le test (`["a", "a"]` au lieu de
/// `["a"]`). Ce qu'il mesure est donc l'INVARIANT — « il reste un seul
/// porteur » — et non le mécanisme qui le produit : c'est bien ce qu'on veut
/// d'un test de conformité, mais cela signifie qu'il ne protège PAS
/// `find_matching_node` pris isolément. Un correctif qui casserait
/// l'appariement en laissant la règle de course debout passerait ici, et se
/// paierait en écritures inutiles — une création puis une suppression à chaque
/// MERGE — sans qu'aucune assertion de ce fichier ne s'en aperçoive.
#[test]
fn merge_repete_dans_une_transaction_laisse_un_seul_noeud() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["MERGE (n:T {id: 'a'})", "MERGE (n:T {id: 'a'})"]);

    assert_eq!(
        identifiants(&db, "T"),
        vec!["a".to_string()],
        "le second MERGE doit apparier le nœud que le premier vient de créer, \
         non encore commité : le balayage ne doit rendre qu'une ligne"
    );
    assert_eq!(
        compte(&db, "T"),
        1,
        "et le compte doit s'accorder avec le balayage"
    );
}

/// INTENTION : deux `MERGE` du même motif dans DEUX transactions successives
/// laissent un seul nœud.
///
/// Même portée réelle que son jumeau intra-transaction, et mesurée de la même
/// façon : l'invariant est défendu par deux mécanismes indépendants
/// (`merge.rs:233` et la règle de course `:263-284`), et ce test ne distingue
/// pas lequel travaille.
#[test]
fn merge_repete_en_deux_transactions_laisse_un_seul_noeud() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["MERGE (n:T {id: 'a'})"]);
    ecrire(&db, &["MERGE (n:T {id: 'a'})"]);

    assert_eq!(
        identifiants(&db, "T"),
        vec!["a".to_string()],
        "MERGE est idempotent à travers le commit : une seule ligne au balayage"
    );
    assert_eq!(
        compte(&db, "T"),
        1,
        "et le compte s'accorde avec le balayage"
    );
}

/// INTENTION : un `MERGE` qui apparie ne touche à RIEN d'autre. Les propriétés
/// que le nœud portait déjà et qui ne figurent pas dans le motif survivent
/// intactes.
///
/// L'assertion porte sur la carte COMPLÈTE des propriétés, pas sur une lecture
/// par clé : c'est la seule façon de voir un MERGE qui écraserait le nœud par
/// le motif au lieu de l'apparier.
#[test]
fn merge_qui_apparie_ne_touche_pas_aux_autres_proprietes() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a', garde: 'intact', n: 7})"]);
    ecrire(&db, &["MERGE (n:T {id: 'a'})"]);

    let p = proprietes(&db, "(n:T {id: 'a'})");
    assert_eq!(p.len(), 3, "aucune propriété n'a été perdue, obtenu {p:?}");
    assert_eq!(p.get("garde"), Some(&Value::String("intact".into())));
    assert_eq!(p.get("n"), Some(&Value::Int64(7)));
    assert_eq!(p.get("id"), Some(&Value::String("a".into())));
}

/// INTENTION : un `MERGE` dont le motif diffère d'UNE propriété crée un SECOND
/// nœud. L'appariement porte sur le motif entier, pas sur l'étiquette.
///
/// C'est la contrepartie nécessaire de l'idempotence : sans elle, `MERGE`
/// apparierait trop et écraserait des nœuds distincts.
#[test]
fn merge_d_un_motif_different_cree_un_second_noeud() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["MERGE (n:T {id: 'a'})"]);
    ecrire(&db, &["MERGE (n:T {id: 'b'})"]);

    assert_eq!(
        identifiants(&db, "T"),
        vec!["a".to_string(), "b".to_string()],
        "deux identifiants distincts, deux nœuds : MERGE apparie sur le motif \
         ENTIER, pas sur la seule étiquette"
    );
    assert_eq!(
        compte(&db, "T"),
        2,
        "et le compte s'accorde avec le balayage"
    );
}

/// INTENTION : `ON CREATE SET` s'applique à la seule branche CRÉATION, et
/// `ON MATCH SET` à la seule branche APPARIEMENT.
///
/// Les deux moitiés sont vérifiées par le MÊME nœud, à deux instants : après la
/// première exécution il porte la marque de création et pas celle
/// d'appariement ; après la seconde il porte les deux. Deux nœuds séparés
/// n'établiraient pas que la branche non prise a été ÉCARTÉE — seulement
/// qu'elle n'a pas écrit là où on regardait.
#[test]
fn merge_on_create_et_on_match_ecrivent_chacun_sur_sa_branche() {
    let db = ObrainDB::new_in_memory();
    let requete = "MERGE (n:T {id: 'a'}) ON CREATE SET n.cree = 1 ON MATCH SET n.apparie = 1";

    ecrire(&db, &[requete]);
    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["cree", "id"]),
        "à la CRÉATION, seule la branche ON CREATE écrit : la marque \
         d'appariement ne doit pas exister"
    );

    ecrire(&db, &[requete]);
    assert_eq!(
        compte(&db, "T"),
        1,
        "la seconde exécution apparie au lieu de créer"
    );
    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["apparie", "cree", "id"]),
        "à l'APPARIEMENT, la branche ON MATCH écrit et la marque de création \
         d'origine est conservée"
    );
    assert_eq!(
        scalaire(&db, "MATCH (n:T {id: 'a'}) RETURN n.cree AS v"),
        Value::Int64(1),
        "ON CREATE n'est pas rejoué à l'appariement — la valeur d'origine tient"
    );
}

/// INTENTION : `MERGE` d'une arête entre deux nœuds appariés ne crée qu'UNE
/// arête, même exécuté deux fois.
///
/// Le compte porte sur les arêtes rendues par le motif : deux arêtes parallèles
/// donneraient deux lignes, une seule en donne une. C'est ici l'observation la
/// plus fine disponible — le langage n'offre pas d'identité d'arête à comparer.
#[test]
fn merge_d_une_arete_repete_ne_laisse_qu_une_arete() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:A {id: 'g'})", "INSERT (:B {id: 'd'})"]);
    let requete = "MATCH (a:A {id: 'g'}), (b:B {id: 'd'}) MERGE (a)-[:VERS]->(b)";
    ecrire(&db, &[requete]);
    ecrire(&db, &[requete]);

    let r = lire(
        &db,
        "MATCH (a:A)-[:VERS]->(b:B) RETURN a.id AS g, b.id AS d",
    );
    assert_eq!(
        r.len(),
        1,
        "MERGE d'arête est idempotent : deux exécutions, une seule arête"
    );
    assert_eq!(
        compte(&db, "A"),
        1,
        "et aucun nœud n'a été dupliqué au passage"
    );
    assert_eq!(compte(&db, "B"), 1);
}

/// INTENTION : après un `DETACH DELETE`, un `MERGE` du même motif CRÉE un
/// nouveau nœud — il n'en ressuscite pas un.
///
/// C'est le point de contact entre `MERGE` et les pierres tombales : un nœud
/// supprimé laisse une trace en stockage, et l'appariement ne doit jamais la
/// voir. Le test vérifie à la fois le compte (un seul nœud) ET la forme (le
/// nœud recréé ne porte QUE les clés du nouveau motif) — un nœud ressuscité
/// traînerait les clés de l'ancien.
#[test]
fn merge_apres_suppression_cree_un_noeud_neuf_sans_trace_de_l_ancien() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a', ancienne: 'trace'})"]);
    ecrire(&db, &["MATCH (n:T {id: 'a'}) DETACH DELETE n"]);
    ecrire(&db, &["MERGE (n:T {id: 'a'})"]);

    assert_eq!(
        compte(&db, "T"),
        1,
        "exactement un nœud T après la recréation"
    );
    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["id"]),
        "le nœud recréé ne porte QUE la clé du motif : aucune trace de la \
         propriété que portait le nœud supprimé"
    );
}

// ===========================================================================
// GROUPE 4 — SET : LA VALEUR ÉCRITE, ET LA FORME QU'ELLE LAISSE
//
// Deux formes de `SET` coexistent et n'ont pas le même contrat :
//
//   `SET n.x = …` et `SET n += {…}` AJOUTENT ou remplacent des clés nommées ;
//   `SET n = {…}` REMPLACE le nœud par la carte — les clés absentes de la
//   carte doivent DISPARAÎTRE, pas être vidées.
//
// La seconde forme est le seul chemin par lequel GQL atteint aujourd'hui
// `remove_node_property` (`obrain-core/src/execution/operators/mutation.rs:1416-1453`).
// C'est donc le seul endroit où l'on peut établir que le moteur SAIT retirer
// une clé — fait dont le GROUPE 5 tire toutes ses conséquences.
// ===========================================================================

/// INTENTION : `SET n.x = …` ajoute la clé `x` et n'y touche à rien d'autre.
#[test]
fn set_scalaire_ajoute_la_cle_sans_toucher_aux_autres() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a', garde: 'intact'})"]);
    ecrire(&db, &["MATCH (n:T {id: 'a'}) SET n.x = 1"]);

    assert_eq!(cles(&db, "(n:T {id: 'a'})"), ens(&["garde", "id", "x"]));
    let p = proprietes(&db, "(n:T {id: 'a'})");
    assert_eq!(p.get("x"), Some(&Value::Int64(1)));
    assert_eq!(
        p.get("garde"),
        Some(&Value::String("intact".into())),
        "une écriture nommée ne touche pas aux clés qu'elle ne nomme pas"
    );
}

/// INTENTION : `SET n.x = …` sur une clé DÉJÀ portée remplace la valeur sans
/// dupliquer la clé.
#[test]
fn set_scalaire_sur_cle_existante_remplace_sans_dupliquer() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a', x: 1})"]);
    ecrire(&db, &["MATCH (n:T {id: 'a'}) SET n.x = 2"]);

    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["id", "x"]),
        "la clé n'apparaît qu'une fois"
    );
    assert_eq!(
        scalaire(&db, "MATCH (n:T {id: 'a'}) RETURN n.x AS x"),
        Value::Int64(2)
    );
}

/// INTENTION : la valeur d'un `SET` peut être une EXPRESSION lue sur le nœud
/// lui-même, y compris sur la propriété qu'on écrit.
///
/// `SET n.x = n.x + 1` est le cas qui distingue une écriture qui lit son état
/// AVANT d'écrire d'une écriture qui lirait après : la valeur attendue est 2,
/// jamais 1 (lecture après remise à zéro) ni 3 (double application).
///
/// HISTORIQUE (corrigé, vague 4a 2026-08-30) : le moteur REFUSAIT la requête
/// avec une erreur `Internal` — `PropertySource` ne savait porter qu'une
/// constante, une colonne ou un accès de propriété nu, et toute expression
/// composée lisant la ligne tombait dans le cas par défaut de
/// `expression_to_property_source` (planner/lpg/mutation.rs). Corrigé par la
/// variante `PropertySource::Expression` : le même évaluateur que les filtres
/// et projections, résolu par ligne AVANT la première écriture de la ligne.
#[test]
fn set_par_expression_lit_l_etat_d_avant_l_ecriture() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a', x: 1})"]);
    ecrire(&db, &["MATCH (n:T {id: 'a'}) SET n.x = n.x + 1"]);

    assert_eq!(
        scalaire(&db, "MATCH (n:T {id: 'a'}) RETURN n.x AS x"),
        Value::Int64(2),
        "l'expression est évaluée sur l'état d'AVANT l'écriture"
    );
}

/// INTENTION : `SET n += {…}` fusionne — les clés de la carte s'ajoutent ou
/// remplacent, celles du nœud absentes de la carte survivent.
#[test]
fn set_carte_fusionnee_ajoute_et_conserve() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a', garde: 'intact', x: 1})"]);
    ecrire(&db, &["MATCH (n:T {id: 'a'}) SET n += {x: 2, y: 3}"]);

    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["garde", "id", "x", "y"]),
        "fusion : la clé absente de la carte survit, la nouvelle est ajoutée"
    );
    let p = proprietes(&db, "(n:T {id: 'a'})");
    assert_eq!(
        p.get("x"),
        Some(&Value::Int64(2)),
        "la clé commune est écrasée"
    );
    assert_eq!(p.get("y"), Some(&Value::Int64(3)));
    assert_eq!(p.get("garde"), Some(&Value::String("intact".into())));
}

/// INTENTION : `SET n = {…}` REMPLACE. Les clés que le nœud portait et qui ne
/// figurent pas dans la carte doivent AVOIR DISPARU de `keys(n)` — pas y
/// figurer avec un NULL.
///
/// C'est le test pivot du fichier. Il établit que le moteur DISPOSE d'un chemin
/// qui retire réellement une clé (`remove_node_property`, appelé depuis
/// `obrain-core/src/execution/operators/mutation.rs:1416-1453`). Tout le
/// GROUPE 5 en dépend : si `SET n = {…}` retire vraiment, alors l'incapacité de
/// `REMOVE` à retirer n'est pas une limite du stockage mais un choix de
/// traduction — donc un défaut réparable, et non une propriété du moteur.
#[test]
fn set_carte_remplacee_fait_disparaitre_les_cles_absentes() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a', vieux: 1, autre: 2})"]);
    ecrire(&db, &["MATCH (n:T {id: 'a'}) SET n = {id: 'a', neuf: 3}"]);

    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["id", "neuf"]),
        "en remplacement, les clés absentes de la carte ne sont pas vidées : \
         elles DISPARAISSENT du nœud"
    );
    let p = proprietes(&db, "(n:T {id: 'a'})");
    assert_eq!(
        p.len(),
        2,
        "properties() ne rend que les deux clés, obtenu {p:?}"
    );
    assert_eq!(p.get("neuf"), Some(&Value::Int64(3)));
}

/// INTENTION : `SET n = {}` vide entièrement le nœud. `keys(n)` rend la liste
/// VIDE, et le nœud subsiste — remplacer par rien n'est pas supprimer.
///
/// Le cas dégénéré du remplacement, et le plus révélateur : il n'y a aucune clé
/// à conserver, donc rien qui puisse masquer une clé survivante.
#[test]
fn set_carte_vide_ne_laisse_aucune_cle_mais_laisse_le_noeud() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a', x: 1, y: 2})"]);
    ecrire(&db, &["MATCH (n:T {id: 'a'}) SET n = {}"]);

    assert_eq!(
        compte(&db, "T"),
        1,
        "le nœud subsiste : vider n'est pas supprimer"
    );
    let vide: Vec<String> = Vec::new();
    assert_eq!(
        en_ensemble(&scalaire(&db, "MATCH (n:T) RETURN keys(n) AS c")),
        vide,
        "après remplacement par la carte vide, le nœud ne porte plus AUCUNE clé"
    );
}

/// INTENTION : `SET n.x = null` retire la clé `x`, comme le fait le
/// remplacement pour une clé absente de sa carte.
///
/// CHOIX SÉMANTIQUE, écrit ici parce qu'il est discutable et doit rester
/// réversible : GQL ne tranche pas explicitement, mais les deux règles que le
/// moteur applique DÉJÀ le déterminent ensemble. `SET n = {id:'a'}` fait
/// disparaître une clé `x` préexistante ; si `SET n.x = null` la laissait avec
/// un NULL, alors deux écritures qui déclarent la même chose — « x n'a pas de
/// valeur » — laisseraient deux états différents, et le nœud garderait la trace
/// de la ROUTE prise plutôt que de l'état déclaré. C'est la lecture de Cypher,
/// et c'est la seule qui rend `SET` cohérent avec lui-même.
///
/// Si le projet retenait l'autre lecture (assigner NULL crée une clé nulle), ce
/// test devrait être RÉÉCRIT en énonçant ce choix — et alors `SET n = {…}`
/// devrait vider les clés absentes plutôt que les retirer, sans quoi
/// l'incohérence ne fait que se déplacer.
///
/// [FAIT — avec le correctif, le test nommé ci-dessous a été DURCI : il
/// vérifie désormais par `keys(n)` le retrait que son nom annonce.]
/// À SIGNALER, ET À NE PAS CORRIGER DEPUIS ICI — un test existant porte déjà ce
/// contrat DANS SON NOM sans le vérifier :
/// `set_property_to_null_removes_it`
/// (`crates/obrain-engine/tests/seam_dml_interactions.rs:217`). Il affirme le
/// retrait par son nom, puis n'assertionne que `n.age` lu à `Value::Null` — et
/// son propre message se replie sur une formulation plus faible que son nom
/// (« Property set to NULL should read as NULL »). L'assertion est satisfaite
/// par le défaut comme par le correctif. Ce n'est pas un défaut délibérément
/// figé — il n'y a pas d'en-tête revendiquant une parité —, mais l'effet
/// pratique est le même : un lecteur qui cherche « le retrait par NULL est-il
/// couvert ? » trouve un test vert qui porte ce nom et passe son chemin. Le
/// présent test est celui qui vérifie ce que l'autre annonce.
// corrigé par ce commit (remove réel : retrait en stockage + enregistrement WAL Remove*Property ; SET/INSERT à null ne matérialisent plus la clé)
#[test]
fn set_a_null_retire_la_cle_comme_le_remplacement() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a', x: 1})"]);
    ecrire(&db, &["MATCH (n:T {id: 'a'}) SET n.x = null"]);

    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["id"]),
        "assigner NULL déclare que la propriété n'a pas de valeur ; le nœud \
         doit alors avoir la MÊME forme que si on l'avait remplacé par une \
         carte sans cette clé"
    );
}

/// INTENTION : un `SET` dont le `MATCH` ne rend aucune ligne n'écrit RIEN et
/// ne crée RIEN.
///
/// L'assertion porte sur le compte de nœuds, toutes étiquettes visées : un
/// `SET` qui créerait le nœud absent (sémantique d'« upsert » que GQL ne donne
/// qu'à `MERGE`) serait invisible à une assertion qui se contenterait de lire
/// une propriété.
#[test]
fn set_sans_ligne_appariee_ne_cree_aucun_noeud() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["MATCH (n:T {id: 'absent'}) SET n.x = 1"]);

    assert_eq!(
        compte(&db, "T"),
        0,
        "SET n'est pas un upsert : sans ligne appariée, il n'écrit ni ne crée"
    );
}

/// INTENTION : une clause `SET` posant deux propriétés les pose TOUTES LES
/// DEUX.
#[test]
fn set_de_deux_proprietes_les_pose_toutes_les_deux() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a'})"]);
    ecrire(&db, &["MATCH (n:T {id: 'a'}) SET n.x = 1, n.y = 2"]);

    assert_eq!(cles(&db, "(n:T {id: 'a'})"), ens(&["id", "x", "y"]));
    let p = proprietes(&db, "(n:T {id: 'a'})");
    assert_eq!(p.get("x"), Some(&Value::Int64(1)));
    assert_eq!(p.get("y"), Some(&Value::Int64(2)));
}

/// INTENTION : un `SET` annulé par `rollback` ne laisse RIEN — ni la valeur
/// nouvelle, ni la clé.
///
/// L'assertion porte sur l'ensemble des clés autant que sur la valeur : une
/// transaction annulée pourrait restaurer la valeur d'origine tout en laissant
/// derrière elle la clé qu'elle a créée.
#[test]
fn set_annule_ne_laisse_ni_la_valeur_ni_la_cle() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a', x: 1})"]);

    let mut session = db.session();
    session.begin_transaction().unwrap();
    session
        .execute("MATCH (n:T {id: 'a'}) SET n.x = 99, n.neuve = 7")
        .unwrap();
    session.rollback().unwrap();

    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["id", "x"]),
        "la clé créée par la transaction annulée ne doit pas subsister"
    );
    assert_eq!(
        scalaire(&db, "MATCH (n:T {id: 'a'}) RETURN n.x AS x"),
        Value::Int64(1),
        "et la valeur d'origine est restaurée"
    );
}

/// INTENTION : `SET` sur une ARÊTE écrit sur l'arête et non sur ses extrémités.
///
/// L'assertion vérifie les deux côtés : la propriété est sur l'arête, ET les
/// deux nœuds gardent leur forme d'origine. Une écriture qui se serait égarée
/// sur un nœud passerait un test qui ne regarderait que l'arête.
#[test]
fn set_sur_arete_ecrit_sur_l_arete_et_pas_sur_les_noeuds() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:A {id: 'g'})-[:VERS]->(:B {id: 'd'})"]);
    ecrire(&db, &["MATCH (:A)-[r:VERS]->(:B) SET r.poids = 5"]);

    assert_eq!(
        scalaire(&db, "MATCH (:A)-[r:VERS]->(:B) RETURN r.poids AS p"),
        Value::Int64(5),
        "l'arête porte la propriété écrite"
    );
    assert_eq!(
        cles(&db, "(n:A)"),
        ens(&["id"]),
        "le nœud source n'a pas reçu l'écriture destinée à l'arête"
    );
    assert_eq!(cles(&db, "(n:B)"), ens(&["id"]), "le nœud cible non plus");
}

/// INTENTION : `SET n:Etiquette` ajoute l'étiquette au nœud existant, sans
/// toucher à ses propriétés ni en créer un second.
#[test]
fn set_d_etiquette_ajoute_sans_dupliquer_le_noeud() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a', x: 1})"]);
    ecrire(&db, &["MATCH (n:T {id: 'a'}) SET n:Marque"]);

    assert_eq!(compte(&db, "T"), 1, "aucun nœud n'a été dupliqué");
    assert_eq!(
        compte(&db, "Marque"),
        1,
        "le nœud porte désormais la nouvelle étiquette"
    );
    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["id", "x"]),
        "les propriétés sont intactes : une étiquette n'est pas une propriété"
    );
}

// ===========================================================================
// GROUPE 5 — REMOVE : LA CLASSE COMPLÈTE
//
// Le lot 4 a établi le cas isolé : `REMOVE n.x` n'efface pas la clé, il y écrit
// un NULL (`obrain-engine/src/query/translators/gql/mod.rs:781-788` compile
// chaque `property_removals` en `SetPropertyOp{ properties: [(prop,
// Literal(Value::Null))], replace: false }`). Ce groupe traite la CLASSE : ce
// que ce choix de traduction fait au reste du langage.
//
// Ce n'est pas la même question. Qu'un REMOVE laisse une clé nulle est une
// divergence locale ; qu'il en CRÉE une là où il n'y en avait pas, qu'il rende
// un nœud inappariable par MERGE, ou qu'il soit ré-ordonné derrière le SET qui
// le suit dans le texte — ce sont des conséquences que le cas isolé ne montre
// pas et que le correctif devra toutes lever ensemble.
// ===========================================================================

/// INTENTION : `REMOVE n.x` sur une clé que le nœud NE PORTE PAS ne change
/// rien. Le nœud garde exactement la forme qu'il avait.
///
/// C'est le cas le plus révélateur du groupe, et le seul où la faute est
/// visible à l'œil nu : retirer ce qui n'existe pas ne peut pas AJOUTER
/// quelque chose. Une traduction qui écrit un NULL fait grossir le nœud à
/// chaque REMOVE d'une clé absente — un balayage défensif qui « nettoie » un
/// graphe en retirant partout une propriété facultative l'inscrit au contraire
/// sur tous les nœuds qui ne l'avaient pas.
///
/// Le test existant `test_remove_nonexistent_property_no_crash`
/// (`gql_spec_compliance.rs:3075`) couvre la même requête, mais n'assertionne
/// que `result.is_ok()` : il est vert et le restera quel que soit l'état
/// laissé. C'est exactement le genre d'assertion sur l'ABSENCE d'erreur que ce
/// lot remplace par une assertion sur l'ÉTAT.
// corrigé par ce commit (remove réel : retrait en stockage + enregistrement WAL Remove*Property ; SET/INSERT à null ne matérialisent plus la clé)
#[test]
fn remove_d_une_cle_absente_ne_change_rien() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a'})"]);
    ecrire(&db, &["MATCH (n:T {id: 'a'}) REMOVE n.jamais_ecrite"]);

    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["id"]),
        "retirer une clé que le nœud ne porte pas ne peut pas la lui AJOUTER"
    );
}

/// INTENTION : `REMOVE n.x` deux fois laisse le même état qu'une fois. Une
/// suppression est idempotente par nature — l'état visé est « la clé n'est pas
/// là », pas « une clé de moins ».
///
/// Le second `REMOVE` porte ici sur une clé que le premier était censé avoir
/// retirée : ce test compose donc les deux cas précédents et montre qu'ils ne
/// se compensent pas — la clé créée par le REMOVE d'une clé absente est
/// exactement celle que le REMOVE précédent a laissée.
// corrigé par ce commit (remove réel : retrait en stockage + enregistrement WAL Remove*Property ; SET/INSERT à null ne matérialisent plus la clé)
#[test]
fn remove_repete_laisse_le_meme_etat_qu_une_seule_fois() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a', x: 1})"]);
    ecrire(
        &db,
        &[
            "MATCH (n:T {id: 'a'}) REMOVE n.x",
            "MATCH (n:T {id: 'a'}) REMOVE n.x",
        ],
    );

    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["id"]),
        "deux REMOVE valent un : l'état visé est « la clé n'est pas là »"
    );
}

/// INTENTION : `REMOVE` puis `SET` rend la valeur, et le nœud porte la clé UNE
/// fois.
///
/// C'est le cycle qu'un appelant écrit sans y penser (effacer puis réécrire).
/// Il doit ramener exactement à l'état de départ.
#[test]
fn remove_puis_set_ramene_la_valeur_et_une_seule_cle() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a', x: 1})"]);
    ecrire(&db, &["MATCH (n:T {id: 'a'}) REMOVE n.x"]);
    ecrire(&db, &["MATCH (n:T {id: 'a'}) SET n.x = 2"]);

    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["id", "x"]),
        "après le cycle effacer-réécrire, la clé figure une seule fois"
    );
    assert_eq!(
        scalaire(&db, "MATCH (n:T {id: 'a'}) RETURN n.x AS x"),
        Value::Int64(2),
        "et elle porte la valeur réécrite"
    );
}

/// INTENTION : après `REMOVE n.x`, un `MERGE (n:T {id:'a', x:1})` ne peut pas
/// apparier ce nœud — il n'a pas de `x` — donc il en CRÉE un second.
///
/// C'est la conséquence la plus coûteuse du défaut, et celle qu'aucun test sur
/// `keys` ne montre : la forme du nœud décide de l'APPARIEMENT. Que le nœud
/// porte une clé nulle ou pas de clé du tout ne change rien à ce cas précis
/// (`null ≠ 1` dans les deux lectures), donc ce test doit être VERT — et sa
/// valeur est là : il borne le défaut. Le REMOVE fautif ne corrompt PAS
/// l'appariement de MERGE ; le dégât reste confiné à l'introspection et au
/// nettoyage.
///
/// PORTÉE RÉELLE : ce test est vert avec ET sans le défaut. Il ne le détecte
/// pas — il établit ce que le défaut n'atteint pas, ce qui est nécessaire pour
/// borner un correctif.
#[test]
fn merge_n_apparie_pas_un_noeud_dont_la_propriete_a_ete_retiree() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a', x: 1})"]);
    ecrire(&db, &["MATCH (n:T {id: 'a'}) REMOVE n.x"]);
    ecrire(&db, &["MERGE (n:T {id: 'a', x: 1})"]);

    assert_eq!(
        compte(&db, "T"),
        2,
        "le nœud dont x a été retiré ne satisfait plus le motif {{id:'a', x:1}} : \
         MERGE doit créer, pas apparier"
    );
}

/// INTENTION : `properties(n)` après `REMOVE n.x` ne contient pas la clé `x`.
///
/// Complément nécessaire du cas `keys` posé par le lot 4 : `keys` et
/// `properties` doivent s'accorder sur la forme du nœud. Un correctif qui ne
/// réparerait que `keys` laisserait `properties` rendre `{"x": null}` et le
/// désaccord serait pire que le défaut d'origine — deux introspections qui se
/// contredisent.
// corrigé par ce commit (remove réel : retrait en stockage + enregistrement WAL Remove*Property ; SET/INSERT à null ne matérialisent plus la clé)
#[test]
fn properties_ne_rend_pas_la_cle_retiree() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a', x: 1})"]);
    ecrire(&db, &["MATCH (n:T {id: 'a'}) REMOVE n.x"]);

    let p = proprietes(&db, "(n:T {id: 'a'})");
    assert!(
        !p.contains_key("x"),
        "après REMOVE, properties() ne doit plus mentionner la clé, obtenu {p:?}"
    );
    assert_eq!(p.len(), 1, "il ne reste que la clé id, obtenu {p:?}");
}

/// INTENTION : après `REMOVE n.x`, `n.x IS NULL` est VRAI.
///
/// PORTÉE RÉELLE — à lire avant de croire ce test utile comme détecteur : il
/// est vert dans les DEUX lectures, la correcte comme la fautive. Une clé
/// absente est nulle ; une clé écrite à NULL l'est aussi. `IS NULL` ne
/// distingue pas les trois états du stockage (cf. l'en-tête du fichier) et ne
/// le doit pas.
///
/// Ce qu'il établit est donc négatif, et c'est ce qui le rend nécessaire : il
/// ferme la porte à un correctif qui « réparerait » `REMOVE` en rendant
/// `n.x IS NULL` faux après un retrait. La valeur d'une propriété absente est
/// NULL, avant comme après le correctif ; c'est la FORME qui doit changer, pas
/// l'algèbre des valeurs.
#[test]
fn is_null_reste_vrai_apres_remove_et_doit_le_rester() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a', x: 1})"]);
    ecrire(&db, &["MATCH (n:T {id: 'a'}) REMOVE n.x"]);

    assert_eq!(
        scalaire(&db, "MATCH (n:T {id: 'a'}) RETURN n.x IS NULL AS v"),
        Value::Bool(true),
        "la valeur d'une propriété retirée est NULL — le correctif de REMOVE ne \
         doit PAS toucher à cela, seulement à la forme du nœud"
    );
}

/// INTENTION : `SET n = {…}` en remplacement nettoie l'état laissé par un
/// `REMOVE`. Après remplacement, le nœud a la forme de la carte, quoi qu'il
/// ait porté avant.
///
/// C'est le seul recours dont dispose aujourd'hui un appelant qui doit
/// réellement retirer une clé, et il faut donc qu'il fonctionne MÊME sur un
/// nœud déjà passé par un REMOVE fautif — c'est-à-dire que le chemin de
/// remplacement doit retirer aussi les clés à valeur nulle, et pas seulement
/// celles qui portent une valeur.
#[test]
fn le_remplacement_nettoie_l_etat_laisse_par_un_remove() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a', x: 1})"]);
    ecrire(&db, &["MATCH (n:T {id: 'a'}) REMOVE n.x"]);
    ecrire(&db, &["MATCH (n:T {id: 'a'}) SET n = {id: 'a'}"]);

    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["id"]),
        "le remplacement retire aussi les clés à valeur nulle : c'est le seul \
         recours d'un appelant qui doit vraiment effacer une propriété"
    );
}

/// INTENTION : `REMOVE r.p` sur une ARÊTE retire la clé de l'arête.
///
/// La traduction de `REMOVE` est commune aux nœuds et aux arêtes ; si le défaut
/// vient bien de la traduction, il doit se manifester des deux côtés. Un défaut
/// qui ne toucherait que les nœuds désignerait le stockage, pas le traducteur.
// corrigé par ce commit (remove réel : retrait en stockage + enregistrement WAL Remove*Property ; SET/INSERT à null ne matérialisent plus la clé)
#[test]
fn remove_sur_arete_retire_la_cle_de_l_arete() {
    let db = ObrainDB::new_in_memory();
    ecrire(
        &db,
        &["INSERT (:A {id: 'g'})-[:VERS {poids: 3}]->(:B {id: 'd'})"],
    );
    ecrire(&db, &["MATCH (:A)-[r:VERS]->(:B) REMOVE r.poids"]);

    let v = scalaire(&db, "MATCH (:A)-[r:VERS]->(:B) RETURN keys(r) AS c");
    let vide: Vec<String> = Vec::new();
    assert_eq!(
        en_ensemble(&v),
        vide,
        "l'arête ne doit plus porter aucune clé après le retrait de la seule \
         qu'elle avait"
    );
}

/// INTENTION : `MATCH (n) REMOVE n.x SET n.x = 5` laisse `x` à 5. Les clauses
/// s'appliquent dans l'ordre où elles sont ÉCRITES ; le `SET` vient après le
/// `REMOVE` et a donc le dernier mot.
///
/// PORTÉE RÉELLE — ce test ne double pas le GROUPE 2, il en étend le domaine.
/// Le GROUPE 2 montre qu'une requête commençant par `INSERT` jette sa queue.
/// Celle-ci commence par `MATCH` et la jette pourtant aussi : `parse_query`
/// analyse les clauses dans un ORDRE FIXE — `SET` (`parser.rs:686-691`) puis
/// `REMOVE` (`:695-698`) — si bien qu'un `SET` placé APRÈS un `REMOVE` dans le
/// texte n'est plus attendu quand on l'atteint, reste non consommé, et se perd
/// dans le même silence (`parse()`, `:241`). La cause profonde est la même —
/// aucun contrôle de fin d'entrée — mais elle frappe ici une requête que le
/// garde de production laisse passer sans hésiter.
// corrigé par ce commit : SET et REMOVE s'entrelacent dans l'ordre d'écriture
// (ordered_clauses porte désormais QueryClause::Remove) et `parse()` refuse
// toute queue non consommée.
#[test]
fn les_clauses_s_appliquent_dans_l_ordre_ou_elles_sont_ecrites() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a', x: 1})"]);
    ecrire(&db, &["MATCH (n:T {id: 'a'}) REMOVE n.x SET n.x = 5"]);

    assert_eq!(
        scalaire(&db, "MATCH (n:T {id: 'a'}) RETURN n.x AS x"),
        Value::Int64(5),
        "le SET est écrit APRÈS le REMOVE : c'est lui qui a le dernier mot"
    );
    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["id", "x"]),
        "et la clé qu'il pose est bien portée par le nœud"
    );
}

/// INTENTION : `MATCH (n) SET n.x = 5 REMOVE n.x` — l'ordre inverse, celui que
/// le traducteur applique de toute façon — laisse le nœud sans la clé `x`.
///
/// Ce test est le jumeau du précédent et sert à le lire : il emprunte le SEUL
/// ordre d'écriture que le parseur consomme entièrement. S'il est rouge quand
/// son jumeau l'est aussi, alors le défaut n'est pas d'ordonnancement mais de
/// `REMOVE` lui-même ; s'il est le seul des deux à échouer sur la FORME, on
/// mesure là le défaut du lot 4 et rien d'autre.
// corrigé par ce commit (remove réel : retrait en stockage + enregistrement WAL Remove*Property ; SET/INSERT à null ne matérialisent plus la clé)
#[test]
fn set_puis_remove_dans_cet_ordre_laisse_le_noeud_sans_la_cle() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a'})"]);
    ecrire(&db, &["MATCH (n:T {id: 'a'}) SET n.x = 5 REMOVE n.x"]);

    assert_eq!(
        scalaire(&db, "MATCH (n:T {id: 'a'}) RETURN n.x AS x"),
        Value::Null,
        "le REMOVE vient en dernier : la valeur posée par le SET est annulée"
    );
    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["id"]),
        "et la clé ne doit pas subsister"
    );
}

// ===========================================================================
// GROUPE 6 — DELETE / DETACH DELETE : CE QUI DISPARAÎT, ET CE QUI RESTE
//
// Une suppression se vérifie mal. « La ligne n'apparaît plus » est vrai pour
// mille raisons dont une seule est la bonne, et c'est justement le piège que
// l'en-tête de ce fichier annonce. On assertionne donc, à chaque fois : le
// COMPTE de ce qui devait disparaître, ET la FORME EXACTE de ce qui devait
// rester. La seconde moitié est la plus importante — une suppression qui
// emporte le voisin passe sans bruit un test qui ne regarde que sa cible.
// ===========================================================================

/// INTENTION : `DELETE` d'un nœud sans arête le fait disparaître, et lui seul.
#[test]
fn delete_d_un_noeud_isole_le_supprime_et_epargne_les_autres() {
    let db = ObrainDB::new_in_memory();
    ecrire(
        &db,
        &["INSERT (:T {id: 'a'})", "INSERT (:T {id: 'b', garde: 1})"],
    );
    ecrire(&db, &["MATCH (n:T {id: 'a'}) DELETE n"]);

    assert_eq!(compte(&db, "T"), 1, "un seul nœud a disparu");
    assert_eq!(
        cles(&db, "(n:T {id: 'b'})"),
        ens(&["garde", "id"]),
        "et le nœud voisin garde sa forme exacte, propriétés comprises"
    );
}

/// INTENTION : `DELETE` nu sur un nœud qui porte des arêtes ÉCHOUE, et ne
/// laisse aucune trace de sa tentative.
///
/// La première moitié (l'erreur) est déjà couverte par
/// `test_nodetach_delete_errors_with_edges` (`gql_spec_compliance.rs:316`). La
/// seconde ne l'est pas, et c'est elle qui compte pour ce lot : un refus doit
/// être ATOMIQUE. Un moteur qui supprimerait les arêtes avant de découvrir
/// qu'il ne peut pas supprimer le nœud rendrait la même erreur en laissant un
/// graphe mutilé.
#[test]
fn delete_nu_refuse_sur_un_noeud_a_aretes_et_ne_laisse_aucune_trace() {
    let db = ObrainDB::new_in_memory();
    ecrire(
        &db,
        &["INSERT (:A {id: 'g'})-[:VERS {poids: 3}]->(:B {id: 'd'})"],
    );

    let mut session = db.session();
    session.begin_transaction().unwrap();
    let refus = session.execute("MATCH (n:A {id: 'g'}) DELETE n").is_err();
    session.rollback().unwrap();

    assert!(refus, "DELETE nu sur un nœud à arêtes doit être refusé");
    assert_eq!(compte(&db, "A"), 1, "le nœud visé est toujours là");
    assert_eq!(compte(&db, "B"), 1, "le voisin aussi");
    let r = lire(&db, "MATCH (:A)-[r:VERS]->(:B) RETURN r.poids AS p");
    assert_eq!(
        r.len(),
        1,
        "et l'arête n'a pas été supprimée par anticipation"
    );
    assert_eq!(
        r[0][0],
        Value::Int64(3),
        "elle porte encore sa propriété : le refus est atomique"
    );
}

/// INTENTION : `DETACH DELETE` supprime le nœud ET ses arêtes, sans toucher au
/// nœud de l'autre extrémité.
///
/// Les trois assertions sont indissociables : le nœud parti, l'arête partie, et
/// le voisin INTACT DANS SA FORME. Un test qui omettrait la troisième ne
/// distinguerait pas un DETACH DELETE d'une suppression en cascade.
#[test]
fn detach_delete_emporte_les_aretes_et_epargne_le_voisin() {
    let db = ObrainDB::new_in_memory();
    ecrire(
        &db,
        &["INSERT (:A {id: 'g'})-[:VERS {poids: 3}]->(:B {id: 'd', garde: 1})"],
    );
    ecrire(&db, &["MATCH (n:A {id: 'g'}) DETACH DELETE n"]);

    assert_eq!(compte(&db, "A"), 0, "le nœud visé a disparu");
    assert_eq!(
        lire(&db, "MATCH ()-[r:VERS]->() RETURN r.poids AS p").len(),
        0,
        "et son arête avec lui"
    );
    assert_eq!(compte(&db, "B"), 1, "le voisin survit");
    assert_eq!(
        cles(&db, "(n:B {id: 'd'})"),
        ens(&["garde", "id"]),
        "et il survit AVEC SA FORME : DETACH DELETE n'est pas une cascade"
    );
}

/// INTENTION : `DELETE r` sur une arête retire l'arête et laisse les DEUX
/// nœuds, chacun dans sa forme.
#[test]
fn delete_d_une_arete_laisse_les_deux_noeuds_intacts() {
    let db = ObrainDB::new_in_memory();
    ecrire(
        &db,
        &["INSERT (:A {id: 'g', x: 1})-[:VERS]->(:B {id: 'd', y: 2})"],
    );
    ecrire(&db, &["MATCH (:A)-[r:VERS]->(:B) DELETE r"]);

    assert_eq!(
        lire(&db, "MATCH ()-[r:VERS]->() RETURN 1 AS u").len(),
        0,
        "l'arête a disparu"
    );
    assert_eq!(cles(&db, "(n:A {id: 'g'})"), ens(&["id", "x"]));
    assert_eq!(cles(&db, "(n:B {id: 'd'})"), ens(&["id", "y"]));
}

/// INTENTION : une suppression est visible immédiatement DANS sa transaction —
/// la ligne supprimée ne revient pas dans un MATCH qui suit.
#[test]
fn delete_est_visible_dans_sa_propre_transaction() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a'})", "INSERT (:T {id: 'b'})"]);

    let mut session = db.session();
    session.begin_transaction().unwrap();
    session.execute("MATCH (n:T {id: 'a'}) DELETE n").unwrap();
    let r = session.execute("MATCH (n:T) RETURN n.id AS i").unwrap();
    assert_eq!(
        r.rows.len(),
        1,
        "la transaction ne doit plus voir le nœud qu'elle vient de supprimer"
    );
    assert_eq!(r.rows[0][0], Value::String("b".into()), "et voit l'autre");
    session.commit().unwrap();
}

/// INTENTION : un `DELETE` annulé rend le nœud AVEC SA FORME EXACTE.
///
/// Le compte seul ne suffirait pas : un moteur qui recréerait un nœud vide au
/// rollback le satisferait. On vérifie la forme et une valeur.
#[test]
fn delete_annule_rend_le_noeud_avec_sa_forme_exacte() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a', x: 1, y: 'deux'})"]);

    let mut session = db.session();
    session.begin_transaction().unwrap();
    session.execute("MATCH (n:T {id: 'a'}) DELETE n").unwrap();
    session.rollback().unwrap();

    assert_eq!(compte(&db, "T"), 1, "le nœud est de retour");
    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["id", "x", "y"]),
        "avec toutes ses clés — pas un nœud vide recréé"
    );
    assert_eq!(
        proprietes(&db, "(n:T {id: 'a'})").get("y"),
        Some(&Value::String("deux".into())),
        "et avec ses valeurs"
    );
}

/// INTENTION : supprimer deux fois le même nœud laisse le graphe vide et ne
/// produit pas d'erreur — la seconde suppression n'apparie plus rien.
#[test]
fn delete_repete_laisse_le_graphe_vide_sans_erreur() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a'})"]);
    ecrire(
        &db,
        &[
            "MATCH (n:T {id: 'a'}) DETACH DELETE n",
            "MATCH (n:T {id: 'a'}) DETACH DELETE n",
        ],
    );

    assert_eq!(compte(&db, "T"), 0, "aucun nœud T ne subsiste");
}

/// INTENTION : après un `DETACH DELETE` commité, un `INSERT` du même motif
/// dépose un nœud NEUF — aucune propriété du nœud supprimé ne refait surface.
///
/// C'est le pendant, côté entité, de ce que le GROUPE 7 examine côté propriété.
/// Une suppression laisse des traces en stockage ; le langage ne doit jamais
/// les voir. L'assertion porte sur l'ensemble EXACT des clés : une propriété
/// ressuscitée s'y verrait, alors qu'une lecture ciblée de `id` ne la
/// montrerait pas.
#[test]
fn insert_apres_suppression_depose_un_noeud_sans_trace_de_l_ancien() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a', ancienne: 'trace', autre: 9})"]);
    ecrire(&db, &["MATCH (n:T {id: 'a'}) DETACH DELETE n"]);
    ecrire(&db, &["INSERT (:T {id: 'a'})"]);

    assert_eq!(compte(&db, "T"), 1, "un seul nœud T");
    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["id"]),
        "le nœud neuf ne porte QUE la clé de son motif : les propriétés du nœud \
         supprimé ne remontent pas du stockage"
    );
}

// ===========================================================================
// GROUPE 7 — LES TROIS ÉTATS D'UNE PROPRIÉTÉ, ET LES ÉCRITURES QUI Y MÈNENT
//
// Le stockage distingue trois états — vivant, pierre tombale, rien
// (`obrain-substrate/src/store.rs:6892`). Le langage n'en voit que deux, et
// seulement par `keys()` / `properties()`. La question de ce lot n'est pas
// « combien d'états existent » (le lot 4 l'a établie) mais « QUELLE ÉCRITURE
// mène à quel état » — car c'est cela qu'un appelant contrôle.
//
// Le semis direct par `create_node_with_props` est utilisé ici, et seulement
// ici : il fabrique un état — la clé nulle dès la création — que le reste du
// groupe compare aux états produits par le langage. Sans lui, on ne pourrait
// pas savoir si `INSERT (… x: null)` et une clé nulle native se ressemblent.
// ===========================================================================

/// INTENTION : un nœud semé avec une propriété à `Value::Null` PORTE la clé.
/// `keys` la liste, `properties` la rend avec la valeur nulle.
///
/// C'est le témoin du groupe : l'état « clé présente, valeur nulle » existe
/// bel et bien en stockage et se laisse observer depuis GQL. Tout le reste du
/// groupe se lit par rapport à lui.
#[test]
fn une_cle_nulle_semee_est_portee_par_le_noeud() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["T"], [("id", Value::String("a".into()))]);
    db.create_node_with_props(
        &["T"],
        [("id", Value::String("b".into())), ("x", Value::Null)],
    );

    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["id"]),
        "le nœud semé sans x ne porte pas la clé"
    );
    assert_eq!(
        cles(&db, "(n:T {id: 'b'})"),
        ens(&["id", "x"]),
        "le nœud semé avec x nul la porte"
    );
    assert_eq!(
        proprietes(&db, "(n:T {id: 'b'})").get("x"),
        Some(&Value::Null),
        "et properties() la rend avec sa valeur nulle"
    );
}

/// INTENTION : les écritures qui déclarent « cette propriété n'a pas de
/// valeur » mènent TOUTES au même état. Le nœud ne doit pas garder la mémoire
/// de la route empruntée.
///
/// Trois routes sont comparées : ne jamais écrire la clé, l'écrire à `null`
/// dans le motif d'`INSERT`, et la retirer par `REMOVE`. Les trois nœuds
/// doivent avoir la MÊME forme.
///
/// C'est la formulation la plus exigeante — et la plus juste — du défaut de
/// `REMOVE` : le problème n'est pas qu'une clé traîne, c'est que la forme d'un
/// nœud dépende de son HISTOIRE plutôt que de son contenu. Deux nœuds qui
/// portent la même information doivent être indistinguables ; sinon toute
/// comparaison de nœuds, tout export, tout diff de graphe devient sensible à
/// l'ordre des écritures passées.
// corrigé par ce commit (remove réel : retrait en stockage + enregistrement WAL Remove*Property ; SET/INSERT à null ne matérialisent plus la clé)
#[test]
fn les_trois_routes_vers_l_absence_de_valeur_menent_au_meme_etat() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'jamais'})"]);
    ecrire(&db, &["INSERT (:T {id: 'null_a_l_insert', x: null})"]);
    ecrire(&db, &["INSERT (:T {id: 'retiree', x: 1})"]);
    ecrire(&db, &["MATCH (n:T {id: 'retiree'}) REMOVE n.x"]);

    let jamais = cles(&db, "(n:T {id: 'jamais'})");
    let insere_nul = cles(&db, "(n:T {id: 'null_a_l_insert'})");
    let retiree = cles(&db, "(n:T {id: 'retiree'})");

    assert_eq!(
        retiree, jamais,
        "un nœud dont x a été RETIRÉ doit avoir la même forme qu'un nœud qui n'a \
         jamais porté x — sinon la forme mémorise l'histoire, pas le contenu"
    );
    assert_eq!(
        insere_nul, jamais,
        "déclarer x nul à l'insertion déclare que x n'a pas de valeur : même \
         état que ne pas l'écrire"
    );
}

/// INTENTION : les trois routes mènent à la même VALEUR — `NULL` — et ce point
/// est acquis quelle que soit l'issue du test précédent.
///
/// PORTÉE RÉELLE : ce test est vert dans les deux lectures et le restera. Il
/// est ici pour délimiter le correctif : ce qui doit converger est la FORME
/// (`keys`), pas la valeur, qui converge déjà. Un correctif qui ferait diverger
/// les valeurs pour distinguer les états casserait la règle de nullité que le
/// lot 4 a établie.
#[test]
fn les_trois_routes_rendent_toutes_la_valeur_nulle() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'jamais'})"]);
    ecrire(&db, &["INSERT (:T {id: 'null_a_l_insert', x: null})"]);
    ecrire(&db, &["INSERT (:T {id: 'retiree', x: 1})"]);
    ecrire(&db, &["MATCH (n:T {id: 'retiree'}) REMOVE n.x"]);

    for id in ["jamais", "null_a_l_insert", "retiree"] {
        assert_eq!(
            scalaire(&db, &format!("MATCH (n:T {{id: '{id}'}}) RETURN n.x AS v")),
            Value::Null,
            "route « {id} » : la valeur lue est NULL"
        );
        assert_eq!(
            scalaire(
                &db,
                &format!("MATCH (n:T {{id: '{id}'}}) RETURN n.x IS NULL AS v")
            ),
            Value::Bool(true),
            "route « {id} » : IS NULL est vrai"
        );
    }
}

/// INTENTION : le remplacement par carte ramène les trois routes à UNE seule
/// forme. C'est l'opération réparatrice — la seule dont dispose aujourd'hui un
/// appelant qui doit normaliser un graphe hétérogène.
///
/// Le test l'applique aux trois nœuds, y compris celui qui n'avait rien à
/// nettoyer, et vérifie qu'ils finissent identiques. Un remplacement qui ne
/// saurait pas retirer une clé NULLE laisserait deux formes derrière lui.
#[test]
fn le_remplacement_ramene_les_trois_routes_a_une_seule_forme() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'jamais'})"]);
    ecrire(&db, &["INSERT (:T {id: 'null_a_l_insert', x: null})"]);
    ecrire(&db, &["INSERT (:T {id: 'retiree', x: 1})"]);
    ecrire(&db, &["MATCH (n:T {id: 'retiree'}) REMOVE n.x"]);

    for id in ["jamais", "null_a_l_insert", "retiree"] {
        ecrire(
            &db,
            &[&format!(
                "MATCH (n:T {{id: '{id}'}}) SET n = {{id: '{id}'}}"
            )],
        );
    }

    for id in ["jamais", "null_a_l_insert", "retiree"] {
        assert_eq!(
            cles(&db, &format!("(n:T {{id: '{id}'}})")),
            ens(&["id"]),
            "après remplacement, la route « {id} » a la forme canonique : le \
             remplacement retire aussi bien une clé nulle qu'une clé pleine"
        );
    }
}

// ===========================================================================
// GROUPE 8 — CE QU'UNE ÉCRITURE REND VISIBLE, ET QUAND
//
// Les groupes précédents lisent après commit. Celui-ci interroge l'entre-deux :
// ce qu'une transaction montre d'elle-même, ce qu'elle montre aux autres, et ce
// qu'elle laisse quand elle échoue. C'est la seule partie du lot où l'INSTANT
// de la lecture fait partie de l'assertion.
// ===========================================================================

/// INTENTION : une écriture non commitée n'est pas visible d'une autre
/// session. Vérifié ici sur la voie du BALAYAGE — un `MATCH` sans prédicat de
/// propriété.
///
/// C'est l'isolation la plus élémentaire, et son échec ne se rattrape pas :
/// une lecture sale ne devient pas propre au commit, elle a déjà été servie.
/// Cette voie-ci est correcte, et c'est ce qui rend les deux tests suivants
/// lisibles : le moteur SAIT masquer une écriture en cours — il ne le fait pas
/// partout.
#[test]
fn le_balayage_ne_montre_pas_l_ecriture_non_commitee_d_une_autre_session() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'commite'})"]);

    let mut ecrivain = db.session();
    ecrivain.begin_transaction().unwrap();
    ecrivain.execute("INSERT (:T {id: 'en_cours'})").unwrap();

    let lecteur = db.session();
    let vu = lecteur
        .execute("MATCH (n:T) RETURN collect(n.id) AS l")
        .unwrap();
    ecrivain.rollback().unwrap();

    assert_eq!(
        vu.rows[0][0],
        Value::List(vec![Value::String("commite".into())].into()),
        "une session tierce ne doit voir que ce qui est commité"
    );
}

/// INTENTION : le COMPTE non plus ne doit pas inclure l'écriture non commitée
/// d'une autre session. Un compte est une lecture comme une autre.
///
/// PORTÉE RÉELLE — ce test ne fait pas double emploi avec le précédent : il
/// interroge la MÊME requête sous une autre projection, et les deux ne
/// s'accordent pas. Le balayage montre une ligne, le compte en annonce deux.
/// Ce n'est pas seulement un compte trop grand : c'est un graphe qui se décrit
/// autrement qu'il ne se laisse lire, et aucune des deux réponses ne permet de
/// deviner l'autre.
/// HISTORIQUE (corrigé, vague 4a 2026-08-30) : le compte empruntait le
/// raccourci `try_plan_count_shortcut` → `node_count_by_label`, un compteur
/// global qui incluait les écritures en attente de TOUTES les transactions,
/// quand seule la voie du balayage passait par `filtrer_par_epoque_et_tx`
/// (obrain-substrate/src/store.rs) et sa carte `en_attente_noeuds`. Corrigé :
/// le raccourci se désarme (`GraphStore::has_pending_writes`) tant que des
/// écritures sont en attente, et le plan repasse par le balayage filtré.
#[test]
fn le_compte_n_inclut_pas_l_ecriture_non_commitee_d_une_autre_session() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'commite'})"]);

    let mut ecrivain = db.session();
    ecrivain.begin_transaction().unwrap();
    ecrivain.execute("INSERT (:T {id: 'en_cours'})").unwrap();

    let lecteur = db.session();
    let vu = lecteur.execute("MATCH (n:T) RETURN count(n) AS c").unwrap();
    let compte_vu = vu.rows[0][0].clone();
    ecrivain.rollback().unwrap();

    assert_eq!(
        compte_vu,
        Value::Int64(1),
        "le compte doit s'accorder avec le balayage : une seule ligne commitée"
    );
}

/// INTENTION : un `MATCH` portant un PRÉDICAT DE PROPRIÉTÉ ne doit pas rendre
/// la ligne écrite par une transaction en cours ailleurs.
///
/// C'est la lecture sale au sens plein — pas un compte faux, mais la DONNÉE
/// elle-même : le nœud est rendu, ses propriétés sont lisibles, `keys()` décrit
/// sa forme. Une session tierce peut donc lire l'intégralité d'une écriture qui
/// sera peut-être annulée. Ce test l'établit sur les deux orthographes du
/// prédicat, en ligne et en `WHERE`, parce qu'elles empruntent le même chemin
/// et qu'un correctif qui n'en fermerait qu'une laisserait la porte ouverte.
/// HISTORIQUE (corrigé, vague 4a 2026-08-30) : les chemins indexés filtraient
/// leurs candidats par `is_node_visible_at_epoch` / `is_node_visible_versioned`
/// (obrain-substrate/src/store.rs), deux fonctions qui se réduisaient à
/// `is_live_on_disk` — un nœud créé par une transaction ouverte, déjà sur le
/// disque, passait. Corrigé : les deux surcharges appliquent désormais la même
/// définition que le filtre par lot `filtrer_par_epoque_et_tx` (carte
/// `en_attente_noeuds` consultée), et `IndexSeekJoinOperator` emprunte le
/// filtre par lot lui-même, exactement comme `ScanOperator`.
#[test]
fn un_predicat_de_propriete_ne_lit_pas_l_ecriture_non_commitee_d_autrui() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'commite'})"]);

    let mut ecrivain = db.session();
    ecrivain.begin_transaction().unwrap();
    ecrivain.execute("INSERT (:T {id: 'en_cours'})").unwrap();

    let lecteur = db.session();
    let en_ligne = lecteur
        .execute("MATCH (n:T {id: 'en_cours'}) RETURN n.id AS i")
        .unwrap()
        .rows;
    let en_where = lecteur
        .execute("MATCH (n:T) WHERE n.id = 'en_cours' RETURN n.id AS i")
        .unwrap()
        .rows;
    ecrivain.rollback().unwrap();

    assert!(
        en_ligne.is_empty(),
        "prédicat en ligne : la ligne d'une transaction ouverte ailleurs ne doit \
         pas être lisible, obtenu {en_ligne:?}"
    );
    assert!(
        en_where.is_empty(),
        "prédicat en WHERE : même exigence, obtenu {en_where:?}"
    );
}

/// INTENTION : les écritures d'une transaction sont commitées ENSEMBLE. Après
/// commit, toutes sont là ; le graphe ne passe pas par un état partiel visible.
///
/// L'assertion porte sur l'état complet du graphe — le compte ET la forme des
/// deux nœuds — parce qu'un commit partiel se reconnaît à ce qu'il laisse, pas
/// à ce qu'il rend.
#[test]
fn les_ecritures_d_une_transaction_sont_commitees_ensemble() {
    let db = ObrainDB::new_in_memory();
    ecrire(
        &db,
        &[
            "INSERT (:T {id: 'a'})",
            "MATCH (n:T {id: 'a'}) SET n.x = 1",
            "INSERT (:T {id: 'b', y: 2})",
        ],
    );

    assert_eq!(compte(&db, "T"), 2, "les deux insertions sont là");
    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["id", "x"]),
        "l'écriture intermédiaire est là aussi"
    );
    assert_eq!(cles(&db, "(n:T {id: 'b'})"), ens(&["id", "y"]));
}

/// INTENTION : le `rollback` d'un `REMOVE` restaure la clé ET sa valeur.
///
/// Ce cas est ici, et pas au GROUPE 5, parce qu'il n'interroge pas `REMOVE`
/// mais l'annulation. Il est vert quelle que soit l'issue du défaut de
/// `REMOVE` : que la transaction ait écrit un NULL ou retiré la clé, l'annuler
/// doit rendre l'état d'avant. C'est ce qui le rend utile — il garantit qu'un
/// correctif de `REMOVE` restera annulable, alors qu'un correctif qui
/// passerait par un chemin de stockage différent pourrait ne plus l'être.
#[test]
fn le_rollback_d_un_remove_restaure_la_cle_et_sa_valeur() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T {id: 'a', x: 1})"]);

    let mut session = db.session();
    session.begin_transaction().unwrap();
    session.execute("MATCH (n:T {id: 'a'}) REMOVE n.x").unwrap();
    session.rollback().unwrap();

    assert_eq!(
        cles(&db, "(n:T {id: 'a'})"),
        ens(&["id", "x"]),
        "la clé est restaurée"
    );
    assert_eq!(
        scalaire(&db, "MATCH (n:T {id: 'a'}) RETURN n.x AS x"),
        Value::Int64(1),
        "et sa valeur avec elle"
    );
}
