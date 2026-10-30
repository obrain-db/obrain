//! COUT-PROPORTIONNEL — l'egalite AVANT la vitesse.
//!
//! Chantier : « qu'aucune requete ne coute plus que sa reponse ». Quatre
//! reecritures sont envisagees (A: `labels(n) CONTAINS 'X'` -> scan
//! d'etiquette ; B: `STARTS WITH 'p'` -> plage sur l'index ordonne ;
//! C: `ORDER BY .. LIMIT k` -> tas borne ; D: projection dans le scan).
//!
//! CE FICHIER N'OPTIMISE RIEN. Il pose, pour chaque reecriture, le contrat
//! d'egalite que la reecriture devra tenir — et il doit etre VERT sur le
//! code d'avant, ou il ne prouve rien.
//!
//! ── LA FORME DES TESTS : UN ORACLE INDEPENDANT DU PLAN ──
//!
//! Comparer « l'ancien plan » et « le nouveau plan » depuis un test est
//! impossible tant que le nouveau n'existe pas. On compare donc chaque
//! requete a un ORACLE calcule en Rust a partir d'une requete dont le plan
//! n'est PAS celui sous test :
//!
//!   · oracle de B : `MATCH (n:S) RETURN n.nom` (scan d'etiquette + projection,
//!     aucun filtre) puis `str::starts_with` en Rust ;
//!   · oracle de C : la meme source, triee integralement en Rust puis coupee ;
//!   · oracle de A : `RETURN labels(n)` (evaluateur) puis appartenance en Rust.
//!
//! L'oracle ne traverse donc jamais le chemin qu'on veut reecrire. Avant la
//! reecriture le test compare le balayage a l'oracle ; apres, l'index a
//! l'oracle. La MEME assertion, inchangee, garde les deux regimes — c'est ce
//! que « l'egalite exacte du resultat » demande.
//!
//! ── CE QUI EST VERIFIE ──
//!
//! Memes lignes, meme compte, et meme ORDRE partout ou la requete le
//! specifie. Les tests de B comparent des ENSEMBLES (la requete ne
//! specifie aucun ordre, en exiger un serait tester autre chose que ce
//! qui est promis) ; les tests de C comparent des SEQUENCES.
//!
//!   cargo test -p obrain-engine --test cout_proportionnel

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

// ───────────────────────────── outillage ─────────────────────────────

/// Base en memoire portant un noeud `:S {nom: …}` par entree.
fn db_noms(noms: &[&str]) -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for n in noms {
        s.create_node_with_props(&["S"], [("nom", Value::String((*n).into()))]);
    }
    db
}

fn lignes(db: &ObrainDB, q: &str) -> Vec<Vec<Value>> {
    db.session()
        .execute(q)
        .unwrap_or_else(|e| panic!("requete refusee : {q}\n{e}"))
        .rows
}

/// Premiere colonne, en texte, dans l'ordre rendu par le moteur.
fn textes(db: &ObrainDB, q: &str) -> Vec<String> {
    lignes(db, q)
        .into_iter()
        .map(|l| match &l[0] {
            Value::String(s) => s.to_string(),
            autre => panic!("valeur non-chaine : {autre:?}"),
        })
        .collect()
}

/// Les memes, tries : pour les requetes SANS `ORDER BY`, ou l'ordre n'est
/// pas promis et ou seule l'egalite des ENSEMBLES est un contrat.
fn textes_tries(db: &ObrainDB, q: &str) -> Vec<String> {
    let mut v = textes(db, q);
    v.sort();
    v
}

/// L'ORACLE de B et C : toutes les valeurs de `nom`, par un plan qui ne
/// contient NI filtre NI tri — scan d'etiquette et projection, rien d'autre.
/// C'est la source de verite contre laquelle les chemins optimises sont
/// mesures, et elle ne partage avec eux aucun code de selection.
fn oracle_noms(db: &ObrainDB) -> Vec<String> {
    textes(db, "MATCH (n:S) RETURN n.nom")
}

/// Un jeu volontairement adversarial pour un prefixe `self:` :
/// voisins lexicographiques immediats de part et d'autre du prefixe et de
/// sa borne haute, casse, prefixe exact, prefixe strictement plus long.
const JEU_PREFIXE: [&str; 14] = [
    "self",      // plus court que le prefixe — DEHORS
    "self:",     // egal au prefixe — DEDANS (starts_with est reflexif)
    "self:a",    // DEDANS
    "self:zzz",  // DEDANS
    "self:9",    // DEDANS
    "self9",     // '9' (0x39) < ':' (0x3A) — DEHORS, sous la borne basse
    "self;",     // ';' (0x3B) = borne haute exclusive — DEHORS
    "self;a",    // DEHORS
    "selg",      // 'g' > 'f' — DEHORS, mais > "self:" en octets : piege
    "sele",      // 'e' < 'f' — DEHORS
    "Self:",     // casse differente — DEHORS
    "",          // chaine vide — DEHORS
    "s",         // DEHORS
    "self:\u{e9}t\u{e9}", // unicode apres le prefixe — DEDANS
];

// ═══════════════════════════════════════════════════════════════════════
// A — `labels(n) CONTAINS 'X'`  et  `'X' IN labels(n)`
// ═══════════════════════════════════════════════════════════════════════

/// La base a etiquettes : un noeud mono-etiquette, un MULTI-etiquette, et
/// un temoin qui ne porte aucune des deux. Le multi-etiquette est le cas
/// que la note de chantier demande explicitement de verifier : il doit
/// apparaitre dans LES DEUX scans d'etiquette.
fn db_etiquettes() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.create_node_with_props(&["Task"], [("nom", Value::String("mono-task".into()))]);
    s.create_node_with_props(&["Node"], [("nom", Value::String("mono-node".into()))]);
    s.create_node_with_props(
        &["Task", "Node"],
        [("nom", Value::String("multi".into()))],
    );
    s.create_node_with_props(&["Autre"], [("nom", Value::String("temoin".into()))]);
    db
}

/// PREMISSE A : ELLE EST DEVENUE VRAIE — ET CE TEST L'EPINGLE AUSSI.
///
/// ── CE QU'AFFIRMAIT CE TEST, ET POURQUOI IL A CHANGE (2026-09-14) ──────────
///
/// La note de chantier posait que `labels(n) CONTAINS 'X'` est « semantiquement
/// identique » a `MATCH (n:X)` quand X est un litteral, et proposait de
/// reecrire l'un en l'autre pour un gain annonce de x83.
///
/// Sur la lignee j6, c'etait FAUX, et le test le montrait : les 2 673 ms etaient
/// depensees « POUR 0 LIGNE RENDUE », parce que `CONTAINS` etait un operateur de
/// CHAINE — `left.as_str()?`, et `Value::as_str` rend `None` pour une
/// `Value::List` comme `labels(n)`. Le predicat ne s'evaluait JAMAIS a vrai.
///
/// ── CE QUE LA CONSOLIDATION A FAIT ENTRER ──────────────────────────────────
///
/// Le tronc avait CORRIGE ce `None` (commit `79a77fd2`, 2026-09-13), pour une
/// raison mesuree au chat du 8432 : une ligne rejetee en silence se lit comme
/// une reponse (« aucun noeud de ce type »), et ce faux negatif a coute sept
/// requetes supplementaires et 181 s de conversation. Le moteur evalue
/// desormais l'APPARTENANCE quand la gauche est une liste
/// (`items.iter().any(values_equal)` — la meme egalite structurelle que `IN`).
///
/// CONSEQUENCE, ET C'EST ELLE QUE CE TEST FIXE MAINTENANT :
///
///   `labels(n) CONTAINS 'X'`  ≡  `'X' IN labels(n)`  ≡  `MATCH (n:X)`
///
/// pour un litteral X. La reecriture tant discutee est donc LEGITIME sous cette
/// semantique : elle ne change plus la reponse. Ce qui reste interdit — et la
/// seconde assertion le tient — c'est de confondre APPARTENANCE et SOUS-CHAINE :
/// `labels(n) CONTAINS 'Tas'` ne retient rien, la ou un `contains` de chaine en
/// aurait retenu deux.
#[test]
fn a_labels_contains_egale_l_appartenance_donc_la_reecriture_est_legitime() {
    let db = db_etiquettes();
    let par_contains = textes_tries(&db, "MATCH (n) WHERE labels(n) CONTAINS 'Task' RETURN n.nom");
    let par_appartenance = textes_tries(&db, "MATCH (n) WHERE 'Task' IN labels(n) RETURN n.nom");
    let par_scan = textes_tries(&db, "MATCH (n:Task) RETURN n.nom");

    assert_eq!(
        par_scan,
        vec!["mono-task".to_string(), "multi".to_string()],
        "temoin : le scan d'etiquette voit bien le noeud multi-etiquette"
    );
    assert_eq!(
        par_contains, par_appartenance,
        "sur une LISTE, `CONTAINS` doit dire exactement ce que dit `IN`"
    );
    assert_eq!(
        par_contains, par_scan,
        "les trois formes coincident : la reecriture de `labels(n) CONTAINS 'X'` \
         vers `MATCH (n:X)` ne change plus la reponse"
    );
    assert!(
        textes_tries(&db, "MATCH (n) WHERE labels(n) CONTAINS 'Tas' RETURN n.nom").is_empty(),
        "`CONTAINS` sur une liste est une APPARTENANCE, jamais une SOUS-CHAINE : \
         'Tas' n'est aucune etiquette entiere"
    );
}

/// La forme qui EST un test d'appartenance : `'X' IN labels(n)`.
///
/// C'est celle que le traducteur GraphQL emet pour les conditions de type
/// (`engine/src/query/translators/graphql.rs`, « type_cond IN labels(var) »),
/// donc un chemin de production. Contrat : identique au scan d'etiquette,
/// noeud multi-etiquette compris.
#[test]
fn a_in_labels_egale_le_scan_d_etiquette() {
    let db = db_etiquettes();
    for etiquette in ["Task", "Node", "Autre"] {
        let par_appartenance = textes_tries(
            &db,
            &format!("MATCH (n) WHERE '{etiquette}' IN labels(n) RETURN n.nom"),
        );
        let par_scan = textes_tries(&db, &format!("MATCH (n:{etiquette}) RETURN n.nom"));
        assert_eq!(
            par_appartenance, par_scan,
            "'{etiquette}' IN labels(n) doit rendre exactement MATCH (n:{etiquette})"
        );
    }
}

/// Le cas nomme par la note de chantier : un noeud `[Task, Node]` doit
/// apparaitre dans LES DEUX scans. Une reecriture qui transformerait une
/// appartenance en « premiere etiquette seulement » le perdrait.
#[test]
fn a_noeud_multi_etiquettes_apparait_dans_les_deux_scans() {
    let db = db_etiquettes();
    assert!(
        textes(&db, "MATCH (n:Task) RETURN n.nom").contains(&"multi".to_string()),
        "le noeud [Task, Node] doit sortir du scan Task"
    );
    assert!(
        textes(&db, "MATCH (n:Node) RETURN n.nom").contains(&"multi".to_string()),
        "le noeud [Task, Node] doit sortir du scan Node"
    );
}

/// L'etiquette portee par une VARIABLE, pas par un litteral : aucune
/// reecriture vers un scan d'etiquette n'est permise ici, l'etiquette
/// n'etant pas connue a la planification. Le garde est pose en comparant
/// a l'oracle d'appartenance calcule en Rust.
#[test]
fn a_appartenance_a_etiquette_non_litterale_reste_correcte() {
    let db = db_etiquettes();
    // `labels(n)` en PROJECTION : l'evaluateur, jamais le scan.
    let mut attendu: Vec<String> = lignes(&db, "MATCH (n) RETURN n.nom, labels(n)")
        .into_iter()
        .filter_map(|l| {
            let nom = match &l[0] {
                Value::String(s) => s.to_string(),
                autre => panic!("nom non-chaine : {autre:?}"),
            };
            let Value::List(ls) = &l[1] else {
                panic!("labels(n) doit rendre une liste, obtenu {:?}", l[1])
            };
            ls.iter()
                .any(|v| matches!(v, Value::String(s) if &**s == "Task"))
                .then_some(nom)
        })
        .collect();
    attendu.sort();

    let par_scan = textes_tries(&db, "MATCH (n:Task) RETURN n.nom");
    assert_eq!(
        par_scan, attendu,
        "l'oracle d'appartenance (evaluateur) et le scan d'etiquette doivent \
         s'accorder : c'est ce qui autorise A sur un litteral"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// B — `STARTS WITH 'prefixe'` -> plage sur l'index ordonne
// ═══════════════════════════════════════════════════════════════════════

/// Le contrat central de B, sur le jeu adversarial : le moteur doit rendre
/// EXACTEMENT `str::starts_with`, ni un noeud de plus ni un de moins.
///
/// `selg` est le piege : il est lexicographiquement SUPERIEUR a `self:`
/// (borne basse) et doit pourtant etre rejete — c'est la borne haute qui
/// l'exclut, et une borne haute mal calculee le laisserait passer.
#[test]
fn b_starts_with_egale_str_starts_with() {
    let db = db_noms(&JEU_PREFIXE);
    let mut attendu: Vec<String> = oracle_noms(&db)
        .into_iter()
        .filter(|n| n.starts_with("self:"))
        .collect();
    attendu.sort();

    let obtenu = textes_tries(
        &db,
        "MATCH (n:S) WHERE n.nom STARTS WITH 'self:' RETURN n.nom",
    );
    assert_eq!(obtenu, attendu, "STARTS WITH doit egaler str::starts_with");
    assert!(
        !obtenu.contains(&"selg".to_string()),
        "'selg' > 'self:' en octets mais ne commence pas par 'self:' : \
         une borne haute mal calculee le laisserait entrer"
    );
    assert!(
        obtenu.contains(&"self:".to_string()),
        "starts_with est reflexif : le prefixe exact est DEDANS (borne basse INCLUSIVE)"
    );
    assert!(
        !obtenu.contains(&"self;".to_string()),
        "';' est la borne haute : elle doit etre EXCLUSIVE"
    );
}

/// Le meme contrat, une fois l'index d'egalite/ordonne DECLARE.
///
/// `create_property_index` declare les deux structures d'un seul acte
/// (substrate/src/store.rs, `create_property_index` : « l'index ordonne se
/// declare par le meme acte »). C'est ce que fait la production sur
/// `qualified_name` et `name` (cli/src/mcp_server.rs:4659-4660). Le test
/// tourne sur une base SUBSTRATE SUR DISQUE : l'index ordonne n'existe que
/// la (`if self.index_ordonne.is_some()`).
#[test]
fn b_starts_with_egale_str_starts_with_avec_index_declare() {
    let rep = tempfile::tempdir().expect("tempdir");
    let db = ObrainDB::open(rep.path().join("base")).expect("ouverture substrate");
    {
        let s = db.session();
        for n in JEU_PREFIXE {
            s.create_node_with_props(&["S"], [("nom", Value::String(n.into()))]);
        }
    }
    db.create_property_index("nom");

    let mut attendu: Vec<String> = oracle_noms(&db)
        .into_iter()
        .filter(|n| n.starts_with("self:"))
        .collect();
    attendu.sort();

    let obtenu = textes_tries(
        &db,
        "MATCH (n:S) WHERE n.nom STARTS WITH 'self:' RETURN n.nom",
    );
    assert_eq!(
        obtenu, attendu,
        "index DECLARE : le resultat doit etre celui du balayage, a l'identique"
    );
}

/// Propriete NON declaree : `candidats_ordonnes` rend `None` et le store
/// reprend le balayage. Le repli doit etre INVISIBLE dans le resultat.
/// (Note de chantier : « il faut retomber proprement sur le balayage ».)
#[test]
fn b_repli_sur_balayage_quand_la_propriete_n_est_pas_declaree() {
    let rep = tempfile::tempdir().expect("tempdir");
    let db = ObrainDB::open(rep.path().join("base")).expect("ouverture substrate");
    {
        let s = db.session();
        for n in JEU_PREFIXE {
            s.create_node_with_props(&["S"], [("nom", Value::String(n.into()))]);
        }
    }
    // AUCUN create_property_index : c'est tout l'objet du test.

    let mut attendu: Vec<String> = oracle_noms(&db)
        .into_iter()
        .filter(|n| n.starts_with("self:"))
        .collect();
    attendu.sort();

    let obtenu = textes_tries(
        &db,
        "MATCH (n:S) WHERE n.nom STARTS WITH 'self:' RETURN n.nom",
    );
    assert_eq!(obtenu, attendu, "sans declaration : balayage, et meme reponse");
}

/// Prefixe VIDE : tout le monde commence par la chaine vide. Aucune borne
/// haute n'existe — une reecriture doit se DESISTER, pas inventer une borne.
#[test]
fn b_prefixe_vide_retient_toutes_les_chaines() {
    let db = db_noms(&JEU_PREFIXE);
    let mut attendu = oracle_noms(&db);
    attendu.sort();
    let obtenu = textes_tries(&db, "MATCH (n:S) WHERE n.nom STARTS WITH '' RETURN n.nom");
    assert_eq!(
        obtenu, attendu,
        "prefixe vide : TOUTES les chaines, la vide comprise"
    );
}

/// LE PIEGE DATETIME, et c'est le plus serieux de B.
///
/// `comparer_ordonnable` (substrate/src/store.rs) compare DEUX chaines
/// RFC3339 par INSTANT et non par octets. Si le prefixe est lui-meme
/// analysable en datetime, la borne basse cesse d'etre lexicographique :
/// `'2024-01-01T00:00:00.000Z'` est EGAL en instant a
/// `'2024-01-01T00:00:00Z'` — donc retenu par la borne basse inclusive —
/// alors qu'il ne COMMENCE PAS par ce prefixe. Une plage naive rendrait
/// une ligne de trop, en silence.
///
/// Contrat : le resultat reste `str::starts_with`. Une reecriture vers une
/// plage doit donc se DESISTER des que `instant_de_chaine` accepte le
/// prefixe OU sa borne haute.
#[test]
fn b_prefixe_datetime_ne_doit_pas_elargir_le_resultat() {
    let jeu = [
        "2024-01-01T00:00:00Z",
        "2024-01-01T00:00:00.000Z", // MEME INSTANT, prefixe DIFFERENT
        "2024-01-01T00:00:00Zsuite",
        "2024-01-01T00:00:01Z",
        "2023-12-31T23:59:59Z",
    ];
    let db = db_noms(&jeu);
    let prefixe = "2024-01-01T00:00:00Z";
    let mut attendu: Vec<String> = oracle_noms(&db)
        .into_iter()
        .filter(|n| n.starts_with(prefixe))
        .collect();
    attendu.sort();

    let obtenu = textes_tries(
        &db,
        &format!("MATCH (n:S) WHERE n.nom STARTS WITH '{prefixe}' RETURN n.nom"),
    );
    assert_eq!(
        obtenu, attendu,
        "prefixe RFC3339 : la comparaison par INSTANT ne doit pas elargir \
         STARTS WITH. '…00:00.000Z' a le meme instant mais un autre prefixe."
    );
    assert!(
        !obtenu.contains(&"2024-01-01T00:00:00.000Z".to_string()),
        "faux positif par egalite d'instant : exactement ce que la plage \
         naive introduirait"
    );
}

/// Valeurs NON-CHAINES sous un `STARTS WITH` : l'evaluateur fait
/// `as_str()?` et ne retient rien. Une plage a bornes chaines ne doit pas
/// davantage les retenir (`meme_classe_de_comparaison` les ecarte). Les
/// deux chemins doivent s'accorder sur « aucune ».
#[test]
fn b_valeurs_non_chaines_ne_sont_jamais_retenues() {
    let db = ObrainDB::new_in_memory();
    {
        let s = db.session();
        s.create_node_with_props(&["S"], [("nom", Value::String("self:ok".into()))]);
        s.create_node_with_props(&["S"], [("nom", Value::Int64(42))]);
        s.create_node_with_props(&["S"], [("nom", Value::Bool(true))]);
        s.create_node_with_props(&["S"], [("autre", Value::String("self:leurre".into()))]);
    }
    let obtenu = textes_tries(
        &db,
        "MATCH (n:S) WHERE n.nom STARTS WITH 'self:' RETURN n.nom",
    );
    assert_eq!(
        obtenu,
        vec!["self:ok".to_string()],
        "seule la CHAINE prefixee sort : ni l'entier, ni le booleen, ni le \
         noeud dont la propriete `nom` est absente"
    );
}

/// Chaine LONGUE (> 256 octets) : elle ne vit ni dans la chaine v2 ni dans
/// la carte heritee mais en COLONNE BLOB (le pavé de `find_nodes_in_range`
/// le documente). Le chemin blob doit rendre la meme reponse.
#[test]
fn b_chaine_longue_en_colonne_blob() {
    let longue = format!("self:{}", "x".repeat(400));
    let autre = format!("selg:{}", "x".repeat(400));
    let db = db_noms(&[&longue, &autre, "self:court"]);

    let mut attendu: Vec<String> = oracle_noms(&db)
        .into_iter()
        .filter(|n| n.starts_with("self:"))
        .collect();
    attendu.sort();

    let obtenu = textes_tries(
        &db,
        "MATCH (n:S) WHERE n.nom STARTS WITH 'self:' RETURN n.nom",
    );
    assert_eq!(
        obtenu, attendu,
        "chaine > 256 octets : le chemin blob doit rendre la meme reponse"
    );
    assert_eq!(obtenu.len(), 2, "la longue prefixee ET la courte prefixee");
}

/// DEBORDEMENT DE LA BORNE HAUTE. Le prefixe finit par le dernier point de
/// code possible : `char::MAX` n'a pas de successeur, donc aucune borne
/// haute ne peut etre construite. Une reecriture doit se desister ; le
/// resultat, lui, reste `str::starts_with`.
#[test]
fn b_prefixe_finissant_au_dernier_point_de_code() {
    let max = char::MAX; // U+10FFFF
    let prefixe = format!("a{max}");
    let dedans = format!("{prefixe}suite");
    let db = db_noms(&[&prefixe, &dedans, "a", "ab"]);

    let mut attendu: Vec<String> = oracle_noms(&db)
        .into_iter()
        .filter(|n| n.starts_with(&prefixe))
        .collect();
    attendu.sort();

    let obtenu = textes_tries(
        &db,
        &format!("MATCH (n:S) WHERE n.nom STARTS WITH '{prefixe}' RETURN n.nom"),
    );
    assert_eq!(
        obtenu, attendu,
        "prefixe finissant a char::MAX : pas de successeur, donc pas de \
         plage — la reponse doit rester exacte"
    );
    assert_eq!(obtenu.len(), 2, "le prefixe lui-meme et son extension");
}

/// Borne haute qui franchit une frontiere UTF-8 : le dernier point de code
/// du prefixe est juste avant la zone des SURROGATES (U+D7FF), dont le
/// successeur U+D800 n'est PAS un `char` valide. Incrementer un OCTET
/// produirait ici de l'UTF-8 invalide.
#[test]
fn b_borne_haute_au_bord_des_surrogates() {
    let avant = char::from_u32(0xD7FF).expect("U+D7FF est un point de code valide");
    assert!(
        char::from_u32(0xD800).is_none(),
        "U+D800 est un surrogate : pas de char, donc pas de borne haute triviale"
    );
    let prefixe = format!("k{avant}");
    let dedans = format!("{prefixe}z");
    let db = db_noms(&[&prefixe, &dedans, "k", "kz"]);

    let mut attendu: Vec<String> = oracle_noms(&db)
        .into_iter()
        .filter(|n| n.starts_with(&prefixe))
        .collect();
    attendu.sort();

    let obtenu = textes_tries(
        &db,
        &format!("MATCH (n:S) WHERE n.nom STARTS WITH '{prefixe}' RETURN n.nom"),
    );
    assert_eq!(obtenu, attendu, "bord des surrogates : reponse exacte exigee");
}

/// `STARTS WITH` SANS etiquette dans le motif : le chemin change (pas de
/// recoupement de label), la reponse ne doit pas changer.
#[test]
fn b_sans_etiquette_meme_reponse_qu_avec() {
    let db = db_noms(&JEU_PREFIXE);
    let avec = textes_tries(
        &db,
        "MATCH (n:S) WHERE n.nom STARTS WITH 'self:' RETURN n.nom",
    );
    let sans = textes_tries(&db, "MATCH (n) WHERE n.nom STARTS WITH 'self:' RETURN n.nom");
    assert_eq!(
        avec, sans,
        "tous les noeuds portent :S — les deux formes doivent coincider"
    );
}

/// `STARTS WITH` en CONJONCTION : une reecriture qui pousserait la plage
/// au scan sans conserver l'autre conjonctif rendrait des lignes en trop.
#[test]
fn b_conjonction_conserve_l_autre_predicat() {
    let db = ObrainDB::new_in_memory();
    {
        let s = db.session();
        for (nom, rang) in [("self:a", 1i64), ("self:b", 2), ("self:c", 3), ("autre", 2)] {
            s.create_node_with_props(
                &["S"],
                [
                    ("nom", Value::String(nom.into())),
                    ("rang", Value::Int64(rang)),
                ],
            );
        }
    }
    let obtenu = textes_tries(
        &db,
        "MATCH (n:S) WHERE n.nom STARTS WITH 'self:' AND n.rang > 1 RETURN n.nom",
    );
    assert_eq!(
        obtenu,
        vec!["self:b".to_string(), "self:c".to_string()],
        "le second conjonctif doit survivre a toute poussee de la plage"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// C — `ORDER BY … LIMIT k` -> tas borne a k
// ═══════════════════════════════════════════════════════════════════════

/// Le jeu de C : des cles DISTINCTES, pour que l'ordre soit entierement
/// determine et qu'une comparaison de SEQUENCES ait un sens.
fn db_versions(n: i64) -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    // Ordre d'insertion volontairement melange : un tas qui « oublierait »
    // de trier rendrait l'ordre d'insertion et serait pris en faute.
    // Les impairs DECROISSANTS puis les pairs CROISSANTS : une partition,
    // donc exactement `n` noeuds a cles DISTINCTES, pour tout `n`. (Un
    // `(0..n).rev().step_by(2)` serait faux : pour `n` impair il repasse sur
    // les PAIRS et fabrique des doublons.)
    let impairs = (0..n).filter(|i| i % 2 == 1).rev();
    let pairs = (0..n).filter(|i| i % 2 == 0);
    for i in impairs.chain(pairs) {
        s.create_node_with_props(
            &["S"],
            [
                ("nom", Value::String(format!("v{i:04}").into())),
                ("version", Value::Int64(i)),
            ],
        );
    }
    db
}

/// L'ORACLE de C : le tri INTEGRAL fait en Rust, puis la coupe a k. C'est
/// exactement « materialiser et trier l'ensemble » — la chose que la
/// reecriture veut eviter, gardee ici comme reference.
fn oracle_tri_coupe(db: &ObrainDB, decroissant: bool, k: usize) -> Vec<String> {
    let mut tout = oracle_noms(db);
    tout.sort();
    if decroissant {
        tout.reverse();
    }
    tout.truncate(k);
    tout
}

/// `ORDER BY … DESC LIMIT k` : memes lignes, MEME ORDRE, meme compte.
#[test]
fn c_order_by_desc_limit_egale_le_tri_integral_coupe() {
    let db = db_versions(200);
    for k in [1usize, 3, 10, 199, 200, 201] {
        let obtenu = textes(
            &db,
            &format!("MATCH (n:S) RETURN n.nom ORDER BY n.nom DESC LIMIT {k}"),
        );
        assert_eq!(
            obtenu,
            oracle_tri_coupe(&db, true, k),
            "ORDER BY DESC LIMIT {k} : sequence exacte exigee"
        );
    }
}

/// La meme chose en ordre CROISSANT.
#[test]
fn c_order_by_asc_limit_egale_le_tri_integral_coupe() {
    let db = db_versions(200);
    for k in [1usize, 3, 10, 199, 200, 201] {
        let obtenu = textes(
            &db,
            &format!("MATCH (n:S) RETURN n.nom ORDER BY n.nom ASC LIMIT {k}"),
        );
        assert_eq!(
            obtenu,
            oracle_tri_coupe(&db, false, k),
            "ORDER BY ASC LIMIT {k} : sequence exacte exigee"
        );
    }
}

/// `k` PLUS GRAND que le nombre de lignes : le tas ne doit rien inventer
/// ni rien perdre. Cas limite ou « borne a k » et « tout trier » coincident.
#[test]
fn c_limite_superieure_au_nombre_de_lignes() {
    let db = db_versions(7);
    let obtenu = textes(&db, "MATCH (n:S) RETURN n.nom ORDER BY n.nom DESC LIMIT 1000");
    assert_eq!(obtenu, oracle_tri_coupe(&db, true, 1000));
    assert_eq!(obtenu.len(), 7, "sept noeuds, sept lignes");
}

/// `SKIP` + `LIMIT` : un tas borne a `k` seul serait FAUX ici, il faut
/// `skip + limit` elements retenus. C'est l'erreur classique de la
/// materialisation tardive, et elle doit rester impossible.
#[test]
fn c_skip_et_limit_ensemble() {
    let db = db_versions(50);
    let obtenu = textes(
        &db,
        "MATCH (n:S) RETURN n.nom ORDER BY n.nom DESC SKIP 5 LIMIT 10",
    );
    let attendu: Vec<String> = oracle_tri_coupe(&db, true, 15).into_iter().skip(5).collect();
    assert_eq!(
        obtenu, attendu,
        "SKIP 5 LIMIT 10 : le tas doit retenir 15 elements, pas 10"
    );
}

/// EX-AEQUO. Avec des cles egales, l'ordre entre elles n'est pas promis ;
/// ce qui l'est, c'est l'ENSEMBLE (multi-ensemble) des cles rendues : ce
/// doivent etre les k plus grandes. Un tas dont la comparaison est mal
/// orientee sur l'egalite se ferait prendre ici.
#[test]
fn c_ex_aequo_rend_les_k_plus_grandes_cles() {
    let db = ObrainDB::new_in_memory();
    {
        let s = db.session();
        // Cinq cles, quatre porteurs chacune : tout k non multiple de 4
        // tombe au MILIEU d'un paquet d'ex-aequo.
        for cle in 0..5i64 {
            for porteur in 0..4 {
                s.create_node_with_props(
                    &["S"],
                    [
                        ("nom", Value::String(format!("c{cle}p{porteur}").into())),
                        ("cle", Value::Int64(cle)),
                    ],
                );
            }
        }
    }
    for k in [1usize, 2, 4, 6, 13, 20] {
        let lignes_obtenues = lignes(
            &db,
            &format!("MATCH (n:S) RETURN n.cle ORDER BY n.cle DESC LIMIT {k}"),
        );
        let mut cles: Vec<i64> = lignes_obtenues
            .iter()
            .map(|l| match &l[0] {
                Value::Int64(i) => *i,
                autre => panic!("cle non-entiere : {autre:?}"),
            })
            .collect();
        assert_eq!(cles.len(), k.min(20), "compte exact pour k={k}");

        // L'oracle : toutes les cles, triees decroissant, coupees a k.
        let mut toutes: Vec<i64> = (0..5i64).flat_map(|c| std::iter::repeat_n(c, 4)).collect();
        toutes.sort_unstable();
        toutes.reverse();
        toutes.truncate(k);

        // Les cles rendues DOIVENT deja etre en ordre decroissant…
        let mut ordonnees = cles.clone();
        ordonnees.sort_unstable();
        ordonnees.reverse();
        assert_eq!(cles, ordonnees, "les cles rendues doivent etre triees, k={k}");
        // …et former exactement le multi-ensemble des k plus grandes.
        cles.sort_unstable();
        toutes.sort_unstable();
        assert_eq!(
            cles, toutes,
            "ex-aequo : les k plus grandes cles, ni plus ni moins (k={k})"
        );
    }
}

/// NULL dans la cle de tri. GQL place les NULL en fin d'ordre croissant et
/// en tete d'ordre decroissant ; quelle que soit la convention du moteur,
/// elle ne doit pas DEPENDRE de la presence d'un `LIMIT`. On compare donc
/// la meme requete avec et sans coupe.
#[test]
fn c_nulls_places_pareil_avec_et_sans_limite() {
    let db = ObrainDB::new_in_memory();
    {
        let s = db.session();
        for i in 0..6i64 {
            s.create_node_with_props(
                &["S"],
                [
                    ("nom", Value::String(format!("n{i}").into())),
                    ("cle", Value::Int64(i)),
                ],
            );
        }
        // Trois noeuds SANS `cle` : la cle de tri y est NULL.
        for i in 0..3 {
            s.create_node_with_props(&["S"], [("nom", Value::String(format!("sans{i}").into()))]);
        }
    }
    for sens in ["ASC", "DESC"] {
        let complet = textes(
            &db,
            &format!("MATCH (n:S) RETURN n.nom ORDER BY n.cle {sens}, n.nom {sens}"),
        );
        for k in [1usize, 4, 7, 9] {
            let coupe = textes(
                &db,
                &format!("MATCH (n:S) RETURN n.nom ORDER BY n.cle {sens}, n.nom {sens} LIMIT {k}"),
            );
            assert_eq!(
                coupe,
                complet[..k.min(complet.len())].to_vec(),
                "ORDER BY {sens} LIMIT {k} doit etre le PREFIXE du tri complet, \
                 NULL compris — la place des NULL ne peut pas dependre du LIMIT"
            );
        }
    }
}

/// Tri sur PLUSIEURS cles avec coupe : la seconde cle doit departager, et
/// le prefixe rester celui du tri complet.
#[test]
fn c_tri_multi_cles_reste_un_prefixe_du_tri_complet() {
    let db = ObrainDB::new_in_memory();
    {
        let s = db.session();
        for a in 0..4i64 {
            for b in 0..4i64 {
                s.create_node_with_props(
                    &["S"],
                    [
                        ("nom", Value::String(format!("a{a}b{b}").into())),
                        ("a", Value::Int64(a)),
                        ("b", Value::Int64(b)),
                    ],
                );
            }
        }
    }
    let complet = textes(&db, "MATCH (n:S) RETURN n.nom ORDER BY n.a DESC, n.b ASC");
    for k in [1usize, 5, 15, 16] {
        let coupe = textes(
            &db,
            &format!("MATCH (n:S) RETURN n.nom ORDER BY n.a DESC, n.b ASC LIMIT {k}"),
        );
        assert_eq!(coupe, complet[..k.min(complet.len())].to_vec(), "k={k}");
    }
}

/// `ORDER BY` sur une expression PROJETEE (alias), avec coupe. Le tas doit
/// trier sur la valeur projetee, pas sur une propriete brute.
#[test]
fn c_order_by_sur_alias_projete() {
    let db = db_versions(40);
    let complet = lignes(
        &db,
        "MATCH (n:S) RETURN n.version AS v ORDER BY v DESC",
    );
    for k in [1usize, 5, 40] {
        let coupe = lignes(
            &db,
            &format!("MATCH (n:S) RETURN n.version AS v ORDER BY v DESC LIMIT {k}"),
        );
        assert_eq!(
            coupe,
            complet[..k.min(complet.len())].to_vec(),
            "ORDER BY sur alias, k={k}"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// D — `MATCH (n)` sans etiquette : la projection dans le scan
// ═══════════════════════════════════════════════════════════════════════

/// Le contrat minimal de D, pose d'avance : un scan sans etiquette voit
/// TOUS les noeuds, quelles que soient leurs etiquettes, y compris ceux
/// qui n'en portent AUCUNE — et une projection poussee dans le scan ne
/// doit ni en perdre ni en inventer.
#[test]
fn d_scan_sans_etiquette_voit_tous_les_noeuds() {
    let db = ObrainDB::new_in_memory();
    {
        let s = db.session();
        s.create_node_with_props(&["A"], [("nom", Value::String("a".into()))]);
        s.create_node_with_props(&["B"], [("nom", Value::String("b".into()))]);
        s.create_node_with_props(&["A", "B"], [("nom", Value::String("ab".into()))]);
        s.create_node_with_props(&[], [("nom", Value::String("nu".into()))]);
    }
    let tous = textes_tries(&db, "MATCH (n) RETURN n.nom");
    assert_eq!(
        tous,
        vec![
            "a".to_string(),
            "ab".to_string(),
            "b".to_string(),
            "nu".to_string()
        ],
        "le scan sans etiquette voit aussi le noeud SANS etiquette"
    );

    let compte = lignes(&db, "MATCH (n) RETURN count(n)");
    assert_eq!(
        compte[0][0],
        Value::Int64(4),
        "count(n) et la projection doivent compter les MEMES noeuds"
    );
}

/// Propriete ABSENTE sous une projection sans etiquette : la ligne existe
/// et la valeur est NULL — elle ne disparait pas. Une projection poussee
/// dans le scan qui sauterait les noeuds sans la propriete perdrait des
/// lignes.
#[test]
fn d_propriete_absente_donne_une_ligne_nulle_pas_zero_ligne() {
    let db = ObrainDB::new_in_memory();
    {
        let s = db.session();
        s.create_node_with_props(&["A"], [("nom", Value::String("a".into()))]);
        s.create_node_with_props(&["A"], [("autre", Value::Int64(1))]);
    }
    let l = lignes(&db, "MATCH (n) RETURN n.nom");
    assert_eq!(l.len(), 2, "deux noeuds, deux lignes");
    assert!(
        l.iter().any(|r| r[0] == Value::Null),
        "le noeud sans `nom` rend une ligne a NULL, pas zero ligne"
    );
}

/// C EST DEJA DEPLOYE — et ce test garde la CONDITION qui le declenche.
///
/// La note de chantier classe « materialisation tardive » comme non
/// deployee et impute 588 ms a un tri qui « trie TOUT avant de couper a
/// 10 ». Verification faite, c'est inexact : `SortOperator::with_limit` et
/// `bounded_top_k` existent dans obrain-core, avec leur preuve de
/// stabilite, et `plan_limit` les cable depuis le 2026-08-21
/// (planner/lpg/project.rs, pave « DESCENTE DE LA LIMITE DANS LE TRI »,
/// qui porte sa propre mesure d'avant : LIMIT 8 a 126,2 ms contre
/// LIMIT 20000 a 129,4 ms, rapport 0,98).
///
/// Mais ce cablage est CONDITIONNEL : il exige que l'entree du `Limit` soit
/// un `Sort` DIRECT. Un `Skip` ou un `Distinct` intercale le desactive, a
/// dessein. Si le traducteur venait a glisser un operateur entre les deux,
/// l'optimisation cesserait de s'appliquer EN SILENCE — sans qu'aucun test
/// de resultat ne rougisse, puisqu'elle ne change aucune sortie.
///
/// Ce test est donc le seul garde possible : il verifie la FORME du plan
/// logique pour la requete de production citee par le chantier.
#[test]
fn c_la_limite_descend_bien_dans_le_tri_pour_la_forme_de_production() {
    use obrain_engine::query::plan::LogicalOperator;

    let plan = obrain_engine::query::translate_gql(
        "MATCH (s:SelfState) RETURN s.qualified_name ORDER BY s.version DESC LIMIT 10",
    )
    .expect("traduction");

    let LogicalOperator::Limit(limite) = &plan.root else {
        panic!(
            "la racine doit etre un Limit, obtenue : {:?}",
            std::mem::discriminant(&plan.root)
        )
    };
    assert!(
        matches!(limite.input.as_ref(), LogicalOperator::Sort(_)),
        "L'ENTREE DU LIMIT DOIT ETRE UN SORT DIRECT : c'est la condition, \
         dans plan_limit, qui declenche `plan_sort(sort, Some(k))` puis \
         `SortOperator::with_limit(k)`. Tout operateur intercale desactive \
         le tri borne en silence."
    );
    assert_eq!(limite.count.value(), 10, "la limite doit valoir 10");
}

/// Et le corollaire mesurable sans chronometre : le tri borne ne change
/// AUCUNE sortie. Les tests `c_*` ci-dessus l'etablissent deja ligne a
/// ligne ; celui-ci le dit sur la forme exacte du chantier, ou la cle de
/// tri est une propriete ABSENTE de la projection — le cas qui force le
/// traducteur a injecter une colonne de tri synthetique puis a la retirer.
#[test]
fn c_cle_de_tri_hors_projection_reste_un_prefixe_du_tri_complet() {
    let db = db_versions(120);
    let complet = textes(&db, "MATCH (n:S) RETURN n.nom ORDER BY n.version DESC");
    for k in [1usize, 7, 119, 120] {
        let coupe = textes(
            &db,
            &format!("MATCH (n:S) RETURN n.nom ORDER BY n.version DESC LIMIT {k}"),
        );
        assert_eq!(
            coupe,
            complet[..k.min(complet.len())].to_vec(),
            "cle de tri hors projection, k={k}"
        );
    }
}
