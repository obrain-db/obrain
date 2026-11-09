//! SCAN-PLAFONNE — le contrat de `node_ids_capped`, au niveau du substrat.
//!
//! Chantier : « qu'un `LIMIT 30` lise 30 slots, pas 3,5 millions ».
//!
//! Ce fichier ne teste PAS la vitesse. Il teste les deux proprietes dont
//! depend la correction du plafond, et une troisieme qui est la preuve
//! comptable du gain :
//!
//!   1. PREFIXE — `node_ids_capped(k)` rend EXACTEMENT les k premiers de
//!      `node_ids()`. Pas « k noeuds vivants quelconques » : les k PREMIERS.
//!      Sans cela, substituer l'un a l'autre sous un `LIMIT` changerait les
//!      lignes rendues, pas seulement leur cout.
//!
//!   2. VIVANTS, PAS SLOTS — le plafond compte les noeuds rendus. Un
//!      substrat troue (tombstones, creneaux jamais ecrits) doit rendre k
//!      entrees tant qu'il en existe k. S'arreter a k slots LUS en rendrait
//!      moins : c'est la faute que le brief appelle « le piege des trous ».
//!
//!   3. COMPTAGE — le compteur de slots lus, qui est deterministe la ou le
//!      chronometre de cette machine ne l'est pas (`BANCS.md` : -39,3 % puis
//!      +63,7 %, p = 0,00, sur du code identique).
//!
//!   cargo test -p obrain-substrate --test scan_plafonne

use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use obrain_substrate::compteurs::mesurer_slots;
use std::sync::Arc;

fn store_de(n: usize) -> Arc<dyn GraphStoreMut> {
    let s: Arc<dyn GraphStoreMut> = Arc::new(SubstrateStore::open_tempfile().unwrap());
    for _ in 0..n {
        s.create_node(&["N"]);
    }
    s
}

// ───────────────────────── 1. le contrat de prefixe ─────────────────────────

#[test]
fn capped_est_le_prefixe_exact_de_node_ids() {
    let s = store_de(500);
    let tout = s.node_ids();
    assert_eq!(tout.len(), 500, "temoin : le store porte bien 500 noeuds");

    for k in [0usize, 1, 2, 30, 499, 500] {
        let cap = s.node_ids_capped(k);
        assert_eq!(cap.len(), k.min(tout.len()), "longueur pour k={k}");
        assert_eq!(cap, tout[..k.min(tout.len())], "prefixe exact pour k={k}");
    }
}

#[test]
fn capped_au_dela_du_compte_rend_tout_sans_rembourrage() {
    let s = store_de(17);
    let tout = s.node_ids();
    // Un `LIMIT` plus grand que la base ne doit pas inventer de lignes ni en
    // perdre — et surtout ne pas boucler en attendant d'atteindre `max`.
    assert_eq!(s.node_ids_capped(1_000), tout);
    assert_eq!(s.node_ids_capped(usize::MAX), tout);
}

// ──────────────── 2. vivants, pas slots : tombstones et trous ────────────────

#[test]
fn le_plafond_compte_les_vivants_pas_les_slots_parcourus() {
    let s = store_de(400);
    let tout_avant = s.node_ids();

    // On tue les 300 PREMIERS slots. Un plafond qui compterait les slots
    // PARCOURUS s'arreterait au coeur du cimetiere et rendrait zero ligne
    // pour `k = 30` ; celui qui compte les vivants doit en rendre 30.
    for id in tout_avant.iter().take(300) {
        assert!(s.delete_node(*id), "suppression du noeud {id:?}");
    }

    let vivants = s.node_ids();
    assert_eq!(vivants.len(), 100, "100 survivants attendus");

    let cap = s.node_ids_capped(30);
    assert_eq!(cap.len(), 30, "30 VIVANTS, pas 30 slots lus");
    assert_eq!(cap, vivants[..30], "et ce sont les 30 premiers survivants");
}

#[test]
fn capped_reste_un_prefixe_meme_troue_en_alternance() {
    // Tombstones en alternance : le pire cas pour un compteur de slots, et le
    // plus proche des creneaux jamais ecrits que laisse l'allocation par
    // communaute.
    let s = store_de(200);
    let tout = s.node_ids();
    for (i, id) in tout.iter().enumerate() {
        if i % 2 == 0 {
            s.delete_node(*id);
        }
    }
    let vivants = s.node_ids();
    assert_eq!(vivants.len(), 100);
    for k in [1usize, 7, 50, 100, 150] {
        let cap = s.node_ids_capped(k);
        assert_eq!(cap, vivants[..k.min(vivants.len())], "prefixe pour k={k}");
    }
}

#[test]
fn store_vide_rend_vide_et_ne_boucle_pas() {
    let s: Arc<dyn GraphStoreMut> = Arc::new(SubstrateStore::open_tempfile().unwrap());
    assert!(s.node_ids_capped(30).is_empty());
    assert!(s.node_ids().is_empty());
}

// ─────────────────────── 3. la preuve comptable du gain ───────────────────────

#[test]
fn capped_lit_un_nombre_de_slots_proportionnel_a_k_pas_a_n() {
    let s = store_de(20_000);

    let (tout, slots_tout) = mesurer_slots(|| s.node_ids());
    let (cap, slots_cap) = mesurer_slots(|| s.node_ids_capped(30));

    assert_eq!(tout.len(), 20_000);
    assert_eq!(cap.len(), 30);

    // Le scan complet lit tous les slots ; le plafonne s'arrete a 30. C'est
    // la propriete qu'on est venu chercher, et elle est COMPTABLE : aucun
    // chronometre, donc aucune dependance a la charge de la machine.
    assert!(
        slots_tout >= 20_000,
        "le scan complet doit lire tous les slots, lu : {slots_tout}"
    );
    assert_eq!(
        slots_cap, 30,
        "le scan plafonne doit lire EXACTEMENT 30 slots (aucun tombstone \
         ici, donc un slot par noeud), lu : {slots_cap}"
    );
}

#[test]
fn capped_ne_parcourt_pas_tout_pour_decouper_ensuite() {
    // La dette que le brief interdit de recontracter : « tout parcourir puis
    // tronquer » rendrait le BON resultat avec le MAUVAIS cout. Le test de
    // prefixe ci-dessus serait vert dans les deux cas ; seul le compteur
    // separe les deux implementations.
    let s = store_de(50_000);
    let (_, slots) = mesurer_slots(|| s.node_ids_capped(10));
    assert!(
        slots < 1_000,
        "un plafond qui s'arrete vraiment lit ~10 slots ; « parcourir puis \
         tronquer » en lirait 50 000. Lu : {slots}"
    );
}

#[test]
fn le_plafond_traverse_le_cimetiere_sans_le_compter_comme_resultat() {
    // Verification croisee du compteur : avec 300 tombstones en tete, rendre
    // 30 vivants coute 330 slots lus — la preuve directe que le plafond
    // compte les vivants tout en lisant les morts.
    let s = store_de(400);
    let tout = s.node_ids();
    for id in tout.iter().take(300) {
        s.delete_node(*id);
    }
    let (cap, slots) = mesurer_slots(|| s.node_ids_capped(30));
    assert_eq!(cap.len(), 30);
    assert_eq!(
        slots, 330,
        "300 morts traverses + 30 vivants retenus = 330 slots lus"
    );
}

// ─────────────── 4. le predicat de surete, tel que le substrat le rend ───────────────

#[test]
fn visibilite_triviale_sur_une_base_sans_transaction() {
    // C'est l'etat permanent d'une base qui n'ouvre pas de transaction — et
    // la condition qui ARME le plafond dans `ScanOperator`. Si ce test
    // devenait rouge, le plafond se desarmerait partout : lent, mais jamais
    // faux.
    let s = store_de(10);
    assert!(
        s.scan_visibility_is_trivial(),
        "aucune carte MVCC peuplee : le filtre de visibilite est l'identite"
    );
}
