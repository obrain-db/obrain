//! Rappel partiel, provenance et AMBIGUÏTÉ d'imbrication (moteur réel + substrat).
//!
//! # Le défaut mesuré
//!
//! Sur un indice PARTIEL, un fragment `{a,b}` devance l'ensemble complet
//! `{a,b,c}` dont il est un sous-ensemble. La cause est structurelle : le chemin
//! direct score `recouvrement / |ensemble|`, donc `1/2 = 0,5` pour le fragment
//! contre `1/3 ≈ 0,33` pour le complet. Aucun signal disponible ne permet de
//! décider lequel est LA réponse.
//!
//! # Ce que ce lot fait, et ce qu'il refuse de faire
//!
//! Il EXPOSE l'ambiguïté ([`diagnostiquer_imbrications`]) sans toucher au
//! classement ni aux API existantes : deux candidats dont l'un contient l'autre
//! et qui expliquent EXACTEMENT les mêmes indices sont signalés comme
//! compatibles avec la même observation.
//!
//! Il ne fusionne rien. Décider que le petit est une version ANTÉRIEURE du grand
//! demande une preuve de filiation ORIENTÉE que le chemin de formation observé
//! ne produit pas : `Engram::source_episodes` — la structure native de
//! provenance — n'est jamais renseigné par `EngramManager::record_activation`,
//! et le pont d'observation refuse explicitement d'attacher une identité sur une
//! formation simple (`observation_id: None`). Ce lot mesure ce trou
//! (`temoin_provenance_absente_dans_le_chemin_observe`) au lieu de l'inférer.
//!
//! Le critère top-1 STRICT de `engram_e2e_test` reste donc rouge et ignoré,
//! avec son assertion intacte : il redeviendra jugeable le jour où une preuve
//! native de filiation existera.
#![cfg(feature = "cognitive-engram")]

use std::time::Duration;

use obrain_cognitive::diagnostiquer_imbrications;
use obrain_common::types::NodeId;
use obrain_engine::{ObrainDB, Session};

const ATTENTE: Duration = Duration::from_secs(5);

fn id_de(session: &Session, nom: &str) -> NodeId {
    let res = session
        .execute(&format!("MATCH (n:Item {{name:'{nom}'}}) RETURN id(n)"))
        .expect("match id");
    NodeId(res.rows[0][0].to_string().parse::<u64>().expect("id"))
}

fn activer(session: &Session, noms: &[&str], valeur: i64) {
    let (a, b, c) = (noms[0], noms[1], noms[2]);
    for (x, y) in [(a, b), (b, c), (c, a)] {
        session
            .execute(&format!(
                "MATCH (n:Item {{name:'{x}'}}), (m:Item {{name:'{y}'}}) \
                 CREATE (n)-[:COACT {{r:{valeur}}}]->(m)"
            ))
            .expect("edge");
    }
}

fn ensemble_trie(ids: &[NodeId]) -> Vec<u64> {
    let mut v: Vec<u64> = ids.iter().map(|n| n.0).collect();
    v.sort_unstable();
    v
}

fn quiesce(db: &ObrainDB) {
    assert!(
        db.cognitive_quiesce(ATTENTE),
        "quiesce : publiés={}, dispatchés={}, perdus={}",
        db.cognitive_events_published(),
        db.cognitive_events_dispatched(),
        db.cognitive_events_lost()
    );
    assert_eq!(db.cognitive_events_lost(), 0, "aucun événement perdu");
}

/// Prépare la base de référence : deux trios disjoints co-activés trois fois
/// chacun, par arêtes (donc {a,b} et {a,b,c} coexistent — état mesuré).
fn base_de_reference(chemin: &std::path::Path) -> ObrainDB {
    let db = ObrainDB::open_substrate_observed(chemin).expect("open");
    let session = db.session();
    session
        .execute(
            "CREATE (:Item {name:'a'}), (:Item {name:'b'}), (:Item {name:'c'}),
                    (:Item {name:'d'}), (:Item {name:'e'}), (:Item {name:'f'})",
        )
        .expect("create");
    quiesce(&db);
    for valeur in 1..=3 {
        for trio in [["a", "b", "c"], ["d", "e", "f"]] {
            activer(&session, &trio, valeur);
            quiesce(&db);
        }
    }
    db
}

/// TÉMOIN DU MANQUE : le chemin observé ne produit AUCUNE provenance.
///
/// C'est la mesure qui fonde tout le lot : sans elle, « on ne peut pas décider »
/// serait une opinion. Tous les engrammes formés automatiquement portent
/// `source_episodes` vide, et `{a,b}` comme `{a,b,c}` existent bien.
#[test]
fn temoin_provenance_absente_dans_le_chemin_observe() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = base_de_reference(&dir.path().join("base"));
    let session = db.session();
    let a = id_de(&session, "a");
    let b = id_de(&session, "b");
    let c = id_de(&session, "c");

    let engine = db.cognitive_engine().expect("moteur");
    let mgr = engine.engram_manager().expect("manager");
    let engrams = mgr.store().list();

    let abc = engrams
        .iter()
        .find(|e| ensemble_trie(&e.node_ids()) == ensemble_trie(&[a, b, c]));
    let ab = engrams
        .iter()
        .find(|e| ensemble_trie(&e.node_ids()) == ensemble_trie(&[a, b]));
    assert!(abc.is_some(), "le trio complet se forme");
    assert!(
        ab.is_some(),
        "le fragment {{a,b}} se forme aussi (état mesuré)"
    );

    // LE TROU, mesuré : aucune provenance, sur AUCUN engramme.
    let avec_provenance = engrams
        .iter()
        .filter(|e| !e.source_episodes.is_empty())
        .count();
    assert_eq!(
        avec_provenance,
        0,
        "mesure : le chemin observé ne renseigne pas `source_episodes` \
         ({} engrammes, {} avec provenance) — rien n'atteste une filiation",
        engrams.len(),
        avec_provenance
    );
}

/// L'indice PARTIEL signale l'imbrication, et le classement n'est PAS modifié.
#[test]
fn indice_partiel_signale_l_imbrication_sans_changer_le_classement() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = base_de_reference(&dir.path().join("base"));
    let session = db.session();
    let a = id_de(&session, "a");
    let b = id_de(&session, "b");
    let c = id_de(&session, "c");

    let engine = db.cognitive_engine().expect("moteur");
    let mgr = engine.engram_manager().expect("manager");

    let resultats = mgr.recall(&[a], 5);
    let rendus: Vec<Vec<u64>> = resultats
        .iter()
        .map(|r| ensemble_trie(&r.engram.node_ids()))
        .collect();
    // Le biais structurel est intact : c'est lui qui rend la réponse ambiguë.
    assert_eq!(
        rendus.first(),
        Some(&vec![a.0.min(b.0), a.0.max(b.0)]),
        "sur [a], le fragment garde l'avantage (1/|E|) — obtenu {rendus:?}"
    );

    let ambiguites = diagnostiquer_imbrications(&[a], &resultats);
    assert_eq!(
        ambiguites.len(),
        1,
        "une imbrication signalée : {ambiguites:?}"
    );
    let amb = &ambiguites[0];
    assert_eq!(
        ensemble_trie(&mgr.store().get(amb.fragment).expect("fragment").node_ids()),
        ensemble_trie(&[a, b]),
        "le fragment désigné est {{a,b}}"
    );
    assert_eq!(
        ensemble_trie(&mgr.store().get(amb.complet).expect("complet").node_ids()),
        ensemble_trie(&[a, b, c]),
        "le complet désigné est {{a,b,c}}"
    );
    assert_eq!(
        amb.indices_communs,
        vec![a],
        "l'indice expliqué par les deux est [a]"
    );
    assert_eq!(amb.rang_fragment, 0, "le fragment est premier");
    assert!(
        amb.rang_complet > amb.rang_fragment,
        "le complet est derrière — le diagnostic le rend visible, il ne le corrige pas"
    );
}

/// Indice BRUITÉ `[a,b,d]` : l'imbrication est signalée sur les indices
/// réellement partagés par les deux ensembles.
#[test]
fn indice_bruite_signale_les_indices_communs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = base_de_reference(&dir.path().join("base"));
    let session = db.session();
    let a = id_de(&session, "a");
    let b = id_de(&session, "b");
    let c = id_de(&session, "c");
    let d = id_de(&session, "d");

    let engine = db.cognitive_engine().expect("moteur");
    let mgr = engine.engram_manager().expect("manager");

    let cues = [a, b, d];
    let resultats = mgr.recall(&cues, 5);
    let mut ambiguites = diagnostiquer_imbrications(&cues, &resultats);
    assert!(
        !ambiguites.is_empty(),
        "au moins l'imbrication {{a,b}}/{{a,b,c}} doit être signalée"
    );

    // L'imbrication qui nous occupe : {a,b} et {a,b,c} expliquent tous deux [a,b].
    let mut attendus = vec![a.0, b.0];
    attendus.sort_unstable();
    let indice = ambiguites
        .iter()
        .position(|amb| {
            let mut c: Vec<u64> = amb.indices_communs.iter().map(|n| n.0).collect();
            c.sort_unstable();
            c == attendus
        })
        .unwrap_or_else(|| panic!("imbrication [a,b] absente — obtenu {ambiguites:?}"));
    let amb = ambiguites.remove(indice);
    assert_eq!(
        ensemble_trie(&mgr.store().get(amb.fragment).expect("f").node_ids()),
        ensemble_trie(&[a, b])
    );
    assert_eq!(
        ensemble_trie(&mgr.store().get(amb.complet).expect("c").node_ids()),
        ensemble_trie(&[a, b, c])
    );

    // Le bruit `d` fait apparaître AUSSI l'imbrication du second trio
    // ({d,e} ⊂ {d,e,f}) sur l'indice `d` : c'est correct — chaque ambiguïté ne
    // porte que sur les indices que SES deux ensembles expliquent.
    for amb in &ambiguites {
        assert!(
            amb.indices_communs.iter().all(|n| cues.contains(n)),
            "un indice commun ne peut être qu'un indice observé : {amb:?}"
        );
        assert!(
            amb.indices_communs.contains(&d),
            "toute autre ambiguïté sur [a,b,d] concerne l'indice d : {amb:?}"
        );
    }
}

/// CONTRE-ÉPREUVE : `[a,c]` ne signale PAS la paire.
///
/// L'ensemble `{a,b}` n'explique que `[a]`, le trio explique `[a,c]` : les deux
/// n'expliquent pas la même chose, donc il n'y a pas d'ambiguïté — le trio est
/// simplement mieux informé. C'est la condition qui empêche le signal de devenir
/// « un petit ensemble contient un indice, donc il est ambigu ».
#[test]
fn indice_a_c_ne_signale_pas_l_imbrication() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = base_de_reference(&dir.path().join("base"));
    let session = db.session();
    let a = id_de(&session, "a");
    let b = id_de(&session, "b");
    let c = id_de(&session, "c");

    let engine = db.cognitive_engine().expect("moteur");
    let mgr = engine.engram_manager().expect("manager");

    let cues = [a, c];
    let resultats = mgr.recall(&cues, 5);
    let ambiguites = diagnostiquer_imbrications(&cues, &resultats);
    assert!(
        ambiguites.is_empty(),
        "sur [a,c] le trio explique un indice de plus : pas d'ambiguïté — obtenu {ambiguites:?}"
    );
    // Et le trio gagne bien, puisque le fragment n'explique que [a].
    let rendus: Vec<Vec<u64>> = resultats
        .iter()
        .map(|r| ensemble_trie(&r.engram.node_ids()))
        .collect();
    assert_eq!(
        rendus.first(),
        Some(&{
            let mut v = vec![a.0, b.0, c.0];
            v.sort_unstable();
            v
        }),
        "sur [a,c], le trio complet arrive premier (obtenu {rendus:?})"
    );
}

/// L'ensemble indépendant (autre trio) n'est ni effondré ni signalé : le
/// diagnostic ne fabrique aucune ambiguïté là où il n'y en a pas.
#[test]
fn ensemble_independant_non_signale() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = base_de_reference(&dir.path().join("base"));
    let session = db.session();
    let d = id_de(&session, "d");
    let e = id_de(&session, "e");

    let engine = db.cognitive_engine().expect("moteur");
    let mgr = engine.engram_manager().expect("manager");

    let resultats = mgr.recall(&[d], 5);
    // {d,e} ⊂ {d,e,f} : même imbrication côté second trio.
    let ambiguites = diagnostiquer_imbrications(&[d], &resultats);
    assert_eq!(
        ambiguites.len(),
        1,
        "l'imbrication du second trio est signalée"
    );
    // Mais AUCUNE ambiguïté ne mélange les deux trios.
    for amb in &ambiguites {
        let frag = mgr.store().get(amb.fragment).expect("fragment");
        let comp = mgr.store().get(amb.complet).expect("complet");
        assert!(
            frag.node_ids().contains(&d) && comp.node_ids().contains(&d),
            "une ambiguïté ne concerne que les ensembles qui expliquent l'indice donné"
        );
    }
    let _ = e;
}

/// RÉOUVERTURE : les résultats et le diagnostic sont identiques après
/// fermeture/réouverture. Aucun état en mémoire n'est nécessaire au signal.
#[test]
fn reouverture_conserve_resultats_et_diagnostic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let chemin = dir.path().join("base");
    let (avant_rendus, avant_diag) = {
        let db = base_de_reference(&chemin);
        let session = db.session();
        let a = id_de(&session, "a");
        let engine = db.cognitive_engine().expect("moteur");
        let mgr = engine.engram_manager().expect("manager");
        let resultats = mgr.recall(&[a], 5);
        let rendus: Vec<Vec<u64>> = resultats
            .iter()
            .map(|r| ensemble_trie(&r.engram.node_ids()))
            .collect();
        let diag = diagnostiquer_imbrications(&[a], &resultats);
        db.close().expect("close");
        drop(db);
        (rendus, diag)
    };

    let db2 = ObrainDB::open_substrate_observed(&chemin).expect("reopen");
    let session2 = db2.session();
    let a2 = id_de(&session2, "a");
    let engine2 = db2.cognitive_engine().expect("moteur2");
    let mgr2 = engine2.engram_manager().expect("manager2");
    let resultats2 = mgr2.recall(&[a2], 5);
    let apres_rendus: Vec<Vec<u64>> = resultats2
        .iter()
        .map(|r| ensemble_trie(&r.engram.node_ids()))
        .collect();
    let apres_diag = diagnostiquer_imbrications(&[a2], &resultats2);

    // Le PRÉFIXE DÉCIDÉ doit être identique : {a,b} (0,5) devant {a,b,c} (~0,33).
    // Les positions suivantes ne sont PAS un contrat : deux engrammes de même
    // confiance (ici les deux morceaux de l'autre trio, ~0,26 tous deux)
    // s'échangent après réouverture, l'ordre entre ex æquo venant de l'itération
    // d'une table de hachage — mesuré ici, hors de la portée de ce lot.
    assert_eq!(
        avant_rendus.iter().take(2).collect::<Vec<_>>(),
        apres_rendus.iter().take(2).collect::<Vec<_>>(),
        "les deux premiers rangs (confiances distinctes) sont identiques"
    );
    let mut avant_ensemble = avant_rendus.clone();
    let mut apres_ensemble = apres_rendus.clone();
    avant_ensemble.sort_unstable();
    apres_ensemble.sort_unstable();
    assert_eq!(
        avant_ensemble, apres_ensemble,
        "le même ENSEMBLE de candidats est rendu (seul l'ordre des ex æquo bouge)"
    );
    assert_eq!(avant_diag.len(), apres_diag.len(), "même diagnostic");
    assert_eq!(
        avant_diag[0].indices_communs, apres_diag[0].indices_communs,
        "mêmes indices communs"
    );
    // Et la provenance reste vide après réouverture : le trou n'est pas comblé
    // par un redémarrage.
    let avec_provenance = mgr2
        .store()
        .list()
        .iter()
        .filter(|e| !e.source_episodes.is_empty())
        .count();
    assert_eq!(
        avec_provenance, 0,
        "toujours aucune provenance après réouverture"
    );
}
