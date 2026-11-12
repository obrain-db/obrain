//! Liaison navigateur d'obrain : une base interrogée par texte, en mémoire
//! (`new Base()`) ou persistée sur OPFS (`Base.ouvrir(nom)` + `persister()`, instantané).
//!
//! Paquet d'essai : pas de publication de version. Les résultats sont rendus en JSON.
//!
//! Chaque méthode qui peut échouer rend une erreur JavaScript (jamais une panique :
//! une panique wasm abattrait toute l'instance de la page).

use std::collections::HashMap;

use obrain::{ObrainDB, Value};
use wasm_bindgen::prelude::*;

fn err(e: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}

/// Un JSON vers une valeur obrain. Les tableaux deviennent des listes ; les
/// objets, des cartes de valeurs.
fn json_vers_valeur(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int64(i)
            } else {
                Value::Float64(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        serde_json::Value::String(s) => Value::from(s.as_str()),
        serde_json::Value::Array(a) => {
            Value::List(a.iter().map(json_vers_valeur).collect::<Vec<_>>().into())
        }
        serde_json::Value::Object(_) => Value::from(v.to_string()),
    }
}

/// Une valeur obrain vers un JSON.
fn valeur_json(v: &Value) -> serde_json::Value {
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => (*b).into(),
        Value::Int64(n) => (*n).into(),
        Value::Float64(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::String(s) => s.to_string().into(),
        Value::List(l) => l.iter().map(valeur_json).collect::<Vec<_>>().into(),
        Value::Vector(v) => v.iter().map(|x| f64::from(*x)).collect::<Vec<_>>().into(),
        autre => format!("{autre:?}").into(),
    }
}

fn parametres(json: &str) -> Result<HashMap<String, Value>, JsValue> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(err)?;
    let serde_json::Value::Object(o) = v else {
        return Err(err("les paramètres doivent être un objet JSON"));
    };
    Ok(o.iter()
        .map(|(k, v)| (k.clone(), json_vers_valeur(v)))
        .collect())
}

fn resultat_json(colonnes: &[String], rows: &[Vec<Value>]) -> String {
    let lignes: Vec<Vec<serde_json::Value>> = rows
        .iter()
        .map(|l| l.iter().map(valeur_json).collect())
        .collect();
    serde_json::json!({ "colonnes": colonnes, "lignes": lignes }).to_string()
}

/// Une base obrain en mémoire.
#[wasm_bindgen]
pub struct Base {
    db: ObrainDB,
}

#[wasm_bindgen]
impl Base {
    /// Crée une base vide, en mémoire.
    #[wasm_bindgen(constructor)]
    pub fn nouvelle() -> Base {
        Base {
            db: ObrainDB::new_in_memory(),
        }
    }

    /// Ouvre la base persistante `nom` (journal OPFS, créée si absente). Rend une
    /// Promise. À appeler depuis un worker dédié : OPFS n'y expose son accès
    /// synchrone que dans ce contexte. Hors worker, ou sans OPFS (node), la Promise
    /// est rejetée par une erreur lisible, jamais par une trappe.
    #[cfg(target_arch = "wasm32")]
    pub async fn ouvrir(nom: String) -> Result<Base, JsValue> {
        ObrainDB::open_browser(&nom)
            .await
            .map(|db| Base { db })
            .map_err(err)
    }

    /// Publie l'état courant dans le journal OPFS. Instantané : une écriture faite
    /// depuis la dernière publication est perdue à une coupure.
    #[cfg(target_arch = "wasm32")]
    pub fn persister(&self) -> Result<(), JsValue> {
        self.db.persist().map_err(err)
    }

    /// Une session (transactions explicites).
    pub fn session(&self) -> Session {
        Session {
            s: self.db.session(),
        }
    }

    /// Exécute une requête GQL. Rend `{"colonnes":[...],"lignes":[[...]]}` en JSON.
    pub fn executer(&self, requete: &str) -> Result<String, JsValue> {
        self.db
            .session()
            .execute(requete)
            .map(|r| resultat_json(&r.columns, &r.rows))
            .map_err(err)
    }

    /// GQL avec paramètres : `parametres_json` est un objet JSON `{"nom": valeur}`.
    pub fn executer_avec_parametres(
        &self,
        requete: &str,
        parametres_json: &str,
    ) -> Result<String, JsValue> {
        let p = parametres(parametres_json)?;
        self.db
            .session()
            .execute_with_params(requete, p)
            .map(|r| resultat_json(&r.columns, &r.rows))
            .map_err(err)
    }

    /// Exécute une requête dans un autre langage : `gql`, `cypher`, `sparql`,
    /// `gremlin`, `graphql`, `sql` (selon les features compilées).
    pub fn executer_langage(&self, langage: &str, requete: &str) -> Result<String, JsValue> {
        let r = match langage {
            "gql" => self.db.session().execute(requete),
            #[cfg(feature = "cypher")]
            "cypher" => self.db.execute_cypher(requete),
            #[cfg(feature = "sparql")]
            "sparql" => self.db.execute_sparql(requete),
            #[cfg(feature = "gremlin")]
            "gremlin" => self.db.execute_gremlin(requete),
            #[cfg(feature = "graphql")]
            "graphql" => self.db.execute_graphql(requete),
            #[cfg(feature = "sql-pgq")]
            "sql" => self.db.execute_sql(requete),
            autre => {
                return Err(err(format!(
                    "langage « {autre} » non disponible dans ce paquet"
                )));
            }
        };
        r.map(|r| resultat_json(&r.columns, &r.rows)).map_err(err)
    }

    /// Comme `executer_langage`, avec paramètres : `parametres_json` est un objet
    /// JSON `{"nom": valeur}`. Les langages sans paramètres lèvent une erreur.
    pub fn executer_langage_avec_parametres(
        &self,
        langage: &str,
        requete: &str,
        parametres_json: &str,
    ) -> Result<String, JsValue> {
        let p = parametres(parametres_json)?;
        self.db
            .session()
            .execute_language(requete, langage, Some(p))
            .map(|r| resultat_json(&r.columns, &r.rows))
            .map_err(err)
    }

    /// Nombre de nœuds.
    pub fn noeuds(&self) -> usize {
        self.db.node_count()
    }

    /// Nombre d'arêtes.
    pub fn aretes(&self) -> usize {
        self.db.edge_count()
    }

    /// Crée un nœud : étiquettes et propriétés (objet JSON). Rend son identifiant.
    pub fn creer_noeud(
        &self,
        etiquettes: Vec<String>,
        proprietes_json: &str,
    ) -> Result<f64, JsValue> {
        let props = parametres(proprietes_json)?;
        let refs: Vec<&str> = etiquettes.iter().map(String::as_str).collect();
        let id = self.db.create_node(&refs);
        for (k, v) in props {
            self.db.set_node_property(id, &k, v);
        }
        Ok(id.0 as f64)
    }

    /// Pose un vecteur (`Float32Array` côté JavaScript) sur un nœud.
    pub fn definir_vecteur(
        &self,
        noeud: f64,
        propriete: &str,
        vecteur: Vec<f32>,
    ) -> Result<(), JsValue> {
        self.db.set_node_property(
            obrain::NodeId(noeud as u64),
            propriete,
            Value::Vector(vecteur.into()),
        );
        Ok(())
    }

    /// Crée un index plein texte.
    #[cfg(feature = "text-index")]
    pub fn creer_index_texte(&self, etiquette: &str, propriete: &str) -> Result<(), JsValue> {
        self.db.create_text_index(etiquette, propriete).map_err(err)
    }

    /// Retire l'index plein texte. Rend `true` s'il existait.
    #[cfg(feature = "text-index")]
    pub fn supprimer_index_texte(&self, etiquette: &str, propriete: &str) -> bool {
        self.db.drop_text_index(etiquette, propriete)
    }

    /// Reconstruit l'index plein texte depuis les données vivantes. S'il est absent, il est
    /// créé (jamais d'erreur pour ce seul motif).
    #[cfg(feature = "text-index")]
    pub fn reconstruire_index_texte(
        &self,
        etiquette: &str,
        propriete: &str,
    ) -> Result<(), JsValue> {
        self.db
            .rebuild_text_index(etiquette, propriete)
            .map_err(err)
    }

    /// Recherche plein texte : `[{"noeud": id, "score": s}]`.
    #[cfg(feature = "text-index")]
    pub fn rechercher_texte(
        &self,
        etiquette: &str,
        propriete: &str,
        requete: &str,
        k: usize,
    ) -> Result<String, JsValue> {
        let r = self
            .db
            .text_search(etiquette, propriete, requete, k)
            .map_err(err)?;
        Ok(serde_json::Value::Array(
            r.iter()
                .map(|(n, s)| serde_json::json!({ "noeud": n.0 as f64, "score": s }))
                .collect(),
        )
        .to_string())
    }

    /// Crée un index vectoriel (`metrique` : `cosine`, `euclidean`, …).
    #[cfg(feature = "vector-index")]
    pub fn creer_index_vectoriel(
        &self,
        etiquette: &str,
        propriete: &str,
        dimensions: usize,
        metrique: &str,
    ) -> Result<(), JsValue> {
        self.db
            .create_vector_index(
                etiquette,
                propriete,
                Some(dimensions),
                Some(metrique),
                None,
                None,
            )
            .map_err(err)
    }

    /// Retire l'index vectoriel. Rend `true` s'il existait.
    #[cfg(feature = "vector-index")]
    pub fn supprimer_index_vectoriel(&self, etiquette: &str, propriete: &str) -> bool {
        self.db.drop_vector_index(etiquette, propriete)
    }

    /// Reconstruit l'index vectoriel. La dimension est relue des données : utile après
    /// une migration d'embeddings. S'il est absent, il est créé ; erreur si aucun vecteur
    /// n'existe pour en déduire la dimension.
    #[cfg(feature = "vector-index")]
    pub fn reconstruire_index_vectoriel(
        &self,
        etiquette: &str,
        propriete: &str,
    ) -> Result<(), JsValue> {
        self.db
            .rebuild_vector_index(etiquette, propriete)
            .map_err(err)
    }

    /// Voisins les plus proches : `[{"noeud": id, "distance": d}]`.
    #[cfg(feature = "vector-index")]
    pub fn rechercher_vecteur(
        &self,
        etiquette: &str,
        propriete: &str,
        vecteur: Vec<f32>,
        k: usize,
    ) -> Result<String, JsValue> {
        let r = self
            .db
            .vector_search(etiquette, propriete, &vecteur, k, None, None)
            .map_err(err)?;
        Ok(serde_json::Value::Array(
            r.iter()
                .map(|(n, d)| serde_json::json!({ "noeud": n.0 as f64, "distance": d }))
                .collect(),
        )
        .to_string())
    }

    /// Recherche hybride texte + vecteur : `[{"noeud": id, "score": s}]`.
    #[cfg(feature = "hybrid-search")]
    pub fn rechercher_hybride(
        &self,
        etiquette: &str,
        propriete_texte: &str,
        propriete_vecteur: &str,
        texte: &str,
        vecteur: Vec<f32>,
        k: usize,
    ) -> Result<String, JsValue> {
        let v = if vecteur.is_empty() {
            None
        } else {
            Some(vecteur.as_slice())
        };
        let r = self
            .db
            .hybrid_search(
                etiquette,
                propriete_texte,
                propriete_vecteur,
                texte,
                v,
                k,
                None,
            )
            .map_err(err)?;
        Ok(serde_json::Value::Array(
            r.iter()
                .map(|(n, s)| serde_json::json!({ "noeud": n.0 as f64, "score": s }))
                .collect(),
        )
        .to_string())
    }

    /// Schéma observé (étiquettes, types de relations, index) : JSON.
    pub fn schema(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.db.schema()).map_err(err)
    }

    /// Empreinte mémoire détaillée de la base : JSON.
    pub fn memoire(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.db.memory_usage()).map_err(err)
    }

    /// Vide le cache des plans de requête. Sans effet sur les résultats.
    pub fn vider_cache_plans(&self) {
        self.db.clear_plan_cache();
    }

    /// Importe un tableau JSON d'objets comme nœuds d'étiquette `etiquette`. Rend le nombre
    /// de nœuds créés. Le lot entier est validé avant la première écriture : une ligne
    /// invalide n'écrit rien. Les arêtes s'écrivent en GQL (`MATCH ... CREATE`).
    pub fn importer_noeuds(&self, etiquette: &str, lignes_json: &str) -> Result<u32, JsValue> {
        let v: serde_json::Value = serde_json::from_str(lignes_json).map_err(err)?;
        let serde_json::Value::Array(lignes) = v else {
            return Err(err("les lignes doivent être un tableau JSON d'objets"));
        };
        let mut lots = Vec::with_capacity(lignes.len());
        for (i, ligne) in lignes.iter().enumerate() {
            let serde_json::Value::Object(o) = ligne else {
                return Err(err(format!("ligne {i} : objet JSON attendu")));
            };
            lots.push(
                o.iter()
                    .map(|(k, v)| (k.clone(), json_vers_valeur(v)))
                    .collect::<Vec<_>>(),
            );
        }
        let n = lots.len();
        for props in lots {
            let id = self.db.create_node(&[etiquette]);
            for (k, v) in props {
                self.db.set_node_property(id, &k, v);
            }
        }
        u32::try_from(n).map_err(err)
    }

    /// Compteurs de requêtes : `{"requetes": n, "erreurs": n}`.
    #[cfg(feature = "metrics")]
    pub fn metriques(&self) -> String {
        let m = self.db.metrics();
        serde_json::json!({ "requetes": m.query_count, "erreurs": m.query_errors }).to_string()
    }
}

/// Une session : requêtes et transactions explicites.
#[wasm_bindgen]
pub struct Session {
    s: obrain::Session,
}

#[wasm_bindgen]
impl Session {
    pub fn executer(&self, requete: &str) -> Result<String, JsValue> {
        self.s
            .execute(requete)
            .map(|r| resultat_json(&r.columns, &r.rows))
            .map_err(err)
    }

    pub fn commencer(&mut self) -> Result<(), JsValue> {
        self.s.begin_transaction().map_err(err)
    }

    pub fn valider(&mut self) -> Result<(), JsValue> {
        self.s.commit().map_err(err)
    }

    pub fn annuler(&mut self) -> Result<(), JsValue> {
        self.s.rollback().map_err(err)
    }
}
