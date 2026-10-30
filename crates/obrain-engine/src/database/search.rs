//! Vector, text, and hybrid search operations for ObrainDB.

#[cfg(any(
    feature = "vector-index",
    feature = "text-index",
    feature = "hybrid-search"
))]
use obrain_common::types::NodeId;
#[cfg(feature = "vector-index")]
use obrain_common::types::Value;
#[cfg(any(feature = "text-index", feature = "hybrid-search"))]
use obrain_common::utils::error::Error;
#[cfg(any(
    feature = "vector-index",
    feature = "text-index",
    feature = "hybrid-search"
))]
use obrain_common::utils::error::Result;

/// Evalue un filtre a operateurs (`{"$gt": 5}`) contre UNE valeur de propriete.
///
/// Vocabulaire : `$eq`, `$ne`, `$gt`, `$gte`, `$lt`, `$lte`, `$in`, `$nin`,
/// `$contains`. Plusieurs operateurs dans la meme map se conjuguent en ET.
///
/// LES COMPARAISONS PASSENT PAR `value_utils::compare_values`, qui délègue
/// au comparateur CANONIQUE (`obrain_common::types::value_cmp`) — le même
/// que les opérateurs de filtre, de tri et d'agrégation. (La campagne de
/// conformité avait relevé que cette affirmation était FAUSSE — ce fichier
/// comparait deux chaînes numériquement quand le filtre GQL ne le faisait
/// pas ; depuis le correctif E elle est vraie par construction.) Un `>` en
/// recherche vectorielle et un `>` en GQL rendent le même verdict :
/// Int64↔Float64 par valeur exacte, chaîne↔nombre incomparables.
///
/// Une valeur INCOMPARABLE (types sans ordre commun) ne correspond pas : c'est
/// la logique trivaluee — inconnu n'est pas vrai. Un noeud sans la propriete
/// est ecarte en amont, pour la meme raison.
fn value_matches_operators(
    valeur: &obrain_common::types::Value,
    filtre: &obrain_common::types::Value,
) -> bool {
    use std::cmp::Ordering;

    // Import LOCAL, et non en tete de fichier : la ligne `use ... Value` du
    // module est derriere un `cfg` de fonctionnalite, alors que cette
    // fonction ne l'est pas. Le decalage ne se voyait PAS a
    // `cargo check --workspace` — l'unification des fonctionnalites entre
    // crates activait l'import — et cassait la construction du seul crate
    // obrain-engine. Un cassage qui n'apparait que dans un mode de
    // construction est pire qu'un cassage franc : la CI peut le manquer.
    use obrain_common::types::Value;

    use obrain_core::execution::operators::value_utils::compare_values;

    let Value::Map(ops) = filtre else {
        // Pas une map d'operateurs : egalite stricte.
        return valeur == filtre;
    };

    ops.iter().all(|(op, attendu)| {
        let ord = || compare_values(valeur, attendu);
        match op.as_str() {
            "$eq" => valeur == attendu,
            "$ne" => valeur != attendu,
            "$gt" => ord() == Some(Ordering::Greater),
            "$gte" => matches!(ord(), Some(Ordering::Greater | Ordering::Equal)),
            "$lt" => ord() == Some(Ordering::Less),
            "$lte" => matches!(ord(), Some(Ordering::Less | Ordering::Equal)),
            "$in" => match attendu {
                Value::List(l) => l.iter().any(|x| x == valeur),
                _ => false,
            },
            "$nin" => match attendu {
                Value::List(l) => !l.iter().any(|x| x == valeur),
                _ => false,
            },
            "$contains" => match (valeur, attendu) {
                (Value::String(h), Value::String(n)) => h.contains(n.as_str()),
                // `$contains` sur une liste : appartenance.
                (Value::List(l), x) => l.iter().any(|e| e == x),
                _ => false,
            },
            // Un operateur INCONNU ne doit pas passer pour vrai. Le laisser
            // filer rendrait un filtre mal orthographie totalement inoperant
            // — et silencieusement permissif.
            _ => false,
        }
    })
}

impl super::ObrainDB {
    /// Computes a node allowlist from property filters.
    ///
    /// Supports equality filters (scalar values) and operator filters (Map values
    /// with `$`-prefixed keys like `$gt`, `$lt`, `$in`, `$contains`).
    ///
    /// Returns `None` if filters is `None` or empty (meaning no filtering),
    /// or `Some(set)` with the intersection (possibly empty).
    #[cfg(feature = "vector-index")]
    fn compute_filter_allowlist(
        &self,
        label: &str,
        filters: Option<&std::collections::HashMap<String, Value>>,
    ) -> Option<std::collections::HashSet<NodeId>> {
        let filters = filters.filter(|f| !f.is_empty())?;

        // Start with all nodes for this label — route via the real backend
        // (substrate in T17 mode) so the filter allowlist contains live node
        // IDs, not dummy-store artefacts.
        let label_nodes: std::collections::HashSet<NodeId> = self
            .data_store()
            .nodes_by_label(label)
            .into_iter()
            .collect();

        let mut allowlist = label_nodes;
        let data = self.data_store();

        for (key, filter_value) in filters {
            // Filtre a operateurs : une Map dont au moins une cle commence par
            // `$` (`$gt`, `$in`, `$ne`...).
            let is_operator_filter = matches!(filter_value, Value::Map(ops) if ops.keys().any(|k| k.as_str().starts_with('$')));

            let matching: std::collections::HashSet<NodeId> = if is_operator_filter {
                // (2026-08-13) CE CHEMIN NE RENDAIT RIEN, DANS AUCUN MODE.
                //
                // Il appelait `GraphStore::find_nodes_matching_filter`, une
                // methode de trait declaree avec un corps par defaut
                // `Vec::new()` et QUE PERSONNE N'IMPLEMENTAIT — ni LpgStore ni
                // SubstrateStore. Tout filtre a operateur rendait donc zero
                // resultat, silencieusement, depuis toujours : la recherche
                // « reussissait » en ne trouvant rien, ce qui se lit comme
                // « aucun voisin ne correspond » et non comme une panne.
                //
                // La methode de trait est SUPPRIMEE (voir traits.rs) et
                // l'evaluation faite ici, sur les noeuds du label deja en
                // main : pas de balayage global, et le meme comportement pour
                // tous les backends.
                let prop = obrain_common::types::PropertyKey::new(key);
                allowlist
                    .iter()
                    .filter(|id| {
                        data.get_node_property(**id, &prop)
                            .is_some_and(|v| value_matches_operators(&v, filter_value))
                    })
                    .copied()
                    .collect()
            } else {
                // Egalite : passer par le VRAI backend. `self.store` est le
                // store fantoche en mode substrat — l'interroger rendait un
                // ensemble vide, donc une liste blanche vide, donc zero
                // resultat. Meme famille de bug que ci-dessus.
                data.find_nodes_by_property(key, filter_value)
                    .into_iter()
                    .collect()
            };
            allowlist = allowlist.intersection(&matching).copied().collect();

            // Short-circuit: empty intersection means no results possible
            if allowlist.is_empty() {
                return Some(allowlist);
            }
        }

        Some(allowlist)
    }

    /// Searches for the k nearest neighbors of a query vector.
    ///
    /// Uses the HNSW index created by [`create_vector_index`](Self::create_vector_index).
    ///
    /// # Arguments
    ///
    /// * `label` - Node label that was indexed
    /// * `property` - Property that was indexed
    /// * `query` - Query vector (slice of floats)
    /// * `k` - Number of nearest neighbors to return
    /// * `ef` - Search beam width (higher = better recall, slower). Uses index default if `None`.
    /// * `filters` - Optional property equality filters. Only nodes matching all
    ///   `(key, value)` pairs will appear in results.
    ///
    /// # Returns
    ///
    /// Vector of `(NodeId, distance)` pairs sorted by distance ascending.
    #[cfg(feature = "vector-index")]
    pub fn vector_search(
        &self,
        label: &str,
        property: &str,
        query: &[f32],
        k: usize,
        ef: Option<usize>,
        filters: Option<&std::collections::HashMap<String, Value>>,
    ) -> Result<Vec<(obrain_common::types::NodeId, f32)>> {
        // T17 W3c slice 5c: vector-index operations live on `LpgStore` and are
        // absent from `SubstrateStore`. Substrate provides its own retrieval
        // path via `SubstrateTieredIndex` — see `ObrainDB::substrate_handle()`
        // and the `obrain_substrate::retrieval` module. Fail loudly instead of
        // silently returning "no index" against the dummy store.
        // (2026-08-13, T17b) LE REFUS DEVIENT UNE RECHERCHE.
        //
        // Le registre vit sur `ObrainDB` — voir le champ
        // `substrate_vector_indexes` et le long commentaire qui explique
        // pourquoi ni le trait ni `SubstrateStore` n'etaient le bon endroit.
        if self.substrate_store.is_some() {
            let _ = ef; // propre a HNSW ; le cascade L0/L1/L2 n'en a pas besoin
            let cle = format!("{label}:{property}");

            // Verrou en LECTURE, et l'Arc est clone AVANT de le relacher : la
            // recherche elle-meme se fait hors verrou, donc N recherches
            // concurrentes sur le meme index ne se serialisent pas, et une
            // creation d'index concurrente ne les bloque pas non plus.
            // (2026-08-14) RECONSTRUCTION A LA DEMANDE. Sur une base rouverte,
            // le registre est vide alors que l'index a bien ete cree un jour :
            // c'est le catalogue durable qui s'en souvient. On le rebatit ici,
            // une seule fois, plutot qu'a l'ouverture — construire un cascade
            // en 384 dimensions au demarrage d'une base de 151 Go serait un
            // cout non borne payé a chaque redemarrage du watchdog.
            let absent = !self.substrate_vector_indexes.read().contains_key(&cle);
            if absent {
                self.rebatir_index_declare("vector", label, property);
            }
            let (dim_native, index) = {
                let registre = self.substrate_vector_indexes.read();
                match registre.get(&cle) {
                    Some((d, i)) => (*d, std::sync::Arc::clone(i)),
                    None => {
                        return Err(obrain_common::utils::error::Error::Internal(format!(
                            "Aucun index vectoriel pour :{label}({property}). \
                             Appelle create_vector_index() d'abord."
                        )));
                    }
                }
            };

            // On valide sur la dimension NATIVE, pas sur celle du cascade.
            // Rembourrer une requete de largeur quelconque jusqu'a 384
            // « marcherait » toujours — et interrogerait un index construit sur
            // de tout autres embeddings sans le dire. C'est exactement le genre
            // de succes silencieux qu'on passe la soiree a retirer.
            if dim_native != query.len() {
                return Err(obrain_common::utils::error::Error::Internal(format!(
                    "Dimension incoherente pour :{label}({property}) : l'index est en \
                     {dim_native} dimensions, la requete en {}.",
                    query.len()
                )));
            }
            let query_plein = super::rembourrer_au_cascade(query).ok_or_else(|| {
                obrain_common::utils::error::Error::Internal(format!(
                    "Requete de {} dimensions au-dela de la largeur du cascade ({}).",
                    query.len(),
                    obrain_substrate::L2_DIM
                ))
            })?;
            let query = query_plein.as_slice();

            // Avec des filtres, on sur-echantillonne : filtrer APRES un top-k
            // exact rendrait moins de k resultats des qu'un seul voisin est
            // ecarte. Facteur 4, borne a 1024 pour ne pas degenerer en scan.
            let k_brut = if filters.is_some() {
                (k.saturating_mul(4)).min(1024).max(k)
            } else {
                k
            };

            // La liste blanche est calculee UNE fois, pas par candidat : c'est
            // le meme helper que le chemin HNSW, deja eprouve.
            let permis = self.compute_filter_allowlist(label, filters);

            let bruts = index.search_top_k(query, k_brut);
            let mut out: Vec<(obrain_common::types::NodeId, f32)> = Vec::with_capacity(k);
            for (offset, cosinus) in bruts {
                let id = obrain_common::types::NodeId(u64::from(offset));
                if let Some(ref set) = permis
                    && !set.contains(&id)
                {
                    continue;
                }
                // Le contrat de cette fonction est une DISTANCE croissante ;
                // le cascade rend une similarite cosinus decroissante.
                out.push((id, 1.0 - cosinus));
                if out.len() == k {
                    break;
                }
            }
            return Ok(out);
        }

        let index = self.store.get_vector_index(label, property).ok_or_else(|| {
            obrain_common::utils::error::Error::Internal(format!(
                "No vector index found for :{label}({property}). Call create_vector_index() first."
            ))
        })?;

        let accessor =
            obrain_core::index::vector::PropertyVectorAccessor::new(&*self.store, property);

        let results = match self.compute_filter_allowlist(label, filters) {
            Some(allowlist) => match ef {
                Some(ef_val) => {
                    index.search_with_ef_and_filter(query, k, ef_val, &allowlist, &accessor)
                }
                None => index.search_with_filter(query, k, &allowlist, &accessor),
            },
            None => match ef {
                Some(ef_val) => index.search_with_ef(query, k, ef_val, &accessor),
                None => index.search(query, k, &accessor),
            },
        };

        Ok(results)
    }

    /// Searches for nearest neighbors for multiple query vectors in parallel.
    ///
    /// Uses rayon parallel iteration under the hood for multi-core throughput.
    ///
    /// # Arguments
    ///
    /// * `label` - Node label that was indexed
    /// * `property` - Property that was indexed
    /// * `queries` - Batch of query vectors
    /// * `k` - Number of nearest neighbors per query
    /// * `ef` - Search beam width (uses index default if `None`)
    /// * `filters` - Optional property equality filters
    #[cfg(feature = "vector-index")]
    pub fn batch_vector_search(
        &self,
        label: &str,
        property: &str,
        queries: &[Vec<f32>],
        k: usize,
        ef: Option<usize>,
        filters: Option<&std::collections::HashMap<String, Value>>,
    ) -> Result<Vec<Vec<(obrain_common::types::NodeId, f32)>>> {
        // (2026-08-13, T17b) On se ramene a `vector_search`, qui porte deja la
        // verification de dimension, la liste blanche des filtres et la
        // conversion cosinus -> distance. Dupliquer cette logique ici serait
        // creer une seconde verite a maintenir.
        //
        // Pas de parallelisme interne : le cascade est deja rapide et les
        // appelants qui veulent du parallelisme le font a leur niveau, ou ils
        // savent combien de threads ils peuvent prendre. En decider ici
        // reviendrait a s'arroger un budget de threads qu'on ne connait pas.
        if self.substrate_store.is_some() {
            let mut sorties = Vec::with_capacity(queries.len());
            for q in queries {
                sorties.push(self.vector_search(label, property, q, k, ef, filters)?);
            }
            return Ok(sorties);
        }

        let index = self.store.get_vector_index(label, property).ok_or_else(|| {
            obrain_common::utils::error::Error::Internal(format!(
                "No vector index found for :{label}({property}). Call create_vector_index() first."
            ))
        })?;

        let accessor =
            obrain_core::index::vector::PropertyVectorAccessor::new(&*self.store, property);

        let results = match self.compute_filter_allowlist(label, filters) {
            Some(allowlist) => match ef {
                Some(ef_val) => {
                    index.batch_search_with_ef_and_filter(queries, k, ef_val, &allowlist, &accessor)
                }
                None => index.batch_search_with_filter(queries, k, &allowlist, &accessor),
            },
            None => match ef {
                Some(ef_val) => index.batch_search_with_ef(queries, k, ef_val, &accessor),
                None => index.batch_search(queries, k, &accessor),
            },
        };

        Ok(results)
    }

    /// Searches for diverse nearest neighbors using Maximal Marginal Relevance (MMR).
    ///
    /// MMR balances relevance (similarity to query) with diversity (dissimilarity
    /// among selected results). This is the algorithm used by LangChain's
    /// `mmr_traversal_search()` for RAG applications.
    ///
    /// # Arguments
    ///
    /// * `label` - Node label that was indexed
    /// * `property` - Property that was indexed
    /// * `query` - Query vector
    /// * `k` - Number of diverse results to return
    /// * `fetch_k` - Number of initial candidates from HNSW (default: `4 * k`)
    /// * `lambda` - Relevance vs. diversity in \[0, 1\] (default: 0.5).
    ///   1.0 = pure relevance, 0.0 = pure diversity.
    /// * `ef` - HNSW search beam width (uses index default if `None`)
    /// * `filters` - Optional property equality filters
    ///
    /// # Returns
    ///
    /// `(NodeId, distance)` pairs in MMR selection order. The f32 is the original
    /// distance from the query, matching [`vector_search`](Self::vector_search).
    #[cfg(feature = "vector-index")]
    #[allow(clippy::too_many_arguments)]
    pub fn mmr_search(
        &self,
        label: &str,
        property: &str,
        query: &[f32],
        k: usize,
        fetch_k: Option<usize>,
        lambda: Option<f32>,
        ef: Option<usize>,
        filters: Option<&std::collections::HashMap<String, Value>>,
    ) -> Result<Vec<(obrain_common::types::NodeId, f32)>> {
        use obrain_core::index::vector::mmr_select;

        // (2026-08-13, T17b) MMR sur le cascade du substrat.
        //
        // Etape 1 identique au chemin HNSW : on tire `fetch_k` candidats — ici
        // par `vector_search`, qui applique deja filtres et dimension. Etape 2
        // identique aussi : `mmr_select`, le meme helper de obrain-core. Seule
        // la SOURCE des candidats change, pas la diversification.
        if self.substrate_store.is_some() {
            use obrain_core::index::vector::VectorAccessor;
            let fetch_k = fetch_k.unwrap_or(k.saturating_mul(4).max(k));
            let lambda = lambda.unwrap_or(0.5);

            let initiaux = self.vector_search(label, property, query, fetch_k, ef, filters)?;
            if initiaux.is_empty() {
                return Ok(Vec::new());
            }

            let accessor =
                obrain_core::index::vector::PropertyVectorAccessor::new(&*self.store, property);
            // Les Arc sont gardes VIVANTS dans `possedes` ; `mmr_select` veut
            // des tranches empruntees, qui pointent dedans. Sans cette etape
            // les Arc seraient liberes avant l'appel.
            let possedes: Vec<(obrain_common::types::NodeId, f32, std::sync::Arc<[f32]>)> =
                initiaux
                    .into_iter()
                    .filter_map(|(id, dist)| accessor.get_vector(id).map(|v| (id, dist, v)))
                    .collect();
            let candidats: Vec<(obrain_common::types::NodeId, f32, &[f32])> = possedes
                .iter()
                .map(|(id, d, v)| (*id, *d, v.as_ref()))
                .collect();

            // La metrique du cascade est le cosinus — c'est ce que
            // `search_top_k` rend, et ce dont on a derive la distance.
            return Ok(mmr_select(
                query,
                &candidats,
                k,
                lambda,
                obrain_core::index::vector::DistanceMetric::Cosine,
            ));
        }

        let index = self.store.get_vector_index(label, property).ok_or_else(|| {
            obrain_common::utils::error::Error::Internal(format!(
                "No vector index found for :{label}({property}). Call create_vector_index() first."
            ))
        })?;

        let accessor =
            obrain_core::index::vector::PropertyVectorAccessor::new(&*self.store, property);

        let fetch_k = fetch_k.unwrap_or(k.saturating_mul(4).max(k));
        let lambda = lambda.unwrap_or(0.5);

        // Step 1: Fetch candidates from HNSW (with optional filter)
        let initial_results = match self.compute_filter_allowlist(label, filters) {
            Some(allowlist) => match ef {
                Some(ef_val) => {
                    index.search_with_ef_and_filter(query, fetch_k, ef_val, &allowlist, &accessor)
                }
                None => index.search_with_filter(query, fetch_k, &allowlist, &accessor),
            },
            None => match ef {
                Some(ef_val) => index.search_with_ef(query, fetch_k, ef_val, &accessor),
                None => index.search(query, fetch_k, &accessor),
            },
        };

        if initial_results.is_empty() {
            return Ok(Vec::new());
        }

        // Step 2: Retrieve stored vectors for MMR pairwise comparison
        use obrain_core::index::vector::VectorAccessor;
        let candidates: Vec<(obrain_common::types::NodeId, f32, std::sync::Arc<[f32]>)> =
            initial_results
                .into_iter()
                .filter_map(|(id, dist)| accessor.get_vector(id).map(|vec| (id, dist, vec)))
                .collect();

        // Step 3: Build slice-based candidates for mmr_select
        let candidate_refs: Vec<(obrain_common::types::NodeId, f32, &[f32])> = candidates
            .iter()
            .map(|(id, dist, vec)| (*id, *dist, vec.as_ref()))
            .collect();

        // Step 4: Run MMR selection
        let metric = index.config().metric;
        Ok(mmr_select(query, &candidate_refs, k, lambda, metric))
    }

    /// Searches a text index using BM25 scoring.
    ///
    /// Returns up to `k` results as `(NodeId, score)` pairs sorted by
    /// descending relevance score.
    ///
    /// # Errors
    ///
    /// Returns an error if no text index exists for this label+property.
    /// (V4) Federation de recherche inter-bases — fan-out SEQUENTIEL sur
    /// les bases fournies + fusion des scores (tri global decroissant,
    /// top-k). Chaque resultat est etiquete du slug de sa base.
    ///
    /// - L'APPELANT fournit les bases (le service tient le pool) — aucun
    ///   scan implicite d'un registre ;
    /// - la garde de droits (V3) s'applique PAR BASE : une base refusee au
    ///   lecteur est simplement absente du resultat (pas d'oracle), comptee
    ///   dans access_denied_refs si un registre est passe ;
    /// - une base sans index texte est SAUTEE avec un avertissement — la
    ///   federation ne meurt pas d'une base mal equipee ;
    /// - fan-out sequentiel V4 (les bases locales repondent en ms) —
    ///   parallelisation = optimisation documentee en reste.
    ///
    /// # Errors
    ///
    /// Ne rend d'erreur que si TOUTES les bases interrogees echouent.
    #[cfg(feature = "text-index")]
    pub fn text_search_federated(
        bases: &[(&str, &Self)],
        access: Option<&obrain_core::execution::AccessContext>,
        label: &str,
        property: &str,
        query: &str,
        k: usize,
    ) -> Result<Vec<(String, NodeId, f64)>> {
        let mut fusion: Vec<(String, NodeId, f64)> = Vec::new();
        let mut erreurs: Vec<String> = Vec::new();
        let mut interrogees = 0usize;
        for (slug, db) in bases {
            if let Some(ctx) = access
                && !ctx.allows(slug)
            {
                // Refus de droit = base absente du resultat (trivalue).
                if let Some(reg) = db.base_registry() {
                    reg.access_denied_refs
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                continue;
            }
            interrogees += 1;
            match db.text_search(label, property, query, k) {
                Ok(resultats) => {
                    fusion.extend(
                        resultats
                            .into_iter()
                            .map(|(id, score)| ((*slug).to_string(), id, score)),
                    );
                }
                Err(e) => {
                    tracing::warn!(base = %slug, erreur = %e,
                        "federation de recherche : base sautee");
                    erreurs.push(format!("{slug}: {e}"));
                }
            }
        }
        if interrogees > 0 && erreurs.len() == interrogees {
            return Err(Error::Internal(format!(
                "recherche federee : toutes les bases ont echoue — {}",
                erreurs.join(" ; ")
            )));
        }
        fusion.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        fusion.truncate(k);
        Ok(fusion)
    }

    #[cfg(feature = "text-index")]
    pub fn text_search(
        &self,
        label: &str,
        property: &str,
        query: &str,
        k: usize,
    ) -> Result<Vec<(NodeId, f64)>> {
        // T17 W3c slice 5c: text-index operations are LpgStore-inherent.
        // Substrate BM25 support is tracked as part of the retrieval-API
        // unification (T17b).
        // (2026-08-13, T17b) Le pendant de `create_text_index` : on consulte
        // le registre porte par la base au lieu de refuser.
        if self.substrate_store.is_some() {
            let cle = format!("{label}:{property}");
            // Arc clone SOUS lecture partagee, puis verrou relache : la
            // recherche BM25 elle-meme se fait hors du verrou de registre.
            // Meme reconstruction a la demande que pour le vectoriel.
            let absent = !self.substrate_text_indexes.read().contains_key(&cle);
            if absent {
                self.rebatir_index_declare("text", label, property);
            }
            let index = {
                let registre = self.substrate_text_indexes.read();
                match registre.get(&cle) {
                    Some(i) => std::sync::Arc::clone(i),
                    None => {
                        return Err(Error::Internal(format!(
                            "Aucun index texte pour :{label}({property}). \
                             Appelle create_text_index() d'abord."
                        )));
                    }
                }
            };
            let resultats = index.read().search(query, k);
            return Ok(resultats);
        }

        let index = self.store.get_text_index(label, property).ok_or_else(|| {
            Error::Internal(format!(
                "No text index found for :{label}({property}). Call create_text_index() first."
            ))
        })?;

        Ok(index.read().search(query, k))
    }

    /// Performs hybrid search combining text (BM25) and vector similarity.
    ///
    /// Runs both text search and vector search, then fuses results using
    /// the specified method (default: Reciprocal Rank Fusion).
    ///
    /// # Arguments
    ///
    /// * `label` - Node label to search within
    /// * `text_property` - Property indexed for text search
    /// * `vector_property` - Property indexed for vector search
    /// * `query_text` - Text query for BM25 search
    /// * `query_vector` - Vector query for similarity search (optional)
    /// * `k` - Number of results to return
    /// * `fusion` - Score fusion method (default: RRF with k=60)
    ///
    /// # Errors
    ///
    /// Returns an error if the required indexes don't exist.
    #[cfg(feature = "hybrid-search")]
    #[allow(clippy::too_many_arguments)]
    pub fn hybrid_search(
        &self,
        label: &str,
        text_property: &str,
        vector_property: &str,
        query_text: &str,
        query_vector: Option<&[f32]>,
        k: usize,
        fusion: Option<obrain_core::index::text::FusionMethod>,
    ) -> Result<Vec<(NodeId, f64)>> {
        use obrain_core::index::text::fuse_results;

        // (2026-08-13, T17b) LA DERNIERE GARDE.
        //
        // La fusion elle-meme ne change pas : c'est `fuse_results`, le meme
        // helper que le chemin LpgStore. Seules les deux SOURCES changent, et
        // elles existent deja — `text_search` et `vector_search` sont branchees
        // sur les registres du substrat.
        //
        // POINT DE SEMANTIQUE, celui qui demandait de la conception plutot que
        // du cablage. Le chemin LpgStore est TOLERANT : si un index manque, il
        // saute simplement cette source (`if let Some(index) = ...`). C'est le
        // contrat de la recherche hybride — fusionner ce qui est disponible —
        // et non un oubli.
        //
        // Mais `text_search` et `vector_search`, elles, RENDENT UNE ERREUR
        // quand l'index manque : c'est ce qu'on veut quand on les appelle
        // directement. Les envelopper dans un `.ok()` ici avalerait AUSSI les
        // vraies pannes — dimension incoherente, requete malformee — et on
        // passerait la soiree a retirer des succes silencieux pour en
        // reintroduire un au dernier moment.
        //
        // On distingue donc les deux cas EXPLICITEMENT : on regarde d'abord si
        // le registre porte l'index (absence = source sautee, comme en LPG), et
        // seulement alors on appelle la recherche — dont l'erreur, si elle
        // survient, REMONTE.
        if self.substrate_store.is_some() {
            let fusion_method = fusion.unwrap_or_default();
            let mut sources: Vec<Vec<(NodeId, f64)>> = Vec::new();
            let sur_echantillon = k.saturating_mul(2).max(k);

            let a_index_texte = self
                .substrate_text_indexes
                .read()
                .contains_key(&format!("{label}:{text_property}"));
            if a_index_texte {
                let resultats =
                    self.text_search(label, text_property, query_text, sur_echantillon)?;
                if !resultats.is_empty() {
                    sources.push(resultats);
                }
            }

            if let Some(query_vec) = query_vector {
                let a_index_vecteur = self
                    .substrate_vector_indexes
                    .read()
                    .contains_key(&format!("{label}:{vector_property}"));
                if a_index_vecteur {
                    let resultats = self.vector_search(
                        label,
                        vector_property,
                        query_vec,
                        sur_echantillon,
                        None,
                        None,
                    )?;
                    if !resultats.is_empty() {
                        sources.push(
                            resultats
                                .into_iter()
                                .map(|(id, dist)| (id, f64::from(dist)))
                                .collect(),
                        );
                    }
                }
            }

            if sources.is_empty() {
                return Ok(Vec::new());
            }
            return Ok(fuse_results(&sources, &fusion_method, k));
        }

        let fusion_method = fusion.unwrap_or_default();
        let mut sources: Vec<Vec<(NodeId, f64)>> = Vec::new();

        // Text search
        if let Some(text_index) = self.store.get_text_index(label, text_property) {
            let text_results = text_index.read().search(query_text, k * 2);
            if !text_results.is_empty() {
                sources.push(text_results);
            }
        }

        // Vector search (if query vector provided)
        if let Some(query_vec) = query_vector
            && let Some(vector_index) = self.store.get_vector_index(label, vector_property)
        {
            let accessor = obrain_core::index::vector::PropertyVectorAccessor::new(
                &*self.store,
                vector_property,
            );
            let vector_results = vector_index.search(query_vec, k * 2, &accessor);
            if !vector_results.is_empty() {
                sources.push(
                    vector_results
                        .into_iter()
                        .map(|(id, dist)| (id, f64::from(dist)))
                        .collect(),
                );
            }
        }

        if sources.is_empty() {
            return Ok(Vec::new());
        }

        Ok(fuse_results(&sources, &fusion_method, k))
    }
}
