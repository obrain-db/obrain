//! Revue #189 (2e passe) : propriétés utilisateur versus métadonnées internes, `reduce`
//! sur éléments distants, liste d'arêtes qualifiée. Attendus tirés à la main.
//!
//! alpha : Note n -[:L {k:'local-l'}]-> Hub h -[:T]-> ref(beta, doc_1), h -[:M]-> Zed z.
//! beta  : Doc doc_1 (name 'remote') ; Doc doc_ok (name 'ok', _id: 42 ABSENT) ;
//!         Doc doc_bad (name 'bad', _id: 7 : propriété réservée) ; doc_1 -[:R {k:'remote-r'}]-> doc_ok.
#![cfg(all(feature = "gql", feature = "cypher"))]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn bases() -> (ObrainDB, ObrainDB) {
    let alpha = ObrainDB::new_in_memory();
    let beta = ObrainDB::new_in_memory();
    let sb = beta.session();
    sb.execute("INSERT (:Doc {id: 'doc_1', name: 'remote'})")
        .expect("doc_1");
    sb.execute("INSERT (:Doc {id: 'doc_ok', name: 'ok'})")
        .expect("doc_ok");
    sb.execute("INSERT (:Doc {id: 'doc_bad', name: 'bad', _id: 7})")
        .expect("doc_bad");
    sb.execute(
        "MATCH (a:Doc {id: 'doc_1'}), (b:Doc {id: 'doc_ok'}) CREATE (a)-[:R {k: 'remote-r'}]->(b)",
    )
    .expect("R");
    let sa = alpha.session();
    sa.execute("INSERT (:Note {name: 'local-n', _id: 99})")
        .expect("note");
    sa.execute("INSERT (:Hub {name: 'local-h'})").expect("hub");
    sa.execute("INSERT (:Zed {name: 'local-z'})").expect("zed");
    sa.execute("MATCH (n:Note), (h:Hub) CREATE (n)-[:L {k: 'local-l'}]->(h)")
        .expect("L");
    sa.execute("MATCH (h:Hub), (z:Zed) CREATE (h)-[:M {k: 'local-m'}]->(z)")
        .expect("M");
    sa.execute("MATCH (h:Hub) CREATE (h)-[:T]->(ref(\"beta\", \"doc_1\"))")
        .expect("T");
    sa.execute("MATCH (h:Hub) CREATE (h)-[:TB]->(ref(\"beta\", \"doc_bad\"))")
        .expect("T bad");
    // Même type d'arête vers un voisin valide et un voisin refusé : le WHERE doit
    // exclure doc_bad SANS que sa matérialisation fasse échouer la requête.
    sa.execute("MATCH (h:Hub) CREATE (h)-[:TX]->(ref(\"beta\", \"doc_ok\"))")
        .expect("TX ok");
    sa.execute("MATCH (h:Hub) CREATE (h)-[:TX]->(ref(\"beta\", \"doc_bad\"))")
        .expect("TX bad");
    alpha.attach_bases("alpha", None);
    alpha.attach_base("beta", &beta).expect("attach beta");
    (alpha, beta)
}

fn entier(v: &Value) -> i64 {
    match v {
        Value::Int64(i) => *i,
        autre => panic!("entier attendu : {autre:?}"),
    }
}

fn textes(v: &Value) -> Vec<Option<String>> {
    match v {
        Value::List(items) => items
            .iter()
            .map(|x| match x {
                Value::String(s) => Some(s.to_string()),
                Value::Null => None,
                autre => panic!("texte attendu : {autre:?}"),
            })
            .collect(),
        autre => panic!("liste attendue : {autre:?}"),
    }
}

fn id_brut(db: &ObrainDB, q: &str) -> i64 {
    entier(&db.session().execute(q).expect("id").rows[0][0])
}

// ── 1. Propriété utilisateur `_id` : valeur utilisateur conservée (local), refus (distant) ──

#[test]
fn propriete_utilisateur_id_locale_conservee_et_identite_reelle() {
    let (alpha, _beta) = bases();
    let r = alpha
        .session()
        .execute("MATCH WALK p = (n:Note)-[:L]->(h:Hub) RETURN [x IN nodes(p) | x._id], [x IN nodes(p) | id(x)]")
        .expect("local");
    let user = match &r.rows[0][0] {
        Value::List(items) => items.iter().map(entier_ou_null).collect::<Vec<_>>(),
        autre => panic!("{autre:?}"),
    };
    assert_eq!(
        user[0],
        Some(99),
        "x._id = valeur utilisateur du nœud local"
    );
    let ids = match &r.rows[0][1] {
        Value::List(items) => items.iter().map(entier).collect::<Vec<_>>(),
        autre => panic!("{autre:?}"),
    };
    let reel = id_brut(&alpha, "MATCH (n:Note) RETURN id(n)");
    assert_eq!(
        ids[0], reel,
        "id(x) = identité réelle, pas la propriété utilisateur"
    );
    assert_ne!(
        ids[0], 99,
        "l'identité réelle n'est pas la propriété utilisateur"
    );
}

fn entier_ou_null(v: &Value) -> Option<i64> {
    match v {
        Value::Int64(i) => Some(*i),
        Value::Null => None,
        autre => panic!("{autre:?}"),
    }
}

#[test]
fn propriete_reservee_sur_noeud_distant_refusee_sans_alteration() {
    let (alpha, beta) = bases();
    let err = alpha
        .session()
        .execute("MATCH WALK p = (n:Note)-[:L]->(h:Hub)-[:T]->(d) RETURN [x IN nodes(p) | x.name]")
        .err();
    // doc_1 seul ne porte pas de clé réservée : la requête vers doc_1 réussit.
    assert!(err.is_none(), "doc_1 sans clé réservée : {err:?}");
    // doc_bad porte `_id` : refus explicite du chemin qui le traverse.
    let refus = alpha
        .session()
        .execute("MATCH WALK p = (n:Note)-[:L]->(h:Hub)-[:TB]->(d) RETURN length(p)");
    let msg = match refus {
        Ok(_) => panic!("doc_bad doit être refusé"),
        Err(e) => format!("{e}"),
    };
    assert!(
        msg.contains("_id") && msg.contains("réservée"),
        "message : {msg}"
    );
    // La donnée n'a pas été modifiée : la propriété reste lisible dans sa base.
    let b = beta
        .session()
        .execute("MATCH (d:Doc {id: 'doc_bad'}) RETURN d._id")
        .expect("beta");
    assert_eq!(entier(&b.rows[0][0]), 7);
}

// ── 2. reduce sur éléments distants ──────────────────────────────────────────────

#[test]
fn reduce_id_labels_type_sur_noeuds_et_aretes_distants() {
    let (alpha, _beta) = bases();
    let id_n = id_brut(&alpha, "MATCH (n:Note) RETURN id(n)");
    let id_h = id_brut(&alpha, "MATCH (h:Hub) RETURN id(h)");
    let r = alpha
        .session()
        .execute(
            "MATCH WALK p = (n:Note)-[:L]->(h:Hub)-[:T]->(d) \
             RETURN reduce(acc = 0, x IN nodes(p) | acc + id(x)), \
                    reduce(acc = '', x IN nodes(p) | acc + labels(x)[0]), \
                    reduce(acc = '', x IN relationships(p) | acc + type(x))",
        )
        .expect("reduce distant");
    assert_eq!(r.rows.len(), 1, "{:?}", r.rows);
    // Somme des identifiants : primaire (n, h) + brut distant (doc_1 dans beta).
    let somme_attendue = id_n + id_h + id_brut(&_beta, "MATCH (d:Doc {id: 'doc_1'}) RETURN id(d)");
    assert_eq!(
        entier(&r.rows[0][0]),
        somme_attendue,
        "id sur noeuds mixtes"
    );
    assert_eq!(
        textes(&Value::List(vec![r.rows[0][1].clone()].into())),
        vec![Some("NoteHubDoc".to_string())],
        "labels distants"
    );
    assert_eq!(
        textes(&Value::List(vec![r.rows[0][2].clone()].into())),
        vec![Some("LT".to_string())],
        "types d'arêtes"
    );
}

// ── 3. liste d'arêtes qualifiée et unicité ───────────────────────────────────────

#[test]
fn liste_aretes_qualifiee_type_exact_locale_et_externe() {
    // Attendu exact : l'arête LOCALE M rend "M", l'arête EXTERNE T rend "T".
    let (alpha, _beta) = bases();
    let r = alpha
        .session()
        .execute("MATCH WALK (n:Note)-[rs:L|T|M*2..2]->(x) RETURN [e IN rs | type(e)]")
        .expect("rs");
    assert_eq!(r.rows.len(), 2, "{:?}", r.rows);
    let mut lignes: Vec<Vec<Option<String>>> = r.rows.iter().map(|l| textes(&l[0])).collect();
    lignes.sort();
    assert_eq!(
        lignes,
        vec![
            vec![Some("L".into()), Some("M".into())],
            vec![Some("L".into()), Some("T".into())],
        ],
        "type(e) exact attendu, jamais Null"
    );
}

#[test]
fn where_exclut_noeud_distant_refuse_sans_echec_global() {
    // doc_bad (refusé à la matérialisation) doit être écarté par le WHERE :
    // la requête rend doc_ok, elle n'échoue pas.
    let (alpha, _beta) = bases();
    let r = alpha
        .session()
        .execute("MATCH (h:Hub)-[:TX]->(d) WHERE d.name = 'ok' RETURN d.name")
        .expect("un refus global avant filtrage est une régression");
    assert_eq!(r.rows, vec![vec![Value::String("ok".into())]]);
}

#[test]
fn liste_aretes_trail_unicite_sur_la_liste() {
    let (alpha, _beta) = bases();
    // TRAIL : 2 hops parmi L, T, M depuis Note. Les deux seuls chemins sont
    // Note-[L]->Hub-[T]->doc_1 et Note-[L]->Hub-[M]->Zed (TB et TX ne sont
    // pas dans le filtre). Vérifie la taille ET les cibles distinctes.
    let r = alpha
        .session()
        .execute(
            "MATCH TRAIL (n:Note)-[rs:L|T|M*2..2]->(x) \
             RETURN size(rs), x.name ORDER BY x.name",
        )
        .expect("trail");
    assert_eq!(r.rows.len(), 2, "attendu 2 chemins TRAIL : {:?}", r.rows);
    assert!(
        r.rows.iter().all(|l| entier(&l[0]) == 2),
        "chaque chemin a 2 arêtes"
    );
    // Les cibles sont distinctes (Zed local et doc_1 distant) : TRAIL ne
    // filtre rien ici car les deux chemins n'ont aucune arête en commun.
    let mut noms: Vec<Option<String>> = r
        .rows
        .iter()
        .map(|l| match &l[1] {
            Value::String(s) => Some(s.to_string()),
            Value::Map(m) => m
                .get(&obrain_common::types::PropertyKey::new("name"))
                .and_then(|v| match v {
                    Value::String(s) => Some(s.to_string()),
                    _ => None,
                }),
            _ => None,
        })
        .collect();
    noms.sort();
    assert_eq!(
        noms,
        vec![Some("local-z".to_string()), Some("remote".to_string())],
        "cibles TRAIL : Zed (local) et doc_1 (distant)"
    );
}

#[test]
fn proprietes_d_aretes_locale_externe_distante_exactes() {
    let (alpha, _beta) = bases();
    let r = alpha
        .session()
        .execute(
            "MATCH WALK p = (n:Note)-[:L]->(h:Hub)-[:T]->(d)-[:R]->(e) \
             RETURN [r IN relationships(p) | r.k], [r IN relationships(p) | type(r)]",
        )
        .expect("propriétés d'arêtes");
    assert_eq!(r.rows.len(), 1, "{:?}", r.rows);
    // L (locale) : 'local-l' ; T (externe, V1 sans propriété) : Null ; R (distante) : 'remote-r'.
    assert_eq!(
        textes(&r.rows[0][0]),
        vec![
            Some("local-l".to_string()),
            None,
            Some("remote-r".to_string())
        ],
        "propriétés des arêtes"
    );
    assert_eq!(
        textes(&r.rows[0][1]),
        vec![
            Some("L".to_string()),
            Some("T".to_string()),
            Some("R".to_string())
        ],
        "types des arêtes"
    );
}

// ── 6. ExpandOperator (simple hop) : écrasement silencieux (limite connue) ───

#[test]
fn expand_simple_hop_ecrasement_silencieux_limite_connue() {
    // (#189 revue finding 1, option C) L'ExpandOperator NE refuse PAS les
    // clés réservées : un refus global avant le WHERE empêcherait les
    // requêtes avec prédicat d'exclure le noeud fautif. Conséquence :
    // `d._id` rend l'identifiant INTERNE (écrasé par `remote_node_value`),
    // pas la propriété utilisateur `7`. C'est une limite connue.
    let (alpha, _beta) = bases();
    let r = alpha
        .session()
        .execute("MATCH (h:Hub)-[:TB]->(d) RETURN d._id")
        .expect("pas de refus en expansion simple");
    assert_eq!(r.rows.len(), 1, "un résultat");
    // La valeur est l'id interne de beta, PAS 7 (écrasement).
    let id = entier(&r.rows[0][0]);
    assert_ne!(
        id, 7,
        "l'id interne a écrasé la propriété utilisateur (limite connue)"
    );
}

#[test]
fn expand_simple_hop_noeud_distant_sans_cle_reservee_ok() {
    // Le même chemin vers un noeud propre (doc_1) ne doit pas échouer.
    let (alpha, _beta) = bases();
    let r = alpha
        .session()
        .execute("MATCH (h:Hub)-[:T]->(d) RETURN d.name")
        .expect("doc_1 sans clé réservée");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0][0], Value::String("remote".into()));
}

// ── 7. Split clés réservées : un noeud avec `_type` n'est PAS refusé ────────

#[test]
fn noeud_distant_avec_propriete_type_pas_refuse() {
    // `_type` est réservé pour les ARÊTES, pas pour les nœuds.
    // Avant le split (`cle_reservee` unique → `cle_reservee_noeud` / `cle_reservee_arete`),
    // un noeud avec `_type` était faussement rejeté.
    let alpha = ObrainDB::new_in_memory();
    let gamma = ObrainDB::new_in_memory();
    gamma
        .session()
        .execute("INSERT (:Item {id: 'gadget', name: 'gadget', _type: 'custom'})")
        .expect("item");
    alpha.session().execute("INSERT (:Start)").expect("start");
    alpha
        .session()
        .execute("MATCH (s:Start) CREATE (s)-[:LINK]->(ref(\"gamma\", \"gadget\"))")
        .expect("link");
    alpha.attach_bases("alpha", None);
    alpha.attach_base("gamma", &gamma).expect("attach gamma");
    // Via ExpandOperator (simple hop) :
    let r = alpha
        .session()
        .execute("MATCH (s:Start)-[:LINK]->(i) RETURN i.name, i._type")
        .expect("_type sur noeud ne devrait pas être refusé");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0][0], Value::String("gadget".into()));
    assert_eq!(r.rows[0][1], Value::String("custom".into()));
    // Via VLE (chemin) :
    let r2 = alpha
        .session()
        .execute("MATCH WALK p = (s:Start)-[:LINK*1..1]->(i) RETURN [x IN nodes(p) | x.name]")
        .expect("_type sur noeud VLE ne devrait pas être refusé");
    assert_eq!(r2.rows.len(), 1);
}
