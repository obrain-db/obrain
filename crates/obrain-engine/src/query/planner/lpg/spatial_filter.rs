//! `WHERE distance(n.pos, point(lon, lat)) <= r` → scan d'index spatial.
//!
//! ── POURQUOI CE MODULE EXISTE ──────────────────────────────────────────────
//!
//! Sans lui, `distance(...) < r` est un `FilterOperator` : chaque nœud du label
//! est hydraté et le prédicat évalué un par un. Mesuré (banc `spatial_bench`,
//! dgx, 21/11/2026) : **29,0 ms** à 100 000 points, quelle que soit la
//! sélectivité — le coût est le balayage, pas la géométrie. Le même prédicat
//! servi par l'index : **2,91 µs** à 0,03 %, **50,4 µs** à 1 %.
//!
//! ── CE QUI GARANTIT L'EXACTITUDE ───────────────────────────────────────────
//!
//! Le plan rendu NE RÉAPPLIQUE PAS le filtre : il doit donc être exact, et il
//! l'est pour trois raisons vérifiables.
//!
//! 1. L'index lui-même rend des distances EXACTES (`dans_rayon` élargit la
//!    requête de la marge du `f32`, puis RETRANCHE en `f64`).
//! 2. La fonction `distance()` de GQL et l'index appellent la MÊME
//!    `haversine_m` (`obrain-core/index/geo.rs`) : deux formules divergeraient
//!    sur la frontière du rayon.
//! 3. La comparaison stricte est honorée : `distance(...) < r` retire `d == r`,
//!    que `dans_rayon` (inclusif par nature) aurait rendu.
//!
//! ── GARDES, dans l'ordre des pièges ────────────────────────────────────────
//!
//! * scan LIBRE uniquement (pas d'input) et AVEC label — un index est par
//!   label ;
//! * point et rayon LITTÉRAUX : un paramètre lié à l'exécution ne peut pas
//!   choisir un plan. On retombe alors sur le balayage, qui reste correct ;
//! * aucune index déclaré pour ce couple → `None`, jamais `Some(vec![])` ;
//! * visibilité MVCC et appartenance au label revérifiées, comme le fait
//!   déjà `plan_range_filter`.

use super::{
    BinaryOp, FilterOp, LogicalExpression, LogicalOperator, NodeId, NodeListOperator, Operator,
    Result,
};

/// Un littéral numérique, ou `None`.
fn nombre(expr: &LogicalExpression) -> Option<f64> {
    match expr {
        LogicalExpression::Literal(v) => obrain_core::index::geo::nombre_f64(v),
        _ => None,
    }
}

/// `(lon, lat)` d'un argument de `distance()` qui désigne un point FIXE.
///
/// Trois formes, et pas une de trop : le plieur de constantes peut avoir déjà
/// remplacé `point(a, b)` par sa valeur, et un utilisateur peut écrire la liste
/// directement.
fn point_fixe(expr: &LogicalExpression) -> Option<(f64, f64)> {
    match expr {
        LogicalExpression::FunctionCall { name, args, .. }
            if name.eq_ignore_ascii_case("point") && args.len() == 2 =>
        {
            Some((nombre(&args[0])?, nombre(&args[1])?))
        }
        LogicalExpression::Literal(v) => obrain_core::index::geo::point_lon_lat(v),
        LogicalExpression::List(items) if items.len() == 2 => {
            Some((nombre(&items[0])?, nombre(&items[1])?))
        }
        _ => None,
    }
}

impl super::Planner {
    /// Tente de servir `distance(<var>.<prop>, <point fixe>) <OP> <rayon>` par
    /// l'index spatial. `Ok(None)` = pas applicable, le filtre générique reste.
    pub(super) fn try_plan_filter_spatial(
        &self,
        filter: &FilterOp,
    ) -> Result<Option<(Box<dyn Operator>, Vec<String>)>> {
        let Some(poignee) = self.index_spatiaux.as_ref() else {
            return Ok(None);
        };

        // 1. Le motif de scan : nu, et avec un label.
        let LogicalOperator::NodeScan(scan) = filter.input.as_ref() else {
            return Ok(None);
        };
        if scan.input.is_some() {
            return Ok(None);
        }
        let Some(label) = scan.label.as_ref() else {
            return Ok(None);
        };

        // 2. Le prédicat, dans les deux sens d'écriture.
        let LogicalExpression::Binary { left, op, right } = &filter.predicate else {
            return Ok(None);
        };
        let (appel, cote_rayon, strict) = match (left.as_ref(), right.as_ref()) {
            // `distance(...) < r` / `<= r`
            (f @ LogicalExpression::FunctionCall { .. }, r)
                if matches!(op, BinaryOp::Lt | BinaryOp::Le) =>
            {
                (f, r, matches!(op, BinaryOp::Lt))
            }
            // `r > distance(...)` / `>= distance(...)`
            (l, f @ LogicalExpression::FunctionCall { .. })
                if matches!(op, BinaryOp::Gt | BinaryOp::Ge) =>
            {
                (f, l, matches!(op, BinaryOp::Gt))
            }
            _ => return Ok(None),
        };
        let LogicalExpression::FunctionCall { name, args, .. } = appel else {
            return Ok(None);
        };
        if !name.eq_ignore_ascii_case("distance") || args.len() != 2 {
            return Ok(None);
        }
        let LogicalExpression::Property { variable, property } = &args[0] else {
            return Ok(None);
        };
        if variable != &scan.variable {
            return Ok(None);
        }
        let Some((lon, lat)) = point_fixe(&args[1]) else {
            return Ok(None);
        };
        let Some(rayon) = nombre(cote_rayon) else {
            return Ok(None);
        };
        // NaN et négatif → balayage : `is_nan` dit ce que `!(r >= 0)` taisait.
        if rayon.is_nan() || rayon < 0.0 {
            return Ok(None);
        }

        // 3. L'index : `None` (aucun index pour ce couple) → balayage.
        let cle = format!("{label}:{property}");
        let Ok(hits) = poignee.rayon(&cle, lon, lat, rayon) else {
            return Ok(None);
        };

        // 4. Exactitude : stricte, label, visibilité MVCC.
        let epoch = self.viewing_epoch;
        let tx = self.transaction_id;
        let ids: Vec<NodeId> = hits
            .into_iter()
            .filter(|(_, d)| !strict || *d < rayon)
            .map(|(id, _)| id)
            .filter(|id| self.store.node_has_label(*id, label).unwrap_or(false))
            .filter(|id| match tx {
                Some(t) => self.store.is_node_visible_versioned(*id, epoch, t),
                None => self.store.is_node_visible_at_epoch(*id, epoch),
            })
            .collect();

        Ok(Some((
            Box::new(NodeListOperator::new(ids, 2048)),
            vec![scan.variable.clone()],
        )))
    }
}
