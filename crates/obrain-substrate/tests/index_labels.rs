//! INDEX DE LABELS (obrain#120, point 5) — l'index doit rendre EXACTEMENT le
//! balayage, sur tous les chemins qui changent les labels d'un nœud.
//!
//! L'oracle est `SubstrateStore::nodes_by_label_balayage` : le balayage de la
//! zone d'avant l'index, même code que le chemin de repli. Chaque comparaison
//! porte sur la liste ENTIÈRE (mêmes slots, même ordre), et sur les préfixes
//! plafonnés (`nodes_by_label_capped`).
//!
//! Ce fichier couvre :
//!   1. une propriété (proptest) : séquences aléatoires de créations, ajouts et
//!      retraits de labels, suppressions, transactions annulées ou validées,
//!      fermetures propres et abandons sans flush (rejeu du WAL) ;
//!   2. les chemins qui INVALIDENT l'index : import en bloc, repermutation de
//!      Hilbert ;
//!   3. un lecteur `open_read_only` vivant pendant qu'un écrivain partagé (autre
//!      processus) ajoute des labels.
//!
//! Le kill -9 est dans `obrain-engine/tests/index_labels_kill9.rs` (commits
//! acquittés par le moteur).
//!
//!   cargo test -p obrain-substrate --test index_labels

use obrain_common::types::{EpochId, NodeId, TransactionId};
use obrain_core::graph::traits::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;
use obrain_substrate::wal_io::SyncMode;
use proptest::prelude::*;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const LABELS: [&str; 5] = ["A", "B", "C", "D", "E"];

/// L'index et le balayage rendent la même chose, pour chaque label (et un
/// label jamais interné), en entier et sous plafond.
fn exiger_egalite(s: &SubstrateStore, quoi: &str) {
    for l in LABELS.iter().copied().chain(["Inconnu"]) {
        let index = s.nodes_by_label(l);
        let balayage = s.nodes_by_label_balayage(l);
        assert_eq!(index, balayage, "{quoi} : label {l}");
        for k in [1usize, 3, 17] {
            let cap = s.nodes_by_label_capped(l, k);
            assert_eq!(
                cap,
                balayage[..k.min(balayage.len())],
                "{quoi} : label {l}, plafond {k}"
            );
        }
    }
}

#[derive(Debug, Clone)]
enum Op {
    Creer(u8),
    Ajouter(usize, u8),
    Retirer(usize, u8),
    Supprimer(usize),
    Interroger(u8),
    /// Transaction : ajouts, retraits, suppressions, créations versionnés,
    /// puis annulation (`true`) ou validation.
    Transaction(Vec<(u8, usize, u8)>, bool),
    /// Fermeture propre + réouverture.
    Fermer,
    /// Abandon SANS flush + réouverture : le rejeu du WAL reconstruit la zone.
    Abandonner,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (0u8..32).prop_map(Op::Creer),
        4 => (any::<usize>(), 0u8..5).prop_map(|(i, l)| Op::Ajouter(i, l)),
        3 => (any::<usize>(), 0u8..5).prop_map(|(i, l)| Op::Retirer(i, l)),
        2 => any::<usize>().prop_map(Op::Supprimer),
        3 => (0u8..5).prop_map(Op::Interroger),
        1 => (
            prop::collection::vec((0u8..4, any::<usize>(), 0u8..5), 1..8),
            any::<bool>()
        )
            .prop_map(|(v, a)| Op::Transaction(v, a)),
        1 => Just(Op::Fermer),
        1 => Just(Op::Abandonner),
    ]
}

fn labels_de(masque: u8) -> Vec<&'static str> {
    (0..5)
        .filter(|b| masque & (1 << b) != 0)
        .map(|b| LABELS[b])
        .collect()
}

fn ouvrir(base: &Path) -> SubstrateStore {
    SubstrateStore::open_with_mode(base, SyncMode::Never).expect("réouverture")
}

fn executer(ops: &[Op]) {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    let mut s = SubstrateStore::create_with_mode(&base, SyncMode::Never).expect("création");
    let mut ids: Vec<NodeId> = Vec::new();
    let mut prochaine_tx = 1u64;
    let choisir = |ids: &[NodeId], i: usize| -> Option<NodeId> {
        (!ids.is_empty()).then(|| ids[i % ids.len()])
    };
    for (n, o) in ops.iter().enumerate() {
        match o {
            Op::Creer(m) => ids.push(s.create_node(&labels_de(*m))),
            Op::Ajouter(i, l) => {
                if let Some(id) = choisir(&ids, *i) {
                    s.add_label(id, LABELS[*l as usize]);
                }
            }
            Op::Retirer(i, l) => {
                if let Some(id) = choisir(&ids, *i) {
                    s.remove_label(id, LABELS[*l as usize]);
                }
            }
            Op::Supprimer(i) => {
                if let Some(id) = choisir(&ids, *i) {
                    s.delete_node(id);
                }
            }
            Op::Interroger(l) => {
                let l = LABELS[*l as usize];
                assert_eq!(
                    s.nodes_by_label(l),
                    s.nodes_by_label_balayage(l),
                    "op {n} : interrogation de {l}"
                );
            }
            Op::Transaction(v, annuler) => {
                let tx = TransactionId(prochaine_tx);
                prochaine_tx += 1;
                for &(genre, i, l) in v {
                    let label = LABELS[l as usize];
                    match genre {
                        0 => {
                            if let Some(id) = choisir(&ids, i) {
                                s.add_label_versioned(id, label, tx);
                            }
                        }
                        1 => {
                            if let Some(id) = choisir(&ids, i) {
                                s.remove_label_versioned(id, label, tx);
                            }
                        }
                        2 => {
                            if let Some(id) = choisir(&ids, i) {
                                s.delete_node_versioned(id, EpochId(1), tx);
                            }
                        }
                        _ => ids.push(s.create_node_versioned(&[label], EpochId(1), tx)),
                    }
                }
                // L'index doit être juste AU MILIEU de la transaction aussi.
                exiger_egalite(&s, &format!("op {n} : transaction en vol"));
                if *annuler {
                    s.discard_uncommitted_versions(tx);
                } else {
                    s.finalize_version_epochs(tx, EpochId(2));
                }
            }
            Op::Fermer => {
                s.close().expect("close");
                drop(s);
                s = ouvrir(&base);
            }
            Op::Abandonner => {
                drop(s);
                s = ouvrir(&base);
            }
        }
    }
    exiger_egalite(&s, "fin de séquence");
    let d = s.diagnostic_index_labels();
    assert!(d.actif, "écrivain exclusif : l'index doit servir");
    // Anti-vacuité : dès qu'un porteur existe, c'est l'index qui a répondu.
    if LABELS
        .iter()
        .any(|l| !s.nodes_by_label_balayage(l).is_empty())
    {
        assert!(d.construit && d.requetes > 0, "anti-vacuité : {d:?}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 48,
        failure_persistence: None,
        .. ProptestConfig::default()
    })]

    #[test]
    fn index_egal_au_balayage_sur_sequences_aleatoires(ops in prop::collection::vec(op(), 1..160)) {
        executer(&ops);
    }
}

/// Séquence fixe qui passe par tous les chemins dans un ordre où l'index est
/// DÉJÀ construit au moment de chaque mutation (le cas où un crochet oublié se
/// verrait).
#[test]
fn chaque_chemin_apres_construction() {
    let s = SubstrateStore::open_tempfile().unwrap();
    let a: Vec<NodeId> = (0..50).map(|_| s.create_node(&["A"])).collect();
    exiger_egalite(&s, "créations");
    assert!(s.diagnostic_index_labels().construit);
    // add_label sur un ancien slot (hors ordre), remove_label, suppression.
    s.add_label(a[3], "B");
    s.add_label(a[40], "B");
    s.add_label(a[1], "B");
    exiger_egalite(&s, "add_label");
    s.remove_label(a[40], "B");
    s.remove_label(a[0], "A");
    exiger_egalite(&s, "remove_label");
    s.delete_node(a[1]);
    s.delete_node(a[2]);
    exiger_egalite(&s, "suppressions");
    // Résurrection par annulation d'une suppression.
    let tx = TransactionId(7);
    s.delete_node_versioned(a[5], EpochId(1), tx);
    s.add_label_versioned(a[6], "C", tx);
    s.remove_label_versioned(a[7], "A", tx);
    exiger_egalite(&s, "transaction en vol");
    s.discard_uncommitted_versions(tx);
    exiger_egalite(&s, "transaction annulée");
    assert_eq!(s.nodes_by_label("A").len(), 50 - 1 - 2);
    // Re-pose d'un label retiré (le slot est un faux positif encore listé).
    s.add_label(a[40], "B");
    s.add_label(a[0], "A");
    exiger_egalite(&s, "re-pose");
}

/// La compaction des faux positifs ne perd pas un porteur.
#[test]
fn compaction_des_perimes() {
    let s = SubstrateStore::open_tempfile().unwrap();
    let ids: Vec<NodeId> = (0..5000).map(|_| s.create_node(&["A", "B"])).collect();
    exiger_egalite(&s, "avant");
    for id in &ids[..4900] {
        s.remove_label(*id, "B");
    }
    exiger_egalite(&s, "après retraits (déclenche la compaction)");
    let d = s.diagnostic_index_labels();
    assert!(d.compactions >= 1, "{d:?}");
    // Re-pose après compaction : les slots doivent revenir.
    for id in &ids[..10] {
        s.add_label(*id, "B");
    }
    exiger_egalite(&s, "re-pose après compaction");
    assert_eq!(s.nodes_by_label("B").len(), 110);
}

/// Import en bloc, puis mutations, puis repermutation de Hilbert : deux
/// invalidations, deux reconstructions.
#[test]
fn import_en_bloc_et_repermutation() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    let s = SubstrateStore::create(&base).unwrap();
    // Index construit (vide) AVANT l'import : l'import doit l'invalider.
    assert!(s.nodes_by_label("A").is_empty());
    let labels: Vec<&str> = (0..3000).map(|i| LABELS[i % 3]).collect();
    let aretes: Vec<(u32, u32, &str)> = (1..3000u32).map(|i| (i, i + 1, "S")).collect();
    s.bulk_import_labels_edges(&labels, &aretes).unwrap();
    exiger_egalite(&s, "après import en bloc");
    s.add_label(NodeId(10), "D");
    s.remove_label(NodeId(11), "C");
    s.delete_node(NodeId(12));
    exiger_egalite(&s, "après mutations");
    let avant = s.diagnostic_index_labels().constructions;
    s.repermuter_par_hilbert(8, 64).unwrap();
    exiger_egalite(&s, "après repermutation de Hilbert");
    assert!(s.diagnostic_index_labels().constructions > avant);
}

// ───────────── 3. lecteur `open_read_only` + écrivain partagé vivant ─────────────

fn annoncer(ligne: &str) {
    println!("{ligne}");
    std::io::stdout().flush().ok();
}

fn attendre_fichier(p: &Path) {
    let debut = Instant::now();
    while !p.exists() {
        assert!(
            debut.elapsed() < Duration::from_secs(120),
            "attente de {}",
            p.display()
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

struct Enfant {
    process: Child,
    lignes: std::io::Lines<std::io::BufReader<std::process::ChildStdout>>,
}

impl Enfant {
    fn lancer(base: &Path, drapeaux: &Path) -> Self {
        let mut process = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "enfant_ecrivain_partage", "--nocapture"])
            .env("IL_BASE", base)
            .env("IL_DRAPEAUX", drapeaux)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("lancer l'enfant");
        let sortie = process.stdout.take().unwrap();
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
}

const TOURS_LECTEUR: usize = 4;

/// Écrivain PARTAGÉ (autre processus) : à chaque tour, pose `B` sur 25 nœuds
/// existants et en crée 10 portant `C`, annonce, attend le parent.
#[test]
fn enfant_ecrivain_partage() {
    let Ok(base) = std::env::var("IL_BASE") else {
        return;
    };
    let drapeaux = PathBuf::from(std::env::var("IL_DRAPEAUX").unwrap());
    let s = SubstrateStore::open_shared_write(&base).expect("écriture partagée");
    assert!(
        !s.diagnostic_index_labels().actif,
        "écriture partagée : index inactif"
    );
    let ids = s.nodes_by_label("A");
    for tour in 0..TOURS_LECTEUR {
        for id in ids.iter().skip(tour * 25).take(25) {
            s.add_label(*id, "B");
        }
        for _ in 0..10 {
            s.create_node(&["C"]);
        }
        s.fsync_wal().expect("fsync");
        annoncer(&format!(
            "TOUR{tour} {} {}",
            s.nodes_by_label("B").len(),
            s.nodes_by_label("C").len()
        ));
        attendre_fichier(&drapeaux.join(format!("suite{tour}")));
    }
    s.flush().expect("flush");
    s.close().expect("close");
    annoncer("FERME");
}

#[test]
fn lecteur_read_only_pendant_qu_un_ecrivain_ajoute_des_labels() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    let drapeaux = td.path().join("drapeaux");
    std::fs::create_dir(&drapeaux).unwrap();
    {
        let s = SubstrateStore::create(&base).unwrap();
        for _ in 0..200 {
            s.create_node(&["A"]);
        }
        s.flush().unwrap();
        s.close().unwrap();
    }
    let mut e = Enfant::lancer(&base, &drapeaux);
    // Lecteur ouvert pendant TOUTE la vie de l'écrivain : la zone change sous
    // lui (pages partagées du cache), son résultat doit rester celui de son
    // propre balayage.
    let vieux = SubstrateStore::open_read_only(&base).expect("lecteur");
    assert!(!vieux.diagnostic_index_labels().actif);
    for tour in 0..TOURS_LECTEUR {
        let vue = e.attendre_ligne(&format!("TOUR{tour} "));
        exiger_egalite(&vieux, &format!("lecteur ancien, tour {tour}"));
        // Lecteur neuf : rejoue le WAL de l'écrivain vivant, doit voir SA vue.
        let neuf = SubstrateStore::open_read_only(&base).expect("lecteur neuf");
        exiger_egalite(&neuf, &format!("lecteur neuf, tour {tour}"));
        assert_eq!(
            format!(
                "{} {}",
                neuf.nodes_by_label("B").len(),
                neuf.nodes_by_label("C").len()
            ),
            vue,
            "tour {tour} : le lecteur neuf voit ce que l'écrivain a écrit"
        );
        drop(neuf);
        std::fs::write(drapeaux.join(format!("suite{tour}")), b"").unwrap();
    }
    e.attendre_ligne("FERME");
    let statut = e.process.wait().unwrap();
    assert!(statut.success());
    drop(vieux);
    // Écrivain exclusif ensuite : l'index sert, et rend le balayage.
    let s = SubstrateStore::open(&base).unwrap();
    exiger_egalite(&s, "écrivain exclusif après l'écrivain partagé");
    assert!(s.diagnostic_index_labels().construit);
    assert_eq!(s.nodes_by_label("B").len(), 100);
    assert_eq!(s.nodes_by_label("C").len(), 40);
}
