//! Compteurs d'instrumentation du substrat.
//!
//! POURQUOI. Cette machine a annonce -39,3 % puis +63,7 % avec p = 0,00 sur
//! du code IDENTIQUE (`BANCS.md`), sous une charge de 15 a 26 pour 16 coeurs.
//! Un chronometre n'y prouve rien. Le NOMBRE DE SLOTS LUS, lui, est
//! comptable et deterministe : il ne depend ni de la charge ni du cache.
//! C'est la mesure que ce module rend disponible.

use std::sync::atomic::{AtomicU64, Ordering};

/// Slots d'enregistrements-noeuds REELLEMENT lus depuis la zone mmap par les
/// balayages (`node_ids`, `node_ids_capped`, `nodes_by_label_capped`, ...).
///
/// Alimente UNE FOIS PAR BALAYAGE, pas par slot : aucun cout atomique sur le
/// chemin chaud.
static SLOTS_NOEUDS_LUS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// La MEME quantite, mais attribuee au fil qui a balaye.
    ///
    /// ── POURQUOI UN SECOND COMPTEUR (2026-09-16) ─────────────────────────
    /// Le compteur global compte les slots de TOUT le processus. Or les tests
    /// Rust tournent en parallele : `VERROU_COMPTAGE` serialisait bien les
    /// tests qui MESURENT, mais pas les ~240 autres du meme binaire, qui
    /// balaient leurs propres bases au meme moment. Mesure :
    /// `expansion_entrante_ne_balaie_pas_la_zone` lisait « 4 slots » au lieu
    /// de 0 — des slots lus par un voisin — des que la limite de descripteurs
    /// laissait enfin tous les tests tourner ensemble.
    ///
    /// Le balayage (`for_each_node_record_range`) s'execute sur le fil
    /// appelant : l'attribuer a ce fil est exact. Et si un jour une lecture
    /// partait sur un autre fil, les TEMOINS des tests de mesure (« un
    /// balayage doit lire > 0 slot ») tomberaient a zero et echoueraient
    /// bruyamment — cette attribution ne peut donc pas masquer un balayage.
    static SLOTS_NOEUDS_LUS_FIL: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Verse `n` slots au compteur global ET au compteur du fil courant.
/// Appele a la sortie de chaque balayage.
pub(crate) fn ajouter_slots_noeuds_lus(n: u64) {
    if n != 0 {
        SLOTS_NOEUDS_LUS.fetch_add(n, Ordering::Relaxed);
        SLOTS_NOEUDS_LUS_FIL.with(|c| c.set(c.get() + n));
    }
}

/// Lecture du compteur global de slots-noeuds lus.
#[must_use]
pub fn slots_noeuds_lus() -> u64 {
    SLOTS_NOEUDS_LUS.load(Ordering::Relaxed)
}

/// Remet le compteur a zero et rend son ancienne valeur.
pub fn raz_slots_noeuds_lus() -> u64 {
    SLOTS_NOEUDS_LUS.swap(0, Ordering::Relaxed)
}

/// Verrou de serialisation des mesures de slots. Le compteur etant global au
/// processus et les tests Rust tournant en parallele, toute mesure doit le
/// detenir, sans quoi elle compte aussi les slots des autres tests.
pub static VERROU_COMPTAGE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Execute `f` en comptant les slots-noeuds qu'elle fait lire.
///
/// Rend `(resultat, slots_lus)`. Serialise par [`VERROU_COMPTAGE`].
///
/// Compte les slots lus PAR CE FIL pendant `f` — pas ceux des autres tests qui
/// balaient au meme moment. Le compteur global n'est plus remis a zero : il
/// sert la telemetrie du processus (`obrain query`), qu'une mesure de test n'a
/// pas a effacer.
pub fn mesurer_slots<T, F: FnOnce() -> T>(f: F) -> (T, u64) {
    let _g = VERROU_COMPTAGE.lock().unwrap_or_else(|e| e.into_inner());
    let avant = SLOTS_NOEUDS_LUS_FIL.with(std::cell::Cell::get);
    let r = f();
    let apres = SLOTS_NOEUDS_LUS_FIL.with(std::cell::Cell::get);
    (r, apres - avant)
}

/// (T27) Temps CPU consommé (utilisateur + système, en secondes) par le
/// processus (`"self"`) ou le fil courant (`"thread-self"`). Sur une machine
/// partagée, c'est le TRAVAIL qu'on mesure — le temps mural dépend de la charge.
pub fn temps_cpu(qui: &str) -> f64 {
    std::fs::read_to_string(format!("/proc/{qui}/stat"))
        .ok()
        .and_then(|s| {
            let apres = s.rsplit_once(") ")?.1;
            let f: Vec<&str> = apres.split_whitespace().collect();
            let u: f64 = f.get(11)?.parse().ok()?;
            let st: f64 = f.get(12)?.parse().ok()?;
            Some((u + st) / 100.0)
        })
        .unwrap_or(0.0)
}

/// (T27) Temps du FIL COURANT, en nanosecondes : `(sur un cœur, en attente
/// d'un cœur)`, lus dans `/proc/thread-self/schedstat`. Sur une machine
/// partagée, le chronomètre mélange le travail et la contention ; ces deux
/// nombres les séparent. `(0, 0)` hors Linux.
#[must_use]
pub fn temps_fil() -> (u64, u64) {
    std::fs::read_to_string("/proc/thread-self/schedstat")
        .ok()
        .and_then(|t| {
            let mut it = t.split_whitespace().map(|x| x.parse::<u64>().ok());
            Some((it.next()??, it.next()??))
        })
        .unwrap_or((0, 0))
}
