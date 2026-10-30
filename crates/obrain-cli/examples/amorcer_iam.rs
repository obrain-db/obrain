//! AMORÇAGE IAM — fabriquer le premier jeton, hors bande.
//!
//! Audit du 2026-09-13 : la garde IAM était branchable, mais **aucun chemin ne
//! permettait d'obtenir un jeton** — les identités, rôles, politiques et jetons
//! s'appelaient depuis la bibliothèque, et rien ne les exposait. Armer la garde
//! supposait donc d'écrire ce programme. Il est écrit, il est ici, et il dit ce
//! qu'il fait.
//!
//! Il vit dans le CLI (et non dans `obrain-iam`) parce qu'il lui faut ouvrir une
//! **base du produit** : la bibliothèque IAM ne connaît que
//! `Arc<dyn GraphStoreMut>`, elle ne sait pas ouvrir un `.obrain`.
//!
//! ```text
//! cargo run --release -p obrain-cli --features iam --example amorcer_iam -- \
//!     <base.obrain> <compte> <espace> [utilisateur]
//! ```
//!
//! Ce qu'il pose, exactement (et il le réimprime) :
//!   * un utilisateur (`<utilisateur>`) et un rôle du même nom ;
//!   * une politique `Allow` pour `graph:read` **et** `graph:write` sur la seule
//!     ressource `("tenant", <espace>)` — donc une base, pas les autres ;
//!   * l'attachement rôle → politique, utilisateur → rôle ;
//!   * un jeton de session, valable 24 h, **imprimé une seule fois**.
//!
//! Le jeton n'est jamais stocké en clair : la bibliothèque n'en garde que
//! l'empreinte. Perdre la sortie de cette commande oblige donc à en émettre un
//! autre — c'est voulu.
//!
//! ── CE QU'IL NE FAIT PAS ────────────────────────────────────────────────
//! Il ne crée pas la base : elle doit exister (`obrain init <chemin>`). Il
//! n'accorde rien d'autre que l'espace demandé (aucun `*`), et il n'arme rien —
//! armer est le rôle de l'exploitant (`OBRAIN_IAM=1` sur le serveur).

#[cfg(feature = "iam")]
fn main() -> anyhow::Result<()> {
    use obrain_iam::model::PolicyEffect;
    use obrain_iam::orn::Orn;
    use obrain_iam::provider::{AuthProvider, AuthRequest, ObrainIamProvider};
    use obrain_iam::store::IamStore;
    use std::sync::Arc;

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        anyhow::bail!(
            "usage : amorcer_iam <base.obrain> <compte> <espace> [utilisateur]\n\
             exemple : amorcer_iam ~/.local/share/obrain-harness/data/runtime-dev.obrain \
             obrain runtime-dev"
        );
    }
    let (chemin, compte, espace) = (&args[0], &args[1], &args[2]);
    let utilisateur = args.get(3).cloned().unwrap_or_else(|| "admin".to_string());
    // Mot de passe : fourni, ou ENGENDRÉ et imprimé. Un mot de passe par défaut
    // (« admin », « changeme ») est le défaut qu'on ne veut pas laisser entrer
    // dans un parc : s'il est engendré, il est imprimé une fois, et l'opérateur
    // le note ou le remplace.
    let mot_de_passe = args.get(4).cloned().unwrap_or_else(|| {
        // 24 caractères hexadécimaux, tirés de l'horloge et du PID : suffisant
        // pour un amorçage, et sans dépendance supplémentaire.
        let melange = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
            ^ (std::process::id() as u128) << 32;
        format!("{melange:024x}")
    });

    // La base doit exister : on ne fabrique pas une identité dans le vide.
    let db = obrain_engine::ObrainDB::open(std::path::Path::new(chemin))
        .map_err(|e| anyhow::anyhow!("ouverture de {chemin} : {e}"))?;
    let store = Arc::new(IamStore::new(db.graph_store()));

    // 1) L'identité et son rôle.
    let id_utilisateur = format!("u-{utilisateur}");
    let id_role = format!("r-{utilisateur}");
    match store.create_user(&id_utilisateur, &utilisateur, None) {
        Ok(_) => println!("utilisateur  : {utilisateur} ({id_utilisateur})"),
        Err(e) => println!("utilisateur  : {utilisateur} — {e} (déjà présent ?)"),
    }
    match store.create_role(&id_role, &utilisateur, Some("amorçage IAM")) {
        Ok(_) => println!("rôle          : {utilisateur} ({id_role})"),
        Err(e) => println!("rôle          : {utilisateur} — {e} (déjà présent ?)"),
    }

    // 2) La politique, bornée à CET espace — jamais un joker.
    let ressource = Orn::tenant(compte, espace);
    let id_politique = format!("p-{espace}-{utilisateur}");
    match store.create_policy(
        &id_politique,
        &format!("{espace} : lecture+écriture pour {utilisateur}"),
        PolicyEffect::Allow,
        &["graph:read", "graph:write"],
        std::slice::from_ref(&ressource),
    ) {
        Ok(_) => println!("politique     : Allow graph:read,graph:write sur {ressource}"),
        Err(e) => println!("politique     : {e} (déjà présente ?)"),
    }
    // ── ET LA PISTE D'AUDIT, SUR UNE ACTION NOMMÉE ─────────────────────────
    // `iam:audit` est une action à part : une politique peut accorder la
    // lecture de la piste sans accorder celle du graphe — et l'inverse. On
    // l'accorde ici à la même identité, pour que la piste soit lisible.
    let id_audit = format!("p-audit-{espace}-{utilisateur}");
    match store.create_policy(
        &id_audit,
        &format!("{espace} : piste d'audit pour {utilisateur}"),
        PolicyEffect::Allow,
        &["iam:audit"],
        std::slice::from_ref(&ressource),
    ) {
        Ok(_) => println!("politique     : Allow iam:audit sur {ressource}"),
        Err(e) => println!("politique     : {e} (déjà présente ?)"),
    }
    store.attach_policy_to_role(&id_role, &id_audit)?;

    store.attach_role(&id_utilisateur, &id_role)?;
    store.attach_policy_to_role(&id_role, &id_politique)?;
    println!("attachements  : {utilisateur} → rôle → politique");

    // ── LE MOT DE PASSE : la porte d'entrée d'un HUMAIN ────────────────────
    // Sans lui, la chaîne ne savait émettre que des jetons fabriqués hors bande :
    // un navigateur ne pouvait pas se connecter. Le mot de passe est haché en
    // bcrypt par la bibliothèque (jamais stocké en clair).
    let amorceur = ObrainIamProvider::new(Arc::clone(&store), compte);
    amorceur.set_user_password(&id_utilisateur, &mot_de_passe)?;

    // 3) Le jeton. `Internal` est le chemin d'amorçage documenté par la
    //    bibliothèque (« trusted, no credential check ») : c'est exactement le
    //    geste d'un exploitant qui vient de créer l'identité.
    let provider = ObrainIamProvider::new(Arc::clone(&store), compte);
    let identite = provider.authenticate(&AuthRequest::Internal {
        username: utilisateur.clone(),
    })?;
    let creds = provider.create_session(&identite, 24 * 3600)?;

    println!();
    println!("JETON (valable 24 h, imprimé une seule fois) :");
    println!("{}", creds.token);
    println!();
    println!("MOT DE PASSE (à changer, ou à noter) :");
    println!("  utilisateur : {utilisateur}");
    println!("  mot de passe: {mot_de_passe}");
    println!();
    println!("Se connecter (le cookie vaut session pour un navigateur) :");
    println!("  curl -c cookies.txt -X POST http://127.0.0.1:8432/api/auth/login \\");
    println!("       -d '{{\"username\":\"{utilisateur}\",\"password\":\"{mot_de_passe}\"}}'");
    println!();
    println!("Pour armer la garde sur le serveur de graphe :");
    println!("  OBRAIN_IAM=1 OBRAIN_IAM_ACCOUNT={compte} OBRAIN_IAM_STORE={chemin} \\");
    println!("    ./target/release/obrain viz serve …");
    println!("Puis, côté client :  Authorization: Bearer <jeton>");
    println!("Pour le pont MCP  :  OBRAIN_IAM_TOKEN=<jeton> dans l'environnement du harness");
    Ok(())
}

#[cfg(not(feature = "iam"))]
fn main() {
    eprintln!(
        "cet exemple exige la chaîne IAM : \
         cargo run --release -p obrain-cli --features iam --example amorcer_iam -- <base> <compte> <espace>"
    );
}
