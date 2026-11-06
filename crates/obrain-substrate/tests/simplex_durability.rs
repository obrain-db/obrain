//! Durabilité de la couche simpliciale (T4, étape WAL+rejeu).
//!
//! Méthode de la campagne de durabilité du substrat (crash_safety.rs,
//! wal_recovery_reelle.rs), appliquée aux zones simpliciales :
//!
//! 1. **Rejeu bit-à-bit** — écrire, msync, WIPER les quatre fichiers
//!    simpliciaux, rejouer depuis le WAL seul : les régions utiles doivent
//!    être identiques à l'octet près (invariant « reconstructible depuis le
//!    WAL seul », engram.rs:29-39, repris par la spec 271180c3 I7).
//! 2. **kill -9 RÉEL** — un processus enfant écrit des simplexes en boucle
//!    (fsync par commit), le parent le tue en plein vol, rouvre : rejeu
//!    sain (au plus une frontière de déchirure), zéro simplexe corrompu,
//!    incidence cohérente.
//! 3. **WAL déchiré** — troncature à des offsets variés : le rejeu s'arrête
//!    proprement à la frontière, tout ce qui précède est intact.

use obrain_substrate::file::Zone;
use obrain_substrate::simplex_incidence::SimplexIncidenceZone;
use obrain_substrate::simplex_zone::{self, SimplexDirZone};
use obrain_substrate::wal_io::SyncMode;
use obrain_substrate::{
    SIMPLEX_HEAP_FILENAME, SIMPLEX_POSTINGS_FILENAME, SubstrateFile, Writer, replay_from,
    simplex_flags, simplex_kind,
};
use std::path::Path;

const FICHIERS_SIMPLICIAUX: &[&str] = &[
    "substrate.simplex_dir",
    SIMPLEX_HEAP_FILENAME,
    "substrate.simplex_incidence",
    SIMPLEX_POSTINGS_FILENAME,
];

/// Écrit un lot mixte de simplexes (triangles, K4, hyperarêtes ORDERED ou
/// non, avec props) — déterministe en fonction de `n`.
fn ecrire_lot(w: &Writer, n: u32) -> Vec<u32> {
    let mut ids = Vec::new();
    for i in 0..n {
        let base = i * 10;
        let (id, created) = w
            .upsert_simplex(
                simplex_kind::TRIANGLE,
                0,
                if i % 3 == 0 { simplex_flags::CYCLIC } else { 0 },
                &[base + 2, base, base + 1],
                &[],
            )
            .unwrap();
        assert!(created);
        ids.push(id);
        if i % 2 == 0 {
            let (id, _) = w
                .upsert_simplex(
                    simplex_kind::HYPEREDGE,
                    3,
                    if i % 4 == 0 {
                        simplex_flags::ORDERED
                    } else {
                        0
                    },
                    &[base + 5, base + 3, base + 4, base + 6],
                    format!("{{\"line\":{i}}}").as_bytes(),
                )
                .unwrap();
            ids.push(id);
        }
        if i % 5 == 0 {
            let (id, _) = w
                .upsert_simplex(
                    simplex_kind::K4,
                    0,
                    0,
                    &[base, base + 1, base + 2, base + 3],
                    &[],
                )
                .unwrap();
            ids.push(id);
        }
        w.commit().unwrap();
    }
    ids
}

fn wipe(sub_path: &Path) {
    for f in FICHIERS_SIMPLICIAUX {
        let p = sub_path.join(f);
        if p.exists() {
            std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&p)
                .unwrap();
        }
    }
}

/// Région UTILE d'un fichier simplicial (l'au-delà n'est que la politique
/// de pré-allocation, non significative) :
/// * dir — en-tête + records jusqu'au high-water ;
/// * heap — en-tête + payload_used ;
/// * incidence/postings — le fichier entier jusqu'au max des deux longueurs
///   (zéro-rempli au-delà des pages allouées).
fn octets_utiles(sub_path: &Path, f: &str) -> Vec<u8> {
    let bytes = std::fs::read(sub_path.join(f)).unwrap_or_default();
    if bytes.is_empty() {
        return bytes;
    }
    let end = match f {
        "substrate.simplex_dir" => {
            let hw = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
            64 + hw * 32
        }
        SIMPLEX_HEAP_FILENAME => {
            let used = u64::from_le_bytes(bytes[8..16].try_into().unwrap()) as usize;
            64 + used
        }
        SIMPLEX_POSTINGS_FILENAME => u64::from_le_bytes(bytes[8..16].try_into().unwrap()) as usize,
        _ => bytes.len(),
    }
    .min(bytes.len());
    let mut v = bytes[..end].to_vec();
    // L'incidence n'a pas de high-water : comparer en normalisant la queue
    // zéro (deux politiques de croissance différentes restent équivalentes).
    if f == "substrate.simplex_incidence" {
        while v.last() == Some(&0) {
            v.pop();
        }
    }
    v
}

/// (1) Zones wipées + rejeu depuis le WAL seul ⇒ reconstruction
/// bit-à-bit des régions utiles.
#[test]
fn rejeu_reconstruit_bit_a_bit_depuis_le_wal_seul() {
    let td = tempfile::tempdir().unwrap();
    let sub_path = td.path().join("kb");
    {
        let sub = SubstrateFile::create(&sub_path).unwrap();
        let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
        let ids = ecrire_lot(&w, 40);
        // Quelques tombstones + réécritures de props pour couvrir 0x73 et
        // les réécritures sous le même id.
        w.tombstone_simplex(ids[3]).unwrap();
        w.tombstone_simplex(ids[10]).unwrap();
        w.upsert_simplex(
            simplex_kind::HYPEREDGE,
            3,
            0,
            &[25, 23, 24, 26],
            b"{\"line\":999}",
        )
        .unwrap();
        w.commit().unwrap();
        w.msync_zones().unwrap();
    }

    // Référence : régions utiles après msync.
    let reference: Vec<Vec<u8>> = FICHIERS_SIMPLICIAUX
        .iter()
        .map(|f| octets_utiles(&sub_path, f))
        .collect();
    assert!(
        reference[0].len() > 64,
        "le lot doit avoir peuplé le répertoire"
    );

    // Wipe total des quatre fichiers, rejeu depuis le WAL seul.
    wipe(&sub_path);
    let sub = SubstrateFile::open(&sub_path).unwrap();
    let stats = replay_from(&sub, 0).unwrap();
    assert_eq!(stats.decode_errors, 0);
    drop(sub);

    for (f, avant) in FICHIERS_SIMPLICIAUX.iter().zip(&reference) {
        let apres = octets_utiles(&sub_path, f);
        assert_eq!(
            &apres,
            avant,
            "rejeu non bit-à-bit pour {f} : {} octets avant, {} après",
            avant.len(),
            apres.len()
        );
    }
}

/// (1 bis) Le rejeu est idempotent : rejouer DEUX fois produit le même état.
#[test]
fn double_rejeu_idempotent() {
    let td = tempfile::tempdir().unwrap();
    let sub_path = td.path().join("kb");
    {
        let sub = SubstrateFile::create(&sub_path).unwrap();
        let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
        let ids = ecrire_lot(&w, 10);
        w.tombstone_simplex(ids[0]).unwrap();
        w.commit().unwrap();
    }
    wipe(&sub_path);
    {
        let sub = SubstrateFile::open(&sub_path).unwrap();
        replay_from(&sub, 0).unwrap();
    }
    let une_fois: Vec<Vec<u8>> = FICHIERS_SIMPLICIAUX
        .iter()
        .map(|f| octets_utiles(&sub_path, f))
        .collect();
    {
        let sub = SubstrateFile::open(&sub_path).unwrap();
        replay_from(&sub, 0).unwrap();
    }
    for (f, avant) in FICHIERS_SIMPLICIAUX.iter().zip(&une_fois) {
        assert_eq!(
            &octets_utiles(&sub_path, f),
            avant,
            "double rejeu a divergé pour {f}"
        );
    }
}

/// (2) — CIBLE DU KILL. Processus enfant : écrit des simplexes en boucle
/// jusqu'à être tué. Ne s'exécute que sous `SPX_KILL_DIR` (sinon no-op :
/// le harnais de test la voit comme un test trivialement vert).
#[test]
fn enfant_ecrit_des_simplexes_en_boucle() {
    let Some(dir) = std::env::var_os("SPX_KILL_DIR") else {
        return;
    };
    let sub = SubstrateFile::create(Path::new(&dir).join("kb")).unwrap();
    let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
    for i in 0u32.. {
        let base = i * 10;
        w.upsert_simplex(
            simplex_kind::TRIANGLE,
            0,
            0,
            &[base, base + 1, base + 2],
            &[],
        )
        .unwrap();
        w.upsert_simplex(
            simplex_kind::HYPEREDGE,
            7,
            0,
            &[base + 3, base + 4, base + 5],
            format!("{{\"i\":{i}}}").as_bytes(),
        )
        .unwrap();
        w.commit().unwrap();
        // Trace de progression pour le parent (nombre de commits durables).
        // Écriture ATOMIQUE par rename : le kill -9 peut tomber n'importe
        // où, le parent ne doit jamais lire un fichier tronqué.
        let tmp = Path::new(&dir).join("progress.tmp");
        std::fs::write(&tmp, format!("{i}")).unwrap();
        std::fs::rename(&tmp, Path::new(&dir).join("progress")).unwrap();
    }
}

/// (2) kill -9 RÉEL pendant l'écriture → réouverture saine.
#[test]
fn kill_9_pendant_ecriture_reouverture_saine() {
    let td = tempfile::tempdir().unwrap();
    let exe = std::env::current_exe().unwrap();
    let mut child = std::process::Command::new(&exe)
        .args([
            "enfant_ecrit_des_simplexes_en_boucle",
            "--exact",
            "--test-threads=1",
            "--nocapture",
        ])
        .env("SPX_KILL_DIR", td.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn du processus enfant");

    // Attendre que l'enfant ait durablement commité quelques lots. On garde
    // la DERNIÈRE valeur lue : après le kill, le fichier est relu mais la
    // valeur observée ici reste le plancher garanti.
    let progress = td.path().join("progress");
    let debut = web_time::Instant::now();
    let mut confirmes_avant_kill = 0u32;
    loop {
        if let Ok(s) = std::fs::read_to_string(&progress) {
            confirmes_avant_kill = confirmes_avant_kill.max(s.trim().parse::<u32>().unwrap_or(0));
        }
        if confirmes_avant_kill >= 20 {
            break;
        }
        assert!(
            debut.elapsed() < std::time::Duration::from_secs(30),
            "l'enfant n'a pas progressé en 30 s"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    // kill -9, en plein vol.
    let pid = child.id();
    let status = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let _ = child.wait();

    // Réouverture : rejeu sain — au plus UNE frontière de déchirure.
    let sub_path = td.path().join("kb");
    let sub = SubstrateFile::open(&sub_path).unwrap();
    let stats = replay_from(&sub, 0).unwrap();
    assert!(
        stats.decode_errors <= 1,
        "au plus une frontière de crash attendue, {} trouvées",
        stats.decode_errors
    );

    // Tous les lots dont le `progress` atteste le commit doivent être
    // présents et bien formés. Relecture post-kill (rename atomique côté
    // enfant), avec la valeur pré-kill comme plancher.
    let commits_confirmes: u32 = std::fs::read_to_string(&progress)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
        .max(confirmes_avant_kill);
    let dir = sub.open_zone(Zone::SimplexDir).unwrap();
    let heap = sub.open_named_zone(SIMPLEX_HEAP_FILENAME).unwrap();
    let inc = sub.open_zone(Zone::SimplexIncidence).unwrap();
    let posts = sub.open_named_zone(SIMPLEX_POSTINGS_FILENAME).unwrap();

    let hw = SimplexDirZone::high_water(&dir).unwrap();
    assert!(
        hw >= 2 * commits_confirmes,
        "au moins {} simplexes attendus (commits confirmés), {hw} trouvés",
        2 * commits_confirmes
    );

    // Chaque record présent est canonique et son incidence est cohérente.
    for id in 0..hw {
        let rec = SimplexDirZone::read(&dir, id)
            .unwrap()
            .expect("aucun trou sous le high-water");
        let (members, _props) = simplex_zone::read_members_props(&rec, &heap).unwrap();
        assert_eq!(members.len(), rec.member_count as usize);
        assert!(
            members.windows(2).all(|w| w[0] < w[1]),
            "membres non canoniques après kill -9 : {members:?}"
        );
        for &m in &members {
            let postings = SimplexIncidenceZone::postings(&inc, &posts, m).unwrap();
            assert!(
                postings.contains(&id),
                "incidence incohérente après kill -9 : nœud {m} sans le simplexe {id}"
            );
        }
    }
}

/// (3) WAL déchiré à des offsets variés : arrêt propre à la frontière,
/// préfixe intact, aucune panique.
#[test]
fn wal_dechire_arret_propre() {
    for seed in 0u64..12 {
        let td = tempfile::tempdir().unwrap();
        let sub_path = td.path().join("kb");
        let wal_path;
        {
            let sub = SubstrateFile::create(&sub_path).unwrap();
            wal_path = sub.wal_path();
            let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
            ecrire_lot(&w, 12);
        }
        // Déchirer le WAL à un offset pseudo-aléatoire (jamais 0).
        let wal_len = std::fs::metadata(&wal_path).unwrap().len();
        let tear = 1 + (seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) % (wal_len - 1));
        let f = std::fs::OpenOptions::new()
            .write(true)
            .open(&wal_path)
            .unwrap();
        f.set_len(tear).unwrap();
        drop(f);

        wipe(&sub_path);
        let sub = SubstrateFile::open(&sub_path).unwrap();
        let stats = replay_from(&sub, 0).unwrap();
        assert!(stats.decode_errors <= 1, "seed {seed} : arrêt non propre");
        assert!(stats.stopped_at <= tear);
        // Tout ce qui a été rejoué est bien formé.
        let dir = sub.open_zone(Zone::SimplexDir).unwrap();
        let heap = sub.open_named_zone(SIMPLEX_HEAP_FILENAME).unwrap();
        if !dir.is_empty() {
            let hw = SimplexDirZone::high_water(&dir).unwrap();
            for id in 0..hw {
                if let Some(rec) = SimplexDirZone::read(&dir, id).unwrap() {
                    let (members, _) = simplex_zone::read_members_props(&rec, &heap).unwrap();
                    if rec.flags & simplex_flags::ORDERED == 0 {
                        assert!(members.windows(2).all(|w| w[0] < w[1]));
                    } else {
                        // ORDERED : ordre d'émission gelé — la CLÉ (tri)
                        // doit rester pairwise-distincte.
                        let mut sorted = members.clone();
                        sorted.sort_unstable();
                        assert!(sorted.windows(2).all(|w| w[0] < w[1]));
                    }
                }
            }
        }
    }
}

/// Écriture perdue sans flush (drop du writer) : le WAL fsync'é suffit.
#[test]
fn drop_sans_flush_rejeu_reconstruit() {
    let td = tempfile::tempdir().unwrap();
    let sub_path = td.path().join("kb");
    let ids;
    {
        let sub = SubstrateFile::create(&sub_path).unwrap();
        let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
        ids = ecrire_lot(&w, 25);
        // PAS de msync_zones : les pages sales peuvent se perdre — le WAL a tout.
    }
    wipe(&sub_path);
    let sub = SubstrateFile::open(&sub_path).unwrap();
    let stats = replay_from(&sub, 0).unwrap();
    assert_eq!(stats.decode_errors, 0);

    // Relecture par l'API writer sur la base rejouée.
    let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
    for &id in &ids {
        assert!(
            w.get_simplex(id).unwrap().is_some(),
            "simplexe {id} perdu après drop sans flush"
        );
    }
    // L'unicité tient après rejeu : re-upserter un triangle du lot est un no-op.
    let (id0, created) = w
        .upsert_simplex(
            simplex_kind::TRIANGLE,
            0,
            simplex_flags::CYCLIC,
            &[0, 1, 2],
            &[],
        )
        .unwrap();
    assert!(
        !created,
        "l'upsert d'une clé existante doit retrouver l'id {id0}"
    );
}

/// Crash entre l'écriture du record du répertoire et la fin de la pose des
/// postings d'incidence (`Writer::upsert_simplex` : WAL, puis
/// `apply_set_raw`, puis une `SimplexIncidenceZone::append` par membre).
/// Après kill -9 à cet endroit, le WAL et le record sont durables, les
/// postings ne le sont pas. Simulé ici en effaçant l'incidence après un commit
/// complet : le rejeu voit le record déjà présent (`Unchanged`) et, sans
/// reposter, laisse chaque membre sans son simplexe.
#[test]
fn rejeu_repose_l_incidence_d_un_record_deja_present() {
    let td = tempfile::tempdir().unwrap();
    let sub_path = td.path().join("kb");
    {
        let sub = SubstrateFile::create(&sub_path).unwrap();
        let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
        let (id, created) = w
            .upsert_simplex(simplex_kind::TRIANGLE, 0, 0, &[10, 20, 30], &[])
            .unwrap();
        assert!(created);
        assert_eq!(id, 0);
        w.commit().unwrap();
    }
    // État post-crash : record et WAL présents, postings absents.
    for f in ["substrate.simplex_incidence", SIMPLEX_POSTINGS_FILENAME] {
        std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(sub_path.join(f))
            .unwrap();
    }

    let sub = SubstrateFile::open(&sub_path).unwrap();
    let stats = replay_from(&sub, 0).unwrap();
    assert_eq!(stats.decode_errors, 0);

    let inc = sub.open_zone(Zone::SimplexIncidence).unwrap();
    let posts = sub.open_named_zone(SIMPLEX_POSTINGS_FILENAME).unwrap();
    for m in [10u32, 20, 30] {
        let p = SimplexIncidenceZone::postings(&inc, &posts, m).unwrap();
        assert_eq!(p, vec![0], "nœud {m} : incidence absente après rejeu");
    }
}
