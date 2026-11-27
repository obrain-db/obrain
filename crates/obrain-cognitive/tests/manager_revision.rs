//! Révision cohérente au niveau d'EngramManager, avec contre-épreuves :
//! poids exacts (règle de reconsolidation), déduplication [C,C], distracteur,
//! indépendance de la signature, réouverture avec tolérances f32.

#![cfg(all(feature = "engram", feature = "substrate"))]
#![allow(clippy::many_single_char_names)]

use std::sync::{Arc, Mutex};

use obrain_cognitive::engram::traits::{InMemoryVectorIndex, VectorIndex};
use obrain_cognitive::engram::{
    EngramId, EngramManager, EngramManagerConfig, RejectCause, RevisionError, StoreError,
};
use obrain_common::types::NodeId;
use obrain_substrate::{SubstrateFile, SyncMode, Writer as SubstrateWriter};

fn weight_of(ens: &[(NodeId, f64)], n: NodeId) -> f64 {
    ens.iter().find(|(m, _)| *m == n).map_or(0.0, |(_, w)| *w)
}

fn members_of(ens: &[(NodeId, f64)]) -> Vec<NodeId> {
    ens.iter().map(|(n, _)| *n).collect()
}

#[test]
fn revision_coherente_survit_a_la_reouverture() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));
    let (d, e) = (NodeId(4), NodeId(5));

    let formed_id;
    let (before_sig, after_sig);
    {
        let manager = EngramManager::new(
            EngramManagerConfig::default(),
            Arc::new(InMemoryVectorIndex::new()),
        );
        let sub = SubstrateFile::create(&path).unwrap();
        let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
        manager.attach_substrate(Arc::new(writer));

        formed_id = manager.store().form(&[a, b]).expect("formation engramme");
        // Distracteur : un second engramme {D,E}, pour que le rappel ne soit
        // pas trivial (un seul engramme existerait sinon).
        let distractor = manager.store().form(&[d, e]).expect("distracteur");
        assert_ne!(formed_id, distractor);

        // Rappel AVANT révision + capture de la signature initiale.
        let before = manager.recall(&[a], 5);
        assert!(!before.is_empty(), "rappel avant révision");
        before_sig = before[0].engram.spectral_signature.clone();

        // Observation indépendante (C co-activé) → révision effective.
        let rev = manager
            .revise(formed_id, &[c], 0.5)
            .expect("révision effectuée");
        after_sig = rev.signature.clone();

        // Règle de reconsolidation : poids unitaires + erreur 0.5 →
        // A=0.95, B=0.95, C=0.25.
        assert!(
            (weight_of(&rev.members, a) - 0.95).abs() < 1e-6,
            "A décroît à 0.95"
        );
        assert!(
            (weight_of(&rev.members, b) - 0.95).abs() < 1e-6,
            "B décroît à 0.95"
        );
        assert!(
            (weight_of(&rev.members, c) - 0.25).abs() < 1e-6,
            "C ajouté à 0.25"
        );
        assert_eq!(rev.members.len(), 3, "pas de doublon : 3 membres");

        // Signature recalculée : non vide ET différente de l'état initial.
        assert!(!after_sig.is_empty(), "signature non vide");
        assert_ne!(before_sig, after_sig, "signature recalculée indépendamment");

        // Rappel par C : doit retrouver l'engramme révisé (pas le distracteur).
        let after = manager.recall(&[c], 5);
        assert!(
            after.iter().any(|r| r.engram_id == formed_id),
            "rappel par C retrouve l'engramme révisé (avec distracteur présent)"
        );
    } // drop → flush

    // Réouverture : état exactement conservé (tolérance f32).
    {
        let manager = EngramManager::new(
            EngramManagerConfig::default(),
            Arc::new(InMemoryVectorIndex::new()),
        );
        let sub = SubstrateFile::open(&path).unwrap();
        let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
        manager.attach_substrate(Arc::new(writer));

        let engram = manager.store().get(formed_id).expect("engramme rechargé");
        let members = members_of(&engram.ensemble);
        assert!(
            members.contains(&a) && members.contains(&b) && members.contains(&c),
            "membres {{A,B,C}} conservés : {members:?}"
        );
        assert!(
            (weight_of(&engram.ensemble, a) - 0.95).abs() < 1e-6,
            "poids A conservé (f32)"
        );
        assert!(
            (weight_of(&engram.ensemble, b) - 0.95).abs() < 1e-6,
            "poids B conservé (f32)"
        );
        assert!(
            (weight_of(&engram.ensemble, c) - 0.25).abs() < 1e-6,
            "poids C conservé (f32)"
        );
    }
}

/// Contre-épreuve : une observation [C, C] ne doit pas ajouter C deux fois.
#[test]
fn reconsolidation_deduplique_les_doublons() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));

    let manager = EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    );
    let sub = SubstrateFile::create(&path).unwrap();
    let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
    manager.attach_substrate(Arc::new(writer));

    let id = manager.store().form(&[a, b]).unwrap();
    let rev = manager.revise(id, &[c, c], 0.5).expect("révision");
    let cs: Vec<NodeId> = rev
        .members
        .iter()
        .filter(|(n, _)| *n == c)
        .map(|(n, _)| *n)
        .collect();
    assert_eq!(cs.len(), 1, "C ajouté une seule fois malgré [C, C]");
    assert_eq!(rev.members.len(), 3, "3 membres, pas de doublon");
}

/// Observation invalide : erreur non finie → refusée (pas d'état persistant).
#[test]
fn observation_invalide_refusee() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));

    let manager = EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    );
    let sub = SubstrateFile::create(&path).unwrap();
    let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
    manager.attach_substrate(Arc::new(writer));

    let id = manager.store().form(&[a, b]).unwrap();
    assert!(matches!(
        manager.revise(id, &[c], f64::NAN),
        Err(RevisionError::InvalidObservation(_))
    ));
    // L'engramme reste {A,B} (pas de C ajouté silencieusement).
    let e = manager.store().get(id).unwrap();
    assert_eq!(
        e.ensemble.len(),
        2,
        "état inchangé après observation invalide"
    );
}

/// Contre-épreuve CONCURRENTE : deux révisions simultanées du même engramme.
/// Sans le verrou par engramme, un read-modify-write entrelacé perdrait l'une
/// des deux additions. La barrière force un départ simultané.
#[test]
fn deux_revisions_concurrentes_coherentes() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b, c, d) = (NodeId(1), NodeId(2), NodeId(3), NodeId(4));

    let manager = Arc::new(EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    ));
    let sub = SubstrateFile::create(&path).unwrap();
    let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
    manager.attach_substrate(Arc::new(writer));

    let id = manager.store().form(&[a, b]).unwrap();

    let barrier = Arc::new(std::sync::Barrier::new(2));
    let (m1, m2) = (manager.clone(), manager.clone());
    let (b1, b2) = (barrier.clone(), barrier.clone());
    let t1 = std::thread::spawn(move || {
        b1.wait();
        m1.revise(id, &[c], 0.5).expect("révision C");
    });
    let t2 = std::thread::spawn(move || {
        b2.wait();
        m2.revise(id, &[d], 0.5).expect("révision D");
    });
    t1.join().unwrap();
    t2.join().unwrap();

    let e = manager.store().get(id).expect("engramme présent");
    let members = members_of(&e.ensemble);
    assert!(
        members.contains(&c) && members.contains(&d),
        "les deux révisions sont appliquées (pas de mise à jour perdue) : {members:?}"
    );
    assert_eq!(
        members.len(),
        4,
        "aucun doublon, aucune perte : {members:?}"
    );
    // Les poids sont décroissants mais cohérents (deux décroissances successives).
    assert!(
        (weight_of(&e.ensemble, a) - 0.9025).abs() < 1e-6,
        "poids A = 0.9025 : deux décroissances successives (sérialisé)"
    );
}

/// Index vectoriel INSTRUMENTÉ : enregistre l'ordre des publications (upsert)
/// de signatures. Sert de contre-épreuve d'index, pas seulement des membres.
struct RecordingIndex {
    inner: InMemoryVectorIndex,
    history: Arc<Mutex<Vec<(String, Vec<f64>)>>>,
}

impl RecordingIndex {
    fn new() -> (Arc<Self>, Arc<Mutex<Vec<(String, Vec<f64>)>>>) {
        let history = Arc::new(Mutex::new(Vec::new()));
        (
            Arc::new(Self {
                inner: InMemoryVectorIndex::new(),
                history: history.clone(),
            }),
            history,
        )
    }
}

impl VectorIndex for RecordingIndex {
    fn upsert(&self, id: &str, v: &[f64]) {
        self.inner.upsert(id, v);
        self.history
            .lock()
            .unwrap()
            .push((id.to_string(), v.to_vec()));
    }
    fn nearest(&self, q: &[f64], k: usize) -> Vec<(String, f64)> {
        self.inner.nearest(q, k)
    }
    fn remove(&self, id: &str) {
        self.inner.remove(id);
    }
    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }
}

/// Contre-épreuve D'INDEX : après deux révisions concurrentes du même engramme,
/// la DERNIÈRE signature publiée dans l'index doit correspondre à l'état final
/// (4 membres), jamais à une projection obsolète (3 membres).
#[test]
fn publication_des_projections_ordonnee() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b, c, d) = (NodeId(1), NodeId(2), NodeId(3), NodeId(4));

    let (index, history) = RecordingIndex::new();
    let manager = Arc::new(EngramManager::new(
        EngramManagerConfig::default(),
        index.clone(),
    ));
    let sub = SubstrateFile::create(&path).unwrap();
    let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
    manager.attach_substrate(Arc::new(writer));

    let id = manager.store().form(&[a, b]).unwrap();

    // Deux révisions concurrentes ([C] et [D]), barrière de départ simultané.
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let (m1, m2) = (manager.clone(), manager.clone());
    let (b1, b2) = (barrier.clone(), barrier.clone());
    let t1 = std::thread::spawn(move || {
        b1.wait();
        m1.revise(id, &[c], 0.5).unwrap();
    });
    let t2 = std::thread::spawn(move || {
        b2.wait();
        m2.revise(id, &[d], 0.5).unwrap();
    });
    t1.join().unwrap();
    t2.join().unwrap();

    // État canonique final (4 membres) et sa signature.
    let final_engram = manager.store().get(id).expect("engramme présent");
    let final_sig = final_engram.spectral_signature.clone();
    assert_eq!(
        members_of(&final_engram.ensemble).len(),
        4,
        "état final : 4 membres"
    );

    // La DERNIÈRE publication d'index pour cet engramme DOIT être l'état final.
    let hist = history.lock().unwrap().clone();
    let last = hist
        .iter()
        .rev()
        .find(|(i, _)| i == &id.to_string())
        .expect("au moins un upsert enregistré");
    assert_eq!(
        &last.1, &final_sig,
        "dernière signature publiée = état final (aucune projection obsolète)"
    );
}

/// Tests DIRECTS de `mutate` (pas via revise) : les invariants de validation
/// sont propres au store, pas au manager.
#[test]
fn mutate_refuse_invariants_invalides() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b) = (NodeId(1), NodeId(2));

    let manager = EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    );
    let sub = SubstrateFile::create(&path).unwrap();
    let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
    manager.attach_substrate(Arc::new(writer));

    let id = manager.store().form(&[a, b]).unwrap();
    let before = manager.store().get(id).unwrap().clone();

    // Changement d'identité logique refusé.
    let r = manager.store().mutate(id, |e| {
        e.id = obrain_cognitive::engram::EngramId(999);
    });
    assert!(
        matches!(r, Err(StoreError::ValidationFailed(_))),
        "identité refusée"
    );
    assert_eq!(
        manager.store().get(id).unwrap().ensemble,
        before.ensemble,
        "état intact (identité)"
    );

    // Membre dupliqué refusé.
    let r = manager.store().mutate(id, |e| {
        e.ensemble.push((a, 1.0));
    });
    assert!(
        matches!(r, Err(StoreError::ValidationFailed(_))),
        "doublon refusé"
    );
    assert_eq!(
        manager.store().get(id).unwrap().ensemble,
        before.ensemble,
        "état intact (doublon)"
    );

    // Poids non fini (f64) refusé.
    let r = manager.store().mutate(id, |e| {
        e.ensemble[0].1 = f64::NAN;
    });
    assert!(
        matches!(r, Err(StoreError::ValidationFailed(_))),
        "NaN refusé"
    );
    assert_eq!(
        manager.store().get(id).unwrap().ensemble,
        before.ensemble,
        "état intact (NaN)"
    );

    // Dépassement f32 refusé (substrate) : 1e300 est fini en f64 mais inf en f32.
    let r = manager.store().mutate(id, |e| {
        e.ensemble[0].1 = 1e300;
    });
    assert!(
        matches!(r, Err(StoreError::ValidationFailed(_))),
        "dépassement f32 refusé"
    );

    // État COMPLET inchangé après chaque refus (pas seulement len()).
    let e = manager.store().get(id).unwrap();
    assert_eq!(
        e.ensemble, before.ensemble,
        "état canonique complet inchangé après refus"
    );
    assert_eq!(e.id, before.id, "identité inchangée");
}

/// Injection d'erreur contrôlée : set_engram_state réussit PUIS add_engram_bit
/// échoue → écriture partielle (substrat r+1, cache r). Observe cache, substrat
/// et définit la source de vérité (le cache, non committé).
#[cfg(feature = "fault-injection")]
#[test]
fn injection_erreur_partielle_set_then_bit() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));

    let manager = EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    );
    let sub = SubstrateFile::create(&path).unwrap();
    let writer = Arc::new(SubstrateWriter::new(sub, SyncMode::Never).unwrap());
    manager.attach_substrate(writer.clone());

    let id = manager.store().form(&[a, b]).unwrap();
    let before = manager.store().get(id).unwrap().clone();

    // Injection : le prochain add_engram_bit échouera APRÈS set_engram_state.
    manager.store().set_fail_bit_after_state(true);
    let r = manager.revise(id, &[c], 0.5);
    assert!(
        matches!(r, Err(RevisionError::PersistenceFailed(_))),
        "échec de persistance propagé, pas un succès silencieux : {r:?}"
    );

    // Cache : candidat NON committé → état inchangé.
    let after = manager.store().get(id).unwrap();
    assert_eq!(
        after.ensemble, before.ensemble,
        "cache inchangé (candidat non committé)"
    );

    // Substrat : set_engram_state a écrit les membres AVANT l'échec du bitset.
    let (members, _weights) = writer.read_engram_state(id.0 as u16).unwrap().unwrap();
    assert!(
        members.contains(&(c.0 as u32)),
        "substrat contient le membre partiel (r+1) : {members:?}"
    );

    // Nouvelle tentative : le candidat échoué n'a jamais été committé, donc C
    // n'existe pas encore → ajouté une fois. Ne prouve PAS l'idempotence
    // d'observation (l'API n'a pas d'identité d'observation).
    manager.store().set_fail_bit_after_state(false);
    let retry = manager.revise(id, &[c], 0.5).expect("reprise réussie");
    let cs: Vec<NodeId> = retry
        .members
        .iter()
        .filter(|(n, _)| *n == c)
        .map(|(n, _)| *n)
        .collect();
    assert_eq!(cs.len(), 1, "C présent une seule fois");
}

/// Contre-épreuve ESSENTIELLE : fermeture immédiate après échec partiel, puis
/// réouverture. Observe l'état réellement restauré et la détection de l'état
/// partiel (membres vs bitset), sans retry manuel.
#[cfg(feature = "fault-injection")]
#[test]
fn reouverture_apres_ecriture_partielle() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));

    let id;
    {
        let manager = EngramManager::new(
            EngramManagerConfig::default(),
            Arc::new(InMemoryVectorIndex::new()),
        );
        let sub = SubstrateFile::create(&path).unwrap();
        let writer = Arc::new(SubstrateWriter::new(sub, SyncMode::Never).unwrap());
        manager.attach_substrate(writer.clone());
        id = manager.store().form(&[a, b]).unwrap();
        manager.store().set_fail_bit_after_state(true);
        let r = manager.revise(id, &[c], 0.5);
        assert!(matches!(r, Err(RevisionError::PersistenceFailed(_))));
        // FERMETURE immédiate, sans nouvelle tentative.
    }

    // Réouverture.
    let manager = EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    );
    let sub = SubstrateFile::open(&path).unwrap();
    let writer = Arc::new(SubstrateWriter::new(sub, SyncMode::Never).unwrap());
    manager.attach_substrate(writer.clone());

    // Couples EXACTS membre/poids dans le SUBSTRAT (zip membres↔poids).
    let (members, weights) = writer.read_engram_state(id.0 as u16).unwrap().unwrap();
    let ws = weights.expect("poids présents");
    assert_eq!(members.len(), 3, "3 membres restaurés");
    let sw = |n: u32| -> f32 {
        members
            .iter()
            .zip(&ws)
            .find(|(m, _)| **m == n)
            .map(|(_, w)| *w)
            .unwrap()
    };
    assert!((sw(a.0 as u32) - 0.95).abs() < 1e-6, "substrat A=0.95");
    assert!((sw(b.0 as u32) - 0.95).abs() < 1e-6, "substrat B=0.95");
    assert!((sw(c.0 as u32) - 0.25).abs() < 1e-6, "substrat C=0.25");

    // CACHE restauré (manager.store().get) : mêmes couples exacts.
    let restored = manager.store().get(id).unwrap();
    assert!(
        (weight_of(&restored.ensemble, a) - 0.95).abs() < 1e-6,
        "cache A=0.95"
    );
    assert!(
        (weight_of(&restored.ensemble, b) - 0.95).abs() < 1e-6,
        "cache B=0.95"
    );
    assert!(
        (weight_of(&restored.ensemble, c) - 0.25).abs() < 1e-6,
        "cache C=0.25"
    );
    assert!(
        !restored.spectral_signature.is_empty(),
        "signature restaurée"
    );

    // Bitset RÉPARÉ au chargement : le bit de C est reconstruit depuis les
    // membres persistés. (Présence du bit = nécessaire, PAS suffisante pour la
    // complétude transactionnelle — c'est un filtre Bloom.)
    let bitset_c = writer.engram_bitset(c.0 as u32).unwrap();
    let expected_mask = 1u64 << ((id.0 as u16) & 0x3F);
    assert_ne!(bitset_c & expected_mask, 0, "bitset de C réparé (bit posé)");

    // Cohérence des deux chemins de rappel RESTAURÉE (plus de divergence).
    let native = manager.store().recall(c).expect("rappel natif");
    assert!(
        native.contains(&id),
        "rappel natif trouve C (bitset réparé)"
    );
    let mgr = manager.recall(&[c], 5);
    assert!(
        mgr.iter().any(|r| r.engram_id == id),
        "rappel manager trouve C"
    );

    // Nouvelle tentative APRÈS réouverture : reconsolidation décroît TOUT.
    let rev = manager.revise(id, &[c], 0.5).expect("reprise réussie");
    assert!(
        (weight_of(&rev.members, a) - 0.9025).abs() < 1e-6,
        "A=0.9025 après retry"
    );
    assert!(
        (weight_of(&rev.members, b) - 0.9025).abs() < 1e-6,
        "B=0.9025 après retry"
    );
    assert!(
        (weight_of(&rev.members, c) - 0.2375).abs() < 1e-6,
        "C=0.2375 après retry"
    );
    let cs: Vec<NodeId> = rev
        .members
        .iter()
        .filter(|(n, _)| *n == c)
        .map(|(n, _)| *n)
        .collect();
    assert_eq!(cs.len(), 1, "C présent une seule fois");
}

/// Idempotence de la réparation : `ensure_engram_bit` ne produit AUCUNE écriture
/// quand le bit est déjà présent (retourne Ok(false)). Une réouverture saine ne
/// réécrit donc pas les bitsets.
#[test]
fn reouverture_saine_ne_reecrit_pas_les_bitsets() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b) = (NodeId(1), NodeId(2));

    let id;
    {
        let manager = EngramManager::new(
            EngramManagerConfig::default(),
            Arc::new(InMemoryVectorIndex::new()),
        );
        let sub = SubstrateFile::create(&path).unwrap();
        let writer = Arc::new(SubstrateWriter::new(sub, SyncMode::Never).unwrap());
        manager.attach_substrate(writer.clone());
        id = manager.store().form(&[a, b]).unwrap();
        // après form, les bits A/B sont posés.
    }
    // Réouverture saine : les bits sont déjà présents → ensure renvoie false.
    let sub = SubstrateFile::open(&path).unwrap();
    let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
    assert!(
        !writer.ensure_engram_bit(a.0 as u32, id.0 as u16).unwrap(),
        "bit A déjà présent : aucune écriture"
    );
    assert!(
        !writer.ensure_engram_bit(b.0 as u32, id.0 as u16).unwrap(),
        "bit B déjà présent : aucune écriture"
    );
}

/// Qualification WAL : une réouverture SAINE ne produit aucune écriture de
/// bitset (ensure_engram_bit no-op sur bits présents). Mesure offset, records
/// écrits et records en tampon autour du chargement réel (attach_substrate).
#[test]
fn chargement_sain_sans_amplification_wal() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b) = (NodeId(1), NodeId(2));

    {
        let manager = EngramManager::new(
            EngramManagerConfig::default(),
            Arc::new(InMemoryVectorIndex::new()),
        );
        let sub = SubstrateFile::create(&path).unwrap();
        let writer = Arc::new(SubstrateWriter::new(sub, SyncMode::Never).unwrap());
        manager.attach_substrate(writer.clone());
        manager.store().form(&[a, b]).unwrap();
        // fermeture propre
    }

    // Réouverture du writer, mesures AVANT le chargement cognitif.
    let sub = SubstrateFile::open(&path).unwrap();
    let writer = Arc::new(SubstrateWriter::new(sub, SyncMode::Never).unwrap());
    let offset_before = writer.wal().offset();
    let ecrits_before = writer.wal().records_ecrits();
    let tampon_before = writer.wal().records_en_tampon();

    // Chargement RÉEL : nouveau manager + attach_substrate.
    let manager = EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    );
    let report = manager.attach_substrate(writer.clone());

    let offset_after = writer.wal().offset();
    let ecrits_after = writer.wal().records_ecrits();
    let tampon_after = writer.wal().records_en_tampon();

    assert_eq!(report.coherent, 1, "un engramme cohérent chargé");
    assert!(report.degraded.is_empty(), "aucun dégradé");
    assert_eq!(report.rejected.len(), 0, "aucun rejet");
    assert_eq!(
        offset_before, offset_after,
        "offset WAL inchangé (aucune écriture)"
    );
    assert_eq!(ecrits_before, ecrits_after, "records_ecrits inchangé");
    assert_eq!(tampon_before, tampon_after, "records_en_tampon inchangé");
}

// ===================================================================
// LoadReport — causes, identifiants, défaillance globale
// ===================================================================

/// Base vide : aucun engramme, zone jamais initialisée (absente) → chargement
/// sain, aucun rejet, aucune défaillance globale.
#[test]
fn base_vide_charge_sainement() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    {
        let manager = EngramManager::new(
            EngramManagerConfig::default(),
            Arc::new(InMemoryVectorIndex::new()),
        );
        let sub = SubstrateFile::create(&path).unwrap();
        let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
        manager.attach_substrate(Arc::new(writer));
        // aucun engramme formé
    }
    let manager = EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    );
    let sub = SubstrateFile::open(&path).unwrap();
    let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
    let report = manager.attach_substrate(Arc::new(writer));
    assert_eq!(report.coherent, 0, "aucun engramme cohérent");
    assert!(report.degraded.is_empty(), "aucun dégradé");
    assert!(report.rejected.is_empty(), "aucun rejet");
    assert!(
        report.global_failure.is_none(),
        "pas de défaillance globale : zone absente = légitime"
    );
    assert_eq!(manager.store().count(), 0, "cache vide");
}

/// Zone structurellement invalide (magic corrompu) : défaillance GLOBALE, pas
/// un balayage de 65 535 rejets. Le chargement refuse SANS réserver aucun
/// identifiant, et n'est jamais présenté comme sain.
#[test]
fn zone_inaccessible_est_une_defaillance_globale() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b) = (NodeId(1), NodeId(2));
    {
        let manager = EngramManager::new(
            EngramManagerConfig::default(),
            Arc::new(InMemoryVectorIndex::new()),
        );
        let sub = SubstrateFile::create(&path).unwrap();
        let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
        manager.attach_substrate(Arc::new(writer));
        manager.store().form(&[a, b]).unwrap();
    }
    // Corrompt le magic de la zone members : la zone devient structurellement
    // invalide (défaillance globale, pas un engramme corrompu).
    let zone = path.join("substrate.engram_members");
    {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = std::fs::OpenOptions::new().write(true).open(&zone).unwrap();
        f.seek(SeekFrom::Start(0)).unwrap();
        f.write_all(&[0xFF, 0xFF, 0xFF, 0xFF]).unwrap();
        f.flush().unwrap();
    }
    let manager = EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    );
    let sub = SubstrateFile::open(&path).unwrap();
    let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
    let report = manager.attach_substrate(Arc::new(writer));

    assert!(
        report.global_failure.is_some(),
        "défaillance globale détectée"
    );
    assert!(
        report.rejected.is_empty(),
        "aucun rejet individuel (pas de balayage des 65 535 identifiants)"
    );
    assert_eq!(report.coherent, 0, "aucun chargé");
    assert!(report.degraded.is_empty(), "aucun dégradé");
    assert_eq!(
        manager.store().count(),
        0,
        "cache vide (jamais présenté sain)"
    );
    // L'espace d'identifiants n'a PAS été épuisé : aucun identifiant réservé.
    assert_eq!(manager.store().next_id().0, 1, "aucun identifiant réservé");
}

/// État canonique invalide (membre dupliqué) : rejeté AVEC l'identifiant et la
/// cause, identifiant réservé (jamais réalloué), aucune réparation ni
/// publication.
#[test]
fn engramme_invalide_rejete_avec_identifiant_et_cause() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    {
        let sub = SubstrateFile::create(&path).unwrap();
        let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
        // Membre DUPLIQUÉ écrit directement via le substrat (form refuserait).
        writer
            .set_engram_state(1, vec![7, 7], vec![1.0, 1.0])
            .unwrap();
    }
    let manager = EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    );
    let sub = SubstrateFile::open(&path).unwrap();
    let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
    let report = manager.attach_substrate(Arc::new(writer));

    assert!(report.global_failure.is_none(), "zone saine");
    assert_eq!(report.rejected.len(), 1, "un seul rejet");
    let (id, cause) = &report.rejected[0];
    assert_eq!(id.0, 1, "identifiant rejeté = 1");
    assert!(
        matches!(cause, RejectCause::Validation(_)),
        "cause = validation (doublon), reçu : {cause:?}"
    );
    assert_eq!(manager.store().count(), 0, "aucun engramme publié");
    // Identifiant réservé : next_id repart à 2 (le 1 n'est pas réalloué).
    assert_eq!(manager.store().next_id().0, 2, "identifiant 1 réservé");
}

/// Incohérence de longueur membres/poids : rejetée avec sa cause dédiée, sans
/// fabriquer de poids.
#[test]
fn incoherence_membres_poids_rejetee() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    {
        let sub = SubstrateFile::create(&path).unwrap();
        let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
        // 1 membre, 2 poids : écrit via les setters séparés (set_engram_state
        // refuse la longueur incohérente).
        writer.set_engram_members(1, vec![7]).unwrap();
        writer.set_engram_weights_legacy(1, vec![1.0, 2.0]).unwrap();
    }
    let manager = EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    );
    let sub = SubstrateFile::open(&path).unwrap();
    let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
    let report = manager.attach_substrate(Arc::new(writer));

    assert!(report.global_failure.is_none(), "zone saine");
    assert_eq!(report.rejected.len(), 1, "un rejet");
    let (id, cause) = &report.rejected[0];
    assert_eq!(id.0, 1, "identifiant rejeté = 1");
    assert!(
        matches!(
            cause,
            RejectCause::InconsistentWeights {
                members: 1,
                weights: 2
            }
        ),
        "cause = incohérence 1 membre / 2 poids, reçu : {cause:?}"
    );
    assert_eq!(manager.store().count(), 0, "aucun engramme publié");
}

// ===================================================================
// Rappel dégradé — repli sur l'état canonique (bitset incomplet)
// ===================================================================

/// Rappel dégradé : un engramme dont le bitset est INCOMPLET (bit manquant,
/// réparation échouée) reste rappelé par l'état canonique, même si le bitset
/// du nœud vaut 0 (retour anticipé du chemin rapide).
#[cfg(feature = "fault-injection")]
#[test]
fn rappel_degrade_replie_sur_etat_canonique_bitset_zero() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let a = NodeId(42);
    {
        let sub = SubstrateFile::create(&path).unwrap();
        let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
        // Membres persistés SANS poser le bit (set_engram_state ne touche pas
        // le bitset) : projection bitset incomplète.
        writer
            .set_engram_state(1, vec![a.0 as u32], vec![1.0])
            .unwrap();
    }
    // Réouverture : la réparation du bit ÉCHOUE (injection) → dégradé.
    let manager = EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    );
    manager.store().set_fail_repair_bit(true);
    let sub = SubstrateFile::open(&path).unwrap();
    let writer = Arc::new(SubstrateWriter::new(sub, SyncMode::Never).unwrap());
    let report = manager.attach_substrate(writer.clone());

    assert!(report.global_failure.is_none(), "zone saine");
    assert_eq!(report.coherent, 0, "aucun cohérent");
    assert_eq!(report.degraded.len(), 1, "un dégradé");
    assert_eq!(
        manager.store().degraded_engrams(),
        vec![EngramId(1)],
        "registre dégradé contient l'engramme"
    );

    // Le bit n'est PAS posé (réparation échouée) : bitset du nœud == 0.
    let bitset = writer.engram_bitset(a.0 as u32).unwrap();
    assert_eq!(bitset, 0, "bitset nul (bit jamais réparé)");

    // Le rappel NATIF doit quand même retrouver l'engramme (repli canonique),
    // y compris via le retour anticipé bitset==0.
    let native = manager.store().recall(a).unwrap();
    assert!(
        native.contains(&EngramId(1)),
        "repli dégradé retrouve l'engramme malgré bitset==0 : {native:?}"
    );
}

/// Cycle de vie du registre dégradé : une mutation réussie (qui ré-écrit le
/// bitset complet) efface la dégradation ; une suppression aussi. Pas de
/// registre périmé.
#[cfg(feature = "fault-injection")]
#[test]
fn cycle_de_vie_du_registre_degrade() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let a = NodeId(42);
    let b = NodeId(43);
    {
        let sub = SubstrateFile::create(&path).unwrap();
        let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
        writer
            .set_engram_state(1, vec![a.0 as u32], vec![1.0])
            .unwrap();
    }
    // Réouverture dégradée (réparation échouée).
    let manager = EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    );
    manager.store().set_fail_repair_bit(true);
    let sub = SubstrateFile::open(&path).unwrap();
    let writer = Arc::new(SubstrateWriter::new(sub, SyncMode::Never).unwrap());
    let report = manager.attach_substrate(writer.clone());
    assert_eq!(report.degraded.len(), 1, "dégradé au chargement");
    assert_eq!(manager.store().degraded_engrams(), vec![EngramId(1)]);

    // Désarme l'injection puis mute (ré-écrit membres + bitset) → dégradation
    // effacée.
    manager.store().set_fail_repair_bit(false);
    manager
        .store()
        .mutate(EngramId(1), |e| {
            e.ensemble.push((b, 0.5));
        })
        .expect("mutation réussie");
    assert!(
        manager.store().degraded_engrams().is_empty(),
        "mutation réussie efface la dégradation (bitset ré-écrit complet)"
    );

    // Re-marque dégradé puis supprime → effacé.
    manager.store().set_fail_repair_bit(true);
    let _ = manager.store().load_from_substrate();
    assert!(!manager.store().degraded_engrams().is_empty(), "re-dégradé");
    manager.store().remove(EngramId(1));
    assert!(
        manager.store().degraded_engrams().is_empty(),
        "suppression efface la dégradation"
    );
}

// ===================================================================
// Qualification WAL — réparation, réouverture suivante, échec contrôlé
// ===================================================================

/// Qualification WAL de la RÉPARATION d'un bit manquant : une réouverture qui
/// répare produit exactement les écritures de réparation (offset et records
/// augmentent), puis vérifie couples membres/poids, rappels et index.
#[test]
fn reparation_bit_manquant_mesuree_wal() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let a = NodeId(10);
    {
        let sub = SubstrateFile::create(&path).unwrap();
        let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
        // Membres persistés SANS le bit (projection incomplète).
        writer
            .set_engram_state(1, vec![a.0 as u32], vec![0.75])
            .unwrap();
    }

    let sub = SubstrateFile::open(&path).unwrap();
    let writer = Arc::new(SubstrateWriter::new(sub, SyncMode::Never).unwrap());
    let offset_before = writer.wal().offset();
    let ecrits_before = writer.wal().records_ecrits();

    let manager = EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    );
    let report = manager.attach_substrate(writer.clone());

    let offset_after = writer.wal().offset();
    let ecrits_after = writer.wal().records_ecrits();

    assert!(report.global_failure.is_none(), "zone saine");
    assert_eq!(report.coherent, 1, "réparé → cohérent");
    assert!(report.degraded.is_empty(), "aucun dégradé");
    assert!(
        offset_after > offset_before,
        "offset WAL a avancé (réparation)"
    );
    assert!(
        ecrits_after > ecrits_before,
        "records_ecrits a augmenté (réparation)"
    );

    // Couple membre/poids exact (f32 → f64).
    let e = manager.store().get(EngramId(1)).unwrap();
    assert_eq!(e.ensemble.len(), 1, "un membre");
    assert_eq!(e.ensemble[0].0, a, "membre exact");
    assert!((e.ensemble[0].1 - 0.75).abs() < 1e-6, "poids exact 0.75");

    // Signature re-dérivée + index (rappel spectral opérationnel).
    assert!(!e.spectral_signature.is_empty(), "signature re-dérivée");
    let mgr = manager.recall(&[a], 5);
    assert!(
        mgr.iter().any(|r| r.engram_id == EngramId(1)),
        "rappel manager/spectral retrouve l'engramme"
    );

    // Rappel natif (bitset réparé).
    let native = manager.store().recall(a).unwrap();
    assert!(
        native.contains(&EngramId(1)),
        "rappel natif retrouve l'engramme"
    );

    // Le bit est maintenant posé.
    let bitset = writer.engram_bitset(a.0 as u32).unwrap();
    assert_ne!(bitset, 0, "bit réparé posé");
}

/// Réouverture SUIVANTE (après réparation) : les bits sont présents, aucune
/// écriture supplémentaire (offset et records inchangés).
#[test]
fn reouverture_suivante_apres_reparation_saine() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let a = NodeId(10);
    {
        let sub = SubstrateFile::create(&path).unwrap();
        let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
        writer
            .set_engram_state(1, vec![a.0 as u32], vec![0.75])
            .unwrap();
    }
    // Première réouverture : réparation du bit manquant.
    {
        let manager = EngramManager::new(
            EngramManagerConfig::default(),
            Arc::new(InMemoryVectorIndex::new()),
        );
        let sub = SubstrateFile::open(&path).unwrap();
        let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
        let report = manager.attach_substrate(Arc::new(writer));
        assert_eq!(report.coherent, 1, "réparé à la première réouverture");
    }

    // Seconde réouverture : bit déjà présent → aucune écriture.
    let sub = SubstrateFile::open(&path).unwrap();
    let writer = Arc::new(SubstrateWriter::new(sub, SyncMode::Never).unwrap());
    let offset_before = writer.wal().offset();
    let ecrits_before = writer.wal().records_ecrits();
    let manager = EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    );
    let report = manager.attach_substrate(writer.clone());
    let offset_after = writer.wal().offset();
    let ecrits_after = writer.wal().records_ecrits();

    assert_eq!(report.coherent, 1, "chargé cohérent");
    assert!(report.degraded.is_empty(), "aucun dégradé");
    assert_eq!(
        offset_before, offset_after,
        "offset inchangé (aucune réécriture)"
    );
    assert_eq!(ecrits_before, ecrits_after, "records_ecrits inchangé");
}

/// Échec contrôlé PENDANT la réparation : l'engramme est dégradé (bit manquant),
/// l'état canonique est publié, AUCUNE écriture de réparation n'a eu lieu et
/// AUCUN apprentissage (poids inchangés, pas de reconsolidation).
#[cfg(feature = "fault-injection")]
#[test]
fn echec_pendant_reparation_ne_reecrit_pas_et_n_apprend_pas() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let a = NodeId(10);
    {
        let sub = SubstrateFile::create(&path).unwrap();
        let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
        writer
            .set_engram_state(1, vec![a.0 as u32], vec![0.75])
            .unwrap();
    }

    let sub = SubstrateFile::open(&path).unwrap();
    let writer = Arc::new(SubstrateWriter::new(sub, SyncMode::Never).unwrap());
    let offset_before = writer.wal().offset();
    let ecrits_before = writer.wal().records_ecrits();

    let manager = EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    );
    manager.store().set_fail_repair_bit(true);
    let report = manager.attach_substrate(writer.clone());

    let offset_after = writer.wal().offset();
    let ecrits_after = writer.wal().records_ecrits();

    assert!(report.global_failure.is_none(), "zone saine (pas globale)");
    assert_eq!(report.degraded.len(), 1, "dégradé (échec de réparation)");
    // Aucune écriture de réparation n'a été émise (l'injection court-circuite
    // l'appel réel à ensure_engram_bit) : offset et records inchangés.
    assert_eq!(
        offset_before, offset_after,
        "offset inchangé (aucune réparation écrite)"
    );
    assert_eq!(ecrits_before, ecrits_after, "records_ecrits inchangé");

    // État canonique PUBLIÉ (membres+poids exacts), poids NON modifiés.
    let e = manager.store().get(EngramId(1)).unwrap();
    assert_eq!(
        e.ensemble,
        vec![(a, 0.75)],
        "état canonique publié, poids inchangés (aucun apprentissage)"
    );

    // Rappel natif : repli dégradé opérationnel malgré l'échec de réparation.
    let native = manager.store().recall(a).unwrap();
    assert!(
        native.contains(&EngramId(1)),
        "repli dégradé après échec de réparation"
    );
}
