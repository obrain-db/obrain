//! Journal de session persistant — zone substrate dédiée par session.
//!
//! Le journal brut des conversations (les lignes `{type,seq,time,data}` du
//! log de session) est stocké DANS obrain, dans une zone mmap par session
//! (`substrate.journal.<session_id>`) — pas dans le graphe (7M lignes en
//! propriétés → store 6 Go → requêtes lentes → timeout bridge) ni en
//! externe. Le bridge `session_load events_only` lit ces zones à la demande.
//!
//! ## Format de la zone
//!
//! ```text
//! header  : magic 4 ("OBJ1") + count u64 + used u64   (16 octets)
//! payload : frames bincode concaténées (len u32 + bytes String)
//! ```
//!
//! La zone est mmap-écrite (grow_to + as_slice_mut), le WAL substrate fige
//! l'état au checkpoint. Lecture séquentielle par offset (after_seq) ou par
//! fenêtre (before_seq/limit) — le plus récent en dernier.

use std::sync::Arc;

use obrain_substrate::SubstrateStore;
use obrain_substrate::file::ZoneFile;

/// Nom de zone pour une session.
pub fn zone_filename(session_id: &str) -> String {
    // Sanitise : les sessions sont des UUID (sûr), mais on garde la règle
    // du substrate (alphanumérique + tiret).
    format!("substrate.journal.{}", session_id)
}

/// Magic d'en-tête.
const MAGIC: &[u8; 4] = b"OBJ1";
/// Taille de l'en-tête (magic 4 + count 8 + used 8).
const HEADER_LEN: usize = 20;
/// Croissance initiale.
const INITIAL_GROW: u64 = 64 * 1024;

/// Écrit le journal d'une session dans sa zone (remplace l'existant).
/// `lines` = les lignes JSON brutes du journal, triées par seq.
pub fn write_session_journal(
    store: &Arc<SubstrateStore>,
    session_id: &str,
    lines: &[String],
) -> Result<(), String> {
    let name = zone_filename(session_id);
    let sub = store.writer().substrate();
    let guard = sub.lock();
    // (ré)crée la zone (mmap truncate à la taille finale)
    let mut zf = create_zone(&guard, &name, lines)?;
    zf.msync().map_err(|e| format!("journal: msync: {e}"))?;
    Ok(())
}

fn create_zone(
    guard: &parking_lot::MutexGuard<'_, obrain_substrate::file::SubstrateFile>,
    name: &str,
    lines: &[String],
) -> Result<ZoneFile, String> {
    // Sérialise toutes les frames d'abord (la taille finale est connue).
    let mut frames: Vec<u8> = Vec::new();
    for line in lines {
        let frame = bincode::serde::encode_to_vec(line, bincode::config::standard())
            .map_err(|e| format!("journal: bincode: {e}"))?;
        frames.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        frames.extend_from_slice(&frame);
    }
    let total = HEADER_LEN + frames.len();

    let mut zf = guard
        .open_named_zone(name)
        .map_err(|e| format!("journal: open: {e}"))?;
    if (total as u64) > zf.len() {
        zf.grow_to(total as u64)
            .map_err(|e| format!("journal: grow: {e}"))?;
    }
    zf.shrink_to(total as u64)
        .map_err(|e| format!("journal: shrink: {e}"))?;

    let slice = zf.as_slice_mut();
    slice[..4].copy_from_slice(MAGIC);
    slice[4..12].copy_from_slice(&(lines.len() as u64).to_le_bytes());
    slice[12..20].copy_from_slice(&(total as u64).to_le_bytes()); // used = fin des données
    slice[HEADER_LEN..].copy_from_slice(&frames);
    Ok(zf)
}

/// APPEND de lignes à la zone d'une session (capture live GraphWriter).
/// Contrairement à `write_session_journal` (remplace l'existant), cette
/// fonction AJOUTE les nouvelles lignes à la fin de la zone et met à jour
/// count/used — le journal accumule les lots successifs d'import_events.
pub fn append_session_journal(
    store: &Arc<SubstrateStore>,
    session_id: &str,
    lines: &[String],
) -> Result<(), String> {
    let name = zone_filename(session_id);
    let sub = store.writer().substrate();
    let guard = sub.lock();

    if lines.is_empty() {
        return Ok(());
    }

    let mut zf = guard
        .open_named_zone(&name)
        .map_err(|e| format!("journal: open: {e}"))?;

    // Zone absente/invalide (len < header ou magic différent) → création
    // complète depuis les lignes fournies (équivalent write).
    let (used, count) = {
        let existing = zf.as_slice();
        let valid = existing.len() >= HEADER_LEN && &existing[..4] == MAGIC;
        if !valid {
            let mut nz = create_zone(&guard, &name, lines)?;
            nz.msync().map_err(|e| format!("journal: msync: {e}"))?;
            return Ok(());
        }
        (
            u64::from_le_bytes(existing[12..20].try_into().unwrap_or([0; 8])) as usize,
            u64::from_le_bytes(existing[4..12].try_into().unwrap_or([0; 8])) as usize,
        )
    };

    /* (2026-10-16) L'ENREGISTREMENT D'EN-TÊTE N'EST ÉCRIT QU'UNE FOIS.
     *
     * L'en-tête de session (`{"type":"session","createdAt":…,"cwd":…}`) ne porte
     * PAS de `seq` : le filtre par seq que l'appelant applique désormais aux
     * autres lignes (`mcp_server::import_events`) ne peut donc pas le
     * dédupliquer, et l'écarter par un `seq = -1 ≤ dernier` priverait le journal
     * de son en-tête — donc `enteteDeJournal` de son `createdAt`, donc la
     * session d'un nœud exploitable. C'est ICI, et seulement ici, que l'état de
     * la zone est connu : une zone qui porte déjà des données (`used >
     * HEADER_LEN`) a déjà son en-tête, et toute copie supplémentaire est un
     * doublon.
     *
     * Mesuré le 2026-10-16 : 2096 copies de cet enregistrement dans une seule
     * zone, 85 124 en quatre jours sur runtime-dev. */
    let retenues: Vec<&String> = if used > HEADER_LEN {
        lines.iter().filter(|l| !est_entete_de_session(l)).collect()
    } else {
        lines.iter().collect()
    };
    if retenues.is_empty() {
        return Ok(());
    }

    // Sérialise les frames des lignes retenues.
    let mut frames: Vec<u8> = Vec::new();
    for line in &retenues {
        let frame = bincode::serde::encode_to_vec(*line, bincode::config::standard())
            .map_err(|e| format!("journal: bincode: {e}"))?;
        frames.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        frames.extend_from_slice(&frame);
    }
    if frames.is_empty() {
        return Ok(());
    }

    let new_used = used + frames.len();
    if (new_used as u64) > zf.len() {
        zf.grow_to(new_used as u64)
            .map_err(|e| format!("journal: grow: {e}"))?;
    }
    {
        let slice = zf.as_slice_mut();
        // Header : count + used mis à jour (le magic et le reste inchangés).
        slice[4..12].copy_from_slice(&((count + retenues.len()) as u64).to_le_bytes());
        slice[12..20].copy_from_slice(&(new_used as u64).to_le_bytes());
        // Les nouvelles frames à la fin des données existantes.
        if used + frames.len() <= slice.len() {
            slice[used..used + frames.len()].copy_from_slice(&frames);
        }
    }
    zf.msync().map_err(|e| format!("journal: msync: {e}"))?;
    Ok(())
}

/// Lit le journal d'une session depuis sa zone. `after_seq` : ne retourne
/// que les lignes avec seq > after_seq (pagination avant). `before_seq` :
/// ne retourne que les lignes avec seq <= before_seq (fenêtre récente).
/// `limit` : plafond de lignes retournées (les DERNIÈRES dans la fenêtre).
pub fn read_session_journal(
    store: &Arc<SubstrateStore>,
    session_id: &str,
    after_seq: i64,
    before_seq: i64,
    limit: usize,
) -> Vec<String> {
    let name = zone_filename(session_id);
    let sub = store.writer().substrate();
    let guard = sub.lock();
    // Ouvre la zone en LECTURE SEULE (le bridge est en mode RO partagé —
    // open_named_zone → open_or_create exige l'écriture → échoue).
    let zf = match guard.open_named_zone_read_only(&name) {
        Ok(z) => z,
        Err(e) => {
            eprintln!("journal: open zone {name} en RO: {e}");
            return Vec::new();
        }
    };
    let bytes = zf.as_slice();
    if bytes.len() < HEADER_LEN || &bytes[..4] != MAGIC {
        return Vec::new();
    }
    let used = u64::from_le_bytes(bytes[12..20].try_into().unwrap_or([0; 8])) as usize;
    let end = used.min(bytes.len());
    if end <= HEADER_LEN {
        return Vec::new();
    }

    // (2026-09-01) Repérage des frames SANS décodage : la zone est une suite
    // de [len:u32][charge utile bincode]. On ne lit que les préfixes et on
    // saute — arithmétique de pointeur, O(nb frames) — puis on ne DÉCODE que
    // la fenêtre demandée. Décoder tout (746k events → 149 Mo) coûtait ~1,6 s
    // par session, d'où session.list à 18 s.
    //
    // La version précédente remontait depuis la FIN en lisant les 4 octets
    // avant chaque frame comme une longueur. Or le cadrage est un PRÉFIXE :
    // ces octets-là sont la fin de la charge utile précédente. La longueur
    // lue était donc arbitraire (~2 Go mesuré), `len_pos - len` débordait en
    // usize et l'indexation partait en panique — « bridge stream error
    // (session_load) » côté harness, sur 14 sessions testées sur 14.
    //
    // (2026-09-08) RESYNCHRONISATION — une frame corrompue au MILIEU de la
    // zone (append interrompu, octets zéroïsés par un crash, écriture
    // concurrente) ne doit plus amputer toute la QUEUE du journal. Constat
    // réel (session 842672aa, 2026-09-07) : 35 octets zéroïsés vers seq
    // ~46752 → l'ancien scan s'arrêtait net et `session_load events_only`
    // (le chemin du resume) ne voyait que seq 0..46751 alors que la zone
    // contenait 732 k frames jusqu'à seq 360343 — le fold LLM repartait
    // d'un point antérieur, « delta des derniers messages perdu ». On
    // repère donc les frames SANS décodage tant que le cadrage est sain ;
    // à la première frame invalide, on cherche la prochaine position où un
    // [len:u32] plausible précède une charge bincode qui DÉCODE en String
    // JSON portant un `seq` numérique (validation forte, pas un simple
    // saut d'octets) — borné à RESYNC_MAX_OCTETS pour ne pas s'acharner
    // sur une zone franchement corrompue. Les cadres ainsi retrouvés
    // entrent dans `cadres` comme les autres ; la fenêtre et le tri par seq
    // en aval n'ont rien à changer.
    let mut cadres: Vec<(usize, usize)> = Vec::new(); // (début charge, longueur)
    let mut off = HEADER_LEN;
    const RESYNC_MAX_OCTETS: usize = 1 << 20; // 1 Mo de recherche max par rupture
    let mut resyncs: usize = 0;
    while off + 4 <= end {
        let len = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        let payload_start = off + 4;
        if len == 0 || payload_start + len > end {
            // Cadrage cassé : chercher la prochaine frame valide.
            let mut repart = None;
            let recherche_max = end.min(payload_start + RESYNC_MAX_OCTETS);
            for cand in payload_start..recherche_max.saturating_sub(4) {
                let clen = u32::from_le_bytes(bytes[cand..cand + 4].try_into().unwrap()) as usize;
                let cpayload = cand + 4;
                if clen == 0 || cpayload + clen > end {
                    continue;
                }
                // Validation forte : la charge doit être une String bincode
                // dont le JSON porte un `seq` numérique. Sans décodage on ne
                // peut pas distinguer une vraie frame d'un artefact.
                let Ok((ligne, _)) = bincode::serde::decode_from_slice::<String, _>(
                    &bytes[cpayload..cpayload + clen],
                    bincode::config::standard(),
                ) else {
                    continue;
                };
                if event_seq(&ligne) < 0 {
                    continue;
                }
                repart = Some((cpayload, clen));
                break;
            }
            match repart {
                Some((p, l)) => {
                    resyncs += 1;
                    cadres.push((p, l));
                    off = p + l;
                }
                None => break, // fin de zone ou corruption irrécupérable
            }
        } else {
            cadres.push((payload_start, len));
            off = payload_start + len;
        }
    }
    if resyncs > 0 {
        // Signal ops : la zone a été lue malgré des ruptures de cadrage. Le
        // journal reste exploitable (les frames valides après chaque rupture
        // sont rendues), mais l'écrivain a laissé des trous — à surveiller.
        eprintln!(
            "journal: zone {name} lue avec {resyncs} resynchronisation(s) (frames corrompues au milieu)"
        );
    }

    // Sélection de la fenêtre : on remonte l'index à REBOURS et on ne décode
    // que ce qui entre dedans. Le journal est append-only et ses seq croissent,
    // donc remonter depuis la fin atteint d'abord les seq les plus hauts —
    // exactement ce que demande une pagination `before_seq`.
    //
    // TOLÉRANCE AUX FRAMES DE QUEUE (2026-09-01, porte « historique ») : une
    // frame illisible est SAUTÉE, pas fatale. Un append interrompu laisse une
    // frame partielle en fin de zone ; abandonner au premier échec rendait 0
    // event sur une zone de 143 Mo pourtant saine sur 99,9 % de sa longueur —
    // mesuré 0 et 1 event là où 3 étaient demandés. La version d'origine
    // dégradait en douceur parce qu'elle décodait depuis le DÉBUT ; en ne
    // décodant que la queue, il faut restituer cette tolérance explicitement.
    let veut = if limit > 0 { limit } else { usize::MAX };
    let sans_filtre = after_seq <= 0 && before_seq <= 0;
    let mut retenus: Vec<String> = Vec::new();
    let mut illisibles: usize = 0;
    for &(s, l) in cadres.iter().rev() {
        if sans_filtre && retenus.len() >= veut {
            break;
        }
        match bincode::serde::decode_from_slice::<String, _>(
            &bytes[s..s + l],
            bincode::config::standard(),
        ) {
            Ok((line, _)) => {
                let seq = event_seq(&line);
                if seq > after_seq && (before_seq <= 0 || seq <= before_seq) {
                    retenus.push(line);
                    if !sans_filtre && retenus.len() >= veut {
                        break;
                    }
                }
            }
            Err(_) => {
                illisibles += 1;
                if illisibles > 64 {
                    break; // zone franchement corrompue, on ne s'acharne pas
                }
            }
        }
    }
    retenus.reverse();
    let mut filtered: Vec<String> = retenus;

    // (2026-09-05, P4b) ORDRE GARANTI PAR SEQ À LA SORTIE. Le contrat
    // documenté du journal (et ce qu'attend le harness : « events triés par
    // seq », session_load/readRecent) est un ordre croissant par seq. La
    // lecture suppose que l'ordre physique == ordre seq (« append-only et
    // seq croissants ») ; en cas d'append désordonné (respawn / retry après
    // EPIPE — historiques harness), les lignes sortiraient dans l'ordre
    // physique. Tri stable par seq : no-op quand l'ordre physique est déjà
    // correct (cas nominal, coût O(k log k) sur la fenêtre déjà bornée par
    // `limit`), correctif sinon. `event_seq` vaut -1 pour une ligne sans seq
    // (frame de contrôle) — triée en tête, stable entre elles.
    filtered.sort_by(|a, b| {
        event_seq(a)
            .cmp(&event_seq(b))
            .then_with(|| std::cmp::Ordering::Equal)
    });

    if limit > 0 && filtered.len() > limit {
        filtered.split_off(filtered.len() - limit)
    } else {
        filtered
    }
}

/// Le seq d'une ligne d'événement (parse JSON léger).
pub fn event_seq(line: &str) -> i64 {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|v| v.get("seq").and_then(|s| s.as_i64()))
        .unwrap_or(-1)
}

/// Alias public pour la lecture du dernier seq.
pub fn session_seq(line: &str) -> i64 {
    event_seq(line)
}

/// Vrai quand la ligne est l'enregistrement d'en-tête d'une session
/// (`{"type":"session",…}`) — le SEUL type d'événement qui ne porte pas de
/// `seq`, donc le seul que le filtre par seq de l'appelant ne peut pas
/// dédupliquer (voir `append_session_journal`).
pub fn est_entete_de_session(line: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|v| {
            v.get("type")
                .and_then(|t| t.as_str())
                .map(|t| t == "session")
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(seq: i64) -> String {
        format!(r#"{{"type":"event","seq":{seq},"time":0,"data":{{"n":{seq}}}}}"#)
    }

    #[test]
    fn lecture_ordonnee_par_seq_meme_si_appends_desordonnes() {
        // P4b — l'invariant de sortie (« events triés par seq », contrat
        // session_load/readRecent) tient même quand l'ordre physique des
        // frames diverge de l'ordre des seq (respawn / retry EPIPE) : la
        // lecture trie la fenêtre décodée par seq au lieu de se fier à
        // l'ordre physique. Cas nominal (physique == seq) : no-op.
        //
        // LIMITE DOCUMENTÉE : la SÉLECTION de la fenêtre (collecte à rebours
        // bornée par `limit`) reste physique — sous désordre profond, les
        // lignes sélectionnées peuvent ne pas être les plus récentes par seq
        // (le harness re-trie et re-filtre de son côté ; corriger la
        // sélection exigerait de décoder toute la zone, coût évité par
        // conception). Ce correctif garantit l'ORDRE de ce qui est rendu.
        let store = Arc::new(SubstrateStore::open_tempfile().unwrap());
        let sid = "session-ordre-test";

        // Nominal 1..5 puis retry désordonné (3, 2) en queue.
        write_session_journal(&store, sid, &[line(1), line(2), line(3), line(4), line(5)]).unwrap();
        append_session_journal(&store, sid, &[line(3)]).unwrap();
        append_session_journal(&store, sid, &[line(2)]).unwrap();

        // Lecture complète : ordre croissant par seq garanti (doublons du
        // retry présents — la déduplication reste au client, comme avant).
        let all = read_session_journal(&store, sid, -1, 0, 0);
        let seqs: Vec<i64> = all.iter().map(|l| event_seq(l)).collect();
        let mut trie = seqs.clone();
        trie.sort_unstable();
        assert_eq!(seqs, trie, "sortie triée par seq: {seqs:?}");
        assert!(
            all.len() >= 5,
            "toutes les lignes rendues (doublons inclus): {}",
            all.len()
        );

        // Pagination after_seq=2 : ordre trié garanti, tous > 2.
        let after = read_session_journal(&store, sid, 2, 0, 0);
        let seqs_a: Vec<i64> = after.iter().map(|l| event_seq(l)).collect();
        let mut a_trie = seqs_a.clone();
        a_trie.sort_unstable();
        assert_eq!(seqs_a, a_trie, "after_seq trié: {seqs_a:?}");
        assert!(seqs_a.iter().all(|&s| s > 2), "tous > 2: {seqs_a:?}");

        // Ordre physique déjà correct (cas nominal) : le tri est un no-op.
        let sid2 = "session-ordre-nominal";
        write_session_journal(&store, sid2, &[line(1), line(2), line(3)]).unwrap();
        let nom = read_session_journal(&store, sid2, -1, 0, 0);
        let seqs_n: Vec<i64> = nom.iter().map(|l| event_seq(l)).collect();
        assert_eq!(seqs_n, vec![1, 2, 3], "cas nominal inchangé");
    }

    #[test]
    fn lecture_sequence_basique() {
        let store = Arc::new(SubstrateStore::open_tempfile().unwrap());
        let sid = "session-seq-basic";
        write_session_journal(&store, sid, &[line(1), line(2), line(3)]).unwrap();
        let all = read_session_journal(&store, sid, -1, 0, 0);
        assert_eq!(all.len(), 3);
        assert_eq!(event_seq(&all[0]), 1);
        assert_eq!(event_seq(&all[2]), 3);
        // Session inconnue → vide, sans erreur.
        assert!(read_session_journal(&store, "session-inconnue", -1, 0, 0).is_empty());
    }

    /// Corrompt la zone d'une session en zéroïsant une plage d'octets au
    /// milieu des frames (simulation d'un append interrompu / crash ayant
    /// laissé un trou binaire). Retourne la plage corrompue.
    fn zerroise_plage(store: &SubstrateStore, sid: &str, debut: usize, longueur: usize) {
        let sub = store.writer().substrate();
        let guard = sub.lock();
        let mut zf = guard.open_named_zone(&zone_filename(sid)).unwrap();
        let slice = zf.as_slice_mut();
        for i in debut..(debut + longueur).min(slice.len()) {
            slice[i] = 0;
        }
        zf.msync().unwrap();
    }

    #[test]
    fn lecture_resiliente_frame_corrompue_au_milieu() {
        // Régression 2026-09-08 : une frame corrompue au MILIEU de la zone
        // (octets zéroïsés) faisait s'arrêter le scan séquentiel → la queue
        // du journal (le « delta » des derniers messages) devenait invisible
        // pour session_load events_only → le resume repartait d'un point
        // antérieur. La lecture doit se resynchroniser sur la prochaine
        // frame valide et rendre TOUT le journal.
        let store = Arc::new(SubstrateStore::open_tempfile().unwrap());
        let sid = "session-resync-milieu";

        // 200 lignes : seq 0..=199.
        let lignes: Vec<String> = (0..200).map(line).collect();
        write_session_journal(&store, sid, &lignes).unwrap();

        // Repère le début de la frame du seq 50 et zéroïse ~64 octets à
        // partir de là : la frame 50 et ses voisines immédiates sont
        // détruites, mais la suite (seq ~55..199) doit rester lisible.
        let all_avant = read_session_journal(&store, sid, -1, 0, 0);
        assert_eq!(all_avant.len(), 200, "zone saine : 200 lignes");
        // Trouve l'offset de début de la frame seq 50 (offset physique).
        let name = zone_filename(sid);
        let sub = store.writer().substrate();
        let guard = sub.lock();
        let zf = guard.open_named_zone_read_only(&name).unwrap();
        let bytes = zf.as_slice();
        let used = u64::from_le_bytes(bytes[12..20].try_into().unwrap()) as usize;
        let mut off = HEADER_LEN;
        let mut debut_seq50 = None;
        let mut seq_courante = 0i64;
        while off + 4 <= used {
            let len = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
            let payload = off + 4;
            if len == 0 || payload + len > used {
                break;
            }
            let Ok((ligne, _)) = bincode::serde::decode_from_slice::<String, _>(
                &bytes[payload..payload + len],
                bincode::config::standard(),
            ) else {
                break;
            };
            seq_courante = event_seq(&ligne);
            if seq_courante == 50 {
                debut_seq50 = Some(payload);
                break;
            }
            off = payload + len;
        }
        drop(zf);
        drop(guard);
        let debut_seq50 = debut_seq50.expect("frame seq 50 localisée");
        zerroise_plage(&store, sid, debut_seq50, 64);

        // Lecture complète après corruption : les seq 0..49 et ~55..199
        // doivent être rendus (le trou 50..~54 est irrécupérable mais ne
        // doit PAS amputer la queue).
        let apres = read_session_journal(&store, sid, -1, 0, 0);
        let seqs: Vec<i64> = apres.iter().map(|l| event_seq(l)).collect();
        assert!(
            seqs.len() >= 190,
            "queue récupérée après corruption: {} lignes",
            seqs.len()
        );
        assert!(
            seqs.contains(&0) && seqs.contains(&49),
            "début intact: {:?}",
            &seqs[..3.min(seqs.len())]
        );
        assert!(
            seqs.contains(&199),
            "dernière frame rendue (queue non amputée): max={:?}",
            seqs.last()
        );
        // L'ordre reste trié par seq.
        let mut trie = seqs.clone();
        trie.sort_unstable();
        assert_eq!(seqs, trie, "sortie triée malgré la resync: {seqs:?}");
    }

    /// Toutes les lignes BRUTES de la zone, doublons et en-tête INCLUS.
    ///
    /// `read_session_journal` filtre par `seq` (c'est son contrat : `after_seq`)
    /// et rend donc `-1` pour l'en-tête, qui n'en porte pas : la duplication de
    /// l'en-tête est invisible à la lecture normale. Elle vit dans la ZONE, et
    /// c'est là qu'il faut la compter — le test qui observait via
    /// `read_session_journal` voyait 0 copie et ne prouvait rien.
    fn lignes_brutes(store: &SubstrateStore, sid: &str) -> Vec<String> {
        let name = zone_filename(sid);
        let sub = store.writer().substrate();
        let guard = sub.lock();
        let zf = guard.open_named_zone_read_only(&name).unwrap();
        let bytes = zf.as_slice();
        let used = u64::from_le_bytes(bytes[12..20].try_into().unwrap()) as usize;
        let mut out = Vec::new();
        let mut off = HEADER_LEN;
        while off + 4 <= used {
            let len = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
            let payload = off + 4;
            if len == 0 || payload + len > used {
                break;
            }
            match bincode::serde::decode_from_slice::<String, _>(
                &bytes[payload..payload + len],
                bincode::config::standard(),
            ) {
                Ok((ligne, _)) => out.push(ligne),
                Err(_) => break,
            }
            off = payload + len;
        }
        out
    }

    #[test]
    fn en_tete_de_session_ecrit_une_seule_fois() {
        /* RÉGRESSION 2026-10-16 — 2096 copies d'un en-tête dans une seule zone.
         *
         * L'en-tête (`{"type":"session",…}`) ne porte pas de `seq` : le filtre
         * `seq > last_imported_seq` que l'appelant applique désormais aux autres
         * lignes ne peut donc pas le dédupliquer. Sans cette règle, chaque
         * rejeu de lot (timeout de bridge, passe de réparation) ajoutait une
         * copie de plus — mesuré sur runtime-dev : 1 copie avant le
         * 2026-10-06 07:50, puis 155 (12/10), 322 (15/10), 2096 (16/10),
         * 85 124 en quatre jours, sur des zones de 149 Mo et 201 Mo.
         */
        let store = Arc::new(SubstrateStore::open_tempfile().unwrap());
        let sid = "session-entete-une-fois";
        let entete = format!(
            r#"{{"createdAt":1789000000000,"cwd":"/tmp","id":"{sid}","type":"session","version":0}}"#
        );
        assert!(
            est_entete_de_session(&entete),
            "la ligne de test EST un en-tête"
        );
        assert_eq!(event_seq(&entete), -1, "un en-tête ne porte pas de seq");

        // Premier envoi : l'en-tête est écrit (la zone est vide).
        append_session_journal(&store, sid, &[entete.clone(), line(0)]).unwrap();
        // Rejeu du MÊME lot — le cas du timeout de bridge.
        append_session_journal(&store, sid, &[entete.clone(), line(0)]).unwrap();
        // Et l'envoi d'en-tête SEUL — le cas de la passe de réparation.
        append_session_journal(&store, sid, &[entete.clone()]).unwrap();

        let brut = lignes_brutes(&store, sid);
        let entetes = brut.iter().filter(|l| est_entete_de_session(l)).count();
        assert_eq!(entetes, 1, "en-tête dupliqué ({entetes} copies): {brut:?}");
        // Les lignes AVEC seq restent, elles : leur déduplication est celle de
        // l'appelant (`seq > last_imported_seq`), pas celle de la zone.
        assert_eq!(
            brut.iter().filter(|l| event_seq(l) == 0).count(),
            2,
            "les événements sont écrits, doublons compris: {brut:?}",
        );
    }

    #[test]
    fn en_tete_ecrit_quand_la_zone_etait_vide() {
        // Le pendant de la règle : sur une zone VIERGE, l'en-tête doit passer —
        // l'écarter priverait `enteteDeJournal` de son `createdAt`, donc la
        // session d'un nœud exploitable (le husk que la réparation combat).
        let store = Arc::new(SubstrateStore::open_tempfile().unwrap());
        let sid = "session-entete-zone-vide";
        let entete = format!(
            r#"{{"createdAt":1789000000000,"cwd":"/tmp","id":"{sid}","type":"session","version":0}}"#
        );
        append_session_journal(&store, sid, &[entete.clone()]).unwrap();
        let brut = lignes_brutes(&store, sid);
        assert_eq!(
            brut.len(),
            1,
            "l'en-tête d'une zone vierge est écrit: {brut:?}"
        );
        assert!(est_entete_de_session(&brut[0]));
    }
}
