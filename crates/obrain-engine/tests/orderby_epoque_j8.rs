//! NATIF-J8 — GARDE ORDER BY / min-max CONSCIENTE DE L'EPOQUE DE L'INDEX.
//!
//! Contrat de tests D'ABORD (docs/plan-j8.md). LA JUSTESSE PRIME SUR LA
//! VITESSE, TOUJOURS.
//!
//! Le probleme : la route ordonnee (`try_plan_order_by_index`) et le fast-path
//! `min`/`max` engagent l'index ORDONNE PAR VALEUR (`substrate.props.ord.idx.v2`)
//! des que `has_pending_writes()==false` et la vue est au present. Or une
//! ecriture AUTO-COMMIT (`TransactionId::SYSTEM`) ne pose JAMAIS
//! `attente_noeuds_active` (store.rs) : `has_pending_writes()` reste faux, mais
//! le WAL a avance et l'index n'a PAS ete rebati. Son drapeau `fiable` est fige
//! depuis l'ouverture / la derniere fusion. La route pouvait donc servir un
//! index PERIME => resultat FAUX (une ligne commitee visible manquante, ou une
//! ancienne extremite disparue encore rendue).
//!
//! La correction : n'engager l'index que si ses ANCRES (offset WAL + pages v2
//! au moment de la fusion) correspondent ENCORE a l'etat committe COURANT
//! (confrontation FRAICHE, en lecture, a chaque requete). Sinon repli balayage
//! — lent mais JAMAIS faux.
//!
//! Oracle : la VRAIE reponse est connue en Rust (les valeurs qu'on a inserees).
//! On n'assert donc pas « index == balayage » mais « resultat == verite ».
//!
//!   cargo test --release -p obrain-engine --test orderby_epoque_j8

#![cfg(feature = "gql")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use obrain_substrate::compteurs::mesurer_slots;

/// Base substrate SUR DISQUE (comme la prod) portant un noeud `:N {v: i}` par
/// entree. Rend `(db, tempdir)` — le tempdir doit vivre aussi longtemps que la
/// base.
fn db_valeurs(vals: &[i64]) -> (ObrainDB, tempfile::TempDir) {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).expect("open_substrate");
    {
        let s = db.session();
        for v in vals {
            s.execute(&format!("INSERT (:N {{v: {v}}})")).unwrap();
        }
    }
    (db, td)
}

/// Bootstrappe l'index ordonne par valeur sur `v` : flush (les proprietes
/// atterrissent en pages v2) puis `index rebuild` (pose les ancres FRAICHES et
/// declare l'index fiable pour la vue courante).
fn batir_index_v(db: &ObrainDB) {
    db.checkpoint().expect("checkpoint");
    let n = db.rebuild_ordered_value_index(&["v".to_string()]);
    assert_eq!(n, 1, "l'index ordonne-valeur sur `v` doit etre bati");
}

/// Ré-ancre l'index juste AVANT une mesure de route, et rend la mesure.
///
/// ── POURQUOI CE RÉ-ANCRAGE (défaut mesuré le 2026-09-17) ────────────────────
/// Le raccourci `min`/`max` et la route `ORDER BY … LIMIT k` ne servent l'index
/// ordonné que si ses ancres ÉGALENT l'état courant, **au bit près** :
///
/// ```text
/// g.fiable() && g.ancre_wal() == self.writer.wal().offset()
///            && g.ancre_pages() == props_zone.allocated_page_count()
/// ```
///
/// Toute écriture qui atterrit entre la construction de l'index et la requête
/// déplace l'offset du WAL et **invalide la route** — volontairement : un `max()`
/// servi par un index en retard serait FAUX. Ces tests écrivaient, bâtissaient
/// l'index, puis mesuraient plusieurs routes d'affilée : entre deux mesures, une
/// écriture de service (métriques, CDC, époque) suffisait à faire retomber la
/// suivante sur un balayage. Reproductible sous charge (`yes > /dev/null` × 6) :
/// `ORDER BY v DESC LIMIT 1` lisait alors 5 slots, `max(v)` en lisait 6.
///
/// On RÉ-ANCRE donc avant chaque mesure au lieu de supposer que rien n'a bougé.
/// L'assertion n'est pas affaiblie — elle reste « 0 slot » — et le test garde
/// ses dents : si la route ne s'engage pas même fraîchement ancrée, il échoue.
fn mesurer_route(db: &ObrainDB, q: &str) -> (Value, u64) {
    batir_index_v(db);
    scalaire_et_slots(db, q)
}

fn i64_de(v: &Value) -> i64 {
    match v {
        Value::Int64(n) => *n,
        autre => panic!("valeur non-Int64 : {autre:?}"),
    }
}

/// Premiere ligne, premiere colonne.
fn scalaire(db: &ObrainDB, q: &str) -> Value {
    let r = db.session().execute(q).unwrap_or_else(|e| panic!("requete refusee : {q}\n{e}"));
    r.rows[0][0].clone()
}

/// Comme `scalaire`, mais rend aussi le nombre de slots-noeuds balayes.
fn scalaire_et_slots(db: &ObrainDB, q: &str) -> (Value, u64) {
    let (r, slots) = mesurer_slots(|| {
        db.session().execute(q).unwrap_or_else(|e| panic!("requete refusee : {q}\n{e}"))
    });
    (r.rows[0][0].clone(), slots)
}

// ─────────────────────────────────────────────────────────────────────
// 1. INSTANT QUAND A JOUR — index frais, vue au present : 0-slot.
// ─────────────────────────────────────────────────────────────────────
#[test]
fn instant_quand_a_jour() {
    let (db, _td) = db_valeurs(&[5, 3, 9, 1, 7]);
    batir_index_v(&db);

    let (v_min, slots_min) = mesurer_route(&db, "MATCH (n) RETURN n.v ORDER BY n.v ASC LIMIT 1");
    assert_eq!(i64_de(&v_min), 1, "min global");
    assert_eq!(slots_min, 0, "ORDER BY ASC LIMIT 1 doit etre 0-slot (index engage), pas un balayage");

    let (v_max, slots_max) = mesurer_route(&db, "MATCH (n) RETURN n.v ORDER BY n.v DESC LIMIT 1");
    assert_eq!(i64_de(&v_max), 9, "max global");
    assert_eq!(slots_max, 0, "ORDER BY DESC LIMIT 1 doit etre 0-slot");

    let (v_mn, slots_mn) = mesurer_route(&db, "MATCH (n) RETURN min(n.v)");
    assert_eq!(i64_de(&v_mn), 1, "min() global");
    assert_eq!(slots_mn, 0, "min() doit etre 0-slot (fast-path index)");

    let (v_mx, slots_mx) = mesurer_route(&db, "MATCH (n) RETURN max(n.v)");
    assert_eq!(i64_de(&v_mx), 9, "max() global");
    assert_eq!(slots_mx, 0, "max() doit etre 0-slot (fast-path index)");
}

// ─────────────────────────────────────────────────────────────────────
// 2. CORRECTION SOUS ECRITURE — nouveau min commite APRES l'index, SANS
//    rebati. Le resultat DOIT etre correct (jamais l'ancienne reponse).
//    RED sur la base buggee (renvoie 1), VERT apres la correction.
// ─────────────────────────────────────────────────────────────────────
#[test]
fn correction_nouveau_min_sous_ecriture() {
    let (db, _td) = db_valeurs(&[5, 3, 9, 1, 7]);
    batir_index_v(&db);
    // Ancre posee. Verite courante : min=1.
    assert_eq!(i64_de(&scalaire(&db, "MATCH (n) RETURN n.v ORDER BY n.v ASC LIMIT 1")), 1);

    // ECRITURE AUTO-COMMIT qui devient le nouveau min, index NON rebati.
    db.session().execute("INSERT (:N {v: 0})").unwrap();

    // L'index est PERIME pour cette requete (WAL avance). Repli obligatoire.
    let vmin = scalaire(&db, "MATCH (n) RETURN n.v ORDER BY n.v ASC LIMIT 1");
    assert_eq!(i64_de(&vmin), 0, "ORDER BY ASC LIMIT 1 doit voir le nouveau min commite (0), jamais l'ancien (1)");

    let mn = scalaire(&db, "MATCH (n) RETURN min(n.v)");
    assert_eq!(i64_de(&mn), 0, "min() doit voir le nouveau min commite (0)");

    // Le top-2 aussi : [0, 1].
    let page = db.session().execute("MATCH (n) RETURN n.v ORDER BY n.v ASC LIMIT 2").unwrap();
    let vals: Vec<i64> = page.rows.iter().map(|r| i64_de(&r[0])).collect();
    assert_eq!(vals, vec![0, 1], "top-2 ASC apres ecriture");
}

// ─────────────────────────────────────────────────────────────────────
// 3. CORRECTION SOUS ECRITURE — le min EXISTANT supprime. La reponse ne
//    doit JAMAIS etre l'ancien min disparu.
// ─────────────────────────────────────────────────────────────────────
#[test]
fn correction_min_supprime_sous_ecriture() {
    let (db, _td) = db_valeurs(&[5, 3, 9, 1, 7]);
    batir_index_v(&db);
    assert_eq!(i64_de(&scalaire(&db, "MATCH (n) RETURN n.v ORDER BY n.v ASC LIMIT 1")), 1);

    // Supprime l'ancien min (v=1), index NON rebati.
    db.session().execute("MATCH (n:N {v: 1}) DETACH DELETE n").unwrap();

    let vmin = scalaire(&db, "MATCH (n) RETURN n.v ORDER BY n.v ASC LIMIT 1");
    assert_eq!(i64_de(&vmin), 3, "apres suppression du min, la reponse est le min SUIVANT (3), jamais 1 disparu");

    let mn = scalaire(&db, "MATCH (n) RETURN min(n.v)");
    assert_eq!(i64_de(&mn), 3, "min() apres suppression du min");
}

// ─────────────────────────────────────────────────────────────────────
// 4. INSTANT APRES FLUSH — apres l'ecriture, un checkpoint rebatit l'index
//    (hook J6). La route DOIT se re-engager, 0-slot, resultat CORRECT
//    incluant l'ecriture.
// ─────────────────────────────────────────────────────────────────────
#[test]
fn instant_apres_flush() {
    let (db, _td) = db_valeurs(&[5, 3, 9, 1, 7]);
    batir_index_v(&db);

    db.session().execute("INSERT (:N {v: 0})").unwrap();
    // Le checkpoint declenche fusionner_index_ord_valeur (hook J6) : ancres
    // fraiches, index a jour incluant v=0.
    db.checkpoint().expect("checkpoint");

    let (vmin, slots) = scalaire_et_slots(&db, "MATCH (n) RETURN n.v ORDER BY n.v ASC LIMIT 1");
    assert_eq!(i64_de(&vmin), 0, "apres flush, ORDER BY voit le nouveau min");
    assert_eq!(slots, 0, "apres flush, la route ordonnee est RE-ENGAGEE (0-slot)");

    let (mn, slots_mn) = scalaire_et_slots(&db, "MATCH (n) RETURN min(n.v)");
    assert_eq!(i64_de(&mn), 0);
    assert_eq!(slots_mn, 0, "apres flush, min() re-engage (0-slot)");
}

// ─────────────────────────────────────────────────────────────────────
// 5. NON-REGRESSION — sur une base sans index bati, tout reste correct
//    (balayage), et le tri complet est respecte.
// ─────────────────────────────────────────────────────────────────────
#[test]
fn non_regression_sans_index() {
    let (db, _td) = db_valeurs(&[5, 3, 9, 1, 7]);
    // PAS de batir_index_v : aucun index ordonne-valeur.
    assert_eq!(i64_de(&scalaire(&db, "MATCH (n) RETURN n.v ORDER BY n.v ASC LIMIT 1")), 1);
    assert_eq!(i64_de(&scalaire(&db, "MATCH (n) RETURN n.v ORDER BY n.v DESC LIMIT 1")), 9);
    assert_eq!(i64_de(&scalaire(&db, "MATCH (n) RETURN min(n.v)")), 1);
    assert_eq!(i64_de(&scalaire(&db, "MATCH (n) RETURN max(n.v)")), 9);

    let page = db.session().execute("MATCH (n) RETURN n.v ORDER BY n.v ASC LIMIT 3").unwrap();
    let vals: Vec<i64> = page.rows.iter().map(|r| i64_de(&r[0])).collect();
    assert_eq!(vals, vec![1, 3, 5], "top-3 ASC");
}
