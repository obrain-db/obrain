//! ── LE PLAFOND D'ÉTIQUETTES NE DOIT PLUS TUER LE PROCESSUS ─────────────────
//!
//! Défaut mesuré le 2026-09-16 : le substrat porte un `label_bitset: u64`, donc
//! **64 étiquettes distinctes par base**. Au 65ᵉ label, `create_node` — dont le
//! contrat est INFAILLIBLE — appelait `.expect("label registry overflow (>64
//! labels)")` : le processus ABANDONNAIT. Une seule requête (ou un agent avec
//! droit d'écriture) suffisait donc à tuer le démon.
//!
//! Le contrat reste : 64 étiquettes, pas une de plus. Ce qui change, c'est la
//! RÉPONSE : l'exécuteur demande la capacité AVANT de créer
//! (`GraphStore::valider_etiquettes`) et refuse la requête avec un message qui
//! dit la limite et la sortie. C'est la garantie du filet : une limite dure est
//! acceptable, une limite qui tue ne l'est pas.
//!
//! Ce test est le filet, et il a été VÉRIFIÉ PAR MUTATION : en retirant les deux
//! appels à `valider_etiquettes` de l'opérateur de mutation, la requête fait
//! tomber le processus —
//! `panicked at store.rs:19870: label registry overflow (>64 labels)` — et le
//! test échoue. C'est exactement le défaut : une REQUÊTE (ou un agent avec droit
//! d'écriture) pouvait tuer le démon. Avec les gardes, elle est refusée et la
//! base reste interrogeable.

use obrain_engine::ObrainDB;

#[test]
fn le_65e_label_est_refuse_proprement_et_le_processus_survit() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();

    // 64 étiquettes distinctes : la capacité exacte du substrat.
    for i in 0..64 {
        session
            .execute(&format!("CREATE (:L{i} {{i: {i}}})"))
            .unwrap_or_else(|e| panic!("création du label {i} refusée : {e}"));
    }
    let avant = session
        .execute("MATCH (n) RETURN count(n) AS n")
        .expect("comptage après 64 étiquettes");
    let compte = format!("{:?}", avant.rows);
    assert!(compte.contains("64"), "64 nœuds attendus, obtenu {compte}");

    // 65ᵉ étiquette : la requête DOIT rendre une erreur explicite.
    let err = session
        .execute("CREATE (:L65 {i: 65})")
        .expect_err("le 65ᵉ label doit être refusé, pas créer un nœud de plus");
    let message = format!("{err:?}");
    assert!(
        message.contains("64") && message.to_lowercase().contains("etiquette")
            || message.contains("L65"),
        "le refus doit dire la LIMITE et ce qui a été demandé : {message}"
    );

    // ── LE PROCESSUS EST TOUJOURS LÀ, ET LA BASE INTACTE ──────────────────
    // C'est l'assertion qui porte tout le test : avant le correctif, on
    // n'arrivait jamais ici.
    let apres = session
        .execute("MATCH (n) RETURN count(n) AS n")
        .expect("la base doit rester interrogeable après un refus");
    assert_eq!(
        format!("{:?}", apres.rows),
        compte,
        "aucun nœud n'a été créé par la requête refusée"
    );

    // Une étiquette DÉJÀ connue reste utilisable : le refus porte sur la
    // capacité, pas sur l'écriture.
    session
        .execute("CREATE (:L0 {i: 0})")
        .expect("réutiliser une étiquette existante doit marcher");
    let final_ = session
        .execute("MATCH (n) RETURN count(n) AS n")
        .expect("comptage final");
    assert!(
        format!("{:?}", final_.rows).contains("65"),
        "le nœud réutilisant L0 doit exister"
    );
}
