//! MIGRATION VERSIONNEE — le contrat de la machinerie (jalon 0, RFC
//! principal-natif-v2 §3).
//!
//! Ce fichier teste la MACHINE de migration, pas une migration reelle : la
//! table de production est vide au jalon 0 (1→1). Les proprietes prouvees :
//!
//!   1. NO-OP — un store a la version courante n'est jamais touche.
//!   2. CHAINE — une table simulee 0→courante s'execute a l'ouverture par la
//!      machine, et UNE SEULE fois (idempotence du double passage).
//!   3. POINT DE COMMIT — la version meta n'est bumpee QU'APRES le succes
//!      complet d'une etape. Une etape dechiree (Err a mi-chemin) laisse la
//!      version intacte ; la relance reprend et aboutit.
//!   4. KILL -9 REEL — un enfant tue par SIGKILL au milieu d'une etape lente
//!      laisse un store a l'ancienne version, jamais mixte ; la reprise
//!      aboutit. (Complement du modele « etape dechiree », qui est le patron
//!      de la suite — cf. community_crash_safety.rs : drop sans msync est
//!      STRICTEMENT plus fort que SIGKILL. On paie ici quand meme le SIGKILL
//!      reel, exige par le chantier.)
//!   5. REFUS — version future refusee proprement ; version ancienne sans
//!      etape dans la table refusee proprement ; ouverture non-exclusive
//!      (read-only) d'un store ancien refusee sans migration.
//!
//!   cargo test -p obrain-substrate --test migration_versionnee

use obrain_core::graph::GraphStore;
use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use obrain_substrate::error::SubstrateError;
use obrain_substrate::meta::SUBSTRATE_FORMAT_VERSION;
use obrain_substrate::migration::{
    Migration, ecrire_version_meta, lire_version_meta, migrer_repertoire,
    migrer_repertoire_avec_table,
};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Cree un store avec `n` noeuds etiquetes, le ferme, rend le repertoire.
fn store_ferme_avec(dir: &Path, n: usize) {
    let store = SubstrateStore::create(dir).expect("create");
    for i in 0..n {
        let label = format!("Contrat{i}");
        store.create_node(&[label.as_str()]);
    }
    drop(store);
}

fn compter_noeuds(dir: &Path) -> usize {
    let store = SubstrateStore::open(dir).expect("open apres migration");
    store.node_ids().len()
}

/// Etape simulee : appose une ligne dans `marqueur.txt` (compteur de
/// passages) — permet de prouver qu'une etape court UNE fois et une seule.
fn etape_marqueur(dir: &Path) -> Result<(), SubstrateError> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("marqueur.txt"))?;
    writeln!(f, "passage")?;
    f.sync_all()?;
    Ok(())
}

fn passages(dir: &Path) -> usize {
    std::fs::read_to_string(dir.join("marqueur.txt"))
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

/// Etape dechiree : fait la moitie de son travail (le marqueur), puis echoue.
fn etape_dechiree(dir: &Path) -> Result<(), SubstrateError> {
    etape_marqueur(dir)?;
    Err(SubstrateError::WalBadFrame(
        "etape dechiree volontairement (simulation de panne)".into(),
    ))
}

fn table_simulee() -> Vec<Migration> {
    vec![Migration {
        de: 0,
        vers: SUBSTRATE_FORMAT_VERSION,
        etape: etape_marqueur,
    }]
}

// ---------------------------------------------------------------------------
// 1. NO-OP sur un store a jour
// ---------------------------------------------------------------------------

#[test]
fn store_neuf_version_courante_et_migrer_noop() {
    let td = tempfile::tempdir().unwrap();
    store_ferme_avec(td.path(), 3);
    assert_eq!(lire_version_meta(td.path()).unwrap(), SUBSTRATE_FORMAT_VERSION);
    migrer_repertoire(td.path()).expect("migrer sur store a jour = no-op");
    assert_eq!(lire_version_meta(td.path()).unwrap(), SUBSTRATE_FORMAT_VERSION);
    assert_eq!(compter_noeuds(td.path()), 3);
}

// ---------------------------------------------------------------------------
// 2. CHAINE simulee 0→courante, executee par l'OUVERTURE
// ---------------------------------------------------------------------------

#[test]
fn chaine_simulee_migre_et_une_seule_fois() {
    let td = tempfile::tempdir().unwrap();
    store_ferme_avec(td.path(), 3);
    ecrire_version_meta(td.path(), 0).unwrap();
    assert_eq!(lire_version_meta(td.path()).unwrap(), 0);

    migrer_repertoire_avec_table(td.path(), &table_simulee()).expect("migration simulee");
    assert_eq!(lire_version_meta(td.path()).unwrap(), SUBSTRATE_FORMAT_VERSION);
    assert_eq!(passages(td.path()), 1, "l'etape court UNE fois");

    // Idempotence : un second passage est un no-op total.
    migrer_repertoire_avec_table(td.path(), &table_simulee()).expect("no-op");
    assert_eq!(passages(td.path()), 1, "pas de second passage");

    // Le store reste integralement lisible.
    assert_eq!(compter_noeuds(td.path()), 3);
}

// ---------------------------------------------------------------------------
// 3. POINT DE COMMIT — etape dechiree, version intacte, reprise
// ---------------------------------------------------------------------------

#[test]
fn etape_dechiree_ne_bump_pas_puis_reprise_aboutit() {
    let td = tempfile::tempdir().unwrap();
    store_ferme_avec(td.path(), 2);
    ecrire_version_meta(td.path(), 0).unwrap();

    let table_cassee = vec![Migration {
        de: 0,
        vers: SUBSTRATE_FORMAT_VERSION,
        etape: etape_dechiree,
    }];
    let err = migrer_repertoire_avec_table(td.path(), &table_cassee)
        .expect_err("l'etape dechiree doit remonter");
    assert!(matches!(err, SubstrateError::WalBadFrame(_)));
    assert_eq!(
        lire_version_meta(td.path()).unwrap(),
        0,
        "la version ne bump JAMAIS avant le succes complet de l'etape"
    );

    // Reprise : la bonne table aboutit (l'etape est idempotente par contrat).
    migrer_repertoire_avec_table(td.path(), &table_simulee()).expect("reprise");
    assert_eq!(lire_version_meta(td.path()).unwrap(), SUBSTRATE_FORMAT_VERSION);
    assert_eq!(compter_noeuds(td.path()), 2);
}

// ---------------------------------------------------------------------------
// 4. KILL -9 REEL pendant une etape lente, puis reprise
// ---------------------------------------------------------------------------

/// Corps de l'ENFANT — ne fait rien sans la variable d'environnement.
/// L'etape lente signale `enfant_pret` puis dort : le parent le tue
/// pendant que l'etape est EN COURS, garanti par construction.
#[test]
fn enfant_migration_lente() {
    let Ok(dir) = std::env::var("OBRAIN_TEST_ENFANT_MIGRATION") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let table = vec![Migration {
        de: 0,
        vers: SUBSTRATE_FORMAT_VERSION,
        etape: etape_lente_pour_enfant,
    }];
    let _ = migrer_repertoire_avec_table(&dir, &table);
}

fn etape_lente_pour_enfant(dir: &Path) -> Result<(), SubstrateError> {
    etape_marqueur(dir)?;
    std::fs::write(dir.join("enfant_pret"), b"pret")?;
    // Dort par petits pas : le parent envoie SIGKILL bien avant la fin.
    for _ in 0..3000 {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(())
}

#[test]
fn kill_9_pendant_migration_puis_reprise() {
    let td = tempfile::tempdir().unwrap();
    store_ferme_avec(td.path(), 4);
    ecrire_version_meta(td.path(), 0).unwrap();

    let exe = std::env::current_exe().unwrap();
    let mut enfant = std::process::Command::new(exe)
        .args(["enfant_migration_lente", "--exact", "--nocapture"])
        .env("OBRAIN_TEST_ENFANT_MIGRATION", td.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn enfant");

    // Attend que l'etape soit EN COURS (marqueur poser par l'etape lente).
    let pret = td.path().join("enfant_pret");
    let mut attentes = 0u32;
    while !pret.exists() {
        attentes += 1;
        assert!(attentes < 600, "l'enfant n'a jamais demarre son etape");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // SIGKILL — un vrai kill -9, pas une simulation.
    enfant.kill().expect("kill -9 enfant");
    let _ = enfant.wait();

    // JAMAIS MIXTE : l'etape etait en cours, la version n'a pas bouge.
    assert_eq!(
        lire_version_meta(td.path()).unwrap(),
        0,
        "kill -9 au milieu d'une etape : la version meta reste l'ancienne"
    );

    // REPRISE idempotente : la migration relancee aboutit.
    migrer_repertoire_avec_table(td.path(), &table_simulee()).expect("reprise apres kill");
    assert_eq!(lire_version_meta(td.path()).unwrap(), SUBSTRATE_FORMAT_VERSION);
    assert_eq!(compter_noeuds(td.path()), 4);
}

// ---------------------------------------------------------------------------
// 5. REFUS propres
// ---------------------------------------------------------------------------

#[test]
fn version_future_refusee_proprement() {
    let td = tempfile::tempdir().unwrap();
    store_ferme_avec(td.path(), 1);
    ecrire_version_meta(td.path(), 99).unwrap();

    let err = match SubstrateStore::open(td.path()) {
        Err(e) => e,
        Ok(_) => panic!("version future acceptee"),
    };
    let msg = format!("{err}");
    assert!(
        msg.contains("99"),
        "le refus nomme la version trouvee : {msg}"
    );
    // Rien n'a ete touche.
    assert_eq!(lire_version_meta(td.path()).unwrap(), 99);
}

#[test]
fn version_ancienne_sans_etape_refusee_a_l_ouverture() {
    let td = tempfile::tempdir().unwrap();
    store_ferme_avec(td.path(), 1);
    // La version 0 n'a d'etape ni dans la table de production du jalon 0
    // (vide) ni dans aucune table future (la premiere entree reelle est 1→2).
    ecrire_version_meta(td.path(), 0).unwrap();

    let err = match SubstrateStore::open(td.path()) {
        Err(e) => e,
        Ok(_) => panic!("version sans chemin acceptee"),
    };
    let msg = format!("{err}");
    assert!(msg.contains('0'), "le refus nomme la version trouvee : {msg}");
    assert_eq!(
        lire_version_meta(td.path()).unwrap(),
        0,
        "un refus ne touche a rien"
    );
}

#[test]
fn ouverture_read_only_ne_migre_jamais() {
    let td = tempfile::tempdir().unwrap();
    store_ferme_avec(td.path(), 1);
    ecrire_version_meta(td.path(), 0).unwrap();

    // Une ouverture non-exclusive (verrou partage) ne peut pas migrer :
    // refus propre, version intacte.
    assert!(
        SubstrateStore::open_read_only(td.path()).is_err(),
        "read-only ne migre pas"
    );
    assert_eq!(lire_version_meta(td.path()).unwrap(), 0);
}
