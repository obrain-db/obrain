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

use obrain_common::types::Value;
use obrain_core::execution::operators::{chemins_enumeres, noeuds_balayes};
use obrain_engine::ObrainDB;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
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

    let db = ObrainDB::open_read_only(base).expect("ouverture en lecture seule");
    for numero in 1..=executions {
        let chemins_avant = chemins_enumeres();
        let balayes_avant = noeuds_balayes();
        let depart = Instant::now();
        let resultat = db
            .execute_cypher_with_params(requete, params.clone())
            .expect("exécution");
        let millis = depart.elapsed().as_secs_f64() * 1000.0;

        let mut lignes: Vec<String> = resultat.rows.iter().map(|l| format!("{l:?}")).collect();
        lignes.sort();
        let mut hasheur = DefaultHasher::new();
        lignes.hash(&mut hasheur);
        println!(
            "exec {numero} : {millis:.1} ms, lignes {}, chemins_enumeres {}, noeuds_balayes {}, hash {:016x}",
            lignes.len(),
            chemins_enumeres() - chemins_avant,
            noeuds_balayes() - balayes_avant,
            hasheur.finish()
        );
    }
}
