//! Node and edge CRUD operations for ObrainDB.

#[cfg(feature = "wal")]
use obrain_adapters::storage::wal::WalRecord;
#[cfg(feature = "vector-index")]
use obrain_substrate::VectorIndex as _;

impl super::ObrainDB {
    /// Indexe un noeud dans TOUS les registres du substrat portant ce label.
    ///
    /// (2026-08-13) Quatre chemins d'ecriture — `add_node_label`,
    /// `remove_node_label`, `batch_create_nodes`, `delete_node` — entretenaient
    /// les index par `self.store.get_vector_index()` /
    /// `vector_index_entries()` / `get_text_index()`. En mode substrat,
    /// `self.store` est le store FANTOCHE : ces appels rendent None ou vide, et
    /// l'entretien ne se faisait tout simplement pas. Un index construit restait
    /// fige des qu'on passait par l'un de ces chemins.
    ///
    /// Les deux helpers ci-dessous font le travail contre les VRAIS registres,
    /// une bonne fois, pour que les quatre appelants disent la meme chose.
    #[allow(dead_code)]
    // Sans `vector-index` ni `text-index`, il n'y a aucun index à tenir : `id`, `label` et
    // leurs dérivés ne servent qu'aux blocs gatés ci-dessous.
    #[cfg_attr(
        not(any(feature = "vector-index", feature = "text-index")),
        allow(unused_variables)
    )]
    pub(super) fn substrat_indexer_pour_label(
        &self,
        id: obrain_common::types::NodeId,
        label: &str,
    ) {
        if self.substrate_store.is_none() {
            return;
        }
        let Some(node) = self.data_store().get_node(id) else {
            return;
        };
        let prefixe = format!("{label}:");

        #[cfg(feature = "vector-index")]
        {
            let entrees: Vec<(
                String,
                usize,
                std::sync::Arc<obrain_substrate::SubstrateTieredIndex>,
            )> = self
                .substrate_vector_indexes
                .read()
                .iter()
                .filter(|(cle, _)| cle.starts_with(&prefixe))
                .map(|(cle, (d, i))| (cle.clone(), *d, std::sync::Arc::clone(i)))
                .collect();
            for (cle, dim_native, idx) in entrees {
                let Some(propriete) = cle.strip_prefix(&prefixe) else {
                    continue;
                };
                let pk = obrain_common::types::PropertyKey::new(propriete);
                if let Some(obrain_common::types::Value::Vector(v)) = node.properties.get(&pk)
                    && v.len() == dim_native
                {
                    idx.insert(id.0 as u32, v);
                }
            }
        }

        #[cfg(feature = "text-index")]
        {
            let entrees: Vec<(
                String,
                std::sync::Arc<parking_lot::RwLock<obrain_core::index::text::InvertedIndex>>,
            )> = self
                .substrate_text_indexes
                .read()
                .iter()
                .filter(|(cle, _)| cle.starts_with(&prefixe))
                .map(|(cle, i)| (cle.clone(), std::sync::Arc::clone(i)))
                .collect();
            for (cle, idx) in entrees {
                let Some(propriete) = cle.strip_prefix(&prefixe) else {
                    continue;
                };
                let pk = obrain_common::types::PropertyKey::new(propriete);
                if let Some(obrain_common::types::Value::String(t)) = node.properties.get(&pk) {
                    idx.write().insert(id, t);
                }
            }
        }
    }

    /// Retire un noeud de tous les registres du substrat portant ce label.
    ///
    /// Ne consulte PAS le noeud : il peut deja avoir disparu (suppression), ou
    /// avoir perdu le label. C'est justement pour ca qu'on prend le label en
    /// argument au lieu de le relire.
    // Sans `vector-index` ni `text-index`, il n'y a aucun index à tenir : `id`, `label` et
    // leurs dérivés ne servent qu'aux blocs gatés ci-dessous.
    #[cfg_attr(
        not(any(feature = "vector-index", feature = "text-index")),
        allow(unused_variables)
    )]
    #[allow(dead_code)]
    pub(super) fn substrat_desindexer_pour_label(
        &self,
        id: obrain_common::types::NodeId,
        label: &str,
    ) {
        if self.substrate_store.is_none() {
            return;
        }
        let prefixe = format!("{label}:");

        #[cfg(feature = "vector-index")]
        for (cle, (_dim, idx)) in self.substrate_vector_indexes.read().iter() {
            if cle.starts_with(&prefixe) {
                idx.delete(id.0 as u32);
            }
        }

        #[cfg(feature = "text-index")]
        {
            let entrees: Vec<
                std::sync::Arc<parking_lot::RwLock<obrain_core::index::text::InvertedIndex>>,
            > = self
                .substrate_text_indexes
                .read()
                .iter()
                .filter(|(cle, _)| cle.starts_with(&prefixe))
                .map(|(_, i)| std::sync::Arc::clone(i))
                .collect();
            for idx in entrees {
                idx.write().remove(id);
            }
        }
    }

    // === Node Operations ===

    /// Creates a node with the given labels and returns its ID.
    ///
    /// Labels categorize nodes - think of them like tags. A node can have
    /// multiple labels (e.g., `["Person", "Employee"]`).
    ///
    /// # Examples
    ///
    /// ```
    /// use obrain_engine::ObrainDB;
    ///
    /// let db = ObrainDB::new_in_memory();
    /// let alix = db.create_node(&["Person"]);
    /// let company = db.create_node(&["Company", "Startup"]);
    /// ```
    pub fn create_node(&self, labels: &[&str]) -> obrain_common::types::NodeId {
        let id = self.data_store().create_node(labels);

        // Log to WAL if enabled
        #[cfg(feature = "wal")]
        if let Err(e) = self.log_wal(&WalRecord::CreateNode {
            id,
            labels: labels.iter().map(|s| (*s).to_string()).collect(),
        }) {
            tracing::warn!("Failed to log CreateNode to WAL: {}", e);
        }

        #[cfg(feature = "cdc")]
        if !self
            .cdc_observer_active
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            self.cdc_log
                .record_create_node(id, self.data_store().current_epoch(), None);
        }

        id
    }

    /// Creates a new node with labels and properties.
    ///
    /// If WAL is enabled, the operation is logged for durability.
    pub fn create_node_with_props(
        &self,
        labels: &[&str],
        properties: impl IntoIterator<
            Item = (
                impl Into<obrain_common::types::PropertyKey>,
                impl Into<obrain_common::types::Value>,
            ),
        >,
    ) -> obrain_common::types::NodeId {
        // Collect properties first so we can log them to WAL
        let props: Vec<(
            obrain_common::types::PropertyKey,
            obrain_common::types::Value,
        )> = properties
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();

        let id = self.data_store().create_node_with_props(labels, &props);

        // Build CDC snapshot before WAL consumes props
        #[cfg(feature = "cdc")]
        let cdc_props: std::collections::HashMap<String, obrain_common::types::Value> = props
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();

        // Log node creation to WAL
        #[cfg(feature = "wal")]
        {
            if let Err(e) = self.log_wal(&WalRecord::CreateNode {
                id,
                labels: labels.iter().map(|s| (*s).to_string()).collect(),
            }) {
                tracing::warn!("Failed to log CreateNode to WAL: {}", e);
            }

            // Log each property to WAL for full durability
            for (key, value) in props {
                if let Err(e) = self.log_wal(&WalRecord::SetNodeProperty {
                    id,
                    key: key.to_string(),
                    value,
                }) {
                    tracing::warn!("Failed to log SetNodeProperty to WAL: {}", e);
                }
            }
        }

        if !self
            .cdc_observer_active
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            #[cfg(feature = "cdc")]
            self.cdc_log.record_create_node(
                id,
                self.data_store().current_epoch(),
                if cdc_props.is_empty() {
                    None
                } else {
                    Some(cdc_props)
                },
            );
        }

        // Auto-insert into matching text indexes for the new node
        #[cfg(feature = "text-index")]
        if let Some(node) = self.data_store().get_node(id) {
            for label in &node.labels {
                for (prop_key, prop_val) in &node.properties {
                    if let obrain_common::types::Value::String(text) = prop_val
                        && let Some(index) =
                            self.store.get_text_index(label.as_str(), prop_key.as_ref())
                    {
                        index.write().insert(id, text);
                    }
                }
            }
        }

        id
    }

    /// Gets a node by ID.
    #[must_use]
    pub fn get_node(
        &self,
        id: obrain_common::types::NodeId,
    ) -> Option<obrain_core::graph::lpg::Node> {
        self.data_store().get_node(id)
    }

    /// Gets a node as it existed at a specific epoch.
    ///
    /// Uses pure epoch-based visibility (not transaction-aware), so the node
    /// is visible if and only if `created_epoch <= epoch` and it was not
    /// deleted at or before `epoch`.
    #[must_use]
    pub fn get_node_at_epoch(
        &self,
        id: obrain_common::types::NodeId,
        epoch: obrain_common::types::EpochId,
    ) -> Option<obrain_core::graph::lpg::Node> {
        self.data_store().get_node_at_epoch(id, epoch)
    }

    /// Gets an edge as it existed at a specific epoch.
    ///
    /// Uses pure epoch-based visibility (not transaction-aware).
    #[must_use]
    pub fn get_edge_at_epoch(
        &self,
        id: obrain_common::types::EdgeId,
        epoch: obrain_common::types::EpochId,
    ) -> Option<obrain_core::graph::lpg::Edge> {
        self.data_store().get_edge_at_epoch(id, epoch)
    }

    /// Returns all versions of a node with their creation/deletion epochs.
    ///
    /// Properties and labels reflect the current state (not versioned per-epoch).
    #[must_use]
    pub fn get_node_history(
        &self,
        id: obrain_common::types::NodeId,
    ) -> Vec<(
        obrain_common::types::EpochId,
        Option<obrain_common::types::EpochId>,
        obrain_core::graph::lpg::Node,
    )> {
        self.data_store().get_node_history(id)
    }

    /// Returns all versions of an edge with their creation/deletion epochs.
    ///
    /// Properties reflect the current state (not versioned per-epoch).
    #[must_use]
    pub fn get_edge_history(
        &self,
        id: obrain_common::types::EdgeId,
    ) -> Vec<(
        obrain_common::types::EpochId,
        Option<obrain_common::types::EpochId>,
        obrain_core::graph::lpg::Edge,
    )> {
        self.data_store().get_edge_history(id)
    }

    /// Returns the current epoch of the database.
    #[must_use]
    pub fn current_epoch(&self) -> obrain_common::types::EpochId {
        self.data_store().current_epoch()
    }

    /// Deletes a node and all its edges.
    ///
    /// If WAL is enabled, the operation is logged for durability.
    pub fn delete_node(&self, id: obrain_common::types::NodeId) -> bool {
        // Capture properties for CDC before deletion
        #[cfg(feature = "cdc")]
        let cdc_props = self.data_store().get_node(id).map(|node| {
            node.properties
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect::<std::collections::HashMap<String, obrain_common::types::Value>>()
        });

        // Collect matching text indexes BEFORE deletion removes labels
        #[cfg(feature = "text-index")]
        let text_indexes_to_clean: Vec<
            std::sync::Arc<parking_lot::RwLock<obrain_core::index::text::InvertedIndex>>,
        > = self
            .store
            .get_node(id)
            .map(|node| {
                let mut indexes = Vec::new();
                for label in &node.labels {
                    let prefix = format!("{}:", label.as_str());
                    for (key, index) in self.store.text_index_entries() {
                        if key.starts_with(&prefix) {
                            indexes.push(index);
                        }
                    }
                    // (2026-08-13, T17b) ENTRETIEN DU REGISTRE SUBSTRAT.
                    // Sans ceci, un index texte construit par
                    // `create_text_index` en mode substrat ne se nettoyait
                    // JAMAIS a la suppression d'un noeud : le document
                    // restait dans le BM25 et ressortait dans les
                    // recherches, en pointant un noeud disparu.
                    // Les deux registres portent le meme type, on fusionne.
                    for (key, index) in self.substrate_text_indexes.read().iter() {
                        if key.starts_with(&prefix) {
                            indexes.push(std::sync::Arc::clone(index));
                        }
                    }
                }
                indexes
            })
            .unwrap_or_default();

        // Les labels doivent etre lus AVANT la suppression, et contre le VRAI
        // store — apres, le noeud n'existe plus.
        #[cfg(feature = "vector-index")]
        let labels_avant: Vec<String> = self
            .data_store()
            .get_node(id)
            .map(|n| n.labels.iter().map(|l| l.as_str().to_string()).collect())
            .unwrap_or_default();

        // Contrat documenté (« Deletes a node and all its edges ») : les arêtes
        // incidentes partent AVANT le nœud. Sans ce détachement, une arête entrante
        // survivait à son nœud — mesuré sur le noyau : une arête CONTENT fantôme
        // pointait vers un Blob après la suppression du fichier qui le contenait.
        self.data_store().delete_node_edges(id);
        let result = self.data_store().delete_node(id);

        // Désindexation substrat : les labels sont capturés AVANT la suppression
        // (`labels_avant`), car le nœud n'existe plus après. Les index vectoriels
        // du substrat sont retirés par l'observateur (T5), pas ici.
        #[cfg(feature = "vector-index")]
        if result {
            for label in &labels_avant {
                self.substrat_desindexer_pour_label(id, label);
            }
        }

        // Remove from text indexes after successful deletion
        #[cfg(feature = "text-index")]
        if result {
            for index in text_indexes_to_clean {
                index.write().remove(id);
            }
        }

        #[cfg(feature = "wal")]
        if result && let Err(e) = self.log_wal(&WalRecord::DeleteNode { id }) {
            tracing::warn!("Failed to log DeleteNode to WAL: {}", e);
        }

        #[cfg(feature = "cdc")]
        if result {
            self.cdc_log.record_delete(
                crate::cdc::EntityId::Node(id),
                self.data_store().current_epoch(),
                cdc_props,
            );
        }

        result
    }

    /// Sets a property on a node.
    ///
    /// If WAL is enabled, the operation is logged for durability.
    pub fn set_node_property(
        &self,
        id: obrain_common::types::NodeId,
        key: &str,
        value: obrain_common::types::Value,
    ) {
        // Extract vector data before the value is moved into the store
        #[cfg(feature = "vector-index")]
        let vector_data = match &value {
            obrain_common::types::Value::Vector(v) => Some(v.clone()),
            _ => None,
        };

        // Log to WAL first
        #[cfg(feature = "wal")]
        if let Err(e) = self.log_wal(&WalRecord::SetNodeProperty {
            id,
            key: key.to_string(),
            value: value.clone(),
        }) {
            tracing::warn!("Failed to log SetNodeProperty to WAL: {}", e);
        }

        // Capture old value for CDC before the store write
        #[cfg(feature = "cdc")]
        let cdc_old_value = self
            .store
            .get_node_property(id, &obrain_common::types::PropertyKey::new(key));
        #[cfg(feature = "cdc")]
        let cdc_new_value = value.clone();

        self.data_store().set_node_property(id, key, value);

        #[cfg(feature = "cdc")]
        self.cdc_log.record_update(
            crate::cdc::EntityId::Node(id),
            self.data_store().current_epoch(),
            key,
            cdc_old_value,
            cdc_new_value,
        );

        // Auto-insert into matching vector indexes
        #[cfg(feature = "vector-index")]
        if let Some(vec) = vector_data
            && let Some(node) = self.data_store().get_node(id)
        {
            // (2026-08-13, T17b) ENTRETIEN DU REGISTRE SUBSTRAT.
            // Sans ceci, un index construit par `create_vector_index` restait
            // FIGE : l'ecriture d'un nouvel embedding ne l'atteignait pas, et
            // la recherche rendait l'ancien voisinage sans le dire. Un index
            // fige est plus dangereux qu'un index absent — l'absence se voit.
            // La dimension est verifiee : le cascade n'accepte que L2_DIM, et
            // un vecteur d'une autre largeur doit etre IGNORE, pas paniquer.
            {
                for label in &node.labels {
                    let cle = format!("{}:{}", label.as_str(), key);
                    let entree = self.substrate_vector_indexes.read().get(&cle).cloned();
                    // Comparaison sur la dimension NATIVE de l'index (c'est lui
                    // qui projette, O-VEC C4). Un embedding d'une autre largeur
                    // est ignore — jamais fatal : une base peut porter plusieurs
                    // modeles sur la meme propriete.
                    if let Some((dim_native, idx)) = entree
                        && dim_native == vec.len()
                    {
                        idx.insert(id.0 as u32, &vec);
                    }
                }
            }
        }

        // Auto-update matching text indexes
        #[cfg(feature = "text-index")]
        if let Some(node) = self.data_store().get_node(id) {
            let text_val = node
                .properties
                .get(&obrain_common::types::PropertyKey::new(key))
                .and_then(|v| match v {
                    obrain_common::types::Value::String(s) => Some(s.to_string()),
                    _ => None,
                });
            for label in &node.labels {
                if let Some(index) = self.store.get_text_index(label.as_str(), key) {
                    let mut idx = index.write();
                    if let Some(ref text) = text_val {
                        idx.insert(id, text);
                    } else {
                        idx.remove(id);
                    }
                }
            }
            // (T17b) Meme entretien pour le registre texte du substrat : un
            // BM25 qui ne suit pas les ecritures rend des documents perimes
            // en pointant des noeuds dont le texte a change.
            {
                for label in &node.labels {
                    let cle = format!("{}:{}", label.as_str(), key);
                    let idx = self.substrate_text_indexes.read().get(&cle).cloned();
                    if let Some(idx) = idx {
                        match text_val.as_deref() {
                            Some(t) => idx.write().insert(id, t),
                            None => {
                                idx.write().remove(id);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Adds a label to an existing node.
    ///
    /// Returns `true` if the label was added, `false` if the node doesn't exist
    /// or already has the label.
    ///
    /// # Examples
    ///
    /// ```
    /// use obrain_engine::ObrainDB;
    ///
    /// let db = ObrainDB::new_in_memory();
    /// let alix = db.create_node(&["Person"]);
    ///
    /// // Promote Alix to Employee
    /// let added = db.add_node_label(alix, "Employee");
    /// assert!(added);
    /// ```
    pub fn add_node_label(&self, id: obrain_common::types::NodeId, label: &str) -> bool {
        let result = self.data_store().add_label(id, label);

        #[cfg(feature = "wal")]
        if result {
            // Log to WAL if enabled
            if let Err(e) = self.log_wal(&WalRecord::AddNodeLabel {
                id,
                label: label.to_string(),
            }) {
                tracing::warn!("Failed to log AddNodeLabel to WAL: {}", e);
            }
        }

        // Auto-insert into text indexes for the newly-added label
        #[cfg(feature = "text-index")]
        if result && let Some(node) = self.data_store().get_node(id) {
            for (prop_key, prop_val) in &node.properties {
                if let obrain_common::types::Value::String(text) = prop_val
                    && let Some(index) = self.store.get_text_index(label, prop_key.as_ref())
                {
                    index.write().insert(id, text);
                }
            }
        }

        // (2026-08-13) Registres du substrat. Un vecteur ecrit AVANT que le
        // label n'existe n'etait indexe par personne : le chemin d'ecriture de
        // propriete ne voyait pas le label, et ce chemin-ci n'interrogeait que
        // le store fantoche.
        if result {
            self.substrat_indexer_pour_label(id, label);
        }

        result
    }

    /// Removes a label from a node.
    ///
    /// Returns `true` if the label was removed, `false` if the node doesn't exist
    /// or doesn't have the label.
    ///
    /// # Examples
    ///
    /// ```
    /// use obrain_engine::ObrainDB;
    ///
    /// let db = ObrainDB::new_in_memory();
    /// let alix = db.create_node(&["Person", "Employee"]);
    ///
    /// // Remove Employee status
    /// let removed = db.remove_node_label(alix, "Employee");
    /// assert!(removed);
    /// ```
    pub fn remove_node_label(&self, id: obrain_common::types::NodeId, label: &str) -> bool {
        // Collect text indexes to clean BEFORE removing the label
        #[cfg(feature = "text-index")]
        let text_indexes_to_clean: Vec<
            std::sync::Arc<parking_lot::RwLock<obrain_core::index::text::InvertedIndex>>,
        > = {
            let prefix = format!("{label}:");
            self.store
                .text_index_entries()
                .into_iter()
                .filter(|(key, _)| key.starts_with(&prefix))
                .map(|(_, index)| index)
                .collect()
        };

        let result = self.data_store().remove_label(id, label);

        #[cfg(feature = "wal")]
        if result {
            // Log to WAL if enabled
            if let Err(e) = self.log_wal(&WalRecord::RemoveNodeLabel {
                id,
                label: label.to_string(),
            }) {
                tracing::warn!("Failed to log RemoveNodeLabel to WAL: {}", e);
            }
        }

        // Remove from text indexes for the removed label
        #[cfg(feature = "text-index")]
        if result {
            for index in text_indexes_to_clean {
                index.write().remove(id);
            }
        }

        // (2026-08-13) Le noeud perd ce label : il sort des index qui y sont
        // attaches. Symetrique de `add_node_label`.
        if result {
            self.substrat_desindexer_pour_label(id, label);
        }

        result
    }

    /// Gets all labels for a node.
    ///
    /// Returns `None` if the node doesn't exist.
    ///
    /// # Examples
    ///
    /// ```
    /// use obrain_engine::ObrainDB;
    ///
    /// let db = ObrainDB::new_in_memory();
    /// let alix = db.create_node(&["Person", "Employee"]);
    ///
    /// let labels = db.get_node_labels(alix).unwrap();
    /// assert!(labels.contains(&"Person".to_string()));
    /// assert!(labels.contains(&"Employee".to_string()));
    /// ```
    #[must_use]
    pub fn get_node_labels(&self, id: obrain_common::types::NodeId) -> Option<Vec<String>> {
        self.store
            .get_node(id)
            .map(|node| node.labels.iter().map(|s| s.to_string()).collect())
    }

    // === Edge Operations ===

    /// Creates an edge (relationship) between two nodes.
    ///
    /// Edges connect nodes and have a type that describes the relationship.
    /// They're directed - the order of `src` and `dst` matters.
    ///
    /// # Examples
    ///
    /// ```
    /// use obrain_engine::ObrainDB;
    ///
    /// let db = ObrainDB::new_in_memory();
    /// let alix = db.create_node(&["Person"]);
    /// let gus = db.create_node(&["Person"]);
    ///
    /// // Alix knows Gus (directed: Alix -> Gus)
    /// let edge = db.create_edge(alix, gus, "KNOWS");
    /// ```
    pub fn create_edge(
        &self,
        src: obrain_common::types::NodeId,
        dst: obrain_common::types::NodeId,
        edge_type: &str,
    ) -> obrain_common::types::EdgeId {
        let id = self.data_store().create_edge(src, dst, edge_type);

        // Log to WAL if enabled
        #[cfg(feature = "wal")]
        if let Err(e) = self.log_wal(&WalRecord::CreateEdge {
            id,
            src,
            dst,
            edge_type: edge_type.to_string(),
        }) {
            tracing::warn!("Failed to log CreateEdge to WAL: {}", e);
        }

        #[cfg(feature = "cdc")]
        if !self
            .cdc_observer_active
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            self.cdc_log
                .record_create_edge(id, self.data_store().current_epoch(), None);
        }

        id
    }

    /// Crée un LOT d'arêtes (batch natif, chemin `GraphStoreMut::batch_create_edges`)
    /// — import de gros volumes sans aller-retour par arête (RFC §12, index de
    /// code cbm→substrate). WAL/CDC journalisés par arête comme le chemin
    /// unitaire. L'appelant checkpoint/flush ensuite pour la durabilité.
    pub fn create_edges_batch(
        &self,
        edges: &[(
            obrain_common::types::NodeId,
            obrain_common::types::NodeId,
            &str,
        )],
    ) -> Vec<obrain_common::types::EdgeId> {
        if edges.is_empty() {
            return Vec::new();
        }
        let ids = self.data_store().batch_create_edges(edges);
        #[cfg(feature = "wal")]
        for (id, (src, dst, edge_type)) in ids.iter().zip(edges.iter()) {
            if let Err(e) = self.log_wal(&WalRecord::CreateEdge {
                id: *id,
                src: *src,
                dst: *dst,
                edge_type: (*edge_type).to_string(),
            }) {
                tracing::warn!("Failed to log CreateEdge (batch) to WAL: {}", e);
            }
        }
        #[cfg(feature = "cdc")]
        {
            let epoch = self.data_store().current_epoch();
            for id in &ids {
                if !self
                    .cdc_observer_active
                    .load(std::sync::atomic::Ordering::Relaxed)
                {
                    self.cdc_log.record_create_edge(*id, epoch, None);
                }
            }
        }
        ids
    }

    /// Crée un nœud SANS journalisation engine (WAL/CDC) — pour les imports
    /// de gros volumes (l'importeur checkpoint ensuite). Le reste du chemin
    /// (labels, slots, propriétés éventuelles) est identique.
    pub fn create_node_no_wal(&self, labels: &[&str]) -> obrain_common::types::NodeId {
        self.data_store().create_node(labels)
    }

    /// Variante BULK LOAD de [`Self::create_edges_batch`] : SANS journalisation
    /// WAL/CDC par arête — l'importeur appelle `checkpoint()` à la fin pour la
    /// durabilité (RFC §12, import index de code cbm→substrate). Destinée aux
    /// imports de gros volumes jetables/en lot, pas au hot path applicatif.
    pub fn create_edges_batch_no_wal(
        &self,
        edges: &[(
            obrain_common::types::NodeId,
            obrain_common::types::NodeId,
            &str,
        )],
    ) -> Vec<obrain_common::types::EdgeId> {
        if edges.is_empty() {
            return Vec::new();
        }
        self.data_store().batch_create_edges(edges)
    }

    /// Creates a new edge with properties.
    ///
    /// If WAL is enabled, the operation is logged for durability.
    pub fn create_edge_with_props(
        &self,
        src: obrain_common::types::NodeId,
        dst: obrain_common::types::NodeId,
        edge_type: &str,
        properties: impl IntoIterator<
            Item = (
                impl Into<obrain_common::types::PropertyKey>,
                impl Into<obrain_common::types::Value>,
            ),
        >,
    ) -> obrain_common::types::EdgeId {
        // Collect properties first so we can log them to WAL
        let props: Vec<(
            obrain_common::types::PropertyKey,
            obrain_common::types::Value,
        )> = properties
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();

        let id = self
            .data_store()
            .create_edge_with_props(src, dst, edge_type, &props);

        // Build CDC snapshot before WAL consumes props
        #[cfg(feature = "cdc")]
        let cdc_props: std::collections::HashMap<String, obrain_common::types::Value> = props
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();

        // Log edge creation to WAL
        #[cfg(feature = "wal")]
        {
            if let Err(e) = self.log_wal(&WalRecord::CreateEdge {
                id,
                src,
                dst,
                edge_type: edge_type.to_string(),
            }) {
                tracing::warn!("Failed to log CreateEdge to WAL: {}", e);
            }

            // Log each property to WAL for full durability
            for (key, value) in props {
                if let Err(e) = self.log_wal(&WalRecord::SetEdgeProperty {
                    id,
                    key: key.to_string(),
                    value,
                }) {
                    tracing::warn!("Failed to log SetEdgeProperty to WAL: {}", e);
                }
            }
        }

        if !self
            .cdc_observer_active
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            #[cfg(feature = "cdc")]
            self.cdc_log.record_create_edge(
                id,
                self.data_store().current_epoch(),
                if cdc_props.is_empty() {
                    None
                } else {
                    Some(cdc_props)
                },
            );
        }

        id
    }

    /// Gets an edge by ID.
    #[must_use]
    pub fn get_edge(
        &self,
        id: obrain_common::types::EdgeId,
    ) -> Option<obrain_core::graph::lpg::Edge> {
        self.data_store().get_edge(id)
    }

    /// Deletes an edge.
    ///
    /// If WAL is enabled, the operation is logged for durability.
    pub fn delete_edge(&self, id: obrain_common::types::EdgeId) -> bool {
        // Capture properties for CDC before deletion
        #[cfg(feature = "cdc")]
        let cdc_props = self.data_store().get_edge(id).map(|edge| {
            edge.properties
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect::<std::collections::HashMap<String, obrain_common::types::Value>>()
        });

        let result = self.data_store().delete_edge(id);

        #[cfg(feature = "wal")]
        if result && let Err(e) = self.log_wal(&WalRecord::DeleteEdge { id }) {
            tracing::warn!("Failed to log DeleteEdge to WAL: {}", e);
        }

        #[cfg(feature = "cdc")]
        if result {
            self.cdc_log.record_delete(
                crate::cdc::EntityId::Edge(id),
                self.data_store().current_epoch(),
                cdc_props,
            );
        }

        result
    }

    /// Sets a property on an edge.
    ///
    /// If WAL is enabled, the operation is logged for durability.
    pub fn set_edge_property(
        &self,
        id: obrain_common::types::EdgeId,
        key: &str,
        value: obrain_common::types::Value,
    ) {
        // Log to WAL first
        #[cfg(feature = "wal")]
        if let Err(e) = self.log_wal(&WalRecord::SetEdgeProperty {
            id,
            key: key.to_string(),
            value: value.clone(),
        }) {
            tracing::warn!("Failed to log SetEdgeProperty to WAL: {}", e);
        }

        // Capture old value for CDC before the store write
        #[cfg(feature = "cdc")]
        let cdc_old_value = self
            .store
            .get_edge_property(id, &obrain_common::types::PropertyKey::new(key));
        #[cfg(feature = "cdc")]
        let cdc_new_value = value.clone();

        self.data_store().set_edge_property(id, key, value);

        #[cfg(feature = "cdc")]
        self.cdc_log.record_update(
            crate::cdc::EntityId::Edge(id),
            self.data_store().current_epoch(),
            key,
            cdc_old_value,
            cdc_new_value,
        );
    }

    /// Removes a property from a node.
    ///
    /// Returns true if the property existed and was removed, false otherwise.
    pub fn remove_node_property(&self, id: obrain_common::types::NodeId, key: &str) -> bool {
        let removed = self.data_store().remove_node_property(id, key).is_some();

        #[cfg(feature = "wal")]
        if removed
            && let Err(e) = self.log_wal(&WalRecord::RemoveNodeProperty {
                id,
                key: key.to_string(),
            })
        {
            tracing::warn!("WAL log for RemoveNodeProperty failed: {e}");
        }

        // Remove from matching text indexes
        #[cfg(feature = "text-index")]
        if removed && let Some(node) = self.data_store().get_node(id) {
            for label in &node.labels {
                if let Some(index) = self.store.get_text_index(label.as_str(), key) {
                    index.write().remove(id);
                }
            }
            // (T17b) Ici la propriete est RETIREE : le document sort de
            // l'index, il n'y a pas de nouveau texte a indexer. Le bloc
            // symetrique du chemin d'ecriture, lui, reindexe.
            {
                for label in &node.labels {
                    let cle = format!("{}:{}", label.as_str(), key);
                    let idx = self.substrate_text_indexes.read().get(&cle).cloned();
                    if let Some(idx) = idx {
                        idx.write().remove(id);
                    }
                }
            }
        }

        removed
    }

    /// Removes a property from an edge.
    ///
    /// Returns true if the property existed and was removed, false otherwise.
    pub fn remove_edge_property(&self, id: obrain_common::types::EdgeId, key: &str) -> bool {
        let removed = self.data_store().remove_edge_property(id, key).is_some();

        #[cfg(feature = "wal")]
        if removed
            && let Err(e) = self.log_wal(&WalRecord::RemoveEdgeProperty {
                id,
                key: key.to_string(),
            })
        {
            tracing::warn!("WAL log for RemoveEdgeProperty failed: {e}");
        }

        removed
    }

    /// Creates multiple nodes in bulk, each with a single vector property.
    ///
    /// Much faster than individual `create_node_with_props` calls because it
    /// acquires internal locks once and loops in Rust rather than crossing
    /// the FFI boundary per vector.
    ///
    /// # Arguments
    ///
    /// * `label` - Label applied to all created nodes
    /// * `property` - Property name for the vector data
    /// * `vectors` - Vector data for each node
    ///
    /// # Returns
    ///
    /// Vector of created `NodeId`s in the same order as the input vectors.
    pub fn batch_create_nodes(
        &self,
        label: &str,
        property: &str,
        vectors: Vec<Vec<f32>>,
    ) -> Vec<obrain_common::types::NodeId> {
        use obrain_common::types::{PropertyKey, Value};

        let prop_key = PropertyKey::new(property);
        let labels: &[&str] = &[label];

        let ids: Vec<obrain_common::types::NodeId> = vectors
            .into_iter()
            .map(|vec| {
                let value = Value::Vector(vec.into());
                let id = self
                    .data_store()
                    .create_node_with_props(labels, &[(prop_key.clone(), value.clone())]);

                // Log to WAL
                #[cfg(feature = "wal")]
                {
                    if let Err(e) = self.log_wal(&WalRecord::CreateNode {
                        id,
                        labels: labels.iter().map(|s| (*s).to_string()).collect(),
                    }) {
                        tracing::warn!("Failed to log CreateNode to WAL: {}", e);
                    }
                    if let Err(e) = self.log_wal(&WalRecord::SetNodeProperty {
                        id,
                        key: property.to_string(),
                        value,
                    }) {
                        tracing::warn!("Failed to log SetNodeProperty to WAL: {}", e);
                    }
                }

                id
            })
            .collect();

        // (2026-08-13) Meme entretien pour le substrat. Sans ceci, un lot cree
        // APRES `create_vector_index` restait invisible a la recherche — la
        // base repondait, sans les noeuds du lot, et rien ne le signalait.
        for &id in &ids {
            self.substrat_indexer_pour_label(id, label);
        }

        ids
    }
}
