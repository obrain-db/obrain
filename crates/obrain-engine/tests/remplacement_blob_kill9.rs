//! obrain#172 — UNE VALEUR ACQUITTÉE NE DISPARAÎT JAMAIS : un `kill -9` reçu
//! PENDANT le remplacement d'une valeur blob (> 256 o) laisse l'ancienne valeur
//! ou la nouvelle, jamais aucune des deux.
//!
//! Mesuré sur main `4d3782eb` (test PO `kill9_during_file_writes…`, macOS) : un
//! nœud `File` dont le `hash` long acquitté était remplacé par un `hash` court
//! au moment du kill rouvrait SANS `hash` (quarantaine « File sans propriété
//! `hash` »), 2/20 puis 1/30 exécutions. Mécanisme (`store.rs`,
//! `set_node_property`, branche non-blob) : (1) `effacer_ombre_blob` met à zéro
//! EN PLACE l'entrée d'index de la colonne blob — l'ancienne valeur est
//! illisible aussitôt, et la page mmap survit au processus ; (2) la nouvelle
//! valeur courte n'est écrite dans la zone v2 qu'APRÈS. Un kill entre (1) et (2)
//! laisse le nœud sans valeur. Une écriture hors transaction (le cas de PO) ne
//! déclare aucune valeur d'avant : rien à restaurer à l'ouverture.
//!
//! Le remplacement blob → blob (`route_blob_write`) n'a pas ce trou : l'arène est
//! append-only et l'entrée d'index bascule en un geste de l'ancienne à la
//! nouvelle valeur. Il est mesuré ici aussi, comme témoin.
//!
//! PROTOCOLE : le substrat expose, sous la feature `points-de-crash`
//! (dépendance de test seulement), des points nommés par leur POSITION dans le
//! remplacement (`avant_tout`, `entre_les_deux`, `apres_tout` ; `avant_route`,
//! `apres_route` pour blob → blob). Le parent lance une première fois son propre
//! binaire pour PRÉPARER la base (un nœud, deux valeurs longues, flush complet),
//! puis une seconde fois pour REMPLACER une valeur avec un point de crash armé :
//! l'enfant, arrivé au point, pose un marqueur et dort ; le parent le tue -9,
//! rouvre et lit. Le journal porte l'INTENTION avant le remplacement et le
//! RETOUR après. Oracle : la valeur durable est l'acquittée ou l'intention —
//! jamais rien, jamais autre chose.
//!
//! Le second test tue AU HASARD pendant une boucle de remplacements, comme le
//! test de PO, avec le même oracle : il ne reproduit le défaut qu'à un faible
//! taux (la fenêtre dure quelques microsecondes) et sert de garde, pas de
//! preuve ; la preuve rouge/vert est le premier test.
#![cfg(feature = "wal")]

use obrain_common::types::{NodeId, Value};
use obrain_engine::ObrainDB;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// `<phase>:<répertoire>` — `preparer`, `remplacer_inline`, `remplacer_blob`,
/// `boucle`.
const ENV_ENFANT: &str = "OBRAIN_TEST_REMPLACEMENT_BLOB_ENFANT";
/// Lu par `obrain-substrate` (feature `points-de-crash`) :
/// `<nom du point>:<fichier marqueur>`.
const ENV_POINT: &str = "OBRAIN_SUBSTRATE_POINT_DE_CRASH";

fn long(prefixe: &str, graine: &str) -> String {
    format!("{}{graine}", prefixe.repeat(340))
}

fn durable(db: &ObrainDB) {
    // Le geste de PO : fsync du WAL, puis flush complet (colonnes blob comprises).
    db.flush_wal().expect("flush_wal");
    db.commit_wal().expect("commit_wal");
}

fn noter(dir: &Path, ligne: &str) {
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("journal"))
        .expect("journal");
    // UNE écriture par ligne : un `writeln!` sur un `File` brut fait plusieurs
    // `write` et un kill entre deux laisse une demi-ligne.
    f.write_all(format!("{ligne}\n").as_bytes())
        .expect("journal");
}

/// Hash de la lecture : `Some(valeur)` ou `None` si le nœud n'a pas la clé.
fn relire(base: &Path, cle: &str) -> Option<String> {
    let db = ObrainDB::open(base).expect("réouverture");
    let r = db
        .session()
        .execute(&format!("MATCH (f:File) RETURN f.{cle}"))
        .expect("relecture");
    assert_eq!(
        r.rows.len(),
        1,
        "un seul nœud File attendu, lu {:?}",
        r.rows
    );
    match r.rows[0].first() {
        Some(Value::String(s)) => Some(s.to_string()),
        _ => None,
    }
}

#[test]
fn enfant_remplacement() {
    let Ok(consigne) = std::env::var(ENV_ENFANT) else {
        return;
    };
    let (phase, dir) = consigne.split_once(':').expect("phase:dir");
    let dir = PathBuf::from(dir);
    let base = dir.join("base");
    match phase {
        "preparer" => {
            let db = ObrainDB::open(&base).expect("open");
            let id = db.create_node(&["File"]);
            db.set_node_property(id, "path", Value::String(long("d", "-path-1").into()));
            db.set_node_property(id, "hash", Value::String(long("h", "-hash-1").into()));
            durable(&db);
            std::fs::write(dir.join("noeud"), id.0.to_string()).unwrap();
            noter(&dir, &format!("A\tpath\t{}", long("d", "-path-1")));
            noter(&dir, &format!("A\thash\t{}", long("h", "-hash-1")));
            db.close().expect("close");
        }
        "remplacer_inline" | "remplacer_blob" => {
            let db = ObrainDB::open(&base).expect("open");
            let id = NodeId(
                std::fs::read_to_string(dir.join("noeud"))
                    .unwrap()
                    .parse()
                    .unwrap(),
            );
            let (cle, neuve) = if phase == "remplacer_inline" {
                ("hash", "court-hash-2".to_string())
            } else {
                ("path", long("e", "-path-2"))
            };
            noter(&dir, &format!("I\t{cle}\t{neuve}"));
            // Le point de crash armé par le parent est DANS cet appel.
            db.set_node_property(id, cle, Value::String(neuve.clone().into()));
            durable(&db);
            noter(&dir, &format!("R\t{cle}\t{neuve}"));
        }
        "boucle" => {
            let db = ObrainDB::open(&base).expect("open");
            let mut i = 0u64;
            loop {
                i += 1;
                let path = long("d", &format!("-{i}"));
                let id = db.create_node(&["File"]);
                db.set_node_property(id, "path", Value::String(path.clone().into()));
                db.set_node_property(
                    id,
                    "hash",
                    Value::String(long("h", &format!("-{i}")).into()),
                );
                durable(&db);
                noter(&dir, &format!("A\t{path}\t{}", long("h", &format!("-{i}"))));
                let court = format!("court-{i}");
                noter(&dir, &format!("I\t{path}\t{court}"));
                db.set_node_property(id, "hash", Value::String(court.clone().into()));
                durable(&db);
                noter(&dir, &format!("R\t{path}\t{court}"));
            }
        }
        autre => panic!("phase inconnue {autre:?}"),
    }
}

fn lancer(phase: &str, dir: &Path, point: Option<&str>) -> std::process::Child {
    let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
    cmd.args(["enfant_remplacement", "--exact", "--nocapture"])
        .env(ENV_ENFANT, format!("{phase}:{}", dir.display()))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Some(p) = point {
        cmd.env(ENV_POINT, format!("{p}:{}", dir.join("marqueur").display()));
    }
    cmd.spawn().expect("spawn enfant")
}

fn attendre(fichier: &Path, quoi: &str) {
    for _ in 0..1200 {
        if fichier.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("l'enfant n'a jamais produit {quoi} en 60 s");
}

/// Un `kill -9` à chaque point d'un remplacement blob → inline et blob → blob :
/// la valeur durable est l'ancienne (acquittée) ou la nouvelle (l'intention),
/// jamais absente. La trace imprimée dit, point par point, laquelle.
#[test]
fn un_remplacement_tue_a_chaque_point_laisse_l_ancienne_ou_la_nouvelle_valeur() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    let cas: [(&str, &str, &str); 5] = [
        (
            "remplacer_inline",
            "hash",
            "noeud:remplacement_inline:avant_tout",
        ),
        (
            "remplacer_inline",
            "hash",
            "noeud:remplacement_inline:entre_les_deux",
        ),
        (
            "remplacer_inline",
            "hash",
            "noeud:remplacement_inline:apres_tout",
        ),
        ("remplacer_blob", "path", "noeud:blob:avant_route"),
        ("remplacer_blob", "path", "noeud:blob:apres_route"),
    ];
    let mut perdus: Vec<String> = Vec::new();
    for (phase, cle, point) in cas {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path().join("base");
        let statut = lancer("preparer", dir.path(), None)
            .wait()
            .expect("preparer");
        assert!(
            statut.success(),
            "{point}: la préparation a échoué ({statut:?})"
        );

        let mut enfant = lancer(phase, dir.path(), Some(point));
        attendre(
            &dir.path().join("marqueur"),
            &format!("le marqueur du point {point}"),
        );
        if let Ok(Some(s)) = enfant.try_wait() {
            panic!("{point}: l'enfant s'est arrêté seul ({s:?})");
        }
        enfant.kill().expect("kill -9");
        let _ = enfant.wait();

        // Journal : acquittée (A) et intention (I) pour la clé remplacée.
        let texte = std::fs::read_to_string(dir.path().join("journal")).unwrap();
        let mut acquittee = None;
        let mut intention = None;
        for l in texte.lines() {
            let mut c = l.splitn(3, '\t');
            if let (Some(q), Some(k), Some(v)) = (c.next(), c.next(), c.next())
                && k == cle
            {
                match q {
                    "A" => acquittee = Some(v.to_string()),
                    "I" => intention = Some(v.to_string()),
                    _ => {}
                }
            }
        }
        let acquittee = acquittee.expect("valeur acquittée au journal");
        let intention = intention.expect("intention au journal");
        assert_ne!(acquittee, intention);

        let durable = relire(&base, cle);
        let verdict = match durable.as_deref() {
            Some(v) if v == acquittee => "ANCIENNE (acquittée)",
            Some(v) if v == intention => "NOUVELLE (intention)",
            Some(_) => "AUTRE",
            None => "AUCUNE",
        };
        eprintln!(
            "{point}: après kill -9, {cle} durable = {verdict}{}",
            durable
                .as_deref()
                .map(|v| format!(" [{}…, {} o]", &v[..v.len().min(12)], v.len()))
                .unwrap_or_default()
        );
        if verdict == "AUCUNE" || verdict == "AUTRE" {
            perdus.push(format!("{point}: {verdict}"));
        }
        // Les points extrêmes sont des témoins du protocole : avant tout geste
        // l'ancienne est là ; après tous, la nouvelle.
        if point.ends_with("avant_tout") || point.ends_with("avant_route") {
            assert_eq!(durable.as_deref(), Some(acquittee.as_str()), "{point}");
        }
        if point.ends_with("apres_tout") || point.ends_with("apres_route") {
            assert_eq!(durable.as_deref(), Some(intention.as_str()), "{point}");
        }
    }
    assert!(
        perdus.is_empty(),
        "une valeur acquittée a disparu : la valeur durable n'est ni l'acquittée ni \
         l'intention : {perdus:?}"
    );
}

/// Garde non déterministe, protocole de PO : l'enfant enchaîne création longue
/// puis remplacement court, le parent tue au hasard après un nombre IMPAIR de
/// retours (le geste en vol est alors un remplacement). Même oracle.
#[test]
fn des_remplacements_tues_au_hasard_ne_perdent_aucune_valeur_acquittee() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    const TOURS: u64 = 6;
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join("base");
    let journal = dir.path().join("journal");
    let lignes = || -> Vec<(String, String, String)> {
        let s = std::fs::read_to_string(&journal).unwrap_or_default();
        s.split_inclusive('\n')
            .filter_map(|l| l.strip_suffix('\n'))
            .filter_map(|l| {
                let mut c = l.splitn(3, '\t');
                Some((
                    c.next()?.to_string(),
                    c.next()?.to_string(),
                    c.next()?.to_string(),
                ))
            })
            .collect()
    };
    let retours = || {
        lignes()
            .iter()
            .filter(|(q, ..)| q == "R" || q == "A")
            .count()
    };
    for tour in 0..TOURS {
        let avant = retours();
        let mut enfant = lancer("boucle", dir.path(), None);
        for _ in 0..2400 {
            if retours() >= avant + 3 {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            retours() >= avant + 3,
            "tour {tour}: aucun acquittement en 60 s"
        );
        std::thread::sleep(Duration::from_micros(1_500 * (tour % 9 + 1)));
        if let Ok(Some(s)) = enfant.try_wait() {
            panic!("tour {tour}: l'enfant s'est arrêté seul ({s:?})");
        }
        enfant.kill().expect("kill -9");
        let _ = enfant.wait();

        let mut acquittes: std::collections::HashMap<String, String> = Default::default();
        let mut intentions: std::collections::HashMap<String, String> = Default::default();
        for (q, p, v) in lignes() {
            match q.as_str() {
                "A" | "R" => {
                    acquittes.insert(p, v);
                }
                "I" => {
                    intentions.insert(p, v);
                }
                _ => {}
            }
        }
        let db = ObrainDB::open(&base).expect("réouverture");
        let r = db
            .session()
            .execute("MATCH (f:File) RETURN f.path, f.hash")
            .expect("relecture");
        let mut relus: std::collections::HashMap<String, Option<String>> = Default::default();
        for l in &r.rows {
            if let Some(Value::String(p)) = l.first() {
                relus.insert(
                    p.to_string(),
                    match l.get(1) {
                        Some(Value::String(h)) => Some(h.to_string()),
                        _ => None,
                    },
                );
            }
        }
        for (p, ack) in &acquittes {
            let d = relus.get(p).cloned().flatten();
            let ok = d.as_deref() == Some(ack.as_str())
                || intentions
                    .get(p)
                    .is_some_and(|i| d.as_deref() == Some(i.as_str()));
            assert!(
                ok,
                "tour {tour}: {} → la valeur durable {:?} n'est ni l'acquittée {} ni \
                 l'intention {:?}",
                &p[p.len() - 8..],
                d.as_deref().map(|v| &v[..v.len().min(12)]),
                &ack[..12],
                intentions.get(p).map(|v| &v[..v.len().min(12)])
            );
        }
        db.close().expect("close");
    }
}
