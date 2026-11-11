//! ── LIMITE v0.0.1 : L'ARÈNE DES BLOBS EST ÉCRASÉE EN ÉCRITURE PARTAGÉE ──────
//!
//! Limite connue, verrouillée : voir `docs/LIMITES-v0.0.1.md`, ligne
//! « Écriture partagée : arène des blobs ». Ce test ne vérifie PAS une
//! garantie. Il fige le défaut actuel pour qu'un correctif le fasse rougir.
//!
//! Mécanisme (`obrain-substrate/src/blob_column.rs`) : `BlobColumnWriter::create`
//! lit `arena_end` une seule fois, à l'ouverture (et `hydrate_from_dict` crée le
//! writer dès l'ouverture quand la colonne existe). `write_slot` écrit ensuite
//! à ce `arena_end` mis en cache. Deux processus ouverts en écriture partagée
//! écrivent donc au MÊME offset de `dat`. Le second écrit par-dessus les octets
//! du premier, et l'entrée d'idx du premier pointe alors vers des octets d'un
//! autre.
//!
//! Scénario : A ouvre la base (writer hydraté, `arena_end` = E). Le pair B ouvre,
//! écrit une valeur de 300 octets à E, puis ferme. A écrit une valeur de
//! 400 octets à E, puis ferme. Un troisième processus relit la valeur longue de
//! chacun des deux nœuds, identifiés par l'id que leur écrivain a rapporté.
//!
//! Résultat attendu aujourd'hui : la valeur de B est relue comme les 300 premiers
//! octets de la valeur de A (corrompue). A est relue intacte.
//!
//! Hors périmètre de ce test : la propriété courte `auteur` n'est pas relue (les
//! propriétés inline partagent leurs pages entre processus, défaut distinct et
//! documenté « propriétés et tas »). On ne relit donc que la valeur longue.
//!
//! Harnais : les écrivains et le lecteur sont des SOUS-PROCESSUS (même binaire
//! de test, rôle par variable d'environnement), comme
//! `vector_index_deux_processus.rs`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use obrain_substrate::BLOB_COLUMN_THRESHOLD_BYTES;
use tempfile::tempdir;

const ROLE: &str = "ARENE_MP_ROLE";
const DOSSIER: &str = "ARENE_MP_DIR";
const TEST: &str = "limite_v0_0_1_arene_blobs_ecrasee";

/// Charges au-dessus du seuil blob : elles passent par l'arène `dat`.
const LONG_B: usize = 300;
const LONG_A: usize = 400;
const _: () = assert!(LONG_B > BLOB_COLUMN_THRESHOLD_BYTES && LONG_A > BLOB_COLUMN_THRESHOLD_BYTES);

fn role() -> Option<String> {
    std::env::var(ROLE).ok()
}

/// Lance ce même test dans un sous-processus avec le rôle donné.
fn enfant(role: &str, dir: &Path) {
    let moi = std::env::current_exe().expect("current_exe");
    let statut = Command::new(moi)
        .args(["--exact", TEST, "--nocapture"])
        .env(ROLE, role)
        .env(DOSSIER, dir)
        .status()
        .expect("lancer le sous-processus");
    assert!(statut.success(), "sous-processus `{role}` en échec");
}

/// Crée un nœud `Doc` portant une valeur longue de `n` fois `c` ; rend son id.
fn ecrire_valeur(db: &ObrainDB, n: usize, c: char) -> u64 {
    let id = db.create_node(&["Doc"]);
    db.set_node_property(id, "corps", Value::String(c.to_string().repeat(n).into()));
    id.0
}

/// Base de départ : une valeur longue, pour que la colonne blob et son
/// spécifiant existent déjà à l'ouverture des deux écrivains.
fn base_de_depart(dir: &Path) {
    let db = ObrainDB::open(dir).expect("open");
    ecrire_valeur(&db, LONG_B, 'z');
    db.close().expect("close");
}

/// Le pair B : ouvre en écriture partagée, écrit sa valeur, ferme, et dépose
/// l'id de son nœud dans `<parent>/id_b.txt`.
fn ecrire_pair_b(dir: &Path) {
    let db = ObrainDB::open_shared_write(dir).expect("open_shared_write pair B");
    let id = ecrire_valeur(&db, LONG_B, 'b');
    db.close().expect("close pair B");
    std::fs::write(
        dir.parent().expect("parent").join("id_b.txt"),
        id.to_string(),
    )
    .expect("écrire l'id de B");
}

/// Le troisième processus : relit `id(n)` et `n.corps` de chaque nœud `Doc` et
/// dépose une ligne `id<TAB>corps` par nœud dans `<parent>/lecture.txt`.
fn lire(dir: &Path) {
    let db = ObrainDB::open_shared_write(dir).expect("reouverture lecteur");
    let resultat = db
        .execute("MATCH (n:Doc) RETURN id(n), n.corps")
        .expect("requête de lecture");
    let mut sortie = String::new();
    for ligne in &resultat.rows {
        let id = match ligne.first() {
            Some(Value::Int64(i)) => i.to_string(),
            autre => format!("<{autre:?}>"),
        };
        let corps = match ligne.get(1) {
            Some(Value::String(s)) => s.to_string(),
            autre => format!("<{autre:?}>"),
        };
        sortie.push_str(&format!("{id}\t{corps}\n"));
    }
    db.close().expect("close lecteur");
    let chemin = dir.parent().expect("parent").join("lecture.txt");
    std::fs::write(chemin, sortie).expect("écrire la lecture");
}

#[test]
fn limite_v0_0_1_arene_blobs_ecrasee() {
    // Défaut actuel : voir l'en-tête du fichier et docs/LIMITES-v0.0.1.md.
    if let Some(r) = role() {
        let dir = PathBuf::from(std::env::var(DOSSIER).expect("dossier"));
        match r.as_str() {
            "pair_b" => ecrire_pair_b(&dir),
            "lit" => lire(&dir),
            autre => panic!("rôle inconnu : {autre}"),
        }
        return;
    }

    let td = tempdir().expect("tempdir");
    let dir = td.path().join("arene");
    base_de_depart(&dir);

    // A ouvre la base AVANT le pair : son writer de blobs cache `arena_end`.
    let a = ObrainDB::open_shared_write(&dir).expect("A open_shared_write");
    // B écrit à ce même `arena_end` et ferme : l'en-tête sur disque avance.
    enfant("pair_b", &dir);
    // A écrit à son `arena_end` périmé, par-dessus les octets de B, puis ferme.
    let id_a = ecrire_valeur(&a, LONG_A, 'a');
    a.close().expect("close A");

    enfant("lit", &dir);

    let id_b: u64 = std::fs::read_to_string(td.path().join("id_b.txt"))
        .expect("id_b.txt")
        .trim()
        .parse()
        .expect("id de B");
    let lecture = std::fs::read_to_string(td.path().join("lecture.txt")).expect("lecture.txt");
    let relus: HashMap<String, String> = lecture
        .lines()
        .filter_map(|l| {
            let mut champs = l.split('\t');
            Some((champs.next()?.to_string(), champs.next()?.to_string()))
        })
        .collect();

    let corps_a = relus
        .get(&id_a.to_string())
        .unwrap_or_else(|| panic!("le nœud de A ({id_a}) doit être relu ; lecture :\n{lecture}"));
    let corps_b = relus
        .get(&id_b.to_string())
        .unwrap_or_else(|| panic!("le nœud de B ({id_b}) doit être relu ; lecture :\n{lecture}"));

    // A est relue intacte : c'est elle qui a gagné l'arène.
    assert_eq!(*corps_a, "a".repeat(LONG_A), "A relue intacte");
    // B est CORROMPUE : relue comme les LONG_B premiers octets de A, au lieu de
    // ses propres 300 octets `b`. Limite v0.0.1, à corriger en v0.0.2.
    assert_ne!(
        *corps_b,
        "b".repeat(LONG_B),
        "LIMITE v0.0.1 : la valeur de B n'est plus relue telle qu'écrite"
    );
    assert_eq!(
        *corps_b,
        "a".repeat(LONG_B),
        "LIMITE v0.0.1 : B est relue comme le préfixe des octets écrits par A à son arena_end périmé"
    );
}
