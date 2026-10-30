//! Fabrique du store d'essai NATIF-J4 (scratch, non commite) : un store v3
//! ou DEUX tenants et du public cohabitent DANS LE MEME fichier.
//!
//!   slots 1..=3 : DocA  (tenant 1)   — patch disque, technique du test J3
//!   slots 4..=6 : DocB  (tenant 2)
//!   slots 7..=9 : DocPub (tenant 0, public — le defaut de creation)
//!   aretes : 7 -> 1 (public vers A) et 7 -> 4 (public vers B), type `lien`
//!            (la visibilite d'une traversee suit l'extremite lointaine — J3)
//!
//!   cargo run --release -p obrain-engine --example cree_store_melange_j4 -- <chemin>

use obrain_common::types::{NodeId, Value};
use obrain_engine::ObrainDB;
use obrain_substrate::record::NodeRecord;
use std::path::Path;

fn poser_tenant(dir: &Path, slot: u32, t: u32) {
    use std::io::{Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.join("substrate.nodes"))
        .unwrap();
    f.seek(SeekFrom::Start(slot as u64 * NodeRecord::SIZE as u64 + 32))
        .unwrap();
    f.write_all(&t.to_le_bytes()).unwrap();
    f.sync_all().unwrap();
}

fn main() {
    let chemin = std::env::args()
        .nth(1)
        .expect("usage: cree_store_melange_j4 <chemin>");
    let dir = Path::new(&chemin);
    assert!(
        !dir.exists(),
        "{chemin} existe deja — ce fabricant ne cree que du neuf"
    );

    {
        let d = ObrainDB::open_substrate(dir).expect("open_substrate");
        let s = d.session();
        for i in 0..3i64 {
            s.create_node_with_props(
                &["DocA"],
                [("camp", Value::from("A")), ("i", Value::Int64(i))],
            );
        }
        for i in 0..3i64 {
            s.create_node_with_props(
                &["DocB"],
                [("camp", Value::from("B")), ("i", Value::Int64(i))],
            );
        }
        for i in 0..3i64 {
            s.create_node_with_props(
                &["DocPub"],
                [("camp", Value::from("public")), ("i", Value::Int64(i))],
            );
        }
        // 7 est un DocPub ; 1 est un DocA ; 4 est un DocB.
        let _ = s.create_edge(NodeId::new(7), NodeId::new(1), "CONTAINS");
        let _ = s.create_edge(NodeId::new(7), NodeId::new(4), "CONTAINS");
        d.checkpoint().expect("checkpoint");
    }

    for slot in 1..=3 {
        poser_tenant(dir, slot, 1);
    }
    for slot in 4..=6 {
        poser_tenant(dir, slot, 2);
    }

    // Verification immediate : trois vues, trois comptes.
    for (tenant, attendu) in [(1u32, 6i64), (2, 6), (0, 3)] {
        let d = ObrainDB::open_read_only(dir).expect("reopen ro");
        d.avec_principal(tenant).expect("principal");
        let r = d
            .session()
            .execute("MATCH (n) RETURN count(n)")
            .expect("count");
        assert_eq!(
            r.rows[0][0],
            Value::Int64(attendu),
            "vue tenant {tenant} : compte inattendu"
        );
        println!("vue tenant {tenant} : count(n) = {attendu} — conforme");
    }
    println!(
        "store melange cree a {chemin} (v3, 3 DocA t1 + 3 DocB t2 + 3 DocPub publics, 2 aretes)"
    );
}
