//! Liaison navigateur d'obrain : une base EN MÉMOIRE, interrogée par texte.
//!
//! Paquet d'essai : pas de persistance (le navigateur n'a pas encore de backend
//! durable fusionné), pas de publication de version. Les résultats sont rendus en JSON.

use obrain::{ObrainDB, Value};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct Base {
    db: ObrainDB,
}

#[wasm_bindgen]
impl Base {
    /// Crée une base vide, en mémoire.
    #[wasm_bindgen(constructor)]
    pub fn nouvelle() -> Base {
        Base { db: ObrainDB::new_in_memory() }
    }

    /// Exécute une requête GQL. Rend `{"colonnes":[...],"lignes":[[...]]}` en JSON,
    /// ou lève une erreur JS avec le message du moteur.
    pub fn executer(&self, requete: &str) -> Result<String, JsValue> {
        let resultat = self
            .db
            .session()
            .execute(requete)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let lignes: Vec<Vec<serde_json::Value>> = resultat
            .rows
            .iter()
            .map(|l| l.iter().map(valeur_json).collect())
            .collect();
        Ok(serde_json::json!({ "colonnes": resultat.columns, "lignes": lignes }).to_string())
    }

    /// Nombre de nœuds.
    pub fn noeuds(&self) -> usize {
        self.db.node_count()
    }

    /// Nombre d'arêtes.
    pub fn aretes(&self) -> usize {
        self.db.edge_count()
    }
}

fn valeur_json(v: &Value) -> serde_json::Value {
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => (*b).into(),
        Value::Int64(n) => (*n).into(),
        Value::Float64(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::String(s) => s.to_string().into(),
        autre => format!("{autre:?}").into(),
    }
}
