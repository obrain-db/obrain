//! Rejoue une requête du banc PO à chaud, sur une copie en lecture seule (obrain#189).
//!
//! La requête et ses paramètres sont lus dans le plan du banc (`plan_reel.tsv` :
//! colonne 2 = Cypher, colonne 3 = `nom[:type]=valeur;;...`), pour reproduire
//! exactement les paramètres de la mesure, sans les recopier à la main.
//!
//! ```bash
//! cargo run -p obrain-engine --release --features cypher --example mesure_propagees -- \
//!     <copie_base> <plan.tsv> note_22_get_propagated_notes_file_chat 3
//! ```
//!
//! Imprime, pour chaque exécution, la durée, le nombre de lignes et le hash des
//! lignes triées (identique avant/après un correctif si le résultat est inchangé).

use obrain_common::types::{EdgeId, LogicalType, NodeId, Value};
use obrain_core::execution::operators::{chemins_enumeres, noeuds_balayes};
use obrain_engine::ObrainDB;
use obrain_engine::query::phases::{PHASES, phases_ns};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::time::Instant;

fn parse_params(texte: &str) -> HashMap<String, Value> {
    let mut params = HashMap::new();
    for paire in texte.split(";;").filter(|s| !s.is_empty()) {
        let (gauche, valeur) = paire.split_once('=').expect("paramètre sans '='");
        let (nom, type_) = gauche.split_once(':').unwrap_or((gauche, "str"));
        let valeur_typee = match type_ {
            "float" => Value::Float64(valeur.parse().expect("float")),
            "bool" => Value::Bool(valeur.parse().expect("bool")),
            "int" => Value::Int64(valeur.parse().expect("int")),
            _ => Value::String(valeur.to_string().into()),
        };
        params.insert(nom.to_string(), valeur_typee);
    }
    params
}

// ---------------------------------------------------------------------------
// Empreinte du banc de parité (copie de `spike-migrateur/src/bench.rs`, commit
// 80025c5) : FNV-1a 64 du texte canonique, colonnes triées par nom, lignes dans
// l'ordre rendu. C'est elle qui est comparable à la face Neo4j
// (`resultats_frozen.tsv`) ; le hash `{:?}` trié ci-dessous ne l'est pas.
// ---------------------------------------------------------------------------

fn fnv1a(texte: &[u8], mut h: u64) -> u64 {
    for b in texte {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn empreinte_banc(lignes: &[String]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for l in lignes {
        h = fnv1a(l.as_bytes(), h);
        h = fnv1a(b"\n", h);
    }
    h
}

const CLES_IDENTITE: [&str; 6] = ["id", "path", "slug", "email", "name", "key"];

fn canon_noeud(props: &BTreeMap<String, Value>) -> String {
    for k in CLES_IDENTITE {
        if let Some(v) = props.get(k)
            && !v.is_null()
        {
            return format!("({k}={})", canon_valeur(v));
        }
    }
    let corps: Vec<String> = props
        .iter()
        .map(|(k, v)| format!("{k}={}", canon_valeur(v)))
        .collect();
    format!("({})", corps.join(";"))
}

fn canon_valeur(v: &Value) -> String {
    match v {
        Value::List(l) => format!(
            "[{}]",
            l.iter().map(canon_valeur).collect::<Vec<_>>().join(", ")
        ),
        Value::Map(m) => {
            let plat: BTreeMap<String, Value> =
                m.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
            if plat.contains_key("_id") || plat.contains_key("_labels") {
                let props: BTreeMap<String, Value> = plat
                    .into_iter()
                    .filter(|(k, _)| !k.starts_with('_'))
                    .collect();
                return canon_noeud(&props);
            }
            format!(
                "{{{}}}",
                plat.iter()
                    .map(|(k, v)| format!("{k}: {}", canon_valeur(v)))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        Value::Vector(x) => format!(
            "[{}]",
            x.iter()
                .map(|f| f64::from(*f).to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        autre => autre.to_string(),
    }
}

fn canon_obrain(db: &ObrainDB, v: &Value, t: &LogicalType) -> String {
    match (t, v) {
        (LogicalType::Node, Value::Int64(id)) => match db.get_node(NodeId(*id as u64)) {
            Some(n) => {
                let props = n
                    .properties_as_btree()
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect();
                canon_noeud(&props)
            }
            None => format!("(nœud {id} introuvable)"),
        },
        (LogicalType::Edge, Value::Int64(id)) => match db.get_edge(EdgeId(*id as u64)) {
            Some(e) => format!("-[{}]-", e.edge_type),
            None => format!("-[arête {id} introuvable]-"),
        },
        (LogicalType::List(interieur), Value::List(l)) => format!(
            "[{}]",
            l.iter()
                .map(|x| canon_obrain(db, x, interieur))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => canon_valeur(v),
    }
}

/// (user + sys CPU ms, minor faults, major faults) of the whole process.
fn rusage() -> (f64, i64, i64) {
    // SAFETY: getrusage fills the zeroed struct it is given.
    let mut u: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
    let ms = |t: libc::timeval| t.tv_sec as f64 * 1000.0 + t.tv_usec as f64 / 1000.0;
    (ms(u.ru_utime) + ms(u.ru_stime), u.ru_minflt, u.ru_majflt)
}

/// `uptime` load averages, read at the time of the call.
fn charge() -> String {
    std::process::Command::new("uptime")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, base, plan, nom, executions] = args.as_slice() else {
        panic!("usage : mesure_propagees <copie_base> <plan.tsv> <nom_requete> <executions>");
    };
    let executions: u32 = executions.parse().expect("nombre d'exécutions");

    let plan_texte = std::fs::read_to_string(plan).expect("lecture du plan");
    let ligne = plan_texte
        .lines()
        .find(|l| l.split('\t').next() == Some(nom.as_str()))
        .expect("requête absente du plan");
    let champs: Vec<&str> = ligne.split('\t').collect();
    // Variante ad hoc (mesure d'un étage) : MESURE_REQUETE remplace le texte du plan.
    let requete_plan = champs[1];
    let requete_var = std::env::var("MESURE_REQUETE").ok();
    let requete = requete_var.as_deref().unwrap_or(requete_plan);
    let params = parse_params(champs.get(2).copied().unwrap_or(""));
    println!("requête : {nom}");
    println!("paramètres : {params:?}");

    let debut_ouverture = Instant::now();
    let db = ObrainDB::open_read_only(base).expect("ouverture en lecture seule");
    println!(
        "ouverture (ObrainDB::open_read_only) : {:.1} ms",
        debut_ouverture.elapsed().as_secs_f64() * 1000.0
    );
    // MESURE_PREFIXE="PROFILE " : rows par opérateur au lieu du hash ; "EXPLAIN " : plan logique.
    let prefixe = std::env::var("MESURE_PREFIXE").unwrap_or_default();
    let profil = prefixe.trim() == "PROFILE";
    let requete_exec = format!("{prefixe}{requete}");
    for numero in 1..=executions {
        // Avec PROFILE, la première exécution est un échauffement sans préfixe
        // (la première du processus est froide et fausse la répartition).
        let echauffement = profil && numero == 1;
        let texte = if echauffement { requete } else { requete_exec.as_str() };
        println!("  charge avant exec {numero} : {}", charge());
        let chemins_avant = chemins_enumeres();
        let balayes_avant = noeuds_balayes();
        let phases_avant = phases_ns();
        let (cpu_avant, minflt_avant, majflt_avant) = rusage();
        let depart = Instant::now();
        let resultat = db
            .execute_cypher_with_params(texte, params.clone())
            .expect("exécution");
        let millis = depart.elapsed().as_secs_f64() * 1000.0;
        let (cpu_apres, minflt_apres, majflt_apres) = rusage();
        let phases_apres = phases_ns();
        let mut ligne_phases = String::new();
        for (i, phase) in PHASES.iter().enumerate() {
            let ms = (phases_apres[i] - phases_avant[i]) as f64 / 1e6;
            ligne_phases.push_str(&format!(" {phase:?}={ms:.1}"));
        }
        println!("  phases (ms) :{ligne_phases}");
        println!(
            "  cpu processus {:.1} ms (mur {millis:.1} ms), fautes mineures {}, majeures {}",
            cpu_apres - cpu_avant,
            minflt_apres - minflt_avant,
            majflt_apres - majflt_avant
        );
        if echauffement {
            println!("exec {numero} (échauffement, sans PROFILE) : {millis:.1} ms");
            continue;
        }
        if !prefixe.is_empty() {
            println!("exec {numero} : {millis:.1} ms");
            if let Some(Value::String(texte)) = resultat.rows.first().and_then(|l| l.first()) {
                println!("{texte}");
            }
            continue;
        }

        let mut ordre: Vec<usize> = (0..resultat.columns.len()).collect();
        ordre.sort_by(|a, b| resultat.columns[*a].cmp(&resultat.columns[*b]));
        let canon: Vec<String> = resultat
            .rows
            .iter()
            .map(|l| {
                ordre
                    .iter()
                    .map(|&i| {
                        let t = resultat
                            .column_types
                            .get(i)
                            .cloned()
                            .unwrap_or(LogicalType::Any);
                        l.get(i)
                            .map(|v| canon_obrain(&db, v, &t))
                            .unwrap_or_else(|| "NULL".into())
                    })
                    .collect::<Vec<_>>()
                    .join("\t")
            })
            .collect();
        println!("  empreinte banc (comparable Neo4j) {:016x}", empreinte_banc(&canon));
        if let Ok(dossier) = std::env::var("MESURE_DUMP") {
            let _ = std::fs::write(format!("{dossier}/{nom}_exec{numero}.txt"), canon.join("\n"));
        }
        let debut_materialisation = Instant::now();
        let mut lignes: Vec<String> = resultat.rows.iter().map(|l| format!("{l:?}")).collect();
        lignes.sort();
        let mut hasheur = DefaultHasher::new();
        lignes.hash(&mut hasheur);
        drop(resultat);
        println!(
            "  matérialisation (format + tri + hash + libération du résultat, côté appelant) : {:.1} ms",
            debut_materialisation.elapsed().as_secs_f64() * 1000.0
        );
        println!(
            "exec {numero} : {millis:.1} ms, lignes {}, chemins_enumeres {}, noeuds_balayes {}, hash {:016x}",
            lignes.len(),
            chemins_enumeres() - chemins_avant,
            noeuds_balayes() - balayes_avant,
            hasheur.finish()
        );
    }
}
