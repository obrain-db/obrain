//! ── L'INDEX VECTORIEL SURVIT AU PROCESSUS (O-VEC C3) ────────────────────────
//!
//! Jusqu'ici, `create_vector_index` bâtissait le cascade L0/L1/L2 **en RAM par
//! processus** (`substrate_vector_indexes`) ; une base rouverte le rebâtissait
//! à la première recherche (`rebatir_index_declare`), par un balayage complet
//! du label et une reprojection de chaque vecteur — O(N) payé par chaque
//! ouverture, prohibitif au million.
//!
//! ── CE QUI EST PERSISTÉ ────────────────────────────────────────────────────
//! Les trois paliers et la projection apprise de chaque index, dans des zones
//! NOMMÉES par (label, propriété) :
//! `substrate.tier{0,1,2}.<label>.<prop>` et `substrate.vecproj.<label>.<prop>`
//! (`SubstrateTieredIndex::persister_nomme`). Chaque palier porte une
//! **empreinte** du store (offset du WAL + nœuds vivants) et l'epoch de
//! repermutation des slots.
//!
//! ── CE QUI DÉCIDE DE LA VALIDITÉ ───────────────────────────────────────────
//! À la réouverture, l'index n'est chargé que si son empreinte décrit encore
//! le store (`SubstrateTieredIndex::charger_nomme`). L'empreinte est
//! conservatrice : toute mutation la change — y compris une écriture de
//! vecteur, grâce au marqueur WAL `VecColumnWrite` (sans lui, réécrire le
//! vecteur d'un nœud existant ne déplaçait pas l'offset et validait un index
//! périmé). Sinon : reconstruction depuis la colonne, puis réécriture.
//!
//! ── QUAND ON PERSISTE ──────────────────────────────────────────────────────
//! À la création (`create_vector_index`) et à la FERMETURE, après le flush,
//! à l'état exact où la base s'arrête. À la fermeture, un index n'est
//! réécrit que s'il a VU chaque écriture de vecteur :
//!
//! * base en écriture exclusive : le compteur du store
//!   (`nb_ecritures_vectorielles_noeuds`, incrémenté à chaque écriture routée
//!   vers une colonne) est égal au compteur miroir du moteur
//!   (`ecritures_vectorielles_vues`, incrémenté par chaque chemin d'API qui
//!   entretient les index). Une écriture qui contourne le moteur (GQL `SET`,
//!   défaut connu T5) rend les compteurs inégaux ⇒ les zones sont SUPPRIMÉES
//!   et la réouverture reconstruit — le comportement d'avant, jamais un index
//!   périmé ;
//! * base en écriture PARTAGÉE : un pair a pu écrire sans que ce processus le
//!   voie ; règle des index texte — l'offset du WAL doit être inchangé depuis
//!   la dernière synchronisation de l'index.
//!
//! Reconstruire reste toujours possible et correct : c'est le filet.

#[cfg(feature = "vector-index")]
use std::sync::Arc;

#[cfg(feature = "vector-index")]
use obrain_substrate::retrieval::EmpreinteStore;
#[cfg(feature = "vector-index")]
use obrain_substrate::{SubstrateTieredIndex, VectorIndex as _};

impl super::ObrainDB {
    /// L'empreinte de l'état du store : `(offset du WAL, nœuds vivants)` —
    /// la même que celle des index texte.
    #[cfg(feature = "vector-index")]
    fn empreinte_store_vectorielle(&self) -> EmpreinteStore {
        let wal_offset = self
            .substrate_store
            .as_ref()
            .map_or(0, |s| s.writer().wal().offset());
        EmpreinteStore {
            wal_offset,
            noeuds_vivants: self.data_store().node_ids().len() as u64,
        }
    }

    /// Persiste un index dans ses zones nommées, estampillé de l'empreinte
    /// courante. Sans effet sur une base en mémoire (rien de durable à
    /// écrire). Un échec est journalisé : l'index sera reconstruit à la
    /// prochaine ouverture, jamais lu faux.
    #[cfg(feature = "vector-index")]
    pub(super) fn persister_index_vectoriel(
        &self,
        label: &str,
        property: &str,
        index: &SubstrateTieredIndex,
    ) {
        let Some(ref sub) = self.substrate_store else {
            return;
        };
        if self.config.path.is_none() {
            return; // base en mémoire : rien à persister, et c'est honnête
        }
        let empreinte = self.empreinte_store_vectorielle();
        let cle = format!("{label}:{property}");
        match sub.avec_substrate(|s| index.persister_nomme(s, label, property, empreinte)) {
            Ok(()) => {
                self.index_vectoriel_empreintes
                    .write()
                    .insert(cle, empreinte.wal_offset);
                self.index_vectoriel_persistes
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            Err(e) => {
                tracing::warn!(
                    erreur = %e, label, property,
                    "index vectoriel NON persiste — il sera reconstruit a la prochaine ouverture"
                );
                self.index_vectoriel_empreintes.write().remove(&cle);
            }
        }
    }

    /// Charge un index persisté si ses zones décrivent encore le store
    /// (empreinte, epoch de remap, CRC, projection). `None` ⇒ reconstruire.
    #[cfg(feature = "vector-index")]
    pub(super) fn charger_index_vectoriel(
        &self,
        label: &str,
        property: &str,
        dim_native: usize,
    ) -> Option<Arc<SubstrateTieredIndex>> {
        let sub = self.substrate_store.as_ref()?;
        self.config.path.as_ref()?;
        let empreinte = self.empreinte_store_vectorielle();
        let charge = match sub.avec_substrate(|s| {
            SubstrateTieredIndex::charger_nomme(s, label, property, dim_native, empreinte)
        }) {
            Ok(Some(i)) => i,
            Ok(None) => return None,
            Err(e) => {
                tracing::info!(
                    erreur = %e, label, property,
                    "zones d'index vectoriel illisibles — reconstruction"
                );
                return None;
            }
        };
        // Le rerank exact relit les vecteurs natifs dans la colonne (C4).
        charge.definir_source_native(self.source_native_vectorielle(property));
        self.index_vectoriel_empreintes
            .write()
            .insert(format!("{label}:{property}"), empreinte.wal_offset);
        self.index_vectoriel_charges
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        tracing::info!(
            label,
            property,
            vecteurs = charge.len(),
            "index vectoriel CHARGE depuis ses zones (pas de reconstruction)"
        );
        Some(Arc::new(charge))
    }

    /// Retire les zones persistées d'un index (suppression, ou index jugé
    /// périmé à la fermeture).
    #[cfg(feature = "vector-index")]
    pub(super) fn supprimer_zones_index_vectoriel(&self, label: &str, property: &str) {
        let Some(ref sub) = self.substrate_store else {
            return;
        };
        if let Err(e) =
            sub.avec_substrate(|s| SubstrateTieredIndex::supprimer_nomme(s, label, property))
        {
            tracing::warn!(erreur = %e, label, property, "zones d'index vectoriel non supprimees");
        }
        self.index_vectoriel_empreintes
            .write()
            .remove(&format!("{label}:{property}"));
    }

    /// À la fermeture, après le flush : réécrit les index qui ont vu chaque
    /// écriture, supprime les zones des autres (voir l'en-tête du module).
    #[cfg(feature = "vector-index")]
    pub(super) fn repersister_index_vectoriels(&self) {
        let Some(ref sub) = self.substrate_store else {
            return;
        };
        if self.config.path.is_none() || self.read_only {
            return;
        }
        let entrees: Vec<(String, Arc<SubstrateTieredIndex>)> = self
            .substrate_vector_indexes
            .read()
            .iter()
            .map(|(cle, (_, idx))| (cle.clone(), Arc::clone(idx)))
            .collect();
        if entrees.is_empty() {
            return;
        }
        let offset_courant = sub.writer().wal().offset();
        let ecritures_store = sub.nb_ecritures_vectorielles_noeuds();
        let ecritures_vues = self
            .ecritures_vectorielles_vues
            .load(std::sync::atomic::Ordering::Acquire);
        for (cle, idx) in entrees {
            let Some((label, property)) = cle.split_once(':') else {
                continue;
            };
            let a_tout_vu = if self.ecriture_partagee {
                // Un pair a pu écrire : seul un WAL immobile depuis la dernière
                // synchronisation prouve que l'index n'a rien manqué.
                self.index_vectoriel_empreintes
                    .read()
                    .get(&cle)
                    .is_some_and(|o| *o == offset_courant)
            } else {
                ecritures_store == ecritures_vues
            };
            if a_tout_vu {
                self.persister_index_vectoriel(label, property, &idx);
            } else {
                tracing::info!(
                    label,
                    property,
                    ecritures_store,
                    ecritures_vues,
                    partage = self.ecriture_partagee,
                    "index vectoriel non persiste a la fermeture : des ecritures lui ont echappe"
                );
                self.supprimer_zones_index_vectoriel(label, property);
            }
        }
    }

    /// `true` si la base est ouverte en écriture PARTAGÉE
    /// (`open_shared_write`) : un pair peut écrire sans que ce processus le
    /// voie, ce qui durcit les règles de persistance des index.
    #[must_use]
    pub fn est_en_ecriture_partagee(&self) -> bool {
        self.ecriture_partagee
    }

    /// Nombre d'index vectoriels CHARGÉS depuis leurs zones (sans
    /// reconstruction) depuis l'ouverture.
    #[must_use]
    pub fn nb_index_vectoriel_charges(&self) -> usize {
        self.index_vectoriel_charges
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Nombre d'index vectoriels RECONSTRUITS depuis la colonne (déclaration
    /// présente, zones absentes ou périmées) depuis l'ouverture.
    #[must_use]
    pub fn nb_index_vectoriel_reconstruits(&self) -> usize {
        self.index_vectoriel_reconstruits
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Nombre de persistances d'index vectoriel réussies depuis l'ouverture.
    #[must_use]
    pub fn nb_index_vectoriel_persistes(&self) -> usize {
        self.index_vectoriel_persistes
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// ── L'OBSERVATEUR D'INDEX VECTORIELS (T5) ───────────────────────────────────
///
/// Installé sur le store à l'ouverture (`add_mutation_observer`). Le store le
/// prévient depuis ses points UNIQUES d'écriture (`set_node_property` pour un
/// `Value::Vector`, `delete_node`, `remove_node_property`), donc TOUS les
/// chemins — API Rust, GQL `SET` / `CREATE` / `MERGE` — entretiennent les index,
/// sans crochet dans les opérateurs.
///
/// Il ne détient qu'un `Weak` du store (le store le détient déjà : un `Arc`
/// ferait un cycle) et partage le registre et le compteur du moteur.
///
/// Le compteur `ecritures_vectorielles_vues` s'incrémente ICI, à chaque
/// écriture vue : il égale donc le compteur du store par construction, et la
/// règle de persistance à la fermeture ne rejette plus que les index qui ont
/// réellement manqué une écriture.
#[cfg(feature = "vector-index")]
pub(super) struct ObservateurIndexVectoriel {
    pub(super) registre: Arc<
        parking_lot::RwLock<std::collections::HashMap<String, (usize, Arc<SubstrateTieredIndex>)>>,
    >,
    pub(super) compteur: Arc<std::sync::atomic::AtomicU64>,
    pub(super) store: std::sync::Weak<obrain_substrate::SubstrateStore>,
}

#[cfg(feature = "vector-index")]
impl obrain_substrate::StoreMutationObserver for ObservateurIndexVectoriel {
    fn node_created(&self, _id: obrain_common::types::NodeId, _epoch: u64) {}

    fn edge_created(
        &self,
        _id: obrain_common::types::EdgeId,
        _src: obrain_common::types::NodeId,
        _dst: obrain_common::types::NodeId,
        _epoch: u64,
    ) {
    }

    fn vector_written(&self, id: obrain_common::types::NodeId, key: &str, vector: &[f32]) {
        self.compteur
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let Some(store) = self.store.upgrade() else {
            return;
        };
        let suffixe = format!(":{key}");
        // Copie des entrées concernées : l'insertion se fait HORS du verrou du
        // registre.
        let concernes: Vec<(String, usize, Arc<SubstrateTieredIndex>)> = self
            .registre
            .read()
            .iter()
            .filter(|(cle, _)| cle.ends_with(&suffixe))
            .map(|(cle, (dim, idx))| (cle.clone(), *dim, Arc::clone(idx)))
            .collect();
        if concernes.is_empty() {
            return;
        }
        use obrain_core::graph::traits::GraphStore as _;
        let Some(noeud) = store.get_node(id) else {
            return;
        };
        for (cle, dim_native, idx) in concernes {
            let Some(label) = cle.strip_suffix(&suffixe) else {
                continue;
            };
            // Un embedding d'une autre largeur est ignoré — jamais fatal : une
            // base peut porter plusieurs modèles sur la même propriété.
            if dim_native == vector.len() && noeud.labels.iter().any(|l| l.as_str() == label) {
                idx.insert(id.0 as u32, vector);
            }
        }
    }

    fn node_deleted(&self, id: obrain_common::types::NodeId) {
        // `delete` est sans effet si le nœud n'est pas indexé : pas besoin des
        // labels (déjà tombstonés ici).
        let indexes: Vec<Arc<SubstrateTieredIndex>> = self
            .registre
            .read()
            .values()
            .map(|(_, idx)| Arc::clone(idx))
            .collect();
        for idx in indexes {
            idx.delete(id.0 as u32);
        }
    }

    fn node_property_removed(&self, id: obrain_common::types::NodeId, key: &str) {
        let suffixe = format!(":{key}");
        let indexes: Vec<Arc<SubstrateTieredIndex>> = self
            .registre
            .read()
            .iter()
            .filter(|(cle, _)| cle.ends_with(&suffixe))
            .map(|(_, (_, idx))| Arc::clone(idx))
            .collect();
        for idx in indexes {
            idx.delete(id.0 as u32);
        }
    }
}
