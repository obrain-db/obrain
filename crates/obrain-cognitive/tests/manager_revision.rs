//! Révision cohérente au niveau d'EngramManager, avec contre-épreuves :
//! poids exacts (règle de reconsolidation), déduplication [C,C], distracteur,
//! indépendance de la signature, réouverture avec tolérances f32.

#![cfg(all(feature = "engram", feature = "substrate"))]

use std::sync::{Arc, Mutex};

use obrain_common::types::NodeId;
use obrain_cognitive::engram::traits::{InMemoryVectorIndex, VectorIndex};
use obrain_cognitive::engram::{EngramManager, EngramManagerConfig, RevisionError, StoreError};
use obrain_substrate::{SubstrateFile, SyncMode, Writer as SubstrateWriter};

fn weight_of(ens: &[(NodeId, f64)], n: NodeId) -> f64 {
    ens.iter().find(|(m, _)| *m == n).map(|(_, w)| *w).unwrap_or(0.0)
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
        let manager =
            EngramManager::new(EngramManagerConfig::default(), Arc::new(InMemoryVectorIndex::new()));
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
        let rev = manager.revise(formed_id, &[c], 0.5).expect("révision effectuée");
        after_sig = rev.signature.clone();

        // Règle de reconsolidation : poids unitaires + erreur 0.5 →
        // A=0.95, B=0.95, C=0.25.
        assert!((weight_of(&rev.members, a) - 0.95).abs() < 1e-6, "A décroît à 0.95");
        assert!((weight_of(&rev.members, b) - 0.95).abs() < 1e-6, "B décroît à 0.95");
        assert!((weight_of(&rev.members, c) - 0.25).abs() < 1e-6, "C ajouté à 0.25");
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
        let manager =
            EngramManager::new(EngramManagerConfig::default(), Arc::new(InMemoryVectorIndex::new()));
        let sub = SubstrateFile::open(&path).unwrap();
        let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
        manager.attach_substrate(Arc::new(writer));

        let engram = manager.store().get(formed_id).expect("engramme rechargé");
        let members = members_of(&engram.ensemble);
        assert!(members.contains(&a) && members.contains(&b) && members.contains(&c),
            "membres {{A,B,C}} conservés : {members:?}");
        assert!((weight_of(&engram.ensemble, a) - 0.95).abs() < 1e-6, "poids A conservé (f32)");
        assert!((weight_of(&engram.ensemble, b) - 0.95).abs() < 1e-6, "poids B conservé (f32)");
        assert!((weight_of(&engram.ensemble, c) - 0.25).abs() < 1e-6, "poids C conservé (f32)");
    }
}

/// Contre-épreuve : une observation [C, C] ne doit pas ajouter C deux fois.
#[test]
fn reconsolidation_deduplique_les_doublons() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));

    let manager =
        EngramManager::new(EngramManagerConfig::default(), Arc::new(InMemoryVectorIndex::new()));
    let sub = SubstrateFile::create(&path).unwrap();
    let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
    manager.attach_substrate(Arc::new(writer));

    let id = manager.store().form(&[a, b]).unwrap();
    let rev = manager.revise(id, &[c, c], 0.5).expect("révision");
    let cs: Vec<NodeId> = rev.members.iter().filter(|(n, _)| *n == c).map(|(n, _)| *n).collect();
    assert_eq!(cs.len(), 1, "C ajouté une seule fois malgré [C, C]");
    assert_eq!(rev.members.len(), 3, "3 membres, pas de doublon");
}

/// Observation invalide : erreur non finie → refusée (pas d'état persistant).
#[test]
fn observation_invalide_refusee() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));

    let manager =
        EngramManager::new(EngramManagerConfig::default(), Arc::new(InMemoryVectorIndex::new()));
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
    assert_eq!(e.ensemble.len(), 2, "état inchangé après observation invalide");
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
    assert!(members.contains(&c) && members.contains(&d),
        "les deux révisions sont appliquées (pas de mise à jour perdue) : {members:?}");
    assert_eq!(members.len(), 4, "aucun doublon, aucune perte : {members:?}");
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
            Arc::new(Self { inner: InMemoryVectorIndex::new(), history: history.clone() }),
            history,
        )
    }
}

impl VectorIndex for RecordingIndex {
    fn upsert(&self, id: &str, v: &[f64]) {
        self.inner.upsert(id, v);
        self.history.lock().unwrap().push((id.to_string(), v.to_vec()));
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
    let t1 = std::thread::spawn(move || { b1.wait(); m1.revise(id, &[c], 0.5).unwrap(); });
    let t2 = std::thread::spawn(move || { b2.wait(); m2.revise(id, &[d], 0.5).unwrap(); });
    t1.join().unwrap();
    t2.join().unwrap();

    // État canonique final (4 membres) et sa signature.
    let final_engram = manager.store().get(id).expect("engramme présent");
    let final_sig = final_engram.spectral_signature.clone();
    assert_eq!(members_of(&final_engram.ensemble).len(), 4, "état final : 4 membres");

    // La DERNIÈRE publication d'index pour cet engramme DOIT être l'état final.
    let hist = history.lock().unwrap().clone();
    let last = hist.iter().rev().find(|(i, _)| i == &id.to_string())
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

    let manager =
        EngramManager::new(EngramManagerConfig::default(), Arc::new(InMemoryVectorIndex::new()));
    let sub = SubstrateFile::create(&path).unwrap();
    let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
    manager.attach_substrate(Arc::new(writer));

    let id = manager.store().form(&[a, b]).unwrap();
    let before = manager.store().get(id).unwrap().clone();

    // Changement d'identité logique refusé.
    let r = manager.store().mutate(id, |e| { e.id = obrain_cognitive::engram::EngramId(999); });
    assert!(matches!(r, Err(StoreError::ValidationFailed(_))), "identité refusée");
    assert_eq!(manager.store().get(id).unwrap().ensemble, before.ensemble, "état intact (identité)");

    // Membre dupliqué refusé.
    let r = manager.store().mutate(id, |e| { e.ensemble.push((a, 1.0)); });
    assert!(matches!(r, Err(StoreError::ValidationFailed(_))), "doublon refusé");
    assert_eq!(manager.store().get(id).unwrap().ensemble, before.ensemble, "état intact (doublon)");

    // Poids non fini (f64) refusé.
    let r = manager.store().mutate(id, |e| { e.ensemble[0].1 = f64::NAN; });
    assert!(matches!(r, Err(StoreError::ValidationFailed(_))), "NaN refusé");
    assert_eq!(manager.store().get(id).unwrap().ensemble, before.ensemble, "état intact (NaN)");

    // Dépassement f32 refusé (substrate) : 1e300 est fini en f64 mais inf en f32.
    let r = manager.store().mutate(id, |e| { e.ensemble[0].1 = 1e300; });
    assert!(matches!(r, Err(StoreError::ValidationFailed(_))), "dépassement f32 refusé");

    // État COMPLET inchangé après chaque refus (pas seulement len()).
    let e = manager.store().get(id).unwrap();
    assert_eq!(e.ensemble, before.ensemble, "état canonique complet inchangé après refus");
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

    let manager =
        EngramManager::new(EngramManagerConfig::default(), Arc::new(InMemoryVectorIndex::new()));
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
    assert_eq!(after.ensemble, before.ensemble, "cache inchangé (candidat non committé)");

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
    let cs: Vec<NodeId> = retry.members.iter().filter(|(n, _)| *n == c).map(|(n, _)| *n).collect();
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
        let manager =
            EngramManager::new(EngramManagerConfig::default(), Arc::new(InMemoryVectorIndex::new()));
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
    let manager =
        EngramManager::new(EngramManagerConfig::default(), Arc::new(InMemoryVectorIndex::new()));
    let sub = SubstrateFile::open(&path).unwrap();
    let writer = Arc::new(SubstrateWriter::new(sub, SyncMode::Never).unwrap());
    manager.attach_substrate(writer.clone());

    // Couples EXACTS membre/poids dans le SUBSTRAT (zip membres↔poids).
    let (members, weights) = writer.read_engram_state(id.0 as u16).unwrap().unwrap();
    let ws = weights.expect("poids présents");
    assert_eq!(members.len(), 3, "3 membres restaurés");
    let sw = |n: u32| -> f32 {
        members.iter().zip(&ws).find(|(m, _)| **m == n).map(|(_, w)| *w).unwrap()
    };
    assert!((sw(a.0 as u32) - 0.95).abs() < 1e-6, "substrat A=0.95");
    assert!((sw(b.0 as u32) - 0.95).abs() < 1e-6, "substrat B=0.95");
    assert!((sw(c.0 as u32) - 0.25).abs() < 1e-6, "substrat C=0.25");

    // CACHE restauré (manager.store().get) : mêmes couples exacts.
    let restored = manager.store().get(id).unwrap();
    assert!((weight_of(&restored.ensemble, a) - 0.95).abs() < 1e-6, "cache A=0.95");
    assert!((weight_of(&restored.ensemble, b) - 0.95).abs() < 1e-6, "cache B=0.95");
    assert!((weight_of(&restored.ensemble, c) - 0.25).abs() < 1e-6, "cache C=0.25");
    assert!(!restored.spectral_signature.is_empty(), "signature restaurée");

    // Bitset RÉPARÉ au chargement : le bit de C est reconstruit depuis les
    // membres persistés. (Présence du bit = nécessaire, PAS suffisante pour la
    // complétude transactionnelle — c'est un filtre Bloom.)
    let bitset_c = writer.engram_bitset(c.0 as u32).unwrap();
    let expected_mask = 1u64 << ((id.0 as u16) & 0x3F);
    assert_ne!(bitset_c & expected_mask, 0, "bitset de C réparé (bit posé)");

    // Cohérence des deux chemins de rappel RESTAURÉE (plus de divergence).
    let native = manager.store().recall(c).expect("rappel natif");
    assert!(native.iter().any(|e| *e == id), "rappel natif trouve C (bitset réparé)");
    let mgr = manager.recall(&[c], 5);
    assert!(mgr.iter().any(|r| r.engram_id == id), "rappel manager trouve C");

    // Nouvelle tentative APRÈS réouverture : reconsolidation décroît TOUT.
    let rev = manager.revise(id, &[c], 0.5).expect("reprise réussie");
    assert!((weight_of(&rev.members, a) - 0.9025).abs() < 1e-6, "A=0.9025 après retry");
    assert!((weight_of(&rev.members, b) - 0.9025).abs() < 1e-6, "B=0.9025 après retry");
    assert!((weight_of(&rev.members, c) - 0.2375).abs() < 1e-6, "C=0.2375 après retry");
    let cs: Vec<NodeId> = rev.members.iter().filter(|(n, _)| *n == c).map(|(n, _)| *n).collect();
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
        let manager =
            EngramManager::new(EngramManagerConfig::default(), Arc::new(InMemoryVectorIndex::new()));
        let sub = SubstrateFile::create(&path).unwrap();
        let writer = Arc::new(SubstrateWriter::new(sub, SyncMode::Never).unwrap());
        manager.attach_substrate(writer.clone());
        id = manager.store().form(&[a, b]).unwrap();
        // après form, les bits A/B sont posés.
    }
    // Réouverture saine : les bits sont déjà présents → ensure renvoie false.
    let sub = SubstrateFile::open(&path).unwrap();
    let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
    assert_eq!(
        writer.ensure_engram_bit(a.0 as u32, id.0 as u16).unwrap(),
        false,
        "bit A déjà présent : aucune écriture"
    );
    assert_eq!(
        writer.ensure_engram_bit(b.0 as u32, id.0 as u16).unwrap(),
        false,
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
        let manager =
            EngramManager::new(EngramManagerConfig::default(), Arc::new(InMemoryVectorIndex::new()));
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
    let manager =
        EngramManager::new(EngramManagerConfig::default(), Arc::new(InMemoryVectorIndex::new()));
    let report = manager.attach_substrate(writer.clone());

    let offset_after = writer.wal().offset();
    let ecrits_after = writer.wal().records_ecrits();
    let tampon_after = writer.wal().records_en_tampon();

    assert_eq!(report.coherent, 1, "un engramme cohérent chargé");
    assert!(report.degraded.is_empty(), "aucun dégradé");
    assert_eq!(report.rejected.len(), 0, "aucun rejet");
    assert_eq!(offset_before, offset_after, "offset WAL inchangé (aucune écriture)");
    assert_eq!(ecrits_before, ecrits_after, "records_ecrits inchangé");
    assert_eq!(tampon_before, tampon_after, "records_en_tampon inchangé");
}
