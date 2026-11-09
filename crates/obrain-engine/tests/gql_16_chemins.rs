//! Lot 16 — CONFORMITÉ GQL : LES CHAÎNES DE MOTIFS ET LE LABEL TERMINAL.
//!
//! `MATCH (a)-[:R]->(b)-[:R]->(c:Label) WHERE c.prop = valeur`.
//!
//! # L'ANGLE DE CE FICHIER
//!
//! Le 30/08, la validation croisée (note PO 563e670f) a établi sur une base
//! réelle une matrice exacte, jugée par SQL sur le graphe source :
//!
//!   1 saut,  cible par nom, sans label        →  25   rendu 25   juste
//!   1 saut,  cible par nom, label :Function   →  21   rendu 21   juste
//!   2 sauts, cible par nom, sans label        → 134   rendu 134  juste
//!   2 sauts, cible par nom, label :Function   → 117   rendu 20   FAUX
//!
//! Et une FUITE : un nœud `:Method` rendu par le motif `(c:Function)`, des
//! liaisons dupliquées, une perte non uniforme par cible (histogram 9/78,
//! zone_map 7/32, external_sort 3/3, trois cibles à 0).
//!
//! # LA CAUSE, ÉTABLIE EN LA TRAÇANT JUSQU'À L'INDICE
//!
//! Rien à voir avec la planification : le plan logique est correct
//! (Filter(name) ∘ Filter(hasLabel) ∘ Expand ∘ Expand ∘ Scan). La perte est
//! dans l'EXÉCUTION :
//!
//!   1. une chaîne de 2 sauts et plus passe par `LazyFactorizedChainOperator`
//!      (`obrain-engine/src/query/planner/lpg/mod.rs:585`), qui APLATIT toute
//!      l'expansion en UN SEUL DataChunk — 178 593 lignes mesurées sur la
//!      base carto ;
//!   2. `SelectionVector` stockait ses indices en `u16`, et
//!      `from_predicate` les écrivait par `i as u16` SANS GARDE
//!      (`obrain-core/src/execution/selection.rs:51` avant correctif) : tout
//!      indice ≥ 65 536 s'aliasait modulo 65 536 sur une ligne antérieure ;
//!   3. le premier filtre (hasLabel) posait donc une sélection au COMPTE
//!      juste mais aux indices corrompus — doublons, lignes d'autrui (la
//!      fuite du `:Method`) — et le second filtre (name), EMPILÉ sur cette
//!      sélection (`operators/filter.rs:3695`), évaluait la propriété sur
//!      les lignes aliasées : 20 liaisons rendues sur 117.
//!
//! C'est pourquoi la matrice de la note est ce qu'elle est : UN filtre seul
//! rend un compte juste même corrompu (count = longueur de sélection,
//! calculée AVANT la troncature) — d'où le 134 juste, le 78 juste, et le
//! `WHERE c.name=… AND c:Function` juste (un seul filtre ET) — tandis que
//! DEUX filtres empilés relisent les indices corrompus — d'où le 20. Et
//! c'est pourquoi la fixture minimale (GROUPES 0-2) n'a JAMAIS mordu : sous
//! 65 536 lignes aplaties, `u16` suffit. Le GROUPE 3 construit un graphe qui
//! dépasse ce seuil et reproduit le défaut chez lui.
//!
//! # MÉTHODE — lire avant de modifier ce fichier
//!
//! Chaque assertion encode une INTENTION SÉMANTIQUE, jamais une observation
//! du moteur — les comptes attendus sont calculés à la main sur la fixture.
//! Quand le moteur diverge de l'intention, le test est placé derrière
//! `#[cfg(defauts_connus)]` et précédé d'un commentaire
//! `// DÉFAUT:` qui NOMME la divergence et son emplacement en fichier:ligne.
//! Il reste rouge par construction — jamais réécrit pour devenir vert.
//!
//!   `cargo test -p obrain-engine --test gql_16_chemins`
//!       la suite par défaut : verte, les défauts ne sont pas compilés.
//!   `cargo test -p obrain-engine --test gql_16_chemins --cfg defauts_connus (RUSTFLAGS)`
//!       les défauts tournent et ÉCHOUENT — légitimement. Ce sont les tests
//!       à faire passer, pas à supprimer.
//!
//! # ANTI-VACUITÉ
//!
//! Un motif qui ne matche rien rend zéro ligne, et un test qui compare des
//! ensembles vides serait vert sur une fixture vide. Trois parades :
//!   - les comptes attendus sont STRICTEMENT POSITIFS partout où la
//!     sémantique l'exige ;
//!   - `la_fixture_porte_exactement_ce_qui_est_annonce` (et son jumeau
//!     grande échelle) échoue sur toute fixture appauvrie — une écriture de
//!     fixture perdue en silence se voit ICI, pas dans un test devenu
//!     vacuement vert ;
//!   - chaque test de fuite vérifie que l'ensemble observé est NON VIDE
//!     avant d'en vérifier le contenu.
//!
//! # CONTRE-VÉRIFICATION SUR LA BASE RÉELLE (après correctif)
//!
//! La fixture du GROUPE 3 reproduit le défaut chez elle ; elle ne prouve pas
//! que la BASE RÉELLE guérit. Cette vérification-là a été refaite sur
//! `obrain-carto-db` (44 811 nœuds) avec un binaire compilé du correctif, et
//! jugée contre la vérité SQL du graphe source (134 lignes de la chaîne
//! 2 sauts, dont le label du terminal est connu ligne à ligne) :
//!
//!   1 saut  sans label        →  25 dû,  25 rendu
//!   1 saut  label :Function   →  21 dû,  21 rendu
//!   2 sauts sans label        → 134 dû, 134 rendu
//!   2 sauts label :Function   → 117 dû, 117 rendu   (20 AVANT le correctif)
//!
//! et, ce que la somme seule ne saurait établir :
//!   - répartition par cible EXACTE — histogram 78, zone_map 32,
//!     external_sort 3, rdf.mod 2, merge 1, check_eval 1 ;
//!   - répartition par APPELANT exacte — 103 appelants distincts, zéro
//!     appelant en désaccord avec SQL : ni doublon, ni compensation entre
//!     cibles ;
//!   - la fuite a disparu — le motif `:Method` sur la même chaîne rend 17
//!     (dû 17, contre 20 avant), et 117 + 17 = 134 : la partition est nette,
//!     sans recouvrement. Avant le correctif, `ColumnPredicate.compare_values`
//!     (un `:Method`) était rendu sous le motif `(c:Function)`.
//!
//! Le passage de `u16` à `u32` ne coûte rien de mesurable : en paires
//! alternées base/correctif à charge calme, l'écart des minima est de 0 à
//! 1 ms sur des requêtes de 104 à 186 ms, avec `RETURN 1` (que le correctif
//! ne peut pas affecter) comme témoin à 0 ms.
//!
//! Run : cargo test -p obrain-engine --test gql_16_chemins

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// FIXTURE MINIMALE — un graphe d'appels miniature, calqué sur la forme réelle.
//
// Trois nœuds portent le MÊME nom `cv` sous des labels DIFFÉRENTS — la
// condition multi-graines de la note 563e670f :
//
//   f1:Function {name:'cv'}    f2:Function {name:'cv'}    m1:Method {name:'cv'}
//
// Appelants directs (1 saut) :
//   b1:Function → f1        b2:Function → f1        b3:Function → f2
//   b4:Method   → m1        b5:Function → autre (bruit, n'appelle pas cv)
//
// Appelants des appelants (2 sauts) :
//   a1 → b1    a2 → b1    a3 → b2    a4 → b3    a5 → b4    a6 → b5 (bruit)
//
// COMPTES CALCULÉS À LA MAIN (liaisons (…, c), une ligne par liaison) :
//
//   1 saut  vers name='cv', sans label : b1→f1, b2→f1, b3→f2, b4→m1      = 4
//   1 saut  vers name='cv', c:Function : b1→f1, b2→f1, b3→f2             = 3
//   2 sauts vers name='cv', sans label : a1→b1→f1, a2→b1→f1, a3→b2→f1,
//                                        a4→b3→f2, a5→b4→m1              = 5
//   2 sauts vers name='cv', c:Function : les mêmes moins a5→b4→m1        = 4
//   par cible (2 sauts, c:Function)    : f1 = 3, f2 = 1, m1 = 0
//   2 sauts vers qn='f1', sans label   : a1, a2, a3                      = 3
//
// PORTÉE RÉELLE, à ne pas surestimer : cette fixture reste SOUS le seuil de
// troncature (65 536 lignes aplaties), donc ces tests étaient déjà verts
// AVEC le défaut. Ils ancrent la sémantique de référence à petite échelle ;
// c'est le GROUPE 3 qui mord.
// ---------------------------------------------------------------------------

fn fixture() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let mut session = db.session();
    session.begin_transaction().unwrap();
    for r in [
        // Les cibles : même nom, labels différents.
        "INSERT (:Function {name: 'cv', qn: 'f1'})",
        "INSERT (:Function {name: 'cv', qn: 'f2'})",
        "INSERT (:Method {name: 'cv', qn: 'm1'})",
        // Les appelants directs.
        "INSERT (:Function {name: 'b1', qn: 'b1'})",
        "INSERT (:Function {name: 'b2', qn: 'b2'})",
        "INSERT (:Function {name: 'b3', qn: 'b3'})",
        "INSERT (:Method {name: 'b4', qn: 'b4'})",
        "INSERT (:Function {name: 'b5', qn: 'b5'})",
        // Les appelants des appelants.
        "INSERT (:Function {name: 'a1', qn: 'a1'})",
        "INSERT (:Function {name: 'a2', qn: 'a2'})",
        "INSERT (:Function {name: 'a3', qn: 'a3'})",
        "INSERT (:Function {name: 'a4', qn: 'a4'})",
        "INSERT (:Function {name: 'a5', qn: 'a5'})",
        "INSERT (:Function {name: 'a6', qn: 'a6'})",
        // La cible de bruit : b5 appelle un nœud qui ne s'appelle PAS cv.
        "INSERT (:Function {name: 'autre', qn: 'autre'})",
        // Sauts 1 : b → cible.
        "MATCH (s {qn: 'b1'}), (t {qn: 'f1'}) INSERT (s)-[:CALLS]->(t)",
        "MATCH (s {qn: 'b2'}), (t {qn: 'f1'}) INSERT (s)-[:CALLS]->(t)",
        "MATCH (s {qn: 'b3'}), (t {qn: 'f2'}) INSERT (s)-[:CALLS]->(t)",
        "MATCH (s {qn: 'b4'}), (t {qn: 'm1'}) INSERT (s)-[:CALLS]->(t)",
        "MATCH (s {qn: 'b5'}), (t {qn: 'autre'}) INSERT (s)-[:CALLS]->(t)",
        // Sauts 2 : a → b.
        "MATCH (s {qn: 'a1'}), (t {qn: 'b1'}) INSERT (s)-[:CALLS]->(t)",
        "MATCH (s {qn: 'a2'}), (t {qn: 'b1'}) INSERT (s)-[:CALLS]->(t)",
        "MATCH (s {qn: 'a3'}), (t {qn: 'b2'}) INSERT (s)-[:CALLS]->(t)",
        "MATCH (s {qn: 'a4'}), (t {qn: 'b3'}) INSERT (s)-[:CALLS]->(t)",
        "MATCH (s {qn: 'a5'}), (t {qn: 'b4'}) INSERT (s)-[:CALLS]->(t)",
        "MATCH (s {qn: 'a6'}), (t {qn: 'b5'}) INSERT (s)-[:CALLS]->(t)",
    ] {
        session
            .execute(r)
            .unwrap_or_else(|e| panic!("l'écriture `{r}` doit s'exécuter, erreur : {e:?}"));
    }
    session.commit().unwrap();
    db
}

// ---------------------------------------------------------------------------
// FIXTURE GRANDE ÉCHELLE — le même motif, au-delà du seuil de troncature.
//
// AA appelants → un concentrateur `hub` → CC cibles. L'aplatissement de la
// chaîne 2 sauts matérialise AA × CC lignes : avec AA = CC = 300, cela fait
// 90 000 lignes > 65 536 — chaque ligne au-delà du seuil avait, avant
// correctif, son indice de sélection tronqué modulo 65 536.
//
//   a_1..a_300 : Function        (chacun appelle hub)
//   hub        : Function        (appelle chaque cible)
//   n_1..n_297 : Function {name:'n_i'}  — bruit
//   cv_f1, cv_f2 : Function {name:'cv'} — les cibles cherchées
//   cv_m       : Method   {name:'cv'}   — le témoin de fuite
//
// Les trois cibles `cv` sont insérées EN DERNIER dans l'adjacence du hub :
// leurs lignes tombent en QUEUE de chaque bloc de 300, donc au-delà du seuil
// pour tout a_i d'indice ≥ 219 — c'est là que la troncature mordait.
//
// COMPTES CALCULÉS À LA MAIN :
//   nœuds  : 300 + 1 + 297 + 2 + 1                                   = 601
//   arêtes : 300 (a→hub) + 300 (hub→cible)                           = 600
//   1 saut  vers name='cv', sans label : hub→cv_f1, hub→cv_f2, hub→cv_m = 3
//   1 saut  vers name='cv', c:Function :                                 2
//   2 sauts vers name='cv', sans label : 300 appelants × 3            = 900
//   2 sauts vers name='cv', c:Function : 300 × 2                      = 600
//   par cible (2 sauts, c:Function)    : cv_f1 = 300, cv_f2 = 300
// ---------------------------------------------------------------------------

const GRAND_AA: usize = 300;
const GRAND_BRUIT: usize = 297;

fn fixture_grande() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let mut session = db.session();
    session.begin_transaction().unwrap();
    let executer = |r: String| {
        session
            .execute(&r)
            .unwrap_or_else(|e| panic!("l'écriture `{r}` doit s'exécuter, erreur : {e:?}"));
    };
    executer("INSERT (:Function {name: 'hub', qn: 'hub'})".to_string());
    for i in 1..=GRAND_AA {
        executer(format!("INSERT (:Function {{name: 'a_{i}', qn: 'a_{i}'}})"));
    }
    for i in 1..=GRAND_BRUIT {
        executer(format!("INSERT (:Function {{name: 'n_{i}', qn: 'n_{i}'}})"));
    }
    executer("INSERT (:Function {name: 'cv', qn: 'cv_f1'})".to_string());
    executer("INSERT (:Function {name: 'cv', qn: 'cv_f2'})".to_string());
    executer("INSERT (:Method {name: 'cv', qn: 'cv_m'})".to_string());
    for i in 1..=GRAND_AA {
        executer(format!(
            "MATCH (s {{qn: 'a_{i}'}}), (t {{qn: 'hub'}}) INSERT (s)-[:CALLS]->(t)"
        ));
    }
    // Le bruit d'abord, les cibles `cv` EN DERNIER : leurs lignes tombent en
    // queue de bloc, au-delà du seuil de troncature pour les derniers a_i.
    for i in 1..=GRAND_BRUIT {
        executer(format!(
            "MATCH (s {{qn: 'hub'}}), (t {{qn: 'n_{i}'}}) INSERT (s)-[:CALLS]->(t)"
        ));
    }
    for qn in ["cv_f1", "cv_f2", "cv_m"] {
        executer(format!(
            "MATCH (s {{qn: 'hub'}}), (t {{qn: '{qn}'}}) INSERT (s)-[:CALLS]->(t)"
        ));
    }
    session.commit().unwrap();
    db
}

/// Lit dans une session neuve — après commit, jamais depuis le cache d'une
/// transaction ouverte.
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

/// Le compte rendu par `RETURN count(*)`.
fn compter(db: &ObrainDB, requete: &str) -> i64 {
    match scalaire(db, requete) {
        Value::Int64(n) => n,
        autre => panic!("count doit rendre un Int64, obtenu {autre:?}"),
    }
}

/// Les lignes `(chaîne, compte)` d'un `RETURN x, count(*)`, en carte triée.
fn repartition(db: &ObrainDB, requete: &str) -> BTreeMap<String, i64> {
    let mut carte = BTreeMap::new();
    for r in lire(db, requete) {
        let cle = match &r[0] {
            Value::String(s) => s.to_string(),
            autre => panic!("la clé de répartition doit être une chaîne, obtenu {autre:?}"),
        };
        let n = match &r[1] {
            Value::Int64(n) => *n,
            autre => panic!("count doit rendre un Int64, obtenu {autre:?}"),
        };
        carte.insert(cle, n);
    }
    carte
}

/// Les valeurs d'une colonne de chaînes, non dédupliquées.
fn chaines(rows: &[Vec<Value>]) -> Vec<String> {
    rows.iter()
        .map(|r| match &r[0] {
            Value::String(s) => s.to_string(),
            autre => panic!("attendu une chaîne, obtenu {autre:?}"),
        })
        .collect()
}

// ===========================================================================
// GROUPE 0 — ANTI-VACUITÉ : LA FIXTURE PORTE CE QU'ELLE ANNONCE
//
// Tous les comptes du fichier reposent sur les fixtures. Si une écriture de
// fixture échouait en silence — arête non posée, nœud absent — les motifs
// rendraient MOINS de lignes et certains tests deviendraient vacuement
// verts. Ce groupe échoue le premier dans ce cas.
// ===========================================================================

/// INTENTION : la fixture minimale porte exactement les nœuds et arêtes
/// annoncés dans son en-tête — ni plus, ni moins.
#[test]
fn la_fixture_porte_exactement_ce_qui_est_annonce() {
    let db = fixture();

    assert_eq!(
        compter(&db, "MATCH (n) RETURN count(*)"),
        15,
        "15 nœuds : 3 cibles, 5 appelants directs, 6 grands-appelants, 1 cible \
         de bruit"
    );
    assert_eq!(
        compter(&db, "MATCH ()-[:CALLS]->() RETURN count(*)"),
        11,
        "11 arêtes CALLS : 5 du rang b, 6 du rang a"
    );
    assert_eq!(
        compter(&db, "MATCH (c) WHERE c.name = 'cv' RETURN count(*)"),
        3,
        "trois nœuds portent le nom 'cv' — la condition multi-graines du défaut"
    );
    assert_eq!(
        compter(&db, "MATCH (c:Method) WHERE c.name = 'cv' RETURN count(*)"),
        1,
        "dont UN :Method — le témoin de la fuite de label"
    );
    assert_eq!(
        compter(
            &db,
            "MATCH (c:Function) WHERE c.name = 'cv' RETURN count(*)"
        ),
        2,
        "et DEUX :Function"
    );
}

// ===========================================================================
// GROUPE 1 — UN SAUT : LA RÉFÉRENCE QUI TIENT
//
// Sur la base réelle, le motif 1 saut est JUSTE avec et sans label (25/25 et
// 21/21) : l'expansion simple traite ses lignes chunk par chunk et ne
// franchit jamais le seuil de troncature. Ces tests ancrent la référence :
// si un correctif du 2 sauts cassait le 1 saut, c'est ici que ça se verrait.
// ===========================================================================

/// INTENTION : 1 saut vers les porteurs du nom, sans label — 4 liaisons.
#[test]
fn un_saut_vers_le_nom_sans_label_rend_quatre_liaisons() {
    let db = fixture();
    assert_eq!(
        compter(
            &db,
            "MATCH (b)-[:CALLS]->(c) WHERE c.name = 'cv' RETURN count(*)"
        ),
        4,
        "b1→f1, b2→f1, b3→f2, b4→m1 : quatre liaisons, une par arête entrante \
         d'un porteur du nom"
    );
}

/// INTENTION : 1 saut vers les porteurs du nom, terminal `c:Function` — le
/// :Method est écarté, 3 liaisons.
#[test]
fn un_saut_vers_le_nom_avec_label_ecarte_le_method() {
    let db = fixture();
    assert_eq!(
        compter(
            &db,
            "MATCH (b)-[:CALLS]->(c:Function) WHERE c.name = 'cv' RETURN count(*)"
        ),
        3,
        "b1→f1, b2→f1, b3→f2 : le label du terminal écarte b4→m1"
    );
}

// ===========================================================================
// GROUPE 2 — DEUX SAUTS, PETITE ÉCHELLE : LA SÉMANTIQUE DE RÉFÉRENCE
//
// La forme de la note 563e670f — chaîne de deux sauts, filtre par nom
// multi-graines, label sur le nœud TERMINAL — mais sous le seuil de
// troncature : ces tests étaient verts AVEC le défaut. Ils établissent ce
// que la forme DOIT rendre, et que le correctif n'a rien déplacé à petite
// échelle. La reproduction du défaut est au GROUPE 3.
// ===========================================================================

/// INTENTION : 2 sauts vers les porteurs du nom, sans label — 5 liaisons.
#[test]
fn deux_sauts_vers_le_nom_sans_label_rend_cinq_liaisons() {
    let db = fixture();
    assert_eq!(
        compter(
            &db,
            "MATCH (a)-[:CALLS]->(b)-[:CALLS]->(c) WHERE c.name = 'cv' RETURN count(*)"
        ),
        5,
        "a1→b1→f1, a2→b1→f1, a3→b2→f1, a4→b3→f2, a5→b4→m1"
    );
}

/// INTENTION : 2 sauts vers les porteurs du nom, terminal `c:Function` — les
/// mêmes liaisons moins celle qui aboutit au :Method, donc 4.
#[test]
fn deux_sauts_vers_le_nom_avec_label_rend_quatre_liaisons() {
    let db = fixture();
    assert_eq!(
        compter(
            &db,
            "MATCH (a)-[:CALLS]->(b)-[:CALLS]->(c:Function) WHERE c.name = 'cv' RETURN count(*)"
        ),
        4,
        "les cinq liaisons du motif sans label, moins a5→b4→m1 que le label \
         du terminal doit écarter"
    );
}

/// INTENTION : le détail PAR CIBLE s'accorde avec le total — f1 reçoit 3
/// chemins de 2 sauts, f2 en reçoit 1, m1 zéro (écarté par le label).
#[test]
fn deux_sauts_le_detail_par_cible_s_accorde_avec_le_total() {
    let db = fixture();
    let obtenu = repartition(
        &db,
        "MATCH (a)-[:CALLS]->(b)-[:CALLS]->(c:Function) WHERE c.name = 'cv' \
         RETURN c.qn, count(*)",
    );
    let attendu: BTreeMap<String, i64> = [("f1".to_string(), 3), ("f2".to_string(), 1)].into();
    assert_eq!(
        obtenu, attendu,
        "f1 reçoit a1, a2, a3 ; f2 reçoit a4 ; m1 n'apparaît pas (label)"
    );
}

/// INTENTION : 2 sauts vers une cible UNIQUE (`qn`, sans label) — 3 liaisons.
///
/// C'est le témoin de la note 563e670f : la même chaîne, filtrée sur une clé
/// qui ne sélectionne qu'UNE graine, était juste (78/78 sur la base réelle).
#[test]
fn deux_sauts_vers_une_cible_unique_sans_label_rend_trois_liaisons() {
    let db = fixture();
    assert_eq!(
        compter(
            &db,
            "MATCH (a)-[:CALLS]->(b)-[:CALLS]->(c) WHERE c.qn = 'f1' RETURN count(*)"
        ),
        3,
        "a1→b1→f1, a2→b1→f1, a3→b2→f1"
    );
}

/// INTENTION : 2 sauts vers une cible unique AVEC label — mêmes 3 liaisons.
#[test]
fn deux_sauts_vers_une_cible_unique_avec_label_rend_trois_liaisons() {
    let db = fixture();
    assert_eq!(
        compter(
            &db,
            "MATCH (a)-[:CALLS]->(b)-[:CALLS]->(c:Function) WHERE c.qn = 'f1' RETURN count(*)"
        ),
        3,
        "le label du terminal n'écarte rien ici : f1 EST une :Function"
    );
}

// ===========================================================================
// GROUPE 3 — DEUX SAUTS, GRANDE ÉCHELLE : LÀ OÙ LE DÉFAUT MORDAIT
//
// La même matrice que le GROUPE 2, sur la fixture qui aplatit 90 000 lignes
// — au-delà des 65 536 indices que `u16` sait nommer. Avant correctif :
// le premier filtre (hasLabel) posait une sélection aux indices tronqués
// modulo 65 536, le second (name) évaluait sur les lignes aliasées — compte
// effondré, doublons, fuite du :Method. C'est la reproduction autonome du
// 20/117 de la base carto.
// ===========================================================================

/// INTENTION : l'anti-vacuité de la fixture grande échelle — 601 nœuds,
/// 600 arêtes, et le motif 2 sauts SANS filtre matérialise bien 90 000
/// liaisons, au-delà du seuil de troncature. Si ce compte passait sous
/// 65 536, tout le groupe testerait un cas que le défaut ne couvrait pas.
#[test]
fn la_fixture_grande_franchit_le_seuil_de_troncature() {
    let db = fixture_grande();
    assert_eq!(
        compter(&db, "MATCH (n) RETURN count(*)"),
        601,
        "300 appelants + hub + 297 bruits + 2 cibles Function + 1 Method"
    );
    assert_eq!(
        compter(&db, "MATCH ()-[:CALLS]->() RETURN count(*)"),
        600,
        "300 arêtes a→hub, 300 arêtes hub→cible"
    );
    assert_eq!(
        compter(&db, "MATCH (a)-[:CALLS]->(b)-[:CALLS]->(c) RETURN count(*)"),
        (GRAND_AA * (GRAND_BRUIT + 3)) as i64,
        "90 000 liaisons 2 sauts : c'est le franchissement du seuil de 65 536 \
         qui donne au groupe sa raison d'être"
    );
}

/// INTENTION : 2 sauts vers les porteurs du nom, sans label — 900 liaisons
/// (300 appelants × 3 porteurs du nom).
///
/// Ce compte était JUSTE avec le défaut (un seul filtre : le compte est la
/// longueur de la sélection, calculée avant la troncature des indices). Il
/// tient le rôle du 134/134 de la base réelle : le témoin qui isole le
/// défaut sur l'EMPILEMENT.
#[test]
fn grande_echelle_deux_sauts_sans_label_rend_neuf_cents_liaisons() {
    let db = fixture_grande();
    assert_eq!(
        compter(
            &db,
            "MATCH (a)-[:CALLS]->(b)-[:CALLS]->(c) WHERE c.name = 'cv' RETURN count(*)"
        ),
        (GRAND_AA * 3) as i64,
        "chaque a_i atteint cv_f1, cv_f2 et cv_m à travers le hub"
    );
}

/// INTENTION : 2 sauts vers les porteurs du nom, terminal `c:Function` —
/// 600 liaisons (300 appelants × 2 porteurs :Function). Le :Method est
/// écarté, aucune liaison n'est perdue ni dupliquée.
///
/// C'est LA forme fautive de la note 563e670f (20 rendu pour 117 dû sur la
/// base carto ; ici, avec le défaut, le compte s'effondrait de la même
/// façon).
// corrigé par ce commit : SelectionVector stocke ses indices en u32 et
// from_predicate REFUSE un chunk intronquable au lieu d'aliaser
// (obrain-core/src/execution/selection.rs). La troncature `i as u16` sans
// garde corrompait la sélection posée par le premier filtre (hasLabel) dès
// que le chunk aplati par LazyFactorizedChainOperator dépassait 65 536
// lignes ; le second filtre (name), empilé dessus
// (operators/filter.rs:3695), évaluait alors les lignes aliasées.
#[test]
fn grande_echelle_deux_sauts_avec_label_rend_six_cents_liaisons() {
    let db = fixture_grande();
    assert_eq!(
        compter(
            &db,
            "MATCH (a)-[:CALLS]->(b)-[:CALLS]->(c:Function) WHERE c.name = 'cv' RETURN count(*)"
        ),
        (GRAND_AA * 2) as i64,
        "les 900 liaisons du motif sans label, moins les 300 qui aboutissent \
         au :Method que le label du terminal doit écarter"
    );
}

/// INTENTION : le label du terminal est APPLIQUÉ — aucun nœud rendu par
/// `(c:Function)` ne porte le label `Method`, et l'ensemble rendu est NON
/// VIDE (anti-vacuité : un motif qui ne rend rien serait vert sans rien
/// prouver).
///
/// Sur la base carto, un `:Method` était rendu par le motif `(c:Function)` :
/// l'indice tronqué du filtre de nom retombait sur une ligne dont le
/// terminal n'avait jamais passé le filtre de label.
// corrigé par ce commit : même cause, même correction (indices de sélection
// u32, obrain-core/src/execution/selection.rs) — l'aliasing modulo 65 536
// faisait rendre des lignes que le filtre hasLabel n'avait jamais retenues.
#[test]
fn grande_echelle_le_label_du_terminal_ne_fuit_pas() {
    let db = fixture_grande();
    let rows = lire(
        &db,
        "MATCH (a)-[:CALLS]->(b)-[:CALLS]->(c:Function) WHERE c.name = 'cv' \
         RETURN DISTINCT c.qn",
    );
    assert!(
        !rows.is_empty(),
        "anti-vacuité : le motif doit rendre au moins une cible"
    );
    let qns = chaines(&rows);
    assert!(
        !qns.contains(&"cv_m".to_string()),
        "le motif (c:Function) ne doit JAMAIS rendre le nœud :Method cv_m, \
         obtenu {qns:?}"
    );
}

/// INTENTION : le détail PAR CIBLE s'accorde avec le total — cv_f1 et cv_f2
/// reçoivent chacun les 300 appelants, cv_m zéro (écarté par le label).
///
/// Sur la base réelle, la perte n'était pas uniforme (histogram 9/78,
/// zone_map 7/32, external_sort 3/3, trois cibles à 0) : la troncature
/// épargnait les lignes sous le seuil et aliasait les autres. Ce test fige
/// la répartition due, pas seulement la somme — une compensation entre
/// cibles (l'une gonflée par les doublons d'aliasing, l'autre amputée) ne
/// peut pas le tromper.
// corrigé par ce commit : même cause, même correction (indices de sélection
// u32, obrain-core/src/execution/selection.rs).
#[test]
fn grande_echelle_le_detail_par_cible_s_accorde_avec_le_total() {
    let db = fixture_grande();
    let obtenu = repartition(
        &db,
        "MATCH (a)-[:CALLS]->(b)-[:CALLS]->(c:Function) WHERE c.name = 'cv' \
         RETURN c.qn, count(*)",
    );
    let attendu: BTreeMap<String, i64> = [
        ("cv_f1".to_string(), GRAND_AA as i64),
        ("cv_f2".to_string(), GRAND_AA as i64),
    ]
    .into();
    assert_eq!(
        obtenu, attendu,
        "chaque appelant atteint chaque cible :Function exactement une fois ; \
         cv_m n'apparaît pas (label)"
    );
}

/// INTENTION : les liaisons rendues sont exactement les paires (a_i, cible),
/// SANS doublon — la même liaison rendue deux fois est aussi fausse qu'une
/// liaison perdue.
///
/// Sur la base carto, le motif fautif rendait 20 lignes dont 4 EN DOUBLE
/// (16 triplets distincts) : deux indices distincts aliasés modulo 65 536
/// sur la même ligne. `RETURN DISTINCT` ne peut pas voir ça ; ce test
/// compare le compte BRUT au compte d'appelants distincts.
// corrigé par ce commit : même cause, même correction (indices de sélection
// u32, obrain-core/src/execution/selection.rs).
#[test]
fn grande_echelle_aucune_liaison_n_est_rendue_en_double() {
    let db = fixture_grande();
    let rows = lire(
        &db,
        "MATCH (a)-[:CALLS]->(b)-[:CALLS]->(c:Function) \
         WHERE c.name = 'cv' AND c.qn = 'cv_f1' RETURN a.qn",
    );
    assert!(
        !rows.is_empty(),
        "anti-vacuité : le motif doit rendre au moins une liaison"
    );
    let mut appelants = chaines(&rows);
    let brut = appelants.len();
    appelants.sort();
    appelants.dedup();
    assert_eq!(
        brut,
        appelants.len(),
        "chaque appelant n'atteint cv_f1 que par UN chemin : toute ligne en \
         double est un indice aliasé"
    );
    assert_eq!(
        appelants.len(),
        GRAND_AA,
        "et TOUS les appelants sont là — aucune liaison perdue"
    );
}
