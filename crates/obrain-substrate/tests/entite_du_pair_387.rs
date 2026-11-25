//! #387 — un nœud (ou une arête) créé par un PAIR après notre ouverture doit
//! être visible pour l'écriture de propriété, pas un no-op silencieux.
//!
//! ── LE DÉFAUT ─────────────────────────────────────────────────────────────
//! `next_node_id` / `next_edge_id` ne sont relus qu'à l'ouverture. Un nœud
//! créé par un PAIR (même store, `open_shared_write`) après notre ouverture a
//! donc un id ≥ notre miroir, et `is_live_on_disk` le déclare « absent » :
//! `set_node_property` devient un no-op silencieux, la propriété n'atterrit
//! jamais sur disque.
//!
//! Deux couches à ce défaut, comme MP-3b l'a montré pour la zone de
//! propriétés :
//!   1. le MIROIR est périmé (la borne partagée a avancé chez le pair) ;
//!   2. la PROJECTION mmap de la zone des nœuds est plus courte que le
//!      fichier réel (le pair a grandi la zone), donc `read_node(slot)`
//!      rend `None` même après une relecture de la borne seule.
//!
//! ── HARNAIS ───────────────────────────────────────────────────────────────
//! Le binaire de test se RELANCE lui-même en enfant
//! (`std::env::current_exe()` + `--exact <ce test> --nocapture`), le rôle est
//! choisi par `SW387_ROLE`, la synchronisation passe par des lignes sur stdout
//! et un chemin passé en variable d'environnement. Motif identique à
//! `mw_interprocess_phase_b.rs`, mais harnais PROPRE à ce fichier.

use std::io::{BufRead as _, Write as _};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::{GraphStore as _, GraphStoreMut as _};
use obrain_substrate::SubstrateStore;

// ---------------------------------------------------------------------------
// Harnais inter-processus
// ---------------------------------------------------------------------------

fn role() -> Option<String> {
    std::env::var("SW387_ROLE").ok()
}

fn env_obligatoire(cle: &str) -> String {
    std::env::var(cle).unwrap_or_else(|_| panic!("{cle} manquant dans l'enfant"))
}

fn annoncer(ligne: &str) {
    println!("{ligne}");
    std::io::stdout().flush().ok();
}

/// Délai maximal d'attente d'une ligne d'enfant : un verrou bloqué doit faire
/// ÉCHOUER le test, pas figer la CI.
const DELAI_LIGNE: Duration = Duration::from_secs(60);

struct Enfant {
    process: Child,
    lignes: std::sync::mpsc::Receiver<String>,
    _lecteur: std::thread::JoinHandle<()>,
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
        let (tx, rx) = std::sync::mpsc::channel();
        let lecteur = std::thread::spawn(move || {
            for ligne in std::io::BufReader::new(sortie)
                .lines()
                .map_while(Result::ok)
            {
                if tx.send(ligne).is_err() {
                    break;
                }
            }
        });
        Self {
            process,
            lignes: rx,
            _lecteur: lecteur,
        }
    }

    /// Lit stdout jusqu'à la ligne qui COMMENCE par `prefixe`, et rend le reste.
    fn attendre_ligne(&mut self, prefixe: &str) -> String {
        loop {
            match self.lignes.recv_timeout(DELAI_LIGNE) {
                Ok(l) => {
                    if let Some(reste) = l.strip_prefix(prefixe) {
                        return reste.trim().to_string();
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    panic!("enfant muet depuis {DELAI_LIGNE:?} en attendant « {prefixe} »")
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("enfant terminé sans « {prefixe} »")
                }
            }
        }
    }

    fn terminer(mut self) {
        let statut = self.process.wait().expect("attendre l'enfant");
        assert!(
            statut.success(),
            "l'enfant a échoué : {statut:?} (voir son stdout ci-dessus)"
        );
    }
}

// ---------------------------------------------------------------------------
// Enfant
// ---------------------------------------------------------------------------

/// Rôle « enfant » : ouvre le store en écriture partagée, crée UN nœud, flush,
/// annonce son id, sort.
fn role_enfant() {
    let root = PathBuf::from(env_obligatoire("SW387_DIR"));
    let s = SubstrateStore::open_shared_write(&root).expect("ouvrir le store (enfant)");
    let id = s.create_node(&["MW"]);
    s.flush().expect("flush (enfant)");
    annoncer(&format!("NOEUD {}", id.0));
    drop(s);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Reproduction LITTÉRALE du défaut : le parent crée puis OUVRE le store en
/// écriture partagée, un enfant crée un nœud, et le parent doit pouvoir écrire
/// une propriété dessus. La zone des nœuds n'est jamais touchée par le parent
/// avant l'écriture : la projection est soit absente, soit en retard.
#[test]
fn noeud_du_pair_accepte_une_propriete() {
    if role().as_deref() == Some("enfant") {
        role_enfant();
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("kb");

    // 1. Création + flush, puis fermeture (dimensionne le dictionnaire).
    {
        let s = SubstrateStore::create(&root).expect("créer le store");
        s.flush().expect("flush initial");
    }

    // 2. Le parent OUVRE en écriture partagée : son miroir est figé ici.
    let s = SubstrateStore::open_shared_write(&root).expect("ouvrir en partagé");

    // 3. Un enfant crée un nœud APRES notre ouverture.
    let mut enfant = Enfant::lancer(
        "noeud_du_pair_accepte_une_propriete",
        &[
            ("SW387_ROLE", "enfant".into()),
            ("SW387_DIR", root.to_string_lossy().into_owned()),
        ],
    );
    let ligne = enfant.attendre_ligne("NOEUD ");
    let id: u64 = ligne.parse().expect("id annoncé par l'enfant");
    enfant.terminer();

    // 4. Écriture d'une propriété sur ce nœud : était un no-op silencieux.
    s.set_node_property(NodeId(id), "couleur", Value::String("rouge".into()));
    s.flush().expect("flush (parent)");
    drop(s);

    // 5. Réouverture exclusive et relecture : la propriété DOIT être là.
    let s2 = SubstrateStore::open(&root).expect("réouverture exclusive");
    let lu = s2.get_node_property(NodeId(id), &PropertyKey::new("couleur"));
    assert_eq!(
        lu,
        Some(Value::String("rouge".into())),
        "la propriété écrite sur le nœud du pair doit être relue"
    );
}

/// Variante qui exerce la PROJECTION PÉRIMÉE (la couche 2 du défaut) : le
/// parent force l'ouverture de sa zone des nœuds quand elle ne contient que la
/// sentinelle (40 octets), puis l'enfant crée un nœud qui fait GRANDIR le
/// fichier au-delà de cette projection. `read_node` rendait `None` même après
/// une relecture de la borne seule ; la reprojection doit le rendre visible.
#[test]
fn noeud_du_pair_hors_projection_accepte_une_propriete() {
    if role().as_deref() == Some("enfant") {
        role_enfant();
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("kb");

    {
        let s = SubstrateStore::create(&root).expect("créer le store");
        s.flush().expect("flush initial");
    }

    let s = SubstrateStore::open_shared_write(&root).expect("ouvrir en partagé");

    // Force l'ouverture de la zone des nœuds à sa taille actuelle (la seule
    // sentinelle, 40 octets) : c'est cette projection que l'enfant va dépasser.
    let sentinelle = s.writer().read_node(0).expect("lire la sentinelle");
    assert!(sentinelle.is_some(), "la sentinelle de slot 0 doit exister");

    let mut enfant = Enfant::lancer(
        "noeud_du_pair_hors_projection_accepte_une_propriete",
        &[
            ("SW387_ROLE", "enfant".into()),
            ("SW387_DIR", root.to_string_lossy().into_owned()),
        ],
    );
    let ligne = enfant.attendre_ligne("NOEUD ");
    let id: u64 = ligne.parse().expect("id annoncé par l'enfant");
    enfant.terminer();

    s.set_node_property(NodeId(id), "couleur", Value::String("bleu".into()));
    s.flush().expect("flush (parent)");
    drop(s);

    let s2 = SubstrateStore::open(&root).expect("réouverture exclusive");
    let lu = s2.get_node_property(NodeId(id), &PropertyKey::new("couleur"));
    assert_eq!(lu, Some(Value::String("bleu".into())));
}
