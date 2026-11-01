//! ── LA GARDE : CE QUI FAIT QU'UNE GÉNÉRATION S'ARRÊTE ───────────────────────
//!
//! Trois raisons d'arrêter, mesurées toutes les trois dans ce dépôt :
//!
//! | raison | défaut observé | ce qui l'arrête ici |
//! |---|---|---|
//! | le **client est parti** | un onglet fermé laissait l'engine générer, à 600 s de délai par appel de modèle | `client_parti` (drapeau partagé, posé par l'écrivain du flux) |
//! | la **question a dépassé son budget** | la seule borne était la longueur (`plafond_generation`), donc une question pouvait durer indéfiniment en enchaînant les tours d'outils | `Reglages::budget` (durée TOTALE, pas par appel) |
//! | le **modèle se répète** | « génération en boucle infinie » — le modèle écrivait jusqu'au plafond | [`crate::repetition`], sur la prose accumulée (raisonnement ET réponse) |
//!
//! # Pourquoi un objet, et pas trois fonctions
//!
//! Parce que les trois raisons doivent être consultées aux mêmes endroits d'une
//! boucle, et qu'aujourd'hui elles étaient recopiées — ou oubliées. Une garde
//! s'instancie une fois par question, se consulte **avant chaque lecture** et
//! **après chaque fragment de prose**, et retient le motif du premier arrêt :
//! un arrêt ne se rediscute pas, et il se DIT (voir [`Garde::phrase_arret`]).
//!
//! # Ce qu'elle ne fait pas
//!
//! Elle n'ouvre aucune connexion, ne lit aucun flux et ne connaît aucun modèle :
//! elle reçoit. C'est ce qui la rend testable sans service — et appelable depuis
//! `selfd`, qui n'a pas de dépendances à dépenser.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::repetition::Repetition;

/// Réglages d'une garde. Nommés pour être cités par les tests et les messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reglages {
    /// Durée TOTALE accordée à une question (tous appels au modèle confondus).
    ///
    /// C'est une somme, pas un délai par appel : un modèle local lent peut
    /// légitimement mettre 30 s, mais une question qui en enchaîne dix a un
    /// problème que seul un budget total révèle.
    pub budget: Duration,
    /// Nombre maximal d'appels d'outil retenus pour un même tour de modèle.
    ///
    /// Sert de plafond à l'allocation guidée par le flux : un index venu du
    /// service distant ne doit jamais dimensionner un `Vec`.
    pub max_appels_par_tour: usize,
}

impl Default for Reglages {
    fn default() -> Self {
        // Les valeurs du chat : mesurées, pas choisies par goût (voir le
        // commentaire de `BUDGET_QUESTION_S` dans `obrain-cli/src/engine.rs`).
        Self { budget: Duration::from_secs(300), max_appels_par_tour: 8 }
    }
}

/// Pourquoi une génération s'est arrêtée. Le motif se dit, il ne se devine pas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Motif {
    /// Le client a fermé : plus personne n'écoute, on cesse d'écrire.
    ClientParti,
    /// La question a dépassé son budget total (en secondes).
    Budget(u64),
    /// Le modèle se répète (bloc ou ligne, avec le détail de ce qui a été vu).
    Repetition(Repetition),
}

impl Motif {
    /// Le code stable du motif — pour un journal, une métrique, un test.
    pub fn code(&self) -> &'static str {
        match self {
            Motif::ClientParti => "client_parti",
            Motif::Budget(_) => "budget_question",
            Motif::Repetition(_) => "repetition",
        }
    }

    /// La phrase montrée à l'utilisateur : elle DIT ce qui s'est passé.
    pub fn phrase(&self) -> String {
        match self {
            Motif::ClientParti => "génération arrêtée : le client a fermé le flux".to_string(),
            Motif::Budget(s) => {
                format!("réponse interrompue : la question a dépassé son budget de {s} s")
            }
            Motif::Repetition(r) => r.phrase(),
        }
    }
}

/// Le verdict d'une génération en cours : `Ok(())` = on continue.
pub type Verdict = Result<(), Motif>;

/// La garde d'UNE question.
#[derive(Debug)]
pub struct Garde {
    debut: Instant,
    reglages: Reglages,
    client_parti: Option<Arc<AtomicBool>>,
    prose: String,
    motif: Option<Motif>,
}

impl Garde {
    /// Une garde qui ne surveille que le temps et la répétition.
    pub fn nouvelle(reglages: Reglages) -> Self {
        Self { debut: Instant::now(), reglages, client_parti: None, prose: String::new(), motif: None }
    }

    /// Une garde qui surveille en plus le départ du client.
    ///
    /// Le drapeau est *partagé* : c'est l'écrivain du flux qui le pose, à la
    /// première écriture refusée. La garde ne le pose jamais elle-même — elle
    /// n'écrit nulle part.
    pub fn avec_client(reglages: Reglages, client_parti: Option<Arc<AtomicBool>>) -> Self {
        let mut g = Self::nouvelle(reglages);
        g.client_parti = client_parti;
        g
    }

    /// Les réglages en vigueur.
    pub fn reglages(&self) -> Reglages {
        self.reglages
    }

    /// Le temps écoulé depuis l'ouverture de la question.
    pub fn ecoule(&self) -> Duration {
        self.debut.elapsed()
    }

    /// Le client est-il parti ? (`false` quand aucun drapeau n'est surveillé.)
    pub fn client_parti(&self) -> bool {
        self.client_parti.as_ref().is_some_and(|d| d.load(Ordering::Relaxed))
    }

    /// La prose vue jusqu'ici (raisonnement ET réponse confondus).
    pub fn prose_vue(&self) -> &str {
        &self.prose
    }

    /// Le motif du premier arrêt décidé, s'il y en a un.
    pub fn motif(&self) -> Option<&Motif> {
        self.motif.as_ref()
    }

    /// Le motif a-t-il déjà été décidé ? (Un arrêt ne se rediscute pas.)
    pub fn arrete(&self) -> bool {
        self.motif.is_some()
    }

    /// La phrase d'arrêt, vide tant qu'aucun arrêt n'a été décidé.
    pub fn phrase_arret(&self) -> String {
        self.motif.as_ref().map(Motif::phrase).unwrap_or_default()
    }

    /// Décide l'arrêt — le PREMIER motif gagne, les suivants sont ignorés.
    ///
    /// Sans cette règle, un budget dépassé réécrirait le motif « client parti »
    /// constaté avant lui, et le journal raconterait la mauvaise histoire.
    pub fn arreter(&mut self, motif: Motif) -> Motif {
        if let Some(premier) = &self.motif {
            return premier.clone();
        }
        self.motif = Some(motif.clone());
        motif
    }

    /// Le point de contrôle d'un tour de boucle : client parti, puis budget.
    ///
    /// À appeler **avant** chaque appel au modèle et **entre** deux lectures du
    /// flux. Une garde déjà arrêtée rend son motif, sans le recalculer.
    pub fn tour(&mut self) -> Verdict {
        if let Some(m) = &self.motif {
            return Err(m.clone());
        }
        if self.client_parti() {
            return Err(self.arreter(Motif::ClientParti));
        }
        let ecoule = self.ecoule();
        if ecoule >= self.reglages.budget {
            return Err(self.arreter(Motif::Budget(self.reglages.budget.as_secs())));
        }
        Ok(())
    }

    /// Le point de contrôle du texte : on accumule, puis on juge.
    ///
    /// Rend `Err` au PREMIER motif dégénéré. La prose est accumulée même quand
    /// la détection ne conclut pas : c'est la fenêtre du texte vu qui décide, et
    /// un fragment seul ne dit rien.
    pub fn prose(&mut self, texte: &str) -> Verdict {
        if let Some(m) = &self.motif {
            return Err(m.clone());
        }
        self.prose.push_str(texte);
        match crate::repetition::boucle(&self.prose) {
            Some(r) => Err(self.arreter(Motif::Repetition(r))),
            None => Ok(()),
        }
    }

    /// Le « continuer ? » d'un rappel de flux : `true` = on lit la suite.
    ///
    /// C'est la seule chose qu'un client SSE a besoin de savoir, et c'est
    /// exactement ce que `obrain-httpmin` ne permettait pas de dire.
    pub fn continuer(&mut self) -> bool {
        self.tour().is_ok()
    }

    /// Ouvre un tour de modèle : la prose repart à zéro, **le reste non**.
    ///
    /// La répétition se juge sur ce qu'un même tour a émis — deux tours qui se
    /// ressemblent sont le lot d'une conversation normale (une formule
    /// d'introduction répétée à chaque étape n'est pas une boucle). Mais
    /// l'horloge et le drapeau du client, eux, ne se remettent JAMAIS à zéro :
    /// c'est la somme de la question qui est budgétée, pas chaque tour.
    pub fn nouvelle_page(&mut self) {
        self.prose.clear();
    }

    /// L'instantané à consulter **avant chaque lecture** d'un flux.
    ///
    /// Un flux SSE a besoin de deux choses à la fois : un rappel qui juge ce
    /// qu'il vient de lire (et qui a donc besoin de la garde entière), et une
    /// commande d'arrêt consultée avant chaque ligne — parce qu'un client parti
    /// ou un budget épuisé ne se voient pas dans le texte. Deux points d'entrée
    /// qui toucheraient la même garde mutable ne peuvent pas coexister dans une
    /// même signature ; cette veille est la copie bon marché qui lève la
    /// difficulté sans mentir sur l'état : elle lit le MÊME drapeau partagé et
    /// la MÊME échéance.
    pub fn veille(&self) -> Veille {
        Veille {
            debut: self.debut,
            budget: self.reglages.budget,
            client_parti: self.client_parti.as_ref().map(Arc::clone),
        }
    }
}

/// Ce qui décide, AVANT une lecture, si l'on continue de lire.
#[derive(Debug, Clone)]
pub struct Veille {
    debut: Instant,
    budget: Duration,
    client_parti: Option<Arc<AtomicBool>>,
}

impl Veille {
    /// `true` = on lit la suite. `false` = le client est parti ou le budget
    /// est épuisé : la boucle appelante doit poser le motif (`Garde::tour`).
    pub fn continuer(&self) -> bool {
        if self.client_parti.as_ref().is_some_and(|d| d.load(Ordering::Relaxed)) {
            return false;
        }
        self.debut.elapsed() < self.budget
    }
}

/// L'index d'un appel d'outil venu du flux est-il exploitable ?
///
/// `false` quand il dépasse le plafond par tour. L'appelant **ignore** alors
/// l'entrée au lieu de dimensionner une structure sur elle : un index aberrant
/// n'est pas une erreur à propager (le flux reste lisible), c'est une entrée à
/// jeter. Défaut mesuré dans ce dépôt : `while appels.len() <= i` avec
/// `i = 1e9` allouait ~70 Go et faisait avorter le processus.
pub fn index_appel_valide(index: usize, max: usize) -> bool {
    index < max
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prose_longue(suite: &str) -> String {
        // Un préambule plausible, assez long pour que la détection ait le droit
        // de conclure (voir `repetition::MIN_TEXTE`).
        "Dans la base obrain, la lecture a porté sur les nœuds du module demandé.\n".repeat(10) + suite
    }

    #[test]
    fn une_generation_normale_ne_declenche_rien() {
        let mut g = Garde::nouvelle(Reglages::default());
        assert!(g.tour().is_ok());
        assert!(g.prose(&prose_longue("Le module contient 1 234 nœuds et 12 communautés.")).is_ok());
        assert!(!g.arrete());
        assert_eq!(g.phrase_arret(), "");
        assert!(g.continuer());
    }

    #[test]
    fn le_budget_s_arrete_et_se_dit() {
        let reglages = Reglages { budget: Duration::from_millis(1), ..Reglages::default() };
        let mut g = Garde::nouvelle(reglages);
        std::thread::sleep(Duration::from_millis(4));
        let motif = g.tour().expect_err("le budget dépassé doit arrêter");
        assert_eq!(motif.code(), "budget_question");
        assert!(g.phrase_arret().contains("budget"), "{}", g.phrase_arret());
        // L'arrêt est DÉFINITIF : la garde ne se remet pas à zéro toute seule.
        assert!(g.tour().is_err());
        assert!(g.arrete());
    }

    #[test]
    fn le_budget_est_une_somme_et_ne_repart_pas_a_chaque_tour() {
        // Deux tours courts dans un budget total court : c'est la somme qui
        // décide, pas chaque appel. Sans cette règle, dix tours de 30 s
        // passeraient sous un budget de 300 s sans jamais l'atteindre.
        let reglages = Reglages { budget: Duration::from_millis(30), ..Reglages::default() };
        let mut g = Garde::nouvelle(reglages);
        let mut tours = 0usize;
        while g.tour().is_ok() {
            std::thread::sleep(Duration::from_millis(5));
            tours += 1;
            assert!(tours < 100, "la garde n'a pas arrêté");
        }
        assert!(tours >= 1, "la garde a arrêté trop tôt");
        assert_eq!(g.motif().map(Motif::code), Some("budget_question"));
    }

    #[test]
    fn un_client_parti_arrete_la_lecture() {
        let drapeau = Arc::new(AtomicBool::new(false));
        let mut g = Garde::avec_client(Reglages::default(), Some(Arc::clone(&drapeau)));
        assert!(g.continuer(), "tant que le client est là, on lit");
        drapeau.store(true, Ordering::Relaxed);
        assert!(!g.continuer());
        assert_eq!(g.motif().map(Motif::code), Some("client_parti"));
        assert!(g.phrase_arret().contains("client"));
    }

    #[test]
    fn la_prose_qui_boucle_est_detectee_et_l_arret_est_pose() {
        let mut g = Garde::nouvelle(Reglages::default());
        let bloc = "D'après le graphe, la fonction tcp_v4_rcv reçoit 11 077 appels entrants. ";
        assert!(g.prose(&prose_longue(&bloc.repeat(2))).is_ok(), "deux fois n'est pas encore une boucle");
        let motif = g.prose(bloc).expect_err("la troisième fois doit arrêter");
        assert_eq!(motif.code(), "repetition");
        assert!(matches!(motif, Motif::Repetition(_)));
        assert!(g.phrase_arret().contains("se répète"));
        // La prose vue est conservée : l'exploitant peut la montrer.
        assert!(g.prose_vue().contains("tcp_v4_rcv"));
    }

    #[test]
    fn le_premier_motif_gagne_et_ne_se_reecrit_pas() {
        // Un budget dépassé APRÈS un client parti ne doit pas raconter l'histoire
        // du budget : le journal doit dire la cause réelle du premier arrêt.
        let mut g = Garde::nouvelle(Reglages { budget: Duration::from_millis(1), ..Reglages::default() });
        let premier = g.arreter(Motif::ClientParti);
        assert_eq!(premier.code(), "client_parti");
        std::thread::sleep(Duration::from_millis(4));
        let suivant = g.arreter(Motif::Budget(1));
        assert_eq!(suivant.code(), "client_parti", "le premier arrêt ne se réécrit pas");
        assert_eq!(g.motif().map(Motif::code), Some("client_parti"));
    }

    #[test]
    fn un_index_d_appel_aberrant_est_refuse() {
        // Le défaut mesuré : `while appels.len() <= i` avec i = 1e9.
        assert!(index_appel_valide(0, 8));
        assert!(index_appel_valide(7, 8));
        assert!(!index_appel_valide(8, 8), "un index égal au plafond est hors plafond");
        assert!(!index_appel_valide(1_000_000_000, 8));
        assert!(!index_appel_valide(usize::MAX, 8));
    }

    #[test]
    fn la_veille_voit_le_meme_etat_que_la_garde() {
        // Deux points d'entrée (le rappel de flux, la commande d'arrêt) doivent
        // lire le MÊME état : une veille qui aurait sa propre horloge ou son
        // propre drapeau serait la troisième copie du défaut.
        let drapeau = Arc::new(AtomicBool::new(false));
        let mut g = Garde::avec_client(Reglages::default(), Some(Arc::clone(&drapeau)));
        let v = g.veille();
        assert!(v.continuer());
        assert!(g.tour().is_ok());
        drapeau.store(true, Ordering::Relaxed);
        assert!(!v.continuer(), "la veille voit le drapeau partagé");
        assert!(g.tour().is_err(), "la garde conclut sur le même drapeau");
    }

    #[test]
    fn la_veille_arrete_sur_le_budget_sans_attendre_le_tour_suivant() {
        let g = Garde::nouvelle(Reglages { budget: Duration::from_millis(1), ..Reglages::default() });
        let v = g.veille();
        std::thread::sleep(Duration::from_millis(4));
        assert!(!v.continuer(), "le budget épuisé se voit AVANT la lecture suivante");
    }

    #[test]
    fn une_nouvelle_page_oublie_la_prose_mais_pas_l_horloge() {
        let mut g = Garde::nouvelle(Reglages { budget: Duration::from_millis(1), ..Reglages::default() });
        assert!(g.prose(&prose_longue("premier tour")).is_ok());
        assert!(!g.prose_vue().is_empty());
        std::thread::sleep(Duration::from_millis(4));
        g.nouvelle_page();
        assert!(g.prose_vue().is_empty(), "la prose d'un tour ne déborde pas sur le suivant");
        assert!(g.tour().is_err(), "l'horloge, elle, ne repart pas à zéro");
    }

    #[test]
    fn la_garde_ne_conclut_rien_sur_un_texte_court() {
        // Prudence par construction : une réponse brève qui se répète (« oui…
        // oui… oui ») est légitime, et la couper serait pire que le défaut.
        let mut g = Garde::nouvelle(Reglages::default());
        assert!(g.prose("oui oui oui oui oui oui oui oui oui oui").is_ok());
        assert!(g.prose("oui\noui\noui\n").is_ok());
        assert!(!g.arrete());
    }
}
