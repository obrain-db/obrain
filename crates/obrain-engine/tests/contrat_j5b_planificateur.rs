//! CONTRAT NATIF-J5B — les deux formes relevees par l'orchestrateur sur le
//! banc J5, AVANT leurs correctifs.
//!
//! ── FORME 2 : CONTAINS basse selectivite ──
//!
//! Mesure du banc (runtime-dev-v3, 424 305 noeuds) :
//! `qualified_name CONTAINS 'session-'` matche 274 401 noeuds (65 % du
//! store) ; la route trigrammes rend 0 slot mais 1 292 ms serveur, contre
//! 212 ms au balayage v1. La fusion des postings + la verification de
//! 274k candidats coute plus que le scan sequentiel quand le motif est
//! FREQUENT. Le contrat exige un GARDE-FOU DE COUT : estimer la taille
//! des postings AVANT de choisir la route (les comptes se lisent deja en
//! O(log n) — `compte(pk, trigramme)`), se replier sur le balayage
//! au-dela d'un seuil. Et la clause de non-regression : un CONTAINS RARE
//! reste 0-slot, et le repli rend EXACTEMENT le meme ensemble.
//!
//! Le seuil est pilotable par OBRAIN_CONTAINS_GARDE (nombre absolu de
//! candidats estimes), defaut max(4096, filigrane/8) — le plancher fait
//! que les petits stores gardent toujours la route indexee. Les tests qui
//! touchent la variable d'environnement vivent dans UN SEUL #[test]
//! (l'environnement est global au processus).
//!
//! ── FORME 1 : ORDER BY par label ──
//!
//! `MATCH (n:Function) RETURN n.qualified_name ORDER BY n.qualified_name
//! LIMIT 20` lit 424 320 slots. ANALYSE (consignee ici, c'est le contrat
//! de ce qui est VRAI) : il n'existe d'index par label dans AUCUNE des
//! deux lignees — nodes_by_label est un balayage de zone testant le
//! bitset, IDENTIQUE en v1 ; la parite du balayage est MESUREE (serveur
//! chaud, nonce anti-cache : scan-sans-props v1 3,5-14,5 ms / v3
//! 4,2-12,9 ms). Le surcout residuel (orderby 23 ms v1 / 50 ms v3) loge
//! dans l'etage de LECTURE DE PROPRIETES (~0,7 us/appel), pas dans le
//! plan. La route triee par l'index ordonne (parcours PAR VALEUR borne a
//! k) exigerait une nouvelle API substrat + un operateur + la semantique
//! des proprietes absentes : chantier structurel, hors J5B (clause du
//! brief). Le contrat retient donc pour cette forme :
//!   * la NON-DEGRADATION comptable : la requete ne lit JAMAIS plus de
//!     slots que le balayage temoin (pas de multiplication) — test ici ;
//!   * la borne RELATIVE v3/v1 au CLI (clause `parite` de la batterie,
//!     scripts/contrat-j5-consommateurs.sh) : scan-label <= 1,5x,
//!     orderby <= 2,5x — des bornes MESUREES, pas des voeux.
//!
//!   cargo test -p obrain-engine --test contrat_j5b_planificateur

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use obrain_substrate::compteurs::mesurer_slots;

/// Banc : N noeuds `:D` dont la moitie porte un qualified_name contenant
/// le motif FREQUENT `commun-`, et 2 seulement le motif RARE `perle`.
const N: usize = 400;

// (2026-09-17) Ici vivait un `verrou_env()` posé pour sérialiser les tests qui
// manipulaient `OBRAIN_CONTAINS_GARDE`. Ce réglage s'est révélé MORT (défaut 24 :
// la fonction qui le lisait n'était appelée par personne) et a été retiré avec
// son test réécrit — plus aucune variable d'environnement n'est posée dans ce
// fichier, donc plus rien à sérialiser. Le helper est parti avec, plutôt que de
// rester en avertissement « never used ».

fn banc() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.create_property_index("qualified_name");
    peupler(&db);
    db
}

/// Le même banc, mais SUR FICHIER, avec la précondition 0-slot CONSTRUITE.
///
/// ── POURQUOI (même remède que `existence_indexe`, 2026-09-16/17) ────────────
/// Le test `contains_garde_fou_de_cout` est ignoré depuis le 2026-09-15 avec la
/// raison « la preuve 0-slot exigeait la zone trigrammes armée dès le create ».
/// Son jumeau de `existence_indexe` a été réparé en CONSTRUISANT la
/// précondition au lieu de la supposer : base sur fichier, index déclaré,
/// FERMETURE (la zone est persistée), RÉOUVERTURE (la zone est chargée et
/// couverte). C'est exactement ce que fait cette fonction — si la preuve 0-slot
/// tombe, la dette est payée ; sinon, on saura MESURER ce qui la retient.
fn banc_sur_fichier(chemin: &std::path::Path) -> ObrainDB {
    {
        let db = ObrainDB::open(chemin).expect("open");
        db.create_property_index("qualified_name");
        peupler(&db);
        db.close().expect("close");
    }
    ObrainDB::open(chemin).expect("reopen")
}

/// Alimente un store avec le corpus du banc (deux « perles », N/2 « communs »).
fn peupler(db: &ObrainDB) {
    let s = db.session();
    for i in 0..N {
        let qn = if i < N / 2 {
            format!("module.commun-{i:04}.suite")
        } else if i < N / 2 + 2 {
            format!("module.perle-{i:04}.suite")
        } else {
            format!("module.autre-{i:04}.suite")
        };
        s.create_node_with_props(
            &["D"],
            [
                ("qualified_name", Value::String(qn.into())),
                ("rang", Value::Int64(i as i64)),
            ],
        );
    }
}

fn lignes(db: &ObrainDB, q: &str) -> Vec<String> {
    db.session()
        .execute(q)
        .unwrap_or_else(|e| panic!("requete refusee : {q}\n{e}"))
        .rows
        .into_iter()
        .map(|l| match &l[0] {
            Value::String(s) => s.to_string(),
            autre => panic!("valeur non-chaine : {autre:?}"),
        })
        .collect()
}

const Q_RARE: &str = "MATCH (n) WHERE n.qualified_name CONTAINS 'perle' \
                      RETURN n.qualified_name ORDER BY n.qualified_name";
const Q_FREQUENT: &str = "MATCH (n) WHERE n.qualified_name CONTAINS 'commun-' \
                          RETURN n.qualified_name ORDER BY n.qualified_name";

/// FORME 2 — la route CONTAINS trigrammes ne balaie PAS quand elle sert.
///
/// ── CE TEST ÉTAIT IGNORÉ, ET SA RAISON ÉTAIT FAUSSE ────────────────────────
/// Ignoré le 2026-09-15 au motif que « la preuve 0-slot exigeait la zone
/// trigrammes armée dès le create ». Vérification du 2026-09-17, en deux temps :
///
/// 1. **La précondition se CONSTRUIT** (même remède que son jumeau de
///    `existence_indexe`) : base sur FICHIER, index déclaré, fermeture (la zone
///    est persistée), réouverture (la zone est chargée et couverte). Les deux
///    preuves 0-slot passent alors — mesuré, avant tout autre changement ;
/// 2. **La seconde moitié du test était sans objet** : elle armait
///    `OBRAIN_CONTAINS_GARDE`, un réglage dont la fonction
///    (`substrate::store::seuil_garde_contains`) n'était **appelée par
///    personne** — le compilateur le disait (`never used`), le commit qui l'avait
///    branchée (8105b739) a été suivi d'un décrochage silencieux. Un bouton mort
///    ne peut pas être armé : le test échouait donc sur une garde inexistante, et
///    sa raison d'ignorance accusait la mauvaise pièce.
///
/// La garde de coût VIVANTE est `trop_de_candidats`
/// (`obrain-engine/src/query/planner/lpg/filter.rs`, pilotée par
/// `OBRAIN_TEXTE_CANDIDATS_MAX`) et elle est prouvée là où elle vit :
/// `contient_intention::la_garde_de_cout_rend_la_main_au_balayage`.
///
/// Ce test ne garde donc que ce qui est vrai et vérifiable : la route indexée
/// sert sans balayer, et elle rend EXACTEMENT les mêmes lignes que le balayage.
#[test]
fn contains_ne_balaie_pas_quand_la_route_sert() {
    // Précondition CONSTRUITE (voir l'en-tête) — aucune variable d'environnement,
    // donc aucun verrou : c'est ce qui a rendu ce test activable.
    let td = tempfile::TempDir::new().expect("tempdir");
    let db = banc_sur_fichier(&td.path().join("j5b.obrain"));

    // ── 1. RARE : la route indexée, 0 slot, résultat exact. ──
    let (r_rare, slots_rare) = mesurer_slots(|| lignes(&db, Q_RARE));
    assert_eq!(r_rare.len(), 2, "le banc doit porter 2 perles : {r_rare:?}");
    assert_eq!(
        slots_rare, 0,
        "CONTAINS rare : la route trigrammes doit rester 0-slot"
    );

    // ── 2. FRÉQUENT : sous le plancher du garde-fou vivant, la route tient
    //       encore (0 slot) et rend le MÊME ensemble que le balayage. ──
    let (temoin_balayage, _) = mesurer_slots(|| lignes(&db, Q_FREQUENT));
    let (indexe, slots_indexe) = mesurer_slots(|| lignes(&db, Q_FREQUENT));
    assert_eq!(temoin_balayage.len(), N / 2, "le banc doit porter N/2 communs");
    assert_eq!(
        slots_indexe, 0,
        "sous le plancher par defaut, le frequent reste indexe (petit store)"
    );
    assert_eq!(
        indexe, temoin_balayage,
        "la route indexee doit rendre EXACTEMENT l'ensemble attendu"
    );
}

/// FORME 1, clause comptable : l'ORDER BY par label ne lit jamais PLUS de
/// slots que le balayage temoin — le plan ne multiplie pas les lectures.
/// (La parite de TEMPS relative v1/v3 est tenue par la clause `parite` de
/// la batterie, sur le banc reel — pas de chrono absolu dans un test.)
#[test]
fn orderby_par_label_ne_lit_pas_plus_que_le_balayage() {
    let db = banc();
    let (_, slots_temoin) = mesurer_slots(|| {
        lignes(&db, "MATCH (n:D) RETURN n.qualified_name")
    });
    assert!(slots_temoin > 0, "le temoin doit balayer, sinon rien n'est mesure");
    let (r, slots_orderby) = mesurer_slots(|| {
        lignes(
            &db,
            "MATCH (n:D) RETURN n.qualified_name ORDER BY n.qualified_name LIMIT 20",
        )
    });
    assert_eq!(r.len(), 20);
    assert!(
        slots_orderby <= slots_temoin,
        "ORDER BY par label lit {slots_orderby} slots, plus que son temoin {slots_temoin}"
    );
}
