//! L'annulation coopérative, prouvée opérateur par opérateur.
//!
//! ── CE QUE CES TESTS DOIVENT EMPÊCHER ────────────────────────────────
//!
//! Un test d'annulation est vide par nature s'il se contente de constater
//! qu'une requête annulée rend une erreur : une requête assez courte rend
//! une erreur même sans mécanisme, et un opérateur dont on aurait oublié le
//! point de contrôle passerait le test sans broncher.
//!
//! Chaque cas ci-dessous est donc un TRIPLET, et le triplet est la preuve :
//!
//!   1. ANNULÉ      — le jeton est armé au N-ième appel de l'enfant.
//!                    L'opérateur doit rendre `Cancelled` ET s'être arrêté
//!                    à ~N appels, non à la fin de la source.
//!   2. CONTRE-ÉPREUVE (rien n'annule) — même montage, jeton jamais armé.
//!                    L'opérateur doit consommer la source ENTIÈRE. C'est ce
//!                    qui établit que le compte de l'étape 1 est bien un
//!                    arrêt anticipé et non la longueur naturelle du travail.
//!   3. ANTI-VACUITÉ (mécanisme neutralisé) — jeton armé, mais AUCUNE portée
//!                    installée. L'opérateur doit consommer la source
//!                    ENTIÈRE, exactement comme en 2.
//!
//! L'étape 3 est celle qui a de la valeur. Elle exécute le même code, avec
//! le même drapeau armé, et le seul écart avec l'étape 1 est que le point de
//! contrôle n'est pas consulté. Si un opérateur n'a pas de point de contrôle,
//! l'étape 1 se comporte comme l'étape 3 et le test ÉCHOUE. C'est la seule
//! façon de prouver que le détecteur détecte.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use obrain_common::types::LogicalType;
use obrain_common::utils::cancel::{self, CancelToken};
use obrain_core::execution::DataChunk;
use obrain_core::execution::operators::{
    AggregateExpr, DistinctOperator, HashAggregateOperator, HashJoinOperator, JoinType,
    LimitOperator, Operator, OperatorError, OperatorResult, SortKey, SortOperator,
};

/// Nombre de chunks que la source rend avant de s'épuiser.
///
/// Assez grand pour que « s'est arrêté tôt » et « est allé au bout » ne
/// puissent pas être confondus par accident.
const SOURCE: usize = 20_000;

/// Rang de l'appel auquel le chien de garde arme le jeton.
const ARMEMENT: usize = 100;

/// Source finie et INSTRUMENTÉE : elle compte ses appels, et peut armer un
/// jeton au N-ième — ce qui rend le test déterministe, sans thread ni
/// temporisation, donc sans flottement.
struct SourceComptee {
    appels: Arc<AtomicUsize>,
    reste: usize,
    /// `Some((rang, jeton))` : arme `jeton` quand le compteur atteint `rang`.
    armer_a: Option<(usize, CancelToken)>,
    colonnes: usize,
}

impl SourceComptee {
    fn new(appels: Arc<AtomicUsize>, colonnes: usize) -> Self {
        Self {
            appels,
            reste: SOURCE,
            armer_a: None,
            colonnes,
        }
    }

    fn armant(mut self, rang: usize, jeton: CancelToken) -> Self {
        self.armer_a = Some((rang, jeton));
        self
    }

    fn boitee(self) -> Box<dyn Operator> {
        Box::new(self)
    }
}

impl Operator for SourceComptee {
    fn next(&mut self) -> OperatorResult {
        let n = self.appels.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some((rang, jeton)) = &self.armer_a
            && n == *rang
        {
            jeton.cancel();
        }
        if self.reste == 0 {
            return Ok(None);
        }
        self.reste -= 1;

        // Une ligne par chunk : le nombre d'appels EST le nombre de lignes,
        // donc le compteur se lit sans conversion.
        let schema = vec![LogicalType::Int64; self.colonnes];
        let mut chunk = DataChunk::with_capacity(&schema, 1);
        for c in 0..self.colonnes {
            chunk
                .column_mut(c)
                .expect("colonne creee par le schema")
                .push_int64((n % 7) as i64);
        }
        chunk.set_count(1);
        Ok(Some(chunk))
    }

    fn reset(&mut self) {
        self.reste = SOURCE;
    }

    fn name(&self) -> &'static str {
        "SourceComptee"
    }
}

/// Épuise un opérateur et rend `(erreur éventuelle, appels à la source)`.
fn drainer(op: &mut dyn Operator, appels: &AtomicUsize) -> (Option<OperatorError>, usize) {
    loop {
        match op.next() {
            Ok(Some(_)) => {}
            Ok(None) => return (None, appels.load(Ordering::SeqCst)),
            Err(e) => return (Some(e), appels.load(Ordering::SeqCst)),
        }
    }
}

/// Le triplet, appliqué à un opérateur construit par `fabrique`.
///
/// `fabrique(compteur, jeton_a_armer_ou_none)` doit brancher UNE
/// `SourceComptee` sur le compteur donné (les opérateurs binaires en
/// branchent deux : voir le cas du join).
fn triplet<F>(nom: &str, fabrique: F)
where
    F: Fn(Arc<AtomicUsize>, Option<CancelToken>) -> Box<dyn Operator>,
{
    // ── 1. ANNULÉ ────────────────────────────────────────────────────
    let compteur = Arc::new(AtomicUsize::new(0));
    let jeton = CancelToken::new();
    let mut op = fabrique(Arc::clone(&compteur), Some(jeton.clone()));
    let (err, appels_annule) = cancel::scope_en_vigueur(&jeton, || drainer(op.as_mut(), &compteur));
    assert!(
        matches!(err, Some(OperatorError::Cancelled)),
        "{nom} : attendu OperatorError::Cancelled, obtenu {err:?}"
    );

    // ── 2. CONTRE-ÉPREUVE : rien n'annule ────────────────────────────
    let compteur2 = Arc::new(AtomicUsize::new(0));
    let jamais = CancelToken::new();
    let mut op2 = fabrique(Arc::clone(&compteur2), None);
    let (err2, appels_complet) =
        cancel::scope_en_vigueur(&jamais, || drainer(op2.as_mut(), &compteur2));
    assert!(
        err2.is_none(),
        "{nom} : sans annulation, la requete doit ABOUTIR — {err2:?}"
    );

    // ── 3. ANTI-VACUITÉ : jeton armé, mécanisme neutralisé ───────────
    //
    // Le jeton est armé AVANT même de commencer, mais aucune portée n'est
    // installée : les points de contrôle ne voient rien. Si l'opérateur
    // s'arrêtait quand même, c'est que le test de l'étape 1 mesurait autre
    // chose que le mécanisme.
    let compteur3 = Arc::new(AtomicUsize::new(0));
    let arme_hors_portee = CancelToken::new();
    arme_hors_portee.cancel();
    let mut op3 = fabrique(Arc::clone(&compteur3), Some(arme_hors_portee));
    let (err3, appels_neutralise) = drainer(op3.as_mut(), &compteur3);
    assert!(
        err3.is_none(),
        "{nom} : ANTI-VACUITE — un jeton arme HORS PORTEE ne doit rien \
         interrompre, or {err3:?}"
    );
    assert_eq!(
        appels_neutralise, appels_complet,
        "{nom} : ANTI-VACUITE — mecanisme neutralise, le travail doit etre \
         identique au cas nominal"
    );

    // ── le verdict ───────────────────────────────────────────────────
    assert!(
        appels_complet >= SOURCE,
        "{nom} : la source doit etre consommee entierement sans annulation \
         ({appels_complet} < {SOURCE})"
    );
    assert!(
        appels_annule < appels_complet / 10,
        "{nom} : l annulation doit ARRETER LE TRAVAIL, pas seulement rendre \
         une erreur a la fin — {appels_annule} appels contre {appels_complet} \
         sans annulation"
    );
    assert!(
        appels_annule <= ARMEMENT + 8,
        "{nom} : arret trop tardif — {appels_annule} appels alors que le \
         jeton est arme au {ARMEMENT}e. Le point de controle est-il DANS la \
         boucle qui draine ?"
    );
}

// ─────────────────────────────────────────────────────────────────────
// Les opérateurs BLOQUANTS : ceux qui consomment tout leur enfant à
// l'intérieur d'un SEUL `next()`. Ce sont eux qui échappaient entièrement
// au contrôle de délai de l'`Executor`, lequel ne s'exerce qu'ENTRE deux
// `next()`.
// ─────────────────────────────────────────────────────────────────────

#[test]
fn sort_s_interrompt_pendant_la_materialisation() {
    triplet("Sort", |compteur, jeton| {
        let mut src = SourceComptee::new(compteur, 1);
        if let Some(j) = jeton {
            src = src.armant(ARMEMENT, j);
        }
        Box::new(SortOperator::new(
            src.boitee(),
            vec![SortKey::ascending(0)],
            vec![LogicalType::Int64],
        ))
    });
}

#[test]
fn hash_aggregate_s_interrompt_pendant_la_construction() {
    triplet("HashAggregate", |compteur, jeton| {
        let mut src = SourceComptee::new(compteur, 1);
        if let Some(j) = jeton {
            src = src.armant(ARMEMENT, j);
        }
        Box::new(HashAggregateOperator::new(
            src.boitee(),
            vec![0],
            vec![AggregateExpr::count(0)],
            vec![LogicalType::Int64, LogicalType::Int64],
        ))
    });
}

#[test]
fn hash_join_s_interrompt_pendant_la_construction_de_la_table() {
    triplet("HashJoin(build)", |compteur, jeton| {
        // Le compteur est branché sur le côté BUILD : c'est lui qui est
        // consommé en entier dans le premier `next()`.
        let mut build = SourceComptee::new(compteur, 1);
        if let Some(j) = jeton {
            build = build.armant(ARMEMENT, j);
        }
        let probe = SourceComptee::new(Arc::new(AtomicUsize::new(0)), 1);
        Box::new(HashJoinOperator::new(
            probe.boitee(),
            build.boitee(),
            vec![0],
            vec![0],
            JoinType::Inner,
            vec![LogicalType::Int64, LogicalType::Int64],
        ))
    });
}

// ─────────────────────────────────────────────────────────────────────
// Les opérateurs en FLUX : ils rendent la main entre deux chunks, donc le
// contrôle de l'`Executor` finissait par s'y exercer. Le point de contrôle
// interne leur sert quand même — un `Distinct` sur un flux qui ne rend
// jamais de ligne nouvelle boucle sans jamais rien remonter.
// ─────────────────────────────────────────────────────────────────────

#[test]
fn distinct_s_interrompt_dans_sa_boucle() {
    triplet("Distinct", |compteur, jeton| {
        let mut src = SourceComptee::new(compteur, 1);
        if let Some(j) = jeton {
            src = src.armant(ARMEMENT, j);
        }
        Box::new(DistinctOperator::new(
            src.boitee(),
            vec![LogicalType::Int64],
        ))
    });
}

#[test]
fn limit_s_interrompt_dans_sa_boucle() {
    triplet("Limit", |compteur, jeton| {
        let mut src = SourceComptee::new(compteur, 1);
        if let Some(j) = jeton {
            src = src.armant(ARMEMENT, j);
        }
        // LIMIT plus grand que la source : la boucle draine tout.
        Box::new(LimitOperator::new(
            src.boitee(),
            SOURCE * 2,
            vec![LogicalType::Int64],
        ))
    });
}

// ─────────────────────────────────────────────────────────────────────
// La portée, vue depuis un opérateur
// ─────────────────────────────────────────────────────────────────────

#[test]
fn l_echeance_arrete_les_memes_boucles_que_le_jeton() {
    // UN SEUL MECANISME : le point de controle des boucles chaudes sert
    // l'echeance exactement comme il sert le jeton. Sans cela, la
    // `query_timeout` du moteur resterait cantonnee a l'`Executor`, donc
    // aveugle aux operateurs bloquants — ce qu'elle etait.
    let compteur = Arc::new(AtomicUsize::new(0));
    let src = SourceComptee::new(Arc::clone(&compteur), 1);
    let mut op = SortOperator::new(
        src.boitee(),
        vec![SortKey::ascending(0)],
        vec![LogicalType::Int64],
    );
    let _portee = cancel::installer_echeance(Some(
        web_time::Instant::now() - std::time::Duration::from_secs(1),
    ));
    let (err, appels) = drainer(&mut op, &compteur);
    assert!(
        matches!(err, Some(OperatorError::Cancelled)),
        "une echeance depassee doit arreter le tri, obtenu {err:?}"
    );
    assert!(
        appels < SOURCE / 10,
        "l echeance doit arreter TOT ({appels} appels)"
    );
}

#[test]
fn la_portee_ne_survit_pas_a_la_requete() {
    // Un jeton oublie dans le thread-local d'un thread de pool annulerait la
    // requete SUIVANTE servie par ce thread. `spawn_blocking` recycle ses
    // threads : ce serait une annulation fantome, impossible a attribuer.
    let jeton = CancelToken::new();
    jeton.cancel();
    cancel::scope_en_vigueur(&jeton, || {
        assert!(cancel::is_cancelled());
    });

    let compteur = Arc::new(AtomicUsize::new(0));
    let src = SourceComptee::new(Arc::clone(&compteur), 1);
    let mut op = SortOperator::new(
        src.boitee(),
        vec![SortKey::ascending(0)],
        vec![LogicalType::Int64],
    );
    let (err, appels) = drainer(&mut op, &compteur);
    assert!(err.is_none(), "requete suivante empoisonnee : {err:?}");
    assert!(appels >= SOURCE);
}
