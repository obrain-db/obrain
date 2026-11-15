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

/// Affichage seulement (jamais l'oracle) : préfixe d'au plus 12 octets, reculé
/// jusqu'à une frontière de caractère. Ne panique jamais, quelle que soit la longueur.
fn abrege(s: &str) -> &str {
    let mut n = s.len().min(12);
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    &s[..n]
}

/// Queue d'un chemin, pour l'affichage (même garantie que `abrege`).
fn queue(s: &str) -> &str {
    let mut debut = s.len().saturating_sub(8);
    while !s.is_char_boundary(debut) {
        debut += 1;
    }
    &s[debut..]
}

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
                // Chemin unique par processus enfant : un tour suivant ne recrée JAMAIS le
                // chemin d'un nœud déjà écrit (sinon la lecture par chemin écrase une ligne).
                let path = long("d", &format!("-{}-{i}", std::process::id()));
                let id = db.create_node(&["File"]);
                noter(&dir, &format!("N\t{path}\t{}", id.0));
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
                .map(|v| format!(" [{}…, {} o]", abrege(v), v.len()))
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
/// Graine du test au hasard : fixe le délai de kill et le nombre de retours avant
/// le kill. `OBRAIN_TEST_GRAINE=<u64>` la rejoue ; sinon elle vient de l'horloge.
/// Imprimée dans le message d'échec : une occurrence se rejoue avec la même graine.
fn graine_du_test() -> u64 {
    std::env::var("OBRAIN_TEST_GRAINE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(1)
        })
}

/// Diagnostic d'une divergence, lu JUSTE APRÈS la réouverture : le journal du chemin,
/// la queue du WAL (offsets, enregistrements), et le slot de la colonne blob du nœud
/// (entrée d'index : offset d'arène et longueur ; en-tête brut, les 16 premiers octets).
fn diagnostic(
    base: &Path,
    graine: u64,
    tour: u64,
    chemin: &str,
    id: Option<u64>,
    lignes: &[(String, String, String)],
    durable: Option<&str>,
) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "DIAG graine={graine} tour={tour} chemin={} node_id={id:?} durable={durable:?}",
        queue(chemin)
    );
    for (q, p, v) in lignes.iter().filter(|(_, p, _)| p == chemin) {
        let _ = writeln!(out, "  journal {q} {} {}", queue(p), abrege(v));
    }
    match obrain_substrate::wal_io::WalReader::open(base.join("substrate.wal")) {
        Ok(reader) => {
            let mut queue_wal: std::collections::VecDeque<String> = Default::default();
            for item in reader.iter_from(0) {
                match item {
                    Ok((rec, offset, _)) => {
                        let texte: String =
                            format!("off={offset} lsn={} {:?}", rec.lsn, rec.payload)
                                .chars()
                                .take(220)
                                .collect();
                        queue_wal.push_back(texte);
                        if queue_wal.len() > 14 {
                            queue_wal.pop_front();
                        }
                    }
                    Err(e) => {
                        queue_wal.push_back(format!("ARRÊT: {e:?}"));
                        break;
                    }
                }
            }
            for t in queue_wal {
                let _ = writeln!(out, "  wal {t}");
            }
        }
        Err(e) => {
            let _ = writeln!(out, "  wal illisible: {e:?}");
        }
    }
    if let Some(id) = id {
        let slot = id as usize;
        if let Ok(entrees) = std::fs::read_dir(base) {
            let mut noms: Vec<_> = entrees
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.starts_with("substrate.blobcol.") && n.ends_with(".idx"))
                .collect();
            noms.sort();
            for nom in noms {
                let octets = std::fs::read(base.join(&nom)).unwrap_or_default();
                let entree = 64 + slot * 16;
                let _ = write!(out, "  blob {nom} taille={} ", octets.len());
                if octets.len() >= entree + 16 {
                    let arena = u64::from_le_bytes(octets[entree..entree + 8].try_into().unwrap());
                    let len =
                        u32::from_le_bytes(octets[entree + 8..entree + 12].try_into().unwrap());
                    let _ = write!(out, "slot[{slot}]=arena_offset:{arena} len:{len} ");
                } else {
                    let _ = write!(out, "slot[{slot}] hors fichier ");
                }
                let entete: Vec<String> =
                    octets.iter().take(16).map(|b| format!("{b:02x}")).collect();
                let _ = writeln!(out, "entete16={}", entete.join(" "));
            }
        }
    }
    out
}

#[test]
fn des_remplacements_tues_au_hasard_ne_perdent_aucune_valeur_acquittee() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    const TOURS: u64 = 6;
    let graine = graine_du_test();
    eprintln!("GRAINE {graine} (OBRAIN_TEST_GRAINE={graine} pour rejouer)");
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
        // Timing dérivé de la graine : seuil de retours et délai de kill variés.
        let melange = graine
            .wrapping_add(tour.wrapping_mul(0x9E37_79B9_7F4A_7C15))
            .wrapping_mul(0xBF58_476D_1CE4_E5B9);
        let seuil = 3 + (melange >> 40) as usize % 4;
        let delai = Duration::from_micros((melange >> 17) % 30_000);
        let avant = retours();
        let mut enfant = lancer("boucle", dir.path(), None);
        for _ in 0..2400 {
            if retours() >= avant + seuil {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            retours() >= avant + seuil,
            "tour {tour}: aucun acquittement en 60 s"
        );
        std::thread::sleep(delai);
        if let Ok(Some(s)) = enfant.try_wait() {
            panic!("tour {tour}: l'enfant s'est arrêté seul ({s:?})");
        }
        enfant.kill().expect("kill -9");
        let _ = enfant.wait();

        let lus = lignes();
        let mut acquittes: std::collections::HashMap<String, String> = Default::default();
        let mut intentions: std::collections::HashMap<String, String> = Default::default();
        let mut ids: std::collections::HashMap<String, u64> = Default::default();
        for (q, p, v) in &lus {
            match q.as_str() {
                "A" | "R" => {
                    acquittes.insert(p.clone(), v.clone());
                }
                "I" => {
                    intentions.insert(p.clone(), v.clone());
                }
                "N" => {
                    ids.insert(p.clone(), v.parse().unwrap_or(0));
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
            if !ok {
                // Diagnostic lu après réouverture, imprimé dans le message d'échec.
                let diag = diagnostic(
                    &base,
                    graine,
                    tour,
                    p,
                    ids.get(p).copied(),
                    &lus,
                    d.as_deref(),
                );
                // Toutes les lignes File de ce chemin (un chemin peut porter plusieurs
                // nœuds si l'enfant l'a recréé dans un tour suivant).
                let toutes: Vec<Option<String>> = r
                    .rows
                    .iter()
                    .filter(|l| matches!(l.first(), Some(Value::String(q)) if q.as_str() == p))
                    .map(|l| match l.get(1) {
                        Some(Value::String(h)) => Some(abrege(h).to_string()),
                        _ => None,
                    })
                    .collect();
                // Identifiants réels des nœuds File du chemin, lus au même instant.
                let ids_lus = db
                    .session()
                    .execute("MATCH (f:File) RETURN id(f), f.path, f.hash")
                    .map(|rr| {
                        rr.rows
                            .iter()
                            .filter(
                                |l| matches!(l.get(1), Some(Value::String(q)) if q.as_str() == p),
                            )
                            .map(|l| {
                                format!(
                                    "{:?} hash={:?}",
                                    l.first(),
                                    l.get(2).map(|v| format!("{v:?}")
                                        .chars()
                                        .take(30)
                                        .collect::<String>())
                                )
                            })
                            .collect::<Vec<String>>()
                    })
                    .unwrap_or_else(|e| vec![format!("requête id(f) refusée: {e:?}")]);
                let diag = format!(
                    "{diag}  lignes File du chemin : {toutes:?}\n  id(f) lus: {ids_lus:?}\n"
                );
                eprintln!("{diag}");
                panic!(
                    "{diag}tour {tour}: {} → la valeur durable {:?} n'est ni l'acquittée {} ni \
                     l'intention {:?} (GRAINE {graine})",
                    queue(p),
                    d.as_deref().map(abrege),
                    abrege(ack),
                    intentions.get(p).map(|v| abrege(v))
                );
            }
        }
        db.close().expect("close");
    }
}

#[cfg(test)]
mod affichage {
    use super::{abrege, queue};

    #[test]
    fn l_affichage_ne_panique_jamais_quelle_que_soit_la_longueur() {
        for n in 0..=40 {
            let ascii = "a".repeat(n);
            assert!(abrege(&ascii).len() <= 12 && ascii.starts_with(abrege(&ascii)));
            assert!(queue(&ascii).len() <= 8 && ascii.ends_with(queue(&ascii)));
            // multi-octets : 'é' = 2 octets ; une coupure à 12 peut tomber au milieu.
            let mb = format!("x{}", "é".repeat(n));
            assert!(abrege(&mb).len() <= 12 && mb.starts_with(abrege(&mb)));
            assert!(mb.ends_with(queue(&mb)));
        }
        assert_eq!(
            abrege("court-2"),
            "court-2",
            "chaîne de 7 octets : rendue entière"
        );
        assert_eq!(abrege(""), "");
        assert_eq!(abrege("hhhhhhhhhhhhhhhhhhhhhhhh"), "hhhhhhhhhhhh");
    }
}
