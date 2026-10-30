//! ── GROUP COMMIT À NEEDLE DE DURABILITÉ (2026-10-06) — PIÈCE 1 ──────────────
//!
//! CE QUE CES TESTS DOIVENT PROUVER, ET CONTRE QUOI ILS SE BATTENT.
//!
//! Le constat mesuré : le chemin de production (`Writer::commit()` → un `append`
//! par commit, `SyncMode::EveryCommit`) tient 644 rec/s, parce que le `fsync` est
//! indexé sur le **LOT** (`SyncMode::EveryCommit => commit_dans_le_lot`) et que le
//! lot d'un commit vaut 1. Le gain vient donc d'une seule chose : indexer le
//! `fsync` sur une **NEEDLE** (4 096 records / 4 Mio / 2 ms) au lieu du lot.
//!
//! Trois contrats sont testés ici, et chacun ÉCHOUE sur l'état d'avant :
//!
//! 1. **Le nombre de `fsync` suit la needle, pas les lots.** 12 288 records
//!    poussés un par un (12 288 LOTS) donnent 3 `fsync`, pas 12 288 — et le même
//!    chemin avec le group commit ÉTEINT en donne bien 1 par lot, ce qui est la
//!    moitié « rouge » de la comparaison.
//! 2. **La fenêtre de perte est bornée par `SEUIL_TEMPS`.** Un record poussé seul
//!    n'est PAS dans le fichier au retour de l'`append` — et il y est, `fsync`
//!    compris, sans que l'appelant ait rien fait, `SEUIL_TEMPS` plus tard.
//! 3. **Un record à exigence de synchronisation force le flush immédiat.**
//!    `FLAG_COMMIT` seul laisse le record en tampon ; `FLAG_COMMIT |
//!    FLAG_SYNC_ATTENDU` le rend durable AVANT le retour — et le bit n'est PAS
//!    écrit sur disque (le format ne bouge pas d'un octet).
//!
//! Deux tests gardent en plus ce que le group commit ne doit PAS casser : les
//! OCTETS écrits (identiques à N appends successifs — le rejeu n'a rien à savoir),
//! et l'invariant `lsn = ancre + position` quand un pair écrivain s'est glissé
//! dans le tampon (chemin de ré-estampillage).

use obrain_substrate::SyncMode;
use obrain_substrate::wal::{WalPayload, WalRecord};
use obrain_substrate::wal_io::{
    ANCRE_INITIALE, CoordinationAppend, SEUIL_OCTETS, SEUIL_RECORDS, SEUIL_TEMPS, WalReader,
    WalWriter,
};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Record réaliste : `PropSetInline` (clé courte + valeur), ~90 octets encodés
/// avec `taille_valeur` petit — la taille d'une écriture de graphe courante.
fn record(seq: u64, taille_valeur: usize) -> WalRecord {
    let mut valeur = Vec::with_capacity(taille_valeur);
    valeur.push((seq % 251) as u8);
    valeur.extend(std::iter::repeat_n(0xA5u8, taille_valeur.saturating_sub(1)));
    WalRecord {
        lsn: 0, // écrasé par l'append (ancre + position)
        timestamp: 1_700_000_000,
        flags: WalRecord::FLAG_COMMIT,
        payload: WalPayload::PropSetInline {
            node_id: seq as u32,
            key: format!("k{seq}"),
            value_bincode: valeur,
        },
    }
}

fn ouvrir(dir: &Path, nom: &str, mode: SyncMode) -> Arc<WalWriter> {
    Arc::new(WalWriter::open(dir.join(nom), mode, ANCRE_INITIALE).unwrap())
}

/// `(lsn, offset, longueur)` de chaque frame du WAL, dans l'ordre physique.
fn frames(wal: &Path) -> Vec<(u64, u64, u64)> {
    let r = WalReader::open(wal).unwrap();
    let mut v = Vec::new();
    for item in r.iter_from(0) {
        let (rec, off, len) = item.expect("le WAL doit se relire sans erreur");
        v.push((rec.lsn, off, len as u64));
    }
    v
}

fn taille_fichier(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

/// Écarte la needle de TEMPS (seuil très grand) : les `fsync` observés ne peuvent
/// alors venir que des seuils de RECORDS ou d'OCTETS, et leur compte devient exact
/// et reproductible. Le chemin exercé reste celui de production (le veilleur lit
/// ce seuil).
fn ecarter_la_needle_de_temps(w: &WalWriter) {
    w.forcer_seuil_temps(Duration::from_secs(3600));
}

// ===========================================================================
// 1 — LE FSYNC EST INDEXÉ SUR LA NEEDLE, PAS SUR LE LOT
// ===========================================================================
#[test]
fn needle_le_fsync_est_indexe_sur_la_needle_pas_sur_les_lots() {
    let dir = tempfile::tempdir().unwrap();

    // ── MOITIÉ « ROUGE » : LE CHEMIN D'AUJOURD'HUI, GROUP COMMIT ÉTEINT ────
    // Huit records, chacun dans SON lot (une seule itération d'`append`) :
    // huit `fsync`. C'est la règle `EveryCommit => commit_dans_le_lot` mesurée à
    // 644 rec/s en production — le lot d'un `commit()` vaut 1.
    let sans = ouvrir(dir.path(), "sans.wal", SyncMode::EveryCommit);
    for i in 0..8u64 {
        sans.append(record(i, 40)).unwrap();
    }
    assert_eq!(
        sans.fsyncs(),
        8,
        "hors group commit, l'`EveryCommit` doit fsyncer UNE FOIS PAR LOT : \
         le chemin mesuré à 644 rec/s est celui-là"
    );
    assert_eq!(sans.poses(), 8);

    // ── MOITIÉ « VERTE » : 12 288 LOTS, 3 FSYNC ────────────────────────────
    // Le MÊME chemin d'écriture (un `append` par record, donc un lot de 1 à
    // chaque fois), mais la needle décide : 3 × SEUIL_RECORDS records → 3 poses,
    // 3 `fsync`. Si le `fsync` était indexé sur le lot, on en compterait 12 288.
    let avec = ouvrir(dir.path(), "avec.wal", SyncMode::EveryCommit);
    ecarter_la_needle_de_temps(&avec);
    avec.activer_group_commit();
    let n = 3 * SEUIL_RECORDS;
    for i in 0..n as u64 {
        avec.append(record(i, 40)).unwrap();
    }
    let fsyncs = avec.fsyncs();
    assert_eq!(
        fsyncs, 3,
        "le fsync doit être indexé sur la NEEDLE : {n} records d'un seul record par \
         lot doivent donner {n}/{SEUIL_RECORDS} = 3 fsync, pas {n}. Obtenu : {fsyncs}"
    );
    assert_eq!(avec.poses(), 3, "un `write` par flush de needle");
    assert_eq!(avec.records_en_tampon(), 0, "la needle a vidé le tampon");
    assert_eq!(avec.octets_en_tampon(), 0);

    // Le fichier contient bien les 12 288 frames, dans l'ordre, sans trou : la
    // needle n'a rien fait disparaître, elle a seulement groupé les `write`.
    let chemin = dir.path().join("avec.wal");
    assert_eq!(taille_fichier(&chemin), avec.offset());
    let v = frames(&chemin);
    assert_eq!(v.len(), n);
    let mut fin = 0u64;
    for (lsn, off, len) in &v {
        assert_eq!(*off, fin, "les frames doivent être contiguës, sans trou");
        assert_eq!(
            *lsn,
            ANCRE_INITIALE + *off,
            "invariant lsn = ancre + position, sous group commit comme sans"
        );
        fin += len;
    }
    eprintln!(
        "NEEDLE records : {n} records / {n} lots → {fsyncs} fsync \
         (×{:.0} de fsync en moins ; group commit éteint : 1 fsync par lot)",
        n as f64 / fsyncs as f64
    );
}

// ===========================================================================
// 2 — LE SEUIL D'OCTETS DÉCLENCHE AVANT LE SEUIL DE RECORDS
// ===========================================================================
#[test]
fn needle_le_seuil_d_octets_declenche_avant_le_seuil_de_records() {
    let dir = tempfile::tempdir().unwrap();
    let w = ouvrir(dir.path(), "octets.wal", SyncMode::EveryCommit);
    ecarter_la_needle_de_temps(&w);
    w.activer_group_commit();

    // 192 records de ~64 Kio : bien MOINS que SEUIL_RECORDS, mais 12,6 Mio, soit
    // trois fois SEUIL_OCTETS. Le seuil de records ne peut donc PAS être la cause
    // des flush observés — c'est bien la borne MÉMOIRE qui déclenche.
    let n = 192usize;
    assert!(n < SEUIL_RECORDS, "le seuil de records ne doit pas pouvoir jouer");
    for i in 0..n as u64 {
        w.append(record(i, 65_536)).unwrap();
    }
    let fsyncs = w.fsyncs();
    assert_eq!(
        fsyncs, 3,
        "{n} records de 64 Kio font 12,6 Mio, soit 3 × SEUIL_OCTETS ({SEUIL_OCTETS} o) : \
         on attend 3 flush. Obtenu : {fsyncs}"
    );
    assert!(w.offset() > 3 * SEUIL_OCTETS as u64);
    eprintln!(
        "NEEDLE octets : {n} records ({} o) → {fsyncs} fsync, soit {} o par flush \
         (needle = {SEUIL_OCTETS} o)",
        w.offset(),
        w.offset() / fsyncs as u64
    );
}

// ===========================================================================
// 3 — LA FENÊTRE DE PERTE EST BORNÉE PAR SEUIL_TEMPS
// ===========================================================================
#[test]
fn needle_la_fenetre_de_perte_est_bornee_par_le_seuil_de_temps() {
    let dir = tempfile::tempdir().unwrap();
    let chemin = dir.path().join("temps.wal");
    let w = ouvrir(dir.path(), "temps.wal", SyncMode::EveryCommit);
    assert_eq!(w.seuil_temps(), SEUIL_TEMPS, "2 ms par défaut");
    w.activer_group_commit();

    let debut = Instant::now();
    w.append(record(1, 40)).unwrap();
    // ── AU RETOUR DE L'APPEND : RIEN N'EST SUR LE DISQUE ───────────────────
    // C'est l'aveu explicite du group commit — et ce que le veilleur doit fermer.
    assert_eq!(w.fsyncs(), 0, "aucun seuil franchi : aucun fsync");
    assert_eq!(taille_fichier(&chemin), 0, "le record est encore en tampon");
    assert_eq!(w.records_en_tampon(), 1);
    assert!(w.octets_en_tampon() > 0);

    // ── APRÈS L'ÉCHÉANCE : POSÉ ET FSYNCÉ, SANS QUE PERSONNE NE L'AIT DEMANDÉ ─
    // On SONDER pour MESURER la fenêtre réellement observée, pas pour la
    // supposer. Le repère est le COMPTEUR DE FSYNC et non la taille du fichier :
    // les octets apparaissent dès le `write` (page cache), mais la durabilité
    // n'est acquise qu'au retour du `fsync` — qui prend ici ~2,65 ms sur ce
    // disque. Sonder la taille mesurerait donc la fin du `write`, pas la fin de
    // la fenêtre de perte (piège trouvé en écrivant ce test : le compte tombait à
    // 0 alors que le veilleur était encore DANS le `fsync`).
    let limite = debut + Duration::from_millis(500);
    while w.fsyncs() == 0 && Instant::now() < limite {
        std::thread::sleep(Duration::from_micros(100));
    }
    let fenetre = debut.elapsed();
    assert_eq!(
        w.fsyncs(),
        1,
        "l'échéance de SEUIL_TEMPS doit avoir fait la pose ET le fsync"
    );
    assert_eq!(w.records_en_tampon(), 0);
    assert_eq!(w.octets_en_tampon(), 0);
    assert_eq!(taille_fichier(&chemin), w.offset());
    assert_eq!(frames(&chemin).len(), 1);
    assert!(
        fenetre < Duration::from_millis(100),
        "la fenêtre de perte doit être bornée par SEUIL_TEMPS={SEUIL_TEMPS:?} (+ le \
         temps du fsync) : mesurée à {fenetre:?}"
    );
    eprintln!(
        "NEEDLE temps : record rendu DURABLE au bout de {fenetre:?} \
         (échéance {SEUIL_TEMPS:?} + fsync) — l'appelant n'a jamais attendu"
    );
}

// ===========================================================================
// 4 — FLAG_COMMIT + EXIGENCE WAIT = FLUSH IMMÉDIAT
// ===========================================================================
#[test]
fn needle_un_record_a_exigence_de_sync_force_le_flush_immediat() {
    let dir = tempfile::tempdir().unwrap();
    let chemin = dir.path().join("wait.wal");
    let w = ouvrir(dir.path(), "wait.wal", SyncMode::EveryCommit);
    // La needle de temps est ÉCARTÉE : le seul flush possible est celui que
    // l'exigence du record demande. Sans cela, le test ne prouverait rien.
    ecarter_la_needle_de_temps(&w);
    w.activer_group_commit();

    // (a) Un commit ORDINAIRE reste en tampon — c'est la contre-épreuve.
    let ordinaire = record(1, 40);
    assert!(ordinaire.is_commit());
    assert!(!ordinaire.exige_sync());
    w.append(ordinaire).unwrap();
    assert_eq!(w.fsyncs(), 0, "FLAG_COMMIT seul ne suffit plus à fsyncer");
    assert_eq!(taille_fichier(&chemin), 0);

    // (b) Le MÊME record avec l'exigence : durable AVANT le retour de l'append.
    let mut exigeant = record(2, 40);
    exigeant.flags = WalRecord::FLAG_COMMIT | WalRecord::FLAG_SYNC_ATTENDU;
    assert!(exigeant.exige_sync());
    w.append(exigeant).unwrap();
    assert_eq!(
        w.fsyncs(),
        1,
        "un record à exigence WAIT doit forcer la pose ET le fsync immédiatement"
    );
    assert_eq!(taille_fichier(&chemin), w.offset());
    assert_eq!(w.records_en_tampon(), 0);

    // (c) Les DEUX frames sont là, et le bit WAIT n'a PAS été écrit sur disque :
    // c'est une demande adressée à l'écrivain, pas un état du journal (aucun
    // octet du format ne change).
    let r = WalReader::open(&chemin).unwrap();
    let lus: Vec<WalRecord> = r
        .iter_from(0)
        .map(|i| i.expect("relu").0)
        .collect();
    assert_eq!(lus.len(), 2);
    for rec in &lus {
        assert_eq!(
            rec.flags & WalRecord::FLAG_SYNC_ATTENDU,
            0,
            "le bit WAIT est une DEMANDE : il ne doit pas atteindre le disque"
        );
    }
    eprintln!(
        "NEEDLE wait : FLAG_COMMIT seul → 0 fsync, tampon ; FLAG_COMMIT|WAIT → \
         1 fsync immédiat, et l'octet flags relu vaut {} (bit WAIT absent)",
        lus[1].flags
    );
}

// ===========================================================================
// 5 — LES OCTETS SONT CEUX DE N APPENDS SUCCESSIFS (LE REJEU N'A RIEN À SAVOIR)
// ===========================================================================
#[test]
fn needle_le_group_commit_ecrit_les_memes_octets_que_n_appends() {
    let dir = tempfile::tempdir().unwrap();
    let n = 64u64;

    // Deux écrivains, même ancre, mêmes records : l'un groupé, l'autre immédiat.
    let groupe = ouvrir(dir.path(), "groupe.wal", SyncMode::EveryCommit);
    ecarter_la_needle_de_temps(&groupe);
    groupe.activer_group_commit();
    let immediat = ouvrir(dir.path(), "immediat.wal", SyncMode::EveryCommit);

    for i in 0..n {
        let rec = record(i, 40);
        groupe.append(rec.clone()).unwrap();
        immediat.append(rec).unwrap();
    }
    groupe.commit_point().unwrap();

    let (pg, pi) = (dir.path().join("groupe.wal"), dir.path().join("immediat.wal"));
    assert_eq!(
        std::fs::read(&pg).unwrap(),
        std::fs::read(&pi).unwrap(),
        "un tampon de group commit posé d'un bloc doit produire EXACTEMENT les \
         mêmes octets que {n} appends successifs (mêmes LSN, mêmes frames, même CRC)"
    );
    assert_eq!(groupe.offset(), immediat.offset());
    let (vg, vi) = (frames(&pg), frames(&pi));
    assert_eq!(vg, vi);
    assert_eq!(vg.len(), n as usize);
    eprintln!(
        "NEEDLE octets-identiques : {} o écrits par group commit == {} o écrits \
         append par append",
        taille_fichier(&pg),
        taille_fichier(&pi)
    );
}

// ===========================================================================
// 6 — LE DROP POSE LE TAMPON (DERNIER MAILLON DE LA FENÊTRE DE PERTE)
// ===========================================================================
#[test]
fn needle_le_drop_pose_le_tampon_sans_perte() {
    let dir = tempfile::tempdir().unwrap();
    let chemin = dir.path().join("drop.wal");
    let attendu;
    {
        let w = ouvrir(dir.path(), "drop.wal", SyncMode::EveryCommit);
        ecarter_la_needle_de_temps(&w);
        w.activer_group_commit();
        let mut fin = 0u64;
        for i in 0..100u64 {
            // ── L'OFFSET ATTENDU VIENT DU RETOUR DE L'APPEND, PAS DE `offset()` ─
            // Sous group commit, `offset()` ne désigne que ce qui est DÉJÀ POSÉ :
            // il vaut 0 tant que le tampon n'est pas vidé. L'attendre de
            // `offset()` après le Drop serait impossible (l'écrivain n'existe
            // plus) : on le construit donc à partir des positions rendues.
            let (off, len) = w.append(record(i, 40)).unwrap();
            fin = off + len as u64;
        }
        assert_eq!(
            w.offset(),
            0,
            "sous group commit, `offset()` ne compte que le POSÉ : rien n'est posé"
        );
        assert_eq!(taille_fichier(&chemin), 0, "100 records tiennent en tampon");
        assert_eq!(w.records_en_tampon(), 100);
        attendu = fin;
        // `w` (le seul Arc fort) est lâché ici : le veilleur ne tient qu'un Weak,
        // donc c'est bien le Drop du WalWriter qui s'exécute.
    }
    assert_eq!(taille_fichier(&chemin), attendu);
    assert_eq!(frames(&chemin).len(), 100);
}

// ===========================================================================
// 7 — UN PAIR QUI ÉCRIT PENDANT LE TAMPON : RÉ-ESTAMPILLAGE, LSN INTACTS
// ===========================================================================
#[test]
fn needle_un_pair_ecrivain_force_le_re_estampillage_du_tampon() {
    let dir = tempfile::tempdir().unwrap();
    let chemin = dir.path().join("partage.wal");

    let a = ouvrir(dir.path(), "partage.wal", SyncMode::EveryCommit);
    a.coordination(CoordinationAppend::OAppend);
    ecarter_la_needle_de_temps(&a);
    a.activer_group_commit();

    let b = Arc::new(WalWriter::open(&chemin, SyncMode::Never, ANCRE_INITIALE).unwrap());
    b.coordination(CoordinationAppend::OAppend);

    // `a` met un record en tampon SANS RIEN POSER : son `base` estampillé vaut 0.
    a.append(record(1, 40)).unwrap();
    assert_eq!(taille_fichier(&chemin), 0);
    // `b` écrit, lui, pour de vrai : le fichier avance sous le tampon de `a`.
    let (off_b, len_b) = b.append(record(100, 40)).unwrap();
    assert_eq!(off_b, 0, "b écrit à la fin réelle, qui était 0");
    // `a` empile un second record : sa prédiction (offset 0 + octets en tampon)
    // est juste du point de vue de `a`, et pourtant la position RÉELLE du tampon
    // a changé — c'est le cas que le ré-estampillage doit couvrir.
    a.append(record(2, 40)).unwrap();
    a.commit_point().unwrap();

    let v = frames(&chemin);
    assert_eq!(v.len(), 3, "aucune frame perdue (2 de a, 1 de b)");
    let mut fin = 0u64;
    for (lsn, off, len) in &v {
        assert_eq!(*off, fin, "les frames doivent être CONTIGUËS, sans trou");
        assert_eq!(
            *lsn,
            ANCRE_INITIALE + *off,
            "après ré-estampillage, lsn = ancre + position doit tenir pour TOUTES \
             les frames du tampon (y compris celles estampillées avant l'arrivée \
             du pair)"
        );
        fin += len;
    }
    assert!(len_b > 0);
    eprintln!(
        "NEEDLE pair : 3 frames contiguës ({} o), LSN = ancre + position partout \
         malgré une écriture d'un pair sous le tampon",
        taille_fichier(&chemin)
    );
}

// ===========================================================================
// 8 — LA NON-RÉGRESSION : GROUP COMMIT ÉTEINT, RIEN NE CHANGE
// ===========================================================================
#[test]
fn needle_group_commit_eteint_garde_le_chemin_d_aujourd_hui() {
    let dir = tempfile::tempdir().unwrap();
    let chemin = dir.path().join("eteint.wal");
    let w = ouvrir(dir.path(), "eteint.wal", SyncMode::Never);
    assert!(!w.group_commit_actif());

    for i in 0..500u64 {
        let (off, len) = w.append(record(i, 40)).unwrap();
        // En mode `Never` (et group commit éteint), la frame est écrite TOUT DE
        // SUITE : c'est la définition du chemin d'avant, et un `append` sous
        // group commit ne pourrait pas l'affirmer (la frame est encore en
        // tampon). C'est la mesure du « rien ne change tant que rien n'est armé ».
        assert_eq!(taille_fichier(&chemin), off + len as u64);
    }
    assert_eq!(w.fsyncs(), 0, "mode Never : aucun fsync");
    assert_eq!(w.octets_en_tampon(), 0);
    assert_eq!(w.records_en_tampon(), 0);
    assert_eq!(frames(&chemin).len(), 500);
}

// ===========================================================================
// 9 — TRONCATURE (CHECKPOINT) : L'ANCRE COMPTE LE TAMPON, AUCUN LSN RÉÉMIS
// ===========================================================================
//
// Un checkpoint tronque le WAL ET avance l'ancre de la taille du segment
// abandonné. Si les frames en attente dans le tampon du group commit n'étaient
// pas comptées, l'ancre avancerait trop PEU et les LSN du nouveau segment
// RECOUVRERAIENT ceux des frames jetées — exactement le doublon que l'ancre
// existe pour empêcher. C'est pourquoi `truncate` VIDE le tampon d'abord (voir
// son commentaire) : il est écrit, donc compté, puis jeté avec le segment.
#[test]
fn needle_la_troncature_compte_le_tampon_et_ne_reeemet_aucun_lsn() {
    let dir = tempfile::tempdir().unwrap();
    let chemin = dir.path().join("tronque.wal");
    let w = ouvrir(dir.path(), "tronque.wal", SyncMode::EveryCommit);
    ecarter_la_needle_de_temps(&w);
    w.activer_group_commit();

    let mut octets_en_attente = 0u64;
    for i in 0..10u64 {
        let (off, len) = w.append(record(i, 40)).unwrap();
        octets_en_attente = off + len as u64;
        assert_eq!(off + len as u64, octets_en_attente);
    }
    let ancre_avant = w.ancre();
    let lsn_max_du_tampon = ancre_avant + octets_en_attente - 1;

    let nouvelle_ancre = w.truncate().unwrap();
    assert_eq!(
        nouvelle_ancre,
        ancre_avant + octets_en_attente,
        "l'ancre doit avancer de la taille du segment abandonné, tampon COMPRIS"
    );
    assert_eq!(w.records_en_tampon(), 0, "le tampon a été vidé par la troncature");
    assert_eq!(w.octets_en_tampon(), 0);
    assert_eq!(taille_fichier(&chemin), 0, "le segment est bien tronqué");

    // Les records suivants portent des LSN STRICTEMENT supérieurs à ceux des
    // frames jetées : rien n'est réémis.
    let (off, _) = w.append(record(1000, 40)).unwrap();
    let lsn_du_nouveau = w.ancre() + off;
    assert!(
        lsn_du_nouveau > lsn_max_du_tampon,
        "LSN réémis après troncature : {lsn_du_nouveau} ≤ {lsn_max_du_tampon}"
    );
    eprintln!(
        "NEEDLE troncature : {octets_en_attente} o en attente comptés dans l'ancre \
         ({ancre_avant} → {nouvelle_ancre}) ; LSN suivant {lsn_du_nouveau} > \
         {lsn_max_du_tampon} (aucune réémission)"
    );
}

// ===========================================================================
// 10 — QUEUE DÉCHIRÉE : LA POSE DU TAMPON EST REFUSÉE COMME LES APPENDS
// ===========================================================================
//
// Défaut trouvé en RELISANT le code après coup : `poser_le_tampon` était le seul
// chemin d'écriture à ne pas passer par la garde de queue, donc le seul à pouvoir
// écrire DERRIÈRE une déchirure — par la needle, par le veilleur, par
// `commit_point`, par `fsync` ou par le Drop. Le contrat T4 est pourtant explicite
// (« tout append est refusé jusqu'à réparation ») : écrire derrière une déchirure
// fabriquerait des frames qu'aucun lecteur ne verrait, puisque le rejeu s'arrête
// sur la déchirure.
#[test]
fn needle_une_queue_dechiree_refuse_aussi_la_pose_du_tampon() {
    let dir = tempfile::tempdir().unwrap();
    let chemin = dir.path().join("dechire.wal");
    let w = ouvrir(dir.path(), "dechire.wal", SyncMode::EveryCommit);
    ecarter_la_needle_de_temps(&w);
    w.activer_group_commit();

    w.append(record(1, 40)).unwrap();
    assert_eq!(w.records_en_tampon(), 1);

    w.forcer_queue_a_repairer("déchirure simulée pour le test");
    assert!(w.append(record(2, 40)).is_err(), "l'append doit être refusé");
    assert!(
        w.commit_point().is_err(),
        "la POSE du tampon doit être refusée elle aussi : c'est le défaut corrigé"
    );
    assert!(w.fsync().is_err(), "le fsync passe par la même pose");
    assert_eq!(
        taille_fichier(&chemin),
        0,
        "rien ne doit être écrit derrière une déchirure"
    );

    // Après réparation (le fichier est vide : la réparation lève simplement le
    // refus), la pose repart et la frame en attente est écrite.
    w.reparer_queue().unwrap();
    w.commit_point().unwrap();
    assert_eq!(frames(&chemin).len(), 1);
    eprintln!(
        "NEEDLE queue déchirée : append, commit_point et fsync REFUSÉS tant que la queue \
         n'est pas réparée (0 octet écrit) ; après réparation, la frame en attente est \
         posée ({})",
        taille_fichier(&chemin)
    );
}
