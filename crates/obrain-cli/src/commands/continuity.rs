//! `obrain continuity <store> [--stale-after-min N]` — SENTINELLE de continuité
//! d'écriture des journaux de session (chantier « que ça n'arrive plus »,
//! 2026-09-05 : écrivain absent ~1 h → base figée sans alerte).
//!
//! LECTURE pure : scanne les zones `substrate.journal.<session>` du store et
//! lit la QUEUE de chaque journal (dernier événement horodaté) ; rapporte la
//! fraîcheur store-wide + par session. `stale` (exit 1) dès que le store n'a
//! reçu AUCUN événement depuis `--stale-after-min` minutes — c'est le signal
//! « l'écrivain est mort/absent » qu'un timer ops (systemd/cron) peut brancher.
//!
//! Sortie JSON stable : `{store, checked_at_ms, stale_after_min, stale,
//! last_event_at_ms, sessions:[{session_id, last_seq, last_event_at_ms,
//! last_event_age_sec, stale}]}`.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::OutputFormat;

/// Préfixe des zones de journal de session dans le store.
pub const JOURNAL_PREFIX: &str = "substrate.journal.";

/// État de continuité d'une session.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SessionStaleness {
    pub session_id: String,
    pub last_seq: i64,
    pub last_event_at_ms: i64,
    pub last_event_age_sec: i64,
    pub stale: bool,
}

/// Rapport de continuité du store.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ContinuityReport {
    pub store: String,
    pub checked_at_ms: i64,
    pub stale_after_min: u64,
    /// Le store n'a reçu AUCUN événement depuis `stale_after_min`.
    pub stale: bool,
    /// Dernier événement horodaté toutes sessions confondues (ms Unix, 0 si
    /// aucun journal).
    pub last_event_at_ms: i64,
    /// Âge du dernier événement (secondes).
    pub last_event_age_sec: i64,
    /// Nombre de zones de journal présentes.
    pub journals: usize,
    pub sessions: Vec<SessionStaleness>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("horloge")
        .as_millis() as i64
}

fn mtime_ms(path: &std::path::Path) -> Option<i64> {
    let m = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(m.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as i64)
}

/// Calcule le rapport de continuité à partir des FICHIERS de zone
/// (`substrate.journal.<session>`) du répertoire du store : la fraîcheur est
/// le MTIME du fichier (chaque append msync met à jour le fichier). LECTURE
/// de métadonnées uniquement — instantané même sur des stores multi-Go, sans
/// verrou, utilisable pendant que l'écrivain tourne.
pub fn compute(dir: &std::path::Path, now: i64, stale_after_min: u64) -> ContinuityReport {
    let stale_after_ms = (stale_after_min.max(1)) * 60_000;
    let mut sessions: Vec<SessionStaleness> = Vec::new();
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name();
            let Some(n) = name.to_str() else { continue };
            if n.starts_with(JOURNAL_PREFIX) && n.len() > JOURNAL_PREFIX.len() {
                files.push(e.path());
            }
        }
    }
    files.sort();
    let mut last_event_at_ms: i64 = 0;
    for path in &files {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let session_id = name[JOURNAL_PREFIX.len()..].to_string();
        let Some(ts) = mtime_ms(path) else { continue };
        let age = ((now - ts) / 1000).max(0);
        sessions.push(SessionStaleness {
            session_id,
            last_seq: -1, // non utilisé en mode mtime
            last_event_at_ms: ts,
            last_event_age_sec: age,
            stale: age as u64 > stale_after_ms / 1000,
        });
        last_event_at_ms = last_event_at_ms.max(ts);
    }
    sessions.sort_by(|a, b| b.last_event_at_ms.cmp(&a.last_event_at_ms));
    let age_sec = if last_event_at_ms > 0 {
        ((now - last_event_at_ms) / 1000).max(0)
    } else {
        -1
    };
    ContinuityReport {
        store: dir.display().to_string(),
        checked_at_ms: now,
        stale_after_min,
        stale: last_event_at_ms > 0 && (age_sec as u64) > stale_after_ms / 1000,
        last_event_at_ms,
        last_event_age_sec: age_sec,
        journals: sessions.len(),
        sessions,
    }
}

/// `obrain continuity <store> [--stale-after-min N]` — imprime le rapport et
/// sort en code 1 si le store est PÉRIMÉ (aucune zone de journal modifiée
/// depuis N minutes). Stat-only : aucune ouverture/verrou du store.
pub fn run(path: &Path, stale_after_min: u64, format: OutputFormat, quiet: bool) -> Result<()> {
    if !path.is_dir() {
        anyhow::bail!("continuity: store introuvable: {}", path.display());
    }
    let now = now_ms();
    let report = compute(path, now, stale_after_min);
    match format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&report).context("continuity: sérialisation")?
        ),
        _ => {
            if !quiet {
                println!(
                    "continuity {}: dernière écriture il y a {}s (seuil {} min) — {} journals, {} stale",
                    report.store,
                    report.last_event_age_sec,
                    stale_after_min,
                    report.journals,
                    report.sessions.iter().filter(|s| s.stale).count()
                );
                for s in &report.sessions {
                    println!(
                        "  {:<48} age={}s{}",
                        s.session_id,
                        s.last_event_age_sec,
                        if s.stale { "  ⚠ STALE" } else { "" }
                    );
                }
            }
        }
    }
    if report.stale {
        eprintln!(
            "continuity: STALE — aucune écriture de journal depuis {} min",
            stale_after_min
        );
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use obrain_engine::journal_zone::write_session_journal;
    use obrain_substrate::SubstrateStore;

    fn line(seq: i64) -> String {
        format!(r#"{{"type":"event","seq":{seq},"time":1700000000000,"data":{{"n":{seq}}}}}"#)
    }

    fn temp_store() -> (tempfile::TempDir, std::path::PathBuf) {
        let td = tempfile::tempdir().unwrap();
        let dir = td.path().join("kb");
        let store = std::sync::Arc::new(SubstrateStore::create(&dir).unwrap());
        drop(store);
        (td, dir)
    }

    /// Écrit un journal puis force son mtime à `age_sec` secondes dans le
    /// passé (le mtime du fichier = dernière écriture de la zone).
    fn journal_age(dir: &Path, session: &str, age_sec: i64) {
        let store = std::sync::Arc::new(SubstrateStore::open(dir).unwrap());
        write_session_journal(&store, session, &[line(1)]).unwrap();
        drop(store);
        let now = std::time::SystemTime::now();
        let past = now - std::time::Duration::from_secs(age_sec.max(1) as u64);
        let file = std::fs::File::open(dir.join(format!("{JOURNAL_PREFIX}{session}"))).unwrap();
        let times = std::fs::FileTimes::new().set_modified(past);
        file.set_times(times).unwrap();
    }

    #[test]
    fn continuity_detecte_un_store_fige() {
        let (td, dir) = temp_store();
        let now = now_ms();
        // Deux sessions figées depuis 20 min.
        journal_age(&dir, "session-a", 20 * 60);
        journal_age(&dir, "session-b", 20 * 60 + 1);

        let rep = compute(&dir, now, 10);
        assert!(rep.stale, "store figé détecté");
        assert_eq!(rep.journals, 2);
        assert!(rep.sessions.iter().all(|s| s.stale));

        // Une session fraîche dé-fige le STORE…
        journal_age(&dir, "session-c", 1);
        let rep2 = compute(&dir, now, 10);
        assert!(!rep2.stale, "session fraîche ⇒ store non figé");
        let a = rep2
            .sessions
            .iter()
            .find(|s| s.session_id == "session-a")
            .unwrap();
        let c = rep2
            .sessions
            .iter()
            .find(|s| s.session_id == "session-c")
            .unwrap();
        assert!(a.stale, "session-a stale individuellement");
        assert!(!c.stale, "session-c fraîche");

        // Seuil large (30 min) : rien n'est stale.
        let rep3 = compute(&dir, now, 30);
        assert!(!rep3.stale);
        assert!(rep3.sessions.iter().all(|s| !s.stale));
        let _ = td;
    }

    #[test]
    fn continuity_store_sans_journal_est_ok() {
        let (td, dir) = temp_store();
        let rep = compute(&dir, now_ms(), 10);
        assert!(!rep.stale, "pas de journal → pas d'alarme");
        assert_eq!(rep.journals, 0);
        assert!(rep.sessions.is_empty());
        let _ = td;
    }
}
