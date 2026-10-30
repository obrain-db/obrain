//! ── LE COUPE-CIRCUIT DE RÉPÉTITION ──────────────────────────────────────────
//!
//! PROBLÈME MESURÉ (signalé le 2026-09-16) : « l'engine a tendance à faire une
//! génération en boucle infinie ». Vérifié dans le code : la boucle d'outils est
//! bornée (4 à 6 tours), il n'y a AUCUN réessai, et la seule borne de longueur est
//! `plafond_generation(demande, restant)` — donc un modèle qui se répète écrit
//! jusqu'au plafond, ce qui se lit, à l'écran, comme une génération infinie. Rien,
//! nulle part, ne détecte la répétition.
//!
//! Ce module est PUR (aucune E/S, aucun réseau) : il se teste sans service, comme
//! les budgets de `engine.rs` qui prennent la fenêtre en paramètre. Il ne juge PAS
//! la qualité d'une réponse : il cherche un motif DÉGÉNÉRÉ — un bloc qui se répète
//! à l'identique, ou une ligne répétée à l'enfilade — et il rend de quoi le DIRE.
//!
//! ── POURQUOI DEUX RÈGLES, ET POURQUOI ELLES SONT PRUDENTES ──────────────────
//!
//! Un texte légitime se répète : une liste de fichiers, un tableau, une énumération
//! « oui… oui… oui », ou une réponse courte du genre « Bonjour ! 👋 » suivie d'un
//! rappel. Un détecteur qui crie au loup couperait des réponses JUSTES — ce serait
//! pire que le défaut. On exige donc, ENSEMBLE :
//!   · une répétition LONGUE (bloc de 40 caractères et plus, ou ligne de 8 et plus) ;
//!   · au moins TROIS (bloc) ou SIX (ligne) répétitions CONSÉCUTIVES ;
//!   · un texte déjà substantiel (≥ `MIN_TEXTE` caractères), pour qu'une réponse
//!     brève ne puisse jamais déclencher l'arrêt.
//! Les seuils sont nommés et testés un par un, y compris leurs bords.

/// Ce qui a été détecté — de quoi l'annoncer sans le deviner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repetition {
    /// Nombre de répétitions consécutives observées.
    pub fois: usize,
    /// Longueur du motif répété, en caractères.
    pub motif: usize,
    /// Un extrait court du motif, pour que le message le montre.
    pub extrait: String,
    /// La règle qui a conclu (`"bloc"` ou `"ligne"`).
    pub regle: &'static str,
}

impl Repetition {
    /// La phrase affichée à l'utilisateur — elle DIT ce qui s'est passé.
    pub fn phrase(&self) -> String {
        format!(
            "génération arrêtée : le modèle se répète ({} fois « {}… », motif de {} caractères, règle « {} »)",
            self.fois,
            self.extrait,
            self.motif,
            self.regle
        )
    }
}

/// Seuils du détecteur. Nommés pour être cités par les tests et le message.
///
/// Chacun se lit comme une raison, pas comme un réglage : c'est ce que les sept
/// cas de ce module vérifient, bords compris.
pub const MOTIF_MIN: usize = 40;      // un bloc plus court est trop banal pour conclure
/// Trois fois le MÊME bloc : ce n'est plus une liste.
pub const FOIS_MIN: usize = 3;
/// Une ligne courte se répète souvent légitimement.
pub const LIGNE_MIN: usize = 8;
/// Six lignes identiques d'affilée.
pub const LIGNES_MIN: usize = 6;
/// En dessous, on ne conclut RIEN (une réponse brève ne peut pas déclencher).
pub const MIN_TEXTE: usize = 600;
/// On ne regarde que la fin du texte.
pub const FENETRE: usize = 4096;

/// Le texte est-il en train de boucler ? `None` = on ne conclut pas.
///
/// `texte` est TOUT ce qui a été émis jusqu'ici (raisonnement ET réponse) : une
/// boucle se produit dans l'un comme dans l'autre, et le raisonnement est même le
/// premier endroit où le défaut a été observé dans ce dépôt.
pub fn boucle(texte: &str) -> Option<Repetition> {
    if texte.chars().count() < MIN_TEXTE {
        return None;
    }
    let fin: String = {
        let n = texte.chars().count();
        texte.chars().skip(n.saturating_sub(FENETRE)).collect()
    };
    // 1. Une LIGNE répétée à l'enfilade — le cas le plus visible à l'écran.
    if let Some(r) = ligne_repetee(&fin) {
        return Some(r);
    }
    // 2. Un BLOC répété à l'identique, à la fin du texte.
    bloc_repete(&fin)
}

fn bloc_repete(fin: &str) -> Option<Repetition> {
    let octets: Vec<char> = fin.chars().collect();
    // On cherche le plus PETIT motif qui se répète au moins `FOIS_MIN` fois à la
    // fin : un motif plus court est une preuve plus forte (une liste de 3 phrases
    // différentes ne peut pas produire un motif court commun).
    for motif in MOTIF_MIN..=200.min(octets.len() / FOIS_MIN) {
        let debut = octets.len() - motif;
        let reference: String = octets[debut..].iter().collect();
        let mut fois = 1usize;
        let mut i = debut;
        while i >= motif {
            let precedent: String = octets[i - motif..i].iter().collect();
            if precedent != reference {
                break;
            }
            fois += 1;
            i -= motif;
        }
        if fois >= FOIS_MIN {
            return Some(Repetition {
                fois,
                motif,
                extrait: reference.chars().take(60).collect::<String>().replace('\n', " "),
                regle: "bloc",
            });
        }
    }
    None
}

/// Une même ligne, non vide et assez longue, répétée `LIGNES_MIN` fois d'affilée.
fn ligne_repetee(fin: &str) -> Option<Repetition> {
    let mut lignes: Vec<&str> = fin.lines().collect();
    // ── UN TEXTE QUI FINIT PAR UN SAUT DE LIGNE N'A PAS DE LIGNE VIDE ────────
    // TROUVÉ PAR LE TEST : `"...\n".lines()` rend une dernière ligne VIDE, donc la
    // règle concluait « motif trop court » et laissait passer une ligne répétée six
    // fois — le cas le plus visible à l'écran. On regarde la dernière ligne NON
    // VIDE : c'est celle que l'utilisateur voit en bas de son écran.
    while lignes.last().is_some_and(|l| l.trim().is_empty()) {
        lignes.pop();
    }
    if lignes.len() < LIGNES_MIN {
        return None;
    }
    let mut fois = 1usize;
    let derniere = lignes[lignes.len() - 1];
    if derniere.trim().chars().count() < LIGNE_MIN {
        return None;
    }
    for l in lignes.iter().rev().skip(1) {
        if *l == derniere {
            fois += 1;
        } else {
            break;
        }
    }
    (fois >= LIGNES_MIN).then(|| Repetition {
        fois,
        motif: derniere.chars().count(),
        extrait: derniere.trim().chars().take(60).collect(),
        regle: "ligne",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn long_reponse(suite: &str) -> String {
        // Un préambule plausible (assez long pour passer MIN_TEXTE) puis la suite.
        // Le préambule finit par un SAUT DE LIGNE : sans lui, sa dernière ligne
        // fusionne avec la première ligne de `suite`, et une suite de six lignes
        // identiques n'en compte plus que cinq en fin de texte — c'est ce qui a
        // fait échouer le premier jet de ce test, pas le détecteur.
        let preambule = "Dans la base obrain, la lecture a porté sur les nœuds du module demandé, \
                         et voici ce qui en ressort, avec les chiffres réels du moteur.\n";
        preambule.repeat(8) + suite
    }

    #[test]
    fn une_reponse_normale_ne_declenche_rien() {
        let texte = long_reponse(
            "Le module contient 1 234 nœuds, 5 678 arêtes, et 12 communautés. \
             Les fonctions les plus appelées sont tcp_v4_rcv, e1000_probe et bus_register.",
        );
        assert_eq!(boucle(&texte), None);
    }

    #[test]
    fn une_liste_repetitive_mais_courte_ne_declenche_pas() {
        // Trois lignes identiques : c'est une liste, pas une boucle (il en faut six).
        let texte = long_reponse("fichier.c\nfichier.c\nfichier.c\n");
        assert_eq!(boucle(&texte), None);
    }

    #[test]
    fn un_texte_court_ne_conclut_jamais() {
        // Deux fois le même bloc, mais texte trop court : on ne conclut pas.
        let texte = "bonjour bonjour bonjour bonjour bonjour bonjour bonjour bonjour";
        assert_eq!(boucle(texte), None);
    }

    #[test]
    fn un_bloc_repete_trois_fois_est_detecte() {
        let bloc = "D'après le graphe, la fonction tcp_v4_rcv reçoit 11 077 appels entrants. ";
        let texte = long_reponse(&bloc.repeat(3));
        let r = boucle(&texte).expect("le bloc répété doit être détecté");
        assert_eq!(r.regle, "bloc");
        assert!(r.fois >= 3, "fois = {}", r.fois);
        assert!(r.phrase().contains("se répète"));
    }

    #[test]
    fn une_ligne_repete_six_fois_est_detectee() {
        let texte = long_reponse(&"tcp_v4_rcv -> e1000_probe\n".repeat(6));
        let r = boucle(&texte).expect("la ligne répétée doit être détectée");
        assert_eq!(r.regle, "ligne");
        assert!(r.fois >= 6);
    }

    #[test]
    fn le_raisonnement_boucle_est_detecte_comme_le_texte() {
        // Le même détecteur sert aux deux canaux : c'est là que le défaut est né.
        let pensee = long_reponse(&"Je dois appeler l'outil pour compter les nœuds du module. ".repeat(4));
        assert!(boucle(&pensee).is_some());
    }

    #[test]
    fn le_message_dit_la_regle_et_le_motif() {
        let texte = long_reponse(&"la même phrase encore et encore et encore, mot pour mot. ".repeat(4));
        let r = boucle(&texte).unwrap();
        let p = r.phrase();
        assert!(p.contains("génération arrêtée"), "{p}");
        assert!(p.contains("bloc") || p.contains("ligne"), "{p}");
    }
}
