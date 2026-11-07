//! MP-6 (v0.0.1, 2026-11-07) — ÉCRITURE MULTI-PROCESSUS PAR LE MOTEUR.
//!
//! ── LA GARANTIE VERROUILLÉE (décision PO f7740e26 : garantie RESTREINTE) ───
//! « N processus en `ObrainDB::open_shared_write` créent nœuds et arêtes SANS
//! propriétés, sans perte ni collision ; chacun voit les écritures des autres à
//! sa réouverture ; les commits acquittés d'un processus tué -9 sont présents,
//! ses non-commités absents. »
//!
//! Et rien de plus : PAS de propriétés, PAS de transactions concurrentes
//! multi-processus sur les mêmes entités. Ces deux points sont HORS garantie
//! v0.0.1 et sont déclarés comme limites, pas testés ici.
//!
//! ── POURQUOI DES PROCESSUS, ET PAS DES THREADS ─────────────────────────────
//! Les tests multi-écrivains du moteur (`multi_writer_invariants.rs`) sont
//! MONO-PROCESSUS : un seul `ObrainDB`, donc un seul compteur d'ids, un seul
//! `WalWriter`, un seul gestionnaire de transactions. Tout ce qui est « par
//! processus » (allocation, ancre WAL, marqueurs de transaction en mémoire) y
//! est invisible. Ce fichier est la seule preuve au niveau MOTEUR (sessions GQL,
//! commit implicite et explicite) que le substrat multi-écrivains (MP-1, MP-2,
//! axe append, phase B) tient de bout en bout.
//!
//! ── HARNAIS ────────────────────────────────────────────────────────────────
//! Même motif que `obrain-substrate/tests/mw_interprocess_phase_b.rs` : le
//! binaire de test se RELANCE lui-même en enfant (`std::env::current_exe()` +
//! `--exact <ce test> --nocapture --include-ignored`), le rôle est choisi par
//! `MWB_ROLE`, la synchronisation passe par des lignes sur stdout et par des
//! fichiers-drapeaux `go_*` dans un répertoire temporaire.
//!
//! ── SCÉNARIO ───────────────────────────────────────────────────────────────
//! 1. Le parent crée la base en EXCLUSIF (`ObrainDB::open`), y pose un nœud
//!    d'amorce, checkpointe et la FERME.
//! 2. Quatre enfants ouvrent en `open_shared_write` (chacun annonce « OUVERTURE
//!    ok » ou « OUVERTURE refus: … » : AUCUN refus n'est toléré), attendent le
//!    top départ, puis jouent chacun `N` instructions `INSERT (:RoleX)` et `M`
//!    instructions `INSERT (:RoleX)-[:LIEN_X]->(:RoleX)` en auto-commit (une
//!    instruction = une transaction acquittée). `RoleB` fait un
//!    `db.checkpoint()` au milieu (après ses nœuds, avant ses arêtes) : le
//!    checkpoint d'un pair PENDANT que les autres écrivent. `RoleD` est le
//!    MORT : il s'arrête après `K` instructions acquittées, annonce
//!    « COMMITE K », ouvre une transaction EXPLICITE, y insère des nœuds et
//!    des arêtes SANS commiter, annonce « ENCOURS », et dort. Le parent le
//!    tue -9 : ni `close()`, ni `Drop`, ni flush.
//! 3. Les trois survivants annoncent « FIN », ferment leur base, attendent
//!    `go_relecture_*`, ROUVRENT en `open_shared_write` et annoncent ce qu'ils
//!    VOIENT (« VU RoleA=… RoleB=… RoleC=… RoleD=… LIEN_A=… … ») : chacun doit
//!    voir EXACTEMENT les écritures acquittées de tous les autres.
//! 4. Le parent rouvre en EXCLUSIF et vérifie les comptes EXACTS par rôle et
//!    par type d'arête, nœud d'amorce compris : les `K` commits du mort sont
//!    là, sa transaction ouverte n'y est pas, et rien d'autre.
//!
//! ── MESURÉ sur e67ea5ec (2026-11-07, macOS, 3 exécutions avant que le
//! gouverneur de disque n'interrompe la série de 20) ────────────────────────
//! * Zéro perte, zéro collision, les `K` commits du mort présents, aucun refus
//!   d'ouverture, le checkpoint d'un pair sans effet sur les comptes : TENU.
//! * « Ses non-commités absents » : ROUGE, 3/3, déterministe. Relevé :
//!   `RoleD=105 LIEN_D=20` pour 90 / 15 attendus — c'est EXACTEMENT les 15
//!   nœuds + 5 arêtes de la transaction jamais commitée du processus tué.
//! * La variante de diagnostic (`variante_sans_flush_de_pair_…`, aucun flush
//!   de pair après la mort) est VERTE : la transaction en vol est écartée.
//!
//! Cause isolée par lecture : `SubstrateStore::flush()`
//! (`obrain-substrate/src/store.rs:8696`) capture `self.writer.wal().offset()`
//! — la FIN du WAL PARTAGÉ, qui contient les records appendus par les AUTRES
//! processus — et la persiste comme `wal_offset_at_flush` (`store.rs:8330`).
//! La réouverture suivante ne rejoue que depuis cet offset (`store.rs:6734`),
//! et l'écartement des transactions en vol (`replay.rs:376`) ne voit que les
//! `TxNoeudEnAttente` / `TxAreteEnAttente` de cette fenêtre. Un `close()` (ou
//! un checkpoint) d'un pair, postérieur aux déclarations d'attente du mort,
//! les sort donc de la fenêtre ; ses entités, physiquement dans les zones et
//! msyncées par ce même flush, sont publiées sans que rien ne les tombstone.
//! En mono-processus, c'est impossible : nul ne flushe à la place du mort.
#![cfg(feature = "wal")]

use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// Nœuds isolés par enfant (`INSERT (:RoleX)`).
const N: u64 = 60;
/// Arêtes par enfant (`INSERT (:RoleX)-[:LIEN_X]->(:RoleX)` : 2 nœuds + 1 arête).
const M: u64 = 30;
/// Instructions ACQUITTÉES par le mort avant son `kill -9` : ses `N` nœuds et
/// la moitié de ses arêtes — pour que les deux formes (nœud seul, chemin avec
/// arête) soient représentées dans ce qui doit survivre.
const K: u64 = N + M / 2;
/// Entités insérées par le mort dans sa transaction JAMAIS commitée.
const FANTOMES: u64 = 10;

const ROLES: [&str; 4] = ["RoleA", "RoleB", "RoleC", "RoleD"];
const ROLE_CHECKPOINT: &str = "RoleB";
const ROLE_MORT: &str = "RoleD";

// ---------------------------------------------------------------------------
// Harnais inter-processus
// ---------------------------------------------------------------------------

fn role() -> Option<String> {
    std::env::var("MWB_ROLE").ok()
}

fn env_obligatoire(cle: &str) -> String {
    std::env::var(cle).unwrap_or_else(|_| panic!("{cle} manquant dans l'enfant"))
}

fn annoncer(ligne: &str) {
    println!("{ligne}");
    std::io::stdout().flush().ok();
}

/// Attend l'existence d'un fichier-drapeau (borne : 60 s).
fn attendre_fichier(p: &Path, quoi: &str) {
    let debut = Instant::now();
    while !p.exists() {
        assert!(
            debut.elapsed() < Duration::from_secs(60),
            "attente de {quoi} ({}) : délai dépassé",
            p.display()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

struct Enfant {
    role: &'static str,
    process: Child,
    lignes: std::io::Lines<std::io::BufReader<std::process::ChildStdout>>,
}

impl Enfant {
    fn lancer(test: &str, role: &'static str, vars: &[(&str, String)]) -> Self {
        let moi = std::env::current_exe().expect("current_exe");
        let mut cmd = Command::new(moi);
        // `--include-ignored` : l'enfant est CE MÊME test, rejoué avec un rôle ;
        // il doit le jouer quel que soit son marquage (voir
        // `mw_interprocess_append.rs`).
        cmd.args(["--exact", test, "--nocapture", "--include-ignored"])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        for (k, v) in vars {
            cmd.env(k, v);
        }
        let mut process = cmd.spawn().expect("lancer l'enfant");
        let sortie = process.stdout.take().expect("stdout de l'enfant");
        Self {
            role,
            process,
            lignes: std::io::BufReader::new(sortie).lines(),
        }
    }

    /// Lit stdout jusqu'à la ligne qui COMMENCE par `prefixe`, et rend le reste
    /// de cette ligne.
    fn attendre_ligne(&mut self, prefixe: &str) -> String {
        loop {
            match self.lignes.next() {
                Some(Ok(l)) => {
                    if let Some(reste) = l.strip_prefix(prefixe) {
                        return reste.trim().to_string();
                    }
                }
                autre => panic!(
                    "enfant {} terminé sans « {prefixe} » ({autre:?})",
                    self.role
                ),
            }
        }
    }

    fn terminer(mut self) {
        let statut = self.process.wait().expect("attendre l'enfant");
        assert!(
            statut.success(),
            "l'enfant {} a échoué : {statut:?} (voir son stderr ci-dessus)",
            self.role
        );
    }

    /// `SIGKILL` : le scénario « crash sans flush » — aucun `Drop`, aucun
    /// `close()`, aucun msync. Le noyau rend le `flock` au passage.
    fn tuer(mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

// ---------------------------------------------------------------------------
// Requêtes
// ---------------------------------------------------------------------------

fn scalaire(db: &ObrainDB, requete: &str) -> i64 {
    let s = db.session();
    let r = s
        .execute(requete)
        .unwrap_or_else(|e| panic!("{requete} : {e}"));
    match r.rows.first().and_then(|row| row.first()) {
        Some(Value::Int64(n)) => *n,
        autre => panic!("{requete} : scalaire inattendu {autre:?}"),
    }
}

fn compte_noeuds(db: &ObrainDB, etiquette: &str) -> i64 {
    scalaire(db, &format!("MATCH (n:{etiquette}) RETURN count(n)"))
}

/// Arêtes du type `LIEN_X` dont les DEUX extrémités portent l'étiquette du
/// rôle : c'est « des arêtes entre SES nœuds ».
fn compte_aretes_propres(db: &ObrainDB, role: &str) -> i64 {
    scalaire(
        db,
        &format!(
            "MATCH (a:{role})-[r:{}]->(b:{role}) RETURN count(r)",
            lien(role)
        ),
    )
}

/// Arêtes du type `LIEN_X`, quelles que soient les extrémités.
fn compte_aretes_toutes(db: &ObrainDB, role: &str) -> i64 {
    scalaire(
        db,
        &format!("MATCH ()-[r:{}]->() RETURN count(r)", lien(role)),
    )
}

fn lien(role: &str) -> String {
    format!("LIEN_{}", role.strip_prefix("Role").unwrap_or(role))
}

/// Instruction numéro `i` du programme d'un enfant : `N` nœuds, puis `M`
/// chemins « nœud —arête→ nœud ».
fn instruction(role: &str, i: u64) -> String {
    if i < N {
        format!("INSERT (:{role})")
    } else {
        format!("INSERT (:{role})-[:{}]->(:{role})", lien(role))
    }
}

/// Ce que `k` instructions acquittées laissent : `(nœuds, arêtes)`.
fn attendu(k: u64) -> (i64, i64) {
    let noeuds = k.min(N);
    let chemins = k.saturating_sub(N);
    ((noeuds + 2 * chemins) as i64, chemins as i64)
}

fn relever(db: &ObrainDB) -> String {
    let mut champs = Vec::new();
    for r in ROLES {
        champs.push(format!("{r}={}", compte_noeuds(db, r)));
    }
    for r in ROLES {
        champs.push(format!("{}={}", lien(r), compte_aretes_toutes(db, r)));
    }
    champs.push(format!("Amorce={}", compte_noeuds(db, "Amorce")));
    champs.join(" ")
}

/// Le relevé ATTENDU après la mort de `ROLE_MORT` à `K` et la fin des autres.
fn releve_attendu() -> String {
    let mut champs = Vec::new();
    for r in ROLES {
        let k = if r == ROLE_MORT { K } else { N + M };
        champs.push(format!("{r}={}", attendu(k).0));
    }
    for r in ROLES {
        let k = if r == ROLE_MORT { K } else { N + M };
        champs.push(format!("{}={}", lien(r), attendu(k).1));
    }
    champs.push("Amorce=1".to_string());
    champs.join(" ")
}

// ---------------------------------------------------------------------------
// Enfants
// ---------------------------------------------------------------------------

/// Ouvre en écriture partagée et ANNONCE le résultat : un refus est une
/// information de la garantie, le parent doit la lire, pas la deviner.
fn ouvrir_partage(dir: &Path) -> ObrainDB {
    match ObrainDB::open_shared_write(dir) {
        Ok(db) => {
            annoncer("OUVERTURE ok");
            db
        }
        Err(e) => {
            annoncer(&format!("OUVERTURE refus: {e}"));
            std::process::exit(2);
        }
    }
}

/// Écrivain SURVIVANT : `N + M` instructions acquittées, checkpoint au milieu
/// pour `ROLE_CHECKPOINT`, puis fermeture, réouverture sur ordre du parent et
/// relevé de ce qu'il voit.
fn enfant_ecrivain(dir: &Path, role: &str, go: &Path, go_relecture: &Path) -> ! {
    // Variante de DIAGNOSTIC (`MWB_SORTIE=brutale`) : AUCUN flush de pair —
    // ni checkpoint au milieu, ni `close()` à la fin — pour isoler ce qu'un
    // flush de PAIR change à ce que la réouverture voit du processus tué.
    let brutale = std::env::var("MWB_SORTIE").as_deref() == Ok("brutale");
    let db = ouvrir_partage(dir);
    attendre_fichier(go, "le top départ");
    let s = db.session();
    for i in 0..(N + M) {
        s.execute(&instruction(role, i))
            .unwrap_or_else(|e| panic!("{role} instruction {i} : {e}"));
        if i + 1 == N && role == ROLE_CHECKPOINT && !brutale {
            db.checkpoint()
                .unwrap_or_else(|e| panic!("{role} checkpoint : {e}"));
            annoncer("CHECKPOINT fait");
        }
    }
    drop(s);
    annoncer(&format!("FIN {}", N + M));
    if brutale {
        // Sortie sans `close()` ni `Drop` : pas le moindre flush.
        std::process::exit(0);
    }
    // Fermeture PROPRE (le contrat « voit les autres à sa réouverture » porte
    // sur une réouverture, pas sur une poignée vivante).
    db.close().unwrap_or_else(|e| panic!("{role} close : {e}"));
    drop(db);

    attendre_fichier(go_relecture, "l'ordre de relecture");
    let db = ouvrir_partage(dir);
    annoncer(&format!("VU {}", relever(&db)));
    db.close()
        .unwrap_or_else(|e| panic!("{role} close (relecture) : {e}"));
    std::process::exit(0);
}

/// Écrivain qui MEURT : `K` instructions acquittées, puis une transaction
/// explicite JAMAIS commitée, puis attente du `SIGKILL`.
fn enfant_mort(dir: &Path, role: &str, go: &Path) -> ! {
    let db = ouvrir_partage(dir);
    attendre_fichier(go, "le top départ");
    let s = db.session();
    for i in 0..K {
        s.execute(&instruction(role, i))
            .unwrap_or_else(|e| panic!("{role} instruction {i} : {e}"));
    }
    drop(s);
    // Le marqueur n'est annoncé QU'APRÈS les `Ok` : tout ce qui précède est
    // acquitté au sens du contrat.
    annoncer(&format!("COMMITE {K}"));

    let mut t = db.session();
    t.begin_transaction().expect("begin");
    for i in 0..FANTOMES {
        // Même forme que les survivants (nœuds puis chemins), pour que les
        // fantômes ressemblent trait pour trait à ce qui doit survivre.
        t.execute(&instruction(role, if i % 2 == 0 { 0 } else { N }))
            .unwrap_or_else(|e| panic!("{role} fantôme {i} : {e}"));
    }
    annoncer(&format!("ENCOURS {FANTOMES}"));
    // Ni commit, ni close, ni Drop : le parent va nous SIGKILL.
    loop {
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ===========================================================================
// LE TEST
// ===========================================================================

/// LA GARANTIE, telle que décidée : survivants qui FERMENT proprement (donc
/// flushent), rouvrent et relèvent ; puis réouverture exclusive du parent.
#[test]
fn quatre_processus_ecrivent_sans_perte_et_un_kill_9_ne_laisse_que_l_acquitte() {
    scenario(
        "quatre_processus_ecrivent_sans_perte_et_un_kill_9_ne_laisse_que_l_acquitte",
        false,
    );
}

/// VARIANTE DE DIAGNOSTIC — même scénario, mais AUCUN flush de pair : pas de
/// checkpoint au milieu, et les survivants sortent SANS `close()` et ne
/// rouvrent pas ; seul le parent rouvre, en exclusif. Elle isole la cause du
/// rouge de la garantie : si cette variante est verte et la garantie rouge, ce
/// n'est pas le rejeu qui publie la transaction en vol du mort, c'est le FLUSH
/// D'UN PAIR qui fait avancer `wal_offset_at_flush` au-delà de ses
/// `TxNoeudEnAttente` / `TxAreteEnAttente`, hors de la fenêtre de rejeu.
#[test]
fn variante_sans_flush_de_pair_la_transaction_en_vol_du_mort_est_ecartee() {
    scenario(
        "variante_sans_flush_de_pair_la_transaction_en_vol_du_mort_est_ecartee",
        true,
    );
}

fn scenario(test: &str, sortie_brutale: bool) {
    if let Some(r) = role() {
        let dir = PathBuf::from(env_obligatoire("MWB_DIR"));
        let etiquette = env_obligatoire("MWB_ETIQUETTE");
        let go = PathBuf::from(env_obligatoire("MWB_GO"));
        match r.as_str() {
            "ecrivain" => {
                let go_relecture = PathBuf::from(env_obligatoire("MWB_GO_RELECTURE"));
                enfant_ecrivain(&dir, &etiquette, &go, &go_relecture)
            }
            "mort" => enfant_mort(&dir, &etiquette, &go),
            autre => panic!("rôle inconnu {autre}"),
        }
    }

    let td = tempfile::tempdir().unwrap();
    let dir = td.path().join("base");

    // ── 1. CRÉATION EN EXCLUSIF, PUIS FERMETURE ────────────────────────────
    {
        let db = ObrainDB::open(&dir).expect("créer la base en exclusif");
        db.session()
            .execute("INSERT (:Amorce)")
            .expect("nœud d'amorce");
        db.checkpoint().expect("checkpoint initial");
        db.close().expect("fermer la base");
    }

    // ── 2. QUATRE ENFANTS EN ÉCRITURE PARTAGÉE ─────────────────────────────
    let mut enfants: Vec<(PathBuf, Enfant)> = Vec::new();
    for r in ROLES {
        let go = td.path().join(format!("go_{r}"));
        let go_relecture = td.path().join(format!("go_relecture_{r}"));
        let role_env = if r == ROLE_MORT { "mort" } else { "ecrivain" };
        let e = Enfant::lancer(
            test,
            r,
            &[
                ("MWB_ROLE", role_env.into()),
                ("MWB_DIR", dir.display().to_string()),
                ("MWB_ETIQUETTE", r.into()),
                ("MWB_GO", go.display().to_string()),
                ("MWB_GO_RELECTURE", go_relecture.display().to_string()),
                (
                    "MWB_SORTIE",
                    if sortie_brutale { "brutale" } else { "propre" }.into(),
                ),
            ],
        );
        enfants.push((go, e));
    }

    // Toutes les ouvertures ont réussi AVANT la moindre écriture : quatre
    // écrivains partagés coexistent, aucun n'est refusé.
    for (_, e) in &mut enfants {
        let verdict = e.attendre_ligne("OUVERTURE ");
        assert_eq!(
            verdict, "ok",
            "l'enfant {} s'est vu REFUSER l'ouverture en écriture partagée : {verdict}",
            e.role
        );
    }
    for (go, _) in &enfants {
        std::fs::write(go, b"go").expect("top départ");
    }

    // ── 3. LE MORT : K ACQUITTÉS, UNE TRANSACTION OUVERTE, SIGKILL ─────────
    let (mut survivants, mut morts): (Vec<_>, Vec<_>) =
        enfants.into_iter().partition(|(_, e)| e.role != ROLE_MORT);
    let (_, mort) = morts.pop().expect("un mort");
    let mut mort = mort;
    let k: u64 = mort
        .attendre_ligne("COMMITE ")
        .parse()
        .expect("compte de commits");
    assert_eq!(k, K);
    let fantomes: u64 = mort
        .attendre_ligne("ENCOURS ")
        .parse()
        .expect("compte de fantômes");
    assert_eq!(fantomes, FANTOMES);
    mort.tuer(); // SIGKILL : ni close, ni Drop, ni flush.

    // ── 4. LES SURVIVANTS FINISSENT, PUIS ROUVRENT ET RELÈVENT ─────────────
    for (_, e) in &mut survivants {
        if e.role == ROLE_CHECKPOINT && !sortie_brutale {
            e.attendre_ligne("CHECKPOINT ");
        }
        let fin: u64 = e.attendre_ligne("FIN ").parse().expect("compte FIN");
        assert_eq!(
            fin,
            N + M,
            "{} n'a pas joué toutes ses instructions",
            e.role
        );
    }
    let releve_voulu = releve_attendu();
    if sortie_brutale {
        // Les survivants sont déjà sortis sans flush : on les attend, puis on
        // passe directement à la réouverture exclusive du parent.
        for (_, e) in survivants {
            e.terminer();
        }
        verifier_en_exclusif(&dir, &releve_voulu);
        return;
    }
    for (_, e) in &survivants {
        std::fs::write(td.path().join(format!("go_relecture_{}", e.role)), b"go")
            .expect("ordre de relecture");
    }
    for (_, e) in &mut survivants {
        let verdict = e.attendre_ligne("OUVERTURE ");
        assert_eq!(
            verdict, "ok",
            "l'enfant {} s'est vu REFUSER sa RÉOUVERTURE en écriture partagée : {verdict}",
            e.role
        );
        let vu = e.attendre_ligne("VU ");
        assert_eq!(
            vu,
            releve_voulu,
            "{} ne voit pas, à sa réouverture, exactement les écritures \
             acquittées des autres (K={K} pour le mort, N+M={} pour chacun des \
             autres)",
            e.role,
            N + M
        );
    }
    for (_, e) in survivants {
        e.terminer();
    }
    verifier_en_exclusif(&dir, &releve_voulu);
}

/// ── 5. RÉOUVERTURE EXCLUSIVE PAR LE PARENT : COMPTES EXACTS ────────────────
fn verifier_en_exclusif(dir: &Path, releve_voulu: &str) {
    let db = ObrainDB::open(dir).expect("réouverture exclusive après la mort d'un pair");
    let releve = relever(&db);
    assert_eq!(
        releve, releve_voulu,
        "relevé après réouverture exclusive ≠ attendu : perte, collision, ou \
         fantôme du processus tué (sa transaction ouverte portait {FANTOMES} \
         entités de plus)"
    );
    for r in ROLES {
        let k = if r == ROLE_MORT { K } else { N + M };
        let (_, aretes) = attendu(k);
        assert_eq!(
            compte_aretes_propres(&db, r),
            aretes,
            "les arêtes {} ne relient pas toutes deux nœuds {r} : une extrémité \
             a été écrasée par un pair (collision d'ids)",
            lien(r)
        );
    }
    let total_noeuds: i64 = ROLES.iter().map(|r| compte_noeuds(&db, r)).sum::<i64>() + 1;
    assert_eq!(
        db.node_count() as i64,
        total_noeuds,
        "des nœuds sans étiquette attendue existent : fantôme ou collision"
    );
    let total_aretes: i64 = ROLES.iter().map(|r| compte_aretes_toutes(&db, r)).sum();
    assert_eq!(
        db.edge_count() as i64,
        total_aretes,
        "des arêtes d'un type inattendu existent : fantôme ou collision"
    );
    eprintln!(
        "MP-6 : 4 processus × ({N} nœuds + {M} chemins), mort à K={K} avec \
         {FANTOMES} fantômes non commités, checkpoint de {ROLE_CHECKPOINT} au \
         milieu — relevé = {releve}"
    );
}
