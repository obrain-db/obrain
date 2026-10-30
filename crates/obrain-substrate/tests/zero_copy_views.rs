//! Tests du pont mémoire zero-copy (chantier O1) — validité des vues sous
//! garde, sémantique LWW/tombstone, chemins Zone/Blob, et goldens
//! little-endian qui figent la représentation multi-architectures.
//!
//! Le refus HORS garde est un doc-test `compile_fail` sur le module
//! `obrain_substrate::view` (le borrow checker est le test).

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::traits::{GraphStore, GraphStoreMut};
use obrain_substrate::blob_column::BlobSlotEntry;
use obrain_substrate::heap::{HeapPage, HEAP_PAGE_SIZE};
use obrain_substrate::store::SubstrateStore;
use obrain_substrate::view::StrViewSource;

// ---------------------------------------------------------------------------
// Vues sous garde — validité, cohérence avec le chemin possédé
// ---------------------------------------------------------------------------

#[test]
fn vue_zone_egale_valeur_possedee() {
    let s = SubstrateStore::open_tempfile().unwrap();
    let n = s.create_node(&["Fait"]);
    s.set_node_property(n, "texte", Value::String("le graphe est la mémoire".into()));

    let vue = s.node_prop_str_view(n, "texte").expect("vue présente");
    assert_eq!(&*vue, "le graphe est la mémoire");
    assert_eq!(
        vue.source(),
        StrViewSource::Zone,
        "une base neuve écrit en PropsZone v2 : la vue doit emprunter au heap mmap"
    );

    // La vue dit EXACTEMENT ce que dit le chemin possédé.
    let possedee = s.get_node_property(n, &PropertyKey::new("texte"));
    assert_eq!(
        possedee,
        Some(Value::String("le graphe est la mémoire".into()))
    );
}

#[test]
fn vue_blob_pour_chaine_de_plus_de_256_octets() {
    let s = SubstrateStore::open_tempfile().unwrap();
    let n = s.create_node(&["Fait"]);
    let long = "seeding-spéculatif ".repeat(20); // 400 o > seuil blob (256)
    assert!(long.len() > 256);
    s.set_node_property(n, "texte", Value::String(long.as_str().into()));

    let vue = s.node_prop_str_view(n, "texte").expect("vue présente");
    assert_eq!(&*vue, long.as_str());
    assert_eq!(
        vue.source(),
        StrViewSource::Blob,
        "au-delà de 256 o la valeur vit en colonne blob"
    );
    // Deux accès successifs re-dérivent le même slice (bornes + UTF-8 revalidés).
    assert_eq!(vue.as_str(), vue.as_str());
}

#[test]
fn lecture_possedee_pendant_vue_tenue() {
    // Les gardes de lecture sont PARTAGÉES : tenir une vue n'empêche pas
    // les autres lectures (cohabitation avec le lock RO partagé).
    let s = SubstrateStore::open_tempfile().unwrap();
    let a = s.create_node(&["Fait"]);
    let b = s.create_node(&["Fait"]);
    s.set_node_property(a, "texte", Value::String("premier".into()));
    s.set_node_property(b, "texte", Value::String("second".into()));

    let vue_a = s.node_prop_str_view(a, "texte").unwrap();
    let vue_b = s.node_prop_str_view(b, "texte").unwrap();
    let possedee_b = s.get_node_property(b, &PropertyKey::new("texte"));
    assert_eq!(&*vue_a, "premier");
    assert_eq!(&*vue_b, "second");
    assert_eq!(possedee_b, Some(Value::String("second".into())));
}

#[test]
fn tombstone_et_noeud_mort_rendent_none() {
    let s = SubstrateStore::open_tempfile().unwrap();
    let n = s.create_node(&["Fait"]);
    s.set_node_property(n, "texte", Value::String("éphémère".into()));
    assert!(s.node_prop_str_view(n, "texte").is_some());

    // Tombstone LWW : la propriété retirée est invisible à la vue.
    s.remove_node_property(n, "texte");
    assert!(
        s.node_prop_str_view(n, "texte").is_none(),
        "la tombstone doit masquer la valeur"
    );

    // Nœud mort : aucune vue.
    let m = s.create_node(&["Fait"]);
    s.set_node_property(m, "texte", Value::String("condamné".into()));
    assert!(s.delete_node(m));
    assert!(s.node_prop_str_view(m, "texte").is_none());
}

#[test]
fn valeur_non_string_et_cle_absente_rendent_none() {
    let s = SubstrateStore::open_tempfile().unwrap();
    let n = s.create_node(&["Fait"]);
    s.set_node_property(n, "poids", Value::Int64(42));
    assert!(
        s.node_prop_str_view(n, "poids").is_none(),
        "une vue str ne se pose pas sur un entier"
    );
    assert!(s.node_prop_str_view(n, "absente").is_none());
    assert!(s.node_prop_str_view(NodeId(999_999), "texte").is_none());
}

#[test]
fn derniere_ecriture_gagne_sous_la_vue() {
    // LWW : après réécriture, la vue voit la DERNIÈRE valeur.
    let s = SubstrateStore::open_tempfile().unwrap();
    let n = s.create_node(&["Fait"]);
    s.set_node_property(n, "texte", Value::String("v1".into()));
    s.set_node_property(n, "texte", Value::String("v2 — corrigée".into()));
    let vue = s.node_prop_str_view(n, "texte").unwrap();
    assert_eq!(&*vue, "v2 — corrigée");
}

// ---------------------------------------------------------------------------
// Goldens little-endian — la représentation est FIGÉE octet à octet.
// Un portage big-endian cassera ces tests, pas des données.
// ---------------------------------------------------------------------------

#[test]
fn golden_le_prefixe_longueur_du_heap() {
    // HeapPage::append écrit `(u32 len LE, octets)` ; read_at relit la
    // longueur via from_le_bytes. On fige la forme : 5 = 05 00 00 00.
    let mut p = HeapPage::new(0);
    let off = p.append(b"salut").expect("page neuve, ça tient");
    assert_eq!(p.read_at(off), Some(&b"salut"[..]));

    let bytes = bytemuck::bytes_of(&p);
    let golden: &[u8] = &[5, 0, 0, 0, b's', b'a', b'l', b'u', b't'];
    assert!(
        bytes.windows(golden.len()).any(|w| w == golden),
        "le préfixe de longueur doit être little-endian explicite"
    );
}

#[test]
fn golden_le_entree_index_blob() {
    // BlobSlotEntry = { arena_offset: u64 LE, len: u32 LE, pad } sur 16 o.
    // Relu par try_pod_read_unaligned : on fige les octets.
    let raw: [u8; 16] = [
        0x2A, 0, 0, 0, 0, 0, 0, 0, // arena_offset = 42
        0x05, 0, 0, 0, // len = 5
        0, 0, 0, 0, // pad
    ];
    let e: BlobSlotEntry = bytemuck::pod_read_unaligned(&raw);
    assert_eq!(e.arena_offset, 42);
    assert_eq!(e.len, 5);
}

#[test]
fn alignement_verifie_jamais_suppose() {
    // Le chemin de vue pose une réf `&HeapPage` par bytemuck::try_from_bytes,
    // qui VÉRIFIE l'alignement à l'exécution. Sur un buffer volontairement
    // désaligné : refus propre, ni UB ni panique.
    let buf = vec![0u8; HEAP_PAGE_SIZE + 8];
    let base = buf.as_ptr() as usize;
    let align = core::mem::align_of::<HeapPage>();
    let off = if align > 1 {
        (align - (base % align)) % align + 1 // adresse ≡ 1 (mod align)
    } else {
        1
    };
    let desaligne = &buf[off..off + HEAP_PAGE_SIZE];
    assert!(
        bytemuck::try_from_bytes::<HeapPage>(desaligne).is_err(),
        "un slice désaligné doit être refusé, pas transmuté"
    );
}
