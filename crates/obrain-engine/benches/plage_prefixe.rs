//! `STARTS WITH` — ce que la plage ordonnée change, et ce qu'elle ne change pas.
//!
//! POURQUOI CE BANC EXISTE. `n.prop STARTS WITH 'p'` tombait sur le
//! `FilterOperator` générique : un balayage intégral, quelle que soit la
//! sélectivité du préfixe et quelle que soit la limite au-dessus. Mesuré par
//! l'opérateur sur le store vivant : `LIMIT 30` à **825 ms**, pour trente
//! lignes rendues. La réécriture (`prefix_range`) le traduit en intervalle
//! `[p, haute(p))`, que `find_nodes_in_range` sait servir par l'index ordonné
//! quand la propriété est déclarée.
//!
//! ── CE QUI SE LIT ICI, ET CE QUI NE SE LIT PAS ──
//!
//! **Le résultat est un RAPPORT entre deux bras, jamais une valeur absolue.**
//! `BANCS.md` (obrain-substrate/benches) établit que cette machine a annoncé
//! −39,3 % puis +63,7 % avec p = 0,00 **sur du code identique** dès que la
//! charge monte. Une valeur absolue prise ici ne mesure pas : elle devine. Un
//! rapport entre deux bras du même groupe, exécutés entrelacés dans le même
//! processus sur le même corpus, absorbe la charge — elle les affecte tous
//! deux également.
//!
//! ── LES BRAS ──
//!
//! * `prefixe_declare` / `prefixe_non_declare` — LA MÊME requête sur LE MÊME
//!   corpus ; seule diffère la déclaration d'index (`create_property_index`,
//!   qui déclare l'index ordonné par le même acte). **C'est ce rapport qui
//!   est le résultat de la réécriture B.**
//! * `contains_temoin` — `CONTAINS`, qu'aucune plage ne peut représenter
//!   (un infixe n'est pas un intervalle). Reste sur le balayage générique :
//!   c'est le TÉMOIN NON TRAITÉ, la référence haute. Si `prefixe_non_declare`
//!   ne s'en distingue pas, le balayage est bien le régime commun.
//! * `egalite_declaree` — `=` sur la propriété déclarée, servi par l'index de
//!   hachage. PLANCHER : ce que coûte la requête quand l'index répond en O(1).
//!   `prefixe_declare` ne peut pas descendre en dessous.
//! * `prefixe_tres_selectif` / `prefixe_peu_selectif` — même index, deux
//!   sélectivités. Le coût DOIT suivre la réponse : c'est la thèse du
//!   chantier, et ce rapport-ci est le seul qui la teste directement.
//!
//! ── AVANT DE LIRE UN CHIFFRE ──
//!
//! `uptime`, et rien au-dessus de 5. Puis le contrôle à blanc de `BANCS.md` :
//!
//! ```bash
//! cargo bench -p obrain-engine --bench plage_prefixe -- --save-baseline temoin
//! # sans rien changer :
//! cargo bench -p obrain-engine --bench plage_prefixe -- --baseline temoin
//! ```
//!
//! L'écart attendu est zéro. S'il ne l'est pas, on ne lit AUCUN verdict de la
//! campagne. Une référence ne vaut que dans la session où elle est prise.
//!
//! ```bash
//! cargo bench -p obrain-engine --bench plage_prefixe
//! PLAGE_PREFIXE_BENCH_N=200000 cargo bench -p obrain-engine --bench plage_prefixe
//! ```
//!
//! NON EXÉCUTÉ à l'écriture : charge de l'hôte à 22 sur 16 cœurs, très au-delà
//! du seuil. Aucun chiffre n'est avancé pour la réécriture B.

use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use obrain_engine::ObrainDB;

fn nombre_de_noeuds() -> usize {
    std::env::var("PLAGE_PREFIXE_BENCH_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50_000)
}

/// Le préfixe RARE : une poignée de nœuds sur `n`. C'est la forme du chantier
/// — `LIMIT 30` sur un store de 3,5 millions de nœuds.
const PREFIXE_RARE: &str = "self:rare:";
/// Le préfixe COMMUN : un nœud sur deux. Le balayage y est presque optimal,
/// donc l'index n'y a presque rien à gagner — et c'est exactement ce que le
/// rapport avec le bras rare doit montrer.
const PREFIXE_COMMUN: &str = "self:a";

/// Corpus déterministe, monté UNE fois et HORS de `b.iter` — la faute du
/// 2026-08-21 documentée dans `BANCS.md` était de le reconstruire par
/// itération : criterion l'excluait du chronomètre, mais ni du cache de pages
/// ni de l'ordonnanceur, et le banc annonçait −39,3 % sur du code identique.
///
/// Les valeurs sont volontairement DÉSORDONNÉES par rapport à l'ordre
/// d'insertion (mélange multiplicatif) : un corpus déjà trié flatterait
/// l'index ordonné.
fn monter(n: usize, declarer: bool) -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    for i in 0..n {
        let melange = (i * 7919) % n;
        // Un nœud sur deux sous PREFIXE_COMMUN ; une poignée sous
        // PREFIXE_RARE ; le reste ailleurs, pour que le balayage ait du
        // travail que l'index puisse lui épargner.
        let nom = if melange.is_multiple_of(1000) {
            format!("{PREFIXE_RARE}{melange:08}")
        } else if melange.is_multiple_of(2) {
            format!("{PREFIXE_COMMUN}{melange:08}")
        } else {
            format!("autre:{melange:08}")
        };
        session
            .execute(&format!("INSERT (:Grain {{nom: '{nom}', rang: {melange}}})"))
            .expect("semis");
    }
    if declarer {
        // Déclare l'index d'égalité ET l'index ordonné (un seul acte, voir
        // `SubstrateStore::create_property_index`). C'est ce que fait la
        // production sur `qualified_name` et `name`.
        db.create_property_index("nom");
    }
    db
}

fn compter(db: &ObrainDB, requete: &str) -> usize {
    db.session()
        .execute(requete)
        .unwrap_or_else(|e| panic!("requête refusée : {requete}\n{e}"))
        .rows
        .len()
}

fn bench_plage_prefixe(c: &mut Criterion) {
    let n = nombre_de_noeuds();
    let declare = monter(n, true);
    let non_declare = monter(n, false);

    let q_rare = format!("MATCH (g:Grain) WHERE g.nom STARTS WITH '{PREFIXE_RARE}' RETURN g.nom");
    let q_commun =
        format!("MATCH (g:Grain) WHERE g.nom STARTS WITH '{PREFIXE_COMMUN}' RETURN g.nom");
    let q_contains = format!("MATCH (g:Grain) WHERE g.nom CONTAINS '{PREFIXE_RARE}' RETURN g.nom");

    // L'ÉGALITÉ DES DEUX CORPUS, vérifiée avant de chronométrer quoi que ce
    // soit : deux bras qui ne rendent pas le même nombre de lignes ne se
    // comparent pas. C'est le banc qui porte ici la règle du chantier.
    let lignes_declare = compter(&declare, &q_rare);
    let lignes_non_declare = compter(&non_declare, &q_rare);
    assert_eq!(
        lignes_declare, lignes_non_declare,
        "les deux corpus doivent rendre le MÊME nombre de lignes — sans quoi \
         le rapport entre les deux bras ne mesure rien"
    );
    let q_egalite = {
        let cible = declare
            .session()
            .execute(&format!("{q_rare} LIMIT 1"))
            .expect("cible")
            .rows
            .first()
            .and_then(|l| l.first().cloned())
            .expect("au moins un nœud sous le préfixe rare");
        let cible = match cible {
            obrain_common::types::Value::String(s) => s.to_string(),
            autre => panic!("nom non-chaîne : {autre:?}"),
        };
        format!("MATCH (g:Grain) WHERE g.nom = '{cible}' RETURN g.nom")
    };

    eprintln!(
        "corpus : {n} nœuds ; préfixe rare → {lignes_declare} lignes ; \
         préfixe commun → {} lignes",
        compter(&declare, &q_commun)
    );

    let mut g = c.benchmark_group("plage_prefixe");
    g.throughput(Throughput::Elements(n as u64));

    // ── LE RAPPORT QUI EST LE RÉSULTAT ──
    g.bench_function("prefixe_declare", |b| {
        b.iter(|| black_box(compter(&declare, &q_rare)));
    });
    g.bench_function("prefixe_non_declare", |b| {
        b.iter(|| black_box(compter(&non_declare, &q_rare)));
    });

    // ── LES DEUX BORNES ──
    g.bench_function("contains_temoin", |b| {
        b.iter(|| black_box(compter(&declare, &q_contains)));
    });
    g.bench_function("egalite_declaree", |b| {
        b.iter(|| black_box(compter(&declare, &q_egalite)));
    });

    // ── LE COÛT SUIT-IL LA RÉPONSE ? ──
    g.bench_function("prefixe_tres_selectif", |b| {
        b.iter(|| black_box(compter(&declare, &q_rare)));
    });
    g.bench_function("prefixe_peu_selectif", |b| {
        b.iter(|| black_box(compter(&declare, &q_commun)));
    });

    g.finish();
}

criterion_group!(bancs, bench_plage_prefixe);
criterion_main!(bancs);
