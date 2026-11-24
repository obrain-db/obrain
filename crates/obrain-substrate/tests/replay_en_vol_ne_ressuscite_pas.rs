//! obrain#350 — UN REJEU NE REND JAMAIS VIVANTE L'ENTITÉ D'UNE TRANSACTION
//! JAMAIS SCELLÉE, MÊME EN CHEMIN.
//!
//! Constat d'origine : après le `kill -9` d'un écrivain en transaction explicite,
//! `quatre_processus_ecrivent_sans_perte_et_un_kill_9_ne_laisse_que_l_acquitte`
//! (`obrain-engine`) voyait de temps en temps `enum_RoleD = 95` pour 90 : des
//! nœuds jamais validés, énumérés par un PAIR vivant qui venait de rouvrir.
//!
//! Cause : la transaction du mort reste « en vol » dans le WAL pour toujours (le
//! rejeu écarte ses entités en posant des tombstones DANS LES ZONES, sans rien
//! journaliser). Chaque ouverture rejoue donc toute la fenêtre qui la contient.
//! Or `NodeUpdateV3` / `EdgeUpdate` réécrivent les `flags` du record avec ceux du
//! WAL : le tombstone posé par le rejeu précédent est EFFACÉ, l'entité redevient
//! vivante dans les zones PARTAGÉES, et ne retrouve son tombstone qu'à la FIN du
//! rejeu. Pendant ce temps, un pair qui énumère la sert.
//!
//! Le test est DÉTERMINISTE : pas de processus, pas de course. Il prouve
//! l'invariant sur les états INTERMÉDIAIRES du rejeu, qu'un observateur lit après
//! chaque record appliqué.

use std::path::Path;

use obrain_common::types::{EpochId, NodeId, TransactionId};
use obrain_core::graph::GraphStoreMut as _;
use obrain_substrate::replay::replay_from_observe;
use obrain_substrate::{EdgeRecord, NodeRecord, SubstrateFile, SubstrateStore, Zone, replay_from};

const TX_MORTE: u64 = 77;

fn copier(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for e in std::fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let cible = dst.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copier(&e.path(), &cible);
        } else {
            std::fs::copy(e.path(), &cible).unwrap();
        }
    }
}

fn noeud_vivant(sub: &SubstrateFile, id: NodeId) -> bool {
    let z = sub.open_zone(Zone::Nodes).unwrap();
    let o = id.0 as usize * NodeRecord::SIZE;
    let r: &NodeRecord = bytemuck::from_bytes(&z.as_slice()[o..o + NodeRecord::SIZE]);
    r.is_live()
}

fn arete_vivante(sub: &SubstrateFile, id: u64) -> bool {
    let z = sub.open_zone(Zone::Edges).unwrap();
    let o = id as usize * EdgeRecord::SIZE;
    let r: &EdgeRecord = bytemuck::from_bytes(&z.as_slice()[o..o + EdgeRecord::SIZE]);
    !r.is_tombstoned()
}

#[test]
fn un_rejeu_ne_rend_jamais_vivante_l_entite_d_une_transaction_en_vol() {
    let td = tempfile::tempdir().unwrap();
    let dir = td.path().join("store");
    let image = td.path().join("image_apres_kill9");

    // Un nœud acquitté, puis une transaction explicite JAMAIS scellée : un nœud
    // seul, et un chemin (deux nœuds + une arête, dont le `NodeUpdateV3` de la
    // source réécrit les `flags`).
    let s = SubstrateStore::create(&dir).expect("créer");
    let acquitte = s.create_node(&["Acquitte"]);
    s.flush().expect("flush");
    let tx = TransactionId(TX_MORTE);
    let seul = s.create_node_versioned(&["Fantome"], EpochId::new(0), tx);
    let a = s.create_node_versioned(&["Fantome"], EpochId::new(0), tx);
    let b = s.create_node_versioned(&["Fantome"], EpochId::new(0), tx);
    let e = s.create_edge_versioned(a, b, "LIEN", EpochId::new(0), tx);
    // L'image disque d'un `kill -9` : zones (pages partagées) et WAL tels
    // quels, ni flush ni scellement.
    copier(&dir, &image);
    drop(s);

    let sub = SubstrateFile::open(&image).expect("ouvrir l'image");
    // Premier rejeu = la première ouverture après la mort : il écarte.
    let st = replay_from(&sub, 0).expect("premier rejeu");
    assert_eq!(
        st.transactions_en_vol, 1,
        "la transaction du mort est en vol"
    );
    for n in [seul, a, b] {
        assert!(
            !noeud_vivant(&sub, n),
            "{n:?} doit être écarté après le premier rejeu"
        );
    }
    assert!(
        !arete_vivante(&sub, e.0),
        "l'arête doit être écartée après le premier rejeu"
    );
    assert!(
        noeud_vivant(&sub, acquitte),
        "le nœud acquitté reste vivant"
    );

    // Second rejeu = l'ouverture d'un PAIR (la transaction est toujours « en
    // vol » dans le WAL : rien n'a été journalisé). À CHAQUE record appliqué,
    // les entités écartées doivent rester mortes.
    let mut resurrections: Vec<String> = Vec::new();
    let mut pas = 0u32;
    replay_from_observe(&sub, 0, &mut || {
        pas += 1;
        for (nom, n) in [("seul", seul), ("a", a), ("b", b)] {
            if noeud_vivant(&sub, n) {
                resurrections.push(format!("nœud {nom} vivant après le record #{pas}"));
            }
        }
        if arete_vivante(&sub, e.0) {
            resurrections.push(format!("arête vivante après le record #{pas}"));
        }
    })
    .expect("second rejeu");
    assert!(pas > 0, "l'observateur n'a jamais été appelé");
    assert!(
        resurrections.is_empty(),
        "le rejeu a rendu VIVANTE une entité d'une transaction jamais scellée ({} fois), \
         visible d'un pair pendant ce temps : {:?}",
        resurrections.len(),
        &resurrections[..resurrections.len().min(6)]
    );
    // État final inchangé.
    for n in [seul, a, b] {
        assert!(!noeud_vivant(&sub, n));
    }
    assert!(!arete_vivante(&sub, e.0));
    assert!(noeud_vivant(&sub, acquitte));
}
