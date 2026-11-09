//! CONTIENT-INTENTION — la primitive `db.contient(prop, aiguille)` contre GQL.
//!
//! Ce fichier est le CONTRAT de la primitive (chantier « l'outil paie le
//! texte ») : `ObrainDB::contient` doit rendre EXACTEMENT ce que rend
//! `MATCH (n) WHERE n.<prop> CONTAINS '<aiguille>'`, sur les deux routes
//! (index de trigrammes, balayage vérifié), y compris là où l'intuition se
//! trompe.
//!
//! ── LES PIÈGES QUE CE FICHIER EXISTE POUR ATTRAPER ──
//!
//! 1. le FAUX NÉGATIF de tokenisation : `ixgbe_read` matche `mixgbe_ready`
//!    PAR SOUS-CHAÎNE ; aucun découpage en mots ne le voit ;
//! 2. la VALEUR NON TEXTUELLE : l'évaluateur fait `as_str()?`, donc un
//!    `Int64` ne matche JAMAIS — la primitive ne doit pas inventer de ligne ;
//! 3. l'AIGUILLE VIDE : `''` est contenu dans toute chaîne (donc toute
//!    valeur CHAÎNE sort, aucune autre) ;
//! 4. les OCTETS UTF-8 MULTI-OCTETS (une aiguille qui tombe au milieu d'un
//!    caractère ne matche rien, une qui tombe juste matche) ;
//! 5. l'ÉCRITURE APRÈS COUP : un nœud supprimé doit sortir, un nœud créé doit
//!    entrer — l'index n'est jamais cru sur parole ;
//! 6. l'INDEX MUET (aiguille dont tous les grammes sont communs) : la route
//!    change, le résultat ne change pas.
//!
//!   cargo test -p obrain-engine --test contient_intention

use std::collections::BTreeSet;

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use obrain_engine::database::QueryResult;
use obrain_engine::database::contient::RouteContient;

const PROP: &str = "txt";

/// Corpus des cas tordus, plus une valeur NON CHAÎNE (jamais rendue).
const CORPUS: &[&str] = &[
    "drivers.net.ixgbe.ixgbe_read_reg",
    "drivers.net.ixgbe.ixgbe_read_pci_cfg_word",
    "drivers.net.mixgbe_ready.helper", // « ixgbe_read » À CHEVAL sur un séparateur
    "obrain-iam.token.verifier",
    "obrain-iam",
    "r\u{e9}acteur_principal", // multi-octets : « é » = 2 octets
    "module_sans_rapport",
    "", // la chaîne VIDE est une chaîne : elle contient ''
];

/// Sérialise les tests qui posent `OBRAIN_TEXTE_CANDIDATS_MAX` (variable GLOBALE
/// au processus) : sans cela, un test peut lire le plafond posé par un autre et
/// échouer sur une mesure qui ne décrit plus son scénario. Même remède que dans
/// `existence_indexe.rs`.
fn verrou_env() -> std::sync::MutexGuard<'static, ()> {
    static VERROU: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    VERROU
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn peupler(d: &ObrainDB) {
    let s = d.session();
    for (i, t) in CORPUS.iter().enumerate() {
        s.create_node_with_props(
            &["S"],
            [
                (PROP, Value::String((*t).into())),
                ("rang", Value::Int64(i as i64)),
            ],
        );
    }
    // Valeur NON textuelle : `as_str()?` rend NULL, donc aucune ligne — même
    // pour l'aiguille vide.
    s.create_node_with_props(&["S"], [(PROP, Value::Int64(7))]);
}

/// Les identifiants de la primitive, dans l'ordre.
fn ids_primitive(d: &ObrainDB, aiguille: &str) -> Vec<i64> {
    let r = d
        .contient(PROP, aiguille)
        .unwrap_or_else(|e| panic!("primitive refusee ({aiguille:?}) : {e}"));
    identifiants(&r)
}

fn identifiants(r: &QueryResult) -> Vec<i64> {
    r.rows
        .iter()
        .map(|l| {
            l[0].as_int64().unwrap_or_else(|| {
                panic!("la primitive doit rendre des identifiants, pas {:?}", l[0])
            })
        })
        .collect()
}

/// Le témoin : GQL, et rien d'autre.
fn ids_gql(d: &ObrainDB, aiguille: &str) -> Vec<i64> {
    // (Pas de `ORDER BY id(n)` : le moteur refuse un ORDER BY sur un appel de
    // fonction — « Function column 'id' not found for ORDER BY » — et ce n'est
    // pas le sujet de ce fichier. On trie ici, ce qui rend la comparaison
    // indépendante de l'ordre de rendu des deux routes.)
    let q = format!("MATCH (n) WHERE n.{PROP} CONTAINS '{aiguille}' RETURN id(n)");
    let r = d
        .session()
        .execute(&q)
        .unwrap_or_else(|e| panic!("requete refusee : {q}\n{e}"));
    let mut v = identifiants(&r);
    v.sort_unstable();
    v
}

/// Le compte, par la forme EXACTE de la mission.
fn compte_gql(d: &ObrainDB, aiguille: &str) -> i64 {
    let q = format!("MATCH (n) WHERE n.{PROP} CONTAINS '{aiguille}' RETURN count(n)");
    let r = d.session().execute(&q).expect("count");
    match r.rows.first().and_then(|l| l.first()) {
        Some(Value::Int64(c)) => *c,
        autre => panic!("count(n) doit rendre un entier, pas {autre:?}"),
    }
}

/// L'oracle calculé à la main, jamais par une réimplémentation du moteur : on
/// filtre le corpus avec `str::contains` — la sémantique que la documentation
/// de la primitive revendique.
fn oracle(aiguille: &str) -> BTreeSet<&'static str> {
    CORPUS
        .iter()
        .filter(|t| t.contains(aiguille))
        .copied()
        .collect()
}

const AIGUILLES: &[&str] = &[
    "ixgbe_read", // sous-chaîne à cheval — le piège de tokenisation
    "obrain-iam", // le terme de la mesure
    "iam",
    "",
    "\u{e9}acteur", // aiguille multi-octets entière
    "\u{e9}",       // un octet de tête seul, valide en UTF-8
    "zzqq-absent",  // absence : zéro ligne, sans balayer
    "ion",          // tous les grammes communs : l'index se TAIT
    ".",            // ponctuation banale
];

// ══════════════ 1. l'égalité primitive-vs-GQL, sur les deux routes ══════════════

#[test]
fn la_primitive_rend_ce_que_gql_rend() {
    let d = ObrainDB::new_in_memory();
    peupler(&d);
    for aiguille in AIGUILLES {
        let attendu = ids_gql(&d, aiguille);
        let obtenu = ids_primitive(&d, aiguille);
        assert_eq!(obtenu, attendu, "primitive ≠ GQL sur {aiguille:?}");
        assert_eq!(
            obtenu.len() as i64,
            compte_gql(&d, aiguille),
            "le compte de la primitive differe de count(n) sur {aiguille:?}"
        );
        // Et l'oracle indépendant, pour que deux erreurs identiques ne se
        // congratulent pas : la primitive doit aussi égaler `str::contains`.
        let attendu_textes = oracle(aiguille);
        let r = d.contient(PROP, aiguille).expect("primitive");
        let textes: BTreeSet<String> = r
            .rows
            .iter()
            .map(|l| {
                let id = l[0].as_int64().expect("identifiant") as u64;
                d.session()
                    .execute(&format!("MATCH (n) WHERE id(n) = {id} RETURN n.{PROP}"))
                    .expect("relecture")
                    .rows
                    .first()
                    .and_then(|l| l.first())
                    .and_then(|v| v.as_str().map(str::to_string))
                    .expect("valeur texte")
            })
            .collect();
        let attendu_owned: BTreeSet<String> =
            attendu_textes.iter().map(|s| (*s).to_string()).collect();
        assert_eq!(
            textes, attendu_owned,
            "oracle `str::contains` ≠ primitive sur {aiguille:?}"
        );
    }
}

/// La valeur NON textuelle ne sort jamais — pas même pour l'aiguille vide.
#[test]
fn une_valeur_non_textuelle_ne_matche_jamais() {
    let d = ObrainDB::new_in_memory();
    peupler(&d);
    let vide = d.contient(PROP, "").expect("primitive");
    let textes: Vec<Value> = vide.rows.iter().map(|l| l[0].clone()).collect();
    assert_eq!(
        textes.len(),
        CORPUS.len(),
        "l'aiguille vide doit rendre les {} valeurs CHAÎNE (et rien d'autre)",
        CORPUS.len()
    );
}

// ══════════════ 2. index contre balayage : même réponse, route nommée ══════════════

#[test]
#[allow(unsafe_code)] // set_var/remove_var sont unsafe en édition 2024 ; `verrou_env` sérialise l'accès
fn la_route_change_la_reponse_non() {
    // Ce test POSE le plafond à 0 (route de balayage forcée) : il prend le verrou.
    let _verrou = verrou_env();
    let d = ObrainDB::new_in_memory();
    peupler(&d);
    // Déclarer la propriété, puis deux appels : le substrat construit son index
    // de trigrammes à la 2e demande (`OBRAIN_TRIGRAMMES_DES_LA`).
    d.create_property_index(PROP);
    for aiguille in AIGUILLES {
        let _ = d.contient(PROP, aiguille);
    }

    for aiguille in AIGUILLES {
        let reference = ids_gql(&d, aiguille);
        let (r1, route1) = d
            .contient_avec_route(PROP, aiguille, None)
            .map(|r| (identifiants(&r.result), r.route))
            .expect("primitive");
        let (r2, route2) = d
            .contient_avec_route(PROP, aiguille, None)
            .map(|r| (identifiants(&r.result), r.route))
            .expect("primitive");
        assert_eq!(
            r1,
            reference,
            "route {} ≠ GQL sur {aiguille:?}",
            route1.nom()
        );
        assert_eq!(
            r2,
            reference,
            "route {} ≠ GQL sur {aiguille:?}",
            route2.nom()
        );
        assert_eq!(
            r1, r2,
            "deux appels identiques ont divergé sur {aiguille:?}"
        );
    }

    // Sur un terme SÉLECTIF, l'index doit servir (sinon la primitive n'aurait
    // aucun intérêt) ; et la réponse ne doit pas dépendre de la route.
    let selective = d
        .contient_avec_route(PROP, "ixgbe_read", None)
        .expect("primitive")
        .route;
    assert_eq!(
        selective,
        RouteContient::Index,
        "un terme sélectif doit être servi par l'index"
    );

    // ── POURQUOI ON NE PROUVE PAS « UN TERME BANAL PASSE AU BALAYAGE » ICI ──
    //
    // Première version de ce test : elle affirmait qu'une aiguille dont tous
    // les grammes sont communs (« ion ») rendait la main au balayage. FAUX sur
    // un corpus de huit valeurs — aucun gramme n'y dépasse le seuil de rareté,
    // donc TOUS sont indexés et l'index répond. La route dépend du CORPUS, pas
    // du terme : c'est précisément pourquoi la réponse, elle, ne doit pas en
    // dépendre.
    //
    // On force donc la route de balayage par la garde de coût (plafond à 0) et
    // on exige la MÊME réponse des deux routes, sur toutes les aiguilles.
    for aiguille in AIGUILLES {
        let reference = ids_gql(&d, aiguille);
        unsafe { std::env::set_var("OBRAIN_TEXTE_CANDIDATS_MAX", "0") };
        let force = d
            .contient_avec_route(PROP, aiguille, None)
            .map(|r| (identifiants(&r.result), r.route));
        unsafe { std::env::remove_var("OBRAIN_TEXTE_CANDIDATS_MAX") };
        let (ids_balayage, route_balayage) = force.expect("primitive");
        // Un index qui rend une liste VIDE ne coûte rien et PROUVE l'absence :
        // il garde la main même sous un plafond nul (c'est aussi la règle du
        // planificateur, `trouves.len() > plafond`). Ce n'est donc un balayage
        // forcé que là où il y a effectivement des candidats à examiner.
        if reference.is_empty() {
            assert_eq!(
                route_balayage,
                RouteContient::Index,
                "une absence prouvee par l'index ne doit pas balayer ({aiguille:?})"
            );
        } else {
            assert_eq!(
                route_balayage,
                RouteContient::Balayage,
                "le plafond a 0 doit forcer le balayage ({aiguille:?})"
            );
        }
        assert_eq!(
            ids_balayage, reference,
            "les deux routes divergent sur {aiguille:?}"
        );
    }
}

/// La garde de coût est celle du planificateur : au-delà du plafond, on balaie.
/// On abaisse le plafond par la variable d'environnement de MESURE pour forcer
/// la bascule sur un corpus minuscule.
#[allow(unsafe_code)] // set_var/remove_var sont unsafe en édition 2024 ; `verrou_env` sérialise l'accès
#[test]
fn la_garde_de_cout_rend_la_main_au_balayage() {
    let _verrou = verrou_env();
    let d = ObrainDB::new_in_memory();
    peupler(&d);
    d.create_property_index(PROP);
    for aiguille in AIGUILLES {
        let _ = d.contient(PROP, aiguille);
    }
    // SAFETY : le verrou d'environnement est tenu (voir `verrou_env`) — la
    // variable est GLOBALE au processus, et les tests d'un même binaire tournent
    // en parallèle : la justification « test mono-thread » qui figurait ici était
    // FAUSSE, et c'est exactement ce qui rendait la course possible.
    unsafe { std::env::set_var("OBRAIN_TEXTE_CANDIDATS_MAX", "0") };
    let (r, route) = d
        .contient_avec_route(PROP, "ixgbe_read", None)
        .map(|r| (identifiants(&r.result), r.route))
        .expect("primitive");
    unsafe { std::env::remove_var("OBRAIN_TEXTE_CANDIDATS_MAX") };
    assert_eq!(route, RouteContient::Balayage, "plafond à 0 ⇒ balayage");
    assert_eq!(
        r,
        ids_gql(&d, "ixgbe_read"),
        "le balayage doit rendre les mêmes lignes"
    );
}

// ══════════════ 3. les écritures d'après : rien n'est cru sur parole ══════════════

#[test]
fn une_ecriture_apres_coup_est_vue() {
    let d = ObrainDB::new_in_memory();
    peupler(&d);
    d.create_property_index(PROP);
    let _ = d.contient(PROP, "iam");
    let avant = ids_primitive(&d, "iam");
    assert_eq!(avant, ids_gql(&d, "iam"));

    // Un nœud qui matche, créé APRÈS la construction de l'index.
    let nouveau = d
        .session()
        .create_node_with_props(&["S"], [(PROP, Value::String("obrain-iam.neuf".into()))]);
    // Et un nœud qui matche, SUPPRIMÉ après.
    let id_supprime = {
        let gql = ids_gql(&d, "r\u{e9}acteur");
        assert_eq!(
            gql.len(),
            1,
            "le corpus doit porter exactement un « réacteur »"
        );
        gql[0]
    };
    d.session()
        .execute(&format!("MATCH (n) WHERE id(n) = {id_supprime} DELETE n"))
        .expect("suppression");

    let apres = ids_primitive(&d, "iam");
    assert_eq!(apres, ids_gql(&d, "iam"), "apres ecriture, primitive ≠ GQL");
    assert_eq!(
        apres.len(),
        avant.len() + 1,
        "le nœud créé doit entrer (identifiant {nouveau:?}) et rien d'autre"
    );
    assert!(
        !ids_primitive(&d, "r\u{e9}acteur").contains(&id_supprime),
        "un nœud supprimé ne doit plus sortir"
    );
    assert_eq!(
        ids_primitive(&d, "r\u{e9}acteur"),
        ids_gql(&d, "r\u{e9}acteur"),
        "primitive ≠ GQL après suppression"
    );
}

// ══════════════ 4. le plafond : un préfixe déterministe de la liste ══════════════

#[test]
fn la_limite_est_un_prefixe_de_la_liste_complete() {
    let d = ObrainDB::new_in_memory();
    peupler(&d);
    for aiguille in AIGUILLES {
        let complet = d.contient(PROP, aiguille).expect("primitive");
        let total = identifiants(&complet);
        for k in [0usize, 1, 3, total.len()] {
            let borne = d.contient_limite(PROP, aiguille, k).expect("primitive");
            assert_eq!(
                identifiants(&borne),
                total[..k.min(total.len())].to_vec(),
                "la limite {k} n'est pas le prefixe de la liste complete ({aiguille:?})"
            );
        }
    }
}

/// Le contrat de retour est celui de l'existant : `columns`, `rows`, et une
/// mesure quand elle existe.
#[test]
fn le_contrat_de_retour_est_celui_de_l_existant() {
    let d = ObrainDB::new_in_memory();
    peupler(&d);
    let r = d.contient(PROP, "iam").expect("primitive");
    assert_eq!(r.columns, vec!["n".to_string()]);
    assert_eq!(r.column_types.len(), r.columns.len());
    assert!(
        r.execution_time_ms.is_some(),
        "la primitive mesure son execution"
    );
    assert!(
        r.rows_scanned.is_some(),
        "la primitive dit ce qu'elle a examine"
    );
    assert_eq!(r.rows.len(), ids_gql(&d, "iam").len());
}
