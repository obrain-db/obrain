//! `obrain viz` — couche de restitution visuelle (P0-v04).
//!
//! * `viz subgraph <store> <center> [--hops N] [--limit N] [--edge-type T…]
//!   [--outbound-only|--inbound-only] [--no-signals] [--format dot|mermaid|json]
//!   [--overlay ricci|energy|roles]` — un appel = un sous-graphe rendable
//!   (PO : `viz_subgraph`) ;
//! * `viz export <store> [--edge-type T…] [--no-signals] [--format …]
//!   [--overlay …]` — export du store entier (PO : `viz_export`).
//!
//! Sortie : l'artefact brut (json-graph v1 / DOT / Mermaid) sur stdout —
//! réutilisable tel quel dans un fichier ou un artifact. Le rapport
//! d'extraction (compteurs, truncation, legende d'overlay) part sur stderr
//! (sauf `--quiet`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Subcommand, ValueEnum};
use obrain_engine::ObrainDB;
use obrain_viz::{
    Overlay, Subgraph, apply_overlay, to_dot, to_graphml, to_json_string, to_mermaid,
};

use crate::viz_extract::{ExtractOptions, ExtractReport};

/// Format d'artefact demandé par `viz` (indépendant du format de sortie global).
#[derive(Clone, Copy, ValueEnum, Default)]
pub enum ArtifactFormat {
    /// json-graph v1 (source de vérité versionnée)
    #[default]
    Json,
    /// Graphviz DOT
    Dot,
    /// Mermaid flowchart
    Mermaid,
    /// GraphML (interop networkx/Gephi/Cytoscape)
    Graphml,
}

/// Overlay CLI → `obrain_viz::Overlay`.
#[derive(Clone, Copy, ValueEnum)]
pub enum OverlayArg {
    /// Courbure Ricci per-edge (persistée) — couleurs divergentes autour de 0
    Ricci,
    /// Énergie nodale (Q1.15) — rampe froide→chaude + taille
    Energy,
    /// Rôles structurels (scc_id) — pastel déterministe par composante
    Roles,
    /// FSRS-5 (stabilité/difficulté d'engram) — VOLATILE, export horodaté
    Fsrs,
    /// Appartenance à un engram — VOLATILE
    Engrams,
    /// Phéromones stigmergy sur arêtes — VOLATILE (normalisation au max du sous-graphe)
    Pheromone,
    /// Tiers mémoire (masse → chaud/tiède/froid).
    Tier,
}

impl From<OverlayArg> for Overlay {
    fn from(value: OverlayArg) -> Self {
        match value {
            OverlayArg::Ricci => Overlay::Ricci,
            OverlayArg::Energy => Overlay::Energy,
            OverlayArg::Roles => Overlay::Roles,
            OverlayArg::Fsrs => Overlay::Fsrs,
            OverlayArg::Engrams => Overlay::Engrams,
            OverlayArg::Pheromone => Overlay::Pheromone,
            OverlayArg::Tier => Overlay::Tier,
        }
    }
}

/// Sous-commandes `viz`.
#[derive(Subcommand)]
pub enum VizCommands {
    /// Sous-graphe rendable autour d'un nœud (id ou qualified_name).
    Subgraph {
        /// Store substrate (répertoire contenant substrate.nodes…)
        path: PathBuf,
        /// Nœud centre : id numérique ou qualified_name exact
        center: String,
        /// Sauts (1..=3)
        #[arg(long, default_value_t = 2)]
        hops: u8,
        /// Cap global de nœuds (défaut 500 — arbitrage V4-F)
        #[arg(long, default_value_t = 500)]
        limit: usize,
        /// Filtre : types d'arêtes inclus (répétable, ex. --edge-type CALLS)
        #[arg(long = "edge-type")]
        edge_types: Vec<String>,
        /// Étendre seulement vers les arêtes sortantes
        #[arg(long)]
        outbound_only: bool,
        /// Étendre seulement vers les arêtes entrantes
        #[arg(long)]
        inbound_only: bool,
        /// Ne pas lire les signaux persistés (scc/energy/ricci/weight)
        #[arg(long)]
        no_signals: bool,
        /// Format d'artefact
        #[arg(long = "as", value_enum, default_value_t = ArtifactFormat::Json)]
        artifact: ArtifactFormat,
        /// Overlay cognitif à attacher (color_hint/size_hint dans tous les exports)
        #[arg(long, value_enum)]
        overlay: Option<OverlayArg>,
    },

    /// Export du store entier en un artefact (nœuds + arêtes vivants).
    Export {
        /// Store substrate
        path: PathBuf,
        /// Filtre : types d'arêtes inclus (répétable)
        #[arg(long = "edge-type")]
        edge_types: Vec<String>,
        /// Ne pas lire les signaux persistés
        #[arg(long)]
        no_signals: bool,
        /// Format d'artefact
        #[arg(long = "as", value_enum, default_value_t = ArtifactFormat::Json)]
        artifact: ArtifactFormat,
        /// Overlay cognitif à attacher
        #[arg(long, value_enum)]
        overlay: Option<OverlayArg>,
    },

    /// Diff structurel entre deux états (fichiers json-graph v1 ou stores) :
    /// deltas datés d'un timeline (V1-B) — ajouts/retraits nœuds et arêtes.
    Compare {
        /// Premier état (fichier .json json-graph v1, ou store)
        a: PathBuf,
        /// Second état (fichier .json json-graph v1, ou store)
        b: PathBuf,
        /// Nombre max d'échantillons par direction (réservé)
        #[arg(long, default_value_t = 20)]
        sample: usize,
    },

    /// Rend l'arbre d'opérateurs d'un plan EXPLAIN (V2-D) en json-graph v1 /
    /// dot / mermaid / graphml — les hints pushdown annotés par le moteur
    /// sont conservés dans les libellés des opérateurs.
    Explain {
        /// Store (base)
        path: PathBuf,
        /// Requête EXPLAIN complète (ex. "EXPLAIN MATCH (n) RETURN n")
        query: String,
        /// Format d'artefact
        #[arg(long = "as", value_enum, default_value_t = ArtifactFormat::Json)]
        artifact: ArtifactFormat,
    },

    /// Timeline d'une séquence de snapshots (fichiers json-graph v1 ou
    /// stores, ordre = chronologie) : deltas datés par étape — base
    /// d'animation d'évolution (V3-E).
    Timeline {
        /// Snapshots ordonnés (≥ 2) : fichier .json json-graph v1 ou store
        files: Vec<PathBuf>,
    },

    /// Tisse les mondes entre eux et PERSISTE la base synaptique de l'agent
    /// (voir `docs/ADR-COSMOS-TOILE-2026-09-10.md`) : chaque synapse est
    /// fondée sur une preuve réelle (entité partagée entre mondes, plan self
    /// ou utilisateur traversant) et datée par `Session.createdAt`.
    Weave {
        /// Monde : `--world "Nom=/chemin/store.obrain"` (répétable, ≥ 2)
        #[arg(long = "world", value_name = "LABEL=PATH")]
        worlds: Vec<String>,
        /// Store de la base synaptique de l'agent (créé si absent)
        #[arg(long)]
        out: PathBuf,
        /// `qualified_name` du Self propriétaire (défaut : self:triviere-dsh)
        #[arg(long = "self", default_value = "self:triviere-dsh")]
        self_qn: String,
        /// Population d'ancres lue par label et par monde
        #[arg(long = "per-label", default_value_t = 400)]
        per_label: usize,
        /// Labels d'ancrage (séparés par des virgules ; vide = défauts)
        #[arg(long = "anchor-labels", default_value = "")]
        anchor_labels: String,
    },

    /// Sert la couche viz en HTTP (V4-G2a) : viewer interactif + endpoint
    /// json-graph v1, ouverture READ-ONLY — bind 127.0.0.1 par défaut.
    Serve {
        /// Store par défaut (chemin direct, requis si pas de preset)
        #[arg(long)]
        store: Option<PathBuf>,
        /// Preset nommé : `--preset "Linux=/chemin/store.obrain"` (répétable)
        #[arg(long = "preset", value_name = "LABEL=PATH")]
        presets: Vec<String>,
        /// Label du preset chargé par défaut
        #[arg(long = "preset-default", value_name = "LABEL")]
        preset_default: Option<String>,
        /// Base synaptique de l'agent : store obrain produit par `viz weave`,
        /// servi par `/api/viz/weave` (vue Cosmos — la toile de l'univers)
        #[arg(long = "synapse-store")]
        synapse_store: Option<PathBuf>,
        /// Entretien de la courbure de Ricci, fait PAR LE SERVEUR dans son propre
        /// processus (minutes entre deux passes ; 0 = désactivé). Indispensable :
        /// le serveur détient le verrou exclusif des bases, donc `obrain enrich
        /// --ricci` lancé à côté échoue tant qu'il tourne.
        #[arg(long = "enrich-ricci", value_name = "MINUTES", default_value_t = 60)]
        enrich_ricci_minutes: u64,
        /// Hôte à écouter
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Port d'écoute
        #[arg(long, default_value_t = 8432)]
        port: u16,
    },
    /// Ajoute un dépôt GitHub comme monde (clone, indexation, enrichissement).
    ///
    /// ── POURQUOI EN LIGNE DE COMMANDE AUSSI ─────────────────────────────────
    /// Ce chemin n'existait que dans le serveur (`POST /api/worlds`) : il n'était
    /// donc ESSAYABLE qu'avec un navigateur, une session IAM et un service en
    /// marche. Un chemin qu'on ne peut pas exercer seul ne se vérifie pas — et
    /// c'est exactement ce qui a laissé passer un ajout de monde SANS
    /// enrichissement : personne n'avait de moyen simple de regarder ce que la
    /// fabrique produisait. La ligne de commande rend la même fabrique
    /// observable : l'avancement étape par étape, puis le compte rendu
    /// d'enrichissement — et le code de sortie dit si l'enrichissement a été
    /// COMPLET ou partiel.
    Monde {
        /// URL du dépôt : `https://github.com/<propriétaire>/<dépôt>`
        url: String,
        /// Libellé du monde (défaut : `<propriétaire>/<dépôt> (GitHub)`)
        #[arg(long)]
        label: Option<String>,
    },
}

/// Point d'entrée du groupe `viz`.
pub fn run(cmd: VizCommands, quiet: bool) -> Result<()> {
    match cmd {
        VizCommands::Subgraph {
            path,
            center,
            hops,
            limit,
            edge_types,
            outbound_only,
            inbound_only,
            no_signals,
            artifact,
            overlay,
        } => {
            let db = open_store(&path)?;
            let opts = ExtractOptions {
                hops,
                closure_max_depth: None, // CLI : closure illimitée (au caller de borner)
                limit_nodes: (limit > 0).then_some(limit),
                edge_types: (!edge_types.is_empty()).then_some(edge_types),
                outbound: !inbound_only, // défaut outbound
                inbound: !outbound_only,
                signals: !no_signals,
                auto_suffix: false, // CLI stricte (ambiguïté = erreur explicite)
            };
            let (sg, report) = crate::viz_extract::extract_subgraph(&db, &center, &opts)
                .with_context(|| format!("viz subgraph {center:?}"))?;
            emit(&sg, overlay.map(Into::into), artifact, quiet)?;
            report_progress(&report, quiet);
            Ok(())
        }
        VizCommands::Export {
            path,
            edge_types,
            no_signals,
            artifact,
            overlay,
        } => {
            let db = open_store(&path)?;
            let opts = ExtractOptions {
                edge_types: (!edge_types.is_empty()).then_some(edge_types),
                signals: !no_signals,
                ..Default::default()
            };
            let (sg, report) = crate::viz_extract::export_all(&db, &opts)
                .with_context(|| "viz export du store entier")?;
            emit(&sg, overlay.map(Into::into), artifact, quiet)?;
            report_progress(&report, quiet);
            Ok(())
        }
        VizCommands::Compare { a, b, sample } => {
            let base = load_graph_state(&a)?;
            let other = load_graph_state(&b)?;
            let diff_json =
                obrain_viz::diff_json_string(&base, &other).context("viz compare : diff JSON")?;
            print!("{diff_json}");
            if !quiet {
                // Compteurs exacts du diff (le JSON imprimé fait foi).
                let d = obrain_viz::diff_subgraphs(&base, &other);
                eprintln!(
                    "viz compare : +{}/-{} nœuds, +{}/-{} arêtes ({} nœuds / {} arêtes inchangés, plafond échantillon {})",
                    d.added_nodes.len(),
                    d.removed_nodes.len(),
                    d.added_edges.len(),
                    d.removed_edges.len(),
                    d.unchanged_nodes,
                    d.unchanged_edges,
                    sample
                );
            }
            Ok(())
        }
        VizCommands::Explain {
            path,
            query,
            artifact,
        } => {
            let db = open_store(&path)?;
            let res = db
                .session()
                .execute(&query)
                .with_context(|| "viz explain : exécution de la requête EXPLAIN")?;
            let text = res
                .rows
                .first()
                .and_then(|row| row.first())
                .and_then(|v| match v {
                    obrain_common::types::Value::String(s) => Some(s.to_string()),
                    _ => None,
                })
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "viz explain : aucune ligne texte — la requête doit être une EXPLAIN (ex. EXPLAIN MATCH (n) RETURN n)"
                    )
                })?;
            let sg = obrain_viz::tree_subgraph_from_explain(&text)
                .context("viz explain : arbre d'opérateurs")?;
            let out = match artifact {
                ArtifactFormat::Json => to_json_string(&sg)?,
                ArtifactFormat::Dot => to_dot(&sg)?,
                ArtifactFormat::Mermaid => to_mermaid(&sg)?,
                ArtifactFormat::Graphml => to_graphml(&sg)?,
            };
            print!("{out}");
            if !quiet {
                eprintln!(
                    "viz explain : {} opérateurs / {} liens PLAN",
                    sg.nodes.len(),
                    sg.edges.len()
                );
            }
            Ok(())
        }
        VizCommands::Weave {
            worlds,
            out,
            self_qn,
            per_label,
            anchor_labels,
        } => {
            let specs: Vec<crate::viz_weave::WorldSpec> = worlds
                .iter()
                .filter_map(|kv| {
                    // Forme : `Nom=/chemin/store.obrain` ou
                    // `Nom=/chemin/store.obrain#/racine/du/territoire`
                    // (la racine est la preuve de tissage par le vécu).
                    let (label, rest) = kv.split_once('=')?;
                    let (path, root) = match rest.split_once('#') {
                        Some((p, r)) => (p, Some(r.trim().to_string())),
                        None => (rest, None),
                    };
                    let (label, path) = (label.trim(), path.trim());
                    (!label.is_empty() && !path.is_empty()).then(|| {
                        crate::viz_weave::WorldSpec {
                            label: label.to_string(),
                            path: std::path::PathBuf::from(path),
                            root: root.filter(|r| !r.is_empty()),
                        }
                    })
                })
                .collect();
            if specs.len() < 2 {
                anyhow::bail!(
                    "viz weave : au moins 2 mondes requis \
                     (--world \"Nom=/chemin/store.obrain\" répétable)"
                );
            }
            let labels: Vec<String> = if anchor_labels.trim().is_empty() {
                crate::viz_weave::DEFAULT_ANCHOR_LABELS
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect()
            } else {
                anchor_labels
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            };
            let opts = crate::viz_weave::WeaveOptions {
                worlds: specs,
                self_qn,
                out,
                per_label,
                anchor_labels: labels,
            };
            let payload = crate::viz_weave::run_weave(&opts, quiet)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&payload)
                    .context("viz weave : sérialisation du rapport")?
            );
            Ok(())
        }
        VizCommands::Serve {
            store,
            presets,
            preset_default,
            synapse_store,
            enrich_ricci_minutes,
            host,
            port,
        } => {
            let presets: Vec<(String, std::path::PathBuf)> = presets
                .into_iter()
                .map(|kv| {
                    let Some((label, path)) = kv.split_once('=') else {
                        return (kv.clone(), std::path::PathBuf::new());
                    };
                    (
                        label.trim().to_string(),
                        std::path::PathBuf::from(path.trim()),
                    )
                })
                .filter(|(l, p)| !l.is_empty() && !p.as_os_str().is_empty())
                .collect();
            let config = crate::viz_serve::ServeConfig {
                default_store: store,
                presets,
                default_label: preset_default,
                synapse_store,
                enrich_ricci_minutes,
            };
            crate::viz_serve::run_serve(config, &host, port, quiet)?;
            Ok(())
        }
        // ── LA MÊME FABRIQUE QUE `POST /api/worlds`, SANS LE SERVEUR ────────
        // Clone, indexation, PUIS enrichissement — et on attend la fin pour
        // rendre le compte rendu. Le code de sortie vaut 1 si l'enrichissement
        // est PARTIEL : un script qui enchaîne des ajouts de mondes doit pouvoir
        // s'arrêter sur un monde dégradé sans lire du texte.
        VizCommands::Monde { url, label } => {
            let id = crate::worlds::lancer(url, label, None)?;
            if !quiet {
                eprintln!("viz monde : travail {id} — clone, indexation, enrichissement");
            }
            let mut derniere_etape = String::new();
            loop {
                let Some(t) = crate::worlds::etat(&id) else {
                    anyhow::bail!("viz monde : travail {id} disparu du registre");
                };
                if t.etape != derniere_etape && !quiet {
                    eprintln!("  · {}", t.etape);
                    derniere_etape = t.etape.clone();
                }
                match t.etat.as_str() {
                    "erreur" => {
                        anyhow::bail!(
                            "viz monde : {} — {}",
                            t.label,
                            t.erreur.unwrap_or_else(|| "échec sans message".into())
                        );
                    }
                    "fini" => {
                        let secs = (t.fin_ms.unwrap_or(t.debut_ms) - t.debut_ms) as f64 / 1000.0;
                        println!(
                            "{} — {} nœuds, {} arêtes, {:.1} s (dont enrichissement {:.1} s)",
                            t.label,
                            t.noeuds.unwrap_or(0),
                            t.aretes.unwrap_or(0),
                            secs,
                            t.enrich_ms.unwrap_or(0) as f64 / 1000.0
                        );
                        println!(
                            "  courbure : {} arête(s) visitée(s), {} mise(s) à jour, max |κ| {:.4}",
                            t.aretes_visitees.unwrap_or(0),
                            t.aretes_curvature.unwrap_or(0),
                            t.courbure_max.unwrap_or(0.0)
                        );
                        match &t.hubs_refus {
                            Some(refus) => println!("  hubs : REFUSÉS — {refus}"),
                            None => println!("  hubs : {} retenu(s)", t.hubs.unwrap_or(0)),
                        }
                        println!(
                            "  index de texte : {}",
                            match &t.trigrammes {
                                Some(p) if !p.is_empty() => p.join(", "),
                                _ => "aucune propriété textuelle déclarée".into(),
                            }
                        );
                        if !t.enrich_erreurs.is_empty() {
                            anyhow::bail!(
                                "viz monde : enrichissement PARTIEL — {}",
                                t.enrich_erreurs.join(" · ")
                            );
                        }
                        return Ok(());
                    }
                    _ => std::thread::sleep(std::time::Duration::from_millis(500)),
                }
            }
        }
        VizCommands::Timeline { files } => {
            if files.len() < 2 {
                anyhow::bail!(
                    "viz timeline : au moins 2 snapshots (fichiers .json ou stores) requis"
                );
            }
            let mut snapshots: Vec<Subgraph> = Vec::with_capacity(files.len());
            for f in &files {
                snapshots.push(load_graph_state(f)?);
            }
            let refs: Vec<&Subgraph> = snapshots.iter().collect();
            let report = obrain_viz::timeline_subgraphs(&refs, 10);
            print!(
                "{}",
                serde_json::to_string_pretty(&report)
                    .context("viz timeline : sérialisation du rapport")?
            );
            if !quiet {
                eprintln!(
                    "viz timeline : {} snapshots, {} étapes",
                    report.snapshots,
                    report.steps.len()
                );
            }
            Ok(())
        }
    }
}

/// Charge un état : fichier json-graph v1 (`.json`) ou store (`export_all`).
fn load_graph_state(path: &Path) -> Result<Subgraph> {
    if path.is_file() && path.extension().is_some_and(|e| e == "json") {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("viz compare : lecture {}", path.display()))?;
        return obrain_viz::from_json_str(&text)
            .with_context(|| format!("viz compare : json-graph invalide {}", path.display()));
    }
    let db = open_store(path)?;
    let (sg, _) = crate::viz_extract::export_all(&db, &ExtractOptions::default())
        .with_context(|| format!("viz compare : export de {}", path.display()))?;
    Ok(sg)
}

fn open_store(path: &Path) -> Result<ObrainDB> {
    if !path.exists() {
        anyhow::bail!(
            "viz: store introuvable : {} (répertoire de base attendu)",
            path.display()
        );
    }
    // Ouverture READ-ONLY (verrou partagé) : la couche viz ne lit jamais —
    // `ObrainDB::open_read_only` ouvre les stores vivants verrouillés par leur
    // écrivain (V4-B) et garantit l'absence d'effet de bord.
    ObrainDB::open_read_only(path)
        .with_context(|| format!("viz: ouverture lecture seule de {}", path.display()))
}

/// Applique l'overlay optionnel puis imprime l'artefact sur stdout.
fn emit(
    sg: &Subgraph,
    overlay: Option<Overlay>,
    format: ArtifactFormat,
    quiet: bool,
) -> Result<()> {
    let styled;
    let target: &Subgraph = if let Some(o) = overlay {
        let (s, rep) = apply_overlay(sg, o);
        if !quiet {
            let volatile = if o.is_volatile() {
                format!(
                    " — VOLATIL (generated_at {})",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or_else(|_| "?".into(), |d| d.as_millis().to_string())
                )
            } else {
                String::new()
            };
            eprintln!(
                "viz: overlay {} — {} nœuds stylés / {} arêtes stylées / {} stale{}{}",
                rep.overlay,
                rep.nodes_styled,
                rep.edges_styled,
                rep.stale_edges,
                volatile,
                if rep.legend.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", rep.legend.join(" ; "))
                }
            );
        }
        styled = s;
        &styled
    } else {
        sg
    };
    let text = match format {
        ArtifactFormat::Json => to_json_string(target)?,
        ArtifactFormat::Dot => to_dot(target)?,
        ArtifactFormat::Mermaid => to_mermaid(target)?,
        ArtifactFormat::Graphml => to_graphml(target)?,
    };
    print!("{text}");
    Ok(())
}

/// Affichage du rapport d'extraction sur stderr (hors `--quiet`).
fn report_progress(report: &ExtractReport, quiet: bool) {
    if quiet {
        return;
    }
    eprintln!(
        "viz: centre {} — {} nœuds / {} arêtes ({} visités){}",
        report.center.0,
        report.nodes,
        report.edges,
        report.visited,
        if report.truncated {
            format!(" (TRONQUÉ, {} nœuds ignorés)", report.dropped)
        } else {
            String::new()
        }
    );
    if let Some(qn) = &report.center_qn {
        eprintln!("viz: qn du centre : {qn}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_arg_maps_to_overlay() {
        assert!(matches!(Overlay::from(OverlayArg::Ricci), Overlay::Ricci));
        assert!(matches!(Overlay::from(OverlayArg::Energy), Overlay::Energy));
        assert!(matches!(Overlay::from(OverlayArg::Roles), Overlay::Roles));
    }
}
