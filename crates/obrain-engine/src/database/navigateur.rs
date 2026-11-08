//! Persistance navigateur (wasm32 seulement) : une base adossée au journal OPFS.
//!
//! Contrat, identique à celui du substrat (`SubstrateStore::persister_sur_miroir`) :
//! `persist` publie un INSTANTANÉ du store. Une écriture non publiée est perdue à la
//! coupure : ce n'est pas un journal. Le WAL sur OPFS (décision C) n'existe pas
//! encore.
//!
//! Tout échec remonte en `Err` : aucune de ces fonctions ne panique, une panique
//! wasm abattrait l'instance entière.

use std::sync::Arc;

use obrain_common::utils::error::{Error, Result};

use super::ObrainDB;

fn erreur_navigateur(e: obrain_substrate::SubstrateError) -> Error {
    Error::Internal(format!("persistance navigateur : {e}"))
}

impl ObrainDB {
    /// Ouvre la base persistante `nom` dans le navigateur, créée vide si absente.
    ///
    /// Le journal OPFS `nom` est rejoué, puis le store est restauré depuis sa
    /// dernière image publiée. Une base ouverte ici ne persiste rien tant que
    /// [`ObrainDB::persist`] n'a pas été appelé.
    ///
    /// Doit être appelée depuis un worker dédié : OPFS n'expose son accès
    /// synchrone (`SyncAccessHandle`) que là.
    ///
    /// # Errors
    ///
    /// Erreur si OPFS est absent, si le fichier est déjà ouvert par un autre
    /// handle, ou si le journal est corrompu.
    pub async fn open_browser(nom: &str) -> Result<Self> {
        let (store, miroir) = obrain_substrate::SubstrateStore::ouvrir_navigateur(nom)
            .await
            .map_err(erreur_navigateur)?;
        let mut db = Self::with_substrate_store(store, None, false)?;
        db.miroir = Some(miroir);
        Ok(db)
    }

    /// Publie l'état courant dans le journal OPFS. Durable quand l'appel rend `Ok`.
    ///
    /// Les écritures faites depuis la dernière publication sont perdues à une
    /// coupure : seul l'instantané publié survit.
    ///
    /// # Errors
    ///
    /// Erreur si la base n'a pas été ouverte par [`ObrainDB::open_browser`], ou si
    /// la publication échoue (quota OPFS, plafond de 2 Gio, E/S).
    pub fn persist(&self) -> Result<()> {
        let (Some(store), Some(miroir)) = (self.substrate_store.as_ref(), self.miroir.as_ref())
        else {
            return Err(Error::Internal(
                "persist : base non ouverte par open_browser".to_string(),
            ));
        };
        store
            .persister_sur_miroir(Arc::clone(miroir))
            .map_err(erreur_navigateur)
    }
}
