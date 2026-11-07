//! ── MP-8 — LA FENÊTRE DE REJEU NE DOIT PAS SE REFERMER SUR UNE TRANSACTION
//! EN VOL D'UN AUTRE PROCESSUS ────────────────────────────────────────────────
//!
//! Défaut MESURÉ par MP-6 (`crates/obrain-engine/tests/ecriture_multi_processus.rs`,
//! 3/3 déterministe) : un processus A déclare des entités en attente
//! (`TxNoeudEnAttente` / `TxAreteEnAttente`) dans une transaction qu'il ne
//! scelle jamais ; un PAIR B flushe (ou checkpointe) APRÈS ces déclarations ;
//! A est tué `-9`. À la réouverture, les entités de A sont PUBLIÉES.
//!
//! Cause : `SubstrateStore::flush()` persiste `wal_offset_at_flush` = FIN du
//! WAL partagé, records des pairs compris. Le rejeu ne relit que depuis cet
//! offset ; les déclarations de A sont sorties de la fenêtre, et les entités —
//! déjà dans les zones, msyncées par ce même flush — ne sont jamais écartées.
//! Un checkpoint élu, lui, TRONQUE le WAL sans regarder les transactions
//! ouvertes : même perte, par une autre porte.
//!
//! Ce fichier reproduit le défaut au niveau du substrat, plus vite que le test
//! moteur : A (enfant) déclare en vol, B (le parent, pair `open_shared_write`)
//! flushe ou checkpointe, A est tué, réouverture EXCLUSIVE : les entités de A
//! doivent être ABSENTES.
//!
//! Harnais : le binaire de test se relance lui-même en enfant (`MWB_ROLE`),
//! même motif que `mw_interprocess_phase_b.rs`.

use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use obrain_common::types::{EpochId, TransactionId};
use obrain_core::graph::{GraphStore as _, GraphStoreMut as _};
use obrain_substrate::SubstrateStore;
use obrain_substrate::wal_io::WalReader;

/// Nœuds déclarés en vol par A.
const NB_NOEUDS: usize = 10;
/// Transaction de A, jamais scellée. Distincte de tout identifiant que le
/// parent pourrait émettre : les identifiants de transaction sont PAR
/// PROCESSUS (voir l'objection dans la PR), et une collision scellerait à
/// tort la transaction du mort.
const TX_EN_VOL: u64 = 7_777;

fn role() -> Option<String> {
    std::env::var("MWB_ROLE").ok()
}

fn env_obligatoire(cle: &str) -> String {
    std::env::var(cle).unwrap_or_else(|_| panic!("{cle} manquant dans l'enfant"))
}

fn annoncer(ligne: &str) {
    println!("{ligne}");
    std::io::stdout().flush().ok();
}

struct Enfant {
    process: Child,
    lignes: std::io::Lines<std::io::BufReader<std::process::ChildStdout>>,
}

impl Enfant {
    fn lancer(test: &str, vars: &[(&str, String)]) -> Self {
        let moi = std::env::current_exe().expect("current_exe");
        let mut cmd = Command::new(moi);
        cmd.args(["--exact", test, "--nocapture"])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        for (k, v) in vars {
            cmd.env(k, v);
        }
        let mut process = cmd.spawn().expect("lancer l'enfant");
        let sortie = process.stdout.take().expect("stdout de l'enfant");
        Self {
            process,
            lignes: std::io::BufReader::new(sortie).lines(),
        }
    }

    fn attendre_ligne(&mut self, prefixe: &str) -> String {
        loop {
            match self.lignes.next() {
                Some(Ok(l)) => {
                    if let Some(reste) = l.strip_prefix(prefixe) {
                        return reste.trim().to_string();
                    }
                }
                autre => panic!("enfant terminé sans « {prefixe} » ({autre:?})"),
            }
        }
    }

    /// `SIGKILL` : ni `Drop`, ni flush, ni `TxCommit`. La transaction de
    /// l'enfant reste à jamais non scellée.
    fn tuer(mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

fn ids_depuis(csv: &str) -> Vec<u64> {
    csv.split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().expect("id"))
        .collect()
}

/// Le store d'amorce : un nœud publié, un flush, fermeture propre.
fn creer_store(chemin: &Path) -> u64 {
    let s = SubstrateStore::create(chemin).expect("créer le store");
    let amorce = s.create_node(&["Amorce"]).0;
    s.flush().expect("flush initial");
    amorce
}

/// Enfant A : ouvre en écrivain PARTAGÉ, déclare `NB_NOEUDS` nœuds et
/// `NB_NOEUDS - 1` arêtes EN VOL (transaction `TX_EN_VOL`, jamais scellée),
/// annonce ses identifiants, puis dort jusqu'au `kill -9`.
fn enfant_en_vol(dir: &Path) -> ! {
    let s = SubstrateStore::open_shared_write(dir).expect("open_shared_write de A");
    let tx = TransactionId::new(TX_EN_VOL);
    let epoque = EpochId::new(1);
    let noeuds: Vec<_> = (0..NB_NOEUDS)
        .map(|_| s.create_node_versioned(&["EnVol"], epoque, tx))
        .collect();
    let aretes: Vec<_> = noeuds
        .windows(2)
        .map(|w| s.create_edge_versioned(w[0], w[1], "SUIT", epoque, tx))
        .collect();
    let n = noeuds
        .iter()
        .map(|n| n.0.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let a = aretes
        .iter()
        .map(|e| e.0.to_string())
        .collect::<Vec<_>>()
        .join(",");
    annoncer(&format!("NOEUDS {n}"));
    annoncer(&format!("ARETES {a}"));
    annoncer("PRET");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

/// Ce que fait le PAIR après les déclarations de A.
#[derive(Clone, Copy)]
enum ActeDuPair {
    Flush,
    Checkpoint,
}

/// Le scénario commun : A déclare en vol, le pair agit, A est tué -9,
/// réouverture exclusive, relevé.
fn scenario(test: &str, acte: ActeDuPair) {
    if let Some(r) = role() {
        let dir = PathBuf::from(env_obligatoire("MWB_DIR"));
        match r.as_str() {
            "en_vol" => enfant_en_vol(&dir),
            autre => panic!("rôle inconnu {autre}"),
        }
    }
    let td = tempfile::tempdir().unwrap();
    let dir = td.path().join("store-mp8");
    let amorce = creer_store(&dir);

    // Le PAIR est ouvert AVANT A — comme les survivants de MP-6, ouverts
    // ensemble — et ne rouvre jamais : ce qu'il persiste, il le persiste sans
    // avoir « revu » quoi que ce soit.
    let pair = SubstrateStore::open_shared_write(&dir).expect("open_shared_write du pair");

    let mut a = Enfant::lancer(
        test,
        &[
            ("MWB_ROLE", "en_vol".into()),
            ("MWB_DIR", dir.display().to_string()),
        ],
    );
    let noeuds_en_vol = ids_depuis(&a.attendre_ligne("NOEUDS "));
    let aretes_en_vol = ids_depuis(&a.attendre_ligne("ARETES "));
    a.attendre_ligne("PRET");
    assert_eq!(noeuds_en_vol.len(), NB_NOEUDS, "A a déclaré ses nœuds");
    assert_eq!(aretes_en_vol.len(), NB_NOEUDS - 1, "A a déclaré ses arêtes");

    // ── L'ACTE DU PAIR, POSTÉRIEUR AUX DÉCLARATIONS DE A ──────────────────
    let publie_par_le_pair = pair.create_node(&["Pair"]).0;
    let wal = dir.join("substrate.wal");
    let taille_avant = std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
    match acte {
        ActeDuPair::Flush => pair.flush().expect("flush du pair"),
        ActeDuPair::Checkpoint => {
            let issue = pair.checkpoint_elu().expect("checkpoint élu du pair");
            // Un checkpoint qui TRONQUE pendant qu'une transaction d'un pair
            // est en vol emporte ses déclarations : c'est précisément ce que
            // ce test interdit. Le pair doit avoir été élu (personne d'autre
            // ne checkpointe) et DIFFÉRÉ sa troncature.
            assert!(
                !matches!(
                    issue,
                    obrain_substrate::checkpoint::CheckpointOutcome::NonElu { .. }
                ),
                "le pair devait être élu : {issue:?}"
            );
            assert!(
                !issue.fait(),
                "le checkpoint du pair a TRONQUÉ le WAL alors que la transaction \
                 {TX_EN_VOL} de A était en vol : {issue:?}"
            );
            let taille_apres = std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
            assert!(
                taille_apres >= taille_avant,
                "le WAL a été tronqué ({taille_avant} → {taille_apres} octets) \
                 avec une transaction en vol"
            );
        }
    }
    let fenetre_persistee = pair.offset_wal_du_dernier_flush();

    // ── A MEURT SANS JAMAIS SCELLER ──────────────────────────────────────────
    a.tuer();
    // Le pair se ferme proprement (flush) : un second flush de pair, comme les
    // survivants de MP-6.
    drop(pair);

    // ── RÉOUVERTURE EXCLUSIVE : LES ENTITÉS DE A SONT ABSENTES ───────────────
    // (Le SYMPTÔME d'abord — c'est lui que MP-6 a mesuré ; la cause, la
    // fenêtre persistée, est vérifiée après.)
    let s = SubstrateStore::open(&dir).expect("réouverture exclusive");
    assert!(
        s.get_node(amorce.into()).is_some(),
        "le nœud d'amorce est là"
    );
    assert!(
        s.get_node(publie_par_le_pair.into()).is_some(),
        "le nœud publié par le pair est là"
    );
    let vivants: Vec<u64> = noeuds_en_vol
        .iter()
        .copied()
        .filter(|id| s.get_node((*id).into()).is_some())
        .collect();
    assert!(
        vivants.is_empty(),
        "{} nœud(s) de la transaction EN VOL de A (jamais scellée, processus \
         tué -9) sont PUBLIÉS après réouverture : {vivants:?}",
        vivants.len()
    );
    let aretes_vivantes: Vec<u64> = aretes_en_vol
        .iter()
        .copied()
        .filter(|id| s.get_edge((*id).into()).is_some())
        .collect();
    assert!(
        aretes_vivantes.is_empty(),
        "{} arête(s) de la transaction EN VOL de A sont PUBLIÉES après \
         réouverture : {aretes_vivantes:?}",
        aretes_vivantes.len()
    );
    let ids: Vec<u64> = s.node_ids().into_iter().map(|n| n.0).collect();
    assert!(
        !ids.iter().any(|id| noeuds_en_vol.contains(id)),
        "le balayage des nœuds rend encore des nœuds en vol de A : {ids:?}"
    );
    drop(s);

    // ── LA CAUSE : LA FENÊTRE PERSISTÉE CONTIENT ENCORE LES DÉCLARATIONS DE A ─
    let lecteur = WalReader::open(&wal).expect("relire le WAL");
    let premiere_declaration = lecteur
        .iter_from(0)
        .filter_map(Result::ok)
        .find(|(rec, _, _)| {
            matches!(
                rec.payload,
                obrain_substrate::wal::WalPayload::TxNoeudEnAttente { tx, .. } if tx == TX_EN_VOL
            )
        })
        .map(|(_, off, _)| off);
    match premiere_declaration {
        Some(off) => assert!(
            fenetre_persistee <= off,
            "`wal_offset_at_flush` ({fenetre_persistee}) a dépassé la première \
             déclaration en vol de A (offset {off}) : le rejeu ne la verra pas"
        ),
        None => panic!("les déclarations en vol de A ont disparu du WAL (tronqué ?)"),
    }
}

/// Le FLUSH d'un pair, postérieur aux déclarations de A, ne doit pas faire
/// sortir ces déclarations de la fenêtre de rejeu.
#[test]
fn le_flush_d_un_pair_ne_publie_pas_la_transaction_en_vol_du_mort() {
    scenario(
        "le_flush_d_un_pair_ne_publie_pas_la_transaction_en_vol_du_mort",
        ActeDuPair::Flush,
    );
}

/// Le CHECKPOINT ÉLU d'un pair ne doit pas tronquer un WAL qui porte une
/// déclaration en vol non scellée.
#[test]
fn le_checkpoint_d_un_pair_ne_tronque_pas_la_transaction_en_vol_du_mort() {
    scenario(
        "le_checkpoint_d_un_pair_ne_tronque_pas_la_transaction_en_vol_du_mort",
        ActeDuPair::Checkpoint,
    );
}
