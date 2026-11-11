//! Recherche par égalité À L'ÉCHELLE — 1 M, 5 M, 10 M de nœuds.
//!
//! POURQUOI CE BANC EXISTE. `resolution_selective` mesure 5 000 nœuds : il
//! prouve que l'ordre des conditions ne coûte plus rien, il ne dit rien de ce
//! que devient une recherche indexée quand la base ne tient plus dans les
//! caches. La barre du projet est la CENTAINE DE NANOSECONDES sur des
//! MILLIONS de nœuds, indépendamment de N ; ce banc donne, pour chaque
//! chemin, la médiane ET le p99, et se relance à plusieurs tailles pour
//! montrer l'évolution avec N.
//!
//! Ce n'est PAS un banc criterion : criterion rend une moyenne et son
//! intervalle, pas un p99, et il ne sait pas mesurer une ouverture ni une
//! PREMIÈRE requête (qui n'a lieu qu'une fois). Ici chaque opération est
//! chronométrée seule (`Instant`, ~20-40 ns de surcoût sur cette machine,
//! INCLUS dans les chiffres) et les quantiles sont lus sur l'échantillon.
//!
//! DEUX TEMPS, DEUX PROCESSUS LOGIQUES
//!
//! * `construire` — crée la base SANS index (chemin d'import : ni DashMap ni
//!   index en RAM), la ferme, puis déclare chaque index dans une ouverture
//!   distincte. La mémoire résidente reste bornée par UN index à la fois.
//! * `mesurer` — rouvre la base (les index sont alors servis par la zone
//!   persistée, comme dans un service qui redémarre) et mesure.
//!
//! La base vit sous `ECHELLE_BASE` (obligatoire) ; ce banc ne la supprime
//! pas lui-même — l'appelant le fait après la mesure.
//!
//! ```bash
//! ECHELLE_BASE=/chemin/base ECHELLE_N=1000000 \
//!   cargo bench -p obrain-substrate --bench echelle_resolution -- construire
//! ECHELLE_BASE=/chemin/base ECHELLE_N=1000000 \
//!   cargo bench -p obrain-substrate --bench echelle_resolution -- mesurer
//! ```
//!
//! LES COLONNES
//!
//! | clé | valeur du nœud `i`                 | cardinalité |
//! |-----|------------------------------------|-------------|
//! | `a` | `Int64(i % 2)`                     | 2           |
//! | `c` | `Int64(i % 1000)`                  | 1 000       |
//! | `b` | `Int64(i)`                         | unique      |
//! | `f` | `Float64(i)` (flottant ENTIER)     | unique      |
//! | `d` | chaîne RFC3339, `i` secondes, `Z`  | unique      |
//! | `s` | `"ligne-0000042"`                  | unique      |

use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use obrain_common::types::Value;
use obrain_core::graph::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;

const CLES: [&str; 6] = ["a", "c", "b", "f", "d", "s"];

/// Période des instants écrits en forme LONGUE (> 256 octets, donc routés
/// en colonne blob) sur la clé `d` : un nœud sur `ECHELLE_BLOBS_D` (0 = aucun).
/// Ces nœuds portent le MÊME instant qu'en forme courte, la clé `d` a donc une
/// colonne blob ET des valeurs indexées ordinaires — le cas de la revue 4, A2.
fn periode_blobs_d() -> i64 {
    std::env::var("ECHELLE_BLOBS_D")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// Écriture LONGUE (fraction de 300 zéros) de l'instant `secondes`.
fn rfc3339_longue(secondes: i64) -> String {
    let courte = rfc3339(secondes, 0);
    format!("{}.{}Z", courte.trim_end_matches('Z'), "0".repeat(300))
}

fn n_noeuds() -> i64 {
    std::env::var("ECHELLE_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1_000_000)
}

fn base() -> PathBuf {
    PathBuf::from(std::env::var("ECHELLE_BASE").expect("ECHELLE_BASE : chemin de la base de banc"))
}

/// Date civile (année, mois, jour) d'un nombre de jours depuis 1970-01-01 —
/// algorithme de Howard Hinnant, sans dépendance.
fn civil(jours: i64) -> (i64, i64, i64) {
    let z = jours + 719_468;
    let ere = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let j = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + ere * 400 + i64::from(m <= 2), m, j)
}

/// Chaîne RFC3339 de l'instant `secondes` (depuis l'époque Unix), écrite
/// dans le fuseau `decalage_h` : même INSTANT, écriture différente.
fn rfc3339(secondes: i64, decalage_h: i64) -> String {
    let local = secondes + decalage_h * 3_600;
    let (a, m, j) = civil(local.div_euclid(86_400));
    let r = local.rem_euclid(86_400);
    let suffixe = if decalage_h == 0 {
        "Z".to_string()
    } else {
        format!(
            "{}{:02}:00",
            if decalage_h < 0 { '-' } else { '+' },
            decalage_h.abs()
        )
    };
    format!(
        "{a:04}-{m:02}-{j:02}T{:02}:{:02}:{:02}{suffixe}",
        r / 3_600,
        r % 3_600 / 60,
        r % 60
    )
}

/// Origine des dates du banc : 2024-01-01T00:00:00Z.
const EPOQUE: i64 = 1_704_067_200;

fn chaine_s(i: i64) -> Value {
    Value::String(format!("ligne-{i:08}").into())
}

/// Mémoire résidente MAXIMALE du processus, en Mo (macOS : octets ; Linux :
/// kilo-octets).
#[allow(unsafe_code)]
fn rss_max_mo() -> u64 {
    // SAFETY: `getrusage` écrit dans la structure fournie et rien d'autre.
    let mut u: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: pointeur valide vers une `rusage` initialisée à zéro.
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
    let brut = u.ru_maxrss as u64;
    if cfg!(target_os = "macos") {
        brut / (1024 * 1024)
    } else {
        brut / 1024
    }
}

fn charge() -> String {
    std::process::Command::new("uptime")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| {
            s.rsplit("load average")
                .next()
                .map(|x| x.trim().to_string())
        })
        .unwrap_or_default()
}

fn construire() {
    let n = n_noeuds();
    let chemin = base();
    let t = Instant::now();
    {
        let s = SubstrateStore::create(&chemin).expect("création");
        for i in 0..n {
            let id = s.create_node(&["Ligne"]);
            s.set_node_property(id, "a", Value::Int64(i % 2));
            s.set_node_property(id, "c", Value::Int64(i % 1000));
            s.set_node_property(id, "b", Value::Int64(i));
            #[allow(clippy::cast_precision_loss)]
            s.set_node_property(id, "f", Value::Float64(i as f64));
            let p = periode_blobs_d();
            let d = if p > 0 && i % p == 0 {
                rfc3339_longue(EPOQUE + i)
            } else {
                rfc3339(EPOQUE + i, 0)
            };
            s.set_node_property(id, "d", Value::String(d.into()));
            s.set_node_property(id, "s", chaine_s(i));
        }
        s.flush().expect("flush");
        s.close().expect("fermeture");
    }
    println!(
        "[construire] N={n} nœuds écrits en {:.1} s · RSS max {} Mo",
        t.elapsed().as_secs_f64(),
        rss_max_mo()
    );
    for cle in CLES {
        let t = Instant::now();
        let s = SubstrateStore::open(&chemin).expect("ouverture");
        s.create_property_index(cle);
        s.flush().expect("flush");
        s.close().expect("fermeture");
        println!(
            "[construire] index `{cle}` : {:.2} s · RSS max {} Mo",
            t.elapsed().as_secs_f64(),
            rss_max_mo()
        );
    }
}

/// Générateur congruentiel : des valeurs étalées sur `[0, n)`, reproductibles.
struct Tirage(u64);
impl Tirage {
    fn suivant(&mut self, n: i64) -> i64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) % n as u64) as i64
    }
}

/// Exécute `op` jusqu'à `max_ops` fois ou `budget`, chronomètre CHAQUE appel,
/// et imprime médiane / p99 / max. `attendu` contrôle le nombre de résultats.
fn mesurer_scenario(
    nom: &str,
    n: i64,
    max_ops: usize,
    budget: Duration,
    mut op: impl FnMut(i64) -> usize,
    attendu: usize,
) {
    let mut tirage = Tirage(0x9E37_79B9_7F4A_7C15);
    // Échauffement court et BORNÉ EN TEMPS, hors mesure (un chemin qui
    // balaie toute la base ne doit pas y passer une minute).
    let chauffe = Instant::now();
    for _ in 0..max_ops.min(30_000) {
        black_box(op(tirage.suivant(n)));
        if chauffe.elapsed() > Duration::from_millis(1_500) {
            break;
        }
    }
    let mut durees: Vec<u64> = Vec::with_capacity(max_ops);
    let mut faux = 0usize;
    let debut = Instant::now();
    while durees.len() < max_ops && (debut.elapsed() < budget || durees.len() < 5) {
        let i = tirage.suivant(n);
        let t = Instant::now();
        let rendus = op(i);
        durees.push(t.elapsed().as_nanos() as u64);
        // Un résultat FAUX se compte et s'affiche : il ne doit ni passer
        // inaperçu, ni interrompre la mesure des autres chemins.
        if rendus != attendu {
            faux += 1;
        }
    }
    durees.sort_unstable();
    let q = |p: f64| durees[((durees.len() - 1) as f64 * p) as usize];
    println!(
        "[mesure] {nom:<34} ops={:>7}  médiane={:>10} ns  p99={:>10} ns  max={:>11} ns{}",
        durees.len(),
        q(0.5),
        q(0.99),
        durees[durees.len() - 1],
        if faux == 0 {
            String::new()
        } else {
            format!("  ⚠ RÉSULTAT FAUX sur {faux} appels (attendu {attendu})")
        }
    );
}

fn mesurer() {
    let n = n_noeuds();
    let chemin = base();
    // DEUX CHEMINS D'INDEX, mesurés séparément :
    //  · `zone` (défaut) — la base rouverte telle quelle : les index sont
    //    servis par la zone PERSISTÉE, rien n'est rebâti ;
    //  · `ram` — le fichier de zone est retiré avant l'ouverture : les index
    //    interrogés sont bâtis en mémoire (carte de hachage), c'est l'état
    //    d'un processus qui a construit ses index dans la session. Seules les
    //    colonnes entières (`a`, `b`, `c`) sont alors interrogées, pour
    //    borner la mémoire résidente.
    let mode_ram = std::env::var("ECHELLE_CHEMIN").is_ok_and(|v| v == "ram");
    // Mode `v1` : la zone persistée est ramenée à la version 1 (écrite avant
    // la clé canonique) — version et CRC de la racine réécrits, séries
    // intactes. C'est l'état d'une base existante ouverte par le binaire de
    // cette PR : la zone doit être LUE, pas jetée, et son complément bâti
    // hors du chemin des requêtes.
    if std::env::var("ECHELLE_CHEMIN").is_ok_and(|v| v == "v1") {
        let fichier = chemin.join("substrate.props.eq.idx.v2");
        let mut o = std::fs::read(&fichier).expect("zone d'égalité");
        o[4..8].copy_from_slice(&1u32.to_le_bytes());
        o[8..12].copy_from_slice(&0u32.to_le_bytes());
        let crc = crc32fast::hash(&o[..4096]);
        o[8..12].copy_from_slice(&crc.to_le_bytes());
        std::fs::write(&fichier, &o).expect("réécriture de la version");
    }
    if mode_ram {
        let _ = std::fs::remove_file(chemin.join("substrate.props.eq.idx.v2"));
    }
    println!(
        "[mesure] N={n} · chemin d'index : {} · charge au départ : {}",
        if mode_ram {
            "RAM"
        } else if std::env::var("ECHELLE_CHEMIN").is_ok_and(|v| v == "v1") {
            "ZONE persistée de version 1 (migration)"
        } else {
            "ZONE persistée"
        },
        charge()
    );

    // ── Ouverture : 5 fois, la base étant propre (flush + close). ──
    let mut ouvertures: Vec<u128> = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let s = SubstrateStore::open(&chemin).expect("ouverture");
        ouvertures.push(t.elapsed().as_micros());
        drop(s);
    }
    ouvertures.sort_unstable();
    println!(
        "[mesure] ouverture                          médiane={} µs  min={} µs  max={} µs",
        ouvertures[2], ouvertures[0], ouvertures[4]
    );

    // ── Première requête après réouverture, par chemin. ──
    let s = std::sync::Arc::new(SubstrateStore::open(&chemin).expect("ouverture"));
    let milieu = n / 2;
    // ── MIGRATION : une zone d'égalité HÉRITÉE (base écrite par un binaire
    // d'avant la clé canonique) ou un index déclaré non couvert. Le moteur
    // (`ObrainDB`) lance alors `preparer_index_egalite` sur un fil
    // d'arrière-plan juste après l'ouverture ; on fait de même, et on mesure
    // ce que voit une requête qui arrive PENDANT la passe.
    if s.index_egalite_a_preparer() {
        let t0 = Instant::now();
        let fil = {
            let s = std::sync::Arc::clone(&s);
            std::thread::spawn(move || {
                let t = Instant::now();
                s.preparer_index_egalite();
                t.elapsed()
            })
        };
        let t = Instant::now();
        let rendus = s.find_nodes_by_property("s", &chaine_s(milieu)).len();
        println!(
            "[migration] chaîne ordinaire PENDANT la passe (servie par la zone héritée) : {:>10} ns  ({rendus} résultat)",
            t.elapsed().as_nanos()
        );
        let t = Instant::now();
        let rendus = s.find_nodes_by_property("b", &Value::Int64(milieu)).len();
        println!(
            "[migration] entier PENDANT la passe (attend le complément)                : {:>10} ns  ({rendus} résultat)",
            t.elapsed().as_nanos()
        );
        let passe = fil.join().expect("fil de préparation");
        println!(
            "[migration] passe d'arrière-plan : {:.3} s (du lancement à la fin : {:.3} s)",
            passe.as_secs_f64(),
            t0.elapsed().as_secs_f64()
        );
    }
    #[allow(clippy::type_complexity)] // tableau de scénarios de banc : lisible tel quel
    let premieres: [(&str, Box<dyn Fn() -> usize>); 4] = [
        (
            "mono entier (b)",
            Box::new(|| s.find_nodes_by_property("b", &Value::Int64(milieu)).len()),
        ),
        (
            "mono chaîne (s)",
            Box::new(|| s.find_nodes_by_property("s", &chaine_s(milieu)).len()),
        ),
        (
            "mono datetime (d)",
            Box::new(|| {
                s.find_nodes_by_property("d", &Value::String(rfc3339(EPOQUE + milieu, 2).into()))
                    .len()
            }),
        ),
        (
            "deux conditions (a, b)",
            Box::new(|| {
                s.find_nodes_by_properties(&[
                    ("a", Value::Int64(milieu % 2)),
                    ("b", Value::Int64(milieu)),
                ])
                .len()
            }),
        ),
    ];
    for (nom, f) in &premieres {
        if mode_ram && (nom.contains("(s)") || nom.contains("(d)")) {
            continue;
        }
        let t = Instant::now();
        let rendus = f();
        println!(
            "[mesure] 1re requête — {nom:<22} {:>10} ns  ({rendus} résultat)",
            t.elapsed().as_nanos()
        );
    }
    drop(premieres);

    let max_ops = 200_000usize;
    let budget = Duration::from_secs(4);
    #[allow(clippy::cast_precision_loss, clippy::type_complexity)]
    let scenarios: Vec<(&str, Box<dyn FnMut(i64) -> usize + '_>, usize)> = vec![
        (
            "mono b = Int64 (unique)",
            Box::new(|i| s.find_nodes_by_property("b", &Value::Int64(i)).len()),
            1,
        ),
        (
            "mono s = chaîne (unique)",
            Box::new(|i| s.find_nodes_by_property("s", &chaine_s(i)).len()),
            1,
        ),
        (
            "mono b = Int64 ABSENT",
            Box::new(|i| s.find_nodes_by_property("b", &Value::Int64(n + i)).len()),
            0,
        ),
        (
            "mono b = Float64 entier",
            Box::new(|i| {
                s.find_nodes_by_property("b", &Value::Float64(i as f64))
                    .len()
            }),
            1,
        ),
        (
            "mono f(flottant) = Int64",
            Box::new(|i| s.find_nodes_by_property("f", &Value::Int64(i)).len()),
            1,
        ),
        (
            "mono d = datetime autre fuseau",
            Box::new(|i| {
                s.find_nodes_by_property("d", &Value::String(rfc3339(EPOQUE + i, 2).into()))
                    .len()
            }),
            1,
        ),
        (
            "2 cond. (a, b)",
            Box::new(|i| {
                s.find_nodes_by_properties(&[("a", Value::Int64(i % 2)), ("b", Value::Int64(i))])
                    .len()
            }),
            1,
        ),
        (
            "2 cond. (b, a)",
            Box::new(|i| {
                s.find_nodes_by_properties(&[("b", Value::Int64(i)), ("a", Value::Int64(i % 2))])
                    .len()
            }),
            1,
        ),
        (
            "2 cond. (a, b) ABSENT",
            Box::new(|i| {
                s.find_nodes_by_properties(&[
                    ("a", Value::Int64(i % 2)),
                    ("b", Value::Int64(n + i)),
                ])
                .len()
            }),
            0,
        ),
    ];
    for (nom, mut op, attendu) in scenarios {
        if mode_ram && (nom.contains(" s ") || nom.contains(" f(") || nom.contains(" d ")) {
            continue;
        }
        mesurer_scenario(nom, n, max_ops, budget, &mut *op, attendu);
    }
    // ── Le DÉCOMPTE seul (l'estimateur de sélectivité de l'élection). ──
    // `a` porte 2 valeurs : son jeu fait N/2 entrées. Le décompte plafonné
    // ne doit pas en dépendre ; le décompte EXACT (plafond infini) montre ce
    // qu'il en coûterait de compter sans plafond.
    mesurer_scenario(
        "décompte a (N/2), plafond 8",
        n,
        max_ops,
        budget,
        |i| {
            usize::from(
                s.taille_candidats_indexes("a", &Value::Int64(i % 2), 8)
                    .is_some_and(|(t, exacte)| t > 8 && !exacte || t > 8),
            )
        },
        1,
    );
    mesurer_scenario(
        "décompte a (N/2), exact",
        n,
        max_ops,
        budget,
        |i| {
            usize::from(
                s.taille_candidats_indexes("a", &Value::Int64(i % 2), u64::MAX)
                    .is_some_and(|(t, _)| t >= (n as u64) / 2),
            )
        },
        1,
    );
    mesurer_scenario(
        "décompte b (unique), plafond 8",
        n,
        max_ops,
        budget,
        |i| {
            usize::from(
                s.taille_candidats_indexes("b", &Value::Int64(i), 8)
                    .is_some_and(|(t, exacte)| t == 1 && exacte),
            )
        },
        1,
    );
    // Trois conditions, dans les six ordres.
    let ordres: [[usize; 3]; 6] = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    for ordre in ordres {
        let noms = ["a", "c", "b"];
        let nom = format!(
            "3 cond. ({}, {}, {})",
            noms[ordre[0]], noms[ordre[1]], noms[ordre[2]]
        );
        mesurer_scenario(
            &nom,
            n,
            max_ops,
            budget,
            |i| {
                let v = [Value::Int64(i % 2), Value::Int64(i % 1000), Value::Int64(i)];
                let conditions = [
                    (noms[ordre[0]], v[ordre[0]].clone()),
                    (noms[ordre[1]], v[ordre[1]].clone()),
                    (noms[ordre[2]], v[ordre[2]].clone()),
                ];
                s.find_nodes_by_properties(&conditions).len()
            },
            1,
        );
    }
    // ── A2 : recherche d'instant dont le porteur vit en COLONNE BLOB ──
    let p = periode_blobs_d();
    if p > 0 {
        mesurer_scenario(
            "mono d = instant porté par un BLOB",
            n,
            max_ops,
            budget,
            |i| {
                let j = i - i % p;
                s.find_nodes_by_property("d", &Value::String(rfc3339(EPOQUE + j, 2).into()))
                    .len()
            },
            1,
        );
    }
    // ── Coût du DROP (revue 4, A1 : réécriture immédiate de la zone) ──
    if std::env::var("ECHELLE_DROP").is_ok_and(|v| v == "1") {
        let t = Instant::now();
        let existait = s.drop_property_index("s");
        println!(
            "[drop] drop_property_index(\"s\") : {:.1} ms (existait={existait})",
            t.elapsed().as_secs_f64() * 1e3
        );
        // Une écriture indexée, puis la fusion suivante (flush).
        s.set_node_property(obrain_common::types::NodeId(1), "b", Value::Int64(-1));
        let t = Instant::now();
        s.flush().expect("flush");
        println!(
            "[drop] flush suivant (fusion de la zone, 5 index) : {:.1} ms",
            t.elapsed().as_secs_f64() * 1e3
        );
        let t = Instant::now();
        s.flush().expect("flush");
        println!(
            "[drop] flush sans écriture indexée (ancres seules) : {:.1} ms",
            t.elapsed().as_secs_f64() * 1e3
        );
    }
    println!(
        "[mesure] RSS max {} Mo · charge à la fin : {}",
        rss_max_mo(),
        charge()
    );
}

fn main() {
    // `cargo bench` passe `--bench` : on ne retient que le premier mot nu.
    let commande = std::env::args()
        .skip(1)
        .find(|a| !a.starts_with('-'))
        .unwrap_or_default();
    match commande.as_str() {
        "construire" => construire(),
        "mesurer" => mesurer(),
        _ => eprintln!(
            "usage : ECHELLE_BASE=… ECHELLE_N=… cargo bench -p obrain-substrate \
             --bench echelle_resolution -- construire|mesurer"
        ),
    }
}
