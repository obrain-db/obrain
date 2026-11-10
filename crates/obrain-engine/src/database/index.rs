//! Index management for ObrainDB (property, vector, and text indexes).

#[cfg(any(feature = "vector-index", feature = "text-index"))]
use std::sync::Arc;

#[cfg(feature = "text-index")]
use parking_lot::RwLock;

use obrain_common::utils::error::Result;

/// (O-VEC C4) Graine de la projection apprise (PCA par iteration de sous-espace,
/// `vec_projection`) : fixe, pour qu'un `rebuild` sur les memes vecteurs rende
/// les memes paliers au bit pres.
const GRAINE_PROJECTION: u64 = 0x0B2A_1A5E_ED00_0001;

impl super::ObrainDB {
    // =========================================================================
    // PROPERTY INDEX API
    // =========================================================================

    /// Creates an index on a node property for O(1) lookups by value.
    ///
    /// After creating an index, calls to [`Self::find_nodes_by_property`] will be
    /// O(1) instead of O(n) for this property. The index is automatically
    /// maintained when properties are set or removed.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use obrain_engine::ObrainDB;
    /// # use obrain_common::types::Value;
    /// # let db = ObrainDB::new_in_memory();
    /// // Create an index on the 'email' property
    /// db.create_property_index("email");
    ///
    /// // Now lookups by email are O(1)
    /// let nodes = db.find_nodes_by_property("email", &Value::from("alix@example.com"));
    /// ```
    pub fn create_property_index(&self, property: &str) {
        // T17b (2026-08-05) : le substrat porte désormais son propre index
        // de propriété natif (`SubstrateStore::create_property_index`) —
        // l'ancien no-op explicite de la slice T17 W3c 5d est remplacé par
        // un routage vers `data_store()`, le même chemin que les lectures.
        if self.substrate_store.is_some() {
            self.data_store().create_property_index(property);
            return;
        }
        self.store.create_property_index(property);
    }

    /// (NS-PARTOUT) Rebâtit depuis les chaînes, puis PERSISTE, les index
    /// de toutes les propriétés déclarées au catalogue substrate — l'acte
    /// de maintenance qui donne ses séries à une zone d'index introduite
    /// APRÈS l'indexation du store (la zone trigrammes sur un store déjà
    /// couvert par l'égalité : sous le régime couvert, la fusion ne peut
    /// pas dériver les trigrammes, les valeurs ne sont plus en RAM).
    ///
    /// Rend le nombre de propriétés rebâties — `0` hors substrate ou sans
    /// déclaration. Pendant la passe, les requêtes concernées balaient :
    /// plus lentes, jamais fausses.
    pub fn rebuild_property_indexes(&self) -> usize {
        match self.substrate_store.as_ref() {
            Some(s) => s.rebatir_index_declares(),
            None => 0,
        }
    }

    /// (NATIF-J6) Rebatit et persiste l'index ORDONNE PAR VALEUR des
    /// proprietes donnees, depuis les seules chaines. C'est ce qui rend
    /// `ORDER BY n.<prop> LIMIT k`, `min` et `max` instantanes (O(k) / O(1)).
    /// Rend le nombre de proprietes indexees ; `0` hors substrate.
    pub fn rebuild_ordered_value_index(&self, proprietes: &[String]) -> usize {
        match self.substrate_store.as_ref() {
            Some(s) => s.rebatir_ord_valeur(proprietes),
            None => 0,
        }
    }

    /// Drops an index on a node property.
    ///
    /// Returns `true` if the index existed and was removed.
    pub fn drop_property_index(&self, property: &str) -> bool {
        if self.substrate_store.is_some() {
            // T17b : routé vers l'index natif substrate.
            return self.data_store().drop_property_index(property);
        }
        self.store.drop_property_index(property)
    }

    /// Returns `true` if the property has an index.
    #[must_use]
    pub fn has_property_index(&self, property: &str) -> bool {
        if self.substrate_store.is_some() {
            // T17b : routé vers l'index natif substrate.
            return self.data_store().has_property_index(property);
        }
        self.store.has_property_index(property)
    }

    /// Finds all nodes that have a specific property value.
    ///
    /// If the property is indexed, this is O(1). Otherwise, it scans all nodes
    /// which is O(n). Use [`Self::create_property_index`] for frequently queried properties.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use obrain_engine::ObrainDB;
    /// # use obrain_common::types::Value;
    /// # let db = ObrainDB::new_in_memory();
    /// // Create index for fast lookups (optional but recommended)
    /// db.create_property_index("city");
    ///
    /// // Find all nodes where city = "NYC"
    /// let nyc_nodes = db.find_nodes_by_property("city", &Value::from("NYC"));
    /// ```
    #[must_use]
    pub fn find_nodes_by_property(
        &self,
        property: &str,
        value: &obrain_common::types::Value,
    ) -> Vec<obrain_common::types::NodeId> {
        self.data_store().find_nodes_by_property(property, value)
    }

    // =========================================================================
    // VECTOR INDEX API
    // =========================================================================

    /// Creates a vector similarity index on a node property.
    ///
    /// This enables efficient approximate nearest-neighbor search on vector
    /// properties. Currently validates the index parameters and scans existing
    /// nodes to verify the property contains vectors of the expected dimensions.
    ///
    /// # Arguments
    ///
    /// * `label` - Node label to index (e.g., `"Doc"`)
    /// * `property` - Property containing vector embeddings (e.g., `"embedding"`)
    /// * `dimensions` - Expected vector dimensions (inferred from data if `None`)
    /// * `metric` - Distance metric: `"cosine"` (default), `"euclidean"`, `"dot_product"`, `"manhattan"`
    /// * `m` - HNSW links per node (default: 16). Higher = better recall, more memory.
    /// * `ef_construction` - Construction beam width (default: 128). Higher = better index quality, slower build.
    ///
    /// # Errors
    ///
    /// Returns an error if the metric is invalid, no vectors are found, or
    /// dimensions don't match.
    pub fn create_vector_index(
        &self,
        label: &str,
        property: &str,
        dimensions: Option<usize>,
        metric: Option<&str>,
        m: Option<usize>,
        ef_construction: Option<usize>,
    ) -> Result<()> {
        use obrain_common::types::{PropertyKey, Value};
        use obrain_core::index::vector::DistanceMetric;

        // T17 W3c slice 5d: the vector-index registry is LpgStore-inherent.
        // Substrate mode exposes retrieval via SubstrateTieredIndex
        // (see `ObrainDB::substrate_handle()`) and does not consult this
        // registry — creating an index on the dummy store would silently
        // succeed yet never be read. Refuse loudly for symmetry with
        // `vector_search()` (gated in slice 5c).
        // (2026-08-13, T17b) LE REFUS DEVIENT UNE CONSTRUCTION.
        //
        // Ce bloc rendait une erreur « LpgStore-only ... tracked as T17b ».
        // T17b, c'est ici. Le substrat sait deja tout faire — `SubstrateTieredIndex`
        // implemente le contrat `VectorIndex`, se persiste en zones et se
        // recharge — il ne lui manquait qu'un registre et deux branchements.
        //
        // On lit les vecteurs par le chemin PUBLIC des proprietes
        // (`nodes_by_label` + `get_node_property`), donc sans toucher au
        // `VecColumnRegistry` interne : le routage `Value::Vector` vers les
        // colonnes est deja fait a l'ecriture.
        if self.substrate_store.is_some() {
            let _ = (metric, m, ef_construction);
            let prop_key = PropertyKey::new(property);
            let data = self.data_store();
            let nodes = data.nodes_by_label(label);

            // Dimension : celle annoncee, sinon celle du premier vecteur trouve.
            let mut pairs: Vec<(u32, Vec<f32>)> = Vec::new();
            let mut dim = dimensions.unwrap_or(0);
            for &node_id in &nodes {
                if let Some(Value::Vector(v)) = data.get_node_property(node_id, &prop_key) {
                    if dim == 0 {
                        dim = v.len();
                    }
                    if v.len() == dim {
                        pairs.push((node_id.0 as u32, v.to_vec()));
                    }
                    // Un vecteur de dimension incoherente est IGNORE, pas fatal :
                    // une base peut porter des embeddings de plusieurs modeles sur
                    // la meme propriete. Les compter serait mentir sur l'index.
                }
            }

            if dim == 0 {
                return Err(obrain_common::utils::error::Error::Internal(format!(
                    "create_vector_index(:{label}({property})) : aucun vecteur trouve et \
                     aucune dimension fournie — impossible de deviner la dimension. \
                     Passe `dimensions`, ou ecris au moins un vecteur d'abord."
                )));
            }

            // ── DIMENSION NATIVE (O-VEC C4) ─────────────────────────────────
            // Les paliers travaillent en L2_DIM (384) ; l'index porte la
            // PROJECTION qui l'y amene : identite + rembourrage pour dim <= 384
            // (cosinus preserve au bit pres), PCA APPRISE sur un echantillon
            // (<= ECHANTILLON_MAX vecteurs, graine fixe) au-dela — jamais
            // re-ajustee hors `rebuild_vector_index`, puisque les paliers
            // n'ont de sens qu'avec elle. Les vecteurs natifs restent la source
            // de verite : le rerank exact des candidats les relit dans la
            // colonne (`source_native_vectorielle`). MESURE (spike cascade-768,
            // 10/11/2026) : recall@10 0,986 sur les 32 521 vecteurs reels de
            // PO, 0,997 a 1 M avec k0 = 4 000.
            let projection = if dim <= obrain_substrate::L2_DIM {
                obrain_substrate::vec_projection::Projection::identite(dim)
            } else {
                if pairs.len() < 2 {
                    return Err(obrain_common::utils::error::Error::Internal(format!(
                        "create_vector_index(:{label}({property})) : {} vecteur(s) en {dim} \
                         dimensions ; apprendre la projection vers {} exige au moins 2 vecteurs.",
                        pairs.len(),
                        obrain_substrate::L2_DIM
                    )));
                }
                let n_ech = pairs
                    .len()
                    .min(obrain_substrate::vec_projection::ECHANTILLON_MAX);
                let mut echantillon = Vec::with_capacity(n_ech * dim);
                for (_, v) in pairs.iter().take(n_ech) {
                    echantillon.extend_from_slice(v);
                }
                obrain_substrate::vec_projection::ProjectionApprise::ajuster(
                    &echantillon,
                    dim,
                    GRAINE_PROJECTION,
                )
                .map(obrain_substrate::vec_projection::Projection::Apprise)
            }
            .map_err(|e| {
                obrain_common::utils::error::Error::Internal(format!(
                    "create_vector_index(:{label}({property})) : projection {dim} -> {} \
                     impossible : {e}",
                    obrain_substrate::L2_DIM
                ))
            })?;
            // `metric` : le cascade classe par cosinus (sur des vecteurs normalises,
            // l'ordre euclidien est le meme) ; `m` / `ef_construction` sont
            // propres a HNSW : acceptes, sans effet (API conservee).

            // Le trait doit etre en portee pour que `rebuild` soit visible.
            use obrain_core::index::vector::VectorIndex as _;
            let index = obrain_substrate::SubstrateTieredIndex::avec_projection(projection);
            index.rebuild(&pairs);
            index.definir_source_native(self.source_native_vectorielle(property));
            // (O-VEC C3) Lie a l'epoch de repermutation courant : l'index vient
            // d'etre bati depuis les donnees VIVANTES, donc APRES toute
            // repermutation ; sans cette liaison, la persistance refuserait
            // d'ecrire sur une base deja repermutee.
            let index = match self.substrate_store.as_ref() {
                Some(sub) => sub
                    .avec_substrate(|s| index.lier_a_l_epoch_courant(s))
                    .map_err(|e| {
                        obrain_common::utils::error::Error::Internal(format!(
                            "create_vector_index(:{label}({property})) : epoch de remap illisible : {e}"
                        ))
                    })?,
                None => index,
            };
            let index = std::sync::Arc::new(index);
            let key = format!("{label}:{property}");
            self.substrate_vector_indexes
                .write()
                .insert(key, (dim, std::sync::Arc::clone(&index)));
            // (O-VEC C3) Persistance immediate : une base rouverte sans ecriture
            // rechargera ces zones au lieu de tout rebatir.
            #[cfg(feature = "vector-index")]
            self.persister_index_vectoriel(label, property, &index);

            // (2026-08-14) DECLARATION DURABLE. L'index lui-meme est une
            // structure derivee — on sait toujours le rebatir depuis les
            // proprietes. Ce qui ne se rebatit pas, c'est de SAVOIR qu'il doit
            // exister : sans cette ligne, l'index disparaissait a la
            // fermeture, et la recherche suivante repondait « aucun index »
            // sur une base ou l'utilisateur en avait cree un.
            self.declarer_index_substrat(obrain_substrate::DeclarationIndex {
                genre: "vector".to_string(),
                label: label.to_string(),
                propriete: property.to_string(),
                dimension: Some(dim),
                metrique: metric.map(str::to_string),
            });
            return Ok(());
        }

        let metric = match metric {
            Some(m) => DistanceMetric::from_str(m).ok_or_else(|| {
                obrain_common::utils::error::Error::Internal(format!(
                    "Unknown distance metric '{}'. Use: cosine, euclidean, dot_product, manhattan",
                    m
                ))
            })?,
            None => DistanceMetric::Cosine,
        };

        // Pass 1: count vectors + detect dimensions (no cloning).
        let prop_key = PropertyKey::new(property);
        let mut found_dims: Option<usize> = dimensions;
        let mut vector_count = 0usize;

        for node in self.store.nodes_with_label(label) {
            if let Some(Value::Vector(v)) = node.properties.get(&prop_key) {
                if let Some(expected) = found_dims {
                    if v.len() != expected {
                        return Err(obrain_common::utils::error::Error::Internal(format!(
                            "Vector dimension mismatch: expected {}, found {} on node {}",
                            expected,
                            v.len(),
                            node.id.0
                        )));
                    }
                } else {
                    found_dims = Some(v.len());
                }
                vector_count += 1;
            }
        }

        let Some(dims) = found_dims else {
            // No vectors found yet: caller must have supplied explicit dimensions
            // so we can create an empty index that auto-populates via set_node_property.
            return if let Some(d) = dimensions {
                #[cfg(feature = "vector-index")]
                {
                    use obrain_core::index::vector::{HnswConfig, HnswIndex};

                    let mut config = HnswConfig::new(d, metric);
                    if let Some(m_val) = m {
                        config = config.with_m(m_val);
                    }
                    if let Some(ef_c) = ef_construction {
                        config = config.with_ef_construction(ef_c);
                    }

                    let index = HnswIndex::new(config);
                    self.store
                        .add_vector_index(label, property, Arc::new(index));
                }

                let _ = (m, ef_construction);
                tracing::info!(
                    "Empty vector index created: :{label}({property}) - 0 vectors, {d} dimensions, metric={metric_name}",
                    metric_name = metric.name()
                );
                Ok(())
            } else {
                Err(obrain_common::utils::error::Error::Internal(format!(
                    "No vector properties found on :{label}({property}) and no dimensions specified"
                )))
            };
        };

        // Build and populate the HNSW index
        #[cfg(feature = "vector-index")]
        {
            use obrain_core::index::vector::{HnswConfig, HnswIndex};

            let mut config = HnswConfig::new(dims, metric);
            if let Some(m_val) = m {
                config = config.with_m(m_val);
            }
            if let Some(ef_c) = ef_construction {
                config = config.with_ef_construction(ef_c);
            }

            let index = HnswIndex::with_capacity(config, vector_count);
            let accessor =
                obrain_core::index::vector::PropertyVectorAccessor::new(&*self.store, property);
            // Pass 2: iterate nodes again, insert directly without cloning vectors.
            for node in self.store.nodes_with_label(label) {
                if let Some(Value::Vector(v)) = node.properties.get(&prop_key) {
                    index.insert(node.id, v, &accessor);
                }
            }

            self.store
                .add_vector_index(label, property, Arc::new(index));
        }

        // Suppress unused variable warnings when vector-index is off
        let _ = (m, ef_construction);

        tracing::info!(
            "Vector index created: :{label}({property}) - {vector_count} vectors, {dims} dimensions, metric={metric_name}",
            metric_name = metric.name()
        );

        Ok(())
    }

    /// Drops a vector index for the given label and property.
    ///
    /// Returns `true` if the index existed and was removed, `false` if no
    /// index was found.
    ///
    /// After dropping, [`vector_search`](Self::vector_search) for this
    /// label+property pair will return an error.
    #[cfg(feature = "vector-index")]
    pub fn drop_vector_index(&self, label: &str, property: &str) -> bool {
        // (T17b) Idem : le registre vectoriel du substrat vit sur la base.
        if self.substrate_store.is_some() {
            let retire = self
                .substrate_vector_indexes
                .write()
                .remove(&format!("{label}:{property}"))
                .is_some();
            self.supprimer_zones_index_vectoriel(label, property);
            // La declaration part MEME si l'index n'etait pas en memoire : sur
            // une base rouverte, il ne l'est justement pas encore, et un
            // `DROP` doit valoir pour de bon.
            self.retirer_declaration_substrat("vector", label, property);
            if retire {
                tracing::info!("Index vectoriel (substrat) retire : :{label}({property})");
            }
            return retire;
        }
        let removed = self.store.remove_vector_index(label, property);
        if removed {
            tracing::info!("Vector index dropped: :{label}({property})");
        }
        removed
    }

    /// Drops and recreates a vector index, rescanning all matching nodes.
    ///
    /// This is useful after bulk inserts or when the index may be out of sync.
    /// When the index still exists, the previous configuration (dimensions,
    /// metric, M, ef\_construction) is preserved. When it has already been
    /// dropped, dimensions are inferred from existing data and default
    /// parameters are used.
    ///
    /// # Errors
    ///
    /// Returns an error if the rebuild fails (e.g., no matching vectors found
    /// and no dimensions can be inferred).
    #[cfg(feature = "vector-index")]
    pub fn rebuild_vector_index(&self, label: &str, property: &str) -> Result<()> {
        // (T17b) Reconstruire, c'est retirer puis recreer. La dimension est
        // relue des donnees : c'est justement ce qu'on veut apres une
        // migration d'embeddings.
        if self.substrate_store.is_some() {
            self.substrate_vector_indexes
                .write()
                .remove(&format!("{label}:{property}"));
            return self.create_vector_index(label, property, None, None, None, None);
        }
        // Preserve config from existing index if available
        let config = self
            .store
            .get_vector_index(label, property)
            .map(|idx| idx.config().clone());

        self.store.remove_vector_index(label, property);

        if let Some(config) = config {
            self.create_vector_index(
                label,
                property,
                Some(config.dimensions),
                Some(config.metric.name()),
                Some(config.m),
                Some(config.ef_construction),
            )
        } else {
            // Index was already dropped: infer dimensions from data
            self.create_vector_index(label, property, None, None, None, None)
        }
    }

    // =========================================================================
    // TEXT INDEX API
    // =========================================================================

    /// Creates a BM25 text index on a node property for full-text search.
    ///
    /// Indexes all existing nodes with the given label and property.
    /// The index stays in sync automatically as nodes are created, updated,
    /// or deleted. Use [`rebuild_text_index`](Self::rebuild_text_index) only
    /// if the index was created before existing data was loaded.
    ///
    /// # Errors
    ///
    /// Returns an error if the label has no nodes or the property contains no text values.
    #[cfg(feature = "text-index")]
    pub fn create_text_index(&self, label: &str, property: &str) -> Result<()> {
        use obrain_common::types::{PropertyKey, Value};
        use obrain_core::index::text::{BM25Config, InvertedIndex};

        // (2026-08-13, T17b) LE REFUS DEVIENT UNE CONSTRUCTION.
        //
        // Le message qu'il y avait ici etait FAUX : il renvoyait vers
        // `SubstrateTieredIndex`, qui est un cascade VECTORIEL. Un grep sur
        // tout obrain-substrate ne trouve ni InvertedIndex ni BM25 — le
        // substrat n'a aucun index texte, et n'en a pas besoin : le BM25 de
        // obrain-core se construit tres bien a partir de ses donnees, par le
        // meme parcours que le chemin LpgStore juste en dessous.
        if self.substrate_store.is_some() {
            let mut index = InvertedIndex::new(BM25Config::default());
            let prop_key = PropertyKey::new(property);
            let data = self.data_store();
            // Meme boucle que le chemin legacy : un noeud a la fois, la chaine
            // est relachee apres insertion. Sur un label a 8 M de documents,
            // tout charger d'un coup allouerait des millions de String
            // simultanement.
            for &node_id in &data.nodes_by_label(label) {
                if let Some(Value::String(text)) = data.get_node_property(node_id, &prop_key) {
                    index.insert(node_id, text.as_str());
                }
            }
            let key = format!("{label}:{property}");
            // ── PERSISTER AVANT D'INSTALLER (l'index est déplacé au partage) ──
            // L'export a lieu ici, une fois, pendant qu'on tient encore l'index
            // par valeur : après le `Arc::new`, il appartient au registre et on
            // ne le relirait plus sans verrou.
            self.persister_index_texte(label, property, &index);
            self.substrate_text_indexes
                .write()
                .insert(key, Arc::new(RwLock::new(index)));
            self.declarer_index_substrat(obrain_substrate::DeclarationIndex {
                genre: "text".to_string(),
                label: label.to_string(),
                propriete: property.to_string(),
                dimension: None,
                metrique: None,
            });
            return Ok(());
        }

        let mut index = InvertedIndex::new(BM25Config::default());
        let prop_key = PropertyKey::new(property);

        // Stream through nodes one-by-one instead of batch-fetching all
        // property values at once, which would allocate millions of strings
        // simultaneously for large labels (e.g. 8M+ Document nodes).
        // Each string is dropped after BM25 insert, keeping memory bounded.
        // Iterate nodes via the real backend (substrate in T17 mode, else
        // the LpgStore data store). The text-index registry itself still
        // lives on `self.store` (LpgStore-only API until the substrate BM25
        // rewrite lands — see W3b/W4).
        let data = self.data_store();
        let nodes = data.nodes_by_label(label);
        for &node_id in &nodes {
            if let Some(Value::String(text)) = data.get_node_property(node_id, &prop_key) {
                index.insert(node_id, text.as_str());
            }
        }

        self.store
            .add_text_index(label, property, Arc::new(RwLock::new(index)));
        Ok(())
    }

    /// Drops a text index on a label+property pair.
    ///
    /// Returns `true` if the index existed and was removed.
    #[cfg(feature = "text-index")]
    pub fn drop_text_index(&self, label: &str, property: &str) -> bool {
        // (T17b) Le substrat a desormais SON registre — porte par la base.
        if self.substrate_store.is_some() {
            let retire = self
                .substrate_text_indexes
                .write()
                .remove(&format!("{label}:{property}"))
                .is_some();
            // Comme pour le vectoriel : la declaration part meme si l'index
            // n'etait pas encore en memoire.
            self.retirer_declaration_substrat("text", label, property);
            return retire;
        }
        self.store.remove_text_index(label, property)
    }

    /// Rebuilds a text index by re-scanning all matching nodes.
    ///
    /// Use after bulk property updates to keep the index current.
    ///
    /// # Errors
    ///
    /// Returns an error if no text index exists for this label+property.
    #[cfg(feature = "text-index")]
    pub fn rebuild_text_index(&self, label: &str, property: &str) -> Result<()> {
        // (T17b) Idem cote texte : on jette et on reconstruit depuis les
        // donnees vivantes. `create_text_index` ecrase la cle de toute facon,
        // mais le retrait explicite garde la semantique lisible — et laisse la
        // base sans index si la reconstruction echoue, plutot qu'avec un
        // ancien index qu'on croirait a jour.
        if self.substrate_store.is_some() {
            self.substrate_text_indexes
                .write()
                .remove(&format!("{label}:{property}"));
            return self.create_text_index(label, property);
        }
        self.store.remove_text_index(label, property);
        self.create_text_index(label, property)
    }

    /// L'index texte (label, propriete) est-il PRESENT dans le registre ?
    ///
    /// (2026-09-09, P2) Predicat de presence pour le chemin a chaud de
    /// `runtime_search`. « Present » ne dit pas « FRAIS » — la fraicheur est
    /// jugee par l'appelant via une generation (epoch de commit, comparee
    /// entre deux appels du meme process). Mais « absent » dit avec certitude
    /// qu'une recherche echouerait ou declencherait une reconstruction : le
    /// predicate permet de ne reconstruire qu'a bon escient, au lieu de
    /// REBUILD integral a chaque requete (le comportement de
    /// `create_text_index`, volontairement inchange : `import_events` s'en
    /// sert pour rafraichir apres des ecritures GQL qui n'entretiennent pas
    /// l'index incrementalement).
    #[cfg(feature = "text-index")]
    pub fn has_text_index(&self, label: &str, property: &str) -> bool {
        let cle = format!("{label}:{property}");
        if self.substrate_store.is_some() {
            self.substrate_text_indexes.read().contains_key(&cle)
        } else {
            self.store.get_text_index(label, property).is_some()
        }
    }
}

/// Catalogue durable des index — declaration, retrait, reconstruction.
///
/// Ces trois methodes vivent dans leur PROPRE bloc `impl` a dessein. Les
/// glisser au milieu du precedent revenait a s'inserer entre un doc-comment et
/// l'element qu'il documente : l'element perd sa documentation, l'insertion
/// herite d'une doc qui ne la decrit pas, et le seul signal est un
/// « missing documentation » sur une ligne qui semble pourtant en avoir.
impl super::ObrainDB {
    /// Enregistre durablement l'intention « cet index doit exister ».
    ///
    /// (2026-08-14) L'index est une structure DERIVEE : on sait toujours le
    /// rebatir depuis les proprietes. Ce qui ne se rebatit pas, c'est de
    /// SAVOIR qu'il doit exister — et cette intention ne vivait nulle part.
    pub(super) fn declarer_index_substrat(&self, decl: obrain_substrate::DeclarationIndex) {
        let Some(ref sub) = self.substrate_store else {
            return;
        };
        if let Err(e) = sub.declarer_index(decl) {
            tracing::warn!(erreur = %e, "declaration d'index non persistee");
        }
    }

    #[cfg(any(feature = "vector-index", feature = "text-index"))]
    pub(super) fn retirer_declaration_substrat(&self, genre: &str, label: &str, propriete: &str) {
        let Some(ref sub) = self.substrate_store else {
            return;
        };
        if let Err(e) = sub.retirer_declaration_index(genre, label, propriete) {
            tracing::warn!(erreur = %e, "retrait de declaration non persiste");
        }
    }

    /// Rebatit un index DECLARE mais absent du registre — le cas d'une base
    /// qu'on vient de rouvrir.
    ///
    /// PARESSEUX ET NON A L'OUVERTURE, deliberement : construire un cascade en
    /// 384 dimensions sur une base de 151 Go ajouterait au demarrage un cout
    /// non borne, sur un agent que le watchdog redemarre. On paie a la
    /// premiere recherche qui en a besoin, et une seule fois.
    ///
    /// Rend `true` si un index a ete rebati.
    /// (O-VEC C4) Source des vecteurs NATIFS pour le rerank exact d'un index :
    /// relit la colonne par le chemin public des proprietes (`Value::Vector`,
    /// `Arc<[f32]>`), sans copie dans l'index. `None` sans substrat.
    pub(super) fn source_native_vectorielle(
        &self,
        property: &str,
    ) -> Option<std::sync::Arc<dyn obrain_substrate::SourceNative>> {
        self.substrate_store.as_ref()?;
        let data = self.data_store();
        let cle = obrain_common::types::PropertyKey::new(property);
        Some(std::sync::Arc::new(
            move |offset: obrain_substrate::NodeOffset, dest: &mut Vec<f32>| match data
                .get_node_property(obrain_common::types::NodeId(u64::from(offset)), &cle)
            {
                Some(obrain_common::types::Value::Vector(v)) => {
                    dest.extend_from_slice(&v);
                    true
                }
                _ => false,
            },
        ))
    }

    #[cfg(any(feature = "vector-index", feature = "text-index"))]
    pub(super) fn rebatir_index_declare(&self, genre: &str, label: &str, property: &str) -> bool {
        let Some(ref sub) = self.substrate_store else {
            return false;
        };
        let Some(decl) = sub
            .declarations_index()
            .into_iter()
            .find(|d| d.genre == genre && d.label == label && d.propriete == property)
        else {
            return false;
        };
        tracing::info!(
            genre,
            label,
            property,
            "index declare absent du registre — reconstruction a la demande"
        );
        let ok = match genre {
            // (O-VEC C3) D'abord les zones persistees : chargees si elles
            // decrivent encore le store (empreinte, epoch, CRC, projection) ;
            // sinon reconstruction par balayage, puis reecriture.
            #[cfg(feature = "vector-index")]
            "vector" => {
                // `dim` est la dimension NATIVE : l'index porte sa projection
                // (identite <= 384, PCA apprise relue de sa zone au-dela).
                if let Some(dim) = decl.dimension
                    && let Some(index) = self.charger_index_vectoriel(label, property, dim)
                {
                    self.substrate_vector_indexes
                        .write()
                        .insert(format!("{label}:{property}"), (dim, index));
                    true
                } else {
                    let ok = self
                        .create_vector_index(
                            label,
                            property,
                            decl.dimension,
                            decl.metrique.as_deref(),
                            None,
                            None,
                        )
                        .is_ok();
                    if ok {
                        self.index_vectoriel_reconstruits
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    ok
                }
            }
            #[cfg(not(feature = "vector-index"))]
            "vector" => self
                .create_vector_index(
                    label,
                    property,
                    decl.dimension,
                    decl.metrique.as_deref(),
                    None,
                    None,
                )
                .is_ok(),
            #[cfg(feature = "text-index")]
            "text" => self.create_text_index(label, property).is_ok(),
            // Sans la feature `text-index`, la methode n'existe pas. On ne
            // peut pas reconstruire : `false` dit l'echec, la declaration
            // survit, et le journal juste en dessous l'explique.
            #[cfg(not(feature = "text-index"))]
            "text" => false,
            _ => false,
        };
        if !ok {
            // La declaration survit a un echec de reconstruction : les donnees
            // peuvent etre revenues demain (propriete pas encore reecrite,
            // dimension momentanement incoherente). Effacer l'intention parce
            // qu'une tentative a echoue serait perdre l'information la plus
            // durable au profit de la plus circonstancielle.
            tracing::warn!(
                genre,
                label,
                property,
                "reconstruction de l'index declare echouee"
            );
        }
        ok
    }
}
