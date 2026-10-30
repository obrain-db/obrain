//! VOISINAGE-ENTRANT — l'egalite du resultat, AVANT la vitesse.
//!
//! Chantier : « une traversee entrante ne doit pas couter 1,7 million de
//! fois la sortante ». Mesure du superviseur sur runtime-dev :
//!
//!   SORTANTE  MATCH (a {qualified_name:'QN'})-[r]->(b) ... →      0,19 ms
//!   ENTRANTE  MATCH (a)-[r]->(b {qualified_name:'QN'}) ... → 319 572 ms
//!
//! La cause : le traducteur ancre TOUJOURS sur le noeud SOURCE du motif
//! (translate_path_pattern_with_alias) ; la contrainte du noeud CIBLE
//! devient un Filter POSE AU-DESSUS de l'Expand, que le planificateur
//! physique ne sait pas re-ancrer. Le plan balaie donc tous les noeuds.
//!
//! La reecriture visee : reconnaitre Filter(pred-sur-b, Expand{a->b, 1 saut,
//! NodeScan(a) libre}) et re-planifier ancre sur b (seek indexe) avec
//! l'Expand INVERSE (Direction::Incoming, natif dans le substrat).
//!
//! CE FICHIER N'OPTIMISE RIEN. Il pose le contrat d'egalite que la
//! reecriture devra tenir — et il doit etre VERT sur le code d'AVANT,
//! ou il ne prouve rien.
//!
//! ── LA FORME DES TESTS : UN ORACLE INDEPENDANT DU MOTEUR ──
//!
//! L'oracle n'est PAS une autre requete : c'est la liste des aretes
//! construite en Rust au moment de l'insertion. Aucun plan du moteur,
//! ancien ou nouveau, ne participe a son calcul. Avant la reecriture le
//! test compare le balayage a cette liste ; apres, le seek a la meme
//! liste. La MEME assertion garde les deux regimes.
//!
//! Les requetes sans ORDER BY comparent des ENSEMBLES (l'ordre n'est pas
//! promis) ; celles avec ORDER BY comparent des SEQUENCES.
//!
//!   cargo test -p obrain-engine --test voisinage_entrant

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use obrain_substrate::compteurs::mesurer_slots;

// ───────────────────────────── outillage ─────────────────────────────

/// La base ET sa verite : chaque arete inseree est journalisee en Rust
/// sous la forme (nom_source, type, nom_cible, flag_source).
struct Terrain {
    db: ObrainDB,
    /// (nom de a, type de r, nom de b, a.flag) pour CHAQUE arete creee.
    aretes: Vec<(String, String, String, bool)>,
}

/// Le graphe de reference du chantier :
///  - 200 noeuds :P {nom: p000..p199, flag: pair}
///  - 1 hub    :C {nom: 'hub',   qualified_name: 'cible'}
///  - 1 leurre :C {nom: 'leurre', qualified_name: 'autre'}
///  - p000..p119 -[:T1]-> hub        (120 entrantes T1)
///  - p120..p149 -[:T2]-> hub        ( 30 entrantes T2)
///  - leurre     -[:T3]-> hub        (  1 entrante T3, source elle-meme qualifiee)
///  - p150..p169 -[:T1]-> leurre     ( 20 entrantes vers l'AUTRE cible)
///  - hub        -[:T1]-> p190..p194 (  5 SORTANTES du hub — jamais dans l'entrant)
///  - p100..p109 -[:T1]-> p000..p009 ( 10 aretes pour les chemins a 2 sauts)
fn terrain() -> Terrain {
    let db = ObrainDB::new_in_memory();
    db.create_property_index("qualified_name");
    let s = db.session();
    let mut aretes = Vec::new();

    let mut ps = Vec::new();
    let mut flags = Vec::new();
    for i in 0..200 {
        let flag = i % 2 == 0;
        let n = s.create_node_with_props(
            &["P"],
            [
                ("nom", Value::String(format!("p{i:03}").into())),
                ("flag", Value::Bool(flag)),
            ],
        );
        ps.push(n);
        flags.push(flag);
    }
    let hub = s.create_node_with_props(
        &["C"],
        [
            ("nom", Value::String("hub".into())),
            ("qualified_name", Value::String("cible".into())),
        ],
    );
    let leurre = s.create_node_with_props(
        &["C"],
        [
            ("nom", Value::String("leurre".into())),
            ("qualified_name", Value::String("autre".into())),
        ],
    );

    let nom = |i: usize| format!("p{i:03}");

    for i in 0..120 {
        s.create_edge(ps[i], hub, "T1");
        aretes.push((nom(i), "T1".into(), "hub".into(), flags[i]));
    }
    for i in 120..150 {
        s.create_edge(ps[i], hub, "T2");
        aretes.push((nom(i), "T2".into(), "hub".into(), flags[i]));
    }
    s.create_edge(leurre, hub, "T3");
    aretes.push(("leurre".into(), "T3".into(), "hub".into(), false));
    for i in 150..170 {
        s.create_edge(ps[i], leurre, "T1");
        aretes.push((nom(i), "T1".into(), "leurre".into(), flags[i]));
    }
    for i in 190..195 {
        s.create_edge(hub, ps[i], "T1");
        aretes.push(("hub".into(), "T1".into(), nom(i), false));
    }
    for i in 0..10 {
        s.create_edge(ps[100 + i], ps[i], "T1");
        aretes.push((nom(100 + i), "T1".into(), nom(i), flags[100 + i]));
    }

    Terrain { db, aretes }
}

impl Terrain {
    /// L'oracle entrant : (nom de a, type de r) de chaque arete dont la
    /// cible est `cible`, filtre optionnel sur a.flag. Trie pour la
    /// comparaison d'ENSEMBLES.
    fn entrantes(&self, cible: &str, flag: Option<bool>) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self
            .aretes
            .iter()
            .filter(|(_, _, b, f)| b == cible && flag.is_none_or(|want| *f == want))
            .map(|(a, t, _, _)| (a.clone(), t.clone()))
            .collect();
        v.sort();
        v
    }

    /// L'oracle sortant : (type de r, nom de b) de chaque arete dont la
    /// source est `source`.
    fn sortantes(&self, source: &str) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self
            .aretes
            .iter()
            .filter(|(a, _, _, _)| a == source)
            .map(|(_, t, b, _)| (t.clone(), b.clone()))
            .collect();
        v.sort();
        v
    }
}

fn lignes(db: &ObrainDB, q: &str) -> Vec<Vec<Value>> {
    db.session()
        .execute(q)
        .unwrap_or_else(|e| panic!("requete refusee : {q}\n{e}"))
        .rows
}

/// Deux premieres colonnes en texte, TELLES QUE rendues.
fn paires(db: &ObrainDB, q: &str) -> Vec<(String, String)> {
    lignes(db, q)
        .into_iter()
        .map(|l| (texte(&l[0]), texte(&l[1])))
        .collect()
}

/// Les memes, triees : pour les requetes SANS ORDER BY, seule l'egalite
/// des ENSEMBLES est un contrat.
fn paires_triees(db: &ObrainDB, q: &str) -> Vec<(String, String)> {
    let mut v = paires(db, q);
    v.sort();
    v
}

fn texte(v: &Value) -> String {
    match v {
        Value::String(s) => s.to_string(),
        autre => panic!("valeur non-chaine : {autre:?}"),
    }
}

// ══════════════ 1. LA REQUETE DU DEFAUT : entrante, b contraint ══════════════

#[test]
fn entrante_b_contraint_a_libre() {
    let t = terrain();
    let obtenu = paires_triees(
        &t.db,
        "MATCH (a)-[r]->(b {qualified_name: 'cible'}) RETURN a.nom, type(r)",
    );
    assert_eq!(
        obtenu,
        t.entrantes("hub", None),
        "l'entrante ancree sur b doit rendre EXACTEMENT les aretes dont la \
         cible est le hub : 120 T1 + 30 T2 + 1 T3, jamais les sortantes du \
         hub ni les entrantes du leurre"
    );
    assert_eq!(obtenu.len(), 151, "temoin de la fixture");
}

#[test]
fn entrante_syntaxe_inversee_meme_ensemble() {
    // (b {qn})<-[r]-(a) ancre b comme SOURCE syntaxique : ce plan-la est
    // deja ancre correctement aujourd'hui. Les deux ecritures du MEME
    // motif doivent rendre le MEME ensemble — c'est l'egalite que la
    // reecriture devra reproduire.
    let t = terrain();
    let directe = paires_triees(
        &t.db,
        "MATCH (a)-[r]->(b {qualified_name: 'cible'}) RETURN a.nom, type(r)",
    );
    let inversee = paires_triees(
        &t.db,
        "MATCH (b {qualified_name: 'cible'})<-[r]-(a) RETURN a.nom, type(r)",
    );
    assert_eq!(directe, inversee, "meme motif, deux ecritures, meme ensemble");
}

#[test]
fn entrante_ordre_specifie_est_une_sequence() {
    // ORDER BY a.nom seulement : chaque source n'a qu'UNE arete vers le
    // hub, l'ordre sur nom est donc total. (`ORDER BY type(r)` n'est pas
    // supporte par le moteur — contrainte relevee sur le code d'avant.)
    let t = terrain();
    let obtenu = paires(
        &t.db,
        "MATCH (a)-[r]->(b {qualified_name: 'cible'}) RETURN a.nom, type(r) \
         ORDER BY a.nom",
    );
    let oracle = t.entrantes("hub", None); // triee (nom, type), noms uniques
    assert_eq!(obtenu, oracle, "ORDER BY specifie : meme SEQUENCE, pas seulement meme ensemble");
}

// ══════════════ 2. LIMIT : le plafond ne doit jamais mentir ══════════════

#[test]
fn entrante_limit_rend_k_lignes_du_bon_ensemble() {
    let t = terrain();
    let obtenu = paires(
        &t.db,
        "MATCH (a)-[r]->(b {qualified_name: 'cible'}) RETURN a.nom, type(r) LIMIT 30",
    );
    let oracle = t.entrantes("hub", None);
    assert_eq!(obtenu.len(), 30, "151 aretes entrantes, LIMIT 30 : 30 lignes");
    for ligne in &obtenu {
        assert!(
            oracle.contains(ligne),
            "chaque ligne rendue sous LIMIT doit appartenir a l'ensemble exact : {ligne:?}"
        );
    }
}

#[test]
fn entrante_limit_derriere_filtre_aval_doit_se_desarmer() {
    // LE PIEGE SCAN, version entrante : un WHERE sur a REDUIT les lignes.
    // Couper a k ARETES avant le filtre rendrait moins de k LIGNES.
    // a.flag est vrai une arete sur deux : 75 des 150 p-aretes passent.
    let t = terrain();
    let obtenu = paires(
        &t.db,
        "MATCH (a)-[r]->(b {qualified_name: 'cible'}) WHERE a.flag = true \
         RETURN a.nom, type(r) LIMIT 30",
    );
    let oracle = t.entrantes("hub", Some(true));
    assert!(
        oracle.len() >= 30,
        "temoin : au moins 30 aretes passent le filtre (reel : {})",
        oracle.len()
    );
    assert_eq!(
        obtenu.len(),
        30,
        "un LIMIT 30 derriere un filtre sur a doit rendre 30 lignes : le \
         plafond se desarme, il ne coupe pas a 30 aretes AVANT le filtre"
    );
    for ligne in &obtenu {
        assert!(oracle.contains(ligne), "ligne hors ensemble filtre : {ligne:?}");
    }
}

#[test]
fn entrante_filtre_aval_sans_limit_ensemble_exact() {
    let t = terrain();
    let obtenu = paires_triees(
        &t.db,
        "MATCH (a)-[r]->(b {qualified_name: 'cible'}) WHERE a.flag = true \
         RETURN a.nom, type(r)",
    );
    assert_eq!(obtenu, t.entrantes("hub", Some(true)));
}

// ══════════════ 3. LES VARIANTES QUI NE DOIVENT PAS DERAILLER ══════════════

#[test]
fn entrante_type_d_arete_contraint() {
    let t = terrain();
    let obtenu = paires_triees(
        &t.db,
        "MATCH (a)-[r:T1]->(b {qualified_name: 'cible'}) RETURN a.nom, type(r)",
    );
    let oracle: Vec<(String, String)> = t
        .entrantes("hub", None)
        .into_iter()
        .filter(|(_, ty)| ty == "T1")
        .collect();
    assert_eq!(obtenu, oracle, "120 aretes T1 seulement — ni T2 ni T3");
    assert_eq!(obtenu.len(), 120, "temoin de la fixture");
}

#[test]
fn entrante_label_sur_a() {
    // a porte un label mais reste sans contrainte d'egalite : le leurre
    // (label C) doit disparaitre, les 150 sources P rester.
    let t = terrain();
    let obtenu = paires_triees(
        &t.db,
        "MATCH (a:P)-[r]->(b {qualified_name: 'cible'}) RETURN a.nom, type(r)",
    );
    let oracle: Vec<(String, String)> = t
        .entrantes("hub", None)
        .into_iter()
        .filter(|(a, _)| a != "leurre")
        .collect();
    assert_eq!(obtenu, oracle);
    assert_eq!(obtenu.len(), 150, "temoin : 151 moins l'arete du leurre");
}

#[test]
fn deux_extremites_contraintes() {
    // Piege n3 : a ET b portent une contrainte d'egalite. Quel que soit
    // l'ancrage choisi, l'ensemble est le meme.
    let t = terrain();
    let obtenu = paires_triees(
        &t.db,
        "MATCH (a {nom: 'leurre'})-[r]->(b {qualified_name: 'cible'}) \
         RETURN a.nom, type(r)",
    );
    assert_eq!(obtenu, vec![("leurre".into(), "T3".into())]);
}

#[test]
fn sortante_reste_intacte() {
    // Le temoin du defaut : la sortante ancree sur a etait deja correcte
    // et rapide — elle doit le rester, a l'identique.
    let t = terrain();
    let obtenu = paires_triees(
        &t.db,
        "MATCH (a {qualified_name: 'cible'})-[r]->(b) RETURN type(r), b.nom",
    );
    assert_eq!(obtenu, t.sortantes("hub"), "5 sortantes T1 du hub");
    assert_eq!(obtenu.len(), 5, "temoin de la fixture");
}

#[test]
fn entrante_b_sans_contrainte_reste_intacte() {
    // b libre : aucune reecriture possible, le plan actuel fait foi.
    let t = terrain();
    let obtenu = lignes(&t.db, "MATCH (a)-[r]->(b) RETURN count(r)");
    assert_eq!(
        obtenu,
        vec![vec![Value::Int64(t.aretes.len() as i64)]],
        "toutes les aretes du terrain, ni plus ni moins"
    );
}

// ══════════════ 4. LES FORMES EXCLUES DE LA REECRITURE ══════════════

#[test]
fn deux_sauts_b_contraint_au_bout() {
    // Le filtre sur b coiffe un Expand dont l'entree est un AUTRE Expand,
    // pas un NodeScan : la reecriture v1 ne s'applique pas, l'egalite si.
    let t = terrain();
    let obtenu = paires_triees(
        &t.db,
        "MATCH (a)-[r1]->(m)-[r2]->(b {qualified_name: 'cible'}) \
         RETURN a.nom, m.nom",
    );
    // Oracle : a->m puis m->hub, recompose depuis la liste d'aretes.
    let vers_hub: Vec<&String> = t
        .aretes
        .iter()
        .filter(|(_, _, b, _)| b == "hub")
        .map(|(a, _, _, _)| a)
        .collect();
    let mut oracle: Vec<(String, String)> = t
        .aretes
        .iter()
        .filter(|(_, _, m, _)| vers_hub.contains(&m))
        .map(|(a, _, m, _)| (a.clone(), m.clone()))
        .collect();
    oracle.sort();
    assert_eq!(obtenu, oracle, "chemins a 2 sauts finissant au hub");
    assert_eq!(
        obtenu.len(),
        30,
        "temoin : p100..p109 -> p000..p009 -> hub, ET p150..p169 -> leurre -> hub"
    );
}

// ══════════════ 5. LA PREUVE COMPTABLE : seek, pas balayage ══════════════
//
// La machine ment sous charge (BANCS.md : -39/+64 pct sur code identique) ;
// le NOMBRE DE SLOTS-NOEUDS LUS, lui, est deterministe. La preuve est
// DIFFERENTIELLE, sur le meme store et la meme forme de requete :
//   - propriete INDEXEE  → la reecriture tire  → seek,     ~0 slot ;
//   - propriete non indexee → pas de reecriture → balayage, >= N slots.
// Le second membre est le temoin interne de ce que valait l'ancien plan.

#[test]
fn preuve_comptable_seek_contre_balayage() {
    let t = terrain();
    let n_noeuds = 202; // 200 :P + hub + leurre

    // Chauffe hors comptage : plans, et l'OnceLock incoming_heads du
    // substrat (un balayage O(E) paye UNE fois par ouverture).
    let _ = paires(
        &t.db,
        "MATCH (a)-[r]->(b {qualified_name: 'cible'}) RETURN a.nom, type(r) LIMIT 1",
    );
    let _ = paires(
        &t.db,
        "MATCH (a)-[r]->(b {nom: 'hub'}) RETURN a.nom, type(r) LIMIT 1",
    );

    // TEMOIN : contrainte sur une propriete NON indexee — la regle ne
    // s'applique pas, l'ancien plan balaie tous les noeuds.
    let (temoin, slots_balayage) = mesurer_slots(|| {
        paires_triees(
            &t.db,
            "MATCH (a)-[r]->(b {nom: 'hub'}) RETURN a.nom, type(r)",
        )
    });
    assert_eq!(temoin, t.entrantes("hub", None), "le temoin rend le meme ensemble");
    assert!(
        slots_balayage >= n_noeuds,
        "temoin : l'ancien plan lit au moins tous les noeuds ({slots_balayage} slots \
         pour {n_noeuds} noeuds)"
    );

    // PREUVE : la meme forme, propriete indexee — seek, pas de balayage.
    //
    // ── ON MESURE LE RÉGIME ÉTABLI, PAS L'ÉCHAUFFEMENT (2026-09-17) ────────
    // Le commentaire de chauffe plus haut vise juste (« un balayage O(E) payé
    // UNE fois par ouverture ») mais la chauffe ne pouvait pas être garantie :
    // la structure paresseuse (`OnceLock incoming_heads`) est construite au
    // premier ACCÈS RÉEL au plan, et sous la charge d'une suite complète
    // l'exécution de chauffe pouvait ne pas être celle qui la construit. Mesure
    // comptée = celle du balayage d'initialisation → `slots_seek` ≈ 202, échec.
    //
    // On exécute donc la requête UNE FOIS HORS COMPTAGE, puis on mesure : c'est
    // le régime établi qui doit être seek. Le test garde ses dents — si la route
    // n'est pas un seek, les DEUX exécutions le montrent.
    let _ = paires_triees(
        &t.db,
        "MATCH (a)-[r]->(b {qualified_name: 'cible'}) RETURN a.nom, type(r)",
    );
    let (obtenu, slots_seek) = mesurer_slots(|| {
        paires_triees(
            &t.db,
            "MATCH (a)-[r]->(b {qualified_name: 'cible'}) RETURN a.nom, type(r)",
        )
    });
    assert_eq!(obtenu, t.entrantes("hub", None), "l'egalite d'abord, toujours");
    assert!(
        slots_seek < n_noeuds / 4,
        "ancree sur b par l'index, la traversee entrante ne balaie plus les \
         noeuds : {slots_seek} slots lus (temoin balayage : {slots_balayage})"
    );
}

// ══════════════ 6. LE BANC A L'ECHELLE DU DEFAUT (ignore) ══════════════
//
// La mesure du superviseur : ~375 000 noeuds balayes pour repondre a une
// entrante LIMIT 30 (319 572 ms sous charge). Ce banc reconstruit cette
// echelle : 375 000 noeuds libres, un hub a 10 000 aretes entrantes,
// 100 000 aretes de bruit. L'AVANT est mesure par procuration : la meme
// forme de requete sur une propriete NON indexee emprunte exactement
// l'ancien plan (balayage + expansion + filtre). Slots comptes, temps
// donne a titre INDICATIF seulement (machine partagee, charge 20+).
//
//   cargo test -p obrain-engine --test voisinage_entrant -- --ignored --nocapture

#[test]
#[ignore = "banc a 375k noeuds : lancer explicitement, --ignored --nocapture"]
fn banc_echelle_du_defaut() {
    let n: usize = 375_000;
    let entrantes_hub: usize = 10_000;
    let db = ObrainDB::new_in_memory();
    db.create_property_index("qualified_name");
    let s = db.session();

    let mut ids = Vec::with_capacity(n);
    for i in 0..n {
        ids.push(s.create_node_with_props(&["N"], [("k", Value::Int64(i as i64))]));
    }
    let hub = s.create_node_with_props(
        &["C"],
        [
            ("nom", Value::String("hub".into())),
            ("qualified_name", Value::String("plan:racine".into())),
        ],
    );
    for &src in ids.iter().take(entrantes_hub) {
        s.create_edge(src, hub, "REL");
    }
    // Bruit : des aretes qui ne touchent pas le hub, pour que l'expansion
    // de l'ancien plan ait quelque chose a developper.
    for i in entrantes_hub..(entrantes_hub + 100_000) {
        s.create_edge(ids[i], ids[(i * 7) % n], "REL");
    }

    let compte = |q: &str| -> i64 {
        match &lignes(&db, q)[0][0] {
            Value::Int64(c) => *c,
            autre => panic!("compte non entier : {autre:?}"),
        }
    };

    // Chauffe : plans + OnceLock incoming_heads (balayage O(E), une fois).
    let _ = compte("MATCH (a)-[r]->(b {qualified_name: 'plan:racine'}) RETURN count(r)");
    let _ = compte("MATCH (a)-[r]->(b {nom: 'hub'}) RETURN count(r)");

    // AVANT, par procuration (nom non indexe -> ancien plan).
    let debut = web_time::Instant::now();
    let (compte_avant, slots_avant) =
        mesurer_slots(|| compte("MATCH (a)-[r]->(b {nom: 'hub'}) RETURN count(r)"));
    let t_avant = debut.elapsed();

    // APRES (qualified_name indexe -> re-ancrage sur b).
    let debut = web_time::Instant::now();
    let (compte_apres, slots_apres) =
        mesurer_slots(|| compte("MATCH (a)-[r]->(b {qualified_name: 'plan:racine'}) RETURN count(r)"));
    let t_apres = debut.elapsed();

    // LIMIT 30 avec projection du far-endpoint (piege n4 : lecture de a).
    let debut = web_time::Instant::now();
    let lim = lignes(
        &db,
        "MATCH (a)-[r]->(b {qualified_name: 'plan:racine'}) RETURN a.k LIMIT 30",
    );
    let t_limit = debut.elapsed();

    eprintln!("── banc echelle du defaut ({n} noeuds, hub a {entrantes_hub} entrantes) ──");
    eprintln!("AVANT  (proxy non indexe) : {slots_avant:>7} slots lus, {t_avant:?} (indicatif)");
    eprintln!("APRES  (re-ancrage indexe): {slots_apres:>7} slots lus, {t_apres:?} (indicatif)");
    eprintln!("LIMIT 30 + a.k            : {t_limit:?} (indicatif)");

    // L'egalite d'abord.
    assert_eq!(compte_avant, entrantes_hub as i64);
    assert_eq!(compte_apres, compte_avant, "meme compte par les deux plans");
    assert_eq!(lim.len(), 30);

    // La preuve structurelle : balayage -> seek.
    assert!(
        slots_avant >= n as u64,
        "temoin : l'ancien plan lit tous les noeuds ({slots_avant})"
    );
    assert!(
        slots_apres < 1_000,
        "le plan re-ancre ne balaie plus : {slots_apres} slots (avant : {slots_avant})"
    );
}

#[test]
fn saut_variable_b_contraint() {
    // min/max != 1 : exclu de la reecriture, l'egalite tient toujours.
    let t = terrain();
    let obtenu = lignes(
        &t.db,
        "MATCH (a)-[r*1..2]->(b {qualified_name: 'cible'}) RETURN count(*)",
    );
    // Oracle recompose depuis la liste d'aretes : longueur 1 + longueur 2.
    let len1 = t.aretes.iter().filter(|(_, _, b, _)| b == "hub").count();
    let len2 = t
        .aretes
        .iter()
        .filter(|(_, _, m, _)| t.aretes.iter().any(|(a2, _, b2, _)| a2 == m && b2 == "hub"))
        .count();
    assert_eq!(len1 + len2, 181, "temoin fixe de la fixture : 151 + 30");
    assert_eq!(obtenu, vec![vec![Value::Int64((len1 + len2) as i64)]]);
}
