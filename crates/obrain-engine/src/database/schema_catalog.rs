//! Catalogue DURABLE des TYPES déclarés (`CREATE NODE|EDGE TYPE`) —
//! déclaration, retrait, hydratation au boot.
//!
//! ── POURQUOI CE MODULE EXISTE (même constat que `procedure_catalog`, 2026-09-07) ─
//! Le catalogue de schéma (`Catalog::SchemaCatalog.node_types` / `edge_types`)
//! vit en RAM par processus. La session écrit bien un `WalRecord::CreateNodeType`
//! (ou `CreateEdgeType`), **mais ce record n'est jamais rejoué** : le rejeu du
//! substrat ne porte que les mutations de données, et `replay_from()` n'a aucun
//! appelant en production — constat déjà établi pour les index (2026-08-14) puis
//! pour les procédures (2026-09-07).
//!
//! Résultat mesuré : un `CREATE NODE TYPE Vehicle (…)` **disparaissait au
//! redémarrage**. Quatre tests de `crates/obrain-engine/tests/wal_recovery.rs`
//! le constataient (`test_create_node_type_persists`, `test_create_edge_type_persists`,
//! `test_schema_with_data_persists`, et la même famille pour le statut WAL), et
//! **ils avaient raison** : un schéma qui ne survit pas à un redémarrage n'est
//! pas un schéma, c'est un commentaire.
//!
//! Le remède est celui des index (`substrate.indexcat`) et des procédures
//! (`substrate.proccat`) : l'INTENTION est persistée côté substrat dans
//! `substrate.typecat` (une définition JSON compacte par ligne, écrite
//! atomiquement), puis
//!
//! 1. **déclarée** / **retirée** à chaque `CREATE`/`DROP … TYPE` — fait par la
//!    session sur son handle substrate ;
//! 2. **hydratée** dans le catalogue du moteur à l'ouverture
//!    ([`super::ObrainDB::hydrater_types`]), pour que toute session d'un
//!    processus neuf voie les types sans les redéclarer.
//!
//! Le catalogue RAM reste la source chaude (résolution des vérifications de
//! schéma) ; le fichier substrate est la source froide qui le repeuple.

use crate::catalog::{EdgeTypeDefinition, NodeTypeDefinition};

/// Genre d'un type déclaré, tel qu'il est persisté.
pub(crate) const GENRE_NOEUD: &str = "node";
/// Genre d'un type déclaré, tel qu'il est persisté.
pub(crate) const GENRE_ARETE: &str = "edge";

impl super::ObrainDB {
    /// Hydrate les types déclarés depuis le catalogue du substrat.
    ///
    /// Appelé à l'ouverture d'une base substrate (juste après
    /// `hydrater_procedures`). Ne fait RIEN sur une base en mémoire : le
    /// catalogue RAM y vit et meurt avec le processus, c'est le contrat.
    ///
    /// Idempotent : `register_node_type` rend une erreur sur un nom déjà
    /// présent, et cette erreur est ici **le cas normal** (on rouvre une base
    /// dont le catalogue a déjà été hydraté, ou qui a été créée dans ce
    /// processus). Le fichier est la vérité ; le catalogue RAM s'y plie — donc
    /// on remplace au lieu d'échouer, et une ligne illisible est ignorée (un
    /// type perdu dégrade une vérification, il ne doit pas empêcher l'ouverture).
    pub(super) fn hydrater_types(&self) {
        let Some(ref sub) = self.substrate_store else {
            return;
        };
        let declarations = sub.declarations_type();
        if declarations.is_empty() {
            return;
        }
        let (mut noeuds, mut aretes, mut ignorees) = (0usize, 0usize, 0usize);
        for decl in declarations {
            match decl.genre.as_str() {
                GENRE_NOEUD => {
                    match serde_json::from_str::<NodeTypeDefinition>(&decl.definition) {
                        Ok(def) => {
                            // Remplacement explicite : la définition du fichier
                            // fait autorité, y compris contre une définition
                            // homonyme construite dans ce processus.
                            let nom = def.name.clone();
                            let _ = self.catalog.drop_node_type(&nom);
                            if let Err(e) = self.catalog.register_node_type(def) {
                                tracing::warn!(erreur = %e, nom = %nom,
                                    "hydratation de type de nœud échouée — ignorée");
                                ignorees += 1;
                            } else {
                                noeuds += 1;
                            }
                        }
                        Err(e) => {
                            tracing::warn!(erreur = %e, nom = %decl.nom,
                                "définition de type de nœud illisible — ignorée");
                            ignorees += 1;
                        }
                    }
                }
                GENRE_ARETE => {
                    match serde_json::from_str::<EdgeTypeDefinition>(&decl.definition) {
                        Ok(def) => {
                            let nom = def.name.clone();
                            let _ = self.catalog.drop_edge_type_def(&nom);
                            if let Err(e) = self.catalog.register_edge_type_def(def) {
                                tracing::warn!(erreur = %e, nom = %nom,
                                    "hydratation de type d'arête échouée — ignorée");
                                ignorees += 1;
                            } else {
                                aretes += 1;
                            }
                        }
                        Err(e) => {
                            tracing::warn!(erreur = %e, nom = %decl.nom,
                                "définition de type d'arête illisible — ignorée");
                            ignorees += 1;
                        }
                    }
                }
                autre => {
                    tracing::warn!(genre = %autre, "genre de type inconnu — ignoré");
                    ignorees += 1;
                }
            }
        }
        tracing::info!(
            noeuds,
            aretes,
            ignorees,
            "catalogue de types hydrate depuis substrate.typecat"
        );
    }
}
