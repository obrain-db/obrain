//! T27 — l'ingestion des propriétés EN LOT doit rendre EXACTEMENT ce que rend
//! le chemin unitaire, après réouverture à froid.
//!
//! Ce test est la condition pour que `index_repository` passe par le lot : un
//! gain de ×100 sur l'écriture ne vaut rien s'il change une seule valeur lue.

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::traits::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;

/// Étiquettes, arêtes (source, cible, type) et propriétés par nœud du jeu.
type Jeu = (
    Vec<&'static str>,
    Vec<(u32, u32, &'static str)>,
    Vec<Vec<(String, Value)>>,
);

/// Le jeu : ce qu'un index de code produit, plus les cas limites du stockage.
fn jeu() -> Jeu {
    let n = 300u32;
    let labels: Vec<&str> = (0..n)
        .map(|i| if i % 3 == 0 { "Function" } else { "File" })
        .collect();
    let aretes: Vec<(u32, u32, &str)> = (1..n).map(|i| (i, i + 1, "CALLS")).collect();
    let longue = "x".repeat(700); // au-delà du seuil des colonnes blob (256)
    let mut props = Vec::new();
    for i in 0..n {
        let mut l: Vec<(String, Value)> = vec![
            (
                "qualified_name".into(),
                Value::String(format!("mod::f{i}").into()),
            ),
            ("name".into(), Value::String(format!("f{i}").into())),
            (
                "file_path".into(),
                Value::String(format!("src/{}.c", i % 17).into()),
            ),
            ("start_line".into(), Value::Int64(i as i64)),
            ("end_line".into(), Value::Int64(i as i64 + 10)),
            ("is_exported".into(), Value::Bool(i % 2 == 0)),
            ("ratio".into(), Value::Float64(i as f64 / 7.0)),
        ];
        if i % 5 == 0 {
            l.push(("text".into(), Value::String(longue.clone().into())));
        }
        if i == 42 {
            // Débordement : bien plus qu'une page de 4 Ko d'entrées.
            for j in 0..200 {
                l.push((
                    format!("extra_{j}"),
                    Value::String(format!("valeur-{j}-{}", "y".repeat(20)).into()),
                ));
            }
        }
        props.push(l);
    }
    (labels, aretes, props)
}

fn unitaire(dir: &std::path::Path) {
    let (labels, aretes, props) = jeu();
    let s = SubstrateStore::create(dir).unwrap();
    let ids: Vec<NodeId> = labels.iter().map(|l| s.create_node(&[l])).collect();
    for (i, l) in props.iter().enumerate() {
        for (k, v) in l {
            s.set_node_property(ids[i], k, v.clone());
        }
    }
    for (a, b, t) in aretes {
        s.create_edge(ids[a as usize - 1], ids[b as usize - 1], t);
    }
    s.checkpoint().unwrap();
}

fn en_lot(dir: &std::path::Path) {
    let (labels, aretes, props) = jeu();
    let s = SubstrateStore::create(dir).unwrap();
    s.bulk_import_labels_edges(&labels, &aretes).unwrap();
    let refs: Vec<Vec<(&str, Value)>> = props
        .iter()
        .map(|l| l.iter().map(|(k, v)| (k.as_str(), v.clone())).collect())
        .collect();
    // En deux paquets, pour éprouver `premier_slot`.
    let n = refs.len();
    s.bulk_import_node_props(1, &refs[..n / 2]).unwrap();
    s.bulk_import_node_props(1 + (n / 2) as u32, &refs[n / 2..])
        .unwrap();
    s.checkpoint().unwrap();
}

#[test]
fn le_lot_relit_exactement_comme_l_unitaire_apres_reouverture() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    unitaire(a.path());
    en_lot(b.path());
    let (sa, sb) = (
        SubstrateStore::open(a.path()).unwrap(),
        SubstrateStore::open(b.path()).unwrap(),
    );

    assert_eq!(sa.node_count(), sb.node_count(), "même nombre de nœuds");
    assert_eq!(sa.edge_count(), sb.edge_count(), "même nombre d'arêtes");

    let (_, _, props) = jeu();
    let mut comparees = 0;
    for (i, l) in props.iter().enumerate() {
        let id = NodeId((i + 1) as u64);
        for (k, v) in l {
            let cle = PropertyKey::new(k.as_str());
            let (va, vb) = (
                sa.get_node_property(id, &cle),
                sb.get_node_property(id, &cle),
            );
            assert_eq!(va.as_ref(), Some(v), "unitaire : nœud {i}, {k}");
            assert_eq!(vb.as_ref(), Some(v), "EN LOT : nœud {i}, {k}");
            comparees += 1;
        }
    }
    assert!(
        comparees > 2_000,
        "le jeu couvre bien des milliers de valeurs ({comparees})"
    );
}

#[test]
fn l_index_d_egalite_se_batit_sur_une_base_ingeree_en_lot() {
    let b = tempfile::tempdir().unwrap();
    en_lot(b.path());
    let s = SubstrateStore::open(b.path()).unwrap();
    s.create_property_index("qualified_name");
    let trouves = s.find_nodes_by_property("qualified_name", &Value::String("mod::f123".into()));
    assert_eq!(trouves, vec![NodeId(124)]);
}

/// La réservation laisse une fin de zone creuse : après réouverture, une
/// écriture ordinaire doit s'ajouter APRÈS les pages existantes — jamais les
/// recouvrir. On écrit sur plusieurs nœuds, puis on relit TOUT.
#[test]
fn apres_un_lot_une_ecriture_ordinaire_n_ecrase_rien() {
    let b = tempfile::tempdir().unwrap();
    en_lot(b.path());
    {
        let s = SubstrateStore::open(b.path()).unwrap();
        for i in [1u64, 42, 43, 150, 300] {
            s.set_node_property(
                NodeId(i),
                "ajout_tardif",
                Value::String(format!("tard-{i}").into()),
            );
            s.set_node_property(
                NodeId(i),
                "name",
                Value::String(format!("renomme-{i}").into()),
            );
        }
        s.checkpoint().unwrap();
    }
    let s = SubstrateStore::open(b.path()).unwrap();
    let (_, _, props) = jeu();
    for (i, l) in props.iter().enumerate() {
        let id = NodeId((i + 1) as u64);
        let touche = [1usize, 42, 43, 150, 300].contains(&(i + 1));
        for (k, v) in l {
            let lu = s.get_node_property(id, &PropertyKey::new(k.as_str()));
            if touche && k == "name" {
                assert_eq!(
                    lu,
                    Some(Value::String(format!("renomme-{}", i + 1).into())),
                    "nœud {i}"
                );
            } else {
                assert_eq!(lu.as_ref(), Some(v), "nœud {i}, {k} recouvert ?");
            }
        }
        if touche {
            assert_eq!(
                s.get_node_property(id, &PropertyKey::new("ajout_tardif")),
                Some(Value::String(format!("tard-{}", i + 1).into()))
            );
        }
    }
}

/// Les pages partent par blocs de 64 Mo : ce test en écrit ≈ 80 Mo de pages
/// de propriétés (20 000 nœuds) pour que le vidage INTERMÉDIAIRE ait lieu, puis
/// relit tout à froid.
#[test]
fn le_vidage_par_blocs_des_pages_ne_perd_rien() {
    let d = tempfile::tempdir().unwrap();
    let n = 20_000u32;
    {
        let s = SubstrateStore::create(d.path()).unwrap();
        let labels: Vec<&str> = (0..n).map(|_| "Function").collect();
        s.bulk_import_labels_edges(&labels, &[]).unwrap();
        let props: Vec<Vec<(&str, Value)>> = (0..n)
            .map(|i| vec![("qualified_name", Value::String(format!("q{i}").into()))])
            .collect();
        s.bulk_import_node_props(1, &props).unwrap();
        s.checkpoint().unwrap();
    }
    let s = SubstrateStore::open(d.path()).unwrap();
    for i in (0..n).step_by(97).chain([0, n - 1]) {
        assert_eq!(
            s.get_node_property(
                NodeId(u64::from(i) + 1),
                &PropertyKey::new("qualified_name")
            ),
            Some(Value::String(format!("q{i}").into())),
            "nœud {i}"
        );
    }
}

/// Idem pour le tas : ≈ 80 Mo de chaînes, donc au moins un vidage de bloc
/// intermédiaire des pages de tas neuves.
#[test]
fn le_vidage_par_blocs_du_tas_ne_perd_rien() {
    let d = tempfile::tempdir().unwrap();
    let n = 4_000u32;
    let cles: Vec<String> = (0..100).map(|j| format!("c{j}")).collect();
    let valeur = |i: u32, j: usize| format!("{i}-{j}-{}", "z".repeat(190));
    {
        let s = SubstrateStore::create(d.path()).unwrap();
        let labels: Vec<&str> = (0..n).map(|_| "Function").collect();
        s.bulk_import_labels_edges(&labels, &[]).unwrap();
        let props: Vec<Vec<(&str, Value)>> = (0..n)
            .map(|i| {
                cles.iter()
                    .enumerate()
                    .map(|(j, c)| (c.as_str(), Value::String(valeur(i, j).into())))
                    .collect()
            })
            .collect();
        s.bulk_import_node_props(1, &props).unwrap();
        s.checkpoint().unwrap();
    }
    let s = SubstrateStore::open(d.path()).unwrap();
    for i in (0..n).step_by(37).chain([0, n - 1]) {
        for j in [0usize, 49, 99] {
            assert_eq!(
                s.get_node_property(NodeId(u64::from(i) + 1), &PropertyKey::new(&cles[j])),
                Some(Value::String(valeur(i, j).into())),
                "nœud {i}, clé {j}"
            );
        }
    }
}
