//! Cumulative wall-clock time per query phase (obrain#189).
//!
//! Each phase of a query accumulates its elapsed time in a process-wide
//! counter. A caller reads [`phases_ns`] before and after a query and takes
//! the difference: this attributes the time of one query to its phases
//! (session, parse, planning, execution, ...) without a profiler.
//!
//! The counters are plain relaxed atomics: the overhead is one `Instant::now`
//! and one `fetch_add` per phase per query.

use std::sync::atomic::{AtomicU64, Ordering};

/// The measured phases, in the order in which a query goes through them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// `ObrainDB::session()`: creating the temporary session (`with_session`).
    Session,
    /// `Session::execute_language`: building the `QueryProcessor` (inside the
    /// auto-commit wrapper, before `process`).
    Preparation,
    /// `QueryProcessor::translate_lpg`: parse and translation to a logical plan.
    Parse,
    /// `substitute_params`: parameter substitution in the logical plan.
    Parametres,
    /// `Binder::bind`: semantic validation.
    Liaison,
    /// `Optimizer::optimize`: logical optimisation.
    Optimisation,
    /// `Planner::plan` / `plan_profiled`: physical planning (operator
    /// construction).
    PlanPhysique,
    /// `Executor::execute`: pulling every chunk from the operator tree and
    /// collecting the result rows.
    Execution,
    /// Drop of the physical operator tree, at the end of `process_lpg`.
    Liberation,
    /// `Session::with_auto_commit`: the whole wrapper (begin / body /
    /// commit or rollback). Contains the phases above except `Session`.
    AutoCommit,
}

const NB_PHASES: usize = 10;

/// Every phase, in declaration order (index `i` of [`phases_ns`]).
pub const PHASES: [Phase; NB_PHASES] = [
    Phase::Session,
    Phase::Preparation,
    Phase::Parse,
    Phase::Parametres,
    Phase::Liaison,
    Phase::Optimisation,
    Phase::PlanPhysique,
    Phase::Execution,
    Phase::Liberation,
    Phase::AutoCommit,
];

static COMPTEURS: [AtomicU64; NB_PHASES] = [const { AtomicU64::new(0) }; NB_PHASES];

/// Adds `ns` nanoseconds to `phase`.
pub fn ajoute(phase: Phase, ns: u64) {
    COMPTEURS[phase as usize].fetch_add(ns, Ordering::Relaxed);
}

/// Cumulative nanoseconds per phase since process start, indexed as [`PHASES`].
pub fn phases_ns() -> [u64; NB_PHASES] {
    std::array::from_fn(|i| COMPTEURS[i].load(Ordering::Relaxed))
}

/// Runs `f` and adds its elapsed time to `phase`.
pub fn mesure<T>(phase: Phase, f: impl FnOnce() -> T) -> T {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let depart = std::time::Instant::now();
        let resultat = f();
        ajoute(phase, depart.elapsed().as_nanos() as u64);
        resultat
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = phase;
        f()
    }
}
