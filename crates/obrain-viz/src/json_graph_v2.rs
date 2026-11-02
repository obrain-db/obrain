//! Export json-graph **v2** : arêtes en COLONNES + table de chaînes.
//!
//! ── POURQUOI UNE v2 (2026-09-15) ──────────────────────────────────────────
//!
//! La v1 écrit un OBJET par arête. Sur une charge réelle mesurée
//! (`/api/viz/subgraph?center=…&hops=1`, `runtime-dev`, 500 nœuds /
//! 24 502 arêtes, 2 102 520 octets après le raccourcissement des signaux f32) :
//!
//! | ce que paie la v1                                      | octets  |
//! |--------------------------------------------------------|---------|
//! | noms de clés + guillemets + accolades, PAR ARÊTE         | ~1 000 K|
//! | `type` — répété alors qu'il n'a que **3** valeurs        |   270 K |
//! | `src` — répété alors qu'il n'a qu'**1** valeur           |   196 K |
//! | `dst`, un par arête (incompressible)                     |   196 K |
//! | `ricci` (forme courte f32)                               |   248 K |
//! | `weight`                                                 |    74 K |
//!
//! Un sous-graphe en étoile paie donc ~22 octets de NOMS DE CLÉS par arête pour
//! redire `"src"`, `"dst"`, `"type"`, `"weight"`, `"ricci"`, et réécrit 24 502
//! fois une chaîne `src` qui n'a qu'une valeur.
//!
//! La v2 sépare les COLONNES des VALEURS : les chaînes sont écrites une fois
//! dans `strings`, et chaque arête n'est plus qu'un INDICE. Mesure sur la même
//! requête : **2 102 520 → ~0,78 Mo (−63 %)**. C'est le poste que paie le
//! moteur de rendu à chaque vue (transfert ET `JSON.parse`).
//!
//! ── ADDITIF, ET C'EST DÉLIBÉRÉ ────────────────────────────────────────────
//!
//! La v1 ne bouge pas d'un octet : `JSON_GRAPH_SCHEMA_V1` reste le défaut, et
//! les lecteurs v1 continuent de lire la v1. La v2 n'est servie que sur demande
//! explicite (`?format=json-graph-2`). Un changement de contrat de fil qui
//! casserait le dashboard n'a pas à être imposé : il s'adopte.
//!
//! `generated_at` suit la même règle qu'en v1 : absent par défaut, pour que la
//! sortie reste déterministe à entrée canonique identique.
//!
//! ── CE QUE LA v2 N'ENCODE PAS ENCORE ──────────────────────────────────────
//!
//! `src`/`dst` restent des INDICES DANS `strings`, pas dans `nodes`. Indexer
//! les nœuds (`id` déjà présent dans `nodes[].id`) économiserait la table des
//! `dst` — ~175 Ko de plus — mais une arête peut légitimement pendre hors de la
//! fenêtre (`limit` sur un nœud très connecté : voir
//! [`crate::JsonGraphDoc::into_subgraph_lenient`]), et il faudrait alors une
//! table d'externes. Non fait ici : le gain ne vaut pas le risque tant que la
//! v2 n'est pas adoptée côté client.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::ir::{Stats, Subgraph, VizEdge, VizNode};
use crate::{VizError, VizResult};

/// Identifiant de schéma de la v2.
pub const JSON_GRAPH_SCHEMA_V2: &str = "obrain/json-graph@2";

/// Arêtes en colonnes. Chaque tableau a EXACTEMENT la longueur du nombre
/// d'arêtes ; `src`, `dst` et `type` portent des indices dans `strings`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeColumns {
    /// Indices dans `strings`.
    pub src: Vec<u32>,
    /// Indices dans `strings`.
    pub dst: Vec<u32>,
    /// Indices dans `strings`.
    #[serde(rename = "type")]
    pub type_id: Vec<u32>,
    /// Colonne OMISE si aucune arête ne porte le champ ; `null` sur une arête
    /// qui ne le porte pas (une colonne creuse reste plus courte qu'un objet
    /// par arête, mais elle est écrite telle quelle, jamais devinée).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<Vec<Option<f64>>>,
    /// Courbure Ricci — `f32`, comme en v1 depuis le 2026-09-15.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ricci: Option<Vec<Option<f32>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale: Option<Vec<Option<bool>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pheromone: Option<Vec<Option<f64>>>,
    /// Indices dans `strings` (couleur `#rrggbb`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_hint: Option<Vec<Option<u32>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_hint: Option<Vec<Option<f64>>>,
}

/// Document json-graph v2 (enveloppe sérialisée).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonGraphDocV2 {
    pub schema: String,
    pub stats: Stats,
    pub nodes: Vec<VizNode>,
    /// Table des chaînes : chaque `src`/`dst`/`type`/`color_hint` distinct y
    /// figure UNE fois. Ordre de première rencontre (sortie déterministe).
    pub strings: Vec<String>,
    pub edges: EdgeColumns,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<String>,
}

/// Interne : table de chaînes à insertion stable (indice = ordre de rencontre).
#[derive(Default)]
struct Table {
    index: HashMap<String, u32>,
    valeurs: Vec<String>,
}

impl Table {
    fn intern(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.index.get(s) {
            return i;
        }
        let i = self.valeurs.len() as u32;
        self.valeurs.push(s.to_string());
        self.index.insert(s.to_string(), i);
        i
    }

    fn lire(&self, i: u32) -> VizResult<&str> {
        self.valeurs
            .get(i as usize)
            .map(String::as_str)
            .ok_or_else(|| {
                VizError::Validation(format!(
                    "json-graph@2 : indice de chaîne {i} hors de la table ({} entrées)",
                    self.valeurs.len()
                ))
            })
    }
}

/// Colonne optionnelle : `None` si AUCUNE arête ne porte le champ (la colonne
/// disparaît alors du document au lieu d'être écrite en `null` partout).
fn colonne<T: Clone>(valeurs: &[Option<T>]) -> Option<Vec<Option<T>>> {
    valeurs
        .iter()
        .any(Option::is_some)
        .then(|| valeurs.to_vec())
}

impl JsonGraphDocV2 {
    /// Construit le document v2 depuis un sous-graphe canonique.
    #[must_use]
    pub fn from_subgraph(subgraph: &Subgraph) -> Self {
        let mut table = Table::default();
        let n = subgraph.edges.len();
        let mut src = Vec::with_capacity(n);
        let mut dst = Vec::with_capacity(n);
        let mut type_id = Vec::with_capacity(n);
        let mut weight = Vec::with_capacity(n);
        let mut ricci = Vec::with_capacity(n);
        let mut stale = Vec::with_capacity(n);
        let mut pheromone = Vec::with_capacity(n);
        let mut color_hint = Vec::with_capacity(n);
        let mut size_hint = Vec::with_capacity(n);
        for e in &subgraph.edges {
            src.push(table.intern(&e.src));
            dst.push(table.intern(&e.dst));
            type_id.push(table.intern(&e.edge_type));
            weight.push(e.weight);
            // `f32` : la source EST un f32 (voir `crate::ir::f32_court`). Le
            // passer par f64 puis le raccourcir serait un aller-retour pour rien.
            ricci.push(e.ricci.map(|v| v as f32));
            stale.push(e.stale);
            pheromone.push(e.pheromone);
            color_hint.push(e.color_hint.as_deref().map(|c| table.intern(c)));
            size_hint.push(e.size_hint);
        }
        Self {
            schema: JSON_GRAPH_SCHEMA_V2.to_string(),
            stats: subgraph.stats(),
            nodes: subgraph.nodes.clone(),
            strings: table.valeurs,
            edges: EdgeColumns {
                src,
                dst,
                type_id,
                weight: colonne(&weight),
                ricci: colonne(&ricci),
                stale: colonne(&stale),
                pheromone: colonne(&pheromone),
                color_hint: colonne(&color_hint),
                size_hint: colonne(&size_hint),
            },
            generated_at: None,
        }
    }

    /// Reconstitue le sous-graphe. Le schéma est vérifié (c'est lui qui protège
    /// d'un document étranger) ; la validation « bien-formé » est laissée à
    /// l'appelant, comme en v1 (`into_subgraph` vs `into_subgraph_lenient`).
    pub fn into_subgraph(self) -> VizResult<Subgraph> {
        if self.schema != JSON_GRAPH_SCHEMA_V2 {
            return Err(VizError::Validation(format!(
                "schéma inconnu: {:?} (attendu {JSON_GRAPH_SCHEMA_V2})",
                self.schema
            )));
        }
        let table = Table {
            index: HashMap::new(),
            valeurs: self.strings,
        };
        let n = self.edges.src.len();
        // LONGUEURS : une colonne plus courte que les autres produirait un
        // sous-graphe silencieusement tronqué — on refuse.
        for (nom, len) in [
            ("dst", self.edges.dst.len()),
            ("type", self.edges.type_id.len()),
        ] {
            if len != n {
                return Err(VizError::Validation(format!(
                    "json-graph@2 : colonne {nom} de longueur {len}, attendu {n} (src)"
                )));
            }
        }
        let colonne_len = |nom: &str, len: usize| -> VizResult<()> {
            if len == n {
                Ok(())
            } else {
                Err(VizError::Validation(format!(
                    "json-graph@2 : colonne {nom} de longueur {len}, attendu {n}"
                )))
            }
        };
        for (nom, len) in [
            ("weight", self.edges.weight.as_ref().map(Vec::len)),
            ("ricci", self.edges.ricci.as_ref().map(Vec::len)),
            ("stale", self.edges.stale.as_ref().map(Vec::len)),
            ("pheromone", self.edges.pheromone.as_ref().map(Vec::len)),
            ("color_hint", self.edges.color_hint.as_ref().map(Vec::len)),
            ("size_hint", self.edges.size_hint.as_ref().map(Vec::len)),
        ] {
            if let Some(l) = len {
                colonne_len(nom, l)?;
            }
        }
        let mut edges = Vec::with_capacity(n);
        for i in 0..n {
            let optionnel = |v: &Vec<Option<f64>>| v.get(i).copied().flatten();
            edges.push(VizEdge {
                src: table.lire(self.edges.src[i])?.to_string(),
                dst: table.lire(self.edges.dst[i])?.to_string(),
                edge_type: table.lire(self.edges.type_id[i])?.to_string(),
                weight: self.edges.weight.as_ref().and_then(|c| optionnel(c)),
                ricci: self
                    .edges
                    .ricci
                    .as_ref()
                    .and_then(|c| c.get(i).copied().flatten())
                    .map(f64::from),
                stale: self
                    .edges
                    .stale
                    .as_ref()
                    .and_then(|c| c.get(i).copied().flatten()),
                pheromone: self.edges.pheromone.as_ref().and_then(|c| optionnel(c)),
                color_hint: match self
                    .edges
                    .color_hint
                    .as_ref()
                    .and_then(|c| c.get(i).copied().flatten())
                {
                    Some(idx) => Some(table.lire(idx)?.to_string()),
                    None => None,
                },
                size_hint: self.edges.size_hint.as_ref().and_then(|c| optionnel(c)),
            });
        }
        Ok(Subgraph {
            nodes: self.nodes,
            edges,
            truncated: self.stats.truncated,
        })
    }
}

/// Sérialise un sous-graphe en json-graph v2 COMPACT (sans indentation).
///
/// Canonicalisé d'abord, comme la v1 : sortie déterministe indépendante de
/// l'ordre d'extraction — c'est ce qui permet de mémoïser le corps tel quel.
pub fn to_json_string_v2(subgraph: &Subgraph) -> VizResult<String> {
    let canon = subgraph.clone().canonical();
    Ok(serde_json::to_string(&JsonGraphDocV2::from_subgraph(
        &canon,
    ))?)
}

/// Reconstruit un sous-graphe depuis une chaîne json-graph v2.
pub fn from_json_str_v2(input: &str) -> VizResult<Subgraph> {
    serde_json::from_str::<JsonGraphDocV2>(input)?.into_subgraph()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un sous-graphe en ÉTOILE, avec des `src`/`type` répétés — exactement la
    /// forme réelle qui justifie la v2.
    fn etoile(n: usize) -> Subgraph {
        let mut nodes = vec![VizNode::new("hub")];
        let mut edges = Vec::new();
        for i in 0..n {
            let id = format!("leaf{i}");
            nodes.push(VizNode::new(id.clone()));
            let mut e = VizEdge::new("hub", id, "CALLS");
            e.ricci = Some(-0.8352941274642944_f64);
            e.weight = Some(1.0);
            edges.push(e);
        }
        Subgraph {
            nodes,
            edges,
            truncated: false,
        }
    }

    #[test]
    fn schema_constant_is_v2() {
        assert_eq!(JSON_GRAPH_SCHEMA_V2, "obrain/json-graph@2");
    }

    #[test]
    fn roundtrip_preserves_content_the_same_way_v1_does() {
        let original = etoile(64).canonical();
        let json = to_json_string_v2(&original).unwrap();
        assert!(json.contains(JSON_GRAPH_SCHEMA_V2), "{json}");
        let back = from_json_str_v2(&json).unwrap();
        assert_eq!(
            back, original,
            "la v2 doit rendre le MEME sous-graphe que la v1"
        );
    }

    #[test]
    fn v1_et_v2_restaurent_l_original_a_l_identique() {
        // ── CETTE ASSERTION A ÉTÉ INVERSÉE LE 2026-09-15, ET VOICI POURQUOI ──
        //
        // Elle affirmait `assert_ne!(par_v1, original)` : la v1 écrivait `ricci`
        // en forme courte f32 SANS relire en f32, donc relire son propre corps
        // rendait `-0.8352941` au lieu de `-0.8352941274642944` — un troisième
        // nombre, ni l'original ni sa forme élargie. Le commentaire d'origine
        // classait la conséquence « sans effet sur le rendu (même f32) », en
        // notant que la voie WebSocket (`subgraph_for_stream` → `from_json_str`)
        // servait alors des f64 différents de la voie HTTP pour la MÊME requête.
        //
        // Le défaut a été corrigé à la racine : la v1 relit désormais en f32
        // (`f32_depuis_json`), donc son aller-retour est EXACT lui aussi. La
        // divergence HTTP/WebSocket n'existe plus — elle n'était pas seulement
        // cosmétique : `compute_delta` compare deux `Subgraph` par ÉGALITÉ, et
        // deux valeurs f64 différentes faisaient annoncer des changements
        // inexistants au client.
        //
        // Ce que ce test verrouille maintenant : les DEUX formats restaurent
        // l'IR à l'identique, et la v2 reste celle qui écrit le moins d'octets
        // (voir `v2_is_much_smaller_than_v1_on_a_star_graph`).
        let original = etoile(8).canonical();
        let par_v1 =
            crate::from_json_str(&crate::to_json_string_compact(&original).unwrap()).unwrap();
        let par_v2 = from_json_str_v2(&to_json_string_v2(&original).unwrap()).unwrap();
        assert_eq!(par_v2, original, "la v2 doit restaurer l'IR à l'identique");
        assert_eq!(
            par_v1, original,
            "la v1 relit en f32 : elle doit restaurer l'IR à l'identique, elle aussi"
        );
        for (a, b) in par_v1.edges.iter().zip(par_v2.edges.iter()) {
            assert_eq!(a.ricci.map(|v| v as f32), b.ricci.map(|v| v as f32));
        }
    }

    #[test]
    fn v2_is_much_smaller_than_v1_on_a_star_graph() {
        let s = etoile(400).canonical();
        let v1 = crate::to_json_string_compact(&s).unwrap().len();
        let v2 = to_json_string_v2(&s).unwrap().len();
        // Le RATIO dépend des données (longueur des ids, valeurs) : ~1,9× sur ce
        // graphe synthétique, ~2,7× sur la charge réelle mesurée. On verrouille
        // le sens de l'inégalité et une borne prudente, jamais un chiffre qui
        // dépend de l'échantillon.
        assert!(v2 < v1, "v1={v1} v2={v2}");
        assert!(
            v2 * 10 < v1 * 6,
            "la v2 doit rendre au moins 40 % de moins : v1={v1} v2={v2}"
        );
    }

    #[test]
    fn strings_are_written_once() {
        let s = etoile(400).canonical();
        let doc = JsonGraphDocV2::from_subgraph(&s);
        // "hub" et "CALLS" n'apparaissent qu'une fois dans la table, quelle que
        // soit le nombre d'aretes qui les referencent.
        assert_eq!(
            doc.strings.iter().filter(|x| x.as_str() == "hub").count(),
            1
        );
        assert_eq!(
            doc.strings.iter().filter(|x| x.as_str() == "CALLS").count(),
            1
        );
        assert!(
            doc.strings.len() < 410,
            "table = {} entrées",
            doc.strings.len()
        );
    }

    #[test]
    fn optional_columns_are_omitted_when_no_edge_carries_them() {
        let s = etoile(4).canonical();
        let doc = JsonGraphDocV2::from_subgraph(&s);
        assert!(doc.edges.weight.is_some());
        assert!(doc.edges.ricci.is_some());
        assert!(doc.edges.stale.is_none(), "aucune arete n'a de stale");
        assert!(doc.edges.pheromone.is_none());
        assert!(doc.edges.color_hint.is_none());
        assert!(doc.edges.size_hint.is_none());
    }

    #[test]
    fn a_hole_in_an_optional_column_stays_a_hole() {
        // Une arete SANS ricci ne doit pas hériter de la valeur de sa voisine
        // (le piège classique des encodages colonnaires).
        let mut s = etoile(3).canonical();
        s.edges[1].ricci = None;
        let back = from_json_str_v2(&to_json_string_v2(&s).unwrap()).unwrap();
        assert_eq!(back.edges[0].ricci, s.edges[0].ricci);
        assert_eq!(back.edges[1].ricci, None);
        assert_eq!(back.edges[2].ricci, s.edges[2].ricci);
    }

    #[test]
    fn output_is_byte_deterministic_regardless_of_input_order() {
        let a = etoile(16);
        let b = etoile(16).canonical();
        assert_eq!(
            to_json_string_v2(&a).unwrap(),
            to_json_string_v2(&b).unwrap()
        );
    }

    #[test]
    fn unknown_schema_is_rejected() {
        let json = to_json_string_v2(&etoile(2)).unwrap();
        let bad = json.replace(JSON_GRAPH_SCHEMA_V2, "obrain/json-graph@9");
        assert!(
            from_json_str_v2(&bad)
                .unwrap_err()
                .to_string()
                .contains("schéma inconnu")
        );
    }

    #[test]
    fn a_ragged_column_is_refused_not_silently_truncated() {
        let mut s = etoile(4).canonical();
        let doc = JsonGraphDocV2::from_subgraph(&s);
        let mut ragged = serde_json::to_value(doc).unwrap();
        ragged["edges"]["dst"].as_array_mut().unwrap().pop();
        let err = from_json_str_v2(&ragged.to_string())
            .unwrap_err()
            .to_string();
        assert!(err.contains("colonne dst"), "{err}");
        // et une colonne optionnelle courte est refusée aussi
        let doc2 = JsonGraphDocV2::from_subgraph(&s);
        let mut ragged2 = serde_json::to_value(doc2).unwrap();
        ragged2["edges"]["ricci"].as_array_mut().unwrap().pop();
        let err2 = from_json_str_v2(&ragged2.to_string())
            .unwrap_err()
            .to_string();
        assert!(err2.contains("colonne ricci"), "{err2}");
        s.truncated = true;
        assert!(JsonGraphDocV2::from_subgraph(&s).stats.truncated);
    }
}
