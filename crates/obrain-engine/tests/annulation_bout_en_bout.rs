//! L'annulation coopérative, prouvée DE BOUT EN BOUT et PAR LE TEMPS.
//!
//! ── POURQUOI CE TEST NE MESURE PAS UN DRAPEAU ────────────────────────
//!
//! Un test qui vérifie qu'une requête annulée rend `Err` ne prouve rien : il
//! passerait aussi bien avec le délai d'avant ce travail, qui libérait
//! l'appelant en laissant la requête tourner. C'est exactement la panne
//! qu'on répare — « le délai libère l'APPELANT, il ne tue pas la requête ».
//!
//! On mesure donc le TEMPS, et on l'encadre par deux exécutions de contrôle
//! sur la MÊME base et la MÊME requête :
//!
//!   · COMPLÈTE          — personne n'annule. Établit `t_complet`, c'est-à-
//!                         dire que la requête est réellement longue.
//!   · ANNULÉE           — jeton armé en vol. Doit rendre `is_cancelled()`
//!                         ET revenir en une fraction de `t_complet`.
//!   · ANTI-VACUITÉ      — jeton armé AVANT le départ, mais portée
//!                         SUSPENDUE : le mécanisme est neutralisé sans
//!                         qu'une ligne de code change. La requête doit
//!                         aller au bout et coûter `t_complet`.
//!
//! Le troisième est celui qui donne sa valeur aux deux autres. Si le
//! mécanisme n'existait pas, le cas ANNULÉ se comporterait comme lui.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use web_time::Instant;

use obrain_common::types::Value;
use obrain_common::utils::cancel::{self, CancelToken};
use obrain_engine::ObrainDB;

/// Assez de nœuds ET assez de travail par nœud pour que le balayage + le tri se
/// comptent en centaines de millisecondes : sans cela, « s'est arrêté tôt »
/// n'est pas mesurable.
///
/// ── CE QUE LA GARDE EXIGE VRAIMENT, ET POURQUOI ELLE SE RECALIBRE ────────
///
/// Le chien de garde annule à `t_complet / 7`. Pour que « la requête s'est
/// arrêtée tôt » se distingue du bruit d'ordonnancement, ce point d'annulation
/// doit tomber LOIN devant la latence d'un réveil de fil (quelques ms).
///
/// Ce plancher n'est donc pas une exigence du moteur mais du PROTOCOLE : il
/// grandit mécaniquement quand le moteur s'accélère, et il a déjà changé trois
/// fois de support. Historique des mesures :
///   · 120 000 nœuds, tri d'entiers : 154 ms — trop court ;
///   · 400 000 nœuds, tri de chaînes : 513 ms attendu, tombé à **335,9 ms**
///     (0,84 µs/nœud) après la lecture par LOT des cellules de propriété ;
///   · 400 000 nœuds, référence mesurée CHAUDE : **164 ms** (0,41 µs/nœud) ;
///   · 1 500 000 nœuds : la référence tient ~1,3 s, MAIS le fixture coûte
///     **5,8 Go de disque par exécution** — mesuré le 2026-09-13, le volume a
///     rempli la racine (100 %) et le binaire de test a échoué SANS UN MOT.
///
/// Deux conséquences, tirées de ces mesures et non d'une intuition :
///
/// 1. **Le fixture ne grandit plus, faute de disque.** Agrandir le nombre de
///    nœuds est le levier le plus cher en disque : 1,5 M de nœuds = 5,8 Go. On
///    a donc essayé de payer en TRAVAIL PAR NŒUD plutôt qu'en nœuds (étiquettes
///    à long préfixe commun, voir [`PREFIXE_COMMUN`]) — mesuré **+9 %**, très
///    loin des 2× espérés. Le levier réel reste le nombre de lignes ; il est
///    refusé ici pour la raison de disque ci-dessus, et c'est le PROTOCOLE qui
///    s'adapte à la place.
/// 2. **La sonde d'armement s'adapte.** Un délai unique calé sur
///    `complet / 7` a échoué le 2026-09-13 non parce que le jeton était
///    ignoré, mais parce que l'exécution avait FINI avant l'armement : la
///    référence, mesurée à froid sur 1,5 M de nœuds, valait 7,38 s quand la
///    même exécution chaude en valait 0,91 s. Le délai est une SONDE, pas une
///    propriété du moteur : on la resserre tant qu'elle ne tombe pas dans
///    l'exécution.
const NOEUDS: usize = 400_000;

/// Le nombre d'octets IDENTIQUES en tête de chaque étiquette.
///
/// L'INTENTION était de rendre chaque comparaison du tri coûteuse (parcourir
/// 48 octets avant de trouver celui qui sépare). MESURE le 2026-09-13 :
/// **+9 % seulement** — 164 ms → 179 ms sur 400 000 nœuds. Le tri n'est donc
/// pas dominé par le comparateur, contrairement à ce que laissait croire le
/// commentaire d'origine sur « un tri de chaînes, plus cher qu'un tri
/// d'entiers ». C'est gardé parce que c'est un gain réel à coût de disque nul,
/// mais ce n'est PAS le levier : le levier, c'est le nombre de lignes, et il
/// coûte 5,8 Go par million et demi de nœuds (mesuré, voir plus haut).
const PREFIXE_COMMUN: usize = 48;

/// Le plancher de la référence, en millisecondes.
///
/// CE N'EST PLUS LA GARANTIE PRINCIPALE, et il faut le dire. À l'origine, ce
/// plancher était censé garantir que le point d'annulation (`duree / 7`) reste
/// loin de la gigue d'ordonnancement. Depuis, trois choses le remplacent, et
/// chacune est mesurée :
///   · l'ÉCHAUFFEMENT (voir plus bas) — c'est lui qui avait fait échouer le
///     test le 2026-09-13, pas la gigue : référence froide 7,38 s contre
///     exécution chaude 0,91 s ;
///   · la SONDE ADAPTATIVE (`SONDES`) — elle se resserre tant qu'elle ne tombe
///     pas dans l'exécution ;
///   · les MESURES PUBLIÉES avant les assertions — un échec se lit désormais
///     sans recompiler.
///
/// Il reste un garde-fou contre l'absurde : une requête de référence tombée
/// sous ~100 ms signifierait que le moteur a changé d'ordre de grandeur, et
/// qu'il faut reprendre le protocole plutôt que le laisser mesurer du bruit.
/// Mesuré à 400 000 nœuds : 179 ms (1,8× le plancher).
const PLANCHER_REFERENCE_MS: u64 = 100;

/// Les diviseurs successifs de la sonde d'armement, du plus large au plus
/// serré. Le premier qui tombe À L'INTÉRIEUR de l'exécution est retenu ; si
/// aucun ne tombe, c'est que le jeton armé en vol n'a jamais rien arrêté, et le
/// test le dit avec le journal des trois essais.
const SONDES: [u32; 2] = [7, 20];

/// La requête : un balayage de label COMPLET, une lecture de propriété par
/// nœud, puis un tri qui matérialise tout. Les trois familles de boucles
/// chaudes visées par la mission, dans un seul plan.
///
/// Le tri porte sur une CHAÎNE et non sur un entier : le décodage de valeur
/// et la comparaison lexicographique sont ce qui donne à la requête une durée
/// mesurable en secondes, donc un écart lisible.
const REQUETE: &str = "MATCH (n:Grain) RETURN n.etiquette ORDER BY n.etiquette";

/// La base est fabriquée UNE FOIS pour tout le binaire de test : trois
/// fixtures de 400 000 nœuds coûteraient plus cher que les mesures.
static BASE: std::sync::OnceLock<(tempfile::TempDir, std::path::PathBuf)> =
    std::sync::OnceLock::new();

fn base_partagee() -> &'static std::path::Path {
    let (_td, p) = BASE.get_or_init(|| {
        use obrain_core::graph::traits::GraphStoreMut;
        let td = tempfile::tempdir().unwrap();
        let chemin = td.path().join("base");
        let store = obrain_substrate::SubstrateStore::create(&chemin).expect("creation substrat");
        for i in 0..NOEUDS {
            let n = store.create_node(&["Grain"]);
            store.set_node_property(n, "v", Value::Int64(i as i64));
            // Toutes les étiquettes partagent `PREFIXE_COMMUN` octets : chaque
            // comparaison du tri les parcourt avant de trouver l'octet qui
            // sépare. C'est ce qui achète du TEMPS DE REQUÊTE sans acheter du
            // disque — le seul levier qui reste quand agrandir le fixture
            // remplit le volume.
            let prefixe = "g".repeat(PREFIXE_COMMUN);
            store.set_node_property(
                n,
                "etiquette",
                Value::String(format!("{prefixe}{:08}", NOEUDS - i).into()),
            );
        }
        drop(store);
        (td, chemin)
    });
    p
}

struct Mesure {
    duree: Duration,
    annulee: bool,
    lignes: usize,
    /// L'ERREUR, TELLE QU'ELLE EST. Sans elle, un échec de ce test ne dit que
    /// « ce n'est pas une annulation » — et l'on ne peut pas distinguer un
    /// moteur qui ignore le jeton, une annulation rendue sous une autre forme,
    /// ou une panne de tout autre genre (allocation, E/S). Un test qui échoue
    /// sans dire pourquoi coûte une recompilation à celui qui le lit.
    erreur: Option<String>,
}

fn executer(db: &ObrainDB, jeton: Option<&CancelToken>, suspendre: bool) -> Mesure {
    let t0 = Instant::now();
    let corps = || {
        // `suspend` est du code de PRODUCTION (c'est lui qui protège les
        // écritures). L'employer ici neutralise le mécanisme sans toucher
        // une ligne du moteur : c'est ce qui rend l'anti-vacuité honnête.
        let _neutralise = suspendre.then(cancel::suspend);
        db.session().execute(REQUETE)
    };
    let r = match jeton {
        Some(j) => cancel::scope(j, corps),
        None => corps(),
    };
    let duree = t0.elapsed();
    match r {
        Ok(qr) => Mesure {
            duree,
            annulee: false,
            lignes: qr.rows.len(),
            erreur: None,
        },
        Err(e) => Mesure {
            duree,
            annulee: e.is_cancelled(),
            lignes: 0,
            erreur: Some(format!("{e}")),
        },
    }
}

#[test]
fn une_requete_annulee_en_vol_s_arrete_vraiment() {
    let db = ObrainDB::open_substrate(base_partagee()).expect("ouverture");

    // ── 1. CONTRE-ÉPREUVE : personne n'annule, la requête aboutit ────
    //
    // ÉCHAUFFEMENT, ET C'EST LE CŒUR DU PROTOCOLE. La première traversée paie
    // les pages à froid ; le chien de garde, calibré sur ce temps FROID,
    // armerait alors le jeton APRÈS la fin de l'exécution CHAUDE. Le test
    // accuserait le moteur d'ignorer le jeton alors qu'il a simplement fini
    // avant l'ordre.
    //
    // Mesuré le 2026-09-13 sur 1 500 000 nœuds : référence FROIDE 7,38 s,
    // exécution CHAUDE 0,91 s — un facteur 8, très au-delà du septième qui
    // sépare l'armement de la fin. Une première exécution est donc jetée pour
    // que la référence et le cas annulé courent dans le MÊME état de cache :
    // c'est la seule façon d'attribuer au jeton ce qui lui revient.
    let _echauffement = executer(&db, None, false);
    let complet = executer(&db, None, false);
    // LES MESURES SONT PUBLIÉES AVANT LES ASSERTIONS. Une assertion qui tombe
    // emporte le journal avec elle : sans ces lignes, l'échec ne laisse que le
    // message de la garde, et il faut recompiler pour savoir ce qui s'est
    // réellement passé.
    eprintln!(
        "[1] contre-épreuve : {:?}, {} lignes, erreur={:?}",
        complet.duree, complet.lignes, complet.erreur
    );
    assert!(!complet.annulee, "sans annulation, la requete doit ABOUTIR");
    assert_eq!(
        complet.lignes, NOEUDS,
        "la contre-epreuve doit rendre TOUTES les lignes"
    );
    assert!(
        complet.duree >= Duration::from_millis(PLANCHER_REFERENCE_MS),
        "la requete de reference doit etre reellement longue, sinon « s est \
         arrete tot » n est pas mesurable — mesure : {:?} pour un plancher de \
         {} ms. Relever NOEUDS si le disque le permet, sinon alourdir la \
         requete : ne PAS baisser le plancher, cela perdrait ce que ce test \
         existe pour prouver.",
        complet.duree,
        PLANCHER_REFERENCE_MS
    );

    // ── 2. ANNULÉE EN VOL ────────────────────────────────────────────
    //
    // Le chien de garde arme le jeton APRÈS un délai. Ce délai est une SONDE :
    // trop large, elle mesure la fin de la requête au lieu de l'annulation.
    // On la resserre donc tant qu'elle ne tombe pas À L'INTÉRIEUR de
    // l'exécution. Le cran le plus large reste essayé en premier : le cas
    // nominal est inchangé, et les crans suivants n'existent que pour un
    // moteur devenu plus rapide que la sonde.
    let mut journal = String::new();
    let mut retenue: Option<(u32, Duration, Mesure)> = None;
    for diviseur in SONDES {
        let delai = complet.duree / diviseur;
        let jeton = CancelToken::new();
        let garde = jeton.clone();
        let parti = Arc::new(AtomicBool::new(false));
        let parti_c = Arc::clone(&parti);
        let chien = std::thread::spawn(move || {
            while !parti_c.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            std::thread::sleep(delai);
            garde.cancel();
        });
        parti.store(true, Ordering::Release);
        let m = executer(&db, Some(&jeton), false);
        chien.join().unwrap();
        let touche = m.annulee;
        journal.push_str(&format!(
            "\n  sonde /{diviseur} : armement à {delai:?} → {:?}, {} lignes, \
             annulée={touche}, erreur={:?}",
            m.duree, m.lignes, m.erreur
        ));
        if touche {
            retenue = Some((diviseur, delai, m));
            break;
        }
    }
    let Some((diviseur, delai, annule)) = retenue else {
        panic!(
            "AUCUNE sonde n a atteint l execution : le jeton armé en vol n a \
             jamais rien arrêté.\n  référence (chaude) : {:?}{journal}\n  Si \
             toutes les sondes ont rendu un résultat COMPLET, l exécution \
             finit avant l armement — resserrer SONDES ou alourdir la requête. \
             Si l une a rendu une AUTRE erreur, c est le jeton qui est ignoré.",
            complet.duree
        );
    };
    eprintln!(
        "[2] annulée en vol : sonde /{diviseur} (armement à {delai:?}), {:?}, \
         {} lignes, erreur={:?} — référence chaude {:?}",
        annule.duree, annule.lignes, annule.erreur, complet.duree
    );
    assert!(
        annule.duree < complet.duree / 2,
        "L ANNULATION N A PAS ARRETE LE TRAVAIL : {:?} contre {:?} sans \
         annulation (sonde /{diviseur}). C est exactement la panne d origine — \
         l appelant est libere, la requete continue.",
        annule.duree,
        complet.duree
    );

    // ── 3. ANTI-VACUITÉ : mécanisme neutralisé ───────────────────────
    //
    // Jeton arme AVANT le depart, mais portee suspendue : les points de
    // controle ne voient rien. Si la requete s arretait quand meme, le
    // cas 2 mesurerait autre chose que le mecanisme.
    let arme = CancelToken::new();
    arme.cancel();
    let neutralise = executer(&db, Some(&arme), true);
    eprintln!(
        "[3] anti-vacuité  : {:?}, {} lignes, erreur={:?}",
        neutralise.duree, neutralise.lignes, neutralise.erreur
    );
    assert!(
        !neutralise.annulee,
        "ANTI-VACUITE : mecanisme neutralise, la requete ne doit PAS \
         s interrompre"
    );
    assert_eq!(
        neutralise.lignes, NOEUDS,
        "ANTI-VACUITE : mecanisme neutralise, la requete doit aller au bout"
    );
    assert!(
        neutralise.duree > annule.duree * 2,
        "ANTI-VACUITE : mecanisme neutralise, le cout doit revenir a celui \
         d une execution complete ({:?} contre {:?} annulee)",
        neutralise.duree,
        annule.duree
    );

    eprintln!(
        "annulation bout en bout — complet {:?} / annule {:?} / neutralise {:?}",
        complet.duree, annule.duree, neutralise.duree
    );
}

#[test]
fn un_jeton_arme_avant_le_depart_ne_lance_aucun_balayage() {
    // Le cas de la requete DEJA condamnee : relancee alors que la
    // precedente vient d expirer. Le controle place AVANT `load_batch` doit
    // lui epargner le balayage integral, pas seulement l interrompre.
    let db = ObrainDB::open_substrate(base_partagee()).expect("ouverture");

    let reference = executer(&db, None, false);

    let jeton = CancelToken::new();
    jeton.cancel();
    let m = executer(&db, Some(&jeton), false);
    assert!(m.annulee, "doit rendre une annulation");
    assert!(
        m.duree < reference.duree / 5,
        "une requete deja condamnee ne doit engager AUCUN balayage : {:?} \
         contre {:?}",
        m.duree,
        reference.duree
    );
}

#[test]
fn une_ecriture_ignore_le_jeton() {
    // FRONTIERE DE SURETE : les ecritures ne sont pas annulables, par
    // decision explicite (voir `cancel::suspend`). Ce test fige la
    // decision — si quelqu un rend un jour les ecritures interruptibles, il
    // devra passer par ici et assumer la demonstration qui va avec.
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("base");
    let db = ObrainDB::open_substrate(&chemin).expect("ouverture");

    let jeton = CancelToken::new();
    jeton.cancel();
    let r = cancel::scope(&jeton, || {
        db.session()
            .execute("INSERT (:Ecrit {k: 1}), (:Ecrit {k: 2})")
    });
    assert!(
        r.is_ok(),
        "une ecriture doit ignorer le jeton : {:?}",
        r.err()
    );

    // Et l ecriture a bien eu lieu — l annulation n a rien tronque.
    let compte = db
        .session()
        .execute("MATCH (n:Ecrit) RETURN count(n)")
        .expect("relecture");
    assert_eq!(compte.rows.len(), 1);
    assert_eq!(compte.rows[0][0], Value::Int64(2));
}
