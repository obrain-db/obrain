//! Index management for ObrainDB (property, vector, and text indexes).

// Inconditionnels : l'index SPATIAL (toujours compilé) en a besoin, même sans
// `vector-index` ni `text-index`.
use std::sync::Arc;

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

        // Hors substrat (store externe) : aucun registre vectoriel n'existe.
        // L'ancien chemin HNSW du store, sans aucune implémentation, est
        // supprimé (O-VEC nettoyage). Refus explicite plutôt qu'un index fantôme.
        let _ = (metric, m, ef_construction, dimensions);
        Err(obrain_common::utils::error::Error::Internal(format!(
            "create_vector_index(:{label}({property})) : index vectoriel indisponible sur un \
             store externe — seule une base substrat en porte."
        )))
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
            // (#209) Le nom part avec sa declaration (catalogue) : le cache
            // suit, sans quoi `queryNodes` resoudrait un nom orphelin.
            self.noms_index_vectoriels
                .write()
                .retain(|_, (l, p)| !(l == label && p == property));
            if retire {
                tracing::info!("Index vectoriel (substrat) retire : :{label}({property})");
            }
            return retire;
        }
        // Hors substrat : aucun index vectoriel à retirer.
        false
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
        // Hors substrat : pas de registre à reconstruire ; la création refusera
        // explicitement (voir create_vector_index).
        self.create_vector_index(label, property, None, None, None, None)
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

    // Plus d'appelant spatial (DROP spatial : `supprimer_index`, qui PROPAGE
    // l'échec) ; restent le vectoriel et le textuel, sous leurs features.
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
        // Reference FAIBLE : l'index vit dans le registre, partage avec l'observateur
        // que le store detient lui-meme. Une reference forte ferait un cycle
        // (store -> observateur -> registre -> index -> store) : le store ne
        // serait jamais libere et le verrou du repertoire resterait pris.
        let data = std::sync::Arc::downgrade(&self.data_store());
        let cle = obrain_common::types::PropertyKey::new(property);
        Some(std::sync::Arc::new(
            move |offset: obrain_substrate::NodeOffset, dest: &mut Vec<f32>| {
                let Some(data) = data.upgrade() else {
                    return false;
                };
                match data.get_node_property(obrain_common::types::NodeId(u64::from(offset)), &cle)
                {
                    Some(obrain_common::types::Value::Vector(v)) => {
                        dest.extend_from_slice(&v);
                        true
                    }
                    _ => false,
                }
            },
        ))
    }

    // ── INDEX SPATIAL ───────────────────────────────────────────────────────

    /// Construit (ou reconstruit) l'index spatial d'un label sur une propriété
    /// qui porte un point `[lon, lat]` — la forme que `point(lon, lat)` produit.
    ///
    /// Rend le nombre de points indexés. Les nœuds dont la propriété n'est pas
    /// un point (absente, liste d'une autre longueur, valeurs non numériques,
    /// latitude hors `[-90, 90]`) sont IGNORÉS, pas fatals : une base peut
    /// porter des coordonnées manquantes, et les compter serait mentir sur ce
    /// que l'index contient.
    ///
    /// # Errors
    ///
    /// Base sans substrat — seule une base substrat porte un catalogue d'index.
    pub fn create_spatial_index(&self, label: &str, property: &str) -> Result<usize> {
        if self.substrate_store.is_none() {
            return Err(obrain_common::utils::error::Error::Internal(format!(
                "create_spatial_index(:{label}({property})) : index spatial indisponible \
                 sur un store externe — seule une base substrat en porte."
            )));
        }
        let store = self.data_store();
        let index = self
            .substrate_spatial_indexes
            .rebatir_sous_verrou(&format!("{label}:{property}"), || {
                super::spatial_procedure::construire_index(&*store, label, property)
            });
        let n = index.read().len();
        // DÉCLARATION DURABLE : l'arbre se rebâtit depuis les propriétés, mais
        // le FAIT qu'il doive exister ne se rebâtit pas. Sans cette ligne,
        // l'index disparaîtrait à la fermeture (même leçon que le vectoriel).
        self.declarer_index_substrat(obrain_substrate::DeclarationIndex {
            genre: "spatial".to_string(),
            label: label.to_string(),
            propriete: property.to_string(),
            dimension: Some(2),
            metrique: Some("haversine".to_string()),
        });
        Ok(n)
    }

    /// Crée l'index ET lui donne un nom durable, d'un seul geste.
    ///
    /// # Errors
    ///
    /// Nom déjà porté par un autre index spatial, ou création impossible.
    pub fn create_named_spatial_index(
        &self,
        name: &str,
        label: &str,
        property: &str,
    ) -> Result<usize> {
        if let Some((l, p)) = self.resoudre_index_spatial_catalogue(name)
            && !(l == label && p == property)
        {
            return Err(obrain_common::utils::error::Error::Internal(format!(
                "create_named_spatial_index('{name}') : ce nom désigne déjà l'index spatial \
                 :{l}({p})"
            )));
        }
        let n = self.create_spatial_index(label, property)?;
        self.name_spatial_index(name, label, property)?;
        Ok(n)
    }

    /// Donne un nom durable à l'index spatial existant sur `:label(propriété)`.
    /// Idempotent ; un index porte au plus un nom.
    ///
    /// # Errors
    ///
    /// Aucun index déclaré sur ce couple, nom invalide ou déjà pris.
    pub fn name_spatial_index(&self, name: &str, label: &str, property: &str) -> Result<()> {
        let Some(ref sub) = self.substrate_store else {
            return Err(obrain_common::utils::error::Error::Internal(format!(
                "name_spatial_index('{name}') : index spatial indisponible sur un store externe."
            )));
        };
        let resultat = sub
            .nommer_index("spatial", label, property, name)
            .map_err(|e| {
                obrain_common::utils::error::Error::Internal(format!(
                    "name_spatial_index('{name}', :{label}({property})) : {e}"
                ))
            });
        // Le nom est écrit au catalogue : le cache de noms le sait périmé.
        self.substrate_spatial_indexes.invalider_noms();
        resultat
    }

    /// Retire l'index spatial et sa déclaration DURABLE. Rend `true` s'il
    /// était déclaré ou chargé.
    ///
    /// # Errors
    ///
    /// Le retrait durable de la déclaration a échoué : RIEN n'a été modifié,
    /// l'index reste en mémoire et au catalogue (avant : l'échec était
    /// seulement journalisé et l'index revenait à la réouverture).
    pub fn drop_spatial_index(&self, label: &str, property: &str) -> Result<bool> {
        super::spatial_procedure::supprimer_index(
            &self.substrate_spatial_indexes,
            self.substrate_store.as_deref(),
            label,
            property,
        )
    }

    /// Reconstruit l'index par un rescan complet.
    ///
    /// Inutile après une écriture (la surcouche la porte déjà) : c'est le geste
    /// d'après un chargement massif, ou quand `doit_etre_rebati` le demande.
    ///
    /// # Errors
    ///
    /// Création impossible (base sans substrat).
    pub fn rebuild_spatial_index(&self, label: &str, property: &str) -> Result<usize> {
        // Pas de retrait préalable : la construction REMPLACE l'index sous le
        // verrou du registre (`rebatir_sous_verrou`), sans fenêtre où le
        // couple ne serait plus suivi.
        self.create_spatial_index(label, property)
    }

    /// Les index spatiaux NOMMÉS, `(nom, label, propriété)`, triés par nom.
    #[must_use]
    pub fn spatial_index_names(&self) -> Vec<(String, String, String)> {
        self.substrate_spatial_indexes
            .noms(self.substrate_store.as_deref())
    }

    /// `(label, propriété)` d'un nom d'index spatial, lu au catalogue durable.
    fn resoudre_index_spatial_catalogue(&self, name: &str) -> Option<(String, String)> {
        self.spatial_index_names()
            .into_iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, l, p)| (l, p))
    }

    /// `(label, propriété)` d'un index désigné par son NOM ou par
    /// `label:propriété`, ou une ERREUR qui nomme ce qui existe.
    ///
    /// Une erreur plutôt qu'un résultat vide : une faute de frappe dans le nom
    /// d'un index doit se voir, pas se confondre avec « aucun point ici ».
    ///
    /// # Errors
    ///
    /// Nom inconnu et identifiant qui ne se lit pas comme `label:propriété`.
    pub(super) fn resoudre_index_spatial(&self, nom_ou_ident: &str) -> Result<(String, String)> {
        if let Some(paire) = self.resoudre_index_spatial_catalogue(nom_ou_ident) {
            return Ok(paire);
        }
        // Un identifiant `label:propriété` n'est accepté que s'il est DÉCLARÉ
        // (même règle que `PoigneeIndexSpatiaux::resoudre`, revue de #282) :
        // sinon l'API Rust bâtissait un index implicite, non durable,
        // invisible de SHOW INDEXES.
        if let Some((l, p)) = nom_ou_ident.split_once(':')
            && super::spatial_procedure::declaration_spatiale(self.substrate_store.as_deref(), l, p)
        {
            return Ok((l.to_string(), p.to_string()));
        }
        Err(obrain_common::utils::error::Error::Internal(format!(
            "index spatial inconnu : '{nom_ou_ident}' — index déclarés : [{}]",
            self.spatial_index_names()
                .into_iter()
                .map(|(n, l, p)| format!("{n} = :{l}({p})"))
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }

    /// L'index spatial d'un nom, chargé ou REBÂTI À LA DEMANDE.
    ///
    /// Paresseux comme le vectoriel : une base rouverte ne porte pas ses index
    /// en mémoire, et rebâtir 10⁶ points au démarrage (648 ms mesurés)
    /// ajouterait un coût de démarrage non borné.
    ///
    /// # Errors
    ///
    /// Nom inconnu.
    pub(super) fn index_spatial(
        &self,
        nom_ou_ident: &str,
    ) -> Result<Arc<RwLock<obrain_core::index::spatial::SpatialIndex>>> {
        let (label, property) = self.resoudre_index_spatial(nom_ou_ident)?;
        let cle = format!("{label}:{property}");
        if let Some(idx) = self.substrate_spatial_indexes.get(&cle) {
            return Ok(idx);
        }
        // Reconstruction DIRECTE, sans passer par `rebatir_index_declare` :
        // ce dernier est gaté sur les features `vector-index`/`text-index`,
        // alors qu'un index spatial n'a aucune feature à demander.
        let store = self.data_store();
        Ok(self
            .substrate_spatial_indexes
            .construire_sous_verrou(&cle, || {
                super::spatial_procedure::construire_index(&*store, &label, &property)
            }))
    }

    /// Les nœuds à moins de `rayon_m` mètres de `(lon, lat)`, triés par
    /// distance croissante, avec leur distance EXACTE en mètres.
    ///
    /// # Errors
    ///
    /// Index inconnu (voir `resoudre_index_spatial`).
    pub fn spatial_search(
        &self,
        nom_ou_ident: &str,
        lon: f64,
        lat: f64,
        rayon_m: f64,
    ) -> Result<Vec<(obrain_common::types::NodeId, f64)>> {
        let idx = self.index_spatial(nom_ou_ident)?;
        let garde = idx.read();
        Ok(garde
            .dans_rayon(lon, lat, rayon_m)
            .into_iter()
            .map(|(id, d)| (obrain_common::types::NodeId(u64::from(id)), d))
            .collect())
    }

    /// Les `k` nœuds les plus proches de `(lon, lat)`, par distance croissante.
    ///
    /// # Errors
    ///
    /// Index inconnu (voir `resoudre_index_spatial`).
    pub fn spatial_knn(
        &self,
        nom_ou_ident: &str,
        lon: f64,
        lat: f64,
        k: usize,
    ) -> Result<Vec<(obrain_common::types::NodeId, f64)>> {
        let idx = self.index_spatial(nom_ou_ident)?;
        let garde = idx.read();
        Ok(garde
            .plus_proches(lon, lat, k)
            .into_iter()
            .map(|(id, d)| (obrain_common::types::NodeId(u64::from(id)), d))
            .collect())
    }

    /// La poignée que la SESSION porte : DDL, `CALL` et planificateur passent
    /// par elle, jamais par `ObrainDB` (qui n'est pas atteignable depuis une
    /// session, et ne doit pas l'être — le cycle empêcherait la libération).
    pub(crate) fn poignee_index_spatiaux(
        &self,
    ) -> std::sync::Arc<dyn super::spatial_procedure::IndexSpatiaux> {
        std::sync::Arc::new(super::spatial_procedure::PoigneeIndexSpatiaux::nouvelle(
            self.substrate_store.clone(),
            self.data_store(),
            std::sync::Arc::clone(&self.substrate_spatial_indexes),
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
            // L'index spatial se rebâtit depuis les propriétés, comme le
            // vectoriel : rien de persistant à recharger, la déclaration suffit.
            "spatial" => self.create_spatial_index(label, property).is_ok(),
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

/// (#209) Index vectoriels NOMMES — la resolution `nom -> (label, propriete)`
/// qu'utilise `CALL db.index.vector.queryNodes`.
///
/// Le nom vit dans le catalogue durable (`substrate.indexcat`, 6e colonne
/// facultative) : il survit a la fermeture, suit toute copie du repertoire et
/// disparait avec la declaration (`drop_vector_index`). Un catalogue ecrit
/// avant #209 se relit tel quel, sans nom.
#[cfg(feature = "vector-index")]
impl super::ObrainDB {
    /// Creates a vector index (see [`create_vector_index`](Self::create_vector_index))
    /// and gives it a durable `name`, the one `CALL db.index.vector.queryNodes(name, k, vector)`
    /// resolves — the equivalent of Neo4j's
    /// `CREATE VECTOR INDEX name FOR (n:Label) ON (n.property)`.
    ///
    /// # Errors
    ///
    /// Same as `create_vector_index`, plus: invalid name, or `name` already
    /// designating the vector index of another `(label, property)` (checked
    /// BEFORE anything is built).
    #[allow(clippy::too_many_arguments)]
    pub fn create_named_vector_index(
        &self,
        name: &str,
        label: &str,
        property: &str,
        dimensions: Option<usize>,
        metric: Option<&str>,
        m: Option<usize>,
        ef_construction: Option<usize>,
    ) -> Result<()> {
        if let Some((l, p)) = self.resoudre_nom_vectoriel_catalogue(name)
            && !(l == label && p == property)
        {
            return Err(obrain_common::utils::error::Error::Internal(format!(
                "create_named_vector_index('{name}') : ce nom designe deja l'index vectoriel \
                 :{l}({p})"
            )));
        }
        self.create_vector_index(label, property, dimensions, metric, m, ef_construction)?;
        self.name_vector_index(name, label, property)
    }

    /// Gives a durable `name` to the existing vector index on `:label(property)`
    /// — typically one created by a migration with
    /// [`create_vector_index`](Self::create_vector_index), which has no name.
    /// No rebuild if the index is loaded; a declared-but-unloaded index (reopened
    /// database) is loaded here, once, so the next `queryNodes` finds it.
    ///
    /// Idempotent. An index carries at most one name: naming it again renames it.
    ///
    /// # Errors
    ///
    /// No vector index declared on `:label(property)`, invalid name (empty, or
    /// containing a tab or newline), name already used by another vector index,
    /// read-only database, or a database without substrate.
    pub fn name_vector_index(&self, name: &str, label: &str, property: &str) -> Result<()> {
        let Some(ref sub) = self.substrate_store else {
            return Err(obrain_common::utils::error::Error::Internal(format!(
                "name_vector_index('{name}') : index vectoriel indisponible sur un store \
                 externe — seule une base substrat en porte."
            )));
        };
        sub.nommer_index("vector", label, property, name)
            .map_err(|e| {
                obrain_common::utils::error::Error::Internal(format!(
                    "name_vector_index('{name}', :{label}({property})) : {e}"
                ))
            })?;
        {
            let mut noms = self.noms_index_vectoriels.write();
            noms.retain(|_, (l, p)| !(l == label && p == property));
            noms.insert(name.to_string(), (label.to_string(), property.to_string()));
        }
        if !self
            .substrate_vector_indexes
            .read()
            .contains_key(&format!("{label}:{property}"))
        {
            self.rebatir_index_declare("vector", label, property);
        }
        Ok(())
    }

    /// The named vector indexes, as `(name, label, property)`, sorted by name.
    /// Read from the durable catalog.
    #[must_use]
    pub fn vector_index_names(&self) -> Vec<(String, String, String)> {
        let Some(ref sub) = self.substrate_store else {
            return Vec::new();
        };
        let mut noms: Vec<(String, String, String)> = sub
            .noms_index()
            .into_iter()
            .filter(|(_, d)| d.genre == "vector")
            .map(|(n, d)| (n, d.label, d.propriete))
            .collect();
        noms.sort();
        noms
    }

    /// `(label, propriete)` du nom d'index vectoriel, lu au catalogue durable.
    fn resoudre_nom_vectoriel_catalogue(&self, name: &str) -> Option<(String, String)> {
        self.vector_index_names()
            .into_iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, l, p)| (l, p))
    }

    /// Charge les index NOMMES une fois par processus, a la premiere session.
    ///
    /// POURQUOI ICI ET PAS DANS LA PROCEDURE : la poignee qu'emporte une
    /// session n'a pas `&ObrainDB`, donc ne sait pas rebatir un index declare.
    /// Sans ce prechargement, `queryNodes` echouerait sur toute base ROUVERTE
    /// (registre vide) — c'est-a-dire a chaque redemarrage de PO.
    ///
    /// POURQUOI CE N'EST PAS UNE REGRESSION DE L'OUVERTURE PARESSEUSE : seuls
    /// les index NOMMES sont concernes, et un nom n'existe que si quelqu'un l'a
    /// pose pour `queryNodes`, qui en aura besoin a sa premiere requete. Une
    /// base sans nom (toutes celles d'avant #209) ne paie qu'une lecture du
    /// catalogue, une fois ; ensuite chaque session ne paie qu'une lecture
    /// atomique.
    fn precharger_index_vectoriels_nommes(&self) {
        use std::sync::atomic::Ordering;
        if self.noms_vectoriels_prets.load(Ordering::Acquire) {
            return;
        }
        let _garde = self.prechargement_noms_vectoriels.lock();
        if self.noms_vectoriels_prets.load(Ordering::Acquire) {
            return;
        }
        let noms: std::collections::HashMap<String, (String, String)> = self
            .vector_index_names()
            .into_iter()
            .map(|(n, l, p)| (n, (l, p)))
            .collect();
        for (label, property) in noms.values() {
            if !self
                .substrate_vector_indexes
                .read()
                .contains_key(&format!("{label}:{property}"))
            {
                // Echec = journalise par `rebatir_index_declare` ; la procedure
                // nommera l'index « declare mais pas charge » au lieu de
                // rendre vide.
                self.rebatir_index_declare("vector", label, property);
            }
        }
        *self.noms_index_vectoriels.write() = noms;
        self.noms_vectoriels_prets.store(true, Ordering::Release);
    }

    /// La poignee `CALL db.index.vector.queryNodes` de cette base (clones
    /// `Arc`), cablee jusqu'au planificateur par `ObrainDB::session()`.
    pub(crate) fn poignee_index_vectoriels(
        &self,
    ) -> std::sync::Arc<dyn super::vector_query_nodes::IndexVectorielsNommes> {
        self.precharger_index_vectoriels_nommes();
        std::sync::Arc::new(super::search::PoigneeIndexVectoriels {
            substrate_store: self.substrate_store.clone(),
            noms: std::sync::Arc::clone(&self.noms_index_vectoriels),
            vecteurs: std::sync::Arc::clone(&self.substrate_vector_indexes),
            prets: std::sync::Arc::clone(&self.noms_vectoriels_prets),
        })
    }
}
