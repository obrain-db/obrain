//! Commande `obrain entretien` — les réparations **attachées en vol**, et leur
//! état.
//!
//! Un défaut constaté pendant que la base tourne ne peut pas être réparé (le
//! substrat exige l'exclusivité). Il n'est plus perdu pour autant : il est
//! **attaché** ici, et l'entretien de redémarrage le rejoue puis l'**accorde**.
//!
//! ```text
//! obrain entretien                                   # ce qui est dû, et depuis quand
//! obrain entretien --attacher <store> --genre degres --raison "…"
//! obrain entretien --accorder <store> --genre degres # la réparation a réussi
//! ```
//!
//! L'âge de la plus ancienne demande est affiché : une réparation qui traîne
//! depuis trois jours se VOIT, au lieu de disparaître dans un journal muet.

use std::path::PathBuf;

use anyhow::Result;
use clap::Args;

use crate::entretien::{Demande, Genre};

#[derive(Args, Debug)]
pub struct EntretienArgs {
    /// Attacher une demande pour cette base.
    #[arg(long)]
    pub attacher: Option<PathBuf>,
    /// Accorder (retirer) la demande de cette base : la réparation a réussi.
    #[arg(long)]
    pub accorder: Option<PathBuf>,
    /// Genre visé : `degres` ou `enregistrements`.
    #[arg(long, default_value = "degres")]
    pub genre: String,
    /// Ce qui justifie la demande — une MESURE, pas une impression.
    #[arg(long, default_value = "")]
    pub raison: String,
    /// Qui l'a constatée.
    #[arg(long, default_value = "cli")]
    pub source: String,
}

pub fn run(args: EntretienArgs, quiet: bool) -> Result<()> {
    let Some(genre) = Genre::depuis_publique(&args.genre) else {
        anyhow::bail!(
            "genre inconnu : « {} » — attendu degres ou enregistrements",
            args.genre
        );
    };
    if let Some(store) = args.attacher.as_deref() {
        crate::entretien::attacher(store, genre, &args.raison, &args.source)?;
        if !quiet {
            eprintln!(
                "entretien : demande attachée — {} / {}",
                store.display(),
                genre.nom()
            );
        }
    }
    if let Some(store) = args.accorder.as_deref() {
        crate::entretien::accorder(store, genre);
        if !quiet {
            eprintln!(
                "entretien : demande accordée — {} / {}",
                store.display(),
                genre.nom()
            );
        }
    }
    let demandes: Vec<Demande> = crate::entretien::en_attente();
    let age = crate::entretien::age_max_secondes();
    let corps = serde_json::json!({
        "en_attente": demandes.len(),
        "age_max_secondes": age,
        "demandes": demandes
            .iter()
            .map(|d| serde_json::json!({
                "store": d.store, "genre": d.genre.nom(),
                "raison": d.raison, "source": d.source, "t": d.t_ms,
            }))
            .collect::<Vec<_>>(),
        "protocole": "attaché en vol pendant que la base tourne ; rejoué et accordé à \
                      l'entretien de redémarrage (scripts/entretien-bases.sh)",
    });
    println!("{}", serde_json::to_string_pretty(&corps)?);
    Ok(())
}
