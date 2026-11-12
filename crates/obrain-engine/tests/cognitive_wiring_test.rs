//! Câblage mutation → énergie / synapses par l'API PUBLIQUE (2026-11-12).
//!
//! Rouge sans le correctif (observateur substrat → `MutationBus`, runtime
//! dédié) : après un `CREATE` GQL, `energy_store()` et `synapse_store()`
//! restaient à zéro — les listeners étaient enregistrés sur un bus que
//! personne n'alimentait. Aucun câblage manuel ici : c'est exactement ce
//! qu'un utilisateur de `ObrainDB` obtient.
#![cfg(feature = "cognitive")]

use std::time::{Duration, Instant};

use obrain_common::types::NodeId;
use obrain_engine::ObrainDB;

const ATTENTE: Duration = Duration::from_secs(5);

fn ids_des_noeuds(session: &obrain_engine::Session, label: &str) -> Vec<NodeId> {
    session
        .execute(&format!("MATCH (n:{label}) RETURN id(n) ORDER BY id(n)"))
        .expect("ids")
        .rows
        .iter()
        .filter_map(|r| r.first())
        .map(|v| NodeId(v.to_string().parse::<u64>().expect("id entier")))
        .collect()
}

/// Un CREATE par GQL, puis l'énergie des nœuds touchés est > 0 et une
/// synapse existe entre les extrémités de l'arête — sans rien brancher.
fn verifie_energie_et_synapses(db: &ObrainDB) {
    let session = db.session();
    session
        .execute("CREATE (:Doc {name:'a'})-[:LINKS_TO]->(:Doc {name:'b'})")
        .expect("create");
    let ids = ids_des_noeuds(&session, "Doc");
    assert_eq!(ids.len(), 2, "deux nœuds créés");
    let (a, b) = (ids[0], ids[1]);

    assert!(
        db.cognitive_quiesce(ATTENTE),
        "les événements publiés doivent être dispatchés avant {ATTENTE:?} \
         (publiés={}, dispatchés={}, perdus={})",
        db.cognitive_events_published(),
        db.cognitive_events_dispatched(),
        db.cognitive_events_lost()
    );
    assert_eq!(db.cognitive_events_lost(), 0, "aucun événement perdu");

    // Anti-boucle : ce que la cognition écrit (arêtes SYNAPSE, propriétés
    // `_cog_*`) ne doit pas republier d'événement. Le compte des publiés
    // est STABLE une fois les listeners passés (délai de lot : 50 ms).
    let publies = db.cognitive_events_published();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        db.cognitive_events_published(),
        publies,
        "les écritures des listeners relancent la cognition (boucle)"
    );

    let engine = db.cognitive_engine().expect("moteur cognitif construit");
    let energy = engine.energy_store().expect("energy store");
    let synapses = engine.synapse_store().expect("synapse store");

    assert!(
        energy.get_energy(a) > 0.0,
        "énergie de a après CREATE : {}",
        energy.get_energy(a)
    );
    assert!(
        energy.get_energy(b) > 0.0,
        "énergie de b après CREATE : {}",
        energy.get_energy(b)
    );
    assert!(
        synapses.get_synapse(a, b).is_some() || synapses.get_synapse(b, a).is_some(),
        "une synapse entre les extrémités de l'arête (synapses={})",
        synapses.len()
    );
}

/// Chemin `with_config` (`new_in_memory`) : base tempfile substrat.
/// Fonction de test SYNCHRONE, sans runtime tokio : la base doit créer le sien.
#[test]
fn creation_gql_alimente_energie_et_synapses_sans_runtime_tokio() {
    let db = ObrainDB::new_in_memory();
    verifie_energie_et_synapses(&db);
}

/// Chemin `open_substrate` (`with_substrate_store`) : base sur disque.
#[test]
fn creation_gql_alimente_energie_et_synapses_sur_substrat_fichier() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = ObrainDB::open_substrate(dir.path().join("base")).expect("open");
    verifie_energie_et_synapses(&db);
}

/// Dans un runtime tokio déjà présent (cas du hub) : pas de runtime dédié,
/// le scheduler s'accroche au runtime courant ; et le drop de la base dans
/// un contexte async ne doit ni bloquer ni paniquer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn creation_gql_alimente_energie_dans_un_runtime_tokio_existant() {
    let db = ObrainDB::new_in_memory();
    verifie_energie_et_synapses(&db);
    drop(db);
}

/// Le plan chaud n'attend jamais la cognition : 200 CREATE successifs ne
/// doivent pas coûter plus que quelques ms chacun en moyenne, listeners
/// enregistrés ou non. Borne large (50 ms/commit) : c'est un garde-fou
/// contre un dispatch synchrone, pas un bench.
#[test]
fn le_commit_n_attend_pas_le_dispatch_des_listeners() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    let debut = Instant::now();
    for i in 0..200 {
        session
            .execute(&format!("CREATE (:Doc {{name:'n{i}'}})"))
            .expect("create");
    }
    let par_commit = debut.elapsed() / 200;
    assert!(
        par_commit < Duration::from_millis(50),
        "commit moyen {par_commit:?} : le dispatch cognitif bloque le plan chaud"
    );
    assert!(db.cognitive_quiesce(ATTENTE), "quiesce après 200 créations");
    let engine = db.cognitive_engine().expect("moteur");
    assert_eq!(
        engine.energy_store().expect("energy").len(),
        200,
        "200 nœuds énergisés"
    );
}
