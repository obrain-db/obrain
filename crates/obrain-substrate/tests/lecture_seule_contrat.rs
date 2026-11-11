//! ── LE CONTRAT DE LA LECTURE SEULE ───────────────────────────────────────────
//!
//! Ouvrir une base en lecture seule ne modifie AUCUN octet d'AUCUN fichier de
//! la base, n'en crée aucun, n'en supprime aucun — quel que soit l'état où un
//! écrivain l'a laissée, et qu'un écrivain partagé soit vivant ou non. Toute
//! tentative de mutation par cette session est refusée par une erreur nommée.
//! Ouvrir en lecture seule un dossier qui n'est pas une base échoue sans rien
//! créer.
//!
//! ── CE QUE CES TESTS REPRODUISENT ───────────────────────────────────────────
//! Avant le correctif, `SubstrateFile::open_read_only` ouvrait exactement comme
//! `open_shared_write` (fichiers en lecture-écriture, `MAP_SHARED`), et le
//! drapeau `read_only` du store n'était posé qu'APRÈS l'ouverture. Chaque test
//! fabrique l'état qui déclenchait une écriture, prend l'EMPREINTE de tous les
//! fichiers (liste, taille, date, inode, contenu), ouvre en lecture seule, lit,
//! ferme, et exige une empreinte identique :
//!
//! | chemin d'écriture                                   | test                                                  |
//! |-----------------------------------------------------|-------------------------------------------------------|
//! | rejeu du WAL dans les zones (+ msync/fsync)          | `ecrivain_tue_avant_flush_…`                          |
//! | degrés au CRC périmé remis à zéro et reconstruits    | `degres_au_crc_perime_…`, `degres_absents_…`          |
//! | graphe nommé ouvert en écriture exclusive            | `graphe_nomme_…`                                      |
//! | `flush()` complet (dict v5 → v6)                     | `dictionnaire_sans_histogrammes_…`                    |
//! | `set_node_property` d'un sidecar hérité              | `sidecar_herite_avec_vecteur_…`                       |
//! | suppression des `*.remap.tmp`                        | `temporaires_de_remap_…`                              |
//! | création des fichiers absents                        | `fichiers_absents_…`, `sans_fichier_de_verrou_…`      |
//! | republication de `substrate.wal.lsn`                 | `ancre_du_wal_absente_…`                              |
//! | `substrate.alloc` réécrit à chaque ouverture         | `base_propre_…` (et tous les autres)                  |
//! | écrivain partagé VIVANT                              | `ecrivain_partage_vivant_…` (inter-processus)         |
//! | mutations acceptées par un store en lecture seule    | `les_mutations_sont_refusees_…`                       |
//! | dossier qui n'est pas une base                       | `un_dossier_qui_n_est_pas_une_base_…`                 |
//! | garantie par le SYSTÈME (fichiers non inscriptibles) | `base_non_inscriptible_…`                             |
//!
//! ── HARNAIS ─────────────────────────────────────────────────────────────────
//! Les états « écrivain tué » et « écrivain vivant » demandent un AUTRE
//! processus : le binaire de test se relance lui-même (`current_exe` +
//! `--exact enfant`), le rôle est choisi par `LS_ROLE`. Même motif que
//! `mw_interprocess_phase_b.rs`.

use std::collections::BTreeMap;
use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::{GraphStore as _, GraphStoreMut as _};
use obrain_substrate::SubstrateStore;

// ---------------------------------------------------------------------------
// Empreinte : TOUT ce qu'une écriture peut changer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct Trace {
    repertoire: bool,
    taille: u64,
    modifie: SystemTime,
    inode: u64,
    condense: u64,
}

type Empreinte = BTreeMap<PathBuf, Trace>;

fn condense(chemin: &Path) -> u64 {
    let octets = std::fs::read(chemin).expect("lire pour l'empreinte");
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in octets {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

#[cfg(unix)]
fn inode(m: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    m.ino()
}

#[cfg(not(unix))]
fn inode(_m: &std::fs::Metadata) -> u64 {
    0
}

/// Empreinte récursive de `racine`. La date des RÉPERTOIRES en fait partie :
/// un fichier créé puis supprimé (verrou, temporaire) ne laisse que cette trace.
fn empreinte(racine: &Path) -> Empreinte {
    fn dans(racine: &Path, dossier: &Path, e: &mut Empreinte) {
        let m = std::fs::metadata(dossier).expect("metadata du dossier");
        e.insert(
            dossier.strip_prefix(racine).unwrap().to_path_buf(),
            Trace {
                repertoire: true,
                taille: 0,
                modifie: m.modified().unwrap(),
                inode: inode(&m),
                condense: 0,
            },
        );
        for entree in std::fs::read_dir(dossier).expect("read_dir") {
            let chemin = entree.unwrap().path();
            let m = std::fs::metadata(&chemin).expect("metadata");
            if m.is_dir() {
                dans(racine, &chemin, e);
            } else {
                e.insert(
                    chemin.strip_prefix(racine).unwrap().to_path_buf(),
                    Trace {
                        repertoire: false,
                        taille: m.len(),
                        modifie: m.modified().unwrap(),
                        inode: inode(&m),
                        condense: condense(&chemin),
                    },
                );
            }
        }
    }
    let mut e = Empreinte::new();
    dans(racine, racine, &mut e);
    e
}

/// Dit ce qui diffère, chemin par chemin — c'est la sortie du test rouge.
fn ecarts(avant: &Empreinte, apres: &Empreinte) -> Vec<String> {
    let mut v = Vec::new();
    for (chemin, a) in avant {
        match apres.get(chemin) {
            None => v.push(format!("SUPPRIMÉ   {}", chemin.display())),
            Some(b) if a != b => {
                let mut quoi = Vec::new();
                if a.taille != b.taille {
                    quoi.push(format!("taille {} → {}", a.taille, b.taille));
                }
                if a.condense != b.condense {
                    quoi.push("contenu".to_string());
                }
                if a.inode != b.inode {
                    quoi.push("inode (remplacé)".to_string());
                }
                if a.modifie != b.modifie {
                    quoi.push("date".to_string());
                }
                v.push(format!(
                    "MODIFIÉ    {} ({})",
                    chemin.display(),
                    quoi.join(", ")
                ));
            }
            Some(_) => {}
        }
    }
    for chemin in apres.keys() {
        if !avant.contains_key(chemin) {
            v.push(format!("CRÉÉ       {}", chemin.display()));
        }
    }
    v
}

#[track_caller]
fn exiger_identique(avant: &Empreinte, racine: &Path, quoi: &str) {
    let e = ecarts(avant, &empreinte(racine));
    assert!(
        e.is_empty(),
        "{quoi} : la lecture seule a modifié la base ({} écart(s)) :\n  {}",
        e.len(),
        e.join("\n  ")
    );
}

/// Laisse l'horloge du système de fichiers avancer : une réécriture à
/// l'identique doit changer la DATE, et elle ne le fait que si le temps a passé.
fn laisser_passer_l_horloge() {
    std::thread::sleep(Duration::from_millis(30));
}

// ---------------------------------------------------------------------------
// Fabrique et lectures
// ---------------------------------------------------------------------------

/// Écrit `n` personnes (numérotées depuis `debut`) : propriétés, une arête
/// `CONNAIT` vers la précédente, une arête `SUIT` vers la première une fois
/// sur trois.
fn peupler(s: &SubstrateStore, debut: usize, n: usize, avec_proprietes: bool) -> Vec<NodeId> {
    let mut ids = Vec::with_capacity(n);
    let premier = s.node_ids().first().copied();
    for i in debut..debut + n {
        let id = s.create_node(&["Personne"]);
        if avec_proprietes {
            s.set_node_property(id, "nom", Value::String(format!("p{i}").into()));
            s.set_node_property(id, "age", Value::Int64(i as i64));
        }
        if let Some(&prec) = ids.last() {
            s.create_edge(id, prec, "CONNAIT");
        }
        if i % 3 == 0
            && let Some(p) = premier.or(ids.first().copied())
        {
            s.create_edge(id, p, "SUIT");
        }
        ids.push(id);
    }
    ids
}

/// Ce qu'un lecteur voit. Deux vues égales = même graphe observable.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Vue {
    noeuds: usize,
    aretes: usize,
    personnes: usize,
    somme_sortants: usize,
    somme_entrants: usize,
    somme_sortants_connait: u64,
    noms: Vec<String>,
}

/// Lit LARGEMENT — y compris par les chemins paresseux qui, avant le
/// correctif, créaient des fichiers au passage (`open_named_zone` est un
/// open-or-create : zones de synapses, de sauts, de simplexes, d'engrammes).
fn lire(s: &SubstrateStore) -> Vue {
    let ids = s.node_ids();
    let mut somme_sortants = 0;
    let mut somme_entrants = 0;
    let mut somme_sortants_connait = 0u64;
    let mut noms = Vec::new();
    for &id in &ids {
        somme_sortants += s.out_degree(id) as usize;
        somme_entrants += s.in_degree(id) as usize;
        somme_sortants_connait += u64::from(s.out_degree_by_type(id, "CONNAIT"));
        if let Some(Value::String(nom)) = s.get_node_property(id, &PropertyKey::new("nom")) {
            noms.push(nom.to_string());
        }
        let _ = s.get_node(id);
        let _ = s.synapse_edges_of(id);
        let _ = s.engram_bitset(id);
        let _ = s.simplices_of_node(id);
    }
    noms.sort();
    // Recherche indexée, texte, fiabilité des degrés typés, sauts, engrammes,
    // simplexes, tiers : autant de chemins qui ouvrent des zones à la demande.
    let _ = s.find_nodes_by_property("nom", &Value::String("p1".into()));
    let _ = s.find_nodes_by_property_contains("nom", "p");
    let _ = s.typed_degree_reliable("CONNAIT");
    let _ = s.typed_degree_reliable("SUIT");
    let _ = s.jump_count("CONNAIT", "SUIT");
    let _ = s.jump_pairs("CONNAIT", "SUIT", 1);
    let _ = s.engram_members(1);
    let _ = s.iter_simplices_kind(2);
    let _ = s.passes_simpliciales_config();
    let _ = s.load_tier_index(8);
    let _ = s.community_slot_range(1);
    let _ = s.edge_target_labels("CONNAIT");
    let _ = s.live_count_by_edge_type("CONNAIT");
    for nom in s.named_graph_names() {
        if let Some(g) = s.named_graph(&nom) {
            let _ = g.node_count();
            let _ = g.node_ids();
        }
    }
    Vue {
        noeuds: s.node_count(),
        aretes: s.edge_count(),
        personnes: s.nodes_by_label("Personne").len(),
        somme_sortants,
        somme_entrants,
        somme_sortants_connait,
        noms,
    }
}

/// Ouvre en lecture seule, lit, ferme ; rend la vue.
fn ouvrir_lire_fermer(base: &Path) -> Vue {
    let s = SubstrateStore::open_read_only(base).expect("ouverture en lecture seule");
    let vue = lire(&s);
    s.close().expect("fermeture d'un store en lecture seule");
    drop(s);
    vue
}

/// Base fermée proprement : `n` personnes, flush, close.
fn base_propre(base: &Path, n: usize) -> Vue {
    let s = SubstrateStore::create(base).expect("création");
    peupler(&s, 0, n, true);
    s.flush().expect("flush");
    let vue = lire(&s);
    s.close().expect("close");
    drop(s);
    vue
}

// ---------------------------------------------------------------------------
// Harnais inter-processus
// ---------------------------------------------------------------------------

fn annoncer(ligne: &str) {
    println!("{ligne}");
    std::io::stdout().flush().ok();
}

fn attendre_fichier(p: &Path) {
    let debut = Instant::now();
    while !p.exists() {
        assert!(
            debut.elapsed() < Duration::from_secs(120),
            "attente de {} : délai dépassé",
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
    fn lancer(role: &str, base: &Path, vars: &[(&str, String)]) -> Self {
        let moi = std::env::current_exe().expect("current_exe");
        let mut cmd = Command::new(moi);
        cmd.args(["--exact", "enfant", "--nocapture"])
            .env("LS_ROLE", role)
            .env("LS_BASE", base)
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

    /// Lit stdout jusqu'à la ligne qui commence par `prefixe` ; rend le reste.
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

    /// `kill -9` : aucun destructeur, aucun flush.
    fn tuer(mut self) {
        self.process.kill().expect("kill -9");
        self.process.wait().expect("attendre l'enfant tué");
    }

    fn terminer(mut self) {
        let statut = self.process.wait().expect("attendre l'enfant");
        assert!(statut.success(), "l'enfant a échoué : {statut:?}");
    }
}

/// Dort jusqu'à être tué.
fn attendre_la_mort() -> ! {
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

fn vue_en_ligne(v: &Vue) -> String {
    format!(
        "{} {} {} {} {} {}",
        v.noeuds,
        v.aretes,
        v.personnes,
        v.somme_sortants,
        v.somme_entrants,
        v.somme_sortants_connait
    )
}

/// Point d'entrée des ENFANTS. Sans `LS_ROLE` (exécution ordinaire de la
/// suite), ne fait rien et passe.
#[test]
fn enfant() {
    let Ok(role) = std::env::var("LS_ROLE") else {
        return;
    };
    let base = PathBuf::from(std::env::var("LS_BASE").expect("LS_BASE"));
    match role.as_str() {
        // Écrivain exclusif : flush, puis écritures NON flushées, puis il
        // attend d'être tué. Le WAL porte la fenêtre ; les zones et les degrés
        // ne sont pas consolidés ; le dictionnaire est en retard.
        "tue_avant_flush" => {
            let s = SubstrateStore::create(&base).expect("création");
            peupler(&s, 0, 40, true);
            s.flush().expect("flush");
            peupler(&s, 40, 25, true);
            annoncer(&format!("PRET {}", vue_en_ligne(&lire(&s))));
            attendre_la_mort();
        }
        // Idem, avec un graphe nommé qui porte lui aussi une fenêtre WAL.
        "graphe_nomme" => {
            let s = SubstrateStore::create(&base).expect("création");
            peupler(&s, 0, 20, true);
            assert!(s.create_named_graph("archives").expect("graphe nommé"));
            let g = s.named_graph("archives").expect("graphe créé");
            for _ in 0..7 {
                g.create_node(&["Archive"]);
            }
            s.flush().expect("flush");
            // Fenêtre WAL non vide dans l'enfant ET dans le parent.
            for _ in 0..5 {
                g.create_node(&["Archive"]);
            }
            peupler(&s, 20, 6, true);
            annoncer(&format!("PRET {}", g.node_count()));
            attendre_la_mort();
        }
        // Base du régime hérité (propriétés dans le sidecar `substrate.props`).
        "heritee" => {
            let s = SubstrateStore::create(&base).expect("création");
            let a = s.create_node(&["Doc"]);
            s.set_node_property(a, "titre", Value::String("ancien".into()));
            s.flush().expect("flush");
            s.close().expect("close");
            annoncer(&format!("PRET {}", a.0));
        }
        // Écrivain PARTAGÉ vivant : il écrit, s'arrête à un point calme sur
        // commande, continue, puis vérifie SA propre vue avant de fermer.
        "partage_vivant" => {
            let drapeaux = PathBuf::from(std::env::var("LS_DRAPEAUX").expect("LS_DRAPEAUX"));
            let s = SubstrateStore::open_shared_write(&base).expect("écriture partagée");
            let mut total = 0;
            for lot in 1..=3 {
                peupler(&s, total, 60, false);
                total += 60;
                // Fenêtre WAL non vide, écrivain vivant, rien de flushé.
                annoncer(&format!("LOT{lot} {}", vue_en_ligne(&lire(&s))));
                attendre_fichier(&drapeaux.join(format!("suite{lot}")));
            }
            // La vue de l'écrivain après que des lecteurs se sont ouverts et
            // fermés sous lui : elle doit être EXACTEMENT ce qu'il a écrit.
            annoncer(&format!("FINAL {}", vue_en_ligne(&lire(&s))));
            s.flush().expect("flush de l'écrivain");
            s.close().expect("close de l'écrivain");
            annoncer("FERME");
        }
        autre => panic!("rôle inconnu : {autre}"),
    }
}

// ---------------------------------------------------------------------------
// 0. Base propre — `substrate.alloc` réécrit à chaque ouverture
// ---------------------------------------------------------------------------

#[test]
fn base_propre_ouverte_lue_fermee_reste_identique_a_l_octet() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("propre");
    let attendue = base_propre(&base, 50);
    let avant = empreinte(&base);
    laisser_passer_l_horloge();

    for _ in 0..3 {
        assert_eq!(ouvrir_lire_fermer(&base), attendue, "vue du lecteur");
    }

    exiger_identique(&avant, &base, "base propre");
}

// ---------------------------------------------------------------------------
// 1. Écrivain tué avant son flush — rejeu du WAL
// ---------------------------------------------------------------------------

#[test]
fn ecrivain_tue_avant_flush_le_wal_est_rejoue_en_memoire_pas_dans_les_zones() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("tue");
    let mut e = Enfant::lancer("tue_avant_flush", &base, &[]);
    let vue_ecrivain = e.attendre_ligne("PRET ");
    e.tuer();

    let wal = std::fs::metadata(base.join("substrate.wal")).unwrap().len();
    assert!(wal > 0, "précondition : la fenêtre WAL est non vide");
    let avant = empreinte(&base);
    laisser_passer_l_horloge();

    // La vue cohérente : tout ce que l'écrivain avait écrit, flushé ou non.
    let vue = ouvrir_lire_fermer(&base);
    assert_eq!(
        vue_en_ligne(&vue),
        vue_ecrivain,
        "le lecteur doit voir la fenêtre WAL (nœuds, arêtes, degrés)"
    );
    assert_eq!(vue.noms.len(), 65, "les propriétés journalisées sont lues");
    exiger_identique(&avant, &base, "écrivain tué avant flush");

    // Une seconde ouverture rejoue de nouveau, toujours sans écrire.
    assert_eq!(vue_en_ligne(&ouvrir_lire_fermer(&base)), vue_ecrivain);
    exiger_identique(&avant, &base, "écrivain tué avant flush, 2ᵉ ouverture");

    // Et la base n'a rien perdu : un écrivain la reprend à l'identique.
    let s = SubstrateStore::open(&base).expect("reprise en écriture");
    assert_eq!(vue_en_ligne(&lire(&s)), vue_ecrivain);
}

// ---------------------------------------------------------------------------
// 2. Degrés : CRC périmé, fichier absent
// ---------------------------------------------------------------------------

#[test]
fn degres_au_crc_perime_sont_recalcules_en_memoire_le_fichier_reste_tel_quel() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("crc");
    let attendue = base_propre(&base, 60);
    // Le CRC de l'en-tête ne correspond plus au contenu : c'est l'état que
    // laisse un écrivain tué après des arêtes non flushées.
    let colonne = base.join("substrate.degrees.node.u32");
    let mut octets = std::fs::read(&colonne).expect("colonne de degrés présente");
    octets[16] ^= 0xFF;
    std::fs::write(&colonne, &octets).unwrap();
    let avant = empreinte(&base);
    laisser_passer_l_horloge();

    let vue = ouvrir_lire_fermer(&base);
    assert_eq!(vue, attendue, "degrés exacts malgré le CRC périmé");
    exiger_identique(&avant, &base, "degrés au CRC périmé");
}

#[test]
fn degres_absents_sont_calcules_en_memoire_aucun_fichier_n_est_cree() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("sans-degres");
    let attendue = base_propre(&base, 60);
    for entree in std::fs::read_dir(&base).unwrap() {
        let chemin = entree.unwrap().path();
        let nom = chemin.file_name().unwrap().to_string_lossy().into_owned();
        if nom.starts_with("substrate.degrees") {
            std::fs::remove_file(&chemin).unwrap();
        }
    }
    let avant = empreinte(&base);
    laisser_passer_l_horloge();

    let vue = ouvrir_lire_fermer(&base);
    assert_eq!(vue.noeuds, attendue.noeuds);
    assert_eq!(vue.aretes, attendue.aretes);
    assert_eq!(
        (vue.somme_sortants, vue.somme_entrants),
        (attendue.somme_sortants, attendue.somme_entrants),
        "degrés génériques exacts sans la colonne"
    );
    exiger_identique(&avant, &base, "colonnes de degrés absentes");
}

// ---------------------------------------------------------------------------
// 3. Graphe nommé
// ---------------------------------------------------------------------------

#[test]
fn graphe_nomme_s_ouvre_lui_aussi_en_lecture_seule() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("graphes");
    let mut e = Enfant::lancer("graphe_nomme", &base, &[]);
    let archives: usize = e.attendre_ligne("PRET ").parse().unwrap();
    e.tuer();
    assert_eq!(archives, 12);
    assert!(base.join("graphs").join("archives").is_dir());
    let avant = empreinte(&base);
    laisser_passer_l_horloge();

    {
        let s = SubstrateStore::open_read_only(&base).expect("ouverture en lecture seule");
        let g = s.named_graph("archives").expect("graphe nommé monté");
        assert_eq!(g.node_count(), archives, "fenêtre WAL de l'enfant rejouée");
        assert_eq!(s.node_count(), 26);
        // Le graphe nommé est lui aussi en lecture seule : une mutation de
        // l'ENFANT est refusée avant d'agir (il n'est pas ouvert en écriture).
        let refus = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            g.create_node(&["Intrus"]);
        }));
        let message = match refus {
            Ok(()) => panic!("create_node accepté par un graphe nommé en lecture seule"),
            Err(p) => p.downcast_ref::<String>().cloned().unwrap_or_default(),
        };
        assert!(
            message.contains("lecture seule"),
            "refus non nommé : {message}"
        );
        assert_eq!(
            g.node_count(),
            archives,
            "le refus a modifié le graphe nommé"
        );
        let _ = lire(&s);
        s.close().unwrap();
    }

    exiger_identique(&avant, &base, "graphe nommé");
}

// ---------------------------------------------------------------------------
// 4. Dictionnaire sans histogrammes — `flush()` pendant l'ouverture
// ---------------------------------------------------------------------------

#[test]
fn dictionnaire_sans_histogrammes_est_complete_en_memoire_sans_flush() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("dict-v5");
    let attendue = base_propre(&base, 30);
    // État d'un dictionnaire v5 : compteurs d'arêtes par type présents,
    // histogrammes de labels pairs absents.
    let chemin_dict = base.join("substrate.dict");
    let mut dict = obrain_substrate::DictSnapshot::load(&chemin_dict).expect("dict");
    let compteurs = dict.counters.as_mut().expect("compteurs persistés");
    assert!(!compteurs.edge_type_counts.is_empty());
    compteurs.edge_type_target_label_counts.clear();
    compteurs.edge_type_source_label_counts.clear();
    dict.persist(&chemin_dict).expect("réécrire le dict");
    let avant = empreinte(&base);
    laisser_passer_l_horloge();

    {
        let s = SubstrateStore::open_read_only(&base).expect("ouverture en lecture seule");
        assert!(
            s.edge_target_labels("CONNAIT").contains("Personne"),
            "histogrammes reconstruits en mémoire"
        );
        assert_eq!(lire(&s), attendue);
        s.close().unwrap();
    }

    exiger_identique(&avant, &base, "dictionnaire sans histogrammes");
}

// ---------------------------------------------------------------------------
// 5. Sidecar hérité
// ---------------------------------------------------------------------------

fn base_heritee(base: &Path) -> NodeId {
    // `OBRAIN_PROPS_V2=0` dans l'ENFANT seulement : la variable est lue à
    // l'ouverture, et la poser ici contaminerait les tests voisins.
    let mut e = Enfant::lancer("heritee", base, &[("OBRAIN_PROPS_V2", "0".to_string())]);
    let id: u64 = e.attendre_ligne("PRET ").parse().unwrap();
    e.terminer();
    assert!(
        std::fs::metadata(base.join(obrain_substrate::PROPS_FILENAME))
            .unwrap()
            .len()
            > 0,
        "précondition : le sidecar hérité porte des données"
    );
    NodeId(id)
}

#[test]
fn sidecar_herite_scalaire_est_lu_sans_rien_ecrire() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("heritee");
    let id = base_heritee(&base);
    let avant = empreinte(&base);
    laisser_passer_l_horloge();

    {
        let s = SubstrateStore::open_read_only(&base).expect("ouverture en lecture seule");
        assert_eq!(
            s.get_node_property(id, &PropertyKey::new("titre")),
            Some(Value::String("ancien".into()))
        );
        let _ = lire(&s);
        s.close().unwrap();
    }

    exiger_identique(&avant, &base, "sidecar hérité (scalaires)");
}

#[test]
fn sidecar_herite_avec_vecteur_a_migrer_fait_echouer_l_ouverture_sans_migrer() {
    use obrain_substrate::PropertiesStreamingWriter;
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("heritee-vecteur");
    let id = base_heritee(&base);
    // Sidecar d'avant les colonnes vectorielles : le vecteur est DANS le
    // sidecar, et l'ouverture le migrait vers une colonne par
    // `set_node_property` (fichiers créés, écrits).
    let vecteur: std::sync::Arc<[f32]> = (0..16).map(|i| i as f32 * 0.5).collect();
    {
        let mut w =
            PropertiesStreamingWriter::open(base.join(obrain_substrate::PROPS_FILENAME)).unwrap();
        w.append_node(
            id.0,
            &[
                ("titre".to_string(), Value::String("ancien".into())),
                ("plongement".to_string(), Value::Vector(vecteur)),
            ],
        )
        .unwrap();
        w.finish().unwrap();
    }
    let avant = empreinte(&base);
    laisser_passer_l_horloge();

    let ouverture = SubstrateStore::open_read_only(&base);
    exiger_identique(&avant, &base, "sidecar hérité à migrer");
    let erreur = match ouverture {
        Ok(_) => {
            panic!("l'ouverture en lecture seule a réussi : elle a migré, ou ignoré, le vecteur")
        }
        Err(e) => e.to_string(),
    };
    assert!(
        erreur.contains("lecture seule") && erreur.contains("écriture"),
        "le refus doit être nommé et dire quoi lancer : {erreur}"
    );
}

// ---------------------------------------------------------------------------
// 6. Temporaires de remap
// ---------------------------------------------------------------------------

#[test]
fn temporaires_de_remap_orphelins_ne_sont_pas_supprimes() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("remap-tmp");
    let attendue = base_propre(&base, 20);
    std::fs::write(
        base.join("substrate.veccol.node.plongement.f32.16.remap.7.tmp"),
        b"trace d'un remap interrompu",
    )
    .unwrap();
    let avant = empreinte(&base);
    laisser_passer_l_horloge();

    assert_eq!(ouvrir_lire_fermer(&base), attendue);

    exiger_identique(&avant, &base, "temporaire de remap orphelin");
}

// ---------------------------------------------------------------------------
// 7. Fichiers absents
// ---------------------------------------------------------------------------

#[test]
fn fichiers_absents_ne_sont_pas_crees() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("troue");
    let attendue = base_propre(&base, 20);
    // Tout ce que l'ouverture recréait : la borne d'allocation, le WAL, son
    // ancre, et les zones annexes sondées par `open_or_create`.
    for nom in [
        "substrate.alloc",
        "substrate.wal",
        "substrate.wal.lsn",
        "substrate.community",
        "substrate.zorder",
        "substrate.hilbert",
        "substrate.engram_members",
        "substrate.engram_bitset",
        "substrate.simplex_dir",
        "substrate.tier0",
        "substrate.tier1",
        "substrate.tier2",
        "substrate.checkpoint",
    ] {
        let _ = std::fs::remove_file(base.join(nom));
    }
    let avant = empreinte(&base);
    laisser_passer_l_horloge();

    assert_eq!(ouvrir_lire_fermer(&base), attendue);

    exiger_identique(&avant, &base, "fichiers absents");
}

#[test]
fn sans_fichier_de_verrou_l_ouverture_est_refusee_et_ne_le_cree_pas() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("sans-verrou");
    base_propre(&base, 5);
    std::fs::remove_file(base.join("substrate.dirlock")).unwrap();
    let avant = empreinte(&base);
    laisser_passer_l_horloge();

    let ouverture = SubstrateStore::open_read_only(&base);
    exiger_identique(&avant, &base, "base sans fichier de verrou");
    let erreur = match ouverture {
        Ok(_) => panic!("ouverte sans verrou : rien ne la protège d'un écrivain exclusif"),
        Err(e) => e.to_string(),
    };
    assert!(
        erreur.contains("lecture seule") && erreur.contains("substrate.dirlock"),
        "refus nommé attendu : {erreur}"
    );
}

// ---------------------------------------------------------------------------
// 8. Ancre du WAL
// ---------------------------------------------------------------------------

#[test]
fn ancre_du_wal_absente_n_est_pas_republiee() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("ancre");
    let mut e = Enfant::lancer("tue_avant_flush", &base, &[]);
    let vue_ecrivain = e.attendre_ligne("PRET ");
    e.tuer();
    // WAL non vide et side-car d'ancre absent : l'ouverture « réancre en
    // avant » et republiait le side-car.
    std::fs::remove_file(base.join("substrate.wal.lsn")).expect("ancre présente");
    let avant = empreinte(&base);
    laisser_passer_l_horloge();

    assert_eq!(vue_en_ligne(&ouvrir_lire_fermer(&base)), vue_ecrivain);

    exiger_identique(&avant, &base, "ancre du WAL absente");
}

// ---------------------------------------------------------------------------
// 9. Dossier qui n'est pas une base
// ---------------------------------------------------------------------------

/// CE QUE CE TEST PROUVE, ET CE QU'IL NE PROUVE PAS : `SubstrateFile::open_read_only`
/// (ouverture sans droit d'écriture, sans création) refuse un dossier sans
/// `substrate.meta` par une erreur NOMMÉE, et ne pose rien. L'échec vient donc de
/// la couche substrat ; la garde qui empêchait `ObrainDB::open_read_only` de
/// CRÉER une base vide vit dans le moteur et est éprouvée par
/// `obrain-engine/tests/lecture_seule_contrat.rs`
/// (`un_dossier_qui_n_est_pas_une_base_echoue_sans_rien_creer`).
#[test]
fn le_substrat_refuse_un_dossier_sans_substrate_meta_par_une_erreur_nommee_et_ne_cree_rien() {
    let td = tempfile::tempdir().unwrap();
    let parent = td.path().join("bases");
    std::fs::create_dir(&parent).unwrap();
    base_propre(&parent.join("une"), 3);
    let vide = td.path().join("vide");
    std::fs::create_dir(&vide).unwrap();
    let absent = td.path().join("absent");
    let avant = empreinte(td.path());
    laisser_passer_l_horloge();

    for (quoi, chemin, attendu) in [
        ("dossier vide", &vide, "n'est pas une base obrain"),
        (
            "dossier PARENT de bases",
            &parent,
            "n'est pas une base obrain",
        ),
        ("chemin absent", &absent, "does not exist"),
    ] {
        let erreur = match SubstrateStore::open_read_only(chemin) {
            Ok(_) => panic!("{quoi} : ouvert comme une base"),
            Err(e) => e.to_string(),
        };
        assert!(
            erreur.contains(attendu),
            "{quoi} : erreur non nommée : {erreur}"
        );
    }

    exiger_identique(&avant, td.path(), "dossiers qui ne sont pas des bases");
    assert!(!absent.exists());
}

// ---------------------------------------------------------------------------
// 10. Écrivain partagé VIVANT (inter-processus)
// ---------------------------------------------------------------------------

#[test]
fn ecrivain_partage_vivant_le_lecteur_ne_touche_a_rien_et_l_ecrivain_garde_sa_vue() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("partage");
    let drapeaux = td.path().join("drapeaux");
    std::fs::create_dir(&drapeaux).unwrap();
    drop(SubstrateStore::create(&base).expect("création"));

    let mut e = Enfant::lancer(
        "partage_vivant",
        &base,
        &[("LS_DRAPEAUX", drapeaux.display().to_string())],
    );
    let mut derniere = String::new();
    for lot in 1..=3 {
        // L'écrivain est VIVANT, au repos, fenêtre WAL non vide.
        let vue_ecrivain = e.attendre_ligne(&format!("LOT{lot} "));
        let avant = empreinte(&base);
        laisser_passer_l_horloge();

        for _ in 0..3 {
            let vue = ouvrir_lire_fermer(&base);
            assert_eq!(
                vue_en_ligne(&vue),
                vue_ecrivain,
                "lot {lot} : le lecteur voit ce que l'écrivain vivant a écrit"
            );
        }

        exiger_identique(
            &avant,
            &base,
            &format!("écrivain partagé vivant, lot {lot}"),
        );
        derniere = vue_ecrivain;
        std::fs::write(drapeaux.join(format!("suite{lot}")), b"").unwrap();
    }
    // La vue de l'écrivain n'a pas bougé sous lui : ses degrés ne divergent pas.
    assert_eq!(
        e.attendre_ligne("FINAL "),
        derniere,
        "l'écrivain vivant a vu sa propre base changer pendant que des lecteurs s'ouvraient"
    );
    e.attendre_ligne("FERME");
    e.terminer();

    // Vérité du disque après fermeture propre de l'écrivain.
    let s = SubstrateStore::open(&base).expect("réouverture en écriture");
    assert_eq!(vue_en_ligne(&lire(&s)), derniere);
}

// ---------------------------------------------------------------------------
// 11. Mutations
// ---------------------------------------------------------------------------

/// Exécute `f` ; rend le message de l'erreur ou de la panique, `None` si `f`
/// a réussi.
fn refus<T>(f: impl FnOnce() -> Result<T, String>) -> Option<String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(Ok(_)) => None,
        Ok(Err(e)) => Some(e),
        Err(p) => Some(
            p.downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_string()))
                .unwrap_or_else(|| "panique sans message".to_string()),
        ),
    }
}

#[test]
fn les_mutations_sont_refusees_par_une_erreur_nommee_et_n_ecrivent_rien() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("mutations");
    let attendue = base_propre(&base, 12);
    let avant = empreinte(&base);
    laisser_passer_l_horloge();

    let s = SubstrateStore::open_read_only(&base).expect("ouverture en lecture seule");
    let ids = s.node_ids();
    let (a, b) = (ids[0], ids[1]);

    type Tentative<'a> = (&'a str, Box<dyn FnOnce() -> Result<(), String> + 'a>);
    fn ok<T>(_: T) -> Result<(), String> {
        Ok(())
    }
    let texte = |e: obrain_substrate::SubstrateError| e.to_string();
    let tentatives: Vec<Tentative<'_>> = vec![
        // ── graphe : nœuds, arêtes, propriétés, labels ──
        ("create_node", Box::new(|| ok(s.create_node(&["Intrus"])))),
        (
            "create_edge",
            Box::new(|| ok(s.create_edge(a, b, "INTRUS"))),
        ),
        (
            "batch_create_edges",
            Box::new(|| ok(s.batch_create_edges(&[(a, b, "INTRUS")]))),
        ),
        ("delete_node", Box::new(|| ok(s.delete_node(a)))),
        (
            "delete_node_edges",
            Box::new(|| {
                s.delete_node_edges(a);
                Ok(())
            }),
        ),
        (
            "set_node_property",
            Box::new(|| {
                s.set_node_property(a, "nom", Value::String("x".into()));
                Ok(())
            }),
        ),
        (
            "remove_node_property",
            Box::new(|| ok(s.remove_node_property(a, "nom"))),
        ),
        ("add_label", Box::new(|| ok(s.add_label(a, "Intrus")))),
        (
            "remove_label",
            Box::new(|| ok(s.remove_label(a, "Personne"))),
        ),
        (
            "create_external_edge",
            Box::new(|| ok(s.create_external_edge(a, "VERS", None, "autre", "cle"))),
        ),
        // ── graphes nommés ──
        (
            "create_named_graph",
            Box::new(|| {
                s.create_named_graph("intrus")
                    .map(|_| ())
                    .map_err(|e| format!("lecture seule ({e:?})"))
            }),
        ),
        // ── durabilité, consolidation, réparation ──
        ("flush", Box::new(|| s.flush().map_err(texte))),
        (
            "checkpoint",
            Box::new(|| s.checkpoint().map(|_| ()).map_err(texte)),
        ),
        (
            "repair_outgoing_chains",
            Box::new(|| s.repair_outgoing_chains().map(|_| ()).map_err(texte)),
        ),
        (
            "rebuild_degree_columns",
            Box::new(|| s.rebuild_degree_columns().map(|_| ()).map_err(texte)),
        ),
        (
            "repermuter_par_hilbert",
            Box::new(|| s.repermuter_par_hilbert(4, 8).map_err(texte)),
        ),
        // ── couche cognitive : énergie, synapses ──
        (
            "set_node_energy_f32",
            Box::new(|| s.set_node_energy_f32(a, 0.5).map(|_| ()).map_err(texte)),
        ),
        (
            "decay_all_energy",
            Box::new(|| s.decay_all_energy(0.9).map_err(texte)),
        ),
        // ── index et catalogues ──
        (
            "fusionner_index_egalite",
            Box::new(|| s.fusionner_index_egalite().map_err(texte)),
        ),
        (
            "fusionner_index_ordonne",
            Box::new(|| s.fusionner_index_ordonne().map_err(texte)),
        ),
        (
            "fusionner_index_ord_valeur",
            Box::new(|| s.fusionner_index_ord_valeur().map_err(texte)),
        ),
        (
            "declarer_type",
            Box::new(|| s.declarer_type("node", "Intrus", "()").map_err(texte)),
        ),
        (
            "retirer_type",
            Box::new(|| s.retirer_type("node", "Personne").map_err(texte)),
        ),
        // ── graphes nommés ──
        (
            "drop_named_graph",
            Box::new(|| ok(s.drop_named_graph("archives"))),
        ),
        (
            "copy_named_graph",
            Box::new(|| s.copy_named_graph(None, Some("copie"))),
        ),
        (
            "write_meta_header",
            Box::new(|| {
                s.avec_substrate(|f| {
                    let mut f2 =
                        obrain_substrate::SubstrateFile::open_read_only(f.path()).map_err(texte)?;
                    let h = f2.meta_header();
                    f2.write_meta_header(&h).map_err(texte)
                })
            }),
        ),
        // ── journal ──
        (
            "append au WAL",
            Box::new(|| {
                s.writer()
                    .wal()
                    .append(obrain_substrate::WalRecord {
                        lsn: 0,
                        timestamp: 0,
                        flags: 0,
                        payload: obrain_substrate::WalPayload::NodeDelete { node_id: 1 },
                    })
                    .map(|_| ())
                    .map_err(texte)
            }),
        ),
        (
            "troncature du WAL",
            Box::new(|| s.writer().wal().truncate().map(|_| ()).map_err(texte)),
        ),
    ];

    let mut acceptees = Vec::new();
    let mut mal_nommees = Vec::new();
    for (nom, tentative) in tentatives {
        match refus(tentative) {
            None => acceptees.push(nom),
            Some(message) if !message.contains("lecture seule") => {
                mal_nommees.push(format!("{nom} : {message}"));
            }
            Some(_) => {}
        }
    }
    assert!(
        acceptees.is_empty(),
        "mutations ACCEPTÉES par un store en lecture seule : {acceptees:?}"
    );
    assert!(
        mal_nommees.is_empty(),
        "refus qui ne nomment pas la lecture seule :\n  {}",
        mal_nommees.join("\n  ")
    );

    // Les refus n'ont rien changé : ni la vue de la session, ni le disque.
    assert_eq!(lire(&s), attendue, "la vue du lecteur après les refus");
    s.close().unwrap();
    drop(s);
    exiger_identique(&avant, &base, "mutations refusées");
}

// ---------------------------------------------------------------------------
// 12. Garantie par le système : base non inscriptible
// ---------------------------------------------------------------------------

/// Retire tout droit d'écriture sur la base (fichiers ET répertoires) : toute
/// écriture, création ou suppression oubliée devient une erreur du noyau. La
/// lecture seule doit s'ouvrir et répondre quand même — c'est la preuve qu'elle
/// n'a besoin d'AUCUN droit d'écriture.
#[cfg(unix)]
#[test]
fn base_non_inscriptible_s_ouvre_et_repond_en_lecture_seule() {
    use std::os::unix::fs::PermissionsExt as _;

    fn droits(racine: &Path, fichier: u32, dossier: u32) {
        for entree in std::fs::read_dir(racine).unwrap() {
            let chemin = entree.unwrap().path();
            if chemin.is_dir() {
                droits(&chemin, fichier, dossier);
            } else {
                std::fs::set_permissions(&chemin, std::fs::Permissions::from_mode(fichier))
                    .unwrap();
            }
        }
        std::fs::set_permissions(racine, std::fs::Permissions::from_mode(dossier)).unwrap();
    }

    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("gravee");
    let mut e = Enfant::lancer("tue_avant_flush", &base, &[]);
    let vue_ecrivain = e.attendre_ligne("PRET ");
    e.tuer();

    droits(&base, 0o444, 0o555);
    let resultat = std::panic::catch_unwind(|| {
        let avant = empreinte(&base);
        let vue = ouvrir_lire_fermer(&base);
        (avant, vue)
    });
    // Rendre les droits AVANT toute assertion : le répertoire temporaire doit
    // pouvoir être supprimé même si le test échoue.
    droits(&base, 0o644, 0o755);

    let (avant, vue) = resultat.expect("ouverture en lecture seule d'une base non inscriptible");
    assert_eq!(
        vue_en_ligne(&vue),
        vue_ecrivain,
        "fenêtre WAL rejouée en mémoire sur une base gravée"
    );
    // Seuls les droits ont changé entre-temps (par ce test) : contenu identique.
    let apres = empreinte(&base);
    for (chemin, a) in &avant {
        let b = apres.get(chemin).expect("fichier toujours là");
        assert_eq!(
            (a.taille, a.condense, a.inode),
            (b.taille, b.condense, b.inode),
            "{} a changé",
            chemin.display()
        );
    }
    assert_eq!(avant.len(), apres.len(), "aucun fichier créé ni supprimé");
}
