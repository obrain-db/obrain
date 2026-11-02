//! `reparer_etiquettes` — retrait des DOUBLONS d'étiquette `Self` (maintenance).
//!
//! # Le défaut constaté (2026-09-16)
//!
//! `MATCH (n:Self)` rendait **4** nœuds : le vrai soi (`self:agent`) et **trois
//! doublons** — une roadmap (`plan:…`), un ADR (`adr:…`), une étape de session
//! (`session-…:step:…`) — chacun portant `Self` **comme seule étiquette**, alors
//! que le nœud correct existe **par ailleurs** sous son vrai label
//! (`Plan`, `ADR`, `Step`). Résidu d'anciennes versions de l'importeur.
//!
//! Conséquence : une IA qui se lit **par étiquette** se prend pour une roadmap ;
//! et `viz_weave` teste `count_label("Self") > 0`, donc le bruit suffit à faire
//! croire que le soi existe.
//!
//! # Trois gardes, parce qu'une suppression est irréversible
//!
//! 1. **On ne supprime JAMAIS un nœud sans jumeau correctement étiqueté.** Si le
//!    même `qualified_name` n'existe pas sous un autre label, le nœud est peut-être
//!    le seul porteur de l'information : on refuse au lieu de deviner.
//! 2. **On ne touche ni `self:agent`, ni un nœud rattaché à un compte.** Le soi
//!    d'un compte utilisateur (`OWNED_BY` vers un `Account`) n'est pas un
//!    doublon : c'est une identité, et la retirer effacerait quelqu'un.
//! 3. **On refuse de tourner s'il y a un AUTRE écrivain sur ce store.** Le mode
//!    `shared-write` n'exclut personne : deux écrivains corrompraient le WAL
//!    (constat du 2026-09-01). La maintenance se fait donc seule, ou pas.
//!
//! À blanc par défaut ; `--apply` exécute. Usage :
//!
//! ```sh
//! cargo run --release -p obrain-cli --example reparer_etiquettes -- <store> [--apply]
//! ```

use std::path::Path;

use anyhow::{Context, Result, bail};
use obrain_engine::ObrainDB;

/// Le seul soi légitime de ce store : jamais touché, jamais supprimé.
const SOI: &str = "self:agent";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let apply = args.iter().any(|a| a == "--apply");
    let Some(store) = args.iter().find(|a| !a.starts_with("--")) else {
        bail!("usage: reparer_etiquettes <store> [--apply]");
    };
    let store = Path::new(store);

    // ── GARDE 3 : suis-je le SEUL écrivain ? ────────────────────────────────
    // On lit `/proc/*/environ` : tout process qui porte `OBRAIN_WRITE=1` sur CE
    // store est un écrivain concurrent. On s'arrête net plutôt que de risquer le
    // WAL d'un store vivant — et on NOMME les coupables, pour que l'opérateur
    // sache quoi arrêter.
    let store_abs = std::fs::canonicalize(store)
        .unwrap_or_else(|_| store.to_path_buf())
        .display()
        .to_string();
    let mut autres: Vec<(u32, String)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir("/proc") {
        for e in rd.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            if pid == std::process::id() {
                continue;
            }
            let Ok(env) = std::fs::read(format!("/proc/{pid}/environ")) else {
                continue;
            };
            let env = String::from_utf8_lossy(&env).replace('\0', "\n");
            if !env.lines().any(|l| l == "OBRAIN_WRITE=1") {
                continue;
            }
            let meme_store = env
                .lines()
                .filter_map(|l| l.strip_prefix("OBRAIN_STORE="))
                .any(|s| s == store_abs || s == store.display().to_string());
            if meme_store {
                let cmd = std::fs::read_to_string(format!("/proc/{pid}/cmdline"))
                    .unwrap_or_default()
                    .replace('\0', " ");
                autres.push((pid, cmd.trim().to_string()));
            }
        }
    }
    if !autres.is_empty() {
        for (pid, cmd) in &autres {
            eprintln!("écrivain concurrent : pid {pid} — {cmd}");
        }
        bail!(
            "{} écrivain(s) concurrent(s) sur {} : la maintenance exige d'être SEUL \
             (deux écrivains shared corrompraient le WAL, constat du 2026-09-01). \
             Arrêter les GraphWriter, puis relancer.",
            autres.len(),
            store.display()
        );
    }

    println!(
        "ouverture en écriture (seul écrivain) : {}",
        store.display()
    );
    let db = ObrainDB::open_shared_write(store)
        .with_context(|| format!("ouverture en écriture de {}", store.display()))?;
    let session = db.session();

    // ── Inventaire : tout nœud étiqueté `Self` ─────────────────────────────
    let res = session.execute("MATCH (n:Self) RETURN n.qualified_name")?;
    let qns: Vec<String> = res
        .rows
        .iter()
        .filter_map(|r| r.first())
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
    println!("nœuds étiquetés `Self` : {}", qns.len());

    let mut a_retirer: Vec<String> = Vec::new();
    for qn in &qns {
        if qn == SOI {
            println!("  GARDÉ   {qn} (le soi)");
            continue;
        }
        if qn.starts_with("self:") {
            println!("  GARDÉ   {qn} (un soi, pas un doublon)");
            continue;
        }
        // GARDE 2 : rattaché à un compte ? C'est une identité, pas un doublon.
        let q = format!(
            "MATCH (n:Self {{qualified_name: '{qn}'}})-[:OWNED_BY]->(a:Account) RETURN a.qualified_name"
        );
        if session
            .execute(&q)
            .map(|r| !r.rows.is_empty())
            .unwrap_or(false)
        {
            println!("  GARDÉ   {qn} (rattaché à un compte : c'est une identité)");
            continue;
        }
        // GARDE 1 : un jumeau correctement étiqueté existe-t-il ?
        //
        // La question est posée au graphe en UNE requête (`WHERE NOT n:Self`) :
        // `RETURN labels(n)` rend une LISTE, pas une chaîne — la lire avec
        // `as_str()` rendait une liste vide, donc « aucun jumeau » pour TOUS les
        // nœuds, y compris le vrai doublon. Fail-closed (rien n'aurait été
        // supprimé), mais un outil qui refuse tout ne répare rien : c'est la
        // répétition sur un store synthétique qui l'a montré.
        let q = format!("MATCH (n {{qualified_name: '{qn}'}}) WHERE NOT n:Self RETURN id(n)");
        let jumeaux: Vec<i64> = session
            .execute(&q)?
            .rows
            .iter()
            .filter_map(|r| r.first())
            .filter_map(|v| v.as_int64())
            .collect();
        if let Some(id_jumeau) = jumeaux.first() {
            println!(
                "  DOUBLON {qn} — jumeau non-`Self` présent (id {id_jumeau}) : le nœud est \
                 conservé sous son vrai label"
            );
            a_retirer.push(qn.clone());
        } else {
            println!(
                "  REFUSÉ  {qn} — AUCUN jumeau sous un autre label : le supprimer perdrait \
                 le nœud (c'est peut-être un soi légitime d'un autre compte)"
            );
        }
    }

    println!();
    println!("à retirer : {} doublon(s)", a_retirer.len());
    if !apply {
        println!("À BLANC — rien n'a été modifié. Relancer avec `--apply` pour exécuter.");
        return Ok(());
    }
    for qn in &a_retirer {
        let q = format!("MATCH (n:Self {{qualified_name: '{qn}'}}) DELETE n");
        match session.execute(&q) {
            Ok(_) => println!("  retiré  {qn}"),
            Err(e) => eprintln!("  ÉCHEC   {qn} : {e}"),
        }
    }
    let apres = session.execute("MATCH (n:Self) RETURN n.qualified_name")?;
    println!("nœuds étiquetés `Self` après : {}", apres.rows.len());
    Ok(())
}
