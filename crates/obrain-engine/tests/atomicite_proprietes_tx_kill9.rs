//! obrain#164 — ATOMICITÉ DES PROPRIÉTÉS : un `SET` / `REMOVE` sur un nœud
//! EXISTANT, fait dans une transaction explicite jamais commitée, ne doit pas
//! réapparaître après un `kill -9`.
//!
//! Constat de l'issue (déduit de la lecture du code) : le rejeu n'écarte les
//! transactions non scellées que pour les CRÉATIONS (registre `en_vol` de
//! `replay_from`, alimenté par `TxNoeudEnAttente` / `TxAreteEnAttente`). Les
//! trames de propriétés (`PropSetInline`, `PropRemoveInline`,
//! `BlobColumnWrite` / `BlobColumnClear`, `VecColumnSet`) ne portent pas
//! d'identifiant de transaction.
//!
//! Scénario : un nœud `:Cible` est créé et COMMITÉ avec une valeur courte
//! (`court`), une valeur longue > 256 o (`doc`, colonne blob), un vecteur
//! (`emb`, colonne de vecteurs) et une quatrième propriété (`tag`). Une
//! transaction explicite fait `SET` des trois premières à d'autres valeurs et
//! `REMOVE` de la quatrième, puis l'enfant est tué par SIGKILL.
//!
//! Oracle : après réouverture, pour le lecteur `open_read_only` PUIS pour
//! l'écrivain qui rouvre, chaque valeur est EXACTEMENT celle d'avant la
//! transaction. Témoin : la même transaction commitée avant le kill laisse les
//! nouvelles valeurs. Variantes : `rollback()` explicite avant le kill ; base
//! flushée (`close()`) avant la transaction ; commit d'une AUTRE session après
//! les `SET` non commités (son `fsync` emporte les trames de la transaction
//! ouverte, qui précèdent la sienne dans le WAL).
//!
//! MESURÉ sur main 8d7e3b09 (13/11/2026, macOS), 20 exécutions, features par
//! défaut et `--all-features` : ROUGE 20/20 pour chaque cas non commité (`kill`
//! et `tiers`, base neuve et flushée) — les quatre valeurs relues sont celles
//! de la transaction, pour le lecteur comme pour l'écrivain ; témoins `commit`
//! et `rollback` verts 20/20.
//!
//! CAUSE (mesurée) : pas le rejeu des trames. WAL tronqué à sa taille d'avant la
//! transaction (2 039 → 1 238 o, base flushée), l'écrivain qui rouvre sert
//! encore les quatre valeurs non commitées : un `SET` est écrit EN PLACE (chaîne
//! v2, colonne blob, colonne de vecteurs, pages mmap qui survivent au processus)
//! et le journal d'annulation (`tx_undo`) ne vivait qu'en mémoire.
//!
//! CORRECTION : la valeur d'avant part au WAL avant l'écriture
//! (`TxProprieteAvant`, 0x59), le `rollback` clôt la transaction (`TxAnnulee`,
//! 0x5A), et l'ouverture restaure les propriétés d'une transaction ni commitée
//! ni close (`SubstrateStore::annuler_proprietes_en_vol`).
//!
//! L'enfant vérifie, avant de poser son marqueur, ce qu'il voit lui-même (les
//! nouvelles valeurs dans sa transaction, ou les anciennes après `rollback`) :
//! un échec là n'est pas un défaut de rejeu, et il le signale par un code de
//! sortie au lieu du marqueur.
//!
//! Protocole de `durabilite_commit_kill9.rs` : le parent relance SON binaire de
//! test sur le cas enfant (`--exact`), la spec en variable d'environnement.
#![cfg(feature = "wal")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

/// Variable de l'enfant : `<cas>:<flushee 0|1>:<répertoire>`.
const ENV_ENFANT: &str = "OBRAIN_TEST_ATOMICITE_PROPRIETES_ENFANT";
/// Longueur des valeurs longues (bien au-dessus du seuil blob de 256 o).
const LONGUEUR: usize = 585;
/// Dimension du vecteur.
const D: usize = 16;

fn valeur_longue(motif: &str) -> String {
    let motif = format!("[{motif}]");
    let mut s = String::with_capacity(LONGUEUR);
    while s.len() + motif.len() <= LONGUEUR {
        s.push_str(&motif);
    }
    while s.len() < LONGUEUR {
        s.push('.');
    }
    s
}

fn vecteur(base: usize) -> Vec<f32> {
    (0..D).map(|i| (base * 100 + i) as f32 + 0.5).collect()
}

fn litteral_vecteur(v: &[f32]) -> String {
    let v: Vec<String> = v.iter().map(|x| format!("{x:.1}")).collect();
    format!("vector([{}])", v.join(", "))
}

/// Ce qu'on lit du nœud `:Cible`.
#[derive(Debug, Clone, PartialEq)]
struct Etat {
    court: Option<String>,
    doc: Option<String>,
    /// Bits exacts des composantes.
    emb: Option<Vec<u32>>,
    tag: Option<String>,
}

fn etat_avant() -> Etat {
    Etat {
        court: Some("ancien".into()),
        doc: Some(valeur_longue("ancienne-valeur-longue")),
        emb: Some(vecteur(1).iter().map(|x| x.to_bits()).collect()),
        tag: Some("a-retirer".into()),
    }
}

fn etat_apres() -> Etat {
    Etat {
        court: Some("nouveau".into()),
        doc: Some(valeur_longue("NOUVELLE-VALEUR-LONGUE")),
        emb: Some(vecteur(2).iter().map(|x| x.to_bits()).collect()),
        tag: None,
    }
}

/// Valeurs écrites et commitées APRÈS la restauration (cas `reprise`).
fn etat_reprise() -> Etat {
    Etat {
        court: Some("repris".into()),
        doc: Some(valeur_longue("valeur-longue-reprise")),
        emb: Some(vecteur(3).iter().map(|x| x.to_bits()).collect()),
        tag: Some("remis".into()),
    }
}

fn creer_cible(db: &ObrainDB) {
    let a = etat_avant();
    db.session()
        .execute(&format!(
            "INSERT (:Cible {{court: '{}', doc: '{}', emb: {}, tag: '{}'}})",
            a.court.unwrap(),
            a.doc.unwrap(),
            litteral_vecteur(&vecteur(1)),
            a.tag.unwrap()
        ))
        .expect("insert cible");
}

/// Lit l'unique `:Cible` (panique s'il n'y en a pas exactement un).
fn lire(db: &ObrainDB) -> Etat {
    let r = db
        .session()
        .execute("MATCH (n:Cible) RETURN n.court, n.doc, n.emb, n.tag")
        .expect("relecture");
    assert_eq!(
        r.rows.len(),
        1,
        "exactement un :Cible attendu : {:?}",
        r.rows
    );
    lire_ligne(&r.rows[0])
}

fn lire_ligne(l: &[Value]) -> Etat {
    let texte = |v: Option<&Value>| match v {
        Some(Value::String(s)) => Some(s.as_str().to_string()),
        None | Some(Value::Null) => None,
        autre => Some(format!("<inattendu {autre:?}>")),
    };
    Etat {
        court: texte(l.first()),
        doc: texte(l.get(1)),
        emb: match l.get(2) {
            Some(Value::Vector(v)) => Some(v.iter().map(|x| x.to_bits()).collect()),
            None | Some(Value::Null) => None,
            autre => Some(vec![u32::MAX, format!("{autre:?}").len() as u32]),
        },
        tag: texte(l.get(3)),
    }
}

/// Pour un message d'échec lisible : quelles propriétés diffèrent, et de quoi.
fn ecarts(lu: &Etat, attendu: &Etat) -> Vec<String> {
    let court = |o: &Option<String>| {
        o.as_ref().map_or_else(
            || "<absent>".into(),
            |s| s.chars().take(28).collect::<String>(),
        )
    };
    let mut out = Vec::new();
    if lu.court != attendu.court {
        out.push(format!(
            "court : lu {} attendu {}",
            court(&lu.court),
            court(&attendu.court)
        ));
    }
    if lu.doc != attendu.doc {
        out.push(format!(
            "doc (blob) : lu {} attendu {}",
            court(&lu.doc),
            court(&attendu.doc)
        ));
    }
    if lu.emb != attendu.emb {
        let premier = |o: &Option<Vec<u32>>| {
            o.as_ref()
                .and_then(|v| v.first().map(|b| f32::from_bits(*b).to_string()))
                .unwrap_or_else(|| "<absent>".into())
        };
        out.push(format!(
            "emb (vecteur) : lu [{}, ..] attendu [{}, ..]",
            premier(&lu.emb),
            premier(&attendu.emb)
        ));
    }
    if lu.tag != attendu.tag {
        out.push(format!(
            "tag (REMOVE) : lu {} attendu {}",
            court(&lu.tag),
            court(&attendu.tag)
        ));
    }
    out
}

/// Écrit `etat_reprise()` par `ObrainDB::set_node_property`, hors de toute
/// transaction.
fn ecrire_hors_transaction(db: &ObrainDB) {
    let r = etat_reprise();
    let id = match db
        .session()
        .execute("MATCH (n:Cible) RETURN id(n)")
        .expect("id")
        .rows
        .first()
        .and_then(|l| l.first())
    {
        Some(Value::Int64(i)) => obrain_common::types::NodeId(*i as u64),
        autre => panic!("id inattendu : {autre:?}"),
    };
    db.set_node_property(id, "court", Value::from(r.court.unwrap().as_str()));
    db.set_node_property(id, "doc", Value::from(r.doc.unwrap().as_str()));
    db.set_node_property(id, "emb", Value::Vector(vecteur(3).into()));
    db.set_node_property(id, "tag", Value::from(r.tag.unwrap().as_str()));
}

fn dormir_pour_toujours() -> ! {
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

/// Enfant. `cas` : `kill` (transaction laissée ouverte), `rollback`
/// (`rollback()` explicite), `commit` (témoin), `tiers` (transaction ouverte,
/// puis commit acquitté d'une autre session), `hors_tx` (transaction ouverte,
/// puis écritures hors transaction sur les mêmes clés), `checkpoint` (comme `tiers`,
/// checkpoint automatique à chaque commit), `rollback_puis_ecriture`
/// (`rollback()` puis écritures hors transaction), `reprise` (rouvre une base
/// laissée par `kill`, vérifie la restauration, puis écrit de nouvelles
/// valeurs hors transaction). Pose `pret` puis dort.
#[test]
fn enfant_transaction() {
    let Ok(spec) = std::env::var(ENV_ENFANT) else {
        return;
    };
    let mut it = spec.splitn(3, ':');
    let cas = it.next().unwrap().to_string();
    let flushee = it.next().unwrap() == "1";
    let dir = Path::new(it.next().unwrap()).to_path_buf();
    let db = ObrainDB::open(dir.join("base")).expect("open enfant");
    if cas == "reprise" {
        // La restauration a eu lieu à l'ouverture ; ce qui est commité ensuite
        // ne doit pas être écrasé par une seconde restauration.
        if lire(&db) != etat_avant() {
            std::process::exit(8);
        }
        // Écritures HORS transaction : elles ne déclarent aucune valeur
        // d'avant, et seul le `TxAnnulee` posé par la restauration empêche la
        // suivante de les écraser.
        ecrire_hors_transaction(&db);
        std::fs::write(dir.join("pret"), b"ok").unwrap();
        dormir_pour_toujours();
    }
    if !flushee {
        // Créé et commité (auto-commit) dans CE processus : ni flush ni close.
        creer_cible(&db);
    }
    if lire(&db) != etat_avant() {
        std::process::exit(4);
    }
    let n = etat_apres();
    let mut t = db.session();
    t.begin_transaction().expect("begin");
    t.execute(&format!(
        "MATCH (n:Cible) SET n.court = '{}', n.doc = '{}', n.emb = {}",
        n.court.as_ref().unwrap(),
        n.doc.as_ref().unwrap(),
        litteral_vecteur(&vecteur(2))
    ))
    .expect("SET dans la transaction");
    t.execute("MATCH (n:Cible) REMOVE n.tag")
        .expect("REMOVE dans la transaction");
    // La transaction voit ses propres écritures : le scénario a bien eu lieu.
    let vu = t
        .execute("MATCH (n:Cible) RETURN n.court, n.doc, n.emb, n.tag")
        .expect("lecture dans la transaction");
    if vu.rows.len() != 1 || lire_ligne(&vu.rows[0]) != etat_apres() {
        std::process::exit(5);
    }
    match cas.as_str() {
        "kill" => {}
        "hors_tx" => {
            // Écriture HORS transaction, acquittée APRÈS la déclaration des
            // valeurs d'avant de la transaction ouverte (même entité, mêmes
            // clés) : la restauration ne doit pas la défaire.
            ecrire_hors_transaction(&db);
        }
        "tiers" | "checkpoint" => {
            // Commit acquitté d'une autre session APRÈS les trames non
            // commitées : son fsync les rend durables elles aussi. Pour
            // `checkpoint` (seuil d'un octet, voir `tuer_enfant`), un `flush()`
            // suit et déclenche le checkpoint automatique, qui ne doit pas
            // tronquer les valeurs d'avant de la transaction ouverte.
            db.session()
                .execute("INSERT (:Temoin {k: 1})")
                .expect("commit tiers");
            if cas == "checkpoint" {
                // Le checkpoint automatique ne part que d'un `flush()`.
                db.checkpoint().expect("flush");
            }
        }
        "rollback" | "rollback_puis_ecriture" => {
            t.rollback().expect("rollback");
            if lire(&db) != etat_avant() {
                std::process::exit(6);
            }
            if cas == "rollback_puis_ecriture" {
                // Sans le `TxAnnulee` du rollback, la réouverture restaurerait
                // les valeurs d'avant par-dessus ces écritures.
                ecrire_hors_transaction(&db);
            }
        }
        "commit" => {
            t.commit().expect("commit");
            if lire(&db) != etat_apres() {
                std::process::exit(7);
            }
        }
        autre => panic!("cas inconnu {autre}"),
    }
    std::fs::write(dir.join("pret"), b"ok").unwrap();
    dormir_pour_toujours();
}

/// Lance l'enfant, attend `pret`, le tue par SIGKILL, rend le répertoire.
fn executer(cas: &str, flushee: bool) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join("base");
    if flushee {
        let db = ObrainDB::open(&base).expect("open");
        creer_cible(&db);
        db.close().expect("close");
    }
    tuer_enfant(cas, flushee, dir.path());
    dir
}

/// Lance l'enfant `cas` sur `dir`, attend `pret` (qu'il retire), le tue par
/// SIGKILL.
fn tuer_enfant(cas: &str, flushee: bool, dir: &Path) {
    let _ = std::fs::remove_file(dir.join("pret"));
    let mut commande = std::process::Command::new(std::env::current_exe().unwrap());
    commande
        .args(["enfant_transaction", "--exact", "--nocapture"])
        .env(
            ENV_ENFANT,
            format!("{cas}:{}:{}", u8::from(flushee), dir.display()),
        )
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if cas == "checkpoint" {
        // Checkpoint automatique à chaque commit.
        commande.env("OBRAIN_WAL_CHECKPOINT_BYTES", "1");
    }
    let mut enfant = commande.spawn().expect("spawn enfant");
    let pret = dir.join("pret");
    let mut tours = 0u32;
    while !pret.exists() {
        if let Some(statut) = enfant.try_wait().expect("try_wait") {
            panic!("{cas} : l'enfant est mort avant le marqueur : {statut:?}");
        }
        tours += 1;
        assert!(tours < 1200, "{cas} : l'enfant n'a jamais posé le marqueur");
        std::thread::sleep(Duration::from_millis(50));
    }
    // `Child::kill` envoie SIGKILL sous Unix : ni drop, ni flush, ni close.
    enfant.kill().expect("kill -9");
    let _ = enfant.wait();
}

/// Empreinte de tous les fichiers de la base (chemin relatif → octets), pour
/// vérifier que le lecteur, qui restaure dans ses zones privées, n'écrit rien.
fn empreinte(base: &Path) -> BTreeMap<String, Vec<u8>> {
    fn visiter(racine: &Path, dossier: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(dossier).expect("read_dir") {
            let chemin = e.unwrap().path();
            if chemin.is_dir() {
                visiter(racine, &chemin, out);
            } else {
                let nom = chemin
                    .strip_prefix(racine)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.insert(nom, std::fs::read(&chemin).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    visiter(base, base, &mut out);
    out
}

/// Lecteur `open_read_only`, puis écrivain qui rouvre : tous deux doivent lire
/// exactement `attendu`, et le lecteur ne modifie aucun octet de la base.
fn verifier(cas: &str, flushee: bool, attendu: &Etat) {
    let dir = executer(cas, flushee);
    verifier_base(cas, flushee, dir.path(), attendu);
}

fn verifier_base(cas: &str, flushee: bool, dir: &Path, attendu: &Etat) {
    let base = dir.join("base");
    let avant = empreinte(&base);
    let lecteur = ObrainDB::open_read_only(&base).expect("lecteur");
    let l = lire(&lecteur);
    drop(lecteur);
    let apres = empreinte(&base);
    let changes: Vec<&String> = avant
        .keys()
        .chain(apres.keys())
        .filter(|n| avant.get(*n) != apres.get(*n))
        .collect();
    assert!(
        changes.is_empty(),
        "cas `{cas}` : le lecteur seul a modifié la base : {changes:?}"
    );
    let ecrivain = ObrainDB::open(&base).expect("écrivain");
    let e = lire(&ecrivain);
    drop(ecrivain);
    let el = ecarts(&l, attendu);
    let ee = ecarts(&e, attendu);
    assert!(
        el.is_empty() && ee.is_empty(),
        "obrain#164 cas `{cas}` (base {}) après kill -9 : lecteur seul {el:?} ; écrivain \
         rouvrant {ee:?}",
        if flushee { "flushée" } else { "neuve" }
    );
}

#[test]
fn tx_non_commitee_tuee_laisse_les_anciennes_valeurs() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    verifier("kill", false, &etat_avant());
}

#[test]
fn tx_non_commitee_tuee_sur_base_flushee_laisse_les_anciennes_valeurs() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    verifier("kill", true, &etat_avant());
}

#[test]
fn tx_non_commitee_suivie_d_un_commit_tiers_laisse_les_anciennes_valeurs() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    verifier("tiers", false, &etat_avant());
}

#[test]
fn tx_non_commitee_suivie_d_un_commit_tiers_sur_base_flushee_laisse_les_anciennes_valeurs() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    verifier("tiers", true, &etat_avant());
}

/// Un checkpoint automatique tombé pendant la transaction ouverte ne tronque
/// pas ses valeurs d'avant (borne MP-8, `plus_ancienne_declaration_en_vol`).
#[test]
fn un_checkpoint_pendant_la_tx_non_commitee_ne_perd_pas_les_valeurs_d_avant() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    for flushee in [false, true] {
        verifier("checkpoint", flushee, &etat_avant());
    }
}

#[test]
fn rollback_puis_kill_laisse_les_anciennes_valeurs() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    verifier("rollback", false, &etat_avant());
}

#[test]
fn rollback_puis_kill_sur_base_flushee_laisse_les_anciennes_valeurs() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    verifier("rollback", true, &etat_avant());
}

#[test]
fn temoin_tx_commitee_puis_kill_laisse_les_nouvelles_valeurs() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    verifier("commit", false, &etat_apres());
}

#[test]
fn temoin_tx_commitee_puis_kill_sur_base_flushee_laisse_les_nouvelles_valeurs() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    verifier("commit", true, &etat_apres());
}

/// La restauration faite à l'ouverture est CLOSE (`TxAnnulee`) : des valeurs
/// écrites après elle (hors transaction), puis un second `kill -9`, ne sont pas
/// écrasées par une seconde restauration.
#[test]
fn les_valeurs_ecrites_apres_la_restauration_survivent_a_un_second_kill() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    for flushee in [false, true] {
        let dir = executer("kill", flushee);
        tuer_enfant("reprise", flushee, dir.path());
        verifier_base("reprise", flushee, dir.path(), &etat_reprise());
    }
}

/// `rollback()` clôt la transaction (`TxAnnulee`) : des écritures faites
/// ensuite hors transaction ne sont pas écrasées à la réouverture.
#[test]
fn les_valeurs_ecrites_apres_un_rollback_survivent_a_un_kill() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    for flushee in [false, true] {
        verifier("rollback_puis_ecriture", flushee, &etat_reprise());
    }
}

/// obrain#164 (relecture de #169) : une écriture hors transaction, acquittée
/// pendant qu'une transaction ouverte a déclaré ses valeurs d'avant sur la même
/// (entité, clé), survit au `kill -9`.
#[test]
fn une_ecriture_hors_transaction_acquittee_pendant_une_tx_ouverte_survit_au_kill() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    for flushee in [false, true] {
        verifier("hors_tx", flushee, &etat_reprise());
    }
}
