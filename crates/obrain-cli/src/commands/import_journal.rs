//! Journal d'import (RFC docs/rfc/diff-incremental-variantes.md, étape 1).
//!
//! Brique pure-Rust : à partir de DEUX états d'un même dépôt indexé, produire
//! le JOURNAL des changements (nœuds/arêtes ajoutés/retirés) — base de la
//! consommation O(changements) (étapes 2-3). Identité = `qualified_name`
//! normalisé (préfixe projet) ; arêtes en ENSEMBLE (srcQn, type, dstQn).

use std::path::{Path, PathBuf};

use anyhow::Result;
use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::GraphStore;
use obrain_substrate::SubstrateStore;

/// Journal : listes triées et dédupliquées des changements.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ImportJournal {
    pub added_nodes: Vec<String>,
    pub removed_nodes: Vec<String>,
    pub added_edges: Vec<(String, String, String)>,
    pub removed_edges: Vec<(String, String, String)>,
}

impl ImportJournal {
    pub fn len(&self) -> usize {
        self.added_nodes.len()
            + self.removed_nodes.len()
            + self.added_edges.len()
            + self.removed_edges.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Sérialisation textuelle (une ligne par événement).
    pub fn to_lines(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(self.len());
        for qn in &self.added_nodes {
            out.push(format!("+N\t{}", esc(qn)));
        }
        for qn in &self.removed_nodes {
            out.push(format!("-N\t{}", esc(qn)));
        }
        for (s, t, d) in &self.added_edges {
            out.push(format!("+E\t{}\t{}\t{}", esc(s), esc(t), esc(d)));
        }
        for (s, t, d) in &self.removed_edges {
            out.push(format!("-E\t{}\t{}\t{}", esc(s), esc(t), esc(d)));
        }
        out
    }
    pub fn from_lines(lines: &[String]) -> Result<Self> {
        let mut j = ImportJournal::default();
        for l in lines {
            let p: Vec<&str> = l.split('\t').collect();
            match p.as_slice() {
                ["+N", qn] => j.added_nodes.push(unesc(qn)),
                ["-N", qn] => j.removed_nodes.push(unesc(qn)),
                ["+E", s, t, d] => j.added_edges.push((unesc(s), unesc(t), unesc(d))),
                ["-E", s, t, d] => j.removed_edges.push((unesc(s), unesc(t), unesc(d))),
                other => anyhow::bail!("journal: ligne invalide: {other:?}"),
            }
        }
        Ok(j)
    }
}

fn qn_of(store: &SubstrateStore, n: NodeId, prefix: Option<&str>) -> Option<String> {
    let v = store.get_node_property(n, &PropertyKey::new("qualified_name"))?;
    let Value::String(qn) = v else { return None };
    let mut q = qn.to_string();
    if let Some(p) = prefix {
        let pfx = format!("{p}.");
        if q.starts_with(&pfx) {
            q = q[pfx.len()..].to_string();
        }
    }
    Some(q)
}

/// Manifest d'un store : (nœuds triés, arêtes triées) — état de référence.
pub fn manifest(
    store: &SubstrateStore,
    prefix: Option<&str>,
) -> Result<(Vec<String>, Vec<(String, String, String)>)> {
    let ids = GraphStore::node_ids(store);
    let mut hw = 0usize;
    for n in &ids {
        hw = hw.max(n.0 as usize + 1);
    }
    let mut slot_qn: Vec<Option<String>> = vec![None; hw];
    let mut nodes: Vec<String> = Vec::new();
    for n in &ids {
        if let Some(q) = qn_of(store, *n, prefix) {
            slot_qn[n.0 as usize] = Some(q.clone());
            nodes.push(q);
        }
    }
    nodes.sort_unstable();
    nodes.dedup();

    let mut edges: Vec<(String, String, String)> = Vec::new();
    store
        .for_each_live_edge(|slot, rec| {
            let Some(s) = slot_qn.get(rec.src as usize).cloned().flatten() else {
                return;
            };
            let Some(d) = slot_qn.get(rec.dst as usize).cloned().flatten() else {
                return;
            };
            let ty = GraphStore::edge_type(store, obrain_common::types::EdgeId(slot))
                .map(|t| t.to_string())
                .unwrap_or_else(|| "?".to_string());
            edges.push((s, ty, d));
        })
        .map_err(|e| anyhow::anyhow!("journal: scan edges: {e}"))?;
    edges.sort_unstable();
    edges.dedup();
    Ok((nodes, edges))
}

/// Diff entre deux états (listes triées) → journal O(N log N + E log E).
pub fn diff(
    a_nodes: &[String],
    a_edges: &[(String, String, String)],
    b_nodes: &[String],
    b_edges: &[(String, String, String)],
) -> ImportJournal {
    fn d<T: Ord + Clone>(old: &[T], new: &[T]) -> (Vec<T>, Vec<T>) {
        let (mut added, mut removed) = (Vec::new(), Vec::new());
        let (mut i, mut j) = (0usize, 0usize);
        while i < old.len() && j < new.len() {
            match old[i].cmp(&new[j]) {
                std::cmp::Ordering::Less => {
                    removed.push(old[i].clone());
                    i += 1;
                }
                std::cmp::Ordering::Greater => {
                    added.push(new[j].clone());
                    j += 1;
                }
                std::cmp::Ordering::Equal => {
                    i += 1;
                    j += 1;
                }
            }
        }
        removed.extend(old[i..].iter().cloned());
        added.extend(new[j..].iter().cloned());
        (added, removed)
    }
    let (an, rn) = d(a_nodes, b_nodes);
    let (ae, re) = d(a_edges, b_edges);
    ImportJournal {
        added_nodes: an,
        removed_nodes: rn,
        added_edges: ae,
        removed_edges: re,
    }
}

/// Journal entre deux stores réels (tests / import).
pub fn journal_entre_stores(
    a: &SubstrateStore,
    prefix_a: Option<&str>,
    b: &SubstrateStore,
    prefix_b: Option<&str>,
) -> Result<ImportJournal> {
    let (an, ae) = manifest(a, prefix_a)?;
    let (bn, be) = manifest(b, prefix_b)?;
    Ok(diff(&an, &ae, &bn, &be))
}

/// Échappement des champs textuels (les qn peuvent contenir des fins de
/// ligne / tabulations — ex. sources multi-lignes) : format ligne sûr.
fn esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}
fn unesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('\\') => out.push('\\'),
                Some(o) => {
                    out.push('\\');
                    out.push(o);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Manifest sérialisé : lignes `n\t<qn>` puis `e\t<src>\t<type>\t<dst>` (triées).
pub fn manifest_lines(store: &SubstrateStore, prefix: Option<&str>) -> Result<Vec<String>> {
    let (nodes, edges) = manifest(store, prefix)?;
    let mut out = Vec::with_capacity(nodes.len() + edges.len());
    for qn in &nodes {
        out.push(format!("n\t{}", esc(qn)));
    }
    for (s, t, d) in &edges {
        out.push(format!("e\t{}\t{}\t{}", esc(s), esc(t), esc(d)));
    }
    Ok(out)
}

fn parse_manifest(lines: &[String]) -> (Vec<String>, Vec<(String, String, String)>) {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for l in lines {
        let p: Vec<&str> = l.split('\t').collect();
        match p.as_slice() {
            ["n", qn] => nodes.push(unesc(qn)),
            ["e", s, t, d] => edges.push((unesc(s), unesc(t), unesc(d))),
            _ => {}
        }
    }
    (nodes, edges)
}

fn manifest_file(dir: &Path, prefix: Option<&str>) -> PathBuf {
    let sfx = match prefix {
        Some(p) => format!(".{}", fnv(p.as_bytes())),
        None => String::new(),
    };
    dir.join(format!("substrate.import-manifest{sfx}"))
}

fn journal_file(dir: &Path, prefix: Option<&str>) -> PathBuf {
    let sfx = match prefix {
        Some(p) => format!(".{}", fnv(p.as_bytes())),
        None => String::new(),
    };
    dir.join(format!("substrate.import-journal{sfx}"))
}

/// FNV-1a 64 (déterministe, simple).
fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Étape 2 — captation : appelé APRÈS chaque import code.
///   1. manifest du store nouvellement importé ;
///   2. si un manifest précédent existe → diff → `substrate.import-journal`
///      (événements A/D) ;
///   3. rotation atomique du manifest (tmp + rename).
/// Retourne le journal (vide au premier import).
pub fn run_journal_import(dir: &Path, prefix: Option<&str>) -> Result<ImportJournal> {
    // L'import (binding node) peut tenir le store quelques instants après sa
    // fermeture : retries bornés avant d'abandonner (lancement détaché).
    let mut store = None;
    for essai in 0..10 {
        match SubstrateStore::open_read_only(dir) {
            Ok(s) => {
                store = Some(s);
                break;
            }
            Err(e) if essai < 9 => {
                std::thread::sleep(std::time::Duration::from_millis(150));
                let _ = e;
            }
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "journal-import: ouverture {}: {e}",
                    dir.display()
                ));
            }
        }
    }
    let store = store.expect("retry boucle");

    let new_lines = manifest_lines(&store, prefix)?;
    let mf = manifest_file(dir, prefix);
    let jf = journal_file(dir, prefix);
    let journal = if mf.exists() {
        let old_raw = std::fs::read_to_string(&mf)
            .map_err(|e| anyhow::anyhow!("journal-import: lecture manifest: {e}"))?;
        let old_lines: Vec<String> = old_raw.lines().map(str::to_string).collect();
        let (on, oe) = parse_manifest(&old_lines);
        let (nn, ne) = parse_manifest(&new_lines);
        diff(&on, &oe, &nn, &ne)
    } else {
        ImportJournal::default()
    };
    let _ = std::fs::write(&jf, journal.to_lines().join("\n"));
    let tmp = mf.with_extension("tmp");
    let _ = std::fs::write(&tmp, new_lines.join("\n"));
    let _ = std::fs::rename(&tmp, &mf);
    // Étape 3 : préconstruit AUSSI les résumés de comparaison (empreintes +
    // arêtes) — `compare` les réutilise sans rescan tant que le store ne
    // bouge plus (coût payé UNE fois par import, amorti sur N compares).
    let _ = crate::commands::compare::prebuild_summaries(dir, prefix);
    Ok(journal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use obrain_core::graph::{GraphStore as _, GraphStoreMut as _};

    fn ecrire(path: &std::path::Path, qns: &[&str], calls: &[(usize, usize)]) {
        let s = SubstrateStore::create(path).unwrap();
        let mut ids: Vec<NodeId> = Vec::new();
        for (i, qn) in qns.iter().enumerate() {
            let id = s.create_node(&["Function"]);
            s.set_node_property(id, "qualified_name", Value::String((*qn).into()));
            ids.push(id);
        }
        for (i, j) in calls {
            s.create_edge(ids[*i], ids[*j], "CALLS");
        }
        s.flush().unwrap();
    }

    #[test]
    fn journal_detecte_ajouts_retraits_noeuds_et_aretes() {
        let td = tempfile::tempdir().unwrap();
        let pa = td.path().join("a");
        let pb = td.path().join("b");
        ecrire(&pa, &["f1", "f2", "f3"], &[(0, 1)]);
        ecrire(&pb, &["f1", "f2", "f4"], &[(0, 1), (0, 2)]);
        let a = SubstrateStore::open_read_only(&pa).unwrap();
        let b = SubstrateStore::open_read_only(&pb).unwrap();
        let j = journal_entre_stores(&a, None, &b, None).unwrap();
        assert_eq!(j.added_nodes, vec!["f4"]);
        assert_eq!(j.removed_nodes, vec!["f3"]);
        assert_eq!(
            j.added_edges,
            vec![("f1".into(), "CALLS".into(), "f4".into())]
        );
        assert!(j.removed_edges.is_empty());
        assert!(journal_entre_stores(&a, None, &a, None).unwrap().is_empty());
    }

    #[test]
    fn journal_roundtrip_lignes() {
        let j = ImportJournal {
            added_nodes: vec!["x".into()],
            removed_nodes: vec!["y".into()],
            added_edges: vec![("s".into(), "T".into(), "d".into())],
            removed_edges: vec![("s2".into(), "T".into(), "d2".into())],
        };
        let j2 = ImportJournal::from_lines(&j.to_lines()).unwrap();
        assert_eq!(j, j2);
    }

    /// Les qn multi-lignes (sources) ne doivent pas casser le format lignes.
    #[test]
    fn manifest_supporte_les_qn_multilignes() {
        let td = tempfile::tempdir().unwrap();
        let pa = td.path().join("s");
        {
            let s = SubstrateStore::create(&pa).unwrap();
            let n = s.create_node(&["Function"]);
            s.set_node_property(n, "qualified_name", Value::String("x\ny".into()));
            let m = s.create_node(&["Function"]);
            s.set_node_property(m, "qualified_name", Value::String("z".into()));
            s.create_edge(n, m, "T\tU");
            s.flush().unwrap();
        }
        let j1 = run_journal_import(&pa, None).unwrap();
        assert!(j1.is_empty());
        // Deuxième passe sur contenu identique : PAS d'oscillation.
        let j2 = run_journal_import(&pa, None).unwrap();
        assert!(j2.is_empty(), "store inchangé → journal vide");
    }

    /// Étape 2 — premier import = journal vide + manifest écrit ; un second
    /// import (contenu changé) produit les événements attendus.
    #[test]
    fn captation_premier_puis_second_import() {
        let td = tempfile::tempdir().unwrap();
        let pa = td.path().join("s");
        ecrire(&pa, &["f1", "f2"], &[(0, 1)]);
        let j1 = run_journal_import(&pa, None).unwrap();
        assert!(j1.is_empty(), "premier import : pas de journal");
        assert!(manifest_file(&pa, None).exists());
        // Étape 3 : l'import préconstruit AUSSI les résumés de comparaison.
        let dir = std::fs::read_dir(&pa).unwrap();
        let names: Vec<String> = dir
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            names
                .iter()
                .any(|n| n.starts_with("substrate.compare-hash")),
            "empreintes préconstruites par l'import"
        );
        assert!(
            names
                .iter()
                .any(|n| n.starts_with("substrate.compare-edges")),
            "résumé d'arêtes préconstruit par l'import"
        );
        // « Re-import » : même chemin, contenu différent — le sidecar
        // manifest SURVIT à la reconstruction du store (l'outil d'import
        // préserve `substrate.import-manifest*` entre deux imports).
        let old_manifest = std::fs::read(&manifest_file(&pa, None)).unwrap();
        let _ = std::fs::remove_dir_all(&pa);
        ecrire(&pa, &["f1", "f3"], &[(0, 1)]);
        std::fs::write(manifest_file(&pa, None), old_manifest).unwrap();
        let j2 = run_journal_import(&pa, None).unwrap();
        assert_eq!(j2.added_nodes, vec!["f3"]);
        assert_eq!(j2.removed_nodes, vec!["f2"]);
        assert_eq!(
            j2.added_edges,
            vec![("f1".into(), "CALLS".into(), "f3".into())]
        );
        assert_eq!(
            j2.removed_edges,
            vec![("f1".into(), "CALLS".into(), "f2".into())]
        );
        let jf = journal_file(&pa, None);
        assert!(jf.exists());
        let parsed = ImportJournal::from_lines(
            &std::fs::read_to_string(&jf)
                .unwrap()
                .lines()
                .map(str::to_string)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert_eq!(parsed, j2);
    }

    #[test]
    fn journal_diff_est_la_difference_symetrique() {
        let mut st: u64 = 0xAB_CD;
        let mut rnd = move || {
            st ^= st << 13;
            st ^= st >> 7;
            st ^= st << 17;
            st
        };
        for graine in 0..25u64 {
            st = 0xAB_CD ^ graine;
            let mut nodes = |extra: &str| -> Vec<String> {
                let n = 5 + (rnd() % 12) as usize;
                let mut v: Vec<String> = (0..n).map(|i| format!("f{graine}_{i}{extra}")).collect();
                v.push(format!("{extra}z{graine}"));
                v.sort();
                v.dedup();
                v
            };
            let an = nodes("");
            let bn = nodes("b");
            let mut mk_edges = |ns: &[String]| -> Vec<(String, String, String)> {
                let mut e = Vec::new();
                for _ in 0..(3 + rnd() % 8) {
                    let a = (rnd() % ns.len() as u64) as usize;
                    let b = (rnd() % ns.len() as u64) as usize;
                    if a != b {
                        e.push((ns[a].clone(), "T".into(), ns[b].clone()));
                    }
                }
                e.sort();
                e.dedup();
                e
            };
            let ae = mk_edges(&an);
            let be = mk_edges(&bn);
            let j = diff(&an, &ae, &bn, &be);
            let sa: std::collections::HashSet<_> = an.iter().collect();
            let sb: std::collections::HashSet<_> = bn.iter().collect();
            assert_eq!(
                j.added_nodes.len(),
                sb.difference(&sa).count(),
                "graine {graine}"
            );
            assert_eq!(
                j.removed_nodes.len(),
                sa.difference(&sb).count(),
                "graine {graine}"
            );
            assert_eq!(j.len(), j.to_lines().len(), "graine {graine}");
            assert_eq!(
                ImportJournal::from_lines(&j.to_lines()).unwrap(),
                j,
                "graine {graine}"
            );
        }
    }
}
