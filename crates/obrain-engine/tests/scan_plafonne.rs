//! SCAN-PLAFONNE — l'egalite du resultat, bout en bout, AVANT le gain.
//!
//! Chantier : « qu'un `LIMIT 30` lise 30 slots, pas 3,5 millions ».
//!
//! ── LE PIEGE QUE CE FICHIER EXISTE POUR ATTRAPER ──
//!
//! On ne peut pas plafonner le scan a k quand un filtre se trouve en aval.
//! `MATCH (n) WHERE n.x = 'y' RETURN n LIMIT 30` doit lire JUSQU'A ce que 30
//! lignes PASSENT le filtre — pas lire 30 slots. Un plafond naif rend alors
//! moins de lignes que demande, SILENCIEUSEMENT, et seulement sur les jeux de
//! donnees ou le filtre rejette beaucoup. C'est la faute la plus facile a
//! commettre ici et la plus difficile a voir.
//!
//! `filtre_en_aval_le_plafond_doit_se_desarmer` est donc construit pour etre
//! ROUGE sous un plafond naif : un noeud sur vingt passe le filtre, si bien
//! que les 30 premiers slots n'en contiennent que deux. Plafonner a 30
//! rendrait 2 lignes au lieu de 30.
//!
//! Les autres formes reductrices — tri, DISTINCT, agregat, jointure — ont
//! chacune leur test, sur le meme principe : le resultat doit etre celui du
//! plan generique, au caractere pres.
//!
//!   cargo test -p obrain-engine --test scan_plafonne

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use obrain_substrate::compteurs::mesurer_slots;

// ───────────────────────────── outillage ─────────────────────────────

/// `n` noeuds `:S {nom, groupe}`. `groupe = 'rare'` un sur vingt — soit
/// `n / 20` lignes retenues, dispersees sur toute l'etendue des slots.
fn db(n: usize) -> ObrainDB {
    let d = ObrainDB::new_in_memory();
    let s = d.session();
    for i in 0..n {
        let groupe = if i % 20 == 0 { "rare" } else { "commun" };
        s.create_node_with_props(
            &["S"],
            [
                ("nom", Value::String(format!("n{i:05}").into())),
                ("groupe", Value::String(groupe.into())),
            ],
        );
    }
    d
}

fn lignes(d: &ObrainDB, q: &str) -> Vec<Vec<Value>> {
    d.session()
        .execute(q)
        .unwrap_or_else(|e| panic!("requete refusee : {q}\n{e}"))
        .rows
}

fn textes(d: &ObrainDB, q: &str) -> Vec<String> {
    lignes(d, q)
        .into_iter()
        .map(|l| match &l[0] {
            Value::String(s) => s.to_string(),
            autre => panic!("valeur non-chaine : {autre:?}"),
        })
        .collect()
}

// ══════════════════ 1. LE PIEGE : une reduction en aval ══════════════════

#[test]
fn filtre_en_aval_le_plafond_doit_se_desarmer() {
    let d = db(1_000);

    let obtenu = textes(
        &d,
        "MATCH (n) WHERE n.groupe = 'rare' RETURN n.nom LIMIT 30",
    );

    // ORACLE independant du plan : le meme filtre, sans LIMIT.
    let oracle: Vec<String> = textes(&d, "MATCH (n) WHERE n.groupe = 'rare' RETURN n.nom")
        .into_iter()
        .take(30)
        .collect();

    assert_eq!(oracle.len(), 30, "temoin : 50 lignes passent le filtre");
    assert_eq!(
        obtenu.len(),
        30,
        "un LIMIT 30 derriere un filtre doit rendre 30 lignes. Un plafond \
         naif a 30 SLOTS n'en aurait rendu que 2 — un noeud sur vingt passe \
         le filtre."
    );
    assert_eq!(obtenu, oracle, "memes lignes, meme ordre");
}

#[test]
fn filtre_tres_selectif_limite_sous_le_compte_disponible() {
    // Le filtre ne retient que la toute FIN des slots : un plafond naif
    // rendrait zero ligne.
    let d = ObrainDB::new_in_memory();
    let s = d.session();
    for i in 0..2_000 {
        let groupe = if i >= 1_950 { "queue" } else { "corps" };
        s.create_node_with_props(
            &["S"],
            [
                ("nom", Value::String(format!("n{i:05}").into())),
                ("groupe", Value::String(groupe.into())),
            ],
        );
    }
    let obtenu = textes(
        &d,
        "MATCH (n) WHERE n.groupe = 'queue' RETURN n.nom LIMIT 10",
    );
    assert_eq!(obtenu.len(), 10, "10 lignes, toutes en fin de balayage");
    assert_eq!(obtenu[0], "n01950");
}

#[test]
fn distinct_ne_doit_pas_etre_plafonne_en_amont() {
    // DISTINCT peut effondrer des lignes : N entrees peuvent rendre moins de
    // N sorties, donc le plafond ne peut pas descendre.
    let d = db(1_000);
    let obtenu = textes(&d, "MATCH (n) RETURN DISTINCT n.groupe LIMIT 2");
    let mut attendu = obtenu.clone();
    attendu.sort();
    assert_eq!(attendu, vec!["commun".to_string(), "rare".to_string()]);
}

#[test]
fn tri_borne_le_plafond_ne_doit_pas_couper_son_entree() {
    // Le tri borne a besoin de TOUTES les lignes pour etre correct. Couper
    // son entree a k rendrait « les k premiers slots, tries » au lieu des
    // « k plus petits ». Ici le plus petit nom est le DERNIER slot.
    let d = ObrainDB::new_in_memory();
    let s = d.session();
    for i in 0..500 {
        // nom decroissant : le minimum global est cree en dernier.
        s.create_node_with_props(
            &["S"],
            [("nom", Value::String(format!("n{:05}", 500 - i).into()))],
        );
    }
    let obtenu = textes(&d, "MATCH (n) RETURN n.nom ORDER BY n.nom LIMIT 10");

    let mut oracle = textes(&d, "MATCH (n) RETURN n.nom");
    oracle.sort();
    oracle.truncate(10);

    assert_eq!(obtenu.len(), 10);
    assert_eq!(
        obtenu, oracle,
        "le tri borne doit voir toutes les lignes : son minimum est au \
         dernier slot"
    );
    assert_eq!(obtenu[0], "n00001");
}

#[test]
fn agregat_sous_limite_voit_toutes_les_lignes() {
    let d = db(1_000);
    let l = lignes(&d, "MATCH (n) RETURN count(n) AS c LIMIT 1");
    assert_eq!(l.len(), 1);
    assert_eq!(
        l[0][0],
        Value::Int64(1_000),
        "l'agregat compte les 1 000 noeuds, pas 1"
    );
}

// ══════════════ 2. NON-REGRESSION : sans LIMIT, tout est vu ══════════════

#[test]
fn scan_sans_limite_voit_tous_les_noeuds() {
    let d = db(1_000);
    assert_eq!(textes(&d, "MATCH (n) RETURN n.nom").len(), 1_000);
    assert_eq!(lignes(&d, "MATCH (n) RETURN n").len(), 1_000);
    assert_eq!(
        textes(&d, "MATCH (n) WHERE n.groupe = 'rare' RETURN n.nom").len(),
        50
    );
}

#[test]
fn limite_plus_grande_que_la_base_rend_tout() {
    let d = db(137);
    assert_eq!(textes(&d, "MATCH (n) RETURN n.nom LIMIT 10000").len(), 137);
}

#[test]
fn limite_zero_rend_zero() {
    let d = db(100);
    assert_eq!(textes(&d, "MATCH (n) RETURN n.nom LIMIT 0").len(), 0);
}

// ══════════ 3. LE CAS LEGITIME : sans etiquette, sans filtre, sans tri ══════════

#[test]
fn limite_sans_etiquette_rend_le_prefixe_exact_du_scan_complet() {
    let d = db(1_000);
    let complet = textes(&d, "MATCH (n) RETURN n.nom");
    for k in [1usize, 5, 30, 999, 1_000] {
        let obtenu = textes(&d, &format!("MATCH (n) RETURN n.nom LIMIT {k}"));
        assert_eq!(obtenu.len(), k.min(1_000), "compte pour LIMIT {k}");
        assert_eq!(obtenu, complet[..k.min(1_000)], "prefixe pour LIMIT {k}");
    }
}

#[test]
fn limite_sans_etiquette_rendant_le_noeud_entier() {
    // `RETURN n` (Variable) et non `RETURN n.nom` (Property) : les deux
    // formes empruntent la descente sous le Return.
    let d = db(500);
    let complet = lignes(&d, "MATCH (n) RETURN n");
    let obtenu = lignes(&d, "MATCH (n) RETURN n LIMIT 30");
    assert_eq!(obtenu.len(), 30);
    assert_eq!(obtenu, complet[..30]);
}

#[test]
fn tombstones_la_limite_rend_k_noeuds_vivants() {
    // Le piege des trous, vu du moteur : 300 morts en tete, un LIMIT 30 doit
    // rendre 30 VIVANTS, pas s'arreter dans le cimetiere.
    let d = db(400);
    d.session()
        .execute("MATCH (n) WHERE n.nom < 'n00300' DETACH DELETE n")
        .expect("suppression");

    let restants = textes(&d, "MATCH (n) RETURN n.nom");
    assert_eq!(restants.len(), 100, "100 survivants");

    let obtenu = textes(&d, "MATCH (n) RETURN n.nom LIMIT 30");
    assert_eq!(obtenu.len(), 30, "30 vivants malgre 300 tombstones en tete");
    assert_eq!(obtenu, restants[..30]);
}

// ══════════════════ 4. MVCC : le plafond doit se desarmer ══════════════════

#[test]
fn transaction_ouverte_la_limite_voit_les_ecritures_de_sa_session() {
    // Avec une transaction ouverte, les cartes MVCC sont peuplees :
    // `scan_visibility_is_trivial` rend faux et le plafond se desarme. Le
    // balayage repasse par `all_node_ids()` + filtre de visibilite, qui est
    // le seul chemin voyant les versions EN ATTENTE. Un plafond pose avant
    // ce filtre en rendrait moins que k.
    let d = db(200);
    let mut s = d.session();
    s.begin_transaction().expect("BEGIN");
    for i in 0..50 {
        s.create_node_with_props(&["S"], [("nom", Value::String(format!("z{i:05}").into()))]);
    }

    let tout = s.execute("MATCH (n) RETURN n.nom").expect("scan").rows;
    assert_eq!(
        tout.len(),
        250,
        "200 commis + 50 en attente, vus par leur session"
    );

    let borne = s
        .execute("MATCH (n) RETURN n.nom LIMIT 30")
        .expect("scan borne")
        .rows;
    assert_eq!(borne.len(), 30, "un LIMIT 30 rend 30 lignes en transaction");
    assert_eq!(borne, tout[..30], "et ce sont les 30 premieres");

    s.commit().expect("COMMIT");
    assert_eq!(textes(&d, "MATCH (n) RETURN n.nom").len(), 250);
}

// ══════════════════ 5. LA PREUVE COMPTABLE, bout en bout ══════════════════

#[test]
fn limite_sans_etiquette_ne_lit_plus_toute_la_base() {
    let d = db(20_000);

    let (borne, slots_borne) = mesurer_slots(|| textes(&d, "MATCH (n) RETURN n.nom LIMIT 30"));
    let (complet, slots_complet) = mesurer_slots(|| textes(&d, "MATCH (n) RETURN n.nom"));

    assert_eq!(borne.len(), 30);
    assert_eq!(complet.len(), 20_000);

    assert!(
        slots_complet >= 20_000,
        "temoin : le scan complet lit toute la base ({slots_complet})"
    );
    // C'est l'affirmation du chantier, et elle est comptable : aucun
    // chronometre, donc insensible a la charge de la machine.
    assert!(
        slots_borne < 200,
        "un LIMIT 30 sans etiquette doit lire ~30 slots, pas {slots_complet}. \
         Lu : {slots_borne}"
    );
}

#[test]
fn le_plafond_ne_s_applique_pas_derriere_un_filtre_et_le_compteur_le_montre() {
    // Contre-epreuve du precedent : avec un filtre, le balayage DOIT rester
    // integral. Le compteur sert ici a prouver qu'on n'a pas triche.
    let d = db(20_000);
    let (r, slots) = mesurer_slots(|| {
        textes(
            &d,
            "MATCH (n) WHERE n.groupe = 'rare' RETURN n.nom LIMIT 30",
        )
    });
    assert_eq!(r.len(), 30);
    assert!(
        slots >= 20_000,
        "le scan derriere un filtre reste integral : c'est le prix de la \
         correction. Lu : {slots}"
    );
}
