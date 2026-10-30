//! # `bench_intention_contient` — l'INTENTION contre le TEXTE, en processus
//!
//! Un outil qui veut savoir « qui contient `obrain-iam` ? » a deux façons de
//! poser la question au moteur, et ce banc mesure les deux DANS LE MÊME
//! PROCESSUS, sur le MÊME store, avec le MÊME texte de requête :
//!
//! ```text
//!   TEXTE      : session.execute("MATCH (n) WHERE n.qualified_name CONTAINS
//!                                'obrain-iam' RETURN count(n)")
//!   INTENTION  : db.contient("qualified_name", "obrain-iam")
//! ```
//!
//! ── CE QUE LE BANC VENTILE, ET POURQUOI C'EST LA PREMIÈRE CHOSE À FAIRE ──
//!
//! Optimiser sans ventiler, c'est optimiser à l'aveugle. Le banc mesure donc
//! séparément :
//!
//!   · ANALYSE       — `translate_gql(texte)` : lexer, parseur, plan logique.
//!                     Payée au premier appel d'un texte, sautée ensuite par le
//!                     cache de plans logiques ;
//!   · SONDE D'INDEX — `noeuds_par_predicat_texte` seule, c'est-à-dire le
//!                     travail que les DEUX chemins PARTAGENT. Sans cette ligne,
//!                     on attribuerait au « texte » ce qui appartient à l'index ;
//!   · TEXTE         — `session.execute(texte)` : analyse (en cache),
//!                     planification PHYSIQUE — jamais en cache, et c'est là que
//!                     le planificateur sonde l'index pour `CONTAINS` —,
//!                     opérateurs, agrégat. `OBRAIN_PHASES=1` sépare la
//!                     planification de l'exécution sur la sortie d'erreur ;
//!   · INTENTION     — `db.contient`, qui ne fait ni analyse ni planification.
//!
//! ── LES TROIS PIÈGES DE MESURE QUE CE BANC DÉSAMORCE ──
//!
//! 1. LE PREMIER TOUR N'EST COMPARABLE QUE DANS UN PROCESSUS NEUF PAR CHEMIN.
//!    La construction de l'index de trigrammes se paie une fois par processus et
//!    par propriété : le chemin qui passe le second trouve l'index chaud. Le mode
//!    `gql` / `intention` ne joue donc qu'un seul chemin — un processus par
//!    chemin, côté shell. Mesuré SANS cette précaution : « 120 ms contre 1,2 ms au
//!    premier tour », alors que les 120 ms étaient la CONSTRUCTION de l'index,
//!    payée par le premier appelant quel qu'il soit.
//! 2. LE MINIMUM, PAS LA MOYENNE. Un minimum sur 200 tours est le seul chiffre qui
//!    ne soit pas pollué par l'ordonnanceur ; la médiane est donnée à côté pour
//!    montrer la dispersion.
//! 3. LA PREUVE, PAS LA CONFIANCE. Les comptes sont comparés (`count(n)` d'un
//!    côté, `rows.len()` de l'autre) ET les lignes : `RETURN id(n)` est exécuté
//!    UNE fois et ses identifiants triés sont comparés à ceux de la primitive.
//!    Deux routes qui rendraient le même compte avec des nœuds différents
//!    seraient un bug que le compte seul ne verrait pas.
//!
//! ## Usage
//!
//! ```text
//! cargo run --release -p obrain-engine --example bench_intention_contient \
//!     -- <STORE> [TOURS] [PROPRIETE] [AIGUILLE] [gql|intention|les-deux]
//!
//! OBRAIN_PHASES=1            # ventile planification/exécution (sortie d'erreur)
//! OBRAIN_LECTURE_SEULE=1     # ouvre en verrou PARTAGÉ (store vivant)
//! ```

use web_time::Instant;
use std::time::Duration;

use obrain_common::types::{PropertyKey, Value};
use obrain_core::graph::GraphStore;
use obrain_core::graph::traits::PredicatTexte;
use obrain_engine::ObrainDB;

/// Statistiques d'une boucle de mesures.
struct Stats {
    premier: Duration,
    min: Duration,
    mediane: Duration,
}

impl Stats {
    fn new(mut tours: Vec<Duration>, premier: Duration) -> Self {
        tours.sort();
        let min = tours.first().copied().unwrap_or(Duration::ZERO);
        let mediane = tours.get(tours.len() / 2).copied().unwrap_or(Duration::ZERO);
        Self { premier, min, mediane }
    }

    fn ms(d: Duration) -> f64 {
        d.as_secs_f64() * 1e3
    }

    fn ligne(&self, nom: &str, extra: &str) {
        println!(
            "  {nom:<30} {:>10.3} {:>10.3} {:>10.3} {extra}",
            Self::ms(self.premier),
            Self::ms(self.min),
            Self::ms(self.mediane),
        );
    }
}

/// Mesure `tours` tours de `f`, plus le PREMIER tour à part.
fn mesurer<T>(tours: usize, mut f: impl FnMut() -> T) -> (Stats, T) {
    let t = Instant::now();
    let premiere_valeur = f();
    let premier = t.elapsed();
    let mut durees: Vec<Duration> = Vec::with_capacity(tours);
    for _ in 0..tours {
        let t = Instant::now();
        let v = f();
        durees.push(t.elapsed());
        std::hint::black_box(&v);
    }
    (Stats::new(durees, premier), premiere_valeur)
}

fn ids_primitive(db: &ObrainDB, propriete: &str, aiguille: &str) -> Vec<i64> {
    db.contient(propriete, aiguille)
        .expect("primitive")
        .rows
        .iter()
        .map(|l| {
            l.first()
                .and_then(Value::as_int64)
                .unwrap_or_else(|| panic!("identifiant attendu, recu {:?}", l.first()))
        })
        .collect()
}

fn ids_gql(db: &ObrainDB, texte: &str) -> Vec<i64> {
    db.session()
        .execute(texte)
        .expect("requete GQL")
        .rows
        .iter()
        .map(|l| {
            l.first()
                .and_then(Value::as_int64)
                .unwrap_or_else(|| panic!("identifiant attendu, recu {:?}", l.first()))
        })
        .collect()
}

fn compte_gql(db: &ObrainDB, texte: &str) -> i64 {
    db.session()
        .execute(texte)
        .expect("count GQL")
        .rows
        .first()
        .and_then(|l| l.first())
        .and_then(Value::as_int64)
        .expect("count(n) attendu")
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(store_path) = args.next() else {
        eprintln!(
            "usage: bench_intention_contient <STORE> [TOURS] [PROPRIETE] [AIGUILLE] [gql|intention|les-deux]"
        );
        std::process::exit(2);
    };
    let tours: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(200);
    let propriete = args.next().unwrap_or_else(|| "qualified_name".to_string());
    let aiguille = args.next().unwrap_or_else(|| "obrain-iam".to_string());
    // ── POURQUOI CE 5e ARGUMENT EXISTE ──────────────────────────────────────
    //
    // Le PREMIER TOUR n'est comparable entre les deux chemins que dans un
    // processus NEUF chacun : la construction de l'index de trigrammes (et la
    // lecture de son fichier persisté) se paie UNE fois par processus et par
    // propriété, et le chemin qui passe en premier l'emporte dans sa colonne.
    // Mesurer les deux dans le même processus, c'est offrir un index chaud au
    // second — constaté, et c'est exactement le genre de chiffre qui fait
    // annoncer « ×100 au premier tour ».
    let mode = args.next().unwrap_or_else(|| "les-deux".to_string());
    assert!(
        matches!(mode.as_str(), "gql" | "intention" | "les-deux"),
        "mode inconnu : {mode} (attendu gql|intention|les-deux)"
    );
    assert!(
        !aiguille.contains('\'') && !propriete.contains('\''),
        "l'aiguille et la propriete ne doivent pas porter d'apostrophe (le banc les inline dans le texte)"
    );

    let lecture_seule = std::env::var("OBRAIN_LECTURE_SEULE").is_ok_and(|v| v == "1");
    let db = if lecture_seule {
        ObrainDB::open_read_only(&store_path).expect("ouverture en lecture seule")
    } else {
        ObrainDB::open_substrate(&store_path).expect("ouverture du store")
    };

    // UN SEUL texte de requête pour la boucle GQL. Le second texte (`RETURN
    // id(n)`) n'est PAS mesuré : il ne sert qu'à la comparaison des lignes.
    let texte_count = format!("MATCH (n) WHERE n.{propriete} CONTAINS '{aiguille}' RETURN count(n)");
    let texte_ids = format!("MATCH (n) WHERE n.{propriete} CONTAINS '{aiguille}' RETURN id(n)");

    println!("store    : {store_path}");
    println!(
        "ouverture: {}",
        if lecture_seule { "LECTURE SEULE (verrou partagé)" } else { "écriture (copie)" }
    );
    println!("propriete: {propriete}   aiguille: {aiguille:?}   mode: {mode}");
    println!("tours    : {tours}");
    println!("TEXTE    : {texte_count}");
    println!("INTENTION: db.contient({propriete:?}, {aiguille:?})");
    println!();

    // ═════════════ VENTILATION DE L'EXISTANT ═════════════
    let mut analyse: Option<Stats> = None;
    let mut sonde: Option<Stats> = None;
    let mut gql: Option<Stats> = None;
    let mut prim: Option<Stats> = None;
    let mut compte: i64 = 0;
    let ids_prim: Vec<i64>;

    if mode == "intention" {
        let (s, v) = mesurer(tours, || ids_primitive(&db, &propriete, &aiguille));
        prim = Some(s);
        ids_prim = v;
    } else {
        // ANALYSE : le texte → plan logique, sans rien exécuter.
        analyse = Some(
            mesurer(tours, || {
                obrain_engine::query::translate_gql(&texte_ids).expect("traduction")
            })
            .0,
        );

        // SONDE SEULE : le travail PARTAGÉ par les deux chemins. C'est elle qui
        // dit si le gain possible est de l'ordre de la milliseconde (sonde) ou de
        // la microseconde (reste du chemin).
        let store: &dyn GraphStore = db.store().as_ref();
        let cle = PropertyKey::new(&propriete);
        sonde = Some(
            mesurer(tours, || {
                store.noeuds_par_predicat_texte(&cle, &PredicatTexte::Contient(&aiguille))
            })
            .0,
        );

        // TEXTE : le premier tour paie l'analyse, la planification physique et —
        // le cas échéant — la construction de l'index de trigrammes.
        let (s, c) = mesurer(tours, || compte_gql(&db, &texte_count));
        gql = Some(s);
        compte = c;
        assert_eq!(
            compte_gql(&db, &texte_count),
            compte,
            "le compte GQL a change en cours de boucle"
        );

        if mode == "gql" {
            ids_prim = Vec::new();
        } else {
            let (s, v) = mesurer(tours, || ids_primitive(&db, &propriete, &aiguille));
            prim = Some(s);
            ids_prim = v;
        }
    }

    println!("  phase (ms)                       1er tour        min    médiane");
    if let Some(a) = &analyse {
        a.ligne("analyse (translate_gql)", "");
    }
    if let Some(s) = &sonde {
        s.ligne("sonde d'index (substrat)", "partagée");
    }
    if let Some(g) = &gql {
        g.ligne("TEXTE (session.execute)", &format!("lignes={compte}"));
    }
    if let Some(p) = &prim {
        p.ligne("INTENTION (db.contient)", &format!("lignes={}", ids_prim.len()));
    }
    println!();
    println!("  « analyse »  = lexer/parseur/plan logique, SEUL (aucune exécution) ;");
    println!("  « sonde »    = l'accès à l'index de sous-chaînes, SEUL — les DEUX chemins le paient ;");
    println!("  « TEXTE » et « INTENTION » sont les appels complets, premiers tours compris.");
    println!("  Pour séparer planification physique et exécution de la ligne TEXTE :");
    println!("      OBRAIN_PHASES=1 … --example bench_intention_contient -- <STORE> <TOURS> …");

    if mode != "les-deux" {
        println!();
        println!(
            "  (mode « {mode} » : un seul chemin est joué — c'est ce qui rend les PREMIERS tours"
        );
        println!(
            "   comparables entre deux processus neufs, l'index de trigrammes se payant une fois par processus.)"
        );
        return;
    }

    // ═════════════ LA PREUVE : MÊMES COMPTES, MÊMES LIGNES ═════════════

    let ids_ref = ids_gql(&db, &texte_ids);
    let refs: std::collections::BTreeSet<i64> = ids_ref.iter().copied().collect();
    let notres: std::collections::BTreeSet<i64> = ids_prim.iter().copied().collect();
    let identiques = refs == notres;

    println!();
    println!("  comptes : count(n) = {compte}   |   primitive = {}", ids_prim.len());
    println!(
        "  lignes  : id(n) = {} distincts   |   primitive = {} distincts",
        refs.len(),
        notres.len()
    );
    println!(
        "  VERDICT : comptes {} — lignes {} (ensemble d'identifiants comparé, pas seulement le nombre)",
        if compte as usize == ids_prim.len() { "IDENTIQUES" } else { "DIFFERENTS" },
        if identiques { "IDENTIQUES" } else { "DIFFERENTES" }
    );
    if !identiques {
        let manquants: Vec<&i64> = refs.difference(&notres).take(5).collect();
        let en_trop: Vec<&i64> = notres.difference(&refs).take(5).collect();
        println!("  manquants (≤5) : {manquants:?}");
        println!("  en trop   (≤5) : {en_trop:?}");
        std::process::exit(1);
    }

    if let (Some(g), Some(p)) = (&gql, &prim) {
        let gain = Stats::ms(g.min) - Stats::ms(p.min);
        println!();
        println!(
            "  régime établi : TEXTE {:.3} ms → INTENTION {:.3} ms  ({:+.3} ms, ×{:.2})",
            Stats::ms(g.min),
            Stats::ms(p.min),
            -gain,
            Stats::ms(g.min) / Stats::ms(p.min)
        );
        if let Some(s) = &sonde {
            println!(
                "  dont partagé  : sonde d'index {:.3} ms — le gain PLAFOND est le reste ({:.3} ms)",
                Stats::ms(s.min),
                Stats::ms(g.min) - Stats::ms(s.min)
            );
        }
    }
}
