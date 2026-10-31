//! EXISTENCE-INDEXÉE — `n.p IS NOT NULL` servi par la SÉRIE de propriété.
//!
//! Le défaut mesuré (2026-09-14, `runtime-dev`, 433 618 nœuds) :
//! `MATCH (n) WHERE n.id IS NOT NULL RETURN count(n)` — **514 ms, 433 633
//! slots lus, 110 lignes**. L'égalité exige une VALEUR à hacher, et une
//! existence n'en a pas : aucune route ne pouvait la servir, et le prédicat
//! restait un filtre d'expression au-dessus d'un balayage complet.
//!
//! Ce que la route (`try_plan_filter_with_property_existence`) ajoute est une
//! SOURCE de candidats, pas une vérité : la liste vient de la série d'une
//! propriété déclarée, puis chaque candidat est REVÉRIFIÉ sur sa valeur
//! courante. Ces tests sont le contrat qui l'autorise — et ils attrapent, dans
//! l'ordre, les pièges qui ont déjà coûté ailleurs dans ce moteur :
//!
//! 1. l'index cru SUR PAROLE : un nœud ajouté, ou supprimé, APRÈS la fusion
//!    doit entrer ou sortir du résultat ;
//! 2. le nœud SANS la propriété, qui ne doit jamais être rendu ;
//! 3. le résidu perdu : `n.p IS NOT NULL AND n.q >= 2` — la liste est exacte
//!    pour `p` SEUL, tout le reste doit rester filtré au-dessus ;
//! 4. l'étiquette du motif, qui doit être respectée par un test de bit ;
//! 5. la GARDE : au-delà du seuil de candidats, la route rend la main au
//!    balayage — et le résultat ne bouge pas d'une ligne.
//!
//!   cargo test -p obrain-engine --test existence_indexe

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use obrain_substrate::compteurs::mesurer_slots;

// ───────────────────────────── outillage ─────────────────────────────

/// Corpus : cinq `S` porteurs de `id`, un `S` SANS `id`, un `T` porteur.
/// Le sixième nœud est le piège permanent — une existence servie par un index
/// qui rendrait « tous les nœuds » le laisserait passer.
const AVEC_ID: &[(&str, i64)] = &[("a", 0), ("b", 1), ("c", 2), ("d", 3), ("e", 4)];

/// Sérialise les tests qui posent ou mesurent `OBRAIN_EXISTENCE_CANDIDATS_MAX`.
///
/// La variable est GLOBALE au processus : un test qui l'abaisse change le
/// scénario des autres. Le verrou n'est pas une élégance, c'est ce qui rend
/// l'affirmation « la route indexée n'a pas balayé » vraie dans SON scénario.
fn verrou_env() -> std::sync::MutexGuard<'static, ()> {
    static VERROU: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    VERROU
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn peupler(d: &ObrainDB) {
    let s = d.session();
    for (id, rang) in AVEC_ID {
        s.create_node_with_props(
            &["S"],
            [
                ("id", Value::String((*id).into())),
                ("rang", Value::Int64(*rang)),
            ],
        );
    }
    // Sans `id` : ne doit JAMAIS sortir d'une existence sur `id`.
    s.create_node_with_props(&["S"], [("rang", Value::Int64(-1))]);
    // Une autre étiquette, porteuse : le filtre d'étiquette doit la couper.
    s.create_node_with_props(
        &["T"],
        [("id", Value::String("t".into())), ("rang", Value::Int64(9))],
    );
}

fn ids(d: &ObrainDB, q: &str) -> Vec<String> {
    d.session()
        .execute(q)
        .unwrap_or_else(|e| panic!("requete refusee : {q}\n{e}"))
        .rows
        .into_iter()
        .map(|l| match &l[0] {
            Value::String(s) => s.to_string(),
            autre => panic!("valeur non-chaine : {autre:?}"),
        })
        .collect()
}

/// Les nœuds porteurs de `id`, triés — le témoin est TOUJOURS la liste
/// attendue écrite à la main, jamais le résultat d'avant.
fn attendus() -> Vec<String> {
    let mut v: Vec<String> = AVEC_ID.iter().map(|(id, _)| (*id).to_string()).collect();
    v.push("t".into());
    v.sort();
    v
}

const Q_TOUS: &str = "MATCH (n) WHERE n.id IS NOT NULL RETURN n.id ORDER BY n.id";

// ═══════════ 1. l'égalité scan-vs-index, même processus ═══════════

/// Le même corpus, la même requête : le balayage est le témoin, la route
/// indexée doit rendre la MÊME liste, au caractère près.
#[test]
fn le_resultat_indexe_est_celui_du_balayage() {
    let d = ObrainDB::new_in_memory();
    peupler(&d);
    let temoin = ids(&d, Q_TOUS);
    assert_eq!(temoin, attendus(), "le témoin lui-meme diverge");
    d.create_property_index("id");
    assert_eq!(ids(&d, Q_TOUS), temoin, "balayage vs index divergents");
}

/// Le nœud SANS la propriété ne sort jamais — ni avant, ni après l'index.
#[test]
fn le_noeud_sans_la_propriete_ne_sort_pas() {
    let d = ObrainDB::new_in_memory();
    peupler(&d);
    d.create_property_index("id");
    let r = ids(&d, Q_TOUS);
    assert_eq!(
        r.len(),
        6,
        "6 porteurs attendus, {} rendus : {r:?}",
        r.len()
    );
    assert!(!r.is_empty());
    // `rang = -1` est LE nœud sans `id` : il ne doit apparaître nulle part.
    //
    // LA REQUÊTE PORTE L'EXISTENCE, et ce n'est pas un détail de rédaction :
    // sans `n.id IS NOT NULL`, elle ne mesurait RIEN de ce que ce test annonce
    // — elle rendait une ligne dont la projection vaut NULL (comportement
    // CORRECT de GQL : une propriété absente se projette en NULL, la ligne
    // reste), et le contrôle échouait en accusant le nœud d'être « sorti ».
    // C'est le prédicat d'existence qui doit l'écarter, pas l'absence de valeur.
    let sans = ids(
        &d,
        "MATCH (n) WHERE n.id IS NOT NULL AND n.rang = -1 RETURN n.id",
    );
    assert!(sans.is_empty(), "le noeud sans `id` a rendu {sans:?}");
}

/// L'étiquette du motif est respectée : `:S` ne rend pas `t`, `:T` ne rend
/// que lui, une étiquette absente ne rend rien.
#[test]
fn l_etiquette_du_motif_est_respectee() {
    let d = ObrainDB::new_in_memory();
    peupler(&d);
    d.create_property_index("id");
    assert_eq!(
        ids(
            &d,
            "MATCH (n:S) WHERE n.id IS NOT NULL RETURN n.id ORDER BY n.id"
        ),
        vec!["a", "b", "c", "d", "e"]
    );
    assert_eq!(
        ids(
            &d,
            "MATCH (n:T) WHERE n.id IS NOT NULL RETURN n.id ORDER BY n.id"
        ),
        vec!["t"]
    );
    assert!(
        ids(&d, "MATCH (n:Absente) WHERE n.id IS NOT NULL RETURN n.id").is_empty(),
        "une etiquette absente a rendu des lignes"
    );
}

/// ── LE RÉSIDU ── `IS NOT NULL AND rang >= 2` : la liste indexée est exacte
/// pour `id` SEUL. Si le conjoint était perdu, le nombre de lignes
/// augmenterait — c'est exactement ce que ce test interdit.
#[test]
fn un_conjoint_n_est_jamais_perdu() {
    let d = ObrainDB::new_in_memory();
    peupler(&d);
    let q = "MATCH (n) WHERE n.id IS NOT NULL AND n.rang >= 2 \
             RETURN n.id ORDER BY n.id";
    let temoin = ids(&d, q);
    d.create_property_index("id");
    let indexe = ids(&d, q);
    assert_eq!(indexe, temoin, "le residu a ete perdu ou duplique");
    assert_eq!(indexe, vec!["c", "d", "e", "t"]);
}

/// Deux `IS NOT NULL` dans le même prédicat : l'un est servi, l'autre reste
/// au-dessus. Les deux doivent mordre.
#[test]
fn deux_existences_dans_le_meme_predicat() {
    let d = ObrainDB::new_in_memory();
    peupler(&d);
    let q = "MATCH (n) WHERE n.id IS NOT NULL AND n.rang IS NOT NULL \
             RETURN n.id ORDER BY n.id";
    let temoin = ids(&d, q);
    d.create_property_index("id");
    assert_eq!(ids(&d, q), temoin, "deux existences : resultat different");
}

/// `IS NULL` n'est PAS servi par cette route (il demande le COMPLÉMENT d'une
/// série, donc le store entier) : il doit rester juste par le chemin
/// d'origine — le nœud sans `id` sort, les autres non.
#[test]
fn is_null_reste_juste() {
    let d = ObrainDB::new_in_memory();
    peupler(&d);
    d.create_property_index("id");
    let q = "MATCH (n) WHERE n.id IS NULL RETURN count(n)";
    let n = d.session().execute(q).unwrap().rows[0][0].clone();
    assert_eq!(
        n,
        Value::Int64(1),
        "IS NULL doit rendre le seul noeud sans id"
    );
}

// ═══════════ 2. la preuve par les slots ═══════════

/// La charge machine rend les chronomètres menteurs ; le compteur de slots,
/// lui, est déterministe. Le balayage lit TOUS les slots ; la route indexée
/// n'en lit AUCUN — c'est la revérification qui tranche, pas un balayage.
#[test]
fn la_route_indexee_ne_balaie_pas_et_le_compteur_le_montre() {
    // ── CE TEST MESURE LA ROUTE, DONC IL PREND LE VERROU D'ENVIRONNEMENT ────
    // Il affirme `slots == 0` — la route indexée n'a rien balayé. Or un AUTRE
    // test du même binaire (`la_garde_rend_la_main_au_balayage`) pose
    // `OBRAIN_EXISTENCE_CANDIDATS_MAX=0`, ce qui fait PRÉCISÉMENT rendre la main
    // au balayage : si les deux se chevauchent, celui-ci lit un plafond qu'il
    // n'a pas posé et échoue sur une mesure qui ne décrit plus son scénario.
    // Course RÉELLE (les tests d'un même binaire tournent en parallèle), pas
    // encore observée en douze exécutions sous charge — un verrou coûte moins
    // cher qu'un flake qui apparaîtra un jour de CI chargée.
    let _verrou = verrou_env();

    // ── LA PRÉCONDITION, ÉCRITE ET CONSTRUITE (2026-09-16) ──────────────────
    //
    // Ce test mesurait la route indexée sur un store EN MÉMOIRE, après un seul
    // `create_property_index("id")` — et il échouait : 21 slots lus au lieu de 0,
    // c'est-à-dire un balayage complet. Ce n'était pas un défaut de la route,
    // mais une PRÉCONDITION non remplie : `find_nodes_with_property` exige une
    // série COUVERTE (`egalite_couverte`), que seule l'ouverture d'un store
    // portant une zone d'égalité persistée et fiable renseigne. Sur une base en
    // mémoire, `egalite_couverte` reste vide : la route rend `None` et le
    // balayage — correct, simplement pas accéléré.
    //
    // Le test construit donc sa précondition au lieu de la supposer : base sur
    // fichier, index déclaré, fermeture (la zone est persistée), réouverture
    // (la zone est chargée et couverte). C'est là, et seulement là, que « la
    // route indexée ne balaie pas » veut dire quelque chose.
    let dir = tempfile::TempDir::new().unwrap();
    let chemin = dir.path().join("base.obrain");

    // 1. Le TÉMOIN : sans index, la même requête balaie — et rend les mêmes lignes.
    {
        let d = ObrainDB::open(&chemin).unwrap();
        peupler(&d);
        let (temoin, slots_scan) = mesurer_slots(|| ids(&d, Q_TOUS));
        assert_eq!(temoin, attendus(), "le témoin lui-même diverge");
        assert!(
            slots_scan > 0,
            "le témoin devait balayer (slots = {slots_scan})"
        );

        // 2. L'INDEX, déclaré et PERSISTÉ.
        d.create_property_index("id");
        d.close().unwrap();
    }

    // 3. COUVERT après réouverture : la route peut servir la série.
    let d = ObrainDB::open(&chemin).unwrap();
    let (indexe, slots_index) = mesurer_slots(|| ids(&d, Q_TOUS));
    assert_eq!(indexe, attendus(), "le gain a changé la réponse");
    assert_eq!(
        slots_index, 0,
        "la route d'existence a balayé {slots_index} slots — elle devait lire la série seule"
    );
    d.close().unwrap();
}

// ═══════════ 3. la garde : jamais pire qu'un balayage ═══════════

/// `OBRAIN_EXISTENCE_CANDIDATS_MAX=0` abaisse le plafond à zéro candidat :
/// la route DOIT rendre la main au balayage. Le résultat ne bouge pas d'une
/// ligne, et les slots repassent au balayage complet — c'est la preuve que
/// la garde est branchée, et qu'elle renonce au lieu de mentir.
#[test]
fn la_garde_rend_la_main_au_balayage() {
    let _verrou = verrou_env();
    let d = ObrainDB::new_in_memory();
    peupler(&d);
    d.create_property_index("id");
    let (temoin, _) = mesurer_slots(|| ids(&d, Q_TOUS));
    unsafe { std::env::set_var("OBRAIN_EXISTENCE_CANDIDATS_MAX", "0") };
    let (garde, slots_garde) = mesurer_slots(|| ids(&d, Q_TOUS));
    unsafe { std::env::remove_var("OBRAIN_EXISTENCE_CANDIDATS_MAX") };
    assert_eq!(garde, temoin, "la garde a change la reponse");
    assert!(
        slots_garde > 0,
        "la garde devait rendre la main au balayage (slots = {slots_garde})"
    );
}

/// Le seuil par DÉFAUT est relatif (1 %, plancher 256) : un store de six
/// nœuds ne peut pas dépasser le plancher, donc la route sert. Une propriété
/// NON déclarée, elle, ne peut pas être servie du tout — et le résultat reste
/// celui du balayage.
#[test]
fn une_propriete_non_declaree_est_balayee() {
    let d = ObrainDB::new_in_memory();
    peupler(&d);
    let q = "MATCH (n) WHERE n.rang IS NOT NULL RETURN count(n)";
    let temoin = d.session().execute(q).unwrap().rows[0][0].clone();
    assert_eq!(temoin, Value::Int64(7), "7 porteurs de rang attendus");
    // Aucun index sur `rang` : la route ne peut rien promettre.
    let (_, slots) = mesurer_slots(|| d.session().execute(q).unwrap());
    assert!(slots > 0, "sans declaration, l'existence doit balayer");
}

// ═══════════ 4. la persistance, et l'index jamais cru sur parole ═══════════

/// Déclaration → PERSISTE → réouverture → la zone répond, et les écritures
/// POSTÉRIEURES à la fusion sont visibles : un nœud ajouté entre, un nœud
/// supprimé sort. C'est le piège « index cru sur parole », celui qui a déjà
/// rendu des listes incomplètes dans ce moteur.
#[test]
fn apres_reouverture_l_index_sert_et_les_ecritures_de_session_aussi() {
    let dir = tempfile::TempDir::new().unwrap();
    let chemin = dir.path().join("base.obrain");
    {
        let d = ObrainDB::open(&chemin).unwrap();
        peupler(&d);
        d.create_property_index("id");
        assert!(
            chemin.join("substrate.props.eq.idx.v2").exists(),
            "create_property_index doit persister la serie SUR-LE-CHAMP"
        );
        d.close().unwrap();
    }
    let d = ObrainDB::open(&chemin).unwrap();
    assert_eq!(ids(&d, Q_TOUS), attendus(), "apres reouverture");
    let s = d.session();
    s.execute("INSERT (:S {id: 'neuf', rang: 7})").unwrap();
    s.execute("MATCH (n {id: 'b'}) DETACH DELETE n").unwrap();
    let mut attendu = attendus();
    attendu.push("neuf".into());
    attendu.retain(|q| q != "b");
    attendu.sort();
    assert_eq!(
        ids(&d, Q_TOUS),
        attendu,
        "l'index a ete cru sur parole : delta de session perdu ou fantome rendu"
    );
    d.close().unwrap();
}
