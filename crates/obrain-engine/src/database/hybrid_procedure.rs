//! `CALL obrain.hybrid_search(...)` — la recherche hybride (BM25 + vecteur,
//! fusion RRF ou pondérée) comme procédure GQL.
//!
//! ── POURQUOI UNE POIGNÉE, ET PAS `ObrainDB` ──────────────────────────────
//!
//! Les index texte et vectoriel vivent dans `ObrainDB` (registres
//! `substrate_text_indexes` / `substrate_vector_indexes`), pas dans le
//! `GraphStore` que reçoit un `GraphAlgorithm` du registre. Et
//! `ObrainDB::session()` prend `&self` : la session n'a aucun `Arc<ObrainDB>`
//! à transporter jusqu'au planificateur. On câble donc une POIGNÉE — un
//! `Arc<dyn RechercheHybride>` construit par `ObrainDB` à partir de clones
//! `Arc` des registres et du store substrat — sur le chemin qu'empruntent
//! déjà `substrate_store` et `cdc_log` :
//!
//!   ObrainDB::session() → Session::set_recherche_hybride
//!     → QueryProcessor::with_recherche_hybride
//!       → Planner::with_recherche_hybride
//!         → plan_call_procedure (planner/lpg/mutation.rs)
//!
//! `ObrainDB::hybrid_search` (API Rust) passe par la MÊME poignée : une seule
//! fusion, un seul contrat (voir `database/search.rs`). Pas de registre
//! global : la poignée est propre à la base, comme ses index.
//!
//! ── FORME GQL ────────────────────────────────────────────────────────────
//!
//! ```text
//! CALL obrain.hybrid_search(label, text_property, vector_property, query_text
//!                           [, query_vector] [, {k, fusion, rrf_k, weights}])
//! YIELD node_id, score
//! ```
//!
//! Quatre arguments positionnels obligatoires (comme `search_by_role`), le
//! vecteur de requête en cinquième (liste de nombres, `Value::Vector`, ou
//! paramètre `$vec`), et une map d'options en dernier (comme
//! `pagerank({damping})`) : `k` (défaut 10), `fusion` (`'rrf'` défaut ou
//! `'weighted'`), `rrf_k` (défaut 60), `weights` (fusion pondérée, défaut
//! `[0.5, 0.5]`).
//!
//! ── INDEX : EXPLICITE, JAMAIS SILENCIEUX ─────────────────────────────────
//!
//! L'API Rust est tolérante (elle fusionne ce qui est disponible). La
//! procédure, elle, refuse ce qu'elle ne peut pas honorer : l'index texte
//! est requis (le texte de requête est obligatoire), l'index vectoriel est
//! requis dès qu'un vecteur est fourni. Un index déclaré dans le catalogue
//! mais pas encore chargé dans ce processus (base rouverte, reconstruction à
//! la demande que seule l'API Rust déclenche) est nommé, pas sauté.

use std::sync::Arc;

use obrain_adapters::plugins::AlgorithmResult;
use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_common::utils::error::{Error, Result};
use obrain_core::index::text::FusionMethod;

use crate::query::plan::{LogicalExpression, UnaryOp};

/// Nom qualifié de la procédure.
pub const NOM: &str = "obrain.hybrid_search";

/// `k` par défaut (nombre de résultats).
pub const K_PAR_DEFAUT: usize = 10;

/// Constante RRF par défaut (celle de `FusionMethod::default()`).
pub const RRF_K_PAR_DEFAUT: usize = 60;

/// Ce que la poignée sait d'un index (texte ou vectoriel) sur `(label, propriété)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EtatIndex {
    /// Présent dans le registre du processus : interrogeable.
    Charge,
    /// Déclaré dans le catalogue durable mais absent du registre (base
    /// rouverte, pas encore reconstruit à la demande).
    DeclareNonCharge,
    /// Ni chargé ni déclaré.
    Absent,
}

/// Ce dont la procédure a besoin de la base : l'état des index et la
/// recherche elle-même. Implémenté par `PoigneeRechercheHybride`
/// (`database/search.rs`), construit par `ObrainDB`.
pub trait RechercheHybride: Send + Sync {
    /// État de l'index texte sur `:label(property)`.
    fn etat_index_texte(&self, label: &str, property: &str) -> EtatIndex;
    /// État de l'index vectoriel sur `:label(property)`.
    fn etat_index_vectoriel(&self, label: &str, property: &str) -> EtatIndex;
    /// Recherche hybride : BM25 sur `text_property`, cosinus sur
    /// `vector_property` si `query_vector` est fourni, fusion des sources
    /// disponibles. Rend `(node_id, score)` par score décroissant.
    ///
    /// # Errors
    ///
    /// Dimension de requête incohérente avec l'index vectoriel.
    #[allow(clippy::too_many_arguments)]
    fn hybrid_search(
        &self,
        label: &str,
        text_property: &str,
        vector_property: &str,
        query_text: &str,
        query_vector: Option<&[f32]>,
        k: usize,
        fusion: Option<FusionMethod>,
    ) -> Result<Vec<(NodeId, f64)>>;
}

/// Ce nom désigne-t-il la procédure (`obrain.hybrid_search` ou `hybrid_search`) ?
#[must_use]
pub fn est_hybrid_search(resolved_name: &str) -> bool {
    resolved_name == NOM || resolved_name == "hybrid_search"
}

/// Exécute `CALL obrain.hybrid_search(...)` si le nom matche.
///
/// `Ok(None)` si le nom n'est pas celui de la procédure.
///
/// # Errors
///
/// Poignée absente (session sans base), arguments mal formés, index requis
/// absent ou non chargé, dimension de vecteur incohérente.
pub fn try_execute_hybrid_search_procedure(
    resolved_name: &str,
    arguments: &[LogicalExpression],
    poignee: Option<&Arc<dyn RechercheHybride>>,
) -> Result<Option<AlgorithmResult>> {
    if !est_hybrid_search(resolved_name) {
        return Ok(None);
    }
    let Some(poignee) = poignee else {
        return Err(Error::Internal(format!(
            "{NOM} : aucune base attachée à cette session (poignée de recherche hybride absente)"
        )));
    };
    let args = ArgumentsHybride::depuis(arguments)?;
    verifier_index(poignee.as_ref(), &args)?;
    let lignes = poignee.hybrid_search(
        &args.label,
        &args.text_property,
        &args.vector_property,
        &args.query_text,
        args.query_vector.as_deref(),
        args.k,
        Some(args.fusion),
    )?;
    let mut result = AlgorithmResult::new(vec!["node_id".to_string(), "score".to_string()]);
    for (id, score) in lignes {
        result.add_row(vec![Value::Int64(id.0 as i64), Value::Float64(score)]);
    }
    Ok(Some(result))
}

/// La ligne de `obrain.procedures()` pour cette procédure :
/// `(name, description, parameters, output_columns)`.
#[must_use]
pub fn ligne_procedures() -> Vec<Value> {
    vec![
        Value::from(NOM),
        Value::from(
            "Hybrid search: BM25 on a text index + cosine on a vector index, fused by RRF \
             (default) or weighted scores",
        ),
        Value::from(
            "label (String), text_property (String), vector_property (String), \
             query_text (String), query_vector (List<Float>, optional), \
             options (Map, optional: k=10, fusion='rrf'|'weighted', rrf_k=60, weights)",
        ),
        Value::from("node_id, score"),
    ]
}

/// Arguments décodés de l'appel.
struct ArgumentsHybride {
    label: String,
    text_property: String,
    vector_property: String,
    query_text: String,
    query_vector: Option<Vec<f32>>,
    k: usize,
    fusion: FusionMethod,
}

impl ArgumentsHybride {
    fn depuis(arguments: &[LogicalExpression]) -> Result<Self> {
        if arguments.len() < 4 || arguments.len() > 6 {
            return Err(Error::Internal(format!(
                "{NOM} attend 4 à 6 arguments : (label, text_property, vector_property, \
                 query_text [, query_vector] [, {{k, fusion, rrf_k, weights}}]), reçu {}",
                arguments.len()
            )));
        }
        let valeurs = arguments
            .iter()
            .map(evaluer_constante)
            .collect::<Result<Vec<Value>>>()?;

        let label = chaine(&valeurs[0], 0, "label")?;
        let text_property = chaine(&valeurs[1], 1, "text_property")?;
        let vector_property = chaine(&valeurs[2], 2, "vector_property")?;
        let query_text = chaine(&valeurs[3], 3, "query_text")?;

        let mut query_vector = None;
        let mut options = None;
        for (i, v) in valeurs.iter().enumerate().skip(4) {
            match v {
                Value::Map(m) if options.is_none() => options = Some(Arc::clone(m)),
                Value::Map(_) => {
                    return Err(Error::Internal(format!(
                        "{NOM} : argument {i} — une seule map d'options est admise"
                    )));
                }
                Value::List(_) | Value::Vector(_)
                    if query_vector.is_none() && options.is_none() =>
                {
                    query_vector = Some(vecteur(v, i, "query_vector")?);
                }
                Value::Null if query_vector.is_none() && options.is_none() => {}
                autre => {
                    return Err(Error::Internal(format!(
                        "{NOM} : argument {i} — attendu query_vector (liste de nombres) \
                         puis une map d'options, reçu {autre:?}"
                    )));
                }
            }
        }

        let mut k = K_PAR_DEFAUT;
        let mut fusion_nom = "rrf".to_string();
        let mut rrf_k = RRF_K_PAR_DEFAUT;
        let mut weights: Option<Vec<f64>> = None;
        if let Some(opts) = options {
            for (cle, valeur) in opts.iter() {
                match cle.as_str() {
                    "k" => k = entier_positif(valeur, "k", false)?,
                    "rrf_k" => rrf_k = entier_positif(valeur, "rrf_k", true)?,
                    "fusion" => fusion_nom = chaine(valeur, 5, "fusion")?,
                    "weights" => {
                        weights = Some(
                            vecteur(valeur, 5, "weights")?
                                .into_iter()
                                .map(f64::from)
                                .collect(),
                        );
                    }
                    inconnue => {
                        return Err(Error::Internal(format!(
                            "{NOM} : option inconnue '{inconnue}' (admises : k, fusion, rrf_k, weights)"
                        )));
                    }
                }
            }
        }
        let fusion = match fusion_nom.to_ascii_lowercase().as_str() {
            "rrf" => FusionMethod::Rrf { k: rrf_k },
            "weighted" => FusionMethod::Weighted {
                weights: weights.unwrap_or_else(|| vec![0.5, 0.5]),
            },
            autre => {
                return Err(Error::Internal(format!(
                    "{NOM} : fusion '{autre}' inconnue (admises : 'rrf', 'weighted')"
                )));
            }
        };

        Ok(Self {
            label,
            text_property,
            vector_property,
            query_text,
            query_vector,
            k,
            fusion,
        })
    }
}

/// Les index que l'appel exige doivent être chargés : texte toujours,
/// vectoriel dès qu'un vecteur est fourni. Le message nomme les index
/// attendus et comment les créer.
fn verifier_index(poignee: &dyn RechercheHybride, a: &ArgumentsHybride) -> Result<()> {
    let label = &a.label;
    let (tp, vp) = (&a.text_property, &a.vector_property);
    let texte = poignee.etat_index_texte(label, tp);
    let vecteur = poignee.etat_index_vectoriel(label, vp);

    let attendus = format!(
        "un index texte :{label}({tp}) (ObrainDB::create_text_index('{label}', '{tp}') ou \
         CREATE TEXT INDEX) et, pour la part vectorielle, un index vectoriel :{label}({vp}) \
         (ObrainDB::create_vector_index('{label}', '{vp}', Some(dimension), Some(\"cosine\"), …))"
    );
    match texte {
        EtatIndex::Absent => {
            return Err(Error::Internal(format!(
                "{NOM} : aucun index texte sur :{label}({tp}) — la procédure attend {attendus}"
            )));
        }
        EtatIndex::DeclareNonCharge => return Err(non_charge("texte", label, tp, "text_search")),
        EtatIndex::Charge => {}
    }
    if a.query_vector.is_some() {
        match vecteur {
            EtatIndex::Absent => {
                return Err(Error::Internal(format!(
                    "{NOM} : un vecteur de requête est fourni mais il n'y a aucun index \
                     vectoriel sur :{label}({vp}) — la procédure attend {attendus}"
                )));
            }
            EtatIndex::DeclareNonCharge => {
                return Err(non_charge("vectoriel", label, vp, "vector_search"));
            }
            EtatIndex::Charge => {}
        }
    }
    Ok(())
}

fn non_charge(genre: &str, label: &str, prop: &str, api: &str) -> Error {
    Error::Internal(format!(
        "{NOM} : l'index {genre} :{label}({prop}) est déclaré dans le catalogue mais pas \
         encore chargé dans ce processus — appelle ObrainDB::{api} une fois (reconstruction \
         à la demande) avant la procédure"
    ))
}

/// Évalue un argument constant : littéral, négation, liste et map de
/// constantes. (Les paramètres `$x` ont déjà été substitués par le
/// processeur.)
fn evaluer_constante(expr: &LogicalExpression) -> Result<Value> {
    match expr {
        LogicalExpression::Literal(v) => Ok(v.clone()),
        LogicalExpression::Unary {
            op: UnaryOp::Neg,
            operand,
        } => match evaluer_constante(operand)? {
            Value::Int64(n) => Ok(Value::Int64(-n)),
            Value::Float64(f) => Ok(Value::Float64(-f)),
            autre => Err(Error::Internal(format!(
                "{NOM} : impossible de nier {autre:?}"
            ))),
        },
        LogicalExpression::List(items) => Ok(Value::List(
            items
                .iter()
                .map(evaluer_constante)
                .collect::<Result<Vec<_>>>()?
                .into(),
        )),
        LogicalExpression::Map(pairs) => {
            let mut map = std::collections::BTreeMap::new();
            for (cle, v) in pairs {
                map.insert(PropertyKey::new(cle), evaluer_constante(v)?);
            }
            Ok(Value::Map(Arc::new(map)))
        }
        LogicalExpression::Parameter(nom) => Err(Error::Internal(format!(
            "{NOM} : paramètre ${nom} non fourni"
        ))),
        autre => Err(Error::Internal(format!(
            "{NOM} : les arguments doivent être des constantes, reçu {autre:?}"
        ))),
    }
}

fn chaine(v: &Value, i: usize, nom: &str) -> Result<String> {
    match v {
        Value::String(s) => Ok(s.to_string()),
        autre => Err(Error::Internal(format!(
            "{NOM} : argument {i} ({nom}) doit être une chaîne, reçu {autre:?}"
        ))),
    }
}

fn entier_positif(v: &Value, nom: &str, zero_admis: bool) -> Result<usize> {
    match v {
        Value::Int64(n) if *n > 0 || (zero_admis && *n == 0) => Ok(*n as usize),
        autre => Err(Error::Internal(format!(
            "{NOM} : option {nom} doit être un entier {}, reçu {autre:?}",
            if zero_admis { "positif ou nul" } else { "strictement positif" }
        ))),
    }
}

fn vecteur(v: &Value, i: usize, nom: &str) -> Result<Vec<f32>> {
    match v {
        Value::Vector(xs) => Ok(xs.to_vec()),
        Value::List(items) => items
            .iter()
            .map(|x| match x {
                Value::Float64(f) => Ok(*f as f32),
                Value::Int64(n) => Ok(*n as f32),
                autre => Err(Error::Internal(format!(
                    "{NOM} : argument {i} ({nom}) doit être une liste de nombres, élément {autre:?}"
                ))),
            })
            .collect(),
        autre => Err(Error::Internal(format!(
            "{NOM} : argument {i} ({nom}) doit être une liste de nombres, reçu {autre:?}"
        ))),
    }
}
