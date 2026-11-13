//! obrain#120 (commentaire du 11/11, 21:10) — ÉCRIVAIN TUÉ, BASE RELUE EN
//! LECTURE SEULE : valeurs longues (colonne blob) ET vecteurs.
//!
//! Scénario du ticket : un écrivain écrit des nœuds portant une propriété texte
//! longue `doc` (> 256 o, donc colonne blob) et une propriété vectorielle
//! `emb`, un commit acquitté par nœud, puis il est tué par `kill -9` sans
//! `close()` ni flush complet. Un lecteur ouvre ensuite la base par
//! `open_read_only`. Mesuré au relevé du ticket : 30/39 pour le texte
//! (`MATCH (n) WHERE n.doc CONTAINS 'aiguille'`), 28/39 pour le vecteur.
//!
//! MESURÉ sur main 9802e661 (13/11/2026, macOS), base neuve, 39 acquittés :
//!   - lecteur seul : texte **0/39**, vecteur **0/39**, aucune valeur fausse ;
//!   - écrivain qui rouvre : texte 39/39, vecteur **0/39** ;
//!   - par intermittence, le lecteur voit en plus un nœud fantôme (ci-dessous).
//!
//! Base fermée proprement après 100 nœuds, puis 39 acquittés et tués :
//! lecteur 100/139 textes et 100/139 vecteurs, écrivain 139/139 textes et
//! 100/139 vecteurs.
//!
//! CAUSE DU TEXTE (corrigée) : le rejeu des trames `BlobColumnWrite` /
//! `BlobColumnClear` (`merge_wal_inline_props`) était réservé à l'écrivain
//! EXCLUSIF (`!est_ecriture_partagee()`), et le lecteur porte un verrou partagé.
//! Or ses zones sont PRIVÉES (copy-on-write) : il peut rejouer sans rien écrire
//! sur disque, ce qu'il fait déjà pour les zones nœuds et propriétés.
//!
//! CAUSE DU NŒUD FANTÔME (corrigée) : intermittent, 4 exécutions sur 15 sur
//! main. Le lecteur voyait un `:Doc` de plus que l'écrivain rouvrant : le nœud
//! dont l'`INSERT` était en vol au SIGKILL (déclaré `TxNoeudEnAttente`, sans
//! `TxCommit`), sans propriétés ou complet. L'écrivain qui rouvre l'écarte
//! (`replay_from`) ; le lecteur rejouait par `replay_from_lecteur` (#60), qui ne
//! l'écarte pas, choix fait quand ses zones étaient partagées. Elles sont
//! privées depuis #115 : il l'écarte désormais lui aussi.
//!
//! CAUSE DU VECTEUR (NON corrigée ici, limite verrouillée) : le WAL ne porte
//! qu'un marqueur `VecColumnWrite` (entité, slot, clé), PAS le vecteur. La
//! colonne (`substrate.veccol.*`) reçoit les octets dans son mmap, mais son
//! en-tête (`n_slots`, CRC) n'est réécrit qu'au `sync` d'un flush complet, et la
//! spec de la colonne n'entre dans le dictionnaire qu'à ce flush. Mesuré après
//! le kill : base neuve, les 39 vecteurs sont dans le fichier, l'en-tête est nul
//! (magic 0, `n_slots` 0) et `substrate.dict` n'existe pas ; base flushée à 100
//! nœuds, l'en-tête dit toujours `n_slots = 101`. Tout slot au-delà se relit
//! absent, par le lecteur comme par l'écrivain. Corriger demande de journaliser
//! le vecteur (nouvelle trame, ~4 o × dimension par écriture, y compris sur le
//! chemin en lot `ecrire_vecteurs_en_lot`) : changement de format et coût
//! d'écriture à mesurer, hors de cette PR.
//!
//! Oracle : le lecteur voit EXACTEMENT les nœuds acquittés (plus, au plus, le
//! nœud en vol dont le commit a pu aboutir sans que l'acquittement ait été
//! imprimé avant le SIGKILL), chacun avec SA valeur longue octet pour octet, et
//! aucune valeur fausse (texte ou vecteur). Le lecteur ne modifie aucun octet
//! de la base. La même base rouverte ensuite en écriture voit la même chose.
//!
//! Protocole repris de `durabilite_commit_kill9.rs` / `blob_hors_wal_kill9.rs` :
//! le parent relance SON propre binaire de test sur le cas enfant (`--exact`),
//! répertoire passé par variable d'environnement. L'enfant écrit sans fin, un
//! `INSERT` (commit implicite) par nœud, et imprime `ACK <k>` sur stdout après
//! chaque commit acquitté ; le parent lit stdout, tue l'enfant par SIGKILL dès
//! le `N`-ième acquittement, puis lit le reste de stdout jusqu'à EOF.
#![cfg(feature = "wal")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write as _};
use std::path::Path;

/// Acquittements attendus avant le SIGKILL (le ticket : 39 nœuds).
const N: usize = 39;
/// Nœuds écrits et fermés proprement avant l'écrivain tué (cas « base
/// flushée »).
const AVANT: usize = 100;
/// Longueur des valeurs longues (bien au-dessus du seuil blob de 256 o).
const LONGUEUR: usize = 585;
/// Dimension des vecteurs.
const D: usize = 16;
/// Variable de l'enfant : `<premier k>:<répertoire>`.
const ENV_ENFANT: &str = "OBRAIN_TEST_RO_KILL9_BLOB_VEC_ENFANT";

/// Valeur longue propre au nœud `k` : contient `aiguille` et l'identité du
/// nœud, répétée, pour qu'une valeur relue sur le mauvais nœud se voie.
fn valeur_longue(k: usize) -> String {
    let motif = format!("[aiguille-k{k:04}]");
    let mut s = String::with_capacity(LONGUEUR);
    while s.len() + motif.len() <= LONGUEUR {
        s.push_str(&motif);
    }
    while s.len() < LONGUEUR {
        s.push('.');
    }
    s
}

/// Vecteur propre au nœud `k` (valeurs exactement représentables en f32 et
/// dans le littéral GQL).
fn vecteur(k: usize) -> Vec<f32> {
    (0..D).map(|i| (k * 100 + i) as f32 + 0.5).collect()
}

fn insert(k: usize) -> String {
    let v: Vec<String> = vecteur(k).iter().map(|x| format!("{x:.1}")).collect();
    format!(
        "INSERT (:Doc {{k: {k}, court: 'c{k}', doc: '{}', emb: vector([{}])}})",
        valeur_longue(k),
        v.join(", ")
    )
}

/// Écrit sans fin des nœuds `:Doc {k, court, doc, emb}` à partir du premier
/// `k`, un commit par nœud, et imprime `ACK k` après chaque acquittement. Ne
/// s'arrête qu'au SIGKILL.
#[test]
fn enfant_ecrivain() {
    let Ok(spec) = std::env::var(ENV_ENFANT) else {
        return;
    };
    let (premier, dir) = spec.split_once(':').unwrap();
    let premier: usize = premier.parse().unwrap();
    let db = ObrainDB::open(Path::new(dir).join("base")).expect("open enfant");
    let s = db.session();
    let mut sortie = std::io::stdout().lock();
    for k in premier.. {
        s.execute(&insert(k)).expect("insert acquitté");
        // Ligne neuve d'abord : libtest a pu laisser `test enfant_ecrivain ... `
        // sans retour à la ligne.
        writeln!(sortie, "\nACK {k}").unwrap();
        sortie.flush().unwrap();
    }
}

/// Lance l'enfant à partir de `premier`, le tue par SIGKILL au `N`-ième
/// acquittement, et rend le nombre d'acquittements imprimés (≥ `N`).
fn ecrire_puis_tuer(dir: &Path, premier: usize) -> usize {
    let mut enfant = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["enfant_ecrivain", "--exact", "--nocapture"])
        .env(ENV_ENFANT, format!("{premier}:{}", dir.display()))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn enfant");
    let mut lignes = BufReader::new(enfant.stdout.take().unwrap()).lines();
    let mut acks = Vec::new();
    while acks.len() < N {
        let ligne = lignes
            .next()
            .expect("l'enfant s'est arrêté avant N acquittements")
            .expect("lecture stdout");
        if let Some(k) = ligne.strip_prefix("ACK ") {
            acks.push(k.parse::<usize>().unwrap());
        }
    }
    // `Child::kill` envoie SIGKILL sous Unix : ni drop, ni flush, ni close.
    enfant.kill().expect("kill -9");
    let _ = enfant.wait();
    // Acquittements imprimés entre le N-ième et la mort.
    for ligne in lignes.map_while(Result::ok) {
        if let Some(k) = ligne.strip_prefix("ACK ") {
            acks.push(k.parse::<usize>().unwrap());
        }
    }
    assert_eq!(
        acks,
        (premier..premier + acks.len()).collect::<Vec<_>>(),
        "acquittements dans l'ordre"
    );
    acks.len()
}

/// Écrit `AVANT` nœuds puis `close()` propre (flush complet).
fn base_flushee(base: &Path) {
    let db = ObrainDB::open(base).expect("open");
    let s = db.session();
    for k in 0..AVANT {
        s.execute(&insert(k)).expect("insert");
    }
    drop(s);
    db.close().expect("close");
}

/// Ce qu'un lecteur voit de la base.
#[derive(Debug, Default, PartialEq)]
struct Bilan {
    /// Nœuds `:Doc` présents (témoin WAL : `k` et `court`).
    noeuds: usize,
    /// `MATCH (n) WHERE n.doc CONTAINS 'aiguille'` (requête du ticket).
    aiguilles: usize,
    /// Nœuds dont `doc` est exactement la valeur attendue.
    docs_exacts: usize,
    docs_absents: usize,
    /// Nœuds dont `emb` est exactement le vecteur attendu.
    vecs_exacts: usize,
    vecs_absents: usize,
    /// Valeurs FAUSSES (doc ou vecteur d'un autre nœud, tronqué, mélangé).
    faux: Vec<String>,
    /// Les `k` vus, triés.
    ks: Vec<usize>,
    /// Les `k` dont le vecteur est relu exact, triés.
    ks_vec: Vec<usize>,
}

fn bilan(db: &ObrainDB) -> Bilan {
    let session = db.session();
    let resultat = session
        .execute("MATCH (n:Doc) RETURN n.k, n.court, n.doc, n.emb")
        .expect("relecture");
    let mut bilan = Bilan::default();
    for ligne in &resultat.rows {
        bilan.noeuds += 1;
        let k = match ligne.first() {
            Some(Value::Int64(k)) => *k as usize,
            _ => {
                // Nœud sans `k` : un nœud dont l'écriture n'a pas été
                // commitée (en vol au SIGKILL) et qui ne devrait pas exister.
                bilan.faux.push(format!(
                    "nœud sans k : {}",
                    format!("{ligne:?}").chars().take(120).collect::<String>()
                ));
                continue;
            }
        };
        bilan.ks.push(k);
        match ligne.get(1) {
            Some(Value::String(court)) if court.as_str() == format!("c{k}") => {}
            autre => bilan.faux.push(format!("k{k} court : {autre:?}")),
        }
        match ligne.get(2) {
            Some(Value::String(doc)) if doc.as_bytes() == valeur_longue(k).as_bytes() => {
                bilan.docs_exacts += 1;
            }
            Some(Value::String(doc)) => bilan.faux.push(format!(
                "k{k} doc : {} o, commence par {:?}",
                doc.len(),
                &doc.as_str()[..doc.len().min(24)]
            )),
            None | Some(Value::Null) => bilan.docs_absents += 1,
            autre => bilan.faux.push(format!("k{k} doc : {autre:?}")),
        }
        match ligne.get(3) {
            Some(Value::Vector(v))
                if v.len() == D
                    && v.iter()
                        .zip(vecteur(k))
                        .all(|(lu, attendu)| lu.to_bits() == attendu.to_bits()) =>
            {
                bilan.vecs_exacts += 1;
                bilan.ks_vec.push(k);
            }
            None | Some(Value::Null) => bilan.vecs_absents += 1,
            autre => bilan.faux.push(format!(
                "k{k} emb : {}",
                format!("{autre:?}").chars().take(60).collect::<String>()
            )),
        }
    }
    bilan.ks.sort_unstable();
    bilan.ks_vec.sort_unstable();
    let resultat = session
        .execute("MATCH (n) WHERE n.doc CONTAINS 'aiguille' RETURN count(n)")
        .expect("requête du ticket");
    bilan.aiguilles = match resultat.rows.first().and_then(|l| l.first()) {
        Some(Value::Int64(n)) => *n as usize,
        autre => panic!("count inattendu : {autre:?}"),
    };
    bilan
}

/// L'oracle. `flushes` nœuds fermés proprement, puis `acks` acquittés par
/// l'écrivain tué : la base porte exactement `flushes + acks` nœuds (plus au
/// plus le nœud en vol, qui doit alors être exact lui aussi), chacun avec SA
/// valeur longue, rien de faux. Vecteurs : ceux des nœuds flushés exacts,
/// ceux de la fenêtre tuée ABSENTS (limite verrouillée, voir l'en-tête).
fn verifier(qui: &str, b: &Bilan, flushes: usize, acks: usize) {
    assert!(
        b.faux.is_empty(),
        "{qui} : valeurs FAUSSES {:?} — {b:?}",
        b.faux
    );
    let vus = b.noeuds;
    assert!(
        vus == flushes + acks || vus == flushes + acks + 1,
        "{qui} : {vus} nœuds vus pour {flushes} flushés + {acks} acquittés — {b:?}"
    );
    assert_eq!(b.ks, (0..vus).collect::<Vec<_>>(), "{qui} : nœuds vus");
    assert_eq!(
        (b.aiguilles, b.docs_exacts),
        (vus, vus),
        "obrain#120 {qui} après kill -9 : {flushes} flushés + {acks} acquittés ; texte {}/{vus} \
         exacts (CONTAINS 'aiguille' : {}), {} absents — {b:?}",
        b.docs_exacts,
        b.aiguilles,
        b.docs_absents
    );
    assert_eq!(
        b.ks_vec,
        (0..flushes).collect::<Vec<_>>(),
        "LIMITE verrouillée (vecteur hors WAL) — {qui} : attendu les {flushes} vecteurs flushés \
         exacts et les {} de la fenêtre tuée absents ; relu {}/{vus} exacts, {} absents. Si \
         les vecteurs de la fenêtre tuée sont désormais relus, la limite est levée : mettre à \
         jour ce test et `docs/LIMITES-v0.0.1.md`. — {b:?}",
        vus - flushes,
        b.vecs_exacts,
        b.vecs_absents
    );
}

/// Empreinte de tous les fichiers de la base (nom → octets), pour vérifier que
/// le lecteur, qui rejoue désormais les valeurs longues, n'écrit rien.
fn empreinte(base: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(base)
        .expect("read_dir")
        .map(|e| {
            let e = e.unwrap();
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect()
}

/// Ouvre en lecture seule, rend le bilan, et vérifie qu'aucun octet de la base
/// n'a changé (ni pendant, ni après la fermeture du lecteur).
fn lire_seul(base: &Path) -> Bilan {
    let avant = empreinte(base);
    let lecteur = ObrainDB::open_read_only(base).expect("lecteur");
    let b = bilan(&lecteur);
    drop(lecteur);
    let apres = empreinte(base);
    let changes: Vec<&String> = avant
        .keys()
        .chain(apres.keys())
        .filter(|n| avant.get(*n) != apres.get(*n))
        .collect();
    assert!(
        changes.is_empty(),
        "le lecteur seul a modifié la base : {changes:?}"
    );
    b
}

/// Cas du ticket : base neuve, écrivain tué, relue par `open_read_only`.
#[test]
fn un_lecteur_seul_voit_exactement_les_valeurs_longues_acquittees_apres_kill_9() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let acks = ecrire_puis_tuer(dir.path(), 0);
    let b = lire_seul(&dir.path().join("base"));
    verifier("lecteur seul", &b, 0, acks);
}

/// Même base, lue d'abord par `open_read_only` puis rouverte en écriture : les
/// deux lectures satisfont l'oracle et sont identiques.
#[test]
fn lecteur_seul_puis_ecrivain_rouvrant_voient_la_meme_chose_apres_kill_9() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join("base");
    let acks = ecrire_puis_tuer(dir.path(), 0);
    let l = lire_seul(&base);
    let ecrivain = ObrainDB::open(&base).expect("écrivain");
    let e = bilan(&ecrivain);
    drop(ecrivain);
    verifier("écrivain rouvrant", &e, 0, acks);
    verifier("lecteur seul", &l, 0, acks);
    assert_eq!(l, e, "le lecteur seul et l'écrivain rouvrant divergent");
}

/// Base déjà flushée (colonnes et dictionnaire durables), puis écrivain tué :
/// lecteur seul, puis écrivain rouvrant, comparés.
#[test]
fn sur_une_base_flushee_lecteur_seul_et_ecrivain_rouvrant_voient_la_meme_chose_apres_kill_9() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join("base");
    base_flushee(&base);
    let acks = ecrire_puis_tuer(dir.path(), AVANT);
    let l = lire_seul(&base);
    let ecrivain = ObrainDB::open(&base).expect("écrivain");
    let e = bilan(&ecrivain);
    drop(ecrivain);
    verifier("écrivain rouvrant (base flushée)", &e, AVANT, acks);
    verifier("lecteur seul (base flushée)", &l, AVANT, acks);
    assert_eq!(l, e, "le lecteur seul et l'écrivain rouvrant divergent");
}

/// TÉMOIN (sans kill) : les mêmes écritures sont relues exactes, vecteurs
/// compris, par l'écrivain vivant, puis après un `close()` propre par un
/// lecteur seul et par un écrivain. Valide l'écriture (littéral `vector(...)`,
/// valeur longue) et la lecture du bilan : la perte des vecteurs mesurée plus
/// haut tient au kill, pas au test.
#[test]
fn temoin_sans_kill_les_memes_ecritures_sont_relues_exactes() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join("base");
    let db = ObrainDB::open(&base).expect("open");
    let s = db.session();
    for k in 0..N {
        s.execute(&insert(k)).expect("insert");
    }
    drop(s);
    let tout_exact = |qui: &str, b: &Bilan| {
        assert!(b.faux.is_empty(), "{qui} : {b:?}");
        assert_eq!(
            (b.noeuds, b.aiguilles, b.docs_exacts, b.vecs_exacts),
            (N, N, N, N),
            "{qui} : {b:?}"
        );
    };
    tout_exact("écrivain vivant", &bilan(&db));
    db.close().expect("close");
    tout_exact("lecteur seul après close", &lire_seul(&base));
    let ecrivain = ObrainDB::open(&base).expect("écrivain");
    tout_exact("écrivain après close", &bilan(&ecrivain));
}
