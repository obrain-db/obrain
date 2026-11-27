//! Témoins de NON-recertification au remappage des engrammes (T6-recert).
//!
//! `bulk_sort_by_hilbert` et `compact_community` ré-écrivent les MEMBRES d'un
//! engramme (les payloads sont des slots nœuds) puis appellent
//! `EngramZone::rebind_weights_after_remap` pour recalculer l'empreinte d'un
//! snapshot de poids LIÉ. La question posée : ce ré-attachement reconnaît-il
//! les poids liés par leur CARDINALITÉ seule (`len == membres + 2`) ou par leur
//! IDENTITÉ (empreinte cohérente AVANT le remap) ?
//!
//! Trois témoins, chacun exercé en application directe ET au rejeu :
//!   1. engramme VALIDE (empreinte cohérente) → conservation EXACTE membre↔poids ;
//!   2. empreinte DIVERGENTE mais cardinalité correcte → ne doit PAS être
//!      blanchi (recertifié) par le remap ;
//!   3. legacy NON LIÉ (poids positionnels sans empreinte) → ne doit JAMAIS
//!      être certifié rétroactivement.
//!
//! Le témoin 2 encode le comportement SOUHAITÉ : il échoue tant que
//! `rebind_weights_after_remap` (engram.rs:249) recertifie sur la seule
//! cardinalité. Un échec du témoin 2 est donc la PREUVE du défaut, pas un
//! test cassé.

use obrain_substrate::replay::replay_from;
use obrain_substrate::{
    EdgeRecord, EngramIncoherence, NodeRecord, PackedScarUtilAff, SubstrateError, SubstrateFile,
    U48, Writer, f32_to_q1_15, wal_io::SyncMode,
};

mod common;

const ENGRAM: u16 = 1;
const MEMBERS: [u32; 5] = [1, 2, 3, 4, 5];
const WEIGHTS: [f32; 5] = [0.1, 0.2, 0.3, 0.4, 0.5];
/// Même cardinalité que MEMBERS, valeurs différentes → empreinte divergente.
const MEMBERS_DIVERGENTS: [u32; 5] = [5, 4, 3, 2, 1];

fn sample_node(community: u32, centrality: u16) -> NodeRecord {
    NodeRecord {
        label_bitset: 1,
        first_edge_off: U48::default(),
        first_prop_off: U48::default(),
        community_id: community,
        energy: f32_to_q1_15(0.5),
        scar_util_affinity: PackedScarUtilAff::new(0, 0, 0, false).pack(),
        centrality_cached: centrality,
        flags: 0,
        tenant_id: 0,
        _reserve: 0,
    }
}

fn sample_edge(src: u32, dst: u32) -> EdgeRecord {
    EdgeRecord {
        src,
        dst,
        edge_type: 1,
        weight_u16: f32_to_q1_15(0.9),
        next_from: U48::default(),
        next_to: U48::default(),
        first_prop_off: U48::ZERO,
        ricci_u8: 128,
        flags: 0,
        engram_tag: 0,
        _pad: [0; 2],
    }
}

/// Classification de l'état membres/poids OBSERVÉ, matchable sans décoder de
/// texte d'erreur.
#[derive(Debug, Clone, PartialEq)]
enum Etat {
    /// Lecture cohérente : poids servis (empreinte valide).
    Coherent(Vec<f32>),
    /// Empreinte en contradiction avec les membres (association corrompue).
    Divergent,
    /// Poids non liés (legacy positionnel) — refus sans certification.
    NonLie,
    /// Tout autre résultat (absence, cardinalité, erreur inattendue).
    Autre(String),
}

fn observer(w: &Writer) -> Etat {
    match w.read_engram_state(ENGRAM) {
        Ok(Some((_, Some(ws)))) => Etat::Coherent(ws),
        Err(SubstrateError::EngramIncoherent {
            incoherence: EngramIncoherence::EmpreinteDivergente { .. },
            ..
        }) => Etat::Divergent,
        Err(SubstrateError::EngramIncoherent {
            incoherence: EngramIncoherence::PoidsNonLies { .. },
            ..
        }) => Etat::NonLie,
        Ok(Some((_, None))) => Etat::Autre("absence de poids (legacy sans zone)".into()),
        Ok(None) => Etat::Autre("engramme absent".into()),
        Err(e) => Etat::Autre(format!("{e:?}")),
    }
}

/// Rapport d'un cycle remap → réouverture → rejeu.
#[derive(Debug)]
struct Cycle {
    /// État AVANT le remap (preuve que le témoin est bien construit).
    avant: Etat,
    /// État APRÈS le remap, en application directe.
    direct: Etat,
    /// État APRÈS réouverture + rejeu depuis 0.
    rejeu: Etat,
    /// Membres lus après remap direct.
    membres_direct: Option<Vec<u32>>,
    /// Membres lus après rejeu.
    membres_rejeu: Option<Vec<u32>>,
}

/// Pose les nœuds brouillés pour `bulk_sort_by_hilbert` (vraie permutation).
fn poser_noeuds_bulk(w: &Writer) -> u32 {
    let layout: Vec<u32> = vec![2, 0, 2, 1, 0, 1];
    let centrality: Vec<u16> = vec![1000, 2000, 3000, 500, 1500, 800];
    let n = layout.len() as u32;
    for i in 0..n {
        w.write_node(i, sample_node(layout[i as usize], centrality[i as usize]))
            .unwrap();
    }
    w.write_edge(0, sample_edge(1, 2)).unwrap();
    w.write_edge(1, sample_edge(3, 4)).unwrap();
    n
}

/// Pose les nœuds entrelacés pour `compact_community` (relocation réelle).
fn poser_noeuds_compact(w: &Writer) {
    let layout: Vec<u32> = vec![1, 2, 1, 2, 1, 2]; // slots 1..6
    let centrality: Vec<u16> = vec![101, 201, 102, 202, 103, 203];
    for (i, (&cid, &c)) in layout.iter().zip(centrality.iter()).enumerate() {
        w.write_node((i + 1) as u32, sample_node(cid, c)).unwrap();
    }
}

fn bulk_cycle(setup: impl FnOnce(&Writer)) -> Cycle {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (avant, direct, membres_direct) = {
        let sub = SubstrateFile::create(&path).unwrap();
        let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
        let n = poser_noeuds_bulk(&w);
        setup(&w);
        w.commit().unwrap();
        let avant = observer(&w);
        w.vider_fenetre_de_rejeu().unwrap();
        w.bulk_sort_by_hilbert(n, 2, 4, 32).unwrap();
        let direct = observer(&w);
        let membres_direct = w.engram_members(ENGRAM).unwrap();
        (avant, direct, membres_direct)
    };
    let sub = SubstrateFile::open(&path).unwrap();
    let stats = replay_from(&sub, 0).unwrap();
    assert_eq!(stats.decode_errors, 0, "frame indécodable au rejeu");
    let w = Writer::new(sub, SyncMode::Never).unwrap();
    let rejeu = observer(&w);
    let membres_rejeu = w.engram_members(ENGRAM).unwrap();
    Cycle {
        avant,
        direct,
        rejeu,
        membres_direct,
        membres_rejeu,
    }
}

fn compact_cycle(setup: impl FnOnce(&Writer)) -> Cycle {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (avant, direct, membres_direct) = {
        let sub = SubstrateFile::create(&path).unwrap();
        let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
        poser_noeuds_compact(&w);
        setup(&w);
        w.commit().unwrap();
        let avant = observer(&w);
        w.vider_fenetre_de_rejeu().unwrap();
        let result = w.compact_community(1, 7, 0, 4, 32).unwrap();
        assert!(
            !result.relocations.is_empty(),
            "aucune relocation — témoin non probant"
        );
        let direct = observer(&w);
        let membres_direct = w.engram_members(ENGRAM).unwrap();
        (avant, direct, membres_direct)
    };
    let sub = SubstrateFile::open(&path).unwrap();
    let stats = replay_from(&sub, 0).unwrap();
    assert_eq!(stats.decode_errors, 0, "frame indécodable au rejeu");
    let w = Writer::new(sub, SyncMode::Never).unwrap();
    let rejeu = observer(&w);
    let membres_rejeu = w.engram_members(ENGRAM).unwrap();
    Cycle {
        avant,
        direct,
        rejeu,
        membres_direct,
        membres_rejeu,
    }
}

// ─────────────────────────────────────────────────────────────────────
// Témoin 1 — engramme VALIDE : conservation EXACTE membre↔poids (légitime).
// ─────────────────────────────────────────────────────────────────────

#[test]
fn bulk_conserve_poids_lies_valides() {
    let c = bulk_cycle(|w| {
        w.set_engram_state(ENGRAM, MEMBERS.to_vec(), WEIGHTS.to_vec())
            .unwrap();
    });
    assert_eq!(c.avant, Etat::Coherent(WEIGHTS.to_vec()), "pré-remap incohérent");
    assert_eq!(
        c.direct,
        Etat::Coherent(WEIGHTS.to_vec()),
        "poids liés valides altérés par le remap direct"
    );
    assert_eq!(
        c.rejeu,
        Etat::Coherent(WEIGHTS.to_vec()),
        "poids liés valides altérés au rejeu"
    );
    assert_ne!(
        c.membres_direct.as_deref(),
        Some(MEMBERS.as_slice()),
        "remap sans effet sur les membres — témoin non probant"
    );
    assert_eq!(c.membres_direct, c.membres_rejeu, "membres divergents direct/rejeu");
}

#[test]
fn compact_conserve_poids_lies_valides() {
    let c = compact_cycle(|w| {
        w.set_engram_state(ENGRAM, MEMBERS.to_vec(), WEIGHTS.to_vec())
            .unwrap();
    });
    assert_eq!(c.avant, Etat::Coherent(WEIGHTS.to_vec()), "pré-remap incohérent");
    assert_eq!(
        c.direct,
        Etat::Coherent(WEIGHTS.to_vec()),
        "poids liés valides altérés par la compaction directe"
    );
    assert_eq!(
        c.rejeu,
        Etat::Coherent(WEIGHTS.to_vec()),
        "poids liés valides altérés au rejeu"
    );
    assert_ne!(
        c.membres_direct.as_deref(),
        Some(MEMBERS.as_slice()),
        "compaction sans effet sur les membres — témoin non probant"
    );
    assert_eq!(c.membres_direct, c.membres_rejeu, "membres divergents direct/rejeu");
}

// ─────────────────────────────────────────────────────────────────────
// Témoin 2 — empreinte DIVERGENTE, cardinalité correcte : ne doit PAS être
// blanchi. Comportement SOUHAITÉ (échoue tant que le défaut subsiste).
// ─────────────────────────────────────────────────────────────────────

#[test]
fn bulk_ne_recertifie_pas_une_empreinte_divergente() {
    let c = bulk_cycle(|w| {
        w.set_engram_state(ENGRAM, MEMBERS.to_vec(), WEIGHTS.to_vec())
            .unwrap();
        // Mutation des membres SEULE, même cardinalité → empreinte divergente.
        w.set_engram_members(ENGRAM, MEMBERS_DIVERGENTS.to_vec())
            .unwrap();
    });
    assert_eq!(c.avant, Etat::Divergent, "le témoin n'est pas divergente avant remap");
    let direct_blanchi = c.direct != Etat::Divergent;
    let rejeu_blanchi = c.rejeu != Etat::Divergent;
    assert!(
        !direct_blanchi && !rejeu_blanchi,
        "DÉFAUT: recertification d'une association divergente jamais vérifiée — \
         direct={:?} (blanchi={direct_blanchi}), rejeu={:?} (blanchi={rejeu_blanchi})",
        c.direct, c.rejeu
    );
}

#[test]
fn compact_ne_recertifie_pas_une_empreinte_divergente() {
    let c = compact_cycle(|w| {
        w.set_engram_state(ENGRAM, MEMBERS.to_vec(), WEIGHTS.to_vec())
            .unwrap();
        w.set_engram_members(ENGRAM, MEMBERS_DIVERGENTS.to_vec())
            .unwrap();
    });
    assert_eq!(c.avant, Etat::Divergent, "le témoin n'est pas divergente avant remap");
    let direct_blanchi = c.direct != Etat::Divergent;
    let rejeu_blanchi = c.rejeu != Etat::Divergent;
    assert!(
        !direct_blanchi && !rejeu_blanchi,
        "DÉFAUT: recertification d'une association divergente jamais vérifiée — \
         direct={:?} (blanchi={direct_blanchi}), rejeu={:?} (blanchi={rejeu_blanchi})",
        c.direct, c.rejeu
    );
}

// ─────────────────────────────────────────────────────────────────────
// Témoin 3 — legacy NON LIÉ (positionnel sans empreinte) : jamais certifié.
// ─────────────────────────────────────────────────────────────────────

#[test]
fn bulk_ne_certifie_pas_les_poids_legacy_non_lies() {
    let c = bulk_cycle(|w| {
        w.set_engram_members(ENGRAM, MEMBERS.to_vec()).unwrap();
        common::set_engram_weights_legacy(w, ENGRAM, WEIGHTS.to_vec()).unwrap();
    });
    assert_eq!(c.avant, Etat::NonLie, "le témoin n'est pas legacy avant remap");
    assert_eq!(
        c.direct,
        Etat::NonLie,
        "poids legacy certifiés rétroactivement par le remap direct"
    );
    assert_eq!(
        c.rejeu,
        Etat::NonLie,
        "poids legacy certifiés rétroactivement au rejeu"
    );
}

#[test]
fn compact_ne_certifie_pas_les_poids_legacy_non_lies() {
    let c = compact_cycle(|w| {
        w.set_engram_members(ENGRAM, MEMBERS.to_vec()).unwrap();
        common::set_engram_weights_legacy(w, ENGRAM, WEIGHTS.to_vec()).unwrap();
    });
    assert_eq!(c.avant, Etat::NonLie, "le témoin n'est pas legacy avant remap");
    assert_eq!(
        c.direct,
        Etat::NonLie,
        "poids legacy certifiés rétroactivement par la compaction directe"
    );
    assert_eq!(
        c.rejeu,
        Etat::NonLie,
        "poids legacy certifiés rétroactivement au rejeu"
    );
}
