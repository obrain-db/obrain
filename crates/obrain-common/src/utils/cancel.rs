//! Annulation coopérative des requêtes.
//!
//! # Le problème
//!
//! `Session::execute` est synchrone. Un appelant qui pose un `timeout` autour
//! ne libère que LUI-MÊME : la requête continue de brûler son cœur jusqu'à son
//! terme, en tenant son permis de concurrence. Mesuré en production le
//! 2026-08-13 sur obrain-tmn : « 5, puis 6, puis 3 exemplaires du MÊME tri en
//! vol, avec vif à 33 Go contre 9 de ligne de base », et 700-800 % de CPU en
//! permanence.
//!
//! # Le mécanisme
//!
//! Un drapeau atomique partagé ([`CancelToken`]) plus des points de
//! vérification ([`check`]) dans les boucles chaudes. Le même point de
//! contrôle sert AUSSI l'échéance de la requête ([`installer_echeance`]) :
//! un seul mécanisme, deux raisons de s'arrêter ([`Raison`]), parce que deux
//! dispositifs concurrents pour la même question finissent par diverger. L'appelant arme le
//! drapeau depuis un AUTRE thread ; la requête le constate à son prochain
//! point de contrôle et remonte [`Cancelled`] comme un `Err` ordinaire, en
//! déroulant sa pile normalement — donc en relâchant ses gardes RAII
//! (verrous, permis) au passage.
//!
//! # Pourquoi un thread-local et pas un paramètre
//!
//! Le trait `Operator` expose `fn next(&mut self)` sans contexte, et le trait
//! `GraphStore` compte une centaine de méthodes. Faire descendre un jeton par
//! signature imposerait de toucher ~34 opérateurs, toutes leurs
//! implémentations, tous leurs sites de construction dans le planificateur, et
//! toute la surface du substrat — pour un diff illisible et une revue
//! impossible.
//!
//! Le jeton voyage donc dans un thread-local, installé par une garde RAII au
//! point d'entrée ([`scope`]). C'est exactement le `CHECK_FOR_INTERRUPTS()` de
//! PostgreSQL : un drapeau que le moteur consulte, pas un signal qui
//! l'interrompt. Une requête s'exécutant sur un thread donné voit le jeton de
//! CE thread ; il n'y a pas d'ambiguïté possible tant qu'une requête = un
//! thread.
//!
//! # La frontière, et pourquoi elle n'est posée qu'une fois
//!
//! Une lecture interrompue ne laisse rien derrière elle ; une écriture
//! interrompue en vol dépend, pour être sûre, de tout le chemin de retour
//! arrière. Le mécanisme refuse donc l'annulation aux plans qui mutent.
//!
//! Cette décision est prise à UN SEUL endroit — [`frontiere`], appelée
//! depuis `Session::with_auto_commit`, que les douze points d'entrée du
//! moteur traversent tous — et la polarité est inversée pour que l'oubli
//! soit bénin : un jeton confié par [`scope`] est INERTE jusqu'à ce que la
//! frontière le mette en vigueur. Un chemin d'exécution qui ne la traverse
//! pas n'est pas annulable ; il n'est jamais annulé à tort. Voir le pavé de
//! `Portee::en_vigueur`.
//!
//! # La limite, dite franchement
//!
//! Un thread-local ne franchit PAS une frontière de thread. Tout travail
//! délégué à un pool (rayon) s'exécute hors de portée du jeton, à moins que le
//! site de délégation ne le réinstalle explicitement — voir [`propagate`].
//! Un point de vérification qui manque est un endroit où l'annulation attend ;
//! il ne peut pas produire de faux positif.
//!
//! ## Inventaire des délégations rayon, site par site (2026-08-24, v2)
//!
//! La v1 de cet inventaire ouvrait sur « LE FAIT QUI DOMINE : aucune de
//! ces délégations n'est atteignable depuis une requête ». C'était FAUX, et
//! la revue l'a réfuté par exécution. Cette v2 nomme chaque site, dit qui
//! l'appelle, et se vérifie par `grep`.
//!
//! ATTEINT PAR UNE REQUÊTE, ET C'EST LE SEUL :
//!
//!   · `SubstrateStore::incoming_heads` — `into_par_iter` O(E) sur toute
//!     la zone Edges, appelé par `walk_incoming_chain`, donc par
//!     `edges_from(Incoming | Both)` et `in_degree`, donc par TOUS les
//!     opérateurs d'expansion (`expand`, `variable_length_expand`,
//!     `shortest_path`, `factorized_expand`). `rayon` est une dépendance
//!     INCONDITIONNELLE d'`obrain-substrate`. La v1 le classait parmi les
//!     écritures : erreur de lecture de ma part, il est sur le chemin de
//!     LECTURE le plus emprunté du moteur.
//!
//!   · Traitement : un point de contrôle est posé AVANT l'appel, dans
//!     `walk_incoming_chain`. Une requête condamnée ne déclenche plus la
//!     construction, et l'expansion elle-même devient interruptible ligne
//!     par ligne. La construction, une fois lancée, va à son terme —
//!     délibérément, parce que son résultat est un cache partagé : le
//!     raisonnement complet est sur place.
//!
//! CÂBLÉ, mais sans appelant de production à ce jour :
//!
//!   · `tiered_scan::l0_top_k_parallel` — une portée par worker, plus un
//!     contrôle en tête de shard.
//!
//! LECTURES SANS APPELANT DE PRODUCTION — `grep` ne trouve que des
//! tests. Non câblées, et la raison est qu'il n'y a rien à câbler tant
//! qu'aucun chemin n'y mène :
//!
//!   · `SubstrateStore::vector_top_k` — parallélisme par NŒUD ; le câbler
//!     par lot suppose de regrouper la boucle, donc de ré-indenter le
//!     noyau dont le contrat de parité B2/M1/M2 vient d'être revu.
//!   · `hnsw::batch_search` et ses quatre variantes (`_with_ef`,
//!     `_with_filter`, `_with_ef_and_filter`), `quantized_hnsw::batch_search`
//!     — une tâche rayon par requête du lot, derrière `vector-index` +
//!     `parallel`. La v1 décrivait `quantized_hnsw` comme une
//!     « construction d'index » : c'est un `batch_search`, donc une
//!     lecture. Correction.
//!
//! ÉCRITURES. La frontière les met hors vigueur par construction :
//! [`current`] y rend `None` et [`propagate`] y serait un `no-op`.
//! Interrompre une écriture longue demanderait d'abord de démontrer son
//! retour arrière — un autre travail, qui commence par là.
//!
//!   · `SubstrateStore::community_ranges` — relevé des plages de
//!     communauté (cache, même forme que `incoming_heads`).
//!   · `geometric::refresh_all_ricci` — recalcule et RÉÉCRIT les
//!     enregistrements d'arête.
//!   · `geometric::heat_step_csr_generic` — pas de production non plus.
//!
//!
//! # Le coût
//!
//! [`check`] = une lecture de thread-local (initialisée en `const`, donc sans
//! test de première-utilisation) puis, si un jeton est installé, un
//! `load(Relaxed)` sur un `AtomicBool`. Aucune barrière mémoire, aucune
//! contention : `Relaxed` n'ordonne rien et un `AtomicBool` en lecture seule
//! reste en état partagé dans les caches. Les appelants amortissent par un
//! compteur ([`Vigie`]) pour ne payer cela qu'une itération sur N.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Le jeton d'annulation : un drapeau partagé, armé une fois, jamais désarmé.
///
/// Cloner un jeton partage le MÊME drapeau (c'est un `Arc`). Armer depuis
/// n'importe quel clone annule tous les autres.
///
/// ```
/// # use obrain_common::utils::cancel::CancelToken;
/// let jeton = CancelToken::new();
/// let copie = jeton.clone();
/// assert!(!copie.is_cancelled());
/// jeton.cancel();
/// assert!(copie.is_cancelled());
/// ```
#[derive(Clone, Debug, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
}

impl CancelToken {
    /// Crée un jeton non armé.
    #[must_use]
    pub fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Arme le jeton. Idempotent, appelable depuis n'importe quel thread.
    ///
    /// `Release` en écriture (contre `Relaxed` en lecture) : le côté qui arme
    /// est toujours un thread tiers — un chien de garde, un délai qui expire —
    /// et il n'a aucune donnée à publier avec le drapeau. La seule garantie
    /// utile est que l'écriture finisse par être visible, ce que tout
    /// ordonnancement atomique fournit.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Release);
    }

    /// Le jeton est-il armé ?
    #[inline]
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }
}

/// L'erreur d'annulation, à la racine.
///
/// Volontairement sans donnée : elle traverse des couches (`obrain-substrate`,
/// `obrain-core`, `obrain-engine`) qui ont chacune leur type d'erreur, et
/// chaque conversion doit rester un `From` trivial. Le CONTEXTE (quelle
/// requête, quel opérateur) appartient à l'appelant qui a armé le jeton : lui
/// seul le connaît sans avoir à le transporter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "query cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// Pourquoi un point de contrôle a dit d'arrêter.
///
/// La distinction n'est pas cosmétique : un délai dépassé est une
/// constatation du moteur, une annulation est une décision de l'appelant.
/// Les compter ensemble rendrait faux le seul chiffre qui dit si le
/// mécanisme d'annulation sert à quelque chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Raison {
    /// Un jeton a été armé — quelqu'un a DEMANDÉ l'arrêt.
    Annulation,
    /// L'échéance de la requête est passée.
    Echeance,
}

/// Ce qu'une requête en cours a le droit de savoir sur sa propre fin.
///
/// ── UN SEUL MÉCANISME, DEUX RAISONS (2026-08-24) ─────────────────────
///
/// Le moteur avait DÉJÀ une échéance : `SessionConfig::query_timeout` →
/// `Session::query_deadline()` → `Executor::deadline` → `check_deadline()`.
/// Complète, testée… et jamais armée en production (`query_timeout` vaut
/// `None` par défaut et le pool d'obrain-tmn ne l'a jamais renseignée), donc
/// `check_deadline` y est un no-op permanent depuis toujours.
///
/// Elle souffre en outre du défaut qui a motivé tout ce travail : elle n'est
/// consultée qu'ENTRE deux `Operator::next()`. Un scan de label matérialise
/// son `Vec` entier dans UN seul `next()` — l'échéance ne s'y exerce jamais.
///
/// Il aurait été facile d'ajouter à côté un second dispositif d'annulation.
/// Deux mécanismes pour la même question finissent toujours par diverger, et
/// c'est le genre de divergence qu'on ne découvre que le jour où l'un des
/// deux devait protéger. La portée porte donc les DEUX raisons, et les
/// points de contrôle des boucles chaudes — les seuls qui atteignent le
/// substrat — les servent l'une comme l'autre. `Executor::deadline` reste où
/// il est, en filet extérieur, alimenté par la MÊME configuration.
///
/// COÛT : le jeton est lu d'abord (`load(Relaxed)`, ~1 ns) ; l'horloge n'est
/// consultée que si aucun jeton n'est armé ET qu'une échéance existe. Une
/// [`Vigie`] n'appelle tout cela qu'une itération sur 1024.
#[derive(Clone, Debug, Default)]
struct Portee {
    jeton: Option<CancelToken>,
    echeance: Option<web_time::Instant>,
    /// La portée est-elle EN VIGUEUR ?
    ///
    /// ── LA POLARITÉ EST INVERSÉE, ET C'EST LE CŒUR DU DISPOSITIF ──
    ///
    /// Un jeton confié par [`scope`] est INERTE tant que la [`frontiere`]
    /// ne l'a pas mis en vigueur. Tout ce qui n'a pas traversé la frontière
    /// — un chemin d'exécution ajouté demain, un `return` anticipé, une
    /// commande de session, un DDL — s'exécute donc SANS annulation, c'est
    /// à dire exactement comme avant que ce mécanisme n'existe.
    ///
    /// L'inverse (« armé par défaut, à désarmer explicitement là où il ne
    /// faut pas ») a été essayé : il a produit SEPT oublis, dont quatre
    /// n'ont été trouvés qu'en revue adversariale, et il en garantissait un
    /// huitième au prochain point d'entrée. Un mécanisme de sûreté dont le
    /// mode par défaut est « dangereux » n'est pas un mécanisme de sûreté :
    /// c'est une liste de choses à ne pas oublier.
    ///
    /// Le prix de l'inversion est un oubli SYMÉTRIQUE mais bénin : une
    /// entrée qui ne traverse pas la frontière n'est pas annulable. C'est
    /// une occasion manquée, jamais une écriture fausse.
    en_vigueur: bool,
}

impl Portee {
    #[inline]
    fn raison(&self) -> Option<Raison> {
        if !self.en_vigueur {
            return None;
        }
        if self.jeton.as_ref().is_some_and(CancelToken::is_cancelled) {
            return Some(Raison::Annulation);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(t) = self.echeance
            && web_time::Instant::now() >= t
        {
            return Some(Raison::Echeance);
        }
        None
    }
}

thread_local! {
    /// La portée du thread courant, s'il y en a une.
    ///
    /// `const { }` : sans initialiseur paresseux, l'accès se compile en un
    /// simple accès relatif au pointeur de thread — pas de test « déjà
    /// initialisé ? » à chaque lecture. C'est ce qui rend [`check`] assez bon
    /// marché pour être placé dans une boucle.
    static COURANT: RefCell<Option<Portee>> = const { RefCell::new(None) };
}

/// Garde RAII : désinstalle le jeton (et restaure le précédent) à la sortie.
///
/// Restaurer le PRÉCÉDENT et non `None` : une procédure stockée, un sous-plan,
/// un `CALL` imbriqué peuvent rouvrir une portée. Remettre `None` désarmerait
/// silencieusement la requête englobante — le pire des bugs pour ce mécanisme,
/// puisqu'il ne se manifesterait QUE le jour où l'annulation doit servir.
pub struct PorteeAnnulation {
    precedent: Option<Portee>,
}

impl Drop for PorteeAnnulation {
    fn drop(&mut self) {
        let precedent = self.precedent.take();
        COURANT.with(|c| {
            *c.borrow_mut() = precedent;
        });
    }
}

/// CONFIE `token` au thread courant pour la durée de `f`, **en attente**.
///
/// Le jeton est présent mais SANS EFFET : [`check`] rend `Ok` tant que la
/// [`frontiere`] ne l'a pas mis en vigueur. C'est la porte d'entrée du
/// moteur (`Session::execute_annulable`) et le mode par défaut voulu — voir
/// le pavé de `Portee::en_vigueur` pour la raison.
///
/// La désinstallation passe par une garde RAII : elle a donc lieu aussi bien
/// sur un retour normal que sur un `?` que sur un déroulement de pile. Un
/// jeton oublié dans le thread-local d'un thread de pool (`spawn_blocking` les
/// recycle) annulerait la requête SUIVANTE servie par ce thread.
///
/// ```
/// # use obrain_common::utils::cancel::{self, CancelToken};
/// let jeton = CancelToken::new();
/// jeton.cancel();
/// cancel::scope(&jeton, || {
///     assert!(cancel::check().is_ok(), "confié, pas encore en vigueur");
///     let _f = cancel::frontiere(Some(false), None); // plan de LECTURE
///     assert!(cancel::check().is_err(), "la frontière l'a mis en vigueur");
/// });
/// ```
pub fn scope<R>(token: &CancelToken, f: impl FnOnce() -> R) -> R {
    installer_pour(
        Portee {
            jeton: Some(token.clone()),
            echeance: None,
            en_vigueur: false,
        },
        f,
    )
}

/// Met `token` EN VIGUEUR immédiatement, sans passer par la frontière.
///
/// Primitive de bas niveau. Deux usages légitimes, et deux seulement :
///
///   · [`propagate`], qui réinstalle dans un thread de travail un jeton
///     déjà mis en vigueur par la frontière dans le thread appelant ;
///   · les tests qui éprouvent un point de contrôle isolément, sans
///     moteur autour pour poser la frontière.
///
/// Un appel depuis un chemin d'exécution du moteur serait un contournement
/// de la frontière : c'est précisément ce que l'inversion de polarité
/// interdit.
pub fn scope_en_vigueur<R>(token: &CancelToken, f: impl FnOnce() -> R) -> R {
    installer_pour(
        Portee {
            jeton: Some(token.clone()),
            echeance: None,
            en_vigueur: true,
        },
        f,
    )
}

fn installer_pour<R>(portee: Portee, f: impl FnOnce() -> R) -> R {
    let _garde = installer(portee);
    f()
}

#[must_use]
fn installer(portee: Portee) -> PorteeAnnulation {
    let precedent = COURANT.with(|c| c.borrow_mut().replace(portee));
    PorteeAnnulation { precedent }
}

/// Le jeton du thread courant, EN VIGUEUR ou non.
fn jeton_confie() -> Option<CancelToken> {
    COURANT.with(|c| c.borrow().as_ref().and_then(|p| p.jeton.clone()))
}

/// LA FRONTIÈRE ÉCRITURE / LECTURE. Le seul endroit du moteur qui met un
/// jeton en vigueur.
///
/// `mute` est la réponse STRUCTURELLE `plan.root.has_mutations()`, jamais
/// une devinette sur le texte de la requête. D'où le `Option` : un point
/// d'entrée qui n'a pas encore de plan dit `None` — « je ne sais pas » — au
/// lieu de deviner.
///
///   · `Some(true)`  → la portée est retirée. Une écriture s'exécute
///     exactement comme avant ce mécanisme, au bit près, et rien de plus
///     profond ne peut la remettre en vigueur.
///   · `Some(false)` → le jeton confié par [`scope`] entre en vigueur, et
///     l'échéance de la requête le rejoint dans la MÊME portée.
///   · `None`        → hors vigueur, mais le jeton et l'échéance sont
///     transportés jusqu'à la frontière intérieure qui aura le plan
///     ([`frontiere_heritee`]). C'est le cas des quatre entrées
///     paramétrées, qui ne disposent que du TEXTE de la requête.
///
/// Rend une garde : l'état précédent est restauré à sa destruction, y
/// compris au déroulement de pile. Les frontières s'imbriquent donc
/// correctement (`CALL` imbriqué, sous-plan, procédure stockée), et une
/// frontière INTÉRIEURE fait autorité sur une frontière extérieure — c'est
/// ce qui permet au `QueryProcessor`, qui connaît le plan, de corriger la
/// décision prise en amont par la session, qui ne connaît que le texte.
///
/// # Pourquoi la frontière est là, et pas dans les points de contrôle
///
/// Une lecture annulée ne laisse RIEN derrière elle — pas de WAL, pas
/// d'index reconstruit à moitié, pas de compteur décrémenté sans son
/// incrément. Une écriture abandonnée en vol dépend, pour être sûre, du bon
/// fonctionnement de tout le chemin de retour arrière. C'est probablement
/// vrai (`with_auto_commit` défait sur `Err`), mais ce n'est pas DÉMONTRÉ,
/// et un mécanisme de sûreté qui repose sur un « probablement » n'est pas un
/// mécanisme de sûreté.
#[must_use]
pub fn frontiere(mute: Option<bool>, echeance: Option<web_time::Instant>) -> PorteeAnnulation {
    match mute {
        Some(true) => suspend(),
        Some(false) => installer_echeance(echeance),
        // INDÉCIS. Le point d'entrée n'a pas de plan : il ne SAIT pas. La
        // portée reste hors vigueur — donc sûre — mais elle transporte le
        // jeton et l'échéance jusqu'à la frontière intérieure qui, elle,
        // aura le plan. Ne rien installer du tout perdrait l'échéance de
        // session ; mettre en vigueur « au cas où » serait exactement la
        // faute qu'on répare.
        None => installer(Portee {
            jeton: jeton_confie(),
            echeance,
            en_vigueur: false,
        }),
    }
}

/// [`frontiere`] qui CONSERVE l'échéance déjà posée par la frontière
/// extérieure au lieu d'en fournir une.
///
/// Sert au `QueryProcessor` : il découvre `has_mutations()` après avoir
/// optimisé, donc après que la session a déjà tranché sur le seul indice
/// dont elle disposait (le texte de la requête). Il fait autorité — c'est
/// lui qui a le plan — mais il n'a pas la configuration de session d'où
/// vient l'échéance.
#[must_use]
pub fn frontiere_heritee(mute: bool) -> PorteeAnnulation {
    let echeance = COURANT.with(|c| c.borrow().as_ref().and_then(|p| p.echeance));
    frontiere(Some(mute), echeance)
}

/// Ajoute une ÉCHÉANCE à la portée courante SANS toucher au jeton déjà posé.
///
/// C'est l'appelant qui pose le jeton (il seul sait quand abandonner) et le
/// moteur qui pose l'échéance (lui seul connaît `query_timeout`, et seulement
/// après avoir traduit la requête). Les deux doivent coexister : sans cette
/// composition, installer l'échéance écraserait le jeton de l'appelant et
/// l'annulation explicite cesserait de fonctionner dès qu'un délai serait
/// configuré — une régression qui ne se verrait qu'en production.
///
/// Rend une garde : l'échéance est retirée à sa destruction, y compris au
/// déroulement de pile.
#[must_use]
pub fn installer_echeance(echeance: Option<web_time::Instant>) -> PorteeAnnulation {
    let nouvelle = Portee {
        jeton: jeton_confie(),
        echeance,
        // MISE EN VIGUEUR : c'est ici, et nulle part ailleurs, qu'un jeton
        // confié par `scope` devient effectif. Passer par `frontiere` plutôt
        // que d'appeler ceci directement depuis le moteur.
        en_vigueur: true,
    };
    let precedent = COURANT.with(|c| c.borrow_mut().replace(nouvelle));
    PorteeAnnulation { precedent }
}

/// Suspend l'annulation pour la durée de vie de la garde rendue.
///
/// ── POURQUOI CELA EXISTE (2026-08-24) ────────────────────────────────
///
/// Le jeton est installé par l'APPELANT, qui ne sait pas si sa requête lit
/// ou écrit. C'est le moteur qui le sait — après traduction et
/// optimisation, `plan.root.has_mutations()`. Il doit donc pouvoir REFUSER
/// l'annulation pour un plan qui écrit, sans que l'appelant ait à
/// s'interroger.
///
/// La frontière est là et pas ailleurs parce qu'une lecture annulée ne
/// laisse RIEN derrière elle — pas de WAL, pas d'index reconstruit à
/// moitié, pas de compteur décrémenté sans son incrément — alors qu'une
/// écriture abandonnée en vol dépend, pour être sûre, du bon
/// fonctionnement de tout le chemin de retour arrière. C'est peut-être vrai
/// (`with_auto_commit` défait sur `Err`), mais ce n'est pas DÉMONTRÉ, et un
/// mécanisme de sûreté qui repose sur un « probablement » n'est pas un
/// mécanisme de sûreté.
#[must_use]
pub fn suspend() -> PorteeAnnulation {
    // La portée entière est retirée, jeton compris : la suspension est
    // DÉFINITIVE pour la durée de la garde. Rien à l'intérieur ne peut la
    // révoquer, pas même une frontière plus profonde — c'est ce qui fait la
    // différence entre « on n'a pas encore tranché » (voir
    // `frontiere(None, _)`) et « on a tranché : ça écrit ».
    let precedent = COURANT.with(|c| c.borrow_mut().take());
    PorteeAnnulation { precedent }
}

/// Le jeton EN VIGUEUR du thread courant, cloné. `None` si la frontière ne
/// l'a pas mis en vigueur — donc `None` sur tout chemin d'ÉCRITURE.
///
/// Sert aux sites qui délèguent à un autre thread : on capture ici, on
/// réinstalle là-bas avec [`propagate`]. Le filtre est ce qui rend cette
/// paire sûre à câbler sans réfléchir : un site de délégation partagé entre
/// un chemin de lecture et une reconstruction d'index capture `None` du
/// côté reconstruction, et la propagation y est un `no-op`.
#[must_use]
pub fn current() -> Option<CancelToken> {
    COURANT.with(|c| {
        c.borrow()
            .as_ref()
            .filter(|p| p.en_vigueur)
            .and_then(|p| p.jeton.clone())
    })
}

/// Pourquoi le dernier [`check`] a échoué — à consulter APRÈS lui.
///
/// Le signal qui remonte des boucles chaudes est volontairement opaque
/// ([`Cancelled`], sans donnée) pour qu'il traverse trois crates sans
/// alourdir aucune signature. La RAISON se relit ici, une fois, à la
/// frontière du moteur, là où elle sert à choisir l'erreur rendue.
#[must_use]
pub fn raison() -> Option<Raison> {
    COURANT.with(|c| c.borrow().as_ref().and_then(Portee::raison))
}

/// Réinstalle un jeton capturé par [`current`] dans le thread courant.
///
/// À utiliser à l'entrée d'une closure rayon / d'un thread de travail :
/// ```ignore
/// let jeton = cancel::current();
/// items.par_iter().for_each(|x| {
///     cancel::propagate(jeton.as_ref(), || travail(x))
/// });
/// ```
pub fn propagate<R>(token: Option<&CancelToken>, f: impl FnOnce() -> R) -> R {
    match token {
        // EN VIGUEUR d'emblée : `current()` ne rend un jeton que si la
        // frontière l'avait déjà mis en vigueur dans le thread appelant. Le
        // thread de travail hérite donc d'un état déjà tranché, il n'a pas à
        // re-poser une frontière qu'il n'a pas les moyens d'évaluer.
        Some(t) => scope_en_vigueur(t, f),
        None => f(),
    }
}

/// Le point de vérification. `Ok(())` si rien n'est armé.
///
/// # Errors
///
/// [`Cancelled`] si le jeton du thread courant est armé.
#[inline]
pub fn check() -> Result<(), Cancelled> {
    if is_cancelled() {
        Err(Cancelled)
    } else {
        Ok(())
    }
}

/// Variante sans `Result`, pour les boucles qui ne peuvent que `break`.
#[inline]
#[must_use]
pub fn is_cancelled() -> bool {
    COURANT.with(|c| c.borrow().as_ref().is_some_and(|p| p.raison().is_some()))
}

/// Intervalle par défaut entre deux vérifications, en itérations.
///
/// 1024 : calibré, pas deviné. Voir `benches`/le rapport de mission —
/// à 1024, le surcoût d'une boucle de balayage est sous le bruit de mesure,
/// et le retard d'annulation reste très inférieur à la milliseconde pour du
/// travail par élément de l'ordre de la centaine de nanosecondes.
pub const INTERVALLE_VERIF: usize = 1024;

/// Compteur qui n'appelle [`check`] qu'une fois sur `INTERVALLE_VERIF`.
///
/// Le compteur est un `usize` de pile, sans atomique ni synchronisation :
/// dans le cas nominal, une boucle équipée d'une `Vigie` paie un incrément et
/// un `&`-masque. C'est ce qui rend le « coût nul quand personne n'annule »
/// vérifiable plutôt que promis.
///
/// ```
/// # use obrain_common::utils::cancel::{Vigie, Cancelled};
/// # fn exemple(items: &[u32]) -> Result<u32, Cancelled> {
/// let mut vigie = Vigie::new();
/// let mut total = 0;
/// for x in items {
///     vigie.tick()?;
///     total += x;
/// }
/// Ok(total)
/// # }
/// ```
#[derive(Debug)]
pub struct Vigie {
    compteur: usize,
    masque: usize,
}

impl Default for Vigie {
    fn default() -> Self {
        Self::new()
    }
}

impl Vigie {
    /// Vigie à l'intervalle par défaut ([`INTERVALLE_VERIF`]).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            compteur: 0,
            masque: INTERVALLE_VERIF - 1,
        }
    }

    /// Vigie à intervalle choisi. `intervalle` est arrondi à la puissance de
    /// deux immédiatement supérieure pour que le test reste un masque.
    ///
    /// Utile quand le travail par itération est LOURD (une expansion qui lit
    /// des propriétés sur disque) : vérifier toutes les 1024 itérations
    /// donnerait un délai de réaction en secondes.
    #[must_use]
    pub fn avec_intervalle(intervalle: usize) -> Self {
        let n = intervalle.max(1).next_power_of_two();
        Self {
            compteur: 0,
            masque: n - 1,
        }
    }

    /// Une itération de plus. Vérifie une fois sur N.
    ///
    /// # Errors
    ///
    /// [`Cancelled`] si le jeton du thread est armé au moment du contrôle.
    #[inline]
    pub fn tick(&mut self) -> Result<(), Cancelled> {
        self.compteur += 1;
        if self.compteur & self.masque == 0 {
            check()
        } else {
            Ok(())
        }
    }

    /// `tick` sans `Result`, pour les boucles qui ne peuvent que `break`.
    #[inline]
    pub fn tick_bool(&mut self) -> bool {
        self.compteur += 1;
        self.compteur & self.masque == 0 && is_cancelled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jeton_neuf_non_arme() {
        assert!(!CancelToken::new().is_cancelled());
    }

    #[test]
    fn clone_partage_le_drapeau() {
        let a = CancelToken::new();
        let b = a.clone();
        a.cancel();
        assert!(b.is_cancelled(), "le clone doit voir l'armement");
    }

    #[test]
    fn hors_portee_check_est_ok() {
        assert!(
            check().is_ok(),
            "sans jeton installé, aucun coût, aucune erreur"
        );
    }

    #[test]
    fn portee_installe_et_desinstalle() {
        let t = CancelToken::new();
        t.cancel();
        scope_en_vigueur(&t, || {
            assert_eq!(check(), Err(Cancelled));
        });
        assert!(
            check().is_ok(),
            "le jeton doit être désinstallé à la sortie de la portée"
        );
    }

    #[test]
    fn portee_desinstalle_meme_sur_panique() {
        let t = CancelToken::new();
        t.cancel();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            scope_en_vigueur(&t, || panic!("boum"));
        }));
        assert!(r.is_err());
        assert!(
            check().is_ok(),
            "une panique ne doit pas laisser le jeton collé au thread"
        );
    }

    #[test]
    fn portee_imbriquee_restaure_la_precedente() {
        let externe = CancelToken::new();
        externe.cancel();
        let interne = CancelToken::new(); // NON armé
        scope_en_vigueur(&externe, || {
            assert_eq!(check(), Err(Cancelled));
            scope_en_vigueur(&interne, || {
                assert!(check().is_ok(), "la portée interne masque l'externe");
            });
            assert_eq!(
                check(),
                Err(Cancelled),
                "à la sortie de l'interne, l'externe doit être restaurée"
            );
        });
    }

    #[test]
    fn le_jeton_ne_traverse_pas_les_threads_sans_propagate() {
        let t = CancelToken::new();
        t.cancel();
        scope_en_vigueur(&t, || {
            let vu = std::thread::spawn(|| check().is_err()).join().unwrap();
            assert!(
                !vu,
                "limite documentée : un thread-local ne franchit pas une frontière de thread"
            );
        });
    }

    #[test]
    fn propagate_traverse_les_threads() {
        let t = CancelToken::new();
        t.cancel();
        scope_en_vigueur(&t, || {
            let capture = current();
            let vu = std::thread::spawn(move || propagate(capture.as_ref(), || check().is_err()))
                .join()
                .unwrap();
            assert!(
                vu,
                "propagate doit réinstaller le jeton dans le thread de travail"
            );
        });
    }

    #[test]
    fn vigie_ne_verifie_qu_une_fois_sur_n() {
        let t = CancelToken::new();
        t.cancel();
        scope_en_vigueur(&t, || {
            let mut v = Vigie::avec_intervalle(8);
            // Les 7 premiers ticks ne regardent rien.
            for i in 1..8 {
                assert!(v.tick().is_ok(), "tick {i} ne devait pas vérifier");
            }
            assert_eq!(v.tick(), Err(Cancelled), "le 8e tick doit vérifier");
        });
    }

    #[test]
    fn vigie_arrondit_a_la_puissance_de_deux() {
        // 100 → 128 : le 127e tick passe, le 128e vérifie.
        let t = CancelToken::new();
        t.cancel();
        scope_en_vigueur(&t, || {
            let mut v = Vigie::avec_intervalle(100);
            for _ in 0..127 {
                assert!(v.tick().is_ok());
            }
            assert_eq!(v.tick(), Err(Cancelled));
        });
    }

    #[test]
    fn echeance_passee_declenche_le_meme_point_de_controle() {
        let _g = installer_echeance(Some(
            web_time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(1))
                .unwrap(),
        ));
        assert_eq!(check(), Err(Cancelled));
        assert_eq!(raison(), Some(Raison::Echeance));
    }

    #[test]
    fn echeance_future_ne_declenche_pas() {
        let _g = installer_echeance(Some(
            web_time::Instant::now() + std::time::Duration::from_secs(3600),
        ));
        assert!(check().is_ok());
        assert_eq!(raison(), None);
    }

    #[test]
    fn echeance_sans_echeance_ne_declenche_pas() {
        let _g = installer_echeance(None);
        assert!(check().is_ok());
    }

    #[test]
    fn installer_echeance_conserve_le_jeton_de_l_appelant() {
        // LA régression que ce test empêche : si `installer_echeance`
        // remplaçait la portée au lieu de la composer, l'annulation
        // explicite cesserait de fonctionner dès qu'un délai serait
        // configuré — et seulement à ce moment-là.
        let t = CancelToken::new();
        scope_en_vigueur(&t, || {
            let _g = installer_echeance(Some(
                web_time::Instant::now() + std::time::Duration::from_secs(3600),
            ));
            assert!(check().is_ok());
            t.cancel();
            assert_eq!(check(), Err(Cancelled));
            assert_eq!(raison(), Some(Raison::Annulation));
        });
    }

    #[test]
    fn le_jeton_prime_sur_l_echeance_dans_la_raison() {
        let t = CancelToken::new();
        t.cancel();
        scope_en_vigueur(&t, || {
            let _g = installer_echeance(Some(
                web_time::Instant::now()
                    .checked_sub(std::time::Duration::from_secs(1))
                    .unwrap(),
            ));
            // Les deux sont vraies ; la DEMANDE explicite l'emporte, parce
            // qu'elle est la seule information que l'appelant ne peut pas
            // déduire lui-même.
            assert_eq!(raison(), Some(Raison::Annulation));
        });
    }

    #[test]
    fn suspend_neutralise_aussi_l_echeance() {
        let _g = installer_echeance(Some(
            web_time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(1))
                .unwrap(),
        ));
        assert_eq!(check(), Err(Cancelled));
        {
            let _s = suspend();
            assert!(check().is_ok(), "une ecriture ne doit pas etre interrompue");
        }
        assert_eq!(check(), Err(Cancelled), "restauree a la sortie");
    }

    // ── L'INVERSION DE POLARITÉ ──────────────────────────────────────
    //
    // Ces tests figent la propriété qui remplace sept suspensions posées à
    // la main : ce n'est pas « on a pensé à désarmer partout », c'est « rien
    // n'est armé tant que la frontière n'a pas tranché ».

    #[test]
    fn un_jeton_confie_est_inerte_avant_la_frontiere() {
        let t = CancelToken::new();
        t.cancel();
        scope(&t, || {
            assert!(
                check().is_ok(),
                "LA propriété : un chemin qui ne traverse pas la \
                 frontière s'exécute comme avant que ce mécanisme n'existe"
            );
            assert!(current().is_none(), "rien à propager hors vigueur");
        });
    }

    #[test]
    fn la_frontiere_met_en_vigueur_pour_une_lecture() {
        let t = CancelToken::new();
        t.cancel();
        scope(&t, || {
            let _f = frontiere(Some(false), None);
            assert_eq!(check(), Err(Cancelled));
            assert_eq!(raison(), Some(Raison::Annulation));
            assert!(current().is_some(), "propageable une fois en vigueur");
        });
    }

    #[test]
    fn la_frontiere_laisse_une_ecriture_hors_vigueur() {
        let t = CancelToken::new();
        t.cancel();
        scope(&t, || {
            let _f = frontiere(Some(true), None);
            assert!(check().is_ok(), "une écriture n'est jamais interrompue");
            assert!(
                current().is_none(),
                "et ne propage rien à un thread de travail"
            );
        });
    }

    #[test]
    fn la_frontiere_restaure_l_etat_precedent() {
        let t = CancelToken::new();
        t.cancel();
        scope(&t, || {
            {
                let _f = frontiere(Some(false), None);
                assert_eq!(check(), Err(Cancelled));
            }
            assert!(check().is_ok(), "retour à l'état confié, pas en vigueur");
        });
    }

    #[test]
    fn l_indecis_transporte_le_jeton_sans_le_mettre_en_vigueur() {
        // Le cas des quatre entrées PARAMÉTRÉES : la session n'a que le
        // texte. Elle dit `None`, et c'est le `QueryProcessor` — qui a le
        // plan — qui tranche. Entre les deux, rien n'est annulable.
        let t = CancelToken::new();
        t.cancel();
        scope(&t, || {
            let _ext = frontiere(None, None);
            assert!(
                check().is_ok(),
                "tant que personne n'a le plan, personne n'annule"
            );
            {
                let _lecture = frontiere_heritee(false);
                assert_eq!(check(), Err(Cancelled), "le plan dit : lecture");
            }
            let _ecriture = frontiere_heritee(true);
            assert!(check().is_ok(), "le plan dit : écriture");
        });
    }

    #[test]
    fn l_indecis_transporte_aussi_l_echeance_de_session() {
        // Sans cela, `query_timeout` se perdrait sur les quatre entrées
        // paramétrées — le chemin le plus emprunté.
        let t = CancelToken::new();
        scope(&t, || {
            let _ext = frontiere(
                None,
                Some(
                    web_time::Instant::now()
                        .checked_sub(std::time::Duration::from_secs(1))
                        .unwrap(),
                ),
            );
            assert!(check().is_ok(), "hors vigueur : l'échéance non plus");
            let _int = frontiere_heritee(false);
            assert_eq!(raison(), Some(Raison::Echeance));
        });
    }

    #[test]
    fn une_ecriture_tranchee_ne_peut_pas_etre_revoquee_plus_bas() {
        // `Some(true)` retire la portée : aucune frontière plus profonde ne
        // peut ressusciter le jeton. C'est ce qui rend l'anti-vacuité du
        // test de bout en bout honnête, et ce qui distingue « ça écrit » de
        // « je ne sais pas ».
        let t = CancelToken::new();
        t.cancel();
        scope_en_vigueur(&t, || {
            let _ext = frontiere(Some(true), None);
            assert!(check().is_ok());
            let _int = frontiere_heritee(false);
            assert!(
                check().is_ok(),
                "une écriture tranchée reste hors de portée de l'annulation"
            );
        });
    }
    #[test]
    fn frontiere_heritee_conserve_l_echeance_de_l_amont() {
        let t = CancelToken::new();
        scope(&t, || {
            let _ext = frontiere(
                Some(false),
                Some(
                    web_time::Instant::now()
                        .checked_sub(std::time::Duration::from_secs(1))
                        .unwrap(),
                ),
            );
            assert_eq!(raison(), Some(Raison::Echeance));
            let _int = frontiere_heritee(false);
            assert_eq!(
                raison(),
                Some(Raison::Echeance),
                "l'échéance de session ne doit pas se perdre en \
                 traversant la frontière du processeur"
            );
        });
    }

    #[test]
    fn hors_de_toute_portee_la_frontiere_ne_cree_rien() {
        let _f = frontiere(Some(false), None);
        assert!(
            check().is_ok(),
            "aucun jeton confié : rien à mettre en vigueur"
        );
    }

    #[test]
    fn vigie_hors_portee_ne_declenche_jamais() {
        let mut v = Vigie::avec_intervalle(2);
        for _ in 0..10_000 {
            assert!(v.tick().is_ok());
        }
    }
}
