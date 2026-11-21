//! # `histogramme_props_181` — longueur réelle des chaînes de pages de propriétés
//!
//! `mesure_props_v2` reproduit l'amplification sur une base **synthétique**.
//! Il manquait la mesure sur une base **réelle** : l'issue #181 écrit qu'un
//! « histogramme `first_prop_off` → longueur de chaîne trancherait entre
//! *petites pages gaspillées* et *débordements* ». C'est cette sonde, et c'est
//! sa seule ambition : elle imprime, elle n'affirme rien.
//!
//! ## Ce qu'elle mesure, et à quoi ça sert
//!
//! 1. **Pages par nœud** — histogramme de la longueur de chaîne. Si la masse
//!    est à une page, le coût vient de la granularité (4 Kio par nœud) et la
//!    piste « slotted pages » de l'issue est la bonne. Si la masse déborde,
//!    c'est la taille de page qu'il faut revoir. Les deux correctifs n'ont
//!    rien à voir : c'est pourquoi la mesure précède le code.
//! 2. **Remplissage des pages** — distribution de `free_offset` sur les pages
//!    de nœuds. Une page à 200 o occupés sur 4 072 est du gaspillage pur ; une
//!    page à 4 000 o est pleine et ne doit rien à personne.
//! 3. **Pages orphelines** — pages de `props.v2` qu'aucune tête de chaîne
//!    n'atteint : elles occupent du disque sans servir aucune lecture.
//!
//! ## Méthode, et pourquoi elle est vérifiée
//!
//! Deux balayages séquentiels, aucun accès aléatoire :
//!
//! * `substrate.nodes` : 40 o par nœud (`NodeRecord::SIZE`), `first_prop_off`
//!   = `u48` LE à l'offset 14 ([`obrain_substrate::record::NodeRecord`]).
//! * `substrate.props.v2` : pages de 4 096 o, en-tête de 24 o —
//!   `magic` u32 @0, `node_id` u32 @4, `crc32` u32 @8, `next_page` u48 @12,
//!   `entry_count` u16 @18, `free_offset` u16 @20, `tombstones` u16 @22.
//!   `free_offset` compte les octets occupés dans la charge utile de 4 072 o
//!   (`PropertyPage::append_entry`) ; la page 0 est un sentinelle et
//!   `next_page == 0` termine une chaîne.
//!
//! Le balayage des pages donne la longueur de chaîne par propriétaire sans
//! suivre un seul lien. Pour ne pas prendre ce raccourci sur parole, la sonde
//! **remesure** N chaînes tirées à gauche et à droite en suivant réellement
//! `next_page` depuis `first_prop_off`, et imprime le taux d'accord. Un écart
//! n'est pas une erreur de mesure : c'est le nombre de pages orphelines.
//!
//! ## Usage
//!
//! ```bash
//! cargo run -p obrain-substrate --example histogramme_props_181 -- <base.obrain> [tirage]
//! ```
//!
//! `tirage` règle le nombre de chaînes re-suivies (défaut 100 000).

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

const PAGE_SIZE: u64 = 4096;
const PAYLOAD: u64 = 4072; // PAGE_SIZE - PAGE_HEADER
const NODE_RECORD: usize = 40;
const NODE_FIRST_PROP_OFF: usize = 14;
const MAGIC_NODE: u32 = 0xF507_5FA6;
const MAGIC_EDGE: u32 = 0xE507_5FA6;

#[inline]
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

#[inline]
fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

#[inline]
fn u48_at(b: &[u8], o: usize) -> u64 {
    let mut v = [0u8; 8];
    v[..6].copy_from_slice(&b[o..o + 6]);
    u64::from_le_bytes(v)
}

fn gio(o: u64) -> f64 {
    o as f64 / (1u64 << 30) as f64
}

/// Seaux de remplissage : les deux extrêmes disent tout.
const SEAUX: [(&str, u16, u16); 8] = [
    ("0 – 255 o   (quasi vide)", 0, 255),
    ("256 – 1023 o", 256, 1023),
    ("1024 – 2047 o", 1024, 2047),
    ("2048 – 3071 o", 2048, 3071),
    ("3072 – 3583 o", 3072, 3583),
    ("3584 – 3839 o", 3584, 3839),
    ("3840 – 3967 o", 3840, 3967),
    ("3968 – 4072 o (pleine)", 3968, u16::MAX),
];

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().unwrap_or_else(|| {
        eprintln!("usage : histogramme_props_181 <base.obrain> [tirage]");
        std::process::exit(2);
    }));
    let tirage: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(100_000);

    let nodes_path = dir.join("substrate.nodes");
    let props_path = dir.join("substrate.props.v2");
    for p in [&nodes_path, &props_path] {
        if !p.exists() {
            eprintln!("absent : {}", p.display());
            std::process::exit(2);
        }
    }

    let meta_props = std::fs::metadata(&props_path)?;
    let nodes_len = std::fs::metadata(&nodes_path)?.len();
    let props_len = meta_props.len();
    // macOS retire les blocs d'un fichier creux : c'est la seule mesure qui
    // distingue la longueur logique de ce qui est réellement pris au disque,
    // et l'issue #181 mélange les deux (30,4 Gio logiques / 24 Go alloués).
    let props_alloc = meta_props.blocks() * 512;
    let nb_noeuds = nodes_len / NODE_RECORD as u64;
    let nb_pages = props_len / PAGE_SIZE;

    // ── 1. Balayage séquentiel des pages ────────────────────────────────────
    let mut f_props = File::open(&props_path)?;
    let mut pages_par_owner: HashMap<u32, u32> = HashMap::new();
    let (mut pg_noeud, mut pg_arete, mut pg_vides, mut pg_inconnues) = (0u64, 0u64, 0u64, 0u64);
    // Les pages nulles ne sont pas toutes de même nature : la marge de croissance
    // ×1,5 est une plage contiguë en FIN de fichier, alors que des pages libérées
    // au milieu signaleraient un défaut de recyclage. Les deux se corrigent
    // différemment (piste 2 de l'issue, ou pas du tout) : on garde les plages.
    let mut vides_plages: Vec<(u64, u64)> = Vec::new();
    let mut vide_debut: Option<u64> = None;
    let mut vide_prec: u64 = 0;
    let mut remplissage = [0u64; SEAUX.len()];
    let mut octets_entrees = 0u64;
    let mut tombstones = 0u64;
    let mut buf = vec![0u8; PAGE_SIZE as usize];
    for idx in 0..nb_pages {
        f_props.seek(SeekFrom::Start(idx * PAGE_SIZE))?;
        f_props.read_exact(&mut buf)?;
        let magic = u32_at(&buf, 0);
        if magic == 0 {
            pg_vides += 1;
            match vide_debut {
                None => vide_debut = Some(idx),
                Some(d) if idx != vide_prec + 1 => {
                    vides_plages.push((d, vide_prec));
                    vide_debut = Some(idx);
                }
                Some(_) => {}
            }
            vide_prec = idx;
            continue;
        }
        if magic == MAGIC_EDGE {
            pg_arete += 1;
            continue;
        }
        if magic != MAGIC_NODE {
            pg_inconnues += 1;
            continue;
        }
        pg_noeud += 1;
        *pages_par_owner.entry(u32_at(&buf, 4)).or_insert(0) += 1;
        let used = u16_at(&buf, 20);
        octets_entrees += used.min(PAYLOAD as u16) as u64;
        tombstones += u16_at(&buf, 22) as u64;
        for (i, (_, lo, hi)) in SEAUX.iter().enumerate() {
            if used >= *lo && used <= *hi {
                remplissage[i] += 1;
                break;
            }
        }
    }
    if let Some(d) = vide_debut {
        vides_plages.push((d, vide_prec));
    }

    // ── 2. Balayage séquentiel des nœuds : têtes de chaîne ──────────────────
    let mut f_nodes = BufReader::with_capacity(1 << 20, File::open(&nodes_path)?);
    let mut tetes: Vec<(u32, u32)> = Vec::new();
    let mut sans_props = 0u64;
    let mut rec = [0u8; NODE_RECORD];
    for id in 0..nb_noeuds {
        f_nodes.read_exact(&mut rec)?;
        let head = u48_at(&rec, NODE_FIRST_PROP_OFF);
        if head == 0 {
            sans_props += 1;
        } else if head < nb_pages {
            tetes.push((id as u32, head as u32));
        }
    }

    // ── 3. Histogramme des longueurs de chaîne (compte par propriétaire) ────
    let mut hist: HashMap<u32, u64> = HashMap::new();
    let mut chaine_max = 0u32;
    let mut chaine_max_owner = 0u32;
    let mut sournoises = 0u64; // chaînes plus longues que la page de tête
    for (owner, n) in &pages_par_owner {
        *hist.entry(*n).or_insert(0) += 1;
        if *n > chaine_max {
            chaine_max = *n;
            chaine_max_owner = *owner;
        }
        if *n > 1 {
            sournoises += 1;
        }
    }
    let total_chaines: u64 = hist.values().sum();

    println!("base                            : {}", dir.display());
    println!("nœuds (emplacements)            : {nb_noeuds}");
    println!("nœuds sans propriété            : {sans_props}");
    println!("nœuds à ≥ 1 page                : {}", tetes.len());
    println!("propriétaires vus dans le fichier: {total_chaines}");
    println!("pages de props.v2               : {nb_pages}");
    println!(
        "props.v2 logique / alloué       : {:.2} Gio / {:.2} Gio",
        gio(props_len),
        gio(props_alloc)
    );
    println!();
    println!(
        "pages par nature                : nœud {pg_noeud}, arête {pg_arete}, vides {pg_vides}, inconnues {pg_inconnues}"
    );
    if pg_vides > 0 && !vides_plages.is_empty() {
        let mut plages = vides_plages.clone();
        plages.sort_by_key(|(a, b)| std::cmp::Reverse(b - a));
        let (a, b) = plages[0];
        let en_fin = b + 1 == nb_pages;
        println!(
            "pages vides                     : {} plages, plus grande {} pages ([{a}..{b}], {:.2} Gio){}",
            vides_plages.len(),
            b - a + 1,
            gio((b - a + 1) * PAGE_SIZE),
            if en_fin {
                ", jusqu'à la fin du fichier"
            } else {
                ", en milieu de fichier"
            }
        );
        if en_fin {
            println!(
                "  → c'est la marge de croissance ×1,5, pas des pages libérées (piste 2 de l'issue)"
            );
        } else {
            println!(
                "  → ces pages sont au MILIEU du fichier : pages libérées non recyclées, à instruire"
            );
        }
    }
    println!();
    println!("── longueur de chaîne par nœud (pages) ──");
    println!(
        "{:<10} {:>12} {:>10} {:>14}",
        "pages", "nœuds", "% nœuds", "pages"
    );
    let mut cles: Vec<&u32> = hist.keys().collect();
    cles.sort();
    for k in cles {
        let n = hist[k];
        println!(
            "{:<10} {:>12} {:>9.2}% {:>14}",
            k,
            n,
            100.0 * n as f64 / total_chaines.max(1) as f64,
            *k as u64 * n
        );
    }
    println!();
    println!("── remplissage des pages de nœuds (free_offset / {PAYLOAD} o) ──");
    println!("{:<30} {:>12} {:>10}", "octets occupés", "pages", "% pages");
    for (i, (nom, _, _)) in SEAUX.iter().enumerate() {
        let n = remplissage[i];
        println!(
            "{:<30} {:>12} {:>9.2}%",
            nom,
            n,
            100.0 * n as f64 / pg_noeud.max(1) as f64
        );
    }
    println!();
    println!("octets d'entrées (somme free_offset) : {octets_entrees}");
    println!("pierres tombales déclarées           : {tombstones}");
    println!(
        "amplification logique                : {:.2}×  ({:.2} Gio pour {:.2} Gio d'entrées)",
        pg_noeud as f64 * PAGE_SIZE as f64 / octets_entrees.max(1) as f64,
        gio(pg_noeud * PAGE_SIZE),
        gio(octets_entrees)
    );
    println!(
        "amplification allouée                : {:.2}×",
        props_alloc as f64 / octets_entrees.max(1) as f64
    );
    println!(
        "chaîne la plus longue                : {chaine_max} pages (propriétaire {chaine_max_owner})"
    );
    println!("nœuds à plus d'une page              : {sournoises}");

    // ── 4. Re-suivre N chaînes pour éprouver le raccourci ───────────────────
    if !tetes.is_empty() && tirage > 0 {
        let mut walker = File::open(&props_path)?;
        let pas = (tetes.len() / tirage.max(1)).max(1);
        let mut accord = 0u64;
        let mut desaccord = 0u64;
        let mut ecart_pages = 0i64;
        let mut vus = 0u64;
        for (owner, head) in tetes.iter().step_by(pas).take(tirage) {
            let mut n = 0u32;
            let mut idx = *head as u64;
            let mut garde = 0u64;
            while idx != 0 && idx < nb_pages && garde < 1_000_000 {
                n += 1;
                walker.seek(SeekFrom::Start(idx * PAGE_SIZE + 12))?;
                let mut nx = [0u8; 6];
                walker.read_exact(&mut nx)?;
                idx = u48_at(&nx, 0);
                garde += 1;
            }
            let attendu = *pages_par_owner.get(owner).unwrap_or(&0);
            if n == attendu {
                accord += 1;
            } else {
                desaccord += 1;
                ecart_pages += attendu as i64 - n as i64;
            }
            vus += 1;
        }
        println!();
        println!("── contrôle par re-suivi de {vus} chaînes ──");
        println!(
            "accord {accord} ({:.2} %), désaccord {desaccord}, écart cumulé {ecart_pages} pages",
            100.0 * accord as f64 / vus.max(1) as f64
        );
        if desaccord == 0 {
            println!(
                "→ le compte par balayage séquentiel reproduit exactement les chaînes suivies"
            );
        } else {
            println!(
                "→ les pages comptées et non atteintes ({}) sont des orphelines ou des chaînes écrasées",
                ecart_pages.max(0)
            );
        }
    }

    Ok(())
}
