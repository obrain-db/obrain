//! Commande `obrain memoire` — l'état de la mémoire du chat, et son application.
//!
//! Deux gestes, et un seul ordre possible :
//!
//! ```text
//! obrain memoire --self self:UTILISATEUR              # ce qui est en attente
//! obrain memoire --store <base-du-self> --appliquer     # ce qui est écrit
//! ```
//!
//! L'application ouvre la base en EXCLUSIF : elle échoue si un lecteur la tient
//! (le dashboard). C'est l'invariant du substrat, pas un défaut — d'où
//! `scripts/memoire.sh`, qui arrête, applique, relance.
//!
//! La sortie est du JSON sur `stdout` : c'est un rapport d'état, destiné autant
//! à un humain qu'au tableau de bord (qui affiche le nombre d'entrées en attente).

use std::path::PathBuf;

use anyhow::Result;
use clap::Args;

#[derive(Args, Debug)]
pub struct MemoireArgs {
    /// Base du Self où appliquer la mémoire (ex. …/self-demo.obrain).
    /// Requis avec --appliquer.
    #[arg(long)]
    pub store: Option<PathBuf>,
    /// Self concerné (défaut : le Self de l'agent).
    ///
    /// `--self` et non `--self-qn` : le nom du drapeau suit le vocabulaire du
    /// domaine (« le Self »), pas celui du champ interne.
    #[arg(long = "self", default_value = "self:UTILISATEUR")]
    pub self_qn: String,
    /// Applique le journal dans la base du Self (ouverture EXCLUSIVE).
    #[arg(long)]
    pub appliquer: bool,
}

pub fn run(args: MemoireArgs, quiet: bool) -> Result<()> {
    let journal = crate::memoire::journal(&args.self_qn);
    let entrees = crate::memoire::en_attente(&args.self_qn);

    if !args.appliquer {
        let items: Vec<serde_json::Value> = entrees
            .iter()
            .map(|e| {
                serde_json::json!({
                    "t": e.t_ms,
                    "genre": e.intention.genre(),
                    "reference": e.intention.reference(),
                    "provenance": e.provenance,
                })
            })
            .collect();
        let rapport = serde_json::json!({
            "self": args.self_qn,
            "journal": journal.to_string_lossy(),
            "en_attente": entrees.len(),
            "entrees": items,
        });
        println!("{}", serde_json::to_string_pretty(&rapport)?);
        if !quiet && entrees.is_empty() {
            eprintln!("mémoire : rien en attente pour {}", args.self_qn);
        }
        return Ok(());
    }

    let Some(store) = args.store.as_deref() else {
        anyhow::bail!(
            "mémoire : --store est requis avec --appliquer — indiquez la base du Self \
             (celle qui porte les nœuds Self / SelfState / Goal / Procedure)"
        );
    };
    let r = crate::memoire::appliquer(store, &args.self_qn)?;
    let rapport = serde_json::json!({
        "self": r.self_qn,
        "store": r.store,
        "appliquees": r.appliquees,
        "refusees": r.refusees.iter().map(|(g, w)| serde_json::json!({
            "genre": g, "raison": w
        })).collect::<Vec<_>>(),
        "version_self": r.version_self,
        "journal": journal.to_string_lossy(),
    });
    println!("{}", serde_json::to_string_pretty(&rapport)?);
    if !quiet {
        eprintln!(
            "mémoire : {} appliquée(s) dans {} · {} refusée(s){}",
            r.appliquees,
            r.store,
            r.refusees.len(),
            r.version_self
                .map(|v| format!(" · SelfState v{v}"))
                .unwrap_or_default()
        );
    }
    Ok(())
}
