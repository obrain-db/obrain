//! Filter operator for applying predicates.

use super::{Operator, OperatorResult};
use crate::execution::{ChunkZoneHints, DataChunk, SelectionVector, ValueVector};
use crate::graph::Direction;
use crate::graph::GraphStore;
use crate::graph::traits::PredicatTexte;
use crate::graph::lpg::{Edge, Node};
use obrain_common::types::{
    EpochId, HashableValue, NodeId, PropertyKey, TransactionId, Value, value_cmp,
};
#[cfg(feature = "regex")]
use regex::Regex;
#[cfg(all(feature = "regex-lite", not(feature = "regex")))]
use regex_lite::Regex;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// Extracts a required integer field from a temporal constructor map.
fn map_int(m: &BTreeMap<PropertyKey, Value>, key: &str) -> Option<i64> {
    match m.get(&PropertyKey::from(key))? {
        Value::Int64(v) => Some(*v),
        Value::Float64(f) => Some(*f as i64),
        _ => None,
    }
}

/// Extracts an optional integer field from a temporal constructor map,
/// returning a default value when the key is absent.
fn map_int_or(m: &BTreeMap<PropertyKey, Value>, key: &str, default: i64) -> i64 {
    map_int(m, key).unwrap_or(default)
}

/// A predicate for filtering rows.
pub trait Predicate: Send + Sync {
    /// Evaluates the predicate for a single row.
    fn evaluate(&self, chunk: &DataChunk, row: usize) -> bool;

    /// Returns `false` if zone map proves no rows in this chunk can match.
    ///
    /// This method enables chunk-level filtering optimization. When a chunk
    /// has zone map hints attached, the filter operator calls this method
    /// first. If it returns `false`, the entire chunk is skipped without
    /// evaluating any rows.
    ///
    /// The default implementation is conservative and returns `true` (might match).
    /// Predicates that support zone map checking should override this.
    fn might_match_chunk(&self, _hints: &ChunkZoneHints) -> bool {
        true
    }

    /// Annonce le chunk dont les lignes vont être évaluées, AVANT la première.
    ///
    /// POURQUOI CETTE ANNONCE EXISTE (2026-09-12). Sans elle, un prédicat qui
    /// veut amortir une lecture de propriété n'a aucun moyen de savoir que les
    /// 2 048 `evaluate` qui suivent portent sur LE MÊME chunk : il ne peut que
    /// refaire la lecture, par ligne et par occurrence de prédicat — soit
    /// `get_node_property` scalaire, mesuré ~1 µs/cellule.
    ///
    /// `ExpressionPredicate` pré-extrait déjà la propriété de tout le chunk ici
    /// (voir `prepare_chunk` de son impl), puis sert chaque ligne depuis sa
    /// copie — une lecture pour 2 048 lignes au lieu d'une par ligne. Les
    /// prédicats qui n'ont rien à amortir gardent le défaut : rien.
    fn prepare_chunk(&self, _chunk: &DataChunk) {}

    /// Relève (et efface) la première erreur d'évaluation consignée depuis
    /// le dernier relevé.
    ///
    /// L'évaluateur d'expressions rend `Option<Value>` : `None` y signifie
    /// « pas de valeur » et devient NULL en projection, `false` en filtre.
    /// Une division par zéro ou un débordement i64 ne doivent être ni l'un
    /// ni l'autre — la requête entière doit être REFUSÉE. Les prédicats qui
    /// détectent une telle faute la consignent, et les opérateurs
    /// (`FilterOperator`, `ProjectOperator`) la relèvent ici pour la
    /// transformer en `OperatorError::Execution`. Par défaut : aucun
    /// prédicat n'a d'erreur à signaler.
    fn take_eval_error(&self) -> Option<String> {
        None
    }
}

// The `CompareOp` enum and `ComparisonPredicate` struct that used to live
// here (as `#[cfg(test)] pub(crate)` helpers) have been relocated to
// `crates/obrain-substrate/tests/operators_filter.rs` as part of T17 W4.p4
// — substrate-backed fixtures cannot live in `obrain-core` due to the
// dev-dep cycle. They were never part of the crate's public or internal API.

/// Opération portée par l'arithmétique numérique GARDÉE de l'évaluateur.
///
/// Le modulo a son propre chemin ([`ExpressionPredicate::eval_modulo`]) ;
/// voir [`ExpressionPredicate::eval_arithmetic`] pour la sémantique et le
/// miroir avec `check_eval.rs`.
#[derive(Clone, Copy)]
enum ArithGardee {
    Add,
    Sub,
    Mul,
    Div,
}

impl ArithGardee {
    /// L'opération flottante correspondante — IEEE-754, non gardée à dessein.
    fn flottant(self, a: f64, b: f64) -> f64 {
        match self {
            ArithGardee::Add => a + b,
            ArithGardee::Sub => a - b,
            ArithGardee::Mul => a * b,
            ArithGardee::Div => a / b,
        }
    }

    /// Le symbole de l'opération, pour les messages d'erreur.
    fn symbole(self) -> char {
        match self {
            ArithGardee::Add => '+',
            ArithGardee::Sub => '-',
            ArithGardee::Mul => '*',
            ArithGardee::Div => '/',
        }
    }
}

/// Une propriété référencée par un prédicat, EXTRAITE EN UN LOT par chunk.
///
/// POURQUOI CETTE STRUCTURE (2026-09-12). L'évaluateur lisait chaque cellule
/// par sa route scalaire (`store.get_node_property`) : cinq à huit accès
/// partagés et deux allocations par cellule — le commentaire de
/// `eval_expr` le chiffre lui-même à « ~1 µs per cell ». Or un filtre
/// n'évalue pas une ligne isolée : il évalue 2 048 lignes d'un même chunk,
/// avec le MÊME couple (variable, propriété) souvent répété dans plusieurs
/// termes. Deux amortissements deviennent alors possibles, et ce sont eux que
/// cette structure achète :
///
///   1. la lecture par LOT (`GraphStore::get_node_property_batch`), qui résout
///      UNE fois par lot ce qui ne dépend que de la clé (spec des zones, id de
///      clé, verrous de zone) au lieu de le refaire par ligne ;
///   2. la MISE EN COMMUN entre occurrences : `n.name CONTAINS 'a' OR
///      n.name CONTAINS 'b' OR …` lisait `n.name` QUATRE fois par ligne.
///      Mesuré : 4 termes sur la même propriété = 4 × le prix, soit 1 348 ms
///      là où un seul terme en coûtait 362.
///
/// VALIDITÉ. `lignes[i].0` est l'identifiant du nœud lu à la ligne `i`, et il
/// sert de PREUVE : la valeur d'une propriété ne dépend que de (identifiant,
/// clé). Si l'identifiant de la ligne en cours d'évaluation est celui qu'on a
/// extrait, la valeur en cache est la bonne — même si le chunk a changé entre
/// l'extraction et la lecture (cas d'un `eval_at` appelé par un autre
/// opérateur, qui ne prépare rien). Aucune valeur fausse n'est donc servie :
/// en cas de désaccord, l'appelant reprend la route scalaire, inchangée.
struct ProprietePreExtraite {
    /// Nom de la variable (colonne du chunk).
    variable: String,
    /// Nom de la propriété.
    propriete: String,
    /// Clé construite UNE fois par requête : `PropertyKey::new` allouait un
    /// `ArcStr` (arcstr::ArcStr::from(&str) alloue) à chaque cellule évaluée.
    cle: PropertyKey,
    /// Une case par ligne du chunk : (identifiant prouvant la validité, valeur).
    lignes: Vec<(NodeId, Option<Value>)>,
}

/// Réponse de la pré-extraction pour une (ligne, variable, propriété).
enum ReponsePre {
    /// Valeur extraite pour CE nœud.
    Valeur(Value),
    /// Le lot a dit « absente » pour ce nœud (et l'identifiant concorde).
    Absente,
    /// Rien d'exploitable : le prédicat suit la route scalaire, inchangée.
    Inconnue,
}

/// Trace opt-in du chemin de lecture des cellules (`OBRAIN_FILTRE_TRACE=1`).
///
/// Elle existe parce qu'un banc ne dit pas POURQUOI : sur le snapshot figé, la
/// lecture par lot du lot entier (2 048 lignes) coûte 0,04 à 1,1 ms, et la
/// requête coûte encore 1 300 ms — donc le temps est AILLEURS, dans la boucle
/// par ligne. Ces compteurs disent lequel des trois chemins chaque ligne prend,
/// sans quoi on optimise à l'aveugle.
mod trace_filtre {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// Positionné une fois par prédicat, depuis l'environnement.
    pub(super) static ACTIVE: AtomicBool = AtomicBool::new(false);
    pub(super) static VALEUR: AtomicUsize = AtomicUsize::new(0);
    pub(super) static ABSENTE: AtomicUsize = AtomicUsize::new(0);
    pub(super) static INCONNUE: AtomicUsize = AtomicUsize::new(0);

    #[inline]
    pub(super) fn actif() -> bool {
        ACTIVE.load(Ordering::Relaxed)
    }

    pub(super) fn remise_a_zero() {
        VALEUR.store(0, Ordering::Relaxed);
        ABSENTE.store(0, Ordering::Relaxed);
        INCONNUE.store(0, Ordering::Relaxed);
    }

    pub(super) fn ligne() -> String {
        format!(
            "valeurs {} · absentes {} · scalaires {}",
            VALEUR.load(Ordering::Relaxed),
            ABSENTE.load(Ordering::Relaxed),
            INCONNUE.load(Ordering::Relaxed)
        )
    }
}

/// Rassemble les couples (variable, propriété) référencés par une expression.
///
/// CONSERVATEUR À DESSEIN : un variant non traité (ou ajouté plus tard) n'est
/// pas collecté, donc sa propriété est évaluée par la route scalaire —
/// correct, seulement plus lent. Jamais l'inverse : rien n'est pré-extrait sur
/// une supposition.
fn collecte_proprietes(expr: &FilterExpression, out: &mut Vec<(String, String)>) {
    match expr {
        FilterExpression::Property {
            variable,
            property,
        } => {
            if !out.iter().any(|(v, p)| v == variable && p == property) {
                out.push((variable.clone(), property.clone()));
            }
        }
        FilterExpression::Binary { left, right, .. } => {
            collecte_proprietes(left, out);
            collecte_proprietes(right, out);
        }
        FilterExpression::Unary { operand, .. } => collecte_proprietes(operand, out),
        FilterExpression::FunctionCall { args, .. } => {
            for a in args {
                collecte_proprietes(a, out);
            }
        }
        FilterExpression::List(items) => {
            for i in items {
                collecte_proprietes(i, out);
            }
        }
        FilterExpression::Map(pairs) => {
            for (_, v) in pairs {
                collecte_proprietes(v, out);
            }
        }
        FilterExpression::IndexAccess { base, index } => {
            collecte_proprietes(base, out);
            collecte_proprietes(index, out);
        }
        FilterExpression::SliceAccess { base, start, end } => {
            collecte_proprietes(base, out);
            if let Some(s) = start {
                collecte_proprietes(s, out);
            }
            if let Some(e) = end {
                collecte_proprietes(e, out);
            }
        }
        FilterExpression::Case {
            operand,
            when_clauses,
            else_clause,
        } => {
            if let Some(o) = operand {
                collecte_proprietes(o, out);
            }
            for (quand, alors) in when_clauses {
                collecte_proprietes(quand, out);
                collecte_proprietes(alors, out);
            }
            if let Some(e) = else_clause {
                collecte_proprietes(e, out);
            }
        }
        FilterExpression::ListComprehension {
            list_expr,
            filter_expr,
            map_expr,
            ..
        } => {
            collecte_proprietes(list_expr, out);
            if let Some(f) = filter_expr {
                collecte_proprietes(f, out);
            }
            collecte_proprietes(map_expr, out);
        }
        FilterExpression::ListPredicate {
            list_expr,
            predicate,
            ..
        } => {
            collecte_proprietes(list_expr, out);
            collecte_proprietes(predicate, out);
        }
        // Littéraux, variables, id/labels/type, Reduce, sous-requêtes : rien à
        // collecter ici (voir la note « conservateur à dessein »).
        _ => {}
    }
}

/// Un prédicat TEXTE pré-évalué pour tout un chunk — voir
/// [`GraphStore::verdicts_texte_batch`].
///
/// POURQUOI (2026-09-12). Les valeurs pré-extraites coûtent une allocation par
/// cellule PRÉSENTE (le `ArcStr` du `Value`). Pour `CONTAINS` / `STARTS WITH` /
/// `ENDS WITH`, la valeur n'a pas besoin d'exister : seuls ses OCTETS comptent.
/// Ce verdict-ci vient du substrat, lu sur l'arène mmap, sans une allocation.
///
/// MESURE (banc contrôlé `micro-props`, 2 000 cellules 100 % présentes) : 1,1 ms
/// → 0,64-0,89 ms pour la lecture seule, allocation comprise ; c'est cette
/// allocation-ci que la voie des verdicts supprime.
struct VerdictTexte {
    /// Variable du chunk.
    variable: String,
    /// Propriété interrogée.
    propriete: String,
    /// Opérateur, pour ne servir le verdict qu'à l'expression EXACTE.
    op: BinaryFilterOp,
    /// Aiguille littérale (`'iam'` dans `CONTAINS 'iam'`).
    aiguille: String,
    /// Un identifiant par ligne : même PREUVE de validité que les valeurs
    /// pré-extraites (la réponse ne dépend que du nœud).
    ids: Vec<NodeId>,
    /// Un verdict par ligne : `None` = NULL, `Some(b)` = verdict.
    verdicts: Vec<Option<bool>>,
    /// Lignes observées et lignes NON NULLES depuis l'activation — la densité
    /// décide, et elle est MESURÉE, pas supposée.
    vues: u64,
    non_nulles: u64,
    /// Faux quand la densité observée ne justifie plus cette voie : l'entrée
    /// redevient inerte, donc la lecture de valeurs reprend la main (et la
    /// propriété est matérialisée comme avant).
    actif: bool,
}

/// Densité en dessous de laquelle la voie des verdicts est ABANDONNÉE.
///
/// MESURÉ (2026-09-12, temps CPU, `OBRAIN_VERDICTS_TEXTE` coupé/suivi) :
///   `qualified_name` (100 % des lignes portent une valeur)  ×1,54
///   `name` (15 %)                                            ×0,84
/// Sur une propriété creuse, prouver NULL oblige à traverser toute la chaîne
/// alors que la lecture de valeurs n'aurait payé aucun décodage : la voie coûte
/// donc plus qu'elle ne rapporte. La frontière est franche ; un demi est un
/// partage honnête, et il est vérifié par le banc des deux côtés.
const DENSITE_MIN_VERDICTS: u64 = 2;

/// Nombre de lignes observées avant de trancher (une seule tranche de 2 048
/// suffit déjà à séparer 15 % de 100 %).
const OBSERVATIONS_MIN_VERDICTS: u64 = 512;

/// Réponse de la voie des verdicts pour une ligne.
enum ReponseVerdict {
    /// Le prédicat a un verdict.
    Verdict(bool),
    /// NULL — valeur absente ou non textuelle, exactement ce que l'évaluateur
    /// rendrait (`left.as_str()?` échoue).
    Nul,
    /// Rien de pré-évalué : suivre le chemin d'origine, inchangé.
    Aucun,
}

/// Réponse du chemin rapide `split(s, d)[k]` (`ReponseSplitIndexe`).
///
/// ── POURQUOI CE MOTIF MÉRITE SON PROPRE CHEMIN (2026-09-15) ────────────────
///
/// `split(qn, '.')[k]` est l'idiome d'indexation de chemin de TOUT ce dépôt :
/// le dashboard agrège par module avec, la couche viz l'écrit dans chaque
/// route, et le chat s'en sert pour nommer. Or la voie générale matérialise la
/// liste ENTIÈRE — une `Vec<Value>` et une `ArcStr` ALLOUÉE PAR TOKEN — pour n'en
/// lire qu'UN élément.
///
/// Coût mesuré sur `runtime-dev` (1 324 105 arêtes CALLS), par étage :
///
/// | étage                                   | wall   | delta    |
/// |-----------------------------------------|--------|----------|
/// | balayage des arêtes CALLS, `count(r)`   |   80 ms| —        |
/// | + `a.qualified_name` / `b.qualified_name`|  340 ms| +260 ms  |
/// | + `split(a.qn,'.')[4]`                  |  640 ms| +300 ms  |
/// | + `split(b.qn,'.')[4]`                  | 1000 ms| +360 ms  |
/// | + `GROUP BY sa, sb, count(r)`           | 1050 ms|  +50 ms  |
///
/// Le groupement — ce qu'on croit toujours être le coût — pèse 50 ms. Ce qui
/// pèse, c'est de payer 2 allocations par arête (2,6 M) pour lire 2 tokens.
/// Le nombre de NŒUDS distincts est pourtant 439 688 : le travail est fait six
/// fois de trop, par construction de la voie générale.
///
/// Le chemin rapide ne change AUCUNE sémantique : il consomme le MÊME itérateur
/// `str::split` que la voie générale, dans le MÊME ordre, et n'alloue que le
/// token demandé. Il suit donc à l'identique les cas limites — séparateur vide
/// (`"abc".split("")` rend 5 éléments, dont deux vides), index hors bornes
/// (→ `None`), index négatif (compté depuis la fin), et arguments non textuels
/// (→ `None`).
enum ReponseSplitIndexe {
    /// Le motif s'applique : voici ce que l'indexation rend (`None` = hors
    /// bornes, exactement ce que `items.get(idx)` rendrait).
    Valeur(Option<Value>),
    /// Le motif ne s'applique pas : suivre l'évaluation générale, inchangée.
    Aucun,
}

/// Rassemble les prédicats texte évaluables par le substrat :
/// `Property <CONTAINS|STARTS WITH|ENDS WITH> 'littéral'`.
///
/// CONSERVATEUR À DESSEIN, comme `collecte_proprietes` : seule cette forme
/// exacte est retenue (droit littéral de type chaîne). Tout le reste — opérande
/// calculé, littéral non textuel, comparaison d'égalité — suit le chemin
/// d'origine, donc aucune sémantique n'est réécrite ici.
fn collecte_predicats_texte(
    expr: &FilterExpression,
    out: &mut Vec<(String, String, BinaryFilterOp, String)>,
) {
    if let FilterExpression::Binary { left, op, right } = expr
        && matches!(
            op,
            BinaryFilterOp::Contains | BinaryFilterOp::StartsWith | BinaryFilterOp::EndsWith
        )
        && let FilterExpression::Property {
            variable,
            property,
        } = &**left
        && let FilterExpression::Literal(Value::String(aiguille)) = &**right
        && !out
            .iter()
            .any(|(v, p, o, a)| v == variable && p == property && *o == *op && a == aiguille.as_str())
    {
        out.push((
            variable.clone(),
            property.clone(),
            *op,
            aiguille.to_string(),
        ));
    }
    match expr {
        FilterExpression::Binary { left, right, .. } => {
            collecte_predicats_texte(left, out);
            collecte_predicats_texte(right, out);
        }
        FilterExpression::Unary { operand, .. } => collecte_predicats_texte(operand, out),
        FilterExpression::FunctionCall { args, .. } => {
            for a in args {
                collecte_predicats_texte(a, out);
            }
        }
        FilterExpression::List(items) => {
            for i in items {
                collecte_predicats_texte(i, out);
            }
        }
        FilterExpression::Map(pairs) => {
            for (_, v) in pairs {
                collecte_predicats_texte(v, out);
            }
        }
        FilterExpression::IndexAccess { base, index } => {
            collecte_predicats_texte(base, out);
            collecte_predicats_texte(index, out);
        }
        FilterExpression::SliceAccess { base, start, end } => {
            collecte_predicats_texte(base, out);
            if let Some(s) = start {
                collecte_predicats_texte(s, out);
            }
            if let Some(e) = end {
                collecte_predicats_texte(e, out);
            }
        }
        FilterExpression::Case {
            operand,
            when_clauses,
            else_clause,
        } => {
            if let Some(o) = operand {
                collecte_predicats_texte(o, out);
            }
            for (quand, alors) in when_clauses {
                collecte_predicats_texte(quand, out);
                collecte_predicats_texte(alors, out);
            }
            if let Some(e) = else_clause {
                collecte_predicats_texte(e, out);
            }
        }
        FilterExpression::ListComprehension {
            list_expr,
            filter_expr,
            map_expr,
            ..
        } => {
            collecte_predicats_texte(list_expr, out);
            if let Some(f) = filter_expr {
                collecte_predicats_texte(f, out);
            }
            collecte_predicats_texte(map_expr, out);
        }
        FilterExpression::ListPredicate {
            list_expr,
            predicate,
            ..
        } => {
            collecte_predicats_texte(list_expr, out);
            collecte_predicats_texte(predicate, out);
        }
        _ => {}
    }
}

/// Compte, par couple (variable, propriété), combien de fois la propriété est
/// référencée et combien de ces références sont COUVERTES par un verdict texte.
///
/// POURQUOI (mesuré 2026-09-12). Sans ce compte, une propriété interrogée par
/// `CONTAINS` était lue DEUX fois : une pour le verdict, une pour les valeurs —
/// et payer deux traversées de chaîne coûtait 9 % de CPU de plus que n'en
/// rapportait l'allocation économisée. Une propriété dont TOUTES les références
/// sont couvertes n'a plus besoin d'être matérialisée du tout.
fn compte_occurrences(expr: &FilterExpression, couvert: bool, out: &mut Vec<(String, String, usize, usize)>) {
    match expr {
        FilterExpression::Property {
            variable,
            property,
        } => {
            let entree = match out
                .iter_mut()
                .find(|(v, p, _, _)| v == variable && p == property)
            {
                Some(e) => e,
                None => {
                    out.push((variable.clone(), property.clone(), 0, 0));
                    out.last_mut().expect("vient d'être poussée")
                }
            };
            entree.2 += 1;
            if couvert {
                entree.3 += 1;
            }
        }
        FilterExpression::Binary { left, op, right } => {
            // La référence est COUVERTE quand elle est l'opérande gauche d'une
            // forme reconnue par `collecte_predicats_texte`.
            let couvre_gauche = matches!(
                op,
                BinaryFilterOp::Contains | BinaryFilterOp::StartsWith | BinaryFilterOp::EndsWith
            ) && matches!(
                (&**left, &**right),
                (FilterExpression::Property { .. }, FilterExpression::Literal(Value::String(_)))
            );
            compte_occurrences(left, couvert || couvre_gauche, out);
            compte_occurrences(right, false, out);
        }
        FilterExpression::Unary { operand, .. } => compte_occurrences(operand, false, out),
        FilterExpression::FunctionCall { args, .. } => {
            for a in args {
                compte_occurrences(a, false, out);
            }
        }
        FilterExpression::List(items) => {
            for i in items {
                compte_occurrences(i, false, out);
            }
        }
        FilterExpression::Map(pairs) => {
            for (_, v) in pairs {
                compte_occurrences(v, false, out);
            }
        }
        FilterExpression::IndexAccess { base, index } => {
            compte_occurrences(base, false, out);
            compte_occurrences(index, false, out);
        }
        FilterExpression::SliceAccess { base, start, end } => {
            compte_occurrences(base, false, out);
            if let Some(s) = start {
                compte_occurrences(s, false, out);
            }
            if let Some(e) = end {
                compte_occurrences(e, false, out);
            }
        }
        FilterExpression::Case {
            operand,
            when_clauses,
            else_clause,
        } => {
            if let Some(o) = operand {
                compte_occurrences(o, false, out);
            }
            for (quand, alors) in when_clauses {
                compte_occurrences(quand, false, out);
                compte_occurrences(alors, false, out);
            }
            if let Some(e) = else_clause {
                compte_occurrences(e, false, out);
            }
        }
        FilterExpression::ListComprehension {
            list_expr,
            filter_expr,
            map_expr,
            ..
        } => {
            compte_occurrences(list_expr, false, out);
            if let Some(f) = filter_expr {
                compte_occurrences(f, false, out);
            }
            compte_occurrences(map_expr, false, out);
        }
        FilterExpression::ListPredicate {
            list_expr,
            predicate,
            ..
        } => {
            compte_occurrences(list_expr, false, out);
            compte_occurrences(predicate, false, out);
        }
        _ => {}
    }
}

/// An expression-based predicate that evaluates logical expressions.
///
/// This predicate can evaluate complex expressions involving variables,
/// properties, and operators.
pub struct ExpressionPredicate {
    /// The expression to evaluate.
    expression: FilterExpression,
    /// Map from variable name to column index.
    variable_columns: HashMap<String, usize>,
    /// The graph store for property lookups.
    store: Arc<dyn GraphStore>,
    /// Transaction ID for MVCC-aware lookups.
    transaction_id: Option<TransactionId>,
    /// Viewing epoch for MVCC-aware lookups.
    viewing_epoch: Option<EpochId>,
    /// Session context for introspection functions (info, schema, current_schema, etc.).
    session_context: SessionContext,
    /// Première erreur d'évaluation (arithmétique gardée) en attente de relevé.
    ///
    /// Voir [`Predicate::take_eval_error`] : l'évaluation elle-même rend
    /// `Option<Value>` et n'a pas de canal d'erreur ; ce logement, protégé
    /// par un `Mutex` parce que `Predicate` exige `Send + Sync`, porte la
    /// faute jusqu'à l'opérateur qui saura la transformer en erreur de
    /// requête.
    eval_error: std::sync::Mutex<Option<String>>,
    /// Propriétés référencées par l'expression, avec leur extraction du chunk
    /// en cours (voir [`ProprietePreExtraite`] et `Predicate::prepare_chunk`).
    ///
    /// `Mutex` pour la même raison que `eval_error` : `Predicate` exige
    /// `Send + Sync`, et l'annonce du chunk arrive par `&self`. Le verrou est
    /// pris une fois par LIGNE (et non une fois par cellule lue au store
    /// comme avant) : un `Mutex` non contentieux vaut ~20 ns, à comparer aux
    /// ~1 µs/cellule de la route scalaire qu'il remplace.
    props_chunk: std::sync::Mutex<Vec<ProprietePreExtraite>>,
    /// Prédicats texte pré-évalués par chunk (voir [`VerdictTexte`]) : le canal
    /// qui évite toute allocation par cellule pour `CONTAINS` / `STARTS WITH` /
    /// `ENDS WITH`.
    verdicts_texte: std::sync::Mutex<Vec<VerdictTexte>>,
    /// Par couple (variable, propriété) : nombre de références, et combien sont
    /// couvertes par un verdict texte (voir [`compte_occurrences`]). Sert à ne
    /// PAS matérialiser une propriété que les verdicts suffisent à trancher.
    occurrences: Vec<(String, String, usize, usize)>,
}

/// A lazily-computed, cloneable value.
///
/// The factory runs at most once (via `OnceLock`). Cloning is cheap because
/// both the lock and the factory are behind `Arc`.
#[derive(Clone)]
pub struct LazyValue {
    cell: Arc<std::sync::OnceLock<Value>>,
    factory: Arc<dyn Fn() -> Value + Send + Sync>,
}

impl LazyValue {
    /// Creates a new lazy value with the given factory.
    pub fn new(factory: impl Fn() -> Value + Send + Sync + 'static) -> Self {
        Self {
            cell: Arc::new(std::sync::OnceLock::new()),
            factory: Arc::new(factory),
        }
    }

    /// Returns the value, computing it on first access.
    pub fn get(&self) -> &Value {
        self.cell.get_or_init(|| (self.factory)())
    }
}

impl std::fmt::Debug for LazyValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.cell.get() {
            Some(v) => write!(f, "LazyValue({v:?})"),
            None => write!(f, "LazyValue(<not yet computed>)"),
        }
    }
}

impl Default for LazyValue {
    fn default() -> Self {
        Self::new(|| Value::Null)
    }
}

/// Session-level context passed to the filter evaluator for introspection functions.
///
/// Lightweight strings (`current_schema`, `current_graph`) are stored directly.
/// Expensive introspection maps (`db_info`, `schema_info`) are lazily computed
/// on first access, so queries that never call `info()` or `schema()` pay zero cost.
#[derive(Debug, Clone, Default)]
pub struct SessionContext {
    /// (2026-08-17, aretes inter-bases V2) Registre de la session
    /// multi-store : `{slug -> store}`, ouverture paresseuse. `None` =
    /// session mono-base, comportement historique inchange.
    pub base_registry: Option<Arc<crate::execution::BaseRegistry>>,
    /// (V3) Contexte de droits du LECTEUR — evalue sur le store CIBLE a
    /// chaque franchissement de frontiere (`store_for_reader`). `None` =
    /// acces total, comportement historique.
    pub access_context: Option<Arc<crate::execution::AccessContext>>,
    /// (V3) Vrai dans une transaction EXPLICITE (START TRANSACTION actif,
    /// pas l'auto-commit) : les ecritures inter-bases y sont refusees avec
    /// un message enseignant tant que le registre externe est hors
    /// undo-log.
    pub in_explicit_transaction: bool,
    /// Current session schema name (for `CURRENT_SCHEMA`).
    pub current_schema: Option<String>,
    /// Current session graph name (for `CURRENT_GRAPH`).
    pub current_graph: Option<String>,
    /// Lazily-computed `info()` result.
    pub db_info: LazyValue,
    /// Lazily-computed `schema()` result.
    pub schema_info: LazyValue,
}

/// A filter expression that can be evaluated.
#[derive(Debug, Clone)]
pub enum FilterExpression {
    /// A literal value.
    Literal(Value),
    /// A variable reference (column index).
    Variable(String),
    /// Property access on a variable.
    Property {
        /// The variable name.
        variable: String,
        /// The property name.
        property: String,
    },
    /// Binary operation.
    Binary {
        /// Left operand.
        left: Box<FilterExpression>,
        /// Operator.
        op: BinaryFilterOp,
        /// Right operand.
        right: Box<FilterExpression>,
    },
    /// Unary operation.
    Unary {
        /// Operator.
        op: UnaryFilterOp,
        /// Operand.
        operand: Box<FilterExpression>,
    },
    /// Function call.
    FunctionCall {
        /// Function name (e.g., "id", "labels", "type", "size", "coalesce", "exists").
        name: String,
        /// Arguments.
        args: Vec<FilterExpression>,
    },
    /// List literal.
    List(Vec<FilterExpression>),
    /// Map literal (e.g., {name: 'Alix', age: 30}).
    Map(Vec<(String, FilterExpression)>),
    /// Index access (e.g., `list[0]`).
    IndexAccess {
        /// The base expression.
        base: Box<FilterExpression>,
        /// The index expression.
        index: Box<FilterExpression>,
    },
    /// Slice access (e.g., list[1..3]).
    SliceAccess {
        /// The base expression.
        base: Box<FilterExpression>,
        /// Start index (None means from beginning).
        start: Option<Box<FilterExpression>>,
        /// End index (None means to end).
        end: Option<Box<FilterExpression>>,
    },
    /// CASE expression.
    Case {
        /// Test expression (for simple CASE).
        operand: Option<Box<FilterExpression>>,
        /// WHEN clauses (condition, result).
        when_clauses: Vec<(FilterExpression, FilterExpression)>,
        /// ELSE clause.
        else_clause: Option<Box<FilterExpression>>,
    },
    /// Entity ID access.
    Id(String),
    /// Node labels access.
    Labels(String),
    /// Edge type access.
    Type(String),
    /// List comprehension: [x IN list WHERE predicate | expression]
    ListComprehension {
        /// Variable name for each element.
        variable: String,
        /// The source list expression.
        list_expr: Box<FilterExpression>,
        /// Optional filter predicate.
        filter_expr: Option<Box<FilterExpression>>,
        /// The mapping expression for each element.
        map_expr: Box<FilterExpression>,
    },
    /// List predicate: all/any/none/single(x IN list WHERE pred).
    ListPredicate {
        /// The kind of list predicate.
        kind: ListPredicateKind,
        /// The iteration variable name.
        variable: String,
        /// The source list expression.
        list_expr: Box<FilterExpression>,
        /// The predicate to test for each element.
        predicate: Box<FilterExpression>,
    },
    /// EXISTS subquery: evaluates inner plan and returns true if results exist.
    ExistsSubquery {
        /// The start node variable from outer query.
        start_var: String,
        /// Direction of edge traversal.
        direction: Direction,
        /// Edge type filter (empty = match all types, multiple = match any).
        edge_types: Vec<String>,
        /// Optional end node labels filter.
        end_labels: Option<Vec<String>>,
        /// Minimum number of hops (for variable-length patterns).
        min_hops: Option<u32>,
        /// Maximum number of hops (for variable-length patterns).
        max_hops: Option<u32>,
    },
    /// COUNT subquery: counts matching edges from a node (fast path).
    CountSubquery {
        /// The start node variable from outer query.
        start_var: String,
        /// Direction of edge traversal.
        direction: Direction,
        /// Edge type filter (empty = match all types, multiple = match any).
        edge_types: Vec<String>,
    },
    /// reduce() accumulator: `reduce(acc = init, x IN list | expr)`.
    Reduce {
        /// Accumulator variable name.
        accumulator: String,
        /// Initial value for the accumulator.
        initial: Box<FilterExpression>,
        /// Iteration variable name.
        variable: String,
        /// List to iterate over.
        list: Box<FilterExpression>,
        /// Body expression (references both accumulator and variable).
        expression: Box<FilterExpression>,
    },
}

/// The kind of list predicate function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListPredicateKind {
    /// all(x IN list WHERE pred): true if pred holds for every element.
    All,
    /// any(x IN list WHERE pred): true if pred holds for at least one element.
    Any,
    /// none(x IN list WHERE pred): true if pred holds for no element.
    None,
    /// single(x IN list WHERE pred): true if pred holds for exactly one element.
    Single,
}

/// Binary operators for filter expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryFilterOp {
    /// Equal.
    Eq,
    /// Not equal.
    Ne,
    /// Less than.
    Lt,
    /// Less than or equal.
    Le,
    /// Greater than.
    Gt,
    /// Greater than or equal.
    Ge,
    /// Logical AND.
    And,
    /// Logical OR.
    Or,
    /// Logical XOR.
    Xor,
    /// Addition.
    Add,
    /// Subtraction.
    Sub,
    /// Multiplication.
    Mul,
    /// Division.
    Div,
    /// Modulo.
    Mod,
    /// String starts with.
    StartsWith,
    /// String ends with.
    EndsWith,
    /// String contains.
    Contains,
    /// List membership.
    In,
    /// Regex match (=~).
    Regex,
    /// Power/exponentiation (^).
    Pow,
    /// SQL LIKE pattern matching (% = any chars, _ = single char).
    Like,
    /// String concatenation (||).
    Concat,
}

/// Unary operators for filter expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryFilterOp {
    /// Logical NOT.
    Not,
    /// IS NULL.
    IsNull,
    /// IS NOT NULL.
    IsNotNull,
    /// Numeric negation.
    Neg,
}

impl ExpressionPredicate {
    /// Creates a new expression predicate.
    pub fn new(
        expression: FilterExpression,
        variable_columns: HashMap<String, usize>,
        store: Arc<dyn GraphStore>,
    ) -> Self {
        // Ce que l'expression référence est connu ICI et ne changera plus :
        // on collecte les couples (variable, propriété) et on construit leurs
        // clés une seule fois, plutôt que par cellule évaluée.
        // Interrupteur de la voie des verdicts (`OBRAIN_VERDICTS_TEXTE=0` la
        // coupe). Il sert à DEUX choses, et la première est la plus importante :
        // prouver que les lignes rendues sont IDENTIQUES avec et sans cette voie
        // (mêmes résultats, pas seulement mêmes tests) ; la seconde est de
        // mesurer les deux dans le MÊME binaire, donc sous la même charge.
        let mut predicats: Vec<(String, String, BinaryFilterOp, String)> = Vec::new();
        if std::env::var("OBRAIN_VERDICTS_TEXTE").map_or(true, |v| v != "0") {
            collecte_predicats_texte(&expression, &mut predicats);
        }
        let mut occurrences: Vec<(String, String, usize, usize)> = Vec::new();
        compte_occurrences(&expression, false, &mut occurrences);

        let mut paires: Vec<(String, String)> = Vec::new();
        collecte_proprietes(&expression, &mut paires);
        // → une propriété dont TOUTES les références sont couvertes par un
        //   verdict texte n'est pas matérialisée : le verdict suffit, et le
        //   `Value` par cellule n'aurait servi à personne.
        let _ = &occurrences;
        let props_chunk = paires
            .into_iter()
            .map(|(variable, propriete)| ProprietePreExtraite {
                cle: PropertyKey::new(propriete.as_str()),
                variable,
                propriete,
                lignes: Vec::new(),
            })
            .collect();
        let verdicts_texte = predicats
            .into_iter()
            .map(|(variable, propriete, op, aiguille)| VerdictTexte {
                variable,
                propriete,
                op,
                aiguille,
                ids: Vec::new(),
                verdicts: Vec::new(),
                vues: 0,
                non_nulles: 0,
                actif: true,
            })
            .collect();
        trace_filtre::ACTIVE.store(
            std::env::var_os("OBRAIN_FILTRE_TRACE").is_some(),
            std::sync::atomic::Ordering::Relaxed,
        );
        Self {
            expression,
            variable_columns,
            store,
            transaction_id: None,
            viewing_epoch: None,
            session_context: SessionContext::default(),
            eval_error: std::sync::Mutex::new(None),
            props_chunk: std::sync::Mutex::new(props_chunk),
            verdicts_texte: std::sync::Mutex::new(verdicts_texte),
            occurrences,
        }
    }

    /// Consigne une erreur d'évaluation (la PREMIÈRE gagne) et rend `None`.
    ///
    /// `None` fait sortir proprement de l'évaluation en cours ; l'opérateur
    /// appelant relèvera la faute via [`ExpressionPredicate::take_eval_error`]
    /// et refusera la requête. Ne sert qu'aux fautes qui doivent REFUSER la
    /// requête (division/modulo par zéro, débordement i64) — jamais aux
    /// simples absences de valeur.
    fn signale_erreur(&self, msg: String) -> Option<Value> {
        let mut slot = self.eval_error.lock().unwrap_or_else(|e| e.into_inner());
        slot.get_or_insert(msg);
        None
    }

    /// Relève (et efface) la première erreur d'évaluation consignée.
    ///
    /// Méthode inhérente pour que les détenteurs directs (`ProjectOperator`,
    /// via `eval_at`) n'aient pas besoin d'importer le trait [`Predicate`].
    pub fn take_eval_error(&self) -> Option<String> {
        self.eval_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    /// Lit une propriété à l'ÉPOQUE demandée, quand le store courant ne l'a
    /// plus.
    ///
    /// C'est le seul chemin qui reste après qu'une cellule pré-extraite a été
    /// déclarée absente : `resolve_node` rend le nœud tel qu'il était vu à
    /// l'époque de la transaction, et c'est de LUI que vient la valeur — le
    /// store courant, lui, vient d'être interrogé par le lot (mêmes zones,
    /// même ordre) et n'a rien.
    fn propriete_a_l_epoque(&self, node_id: NodeId, property: &str) -> Option<Value> {
        self.resolve_node(node_id)
            .and_then(|node| node.get_property(property).cloned())
    }

    /// Extrait, EN UN LOT, chaque propriété référencée par l'expression pour
    /// TOUT le chunk annoncé.
    ///
    /// C'est ici que le prix par cellule tombe : une lecture
    /// (`get_node_property_batch`) pour tout le chunk au lieu d'une par ligne,
    /// et une seule lecture par couple (variable, propriété) même quand le
    /// prédicat l'écrit plusieurs fois (`OR`, `CASE` répétant le même test).
    ///
    /// Le verrou `props_chunk` est tenu pendant la lecture du store. C'est sans
    /// risque de cycle : le store ne rappelle JAMAIS un prédicat, et un prédicat
    /// n'appartient qu'à un opérateur, lui-même tenu par un seul fil.
    fn pre_extraire_chunk(&self, chunk: &DataChunk) {
        let trace = std::env::var_os("OBRAIN_FILTRE_TRACE").is_some();
        self.pre_extraire_verdicts(chunk);
        let debut = web_time::Instant::now();
        let mut entrees = self.props_chunk.lock().unwrap_or_else(|e| e.into_inner());
        if entrees.is_empty() {
            // Diagnostic : `prepare_chunk` EST appelé mais l'expression ne
            // référence AUCUNE propriété reconnue par `collecte_proprietes`.
            // Sans cette ligne, l'absence de trace se confond avec « le crochet
            // n'est pas appelé » — deux causes très différentes (expression
            // non reconnue ici, ou autre opérateur qui n'annonce pas le chunk).
            if trace_filtre::actif() {
                eprintln!("[filtre] prepare_chunk appelé, aucune propriété collectée");
            }
            return;
        }
        let lignes = chunk.total_row_count();
        for entree in entrees.iter_mut() {
            entree.lignes.clear();
            // Couverte par des verdicts actifs : rien à matérialiser. Les
            // verdicts ont été extraits AVANT cet appel, donc la densité du
            // chunk courant est déjà connue — on ne paie pas une lecture pour
            // une propriété que la voie sans allocation tranche déjà.
            if self.couverte_par_verdicts_actifs(&entree.variable, &entree.propriete) {
                continue;
            }
            let Some(colonne) = self
                .variable_columns
                .get(&entree.variable)
                .and_then(|i| chunk.column(*i))
            else {
                // Variable absente du chunk (plan inattendu) : route scalaire.
                continue;
            };
            let n = lignes.min(colonne.len());
            let mut ids: Vec<NodeId> = Vec::with_capacity(n);
            for r in 0..n {
                match colonne.get_node_id(r) {
                    Some(id) => ids.push(id),
                    // La colonne ne porte pas d'identifiants de nœud (carte
                    // issue d'un UNWIND, liste…) : rien à extraire. On exige
                    // TOUTES les lignes en nœud — un mélange nœud/non-nœud
                    // rendrait la preuve de validité fausse.
                    None => {
                        ids.clear();
                        break;
                    }
                }
            }
            if ids.is_empty() {
                continue;
            }
            let t_lot = web_time::Instant::now();
            let valeurs = self.store.get_node_property_batch(&ids, &entree.cle);
            let ms_lot = t_lot.elapsed().as_secs_f64() * 1000.0;
            if trace {
                let presents = valeurs.iter().filter(|v| v.is_some()).count();
                eprintln!(
                    "[filtre] {}.{} : {} lignes, {} valeurs présentes, lot {:.2} ms ({:.0} ns/ligne)",
                    entree.variable,
                    entree.propriete,
                    ids.len(),
                    presents,
                    ms_lot,
                    ms_lot * 1e6 / ids.len() as f64
                );
            }
            entree.lignes = ids.into_iter().zip(valeurs).collect();
        }
        if trace {
            eprintln!(
                "[filtre] chunk de {} lignes préparé en {:.2} ms — {} · époque vue {:?} · courante {:?} · tx {:?}",
                lignes,
                debut.elapsed().as_secs_f64() * 1000.0,
                trace_filtre::ligne(),
                self.viewing_epoch,
                self.store.current_epoch(),
                self.transaction_id
            );
            trace_filtre::remise_a_zero();
        }
    }

    /// La propriété est-elle ENTIÈREMENT couverte par des verdicts ACTIFS ?
    ///
    /// Si oui, la matérialiser serait un travail perdu : chaque référence est
    /// déjà tranchée sur les octets, et un `Value` par cellule n'irait nulle
    /// part. Si un seul verdict du couple est retombé (densité insuffisante),
    /// on répond NON : la lecture de valeurs reprend ses droits pour tout le
    /// couple, sans état intermédiaire à moitié valide.
    fn couverte_par_verdicts_actifs(&self, variable: &str, propriete: &str) -> bool {
        let Some(&(_, _, total, couvertes)) = self
            .occurrences
            .iter()
            .find(|(v, p, _, _)| v == variable && p == propriete)
        else {
            return false;
        };
        if total == 0 || total != couvertes {
            return false;
        }
        let verdicts = self
            .verdicts_texte
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut au_moins_un = false;
        for entree in verdicts
            .iter()
            .filter(|e| e.variable == variable && e.propriete == propriete)
        {
            if !entree.actif {
                return false;
            }
            au_moins_un = true;
        }
        au_moins_un
    }

    /// Pré-évalue, POUR TOUT LE CHUNK, les prédicats texte reconnus — les
    /// verdicts viennent du substrat, lus sur l'arène mmap, sans allouer.
    fn pre_extraire_verdicts(&self, chunk: &DataChunk) {
        let mut entrees = self
            .verdicts_texte
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if entrees.is_empty() {
            return;
        }
        let lignes = chunk.total_row_count();
        for entree in entrees.iter_mut() {
            entree.ids.clear();
            entree.verdicts.clear();
        }
        // Les verdicts sont demandés GROUPÉS PAR (variable, propriété) : une
        // seule traversée de chaîne pour toutes les aiguilles de la même
        // propriété (`CONTAINS 'a' OR CONTAINS 'b'` demandait sinon N fois le
        // même parcours — mesuré 270 ms contre 218 ms pour 347 970 cellules).
        let mut deja_fait: Vec<(String, String)> = Vec::new();
        for idx in 0..entrees.len() {
            let variable = entrees[idx].variable.clone();
            let propriete = entrees[idx].propriete.clone();
            if deja_fait
                .iter()
                .any(|(v, p)| v == &variable && p == &propriete)
            {
                continue;
            }
            deja_fait.push((variable.clone(), propriete.clone()));
            let groupe: Vec<usize> = (0..entrees.len())
                .filter(|&i| entrees[i].variable == variable && entrees[i].propriete == propriete)
                .collect();
            let Some(colonne) = self
                .variable_columns
                .get(&variable)
                .and_then(|i| chunk.column(*i))
            else {
                continue;
            };
            let n = lignes.min(colonne.len());
            let mut ids: Vec<NodeId> = Vec::with_capacity(n);
            for r in 0..n {
                match colonne.get_node_id(r) {
                    Some(id) => ids.push(id),
                    None => {
                        ids.clear();
                        break;
                    }
                }
            }
            if ids.is_empty() {
                continue;
            }
            let predicats: Vec<PredicatTexte<'_>> = groupe
                .iter()
                .filter_map(|&i| {
                    let entree = &entrees[i];
                    Some(match entree.op {
                        BinaryFilterOp::Contains => PredicatTexte::Contient(entree.aiguille.as_str()),
                        BinaryFilterOp::StartsWith => {
                            PredicatTexte::CommencePar(entree.aiguille.as_str())
                        }
                        BinaryFilterOp::EndsWith => PredicatTexte::FiniPar(entree.aiguille.as_str()),
                        // `collecte_predicats_texte` ne retient que ces trois-là.
                        _ => return None,
                    })
                })
                .collect();
            if predicats.len() != groupe.len() {
                continue;
            }
            let cle = PropertyKey::new(propriete.as_str());
            // Fin de l'emprunt immuable d'`entrees` ici : la suite écrit.
            let Some(par_predicat) = self.store.verdicts_texte_batch(&ids, &cle, &predicats) else {
                continue;
            };
            if par_predicat.len() != groupe.len() {
                continue;
            }
            for (rang, &i) in groupe.iter().enumerate() {
                let verdicts = par_predicat[rang].clone();
                let entree = &mut entrees[i];
                entree.vues += verdicts.len() as u64;
                entree.non_nulles += verdicts.iter().filter(|v| v.is_some()).count() as u64;
                // La densité observée tranche : sous le seuil, la voie est
                // abandonnée et la lecture de valeurs reprend la propriété.
                if entree.vues >= OBSERVATIONS_MIN_VERDICTS
                    && entree.non_nulles * DENSITE_MIN_VERDICTS < entree.vues
                {
                    entree.actif = false;
                }
                entree.ids = ids.clone();
                entree.verdicts = verdicts;
            }
        }
    }

    /// Verdict pré-évalué pour cette ligne, si la forme ET la preuve concordent.
    fn verdict_texte(
        &self,
        chunk: &DataChunk,
        row: usize,
        gauche: &FilterExpression,
        droite: &FilterExpression,
        op: BinaryFilterOp,
    ) -> ReponseVerdict {
        if !matches!(
            op,
            BinaryFilterOp::Contains | BinaryFilterOp::StartsWith | BinaryFilterOp::EndsWith
        ) {
            return ReponseVerdict::Aucun;
        }
        let FilterExpression::Property {
            variable,
            property,
        } = gauche
        else {
            return ReponseVerdict::Aucun;
        };
        let FilterExpression::Literal(Value::String(aiguille)) = droite else {
            return ReponseVerdict::Aucun;
        };
        let Some(colonne) = self
            .variable_columns
            .get(variable)
            .and_then(|i| chunk.column(*i))
        else {
            return ReponseVerdict::Aucun;
        };
        let Some(node_id) = colonne.get_node_id(row) else {
            return ReponseVerdict::Aucun;
        };
        let entrees = self
            .verdicts_texte
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(entree) = entrees.iter().find(|e| {
            e.variable == *variable
                && e.propriete == *property
                && e.op == op
                && e.aiguille == aiguille.as_str()
        }) else {
            return ReponseVerdict::Aucun;
        };
        if !entree.actif {
            return ReponseVerdict::Aucun;
        }
        match (entree.ids.get(row), entree.verdicts.get(row)) {
            // LA PREUVE : même nœud ⇒ même verdict. Sinon chemin d'origine.
            (Some(id), Some(verdict)) if *id == node_id => match verdict {
                Some(b) => ReponseVerdict::Verdict(*b),
                None => ReponseVerdict::Nul,
            },
            _ => ReponseVerdict::Aucun,
        }
    }

    /// Lit la valeur pré-extraite de `variable.propriete` pour cette ligne.
    ///
    /// Rend [`ReponsePre::Inconnue`] dès que la preuve manque (cache vide,
    /// ligne hors cache, identifiant de nœud différent) : l'appelant reprend
    /// alors la route scalaire, qui n'a pas été modifiée d'un octet.
    fn propriete_pre_extraite(
        &self,
        node_id: Option<NodeId>,
        row: usize,
        variable: &str,
        property: &str,
    ) -> ReponsePre {
        let trace = trace_filtre::actif();
        let entrees = self.props_chunk.lock().unwrap_or_else(|e| e.into_inner());
        let Some(entree) = entrees
            .iter()
            .find(|e| e.variable == variable && e.propriete == property)
        else {
            if trace {
                trace_filtre::INCONNUE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            return ReponsePre::Inconnue;
        };
        let Some((id_extrait, valeur)) = entree.lignes.get(row) else {
            if trace {
                trace_filtre::INCONNUE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            return ReponsePre::Inconnue;
        };
        // LA PREUVE, et elle tient en une comparaison : la valeur d'une
        // propriété ne dépend que de (identifiant, clé). Même identifiant à
        // cette ligne ⇒ même valeur. Un autre chunk rend un autre identifiant,
        // et la route scalaire reprend la main — aucune valeur fausse ne peut
        // être servie, même par un `eval_at` qui n'a rien préparé.
        if node_id != Some(*id_extrait) {
            if trace {
                trace_filtre::INCONNUE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            return ReponsePre::Inconnue;
        }
        match valeur {
            Some(v) => {
                if trace {
                    trace_filtre::VALEUR.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                ReponsePre::Valeur(v.clone())
            }
            None => {
                if trace {
                    trace_filtre::ABSENTE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                ReponsePre::Absente
            }
        }
    }

    /// Sets the transaction context for MVCC-aware property lookups.
    pub fn with_transaction_context(
        mut self,
        epoch: EpochId,
        transaction_id: Option<TransactionId>,
    ) -> Self {
        self.viewing_epoch = Some(epoch);
        self.transaction_id = transaction_id;
        self
    }

    /// Sets the session context for introspection functions.
    pub fn with_session_context(mut self, context: SessionContext) -> Self {
        self.session_context = context;
        self
    }

    /// Le repli d'ÉPOQUE peut-il apprendre quelque chose que le store courant
    /// ignore ?
    ///
    /// POURQUOI CETTE QUESTION (2026-09-12, mesuré). Le filtre appelait
    /// `resolve_node` — donc `get_node_versioned`/`get_node_at_epoch`, qui
    /// HYDRATENT TOUTES les propriétés du nœud — dès qu'une propriété manquait
    /// et qu'une époque était posée. Or le planificateur pose TOUJOURS une
    /// époque : sur une lecture ordinaire on lit `EpochId(0)` alors que
    /// `current_epoch()` vaut `EpochId(0)`, sans transaction. Lire l'époque
    /// COURANTE sans transaction, c'est lire l'état du store : le repli ne peut
    /// rien rendre de plus que le `None` déjà obtenu, et il le paie au prix
    /// d'une hydratation complète du nœud, PAR LIGNE.
    ///
    /// Coût mesuré de ce zéro (snapshot figé, 347 970 nœuds) :
    /// « balayage + 1 cellule ABSENTE » = 1 280 ms, contre 400 ms pour la même
    /// cellule PRÉSENTE — soit 3,3 µs/ligne pour rendre `None`.
    ///
    /// Une propriété absente est le cas le PLUS fréquent d'un filtre (la
    /// plupart des nœuds n'ont pas la plupart des propriétés) : ce n'était pas
    /// un cas de bord, c'était le régime normal.
    ///
    /// Le repli reste donc entier dès qu'il peut voir autre chose : époque
    /// différente de la courante (lecture historique) ou transaction en cours
    /// (écritures non commitées, que la vue versionnée seule connaît).
    fn repli_epoque_utile(&self) -> bool {
        match self.viewing_epoch {
            None => false,
            Some(epoque) => self.transaction_id.is_some() || epoque != self.store.current_epoch(),
        }
    }

    /// Resolves a node using transaction-aware access when available.
    fn resolve_node(&self, node_id: NodeId) -> Option<Node> {
        if let (Some(ep), Some(tx)) = (self.viewing_epoch, self.transaction_id) {
            self.store.get_node_versioned(node_id, ep, tx)
        } else if let Some(ep) = self.viewing_epoch {
            self.store.get_node_at_epoch(node_id, ep)
        } else {
            self.store.get_node(node_id)
        }
    }

    /// Resolves an edge using transaction-aware access when available.
    fn resolve_edge(&self, edge_id: obrain_common::types::EdgeId) -> Option<Edge> {
        if let (Some(ep), Some(tx)) = (self.viewing_epoch, self.transaction_id) {
            self.store.get_edge_versioned(edge_id, ep, tx)
        } else if let Some(ep) = self.viewing_epoch {
            self.store.get_edge_at_epoch(edge_id, ep)
        } else {
            self.store.get_edge(edge_id)
        }
    }

    /// Evaluates the expression for a specific row in a chunk, returning the result value.
    /// This is useful for evaluating expressions in contexts like RETURN clauses.
    pub fn eval_at(&self, chunk: &DataChunk, row: usize) -> Option<Value> {
        self.eval_expr(&self.expression, chunk, row)
    }

    /// Evaluates the expression for a row, returning the result value.
    fn eval(&self, chunk: &DataChunk, row: usize) -> Option<Value> {
        self.eval_expr(&self.expression, chunk, row)
    }

    /// `split(s, d)[k]` — rend le k-ième token SANS matérialiser la liste.
    ///
    /// Voir `ReponseSplitIndexe` pour la mesure qui justifie ce chemin et pour
    /// l'inventaire des cas limites suivis à l'identique.
    ///
    /// L'ORDRE D'ÉVALUATION EST CELUI DE LA VOIE GÉNÉRALE : les arguments de
    /// `split` (gauche → droite), PUIS l'index. Il compte : quand `split` rend
    /// `None` (argument non textuel), la voie générale sort par `?` et
    /// n'évalue JAMAIS l'index. On sort donc au même endroit, sans évaluer
    /// l'index — un `UNWIND` ou une division gardée dans l'index ne doit pas se
    /// mettre à s'exécuter parce qu'on a optimisé.
    fn split_indexe_direct(
        &self,
        base: &FilterExpression,
        index: &FilterExpression,
        chunk: &DataChunk,
        row: usize,
    ) -> ReponseSplitIndexe {
        let FilterExpression::FunctionCall { name, args } = base else {
            return ReponseSplitIndexe::Aucun;
        };
        if args.len() != 2 || !name.eq_ignore_ascii_case("split") {
            return ReponseSplitIndexe::Aucun;
        }
        // Types : `split` rend `None` dès qu'un argument n'est pas une chaîne.
        let Some(Value::String(s)) = self.eval_expr(&args[0], chunk, row) else {
            return ReponseSplitIndexe::Valeur(None);
        };
        let Some(Value::String(delim)) = self.eval_expr(&args[1], chunk, row) else {
            return ReponseSplitIndexe::Valeur(None);
        };
        let Some(Value::Int64(k)) = self.eval_expr(index, chunk, row) else {
            // Index absent ou non entier : la voie générale a d'autres bras que
            // celui de la liste (`(_, Value::String(key))`…). On la laisse
            // trancher plutôt que de réécrire ici une règle qu'elle seule porte.
            return ReponseSplitIndexe::Aucun;
        };
        let token = |rang: usize| -> Option<Value> {
            s.split(delim.as_str())
                .nth(rang)
                .map(|p| Value::String(p.to_string().into()))
        };
        if k >= 0 {
            return ReponseSplitIndexe::Valeur(token(k as usize));
        }
        // Index NÉGATIF : il faut la longueur, donc un comptage — mais jamais la
        // liste allouée. `count()` consomme le même itérateur que la voie
        // générale, qui construisait la `Vec` uniquement pour en lire `.len()`.
        let len = s.split(delim.as_str()).count() as i64;
        let rang = len + k;
        if rang < 0 {
            return ReponseSplitIndexe::Valeur(None);
        }
        ReponseSplitIndexe::Valeur(token(rang as usize))
    }

    fn eval_expr(&self, expr: &FilterExpression, chunk: &DataChunk, row: usize) -> Option<Value> {
        match expr {
            FilterExpression::Literal(v) => Some(v.clone()),
            FilterExpression::Variable(name) => {
                let col_idx = *self.variable_columns.get(name)?;
                chunk.column(col_idx)?.get_value(row)
            }
            FilterExpression::Property { variable, property } => {
                let col_idx = *self.variable_columns.get(variable)?;
                let col = chunk.column(col_idx)?;
                // L'identifiant est lu UNE fois : il sert de clé de cache ET de
                // preuve de validité (voir `propriete_pre_extraite`).
                let node_id = col.get_node_id(row);
                if node_id.is_some() {
                    // PRÉ-EXTRAIT PAR CHUNK (voir `Predicate::prepare_chunk`) :
                    // une lecture par chunk et par couple (variable, propriété),
                    // au lieu d'une par ligne ET par occurrence du prédicat.
                    match self.propriete_pre_extraite(node_id, row, variable, property) {
                        ReponsePre::Valeur(v) => return Some(v),
                        // ABSENTE : le lot vient d'interroger les MÊMES quatre
                        // zones dans le MÊME ordre que `get_node_property`.
                        // Rappeler le scalaire ne peut donc rien apprendre de
                        // neuf — et ce rappel coûte mesuré ~3,3 µs/ligne : sur
                        // le snapshot figé, « balayage + 1 cellule ABSENTE »
                        // prend 1 280 ms contre 400 ms pour une cellule
                        // PRÉSENTE, écart qui EST ce rappel.
                        //
                        // Ce qui reste, en revanche, est irremplaçable : le
                        // repli d'ÉPOQUE, seule voie capable de voir une valeur
                        // que le store COURANT n'a plus. On le garde tel quel.
                        ReponsePre::Absente => {
                            if !self.repli_epoque_utile() {
                                return None;
                            }
                            return self.propriete_a_l_epoque(node_id?, property);
                        }
                        ReponsePre::Inconnue => {}
                    }
                }
                // T17k : single-property access. Route directly through
                // `store.get_node_property` / `get_edge_property` which
                // do a targeted read (O(1) mmap lookup per zone) instead
                // of going through `get_node` which bulk-hydrates all
                // ~N properties of the entity (O(N) lock-per-column).
                // On wide entities (File with 57 cols), bulk hydration
                // per-row turns a 21 ms query into 1.4 s. Scalar route
                // is ~1 μs per cell.
                let key = obrain_common::types::PropertyKey::new(property);
                if let Some(node_id) = col.get_node_id(row) {
                    // Fast path : direct scalar lookup on the store.
                    if let Some(v) = self.store.get_node_property(node_id, &key) {
                        return Some(v);
                    }
                    // Fall through to MVCC-aware path only when the
                    // scalar store misses (lets `get_node_at_epoch` kick
                    // in for historical reads).
                    // …et seulement si ce repli peut voir autre chose que le
                    // store courant : à l'époque courante sans transaction, il
                    // hydratait tout le nœud pour rendre `None` (voir
                    // `repli_epoque_utile`).
                    if self.repli_epoque_utile() {
                        return self.propriete_a_l_epoque(node_id, property);
                    }
                    return None;
                }
                if let Some(edge_id) = col.get_edge_id(row) {
                    if let Some(v) = self.store.get_edge_property(edge_id, &key) {
                        return Some(v);
                    }
                    if self.repli_epoque_utile()
                        && let Some(edge) = self.resolve_edge(edge_id)
                    {
                        return edge.get_property(property).cloned();
                    }
                    return None;
                }
                // Try as map value (e.g. from UNWIND with map elements)
                if let Some(Value::Map(map)) = col.get_value(row) {
                    return map.get(&key).cloned();
                }
                None
            }
            FilterExpression::Binary { left, op, right } => {
                // VOIE DES VERDICTS (2026-09-12) : pour `CONTAINS` / `STARTS WITH`
                // / `ENDS WITH` sur une propriété face à un littéral, le substrat
                // a déjà répondu sur les octets mmap — sans matérialiser de
                // `Value`, donc sans une allocation par cellule. `Nul` est le NULL
                // que l'évaluateur rendrait (valeur absente ou non textuelle).
                match self.verdict_texte(chunk, row, left, right, *op) {
                    ReponseVerdict::Verdict(b) => return Some(Value::Bool(b)),
                    ReponseVerdict::Nul => return None,
                    ReponseVerdict::Aucun => {}
                }
                // For IN operator, right side is a list that we evaluate specially
                if *op == BinaryFilterOp::In {
                    let left_val = self.eval_expr(left, chunk, row)?;
                    return self.eval_in_operator(&left_val, right, chunk, row);
                }
                // D7 (lot 02/04) : PAS de `?` ici. Une propriété absente
                // remonte `None` ; le court-circuit Rust avalait TOUTE
                // l'expression AVANT la table de vérité ternaire, et
                // `UNKNOWN OR TRUE` perdait la ligne. L'absence de valeur
                // EST l'UNKNOWN de la logique trivaluée : on le matérialise
                // en `Value::Null` et on laisse la table décider (absorption
                // comprise : U AND F → F, U OR T → T). Les erreurs signalées
                // via `signale_erreur` survivent — le slot est relevé par
                // l'opérateur après coup, indépendamment de la valeur rendue.
                let left_val = self
                    .eval_expr(left, chunk, row)
                    .unwrap_or(Value::Null);
                let right_val = self
                    .eval_expr(right, chunk, row)
                    .unwrap_or(Value::Null);
                self.eval_binary_op(&left_val, *op, &right_val)
            }
            FilterExpression::Unary { op, operand } => {
                let val = self.eval_expr(operand, chunk, row);
                self.eval_unary_op(*op, val)
            }
            FilterExpression::FunctionCall { name, args } => {
                self.eval_function(name, args, chunk, row)
            }
            FilterExpression::List(items) => {
                let values: Vec<Value> = items
                    .iter()
                    .filter_map(|item| self.eval_expr(item, chunk, row))
                    .collect();
                Some(Value::List(values.into()))
            }
            FilterExpression::Map(pairs) => {
                let mut map = BTreeMap::new();
                for (k, v) in pairs {
                    if let Some(val) = self.eval_expr(v, chunk, row) {
                        if k == "*" {
                            // AllProperties marker: flatten the inner map into the result
                            if let Value::Map(inner) = val {
                                map.extend(inner.iter().map(|(pk, pv)| (pk.clone(), pv.clone())));
                            }
                        } else {
                            map.insert(PropertyKey::new(k.as_str()), val);
                        }
                    }
                }
                Some(Value::Map(Arc::new(map)))
            }
            FilterExpression::IndexAccess { base, index } => {
                // CHEMIN RAPIDE `split(s, d)[k]` : voir `ReponseSplitIndexe`.
                // Placé AVANT l'évaluation de `base`, parce que c'est justement
                // l'évaluation de `base` (qui alloue la liste entière) qu'on
                // cherche à ne pas payer.
                if let ReponseSplitIndexe::Valeur(v) =
                    self.split_indexe_direct(base, index, chunk, row)
                {
                    return v;
                }
                let base_val = self.eval_expr(base, chunk, row)?;
                let index_val = self.eval_expr(index, chunk, row)?;
                match (&base_val, &index_val) {
                    (Value::List(items), Value::Int64(i)) => {
                        let idx = if *i < 0 {
                            // Negative indexing from end
                            let len = items.len() as i64;
                            (len + i) as usize
                        } else {
                            *i as usize
                        };
                        items.get(idx).cloned()
                    }
                    (Value::String(s), Value::Int64(i)) => {
                        let idx = if *i < 0 {
                            let len = s.len() as i64;
                            (len + i) as usize
                        } else {
                            *i as usize
                        };
                        s.chars()
                            .nth(idx)
                            .map(|c| Value::String(c.to_string().into()))
                    }
                    (Value::Map(m), Value::String(key)) => {
                        let prop_key = PropertyKey::new(key.as_str());
                        m.get(&prop_key).cloned()
                    }
                    (_, Value::String(key)) => {
                        // Node/edge bracket access: n['name']. T17k :
                        // use the scalar store accessor (targeted zone
                        // lookup) instead of bulk-hydrating via
                        // `resolve_node` — same rationale as the
                        // FilterExpression::Property branch above.
                        if let FilterExpression::Variable(var) = base.as_ref()
                            && let Some(&col_idx) = self.variable_columns.get(var)
                            && let Some(col) = chunk.column(col_idx)
                        {
                            let prop_key = PropertyKey::new(key.as_str());
                            if let Some(node_id) = col.get_node_id(row) {
                                return self.store.get_node_property(node_id, &prop_key);
                            }
                            if let Some(edge_id) = col.get_edge_id(row) {
                                return self.store.get_edge_property(edge_id, &prop_key);
                            }
                        }
                        None
                    }
                    _ => None,
                }
            }
            FilterExpression::SliceAccess { base, start, end } => {
                let base_val = self.eval_expr(base, chunk, row)?;
                let start_idx = start
                    .as_ref()
                    .and_then(|s| self.eval_expr(s, chunk, row))
                    .and_then(|v| {
                        if let Value::Int64(i) = v {
                            Some(i as usize)
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0);

                match &base_val {
                    Value::List(items) => {
                        let end_idx = end
                            .as_ref()
                            .and_then(|e| self.eval_expr(e, chunk, row))
                            .and_then(|v| {
                                if let Value::Int64(i) = v {
                                    Some(i as usize)
                                } else {
                                    None
                                }
                            })
                            .unwrap_or(items.len());
                        let sliced: Vec<Value> = items
                            .get(start_idx..end_idx.min(items.len()))
                            .unwrap_or(&[])
                            .to_vec();
                        Some(Value::List(sliced.into()))
                    }
                    Value::String(s) => {
                        let chars: Vec<char> = s.chars().collect();
                        let end_idx = end
                            .as_ref()
                            .and_then(|e| self.eval_expr(e, chunk, row))
                            .and_then(|v| {
                                if let Value::Int64(i) = v {
                                    Some(i as usize)
                                } else {
                                    None
                                }
                            })
                            .unwrap_or(chars.len());
                        let sliced: String = chars
                            .get(start_idx..end_idx.min(chars.len()))
                            .unwrap_or(&[])
                            .iter()
                            .collect();
                        Some(Value::String(sliced.into()))
                    }
                    _ => None,
                }
            }
            FilterExpression::Case {
                operand,
                when_clauses,
                else_clause,
            } => self.eval_case(
                operand.as_deref(),
                when_clauses,
                else_clause.as_deref(),
                chunk,
                row,
            ),
            FilterExpression::Id(variable) => {
                let col_idx = *self.variable_columns.get(variable)?;
                let col = chunk.column(col_idx)?;
                // Try as node first, then as edge
                if let Some(node_id) = col.get_node_id(row) {
                    Some(Value::Int64(node_id.0 as i64))
                } else {
                    col.get_edge_id(row)
                        .map(|edge_id| Value::Int64(edge_id.0 as i64))
                }
            }
            FilterExpression::Labels(variable) => {
                let col_idx = *self.variable_columns.get(variable)?;
                let col = chunk.column(col_idx)?;
                let node_id = col.get_node_id(row)?;
                // T12 label pushdown : read labels from the store's O(1)
                // bitset path instead of `resolve_node`, which bulk-hydrates
                // every property of the node. Fast path first, epoch fallback
                // on miss — same rationale as hasLabel/Property.
                if let Some(labels) = self.store.get_node_labels(node_id) {
                    let labels: Vec<Value> =
                        labels.iter().map(|l| Value::String(l.clone())).collect();
                    return Some(Value::List(labels.into()));
                }
                if self.viewing_epoch.is_some()
                    && let Some(node) = self.resolve_node(node_id)
                {
                    let labels: Vec<Value> = node
                        .labels
                        .iter()
                        .map(|l| Value::String(l.clone()))
                        .collect();
                    return Some(Value::List(labels.into()));
                }
                None
            }
            FilterExpression::Type(variable) => {
                let col_idx = *self.variable_columns.get(variable)?;
                let col = chunk.column(col_idx)?;
                let edge_id = col.get_edge_id(row)?;
                let edge = self.resolve_edge(edge_id)?;
                Some(Value::String(edge.edge_type.clone()))
            }
            FilterExpression::ListComprehension {
                variable,
                list_expr,
                filter_expr,
                map_expr,
            } => {
                // Evaluate the source list (accept both List and Vector)
                let list_val = self.eval_expr(list_expr, chunk, row)?;
                let owned_items: Vec<Value>;
                let items: &[Value] = match &list_val {
                    Value::List(list) => list,
                    Value::Vector(vec) => {
                        owned_items = vec.iter().map(|&f| Value::Float64(f64::from(f))).collect();
                        &owned_items
                    }
                    _ => return None,
                };

                // Build the result list by iterating over source items
                let mut result = Vec::new();
                for item in items {
                    // Create a temporary context with the iteration variable bound
                    // For now, we'll do a simplified version that works for literals
                    // A full implementation would need to create a sub-evaluator

                    // Check filter predicate if present
                    let passes_filter = if let Some(filter) = filter_expr {
                        // Simplified: evaluate filter with item as context
                        // This works for simple cases like x > 5
                        matches!(
                            self.eval_comprehension_expr(filter, item, variable),
                            Some(Value::Bool(true))
                        )
                    } else {
                        true
                    };

                    if passes_filter {
                        // Apply the mapping expression
                        if let Some(mapped) = self.eval_comprehension_expr(map_expr, item, variable)
                        {
                            result.push(mapped);
                        }
                    }
                }

                Some(Value::List(result.into()))
            }
            FilterExpression::ListPredicate {
                kind,
                variable,
                list_expr,
                predicate,
            } => {
                let list_val = self.eval_expr(list_expr, chunk, row)?;
                // Accept both List and Vector as iterable sequences
                let items: Vec<&Value>;
                let vec_items: Vec<Value>;
                match &list_val {
                    Value::List(list) => {
                        items = list.iter().collect();
                    }
                    Value::Vector(vec) => {
                        vec_items = vec.iter().map(|&f| Value::Float64(f64::from(f))).collect();
                        items = vec_items.iter().collect();
                    }
                    _ => return None,
                }

                let mut match_count: u32 = 0;
                for item in &items {
                    let result = self.eval_comprehension_expr(predicate, item, variable);
                    if matches!(result, Some(Value::Bool(true))) {
                        match_count += 1;
                    }
                }

                let result = match kind {
                    ListPredicateKind::All => match_count == items.len() as u32,
                    ListPredicateKind::Any => match_count > 0,
                    ListPredicateKind::None => match_count == 0,
                    ListPredicateKind::Single => match_count == 1,
                };

                Some(Value::Bool(result))
            }
            FilterExpression::ExistsSubquery {
                start_var,
                direction,
                edge_types,
                ..
            } => {
                // Get the start node (local NodeId, or remote Map — V2)
                let col_idx = *self.variable_columns.get(start_var)?;
                let col = chunk.column(col_idx)?;
                let (src_store, start_node_id) = self.subquery_source(col, row)?;

                // Check if any matching edges exist
                let exists = src_store
                    .edges_from(start_node_id, *direction)
                    .into_iter()
                    .any(|(_, edge_id)| {
                        // Check edge type if specified
                        if !edge_types.is_empty() {
                            if let Some(actual_type) = src_store.edge_type(edge_id) {
                                edge_types
                                    .iter()
                                    .any(|t| actual_type.as_str().eq_ignore_ascii_case(t.as_str()))
                            } else {
                                false
                            }
                        } else {
                            true
                        }
                    })
                    // Aretes inter-bases (V2) : le predicat d'existence voit
                    // aussi les aretes EXTERNES qui RESOLVENT — une ref
                    // pendante reste absente (trivalue). Sans ce bras, NOT
                    // (a)-[:T]->() rendait un FAUX POSITIF (trou P7 de la
                    // verification adversariale).
                    || self.external_subquery_count(
                        &*src_store,
                        start_node_id,
                        *direction,
                        edge_types,
                        true,
                    ) > 0;

                Some(Value::Bool(exists))
            }
            FilterExpression::CountSubquery {
                start_var,
                direction,
                edge_types,
            } => {
                let col_idx = *self.variable_columns.get(start_var)?;
                let col = chunk.column(col_idx)?;
                let (src_store, start_node_id) = self.subquery_source(col, row)?;

                let count = src_store
                    .edges_from(start_node_id, *direction)
                    .into_iter()
                    .filter(|(_, edge_id)| {
                        if !edge_types.is_empty() {
                            if let Some(actual_type) = src_store.edge_type(*edge_id) {
                                edge_types
                                    .iter()
                                    .any(|t| actual_type.as_str().eq_ignore_ascii_case(t.as_str()))
                            } else {
                                false
                            }
                        } else {
                            true
                        }
                    })
                    .count()
                    // Aretes inter-bases (V2) : les aretes externes qui
                    // resolvent comptent ; une ref pendante compte zero.
                    + self.external_subquery_count(
                        &*src_store,
                        start_node_id,
                        *direction,
                        edge_types,
                        false,
                    );

                Some(Value::Int64(count as i64))
            }
            FilterExpression::Reduce {
                accumulator,
                initial,
                variable,
                list,
                expression,
            } => {
                let init_val = self.eval_expr(initial, chunk, row)?;
                let list_val = self.eval_expr(list, chunk, row)?;
                let owned_items: Vec<Value>;
                let items: &[Value] = match &list_val {
                    Value::List(list) => list,
                    Value::Vector(vec) => {
                        owned_items = vec.iter().map(|&f| Value::Float64(f64::from(f))).collect();
                        &owned_items
                    }
                    _ => return None,
                };
                let mut acc = init_val;
                for item in items {
                    acc = self.eval_reduce_expr(
                        expression,
                        &acc,
                        accumulator,
                        item,
                        variable,
                        (chunk, row),
                    )?;
                }
                Some(acc)
            }
        }
    }

    /// Source d'un sous-predicat d'existence/comptage : le noeud LOCAL
    /// (NodeId) ou DISTANT (Map `_base`/`_id`, resolue via le registre de
    /// la session — aretes inter-bases V2). `None` = inconnu, le predicat
    /// rend Null (trivalue) exactement comme l'historique.
    fn subquery_source(
        &self,
        col: &ValueVector,
        row: usize,
    ) -> Option<(Arc<dyn GraphStore>, NodeId)> {
        if let Some(id) = col.get_node_id(row) {
            return Some((Arc::clone(&self.store), id));
        }
        let reg = self.session_context.base_registry.as_ref()?;
        let Some(Value::Map(map)) = col.get_value(row) else {
            return None;
        };
        let slug = match map.get(&PropertyKey::new(crate::execution::base_registry::BASE_KEY)) {
            Some(Value::String(s)) => s.clone(),
            _ => return None,
        };
        let id = match map.get(&PropertyKey::new("_id")) {
            Some(Value::Int64(i)) => NodeId(*i as u64),
            _ => return None,
        };
        let store = reg.store_for_reader(
            self.session_context.access_context.as_deref(),
            None,
            slug.as_str(),
            "",
        )?;
        Some((store, id))
    }

    /// Bras EXTERNE des predicats d'existence/comptage (aretes
    /// inter-bases V2) : nombre d'aretes externes du bon type dont la
    /// cible RESOUT dans une base attachee. `au_plus_une` court-circuite
    /// au premier succes (EXISTS). Ref pendante, hote distant ou base
    /// absente = absent — jamais compte, jamais une erreur.
    fn external_subquery_count(
        &self,
        store: &dyn GraphStore,
        source: NodeId,
        direction: Direction,
        edge_types: &[String],
        au_plus_une: bool,
    ) -> usize {
        if matches!(direction, Direction::Incoming) {
            return 0;
        }
        let Some(reg) = &self.session_context.base_registry else {
            return 0;
        };
        let mut n = 0usize;
        for e in store.external_edges_from(source) {
            if !edge_types.is_empty()
                && !edge_types
                    .iter()
                    .any(|t| e.edge_type.eq_ignore_ascii_case(t.as_str()))
            {
                continue;
            }
            let resolue = reg
                .store_for_reader(
                    self.session_context.access_context.as_deref(),
                    e.to_host.as_deref(),
                    &e.to_store,
                    &e.to_key,
                )
                .and_then(|ts| {
                    crate::execution::base_registry::resolve_node_key(&*ts, &e.to_key)
                })
                .is_some();
            if resolue {
                n += 1;
                if au_plus_une {
                    return n;
                }
            }
        }
        n
    }

    /// Evaluates an expression in the context of a reduce() call.
    ///
    /// Both the accumulator variable and the iteration variable are bound.
    /// The `ctx` parameter provides chunk context `(chunk, row)` for resolving
    /// outer-scope variables (variables not bound by reduce).
    fn eval_reduce_expr(
        &self,
        expr: &FilterExpression,
        acc_val: &Value,
        acc_name: &str,
        item_val: &Value,
        item_name: &str,
        ctx: (&DataChunk, usize),
    ) -> Option<Value> {
        // Closure for recursive calls with all bindings
        let recurse = |e| self.eval_reduce_expr(e, acc_val, acc_name, item_val, item_name, ctx);
        match expr {
            FilterExpression::Variable(name) if name == acc_name => Some(acc_val.clone()),
            FilterExpression::Variable(name) if name == item_name => Some(item_val.clone()),
            FilterExpression::Literal(v) => Some(v.clone()),
            FilterExpression::Binary { left, op, right } => {
                // IN operator needs special handling: right side is a list
                if *op == BinaryFilterOp::In {
                    let l = recurse(left)?;
                    let r = recurse(right)?;
                    return match r {
                        Value::List(items) => {
                            if l.is_null() {
                                return Some(Value::Null);
                            }
                            let mut has_null = false;
                            for v in items.iter() {
                                if v.is_null() {
                                    has_null = true;
                                } else if Self::values_equal(&l, v) {
                                    return Some(Value::Bool(true));
                                }
                            }
                            if has_null {
                                Some(Value::Null)
                            } else {
                                Some(Value::Bool(false))
                            }
                        }
                        _ => None,
                    };
                }
                let l = recurse(left)?;
                let r = recurse(right)?;
                self.eval_binary_op(&l, *op, &r)
            }
            FilterExpression::Unary { op, operand } => {
                let val = recurse(operand);
                self.eval_unary_op(*op, val)
            }
            FilterExpression::Property {
                variable: var,
                property,
            } if var == item_name => {
                if let Value::Map(map) = item_val {
                    Some(
                        map.iter()
                            .find(|(k, _)| k.as_str() == property)
                            .map_or(Value::Null, |(_, v)| v.clone()),
                    )
                } else {
                    None
                }
            }
            FilterExpression::Property {
                variable: var,
                property,
            } if var == acc_name => {
                if let Value::Map(map) = acc_val {
                    Some(
                        map.iter()
                            .find(|(k, _)| k.as_str() == property)
                            .map_or(Value::Null, |(_, v)| v.clone()),
                    )
                } else {
                    None
                }
            }
            FilterExpression::List(items) => {
                let values: Vec<Value> = items.iter().filter_map(&recurse).collect();
                Some(Value::List(values.into()))
            }
            FilterExpression::Case {
                operand,
                when_clauses,
                else_clause,
            } => {
                if let Some(test_expr) = operand.as_deref() {
                    let test_val = recurse(test_expr)?;
                    for (when_expr, then_expr) in when_clauses {
                        let when_val = recurse(when_expr)?;
                        if Self::values_equal(&test_val, &when_val) {
                            return recurse(then_expr);
                        }
                    }
                } else {
                    for (when_expr, then_expr) in when_clauses {
                        let when_val = recurse(when_expr)?;
                        if when_val.as_bool() == Some(true) {
                            return recurse(then_expr);
                        }
                    }
                }
                if let Some(else_expr) = else_clause.as_deref() {
                    recurse(else_expr)
                } else {
                    Some(Value::Null)
                }
            }
            FilterExpression::IndexAccess { base, index } => {
                let base_val = recurse(base)?;
                let index_val = recurse(index)?;
                match (&base_val, &index_val) {
                    (Value::List(items), Value::Int64(i)) => {
                        let idx = if *i < 0 {
                            let len = items.len() as i64;
                            (len + i) as usize
                        } else {
                            *i as usize
                        };
                        items.get(idx).cloned()
                    }
                    (Value::String(s), Value::Int64(i)) => {
                        let idx = if *i < 0 {
                            let len = s.len() as i64;
                            (len + i) as usize
                        } else {
                            *i as usize
                        };
                        s.chars()
                            .nth(idx)
                            .map(|c| Value::String(c.to_string().into()))
                    }
                    (Value::Map(m), Value::String(key)) => {
                        let prop_key = PropertyKey::new(key.as_str());
                        m.get(&prop_key).cloned()
                    }
                    _ => None,
                }
            }
            // For expressions not referencing the local variables, resolve
            // from the outer scope (chunk/row)
            _ => self.eval_expr(expr, ctx.0, ctx.1),
        }
    }

    /// Evaluates an expression in the context of a list comprehension.
    /// The `item` is the current iteration value bound to `variable`.
    fn eval_comprehension_expr(
        &self,
        expr: &FilterExpression,
        item: &Value,
        variable: &str,
    ) -> Option<Value> {
        match expr {
            FilterExpression::Variable(name) if name == variable => Some(item.clone()),
            FilterExpression::Literal(v) => Some(v.clone()),
            FilterExpression::Binary { left, op, right } => {
                // IN operator needs special handling: right side is a list
                if *op == BinaryFilterOp::In {
                    let left_val = self.eval_comprehension_expr(left, item, variable)?;
                    let right_val = self.eval_comprehension_expr(right, item, variable)?;
                    return match right_val {
                        Value::List(items) => {
                            if left_val.is_null() {
                                return Some(Value::Null);
                            }
                            let mut has_null = false;
                            for v in items.iter() {
                                if v.is_null() {
                                    has_null = true;
                                } else if Self::values_equal(&left_val, v) {
                                    return Some(Value::Bool(true));
                                }
                            }
                            if has_null {
                                Some(Value::Null)
                            } else {
                                Some(Value::Bool(false))
                            }
                        }
                        _ => None,
                    };
                }
                let left_val = self.eval_comprehension_expr(left, item, variable)?;
                let right_val = self.eval_comprehension_expr(right, item, variable)?;
                self.eval_binary_op(&left_val, *op, &right_val)
            }
            FilterExpression::Unary { op, operand } => {
                let val = self.eval_comprehension_expr(operand, item, variable);
                self.eval_unary_op(*op, val)
            }
            FilterExpression::Property {
                variable: var,
                property,
            } if var == variable => {
                // Property access on the iteration variable
                if let Value::Map(m) = item {
                    let key = PropertyKey::new(property.as_str());
                    m.get(&key).cloned()
                } else {
                    None
                }
            }
            FilterExpression::List(items) => {
                let values: Vec<Value> = items
                    .iter()
                    .filter_map(|i| self.eval_comprehension_expr(i, item, variable))
                    .collect();
                Some(Value::List(values.into()))
            }
            FilterExpression::Case {
                operand,
                when_clauses,
                else_clause,
            } => self.eval_case_in_comprehension(
                operand.as_deref(),
                when_clauses,
                else_clause.as_deref(),
                item,
                variable,
            ),
            // For other expression types, return None (unsupported in comprehension)
            _ => None,
        }
    }

    /// Evaluates a CASE expression inside a list comprehension or predicate context.
    fn eval_case_in_comprehension(
        &self,
        operand: Option<&FilterExpression>,
        when_clauses: &[(FilterExpression, FilterExpression)],
        else_clause: Option<&FilterExpression>,
        item: &Value,
        variable: &str,
    ) -> Option<Value> {
        if let Some(test_expr) = operand {
            let test_val = self
                .eval_comprehension_expr(test_expr, item, variable)
                .unwrap_or(Value::Null);
            for (when_expr, then_expr) in when_clauses {
                let when_val = self
                    .eval_comprehension_expr(when_expr, item, variable)
                    .unwrap_or(Value::Null);
                if !test_val.is_null()
                    && !when_val.is_null()
                    && Self::values_equal(&test_val, &when_val)
                {
                    return self.eval_comprehension_expr(then_expr, item, variable);
                }
            }
        } else {
            for (when_expr, then_expr) in when_clauses {
                let when_val = self.eval_comprehension_expr(when_expr, item, variable)?;
                if when_val.as_bool() == Some(true) {
                    return self.eval_comprehension_expr(then_expr, item, variable);
                }
            }
        }
        if let Some(else_expr) = else_clause {
            self.eval_comprehension_expr(else_expr, item, variable)
        } else {
            Some(Value::Null)
        }
    }

    /// Evaluates a binary operator with ISO three-valued logic.
    ///
    /// NULL propagation: `NULL = x`, `x = NULL`, `NULL <> x` all yield
    /// `Value::Null` (UNKNOWN), not `Value::Bool`. AND/OR/XOR follow the
    /// standard truth tables where FALSE AND UNKNOWN = FALSE, etc.
    ///
    /// For structural equality (DISTINCT, GROUP BY), use [`values_equal`]
    /// directly, which treats NULL == NULL as true.
    fn eval_binary_op(&self, left: &Value, op: BinaryFilterOp, right: &Value) -> Option<Value> {
        match op {
            // Three-valued logic for AND/OR/XOR (ISO/IEC 39075 Section 21)
            BinaryFilterOp::And => match (left.as_bool(), right.as_bool()) {
                (Some(false), _) | (_, Some(false)) => Some(Value::Bool(false)),
                (Some(true), Some(true)) => Some(Value::Bool(true)),
                _ => Some(Value::Null), // UNKNOWN
            },
            BinaryFilterOp::Or => match (left.as_bool(), right.as_bool()) {
                (Some(true), _) | (_, Some(true)) => Some(Value::Bool(true)),
                (Some(false), Some(false)) => Some(Value::Bool(false)),
                _ => Some(Value::Null), // UNKNOWN
            },
            BinaryFilterOp::Xor => match (left.as_bool(), right.as_bool()) {
                (Some(l), Some(r)) => Some(Value::Bool(l ^ r)),
                _ => Some(Value::Null), // UNKNOWN
            },
            // COMPARATEUR CANONIQUE (correctif E) : `=`/`<>` et les quatre
            // opérateurs d'ordre délèguent à obrain_common::types::value_cmp —
            // la MÊME sémantique que le balayage par intervalle et le tri.
            // NULL ou classes disjointes (chaîne vs nombre) → UNKNOWN ;
            // NaN → tout FAUX sauf `<>` (IEEE) ; flottants stricts, sans
            // EPSILON ; Int64↔Float64 exact ; branches temporelles incluses.
            BinaryFilterOp::Eq => Some(match value_cmp::egaler(left, right) {
                Some(b) => Value::Bool(b),
                None => Value::Null,
            }),
            BinaryFilterOp::Ne => Some(match value_cmp::egaler(left, right) {
                Some(b) => Value::Bool(!b),
                None => Value::Null,
            }),
            BinaryFilterOp::Lt => Some(Self::verdict_ordre(left, right, Ordering::is_lt)),
            BinaryFilterOp::Le => Some(Self::verdict_ordre(left, right, Ordering::is_le)),
            BinaryFilterOp::Gt => Some(Self::verdict_ordre(left, right, Ordering::is_gt)),
            BinaryFilterOp::Ge => Some(Self::verdict_ordre(left, right, Ordering::is_ge)),
            // Arithmetic operators
            BinaryFilterOp::Add => {
                // String concatenation: string + string, or string + other
                match (left, right) {
                    (Value::String(a), Value::String(b)) => {
                        let mut s = String::with_capacity(a.len() + b.len());
                        s.push_str(a);
                        s.push_str(b);
                        Some(Value::String(s.into()))
                    }
                    (Value::String(a), other) => {
                        let b = match other {
                            Value::Int64(i) => i.to_string(),
                            Value::Float64(f) => f.to_string(),
                            Value::Bool(b) => b.to_string(),
                            Value::Null => return Some(Value::Null),
                            _ => return None,
                        };
                        let mut s = String::with_capacity(a.len() + b.len());
                        s.push_str(a);
                        s.push_str(&b);
                        Some(Value::String(s.into()))
                    }
                    // Temporal addition
                    (Value::Date(d), Value::Duration(dur))
                    | (Value::Duration(dur), Value::Date(d)) => {
                        Some(Value::Date(d.add_duration(dur)))
                    }
                    (Value::Time(t), Value::Duration(dur))
                    | (Value::Duration(dur), Value::Time(t)) => {
                        Some(Value::Time(t.add_duration(dur)))
                    }
                    (Value::Timestamp(ts), Value::Duration(dur))
                    | (Value::Duration(dur), Value::Timestamp(ts)) => {
                        Some(Value::Timestamp(ts.add_duration(dur)))
                    }
                    (Value::Duration(a), Value::Duration(b)) => Some(Value::Duration(a.add(*b))),
                    _ => self.eval_arithmetic(left, right, ArithGardee::Add),
                }
            }
            BinaryFilterOp::Sub => match (left, right) {
                // Temporal subtraction
                (Value::Date(a), Value::Duration(dur)) => Some(Value::Date(a.sub_duration(dur))),
                (Value::Time(a), Value::Duration(dur)) => {
                    Some(Value::Time(a.add_duration(&dur.neg())))
                }
                (Value::Timestamp(a), Value::Duration(dur)) => {
                    Some(Value::Timestamp(a.add_duration(&dur.neg())))
                }
                (Value::Date(a), Value::Date(b)) => {
                    let days = a.as_days() as i64 - b.as_days() as i64;
                    Some(Value::Duration(obrain_common::types::Duration::from_days(
                        days,
                    )))
                }
                (Value::Time(a), Value::Time(b)) => {
                    let nanos = a.as_nanos() as i64 - b.as_nanos() as i64;
                    Some(Value::Duration(obrain_common::types::Duration::from_nanos(
                        nanos,
                    )))
                }
                (Value::Timestamp(a), Value::Timestamp(b)) => {
                    let micros = a.duration_since(*b);
                    Some(Value::Duration(obrain_common::types::Duration::from_nanos(
                        micros * 1000,
                    )))
                }
                (Value::Duration(a), Value::Duration(b)) => Some(Value::Duration(a.sub(*b))),
                _ => self.eval_arithmetic(left, right, ArithGardee::Sub),
            },
            BinaryFilterOp::Mul => match (left, right) {
                (Value::Duration(d), Value::Int64(n)) | (Value::Int64(n), Value::Duration(d)) => {
                    Some(Value::Duration(d.mul(*n)))
                }
                _ => self.eval_arithmetic(left, right, ArithGardee::Mul),
            },
            BinaryFilterOp::Div => match (left, right) {
                (Value::Duration(d), Value::Int64(n)) if *n != 0 => {
                    Some(Value::Duration(d.div(*n)))
                }
                _ => self.eval_arithmetic(left, right, ArithGardee::Div),
            },
            BinaryFilterOp::Mod => self.eval_modulo(left, right),
            // String operators
            BinaryFilterOp::StartsWith => {
                let l = left.as_str()?;
                let r = right.as_str()?;
                Some(Value::Bool(l.starts_with(r)))
            }
            BinaryFilterOp::EndsWith => {
                let l = left.as_str()?;
                let r = right.as_str()?;
                Some(Value::Bool(l.ends_with(r)))
            }
            // ── `CONTAINS` SUR UNE LISTE EST UNE APPARTENANCE, PAS UNE SOUS-CHAÎNE ─
            //
            // Défaut mesuré le 2026-09-13, depuis le chat du dashboard : le modèle
            // a écrit `MATCH (n) WHERE n.qualified_name STARTS WITH '…' AND
            // labels(n) CONTAINS 'Class' RETURN …` — le moteur a rendu **0 ligne en
            // 147 ms** alors que 15 004 nœuds portent ce label.
            //
            // La cause est ici : `labels(n)` est une LISTE (`Value::List`), donc
            // `as_str()` rendait `None`, `eval_binary_op` rendait `None`, et la
            // ligne était **silencieusement rejetée**. Un faux négatif se lit comme
            // une réponse (« aucun nœud de ce type »), et il a coûté sept requêtes
            // supplémentaires au modèle (181 s de conversation mesurées).
            //
            // Une liste ne contient pas une SOUS-CHAÎNE : elle contient un ÉLÉMENT.
            // On évalue donc l'appartenance, avec l'égalité structurelle du moteur
            // (`values_equal`) — la même que celle d'`IN`, qui rendait déjà les
            // 15 004 lignes attendues.
            BinaryFilterOp::Contains => match (left, right) {
                (Value::String(l), Value::String(r)) => {
                    Some(Value::Bool(l.as_str().contains(r.as_str())))
                }
                (Value::List(items), r) => Some(Value::Bool(
                    items.iter().any(|v| Self::values_equal(v, r)),
                )),
                // Classes disjointes : `None` (UNKNOWN) plutôt qu'un FAUX, pour que
                // la logique trivaluée en tienne compte au lieu de rejeter la ligne
                // en silence — c'est exactement le défaut corrigé ci-dessus.
                _ => None,
            },
            // IN is handled separately
            BinaryFilterOp::In => None,
            // Regex match (=~)
            BinaryFilterOp::Regex => {
                #[cfg(any(feature = "regex", feature = "regex-lite"))]
                match (left, right) {
                    (Value::String(s), Value::String(pattern)) => match Regex::new(pattern) {
                        Ok(re) => Some(Value::Bool(re.is_match(s))),
                        Err(_) => None,
                    },
                    _ => None,
                }
                #[cfg(not(any(feature = "regex", feature = "regex-lite")))]
                {
                    let _ = (left, right);
                    None
                }
            }
            // Power/exponentiation (^)
            BinaryFilterOp::Pow => {
                match (left, right) {
                    (Value::Int64(base), Value::Int64(exp)) => {
                        Some(Value::Float64((*base as f64).powf(*exp as f64)))
                    }
                    (Value::Float64(base), Value::Float64(exp)) => {
                        Some(Value::Float64(base.powf(*exp)))
                    }
                    (Value::Int64(base), Value::Float64(exp)) => {
                        Some(Value::Float64((*base as f64).powf(*exp)))
                    }
                    (Value::Float64(base), Value::Int64(exp)) => {
                        Some(Value::Float64(base.powf(*exp as f64)))
                    }
                    _ => None, // Type mismatch
                }
            }
            // SQL LIKE pattern matching
            BinaryFilterOp::Like => {
                #[cfg(any(feature = "regex", feature = "regex-lite"))]
                match (left, right) {
                    (Value::String(s), Value::String(pattern)) => {
                        let mut regex_pattern = String::with_capacity(pattern.len() + 4);
                        regex_pattern.push('^');
                        let mut chars = pattern.chars().peekable();
                        while let Some(ch) = chars.next() {
                            match ch {
                                '%' => regex_pattern.push_str(".*"),
                                '_' => regex_pattern.push('.'),
                                '\\' => {
                                    if let Some(next) = chars.next() {
                                        regex_escape_char(next, &mut regex_pattern);
                                    }
                                }
                                _ => regex_escape_char(ch, &mut regex_pattern),
                            }
                        }
                        regex_pattern.push('$');
                        match Regex::new(&regex_pattern) {
                            Ok(re) => Some(Value::Bool(re.is_match(s))),
                            Err(_) => None,
                        }
                    }
                    (Value::Null, _) | (_, Value::Null) => Some(Value::Null),
                    _ => None,
                }
                #[cfg(not(any(feature = "regex", feature = "regex-lite")))]
                {
                    let _ = (left, right);
                    None
                }
            }
            // String concatenation (||)
            BinaryFilterOp::Concat => match (left, right) {
                (Value::String(a), Value::String(b)) => {
                    let mut s = String::with_capacity(a.len() + b.len());
                    s.push_str(a);
                    s.push_str(b);
                    Some(Value::String(s.into()))
                }
                (Value::String(a), other) => {
                    let b = value_to_string(other)?;
                    let mut s = String::with_capacity(a.len() + b.len());
                    s.push_str(a);
                    s.push_str(&b);
                    Some(Value::String(s.into()))
                }
                (other, Value::String(b)) => {
                    let a = value_to_string(other)?;
                    let mut s = String::with_capacity(a.len() + b.len());
                    s.push_str(&a);
                    s.push_str(b);
                    Some(Value::String(s.into()))
                }
                (Value::Null, _) | (_, Value::Null) => Some(Value::Null),
                _ => None,
            },
        }
    }

    /// Arithmétique numérique GARDÉE du chemin vif des requêtes (WHERE et
    /// RETURN partagent cet évaluateur).
    ///
    /// MIROIR de `crates/obrain-engine/src/catalog/check_eval.rs` (fn
    /// `eval_arithmetic`, chemin des contraintes CHECK) : mêmes gardes
    /// `checked_add`/`checked_sub`/`checked_mul`/`checked_div` et même refus
    /// explicite de la division par zéro. Le partage direct du code est
    /// impossible sans dépendance inverse — `obrain-engine` dépend
    /// d'`obrain-core`, pas le contraire — la sémantique est donc DUPLIQUÉE
    /// ici ; toute évolution doit être répercutée des deux côtés.
    ///
    /// Sur `i64`, une faute (division par zéro, débordement) REFUSE la
    /// requête via [`Self::signale_erreur`] : jamais de panique, jamais
    /// d'enroulement — le comportement est identique en debug et en release
    /// puisqu'il ne dépend plus d'`overflow-checks`. Sur `f64`, IEEE-754
    /// fait loi (`1.0 / 0.0` vaut `+∞`), inchangé.
    fn eval_arithmetic(&self, left: &Value, right: &Value, op: ArithGardee) -> Option<Value> {
        match (left, right) {
            (Value::Int64(a), Value::Int64(b)) => {
                let r = match op {
                    ArithGardee::Add => a.checked_add(*b).ok_or("integer overflow"),
                    ArithGardee::Sub => a.checked_sub(*b).ok_or("integer underflow"),
                    ArithGardee::Mul => a.checked_mul(*b).ok_or("integer overflow"),
                    ArithGardee::Div => {
                        if *b == 0 {
                            Err("division by zero")
                        } else {
                            // Seul quotient d'entiers qui déborde :
                            // i64::MIN / -1.
                            a.checked_div(*b).ok_or("integer overflow")
                        }
                    }
                };
                match r {
                    Ok(v) => Some(Value::Int64(v)),
                    Err(e) => {
                        self.signale_erreur(format!("{e}: {a} {} {b}", op.symbole()))
                    }
                }
            }
            (Value::Float64(a), Value::Float64(b)) => Some(Value::Float64(op.flottant(*a, *b))),
            (Value::Int64(a), Value::Float64(b)) => {
                Some(Value::Float64(op.flottant(*a as f64, *b)))
            }
            (Value::Float64(a), Value::Int64(b)) => {
                Some(Value::Float64(op.flottant(*a, *b as f64)))
            }
            _ => None,
        }
    }

    /// Modulo gardé — même miroir de `check_eval.rs` que [`Self::eval_arithmetic`].
    ///
    /// Le modulo ENTIER par zéro refuse la requête (avant ce commit il
    /// rendait un NULL silencieux, indiscernable d'une propriété absente),
    /// et `i64::MIN % -1` — le seul reste qui déborde — est gardé par
    /// `checked_rem`. Les chemins FLOTTANTS conservent leur sémantique
    /// existante (diviseur nul → NULL) : l'arithmétique IEEE est hors du
    /// périmètre de la campagne arithmétique gardée (cf. lot 02).
    fn eval_modulo(&self, left: &Value, right: &Value) -> Option<Value> {
        match (left, right) {
            (Value::Int64(a), Value::Int64(b)) => {
                if *b == 0 {
                    return self.signale_erreur(format!("modulo by zero: {a} % {b}"));
                }
                match a.checked_rem(*b) {
                    Some(v) => Some(Value::Int64(v)),
                    None => self.signale_erreur(format!("integer overflow: {a} % {b}")),
                }
            }
            (Value::Float64(a), Value::Float64(b)) if *b != 0.0 => Some(Value::Float64(a % b)),
            (Value::Int64(a), Value::Float64(b)) if *b != 0.0 => {
                Some(Value::Float64(*a as f64 % b))
            }
            (Value::Float64(a), Value::Int64(b)) if *b != 0 => Some(Value::Float64(a % *b as f64)),
            _ => None,
        }
    }

    /// Evaluates `left IN right` with three-valued NULL semantics.
    ///
    /// - `NULL IN [...]` yields UNKNOWN.
    /// - If no element matches but the list contains NULLs, yields UNKNOWN.
    /// - Otherwise yields `true` on first match, `false` if none match.
    fn eval_in_operator(
        &self,
        left: &Value,
        right: &FilterExpression,
        chunk: &DataChunk,
        row: usize,
    ) -> Option<Value> {
        // ── T17i : ne pas rematérialiser la liste à chaque ligne ──
        //
        // `MATCH (n:L) WHERE n.id IN [ …40 ids… ]` évaluait `right` par
        // ligne : `FilterExpression::List` construit un `Value::List` neuf,
        // ce qui clone les 40 chaînes. Sur un scan de 31 357 nœuds, c'est
        // 1,25 million d'allocations de chaînes pour 1,25 million de
        // comparaisons — l'allocation domine largement la comparaison.
        //
        // Quand le côté droit est syntaxiquement une liste, on itère
        // dessus directement et on court-circuite au premier match. La
        // sémantique est identique, y compris la logique trivaluée et le
        // fait qu'un élément qui ne s'évalue pas est ignoré (le chemin
        // générique le laissait tomber via `filter_map`).
        if let FilterExpression::List(items) = right {
            if left.is_null() {
                return Some(Value::Null);
            }
            let mut has_null = false;
            for item in items {
                let Some(v) = self.eval_expr(item, chunk, row) else {
                    continue;
                };
                if v.is_null() {
                    has_null = true;
                } else if Self::values_equal(left, &v) {
                    return Some(Value::Bool(true));
                }
            }
            return Some(if has_null {
                Value::Null
            } else {
                Value::Bool(false)
            });
        }

        let right_val = self.eval_expr(right, chunk, row)?;
        match right_val {
            Value::List(items) => {
                // Three-valued IN: NULL IN (...) is UNKNOWN
                if left.is_null() {
                    return Some(Value::Null);
                }
                let mut has_null = false;
                for item in items.iter() {
                    if item.is_null() {
                        has_null = true;
                    } else if Self::values_equal(left, item) {
                        return Some(Value::Bool(true));
                    }
                }
                if has_null {
                    Some(Value::Null) // no match but NULLs present: UNKNOWN
                } else {
                    Some(Value::Bool(false))
                }
            }
            _ => None,
        }
    }

    fn eval_function(
        &self,
        name: &str,
        args: &[FilterExpression],
        chunk: &DataChunk,
        row: usize,
    ) -> Option<Value> {
        match name.to_lowercase().as_str() {
            "id" => {
                if args.len() != 1 {
                    return None;
                }
                if let FilterExpression::Variable(var) = &args[0] {
                    let col_idx = *self.variable_columns.get(var)?;
                    let col = chunk.column(col_idx)?;
                    if let Some(node_id) = col.get_node_id(row) {
                        return Some(Value::Int64(node_id.0 as i64));
                    } else if let Some(edge_id) = col.get_edge_id(row) {
                        return Some(Value::Int64(edge_id.0 as i64));
                    }
                }
                None
            }
            "element_id" | "elementid" => {
                if args.len() != 1 {
                    return None;
                }
                if let FilterExpression::Variable(var) = &args[0] {
                    let col_idx = *self.variable_columns.get(var)?;
                    let col = chunk.column(col_idx)?;
                    // Resolve ambiguity between node/edge by verifying against the
                    // store. VectorData::Generic stores raw Int64 values that both
                    // get_node_id and get_edge_id accept, so we must check which
                    // entity actually exists.
                    if let Some(edge_id) = col.get_edge_id(row)
                        && self.resolve_edge(edge_id).is_some()
                    {
                        return Some(Value::String(format!("e:{}", edge_id.0).into()));
                    }
                    if let Some(node_id) = col.get_node_id(row)
                        && self.resolve_node(node_id).is_some()
                    {
                        return Some(Value::String(format!("n:{}", node_id.0).into()));
                    }
                }
                None
            }
            "base" => {
                // Aretes inter-bases (V2) : slug de la base du noeud.
                // - noeud local -> slug primaire du registre (Null si la
                //   session n'a pas de registre : la notion n'existe pas) ;
                // - noeud distant (Value::Map hydratee a la frontiere) ->
                //   sa cle `_base` ;
                // - absent/inconnu -> Null (trivalue, jamais une erreur).
                if args.len() != 1 {
                    return None;
                }
                if let FilterExpression::Variable(var) = &args[0] {
                    let col_idx = *self.variable_columns.get(var)?;
                    let col = chunk.column(col_idx)?;
                    if let Some(Value::Map(map)) = col.get_value(row)
                        && let Some(Value::String(slug)) =
                            map.get(&PropertyKey::new(crate::execution::base_registry::BASE_KEY))
                    {
                        return Some(Value::String(slug.clone()));
                    }
                    if col.get_node_id(row).is_some() {
                        if let Some(reg) = &self.session_context.base_registry {
                            return Some(Value::String(reg.primary_slug().into()));
                        }
                        return Some(Value::Null);
                    }
                }
                Some(Value::Null)
            }
            "labels" => {
                if args.len() != 1 {
                    return None;
                }
                if let FilterExpression::Variable(var) = &args[0] {
                    let col_idx = *self.variable_columns.get(var)?;
                    let col = chunk.column(col_idx)?;
                    // Aretes inter-bases (V2) : noeud distant -> `_labels`.
                    if let Some(Value::Map(map)) = col.get_value(row)
                        && let Some(l @ Value::List(_)) =
                            map.get(&PropertyKey::new("_labels"))
                    {
                        return Some(l.clone());
                    }
                    let node_id = col.get_node_id(row)?;
                    // Bitset path, no property hydration (T12) — epoch
                    // fallback on miss, same pattern as hasLabel.
                    if let Some(labels) = self.store.get_node_labels(node_id) {
                        let labels: Vec<Value> =
                            labels.iter().map(|l| Value::String(l.clone())).collect();
                        return Some(Value::List(labels.into()));
                    }
                    if self.viewing_epoch.is_some()
                        && let Some(node) = self.resolve_node(node_id)
                    {
                        let labels: Vec<Value> = node
                            .labels
                            .iter()
                            .map(|l| Value::String(l.clone()))
                            .collect();
                        return Some(Value::List(labels.into()));
                    }
                }
                None
            }
            "type" => {
                if args.len() != 1 {
                    return None;
                }
                if let FilterExpression::Variable(var) = &args[0] {
                    let col_idx = *self.variable_columns.get(var)?;
                    let col = chunk.column(col_idx)?;
                    let edge_id = col.get_edge_id(row)?;
                    let edge = self.resolve_edge(edge_id)?;
                    return Some(Value::String(edge.edge_type.clone()));
                }
                None
            }
            "size" | "length" | "cardinality" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::List(items) => Some(Value::Int64(items.len() as i64)),
                    // Des CARACTÈRES, pas des octets (gql_05) : size(s) doit
                    // compter ce que left(s, size(s)) rend — left/substring
                    // découpent en chars(). octet_length reste la porte des
                    // octets.
                    Value::String(s) => Some(Value::Int64(s.chars().count() as i64)),
                    Value::Path { edges, .. } => Some(Value::Int64(edges.len() as i64)),
                    _ => None,
                }
            }
            "coalesce" => {
                for arg in args {
                    if let Some(val) = self.eval_expr(arg, chunk, row)
                        && !matches!(val, Value::Null)
                    {
                        return Some(val);
                    }
                }
                Some(Value::Null)
            }
            "exists" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row);
                Some(Value::Bool(
                    val.is_some() && !matches!(val, Some(Value::Null)),
                ))
            }
            "tostring" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                let s = match &val {
                    Value::String(s) => s.to_string(),
                    Value::Int64(i) => i.to_string(),
                    Value::Float64(f) => f.to_string(),
                    Value::Bool(b) => b.to_string(),
                    Value::Null => return Some(Value::Null),
                    _ => format!("{val:?}"),
                };
                Some(Value::String(s.into()))
            }
            "tointeger" | "toint" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Int64(i) => Some(Value::Int64(i)),
                    Value::Float64(f) => Some(Value::Int64(f as i64)),
                    Value::String(s) => s.parse::<i64>().ok().map(Value::Int64),
                    _ => None,
                }
            }
            "tofloat" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Int64(i) => Some(Value::Float64(i as f64)),
                    Value::Float64(f) => Some(Value::Float64(f)),
                    Value::String(s) => s.parse::<f64>().ok().map(Value::Float64),
                    _ => None,
                }
            }
            "toboolean" | "tobool" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Bool(b) => Some(Value::Bool(b)),
                    Value::String(s) => match s.to_lowercase().as_str() {
                        "true" => Some(Value::Bool(true)),
                        "false" => Some(Value::Bool(false)),
                        _ => None,
                    },
                    _ => None,
                }
            }
            "haslabel" => {
                // hasLabel(node, label) - checks if a node has a specific label
                if args.len() != 2 {
                    return None;
                }
                // First arg is the node variable
                let node_id = if let FilterExpression::Variable(var) = &args[0] {
                    let col_idx = *self.variable_columns.get(var)?;
                    let col = chunk.column(col_idx)?;
                    match col.get_node_id(row) {
                        Some(id) => id,
                        None => {
                            // Aretes inter-bases (V2) : noeud DISTANT
                            // materialise en Map — ses labels voyagent dans
                            // `_labels`, son store n'est pas ici.
                            if let Some(Value::Map(map)) = col.get_value(row)
                                && let Some(Value::List(labels)) =
                                    map.get(&PropertyKey::new("_labels"))
                            {
                                let Value::String(label) =
                                    self.eval_expr(&args[1], chunk, row)?
                                else {
                                    return None;
                                };
                                return Some(Value::Bool(labels.iter().any(|l| {
                                    matches!(l, Value::String(s) if s.as_str() == label.as_str())
                                })));
                            }
                            return None;
                        }
                    }
                } else {
                    return None;
                };
                // Second arg is the label to check
                let Value::String(label) = self.eval_expr(&args[1], chunk, row)? else {
                    return None;
                };
                // Check if the node has this label.
                //
                // T12 label pushdown : `hasLabel()` is what label predicates
                // (`WHERE s:Function OR s:Struct …`) compile to. Resolving
                // the FULL node here bulk-hydrates every property (85 on PO
                // File nodes) to test one bit — measured 21.7 s on
                // list_project_symbols. Route through the store's O(1)
                // label read instead; keep the versioned resolve for MVCC.
                // Fast path FIRST, regardless of epoch context — mirrors the
                // `Property` access pattern above (T17k): sessions always set
                // viewing_epoch = Some(EpochId(0)) as the read-only baseline,
                // so gating the fast path on `viewing_epoch.is_none()` means
                // it never fires in a real session and every hasLabel
                // bulk-hydrates the node. Fall back to the versioned resolve
                // only when the scalar path misses (historical reads).
                if let Some(b) = self.store.node_has_label(node_id, label.as_str()) {
                    return Some(Value::Bool(b));
                }
                if self.viewing_epoch.is_some()
                    && let Some(node) = self.resolve_node(node_id)
                {
                    let has_label = node.labels.iter().any(|l| l.as_str() == label.as_str());
                    return Some(Value::Bool(has_label));
                }
                None
            }
            "istyped" => {
                // isTyped(value, type_name) - checks if a value has a specific GQL type
                if args.len() != 2 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                let Value::String(type_name) = self.eval_expr(&args[1], chunk, row)? else {
                    return None;
                };
                let matches = match type_name.to_uppercase().as_str() {
                    "BOOLEAN" | "BOOL" => matches!(val, Value::Bool(_)),
                    "INTEGER" | "INT" | "INT64" => matches!(val, Value::Int64(_)),
                    "FLOAT" | "FLOAT64" | "DOUBLE" => matches!(val, Value::Float64(_)),
                    "STRING" => matches!(val, Value::String(_)),
                    "LIST" => matches!(val, Value::List(_)),
                    "MAP" | "RECORD" => matches!(val, Value::Map(_)),
                    "NULL" => matches!(val, Value::Null),
                    "DATE" => matches!(val, Value::Date(_)),
                    "TIME" => matches!(val, Value::Time(_)),
                    "DATETIME" | "TIMESTAMP" => matches!(val, Value::Timestamp(_)),
                    "DURATION" => matches!(val, Value::Duration(_)),
                    "PATH" => matches!(val, Value::Path { .. }),
                    "NODE" | "EDGE" | "GRAPH" => false,
                    s if s.starts_with("LIST<") && s.ends_with('>') => {
                        let elem_type = &s[5..s.len() - 1];
                        match &val {
                            Value::List(items) => {
                                items.iter().all(|v| Self::value_matches_type(v, elem_type))
                            }
                            _ => false,
                        }
                    }
                    _ => false,
                };
                Some(Value::Bool(matches))
            }
            "isdirected" => {
                // isDirected(edge) - checks if an edge is directed (always true in LPG)
                if args.len() != 1 {
                    return None;
                }
                // In LPG, all edges are directed
                if let FilterExpression::Variable(var) = &args[0] {
                    let col_idx = *self.variable_columns.get(var)?;
                    let col = chunk.column(col_idx)?;
                    // If the column contains an edge ID, it's directed
                    if col.get_edge_id(row).is_some() {
                        return Some(Value::Bool(true));
                    }
                }
                Some(Value::Bool(false))
            }
            "issource" => {
                // isSource(node, edge) - checks if node is the source of edge
                if args.len() != 2 {
                    return None;
                }
                let node_id = if let FilterExpression::Variable(var) = &args[0] {
                    let col_idx = *self.variable_columns.get(var)?;
                    let col = chunk.column(col_idx)?;
                    col.get_node_id(row)?
                } else {
                    return None;
                };
                let edge_id = if let FilterExpression::Variable(var) = &args[1] {
                    let col_idx = *self.variable_columns.get(var)?;
                    let col = chunk.column(col_idx)?;
                    col.get_edge_id(row)?
                } else {
                    return None;
                };
                let edge = self.resolve_edge(edge_id)?;
                Some(Value::Bool(edge.src == node_id))
            }
            "isdestination" => {
                // isDestination(node, edge) - checks if node is the destination of edge
                if args.len() != 2 {
                    return None;
                }
                let node_id = if let FilterExpression::Variable(var) = &args[0] {
                    let col_idx = *self.variable_columns.get(var)?;
                    let col = chunk.column(col_idx)?;
                    col.get_node_id(row)?
                } else {
                    return None;
                };
                let edge_id = if let FilterExpression::Variable(var) = &args[1] {
                    let col_idx = *self.variable_columns.get(var)?;
                    let col = chunk.column(col_idx)?;
                    col.get_edge_id(row)?
                } else {
                    return None;
                };
                let edge = self.resolve_edge(edge_id)?;
                Some(Value::Bool(edge.dst == node_id))
            }
            "all_different" => {
                // ALL_DIFFERENT - ISO GQL predicate (G113)
                // Two calling conventions:
                //   all_different(list)          - check list elements are distinct
                //   ALL_DIFFERENT(var1, var2, ..) - check graph elements are distinct
                if args.is_empty() {
                    return Some(Value::Bool(true));
                }
                if args.len() == 1 {
                    // Single-argument: treat as list check
                    let val = self.eval_expr(&args[0], chunk, row)?;
                    return match val {
                        Value::List(items) => {
                            let mut seen = std::collections::HashSet::new();
                            let all_diff = items.iter().all(|item| {
                                let key = format!("{item:?}");
                                seen.insert(key)
                            });
                            Some(Value::Bool(all_diff))
                        }
                        _ => Some(Value::Bool(true)),
                    };
                }
                // Multi-argument: compare element IDs
                let mut ids: Vec<u64> = Vec::with_capacity(args.len());
                for arg in args {
                    if let FilterExpression::Variable(var) = arg {
                        let col_idx = *self.variable_columns.get(var)?;
                        let col = chunk.column(col_idx)?;
                        if let Some(nid) = col.get_node_id(row) {
                            ids.push(nid.0);
                        } else if let Some(eid) = col.get_edge_id(row) {
                            ids.push(eid.0);
                        } else {
                            return None;
                        }
                    } else {
                        return None;
                    }
                }
                let length = ids.len();
                ids.sort_unstable();
                ids.dedup();
                Some(Value::Bool(ids.len() == length))
            }
            "same" => {
                // SAME - ISO GQL predicate (G114)
                // Two calling conventions:
                //   same(list)          - check list elements are equal
                //   SAME(var1, var2, ..) - check graph elements are identical
                if args.is_empty() {
                    return Some(Value::Bool(true));
                }
                if args.len() == 1 {
                    // Single-argument: treat as list check
                    let val = self.eval_expr(&args[0], chunk, row)?;
                    return match val {
                        Value::List(items) => {
                            let all_same = if items.is_empty() {
                                true
                            } else {
                                items.iter().all(|item| item == &items[0])
                            };
                            Some(Value::Bool(all_same))
                        }
                        _ => Some(Value::Bool(true)),
                    };
                }
                // Multi-argument: compare element IDs
                let mut first_id: Option<u64> = None;
                for arg in args {
                    if let FilterExpression::Variable(var) = arg {
                        let col_idx = *self.variable_columns.get(var)?;
                        let col = chunk.column(col_idx)?;
                        let current_id = if let Some(nid) = col.get_node_id(row) {
                            nid.0
                        } else if let Some(eid) = col.get_edge_id(row) {
                            eid.0
                        } else {
                            return None;
                        };
                        match first_id {
                            None => first_id = Some(current_id),
                            Some(fid) if fid != current_id => return Some(Value::Bool(false)),
                            _ => {}
                        }
                    } else {
                        return None;
                    }
                }
                Some(Value::Bool(true))
            }
            "property_exists" => {
                // property_exists(entity, key) - checks if a property key exists on an entity
                if args.len() != 2 {
                    return None;
                }
                let Value::String(key) = self.eval_expr(&args[1], chunk, row)? else {
                    return None;
                };
                // Try node first, then edge
                if let FilterExpression::Variable(var) = &args[0] {
                    let col_idx = *self.variable_columns.get(var)?;
                    let col = chunk.column(col_idx)?;
                    if let Some(nid) = col.get_node_id(row)
                        && let Some(node) = self.resolve_node(nid)
                    {
                        let exists = node
                            .properties
                            .iter()
                            .any(|(k, _)| k.as_str() == key.as_str());
                        return Some(Value::Bool(exists));
                    }
                    if let Some(eid) = col.get_edge_id(row)
                        && let Some(edge) = self.resolve_edge(eid)
                    {
                        let exists = edge
                            .properties
                            .iter()
                            .any(|(k, _)| k.as_str() == key.as_str());
                        return Some(Value::Bool(exists));
                    }
                }
                Some(Value::Bool(false))
            }
            "head" => {
                // head(list) - returns the first element of a list
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::List(items) => items.first().cloned(),
                    _ => None,
                }
            }
            "tail" => {
                // tail(list) - returns all elements except the first
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::List(items) => {
                        if items.is_empty() {
                            Some(Value::List(vec![].into()))
                        } else {
                            Some(Value::List(items[1..].to_vec().into()))
                        }
                    }
                    _ => None,
                }
            }
            "last" => {
                // last(list) - returns the last element of a list
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::List(items) => items.last().cloned(),
                    _ => None,
                }
            }
            "reverse" => {
                // reverse(list) - returns the list in reverse order
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::List(items) => {
                        let reversed: Vec<Value> = items.iter().rev().cloned().collect();
                        Some(Value::List(reversed.into()))
                    }
                    Value::String(s) => {
                        let reversed: String = s.chars().rev().collect();
                        Some(Value::String(reversed.into()))
                    }
                    _ => None,
                }
            }
            // vector(list) - converts a list of numbers to a Vector
            "vector" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::List(items) => {
                        let floats: Vec<f32> = items
                            .iter()
                            .filter_map(|v| match v {
                                Value::Float64(f) => Some(*f as f32),
                                Value::Int64(i) => Some(*i as f32),
                                _ => None,
                            })
                            .collect();
                        if floats.len() == items.len() {
                            Some(Value::Vector(floats.into()))
                        } else {
                            None
                        }
                    }
                    Value::Vector(v) => Some(Value::Vector(v)),
                    _ => None,
                }
            }
            // Vector distance / similarity functions (SIMD-accelerated)
            "cosine_similarity" => {
                if args.len() != 2 {
                    return None;
                }
                let a_val = self.eval_expr(&args[0], chunk, row)?;
                let b_val = self.eval_expr(&args[1], chunk, row)?;
                let a = a_val.as_vector()?;
                let b = b_val.as_vector()?;
                if a.len() != b.len() {
                    return None;
                }
                Some(Value::Float64(
                    crate::index::vector::cosine_similarity(a, b) as f64,
                ))
            }
            "euclidean_distance" => {
                if args.len() != 2 {
                    return None;
                }
                let a_val = self.eval_expr(&args[0], chunk, row)?;
                let b_val = self.eval_expr(&args[1], chunk, row)?;
                let a = a_val.as_vector()?;
                let b = b_val.as_vector()?;
                if a.len() != b.len() {
                    return None;
                }
                Some(Value::Float64(
                    crate::index::vector::euclidean_distance(a, b) as f64,
                ))
            }
            "dot_product" => {
                if args.len() != 2 {
                    return None;
                }
                let a_val = self.eval_expr(&args[0], chunk, row)?;
                let b_val = self.eval_expr(&args[1], chunk, row)?;
                let a = a_val.as_vector()?;
                let b = b_val.as_vector()?;
                if a.len() != b.len() {
                    return None;
                }
                Some(Value::Float64(
                    crate::index::vector::dot_product(a, b) as f64
                ))
            }
            "manhattan_distance" => {
                if args.len() != 2 {
                    return None;
                }
                let a_val = self.eval_expr(&args[0], chunk, row)?;
                let b_val = self.eval_expr(&args[1], chunk, row)?;
                let a = a_val.as_vector()?;
                let b = b_val.as_vector()?;
                if a.len() != b.len() {
                    return None;
                }
                Some(Value::Float64(
                    crate::index::vector::manhattan_distance(a, b) as f64,
                ))
            }
            // --- String functions ---
            "keys" => {
                if args.len() != 1 {
                    return None;
                }
                // keys(n) on a node variable: get property keys from the store
                if let FilterExpression::Variable(var) = &args[0] {
                    let col_idx = *self.variable_columns.get(var)?;
                    let col = chunk.column(col_idx)?;
                    if let Some(node_id) = col.get_node_id(row) {
                        let node = self.resolve_node(node_id)?;
                        let keys: Vec<Value> = node
                            .properties
                            .iter()
                            .map(|(k, _)| Value::String(k.as_str().into()))
                            .collect();
                        return Some(Value::List(keys.into()));
                    } else if let Some(edge_id) = col.get_edge_id(row) {
                        // keys(r) sur une ARETE : meme forme que sur un
                        // noeud. Sans cette branche, keys(r) rendait NULL —
                        // ce qui masquait l'etat reel de l'arete apres un
                        // REMOVE r.p (gql_13, remove_sur_arete_...).
                        let edge = self.resolve_edge(edge_id)?;
                        let keys: Vec<Value> = edge
                            .properties
                            .iter()
                            .map(|(k, _)| Value::String(k.as_str().into()))
                            .collect();
                        return Some(Value::List(keys.into()));
                    }
                }
                // keys(map) on a map value
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Map(map) => {
                        let keys: Vec<Value> = map
                            .keys()
                            .map(|k| Value::String(k.as_str().into()))
                            .collect();
                        Some(Value::List(keys.into()))
                    }
                    _ => None,
                }
            }
            "properties" => {
                if args.len() != 1 {
                    return None;
                }
                if let FilterExpression::Variable(var) = &args[0] {
                    let col_idx = *self.variable_columns.get(var)?;
                    let col = chunk.column(col_idx)?;
                    if let Some(node_id) = col.get_node_id(row) {
                        let node = self.resolve_node(node_id)?;
                        let map: std::collections::BTreeMap<PropertyKey, Value> = node
                            .properties
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect();
                        return Some(Value::Map(Arc::new(map)));
                    } else if let Some(edge_id) = col.get_edge_id(row) {
                        let edge = self.resolve_edge(edge_id)?;
                        let map: std::collections::BTreeMap<PropertyKey, Value> = edge
                            .properties
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect();
                        return Some(Value::Map(Arc::new(map)));
                    }
                }
                None
            }
            "trim" => {
                if args.len() == 1 {
                    // Simple trim(string) - trim whitespace
                    let val = self.eval_expr(&args[0], chunk, row)?;
                    return match val {
                        Value::String(s) => Some(Value::String(s.trim().to_string().into())),
                        _ => None,
                    };
                }
                if args.len() == 3 {
                    // Extended trim(string, chars, mode)
                    // mode: 0=both, 1=leading, 2=trailing
                    let val = self.eval_expr(&args[0], chunk, row)?;
                    let chars_val = self.eval_expr(&args[1], chunk, row)?;
                    let mode_val = self.eval_expr(&args[2], chunk, row)?;
                    let Value::String(s) = val else { return None };
                    let Value::String(chars) = chars_val else {
                        return None;
                    };
                    let Value::Int64(mode) = mode_val else {
                        return None;
                    };
                    let char_set: Vec<char> = chars.chars().collect();
                    let result = match mode {
                        0 => s.trim_matches(|c| char_set.contains(&c)).to_string(),
                        1 => s.trim_start_matches(|c| char_set.contains(&c)).to_string(),
                        2 => s.trim_end_matches(|c| char_set.contains(&c)).to_string(),
                        _ => return None,
                    };
                    return Some(Value::String(result.into()));
                }
                None
            }
            "ltrim" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::String(s) => Some(Value::String(s.trim_start().to_string().into())),
                    _ => None,
                }
            }
            "rtrim" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::String(s) => Some(Value::String(s.trim_end().to_string().into())),
                    _ => None,
                }
            }
            "replace" => {
                if args.len() != 3 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                let search = self.eval_expr(&args[1], chunk, row)?;
                let replacement = self.eval_expr(&args[2], chunk, row)?;
                match (&val, &search, &replacement) {
                    (Value::String(s), Value::String(from), Value::String(to)) => {
                        Some(Value::String(s.replace(from.as_str(), to.as_str()).into()))
                    }
                    _ => None,
                }
            }
            "substring" => {
                if args.len() < 2 || args.len() > 3 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                let start = self.eval_expr(&args[1], chunk, row)?;
                let Value::String(s) = val else {
                    return None;
                };
                let Value::Int64(start_idx) = start else {
                    return None;
                };
                let start_idx = start_idx.max(0) as usize;
                if args.len() == 3 {
                    let length = self.eval_expr(&args[2], chunk, row)?;
                    let Value::Int64(len) = length else {
                        return None;
                    };
                    let len = len.max(0) as usize;
                    let chars: String = s.chars().skip(start_idx).take(len).collect();
                    Some(Value::String(chars.into()))
                } else {
                    let chars: String = s.chars().skip(start_idx).collect();
                    Some(Value::String(chars.into()))
                }
            }
            "split" => {
                if args.len() != 2 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                let delim = self.eval_expr(&args[1], chunk, row)?;
                match (&val, &delim) {
                    (Value::String(s), Value::String(d)) => {
                        let parts: Vec<Value> = s
                            .split(d.as_str())
                            .map(|p| Value::String(p.to_string().into()))
                            .collect();
                        Some(Value::List(parts.into()))
                    }
                    _ => None,
                }
            }
            "toupper" | "upper" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::String(s) => Some(Value::String(s.to_uppercase().into())),
                    _ => None,
                }
            }
            "tolower" | "lower" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::String(s) => Some(Value::String(s.to_lowercase().into())),
                    _ => None,
                }
            }
            // `CHARACTER_LENGTH` est le synonyme ISO/IEC 39075 de
            // `CHAR_LENGTH` (gql_05) — les deux écritures sont
            // interchangeables.
            "char_length" | "charlength" | "character_length" | "characterlength" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::String(s) => Some(Value::Int64(s.chars().count() as i64)),
                    _ => None,
                }
            }
            // --- Numeric functions ---
            "abs" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Int64(i) => Some(Value::Int64(i.abs())),
                    Value::Float64(f) => Some(Value::Float64(f.abs())),
                    _ => None,
                }
            }
            "ceil" | "ceiling" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Int64(i) => Some(Value::Int64(i)),
                    Value::Float64(f) => Some(Value::Int64(f.ceil() as i64)),
                    _ => None,
                }
            }
            "floor" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Int64(i) => Some(Value::Int64(i)),
                    Value::Float64(f) => Some(Value::Int64(f.floor() as i64)),
                    _ => None,
                }
            }
            "round" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Int64(i) => Some(Value::Int64(i)),
                    Value::Float64(f) => Some(Value::Int64(f.round() as i64)),
                    _ => None,
                }
            }
            "sqrt" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Int64(i) => Some(Value::Float64((i as f64).sqrt())),
                    Value::Float64(f) => Some(Value::Float64(f.sqrt())),
                    _ => None,
                }
            }
            "rand" | "random" => {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                row.hash(&mut hasher);
                web_time::SystemTime::now()
                    .duration_since(web_time::UNIX_EPOCH)
                    .ok()?
                    .as_nanos()
                    .hash(&mut hasher);
                let hash = hasher.finish();
                let random = (hash as f64) / (u64::MAX as f64);
                Some(Value::Float64(random))
            }
            // --- Collection functions ---
            "range" => {
                if args.len() < 2 || args.len() > 3 {
                    return None;
                }
                let start = self.eval_expr(&args[0], chunk, row)?;
                let stop = self.eval_expr(&args[1], chunk, row)?;
                let Value::Int64(start_val) = start else {
                    return None;
                };
                let Value::Int64(end_val) = stop else {
                    return None;
                };
                let step = if args.len() == 3 {
                    let s = self.eval_expr(&args[2], chunk, row)?;
                    let Value::Int64(sv) = s else {
                        return None;
                    };
                    if sv == 0 {
                        return None;
                    }
                    sv
                } else {
                    1
                };
                let mut result = Vec::new();
                let mut current = start_val;
                if step > 0 {
                    while current <= end_val {
                        result.push(Value::Int64(current));
                        current += step;
                    }
                } else {
                    while current >= end_val {
                        result.push(Value::Int64(current));
                        current += step;
                    }
                }
                Some(Value::List(result.into()))
            }
            // --- String functions (left, right) ---
            "left" => {
                if args.len() != 2 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                let len = self.eval_expr(&args[1], chunk, row)?;
                match (&val, &len) {
                    (Value::String(s), Value::Int64(n)) => {
                        let n = (*n).max(0) as usize;
                        let result: String = s.chars().take(n).collect();
                        Some(Value::String(result.into()))
                    }
                    _ => None,
                }
            }
            "right" => {
                if args.len() != 2 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                let len = self.eval_expr(&args[1], chunk, row)?;
                match (&val, &len) {
                    (Value::String(s), Value::Int64(n)) => {
                        let n = (*n).max(0) as usize;
                        let char_count = s.chars().count();
                        let skip = char_count.saturating_sub(n);
                        let result: String = s.chars().skip(skip).collect();
                        Some(Value::String(result.into()))
                    }
                    _ => None,
                }
            }
            // --- Numeric functions (sign, log, log10, exp, e, pi) ---
            "sign" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Int64(i) => Some(Value::Int64(i.signum())),
                    Value::Float64(f) => {
                        if f > 0.0 {
                            Some(Value::Int64(1))
                        } else if f < 0.0 {
                            Some(Value::Int64(-1))
                        } else {
                            Some(Value::Int64(0))
                        }
                    }
                    _ => None,
                }
            }
            "power" | "pow" => {
                if args.len() != 2 {
                    return None;
                }
                let base_val = self.eval_expr(&args[0], chunk, row)?;
                let exp_val = self.eval_expr(&args[1], chunk, row)?;
                let base = match base_val {
                    Value::Int64(i) => i as f64,
                    Value::Float64(f) => f,
                    _ => return None,
                };
                let exponent = match exp_val {
                    Value::Int64(i) => i as f64,
                    Value::Float64(f) => f,
                    _ => return None,
                };
                Some(Value::Float64(base.powf(exponent)))
            }
            "log" | "ln" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Int64(i) => Some(Value::Float64((i as f64).ln())),
                    Value::Float64(f) => Some(Value::Float64(f.ln())),
                    _ => None,
                }
            }
            "log10" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Int64(i) => Some(Value::Float64((i as f64).log10())),
                    Value::Float64(f) => Some(Value::Float64(f.log10())),
                    _ => None,
                }
            }
            "log2" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Int64(i) => Some(Value::Float64((i as f64).log2())),
                    Value::Float64(f) => Some(Value::Float64(f.log2())),
                    _ => None,
                }
            }
            "exp" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Int64(i) => Some(Value::Float64((i as f64).exp())),
                    Value::Float64(f) => Some(Value::Float64(f.exp())),
                    _ => None,
                }
            }
            "e" => Some(Value::Float64(std::f64::consts::E)),
            "pi" => Some(Value::Float64(std::f64::consts::PI)),
            // --- Trigonometric functions ---
            "sin" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Int64(i) => Some(Value::Float64((i as f64).sin())),
                    Value::Float64(f) => Some(Value::Float64(f.sin())),
                    _ => None,
                }
            }
            "cos" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Int64(i) => Some(Value::Float64((i as f64).cos())),
                    Value::Float64(f) => Some(Value::Float64(f.cos())),
                    _ => None,
                }
            }
            "tan" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Int64(i) => Some(Value::Float64((i as f64).tan())),
                    Value::Float64(f) => Some(Value::Float64(f.tan())),
                    _ => None,
                }
            }
            "asin" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Int64(i) => Some(Value::Float64((i as f64).asin())),
                    Value::Float64(f) => Some(Value::Float64(f.asin())),
                    _ => None,
                }
            }
            "acos" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Int64(i) => Some(Value::Float64((i as f64).acos())),
                    Value::Float64(f) => Some(Value::Float64(f.acos())),
                    _ => None,
                }
            }
            "atan" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Int64(i) => Some(Value::Float64((i as f64).atan())),
                    Value::Float64(f) => Some(Value::Float64(f.atan())),
                    _ => None,
                }
            }
            "atan2" => {
                if args.len() != 2 {
                    return None;
                }
                let y_val = self.eval_expr(&args[0], chunk, row)?;
                let x_val = self.eval_expr(&args[1], chunk, row)?;
                let y = match y_val {
                    Value::Int64(i) => i as f64,
                    Value::Float64(f) => f,
                    _ => return None,
                };
                let x = match x_val {
                    Value::Int64(i) => i as f64,
                    Value::Float64(f) => f,
                    _ => return None,
                };
                Some(Value::Float64(y.atan2(x)))
            }
            "degrees" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Int64(i) => Some(Value::Float64((i as f64).to_degrees())),
                    Value::Float64(f) => Some(Value::Float64(f.to_degrees())),
                    _ => None,
                }
            }
            "radians" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Int64(i) => Some(Value::Float64((i as f64).to_radians())),
                    Value::Float64(f) => Some(Value::Float64(f.to_radians())),
                    _ => None,
                }
            }
            // Temporal constructors and accessors
            "date" | "todate" => {
                if args.is_empty() {
                    return Some(Value::Date(obrain_common::types::Date::today()));
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::String(s) => obrain_common::types::Date::parse(&s).map(Value::Date),
                    Value::Timestamp(ts) => Some(Value::Date(ts.to_date())),
                    Value::Date(_) => Some(val),
                    Value::Map(m) => {
                        let year = map_int(&m, "year")? as i32;
                        let month = map_int_or(&m, "month", 1) as u32;
                        let day = map_int_or(&m, "day", 1) as u32;
                        obrain_common::types::Date::from_ymd(year, month, day).map(Value::Date)
                    }
                    _ => None,
                }
            }
            "time" | "totime" | "local_time" => {
                if args.is_empty() {
                    return Some(Value::Time(obrain_common::types::Time::now()));
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::String(s) => obrain_common::types::Time::parse(&s).map(Value::Time),
                    Value::Timestamp(ts) => Some(Value::Time(ts.to_time())),
                    Value::Time(_) => Some(val),
                    Value::Map(m) => {
                        let hour = map_int_or(&m, "hour", 0) as u32;
                        let minute = map_int_or(&m, "minute", 0) as u32;
                        let second = map_int_or(&m, "second", 0) as u32;
                        let nanosecond = map_int_or(&m, "nanosecond", 0) as u32;
                        obrain_common::types::Time::from_hms_nano(hour, minute, second, nanosecond)
                            .map(Value::Time)
                    }
                    _ => None,
                }
            }
            "datetime" | "localdatetime" | "local_datetime" | "todatetime" => {
                if args.is_empty() {
                    return Some(Value::Timestamp(obrain_common::types::Timestamp::now()));
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::String(s) => {
                        // Parse ISO datetime: try Date first, then full timestamp
                        if let Some(d) = obrain_common::types::Date::parse(&s) {
                            return Some(Value::Timestamp(d.to_timestamp()));
                        }
                        // Try full ISO format: YYYY-MM-DDTHH:MM:SS[.fff][Z|+HH:MM]
                        if let Some(pos) = s.find('T') {
                            let date_part = &s[..pos];
                            let time_part = &s[pos + 1..];
                            if let (Some(d), Some(t)) = (
                                obrain_common::types::Date::parse(date_part),
                                obrain_common::types::Time::parse(time_part),
                            ) {
                                return Some(Value::Timestamp(
                                    obrain_common::types::Timestamp::from_date_time(d, t),
                                ));
                            }
                        }
                        None
                    }
                    Value::Timestamp(_) => Some(val),
                    Value::Map(m) => {
                        let year = map_int(&m, "year")? as i32;
                        let month = map_int_or(&m, "month", 1) as u32;
                        let day = map_int_or(&m, "day", 1) as u32;
                        let hour = map_int_or(&m, "hour", 0) as u32;
                        let minute = map_int_or(&m, "minute", 0) as u32;
                        let second = map_int_or(&m, "second", 0) as u32;
                        let nanosecond = map_int_or(&m, "nanosecond", 0) as u32;
                        let date = obrain_common::types::Date::from_ymd(year, month, day)?;
                        let time = obrain_common::types::Time::from_hms_nano(
                            hour, minute, second, nanosecond,
                        )?;
                        Some(Value::Timestamp(
                            obrain_common::types::Timestamp::from_date_time(date, time),
                        ))
                    }
                    _ => None,
                }
            }
            "duration" | "toduration" => {
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::String(s) => {
                        obrain_common::types::Duration::parse(&s).map(Value::Duration)
                    }
                    Value::Duration(_) => Some(val),
                    Value::Map(m) => {
                        let years = map_int_or(&m, "years", 0);
                        let months = map_int_or(&m, "months", 0);
                        let weeks = map_int_or(&m, "weeks", 0);
                        let days = map_int_or(&m, "days", 0);
                        let hours = map_int_or(&m, "hours", 0);
                        let minutes = map_int_or(&m, "minutes", 0);
                        let seconds = map_int_or(&m, "seconds", 0);
                        let nanoseconds = map_int_or(&m, "nanoseconds", 0);
                        let total_months = years * 12 + months;
                        let total_days = weeks * 7 + days;
                        let total_nanos = hours * 3_600_000_000_000
                            + minutes * 60_000_000_000
                            + seconds * 1_000_000_000
                            + nanoseconds;
                        Some(Value::Duration(obrain_common::types::Duration::new(
                            total_months,
                            total_days,
                            total_nanos,
                        )))
                    }
                    _ => None,
                }
            }
            "tozoneddatetime" | "zoneddatetime" | "zoned_datetime" => {
                if args.is_empty() {
                    return Some(Value::ZonedDatetime(
                        obrain_common::types::ZonedDatetime::from_timestamp_offset(
                            obrain_common::types::Timestamp::now(),
                            0,
                        ),
                    ));
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::String(s) => {
                        obrain_common::types::ZonedDatetime::parse(&s).map(Value::ZonedDatetime)
                    }
                    Value::Timestamp(ts) => Some(Value::ZonedDatetime(
                        obrain_common::types::ZonedDatetime::from_timestamp_offset(ts, 0),
                    )),
                    Value::ZonedDatetime(_) => Some(val),
                    _ => None,
                }
            }
            "tozonedtime" | "zonedtime" => {
                let val = self.eval_expr(args.first()?, chunk, row)?;
                match val {
                    Value::String(s) => {
                        let t = obrain_common::types::Time::parse(&s)?;
                        if t.offset_seconds().is_some() {
                            Some(Value::Time(t))
                        } else {
                            None
                        }
                    }
                    Value::Time(t) if t.offset_seconds().is_some() => Some(val),
                    _ => None,
                }
            }
            "current_date" | "currentdate" => {
                Some(Value::Date(obrain_common::types::Date::today()))
            }
            "current_time" | "currenttime" => Some(Value::Time(obrain_common::types::Time::now())),
            "now" | "current_timestamp" | "currenttimestamp" => {
                Some(Value::Timestamp(obrain_common::types::Timestamp::now()))
            }
            "timestamp" => {
                // Neo4j-compatible: returns milliseconds since Unix epoch as Int64
                let duration = web_time::SystemTime::now()
                    .duration_since(web_time::UNIX_EPOCH)
                    .unwrap_or(std::time::Duration::ZERO);
                Some(Value::Int64(duration.as_millis() as i64))
            }
            // Component extraction
            "year" => {
                let val = self.eval_expr(args.first()?, chunk, row)?;
                match val {
                    Value::Date(d) => Some(Value::Int64(i64::from(d.year()))),
                    Value::Timestamp(ts) => Some(Value::Int64(i64::from(ts.to_date().year()))),
                    Value::ZonedDatetime(zdt) => {
                        Some(Value::Int64(i64::from(zdt.to_local_date().year())))
                    }
                    _ => None,
                }
            }
            "month" => {
                let val = self.eval_expr(args.first()?, chunk, row)?;
                match val {
                    Value::Date(d) => Some(Value::Int64(i64::from(d.month()))),
                    Value::Timestamp(ts) => Some(Value::Int64(i64::from(ts.to_date().month()))),
                    Value::ZonedDatetime(zdt) => {
                        Some(Value::Int64(i64::from(zdt.to_local_date().month())))
                    }
                    _ => None,
                }
            }
            "day" => {
                let val = self.eval_expr(args.first()?, chunk, row)?;
                match val {
                    Value::Date(d) => Some(Value::Int64(i64::from(d.day()))),
                    Value::Timestamp(ts) => Some(Value::Int64(i64::from(ts.to_date().day()))),
                    Value::ZonedDatetime(zdt) => {
                        Some(Value::Int64(i64::from(zdt.to_local_date().day())))
                    }
                    _ => None,
                }
            }
            "hour" => {
                let val = self.eval_expr(args.first()?, chunk, row)?;
                match val {
                    Value::Time(t) => Some(Value::Int64(i64::from(t.hour()))),
                    Value::Timestamp(ts) => Some(Value::Int64(i64::from(ts.to_time().hour()))),
                    Value::ZonedDatetime(zdt) => {
                        Some(Value::Int64(i64::from(zdt.to_local_time().hour())))
                    }
                    _ => None,
                }
            }
            "minute" => {
                let val = self.eval_expr(args.first()?, chunk, row)?;
                match val {
                    Value::Time(t) => Some(Value::Int64(i64::from(t.minute()))),
                    Value::Timestamp(ts) => Some(Value::Int64(i64::from(ts.to_time().minute()))),
                    Value::ZonedDatetime(zdt) => {
                        Some(Value::Int64(i64::from(zdt.to_local_time().minute())))
                    }
                    _ => None,
                }
            }
            "second" => {
                let val = self.eval_expr(args.first()?, chunk, row)?;
                match val {
                    Value::Time(t) => Some(Value::Int64(i64::from(t.second()))),
                    Value::Timestamp(ts) => Some(Value::Int64(i64::from(ts.to_time().second()))),
                    Value::ZonedDatetime(zdt) => {
                        Some(Value::Int64(i64::from(zdt.to_local_time().second())))
                    }
                    _ => None,
                }
            }
            // --- Temporal truncation ---
            "date_trunc" | "truncate" => {
                if args.len() < 2 {
                    return None;
                }
                let unit = match self.eval_expr(&args[0], chunk, row)? {
                    Value::String(s) => s.to_lowercase(),
                    _ => return None,
                };
                let val = self.eval_expr(&args[1], chunk, row)?;
                match val {
                    Value::Date(d) => Some(Value::Date(d.truncate(&unit)?)),
                    Value::Time(t) => Some(Value::Time(t.truncate(&unit)?)),
                    Value::Timestamp(ts) => Some(Value::Timestamp(ts.truncate(&unit)?)),
                    Value::ZonedDatetime(zdt) => Some(Value::ZonedDatetime(zdt.truncate(&unit)?)),
                    _ => None,
                }
            }
            // --- Path constructor ---
            "path" => {
                if args.len() == 2 {
                    // path(nodes_list, edges_list) - construct from component lists
                    let nodes_val = self.eval_expr(&args[0], chunk, row)?;
                    let edges_val = self.eval_expr(&args[1], chunk, row)?;
                    match (&nodes_val, &edges_val) {
                        (Value::Null, _) | (_, Value::Null) => Some(Value::Null),
                        (Value::List(nodes), Value::List(edges)) => {
                            if nodes.is_empty() || edges.len() != nodes.len() - 1 {
                                return None;
                            }
                            Some(Value::Path {
                                nodes: Arc::from(nodes.as_ref()),
                                edges: Arc::from(edges.as_ref()),
                            })
                        }
                        _ => None,
                    }
                } else {
                    // path(node1, edge1, node2, ...) - alternating nodes and edges
                    if args.is_empty() || args.len().is_multiple_of(2) {
                        return None;
                    }
                    let mut nodes = Vec::with_capacity(args.len() / 2 + 1);
                    let mut edges = Vec::with_capacity(args.len() / 2);
                    for (i, arg) in args.iter().enumerate() {
                        let val = self.eval_expr(arg, chunk, row)?;
                        if i % 2 == 0 {
                            nodes.push(val);
                        } else {
                            edges.push(val);
                        }
                    }
                    Some(Value::Path {
                        nodes: nodes.into(),
                        edges: edges.into(),
                    })
                }
            }
            // --- Path decomposition functions ---
            "nodes" => {
                // nodes(path) - extracts nodes from a path value
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Path { nodes, .. } => Some(Value::List(nodes)),
                    Value::Map(map) => map.get(&PropertyKey::from("nodes")).cloned(),
                    Value::List(items) => {
                        // Legacy: alternating node, edge, node, edge, ...
                        let nodes: Vec<Value> = items.iter().step_by(2).cloned().collect();
                        Some(Value::List(nodes.into()))
                    }
                    _ => None,
                }
            }
            "edges" | "relationships" => {
                // edges(path) / relationships(path) - extracts edges from a path value
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Path { edges, .. } => Some(Value::List(edges)),
                    Value::Map(map) => map.get(&PropertyKey::from("edges")).cloned(),
                    Value::List(items) => {
                        // Legacy: alternating node, edge, node, edge, ...
                        let edges: Vec<Value> = items.iter().skip(1).step_by(2).cloned().collect();
                        Some(Value::List(edges.into()))
                    }
                    _ => None,
                }
            }
            // --- Path predicate functions (ISO GQL) ---
            "isacyclic" => {
                // isAcyclic(path) - true if no node appears more than once
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Path { nodes, .. } => {
                        let mut seen = std::collections::HashSet::new();
                        let acyclic = nodes
                            .iter()
                            .all(|n| seen.insert(HashableValue::new(n.clone())));
                        Some(Value::Bool(acyclic))
                    }
                    Value::Null => Some(Value::Null),
                    _ => None,
                }
            }
            "issimple" => {
                // isSimple(path) - true if no node repeats except possibly first == last
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Path { nodes, .. } => {
                        if nodes.is_empty() {
                            return Some(Value::Bool(true));
                        }
                        let mut seen = std::collections::HashSet::new();
                        let simple = nodes.iter().enumerate().all(|(i, n)| {
                            let hv = HashableValue::new(n.clone());
                            if !seen.insert(hv) {
                                // Duplicate allowed only if last node == first node
                                i == nodes.len() - 1 && n == &nodes[0]
                            } else {
                                true
                            }
                        });
                        Some(Value::Bool(simple))
                    }
                    Value::Null => Some(Value::Null),
                    _ => None,
                }
            }
            "istrail" => {
                // isTrail(path) - true if no edge repeats
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::Path { edges, .. } => {
                        let mut seen = std::collections::HashSet::new();
                        let trail = edges
                            .iter()
                            .all(|e| seen.insert(HashableValue::new(e.clone())));
                        Some(Value::Bool(trail))
                    }
                    Value::Null => Some(Value::Null),
                    _ => None,
                }
            }
            // --- GQL ISO standard functions ---
            "normalize" => {
                // normalize(string) — NFC réelle (gql_05) : la sortie doit
                // satisfaire le juge voisin `IS NFC NORMALIZED` (bras
                // "isnormalized", même crate unicode-normalization). L'état
                // antérieur rendait la chaîne TELLE QUELLE (« deferred to
                // Phase 2 ») : le moteur se contredisait lui-même.
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::String(s) => {
                        use unicode_normalization::UnicodeNormalization;
                        Some(Value::String(s.nfc().collect::<String>().into()))
                    }
                    Value::Null => Some(Value::Null),
                    _ => None,
                }
            }
            "isnormalized" => {
                // IS [NFC|NFD|NFKC|NFKD] NORMALIZED - check Unicode normalization form.
                // Args: (string_value) or (string_value, form_name)
                // Default form is NFC when not specified.
                if args.is_empty() || args.len() > 2 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                let form = if args.len() == 2 {
                    match self.eval_expr(&args[1], chunk, row)? {
                        Value::String(s) => s.to_uppercase(),
                        _ => return None,
                    }
                } else {
                    "NFC".to_string()
                };
                match val {
                    Value::String(ref s) => {
                        use unicode_normalization::UnicodeNormalization;
                        let normalized = match form.as_str() {
                            "NFC" => s.nfc().collect::<String>() == s.as_ref(),
                            "NFD" => s.nfd().collect::<String>() == s.as_ref(),
                            "NFKC" => s.nfkc().collect::<String>() == s.as_ref(),
                            "NFKD" => s.nfkd().collect::<String>() == s.as_ref(),
                            _ => return None,
                        };
                        Some(Value::Bool(normalized))
                    }
                    Value::Null => Some(Value::Null),
                    _ => None,
                }
            }
            "string_join" => {
                // string_join(list, separator) - join list elements with separator
                if args.len() != 2 {
                    return None;
                }
                let list_val = self.eval_expr(&args[0], chunk, row)?;
                let sep_val = self.eval_expr(&args[1], chunk, row)?;
                match (list_val, sep_val) {
                    (Value::List(items), Value::String(sep)) => {
                        let sep_str: &str = &sep;
                        let joined: String = items
                            .iter()
                            .filter_map(|v| match v {
                                Value::String(s) => Some(s.to_string()),
                                Value::Int64(i) => Some(i.to_string()),
                                Value::Float64(f) => Some(f.to_string()),
                                Value::Bool(b) => Some(b.to_string()),
                                Value::Null => None,
                                other => Some(format!("{other}")),
                            })
                            .collect::<Vec<String>>()
                            .join(sep_str);
                        Some(Value::String(joined.into()))
                    }
                    (Value::Null, _) | (_, Value::Null) => Some(Value::Null),
                    _ => None,
                }
            }
            "session_user" => {
                // session_user() - returns the current session user
                // For embedded databases, returns a default user string
                Some(Value::String("default".into()))
            }
            // ISO/IEC 39075 Section 17.1 / Section 21: session schema/graph references
            "current_schema" => Some(self.session_context.current_schema.as_ref().map_or_else(
                || Value::String("default".into()),
                |s| Value::String(s.clone().into()),
            )),
            "current_graph" => Some(self.session_context.current_graph.as_ref().map_or_else(
                || Value::String("default".into()),
                |g| Value::String(g.clone().into()),
            )),
            "home_schema" | "home_graph" => {
                // Home schema/graph: not configurable yet, returns null
                Some(Value::Null)
            }
            // Obrain extension: info() returns database metadata as a map
            "info" => Some(self.session_context.db_info.get().clone()),
            // Obrain extension: schema() returns schema metadata as a map
            "schema" => Some(self.session_context.schema_info.get().clone()),
            "octet_length" | "byte_length" => {
                // octet_length(string) - byte length
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::String(s) => Some(Value::Int64(s.len() as i64)),
                    Value::Null => Some(Value::Null),
                    _ => None,
                }
            }
            "tolist" => {
                // toList(value) - wraps a scalar in a single-element list, or returns list as-is
                if args.len() != 1 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                match val {
                    Value::List(_) => Some(val),
                    Value::Null => Some(Value::Null),
                    other => Some(Value::List(vec![other].into())),
                }
            }
            "totypedlist" => {
                // toTypedList(value, element_type) - coerces value to a list with typed elements
                if args.len() != 2 {
                    return None;
                }
                let val = self.eval_expr(&args[0], chunk, row)?;
                let Value::String(elem_type) = self.eval_expr(&args[1], chunk, row)? else {
                    return None;
                };
                if matches!(val, Value::Null) {
                    return Some(Value::Null);
                }
                // Wrap scalar in a list first
                let items = match val {
                    Value::List(items) => items.to_vec(),
                    other => vec![other],
                };
                // Coerce each element to the target type
                let coerced: Option<Vec<Value>> = items
                    .into_iter()
                    .map(|v| Self::coerce_to_type(v, &elem_type))
                    .collect();
                coerced.map(|v| Value::List(v.into()))
            }
            "nullif" => {
                // NULLIF(expr1, expr2) - returns NULL if expr1 = expr2, else expr1
                if args.len() != 2 {
                    return None;
                }
                let val1 = self.eval_expr(&args[0], chunk, row)?;
                let val2 = self.eval_expr(&args[1], chunk, row)?;
                // Three-valued: NULLIF(NULL, x) = NULL; NULLIF(x, NULL) = x
                if val1.is_null() || val2.is_null() {
                    Some(val1)
                } else if Self::values_equal(&val1, &val2) {
                    Some(Value::Null)
                } else {
                    Some(val1)
                }
            }
            "startnode" => {
                // startNode(r) — returns the source node of relationship r
                if args.len() != 1 {
                    return None;
                }
                let edge_id = if let FilterExpression::Variable(var) = &args[0] {
                    let col_idx = *self.variable_columns.get(var)?;
                    let col = chunk.column(col_idx)?;
                    col.get_edge_id(row)?
                } else {
                    let val = self.eval_expr(&args[0], chunk, row)?;
                    match val {
                        Value::Int64(id) => obrain_common::types::EdgeId(id as u64),
                        _ => return None,
                    }
                };
                let edge = self.resolve_edge(edge_id)?;
                let node = self.resolve_node(edge.src)?;
                Some(Value::Int64(node.id.0 as i64))
            }
            "endnode" => {
                // endNode(r) — returns the destination node of relationship r
                if args.len() != 1 {
                    return None;
                }
                let edge_id = if let FilterExpression::Variable(var) = &args[0] {
                    let col_idx = *self.variable_columns.get(var)?;
                    let col = chunk.column(col_idx)?;
                    col.get_edge_id(row)?
                } else {
                    let val = self.eval_expr(&args[0], chunk, row)?;
                    match val {
                        Value::Int64(id) => obrain_common::types::EdgeId(id as u64),
                        _ => return None,
                    }
                };
                let edge = self.resolve_edge(edge_id)?;
                let node = self.resolve_node(edge.dst)?;
                Some(Value::Int64(node.id.0 as i64))
            }
            // Une fonction INCONNUE est une erreur de requête (ISO/IEC
            // 39075 §23, condition 42000) : rendre NULL transformait une
            // faute de frappe en colonne silencieusement vide (gql_05).
            // Même canal que la division par zéro (vague 1) : l'erreur est
            // consignée, l'opérateur de projection la relève et l'exécution
            // échoue franchement.
            inconnue => {
                // ── L'HÔTE A-T-IL CETTE FONCTION ? ──────────────────────────
                // `obrain.energy(n)`, une métrique branchée par un embedder, un
                // algorithme enregistré par un plugin : le noyau ne peut pas les
                // connaître (il ne dépend pas d'`obrain-adapters`), mais il ne
                // doit pas les DÉCLARER INCONNUES. Le vocabulaire est un point
                // d'extension, et un hôte l'installe une fois par processus.
                // Sans hôte, la conduite est celle d'avant — mot pour mot.
                let valeurs: Option<Vec<Value>> =
                    args.iter().map(|a| self.eval_expr(a, chunk, row)).collect();
                if let Some(valeurs) = valeurs
                    && let Some(resultat) =
                        crate::execution::fonction_hote::appeler(inconnue, &valeurs)
                {
                    return match resultat {
                        Ok(valeur) => Some(valeur),
                        Err(raison) => self.signale_erreur(format!(
                            "la fonction hôte '{inconnue}' a échoué : {raison}"
                        )),
                    };
                }
                self.signale_erreur(format!(
                    "fonction inconnue : {inconnue} (aucune fonction de ce nom \
                     dans l'évaluateur ; vérifier l'orthographe)"
                ))
            }
        }
    }

    fn eval_case(
        &self,
        operand: Option<&FilterExpression>,
        when_clauses: &[(FilterExpression, FilterExpression)],
        else_clause: Option<&FilterExpression>,
        chunk: &DataChunk,
        row: usize,
    ) -> Option<Value> {
        if let Some(test_expr) = operand {
            // Simple CASE: CASE expr WHEN val1 THEN res1 ...
            // Use unwrap_or(Null) so a NULL test expression falls through to ELSE
            // rather than short-circuiting the entire CASE (NULL != anything).
            let test_val = self.eval_expr(test_expr, chunk, row).unwrap_or(Value::Null);
            for (when_expr, then_expr) in when_clauses {
                let when_val = self.eval_expr(when_expr, chunk, row).unwrap_or(Value::Null);
                // Three-valued logic: NULL never matches anything in simple CASE
                if !test_val.is_null()
                    && !when_val.is_null()
                    && Self::values_equal(&test_val, &when_val)
                {
                    return self.eval_expr(then_expr, chunk, row);
                }
            }
        } else {
            // Searched CASE: CASE WHEN cond1 THEN res1 ...
            // Use unwrap_or(Null) so a NULL/UNKNOWN condition falls through
            // to the next WHEN or ELSE (three-valued logic: only TRUE matches).
            for (when_expr, then_expr) in when_clauses {
                let when_val = self.eval_expr(when_expr, chunk, row).unwrap_or(Value::Null);
                if when_val.as_bool() == Some(true) {
                    return self.eval_expr(then_expr, chunk, row);
                }
            }
        }
        // No match - return ELSE or NULL
        if let Some(else_expr) = else_clause {
            self.eval_expr(else_expr, chunk, row)
        } else {
            Some(Value::Null)
        }
    }

    fn eval_unary_op(&self, op: UnaryFilterOp, val: Option<Value>) -> Option<Value> {
        match op {
            UnaryFilterOp::Not => {
                let v = val?.as_bool()?;
                Some(Value::Bool(!v))
            }
            UnaryFilterOp::IsNull => Some(Value::Bool(
                val.is_none() || matches!(val, Some(Value::Null)),
            )),
            UnaryFilterOp::IsNotNull => Some(Value::Bool(
                val.is_some() && !matches!(val, Some(Value::Null)),
            )),
            UnaryFilterOp::Neg => match val? {
                // Gardé : `-i64::MIN` déborde (il vaudrait i64::MAX + 1) —
                // même règle que l'arithmétique binaire, cf. eval_arithmetic.
                Value::Int64(i) => match i.checked_neg() {
                    Some(v) => Some(Value::Int64(v)),
                    None => self.signale_erreur(format!("integer overflow: -({i})")),
                },
                Value::Float64(f) => Some(Value::Float64(-f)),
                _ => None,
            },
        }
    }

    /// Égalité STRUCTURELLE (IN, CASE simple, NULLIF, listes/maps) — délègue
    /// au comparateur canonique : `NULL = NULL` VRAI (classes d'équivalence),
    /// flottants IEEE stricts, Int64↔Float64 exact, branches temporelles,
    /// et PLUS AUCUNE coercition chaîne↔nombre (le hack RDF vit désormais
    /// dans le seul planificateur RDF). Pour `=`/`<>`, `eval_binary_op`
    /// passe par `value_cmp::egaler` qui rend UNKNOWN sur classes disjointes.
    fn values_equal(left: &Value, right: &Value) -> bool {
        value_cmp::egalite_structurelle(left, right)
    }

    /// Verdict trivalué d'un opérateur d'ordre : VRAI/FAUX quand les valeurs
    /// s'ordonnent, FAUX quand la classe est commune mais sans ordre (NaN —
    /// IEEE : `<=` reste `<` OU `=`), NULL (UNKNOWN) quand les classes sont
    /// disjointes ou qu'un opérande est NULL.
    fn verdict_ordre(left: &Value, right: &Value, sens: fn(Ordering) -> bool) -> Value {
        match value_cmp::comparer(left, right) {
            value_cmp::Comparaison::Ordonnees(o) => Value::Bool(sens(o)),
            value_cmp::Comparaison::NonOrdonnees => Value::Bool(false),
            value_cmp::Comparaison::Incomparables => Value::Null,
        }
    }

    /// Checks if a value matches a GQL type name (used by IS TYPED).
    fn value_matches_type(val: &Value, type_name: &str) -> bool {
        match type_name {
            "BOOLEAN" | "BOOL" => matches!(val, Value::Bool(_)),
            "INTEGER" | "INT" | "INT64" => matches!(val, Value::Int64(_)),
            "FLOAT" | "FLOAT64" | "DOUBLE" => matches!(val, Value::Float64(_)),
            "STRING" => matches!(val, Value::String(_)),
            "LIST" => matches!(val, Value::List(_)),
            "MAP" | "RECORD" => matches!(val, Value::Map(_)),
            "NULL" => matches!(val, Value::Null),
            "DATE" => matches!(val, Value::Date(_)),
            "TIME" => matches!(val, Value::Time(_)),
            "DATETIME" | "TIMESTAMP" => matches!(val, Value::Timestamp(_)),
            "DURATION" => matches!(val, Value::Duration(_)),
            "PATH" => matches!(val, Value::Path { .. }),
            "NODE" | "EDGE" | "GRAPH" => false, // element refs not stored as values
            _ => false,
        }
    }

    /// Coerces a value to a target GQL type. Returns None if coercion is impossible.
    fn coerce_to_type(val: Value, type_name: &str) -> Option<Value> {
        match type_name.to_uppercase().as_str() {
            "INTEGER" | "INT" | "INT64" => match val {
                Value::Int64(_) => Some(val),
                Value::Float64(f) => Some(Value::Int64(f as i64)),
                Value::String(ref s) => s.parse::<i64>().ok().map(Value::Int64),
                Value::Bool(b) => Some(Value::Int64(i64::from(b))),
                _ => None,
            },
            "FLOAT" | "FLOAT64" | "DOUBLE" => match val {
                Value::Float64(_) => Some(val),
                Value::Int64(i) => Some(Value::Float64(i as f64)),
                Value::String(ref s) => s.parse::<f64>().ok().map(Value::Float64),
                _ => None,
            },
            "STRING" => match val {
                Value::String(_) => Some(val),
                other => Some(Value::String(other.to_string().into())),
            },
            "BOOLEAN" | "BOOL" => match val {
                Value::Bool(_) => Some(val),
                Value::String(ref s) => match s.to_lowercase().as_str() {
                    "true" => Some(Value::Bool(true)),
                    "false" => Some(Value::Bool(false)),
                    _ => None,
                },
                _ => None,
            },
            _ => {
                // For unknown types, keep value as-is if it already matches
                if Self::value_matches_type(&val, &type_name.to_uppercase()) {
                    Some(val)
                } else {
                    None
                }
            }
        }
    }

}

impl Predicate for ExpressionPredicate {
    fn evaluate(&self, chunk: &DataChunk, row: usize) -> bool {
        match self.eval(chunk, row) {
            Some(Value::Bool(b)) => b,
            _ => false,
        }
    }

    fn take_eval_error(&self) -> Option<String> {
        ExpressionPredicate::take_eval_error(self)
    }

    fn prepare_chunk(&self, chunk: &DataChunk) {
        self.pre_extraire_chunk(chunk);
    }
}

/// A filter operator that applies a predicate to filter rows.
pub struct FilterOperator {
    /// Child operator to read from.
    child: Box<dyn Operator>,
    /// Predicate to apply.
    predicate: Box<dyn Predicate>,
}

impl FilterOperator {
    /// Creates a new filter operator.
    pub fn new(child: Box<dyn Operator>, predicate: Box<dyn Predicate>) -> Self {
        Self { child, predicate }
    }
}

impl Operator for FilterOperator {
    fn next(&mut self) -> OperatorResult {
        loop {
            super::verifier_annulation()?;
            // Get next chunk from child
            let Some(mut chunk) = self.child.next()? else {
                return Ok(None);
            };

            // Zone map check: skip entire chunk if no rows can match
            if let Some(hints) = chunk.zone_hints()
                && !self.predicate.might_match_chunk(hints)
            {
                continue; // Skip entire chunk - zone map proves no matches
            }

            // ANNONCE DU CHUNK, avant la première ligne : c'est le seul moment
            // où un prédicat peut amortir une lecture de propriété sur les
            // 2 048 lignes qui suivent (voir `Predicate::prepare_chunk`).
            self.predicate.prepare_chunk(&chunk);

            // Apply predicate to create selection vector, respecting any
            // existing selection from child operators (stacked filters).
            let selection = if let Some(existing) = chunk.selection() {
                let mut sel = SelectionVector::new_empty();
                for pos in 0..existing.len() {
                    if let Some(row) = existing.get(pos)
                        && self.predicate.evaluate(&chunk, row)
                    {
                        sel.push(row);
                    }
                }
                sel
            } else {
                let count = chunk.total_row_count();
                SelectionVector::from_predicate(count, |row| self.predicate.evaluate(&chunk, row))
            };

            // Arithmétique gardée : une faute d'évaluation (division par
            // zéro, débordement i64) consignée pendant le balayage du chunk
            // REFUSE la requête — elle ne doit jamais se déguiser en
            // « ligne non retenue ».
            if let Some(e) = self.predicate.take_eval_error() {
                return Err(super::OperatorError::Execution(e));
            }

            // If nothing passes, skip to next chunk
            if selection.is_empty() {
                continue;
            }

            chunk.set_selection(selection);
            return Ok(Some(chunk));
        }
    }

    fn reset(&mut self) {
        self.child.reset();
    }

    fn name(&self) -> &'static str {
        "Filter"
    }
}

/// Escapes a character for use in a regex pattern.
#[cfg(any(feature = "regex", feature = "regex-lite"))]
fn regex_escape_char(ch: char, out: &mut String) {
    if ".+*?^${}()|[]\\".contains(ch) {
        out.push('\\');
    }
    out.push(ch);
}

/// Converts a `Value` to its string representation for concatenation.
fn value_to_string(val: &Value) -> Option<String> {
    match val {
        Value::Int64(i) => Some(i.to_string()),
        Value::Float64(f) => Some(f.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::String(s) => Some(s.to_string()),
        Value::Null => None,
        _ => Some(format!("{val}")),
    }
}
