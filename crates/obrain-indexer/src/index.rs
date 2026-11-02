//! Orchestration d'une indexation complète (RFC M6 §2.3).
//!
//! Le pipeline reproduit l'ordre de l'indexeur C :
//! structure (Project/Folder/File) → extract (définitions/appels/imports par
//! fichier via la grammaire du langage) → (resolve : à brancher) →
//! (predump : similarity/semantic/complexity/hyperedges — à brancher).
//!
//! L'écriture substrate (bulk zone-direct) est une étape ultérieure (RFC M6
//! §5 point 4) : `index_repository` produit un [`crate::graph::CodeGraph`]
//! en mémoire.

use std::path::Path;

use crate::discovery::{DiscoveryOptions, Exclusion, discover};
use crate::graph::{CodeGraph, NodeProps};
use crate::lang;
use crate::schema::{EdgeKind, NodeKind};
use rayon::prelude::*;

/// Options d'une indexation (alignées sur les modes C full/moderate/fast).
#[derive(Debug, Clone, Default)]
pub struct IndexOptions {
    pub discovery: DiscoveryOptions,
    /// Mode rapide : saute similarity/semantic/githistory (mode C « fast »).
    pub fast: bool,
    /// (T27) Dossier du cache d'extraction. `None` → `$OBRAIN_INDEX_CACHE_DIR` ;
    /// ni l'un ni l'autre → pas de cache. Voir `cache_extraction`.
    pub cache_dir: Option<std::path::PathBuf>,
}

/// Knob de mesure (chantier « résolution sémantique », étape 1) : autoriser
/// le repli « résolution par nom seul » quand un appel a un qualifier qui
/// n'a pas été résolu en type de récepteur. Les deux variantes sont mesurées
/// par le bench gardien (`scripts/bench-verify/semantic-resolution.sh`) —
/// `false` = pas d'arête inventée au nom seul quand le récepteur est inconnu
/// (les récepteurs non typés résolus par unique/suffix au nom seul sont une
/// source de faux positifs : `self.inner.x()` → méthode homonyme d'un autre
/// type).
const NAME_FALLBACK_ON_UNRESOLVED_QUALIFIER: bool = true;

/// Knob (étape 2 avancée) : isoler AUSSI les fichiers rust non mod-root de
/// `src/` comme modules de fichier (chaque `x.rs` est son propre scope —
/// fidèle au Rust, supprime les collisions de FQN entre frères de répertoire
/// et les résolutions same-module qui franchissent les vrais scopes).
const ISOLATE_SRC_FILE_MODULES: bool = true;

/// Le fichier est-il du Rust (extension .rs) ?
fn ex_lang_is_rust(file: &Path) -> bool {
    file.extension().and_then(|e| e.to_str()) == Some("rs")
}

/// Indexe un dépôt et retourne le graphe de code en mémoire.
///
/// Pipeline (RFC M6 §2.3) :
/// 1. **structure** — Project/Folder/File + CONTAINS_FILE.
/// 2. **extract** — parse chaque fichier reconnu (grammaire du langage) :
///    nœuds de définitions + DEFINES/DEFINES_METHOD + registre des symboles
///    (module → FQN) + calls bruts mémorisés par module appelant.
/// 3. **resolve** — après extraction complète, résout chaque call via le
///    registre (chaîne de confiance : même module → nom unique → suffixe) et
///    crée les arêtes CALLS (quand le callee est résolu).
pub fn index_repository(root: &Path, opts: &IndexOptions) -> IndexOutcome {
    // Sondes de phase (T27) : `OBRAIN_INDEX_PROF=1` — on ne raccourcit pas un
    // pipeline sans savoir laquelle de ses étapes coûte.
    let prof = std::env::var("OBRAIN_INDEX_PROF").is_ok();
    let mut t_etape = std::time::Instant::now();
    let mut cpu_etape = crate::temps_cpu("self");
    let mut sonde = |nom: &str| {
        if prof {
            let cpu = crate::temps_cpu("self");
            eprintln!(
                "[prof]     indexeur · {nom} : {:.2}s mur · {:.2}s CPU",
                t_etape.elapsed().as_secs_f64(),
                cpu - cpu_etape
            );
            cpu_etape = cpu;
        }
        t_etape = std::time::Instant::now();
    };
    let mut warnings = Vec::new();
    let registre = lang::build_registry();

    // Nom de projet : chemin absolu, `/` → `.` (contrat C, fqn.c).
    let project_name = project_name_from_path(root);

    let disco = discover(root, &opts.discovery);
    sonde("découverte");

    // ── Phase 1 PARALLÈLE : structure + extraction (map) ────────────────
    // Chaque thread de rayon extrait ses fichiers localement (parser
    // thread_local — pas de Mutex global qui sérialiserait). Le résultat par
    // fichier est une structure SENDABLE : FQN du fichier, définitions avec
    // FQN déjà résolus, imports/calls pour la phase 2.
    /// Nom court du langage depuis l'extension (pour is_test_function).
    fn lang_for_ext(ext: &str) -> &'static str {
        match ext {
            "rs" => "rust",
            "py" => "python",
            "go" => "go",
            "js" | "jsx" | "mjs" | "cjs" => "javascript",
            "ts" | "tsx" => "typescript",
            "java" => "java",
            "c" | "h" => "c",
            _ => "",
        }
    }

    struct FichierExtrait {
        file_qn: String,
        file_rel: String,
        module_qn: String,
        /// Chemin rust-rel du module du fichier (ex: `query.optimizer`) —
        /// pour la résolution des qualifiers `crate::`/`super::`/`self::`.
        rust_rel: String,
        /// Le fichier EST le module du répertoire (mod.rs/lib.rs/main.rs).
        file_is_mod_root: bool,
        ex: crate::parse::FileExtraction,
        /// (T27) Ce que la fusion calculait séquentiellement pour chaque
        /// définition — FQN, propriétés, type de retour — préparé ici, dans la
        /// phase PARALLÈLE. Aligné sur `ex.definitions` (même ordre).
        prepares: Vec<DefPrete>,
        /// (T27) Empreinte du contenu — clé du cache d'extraction.
        empreinte: u128,
    }

    struct DefPrete {
        fqn: String,
        props: NodeProps,
        cls_qn: Option<String>,
        ret: Option<String>,
    }

    // (T27) Cache d'extraction : un fichier au contenu inchangé n'est pas
    // réanalysé (voir `cache_extraction`).
    let cache = crate::cache_extraction::Cache::charger(root, opts.cache_dir.as_deref());
    let analyses = std::sync::atomic::AtomicUsize::new(0);
    let mut extraits: Vec<FichierExtrait> = disco
        .files
        .par_iter()
        .filter_map(|file| {
            // FQN du fichier : project.<rel complet avec extension>.__file__.
            let rel = file
                .strip_prefix(root)
                .unwrap_or(file)
                .to_string_lossy()
                .replace('/', ".");
            let file_qn = format!("{project_name}.{rel}.__file__");
            let Ok(source) = std::fs::read_to_string(file) else {
                return None;
            };
            let lang = registre.language_for_path(file)?;
            let empreinte = crate::cache_extraction::empreinte(source.as_bytes());
            let ex = cache
                .as_ref()
                .and_then(|c| c.prendre(&file.to_string_lossy(), empreinte))
                .unwrap_or_else(|| {
                    analyses.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    lang.extract(&source, file)
                });
            let base = file
                .file_name()
                .map(|b| b.to_string_lossy().to_string())
                .unwrap_or_default();
            let file_is_mod_root = matches!(base.as_str(), "mod.rs" | "lib.rs" | "main.rs");
            // Chantier « résolution sémantique », étape 2 — isolation par
            // fichier : chaque fichier qui EST un module (crate tests/benches,
            // ou fichier rust non mod-root — un fichier x.rs est son propre
            // scope de module, jamais confondu avec ses frères de répertoire)
            // porte le stem dans son module. Sans cela, deux fichiers d'un
            // même répertoire définissant le même nom collisionnent sur le
            // même FQN (nœud fusionné → arêtes attribuées au mauvais fichier)
            // et la résolution same-module franchit les scopes réels.
            let is_file_scope = isolation_scope(root, file).is_some()
                || (ex_lang_is_rust(file) && !file_is_mod_root && ISOLATE_SRC_FILE_MODULES);
            let module_qn = if is_file_scope {
                module_qn_per_file(&project_name, root, file)
            } else {
                module_qn(&project_name, root, file)
            };
            // rust-rel : répertoires du fichier SOUS le premier répertoire
            // `src` du dépôt (la racine du crate rust). Pour un module de
            // fichier isolé sous src, le stem est ajouté (`src/query/cache.rs`
            // → `query.cache`) — les imports `crate::query::cache::…`
            // résolvent alors vers le bon module M6. Les fichiers hors src
            // (tests/benches/exemples : crates séparées) gardent un rust_rel
            // vide — ils ne participent pas aux chemins crate::.
            let mut rust_rel = String::new();
            if let Ok(rel_path) = file.strip_prefix(root) {
                let comps: Vec<String> = rel_path
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().to_string())
                    .collect();
                let mut in_src = false;
                let mut parts: Vec<String> = Vec::new();
                for (i, c) in comps.iter().enumerate() {
                    if c == "src" {
                        in_src = true;
                        continue;
                    }
                    if in_src && i + 1 < comps.len() {
                        parts.push(c.clone());
                    }
                }
                if in_src && is_file_scope && !file_is_mod_root {
                    // stem ajouté au chemin rust-rel (module de fichier).
                    let stem = file
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if !stem.is_empty() {
                        parts.push(stem);
                    }
                }
                rust_rel = parts.join(".");
            }
            let file_rel = file.to_string_lossy().to_string();
            let file_is_test = crate::tests_detect::is_test_file(&file_rel);
            let file_lang = file_rel.rsplit('.').next().unwrap_or("");
            let prepares: Vec<DefPrete> = ex
                .definitions
                .iter()
                .map(|def| {
                    let fqn = match &def.parent_class {
                        Some(cls) => format!("{}.{}.{}", module_qn, cls, def.name),
                        None => format!("{}.{}", module_qn, def.name),
                    };
                    let mut props = NodeProps {
                        name: Some(def.name.clone()),
                        file_path: Some(file_rel.clone()),
                        start_line: Some(def.start_line),
                        end_line: Some(def.end_line),
                        ..Default::default()
                    };
                    if let Some(sig) = &def.signature {
                        props
                            .extra
                            .insert("signature".into(), serde_json::Value::String(sig.clone()));
                    }
                    if let Some(pc) = &def.parent_class {
                        props
                            .extra
                            .insert("parent_class".into(), serde_json::Value::String(pc.clone()));
                    }
                    if let Some(doc) = &def.docstring {
                        props
                            .extra
                            .insert("docstring".into(), serde_json::Value::String(doc.clone()));
                    }
                    let est_test = file_is_test
                        || crate::tests_detect::is_test_function(
                            &def.name,
                            lang_for_ext(file_lang),
                        );
                    props
                        .extra
                        .insert("is_test".into(), serde_json::Value::Bool(est_test));
                    props.extra.insert(
                        "is_exported".into(),
                        serde_json::Value::Bool(crate::tests_detect::is_exported(
                            &def.name,
                            lang_for_ext(file_lang),
                        )),
                    );
                    if let Some(m) = &def.metrics {
                        props
                            .extra
                            .insert("complexity".into(), serde_json::Value::from(m.complexity));
                        props
                            .extra
                            .insert("cognitive".into(), serde_json::Value::from(m.cognitive));
                        props
                            .extra
                            .insert("loop_count".into(), serde_json::Value::from(m.loop_count));
                        props
                            .extra
                            .insert("loop_depth".into(), serde_json::Value::from(m.loop_depth));
                        props.extra.insert(
                            "max_access_depth".into(),
                            serde_json::Value::from(m.max_access_depth),
                        );
                    }
                    let cls_qn = def
                        .parent_class
                        .as_ref()
                        .map(|p| format!("{}.{}", module_qn, p));
                    let ret = def
                        .signature
                        .as_deref()
                        .and_then(crate::resolve::SymbolRegistry::return_type_of_signature);
                    DefPrete {
                        fqn,
                        props,
                        cls_qn,
                        ret,
                    }
                })
                .collect();
            Some(FichierExtrait {
                file_qn,
                file_rel,
                module_qn,
                rust_rel,
                file_is_mod_root,
                ex,
                prepares,
                empreinte,
            })
        })
        .collect();

    let files_scanned = extraits.len();
    let analyses = analyses.into_inner();
    if prof {
        eprintln!(
            "[prof]     indexeur · cache d'extraction : {} fichiers servis, {} analysés{}",
            files_scanned - analyses,
            analyses,
            if cache.is_some() {
                ""
            } else {
                " (pas de cache utilisable)"
            }
        );
    }
    drop(cache);
    sonde("extraction (parallèle)");

    // ── Fusion (reduce) : graphe structure + registre des symboles ──────
    let n_defs: usize = extraits.iter().map(|fe| fe.ex.definitions.len()).sum();
    let n_calls: usize = extraits.iter().map(|fe| fe.ex.calls.len()).sum();
    let mut graph = CodeGraph::with_capacity(n_defs + disco.files.len() + 64, n_defs + n_calls);
    let mut symbol_registry = crate::resolve::SymbolRegistry::new();
    let project_id = graph.upsert_node(
        NodeKind::Project,
        project_name.clone(),
        NodeProps {
            name: Some(project_name.clone()),
            ..Default::default()
        },
    );

    // ── Contexte git : nœud Branch + HAS_BRANCH (tissage code ↔ historique) ──
    let git_ctx = crate::git_ctx::capture(root);
    sonde("contexte git");
    if git_ctx.is_git {
        let branch_qn = format!("{project_name}:branch:{}", git_ctx.branch);
        let mut bprops = NodeProps {
            name: Some(git_ctx.branch.clone()),
            ..Default::default()
        };
        // Propriétés git (mêmes clés que le C : base_sha, head_sha, …).
        for (k, v) in [
            ("base_sha", git_ctx.base_sha.clone()),
            ("head_sha", git_ctx.head_sha.clone()),
            ("branch", git_ctx.branch.clone()),
            ("git_dir", git_ctx.git_dir.clone()),
            ("git_common_dir", git_ctx.git_common_dir.clone()),
        ] {
            if !v.is_empty() {
                bprops.extra.insert(k.into(), serde_json::Value::String(v));
            }
        }
        if git_ctx.is_detached {
            bprops
                .extra
                .insert("is_detached".into(), serde_json::Value::Bool(true));
        }
        if git_ctx.is_worktree {
            bprops
                .extra
                .insert("is_worktree".into(), serde_json::Value::Bool(true));
        }
        let branch_id = graph.upsert_node(NodeKind::Branch, branch_qn, bprops);
        graph.insert_edge(
            EdgeKind::HasBranch,
            project_id,
            branch_id,
            Default::default(),
        );
    }

    for fe in extraits.iter_mut() {
        let prepares = std::mem::take(&mut fe.prepares);
        let fe = &*fe;
        // (2026-09-09) NOM AFFICHABLE = BASENAME RÉEL du fichier, PAS le
        // dernier segment du qn. Le qn d'un fichier finit par la sentinelle
        // d'ancrage `.__file__` (convention historique, héritée du contrat C :
        // project.<rel complet>.__file__) — en prendre le dernier segment
        // donnait `__file__` pour CHAQUE fichier indexé (14 449 nœuds File
        // nommés `__file__` dans le graphe). Le basename (`mcp_server.rs`)
        // est la valeur que les consommateurs (UI, exports json-graph,
        // search) affichent pour `name` ; le qn d'ancrage, lui, ne change pas.
        let file_base = fe
            .file_rel
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string();
        let file_id = graph.upsert_node(
            NodeKind::File,
            fe.file_qn.clone(),
            NodeProps {
                name: Some(file_base),
                file_path: Some(fe.file_rel.clone()),
                ..Default::default()
            },
        );
        graph.insert_edge(
            EdgeKind::ContainsFile,
            project_id,
            file_id,
            Default::default(),
        );

        for (def, prete) in fe.ex.definitions.iter().zip(prepares) {
            let DefPrete {
                fqn,
                props,
                cls_qn,
                ret,
            } = prete;
            let sym_id = graph.upsert_node(def.kind, fqn.clone(), props);
            graph.insert_edge(EdgeKind::Defines, file_id, sym_id, Default::default());
            if def.parent_class.is_some() {
                // DEFINES_METHOD : classe → méthode (contrat C). Avec les
                // modules de fichier (étape 2), l'impl peut vivre dans un
                // fichier différent de la struct (`impl ObrainDB` dans
                // crud.rs, struct dans mod.rs) : le FQN « module du fichier »
                // ne matche plus → repli sur le type parent unique global.
                let parent = def.parent_class.as_deref().unwrap();
                let cls_qn = cls_qn.as_deref().unwrap_or_default();
                let cls: Option<crate::graph::NodeId> = graph
                    .node_by_qn(cls_qn)
                    .map(|n| n.id)
                    // (T27) index par dernier segment — même verdict que le
                    // balayage complet qu'il remplace (voir CodeGraph).
                    .or_else(|| graph.type_unique_par_suffixe(parent));
                if let Some(cls) = cls {
                    graph.insert_edge(EdgeKind::DefinesMethod, cls, sym_id, Default::default());
                }
            }
            // Registre des symboles pour la résolution — avec le rust-rel du
            // module (qualifiers crate/super), le type parent éventuel
            // (méthodes) et la nature « type » (passe récepteur).
            let is_type = matches!(
                def.kind,
                NodeKind::Struct
                    | NodeKind::Interface
                    | NodeKind::Enum
                    | NodeKind::Type
                    | NodeKind::Class
            );
            symbol_registry.register_full(
                &fe.module_qn,
                Some(&fe.rust_rel),
                def.parent_class.as_deref(),
                &def.name,
                fqn.clone(),
                is_type,
            );
            // Type de retour des méthodes (cascade de bindings récepteur :
            // `let s = db.session(); s.execute(…)`).
            if let (Some(parent), Some(ret)) = (def.parent_class.as_deref(), ret.as_deref()) {
                symbol_registry.register_method_return(&fe.module_qn, parent, &def.name, ret);
            }
            // Type de retour des FONCTIONS LIBRES (étape 2 du chantier :
            // `fn setup() -> ObrainDB` puis `let db = setup(); db.…()`).
            if def.parent_class.is_none()
                && def.kind == NodeKind::Function
                && let Some(ret) = ret.as_deref()
            {
                symbol_registry.register_fn_return(&fqn, ret);
            }
        }
        // Champs de struct (étape 2) : registre (module, struct) → [(champ,
        // type)] pour la résolution `self.field.method()`.
        for (sname, fname, fty) in &fe.ex.struct_fields {
            symbol_registry.register_struct_field(&fe.module_qn, sname, fname, fty);
        }
    }

    sonde("fusion (séquentielle)");
    // ── Phase 2 PARALLÈLE : résolution des appels → arêtes CALLS ────────
    // Chaque call résolu porte sa provenance (strategy/confidence/line/
    // caller_scope) — le prérequis « apprentissage » du chantier : une arête
    // est traçable vers la stratégie qui l'a émise. Le graphe et le registre
    // sont lus seuls (Sync) ; le reduce insère les arêtes séquentiellement.
    //
    // Anti-bruit (étape 1) :
    // - self-loop : un call résolu vers le def qui le contient (même FQN) est
    //   conservé UNIQUEMENT pour la récursion directe par nom nu
    //   (`fn f() { f() }`) ; les `self.x()` / `self.inner.x()` résolus vers
    //   l'appelant par résolution par nom sont des artefacts → filtrés.
    // - récepteur typé : passe « récepteur » d'abord (confiance élevée) ;
    //   le repli « nom seul » pour un qualifier non résolu est piloté par un
    //   knob (les deux variantes sont mesurées par le bench gardien).
    //
    /// Résultat de résolution d'un fichier : arêtes + compteurs d'anti-bruit.
    struct ResolutionFile {
        edges: Vec<CallEdge>,
        self_loops_filtres: usize,
        self_loops_gardes: usize,
        /// (T27) Les arêtes RÉSOLUES en identifiants, propriétés construites —
        /// préparées dans la phase parallèle. La boucle séquentielle ne fait
        /// plus que les insérer. Le graphe ne reçoit aucun nœud entre les deux :
        /// `node_by_qn` y rend donc exactement ce que rendait `link_qns`.
        liens: Vec<(
            crate::graph::NodeId,
            crate::graph::NodeId,
            crate::graph::EdgeProps,
            &'static str,
        )>,
        non_resolus: usize,
    }
    /// Une arête CALLS résolue, avec provenance.
    #[derive(Clone)]
    struct CallEdge {
        caller_qn: String,
        target_qn: String,
        strategy: &'static str,
        confidence: f64,
        line: u32,
    }
    let resolutions: Vec<ResolutionFile> = extraits
        .par_iter()
        .map(|fe| {
            let ex = &fe.ex;
            let module_qn = &fe.module_qn;
            let mut out = ResolutionFile {
                edges: Vec::new(),
                self_loops_filtres: 0,
                self_loops_gardes: 0,
                liens: Vec::new(),
                non_resolus: 0,
            };
            if ex.calls.is_empty() {
                return out;
            }
            // Scopes des defs du fichier : (start, end, fqn, nom, parent).
            let mut scopes: Vec<(u32, u32, String, String, Option<String>)> = Vec::new();
            for def in &ex.definitions {
                let fqn = match &def.parent_class {
                    Some(cls) => format!("{module_qn}.{cls}.{}", def.name),
                    None => format!("{module_qn}.{}", def.name),
                };
                scopes.push((
                    def.start_line,
                    def.end_line,
                    fqn,
                    def.name.clone(),
                    def.parent_class.clone(),
                ));
            }
            // Bindings triés par ligne (ordre source) — la passe récepteur
            // filtre elle-même par ligne d'appel / début de def.
            let mut bindings: Vec<&crate::parse::Binding> = ex.bindings.iter().collect();
            bindings.sort_by_key(|b| b.line);
            let imports_modules: Vec<String> =
                ex.imports.iter().map(|i| i.module.clone()).collect();
            for call in &ex.calls {
                let name = &call.name;
                // Def contenant l'appel (attribution du caller). Le def le
                // plus PROFOND (start_line max) : un call dans `mod tests {
                // fn test_x() { … } }` appartient à test_x, pas au mod —
                // attribuer au mod (défaut historique min_by_key) envoyait les
                // appels des fns imbriquées vers le nœud Module (perdus pour
                // les arêtes fonctionnelles).
                let scope = scopes
                    .iter()
                    .filter(|(s, e, ..)| *s <= call.line && call.line <= *e)
                    .max_by_key(|(s, _, _, _, _)| *s);
                let caller_qn = match scope {
                    Some((_, _, fqn, _, _)) => fqn.clone(),
                    None => module_qn.clone(),
                };
                let caller_parent = scope.and_then(|(_, _, _, _, p)| p.clone());
                let scope_start = scope.map(|(s, _, _, _, _)| *s);
                let ctx = crate::resolve::CallContext {
                    module_qn,
                    rust_rel: &fe.rust_rel,
                    file_is_mod_root: fe.file_is_mod_root,
                    caller_parent: caller_parent.as_deref(),
                    imports: &ex.imports,
                    bindings: &bindings,
                    call_line: call.line,
                    scope_start,
                };
                let resolution = match &call.qualifier {
                    Some(q) => {
                        let recv = symbol_registry.resolve_receiver(name, q, &ctx);
                        if recv.is_some() {
                            recv
                        } else if NAME_FALLBACK_ON_UNRESOLVED_QUALIFIER {
                            symbol_registry.resolve(name, module_qn, &imports_modules)
                        } else {
                            None
                        }
                    }
                    None => symbol_registry.resolve(name, module_qn, &imports_modules),
                };
                let Some(res) = resolution else { continue };
                // Anti-bruit étape 2 (retours de fonctions libres) : quand la
                // méthode résolue par retour de fonction libre n'est qu'un
                // SAUT INTERMÉDIAIRE de cascade — un binding postérieur
                // consomme exactement `base.name()` comme récepteur d'init
                // (`let s = db.session(); s.execute(…)`) — l'indexeur C ne
                // persiste PAS d'arête vers ce saut (il n'enregistre que le
                // dispatch terminal). Mesuré : ces arêtes receiver_fn_return
                // sont 484/484 inventées (chute de précision ~5 pts) → on
                // s'abstient. Le dispatch terminal (execute) passe par la
                // cascade method_return (stratégie receiver_binding) et reste.
                if res.strategy == "receiver_fn_return"
                    && let Some(q) = &call.qualifier
                {
                    let base = q.split('.').next().unwrap_or(q).trim();
                    let hop = bindings.iter().any(|b2| {
                        b2.line >= call.line
                            && scope_start.map(|s| b2.line >= s).unwrap_or(true)
                            && b2
                                .init_call
                                .as_ref()
                                .is_some_and(|(r, m)| r == base && m == name)
                    });
                    if hop {
                        continue;
                    }
                }
                let target_qn = res.target_qn.clone();
                if caller_qn == target_qn {
                    // Self-loop : garde uniquement la récursion directe par
                    // nom nu (`fn f() { f() }`) — les self.x() mal résolus
                    // vers l'appelant sont des artefacts (mesure : C n'émet
                    // AUCUN self-loop).
                    let is_direct_recursion = call.qualifier.is_none()
                        && scope
                            .map(|(_, _, _, def_name, _)| def_name == name)
                            .unwrap_or(false);
                    if is_direct_recursion {
                        out.self_loops_gardes += 1;
                    } else {
                        out.self_loops_filtres += 1;
                        continue;
                    }
                }
                out.edges.push(CallEdge {
                    caller_qn,
                    target_qn,
                    strategy: res.strategy,
                    confidence: res.confidence.as_f32() as f64,
                    line: call.line,
                });
            }
            for edge in std::mem::take(&mut out.edges) {
                let ids = graph
                    .node_by_qn(&edge.caller_qn)
                    .map(|n| n.id)
                    .zip(graph.node_by_qn(&edge.target_qn).map(|n| n.id));
                let Some((src, dst)) = ids else {
                    out.non_resolus += 1;
                    continue;
                };
                let mut props = crate::graph::EdgeProps::default();
                props.extra.insert(
                    "strategy".into(),
                    serde_json::Value::String(edge.strategy.into()),
                );
                props.extra.insert(
                    "confidence".into(),
                    serde_json::Value::from(edge.confidence),
                );
                props
                    .extra
                    .insert("line".into(), serde_json::Value::from(edge.line));
                props.extra.insert(
                    "caller_scope".into(),
                    serde_json::Value::String(edge.caller_qn),
                );
                out.liens.push((src, dst, props, edge.strategy));
            }
            out
        })
        .collect();

    let mut calls_resolus = 0usize;
    let mut calls_non_resolus = 0usize;
    let mut self_loops_filtres = 0usize;
    let mut self_loops_gardes = 0usize;
    let mut par_strategie: std::collections::HashMap<&'static str, usize> =
        std::collections::HashMap::new();
    sonde("résolution (parallèle)");
    for fr in resolutions {
        self_loops_filtres += fr.self_loops_filtres;
        self_loops_gardes += fr.self_loops_gardes;
        calls_non_resolus += fr.non_resolus;
        for (src, dst, props, strategie) in fr.liens {
            graph.insert_edge(EdgeKind::Calls, src, dst, props);
            calls_resolus += 1;
            *par_strategie.entry(strategie).or_insert(0) += 1;
        }
    }
    let mut strategies: Vec<(&str, usize)> = par_strategie.into_iter().collect();
    strategies.sort_by(|a, b| b.1.cmp(&a.1));

    warnings.push(format!(
        "{files_scanned} fichiers reconnus parsés ; {} fichiers découverts ; {} langages branchés ; calls résolus {calls_resolus} / non résolus {calls_non_resolus} ; self-loops filtrés {self_loops_filtres} / gardés {self_loops_gardes} ; stratégies : {}",
        disco.files.len(),
        registre.len(),
        strategies
            .iter()
            .map(|(s, c)| format!("{s}={c}"))
            .collect::<Vec<_>>()
            .join(" ")
    ));

    // Passe interprocédurale : transitive_loop_depth + recursive le long des
    // CALLS (équivalent pass_complexity du C — layer cognitif).
    crate::metrics::complexity_pass(&mut graph);

    // Colonnes de métriques vectorisées (has_docstring dérivé en interne).
    // Tournent AVANT tests_pass pour qu'il lise is_test depuis la colonne.
    graph.build_metric_columns(&[
        "cognitive",
        "complexity",
        "loop_depth",
        "transitive_loop_depth",
        "change_count",
        "recursive",
        "is_test",
        "has_docstring",
    ]);

    // Passe TESTS : edges tests → fonctions testées (lit is_test colonne).
    crate::metrics::tests_pass(&mut graph);
    // Caches top-N pré-triés par (kind, métrique) — requêtes O(k).
    let top_kinds = [
        NodeKind::Function,
        NodeKind::Method,
        NodeKind::File,
        NodeKind::Class,
        NodeKind::Struct,
    ];
    graph.build_top_caches(
        &top_kinds,
        &[
            "cognitive",
            "complexity",
            "loop_depth",
            "transitive_loop_depth",
            "change_count",
        ],
        100,
    );

    // Compteurs booléens O(1) : recursive / is_test / has_docstring par kind.
    graph.build_bool_counts(
        &[
            NodeKind::Function,
            NodeKind::Method,
            NodeKind::Class,
            NodeKind::Struct,
        ],
        &["recursive", "is_test", "has_docstring"],
    );

    sonde("arêtes + passes (séquentiel)");
    // ── Historique git : temporalité des fichiers + co-évolution ──────────
    if git_ctx.is_git {
        let hist = crate::git_history::analyze(root);
        // Map chemin RELATIF (format git log) → NodeId : file_path est
        // absolu, on retire le préfixe racine pour matcher git.
        let root_prefix = root.to_string_lossy().to_string();
        let file_ids: std::collections::HashMap<String, crate::graph::NodeId> = graph
            .nodes()
            .iter()
            .filter(|n| n.kind == NodeKind::File)
            .filter_map(|n| {
                let abs = n.props.file_path.as_deref()?;
                let rel = abs
                    .strip_prefix(&root_prefix)
                    .map(|r| r.trim_start_matches('/'))
                    .unwrap_or(abs);
                Some((rel.to_string(), n.id))
            })
            .collect();
        // Métadonnées temporelles sur les File existants (change_count,
        // last_modified) — pour l'analyse de hotspots.
        for ft in &hist.file_temporal {
            let Some(&fid) = file_ids.get(&ft.file_path) else {
                continue;
            };
            let Some(node) = graph.nodes_mut().iter_mut().find(|n| n.id == fid) else {
                continue;
            };
            node.props.extra.insert(
                "change_count".into(),
                serde_json::Value::from(ft.change_count),
            );
            node.props.extra.insert(
                "last_modified".into(),
                serde_json::Value::from(ft.last_modified),
            );
        }
        // FILE_CHANGES_WITH : paires de fichiers co-évoluantes présentes dans
        // le graphe (pas de nœuds fantômes).
        for cc in &hist.couplings {
            let (Some(&ida), Some(&idb)) = (file_ids.get(&cc.file_a), file_ids.get(&cc.file_b))
            else {
                continue;
            };
            if ida == idb {
                continue;
            }
            let mut eprops = crate::graph::EdgeProps::default();
            eprops.extra.insert(
                "co_changes".into(),
                serde_json::Value::from(cc.co_change_count),
            );
            eprops.extra.insert(
                "coupling_score".into(),
                serde_json::Value::from(cc.coupling_score),
            );
            eprops.extra.insert(
                "last_co_change".into(),
                serde_json::Value::from(cc.last_co_change),
            );
            graph.insert_edge(EdgeKind::FileChangesWith, ida, idb, eprops);
        }
    }

    sonde("historique git");
    let empreinte_depot = {
        let mut refs: Vec<(&str, u128)> = extraits
            .iter()
            .map(|fe| (fe.file_rel.as_str(), fe.empreinte))
            .collect();
        crate::cache_extraction::combiner(&mut refs, &git_ctx)
    };
    // (T27) Le cache n'est réécrit que si quelque chose a été analysé : une
    // réindexation sans changement ne paie pas l'écriture.
    if analyses > 0 {
        let r = crate::cache_extraction::ecrire(
            root,
            opts.cache_dir.as_deref(),
            extraits
                .par_iter()
                .map(|fe| (fe.file_rel.as_str(), fe.empreinte, &fe.ex)),
        );
        if let Err(e) = r {
            warnings.push(format!("cache d'extraction non écrit : {e}"));
        }
        sonde("écriture du cache d'extraction");
    }
    IndexOutcome {
        graph,
        files_scanned,
        warnings,
        // ── RÉUNIFICATION (2026-10-06) : les deux champs sont gardés ─────────
        // `fichiers_depuis_cache` / `empreinte_depot` viennent de la lignée
        // servie (T27, indexeur : cache d'extraction par empreinte) ; `exclusions`
        // vient du multi-écrivain. Ce sont des AJOUTS indépendants au même
        // littéral : aucun des deux ne remplace l'autre.
        fichiers_depuis_cache: files_scanned - analyses,
        empreinte_depot,
        // Les exclusions remontent AVEC leur raison : une politique qu'on ne
        // peut pas compter n'est pas une politique, c'est un `continue`.
        exclusions: disco.excluded,
    }
}

/// (T27) L'empreinte d'un dépôt SANS l'indexer : découverte, lecture et
/// empreinte de chaque fichier que l'indexation retiendrait (même langage
/// reconnu, même lecture UTF-8), plus le binaire et le contexte git. Sur le
/// noyau Linux : quelques secondes, contre des minutes de CPU pour
/// l'extraction. Égale à `IndexOutcome::empreinte_depot` pour le même état.
pub fn empreinte_depot(root: &Path, opts: &IndexOptions) -> u128 {
    let registre = lang::build_registry();
    let disco = discover(root, &opts.discovery);
    let fichiers: Vec<(String, u128)> = disco
        .files
        .par_iter()
        .filter_map(|file| {
            let source = std::fs::read_to_string(file).ok()?;
            registre.language_for_path(file)?;
            Some((
                file.to_string_lossy().to_string(),
                crate::cache_extraction::empreinte(source.as_bytes()),
            ))
        })
        .collect();
    let mut refs: Vec<(&str, u128)> = fichiers.iter().map(|(c, e)| (c.as_str(), *e)).collect();
    crate::cache_extraction::combiner(&mut refs, &crate::git_ctx::capture(root))
}

/// FQN du symbole appelant : si le call est dans un fichier au module M,
/// l'appelant est le symbole qui CONTIENT le call — approximé au module
/// (le caller_qn précis exige le suivi de portée ; approximation documentée).
/// Nom de projet depuis le chemin absolu (`/a/b` → `a.b`), contrat C fqn.c.
fn project_name_from_path(root: &Path) -> String {
    let canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let s = canon.to_string_lossy();
    let trimmed = s.trim_start_matches('/');
    let name: String = trimmed
        .chars()
        .map(|c| if c == '/' || c == ':' { '.' } else { c })
        .collect();
    // `-` et autres caractères non FQN-friendly → `-` (le C fait pareil).
    let name = name.replace('-', "_");
    if name.is_empty() {
        "projet".to_string()
    } else {
        name
    }
}

/// FQN du module (chemin sans extension ni nom de fichier) — contrat C.
fn module_qn(project: &str, root: &Path, file: &Path) -> String {
    let rel = file.strip_prefix(root).unwrap_or(file);
    let mut parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    if parts.is_empty() {
        return project.to_string();
    }
    parts.pop(); // retire le nom du fichier
    let mut qn = project.to_string();
    for p in parts {
        qn.push('.');
        qn.push_str(&p);
    }
    qn
}

/// Le fichier vit-il dans un scope d'isolation tests/benches (crate séparée) ?
///
/// Rust compile chaque fichier de `tests/` et `benches/` comme une crate
/// séparée : ses symboles n'appartiennent PAS au module du répertoire. On
/// isole un fichier quand son répertoire PARENT (le dernier composant avant
/// le nom de fichier) s'appelle `tests` ou `benches` — l'unité d'isolation
/// est le fichier (module = répertoires + stem). Les sous-répertoires
/// (`tests/common/x.rs`) restent des modules du répertoire : ils forment une
/// seule crate (déclarée `mod common;`), comme le C.
fn isolation_scope(_root: &Path, file: &Path) -> Option<String> {
    let parent_dir = file
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("");
    match parent_dir {
        "tests" | "benches" => Some(parent_dir.to_string()),
        _ => None,
    }
}

/// FQN du module d'un fichier isolé (tests/benches) : répertoires + STEM du
/// fichier (`…tests/foo.rs` → module `…tests.foo`) — le fichier EST le module
/// de sa propre crate (aligné sur la convention C
/// `…tests.<fichier>.<symbole>`).
fn module_qn_per_file(project: &str, root: &Path, file: &Path) -> String {
    let rel = file.strip_prefix(root).unwrap_or(file);
    let mut parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    if parts.is_empty() {
        return project.to_string();
    }
    // Stem du fichier (sans extension) — remplace le nom de fichier dans le
    // chemin de module.
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    parts.pop();
    let mut qn = project.to_string();
    for p in &parts {
        qn.push('.');
        qn.push_str(p);
    }
    if !stem.is_empty() {
        qn.push('.');
        qn.push_str(&stem);
    }
    qn
}

/// Résultat d'une indexation.
#[derive(Debug)]
pub struct IndexOutcome {
    pub graph: CodeGraph,
    pub files_scanned: usize,
    pub warnings: Vec<String>,
    /// (T27) Fichiers dont l'extraction vient du cache (non réanalysés).
    pub fichiers_depuis_cache: usize,
    /// (T27) Empreinte du dépôt indexé — la même que rend
    /// [`empreinte_depot`] sur ce dépôt dans le même état. À ranger avec la
    /// base : une empreinte égale plus tard ⇒ réindexer ne changerait rien.
    pub empreinte_depot: u128,
    /// Ce qui a été écarté, et pourquoi (voir `discovery::Exclusion`).
    pub exclusions: Vec<Exclusion>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn indexe_un_petit_depot_rust() {
        let td = tempfile::tempdir().unwrap();
        fs::create_dir_all(td.path().join("src")).unwrap();
        fs::write(
            td.path().join("src/lib.rs"),
            "pub fn helper() {}\npub struct Point {}\n",
        )
        .unwrap();
        let out = index_repository(td.path(), &IndexOptions::default());
        // 1 Project + 1 File + 2 symboles (Function + Struct).
        assert!(
            out.graph.node_count() >= 4,
            "nœuds: {}",
            out.graph.node_count()
        );
        // DEFINES : File → symbole.
        assert!(
            out.graph
                .edges()
                .iter()
                .any(|e| e.kind == EdgeKind::Defines)
        );
        // FQN du fichier : __file__ conservé avec l'extension.
        assert!(
            out.graph
                .node_by_qn(&format!(
                    "{}.src.lib.rs.__file__",
                    project_name_from_path(td.path())
                ))
                .is_some()
        );
    }

    #[test]
    fn self_loops_filtres_recursion_gardee_et_provenance() {
        // Petit dépôt rust : `self.x()` résolu vers l'appelant = artefact
        // filtré ; récursion directe par nom nu = gardée ; props de
        // provenance sur chaque CALLS.
        let td = tempfile::tempdir().unwrap();
        fs::create_dir_all(td.path().join("src")).unwrap();
        fs::write(
            td.path().join("src/lib.rs"),
            r#"
struct Wal;
impl Wal {
    fn helper(&self) {}
    fn run(&self) { self.helper(); self.run(); }
}
fn eval(n: i64) -> i64 { if n <= 0 { 0 } else { eval(n - 1) } }
"#,
        )
        .unwrap();
        let out = index_repository(td.path(), &IndexOptions::default());
        let g = &out.graph;
        let pairs: Vec<(String, String)> = g
            .edges()
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .filter_map(|e| {
                let (a, b) = (g.node(e.source)?, g.node(e.target)?);
                Some((a.qualified_name.clone(), b.qualified_name.clone()))
            })
            .collect();
        // receiver_self : run → helper (méthode du MÊME type).
        assert!(
            pairs
                .iter()
                .any(|(a, b)| a.ends_with(".Wal.run") && b.ends_with(".Wal.helper")),
            "run->helper manquant: {pairs:?}"
        );
        // self.run() résolu en self-loop → filtré (qualifier présent, pas de
        // récursion directe par nom nu).
        assert!(
            !pairs
                .iter()
                .any(|(a, b)| a.ends_with(".Wal.run") && b.ends_with(".Wal.run")),
            "self-loop self.run() non filtré: {pairs:?}"
        );
        // récursion directe par nom nu → gardée.
        assert!(
            pairs
                .iter()
                .any(|(a, b)| a.ends_with(".eval") && b.ends_with(".eval")),
            "récursion eval perdue: {pairs:?}"
        );
        // provenance : chaque CALLS porte strategy + confidence.
        let n_calls = g
            .edges()
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .count();
        let n_props = g
            .edges()
            .iter()
            .filter(|e| {
                e.kind == EdgeKind::Calls
                    && e.props.extra.contains_key("strategy")
                    && e.props.extra.contains_key("confidence")
            })
            .count();
        assert_eq!(n_calls, n_props, "provenance manquante sur CALLS");
        // Le diagnostic d'indexation le mentionne.
        assert!(out.warnings.iter().any(|w| w.contains("self-loops")));
    }

    #[test]
    fn projet_vide_graphe_structure() {
        let td = tempfile::tempdir().unwrap();
        let out = index_repository(td.path(), &IndexOptions::default());
        assert_eq!(out.graph.node_count(), 1); // Project seul
        assert_eq!(out.files_scanned, 0);
    }

    #[test]
    fn fichiers_tests_isoles_par_fichier() {
        // Étape 2 : les fichiers `tests/` sont des crates séparées — chaque
        // fichier a SON module (`…tests.a` vs `…tests.b`) : deux fns homonymes
        // dans deux fichiers ne collisionnent plus et les appels restent dans
        // le fichier qui définit le symbole.
        let td = tempfile::tempdir().unwrap();
        fs::create_dir_all(td.path().join("tests")).unwrap();
        fs::write(
            td.path().join("tests/a.rs"),
            "fn setup() {}\nfn run_a() { setup(); }\n",
        )
        .unwrap();
        fs::write(
            td.path().join("tests/b.rs"),
            "fn setup() {}\nfn run_b() { setup(); }\n",
        )
        .unwrap();
        let out = index_repository(td.path(), &IndexOptions::default());
        let g = &out.graph;
        let proj = project_name_from_path(td.path());
        // Deux FQN distincts pour `setup` (un par fichier isolé).
        let setup_a = format!("{proj}.tests.a.setup");
        let setup_b = format!("{proj}.tests.b.setup");
        assert!(
            g.node_by_qn(&setup_a).is_some(),
            "setup de tests/a manquant"
        );
        assert!(
            g.node_by_qn(&setup_b).is_some(),
            "setup de tests/b manquant"
        );
        let pairs: Vec<(String, String)> = g
            .edges()
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .filter_map(|e| {
                let (a, b) = (g.node(e.source)?, g.node(e.target)?);
                Some((a.qualified_name.clone(), b.qualified_name.clone()))
            })
            .collect();
        // Chaque run appelle le setup de SON fichier (pas de fuite entre
        // fichiers du même répertoire tests/).
        assert!(
            pairs
                .iter()
                .any(|(a, b)| a.ends_with(".tests.a.run_a") && b == &setup_a),
            "run_a->setup(a) manquant: {pairs:?}"
        );
        assert!(
            pairs
                .iter()
                .any(|(a, b)| a.ends_with(".tests.b.run_b") && b == &setup_b),
            "run_b->setup(b) manquant: {pairs:?}"
        );
        assert!(
            !pairs
                .iter()
                .any(|(a, b)| a.ends_with(".tests.a.run_a") && b == &setup_b),
            "fuite inter-fichiers tests: {pairs:?}"
        );
    }

    #[test]
    fn attribution_scope_le_plus_profond() {
        // Correction d'attribution : un appel dans `mod tests { fn inner() }
        // appartient à inner (def le plus profond), pas au nœud Module — sinon
        // l'arête est perdue (caller non-fonction).
        let td = tempfile::tempdir().unwrap();
        fs::create_dir_all(td.path().join("src")).unwrap();
        fs::write(
            td.path().join("src/lib.rs"),
            r#"
fn helper() {}
mod tests {
    fn inner() { helper(); }
}
"#,
        )
        .unwrap();
        let out = index_repository(td.path(), &IndexOptions::default());
        let g = &out.graph;
        let proj = project_name_from_path(td.path());
        let module_tests = format!("{proj}.src.tests"); // nœud Module du mod inline
        let pairs: Vec<(String, String)> = g
            .edges()
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .filter_map(|e| {
                let (a, b) = (g.node(e.source)?, g.node(e.target)?);
                Some((a.qualified_name.clone(), b.qualified_name.clone()))
            })
            .collect();
        // inner (fonction imbriquée, pas le Module) → helper.
        assert!(
            pairs
                .iter()
                .any(|(a, b)| a.ends_with(".src.inner") && b.ends_with(".src.helper")),
            "inner->helper attribué au mauvais scope: {pairs:?}"
        );
        assert!(
            pairs.iter().all(|(a, _)| a != &module_tests),
            "caller = Module (attribution non corrigée): {pairs:?}"
        );
    }
}
