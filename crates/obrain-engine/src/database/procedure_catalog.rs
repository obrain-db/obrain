//! Catalogue DURABLE des procedures stockees — declaration, retrait,
//! hydratation au boot.
//!
//! (2026-09-07) Pourquoi ce module existe : le catalogue GQL
//! (`CREATE PROCEDURE`, tenu par `Catalog::SchemaCatalog.procedures`) vit
//! en RAM par processus. `session` ecrit bien un `WalRecord::CreateProcedure`,
//! mais ce record n'est JAMAIS rejoue — `replay_from()` n'a aucun appelant
//! en production (constat deja fait pour les index le 2026-08-14), et le WAL
//! substrate ne porte que les mutations de donnees. Resultat mesure sur le
//! binaire 525dae0e : une procedure creee en process A est `Unknown
//! procedure` en process B apres reopen.
//!
//! Le remede est celui des index (`substrate.indexcat`) : l'INTENTION est
//! persister cote substrate dans `substrate.proccat` (JSON par ligne, ecrit
//! atomiquement), puis
//!
//! 1. **declarer** / **retirer** a chaque `CREATE`/`DROP PROCEDURE` — fait
//!    directement par la session sur son handle substrate (`Session` porte
//!    `substrate_store`, voir `session::execute_schema_command`) ;
//! 2. **hydrater** le catalogue du moteur au boot (`hydrater_procedures`),
//!    pour que toute session d'un process neuf voie les procedures sans
//!    re-`CREATE PROCEDURE` manuel.
//!
//! Le catalogue RAM reste la source chaude de resolution (`get_procedure`) ;
//! le fichier substrate est la source froide qui le repeuple.

use crate::catalog::ProcedureDefinition;

impl super::ObrainDB {
    /// Hydrate le catalogue du moteur depuis les declarations substrate.
    ///
    /// Appele a l'ouverture d'une base substrate (voir `with_substrate_store`).
    /// Ne fait RIEN si la base n'est pas substrate (in-memory : le catalogue
    /// RAM vit et meurt avec le process, c'est le comportement attendu).
    ///
    /// Idempotent : reouvrir une base deja ouverte re-ecrase les memes noms
    /// avec les memes definitions. `replace_procedure` est utilise (et non
    /// `register_procedure`) pour que l'hydratation ne puisse pas echouer sur
    /// un doublon — le fichier est la verite, le catalogue RAM s'y plie. Une
    /// erreur d'hydratation est journalisee, jamais fatale a l'ouverture.
    pub(super) fn hydrater_procedures(&self) {
        let Some(ref sub) = self.substrate_store else {
            return;
        };
        let declarations = sub.declarations_procedure();
        if declarations.is_empty() {
            return;
        }
        let mut chargees = 0usize;
        for decl in declarations {
            let nom = decl.name.clone();
            let def = ProcedureDefinition {
                name: decl.name,
                params: decl.params,
                returns: decl.returns,
                body: decl.body,
            };
            if let Err(e) = self.catalog.replace_procedure(def) {
                tracing::warn!(
                    erreur = %e,
                    nom = %nom,
                    "hydratation de procedure echouee — ignoree"
                );
                continue;
            }
            chargees += 1;
        }
        tracing::info!(
            procedures = chargees,
            "catalogue de procedures hydrate depuis substrate.proccat"
        );
    }
}
