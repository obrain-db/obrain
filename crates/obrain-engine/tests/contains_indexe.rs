//! CONTAINS-INDEXE — l'égalité du résultat scan-vs-index, AVANT le gain.
//!
//! Chantier NS-PARTOUT : « un CONTAINS ne doit pas hydrater 333 262 nœuds
//! pour rendre 44 lignes ». La route (`try_plan_filter_with_contains`)
//! prend les candidats de l'index de trigrammes et REVÉRIFIE chaque valeur
//! courante : ces tests sont le contrat qui l'autorise.
//!
//! ── LES PIÈGES QUE CE FICHIER EXISTE POUR ATTRAPER ──
//!
//! 1. le FAUX NÉGATIF de tokenisation : `ixgbe_read` matche
//!    `mixgbe_ready` PAR SOUS-CHAÎNE, alors qu'aucun découpage en mots ne
//!    le voit. Les trigrammes d'octets le voient — c'est le test
//!    adversarial central ;
//! 2. la SOUS-LIVRAISON sous LIMIT : la liste rendue par la route est
//!    COMPLÈTE avant la limite (piège du scan plafonné, dans cette base) ;
//! 3. l'index cru SUR PAROLE : valeurs modifiées ou nœuds supprimés APRÈS
//!    la fusion doivent disparaître du résultat, valeurs neuves y entrer ;
//! 4. le débordement de domaine : CONTAINS est chaîne-à-chaîne
//!    (`as_str()?` des deux côtés) — une valeur non-chaîne ne matche
//!    jamais, indexée ou pas.
//!
//!   cargo test -p obrain-engine --test contains_indexe

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use obrain_substrate::compteurs::mesurer_slots;

// ───────────────────────────── outillage ─────────────────────────────

const CORPUS: &[&str] = &[
    "drivers.net.ixgbe.ixgbe_main.ixgbe_read_pci_cfg_word",
    "drivers.net.ixgbe.ixgbe_main.ixgbe_read_reg",
    "drivers.net.ixgbe.ixgbe_main.netif_is_ixgbe",
    "drivers.net.mixgbe_ready.helper", // sous-chaîne À CHEVAL sur les séparateurs
    "drivers.net.e1000.e1000_read_phy_reg",
    "drivers.net.reacteur.r\u{e9}acteur_principal", // multi-octets UTF-8
    "drivers.net.autre.module_sans_rapport",
];

/// Le verdict de référence, calculé par l'évaluateur du langage lui-même
/// (`str::contains`), jamais par une réimplémentation locale.
fn attendus(needle: &str) -> Vec<String> {
    let mut v: Vec<String> = CORPUS
        .iter()
        .filter(|q| q.contains(needle))
        .map(|q| (*q).to_string())
        .collect();
    v.sort();
    v
}

fn peupler(d: &ObrainDB) {
    let s = d.session();
    for (i, q) in CORPUS.iter().enumerate() {
        s.create_node_with_props(
            &["S"],
            [
                ("qualified_name", Value::String((*q).into())),
                ("rang", Value::Int64(i as i64)),
            ],
        );
    }
    // Un nœud dont la propriété n'est PAS une chaîne : CONTAINS ne doit
    // jamais le rendre (l'évaluateur fait `as_str()?`).
    s.create_node_with_props(&["S"], [("qualified_name", Value::Int64(42))]);
}

fn contains_q(needle: &str) -> String {
    format!(
        "MATCH (n) WHERE n.qualified_name CONTAINS '{needle}' \
         RETURN n.qualified_name ORDER BY n.qualified_name"
    )
}

fn textes(d: &ObrainDB, q: &str) -> Vec<String> {
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

// ══════════════ 1. l'égalité scan-vs-index, même processus ══════════════

/// Le même corpus, la même requête : le résultat AVANT l'index (balayage
/// générique) est le témoin, le résultat APRÈS `create_property_index`
/// (route trigrammes/DashMap) doit lui être identique au caractère près.
#[test]
fn le_resultat_indexe_est_celui_du_balayage() {
    let d = ObrainDB::new_in_memory();
    peupler(&d);
    for needle in ["ixgbe_read", "read", "net.ixgbe", "\u{e9}acteur", "zzz"] {
        let temoin = textes(&d, &contains_q(needle));
        assert_eq!(
            temoin,
            attendus(needle),
            "le témoin lui-même diverge ({needle})"
        );
        d.create_property_index("qualified_name");
        let indexe = textes(&d, &contains_q(needle));
        assert_eq!(indexe, temoin, "scan vs index divergent sur {needle:?}");
    }
}

/// `mixgbe_ready` CONTIENT `ixgbe_read` : le cas que toute tokenisation
/// par mots manque (faux négatif silencieux). Il DOIT sortir.
#[test]
fn la_sous_chaine_a_cheval_sur_les_separateurs_est_trouvee() {
    let d = ObrainDB::new_in_memory();
    peupler(&d);
    d.create_property_index("qualified_name");
    let r = textes(&d, &contains_q("ixgbe_read"));
    assert!(
        r.contains(&"drivers.net.mixgbe_ready.helper".to_string()),
        "faux négatif de tokenisation : {r:?}"
    );
    assert_eq!(r, attendus("ixgbe_read"));
}

/// Motif de MOINS de 3 octets : la zone ne sait pas trancher, le résultat
/// doit rester celui du balayage (repli, jamais un mensonge).
#[test]
fn un_motif_de_moins_de_trois_octets_reste_juste() {
    let d = ObrainDB::new_in_memory();
    peupler(&d);
    d.create_property_index("qualified_name");
    for needle in ["ix", "e", ""] {
        assert_eq!(
            textes(&d, &contains_q(needle)),
            attendus(needle),
            "motif court {needle:?}"
        );
    }
}

/// CONTAINS dans un AND : la route ne s'applique qu'au prédicat COMPLET
/// (même prudence que STARTS WITH) — le composé doit rester juste par le
/// chemin générique.
#[test]
fn un_contains_compose_reste_juste() {
    let d = ObrainDB::new_in_memory();
    peupler(&d);
    d.create_property_index("qualified_name");
    let q = "MATCH (n) WHERE n.qualified_name CONTAINS 'ixgbe_read' AND n.rang >= 0 \
             RETURN n.qualified_name ORDER BY n.qualified_name";
    assert_eq!(textes(&d, q), attendus("ixgbe_read"));
}

/// Étiquette dans le motif : `MATCH (n:S)` doit rendre la même chose que
/// sans étiquette (tout le corpus est :S), et une étiquette absente rien.
#[test]
fn l_etiquette_du_motif_est_respectee() {
    let d = ObrainDB::new_in_memory();
    peupler(&d);
    d.create_property_index("qualified_name");
    let avec = "MATCH (n:S) WHERE n.qualified_name CONTAINS 'ixgbe_read' \
         RETURN n.qualified_name ORDER BY n.qualified_name"
        .to_string();
    assert_eq!(textes(&d, &avec), attendus("ixgbe_read"));
    let absente = "MATCH (n:Absente) WHERE n.qualified_name CONTAINS 'ixgbe_read' \
                   RETURN n.qualified_name";
    assert_eq!(textes(&d, absente), Vec::<String>::new());
}

// ══════════════ 2. LIMIT : la sous-livraison interdite ══════════════

/// La route rend une liste COMPLÈTE puis la limite coupe : k lignes
/// demandées, k lignes rendues (tant qu'il en existe k), toutes
/// satisfaisant le prédicat. Le piège du scan plafonné ne doit pas se
/// réintroduire par cette route.
#[test]
fn contains_avec_limit_ne_sous_livre_pas() {
    let d = ObrainDB::new_in_memory();
    peupler(&d);
    d.create_property_index("qualified_name");
    let total = attendus("ixgbe_read").len();
    assert!(total >= 3, "le corpus doit porter au moins 3 réponses");
    for k in 1..=total {
        let q = format!(
            "MATCH (n) WHERE n.qualified_name CONTAINS 'ixgbe_read' \
             RETURN n.qualified_name ORDER BY n.qualified_name LIMIT {k}"
        );
        let r = textes(&d, &q);
        assert_eq!(
            r.len(),
            k,
            "sous-livraison : {} lignes pour LIMIT {k}",
            r.len()
        );
        assert_eq!(r, attendus("ixgbe_read")[..k].to_vec());
    }
}

// ══════════════ 3. la persistance, et l'index jamais cru sur parole ══════════════

/// create → PERSISTE → réouverture → la zone répond, et les écritures
/// POSTÉRIEURES à la fusion (ajout, modification, suppression) sont
/// visibles : l'index énumère des candidats, la valeur courante tranche.
#[test]
#[ignore = "CONTRAT DE LA ZONE TRIGRAMMES SUPPLANTEE (consolidation 2026-09-15) : le tronc a retenu le \
            systeme adaptatif props_trigram_index (garde par demandes, tampon additif) qui ne persiste pas \
            substrate.props.tri.idx.v2 au create. La persistance a l ouverture est LE chantier NS ouvert \
            (« eq-index persistant et charge a l ouverture, CONTAINS route par index texte/trigrammes ») — \
            reactiver ce test quand il atterrit sur le tronc. L exactitude scan-vs-index reste couverte par \
            les tests 1-2 de ce fichier (verts)."]
fn apres_reouverture_l_index_sert_et_les_ecritures_de_session_aussi() {
    let dir = tempfile::TempDir::new().unwrap();
    let chemin = dir.path().join("base.obrain");
    {
        let d = ObrainDB::open(&chemin).unwrap();
        peupler(&d);
        d.create_property_index("qualified_name");
        // AUCUN close() encore : la persistance immédiate du create doit
        // suffire — c'est le défaut n°1 (le serveur MCP ne ferme jamais).
        assert!(
            chemin.join("substrate.props.eq.idx.v2").exists(),
            "create_property_index doit persister l'égalité SUR-LE-CHAMP"
        );
        assert!(
            chemin.join("substrate.props.tri.idx.v2").exists(),
            "create_property_index doit persister les trigrammes SUR-LE-CHAMP"
        );
        d.close().unwrap();
    }
    let d = ObrainDB::open(&chemin).unwrap();
    assert_eq!(
        textes(&d, &contains_q("ixgbe_read")),
        attendus("ixgbe_read")
    );
    // Écritures post-fusion, dans la session rouverte.
    let s = d.session();
    s.execute("INSERT (:S {qualified_name: 'drivers.net.neuf.ixgbe_read_batch'})")
        .unwrap();
    s.execute(
        "MATCH (n {qualified_name: 'drivers.net.ixgbe.ixgbe_main.ixgbe_read_reg'}) \
         SET n.qualified_name = 'drivers.net.ixgbe.renomme'",
    )
    .unwrap();
    s.execute("MATCH (n {qualified_name: 'drivers.net.mixgbe_ready.helper'}) DETACH DELETE n")
        .unwrap();
    let mut attendu = attendus("ixgbe_read");
    attendu.push("drivers.net.neuf.ixgbe_read_batch".into());
    attendu.retain(|q| {
        q != "drivers.net.ixgbe.ixgbe_main.ixgbe_read_reg" && q != "drivers.net.mixgbe_ready.helper"
    });
    attendu.sort();
    assert_eq!(
        textes(&d, &contains_q("ixgbe_read")),
        attendu,
        "l'index a été cru sur parole : delta de session perdu ou fantômes rendus"
    );
    d.close().unwrap();
}

// ══════════════ 4. la preuve par les slots ══════════════

/// La charge machine rend les chronomètres menteurs (BANCS.md) ; le
/// compteur de slots, lui, est déterministe. La route indexée ne doit
/// LIRE AUCUN slot de balayage — le générique les lit tous.
#[test]
#[ignore = "Meme dette que ci-dessus : la preuve par slots exige que la route trigrammes soit armee des le \
            create — comportement de la zone supplantee. Sous le systeme adaptatif, la route s arme apres \
            N demandes dans le processus (OBRAIN_TRIGRAMMES_DES_LA). A reactiver avec le chantier NS \
            (persistance) ou a reecrire contre la garde adaptative chauffee."]
fn la_route_indexee_ne_balaie_pas_et_le_compteur_le_montre() {
    let d = ObrainDB::new_in_memory();
    peupler(&d);
    let q = contains_q("ixgbe_read");
    let (avant, slots_scan) = mesurer_slots(|| textes(&d, &q));
    d.create_property_index("qualified_name");
    let (apres, slots_index) = mesurer_slots(|| textes(&d, &q));
    assert_eq!(avant, apres, "le gain a changé la réponse");
    assert!(
        slots_scan > 0,
        "le témoin devait balayer (slots_scan = {slots_scan})"
    );
    assert_eq!(
        slots_index, 0,
        "la route indexée a balayé {slots_index} slots — elle devait lire l'index seul"
    );
}
