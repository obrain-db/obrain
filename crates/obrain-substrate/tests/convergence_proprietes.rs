//! #334, étape 2 — la convergence par réordonnancement.
//!
//! La règle actuelle d'écriture partagée est **« dernier arrivé dans le WAL
//! gagne »** : le rejeu saute `PropSet`/`PropDelete` et applique les
//! `PropSetInline` dans l'ordre du journal, par simple `insert`. Aucune
//! écriture ne porte d'auteur ni d'horodatage, donc l'état final dépend de
//! l'ORDRE D'ARRIVÉE, pas des opérations elles-mêmes.
//!
//! Deux répliques qui reçoivent les mêmes opérations dans des ordres différents
//! devraient converger. Aujourd'hui elles divergent — c'est ce que ce fichier
//! constate, et c'est ce que l'étape 3 (#334) doit faire passer.
//!
//! Le test de divergence est `#[ignore]` : il est ROUGE par construction tant
//! que l'étape 3 n'est pas faite, et un rouge permanent en CI ne protège rien
//! (on apprend à l'ignorer). Il s'active en retirant l'attribut, le jour où le
//! semi-treillis existe — et il doit alors passer SANS modification.
//!
//! Le second test, lui, tourne : il vérifie que le montage n'est pas vide
//! (même ordre ⇒ même état). Sans lui, une divergence pourrait venir d'un
//! harnais cassé plutôt que de la règle d'écriture.

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;

/// Une réplique isolée, avec un nœud prêt à recevoir des propriétés.
fn replique() -> (tempfile::TempDir, SubstrateStore, NodeId) {
    let td = tempfile::tempdir().expect("tempdir");
    let s = SubstrateStore::create(td.path()).expect("create");
    let n = s.create_node(&["N"]);
    (td, s, n)
}

fn lire(s: &SubstrateStore, n: NodeId) -> Option<Value> {
    s.get_node_property(n, &PropertyKey::new("x"))
}

/// Témoin : les mêmes écritures, dans le même ordre, donnent le même état.
///
/// Il passe aujourd'hui et doit continuer. S'il tombait, le montage serait en
/// cause et le test de divergence ci-dessous ne prouverait plus rien.
#[test]
fn meme_ordre_meme_etat() {
    let (_ta, a, na) = replique();
    let (_tb, b, nb) = replique();

    // Chaque réplique écrit sur SON nœud : les identifiants sont propres à
    // chaque magasin, et les mélanger ferait passer le témoin par coïncidence.
    for (s, node) in [(&a, na), (&b, nb)] {
        s.set_node_property(node, "x", Value::Int64(1));
        s.set_node_property(node, "x", Value::Int64(2));
    }

    let (va, vb) = (lire(&a, na), lire(&b, nb));
    assert_eq!(va, vb, "même ordre, états différents : le harnais est faux");
    assert_eq!(
        va,
        Some(Value::Int64(2)),
        "dernier écrit gagne, dans l'ordre"
    );
}

/// Deux répliques, les mêmes deux écritures, ordres INVERSÉS.
///
/// Une propriété en écriture partagée doit converger : l'état final ne peut pas
/// dépendre de l'ordre d'arrivée. Aujourd'hui il en dépend.
#[test]
#[ignore = "rouge par construction tant que #334 étape 3 n'est pas faite : « dernier dans le WAL gagne », sans auteur ni horodatage"]
fn deux_repliques_convergent_quel_que_soit_l_ordre() {
    let (_ta, a, na) = replique();
    let (_tb, b, nb) = replique();

    // Réplique A : x = 1 puis x = 2.
    a.set_node_property(na, "x", Value::Int64(1));
    a.set_node_property(na, "x", Value::Int64(2));

    // Réplique B : les mêmes écritures, dans l'ordre inverse.
    b.set_node_property(nb, "x", Value::Int64(2));
    b.set_node_property(nb, "x", Value::Int64(1));

    assert_eq!(
        lire(&a, na),
        lire(&b, nb),
        "les deux répliques ont reçu les mêmes écritures et divergent : la \
         résolution dépend de l'ordre d'arrivée, donc il n'y a pas de \
         convergence (voir #334 étape 3 : auteur + horloge dans la trame)"
    );
}
