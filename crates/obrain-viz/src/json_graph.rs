//! Export json-graph **v1** : source de vérité versionnée du sous-graphe
//! (base du diff `viz_timeline`/`compare` et de l'interop).
//!
//! Enveloppe : `{schema, stats, nodes, edges, generated_at?}`. `stats` est
//! toujours recalculé depuis les vecteurs (cohérence garantie) ;
//! `generated_at` n'est posé que si demandé — par défaut l'octet de sortie est
//! **déterministe** à entrée canonique identique.

use serde::{Deserialize, Serialize};

use crate::ir::{Stats, Subgraph, VizEdge, VizNode};
use crate::{VizResult, validate_edges, validate_ids};

/// Identifiant de schéma de la v1.
pub const JSON_GRAPH_SCHEMA_V1: &str = "obrain/json-graph@1";

/// Document json-graph v1 (enveloppe sérialisée).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonGraphDoc {
    pub schema: String,
    pub stats: Stats,
    pub nodes: Vec<VizNode>,
    pub edges: Vec<VizEdge>,
    /// Horodatage optionnel (overlays volatils / exports datés). Omi par défaut
    /// pour préserver le déterminisme octet-à-octet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<String>,
}

impl JsonGraphDoc {
    /// Construit le document (schéma + stats recalculées) depuis un sous-graphe.
    #[must_use]
    pub fn from_subgraph(subgraph: &Subgraph) -> Self {
        Self {
            schema: JSON_GRAPH_SCHEMA_V1.to_string(),
            stats: subgraph.stats(),
            nodes: subgraph.nodes.clone(),
            edges: subgraph.edges.clone(),
            generated_at: None,
        }
    }

    /// Pose un horodatage (rend l'octet de sortie non déterministe — assumé
    /// pour les exports datés / overlays volatils).
    #[must_use]
    pub fn with_generated_at(mut self, generated_at: impl Into<String>) -> Self {
        self.generated_at = Some(generated_at.into());
        self
    }

    /// Reconstitue le sous-graphe, en vérifiant le schéma puis le bien-formé
    /// (ids uniques, aucune arête pendante).
    pub fn into_subgraph(self) -> VizResult<Subgraph> {
        if self.schema != JSON_GRAPH_SCHEMA_V1 {
            return Err(crate::VizError::Validation(format!(
                "schéma inconnu: {:?} (attendu {JSON_GRAPH_SCHEMA_V1})",
                self.schema
            )));
        }
        validate_ids(&self.nodes)?;
        validate_edges(&self.edges, &self.nodes)?;
        Ok(Subgraph {
            nodes: self.nodes,
            edges: self.edges,
            truncated: self.stats.truncated,
        })
    }

    /// Comme [`Self::into_subgraph`], mais **sans** les deux validations.
    ///
    /// POURQUOI (2026-09-15) : le cache de sous-graphe du viz relit son PROPRE
    /// document (`subgraph_for_stream`, voie WebSocket). Or un sous-graphe
    /// **tronqué** a légitimement des arêtes pendantes — une arête dont l'autre
    /// extrémité est hors de la fenêtre (`limit=50` sur un nœud à 397 arêtes) —
    /// donc la validation stricte refuse un document que nous venons d'écrire.
    /// Mesuré, cache chaud : la voie WS rendait « cache illisible: arête
    /// pendante » au lieu de streamer, et le client retombait sur HTTP au bout de
    /// son délai. Le **schéma** reste vérifié : c'est lui qui protège d'un
    /// document étranger, et il suffit.
    pub fn into_subgraph_lenient(self) -> VizResult<Subgraph> {
        if self.schema != JSON_GRAPH_SCHEMA_V1 {
            return Err(crate::VizError::Validation(format!(
                "schéma inconnu: {:?} (attendu {JSON_GRAPH_SCHEMA_V1})",
                self.schema
            )));
        }
        Ok(Subgraph {
            nodes: self.nodes,
            edges: self.edges,
            truncated: self.stats.truncated,
        })
    }
}

/// Document json-graph v1 depuis un sous-graphe (sans horodatage).
#[must_use]
pub fn doc_from_subgraph(subgraph: &Subgraph) -> JsonGraphDoc {
    JsonGraphDoc::from_subgraph(subgraph)
}

/// Sérialise un sous-graphe en json-graph v1 (pretty, 2 espaces — stable).
/// Le sous-graphe est canonicalisé au préalable : sortie déterministe
/// indépendante de l'ordre d'extraction.
pub fn to_json_string(subgraph: &Subgraph) -> VizResult<String> {
    let canon = subgraph.clone().canonical();
    Ok(serde_json::to_string_pretty(&JsonGraphDoc::from_subgraph(
        &canon,
    ))?)
}

/// Variante COMPACTE (sans indentation) : meme contenu que [`to_json_string`],
/// octet deterministe (entree canonicalisee), mais ~35 % plus petite.
///
/// C'est la forme servie par l'endpoint HTTP viz (NATIF-J7, allegement du
/// payload) : `JSON.parse` ignore l'espace, donc le rendu du dashboard est
/// strictement identique -- seule la taille NON compressee baisse.
pub fn to_json_string_compact(subgraph: &Subgraph) -> VizResult<String> {
    let canon = subgraph.clone().canonical();
    Ok(serde_json::to_string(&JsonGraphDoc::from_subgraph(&canon))?)
}

/// Reconstruit un sous-graphe depuis une chaîne json-graph v1 (schéma + bien-formé
/// vérifiés). Erreur si le document n'est pas un json-graph v1 valide.
pub fn from_json_str(input: &str) -> VizResult<Subgraph> {
    let doc: JsonGraphDoc = serde_json::from_str(input)?;
    doc.into_subgraph()
}

/// Reconstruit un sous-graphe depuis un json-graph v1 **sans** la validation
/// « bien-formé » (schéma vérifié, arêtes pendantes tolérées).
///
/// Réservé aux documents que NOUS avons produits — le cache du viz, dont les
/// sous-graphes tronqués ont des arêtes pendantes légitimes. Voir
/// [`JsonGraphDoc::into_subgraph_lenient`].
pub fn from_json_str_lenient(input: &str) -> VizResult<Subgraph> {
    let doc: JsonGraphDoc = serde_json::from_str(input)?;
    doc.into_subgraph_lenient()
}

#[cfg(test)]
mod tests {
    use super::{JSON_GRAPH_SCHEMA_V1, from_json_str, to_json_string, to_json_string_compact};
    use crate::ir::{Subgraph, VizEdge, VizNode};

    fn sample_shuffled() -> Subgraph {
        Subgraph {
            nodes: vec![
                VizNode::new("fn:hub"),
                VizNode::new("fn:parse"),
                VizNode::new("lib"),
            ],
            edges: vec![
                VizEdge::new("fn:parse", "fn:hub", "CALLS"),
                VizEdge::new("fn:hub", "lib", "CALLS"),
            ],
            truncated: false,
        }
    }

    #[test]
    fn schema_constant_is_v1() {
        assert_eq!(JSON_GRAPH_SCHEMA_V1, "obrain/json-graph@1");
    }

    #[test]
    fn roundtrip_preserves_content_and_schema() {
        let original = sample_shuffled().canonical();
        let json = to_json_string(&original).unwrap();
        assert!(
            json.contains("\"schema\": \"obrain/json-graph@1\""),
            "{json}"
        );
        let back = from_json_str(&json).unwrap();
        assert_eq!(back, original);
        assert!(!back.truncated);
    }

    #[test]
    fn output_is_byte_deterministic_regardless_of_input_order() {
        let shuffled = sample_shuffled();
        let canonical = shuffled.clone().canonical();
        let a = to_json_string(&shuffled.clone().canonical()).unwrap();
        let b = to_json_string(&canonical).unwrap();
        assert_eq!(a, b);
        // Et le même contenu deux fois de suite → mêmes octets.
        assert_eq!(a, to_json_string(&shuffled.clone().canonical()).unwrap());
    }

    #[test]
    fn stats_header_matches_content() {
        let json = to_json_string(&sample_shuffled()).unwrap();
        assert!(json.contains("\"node_count\": 3"), "{json}");
        assert!(json.contains("\"edge_count\": 2"), "{json}");
        assert!(json.contains("\"truncated\": false"), "{json}");
    }

    #[test]
    fn truncated_flag_is_round_tripped() {
        let mut s = sample_shuffled();
        s.truncated = true;
        let back = from_json_str(&to_json_string(&s).unwrap()).unwrap();
        assert!(back.truncated);
    }

    #[test]
    fn unknown_schema_is_rejected() {
        let json = to_json_string(&sample_shuffled()).unwrap();
        let bad = json.replace(JSON_GRAPH_SCHEMA_V1, "obrain/json-graph@0");
        let err = from_json_str(&bad).unwrap_err().to_string();
        assert!(err.contains("schéma inconnu"), "{err}");
    }

    #[test]
    fn dangling_edge_rejected_on_import() {
        let mut s = sample_shuffled();
        s.edges.push(VizEdge::new("fn:parse", "ghost", "CALLS"));
        let json = to_json_string(&s).unwrap();
        assert!(from_json_str(&json).is_err());
    }

    #[test]
    fn compact_has_no_pretty_whitespace_and_roundtrips() {
        let original = sample_shuffled().canonical();
        let pretty = to_json_string(&original).unwrap();
        let compact = to_json_string_compact(&original).unwrap();
        // Compact : aucune indentation ni saut de ligne.
        assert!(!compact.contains('\n'), "{compact}");
        assert!(!compact.contains("\": "), "{compact}");
        // Strictement plus court que la forme pretty.
        assert!(compact.len() < pretty.len());
        // Meme contenu : reimport identique, et meme schema.
        assert!(compact.contains(JSON_GRAPH_SCHEMA_V1));
        let back = from_json_str(&compact).unwrap();
        assert_eq!(back, original);
        // Deterministe : deux appels donnent les memes octets.
        assert_eq!(compact, to_json_string_compact(&original).unwrap());
    }
}
