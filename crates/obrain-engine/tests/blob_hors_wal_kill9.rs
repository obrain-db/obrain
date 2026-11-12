//! obrain#120 — UNE VALEUR DE PLUS DE 256 OCTETS SURVIT À UN `kill -9` ET NE
//! CONTAMINE AUCUN AUTRE NŒUD.
//!
//! Une chaîne de plus de `BLOB_COLUMN_THRESHOLD_BYTES` (256 o) part en colonne
//! blob (`substrate.blobcol.*.idx` + `.dat`), dont l'en-tête (n_slots, fin
//! d'arène, CRC) n'est réécrit qu'au `sync` d'un flush complet. Avant le
//! correctif, le WAL ne portait pas la valeur : mesuré sur main 28020628,
//! 0/40 valeurs acquittées relues, et 40/40 nœuds relisant les octets d'un
//! autre après une réécriture tuée suivie d'un ajout tué. Le correctif journalise
//! la valeur (`BlobColumnWrite`) et son effacement (`BlobColumnClear`), et
//! l'écrivain qui rouvre une colonne recalcule la fin d'arène sur l'index
//! (une écriture de colonne sans trame ne se fait plus écraser).
//!
//! Ce que ce fichier vérifie, après un `kill -9` sans `close()` ni
//! `checkpoint()` :
//!   1. PERTE : chaque nœud acquitté porte encore SA valeur longue (les valeurs
//!      courtes des mêmes commits servent de témoin : elles passent par le WAL).
//!   2. CONTAMINATION : des écrivains successifs tués ne font jamais relire à un
//!      nœud la valeur d'un autre — y compris après une écriture de colonne SANS
//!      trame (valeur au-delà du plafond WAL).
//!   3. RÉSURRECTION : une valeur courte écrite après une longue reste la valeur.
//!   4. LIMITES verrouillées : lecteur seul, plafond de 32 Mio.
//!
//! Protocole repris de `durabilite_commit_kill9.rs` : le parent relance SON
//! propre binaire de test sur le cas enfant (`--exact`), répertoire passé par
//! variable d'environnement ; l'enfant écrit, commite, pose un marqueur, dort ;
//! le parent attend le marqueur, tue -9, rouvre, compte.
#![cfg(feature = "wal")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use std::path::Path;
use std::time::Duration;

/// Nœuds écrits par chaque écrivain.
const N: i64 = 40;
/// Longueur des valeurs longues : celle mesurée dans le ticket (585 o), bien
/// au-dessus du seuil de routage blob (256 o).
const LONGUEUR: usize = 585;
/// Variable de l'enfant : `<tour>:<fermer 0|1>:<maj 0|1>:<répertoire>`.
/// `fermer = 1` : `close()` propre (flush complet) au lieu d'attendre le kill.
/// `maj = 1` : au lieu de créer des nœuds, RÉÉCRIT (`SET`) la valeur longue des
/// nœuds du tour 1 avec celle du tour courant. `maj = 2` : la réécrit longue
/// PUIS courte (`valeur_courte`), deux commits par nœud.
/// `maj = 3` : crée DEUX nœuds `:Gros` par l'API (pas de Cypher), l'un avec une
/// valeur de 1 Mio (sous le plafond WAL), l'autre juste au-dessus du plafond,
/// puis `flush_wal`.
const ENV_ENFANT: &str = "OBRAIN_TEST_BLOB_KILL9_ENFANT";

/// Valeur longue propre au nœud `k` du tour `tour` : l'identité du nœud est en
/// tête ET répétée, de sorte qu'une valeur relue sur le mauvais nœud, ou
/// réécrite en partie par une autre, se voit.
fn valeur_longue(tour: u8, k: i64) -> String {
    let motif = format!("[t{tour}-k{k:04}]");
    let mut s = String::with_capacity(LONGUEUR);
    while s.len() + motif.len() <= LONGUEUR {
        s.push_str(&motif);
    }
    while s.len() < LONGUEUR {
        s.push('.');
    }
    s
}

/// Deux valeurs énormes, par l'API : `sous` (1 Mio, journalisée) et `dessus`
/// (plafond WAL + 1, non journalisée). Un `flush_wal` : les deux colonnes sont
/// écrites, seule la première a sa trame fsyncée.
fn ecrire_gros(db: &ObrainDB) {
    let plafond = obrain_substrate::BLOB_WAL_MAX_BYTES;
    for (nom, taille) in [("sous", 1usize << 20), ("dessus", plafond + 1)] {
        let id = db.create_node(&["Gros"]);
        db.set_node_property(id, "nom", Value::String(nom.into()));
        db.set_node_property(id, "doc", Value::String("g".repeat(taille).into()));
    }
    db.flush_wal().expect("flush_wal");
}

/// Réécrit la valeur longue du nœud t1-k0 avec une valeur AU-DELÀ du plafond
/// WAL : la colonne est écrite (octets + entrée d'index, dans le mmap), aucune
/// trame ne l'est. C'est, en production, l'état exact d'un kill tombé entre
/// l'écriture de colonne et la trame. Puis `flush_wal`, sans flush complet.
fn reecrire_au_dela_du_plafond(db: &ObrainDB) {
    let r = db
        .session()
        .execute("MATCH (n:Doc) WHERE n.tour = 1 AND n.k = 0 RETURN id(n)")
        .expect("id de t1-k0");
    let Some(Value::Int64(id)) = r.rows.first().and_then(|l| l.first()).cloned() else {
        panic!("t1-k0 introuvable : {:?}", r.rows);
    };
    let enorme = "E".repeat(obrain_substrate::BLOB_WAL_MAX_BYTES + 1);
    db.set_node_property(
        obrain_common::types::NodeId(id as u64),
        "doc",
        Value::String(enorme.into()),
    );
    db.flush_wal().expect("flush_wal");
}

/// Valeur COURTE (sous le seuil blob) qui remplace une valeur longue.
fn valeur_courte(tour: u8, k: i64) -> String {
    format!("court-t{tour}-k{k}")
}

fn attendre(marqueur: &Path) {
    let mut tours = 0u32;
    while !marqueur.exists() {
        tours += 1;
        assert!(
            tours < 2400,
            "l'enfant n'a jamais posé {}",
            marqueur.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn lancer_enfant(tour: u8, fermer: bool, dir: &Path) -> std::process::Child {
    lancer_enfant_mode(tour, fermer, 0, dir)
}

fn lancer_enfant_mode(tour: u8, fermer: bool, maj: u8, dir: &Path) -> std::process::Child {
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(["enfant_ecrivain", "--exact", "--nocapture"])
        .env(
            ENV_ENFANT,
            format!("{tour}:{}:{}:{}", u8::from(fermer), maj, dir.display()),
        )
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn enfant")
}

fn tuer(mut enfant: std::process::Child) {
    // `Child::kill` envoie SIGKILL sous Unix : ni drop, ni flush, ni close.
    enfant.kill().expect("kill -9");
    let _ = enfant.wait();
}

fn dormir_pour_toujours() -> ! {
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

/// Écrit `N` nœuds `:Doc {tour, k, court, doc}`, un commit (implicite) par
/// nœud, puis : `fermer` → `close()` propre et sortie ; sinon pose le
/// marqueur et dort jusqu'au kill.
#[test]
fn enfant_ecrivain() {
    let Ok(spec) = std::env::var(ENV_ENFANT) else {
        return;
    };
    let mut morceaux = spec.splitn(4, ':');
    let tour: u8 = morceaux.next().unwrap().parse().unwrap();
    let fermer = morceaux.next().unwrap() == "1";
    let maj: u8 = morceaux.next().unwrap().parse().unwrap();
    let dir = Path::new(morceaux.next().unwrap());
    let db = ObrainDB::open(dir.join("base")).expect("open enfant");
    if maj == 4 {
        reecrire_au_dela_du_plafond(&db);
        std::fs::write(dir.join(format!("pret{tour}")), b"").unwrap();
        dormir_pour_toujours();
    }
    if maj == 3 {
        ecrire_gros(&db);
        std::fs::write(dir.join(format!("pret{tour}")), b"").unwrap();
        dormir_pour_toujours();
    }
    let s = db.session();
    for k in 0..N {
        let doc = valeur_longue(tour, k);
        let cible = format!("MATCH (n:Doc) WHERE n.tour = 1 AND n.k = {k}");
        match maj {
            0 => {
                s.execute(&format!(
                    "INSERT (:Doc {{tour: {tour}, k: {k}, court: 'c{tour}-{k}', doc: '{doc}'}})"
                ))
                .expect("insert acquitté");
            }
            1 => {
                s.execute(&format!("{cible} SET n.doc = '{doc}'"))
                    .expect("réécriture acquittée");
            }
            _ => {
                s.execute(&format!("{cible} SET n.doc = '{doc}'"))
                    .expect("réécriture longue acquittée");
                let court = valeur_courte(tour, k);
                s.execute(&format!("{cible} SET n.doc = '{court}'"))
                    .expect("réécriture courte acquittée");
            }
        }
    }
    drop(s);
    if fermer {
        db.close().expect("close propre");
        std::fs::write(dir.join(format!("pret{tour}")), b"").unwrap();
        return;
    }
    std::fs::write(dir.join(format!("pret{tour}")), b"").unwrap();
    dormir_pour_toujours();
}

/// Bilan d'un tour relu après coup.
#[derive(Debug, Default)]
struct Bilan {
    /// Nœuds du tour présents (témoin : propriétés courtes, via le WAL).
    noeuds: usize,
    /// Valeurs courtes exactes.
    courts_exacts: usize,
    /// Valeurs longues exactes (celle du nœud lui-même).
    longs_exacts: usize,
    /// Valeurs longues absentes.
    longs_absents: usize,
    /// Valeurs longues présentes mais FAUSSES (celle d'un autre nœud, ou un
    /// mélange) — la contamination.
    longs_faux: Vec<String>,
}

fn bilan(db: &ObrainDB, tour: u8) -> Bilan {
    bilan_attendu(db, tour, tour)
}

/// Bilan des nœuds du tour `tour`, dont la valeur longue attendue est celle du
/// tour `tour_valeur` (différent quand un tour ultérieur l'a réécrite).
fn bilan_attendu(db: &ObrainDB, tour: u8, tour_valeur: u8) -> Bilan {
    let session = db.session();
    let resultat = session
        .execute(&format!(
            "MATCH (n:Doc) WHERE n.tour = {tour} RETURN n.k, n.court, n.doc"
        ))
        .expect("relecture");
    let mut bilan = Bilan::default();
    for ligne in &resultat.rows {
        bilan.noeuds += 1;
        let k = match ligne.first() {
            Some(Value::Int64(k)) => *k,
            autre => panic!("k inattendu : {autre:?}"),
        };
        if let Some(Value::String(c)) = ligne.get(1)
            && c.as_str() == format!("c{tour}-{k}")
        {
            bilan.courts_exacts += 1;
        }
        match ligne.get(2) {
            Some(Value::String(d)) if d.as_str() == valeur_longue(tour_valeur, k) => {
                bilan.longs_exacts += 1;
            }
            Some(Value::String(d)) => bilan.longs_faux.push(format!(
                "nœud t{tour}-k{k} (attendu [t{tour_valeur}-k{k:04}]) relit {:?}…",
                &d.as_str()[..d.len().min(24)]
            )),
            _ => bilan.longs_absents += 1,
        }
    }
    bilan
}

/// PERTE : écrivain tué après N commits acquittés portant une valeur longue.
/// Attendu : les N valeurs longues relues, exactes. Sur main (11/11/2026) :
/// voir le message d'échec, qui donne les comptes.
#[test]
fn une_valeur_longue_acquittee_survit_a_un_kill_9() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join("base");
    let enfant = lancer_enfant(1, false, dir.path());
    attendre(&dir.path().join("pret1"));
    tuer(enfant);

    let db = ObrainDB::open(&base).expect("réouverture");
    let b = bilan(&db, 1);
    assert_eq!(
        (b.noeuds, b.courts_exacts),
        (N as usize, N as usize),
        "témoin : les nœuds et leurs valeurs COURTES acquittés doivent survivre ({b:?})"
    );
    assert!(
        b.longs_exacts == N as usize,
        "obrain#120 : sur {N} valeurs longues ({LONGUEUR} o) acquittées, {} relues exactes, \
         {} absentes, {} fausses après kill -9 : {:?}",
        b.longs_exacts,
        b.longs_absents,
        b.longs_faux.len(),
        b.longs_faux
    );
}

/// CONTAMINATION : un second écrivain rouvre la base laissée par le premier,
/// écrit N autres valeurs longues, est tué à son tour. Aucun nœud, d'aucun des
/// deux tours, ne doit relire la valeur d'un autre.
#[test]
fn un_second_ecrivain_tue_ne_fait_relire_a_aucun_noeud_la_valeur_d_un_autre() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join("base");
    let enfant = lancer_enfant(1, false, dir.path());
    attendre(&dir.path().join("pret1"));
    tuer(enfant);
    let enfant = lancer_enfant(2, false, dir.path());
    attendre(&dir.path().join("pret2"));
    tuer(enfant);

    let db = ObrainDB::open(&base).expect("réouverture");
    let b1 = bilan(&db, 1);
    let b2 = bilan(&db, 2);
    let faux: Vec<&String> = b1.longs_faux.iter().chain(&b2.longs_faux).collect();
    assert!(
        faux.is_empty(),
        "obrain#120 (contamination) : {} valeur(s) longue(s) relue(s) sur le mauvais nœud \
         après deux écrivains tués — tour 1 {b1:?} ; tour 2 {b2:?}",
        faux.len()
    );
    assert_eq!(
        (b1.longs_exacts, b2.longs_exacts),
        (N as usize, N as usize),
        "obrain#120 (perte) : tour 1 {b1:?} ; tour 2 {b2:?}"
    );
}

/// CONTAMINATION sur une colonne DÉJÀ DURABLE (le cas du ticket : « observé au
/// tour 2 ») : le tour 1 se ferme proprement (en-tête de colonne à jour), les
/// tours 2 et 3 sont tués sans flush. La fin d'arène de l'en-tête est celle du
/// tour 1 ; le tour 3 rouvre sur cet en-tête périmé et ajoute à partir de là,
/// par-dessus les octets du tour 2 dont les entrées d'index ont survécu.
#[test]
fn apres_un_flush_deux_ecrivains_tues_ne_font_relire_a_aucun_noeud_la_valeur_d_un_autre() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join("base");
    let mut enfant = lancer_enfant(1, true, dir.path());
    assert!(
        enfant.wait().expect("attente").success(),
        "tour 1 (close propre)"
    );
    attendre(&dir.path().join("pret1"));
    for tour in [2u8, 3] {
        let enfant = lancer_enfant(tour, false, dir.path());
        attendre(&dir.path().join(format!("pret{tour}")));
        tuer(enfant);
    }

    let db = ObrainDB::open(&base).expect("réouverture");
    let bilans: Vec<Bilan> = [1u8, 2, 3].iter().map(|t| bilan(&db, *t)).collect();
    let faux: Vec<&String> = bilans.iter().flat_map(|b| &b.longs_faux).collect();
    assert!(
        faux.is_empty(),
        "obrain#120 (contamination) : {} valeur(s) longue(s) relue(s) sur le mauvais nœud : \
         {faux:?} — bilans {bilans:?}",
        faux.len()
    );
    let exacts: Vec<usize> = bilans.iter().map(|b| b.longs_exacts).collect();
    assert_eq!(
        exacts,
        vec![N as usize; 3],
        "obrain#120 (perte) : valeurs longues exactes par tour — bilans {bilans:?}"
    );
}

/// CONTAMINATION par RÉÉCRITURE de slots déjà durables : tour 1 fermé proprement
/// (en-tête à jour : n_slots et fin d'arène du tour 1), tour 2 RÉÉCRIT les 40
/// valeurs longues du tour 1 puis est tué (les entrées d'index, dans des slots
/// < n_slots, pointent au-delà de la fin d'arène de l'en-tête), tour 3 crée 40
/// nœuds puis est tué (il rouvre sur l'en-tête du tour 1 et ajoute à partir de
/// sa fin d'arène, donc par-dessus les octets du tour 2).
#[test]
fn une_reecriture_tuee_puis_un_ajout_tue_ne_font_relire_a_aucun_noeud_la_valeur_d_un_autre() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join("base");
    let mut enfant = lancer_enfant(1, true, dir.path());
    assert!(
        enfant.wait().expect("attente").success(),
        "tour 1 (close propre)"
    );
    attendre(&dir.path().join("pret1"));
    let enfant = lancer_enfant_mode(2, false, 1, dir.path());
    attendre(&dir.path().join("pret2"));
    tuer(enfant);
    let enfant = lancer_enfant(3, false, dir.path());
    attendre(&dir.path().join("pret3"));
    tuer(enfant);

    let db = ObrainDB::open(&base).expect("réouverture");
    // Les nœuds du tour 1 portent, acquittée, la valeur du tour 2.
    let b1 = bilan_attendu(&db, 1, 2);
    let b3 = bilan(&db, 3);
    let faux: Vec<&String> = b1.longs_faux.iter().chain(&b3.longs_faux).collect();
    let contamines = faux.iter().filter(|f| f.contains("[t3-")).count();
    assert!(
        faux.is_empty(),
        "obrain#120 (contamination) : {} valeur(s) longue(s) fausse(s), dont {contamines} \
         portant les octets du tour 3 : {faux:?} — tour 1 {b1:?} ; tour 3 {b3:?}",
        faux.len()
    );
    assert_eq!(
        (b1.longs_exacts, b3.longs_exacts),
        (N as usize, N as usize),
        "obrain#120 (perte) : tour 1 réécrit {b1:?} ; tour 3 {b3:?}"
    );
}

/// PAS DE RÉSURRECTION : dans la même fenêtre non flushée, chaque nœud reçoit
/// une valeur longue PUIS une valeur courte, et l'écrivain est tué. Le rejeu des
/// écritures blob ne doit pas remettre la valeur longue par-dessus la courte
/// (d'où la trame `BlobColumnClear`).
#[test]
fn une_valeur_courte_ecrite_apres_une_longue_reste_la_valeur_relue_apres_kill_9() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join("base");
    let mut enfant = lancer_enfant(1, true, dir.path());
    assert!(
        enfant.wait().expect("attente").success(),
        "tour 1 (close propre)"
    );
    attendre(&dir.path().join("pret1"));
    let enfant = lancer_enfant_mode(2, false, 2, dir.path());
    attendre(&dir.path().join("pret2"));
    tuer(enfant);

    let db = ObrainDB::open(&base).expect("réouverture");
    let r = db
        .session()
        .execute("MATCH (n:Doc) WHERE n.tour = 1 RETURN n.k, n.doc")
        .expect("relecture");
    let mut ecarts = Vec::new();
    for ligne in &r.rows {
        let Some(Value::Int64(k)) = ligne.first() else {
            panic!("k inattendu : {ligne:?}");
        };
        let attendu = valeur_courte(2, *k);
        match ligne.get(1) {
            Some(Value::String(d)) if d.as_str() == attendu => {}
            autre => ecarts.push(format!(
                "k{k} : attendu {attendu:?}, relu {:?}",
                autre.map(|v| format!("{v:?}").chars().take(30).collect::<String>())
            )),
        }
    }
    assert_eq!(r.rows.len(), N as usize, "nœuds du tour 1");
    assert!(
        ecarts.is_empty(),
        "{} nœud(s) ne relisent pas leur valeur courte : {ecarts:?}",
        ecarts.len()
    );
}

/// LIMITE v0.0.1, verrouillée : un LECTEUR (`open_read_only`) ouvert sur la base
/// laissée par un écrivain tué ne rejoue PAS les valeurs longues de la fenêtre
/// post-flush (il n'écrit rien, et la colonne blob ne se rejoue qu'en écrivant).
/// Il ne lit jamais une valeur FAUSSE ; il lit les valeurs ABSENTES jusqu'à ce
/// qu'un écrivain rouvre la base, qui les rejoue. Ce test rougit le jour où le
/// lecteur les servira (à mettre alors à jour, avec la ligne de
/// `docs/LIMITES-v0.0.1.md`).
#[test]
fn limite_v0_0_1_un_lecteur_seul_ne_rejoue_pas_les_valeurs_longues_d_un_ecrivain_tue() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join("base");
    let mut enfant = lancer_enfant(1, true, dir.path());
    assert!(
        enfant.wait().expect("attente").success(),
        "tour 1 (close propre)"
    );
    attendre(&dir.path().join("pret1"));
    let enfant = lancer_enfant(2, false, dir.path());
    attendre(&dir.path().join("pret2"));
    tuer(enfant);

    let lecteur = ObrainDB::open_read_only(&base).expect("lecteur");
    let (l1, l2) = (bilan(&lecteur, 1), bilan(&lecteur, 2));
    drop(lecteur);
    assert!(
        l1.longs_faux.is_empty() && l2.longs_faux.is_empty(),
        "un lecteur ne doit jamais lire une valeur fausse : {l1:?} ; {l2:?}"
    );
    assert_eq!(
        (
            l1.longs_exacts,
            l2.noeuds,
            l2.longs_exacts,
            l2.longs_absents
        ),
        (N as usize, N as usize, 0, N as usize),
        "limite mesurée : le lecteur sert le tour flushé, voit les nœuds du tour tué mais pas \
         leurs valeurs longues — {l1:?} ; {l2:?}"
    );

    let ecrivain = ObrainDB::open(&base).expect("écrivain");
    let e2 = bilan(&ecrivain, 2);
    assert_eq!(
        e2.longs_exacts, N as usize,
        "un écrivain qui rouvre rejoue la fenêtre : {e2:?}"
    );
}

/// LIMITE v0.0.1, verrouillée : une valeur au-delà de `BLOB_WAL_MAX_BYTES`
/// (32 Mio) est écrite dans sa colonne mais PAS journalisée ; elle ne survit
/// pas à un `kill -9` avant le prochain flush, alors qu'une valeur de 1 Mio,
/// elle, survit. Ce test rougit le jour où le plafond saute (à mettre alors à
/// jour avec la ligne de `docs/LIMITES-v0.0.1.md`).
#[test]
fn limite_v0_0_1_une_valeur_au_dela_du_plafond_wal_n_est_pas_journalisee() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join("base");
    let enfant = lancer_enfant_mode(1, false, 3, dir.path());
    attendre(&dir.path().join("pret1"));
    tuer(enfant);

    let db = ObrainDB::open(&base).expect("réouverture");
    let r = db
        .session()
        .execute("MATCH (n:Gros) RETURN n.nom, size(n.doc)")
        .expect("relecture");
    let mut tailles = std::collections::HashMap::new();
    for ligne in &r.rows {
        if let Some(Value::String(nom)) = ligne.first() {
            tailles.insert(nom.to_string(), ligne.get(1).cloned());
        }
    }
    assert_eq!(
        r.rows.len(),
        2,
        "les deux nœuds survivent (WAL) : {tailles:?}"
    );
    assert_eq!(
        tailles.get("sous").cloned().flatten(),
        Some(Value::Int64(1 << 20)),
        "1 Mio sous le plafond : journalisée, relue entière"
    );
    assert!(
        !matches!(
            tailles.get("dessus").cloned().flatten(),
            Some(Value::Int64(_))
        ),
        "limite mesurée : au-delà du plafond, la valeur n'est pas journalisée et ne \
         survit pas au kill — relu {:?}",
        tailles.get("dessus")
    );
}

/// CONTAMINATION APRÈS UNE ÉCRITURE DE COLONNE SANS TRAME (relecture de #123) :
/// tour 1 fermé proprement ; tour 2 réécrit t1-k0 avec une valeur au-delà du
/// plafond WAL (colonne écrite, aucune trame) et est tué ; tour 3 crée 40 nœuds
/// à valeurs longues et est tué. La fin d'arène de l'en-tête est celle du tour
/// 1 : sans le recalcul à l'ouverture de l'écrivain, le tour 3 ajoutait par-
/// dessus les octets du tour 2, et t1-k0 relisait la valeur d'un nœud du tour 3.
/// Attendu : t1-k0 relit SA valeur de tour 1 (la réécriture non journalisée est
/// perdue : limite documentée) OU la valeur énorme entière, jamais autre chose.
#[test]
fn une_ecriture_de_colonne_sans_trame_puis_un_ajout_ne_contaminent_aucun_noeud() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join("base");
    let mut enfant = lancer_enfant(1, true, dir.path());
    assert!(
        enfant.wait().expect("attente").success(),
        "tour 1 (close propre)"
    );
    attendre(&dir.path().join("pret1"));
    let enfant = lancer_enfant_mode(2, false, 4, dir.path());
    attendre(&dir.path().join("pret2"));
    tuer(enfant);
    let enfant = lancer_enfant(3, false, dir.path());
    attendre(&dir.path().join("pret3"));
    tuer(enfant);

    let db = ObrainDB::open(&base).expect("réouverture");
    let r = db
        .session()
        .execute("MATCH (n:Doc) WHERE n.tour = 1 AND n.k = 0 RETURN n.doc")
        .expect("relecture de t1-k0");
    let relu = match r.rows.first().and_then(|l| l.first()) {
        Some(Value::String(d)) => d.to_string(),
        autre => panic!("t1-k0 : valeur inattendue {autre:?}"),
    };
    let enorme_entiere =
        relu.len() == obrain_substrate::BLOB_WAL_MAX_BYTES + 1 && relu.bytes().all(|b| b == b'E');
    assert!(
        relu == valeur_longue(1, 0) || enorme_entiere,
        "t1-k0 relit ni sa valeur ni la réécriture entière, mais {} o commençant par {:?} \
         (contamination)",
        relu.len(),
        &relu[..relu.len().min(24)]
    );
    // Aucun autre nœud n'est touché.
    let b1 = bilan(&db, 1);
    let b3 = bilan(&db, 3);
    let faux: Vec<&String> = b1
        .longs_faux
        .iter()
        .filter(|f| !f.contains("t1-k0 "))
        .chain(&b3.longs_faux)
        .collect();
    assert!(faux.is_empty(), "autres nœuds contaminés : {faux:?}");
    assert_eq!(b3.longs_exacts, N as usize, "tour 3 : {b3:?}");
}
