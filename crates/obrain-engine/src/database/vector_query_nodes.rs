//! `CALL db.index.vector.queryNodes(index_name, k, query_vector) YIELD node, score`
//! — la forme Neo4j de la recherche vectorielle, en DÉLÉGATION vers le chemin
//! natif d'obrain (#209).
//!
//! ── POURQUOI UN NOM NEO4J DANS OBRAIN ─────────────────────────────────────
//!
//! Le portage de PO (project-orchestrator) parle à Neo4j et à obrain avec le
//! MÊME code : toute sa recherche sémantique passe par cette procédure, depuis
//! 18 sites. Décision PO fa93cf0a (option A) : exposer la procédure plutôt que
//! réécrire ces sites en deux chemins divergents. Ce n'est PAS un second
//! moteur vectoriel : la recherche est celle de `ObrainDB::vector_search`
//! (même registre, même cascade, même calcul — `search::chercher_top_k`).
//!
//! ── RÉSOLUTION DU NOM ─────────────────────────────────────────────────────
//!
//! Neo4j désigne un index par son NOM ; obrain, par `(label, propriété)`. Le
//! nom est porté par le catalogue durable (`substrate.indexcat`, 6e colonne
//! facultative, rétrocompatible) et posé par
//! `ObrainDB::create_named_vector_index` / `ObrainDB::name_vector_index`.
//! Un nom INCONNU est une ERREUR qui le nomme — jamais un résultat vide :
//! « aucun voisin » et « cet index n'existe pas » ne doivent pas se lire pareil.
//!
//! ── SCORE ─────────────────────────────────────────────────────────────────
//!
//! `score` = similarité cosinus = `1 - distance` de `vector_search` (la même
//! métrique, au bit près en `f32`), lignes par score DÉCROISSANT. Attention :
//! Neo4j normalise le cosinus en `(1 + cos) / 2` ; obrain rend le cosinus
//! lui-même — un seuil `score > $min` calibré sur Neo4j n'a donc pas la même
//! valeur ici (voir #209).
//!
//! ── `node` EST UN NŒUD ────────────────────────────────────────────────────
//!
//! La colonne `node` porte l'identifiant du nœud et n'est PAS déclarée
//! scalaire au planificateur : `YIELD node AS n … WHERE n.status = …
//! RETURN n` y lit des propriétés et y résout un nœud, comme après un `MATCH`.

use std::sync::Arc;

use obrain_adapters::plugins::AlgorithmResult;
use obrain_common::types::{NodeId, Value};
use obrain_common::utils::error::{Error, Result};

use crate::query::plan::LogicalExpression;

/// Nom qualifié de la procédure (celui de Neo4j).
pub const NOM: &str = "db.index.vector.queryNodes";

/// Colonnes de sortie : `node` (un nœud), `score` (Float64).
pub const COLONNES: [&str; 2] = ["node", "score"];

/// Colonnes de sortie qui portent un NŒUD (et non un scalaire).
pub const COLONNES_NOEUD: [&str; 1] = ["node"];

/// Ce dont la procédure a besoin de la base. Implémenté par
/// `PoigneeIndexVectoriels` (`database/search.rs`), construit par `ObrainDB`.
pub trait IndexVectorielsNommes: Send + Sync {
    /// Recherche les `k` plus proches voisins de `query` dans l'index nommé
    /// `nom`. Rend `(nœud, distance)` par distance croissante — exactement
    /// le contrat de `ObrainDB::vector_search`.
    ///
    /// # Errors
    ///
    /// Nom inconnu, index déclaré mais pas chargé, dimension incohérente.
    fn chercher_par_nom(&self, nom: &str, query: &[f32], k: usize) -> Result<Vec<(NodeId, f32)>>;
}

/// Ce nom désigne-t-il la procédure ? Correspondance EXACTE, comme Neo4j
/// (dont les noms de procédures sont sensibles à la casse) et comme la liste
/// des procédures de lecture (`query::plan::PROCEDURES_DE_LECTURE`).
#[must_use]
pub fn est_query_nodes(resolved_name: &str) -> bool {
    resolved_name == NOM
}

/// Exécute `CALL db.index.vector.queryNodes(...)` si le nom matche.
///
/// `Ok(None)` si le nom n'est pas celui de la procédure.
///
/// # Errors
///
/// Poignée absente, arguments mal formés, index inconnu ou non chargé,
/// dimension de vecteur incohérente.
pub fn try_execute_query_nodes_procedure(
    resolved_name: &str,
    arguments: &[LogicalExpression],
    poignee: Option<&Arc<dyn IndexVectorielsNommes>>,
) -> Result<Option<AlgorithmResult>> {
    if !est_query_nodes(resolved_name) {
        return Ok(None);
    }
    let Some(poignee) = poignee else {
        return Err(Error::Internal(format!(
            "{NOM} : aucune base attachée à cette session (poignée d'index vectoriels absente)"
        )));
    };
    if arguments.len() != 3 {
        return Err(Error::Internal(format!(
            "{NOM} attend 3 arguments : (index_name, k, query_vector), reçu {}",
            arguments.len()
        )));
    }
    let valeurs = arguments
        .iter()
        .map(evaluer_constante)
        .collect::<Result<Vec<Value>>>()?;
    let nom = match &valeurs[0] {
        Value::String(s) => s.to_string(),
        autre => {
            return Err(Error::Internal(format!(
                "{NOM} : argument 0 (index_name) doit être une chaîne, reçu {autre:?}"
            )));
        }
    };
    let k = match &valeurs[1] {
        Value::Int64(n) if *n > 0 => *n as usize,
        // Neo4j accepte un entier ; un flottant entier (`10.0`, venu d'un
        // client JSON) est admis, un flottant fractionnaire ne l'est pas.
        Value::Float64(f) if *f >= 1.0 && f.fract() == 0.0 => *f as usize,
        autre => {
            return Err(Error::Internal(format!(
                "{NOM} : argument 1 (k) doit être un entier strictement positif, reçu {autre:?}"
            )));
        }
    };
    let query = vecteur(&valeurs[2])?;

    let mut lignes = poignee.chercher_par_nom(&nom, &query, k)?;
    // Le cascade rend déjà les distances croissantes ; le tri explicite (stable)
    // GARANTIT l'ordre par score décroissant que promet la procédure.
    lignes.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut result = AlgorithmResult::new(COLONNES.iter().map(|c| (*c).to_string()).collect());
    for (id, distance) in lignes {
        // `1 - distance` défait EXACTEMENT la conversion de `vector_search`
        // (`distance = 1 - cosinus`, en f32) : le score est le cosinus.
        let score = 1.0_f32 - distance;
        result.add_row(vec![
            Value::Int64(id.0 as i64),
            Value::Float64(f64::from(score)),
        ]);
    }
    Ok(Some(result))
}

/// La ligne de `obrain.procedures()` pour cette procédure.
#[must_use]
pub fn ligne_procedures() -> Vec<Value> {
    vec![
        Value::from(NOM),
        Value::from(
            "Neo4j-compatible vector search: the k nearest neighbours of query_vector in the \
             named vector index (delegates to ObrainDB::vector_search; score = cosine \
             similarity, descending)",
        ),
        Value::from("index_name (String), k (Int64), query_vector (List<Float>)"),
        Value::from("node, score"),
    ]
}

/// Évalue un argument constant (les paramètres `$x` sont déjà substitués).
fn evaluer_constante(expr: &LogicalExpression) -> Result<Value> {
    match expr {
        LogicalExpression::Literal(v) => Ok(v.clone()),
        LogicalExpression::List(items) => Ok(Value::List(
            items
                .iter()
                .map(evaluer_constante)
                .collect::<Result<Vec<_>>>()?
                .into(),
        )),
        LogicalExpression::Parameter(nom) => Err(Error::Internal(format!(
            "{NOM} : paramètre ${nom} non fourni"
        ))),
        autre => crate::query::planner::eval_constant_expression(autre).map_err(|e| {
            Error::Internal(format!(
                "{NOM} : les arguments doivent être des constantes ({e})"
            ))
        }),
    }
}

fn vecteur(v: &Value) -> Result<Vec<f32>> {
    match v {
        Value::Vector(xs) => Ok(xs.to_vec()),
        Value::List(items) => items
            .iter()
            .map(|x| match x {
                Value::Float64(f) => Ok(*f as f32),
                Value::Int64(n) => Ok(*n as f32),
                autre => Err(Error::Internal(format!(
                    "{NOM} : argument 2 (query_vector) doit être une liste de nombres, \
                     élément {autre:?}"
                ))),
            })
            .collect(),
        autre => Err(Error::Internal(format!(
            "{NOM} : argument 2 (query_vector) doit être une liste de nombres, reçu {autre:?}"
        ))),
    }
}
