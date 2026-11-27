//! Réservation d'identité générique (#416) — garanties de la primitive
//! `SubstrateStore::reserver_identite`, indépendante de `KnowledgeSpec` et
//! des engrammes.
//!
//! Les cinq garanties du livrable :
//! 1. deux appels concurrents, même clé + même définition → UNE identité ;
//! 2. même clé + définition incompatible → conflit explicite ;
//! 3. abandon/rollback → une nouvelle construction reste possible ;
//! 4. aucune visibilité partielle (chaque identité rendue est un nœud complet) ;
//! 5. réouverture + recherche par clé → l'identité est retrouvée, pas re-créée.

use std::collections::BTreeSet;
use std::sync::{Arc, Barrier};
use std::thread;

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::{GraphStore, GraphStoreMut};
use obrain_substrate::{IssueReservation, PROP_CLE, PROP_DEFINITION, SubstrateStore};

const KIND: u8 = 1;
const CLE: &[u8] = b"identite-canonique";
const DEFINITION: &[u8] = b"{\"titre\":\"def-v1\"}";

/// Le résultat d'une construction : `(labels, propriétés)` ou une erreur.
type Construction = obrain_substrate::SubstrateResult<(Vec<String>, Vec<(PropertyKey, Value)>)>;

fn construction_defaut() -> Construction {
    Ok((
        vec!["structure".to_string()],
        vec![(PropertyKey::new("payload"), Value::Int64(1))],
    ))
}

/// La clé persistée telle que la primitive l'écrit : `[kind] ++ cle`.
fn cle_persistee() -> Vec<u8> {
    std::iter::once(KIND).chain(CLE.iter().copied()).collect()
}

#[test]
fn concurrent_meme_cle_meme_definition_rend_une_seule_identite() {
    let td = tempfile::tempdir().unwrap();
    let store = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());

    let n = 8;
    let barriere = Arc::new(Barrier::new(n));
    let mut fils = Vec::new();
    for _ in 0..n {
        let store = Arc::clone(&store);
        let barriere = Arc::clone(&barriere);
        fils.push(thread::spawn(move || {
            barriere.wait();
            store
                .reserver_identite(KIND, CLE, DEFINITION, construction_defaut)
                .unwrap()
        }));
    }
    let sorties: Vec<IssueReservation> = fils.into_iter().map(|f| f.join().unwrap()).collect();

    let ids: BTreeSet<u32> = sorties.iter().map(IssueReservation::identite).collect();
    assert_eq!(
        ids.len(),
        1,
        "une seule identité sous concurrence : {ids:?}"
    );
    let crees = sorties.iter().filter(|r| r.est_creation()).count();
    assert_eq!(crees, 1, "une seule construction, les autres retrouvent");

    let vivants = store
        .node_ids()
        .into_iter()
        .filter(|id| {
            store
                .get_node(*id)
                .is_some_and(|no| no.labels.iter().any(|l| l.as_str() == "structure"))
        })
        .count();
    assert_eq!(vivants, 1, "un seul nœud vivant label+clé dans la base");
}

#[test]
fn meme_cle_definition_incompatible_rend_un_conflit_explicite() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(td.path().join("kb")).unwrap();

    let cree = store
        .reserver_identite(KIND, CLE, DEFINITION, construction_defaut)
        .unwrap();
    assert!(
        matches!(cree, IssueReservation::Cree(_)),
        "première réservation : {cree:?}"
    );

    let conflit = store
        .reserver_identite(KIND, CLE, b"definition-differente", construction_defaut)
        .unwrap();
    let IssueReservation::Conflit(id) = conflit else {
        panic!("attendu un conflit explicite, obtenu {conflit:?}");
    };
    assert_eq!(id, cree.identite(), "le conflit nomme l'identité existante");

    let vivants = store
        .node_ids()
        .into_iter()
        .filter(|n| {
            store
                .get_node(*n)
                .is_some_and(|no| no.labels.iter().any(|l| l.as_str() == "structure"))
        })
        .count();
    assert_eq!(vivants, 1, "le conflit n'écrit rien");
}

#[test]
fn abandon_puis_reconstruction_possible() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(td.path().join("kb")).unwrap();

    // Abandon : la construction échoue → aucune identité, aucune entrée d'index.
    let abandonne = store.reserver_identite(KIND, CLE, DEFINITION, || {
        Err(obrain_substrate::SubstrateError::Internal(
            "abandon simulé".into(),
        ))
    });
    assert!(
        abandonne.is_err(),
        "la construction refusée rend une erreur"
    );
    assert_eq!(store.node_ids().len(), 0, "aucun nœud après abandon");

    // Une construction suivante réussit : la réservation n'est pas restée
    // bloquée par l'abandon.
    let cree = store
        .reserver_identite(KIND, CLE, DEFINITION, construction_defaut)
        .unwrap();
    assert!(
        matches!(cree, IssueReservation::Cree(_)),
        "reconstruction possible : {cree:?}"
    );
    let id = cree.identite();

    // Rollback (= suppression du nœud) : l'identité redevient constructible,
    // et la nouvelle identité est distincte de l'ancienne.
    assert!(store.delete_node(NodeId(u64::from(id))));
    let recree = store
        .reserver_identite(KIND, CLE, DEFINITION, construction_defaut)
        .unwrap();
    assert!(
        matches!(recree, IssueReservation::Cree(_)),
        "après suppression, nouvelle identité : {recree:?}"
    );
    assert_ne!(
        recree.identite(),
        id,
        "nouvelle identité distincte de l'ancienne"
    );
}

#[test]
fn aucune_visibilite_partielle() {
    let td = tempfile::tempdir().unwrap();
    let store = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());

    let n = 8;
    let barriere = Arc::new(Barrier::new(n));
    let mut fils = Vec::new();
    for _ in 0..n {
        let store = Arc::clone(&store);
        let barriere = Arc::clone(&barriere);
        fils.push(thread::spawn(move || {
            barriere.wait();
            store
                .reserver_identite(KIND, CLE, DEFINITION, construction_defaut)
                .unwrap()
        }));
    }
    let sorties: Vec<IssueReservation> = fils.into_iter().map(|f| f.join().unwrap()).collect();

    for issue in &sorties {
        let id = issue.identite();
        let noeud = store.get_node(NodeId(u64::from(id))).expect("nœud vivant");
        assert!(
            noeud.labels.iter().any(|l| l.as_str() == "structure"),
            "labels complets, pas un nœud à demi construit"
        );
        match store.get_node_property(NodeId(u64::from(id)), &PropertyKey::new(PROP_CLE)) {
            Some(Value::Bytes(b)) => {
                assert_eq!(b.as_ref(), cle_persistee().as_slice(), "clé complète")
            }
            autre => panic!("clé réservée absente ou mal typée : {autre:?}"),
        }
        match store.get_node_property(NodeId(u64::from(id)), &PropertyKey::new(PROP_DEFINITION)) {
            Some(Value::Bytes(b)) => assert_eq!(b.as_ref(), DEFINITION, "définition complète"),
            autre => panic!("définition absente ou mal typée : {autre:?}"),
        }
        assert_eq!(
            store.get_node_property(NodeId(u64::from(id)), &PropertyKey::new("payload")),
            Some(Value::Int64(1)),
            "propriété appelante présente"
        );
    }
    assert_eq!(sorties.iter().filter(|r| r.est_creation()).count(), 1);
}

#[test]
fn reouverture_et_recherche_par_cle() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let id = {
        let store = SubstrateStore::create(&chemin).unwrap();
        let cree = store
            .reserver_identite(KIND, CLE, DEFINITION, construction_defaut)
            .unwrap();
        assert!(matches!(cree, IssueReservation::Cree(_)));
        store.close().unwrap();
        cree.identite()
    };

    // Réouverture : l'identité est retrouvée par la clé, pas re-créée.
    let store = SubstrateStore::open(&chemin).unwrap();
    let retrouvee = store
        .reserver_identite(KIND, CLE, DEFINITION, construction_defaut)
        .unwrap();
    assert_eq!(
        retrouvee,
        IssueReservation::Existante(id),
        "identité retrouvée après réouverture"
    );
}

/// La même clé dans deux `kind` distincts réserve DEUX identités (le kind est
/// partie intégrante de la clé d'identité).
#[test]
fn meme_cle_dans_deux_namespaces_rend_deux_identites() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let (a, b) = {
        let store = SubstrateStore::create(&chemin).unwrap();
        let a = store
            .reserver_identite(1, CLE, DEFINITION, construction_defaut)
            .unwrap();
        let b = store
            .reserver_identite(2, CLE, DEFINITION, construction_defaut)
            .unwrap();
        assert!(matches!(a, IssueReservation::Cree(_)), "{a:?}");
        assert!(matches!(b, IssueReservation::Cree(_)), "{b:?}");
        assert_ne!(
            a.identite(),
            b.identite(),
            "deux namespaces = deux identités"
        );
        store.close().unwrap();
        (a.identite(), b.identite())
    };

    // Après réouverture, chaque namespace retrouve la sienne.
    let store = SubstrateStore::open(&chemin).unwrap();
    assert_eq!(
        store
            .reserver_identite(1, CLE, DEFINITION, construction_defaut)
            .unwrap(),
        IssueReservation::Existante(a)
    );
    assert_eq!(
        store
            .reserver_identite(2, CLE, DEFINITION, construction_defaut)
            .unwrap(),
        IssueReservation::Existante(b)
    );
}

/// Deux définitions incompatibles sous concurrence : UNE construction, UN
/// conflit explicite, une seule identité — jamais deux nœuds.
#[test]
fn definition_incompatible_sous_concurrence_rend_un_conflit() {
    let td = tempfile::tempdir().unwrap();
    let store = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());

    let n = 2;
    let barriere = Arc::new(Barrier::new(n));
    let defs: [&[u8]; 2] = [b"def-A", b"def-B"];
    let mut fils = Vec::new();
    for def in defs {
        let store = Arc::clone(&store);
        let barriere = Arc::clone(&barriere);
        fils.push(thread::spawn(move || {
            barriere.wait();
            store
                .reserver_identite(KIND, CLE, def, construction_defaut)
                .unwrap()
        }));
    }
    let sorties: Vec<IssueReservation> = fils.into_iter().map(|f| f.join().unwrap()).collect();

    let crees = sorties.iter().filter(|r| r.est_creation()).count();
    let conflits = sorties
        .iter()
        .filter(|r| matches!(r, IssueReservation::Conflit(_)))
        .count();
    assert_eq!(crees, 1, "une seule construction : {sorties:?}");
    assert_eq!(conflits, 1, "un conflit explicite : {sorties:?}");
    let ids: BTreeSet<u32> = sorties.iter().map(IssueReservation::identite).collect();
    assert_eq!(ids.len(), 1, "une seule identité : {ids:?}");
}

/// La reconstruction paresseuse de l'index après réouverture retrouve TOUTES
/// les réservations (balayage des nœuds vivants), pas seulement la première.
#[test]
fn reconstruction_index_apres_reouverture_retrouve_toutes() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let (id1, id2, id3) = {
        let store = SubstrateStore::create(&chemin).unwrap();
        let a = store
            .reserver_identite(1, b"cle-1", DEFINITION, construction_defaut)
            .unwrap();
        let b = store
            .reserver_identite(2, b"cle-2", DEFINITION, construction_defaut)
            .unwrap();
        let c = store
            .reserver_identite(3, b"cle-3", DEFINITION, construction_defaut)
            .unwrap();
        store.close().unwrap();
        (a.identite(), b.identite(), c.identite())
    };

    let store = SubstrateStore::open(&chemin).unwrap();
    assert_eq!(
        store
            .reserver_identite(1, b"cle-1", DEFINITION, construction_defaut)
            .unwrap(),
        IssueReservation::Existante(id1)
    );
    assert_eq!(
        store
            .reserver_identite(2, b"cle-2", DEFINITION, construction_defaut)
            .unwrap(),
        IssueReservation::Existante(id2)
    );
    assert_eq!(
        store
            .reserver_identite(3, b"cle-3", DEFINITION, construction_defaut)
            .unwrap(),
        IssueReservation::Existante(id3)
    );
}

/// L'identité rendue est un SLOT de nœud `u32` (borné par l'espace de slots du
/// substrat) ; il résout en `NodeId(u64::from(slot))` vivant.
#[test]
fn identite_est_un_slot_u32_qui_resout_en_nodeid() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(td.path().join("kb")).unwrap();

    let cree = store
        .reserver_identite(KIND, CLE, DEFINITION, construction_defaut)
        .unwrap();
    let IssueReservation::Cree(slot) = cree else {
        panic!("attendu Cree, obtenu {cree:?}");
    };
    let nid = NodeId(u64::from(slot));
    let noeud = store.get_node(nid).expect("le slot résout en nœud vivant");
    assert!(noeud.labels.iter().any(|l| l.as_str() == "structure"));
    assert_eq!(store.node_ids(), vec![nid], "un seul nœud, le slot réservé");
}
