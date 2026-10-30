//! Filet d'exactitude pour [`SubstrateStore::vector_top_k`] (contrat
//! B2/M1/M2 issu de la revue adversariale) :
//!
//!   • bloc NULL d'abord (`None`, offset asc) : dimension ≠ requête,
//!     similarité non finie, List non numérique / embedding non-List — ces
//!     lignes sont émises avec similarité NULL et sortent EN TÊTE. Ce n'est
//!     plus une « parité legacy » constatée empiriquement contre un moteur
//!     au comparateur fautif : c'est le DÉFAUT du moteur pour `ORDER BY
//!     similarity DESC` sans clause NULLS — DESC → NullsFirst, convention
//!     PostgreSQL/Neo4j (NULL = plus grande valeur) — obtenu depuis la
//!     correction de l'ordre des NULL par le CHEMIN JUSTE (placement absolu
//!     dans les comparateurs + défaut par direction dans le planificateur).
//!     La sortie de `vector_top_k` était déjà la bonne ; seule sa
//!     justification a changé ;
//!   • puis les scores FINIS, tri `(similarité desc, offset asc)` ;
//!   • embedding TOUT-ZÉRO adossé à une List → score FINI 0.0 (parité avec
//!     l'ε du dénominateur legacy) — la leçon RESTE rappelable ;
//!   • veccol tout-zéro SANS List → absent (indistinguable d'un slot jamais
//!     écrit ; en production la migration additive garantit la List) ;
//!   • aucun embedding → absent (parité `WHERE n.embedding IS NOT NULL`).
//!
//! La référence série calcule sur les données CONNUES (mode de stockage
//! inclus) et réplique EXACTEMENT ces règles — y compris le routage des
//! similarités non finies dans le bloc NULL de tête (un `unwrap_or(Equal)`
//! sous tri aurait reproduit le bug M2 et rendu le filet aveugle).
//!
//! Run : cargo test -p obrain-substrate --test vector_top_k_exact

use obrain_common::types::NodeId;
use obrain_common::types::Value;
use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use std::collections::HashSet;
use std::sync::Arc;

const DIM: usize = 64;

#[derive(Clone, Copy, PartialEq)]
enum Stockage {
    /// Écrit uniquement en veccol (`Value::Vector`).
    Veccol,
    /// Écrit uniquement en `Value::List` (nœud non migré).
    Liste,
}

fn vec_for_dim(seed: u64, dim: usize) -> Vec<f32> {
    let mut x = seed.wrapping_mul(2654435761).wrapping_add(1);
    (0..dim)
        .map(|_| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((x >> 33) as f32 / (1u64 << 31) as f32) - 1.0
        })
        .collect()
}

fn vec_for(seed: u64) -> Vec<f32> {
    vec_for_dim(seed, DIM)
}

/// Cosinus série — même formule que `vector_top_k`, sommée dans l'ordre.
fn cosine(q: &[f32], v: &[f32]) -> f32 {
    let mut dot = 0.0f32;
    let mut qn = 0.0f32;
    let mut vn = 0.0f32;
    for i in 0..v.len() {
        dot += q[i] * v[i];
        qn += q[i] * q[i];
        vn += v[i] * v[i];
    }
    dot / (qn.sqrt() * vn.sqrt())
}

fn as_list(v: &[f32]) -> Value {
    Value::List(v.iter().map(|f| Value::Float64(*f as f64)).collect::<Vec<_>>().into())
}

fn poser(store: &SubstrateStore, n: NodeId, v: &[f32], mode: Stockage) {
    match mode {
        Stockage::Veccol => store.set_node_property(n, "embedding", Value::Vector(v.to_vec().into())),
        Stockage::Liste => store.set_node_property(n, "embedding", as_list(v)),
    }
}

/// Référence SÉRIE sur données CONNUES, répliquant le contrat de parité.
fn reference_top_k<F: Fn(NodeId) -> bool>(
    connus: &[(NodeId, Vec<f32>, Stockage)],
    query: &[f32],
    k: usize,
    filtre: F,
) -> Vec<(NodeId, Option<f32>)> {
    let mut scored: Vec<(f32, u64)> = Vec::new();
    let mut bloc_null: Vec<u64> = Vec::new();
    for (id, v, mode) in connus {
        if !filtre(*id) {
            continue;
        }
        let tout_zero = v.iter().all(|x| *x == 0.0);
        if tout_zero && *mode == Stockage::Veccol {
            continue; // veccol tout-zéro sans List → absent
        }
        if v.len() != query.len() {
            bloc_null.push(id.0); // dim ≠ → similarité NULL → bloc de tête
            continue;
        }
        if tout_zero {
            scored.push((0.0, id.0)); // parité ε legacy → 0.0 fini
            continue;
        }
        let sim = cosine(query, v);
        if !sim.is_finite() {
            bloc_null.push(id.0); // M2 : non fini → bloc NULL, déterministe
            continue;
        }
        scored.push((sim, id.0));
    }
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
    });
    bloc_null.sort_unstable();
    let mut out: Vec<(NodeId, Option<f32>)> = Vec::new();
    for o in bloc_null {
        if out.len() == k {
            break;
        }
        // NULLS FIRST : défaut moteur DESC → NullsFirst, chemin juste.
        out.push((NodeId(o), None));
    }
    for (s, o) in scored {
        if out.len() == k {
            break;
        }
        out.push((NodeId(o), Some(s)));
    }
    out
}

fn assert_meme_classement(actual: &[(NodeId, Option<f32>)], attendu: &[(NodeId, Option<f32>)]) {
    let ids_a: Vec<u64> = actual.iter().map(|(n, _)| n.0).collect();
    let ids_e: Vec<u64> = attendu.iter().map(|(n, _)| n.0).collect();
    assert_eq!(ids_a, ids_e, "ids et ORDRE divergent");
    for ((na, sa), (_, se)) in actual.iter().zip(attendu.iter()) {
        match (sa, se) {
            (Some(a), Some(e)) => assert!(
                (a - e).abs() < 1e-6,
                "score divergent au noeud {}: {a} vs {e}",
                na.0
            ),
            (None, None) => {}
            autre => panic!("nature du score divergente au noeud {}: {autre:?}", na.0),
        }
    }
}

/// Cas nominal : parallèle (veccol) == référence, autres labels exclus.
#[test]
fn parallele_egale_reference_serie() {
    let store = Arc::new(SubstrateStore::open_tempfile().unwrap());
    let mut connus: Vec<(NodeId, Vec<f32>, Stockage)> = Vec::new();
    for i in 0..80u64 {
        let n = store.create_node(&["EpisodicLesson"]);
        let e = vec_for(i);
        poser(&store, n, &e, Stockage::Veccol);
        connus.push((n, e, Stockage::Veccol));
    }
    let mut autres: HashSet<u64> = HashSet::new();
    for i in 0..20u64 {
        let n = store.create_node(&["ChatEvent"]);
        poser(&store, n, &vec_for(10_000 + i), Stockage::Veccol);
        autres.insert(n.0);
    }
    let query = vec_for(999);
    let k = 12;
    let actual = store.vector_top_k("EpisodicLesson", "embedding", "embedding", &query, k, |_| true);
    let attendu = reference_top_k(&connus, &query, k, |_| true);
    assert_eq!(actual.len(), k);
    assert_meme_classement(&actual, &attendu);
    for (n, _) in &actual {
        assert!(!autres.contains(&n.0), "un noeud d'un autre label a fui : {}", n.0);
    }
}

/// Filtre appliqué à l'identique des deux côtés.
#[test]
fn filtre_identique_des_deux_cotes() {
    let store = Arc::new(SubstrateStore::open_tempfile().unwrap());
    let mut connus: Vec<(NodeId, Vec<f32>, Stockage)> = Vec::new();
    for i in 0..60u64 {
        let n = store.create_node(&["EpisodicLesson"]);
        let e = vec_for(i);
        poser(&store, n, &e, Stockage::Veccol);
        connus.push((n, e, Stockage::Veccol));
    }
    let exclus: HashSet<u64> = connus.iter().map(|(n, _, _)| n.0).filter(|o| o % 3 == 0).collect();
    let exclus2 = exclus.clone();
    let filtre = move |n: NodeId| !exclus2.contains(&n.0);
    let query = vec_for(7);
    let k = 15;
    let actual = store.vector_top_k("EpisodicLesson", "embedding", "embedding", &query, k, &filtre);
    let attendu = reference_top_k(&connus, &query, k, &filtre);
    assert_meme_classement(&actual, &attendu);
    for (n, _) in &actual {
        assert!(!exclus.contains(&n.0), "noeud filtré présent : {}", n.0);
    }
}

/// Veccol tout-zéro SANS List : absent (aucune List pour trancher).
#[test]
fn veccol_tout_zero_sans_liste_est_absent() {
    let store = Arc::new(SubstrateStore::open_tempfile().unwrap());
    let mut connus: Vec<(NodeId, Vec<f32>, Stockage)> = Vec::new();
    for i in 0..30u64 {
        let n = store.create_node(&["EpisodicLesson"]);
        let e = vec_for(i);
        poser(&store, n, &e, Stockage::Veccol);
        connus.push((n, e, Stockage::Veccol));
    }
    let mut zeros: HashSet<u64> = HashSet::new();
    for _ in 0..3 {
        let n = store.create_node(&["EpisodicLesson"]);
        poser(&store, n, &vec![0.0f32; DIM], Stockage::Veccol);
        connus.push((n, vec![0.0f32; DIM], Stockage::Veccol));
        zeros.insert(n.0);
    }
    let query = vec_for(3);
    let k = 40;
    let actual = store.vector_top_k("EpisodicLesson", "embedding", "embedding", &query, k, |_| true);
    let attendu = reference_top_k(&connus, &query, k, |_| true);
    assert_meme_classement(&actual, &attendu);
    for (n, _) in &actual {
        assert!(!zeros.contains(&n.0), "veccol tout-zéro sans List remonté : {}", n.0);
    }
    assert_eq!(actual.len(), 30);
}

/// M1 — tout-zéro adossé à une List : score FINI 0.0, la leçon RESTE
/// rappelable (parité ε legacy), classée sous les scores positifs.
#[test]
fn tout_zero_avec_liste_rend_zero_fini() {
    let store = Arc::new(SubstrateStore::open_tempfile().unwrap());
    let mut connus: Vec<(NodeId, Vec<f32>, Stockage)> = Vec::new();
    // Un tout-zéro en LIST (non migré, ou migré : la List existe toujours).
    let z = store.create_node(&["EpisodicLesson"]);
    poser(&store, z, &vec![0.0f32; DIM], Stockage::Liste);
    connus.push((z, vec![0.0f32; DIM], Stockage::Liste));
    for i in 0..10u64 {
        let n = store.create_node(&["EpisodicLesson"]);
        let e = vec_for(i);
        poser(&store, n, &e, Stockage::Veccol);
        connus.push((n, e, Stockage::Veccol));
    }
    let query = vec_for(4);
    let k = 20;
    let actual = store.vector_top_k("EpisodicLesson", "embedding", "embedding", &query, k, |_| true);
    let attendu = reference_top_k(&connus, &query, k, |_| true);
    assert_meme_classement(&actual, &attendu);
    let entree_z = actual.iter().find(|(n, _)| *n == z);
    assert_eq!(
        entree_z.map(|(_, s)| *s),
        Some(Some(0.0)),
        "le tout-zéro avec List doit être présent avec un score fini 0.0"
    );
}

/// B2 — DIMENSIONS MIXTES (le cas réel 768/1024) : les nœuds de dimension ≠
/// requête sont émis (jamais écartés) avec similarité None, dans le bloc
/// NULL DE TÊTE (défaut moteur DESC → NullsFirst), ordre offset asc.
#[test]
fn dimensions_mixtes_emises_dans_le_bloc_null_de_tete() {
    let store = Arc::new(SubstrateStore::open_tempfile().unwrap());
    let mut connus: Vec<(NodeId, Vec<f32>, Stockage)> = Vec::new();
    let mut dim_courte: Vec<u64> = Vec::new();
    for i in 0..30u64 {
        let n = store.create_node(&["EpisodicLesson"]);
        if i % 3 == 0 {
            // « ancienne génération » : dimension moitié (comme 768 vs 1024)
            let e = vec_for_dim(i, DIM / 2);
            poser(&store, n, &e, Stockage::Liste);
            connus.push((n, e, Stockage::Liste));
            dim_courte.push(n.0);
        } else {
            let e = vec_for(i);
            poser(&store, n, &e, if i % 2 == 0 { Stockage::Veccol } else { Stockage::Liste });
            connus.push((n, e, if i % 2 == 0 { Stockage::Veccol } else { Stockage::Liste }));
        }
    }
    let query = vec_for(999);
    let k = 30; // assez grand pour englober le bloc NULL
    let actual = store.vector_top_k("EpisodicLesson", "embedding", "embedding", &query, k, |_| true);
    let attendu = reference_top_k(&connus, &query, k, |_| true);
    assert_meme_classement(&actual, &attendu);
    // Le bloc NULL est présent, EN TÊTE (NULLS FIRST), offset asc.
    let bloc_null_actual: Vec<u64> = actual.iter().filter(|(_, s)| s.is_none()).map(|(n, _)| n.0).collect();
    let mut attendu_bloc_null = dim_courte.clone();
    attendu_bloc_null.sort_unstable();
    assert_eq!(bloc_null_actual, attendu_bloc_null, "bloc NULL = tous les dim≠, offset asc");
    assert!(
        actual[..bloc_null_actual.len()].iter().all(|(_, s)| s.is_none()),
        "le bloc NULL ouvre la liste (NULLS FIRST)"
    );
    assert!(
        actual[bloc_null_actual.len()..].iter().all(|(_, s)| s.is_some()),
        "puis uniquement des scores finis"
    );
}

/// M2 — similarité non finie (inf stocké) : similarité NULL, donc bloc NULL
/// de tête, ordre déterministe, stable sur plusieurs exécutions.
#[test]
fn similarite_non_finie_dans_le_bloc_null_deterministe() {
    let store = Arc::new(SubstrateStore::open_tempfile().unwrap());
    let mut connus: Vec<(NodeId, Vec<f32>, Stockage)> = Vec::new();
    // Nœud pathologique : inf dans la List → cosinus NaN.
    let p = store.create_node(&["EpisodicLesson"]);
    let mut patho = vec_for(1);
    patho[0] = f32::INFINITY;
    poser(&store, p, &patho, Stockage::Liste);
    connus.push((p, patho, Stockage::Liste));
    for i in 0..12u64 {
        let n = store.create_node(&["EpisodicLesson"]);
        let e = vec_for(100 + i);
        poser(&store, n, &e, Stockage::Veccol);
        connus.push((n, e, Stockage::Veccol));
    }
    let query = vec_for(2);
    let k = 20;
    let attendu = reference_top_k(&connus, &query, k, |_| true);
    let mut precedent: Option<Vec<u64>> = None;
    for _ in 0..6 {
        let actual = store.vector_top_k("EpisodicLesson", "embedding", "embedding", &query, k, |_| true);
        assert_meme_classement(&actual, &attendu);
        let entree_p = actual.iter().find(|(n, _)| *n == p).expect("le pathologique est émis");
        assert_eq!(entree_p.1, None, "similarité non finie → bloc NULL (None)");
        let ids: Vec<u64> = actual.iter().map(|(n, _)| n.0).collect();
        if let Some(prev) = &precedent {
            assert_eq!(&ids, prev, "ordre STABLE entre exécutions");
        }
        precedent = Some(ids);
    }
}

/// Tie-break déterministe sur scores finis égaux.
#[test]
fn tie_break_deterministe() {
    let store = Arc::new(SubstrateStore::open_tempfile().unwrap());
    let query = vec_for(42);
    let a = store.create_node(&["EpisodicLesson"]);
    poser(&store, a, &query, Stockage::Veccol);
    let b = store.create_node(&["EpisodicLesson"]);
    poser(&store, b, &query, Stockage::Veccol);
    for i in 0..10u64 {
        let n = store.create_node(&["EpisodicLesson"]);
        poser(&store, n, &vec_for(100 + i), Stockage::Veccol);
    }
    assert!(a.0 < b.0);
    for _ in 0..8 {
        let r = store.vector_top_k("EpisodicLesson", "embedding", "embedding", &query, 2, |_| true);
        assert_eq!(r[0].0, a, "à égalité de score, le plus petit offset d'abord");
        assert_eq!(r[1].0, b);
    }
}

/// k supérieur au nombre de candidats.
#[test]
fn k_superieur_aux_candidats() {
    let store = Arc::new(SubstrateStore::open_tempfile().unwrap());
    let mut connus: Vec<(NodeId, Vec<f32>, Stockage)> = Vec::new();
    for i in 0..5u64 {
        let n = store.create_node(&["EpisodicLesson"]);
        let e = vec_for(i);
        poser(&store, n, &e, Stockage::Veccol);
        connus.push((n, e, Stockage::Veccol));
    }
    let query = vec_for(1);
    let actual = store.vector_top_k("EpisodicLesson", "embedding", "embedding", &query, 100, |_| true);
    let attendu = reference_top_k(&connus, &query, 100, |_| true);
    assert_eq!(actual.len(), 5);
    assert_meme_classement(&actual, &attendu);
}

/// Embeddings entièrement en List (état PRÉ-migration).
#[test]
fn embeddings_en_liste_non_migres_sont_couverts() {
    let store = Arc::new(SubstrateStore::open_tempfile().unwrap());
    let mut connus: Vec<(NodeId, Vec<f32>, Stockage)> = Vec::new();
    for i in 0..60u64 {
        let n = store.create_node(&["EpisodicLesson"]);
        let e = vec_for(i);
        poser(&store, n, &e, Stockage::Liste);
        connus.push((n, e, Stockage::Liste));
    }
    let query = vec_for(5);
    let k = 12;
    let actual = store.vector_top_k("EpisodicLesson", "embedding", "embedding", &query, k, |_| true);
    let attendu = reference_top_k(&connus, &query, k, |_| true);
    assert_eq!(actual.len(), k);
    assert_meme_classement(&actual, &attendu);
}

/// ÉTAT MIXTE veccol/List avec témoin List de slot bas (zéro-shadow) : AUCUN
/// nœud manqué.
#[test]
fn etat_mixte_veccol_et_liste_aucun_noeud_manque() {
    let store = Arc::new(SubstrateStore::open_tempfile().unwrap());
    let query = vec_for(999);
    let mut connus: Vec<(NodeId, Vec<f32>, Stockage)> = Vec::new();

    let temoin = store.create_node(&["EpisodicLesson"]);
    poser(&store, temoin, &query, Stockage::Liste);
    connus.push((temoin, query.clone(), Stockage::Liste));

    for i in 0..80u64 {
        let n = store.create_node(&["EpisodicLesson"]);
        let e = vec_for(i);
        let mode = if i % 2 == 0 { Stockage::Veccol } else { Stockage::Liste };
        poser(&store, n, &e, mode);
        connus.push((n, e, mode));
    }
    let k = 20;
    let actual = store.vector_top_k("EpisodicLesson", "embedding", "embedding", &query, k, |_| true);
    let attendu = reference_top_k(&connus, &query, k, |_| true);
    assert_eq!(actual.len(), k);
    assert_meme_classement(&actual, &attendu);
    assert_eq!(actual[0].0, temoin, "le témoin List (cosinus 1.0) doit être #1");
}

/// Clés distinctes = schéma recall réel (embedding_vec / embedding).
#[test]
fn cles_distinctes_embedding_vec_et_embedding() {
    let store = Arc::new(SubstrateStore::open_tempfile().unwrap());
    let query = vec_for(7);
    let mut connus: Vec<(NodeId, Vec<f32>, Stockage)> = Vec::new();

    let temoin = store.create_node(&["EpisodicLesson"]);
    store.set_node_property(temoin, "embedding", as_list(&query));
    connus.push((temoin, query.clone(), Stockage::Liste));

    for i in 0..40u64 {
        let n = store.create_node(&["EpisodicLesson"]);
        let e = vec_for(i);
        if i % 2 == 0 {
            store.set_node_property(n, "embedding_vec", Value::Vector(e.clone().into()));
            connus.push((n, e, Stockage::Veccol));
        } else {
            store.set_node_property(n, "embedding", as_list(&e));
            connus.push((n, e, Stockage::Liste));
        }
    }
    let k = 15;
    let actual = store.vector_top_k("EpisodicLesson", "embedding_vec", "embedding", &query, k, |_| true);
    let attendu = reference_top_k(&connus, &query, k, |_| true);
    assert_eq!(actual.len(), k);
    assert_meme_classement(&actual, &attendu);
    assert_eq!(actual[0].0, temoin, "témoin non migré (List sous `embedding`) doit être #1");
}
