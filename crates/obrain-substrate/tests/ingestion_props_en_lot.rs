//! T27 — l'ingestion des propriétés EN LOT doit rendre EXACTEMENT ce que rend
//! le chemin unitaire, après réouverture à froid.
//!
//! Ce test est la condition pour que `index_repository` passe par le lot : un
//! gain de ×100 sur l'écriture ne vaut rien s'il change une seule valeur lue.

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::traits::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;
use obrain_substrate::wal::WalPayload;
use obrain_substrate::wal_io::WalReader;

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
        // (#205) Un embedding sur un nœud sur quatre : le chemin lot des
        // vecteurs doit rendre exactement ce que rend le chemin unitaire.
        if i % 4 == 1 {
            let v: Vec<f32> = (0..8).map(|d| (i * 8 + d) as f32 / 3.0).collect();
            l.push(("embedding".into(), Value::Vector(v.into())));
        }
        // (#205) UNE valeur de chaque catégorie de blob, chacune sous sa propre
        // clé (une trame marqueur par clé : le compte du test WAL en dépend).
        match i {
            10 => l.push(("c_s300".into(), Value::String("a".repeat(300).into()))),
            11 => l.push(("c_s5k".into(), Value::String("é".repeat(2_500).into()))),
            12 => l.push(("c_b400".into(), Value::Bytes(octets(400, 3)))),
            13 => l.push(("c_b100k".into(), Value::Bytes(octets(100_000, 7)))),
            14 => {
                // bincode largement au-delà d'une page de 4 Ko
                let liste: Vec<Value> = (0..400)
                    .map(|j| Value::String(format!("élément-{j}-{}", "z".repeat(12)).into()))
                    .collect();
                l.push(("c_liste".into(), Value::List(liste.into())));
            }
            15 => l.push(("c_682k".into(), Value::String("m".repeat(682_431).into()))),
            _ => {}
        }
        props.push(l);
    }
    (labels, aretes, props)
}

/// Octets non constants (un motif répété masquerait un décalage d'offset).
fn octets(n: usize, graine: u8) -> std::sync::Arc<[u8]> {
    (0..n)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(graine))
        .collect::<Vec<u8>>()
        .into()
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

/// Rend le nombre de vecteurs de nœud écrits (#205 : doit valoir 75 sur le jeu).
fn en_lot(dir: &std::path::Path) -> u64 {
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
    s.nb_ecritures_vectorielles_noeuds()
}

#[test]
fn le_lot_relit_exactement_comme_l_unitaire_apres_reouverture() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    unitaire(a.path());
    let vecteurs_ecrits = en_lot(b.path());
    // 300 nœuds, un embedding sur quatre (i % 4 == 1) : 75 vecteurs, ni plus ni moins.
    assert_eq!(
        vecteurs_ecrits, 75,
        "chaque vecteur du lot est écrit une fois"
    );
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

/// Un import en lot sur une base qui porte DÉJÀ un index d'égalité : l'index
/// est tenu, jamais laissé partiel et réputé complet.
///
/// Le contrat d'origine était implicite (« les index se déclarent après
/// l'import ») : un index déclaré entre deux paquets restait marqué complet
/// alors qu'il ignorait tout le second paquet, et une recherche rendait `[]`
/// pour un nœud bien présent.
#[test]
fn un_import_en_lot_tient_un_index_d_egalite_deja_declare() {
    let (labels, aretes, props) = jeu();
    let b = tempfile::tempdir().unwrap();
    let verifier = |s: &SubstrateStore, contexte: &str| {
        for i in [10i64, 149, 150, 250, 299] {
            let attendu = vec![NodeId(i as u64 + 1)];
            assert_eq!(
                s.find_nodes_by_property("start_line", &Value::Int64(i)),
                attendu,
                "{contexte} : start_line = {i}"
            );
            assert_eq!(
                s.find_nodes_by_property(
                    "qualified_name",
                    &Value::String(format!("mod::f{i}").into())
                ),
                attendu,
                "{contexte} : qualified_name de {i}"
            );
            assert_eq!(
                s.find_nodes_by_properties(&[
                    ("is_exported", Value::Bool(i % 2 == 0)),
                    ("start_line", Value::Int64(i)),
                ]),
                attendu,
                "{contexte} : deux conditions pour {i}"
            );
        }
    };
    {
        let s = SubstrateStore::create(b.path()).unwrap();
        s.bulk_import_labels_edges(&labels, &aretes).unwrap();
        let refs: Vec<Vec<(&str, Value)>> = props
            .iter()
            .map(|l| l.iter().map(|(k, v)| (k.as_str(), v.clone())).collect())
            .collect();
        let n = refs.len();
        s.bulk_import_node_props(1, &refs[..n / 2]).unwrap();
        // Index déclarés ENTRE les deux paquets : bâtis, complets pour le
        // premier paquet, et marqués comme tels.
        for cle in ["start_line", "qualified_name", "is_exported"] {
            s.create_property_index(cle);
        }
        assert_eq!(
            s.find_nodes_by_property("start_line", &Value::Int64(10)),
            vec![NodeId(11)]
        );
        s.bulk_import_node_props(1 + (n / 2) as u32, &refs[n / 2..])
            .unwrap();
        verifier(&s, "dans la session de l'import");
        s.checkpoint().unwrap();
    }
    let s = SubstrateStore::open(b.path()).unwrap();
    verifier(&s, "après réouverture");
}

/// Trames vectorielles présentes au WAL : (`VecColumnWrite`, `VecColumnSet`).
fn trames_vectorielles(dir: &std::path::Path) -> (usize, usize) {
    let mut n = (0, 0);
    for (rec, _, _) in WalReader::open(dir.join("substrate.wal"))
        .expect("relire le WAL")
        .iter_from(0)
        .filter_map(Result::ok)
    {
        match rec.payload {
            WalPayload::VecColumnWrite { .. } => n.0 += 1,
            WalPayload::VecColumnSet { .. } => n.1 += 1,
            _ => {}
        }
    }
    n
}

fn refs_du_jeu(props: &[Vec<(String, Value)>]) -> Vec<Vec<(&str, Value)>> {
    props
        .iter()
        .map(|l| l.iter().map(|(k, v)| (k.as_str(), v.clone())).collect())
        .collect()
}

/// (#205) Le gain du lot vient de là : AUCUNE trame `VecColumnSet` (le vecteur
/// complet, ~3 Ko à 768 dimensions) et UNE SEULE trame `VecColumnWrite` par clé
/// et par appel — celle qui déplace l'offset du WAL pour qu'un index vectoriel
/// persisté ne soit pas validé à tort (O-VEC C3). Le témoin unitaire, sur les
/// mêmes vecteurs, journalise chaque vecteur : sans lui, un WAL tronqué ou relu
/// au mauvais endroit rendrait aussi zéro et le test ne prouverait rien.
#[test]
fn le_lot_n_ecrit_qu_un_marqueur_vectoriel_par_cle_au_wal() {
    let (labels, aretes, props) = jeu();
    let vecteurs = props
        .iter()
        .flatten()
        .filter(|(_, v)| matches!(v, Value::Vector(_)))
        .count();
    assert_eq!(vecteurs, 75, "le jeu porte 75 vecteurs");

    // Témoin : le chemin unitaire journalise chaque vecteur.
    let a = tempfile::tempdir().unwrap();
    {
        let s = SubstrateStore::create(a.path()).unwrap();
        let ids: Vec<NodeId> = labels.iter().map(|l| s.create_node(&[l])).collect();
        for (i, l) in props.iter().enumerate() {
            for (k, v) in l {
                if matches!(v, Value::Vector(_)) {
                    s.set_node_property(ids[i], k, v.clone());
                }
            }
        }
        s.flush().unwrap();
    }
    let (marqueurs, vecteurs_au_wal) = trames_vectorielles(a.path());
    assert_eq!(
        (marqueurs, vecteurs_au_wal),
        (vecteurs, vecteurs),
        "témoin : le chemin unitaire écrit un marqueur et un vecteur par vecteur"
    );

    // Lot : un seul appel, une seule clé vectorielle → un marqueur, aucun vecteur.
    let b = tempfile::tempdir().unwrap();
    {
        let s = SubstrateStore::create(b.path()).unwrap();
        s.bulk_import_labels_edges(&labels, &aretes).unwrap();
        s.bulk_import_node_props(1, &refs_du_jeu(&props)).unwrap();
        s.flush().unwrap();
    }
    assert_eq!(
        trames_vectorielles(b.path()),
        (1, 0),
        "le lot écrit un marqueur VecColumnWrite par clé, et aucun vecteur au WAL"
    );
}

/// (#205, revue de #208) Les vecteurs ne passent plus par `set_node_property`,
/// qui refusait l'écriture en lecture seule : le lot doit refuser lui-même,
/// comme `bulk_import_edge_props`, plutôt que d'écrire dans la colonne.
#[test]
#[should_panic(expected = "bulk_import_node_props")]
fn le_lot_refuse_une_base_ouverte_en_lecture_seule() {
    let (labels, aretes, props) = jeu();
    let d = tempfile::tempdir().unwrap();
    {
        let s = SubstrateStore::create(d.path()).unwrap();
        s.bulk_import_labels_edges(&labels, &aretes).unwrap();
        s.checkpoint().unwrap();
    }
    let s = SubstrateStore::open_read_only(d.path()).unwrap();
    let _ = s.bulk_import_node_props(1, &refs_du_jeu(&props));
}

/// Trames blob présentes au WAL : (`BlobColumnWrite`, clés distinctes).
fn trames_blob(dir: &std::path::Path) -> (usize, usize) {
    let mut cles = std::collections::HashSet::new();
    let mut n = 0;
    for (rec, _, _) in WalReader::open(dir.join("substrate.wal"))
        .expect("relire le WAL")
        .iter_from(0)
        .filter_map(Result::ok)
    {
        if let WalPayload::BlobColumnWrite { key, .. } = rec.payload {
            n += 1;
            cles.insert(key);
        }
    }
    (n, cles.len())
}

fn blobs_du_jeu(props: &[Vec<(String, Value)>]) -> usize {
    props
        .iter()
        .flatten()
        .filter(|(_, v)| match v {
            Value::String(t) => t.len() > 256,
            Value::Bytes(b) => b.len() > 256,
            Value::List(_) => true,
            _ => false,
        })
        .count()
}

/// (#205) Le gain du lot vient de là : au lieu d'une trame `BlobColumnWrite`
/// par valeur (la valeur entière, jusqu'à 682 Ko), UNE trame par clé et par
/// appel — celle qui déplace l'offset du WAL. Le témoin unitaire, sur les mêmes
/// valeurs, en journalise une par valeur : sans lui, un WAL relu au mauvais
/// endroit rendrait aussi peu de trames et le test ne prouverait rien. Ce test
/// vérifie aussi que le lot relit les valeurs de chaque catégorie à l'identique
/// après réouverture à froid (celles du jeu, dont 682 Ko).
#[test]
fn le_lot_n_ecrit_qu_une_trame_blob_par_cle_au_wal() {
    let (labels, aretes, props) = jeu();
    let blobs = blobs_du_jeu(&props);
    // 60 valeurs `text` + 6 catégories (c_s300 … c_682k).
    assert_eq!(blobs, 66, "le jeu porte 66 valeurs blob");

    let a = tempfile::tempdir().unwrap();
    {
        let s = SubstrateStore::create(a.path()).unwrap();
        let ids: Vec<NodeId> = labels.iter().map(|l| s.create_node(&[l])).collect();
        for (i, l) in props.iter().enumerate() {
            for (k, v) in l {
                s.set_node_property(ids[i], k, v.clone());
            }
        }
        s.flush().unwrap();
    }
    assert_eq!(
        trames_blob(a.path()),
        (blobs, 7),
        "témoin : le chemin unitaire journalise chaque valeur blob"
    );

    let b = tempfile::tempdir().unwrap();
    {
        let s = SubstrateStore::create(b.path()).unwrap();
        s.bulk_import_labels_edges(&labels, &aretes).unwrap();
        s.bulk_import_node_props(1, &refs_du_jeu(&props)).unwrap();
        s.flush().unwrap();
    }
    assert_eq!(
        trames_blob(b.path()),
        (7, 7),
        "le lot écrit une trame BlobColumnWrite par clé (7 clés blob), pas une par valeur"
    );
    // Réouverture à froid : chaque valeur, de chaque catégorie, à l'identique.
    let (sa, sb) = (
        SubstrateStore::open(a.path()).unwrap(),
        SubstrateStore::open(b.path()).unwrap(),
    );
    let mut relues = 0;
    for (i, l) in props.iter().enumerate() {
        let id = NodeId((i + 1) as u64);
        for (k, v) in l {
            let cle = PropertyKey::new(k.as_str());
            if blobs_du_jeu(&[vec![(k.clone(), v.clone())]]) == 1 {
                assert_eq!(
                    sa.get_node_property(id, &cle).as_ref(),
                    Some(v),
                    "unitaire {k}"
                );
                assert_eq!(sb.get_node_property(id, &cle).as_ref(), Some(v), "lot {k}");
                relues += 1;
            }
        }
    }
    assert_eq!(relues, blobs);
}

/// (#205) Une clé qui porte un index d'égalité garde le chemin unitaire (retrait
/// de l'ancienne valeur, entrée d'instant : voir la doc de la fonction), et la
/// recherche rend les mêmes porteurs que sur la base unitaire.
#[test]
fn une_cle_blob_indexee_garde_le_chemin_unitaire() {
    let (labels, aretes, props) = jeu();
    let longue = Value::String("x".repeat(700).into());
    let porteurs: Vec<NodeId> = (0..300u64)
        .filter(|i| i % 5 == 0)
        .map(|i| NodeId(i + 1))
        .collect();
    let d = tempfile::tempdir().unwrap();
    {
        let s = SubstrateStore::create(d.path()).unwrap();
        s.bulk_import_labels_edges(&labels, &aretes).unwrap();
        // Index déclaré ENTRE deux paquets (l'index n'est enregistré qu'une fois
        // bâti, et le seul moment où il l'est avec des propriétés derrière lui).
        let refs = refs_du_jeu(&props);
        let moitie = refs.len() / 2;
        s.bulk_import_node_props(1, &refs[..moitie]).unwrap();
        s.create_property_index("text");
        s.bulk_import_node_props(1 + moitie as u32, &refs[moitie..])
            .unwrap();
        assert_eq!(s.find_nodes_by_property("text", &longue), porteurs);
        s.flush().unwrap();
    }
    // 1er paquet : une trame par clé (7). 2e paquet : `text` indexée garde le
    // chemin unitaire, soit une trame par valeur (30, les nœuds >= 150), pendant
    // que les 6 autres clés ne pèsent plus rien (leurs valeurs sont dans le 1er).
    let (n, cles) = trames_blob(d.path());
    assert_eq!(
        (n, cles),
        (7 + 30, 7),
        "text indexée : une trame par valeur"
    );
    let s = SubstrateStore::open(d.path()).unwrap();
    assert_eq!(s.find_nodes_by_property("text", &longue), porteurs);
}

/// (#205) Un lot de blobs refuse une base ouverte en lecture seule, comme celui
/// des vecteurs — un jeu SANS vecteur, pour que ce soit bien le chemin blob.
#[test]
#[should_panic(expected = "bulk_import_node_props")]
fn le_lot_de_blobs_refuse_une_base_ouverte_en_lecture_seule() {
    let (labels, aretes, props) = jeu();
    let d = tempfile::tempdir().unwrap();
    {
        let s = SubstrateStore::create(d.path()).unwrap();
        s.bulk_import_labels_edges(&labels, &aretes).unwrap();
        s.checkpoint().unwrap();
    }
    let seuls: Vec<Vec<(String, Value)>> = props
        .into_iter()
        .map(|l| l.into_iter().filter(|(k, _)| k.starts_with("c_")).collect())
        .collect();
    let s = SubstrateStore::open_read_only(d.path()).unwrap();
    let _ = s.bulk_import_node_props(1, &refs_du_jeu(&seuls));
}
