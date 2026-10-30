//! Concurrence et cout des registres d'index du substrat.
//!
//! Ces tests ne verifient PAS que la recherche rend les bons voisins — c'est
//! le travail de `search_operations`, `vector_filtered`, `mmr_search`. Ils
//! verifient une propriete distincte, et plus difficile a rattraper apres
//! coup : QUE RIEN NE SE SERIALISE QUI NE DOIVE L'ETRE.
//!
//! Un moteur peut etre parfaitement correct et rester inutilisable parce qu'un
//! seul verrou global transforme N lecteurs en file d'attente. C'est arrive sur
//! cette base — les zones du substrat se lisaient sous verrou EXCLUSIF, et la
//! contention se voyait a x7 sur toutes les lectures en production. Une fois
//! corrige, rien n'empechait la regression de revenir : aucun test ne disait
//! que la propriete devait tenir.
//!
//! Ces tests le disent.
//!
//! SUR LES SEUILS. Les assertions temporelles sont volontairement LARGES
//! (facteurs 3 a 8, jamais 1.2). Elles ne mesurent pas une performance, elles
//! attrapent un changement de NATURE : « les lectures se serialisent » ou « la
//! creation d'index attend la fin des recherches ». Un seuil serre serait
//! instable sous charge et finirait desactive, ce qui reviendrait a ne rien
//! tester du tout.

// (2026-08-30, vague 4a) GATING DE COMPILATION : chaque test du fichier
// appelle vector_search / create_vector_index, eux-memes derriere
// `vector-index`. Sans la meme garde, `cargo test` en features PAR DEFAUT
// ne compilait plus aucun test d'integration du crate (11 x E0599, casse
// arrivee avec le catalogue d'index durable). Aucune assertion touchee.
#![cfg(feature = "vector-index")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use web_time::Instant;

/// La porte de performance est-elle ARMÉE ?
///
/// Même contrat que `obrain-cognitive/tests` et `multi_writer_invariants` :
/// déarmée (défaut), les mesures se font et se DISENT ; armée
/// (`OBRAIN_PERF_GATE=1`), les gardes dont la marge est plus petite que le bruit
/// d'une machine chargée sont exigées.
#[must_use]
pub fn porte_armee() -> bool {
    std::env::var("OBRAIN_PERF_GATE").is_ok_and(|v| v.trim() == "1")
}

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// Largeur canonique du cascade. Les vecteurs plus courts y sont plonges sans
/// perte, mais ici on travaille a pleine largeur : c'est le cas reel, et c'est
/// le seul qui coute assez pour que les mesures veuillent dire quelque chose.
const DIM: usize = 384;

/// Vecteur deterministe et non degenere, deduit de la graine.
fn vecteur(graine: u64) -> Value {
    let mut v = Vec::with_capacity(DIM);
    let mut x = graine.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
    for _ in 0..DIM {
        // xorshift : rapide, reproductible, sans dependance.
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.push((x >> 40) as f32 / 16_777_216.0 - 0.5);
    }
    Value::Vector(v.into())
}

fn requete(graine: u64) -> Vec<f32> {
    match vecteur(graine) {
        Value::Vector(v) => v.to_vec(),
        _ => unreachable!(),
    }
}

/// Base peuplee de `n` noeuds portant un embedding, avec l'index construit.
fn base_indexee(label: &str, n: u64) -> Arc<ObrainDB> {
    let db = Arc::new(ObrainDB::new_in_memory());
    for i in 0..n {
        let id = db.create_node(&[label]);
        db.set_node_property(id, "emb", vecteur(i));
    }
    db.create_vector_index(label, "emb", Some(DIM), Some("cosine"), None, None)
        .expect("construction de l'index");
    db
}

/// Duree MINIMALE observee sur `tours` executions de `f`.
///
/// POURQUOI LE MINIMUM ET NON LA MOYENNE. Sur une machine partagee — celle-ci
/// heberge un agent qui tourne en permanence — le bruit d'ordonnancement
/// n'AJOUTE que du temps : il ne peut pas rendre une operation plus rapide que
/// son cout reel. La moyenne melange donc le cout et l'interference, tandis que
/// le minimum converge vers le cout seul.
///
/// Premiere version de ces tests : mesure unique. Le meme scenario donnait 59,
/// 66 puis 113 ms — les seuils devenaient une loterie, et un test qui echoue au
/// hasard finit desactive, ce qui revient a ne rien tester.
///
/// Les appelants ALTERNENT les configurations d'un tour a l'autre, pour que
/// chacune traverse les memes phases de charge.
fn duree_min(tours: usize, mut f: impl FnMut()) -> Duration {
    let mut meilleur = Duration::MAX;
    for _ in 0..tours {
        let debut = Instant::now();
        f();
        meilleur = meilleur.min(debut.elapsed());
    }
    meilleur
}

const TOURS: usize = 5;

// ─────────────────────────────────────────────────────────────────────────
// 1. Les lectures ne se serialisent pas entre elles.
// ─────────────────────────────────────────────────────────────────────────

/// N fils qui cherchent en meme temps doivent finir en un temps proche de
/// celui d'UN fil — pas N fois plus long.
///
/// Si le registre etait un `Mutex`, ou si la recherche se faisait SOUS le
/// verrou de lecture du registre, ce test s'effondrerait : le temps total
/// croitrait lineairement avec le nombre de fils.
#[test]
fn lectures_concurrentes_ne_se_serialisent_pas() {
    const N_NOEUDS: u64 = 4_000;
    const RECHERCHES: usize = 200;
    let db = base_indexee("Doc", N_NOEUDS);

    // Les deux configurations sont mesurees en ALTERNANCE, `TOURS` fois, et
    // on garde le minimum de chacune (voir `duree_min`).
    let mut solo = Duration::MAX;
    let mut parallele = Duration::MAX;
    let fils = 8usize;

    for _ in 0..TOURS {
        // Reference : un seul fil.
        solo = solo.min(duree_min(1, || {
            for i in 0..RECHERCHES {
                db.vector_search("Doc", "emb", &requete(i as u64), 10, None, None)
                    .expect("recherche");
            }
        }));

        // CHAQUE fil refait le travail COMPLET — pas une part de celui-ci.
        //
        // Ce detail est tout le test. Si on repartissait `RECHERCHES` entre
        // les fils, la serialisation totale donnerait `parallele == solo`, et
        // une assertion « parallele < solo * 3 » passerait au vert EN CAS DE
        // PANNE : elle mesurerait le travail, pas le parallelisme.
        //
        // En donnant la charge entiere a chacun :
        //   parallelisme parfait   parallele ~= solo
        //   serialisation totale   parallele ~= solo * 8
        parallele = parallele.min(duree_min(1, || {
            std::thread::scope(|s| {
                for f in 0..fils {
                    let db = Arc::clone(&db);
                    s.spawn(move || {
                        for i in 0..RECHERCHES {
                            let graine = (f * RECHERCHES + i) as u64;
                            db.vector_search("Doc", "emb", &requete(graine), 10, None, None)
                                .expect("recherche concurrente");
                        }
                    });
                }
            });
        }));
    }
    eprintln!("[serialisation] solo={solo:?} parallele_8_fils={parallele:?}");

    // Seuil a 4x : la moitie du cout d'une serialisation complete sur 8 fils.
    // Large, pour absorber le bruit d'ordonnancement et une machine occupee —
    // on attrape un changement de nature, pas une milliseconde.
    assert!(
        parallele < solo * 4,
        "les lectures se serialisent : 8 fils faisant CHACUN le travail de \
         reference ont pris {parallele:?}, contre {solo:?} pour un seul fil — \
         soit un facteur proche des 8 attendus d'une file d'attente. Un \
         registre sous Mutex, ou une recherche faite SOUS le verrou du \
         registre, donnent exactement ce profil."
    );
}

// ─────────────────────────────────────────────────────────────────────────
// 2. Creer un index ne gele pas les recherches en cours sur un autre index.
// ─────────────────────────────────────────────────────────────────────────

/// C'est LA propriete qui justifie de cloner l'`Arc` sous verrou partage puis
/// de relacher avant de chercher.
///
/// `create_vector_index` prend le verrou en ECRITURE. Si les recherches
/// tenaient le verrou en lecture pendant toute leur duree, la creation
/// attendrait qu'elles cessent toutes — et, `parking_lot` etant equitable, les
/// recherches suivantes attendraient a leur tour derriere la creation. Une
/// seule creation d'index suffirait a faire un point d'arret global.
#[test]
fn creer_un_index_ne_bloque_pas_les_recherches_en_cours() {
    let db = base_indexee("Doc", 3_000);

    // Un second label, non indexe, pret a l'etre.
    for i in 0..3_000u64 {
        let id = db.create_node(&["Autre"]);
        db.set_node_property(id, "emb", vecteur(100_000 + i));
    }

    let stop = Arc::new(AtomicBool::new(false));
    let faites = Arc::new(AtomicUsize::new(0));

    std::thread::scope(|s| {
        // Quatre fils qui cherchent sans relache sur "Doc".
        for f in 0..4 {
            let db = Arc::clone(&db);
            let stop = Arc::clone(&stop);
            let faites = Arc::clone(&faites);
            s.spawn(move || {
                let mut i = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    db.vector_search("Doc", "emb", &requete(f * 1000 + i), 10, None, None)
                        .expect("recherche pendant la creation");
                    faites.fetch_add(1, Ordering::Relaxed);
                    i += 1;
                }
            });
        }

        // Laisse les lecteurs s'installer.
        std::thread::sleep(Duration::from_millis(50));
        let avant = faites.load(Ordering::Relaxed);

        // Pendant ce temps : creation d'un index sur l'AUTRE label.
        let debut = Instant::now();
        db.create_vector_index("Autre", "emb", Some(DIM), Some("cosine"), None, None)
            .expect("creation concurrente");
        let duree_creation = debut.elapsed();

        let apres = faites.load(Ordering::Relaxed);
        stop.store(true, Ordering::Relaxed);

        assert!(
            apres > avant,
            "aucune recherche n'a abouti pendant la creation d'index : les \
             lecteurs etaient bloques ({avant} avant, {apres} apres)"
        );
        assert!(
            duree_creation < Duration::from_secs(10),
            "la creation d'index a mis {duree_creation:?} alors que des \
             recherches tournaient — signe qu'elle attendait leur fin"
        );
    });
}

// ─────────────────────────────────────────────────────────────────────────
// 3. Ecritures et lectures simultanees : ni panique, ni interblocage.
// ─────────────────────────────────────────────────────────────────────────

/// L'entretien d'index a l'ecriture prend le verrou de l'index lui-meme, apres
/// avoir relache celui du registre. Deux verrous imbriques, c'est deux
/// occasions d'interblocage — ce test les cherche.
///
/// Il verifie aussi que le compte final est JUSTE : une insertion concurrente
/// perdue ne se verrait pas autrement que par un voisin manquant, des mois plus
/// tard, dans un resultat de recherche.
#[test]
fn ecritures_et_lectures_simultanees_sans_perte() {
    let db = base_indexee("Doc", 500);
    let stop = Arc::new(AtomicBool::new(false));
    const NOUVEAUX_PAR_FIL: u64 = 250;
    const FILS_ECRIVAINS: u64 = 4;

    std::thread::scope(|s| {
        // Lecteurs.
        for f in 0..4u64 {
            let db = Arc::clone(&db);
            let stop = Arc::clone(&stop);
            s.spawn(move || {
                let mut i = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    db.vector_search("Doc", "emb", &requete(f * 77 + i), 5, None, None)
                        .expect("recherche pendant ecriture");
                    i += 1;
                }
            });
        }

        // Ecrivains.
        let mut ecrivains = Vec::new();
        for f in 0..FILS_ECRIVAINS {
            let db = Arc::clone(&db);
            ecrivains.push(s.spawn(move || {
                for i in 0..NOUVEAUX_PAR_FIL {
                    let id = db.create_node(&["Doc"]);
                    db.set_node_property(id, "emb", vecteur(1_000_000 + f * 10_000 + i));
                }
            }));
        }
        for e in ecrivains {
            e.join().expect("un fil ecrivain a panique");
        }
        stop.store(true, Ordering::Relaxed);
    });

    // Tous les noeuds ecrits doivent etre trouvables. On demande k tres large
    // et on compte : si l'entretien concurrent avait perdu des insertions,
    // l'index en contiendrait moins que la base.
    let attendus = 500 + (FILS_ECRIVAINS * NOUVEAUX_PAR_FIL) as usize;
    let trouves = db
        .vector_search("Doc", "emb", &requete(1), attendus * 2, None, None)
        .expect("recherche finale")
        .len();
    assert_eq!(
        trouves, attendus,
        "l'index a perdu des insertions faites en concurrence : {trouves} \
         entrees pour {attendus} noeuds ecrits"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// 4. Deux index differents sont independants.
// ─────────────────────────────────────────────────────────────────────────

/// Rien ne doit lier le travail sur `A:emb` a celui sur `B:emb`. Un verrou
/// unique couvrant tout le registre serait fonctionnellement correct et
/// couterait exactement cette independance — le genre de regression qui ne se
/// voit que sous charge, en production.
#[test]
fn deux_index_ne_se_genent_pas() {
    let db = Arc::new(ObrainDB::new_in_memory());
    for i in 0..2_000u64 {
        let a = db.create_node(&["A"]);
        db.set_node_property(a, "emb", vecteur(i));
        let b = db.create_node(&["B"]);
        db.set_node_property(b, "emb", vecteur(500_000 + i));
    }
    db.create_vector_index("A", "emb", Some(DIM), Some("cosine"), None, None)
        .expect("index A");
    db.create_vector_index("B", "emb", Some(DIM), Some("cosine"), None, None)
        .expect("index B");

    // TROIS configurations, mesurees en ALTERNANCE et retenues au minimum :
    //
    //   sequentiel   200 recherches, un fil, les deux index l'un apres l'autre
    //   parallele    les memes 200, deux fils, UN INDEX CHACUN
    //   temoin       les memes 200, deux fils, LE MEME INDEX pour les deux
    //
    // Le temoin est la piece maitresse. Sans lui on ne peut pas distinguer
    // « les deux index se genent » de « la machine ne donne pas deux coeurs ».
    let mut sequentiel = Duration::MAX;
    let mut parallele = Duration::MAX;
    let mut temoin = Duration::MAX;

    for _ in 0..TOURS {
        sequentiel = sequentiel.min(duree_min(1, || {
            for i in 0..100u64 {
                db.vector_search("A", "emb", &requete(i), 10, None, None)
                    .expect("A");
            }
            for i in 0..100u64 {
                db.vector_search("B", "emb", &requete(i), 10, None, None)
                    .expect("B");
            }
        }));

        parallele = parallele.min(duree_min(1, || {
            std::thread::scope(|s| {
                for label in ["A", "B"] {
                    let db = Arc::clone(&db);
                    s.spawn(move || {
                        for i in 0..100u64 {
                            db.vector_search(label, "emb", &requete(i), 10, None, None)
                                .expect("recherche independante");
                        }
                    });
                }
            });
        }));

        temoin = temoin.min(duree_min(1, || {
            std::thread::scope(|s| {
                for f in 0..2u64 {
                    let db = Arc::clone(&db);
                    s.spawn(move || {
                        for i in 0..100u64 {
                            db.vector_search("A", "emb", &requete(f * 1000 + i), 10, None, None)
                                .expect("temoin");
                        }
                    });
                }
            });
        }));
    }
    eprintln!(
        "[independance] sequentiel={sequentiel:?} deux_index={parallele:?} un_index={temoin:?}"
    );

    // ── POURQUOI ON COMPARE AU TEMOIN ET NON A LA THEORIE ────────────────
    //
    // La premiere version de ce test exigeait `parallele < sequentiel * 0.8`,
    // au motif que deux index independants devraient donner la moitie du temps
    // sequentiel. ELLE A ECHOUE — et l'accusation etait FAUSSE.
    //
    // Mesure : sequentiel 67 ms, parallele a deux index 46 ms, temoin a UN
    // SEUL index 45 ms. Les deux cas paralleles sont indiscernables. Le
    // plafond a x1,45 au lieu de x2 ne vient donc pas d'un verrou partage
    // entre les index : il vient de la machine (20 coeurs, moitie occupes, ce
    // processus a nice 19) et des couts fixes.
    //
    // Un seuil absolu aurait donc accuse le code d'un defaut appartenant a
    // l'environnement — et, pire, il aurait ete instable : vert sur une
    // machine au repos, rouge en CI chargee, donc desactive au bout de trois
    // semaines.
    //
    // La comparaison au temoin, elle, ANNULE l'environnement : les deux
    // mesures le subissent identiquement. Ce qui reste est exactement la
    // question posee — deux index se genent-ils PLUS que deux fils sur un
    // seul ?
    // CE QUE CE TEST NE VOIT PAS, ET QUI COUVRE LE TROU. Un verrou GLOBAL sur
    // tout le registre ralentirait le temoin AUTANT que le parallele — le
    // rapport resterait proche de 1 et ce test passerait. Il ne detecte donc
    // qu'un verrou par ENTREE mal partage.
    //
    // Le cas global est couvert par `lectures_concurrentes_ne_se_serialisent_pas`,
    // ou 8 fils frappent un seul index : la, un verrou global se paie x8. Les
    // deux tests ne se recouvrent pas, ils se completent — il faut les deux.
    assert!(
        parallele < temoin * 2,
        "les deux index se genent mutuellement : 2 fils sur DEUX index ont \
         pris {parallele:?}, contre {temoin:?} pour 2 fils sur UN SEUL index \
         faisant le meme travail. L'ecart ne s'explique pas par la machine, \
         qui pese identiquement sur les deux mesures — il vient d'un verrou \
         partage par les entrees du registre."
    );

    // ── GARDE-FOU DE BRUIT : OPT-IN, PARCE QUE SA MARGE EST PLUS PETITE QUE
    //    LE BRUIT DE LA MACHINE (2026-09-17)
    //
    // Cette inegalite (« deux fils font mieux qu'un ») a une marge MESURÉE de
    // 1,5 % : sequentiel=241,79 ms contre parallele=245,32 ms sur une machine
    // chargée — où un processus étranger à ce dépôt consommait 83 % d'un cœur.
    // Un seuil dont la marge est plus petite que la charge de la machine ne
    // prouve rien : il est vert au repos, rouge en CI chargée, et désactivé au
    // bout de trois semaines (le raisonnement est celui du commentaire plus
    // haut, appliqué à la garde la plus fragile).
    //
    // L'assertion STRUCTURELLE, elle, reste TOUJOURS active : `parallele <
    // temoin * 2` a une marge de 100 %, et c'est elle qui attrape le défaut
    // visé (un verrou partagé par les entrées du registre). La mesure, elle,
    // est imprimée dans les deux cas.
    if porte_armee() {
        assert!(
            parallele < sequentiel,
            "deux fils ne vont pas plus vite qu'un seul ({parallele:?} contre \
             {sequentiel:?}) : le recouvrement a disparu"
        );
    } else {
        eprintln!(
            "  ⚠ recouvrement non exigé (OBRAIN_PERF_GATE=1 pour l'exiger) — \
             sequentiel={sequentiel:?} parallele={parallele:?}"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────
// 5. Les suppressions concurrentes ne laissent pas de fantomes.
// ─────────────────────────────────────────────────────────────────────────

/// Un noeud supprime ne doit plus JAMAIS ressortir. Le contraire — un
/// « fantome » — est le pire resultat possible : la recherche reussit et
/// designe un noeud qui n'existe plus, ce qui se lit comme une donnee valide.
#[test]
fn suppressions_concurrentes_sans_fantome() {
    let db = base_indexee("Doc", 1_200);
    let ids: Vec<_> = db.graph_store().nodes_by_label("Doc");
    assert_eq!(ids.len(), 1_200);

    let a_supprimer: Vec<_> = ids.iter().copied().take(600).collect();
    let stop = Arc::new(AtomicBool::new(false));

    std::thread::scope(|s| {
        for f in 0..3u64 {
            let db = Arc::clone(&db);
            let stop = Arc::clone(&stop);
            s.spawn(move || {
                let mut i = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    db.vector_search("Doc", "emb", &requete(f * 31 + i), 20, None, None)
                        .expect("recherche pendant suppression");
                    i += 1;
                }
            });
        }

        let mut suppresseurs = Vec::new();
        for tranche in a_supprimer.chunks(150) {
            let db = Arc::clone(&db);
            let tranche = tranche.to_vec();
            suppresseurs.push(s.spawn(move || {
                for id in tranche {
                    db.delete_node(id);
                }
            }));
        }
        for x in suppresseurs {
            x.join().expect("un fil suppresseur a panique");
        }
        stop.store(true, Ordering::Relaxed);
    });

    let restants: std::collections::HashSet<_> =
        db.graph_store().nodes_by_label("Doc").into_iter().collect();
    let resultats = db
        .vector_search("Doc", "emb", &requete(7), 2_000, None, None)
        .expect("recherche finale");

    for (id, _) in &resultats {
        assert!(
            restants.contains(id),
            "FANTOME : le noeud {id:?} a ete supprime mais ressort encore de \
             l'index. Une recherche qui designe un noeud disparu est pire \
             qu'une recherche qui echoue — elle a l'air juste."
        );
    }
    assert_eq!(
        resultats.len(),
        600,
        "l'index devrait contenir exactement les 600 noeuds survivants"
    );
}
