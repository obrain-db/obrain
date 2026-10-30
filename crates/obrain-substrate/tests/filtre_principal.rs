//! FILTRE PAR PRINCIPAL — le contrat du jalon 3 (RFC principal-natif-v2,
//! arbitrages A1/A3/A5).
//!
//! C'est le jalon ou le multi-tenant devient EFFECTIF : un noeud d'autrui
//! est invisible AU NIVEAU DU SUBSTRAT — scans, seeks, index, traversees,
//! compteurs — incontournable meme en acces direct au store.
//!
//! La semantique, en une ligne :
//!
//!   visible(noeud, principal) = is_live()
//!       ET (pas de principal → tout ;
//!           principal t     → noeud.tenant == t OU noeud.tenant == PUBLIC)
//!   ... et la sentinelle `u32::MAX` n'est visible de PERSONNE, meme sans
//!   principal (fail-closed A5 : un noeud non migre n'existe pas).
//!
//! Le banc : UN store, DEUX tenants (A=7, B=9), du public, une sentinelle,
//! des aretes qui les traversent — puis chaque primitive interrogee depuis
//! chaque point de vue (sans principal, A, B, public pur). Les tenants sont
//! forges par patch DISQUE des octets 32..36 du record (comme les sondes du
//! jalon 2) : aucun writer ne sait poser autre chose que le tenant du
//! record, et c'est tres bien ainsi.
//!
//!   cargo test -p obrain-substrate --test filtre_principal

use obrain_common::types::{EdgeId, EpochId, NodeId, PropertyKey, TransactionId, Value};
use obrain_core::graph::Direction;
use obrain_core::graph::GraphStore;
use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::PrincipalSubstrat;
use obrain_substrate::SubstrateStore;
use obrain_substrate::error::SubstrateError;
use obrain_substrate::record::{NodeRecord, tenant};
use std::path::Path;

const TENANT_A: u32 = 7;
const TENANT_B: u32 = 9;

// ---------------------------------------------------------------------------
// Fixture : le store bitenant + public + sentinelle
// ---------------------------------------------------------------------------

/// Patch DISQUE du `tenant_id` du record `slot` (octets 32..36, stride 40).
/// Se fait store FERME — le prochain open mappe la zone patchee.
fn poser_tenant(dir: &Path, slot: u32, t: u32) {
    use std::io::{Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.join("substrate.nodes"))
        .unwrap();
    f.seek(SeekFrom::Start(slot as u64 * NodeRecord::SIZE as u64 + 32))
        .unwrap();
    f.write_all(&t.to_le_bytes()).unwrap();
    f.sync_all().unwrap();
}

/// Le banc commun. Slots (create_node alloue sequentiellement depuis 1) :
///
///   1..=10  label Doc — deviendront TENANT B (9)   [b1 = slot 1]
///   11..=20 label Doc — deviendront TENANT A (7)   [a1 = 11, a2 = 12]
///   21..=25 label Doc — restent PUBLIC (0)         [p1 = 21]
///   26      label Fantome — deviendra SENTINELLE   [s = 26]
///
/// Proprietes (posees AVANT le patch, par les writers publics) :
///   k   = "commun"        sur b1, a1, p1, s     (route egalite)
///   txt = "le papillon.." sur b1, a1, p1        (route trigrammes/CONTAINS)
///   n   = 1, 2, 3, 4      sur b1, a1, p1, s     (route ordonnee/intervalle)
///
/// Aretes (type T) :  e1 a1->b1 ; e2 a1->p1 ; e3 b1->p1 ; e4 p1->s ;
///                    e5 a1->a2.  e1 porte w=42.
///
/// Rend le chemin ; chaque test ouvre ensuite le point de vue qu'il veut.
fn banc_bitenant(dir: &Path) {
    let store = SubstrateStore::create(dir).expect("create");
    for _ in 1..=25 {
        store.create_node(&["Doc"]);
    }
    let s = store.create_node(&["Fantome"]);
    assert_eq!(s.0, 26, "l'allocation sequentielle est le socle du banc");

    let (b1, a1, a2, p1) = (id(1), id(11), id(12), id(21));
    for n in [b1, a1, p1, s] {
        store.set_node_property(n, "k", Value::String("commun".into()));
    }
    for n in [b1, a1, p1] {
        store.set_node_property(n, "txt", Value::String("le papillon vole".into()));
    }
    for (n, v) in [(b1, 1i64), (a1, 2), (p1, 3), (s, 4)] {
        store.set_node_property(n, "n", Value::Int64(v));
    }

    let e1 = store.create_edge(a1, b1, "T");
    store.create_edge(a1, p1, "T");
    store.create_edge(b1, p1, "T");
    store.create_edge(p1, s, "T");
    store.create_edge(a1, a2, "T");
    store.set_edge_property(e1, "w", Value::Int64(42));

    store.flush().expect("flush");
    drop(store);

    for slot in 1..=10 {
        poser_tenant(dir, slot, TENANT_B);
    }
    for slot in 11..=20 {
        poser_tenant(dir, slot, TENANT_A);
    }
    poser_tenant(dir, 26, tenant::SENTINELLE_NON_MIGRE);
}

fn id(slot: u64) -> NodeId {
    NodeId(slot)
}

fn ouvrir(dir: &Path, principal: Option<u32>) -> SubstrateStore {
    let store = SubstrateStore::open(dir).expect("open");
    if let Some(t) = principal {
        store
            .avec_principal(PrincipalSubstrat::nouveau(t).unwrap())
            .expect("pose du principal");
    }
    store
}

fn slots(ids: &[NodeId]) -> Vec<u64> {
    ids.iter().map(|n| n.0).collect()
}

// ---------------------------------------------------------------------------
// 1. Le type : immuable, et la sentinelle irrecevable
// ---------------------------------------------------------------------------

#[test]
fn principal_immuable_une_fois_pose_et_sentinelle_irrecevable() {
    let td = tempfile::tempdir().unwrap();
    banc_bitenant(td.path());
    let store = ouvrir(td.path(), Some(TENANT_A));

    // Re-poser — meme identite ou une autre — est refuse.
    let err = store
        .avec_principal(PrincipalSubstrat::nouveau(TENANT_B).unwrap())
        .unwrap_err();
    assert!(
        matches!(err, SubstrateError::PrincipalDejaPose { existant } if existant == TENANT_A),
        "re-pose refusee avec l'identite existante : {err}"
    );
    // Et le principal n'a PAS bouge.
    assert_eq!(store.principal().unwrap().tenant_id(), TENANT_A);

    // La sentinelle ne peut pas devenir une portee de lecture.
    assert!(matches!(
        PrincipalSubstrat::nouveau(u32::MAX),
        Err(SubstrateError::PrincipalIrrecevable(_))
    ));
}

// ---------------------------------------------------------------------------
// 2. COMPAT : sans principal, tout est visible — sauf la sentinelle
// ---------------------------------------------------------------------------

#[test]
fn sans_principal_tout_visible_sauf_sentinelle() {
    let td = tempfile::tempdir().unwrap();
    banc_bitenant(td.path());
    let store = ouvrir(td.path(), None);

    // COMPTEURS DE SLOTS COMME PREUVE : 26 crees, 25 visibles — le 26e est
    // la sentinelle, exclue de TOUTES les vues, atomiques d'ouverture
    // comprises (node_count == node_ids().len(), l'invariant tient).
    assert_eq!(store.node_ids().len(), 25);
    assert_eq!(store.node_count(), 25);
    assert_eq!(store.node_count_by_label("Doc"), 25);
    assert_eq!(store.node_count_by_label("Fantome"), 0, "sentinelle : 0");
    assert_eq!(store.nodes_by_label("Fantome"), vec![]);
    assert!(store.get_node(id(26)).is_none(), "seek sentinelle : absent");
    assert!(
        store
            .get_node_property(id(26), &PropertyKey::new("k"))
            .is_none()
    );

    // Les deux tenants et le public se lisent comme avant.
    for slot in [1u64, 11, 21] {
        assert!(store.get_node(id(slot)).is_some(), "slot {slot} visible");
    }
    // Les routes indexees rendent tenants confondus — sentinelle exclue.
    store.create_property_index("k");
    let hits = store.find_nodes_by_property("k", &Value::String("commun".into()));
    assert_eq!(slots(&hits), vec![1, 11, 21], "eq : tous sauf sentinelle");
    let bornes = store.find_nodes_in_range(
        "n",
        Some(&Value::Int64(0)),
        Some(&Value::Int64(10)),
        true,
        true,
    );
    assert_eq!(slots(&bornes), vec![1, 11, 21], "range : idem");
}

// ---------------------------------------------------------------------------
// 3. Les primitives, depuis A, depuis B, depuis le public pur
// ---------------------------------------------------------------------------

#[test]
fn scans_et_seeks_depuis_chaque_point_de_vue() {
    let td = tempfile::tempdir().unwrap();
    banc_bitenant(td.path());

    // Depuis A : ses 10 noeuds + les 5 publics. Jamais B, jamais la
    // sentinelle.
    let a = ouvrir(td.path(), Some(TENANT_A));
    assert_eq!(
        slots(&a.node_ids()),
        (11..=25).collect::<Vec<u64>>(),
        "A voit A + public, dans l'ordre des slots"
    );
    assert_eq!(a.node_count(), 15);
    assert_eq!(a.node_count_by_label("Doc"), 15);
    assert!(a.get_node(id(1)).is_none(), "seek d'un noeud de B : absent");
    assert!(a.get_node(id(26)).is_none(), "seek sentinelle : absent");
    assert!(a.get_node(id(21)).is_some(), "le public se lit");
    assert!(
        a.get_node_property(id(1), &PropertyKey::new("k")).is_none(),
        "propriete d'autrui : absente"
    );
    assert_eq!(
        a.filter_visible_node_ids(&[id(1), id(11), id(21), id(26)], EpochId::new(0))
            .len(),
        2,
        "filtre par lot : A + public seulement"
    );
    drop(a);

    // Depuis B : symetrique.
    let b = ouvrir(td.path(), Some(TENANT_B));
    let vus: Vec<u64> = slots(&b.node_ids());
    assert_eq!(
        vus,
        (1..=10).chain(21..=25).collect::<Vec<u64>>(),
        "B voit B + public"
    );
    assert_eq!(b.node_count(), 15);
    assert!(
        b.get_node(id(11)).is_none(),
        "seek d'un noeud de A : absent"
    );
    drop(b);

    // Principal PUBLIC (tenant 0) : ne voit QUE le public — legal, utile
    // aux services jamais tenantes.
    let p = ouvrir(td.path(), Some(tenant::PUBLIC));
    assert_eq!(slots(&p.node_ids()), (21..=25).collect::<Vec<u64>>());
    assert_eq!(p.node_count(), 5);
}

// ---------------------------------------------------------------------------
// 4. LIMIT sous filtre : le plafond compte les VISIBLES — pas de
//    sous-livraison (le piege du chantier scan-plafonne, sous tenant)
// ---------------------------------------------------------------------------

#[test]
fn plafond_sous_filtre_sans_sous_livraison() {
    let td = tempfile::tempdir().unwrap();
    banc_bitenant(td.path());
    let a = ouvrir(td.path(), Some(TENANT_A));

    // Les 10 premiers slots sont a B : un plafond naif « max slots LUS »
    // rendrait 2 resultats au lieu de 12. Le plafond compte APRES le
    // predicat complet.
    let douze = a.node_ids_capped(12);
    assert_eq!(douze.len(), 12, "LIMIT 12 rend 12 (il y a 15 visibles)");
    assert_eq!(
        slots(&douze),
        (11..=22).collect::<Vec<u64>>(),
        "et c'est LE PREFIXE de node_ids()"
    );
    let douze_l = a.nodes_by_label_capped("Doc", 12);
    assert_eq!(slots(&douze_l), (11..=22).collect::<Vec<u64>>());

    // Plafond au-dela du visible : tout le visible, pas plus.
    assert_eq!(a.node_ids_capped(100).len(), 15);
    assert_eq!(a.nodes_by_label_capped("Doc", 100).len(), 15);
}

// ---------------------------------------------------------------------------
// 5. Les TROIS routes indexees ne fuient pas : la revarification applique
//    la visibilite (les candidats, eux, peuvent venir de tous les tenants)
// ---------------------------------------------------------------------------

#[test]
fn les_trois_routes_indexees_ne_fuient_pas() {
    let td = tempfile::tempdir().unwrap();
    banc_bitenant(td.path());
    let a = ouvrir(td.path(), Some(TENANT_A));
    a.create_property_index("k");
    a.create_property_index("txt");

    // Route EGALITE : b1 (slot 1) et la sentinelle portent la meme valeur
    // — seuls a1 (11) et p1 (21) sortent.
    let eq = a.find_nodes_by_property("k", &Value::String("commun".into()));
    assert_eq!(slots(&eq), vec![11, 21], "eq-index : pas de fuite");

    // Route TRIGRAMMES / CONTAINS : trois porteurs de « pillon », deux
    // visibles. `Some(..)` prouve que la route INDEXEE a repondu (None
    // signifierait « l'appelant balaie » — un autre chemin que celui-ci).
    // (2026-09-16) La construction de l'index de trigrammes est OPT-IN a la
    // DEUXIEME demande (`trigrammes_des_la` = 2 : la premiere requete balaie,
    // cout connu et sans regression ; la deuxieme construit). Ce test vient
    // d'une lignee ou `create_property_index` batissait immediatement : une
    // seule demande rendait donc `None` — non pas une fuite, mais un balayage,
    // et l'assertion ci-dessous ne mettait plus rien a l'epreuve. On demande
    // donc deux fois : c'est bien la route INDEXEE qu'on veut eprouver.
    let _amorce = a.find_nodes_by_property_contains("txt", "pillon");
    let ct = a
        .find_nodes_by_property_contains("txt", "pillon")
        .expect("la route indexee doit repondre : cle declaree et batie");
    assert_eq!(slots(&ct), vec![11, 21], "trigrammes : pas de fuite");

    // Route ORDONNEE / INTERVALLE : n dans [0, 10] — b1 (n=1) et la
    // sentinelle (n=4) restent dehors.
    let rg = a.find_nodes_in_range(
        "n",
        Some(&Value::Int64(0)),
        Some(&Value::Int64(10)),
        true,
        true,
    );
    assert_eq!(slots(&rg), vec![11, 21], "intervalle : pas de fuite");

    // Multi-criteres, meme regle.
    let multi = a.find_nodes_by_properties(&[("k", Value::String("commun".into()))]);
    assert_eq!(slots(&multi), vec![11, 21]);
}

// ---------------------------------------------------------------------------
// 6. Cascade aretes (A1) : far-endpoint invisible => ni l'arete ni le
//    voisin ; le re-ancrage entrant compose (zero resultat, pas d'erreur)
// ---------------------------------------------------------------------------

#[test]
fn cascade_aretes_et_reancrage_entrant() {
    let td = tempfile::tempdir().unwrap();
    banc_bitenant(td.path());
    let (b1, a1, p1) = (id(1), id(11), id(21));

    // SANS principal : seule l'arete vers la sentinelle disparait.
    let s0 = ouvrir(td.path(), None);
    let mut voisins = slots(&s0.neighbors(a1, Direction::Outgoing));
    voisins.sort_unstable();
    assert_eq!(voisins, vec![1, 12, 21], "sans principal : b1, a2, p1");
    assert_eq!(
        s0.neighbors(p1, Direction::Outgoing),
        vec![],
        "l'arete p1->sentinelle n'existe pour personne"
    );
    assert_eq!(GraphStore::out_degree(&s0, p1), 0, "degre coherent");
    assert_eq!(GraphStore::out_degree(&s0, a1), 3);
    assert_eq!(GraphStore::in_degree(&s0, p1), 2, "a1 et b1 pointent p1");
    drop(s0);

    // Depuis A.
    let a = ouvrir(td.path(), Some(TENANT_A));
    let mut voisins = slots(&a.neighbors(a1, Direction::Outgoing));
    voisins.sort_unstable();
    assert_eq!(
        voisins,
        vec![12, 21],
        "a1 -> b1 est cachee (far endpoint chez B) ; a2 et p1 restent"
    );
    assert_eq!(
        a.edges_from(b1, Direction::Both),
        vec![],
        "source invisible : aucune arete, pas d'erreur"
    );
    assert_eq!(
        slots(&a.neighbors(p1, Direction::Incoming)),
        vec![11],
        "entrant sur le public : la branche de B est cachee"
    );
    // RE-ANCRAGE ENTRANT compose : la cible contrainte (b1, chez B) est
    // invisible => zero resultat, pas d'erreur — l'equivalent substrat de
    // MATCH (x)-->(cible {chez B}) depuis A.
    assert_eq!(a.edges_from(b1, Direction::Incoming), vec![]);

    // Degres = longueurs d'edges_from, sous toute portee.
    assert_eq!(GraphStore::out_degree(&a, a1), 2);
    assert_eq!(GraphStore::in_degree(&a, p1), 1);
    assert_eq!(GraphStore::out_degree(&a, b1), 0, "source invisible");
    assert_eq!(
        GraphStore::out_degree_by_type(&a, a1, Some("T")),
        2,
        "degre type filtre sous principal"
    );
    assert_eq!(GraphStore::out_degree_by_type(&a, a1, None), 2);
    assert!(
        !a.typed_degree_reliable("T"),
        "sous principal, les sidecars O(1) sont declares non fiables"
    );

    // Seek unitaire d'une arete d'autrui (e1 = a1->b1, premiere arete).
    let e1 = EdgeId(1);
    assert!(a.get_edge(e1).is_none(), "arete vers B : absente depuis A");
    assert!(
        a.get_edge_property(e1, &PropertyKey::new("w")).is_none(),
        "metadonnee d'arete d'autrui : absente"
    );
    drop(a);

    // Depuis B : miroir.
    let b = ouvrir(td.path(), Some(TENANT_B));
    assert_eq!(b.edges_from(a1, Direction::Both), vec![], "source chez A");
    assert_eq!(slots(&b.neighbors(p1, Direction::Incoming)), vec![1]);
    let e1 = EdgeId(1);
    assert!(
        b.get_edge(e1).is_none(),
        "a1->b1 invisible AUSSI de B : une extremite chez A suffit"
    );
    // Sans principal, la meme arete se lit (temoin de compat).
    drop(b);
    let s0 = ouvrir(td.path(), None);
    assert!(s0.get_edge(e1).is_some());
    assert_eq!(
        s0.get_edge_property(e1, &PropertyKey::new("w")),
        Some(Value::Int64(42))
    );
}

// ---------------------------------------------------------------------------
// 7. MVCC : les trois sites rendent le MEME verdict pour un (noeud, portee),
//    cartes vides ou non — le tenant n'a pas de chemin rapide
// ---------------------------------------------------------------------------

#[test]
fn mvcc_meme_verdict_cartes_vides_ou_peuplees() {
    let td = tempfile::tempdir().unwrap();
    banc_bitenant(td.path());
    let a = ouvrir(td.path(), Some(TENANT_A));
    let (b1, a1) = (id(1), id(11));
    let ep = EpochId::new(0);

    // Cartes vides (chemin rapide `visibilite_triviale`).
    assert!(!a.is_node_visible_at_epoch(b1, ep), "B invisible — trivial");
    assert!(a.is_node_visible_at_epoch(a1, ep), "A visible — trivial");

    // On peuple les cartes MVCC (noeud versionne d'une tx explicite) : le
    // chemin COMPLET prend le relais — memes verdicts, y compris pour le
    // noeud en attente d'une AUTRE transaction.
    let tx = TransactionId::new(41);
    let pending = a.create_node_versioned(&["Doc"], EpochId::new(1), tx);
    assert!(
        !a.is_node_visible_at_epoch(b1, EpochId::new(2)),
        "B invisible — chemin complet"
    );
    assert!(
        a.is_node_visible_at_epoch(a1, EpochId::new(2)),
        "A visible — chemin complet"
    );
    assert!(
        a.is_node_visible_versioned(pending, EpochId::new(2), tx),
        "sa transaction voit son noeud"
    );
    assert!(
        !a.is_node_visible_versioned(pending, EpochId::new(2), TransactionId::new(99)),
        "une autre transaction ne le voit pas"
    );
    // Et le voyage dans le temps ne traverse pas les tenants.
    assert!(a.get_node_at_epoch(b1, EpochId::new(2)).is_none());
    assert!(a.get_node_at_epoch(a1, EpochId::new(2)).is_some());
}

// ---------------------------------------------------------------------------
// 8. Fail-closed sur les ecritures gatees : le noeud d'autrui est un no-op,
//    jamais une erreur qui fuit son existence
// ---------------------------------------------------------------------------

#[test]
fn ecritures_gatees_fail_closed_sur_autrui() {
    let td = tempfile::tempdir().unwrap();
    banc_bitenant(td.path());
    let a = ouvrir(td.path(), Some(TENANT_A));
    let b1 = id(1);

    assert!(!a.delete_node(b1), "delete d'un noeud de B : no-op");
    assert!(
        a.remove_node_property(b1, "k").is_none(),
        "remove property d'autrui : no-op"
    );
    drop(a);

    // Et rien n'a bouge : sans principal, b1 est intact.
    let s0 = ouvrir(td.path(), None);
    assert!(s0.get_node(b1).is_some(), "b1 vit toujours");
    assert_eq!(
        s0.get_node_property(b1, &PropertyKey::new("k")),
        Some(Value::String("commun".into())),
        "sa propriete aussi"
    );
}
