//! `obrain viz serve` — sert soi-même la couche viz (arbitrage V4-G2a).
//!
//! Mini serveur HTTP **std-only** (zéro dépendance) :
//! - `GET /` → le viewer interactif (`docs/examples/viz/viewer.html`), branché
//!   sur l'endpoint ci-dessous ;
//! - `GET /api/viz/subgraph?store=&center=&hops=&limit=&overlay=` → json-graph
//!   v1 (même implémentation que le tool bridge `viz_subgraph` : parité par
//!   construction), **ouverture READ-ONLY** obligatoire (V4-B).
//!
//! Bind `127.0.0.1` par défaut (`--host` pour exposer). Stores exposés via
//! `--preset <label>=<chemin>` (répétable) ; `--preset-default <label>` choisit
//! le preset chargé par défaut (sinon `--store`). `GET /api/stores` liste les
//! presets pour alimenter la liste déroulante du viewer.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
// Le transport a besoin de l'horloge (délais d'inactivité, `Date`, `ETag`), de
// l'état partagé du battement de cœur SSE, et de `Cow` pour ne pas recopier un
// corps de 1,29 Mo quand il n'y a rien à dégonfler.
use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::viz_extract::{ExtractOptions, extract_subgraph};

/// ── OÙ PASSENT LES MILLISECONDES D'UN APPEL HTTP ? (2026-09-14) ────────────
///
/// Le dashboard sert des requêtes que le moteur rend en ~1 µs et l'appel coûte
/// encore des centaines de microsecondes : c'est donc la SURFACE qui domine, et
/// rien dans le code ne disait laquelle de ses dizaines d'écritures payait. Ce
/// compteur, armé par `OBRAIN_SURFACE_PHASES=1`, ventile UN appel en postes
/// exclusifs — un poste est chronométré là où il est payé, jamais deux fois.
///
/// Il est INERTE quand la variable est absente : `actif()` est un `var_os`
/// mémoïsé, et les postes ne sont pas chronométrés du tout (`chrono` rend la
/// main sans horloge). Une ventilation qu'on paierait quand on ne la demande pas
/// fausserait la mesure qu'elle sert.
mod vent {
    use std::cell::Cell;
    use std::time::Instant;

    /// Les postes, dans l'ordre où un appel les paie. `PUB` : ils sont nommés
    /// par les appelants (`vent::chrono(vent::MTIME, …)`) — un index nu serait
    /// illisible au point de mesure.
    pub const LECTURE: usize = 0; // accept + lecture de la requête sur la socket
    pub const ROUTE: usize = 1; // routage complet (somme des postes ci-dessous)
    pub const PARAMS: usize = 2; // analyse de la chaîne de requête + résolution du store
    pub const MTIME: usize = 3; // date du store (fraîcheur des caches)
    pub const GENERATION: usize = 4; // empreinte du binaire et des réglages
    pub const CLE: usize = 5; // construction de la clé de cache (allocations)
    pub const QCACHE: usize = 6; // verrou + recherche dans le cache de requêtes
    pub const SANS_MESURE: usize = 7; // re-sérialisation du corps servi par le cache
    pub const MOTEUR: usize = 8; // ouverture RO + exécution + sérialisation JSON
    pub const ECRITURE: usize = 9; // en-têtes + corps + flush
    pub const ACTIVATION: usize = 10; // enregistrement d'activation (disque)
    pub const POSTES: usize = 11;

    thread_local! {
        static ACTIF: bool = std::env::var_os("OBRAIN_SURFACE_PHASES").is_some();
        static ACC: Cell<[f64; POSTES]> = const { Cell::new([0.0; POSTES]) };
        static OCTETS: Cell<usize> = const { Cell::new(0) };
    }

    /// Taille du corps écrit — renseignée par `respond_octets`, lue par la ligne
    /// de ventilation (c'est la seule chose que le poste `ecriture` ne dit pas).
    pub fn note_octets(n: usize) {
        if actif() {
            OCTETS.with(|o| o.set(n));
        }
    }

    #[inline]
    pub fn actif() -> bool {
        ACTIF.with(|a| *a)
    }

    /// Chronomètre `f` dans le poste `poste`. Sans `OBRAIN_SURFACE_PHASES`, la
    /// fermeture est appelée sans lire l'horloge : coût nul.
    #[inline]
    pub fn chrono<T>(poste: usize, f: impl FnOnce() -> T) -> T {
        if !actif() {
            return f();
        }
        let d = Instant::now();
        let r = f();
        let us = d.elapsed().as_secs_f64() * 1e6;
        ACC.with(|a| {
            let mut v = a.get();
            v[poste] += us;
            a.set(v);
        });
        r
    }

    /// Remet les compteurs à zéro — à appeler au DÉBUT de chaque appel, sinon la
    /// ligne imprimée décrit la connexion entière et plus l'appel.
    pub fn remise_a_zero() {
        if actif() {
            ACC.with(|a| a.set([0.0; POSTES]));
        }
    }

    /// Imprime la ventilation d'UN appel et remet les compteurs à zéro.
    pub fn debit(chemin: &str) {
        if !actif() {
            return;
        }
        ACC.with(|a| {
            let v = a.get();
            a.set([0.0; POSTES]);
            let octets = OCTETS.with(|o| o.replace(0));
            let postes = v
                .iter()
                .enumerate()
                .map(|(i, us)| format!("{}={us:.0}", NOM[i]))
                .collect::<Vec<_>>()
                .join(" ");
            eprintln!("[surface] {chemin} octets={octets} total={:.0}us {postes}", v[ROUTE]);
        });
    }

    const NOM: [&str; POSTES] = [
        "lecture", "route", "params", "mtime", "generation", "cle", "qcache", "sans_mesure",
        "moteur", "ecriture", "activation",
    ];
}

/// LA CARTE DES HANDLES RO, par store — **une seule**, partagée.
///
/// Défaut que j'ai introduit et corrigé dans la même heure : `cached_ro_si_present`
/// déclarait sa PROPRE carte statique, distincte de celle de `cached_ro`. Elle
/// était donc toujours vide : la lecture « sans ouvrir » ne trouvait jamais le
/// handle que le préchauffage venait de poser, et le chat répondait « base non
/// ouverte » alors que la base ÉTAIT ouverte (vérifié : présente dans les
/// descripteurs du processus). Deux cartes pour un même registre, c'est le même
/// défaut que deux règles de slug pour un même monde — sauf qu'ici il rendait le
/// chat muet.
/// L'ENTRÉE : le handle, et QUAND il a servi pour la dernière fois (LRU).
pub(crate) type EntreeRo = (std::sync::Arc<obrain_engine::ObrainDB>, std::time::Instant);

/// LA carte des bases ouvertes — **une seule**, partagée par l'ouverture ET la
/// lecture.
///
/// Défaut mesuré le 2026-09-12, et c'est le même que les deux règles de slug :
/// `cached_ro` déclarait sa **propre** carte statique locale (avec LRU et
/// horodatage), tandis que la lecture « sans ouvrir » interrogeait une **autre**
/// carte, au niveau du module, que personne ne remplissait jamais. Résultat : la
/// lecture ne trouvait RIEN, toujours, et **quatre fonctionnalités se rabattaient
/// en silence** sur leur repli — l'ancrage du chat, l'ouvreur de mondes, le
/// handle synaptique de `obrain_traverser`, et la sonde de lisibilité de l'atlas.
/// Deux registres pour une même chose.
static HANDLES_RO: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, EntreeRo>>,
> = std::sync::OnceLock::new();

pub(crate) fn carte_handles_ro(
) -> &'static std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, EntreeRo>> {
    HANDLES_RO.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Le handle RO d'un store **s'il est DÉJÀ ouvert** — sans jamais en ouvrir un.
///
/// Pourquoi cette fonction existe (défaut mesuré le 2026-09-12) : ouvrir un store
/// en lecture alors qu'un AUTRE processus le tient en écriture ne renvoie pas
/// toujours une erreur — l'ouverture peut BLOQUER indéfiniment. Relevé des
/// threads du serveur pendant une requête : cinq threads figés sur un `futex`,
/// pendant que `/api/query` répondait en 0,9 ms. Le chat, `/api/viz/weave` et
/// `/api/engine/memoire` étaient morts ; `/api/query` vivait. La différence était
/// la base synaptique, ouverte par les premiers et tenue en exclusif ailleurs.
///
/// Un lecteur ne doit pas mourir parce qu'un écrivain travaille. Les chemins qui
/// ne sont pas indispensables (l'ancrage du sujet, l'atlas, le self) se
/// contentent donc d'un handle DÉJÀ présent, et se rabattent sur leurs replis
/// sinon — tous en ont un, et ils sont documentés.
pub(crate) fn cached_ro_si_present(
    path: &std::path::Path,
) -> Option<std::sync::Arc<obrain_engine::ObrainDB>> {
    let map = carte_handles_ro();
    let g = map.lock().ok()?;
    if let Some((db, _)) = g.get(path) {
        return Some(db.clone());
    }
    // Le MÊME store peut être désigné par deux écritures différentes (chemin
    // relatif, lien symbolique, `..`) : la carte est indexée sur le chemin BRUT,
    // on retente donc sur la forme canonique avant de conclure à l'absence.
    let canon = std::fs::canonicalize(path).ok()?;
    g.iter()
        .find(|(k, _)| std::fs::canonicalize(k).ok().as_deref() == Some(canon.as_path()))
        .map(|(_, (db, _))| db.clone())
}

/// Ouvre la base synaptique en tâche de fond, au démarrage.
///
/// L'ouverture peut bloquer (voir `cached_ro_si_present`) : la faire dans un fil
/// dédié garantit qu'aucune requête n'attend après elle. Quand elle aboutit, les
/// chemins qui s'en servent (atlas, self) la trouvent déjà ouverte ; sinon ils
/// gardent leurs replis, et le service reste vivant.
pub(crate) fn prechauffer_synapse(config: &ServeConfig) {
    let Some(p) = config.synapse_store.clone() else { return };
    std::thread::spawn(move || {
        match cached_ro(&p) {
            Ok(_) => tracing::info!(store = %p.display(), "base synaptique ouverte (tâche de fond)"),
            Err(e) => tracing::warn!(
                store = %p.display(), erreur = %e,
                "base synaptique indisponible — l'atlas et le self se rabattront sur leurs replis"
            ),
        }
    });
}

/// Ouvre le store en RO avec un HANDLE RÉUTILISÉ par store (mmap/plan chaud) :
/// la 1re requête paie le chargement, les suivantes ne re-planifient plus à froid.
///
/// `pub(crate)` : l'ancrage (`crate::ancrage`) lit la base synaptique par ce même
/// handle — un second chemin d'ouverture paierait un chargement froid de plus et
/// désynchroniserait les caches d'agrégats.
/// ── L'ÉCHÉANCE DES REQUÊTES, ARMÉE À L'OUVERTURE DE CHAQUE BASE SERVIE ──────
///
/// Défaut mesuré le 2026-09-12. Le moteur a une échéance de requête complète et
/// testée (`SessionConfig::query_timeout` → `Executor::check_deadline`), et le
/// substrat sait rendre une traversée interruptible (`utils::cancel`) — mais
/// elle n'était armée **nulle part** : `None` partout en production, donc
/// `check_deadline` était un no-op permanent. Le code le dit lui-même
/// (`common/utils/cancel.rs` : « Complète, testée… et jamais armée en
/// production »), et `ObrainDB::with_query_timeout` existe précisément pour
/// l'armer depuis un appelant comme celui-ci.
///
/// Conséquence observée sur ce serveur : `MATCH (a)-[*1..2]->(b) WHERE
/// id(b) IN [315447] RETURN count(a)` — 23 696 arêtes entrantes sur la cible —
/// tournait **encore après 150 s**, sans aucune borne. L'explosion des liens en
/// longueur variable n'a pas d'autre garde (le planificateur refuse les produits
/// cartésiens prouvés, mais le nombre de CHEMINS, lui, ne se borne pas depuis les
/// compteurs), et une requête qui ne finit pas occupe un fil pour toujours.
///
/// Rail de sécurité réglé LARGE : les routes les plus lourdes du serveur se
/// mesurent en secondes (4,1 s à froid pour `/api/viz/arcs`). Le réglage se fait
/// par `OBRAIN_QUERY_TIMEOUT_MS` (`0` désarme), sans recompilation. La limite
/// fine, par requête, reste possible via un jeton d'annulation.
fn echeance_requete() -> Option<std::time::Duration> {
    let ms = std::env::var("OBRAIN_QUERY_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(30_000);
    (ms > 0).then(|| std::time::Duration::from_millis(ms))
}

pub(crate) fn cached_ro(path: &std::path::Path) -> Result<std::sync::Arc<obrain_engine::ObrainDB>> {
    use std::sync::{Arc, Mutex, OnceLock};
    if std::env::var("OBRAIN_NO_KEEP_RO").is_ok() {
        return Ok(Arc::new(
            obrain_engine::ObrainDB::open_read_only(path)?.with_query_timeout(echeance_requete()),
        ));
    }
    // ── LE CACHE DES BASES OUVERTES : BORNÉ, ET PAR USAGE (2026-09-11) ──────
    //
    // Ce cache n'évincait RIEN à l'origine : mesuré sur le serveur en marche,
    // 308 descripteurs ouverts et 13 bases tenues en mémoire, dont 5 mondes
    // ajoutés à chaud que plus personne n'interrogeait. C'était un désagrément
    // tant que l'ouverture était un geste humain ; c'est devenu un levier pour
    // le MODÈLE, puisque la boucle d'outils ouvre un monde par appel
    // (`db_du_monde` → `cached_ro`).
    //
    // Première borne posée : 6 entrées, évincées par le plus petit
    // `Arc::strong_count`. Deux défauts, mesurés au raisonnement puis à la
    // mesure d'ouverture :
    //
    //   · **6 < 7 presets.** Toucher les sept fait donc évincer à coup sûr une
    //     base encore utile — et une éviction n'est pas gratuite : mesuré,
    //     l'ouverture froide de `linux-full3` (1,55 M de nœuds) coûte
    //     **1 236 ms** contre 1,5 ms ensuite. Alterner entre deux stores pouvait
    //     donc infliger une seconde de latence par requête.
    //   · **`strong_count` n'est pas la RÉCENCE.** Toutes les entrées au repos
    //     ont un compte de 1 (la carte est leur seul détenteur) : la « plus
    //     petite » était donc la première dans l'ordre d'itération, c'est-à-dire
    //     un choix ARBITRAIRE qui pouvait très bien tomber sur celle qu'on
    //     venait d'utiliser.
    //
    // On garde donc l'horodatage du dernier usage, on évince le plus ANCIEN, et
    // la borne dépasse largement le nombre de mondes connus (presets + ajoutés à
    // chaud) : la borne existe pour empêcher une croissance sans fin, pas pour
    // arbitrer entre des stores qu'on utilise vraiment.
    const MAX_BASES_GARDEES: usize = 24;

    let map = carte_handles_ro();
    // ── UN SEUL VERROU, SINON LE SERVEUR ENTIER S'ARRÊTE (2026-09-12) ───────
    //
    // Défaut mesuré, puis reproduit hors du projet par un programme minimal :
    // `if let Some(x) = verrou(&map).get(..)` garde le `MutexGuard` du scrutinee
    // VIVANT pendant TOUT le corps du `if let`. Le `verrou(&map)` écrit deux
    // lignes plus bas, sur le MÊME mutex, s'auto-bloquait donc définitivement —
    // en TENANT le verrou, qui n'était plus jamais rendu.
    //
    // Conséquence observée sur le serveur en marche : le premier appel qui
    // trouvait une base DÉJÀ en cache figeait toutes les routes qui ouvrent une
    // base — `/api/info`, `/api/viz/signatures`, `/api/viz/weave`, `/api/worlds`
    // — pour TOUTES les bases, y compris celles que personne d'autre n'avait
    // ouvertes, sans un octet de réponse, sans erreur et sans CPU consommé (les
    // threads dorment sur le futex). Pendant ce temps `/api/viz/status` et
    // `/api/stores` répondaient en 1 ms : voilà pourquoi les vues Monde et
    // Cosmos du dashboard restaient vides alors que la page, elle, se chargeait.
    //
    // Contre-épreuve faite : avec `OBRAIN_NO_KEEP_RO=1` (ce cache contourné), le
    // même binaire ouvre la même base en 22 ms — l'ouverture n'a jamais été en
    // cause. On prend donc le verrou UNE fois, on marque l'usage et on rend le
    // handle dans le même souffle.
    {
        let mut g = verrou(&map);
        if let Some(e) = g.get_mut(path) {
            // Marquer l'usage : sans cela, la base qu'on vient de servir serait
            // évincée en premier — exactement le défaut qu'on corrige.
            e.1 = std::time::Instant::now();
            return Ok(e.0.clone());
        }
    }
    let db = Arc::new(
        obrain_engine::ObrainDB::open_read_only(path)?.with_query_timeout(echeance_requete()),
    );
    {
        let mut g = verrou(&map);
        if g.len() >= MAX_BASES_GARDEES && !g.contains_key(path) {
            let plus_ancien = g.iter().min_by_key(|(_, (_, t))| *t).map(|(k, _)| k.clone());
            if let Some(k) = plus_ancien {
                // `Arc` retiré ⇒ base fermée ⇒ descripteurs rendus (sauf si un
                // appel en cours en détient encore une copie : c'est voulu).
                g.remove(&k);
            }
        }
        g.insert(path.to_path_buf(), (db.clone(), std::time::Instant::now()));
    }
    Ok(db)
}

/// Verrou qui SURVIT à l'empoisonnement.
///
/// `lock().unwrap()` transforme un panic survenu SOUS verrou en panne
/// DÉFINITIVE : le mutex reste empoisonné, donc toutes les requêtes suivantes
/// paniquent à leur tour, et chaque client voit une connexion coupée sans un
/// octet de réponse — le symptôme le plus dur à diagnostiquer. Les données
/// gardées ici (cartes d'agrégats, cache de bases, résolutions) restent valides
/// après un panic : on reprend l'état intérieur au lieu de propager la panique.
pub(crate) fn verrou<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Empreinte d'immuabilité du store : les stores sont des snapshots en
/// lecture seule → la date de modification est stable. Les caches d'agrégats
/// deviennent DUrables (plus de TTL arbitraire) : une fois calculés, les
/// agrégats répondent instantanément jusqu'à ce que le store change réellement.
///
/// `pub(crate)` : l'engine du chat s'en sert pour valider sa propre mémoire de
/// signature (`crate::engine::signature_du_store`) — la même clé de fraîcheur,
/// donc la même garantie : une mesure ne survit jamais à un store modifié.
/// ── LA DATE DU STORE, GARDÉE EN MÉMOIRE LE TEMPS D'UN SOUFFLE ──────────────
///
/// `store_mtime` est appelé PLUSIEURS FOIS PAR APPEL, et chaque appel coûtait
/// deux à quatre `stat` sur disque (`exists()` puis `metadata()`) — sur un
/// chemin chaud qui ne fait rien d'autre. Mesuré sur `/api/query` servi par le
/// cache : c'est le premier poste de la surface après la lecture de la socket.
///
/// On la garde donc en mémoire pendant `OBRAIN_SURFACE_MTIME_TTL_MS`
/// (défaut **100 ms**). Ce qui est échangé, dit sans détour : une écriture
/// faite par un AUTRE processus dans le store peut rester invisible jusqu'à
/// 100 ms — c'est-à-dire qu'un corps de cache peut être resservi pendant au plus
/// 100 ms après une écriture. Aucune route n'écrit dans un store servi, et les
/// fenêtres d'invalidation que ce serveur connaît déjà (``disk_save``, caches
/// d'agrégats) valident sur la MÊME empreinte : elles héritent exactement de la
/// même borne. Mettre `OBRAIN_SURFACE_MTIME_TTL_MS=0` rend la lecture à chaque
/// appel, c'est-à-dire le comportement d'avant, sans recompiler.
pub(crate) fn store_mtime(path: &std::path::Path) -> Option<u128> {
    use std::sync::{Mutex, OnceLock};
    let ttl = std::env::var("OBRAIN_SURFACE_MTIME_TTL_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(100);
    if ttl == 0 {
        return lire_mtime(path);
    }
    static MEMO: OnceLock<Mutex<HashMap<PathBuf, (Instant, Option<u128>)>>> = OnceLock::new();
    let memo = MEMO.get_or_init(|| Mutex::new(HashMap::new()));
    let ttl = Duration::from_millis(ttl);
    {
        let g = verrou(memo);
        if let Some((quand, v)) = g.get(path) {
            if quand.elapsed() < ttl {
                return *v;
            }
        }
    }
    let v = lire_mtime(path);
    verrou(memo).insert(path.to_path_buf(), (Instant::now(), v));
    v
}

/// La lecture elle-même — le corps d'origine de `store_mtime`.
fn lire_mtime(path: &std::path::Path) -> Option<u128> {
    // ── LA FRAÎCHEUR SE LIT SUR LE MANIFESTE, PAS SUR LE DOSSIER ────────────
    //
    // Le DOSSIER d'un store change à la MOINDRE écriture : WAL, télémétrie,
    // enregistrement d'activation. Mesuré : `runtime-dev` avait son dossier
    // modifié à l'instant même et son `substrate.meta` vieux d'un jour. Comme
    // TOUS les caches d'agrégats se valident sur cette empreinte, ils étaient
    // donc invalidés en permanence — d'où, chez l'utilisateur, 20 s pour
    // `modules`, 8 à 14 s pour chacun des 8 appels `space`, et 39 s de
    // chargement À CHAQUE VISITE, sans que rien ne s'améliore jamais.
    //
    // `substrate.meta` est le MANIFESTE du store : il change quand les DONNÉES
    // changent — import, enrichissement — et pas quand on écrit une ligne de
    // journal. C'est exactement la sémantique voulue pour un cache d'agrégat :
    // « ce que j'ai calculé est-il encore vrai ? »
    let meta = path.join("substrate.meta");
    let cible = if meta.exists() { meta } else { path.to_path_buf() };
    std::fs::metadata(&cible)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
}

// ── persistance DISQUE des agrégats (modules / arcs / space) : les caches en
// mémoire s'évanouissent au redémarrage du serveur → on repaierait les scans
// froids (1-11 s) à chaque relance. Un fichier JSON par agrégat, validé par
// mtime du store : réponse ~0 ms même après restart. Répertoire : variable
// OBRAIN_VIZ_CACHE_DIR, sinon /tmp/obrain-viz-cache.
use std::hash::{Hash, Hasher};
fn disk_path(kind: &str, store: &std::path::Path, extra: &str) -> std::path::PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    store.to_string_lossy().hash(&mut h);
    kind.hash(&mut h);
    extra.hash(&mut h);
    dir_cache().join(format!("{kind}-{:016x}.json", h.finish()))
}

/// ── LE CACHE NE VIT PLUS DANS `/tmp` ────────────────────────────────────────
///
/// `/tmp` se vide : au redemarrage, au nettoyage systeme. Or ce cache est
/// exactement ce qui evite de repayer les parcours froids — mesures 3,3 a 8,1 s
/// par agregat sur `linux-full3`. Chaque purge de `/tmp` reinfligeait donc a
/// l'utilisateur le chargement complet, sans qu'il ait rien change.
///
/// Ordre de priorite, du plus explicite au plus general :
/// 1. `OBRAIN_VIZ_CACHE_DIR` — le choix de l'operateur prime toujours ;
/// 2. `XDG_CACHE_HOME/obrain-viz` — la convention du systeme ;
/// 3. `~/.cache/obrain-viz` — son defaut ;
/// 4. `/tmp/obrain-viz-cache` — dernier recours, si l'on n'a meme pas de `HOME`.
fn dir_cache() -> std::path::PathBuf {
    use std::sync::OnceLock;
    static DIR: OnceLock<std::path::PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        if let Some(d) = std::env::var_os("OBRAIN_VIZ_CACHE_DIR") {
            return std::path::PathBuf::from(d);
        }
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")));
        let Some(base) = base else {
            return std::path::PathBuf::from("/tmp/obrain-viz-cache");
        };
        let neuf = base.join("obrain-viz");
        let _ = std::fs::create_dir_all(&neuf);
        migrer_depuis_tmp(&neuf);
        neuf
    })
    .clone()
}

/// REPRISE DE L'ANCIEN EMPLACEMENT, sans le casser.
///
/// On COPIE (on ne deplace pas) les agregats encore presents dans
/// `/tmp/obrain-viz-cache` vers le nouveau repertoire, et seulement ceux qui
/// n'y sont pas deja. Copier plutot que deplacer : un serveur d'une version
/// anterieure peut tourner en parallele et lire encore l'ancien chemin — le lui
/// retirer sous les pieds le ferait repartir a froid. `/tmp` se videra de
/// lui-meme ; c'est precisement ce dont on sort.
fn migrer_depuis_tmp(neuf: &std::path::Path) {
    let ancien = std::path::Path::new("/tmp/obrain-viz-cache");
    if !ancien.is_dir() || ancien == neuf {
        return;
    }
    let Ok(entrees) = std::fs::read_dir(ancien) else { return };
    let mut repris = 0usize;
    for e in entrees.flatten() {
        let chemin = e.path();
        if !chemin.is_file() || chemin.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Some(nom) = chemin.file_name() else { continue };
        let cible = neuf.join(nom);
        if cible.exists() {
            continue;
        }
        if std::fs::copy(&chemin, &cible).is_ok() {
            repris += 1;
        }
    }
    if repris > 0 {
        eprintln!("viz serve : cache — {repris} agregats repris depuis /tmp/obrain-viz-cache");
    }
}
/// MÉMOIRE DU RANG RETENU, par store.
///
/// Prouver qu'un rang NE croise PAS coûte un parcours complet (mesuré : ~2 s par
/// rang sur 618 776 arêtes) — et sur le store obrain, il faut descendre jusqu'au
/// rang 4, donc payer deux parcours inutiles (rangs 2 et 3) à CHAQUE recalcul à
/// froid. Or le rang est une propriété de la STRUCTURE du store, pas de la requête
/// (ni `top` ni `limit` ne le changent) : on le mémorise donc par store, et on
/// repart de là. Si le monde a changé entre-temps, l'agrégat ne rend rien et le
/// repli reprend la recherche complète — l'erreur ne coûte qu'un tour.
fn rang_memo(store: &std::path::Path) -> Option<usize> {
    let text = std::fs::read_to_string(disk_path("arcsrank", store, "")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("rank").and_then(|r| r.as_u64()).map(|r| r as usize)
}
fn rang_ecrire(store: &std::path::Path, rank: usize) {
    let p = disk_path("arcsrank", store, "");
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, serde_json::json!({ "rank": rank }).to_string());
}

pub(crate) fn disk_load(kind: &str, store: &std::path::Path, extra: &str, mt: Option<u128>) -> Option<String> {
    let text = std::fs::read_to_string(disk_path(kind, store, extra)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let m = v.get("m").and_then(|x| x.as_u64());
    if m != mt.map(|m| m as u64) { return None; }
    v.get("b").and_then(|x| x.as_str()).map(String::from)
}
pub(crate) fn disk_save(kind: &str, store: &std::path::Path, extra: &str, mt: Option<u128>, body: &str) {
    let p = disk_path(kind, store, extra);
    if let Some(d) = p.parent() { let _ = std::fs::create_dir_all(d); }
    let j = serde_json::json!({ "m": mt.map(|m| m as u64), "b": body });
    let _ = std::fs::write(p, j.to_string());
}

/// ── ÉLAGAGE DU CACHE DISQUE (2026-09-13) ──────────────────────────────────
///
/// Défaut mesuré : `/tmp/obrain-viz-cache` contenait **1 821 fichiers pour
/// 36 Mo** et RIEN ne les élaguait jamais. La cause est structurelle :
/// `disk_path` hache `(store, kind, extra)` et `disk_save` écrit par
/// `(clé, mtime)` — donc chaque état d'un store produit une GÉNÉRATION
/// d'entrées, et l'ancienne ne disparaît pas (elle est simplement refusée à la
/// lecture, par comparaison de mtime). Le répertoire ne peut que croître.
///
/// On garde les `CACHE_DISQUE_MAX_ENTREES` entrées les plus RÉCEMMENT ÉCRITES :
/// le mtime du fichier est l'horodatage de la dernière UTILISATION, puisque
/// `disk_save` le réécrit à chaque calcul. Une entrée plus vieille que
/// `CACHE_DISQUE_MAX_AGE_JOURS` est jetée d'abord : elle décrit un store qu'on
/// n'ouvre plus.
///
/// Ce qu'on perd est exactement ce que le cache est : du calcul refaisable.
/// Aucune réponse ne dépend de la présence d'un fichier ici.
const CACHE_DISQUE_MAX_ENTREES: usize = 400;
const CACHE_DISQUE_MAX_AGE_JOURS: u64 = 30;

fn elaguer_cache_disque() {
    let dir = std::env::var_os("OBRAIN_VIZ_CACHE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp/obrain-viz-cache"));
    let Ok(lecture) = std::fs::read_dir(&dir) else {
        return; // pas de cache : rien à élaguer (et ce n'est pas une erreur)
    };
    let maintenant = std::time::SystemTime::now();
    let age_max = std::time::Duration::from_secs(CACHE_DISQUE_MAX_AGE_JOURS * 86_400);
    let mut gardees: Vec<(std::path::PathBuf, std::time::SystemTime)> = Vec::new();
    let mut total = 0usize;
    let mut jetes = 0usize;
    for e in lecture.flatten() {
        let Ok(md) = e.metadata() else { continue };
        if !md.is_file() {
            continue; // on ne touche qu'aux fichiers du cache
        }
        total += 1;
        let t = md.modified().unwrap_or(std::time::UNIX_EPOCH);
        let trop_vieux = maintenant
            .duration_since(t)
            .map(|d| d > age_max)
            .unwrap_or(false);
        if trop_vieux {
            if std::fs::remove_file(e.path()).is_ok() {
                jetes += 1;
            }
        } else {
            gardees.push((e.path(), t));
        }
    }
    if gardees.len() > CACHE_DISQUE_MAX_ENTREES {
        gardees.sort_by_key(|(_, t)| *t); // le plus ancien en tête
        let a_jeter = gardees.len() - CACHE_DISQUE_MAX_ENTREES;
        for (p, _) in gardees.drain(..a_jeter) {
            if std::fs::remove_file(&p).is_ok() {
                jetes += 1;
            }
        }
    }
    if jetes > 0 {
        tracing::warn!(
            entrees_avant = total,
            entrees_apres = gardees.len(),
            jetes,
            "cache disque : élagué"
        );
    }
}

/// Vue 2D historique (disponible sous /viewer2d — héritage, non défaut).
const VIEWER2D_HTML: &str = include_str!("../../../docs/examples/viz/viewer.html");
/// Fichiers statiques vendus (three.js r128 + OrbitControls) — servis depuis
/// le repo (voir `vendor_dir`).
const VENDOR_FILES: &[&str] = &["three.min.js", "OrbitControls.js"];
/// Racine du dashboard React (`packages/obrain-dashboard/dist`) servi sous /app.
/// ── LA PAGE NE DISPARAÎT PLUS PENDANT UNE RECONSTRUCTION ───────────────────
///
/// Défaut mesuré le 2026-09-13, signalé par l'utilisateur : « quand j'envoie un
/// message je prends : index absent, ma page recharge et je ne peux pas avoir de
/// réponse ». Cause exacte : `vite build` VIDE `dist/` au DÉBUT de chaque
/// reconstruction (25 s mesurées : 17:46:18 → 17:46:43 au journal), et le
/// veilleur de dev en déclenche une à chaque changement de `src/` — donc
/// régulièrement pendant qu'on travaille dans le dépôt. Pendant cette fenêtre,
/// toute requête sur `/` recevait « index absent », et le rechargement du
/// veilleur tombait pile dedans : page blanche, aucune réponse possible.
///
/// On garde donc la DERNIÈRE page servie et on la ressert quand le fichier
/// manque. Une page d'une seconde en retard vaut infiniment mieux qu'une page
/// absente — et c'est cohérent avec l'en-tête `no-cache` et l'ETag : dès que le
/// fichier revient, la nouvelle page reprend la main au premier rechargement.
/// On le DIT dans le journal au lieu de le taire : un service qui ressert une
/// page périmée sans le dire serait un mensonge de plus.
fn derniere_page() -> &'static std::sync::Mutex<Option<(String, Option<std::time::SystemTime>)>> {
    use std::sync::{Mutex, OnceLock};
    static DERNIERE: OnceLock<Mutex<Option<(String, Option<std::time::SystemTime>)>>> = OnceLock::new();
    DERNIERE.get_or_init(|| Mutex::new(None))
}

/// Le dossier de l'interface SERVIE.
///
/// ── UN CHEMIN DE COMPILATION N'EST PAS UNE ADRESSE DE SERVICE ──────────────
/// Défaut mesuré le 2026-09-16 : ce chemin était figé par `CARGO_MANIFEST_DIR`,
/// donc le binaire servait `dist/` DU WORKTREE OÙ IL AVAIT ÉTÉ COMPILÉ. Or ce
/// `dist` est un lien symbolique que n'importe quel travail en cours déplace :
/// à 21:55 il a pointé vers un dossier d'essai, effacé dans la foulée — et
/// `/app` a rendu 404 à tout le monde. L'interface de tous les comptes tenait
/// à un lien dans l'arbre de travail d'un développeur.
///
/// `OBRAIN_UI_DIR` rend donc l'adresse EXPLICITE : le service désigne le
/// magasin de versions (`ui/current`), qui est justement ce qu'un déploiement
/// bascule atomiquement. Sans la variable, on garde l'ancien comportement —
/// c'est ce qui fait tourner un `cargo run` depuis un dépôt.
fn app_dir() -> std::path::PathBuf {
    if let Ok(d) = std::env::var("OBRAIN_UI_DIR")
        && !d.trim().is_empty()
    {
        return std::path::PathBuf::from(d);
    }
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/obrain-dashboard/dist")
}
pub(crate) const VIZ_JSON: &str = "application/json";

/// Configuration des stores exposés par le serveur.
#[derive(Clone, Debug)]
pub struct ServeConfig {
    /// Store par défaut (flag `--store`, chemin direct).
    pub default_store: Option<PathBuf>,
    /// Presets nommés (`--preset label=chemin`).
    pub presets: Vec<(String, PathBuf)>,
    /// Label du preset par défaut (`--preset-default`).
    pub default_label: Option<String>,
    /// Base synaptique de l'agent (`--synapse-store`, produit par `viz weave`).
    pub synapse_store: Option<PathBuf>,
    /// Entretien : minutes entre deux passes de courbure de Ricci (0 = désactivé).
    ///
    /// La passe est faite par le serveur lui-même — voir [`spawn_enrich_ricci`].
    pub enrich_ricci_minutes: u64,
}

/// TOUS LES MONDES = les presets du démarrage + ceux ajoutés à chaud.
///
/// Un monde ajouté depuis l'interface n'est pas un preset de ligne de commande :
/// il doit pourtant apparaître PARTOUT où les presets apparaissent (signatures,
/// liste des stores, foyers, activation) — sinon le Cosmos ne le verrait pas.
fn tous_les_mondes(config: &ServeConfig) -> Vec<(String, std::path::PathBuf)> {
    let mut v = config.presets.clone();
    for m in crate::worlds::enregistres() {
        if !v.iter().any(|(_, p)| *p == m.store) {
            v.push((m.label, m.store));
        }
    }
    v
}

/// Les mondes que L'APPELANT a le droit de VOIR : ceux dont l'espace se lit
/// (même règle que la garde : nom de fichier sans `.obrain`), plus les mondes
/// qu'il a lui-même ajoutés. Sans IAM armé, tous. IAM armé sans identité :
/// aucun — on ne montre rien plutôt que tout.
///
/// C'est la réponse à une fuite MESURÉE : `weave` et `signatures` rendaient
/// les 12 mondes à un compte qui n'avait le droit d'en lire que 2. La garde de
/// route juge UN espace (celui du store visé, sinon le défaut) ; un handler
/// qui AGRÈGE plusieurs bases doit rejuger chacune — exactement comme
/// `/api/stores` le fait déjà.
fn mondes_visibles(
    entetes: &str,
    query: Option<&str>,
    config: &ServeConfig,
) -> Vec<(String, std::path::PathBuf)> {
    let tous = tous_les_mondes(config);
    let Some(g) = garde_interieure() else { return tous };
    let Some(jeton) = jeton_presente(entetes, query).filter(|j| !j.trim().is_empty()) else {
        return Vec::new();
    };
    let Some(s) = g.authentifier(Some(&jeton)).ok().flatten() else {
        return Vec::new();
    };
    let espace = |p: &std::path::Path| -> String {
        p.file_stem().map(|x| x.to_string_lossy().to_string()).unwrap_or_default()
    };
    let moi = format!("{}@{}", s.username(), compte_iam());
    tous.into_iter()
        .filter(|(_, path)| {
            g.controler_action(Some(&s), ACTION_LECTURE_HTTP, &espace(path)).is_ok()
                || crate::worlds::proprietaire_de(path).is_some_and(|p| p == moi)
        })
        .collect()
}

impl ServeConfig {
    /// Chemin du store par défaut effectif (preset par défaut, sinon --store).
    fn default_path(&self) -> Option<PathBuf> {
        if let Some(label) = &self.default_label
            && let Some((_, path)) = self.presets.iter().find(|(l, _)| l == label)
        {
            return Some(path.clone());
        }
        self.default_store.clone()
    }

    /// Résout le paramètre `store` : label de preset → chemin, sinon chemin brut.
    pub(crate) fn resolve(&self, param: Option<&str>) -> Option<PathBuf> {
        let Some(param) = param else {
            return self.default_path();
        };
        if let Some((_, path)) = self.presets.iter().find(|(l, _)| l == param) {
            return Some(path.clone());
        }
        if let Some(m) = crate::worlds::enregistres().into_iter().find(|m| m.label == param) {
            return Some(m.store);
        }
        Some(PathBuf::from(param))
    }
}

/// Démarre le serveur (boucle mono-thread — usage local / curl / browser).
/// ENTRETIEN PAR LE DÉTENTEUR — la courbure de Ricci, faite par le serveur lui-même.
///
/// Mesuré sur `substrate.meta` via fdinfo (2026-09-10) : le `viz serve` l'ouvre en
/// `O_RDWR` (flags 02100002), y compris à travers `ObrainDB::open_read_only` — la
/// « lecture seule » n'est telle qu'au niveau LOGIQUE, pas au niveau du verrou. Le
/// serveur détient donc l'exclusif sur toutes ses bases, et un `obrain enrich
/// --ricci` lancé à côté échoue en 0 s (« already open by another process »).
/// Il n'existe pas d'entretien possible depuis un autre processus : la seule
/// alternative serait une fenêtre de maintenance qui coupe le dashboard.
///
/// D'où cette passe : elle tourne DANS le serveur, sur le handle qu'il détient
/// déjà. Aucun conflit de verrou n'est possible par construction, aucune fenêtre
/// n'est nécessaire, et le dashboard ne s'arrête jamais.
///
/// Échec NON FATAL : une base que le substrat refuse d'écrire est journalisée et la
/// passe continue. L'entretien ne doit jamais pouvoir tuer le serveur ni une requête.
/// ── LE CATALOGUE DES PROPRIÉTÉS TEXTUELLES (2026-09-13) ────────────────────
///
/// Ce que la base déclare interrogeable par SOUS-CHAÎNE — c'est-à-dire ce dont
/// l'index de trigrammes peut être construit. Le préchauffage ne sondait que
/// `qualified_name` : les six autres propriétés payaient leur index dans la
/// requête de l'utilisateur (voir `prechauffer_les_bases`).
///
/// L'ORDRE EST UNE DÉCISION, et il est tiré du COÛT MESURÉ — pas d'une
/// intuition. Mesuré le 2026-09-13 sur `runtime-dev`, index supprimés avant la
/// passe, démarrage à froid (414 352 nœuds) :
///
/// | propriété | construction | index persisté |
/// |---|---|---|
/// | `qualified_name` | 2 620 ms | 26,1 Mo |
/// | `name`           |   941 ms |  5,4 Mo |
/// | `file_path`      |   923 ms |  3,7 Mo |
/// | `title`          |   407 ms |   64 Ko |
/// | `payload`        |   518 ms |  120 Ko |
/// | `extraction_text`|   741 ms |   816 o |
/// | **`text`**       | **13 845 ms** | **51,5 Mo** |
///
/// Total du monde principal : **20 013 ms**. `text` pèse donc à lui seul 69 % du
/// temps et 59 % du disque : il passe en DERNIER, pour être la première chose que
/// le budget coupe si la passe doit être abrégée — et non `file_path`, qui coûte
/// 15 fois moins et sert à chaque question de chemin.
const PROPS_TEXTUELLES: &[&str] = &[
    "qualified_name",
    "name",
    "file_path",
    "title",
    "payload",
    "extraction_text",
    "text",
];

/// Les propriétés dont le préchauffage est DIFFÉRÉ, et pourquoi.
///
/// Défaut mesuré, et c'est un défaut de PROTOCOLE autant que de code : avec `text`
/// préchauffé au démarrage, les sept `top` d'arcs mesurés juste après passaient de
/// **4,6 s à 19,0 s** pour le PREMIER appel — non parce que le code d'arcs avait
/// changé (il est plus rapide, six appels sur sept sont à 0,8 ms), mais parce que
/// la construction de l'index de `text` occupait la base et le CPU pendant ce
/// temps. Un préchauffage qui rend la première minute d'usage PIRES que le défaut
/// qu'il corrige n'est pas un progrès.
///
/// On le DÉFÈRE donc : les propriétés légères (6 168 ms en tout, mesuré) sont
/// préchauffées tout de suite, et `text` (13 845 ms) après une pause qui laisse
/// passer le premier moment d'usage. C'est la « tâche de fond étalée » que le
/// coût impose — et elle concerne UNIQUEMENT le monde par défaut : sur les autres
/// mondes, `text` reste paresseux (un index de 51 Mo par monde n'a pas de sens).
const PROPS_LOURDES: &[&str] = &["text"];

/// Pause avant le préchauffage des propriétés lourdes (secondes).
///
/// Réglable (`OBRAIN_PRECHO_PAUSE_LOURDES_S`) pour la MESURE : à 0, la phase 2
/// part tout de suite et la contention redevient observable — c'est ainsi qu'on
/// a chiffré le défaut qu'elle évite.
fn precho_pause_lourdes_s() -> u64 {
    std::env::var("OBRAIN_PRECHO_PAUSE_LOURDES_S")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(120)
}

/// Aiguille du préchauffage : sa VALEUR n'a aucune importance (c'est
/// l'EXÉCUTION qui compte — la garde adaptative du substrat construit l'index à
/// la deuxième demande d'une propriété). Elle est choisie improbable pour que le
/// verdict ne ramène rien : on veut payer la construction, pas transporter des
/// lignes.
const AIGUILLE_PRECHO: &str = "_obrain_prechauffage_trigrammes_";

/// Budget TOTAL du préchauffage des trigrammes, pour la PASSE ENTIÈRE (tous les
/// mondes), pas par monde.
///
/// Il existe parce qu'un préchauffage non borné est un démarrage non borné, et
/// le coût mesuré n'est pas théorique : **20 013 ms** pour le seul monde
/// principal, `text` compris. Un budget PAR MONDE laissait donc la passe dériver
/// à 9 × 30 s. Ici, le budget est GLOBAL et l'ordre des mondes est celui de
/// `tous_les_mondes` (le défaut d'abord) : c'est le monde qu'on interroge
/// vraiment qui est préchauffé en entier, les suivants prennent ce qui reste.
///
/// Ce qui est coupé est JOURNALISÉ (`restantes=…`) — un budget tu n'est pas un
/// budget honnête. Réglable par `OBRAIN_PRECHO_TRIGRAMMES_MS` (0 = pas de
/// préchauffage de trigrammes du tout).
fn budget_precho_trigrammes_ms() -> u64 {
    std::env::var("OBRAIN_PRECHO_TRIGRAMMES_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60_000)
}

/// Budget du préchauffage des agrégats d'`arcs` (`OBRAIN_PRECHO_ARCS_MS`).
/// `0` désarme complètement le préchauffage — utile pour mesurer le froid.
fn budget_precho_arcs_ms() -> u64 {
    std::env::var("OBRAIN_PRECHO_ARCS_MS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(30_000)
}

/// ── PRÉCHAUFFAGE DE L'AGRÉGAT D'`arcs` : LE PREMIER VISITEUR NE PAIE PLUS ──
///
/// Défaut mesuré le 2026-09-15, service 8432, `runtime-dev` (439 688 nœuds /
/// 1 324 105 arêtes), cache vidé :
///
/// ```text
///   première requête, store froid .................. 6 240 ms   ← le « >3 s » rapporté
///   la même, store déjà ouvert ..................... 2 050 ms
///   la même, réjouée ...............................     0,55 ms
/// ```
///
/// Le détail des 2 050 ms dit où est le levier : l'agrégat utile pèse ~700 ms,
/// et le reste est fait de SONDES — choix du rang (`group_at`, ~400 ms), preuve
/// d'existence du croisement (jusqu'à 6 parcours d'arêtes, ~800 ms), artefacts de
/// build (~200 ms). Or aucune de ces sondes n'a besoin d'un VISITEUR pour être
/// posée : elles ne décrivent qu'une structure IMMUABLE (les stores sont des
/// snapshots en lecture seule). Les payer au démarrage, en tâche de fond, ne
/// coûte rien à personne ; les payer dans une requête, c'est 2 à 6 secondes
/// devant un écran vide, au moment précis où l'utilisateur juge la réactivité.
///
/// ── CE QU'ON PRÉCHAUTFE, ET CE QU'ON NE PRÉCHAUTFE PAS ────────────────────
///
/// SEULEMENT la forme que le dashboard demande : palier **1 024** (`limit ≤ 1024`
/// — `CosmosWeave.tsx` demande `top=16, limit=60`) et rang **automatique**
/// (`rank` absent, donc `rang_cle = 0`, exactement la clé que la route calcule
/// pour un appel sans `rank`). Les autres paliers restent payés à la demande :
/// les préchauffer multiplierait le coût de démarrage pour des formes que
/// personne ne demande.
///
/// Le monde PAR DÉFAUT passe en premier — c'est celui que la page ouvre — puis
/// les autres, sous une échéance COMMUNE (`budget_precho_arcs_ms`) : un monde
/// lent ne doit pas repousser indéfiniment les suivants.
fn prechauffer_les_arcs(config: &ServeConfig) {
    let budget = budget_precho_arcs_ms();
    if budget == 0 {
        return;
    }
    let config = config.clone();
    std::thread::spawn(move || {
        // On laisse le serveur répondre à ses premières requêtes : ce
        // préchauffage ne doit pas concurrencer la page qui se charge — c'est
        // même tout l'inverse qu'on cherche.
        std::thread::sleep(std::time::Duration::from_millis(2_500));
        let echeance = std::time::Instant::now() + std::time::Duration::from_millis(budget);
        let defaut = config.default_path();
        let mut mondes = tous_les_mondes(&config);
        mondes.sort_by_key(|(_, p)| Some(p) != defaut.as_ref());
        for (label, path) in mondes {
            if std::time::Instant::now() >= echeance {
                tracing::debug!(
                    "préchauffage des arcs : budget épuisé — les mondes restants se feront à la demande"
                );
                break;
            }
            let t = std::time::Instant::now();
            // Le corps est écrit dans les MÊMES caches que la route (mémoire +
            // disque) : le préchauffage ne sert à rien s'il ne remplit pas
            // exactement la clé que la requête ira lire.
            match arcs_corps(&path, 16, palier_arcs(60), None) {
                Ok(body) => tracing::warn!(
                    monde = %label,
                    ms = t.elapsed().as_millis() as u64,
                    octets = body.len(),
                    "agrégat d'arcs préchauffé — le premier visiteur ne paiera plus le froid"
                ),
                Err(e) => tracing::debug!(
                    monde = %label,
                    error = %e,
                    "préchauffage des arcs impossible — l'agrégat se fera à la demande"
                ),
            }
        }
    });
}

/// ── PRÉCHAUFFAGE DES BASES : LA PREMIÈRE REQUÊTE NE PAIE PLUS L'OUVERTURE ───
///
/// Défaut mesuré (2026-09-11) sur le serveur en marche : la PREMIÈRE requête
/// touchant un store paie son ouverture, et ce n'est pas un coût marginal —
/// mesuré, 1 236 ms pour `linux-full3` (1,55 M de nœuds) contre 1,5 ms ensuite.
/// Ce délai tombait donc sur le premier geste de l'utilisateur (ou sur le
/// premier appel d'outil du modèle), au moment précis où il juge la réactivité.
///
/// L'ouverture est en LECTURE SEULE et idempotente : la faire à l'avance ne
/// change aucun résultat, elle déplace seulement le coût là où personne
/// n'attend. On la fait donc en tâche de fond, une base à la fois, avec une
/// petite respiration entre deux : le but est de rendre le service plus rapide,
/// pas de saturer les entrées-sorties au démarrage.
///
/// On journalise la durée RÉELLE de chaque ouverture : c'est la mesure qui
/// permettra de dire, plus tard, si le préchauffage a servi à quelque chose —
/// plutôt que de le supposer.
fn prechauffer_les_bases(config: &ServeConfig) {
    let config = config.clone();
    std::thread::spawn(move || {
        // Le cache disque est élagué AVANT tout : il ne bloque rien (quelques
        // `unlink`) et il rend le démarrage suivant plus léger.
        elaguer_cache_disque();
        // On laisse le serveur répondre à ses premières requêtes d'abord.
        std::thread::sleep(std::time::Duration::from_millis(1200));
        // ── L'ÉCHÉANCE EST COMMUNE À TOUS LES MONDES (2026-09-13) ──────────
        // Mesuré : 20 013 ms pour le seul monde principal (`text` = 13 845 ms).
        // Un budget PAR monde laissait la passe dériver à 9 × 30 s ; une échéance
        // partagée la borne vraiment, et l'ordre des mondes (le défaut d'abord)
        // fait que c'est celui qu'on interroge qui est servi en entier.
        let echeance_precho = std::time::Instant::now()
            + std::time::Duration::from_millis(budget_precho_trigrammes_ms());
        for (label, path) in tous_les_mondes(&config) {
            let t = std::time::Instant::now();
            match cached_ro(&path) {
                // ── POURQUOI `warn!` ET NON `info!` (2026-09-13) ────────────
                // Défaut mesuré : ces deux lignes de durée — la SEULE mesure du
                // poste « payé une fois au démarrage » — étaient écrites en
                // `info!`, alors que le niveau par défaut du binaire est WARN
                // (`main.rs` : `with_max_level(tracing::Level::WARN)` hors
                // `--verbose`/`--quiet`). Vérifié sur le journal du service :
                // 0 ligne INFO sur 61 484. Autrement dit, le préchauffage
                // s'exécutait sans que personne puisse dire ce qu'il coûtait —
                // exactement l'inverse de ce que le commentaire ci-dessus
                // promettait (« la mesure qui permettra de dire si le
                // préchauffage a servi »). On les passe en `warn!` : c'est le
                // seul niveau qui SURVIT au réglage par défaut, et ce sont des
                // lignes de démarrage (une par monde), pas du bruit de requête.
                Ok(_) => tracing::warn!(
                    monde = %label,
                    ms = t.elapsed().as_millis() as u64,
                    "base préchauffée (ouverture en lecture seule)"
                ),
                Err(e) => tracing::debug!(
                    monde = %label,
                    error = %e,
                    "préchauffage impossible — l'ouverture se fera à la demande"
                ),
            }
            // ── LE COÛT DE L'INDEX NE DOIT PAS TOMBER DANS UNE QUESTION ─────
            // Défaut mesuré le 2026-09-13 (chat du 8432) : l'index de TRIGRAMMES
            // se construit à la DEUXIÈME requête d'une propriété (substrat,
            // `OBRAIN_TRIGRAMMES_DES_LA`, défaut 2). Sur un serveur qui vit, cette
            // deuxième requête est une QUESTION D'UTILISATEUR : mesuré
            // **8 007 ms** pour un simple préfixe, en plein milieu d'une
            // conversation. On joue donc ici, en tâche de fond et avant toute
            // question, les deux exécutions qui déclenchent la construction.
            // Le prix est payé une fois, au démarrage, et l'utilisateur ne le voit
            // pas. La sonde est celle des outils (`STARTS WITH` sur
            // `qualified_name`) : c'est cette forme de prédicat qui emprunte la
            // voie de l'index, donc c'est elle qui le construit.
            if let Ok(db) = cached_ro(&path) {
                let t_tri = std::time::Instant::now();
                // ── TOUTES LES PROPRIÉTÉS TEXTUELLES, PAS SEULEMENT `qualified_name` ──
                // Défaut mesuré (2026-09-13) : cette sonde ne visait QUE
                // `qualified_name`, alors que la garde adaptative du substrat
                // (`trigrammes_des_la`, défaut 2) construit l'index PAR
                // PROPRIÉTÉ (`store.rs` : `noeuds_par_predicat_texte`). Toute
                // autre propriété textuelle gardait donc sa construction pour
                // la 2e requête — c'est-à-dire pour une QUESTION D'UTILISATEUR.
                // Re-mesuré ici, clés de cache neuves, `runtime-dev`
                // (414 352 nœuds, 1 267 379 arêtes) :
                //   `n.file_path CONTAINS …` : 10 510 ms · 1 840 ms · 2,7 ms
                //   `n.title     CONTAINS …` :    244 ms ·   611 ms · 2,3 ms
                // (1er appel : balayage complet ; 2e : CONSTRUCTION de l'index ;
                //  3e : régime établi. L'audit annonçait 137/626 ms et
                //  133/261 ms sur la même machine moins chargée.)
                // On joue donc les deux exécutions pour CHAQUE propriété que le
                // store DÉCLARE, en tâche de fond, avant toute question.
                //
                // BORNE, ET POURQUOI : `payload`, `text` et `extraction_text`
                // portent des valeurs possiblement énormes (JSON de nœud
                // entier) ; un budget de temps total empêche le démarrage de
                // dériver, et ce qui n'a PAS été préchauffé est NOMMÉ dans le
                // journal au lieu d'être tu. L'ordre de `PROPS_TEXTUELLES` va du
                // plus utile au moins utile : c'est donc toujours la queue qui
                // est coupée, jamais `qualified_name` ou `file_path`.
                // Le schéma est LIÉ à un local : emprunter ses clés dans un
                // `match db.schema()` les faisait mourir à la fin du bras, et
                // l'ensemble des propriétés présentes ne compilait pas
                // (`E0597`). Le lier ici donne à l'emprunt la durée de vie du
                // préchauffage — c'est la seule chose qui manquait.
                let schema = db.schema();
                let presentes: std::collections::HashSet<&str> = match &schema {
                    obrain_engine::SchemaInfo::Lpg(s) => {
                        s.property_keys.iter().map(|k| k.as_str()).collect()
                    }
                    // Mode RDF : aucun catalogue de ce genre — on ne sonde rien
                    // plutôt que de sonder au hasard.
                    _ => std::collections::HashSet::new(),
                };
                let debut = std::time::Instant::now();
                let mut faites: Vec<&str> = Vec::new();
                let mut sautees: Vec<&str> = Vec::new();
                let mut echeance = echeance_precho;
                // Phase 1 : les propriétés LÉGÈRES (6 168 ms mesurés en tout).
                // Les lourdes (`text`) sont traitées en phase 2, après la pause
                // et pour le seul monde par défaut — voir `PROPS_LOURDES`.
                for prop in PROPS_TEXTUELLES.iter().filter(|p| !PROPS_LOURDES.contains(p)) {
                    // Une propriété ABSENTE du catalogue ferait balayer tout le
                    // store pour rien (et « construirait » un index vide) : on
                    // ne sonde que ce que la base déclare.
                    if !presentes.contains(prop) {
                        continue;
                    }
                    if std::time::Instant::now() >= echeance {
                        sautees.push(prop);
                        continue;
                    }
                    let t_prop = std::time::Instant::now();
                    // L'aiguille est celle des outils (`CONTAINS`) : les trois
                    // prédicats de sous-chaîne traversent le MÊME index
                    // (`store.rs`, `noeuds_par_predicat_texte`), et c'est donc
                    // la même construction qui est déclenchée. Deux exécutions :
                    // la garde adaptative construit à la deuxième.
                    let sonde = format!(
                        "MATCH (n) WHERE n.{prop} CONTAINS '{AIGUILLE_PRECHO}' RETURN 1 LIMIT 1"
                    );
                    for _ in 0..2 {
                        let _ = db.session().execute(&sonde);
                    }
                    tracing::warn!(
                        monde = %label,
                        propriete = prop,
                        ms = t_prop.elapsed().as_millis() as u64,
                        "trigrammes préchauffés (une propriété)"
                    );
                    faites.push(prop);
                }
                tracing::warn!(
                    monde = %label,
                    proprietes = faites.len(),
                    restantes = ?sautees,
                    ms = t_tri.elapsed().as_millis() as u64,
                    "trigrammes préchauffés (la construction ne tombera plus dans une question)"
                );
            }
            // Budget épuisé : inutile d'ouvrir les mondes suivants pour ne rien
            // y préchauffer — on le DIT, puis on sort de la passe.
            if std::time::Instant::now() >= echeance_precho {
                tracing::warn!(
                    budget_ms = budget_precho_trigrammes_ms(),
                    "préchauffage des trigrammes : budget épuisé — les mondes suivants gardent \
                     leurs index pour leur première question (OBRAIN_PRECHO_TRIGRAMMES_MS)"
                );
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        // ── PHASE 2 : LES PROPRIÉTÉS LOURDES, APRÈS LA PAUSE (2026-09-13) ───
        //
        // Mesuré, et c'est ce qui a imposé cette phase : `text` coûte
        // **13 845 ms** de construction (index de 51,5 Mo) — à lui seul 69 % du
        // préchauffage. Fait au démarrage, il faisait passer le PREMIER appel
        // d'arcs de 4,6 s à **19,0 s** (contention sur la base et le CPU) : le
        // remède était pire que le mal. On le fait donc après une pause, quand
        // le premier moment d'usage est passé — et seulement pour le monde PAR
        // DÉFAUT, celui que le chat et les outils interrogent (51 Mo d'index par
        // monde n'auraient aucun sens).
        //
        // Contrepartie ASSUMÉE et mesurée : pendant les ~2 premières minutes,
        // une question sur `text` peut encore payer la construction (mesuré
        // 20 880 ms en AVANT, sur la 2e demande de la propriété). Le remède
        // complet serait de le préchauffer tout de suite — c'est précisément ce
        // qu'on a mesuré comme nuisible.
        if let Some(defaut) = config.default_path() {
            let reste = PROPS_TEXTUELLES
                .iter()
                .filter(|p| PROPS_LOURDES.contains(p))
                .count();
            if reste > 0 {
                tracing::warn!(
                    pause_s = precho_pause_lourdes_s(),
                    proprietes = reste,
                    "préchauffage : phase 2 (propriétés lourdes) dans la pause — elles ne \
                     doivent pas concurrencer les premières requêtes"
                );
                std::thread::sleep(std::time::Duration::from_secs(precho_pause_lourdes_s()));
                if let Ok(db) = cached_ro(&defaut) {
                    let schema = db.schema();
                    let presentes: std::collections::HashSet<&str> = match &schema {
                        obrain_engine::SchemaInfo::Lpg(s) => {
                            s.property_keys.iter().map(|k| k.as_str()).collect()
                        }
                        _ => std::collections::HashSet::new(),
                    };
                    let echeance2 = std::time::Instant::now()
                        + std::time::Duration::from_millis(budget_precho_trigrammes_ms());
                    for prop in PROPS_TEXTUELLES.iter().filter(|p| PROPS_LOURDES.contains(p)) {
                        if !presentes.contains(prop) {
                            continue;
                        }
                        if std::time::Instant::now() >= echeance2 {
                            tracing::warn!(
                                propriete = prop,
                                "phase 2 : budget épuisé — la propriété reste paresseuse"
                            );
                            continue;
                        }
                        let t_prop = std::time::Instant::now();
                        let sonde = format!(
                            "MATCH (n) WHERE n.{prop} CONTAINS '{AIGUILLE_PRECHO}' RETURN 1 LIMIT 1"
                        );
                        for _ in 0..2 {
                            let _ = db.session().execute(&sonde);
                        }
                        tracing::warn!(
                            monde = ?defaut.file_name(),
                            propriete = prop,
                            ms = t_prop.elapsed().as_millis() as u64,
                            "phase 2 : trigrammes préchauffés (propriété lourde)"
                        );
                    }
                }
            }
        }
    });
}

/// L'ENTRETIEN DES BASES — l'enrichissement est le RÉGIME, pas une option.
///
/// Principe posé par l'utilisateur : « tous les enrichissements doivent être de
/// base, on vise une pleine cognition ». Une base servie sans ses signaux n'est
/// pas une base cognitive — c'est un graphe brut. Cette boucle tient donc, en
/// continu et sans opérateur, ce qui fait la différence :
///
///   · **courbure de Ricci** (`refresh_all_ricci`) — le relief qui distingue un
///     pont d'une autoroute, et que `node_curvature` relit ;
///   · **degrés par type** — les registres `substrate.degrees.node.<TYPE>.u32`,
///     que le substrat tient en lazy ; on vérifie qu'ils se matérialisent (une
///     lecture les reconstruit s'ils manquent) ;
///   · et tout ce qui viendra s'ajouter ici.
///
/// ## Ce qui a changé (2026-09-12)
///
/// La version précédente faisait `let Ok(db) = cached_ro(&path) else { continue }`
/// — un `continue` **silencieux**. Un monde qui ne s'ouvrait pas n'était jamais
/// enrichi et rien ne le disait : la base restait brute, indéfiniment, en
/// donnant l'impression que l'entretien passait. C'est le contraire d'un
/// enrichissement de base. On ouvre donc avec persévérance, on **nomme** ce qui
/// échoue et pourquoi, et on rend un compte par passe.
fn spawn_entretien(config: &ServeConfig, minutes: u64) {
    if minutes == 0 {
        // L'arrêt reste possible (diagnostic, mesure), mais il n'est plus le
        // défaut : l'enrichissement est la base.
        eprintln!(
            "viz serve : ENTRETIEN DÉSACTIVÉ (--enrich-ricci 0) — les bases seront servies \
             sans entretien de leurs signaux"
        );
        return;
    }
    let config = config.clone();
    std::thread::spawn(move || {
        // Premier passage différé : on laisse le serveur répondre à ses premières
        // requêtes avant de relire le graphe.
        std::thread::sleep(std::time::Duration::from_secs(30));
        let periode = std::time::Duration::from_secs(minutes.saturating_mul(60));
        loop {
            let mut enrichis = 0usize;
            let mut ignores: Vec<String> = Vec::new();
            for (label, path) in tous_les_mondes(&config) {
                // Peek d'abord (le préchauffage a déjà ouvert les mondes), puis
                // ouverture : un entretien ne doit pas renoncer parce qu'un
                // handle n'est pas encore posé.
                let db = cached_ro_si_present(&path).or_else(|| cached_ro(&path).ok());
                let Some(db) = db else {
                    ignores.push(format!("{label} (illisible)"));
                    continue;
                };
                let Some(sub) = db.substrate_handle() else {
                    ignores.push(format!("{label} (sans substrat)"));
                    continue;
                };
                match obrain_substrate::geometric::refresh_all_ricci(&sub) {
                    Ok(st) => {
                        let _ = sub.flush();
                        // Les DEUX registres de degrés, pour qu'ils se
                        // matérialisent : une lecture les reconstruit s'ils
                        // manquent (lazy-init du substrat).
                        let _ = sub.out_degree(obrain_common::types::NodeId(1));
                        enrichis += 1;
                        // `edges_updated: 0` = déjà à jour (passe idempotente) :
                        // on ne journalise que ce qui a bougé.
                        if st.edges_updated > 0 {
                            eprintln!(
                                "viz serve : entretien {label} — courbure {} arêtes mises à jour \
                                 ({} visitées)",
                                st.edges_updated, st.edges_visited
                            );
                        }
                        // ── LES ENREGISTREMENTS CORROMPUS : DÉTECTION SANS EXCLUSIVITÉ ──
                        // `obrain repair` exige l'exclusivité, donc il ne peut même pas
                        // DIAGNOSTIQUER une base tenue — et aucune demande ne s'attachait
                        // pour elle. Or on vient de les compter au chargement : c'est la
                        // mesure qui manquait. Au-delà de 100 propriétés illisibles, ce
                        // n'est plus un incident isolé, c'est une base à réparer.
                        let echecs = sub.props_decode_failures();
                        if echecs > 100 {
                            let _ = crate::entretien::attacher(
                                &path,
                                crate::entretien::Genre::Enregistrements,
                                &format!(
                                    "{echecs} propriétés illisibles au chargement — réparation \
                                     impossible tant que la base est tenue (le diagnostic lui-même \
                                     exige l'exclusivité)"
                                ),
                                "entretien",
                            );
                            eprintln!(
                                "viz serve : entretien {label} — {echecs} propriétés illisibles, \
                                 réparation ATTACHÉE (à appliquer à la prochaine fenêtre libre)"
                            );
                        }

                        // ── LES HUBS, CALCULÉS PAR LES REGISTRES ────────────
                        // Le classement par degré coûtait 14,1 s par parcours
                        // (`MATCH (n)--(m) … ORDER BY count(m) DESC`) — refusé
                        // par le chat. Les registres de degrés le rendent
                        // linéaire : on le calcule ICI, pendant l'entretien, et
                        // on le persiste pour que les lecteurs le relisent en
                        // quelques millisecondes.
                        let classement =
                            crate::hubs::calculer(&db, &path, crate::hubs::HUBS_PAR_DEFAUT);
                        if let Some(refus) = &classement.refus {
                            // On PERSISTE le refus : un lecteur doit savoir que
                            // le classement n'existe pas, et pourquoi — sinon il
                            // croira qu'aucun nœud n'est connecté.
                            let _ = crate::hubs::persister(&path, &classement);
                            eprintln!("viz serve : entretien {label} — classement REFUSÉ : {refus}");
                        } else if !classement.hubs.is_empty() {
                            let _ = crate::hubs::persister(&path, &classement);
                            eprintln!(
                                "viz serve : entretien {label} — {} hub(s) en {} ms ({})",
                                classement.hubs.len(),
                                classement.calcul_ms,
                                classement.source
                            );
                        }
                    }
                    Err(e) => ignores.push(format!("{label} (courbure : {e})")),
                }
            }
            // LE COMPTE EST RENDU : une passe d'entretien qui ne dit pas ce
            // qu'elle a fait ne se distingue pas d'une passe qui n'a rien fait.
            eprintln!(
                "viz serve : entretien — {enrichis}/{} monde(s) enrichi(s){}",
                enrichis + ignores.len(),
                if ignores.is_empty() {
                    String::new()
                } else {
                    format!(" · NON enrichis : {}", ignores.join(", "))
                }
            );
            std::thread::sleep(periode);
        }
    });
}
pub fn run_serve(config: ServeConfig, host: &str, port: u16, quiet: bool) -> Result<()> {
    let addr = format!("{host}:{port}");
    let listener = TcpListener::bind(&addr).with_context(|| format!("viz serve : bind {addr}"))?;
    if !quiet {
        let label = config.default_label.clone().unwrap_or_else(|| {
            config
                .default_path()
                .map_or_else(|| "(aucun)".into(), |p| p.display().to_string())
        });
        let np = tous_les_mondes(&config).len();
        eprintln!(
            "viz serve : http://{addr}  (défaut : {label}, {np} presets) — Ctrl-C pour arrêter"
        );
        // ── L'ÉTAT D'ACCÈS EST ANNONCÉ, PROTÉGÉ OU NON ──────────────────────
        // Audit du 2026-09-13 : ce serveur a servi le graphe sans demander la
        // moindre identité, et RIEN ne le disait. Un état non protégé doit être
        // visible dans le terminal qui l'a lancé, pas découvrable après coup.
        {
            let garde = GardeIam::partagee();
            if garde.est_armee() {
                eprintln!("viz serve : {}", garde.etat());
            } else {
                eprintln!("viz serve : ⚠ {}", garde.etat());
            }
        }
    }
    spawn_dev_watcher(); // OBRAIN_DEV=1 → rebuild auto + hot reload
    // Entretien de la courbure : le serveur tient les verrous, c'est donc lui qui
    // écrit — sinon aucune passe n'est possible (voir spawn_enrich_ricci).
    spawn_entretien(&config, config.enrich_ricci_minutes);
    // Les MONDES sont préchauffés en tâche de fond — mais la BASE SYNAPTIQUE ne
    // l'était pas. Elle était donc ouverte paresseusement, à la première
    // requête qui en a besoin (le chat, la toile, la mémoire) : une ouverture
    // qui peut BLOQUER quand un autre processus la tient en écriture, et qui
    // figeait alors ces endpoints pendant que `/api/query` continuait de
    // répondre. On la préchauffe comme les autres.
    prechauffer_synapse(&config);
    prechauffer_les_bases(&config);
    // L'agrégat d'`arcs` était le dernier coût froid qui tombait sur une
    // requête : mesuré 2 050 ms store ouvert, 6 240 ms cache disque vide. Il se
    // pose maintenant au démarrage, comme les ouvertures et l'index de
    // trigrammes (voir `prechauffer_les_arcs` pour la mesure et le périmètre).
    prechauffer_les_arcs(&config);
    // Une requête par thread : une extraction lente (gros store) ne bloque
    // plus les autres clients (navigation/vue).
    for stream in listener.incoming() {
        let config = config.clone();
        std::thread::spawn(move || match stream {
            Ok(s) => {
                if let Err(e) = handle(s, &config) {
                    eprintln!("viz serve : requête en erreur : {e:#}");
                }
            }
            Err(e) => eprintln!("viz serve : connexion : {e}"),
        });
    }
    Ok(())
}

/// ── LA GARDE IAM, PARTAGÉE PAR TOUS LES POINTS D'ENTRÉE DU BINAIRE ─────────
///
/// Enveloppe MINCE : la règle vit dans `obrain_iam::Garde` (bibliothèque), et
/// c'est voulu — un seul endroit décide, pour que le serveur HTTP, le pont MCP
/// et le CLI n'en divergent pas. Cette enveloppe n'existe que pour franchir la
/// frontière de FEATURE : le build par défaut n'embarque pas l'IAM (la
/// dépendance reste optionnelle), et un point d'entrée ne doit pas s'en
/// apercevoir autrement que par un ÉTAT.
///
/// Elle est hébergée ici parce que ce module porte déjà les aides d'ouverture de
/// bases partagées (`cached_ro`) : c'est le seul endroit du binaire où un
/// `ObrainDB` s'ouvre pour être partagé.
///
/// ── (RÉ)ÉCRITE LE 2026-09-13 APRÈS UN ÉCRASEMENT ────────────────────────
/// Cette garde avait été posée ici, puis ce fichier a été réécrit par une autre
/// session (le 13/09 à 14:52) et l'a perdue — sans que le reste du binaire
/// s'en aperçoive, puisque `mcp_server.rs` la référençait toujours. Elle est
/// reposée ici, et la leçon est notée : dans ce dépôt, un fichier partagé peut
/// disparaître sous vos pieds.
pub(crate) struct GardeIam {
    #[cfg(feature = "iam")]
    interieure: Option<std::sync::Arc<obrain_iam::Garde>>,
    /// Le magasin d'IDENTITÉS, ouvert sur la MÊME base que la garde.
    ///
    /// ── POURQUOI IL EST ICI, ET PAS ROUVERT AILLEURS ────────────────────────
    /// C'est ce qui permet aux routes d'administration de lire et d'écrire des
    /// comptes. Il vient de l'ouverture qui a servi à la garde : deux ouvertures
    /// distinctes du même fichier donneraient deux vues — l'administration
    /// écrirait dans l'une ce que la garde ne lirait pas, et une promotion
    /// d'administrateur pourrait rester sans effet apparent.
    #[cfg(feature = "iam")]
    identites: Option<std::sync::Arc<obrain_iam::IamStore>>,
    /// Le magasin d'identités est-il ouvert en ÉCRITURE ? Sans cela, émettre ou
    /// révoquer une session n'a aucun effet — et le taire serait un mensonge.
    #[allow(dead_code)]
    ecriture: bool,
    etat: String,
}

// ── RÉUNIFICATION (2026-10-06) : DEUX GARDES, DEUX DÉFAUTS DIFFÉRENTS ───────
// Les deux lignées ont corrigé « deux écrivains dans `iam.obrain` », mais pas
// le MÊME défaut, et aucune des deux corrections ne remplace l'autre :
//   · la lignée servie a corrigé le PONT qui ouvrait `iam.obrain` en écriture
//     (le jeton du compte de service disparaissait au 2e démarrage) → drapeau
//     `IAM_EN_LECTURE`, consommé par `lecteur_seulement()` et par `depuis_env` ;
//   · le multi-écrivain a corrigé la garde elle-même, qui vivait dans un
//     `static` LOCAL à `partagee()` et se refermait donc selon l'environnement
//     du développeur → `GARDE_IAM` au niveau du module, plus
//     `desarmer_pour_les_tests` (herméticité de la suite de tests).
// Les deux `static` sont donc GARDÉS : le code fusionné en a besoin des deux.
/// (2026-09-16) Le processus n'est PAS l'écrivain du magasin d'identités.
///
/// Mesuré sur n3 : chaque pont MCP du harness (`obrain bridge`) ouvrait
/// `iam.obrain` en ÉCRITURE partagée pour y journaliser son audit, en même
/// temps que `viz serve` — le seul écrivain prévu. Plusieurs écrivains
/// s'allouent les mêmes slots et les mêmes pages : le jeton du compte de
/// service `harness-writer` disparaissait au redémarrage suivant de viz
/// (reproduit : valide après le 1er démarrage de viz, refusé après le 2e).
/// Un pont ne fait que VÉRIFIER une identité : il ouvre le magasin en lecture
/// seule, et son audit (qui ne peut plus s'écrire) est perdu plutôt que
/// d'écraser des identités. À poser AVANT le premier `partagee()`.
static IAM_EN_LECTURE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// La garde IAM du processus — ouverte UNE fois, partagée par le pont et le
/// serveur.
///
/// Elle est ici, au niveau du module, plutôt que dans `partagee()` : c'est ce
/// qui permet aux tests unitaires de poser la garde INERTE avant la première
/// lecture (voir [`GardeIam::desarmer_pour_les_tests`]). Une porte qui se
/// referme selon l'environnement du développeur n'est pas une porte : elle est
/// **non hermétique**, et c'est exactement le défaut mesuré le 2026-09-16 —
/// la suite de tests devenait rouge sur une machine où `OBRAIN_IAM` était armé,
/// sans qu'une seule ligne de code ait changé.
static GARDE_IAM: std::sync::OnceLock<GardeIam> = std::sync::OnceLock::new();
impl GardeIam {
    /// Déclare que ce processus ouvrira le magasin d'identités en LECTURE
    /// SEULE (pont, serveur MCP). Sans effet si la garde est déjà ouverte.
    pub(crate) fn lecteur_seulement() {
        IAM_EN_LECTURE.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// La garde du processus — ouverte UNE fois.
    pub(crate) fn partagee() -> &'static GardeIam {
        GARDE_IAM.get_or_init(Self::depuis_env)
    }

    /// (tests) Pose la garde INERTE si rien n'a encore été ouvert.
    ///
    /// Sans elle, les tests unitaires du pont lisaient `OBRAIN_IAM_STORE` /
    /// `OBRAIN_IAM_TOKEN` de la machine qui les exécute : armés, ils recevaient
    /// `401` et échouaient ; désarmés, ils passaient. Un test qui dépend de
    /// l'environnement de celui qui le lance ne prouve rien.
    ///
    /// `get_or_init` et non `set` : si une garde réelle a déjà été ouverte, on
    /// ne la remplace pas — on ne ment pas sur l'état réel.
    #[cfg(test)]
    pub(crate) fn desarmer_pour_les_tests() {
        GARDE_IAM.get_or_init(|| GardeIam {
            #[cfg(feature = "iam")]
            interieure: None,
            #[cfg(feature = "iam")]
            identites: None,
            ecriture: false,
            etat: "IAM NON ARMÉ (isolement des tests unitaires — voir GardeIam::desarmer_pour_les_tests)"
                .to_string(),
        });
    }

    fn depuis_env() -> Self {
        #[cfg(feature = "iam")]
        {
            let inerte = |etat: String| GardeIam { interieure: None, identites: None, ecriture: false, etat };
            let Some(chemin) = std::env::var("OBRAIN_IAM_STORE")
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
            else {
                return inerte(
                    "IAM NON ARMÉ — OBRAIN_IAM_STORE n'est pas défini : aucune identité n'est \
                     exigée, quiconque atteint ce port lit et écrit tout le graphe. \
                     (OBRAIN_IAM=1 arme le refus, OBRAIN_IAM_STORE=<base> désigne les identités.)"
                        .to_string(),
                );
            };
            // ── LE MAGASIN S'OUVRE EN ÉCRITURE, ET C'EST INDISPENSABLE ──────
            // Défaut mesuré le 2026-09-13 : ouvert en LECTURE SEULE, la garde
            // *exécutait* la déconnexion (« revoked: true ») sans que RIEN ne
            // prenne effet — 4 credentials avant, 4 après, et le jeton
            // continuait d'authentifier. Une chaîne qui ne peut pas fermer une
            // session n'en est pas une.
            //
            // `open_shared_write` (verrou PARTAGÉ + WAL) est le mode prévu pour
            // l'UNIQUE écrivain d'un store : ici, le magasin d'IDENTITÉS, qui
            // doit donc être une base DÉDIÉE (`OBRAIN_IAM_STORE=…/iam.obrain`).
            // S'il ne peut pas s'ouvrir en écriture, on se rabat sur la lecture
            // seule — mais on le DIT, et les routes de session le disent aussi :
            // mieux vaut une connexion annoncée impossible qu'une déconnexion
            // qui ne ferme rien.
            let ouverture: std::result::Result<obrain_engine::ObrainDB, String> =
                if IAM_EN_LECTURE.load(std::sync::atomic::Ordering::SeqCst) {
                    Err("processus lecteur (pont) : le magasin d'identités n'a qu'un écrivain, viz serve".to_string())
                } else {
                    obrain_engine::ObrainDB::open_shared_write(std::path::Path::new(&chemin))
                        .map_err(|e| e.to_string())
                };
            match ouverture {
                Ok(db) => {
                    let graphe = db.graph_store();
                    let garde = std::sync::Arc::new(obrain_iam::Garde::depuis_env(
                        std::sync::Arc::clone(&graphe),
                    ));
                    let identites =
                        std::sync::Arc::new(obrain_iam::IamStore::new(graphe));
                    let etat = format!("{} (magasin en écriture : sessions émises et révoquées)", garde.etat());
                    GardeIam { interieure: Some(garde), identites: Some(identites), ecriture: true, etat }
                }
                Err(e_ecriture) => {
                    match obrain_engine::ObrainDB::open_read_only(std::path::Path::new(&chemin)) {
                        Ok(db) => {
                            let graphe = db.graph_store();
                            // Ouvert en lecture : le substrate n'y refuse pas les
                            // mutations, c'est donc la garde qui s'abstient d'écrire.
                            let garde = std::sync::Arc::new(
                                obrain_iam::Garde::depuis_env_lecteur(std::sync::Arc::clone(&graphe)),
                            );
                            let identites =
                                std::sync::Arc::new(obrain_iam::IamStore::new(graphe).sans_audit());
                            let etat = format!(
                                "{} — ⚠ magasin d'identités ouvert en LECTURE SEULE ({e_ecriture}) : \
                                 les sessions ne peuvent ni s'émettre ni se révoquer, et \
                                 l'administration des comptes est fermée.",
                                garde.etat()
                            );
                            GardeIam { interieure: Some(garde), identites: Some(identites), ecriture: false, etat }
                        }
                        Err(e) => inerte(format!(
                            "IAM NON ARMÉ — le magasin d'identités « {chemin} » n'a pas pu être \
                             ouvert ({e}) : aucune identité n'est exigée."
                        )),
                    }
                }
            }
        }
        #[cfg(not(feature = "iam"))]
        {
            GardeIam {
                ecriture: false,
                etat: "IAM NON COMPILÉ — cette construction n'embarque pas l'IAM \
                       (construire avec `--features ai,iam`) : aucune identité n'est exigée."
                    .to_string(),
            }
        }
    }

    /// L'état, pour l'annonce de démarrage et `/api/info`.
    pub(crate) fn etat(&self) -> &str {
        &self.etat
    }

    /// L'IAM exige-t-il une identité ?
    pub(crate) fn est_armee(&self) -> bool {
        #[cfg(feature = "iam")]
        {
            self.interieure.as_ref().is_some_and(|g| g.armee())
        }
        #[cfg(not(feature = "iam"))]
        {
            false
        }
    }

    /// Le magasin est-il ouvert ET ÉCRIVABLE ? Les routes de session en ont
    /// besoin : émettre un jeton écrit un credential, le révoquer écrit une
    /// expiration. Un magasin en lecture seule rendrait ces deux gestes
    /// décoratifs — d'où ce refus explicite plutôt qu'un succès sans effet.
    pub(crate) fn disponible(&self) -> bool {
        #[cfg(feature = "iam")]
        {
            self.interieure.is_some() && self.ecriture
        }
        #[cfg(not(feature = "iam"))]
        {
            false
        }
    }

    /// La garde d'identités SOUS-JACENTE, quand elle est armée.
    ///
    /// Exposée pour les vérifications qui ne portent pas sur un espace VISÉ mais
    /// sur les espaces PROPRES de l'appelant (`Garde::controler_soi`) : le
    /// jugement doit rester dans la garde, avec ses politiques et sa piste
    /// d'audit — le serveur, lui, ne fait que choisir la ressource à juger.
    #[cfg(feature = "iam")]
    pub(crate) fn interieure(&self) -> Option<&std::sync::Arc<obrain_iam::Garde>> {
        self.interieure.as_ref()
    }

    /// Rend `Err((code, raison))` quand il faut refuser : 401 quand l'identité
    /// manque ou ne se lit pas, 403 quand la politique refuse.
    pub(crate) fn controler(
        &self,
        jeton: Option<&str>,
        ecriture: bool,
        espace: &str,
    ) -> Result<(), (u16, String)> {
        #[cfg(feature = "iam")]
        {
            let Some(garde) = self.interieure.as_ref() else {
                return Ok(());
            };
            match garde.authentifier(jeton) {
                Ok(session) => garde
                    .controler(session.as_ref(), ecriture, espace)
                    .map_err(|e| (403, e.to_string())),
                Err(obrain_iam::IamError::AccessDenied { reason }) => Err((401, reason)),
                Err(e) => Err((401, e.to_string())),
            }
        }
        #[cfg(not(feature = "iam"))]
        {
            let _ = (jeton, ecriture, espace);
            Ok(())
        }
    }

    /// ── CONNEXION : le premier maillon de la chaîne ──────────────────────
    pub(crate) fn connecter(
        &self,
        utilisateur: &str,
        mot_de_passe: &str,
        ttl_secs: u64,
    ) -> Result<String, String> {
        #[cfg(feature = "iam")]
        {
            let Some(garde) = self.interieure.as_ref() else {
                return Err("IAM non armé : aucune connexion possible (OBRAIN_IAM=1 et \
                            OBRAIN_IAM_STORE=<base>)"
                    .to_string());
            };
            garde
                .connecter(utilisateur, mot_de_passe, ttl_secs)
                .map_err(|e| e.to_string())
        }
        #[cfg(not(feature = "iam"))]
        {
            let _ = (utilisateur, mot_de_passe, ttl_secs);
            Err("IAM non compilé dans cette construction".to_string())
        }
    }

    /// ── DÉCONNEXION : révocation réelle ──────────────────────────────────
    pub(crate) fn deconnecter(&self, jeton: &str) -> Result<bool, String> {
        #[cfg(feature = "iam")]
        {
            let Some(garde) = self.interieure.as_ref() else {
                return Ok(false);
            };
            garde.deconnecter(jeton).map_err(|e| e.to_string())
        }
        #[cfg(not(feature = "iam"))]
        {
            let _ = jeton;
            Ok(false)
        }
    }

    /// ── RENOUVELLEMENT : rotation du jeton de session ────────────────────
    pub(crate) fn renouveler(&self, jeton: &str, ttl_secs: u64) -> Result<Option<String>, String> {
        #[cfg(feature = "iam")]
        {
            let Some(garde) = self.interieure.as_ref() else {
                return Ok(None);
            };
            garde.renouveler(jeton, ttl_secs).map_err(|e| e.to_string())
        }
        #[cfg(not(feature = "iam"))]
        {
            let _ = (jeton, ttl_secs);
            Ok(None)
        }
    }

    /// ── LA PISTE D'AUDIT, telle qu'un opérateur la lit ───────────────────
    pub(crate) fn audit(&self, limite: usize) -> Vec<serde_json::Value> {
        #[cfg(feature = "iam")]
        {
            let Some(garde) = self.interieure.as_ref() else {
                return Vec::new();
            };
            garde
                .audit(limite)
                .into_iter()
                .map(|e| {
                    serde_json::json!({
                        "quand": e.timestamp,
                        "principal": e.principal.to_string(),
                        "action": e.action,
                        "ressource": e.resource.to_string(),
                        "verdict": format!("{:?}", e.result),
                    })
                })
                .collect()
        }
        #[cfg(not(feature = "iam"))]
        {
            let _ = limite;
            Vec::new()
        }
    }

    /// Contrôle pour une action NOMMÉE (`iam:audit`), au-delà de lire/écrire.
    pub(crate) fn controler_nomme(
        &self,
        jeton: Option<&str>,
        action: &str,
        espace: &str,
    ) -> Result<(), (u16, String)> {
        #[cfg(feature = "iam")]
        {
            let Some(garde) = self.interieure.as_ref() else {
                return Ok(());
            };
            match garde.authentifier(jeton) {
                Ok(session) => garde
                    .controler_action(session.as_ref(), action, espace)
                    .map_err(|e| (403, e.to_string())),
                Err(obrain_iam::IamError::AccessDenied { reason }) => Err((401, reason)),
                Err(e) => Err((401, e.to_string())),
            }
        }
        #[cfg(not(feature = "iam"))]
        {
            let _ = (jeton, action, espace);
            Ok(())
        }
    }

    /// ── JUGER SUR LES ESPACES DU COMPTE, PAS SUR UN NOM DE FICHIER ─────────
    ///
    /// Défaillance 5.2 (`docs/DEVELOPPEMENT-SELF-PAR-COMPTE-2026-09-16.md`),
    /// mesurée sur une instance réelle : identité VALIDE, et pourtant
    ///
    /// ```text
    /// GET /api/engine/memoire → 403 permission denied: action=graph:read
    ///   resource=orn:obrain:graph:obrain:tenant/runtime-essai
    /// ```
    ///
    /// Le compte s'appelait `alice`, son espace `esp-…`, et le refus parlait du
    /// **nom du store servi** : la ressource jugée était un NOM DE FICHIER, pas
    /// la ressource sur laquelle le compte détient un droit. « Au nom de quoi ce
    /// geste est-il refusé ? » n'avait donc pas de bonne réponse.
    ///
    /// Ici, la ressource jugée est l'ESPACE du compte — celui que l'annuaire lui
    /// connaît (`espaces_de_l_appelant`), et dont le DROIT reste évalué par le
    /// moteur de politiques, un espace après l'autre. Nommer n'est pas
    /// autoriser : si aucun de ses espaces ne porte l'action, le refus est
    /// prononcé, et il DIT lesquels ont été essayés — un refus qui ne dit pas où
    /// n'est pas une réponse.
    ///
    /// Le refus est `403` (et non `401`) : l'identité est bonne, c'est le droit
    /// qui manque. `espaces_de_l_appelant` distingue déjà les deux (401 sans
    /// jeton, 503 si l'annuaire est illisible).
    pub(crate) fn controler_ses_espaces(
        &self,
        jeton: Option<&str>,
        ecriture: bool,
    ) -> Result<(), (u16, String)> {
        #[cfg(feature = "iam")]
        {
            if !self.est_armee() {
                // Sans IAM armé, il n'y a aucun droit à juger — même convention
                // que `Garde::controler` et que `/api/stores`.
                return Ok(());
            }
            // ── L'ABSENCE D'IDENTITÉ SE DIT COMME PARTOUT AILLEURS ──────────
            // Sans ce passage, cette famille de routes répondait « aucune
            // identité présentée » là où le reste du serveur répond « aucune
            // identité présentée (en-tête `Authorization: Bearer <jeton>`…) » :
            // deux textes pour une même situation, donc un moyen de DEVINER la
            // route par la forme de son refus — exactement l'oracle que la
            // correction du manque 6 refuse de créer ailleurs.
            self.identite_ou_refus(jeton)?;
            let espaces = espaces_de_l_appelant(jeton)?;
            let Some(garde) = self.interieure.as_ref() else {
                return Ok(());
            };
            let action = if ecriture {
                garde.action_ecriture().to_string()
            } else {
                ACTION_LECTURE_HTTP.to_string()
            };
            // ── LE NOM VIENT DE L'IDENTITÉ, PAS DE LA SESSION ────────────────
            // `authentifier` rend un `Option` (il n'a pas d'identité à rendre
            // quand la garde n'est pas armée — cas déjà écarté plus haut) :
            // l'absence est un refus NOMMÉ, jamais un passage silencieux.
            let nom = jeton
                .map(str::trim)
                .filter(|j| !j.is_empty())
                .and_then(|j| self.identite(j))
                .map(|i| nom_depuis_identite(&i))
                .unwrap_or_else(|| "identité inconnue".to_string());
            let session = match garde.authentifier(jeton) {
                Ok(s) => s,
                Err(obrain_iam::IamError::AccessDenied { reason }) => return Err((401, reason)),
                Err(e) => return Err((401, e.to_string())),
            };
            if session.is_none() {
                return Err((
                    401,
                    "aucune identité authentifiée : cette ressource est celle du compte, il faut \
                     la présenter"
                        .to_string(),
                ));
            }
            if espaces.is_empty() {
                return Err((
                    403,
                    format!(
                        "le compte « {nom} » n'a accès à AUCUN espace : cette ressource est la \
                         sienne, mais le droit se juge sur ses espaces, et il n'en a pas \
                         (demandez à l'administrateur de vous en accorder un)"
                    ),
                ));
            }
            let mut motifs: Vec<String> = Vec::new();
            for espace in &espaces {
                match garde.controler_action(session.as_ref(), &action, espace) {
                    // UN espace qui autorise suffit : la ressource a un lieu où
                    // avoir lieu.
                    Ok(()) => return Ok(()),
                    Err(e) => motifs.push(format!("{espace} : {e}")),
                }
            }
            Err((
                403,
                format!(
                    "aucun des espaces du compte « {nom} » ({}) n'autorise « {action} » : {} — la \
                     ressource visée est pourtant la VÔTRE ; le droit, lui, se juge sur vos \
                     ESPACES, jamais sur le nom du fichier de la base",
                    espaces.join(", "),
                    motifs.join(" ; ")
                ),
            ))
        }
        #[cfg(not(feature = "iam"))]
        {
            let _ = (jeton, ecriture);
            Ok(())
        }
    }

    /// Le magasin d'identités — la matière première de l'administration.
    ///
    /// `None` quand l'IAM n'est pas armé (pas de `OBRAIN_IAM_STORE`) ou quand la
    /// base n'a pas pu s'ouvrir : dans ce cas il n'y a pas d'identités à
    /// administrer, et les routes d'administration le diront.
    #[cfg(feature = "iam")]
    pub(crate) fn identites(&self) -> Option<std::sync::Arc<obrain_iam::IamStore>> {
        self.identites.as_ref().map(std::sync::Arc::clone)
    }

    /// Le magasin d'identités est-il ouvert en ÉCRITURE ?
    ///
    /// Les routes d'administration MUTANTES en ont besoin : sur un magasin
    /// ouvert en lecture seule, elles répondraient « fait » sans rien écrire —
    /// exactement le défaut déjà mesuré ici pour la déconnexion (« 4 credentials
    /// avant, 4 après »). Mieux vaut un refus nommé qu'un succès menteur.
    pub(crate) fn magasin_ecriture(&self) -> bool {
        self.ecriture
    }

    /// L'identité est-elle vérifiable ? `401` sinon, et rien à faire si l'IAM
    /// n'est pas armé.
    ///
    /// ── POURQUOI L'IDENTITÉ SE VÉRIFIE AVANT LE RESTE ───────────────────────
    /// Un refus d'authentification PRIME sur un paramètre manquant. Dans l'autre
    /// ordre, un inconnu apprendrait quelles routes existent et ce qu'elles
    /// attendent ; et un appelant légitime croirait à une faute de sa part alors
    /// qu'il n'a simplement pas de session.
    pub(crate) fn identite_verifiee(&self, jeton: Option<&str>) -> Result<(), (u16, String)> {
        if !self.est_armee() {
            return Ok(());
        }
        let Some(jeton) = jeton.filter(|j| !j.trim().is_empty()) else {
            return Err((
                401,
                "aucune identité présentée (en-tête `Authorization: Bearer <jeton>`, cookie \
                 `obrain_session`, ou `?token=`) : l'IAM est armé."
                    .to_string(),
            ));
        };
        if self.identite(jeton).is_none() {
            return Err((401, "jeton invalide, révoqué ou expiré".to_string()));
        }
        Ok(())
    }

    /// ── L'IDENTITÉ SEULE, SANS JUGER AUCUN ESPACE (2026-09-16) ────────────
    ///
    /// Rend le refus EXACT que prononce la règle générique (`GardeIam::controler`)
    /// quand l'identité manque ou ne vaut rien — parce que c'est le même appel
    /// (`Garde::authentifier`) et donc le même code et le même texte.
    ///
    /// Deux raisons, et pas une :
    ///  1. **ne pas rouvrir l'oracle** : une route INCONNUE doit se refuser
    ///     exactement comme une route connue à qui n'a pas d'identité ; s'il
    ///     fallait pour cela recopier le message, la copie finirait par diverger
    ///     et le `401` redeviendrait un énumérateur de routes ;
    ///  2. **ne pas écrire un refus qui n'a pas eu lieu** : appeler `controler`
    ///     pour ne garder que son verdict d'identité journaliserait un « deny »
    ///     de droit sur un espace que la requête n'a jamais visé — la piste
    ///     d'audit doit dire vrai, elle aussi.
    pub(crate) fn identite_ou_refus(&self, jeton: Option<&str>) -> Result<(), (u16, String)> {
        #[cfg(feature = "iam")]
        {
            let Some(garde) = self.interieure.as_ref() else {
                return Ok(());
            };
            match garde.authentifier(jeton) {
                Ok(_) => Ok(()),
                Err(obrain_iam::IamError::AccessDenied { reason }) => Err((401, reason)),
                Err(e) => Err((401, e.to_string())),
            }
        }
        #[cfg(not(feature = "iam"))]
        {
            let _ = jeton;
            Ok(())
        }
    }

    /// ── QUI SUIS-JE ? ────────────────────────────────────────────────────
    pub(crate) fn identite(&self, jeton: &str) -> Option<String> {
        #[cfg(feature = "iam")]
        {
            self.interieure
                .as_ref()
                .and_then(|g| g.identite(jeton).ok().flatten())
        }
        #[cfg(not(feature = "iam"))]
        {
            let _ = jeton;
            None
        }
    }
}

/// Le jeton présenté : `Authorization: Bearer …`, le cookie `obrain_session`
/// (c'est ce qu'un navigateur porte), ou `?token=…` (le flux SSE passe par
/// `EventSource`, qui ne sait pas poser d'en-tête).
pub(crate) fn jeton_presente(entetes: &str, query: Option<&str>) -> Option<String> {
    for ligne in entetes.lines() {
        let l = ligne.trim();
        if l.len() > 14 && l[..14].eq_ignore_ascii_case("authorization:") {
            let v = l[14..].trim();
            return Some(
                v.get(..7)
                    .filter(|p| p.eq_ignore_ascii_case("bearer "))
                    .map_or_else(|| v.to_string(), |_| v[7..].trim().to_string()),
            );
        }
    }
    for ligne in entetes.lines() {
        let l = ligne.trim();
        // `cookie:` fait SEPT caractères (« c o o k i e : ») — la première
        // version en comparait six, donc ne matchait jamais : le cookie de
        // session était posé par la connexion et JAMAIS relu. Défaut trouvé par
        // l'essai de bout en bout, pas par relecture.
        if l.len() > 7 && l[..7].eq_ignore_ascii_case("cookie:") {
            for morceau in l[7..].split(';') {
                if let Some((nom, valeur)) = morceau.split_once('=')
                    && nom.trim() == "obrain_session"
                {
                    let v = valeur.trim();
                    if !v.is_empty() {
                        return Some(v.to_string());
                    }
                }
            }
        }
    }
    parse_query(query)
        .get("token")
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Les espaces d'un appelant, lus dans l'annuaire des identités.
///
/// ── POURQUOI L'ANNUAIRE EST LE SEUL ENDROIT OÙ LES LIRE ─────────────────────
/// Un droit se DÉDUIT de politiques, il ne se déclare pas : une surface qui
/// annoncerait ses propres espaces pourrait s'en inventer. L'annuaire IAM est
/// tenu en écriture par le serveur armé — une requête HTTP ne peut donc pas le
/// consulter par une route — mais le processus, lui, le détient : c'est de là que
/// vient cette lecture.
///
/// Elle ne fait que NOMMER les espaces candidats ; le droit de chacun est
/// ensuite évalué par le moteur de politiques (`Garde::controler_soi`). Nommer
/// n'est pas autoriser.
fn espaces_de_l_appelant(jeton: Option<&str>) -> Result<Vec<String>, (u16, String)> {
    #[cfg(feature = "iam")]
    {
        let garde = GardeIam::partagee();
        if !garde.est_armee() {
            return Ok(Vec::new());
        }
        let Some(jeton) = jeton.filter(|j| !j.trim().is_empty()) else {
            return Err((401, "aucune identité présentée".to_string()));
        };
        let identite = garde
            .identite(jeton)
            .ok_or_else(|| (401, "jeton invalide, révoqué ou expiré".to_string()))?;
        let utilisateur = nom_depuis_identite(&identite);
        let Some(magasin) = garde.identites() else {
            // IAM armé sans annuaire lisible : c'est une incohérence de
            // configuration, et la dire vaut mieux que laisser passer.
            return Err((
                503,
                "annuaire d'identités illisible : impossible de nommer les espaces de l'appelant"
                    .to_string(),
            ));
        };
        let admin = obrain_iam::admin::Administration::plafonnee_par_l_acteur(
            magasin,
            compte_iam(),
            utilisateur,
        )
        .map_err(|e| (403, e.to_string()))?;
        match admin.montrer_utilisateur(admin.acteur()) {
            Ok(resume) => Ok(resume.espaces),
            Err(e) => Err((403, e.to_string())),
        }
    }
    #[cfg(not(feature = "iam"))]
    {
        let _ = jeton;
        Ok(Vec::new())
    }
}

/// Ce texte GQL peut-il MUTER ? Partagée par le serveur HTTP et le pont MCP :
/// la même question doit avoir la même réponse des deux côtés, sinon un outil
/// passerait là où une route est refusée.
pub(crate) fn gql_ecrivante(texte: &str) -> bool {
    let haut = texte.to_uppercase();
    [
        "CREATE", "MERGE", "DELETE", "DETACH", "SET ", "REMOVE", "DROP", "INSERT", "UPDATE",
    ]
    .iter()
    .any(|mot| haut.contains(mot))
}

/// La route peut-elle ÉCRIRE ? Toute méthode autre que GET l'affirme ; et
/// `/api/query` accepte du GQL arbitraire, donc un motif mutant y demande le
/// droit d'écrire. L'heuristique penche vers l'écriture : se tromper en exigeant
/// trop refuse une requête légitime, se tromper en exigeant trop peu laisse
/// écrire sans droit.
pub(crate) fn route_ecrivante(path: &str, methode: &str, query: Option<&str>) -> bool {
    if methode != "GET" && methode != "HEAD" {
        return true;
    }
    if path != "/api/query" {
        return false;
    }
    gql_ecrivante(&parse_query(query).get("query").cloned().unwrap_or_default())
}

/// Le jeton d'ambiance, pour un point d'entrée sans requête HTTP (le pont MCP
/// parle JSON-RPC sur son entrée standard, il n'a pas d'en-tête à lire).
pub(crate) fn jeton_du_pont() -> Option<String> {
    std::env::var("OBRAIN_IAM_TOKEN")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}


/// ── LE SERVEUR SERT-IL CE CHEMIN ? (manque 6, 2026-09-16) ───────────────────
///
/// Défaut mesuré par le donneur d'ordre, et qui lui a coûté du temps : avec
/// l'IAM armé, `GET /api/harness/health`, `GET /api/harness/machine` et une
/// route PUREMENT INEXISTANTE rendaient tous les trois `403` avec le MÊME corps
/// (`permission denied: … tenant/<store>`). « cette route n'existe pas » et
/// « tu n'as pas le droit » ne se corrigent pas de la même façon — les
/// confondre oblige à deviner, et un diagnostic qui fait deviner n'est pas un
/// diagnostic. Même famille que le silence pris pour une panne.
///
/// ── L'ARBITRAGE, ET POURQUOI CETTE FORME-LÀ ────────────────────────────────
/// Dire « inconnue » à qui n'a pas d'identité transformerait ce serveur en
/// ORACLE : un balayage anonyme énumérerait les routes qui existent (404) et
/// celles qui n'existent pas (401). Le refus d'identité garde donc la
/// PRIORITÉ : sans identité valide, une route inconnue répond `401` exactement
/// comme une route connue — rien ne fuit. C'est seulement UNE FOIS l'identité
/// fournie que les deux cas se distinguent (`404 route inconnue` contre
/// `403 droit insuffisant`), et un appelant authentifié a le droit de savoir ce
/// que ce serveur sert : il n'apprend rien sur les DONNÉES d'autrui, seulement
/// la forme de l'API qu'il utilise déjà.
///
/// ── POURQUOI UNE LISTE, ALORS QUE CE FICHIER DÉNONCE LES SECONDES TABLES ───
/// Le `match path` du routeur reste la SEULE autorité sur ce qui est servi ; ce
/// prédicat est un SURENSEMBLE volontaire, et le sens de l'erreur est choisi :
///  * un chemin déclaré connu qui n'existe pas retombe sur le comportement
///    d'avant (jugement d'espace, puis le `_ =>` du routeur) — aucune
///    régression, aucun trou ;
///  * un chemin servi qui manquerait ici répondrait `404` : la route cesse de
///    marcher, BRUYAMMENT, et le premier essai le dit. C'est le seul sens
///    d'erreur acceptable pour une garde : se tromper vers le visible.
///
/// Les deux préfixes sous-routés (`/api/remote/`, `/api/iam/`) sont déclarés
/// connus : leur sous-routeur a son propre repli, qui dit DÉJÀ « route inconnue
/// du canal » ou « route d'administration inconnue ». Les court-circuiter par un
/// 404 générique remplacerait un message précis par un message vague.
fn route_api_connue(path: &str) -> bool {
    if path.starts_with("/api/remote/") || path.starts_with("/api/iam/") {
        return true;
    }
    matches!(
        path,
        "/api/stores"
            | "/api/info"
            | "/api/query"
            | "/api/functions"
            | "/api/entretien"
            | "/api/worlds"
            | "/api/worlds/status"
            | "/api/viz/status"
            | "/api/viz/subgraph"
            | "/api/viz/subgraph/stream"
            | "/api/viz/overview"
            | "/api/viz/modules"
            | "/api/viz/signatures"
            | "/api/viz/arcs"
            | "/api/viz/ramifications"
            | "/api/viz/space"
            | "/api/viz/weave"
            | "/api/viz/activation"
            | "/api/engine/sante"
            | "/api/engine/memoire"
            | "/api/engine/selfs"
            | "/api/engine/resonance"
            | "/api/engine/ask"
            | "/api/engine/chat"
            | "/api/auth/login"
            | "/api/auth/logout"
            | "/api/auth/whoami"
            | "/api/auth/refresh"
            | "/api/auth/audit"
    )
}

/// ── LA BASE PROPRE DU COMPTE (défaillance 5.2, 2026-09-16) ─────────────────
///
/// Le store visé est-il LA base que le dépôt donne à ce compte ?
///
/// Défaut mesuré (`docs/DEVELOPPEMENT-SELF-PAR-COMPTE-2026-09-16.md` §5.2) : la
/// ressource jugée par la politique était le TENANT, dont le nom est tiré du
/// **fichier du store** (`orn:…:tenant/runtime-essai`), alors que le compte
/// reçoit des droits sur son **espace** (`esp-…`). Conséquence directe sur
/// l'objectif « chaque utilisateur a un agent sur SA base » : une base
/// `selfs/<compte>.obrain` n'était pas lisible par son propre compte dès que son
/// espace ne portait pas le nom du fichier — et le refus parlait d'un espace que
/// la requête n'avait jamais visé.
///
/// ── POURQUOI LE DOSSIER VIENT DU SERVEUR, ET LE NOM DE L'IDENTITÉ ──────────
/// La reconnaissance ne doit pas être un paramètre de confort : si l'appelant
/// pouvait déclarer « cette base est la mienne », il lirait n'importe quoi. Le
/// dossier est donc celui du **store servi** (le serveur), et le nom de fichier
/// vient de l'**identité authentifiée** (le jeton). Exactement la convention que
/// le dépôt applique déjà pour lire la mémoire d'un compte :
/// `dirname(store)/selfs/<compte>.obrain` (`engine.rs::profil_du_self`, et la
/// passe `machines::appliquer` qui écrit dans ces mêmes bases). Un compte ne
/// peut donc ouvrir par cette règle que le fichier que le produit lui destine.
///
/// Les deux chemins sont comparés RÉSOLUS (`canonicalize`) : un chemin qui
/// n'existe pas n'est pas ouvrable, donc rien à accorder — le refus reste, avec
/// son motif. Un lien symbolique posé à cet emplacement est suivi comme le
/// serveur le suivrait lui-même en lisant cette base : la règle n'ouvre pas un
/// droit de plus que ce que le produit considère déjà comme « sa base ».
fn vise_sa_propre_base(config: &ServeConfig, query: Option<&str>, identite: Option<&str>) -> bool {
    let Some(identite) = identite.map(str::trim).filter(|i| !i.is_empty()) else {
        return false;
    };
    let utilisateur = nom_depuis_identite(identite);
    if utilisateur.is_empty() {
        return false;
    }
    let Some(dossier) = config.default_path().and_then(|p| p.parent().map(std::path::Path::to_path_buf))
    else {
        return false;
    };
    let attendu = dossier.join("selfs").join(format!("{utilisateur}.obrain"));
    let Some(vise) = config.resolve(parse_query(query).get("store").map(String::as_str)) else {
        return false;
    };
    match (std::fs::canonicalize(&attendu), std::fs::canonicalize(&vise)) {
        (Ok(a), Ok(v)) => a == v,
        _ => false,
    }
}

/// L'espace visé : le nom de la base interrogée, pour qu'une politique puisse
/// autoriser une base sans autoriser les autres.
pub(crate) fn espace_vise(config: &ServeConfig, query: Option<&str>) -> String {
    let params = parse_query(query);
    config
        .resolve(params.get("store").map(String::as_str))
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "base-inconnue".to_string())
}

/// L'action qui garde l'administration des comptes.
///
/// Lue depuis la bibliothèque quand elle est compilée — pour qu'une constante
/// renommée ailleurs ne puisse pas désarmer cette garde en silence.
#[cfg(feature = "iam")]
const ACTION_ADMIN_HTTP: &str = obrain_iam::admin::ACTION_ADMIN;
/// Repli sans la feature : la route existe et doit s'annoncer incompilée, donc
/// la garde a besoin d'un nom même quand la chaîne n'est pas embarquée.
#[cfg(not(feature = "iam"))]
const ACTION_ADMIN_HTTP: &str = "iam:admin";

/// L'action de LECTURE, du même nom que celui que la garde applique à une
/// requête de lecture (`OBRAIN_IAM_ACTION_READ`, `graph:read` par défaut).
///
/// Sert à une chose précise : décider quelles BASES annoncer à l'appelant. Une
/// route qui liste doit appliquer exactement la décision que la garde
/// appliquera ensuite — sinon elle annonce des bases qu'elle refusera.
const ACTION_LECTURE_HTTP: &str = "graph:read";

/// La garde d'identités SOUS-JACENTE, quand l'IAM est armé et lisible.
fn garde_interieure() -> Option<&'static std::sync::Arc<obrain_iam::Garde>> {
    #[cfg(feature = "iam")]
    {
        GardeIam::partagee().interieure()
    }
    #[cfg(not(feature = "iam"))]
    {
        None
    }
}


/// L'espace d'une requête d'ADMINISTRATION : `?espace=` ou `espace=` du corps.
///
/// Rend `None` quand aucun espace n'est NOMMÉ. C'est un refus, pas un défaut :
/// voir l'appelant (`Attendu : un espace nommé`).
///
/// ── POURQUOI ELLE EXISTE, ET PAS UN CHANGEMENT D'`espace_vise` ──────────────
/// Faire gagner `?espace=` dans `espace_vise` aurait été plus court — et FAUX.
/// Cette fonction sert aussi à `/api/query`, où un client pourrait alors
/// **choisir l'espace sur lequel on le juge** : `?espace=e1` en interrogeant la
/// base `e2` le ferait juger sur un espace qu'il détient pour en lire un autre.
/// Un espace nommé ne gagne donc que sur les routes d'administration, où il
/// désigne la CIBLE du geste et non un paramètre de confort.
///
/// Le CORPS compte autant que l'URL : un formulaire HTML poste son contenu, et
/// ne juger que l'URL ferait trancher la garde sur l'espace par défaut du
/// serveur — un espace que la requête ne vise pas.
fn espace_iam(query: Option<&str>, corps: &str) -> Option<String> {
    espaces_demandes(query, corps).into_iter().next()
}

/// Toutes les valeurs de `espace`, dans l'ordre, URL et corps confondus.
///
/// `parse_query` rend une carte : un paramètre RÉPÉTÉ y perd tout sauf le
/// dernier. Or un compte peut recevoir plusieurs espaces d'un seul geste
/// (`espace=demo&espace=autre`), et n'en garder qu'un serait une perte
/// silencieuse — l'administrateur croirait avoir tout accordé.
fn espaces_demandes(query: Option<&str>, corps: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for source in [query.unwrap_or(""), corps] {
        for pair in source.split('&') {
            let Some((cle, valeur)) = pair.split_once('=') else {
                continue;
            };
            if percent_decode(cle).trim() != "espace" {
                continue;
            }
            let valeur = percent_decode(valeur).trim().to_string();
            if !valeur.is_empty() && !out.contains(&valeur) {
                out.push(valeur);
            }
        }
    }
    out
}

/// Le compte (locataire) porteur des identités — le même que celui de la garde.
fn compte_iam() -> String {
    std::env::var("OBRAIN_IAM_ACCOUNT")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "obrain".to_string())
}

/// Le nom d'utilisateur, extrait de l'identité rendue par la garde.
///
/// `Garde::identite` rend `utilisateur@compte` (le `Display` d'`Identity`). On
/// ne garde que le nom, que le magasin sait résoudre — la coupe est sûre parce
/// que la grammaire des noms interdit l'arobase.
fn nom_depuis_identite(identite: &str) -> String {
    identite
        .split('@')
        .next()
        .unwrap_or(identite)
        .trim()
        .to_string()
}

// ═══════════════════════════════════════════════════════════════════════════
// LA COUCHE TRANSPORT — entre la socket et les routes
// ═══════════════════════════════════════════════════════════════════════════
//
// Réécrite le 2026-09-13 après avoir MESURÉ, sur le service vivant, ce que cette
// couche coûtait. Chaque défaut a son relevé, et chaque correction dit POURQUOI
// elle est faite ainsi — pas seulement ce qu'elle fait.
//
//   * `Connection: close` écrit en dur sur CHAQUE réponse : un socket par
//     requête. Relevé : `curl -sv URL1 URL2` → DEUX « Connected », et des
//     dizaines de sockets en TIME-WAIT sur le port pendant que la vue sondait
//     l'activation toutes les 1,2 s. Ce qui coûte n'est pas le socket, c'est la
//     poignée de main qu'on refait à chaque fois.
//   * AUCUN délai sur la socket : un client qui ouvre et se tait gardait son fil
//     ET son descripteur indéfiniment. Relevé : 50 sockets muets = 50 fils et
//     50 descripteurs de plus, rendus seulement parce que le client est parti de
//     lui-même. C'est le prérequis du keep-alive : garder la connexion ouverte
//     sans délai de lecture multiplierait ce risque au lieu de le réduire.
//   * aucune compression alors que le client annonçait `gzip, deflate, br,
//     zstd` : 1 286 462 octets de bundle pour un contenu qui en pèse 377 771.
//   * `HEAD` refusé (405, et un corps de 18 octets sur une méthode qui n'en veut
//     pas), `Date` absent, aucun `ETag` : trois obligations de la RFC 9110 et
//     trois symptômes — `curl -I` en échec, réponse non datable par un cache, et
//     le bundle renvoyé en entier à chaque revalidation.
//   * une seule lecture de 64 Kio, jamais relue : deux requêtes pipelinées dans
//     le même paquet, et la seconde perdue.
//
// ── POURQUOI UN ÉTAT PAR FIL, ET NON UN PARAMÈTRE ─────────────────────────
// `respond*` est appelé par une trentaine de gestionnaires `api_*` : leur faire
// porter la négociation de contenu demanderait de changer leur signature à tous,
// et un gestionnaire de route n'a pas à savoir ce que le CLIENT accepte de
// dégonfler. Or un fil sert UNE connexion de bout en bout — donc le fil EST le
// contexte. Le contrat est explicite : `poser_etat_connexion` est appelé AVANT
// le routage, `respond_octets` le lit, et la boucle de connexion le REMET À ZÉRO
// à chaque tour — sans quoi l'état d'un `HEAD` (pas de corps !) fuiterait sur la
// réponse suivante de la même connexion, et le client attendrait un corps qui ne
// viendrait jamais.

/// Combien de requêtes une même connexion peut servir avant qu'on la rende.
/// Borné : un client qui tourne en boucle sur une connexion n'occupe pas un fil
/// pour toujours.
const MAX_REQUETES: u32 = 100;
/// Inactivité tolérée entre deux requêtes d'une même connexion.
const INACTIVITE_MAX: Duration = Duration::from_secs(15);
/// Délai de LECTURE de la première requête : plus large que l'inactivité, parce
/// qu'un client qui ouvre sa connexion et réfléchit encore n'est pas un client
/// mort.
const DELAI_LECTURE: Duration = Duration::from_secs(30);
/// Délai d'ÉCRITURE : large, et volontairement — une écriture SSE peut être
/// longue (le moteur réfléchit parfois 60 s avant le premier jeton), et une
/// socket fermée trop tôt perdrait une réponse en cours. Il n'est là que pour
/// qu'un client qui ne lit plus ne fige pas le fil indéfiniment.
const DELAI_ECRITURE: Duration = Duration::from_secs(120);
/// Plafond du bloc d'en-têtes (au-delà : 431).
const MAX_ENTETES: usize = 16 * 1024;
/// Plafond de la cible (au-delà : 414).
const MAX_CIBLE: usize = 8 * 1024;
/// Plafond du corps d'une requête (au-delà : 413, refusé AVANT lecture).
const MAX_CORPS: usize = 1024 * 1024;
/// En dessous, dégonfler coûte plus cher que ça ne rapporte (l'en-tête gzip pèse
/// une vingtaine d'octets, et un corps de 200 octets ne se comprime pas).
const SEUIL_COMPRESSION: usize = 512;
/// Battement de cœur SSE : bien plus court que les 60 s de patience des proxys
/// et que le délai de lecture d'un client intermédiaire.
const BATTEMENT_SSE: Duration = Duration::from_secs(15);
/// Délai de reconnexion annoncé au client SSE (`retry:`), en millisecondes.
const RETRY_SSE_MS: u64 = 2000;
const TYPE_SSE: &str = "text/event-stream; charset=utf-8";

/// La phrase de raison d'un code. Complétée le 2026-09-13 : la table n'en
/// connaissait que six, et TOUT le reste s'annonçait `"Error"` — un journal qui
/// dit « 413 Error » n'apprend rien à qui le lit.
///
/// Le repli n'est plus `"Error"` mais `"Statut inconnu"`, et un test vérifie que
/// chaque code réellement émis par ce fichier a bien sa phrase : ajouter un code
/// sans sa raison fera échouer `tous_les_codes_emis_ont_une_raison`.
fn raison_http(code: u16) -> &'static str {
    match code {
        200 => "OK",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        411 => "Length Required",
        413 => "Payload Too Large",
        414 => "URI Too Long",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        505 => "HTTP Version Not Supported",
        _ => "Statut inconnu",
    }
}

/// Ce que la réponse hérite de la REQUÊTE, sur ce fil.
#[derive(Clone, Debug)]
struct EtatConnexion {
    /// Méthode `HEAD` : les en-têtes partent, le corps NON.
    head: bool,
    /// La connexion survit-elle à cette réponse ?
    keep_alive: bool,
    /// `Accept-Encoding` brut, gardé tel quel pour que la négociation (et sa
    /// lecture des `q=`) soit une fonction PURE, donc vérifiable sans socket.
    accepte_encodage: Option<String>,
    /// `If-None-Match` : c'est lui qui permet de répondre 304 sur un fichier.
    if_none_match: Option<String>,
    /// La réponse a EXIGÉ la fermeture (flux SSE, ou erreur de trame).
    fermer: bool,
    /// Une réponse a-t-elle déjà été écrite ? Sert à ne pas empiler un 500
    /// derrière une réponse déjà partie quand un gestionnaire échoue en route.
    repondu: bool,
}

impl EtatConnexion {
    /// L'état d'une connexion qui n'a encore rien dit : ferme, sans corps à
    /// écrire, sans négociation. C'est aussi l'état des erreurs de trame, qui
    /// naissent AVANT qu'une requête soit identifiée (d'où `keep_alive: false` :
    /// on ne réutilise pas une connexion dont on n'a pas su lire une requête).
    const fn vierge() -> Self {
        Self {
            head: false,
            keep_alive: false,
            accepte_encodage: None,
            if_none_match: None,
            fermer: false,
            repondu: false,
        }
    }
}

std::thread_local! {
    /// L'état de la connexion EN COURS sur ce fil (voir le commentaire de tête).
    static CONNEXION: std::cell::RefCell<EtatConnexion> =
        std::cell::RefCell::new(EtatConnexion::vierge());
}

/// Pose l'état de la requête en cours. Appelé une fois par requête, avant tout
/// routage : aucun gestionnaire ne s'en aperçoit autrement que par la réponse.
fn poser_etat_connexion(etat: EtatConnexion) {
    CONNEXION.with(|c| *c.borrow_mut() = etat);
}

/// Lit (et éventuellement modifie) l'état de la connexion.
///
/// L'emprunt est relâché AVANT d'écrire sur la socket : sans cela, l'écriture
/// d'une réponse se ferait sous l'emprunt du `RefCell`, et le premier appel
/// imbriqué ferait PANIQUER le fil — c'est-à-dire le client, sans un octet
/// d'explication (défaut déjà vécu dans ce fichier, voir `percent_decode`).
fn etat_connexion<R>(f: impl FnOnce(&mut EtatConnexion) -> R) -> R {
    CONNEXION.with(|c| f(&mut c.borrow_mut()))
}

/// ── LA NÉGOCIATION DE CONTENU, EN FONCTION PURE ────────────────────────────
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Encodage {
    Aucun,
    Gzip,
    Deflate,
}

/// Choisit l'encodage de la réponse.
///
/// `text/event-stream` en est EXCLU, et pas par prudence : un déflateur travaille
/// par BLOCS et garde les derniers octets en attendant de quoi remplir le bloc
/// courant. Le premier mot du modèle n'arriverait donc qu'à la fin de la
/// réponse — exactement ce que le SSE existe pour éviter. Un `Content-Encoding`
/// sur un flux, c'est un flux mort.
///
/// Les `q=` sont lus : `gzip;q=0` est un REFUS, pas une acceptation (un client
/// qui a mesuré que gzip le ralentit l'écrit ainsi, et le lui imposer serait
/// ignorer ce qu'il dit).
fn encodage_negocie(accepte: Option<&str>, type_mime: &str, taille: usize) -> Encodage {
    if type_mime.starts_with("text/event-stream") {
        return Encodage::Aucun;
    }
    if taille < SEUIL_COMPRESSION {
        return Encodage::Aucun;
    }
    let Some(accepte) = accepte else {
        return Encodage::Aucun;
    };
    let (mut gzip, mut deflate) = (false, false);
    for item in accepte.split(',') {
        let mut morceaux = item.split(';');
        let nom = morceaux.next().unwrap_or("").trim().to_ascii_lowercase();
        let q = morceaux
            .find_map(|p| {
                let p = p.trim();
                p.strip_prefix("q=").or_else(|| p.strip_prefix("Q=")).map(str::trim)
            })
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(1.0);
        if q <= 0.0 {
            continue;
        }
        match nom.as_str() {
            "gzip" | "x-gzip" => gzip = true,
            "deflate" => deflate = true,
            // `*` = « tout ce que tu veux » : on prend le mieux défendu des deux,
            // et c'est gzip (le `deflate` d'HTTP est ambigu — zlib ou brut selon
            // les serveurs, voir RFC 9110 §8.4.1.2).
            "*" => gzip = true,
            _ => {}
        }
    }
    if gzip {
        Encodage::Gzip
    } else if deflate {
        Encodage::Deflate
    } else {
        Encodage::Aucun
    }
}

/// Dégonfle un corps. Gzip ET deflate, parce que les deux sont annoncés par les
/// clients et que le second coûte trois lignes de plus — mais `deflate` sort au
/// format **zlib** (RFC 1950), qui est celui que la RFC 9110 nomme, et non du
/// deflate brut (RFC 1951) que certains serveurs envoient par erreur.
fn compresser(corps: &[u8], encodage: Encodage) -> std::io::Result<Vec<u8>> {
    use flate2::write::{GzEncoder, ZlibEncoder};
    use flate2::Compression;
    // Niveau 6 = le défaut de zlib, et celui des mesures : 1 286 462 → 377 771
    // octets sur le bundle, pour 30-40 ms de processeur, une fois par visite.
    let niveau = Compression::new(6);
    match encodage {
        Encodage::Gzip => {
            let mut e = GzEncoder::new(Vec::with_capacity(corps.len() / 3 + 64), niveau);
            e.write_all(corps)?;
            e.finish()
        }
        Encodage::Deflate => {
            let mut e = ZlibEncoder::new(Vec::with_capacity(corps.len() / 3 + 64), niveau);
            e.write_all(corps)?;
            e.finish()
        }
        Encodage::Aucun => Ok(corps.to_vec()),
    }
}

/// ── L'ETAG, DÉRIVÉ DE (LONGUEUR, DATE) — ET POURQUOI C'EST SUFFISANT ───────
/// Une empreinte du contenu coûterait une lecture complète et un hachage à
/// chaque requête : hacher 1,29 Mo pour éviter d'envoyer 1,29 Mo, c'est payer le
/// mal pour son remède. La date de modification et la longueur suffisent : sur un
/// système de fichiers, un contenu qui change change l'une des deux (Vite écrit
/// un fichier NEUF, il ne le retouche pas octet à octet). La limite est connue et
/// assumée : un fichier réécrit à l'identique en taille ET en date à la
/// nanoseconde près ne serait pas détecté — et le bundle porte de toute façon son
/// hachage dans son NOM.
fn etag_de(taille: u64, mtime: Option<SystemTime>) -> String {
    let ns = mtime
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("\"{taille:x}-{ns:x}\"")
}

/// `If-None-Match` couvre-t-il notre `ETag` ?
///
/// Comparaison FAIBLE (`W/`) assumée : `If-None-Match` utilise la comparaison
/// faible par définition (RFC 9110 §13.1.2), et un `W/"x"` du client doit donc
/// revalider un `"x"` du serveur. `*` couvre tout.
fn etag_correspond(demande: &str, etag: &str) -> bool {
    demande.split(',').any(|v| {
        let v = v.trim();
        v == "*" || v.strip_prefix("W/").unwrap_or(v) == etag
    })
}

/// Faut-il répondre 304 ? Fonction PURE, pour que la décision soit vérifiable
/// sans socket (voir `la_revalidation_304_*` dans les tests).
fn revalidation_304(code: u16, etag: Option<&str>, if_none_match: Option<&str>) -> bool {
    matches!((code, etag, if_none_match), (200, Some(et), Some(demande)) if etag_correspond(demande, et))
}

/// ── `Date`, OBLIGATOIRE (RFC 9110 §6.6.1) ──────────────────────────────────
/// Absent, il laissait un cache sans repère : ni âge, ni fraîcheur. Écrit ici
/// plutôt qu'en tirant une dépendance de dates, parce que la conversion inverse
/// (jours depuis l'époque → année/mois/jour) tient en dix lignes d'arithmétique
/// entière — et parce que ce serveur est « std-only » par choix.
fn date_http() -> String {
    date_http_a(SystemTime::now())
}

/// Format IMF-fixdate : `Sun, 13 Sep 2026 16:23:19 GMT` (RFC 9110 §5.6.7).
fn date_http_a(t: SystemTime) -> String {
    const JOURS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MOIS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let jours = secs.div_euclid(86_400);
    let reste = secs.rem_euclid(86_400);
    let (annee, mois, jour) = civil_depuis_jours(jours);
    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        // L'époque (jour 0) est un JEUDI : l'index 0 de `JOURS` est donc `Thu`,
        // et non `Mon` — l'erreur la plus facile à commettre ici, et la seule
        // qui produirait des dates valides mais fausses d'un jour.
        JOURS[(jours.rem_euclid(7)) as usize],
        jour,
        MOIS[(mois - 1) as usize],
        annee,
        reste / 3600,
        (reste % 3600) / 60,
        reste % 60
    )
}

/// Jour julien → (année, mois, jour), algorithme de Howard Hinnant. Aucune table,
/// aucune exception de calendrier à retenir : les années bissextiles y sont une
/// conséquence de l'arithmétique (`yoe/4 - yoe/100`), pas une liste.
fn civil_depuis_jours(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let ere = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - ere * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + ere * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11], mars = 0
    let j = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, j)
}

/// ── LA LECTURE D'UNE REQUÊTE ───────────────────────────────────────────────
/// Ce qui peut mal tourner, et ce que le client en apprend :
#[derive(Debug)]
enum ErreurLecture {
    /// Rien reçu, client parti sans rien dire : aucune réponse à écrire.
    Fin,
    /// Rien reçu dans le délai : le client n'a jamais parlé.
    Silence,
    /// Des octets reçus, mais pas de requête complète : 400, et on le DIT —
    /// l'ancien code rendait `Ok(())` sans rien écrire, donc le client pendait
    /// jusqu'à son propre délai sans savoir pourquoi.
    Partielle(String),
    /// Refus net, avec son code : 400, 413, 414, 431, 501, 505.
    Refus(u16, String),
}

/// Résultat d'une tentative d'extraction sur un tampon : soit une requête, soit
/// « il manque des octets ». Pas d'autre cas — c'est ce qui rend la lecture
/// reprenable, et donc le pipelining possible.
#[derive(Debug)]
enum Extraction {
    Prete(Box<Requete>),
    Incomplete,
}

/// Une requête HTTP analysée.
#[derive(Debug)]
struct Requete {
    methode: String,
    /// Cible brute (`/a?b=c`), gardée pour les journaux et les messages d'erreur.
    cible: String,
    /// Chemin SEUL, non décodé : `/app/%2e%2e/x` reste tel quel jusqu'à
    /// `resoudre_sous`, qui décode ET vérifie. Décoder ici ferait entrer un `..`
    /// dans le routeur, c'est-à-dire dans le `match` qui choisit la route.
    chemin: String,
    /// Chaîne de requête sans le `?`.
    requete: Option<String>,
    /// Le bloc d'en-têtes BRUT (ligne de requête comprise), tel que
    /// `jeton_presente` le lit — sa signature prend du texte, pas une carte.
    entetes_brut: String,
    /// Les en-têtes analysés, noms en minuscules.
    entetes: Vec<(String, String)>,
    /// `true` pour HTTP/1.1 : décide du keep-alive par défaut et de `Host`.
    http11: bool,
    corps: Vec<u8>,
}

impl Requete {
    fn entete(&self, nom: &str) -> Option<&str> {
        self.entetes.iter().find(|(n, _)| n == nom).map(|(_, v)| v.as_str())
    }
}

/// Position d'une aiguille dans une meule.
fn trouver(meule: &[u8], aiguille: &[u8]) -> Option<usize> {
    if aiguille.is_empty() || meule.len() < aiguille.len() {
        return None;
    }
    meule.windows(aiguille.len()).position(|f| f == aiguille)
}

fn est_delai(e: &std::io::Error) -> bool {
    matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)
}

/// Extrait UNE requête du tampon, sans toucher à la socket.
///
/// Rend `Incomplete` en laissant le tampon INTACT quand il manque des octets :
/// c'est cette propriété qui rend la lecture reprenable, et donc le pipelining
/// correct — la deuxième requête d'un paquet, et le corps d'un `POST` arrivé en
/// deux morceaux, sont le même problème.
fn extraire_requete(tampon: &mut Vec<u8>) -> Result<Extraction, ErreurLecture> {
    let Some(fin) = trouver(tampon, b"\r\n\r\n") else {
        if tampon.len() > MAX_ENTETES {
            return Err(ErreurLecture::Refus(431, format!("en-têtes au-delà de {MAX_ENTETES} octets")));
        }
        return Ok(Extraction::Incomplete);
    };
    // Le plafond vaut aussi quand le terminateur EST là : un client peut envoyer
    // 1 Mo d'en-têtes d'un seul coup, et le tampon les aurait acceptés.
    if fin > MAX_ENTETES {
        return Err(ErreurLecture::Refus(431, format!("bloc d'en-têtes de {fin} octets")));
    }
    let bloc = String::from_utf8_lossy(&tampon[..fin]).into_owned();

    // ── LA LIGNE DE REQUÊTE ────────────────────────────────────────────────
    // `split(' ')` et non `split_whitespace` : une tabulation dans la ligne de
    // requête n'est pas un séparateur (RFC 9112 §3), et l'accepter serait
    // accepter deux lectures différentes de la même requête.
    let mut lignes = bloc.split("\r\n");
    let ligne = lignes.next().unwrap_or("");
    let champs: Vec<&str> = ligne.split(' ').collect();
    if champs.len() != 3 || champs.iter().any(|c| c.is_empty()) {
        // Couvre la LIGNE VIDE (un client qui envoie `\r\n\r\n` d'entrée) : ses
        // défauts, eux, sont refusés et non devinés.
        return Err(ErreurLecture::Refus(
            400,
            format!("ligne de requête illisible : « {} »", tronque(ligne, 80)),
        ));
    }
    let (methode, cible, version) = (champs[0], champs[1], champs[2]);
    if ligne.len() > MAX_CIBLE {
        return Err(ErreurLecture::Refus(414, format!("cible de {} octets", ligne.len())));
    }
    if !methode.bytes().all(|c| c.is_ascii_uppercase() || c == b'-') {
        return Err(ErreurLecture::Refus(400, format!("méthode invalide : « {} »", tronque(methode, 32))));
    }
    let http11 = match version {
        "HTTP/1.1" => true,
        "HTTP/1.0" => false,
        v if v.starts_with("HTTP/") => {
            return Err(ErreurLecture::Refus(505, format!("version « {v} » non servie (1.0 et 1.1 le sont)")));
        }
        v => return Err(ErreurLecture::Refus(400, format!("version illisible : « {} »", tronque(v, 32)))),
    };

    // ── LES EN-TÊTES ───────────────────────────────────────────────────────
    let mut entetes: Vec<(String, String)> = Vec::new();
    for l in lignes {
        if l.is_empty() {
            continue;
        }
        let Some((nom, valeur)) = l.split_once(':') else {
            return Err(ErreurLecture::Refus(400, format!("en-tête sans deux-points : « {} »", tronque(l, 60))));
        };
        // Un nom d'en-tête est un `token` (RFC 9110 §5.6.2) : ni espace, ni
        // caractère hors ASCII. Un espace avant les deux-points est refusé par la
        // RFC, et c'est aussi la technique classique de désynchronisation entre
        // deux analyseurs qui ne lisent pas la même chose.
        if nom.is_empty() || !nom.bytes().all(|c| c.is_ascii_graphic() && c != b' ') {
            return Err(ErreurLecture::Refus(400, format!("nom d'en-tête invalide : « {} »", tronque(nom, 40))));
        }
        let valeur = valeur.trim();
        if valeur.len() > 8192 {
            return Err(ErreurLecture::Refus(431, format!("valeur de « {nom} » : {} octets", valeur.len())));
        }
        entetes.push((nom.to_ascii_lowercase(), valeur.to_string()));
    }

    // `Host` OBLIGATOIRE en 1.1 (RFC 9112 §3.2). Mesuré avant : un HTTP/1.1 sans
    // `Host` recevait 200 — deux clients pouvaient donc viser deux ressources
    // différentes avec la même requête, et ce serveur répondait aux deux.
    // En 1.0 il reste FACULTATIF : l'exiger casserait des clients légitimes.
    let hote = entetes.iter().find(|(n, _)| n == "host").map(|(_, v)| v.as_str()).unwrap_or("");
    if http11 && hote.is_empty() {
        return Err(ErreurLecture::Refus(400, "HTTP/1.1 sans en-tête Host".to_string()));
    }

    // ── LE CORPS : LONGUEUR D'ABORD, ET SANS AMBIGUÏTÉ ─────────────────────
    if entetes.iter().any(|(n, _)| n == "transfer-encoding") {
        return Err(ErreurLecture::Refus(
            501,
            "Transfer-Encoding non pris en charge (ce serveur ne lit pas les corps par blocs)".to_string(),
        ));
    }
    let longueurs: Vec<&str> =
        entetes.iter().filter(|(n, _)| n == "content-length").map(|(_, v)| v.as_str()).collect();
    let longueur = match longueurs.as_slice() {
        [] => 0,
        [une] => match une.trim().parse::<usize>() {
            Ok(n) => n,
            Err(_) => {
                return Err(ErreurLecture::Refus(400, format!("Content-Length illisible : « {} »", tronque(une, 32))));
            }
        },
        // DEUX `Content-Length` : on refuse au lieu de choisir. C'est la signature
        // d'une désynchronisation de requête (deux analyseurs, deux longueurs), et
        // « choisir » revient à décider lequel des deux ment.
        _ => return Err(ErreurLecture::Refus(400, "Content-Length répété".to_string())),
    };
    if longueur > MAX_CORPS {
        // Refusé AVANT de lire : mesuré, un `Content-Length` menteur de 10 Mo
        // faisait pendre le client 30 s (l'ancien code lisait le corps en boucle
        // sans plafond ni délai).
        return Err(ErreurLecture::Refus(
            413,
            format!("corps de {longueur} octets au-delà du plafond de {MAX_CORPS}"),
        ));
    }
    if tampon.len() < fin + 4 + longueur {
        return Ok(Extraction::Incomplete);
    }

    // ── LES EMPRUNTS SONT MATÉRIALISÉS AVANT LE DÉPLACEMENT DE `bloc` ───────
    // `methode` et `cible` empruntent le texte du bloc d'en-têtes ; or
    // `entetes_brut` emporte ce texte (déplacement). Les garder empruntés tout
    // en déplaçant `bloc` ne compile pas (E0505), et cloner le bloc entier pour
    // ça serait payer une copie par requête pour deux chaînes de quelques
    // octets. On prend donc les deux qui servent, on déplace le bloc, et le
    // reste lit le texte par `entetes_brut`.
    let methode = methode.to_string();
    let cible = cible.to_string();
    let entetes_brut = bloc;
    tampon.drain(..fin + 4);
    let corps: Vec<u8> = tampon.drain(..longueur).collect();
    let (chemin, requete) = match cible.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (cible.to_string(), None),
    };
    Ok(Extraction::Prete(Box::new(Requete {
        methode,
        cible,
        chemin,
        requete,
        entetes_brut,
        entetes,
        http11,
        corps,
    })))
}

/// Un extrait de texte pour un message d'erreur — jamais le tout : un journal qui
/// recopie une requête entière est un journal qu'on cesse de lire. La coupe
/// respecte les frontières de caractères (défaut déjà corrigé ailleurs dans ce
/// fichier : une coupe en octets sur une `&str` fait PANIQUER).
fn tronque(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut fin = max;
    while fin > 0 && !s.is_char_boundary(fin) {
        fin -= 1;
    }
    format!("{}…", &s[..fin])
}

/// Le lecteur d'UNE connexion, et son tampon RÉSIDUEL.
///
/// L'ancienne lecture prenait 64 Kio UNE fois, puis ne regardait plus jamais ce
/// qui restait : deux requêtes pipelinées dans le même paquet, et la seconde
/// était perdue. Ici, ce qui n'a pas été consommé reste dans le tampon et la
/// requête suivante commence exactement là où la précédente s'est arrêtée.
struct Lecteur {
    flux: TcpStream,
    tampon: Vec<u8>,
}

impl Lecteur {
    fn nouveau(flux: TcpStream) -> Self {
        Self { flux, tampon: Vec::with_capacity(8192) }
    }

    fn delai_lecture(&mut self, d: Duration) {
        let _ = self.flux.set_read_timeout(Some(d));
    }

    /// Lit un morceau de plus. `Ok(0)` = le client a fermé.
    fn remplir(&mut self) -> std::io::Result<usize> {
        let mut bloc = [0u8; 16384];
        let n = self.flux.read(&mut bloc)?;
        if n > 0 {
            self.tampon.extend_from_slice(&bloc[..n]);
        }
        Ok(n)
    }

    /// Lit jusqu'à avoir une requête complète, ou dit pourquoi il n'y en a pas.
    fn lire_requete(&mut self) -> Result<Requete, ErreurLecture> {
        loop {
            match extraire_requete(&mut self.tampon)? {
                Extraction::Prete(r) => return Ok(*r),
                Extraction::Incomplete => {}
            }
            match self.remplir() {
                Ok(0) => {
                    return Err(if self.tampon.is_empty() {
                        // Le client a ouvert puis fermé sans parler : il n'attend
                        // rien, il n'y a rien à lui répondre.
                        ErreurLecture::Fin
                    } else {
                        ErreurLecture::Partielle("le client a fermé avant la fin de sa requête".to_string())
                    });
                }
                Ok(_) => {}
                Err(e) if est_delai(&e) => {
                    return Err(if self.tampon.is_empty() {
                        ErreurLecture::Silence
                    } else {
                        ErreurLecture::Partielle("délai dépassé en cours de requête".to_string())
                    });
                }
                Err(e) => return Err(ErreurLecture::Partielle(format!("lecture : {e}"))),
            }
        }
    }
}

/// L'état que la réponse héritera de la requête. Fonction PURE (elle ne touche
/// qu'à la requête), donc la règle de keep-alive est vérifiable sans socket.
fn etat_du_client(req: &Requete) -> EtatConnexion {
    let connexion = req.entete("connection").unwrap_or("");
    let jetons: Vec<String> = connexion.split(',').map(|t| t.trim().to_ascii_lowercase()).collect();
    let fermeture_demandee = jetons.iter().any(|t| t == "close");
    let garde_demandee = jetons.iter().any(|t| t == "keep-alive");
    // HTTP/1.1 garde la connexion PAR DÉFAUT ; 1.0 seulement si le client le
    // demande (RFC 9112 §9.3). Inverser cette règle ferait garder ouverte une
    // connexion qu'un client 1.0 croit fermée — il attendrait la fin d'un corps
    // déjà complet, et pendrait jusqu'à son propre délai.
    let keep_alive = !fermeture_demandee && (req.http11 || garde_demandee);
    EtatConnexion {
        head: req.methode.eq_ignore_ascii_case("HEAD"),
        keep_alive,
        accepte_encodage: req.entete("accept-encoding").map(str::to_string),
        if_none_match: req.entete("if-none-match").map(str::to_string),
        fermer: false,
        repondu: false,
    }
}

/// Le garde-méthode des chemins qui n'existent QUE en `POST` : ils attendent un
/// corps, donc un `GET` ou un `HEAD` dessus n'a rien à lire. Rend l'en-tête
/// `Allow` quand la méthode est refusée.
///
/// Pourquoi seulement ceux-là : ce sont les seuls chemins du routeur dont
/// l'armement exige un `POST` (`/api/auth/login`, `/api/auth/logout`,
/// `/api/auth/refresh`) ; partout ailleurs, `GET` et `HEAD` mènent quelque part.
/// Un tableau « chemin → méthodes » complet aurait été une SECONDE table de
/// routage à tenir en accord avec le `match` — le défaut que ce fichier
/// documente déjà trois fois (deux cartes de handles, deux règles de slug, deux
/// sources pour une même vérité).
fn methode_refusee(chemin: &str, methode: &str) -> Option<&'static str> {
    match chemin {
        "/api/auth/login" | "/api/auth/logout" | "/api/auth/refresh" if methode != "POST" => Some("POST"),
        // ── LES GESTES D'ADMINISTRATION SE POSTENT, ILS NE SE LIENT PAS ─────
        // Sans cette liste, `GET /api/iam/utilisateur/desactiver?nom=alice`
        // fermerait un compte : il suffirait qu'un navigateur ou un préchargeur
        // suive un lien.
        //
        // `/api/iam/utilisateurs` n'y est PAS, et c'est la distinction qui
        // compte : sur ce chemin, `GET` LIT la liste et `POST` CRÉE. L'y mettre
        // refusait la lecture — mesuré : trois 405 sur la liste, là où on
        // attendait 401, 400 et 200.
        "/api/iam/utilisateur/desactiver"
        | "/api/iam/utilisateur/reactiver"
        | "/api/iam/utilisateur/mot-de-passe"
        | "/api/iam/utilisateur/espace"
        | "/api/iam/utilisateur/admin"
        | "/api/iam/utilisateur/jetons/revoquer"
        | "/api/iam/politique/supprimer"
        | "/api/iam/effacer"
            if methode != "POST" =>
        {
            Some("POST")
        }
        _ => None,
    }
}

/// La boucle d'UNE connexion : elle lit, route, et décide de continuer.
///
/// ── CE QUI A CHANGÉ, ET POURQUOI ───────────────────────────────────────────
/// Avant : une lecture, une réponse, `Connection: close`, et le fil mourait. Le
/// coût mesuré n'était pas le fil mais la poignée de main — des sockets en
/// TIME-WAIT relevés par dizaines sur le port pendant que la vue sondait
/// l'activation toutes les 1,2 s (≈ 5,9 fermetures/s).
///
/// Les délais de socket sont posés ICI, avant tout, et c'est l'ordre qui compte :
/// sans délai de lecture, garder la connexion ouverte transformerait un client
/// immobile en fil ET descripteur perdus pour toujours. Le keep-alive n'est sûr
/// QUE parce que les délais existent.
fn handle(flux: TcpStream, config: &ServeConfig) -> Result<()> {
    flux.set_read_timeout(Some(DELAI_LECTURE)).context("délai de lecture de la socket")?;
    flux.set_write_timeout(Some(DELAI_ECRITURE)).context("délai d'écriture de la socket")?;
    // `nodelay` : une réponse tient en une écriture, et l'algorithme de Nagle y
    // ajoutait jusqu'à 40 ms d'attente pour rien.
    let _ = flux.set_nodelay(true);

    let mut lecteur = Lecteur::nouveau(flux);
    let debut = Instant::now();
    let mut servies = 0u32;
    loop {
        if servies >= MAX_REQUETES {
            break;
        }
        if servies > 0 && debut.elapsed() >= INACTIVITE_MAX {
            break;
        }
        // Aucun report d'état d'une requête à l'autre : l'état d'un `HEAD` (pas
        // de corps) ou d'un `Connection: close` ne doit pas survivre à sa propre
        // requête. C'est aussi ce qui rend sûre la réponse aux erreurs de trame,
        // qui naissent avant qu'une requête existe.
        poser_etat_connexion(EtatConnexion::vierge());
        let reste = INACTIVITE_MAX.saturating_sub(debut.elapsed());
        lecteur.delai_lecture(if servies == 0 { DELAI_LECTURE } else { reste.max(Duration::from_millis(1)) });

        vent::remise_a_zero();
        let requete = match vent::chrono(vent::LECTURE, || lecteur.lire_requete()) {
            Ok(r) => r,
            Err(ErreurLecture::Fin) => break,
            Err(ErreurLecture::Silence) => {
                // Une PREMIÈRE requête qui n'arrive jamais mérite une réponse :
                // c'est la seule façon pour l'appelant de savoir pourquoi il
                // attend. Sur une connexion déjà servie, c'est la fin normale de
                // la fenêtre d'inactivité, et se taire est correct.
                if servies == 0 {
                    let _ = repondre_erreur(&mut lecteur.flux, 408, "aucun octet reçu dans le délai");
                }
                break;
            }
            Err(ErreurLecture::Partielle(raison)) => {
                let _ = repondre_erreur(&mut lecteur.flux, 400, &raison);
                break;
            }
            // Ces refus laissent le flux dans un état INCONNU (corps non lu) :
            // la connexion ne peut pas être réutilisée, et on le dit au client
            // par un `Connection: close` (l'état vierge ne garde pas).
            Err(ErreurLecture::Refus(code, raison)) => {
                let _ = repondre_erreur(&mut lecteur.flux, code, &raison);
                break;
            }
        };

        poser_etat_connexion(etat_du_client(&requete));
        let resultat = vent::chrono(vent::ROUTE, || router_requete(&mut lecteur.flux, &requete, config));
        vent::debit(&requete.chemin);
        let (garde, repondu) = etat_connexion(|e| (e.keep_alive && !e.fermer, e.repondu));
        servies += 1;
        if let Err(e) = resultat {
            // Un gestionnaire qui échoue est un défaut du SERVEUR : le client y a
            // droit à un 500, et non à une socket qui se ferme sans un mot
            // (« empty reply from server », ce que `curl` affiche alors). Si une
            // réponse est déjà partie, on ne peut plus qu'empiler — on se tait.
            if !repondu {
                let _ = repondre_erreur(&mut lecteur.flux, 500, &format!("{e:#}"));
            }
            eprintln!("viz serve : requête en erreur : {e:#}");
            break;
        }
        if !garde {
            break;
        }
    }
    Ok(())
}

/// Le ROUTAGE d'UNE requête : c'est l'ancien corps de `handle`, la lecture en
/// moins (elle vit dans `Lecteur`) et la socket en paramètre.
///
/// `mut stream: &mut TcpStream` — le `mut` porte sur LA RÉFÉRENCE, pas sur la
/// socket : les handlers reçoivent `&mut stream` (une ré-emprunt), ce qui exige
/// que la référence elle-même soit déclarée mutable. Sans ce mot, `&mut stream`
/// est refusé (E0596) alors que la socket, elle, est bel et bien mutable.
fn router_requete(
    mut stream: &mut TcpStream,
    requete: &Requete,
    config: &ServeConfig,
) -> Result<()> {
    let entetes: &str = requete.entetes_brut.as_str();
    let corps = String::from_utf8_lossy(&requete.corps);
    let method = requete.methode.as_str();
    let path: &str = requete.chemin.as_str();
    let query: Option<&str> = requete.requete.as_deref();

    // ── LA MÉTHODE, EN PREMIER ─────────────────────────────────────────────
    // `HEAD` est accepté (RFC 9110 §9.3.2 : un serveur DOIT le servir). Mesuré
    // avant : 405, et sur une méthode qui ne veut pas de corps, un corps de
    // 18 octets. `HEAD` suit ensuite le chemin d'un `GET` — c'est ce qui le rend
    // juste — et c'est `respond_octets` qui retient le corps au dernier moment.
    let m = if method == "HEAD" { "GET" } else { method };
    if !matches!(m, "GET" | "POST") {
        // `Allow` n'était PAS annoncé : un 405 disait « non » sans dire « quoi ».
        return respond_entetes(
            stream,
            405,
            "text/plain; charset=utf-8",
            "no-store",
            &[("Allow", "GET, HEAD, POST")],
            "method not allowed",
        );
    }
    if let Some(allow) = methode_refusee(path, m) {
        return respond_entetes(
            stream,
            405,
            "text/plain; charset=utf-8",
            "no-store",
            &[("Allow", allow)],
            "method not allowed",
        );
    }
    // `HEAD` sur le FLUX : répondre « comme un GET » ferait DÉMARRER le moteur —
    // une génération LLM complète, payée par un client qui ne lira rien. On
    // annonce l'endpoint sans le lancer.
    if method == "HEAD" && path == "/api/engine/chat" {
        return respond_octets(stream, 200, TYPE_SSE, "no-cache", None, &[], &[]);
    }

    // ── LA GARDE PASSE AVANT TOUTE ROUTE DE DONNÉES ─────────────────────────
    // Un seul point de contrôle couvre TOUTES les routes de données de ce
    // serveur : une route ajoutée demain est protégée sans qu'on ait à y penser.
    // C'est l'inverse du défaut du middleware du framework `this`, où une route
    // sans politique déclarée PASSE (`Abstain`) : ici, armé, tout ce qui atteint
    // ce point est soumis à la même décision, et le refus DIT pourquoi.
    //
    // * `/api/auth/*` est l'ENTRÉE de la chaîne : la garder interdirait de se
    //   connecter (personne ne détient encore de jeton).
    // * Les actifs statiques (page, bundle, `/viewer2d`) restent publics — un
    //   navigateur doit pouvoir CHARGER l'interface pour se connecter. Ce qui
    //   est protégé, c'est la DONNÉE.
    if path.starts_with("/api/") && !path.starts_with("/api/auth/") {
        let garde = GardeIam::partagee();
        // ── L'ADMINISTRATION A SON ACTION, PAS CELLE DU GRAPHE ──────────────
        // `/api/iam/*` est jugé sur `iam:admin` : exiger EN PLUS `graph:write`
        // ferait dépendre l'administration des comptes d'un droit sur le graphe,
        // qui n'a rien à voir. Le point de contrôle reste UNIQUE — une route
        // d'administration ajoutée demain est gardée sans qu'on ait à y penser.
        let administration = path.starts_with("/api/iam/");
        // ── « QUI SUIS-JE » NE DEMANDE AUCUN DROIT ──────────────────────────
        // L'appelant y apprend ses PROPRES espaces. Sans cette route, une page
        // devrait DEVINER sur quel espace se juger — ou se fier à un espace
        // qu'elle fournit elle-même, ce que cette garde refuse justement de
        // faire. Elle exige une identité, rien de plus : on ne peut lire que
        // soi.
        let mes_droits = path == "/api/iam/moi";
        // ── LISTER SES ACCES EST UNE META-CAPACITE, PAS UNE LECTURE DE BASE ──
        // `/api/stores` repond « quelles bases puis-je ouvrir ? » : il ne LIT
        // aucune base, il les filtre deja par droits dans son handler. Le juger
        // sur l espace par defaut (runtime-dev) exigeait graph:read dessus —
        // donc un compte sans runtime-dev recevait 403 en demandant la liste
        // meme des bases qu il PEUT lire, et l interface tombait sur l erreur
        // brute au lieu d un ecran « aucune base ». Comme /api/iam/moi : une
        // identite suffit, le filtrage fait le reste.
        // ── LE CANAL VERS SA PROPRE MACHINE NE LIT AUCUNE BASE ──────────────
        // `/api/remote/*` ne touche à aucun graphe : il attache la machine de
        // l'appelant, lui pousse des demandes et en rapporte les verdicts. Le
        // juger sur l'espace par défaut du serveur exigerait `graph:read` sur
        // `runtime-dev` pour parler à SON PROPRE ordinateur — la même erreur que
        // celle corrigée sur `/api/stores`. Une identité suffit ; le registre
        // refuse ensuite tout client qui n'est pas à ce compte.
        let capacite_de_soi =
            mes_droits || path == "/api/stores" || path.starts_with("/api/remote/");
        let ecriture = route_ecrivante(path, method, query);
        let jeton = jeton_presente(&entetes, query);
        let espace_nomme = if administration && !mes_droits {
            espace_iam(query, &corps)
        } else {
            None
        };
        // ── L'ESPACE JUGÉ QUAND L'APPELANT N'EN NOMME AUCUN ─────────────────
        // Défaut mesuré le 2026-09-15, en suivant l'erreur rapportée (« le flux
        // s'est fermé avant la première réponse ») jusqu'à sa cause : la garde
        // jugeait alors l'espace du PRESET DU SERVEUR (`runtime-dev`), sans
        // jamais demander qui peut le lire. Un compte ami se voyait donc refuser
        // une permission sur une base qu'il n'avait pas visée, et la vue — qui ne
        // distingue pas un refus d'une coupure — parlait de flux fermé.
        //
        // On mesure donc les droits UNE fois, et on en tire les deux choses dont
        // la garde a besoin : quel espace juger, et s'il y avait seulement quelque
        // chose à juger. Un annuaire illisible reste le cas « on ne sait pas »
        // (défaut du serveur, comportement historique) ; « aucune base lisible »
        // est un FAIT, et le refus doit alors dire QUOI FAIRE.
        // ── L'ESPACE JUGÉ QUAND L'APPELANT N'EN NOMME AUCUN ─────────────────
        // C'est le preset du SERVEUR (via `espace_vise`), et c'est un défaut
        // CONNU : un compte qui n'a pas cet espace est refusé en 403 alors qu'il
        // n'a jamais visé cette base. Mesuré le 2026-09-15 en traçant l'erreur
        // « le flux s'est fermé avant la première réponse » jusqu'ici.
        //
        // Une correction a été TENTÉE puis RETIRÉE : juger la première base
        // LISIBLE par l'appelant. Elle refusait des requêtes légitimes — « aucune
        // base accordée » tombait sur des comptes qui en avaient une — et faisait
        // passer les portes « monde utilisateur » et « self par compte » de
        // vertes à rouges. Sa cause : la résolution des droits de l'appelant
        // rend une liste VIDE aussi bien pour « aucun droit » que pour « je n'ai
        // pas pu regarder ». Refuser sur une mesure qu'on ne sait pas interpréter
        // est pire que refuser tard.
        //
        // CE QUI RESTE, ET QUI CORRIGE LE VRAI DÉFAUT : le refus parle désormais
        // la langue du flux (voir plus bas), donc la vue affiche la CAUSE au lieu
        // de « flux fermé avant la première réponse », qui accusait le transport.
        let espace = espace_nomme
            .clone()
            .unwrap_or_else(|| espace_vise(config, query));
        // ── CE CHEMIN EXISTE-T-IL SEULEMENT ? (manque 6, 2026-09-16) ────────
        // La question se pose AVANT le jugement de droit, et après rien : une
        // route inconnue n'a pas d'espace à juger — lui en inventer un (le store
        // par défaut) est exactement ce qui faisait répondre « permission
        // refusée » là où il fallait lire « n'existe pas ». Voir
        // `route_api_connue` pour l'arbitrage de non-fuite.
        let route_connue = route_api_connue(path);
        // ── LA RESSOURCE JUGÉE EST CELLE DU COMPTE, PAS UN NOM DE FICHIER ────
        // Deux cas, une même cause (défaillance 5.2) :
        //   * `/api/engine/memoire` sert LA mémoire de l'appelant : la ressource
        //     est son Self, pas le store par défaut du serveur. Mesuré : alice,
        //     identité valide, espace `esp-…`, recevait « permission denied
        //     … tenant/runtime-essai » — un espace qu'elle n'a jamais visé ;
        //   * une base `selfs/<compte>.obrain` EST la base de ce compte
        //     (`engine.rs::profil_du_self`) : la juger sur le nom de son FICHIER
        //     revenait à exiger que l'espace s'appelle comme la base, alors que le
        //     compte reçoit des droits sur SON espace.
        // Dans les deux cas le DROIT reste vérifié — sur la bonne ressource.
        let est_sa_base = vise_sa_propre_base(
            config,
            query,
            if garde.est_armee() {
                jeton.as_deref().and_then(|j| garde.identite(j))
            } else {
                None
            }
            .as_deref(),
        );
        let sert_le_compte = est_sa_base || path == "/api/engine/memoire";
        let verdict = if !route_connue {
            // ── LE REFUS D'IDENTITÉ PASSE EN PREMIER, ET C'EST CELUI DE TOUT LE
            // MONDE ────────────────────────────────────────────────────────────
            // On ne réécrit PAS le refus d'identité ici : `identite_ou_refus`
            // prononce exactement celui de la règle générique (`GardeIam::controler`,
            // branche finale), avec le MÊME texte. Deux textes pour deux `401`
            // suffiraient à refaire un oracle : un balayage SANS identité
            // énumérerait les routes qui existent par la seule forme du refus.
            match garde.identite_ou_refus(jeton.as_deref()) {
                Err(refus) => Err(refus),
                // Identité fournie (ou IAM non armé) : la route n'existe pas, et
                // c'est TOUT ce qu'il y a à dire — aucun espace n'a été jugé, donc
                // aucun `403` n'est prononcé, et rien de faux n'entre dans la
                // piste d'audit.
                Ok(()) => Err((
                    404,
                    format!(
                        "route inconnue : « {path} » n'est servie par aucune route de ce serveur \
                         — ce n'est PAS un refus de droit (l'IAM est {}). Un refus de droit se \
                         corrige en accordant un espace ; une route inconnue, en nommant la \
                         bonne route.",
                        if garde.est_armee() { "armé" } else { "non armé" }
                    ),
                )),
            }
        } else if capacite_de_soi {
            garde.identite_verifiee(jeton.as_deref())
        } else if sert_le_compte {
            garde.controler_ses_espaces(jeton.as_deref(), ecriture)
        } else if administration {
            // ── UN ESPACE NOMMÉ, JAMAIS UN DÉFAUT ───────────────────────────
            // Juger une route d'administration sur l'espace par défaut du serveur
            // reviendrait à trancher sur un espace que la requête ne vise pas :
            // soit on refuse un geste légitime sur un espace qu'on ne regardait
            // pas, soit — pire — on l'autorise au titre d'un autre. On demande
            // donc de nommer, et on le dit.
            match &espace_nomme {
                None => match garde.identite_verifiee(jeton.as_deref()) {
                    Ok(()) => Err((
                        400,
                        "cette route exige `espace=` (ou un champ `espace` dans le corps) : \
                         l'administration est jugée sur un espace NOMMÉ, jamais sur un défaut."
                            .to_string(),
                    )),
                    Err(refus) => Err(refus),
                },
                Some(e) => garde.controler_nomme(jeton.as_deref(), ACTION_ADMIN_HTTP, e),
            }
        } else if path.starts_with("/api/worlds") {
            // ── LES MONDES SONT UNE AFFAIRE DE COMPTE, PAS DE BASE VISITÉE ───
            // Ces routes ne portent AUCUN `store` : jugées sur l'espace visé,
            // elles retombaient sur le store PAR DÉFAUT du serveur — un espace
            // que l'appelant n'a jamais nommé. Mesuré le 2026-09-15 en ouvrant
            // obrain aux comptes : un compte autorisé sur SON espace pouvait
            // ajouter un dépôt (l'ajout, lui, a sa ressource propre) mais
            // recevait ensuite « permission denied sur runtime-dev » en
            // essayant de VOIR son monde. Ajouter sans pouvoir voir n'est pas
            // une fonctionnalité.
            //
            // Le droit de chacun est évalué par le moteur de politiques sur SES
            // espaces (`Garde::controler_soi`) : le serveur ne fait que choisir
            // la ressource à juger.
            match espaces_de_l_appelant(jeton.as_deref()) {
                Ok(espaces) => match (garde.interieure(), jeton.as_deref()) {
                    (Some(g), Some(j)) => {
                        let session = g.authentifier(Some(j)).ok().flatten();
                        g.controler_soi(session.as_ref(), &espaces)
                            .map_err(|e| (403, e.to_string()))
                    }
                    // IAM ARMÉ, aucune identité présentée : le refus — c'est le
                    // seul cas où il y a réellement quelque chose à refuser.
                    (Some(_), None) => Err((401, "aucune identité authentifiée".to_string())),
                    // ── IAM NON ARMÉ : IL N'Y A AUCUN DROIT À JUGER (2026-09-16) ──
                    // Même convention que `Garde::controler`, qui rend `Ok(())`
                    // dès que la garde intérieure est absente, et que la règle
                    // déjà écrite pour `/api/stores` (« sans IAM armé, il n'y a
                    // pas de droits à appliquer : la liste est entière »).
                    //
                    // Défaut mesuré : cette branche tombait dans le refus pour
                    // `(None, _)` — donc sur TOUT déploiement non armé,
                    // `/api/worlds` et `/api/worlds/status` rendaient 401. La
                    // porte d'acceptation du 8432 le mesurait sans le nommer
                    // (25/27 endpoints, 2 en échec). Et le corps du refus se
                    // contredisait lui-même, puisqu'il portait « IAM NON ARMÉ —
                    // aucune identité n'est exigée, quiconque atteint ce port lit
                    // et écrit tout le graphe ». Conséquence concrète : la
                    // fenêtre « Ajouter un monde » ne pouvait pas lire la liste.
                    (None, _) => Ok(()),
                },
                Err(refus) => Err(refus),
            }
        } else {
            garde.controler(jeton.as_deref(), ecriture, &espace)
        };
        if let Err((code, raison)) = verdict {
            // ── LE CORPS D'UN REFUS D'IDENTITÉ NE DIT RIEN DE LA ROUTE ──────
            // Deux corps différents pour deux 401 (« connue » vrai/faux) et le
            // serveur redevient l'oracle qu'on refuse d'être : un balayage SANS
            // identité énumérerait les routes qui existent par la seule forme du
            // refus. Donc `connue` vaut « non jugé » (`null`) dès que le refus
            // est un 401 : le refus d'identité est alors rigoureusement le même
            // pour `/api/harness/health` et pour `/api/nimportequoi`.
            let refus_identite = code == 401;
            // ── UN REFUS DE DROIT NOMME SON ESPACE ; « INCONNUE » N'EN A PAS ─
            // Un `404 route inconnue` n'a jugé AUCUN espace : y écrire le store
            // par défaut désignerait une ressource que la requête n'a jamais
            // visée — le défaut même qu'on corrige.
            let corps = serde_json::json!({
                "error": raison,
                "iam": garde.etat(),
                "connue": if refus_identite {
                    serde_json::Value::Null
                } else {
                    serde_json::json!(route_connue)
                },
                "espace": if refus_identite || route_connue {
                    serde_json::json!(espace)
                } else {
                    serde_json::Value::Null
                },
                "acces": if administration {
                    "administration"
                } else if ecriture {
                    "écriture"
                } else {
                    "lecture"
                },
            })
            .to_string();
            // ── UN CLIENT SSE NE DOIT PAS VOIR UN FLUX MOURIR SANS UN MOT ───
            // Défaut rapporté le 2026-09-15 : « le flux s'est fermé avant la
            // première réponse. 4 reprise(s) automatique(s) n'ont pas abouti ».
            // Cause : quand la garde refuse une route de FLUX, on répond du JSON
            // avec un code 401/403. `EventSource` ne sait lire que du SSE : il
            // reçoit une réponse qu'il ne peut pas interpréter, déclenche
            // `onerror`, et la vue — qui ne PEUT PAS distinguer un refus d'une
            // coupure de transport — conclut « flux fermé avant la première
            // réponse », puis relance QUATRE fois un geste que le serveur
            // refusera toujours. La cause réelle (jeton absent ou expiré,
            // politique muette) n'apparaissait NULLE PART : ni à l'écran, ni
            // dans le journal du serveur, où la vue renvoyait pourtant le
            // lecteur.
            //
            // On lui parle donc sa langue : une trame `erreur` dans le corps,
            // qui porte la raison exacte ET le code. Le client l'affiche au lieu
            // de deviner, et le journal garde la cause.
            if entetes.to_ascii_lowercase().contains("text/event-stream") {
                eprintln!(
                    "viz serve : refus {code} sur le FLUX {path} — {raison} (espace visé : {espace})"
                );
                let trame = format!(
                    "event: erreur\ndata: {}\n\n",
                    serde_json::json!({ "error": raison, "code": code, "espace": espace })
                );
                let entetes_sse = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\n\
                     Cache-Control: no-store\r\nConnection: close\r\n\
                     X-Accel-Buffering: no\r\n\r\n";
                let _ = stream.write_all(entetes_sse.as_bytes());
                let _ = stream.write_all(trame.as_bytes());
                let _ = stream.flush();
                return Ok(());
            }
            // Un 401 doit DIRE comment s'authentifier : un client HTTP
            // générique ne lit pas le corps pour le deviner.
            let entetes_refus: Vec<(&str, &str)> = if code == 401 {
                vec![("WWW-Authenticate", "Bearer realm=\"obrain\"")]
            } else {
                Vec::new()
            };
            return respond_entetes(&mut stream, code, VIZ_JSON, "no-store", &entetes_refus, &corps);
        }
    }

    match path {
        // Racine = Dashboard React (unique) — l'ancien viewer n'est plus servi
        // par défaut.
        "/" | "/index.html" => {
            // ── L'ACCUEIL MÈNE À LA CONNEXION QUAND ELLE EST NÉCESSAIRE ─────
            // Sans cela, armer la garde donnait un dashboard qui se charge et
            // dont tous les appels échouent en 401, sans issue visible. La page
            // reste publique (on doit pouvoir la charger pour se connecter) ;
            // c'est le CONTENU qui exige une identité.
            {
                let garde = GardeIam::partagee();
                if garde.est_armee()
                    && jeton_presente(&entetes, query)
                        .as_deref()
                        .and_then(|j| garde.identite(j))
                        .is_none()
                {
                    return respond_entetes(
                        &mut stream,
                        303,
                        "text/plain; charset=utf-8",
                        "no-store",
                        &[("Location", "/login")],
                        "connexion requise",
                    );
                }
            }
            let chemin_page = app_dir().join("index.html");
            match std::fs::read(&chemin_page) {
                Ok(octets) => {
                    let html = String::from_utf8_lossy(&octets).into_owned();
                    let mtime_fichier = std::fs::metadata(&chemin_page)
                        .and_then(|m| m.modified())
                        .ok();
                    // On RETIENT ce qui a été servi : c'est le filet de la
                    // reconstruction en cours (voir `derniere_page`).
                    *verrou(derniere_page()) = Some((html.clone(), mtime_fichier));
                    let body = if dev_enabled() { inject_dev(&html) } else { html };
                    // ── LA PAGE EST REVALIDABLE, ELLE N'ÉTAIT PAS « no-store » ─
                    // Mesuré : le commentaire de `respond_cache` promettait
                    // `no-cache` pour la page d'entrée, mais CETTE route-ci passait
                    // par `respond`, donc `no-store` : le navigateur la
                    // retéléchargeait entièrement à chaque visite, sans jamais
                    // pouvoir demander « est-ce que ça a changé ? ». Ici elle est
                    // revalidable, ET son étiquette est dérivée du corps SERVI :
                    // en mode dev, `inject_dev` ajoute un script, donc l'ETag du
                    // corps injecté ne doit pas être celui du fichier.
                    let mtime = mtime_fichier;
                    let etag = etag_de(body.len() as u64, mtime);
                    respond_octets(
                        &mut stream,
                        200,
                        "text/html; charset=utf-8",
                        "no-cache",
                        Some(&etag),
                        &[],
                        body.as_bytes(),
                    )
                }
                // ── LE FICHIER MANQUE : ON RESERT LA DERNIÈRE PAGE ──────────
                // Cas normal en développement : `vite build` a vidé `dist/` et
                // n'a pas encore réécrit `index.html`. On ne casse pas la page
                // de l'utilisateur pour ça.
                Err(e) => {
                    let garde = verrou(derniere_page());
                    match garde.as_ref() {
                        Some((html, mtime_fichier)) => {
                            tracing::warn!(
                                erreur = %e,
                                "index.html absent (reconstruction en cours ?) —                                  on ressert la dernière page servie"
                            );
                            let body = if dev_enabled() { inject_dev(html) } else { html.clone() };
                            let etag = etag_de(body.len() as u64, *mtime_fichier);
                            respond_octets(
                                &mut stream,
                                200,
                                "text/html; charset=utf-8",
                                "no-cache",
                                Some(&etag),
                                &[],
                                body.as_bytes(),
                            )
                        }
                        // Aucune page n'a JAMAIS été servie : le fichier manque
                        // vraiment. Là, le dire est la seule réponse honnête.
                        None => respond(&mut stream, 404, "text/plain", "index absent"),
                    }
                }
            }
        }
        "/viewer2d" => respond(&mut stream, 200, "text/html; charset=utf-8", VIEWER2D_HTML),
        // NATIF-J9 : streaming WebSocket du MEME subgraph (voie additionnelle,
        // negociee par upgrade). Le HTTP one-shot ci-dessous reste le defaut.
        "/api/viz/subgraph/stream" => api_subgraph_stream(&mut stream, &entetes, query, config),
        "/api/viz/subgraph" => api_subgraph(&mut stream, query, config),
        "/api/viz/timeline" => crate::viz_timeline::api_viz_timeline(&mut stream, query, config),
        "/api/viz/sessions" => crate::viz_timeline::api_viz_sessions(&mut stream, query, config),
        "/api/stores" => api_stores(&mut stream, &entetes, query, config),
        // LE CANAL VERS LA MACHINE DE L'APPELANT — un seul point d'entrée, qui
        // distingue ensuite les cinq gestes (voir `api_remote`).
        p if p.starts_with("/api/remote/") => {
            api_remote(&mut stream, p, method, &entetes, query, &corps)
        }
        "/api/info" => api_info(&mut stream, query, config),
        "/api/viz/overview" => api_overview(&mut stream, query, config),
        "/api/viz/modules" => api_viz_modules(&mut stream, query, config),
        "/api/viz/signatures" => api_viz_signatures(&mut stream, &entetes, query, config),
        // LES ECHELLES QUE LE GRAPHE PERMET (derivees, plus sondees).
        "/api/viz/orbites" => api_viz_orbites(&mut stream, query, config),
        "/api/viz/arcs" => api_viz_arcs(&mut stream, query, config),
        "/api/viz/ramifications" => api_viz_ramifications(&mut stream, query, config),
        "/api/viz/space" => api_viz_space(&mut stream, query, config),
        "/api/viz/weave" => api_viz_weave(&mut stream, &entetes, query, config),
        "/api/viz/activation" => api_viz_activation(&mut stream, query, config),
        // MONDES : l'ajout est une MUTATION → POST. La liste et le suivi sont
        // des lectures → GET.
        "/api/worlds" if method == "POST" => {
            api_worlds_ajouter(&mut stream, &entetes, query, &corps)
        }
        "/api/worlds" => api_worlds_liste(&mut stream, &entetes, query, config),
        "/api/worlds/status" => api_worlds_etat(&mut stream, query),
        "/api/engine/sante" => api_engine_sante(&mut stream),
        // CE QUE LA CONVERSATION A STRUCTURÉ, et qui attend d'être appliqué.
        // Exposé pour que l'utilisateur VOIE la mémoire : une écriture invisible
        // ne se distingue pas d'une écriture qui n'a pas eu lieu.
        "/api/engine/memoire" => api_engine_memoire(&mut stream, &entetes, query, config),
        // QUI EXISTE : les selfs résidents, lus sur les journaux et le registre
        // des mondes. C'est le préalable à toute résonance — on ne relie pas des
        // selfs sans pouvoir dire lesquels existent.
        "/api/engine/selfs" => api_engine_selfs(&mut stream, config),
        // ── LA RÉSONANCE : LES FACETTES PUBLIQUES D'UN AUTRE SELF ───────────
        // `GET` : ce que JE vois (mes abonnements, et les facettes de chacun) et
        // qui me voit. `POST` : le geste — lier ou délier un lecteur de MON self.
        //
        // Rien n'est visible par défaut, et le lien se referme : un partage qui
        // s'active tout seul n'est pas un partage, c'est une fuite qu'on n'a pas
        // annoncée. Le texte libre (`note`) ne sort jamais — voir `resonance.rs`.
        "/api/engine/resonance" if method == "POST" => {
            api_resonance_geste(&mut stream, &entetes, query, &corps, config)
        }
        "/api/engine/resonance" => api_resonance(&mut stream, &entetes, query, config),
        // LES RÉPARATIONS DUES — attachées en vol pendant que les bases
        // tournaient, en attente de la fenêtre d'exclusivité. Exposées parce
        // qu'un protocole qui n'est visible que dans un terminal n'est pas un
        // protocole.
        "/api/entretien" => api_entretien(&mut stream),
        "/api/engine/ask" => api_engine_ask(&mut stream, &entetes, query, config),
        // CONVERSATION EN FLUX (SSE). `EventSource` ne sait faire que du GET :
        // la question, le sujet et l'historique passent donc en paramètres.
        "/api/engine/chat" => api_engine_chat(&mut stream, &entetes, query, config),
        // ── LA CHAÎNE D'AUTHENTIFICATION, EN CLAIR ─────────────────────────
        // Connexion (mot de passe → jeton + cookie), « qui suis-je », et
        // déconnexion (révocation réelle). Ces trois routes sont ce qui manquait
        // pour qu'un navigateur s'authentifie sans proxy.
        "/api/auth/login" if method == "POST" => api_auth_login(&mut stream, &entetes, &corps),
        "/api/auth/logout" if method == "POST" => api_auth_logout(&mut stream, &entetes, query),
        "/api/auth/whoami" => api_auth_whoami(&mut stream, &entetes, query),
        // ROTATION de session : nouveau jeton, l'ancien meurt.
        "/api/auth/refresh" if method == "POST" => {
            api_auth_refresh(&mut stream, &entetes, query)
        }
        // LA PISTE D'AUDIT : qui a été refusé, quand, et pourquoi.
        "/api/auth/audit" => api_auth_audit(&mut stream, &entetes, query, config),
        // ── L'ADMINISTRATION DES COMPTES ────────────────────────────────────
        // UN SEUL point d'entrée. La garde globale a déjà exigé `iam:admin` sur
        // l'espace visé (voir le point de contrôle unique plus haut) ; le
        // sous-routage vit dans `api_iam`, qui PLAFONNE en plus chaque geste sur
        // les droits réels de l'appelant. Deux barrières, et la seconde ne fait
        // pas confiance à la première pour connaître l'étendue des droits.
        //
        // Les gestes qui EMMETTENT un jeton ou amorcent un administrateur n'y
        // sont pas, délibérément : ils exigent le hors bande (le CLI).
        p if p.starts_with("/api/iam/") => {
            api_iam(&mut stream, p, method, &entetes, query, &corps, config)
        }
        // LA PAGE D'ADMINISTRATION : servie comme celle de connexion, sans
        // dépendance — un écran qui exigerait un bundle ne serait pas un écran
        // de secours le jour où le bundle est cassé.
        "/iam" => respond(&mut stream, 200, "text/html; charset=utf-8", PAGE_IAM),
        // LA PAGE DE CONNEXION : sans elle, armer la garde rendait le dashboard
        // inutilisable (une requête `fetch` ne peut pas poser de cookie toute
        // seule). Un formulaire suffit, et il ne dépend d'aucun script.
        "/login" => respond(&mut stream, 200, "text/html; charset=utf-8", PAGE_CONNEXION),
        "/api/viz/status" => api_status(&mut stream, config),
        "/sitemap.xml" => api_sitemap_xml(&mut stream, config),
        "/api/functions" => api_functions(&mut stream, query, config),
        "/api/query" => api_query(&mut stream, query, config),
        "/__dev/version" => {
            let body = serde_json::json!({ "version": dev_version() }).to_string();
            respond(&mut stream, 200, VIZ_JSON, &body)
        }
        "/app" | "/app/" => static_file(&mut stream, &app_dir().join("index.html"), "text/html; charset=utf-8"),
        // ── `/app/*` : LE CHEMIN EST DÉCODÉ, ASSAINI, PUIS VÉRIFIÉ ──────────
        // La faille fermée ici était mesurée : `app_dir().join(rel)` sur un `rel`
        // ni décodé ni assaini, donc
        //   curl --path-as-is '…/app/../../../../../../../etc/hostname' → 200
        // et le contenu du fichier. Lecture de tout ce que le processus peut
        // lire, hors de la garde IAM (elle ne couvre que `/api/**`).
        p2 if p2.starts_with("/app/") => {
            let rel = &p2["/app/".len()..];
            match chemin_sous(&app_dir(), rel) {
                Some(chemin) => {
                    let type_mime = mime_of(&chemin.to_string_lossy());
                    static_file(&mut stream, &chemin, type_mime)
                }
                // 404, et NON 403 : dire « interdit » confirmerait l'existence
                // d'un fichier hors du dossier — exactement ce qu'un balayage
                // cherche à apprendre, et ce que la RFC recommande de taire.
                None => respond(&mut stream, 404, "text/plain", "fichier absent"),
            }
        }
        p if p.starts_with("/vendor/") => static_vendor(&mut stream, &p["/vendor/".len()..]),
        // ── AUCUN CHEMIN `/api` NE FINIT EN « not found » MUET (2026-09-16) ──
        // Filet de la correction du manque 6 : la garde répond désormais
        // « route inconnue » pour tout `/api/*` qui n'existe pas, et cette
        // branche couvre ce qui ne passe PAS par elle — `/api/auth/*` en est
        // exclu (c'est l'entrée de la chaîne : on doit pouvoir se connecter).
        // Le corps est le MÊME qu'avec l'IAM armé : « sans IAM comme avec ».
        p if p.starts_with("/api/") => {
            let corps = serde_json::json!({
                "error": format!(
                    "route inconnue : « {p} » n'est servie par aucune route de ce serveur — ce \
                     n'est PAS un refus de droit."
                ),
                "iam": GardeIam::partagee().etat(),
                "connue": false,
                "espace": serde_json::Value::Null,
                "acces": "lecture",
            })
            .to_string();
            respond(&mut stream, 404, VIZ_JSON, &corps)
        }
        _ => respond(&mut stream, 404, "text/plain", "not found"),
    }
}

// == COMPRESSION gzip DES REPONSES (NATIF-J7) ==============================
// Le cout dominant des endpoints viz est le TRANSFERT du payload JSON (temps
// ~ proportionnel a la taille). gzip donne ~30x sur ce JSON tres repetitif.
//
// Serveur un-thread-par-connexion (Connection: close, une reponse par thread) :
// on lit `Accept-Encoding` une fois dans `handle`, on le depose ici, et
// `respond_cache` le lit au moment d'ecrire. Remis a false au debut de chaque
// requete -> aucun etat remanent.
thread_local! {
    static REQ_ACCEPTS_GZIP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Seuil sous lequel on ne compresse pas : l'overhead (entete gzip + CPU)
/// depasse le gain sur une petite reponse (ex. un message d'erreur de 200 o).
const GZIP_SEUIL: usize = 4096;

/// L'en-tete `Accept-Encoding` de la requete annonce-t-il gzip ? Tolerant :
/// insensible a la casse, liste separee par des virgules, `q=0` traite comme
/// un refus explicite de ce codage. Absence d'en-tete -> false (identite).
fn accept_encoding_veut_gzip(entetes: &str) -> bool {
    for l in entetes.lines() {
        let low = l.trim().to_ascii_lowercase();
        if let Some(v) = low.strip_prefix("accept-encoding:") {
            for tok in v.split(',') {
                let mut parts = tok.split(';').map(str::trim);
                let codec = parts.next().unwrap_or("");
                if codec == "gzip" || codec == "*" {
                    let refuse = parts.any(|p| {
                        let p = p.replace(' ', "");
                        p == "q=0" || p == "q=0.0" || p == "q=0.00" || p == "q=0.000"
                    });
                    if !refuse {
                        return true;
                    }
                }
            }
            return false;
        }
    }
    false
}

/// Type de contenu compressible (texte structure) — jamais un binaire deja
/// compresse (png, woff...).
fn est_compressible(content_type: &str) -> bool {
    content_type.starts_with("application/json")
        || content_type.starts_with("text/")
        || content_type.contains("xml")
        || content_type.contains("javascript")
        || content_type.contains("+json")
}

/// gzip d'un corps. Niveau 6 : meilleur compromis debit/ratio pour ce JSON.
fn gzip_corps(corps: &[u8]) -> std::io::Result<Vec<u8>> {
    use flate2::{Compression, write::GzEncoder};
    let mut enc = GzEncoder::new(Vec::with_capacity(corps.len() / 3 + 64), Compression::new(6));
    enc.write_all(corps)?;
    enc.finish()
}

pub(crate) fn respond(stream: &mut TcpStream, code: u16, content_type: &str, body: &str) -> Result<()> {
    // Les réponses d'API sont dynamiques : jamais de cache.
    respond_cache(stream, code, content_type, "no-store", body)
}

/// Variante qui porte une directive de cache explicite.
///
/// Sans AUCUNE directive, le serveur laissait le navigateur décider seul — et il
/// applique alors une mise en cache HEURISTIQUE, agressive sur mobile. C'est une
/// cause directe du « je ne le vois pas » : après un rebuild, la page pouvait
/// continuer de servir l'ANCIEN bundle, sans que rien ne l'indique.
///
/// La paire correcte, et c'est celle qu'on applique aux fichiers statiques :
///   - `index.html` → `no-cache` : revalidé à chaque visite, donc TOUJOURS à jour ;
///   - les bundles hachés (`index-XXXX.js`) → `immutable` : leur NOM change à
///     chaque build, ils peuvent donc être gardés indéfiniment sans risque.
/// Variante qui porte une directive de cache explicite.
///
/// Sans AUCUNE directive, le serveur laissait le navigateur décider seul — et il
/// applique alors une mise en cache HEURISTIQUE, agressive sur mobile. C'est une
/// cause directe du « je ne le vois pas » : après un rebuild, la page pouvait
/// continuer de servir l'ANCIEN bundle, sans que rien ne l'indique.
///
/// La paire correcte, et c'est celle qu'on applique aux fichiers statiques :
///   - `index.html` → `no-cache` : revalidé à chaque visite, donc TOUJOURS à jour ;
///   - les bundles hachés (`index-XXXX.js`) → `immutable` : leur NOM change à
///     chaque build, ils peuvent donc être gardés indéfiniment sans risque.
/// UNE RÉPONSE SERVIE PAR LE CACHE NE PORTE PAS DE MESURE.
///
/// `execution_time_ms` décrit l'exécution qui a PRODUIT le corps ; le resservir
/// fait croire à une mesure fraîche. C'est ce qui a fait annoncer « 3,71 ms »
/// pour une requête non exécutée, puis « 59,909448 ms » — le MÊME flottant trois
/// fois de suite, signature d'une valeur stockée. On le retire, et on le DIT
/// (`from_cache: true`) : un chiffre absent vaut mieux qu'un chiffre qui ment.
fn sans_mesure(corps: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(corps) {
        Ok(mut v) => {
            if let Some(o) = v.as_object_mut() {
                o.remove("execution_time_ms");
                o.insert("from_cache".to_string(), serde_json::Value::Bool(true));
            }
            v.to_string()
        }
        Err(_) => corps.to_string(),
    }
}

fn respond_cache(
    stream: &mut TcpStream,
    code: u16,
    content_type: &str,
    cache: &str,
    body: &str,
) -> Result<()> {
    respond_entetes(stream, code, content_type, cache, &[], body)
}

/// Réponse avec des EN-TÊTES en plus : `Set-Cookie` pour la session,
/// `WWW-Authenticate` sur un 401.
///
/// ── POURQUOI (2026-09-13) ───────────────────────────────────────────────
/// La garde ne pouvait ni poser un cookie (donc aucun navigateur ne pouvait
/// recevoir de session) ni annoncer `WWW-Authenticate` sur son 401 : les deux
/// exigent des en-têtes, ce que cette écriture ne savait pas faire. Les phrases
/// de raison 401 et 403 manquaient aussi — un refus s'annonçait « Error ».
fn respond_entetes(
    stream: &mut TcpStream,
    code: u16,
    content_type: &str,
    cache: &str,
    entetes: &[(&str, &str)],
    body: &str,
) -> Result<()> {
    respond_octets(stream, code, content_type, cache, None, entetes, body.as_bytes())
}

/// ── L'ÉCRITURE D'UNE RÉPONSE : UN SEUL ENDROIT DÉCIDE ──────────────────────
///
/// Tout passe par ici — les trente routes comme les fichiers, le `HEAD` comme le
/// `GET` — parce que trois choses ne peuvent pas être décidées par la route :
///
///   1. **la revalidation** : `If-None-Match` rencontre l'`ETag` du fichier, et
///      la réponse devient un 304 SANS corps. Avant, une revalidation renvoyait
///      200 + 1 286 462 octets — le client demandait « a-t-il changé ? » et on
///      lui répondait en renvoyant tout.
///   2. **la compression** : `Accept-Encoding` du client contre le type MIME et
///      la taille du corps. Le `Content-Length` annoncé est celui du corps
///      DÉGONFLÉ, parce que c'est ce qui va réellement sortir de la socket —
///      annoncer la taille d'origine ferait attendre au client des octets qui ne
///      viendront jamais, et `curl` sortirait en « transfer closed with N bytes
///      remaining ».
///   3. **le cadre de connexion** : `Connection: keep-alive` ou `close`, la
///      méthode `HEAD` (en-têtes seuls) et le `Date` obligatoire.
///
/// `HEAD` : le corps est retenu, mais l'`ETag` et le `Content-Length` restent
/// ceux du `GET` — c'est leur définition même, et les remplacer par zéro serait
/// un mensonge que `curl -I` afficherait. Le corps est donc bien dégonflé pour un
/// `HEAD` : c'est le seul moyen de connaître exactement la longueur qu'un `GET`
/// aurait envoyée.
fn respond_octets(
    stream: &mut TcpStream,
    code: u16,
    content_type: &str,
    cache: &str,
    etag: Option<&str>,
    entetes: &[(&str, &str)],
    corps: &[u8],
) -> Result<()> {
    let (head, keep_alive, accepte, if_none_match, deja_ferme) = etat_connexion(|e| {
        (e.head, e.keep_alive, e.accepte_encodage.clone(), e.if_none_match.clone(), e.fermer)
    });
    // L'état est relu puis RELÂCHÉ : la suite n'écrit pas sous l'emprunt du
    // `RefCell` (voir `etat_connexion`), et marque la réponse comme écrite pour
    // que la boucle de connexion ne double pas un 500 par-dessus.
    etat_connexion(|e| e.repondu = true);

    let mut code = code;
    let mut corps: Cow<'_, [u8]> = Cow::Borrowed(corps);
    if revalidation_304(code, etag, if_none_match.as_deref()) {
        code = 304;
        // Un 304 n'a pas de corps : c'est tout son intérêt. Ni `Content-Length`
        // ni `Content-Encoding` ne sont alors émis — annoncer une longueur pour
        // un corps absent ferait attendre le client.
        corps = Cow::Borrowed(&[]);
    }
    let encodage =
        if code == 304 { Encodage::Aucun } else { encodage_negocie(accepte.as_deref(), content_type, corps.len()) };
    match encodage {
        Encodage::Aucun => {}
        Encodage::Gzip | Encodage::Deflate => {
            corps = Cow::Owned(compresser(&corps, encodage).map_err(|e| anyhow::anyhow!("compression : {e}"))?);
        }
    }

    let raison = raison_http(code);
    let garder = keep_alive && !deja_ferme;
    let mut tete = String::with_capacity(384);
    tete.push_str(&format!("HTTP/1.1 {code} {raison}\r\n"));
    // `Date` : la RFC 9110 §6.6.1 l'impose à TOUTE réponse. Il manquait partout,
    // donc ni un cache ni un client ne pouvait dater ce qu'il recevait.
    tete.push_str(&format!("Date: {}\r\n", date_http()));
    tete.push_str(&format!("Content-Type: {content_type}\r\n"));
    if code != 304 {
        tete.push_str(&format!("Content-Length: {}\r\n", corps.len()));
    }
    tete.push_str(&format!("Cache-Control: {cache}\r\n"));
    if !content_type.starts_with("text/event-stream") {
        // `Vary: Accept-Encoding` est OBLIGATOIRE dès qu'on négocie, et il l'est
        // même quand on n'a PAS dégonflé cette fois-ci : sans lui, un cache
        // partagé peut servir une réponse non compressée à un client qui accepte
        // gzip, et l'inverse — les deux erreurs sont visibles côté client comme
        // « du texte binaire ».
        tete.push_str("Vary: Accept-Encoding\r\n");
    }
    if let Some(etiquette) = etag {
        tete.push_str(&format!("ETag: {etiquette}\r\n"));
    }
    match encodage {
        Encodage::Gzip => tete.push_str("Content-Encoding: gzip\r\n"),
        Encodage::Deflate => tete.push_str("Content-Encoding: deflate\r\n"),
        Encodage::Aucun => {}
    }
    for (nom, valeur) in entetes {
        tete.push_str(&format!("{nom}: {valeur}\r\n"));
    }
    tete.push_str(if garder { "Connection: keep-alive\r\n" } else { "Connection: close\r\n" });
    tete.push_str("\r\n");
    vent::note_octets(corps.len());
    vent::chrono(vent::ECRITURE, || -> Result<()> {
        stream.write_all(tete.as_bytes())?;
        if !head && !corps.is_empty() {
            stream.write_all(&corps)?;
        }
        stream.flush()?;
        Ok(())
    })?;
    Ok(())
}

/// Une erreur du PROTOCOLE (et non d'une route) : trame illisible, en-têtes trop
/// longs, corps annoncé au-delà du plafond, délai dépassé. Le corps dit en clair
/// ce qui n'a pas été accepté — mesuré avant : la ligne de requête absente
/// rendait `Ok(())` SANS RIEN ÉCRIRE, donc le client pendait jusqu'à son propre
/// délai sans apprendre pourquoi.
fn repondre_erreur(stream: &mut TcpStream, code: u16, quoi: &str) -> Result<()> {
    let corps = format!("{code} {} — {quoi}\n", raison_http(code));
    // L'état est celui d'une connexion vierge : ces erreurs naissent avant qu'une
    // requête soit identifiée, donc la réponse ferme la connexion et porte son
    // corps (aucun `HEAD` ne peut être en cours).
    poser_etat_connexion(EtatConnexion::vierge());
    respond_octets(stream, code, "text/plain; charset=utf-8", "no-store", None, &[], corps.as_bytes())
}

/// Sert un fichier vendor (allowlist stricte) depuis `docs/examples/viz/vendor`.
fn static_vendor(stream: &mut TcpStream, name: &str) -> Result<()> {
    let name = percent_decode(name);
    if name.is_empty()
        || name.contains(['/', '\\'])
        || name.contains("..")
        || !VENDOR_FILES.contains(&name.as_str())
    {
        return respond(stream, 404, "text/plain", "vendor introuvable");
    }
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/viz/vendor");
    let Ok(octets) = std::fs::read(dir.join(&name)) else {
        return respond(
            stream,
            404,
            "text/plain",
            "vendor absent — relancer depuis le dépôt (docs/examples/viz/vendor)",
        );
    };
    // Servi en OCTETS (et non en `String`) : un actif n'est pas forcément de
    // l'UTF-8 valide, et `from_utf8_lossy` remplaçait alors chaque octet invalide
    // par U+FFFD — un fichier servi CORROMPU, silencieusement.
    static_octets(stream, &dir.join(&name), "application/javascript", "no-cache")
}

pub(crate) fn parse_query(query: Option<&str>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(query) = query else { return out };
    for pair in query.split('&') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        out.insert(percent_decode(k), percent_decode(v));
    }
    out
}

/// Décodage percent minimal (`%XX`, `+` → espace) — suffisant pour les
/// chemins/qn usuels.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                // ── LES BORNES SONT DES OCTETS, LA COUPE DOIT L'ÊTRE AUSSI ────
                // Avant : `input[i + 1..i + 3]` — un index d'OCTET servi à une
                // `&str`, donc un slice sur des indices de CARACTÈRES. Dès qu'un
                // caractère multi-octets commençait dans l'intervalle, le thread
                // client PANIQUAIT avant d'écrire quoi que ce soit : `?x=%aé`
                // rendait une connexion coupée sans un octet d'explication
                // (mesuré : `curl` sort en 52, « empty reply from server »).
                // On décode sur les OCTETS, et on n'accepte que de l'hexadécimal
                // ASCII — c'est la définition de `%XX`.
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                if let Some(b) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    out.push(b);
                    i += 2;
                } else {
                    out.push(bytes[i]);
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Mémoïsation (court TTL) de la résolution centre → id.
///
/// Le repli suffixe (`ENDS WITH '.nom'`) scanne tout le store sur les gros
/// dépôts (1,2–3,4 s mesuré sur 1,55 M nœuds) : sans cache, chaque
/// densification d'un même centre paierait ce scan. Le premier appel paie,
/// les suivants (~0 ms) gardent le centre stable pendant toute une session
/// d'exploration (bascules d'échelle 1/2/∞, allers-retours d'ancres).
const RESOLVE_TTL: std::time::Duration = std::time::Duration::from_secs(120);
static RESOLVE_CACHE: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<(std::path::PathBuf, String), (std::time::Instant, u64)>>,
> = std::sync::OnceLock::new();
fn resolve_memo(store: &std::path::Path, arg: &str) -> Option<u64> {
    let cache = RESOLVE_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let map = verrou(&cache);
    if let Some((t, id)) = map.get(&(store.to_path_buf(), arg.to_string())) {
        if t.elapsed() < RESOLVE_TTL {
            return Some(*id);
        }
    }
    None
}
fn resolve_memo_put(store: &std::path::Path, arg: &str, id: u64) {
    let cache = RESOLVE_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut map = verrou(&cache);
    map.insert(
        (store.to_path_buf(), arg.to_string()),
        (std::time::Instant::now(), id),
    );
    if map.len() > 4096 {
        map.retain(|_, (t, _)| t.elapsed() < RESOLVE_TTL);
    }
}

/// Cache court des sous-graphes : les interactions du dashboard
/// (bascules d'échelle, aller-retour d'ancres, overlays) re-demandent des
/// requêtes identiques ; le re-jeu doit être ~0 ms même sur gros store.
///
/// ── POURQUOI 30 s ET NON 8 s (2026-09-13) ────────────────────────────────
/// Défaut mesuré : mesuré 53-117 ms à froid et 46-63 ms « en cache » pour
/// 1,75-2,30 Mo de réponse. Le TTL de 8 s est PLUS COURT que le rythme
/// d'interaction humain sur cette vue (on bouge une ancre, on revient) : on
/// repayait donc le recalcul complet entre deux gestes, et le « cache » ne
/// servait qu'aux rafales. La fraîcheur n'est PAS portée par le TTL mais par le
/// `mt` du store, qui est dans la clé (voir `SubgraphCacheKey`) : allonger le
/// TTL ne peut donc pas servir une donnée périmée après une écriture — le mtime
/// change, la clé change, l'entrée est différente. Le TTL n'est là que pour
/// borner la mémoire.
const SUBGRAPH_TTL: std::time::Duration = std::time::Duration::from_secs(30);
const SUBGRAPH_MAX_CACHE_BYTES: usize = 4 * 1024 * 1024;

/// Format du corps json-graph servi par `/api/viz/subgraph`.
///
/// ── LA v1 RESTE LE DÉFAUT, ET C'EST DÉLIBÉRÉ (2026-09-15) ─────────────────
///
/// Un client qui ne demande rien reçoit EXACTEMENT ce qu'il recevait avant, au
/// même octet. La v2 (arêtes en colonnes + table de chaînes, ~−63 % de charge
/// mesurés) s'obtient par `?format=json-graph-2`. Un changement de contrat de
/// fil ne s'impose pas à un dashboard qu'on ne controle pas : il s'adopte.
///
/// Tant que le client n'envoie pas le paramètre, la v2 n'est servie à personne —
/// c'est voulu : le travail serveur est prêt, la bascule client est une ligne.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FormatSousGraphe {
    V1,
    V2,
}

impl FormatSousGraphe {
    /// `?format=` — tout ce qui n'est pas reconnu retombe sur la v1 (le défaut)
    /// plutôt que d'échouer : un paramètre inconnu ne doit pas casser une vue.
    fn depuis(valeur: Option<&str>) -> Self {
        match valeur.map(str::trim) {
            Some("json-graph-2") | Some("2") | Some("v2") => Self::V2,
            _ => Self::V1,
        }
    }

    /// Code de cache. La clé DOIT porter le format : sans lui, une demande v2
    /// recevrait le corps v1 déjà en cache — le même piège que la clé d'`arcs`
    /// qui ignorait le rang, puis `limit`.
    fn code(self) -> u8 {
        match self {
            Self::V1 => 0,
            Self::V2 => 1,
        }
    }

    fn serialiser(self, s: &obrain_viz::Subgraph) -> obrain_viz::VizResult<String> {
        match self {
            Self::V1 => obrain_viz::to_json_string_compact(s),
            Self::V2 => obrain_viz::to_json_string_v2(s),
        }
    }
}

#[derive(Clone, Eq, PartialEq, Hash)]
struct SubgraphCacheKey {
    store: std::path::PathBuf,
    centers: String, // ids résolus, ordre préservé
    hops: u8,
    limit: usize,
    overlay: String,
    inbound: bool,
    outbound: bool,
    /// Format du corps (`FormatSousGraphe::code`) : une v1 et une v2 de la même
    /// requête sont deux réponses différentes, donc deux entrées différentes.
    format: u8,
    /// MTIME DU STORE — LA FRAÎCHEUR.
    ///
    /// La clé ne portait QUE la requête : après une écriture dans le store, le
    /// cache servait donc la MÊME réponse, indéfiniment — des données périmées,
    /// alors que la demande porte explicitement sur « de nouvelles données
    /// fraîches ». Le mtime est le seul signal disponible côté serveur pour dire
    /// « la base a changé » : il entre dans la clé, donc une écriture produit une
    /// entrée différente, et la suivante recalcule.
    mt: Option<u64>,
}
/// ── LA VALEUR EST `Arc<str>`, PAS `String` (2026-09-13) ──────────────────
/// Défaut mesuré : `subgraph_cache_get` faisait `body.clone()` SOUS LE VERROU,
/// soit 1,75-2,30 Mo de `memcpy` par appel — pendant que TOUTES les autres
/// requêtes de sous-graphe attendaient le même mutex. Un `Arc<str>` ne copie
/// que le POINTEUR (une incrémentation atomique) : le verrou est rendu
/// immédiatement et les 2 Mo ne sont plus jamais recopiés. La copie qui reste
/// est celle de l'écriture dans la socket, inévitable.
static SUBGRAPH_CACHE: std::sync::OnceLock<
    std::sync::Mutex<
        std::collections::HashMap<
            SubgraphCacheKey,
            (std::time::Instant, std::sync::Arc<str>),
        >,
    >,
> = std::sync::OnceLock::new();
fn subgraph_cache_get(key: &SubgraphCacheKey) -> Option<std::sync::Arc<str>> {
    let cache =
        SUBGRAPH_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let map = verrou(&cache);
    if let Some((t, body)) = map.get(key) {
        if t.elapsed() < SUBGRAPH_TTL {
            // Clone d'`Arc` — AUCUNE copie d'octets, et le verrou est rendu ici.
            return Some(body.clone());
        }
    }
    None
}
fn subgraph_cache_put(key: SubgraphCacheKey, body: &str) {
    if body.len() > SUBGRAPH_MAX_CACHE_BYTES {
        return;
    }
    let cache =
        SUBGRAPH_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut map = verrou(&cache);
    map.insert(key, (std::time::Instant::now(), std::sync::Arc::from(body)));
    if map.len() > 128 {
        map.retain(|_, (t, _)| t.elapsed() < SUBGRAPH_TTL);
    }
}

/// Resultat de la preparation d'un subgraph, partage entre le HTTP one-shot
/// (`api_subgraph`) et le streaming WebSocket (`api_subgraph_stream`). Les deux
/// voies passent par le MEME chemin d'extraction : l'identite octet du corps
/// est donc garantie par construction (cf. docs/plan-j9.md, decision archi).
enum PreparedSubgraph {
    /// Erreur (code HTTP, corps JSON deja forme `{"error":...}`).
    Error(u16, String),
    /// Corps compact deja en cache (chaine json-graph v1, partagee sans
    /// copie — NATIF-J7) : re-jeu ~0 ms.
    Cached(std::sync::Arc<str>),
    /// Subgraph frais, style, pret a serialiser (+ la cle de cache a poser).
    Built {
        styled: obrain_viz::Subgraph,
        cache_key: SubgraphCacheKey,
    },
}

/// Prepare le subgraph (parse + resolution + extraction + overlay) SANS ecrire
/// dans le `stream` : renvoie l'issue pour que l'appelant serialise (HTTP) ou
/// streame par chunks (WS). Toute la logique de `/api/viz/subgraph` vit ici.
fn prepare_subgraph(
    query: Option<&str>,
    config: &ServeConfig,
    format: FormatSousGraphe,
) -> PreparedSubgraph {
    let params = parse_query(query);
    let Some(store) = config.resolve(params.get("store").map(String::as_str)) else {
        return PreparedSubgraph::Error(
            400,
            r#"{"error":"store requis (paramètre store, preset ou --store/--preset-default)"}"#
                .to_string(),
        );
    };
    let centers_raw = params.get("centers").or_else(|| params.get("center"));
    let Some(centers_raw) = centers_raw else {
        return PreparedSubgraph::Error(
            400,
            r#"{"error":"center (ou centers, séparés par des virgules) requis"}"#.to_string(),
        );
    };
    let centers: Vec<String> = centers_raw
        .split(',')
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect();
    if centers.is_empty() {
        return PreparedSubgraph::Error(400, r#"{"error":"center vide"}"#.to_string());
    }
    // L'engine vient de CONSULTER ces nœuds : on l'inscrit au journal
    // d'activation avant même de servir la réponse (c'est la consultation, pas
    // la réussite de l'extraction, qui compte).
    crate::activation::record(&store, &centers, "subgraph");
    if !store.exists() {
        return PreparedSubgraph::Error(
            400,
            format!("{{\"error\":\"store introuvable : {}\"}}", store.display()),
        );
    }
    let db = match cached_ro(&store) {
        Ok(db) => db,
        Err(e) => {
            return PreparedSubgraph::Error(400, format!("{{\"error\":\"ouverture RO : {e}\"}}"));
        }
    };
    let hops = params
        .get("hops")
        .and_then(|h| h.parse::<usize>().ok())
        .unwrap_or(1)
        .min(8) as u8; // 0 = arbre complet (closure), 1..=8 voisinage
    // (2026-09-09) La closure (`hops = 0`) sur gros store était REFUSÉE au-delà
    // de 1500 nœuds (erreur dure → l'UI « échelle ∞ » n'affichait rien). Ce
    // refus datait du temps où chaque niveau coûtait ~0,4 s en scan non indexé.
    // Depuis le seek IN multi-seed, un niveau coûte des ms : la closure bornée
    // est mesurée ~50-80 ms / 1000 nœuds sur le kernel. On ne refuse donc plus :
    // la limite est plafonnée (réponse `truncated`), jamais en erreur.
    let is_huge = db
        .substrate_handle()
        .is_some_and(|st| st.node_count_live_estimate() > 400_000);
    let raw_limit = params
        .get("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(500);
    // Plafond de sécurité unique (réponse HTTP bornée, layout navigateur
    // raisonnable) ; le dashboard densifie par paliers (défaut 160-900) et
    // peut remonter jusqu'ici par la limite « nœuds max ».
    // ── PAS DE PLAFOND ARBITRAIRE ───────────────────────────────────────────
    // 20 000 / 50 000 étaient des bornes FIXES : elles coupaient la donnée sans
    // que rien ne le dise autrement que par `truncated`. La seule borne qui reste
    // est celle qui protège le navigateur d'un épuisement mémoire — et elle est
    // si haute qu'aucun store réel ne l'atteint par accident. Le client demande
    // ce qu'il veut ; la réponse DIT si elle est tronquée.
    const PLAFOND_TRANSPORT: usize = 500_000;
    let limit = raw_limit.min(PLAFOND_TRANSPORT);
    let overlay = match params.get("overlay").map(String::as_str) {
        Some("ricci") => Some(obrain_viz::Overlay::Ricci),
        Some("energy") => Some(obrain_viz::Overlay::Energy),
        Some("roles") => Some(obrain_viz::Overlay::Roles),
        Some("fsrs") => Some(obrain_viz::Overlay::Fsrs),
        Some("engrams") => Some(obrain_viz::Overlay::Engrams),
        Some("pheromone") => Some(obrain_viz::Overlay::Pheromone),
        Some("tier") => Some(obrain_viz::Overlay::Tier),
        _ => None,
    };
    // ── TYPES D'ARÊTES : PARITÉ AVEC L'OUTIL MCP ────────────────────────────
    // L'outil que l'IA appelle (`viz_subgraph`) accepte `edge_types` depuis
    // toujours ; l'endpoint HTTP ne l'exposait PAS. Le moteur avait donc une
    // meilleure projection que l'écran — alors que la projection mesurée est
    // dominée par la CONTENANCE : 76,3 % `CONTAINS_FILE`, 22 % `DEFINES`, et
    // 4 `CALLS` sur 14 509 arêtes. Autrement dit un ARBRE DE FICHIERS là où on
    // attend une topologie de relations. On rétablit la parité : liste séparée
    // par des virgules, vide = tous les types.
    let types_filtre: Option<Vec<String>> = params
        .get("edge_types")
        .map(|t| {
            t.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty());

    // Sur les TRÈS gros stores, l'expansion ENTRANTE scanne tout le graphe à
    // chaque planche (pas d'index entrant persistant) → on reste en SORTANTE
    // (indexée par nœud) sauf demande explicite inbound/outbound.
    let want_in = params.get("inbound").map(|v| v != "0");
    let want_out = params.get("outbound").map(|v| v != "0");
    let huge = db
        .substrate_handle()
        .is_some_and(|st| st.node_count_live_estimate() > 400_000);
    let outbound = want_out.unwrap_or(true);
    let inbound = if want_in.is_none() && huge {
        false
    } else {
        want_in.unwrap_or(true)
    };
    // ── Cache court : re-jeu des mêmes requêtes ~0 ms (bascules d'échelle,
    //    retours d'ancres, overlays) sans repayer résolution + extraction.
    let overlay_raw = params
        .get("overlay")
        .cloned()
        .unwrap_or_default();
    let cache_key = SubgraphCacheKey {
        store: store.clone(),
        centers: centers.join(","),
        hops,
        limit,
        // Le filtre de types entre dans la clé : sans lui, une requête filtrée
        // servirait la réponse NON filtrée déjà en cache — le même piège que la
        // clé de l'agrégat `arcs` qui ignorait le rang.
        overlay: format!(
            "{overlay_raw}#{}",
            types_filtre.as_ref().map_or(String::new(), |v| v.join(","))
        ),
        inbound,
        outbound,
        format: format.code(),
        // La fraîcheur fait partie de l'identité de la réponse.
        mt: store_mtime(&store).map(|m| m as u64),
    };
    if let Some(hit) = subgraph_cache_get(&cache_key) {
        return PreparedSubgraph::Cached(hit);
    }
    // ── Résolution mémoïsée des centres : le repli suffixe (ENDS WITH)
    //    scanne tout le store sur les gros dépôts (1,2–3,4 s sur 1,55 M
    //    nœuds) ; id numérique / qn exact sont ~0 ms. Une fois résolu, le
    //    centre reste stable : les densifications suivantes ne rescannent pas.
    let mut resolved_centers: Vec<String> = Vec::with_capacity(centers.len());
    for c in &centers {
        if c.parse::<u64>().is_ok() {
            resolved_centers.push(c.clone());
            continue;
        }
        if let Some(id) = resolve_memo(&store, c) {
            resolved_centers.push(id.to_string());
            continue;
        }
        // (NATIF-J5C, cible 2) RESOLUTION STRICTE : id numerique ou
        // qualified_name EXACT seulement. Le repli par suffixe/nom
        // (resolve_node_opts auto_suffix) resolvait silencieusement un centre
        // absent sur un noeud arbitraire, puis lancait une expansion couteuse
        // sur le mauvais noeud. Un centre non resolu rend 400, vite.
        match crate::viz_extract::resolve_center_strict(&db, c) {
            Ok(id) => {
                resolve_memo_put(&store, c, id.0);
                resolved_centers.push(id.0.to_string());
            }
            Err(_) => {
                return PreparedSubgraph::Error(
                    400,
                    format!("{{\"error\":\"centre introuvable: {c}\"}}"),
                );
            }
        }
    }
    let opts = ExtractOptions {
        hops,
        // Closure (`hops = 0`) : borne de profondeur — une composante entière
        // (ex. graphe d'appels du kernel) ne doit pas descendre indéfiniment ;
        // la réponse sera marquée `truncated`, jamais en erreur.
        closure_max_depth: (hops == 0).then_some(16),
        limit_nodes: (limit > 0).then_some(limit),
        signals: true,
        outbound,
        inbound,
        auto_suffix: true, // visualisation : jamais d'échec sur suffixe ambigu
        edge_types: types_filtre,
        ..Default::default()
    };
    let (sg, _rep) = match crate::viz_extract::extract_subgraph_centers(&db, &resolved_centers, &opts) {
        Ok(ok) => ok,
        Err(e) => {
            return PreparedSubgraph::Error(400, format!("{{\"error\":\"extraction : {e:#}\"}}"));
        }
    };
    let styled = match overlay {
        Some(o) => obrain_viz::apply_overlay(&sg, o).0,
        None => sg,
    };
    PreparedSubgraph::Built { styled, cache_key }
}

/// `GET /api/viz/subgraph` — corps JSON one-shot (HTTP, gzip negocie J7).
/// Comportement byte-identique a l'ancienne implementation : `prepare_subgraph`
/// n'a fait qu'extraire la logique commune.
fn api_subgraph(stream: &mut TcpStream, query: Option<&str>, config: &ServeConfig) -> Result<()> {
    // Le format se lit dans la requête, et `prepare_subgraph` le porte dans sa
    // clé de cache : un corps v1 ne peut donc pas être servi à une demande v2.
    let format = FormatSousGraphe::depuis(parse_query(query).get("format").map(String::as_str));
    match prepare_subgraph(query, config, format) {
        PreparedSubgraph::Error(code, body) => respond(stream, code, VIZ_JSON, &body),
        PreparedSubgraph::Cached(doc) => respond(stream, 200, VIZ_JSON, &doc),
        PreparedSubgraph::Built { styled, cache_key } => {
            let doc = format
                .serialiser(&styled)
                .unwrap_or_else(|e| format!("{{\"error\":\"sérialisation : {e}\"}}"));
            if !doc.starts_with("{\"error\"") {
                subgraph_cache_put(cache_key, &doc);
            }
            respond(stream, 200, VIZ_JSON, &doc)
        }
    }
}

/// Taille d'un lot de noeuds/aretes pousse par frame WS. 512 : la 1re frame
/// (begin) est immediate, la 1re frame de donnees est petite (time-to-first-node
/// ~ms), et les lots suivants restent sous le seuil de fragmentation TCP usuel.
const WS_CHUNK: usize = 512;

/// `GET /api/viz/subgraph/stream` (upgrade WebSocket) — MEME subgraph que
/// `/api/viz/subgraph`, pousse par CHUNKS au lieu d'un seul corps (NATIF-J9).
///
/// Voie ADDITIONNELLE : le HTTP one-shot reste le defaut et le repli. L'identite
/// est garantie par construction — meme `prepare_subgraph`, meme ordre canonique,
/// memes stats. Le client accumule {begin, nodes*, edges*, end} et reconstruit
/// exactement le corps HTTP.
/// Resout le sous-graphe style pour le streaming WS. Reutilise EXACTEMENT le
/// chemin HTTP (`prepare_subgraph`) — identite par construction — et garde la
/// parite du cache HTTP (le `Built` pose le corps compact en cache). Sur cache
/// touche, relit le doc (from_json_str) pour obtenir le `Subgraph`.
fn subgraph_for_stream(
    query: Option<&str>,
    config: &ServeConfig,
) -> std::result::Result<obrain_viz::Subgraph, (u16, String)> {
    // ── LA VOIE WS FORCE LA v1, ET CE N'EST PAS UN OUBLI ────────────────────
    // Le contrat du streaming est `{begin, nodes*, edges*, end}` accumulés par
    // le client pour reconstruire EXACTEMENT le corps HTTP one-shot : ses
    // frames portent des `VizNode`/`VizEdge`, jamais des colonnes. Servir de la
    // v2 ici ne « fonctionnerait » pas à moitié — le client relirait le corps
    // avec `from_json_str` (lecteur v1) et échouerait à le décoder. La v2 est
    // donc réservée au HTTP one-shot tant que le flux n'a pas son propre format.
    match prepare_subgraph(query, config, FormatSousGraphe::V1) {
        PreparedSubgraph::Error(code, body) => Err((code, body)),
        PreparedSubgraph::Cached(doc) => obrain_viz::from_json_str(&doc)
            .map_err(|e| (500, format!("{{\"error\":\"cache illisible: {e}\"}}"))),
        PreparedSubgraph::Built { styled, cache_key } => {
            let doc = obrain_viz::to_json_string_compact(&styled)
                .unwrap_or_else(|e| format!("{{\"error\":\"sérialisation : {e}\"}}"));
            if !doc.starts_with("{\"error\"") {
                subgraph_cache_put(cache_key, &doc);
            }
            Ok(styled)
        }
    }
}

/// Delta entre deux etats d'un sous-graphe (NATIF-J9 priorite 2). Les noeuds
/// AJOUTES/CHANGES portent leur VizNode COMPLET (upsert par id cote client) ;
/// les retraits ne portent que la cle. Identite d'arete par (src,dst,type).
struct StreamDelta {
    added_nodes: Vec<obrain_viz::VizNode>,
    removed_nodes: Vec<String>,
    added_edges: Vec<obrain_viz::VizEdge>,
    removed_edges: Vec<(String, String, String)>,
}

impl StreamDelta {
    fn is_empty(&self) -> bool {
        self.added_nodes.is_empty()
            && self.removed_nodes.is_empty()
            && self.added_edges.is_empty()
            && self.removed_edges.is_empty()
    }
    fn count(&self) -> usize {
        self.added_nodes.len()
            + self.removed_nodes.len()
            + self.added_edges.len()
            + self.removed_edges.len()
    }
}

/// Calcule le delta `old -> new`. Un noeud present dans les deux mais CHANGE
/// (champs differents) est re-emis comme ajout (upsert) : appliquer le delta
/// sur l'etat client reconstruit EXACTEMENT `new`.
fn compute_delta(old: &obrain_viz::Subgraph, new: &obrain_viz::Subgraph) -> StreamDelta {
    use std::collections::HashMap;
    let old_by_id: HashMap<&str, &obrain_viz::VizNode> =
        old.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let new_ids: std::collections::HashSet<&str> =
        new.nodes.iter().map(|n| n.id.as_str()).collect();
    let added_nodes = new
        .nodes
        .iter()
        .filter(|n| old_by_id.get(n.id.as_str()).map_or(true, |o| *o != *n))
        .cloned()
        .collect();
    let removed_nodes = old
        .nodes
        .iter()
        .filter(|n| !new_ids.contains(n.id.as_str()))
        .map(|n| n.id.clone())
        .collect();
    let ekey = |e: &obrain_viz::VizEdge| (e.src.clone(), e.dst.clone(), e.edge_type.clone());
    let old_ekeys: std::collections::HashSet<(String, String, String)> =
        old.edges.iter().map(ekey).collect();
    let new_ekeys: std::collections::HashSet<(String, String, String)> =
        new.edges.iter().map(ekey).collect();
    let added_edges = new
        .edges
        .iter()
        .filter(|e| !old_ekeys.contains(&ekey(e)))
        .cloned()
        .collect();
    let removed_edges = old
        .edges
        .iter()
        .map(ekey)
        .filter(|k| !new_ekeys.contains(k))
        .collect();
    StreamDelta {
        added_nodes,
        removed_nodes,
        added_edges,
        removed_edges,
    }
}

/// Sonde de l'epoque du store (mtime) — meme signal que la cle du cache viz.
const WS_LIVE_POLL: std::time::Duration = std::time::Duration::from_millis(400);
/// Battement de coeur : detecte la deconnexion (echec d'ecriture) meme sans
/// changement, et borne la latence de detection.
const WS_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(15);
/// Duree max d'un abonnement (pas de guetteur > 10 min). Au-dela, `bye` +
/// fermeture ; le client se reconnecte avec `since=<epoch>` (insync, zero re-fetch).
const WS_LIVE_MAX: std::time::Duration = std::time::Duration::from_secs(300);
/// Backpressure : au-dela de ce volume de delta, on RE-SNAPSHOT plutot que de
/// pousser un delta enorme (le client trop en retard repart d'un etat complet).
const WS_RESNAPSHOT_DELTA_MAX: usize = 4096;

fn api_subgraph_stream(
    stream: &mut TcpStream,
    entetes: &str,
    query: Option<&str>,
    config: &ServeConfig,
) -> Result<()> {
    // Handshake d'abord : si ce n'est pas un upgrade WS valide, `handshake` a
    // deja repondu 400 et on s'arrete (le client HTTP normal ne vient pas ici).
    if !crate::viz_ws::handshake(stream, entetes)? {
        return Ok(());
    }
    let params = parse_query(query);
    let live = params.get("live").map(|v| v != "0").unwrap_or(false);
    let since = params.get("since").cloned();
    // Epoque = mtime du store (le meme signal de fraicheur que la cle du cache).
    let store = config.resolve(params.get("store").map(String::as_str));
    let epoch = || store.as_ref().and_then(|s| store_mtime(s)).unwrap_or(0);

    // Reconnexion : si le client est DEJA a l'epoque courante, aucun re-fetch —
    // on confirme `insync` et on passe direct en live (le coeur du « pas de
    // re-fetch a chaque interaction »).
    let cur = epoch();
    let deja_sync = live && since.as_deref() == Some(cur.to_string().as_str());

    let mut baseline: Option<obrain_viz::Subgraph> = None;
    if deja_sync {
        crate::viz_ws::send_text(stream, &format!("{{\"t\":\"insync\",\"epoch\":{cur}}}"))?;
    } else {
        // Snapshot initial (begin/nodes*/edges*/end, priorise par importance).
        match subgraph_for_stream(query, config) {
            Err((_, body)) => {
                crate::viz_ws::send_text(stream, &format!("{{\"t\":\"error\",\"detail\":{body}}}"))?;
                crate::viz_ws::send_close(stream)?;
                return Ok(());
            }
            Ok(sg) => {
                stream_subgraph(stream, &sg)?;
                baseline = Some(sg);
            }
        }
    }

    if live {
        live_loop(stream, query, config, &epoch, cur, baseline)?;
    }
    crate::viz_ws::send_close(stream)?;
    Ok(())
}

/// Boucle d'abonnement vivant : le WS reste OUVERT, le serveur POUSSE les deltas
/// quand l'epoque (mtime) change. Bornee (heartbeat + duree max) ; toute erreur
/// d'ecriture (client parti) rompt la boucle. Backpressure : delta trop gros ->
/// re-snapshot complet.
fn live_loop(
    stream: &mut TcpStream,
    query: Option<&str>,
    config: &ServeConfig,
    epoch: &dyn Fn() -> u128,
    mut last_epoch: u128,
    mut baseline: Option<obrain_viz::Subgraph>,
) -> Result<()> {
    crate::viz_ws::send_text(stream, &format!("{{\"t\":\"live\",\"epoch\":{last_epoch}}}"))?;
    let debut = std::time::Instant::now();
    let mut dernier_battement = std::time::Instant::now();
    while debut.elapsed() < WS_LIVE_MAX {
        std::thread::sleep(WS_LIVE_POLL);
        let ep = epoch();
        if ep != last_epoch {
            last_epoch = ep;
            // L'epoque a bouge : recalcule le sous-graphe (memes params) et diff.
            let neuf = match subgraph_for_stream(query, config) {
                Ok(sg) => sg,
                Err(_) => continue, // transitoire (store en cours d'ecriture) : on retentera
            };
            match &baseline {
                Some(base) => {
                    let delta = compute_delta(base, &neuf);
                    if delta.is_empty() {
                        // Rien de visible : on note juste l'epoque.
                        crate::viz_ws::send_text(
                            stream,
                            &format!("{{\"t\":\"epoch\",\"epoch\":{ep}}}"),
                        )?;
                    } else if delta.count() > WS_RESNAPSHOT_DELTA_MAX {
                        // Backpressure : trop de changements -> re-snapshot complet.
                        crate::viz_ws::send_text(
                            stream,
                            &format!("{{\"t\":\"resnapshot\",\"epoch\":{ep}}}"),
                        )?;
                        stream_subgraph(stream, &neuf)?;
                    } else {
                        envoyer_delta(stream, ep, &delta)?;
                    }
                }
                None => {
                    // Etait insync sans baseline : premier changement -> snapshot.
                    crate::viz_ws::send_text(
                        stream,
                        &format!("{{\"t\":\"resnapshot\",\"epoch\":{ep}}}"),
                    )?;
                    stream_subgraph(stream, &neuf)?;
                }
            }
            baseline = Some(neuf);
            dernier_battement = std::time::Instant::now();
        } else if dernier_battement.elapsed() >= WS_HEARTBEAT {
            // Battement : detecte la deconnexion (echec d'ecriture -> `?` rompt).
            crate::viz_ws::send_text(stream, &format!("{{\"t\":\"heartbeat\",\"epoch\":{ep}}}"))?;
            dernier_battement = std::time::Instant::now();
        }
    }
    crate::viz_ws::send_text(stream, &format!("{{\"t\":\"bye\",\"epoch\":{last_epoch}}}"))?;
    Ok(())
}

/// Emet un delta (noeuds ajoutes/changes complets, retraits par cle).
fn envoyer_delta(stream: &mut TcpStream, epoch: u128, delta: &StreamDelta) -> Result<()> {
    let an = serde_json::to_string(&delta.added_nodes).unwrap_or_else(|_| "[]".into());
    let ae = serde_json::to_string(&delta.added_edges).unwrap_or_else(|_| "[]".into());
    let rn = serde_json::to_string(&delta.removed_nodes).unwrap_or_else(|_| "[]".into());
    let re: Vec<serde_json::Value> = delta
        .removed_edges
        .iter()
        .map(|(s, d, t)| serde_json::json!({"src": s, "dst": d, "type": t}))
        .collect();
    let re = serde_json::to_string(&re).unwrap_or_else(|_| "[]".into());
    crate::viz_ws::send_text(
        stream,
        &format!(
            "{{\"t\":\"delta\",\"epoch\":{epoch},\"added_nodes\":{an},\"removed_nodes\":{rn},\"added_edges\":{ae},\"removed_edges\":{re}}}"
        ),
    )
}

/// Ordre de STREAMING par IMPORTANCE STRUCTURELLE (NATIF-J9, priorite 1).
///
/// Renvoie (ordre des noeuds, ordre des aretes, etiquette d'ordre) sous forme
/// d'INDICES dans `canon` (deja canonicalise, donc deterministe a egalite).
/// Les noeuds les plus importants d'abord — le SQUELETTE (hubs) peint tout de
/// suite, les details suivent. Importance = DEGRE dans le sous-graphe (calcule
/// depuis les aretes, sans toucher l'extracteur fige J5-J8), tie-break energy
/// desc puis id asc (deterministe). Sans aucune arete, repli sur l'ordre
/// canonique (par id).
///
/// L'IDENTITE est preservee : c'est une PERMUTATION des memes noeuds/aretes —
/// l'union recue par le client est identique au corps HTTP, seul l'ORDRE
/// d'arrivee change.
fn importance_order(canon: &obrain_viz::Subgraph) -> (Vec<usize>, Vec<usize>, &'static str) {
    use std::cmp::Ordering;
    // Degre incident par id (src et dst comptent).
    let mut degree: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    for e in &canon.edges {
        *degree.entry(e.src.as_str()).or_default() += 1;
        *degree.entry(e.dst.as_str()).or_default() += 1;
    }
    let deg = |id: &str| degree.get(id).copied().unwrap_or(0);

    let mut node_order: Vec<usize> = (0..canon.nodes.len()).collect();
    let has_structure = !canon.edges.is_empty();
    if has_structure {
        node_order.sort_by(|&a, &b| {
            let na = &canon.nodes[a];
            let nb = &canon.nodes[b];
            deg(&nb.id)
                .cmp(&deg(&na.id)) // degre decroissant : hubs d'abord
                .then_with(|| {
                    nb.energy
                        .unwrap_or(0.0)
                        .partial_cmp(&na.energy.unwrap_or(0.0))
                        .unwrap_or(Ordering::Equal)
                })
                .then_with(|| na.id.cmp(&nb.id)) // deterministe
        });
    }
    // Rang de chaque noeud (par id) dans l'ordre d'importance.
    let mut rank: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::with_capacity(canon.nodes.len());
    for (r, &i) in node_order.iter().enumerate() {
        rank.insert(canon.nodes[i].id.as_str(), r);
    }
    let rk = |id: &str| rank.get(id).copied().unwrap_or(usize::MAX);

    // Aretes : d'abord celles qui touchent les hubs (min rang des extremites),
    // pour que le squelette se relie tot ; tie-break canonique (src,dst,type).
    let mut edge_order: Vec<usize> = (0..canon.edges.len()).collect();
    if has_structure {
        edge_order.sort_by(|&a, &b| {
            let ea = &canon.edges[a];
            let eb = &canon.edges[b];
            let ka = rk(&ea.src).min(rk(&ea.dst));
            let kb = rk(&eb.src).min(rk(&eb.dst));
            ka.cmp(&kb).then_with(|| {
                (&ea.src, &ea.dst, &ea.edge_type).cmp(&(&eb.src, &eb.dst, &eb.edge_type))
            })
        });
    }
    let label = if has_structure {
        "importance:degree"
    } else {
        "canonical"
    };
    (node_order, edge_order, label)
}

/// Streame un subgraph par frames texte JSON. Les noeuds partent par IMPORTANCE
/// decroissante (degre — voir [`importance_order`]) : le squelette peint tout de
/// suite. L'IDENTITE est preservee (permutation) : l'union des chunks est egale
/// au corps HTTP. `stats` = `canon.stats()` (identique a `JsonGraphDoc`).
fn stream_subgraph(stream: &mut TcpStream, sg: &obrain_viz::Subgraph) -> Result<()> {
    let canon = sg.clone().canonical();
    let stats = canon.stats();
    let (node_order, edge_order, order_label) = importance_order(&canon);
    let stats_json = serde_json::to_string(&stats)
        .unwrap_or_else(|_| r#"{"node_count":0,"edge_count":0,"truncated":false}"#.to_string());
    let begin = format!(
        "{{\"t\":\"begin\",\"schema\":\"{}\",\"stats\":{stats_json},\"n_nodes\":{},\"n_edges\":{},\"order\":\"{order_label}\"}}",
        obrain_viz::JSON_GRAPH_SCHEMA_V1,
        canon.nodes.len(),
        canon.edges.len()
    );
    crate::viz_ws::send_text(stream, &begin)?;
    for lot in node_order.chunks(WS_CHUNK) {
        let batch: Vec<&obrain_viz::VizNode> = lot.iter().map(|&i| &canon.nodes[i]).collect();
        let items = serde_json::to_string(&batch).unwrap_or_else(|_| "[]".to_string());
        crate::viz_ws::send_text(stream, &format!("{{\"t\":\"nodes\",\"items\":{items}}}"))?;
    }
    for lot in edge_order.chunks(WS_CHUNK) {
        let batch: Vec<&obrain_viz::VizEdge> = lot.iter().map(|&i| &canon.edges[i]).collect();
        let items = serde_json::to_string(&batch).unwrap_or_else(|_| "[]".to_string());
        crate::viz_ws::send_text(stream, &format!("{{\"t\":\"edges\",\"items\":{items}}}"))?;
    }
    crate::viz_ws::send_text(stream, "{\"t\":\"end\"}")?;
    Ok(())
}

/// `GET /api/stores` — presets exposés (alimente la liste déroulante du viewer).
/// `GET /api/stores` — les bases que l'appelant peut OUVRIR, et rien d'autre.
///
/// ── LA LISTE EST UN DROIT, PAS UN INVENTAIRE ────────────────────────────────
/// Défaut mesuré le 2026-09-15, dès le premier compte ami : cette route annonçait
/// TOUS les presets du serveur. L'interface s'ancre alors sur le premier d'entre
/// eux (`runtime-dev`) — une base que ce compte n'a pas le droit de lire. Tout ce
/// qu'il demande ensuite est refusé (403), la scène reste vide, et il ne voit
/// aucune fenêtre utile : « le compte connecté n'a accès à rien ». Annoncer une
/// base inaccessible, c'est fabriquer ce symptôme.
///
/// On rend donc uniquement les bases dont l'espace est ACCORDÉ à l'appelant,
/// plus les mondes qu'il a lui-même ajoutés (un monde neuf est nommé d'après son
/// dépôt : son espace n'existe pas encore dans les politiques, mais l'ajout lui
/// appartient — c'est déjà la règle d'autorisation d'`/api/worlds`).
///
/// Quand il ne reste RIEN, on le dit et on donne la sortie : un écran vide sans
/// explication se lit comme une panne.
fn api_stores(
    stream: &mut TcpStream,
    entetes: &str,
    query: Option<&str>,
    config: &ServeConfig,
) -> Result<()> {
    let tous: Vec<(String, std::path::PathBuf)> = config
        .presets
        .iter()
        .cloned()
        .chain(
            crate::worlds::enregistres()
                .into_iter()
                .map(|m| (m.label, m.store)),
        )
        .collect();

    // Sans IAM armé, il n'y a pas de droits à appliquer : la liste est entière.
    // (Et si l'IAM est armé, la garde a déjà refusé une requête sans identité.)
    let espace_du_chemin =
        |p: &std::path::Path| -> String { p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default() };

    let mut lus: Vec<serde_json::Value> = Vec::new();
    let mut refuses: Vec<String> = Vec::new();
    let mut espaces: Vec<String> = Vec::new();
    let mut compte: Option<String> = None;

    let session = garde_interieure().and_then(|g| {
        let jeton = jeton_presente(entetes, query).filter(|j| !j.trim().is_empty())?;
        g.authentifier(Some(&jeton)).ok().flatten()
    });

    match (garde_interieure(), session.as_ref()) {
        (Some(g), Some(s)) => {
            compte = Some(s.username().to_string());
            espaces = espaces_de_l_appelant(jeton_presente(entetes, query).as_deref())
                .unwrap_or_default();
            // ── UNE BASE SE LIT SUR SON ESPACE ──────────────────────────────
            // Le nom d'espace d'une base est le nom de son fichier sans
            // `.obrain` — la même règle que `espace_vise`, donc la même
            // décision que celle qu'appliquera la garde à la requête suivante.
            let lisible = |espace: &str| {
                g.controler_action(Some(s), ACTION_LECTURE_HTTP, espace).is_ok()
            };
            // ── ET UN MONDE AJOUTÉ APPARTIENT À SON AUTEUR ──────────────────
            // Un monde neuf est nommé d'après son dépôt : son espace n'existe
            // pas dans les politiques. L'ajout, lui, a été autorisé sur les
            // espaces de l'appelant (règle d'`/api/worlds`), et le registre
            // retient qui l'a fait : c'est cette attribution qui rend le monde
            // lisible par son auteur, et par lui seul.
            let mien = |path: &std::path::Path| {
                crate::worlds::proprietaire_de(path)
                    .is_some_and(|p| p == format!("{}@{}", s.username(), compte_iam()))
            };

            let presets: Vec<(String, std::path::PathBuf)> = config.presets.clone();
            for (label, path) in presets {
                if lisible(&espace_du_chemin(&path)) {
                    lus.push(serde_json::json!({
                        "label": label,
                        "path": path.display().to_string(),
                    }));
                } else {
                    refuses.push(label);
                }
            }
            for (label, path) in crate::worlds::enregistres().into_iter().map(|m| (m.label, m.store)) {
                // Un monde est LISTÉ s'il se lit sur son propre espace, ou s'il
                // est le sien.
                if lisible(&espace_du_chemin(&path)) || mien(&path) {
                    lus.push(serde_json::json!({
                        "label": label,
                        "path": path.display().to_string(),
                        "monde_ajoute": true,
                    }));
                } else {
                    refuses.push(label);
                }
            }
        }
        // IAM armé mais aucune identité : la garde a déjà refusé cette requête
        // avant d'arriver ici. On ne montre rien, plutôt que de montrer tout.
        (Some(_), None) => {}
        // Pas d'IAM : il n'y a pas de droits à appliquer, la liste est entière.
        (None, _) => {
            lus = tous
                .into_iter()
                .map(|(label, path)| {
                    serde_json::json!({ "label": label, "path": path.display().to_string() })
                })
                .collect();
        }
    }

    // Le défaut suit la liste : c'est le premier preset ACCESSIBLE, jamais un
    // preset refusé (sinon l'interface s'ancre sur une base fermée).
    let default_label = config
        .default_label
        .as_ref()
        .filter(|l| lus.iter().any(|p| p["label"] == serde_json::Value::String((*l).clone())))
        .cloned()
        .or_else(|| lus.first().and_then(|p| p["label"].as_str().map(str::to_string)))
        .unwrap_or_default();

    // (2026-09-15) LE CHEMIN PAR DEFAUT SUIT LE LABEL PAR DEFAUT, PAS LE
    // DEFAUT GLOBAL DU SERVEUR. Cause profonde du « permission denied
    // tenant/runtime-dev » : path_default pointait sur le store par defaut du
    // serveur (runtime-dev) meme pour un compte qui ne peut pas le lire ; un
    // bundle qui s ancre sur path_default ouvrait alors une base fermee. On
    // prend le chemin du premier preset ACCESSIBLE (celui de default_label),
    // et rien (null) quand aucune base n est accessible — l interface ne peut
    // plus s ancrer sur un refus.
    let path_default = lus
        .iter()
        .find(|p| p["label"] == serde_json::Value::String(default_label.clone()))
        .or_else(|| lus.first())
        .and_then(|p| p["path"].as_str().map(str::to_string));
    let body = serde_json::json!({
        "default": default_label,
        "presets": lus,
        "path_default": path_default,
        // ── CE QUI EST REFUSÉ SE DIT ────────────────────────────────────────
        // Un compte qui ne voit qu'une base doit pouvoir comprendre POURQUOI les
        // autres ont disparu — sans cela, « il n'y a rien » et « on ne m'a rien
        // accordé » se ressemblent.
        "compte": compte,
        "espaces": espaces,
        "refuses": if refuses.is_empty() { serde_json::Value::Null } else { serde_json::json!(refuses) },
        "aide": "Un monde neuf s'ajoute depuis la fenêtre « Ajouter un monde ». Pour lire une base \
                 existante, demandez à l'administrateur de vous accorder son espace \
                 (nom de la base sans « .obrain »).",
    })
    .to_string();
    respond(stream, 200, VIZ_JSON, &body)
}

/// `GET /api/functions?store=&q=&limit=` — autocomplete de fonctions (filtre
/// `CONTAINS` sur qualified_name, moteur GQL). Réponse : {items:[{id,qn,name}]}.
fn api_functions(stream: &mut TcpStream, query: Option<&str>, config: &ServeConfig) -> Result<()> {
    let params = parse_query(query);
    let Some(store) = config.resolve(params.get("store").map(String::as_str)) else {
        return respond(stream, 400, VIZ_JSON, r#"{"error":"store requis"}"#);
    };
    let Some(q) = params.get("q").filter(|q| !q.is_empty()) else {
        return respond(stream, 400, VIZ_JSON, r#"{"error":"q requis"}"#);
    };
    if q.contains('\'') {
        return respond(stream, 400, VIZ_JSON, r#"{"error":"q invalide"}"#);
    }
    let limit = params
        .get("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(25)
        .min(100);
    // recherche en mémoire (durable par (store, q, limit)) : réitération ~1 ms
    let mt = store_mtime(&store);
    let fextra = format!("{}|{}", q, limit);
    {
        use std::sync::{Mutex, OnceLock};
        static FNC: OnceLock<Mutex<std::collections::HashMap<
            (std::path::PathBuf, String), (Option<u128>, String)
        >>> = OnceLock::new();
        let cache = FNC.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
        let ck = (store.clone(), fextra.clone());
        if let Some((m, b)) = verrou(&cache).get(&ck).map(|(m, b)| (*m, b.clone())) {
            if m == mt { return respond(stream, 200, VIZ_JSON, &b); }
        }
        if let Some(b) = disk_load("fn", &store, &fextra, mt) {
            verrou(&cache).insert(ck, (mt, b.clone()));
            return respond(stream, 200, VIZ_JSON, &b);
        }
    }
    let db = match cached_ro(&store) {
        Ok(db) => db,
        Err(e) => {
            return respond(
                stream,
                400,
                VIZ_JSON,
                &format!("{{\"error\":\"ouverture RO : {e}\"}}"),
            );
        }
    };
    use obrain_common::types::Value as OV;
    // Recherche PAR PALIERS de pertinence (qn exact → nom → suffixe → contient),
    // dédup AVANT plafond : la fenêtre garde les vrais matchs (30 max).
    let tiers = [
        format!("MATCH (n) WHERE n.qualified_name = '{q}'"),
        format!("MATCH (n) WHERE n.name STARTS WITH '{q}'"),
        format!("MATCH (n) WHERE n.qualified_name ENDS WITH '.{q}'"),
        format!("MATCH (n) WHERE n.qualified_name CONTAINS '{q}'"),
    ];
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut items: Vec<serde_json::Value> = Vec::new();
    for t in &tiers {
        let q_full = format!(
            "{t} RETURN id(n), n.qualified_name, n.name, labels(n)[0], n.file_path LIMIT 12"
        );
        let Ok(res) = db.session().execute(&q_full) else { continue };
        for row in res.rows {
            let (Some(OV::Int64(id)), Some(OV::String(qn)), Some(OV::String(name))) =
                (row.first(), row.get(1), row.get(2))
            else {
                continue;
            };
            let ty = match row.get(3) {
                Some(OV::String(x)) => x.to_string(),
                _ => String::new(),
            };
            let fp = match row.get(4) {
                Some(OV::String(x)) => x.to_string(),
                _ => String::new(),
            };
            if !seen.insert(qn.to_string()) {
                continue;
            }
            items.push(serde_json::json!({
                "id": *id as u64,
                "qn": qn.to_string(),
                "name": name.to_string(),
                "type": ty,
                "file_path": fp,
            }));
            if items.len() >= 30 {
                break;
            }
        }
        if items.len() >= 30 {
            break;
        }
    }
    // Une recherche de fonctions est une consultation de l'engine : les
    // résultats trouvés s'inscrivent au journal d'activation.
    let touched: Vec<String> = items
        .iter()
        .filter_map(|i| i.get("qn").and_then(|x| x.as_str()).map(String::from))
        .collect();
    crate::activation::record(&store, &touched, "search");
    let body = serde_json::json!({ "items": items, "total": items.len() }).to_string();
    disk_save("fn", &store, &fextra, mt, &body);
    respond(stream, 200, VIZ_JSON, &body)
}

/// Sert un fichier statique (chemin déjà contrôlé côté caller).

// ── DEV CHANNEL (vite-rs-like) : updates temps réel servis par Rust ─────────
// OBRAIN_DEV=1 active : /__dev/version, injection d'un script de hot-reload,
// et un watcher qui relance `pnpm build` quand src/ change.
fn dev_enabled() -> bool {
    static D: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *D.get_or_init(|| std::env::var("OBRAIN_DEV").map(|v| v != "0").unwrap_or(false))
}
fn pkg_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/obrain-dashboard")
}
fn dev_version() -> String {
    match std::fs::metadata(pkg_dir().join("dist/index.html")) {
        Ok(m) => {
            let t = m.modified().ok()
                .and_then(|d| d.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis()).unwrap_or(0);
            format!("{t}-{}", m.len())
        }
        Err(_) => "0".into(),
    }
}
const DEV_SCRIPT: &str = r#"<script>(function(){let v=null;setInterval(async()=>{try{const r=await fetch('/__dev/version',{cache:'no-store'});const j=await r.json();if(v===null){v=j.version;}else if(j.version!==v){console.log('[obrain dev] rebuild -> reload');location.reload();}}catch(e){}},1000);})();</script>"#;
fn inject_dev(html: &str) -> String {
    if html.contains("</body>") { html.replace("</body>", &format!("{DEV_SCRIPT}</body>")) }
    else { format!("{html}{DEV_SCRIPT}") }
}
fn newest_mtime(dir: &std::path::Path) -> std::time::SystemTime {
    let mut newest = std::time::SystemTime::UNIX_EPOCH;
    let Ok(rd) = std::fs::read_dir(dir) else { return newest };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() { let m = newest_mtime(&p); if m > newest { newest = m; } }
        else if let Ok(m) = e.metadata().and_then(|md| md.modified()) { if m > newest { newest = m; } }
    }
    newest
}
fn spawn_dev_watcher() {
    if !dev_enabled() { return; }
    std::thread::spawn(|| {
        let pkg = pkg_dir();
        let src = pkg.join("src");
        let mut last = newest_mtime(&src);
        eprintln!("[dev] watcher actif sur {} (rebuild auto + reload)", src.display());
        loop {
            std::thread::sleep(std::time::Duration::from_millis(1200));
            let now = newest_mtime(&src);
            if now > last {
                last = now;
                eprintln!("[dev] changement détecté -> pnpm build");
                match std::process::Command::new("pnpm").arg("build").current_dir(&pkg).output() {
                    Ok(o) if o.status.success() => eprintln!("[dev] build OK — reload envoyé"),
                    Ok(o) => eprintln!("[dev] build ÉCHEC : {}",
                        String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("")),
                    Err(e) => eprintln!("[dev] pnpm indisponible : {e}"),
                }
                std::thread::sleep(std::time::Duration::from_millis(800));
                last = newest_mtime(&src);
            }
        }
    });
}

fn static_file(stream: &mut TcpStream, path: &std::path::Path, ctype: &str) -> Result<()> {
    // LE NOM D'UN BUNDLE CONTIENT SON HACHAGE : il change à chaque build, donc on
    // peut le garder indéfiniment. La PAGE, elle, doit être revalidée — c'est
    // elle qui désigne le bundle courant.
    let cache = if path.to_string_lossy().contains("/assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    static_octets(stream, path, ctype, cache)
}

/// Sert un fichier **en octets**, avec son `ETag` dérivé de (longueur, mtime).
///
/// Deux défauts mesurés sont corrigés ici, et ils n'ont rien à voir :
///   * le fichier passait par `String::from_utf8_lossy` : tout actif non-UTF-8
///     (une police, une image, une source map binaire) était servi CORROMPU, et
///     le `Content-Length` était celui du texte perdu — donc un corps de la
///     mauvaise taille, sans que rien ne le signale ;
///   * aucun `ETag` : un fichier inchangé repartait en entier à chaque visite,
///     `index-*.js.map` pesant 5 324 912 octets.
fn static_octets(
    stream: &mut TcpStream,
    chemin: &std::path::Path,
    ctype: &str,
    cache: &str,
) -> Result<()> {
    match std::fs::read(chemin) {
        Ok(octets) => {
            let mtime = std::fs::metadata(chemin).and_then(|m| m.modified()).ok();
            let etag = etag_de(octets.len() as u64, mtime);
            respond_octets(stream, 200, ctype, cache, Some(&etag), &[], &octets)
        }
        Err(_) => respond(stream, 404, "text/plain", "fichier absent"),
    }
}

/// ── L'ASSAINISSEMENT D'UN CHEMIN DE FICHIER ──────────────────────────────
///
/// Reprend — et ÉTEND — la garde que `static_vendor` portait déjà : décoder le
/// pourcent, refuser `..`, `/` initial et `\`. Ce qui est ajouté ici est la
/// vérification qui manquait : le chemin CANONIQUE doit rester SOUS la racine.
/// Les deux verrous ne font pas double emploi — le refus de `..` arrête l'attaque
/// par le NOM, la vérification de préfixe arrête celle qui passe par un LIEN
/// SYMBOLIQUE, où aucun `..` n'apparaît jamais.
fn chemin_sous(racine: &std::path::Path, rel: &str) -> Option<PathBuf> {
    let rel = percent_decode_chemin(rel);
    if rel.is_empty()
        || rel.contains('\0')
        || rel.contains('\\')
        || rel.starts_with('/')
        // Composant par composant, et non « contient .. » : un fichier nommé
        // `index..js` est légitime, et le refuser serait un faux positif qui
        // n'achète aucune sûreté (c'est le COMPOSANT `..` qui remonte).
        || rel.split('/').any(|c| c == ".." || c == ".")
    {
        return None;
    }
    let racine = racine.canonicalize().ok()?;
    let reel = racine.join(&rel).canonicalize().ok()?;
    // `canonicalize` a résolu les liens : si le résultat sort de la racine, c'est
    // un lien qui pointe ailleurs, et il est refusé comme le serait un `..`.
    if reel.starts_with(&racine) {
        Some(reel)
    } else {
        None
    }
}

/// `percent_decode` pour un CHEMIN.
///
/// Pourquoi une variante : dans une chaîne de requête
/// (`application/x-www-form-urlencoded`), `+` vaut un ESPACE — c'est ce que fait
/// `percent_decode`, et c'est juste là. Dans un CHEMIN, la RFC 3986 §3.3 ne
/// connaît pas cette règle : `+` y est un caractère ordinaire, et le transformer
/// en espace chercherait un fichier qui n'existe pas. Les deux usages ont donc
/// deux fonctions, et chacune dit pourquoi.
fn percent_decode_chemin(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                // Décodage sur les OCTETS, et hexadécimal ASCII seulement : voir
                // le long commentaire de `percent_decode` (une coupe en octets
                // sur une `&str` faisait PANIQUER le fil client).
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                if let Some(b) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    out.push(b);
                    i += 2;
                } else {
                    out.push(bytes[i]);
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn mime_of(name: &str) -> &'static str {
    if name.ends_with(".js") { "application/javascript" }
    else if name.ends_with(".mjs") { "application/javascript" }
    else if name.ends_with(".css") { "text/css" }
    else if name.ends_with(".html") { "text/html; charset=utf-8" }
    else if name.ends_with(".svg") { "image/svg+xml" }
    // ── `.map` ÉTAIT DU BINAIRE ───────────────────────────────────────────
    // `index-*.js.map` (5 324 912 octets) partait en
    // `application/octet-stream` : le navigateur ne pouvait donc pas l'afficher
    // dans ses outils, et aucun cache ne le traitait comme du JSON.
    else if name.ends_with(".map") || name.ends_with(".json") { "application/json" }
    else if name.ends_with(".png") { "image/png" }
    else if name.ends_with(".jpg") || name.ends_with(".jpeg") { "image/jpeg" }
    else if name.ends_with(".webp") { "image/webp" }
    else if name.ends_with(".ico") { "image/x-icon" }
    else if name.ends_with(".woff2") { "font/woff2" }
    else if name.ends_with(".woff") { "font/woff" }
    else if name.ends_with(".wasm") { "application/wasm" }
    else if name.ends_with(".txt") { "text/plain; charset=utf-8" }
    else if name.ends_with(".webmanifest") { "application/manifest+json" }
    else { "application/octet-stream" }
}

/// `GET /api/query?store=&query=&language=` — exécution RO (gql|cypher) pour
/// la console « questions au moteur ». Réponse : {columns, rows, execution_time_ms?}.
/// ── LA GÉNÉRATION DU MOTEUR, CALCULÉE UNE FOIS PAR PROCESSUS (2026-09-14) ──
///
/// La clé du cache de requêtes porte une « génération » qui doit changer quand
/// le moteur change : version du binaire, DATE du binaire, et les réglages qui
/// décident du plan. Ses quatre composants sont tous IMMUABLES pendant la vie du
/// processus — le binaire ne se réécrit pas sous lui-même, et un réglage
/// d'environnement ne change pas sans redémarrage.
///
/// Or elle était recalculée À CHAQUE APPEL : `current_exe` (un `readlink`),
/// un `stat` du binaire, deux lectures d'environnement et un `format!` de
/// quatre champs, pour une chaîne de ~40 octets. C'était payer le prix d'une
/// découverte pour une valeur qu'on connaissait déjà.
///
/// `OnceLock` : un calcul par processus, puis un clone de 40 octets. Le
/// raisonnement de sûreté est le même que celui du cache lui-même — une
/// génération ne peut pas être périmée puisqu'elle décrit CE processus.
fn generation_du_processus() -> String {
    use std::sync::OnceLock;
    // ABLATION : `OBRAIN_SURFACE_GENERATION_FROIDE=1` refait le calcul à CHAQUE
    // appel, exactement comme avant ce correctif. C'est ce qui permet de chiffrer
    // le poste `generation` sur le MÊME binaire — un binaire d'époque
    // n'instrumente pas, et deux binaires ne se comparent pas poste à poste.
    if std::env::var_os("OBRAIN_SURFACE_GENERATION_FROIDE").is_some() {
        return calculer_generation();
    }
    static GENERATION: OnceLock<String> = OnceLock::new();
    GENERATION.get_or_init(calculer_generation).clone()
}

/// Le calcul lui-même : quatre composants qui ne bougent qu'au redémarrage.
fn calculer_generation() -> String {
    format!(
        "{}|{}|{}|{}",
        env!("CARGO_PKG_VERSION"),
        std::env::current_exe()
            .ok()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs()),
        std::env::var("OBRAIN_TRIGRAMMES").unwrap_or_default(),
        std::env::var("OBRAIN_TRIGRAMMES_DES_LA").unwrap_or_default(),
    )
}

/// Le cache mémoire des réponses de `/api/query`, par `(store, clé complète)`.
///
/// La valeur gardée est `(mtime du store, corps DÉJÀ prêt à servir)`. Extraire la
/// table du corps de `api_query` n'est pas cosmétique : le point d'ÉCRITURE
/// (après exécution) et le point de LECTURE (avant exécution) doivent désigner la
/// même table, sinon la forme préparée à l'écriture n'est jamais celle qu'on
/// relit — et on repaierait la conversion à chaque appel, en silence.
fn qcache() -> &'static Mutex<HashMap<(PathBuf, String), (Option<u128>, String)>> {
    static QCACHE: std::sync::OnceLock<Mutex<HashMap<(PathBuf, String), (Option<u128>, String)>>> =
        std::sync::OnceLock::new();
    QCACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn api_query(
    stream: &mut TcpStream,
    query: Option<&str>,
    config: &ServeConfig,
) -> Result<()> {
    use obrain_common::types::Value as OV;
    let (params, store) = vent::chrono(vent::PARAMS, || {
        let params = parse_query(query);
        let store = config.resolve(params.get("store").map(String::as_str));
        (params, store)
    });
    let Some(store) = store else {
        return respond(stream, 400, VIZ_JSON, r#"{"error":"store requis"}"#);
    };
    let Some(q) = params.get("query").filter(|s| !s.is_empty()) else {
        return respond(stream, 400, VIZ_JSON, r#"{"error":"query requis"}"#);
    };
    // NB : la requête est exécutée telle quelle (aucune interpolation) —
    // les guillemets simples des littéraux GQL sont donc autorisés.
    let lang = params.get("language").cloned().unwrap_or_else(|| "gql".into());
    // ── LE MÊME DÉPLACEMENT DE LABEL QUE POUR LE MODÈLE ─────────────────────
    // `crate::outils::label_dans_le_motif` est ce qui fait passer la forme
    // `… AND labels(n) CONTAINS 'Class'` de 68,6 ms à 2,4 ms (le planificateur
    // ne reconnaît un label que dans le MOTIF). La couche outil l'appliquait aux
    // requêtes du modèle ; la console du dashboard et les bancs passent par ICI,
    // et mesuraient donc encore 81,4 ms pour la même question. La traduction est
    // AJOUTÉE, jamais retirée : le prédicat reste (redondant), donc la réponse ne
    // peut pas changer. Elle ne s'applique qu'aux dialectes qui la comprennent.
    let q_decalee: String = vent::chrono(vent::CLE, || {
        if lang == "gql" || lang == "cypher" {
            crate::outils::label_dans_le_motif(q)
        } else {
            q.clone()
        }
    });
    let q = &q_decalee;
    // CACHE DURABLE des requêtes lecture (résultat constant tant que le store
    // ne change pas) : exemples console (types/liens…) rescannés à chaque
    // exécution (2,4 s sur runtime) → ~0 ms aux revisites, mémoire + disque.
    let mt = vent::chrono(vent::MTIME, || store_mtime(&store));
    // ── LA GÉNÉRATION FAIT PARTIE DE LA CLÉ (2026-09-14, MESURÉ) ────────────
    //
    // DÉFAUT QUI A COÛTÉ TROIS MESURES FAUSSES EN UNE SOIRÉE. La clé ne
    // dépendait que du (store, langage, texte) et de la date du STORE. Un
    // BINAIRE réinstallé ou un RÉGLAGE changé (`OBRAIN_TRIGRAMMES`,
    // `OBRAIN_TRIGRAMMES_DES_LA`, le préfixe) ne changeaient donc NI la clé NI
    // la réponse : on relisait un corps produit par un AUTRE moteur, en croyant
    // mesurer celui qui tourne. Un cache qui survit à un changement de moteur
    // n'est pas un cache, c'est un mensonge en attente.
    //
    // La génération couvre les deux : la DATE DU BINAIRE (tout redéploiement la
    // change) et les RÉGLAGES QUI DÉCIDENT DU PLAN.
    //
    // ELLE EST CONSTANTE POUR LA VIE DU PROCESSUS, et c'était le défaut : la
    // recalculer à chaque appel payait `current_exe` (un `readlink`), un `stat`
    // du binaire et deux lectures d'environnement — pour une valeur qui ne peut
    // pas changer sans redémarrer le serveur. `generation_du_processus()` la
    // calcule UNE fois (`OnceLock`) et la rend ensuite par clone.
    let generation = vent::chrono(vent::GENERATION, generation_du_processus);
    let qextra = vent::chrono(vent::CLE, || format!("{}|{}|{}", lang, generation, q));
    {
        let cache = qcache();
        let ck = (store.clone(), qextra.clone());
        let servi = vent::chrono(vent::QCACHE, || {
            if let Some((m, b)) = verrou(cache).get(&ck).map(|(m, b)| (*m, b.clone())) {
                if m == mt {
                    return Some(b);
                }
            }
            if let Some(b) = disk_load("q", &store, &qextra, mt) {
                // `sans_mesure` est IDEMPOTENT et n'est payé que sur une entrée
                // relue du DISQUE (une fois par requête et par processus) : la
                // forme servie par la mémoire est déjà la bonne (voir plus bas).
                let b = vent::chrono(vent::SANS_MESURE, || sans_mesure(&b));
                verrou(cache).insert(ck, (mt, b.clone()));
                return Some(b);
            }
            None
        });
        if let Some(b) = servi {
            return respond(stream, 200, VIZ_JSON, &b);
        }
    }
    let db = match vent::chrono(vent::MOTEUR, || cached_ro(&store)) {
        Ok(db) => db,
        Err(e) => {
            return respond(
                stream,
                400,
                VIZ_JSON,
                &format!("{{\"error\":\"ouverture RO : {e}\"}}"),
            );
        }
    };
    let res = vent::chrono(vent::MOTEUR, || match lang.as_str() {
        "cypher" => db.session().execute_cypher(q),
        _ => db.session().execute(q),
    });
    match res {
        Ok(r) => {
            fn val(v: &OV) -> serde_json::Value {
                match v {
                    OV::Int64(i) => serde_json::json!(*i),
                    OV::Float64(f) => serde_json::json!(f),
                    OV::Bool(b) => serde_json::json!(b),
                    OV::String(s) => serde_json::json!(s.to_string()),
                    other => serde_json::json!(other.to_string()),
                }
            }
            let rows: Vec<serde_json::Value> = vent::chrono(vent::MOTEUR, || {
                r.rows
                    .iter()
                    .map(|row| serde_json::Value::Array(row.iter().map(val).collect()))
                    .collect()
            });
            let mut out = serde_json::json!({
                "columns": r.columns,
                "rows": rows,
            });
            // Le moteur a été interrogé : on inscrit la question elle-même, et
            // les valeurs de résultat qui ressemblent à une identité réelle
            // (contiennent un point ou deux-points et sont assez longues). C'est
            // une HEURISTIQUE, assumée : on ne prétend pas savoir quels nœuds la
            // requête a touchés, seulement ce qu'elle a ramené de lisible.
            let mut touched: Vec<String> = vec![format!("query:{}", q.chars().take(80).collect::<String>())];
            for row in r.rows.iter().take(120) {
                for v in row {
                    if let OV::String(s2) = v {
                        let s2 = s2.to_string();
                        if s2.len() > 12 && (s2.contains('.') || s2.contains(':')) && !s2.contains(' ') {
                            touched.push(s2);
                        }
                    }
                }
            }
            vent::chrono(vent::ACTIVATION, || crate::activation::record(&store, &touched, "query"));
            if let Some(ms) = r.execution_time_ms {
                out["execution_time_ms"] = serde_json::json!(ms);
            }
            if let Some(msg) = r.status_message {
                out["status_message"] = serde_json::json!(msg);
            }
            let body2 = vent::chrono(vent::MOTEUR, || out.to_string());
            // ── LA FORME SERVIE PAR LE CACHE EST CALCULÉE ICI, PAS À LA LIVRAISON ──
            //
            // `sans_mesure` re-parsait et re-sérialisait le corps ENTIER à chaque
            // appel servi par le cache : payer une analyse JSON complète (et une
            // ré-écriture) pour retirer UNE clé et en poser une autre. Sur un
            // corps de quelques centaines de kilo-octets, c'était le poste
            // dominant de l'appel — et il était payé à CHAQUE fois, alors que le
            // résultat ne dépend que de la valeur, donc du moment où on l'a
            // calculée. Le corps est donc préparé UNE fois, ici, et la mémoire ne
            // garde QUE la forme servie : rien n'est dupliqué.
            let corps_cache = vent::chrono(vent::MOTEUR, || {
                // ABLATION : reproduire l'ancien comportement (re-sérialisation à
                // chaque livraison) pour chiffrer le poste sur le même binaire.
                if std::env::var_os("OBRAIN_SURFACE_SANS_MESURE_FROID").is_some() {
                    return body2.clone();
                }
                if let Some(o) = out.as_object_mut() {
                    o.remove("execution_time_ms");
                    o.insert("from_cache".to_string(), serde_json::Value::Bool(true));
                }
                out.to_string()
            });
            if corps_cache.len() < 4_000_000 {
                disk_save("q", &store, &qextra, mt, &corps_cache);
            }
            verrou(qcache()).insert((store.clone(), qextra.clone()), (mt, corps_cache));
            respond(stream, 200, VIZ_JSON, &body2)
        }
        Err(e) => respond(
            stream,
            400,
            VIZ_JSON,
            &serde_json::json!({ "error": format!("{e}") }).to_string(),
        ),
    }
}

/// Encodage URL minimal (composant de requête) pour lier les stores en label.
fn qenc(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 16);
    for b in input.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `GET /sitemap.xml` — pages générées à la volée (agrégats chauds/disque) :
/// une URL par store exposé + un niveau de modules/projets par store.
/// Nourrit un crawler / un moteur de recherche sans scan coûteux.
fn api_sitemap_xml(stream: &mut TcpStream, config: &ServeConfig) -> Result<()> {
    const BASE: &str = "http://IP-INTERNE:8432";
    let mut urls: Vec<String> = Vec::new();
    for (label, path) in &config.presets {
        let st = qenc(label);
        urls.push(format!("{BASE}/?store={st}"));
        // modules (cachés disque) → pages projet
        if let Some(b) = disk_load("mod", path, "", store_mtime(path)) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&b) {
                if let Some(mods) = v.get("modules").and_then(|m| m.as_array()) {
                    for m in mods.iter().take(14) {
                        if let Some(name) = m.get("name").and_then(|n| n.as_str()) {
                            if name.is_empty() || name.contains('@') || name.starts_with('0') { continue; }
                            urls.push(format!("{BASE}/?store={st}&dive={}", qenc(name)));
                        }
                    }
                }
            }
        }
    }
    let today = "2026-09-10";
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">");
    for u in urls {
        xml.push_str(&format!("<url><loc>{}</loc><lastmod>{today}</lastmod></url>", u.replace('&', "&amp;")));
    }
    xml.push_str("</urlset>");
    respond(stream, 200, "application/xml; charset=utf-8", &xml)
}

/// `GET /api/viz/status` — état des « bases » : uptime, caches disque par
/// agrégat (modules/arcs/space/query), stores exposés.
fn api_status(stream: &mut TcpStream, config: &ServeConfig) -> Result<()> {
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    let uptime = START.get_or_init(std::time::Instant::now).elapsed().as_secs();
    let dir = std::env::var_os("OBRAIN_VIZ_CACHE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp/obrain-viz-cache"));
    let mut files: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            if let Ok(name) = e.file_name().into_string() {
                if let Some(k) = name.split('-').next() {
                    *files.entry(k.to_string()).or_insert(0) += 1;
                }
            }
        }
    }
    let body = serde_json::json!({
        "uptime_s": uptime,
        "cache_dir": dir,
        "files_by_kind": files,
        "stores_exposes": tous_les_mondes(config).len(),
    }).to_string();
    respond(stream, 200, VIZ_JSON, &body)
}

/// `GET /api/info?store=` — ordre de grandeur du store (contexte visuel).
/// `POST /api/auth/login` — LE PREMIER MAILLON : mot de passe → jeton.
///
/// Rend le jeton dans le corps (pour un script) **et** dans un cookie
/// `HttpOnly` (pour un navigateur, qui ne peut pas poser d'en-tête
/// `Authorization`). Sans ce cookie, la session n'existerait que pour les
/// clients qui savent déjà parler HTTP bas niveau.
fn api_auth_login(stream: &mut TcpStream, entetes: &str, corps: &str) -> Result<()> {
    // ── DEUX CLIENTS, DEUX RÉPONSES ─────────────────────────────────────────
    // Un formulaire HTML ne sait pas lire du JSON : si la requête vient d'un
    // formulaire, on RENVOIE le navigateur vers l'accueil avec son cookie ; un
    // script, lui, reçoit le jeton en JSON. Sans cette distinction, la page de
    // connexion affichait l'objet JSON — techniquement correct, inutilisable.
    let du_formulaire = entetes
        .lines()
        .any(|l| {
            let l = l.trim();
            l.len() > 13
                && l[..13].eq_ignore_ascii_case("content-type:")
                && l[13..].contains("form-urlencoded")
        });
    let garde = GardeIam::partagee();
    if !garde.disponible() {
        return respond(
            stream,
            400,
            VIZ_JSON,
            &serde_json::json!({
                "error": "aucune connexion possible : l'IAM n'est pas armé sur ce serveur",
                "iam": garde.etat(),
            })
            .to_string(),
        );
    }
    // Corps JSON (`{"username":…,"password":…}`) ou formulaire
    // (`username=…&password=…`) : les deux se rencontrent.
    let (mut utilisateur, mut mot_de_passe) = (String::new(), String::new());
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(corps) {
        utilisateur = v.get("username").and_then(|x| x.as_str()).unwrap_or("").to_string();
        mot_de_passe = v.get("password").and_then(|x| x.as_str()).unwrap_or("").to_string();
    } else {
        let params = parse_query(Some(corps));
        utilisateur = params.get("username").cloned().unwrap_or_default();
        mot_de_passe = params.get("password").cloned().unwrap_or_default();
    }
    if utilisateur.trim().is_empty() || mot_de_passe.is_empty() {
        return respond(
            stream,
            400,
            VIZ_JSON,
            r#"{"error":"username et password requis (JSON ou formulaire)"}"#,
        );
    }
    // Durée de session : réglable, 8 h par défaut.
    let ttl = std::env::var("OBRAIN_IAM_TTL_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(8 * 3600);
    match garde.connecter(utilisateur.trim(), &mot_de_passe, ttl) {
        Ok(jeton) => {
            let identite = garde.identite(&jeton).unwrap_or_else(|| utilisateur.clone());
            let cookie = format!(
                "obrain_session={jeton}; HttpOnly; SameSite=Strict; Path=/; Max-Age={ttl}"
            );
            if du_formulaire {
                return respond_entetes(
                    stream,
                    303,
                    "text/plain; charset=utf-8",
                    "no-store",
                    &[("Set-Cookie", &cookie), ("Location", "/")],
                    "connecté — redirection",
                );
            }
            let corps = serde_json::json!({
                "jeton": jeton,
                "identite": identite,
                "ttl_secs": ttl,
                "iam": garde.etat(),
            })
            .to_string();
            // `HttpOnly` : le script de la page ne peut pas lire le cookie (une
            // injection ne peut donc pas l'exfiltrer). `SameSite=Strict` : il
            // n'est pas envoyé depuis un autre site.
            let cookie = format!(
                "obrain_session={jeton}; HttpOnly; SameSite=Strict; Path=/; Max-Age={ttl}"
            );
            respond_entetes(
                stream,
                200,
                VIZ_JSON,
                "no-store",
                &[("Set-Cookie", &cookie)],
                &corps,
            )
        }
        Err(raison) => {
            if du_formulaire {
                return respond_entetes(
                    stream,
                    401,
                    "text/html; charset=utf-8",
                    "no-store",
                    &[("WWW-Authenticate", "Bearer realm=\"obrain\"")],
                    &format!(
                        "<!doctype html><meta charset=utf-8><title>Connexion refusée</title>\
                         <p style=\"font:16px system-ui;padding:2rem\">Connexion refusée : {raison}\
                         <p style=\"padding:0 2rem\"><a href=\"/login\">Réessayer</a>"
                    ),
                );
            }
            respond_entetes(
                stream,
                401,
                VIZ_JSON,
                "no-store",
                &[("WWW-Authenticate", "Bearer realm=\"obrain\"")],
                &serde_json::json!({ "error": raison, "iam": garde.etat() }).to_string(),
            )
        }
    }
}

/// `POST /api/auth/logout` — le DERNIER maillon : le jeton meurt.
fn api_auth_logout(stream: &mut TcpStream, entetes: &str, query: Option<&str>) -> Result<()> {
    let garde = GardeIam::partagee();
    let jeton = jeton_presente(entetes, query);
    let Some(jeton) = jeton else {
        return respond(
            stream,
            400,
            VIZ_JSON,
            r#"{"error":"aucun jeton à révoquer (cookie obrain_session, en-tête, ou ?token=)"}"#,
        );
    };
    match garde.deconnecter(&jeton) {
        Ok(revogue) => respond_entetes(
            stream,
            200,
            VIZ_JSON,
            "no-store",
            // On efface le cookie : une déconnexion qui laisse le cookie en
            // place laisse la session rejouable par le navigateur.
            &[(
                "Set-Cookie",
                "obrain_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0",
            )],
            &serde_json::json!({ "revoked": revogue, "iam": garde.etat() }).to_string(),
        ),
        Err(raison) => respond(
            stream,
            400,
            VIZ_JSON,
            &serde_json::json!({ "error": raison }).to_string(),
        ),
    }
}

/// `POST /api/auth/refresh` — renouveler la session SANS se reconnecter, et en
/// TUANT l'ancien jeton.
fn api_auth_refresh(stream: &mut TcpStream, entetes: &str, query: Option<&str>) -> Result<()> {
    let garde = GardeIam::partagee();
    let Some(jeton) = jeton_presente(entetes, query) else {
        return respond_entetes(
            stream,
            401,
            VIZ_JSON,
            "no-store",
            &[("WWW-Authenticate", "Bearer realm=\"obrain\"")],
            r#"{"error":"aucun jeton à renouveler (cookie obrain_session, en-tête, ou ?token=)"}"#,
        );
    };
    let ttl = std::env::var("OBRAIN_IAM_TTL_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(8 * 3600);
    match garde.renouveler(&jeton, ttl) {
        Ok(Some(nouveau)) => {
            let cookie = format!(
                "obrain_session={nouveau}; HttpOnly; SameSite=Strict; Path=/; Max-Age={ttl}"
            );
            respond_entetes(
                stream,
                200,
                VIZ_JSON,
                "no-store",
                &[("Set-Cookie", &cookie)],
                &serde_json::json!({ "jeton": nouveau, "ttl_secs": ttl }).to_string(),
            )
        }
        Ok(None) => respond_entetes(
            stream,
            401,
            VIZ_JSON,
            "no-store",
            &[("WWW-Authenticate", "Bearer realm=\"obrain\"")],
            r#"{"error":"jeton inconnu ou expiré : reconnectez-vous"}"#,
        ),
        Err(raison) => respond(
            stream,
            400,
            VIZ_JSON,
            &serde_json::json!({ "error": raison }).to_string(),
        ),
    }
}

/// `GET /api/auth/audit?limit=` — la piste, lisible par un opérateur.
///
/// Gardée par l'action NOMMÉE `iam:audit` : une politique peut donc accorder la
/// lecture de la piste sans accorder celle du graphe — et l'inverse.
fn api_auth_audit(
    stream: &mut TcpStream,
    entetes: &str,
    query: Option<&str>,
    config: &ServeConfig,
) -> Result<()> {
    let garde = GardeIam::partagee();
    let espace = espace_vise(config, query);
    let limite = parse_query(query)
        .get("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(100)
        .min(1000);
    if let Err((code, raison)) = garde.controler_nomme(
        jeton_presente(entetes, query).as_deref(),
        "iam:audit",
        &espace,
    ) {
        let corps = serde_json::json!({ "error": raison, "action": "iam:audit", "espace": espace })
            .to_string();
        return respond(stream, code, VIZ_JSON, &corps);
    }
    let evenements = garde.audit(limite);
    let corps = serde_json::json!({
        "evenements": evenements,
        "total": evenements.len(),
        "limite": limite,
    })
    .to_string();
    respond(stream, 200, VIZ_JSON, &corps)
}

/// Un refus nommé, en JSON.
///
/// Un refus qui ne dit pas pourquoi ne se corrige pas : le corps porte la raison,
/// et l'appelant sait s'il doit s'authentifier, viser un autre espace, ou
/// renoncer.
fn refus_iam(stream: &mut TcpStream, code: u16, raison: &str) -> Result<()> {
    let corps = serde_json::json!({ "error": raison }).to_string();
    respond(stream, code, VIZ_JSON, &corps)
}

/// Le code HTTP qui correspond à un refus de la bibliothèque.
///
/// Une distinction utile : « introuvable » (404) et « interdit » (403) ne se
/// corrigent pas de la même façon — on ne cherche pas un compte qu'on n'a pas le
/// droit de voir.
#[cfg(feature = "iam")]
fn code_pour_erreur(e: &obrain_iam::IamError) -> u16 {
    use obrain_iam::IamError as E;
    match e {
        E::ResourceNotFound { .. } => 404,
        E::AccessDenied { .. } | E::PermissionDenied { .. } => 403,
        E::ResourceExists { .. } | E::InvalidParameter { .. } | E::InvalidOrn { .. } => 409,
        _ => 500,
    }
}

/// Le mot de passe fourni par l'appelant, s'il y en a un.
///
/// Absent ou vide, la bibliothèque en ENGENDRE un et le rend une fois : c'est le
/// défaut qu'on veut, parce qu'un mot de passe choisi par un administrateur pour
/// un autre est un mot de passe que deux personnes connaissent.
#[cfg(feature = "iam")]
fn mot_de_passe_fourni(params: &HashMap<String, String>) -> Option<String> {
    params
        .get("mot_de_passe")
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// L'administration plafonnée de l'appelant, ou le refus qui va bien.
///
/// ── DEUX BARRIÈRES, ET LA SECONDE NE FAIT PAS CONFIANCE À LA PREMIÈRE ───────
/// La garde globale a déjà exigé `iam:admin` sur l'espace visé. Celle-ci
/// **déduit** en plus le plafond des droits réels de l'appelant, dans le
/// magasin : une surface qui déclarerait elle-même son plafond pourrait se
/// décrire plus large que ses droits.
///
/// `mutation` distingue un GESTE d'une LECTURE : sur un magasin d'identités
/// ouvert en lecture seule, un geste répondrait « fait » sans rien écrire — le
/// défaut déjà mesuré ici pour la déconnexion. Mieux vaut un refus nommé.
#[cfg(feature = "iam")]
fn administration_appelant(
    entetes: &str,
    query: Option<&str>,
    mutation: bool,
) -> Result<obrain_iam::admin::Administration, (u16, String)> {
    let garde = GardeIam::partagee();
    let Some(magasin) = garde.identites() else {
        return Err((
            503,
            "IAM NON ARMÉ ou magasin d'identités illisible : il n'y a pas d'identités à \
             administrer. `OBRAIN_IAM_STORE=<base>` désigne le magasin d'identités."
                .to_string(),
        ));
    };
    if mutation && !garde.magasin_ecriture() {
        return Err((
            409,
            "magasin d'identités ouvert en LECTURE SEULE : l'administration ne peut pas écrire \
             (une écriture répondrait « fait » sans rien changer)."
                .to_string(),
        ));
    }
    let jeton = jeton_presente(entetes, query)
        .ok_or_else(|| (401, "aucune identité présentée".to_string()))?;
    let identite = garde
        .identite(&jeton)
        .ok_or_else(|| (401, "jeton invalide ou expiré".to_string()))?;
    let acteur = nom_depuis_identite(&identite);
    obrain_iam::admin::Administration::plafonnee_par_l_acteur(magasin, compte_iam(), acteur)
        .map_err(|e| (403, e.to_string()))
}

/// Une opération d'administration rendue en JSON, ou son refus.
#[cfg(feature = "iam")]
fn resultat_iam(
    stream: &mut TcpStream,
    r: obrain_iam::IamResult<serde_json::Value>,
) -> Result<()> {
    match r {
        Ok(valeur) => respond(stream, 200, VIZ_JSON, &valeur.to_string()),
        Err(e) => refus_iam(stream, code_pour_erreur(&e), &e.to_string()),
    }
}

/// `GET|POST /api/iam/…` — l'administration des comptes, PLAFONNÉE.
///
/// ── CE QU'ON NE TROUVE PAS ICI, ET POURQUOI ─────────────────────────────────
/// Ni `amorcer`, ni `emettre_jeton`. Le premier pose `iam:admin` : l'exposer sur
/// une surface réseau reviendrait à laisser toute surface fabriquer son propre
/// administrateur. Le second fabrique un jeton pour autrui sans mot de passe,
/// donc contourne la connexion. Tous deux exigent le HORS BANDE — le CLI, sur un
/// fichier — et refusent d'ailleurs d'eux-mêmes si on les appelle plafonnés : la
/// barrière est dans le noyau, pas seulement dans l'absence de route.
#[cfg(feature = "iam")]
#[allow(clippy::too_many_lines)]
fn api_iam(
    stream: &mut TcpStream,
    path: &str,
    method: &str,
    entetes: &str,
    query: Option<&str>,
    corps: &str,
    _config: &ServeConfig,
) -> Result<()> {
    // Les paramètres viennent de l'URL ET du corps : un formulaire HTML poste son
    // contenu, un script met volontiers tout dans l'URL. Les deux se rencontrent.
    let mut params = parse_query(query);
    params.extend(parse_query(Some(corps)));
    let param = |cle: &str| params.get(cle).cloned().unwrap_or_default();
    let nom = param("nom").trim().to_string();
    let mutation = method != "GET" && method != "HEAD";

    let admin = match administration_appelant(entetes, query, mutation) {
        Ok(a) => a,
        Err((code, raison)) => return refus_iam(stream, code, &raison),
    };

    // Les routes qui VISENT un compte exigent son nom — vérifié une fois, ici,
    // plutôt que dans chacune des dix branches.
    let vise_un_compte = matches!(
        path,
        "/api/iam/utilisateur"
            | "/api/iam/utilisateur/desactiver"
            | "/api/iam/utilisateur/reactiver"
            | "/api/iam/utilisateur/mot-de-passe"
            | "/api/iam/utilisateur/espace"
            | "/api/iam/utilisateur/admin"
            | "/api/iam/utilisateur/jetons"
            | "/api/iam/utilisateur/jetons/revoquer"
            | "/api/iam/export"
            | "/api/iam/effacer"
    );
    if vise_un_compte && nom.is_empty() {
        return refus_iam(stream, 400, "« nom » est requis : il désigne le compte visé");
    }

    match path {
        // ── Qui suis-je, et sur quoi puis-je agir ? ─────────────────────────
        // Aucun droit au-delà d'une identité : on ne lit QUE SOI. C'est ce qui
        // permet à la page de savoir sur quel espace se juger — le deviner, ou
        // le tenir d'un paramètre fourni par l'appelant, serait exactement ce que
        // la garde refuse.
        "/api/iam/moi" => {
            let moi = match admin.montrer_utilisateur(admin.acteur()) {
                Ok(m) => m,
                Err(e) => return refus_iam(stream, code_pour_erreur(&e), &e.to_string()),
            };
            let reponse = serde_json::json!({
                "compte": moi,
                "appelant": admin.acteur(),
                "peut_administrer": !moi.espaces.is_empty(),
            });
            respond(stream, 200, VIZ_JSON, &reponse.to_string())
        }
        // ── La liste, et l'ajout ────────────────────────────────────────────
        "/api/iam/utilisateurs" if mutation => {
            let espaces = espaces_demandes(query, corps);
            let mot_de_passe = mot_de_passe_fourni(&params);
            match admin.ajouter_utilisateur(&nom, &espaces, mot_de_passe) {
                Ok(compte) => {
                    let reponse = serde_json::json!({
                        "compte": compte,
                        "avertissement": "ce mot de passe ne sera plus JAMAIS relisible — le \
                                          magasin n'en garde que l'empreinte bcrypt. Notez-le, \
                                          ou posez-en un autre.",
                    });
                    respond(stream, 200, VIZ_JSON, &reponse.to_string())
                }
                Err(e) => refus_iam(stream, code_pour_erreur(&e), &e.to_string()),
            }
        }
        "/api/iam/utilisateurs" => {
            let comptes = admin.lister_utilisateurs();
            let reponse = serde_json::json!({
                "comptes": comptes,
                "total": comptes.len(),
                "appelant": admin.acteur(),
                "hors_bande": admin.est_hors_bande(),
            });
            respond(stream, 200, VIZ_JSON, &reponse.to_string())
        }
        // ── Les inventaires ─────────────────────────────────────────────────
        "/api/iam/roles" => {
            let roles = admin.lister_roles();
            let reponse = serde_json::json!({ "roles": roles, "total": roles.len() });
            respond(stream, 200, VIZ_JSON, &reponse.to_string())
        }
        "/api/iam/politiques" => {
            let politiques = admin.lister_politiques();
            let reponse =
                serde_json::json!({ "politiques": politiques, "total": politiques.len() });
            respond(stream, 200, VIZ_JSON, &reponse.to_string())
        }
        "/api/iam/audit" => {
            let limite = param("limit")
                .parse::<usize>()
                .ok()
                .filter(|v| *v > 0)
                .unwrap_or(100)
                .min(1000);
            let evenements = admin.piste_audit(limite);
            let reponse = serde_json::json!({
                "evenements": evenements,
                "total": evenements.len(),
                "limite": limite,
            });
            respond(stream, 200, VIZ_JSON, &reponse.to_string())
        }
        // ── La lecture d'un compte ──────────────────────────────────────────
        "/api/iam/utilisateur" => {
            resultat_iam(stream, admin.montrer_utilisateur(&nom).map(|r| serde_json::json!({ "compte": r })))
        }
        "/api/iam/utilisateur/jetons" => resultat_iam(
            stream,
            admin
                .lister_jetons(&nom)
                .map(|jetons| serde_json::json!({ "compte": nom, "jetons": jetons })),
        ),
        "/api/iam/export" => resultat_iam(
            stream,
            admin
                .exporter_utilisateur(&nom)
                .map(|export| serde_json::json!({ "compte": nom, "export": export })),
        ),
        // ── Les gestes sur un compte ────────────────────────────────────────
        "/api/iam/utilisateur/desactiver" => resultat_iam(
            stream,
            admin
                .desactiver_utilisateur(&nom)
                .map(|()| serde_json::json!({ "compte": nom, "statut": "inactive" })),
        ),
        "/api/iam/utilisateur/reactiver" => resultat_iam(
            stream,
            admin
                .reactiver_utilisateur(&nom)
                .map(|()| serde_json::json!({ "compte": nom, "statut": "active" })),
        ),
        "/api/iam/utilisateur/mot-de-passe" => {
            let mot_de_passe = mot_de_passe_fourni(&params);
            resultat_iam(
                stream,
                admin.changer_mot_de_passe(&nom, mot_de_passe).map(|ids| {
                    serde_json::json!({
                        "compte": ids,
                        "avertissement": "mot de passe rendu UNE SEULE FOIS ; le compte devra \
                                          le changer à sa prochaine connexion.",
                    })
                }),
            )
        }
        "/api/iam/utilisateur/espace" => {
            let espace = param("espace").trim().to_string();
            if espace.is_empty() {
                return refus_iam(stream, 400, "« espace » est requis");
            }
            if param("retirer") == "1" {
                resultat_iam(
                    stream,
                    admin.retirer_espace(&nom, &espace).map(|n| {
                        serde_json::json!({
                            "compte": nom, "espace": espace, "politiques_retirees": n,
                        })
                    }),
                )
            } else {
                resultat_iam(
                    stream,
                    admin.accorder_espace(&nom, &espace).map(|()| {
                        serde_json::json!({ "compte": nom, "espace": espace, "accorde": true })
                    }),
                )
            }
        }
        "/api/iam/utilisateur/admin" => {
            let espace = param("espace").trim().to_string();
            if espace.is_empty() {
                return refus_iam(stream, 400, "« espace » est requis");
            }
            if param("retirer") == "1" {
                resultat_iam(
                    stream,
                    admin.retirer_admin(&nom, &espace).map(|()| {
                        serde_json::json!({ "compte": nom, "espace": espace, "admin": false })
                    }),
                )
            } else {
                resultat_iam(
                    stream,
                    admin.promouvoir_admin(&nom, &espace).map(|()| {
                        serde_json::json!({ "compte": nom, "espace": espace, "admin": true })
                    }),
                )
            }
        }
        "/api/iam/utilisateur/jetons/revoquer" => resultat_iam(
            stream,
            admin
                .revoquer_jetons(&nom)
                .map(|n| serde_json::json!({ "compte": nom, "jetons_revoques": n })),
        ),
        // ── Les politiques ──────────────────────────────────────────────────
        "/api/iam/politique/supprimer" => {
            let id = param("id").trim().to_string();
            if id.is_empty() {
                return refus_iam(stream, 400, "« id » est requis : il désigne la politique");
            }
            resultat_iam(
                stream,
                admin
                    .supprimer_politique(&id)
                    .map(|()| serde_json::json!({ "politique": id, "supprimee": true })),
            )
        }
        // ── L'effacement, qui exige d'être assumé ───────────────────────────
        "/api/iam/effacer" => {
            if param("confirme") != "oui" {
                return refus_iam(
                    stream,
                    400,
                    "effacement REFUSÉ : le geste est IRRÉVERSIBLE (compte, rôles, jetons et \
                     données). Ajoutez `confirme=oui` pour l'assumer.",
                );
            }
            resultat_iam(
                stream,
                admin
                    .effacer_utilisateur(&nom)
                    .map(|certificat| serde_json::json!({ "compte": nom, "certificat": certificat })),
            )
        }
        _ => refus_iam(
            stream,
            404,
            &format!("route d'administration inconnue : {path}"),
        ),
    }
}

/// Le repli quand l'IAM n'est pas compilé : la route EXISTE et s'annonce, au lieu
/// de disparaître (une route absente se confond avec une faute de frappe).
#[cfg(not(feature = "iam"))]
fn api_iam(
    stream: &mut TcpStream,
    _path: &str,
    _method: &str,
    _entetes: &str,
    _query: Option<&str>,
    _corps: &str,
    _config: &ServeConfig,
) -> Result<()> {
    refus_iam(
        stream,
        503,
        "IAM NON COMPILÉ — cette construction n'embarque pas l'administration des comptes \
         (construire avec `cargo build --release -p obrain-cli --features ai,iam`).",
    )
}

/// La page de connexion — servie par le serveur, sans dépendance.
///
/// ── POURQUOI ELLE EXISTE ────────────────────────────────────────────────
/// Armer la garde rendait le dashboard inutilisable : la page se charge (les
/// actifs statiques restent publics, exprès), mais ses appels `fetch` recevaient
/// 401 sans qu'aucun écran ne permette d'obtenir un cookie. `fetch` n'envoie pas
/// d'identifiants qu'on ne lui a pas donnés : il faut un formulaire, donc une
/// page. Celle-ci poste en `form-urlencoded` — le serveur sait le lire — et le
/// serveur renvoie le navigateur vers l'accueil avec le cookie posé.
const PAGE_CONNEXION: &str = r#"<!doctype html>
<meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Connexion — obrain</title>
<style>
 body{font:16px system-ui,sans-serif;background:#0f1115;color:#e6e6e6;display:grid;
      place-items:center;min-height:100vh;margin:0}
 form{background:#171a21;padding:2rem;border-radius:12px;min-width:20rem;
      box-shadow:0 8px 30px #0006}
 h1{font-size:1.1rem;margin:0 0 1rem}
 label{display:block;margin:.6rem 0 .2rem;font-size:.85rem;color:#9aa}
 input{width:100%;padding:.55rem;border-radius:6px;border:1px solid #2a2f3a;
       background:#0f1115;color:#e6e6e6}
 button{margin-top:1.1rem;width:100%;padding:.6rem;border:0;border-radius:6px;
        background:#3b82f6;color:#fff;font-weight:600;cursor:pointer}
 p.note{color:#889;font-size:.8rem;margin-top:1rem}
</style>
<h1>Connexion à obrain</h1>
<form method="POST" action="/api/auth/login">
  <label for="u">Utilisateur</label>
  <input id="u" name="username" autocomplete="username" autofocus required>
  <label for="p">Mot de passe</label>
  <input id="p" name="password" type="password" autocomplete="current-password" required>
  <button type="submit">Se connecter</button>
  <p class="note">La session est un cookie <code>HttpOnly</code> : il n'est pas lisible par la page.</p>
</form>
"#;

/// La page d'administration — servie par le serveur, sans dépendance ni build.
///
/// ── POURQUOI UNE COQUILLE, ET PAS UNE PAGE REMPLIE ──────────────────────────
/// `/iam` n'est PAS sous `/api/` : la garde globale ne la couvre donc pas, et une
/// page qui rendrait la liste des comptes côté serveur serait **publique**. La
/// coquille ne contient AUCUNE donnée : elle appelle l'API d'administration, qui
/// est gardée par `iam:admin`. Un inconnu qui ouvre cette page ne lit que du
/// vide — et un 401 qui lui dit d'aller se connecter.
///
/// Le peu de script qu'elle porte est volontairement du JavaScript en clair,
/// sans chaîne de construction : un écran de secours qui exigerait un bundle ne
/// serait pas utilisable le jour où le bundle est cassé.
const PAGE_IAM: &str = r#"<!doctype html>
<meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Administration IAM — obrain</title>
<style>
 body{font:15px system-ui,sans-serif;background:#0f1115;color:#e6e6e6;margin:0;padding:1.5rem}
 h1{font-size:1.15rem;margin:0 0 .3rem} h2{font-size:.95rem;margin:1.6rem 0 .6rem;color:#9aa}
 a{color:#7cb0ff} #etat{color:#9aa;font-size:.85rem;margin-bottom:1rem}
 form{display:flex;gap:.5rem;flex-wrap:wrap;align-items:end;margin:.4rem 0}
 label{font-size:.78rem;color:#9aa;display:block}
 input,select{padding:.4rem;border-radius:6px;border:1px solid #2a2f3a;background:#0f1115;color:#e6e6e6}
 button{padding:.42rem .7rem;border:0;border-radius:6px;background:#3b82f6;color:#fff;font-weight:600;cursor:pointer}
 button.discret{background:#2a2f3a;font-weight:400}
 table{border-collapse:collapse;width:100%;font-size:.88rem}
 th,td{text-align:left;padding:.4rem .5rem;border-bottom:1px solid #21252e}
 th{color:#9aa;font-weight:500}
 code{background:#171a21;padding:.1rem .3rem;border-radius:4px}
 #liste:empty::after{content:'chargement…';color:#667}
</style>
<h1>Administration des comptes</h1>
<div id="etat"></div>

<h2>Nouveau compte</h2>
<form id="f-ajout">
  <div><label for="a-nom">Nom</label><input id="a-nom" required></div>
  <div><label for="a-esp">Espaces (séparés par des virgules)</label><input id="a-esp" placeholder="demo" required></div>
  <div><label for="a-mdp">Mot de passe (vide : engendré)</label><input id="a-mdp"></div>
  <button type="submit">Créer</button>
</form>

<h2>Comptes</h2>
<table><thead><tr><th>Compte</th><th>Statut</th><th>Admin</th><th>Espaces</th><th>Jetons</th><th>Geste</th></tr></thead>
<tbody id="liste"></tbody></table>

<h2>Piste d'audit</h2>
<table><thead><tr><th>Horodatage</th><th>Action</th><th>Acteur</th><th>Cible</th><th>Décision</th></tr></thead>
<tbody id="audit"></tbody></table>

<script>
'use strict';
// Tous les appels passent par l'API GARDÉE : cette page ne détient aucune donnée
// et ne décide de rien. Elle ne fait que montrer ce qu'on lui rend.
// L'espace peut être imposé par l'URL (`/iam?espace=demo`) ; sinon il est demandé
// au serveur.
let ESPACE = new URLSearchParams(location.search).get('espace') || '';
const api = async (chemin, corps) => {
  const options = corps
    ? { method: 'POST', headers: { 'content-type': 'application/x-www-form-urlencoded' }, body: corps }
    : {};
  const r = await fetch(chemin, options);
  if (r.status === 401) throw new Error('non authentifié — <a href="/login">se connecter</a>');
  const t = await r.text();
  let v;
  try { v = JSON.parse(t); } catch (e) { throw new Error(t.slice(0, 200)); }
  if (!r.ok) throw new Error(v.error || ('HTTP ' + r.status));
  return v;
};
const echapper = (s) => String(s == null ? '' : s).replace(/[&<>]/g, (c) => ({'&':'&amp;','<':'&lt;','>':'&gt;'}[c]));
const champ = (objet, cle) => encodeURIComponent(cle) + '=' + encodeURIComponent(objet[cle]);
const poster = (chemin, objet) => {
  const corps = Object.keys(objet).map((c) => champ(objet, c)).join('&');
  return api(chemin, corps).then(charger);
};
const dire = (message, erreur) => {
  document.getElementById('etat').innerHTML = erreur ? '<span style="color:#ff8a8a">' + message + '</span>' : message;
};

async function charger() {
  try {
    // ── L'ESPACE SE DEMANDE, IL NE SE DEVINE PAS ──────────────────────────
    // La garde juge l'administration sur un espace NOMMÉ. La page apprend donc
    // du serveur ceux que l'appelant détient, au lieu de supposer.
    const moi = await api('/api/iam/moi');
    if (!ESPACE) ESPACE = (moi.compte.espaces || [])[0] || '';
    if (!ESPACE) {
      dire('aucun espace à administrer pour <code>' + echapper(moi.appelant) + '</code>', true);
      return;
    }
    const q = '?espace=' + encodeURIComponent(ESPACE);
    const { comptes, appelant } = await api('/api/iam/utilisateurs' + q);
    dire('appelant : <code>' + echapper(appelant) + '</code> — espace <code>' + echapper(ESPACE) +
      '</code> — ' + comptes.length + ' compte(s) — <a href="/login">changer de session</a>');
    document.getElementById('liste').innerHTML = comptes.map((c) => {
      const gestes = [
        ['statut', c.statut === 'active' ? 'désactiver' : 'réactiver', c.statut === 'active' ? 'desactiver' : 'reactiver', {}],
        ['jetons', 'révoquer jetons', 'jetons/revoquer', {}],
        ['mot-de-passe', 'mot de passe', 'mot-de-passe', {}],
      ];
      const boutons = gestes.map(([id, libelle, route]) =>
        '<button class="discret" data-route="' + route + '" data-nom="' + echapper(c.utilisateur) + '">' + libelle + '</button>'
      ).join(' ');
      const espaces = c.espaces.map((e) => '<code>' + echapper(e) + '</code>').join(' ') || '—';
      return '<tr><td>' + echapper(c.utilisateur) +
        (c.doit_changer_mot_de_passe ? ' <span title="mot de passe à changer">&#9888;</span>' : '') +
        '</td><td>' + echapper(c.statut) + '</td><td>' + (c.admin ? 'oui' : 'non') +
        '</td><td>' + espaces + '</td><td>' + c.jetons_vivants + '</td><td>' + boutons + '</td></tr>';
    }).join('');

    const { evenements } = await api('/api/iam/audit?limit=25&espace=' + encodeURIComponent(ESPACE));
    document.getElementById('audit').innerHTML = evenements.map((e) =>
      '<tr><td>' + echapper(e.timestamp) + '</td><td>' + echapper(e.action) + '</td><td>' +
      echapper(nom_orn(e.principal)) + '</td><td>' + echapper(nom_orn(e.resource)) +
      '</td><td>' + echapper(e.result) + '</td></tr>').join('');
  } catch (e) {
    dire(e.message, true);
  }
}

// Un ORN se sérialise selon la bibliothèque : on accepte la chaîne comme l'objet,
// pour qu'un changement de forme ne casse pas l'écran.
const nom_orn = (o) => (typeof o === 'string')
  ? o.split('/').pop()
  : (o && (o.resource_path || o.path) ? (o.resource_path || o.path) : '');

// Les gestes de ligne : un écouteur DÉLÉGUÉ, pour que la table puisse être
// reconstruite après chaque geste sans rien réattacher.
document.getElementById('liste').addEventListener('click', (ev) => {
  const b = ev.target.closest('button[data-route]');
  if (!b) return;
  poster('/api/iam/utilisateur/' + b.dataset.route, { nom: b.dataset.nom, espace: ESPACE })
    .catch((e) => dire(e.message, true));
});

document.getElementById('f-ajout').addEventListener('submit', (ev) => {
  ev.preventDefault();
  const nom = document.getElementById('a-nom').value.trim();
  const espaces = document.getElementById('a-esp').value.split(',')
    .map((s) => s.trim()).filter(Boolean);
  const corps = ['nom=' + encodeURIComponent(nom)]
    .concat(espaces.map((e) => 'espace=' + encodeURIComponent(e)));
  const mdp = document.getElementById('a-mdp').value;
  if (mdp) corps.push('mot_de_passe=' + encodeURIComponent(mdp));
  api('/api/iam/utilisateurs', corps.join('&'))
    .then((r) => {
      const rendu = (r.compte && r.compte.mot_de_passe) ? ' — mot de passe : ' + r.compte.mot_de_passe : '';
      dire('compte créé : <code>' + echapper(nom) + '</code>' + echapper(rendu));
      document.getElementById('f-ajout').reset();
      charger();
    })
    .catch((e) => dire(e.message, true));
});

charger();
</script>
"#;

/// `GET /api/auth/whoami` — à qui parle-t-on ? (`anonyme` sinon)
fn api_auth_whoami(stream: &mut TcpStream, entetes: &str, query: Option<&str>) -> Result<()> {
    let garde = GardeIam::partagee();
    let identite = jeton_presente(entetes, query)
        .as_deref()
        .and_then(|j| garde.identite(j));
    let corps = serde_json::json!({
        "identite": identite,
        "authentifie": identite.is_some(),
        "iam": garde.etat(),
        "iam_armee": garde.est_armee(),
    })
    .to_string();
    respond(stream, 200, VIZ_JSON, &corps)
}

fn api_info(stream: &mut TcpStream, query: Option<&str>, config: &ServeConfig) -> Result<()> {
    let params = parse_query(query);
    let Some(store) = config.resolve(params.get("store").map(String::as_str)) else {
        return respond(stream, 400, VIZ_JSON, r#"{"error":"store requis"}"#);
    };
    let db = match cached_ro(&store) {
        Ok(db) => db,
        Err(e) => {
            return respond(
                stream,
                400,
                VIZ_JSON,
                &format!("{{\"error\":\"ouverture RO : {e}\"}}"),
            );
        }
    };
    let est = db
        .substrate_handle()
        .map(|st| st.node_count_live_estimate())
        .unwrap_or(0);
    let body = serde_json::json!({
        "store": store,
        "nodes_estimate": est,
        // L'état d'accès fait partie de ce que le serveur DIT de lui-même :
        // « qui peut lire ceci, et sous quelle condition » n'est pas un détail
        // d'exploitation, c'est une propriété du service.
        "iam": GardeIam::partagee().etat(),
        "iam_armee": GardeIam::partagee().est_armee(),
    })
    .to_string();
    respond(stream, 200, VIZ_JSON, &body)
}

/// `GET /api/viz/overview?store=` — synthèse du store ENTIER en lecture seule
/// (totaux + histogramme par type de nœud). Rapide (agrégats moteur, <0,5 s
/// sur 1,5M nœuds) : c'est la vue « monde » du dashboard.
fn api_overview(stream: &mut TcpStream, query: Option<&str>, config: &ServeConfig) -> Result<()> {
    use obrain_common::types::Value as OV;
    let params = parse_query(query);
    let Some(store) = config.resolve(params.get("store").map(String::as_str)) else {
        return respond(stream, 400, VIZ_JSON, r#"{"error":"store requis"}"#);
    };
    // CACHE DURABLE de l'overview (nœuds, arêtes, histogramme)
    let mt = store_mtime(&store);
    {
        use std::sync::{Mutex, OnceLock};
        static OVC: OnceLock<Mutex<std::collections::HashMap<std::path::PathBuf, (Option<u128>, String)>>> =
            OnceLock::new();
        let cache = OVC.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
        if let Some((m, b)) = verrou(&cache).get(&store).map(|(m, b)| (*m, b.clone())) {
            if m == mt { return respond(stream, 200, VIZ_JSON, &b); }
        }
        if let Some(b) = disk_load("ov", &store, "", mt) {
            verrou(&cache).insert(store.clone(), (mt, b.clone()));
            return respond(stream, 200, VIZ_JSON, &b);
        }
    }
    let db = match cached_ro(&store) {
        Ok(db) => db,
        Err(e) => {
            return respond(
                stream,
                400,
                VIZ_JSON,
                &format!("{{\"error\":\"ouverture RO : {e}\"}}"),
            );
        }
    };
    // ── (NANO 2026-09-11) OPERATIONS NOMMEES AU LIEU DE TROIS REQUETES GQL
    //
    // Les trois requetes d'origine sont conservees en commentaire parce que
    // la troisieme N'EST PAS remplacee a l'identique — voir SEMANTIQUE.
    //
    //   MATCH (n) RETURN count(n)                       -> db.node_count()
    //   MATCH ()-[r]->() RETURN count(r)                -> db.edge_count()
    //   MATCH (n) RETURN labels(n)[0] AS t, count(n)
    //     AS nbr ORDER BY nbr DESC LIMIT 14             -> db.label_counts()
    //
    // MESURE (criterion, en processus, snapshot runtime 347 970 nœuds /
    // 1 158 516 aretes, banc `chemin_nanoseconde`) :
    //   count(n) GQL          1,48 µs   -> node_count()        3,34 ns
    //   count(r) GQL          1,71 µs   -> edge_count()        3,43 ns
    //   histogramme GQL      77,1 ms    -> label_counts()      (voir banc)
    //
    // Les deux premiers comptes etaient DEJA O(1) sous l'enveloppe GQL :
    // l'enveloppe etait tout le cout. Le troisieme etait un VRAI balayage
    // des 347 970 nœuds — c'est lui qui portait les 77 ms.
    //
    // SEMANTIQUE, ce qui CHANGE et qu'il faut assumer : `labels(n)[0]`
    // attribue chaque nœud a sa PREMIERE etiquette ; `label_counts()`
    // compte un nœud dans CHACUNE de ses etiquettes. Sur un store
    // mono-etiquette les deux coincident exactement (test
    // `label_counts_egale_histogramme_gql_en_mono_etiquette`). Des qu'il
    // existe des nœuds multi-etiquettes, la somme des comptes depasse
    // `node_count()` (test `label_counts_diverge_de_labels_zero_en_multi_etiquette`).
    // Pour un HISTOGRAMME de dashboard, « combien de nœuds portent cette
    // etiquette » est la question que le lecteur se pose ; l'ancienne
    // reponse dependait de l'ordre de declaration des etiquettes, ce qui
    // n'etait pas une propriete voulue. Le champ `by_label_semantique`
    // rend le changement LISIBLE par le client au lieu de le cacher.
    //
    // PAS `db.schema()` : schema() assemble aussi les comptes par type
    // d'arete via `edge_counts_by_type()`, qui balaie les 1,16 M aretes
    // (12,8 ms mesures). L'overview n'a pas besoin de cette moitie.
    let nodes = Some(db.node_count() as u64);
    let edges = Some(db.edge_count() as u64);
    let histo: Vec<serde_json::Value> = db
        .label_counts()
        .into_iter()
        .take(14)
        .map(|(label, count)| serde_json::json!({ "label": label, "count": count }))
        .collect();
    let body = serde_json::json!({
        "store": store,
        "nodes": nodes,
        "edges": edges,
        "by_label": histo,
        "by_label_semantique": "noeuds_portant_l_etiquette",
    })
    .to_string();
        disk_save("ov", &store, "", mt, &body);
    respond(stream, 200, VIZ_JSON, &body)
}

/// `GET /api/viz/modules?store=&top=` — agrégats par MODULE (racine réelle du
/// dépôt, ex. drivers/net/…) pour superposer une couche « modules » dans la vue
/// Monde. Normalisation : le rang de la racine varie selon le nom de projet
/// (linux_6.6 → idx 3, linux_src → idx 2) → on évalue deux découpages et on
/// garde celui qui contient le plus de racines connues du kernel.
/// Sorties de build et caches : jamais du tissu de code.
///
/// Contrairement à `KNOWN_ROOTS` (qui listait les répertoires d'UN dépôt
/// précis, le noyau Linux), ce sont des noms CONVENTIONNELS de l'écosystème :
/// ils ne dépendent d'aucun projet.
///
/// Le filtre s'applique sur le **chemin complet**, pas sur le nom du module :
/// sinon `dist/static/vercel` et `dist/static/git` passeraient (leurs segments
/// ne sont pas des noms d'artefacts), et il faudrait énumérer chaque
/// sous-dossier vendorisé — impossible. En filtrant le chemin, tout le
/// SOUS-ARBRE d'une sortie de build disparaît, et un vrai dossier `static/`
/// situé ailleurs est préservé.
///
/// Mesuré : sans filtre, le tissu dominant du store obrain est `dist → web`
/// (50 439 appels) — le build qui appelle les sources, puis `vercel → git`
/// (2 450), du JS vendorisé.
/// Liste MINIMALE, et chaque entrée est mesurée : deux conditions `CONTAINS` par
/// nom, évaluées sur des millions d'arêtes.
///
/// Mesures de coût, à froid, requêtes distinctes (donc jamais servies par le cache
/// du moteur) :
///   runtime-dev (618 776 CALLS) : agrégat nu 2 834 ms → +filtre 5 947 ms
///   linux-full3 (1 997 541)     : agrégat nu 9 101 ms → +filtre 23 604 ms
/// Le filtre coûtait donc +3,1 s et +14,5 s — il DOUBLAIT le prix de l'agrégat.
/// Avec `dist` seul, le résultat est **strictement identique** sur ces bases (tous
/// leurs artefacts vivent sous `dist/`), pour un coût bien moindre.
///
/// On garde trois noms — les sorties de build les plus universelles des
/// écosystèmes JS/Cargo/npm — en assumant explicitement la contrepartie : une base
/// dont les seuls artefacts seraient `build/` ou `vendor/` les verrait encore. Le
/// jour où ça arrive, la réponse n'est pas d'allonger la liste (on repaierait le
/// prix mesuré ci-dessus) mais de marquer l'artefact à l'INDEXATION : c'est un
/// fait du chemin, pas une inférence à chaque requête.
const ARTEFACTS_BUILD: &[&str] = &["dist", "target", "node_modules"];

/// Noms d'artefacts filtrés AUSSI sur le NOM DE MODULE — et ce filtre-là est
/// GRATUIT (une comparaison en Rust, aucun coût moteur).
///
/// C'est le compromis mesuré : `build` et `vendor` coûtaient deux conditions
/// `CONTAINS` chacun dans la requête, et sans eux le tissu récupérait
/// `vendor → scripts` (65), `vendor → packages` (6), `packages → vendor` (4),
/// `src → build` (3), `vendor → apps` (2) — du bruit d'artefacts, exactement ce
/// que le filtre existe pour retirer. Les remettre dans la REQUÊTE coûtait
/// +1,8 s à froid ; les remettre ICI ne coûte rien.
///
/// Contrepartie assumée et bornée : un artefact n'est attrapé par son nom que
/// lorsqu'il EST le module. Un SOUS-dossier d'artefact (`build/foo/bar` → module
/// `bar`) passe encore. C'est pour cette raison que `dist`, `target` et
/// `node_modules` sont, eux, filtrés sur le CHEMIN complet — ce sont les seuls
/// dont les sous-dossiers portent des noms arbitraires et nombreux.
const ARTEFACTS_MODULE: &[&str] = &["dist", "target", "node_modules", "build", "vendor", "out"];

// ── `KNOWN_ROOTS` RESTE ICI (décision de fusion, 2026-09-16) ───────────────
//
// La branche `feat/orbites` la RETIRE, en déplaçant le choix de rang dans son
// `orbites::Deriveur`. Le tronc, lui, l'utilise encore — et il y a ajouté la
// MÉMOÏSATION de l'agrégat de rang (défaut mesuré : le même agrégat de rang 2
// était exécuté trois fois par appel, ~1,2 s de moteur pur pour trois fois le
// même résultat). Garder la liste est donc le prix de garder la mémoïsation.
//
// Les deux mécanismes cohabitent : `/api/viz/orbites` DÉRIVE les échelles
// (nouvelle route, `orbites.rs`), et le choix de rang de `modules`/`arcs` reste
// celui du tronc. Le jour où le dériveur remplace vraiment cette liste, il
// faudra déplacer la mémoïsation avec lui — pas seulement supprimer la constante.
const KNOWN_ROOTS: &[&str] = &[
    "drivers", "net", "kernel", "mm", "fs", "arch", "include", "init", "lib",
    "block", "ipc", "security", "crypto", "sound", "tools", "samples", "rust",
    "virt", "usr", "Documentation",
];
// ── `KNOWN_ROOTS` A ETE RETIRE ──────────────────────────────────────────────
//
// Cette liste — les repertoires du noyau Linux — servait a decider, ici, si le
// rang 2 etait le bon. La decision a demenage dans `orbites::Deriveur`, ou elle
// s'appuie sur la meme liste (`RACINES_CONNUES`) appliquee a la meme regle.
// La garder ici en aurait fait une SECONDE source de verite pour une seule
// question : deux listes qui divergent un jour, c'est `modules` et `arcs` qui
// cessent de parler du meme monde.


/// LA STRUCTURE D'ORBITES DU STORE, calculee UNE FOIS et gardee.
///
/// C'est le pivot de tout ce fichier : `modules`, `arcs` et la route
/// `/api/viz/orbites` lisent desormais LE MEME objet derive, au lieu de rejouer
/// chacun ses propres parcours pour redecouvrir le meme rang.
///
/// Deux etages de garde, comme les autres agregats : memoire (par store, valide
/// par le mtime) puis disque (`dir_cache()`, durable). La derivation elle-meme
/// ne coute qu'UNE projection — mesuree 3,3 a 3,7 s sur 1 553 071 noeuds, la ou
/// la SEULE agregation du rang 2 en coutait 5,0 a 8,1.
fn orbites_du_store(store: &std::path::Path) -> Option<crate::orbites::Orbites> {
    use std::sync::{Mutex, OnceLock};
    // INTERRUPTEUR DE COMPARAISON. `OBRAIN_ORBITES=0` rend le serveur a son
    // comportement d'avant : `modules` et `arcs` retombent chacun sur leur
    // agregation. C'est ce qui permet un A/B a BINAIRE IDENTIQUE — deux
    // binaires differents ne comparent pas deux chemins, ils comparent deux
    // compilations. C'est aussi la soupape si la derivation deplaisait en
    // production : une variable d'environnement, pas un redeploiement.
    if std::env::var("OBRAIN_ORBITES").as_deref() == Ok("0") {
        return None;
    }
    static ORB: OnceLock<
        Mutex<std::collections::HashMap<std::path::PathBuf, (Option<u128>, crate::orbites::Orbites)>>,
    > = OnceLock::new();
    let cache = ORB.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let mt = store_mtime(store);
    if let Some((m, o)) = cache.lock().unwrap().get(store).map(|(m, o)| (*m, o.clone())) {
        if m == mt {
            return Some(o);
        }
    }
    if let Some(b) = disk_load("orb", store, "", mt) {
        if let Ok(o) = serde_json::from_str::<crate::orbites::Orbites>(&b) {
            cache.lock().unwrap().insert(store.to_path_buf(), (mt, o.clone()));
            return Some(o);
        }
    }
    let db = cached_ro(store).ok()?;
    let sess = db.session();
    let d = crate::orbites::deriver(&sess).ok()?;
    let o = d.structure(400);
    if let Ok(b) = serde_json::to_string(&o) {
        disk_save("orb", store, "", mt, &b);
    }
    cache.lock().unwrap().insert(store.to_path_buf(), (mt, o.clone()));
    Some(o)
}

/// `GET /api/viz/orbites?store=` — LES ECHELLES QUE LE GRAPHE PERMET.
///
/// Une echelle n'est pas un seuil de distance de camera : c'est une propriete
/// de l'entite. Cette route dit, pour le store et pour chacune de ses branches,
/// la liste des orbites ouvertes — du plus large au plus fin — avec pour chacune
/// son rang de decoupage, son effectif, sa cardinalite et de quoi l'etiqueter.
///
/// Le rendu n'a donc plus a sonder ni a deviner : `rang_racine` se LIT, et la
/// profondeur est donnee PAR BRANCHE (`drivers` n'a pas celle de `fs` — une
/// profondeur globale unique serait un mensonge).
fn api_viz_orbites(stream: &mut TcpStream, query: Option<&str>, config: &ServeConfig) -> Result<()> {
    let params = parse_query(query);
    let Some(store) = config.resolve(params.get("store").map(String::as_str)) else {
        return respond(stream, 400, VIZ_JSON, r#"{"error":"store requis"}"#);
    };
    if !store.exists() {
        return respond(
            stream,
            400,
            VIZ_JSON,
            &format!("{{\"error\":\"store introuvable : {}\"}}", store.display()),
        );
    }
    let Some(o) = orbites_du_store(&store) else {
        return respond(stream, 500, VIZ_JSON, r#"{"error":"derivation des orbites impossible"}"#);
    };
    let mut v = serde_json::to_value(&o).unwrap_or_else(|_| serde_json::json!({}));
    v["store"] = serde_json::json!(store);
    respond(stream, 200, VIZ_JSON, &v.to_string())
}

fn api_viz_modules(stream: &mut TcpStream, query: Option<&str>, config: &ServeConfig) -> Result<()> {
    let params = parse_query(query);
    let Some(store) = config.resolve(params.get("store").map(String::as_str)) else {
        return respond(stream, 400, VIZ_JSON, r#"{"error":"store requis"}"#);
    };
    let top = params
        .get("top")
        .and_then(|t| t.parse::<usize>().ok())
        .unwrap_or(200)
        .min(400);
    // Cache DURABLE par store (agrégat = snapshot, immuable tant que le store
    // ne change pas — plus de TTL arbitraire) : ~0 ms aux revisites.
    let mt = store_mtime(&store);
    use std::sync::{Mutex, OnceLock};
    static MOD_CACHE: OnceLock<Mutex<std::collections::HashMap<std::path::PathBuf, (Option<u128>, String)>>> =
        OnceLock::new();
    let cache = MOD_CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Some((m, body)) = verrou(&cache).get(&store).map(|(m, b)| (*m, b.clone())) {
        if m == mt {
            return respond(stream, 200, VIZ_JSON, &body);
        }
    }
    if let Some(b) = disk_load("mod", &store, "", mt) {
        verrou(&cache).insert(store.clone(), (mt, b.clone()));
        return respond(stream, 200, VIZ_JSON, &b);
    }
    // ── LE RANG SE LIT, IL NE SE SONDE PLUS ────────────────────────────────
    //
    // Ici vivait une boucle qui rejouait la MEME agregation en faisant varier
    // le seul indice de `split`, jusqu'a cinq fois, pour DEVINER la profondeur
    // de hierarchie. Chaque essai est un parcours complet : `MATCH (n)` sans
    // etiquette (donc hydratation de chaque noeud), un `CONTAINS`, un `split`,
    // et un `GROUP BY` qui interdit le raccourci O(1) du planificateur.
    //
    // La structure d'orbites porte deja la reponse. Elle est derivee d'UNE
    // SEULE projection brute des `qualified_name` — mesuree 3,3 a 3,7 s sur
    // 1 553 071 noeuds, quand l'agregation d'UN SEUL rang en coutait 5,0 a 8,1.
    // Le rang et les groupes de TOUS les rangs en sortent par simple lecture.
    //
    // La regle de decision est reproduite trait pour trait dans
    // `orbites::Deriveur::rang_racine` : racines Linux reconnues au rang 2, et
    // sinon descente de 2 a 6 jusqu'au premier rang qui diverge (le plus gros
    // ensemble couvrant moins de 95 % du total). Meme rang, memes modules.
    let (rank_eff, groups) = match orbites_du_store(&store) {
        Some(o) => {
            let g = o
                .groupes_au_rang(o.rang_racine)
                .into_iter()
                .take(top)
                .collect::<Vec<_>>();
            (o.rang_racine, g)
        }
        // REPLI, pas second chemin : si la derivation echoue (store illisible,
        // projection refusee), on rend l'agregat du rang 2 plutot que rien.
        None => {
            use obrain_common::types::Value as OV;
            let db = match cached_ro(&store) {
                Ok(db) => db,
                Err(e) => {
                    return respond(
                        stream,
                        400,
                        VIZ_JSON,
                        &format!("{{\"error\":\"ouverture RO : {e}\"}}"),
                    );
                }
            };
            let sess = db.session();
            let q = format!(
                "MATCH (n) WHERE n.qualified_name CONTAINS '.' \
                 RETURN split(n.qualified_name, '.')[2] AS m, count(n) AS c \
                 ORDER BY c DESC LIMIT {top}"
            );
            let g: Vec<(String, u64)> = sess
                .execute(&q)
                .map(|r| {
                    r.rows
                        .iter()
                        .filter_map(|row| {
                            let (Some(OV::String(m)), Some(OV::Int64(c))) =
                                (row.first(), row.get(1))
                            else {
                                return None;
                            };
                            Some((m.to_string(), *c as u64))
                        })
                        .collect()
                })
                .unwrap_or_default();
            (2usize, g)
        }
    };
    let modules: Vec<serde_json::Value> = groups
        .into_iter()
        .filter(|(m, _)| !m.is_empty())
        .map(|(m, c)| serde_json::json!({ "name": m, "count": c }))
        .collect();
    // `rank` = index de découpage effectif (g2/g3) : il permet au front de
    // descendre DANS un module (sous-espace au rang rank+1) — dimension
    // « projets → sous-espaces » du Monde.
    let body = serde_json::json!({ "store": store, "rank": rank_eff, "modules": modules }).to_string();
    disk_save("mod", &store, "", mt, &body);
    verrou(&cache).insert(store.clone(), (mt, body.clone()));
    respond(stream, 200, VIZ_JSON, &body)
}

/// `GET /api/viz/ramifications?store=&center=<qn>[,<qn>…]` — TOUTES les
/// ramifications d une entite (ou d un GROUPE de centres, comptes agreges) :
///
///   · REELLES — entrantes et sortantes, groupees par type d arete, avec
///     leurs comptes : ce qui est deja construit autour du sujet ;
///   · A CONSTRUIRE — les types d aretes que les labels du groupe portent
///     AILLEURS dans le store mais qui manquent ici, avec le compte du store
///     comme indice : le possible pas encore construit, represente au meme
///     titre que le reel.
///
/// Les quatre formes sont des comptes groupes par type : le raccourci du
/// planificateur les sert sans balayage — la reponse reste instantanee.
fn api_viz_ramifications(
    stream: &mut TcpStream,
    query: Option<&str>,
    config: &ServeConfig,
) -> Result<()> {
    let params = parse_query(query);
    let Some(store) = config.resolve(params.get("store").map(String::as_str)) else {
        return respond(stream, 400, VIZ_JSON, r#"{"error":"store requis"}"#);
    };
    let Some(centres_brut) = params.get("center").or_else(|| params.get("centers")) else {
        return respond(stream, 400, VIZ_JSON, r#"{"error":"center (ou centers) requis"}"#);
    };
    let centres: Vec<String> = centres_brut
        .split(',')
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .take(8)
        .collect();
    if centres.is_empty() {
        return respond(stream, 400, VIZ_JSON, r#"{"error":"center vide"}"#);
    }
    // Memo de REPONSE par (store, centres, mtime) : les ramifications d un
    // centre ne changent qu avec le store — la re-ouverture du panneau est
    // gratuite.
    static MEMO: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<(String, String), (u64, String)>>,
    > = std::sync::OnceLock::new();
    let memo = MEMO.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mt = store_mtime(std::path::Path::new(&store)).map(|m| m as u64).unwrap_or(0);
    let cle_memo = (store.to_string_lossy().to_string(), centres.join(","));
    if let Some((m, corps)) = memo.lock().unwrap().get(&cle_memo).cloned() {
        if m == mt {
            return respond(stream, 200, VIZ_JSON, &corps);
        }
    }
    let db = match cached_ro(std::path::Path::new(&store)) {
        Ok(d) => d,
        Err(e) => {
            let body = serde_json::json!({ "error": format!("ouverture: {e}") }).to_string();
            return respond(stream, 500, VIZ_JSON, &body);
        }
    };
    let session = db.session();
    let lit = |s: &str| s.replace('\\', "\\\\").replace('\'', "\\'");
    // Resolution qn -> id MEMOISEE par store : le lookup par egalite coute
    // encore ~250 ms sur cette forme (dette NS — le planner ne route pas ce
    // WHERE par l eq-index) ; on le paie UNE fois par centre et par store.
    static IDS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<(String, String), i64>>,
    > = std::sync::OnceLock::new();
    let ids = IDS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let resoudre = |c: &str| -> Option<i64> {
        if let Ok(n) = c.parse::<i64>() {
            return Some(n);
        }
        let cle = (cle_memo.0.clone(), c.to_string());
        if let Some(id) = ids.lock().unwrap().get(&cle).copied() {
            return Some(id);
        }
        let cq = lit(c);
        let id = session
            .execute(&format!(
                "MATCH (n) WHERE n.qualified_name = '{cq}' RETURN id(n) LIMIT 1"
            ))
            .ok()
            .and_then(|r| r.rows.first().and_then(|row| row.first().and_then(|v| v.as_int64())))?;
        ids.lock().unwrap().insert(cle, id);
        Some(id)
    };
    // Les labels arrivent en representation texte `List([String("A"), …])` :
    // on extrait les contenus de `String("…")` sans en dependre davantage.
    fn labels_du_texte(t: &str) -> Vec<String> {
        // Forme servie par le moteur : `[Session]` ou `[A, B]`.
        t.trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split(',')
            .map(|x| x.trim().trim_matches('"').to_string())
            .filter(|x| !x.is_empty() && x != "NULL")
            .collect()
    }
    use std::collections::{BTreeMap, BTreeSet};
    let mut sortantes: BTreeMap<String, u64> = BTreeMap::new();
    let mut entrantes: BTreeMap<String, u64> = BTreeMap::new();
    let mut labels: BTreeSet<String> = BTreeSet::new();
    let mut grouper = |carte: &mut BTreeMap<String, u64>, q: &str| {
        if let Ok(r) = session.execute(q) {
            for row in &r.rows {
                let t = row.first().map(|v| v.to_string()).unwrap_or_default();
                let n = row.get(1).and_then(|v| v.as_int64()).unwrap_or(0);
                if !t.is_empty() && t != "NULL" && n > 0 {
                    *carte.entry(t).or_insert(0) += n as u64;
                }
            }
        }
    };
    for c in &centres {
        let Some(id) = resoudre(c) else { continue };
        grouper(&mut sortantes, &format!(
            "MATCH (n)-[r]->() WHERE id(n) = {id} RETURN type(r), count(*)"
        ));
        grouper(&mut entrantes, &format!(
            "MATCH ()-[r]->(n) WHERE id(n) = {id} RETURN type(r), count(*)"
        ));
        if let Ok(r) = session.execute(&format!(
            "MATCH (n) WHERE id(n) = {id} RETURN labels(n) LIMIT 1"
        )) {
            for row in &r.rows {
                if let Some(v) = row.first() {
                    for l in labels_du_texte(&v.to_string()) {
                        labels.insert(l);
                    }
                }
            }
        }
    }
    // Le POSSIBLE : ce que ces labels portent ailleurs dans le store.
    let mut possibles_sortants: BTreeMap<String, u64> = BTreeMap::new();
    let mut possibles_entrants: BTreeMap<String, u64> = BTreeMap::new();
    for l in &labels {
        if !l.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue; // un label exotique ne passe pas en syntaxe :Label
        }
        grouper(&mut possibles_sortants, &format!(
            "MATCH (a:{l})-[r]->() RETURN type(r), count(*)"
        ));
        grouper(&mut possibles_entrants, &format!(
            "MATCH ()-[r]->(a:{l}) RETURN type(r), count(*)"
        ));
    }
    let en_liste = |carte: &BTreeMap<String, u64>| -> Vec<serde_json::Value> {
        let mut v: Vec<_> = carte.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        v.into_iter()
            .map(|(t, n)| serde_json::json!({ "type": t, "nb": n }))
            .collect()
    };
    let a_construire = |possibles: &BTreeMap<String, u64>, reelles: &BTreeMap<String, u64>| {
        let mut v: Vec<_> = possibles.iter().filter(|(t, _)| !reelles.contains_key(*t)).collect();
        v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        v.into_iter()
            .map(|(t, n)| serde_json::json!({ "type": t, "indice_store": n }))
            .collect::<Vec<_>>()
    };
    let body = serde_json::json!({
        "centres": centres,
        "labels": labels.iter().collect::<Vec<_>>(),
        "sortantes": en_liste(&sortantes),
        "entrantes": en_liste(&entrantes),
        "a_construire": {
            "sortantes": a_construire(&possibles_sortants, &sortantes),
            "entrantes": a_construire(&possibles_entrants, &entrantes),
        },
    })
    .to_string();
    memo.lock().unwrap().insert(cle_memo, (mt, body.clone()));
    respond(stream, 200, VIZ_JSON, &body)
}

/// `GET /api/viz/arcs?store=&top=12&limit=250` — arcs CALLS inter-modules
/// (racines réelles du dépôt, même normalisation que `/api/viz/modules`).
/// Réponse : `{ store, arcs: [{src, dst, weight}] }` où `weight` = nombre
/// d'arêtes CALLS entre deux modules, paires `src≠dst` triées par poids
/// décroissant (≤ limit). Agrégat moteur en UNE passe groupée sur le graphe
/// CALLS (split du qualified_name au rang racine choisi g2/g3).
fn api_viz_arcs(stream: &mut TcpStream, query: Option<&str>, config: &ServeConfig) -> Result<()> {
    let params = parse_query(query);
    let Some(store) = config.resolve(params.get("store").map(String::as_str)) else {
        return respond(stream, 400, VIZ_JSON, r#"{"error":"store requis"}"#);
    };
    let top = params
        .get("top")
        .and_then(|t| t.parse::<usize>().ok())
        .unwrap_or(12)
        .clamp(1, 100_000);
    let limit = params
        .get("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(250)
        .clamp(1, 500_000);
    // Le GRAIN est un choix, pas un invariant : selon le rang, le tissu décrit
    // le monorepo (`packages → apps`) ou l'architecture (`obrain-engine →
    // obrain-common`). Mesuré : le tissu hors-artefacts croît toujours avec la
    // profondeur, donc « le plus de croisements » choisirait le rang le plus fin.
    // On garde un défaut adaptatif, et on laisse l'appelant imposer son grain.
    let rank_impose = params.get("rank").and_then(|r| r.parse::<usize>().ok());
    // ── LA CLÉ DE CACHE NE PORTE PLUS `limit` NI `top` (2026-09-13) ────────
    //
    // Défaut mesuré : la clé était `(store, top, limit*10 + rang)`. Or `limit`
    // n'entre dans la REQUÊTE que par `engine_limit = limit + 1024`, et le
    // résultat est ensuite TRONQUÉ en Rust. Conséquence : demander
    // `(top=16, limit=60)` après `(top=16, limit=250)` repaie l'agrégat COMPLET
    // — mesuré 1 921 à 1 999 ms par couple neuf sur `runtime-dev`. Le dashboard
    // fait exactement ça : `CosmosWeave.tsx:1917-1918` demande `(top=16,
    // limit=60)` pour chaque monde, et tout changement de `top` (le front en
    // fait varier) rouvrait un froid par store.
    //
    // On quantifie donc `limit` en PALIERS : l'agrégat est calculé au palier
    // atteint, mis en cache sous cette clé, et toute demande dont `limit` tient
    // dans le palier est servie par TRONCATURE du même corps. La validité tient
    // à une propriété de la requête : `ORDER BY w DESC` — le top-N par poids est
    // un PRÉFIXE du top-M pour M ≥ N.
    //
    // `top`, lui, ne figure dans AUCUNE requête d'agrégat : il ne paramètre que
    // le choix heuristique du rang (la requête `group_at`), et ce choix est
    // (a) ignoré dès qu'un rang est MÉMORISÉ (`rang_memo`, écrit juste après),
    // (b) ignoré dès que `rank=` est imposé, (c) écrasé par la sonde d'existence
    // des rangs. Un corps calculé sous un `top` donné est donc valable pour tout
    // autre `top` — et `rang_ecrire` rend le rang définitif dès le premier
    // calcul. C'est ce qui autorise à le retirer de la clé.
    let palier = palier_arcs(limit);
    if !store.exists() {
        return respond(
            stream,
            400,
            VIZ_JSON,
            &format!("{{\"error\":\"store introuvable : {}\"}}", store.display()),
        );
    }
    // ── LE CALCUL EST DELEGUE, PARCE QU'IL N'EST PAS PROPRE A UNE REQUETE ──
    // Voir `arcs_corps` : la meme fonction sert la route ET le prechauffage de
    // demarrage. C'est ce qui permet de ne plus faire payer le froid par un
    // visiteur (voir `prechauffer_les_arcs`).
    let body = match arcs_corps(&store, top, palier, rank_impose) {
        Ok(b) => b,
        Err(e) => {
            return respond(
                stream,
                400,
                VIZ_JSON,
                &serde_json::json!({ "error": e }).to_string(),
            );
        }
    };
    respond(stream, 200, VIZ_JSON, &tronquer_arcs(&body, limit, palier))
}

/// Calcule (ou relit du cache) le corps d'`arcs` pour `(store, palier, rang)`.
///
/// ── POURQUOI CE N'EST PAS DANS LA ROUTE (2026-09-15) ──────────────────────
///
/// Ce corps etait calcule DANS le gestionnaire de requete : il etait donc
/// impossible de le preparer ailleurs, et le PREMIER appel apres un demarrage a
/// froid payait tout, en pleine visite. Mesure sur le service 8432, cache vide,
/// `runtime-dev` (439 688 noeuds / 1 324 105 aretes) :
///
///   ouverture du store ............................ 1 320 ms
///   choix du rang (`group_at`) .....................  ~400 ms
///   sondes d'existence des rangs (jusqu'a 6) .......  ~800 ms
///   sonde d'artefacts de build .....................  ~200 ms
///   AGREGEAT FINAL (ce qu'on veut) .................  ~700 ms
///   ------------------------------------------------ TOTAL ~3 400 ms
///
/// et, cache DISQUE vide en plus (il vit dans `/tmp/obrain-viz-cache`, efface au
/// reboot), **6 240 ms** mesures sur une instance isolee : c'est exactement le
/// « plus de 3 s » rapporte par l'utilisateur.
///
/// Les quatre cinquiemes de ce froid ne sont PAS l'agregat : ce sont des SONDES
/// qui ne font que redecrire une structure IMMUABLE. Les payer une fois par
/// demarrage, en tache de fond, ne coute rien a personne ; les payer dans une
/// requete, c'est 3,4 s devant un ecran vide. D'ou cette extraction : `arcs_corps`
/// est appelable HORS requete (voir `prechauffer_les_arcs`).
///
/// Rend le corps AU PALIER (jamais tronque a `limit`) : c'est la route qui
/// tronque, par `tronquer_arcs`. `Err` porte le message a rendre en 400.
fn arcs_corps(
    store: &std::path::Path,
    top: usize,
    palier: usize,
    rank_impose: Option<usize>,
) -> Result<String, String> {
    use obrain_common::types::Value as OV;
    let rang_cle = rank_impose.unwrap_or(0);
    // Cache DURABLE par (store, palier, rang) : snapshot immuable → plus de TTL.
    let mt = store_mtime(&store);
    use std::sync::{Mutex, OnceLock};
    static ARCS_CACHE: OnceLock<
        Mutex<
            std::collections::HashMap<
                (std::path::PathBuf, usize, usize),
                (Option<u128>, String),
            >,
        >,
    > = OnceLock::new();
    let cache = ARCS_CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let key = (store.to_path_buf(), palier, rang_cle);
    if let Some((m, body)) = verrou(&cache).get(&key).map(|(m, b)| (*m, b.clone())) {
        if m == mt {
            return Ok(body);
        }
    }
    let aextra = format!("{palier}x{rang_cle}");
    if let Some(b) = disk_load("arcs", &store, &aextra, mt) {
        verrou(&cache).insert(key.clone(), (mt, b.clone()));
        return Ok(b);
    }
    let db = match cached_ro(store) {
        Ok(db) => db,
        Err(e) => return Err(format!("ouverture RO : {e}")),
    };
    let sess = db.session();
    // Rang de la racine : même décision que /api/viz/modules, évaluée sur les
    // `top` modules dominants — g2 (linux_src) préféré si racines connues,
    // sinon g3 (linux_6.6).
    //
    // ── L'AGRÉGAT DE RANG SE PAIE UNE FOIS, PAS TROIS (2026-09-13) ────────
    // Défaut mesuré : le MÊME agrégat de rang 2 était exécuté TROIS fois par
    // appel — `known(&group_at(2))`, puis `let mut g = group_at(2)`, puis la
    // PREMIÈRE itération de la boucle (`essai = 2`). Coût mesuré de chaque
    // exécution sur ce store (414 352 nœuds) : 407-478 ms au rang 2, 366-400 ms
    // au rang 3 — soit ~1,2 s de moteur pur pour trois fois le même résultat.
    // On mémoïse donc par rang, LOCALEMENT à l'appel : même requête, même `top`,
    // même session — seul le doublon disparaît.
    let mut memo_rangs: std::collections::HashMap<usize, Vec<(String, u64)>> =
        std::collections::HashMap::new();
    let mut group_at = |idx: usize| -> Vec<(String, u64)> {
        if let Some(g) = memo_rangs.get(&idx) {
            return g.clone();
        }
        let q = format!(
            "MATCH (n) WHERE n.qualified_name CONTAINS '.'              RETURN split(n.qualified_name, '.')[{idx}] AS m, count(n) AS c              ORDER BY c DESC LIMIT {top}"
        );
        let g: Vec<(String, u64)> = sess
            .execute(&q)
            .map(|r| {
                r.rows
                    .iter()
                    .filter_map(|row| {
                        let (Some(OV::String(m)), Some(OV::Int64(c))) = (row.first(), row.get(1)) else {
                            return None;
                        };
                        Some((m.to_string(), *c as u64))
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Rendre le doublon VÉRIFIABLE : sans cette ligne, « deux fois au lieu
        // de trois » resterait une affirmation qu'on ne peut pas contrôler.
        tracing::debug!(
            rang = idx,
            top,
            lignes = g.len(),
            "arcs : agrégat de rang exécuté (mémoïsé pour cet appel)"
        );
        memo_rangs.insert(idx, g.clone());
        g
    };
    let known = |g: &[(String, u64)]| {
        g.iter().filter(|(m, _)| KNOWN_ROOTS.contains(&m.as_str())).count()
    };
    // MÊME RÈGLE QUE `api_viz_modules` — c'est la doc de cette route qui l'exige
    // (« même normalisation que /api/viz/modules »), et sans ça les modules
    // affichés et les arcs ne parleraient plus de la même chose. Voir le
    // commentaire détaillé là-bas : le rang se choisit sur la donnée, pas sur
    // une liste de racines Linux.
    //
    // ── ET ON NE LE CHOISIT PAS QUAND IL EST DÉJÀ PROUVÉ ───────────────────
    // Ce bloc ne sert qu'à CHOISIR un rang ; sa valeur (`idx`) est ensuite
    // ÉCRASÉE par le mémo (`idx_final = r`, plus bas). Le jouer quand même,
    // c'était payer 1 à 5 agrégats de rang — mesuré 407-478 ms chacun — pour un
    // résultat jeté. On lit donc le mémo AVANT, et on saute le choix.
    let memorise = if rank_impose.is_some() { None } else { rang_memo(&store) };
    let mut idx = 2usize;
    if let Some(r) = memorise {
        idx = r.clamp(2, 7);
        tracing::debug!(rang = idx, "arcs : rang mémorisé — choix du rang sauté");
    } else if known(&group_at(2)) == 0 {
        let mut g = group_at(2);
        for essai in 2..=6usize {
            g = group_at(essai);
            if g.is_empty() {
                continue;
            }
            let total: u64 = g.iter().map(|(_, c)| *c).sum();
            let premier = g.first().map(|(_, c)| *c).unwrap_or(0);
            let diverge =
                g.len() >= 2 && (total == 0 || premier.saturating_mul(100) / total.max(1) < 95);
            idx = essai;
            if diverge {
                break;
            }
        }
    }
    // Agrégat groupé en une passe sur le graphe CALLS. Deux contraintes moteur
    // (vérifiées à l'échelle 1,55M nœuds / 2M arêtes) :
    // - le groupement RETOUR direct sur des expressions split() produit des
    //   clés dupliquées → on projette d'abord les modules via WITH en
    //   forwardant r (seul moyen de garder count(r) au RETURN) ;
    // - le filtre `<>` entre expressions split() dans le WHERE n'est pas
    //   fiable à cette échelle → pas d'inégalité moteur : on demande une marge
    //   au-delà de `limit` (une ligne intra-module par module au plus) et on
    //   filtre src≠dst côté serveur, filtre qui fait foi.
    //
    // ── IL EST CALCULÉ AU PALIER, PAS À `limit` (2026-09-13) ──────────────
    // `limit` n'entre dans la requête QUE par cette marge : la requête ne
    // contient nulle part `limit` lui-même (vérifié — c'est ce qui autorise à
    // retirer `limit` de la clé de cache). On demande donc le palier, on met en
    // cache le résultat À CE PALIER, et `tronquer_arcs` en sert le préfixe voulu.
    let engine_limit = palier.saturating_add(1024).min(1_000_000);
    // ── CHOIX DU RANG EN UNE SEULE PASSE (l'échelle l'exige) ───────────────
    //
    // Les bases sont GIGANTESQUES — mesuré : 372 750 nœuds / 1 189 802 arêtes
    // (runtime-dev), 1 553 071 / 4 779 754 (linux-full3). Une passe d'agrégat
    // coûte 4 à 15 s à cette échelle. La première version de ce correctif
    // ITÉRAIT sur les rangs pour trouver celui qui croise : 15,8 s à froid sur
    // linux, 7,9 s sur runtime-dev. C'était un défaut que j'ai introduit.
    //
    // On échantillonne donc les ARÊTES — `LIMIT` fait s'arrêter le moteur tôt,
    // donc un coût CONSTANT quelle que soit la taille de la base — on compte les
    // croisements par profondeur en Rust, et on ne lance QU'UNE agrégation, au
    // rang qui croise le plus. À égalité on prend le rang le plus court, ce qui
    // préserve le comportement Linux (croisement dès le rang 2).
    // ── RANG : COMPTAGE PAR PROFONDEUR, PAS D'ÉCHANTILLON ──────────────────
    //
    // Un échantillon d'arêtes (`LIMIT`) suit l'ORDRE DE STOCKAGE et n'est PAS
    // représentatif : mesuré, 20 000 arêtes annonçaient moins de 5 % de
    // croisement au rang 4 alors que le taux RÉEL y est de 15,03 % (93 029 sur
    // 618 776). Il faisait donc descendre au rang 5, plus fin et moins lisible.
    //
    // On compte donc réellement, par profondeur, avec une requête de COMPTAGE
    // SEULE (ni tri ni LIMIT ni matérialisation de paires : bien moins chère
    // qu'un agrégat complet), et on retient le rang LE PLUS COURT qui croise.
    // Mesuré sur ce store : rang 3 = 0, rang 4 = 93 029 → on s'arrête à 4.
    // Sur l'arbre Linux, le rang 2 croise déjà : une seule requête.
    let mut idx_final = rank_impose.unwrap_or(idx).clamp(2, 7); // repli : rang des modules
    // RANG MÉMORISÉ : on ne repaie pas les sondes qui prouvent l'absence.
    // Le gain est mesuré (voir `rang_memo`) ; il est vérifié plus bas par
    // l'agrégat lui-même — s'il ne rend rien, on refait la recherche complète.
    // (`memorise` est lu PLUS HAUT, pour pouvoir sauter le choix du rang.)
    if let Some(r) = memorise {
        idx_final = r.clamp(2, 7);
    }
    for d in 2..=7usize {
        if rank_impose.is_some() {
            break; // grain imposé : aucun comptage nécessaire
        }
        if memorise.is_some() {
            break; // rang mémorisé : pas de sonde, on valide par l'agrégat
        }
        // SONDE D'EXISTENCE, pas de comptage : on n'a pas besoin du NOMBRE de
        // croisements, seulement de savoir s'il en existe un. Mesuré :
        //   runtime-dev rang 4 : count() 2 105 ms  →  existence 69 ms   (×30)
        //   linux-full3 rang 2 : count() 6 834 ms  →  existence 27 ms   (×253)
        // Le moteur s'arrête à la première arête qui croise. Contrepartie
        // assumée : pour un rang SANS croisement, il faut tout parcourir
        // (~2 s) — c'est le prix de la preuve d'absence, incompressible.
        let qc = format!(
            "MATCH (a)-[r:CALLS]->(b) \
             WITH split(a.qualified_name, '.')[{d}] AS sa, split(b.qualified_name, '.')[{d}] AS sb, r \
             WHERE sa <> sb RETURN 1 AS existe LIMIT 1"
        );
        let existe = sess
            .execute(&qc)
            .map(|res| !res.rows.is_empty())
            .unwrap_or(false);
        if existe {
            idx_final = d;
            break;
        }
    }
    // Filtre de SOUS-ARBRE : une arête touchant une sortie de build n'est pas
    // du tissu de code. Appliqué dans la requête (et pas après) pour que le
    // top-N soit calculé sur le tissu RÉEL, pas sur un top-N d'artefacts.
    // ── LE FILTRE NE SE PAIE QUE S'IL SERT ─────────────────────────────────
    //
    // Les conditions `CONTAINS` sont évaluées sur CHAQUE arête. Sur un arbre qui
    // n'a aucune sortie de build (le noyau Linux), elles ne retirent rien et
    // coûtent quand même : mesuré +14 503 ms sur 1 997 541 arêtes — pour zéro.
    //
    // Une SONDE UNIQUE (une seule requête pour les trois noms, qui s'arrête à la
    // première correspondance) décide donc s'il y a lieu de filtrer. Elle coûte
    // un parcours de nœuds quand la réponse est « non » (~1-2 s) et presque rien
    // quand elle est « oui » — dans les deux cas bien moins que le filtre inutile.
    let a_des_artefacts = {
        let ou: String = ARTEFACTS_BUILD
            .iter()
            .map(|a| format!("n.qualified_name CONTAINS '.{a}.'"))
            .collect::<Vec<_>>()
            .join(" OR ");
        let qa = format!("MATCH (n) WHERE {ou} RETURN 1 AS oui LIMIT 1");
        sess.execute(&qa)
            .map(|res| !res.rows.is_empty())
            .unwrap_or(true) // en cas de doute on filtre : on ne veut pas de bruit
    };
    let filtre_art: String = if a_des_artefacts {
        ARTEFACTS_BUILD
            .iter()
            .map(|a| {
                format!(
                    " AND NOT a.qualified_name CONTAINS '.{a}.' AND NOT b.qualified_name CONTAINS '.{a}.'"
                )
            })
            .collect()
    } else {
        String::new()
    };
    let q = format!(
        "MATCH (a)-[r:CALLS]->(b) WHERE a.qualified_name CONTAINS '.' AND b.qualified_name CONTAINS '.'{filtre_art} \
         WITH split(a.qualified_name, '.')[{idx_final}] AS sa, split(b.qualified_name, '.')[{idx_final}] AS sb, r \
         RETURN sa, sb, count(r) AS w ORDER BY w DESC LIMIT {engine_limit}"
    );
    let mut arcs: Vec<(String, String, u64)> = match sess.execute(&q) {
        Ok(res) => res
            .rows
            .iter()
            .filter_map(|row| {
                let (Some(OV::String(sa)), Some(OV::String(sb)), Some(OV::Int64(w))) =
                    (row.first(), row.get(1), row.get(2))
                else {
                    return None;
                };
                let (sa, sb) = (sa.to_string(), sb.to_string());
                // `src ≠ dst`, et aucun des deux n'est un artefact par son NOM
                // (gratuit ici) : le filtre de CHEMIN, lui, est dans la requête et
                // couvre les sous-dossiers de `dist`/`target`/`node_modules`.
                (sa != sb
                    && !sa.is_empty()
                    && !sb.is_empty()
                    && !ARTEFACTS_MODULE.contains(&sa.as_str())
                    && !ARTEFACTS_MODULE.contains(&sb.as_str()))
                .then(|| (sa, sb, *w as u64))
            })
            .collect(),
        Err(e) => return Err(format!("arcs inter-modules : {e}")),
    };
    // LE RANG MÉMORISÉ DOIT SE JUSTIFIER : si l'agrégat ne rend aucun arc, la
    // structure a changé (ou la mémoire est fausse) → on refait la recherche
    // complète, une seule fois. Une mémoire qui ne se vérifie pas est un piège.
    if arcs.is_empty() && memorise.is_some() {
        let mut secours = idx.clamp(2, 7);
        for d in 2..=7usize {
            let qc = format!(
                "MATCH (a)-[r:CALLS]->(b) \
                 WITH split(a.qualified_name, '.')[{d}] AS sa, split(b.qualified_name, '.')[{d}] AS sb, r \
                 WHERE sa <> sb RETURN 1 AS existe LIMIT 1"
            );
            if sess.execute(&qc).map(|res| !res.rows.is_empty()).unwrap_or(false) {
                secours = d;
                break;
            }
        }
        idx_final = secours;
        let filtre_art2: String = ARTEFACTS_BUILD
            .iter()
            .map(|a| format!(" AND NOT a.qualified_name CONTAINS '.{a}.' AND NOT b.qualified_name CONTAINS '.{a}.'"))
            .collect();
        let q2 = format!(
            "MATCH (a)-[r:CALLS]->(b) WHERE a.qualified_name CONTAINS '.' AND b.qualified_name CONTAINS '.'{filtre_art2} \
             WITH split(a.qualified_name, '.')[{idx_final}] AS sa, split(b.qualified_name, '.')[{idx_final}] AS sb, r \
             RETURN sa, sb, count(r) AS w ORDER BY w DESC LIMIT {engine_limit}"
        );
        if let Ok(res) = sess.execute(&q2) {
            arcs = res
                .rows
                .iter()
                .filter_map(|row| {
                    let (Some(OV::String(sa)), Some(OV::String(sb)), Some(OV::Int64(w))) =
                        (row.first(), row.get(1), row.get(2))
                    else {
                        return None;
                    };
                    let (sa, sb) = (sa.to_string(), sb.to_string());
                    (sa != sb
                        && !sa.is_empty()
                        && !sb.is_empty()
                        && !ARTEFACTS_MODULE.contains(&sa.as_str())
                        && !ARTEFACTS_MODULE.contains(&sb.as_str()))
                    .then(|| (sa, sb, *w as u64))
                })
                .collect();
        }
    }
    rang_ecrire(&store, idx_final); // le prochain froid repartira de là
    arcs.sort_by(|x, y| y.2.cmp(&x.2)); // poids décroissant (le moteur trie déjà ; garanti ici)
    // ON TRONQUE AU PALIER, PAS À `limit` : le corps mis en cache doit pouvoir
    // servir TOUTE demande dont `limit` tient dans ce palier. La troncature
    // demandée est appliquée à la sortie (`tronquer_arcs`), jamais ici.
    arcs.truncate(palier);
    let list: Vec<serde_json::Value> = arcs
        .into_iter()
        .map(|(src, dst, weight)| serde_json::json!({ "src": src, "dst": dst, "weight": weight }))
        .collect();
    let body = serde_json::json!({ "store": store, "arcs": list }).to_string();
    disk_save("arcs", store, &aextra, mt, &body);
    verrou(&cache).insert(key, (mt, body.clone()));
    Ok(body)
}

/// Paliers de `limit` pour `/api/viz/arcs`.
///
/// Mesuré sur `runtime-dev` : l'agrégat `LIMIT 1324` rend **143 lignes** — le
/// nombre de paires de modules est PETIT devant le plafond, donc le coût est
/// dominé par le BALAYAGE, pas par la taille du résultat. Quantifier `limit`
/// (plutôt que de le mettre dans la clé) ne coûte donc presque rien et fait
/// tomber le nombre d'entrées de cache à un par store et par rang.
fn palier_arcs(limit: usize) -> usize {
    const PALIERS: [usize; 4] = [1024, 4096, 65_536, 500_000];
    *PALIERS
        .iter()
        .find(|p| limit <= **p)
        .unwrap_or(&500_000)
}

/// Sert un corps d'`arcs` calculé à `palier` sous la forme demandée (`limit`).
///
/// VALIDITÉ : la requête trie par poids décroissant (`ORDER BY w DESC`), donc le
/// top-`limit` est un PRÉFIXE du top-`palier` pour `limit ≤ palier`. La seule
/// copie faite ici est celle du JSON de réponse (quelques kilo-octets) ; quand
/// `limit` atteint le palier, on ne recopie même pas.
fn tronquer_arcs(body: &str, limit: usize, palier: usize) -> String {
    if limit >= palier {
        return body.to_string();
    }
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(body) else {
        // Corps illisible : on rend l'entier plutôt que de mentir par une
        // réponse tronquée au hasard.
        return body.to_string();
    };
    if let Some(a) = v.get_mut("arcs").and_then(|x| x.as_array_mut()) {
        a.truncate(limit);
    }
    v.to_string()
}

/// `GET /api/viz/space?store=&prefix=&rank=&top=&links=1` — SOUS-ESPACE d'un
/// ensemble : descendre dans un module/projet (dimension « projets » du Monde).
/// * `prefix` : chemin de tokens du parent (« drivers » ou « drivers.net »…,
///   vide = racine du store) ;
/// * `rank`   : rang du token enfant à agréger (`split(qn,'.')[rank]`) — le
///   front descend depuis `rank` exposé par `/api/viz/modules` ;
/// * `top`    : nb max d'ensembles enfants (≤ 200) ;
/// * `links`  : renvoie en plus les arcs CALLS entre enfants du sous-espace.
///
/// Restriction par `CONTAINS '.prefix.'` (les qn des dépôts sont des chemins
/// sans extension, tokens joints par '.') ; agrégats mis en cache 120 s —
/// mêmes ordres de grandeur que modules/arcs sur les gros stores.
fn api_viz_space(stream: &mut TcpStream, query: Option<&str>, config: &ServeConfig) -> Result<()> {
    use obrain_common::types::Value as OV;
    let params = parse_query(query);
    let Some(store) = config.resolve(params.get("store").map(String::as_str)) else {
        return respond(stream, 400, VIZ_JSON, r#"{"error":"store requis"}"#);
    };
    let prefix = params.get("prefix").cloned().unwrap_or_default();
    // Tokens de chemins réels (dossiers npm…) : @, +, %, -, _ autorisés ;
    // toute entrée hostile au littéral GQL (guillemets, backslash, espaces,
    // contrôle…) reste refusée — anti-injection + bornage de longueur.
    let prefix_ok = prefix.len() <= 400
        && prefix.chars().all(|c| matches!(
            c,
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' | '@' | '+' | '%'
        ));
    if !prefix_ok {
        return respond(stream, 400, VIZ_JSON, r#"{"error":"prefix invalide"}"#);
    }
    let rank = params.get("rank").and_then(|r| r.parse::<usize>().ok()).unwrap_or(2).min(32);
    let links = params.get("links").map(|v| v != "0").unwrap_or(true);
    // ── LA PAGE SUIVANTE DU MÊME NIVEAU (densifier, pas approfondir) ────────
    // DÉFAUT MESURÉ (2026-09-15) : le front coupait la liste reçue à 24 groupes
    // sur les 200 que cette route sert, et AUCUN geste ne permettait de voir les
    // 176 autres — la seule façon de voir plus était de DESCENDRE d'un cran
    // (`/api/viz/space` avec un autre `prefix`), c'est-à-dire de changer
    // d'échelle pour une question qui portait sur la même.
    //
    // `offset` est cette page : `ORDER BY c DESC SKIP offset LIMIT 200`. Le front
    // densifie donc SANS changer de niveau, et le plafond de 200 n'est plus un
    // mur mais un pas — celui de la page suivante, chez nous.
    //
    // Bornage : `offset` est plafonné (une page très profonde ne sert personne
    // et ferait payer un SKIP coûteux), et la réponse DIT quand la page est
    // pleine (`tronque`), seule information honnête sur « il y en a d'autres »
    // sans compter les distincts — un second parcours complet pour un compteur.
    let offset = params
        .get("offset")
        .and_then(|o| o.parse::<usize>().ok())
        .unwrap_or(0)
        .min(OFFSET_MAX);
    if !store.exists() {
        return respond(
            stream,
            400,
            VIZ_JSON,
            &format!("{{\"error\":\"store introuvable : {}\"}}", store.display()),
        );
    }
    // Cache DURABLE, clé par (store, prefix, rank, links, OFFSET) : `top` ignoré
    // (toujours 200 ensembles par page, le front découpe) ; links=0 → agrégat
    // allégé (hover/rameaux). L'offset FAIT PARTIE DE LA CLÉ : sans lui, la page
    // 2 se servirait la page 1 depuis le cache — un mensonge silencieux.
    let mt = store_mtime(&store);
    use std::sync::{Mutex, OnceLock};
    static SP_CACHE: OnceLock<Mutex<std::collections::HashMap<
        (std::path::PathBuf, String, usize, bool, usize),
        (Option<u128>, String),
    >>> = OnceLock::new();
    let cache = SP_CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let key = (store.clone(), prefix.clone(), rank, links, offset);
    if let Some((m, body)) = verrou(&cache).get(&key).map(|(m, b)| (*m, b.clone())) {
        if m == mt {
            return respond(stream, 200, VIZ_JSON, &body);
        }
    }
    let sextra = format!("{rank}@{prefix}#{}@{offset}", if links { 1 } else { 0 });
    if let Some(b) = disk_load("sp", &store, &sextra, mt) {
        verrou(&cache).insert(key.clone(), (mt, b.clone()));
        return respond(stream, 200, VIZ_JSON, &b);
    }
    const TOP_EFF: usize = 200; // toujours 200 ensembles par page (le front découpe)
    /// Plafond de la pagination d'un niveau : au-delà, `SKIP` ferait payer un
    /// parcours dont personne ne se sert (200 groupes par page, 200 pages =
    /// 40 000 ensembles ; un niveau qui en porte plus est un défaut de
    /// découpage, pas une échelle qu'on explore à la main).
    const OFFSET_MAX: usize = 40_000;
    let db = match cached_ro(&store) {
        Ok(db) => db,
        Err(e) => {
            return respond(
                stream,
                400,
                VIZ_JSON,
                &format!("{{\"error\":\"ouverture RO : {e}\"}}"),
            );
        }
    };
    let sess = db.session();
    // Filtre de sous-espace : les qn sont des chemins tokens joints par '.'
    // → appartenance au préfixe = CONTAINS '.P1.P2.' (racine : aucun filtre).
    let in_prefix = |col: &str| -> String {
        if prefix.is_empty() {
            format!("{col}.qualified_name CONTAINS '.'")
        } else {
            format!("{col}.qualified_name CONTAINS '.{prefix}.'")
        }
    };
    let group_q = format!(
        "MATCH (n) WHERE {} \
         RETURN split(n.qualified_name, '.')[{rank}] AS m, count(n) AS c \
         ORDER BY c DESC SKIP {offset} LIMIT {TOP_EFF}",
        in_prefix("n")
    );
    let ensembles: Vec<serde_json::Value> = sess
        .execute(&group_q)
        .map(|r| {
            r.rows
                .iter()
                .filter_map(|row| {
                    let (Some(OV::String(m)), Some(OV::Int64(c))) = (row.first(), row.get(1)) else {
                        return None;
                    };
                    Some(serde_json::json!({ "name": m.to_string(), "count": *c as u64 }))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut out = serde_json::json!({
        "store": store, "prefix": prefix, "rank": rank,
        "ensembles": ensembles,
        // ── LA PAGE SE DIT ELLE-MÊME : OFFSET, ET PAGE PLEINE ───────────────
        // `tronque` (page pleine) est le seul aveu honnête de « il y en a
        // d'autres » sans compter les valeurs distinctes — ce qui coûterait un
        // second parcours complet pour un compteur. Le front en fait la règle 3
        // de `densite.ts` : tout est montré, mais le SERVEUR a coupé, donc
        // densifier devient un appel (`offset` suivant) et non une descente.
        "offset": offset,
        "tronque": ensembles.len() >= TOP_EFF,
    });
    if links {
        // Arcs CALLS entre enfants du sous-espace (paires src≠dst, pondérées).
        let engine_limit = 300usize.saturating_add(2048).min(50_000);
        let q = format!(
            "MATCH (a)-[r:CALLS]->(b) \
             WHERE {} AND {} \
             WITH split(a.qualified_name, '.')[{rank}] AS sa, \
                  split(b.qualified_name, '.')[{rank}] AS sb, r \
             RETURN sa, sb, count(r) AS w ORDER BY w DESC LIMIT {engine_limit}",
            in_prefix("a"),
            in_prefix("b")
        );
        let mut arcs: Vec<(String, String, u64)> = match sess.execute(&q) {
            Ok(res) => res
                .rows
                .iter()
                .filter_map(|row| {
                    let (Some(OV::String(sa)), Some(OV::String(sb)), Some(OV::Int64(w))) =
                        (row.first(), row.get(1), row.get(2))
                    else {
                        return None;
                    };
                    let (sa, sb) = (sa.to_string(), sb.to_string());
                    (sa != sb && !sa.is_empty() && !sb.is_empty()).then(|| (sa, sb, *w as u64))
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        arcs.sort_by(|x, y| y.2.cmp(&x.2));
        arcs.truncate(400);
        let list: Vec<serde_json::Value> = arcs
            .into_iter()
            .map(|(src, dst, weight)| serde_json::json!({ "src": src, "dst": dst, "weight": weight }))
            .collect();
        out["arcs"] = serde_json::Value::Array(list);
    }
    let body = out.to_string();
    verrou(&cache).insert(key.clone(), (mt, body.clone()));
    // ── OPTIMISATION AUTOMATIQUE (structure-agnostique) : dès qu'un niveau
    // est agrégé, on PRÉCHAUFFE en arrière-plan les enfants de ses ensembles
    // dominants (ensembles-only, même cache que cet endpoint) → les survols /
    // plongées suivants sont servis par cache, dans n'importe quel graphe.
    {
        static WARM_BUSY: std::sync::OnceLock<
            std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>
        > = std::sync::OnceLock::new();
        let busy = WARM_BUSY.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()));
        let already = {
            let mut b = verrou(&busy);
            !b.insert(store.clone())
        };
        if !already {
            let s2 = store.clone();
            let p2 = prefix.clone();
            let children: Vec<String> = ensembles
                .iter()
                .filter_map(|e| e.get("name").and_then(|n| n.as_str()).map(String::from))
                .filter(|n| !n.is_empty() && !n.contains('@') && !n.starts_with('0'))
                .take(3)
                .collect();
            std::thread::spawn(move || {
                let mut first_child: Option<(String, Vec<serde_json::Value>)> = None;
                // ── `cached_ro`, PAS `open_read_only` (2026-09-13) ──────────
                // Défaut mesuré : ce fil appelait `ObrainDB::open_read_only` EN
                // DIRECT, donc il ne voyait pas le cache de bases de
                // `cached_ro` — et `runtime-dev` est un store de 11 Go dont
                // l'OUVERTURE COMPLÈTE coûte des centaines de millisecondes (et
                // re-audite les compteurs, mesuré 16-46 ms, puis rejette
                // l'index ordonné). Preuve croisée : le journal du service
                // comptait 8 ouvertures de base par un processus vivant depuis
                // 15:55, dont 5 pendant une seule passe de mesure.
                //
                // Le HANDLE est le même que celui des autres routes (même base,
                // même mode LECTURE SEULE : `cached_ro` n'ouvre que par
                // `open_read_only`), et l'ouverture est IDEMPOTENTE — le
                // préchauffage a donc exactement le même effet, sans rouvrir.
                // Seule différence assumée : le handle passe par
                // `with_query_timeout`, donc ces requêtes de préchauffage sont
                // bornées, comme celles des routes.
                if let Ok(db) = cached_ro(&s2) {
                    let sess = db.session();
                    let mt2 = store_mtime(&s2);
                    let cache2 = SP_CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
                    for c in &children {
                        let cp = if p2.is_empty() { c.clone() } else { format!("{p2}.{c}") };
                        let ck = (s2.clone(), cp.clone(), rank + 1, false, 0);
                        if verrou(&cache2).contains_key(&ck) { continue; }
                        let q = format!(
                            "MATCH (n) WHERE n.qualified_name CONTAINS '.{cp}.' \
                             RETURN split(n.qualified_name, '.')[{}] AS m, count(n) AS c \
                             ORDER BY c DESC LIMIT 200",
                            rank + 1
                        );
                        let ens: Vec<serde_json::Value> = sess
                            .execute(&q)
                            .map(|r| {
                                r.rows.iter().filter_map(|row| {
                                    let (Some(OV::String(m)), Some(OV::Int64(c))) = (row.first(), row.get(1)) else { return None; };
                                    Some(serde_json::json!({ "name": m.to_string(), "count": *c as u64 }))
                                }).collect()
                            })
                            .unwrap_or_default();
                        // La page préchauffée est la PREMIÈRE (offset 0), et elle
                        // se présente comme les autres : sans `offset`/`tronque`,
                        // un corps servi depuis ce cache aurait l'air d'une page
                        // incomplète — le front y lirait « rien après ».
                        let b2 = serde_json::json!({
                            "store": s2, "prefix": cp, "rank": rank + 1, "ensembles": ens,
                            "offset": 0, "tronque": ens.len() >= 200
                        }).to_string();
                        verrou(&cache2).insert(ck, (mt2, b2));
                        if first_child.is_none() {
                            first_child = Some((cp.clone(), ens));
                        }
                    }
                    // Étape C-lite : préchauffe aussi les petits-enfants du rameau dominant
                    if let Some((cp1, ens1)) = first_child.clone() {
                        if let Some(top) = ens1.iter()
                            .filter_map(|e| e.get("name").and_then(|n| n.as_str()).map(String::from))
                            .find(|n| !n.is_empty() && !n.contains('@') && !n.starts_with('0'))
                        {
                            let gp = format!("{cp1}.{top}");
                            let gk = (s2.clone(), gp.clone(), rank + 2, false, 0);
                            if !verrou(&cache2).contains_key(&gk) {
                                let qg = format!(
                                    "MATCH (n) WHERE n.qualified_name CONTAINS '.{gp}.' \
                                     RETURN split(n.qualified_name, '.')[{}] AS m, count(n) AS c \
                                     ORDER BY c DESC LIMIT 200",
                                    rank + 2
                                );
                                let gens: Vec<serde_json::Value> = sess
                                    .execute(&qg)
                                    .map(|r| {
                                        r.rows.iter().filter_map(|row| {
                                            let (Some(OV::String(m)), Some(OV::Int64(c))) = (row.first(), row.get(1)) else { return None; };
                                            Some(serde_json::json!({ "name": m.to_string(), "count": *c as u64 }))
                                        }).collect()
                                    })
                                    .unwrap_or_default();
                                let bg = serde_json::json!({
                                    "store": s2, "prefix": gp, "rank": rank + 2, "ensembles": gens,
                                    "offset": 0, "tronque": gens.len() >= 200
                                }).to_string();
                                verrou(&cache2).insert(gk, (mt2, bg));
                            }
                        }
                    }
                }
                verrou(&busy).remove(&s2);
            });
        }
    }
    disk_save("sp", &store, &sextra, mt, &body);
    respond(stream, 200, VIZ_JSON, &body)
}

/// `GET /api/viz/signatures?stores=` — signatures par store (nœuds/arêtes +
/// histogramme de types) : socle du COSMOS (une galaxie par base + similarités).
fn api_viz_signatures(
    stream: &mut TcpStream,
    entetes: &str,
    query: Option<&str>,
    config: &ServeConfig,
) -> Result<()> {
    use obrain_common::types::Value as OV;
    let params = parse_query(query);
    // ── ON NE SIGNE QUE CE QUE L'APPELANT PEUT VOIR ────────────────────────
    // La garde a jugé l'espace par défaut ; ce handler agrège PLUSIEURS bases.
    // Sans ce filtre, `?stores=` contournait la garde (mesuré : 200 ici sur un
    // espace que `subgraph` refuse à 403). La clé de cache suit `targets`,
    // donc deux visibilités différentes ne partagent JAMAIS un corps caché.
    let visibles = mondes_visibles(entetes, query, config);
    // stores explicites (chemins/labels séparés par virgule) sinon presets.
    let wanted = params.get("stores").map(String::as_str).unwrap_or("");
    let targets: Vec<(String, std::path::PathBuf)> = if wanted.is_empty() {
        visibles
    } else {
        wanted
            .split(',')
            .filter_map(|t| config.resolve(Some(t.trim())).map(|p| (t.trim().to_string(), p)))
            .filter(|(_, p)| visibles.iter().any(|(_, v)| v == p))
            .collect()
    };
    // ── CACHE DURABLE ──────────────────────────────────────────────────────
    // Trois requêtes par base (count(n), count(r), agrégation par label — cette
    // dernière coûte ~225 ms à elle seule) : mesuré ~1,1 s À CHAQUE APPEL sur 12
    // bases, à chaud comme à froid, parce que le résultat ne passait par aucun
    // cache. C'est l'entrée de la vue Cosmos : on la paie à chaque ouverture.
    //
    // Clé STABLE (l'ensemble des bases, pas leurs mtimes) pour que le fichier ne
    // se multiplie pas ; la FRAÎCHEUR passe par `mt`, empreinte combinée des
    // mtimes — toute écriture dans n'importe laquelle des bases invalide.
    let mut empreinte: u64 = 0;
    let mut cle = String::new();
    for (label, path) in &targets {
        cle.push_str(label);
        cle.push('=');
        cle.push_str(&path.to_string_lossy());
        cle.push(';');
        empreinte = empreinte.rotate_left(7) ^ (store_mtime(path).unwrap_or(0) as u64);
    }
    if !targets.is_empty()
        && let Some(b) = disk_load("sig", &targets[0].1, &cle, Some(empreinte as u128))
    {
        return respond(stream, 200, VIZ_JSON, &b);
    }

    let out: Vec<serde_json::Value> = targets
        .iter()
        .filter_map(|(label, path)| {
            let db = cached_ro(path).ok()?;
            let sess = db.session();
            let nodes = sess
                .execute("MATCH (n) RETURN count(n)")
                .ok()
                .and_then(|r| {
                    r.rows.first().and_then(|row| row.first()).and_then(|v| match v {
                        OV::Int64(x) => Some(*x as u64),
                        OV::Float64(x) => Some(*x as u64),
                        _ => None,
                    })
                });
            let edges = sess
                .execute("MATCH ()-[r]->() RETURN count(r)")
                .ok()
                .and_then(|r| {
                    r.rows.first().and_then(|row| row.first()).and_then(|v| match v {
                        OV::Int64(x) => Some(*x as u64),
                        OV::Float64(x) => Some(*x as u64),
                        _ => None,
                    })
                });
            let by_label = sess
                .execute("MATCH (n) RETURN labels(n)[0] AS t, count(n) AS nbr ORDER BY nbr DESC LIMIT 10")
                .map(|r| {
                    r.rows
                        .iter()
                        .filter_map(|row| {
                            let (Some(OV::String(t)), Some(OV::Int64(c))) = (row.first(), row.get(1))
                            else {
                                return None;
                            };
                            Some(serde_json::json!({ "label": t.to_string(), "count": *c as u64 }))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            Some(serde_json::json!({
                "label": label, "path": path.to_string_lossy(),
                "nodes": nodes, "edges": edges, "by_label": by_label,
            }))
        })
        .collect();
    let body = serde_json::json!({ "stores": out }).to_string();
    if !targets.is_empty() {
        disk_save("sig", &targets[0].1, &cle, Some(empreinte as u128), &body);
    }
    respond(stream, 200, VIZ_JSON, &body)
}

/// ── LA SIGNATURE D'UN MONDE, MÉMOÏSÉE PAR MONDE (2026-09-13) ──────────────
///
/// **Ce que la fraîcheur garantit après ce changement.** Il y a DEUX étages, et
/// chacun a SA clé :
///
///   · le CORPS de la toile (`disk_load("weave", …)`) est validé par
///     l'empreinte combinée des mtimes de tous les mondes — inchangé. Une
///     écriture dans n'importe quel monde produit donc une empreinte différente,
///     et la toile est RÉASSEMBLÉE : rien de périmé ne peut être servi ;
///   · la SIGNATURE d'un monde (nœuds, arêtes, types) est validée par le mtime
///     de CE monde SEUL. C'est ce qui évite de re-signer les mondes qui n'ont
///     pas bougé quand un autre a bougé — la dépense que l'ancien code refaisait
///     intégralement, ouverture de base comprise.
///
/// Autrement dit : le changement ne touche pas à la FRAÎCHEUR (elle reste
/// portée par les mtimes, tous les mtimes), il touche au TRAVAIL de
/// re-signature. La signature mémoïsée d'un monde est vraie tant que CE monde
/// n'a pas changé — c'est exactement la propriété qu'on veut.
fn signature_monde(path: &std::path::Path) -> Option<serde_json::Value> {
    use std::sync::{Mutex, OnceLock};
    static WSIG: OnceLock<
        Mutex<std::collections::HashMap<std::path::PathBuf, (Option<u128>, serde_json::Value)>>,
    > = OnceLock::new();
    let cache = WSIG.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let mt = store_mtime(path);
    if let Some((m, v)) = verrou(&cache).get(path).map(|(m, v)| (*m, v.clone())) {
        if m == mt {
            tracing::debug!(monde = %path.display(), "signature de monde servie par la mémoire");
            return Some(v);
        }
    }
    if let Some(txt) = disk_load("wsig", path, "", mt) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
            verrou(&cache).insert(path.to_path_buf(), (mt, v.clone()));
            tracing::debug!(monde = %path.display(), "signature de monde servie par le disque");
            return Some(v);
        }
    }
    let db = match cached_ro(path) {
        Ok(db) => db,
        Err(e) => {
            tracing::debug!(monde = %path.display(), error = %e,
                "signature de monde impossible — le monde sera absent de la toile");
            return None;
        }
    };
    let sess = db.session();
    let n = |gql: &str| -> i64 {
        sess.execute(gql)
            .ok()
            .and_then(|r| r.rows.into_iter().next())
            .and_then(|row| row.into_iter().next())
            .and_then(|v| match v {
                obrain_common::types::Value::Int64(x) => Some(x),
                obrain_common::types::Value::Float64(x) => Some(x as i64),
                _ => None,
            })
            .unwrap_or(0)
    };
    let types: Vec<serde_json::Value> = sess
        .execute("MATCH (n) RETURN labels(n)[0] AS t, count(n) AS w ORDER BY w DESC LIMIT 10")
        .map(|r| {
            r.rows
                .iter()
                .filter_map(|row| {
                    let (Some(obrain_common::types::Value::String(t)), Some(c)) =
                        (row.first(), row.get(1))
                    else {
                        return None;
                    };
                    let cnt = match c {
                        obrain_common::types::Value::Int64(x) => *x,
                        obrain_common::types::Value::Float64(x) => *x as i64,
                        _ => 0,
                    };
                    Some(serde_json::json!({ "label": t.to_string(), "count": cnt }))
                })
                .collect()
        })
        .unwrap_or_default();
    let v = serde_json::json!({
        "nodes": n("MATCH (n) RETURN count(n)"),
        "edges": n("MATCH ()-[r]->() RETURN count(r)"),
        "types": types,
    });
    tracing::debug!(monde = %path.display(), "signature de monde CALCULÉE (base ouverte + GQL)");
    disk_save("wsig", path, "", mt, &v.to_string());
    verrou(&cache).insert(path.to_path_buf(), (mt, v.clone()));
    Some(v)
}

/// `GET /api/viz/weave?focus=<spec>` — **la toile de l'univers** : la base
/// synaptique durable de l'agent (produite par `obrain viz weave`) augmentée
/// des FOYERS contextuels réels. Le centre de la vue Cosmos n'est pas fixe :
/// il dépend de l'instant, de l'agent, de la conversation, du sujet, de
/// l'objectif ou de l'utilisateur — c'est ce que renvoie `foci`.
fn api_viz_weave(
    stream: &mut TcpStream,
    entetes: &str,
    query: Option<&str>,
    config: &ServeConfig,
) -> Result<()> {
    use obrain_common::types::Value as OV;
    let params = parse_query(query);
    // Ce que L'IDENTITÉ appelante a le droit de voir — la clé de cache et la
    // toile rendue en découlent toutes les deux.
    let visibles = mondes_visibles(entetes, query, config);
    let store = params
        .get("store")
        .map(std::path::PathBuf::from)
        .or_else(|| config.synapse_store.clone());
    let Some(store) = store else {
        let body = serde_json::json!({
            "schema": crate::viz_weave::WEAVE_SCHEMA,
            "available": false,
            "reason": "aucune base synaptique configurée — lancez `obrain viz weave` \
                       puis `viz serve --synapse-store <chemin>`",
        })
        .to_string();
        return respond(stream, 200, VIZ_JSON, &body);
    };
    // ── CACHE DURABLE ──────────────────────────────────────────────────────
    // L'artefact persisté se relit vite, mais les MONDES AJOUTÉS À CHAUD sont
    // re-signés à chaque appel (une session + deux requêtes par monde absent de
    // la toile) : mesuré ~150 ms À CHAQUE APPEL. Clé : base synaptique + ensemble
    // des mondes ; fraîcheur : empreinte de leurs mtimes (un monde qui bouge, ou
    // une toile retissée, invalide).
    let mut empreinte: u64 = store_mtime(&store).unwrap_or(0) as u64;
    let mut cle = store.to_string_lossy().to_string();
    for (label, path) in &visibles {
        cle.push_str(&label);
        cle.push('=');
        cle.push_str(&path.to_string_lossy());
        cle.push(';');
        empreinte = empreinte.rotate_left(7) ^ (store_mtime(&path).unwrap_or(0) as u64);
    }
    if let Some(b) = disk_load("weave", &store, &cle, Some(empreinte as u128)) {
        return respond(stream, 200, VIZ_JSON, &b);
    }

    let Some(mut payload) = crate::viz_weave::read_persisted(&store) else {
        let body = serde_json::json!({
            "schema": crate::viz_weave::WEAVE_SCHEMA,
            "available": false,
            "reason": format!("base synaptique absente : {}", store.display()),
        })
        .to_string();
        return respond(stream, 200, VIZ_JSON, &body);
    };

    // ── LA TOILE PERSISTÉE CONTIENT TOUS LES MONDES ; L'APPELANT, NON ─────
    // L'artefact est tissé une fois pour toutes, sans identité. On le retaille
    // ici à ce que l'appelant peut lire : mondes hors droits retirés, et les
    // fibres qui les touchaient avec eux — une fibre vers un monde invisible
    // est déjà une fuite (elle nomme ses ancres dans `sample`).
    if let Some(obj) = payload.as_object_mut() {
        let permis: std::collections::HashSet<String> = visibles
            .iter()
            .map(|(_, p)| p.to_string_lossy().to_string())
            .collect();
        let mut qns_gardes: std::collections::HashSet<String> = Default::default();
        if let Some(arr) = obj.get_mut("worlds").and_then(|w| w.as_array_mut()) {
            arr.retain(|w| {
                let garde = w
                    .get("path")
                    .and_then(|p| p.as_str())
                    .is_some_and(|p| permis.contains(p));
                if garde && let Some(qn) = w.get("qn").and_then(|q| q.as_str()) {
                    qns_gardes.insert(qn.to_string());
                }
                garde
            });
        }
        if let Some(arr) = obj.get_mut("fibers").and_then(|f| f.as_array_mut()) {
            arr.retain(|f| {
                let bout = |k: &str| {
                    f.get(k).and_then(|v| v.as_str()).is_some_and(|q| qns_gardes.contains(q))
                };
                bout("a") && bout("b")
            });
            let fibres = arr.len();
            if let Some(c) = obj.get_mut("counts").and_then(|c| c.as_object_mut()) {
                c.insert("fibers".into(), serde_json::json!(fibres));
            }
        }
    }

    // ── MONDES AJOUTÉS À CHAUD, PAS ENCORE TISSÉS ─────────────────────────
    // La toile est un artefact PERSISTÉ (produit par `obrain viz weave`). Un
    // monde ajouté depuis l'interface n'y est donc pas — et l'utilisateur ne
    // verrait pas ce qu'il vient d'ajouter. On l'ajoute donc ici, avec sa
    // signature RÉELLE (nœuds, arêtes, types) et marqué `tisse: false` : il
    // apparaît comme une masse, sans fibre, ce qui est exactement la vérité
    // (aucune preuve de connexion n'a encore été calculée pour lui).
    let mut ajoutes = 0usize;
    if let Some(obj) = payload.as_object_mut() {
        let connus: Vec<String> = obj
            .get("worlds")
            .and_then(|w| w.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|w| w.get("path").and_then(|p| p.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let mut nouveaux: Vec<serde_json::Value> = Vec::new();
        for (label, path) in &visibles {
            if connus.contains(&path.to_string_lossy().to_string()) {
                continue;
            }
            // ── LA SIGNATURE D'UN MONDE SE MÉMOÏSE, ET PAR MONDE ────────────
            // Défaut mesuré : pour CHAQUE monde absent de la toile, on ouvrait
            // la base (`cached_ro`) et on jouait 2-3 GQL — l'ouverture froide de
            // `linux-full3` est documentée à **1 236 ms**, et le
            // `labels(n)[0] … LIMIT 10` a été mesuré à **106 ms**. Comme la clé
            // du corps combine les mtimes de TOUS les mondes, une écriture dans
            // UN monde invalidait la toile entière — et la re-signature repartait
            // pour TOUS les mondes absents, y compris ceux qui n'avaient pas
            // bougé d'un octet. `signature_monde` rend la même vérité (nœuds,
            // arêtes, types), mais validée par le mtime DU MONDE : une écriture
            // dans A ne peut plus rendre la signature de B fausse — seulement la
            // laisser en cache.
            let Some(sig) = signature_monde(&path) else { continue };
            let types: Vec<serde_json::Value> = sig
                .get("types")
                .and_then(|t| t.as_array())
                .cloned()
                .unwrap_or_default();
            let nodes = sig.get("nodes").and_then(|x| x.as_i64()).unwrap_or(0);
            // ── UNE SEULE RÈGLE DE SLUG ─────────────────────────────────────
            // Ici vivait une règle EN LIGNE qui différait de `viz_weave::slug` :
            // elle n'écrasait pas les tirets consécutifs. Pour le même libellé
            // « this-rs/this (GitHub) », elle produisait `this-rs-this--github`
            // là où le tissage produit `this-rs-this-github` — DEUX qn pour un
            // seul monde. Conséquence mesurée : le Cosmos envoyait la forme
            // « à tirets doubles », la résolution du chat cherchait l'autre, et
            // la conversation retombait sur le premier monde de la liste
            // (runtime-dev) en annonçant `this-rs/this`.
            //
            // Un monde nommé par deux règles, c'est un monde introuvable. La
            // règle du tissage fait désormais foi, ici comme partout.
            let slug: String = crate::viz_weave::slug(&label);
            // ── LA TAILLE DOIT DIRE LA VÉRITÉ ────────────────────────────────
            // Ce monde recevait `mass: 0.5` EN DUR. Conséquence mesurée : deux
            // mondes de 310 547 et 1 378 nœuds — un rapport de 225 — obtenaient
            // la même masse à 0,007 près, donc la même taille à l'écran. On
            // reprend donc la loi des autres mondes, telle que le tissage la
            // calcule : masse = log10(1 + nœuds) normalisée par le plus grand.
            // Rien n'est inventé ici : on applique la règle existante à un monde
            // qui y échappait.
            let max_log = obj
                .get("worlds")
                .and_then(|w| w.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.get("nodes").and_then(|v| v.as_f64()))
                        .map(|v| (1.0f64 + v).log10())
                        .fold((1.0f64 + nodes as f64).log10(), f64::max)
                })
                .unwrap_or(1.0);
            let masse = ((1.0f64 + nodes as f64).log10() / max_log.max(1e-9) * 1000.0).round() / 1000.0;

            ajoutes += 1;
            nouveaux.push(serde_json::json!({
                "qn": format!("world:{}", slug.trim_matches('-')),
                "slug": slug.trim_matches('-'),
                "label": label,
                "path": path.to_string_lossy(),
                "nature": "connaissance",
                "nodes": nodes,
                "edges": sig.get("edges").and_then(|x| x.as_i64()).unwrap_or(0),
                "types": types,
                "sessions": 0,
                "mass": masse,
                "gravity": masse * 8.0,
                "orbit": 1.0,
                // AUCUNE fibre : la toile n'a pas encore été retissée pour lui.
                "tisse": false,
            }));
        }
        if !nouveaux.is_empty()
            && let Some(arr) = obj.get_mut("worlds").and_then(|w| w.as_array_mut())
        {
            arr.extend(nouveaux);
        }
        if let Some(c) = obj.get_mut("counts").and_then(|c| c.as_object_mut()) {
            c.insert("worlds".into(), serde_json::json!(connus.len() + ajoutes));
            c.insert("mondes_non_tisses".into(), serde_json::json!(ajoutes));
        }
    }

    // ── foyers : ce qui peut occuper le CENTRE, selon le contexte réel ──
    let foci = cached_foci(config);
    let chosen = params.get("focus").cloned().unwrap_or_default();
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("available".into(), serde_json::json!(true));
        obj.insert("foci".into(), foci.clone());
        obj.insert(
            "focus".into(),
            serde_json::json!({
                "requested": chosen,
                "default": foci.as_array().and_then(|a| a.first()).cloned()
                    .unwrap_or(serde_json::Value::Null),
            }),
        );
    }
    let body = payload.to_string();
    disk_save("weave", &store, &cle, Some(empreinte as u128), &body);
    let _ = OV::Null; // (garde l'import explicite lisible pour la parité viz)
    respond(stream, 200, VIZ_JSON, &body)
}

/// Foyers contextuels candidats, lus dans les mondes réels puis mémorisés
/// 60 s (les presets ne changent pas d'une seconde à l'autre).
fn cached_foci(config: &ServeConfig) -> serde_json::Value {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<Option<(std::time::Instant, serde_json::Value)>>> = OnceLock::new();
    let cell = CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(g) = cell.lock()
        && let Some((t, v)) = g.as_ref()
        && t.elapsed().as_secs() < 60
    {
        return v.clone();
    }
    let value = compute_foci(config);
    if let Ok(mut g) = cell.lock() {
        *g = Some((std::time::Instant::now(), value.clone()));
    }
    value
}

/// Vrai pour un libellé qui n'est qu'un identifiant technique (GUID/hash) :
/// un tel libellé ne peut pas servir de nom de foyer lisible.
fn est_identifiant_brut(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_hexdigit() || c == '-' || c == '_')
}

/// Lit les foyers réels : agent (Self), utilisateur (Account), conversation
/// (session la plus récente), objectifs (Goal), et l'instant (maintenant).
fn compute_foci(config: &ServeConfig) -> serde_json::Value {
    use obrain_common::types::Value as OV;
    let str_of = |v: Option<&OV>| -> Option<String> {
        match v? {
            OV::String(s) => Some(s.to_string()),
            OV::Int64(x) => Some(x.to_string()),
            _ => None,
        }
    };
    let one = |db: &obrain_engine::ObrainDB, gql: &str| -> Option<Vec<OV>> {
        db.session().execute(gql).ok()?.rows.into_iter().next()
    };
    let mut foci: Vec<serde_json::Value> = Vec::new();
    let mut self_qn: Option<String> = None;
    let mut user_qn: Option<String> = None;
    let mut best_session: Option<(String, i64, String)> = None;
    let mut goals: Vec<(String, String)> = Vec::new();

    for (label, path) in &tous_les_mondes(config) {
        let Ok(db) = cached_ro(path) else { continue };
        if self_qn.is_none()
            && let Ok(res) = db
                .session()
                .execute("MATCH (s:Self) RETURN s.qualified_name, s.name, s.role LIMIT 8")
        {
            // Exigeant : le foyer « agent » est le Self RÉEL (`self:<agent>`),
            // pas un nœud étiqueté Self par accident dans un monde de code.
            for row in res.rows {
                if let Some(qn) = str_of(row.first())
                    && qn.starts_with("self:")
                {
                    self_qn = Some(qn.clone());
                    foci.push(serde_json::json!({
                        "kind": "agent", "qn": qn,
                        "label": str_of(row.get(1)).unwrap_or_else(|| "Self".into()),
                        "detail": str_of(row.get(2)).unwrap_or_else(|| "role inconnu".into()),
                        "world": label,
                    }));
                    break;
                }
            }
        }
        if user_qn.is_none()
            && let Some(r) = one(
                &db,
                "MATCH (a:Account) RETURN a.qualified_name, a.name LIMIT 1",
            )
            && let Some(qn) = str_of(r.first())
        {
            user_qn = Some(qn.clone());
            foci.push(serde_json::json!({
                "kind": "utilisateur", "qn": qn,
                "label": str_of(r.get(1)).unwrap_or_else(|| "Utilisateur".into()),
                "detail": "compte propriétaire des sessions",
                "world": label,
            }));
        }
        if let Some(r) = one(
            &db,
            "MATCH (s:Session) RETURN s.qualified_name, s.createdAt, s.title \
             ORDER BY s.createdAt DESC LIMIT 1",
        ) {
            let qn = str_of(r.first());
            let t = match r.get(1) {
                Some(OV::Int64(x)) => Some(*x),
                Some(OV::Float64(x)) => Some(*x as i64),
                _ => None,
            };
            if let (Some(qn), Some(t)) = (qn, t)
                && best_session.as_ref().is_none_or(|(_, bt, _)| t > *bt)
            {
                best_session = Some((
                    qn,
                    t,
                    str_of(r.get(2)).unwrap_or_else(|| label.clone()),
                ));
            }
        }
        // Objectifs : on garde ceux qui portent un NOM lisible. Un monde qui
        // n'a que des identifiants bruts (GUID) ne doit pas voler le foyer.
        if let Ok(res) = db
            .session()
            .execute("MATCH (g:Goal) RETURN g.qualified_name, g.name LIMIT 6")
        {
            let candidats: Vec<(String, String)> = res
                .rows
                .into_iter()
                .filter_map(|row| Some((str_of(row.first())?, str_of(row.get(1))?)))
                .collect();
            let nomme = candidats.iter().any(|(_, n)| !est_identifiant_brut(n));
            if !candidats.is_empty() && (nomme || goals.is_empty()) {
                goals = candidats;
            }
        }
    }

    if let Some((qn, t, title)) = best_session {
        foci.insert(
            0,
            serde_json::json!({
                "kind": "conversation", "qn": qn, "label": title,
                "detail": format!("session la plus récente (t={t})"),
                "ts": t,
            }),
        );
    }
    foci.push(serde_json::json!({
        "kind": "instant", "qn": "now", "label": "Maintenant",
        "detail": "le foyer est l'instant courant : la gravité suit la récence",
        "ts": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64).unwrap_or(0),
    }));
    for (qn, label) in goals {
        foci.push(serde_json::json!({
            "kind": "objectif", "qn": qn, "label": label,
            "detail": "objectif déclaré du self",
        }));
    }
    foci.push(serde_json::json!({
        "kind": "sujet", "qn": "", "label": "Sujet libre",
        "detail": "le foyer est un sujet/question posé (recherche)",
    }));
    serde_json::json!(foci)
}

/// `GET /api/viz/activation?store=&window=` — ce que l'engine a CONSULTÉ, avec
/// son énergie décrue. C'est la source de l'activation en temps réel : lue sur
/// le journal partagé (voir `crate::activation`), donc elle voit AUSSI ce qu'un
/// autre processus (serveur MCP) a touché, pas seulement le serveur viz.
fn api_viz_activation(
    stream: &mut TcpStream,
    query: Option<&str>,
    config: &ServeConfig,
) -> Result<()> {
    let params = parse_query(query);
    let window = params
        .get("window")
        .and_then(|w| w.parse::<i64>().ok())
        .unwrap_or(60_000);
    // Sans `store`, on agrège TOUS les mondes : le Cosmos montre toutes les
    // bases à la fois, il lui faut une seule requête pour savoir lesquelles
    // l'engine vient de consulter. Chaque hit porte donc son store.
    let targets: Vec<std::path::PathBuf> = match params.get("store") {
        Some(st) => config.resolve(Some(st)).into_iter().collect(),
        None => tous_les_mondes(config).into_iter().map(|(_, p)| p).collect(),
    };
    if targets.is_empty() {
        let body = serde_json::json!({ "error": "aucun store" }).to_string();
        return respond(stream, 200, VIZ_JSON, &body);
    }
    let mut hits: Vec<serde_json::Value> = Vec::new();
    let mut par_monde: Vec<serde_json::Value> = Vec::new();
    for path in &targets {
        let h = crate::activation::read(path, window);
        // énergie d'un MONDE = la plus forte activation de ses nœuds : c'est ce
        // qui fait s'allumer une planète quand l'engine la consulte.
        let peak = h.iter().map(|x| x.energy).fold(0.0_f64, f64::max);
        par_monde.push(serde_json::json!({
            "store": path.to_string_lossy(),
            "peak": (peak * 1000.0).round() / 1000.0,
            "nodes": h.len(),
        }));
        for x in h {
            hits.push(serde_json::json!({
                "store": path.to_string_lossy(),
                "qn": x.qn, "kind": x.kind, "t": x.t,
                "energy": (x.energy * 1000.0).round() / 1000.0,
            }));
        }
    }
    hits.sort_by(|a, b| {
        b["energy"].as_f64().unwrap_or(0.0)
            .partial_cmp(&a["energy"].as_f64().unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(600);
    let body = serde_json::json!({
        "now": crate::activation::now_ms(),
        "half_life_ms": crate::activation::HALF_LIFE_MS,
        "worlds": par_monde,
        "hits": hits,
    })
    .to_string();
    respond(stream, 200, VIZ_JSON, &body)
}

/// `GET /api/engine/sante` — le service LLM local répond-il ? On dit POURQUOI
/// il est indisponible plutôt que d'échouer sans explication.
fn api_engine_sante(stream: &mut TcpStream) -> Result<()> {
    let body = crate::engine::sante().to_string();
    respond(stream, 200, VIZ_JSON, &body)
}

/// RÉSOUT L'ANCRAGE D'UN SUJET — **quel store faut-il réellement ouvrir ?**
///
/// Défaut mesuré le 2026-09-11, reproduit : le chat annonçait « ancré sur
/// `world:this-rs-this--github` » et lisait **runtime-dev**. Deux causes
/// cumulées côté vue — un libellé (`focus.world` = « Runtime dev (engrams) »)
/// comparé à un `qn` (`world:runtime-dev-engrams`), jamais égal, puis un repli
/// sur `worlds[0]` : le chat était donc câblé sur le **premier monde**.
///
/// La résolution est faite ici, sur la **base synaptique** (autorité du Cosmos :
/// `World.path`), avec le repli par slug pour un monde pas encore tissé. Elle
/// rend aussi l'`Ancrage` qui repart vers l'écran : la vue affiche ce qui a été
/// **résolu**, elle ne l'interprète plus.
fn ancrer_sujet(
    config: &ServeConfig,
    entetes: &str,
    store_param: Option<&str>,
    qn: &str,
    // L'identité de l'appelant (`utilisateur@compte`), quand il y en a une :
    // c'est elle qui décide QUEL Self porte la mémoire de cette conversation.
    identite: Option<&str>,
) -> Option<(std::path::PathBuf, crate::engine::Ancrage)> {
    // ── LE SELF EST CELUI DE L'APPELANT ─────────────────────────────────────
    // Défaut mesuré le 2026-09-15 : ce chemin résolvait le self de l'INSTANCE
    // (`MATCH (s:Self) RETURN s.qn LIMIT 1`) pendant que la vue mémoire, elle,
    // résolvait déjà le self du compte. Conséquence : deux comptes voyaient
    // chacun leur self dans le panneau, et la CONVERSATION écrivait pourtant
    // ses souvenirs dans celui de l'instance — un ami rangeait donc ses
    // souvenirs chez quelqu'un d'autre. Un seul résolveur pour tout le monde
    // (`ancrage::self_qn_pour`), et le chemin du chat passe par lui.
    let (self_qn, _portee) =
        crate::ancrage::self_qn_pour(config.synapse_store.as_deref(), identite);
    if crate::ancrage::est_un_monde(qn)
        && let Some(m) = crate::ancrage::monde_du_sujet(
            config.synapse_store.as_deref(),
            qn,
            &tous_les_mondes(config),
        )
    {
        let ancrage = crate::engine::Ancrage {
            store: m.path.to_string_lossy().to_string(),
            monde: Some(m.label.clone()),
            self_qn,
        };
        return Some((m.path, ancrage));
    }
    // Un sujet qui n'est pas un monde (ou un monde introuvable) : le store
    // demandé fait foi. On ne devine pas — c'est précisément ce qui produisait
    // une réponse confiante sur la mauvaise base.
    //
    // ── QUAND AUCUNE BASE N'EST NOMMÉE, C'EST LE DÉFAUT DU SERVEUR ───────────
    // Défaut CONNU, mesuré le 2026-09-15 en traçant « le flux s'est fermé avant la
    // première réponse » : un compte qui n'a pas cet espace est refusé en 403
    // alors qu'il n'a jamais visé cette base. Une correction (juger la première
    // base LISIBLE par l'appelant) a été tentée puis RETIRÉE : elle refusait des
    // requêtes légitimes, parce que la résolution des droits rend une liste VIDE
    // aussi bien pour « aucun droit » que pour « je n'ai pas pu regarder ».
    // Refuser sur une mesure qu'on ne sait pas interpréter est pire que refuser
    // tard — et le refus dit maintenant sa cause (voir `router_requete`).
    let store = config.resolve(store_param)?;
    let ancrage = crate::engine::Ancrage {
        store: store.to_string_lossy().to_string(),
        monde: None,
        self_qn,
    };
    Some((store, ancrage))
}

/// `GET /api/entretien` — les réparations **dues**, et depuis quand.
///
/// Le protocole des demandes en vol a trois temps : le producteur attache
/// (pendant que la base tourne et que la réparation est impossible), le
/// consommateur applique et accorde (à la fenêtre du redémarrage), et — sans
/// cette route — **personne ne voit rien entre les deux**. Une réparation due
/// depuis trois jours doit se voir dans l'interface, pas seulement dans la sortie
/// d'une commande que personne ne lance.
fn api_entretien(stream: &mut TcpStream) -> Result<()> {
    let demandes = crate::entretien::en_attente();
    let items: Vec<serde_json::Value> = demandes
        .iter()
        .map(|d| {
            serde_json::json!({
                "store": d.store,
                "store_court": std::path::Path::new(&d.store)
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_else(|| d.store.clone()),
                "genre": d.genre.nom(),
                "raison": d.raison,
                "source": d.source,
                "t": d.t_ms,
                "age_s": ((crate::activation::now_ms() - d.t_ms) / 1000).max(0),
            })
        })
        .collect();
    let body = serde_json::json!({
        "en_attente": demandes.len(),
        "age_max_s": crate::entretien::age_max_secondes(),
        "demandes": items,
        "appliquer": "scripts/vizserve.sh  (l\'entretien s\'exécute pendant le redémarrage)",
    })
    .to_string();
    respond(stream, 200, VIZ_JSON, &body)
}

/// `GET /api/engine/memoire` — ce que la conversation a structuré, en attente.
///
/// Le journal est un fichier append-only (voir `crate::memoire`) : il survit aux
/// redémarrages. L'application dans la base du Self exige une ouverture
/// EXCLUSIVE, que le service ne peut pas prendre — d'où `scripts/memoire.sh`.
/// Cet endpoint dit donc l'état, et la commande qui applique.
fn api_engine_memoire(
    stream: &mut TcpStream,
    entetes: &str,
    query: Option<&str>,
    config: &ServeConfig,
) -> Result<()> {
    // ── LE COMPTE CONNECTE (T12) : la conversation appartient a un Self ──
    // L identite vient de la session IAM presentee (Bearer, cookie ou
    // ?token=) ; sans compte, le circuit historique (Self de l agent)
    // reste exactement le meme.
    let compte_connecte: Option<String> = jeton_presente(entetes, query)
        .as_deref()
        .and_then(|j| GardeIam::partagee().identite(j))
        // L identite s ecrit `utilisateur@espace` : le Self est celui de
        // l UTILISATEUR (un meme humain traverse ses espaces).
        .map(|id| id.split('@').next().unwrap_or(&id).to_string());
    // ── UN SEUL RÉSOLVEUR DE SELF POUR TOUTE LA SURFACE ─────────────────────
    // Cette route avait sa PROPRE règle (`compte_connecte` → `self:<u>`), et le
    // chemin du chat une AUTRE (le self de l'instance). Deux règles pour la même
    // question, c'est deux mémoires possibles : on passe par `self_qn_pour`, et
    // la portée qu'il rend dit si le nœud `Self` est déjà dans la base ou si les
    // souvenirs attendent encore dans le journal de l'utilisateur.
    let (self_qn, portee) = crate::ancrage::self_qn_pour(
        config.synapse_store.as_deref(),
        compte_connecte.as_deref(),
    );
    let entrees = crate::memoire::en_attente(&self_qn);
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
    let body = serde_json::json!({
        "self": self_qn,
        // « compte » : le nœud Self est dans la synapse ; « journal » : le self
        // est à cette identité, ses souvenirs l'attendent ; « instance » :
        // aucune identité présentée (circuit historique).
        "portee": portee,
        "identite": compte_connecte,
        "journal": crate::memoire::journal(&self_qn).to_string_lossy(),
        "en_attente": entrees.len(),
        "entrees": items,
        "appliquer": "scripts/memoire.sh",
    })
    .to_string();
    respond(stream, 200, VIZ_JSON, &body)
}

/// `GET /api/engine/selfs` — QUI EXISTE : les selfs résidents, et ce qu'ils ont
/// apporté.
///
/// ── POURQUOI CETTE ROUTE AVANT LA RÉSONANCE ────────────────────────────────
/// « Interconnecter chacun sa base » suppose de savoir QUI il y a. Tant qu'une
/// seule personne parlait au serveur, la question ne se posait pas ; avec des
/// comptes, elle devient la première : quel self existe, où est sa mémoire,
/// quel monde lui appartient. Sans ce catalogue, relier deux selfs demanderait
/// de deviner des noms — et un lien deviné relie parfois la mauvaise personne.
///
/// ── ELLE NE RELIE RIEN ─────────────────────────────────────────────────────
/// C'est délibéré : cette route LIT (journaux de mémoire, registre des mondes,
/// nœuds `Self` de la synaptique) et n'écrit pas un octet. Un catalogue qui
/// créerait des liens serait déjà la résonance, c'est-à-dire une décision prise
/// par un affichage — au lieu d'un geste explicite et révocable.
///
/// Elle est gardée comme le reste : une identité valide est exigée (l'espace
/// visé est celui du serveur), donc on ne lit pas la population de l'instance
/// sans être entré.
fn api_engine_selfs(stream: &mut TcpStream, config: &ServeConfig) -> Result<()> {
    let cat = crate::memoire::catalogue(config.synapse_store.as_deref());
    let body = serde_json::json!({
        "selfs": cat.selfs,
        "entrees_total": cat.entrees_total,
        "nombre": cat.selfs.len(),
        // Une lecture ratée de la synaptique est DITE : sinon `mondes: []` et
        // `noeud_dans_synapse: false` se liraient comme des faits alors que ce
        // sont des lacunes.
        "synapse_illisible": cat.synapse_illisible,
        "synapse": config.synapse_store.as_deref().map(|p| p.to_string_lossy().to_string()),
    })
    .to_string();
    respond(stream, 200, VIZ_JSON, &body)
}

/// L'identité de l'appelant, réduite à SON self (`self:<utilisateur>`).
///
/// Rend `None` quand aucune identité n'est présentée : la résonance est un
/// rapport ENTRE selfs, donc sans self il n'y a rien à montrer — et surtout rien
/// à autoriser. On ne devine pas « qui » : on refuse.
fn self_de_l_appelant(entetes: &str, query: Option<&str>, config: &ServeConfig) -> Option<String> {
    let identite = jeton_presente(entetes, query)
        .as_deref()
        .and_then(|j| GardeIam::partagee().identite(j))?;
    let (self_qn, _portee) =
        crate::ancrage::self_qn_pour(config.synapse_store.as_deref(), Some(identite.as_str()));
    Some(self_qn)
}

/// `GET /api/engine/resonance` — ce que je vois des autres, et qui me voit.
///
/// ── TROIS LISTES, ET POURQUOI LES TROIS SONT NÉCESSAIRES ───────────────────
///   · `je_vois` : les selfs dont MES liens me donnent les facettes publiques ;
///   · `je_partage` : les selfs autorisés à voir LES MIENNES ;
///   · `candidats` : ceux qui existent et que je ne vois pas encore — sans quoi
///     il faudrait deviner un nom pour créer un lien, et un lien deviné relie
///     parfois la mauvaise personne.
///
/// ── CE QUI N'EST PAS RENDU, ET POURQUOI ────────────────────────────────────
/// Aucun `note` (texte libre), aucun `etat-self` (intime) : les seules choses
/// partagées sont les NOMS d'objectifs et de procédures, les mondes apportés, et
/// des compteurs. `prive_exclu` et `exclu` voyagent avec la réponse pour qu'un
/// appelant ne prenne jamais un partage partiel pour un partage complet.
fn api_resonance(
    stream: &mut TcpStream,
    entetes: &str,
    query: Option<&str>,
    config: &ServeConfig,
) -> Result<()> {
    let Some(moi) = self_de_l_appelant(entetes, query, config) else {
        return refus_iam(stream, 401, "la résonance est un rapport entre selfs : présentez une identité");
    };
    let cat = crate::memoire::catalogue(config.synapse_store.as_deref());

    // ── CE QUE JE VOIS : mes liens, résolus en facettes réelles ─────────────
    let mut je_vois: Vec<serde_json::Value> = Vec::new();
    let mut deja: Vec<String> = Vec::new();
    for publieur in abonnes_lecteur(&moi) {
        deja.push(publieur.clone());
        let fiche = cat.selfs.iter().find(|s| s.self_qn == publieur);
        let journal = crate::memoire::journal(&publieur);
        let fac = crate::resonance::facettes(&publieur, &journal);
        je_vois.push(serde_json::json!({
            "self": publieur,
            "mondes": fiche.map(|s| s.mondes.clone()).unwrap_or_default(),
            "objectifs": fac.objectifs,
            "procedures": fac.procedures,
            "entrees": fac.entrees,
            "derniere_ms": fac.derniere_ms,
            "derniere_activite_ms": fiche.and_then(|s| s.derniere_ms),
            "noeud_dans_synapse": fiche.map(|s| s.noeud_dans_synapse).unwrap_or(false),
            // Le contrat, porté par la réponse elle-même.
            "prive_exclu": fac.prive_exclu,
            "exclu": fac.exclu,
        }));
    }

    // ── QUI ME VOIT (mes propres abonnés) ──────────────────────────────────
    let je_partage: Vec<serde_json::Value> = crate::resonance::abonnes_de(&moi)
        .into_iter()
        .map(|a| serde_json::json!({ "self": a.lecteur, "depuis_ms": a.depuis_ms }))
        .collect();

    // ── LES CANDIDATS : qui existe, et que je ne vois pas encore ───────────
    let candidats: Vec<serde_json::Value> = cat
        .selfs
        .iter()
        .filter(|s| s.self_qn != moi && !deja.contains(&s.self_qn))
        .map(|s| serde_json::json!({
            "self": s.self_qn,
            "mondes": s.mondes,
            "entrees": s.entrees,
            "derniere_ms": s.derniere_ms,
            "noeud_dans_synapse": s.noeud_dans_synapse,
        }))
        .collect();

    let body = serde_json::json!({
        "moi": moi,
        "je_vois": je_vois,
        "je_partage": je_partage,
        "candidats": candidats,
        "synapse_illisible": cat.synapse_illisible,
        // Ce que la résonance NE fait pas, dit à l'endroit où on la lit : elle
        // ne copie aucune mémoire d'un self dans l'autre.
        "regle": "les noms (objectifs, procédures) et les mondes se partagent ; \
                  les notes et les états intimes ne sortent jamais, et aucun \
                  souvenir n'est copié d'un self dans un autre",
    })
    .to_string();
    respond(stream, 200, VIZ_JSON, &body)
}

/// Les publieurs auxquels UN LECTEUR est abonné.
///
/// Défaut évité : les liens sont rangés par PUBLIEUR (chacun ne détient que sa
/// liste d'abonnés). Retrouver ceux d'un lecteur demande donc de parcourir les
/// fichiers de liens — c'est peu de fichiers, ils sont petits, et le contraire
/// (un index par lecteur) publierait qui est abonné à qui.
fn abonnes_lecteur(lecteur: &str) -> Vec<String> {
    let dir = std::env::var_os("OBRAIN_VIZ_CACHE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp/obrain-viz-cache"))
        .join("resonance");
    let Ok(lectures) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for e in lectures.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Ok(txt) = std::fs::read_to_string(&p) else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) else { continue };
        let Some(publieur) = v.get("publieur").and_then(|x| x.as_str()) else { continue };
        let abonne = v
            .get("abonnes")
            .and_then(|x| x.as_array())
            .is_some_and(|arr| {
                arr.iter()
                    .any(|a| a.get("lecteur").and_then(|x| x.as_str()) == Some(lecteur))
            });
        if abonne {
            out.push(publieur.to_string());
        }
    }
    out.sort();
    out
}

/// `POST /api/engine/resonance` — le GESTE : lier ou délier ses lecteurs.
///
/// Corps : `{"lecteur":"self:<nom>"}` et, pour refermer, `{"delier":true}`.
///
/// ── ON NE LIE QUE SON PROPRE SELF ──────────────────────────────────────────
/// Le publieur est TOUJOURS celui de l'appelant : un self n'ouvre pas les
/// facettes d'un autre. C'est la seule règle d'autorisation dont cette fonction
/// ait besoin, et elle vaut mieux qu'un paramètre vérifié — il n'y en a pas.
fn api_resonance_geste(
    stream: &mut TcpStream,
    entetes: &str,
    query: Option<&str>,
    corps: &str,
    config: &ServeConfig,
) -> Result<()> {
    let Some(moi) = self_de_l_appelant(entetes, query, config) else {
        return refus_iam(stream, 401, "la résonance est un geste de self : présentez une identité");
    };
    let v: serde_json::Value =
        serde_json::from_str(corps).unwrap_or_else(|_| serde_json::json!({}));
    let lecteur = v.get("lecteur").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
    let delier = v.get("delier").and_then(|x| x.as_bool()).unwrap_or(false);
    if lecteur.is_empty() {
        return refus_iam(stream, 400, "« lecteur » est requis (le self qui verra vos facettes)");
    }
    // Le self visé doit EXISTER et ne pas être le mien : on refuse tôt, avec la
    // liste des noms qui existent, plutôt que d'écrire un lien vers personne.
    let cat = crate::memoire::catalogue(config.synapse_store.as_deref());
    if !cat.selfs.iter().any(|s| s.self_qn == lecteur) {
        return refus_iam(
            stream,
            404,
            &format!(
                "aucun self « {lecteur} » dans cette instance — connus : {}",
                cat.selfs
                    .iter()
                    .map(|s| s.self_qn.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
    }
    let r = if delier {
        crate::resonance::delier(&moi, &lecteur).map(|change| (change, "refermé"))
    } else {
        crate::resonance::lier(&moi, &lecteur).map(|change| (change, "ouvert"))
    };
    match r {
        Ok((change, mot)) => {
            let body = serde_json::json!({
                "publieur": moi,
                "lecteur": lecteur,
                "change": change,
                "etat": mot,
                // Rien à faire = le lien était déjà dans cet état : on le dit
                // au lieu de laisser croire à une action.
                "note": if change { "lien modifié" } else { "le lien était déjà dans cet état" },
                "je_partage": crate::resonance::abonnes_de(&moi)
                    .into_iter()
                    .map(|a| a.lecteur)
                    .collect::<Vec<_>>(),
            })
            .to_string();
            respond(stream, 200, VIZ_JSON, &body)
        }
        Err(e) => refus_iam(stream, 400, &format!("{e}")),
    }
}

/// `GET /api/engine/ask?store=&qn=&q=&max_tokens=&raisonnement=` — un LLM qui
/// raisonne SUR le graphe, à partir du sujet que l'utilisateur regarde.
///
/// La pensée du modèle est COUPÉE par défaut ici : cette route est bloquante et
/// n'a pas de tour d'outil, donc sa pensée coûtait dix fois le temps sans rien
/// apporter (mesuré). `raisonnement=1` la rétablit.
/// `max_tokens` est honoré tel quel : il est le seul plafond de génération, et la
/// réponse dit lequel a réellement servi (`max_tokens_effectif`, `plafond_atteint`).
///
/// Le service étant thread-par-requête, l'appel LLM (plusieurs secondes) ne
/// bloque ni la navigation ni le sondage d'activation : le graphe continue de
/// s'allumer pendant que le modèle raisonne.
fn api_engine_ask(
    stream: &mut TcpStream,
    entetes: &str,
    query: Option<&str>,
    config: &ServeConfig,
) -> Result<()> {
    // ── LE COMPTE CONNECTE (T12) : la conversation appartient a un Self ──
    // L identite vient de la session IAM presentee (Bearer, cookie ou
    // ?token=) ; sans compte, le circuit historique (Self de l agent)
    // reste exactement le meme.
    let compte_connecte: Option<String> = jeton_presente(entetes, query)
        .as_deref()
        .and_then(|j| GardeIam::partagee().identite(j))
        // L identite s ecrit `utilisateur@espace` : le Self est celui de
        // l UTILISATEUR (un meme humain traverse ses espaces).
        .map(|id| id.split('@').next().unwrap_or(&id).to_string());
    let params = parse_query(query);
    let qn = params.get("qn").cloned().unwrap_or_default();
    let question = params.get("q").cloned().unwrap_or_default();
    if question.trim().is_empty() {
        return respond(stream, 400, VIZ_JSON, r#"{"error":"q (question) requis"}"#);
    }
    if qn.trim().is_empty() {
        // Sans sujet, on ne raisonne sur rien : mieux vaut le dire que d'envoyer
        // au modèle un contexte vide et récupérer une réponse plausible et fausse.
        return respond(
            stream,
            400,
            VIZ_JSON,
            r#"{"error":"qn (sujet) requis — visez un nœud ou un monde d'abord"}"#,
        );
    }
    let Some((store, _ancrage)) = ancrer_sujet(
        config,
        entetes,
        params.get("store").map(String::as_str),
        &qn,
        compte_connecte.as_deref(),
    ) else {
        return respond(stream, 400, VIZ_JSON, r#"{"error":"store requis"}"#);
    };
    let db = match cached_ro(&store) {
        Ok(db) => db,
        Err(e) => {
            return respond(stream, 400, VIZ_JSON, &format!("{{\"error\":\"ouverture RO : {e}\"}}"));
        }
    };
    let max_tokens = params
        .get("max_tokens")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(700);
    // ── LA PENSÉE EST COUPÉE PAR DÉFAUT **SUR CETTE ROUTE** ──────────────────
    // Mesuré le 2026-09-12, même question, même contexte de 4 nœuds, pensée ON
    // contre `raisonnement=0` : **43 481 ms / 400 jetons / aucun texte final**
    // contre **4 248 ms / 43 jetons / une vraie réponse française** citant les
    // bons nœuds. Dix fois plus rapide ET une réponse au lieu d'une pensée.
    //
    // La raison de fond : `demander` ne fait AUCUN tour d'outil (seul
    // `demander_stream` en a). Les trois règles du prompt système qui somment le
    // modèle d'appeler `obrain_echantillon` ou `obrain_query` sont donc
    // inapplicables ici — la pensée n'y sert qu'à consumer le plafond avant la
    // réponse. `?raisonnement=1` rétablit l'ancien comportement à la demande.
    //
    // Le chat en flux, lui, garde la pensée : c'est elle qui déclenche ses outils.
    let pensee = matches!(
        params.get("raisonnement").map(String::as_str),
        Some("1") | Some("true") | Some("on") | Some("oui")
    );
    // Niveau d effort : `?pensee=` s il est donne, sinon le defaut NEUTRE du
    // template (jamais xhigh par omission — 37 s mesures pour un comptage).
    let effort = crate::engine::effort_de_pensee(params.get("pensee").map(String::as_str));
    match crate::engine::demander(&db, &store, &qn, &question, max_tokens, pensee, effort, compte_connecte.as_deref()) {
        Ok(r) => respond(stream, 200, VIZ_JSON, &serde_json::to_string(&r)?),
        Err(e) => {
            let body = serde_json::json!({ "error": format!("{e}") }).to_string();
            respond(stream, 200, VIZ_JSON, &body)
        }
    }
}


/// L'ATLAS, MÉMORISÉ — la carte des mondes et de leurs synapses.
///
/// Il est relu à chaque conversation (12 lignes + 12 fibres dans la base
/// synaptique) mais reste identique d'un tour à l'autre : on le garde donc en
/// mémoire, avec une durée courte. Il change quand un monde est ajouté à chaud
/// ou quand la toile est retissée — pas entre deux questions.
fn atlas_memo(config: &ServeConfig) -> crate::ancrage::Atlas {
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};
    static A: OnceLock<Mutex<Option<(Instant, crate::ancrage::Atlas)>>> = OnceLock::new();
    let m = A.get_or_init(|| Mutex::new(None));
    // ── LE VERROU N'EST PAS TENU PENDANT LE CALCUL ────────────────────────
    // Défaut mesuré (2026-09-11) : le garde restait vivant jusqu'à la fin de la
    // fonction, donc l'ouverture de la base synaptique (`cached_ro`, froide) et
    // ses deux requêtes s'exécutaient VERROU TENU, à chaque expiration des 30 s.
    // Deux conversations simultanées se sérialisaient donc sur un travail de
    // plusieurs centaines de millisecondes, et l'événement `contexte` de la
    // seconde attendait la première. On lit le cache sous verrou, on le
    // RELÂCHE, on calcule, puis on repose. Un calcul concurrent en trop est
    // inoffensif : la lecture est idempotente, et le dernier écrase — c'est
    // toujours le même atlas qui est rendu.
    {
        let g = verrou(&m);
        if let Some((t, a)) = g.as_ref()
            && t.elapsed() < Duration::from_secs(30)
        {
            return a.clone();
        }
    }
    let a = crate::ancrage::atlas(config.synapse_store.as_deref(), &tous_les_mondes(config));
    *verrou(&m) = Some((Instant::now(), a.clone()));
    a
}

/// L'ouvreur de mondes donné aux outils : résout un `qn` de monde et rend un
/// handle RO **déjà ouvert par le préchauffage**.
///
/// Il n'OUVRE pas — il attend un peu (voir `attendre_handle`). Un outil qui ne
/// peut pas lire un monde le DIT (« base non ouverte ») au lieu de figer la
/// conversation ; c'est ce que `obrain_traverser` affiche.
fn ouvreur_de_mondes(
    config: &ServeConfig,
) -> impl Fn(&str) -> Option<std::sync::Arc<obrain_engine::ObrainDB>> + '_ {
    move |qn: &str| {
        let m = crate::ancrage::monde_du_sujet(
            config.synapse_store.as_deref(),
            qn,
            &tous_les_mondes(config),
        )?;
        // Peek d'abord : le préchauffage a déjà ouvert tous les mondes. Sinon on
        // ouvre — un outil qui ne peut pas lire un monde le dit, mais on ne
        // refuse pas par principe.
        cached_ro_si_present(&m.path).or_else(|| cached_ro(&m.path).ok())
    }
}

/// `GET /api/engine/chat` — conversation en FLUX (SSE).
///
/// Ce que la vue ne pouvait pas obtenir avec `/api/engine/ask` : une réponse qui
/// ARRIVE. Un modèle de raisonnement réfléchit parfois 60 s ; en un seul bloc, la
/// fenêtre restait muette tout ce temps. Ici chaque morceau est écrit puis
/// POUSSÉ sur la socket : le *think*, le texte, et l'annonce des appels obrain.
fn api_engine_chat(
    stream: &mut TcpStream,
    entetes: &str,
    query: Option<&str>,
    config: &ServeConfig,
) -> Result<()> {
    use std::io::Write;
    // ── LE COMPTE CONNECTE (T12) : la conversation appartient a un Self ──
    // L identite vient de la session IAM presentee (Bearer, cookie ou
    // ?token=) ; sans compte, le circuit historique (Self de l agent)
    // reste exactement le meme.
    let compte_connecte: Option<String> = jeton_presente(entetes, query)
        .as_deref()
        .and_then(|j| GardeIam::partagee().identite(j))
        // L identite s ecrit `utilisateur@espace` : le Self est celui de
        // l UTILISATEUR (un meme humain traverse ses espaces).
        .map(|id| id.split('@').next().unwrap_or(&id).to_string());
    let params = parse_query(query);
    let question = params.get("q").cloned().unwrap_or_default();
    if question.trim().is_empty() {
        return respond(stream, 400, VIZ_JSON, r#"{"error":"q (question) requis"}"#);
    }
    // Sujet FACULTATIF : sans sujet visé, l'engine part du GRAPHE GLOBAL
    // (signature du store). C'est ce qu'on veut depuis un chat plein écran.
    let sujet = params.get("qn").cloned().unwrap_or_default();
    // ── ANCRAGE RÉSOLU, PAS INTERPRÉTÉ ─────────────────────────────────────
    // Si le sujet est un monde, c'est SON store qu'on ouvre — résolu dans la
    // base synaptique. Corrige le défaut mesuré : annoncer `this-rs/this` et
    // lire `runtime-dev`.
    let Some((store, mut ancrage)) = ancrer_sujet(
        config,
        entetes,
        params.get("store").map(String::as_str),
        &sujet,
        compte_connecte.as_deref(),
    ) else {
        return respond(stream, 400, VIZ_JSON, r#"{"error":"store requis"}"#);
    };
    if let Some(u) = &compte_connecte {
        // La conversation appartient au COMPTE : sa memoire (outil
        // obrain_memoriser) se journalise vers SON Self, et son profil
        // (base self-<compte>.obrain) entre dans le contexte.
        ancrage.self_qn = format!("self:{u}");
    }
    // 900 était FRÔLÉ par un échange normal : mesuré, think 991 caractères
    // (~300 jetons) + réponse 1 568 caractères (~450) = ~800 sur 900. Un
    // raisonnement un peu plus long atteignait le plafond et la réponse était
    // coupée. On laisse de la marge, et surtout on RAPPORTE la coupure (voir
    // `raison` ci-dessous) au lieu de la laisser passer pour une fin naturelle.
    let max_tokens = params.get("max_tokens").and_then(|v| v.parse::<u32>().ok()).unwrap_or(4096);
    // ── LE RAISONNEMENT SE DEMANDE, IL NE SE SUBIT PLUS (rapatrie mc) ──
    // `?pensee=non|courte|moyenne|profonde`, defaut neutre du template.
    let pensee_demande = params.get("pensee").map(String::as_str);
    let pensee = !matches!(pensee_demande, Some("non") | Some("off") | Some("0") | Some("sans"));
    let effort = crate::engine::effort_de_pensee(pensee_demande);
    // Historique multi-tours : `h` = JSON `[["user","…"],["assistant","…"]]`.
    // ── L'HISTORIQUE EST BORNÉ CÔTÉ SERVEUR AUSSI ─────────────────────────
    // La vue ne garde que les 8 derniers échanges, mais le serveur faisait
    // confiance au paramètre : ni le nombre de paires, ni la taille cumulée
    // n'étaient bornés (seule limite de fait : la lecture de la requête). Un
    // client — ou un lien forgé — pouvait donc pousser une conversation
    // arbitrairement longue, saturer la fenêtre de contexte et forcer la branche
    // « sortie_tour < 64 », c'est-à-dire une réponse vide. On tronque ici, et on
    // le DIT au client par un en-tête SSE dédié.
    const MAX_ECHANGES: usize = 12;
    const MAX_CARACTERES_HISTORIQUE: usize = 60_000;
    let brut: Vec<Vec<String>> = params
        .get("h")
        .and_then(|v| serde_json::from_str::<Vec<Vec<String>>>(v).ok())
        .unwrap_or_default();
    let mut taille = 0usize;
    let mut historique: Vec<(String, String)> = Vec::new();
    for p in brut.iter().rev().take(MAX_ECHANGES).rev() {
        if p.len() != 2 {
            continue;
        }
        let (u, a) = (&p[0], &p[1]);
        if taille + u.len() + a.len() > MAX_CARACTERES_HISTORIQUE {
            break;
        }
        taille += u.len() + a.len();
        historique.push((u.clone(), a.clone()));
    }
    historique.reverse();

    // ── LA REPRISE : LE TEXTE DÉJÀ PRODUIT, QUAND LA VUE ROUVRE UN FLUX COUPÉ ─
    // Le transport a coupé une réponse en plein milieu (service redémarré par un
    // déploiement — `status=9/KILL` au journal systemd, 1 248 redémarrages
    // comptés) : la vue renvoie ce qui a déjà été affiché, et l'engine CONTINUE
    // cette réponse au lieu d'en écrire une autre. La coupure cesse d'être une
    // réponse incomplète.
    //
    // Borné, et par la QUEUE : le raccord se fait en fin de texte, pas au début.
    // Le plafond protège la fenêtre de jetons d'un paramètre forgé, et
    // `demander_stream` réserve la place de ce texte dans son budget.
    const MAX_REPRISE: usize = 8_000;
    let reprise_brute: Option<String> = params.get("deja").map(|d| {
        let n = d.chars().count();
        if n <= MAX_REPRISE {
            d.clone()
        } else {
            d.chars().skip(n - MAX_REPRISE).collect()
        }
    });
    let reprise = reprise_brute.as_deref().filter(|d| !d.trim().is_empty());

    // En-têtes SSE + poussée immédiate : le client doit voir la connexion
    // établie avant le premier mot, sinon rien ne distingue « ça réfléchit » de
    // « c'est cassé ».
    //
    // `Date` est posé ici comme partout ailleurs (RFC 9110 §6.6.1), et
    // `X-Accel-Buffering: no` parce que nginx — le plus courant des proxys
    // inverses devant ce service — TAMPONNE par défaut un flux montant : sans cet
    // en-tête, la réponse arrive d'un bloc à la fin, c'est-à-dire exactement ce
    // que le SSE existe pour éviter.
    let entetes = format!(
        "HTTP/1.1 200 OK\r\nDate: {}\r\nContent-Type: text/event-stream; charset=utf-8\r\n\
         Cache-Control: no-cache\r\nConnection: close\r\n\
         X-Accel-Buffering: no\r\nAccess-Control-Allow-Origin: *\r\n\r\n",
        date_http()
    );
    if stream.write_all(entetes.as_bytes()).is_err() {
        return Ok(());
    }
    let _ = stream.flush();
    // LA FERMETURE EST VOULUE, ET LA BOUCLE DE CONNEXION DOIT LE SAVOIR.
    // Un flux SSE se termine par la fermeture — il n'a ni `Content-Length`, ni
    // `Transfer-Encoding: chunked` (et surtout PAS ce dernier :
    // `scripts/chat-reprise-check.mjs` attend la fermeture du flux, et un corps
    // découpé en blocs le ferait pendre jusqu'à ses 180 s).
    etat_connexion(|e| {
        e.fermer = true;
        e.keep_alive = false;
        e.repondu = true;
    });
    // ── LA PREMIÈRE TRAME : `retry` ────────────────────────────────────────
    // Sans elle, `EventSource` applique son propre délai (≈ 3 s) avant de rouvrir
    // un flux coupé. 2 s est le compromis : assez court pour que la reprise ne se
    // voie pas, assez long pour ne pas marteler un serveur qui vient de tomber.
    // CE N'EST PAS la reprise applicative (`deja`/`reprise`) : celle-ci raccorde
    // le TEXTE, `retry` ne fait que décider QUAND on retente.
    let _ = stream.write_all(format!("retry: {RETRY_SSE_MS}\n\n").as_bytes());
    let _ = stream.flush();

    // ── L'IDENTITÉ EST ANNONCÉE TOUT DE SUITE, PAS APRÈS LE MODÈLE ──────────
    // Défaut mesuré le 2026-09-16 : l'écran range la conversation PAR SELF (deux
    // personnes peuvent ouvrir le même navigateur), et le self n'était annoncé
    // que par l'événement `contexte` — qui arrive APRÈS la réflexion du modèle,
    // soit plusieurs dizaines de secondes avec un modèle distant. Pendant tout ce
    // temps, l'écran ne savait pas à qui la conversation appartenait : il écrivait
    // sous une clé provisoire, et la porte a mesuré cet état intermédiaire.
    //
    // Le COMPTE, lui, est connu ICI, avant tout appel au modèle : c'est lui qui
    // distingue deux personnes sur un même navigateur, et il part donc en premier.
    // Le self (`self_qn`) suivra dans `contexte` — c'est le même resolveur, mais
    // il désigne le self RÉSOLU, qui dépend de la base visée.
    let _ = stream.write_all(
        format!(
            "event: identite\ndata: {}\n\n",
            serde_json::json!({ "compte": compte_connecte })
        )
        .as_bytes(),
    );
    let _ = stream.flush();

    // ── LA BASE ANCREE ──────────────────────────────────────────────────────
    // Peek d'abord (aucune ouverture), ouverture ensuite : c'est le
    // comportement qui fonctionnait, et une ouverture n'est payée que si le
    // handle n'est pas déjà là. Le risque de blocage d'une ouverture en lecture
    // est réel (un écrivain peut tenir la base) mais il est PRÉFÉRABLE à un
    // refus : une version intermédiaire exigeait un handle déjà présent et
    // répondait « base non ouverte » alors que la base était ouverte — elle
    // cassait le chat pour éviter un risque. On ne sacrifie pas un service qui
    // marche à une hypothèse qu'on n'a pas su confirmer.
    let db = match cached_ro_si_present(&store) {
        Some(db) => db,
        None => match cached_ro(&store) {
            Ok(db) => db,
            Err(e) => {
                let _ = writeln!(
                    stream,
                    "event: erreur\ndata: {}\n",
                    serde_json::json!({
                        "error": format!("ouverture RO de {} : {e}", store.display())
                    })
                );
                let _ = stream.flush();
                return Ok(());
            }
        },
    };

    // ── LE BATTEMENT DE CŒUR, ET POURQUOI L'ÉCRITURE PASSE PAR UN VERROU ───
    // Entre l'ouverture du flux et le premier jeton, un modèle de raisonnement
    // peut réfléchir 60 s. Sans un octet pendant ce temps, un proxy — ou le client
    // lui-même — tient la connexion pour morte. Le commentaire SSE (`:`) est fait
    // pour ça : `EventSource` ne le remonte pas comme un événement, il prouve
    // seulement que la connexion vit. Mesuré avant : aucune trame `retry`, aucun
    // battement, donc rien entre « la connexion est ouverte » et « le premier
    // mot », parfois pendant une minute.
    //
    // Le verrou n'est pas une précaution de principe. Le battement écrit depuis un
    // AUTRE fil, et `writeln!` sur une socket émet PLUSIEURS écritures (le JSON se
    // sérialise morceau par morceau) : un `: ping` glissé entre `data:` et sa
    // valeur couperait la trame, et comme une ligne vide TERMINE un événement, le
    // jeton serait perdu. Le défaut serait invisible — le flux resterait bien
    // formé, il manquerait juste des mots.
    let Ok(secours) = stream.try_clone() else {
        // Sans second descripteur, pas de battement possible : mieux vaut le dire
        // que servir un flux qui peut mourir en silence au premier proxy venu.
        return Err(anyhow::anyhow!("SSE : socket non duplicable (descripteurs épuisés ?)"));
    };
    let flux: Arc<Mutex<(TcpStream, Instant)>> = Arc::new(Mutex::new((secours, Instant::now())));
    let vivant = Arc::new(AtomicBool::new(true));
    let battement = {
        let flux = Arc::clone(&flux);
        let vivant = Arc::clone(&vivant);
        std::thread::spawn(move || {
            // Réveil court (500 ms) : l'arrêt du fil est alors quasi immédiat, et
            // une connexion rendue ne reste pas ouverte un cycle entier de plus.
            while vivant.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(500));
                if !vivant.load(Ordering::Relaxed) {
                    break;
                }
                let mut g = verrou(&flux);
                if g.1.elapsed() >= BATTEMENT_SSE {
                    let _ = g.0.write_all(b": ping\n\n");
                    let _ = g.0.flush();
                    g.1 = Instant::now();
                }
            }
        })
    };

    // Chaque événement est écrit ET poussé : sans le flush, tout resterait dans
    // le tampon de la socket et arriverait d'un bloc à la fin — exactement ce
    // qu'on veut éviter. La trame est construite ENTIÈREMENT puis écrite en un
    // seul `write_all`, sous le même verrou que le battement.
    /**
     * ── LE CLIENT PARTI SE DIT, ET ARRÊTE LA GÉNÉRATION ─────────────────────
     *
     * L'écriture IGNORAIT l'erreur (`let _ = …`) : un onglet fermé laissait donc
     * l'engine générer jusqu'au bout, à 600 s par appel de modèle. On pose un
     * drapeau à la première écriture refusée ; l'engine le consulte en tête de
     * chaque tour ET dans son rappel de flux, donc la génération cesse.
     */
    let client_parti = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ecrire = |nom: &str, donnees: serde_json::Value| {
        let trame = format!("event: {nom}\ndata: {donnees}\n\n");
        let mut g = verrou(&flux);
        if g.0.write_all(trame.as_bytes()).is_err() || g.0.flush().is_err() {
            client_parti.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        g.1 = Instant::now();
    };
    // Le premier paramètre est conservé pour ne pas toucher aux huit appels qui
    // suivent : c'est le `&mut TcpStream` d'origine, désormais inutilisé puisque
    // l'écriture passe par le descripteur PARTAGÉ ci-dessus.
    let pousser = |_stream: &mut TcpStream, nom: &str, donnees: serde_json::Value| ecrire(nom, donnees);
    let mut pousser = pousser;
    let atlas = atlas_memo(config);
    let ouvrir = ouvreur_de_mondes(config);
    // Le handle synaptique, SEULEMENT s'il est déjà ouvert : la traversée s'en
    // sert quand elle peut, et se rabat sur les fibres sinon. On n'ouvre jamais
    // ici (voir `cached_ro_si_present` — une ouverture peut bloquer).
    // Même contrat que l'ancrage : peek d'abord, ouverture ensuite. Le peek seul
    // ne suffisait pas, et la traversée se rabattait alors sur les échantillons
    // des fibres — moins fort que les `Anchor` de la base synaptique.
    let synapse = config
        .synapse_store
        .as_deref()
        .and_then(|p| cached_ro_si_present(p).or_else(|| cached_ro(p).ok()));
    let resultat = crate::engine::demander_stream(
        &db,
        &store,
        &ancrage,
        &atlas,
        &ouvrir,
        synapse.as_deref(),
        &sujet,
        &question,
        &historique,
        // `reprise` : texte DÉJÀ produit, quand la vue reprend une réponse que le
        // transport a coupée — c'est le paramètre `deja` du chat (voir ci-dessus,
        // et la reprise automatique côté vue).
        reprise,
        max_tokens,
        pensee,
        effort,
        |ev| match ev {
            crate::engine::Evenement::Contexte {
                noeuds,
                relations,
                apercu,
                ms,
                store,
                monde,
                total_noeuds,
                total_aretes,
                par_label,
                hubs,
                hubs_ms,
                self_qn,
                fenetre,
                fenetre_source,
                decompose,
            } => pousser(
                stream,
                "contexte",
                serde_json::json!({
                    "noeuds": noeuds,
                    "relations": relations,
                    "apercu": apercu,
                    "ms": ms,
                    // L'ancrage RÉSOLU repart vers l'écran : c'est ce qui empêche
                    // l'interface d'afficher un monde et d'en lire un autre.
                    "store": store,
                    "monde": monde,
                    "self_qn": self_qn,
                    "hubs": hubs,
                    "hubs_ms": hubs_ms,
                    "fenetre": fenetre,
                    "fenetre_source": fenetre_source,
                    "decompose": decompose,
                    "total_noeuds": total_noeuds,
                    "total_aretes": total_aretes,
                    "par_label": par_label
                        .into_iter()
                        .map(|(l, t, lus)| serde_json::json!({ "label": l, "total": t, "lus": lus }))
                        .collect::<Vec<_>>(),
                }),
            ),
            // L'APPEL D'OUTIL EST VISIBLE : c'est ce qui rend la densification
            // observable — on voit quelle requête a été exécutée, sur quel monde,
            // combien de lignes, en combien de temps, et l'erreur s'il y en a une.
            crate::engine::Evenement::Outil { nom, arguments, lignes, ms, monde, erreur, tronque } => {
                // `tronque` est TRANSMIS : le plafond de caractères était annoncé
                // au modèle mais pas à l'écran, qui affichait « 29 lignes » pour
                // un résultat dont 17 étaient parvenues au modèle (mesuré).
                pousser(
                    stream,
                    "outil",
                    serde_json::json!({
                        "nom": nom, "arguments": arguments, "lignes": lignes,
                        "ms": ms, "monde": monde, "erreur": erreur, "tronque": tronque,
                    }),
                )
            }
            // LA MÉMOIRE EST ANNONCÉE. Ce n'est pas une lecture : c'est une
            // écriture journalisée, en attente d'application. Le dire évite de
            // laisser croire qu'elle est déjà dans la base.
            crate::engine::Evenement::Memoire { genre, reference, self_qn } => {
                pousser(
                    stream,
                    "memoire",
                    serde_json::json!({
                        "genre": genre, "reference": reference, "self_qn": self_qn,
                    }),
                )
            }
            crate::engine::Evenement::Think(t) => {
                pousser(stream, "think", serde_json::json!({ "t": t }))
            }
            crate::engine::Evenement::Texte(t) => {
                pousser(stream, "texte", serde_json::json!({ "t": t }))
            }
            crate::engine::Evenement::Fin { ms_llm, raison } => {
                pousser(stream, "fin", serde_json::json!({ "ms_llm": ms_llm, "raison": raison }))
            }
        },
        // Le client parti arrête la génération (voir `client_parti` plus haut).
        // Un `Arc` et non une référence : la garde (`obrain-garde`) le partage
        // avec le rappel de lecture, exactement comme le fait `selfd`.
        Some(std::sync::Arc::clone(&client_parti)),
    );
    // ── LE BATTEMENT S'ARRÊTE AVANT QUE LA FONCTION RENDE LA MAIN ───────────
    // `join` et non un simple drapeau : le fil de battement détient un
    // DESCRIPTEUR DUPLIQUÉ, et tant qu'il vit, le client ne voit pas la fin du
    // flux — le noyau n'envoie le FIN que quand TOUS les descripteurs sont
    // fermés. Un flux qui ne se ferme pas est précisément ce que
    // `scripts/chat-reprise-check.mjs` attend : il pendrait jusqu'à ses 180 s.
    vivant.store(false, Ordering::Relaxed);
    let _ = battement.join();
    if let Err(e) = resultat {
        pousser(stream, "erreur", serde_json::json!({ "error": format!("{e}") }));
    }
    Ok(())
}

/// `POST /api/worlds` — ajoute un dépôt GitHub comme nouveau monde.
/// Corps : `{"url":"https://github.com/<propriétaire>/<dépôt>","label":"…"}`.
/// Rend IMMÉDIATEMENT un identifiant de travail : le clone, l'indexation et
/// l'enrichissement peuvent durer des minutes, on ne tient pas une connexion
/// ouverte pour ça.
///
/// `entetes` sert à une chose : savoir QUI ajoute. Un monde ajouté apparaît chez
/// tout le monde — dès qu'ils sont plusieurs, « qui a ajouté ça » doit avoir une
/// réponse, et elle s'écrit dans le registre.
/// Le compte de l'appelant, tel que l'IAM le nomme.
///
/// Sans IAM armé il n'y a pas de comptes : tout le monde est le même occupant,
/// et on le DIT (`local`) plutôt que de refuser un service qui n'a alors aucune
/// frontière à défendre.
fn compte_appelant(entetes: &str, query: Option<&str>) -> Option<String> {
    let Some(g) = garde_interieure() else { return Some("local".to_string()) };
    let jeton = jeton_presente(entetes, query).filter(|j| !j.trim().is_empty())?;
    let s = g.authentifier(Some(&jeton)).ok().flatten()?;
    Some(s.username().to_string())
}

/// `/api/remote/*` — LE CANAL entre le moteur et la machine de l'appelant.
///
/// Huit gestes, un seul point d'entrée :
///
/// * `POST /api/remote/attacher`  — la machine s'annonce  → `client_id` ;
/// * `GET  /api/remote/ordres`    — elle demande du travail (attente bornée) ;
/// * `POST /api/remote/resultat`  — elle rend son verdict ;
/// * `POST /api/remote/detacher`  — elle s'en va proprement ;
/// * `GET  /api/remote/clients`   — l'interface liste LES SIENNES ;
/// * `GET  /api/remote/machines`  — LA QUESTION DE LA FLOTTE, répondue : quelles
///   machines ce compte connaît, et ce que chacune sait faire ;
/// * `POST /api/remote/appel`     — le moteur demande une action, et attend ;
/// * `POST /api/remote/memoire`   — la machine NOURRIT la base de son compte
///   (journalise des intentions ; la passe d'application les fait atterrir).
///
/// L'autorisation tient en une phrase : le compte du jeton doit posséder le
/// client. Un identifiant de client n'ouvre rien à lui seul (voir `remote`).
fn api_remote(
    stream: &mut TcpStream,
    path: &str,
    method: &str,
    entetes: &str,
    query: Option<&str>,
    corps: &str,
) -> Result<()> {
    let Some(compte) = compte_appelant(entetes, query) else {
        let body = serde_json::json!({
            "error": "identité requise : ce canal ne parle qu'à VOS machines",
        })
        .to_string();
        return respond(stream, 401, VIZ_JSON, &body);
    };
    let params = parse_query(query);
    let v: serde_json::Value =
        serde_json::from_str(corps).unwrap_or_else(|_| serde_json::json!({}));
    let champ = |cle: &str| -> String {
        v.get(cle)
            .and_then(|x| x.as_str())
            .map(str::to_string)
            .or_else(|| params.get(cle).cloned())
            .unwrap_or_default()
    };
    let erreur = |stream: &mut TcpStream, code: u16, message: &str| -> Result<()> {
        respond(stream, code, VIZ_JSON, &serde_json::json!({ "error": message }).to_string())
    };

    match (path, method) {
        ("/api/remote/attacher", "POST") => {
            let machine = {
                let m = champ("machine");
                if m.trim().is_empty() { "machine sans nom".to_string() } else { m }
            };
            // ── LE CORPS ENTIER, ET PAS SEULEMENT LES CAPACITÉS ────────────
            // Ce handler n'extrayait que `capacites` : la moitié « ce que la
            // machine REFUSE » (`capacites_refusees`, avec son motif) et les
            // `limites` étaient perdues AU TRANSPORT, alors que le nœud les
            // annonçait bel et bien. Conséquence mesurée par la porte
            // d'acceptation : un client lisait `read, list, write, status` sans
            // savoir que `write` était refusé — il proposait donc un geste voué à
            // l'échec — et quand le moteur refusait un appel, il ne pouvait PAS
            // donner le motif du nœud (« le motif de ce refus n'est PAS arrivé avec
            // son annonce »), ce qui rendait le refus inactionnable. C'est
            // exactement ce que la doctrine interdit.
            //
            // On remet la requête telle qu'elle est arrivée : c'est ce que le trait
            // `AnnonceEntrante` prévoit pour un appelant qui détient déjà le corps.
            // Ce qui manque encore est NOMMÉ dans `annonce.manquante` — jamais
            // présumé complet, jamais tu.
            let id = crate::remote::attacher(&compte, &machine, &v);
            // Le journal dit QUI s'est branché, jamais avec quoi : un jeton
            // écrit une fois dans un journal y reste pour toujours.
            eprintln!("[remote] {compte} a attaché « {machine} » ({id})");
            let body = serde_json::json!({
                "client_id": id,
                "compte": compte,
                "attente_max_s": 25,
            })
            .to_string();
            respond(stream, 200, VIZ_JSON, &body)
        }
        ("/api/remote/ordres", "GET") => {
            let client = champ("client");
            // Bornée à 25 s : sous la minute des mandataires qui coupent les
            // connexions inactives, et assez longue pour que le battement ne
            // coûte presque rien.
            let attente = params
                .get("attente")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(25)
                .clamp(1, 25);
            match crate::remote::prochain_ordre(
                &compte,
                &client,
                std::time::Duration::from_secs(attente),
            ) {
                Ok(Some(o)) => respond(stream, 200, VIZ_JSON, &o.en_json().to_string()),
                // 204 : « rien pour toi, rappelle ». Ce n'est pas une erreur —
                // c'est le rythme normal du canal.
                Ok(None) => respond(stream, 204, VIZ_JSON, ""),
                Err(e) => erreur(stream, 403, &e),
            }
        }
        ("/api/remote/resultat", "POST") => {
            let client = champ("client_id");
            let appel = champ("appel_id");
            let resultat = v.get("resultat").cloned().unwrap_or(serde_json::Value::Null);
            match crate::remote::deposer_resultat(&compte, &client, &appel, resultat) {
                Ok(()) => respond(stream, 200, VIZ_JSON, "{\"recu\":true}"),
                Err(e) => erreur(stream, 403, &e),
            }
        }
        ("/api/remote/detacher", "POST") => {
            let client = champ("client_id");
            let parti = crate::remote::detacher(&compte, &client);
            respond(stream, 200, VIZ_JSON, &serde_json::json!({ "detache": parti }).to_string())
        }
        ("/api/remote/clients", "GET") => {
            let liste: Vec<serde_json::Value> =
                crate::remote::clients_du_compte(&compte).iter().map(|c| c.en_json()).collect();
            let body = serde_json::json!({
                "compte": compte,
                "clients": liste,
                "aide": "Sur votre machine : `selfd attacher --moteur <url> --jeton-moteur <votre jeton>` (le drapeau `--jeton` n'existe pas ; préférez `SELFD_MOTEUR_TOKEN=<jeton>` — un secret passé en argument est visible dans `ps`)",
            })
            .to_string();
            respond(stream, 200, VIZ_JSON, &body)
        }
        // ── LA QUESTION DE LA FLOTTE, ENFIN RÉPONDUE (2026-09-16) ──────────
        //
        // « quelles machines connais-tu, et que sait faire chacune ? » — le
        // relevé du 2026-09-15 disait cette question SANS RÉPONSE, et notait
        // qu'elle échouait « par le vide, pas par l'erreur » : aucune route ne
        // la recevait. La voici, et elle tient les trois propriétés que la
        // doctrine exige :
        //
        //   1. NOMMER. Chaque machine est nommée — sans nom, on ne peut ni
        //      l'adresser, ni l'autoriser, ni l'auditer PAR MACHINE.
        //   2. CE QU'ELLE SAIT FAIRE, tel qu'elle l'a ANNONCÉ, et rien de plus.
        //      Le moteur rapporte la politique de la machine ; il n'en fabrique
        //      aucune, et ne s'en sert jamais pour autoriser.
        //   3. DIRE QUAND ON NE SAIT RIEN. Une liste vide ne doit pas avoir la
        //      forme d'une bonne réponse : `aucune` et `reponse` le disent en
        //      toutes lettres, donc « aucune machine » cesse de se confondre
        //      avec « route absente ».
        //
        // Et parce que le registre est journalisé (voir `remote`), une machine
        // arrêtée reste NOMMÉE avec `vivant: false` : c'est ce qui distingue
        // « jamais vue » de « connue mais absente ».
        ("/api/remote/machines", "GET") => {
            let clients = crate::remote::clients_du_compte(&compte);
            let joignables = clients.iter().filter(|c| c.vivant).count();
            let machines: Vec<serde_json::Value> =
                clients.iter().map(|c| c.en_json()).collect();
            let reponse = if machines.is_empty() {
                "ce compte ne connaît AUCUNE machine : rien ne s'est rattaché, ou rien ne \
                 s'est rattaché SOUS CE COMPTE. Sur une machine : `selfd attacher --moteur \
                 <url> --jeton-moteur <votre jeton>` — le self servi est celui de ce compte, \
                 jamais celui d'un autre."
                    .to_string()
            } else {
                format!(
                    "{} machine(s) connue(s) de ce compte, {} joignable(s) ; les autres sont \
                     connues du journal mais ne parlent pas à ce processus.",
                    machines.len(),
                    joignables
                )
            };
            let body = serde_json::json!({
                "compte": compte,
                "question": "quelles machines connais-tu, et que sait faire chacune ?",
                "machines": machines,
                "connues": clients.len(),
                "joignables": joignables,
                "aucune": clients.is_empty(),
                "reponse": reponse,
            })
            .to_string();
            respond(stream, 200, VIZ_JSON, &body)
        }
        // ── LE CHEMIN D'ÉCRITURE (2026-09-15) ──────────────────────────────
        // Jusqu'ici ce canal ne servait qu'à atteindre la MACHINE ; il ne
        // permettait pas à la machine de nourrir la base de SON compte. Or
        // « chaque utilisateur a un agent qui se développe sur SA base » est
        // FAUX tant qu'une machine attachée ne peut rien y écrire : elle lisait
        // son self et gardait ses notes en local. Ce geste est ce qui rend la
        // phrase vraie, sans toucher à l'invariant mono-écrivain : il JOURNALISE
        // (append-only, sans verrou), et la passe d'application fait le reste.
        ("/api/remote/memoire", "POST") => {
            let client = {
                let c = champ("client_id");
                if c.trim().is_empty() { champ("client") } else { c }
            };
            api_remote_memoire(stream, &compte, &v, &client)
        }
        ("/api/remote/appel", "POST") => {
            let client = champ("client_id");
            let outil = champ("outil");
            if outil.trim().is_empty() {
                return erreur(stream, 400, "« outil » requis : que doit faire la machine ?");
            }
            let arguments = v.get("arguments").cloned().unwrap_or(serde_json::json!({}));
            let delai = v
                .get("delai_s")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(30)
                .clamp(1, 120);
            match crate::remote::appeler(
                &compte,
                &client,
                &outil,
                arguments,
                std::time::Duration::from_secs(delai),
            ) {
                Ok(r) => respond(
                    stream,
                    200,
                    VIZ_JSON,
                    &serde_json::json!({ "resultat": r }).to_string(),
                ),
                Err(e) => erreur(stream, 409, &e),
            }
        }
        _ => erreur(stream, 404, "route inconnue du canal"),
    }
}

/// ── ÉCRIRE : UNE MACHINE ATTACHÉE NOURRIT LA BASE DE SON COMPTE ─────────────
///
/// Défaut comblé le 2026-09-15. Une machine attachée LISAIT le self de son
/// compte (`/api/viz/weave`, `/api/engine/memoire`, `/api/query`) mais ne
/// pouvait RIEN y écrire : l'en-tête de `obrain-selfd/src/soi.rs` le disait à
/// l'agent (« tu ne peux pas encore y écrire »), et le nœud gardait donc ses
/// notes en local. Tant que c'était vrai, « chaque utilisateur a un agent qui se
/// développe RÉELLEMENT sur SA base de connaissances » était faux.
///
/// ── POURQUOI UN JOURNAL, ET PAS UNE ÉCRITURE DIRECTE ───────────────────────
/// Ce n'est pas un choix de goût, c'est la contrainte du substrat (voir
/// l'en-tête de `crate::memoire`) : `viz serve` sert ses stores en LECTURE
/// (`flock` partagé), une écriture exige l'EXCLUSIF, et la promotion est
/// refusée. Ce processus ne peut donc pas ouvrir la base du self en écriture —
/// et ce n'est pas un défaut à contourner, c'est l'invariant mono-écrivain. On
/// JOURNALISE ici (append-only, aucun verrou), et la passe d'application
/// (`obrain memoire --store <base> --self <self> --appliquer`, celle que le
/// timer `obrain-self-appliquer` déroule déjà PAR COMPTE) prend l'exclusif et
/// écrit en `MERGE` idempotent.
///
/// ── CE QUI AUTORISE, ET CE QUI DÉCIDE DU SELF ─────────────────────────────
/// Le compte du jeton, et lui seul. Un `client_id` n'ouvre rien : le registre
/// vérifie que ce client APPARTIENT à ce compte (même contrôle que
/// `remote::appeler`). Et le SELF n'est jamais lu dans le corps de la requête :
/// il se DÉDUIT du compte (`ancrage::self_qn_pour`), sinon un client enverrait
/// `self:bob` et écrirait chez le voisin. Un `self` présent dans le corps est
/// signalé dans la réponse comme IGNORÉ — le taire laisserait croire qu'il a
/// servi.
fn api_remote_memoire(
    stream: &mut TcpStream,
    compte: &str,
    corps: &serde_json::Value,
    client_id: &str,
) -> Result<()> {
    let erreur = |stream: &mut TcpStream, code: u16, message: &str| -> Result<()> {
        respond(stream, code, VIZ_JSON, &serde_json::json!({ "error": message }).to_string())
    };

    // ── 1. VALIDER AVANT D'ÉCRIRE ──────────────────────────────────────────
    // « ANNONCER EST DECIDER » : une intention journalisée qu'on ne saura pas
    // appliquer est une promesse qu'on ne tiendra pas — et le journal ne se
    // viderait jamais, panne silencieuse s'il en est. On refuse À L'ENTRÉE, en
    // nommant le champ fautif.
    let intentions = match intentions_recues(corps) {
        Ok(i) => i,
        Err(motif) => return erreur(stream, 400, &motif),
    };

    // ── 2. LE SELF VIENT DU COMPTE DU JETON, JAMAIS DU CORPS ───────────────
    let (self_qn, machine) = match self_du_client_attache(compte, client_id) {
        Ok(x) => x,
        Err(motif) => return erreur(stream, 403, &motif),
    };

    // ── 3. JOURNALISER (append-only, sans verrou) ──────────────────────────
    // La provenance est ce qui rend la mémoire VÉRIFIABLE : une note sans
    // origine ne se rattache à rien. Elle nomme la machine et l'appel, jamais
    // le jeton — un jeton écrit une fois dans un journal y reste pour toujours.
    let provenance = format!("remote:{machine}:{client_id}");
    let mut journalisees: Vec<serde_json::Value> = Vec::new();
    let mut echecs: Vec<String> = Vec::new();
    for intention in intentions {
        let genre = intention.genre();
        match crate::memoire::enregistrer(&self_qn, &provenance, intention) {
            Ok(e) => journalisees.push(serde_json::json!({
                "t": e.t_ms,
                "genre": e.genre,
                "reference": e.reference,
            })),
            Err(e) => echecs.push(format!("{genre} : {e:#}")),
        }
    }
    let chemin = crate::memoire::journal(&self_qn);
    if journalisees.is_empty() {
        // Rien n'a été écrit : c'est une PANNE (disque plein, répertoire
        // refusé), et elle doit remonter comme telle plutôt que comme un
        // succès vide. Le journal d'activation suit le même contrat.
        return erreur(
            stream,
            500,
            &format!(
                "aucune intention n'a pu être journalisée pour {self_qn} : {}",
                echecs.join(" ; ")
            ),
        );
    }
    eprintln!(
        "[remote] {compte} a journalisé {} intention(s) pour {self_qn} (machine « {machine} »)",
        journalisees.len()
    );

    // Le self que le corps a NOMMÉ, s'il y en a un : on ne s'en sert pas, et on
    // le dit. Laisser croire qu'il a servi serait la pire des deux réponses.
    let self_du_corps: Option<String> = ["self", "self_qn", "selfQn"]
        .iter()
        .find_map(|k| corps.get(*k).and_then(|x| x.as_str()))
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty() && s.trim() != self_qn);

    let body = serde_json::json!({
        "self": self_qn,
        "compte": compte,
        "client_id": client_id,
        "machine": machine,
        "journal": chemin.to_string_lossy(),
        "genre_acceptes": ["note", "objectif", "procedure", "etat-self"],
        "journalisees": journalisees,
        "echecs": echecs,
        // Ce qui attend encore dans le journal APRÈS cet envoi : c'est le
        // chiffre qui dit à l'appelant si sa note a rejoint la base ou non.
        "en_attente": crate::memoire::en_attente(&self_qn).len(),
        // ── DEUX TEMPS, ET ON LE DIT ───────────────────────────────────────
        // « dans_la_base: false » n'est pas un échec : c'est le motif même. Le
        // serveur ne peut pas écrire dans une base qu'il tient en lecture.
        "etat": "journalisé",
        "dans_la_base": false,
        "prochaine_etape": format!(
            "obrain memoire --store <base du self> --self {self_qn} --appliquer \
             (ouvre la base en EXCLUSIF : le timer obrain-self-appliquer le fait PAR COMPTE)"
        ),
        "regle": "le self est celui du compte propriétaire du client ; un self nommé dans le corps est IGNORÉ",
        "self_du_corps_ignore": self_du_corps,
    })
    .to_string();
    respond(stream, 200, VIZ_JSON, &body)
}

/// Le Self que NOURRIT une machine attachée : déduit du compte, jamais reçu.
///
/// Le compte vient de `compte_appelant` — c'est-à-dire du JETON, ou du nom
/// d'occupant unique (`local`) quand l'IAM n'est pas armé, auquel cas on refuse
/// (voir plus bas : sans identité, on ne sait pas quel self nourrir).
///
/// Rend `(self_qn, machine)`. Refuse — avec le motif — quand le client n'est pas
/// à ce compte. Le message dit les DEUX causes possibles (« il n'existe pas »,
/// « il est à un autre compte ») plutôt que d'en affirmer une : `clients_du_compte`
/// ne les distingue pas, et un refus qui invente sa cause envoie l'utilisateur
/// chercher au mauvais endroit.
fn self_du_client_attache(compte: &str, client_id: &str) -> Result<(String, String), String> {
    if client_id.trim().is_empty() {
        return Err(
            "« client_id » requis : c'est la machine qui publie, et le self se déduit de son \
             compte. Attachez-la d'abord (`selfd attacher --moteur <url> --jeton-moteur <votre jeton>`)"
                .to_string(),
        );
    }
    let clients = crate::remote::clients_du_compte(compte);
    let Some(client) = clients.iter().find(|c| c.id == client_id) else {
        return Err(format!(
            "le client « {client_id} » n'est pas attaché à votre compte (soit il n'existe pas, \
             soit il est attaché à un autre compte) : la mémoire d'un self ne s'écrit que depuis \
             une machine de CE compte"
        ));
    };
    // « local » est le nom de l'occupant unique quand l'IAM n'est pas armé : il
    // n'y a alors AUCUNE identité, et le self est celui de l'instance — le même
    // que celui que `/api/engine/memoire` résout pour un appelant anonyme. Les
    // deux chemins doivent nommer le même self, sinon on écrirait dans un
    // journal que personne ne lit.
    // ── SANS IDENTITÉ, ON N'ÉCRIT NULLE PART ───────────────────────────────
    // « local » est le nom de l'occupant unique quand l'IAM n'est PAS armé : il
    // n'y a alors aucune identité. Le self de l'instance existe bien — les
    // routes de LECTURE le résolvent avec la base synaptique du serveur — mais
    // ce handler ne la reçoit pas (`api_remote` n'a pas de `ServeConfig`) : il
    // ne peut donc pas garantir qu'il nomme le MÊME self que
    // `/api/engine/memoire`. On REFUSE plutôt que de deviner, parce qu'une
    // écriture est irréversible là où une lecture est indolore : un self
    // deviné, c'est une mémoire rangée chez quelqu'un d'autre — le défaut exact
    // que `self_qn_pour` a été écrit pour corriger (« un ami rangeait donc ses
    // souvenirs chez quelqu'un d'autre »). Le motif dit quoi faire.
    if garde_interieure().is_none() {
        return Err(format!(
            "ce moteur ne connaît aucune identité (IAM non armé) : il ne peut donc pas dire QUEL \
             self nourrir, et deviner reviendrait à ranger les souvenirs d'un compte chez un \
             autre. Armez l'IAM du moteur (OBRAIN_IAM=1, OBRAIN_IAM_STORE=<base des identités>) \
             et présentez le jeton de VOTRE compte — la machine « {} » s'attachera alors sous ce \
             compte.",
            client.machine
        ));
    }
    // Un seul résolveur de self pour toute la surface (`ancrage::self_qn_pour`).
    // On ne rend PAS sa portée : sans base synaptique sous la main dans ce
    // handler, elle vaudrait toujours « journal », c'est-à-dire une mesure qu'on
    // n'a pas faite.
    let (self_qn, _portee) = crate::ancrage::self_qn_pour(None, Some(compte));
    Ok((self_qn, client.machine.clone()))
}

/// ── LE PLAFOND D'UN ENVOI ───────────────────────────────────────────────────
/// Un envoi tient dans le corps HTTP (plafonné à 1 Mio par le serveur), mais un
/// journal n'est PAS un dépotoir : la passe d'application relit tout le fichier
/// à chaque tour, et un client qui publierait mille intentions par seconde
/// ferait payer ce coût à tout le monde. 32 par envoi, et 4 000 caractères par
/// champ : largement au-delà de ce qu'une note de nœud porte, très en dessous de
/// ce qui rendrait une passe ingérable.
const MEMOIRE_MAX_INTENTIONS: usize = 32;
const MEMOIRE_MAX_CHAMP: usize = 4_000;

/// Les intentions d'un corps, VALIDÉES — ou le motif exact du refus.
///
/// On ne réutilise pas `Intention::depuis_json` (qui remplit les champs absents
/// de chaînes vides, donc accepte en silence une note sans texte) : ici, ce qui
/// manque est NOMMÉ, parce que c'est le seul message qui permet à la machine de
/// corriger son envoi.
fn intentions_recues(corps: &serde_json::Value) -> Result<Vec<crate::memoire::Intention>, String> {
    use crate::memoire::Intention;
    let Some(liste) = corps.get("intentions").and_then(|x| x.as_array()) else {
        return Err(
            "« intentions » requis : un tableau d'objets, chacun portant « kind » \
             (note | objectif | procedure | etat-self)"
                .to_string(),
        );
    };
    if liste.is_empty() {
        // Une réponse vide ne doit pas avoir la forme d'une bonne réponse : un
        // envoi sans intention n'est pas un succès, c'est une erreur d'appel.
        return Err("« intentions » est vide : il n'y a rien à journaliser".to_string());
    }
    if liste.len() > MEMOIRE_MAX_INTENTIONS {
        return Err(format!(
            "{} intentions dans cet envoi, au-delà du plafond de {MEMOIRE_MAX_INTENTIONS} : \
             découpez l'envoi (le journal est relu en entier à chaque passe d'application)",
            liste.len()
        ));
    }
    let mut sorties = Vec::with_capacity(liste.len());
    for (i, item) in liste.iter().enumerate() {
        sorties.push(intention_recue(item).map_err(|m| format!("intention {i} : {m}"))?);
    }
    Ok(sorties)
}

/// Une intention, validée champ par champ.
///
/// Ce qui est EXIGÉ est exactement ce dont la passe d'application a besoin pour
/// construire un nœud identifiable (`MERGE` sur un `qn` dérivé du texte ou du
/// nom) : une note sans texte, un objectif sans nom, une procédure sans nom
/// donneraient des nœuds sans identité — journalisés, refusés, et jamais
/// consommés.
fn intention_recue(v: &serde_json::Value) -> Result<crate::memoire::Intention, String> {
    use crate::memoire::Intention;
    let Some(kind) = v.get("kind").and_then(|x| x.as_str()) else {
        return Err("« kind » requis (note | objectif | procedure | etat-self)".to_string());
    };
    let champ = |nom: &str| -> Result<String, String> {
        match v.get(nom) {
            None => Ok(String::new()),
            Some(serde_json::Value::String(s)) => {
                let t = s.trim();
                if t.chars().count() > MEMOIRE_MAX_CHAMP {
                    return Err(format!(
                        "« {nom} » fait {} caractères, au-delà du plafond de {MEMOIRE_MAX_CHAMP}",
                        t.chars().count()
                    ));
                }
                Ok(t.to_string())
            }
            Some(_) => Err(format!("« {nom} » doit être une chaîne")),
        }
    };
    let exiger = |valeur: String, nom: &str| -> Result<String, String> {
        if valeur.is_empty() {
            Err(format!("« {nom} » requis (non vide) pour une intention « {kind} »"))
        } else {
            Ok(valeur)
        }
    };
    Ok(match kind {
        "note" => Intention::Note {
            texte: exiger(champ("texte")?, "texte")?,
            // Le sujet qualifie la note sans la porter : son absence ne rend pas
            // l'intention inapplicable (le `qn` vient du texte), donc on ne la
            // refuse pas — on la journalise telle qu'elle est.
            sujet: champ("sujet")?,
        },
        "objectif" => Intention::Objectif {
            nom: exiger(champ("nom")?, "nom")?,
            texte: champ("texte")?,
        },
        "procedure" => Intention::Procedure {
            nom: exiger(champ("nom")?, "nom")?,
            description: champ("description")?,
            corps: champ("corps")?,
        },
        "etat-self" => Intention::EtatSelf {
            epithet: champ("epithet")?,
            objectif: exiger(champ("objectif")?, "objectif")?,
            prochaine_action: champ("prochaine_action")?,
            note: champ("note")?,
        },
        autre => {
            return Err(format!(
                "kind « {autre} » inconnu : les formes acceptées sont note, objectif, procedure, \
                 etat-self (ce sont les entités qui existent déjà dans la base du self — en \
                 inventer une ferait diverger la mémoire du modèle tissé)"
            ));
        }
    })
}

fn api_worlds_ajouter(
    stream: &mut TcpStream,
    entetes: &str,
    query: Option<&str>,
    corps: &str,
) -> Result<()> {
    let v: serde_json::Value =
        serde_json::from_str(corps).unwrap_or_else(|_| serde_json::json!({}));
    let url = v.get("url").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
    let label = v.get("label").and_then(|x| x.as_str()).map(String::from);
    if url.is_empty() {
        let body = serde_json::json!({
            "error": "url requis — attendu https://github.com/<propriétaire>/<dépôt>"
        })
        .to_string();
        return respond(stream, 200, VIZ_JSON, &body);
    }
    // L'identité telle que l'IAM la rend (`utilisateur@compte`). `None` quand
    // l'IAM n'est pas armé : on n'invente pas d'auteur, on enregistre l'absence.
    let auteur = jeton_presente(entetes, query)
        .and_then(|j| GardeIam::partagee().identite(&j));
    match crate::worlds::lancer(url, label, auteur) {
        Ok(id) => {
            let body = serde_json::json!({ "job": id }).to_string();
            respond(stream, 200, VIZ_JSON, &body)
        }
        Err(e) => {
            let body = serde_json::json!({ "error": format!("{e:#}") }).to_string();
            respond(stream, 200, VIZ_JSON, &body)
        }
    }
}

/// `GET /api/worlds` — les mondes ajoutés à chaud et les travaux en cours.
fn api_worlds_liste(
    stream: &mut TcpStream,
    entetes: &str,
    query: Option<&str>,
    config: &ServeConfig,
) -> Result<()> {
    let mut mondes: Vec<serde_json::Value> = Vec::new();
    // Même règle que `/api/stores` : on ne LISTE que ce que l'appelant lit.
    for (label, path) in mondes_visibles(entetes, query, config) {
        let db = cached_ro(&path).ok();
        let compte = |gql: &str| -> i64 {
            db.as_ref()
                .and_then(|d| d.session().execute(gql).ok())
                .and_then(|r| r.rows.into_iter().next())
                .and_then(|row| row.into_iter().next())
                .and_then(|v| match v {
                    obrain_common::types::Value::Int64(x) => Some(x),
                    obrain_common::types::Value::Float64(x) => Some(x as i64),
                    _ => None,
                })
                .unwrap_or(0)
        };
        mondes.push(serde_json::json!({
            "label": label,
            "path": path.to_string_lossy(),
            "nodes": compte("MATCH (n) RETURN count(n)"),
            "edges": compte("MATCH ()-[r]->() RETURN count(r)"),
            "ajoute_a_chaud": crate::worlds::enregistres().iter().any(|m| m.store == path),
            // ── QUI A AJOUTÉ CE MONDE ───────────────────────────────────────
            // Un monde ajouté à chaud apparaît chez tout le monde : sans cette
            // attribution, un monde inconnu apparaîtrait dans la liste sans
            // qu'on puisse savoir d'où il vient ni à qui demander.
            "proprietaire": crate::worlds::proprietaire_de(&path),
        }));
    }
    let body = serde_json::json!({
        "racine": std::env::var("OBRAIN_WORLDS_DIR").unwrap_or_else(|_| "/tmp/obrain-worlds".into()),
        "mondes": mondes,
        "travaux": crate::worlds::tous(),
    })
    .to_string();
    respond(stream, 200, VIZ_JSON, &body)
}

/// `GET /api/worlds/status?job=` — avancement d'un ajout de monde.
fn api_worlds_etat(stream: &mut TcpStream, query: Option<&str>) -> Result<()> {
    let params = parse_query(query);
    let id = params.get("job").cloned().unwrap_or_default();
    let body = match crate::worlds::etat(&id) {
        Some(t) => serde_json::to_string(&t)?,
        None => serde_json::json!({ "error": format!("travail inconnu : {id}") }).to_string(),
    };
    respond(stream, 200, VIZ_JSON, &body)
}

#[cfg(test)]
mod tests {
    use super::{
        accept_encoding_veut_gzip, compute_delta, est_compressible, gzip_corps, importance_order,
        percent_decode,
    };

    /// Le delta `old -> new`, applique sur l'etat client, reconstruit EXACTEMENT
    /// `new` : ajout de noeud+arete, retrait, et changement de champ (upsert).
    #[test]
    fn compute_delta_reconstruit_exactement_new() {
        use obrain_viz::{Subgraph, VizEdge, VizNode};
        let mut hub = VizNode::new("hub");
        hub.energy = Some(0.1);
        let old = Subgraph {
            nodes: vec![hub.clone(), VizNode::new("a"), VizNode::new("gone")],
            edges: vec![
                VizEdge::new("hub", "a", "CALLS"),
                VizEdge::new("hub", "gone", "CALLS"),
            ],
            truncated: false,
        };
        // new : "gone" retire, "b" ajoute, "hub" change (energy), arete hub->b ajoutee.
        let mut hub2 = VizNode::new("hub");
        hub2.energy = Some(0.9); // change
        let new = Subgraph {
            nodes: vec![hub2.clone(), VizNode::new("a"), VizNode::new("b")],
            edges: vec![
                VizEdge::new("hub", "a", "CALLS"),
                VizEdge::new("hub", "b", "CALLS"),
            ],
            truncated: false,
        };
        let d = compute_delta(&old, &new);
        // hub (change) et b (ajout) dans added_nodes ; gone retire.
        let added_ids: std::collections::HashSet<&str> =
            d.added_nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(added_ids.contains("hub") && added_ids.contains("b"));
        assert!(!added_ids.contains("a")); // inchange
        assert_eq!(d.removed_nodes, vec!["gone".to_string()]);
        assert_eq!(d.added_edges.len(), 1);
        assert_eq!(d.added_edges[0].dst, "b");
        assert_eq!(d.removed_edges, vec![("hub".into(), "gone".into(), "CALLS".into())]);

        // Application du delta sur `old` -> doit egaler `new` (en ensembles).
        let mut etat: std::collections::HashMap<String, VizNode> =
            old.nodes.iter().map(|n| (n.id.clone(), n.clone())).collect();
        for id in &d.removed_nodes {
            etat.remove(id);
        }
        for n in &d.added_nodes {
            etat.insert(n.id.clone(), n.clone());
        }
        let ekey = |e: &VizEdge| (e.src.clone(), e.dst.clone(), e.edge_type.clone());
        let mut edges: std::collections::HashSet<(String, String, String)> =
            old.edges.iter().map(ekey).collect();
        for k in &d.removed_edges {
            edges.remove(k);
        }
        for e in &d.added_edges {
            edges.insert(ekey(e));
        }
        let attendu_nodes: std::collections::HashMap<String, VizNode> =
            new.nodes.iter().map(|n| (n.id.clone(), n.clone())).collect();
        let attendu_edges: std::collections::HashSet<(String, String, String)> =
            new.edges.iter().map(ekey).collect();
        assert_eq!(etat, attendu_nodes, "noeuds reconstruits != new");
        assert_eq!(edges, attendu_edges, "aretes reconstruites != new");
    }

    /// L'ordre d'importance est une PERMUTATION (identite preservee) et met les
    /// HUBS (degre eleve) en tete — le squelette peint d'abord.
    #[test]
    fn importance_order_met_les_hubs_en_tete_et_preserve_identite() {
        use obrain_viz::{Subgraph, VizEdge, VizNode};
        // "hub" a degre 3 ; "a","b","c" degre 1 ; "iso" degre 0.
        let sg = Subgraph {
            nodes: vec![
                VizNode::new("iso"),
                VizNode::new("a"),
                VizNode::new("hub"),
                VizNode::new("b"),
                VizNode::new("c"),
            ],
            edges: vec![
                VizEdge::new("hub", "a", "CALLS"),
                VizEdge::new("hub", "b", "CALLS"),
                VizEdge::new("hub", "c", "CALLS"),
            ],
            truncated: false,
        }
        .canonical();
        let (node_order, edge_order, label) = importance_order(&sg);
        assert_eq!(label, "importance:degree");
        // Le premier noeud stream est le hub.
        assert_eq!(sg.nodes[node_order[0]].id, "hub");
        // Permutation complete : memes indices, aucun perdu/duplique.
        let mut ns = node_order.clone();
        ns.sort_unstable();
        assert_eq!(ns, (0..sg.nodes.len()).collect::<Vec<_>>());
        let mut es = edge_order.clone();
        es.sort_unstable();
        assert_eq!(es, (0..sg.edges.len()).collect::<Vec<_>>());
    }

    /// Sans arete, repli deterministe sur l'ordre canonique (par id).
    #[test]
    fn importance_order_repli_canonique_sans_aretes() {
        use obrain_viz::{Subgraph, VizNode};
        let sg = Subgraph {
            nodes: vec![VizNode::new("z"), VizNode::new("a"), VizNode::new("m")],
            edges: vec![],
            truncated: false,
        }
        .canonical();
        let (node_order, _edge_order, label) = importance_order(&sg);
        assert_eq!(label, "canonical");
        // canon a deja trie par id ; l'ordre reste (0,1,2) -> a,m,z.
        let ids: Vec<&str> = node_order.iter().map(|&i| sg.nodes[i].id.as_str()).collect();
        assert_eq!(ids, vec!["a", "m", "z"]);
    }

    #[test]
    fn percent_decode_basic() {
        assert_eq!(percent_decode("a%20b%2Fc"), "a b/c");
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(percent_decode("plain"), "plain");
    }

    #[test]
    fn percent_decode_invalid_hex_keeps_raw() {
        assert_eq!(percent_decode("a%zz"), "a%zz");
    }

    /// ── UN `%` SUIVI D'UN CARACTÈRE MULTI-OCTETS NE DOIT PAS TUER LE THREAD ──
    ///
    /// Défaut mesuré (2026-09-11) : le décodage coupait la `&str` avec des bornes
    /// en OCTETS (`input[i + 1..i + 3]` alors que `i` avance sur les octets), donc
    /// dès qu'un caractère multi-octets commençait dans l'intervalle, le thread
    /// qui servait le client PANIQUAIT — `curl '…?x=%aé'` sortait en 52,
    /// « empty reply from server », sans un octet d'explication. Le décodeur est
    /// désormais purement octet, et l'hexadécimal non-ASCII est refusé.
    #[test]
    fn percent_decode_ne_panique_pas_sur_multi_octets() {
        assert_eq!(percent_decode("x=%aé"), "x=%aé");
        assert_eq!(percent_decode("%aé"), "%aé");
        // Un vrai encodage multi-octets reste décodé.
        assert_eq!(percent_decode("%C3%A9"), "é");
        assert_eq!(percent_decode("caf%C3%A9"), "café");
        // Un « % » final ou tronqué ne panique pas non plus.
        assert_eq!(percent_decode("fin%"), "fin%");
        assert_eq!(percent_decode("fin%A"), "fin%A");
    }
}

#[cfg(test)]
mod tests_jeton {
    use super::jeton_presente;

    /// Les TROIS façons de présenter une identité, et le décalage d'un
    /// caractère qui a rendu la deuxième inopérante jusqu'à l'essai de bout en
    /// bout du 2026-09-13.
    #[test]
    fn les_trois_sources_de_jeton_sont_lues() {
        let entetes = "GET /api/query HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer abc123\r\n";
        assert_eq!(jeton_presente(entetes, None).as_deref(), Some("abc123"));

        let cookie = "GET /api/query HTTP/1.1\r\nCookie: autre=1; obrain_session=jeton-cookie\r\n";
        assert_eq!(
            jeton_presente(cookie, None).as_deref(),
            Some("jeton-cookie"),
            "le cookie doit être lu (7 caractères pour « cookie: »)"
        );

        // L'en-tête prime sur le cookie, le cookie sur la requête.
        let mixte = "GET / HTTP/1.1\r\nCookie: obrain_session=c\r\nAuthorization: Bearer b\r\n";
        assert_eq!(jeton_presente(mixte, None).as_deref(), Some("b"));

        // `EventSource` ne pose pas d'en-tête : le paramètre sert au flux SSE.
        assert_eq!(jeton_presente("", Some("token=sse")).as_deref(), Some("sse"));
        assert_eq!(jeton_presente("", None), None);
    }
}

/// ── TESTS DE LA COUCHE TRANSPORT ─────────────────────────────────────────────
///
/// Ce module existe parce que les précédents n'en couvraient AUCUNE : ils
/// vérifiaient `percent_decode` et la lecture du jeton, deux fonctions pures,
/// pendant que le pipelining, le keep-alive, la négociation de contenu, la
/// revalidation et l'assainissement des chemins n'étaient vérifiés par RIEN —
/// c'est-à-dire que le transport ne pouvait casser qu'en production.
///
/// Tout ce qui suit est PUR : pas de socket, pas de serveur, pas de LLM, pas de
/// port à ouvrir. C'est délibéré : un test qui prend un port échoue en
/// intégration continue, et un test qui interroge un modèle n'est pas un test.
/// Ce qui exige une socket (les en-têtes réellement écrits, les délais, la
/// fermeture du flux) se vérifie par les mesures du chantier, pas ici.
#[cfg(test)]
mod tests_transport {
    use super::*;
    use std::io::Read;

    /// Analyse une requête complète, ou panique : la lisibilité d'abord.
    fn requete(brut: &str) -> Requete {
        let mut tampon = brut.as_bytes().to_vec();
        match extraire_requete(&mut tampon) {
            Ok(Extraction::Prete(r)) => *r,
            autre => panic!("requête attendue, obtenu {autre:?}"),
        }
    }

    /// Le code du refus attendu, ou panique.
    fn refus(brut: &str) -> u16 {
        let mut tampon = brut.as_bytes().to_vec();
        match extraire_requete(&mut tampon) {
            Err(ErreurLecture::Refus(code, _)) => code,
            autre => panic!("refus attendu, obtenu {autre:?}"),
        }
    }

    /// Un dossier jetable AVEC un fichier dedans : la garde de chemin se vérifie
    /// contre le système de fichiers réel, pas contre une maquette — c'est le
    /// seul moyen de tester la canonicalisation (donc les liens symboliques).
    fn dossier_jetable(nom: &str) -> PathBuf {
        let racine = std::env::temp_dir().join(format!("obrain-transport-{}-{nom}", std::process::id()));
        let _ = std::fs::remove_dir_all(&racine);
        std::fs::create_dir_all(racine.join("assets")).expect("dossier de test");
        std::fs::write(racine.join("index.html"), b"<!doctype html>").unwrap();
        std::fs::write(racine.join("assets/index-abc.js"), b"console.log(1)").unwrap();
        std::fs::write(std::env::temp_dir().join("obrain-transport-secret.txt"), b"secret").unwrap();
        racine
    }

    // ── LA TRAME : CE QUE L'ANCIENNE LECTURE PERDAIT ────────────────────────
    /// Deux requêtes dans le MÊME tampon. L'ancien code lisait 64 Kio une fois et
    /// servait la première : la seconde était perdue, sans trace ni erreur.
    #[test]
    fn deux_requetes_pipelinees_sont_extradites_l_une_apres_l_autre() {
        let mut tampon = b"GET /api/viz/status HTTP/1.1\r\nHost: x\r\n\r\n\
                           GET /app/index.html HTTP/1.1\r\nHost: x\r\n\r\n"
            .to_vec();
        let une = match extraire_requete(&mut tampon).unwrap() {
            Extraction::Prete(r) => *r,
            Extraction::Incomplete => panic!("la première requête est complète"),
        };
        assert_eq!(une.methode, "GET");
        assert_eq!(une.chemin, "/api/viz/status");
        assert!(une.requete.is_none());
        let deux = match extraire_requete(&mut tampon).unwrap() {
            Extraction::Prete(r) => *r,
            Extraction::Incomplete => panic!("la seconde requête est dans le même tampon"),
        };
        assert_eq!(deux.chemin, "/app/index.html");
        assert!(tampon.is_empty(), "le tampon doit être consommé exactement : {:?}", tampon);
    }

    /// Le corps d'un `POST` arrivé en deux morceaux : c'est le même problème que
    /// le pipelining, et la même solution — ne rien consommer tant que la
    /// requête n'est pas complète.
    #[test]
    fn un_corps_en_deux_morceaux_est_attendu_sans_consommer_le_tampon() {
        let complet = b"POST /api/worlds HTTP/1.1\r\nHost: x\r\nContent-Length: 11\r\n\r\n{\"url\":\"a\"}".to_vec();
        let coupe = complet.len() - 4;
        let mut tampon = complet[..coupe].to_vec();
        let avant = tampon.len();
        let premier = extraire_requete(&mut tampon).unwrap();
        assert!(matches!(premier, Extraction::Incomplete), "corps incomplet : {premier:?}");
        assert_eq!(tampon.len(), avant, "rien ne doit être consommé tant qu'il manque des octets");
        tampon.extend_from_slice(&complet[coupe..]);
        match extraire_requete(&mut tampon).unwrap() {
            Extraction::Prete(r) => {
                assert_eq!(r.methode, "POST");
                assert_eq!(r.corps, b"{\"url\":\"a\"}");
            }
            Extraction::Incomplete => panic!("le corps est maintenant complet"),
        }
        assert!(tampon.is_empty());
    }

    #[test]
    fn la_recherche_du_terminateur_est_exacte() {
        assert_eq!(trouver(b"a\r\n\r\nb", b"\r\n\r\n"), Some(1));
        assert_eq!(trouver(b"a\n\nb", b"\r\n\r\n"), None, "un terminateur LF seul n'en est pas un");
        assert_eq!(trouver(b"", b"\r\n\r\n"), None);
    }

    // ── LA RIGUEUR : UN REFUS DOIT SE DIRE ─────────────────────────────────
    /// Mesuré avant : une ligne de requête vide ne recevait AUCUNE réponse, donc
    /// le client pendait jusqu'à son propre délai sans apprendre pourquoi.
    #[test]
    fn une_ligne_de_requete_vide_ou_illisible_est_un_400() {
        assert_eq!(refus("\r\n\r\n"), 400, "`\\r\\n\\r\\n` d'entrée = ligne de requête vide");
        assert_eq!(refus("GET\r\nHost: x\r\n\r\n"), 400, "il manque la cible et la version");
        assert_eq!(refus("GET / HTTP/1.1 extra\r\nHost: x\r\n\r\n"), 400, "quatre champs");
        assert_eq!(refus("GET / HTTP/1.1\r\nHost x\r\n\r\n"), 400, "en-tête sans deux-points");
        assert_eq!(refus("GET / HTTP/1.1\r\nHost : x\r\n\r\n"), 400, "espace avant les deux-points");
    }

    /// RFC 9112 §3.2 : `Host` est OBLIGATOIRE en 1.1, facultatif en 1.0. Mesuré
    /// avant : les deux recevaient 200.
    #[test]
    fn host_obligatoire_en_11_et_tolere_en_10() {
        assert_eq!(refus("GET / HTTP/1.1\r\n\r\n"), 400);
        let dix = requete("GET / HTTP/1.0\r\n\r\n");
        assert!(!dix.http11);
        assert_eq!(dix.chemin, "/");
        assert!(requete("GET / HTTP/1.1\r\nHost: 127.0.0.1:8433\r\n\r\n").http11);
    }

    /// Mesuré avant : un `Content-Length` menteur de 10 Mo faisait pendre le
    /// client 30 s (le corps était lu en boucle, sans plafond ni délai).
    ///
    /// ── LA LIGNE DE REQUÊTE DE CE TEST ÉTAIT FAUSSE (corrigé le 2026-09-13) ──
    /// Elle s'écrivait `POST / x HTTP/1.1` : la CIBLE contenait une espace, donc
    /// la ligne se découpait en QUATRE champs et le lecteur refusait en 400
    /// « ligne de requête illisible » — AVANT d'atteindre le corps. Trois des
    /// quatre assertions passaient donc pour la mauvaise raison (elles
    /// vérifiaient l'analyse de la ligne, pas celle du `Content-Length`), et la
    /// quatrième — `Transfer-Encoding` attendu en 501 — a révélé l'écart en
    /// rendant 400. C'est le seul échec de la suite : le code était juste, le
    /// test mesurait autre chose que ce qu'il annonçait.
    #[test]
    fn un_content_length_menteur_est_refuse_avant_toute_lecture() {
        assert_eq!(refus("POST /api/worlds HTTP/1.1\r\nHost: x\r\nContent-Length: 10000000\r\n\r\n{}"), 413);
        assert_eq!(refus("POST /x HTTP/1.1\r\nHost: x\r\nContent-Length: 2\r\nContent-Length: 3\r\n\r\n{}"), 400);
        assert_eq!(refus("POST /x HTTP/1.1\r\nHost: x\r\nContent-Length: abc\r\n\r\n"), 400);
        assert_eq!(refus("POST /x HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n"), 501);
    }

    #[test]
    fn une_version_non_servie_est_un_505() {
        assert_eq!(refus("GET / HTTP/2.0\r\nHost: x\r\n\r\n"), 505);
        assert_eq!(refus("GET / PLOUF\r\nHost: x\r\n\r\n"), 400);
    }

    #[test]
    fn un_bloc_d_entetes_trop_long_est_un_431() {
        let mut brut = String::from("GET / HTTP/1.1\r\nHost: x\r\n");
        for i in 0..2000 {
            brut.push_str(&format!("X-{i}: v\r\n"));
        }
        brut.push_str("\r\n");
        assert_eq!(refus(&brut), 431);
    }

    // ── LE CADRE DE CONNEXION ──────────────────────────────────────────────
    #[test]
    fn keep_alive_est_le_defaut_en_11_et_sur_demande_en_10() {
        let onze = etat_du_client(&requete("GET / HTTP/1.1\r\nHost: x\r\n\r\n"));
        assert!(onze.keep_alive, "HTTP/1.1 garde par défaut");
        assert!(!onze.head && !onze.fermer && !onze.repondu);

        let ferme = etat_du_client(&requete("GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"));
        assert!(!ferme.keep_alive, "`Connection: close` du client doit être honoré");

        let dix_garde = etat_du_client(&requete("GET / HTTP/1.0\r\nConnection: keep-alive\r\n\r\n"));
        assert!(dix_garde.keep_alive);
        let dix = etat_du_client(&requete("GET / HTTP/1.0\r\n\r\n"));
        assert!(!dix.keep_alive, "HTTP/1.0 ferme par défaut : la garder ouverte ferait pendre le client");

        // `HEAD` : reconnu ici, retenu au dernier moment par `respond_octets`.
        assert!(etat_du_client(&requete("HEAD / HTTP/1.1\r\nHost: x\r\n\r\n")).head);
        // La négociation est transportée telle quelle (l'analyse est pure).
        let negocie = etat_du_client(&requete("GET / HTTP/1.1\r\nHost: x\r\nAccept-Encoding: gzip\r\nIf-None-Match: \"a\"\r\n\r\n"));
        assert_eq!(negocie.accepte_encodage.as_deref(), Some("gzip"));
        assert_eq!(negocie.if_none_match.as_deref(), Some("\"a\""));
    }

    /// `HEAD` sur une route qui n'existe qu'en `POST` : un refus, et il DIT quoi.
    #[test]
    fn head_est_refuse_sur_une_route_post() {
        assert_eq!(methode_refusee("/api/auth/login", "HEAD"), Some("POST"));
        assert_eq!(methode_refusee("/api/auth/login", "GET"), Some("POST"));
        assert_eq!(methode_refusee("/api/auth/logout", "HEAD"), Some("POST"));
        assert_eq!(methode_refusee("/api/auth/refresh", "GET"), Some("POST"));
        assert_eq!(methode_refusee("/api/auth/login", "POST"), None);
        // Une route de lecture accepte HEAD : le corps est retenu plus loin, et
        // c'est ce qui rend `curl -I` juste au lieu de le refuser.
        assert_eq!(methode_refusee("/api/viz/status", "HEAD"), None);
        assert_eq!(methode_refusee("/app/assets/index-abc.js", "HEAD"), None);
        assert_eq!(methode_refusee("/", "HEAD"), None);
    }

    // ── LA FAILLE DE CHEMIN ────────────────────────────────────────────────
    /// La faille mesurée : `/app/../../../../…/etc/hostname` rendait 200 avec le
    /// contenu du fichier. Chaque écriture hostile connue est refusée ici.
    #[test]
    fn le_chemin_d_un_actif_ne_peut_pas_sortir_du_dossier() {
        let racine = dossier_jetable("traversee");
        for hostile in [
            "../obrain-transport-secret.txt",
            "../../../../../../etc/hostname",
            "assets/../../obrain-transport-secret.txt",
            "./../obrain-transport-secret.txt",
            "%2e%2e/obrain-transport-secret.txt",
            "%2E%2E%2Fobrain-transport-secret.txt",
            "..%2fobrain-transport-secret.txt",
            "assets/%2e%2e/%2e%2e/obrain-transport-secret.txt",
            "/etc/hostname",
            "//etc/hostname",
            "assets\\..\\secret",
            "",
            "index.html%00.png",
        ] {
            assert!(chemin_sous(&racine, hostile).is_none(), "{hostile:?} doit être refusé");
        }
        // Ce qui est légitime continue de passer — une garde qui refuse tout
        // « pour être sûre » casse le service qu'elle protège.
        assert!(chemin_sous(&racine, "index.html").is_some());
        assert!(chemin_sous(&racine, "assets/index-abc.js").is_some());
        assert!(chemin_sous(&racine, "assets/index..js").is_none(), "ce fichier n'existe pas — 404");
        // Absent = refusé aussi : c'est le 404 de la route.
        assert!(chemin_sous(&racine, "assets/inexistant.js").is_none());
        let _ = std::fs::remove_dir_all(&racine);
    }

    /// Un LIEN qui sort de la racine : aucun `..` dans le chemin, donc seul le
    /// contrôle de préfixe sur le chemin canonique peut l'arrêter. C'est la
    /// raison d'être des deux verrous.
    #[cfg(unix)]
    #[test]
    fn un_lien_symbolique_qui_sort_de_la_racine_est_refuse() {
        let racine = dossier_jetable("lien");
        let dehors = std::env::temp_dir().join("obrain-transport-secret.txt");
        std::os::unix::fs::symlink(&dehors, racine.join("evasion.txt")).expect("lien symbolique");
        assert!(chemin_sous(&racine, "evasion.txt").is_none(), "le lien doit être refusé");
        let _ = std::fs::remove_dir_all(&racine);
    }

    /// `+` vaut un espace dans une CHAÎNE DE REQUÊTE, jamais dans un chemin.
    #[test]
    fn le_decodage_de_chemin_ne_touche_pas_au_plus() {
        assert_eq!(percent_decode_chemin("a+b.js"), "a+b.js");
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(percent_decode_chemin("index-%2ecss"), "index-.css");
        assert_eq!(percent_decode_chemin("%2e%2e%2fetc"), "../etc");
        // Mêmes garanties que `percent_decode` : pas de panique sur un `%`
        // tronqué ni sur un caractère multi-octets.
        assert_eq!(percent_decode_chemin("%aé"), "%aé");
        assert_eq!(percent_decode_chemin("fin%"), "fin%");
    }

    // ── LA NÉGOCIATION DE CONTENU ──────────────────────────────────────────
    #[test]
    fn l_encodage_se_negocie_et_le_flux_n_est_jamais_degonfle() {
        assert_eq!(encodage_negocie(None, "application/javascript", 10_000), Encodage::Aucun);
        assert_eq!(
            encodage_negocie(Some("gzip, deflate, br, zstd"), "application/javascript", 10_000),
            Encodage::Gzip,
            "c'est l'en-tête que le navigateur envoie réellement"
        );
        assert_eq!(encodage_negocie(Some("deflate"), "application/json", 10_000), Encodage::Deflate);
        assert_eq!(encodage_negocie(Some("*"), "application/json", 10_000), Encodage::Gzip);
        // Sous le seuil, on ne dégonfle pas (un corps de 200 octets grossit).
        assert_eq!(encodage_negocie(Some("gzip"), "application/json", 200), Encodage::Aucun);
        // `q=0` est un REFUS, pas une acceptation.
        assert_eq!(encodage_negocie(Some("gzip;q=0"), "application/json", 10_000), Encodage::Aucun);
        assert_eq!(
            encodage_negocie(Some("gzip;q=0, deflate"), "application/json", 10_000),
            Encodage::Deflate
        );
        // LE FLUX : jamais, même quand le client le demande — un déflateur
        // tamponne, et un flux tamponné n'arrive qu'à la fin.
        assert_eq!(encodage_negocie(Some("gzip"), TYPE_SSE, 100_000), Encodage::Aucun);
        assert_eq!(encodage_negocie(Some("gzip, deflate"), "text/event-stream", 5_000), Encodage::Aucun);
    }

    #[test]
    fn le_corps_degonfle_se_relit_a_l_identique() {
        let corps = "obrain ".repeat(500);
        for encodage in [Encodage::Gzip, Encodage::Deflate] {
            let gonfle = compresser(corps.as_bytes(), encodage).expect("compression");
            assert!(gonfle.len() < corps.len() / 2, "aucun gain sur {encodage:?}");
            let mut relu = Vec::new();
            match encodage {
                Encodage::Gzip => {
                    flate2::read::GzDecoder::new(&gonfle[..]).read_to_end(&mut relu).unwrap();
                }
                Encodage::Deflate => {
                    flate2::read::ZlibDecoder::new(&gonfle[..]).read_to_end(&mut relu).unwrap();
                }
                Encodage::Aucun => unreachable!(),
            }
            assert_eq!(relu, corps.as_bytes(), "le corps doit se relire à l'identique");
        }
    }

    // ── LA REVALIDATION ────────────────────────────────────────────────────
    #[test]
    fn l_etag_se_derive_de_la_longueur_et_de_la_date() {
        let t = UNIX_EPOCH + Duration::from_secs(1_757_782_999);
        let un = etag_de(42, Some(t));
        assert_eq!(un, etag_de(42, Some(t)), "déterministe : deux appels, même étiquette");
        assert_ne!(un, etag_de(43, Some(t)), "la longueur compte");
        assert_ne!(un, etag_de(42, Some(t + Duration::from_secs(1))), "la date compte");
        assert!(un.starts_with('"') && un.ends_with('"'), "un ETag fort est entre guillemets");
    }

    #[test]
    fn if_none_match_declenche_304_et_ne_trompe_personne() {
        let etag = "\"1a-2b\"";
        assert!(revalidation_304(200, Some(etag), Some(etag)));
        assert!(revalidation_304(200, Some(etag), Some("W/\"1a-2b\"")), "comparaison faible");
        assert!(revalidation_304(200, Some(etag), Some("\"autre\", \"1a-2b\"")));
        assert!(revalidation_304(200, Some(etag), Some("*")));
        assert!(!revalidation_304(200, Some(etag), Some("\"autre\"")));
        assert!(!revalidation_304(200, Some(etag), None), "sans If-None-Match, pas de 304");
        assert!(!revalidation_304(200, None, Some(etag)), "sans ETag, pas de 304");
        assert!(!revalidation_304(404, Some(etag), Some(etag)), "un 404 ne devient pas un 304");
        // Deux étiquettes VOISINES ne doivent pas se confondre.
        assert!(!etag_correspond("\"1a-2b\"", "\"1a-2\""));
        assert!(!etag_correspond("\"1a-2\"", "\"1a-2b\""));
    }

    // ── LA DATE, OBLIGATOIRE ───────────────────────────────────────────────
    /// Les valeurs attendues viennent de `date -u`, pas d'un calcul refait par le
    /// même algorithme : sinon le test vérifierait l'erreur avec l'erreur.
    #[test]
    fn la_date_est_au_format_imf_fixdate() {
        assert_eq!(date_http_a(UNIX_EPOCH), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(
            date_http_a(UNIX_EPOCH + Duration::from_secs(1_757_782_999)),
            "Sat, 13 Sep 2025 17:03:19 GMT"
        );
        // 2024 est bissextile : le 29 février est le cas qu'une arithmétique
        // approximative rate d'un jour.
        assert_eq!(
            date_http_a(UNIX_EPOCH + Duration::from_secs(1_709_208_000)),
            "Thu, 29 Feb 2024 12:00:00 GMT"
        );
        assert_eq!(
            date_http_a(UNIX_EPOCH + Duration::from_secs(4_102_444_800)),
            "Fri, 01 Jan 2100 00:00:00 GMT"
        );
    }

    // ── LA TABLE DES RAISONS ───────────────────────────────────────────────
    /// La table en connaissait SIX, et tout le reste s'annonçait « Error ».
    #[test]
    fn tous_les_codes_emis_ont_une_raison() {
        for code in [
            200u16, 204, 206, 301, 302, 303, 304, 400, 401, 403, 404, 405, 408, 411, 413, 414, 429,
            431, 500, 501, 503, 505,
        ] {
            assert_ne!(raison_http(code), "Statut inconnu", "code {code} sans phrase de raison");
        }
        // Le repli DIT qu'il ne sait pas — il ne prétend plus « Error ».
        assert_eq!(raison_http(999), "Statut inconnu");
        assert_ne!(raison_http(999), "Error");
    }

    // ── LE TRI DES ERREURS DE TRAME ────────────────────────────────────────
    #[test]
    fn une_erreur_de_trame_ne_ressemble_pas_a_une_fin_de_connexion() {
        // Client parti sans rien dire : rien à répondre (et surtout PAS un 400
        // sur une socket muette, qui serait un mensonge).
        let mut vide: Vec<u8> = Vec::new();
        assert!(matches!(extraire_requete(&mut vide), Ok(Extraction::Incomplete)));
        // Des octets, mais pas de requête : 400 — jamais un silence.
        let mut partiel = b"GET / HTTP/1.1\r\nHost: x\r\n".to_vec();
        assert!(matches!(extraire_requete(&mut partiel), Ok(Extraction::Incomplete)));
        assert!(!partiel.is_empty(), "les octets reçus restent en attente");
    }
}
