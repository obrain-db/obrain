//! Banc du COÛT DE PLANIFICATION PHYSIQUE (premier appel d'un texte / appels
//! suivants) ET de la JUSTESSE des plans produits.
//!
//! ── POURQUOI CE BANC N'INSTRUMENTE RIEN ──────────────────────────────
//!
//! La ventilation `planification=…us execution=…us` existe déjà dans le
//! moteur (`OBRAIN_PHASES`, `crates/obrain-engine/src/session/mod.rs`) et part
//! sur stderr. Le banc imprime donc un marqueur sur stdout AVANT chaque
//! exécution : en lançant `2>&1` dans un fichier, les deux flux se recollent
//! marqueur par marqueur sans qu'une seule ligne de mesure soit ajoutée au
//! code mesuré.
//!
//! ── LES DEUX PHASES ──────────────────────────────────────────────────
//!
//! 1. MESURE — familles de textes UNIQUES (littéral ou alias distinct), donc
//!    sans service du cache de plans logiques. Chaque texte est joué DEUX
//!    fois : « 1er appel d'un texte » contre « 2e appel du même texte ». Les
//!    familles vont du texte trivial (un compteur) au filtre de propriété sur
//!    un label entier, en passant par une propriété indexée au catalogue.
//!
//! 2. JUSTESSE — batterie de textes FIXES dont on imprime une empreinte des
//!    LIGNES rendues : à lancer avant et après un changement du planificateur,
//!    puis à comparer ligne à ligne. Même texte + même store ⇒ mêmes lignes.
//!
//! Usage :
//!   OBRAIN_PHASES=1 cargo run --release -p obrain-engine --example banc_plan_froid \
//!       -- <chemin.obrain> [nb_tours] [familles] > /tmp/banc.txt 2>&1

use obrain_engine::ObrainDB;
use obrain_engine::session::Session;
use web_time::Instant;

/// Empreinte stable des lignes d'un résultat : FNV-1a sur la forme textuelle
/// de chaque valeur, plus le nombre de lignes. Deux plans équivalents doivent
/// rendre la MÊME empreinte, y compris l'ordre des lignes.
fn empreinte(session: &Session, texte: &str) -> String {
    match session.execute(texte) {
        Ok(res) => {
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            let mut mixer = |s: &str| {
                for b in s.as_bytes() {
                    h ^= u64::from(*b);
                    h = h.wrapping_mul(0x0000_0100_0000_01b3);
                }
                h ^= 0xff;
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            };
            for row in &res.rows {
                for v in row {
                    mixer(&format!("{v:?}"));
                }
            }
            format!("lignes={} empreinte={h:016x}", res.rows.len())
        }
        Err(e) => format!("ERREUR {e}"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let chemin = args
        .first()
        .cloned()
        .or_else(|| std::env::var("OBRAIN_BANC_BASE").ok())
        .expect("usage : banc_plan_froid <chemin.obrain> [nb_tours] [familles]");
    let tours: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(4);
    let familles: String = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "AEBCDFGHPQSZ".to_string());

    // Jamais le chemin reçu : une copie privée (clonée sur APFS), supprimée à
    // la fin. Le banc ne fait que lire, mais toute ouverture écrit sur le
    // disque (docs/BENCHMARKS.md, « Bancs sur bases réelles »).
    let copie = obrain_bases_banc::copie_de(
        std::path::Path::new(&chemin),
        "l'argument ou OBRAIN_BANC_BASE",
    );
    let db = ObrainDB::open_substrate(copie.chemin()).expect("ouverture de la copie du store");
    let session = db.session();
    let actives = |c: char| familles.contains(c);

    // Marqueur imprimé AVANT l'exécution : le recollage avec la ligne
    // `[phases]` qui suit est donc non ambigu.
    let lancer = |etiquette: &str, texte: &str| {
        println!("MARK|{etiquette}|{texte}");
        let t = Instant::now();
        let r = session.execute(texte);
        let total_us = t.elapsed().as_secs_f64() * 1e6;
        match r {
            Ok(res) => println!(
                "DONE|{etiquette}|lignes={} total={total_us:.0}us",
                res.rows.len()
            ),
            Err(e) => println!("DONE|{etiquette}|ERREUR {e} total={total_us:.0}us"),
        }
    };

    // ── A. LE MÊME TEXTE TRIVIAL, 6 FOIS DE SUITE ──────────────────────
    // Sépare « premier appel du processus » de « appels suivants ».
    if actives('A') {
        for k in 0..6 {
            lancer(&format!("A_repet_{k}"), "MATCH (n) RETURN count(n)");
        }
    }

    // ── E. DIX TEXTES TRIVIAUX TOUS NEUFS ──────────────────────────────
    // Même charge de planification que A (un alias change le TEXTE, pas le
    // plan). Si chaque premier appel coûte cher, le surcoût est PAR TEXTE ;
    // s'ils sont tous à ~1 µs, le surcoût de A_0 était l'amorçage du
    // processus, pas la planification.
    if actives('E') {
        for i in 0..10 {
            let t = format!("MATCH (n) RETURN count(n) AS c{i}");
            lancer(&format!("E_neuf_{i}_1er"), &t);
            lancer(&format!("E_neuf_{i}_2e"), &t);
        }
    }

    // ── B. LABEL + PROPRIÉTÉ NON INDEXÉE (littéral distinct) ───────────
    if actives('B') {
        for i in 0..tours {
            let t = format!("MATCH (n:Message) WHERE n.session = 'inexistant-{i}' RETURN count(n)");
            lancer(&format!("B_neuf_{i}_1er"), &t);
            lancer(&format!("B_neuf_{i}_2e"), &t);
        }
    }

    // ── C. LABEL + PROPRIÉTÉ JAMAIS VUE ────────────────────────────────
    if actives('C') {
        for i in 0..tours {
            let t =
                format!("MATCH (n:Message) WHERE n.propriete_fantome_{i} = 'x' RETURN count(n)");
            lancer(&format!("C_prop_{i}_1er"), &t);
            lancer(&format!("C_prop_{i}_2e"), &t);
        }
    }

    // ── D. LABEL + COMPARAISON ─────────────────────────────────────────
    if actives('D') {
        for i in 0..tours {
            let t = format!("MATCH (n:Message) WHERE n.seq >= {i} RETURN count(n)");
            lancer(&format!("D_range_{i}_1er"), &t);
            lancer(&format!("D_range_{i}_2e"), &t);
        }
    }

    // ── P. PROPRIÉTÉ INDEXÉE AU CATALOGUE (qualified_name) ─────────────
    if actives('P') {
        for i in 0..tours {
            let t = format!(
                "MATCH (n:Class) WHERE n.qualified_name = 'zz-inexistant-{i}' RETURN count(n)"
            );
            lancer(&format!("P_idx_{i}_1er"), &t);
            lancer(&format!("P_idx_{i}_2e"), &t);
        }
    }

    // ── Q. PROPRIÉTÉ INDEXÉE AU CATALOGUE (file_path) ──────────────────
    if actives('Q') {
        for i in 0..tours {
            let t =
                format!("MATCH (n:File) WHERE n.file_path = '/zz/inexistant-{i}' RETURN count(n)");
            lancer(&format!("Q_idx_{i}_1er"), &t);
            lancer(&format!("Q_idx_{i}_2e"), &t);
        }
    }

    // ── F. SANS LABEL : PROPRIÉTÉ SEULE (pas de matérialisation) ───────
    if actives('F') {
        for i in 0..tours {
            let t = format!("MATCH (n) WHERE n.session = 'inexistant-{i}' RETURN count(n)");
            lancer(&format!("F_nolab_{i}_1er"), &t);
            lancer(&format!("F_nolab_{i}_2e"), &t);
        }
    }

    // ── G. PARAMÉTRÉ : le TEXTE ne change pas, la valeur si ────────────
    if actives('G') {
        for i in 0..tours {
            let t = "MATCH (n:Message) WHERE n.session = $s RETURN count(n)".to_string();
            println!("MARK|G_param_{i}|{t} $s=inexistant-{i}");
            let mut params = std::collections::HashMap::new();
            params.insert(
                "s".to_string(),
                obrain_common::types::Value::from(format!("inexistant-{i}")),
            );
            let t0 = Instant::now();
            let r = session.execute_with_params(&t, params);
            let total_us = t0.elapsed().as_secs_f64() * 1e6;
            match r {
                Ok(res) => println!(
                    "DONE|G_param_{i}|lignes={} total={total_us:.0}us",
                    res.rows.len()
                ),
                Err(e) => println!("DONE|G_param_{i}|ERREUR {e} total={total_us:.0}us"),
            }
        }
    }

    // ── H. LABEL + PROPRIÉTÉ, PRÉDICAT NON ÉGAL (IS NOT NULL) ──────────
    if actives('H') {
        for i in 0..tours {
            let t = format!("MATCH (n:Message) WHERE n.session{i} IS NOT NULL RETURN count(n)");
            lancer(&format!("H_notnull_{i}_1er"), &t);
            lancer(&format!("H_notnull_{i}_2e"), &t);
        }
    }

    // ── S. LABEL + PRÉFIXE (index de texte/ordonné) ────────────────────
    if actives('S') {
        for i in 0..tours {
            let t = format!(
                "MATCH (n:Class) WHERE n.qualified_name STARTS WITH 'zz{i}' RETURN count(n)"
            );
            lancer(&format!("S_prefixe_{i}_1er"), &t);
            lancer(&format!("S_prefixe_{i}_2e"), &t);
        }
    }

    // ── Z. JUSTESSE — textes FIXES, empreinte des LIGNES ───────────────
    // À comparer avant/après tout changement de planificateur.
    if actives('Z') {
        let batterie: &[(&str, &str)] = &[
            ("z01_count_nu", "MATCH (n) RETURN count(n)"),
            ("z02_count_label", "MATCH (n:Message) RETURN count(n)"),
            (
                "z03_label_eq_absent",
                "MATCH (n:Message) WHERE n.session = 'zzz-absent' RETURN count(n)",
            ),
            (
                "z04_label_eq_present",
                "MATCH (n:Message) WHERE n.role = 'user' RETURN count(n)",
            ),
            (
                "z05_label_eq_present_bis",
                "MATCH (n:Message) WHERE n.role = 'assistant' RETURN count(n)",
            ),
            (
                "z06_label_eq_int",
                "MATCH (n:Message) WHERE n.seq = 1 RETURN count(n)",
            ),
            (
                "z07_label_eq_coercition",
                "MATCH (n:Message) WHERE n.seq = 1.0 RETURN count(n)",
            ),
            (
                "z08_label_eq_inexistant_prop",
                "MATCH (n:Message) WHERE n.propriete_absente = 'x' RETURN count(n)",
            ),
            (
                "z09_label_and",
                "MATCH (n:Message) WHERE n.role = 'user' AND n.seq = 1 RETURN count(n)",
            ),
            (
                "z10_label_in",
                "MATCH (n:Message) WHERE n.role IN ['user', 'assistant'] RETURN count(n)",
            ),
            (
                "z11_label_lt",
                "MATCH (n:Message) WHERE n.seq < 3 RETURN count(n)",
            ),
            (
                "z12_label_neq",
                "MATCH (n:Message) WHERE n.role <> 'user' RETURN count(n)",
            ),
            (
                "z13_label_proj",
                "MATCH (n:Message) WHERE n.role = 'user' RETURN n.session LIMIT 10",
            ),
            (
                "z14_label_proj2",
                "MATCH (n:Message) WHERE n.seq >= 1 RETURN n.role LIMIT 10",
            ),
            (
                "z15_idx_qn",
                "MATCH (n:Class) WHERE n.qualified_name = 'zzz-absent' RETURN count(n)",
            ),
            (
                "z16_idx_qn_prefix",
                "MATCH (n:Class) WHERE n.qualified_name STARTS WITH 'zzz' RETURN count(n)",
            ),
            (
                "z17_nolabel_eq",
                "MATCH (n) WHERE n.role = 'user' RETURN count(n)",
            ),
            ("z18_label_nu", "MATCH (n:Message) RETURN n LIMIT 5"),
            (
                "z19_label_exists",
                "MATCH (n:Message) WHERE n.role IS NOT NULL RETURN count(n)",
            ),
            (
                "z20_label_empty_str",
                "MATCH (n:Message) WHERE n.role = '' RETURN count(n)",
            ),
        ];
        for (nom, texte) in batterie {
            println!("JUSTESSE|{nom}|{texte}|{}", empreinte(&session, texte));
        }
    }

    println!("FIN");
}
