//! #386 — LE LOT NE DOIT PAS DÉTACHER LA CHAÎNE DE PROPRIÉTÉS DÉJÀ ÉCRITE.
//!
//! `bulk_import_node_props` construit ses pages puis pose `first_prop_off` en
//! une passe, sans relire l'ancienne valeur : sur une entité qui portait DÉJÀ
//! des propriétés, la tête est écrasée et la chaîne précédente devient
//! inatteignable — la chaîne entière, pas « une propriété manquante ».
//!
//! Ces tests décrivent le contrat attendu dans les deux ordres d'écriture :
//!   * unitaire PUIS lot  → le lot doit chaîner sur la tête existante ;
//!   * lot PUIS unitaire  → le chemin unitaire chaîne déjà (garde de non-régression).

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::traits::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;

/// Propriétés unitaires : 400 entrées, soit PLUSIEURS pages de 4 Kio — c'est
/// la CHAÎNE qu'on veut retrouver, pas seulement la dernière page.
const AVANT: usize = 400;

fn cles_avant() -> Vec<String> {
    (0..AVANT).map(|j| format!("avant_{j}")).collect()
}

/// Écrit les propriétés unitaires et rend le nombre de celles qui ne sont plus
/// lisibles, en nommant les premières perdues.
fn perdues(s: &SubstrateStore, n: NodeId) -> usize {
    let mut perdues = 0usize;
    for (j, k) in cles_avant().iter().enumerate() {
        let lu = s.get_node_property(n, &PropertyKey::new(k.as_str()));
        if lu.as_ref() != Some(&Value::Int64(j as i64)) {
            perdues += 1;
            if perdues <= 5 {
                eprintln!("  {k} (attendu Int64({j})) → lu {lu:?}");
            }
        }
    }
    perdues
}

#[test]
fn le_lot_ne_detache_pas_la_chaine_ecrite_avant_lui() {
    let d = tempfile::tempdir().unwrap();
    let s = SubstrateStore::create(d.path()).unwrap();
    let n = s.create_node(&["Function"]);

    // 1. Un écrivain UNITAIRE remplit la chaîne de l'entité (plusieurs pages).
    for (j, k) in cles_avant().iter().enumerate() {
        s.set_node_property(n, k, Value::Int64(j as i64));
    }
    assert_eq!(perdues(&s, n), 0, "les écritures unitaires se relisent");

    // 2. Un import EN LOT écrit la MÊME entité.
    s.bulk_import_node_props(
        n.0 as u32,
        &[vec![("lot_a", Value::Int64(1)), ("lot_b", Value::Int64(2))]],
    )
    .unwrap();

    // 3. RELECTURE EXHAUSTIVE : rien n'a le droit d'avoir disparu.
    let perdues_ici = perdues(&s, n);
    if perdues_ici > 0 {
        eprintln!(
            "PERTES : {perdues_ici} propriétés unitaires sur {AVANT} rendues inatteignables \
             par l'import en lot"
        );
    }
    assert_eq!(
        perdues_ici, 0,
        "le lot a écrasé la tête de chaîne : {perdues_ici} propriétés sur {AVANT} \
         ne sont plus lisibles"
    );
    assert_eq!(
        s.get_node_property(n, &PropertyKey::new("lot_a")),
        Some(Value::Int64(1)),
        "et les propriétés du lot sont écrites"
    );
    assert_eq!(
        s.get_node_property(n, &PropertyKey::new("lot_b")),
        Some(Value::Int64(2))
    );

    // 4. Après réouverture à froid, même compte : rien n'était qu'en mémoire.
    s.checkpoint().unwrap();
    drop(s);
    let s = SubstrateStore::open(d.path()).unwrap();
    let perdues_froid = perdues(&s, n);
    assert_eq!(
        perdues_froid, 0,
        "à froid : {perdues_froid} propriétés perdues"
    );
    assert_eq!(
        s.get_node_property(n, &PropertyKey::new("lot_a")),
        Some(Value::Int64(1))
    );
}

#[test]
fn l_ecrivain_unitaire_apres_le_lot_ne_perd_rien() {
    // L'autre ordre est déjà correct (le chemin unitaire relit la tête et
    // chaîne dessus) : ce test le fige, pour que le correctif de #386 ne
    // puisse pas casser la direction qui marchait.
    let d = tempfile::tempdir().unwrap();
    let s = SubstrateStore::create(d.path()).unwrap();
    let n = s.create_node(&["Function"]);
    let cles: Vec<String> = (0..200).map(|j| format!("lot_{j}")).collect();
    let refs: Vec<(&str, Value)> = cles
        .iter()
        .enumerate()
        .map(|(j, k)| (k.as_str(), Value::Int64(j as i64)))
        .collect();
    s.bulk_import_node_props(n.0 as u32, &[refs]).unwrap();

    for (j, k) in cles.iter().enumerate() {
        s.set_node_property(n, k, Value::Int64(1000 + j as i64));
    }
    for (j, k) in cles.iter().enumerate() {
        assert_eq!(
            s.get_node_property(n, &PropertyKey::new(k.as_str())),
            Some(Value::Int64(1000 + j as i64)),
            "{k} : écriture unitaire après le lot"
        );
    }
    assert_eq!(
        s.get_node_property(n, &PropertyKey::new("nouvelle")),
        None,
        "témoin : une clé jamais écrite reste absente"
    );
    s.set_node_property(n, "nouvelle", Value::Bool(true));
    assert_eq!(
        s.get_node_property(n, &PropertyKey::new("nouvelle")),
        Some(Value::Bool(true)),
        "le chemin unitaire ajoute après le lot"
    );
}

#[test]
fn un_lot_sur_des_entites_neuves_ecrit_tout() {
    // Le cas NORMAL (base neuve, l'import d'index) : le correctif ne doit rien
    // y changer, ni en nombre de valeurs écrites, ni en relecture.
    let d = tempfile::tempdir().unwrap();
    let s = SubstrateStore::create(d.path()).unwrap();
    let n = 500u32;
    let labels: Vec<&str> = (0..n).map(|_| "File").collect();
    s.bulk_import_labels_edges(&labels, &[]).unwrap();
    let props: Vec<Vec<(&str, Value)>> = (0..n)
        .map(|i| {
            vec![
                ("nom", Value::String(format!("f{i}").into())),
                ("ligne", Value::Int64(i64::from(i))),
            ]
        })
        .collect();
    let ecrites = s.bulk_import_node_props(1, &props).unwrap();
    assert_eq!(ecrites, u64::from(n) * 2, "toutes les valeurs du lot");
    for i in 0..n {
        let id = NodeId(u64::from(i) + 1);
        assert_eq!(
            s.get_node_property(id, &PropertyKey::new("nom")),
            Some(Value::String(format!("f{i}").into())),
            "nœud {i}"
        );
        assert_eq!(
            s.get_node_property(id, &PropertyKey::new("ligne")),
            Some(Value::Int64(i64::from(i)))
        );
    }
}

/// L'AUTRE SILENCE DU MÊME CHEMIN, MESURÉ EN CORRIGEANT #386 — l'ombre blob.
///
/// Une valeur longue déjà écrite vit dans une colonne BLOB, que la lecture
/// consulte AVANT la zone v2. Le lot posait sa page et sa tête sans lever
/// l'ombre : la lecture rendait l'ancienne valeur, sans erreur (mesuré : 700
/// octets rendus à la place de la valeur courte du lot). Le chemin unitaire
/// efface l'ombre ; le lot doit le faire aussi.
#[test]
fn le_lot_leve_l_ombre_blob_de_la_valeur_longue_ecrite_avant_lui() {
    let d = tempfile::tempdir().unwrap();
    let s = SubstrateStore::create(d.path()).unwrap();
    let n = s.create_node(&["File"]);
    let longue = "x".repeat(700); // au-delà du seuil des colonnes blob (256)
    s.set_node_property(n, "corps", Value::String(longue.into()));
    s.bulk_import_node_props(
        n.0 as u32,
        &[vec![("corps", Value::String("court".into()))]],
    )
    .unwrap();
    assert_eq!(
        s.get_node_property(n, &PropertyKey::new("corps")),
        Some(Value::String("court".into())),
        "la valeur courte du lot est lisible (l'ombre blob est levée)"
    );
    s.checkpoint().unwrap();
    drop(s);
    let s = SubstrateStore::open(d.path()).unwrap();
    assert_eq!(
        s.get_node_property(n, &PropertyKey::new("corps")),
        Some(Value::String("court".into())),
        "à froid, l'ombre n'est pas ressuscitée par le rejeu"
    );
}
