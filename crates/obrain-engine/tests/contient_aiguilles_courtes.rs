//! CONTAINS — LE PREMIER APPEL EN LECTURE SEULE, ET TOUTES LES AIGUILLES TORDUES.
//!
//! Ce fichier existe pour un défaut MESURÉ le 2026-09-16, et il le mesure sans
//! HTTP, sans service et sans navigateur : le même corpus, la même requête, sur
//! deux façons d'ouvrir la même base.
//!
//! ── LE DÉFAUT, TEL QU'IL A ÉTÉ TROUVÉ ──────────────────────────────────────
//!
//! Sur un store de 250 nœuds `a.g000`…`a.g249`, la même requête ne rend pas la
//! même chose selon le processus qui la pose :
//!
//! ```text
//!   obrain query <store> "… CONTAINS '.' … RETURN count(n)"       → 250
//!   obrain viz serve --store <store>  puis  /api/query?query=…    →   0
//! ```
//!
//! `viz serve` sert la base en **lecture seule** (`open_read_only`). Reproduit
//! ici, en mémoire et en une commande :
//!
//! ```text
//!   écriture      : CONTAINS '.' → 6   (juste)
//!   LECTURE SEULE : CONTAINS '.' → 0   (FAUX NÉGATIF)   ← le PREMIER appel GQL
//!                   CONTAINS '.' → 6   (juste)          ← le deuxième
//! ```
//!
//! Ce que la mesure établit, et qui est plus étroit que « c'est cassé » :
//!
//! * c'est **le premier appel GQL** d'un handle fraîchement ouvert en lecture
//!   seule qui rend zéro — pas l'aiguille, pas le prédicat, pas le substrat ;
//! * la **primitive** `ObrainDB::contient` rend la bonne réponse dès ce premier
//!   appel : le défaut est donc dans le CHEMIN DE PLANIFICATION GQL, pas dans
//!   l'index ni dans le verdict du substrat ;
//! * une seconde SESSION sur le même handle est juste : le défaut est par handle
//!   ouvert, pas par session ;
//! * une fois réchauffé, tout est juste — ce qui explique pourquoi il fallait le
//!   premier appel pour le voir, et pourquoi les caches d'affichage le rendent
//!   durable côté service (le corps vide est mis en cache sous son texte).
//!
//! ── LA VOIE EN CAUSE, PROUVÉE PAR L'INTERRUPTEUR ───────────────────────────
//!
//! Le même test, trois réglages, et un seul discriminant :
//!
//! ```text
//!   (défaut, index de trigrammes ACTIF)          → ÉCHEC   0 au lieu de 6
//!   OBRAIN_TRIGRAMMES=0        (index ÉTEINT)    → PASSE   6
//!   OBRAIN_TRIGRAMMES_DES_LA=1 (index dès le 1er)→ ÉCHEC   0 au lieu de 6
//! ```
//!
//! C'est donc la VOIE DE L'INDEX DE TRIGRAMMES qui rendait un ensemble de
//! candidats VIDE là où elle devait rendre la main au balayage. Le balayage, lui,
//! répondait juste — c'est ce que dit la ligne `OBRAIN_TRIGRAMMES=0`.
//!
//! ── LA CAUSE EXACTE, ET SA CORRECTION (2026-09-16) ─────────────────────────
//!
//! Une ligne de `noeuds_par_predicat_texte` (`obrain-substrate/src/store.rs`) :
//! sur une base ouverte en LECTURE SEULE, le chemin qui suit la construction de
//! l'index faisait
//!
//! ```text
//!   reverifier_texte(etat.read().candidats(propriete, aiguille).unwrap_or_default(), …)
//! ```
//!
//! `unwrap_or_default()` sur un `Option<Vec<u32>>` confond « l'index n'a rien à
//! dire » (`None`) et « aucune valeur ne correspond » (`Some(vec![])`) : le
//! silence devenait une AFFIRMATION DE ZÉRO, exactement ce que la doctrine du
//! module interdit partout ailleurs. Le `?` quatre-vingts lignes plus bas, sur le
//! chemin normal, propageait correctement — d'où le symptôme en trois temps :
//!
//! ```text
//!   appel 1  demandes(1) < des_la(2)          → garde de construction → BALAYE      juste
//!   appel 2  des_la atteint : on construit,
//!            puis lecture seule ⇒ unwrap_or_default() → vecteur vide  → ZÉRO        FAUX
//!   appel 3  index déjà construit ⇒ `?` propage le None  → BALAYE      juste
//! ```
//!
//! Les aiguilles de MOINS DE TROIS OCTETS tombent là à coup sûr (`candidats` rend
//! `None` : un trigramme fait trois octets, l'index ne peut rien promettre), et le
//! prédicat racine de `/api/viz/space` est `qualified_name CONTAINS '.'`. D'où le
//! symptôme en production : **le premier niveau qu'un utilisateur ouvre restait
//! vide**, et le corps vide était ensuite mis en cache par la route.
//!
//! La correction : le silence rend la main au balayage (`let Some(cands) = … else
//! { return None }`). Ce fichier est désormais le GARDIEN de cette correction.
//!
//! ── LES TROIS TESTS, ET POURQUOI ILS SONT SÉPARÉS ──────────────────────────
//!
//! 1. `les_aiguilles_courtes_comptent_juste_en_ecriture` — le corpus entier, en
//!    écriture, en mémoire et sur fichier : la référence.
//! 2. `la_premiere_requete_gql_en_lecture_seule_rend_zero` — le premier appel GQL
//!    d'un handle neuf en lecture seule. C'est CE test qui a échoué du premier
//!    coup et qui a mené à la correction ; il échouerait de nouveau si le silence
//!    de l'index redevenait un zéro.
//! 3. `toutes_les_aiguilles_apres_rechauffement_sont_justes_en_lecture_seule` —
//!    ACTIF. Il réchauffe par un premier appel, puis vérifie la MATRICE COMPLÈTE
//!    en lecture seule. Il passe aujourd'hui : c'est ce qui prouve que le défaut
//!    est borné au premier appel, et non à la lecture seule en général.
//!
//! ── POURQUOI CES AIGUILLES-LÀ ──────────────────────────────────────────────
//!
//! * **un seul caractère** (`.`, `g`, `0`, `é`) : contenu dans presque tout.
//!   C'est le contraire exact de ce qu'un index de trigrammes sait faire — un
//!   trigramme fait trois octets — donc la seule réponse correcte est le
//!   balayage, et rendre zéro est un faux négatif INDISTINGUABLE de « il n'y a
//!   rien » ;
//! * **l'aiguille vide** : contenue dans toute chaîne, et dans aucune valeur non
//!   textuelle (`as_str()?` rend NULL) ;
//! * **plus longue que toute valeur** : le pendant du faux négatif — un index
//!   bâclé rendrait « tout » ;
//! * **multi-octets** (`é`) : « courte » se compte en OCTETS, pas en caractères ;
//! * **à cheval sur un séparateur** (`ixgbe` dans `mixgbe_ready`) : le faux
//!   négatif de tokenisation.

use std::collections::BTreeMap;

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

const PROP: &str = "qn";

/// Corpus : des points, un séparateur qui fabrique un chevauchement, du
/// multi-octets, une chaîne vide — et une valeur NON textuelle qui ne doit
/// jamais matcher.
const CORPUS: &[&str] = &[
    "a.g000",
    "a.g001",
    "a.g002",
    "a.g000.long",
    "drivers.net.ixgbe.read",
    "drivers.net.mixgbe_ready",
    "obrain-iam",
    "réacteur",
    "",
];

/// Les aiguilles tordues, chacune avec la raison de sa présence.
const AIGUILLES: &[(&str, &str)] = &[
    (".", "un seul caractère, présent dans TOUTES les valeurs"),
    ("g", "un seul caractère, présent dans presque toutes"),
    ("0", "un seul caractère, présent dans quatre"),
    ("", "vide : toute chaîne, et aucune valeur non textuelle"),
    ("g0", "deux caractères"),
    ("a.g", "trois caractères"),
    ("a.g000", "la valeur entière d'un nœud"),
    ("a.g0000", "plus longue que toute valeur : ne doit RIEN rendre"),
    ("zzz", "absente partout"),
    ("ixgbe", "à cheval sur un séparateur (`mixgbe_ready`)"),
    ("é", "un caractère, DEUX octets : « courte » se compte en octets"),
    ("réacteur", "multi-octets, valeur entière"),
    ("a.g000.long", "valeur entière d'un seul nœud"),
];

/// Ce que le corpus DOIT rendre, calculé en Rust : l'oracle ne passe par aucune
/// route du moteur, donc il ne peut pas se tromper avec lui.
fn oracle(aiguille: &str) -> BTreeMap<&'static str, u64> {
    let mut m = BTreeMap::new();
    for v in CORPUS {
        if v.contains(aiguille) {
            *m.entry(*v).or_insert(0) += 1;
        }
    }
    m
}

fn attendu(aiguille: &str) -> i64 {
    oracle(aiguille).values().copied().sum::<u64>() as i64
}

fn peupler(d: &ObrainDB) {
    let s = d.session();
    for t in CORPUS {
        s.create_node_with_props(&["S"], [(PROP, Value::String((*t).into()))]);
    }
    // Valeur NON textuelle : `as_str()` rend NULL, donc AUCUNE ligne — même pour
    // l'aiguille vide, et même pour une aiguille d'un seul caractère.
    s.create_node_with_props(&["S"], [(PROP, Value::Int64(7))]);
}

/// Le compte par GQL — la forme EXACTE qui a divergé entre la CLI et le service.
fn compte_gql(d: &ObrainDB, aiguille: &str) -> i64 {
    let q = format!("MATCH (n) WHERE n.{PROP} CONTAINS '{aiguille}' RETURN count(n)");
    let r = d
        .session()
        .execute(&q)
        .unwrap_or_else(|e| panic!("requête refusée ({aiguille:?}) : {q}\n{e}"));
    match r.rows.first().and_then(|l| l.first()) {
        Some(Value::Int64(c)) => *c,
        autre => panic!("count(n) doit rendre un entier, pas {autre:?}"),
    }
}

/// Le compte par la PRIMITIVE — l'intention, sans texte GQL.
fn compte_primitive(d: &ObrainDB, aiguille: &str) -> usize {
    d.contient(PROP, aiguille)
        .unwrap_or_else(|e| panic!("primitive refusée ({aiguille:?}) : {e}"))
        .rows
        .len()
}

/// Le corpus entier, sur les deux routes, pour une façon d'ouvrir donnée.
fn verifier_tout(d: &ObrainDB, ouverture: &str) {
    for (aiguille, pourquoi) in AIGUILLES {
        assert_eq!(
            compte_gql(d, aiguille),
            attendu(aiguille),
            "{ouverture} : GQL `CONTAINS {aiguille:?}` ({pourquoi})"
        );
        assert_eq!(
            compte_primitive(d, aiguille),
            attendu(aiguille) as usize,
            "{ouverture} : primitive `contient({aiguille:?})` ({pourquoi})"
        );
    }
}

/// Un store sur disque, peuplé, FERMÉ — prêt à être rouvert.
fn store_peuple(td: &tempfile::TempDir, nom: &str) -> std::path::PathBuf {
    let chemin = td.path().join(nom);
    {
        let d = ObrainDB::open(&chemin).expect("ouverture en écriture");
        peupler(&d);
        // Le handle est rendu ici : une réouverture ne peut pas se tromper de
        // cause si elle échoue.
    }
    chemin
}

#[test]
fn les_aiguilles_courtes_comptent_juste_en_ecriture() {
    let d = ObrainDB::new_in_memory();
    peupler(&d);
    verifier_tout(&d, "écriture (en mémoire)");

    let td = tempfile::tempdir().expect("tempdir");
    let chemin = store_peuple(&td, "kb-contains-rw");
    let rw = ObrainDB::open(&chemin).expect("réouverture en écriture");
    verifier_tout(&rw, "écriture (fichier)");
}

/// LE DÉFAUT, ÉPINGLÉ — et c'est ce test qui l'a fait corriger.
///
/// Il a échoué du premier coup (`0` au lieu de `6`), il a désigné la voie (l'index
/// de trigrammes), puis la ligne exacte (`unwrap_or_default()` sur un `None`, sur
/// le chemin « lecture seule »), et il passe maintenant. Il reste ici pour que le
/// silence de l'index ne puisse pas redevenir un zéro sans que personne ne le voie.
#[test]
fn la_premiere_requete_gql_en_lecture_seule_rend_zero() {
    let td = tempfile::tempdir().expect("tempdir");
    let chemin = store_peuple(&td, "kb-contains-ro-premier");
    let ro = ObrainDB::open_read_only(&chemin).expect("ouverture en lecture seule");

    // 1. LE PREMIER APPEL : c'est ici que ça cassait, et sur une aiguille qui est
    //    dans TOUTES les valeurs — donc un zéro ne peut pas être un « il n'y a
    //    rien ».
    assert_eq!(
        compte_gql(&ro, "."),
        attendu("."),
        "le PREMIER compte GQL en lecture seule doit rendre {} (faux négatif mesuré)",
        attendu(".")
    );

    // 2. LA MÊME SECONDE, DANS LES DEUX ROUTES : elle LOCALISE le défaut.
    //    Si la GQL est fausse et la primitive juste au même instant, alors
    //    l'index et le verdict du substrat ne sont pas en cause.
    assert_eq!(
        compte_primitive(&ro, "."),
        attendu(".") as usize,
        "la primitive, au même instant que la GQL fausse"
    );
    assert_eq!(compte_gql(&ro, "."), attendu("."), "le deuxième appel GQL, lui, est juste");

    // 3. LA MATRICE COMPLÈTE, une fois réchauffé.
    verifier_tout(&ro, "LECTURE SEULE (après réchauffement)");
}

/// CE QUI PASSE AUJOURD'HUI, ET QUI BORNE LE DÉFAUT.
///
/// Un premier appel réchauffe ; ensuite, la lecture seule est juste sur TOUTES
/// les aiguilles. C'est la preuve que le défaut n'est pas « la lecture seule est
/// cassée » — et c'est aussi le filet qui attrapera une régression du chemin
/// normal, celui que le service emprunte à chaque requête.
#[test]
fn toutes_les_aiguilles_apres_rechauffement_sont_justes_en_lecture_seule() {
    let td = tempfile::tempdir().expect("tempdir");
    let chemin = store_peuple(&td, "kb-contains-ro-chaud");
    let ro = ObrainDB::open_read_only(&chemin).expect("ouverture en lecture seule");

    // Réchauffement : le PREMIER appel est celui qui se trompe (voir le test
    // épinglé ci-dessus) — on ne l'ignore pas, on l'absorbe, puisque c'est lui
    // qui est mesuré là-bas.
    let _ = compte_gql(&ro, ".");

    verifier_tout(&ro, "lecture seule (après réchauffement)");

    // Et une SECONDE SESSION sur le même handle : le défaut est par handle
    // ouvert, pas par session. Sans cet appel, on pourrait croire que chaque
    // session neuve repaie le défaut.
    let r = ro
        .session()
        .execute(&format!("MATCH (n) WHERE n.{PROP} CONTAINS '.' RETURN count(n)"))
        .expect("count");
    match r.rows.first().and_then(|l| l.first()) {
        Some(Value::Int64(c)) => assert_eq!(*c, attendu("."), "une session neuve sur un handle chaud"),
        autre => panic!("count(n) doit rendre un entier, pas {autre:?}"),
    }
}
