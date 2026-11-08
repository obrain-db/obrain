//! Résolution des appels (RFC M6 §2.5).
//!
//! Réplique la chaîne de confiance du registre C (`registry.c:57-71`) :
//! import_map 0,95 · import_map_suffix 0,85 · same_module 0,90 ·
//! unique_name 0,75 · suffix_match 0,55 · fuzzy 0,40/0,30.
//!
//! Le registre opère sur les FQN déjà présents dans le graphe
//! ([`crate::graph::CodeGraph`]) — la résolution AST/LSP type-aware (couche
//! supérieure du C) arrive avec les grammaires.
//!
//! Chantier « résolution sémantique qui apprend », étape 1 :
//! 1. chaque CALLS émis porte une provenance traçable — strategy/confidence/
//!    line/caller_scope portées par les props d'arête (voir `index.rs`) ;
//! 2. les self-loops parasites (`self.x()` résolu vers l'appelant) sont
//!    filtrés à l'émission ;
//! 3. passe « récepteur » minimale : `qualifier.méthode()` résolu vers les
//!    méthodes du type du récepteur quand celui-ci est connu localement
//!    (`self`/`Self` dans une méthode d'impl, nom de type du même module,
//!    type importé, ou let-binding `let x = Type::…`/`let x: Type`), avec une
//!    confiance élevée ; sinon pas d'arête au nom seul quand un qualifier
//!    existe mais n'est pas résolu (anti-bruit).

use std::collections::HashMap;

use crate::parse::{Binding, Import};

/// Knobs de mesure (chantier « résolution sémantique qui apprend », étape 2) :
/// chaque règle nouvelle est mesurée seule (precision/recall sur
/// crates/obrain-engine vs la vérité C) avant d'être retenue — une règle qui
/// ferait régresser une métrique de >2 pts est abandonnée.
/// - `RESOLVE_FREE_FN_RETURNS` : cascade des retours de fonctions libres
///   (`fn setup() -> ObrainDB` puis `let db = setup(); db.…()`).
/// - `RESOLVE_STRUCT_FIELD_RECEIVER` : récepteurs `self.field.method()`
///   typés par la définition du champ dans la struct du self.
pub const RESOLVE_FREE_FN_RETURNS: bool = true;
pub const RESOLVE_STRUCT_FIELD_RECEIVER: bool = true;

/// Score de confiance d'une résolution (aligné sur le contrat C).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Confidence(pub u16);

impl Confidence {
    // Constantes du contrat C (×1000 pour rester entier).
    pub const IMPORT_MAP: Confidence = Confidence(950);
    pub const IMPORT_MAP_SUFFIX: Confidence = Confidence(850);
    pub const SAME_MODULE: Confidence = Confidence(900);
    pub const UNIQUE_NAME: Confidence = Confidence(750);
    pub const SUFFIX_MATCH: Confidence = Confidence(550);
    pub const FUZZY_HIGH: Confidence = Confidence(400);
    pub const FUZZY_LOW: Confidence = Confidence(300);
    // Passe « récepteur » (étape 1 du chantier) : le type du récepteur est
    // connu localement → confiance élevée, comparable à l'import_map.
    /// `self`/`Self` dans une méthode d'impl : type = impl du def courant.
    pub const RECEIVER_SELF: Confidence = Confidence(920);
    /// Nom de type exact connu dans le module du fichier appelant.
    pub const RECEIVER_TYPE: Confidence = Confidence(900);
    /// Type résolu via un import (`use crate::…::Type`) du fichier.
    pub const RECEIVER_IMPORT: Confidence = Confidence(880);
    /// Type résolu via un let-binding local (`let x = Type::…` / `let x: T`).
    pub const RECEIVER_BINDING: Confidence = Confidence(850);
    /// Type résolu via le retour d'une FONCTION LIBRE du module (`fn setup()
    /// -> ObrainDB` puis `let db = setup(); db.…()`) — étape 2 du chantier.
    pub const RECEIVER_FN_RETURN: Confidence = Confidence(845);
    /// Type résolu via un CHAMP de struct (`self.field.method()` — type du
    /// champ `field` dans la struct du self) — étape 2 du chantier.
    pub const RECEIVER_FIELD: Confidence = Confidence(860);

    pub fn as_f32(self) -> f32 {
        self.0 as f32 / 1000.0
    }
}

/// Une résolution candidate : cible + confiance.
#[derive(Debug, Clone)]
pub struct Resolution {
    pub target_qn: String,
    pub confidence: Confidence,
    /// Stratégie utilisée (nom du contrat C, ex: "same_module").
    pub strategy: &'static str,
}

/// Contexte d'un call à résoudre : tout ce que la passe « récepteur » sait
/// sur l'environnement lexical du site d'appel (l'AST est perdu après
/// l'extraction — reconstruit depuis les extractions du fichier).
#[derive(Debug, Clone)]
pub struct CallContext<'a> {
    /// Module M6 du fichier appelant (`project.src.database` …).
    pub module_qn: &'a str,
    /// Chemin rust-rel du module du fichier (ex: `query.planner`), pour
    /// normaliser les qualifiers `crate::` / `super::` / `self::`.
    pub rust_rel: &'a str,
    /// Le fichier appelant EST le module du répertoire (mod.rs/lib.rs/main.rs).
    /// En Rust, `super::` depuis un fichier-module pop d'un niveau ; depuis un
    /// sous-module fichier (`foo.rs` sous un répertoire), `super` désigne le
    /// module du répertoire — qui est AUSSI le module M6 du fichier (le schéma
    /// M6 aplatit fichier-module et sous-module sur le même FQN).
    pub file_is_mod_root: bool,
    /// Type parent du def courant quand c'est une méthode (`impl T` /
    /// méthode de trait) — résout `self.x()` et `Self::x()`.
    pub caller_parent: Option<&'a str>,
    /// Imports du fichier appelant.
    pub imports: &'a [Import],
    /// let-bindings du fichier (ordre source) — la passe binding ne retient
    /// que ceux qui précèdent l'appel dans le def contenant.
    pub bindings: &'a [&'a Binding],
    /// Ligne de l'appel (pour filtrer les bindings antérieurs).
    pub call_line: u32,
    /// Ligne de début du def contenant (None = code module).
    pub scope_start: Option<u32>,
}

/// Registre de symboles : FQN indexés par module et par nom court.
#[derive(Debug, Default)]
pub struct SymbolRegistry {
    /// module_qn → [(name, fqn)]
    by_module: HashMap<String, Vec<(String, String)>>,
    /// name → [fqn] (pour unique_name / suffix)
    by_name: HashMap<String, Vec<String>>,
    /// dernier segment du FQN → [fqn] (index suffixe précalculé — le scan
    /// global par call non résolu était le goulot : 146k calls × 26k symboles).
    by_last_segment: HashMap<String, Vec<String>>,
    /// (module_qn, type parent) → [(méthode, fqn)] — index de la passe
    /// « récepteur » : les méthodes d'un type sont cherchées par type, pas
    /// par nom court global.
    by_method: HashMap<(String, String), Vec<(String, String)>>,
    /// module_qn → [nom court de type] — les types définis/implémentés dans
    /// un module (pour valider un récepteur = nom de type local).
    by_type_in_module: HashMap<String, Vec<String>>,
    /// rust_rel (ex: `query.planner`) → [module_qn M6] — permet de résoudre
    /// les chemins `crate::…`/`super::…` des qualifiers vers les modules du
    /// graphe.
    module_by_rust_rel: HashMap<String, Vec<String>>,
    /// nom court de type → [module_qn] — résolution « globale » d'un type
    /// (ex: un `Session` défini dans src/session, appelé depuis tests/) :
    /// restreinte par unicité (un seul module possède la méthode cherchée).
    module_of_type: HashMap<String, Vec<String>>,
    /// (module_qn, type parent, méthode) → type de retour court — la cascade
    /// de bindings (`let s = db.session(); s.execute(…)`).
    method_return: HashMap<(String, String, String), String>,
    /// FQN de fonction libre → type de retour court (étape 2 : `fn setup()
    /// -> ObrainDB` puis `let db = setup(); db.…()`).
    fn_return: HashMap<String, String>,
    /// (module_qn, struct) → [(champ, type brut)] — la définition des champs
    /// de struct (`self.field.method()`, étape 2).
    struct_field: HashMap<(String, String), Vec<(String, String)>>,
}

impl SymbolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enregistre un symbole (FQN) sous son module.
    pub fn register(&mut self, module_qn: &str, name: &str, fqn: String) {
        self.register_full(module_qn, None, None, name, fqn, false);
    }

    /// Enregistrement complet :
    /// - `rust_rel` : chemin rust-rel du module (pour les qualifiers
    ///   `crate::`/`super::`) ;
    /// - `parent` : type parent si le symbole est une méthode (impl/trait) ;
    /// - `is_type` : le symbole est un type (Struct/Enum/Interface/Type/…) —
    ///   utilisé pour valider les récepteurs par nom de type.
    pub fn register_full(
        &mut self,
        module_qn: &str,
        rust_rel: Option<&str>,
        parent: Option<&str>,
        name: &str,
        fqn: String,
        is_type: bool,
    ) {
        self.by_module
            .entry(module_qn.to_string())
            .or_default()
            .push((name.to_string(), fqn.clone()));
        self.by_name
            .entry(name.to_string())
            .or_default()
            .push(fqn.clone());
        let last = fqn.rsplit('.').next().unwrap_or("").to_string();
        if !last.is_empty() {
            self.by_last_segment
                .entry(last)
                .or_default()
                .push(fqn.clone());
        }
        if let Some(parent) = parent {
            self.by_method
                .entry((module_qn.to_string(), parent.to_string()))
                .or_default()
                .push((name.to_string(), fqn));
        }
        if is_type {
            self.by_type_in_module
                .entry(module_qn.to_string())
                .or_default()
                .push(name.to_string());
            let v = self.module_of_type.entry(name.to_string()).or_default();
            if !v.iter().any(|m| m == module_qn) {
                v.push(module_qn.to_string());
            }
        }
        if let Some(rel) = rust_rel {
            if !rel.is_empty() {
                let v = self.module_by_rust_rel.entry(rel.to_string()).or_default();
                // Un module peut être enregistré plusieurs fois (type + ses
                // méthodes…) — la déduplication préserve l'unicité du module.
                if !v.iter().any(|m| m == module_qn) {
                    v.push(module_qn.to_string());
                }
            }
        }
    }

    /// Enregistre le type de retour court d'une méthode (cascade de
    /// bindings : `let s = db.session()` → type de `s` = retour de
    /// `ObrainDB::session`).
    pub fn register_method_return(&mut self, module_qn: &str, parent: &str, name: &str, ret: &str) {
        let ret = type_short_name(ret);
        if let Some(ret) = ret {
            if !ret.is_empty() {
                self.method_return.insert(
                    (module_qn.to_string(), parent.to_string(), name.to_string()),
                    ret,
                );
            }
        }
    }

    /// Enregistre le type de retour court d'une FONCTION LIBRE (étape 2 :
    /// `fn setup() -> ObrainDB` → `let db = setup(); db.…()`).
    pub fn register_fn_return(&mut self, fqn: &str, ret: &str) {
        let ret = type_short_name(ret);
        if let Some(ret) = ret {
            if !ret.is_empty() {
                self.fn_return.insert(fqn.to_string(), ret);
            }
        }
    }

    /// Enregistre le type d'un champ de struct (`struct Wal { store: T }`) —
    /// base de la résolution `self.field.method()` (étape 2).
    pub fn register_struct_field(
        &mut self,
        module_qn: &str,
        struct_name: &str,
        field: &str,
        ty: &str,
    ) {
        if field.is_empty() || ty.is_empty() {
            return;
        }
        self.struct_field
            .entry((module_qn.to_string(), struct_name.to_string()))
            .or_default()
            .push((field.to_string(), ty.to_string()));
    }

    /// Parse le type de retour court depuis une signature (`fn f(..) -> T`).
    /// Retourne le nom court de T (défaussement des wrappers/chemins).
    pub fn return_type_of_signature(sig: &str) -> Option<String> {
        let idx = sig.find("->")?;
        let rest = &sig[idx + 2..];
        // Jusqu'au corps/contrainte/génériques de fin.
        let end = rest
            .find(|c: char| matches!(c, '{' | ';' | '=' | '\n'))
            .unwrap_or(rest.len());
        let ty = rest[..end].trim();
        if ty.is_empty() || ty == "_" {
            None
        } else {
            Some(ty.to_string())
        }
    }

    /// Tente de résoudre `name` appelé depuis `caller_module` (chaîne de
    /// confiance par nom : même module → nom unique → suffixe).
    pub fn resolve(
        &self,
        name: &str,
        caller_module: &str,
        imported_modules: &[String],
    ) -> Option<Resolution> {
        // 0. import_map : module importé contenant `name` exact.
        for imp in imported_modules {
            if let Some((_, fqn)) = self
                .by_module
                .get(imp)
                .and_then(|syms| syms.iter().find(|(n, _)| n == name))
            {
                return Some(Resolution {
                    target_qn: fqn.clone(),
                    confidence: Confidence::IMPORT_MAP,
                    strategy: "import_map",
                });
            }
        }
        // 1. même module : candidats portant le nom. Si PLUSIEURS FQN
        // distincts (typiquement des méthodes homonymes de types différents),
        // le nom seul est ambigu → on ne devine pas (le premier venu était
        // une source majeure de faux positifs).
        if let Some(cands) = self.by_module.get(caller_module) {
            let mut uniq: Vec<&String> = cands
                .iter()
                .filter(|(n, _)| n == name)
                .map(|(_, f)| f)
                .collect();
            uniq.sort();
            uniq.dedup();
            if uniq.len() == 1 {
                return Some(Resolution {
                    target_qn: uniq[0].clone(),
                    confidence: Confidence::SAME_MODULE,
                    strategy: "same_module",
                });
            }
        }
        // 2. nom unique global.
        if let Some(cands) = self.by_name.get(name).filter(|c| c.len() == 1) {
            return Some(Resolution {
                target_qn: cands[0].clone(),
                confidence: Confidence::UNIQUE_NAME,
                strategy: "unique_name",
            });
        }
        // 3. suffixe : nom unique dont le dernier segment == name (index
        //    précalculé — pas de scan global).
        if let Some(cands) = self.by_last_segment.get(name).filter(|c| c.len() == 1) {
            return Some(Resolution {
                target_qn: cands[0].clone(),
                confidence: Confidence::SUFFIX_MATCH,
                strategy: "suffix_match",
            });
        }
        None
    }

    // ─────────────────────────────────────────────────────────────────────
    // Passe « récepteur » (étape 1 du chantier)
    // ─────────────────────────────────────────────────────────────────────

    /// Résout `Type::méthode` / `récepteur.méthode` vers les méthodes du type
    /// du récepteur, quand ce type est connu localement. Chaîne :
    ///
    /// 1. `self` / `Self` dans une méthode → les méthodes du type parent de
    ///    l'impl (`caller_parent`), même module → RECEIVER_SELF ;
    /// 2. nom de type exact du module appelant → RECEIVER_TYPE ;
    /// 3. type importé par le fichier (segment final d'un import, alias
    ///    compris) → RECEIVER_IMPORT ;
    /// 4. variable liée par un let-binding local (`let x = Type::…`,
    ///    `let x: Type`) → RECEIVER_BINDING ;
    /// 5. chemin rust complet (`crate::a::b::Type` / `super::Type` /
    ///    `::Type`) → normalisé vers le module M6 correspondant.
    ///
    /// Retourne `None` quand le récepteur existe mais n'est pas résolu — le
    /// site d'appel ne doit alors PAS être résolu au nom seul (anti-bruit).
    pub fn resolve_receiver(
        &self,
        name: &str,
        qualifier: &str,
        ctx: &CallContext,
    ) -> Option<Resolution> {
        self.resolve_receiver_d(name, qualifier, ctx, 0)
    }

    /// Profondeur max de la cascade de bindings (`let db = ObrainDB::…; let
    /// session = db.session(); session.execute(…)` = 2 sauts de récepteur).
    const BINDING_CASCADE_DEPTH: u32 = 3;

    fn resolve_receiver_d(
        &self,
        name: &str,
        qualifier: &str,
        ctx: &CallContext,
        depth: u32,
    ) -> Option<Resolution> {
        let q = qualifier.trim();
        if q.is_empty() {
            return None;
        }
        // (a) Chemins rust (`::`) — résolution module + type en entier.
        if q.contains("::") {
            return self.resolve_rust_path(name, q, ctx);
        }
        // (b) Récepteur simple : premier segment (avant le premier '.').
        let base = q.split('.').next().unwrap_or(q).trim();
        let base = clean_token(base);
        if base.is_empty() {
            return None;
        }
        // self / Self dans une méthode d'impl.
        if base == "self" || base == "Self" {
            let Some(parent) = ctx.caller_parent else {
                return None;
            };
            // Étape 2 : `self.<champ>.<méthode>()` — le récepteur réel est le
            // type du champ, pas le type du self. Résolution via la
            // définition du champ dans la struct du self (type local/importé
            // du même module) ; si le champ est inconnu, repli sur le
            // comportement historique (méthodes du type parent).
            if RESOLVE_STRUCT_FIELD_RECEIVER
                && let Some(field) = second_segment(q)
                && let Some(ty) = self.field_type_of(ctx.module_qn, parent, field)
                && let Some(res) = self.resolve_ty_ref(name, &ty, ctx, "receiver_field")
            {
                return Some(res);
            }
            return self.resolve_method_of(ctx.module_qn, parent, name, "receiver_self");
        }
        // Nom de type connu du module appelant.
        if self.module_has_type(ctx.module_qn, base) {
            if let Some(res) = self.resolve_method_of(ctx.module_qn, base, name, "receiver_type") {
                return Some(res);
            }
        }
        // Import du fichier dont le segment final (ou l'alias) == base.
        if let Some(import_module) = self.import_module_for(ctx, base) {
            if let Some(res) =
                self.resolve_imported_type(name, &import_module, base, "receiver_import")
            {
                return Some(res);
            }
        }
        // Binding local : `let x = Type::…` / `let x: Type` / `let x = r.m()`
        // / `let x = fn_libre()` puis `x.méthode()`.
        if let Some(b) = last_binding(ctx.bindings, base, ctx.call_line, ctx.scope_start) {
            if let Some(ty) = b.type_ref.as_deref() {
                if let Some(res) = self.resolve_ty_ref(name, ty, ctx, "receiver_binding") {
                    return Some(res);
                }
            } else if depth < Self::BINDING_CASCADE_DEPTH {
                if RESOLVE_FREE_FN_RETURNS
                    && let Some(f) = &b.init_fn
                    && let Some(res) = self.resolve_fn_return(name, f, ctx)
                {
                    return Some(res);
                }
                if let Some((recv, m)) = &b.init_call {
                    if let Some(res) = self.resolve_binding_cascade(name, recv, m, ctx, depth) {
                        return Some(res);
                    }
                }
            }
        }
        None
    }

    /// Type d'un binding initialisé par une FONCTION LIBRE du module :
    /// `let db = setup()` avec `fn setup() -> ObrainDB` → le binding est typé
    /// `ObrainDB`, on résout la méthode `name` sur ce type. Restreint au
    /// module du fichier appelant (ne traverse pas les frontières de module —
    /// et donc pas celles tests/benches isolées en étape 1).
    fn resolve_fn_return(&self, name: &str, f: &str, ctx: &CallContext) -> Option<Resolution> {
        let f = clean_token(f.trim());
        if f.is_empty() || f.contains("::") {
            // Fonctions qualifiées (`super::setup`) : hors périmètre étape 2.
            return None;
        }
        let fqn = self.unique_fqn_in_module(ctx.module_qn, f)?;
        let ret = self.fn_return.get(&fqn)?;
        self.resolve_ty_ref(name, ret, ctx, "receiver_fn_return")
    }

    /// FQN unique du symbole `name` dans le module `module` (ambiguïté → None).
    fn unique_fqn_in_module(&self, module: &str, name: &str) -> Option<String> {
        let cands = self.by_module.get(module)?;
        let mut uniq: Vec<&String> = cands
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, f)| f)
            .collect();
        uniq.sort();
        uniq.dedup();
        if uniq.len() == 1 {
            Some(uniq[0].clone())
        } else {
            None
        }
    }

    /// Type brut d'un champ de struct : `(module, struct)` → champ (`struct
    /// Wal { store: T }`, `self.store` → `T`).
    fn field_type_of(&self, module: &str, struct_name: &str, field: &str) -> Option<String> {
        let fields = self
            .struct_field
            .get(&(module.to_string(), struct_name.to_string()))?;
        let mut hits: Vec<&String> = fields
            .iter()
            .filter(|(f, _)| f == field)
            .map(|(_, t)| t)
            .collect();
        hits.sort();
        hits.dedup();
        if hits.len() == 1 {
            Some(hits[0].clone())
        } else {
            None
        }
    }

    /// Cascade de bindings : `let session = db.session()` — `session` est
    /// typé par le type de retour de la méthode `m` appelée sur le récepteur
    /// `recv` (lui-même résolu récursivement, profondeur bornée).
    fn resolve_binding_cascade(
        &self,
        name: &str,
        recv: &str,
        m: &str,
        ctx: &CallContext,
        depth: u32,
    ) -> Option<Resolution> {
        let inner = self.resolve_receiver_d(m, recv, ctx, depth + 1)?;
        let (module, parent, method) = split_target_qn(&inner.target_qn)?;
        let ret = self.method_return.get(&(module, parent, method))?;
        // Méthodes du type de retour — résolution globale unique.
        self.resolve_methods_global(ret, name, "receiver_binding")
    }

    /// Résout les méthodes du type court `ty` vers `name` : même module,
    /// puis type importé, puis type GLOBAL unique (module_of_type).
    fn resolve_ty_ref(
        &self,
        name: &str,
        ty_raw: &str,
        ctx: &CallContext,
        strategy: &'static str,
    ) -> Option<Resolution> {
        let ty = clean_token(ty_raw.trim());
        if ty.is_empty() {
            return None;
        }
        if ty.contains("::") {
            let rel = ty.replace("::", ".");
            let segs: Vec<String> = rel.split('.').map(|s| s.to_string()).collect();
            return self.resolve_rel_path(name, &segs, ctx);
        }
        // 1. même module.
        if self.module_has_type(ctx.module_qn, ty) {
            if let Some(res) = self.resolve_method_of(ctx.module_qn, ty, name, strategy) {
                return Some(res);
            }
        }
        // 2. importé par le fichier.
        if let Some(import_module) = self.import_module_for(ctx, ty) {
            if let Some(res) = self.resolve_imported_type(name, &import_module, ty, strategy) {
                return Some(res);
            }
        }
        // 3. type global unique (défini ailleurs dans le repo).
        self.resolve_methods_global(ty, name, strategy)
    }

    /// Méthodes de `ty` (nom court) à travers TOUS les modules qui définissent
    /// ce type — n'émet que si UN SEUL module a une méthode `name` (ambiguïté
    /// inter-modules → on s'abstient).
    fn resolve_methods_global(
        &self,
        ty: &str,
        name: &str,
        strategy: &'static str,
    ) -> Option<Resolution> {
        let modules = self.module_of_type.get(ty)?;
        let mut hits = Vec::new();
        for m in modules {
            if let Some(r) = self.resolve_method_of(m, ty, name, strategy) {
                hits.push(r);
            }
        }
        if hits.len() == 1 { hits.pop() } else { None }
    }

    /// Résout un chemin rust complet (`crate::a::b::Type`, `super::Type`,
    /// `self::Type`, `a::b::Type`) vers les méthodes du type final.
    fn resolve_rust_path(&self, name: &str, q: &str, ctx: &CallContext) -> Option<Resolution> {
        let segs: Vec<&str> = q
            .split("::")
            .map(clean_token)
            .filter(|s| !s.is_empty())
            .collect();
        if segs.is_empty() {
            return None;
        }
        // self::Type / Self::Type → type du module courant / de l'impl.
        if segs[0] == "self" {
            let ty = *segs.last().unwrap();
            return self.resolve_method_of(ctx.module_qn, ty, name, "receiver_type");
        }
        if segs[0] == "Self" && ctx.caller_parent.is_some() {
            let ty = ctx.caller_parent.unwrap();
            return self.resolve_method_of(ctx.module_qn, ty, name, "receiver_self");
        }
        // super::… : remonte les niveaux depuis le module du fichier.
        if segs[0] == "super" {
            let mut n_super = 0;
            while n_super < segs.len() && segs[n_super] == "super" {
                n_super += 1;
            }
            let mut rel: Vec<String> = ctx
                .rust_rel
                .split('.')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
            for _ in 0..n_super {
                if ctx.file_is_mod_root {
                    rel.pop();
                }
                // fichier non-mod : module M6 == parent rust → pas de pop.
            }
            rel.extend(segs[n_super..].iter().map(|s| s.to_string()));
            return self.resolve_rel_path(name, &rel, ctx);
        }
        // crate::… ou chemin relatif sans mot-clé → relatif à la racine.
        let start = usize::from(segs[0] == "crate");
        let rel: Vec<String> = segs[start..].iter().map(|s| s.to_string()).collect();
        self.resolve_rel_path(name, &rel, ctx)
    }

    /// Résout type = dernier segment de `rel` ; cherche les méthodes du type
    /// dans le(s) module(s) M6 correspondant au chemin rust-rel `rel`.
    fn resolve_rel_path(
        &self,
        name: &str,
        rel: &[String],
        ctx: &CallContext,
    ) -> Option<Resolution> {
        let ty = rel.last()?;
        let module_rel = rel[..rel.len() - 1].join(".");
        let modules: Vec<String> = if module_rel.is_empty() {
            // type du module courant (self::Type / super::Type d'un fichier
            // sous-module) — le module du fichier est le module M6 courant.
            if self.module_has_type(ctx.module_qn, ty) {
                vec![ctx.module_qn.to_string()]
            } else {
                return None;
            }
        } else {
            match self.module_by_rust_rel.get(&module_rel) {
                Some(m) if !m.is_empty() => m.clone(),
                _ => return None,
            }
        };
        if modules.len() == 1 {
            self.resolve_method_of(&modules[0], ty, name, "receiver_import")
        } else {
            // Plusieurs modules possibles : n'émettre que si UN seul contient
            // la méthode (sinon ambigu → on s'abstient).
            let mut hits = Vec::new();
            for m in &modules {
                if let Some(r) = self.resolve_method_of(m, ty, name, "receiver_import") {
                    hits.push(r);
                }
            }
            if hits.len() == 1 { hits.pop() } else { None }
        }
    }

    /// Import du fichier dont le segment final (ou l'alias) == `base`.
    /// Retourne le chemin d'import complet (ex: `crate.db.ObrainDB`).
    fn import_module_for<'a>(&self, ctx: &'a CallContext, base: &str) -> Option<String> {
        let mut found: Option<String> = None;
        let mut count = 0usize;
        for imp in ctx.imports {
            // Alias : `use crate::x::T as Z` avec base == Z → chemin réel T.
            let is_alias = imp.local_name.as_deref() == Some(base);
            let last = imp.module.rsplit('.').next().unwrap_or("");
            let is_plain = last == base && imp.local_name.is_none();
            if !is_alias && !is_plain {
                continue;
            }
            // Un chemin de type a ≥ 2 segments (`db.ObrainDB`) — un simple
            // identifiant (`HashMap`) n'est pas un import de module.
            if imp.module.split('.').count() < 2 {
                continue;
            }
            found = Some(imp.module.clone());
            count += 1;
        }
        if count == 1 { found } else { None }
    }

    /// Résout un type importé : chemin d'import `crate.a.b.Type` (ou
    /// `a.b.Type`) → module rust-rel `a.b`, type `Type`.
    fn resolve_imported_type(
        &self,
        name: &str,
        import_path: &str,
        ty: &str,
        strategy: &'static str,
    ) -> Option<Resolution> {
        let mut segs: Vec<&str> = import_path.split('.').collect();
        while segs.first() == Some(&"crate") || segs.first() == Some(&"self") {
            segs.remove(0);
        }
        if segs.is_empty() || segs.last().copied() != Some(ty) {
            return None;
        }
        segs.pop();
        let module_rel = segs.join(".");
        let modules = match self.module_by_rust_rel.get(&module_rel) {
            Some(m) if !m.is_empty() => m.clone(),
            _ => return None,
        };
        if modules.len() == 1 {
            self.resolve_method_of(&modules[0], ty, name, strategy)
        } else {
            let mut hits = Vec::new();
            for m in &modules {
                if let Some(r) = self.resolve_method_of(m, ty, name, strategy) {
                    hits.push(r);
                }
            }
            if hits.len() == 1 { hits.pop() } else { None }
        }
    }

    fn module_has_type(&self, module_qn: &str, ty: &str) -> bool {
        self.by_type_in_module
            .get(module_qn)
            .map(|ts| ts.iter().any(|t| t == ty))
            .unwrap_or(false)
    }

    /// Méthodes de `parent` dans `module_qn` ayant le nom `name` — résout si
    /// UN seul FQN distinct (le même module peut contenir plusieurs fichiers
    /// d'impl qui produisent le même FQN — l'upsert les dédoublonne).
    fn resolve_method_of(
        &self,
        module_qn: &str,
        parent: &str,
        name: &str,
        strategy: &'static str,
    ) -> Option<Resolution> {
        let cands = self
            .by_method
            .get(&(module_qn.to_string(), parent.to_string()))?;
        let mut uniq: Vec<&String> = cands
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, f)| f)
            .collect();
        uniq.sort();
        uniq.dedup();
        if uniq.len() == 1 {
            let fqn = uniq[0].clone();
            let confidence = match strategy {
                "receiver_self" => Confidence::RECEIVER_SELF,
                "receiver_import" => Confidence::RECEIVER_IMPORT,
                "receiver_binding" => Confidence::RECEIVER_BINDING,
                "receiver_fn_return" => Confidence::RECEIVER_FN_RETURN,
                "receiver_field" => Confidence::RECEIVER_FIELD,
                _ => Confidence::RECEIVER_TYPE,
            };
            Some(Resolution {
                target_qn: fqn.clone(),
                confidence,
                strategy,
            })
        } else {
            None
        }
    }
}

/// Nettoie un jeton de qualifier (références, génériques, parenthèses…).
fn clean_token(t: &str) -> &str {
    let t = t.trim();
    let t = t.trim_start_matches(|c: char| matches!(c, '&' | '*' | '(' | ')' | '!' | '?'));
    let end = t
        .find(|c: char| matches!(c, '<' | '(' | ' ' | '\t' | ';'))
        .unwrap_or(t.len());
    &t[..end]
}

/// Deuxième segment d'un qualifier pointé (`self.field` → `field`) — le
/// champ de struct quand le premier segment est `self`/`Self`.
fn second_segment(q: &str) -> Option<&str> {
    let mut segs = q.split('.');
    segs.next()?;
    let second = segs.next()?.trim();
    let second = clean_token(second);
    if second.is_empty() || second == "self" || second == "Self" {
        None
    } else {
        Some(second)
    }
}

/// Nom court d'un type depuis une référence brute (`&ObrainDB` → `ObrainDB`,
/// `Arc<Mutex<Session>>` → `Session`, `crate::db::ObrainDB` → `ObrainDB`).
fn type_short_name(ty_raw: &str) -> Option<String> {
    let mut ty = ty_raw.trim();
    // Références.
    loop {
        ty = ty.trim_start_matches('&').trim_start_matches('*');
        let (head, tail) = match ty.split_once('<') {
            Some((h, t)) => (h.trim(), t),
            None => break,
        };
        // Wrappers qui auto-déréférencent vers le type intérieur (les appels
        // de méthode traversent le wrapper). RefCell/Mutex/RwLock/… exigent
        // un .borrow()/.lock() explicite → PAS de déréférence automatique.
        const WRAPPERS: [&str; 5] = ["Box", "Arc", "Rc", "Pin", "Cow"];
        if WRAPPERS.contains(&head.trim_end_matches("::")) {
            // Contenu entre le premier '<' et le dernier '>' (génériques
            // imbriqués : on remonte au type le plus intérieur).
            //
            // ── POURQUOI `rfind` SANS `unwrap_or(tail.len())` ─────────────
            // Un type MALFORMÉ (`Arc<` sans `>` fermant, vu en retour de
            // méthode) donne `tail = ""` : l'ancien `unwrap_or(tail.len())`
            // rendait `end = 0`, puis `end - 1` débordait en `usize::MAX` et
            // `&tail[..usize::MAX]` paniquait en découpant une chaîne vide
            // (panique mesurée sur uzu, resolve.rs:820). Sans `>` du tout, on
            // cesse de dérouler le wrapper et on garde le type tel quel, au
            // lieu de le tronquer d'un caractère.
            let Some(pos) = tail.rfind('>') else {
                break;
            };
            let inner = &tail[..pos];
            if inner.is_empty() {
                break;
            }
            ty = inner;
            continue;
        }
        break;
    }
    let segs: Vec<&str> = ty
        .split("::")
        .filter(|s| !s.is_empty() && !s.contains('<'))
        .collect();
    let last = segs.last().copied().unwrap_or(ty);
    let last = clean_token(last);
    if last.is_empty() || last == "self" || last == "Self" {
        None
    } else {
        Some(last.to_string())
    }
}

/// (module_qn, type parent, méthode) depuis un FQN de méthode
/// `module.Cls.méthode` (ou `module.fn` → None si pas de parent).
fn split_target_qn(qn: &str) -> Option<(String, String, String)> {
    let mut segs: Vec<&str> = qn.split('.').collect();
    let method = segs.pop()?.to_string();
    let parent = segs.pop()?.to_string();
    if parent.is_empty() {
        return None;
    }
    let module = segs.join(".");
    Some((module, parent, method))
}

/// Binding local le plus proche PRÉCÉDANT `call_line` dans le scope du def
/// appelant (`let x = Type::…` / `let x: Type` / `let x = r.méthode()`).
fn last_binding<'a>(
    bindings: &'a [&'a Binding],
    var: &str,
    call_line: u32,
    scope_start: Option<u32>,
) -> Option<&'a Binding> {
    let lo = scope_start.unwrap_or(0);
    bindings
        .iter()
        .copied()
        .filter(|b| b.name == var && b.line < call_line && b.line >= lo)
        .max_by_key(|b| b.line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> SymbolRegistry {
        let mut r = SymbolRegistry::new();
        r.register("pkg.a", "helper", "pkg.a.helper".into());
        r.register("pkg.b", "helper", "pkg.b.helper".into());
        r.register("pkg.a", "util", "pkg.a.util".into());
        r
    }

    #[test]
    fn meme_module_prime() {
        let r = reg();
        let res = r.resolve("util", "pkg.a", &[]).unwrap();
        assert_eq!(res.target_qn, "pkg.a.util");
        assert_eq!(res.strategy, "same_module");
        assert_eq!(res.confidence, Confidence::SAME_MODULE);
    }

    #[test]
    fn import_map_prime_avant_meme_module() {
        let r = reg();
        // helper existe dans pkg.b (appelant) MAIS aussi pkg.a (importé).
        let res = r
            .resolve("helper", "pkg.b", &["pkg.a".to_string()])
            .unwrap();
        assert_eq!(res.target_qn, "pkg.a.helper");
        assert_eq!(res.strategy, "import_map");
    }

    #[test]
    fn ambiguite_non_resolue() {
        let r = reg();
        // helper existe dans pkg.a ET pkg.b, sans import → ambigu.
        assert!(r.resolve("helper", "pkg.other", &[]).is_none());
    }

    // ── Passe récepteur ────────────────────────────────────────────────

    fn base_ctx<'a>(
        module: &'a str,
        parent: Option<&'a str>,
        imports: &'a [Import],
        bindings: &'a [&'a Binding],
    ) -> CallContext<'a> {
        CallContext {
            module_qn: module,
            rust_rel: module,
            file_is_mod_root: true,
            caller_parent: parent,
            imports,
            bindings,
            call_line: 1_000_000,
            scope_start: None,
        }
    }

    #[test]
    fn recepteur_self() {
        // Méthode d'impl S appelant self.helper() → méthode du MÊME type.
        let mut r = SymbolRegistry::new();
        r.register_full(
            "pkg.s",
            Some("s"),
            Some("S"),
            "helper",
            "pkg.s.S.helper".into(),
            false,
        );
        r.register_full(
            "pkg.s",
            Some("s"),
            Some("S"),
            "run",
            "pkg.s.S.run".into(),
            false,
        );
        r.register_full("pkg.s", Some("s"), None, "S", "pkg.s.S".into(), true);
        let res = r
            .resolve_receiver("helper", "self", &base_ctx("pkg.s", Some("S"), &[], &[]))
            .unwrap();
        assert_eq!(res.target_qn, "pkg.s.S.helper");
        assert_eq!(res.strategy, "receiver_self");
        assert_eq!(res.confidence, Confidence::RECEIVER_SELF);
    }

    #[test]
    fn self_sans_impl_non_resolu() {
        let r = reg();
        // `self.x()` hors méthode (fn libre) → on n'invente pas d'arête.
        assert!(
            r.resolve_receiver("helper", "self", &base_ctx("pkg.other", None, &[], &[]))
                .is_none()
        );
    }

    #[test]
    fn recepteur_nom_de_type_du_module() {
        let mut r = SymbolRegistry::new();
        r.register_full(
            "pkg.s",
            Some("s"),
            Some("ObrainDB"),
            "flush",
            "pkg.s.ObrainDB.flush".into(),
            false,
        );
        r.register_full(
            "pkg.s",
            Some("s"),
            None,
            "ObrainDB",
            "pkg.s.ObrainDB".into(),
            true,
        );
        let res = r
            .resolve_receiver("flush", "ObrainDB", &base_ctx("pkg.s", None, &[], &[]))
            .unwrap();
        assert_eq!(res.target_qn, "pkg.s.ObrainDB.flush");
        assert_eq!(res.strategy, "receiver_type");
    }

    #[test]
    fn recepteur_type_sans_methodes_non_resolu() {
        let mut r = SymbolRegistry::new();
        r.register_full(
            "pkg.s",
            Some("s"),
            None,
            "ObrainDB",
            "pkg.s.ObrainDB".into(),
            true,
        );
        // Le type existe mais la méthode n'y est pas → pas d'arête inventée.
        assert!(
            r.resolve_receiver("flush", "ObrainDB", &base_ctx("pkg.s", None, &[], &[]))
                .is_none()
        );
    }

    #[test]
    fn recepteur_importe() {
        let mut r = SymbolRegistry::new();
        r.register_full(
            "pkg.db",
            Some("db"),
            Some("ObrainDB"),
            "flush",
            "pkg.db.ObrainDB.flush".into(),
            false,
        );
        r.register_full(
            "pkg.db",
            Some("db"),
            None,
            "ObrainDB",
            "pkg.db.ObrainDB".into(),
            true,
        );
        let imports = [Import {
            module: "crate.db.ObrainDB".to_string(),
            local_name: None,
        }];
        let c = CallContext {
            module_qn: "pkg.other",
            rust_rel: "other",
            file_is_mod_root: true,
            caller_parent: None,
            imports: &imports,
            bindings: &[],
            call_line: 1_000_000,
            scope_start: None,
        };
        let res = r.resolve_receiver("flush", "ObrainDB", &c).unwrap();
        assert_eq!(res.target_qn, "pkg.db.ObrainDB.flush");
        assert_eq!(res.strategy, "receiver_import");
    }

    #[test]
    fn recepteur_let_binding() {
        let mut r = SymbolRegistry::new();
        r.register_full(
            "pkg.s",
            Some("s"),
            Some("ObrainDB"),
            "flush",
            "pkg.s.ObrainDB.flush".into(),
            false,
        );
        r.register_full(
            "pkg.s",
            Some("s"),
            None,
            "ObrainDB",
            "pkg.s.ObrainDB".into(),
            true,
        );
        let binding = Binding {
            name: "db".to_string(),
            type_ref: Some("ObrainDB".to_string()),
            line: 10,
            init_call: None,
            init_fn: None,
        };
        let bindings = [&binding];
        let res = r
            .resolve_receiver("flush", "db", &base_ctx("pkg.s", None, &[], &bindings))
            .unwrap();
        assert_eq!(res.target_qn, "pkg.s.ObrainDB.flush");
        assert_eq!(res.strategy, "receiver_binding");
    }

    #[test]
    fn recepteur_inconnu_non_resolu() {
        let r = reg();
        // qualifier inconnu → None (pas d'arête au nom seul).
        assert!(
            r.resolve_receiver("helper", "mystere", &base_ctx("pkg.other", None, &[], &[]))
                .is_none()
        );
    }

    #[test]
    fn chemin_crate_resolu() {
        let mut r = SymbolRegistry::new();
        r.register_full(
            "pkg.src.db",
            Some("db"),
            Some("ObrainDB"),
            "flush",
            "pkg.src.db.ObrainDB.flush".into(),
            false,
        );
        r.register_full(
            "pkg.src.db",
            Some("db"),
            None,
            "ObrainDB",
            "pkg.src.db.ObrainDB".into(),
            true,
        );
        // `crate::db::ObrainDB::flush` depuis pkg.src.query.
        let c = CallContext {
            module_qn: "pkg.src.query",
            rust_rel: "query",
            file_is_mod_root: true,
            caller_parent: None,
            imports: &[],
            bindings: &[],
            call_line: 1_000_000,
            scope_start: None,
        };
        let res = r
            .resolve_receiver("flush", "crate::db::ObrainDB", &c)
            .unwrap();
        assert_eq!(res.target_qn, "pkg.src.db.ObrainDB.flush");
        assert_eq!(res.strategy, "receiver_import");
    }

    #[test]
    fn cascade_binding_retour_methode() {
        // `let db = ObrainDB::…; let session = db.session(); session.execute(…)`
        // — typage en cascade par les types de retour enregistrés.
        let mut r = SymbolRegistry::new();
        r.register_full(
            "pkg.src.database",
            Some("database"),
            Some("ObrainDB"),
            "session",
            "pkg.src.database.ObrainDB.session".into(),
            false,
        );
        r.register_full(
            "pkg.src.database",
            Some("database"),
            None,
            "ObrainDB",
            "pkg.src.database.ObrainDB".into(),
            true,
        );
        r.register_method_return("pkg.src.database", "ObrainDB", "session", "Session");
        r.register_full(
            "pkg.src.session",
            Some("session"),
            Some("Session"),
            "execute",
            "pkg.src.session.Session.execute".into(),
            false,
        );
        r.register_full(
            "pkg.src.session",
            Some("session"),
            None,
            "Session",
            "pkg.src.session.Session".into(),
            true,
        );
        let db_b = Binding {
            name: "db".into(),
            type_ref: Some("ObrainDB".into()),
            line: 5,
            init_call: None,
            init_fn: None,
        };
        let session_b = Binding {
            name: "session".into(),
            type_ref: None,
            line: 6,
            init_call: Some(("db".into(), "session".into())),
            init_fn: None,
        };
        let bindings = [&db_b, &session_b];
        let c = CallContext {
            module_qn: "pkg.bench",
            rust_rel: "bench",
            file_is_mod_root: true,
            caller_parent: None,
            imports: &[],
            bindings: &bindings,
            call_line: 1_000_000,
            scope_start: None,
        };
        let res = r.resolve_receiver("execute", "session", &c).unwrap();
        assert_eq!(res.target_qn, "pkg.src.session.Session.execute");
        assert_eq!(res.strategy, "receiver_binding");
    }

    #[test]
    fn qualifier_self_champ_de_type_meme_module() {
        // `self.inner.method()` : récepteur self → méthode du type parent —
        // même si le texte du qualifier est plus long que `self`.
        let mut r = SymbolRegistry::new();
        r.register_full(
            "pkg.s",
            Some("s"),
            Some("WalStore"),
            "has_backward_adjacency",
            "pkg.s.WalStore.has_backward_adjacency".into(),
            false,
        );
        r.register_full(
            "pkg.s",
            Some("s"),
            None,
            "WalStore",
            "pkg.s.WalStore".into(),
            true,
        );
        let res = r
            .resolve_receiver(
                "has_backward_adjacency",
                "self.inner",
                &base_ctx("pkg.s", Some("WalStore"), &[], &[]),
            )
            .unwrap();
        assert_eq!(res.target_qn, "pkg.s.WalStore.has_backward_adjacency");
        assert_eq!(res.strategy, "receiver_self");
    }

    // ── Étape 2 : retours de fonctions libres + champs de struct ──────────

    #[test]
    fn retour_fonction_libre_type_le_binding() {
        // `fn setup() -> ObrainDB` puis `let db = setup(); db.session()` —
        // étape 2 : le binding est typé par le retour de la fonction libre du
        // module (stratégie receiver_fn_return).
        let mut r = SymbolRegistry::new();
        // Fonction libre + son type de retour enregistré.
        r.register_full(
            "pkg.tf",
            Some("tf"),
            None,
            "setup",
            "pkg.tf.setup".into(),
            false,
        );
        r.register_fn_return("pkg.tf.setup", "ObrainDB");
        // Type cible : ObrainDB.session (défini ailleurs dans le repo).
        r.register_full(
            "pkg.db",
            Some("db"),
            Some("ObrainDB"),
            "session",
            "pkg.db.ObrainDB.session".into(),
            false,
        );
        r.register_full(
            "pkg.db",
            Some("db"),
            None,
            "ObrainDB",
            "pkg.db.ObrainDB".into(),
            true,
        );
        let binding = Binding {
            name: "db".to_string(),
            type_ref: None,
            line: 10,
            init_call: None,
            init_fn: Some("setup".to_string()),
        };
        let bindings = [&binding];
        let res = r
            .resolve_receiver("session", "db", &base_ctx("pkg.tf", None, &[], &bindings))
            .unwrap();
        assert_eq!(res.target_qn, "pkg.db.ObrainDB.session");
        assert_eq!(res.strategy, "receiver_fn_return");
        assert_eq!(res.confidence, Confidence::RECEIVER_FN_RETURN);
    }

    #[test]
    fn retour_fonction_libre_inconnue_non_resolue() {
        // Fonction libre non enregistrée (retour inconnu) → pas d'arête.
        let r = SymbolRegistry::new();
        let binding = Binding {
            name: "db".to_string(),
            type_ref: None,
            line: 10,
            init_call: None,
            init_fn: Some("mystere".to_string()),
        };
        let bindings = [&binding];
        assert!(
            r.resolve_receiver("session", "db", &base_ctx("pkg.tf", None, &[], &bindings))
                .is_none()
        );
    }

    #[test]
    fn cascade_retour_fonction_libre_puis_methode() {
        // `fn setup() -> ObrainDB` ; `let db = setup(); let s = db.session();
        // s.execute()` — la cascade traverse le retour de la fonction libre
        // puis le retour de méthode (session → Session.execute).
        let mut r = SymbolRegistry::new();
        r.register_full(
            "pkg.tf",
            Some("tf"),
            None,
            "setup",
            "pkg.tf.setup".into(),
            false,
        );
        r.register_fn_return("pkg.tf.setup", "ObrainDB");
        r.register_full(
            "pkg.db",
            Some("db"),
            Some("ObrainDB"),
            "session",
            "pkg.db.ObrainDB.session".into(),
            false,
        );
        r.register_full(
            "pkg.db",
            Some("db"),
            None,
            "ObrainDB",
            "pkg.db.ObrainDB".into(),
            true,
        );
        r.register_method_return("pkg.db", "ObrainDB", "session", "Session");
        r.register_full(
            "pkg.s",
            Some("s"),
            Some("Session"),
            "execute",
            "pkg.s.Session.execute".into(),
            false,
        );
        r.register_full(
            "pkg.s",
            Some("s"),
            None,
            "Session",
            "pkg.s.Session".into(),
            true,
        );
        let db_b = Binding {
            name: "db".into(),
            type_ref: None,
            line: 5,
            init_call: None,
            init_fn: Some("setup".into()),
        };
        let session_b = Binding {
            name: "session".into(),
            type_ref: None,
            line: 6,
            init_call: Some(("db".into(), "session".into())),
            init_fn: None,
        };
        let bindings = [&db_b, &session_b];
        let c = CallContext {
            module_qn: "pkg.tf",
            rust_rel: "tf",
            file_is_mod_root: true,
            caller_parent: None,
            imports: &[],
            bindings: &bindings,
            call_line: 1_000_000,
            scope_start: None,
        };
        let res = r.resolve_receiver("execute", "session", &c).unwrap();
        assert_eq!(res.target_qn, "pkg.s.Session.execute");
        assert_eq!(res.strategy, "receiver_binding");
    }

    #[test]
    fn champ_de_struct_type_le_recepteur() {
        // `self.store.flush()` : le récepteur réel est le CHAMP `store` de la
        // struct du self (étape 2) — type du champ → méthode de WalStore.
        let mut r = SymbolRegistry::new();
        // Struct + champ `store: WalStore`.
        r.register_full("pkg.w", Some("w"), None, "Wal", "pkg.w.Wal".into(), true);
        r.register_struct_field("pkg.w", "Wal", "store", "WalStore");
        // Type du champ + sa méthode.
        r.register_full(
            "pkg.w",
            Some("w"),
            Some("WalStore"),
            "flush",
            "pkg.w.WalStore.flush".into(),
            false,
        );
        r.register_full(
            "pkg.w",
            Some("w"),
            None,
            "WalStore",
            "pkg.w.WalStore".into(),
            true,
        );
        // Méthode de l'impl Wal appelant self.store.flush().
        let res = r
            .resolve_receiver(
                "flush",
                "self.store",
                &base_ctx("pkg.w", Some("Wal"), &[], &[]),
            )
            .unwrap();
        assert_eq!(res.target_qn, "pkg.w.WalStore.flush");
        assert_eq!(res.strategy, "receiver_field");
        assert_eq!(res.confidence, Confidence::RECEIVER_FIELD);
    }

    #[test]
    fn champ_inconnu_repli_type_parent() {
        // Champ absent de la struct enregistrée → repli historique sur les
        // méthodes du type parent (comportement étape 1 conservé).
        let mut r = SymbolRegistry::new();
        r.register_full("pkg.w", Some("w"), None, "Wal", "pkg.w.Wal".into(), true);
        r.register_full(
            "pkg.w",
            Some("w"),
            Some("Wal"),
            "flush",
            "pkg.w.Wal.flush".into(),
            false,
        );
        let res = r
            .resolve_receiver(
                "flush",
                "self.inconnu",
                &base_ctx("pkg.w", Some("Wal"), &[], &[]),
            )
            .unwrap();
        assert_eq!(res.target_qn, "pkg.w.Wal.flush");
        assert_eq!(res.strategy, "receiver_self");
    }
}
