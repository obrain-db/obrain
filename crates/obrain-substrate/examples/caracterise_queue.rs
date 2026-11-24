//! # `caracterise_queue` — caractériser la queue de rappel de la cascade (#347)
//!
//! `mesure_cascade` n'affiche que la moyenne et le p05 du rappel@k. Cet outil
//! émet **une ligne CSV par requête** pour que la queue (les requêtes mal
//! servies) puisse être analysée hors ligne, axe par axe. Il ne propose aucun
//! remède et ne touche pas au moteur de recherche.
//!
//! ## Protocole
//!
//! - **Tirage** : `n_req` nœuds portant `_st_embedding` (384 d), tirés au
//!   hasard avec une graine fixe (SplitMix64 + Fisher-Yates partiel) —
//!   reproductible, et non plus « les N premiers nœuds ».
//! - **Population** : `OBRAIN_LABELS` (ex. `!Alert` ou `Function,Step`)
//!   restreint la population interrogée ; l'oracle porte toujours sur tout
//!   l'index.
//! - **Auto-voisinage exclu** : le nœud interrogé est dans l'index, donc son
//!   propre premier voisin (cosinus 1). Les deux côtés demandent `k + 1`
//!   résultats, retirent l'offset de la requête, puis tronquent à `k`. Sans
//!   cela le rappel est gonflé d'un voisin sûr sur dix.
//! - **Référence** : `k0 = k1 = n` avec rerank exact (cosinus natif sur tous
//!   les nœuds de l'index) : c'est l'oracle exact. Ses scores donnent `d1`
//!   (1er voisin) et `d10` (k-ième voisin), en distance cosinus `1 - cos`.
//! - **Mesure** : deux cascades aux largeurs `k0`, `k1` données.
//!   `rappel` = cascade AVEC rerank exact des `max(k, k1)` candidats (le
//!   chemin de production, cf. `definir_source_native` côté moteur) ;
//!   `rappel_l2` = cascade SANS rerank (score L2 projeté, ce que mesure
//!   `mesure_cascade`).
//!
//! ## Colonnes CSV
//!
//! `id, label, rappel, rappel_l2, norme, cos_centroide, d1, d10, ratio,
//! nb_doublons, longueur_texte, auto_trouve`
//!
//! - `cos_centroide` : cosinus entre la requête et le centroïde du corpus
//!   embarqué. L'index n'expose PAS le « centre du cône » : c'est un proxy.
//! - `nb_doublons` : voisins de la référence à cosinus ≥ 0,999 (quasi-copies).
//! - `longueur_texte` : longueur en caractères de la première propriété texte
//!   trouvée parmi `OBRAIN_TEXTE` (défaut `text,content,description,name,title`),
//!   vide si aucune.
//! - `auto_trouve` : la cascade (rerank) a-t-elle retrouvé la requête elle-même.
//!
//! ## Usage
//! ```text
//! cargo run --release -p obrain-substrate --example caracterise_queue -- \
//!     <base> [requetes] [k] [k0] [k1] [graine] > queue.csv
//! ```
//! La base est ouverte en lecture seule : aucun octet n'est modifié.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use obrain_common::{PropertyKey, Value};
use obrain_core::graph::traits::GraphStore;
use obrain_substrate::SubstrateStore;
use obrain_substrate::retrieval::{LargeursCascade, NodeOffset, SourceNative};

type BoxErr = Box<dyn std::error::Error + Send + Sync>;

const DIM: usize = 384;
/// Seuil de cosinus au-delà duquel deux vecteurs sont des quasi-copies.
const SEUIL_DOUBLON: f32 = 0.999;

/// SplitMix64 : générateur déterministe, sans dépendance.
struct SplitMix64(u64);

impl SplitMix64 {
    fn suivant(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Tire `n` indices distincts parmi `0..total`, de façon déterministe pour une
/// graine donnée (Fisher-Yates partiel). `n` est borné à `total`.
fn tirer_indices(total: usize, n: usize, graine: u64) -> Vec<usize> {
    let n = n.min(total);
    let mut pool: Vec<usize> = (0..total).collect();
    let mut rng = SplitMix64(graine);
    for i in 0..n {
        let j = i + (rng.suivant() % (total - i) as u64) as usize;
        pool.swap(i, j);
    }
    pool.truncate(n);
    pool
}

/// Retire `auto` des résultats puis tronque à `k` : le nœud interrogé est son
/// propre premier voisin et fausserait le rappel. Rend aussi si `auto` y était.
fn sans_auto(
    resultats: Vec<(NodeOffset, f32)>,
    auto: NodeOffset,
    k: usize,
) -> (Vec<(NodeOffset, f32)>, bool) {
    let avant = resultats.len();
    let mut v: Vec<(NodeOffset, f32)> = resultats.into_iter().filter(|(o, _)| *o != auto).collect();
    let trouve = v.len() < avant;
    v.truncate(k);
    (v, trouve)
}

/// Rappel@k : part des `k` voisins de référence retrouvés. `k == 0` ou
/// référence vide → 0 (jamais de division par zéro).
fn rappel(obtenu: &[NodeOffset], reference: &[NodeOffset]) -> f64 {
    if reference.is_empty() {
        return 0.0;
    }
    let inter = obtenu.iter().filter(|o| reference.contains(o)).count();
    inter as f64 / reference.len() as f64
}

fn norme(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

fn cosinus(a: &[f32], b: &[f32]) -> f32 {
    let (na, nb) = (norme(a), norme(b));
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>() / (na * nb)
}

/// Centroïde des vecteurs normalisés (lignes de `plat`, `DIM` colonnes).
fn centroide(plat: &[f32]) -> Vec<f32> {
    let mut c = vec![0f32; DIM];
    for ligne in plat.as_chunks::<DIM>().0 {
        let n = norme(ligne);
        if n > 0.0 {
            for (acc, x) in c.iter_mut().zip(ligne) {
                *acc += x / n;
            }
        }
    }
    c
}

/// Longueur en caractères de la première propriété texte non vide.
fn longueur_texte(props: &[(String, String)], noms: &[String]) -> Option<usize> {
    noms.iter().find_map(|n| {
        props
            .iter()
            .find(|(k, v)| k == n && !v.is_empty())
            .map(|(_, v)| v.chars().count())
    })
}

/// Filtre de labels du tirage (`OBRAIN_LABELS`) : liste séparée par des
/// virgules ; `!Alert` exclut, `Function` n'admet que ce label. Filtre vide =
/// tout est retenu. Les exclusions l'emportent sur les inclusions.
fn label_retenu(label: &str, filtre: &[String]) -> bool {
    if filtre.iter().any(|f| f.strip_prefix('!') == Some(label)) {
        return false;
    }
    let inclus: Vec<&String> = filtre.iter().filter(|f| !f.starts_with('!')).collect();
    inclus.is_empty() || inclus.iter().any(|f| f.as_str() == label)
}

/// Ligne CSV d'une requête (champs optionnels vides).
struct Ligne {
    id: u64,
    label: String,
    rappel: f64,
    rappel_l2: f64,
    norme: f32,
    cos_centroide: f32,
    d1: f32,
    d10: f32,
    nb_doublons: usize,
    longueur: Option<usize>,
    auto_trouve: bool,
}

const ENTETE: &str = "id,label,rappel,rappel_l2,norme,cos_centroide,d1,d10,ratio,nb_doublons,longueur_texte,auto_trouve";

impl Ligne {
    fn csv(&self) -> String {
        // ratio d10/d1 : indéfini quand d1 == 0 (quasi-copie exacte).
        let ratio = if self.d1 > 0.0 {
            format!("{:.6}", self.d10 / self.d1)
        } else {
            String::new()
        };
        let longueur = self.longueur.map(|l| l.to_string()).unwrap_or_default();
        format!(
            "{},{},{:.2},{:.2},{:.6},{:.6},{:.6},{:.6},{},{},{},{}",
            self.id,
            self.label.replace(',', ";"),
            self.rappel,
            self.rappel_l2,
            self.norme,
            self.cos_centroide,
            self.d1,
            self.d10,
            ratio,
            self.nb_doublons,
            longueur,
            u8::from(self.auto_trouve),
        )
    }
}

fn premier_label(store: &SubstrateStore, id: u64) -> String {
    store
        .get_node(obrain_common::NodeId(id))
        .and_then(|nd| nd.labels.first().map(|l| l.to_string()))
        .unwrap_or_default()
}

fn main() -> Result<(), BoxErr> {
    let args: Vec<String> = std::env::args().collect();
    let base = args.get(1).ok_or_else(|| -> BoxErr {
        "usage: caracterise_queue <base> [requetes] [k] [k0] [k1] [graine]".into()
    })?;
    let n_req: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1000);
    let k: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(10);
    let k0: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(4000);
    let k1: usize = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(400);
    let graine: u64 = args.get(6).and_then(|s| s.parse().ok()).unwrap_or(347);
    let noms_texte: Vec<String> = std::env::var("OBRAIN_TEXTE")
        .unwrap_or_else(|_| "text,content,description,name,title".into())
        .split(',')
        .map(str::to_owned)
        .collect();

    let filtre_labels: Vec<String> = std::env::var("OBRAIN_LABELS")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();

    let store = SubstrateStore::open_read_only(PathBuf::from(base))?;
    let index = store
        .load_tier_index(DIM)?
        .ok_or("no tier index on this base (zones absentes ou incomplètes)")?;

    // Tous les embeddings (le nœud d'offset `o` a pour NodeId `o`, cf. la
    // source native du moteur) : sert de source de rerank et de tirage.
    let cle = PropertyKey::new("_st_embedding");
    let mut ids: Vec<u64> = Vec::new();
    let mut plat: Vec<f32> = Vec::new();
    for id in store.node_ids() {
        if let Some(Value::Vector(v)) = store.get_node_property(id, &cle)
            && v.len() == DIM
        {
            ids.push(id.0);
            plat.extend_from_slice(&v);
        }
    }
    if ids.is_empty() {
        return Err("aucun embedding _st_embedding de 384 dimensions trouvé".into());
    }
    let ligne_de: HashMap<u64, usize> = ids.iter().enumerate().map(|(i, &o)| (o, i)).collect();
    let plat = Arc::new(plat);
    let source: Arc<dyn SourceNative> = {
        let plat = Arc::clone(&plat);
        let ligne_de = ligne_de.clone();
        Arc::new(move |offset: NodeOffset, dest: &mut Vec<f32>| {
            match ligne_de.get(&u64::from(offset)) {
                Some(&i) => {
                    dest.extend_from_slice(&plat[i * DIM..(i + 1) * DIM]);
                    true
                }
                None => false,
            }
        })
    };
    let n = store.node_count();
    eprintln!(
        "caracterise_queue: {n} nœuds, {} embeddings, index de {} entrées, graine={graine}, k={k}, k0={k0}, k1={k1}",
        ids.len(),
        obrain_substrate::retrieval::VectorIndex::len(&index)
    );
    let centre = centroide(&plat);

    let exact = LargeursCascade { k0: n, k1: n };
    let mesure = LargeursCascade { k0, k1 };
    // Population interrogeable : tous les embeddings, ou ceux dont le label
    // passe `OBRAIN_LABELS` (l'oracle, lui, porte toujours sur tout l'index).
    let eligibles: Vec<usize> = (0..ids.len())
        .filter(|&i| {
            filtre_labels.is_empty() || label_retenu(&premier_label(&store, ids[i]), &filtre_labels)
        })
        .collect();
    if eligibles.is_empty() {
        return Err("aucun nœud ne passe OBRAIN_LABELS".into());
    }
    eprintln!("population interrogeable : {} nœuds", eligibles.len());
    let tirage: Vec<usize> = tirer_indices(eligibles.len(), n_req, graine)
        .into_iter()
        .map(|j| eligibles[j])
        .collect();
    println!("{ENTETE}");
    for (fait, &i) in tirage.iter().enumerate() {
        let id = ids[i];
        let auto = id as NodeOffset;
        let q = &plat[i * DIM..(i + 1) * DIM];

        index.definir_source_native(Some(Arc::clone(&source)));
        let (reference, _) = sans_auto(index.chercher(q, k + 1, exact), auto, k);
        let (obtenu, auto_trouve) = sans_auto(index.chercher(q, k + 1, mesure), auto, k);
        index.definir_source_native(None);
        let (obtenu_l2, _) = sans_auto(index.chercher(q, k + 1, mesure), auto, k);

        let offs = |v: &[(NodeOffset, f32)]| v.iter().map(|(o, _)| *o).collect::<Vec<_>>();
        let (ref_o, got_o, l2_o) = (offs(&reference), offs(&obtenu), offs(&obtenu_l2));
        let (d1, d10) = match (reference.first(), reference.last()) {
            (Some(a), Some(b)) => (1.0 - a.1, 1.0 - b.1),
            _ => (f32::NAN, f32::NAN),
        };
        let props: Vec<(String, String)> = store
            .get_node(obrain_common::NodeId(id))
            .map(|nd| {
                nd.properties
                    .iter()
                    .filter_map(|(key, val)| {
                        val.as_str()
                            .map(|s| (key.as_str().to_owned(), s.to_owned()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let label = premier_label(&store, id);
        let ligne = Ligne {
            id,
            label,
            rappel: rappel(&got_o, &ref_o),
            rappel_l2: rappel(&l2_o, &ref_o),
            norme: norme(q),
            cos_centroide: cosinus(q, &centre),
            d1,
            d10,
            nb_doublons: reference
                .iter()
                .filter(|(_, s)| *s >= SEUIL_DOUBLON)
                .count(),
            longueur: longueur_texte(&props, &noms_texte),
            auto_trouve,
        };
        println!("{}", ligne.csv());
        if (fait + 1) % 100 == 0 {
            eprintln!("  {} / {}", fait + 1, tirage.len());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tirage_deterministe_distinct_et_borne() {
        let a = tirer_indices(100, 30, 7);
        assert_eq!(a, tirer_indices(100, 30, 7));
        assert_ne!(a, tirer_indices(100, 30, 8));
        let mut s = a.clone();
        s.sort_unstable();
        s.dedup();
        assert_eq!(s.len(), 30);
        assert!(a.iter().all(|&i| i < 100));
        assert_eq!(tirer_indices(5, 50, 1).len(), 5);
        assert!(tirer_indices(0, 3, 1).is_empty());
    }

    #[test]
    fn auto_voisin_retire_et_tronque() {
        let r = vec![(5, 1.0), (1, 0.9), (2, 0.8)];
        let (v, trouve) = sans_auto(r.clone(), 5, 2);
        assert!(trouve);
        assert_eq!(v, vec![(1, 0.9), (2, 0.8)]);
        // auto absent : on tronque simplement à k.
        let (v, trouve) = sans_auto(r, 9, 2);
        assert!(!trouve);
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn rappel_cas_limites() {
        assert_eq!(rappel(&[1, 2], &[]), 0.0);
        assert_eq!(rappel(&[], &[1, 2]), 0.0);
        assert_eq!(rappel(&[1, 2, 3], &[1, 2, 4, 5]), 0.5);
        assert_eq!(rappel(&[1, 2], &[2, 1]), 1.0);
    }

    #[test]
    fn norme_cosinus_centroide() {
        let mut a = vec![0f32; DIM];
        a[0] = 3.0;
        a[1] = 4.0;
        assert_eq!(norme(&a), 5.0);
        assert!((cosinus(&a, &a) - 1.0).abs() < 1e-6);
        assert_eq!(cosinus(&a, &vec![0f32; DIM]), 0.0);
        assert_eq!(cosinus(&vec![0f32; DIM], &a), 0.0);
        let mut b = vec![0f32; DIM];
        b[2] = 1.0;
        assert!(cosinus(&a, &b).abs() < 1e-6);
        // Deux lignes + une ligne nulle (ignorée) : centroïde des normalisées.
        let mut plat = a.clone();
        plat.extend_from_slice(&b);
        plat.extend(vec![0f32; DIM]);
        let c = centroide(&plat);
        assert!(
            (c[0] - 0.6).abs() < 1e-6 && (c[1] - 0.8).abs() < 1e-6 && (c[2] - 1.0).abs() < 1e-6
        );
    }

    #[test]
    fn longueur_texte_premiere_propriete_non_vide() {
        let noms = vec!["text".to_owned(), "name".to_owned()];
        let props = vec![
            ("name".to_owned(), "abc".to_owned()),
            ("text".to_owned(), String::new()),
        ];
        assert_eq!(longueur_texte(&props, &noms), Some(3));
        let props = vec![("text".to_owned(), "héllo".to_owned())];
        assert_eq!(longueur_texte(&props, &noms), Some(5));
        assert_eq!(longueur_texte(&[], &noms), None);
    }

    #[test]
    fn filtre_de_labels() {
        let v = |s: &[&str]| s.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        assert!(label_retenu("Alert", &[]));
        assert!(!label_retenu("Alert", &v(&["!Alert"])));
        assert!(label_retenu("Step", &v(&["!Alert"])));
        assert!(label_retenu("Step", &v(&["Step", "Function"])));
        assert!(!label_retenu("Alert", &v(&["Step"])));
        assert!(!label_retenu("Step", &v(&["Step", "!Step"])));
    }

    #[test]
    fn csv_ligne_complete_et_champs_vides() {
        let mut l = Ligne {
            id: 7,
            label: "A,B".into(),
            rappel: 0.5,
            rappel_l2: 0.3,
            norme: 1.0,
            cos_centroide: 0.25,
            d1: 0.1,
            d10: 0.2,
            nb_doublons: 2,
            longueur: Some(42),
            auto_trouve: true,
        };
        let c = l.csv();
        assert!(c.starts_with("7,A;B,0.50,0.30,"));
        assert!(c.ends_with(",2.000000,2,42,1"));
        assert_eq!(c.split(',').count(), ENTETE.split(',').count());
        l.d1 = 0.0;
        l.longueur = None;
        l.auto_trouve = false;
        let c = l.csv();
        assert!(c.ends_with(",,2,,0"));
        assert_eq!(c.split(',').count(), ENTETE.split(',').count());
    }
}
