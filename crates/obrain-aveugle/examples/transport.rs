//! Le porteur aveugle, en démonstration — **et ce qu'il peut dire de son trafic**.
//!
//! C'est la forme du protocole de la variante V-B : l'utilisateur parle à un
//! self qui route, et le self ne comprend rien de ce qu'il route. Cette
//! démonstration fait traverser un porteur par une requête et son résultat, puis
//! affiche **exactement** ce que le porteur a pu observer — pour que la
//! différence entre « transporter » et « comprendre » se voie à l'écran plutôt
//! que dans une promesse.
//!
//! ```text
//! cargo run -p obrain-aveugle --example transport
//! ```

use obrain_aveugle::{Adresse, Charge, ErreurAveugle, Porteur, ouvrir, sceller};
use obrain_coffre::{CleEspace, Jeton};
use obrain_journal::CleJournal;

const REQUETE: &str = "graph:read MATCH (n:Client) WHERE n.iban = 'FR76-LE-SECRET-DU-CLIENT'";
const RESULTAT: &str = "resultat-du-calcul-8c1d55aa";

fn main() {
    if let Err(message) = executer() {
        eprintln!("transport : {message}");
        std::process::exit(1);
    }
}

fn executer() -> Result<(), String> {
    // ── Les extrémités ──────────────────────────────────────────────────────
    // Les adresses sont tirées au sort : le porteur voit deux jetons opaques qui
    // se parlent, pas « la machine d'Alice » et « celle de Bob ».
    let alice = Adresse::tirer().map_err(|e| e.to_string())?;
    let bob = Adresse::tirer().map_err(|e| e.to_string())?;

    // ── Le porteur ──────────────────────────────────────────────────────────
    // Il détient une clé de **signature** pour son registre de transitions, et
    // aucune clé d'espace : c'est structurel, pas une convention.
    let cle_porteur = CleJournal::engendrer().map_err(|e| e.to_string())?;
    let publique = cle_porteur.publique();
    let mut porteur = Porteur::neuf("porteur-alpha", cle_porteur, 8).map_err(|e| e.to_string())?;
    porteur.enregistrer(alice);
    porteur.enregistrer(bob);

    // ── L'espace de l'utilisateur : la clé que le porteur n'aura jamais ─────
    let cle_espace = CleEspace::engendrer("espace-alpha").map_err(|e| e.to_string())?;

    println!("── 1. Ce qui traverse ──────────────────────────────────────────────");
    let aller = sceller(
        &cle_espace,
        &Charge::nouvelle(alice, bob, REQUETE.as_bytes().to_vec()),
    )
    .map_err(|e| e.to_string())?;
    println!(
        "  la requête fait {} octets, scellée : {} octets",
        REQUETE.len(),
        aller.len()
    );

    let jeton = Jeton::neuf(&cle_espace).map_err(|e| e.to_string())?;
    let mut retour = Charge::nouvelle(bob, alice, RESULTAT.as_bytes().to_vec());
    retour.jeton = Some(
        jeton
            .emettre("graph:write", 2_000_000_000)
            .map_err(|e| e.to_string())?,
    );
    let en_retour = sceller(&cle_espace, &retour).map_err(|e| e.to_string())?;
    println!(
        "  le résultat et son jeton de capacité : {} octets scellés (même classe)",
        en_retour.len()
    );

    let recu_aller = porteur
        .deposer(alice, bob, aller.clone(), 1_700_000_000)
        .map_err(|e| e.to_string())?;
    porteur
        .deposer(bob, alice, en_retour.clone(), 1_700_000_001)
        .map_err(|e| e.to_string())?;

    // ── Ce que le porteur a vu, et rien d'autre ─────────────────────────────
    println!();
    println!("── 2. Tout ce que le porteur peut dire de son trafic ───────────────");
    for (i, vue) in porteur.ce_que_jai_vu().iter().enumerate() {
        println!(
            "  {i} : {:?} → {:?}, {} octets, à t={}",
            vue.expediteur, vue.destinataire, vue.taille, vue.quand
        );
    }
    let mut vu = Vec::new();
    for vue in porteur.ce_que_jai_vu() {
        vu.extend_from_slice(&vue.vers_octets());
    }
    println!("  (c'est tout : aucun octet de contenu, et le test le vérifie par recherche)");

    // ── La livraison ────────────────────────────────────────────────────────
    println!();
    println!("── 3. Ce que les extrémités, elles, obtiennent ─────────────────────");
    let livre = porteur.retirer(&bob).ok_or("rien pour Bob")?;
    let recue = ouvrir(&cle_espace, &bob, &livre.octets).map_err(|e| e.to_string())?;
    println!("  Bob a reçu : {}", String::from_utf8_lossy(&recue.quoi));

    let livre = porteur.retirer(&alice).ok_or("rien pour Alice")?;
    let recue = ouvrir(&cle_espace, &alice, &livre.octets).map_err(|e| e.to_string())?;
    println!("  Alice a reçu : {}", String::from_utf8_lossy(&recue.quoi));
    if let Some(jeton_recu) = &recue.jeton {
        let portee = jeton
            .verifier(jeton_recu, "graph:write", 1_700_000_002)
            .map_err(|e| e.to_string())?;
        println!(
            "  son jeton autorise « {} » : vérifié à l'arrivée",
            portee.action
        );
    }

    // ── Ce que le porteur ne peut pas faire ─────────────────────────────────
    println!();
    println!("── 4. Ce que le porteur ne peut pas faire ──────────────────────────");
    let autre_espace = CleEspace::engendrer("espace-beta").map_err(|e| e.to_string())?;
    println!(
        "  ouvrir avec une autre clé d'espace : {}",
        refus(ouvrir(&autre_espace, &bob, &aller))
    );
    println!(
        "  ouvrir, en tant qu'Alice, un message scellé pour Bob : {}",
        refus(ouvrir(&cle_espace, &alice, &aller))
    );
    println!(
        "  déposer deux fois la même enveloppe : {}",
        refus(porteur.deposer(alice, bob, aller.clone(), 2))
    );
    println!(
        "  déposer autre chose qu'une taille du protocole : {}",
        refus(porteur.deposer(alice, bob, vec![0u8; 1234], 3))
    );

    // ── Ce qu'il ne peut pas renier ─────────────────────────────────────────
    println!();
    println!("── 5. Ce que le porteur ne peut pas renier ─────────────────────────");
    porteur.verifier(&publique).map_err(|e| e.to_string())?;
    porteur
        .journal()
        .accorde_avec(&recu_aller)
        .map_err(|e| e.to_string())?;
    println!("  le registre des transitions est cohérent et signé");
    println!("  le reçu du routage d'Alice s'accorde toujours avec lui");
    println!(
        "  (et s'il le reconstruit et le resigne, ce reçu cesse de s'accorder — c'est le client qui voit)"
    );

    println!();
    println!("Tout est passé.");
    Ok(())
}

fn refus<T>(resultat: Result<T, ErreurAveugle>) -> String {
    match resultat {
        Err(e) => format!("refusé ({e})"),
        Ok(_) => "ACCEPTÉ — ce serait un défaut".to_owned(),
    }
}
