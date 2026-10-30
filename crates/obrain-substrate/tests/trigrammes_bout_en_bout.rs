//! L'index de TRIGRAMMES, prouvé de bout en bout sur un vrai store.
//!
//! Ce que ce fichier vérifie, et pourquoi chacun de ces points existe :
//!
//! 1. **L'exactitude du chemin indexé.** `noeuds_par_predicat_texte` doit rendre
//!    EXACTEMENT les mêmes nœuds que le balayage — ni plus (le filtre serait
//!    faux), ni moins (le résultat serait amputé en silence).
//! 2. **L'écriture APRÈS construction.** C'est la garantie qui a motivé le
//!    tampon additif : un nœud écrit après que l'index a été bâti doit être
//!    TROUVÉ. Sans le crochet d'écriture, il serait invisible — le faux négatif
//!    que toute la doctrine de ce dépôt interdit.
//! 3. **La réécriture.** Une valeur qui ne contient plus le terme peut rester
//!    dans l'index (faux positif, assumé) : la REVÉRIFICATION doit l'écarter du
//!    résultat. Et la NOUVELLE valeur doit être trouvée.
//! 4. **Les deux silences.** Terme absent du corpus → `Some(vide)`, conclusif.
//!    Terme trop commun (tous ses grammes au-dessus du seuil) → `None`, et le
//!    balayage reprend : nier « zéro résultat » à tort serait pire que se taire.
//! 5. **L'interrupteur.** `OBRAIN_TRIGRAMMES=0` doit rendre `None` partout, sans
//!    changer un octet de la réponse — c'est l'échappatoire qui permet de
//!    mesurer le gain sur le même binaire.
//! 6. **La garde adaptative.** La PREMIÈRE demande sur une propriété ne
//!    construit RIEN : elle rend `None` et l'appelant balaie. La construction
//!    est payée à la deuxième demande — mesuré : sans cette garde, le CLI
//!    (`un processus par requête`) passait de 1,10 s à 2,32 s, en repayant
//!    2,9 s de construction pour une seule requête. Les tests ci-dessous
//!    appellent donc `chauffer()` avant de mesurer, exactement comme un
//!    processus qui vit le ferait.

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::traits::{GraphStore, GraphStoreMut, PredicatTexte};
use obrain_substrate::SubstrateStore;

/// Les identifiants des nœuds dont `name` contient `terme`, par BALAYAGE —
/// le témoin que le chemin indexé doit retrouver à l'identique.
fn par_balayage(store: &SubstrateStore, ids: &[NodeId], terme: &str) -> Vec<NodeId> {
    let cle = PropertyKey::new("name");
    let mut out: Vec<NodeId> = ids
        .iter()
        .copied()
        .filter(|id| {
            store
                .get_node_property(*id, &cle)
                .and_then(|v| match v {
                    Value::String(s) => Some(s.contains(terme)),
                    _ => None,
                })
                .unwrap_or(false)
        })
        .collect();
    out.sort_unstable_by_key(|n| n.0);
    out
}

/// La PREMIÈRE demande sur une propriété ne construit rien (garde adaptative) :
/// ce témoin la consomme, comme le ferait la première requête d'un processus.
fn chauffer(store: &SubstrateStore) {
    let _ = store.noeuds_par_predicat_texte(
        &PropertyKey::new("name"),
        &PredicatTexte::Contient("chauffage"),
    );
}

/// Les identifiants dont `name` COMMENCE par `prefixe`, par balayage — le
/// témoin du chemin `STARTS WITH`, que l'index sert depuis le 2026-09-13.
fn par_balayage_prefixe(store: &SubstrateStore, ids: &[NodeId], prefixe: &str) -> Vec<NodeId> {
    let cle = PropertyKey::new("name");
    let mut out: Vec<NodeId> = ids
        .iter()
        .copied()
        .filter(|id| {
            store
                .get_node_property(*id, &cle)
                .and_then(|v| match v {
                    Value::String(s) => Some(s.starts_with(prefixe)),
                    _ => None,
                })
                .unwrap_or(false)
        })
        .collect();
    out.sort_unstable_by_key(|n| n.0);
    out
}

fn par_index(store: &SubstrateStore, terme: &str) -> Option<Vec<NodeId>> {
    let cle = PropertyKey::new("name");
    let mut r = store.noeuds_par_predicat_texte(&cle, &PredicatTexte::Contient(terme))?;
    r.sort_unstable_by_key(|n| n.0);
    Some(r)
}

fn tous_les_ids(store: &SubstrateStore) -> Vec<NodeId> {
    let mut v: Vec<NodeId> = (1..store.slot_high_water() as u64)
        .map(NodeId::new)
        .filter(|id| store.get_node_property(*id, &PropertyKey::new("name")).is_some())
        .collect();
    v.sort_unstable_by_key(|n| n.0);
    v
}

#[test]
fn le_chemin_indexe_rend_exactement_les_memes_noeuds_que_le_balayage() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(&td.path().join("kb")).unwrap();

    // Un corpus où les termes rares et communs coexistent.
    let valeurs = [
        "iam_controller",
        "Miami_token",
        "auth_service",
        "AuthGuard",
        "token_store",
        "TokenBucket",
        "obrain-iam",
        "fonction_ordinaire",
    ];
    let mut ids = Vec::new();
    for (i, v) in valeurs.iter().enumerate() {
        let n = store.create_node(&["Test"]);
        store.set_node_property(n, "name", Value::String((*v).into()));
        store.set_node_property(n, "i", Value::Int64(i as i64));
        ids.push(n);
    }
    store.flush().unwrap();
    chauffer(&store);

    let tous = tous_les_ids(&store);
    assert_eq!(tous.len(), valeurs.len());

    for terme in ["iam", "IAM", "auth", "Auth", "token", "Token", "obrain-iam", "zzz-absent"] {
        let attendu = par_balayage(&store, &tous, terme);
        let obtenu = par_index(&store, terme)
            .unwrap_or_else(|| panic!("l'index doit répondre pour « {terme} »"));
        assert_eq!(
            obtenu, attendu,
            "« {terme} » : le chemin indexé doit rendre EXACTEMENT le balayage"
        );
        // `STARTS WITH` est servi par le MÊME index, et doit rendre exactement
        // le balayage — c'est la forme la plus banale d'une exploration de
        // graphe de code, et elle n'avait AUCUNE poussée avant le 2026-09-13.
        let pred = PredicatTexte::CommencePar("obr");
        let obtenu = GraphStore::noeuds_par_predicat_texte(&store, &PropertyKey::new("name"), &pred)
            .expect("STARTS WITH doit être servi par l'index de trigrammes");
        assert_eq!(
            obtenu,
            par_balayage_prefixe(&store, &tous, "obr"),
            "STARTS WITH : mêmes nœuds par l'index que par le balayage"
        );
    }
}

/// LA GARANTIE QUI COMPTE : écrire APRÈS que l'index est construit.
///
/// L'index est bâti au premier appel ; le nœud écrit ensuite n'existe donc
/// nulle part dans ses postings. Seul le TAMPON du crochet d'écriture peut le
/// faire connaître. Sans lui, ce nœud serait absent du résultat — sans erreur,
/// sans trace : un faux négatif.
#[test]
fn une_ecriture_apres_construction_est_trouvee() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(&td.path().join("kb")).unwrap();

    let a = store.create_node(&["Test"]);
    store.set_node_property(a, "name", Value::String("alpha".into()));
    store.flush().unwrap();

    // 1. Deuxième appel : l'index se construit ICI, sans le nœud à venir.
    chauffer(&store);
    let avant = par_index(&store, "zzqq").expect("réponse de l'index");
    assert!(avant.is_empty(), "« zzqq » n'existe pas encore : zéro, conclusif");

    // 2. ÉCRITURE APRÈS construction.
    let b = store.create_node(&["Test"]);
    store.set_node_property(b, "name", Value::String("contient-zzqq-maintenant".into()));

    // 3. Le nœud neuf DOIT sortir — et le balayage doit dire la même chose.
    let obtenu = par_index(&store, "zzqq").expect("réponse de l'index");
    let attendu = par_balayage(&store, &tous_les_ids(&store), "zzqq");
    assert_eq!(obtenu, attendu, "le nœud écrit après la construction doit être trouvé");
    assert!(obtenu.contains(&b), "c'est bien le nœud NEUF qui doit sortir");
}

/// Réécrire une valeur : l'ancien posting peut rester (faux positif assumé),
/// mais le RÉSULTAT doit être exact — c'est la revérification qui tranche.
#[test]
fn une_reecriture_donne_un_resultat_exact() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(&td.path().join("kb")).unwrap();

    let a = store.create_node(&["Test"]);
    store.set_node_property(a, "name", Value::String("ancien-mot".into()));
    store.flush().unwrap();
    // Construction de l'index sur la valeur d'origine.
    chauffer(&store);
    assert_eq!(par_index(&store, "ancien").unwrap(), par_balayage(&store, &tous_les_ids(&store), "ancien"));

    // La valeur est REMPLACÉE : elle ne contient plus « ancien ».
    store.set_node_property(a, "name", Value::String("nouveau-mot".into()));

    let tous = tous_les_ids(&store);
    assert_eq!(
        par_index(&store, "nouveau").unwrap(),
        par_balayage(&store, &tous, "nouveau"),
        "la NOUVELLE valeur doit être trouvée"
    );
    assert_eq!(
        par_index(&store, "ancien").unwrap(),
        par_balayage(&store, &tous, "ancien"),
        "l'ancienne valeur ne doit plus sortir, même si son posting traîne"
    );
    assert!(
        par_index(&store, "ancien").unwrap().is_empty(),
        "le nœud réécrit ne contient plus « ancien » : le verdict doit l'écarter"
    );
}

/// Deux silences qu'il ne faut pas confondre : `Some(vide)` = je SAIS qu'il n'y
/// a rien ; `None` = je ne sais pas, et l'appelant balaie.
#[test]
fn les_deux_silences_ne_se_confondent_pas() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(&td.path().join("kb")).unwrap();

    // Un gramme rendu COMMUN : 1 100 valeurs le portent, au-delà du seuil par
    // défaut (1024). Le terme « aaa » ne sera donc servi par AUCUN gramme.
    for i in 0..1_100u32 {
        let n = store.create_node(&["Test"]);
        store
            .set_node_property(n, "name", Value::String(format!("xxaaa{i:05}").into()));
    }
    store.flush().unwrap();
    chauffer(&store);

    assert_eq!(
        par_index(&store, "aaa"),
        None,
        "tous les grammes du terme sont communs : l'index se TAIT, il ne nie pas"
    );
    assert_eq!(
        par_index(&store, "zzqq-absent"),
        Some(Vec::new()),
        "aucun gramme n'existe dans le corpus : l'absence est PROUVÉE, on conclut"
    );
    // Et le balayage, lui, trouve bel et bien les 1 100 porteurs de « aaa ».
    assert_eq!(par_balayage(&store, &tous_les_ids(&store), "aaa").len(), 1_100);
}

/// LA GARDE ADAPTATIVE, FIGÉE ICI — c'est elle qui empêche le remède d'être
/// pire que le mal. Sans elle, un processus qui ne pose qu'une requête paie la
/// construction entière (mesuré au CLI : 1,10 s → 2,32 s).
#[test]
fn la_premiere_demande_ne_construit_pas_l_index() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(&td.path().join("kb")).unwrap();
    let a = store.create_node(&["Test"]);
    store.set_node_property(a, "name", Value::String("iam_controller".into()));
    store.flush().unwrap();

    let cle = PropertyKey::new("name");
    // 1re demande : l'index n'existe pas encore, on rend `None` — donc le
    // balayage, exactement le coût d'avant l'index.
    assert_eq!(
        store.noeuds_par_predicat_texte(&cle, &PredicatTexte::Contient("iam")),
        None,
        "première demande : pas de construction, l'appelant balaie"
    );
    // 2e demande : l'index se construit, et il répond — exactement.
    assert_eq!(
        store.noeuds_par_predicat_texte(&cle, &PredicatTexte::Contient("iam")),
        Some(vec![a]),
        "deuxième demande : l'index est construit et sert la requête"
    );
    // 3e demande : servie par l'index, sans reconstruction.
    assert_eq!(
        store.noeuds_par_predicat_texte(&cle, &PredicatTexte::Contient("iam")),
        Some(vec![a])
    );
}
