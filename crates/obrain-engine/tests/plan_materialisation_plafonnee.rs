//! LE PLAFOND DE MATÉRIALISATION DU PLANIFICATEUR — contrat.
//!
//! ── CE QUI EST MESURÉ (2026-09-14, `banc_plan_froid`) ──────────────────
//!
//! La ventilation `OBRAIN_PHASES` d'une copie de `runtime-dev` (432 910
//! nœuds, label `Message` = 71 445 nœuds) sépare la planification de
//! l'exécution. Textes UNIQUES, chaque texte joué deux fois :
//!
//! ```text
//!   MATCH (n:Message) WHERE n.session = '…' RETURN count(n)
//!       1er appel : planification 192 859 µs   exécution      6 µs
//!       2e  appel : planification 192 306 µs   exécution      4 µs
//!
//!   MATCH (n:Message) WHERE n.seq >= 1 RETURN count(n)
//!       1er appel : planification 339 098 µs   exécution    898 µs
//!       2e  appel : planification 316 725 µs   exécution    900 µs
//! ```
//!
//! Deux faits, et le second est le sujet de ce test :
//!
//! 1. la planification PHYSIQUE est refaite à chaque appel — le 2e appel
//!    coûte exactement ce que coûte le 1er, la mémoïsation n'existe pas ;
//! 2. ce qui coûte n'est pas une DÉCISION mais une LECTURE : sans index,
//!    l'heuristique balayait le label entier et HYDRATAIT chaque nœud
//!    (`get_node_at_epoch`) pour tester la propriété. 71 445 × ~2,7 µs =
//!    192 ms, à chaque appel, pour un plan qui rend 1 ligne.
//!
//! Le plafond (`OBRAIN_PLAN_MATERIALISATION_MAX`, défaut 1024) rend la main
//! au chemin GÉNÉRIQUE au-delà : balayage + filtre À L'EXÉCUTION, ~0,2 µs
//! par nœud au lieu de ~2,7 µs. Le contrat de ce fichier est la clause de
//! NON-RÉGRESSION SÉMANTIQUE de ce repli : les lignes rendues au-dessus du
//! plafond sont celles du chemin d'origine, y compris sur les cas tordus
//! (valeur absente, propriété absente, comparaison, projection, LIMIT).
//!
//! Les deux côtés du plafond sont couverts dans le MÊME test : un label de
//! 3 000 nœuds (repli) et un label de 8 nœuds (matérialisation conservée).
//!
//!   cargo test --release -p obrain-engine --test plan_materialisation_plafonnee

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use std::sync::Arc;

/// Au-dessus du plafond par défaut (1024) : force le repli.
const N_GRAND: usize = 3000;
/// En dessous : la matérialisation reste permise.
const N_PETIT: usize = 8;

fn banc() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for i in 0..N_GRAND {
        // `k` prend trois valeurs ; `rang` sert aux comparaisons.
        s.create_node_with_props(
            &["Grand"],
            [
                ("k", Value::String(format!("v{}", i % 3).into())),
                ("rang", Value::Int64(i as i64)),
            ],
        );
    }
    for i in 0..N_PETIT {
        // Propriétés DISTINCTES (`kp`, `rangp`) : le témoin sans label des
        // assertions d'équivalence ci-dessous doit porter EXACTEMENT le jeu
        // du label `Grand`, sans les nœuds du petit label.
        s.create_node_with_props(
            &["Petit"],
            [
                ("kp", Value::String(format!("v{}", i % 3).into())),
                ("rangp", Value::Int64(i as i64)),
            ],
        );
    }
    db
}

fn compte(db: &Arc<ObrainDB>, q: &str) -> i64 {
    let r = db
        .session()
        .execute(q)
        .unwrap_or_else(|e| panic!("requete refusee : {q}\n{e}"));
    match r.rows.first().and_then(|l| l.first()) {
        Some(Value::Int64(n)) => *n,
        autre => panic!("count attendu, recu {autre:?} pour {q}"),
    }
}

fn valeurs(db: &Arc<ObrainDB>, q: &str) -> Vec<i64> {
    db.session()
        .execute(q)
        .unwrap_or_else(|e| panic!("requete refusee : {q}\n{e}"))
        .rows
        .into_iter()
        .map(|l| match &l[0] {
            Value::Int64(n) => *n,
            autre => panic!("entier attendu, recu {autre:?} pour {q}"),
        })
        .collect()
}

#[test]
fn egalite_sur_propriete_non_indexee_rend_les_memes_lignes_des_deux_cotes_du_plafond() {
    let db = Arc::new(banc());

    // ── GRAND label (3 000 > 1 024) : le repli gouverne. ───────────────
    // 1 000 nœuds portent chaque valeur : la borne est exacte, pas
    // approximative — c'est ce que la matérialisation rendait aussi.
    assert_eq!(
        compte(&db, "MATCH (n:Grand) WHERE n.k = 'v0' RETURN count(n)"),
        (N_GRAND / 3) as i64,
        "egalite presente sur un label au-dessus du plafond"
    );
    assert_eq!(
        compte(&db, "MATCH (n:Grand) WHERE n.k = 'absent' RETURN count(n)"),
        0,
        "valeur absente : le repli doit rendre 0, pas une erreur"
    );
    assert_eq!(
        compte(
            &db,
            "MATCH (n:Grand) WHERE n.propriete_absente = 'x' RETURN count(n)"
        ),
        0,
        "propriete absente : idem"
    );
    // Coercition numérique : `rang = 1.0` doit rendre les mêmes lignes que
    // `rang = 1` (le prédicat est évalué par l'évaluateur d'expression).
    assert_eq!(
        compte(&db, "MATCH (n:Grand) WHERE n.rang = 1 RETURN count(n)"),
        compte(&db, "MATCH (n:Grand) WHERE n.rang = 1.0 RETURN count(n)"),
        "coercition entiere/reelle identique des deux cotes du plafond"
    );
    // Comparaison : le chemin générique doit rendre le même compte qu'un
    // comptage indépendant par le prédicat de plage.
    assert_eq!(
        compte(&db, "MATCH (n:Grand) WHERE n.rang < 10 RETURN count(n)"),
        10,
        "comparaison sur label au-dessus du plafond"
    );
    // AND de deux égalités, dont une sur propriété inexistante.
    assert_eq!(
        compte(
            &db,
            "MATCH (n:Grand) WHERE n.k = 'v1' AND n.propriete_absente = 'x' RETURN count(n)"
        ),
        0,
        "conjonction dont un membre est absente"
    );

    // ── PETIT label (8 < 1 024) : la matérialisation est conservée. ────
    assert_eq!(
        compte(&db, "MATCH (n:Petit) WHERE n.kp = 'v0' RETURN count(n)"),
        3,
        "egalite presente sous le plafond"
    );
    assert_eq!(
        compte(&db, "MATCH (n:Petit) WHERE n.kp = 'absent' RETURN count(n)"),
        0,
        "valeur absente sous le plafond"
    );

    // ── LES DEUX ROUTES DOIVENT S'ACCORDER sur les mêmes données. ──────
    // La même sélection exprimée sans label passe par le chemin générique
    // quel que soit le plafond : c'est le témoin.
    for valeur in ["v0", "v1", "v2", "absent"] {
        let avec_label = compte(
            &db,
            &format!("MATCH (n:Grand) WHERE n.k = '{valeur}' RETURN count(n)"),
        );
        let sans_label = compte(
            &db,
            &format!("MATCH (n) WHERE n.k = '{valeur}' RETURN count(n)"),
        );
        assert_eq!(
            avec_label, sans_label,
            "egalite {valeur} : label (repli) contre sans-label (temoin)"
        );
    }
    for borne in [0_i64, 1, 7, 2999, 3000] {
        let avec_label = compte(
            &db,
            &format!("MATCH (n:Grand) WHERE n.rang >= {borne} RETURN count(n)"),
        );
        let sans_label = compte(
            &db,
            &format!("MATCH (n) WHERE n.rang >= {borne} RETURN count(n)"),
        );
        assert_eq!(
            avec_label, sans_label,
            "comparaison >= {borne} : label contre sans-label"
        );
    }

    // ── PROJECTION + LIMIT : le repli alimente bien les opérateurs aval. ─
    let lus = valeurs(
        &db,
        "MATCH (n:Grand) WHERE n.k = 'v2' RETURN n.rang LIMIT 5",
    );
    assert_eq!(lus.len(), 5, "LIMIT 5 sur un label au-dessus du plafond");
    assert!(
        lus.iter().all(|v| v % 3 == 2),
        "les lignes rendues doivent satisfaire le filtre : {lus:?}"
    );
}
