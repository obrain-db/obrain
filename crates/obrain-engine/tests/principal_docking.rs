//! POINT D'ENTREE DU JALON 4 — la preuve que le branchement existe.
//!
//! Le jalon 3 ne plombe PAS le Principal a travers Session -> plan ->
//! execution (c'est le jalon 4). Mais il expose `ObrainDB::avec_principal`,
//! et ce test prouve la propriete qui rend le jalon 4 possible : une fois
//! le handle confine, le GQL qui le traverse est DEJA filtre — parce que le
//! filtre vit dans les primitives du substrat, pas dans le moteur. Le
//! jalon 4 n'aura qu'a poser le principal par session au lieu de par
//! handle.
//!
//!   cargo test -p obrain-engine --test principal_docking

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use obrain_substrate::record::NodeRecord;
use std::path::Path;

const TENANT_A: u32 = 7;
const TENANT_B: u32 = 9;

/// Patch DISQUE du tenant (octets 32..36 du record, stride 40) — la meme
/// sonde que les bancs substrat : aucun writer ne sait poser un tenant
/// autre que celui du record, et c'est voulu jusqu'au jalon 4.
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

#[test]
fn un_obraindb_confine_filtre_le_gql_de_bout_en_bout() {
    let td = tempfile::tempdir().unwrap();

    // Peuplement, sans principal : 6 Doc (slots 1..=6), i = 0..=5.
    {
        let d = ObrainDB::open_substrate(td.path()).expect("open_substrate");
        let s = d.session();
        for i in 0..6i64 {
            s.create_node_with_props(&["Doc"], [("i", Value::Int64(i))]);
        }
        d.checkpoint().expect("checkpoint");
    }
    // Slots 1..=3 -> tenant B ; 4..=6 -> tenant A.
    for slot in 1..=3 {
        poser_tenant(td.path(), slot, TENANT_B);
    }
    for slot in 4..=6 {
        poser_tenant(td.path(), slot, TENANT_A);
    }

    // Handle confine A : le GQL ne voit que A (aucune plomberie moteur —
    // c'est le substrat qui filtre).
    let d = ObrainDB::open_substrate(td.path()).expect("reopen");
    d.avec_principal(TENANT_A).expect("pose du principal");

    let compte = d
        .session()
        .execute("MATCH (n:Doc) RETURN count(n)")
        .expect("count")
        .rows;
    assert_eq!(compte[0][0], Value::Int64(3), "count(n) : les 3 de A");

    let mut vus: Vec<i64> = d
        .session()
        .execute("MATCH (n:Doc) RETURN n.i")
        .expect("match")
        .rows
        .into_iter()
        .map(|l| match &l[0] {
            Value::Int64(v) => *v,
            autre => panic!("valeur inattendue : {autre:?}"),
        })
        .collect();
    vus.sort_unstable();
    assert_eq!(vus, vec![3, 4, 5], "seuls les i de A sortent");

    // LIMIT sous filtre : les 3 premiers slots sont a B — un plafond
    // naif rendrait moins de 2 lignes. Il en rend 2.
    let lim = d
        .session()
        .execute("MATCH (n:Doc) RETURN n.i LIMIT 2")
        .expect("limit")
        .rows;
    assert_eq!(lim.len(), 2, "LIMIT 2 rend 2 lignes sous filtre");

    // Et l'identite du handle est immuable.
    assert!(d.avec_principal(TENANT_B).is_err(), "re-pose refusee");
}
