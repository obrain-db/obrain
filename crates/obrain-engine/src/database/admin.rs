//! Admin, introspection, and diagnostic operations for ObrainDB.

use std::path::Path;

use obrain_common::utils::error::Result;

impl super::ObrainDB {
    // =========================================================================
    // ADMIN API: Counts
    // =========================================================================

    /// Returns the number of nodes in the database.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.data_store().node_count()
    }

    /// Returns the number of edges in the database.
    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.data_store().edge_count()
    }

    /// Comptage O(1) PAR LABEL (`label_live_counts` substrate via
    /// `data_store()`) — le `count(n:Label)` ne balaie pas.
    #[must_use]
    pub fn node_count_by_label(&self, label: &str) -> u64 {
        self.data_store().node_count_by_label(label) as u64
    }

    /// INGESTION NATIVE zone-direct (RFC §16.1) : délègue au bulk substrate
    /// (zones par blocs, chaînage une passe, compteurs O(1)) — PAS de WAL par
    /// op. Store substrate supposé NEUF. Retourne (n,m).
    pub fn bulk_import(
        &self,
        labels: &[String],
        edge_src: &[u32],
        edge_dst: &[u32],
        edge_types: &[String],
    ) -> Result<(u32, u64)> {
        let Some(store) = self.substrate_store.as_ref() else {
            return Err(obrain_common::utils::error::Error::Internal(
                "bulk_import requires a substrate-backed database".into(),
            ));
        };
        let label_refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
        let type_refs: Vec<&str> = edge_types.iter().map(|s| s.as_str()).collect();
        let edges: Vec<(u32, u32, &str)> = edge_src
            .iter()
            .zip(edge_dst.iter())
            .zip(type_refs.iter())
            .map(|((&s, &d), &t)| (s, d, t))
            .collect();
        store
            .bulk_import_labels_edges(&label_refs, &edges)
            .map_err(|e| obrain_common::utils::error::Error::Internal(format!("bulk_import: {e}")))
    }

    /// (T27) [`Self::bulk_import`] sans copie : libellés et types d'arêtes
    /// par référence. Sur le noyau Linux, la variante `String` allouait
    /// 1,55 M + 4,78 M chaînes pour quelques dizaines de valeurs distinctes.
    pub fn bulk_import_refs(
        &self,
        labels: &[&str],
        edge_src: &[u32],
        edge_dst: &[u32],
        edge_types: &[&str],
    ) -> Result<(u32, u64)> {
        let Some(store) = self.substrate_store.as_ref() else {
            return Err(obrain_common::utils::error::Error::Internal(
                "bulk_import requires a substrate-backed database".into(),
            ));
        };
        let edges: Vec<(u32, u32, &str)> = edge_src
            .iter()
            .zip(edge_dst.iter())
            .zip(edge_types.iter())
            .map(|((&s, &d), &t)| (s, d, t))
            .collect();
        store
            .bulk_import_labels_edges(labels, &edges)
            .map_err(|e| obrain_common::utils::error::Error::Internal(format!("bulk_import: {e}")))
    }

    /// INGESTION NATIVE DES PROPRIÉTÉS (T27) — complément de
    /// [`Self::bulk_import`] : `props[i]` = propriétés du nœud de slot
    /// `premier_slot + i`. Même contrat (base neuve, checkpoint à la fin) ;
    /// voir `SubstrateStore::bulk_import_node_props`.
    pub fn bulk_import_props(
        &self,
        premier_slot: u32,
        props: &[Vec<(&str, obrain_common::types::Value)>],
    ) -> Result<u64> {
        let Some(store) = self.substrate_store.as_ref() else {
            return Err(obrain_common::utils::error::Error::Internal(
                "bulk_import_props requires a substrate-backed database".into(),
            ));
        };
        store
            .bulk_import_node_props(premier_slot, props)
            .map_err(|e| obrain_common::utils::error::Error::Internal(format!("bulk_import_props: {e}")))
    }

    /// (T27) Plusieurs index d'égalité en une passe — voir
    /// `SubstrateStore::creer_index_proprietes`. Sans substrat : clé par clé.
    pub fn create_property_indexes(&self, proprietes: &[&str]) {
        match self.substrate_store.as_ref() {
            Some(store) => store.creer_index_proprietes(proprietes),
            None => {
                for p in proprietes {
                    self.create_property_index(p);
                }
            }
        }
    }

    /// Comptage O(1) PAR TYPE d'arête (compteurs vivants substrate,
    /// `SubstrateStore::live_count_by_edge_type`) — avance structurelle :
    /// un `count(()-[:T]->())` en GQL scannait les arêtes (~20 ms @1,7 M) ;
    /// ce chemin est un load atomique (µs). `None` si la base n'est pas
    /// adossée au substrate (pas de compteurs vivants ici).
    #[must_use]
    pub fn edge_count_by_type(&self, edge_type: &str) -> Option<u64> {
        self.substrate_store
            .as_ref()
            .map(|s| s.live_count_by_edge_type(edge_type))
    }

    /// (NANO) Liste `(etiquette, compte)` en **O(#etiquettes)** — un
    /// `all_labels()` sur l'interneur, puis UN load atomique par etiquette
    /// (`label_live_counts`). Aucun balayage.
    ///
    /// POURQUOI PAS `schema()` : `schema()` assemble AUSSI les comptes par
    /// type d'arete via `edge_counts_by_type()`, qui **balaie toutes les
    /// aretes** (O(#aretes) — le balayage est deliberement conserve la-bas,
    /// les compteurs par type ayant ete mesures faux d'un facteur deux).
    /// Un consommateur qui ne veut que l'histogramme des noeuds paierait
    /// donc un balayage complet des aretes pour rien. Cette operation est
    /// la moitie que l'on peut rendre en temps borne.
    ///
    /// SEMANTIQUE, a ne pas confondre avec `labels(n)[0]` : un noeud
    /// portant DEUX etiquettes est compte dans les DEUX. La somme des
    /// comptes peut donc depasser `node_count()`. Un histogramme GQL bati
    /// sur `labels(n)[0]` attribue au contraire chaque noeud a sa PREMIERE
    /// etiquette seulement : les deux vues different des lors qu'il existe
    /// des noeuds multi-etiquettes.
    ///
    /// Rendu trie par compte decroissant, puis par nom, pour que l'ordre
    /// soit deterministe (l'interneur rend l'ordre des bits).
    #[must_use]
    pub fn label_counts(&self) -> Vec<(String, u64)> {
        let data = self.data_store();
        let mut out: Vec<(String, u64)> = data
            .all_labels()
            .into_iter()
            .map(|nom| {
                let c = data.node_count_by_label(&nom) as u64;
                (nom, c)
            })
            .collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out
    }

    /// Returns the number of distinct labels in the database.
    ///
    /// T17 W3c slice 5b: derived from the trait-safe `all_labels()` so
    /// substrate-backed databases report real counts (previously routed
    /// through the LpgStore-inherent `label_count()` on the dummy store).
    #[must_use]
    pub fn label_count(&self) -> usize {
        self.data_store().all_labels().len()
    }

    /// Returns the number of distinct property keys in the database.
    ///
    /// T17 W3c slice 5b: see `label_count` — routes through the trait's
    /// `all_property_keys()` for substrate parity.
    #[must_use]
    pub fn property_key_count(&self) -> usize {
        self.data_store().all_property_keys().len()
    }

    /// Returns the number of distinct edge types in the database.
    ///
    /// T17 W3c slice 5b: see `label_count` — routes through the trait's
    /// `all_edge_types()` for substrate parity.
    #[must_use]
    pub fn edge_type_count(&self) -> usize {
        self.data_store().all_edge_types().len()
    }

    // =========================================================================
    // ADMIN API: Introspection
    // =========================================================================

    /// Returns true if this database is backed by a file (persistent).
    ///
    /// In-memory databases return false.
    #[must_use]
    pub fn is_persistent(&self) -> bool {
        self.config.path.is_some()
    }

    /// Returns the database file path, if persistent.
    ///
    /// In-memory databases return None.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.config.path.as_deref()
    }

    /// Returns high-level database information.
    ///
    /// Includes node/edge counts, persistence status, and mode (LPG/RDF).
    #[must_use]
    pub fn info(&self) -> crate::admin::DatabaseInfo {
        crate::admin::DatabaseInfo {
            mode: crate::admin::DatabaseMode::Lpg,
            node_count: self.data_store().node_count(),
            edge_count: self.data_store().edge_count(),
            is_persistent: self.is_persistent(),
            path: self.config.path.clone(),
            wal_enabled: self.config.wal_enabled,
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// (mission perf, feature 2.2) FUSION DES DOUBLONS par cle logique —
    /// derniere-version-gagne.
    ///
    /// Pour chaque valeur de `cle` portee par >= 2 noeuds VIVANTS de
    /// `label` : le survivant est le porteur au `updated_at` le plus
    /// recent (RFC3339, ordre lexical ; repli : NodeId le plus grand) ;
    /// les aretes des perdants — entrantes, sortantes ET inter-bases —
    /// sont REMONTEES sur le survivant (dedupliquees : une seule arete
    /// par (type, extremite)), puis les perdants sont supprimes (DETACH).
    ///
    /// - `execute = false` : DRY-RUN pur, rien n est touche, le rapport
    ///   compte et liste.
    /// - `backup` : si execute, exporte AVANT toute fusion les perdants
    ///   (proprietes + aretes) en JSON a ce chemin — le fichier est ecrit
    ///   atomiquement (tmp + rename), la fusion n a lieu qu apres.
    ///
    /// Ne s applique qu aux valeurs de cle de type CHAINE (la regle de
    /// survie des clefs applicatives) — les autres types sont ignores.
    ///
    /// # Errors
    ///
    /// Erreur si le backup demande ne peut pas etre ecrit (la fusion
    /// n est alors PAS executee).
    pub fn fusionner_doublons(
        &self,
        label: &str,
        cle: &str,
        execute: bool,
        backup: Option<&std::path::Path>,
    ) -> Result<crate::admin::RapportFusion> {
        use obrain_common::types::{PropertyKey, Value};
        use std::collections::HashMap;

        let store = self.data_store();
        let pk_cle = PropertyKey::new(cle);
        let pk_maj = PropertyKey::new("updated_at");
        let ids = store.nodes_by_label(label);
        let mut rapport = crate::admin::RapportFusion {
            label: label.to_string(),
            cle: cle.to_string(),
            noeuds_examines: 0,
            groupes: Vec::new(),
            perdants: 0,
            execute,
            backup: None,
        };

        let mut groupes: HashMap<String, Vec<(obrain_common::types::NodeId, String)>> =
            HashMap::new();
        for id in ids {
            let Some(node) = store.get_node(id) else { continue };
            rapport.noeuds_examines += 1;
            let Some(Value::String(v)) = node.properties.get(&pk_cle) else {
                continue;
            };
            let maj = match node.properties.get(&pk_maj) {
                Some(Value::String(m)) => m.to_string(),
                _ => String::new(),
            };
            groupes.entry(v.to_string()).or_default().push((id, maj));
        }

        let mut a_fusionner: Vec<crate::admin::GroupeFusion> = Vec::new();
        for (id_logique, mut porteurs) in groupes {
            if porteurs.len() < 2 {
                continue;
            }
            // Survivant : updated_at max, repli NodeId max (stable).
            porteurs.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.0.cmp(&b.0.0)));
            let (survivant, survivant_maj) = porteurs.last().cloned().expect(">= 2 porteurs");
            let perdants: Vec<obrain_common::types::NodeId> = porteurs
                [..porteurs.len() - 1]
                .iter()
                .map(|(id, _)| *id)
                .collect();
            rapport.perdants += perdants.len();
            a_fusionner.push(crate::admin::GroupeFusion {
                id_logique,
                survivant: survivant.0,
                survivant_maj,
                perdants: perdants.iter().map(|n| n.0).collect(),
                aretes_remontees: 0,
                externes_remontees: 0,
            });
        }
        a_fusionner.sort_by(|a, b| b.perdants.len().cmp(&a.perdants.len()));

        if !execute {
            rapport.groupes = a_fusionner;
            return Ok(rapport);
        }

        // ── BACKUP D ABORD, EN FLUX (atomique) — la fusion n a lieu ──
        // qu apres. JSONL : une ligne d en-tete puis UNE LIGNE PAR
        // PERDANT, ecrites au fil de l eau — la RAM ne porte jamais plus
        // d un perdant a la fois (l ancienne version materialisait tout :
        // 1,25 Go en memoire pour 40k perdants, intenable sur AgentStep
        // et ses millions de lignes).
        if let Some(chemin) = backup {
            use std::io::Write as _;
            if let Some(parent) = chemin.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    obrain_common::utils::error::Error::Internal(format!(
                        "backup fusion: mkdir {}: {e}",
                        parent.display()
                    ))
                })?;
            }
            let tmp = chemin.with_extension("tmp");
            let mut flux = std::io::BufWriter::new(std::fs::File::create(&tmp).map_err(
                |e| {
                    obrain_common::utils::error::Error::Internal(format!(
                        "backup fusion: create {}: {e}",
                        tmp.display()
                    ))
                },
            )?);
            let ecrire = |flux: &mut std::io::BufWriter<std::fs::File>,
                          ligne: &serde_json::Value|
             -> Result<()> {
                serde_json::to_writer(&mut *flux, ligne).map_err(|e| {
                    obrain_common::utils::error::Error::Internal(format!(
                        "backup fusion: serialisation: {e}"
                    ))
                })?;
                flux.write_all(b"\n").map_err(|e| {
                    obrain_common::utils::error::Error::Internal(format!(
                        "backup fusion: write: {e}"
                    ))
                })
            };
            ecrire(
                &mut flux,
                &serde_json::json!({
                    "label": label, "cle": cle,
                    "at_unix": web_time::SystemTime::now()
                        .duration_since(web_time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                }),
            )?;
            for g in &a_fusionner {
                for perdant in &g.perdants {
                    let id = obrain_common::types::NodeId(*perdant);
                    let Some(node) = store.get_node(id) else { continue };
                    let props: serde_json::Map<String, serde_json::Value> = node
                        .properties
                        .iter()
                        .map(|(k, v)| (k.to_string(), serde_json::json!(format!("{v:?}"))))
                        .collect();
                    let sortantes: Vec<serde_json::Value> = store
                        .edges_from(id, obrain_core::graph::Direction::Outgoing)
                        .into_iter()
                        .map(|(dst, e)| {
                            serde_json::json!({
                                "type": store.edge_type(e).map(|t| t.to_string()),
                                "vers": dst.0,
                            })
                        })
                        .collect();
                    let entrantes: Vec<serde_json::Value> = store
                        .edges_from(id, obrain_core::graph::Direction::Incoming)
                        .into_iter()
                        .map(|(src, e)| {
                            serde_json::json!({
                                "type": store.edge_type(e).map(|t| t.to_string()),
                                "de": src.0,
                            })
                        })
                        .collect();
                    let externes: Vec<serde_json::Value> = store
                        .external_edges_from(id)
                        .into_iter()
                        .map(|e| {
                            serde_json::json!({
                                "type": e.edge_type, "host": e.to_host,
                                "store": e.to_store, "key": e.to_key,
                            })
                        })
                        .collect();
                    ecrire(
                        &mut flux,
                        &serde_json::json!({
                            "id_logique": g.id_logique,
                            "survivant": g.survivant,
                            "node_id": perdant,
                            "props": props,
                            "sortantes": sortantes,
                            "entrantes": entrantes,
                            "externes": externes,
                        }),
                    )?;
                }
            }
            flux
                .into_inner()
                .map_err(|e| {
                    obrain_common::utils::error::Error::Internal(format!(
                        "backup fusion: flush: {e}"
                    ))
                })?
                .sync_all()
                .map_err(|e| {
                    obrain_common::utils::error::Error::Internal(format!(
                        "backup fusion: fsync: {e}"
                    ))
                })?;
            std::fs::rename(&tmp, chemin).map_err(|e| {
                obrain_common::utils::error::Error::Internal(format!(
                    "backup fusion: rename {}: {e}",
                    chemin.display()
                ))
            })?;
            rapport.backup = Some(chemin.display().to_string());
        }

        // ── FUSION : remonte puis DETACH DELETE des perdants. ──
        for g in &mut a_fusionner {
            let survivant = obrain_common::types::NodeId(g.survivant);
            // Deduplication : les extremites deja reliees au survivant.
            let mut deja_sortantes: std::collections::HashSet<(String, u64)> = store
                .edges_from(survivant, obrain_core::graph::Direction::Outgoing)
                .into_iter()
                .filter_map(|(dst, e)| {
                    store.edge_type(e).map(|t| (t.to_string(), dst.0))
                })
                .collect();
            let mut deja_entrantes: std::collections::HashSet<(String, u64)> = store
                .edges_from(survivant, obrain_core::graph::Direction::Incoming)
                .into_iter()
                .filter_map(|(src, e)| {
                    store.edge_type(e).map(|t| (t.to_string(), src.0))
                })
                .collect();
            let mut deja_externes: std::collections::HashSet<(String, String, String)> = store
                .external_edges_from(survivant)
                .into_iter()
                .map(|e| (e.edge_type, e.to_store, e.to_key))
                .collect();

            for perdant in &g.perdants {
                let id = obrain_common::types::NodeId(*perdant);
                for (dst, e) in store.edges_from(id, obrain_core::graph::Direction::Outgoing) {
                    if dst == survivant || g.perdants.contains(&dst.0) {
                        continue; // pas d auto-boucle ni d arete vers un futur mort
                    }
                    let Some(t) = store.edge_type(e) else { continue };
                    if deja_sortantes.insert((t.to_string(), dst.0)) {
                        store.create_edge(survivant, dst, t.as_str());
                        g.aretes_remontees += 1;
                    }
                }
                for (src, e) in store.edges_from(id, obrain_core::graph::Direction::Incoming) {
                    if src == survivant || g.perdants.contains(&src.0) {
                        continue;
                    }
                    let Some(t) = store.edge_type(e) else { continue };
                    if deja_entrantes.insert((t.to_string(), src.0)) {
                        store.create_edge(src, survivant, t.as_str());
                        g.aretes_remontees += 1;
                    }
                }
                for e in store.external_edges_from(id) {
                    if deja_externes.insert((
                        e.edge_type.clone(),
                        e.to_store.clone(),
                        e.to_key.clone(),
                    )) {
                        store.create_external_edge(
                            survivant,
                            &e.edge_type,
                            e.to_host.as_deref(),
                            &e.to_store,
                            &e.to_key,
                        );
                        g.externes_remontees += 1;
                    }
                }
                store.delete_node_edges(id);
                store.delete_node(id);
            }
        }
        rapport.groupes = a_fusionner;
        Ok(rapport)
    }

    /// (V4) Sante des aretes inter-bases : compte d'aretes externes du
    /// store + compteurs de traversee du registre de session (pendantes,
    /// hotes distants, refus de droits). Instantane, JAMAIS de remise a
    /// zero. Voir [`crate::admin::InterBaseHealth`].
    #[must_use]
    pub fn inter_base_health(&self) -> crate::admin::InterBaseHealth {
        use std::sync::atomic::Ordering;
        let mut sante = crate::admin::InterBaseHealth {
            external_edge_count: self.data_store().external_edge_count(),
            ..Default::default()
        };
        if let Some(reg) = self.base_registry() {
            sante.dangling_refs = reg.dangling_refs.load(Ordering::Relaxed);
            sante.remote_refs_unresolved = reg.remote_refs_unresolved.load(Ordering::Relaxed);
            sante.access_denied_refs = reg.access_denied_refs.load(Ordering::Relaxed);
        }
        sante
    }

    /// Returns a hierarchical memory usage breakdown.
    ///
    /// Walks all internal structures (store, indexes, MVCC chains, caches,
    /// string pools, buffer manager) and returns estimated heap bytes for each.
    /// Safe to call concurrently with queries.
    #[must_use]
    pub fn memory_usage(&self) -> crate::memory_usage::MemoryUsage {
        use crate::memory_usage::{BufferManagerMemory, CacheMemory, MemoryUsage};
        use obrain_common::memory::MemoryRegion;
        use obrain_common::memory::usage::{
            IndexMemory, MvccMemory, StoreMemory, StringPoolMemory,
        };

        // (2026-08-13) LE TODO EST TENU.
        //
        // Ce bloc rendait des zeros en mode substrat, en expliquant pourquoi :
        // mieux valait qu'un appelant lise « inconnu » plutot qu'un rapport
        // silencieusement faux calcule sur un LpgStore vide. C'etait le bon
        // reflexe — et, de tous les silences trouves dans ce moteur ce soir,
        // le seul qui etait ASSUME et documente au lieu d'etre subi.
        //
        // `SubstrateStore::memory_breakdown()` existe maintenant et rapporte
        // les empreintes de zones. On passe donc par le vrai backend, comme
        // partout ailleurs : `data_store()`, et non `self.store`.
        let (store, indexes, mvcc, string_pool) = self.data_store().memory_breakdown();
        let _ = (
            StoreMemory::default(),
            IndexMemory::default(),
            MvccMemory::default(),
            StringPoolMemory::default(),
        );

        let (parsed_bytes, optimized_bytes, cached_plan_count) =
            self.query_cache.heap_memory_bytes();
        let mut caches = CacheMemory {
            parsed_plan_cache_bytes: parsed_bytes,
            optimized_plan_cache_bytes: optimized_bytes,
            cached_plan_count,
            ..Default::default()
        };
        caches.compute_total();

        let bm_stats = self.buffer_manager.stats();
        let buffer_manager = BufferManagerMemory {
            budget_bytes: bm_stats.budget,
            allocated_bytes: bm_stats.total_allocated,
            graph_storage_bytes: bm_stats.region_usage(MemoryRegion::GraphStorage),
            index_buffers_bytes: bm_stats.region_usage(MemoryRegion::IndexBuffers),
            execution_buffers_bytes: bm_stats.region_usage(MemoryRegion::ExecutionBuffers),
            spill_staging_bytes: bm_stats.region_usage(MemoryRegion::SpillStaging),
        };

        let mut usage = MemoryUsage {
            store,
            indexes,
            mvcc,
            caches,
            string_pool,
            buffer_manager,
            ..Default::default()
        };
        usage.compute_total();
        usage
    }

    /// Returns detailed database statistics.
    ///
    /// Includes counts, memory usage, and index information.
    #[must_use]
    pub fn detailed_stats(&self) -> crate::admin::DatabaseStats {
        #[cfg(feature = "wal")]
        let disk_bytes = self.config.path.as_ref().and_then(|p| {
            if p.exists() {
                Self::calculate_disk_usage(p).ok()
            } else {
                None
            }
        });
        #[cfg(not(feature = "wal"))]
        let disk_bytes: Option<usize> = None;

        // T17 W3c slice 5b: route schema-cardinality reads through data_store()
        // so substrate mode reports real counts (LpgStore-inherent methods
        // target the dummy empty store in substrate mode).
        let data = self.data_store();
        crate::admin::DatabaseStats {
            node_count: data.node_count(),
            edge_count: data.edge_count(),
            label_count: data.all_labels().len(),
            edge_type_count: data.all_edge_types().len(),
            property_key_count: data.all_property_keys().len(),
            index_count: self.catalog.index_count(),
            memory_bytes: self.buffer_manager.allocated(),
            disk_bytes,
        }
    }

    /// Calculates total disk usage for the database directory.
    #[cfg(feature = "wal")]
    fn calculate_disk_usage(path: &Path) -> Result<usize> {
        let mut total = 0usize;
        if path.is_dir() {
            for entry in std::fs::read_dir(path)? {
                let entry = entry?;
                let metadata = entry.metadata()?;
                if metadata.is_file() {
                    total += metadata.len() as usize;
                } else if metadata.is_dir() {
                    total += Self::calculate_disk_usage(&entry.path())?;
                }
            }
        }
        Ok(total)
    }

    /// Returns schema information (labels, edge types, property keys).
    ///
    /// For LPG mode, returns label and edge type information.
    /// For RDF mode, returns predicate and named graph information.
    #[must_use]
    pub fn schema(&self) -> crate::admin::SchemaInfo {
        // T17 W3c slice 5a: trait reads route through data_store() so substrate
        // mode returns real labels/types/keys (previously self.store was the
        // dummy LpgStore → empty results). Per-label counts use the trait's
        // node_count_by_label (substrate overrides with O(1) label-index).
        // Edge-type counts still go through self.store.edges_with_type() —
        // no trait equivalent yet; slice 5b adds substrate parity.
        let data = self.data_store();
        let labels = data
            .all_labels()
            .into_iter()
            .map(|name| crate::admin::LabelInfo {
                count: data.node_count_by_label(&name),
                name,
            })
            .collect();

        // (2026-08-14) Ici vivait le mensonge. Les NOMS venaient du vrai
        // backend (`data`), les COMPTES du `self.store` mort — un `LpgStore`
        // vide depuis le basculement T17, qui rendait donc zero sans jamais
        // echouer. `obrain schema` affichait `count: 0` sur des types ayant
        // des milliers d'aretes vivantes. Les deux cotes lisent desormais la
        // meme source, et l'ignorance se dit `None` au lieu de se deguiser
        // en zero.
        let comptes = data.edge_counts_by_type();
        let edge_types = data
            .all_edge_types()
            .into_iter()
            .map(|name| crate::admin::EdgeTypeInfo {
                count: comptes
                    .as_ref()
                    .map(|carte| carte.get(&name).copied().unwrap_or(0)),
                name,
            })
            .collect();

        let property_keys = data.all_property_keys();

        crate::admin::SchemaInfo::Lpg(crate::admin::LpgSchemaInfo {
            labels,
            edge_types,
            property_keys,
        })
    }

    /// Returns detailed information about all indexes.
    ///
    /// (NS-PARTOUT, 2026-09-13) EN MODE SUBSTRATE, CETTE LISTE MENTAIT.
    /// Elle ne lisait que `self.catalog` — le catalogue GQL, que le
    /// basculement T17 laisse vide — alors que les index de propriété
    /// vivent au catalogue SUBSTRATE (`substrate.indexcat`, posé par
    /// `create_property_index` et relu à l'ouverture). `obrain index list`
    /// répondait `[]` sur un store dont le seek passait par l'index.
    /// Les déclarations substrate sont désormais jointes à la liste.
    #[must_use]
    pub fn list_indexes(&self) -> Vec<crate::admin::IndexInfo> {
        let mut out: Vec<crate::admin::IndexInfo> = self
            .catalog
            .all_indexes()
            .into_iter()
            .map(|def| {
                let label_name = self
                    .catalog
                    .get_label_name(def.label)
                    .unwrap_or_else(|| "?".into());
                let prop_name = self
                    .catalog
                    .get_property_key_name(def.property_key)
                    .unwrap_or_else(|| "?".into());
                crate::admin::IndexInfo {
                    name: format!("idx_{}_{}", label_name, prop_name),
                    index_type: format!("{:?}", def.index_type),
                    target: format!("{}:{}", label_name, prop_name),
                    unique: false,
                    cardinality: None,
                    size_bytes: None,
                }
            })
            .collect();
        if let Some(sub) = self.substrate_store.as_ref() {
            for d in sub.declarations_index() {
                let cible = if d.label.is_empty() {
                    d.propriete.clone()
                } else {
                    format!("{}:{}", d.label, d.propriete)
                };
                out.push(crate::admin::IndexInfo {
                    name: format!("idx_{}_{}", d.genre, cible.replace(':', "_")),
                    index_type: d.genre.clone(),
                    target: cible,
                    unique: false,
                    cardinality: None,
                    size_bytes: None,
                });
            }
        }
        out
    }

    /// Validates database integrity.
    ///
    /// Checks for:
    /// - Dangling edge references (edges pointing to non-existent nodes)
    /// - Internal index consistency
    ///
    /// Returns a list of errors and warnings. Empty errors = valid.
    #[must_use]
    pub fn validate(&self) -> crate::admin::ValidationResult {
        let mut result = crate::admin::ValidationResult::default();

        // T17 W3c slice 5b: dangling-edge scan relies on LpgStore-inherent
        // `all_edges()` iterator. Substrate's edge topology lives in mmap'd
        // zones accessed by slot index; there is no trait-level iterator yet.
        // In substrate mode we bail cleanly — substrate's replay+checksum path
        // already validates on-disk edge integrity, so the admin validator is
        // informational rather than a correctness primitive there.
        if self.substrate_store.is_some() {
            result.warnings.push(crate::admin::ValidationWarning {
                code: "VALIDATE_SUBSTRATE_SKIPPED".to_string(),
                message: "Dangling-edge validator not implemented for substrate backend; \
                          on-disk integrity is verified by WAL replay + zone checksums"
                    .to_string(),
                context: None,
            });
            return result;
        }

        // Check for dangling edge references
        for edge in self.store.all_edges() {
            if self.data_store().get_node(edge.src).is_none() {
                result.errors.push(crate::admin::ValidationError {
                    code: "DANGLING_SRC".to_string(),
                    message: format!(
                        "Edge {} references non-existent source node {}",
                        edge.id.0, edge.src.0
                    ),
                    context: Some(format!("edge:{}", edge.id.0)),
                });
            }
            if self.data_store().get_node(edge.dst).is_none() {
                result.errors.push(crate::admin::ValidationError {
                    code: "DANGLING_DST".to_string(),
                    message: format!(
                        "Edge {} references non-existent destination node {}",
                        edge.id.0, edge.dst.0
                    ),
                    context: Some(format!("edge:{}", edge.id.0)),
                });
            }
        }

        // Add warnings for potential issues
        if self.data_store().node_count() > 0 && self.data_store().edge_count() == 0 {
            result.warnings.push(crate::admin::ValidationWarning {
                code: "NO_EDGES".to_string(),
                message: "Database has nodes but no edges".to_string(),
                context: None,
            });
        }

        result
    }

    /// Returns WAL (Write-Ahead Log) status.
    ///
    /// Returns None if WAL is not enabled.
    #[must_use]
    pub fn wal_status(&self) -> crate::admin::WalStatus {
        // W3a: route through data_store() so substrate-backed DBs report the
        // real substrate epoch rather than the dummy LpgStore's zero epoch.
        // See gotcha note 0b9fcabe-a780-4149-8709-ed32ee9ed82e.
        let current_epoch = self.data_store().current_epoch().as_u64();
        #[cfg(feature = "wal")]
        if let Some(ref wal) = self.wal {
            return crate::admin::WalStatus {
                enabled: true,
                path: self.config.path.as_ref().map(|p| p.join("wal")),
                size_bytes: wal.size_bytes(),
                record_count: wal.record_count() as usize,
                last_checkpoint: wal.last_checkpoint_timestamp(),
                current_epoch,
            };
        }

        // ── EN MODE SUBSTRATE, LE WAL EXISTE : LE DIRE ──────────────────────
        // Défaut mesuré le 2026-09-17 (`wal::test_wal_status_persistent`) : une
        // base persistante rendait `enabled: false` — c'est-à-dire « aucune
        // journalisation » — alors que le substrat écrit bel et bien
        // `substrate.wal`, le rejoue à l'ouverture et fait survivre les
        // écritures. L'ancien `enabled` ne regardait que le champ `self.wal`,
        // vestige de l'ère LpgStore, que le cutover T17 a retiré : l'API
        // MENTAIT sur la durabilité, et `obrain status` avec elle.
        //
        // Les nombres sont ceux que le substrat tient RÉELLEMENT :
        //   · `size_bytes`  : l'offset d'écriture du WAL (octets appendés) ;
        //   · `record_count`: le nombre d'enregistrements APPENDÉS par
        //     l'écrivain de cette session. Ce n'était PAS un champ distinct
        //     avant le 2026-10-06 : il lisait `peek_lsn()`, qui valait « un
        //     incrément par enregistrement appendé ». Depuis que le LSN est
        //     dérivé de la position physique (`lsn = ancre + position`),
        //     `peek_lsn()` vaut ≈ la TAILLE DU WAL EN OCTETS — d'où le compteur
        //     explicite tenu par le substrat ;
        //   · `last_checkpoint`: `None`, parce que l'horodatage du dernier
        //     checkpoint vit dans l'en-tête du substrat et n'est pas exposé ici.
        //     `None` veut dire « pas su », pas « zéro » : inventer 1970 serait
        //     pire qu'avouer l'ignorance.
        if let Some(store) = self.substrate_store.as_ref() {
            let wal = store.writer().wal();
            // ── LE CHEMIN NE SE DONNE QUE S'IL EST DURABLE ──────────────────
            // Une base EN MÉMOIRE a bien un WAL (le substrat en écrit un, dans
            // un répertoire temporaire) : `enabled` doit donc dire vrai. Mais
            // annoncer le chemin de ce répertoire laisserait croire à une
            // durabilité qui n'existe pas — il disparaît à la fermeture. Le
            // chemin suit donc l'EMPLACEMENT CONFIGURÉ (`config.path`), comme
            // la branche historique : `None` veut dire « journalisé, sans lieu
            // durable », et c'est exactement le cas d'une base en mémoire.
            return crate::admin::WalStatus {
                enabled: true,
                path: self.config.path.as_ref().map(|p| p.join("wal")),
                size_bytes: wal.offset() as usize,
                // ── UN VRAI COMPTEUR, PAS `peek_lsn()` (axe APPEND, 2026-10-06) ──
                // Le LSN n'est plus « un incrément par enregistrement appendé » :
                // il vaut `ancre + position`, donc ≈ la TAILLE DU WAL. Annoncer
                // `peek_lsn` ici aurait affiché « Records: 1048576 » pour 500
                // enregistrements (`obrain wal status`, binding Python). Le
                // substrat tient désormais un compteur explicite.
                record_count: wal.records_ecrits() as usize,
                last_checkpoint: None,
                current_epoch,
            };
        }

        crate::admin::WalStatus {
            enabled: false,
            path: None,
            size_bytes: 0,
            record_count: 0,
            last_checkpoint: None,
            current_epoch,
        }
    }

    /// Forces a WAL checkpoint.
    ///
    /// Flushes all pending WAL records to the main storage.
    ///
    /// # Errors
    ///
    /// Returns an error if the checkpoint fails.
    pub fn wal_checkpoint(&self) -> Result<()> {
        #[cfg(feature = "wal")]
        if let Some(ref wal) = self.wal {
            // W3a: route through data_store() — substrate epoch in substrate mode.
            let epoch = self.data_store().current_epoch();
            let transaction_id = self
                .transaction_manager
                .last_assigned_transaction_id()
                .unwrap_or_else(|| self.transaction_manager.begin());
            wal.checkpoint(transaction_id, epoch)?;
            wal.sync()?;
        }

        // T17 final cutover: the single-file `.obrain` checkpoint step
        // (`obrain-file` feature) was retired with the substrate cutover.

        Ok(())
    }

    /// Prunes old WAL log files that have been fully checkpointed.
    ///
    /// This removes WAL files whose sequence number is below the last
    /// checkpoint, freeing disk space. The current active log file is
    /// always preserved.
    ///
    /// Returns the number of bytes freed (approximate, based on size before prune).
    ///
    /// # Errors
    ///
    /// Returns an error if the WAL is not enabled or pruning fails.
    pub fn prune_wal(&self) -> Result<usize> {
        #[cfg(feature = "wal")]
        if let Some(ref wal) = self.wal {
            let size_before = wal.size_bytes();
            wal.prune_old_logs()?;
            let size_after = wal.size_bytes();
            return Ok(size_before.saturating_sub(size_after));
        }

        Ok(0)
    }

    // =========================================================================
    // ADMIN API: Change Data Capture
    // =========================================================================

    /// Returns the full change history for an entity (node or edge).
    ///
    /// Events are ordered chronologically by epoch.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDC feature is not enabled.
    #[cfg(feature = "cdc")]
    pub fn history(
        &self,
        entity_id: impl Into<crate::cdc::EntityId>,
    ) -> Result<Vec<crate::cdc::ChangeEvent>> {
        Ok(self.cdc_log.history(entity_id.into()))
    }

    /// Returns change events for an entity since the given epoch.
    #[cfg(feature = "cdc")]
    pub fn history_since(
        &self,
        entity_id: impl Into<crate::cdc::EntityId>,
        since_epoch: obrain_common::types::EpochId,
    ) -> Result<Vec<crate::cdc::ChangeEvent>> {
        Ok(self.cdc_log.history_since(entity_id.into(), since_epoch))
    }

    /// Returns all change events across all entities in an epoch range.
    #[cfg(feature = "cdc")]
    pub fn changes_between(
        &self,
        start_epoch: obrain_common::types::EpochId,
        end_epoch: obrain_common::types::EpochId,
    ) -> Result<Vec<crate::cdc::ChangeEvent>> {
        Ok(self.cdc_log.changes_between(start_epoch, end_epoch))
    }
}
