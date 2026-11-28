//! Contrat de visibilité de la réservation d'identité (#435) — les quatre
//! démonstrations du correctif de notification des index sur la naissance
//! différée (`fix(substrate): restaure la notification des index`).
//!
//! 1. un observateur notifié lit un nœud COMPLET (pas un nœud partiel) ;
//! 2. une création engagée devient recherchable (label + propriété) ;
//! 3. interruption après publication + réouverture → état cohérent, index
//!    reconstruit, aucune perte d'indexation ;
//! 4. le vecteur d'embedding est notifié à l'index vectoriel (LangChain /
//!    LlamaIndex) — `similarity_search` ne rend plus vide.

use std::sync::{Arc, Mutex};

use obrain_common::types::{EdgeId, NodeId, PropertyKey, Value};
use obrain_core::graph::GraphStore;
use obrain_substrate::{IssueReservation, StoreMutationObserver, SubstrateStore};

const KIND: u8 = 1;
const CLE: &[u8] = b"cle-visibilite";
const DEFINITION: &[u8] = b"definition-visibilite";

/// Le résultat d'une construction : `(labels, propriétés)` ou une erreur.
type Construction = obrain_substrate::SubstrateResult<(Vec<String>, Vec<(PropertyKey, Value)>)>;

fn construction() -> Construction {
    Ok((
        vec!["structure".to_string()],
        vec![(PropertyKey::new("payload"), Value::Int64(1))],
    ))
}

fn construction_avec_vecteur() -> Construction {
    Ok((
        vec!["structure".to_string()],
        vec![
            (PropertyKey::new("payload"), Value::Int64(1)),
            (
                PropertyKey::new("embedding"),
                Value::Vector(Arc::from(vec![1.0f32, 2.0, 3.0])),
            ),
        ],
    ))
}

/// Observateur de test : enregistre ce qu'il reçoit et lit le nœud au moment
/// de `node_created` (qui doit arriver APRÈS le scellement, quand le nœud est
/// complet — jamais un nœud partiel).
struct ObservateurDeTest {
    store: Arc<SubstrateStore>,
    noeuds_crees: Mutex<Vec<NodeId>>,
    proprietes: Mutex<Vec<String>>,
    vecteurs: Mutex<Vec<(String, usize)>>,
    payloads_lus: Mutex<Vec<Option<Value>>>,
}

impl StoreMutationObserver for ObservateurDeTest {
    fn node_created(&self, id: NodeId, _epoch: u64) {
        // Au moment de la notification (APRÈS le scellement), le nœud est
        // complet : le payload est déjà lisible.
        let payload = self
            .store
            .get_node_property(id, &PropertyKey::new("payload"));
        self.noeuds_crees.lock().unwrap().push(id);
        self.payloads_lus.lock().unwrap().push(payload);
    }

    fn edge_created(&self, _id: EdgeId, _src: NodeId, _dst: NodeId, _epoch: u64) {}

    fn vector_written(&self, id: NodeId, key: &str, vector: &[f32]) {
        self.vecteurs
            .lock()
            .unwrap()
            .push((format!("{id}:{key}"), vector.len()));
    }

    fn node_property_set(&self, id: NodeId, key: &str) {
        self.proprietes.lock().unwrap().push(format!("{id}:{key}"));
    }
}

fn observateur(store: Arc<SubstrateStore>) -> Arc<ObservateurDeTest> {
    Arc::new(ObservateurDeTest {
        store,
        noeuds_crees: Mutex::new(Vec::new()),
        proprietes: Mutex::new(Vec::new()),
        vecteurs: Mutex::new(Vec::new()),
        payloads_lus: Mutex::new(Vec::new()),
    })
}

/// (a) Un observateur notifié lit un nœud COMPLET — `node_created` et
/// `node_property_set` arrivent APRÈS le scellement, quand le payload est posé.
#[test]
fn observateur_notifie_lit_un_noeud_complet() {
    let td = tempfile::tempdir().unwrap();
    let store = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let obs = observateur(Arc::clone(&store));
    store.set_mutation_observer(obs.clone());

    let cree = store
        .reserver_identite(KIND, CLE, DEFINITION, construction)
        .unwrap();
    let IssueReservation::Cree(slot) = cree else {
        panic!("attendu Cree, obtenu {cree:?}");
    };
    let nid = NodeId(u64::from(slot));

    assert_eq!(
        *obs.noeuds_crees.lock().unwrap(),
        vec![nid],
        "node_created notifié pour le nœud"
    );
    assert!(
        obs.proprietes
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.ends_with(":payload")),
        "node_property_set notifié pour payload"
    );
    assert_eq!(
        obs.payloads_lus.lock().unwrap().as_slice(),
        &[Some(Value::Int64(1))],
        "l'observateur lit un nœud complet (payload posé), pas un nœud partiel"
    );
}

/// (b) Une création engagée devient recherchable — par label ET par propriété,
/// immédiatement après l'engagement.
#[test]
fn creation_engagee_devient_recherchable() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(td.path().join("kb")).unwrap();

    let cree = store
        .reserver_identite(KIND, CLE, DEFINITION, construction)
        .unwrap();
    let nid = NodeId(u64::from(cree.identite()));

    assert_eq!(
        store.nodes_by_label("structure"),
        vec![nid],
        "recherchable par label"
    );
    assert_eq!(
        store.find_nodes_by_property("payload", &Value::Int64(1)),
        vec![nid],
        "recherchable par propriété"
    );
}

/// (c) Interruption après publication + réouverture → état cohérent : l'index
/// de réservation est reconstruit, la recherche par label et par propriété
/// retrouve le nœud — aucune perte d'indexation.
#[test]
fn interruption_apres_publication_puis_reouverture_sans_perte_d_indexation() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let nid = {
        let store = SubstrateStore::create(&chemin).unwrap();
        let cree = store
            .reserver_identite(KIND, CLE, DEFINITION, construction)
            .unwrap();
        store.close().unwrap();
        NodeId(u64::from(cree.identite()))
    };

    let store = SubstrateStore::open(&chemin).unwrap();
    assert_eq!(
        store
            .reserver_identite(KIND, CLE, DEFINITION, construction)
            .unwrap(),
        IssueReservation::Existante(nid.0 as u32),
        "identité retrouvée après réouverture"
    );
    assert_eq!(
        store.nodes_by_label("structure"),
        vec![nid],
        "recherche par label après réouverture"
    );
    assert_eq!(
        store.find_nodes_by_property("payload", &Value::Int64(1)),
        vec![nid],
        "recherche par propriété après réouverture"
    );
}

/// (d) Le vecteur d'embedding est notifié à l'index vectoriel (LangChain /
/// LlamaIndex) : après une création atomique, `vector_written` est émis avec la
/// bonne dimension — `similarity_search` ne rend plus vide.
#[test]
fn index_vectoriel_recu_le_vecteur_similarite_non_vide() {
    let td = tempfile::tempdir().unwrap();
    let store = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let obs = observateur(Arc::clone(&store));
    store.set_mutation_observer(obs.clone());

    let cree = store
        .reserver_identite(KIND, CLE, DEFINITION, construction_avec_vecteur)
        .unwrap();
    assert!(matches!(cree, IssueReservation::Cree(_)), "{cree:?}");

    let vecteurs = obs.vecteurs.lock().unwrap();
    assert_eq!(
        vecteurs.len(),
        1,
        "un seul vecteur notifié à l'index vectoriel : {vecteurs:?}"
    );
    let (cle, dim) = &vecteurs[0];
    assert!(
        cle.ends_with(":embedding"),
        "le vecteur notifié est l'embedding : {cle}"
    );
    assert_eq!(*dim, 3, "dimension du vecteur préservée");
}
