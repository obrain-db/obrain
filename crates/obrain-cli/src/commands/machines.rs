//! Commande `obrain machines` — la flotte : ce que le registre connaît, et son
//! application dans une base.
//!
//! Trois gestes, et un seul ordre possible :
//!
//! ```text
//! obrain machines                                  # ce que l'on connaît
//! obrain machines --store <base> --appliquer        # ce qui devient un fait du graphe
//! obrain machines --store <base> --compte <c> --appliquer   # une seule base de compte
//! ```
//!
//! L'application ouvre la base en **EXCLUSIF** : elle échoue si un lecteur la
//! tient (le dashboard). C'est l'invariant du substrat, pas un défaut — même
//! contrainte que `obrain memoire --appliquer`.
//!
//! La sortie est du JSON sur `stdout` : c'est un rapport d'état, destiné autant à
//! un humain qu'à un service qui le déclenche.

use std::path::PathBuf;

use anyhow::Result;
use clap::Args;

#[derive(Args, Debug)]
pub struct MachinesArgs {
    /// Base où inscrire la flotte (ex. …/runtime-dev.obrain).
    /// Requis avec --appliquer.
    #[arg(long)]
    pub store: Option<PathBuf>,
    /// Applique le registre des machines dans la base (ouverture EXCLUSIVE).
    #[arg(long)]
    pub appliquer: bool,
    /// Ne écrire QUE les machines de ce compte (base d'un compte : une base
    /// `selfs/<compte>.obrain` que nul service ne tient en exclusif, donc une
    /// cible possible pour un timer).
    #[arg(long)]
    pub compte: Option<String>,
}

pub fn run(args: MachinesArgs, quiet: bool) -> Result<()> {
    if !args.appliquer {
        // ── ON INTERROGE LES DEUX SOURCES, ET C'EST LE POINT ────────────────
        // Une machine entre dans le graphe de DEUX façons : un `selfd` s'y
        // rattache (le registre), ou un MOTEUR y tourne et s'auto-déclare (le
        // journal des déclarations). N'interroger que le registre rendait la
        // réponse MUETTE pour un moteur seul — alors que le graphe, lui, le
        // nomme déjà : la liste et le graphe se seraient contredits.
        let faits = crate::machines::faits_avec_declarations(
            &crate::remote::toutes_les_machines(),
            &crate::machines::lire_declarations(&crate::machines::dir_journal_machines()),
        );
        let mut rattachees = 0usize;
        let mut declarees = 0usize;
        let items: Vec<serde_json::Value> = faits
            .iter()
            .map(|f| {
                if f.comptes.is_empty() {
                    declarees += 1;
                } else {
                    rattachees += 1;
                }
                // Les moteurs déclarés sont rendus à part, et JAMAIS fondus dans
                // les capacités : un moteur seul n'annonce aucune capacité, et
                // lui en prêter une serait « annoncer » à sa place.
                let moteurs: Vec<serde_json::Value> = f
                    .declarations
                    .iter()
                    .map(|d| {
                        serde_json::json!({
                            "instance": d.instance,
                            "base": d.base,
                            "store": d.store,
                            "mode": d.mode,
                            "ecoute": d.ecoute,
                            "declaree_ms": d.declaree_ms,
                            "pid": d.pid,
                        })
                    })
                    .collect();
                serde_json::json!({
                    "machine": f.machine,
                    // Vrai si un `selfd` s'est rattaché ici : c'est CE fait qui
                    // donne un compte et des capacités.
                    "rattachee": !f.comptes.is_empty(),
                    "comptes": f.comptes,
                    "capacites": f.capacites,
                    "premier_attache_ms": f.premier_attache_ms,
                    "derniere_presence_ms": f.derniere_presence_ms,
                    "incarnations": f.incarnations,
                    "moteurs": moteurs,
                })
            })
            .collect();
        let rapport = serde_json::json!({
            "question": "quelles machines connais-tu, et que sait faire chacune ?",
            "machines": items.len(),
            "rattachees": rattachees,
            "declarees_seulement": declarees,
            "connues": items,
            "aucune": items.is_empty(),
        });
        println!("{}", serde_json::to_string_pretty(&rapport)?);
        if !quiet && items.is_empty() {
            // Le vide est un FAIT, et il se dit — en distinguant les DEUX causes,
            // parce qu'elles n'appellent pas le même geste : rien ne s'est
            // rattaché (personne n'a de nœud), ou rien ne s'est déclaré (aucun
            // harness n'a journalisé son identité).
            eprintln!(
                "flotte : AUCUNE machine connue — ni rattachement dans le registre, ni \
                 auto-déclaration de moteur. Vérifiez le journal : {}",
                crate::machines::dir_journal_machines().display()
            );
        }
        return Ok(());
    }

    let Some(store) = args.store.as_deref() else {
        anyhow::bail!(
            "flotte : --store est requis avec --appliquer — indiquez la base qui doit porter \
             les nœuds « machine: » (celle que le harness sert, par exemple)"
        );
    };
    let r = crate::machines::appliquer(store, args.compte.as_deref())?;
    let rapport = serde_json::json!({
        "store": r.store,
        "machines": r.machines,
        // Compris dans `machines` : les nœuds d'INSTANCE (`machine:<hôte>#<base>`),
        // publiés à part parce que « combien de moteurs se sont déclarés » n'est
        // pas « combien de machines ».
        "instances": r.instances,
        "declarations": r.declarations,
        "connues": r.connues,
        "compte": args.compte,
        "faits_par": r.faits_par,
        "refusees": r.refusees
            .iter()
            .map(|(m, w)| serde_json::json!({ "machine": m, "raison": w }))
            .collect::<Vec<_>>(),
    });
    println!("{}", serde_json::to_string_pretty(&rapport)?);
    if !quiet {
        if r.connues == 0 && r.declarations == 0 {
            eprintln!("flotte : rien à appliquer — AUCUNE machine connue du registre");
        } else {
            eprintln!(
                "flotte : {} nœud(s) écrit(s) dans {} (dont {} instance(s) de moteur, \
                 {} déclaration(s)) · {} refusé(s)",
                r.machines,
                r.store,
                r.instances,
                r.declarations,
                r.refusees.len()
            );
        }
    }
    Ok(())
}
