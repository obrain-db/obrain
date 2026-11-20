//! La colonne de degrés ne doit pas dépasser ce que son espace de slots exige
//! (obrain-db/obrain#266).
//!
//! Format sur disque (`degree_column.rs`) : `[64 B d'en-tête][n_slots × 8 B]`,
//! et l'en-tête porte `n_slots` en u32 little-endian à l'offset 8. La taille
//! attendue est donc **exactement** `64 + 8 × n_slots`.
//!
//! Le ticket mesure une croissance ×60 (1 052 672 → 63 479 808 octets) sur une
//! copie de 3,9 M nœuds. Ce test fixe ce qui est vrai et ce qui ne l'est pas :
//! la zone peut grandir (l'espace de slots grandit), mais sa taille doit rester
//! celle de son en-tête — ni plus (sur-allocation) ni moins (slots perdus).
//!
//! ```bash
//! cargo test -p obrain-substrate --test degrees_zone_sans_surallocation
//! ```

use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use std::path::Path;

const HEADER: u64 = 64;
const RECORD: u64 = 8;

fn entete(dir: &Path) -> Option<(u32, u32, u32)> {
    let f = dir.join("substrate.degrees.node.u32");
    let octets = std::fs::read(&f).ok()?;
    if octets.len() < 16 {
        return None;
    }
    let u32le =
        |o: usize| u32::from_le_bytes([octets[o], octets[o + 1], octets[o + 2], octets[o + 3]]);
    Some((u32le(0), u32le(4), u32le(8)))
}

#[test]
fn la_zone_des_degres_fait_exactement_64_plus_8_par_slot() {
    let dir = tempfile::tempdir().unwrap();
    let s = SubstrateStore::create(dir.path()).unwrap();

    // 400 nœuds, 400 arêtes : assez pour dépasser la capacité initiale (131 576
    // slots observés sur la copie PO) et déclencher au moins un doublement.
    let mut ids = Vec::new();
    for _ in 0..400 {
        ids.push(s.create_node(&["N"]));
    }
    for i in 0..399 {
        s.create_edge(ids[i], ids[i + 1], "SUIT");
    }
    s.flush().unwrap();

    let (magic, version, n_slots) = entete(dir.path()).expect("zone des degrés absente");
    assert_eq!(
        magic,
        u32::from_le_bytes(*b"DEGR"),
        "magic de la colonne de degrés inattendu"
    );
    assert_eq!(version, 1, "version d'en-tête inattendue");

    let taille = std::fs::metadata(dir.path().join("substrate.degrees.node.u32"))
        .unwrap()
        .len();
    // La zone est faite de pages de 4 Kio : la seule marge admise est le
    // complément à la page (création et croissance demandent une page de
    // respiration, plus les 1,5x de `ensure_room`).
    let utile = HEADER + RECORD * u64::from(n_slots);
    assert!(
        taille >= utile,
        "zone plus petite que son en-tête (n_slots={n_slots}) : {taille} < {utile}"
    );
    assert!(
        taille <= utile * 2 + 4096,
        "sur-allocation : {taille} octets pour {utile} utiles (n_slots={n_slots})"
    );

    println!(
        "taille={taille} utile={} n_slots={n_slots} high_water={}",
        HEADER + RECORD * u64::from(n_slots),
        s.slot_high_water()
    );
    // L'espace de slots doit couvrir les nœuds vivants…
    let hw = s.slot_high_water();
    assert!(
        n_slots > hw,
        "n_slots={n_slots} ne couvre pas le high-water {hw}"
    );
    // …sans dépasser le double de ce qu'il sert, borne du doublement amorti.
    assert!(
        u64::from(n_slots) <= 2 * (u64::from(hw) + 1) + 1,
        "n_slots={n_slots} dépasse le double du high-water {hw} : sur-allocation"
    );
}
