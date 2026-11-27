//! Témoins COGNITIFS des poids d'engramme NON uniformes (T5 — `exp/t5-poids`).
//!
//! Ces bancs exercent, DEPUIS la couche cognitive (`EngramStore` / `EngramManager`),
//! les chemins de persistance et de relecture des poids d'engramme avec des
//! ensembles NON uniformément pondérés. Ils ne réécrivent PAS le câblage
//! (`mirror_substrate_insert` → `set_engram_state`, format V4) : ils mesurent les
//! garanties exposées par ce câblage depuis la couche qui le consomme.
//!
//! Périmètre (8 items) :
//!   1. formation d'un ensemble non uniforme → poids persistés exacts ;
//!   2. révision d'un poids → pas de demi-état (couple lié complet) ;
//!   3. reconsolidation adaptative → projection spectrale reconstruite ;
//!   4. `rehydrate_signature` reconstruit sans réviser (et sans write-through) ;
//!   5. rappel : associations membres/poids exactes préservées au rechargement ;
//!   6. fermeture/réouverture → poids intacts ;
//!   7. refus sans effet partiel (poids non représentable, NodeId non-u32) ;
//!   8. coordination T6 : effacement+rejeu et mutation « membres seuls »
//!      (remappage) → la protection V4 tient (poids non réassociés).
//!
//! NE DUPLIQUE PAS les témoins legacy du validateur substrate (0x76/0x77, ABA,
//! suppression/recréation) : ceux-là sont couverts par
//! `obrain-substrate/tests/autorite_etat_engram.rs`.

#![cfg(all(feature = "engram", feature = "substrate"))]

use std::sync::Arc;

use obrain_cognitive::InMemoryVectorIndex;
use obrain_cognitive::engram::{
    EngramId, EngramManager, EngramManagerConfig, EngramStore, ObservationScope, StoreError,
};
use obrain_common::types::NodeId;
use obrain_substrate::replay::replay_from;
use obrain_substrate::{SubstrateFile, SyncMode, Writer as SubstrateWriter};

/// Poids (f64) d'un membre dans un ensemble EN CACHE.
fn weight_of(ens: &[(NodeId, f64)], n: NodeId) -> f64 {
    ens.iter().find(|(m, _)| *m == n).map_or(0.0, |(_, w)| *w)
}

/// Poids (f32) d'un membre, lu DANS LE SUBSTRAT via le couple lié (V4).
fn poids_substrat(writer: &SubstrateWriter, id: EngramId, n: NodeId) -> f32 {
    let (members, weights) = writer.read_engram_state(id.0 as u16).unwrap().unwrap();
    let ws = weights.expect("poids liés présents (format V4)");
    members
        .iter()
        .zip(&ws)
        .find(|(m, _)| **m == n.0 as u32)
        .map(|(_, w)| *w)
        .unwrap()
}

/// Manager + writer substrate + tempdir (SyncMode::Never : pas de crash ici).
fn manager_avec_substrate() -> (EngramManager, Arc<SubstrateWriter>, tempfile::TempDir) {
    let td = tempfile::tempdir().unwrap();
    let sub = SubstrateFile::create(td.path().join("kb")).unwrap();
    let writer = Arc::new(SubstrateWriter::new(sub, SyncMode::Never).unwrap());
    let mgr = EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    );
    mgr.attach_substrate(writer.clone());
    (mgr, writer, td)
}

// ===========================================================================
// 1. FORMATION D'UN ENSEMBLE NON UNIFORME → POIDS PERSISTÉS EXACTS
// ===========================================================================

/// Formation épisodique {a,b} (poids unitaires) puis révision additive pe=1.0 :
/// a,b décroissent à 0.95, c entre à 0.5 → ensemble NON uniforme. Le couple
/// (membres, poids) persiste dans le substrat avec les poids EXACTS (liés).
#[test]
fn formation_non_uniforme_persiste_des_poids_exacts() {
    let (mgr, writer, _td) = manager_avec_substrate();
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));

    let id = mgr.form_observation(&[a, b]);
    let rev = mgr.revise(id, &[c], 1.0).expect("révision additive");

    // Cache : ensemble non uniforme exact (règle de reconsolidation additive).
    assert_eq!(rev.members.len(), 3);
    assert!((weight_of(&rev.members, a) - 0.95).abs() < 1e-6, "A=0.95");
    assert!((weight_of(&rev.members, b) - 0.95).abs() < 1e-6, "B=0.95");
    assert!((weight_of(&rev.members, c) - 0.5).abs() < 1e-6, "C=0.5");

    // Substrat : couple LIÉ, ordre membres==poids, valeurs exactes.
    let (members, weights) = writer.read_engram_state(id.0 as u16).unwrap().unwrap();
    let ws = weights.expect("poids liés");
    assert_eq!(members.len(), 3, "3 membres persistés");
    assert_eq!(ws.len(), 3, "3 poids persistés");
    assert!(
        (poids_substrat(writer.as_ref(), id, a) - 0.95).abs() < 1e-5,
        "substrat A=0.95"
    );
    assert!(
        (poids_substrat(writer.as_ref(), id, b) - 0.95).abs() < 1e-5,
        "substrat B=0.95"
    );
    assert!(
        (poids_substrat(writer.as_ref(), id, c) - 0.5).abs() < 1e-5,
        "substrat C=0.5"
    );
}

// ===========================================================================
// 2. RÉVISION D'UN POIDS → PAS DE DEMI-ÉTAT
// ===========================================================================

/// Une révision adaptative complète change les poids (renforce l'observé,
/// décroît les absents). Après la mutation, le substrat doit porter le couple
/// COMPLET lié (cardinalité cohérente, empreinte valide, valeurs exactes) :
/// jamais « membres neufs + poids anciens ».
#[test]
fn revision_d_un_poids_ne_laisse_aucun_demi_etat() {
    let (mgr, writer, _td) = manager_avec_substrate();
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));

    let id = mgr.form_observation(&[a, b]);
    mgr.revise(id, &[c], 1.0).expect("révision initiale"); // {a:0.95,b:0.95,c:0.5}

    // lr = 0.3 + 0.5*0.5 = 0.55 ; a renforcé, b,c décroissent (absence = preuve).
    let rev = mgr
        .revise_adaptive(id, &[a], 0.5, ObservationScope::DeclaredComplete)
        .expect("révision adaptative");

    assert!(
        (weight_of(&rev.members, a) - 0.9775).abs() < 1e-6,
        "A=0.9775"
    );
    assert!(
        (weight_of(&rev.members, b) - 0.4275).abs() < 1e-6,
        "B=0.4275"
    );
    assert!((weight_of(&rev.members, c) - 0.225).abs() < 1e-6, "C=0.225");

    // Substrat : lecture RÉUSSIT (empreinte liée valide) et le couple est
    // COMPLET — un demi-état serait un refus EngramIncoherent ou une
    // cardinalité incohérente.
    let (members, weights) = writer.read_engram_state(id.0 as u16).unwrap().unwrap();
    let ws = weights.expect("poids liés (pas de demi-état)");
    assert_eq!(members.len(), 3);
    assert_eq!(ws.len(), members.len(), "cardinalité membres == poids");
    assert!(
        (poids_substrat(writer.as_ref(), id, a) - 0.9775).abs() < 1e-5,
        "substrat A=0.9775"
    );
    assert!(
        (poids_substrat(writer.as_ref(), id, b) - 0.4275).abs() < 1e-5,
        "substrat B=0.4275"
    );
    assert!(
        (poids_substrat(writer.as_ref(), id, c) - 0.225).abs() < 1e-5,
        "substrat C=0.225"
    );
}

// ===========================================================================
// 3. RECONSOLIDATION ADAPTATIVE → PROJECTION SPECTRALE RECONSTRUITE
// ===========================================================================

/// Après une reconsolidation adaptative, la signature spectrale publiée EST la
/// projection de l'ensemble pondéré FINAL (poids compris) : aucun vecteur
/// périmé n'est conservé.
#[test]
fn reconsolidation_adaptative_reconstruit_la_projection_spectrale() {
    let (mgr, _writer, _td) = manager_avec_substrate();
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));

    let id = mgr.form_observation(&[a, b]);
    mgr.revise(id, &[c], 1.0).expect("révision initiale");

    let rev = mgr
        .revise_adaptive(id, &[a, c], 0.5, ObservationScope::DeclaredComplete)
        .expect("révision adaptative");

    // La signature rendue par la révision est exactement la projection de
    // l'ensemble final (même encodeur déterministe que `encode_weighted`).
    assert!(!rev.signature.is_empty(), "signature non vide");
    assert_eq!(
        rev.signature,
        mgr.encode_weighted(&rev.members),
        "signature = projection spectrale de l'ensemble pondéré final"
    );

    // L'engramme publié porte la même signature (aucun vecteur périmé).
    let e = mgr.store().get(id).unwrap();
    assert_eq!(
        e.spectral_signature,
        mgr.encode_weighted(&e.ensemble),
        "signature de l'engramme reconstruite (poids compris)"
    );
}

// ===========================================================================
// 4. REHYDRATE_SIGNATURE SANS RÉVISER (ET SANS WRITE-THROUGH SUBSTRATE)
// ===========================================================================

/// `rehydrate_signature` reconstruit la projection DÉRIVÉE sans muter l'état
/// canonique : ni l'ensemble, ni la révision de l'engramme, ni le substrat.
/// Seul le cache Hopfield est invalidé (révision du store +1).
#[test]
fn rehydrate_signature_reconstruit_sans_reviser_ni_ecrire() {
    let (mgr, writer, _td) = manager_avec_substrate();
    let (a, b) = (NodeId(1), NodeId(2));

    let id = mgr.form_observation(&[a, b]);
    let before = mgr.store().get(id).unwrap().clone();
    let rev_engram = before.revision;
    let rev_store = mgr.store().revision();
    let sub_before = writer.read_engram_state(id.0 as u16).unwrap();

    let sig = vec![0.1, 0.2, 0.3];
    mgr.store().rehydrate_signature(id, sig.clone());

    let after = mgr.store().get(id).unwrap();
    assert_eq!(after.spectral_signature, sig, "projection dérivée injectée");
    assert_eq!(after.ensemble, before.ensemble, "ensemble non révisé");
    assert_eq!(after.revision, rev_engram, "révision d'engramme inchangée");
    assert_eq!(
        mgr.store().revision(),
        rev_store + 1,
        "seul le cache Hopfield est invalidé"
    );

    // AUCUN write-through durable : le substrat est strictement inchangé.
    assert_eq!(
        writer.read_engram_state(id.0 as u16).unwrap(),
        sub_before,
        "aucune réécriture substrate"
    );
}

// ===========================================================================
// 5. RAPPEL : ASSOCIATIONS MEMBRES/POIDS EXACTES PRÉSERVÉES AU RECHARGEMENT
// ===========================================================================

/// Après fermeture/réouverture, le rappel retrouve l'engramme non uniforme
/// avec les MÊMES associations membre→poids exactes.
#[test]
fn rappel_preserve_les_associations_membres_poids_apres_rechargement() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));

    let id;
    {
        let sub = SubstrateFile::create(&path).unwrap();
        let writer = Arc::new(SubstrateWriter::new(sub, SyncMode::Never).unwrap());
        let mgr = EngramManager::new(
            EngramManagerConfig::default(),
            Arc::new(InMemoryVectorIndex::new()),
        );
        mgr.attach_substrate(writer);
        id = mgr.form_observation(&[a, b]);
        mgr.revise(id, &[c], 1.0).expect("révision"); // {a:0.95,b:0.95,c:0.5}

        let hit = mgr
            .recall(&[c], 5)
            .into_iter()
            .find(|r| r.engram_id == id)
            .expect("rappel avant réouverture");
        assert_eq!(hit.engram.ensemble.len(), 3);
    } // drop → flush

    // Réouverture.
    let sub = SubstrateFile::open(&path).unwrap();
    let writer = Arc::new(SubstrateWriter::new(sub, SyncMode::Never).unwrap());
    let mgr = EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    );
    mgr.attach_substrate(writer);

    let hit = mgr
        .recall(&[c], 5)
        .into_iter()
        .find(|r| r.engram_id == id)
        .expect("rappel après réouverture");
    assert_eq!(hit.engram.ensemble.len(), 3, "3 membres");
    assert!(
        (weight_of(&hit.engram.ensemble, a) - 0.95).abs() < 1e-6,
        "A=0.95 préservé"
    );
    assert!(
        (weight_of(&hit.engram.ensemble, b) - 0.95).abs() < 1e-6,
        "B=0.95 préservé"
    );
    assert!(
        (weight_of(&hit.engram.ensemble, c) - 0.5).abs() < 1e-6,
        "C=0.5 préservé"
    );
}

// ===========================================================================
// 6. FERMETURE/RÉOUVERTURE → POIDS INTACTS
// ===========================================================================

/// Poids NON uniformes (révision adaptative) intacts après fermeture/réouverture,
/// lus à la fois dans le substrat (couple lié) et dans le cache rechargé.
#[test]
fn persistance_et_rechargement_preservent_les_poids_intacts() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));

    let id;
    {
        let sub = SubstrateFile::create(&path).unwrap();
        let writer = Arc::new(SubstrateWriter::new(sub, SyncMode::Never).unwrap());
        let mgr = EngramManager::new(
            EngramManagerConfig::default(),
            Arc::new(InMemoryVectorIndex::new()),
        );
        mgr.attach_substrate(writer);
        id = mgr.form_observation(&[a, b]);
        mgr.revise(id, &[c], 1.0).expect("révision initiale");
        mgr.revise_adaptive(id, &[a], 0.5, ObservationScope::DeclaredComplete)
            .expect("révision adaptative"); // {a:0.9775,b:0.4275,c:0.225}
    } // drop → flush

    // Réouverture : le couple lié est intact dans le substrat.
    let sub = SubstrateFile::open(&path).unwrap();
    let writer = Arc::new(SubstrateWriter::new(sub, SyncMode::Never).unwrap());
    let (members, weights) = writer.read_engram_state(id.0 as u16).unwrap().unwrap();
    let ws = weights.expect("poids liés");
    assert_eq!(members.len(), 3);
    assert_eq!(ws.len(), 3);
    assert!(
        (poids_substrat(writer.as_ref(), id, a) - 0.9775).abs() < 1e-5,
        "substrat A"
    );
    assert!(
        (poids_substrat(writer.as_ref(), id, b) - 0.4275).abs() < 1e-5,
        "substrat B"
    );
    assert!(
        (poids_substrat(writer.as_ref(), id, c) - 0.225).abs() < 1e-5,
        "substrat C"
    );

    // Et le cache rechargé depuis le substrat porte les mêmes poids.
    let mgr = EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    );
    mgr.attach_substrate(writer);
    let e = mgr.store().get(id).expect("engramme rechargé");
    assert!((weight_of(&e.ensemble, a) - 0.9775).abs() < 1e-6, "cache A");
    assert!((weight_of(&e.ensemble, b) - 0.4275).abs() < 1e-6, "cache B");
    assert!((weight_of(&e.ensemble, c) - 0.225).abs() < 1e-6, "cache C");
}

// ===========================================================================
// 7. REFUS SANS EFFET PARTIEL (poids non représentable, NodeId non-u32)
// ===========================================================================

/// Une mutation dont un poids est non-fini (f64), non-représentable en f32
/// (1e300 → inf) ou dont un NodeId déborde l'adressage u32 du substrat est
/// REFUSÉE. L'erreur remonte de `mirror_substrate_insert` (`mutate`) et ne
/// laisse AUCUN effet : ni cache, ni membres, ni bitset.
#[test]
fn refus_sans_effet_partiel_ni_membres_ni_bitset() {
    let (mgr, writer, _td) = manager_avec_substrate();
    let (a, b) = (NodeId(1), NodeId(2));

    let id = mgr.form_observation(&[a, b]);
    let before = mgr.store().get(id).unwrap().clone();
    let sub_before = writer.read_engram_state(id.0 as u16).unwrap();
    let bitset_a = writer.engram_bitset(a.0 as u32).unwrap();

    // (a) Poids non-fini (NaN) : refusé par la validation canonique.
    let r = mgr.store().mutate(id, |e| {
        e.ensemble[0].1 = f64::NAN;
    });
    assert!(
        matches!(r, Err(StoreError::ValidationFailed(_))),
        "NaN refusé (ValidationFailed attendu)"
    );
    assert_eq!(mgr.store().get(id).unwrap().ensemble, before.ensemble);
    assert_eq!(writer.read_engram_state(id.0 as u16).unwrap(), sub_before);
    assert_eq!(writer.engram_bitset(a.0 as u32).unwrap(), bitset_a);

    // (b) Poids non-représentable en f32 (fini f64, inf f32) : refusé PAR
    //     `mirror_substrate_insert` (propagation).
    let r = mgr.store().mutate(id, |e| {
        e.ensemble[0].1 = 1e300;
    });
    assert!(
        matches!(r, Err(StoreError::ValidationFailed(ref m)) if m.contains("f32")),
        "dépassement f32 refusé (ValidationFailed f32 attendu)"
    );
    assert_eq!(mgr.store().get(id).unwrap().ensemble, before.ensemble);
    assert_eq!(writer.read_engram_state(id.0 as u16).unwrap(), sub_before);
    assert_eq!(writer.engram_bitset(a.0 as u32).unwrap(), bitset_a);

    // (c) NodeId non-u32 : refusé PAR `mirror_substrate_insert` AVANT toute
    //     écriture (ni membres ni bitset).
    let r = mgr.store().mutate(id, |e| {
        e.ensemble.push((NodeId(u64::MAX), 0.5));
    });
    assert!(
        matches!(r, Err(StoreError::ValidationFailed(ref m)) if m.contains("non-u32")),
        "NodeId non-u32 refusé (ValidationFailed non-u32 attendu)"
    );
    assert_eq!(mgr.store().get(id).unwrap().ensemble, before.ensemble);
    assert_eq!(writer.read_engram_state(id.0 as u16).unwrap(), sub_before);
    assert_eq!(writer.engram_bitset(a.0 as u32).unwrap(), bitset_a);
}

// ===========================================================================
// 8a. EFFACEMENT + REJEU → LE COUPLE LIÉ EST INTACT (coordination T6)
// ===========================================================================

/// Écrit depuis la couche cognitive, le couple (membres, poids liés) survit à
/// l'effacement des zones dérivées + rejeu depuis le WAL seul. La relecture
/// cognitive rend les poids EXACTS (aucune réassociation positionnelle).
#[test]
fn effacement_et_rejeu_preservent_le_couple_lie() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));

    let id;
    {
        let sub = SubstrateFile::create(&path).unwrap();
        let writer = Arc::new(SubstrateWriter::new(sub, SyncMode::EveryCommit).unwrap());
        let mgr = EngramManager::new(
            EngramManagerConfig::default(),
            Arc::new(InMemoryVectorIndex::new()),
        );
        mgr.attach_substrate(writer.clone());
        id = mgr.form_observation(&[a, b]);
        mgr.revise(id, &[c], 1.0).expect("révision"); // {a:0.95,b:0.95,c:0.5}
        writer.commit().unwrap();
        writer.msync_zones().unwrap();
        writer.wal().fsync().unwrap();
    } // drop

    // Efface le DÉRIVÉ (zones membres+poids) ; le WAL (autorité) reste.
    std::fs::write(
        path.join(obrain_substrate::zone::ENGRAM_MEMBERS),
        Vec::<u8>::new(),
    )
    .unwrap();
    std::fs::write(
        path.join(obrain_substrate::zone::ENGRAM_WEIGHTS),
        Vec::<u8>::new(),
    )
    .unwrap();

    // Rejeu depuis le WAL seul.
    let sub2 = SubstrateFile::open(&path).unwrap();
    let stats = replay_from(&sub2, 0).expect("rejeu");
    assert_eq!(stats.decode_errors, 0, "aucune frame indécodable");

    // Relecture COGNITIVE : le couple lié exact est restauré.
    let writer2 = Arc::new(SubstrateWriter::new(sub2, SyncMode::Never).unwrap());
    let (members, weights) = writer2.read_engram_state(id.0 as u16).unwrap().unwrap();
    let ws = weights.expect("poids liés restaurés");
    assert_eq!(members.len(), 3);
    assert_eq!(ws.len(), 3);
    assert!(
        (poids_substrat(writer2.as_ref(), id, a) - 0.95).abs() < 1e-5,
        "A=0.95 restauré"
    );
    assert!(
        (poids_substrat(writer2.as_ref(), id, b) - 0.95).abs() < 1e-5,
        "B=0.95 restauré"
    );
    assert!(
        (poids_substrat(writer2.as_ref(), id, c) - 0.5).abs() < 1e-5,
        "C=0.5 restauré"
    );

    // Et le chargement cognitif complet (`load_from_substrate`) le réhydrate.
    let store2 = EngramStore::new(None);
    store2.attach_substrate(writer2);
    let report = store2.load_from_substrate();
    assert_eq!(report.coherent, 1, "engramme rechargé cohérent");
    let e = store2.get(id).expect("engramme réhydraté");
    assert!((weight_of(&e.ensemble, a) - 0.95).abs() < 1e-6);
    assert!((weight_of(&e.ensemble, b) - 0.95).abs() < 1e-6);
    assert!((weight_of(&e.ensemble, c) - 0.5).abs() < 1e-6);
}

// ===========================================================================
// 8b. MUTATION « MEMBRES SEULS » (REMAPPAGE) → QUARANTAINE, PAS DE RÉASSOCIATION
// ===========================================================================

/// Un remappage de slots réécrit les membres SANS réécrire les poids (chemin
/// `set_members_raw`). La protection V4 TIENT : la relecture cognitive met
/// l'engramme en QUARANTAINE (rejet nommé) au lieu de réassocier les poids
/// par position à des membres nouveaux.
#[test]
fn mutation_des_membres_seule_est_mise_en_quarantaine_par_le_cognitif() {
    let (mgr, writer, _td) = manager_avec_substrate();
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));

    let id = mgr.form_observation(&[a, b]);
    mgr.revise(id, &[c], 1.0).expect("révision"); // {a:0.95,b:0.95,c:0.5}

    // Mutation « membres seuls » (équivalent du remappage) : un membre change,
    // les poids restent liés à l'ANCIENNE empreinte.
    writer
        .set_engram_members(id.0 as u16, vec![a.0 as u32, b.0 as u32, 999])
        .expect("mutation membres seuls");

    // Le chargement COGNITIF refuse de réassocier : rejet, jamais chargé avec
    // des poids réassociés par position.
    let store2 = EngramStore::new(None);
    store2.attach_substrate(writer);
    let report = store2.load_from_substrate();

    assert_eq!(report.coherent, 0, "aucun engramme chargé cohérent");
    assert_eq!(report.rejected.len(), 1, "un rejet nommé");
    assert!(
        store2.get(id).is_none(),
        "aucun poids réassocié : engramme non chargé"
    );
    assert!(
        store2.next_id().0 > id.0,
        "identifiant réservé après corruption"
    );
}
