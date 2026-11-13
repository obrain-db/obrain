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
#[cfg(feature = "vector-index")]
use obrain_substrate::VectorIndex as _;

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
#[cfg(feature = "vector-index")]
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
            // `ef` = largeur de candidats du cascade (k0 ; k1 = k0 / 10),
            // comme le `ef` d'un HNSW : plus large, plus de recall, plus de
            // latence. `None` = largeurs par defaut de l'index (4 000 / 400).
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
            // La requete part en dimension NATIVE : l'index la projette lui-meme
            // (O-VEC C4) et rerank les candidats sur les vecteurs natifs.

            // La liste blanche est calculee UNE fois, pas par candidat.
            let permis = self.compute_filter_allowlist(label, filters);

            // Recherche filtrée EXACTE. Filtrer après un top-k fixe rendait 0
            // résultat valide dès que les voisins les plus proches étaient tous
            // hors liste blanche (données groupées : les 40 premiers voisins
            // étaient tous du groupe 0, filtre sur le groupe 1, mesuré le
            // 11/11). On élargit donc le top-k par facteur 4 tant qu'il reste
            // moins de k résultats permis ET que l'index n'est pas épuisé. La
            // boucle s'arrête dès que k résultats sont trouvés : le coût ne croît
            // que pour les filtres sélectifs.
            let total = index.len();
            let mut k_brut = if permis.is_some() {
                k.saturating_mul(4).max(k).min(total.max(k))
            } else {
                k
            };
            loop {
                let bruts = match ef {
                    Some(largeur) => index.search_top_k_ef(query, k_brut, largeur),
                    None => index.search_top_k(query, k_brut),
                };
                let mut out: Vec<(obrain_common::types::NodeId, f32)> = Vec::with_capacity(k);
                for (offset, cosinus) in bruts {
                    let id = obrain_common::types::NodeId(u64::from(offset));
                    if let Some(ref set) = permis
                        && !set.contains(&id)
                    {
                        continue;
                    }
                    // Le contrat de cette fonction est une DISTANCE croissante ;
                    // le cascade rend une similarité cosinus décroissante.
                    out.push((id, 1.0 - cosinus));
                    if out.len() == k {
                        break;
                    }
                }
                if out.len() == k || permis.is_none() || k_brut >= total {
                    return Ok(out);
                }
                k_brut = k_brut.saturating_mul(4).min(total);
            }
        }

        Err(obrain_common::utils::error::Error::Internal(format!(
            "index vectoriel absent pour :{label}({property}) : registre hors base substrat supprimé (O-VEC nettoyage)"
        )))
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

        Err(obrain_common::utils::error::Error::Internal(format!(
            "index vectoriel absent pour :{label}({property}) : registre hors base substrat supprimé (O-VEC nettoyage)"
        )))
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

        Err(obrain_common::utils::error::Error::Internal(format!(
            "index vectoriel absent pour :{label}({property}) : registre hors base substrat supprimé (O-VEC nettoyage)"
        )))
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

    /// Recherche BM25 dans l'index texte du couple (`label`, `property`) :
    /// rend au plus `k` nœuds avec leur score. Sur une base substrate, un index
    /// déclaré mais absent du registre est reconstruit à la demande.
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
        // (2026-08-13, T17b) LA DERNIERE GARDE — et (2026-11-12) UNE SEULE
        // FUSION.
        //
        // Le contrat de l'API Rust ne change pas : TOLERANTE, elle fusionne
        // ce qui est disponible (un index absent = une source sautee, comme
        // en LPG). Mais la fusion elle-meme vit desormais dans la POIGNEE
        // (`PoigneeRechercheHybride`), la meme que celle qu'interroge
        // `CALL obrain.hybrid_search` depuis le planificateur : deux chemins,
        // un seul calcul, un seul endroit ou la fusion peut se tromper.
        //
        // Ce que l'API Rust fait EN PLUS de la procedure, et qu'elle seule
        // peut faire parce qu'elle tient `&self` : la reconstruction a la
        // demande d'un index declare mais absent du registre (base rouverte),
        // comme `text_search` et `vector_search`. La procedure, elle, nomme
        // ce cas au lieu de le sauter.
        if self.substrate_store.is_some() {
            let cle_texte = format!("{label}:{text_property}");
            if !self.substrate_text_indexes.read().contains_key(&cle_texte) {
                self.rebatir_index_declare("text", label, text_property);
            }
            if query_vector.is_some() {
                let cle_vecteur = format!("{label}:{vector_property}");
                if !self
                    .substrate_vector_indexes
                    .read()
                    .contains_key(&cle_vecteur)
                {
                    self.rebatir_index_declare("vector", label, vector_property);
                }
            }
        }

        self.poignee_recherche_hybride().hybrid_search(
            label,
            text_property,
            vector_property,
            query_text,
            query_vector,
            k,
            fusion,
        )
    }

    /// La poignée de recherche hybride de cette base : des clones `Arc` des
    /// registres d'index et du store, câblés jusqu'au planificateur par
    /// `ObrainDB::session()` (voir `database/hybrid_procedure.rs`).
    #[cfg(feature = "hybrid-search")]
    pub(crate) fn poignee_recherche_hybride(
        &self,
    ) -> std::sync::Arc<dyn super::hybrid_procedure::RechercheHybride> {
        std::sync::Arc::new(PoigneeRechercheHybride {
            substrate_store: self.substrate_store.clone(),
            store: std::sync::Arc::clone(&self.store),
            textes: std::sync::Arc::clone(&self.substrate_text_indexes),
            vecteurs: std::sync::Arc::clone(&self.substrate_vector_indexes),
        })
    }
}

/// Ce que `CALL obrain.hybrid_search` et `ObrainDB::hybrid_search` partagent :
/// les registres d'index (clones `Arc`), le store substrat (pour savoir si un
/// index absent est tout de même DÉCLARÉ) et le store de repli hors substrat.
///
/// Vit sans `ObrainDB` : c'est ce qui permet à une `Session` (qui n'a pas
/// d'`Arc<ObrainDB>`) de la porter jusqu'au planificateur.
#[cfg(feature = "hybrid-search")]
pub(crate) struct PoigneeRechercheHybride {
    substrate_store: Option<std::sync::Arc<obrain_substrate::SubstrateStore>>,
    store: std::sync::Arc<dyn obrain_core::graph::GraphStoreMut>,
    textes: std::sync::Arc<
        parking_lot::RwLock<
            std::collections::HashMap<
                String,
                std::sync::Arc<parking_lot::RwLock<obrain_core::index::text::InvertedIndex>>,
            >,
        >,
    >,
    vecteurs: std::sync::Arc<
        parking_lot::RwLock<
            std::collections::HashMap<
                String,
                (
                    usize,
                    std::sync::Arc<obrain_substrate::SubstrateTieredIndex>,
                ),
            >,
        >,
    >,
}

#[cfg(feature = "hybrid-search")]
impl PoigneeRechercheHybride {
    /// L'index `(genre, label, property)` est-il déclaré dans le catalogue
    /// durable du substrat ? (Une base en mémoire n'en a pas : `false`.)
    fn declare(&self, genre: &str, label: &str, property: &str) -> bool {
        self.substrate_store.as_ref().is_some_and(|sub| {
            sub.declarations_index()
                .iter()
                .any(|d| d.genre == genre && d.label == label && d.propriete == property)
        })
    }
}

#[cfg(feature = "hybrid-search")]
impl super::hybrid_procedure::RechercheHybride for PoigneeRechercheHybride {
    fn etat_index_texte(&self, label: &str, property: &str) -> super::hybrid_procedure::EtatIndex {
        use super::hybrid_procedure::EtatIndex;
        if self.substrate_store.is_some() {
            if self
                .textes
                .read()
                .contains_key(&format!("{label}:{property}"))
            {
                EtatIndex::Charge
            } else if self.declare("text", label, property) {
                EtatIndex::DeclareNonCharge
            } else {
                EtatIndex::Absent
            }
        } else if self.store.get_text_index(label, property).is_some() {
            EtatIndex::Charge
        } else {
            EtatIndex::Absent
        }
    }

    fn etat_index_vectoriel(
        &self,
        label: &str,
        property: &str,
    ) -> super::hybrid_procedure::EtatIndex {
        use super::hybrid_procedure::EtatIndex;
        if self.substrate_store.is_none() {
            // Hors substrat, il n'existe aucun registre vectoriel (O-VEC
            // nettoyage) : la recherche hybride se réduit à son volet texte.
            return EtatIndex::Absent;
        }
        if self
            .vecteurs
            .read()
            .contains_key(&format!("{label}:{property}"))
        {
            EtatIndex::Charge
        } else if self.declare("vector", label, property) {
            EtatIndex::DeclareNonCharge
        } else {
            EtatIndex::Absent
        }
    }

    fn hybrid_search(
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

        let fusion_method = fusion.unwrap_or_default();
        let mut sources: Vec<Vec<(NodeId, f64)>> = Vec::new();
        // Chaque source est sur-échantillonnée (2k) pour que la fusion ait de
        // quoi reclasser ; `fuse_results` tronque à k.
        let sur_echantillon = k.saturating_mul(2).max(k);

        // Texte (BM25). L'Arc est cloné SOUS le verrou de registre, puis le
        // verrou relâché : la recherche se fait hors verrou.
        let index_texte = if self.substrate_store.is_some() {
            self.textes
                .read()
                .get(&format!("{label}:{text_property}"))
                .cloned()
        } else {
            self.store.get_text_index(label, text_property)
        };
        if let Some(index) = index_texte {
            let resultats = index.read().search(query_text, sur_echantillon);
            if !resultats.is_empty() {
                sources.push(resultats);
            }
        }

        // Vecteur (cosinus par le cascade), substrat seulement.
        if let Some(query) = query_vector
            && self.substrate_store.is_some()
        {
            let entree = self
                .vecteurs
                .read()
                .get(&format!("{label}:{vector_property}"))
                .cloned();
            if let Some((dim_native, index)) = entree {
                // Même garde que `vector_search` : on valide sur la dimension
                // NATIVE, jamais en rembourrant.
                if dim_native != query.len() {
                    return Err(Error::Internal(format!(
                        "Dimension incoherente pour :{label}({vector_property}) : l'index est en \
                         {dim_native} dimensions, la requete en {}.",
                        query.len()
                    )));
                }
                // SCORE, pas distance : la fusion pondérée normalise des
                // scores CROISSANTS (min-max), comme ceux de BM25. Le RRF, lui,
                // ne voit que le rang, identique dans les deux sens.
                let resultats: Vec<(NodeId, f64)> = index
                    .search_top_k(query, sur_echantillon)
                    .into_iter()
                    .map(|(offset, cosinus)| (NodeId(u64::from(offset)), f64::from(cosinus)))
                    .collect();
                if !resultats.is_empty() {
                    sources.push(resultats);
                }
            }
        }

        if sources.is_empty() {
            return Ok(Vec::new());
        }
        Ok(fuse_results(&sources, &fusion_method, k))
    }
}
