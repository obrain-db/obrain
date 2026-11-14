//! obrain#164 (relecture de #169) — LES VALEURS D'AVANT D'UNE TRANSACTION EN VOL
//! (`TxProprieteAvant`) ET CE QUI LES CROISE.
//!
//! Niveau substrat, sans processus enfant : `drop` d'un store ne clôt AUCUNE
//! transaction en vol (le WAL garde les déclarations sans `TxCommit` ni
//! `TxAnnulee`, les valeurs sont en place), donc `drop` puis réouverture donne
//! le même état disque qu'un `kill -9` (la version avec vrai SIGKILL est dans
//! `obrain-engine/tests/atomicite_proprietes_tx_kill9.rs`).
//!
//! Points de la relecture :
//!   1. une écriture HORS transaction, acquittée après la déclaration d'une
//!      transaction en vol sur la même (entité, clé), ne doit pas être écrasée
//!      par la restauration à l'ouverture ;
//!   2. idem quand le scellement du commit n'a pas abouti (la transaction reste
//!      en vol dans le WAL) ;
//!   3. une transaction d'un écrivain en écriture PARTAGÉE ne déclare pas de
//!      valeur d'avant (personne ne la restaurerait, et elle bloquerait le
//!      checkpoint de tous les pairs) ;
//!   5. la borne MP-8 est EXACTE : le checkpoint est différé AU décalage de la
//!      première valeur d'avant en vol, et ne l'est plus après commit ou
//!      rollback.

use std::path::{Path, PathBuf};

use obrain_common::types::{NodeId, PropertyKey, TransactionId, Value};
use obrain_core::graph::{GraphStore as _, GraphStoreMut as _};
use obrain_substrate::SubstrateStore;
use obrain_substrate::checkpoint::CheckpointOutcome;
use obrain_substrate::wal::WalPayload;
use obrain_substrate::wal_io::WalReader;

const TX: u64 = 9;

/// Un store avec un nœud `:C` portant `k = 'v0'`, écrit hors transaction puis
/// flushé et fermé. Rend le chemin et l'identifiant du nœud.
fn amorce(td: &tempfile::TempDir) -> (PathBuf, NodeId) {
    let dir = td.path().join("store");
    let s = SubstrateStore::create(&dir).expect("créer");
    let n = s.create_node(&["C"]);
    s.set_node_property(n, "k", Value::from("v0"));
    s.flush().expect("flush");
    drop(s);
    (dir, n)
}

fn lire(s: &SubstrateStore, n: NodeId) -> Option<String> {
    match s.get_node_property(n, &PropertyKey::new("k")) {
        Some(Value::String(v)) => Some(v.as_str().to_string()),
        autre => autre.map(|v| format!("<{v:?}>")),
    }
}

/// Trames `TxProprieteAvant` du WAL : (décalage, tx).
fn declarations(dir: &Path) -> Vec<(u64, u64)> {
    WalReader::open(dir.join("substrate.wal"))
        .expect("relire le WAL")
        .iter_from(0)
        .filter_map(Result::ok)
        .filter_map(|(rec, off, _)| match rec.payload {
            WalPayload::TxProprieteAvant { tx, .. } => Some((off, tx)),
            _ => None,
        })
        .collect()
}

/// Point 1. (entité, clé) = ('v0') ; la transaction écrit 'tx' (valeur d'avant
/// 'v0' au WAL) ; une écriture hors transaction 'hors-tx' est acquittée ; état
/// disque d'un kill ; l'ouverture exclusive doit servir 'hors-tx'.
#[test]
fn une_ecriture_hors_transaction_apres_une_tx_en_vol_survit_a_la_restauration() {
    let td = tempfile::tempdir().unwrap();
    let (dir, n) = amorce(&td);
    let s = SubstrateStore::open(&dir).expect("ouvrir");
    s.set_node_property_versioned(n, "k", Value::from("tx"), TransactionId::new(TX));
    assert_eq!(lire(&s, n).as_deref(), Some("tx"));
    s.set_node_property(n, "k", Value::from("hors-tx"));
    drop(s);
    let s = SubstrateStore::open(&dir).expect("rouvrir");
    assert_eq!(
        lire(&s, n).as_deref(),
        Some("hors-tx"),
        "une écriture hors transaction ACQUITTÉE a été écrasée par la restauration d'une \
         transaction en vol"
    );
}

/// Point 1, variante retrait : `remove_node_property` hors transaction.
#[test]
fn un_retrait_hors_transaction_apres_une_tx_en_vol_survit_a_la_restauration() {
    let td = tempfile::tempdir().unwrap();
    let (dir, n) = amorce(&td);
    let s = SubstrateStore::open(&dir).expect("ouvrir");
    s.set_node_property_versioned(n, "k", Value::from("tx"), TransactionId::new(TX));
    s.remove_node_property(n, "k");
    drop(s);
    let s = SubstrateStore::open(&dir).expect("rouvrir");
    assert_eq!(
        lire(&s, n),
        None,
        "un retrait hors transaction ACQUITTÉ a été défait par la restauration"
    );
}

/// Pas de marqueur quand aucune transaction n'est en vol : une écriture hors
/// transaction ordinaire n'écrit RIEN de plus au WAL (coût nul hors cas rare).
#[test]
fn sans_transaction_en_vol_une_ecriture_hors_transaction_n_ajoute_aucune_trame() {
    let td = tempfile::tempdir().unwrap();
    let (dir, n) = amorce(&td);
    let s = SubstrateStore::open(&dir).expect("ouvrir");
    s.set_node_property(n, "k", Value::from("plain"));
    s.remove_node_property(n, "k");
    drop(s);
    assert_eq!(
        declarations(&dir),
        vec![],
        "trame de valeur d'avant inattendue"
    );
}

/// Point 2. Le commit a retiré la transaction de ses journaux en mémoire
/// (`commit_transaction_properties`) mais le scellement n'a pas abouti : pas de
/// `TxCommit` au WAL, la transaction reste EN VOL. Une écriture hors transaction
/// acquittée ensuite ne doit pas être défaite à la réouverture.
#[test]
fn un_scellement_echoue_ne_laisse_pas_la_restauration_ecraser_une_ecriture_posterieure() {
    let td = tempfile::tempdir().unwrap();
    let (dir, n) = amorce(&td);
    let s = SubstrateStore::open(&dir).expect("ouvrir");
    let tx = TransactionId::new(TX);
    s.set_node_property_versioned(n, "k", Value::from("tx"), tx);
    // Ce que fait `Session::commit_inner` AVANT `seal_commit(..)?`.
    s.commit_transaction_properties(tx);
    // (`seal_commit` rend ici une erreur : jamais appelé.)
    s.set_node_property(n, "k", Value::from("hors-tx"));
    drop(s);
    let s = SubstrateStore::open(&dir).expect("rouvrir");
    assert_eq!(
        lire(&s, n).as_deref(),
        Some("hors-tx"),
        "après un scellement échoué, la restauration a écrasé une écriture acquittée"
    );
}

/// Point 3. Un écrivain PARTAGÉ qui fait un `SET` transactionnel ne déclare pas
/// de valeur d'avant : le checkpoint (élu, d'un pair ou de lui-même) n'est pas
/// différé par une transaction que personne ne restaurera (seule une ouverture
/// exclusive restaure et clôt).
#[test]
fn une_tx_d_ecriture_partagee_ne_bloque_pas_le_checkpoint() {
    let td = tempfile::tempdir().unwrap();
    let (dir, n) = amorce(&td);
    let s = SubstrateStore::open_shared_write(&dir).expect("open_shared_write");
    s.set_node_property_versioned(n, "k", Value::from("tx"), TransactionId::new(TX));
    assert_eq!(
        declarations(&dir),
        vec![],
        "un écrivain partagé a déclaré une valeur d'avant"
    );
    let issue = s.checkpoint_elu().expect("checkpoint élu");
    assert!(
        issue.fait(),
        "le checkpoint est bloqué par une transaction d'écriture partagée : {issue:?}"
    );
}

/// Point 5. Borne MP-8 exacte, scellée par commit.
#[test]
fn la_borne_mp8_est_exactement_la_premiere_valeur_d_avant_puis_se_leve_au_commit() {
    let td = tempfile::tempdir().unwrap();
    let (dir, n) = amorce(&td);
    let s = SubstrateStore::open(&dir).expect("ouvrir");
    let tx = TransactionId::new(TX);
    s.set_node_property_versioned(n, "k", Value::from("tx"), tx);
    s.set_node_property_versioned(n, "k", Value::from("tx2"), tx);
    let decl = declarations(&dir);
    assert_eq!(decl.len(), 2, "deux valeurs d'avant attendues : {decl:?}");
    let premiere = decl[0].0;
    match s.checkpoint_elu().expect("checkpoint") {
        CheckpointOutcome::Differe { offset, tx: t } => {
            assert_eq!((offset, t), (premiere, TX), "borne MP-8 inexacte");
        }
        autre => panic!("checkpoint attendu DIFFÉRÉ, rendu {autre:?}"),
    }
    s.commit_transaction_properties(tx);
    s.seal_commit(tx).expect("sceller");
    let issue = s.checkpoint_elu().expect("checkpoint");
    assert!(
        issue.fait(),
        "checkpoint encore différé après commit : {issue:?}"
    );
}

/// Point 5. La borne se lève aussi au rollback (`TxAnnulee`).
#[test]
fn la_borne_mp8_se_leve_au_rollback() {
    let td = tempfile::tempdir().unwrap();
    let (dir, n) = amorce(&td);
    let s = SubstrateStore::open(&dir).expect("ouvrir");
    let tx = TransactionId::new(TX);
    s.set_node_property_versioned(n, "k", Value::from("tx"), tx);
    assert!(
        matches!(
            s.checkpoint_elu().expect("checkpoint"),
            CheckpointOutcome::Differe { .. }
        ),
        "checkpoint attendu différé"
    );
    s.discard_uncommitted_versions(tx);
    assert_eq!(
        lire(&s, n).as_deref(),
        Some("v0"),
        "rollback : valeur d'avant"
    );
    let issue = s.checkpoint_elu().expect("checkpoint");
    assert!(
        issue.fait(),
        "checkpoint encore différé après rollback : {issue:?}"
    );
}
