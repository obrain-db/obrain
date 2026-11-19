//! Un échec d'ouverture au milieu des quatre zones simpliciales ne doit pas
//! laisser un cache à moitié rempli (obrain-db/obrain#238).
//!
//! `Writer::with_simplex_zones` ouvre quatre zones dans une seule section
//! critique et les mémorise dans un cache. La garde d'initialisation ne teste
//! que la PREMIÈRE (`simplex_dir.is_none()`) : si la troisième ouverture
//! échoue, le cache garde `dir = Some`, `heap = None`, et **tous** les appels
//! suivants sautent l'initialisation puis déballent un `None` — la panique
//! observée en CI sur `writer.rs:3481` (`Option::unwrap() on a None value`),
//! sur des tests tirés au hasard selon qui passait le premier.
//!
//! Ce test rend l'échec déterministe : un RÉPERTOIRE porte le nom du fichier de
//! la zone heap, donc son ouverture échoue toujours. Deux appels de suite :
//! le premier doit rendre une erreur d' E/S, le second aussi — jamais une
//! panique.
//!
//! ```bash
//! cargo test -p obrain-substrate --test zones_simplex_initialisation_partielle
//! ```

use std::fs;

use obrain_substrate::simplex_passes::SimplexPassesSpec;
use obrain_substrate::store::SubstrateStore;

fn spec() -> SimplexPassesSpec {
    SimplexPassesSpec {
        clique_edge_type: "CONTAINS".to_string(),
        clique_labels: Vec::new(),
        dispatch: None,
        maintenance: false,
    }
}

#[test]
fn un_echec_d_ouverture_ne_laisse_pas_un_cache_a_moitie_rempli() {
    let dir = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(dir.path()).unwrap();

    // Le nom exact de la zone heap, occupé par un répertoire : l'ouverture en
    // fichier échouera, quoi qu'il arrive. Si la création a déjà posé le
    // fichier, on le remplace.
    let heap = dir.path().join("substrate.simplex_heap");
    if heap.exists() {
        fs::remove_file(&heap).unwrap();
    }
    fs::create_dir(&heap).unwrap();

    let premier = store.configurer_passes_simpliciales(&spec());
    assert!(
        premier.is_err(),
        "l'ouverture de la zone heap doit échouer (répertoire au lieu d'un fichier)"
    );

    // Deuxième appel : avec la garde qui ne teste que `simplex_dir`, le cache
    // est jugé complet et le `unwrap` panique ici.
    let second = store.configurer_passes_simpliciales(&spec());
    assert!(
        second.is_err(),
        "le second appel doit rendre la même erreur d'ouverture, pas réussir"
    );
}
