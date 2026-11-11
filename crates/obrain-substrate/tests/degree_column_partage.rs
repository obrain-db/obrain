//! Régression — colonne de degrés en écriture partagée.
//!
//! Deux écrivains ouvrent la même colonne. Le second la fait grandir ; le
//! premier, resté sur son `n_slots` périmé, écrit son en-tête à la fermeture.
//! Sans correctif, cet en-tête ramène la colonne à la taille périmée : les slots
//! du second disparaissent, et le CRC ne correspond plus à l'ouverture suivante.
//!
//! Ce test simule les deux écrivains par deux poignées dans le même processus :
//! la logique d'en-tête est la même que pour deux processus.

use obrain_substrate::SubstrateFile;
use obrain_substrate::degree_column::DegreeColumn;

#[test]
fn l_ecrivain_au_compteur_perime_ne_reduit_pas_la_colonne() {
    let sub = SubstrateFile::open_tempfile().expect("substrat temporaire");
    let mut a = DegreeColumn::create(&sub, 10).expect("création");
    // Comme le moteur : le CRC est posé à la création, sinon la colonne est rejetée.
    a.persist_header_crc();
    let mut b = DegreeColumn::open(&sub)
        .expect("ouverture b")
        .expect("présente");

    // `b` fait grandir la colonne bien au-delà du compteur de `a`.
    b.ensure_slot(40).expect("croissance par b");
    b.persist_header_crc();

    // `a` n'a jamais vu cette croissance : il persiste SON en-tête, à 10 slots.
    a.persist_header_crc();

    drop(a);
    drop(b);

    let c = DegreeColumn::open(&sub)
        .expect("réouverture")
        .expect("la colonne doit se rouvrir avec un CRC valide");
    assert!(
        c.n_slots() >= 41,
        "la colonne a été réduite par un écrivain au compteur périmé : {} slots, 41 attendus",
        c.n_slots()
    );
}
