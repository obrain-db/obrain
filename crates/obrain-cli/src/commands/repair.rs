//! `obrain repair` — diagnostic et réparation d'une repermutation de
//! slots interrompue (chantier SlotRemappable).
//!
//! ══ POURQUOI CETTE COMMANDE EXISTE ══
//!
//! Quand une repermutation est ANNONCÉE et jamais CONFIRMÉE, le substrat
//! refuse d'ouvrir la base : l'état des zones nœuds/arêtes est
//! indéterminé, et servir des requêtes dessus renverrait des réponses
//! fausses avec des CRC valides.
//!
//! Ce refus était juste, mais il laissait l'opérateur sans instrument.
//! La reprise se faisait par une variable d'environnement qui effaçait
//! l'annonce **à l'aveugle** — et la revue a prouvé que cela CORROMPT
//! quand la barrière de durabilité avait été franchie : le rejeu tombe
//! alors dans son cas 3 et ré-applique la permutation sur des zones déjà
//! permutées. La protection fonctionnait, l'alarme sonnait, l'opérateur
//! suivait la consigne écrite, et c'est la consigne qui détruisait.
//!
//! ══ UN SEUL DIAGNOSTIC, DEUX USAGES ══
//!
//! `--dry-run` (défaut) affiche `slot_remap::diagnostiquer` ; `--apply`
//! appelle `slot_remap::appliquer_reparation`, qui **rappelle elle-même**
//! `diagnostiquer` et rend le diagnostic utilisé. Il n'y a donc pas deux
//! règles de décision qui pourraient diverger — c'est la condition qui
//! décide de la valeur de tout ce dispositif.

use std::path::Path;

use anyhow::{Result, bail};
use serde::Serialize;

use crate::OutputFormat;
use crate::output;

use obrain_substrate::file::SubstrateFile;
use obrain_substrate::slot_remap::{
    ActionReparation, DiagnosticRemap, Recommandation, VerdictOperateur, appliquer_reparation,
    consigne_diagnostic, consigne_reparation, diagnostiquer, recommander,
};

/// Verdict que l'opérateur atteste avec `--apply`.
#[derive(Copy, Clone, Debug, clap::ValueEnum)]
pub enum Verdict {
    /// Les zones n'ont PAS été permutées — l'annonce sera effacée.
    ZonesIntactes,
    /// Les zones SONT permutées et durables — l'epoch sera confirmée.
    ZonesPermutees,
}

impl From<Verdict> for VerdictOperateur {
    fn from(v: Verdict) -> Self {
        match v {
            Verdict::ZonesIntactes => VerdictOperateur::ZonesIntactes,
            Verdict::ZonesPermutees => VerdictOperateur::ZonesPermutees,
        }
    }
}

#[derive(Serialize)]
struct SortieReparation {
    dry_run: bool,
    etat: &'static str,
    epoch_annoncee: Option<u32>,
    epoch_confirmee: Option<u32>,
    raison: Option<String>,
    record_dans_le_wal: Option<bool>,
    sidecars: Vec<(String, u32)>,
    debris_anterieurs: Vec<(String, Option<u32>, u64)>,
    recommandation: String,
    action: Option<String>,
}

fn etiquette(d: &DiagnosticRemap) -> &'static str {
    match d {
        DiagnosticRemap::Saine { .. } => "saine",
        DiagnosticRemap::OracleIllisible { .. } => "oracle_illisible",
        DiagnosticRemap::EtatContradictoire { .. } => "etat_contradictoire",
        DiagnosticRemap::RepermutationInterrompue { .. } => "repermutation_interrompue",
    }
}

fn sortie(
    d: &DiagnosticRemap,
    dry_run: bool,
    action: Option<ActionReparation>,
) -> SortieReparation {
    let mut s = SortieReparation {
        dry_run,
        etat: etiquette(d),
        epoch_annoncee: None,
        epoch_confirmee: None,
        raison: None,
        record_dans_le_wal: None,
        sidecars: Vec::new(),
        debris_anterieurs: Vec::new(),
        recommandation: String::new(),
        action: action.map(|a| format!("{a:?}")),
    };
    match d {
        DiagnosticRemap::Saine { committed } => s.epoch_confirmee = Some(*committed),
        DiagnosticRemap::OracleIllisible { raison } => s.raison = Some(raison.clone()),
        DiagnosticRemap::EtatContradictoire {
            epoch,
            committed,
            details,
        } => {
            s.epoch_annoncee = Some(*epoch);
            s.epoch_confirmee = Some(*committed);
            s.raison = Some(details.clone());
        }
        DiagnosticRemap::RepermutationInterrompue {
            epoch,
            committed,
            observations,
        } => {
            s.epoch_annoncee = Some(*epoch);
            s.epoch_confirmee = Some(*committed);
            s.record_dans_le_wal = Some(observations.record_dans_le_wal);
            s.sidecars.clone_from(&observations.sidecars);
            s.debris_anterieurs = observations
                .temoins_anterieurs
                .iter()
                .map(|(n, e, o)| (n.clone(), *e, *o))
                .collect();
        }
    }
    s.recommandation = format!("{:?}", recommander(d));
    s
}

/// Rend le diagnostic en clair — c'est CE texte que lira l'opérateur au
/// moment où il doit décider.
///
/// ══ AUCUNE RÈGLE NE VIT ICI (revue #4) ══
///
/// Cette fonction MET EN FORME `slot_remap::recommander`. Elle ne décide
/// rien. La version précédente calculait ici un « INDICE DÉCISIF » à
/// partir des témoins — une seconde règle de décision, hors du
/// diagnostic partagé et sans un seul test. Elle était fausse : un
/// `.remap.tmp` hérité d'une passe précédente lui faisait conclure « la
/// barrière a été franchie » et recommander le verdict qui corrompt.
fn raconter(d: &DiagnosticRemap, chemin: &Path, quiet: bool) {
    match d {
        DiagnosticRemap::Saine { committed } => {
            output::status(
                &format!(
                    "Base saine : aucune repermutation en suspens (epoch confirmee = {committed}). \
                     Rien a reparer."
                ),
                quiet,
            );
        }
        DiagnosticRemap::OracleIllisible { raison } => {
            output::status(
                &format!(
                    "REFUS DE DIAGNOSTIQUER — la zone d'epoch (l'oracle dont depend toute la \
                     protection) est PRESENTE mais illisible : {raison}.\n\
                     Rendre un diagnostic ici serait pire que n'en rendre aucun, parce qu'il en \
                     porterait l'autorite. Restaurez `substrate.remap.epoch` depuis une \
                     sauvegarde, ou reconstruisez la base. `--apply` refusera egalement."
                ),
                quiet,
            );
        }
        DiagnosticRemap::EtatContradictoire {
            epoch,
            committed,
            details,
        } => {
            output::status(
                &format!(
                    "REFUS DE DIAGNOSTIQUER — l'etat durable se CONTREDIT (epoch annoncee \
                     {epoch}, confirmee {committed}).\n{details}\n\
                     Cet etat est impossible a produire par le code : soit les fichiers ont ete \
                     manipules a la main, soit une invariante du module a ete cassee. Aucune \
                     reparation automatique n'est proposee. `--apply` refusera egalement."
                ),
                quiet,
            );
        }
        DiagnosticRemap::RepermutationInterrompue {
            epoch,
            committed,
            observations,
        } => {
            use std::fmt::Write as _;
            let mut texte = format!(
                "Repermutation INTERROMPUE : epoch {epoch} annoncee, epoch confirmee {committed}.\n\
                 \n\
                 CE QUI EST OBSERVABLE :\n\
                 - record de repermutation encore dans le WAL : {}\n\
                 - side-cars estampilles : {}\n\
                 - debris de passes PRECEDENTES (temoins non dates ou d'une epoch anterieure) : {}\n",
                if observations.record_dans_le_wal {
                    "oui (un rejeu saurait appliquer la permutation)"
                } else {
                    "non (seule la zone d'epoch temoigne)"
                },
                if observations.sidecars.is_empty() {
                    "aucun".to_string()
                } else {
                    observations
                        .sidecars
                        .iter()
                        .map(|(n, e)| format!("{n}=epoch{e}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                },
                if observations.temoins_anterieurs.is_empty() {
                    "aucun".to_string()
                } else {
                    observations
                        .temoins_anterieurs
                        .iter()
                        .map(|(n, e, o)| match e {
                            Some(e) => format!("{n} (epoch {e}, {o} o)"),
                            None => format!("{n} (non date, {o} o)"),
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                },
            );

            // ── LA RECOMMANDATION VIENT DU MODULE PARTAGE ──
            match recommander(d) {
                Recommandation::AucuneReprisSureSansJournal { epoch } => {
                    let _ = write!(
                        texte,
                        "\nAUCUNE REPRISE N'EST SURE DEPUIS L'ETAT DURABLE SEUL, et ce n'est pas \
                         un manque d'effort : la SEULE chose qui prouverait que la barriere de \
                         durabilite a ete franchie est la confirmation elle-meme — c'est \
                         precisement ce qui manque. Tout ce qui se trouve en aval d'elle \
                         (side-cars estampilles, temoins de phase 9) est en aval de ce qui \
                         manque. Les debris listes ci-dessus datent de passes PRECEDENTES : ils \
                         ne disent rien de l'epoch {epoch}.\n\
                         \n\
                         LA SEULE SOURCE QUI TRANCHE est le journal du writer au moment de \
                         l'echec. Cherchez-y, pour cette base :\n\
                         - « cote_barriere=APRES ... la confirmation a echoue deux fois »\n\
                               -> les zones SONT permutees : --verdict zones-permutees\n\
                         - « repermutation abandonnee avant toute mutation, mais l'EFFACEMENT de \
                         l'annonce a echoue »\n\
                               -> les zones sont INTACTES : --verdict zones-intactes\n\
                         - « cote_barriere=AVANT »\n\
                               -> etat INDETERMINE : AUCUNE reprise n'est sure.\n\
                         \n\
                         SI LE JOURNAL NE DIT RIEN (arret brutal : coupure, OOM-kill, \
                         redemarrage de conteneur), les deux cotes restent possibles et la \
                         seule reponse sure est de RESTAURER UNE SAUVEGARDE. Ne devinez pas : \
                         un verdict faux applique ici corrompt definitivement.\n\
                         \n\
                         COMMANDE, seulement apres avoir trouve la ligne de journal :\n  \
                         {}\n",
                        consigne_reparation(chemin, "<zones-intactes|zones-permutees>")
                    );
                }
                autre => {
                    let _ = write!(texte, "\nRecommandation : {autre:?}\n");
                }
            }
            output::status(&texte, quiet);
        }
    }
}

/// Point d'entrée de `obrain repair`.
pub fn run(
    path: &Path,
    apply: bool,
    verdict: Option<Verdict>,
    format: OutputFormat,
    quiet: bool,
) -> Result<()> {
    // On ouvre le FICHIER substrat, jamais le store : le store refuse
    // precisement les bases que cette commande existe pour reparer.
    let sub = SubstrateFile::open(path)?;

    if !apply {
        let diag = diagnostiquer(&sub);
        output::status("Dry run : aucune modification ne sera faite.", quiet);
        raconter(&diag, path, quiet);
        if matches!(format, OutputFormat::Json) && !quiet {
            println!(
                "{}",
                serde_json::to_string_pretty(&sortie(&diag, true, None))?
            );
        }
        return Ok(());
    }

    let Some(verdict) = verdict else {
        bail!(
            "--apply exige --verdict <zones-intactes|zones-permutees>. Les deux valeurs \
             commandent des actions OPPOSEES : se tromper de verdict corrompt la base. \
             Lancez d'abord `{}`.",
            consigne_diagnostic(path)
        );
    };

    // MEME diagnostic que le dry-run — `appliquer_reparation` le rappelle
    // elle-meme et rend celui qu'elle a utilise.
    let (diag, action) = appliquer_reparation(&sub, verdict.into())?;
    raconter(&diag, path, quiet);
    match action {
        ActionReparation::RienAFaire => {
            output::status(
                "Rien a faire : la base est saine, aucune action appliquee.",
                quiet,
            );
        }
        ActionReparation::AnnonceEffacee { epoch } => {
            // (Revue #5) La suite depend de ce que le diagnostic vient
            // d'observer : promettre inconditionnellement « un rejeu
            // appliquera la permutation » est faux quand le record est
            // sorti de la fenetre de rejeu.
            let record_present = matches!(
                &diag,
                DiagnosticRemap::RepermutationInterrompue { observations, .. }
                    if observations.record_dans_le_wal
            );
            let suite = if record_present {
                "le record est encore dans la fenetre de rejeu : la permutation sera appliquee \
                 a la prochaine ouverture"
            } else {
                "le record N'EST PLUS dans la fenetre de rejeu : la permutation ne sera PAS \
                 rejouee, et la base restera dans son etat pre-permutation (ce qui est \
                 coherent, mais le tri devra etre relance)"
            };
            output::status(
                &format!(
                    "APPLIQUE : annonce de l'epoch {epoch} effacee. La base doit desormais \
                     s'ouvrir ; {suite}."
                ),
                quiet,
            );
        }
        ActionReparation::EpochConfirmee { epoch } => {
            output::status(
                &format!(
                    "APPLIQUE : epoch {epoch} confirmee. La base doit desormais s'ouvrir, sans \
                     repermutation ; le rejeu rattrapera les side-cars."
                ),
                quiet,
            );
        }
    }
    if matches!(format, OutputFormat::Json) && !quiet {
        println!(
            "{}",
            serde_json::to_string_pretty(&sortie(&diag, false, Some(action)))?
        );
    }
    Ok(())
}

/// `obrain repair-degrees <path>` — reconstruit les colonnes de degrés
/// (générique + typées) depuis un scan des arêtes.
///
/// Remède aux sidecars périmés : migrés par un binaire plus ancien avec
/// un mapping de slots différent, ils passent la validation d'ouverture
/// (CRC valide) mais portent des compteurs faux → le pré-filtre par
/// degré typé saute tous les nœuds et les expansions sortantes typées
/// renvoient 0 (incident prod obrain-harness 2026-09-01 :
/// `sessions_list`/`session_load` du harness vides).
///
/// Idempotent. Nécessite un accès EXCLUSIF au store (service arrêté) —
/// le verrou est refusé si un autre process tient la base. Après
/// l'opération, redémarrer le service : le prochain open trouve des
/// colonnes fraîches et l'expansion typée redevient correcte.
pub fn run_degrees(path: &Path, format: OutputFormat, quiet: bool) -> Result<()> {
    // ── ON NE SUPPRIME PLUS LES SIDECARS ────────────────────────────────────
    // Cette commande les effaçait pour « forcer » une reconstruction paresseuse.
    // Mesuré le 2026-09-12 sur une copie : le fichier de la colonne générique
    // restait ABSENT après la passe, une seconde passe ne le recréait pas, et
    // seule une NOUVELLE ouverture le recréait. La suppression ne forçait rien —
    // elle détruisait, en supprimant le fichier sous une zone déjà mmapée (l'
    // inode survit dans le processus, les écritures partent dedans, le chemin
    // reste vide).
    //
    // La reconstruction se fait désormais EN PLACE : remise à zéro puis un seul
    // rescan des arêtes, sur la colonne existante, persistée. Aucune suppression,
    // donc aucun inode orphelin, donc aucune perte.
    if !path.is_dir() {
        bail!("repair-degrees: base introuvable: {}", path.display());
    }
    // ── LA BASE EST-ELLE EN COURS D'ÉCRITURE ? ──────────────────────────────
    // La reconstruction EN PLACE n'est PAS atomique vis-à-vis d'un écrivain
    // concurrent : elle remet à zéro puis rescanne, et une arête créée entre les
    // deux est comptée deux fois. C'est l'explication mesurée du seul cas de
    // corruption observé — `runtime-dev`, la seule base écrite EN CONTINU par le
    // harnais, a vu sa somme de degrés passer à 45 fois le nombre réel d'arêtes
    // quand ce chemin tournait automatiquement. Sur une copie immobile, trois
    // passes donnent exactement le même résultat : c'est la concurrence, pas
    // l'algorithme.
    //
    // On ne refuse pas — l'opérateur sait ce qu'il fait — mais on le DIT, avec le
    // chiffre : un avertissement muet ne sert à rien.
    let maintenant = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(entrees) = std::fs::read_dir(path) {
        let mut ecrit_il_y_a: Option<u64> = None;
        for e in entrees.flatten() {
            if let Ok(m) = e.metadata()
                && let Ok(t) = m.modified()
                && let Ok(secs) = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs())
                && secs <= maintenant
            {
                let age = maintenant - secs;
                ecrit_il_y_a = Some(ecrit_il_y_a.map_or(age, |a: u64| a.min(age)));
            }
        }
        if let Some(age) = ecrit_il_y_a
            && age < 300
        {
            output::status(
                &format!(
                    "repair-degrees : ATTENTION — cette base a été écrite il y a {age} s. La \
                     reconstruction EN PLACE n'est pas atomique face à un écrivain concurrent \
                     (une arête créée pendant le zeroing/rescan serait comptée deux fois). \
                     Arrêtez d'abord tout écrivain — le service, les ponts MCP."
                ),
                quiet,
            );
        }
    }
    let db = super::open_existing(path)?;
    let rebuilt = db
        .rebuild_degree_columns()
        .map_err(|e| anyhow::anyhow!("repair-degrees: {e}"))?;
    let _ = rebuilt;
    let statut = "repair-degrees : colonnes de degrés RECONSTRUITES EN PLACE (remise à zéro puis \
                  un seul rescan des arêtes) — aucun fichier supprimé."
        .to_string();
    output::status(&statut, quiet);
    if matches!(format, OutputFormat::Json) && !quiet {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": "ok",
                "methode": "en_place",
                "fichiers_supprimes": 0,
                "path": path.display().to_string(),
            }))?
        );
    }
    Ok(())
}

// ===========================================================================
// `obrain repair-props` — references heap pendantes de la zone props v2
// ===========================================================================

/// Echantillon borne d'entrees pendantes : de quoi aller VERIFIER sur la
/// base, pas de quoi la reconstituer.
#[derive(Serialize)]
struct EchantillonPendante {
    owner_kind: &'static str,
    owner_id: u64,
    prop_key: u16,
    prop_key_name: Option<String>,
    page_id: u32,
    offset: u32,
    /// L'entree est-elle la valeur COURANTE de sa cle ? Seules celles-la
    /// sont retirees par `--apply`.
    current: bool,
}

#[derive(Serialize)]
struct SortieRepairProps {
    dry_run: bool,
    path: String,
    chains_scanned: u64,
    entries_scanned: u64,
    heap_backed_entries: u64,
    dangling_entries: u64,
    dangling_entries_current: u64,
    dangling_without_key_name: u64,
    nodes_affected: usize,
    edges_affected: usize,
    heap_live_pages: u32,
    heap_pages_crc_checked: u32,
    heap_pages_crc_failed: u32,
    removable_entries: usize,
    removed_entries: usize,
    sample: Vec<EchantillonPendante>,
}

const ECHANTILLON_MAX: usize = 20;

fn sortie_props(
    rapport: &obrain_substrate::repair_props::RapportRefsPendantes,
    path: &Path,
    dry_run: bool,
    removed: usize,
) -> SortieRepairProps {
    SortieRepairProps {
        dry_run,
        path: path.display().to_string(),
        chains_scanned: rapport.chaines_scannees,
        entries_scanned: rapport.entrees_scannees,
        heap_backed_entries: rapport.entrees_heap,
        dangling_entries: rapport.entrees_pendantes,
        dangling_entries_current: rapport.entrees_pendantes_courantes,
        dangling_without_key_name: rapport.pendantes_sans_nom_de_cle,
        nodes_affected: rapport.noeuds_touches,
        edges_affected: rapport.aretes_touchees,
        heap_live_pages: rapport.heap_pages_vivantes,
        heap_pages_crc_checked: rapport.heap_pages_crc_examinees,
        heap_pages_crc_failed: rapport.heap_pages_crc_invalide,
        removable_entries: rapport.cibles.len(),
        removed_entries: removed,
        sample: rapport
            .echantillon
            .iter()
            .map(|p| EchantillonPendante {
                owner_kind: p.genre.etiquette(),
                owner_id: p.proprietaire,
                prop_key: p.cle,
                prop_key_name: p.nom_cle.clone(),
                page_id: p.page_id,
                offset: p.offset,
                current: p.courante,
            })
            .collect(),
    }
}

/// Rend le rapport en clair. Les nombres sont donnes BRUTS : c'est
/// l'operateur qui decide, et il ne peut decider que sur ce qu'il voit.
fn raconter_props(s: &SortieRepairProps, quiet: bool) {
    use std::fmt::Write as _;
    let mut texte = format!(
        "Zone props v2 de {} :\n\
         - chaines de proprietes parcourues : {}\n\
         - entrees vivantes scannees : {}\n\
         - entrees adossees au heap : {}\n\
         - entrees PENDANTES (le heap ne resout pas) : {}\n\
         -   dont valeur COURANTE de leur cle (retirables) : {}\n\
         -   dont cle absente du registre (NON retirables) : {}\n\
         - proprietaires touches : {} nœud(s), {} arete(s)\n\
         - extension vivante du heap : {} page(s) de 4 KiB\n\
         - pages heap dont le CRC32 ECHOUE : {} sur {} verifiee(s)\n",
        s.path,
        s.chains_scanned,
        s.entries_scanned,
        s.heap_backed_entries,
        s.dangling_entries,
        s.dangling_entries_current,
        s.dangling_without_key_name,
        s.nodes_affected,
        s.edges_affected,
        s.heap_live_pages,
        s.heap_pages_crc_failed,
        s.heap_pages_crc_checked,
    );

    if s.heap_pages_crc_failed > 0 {
        let _ = write!(
            texte,
            "\nATTENTION — {} page(s) heap ont un CRC32 FAUX. Elles ne sont PAS \
             traitees comme pendantes et cette commande n'y touche pas : leurs \
             octets se resolvent, ils sont simplement peut-etre faux. Les \
             retirer detruirait des donnees eventuellement recuperables. \
             C'est un signalement, pas une action.\n",
            s.heap_pages_crc_failed
        );
    }

    let ensevelies = s.dangling_entries - s.dangling_entries_current;
    if ensevelies > 0 {
        let _ = write!(
            texte,
            "\n{ensevelies} entree(s) pendante(s) sont ENSEVELIES sous une entree \
             plus recente de la meme cle (reecriture saine, ou tombstone deja \
             posee) : plus personne ne les lit. `--apply` ne les touche pas — \
             la seule primitive de retrait sure vise une CLE, pas une entree, \
             et l'employer ici detruirait la valeur saine qui les ensevelit. \
             C'est du gaspillage d'espace, pas une perte de donnees.\n"
        );
    }

    if s.sample.is_empty() {
        let _ = write!(texte, "\nAucune reference pendante trouvee.\n");
    } else {
        let _ = write!(
            texte,
            "\nEchantillon ({} sur {} entree(s) pendante(s), valeurs courantes \
             d'abord) :\n",
            s.sample.len(),
            s.dangling_entries
        );
        for p in &s.sample {
            let _ = write!(
                texte,
                "  {} {} cle {} ({}) -> heap page {} offset {}{}\n",
                p.owner_kind,
                p.owner_id,
                p.prop_key,
                p.prop_key_name.as_deref().unwrap_or("<cle inconnue>"),
                p.page_id,
                p.offset,
                if p.current { "" } else { " [ensevelie]" }
            );
        }
    }

    if s.dry_run {
        let _ = write!(
            texte,
            "\nDry run : RIEN n'a ete modifie. {} entree(s) seraient retirees \
             par `--apply`.\n",
            s.removable_entries
        );
    } else {
        let _ = write!(
            texte,
            "\nAPPLIQUE : {} entree(s) retiree(s) (tombstone posee sur la cle), \
             puis flush. Relancez la commande sans `--apply` pour verifier : \
             c'est le re-scan qui fait foi, pas ce compteur.\n",
            s.removed_entries
        );
    }
    output::status(&texte, quiet);
}

/// `obrain repair-props <store>` — trouve les `HeapRef` PENDANTS de la zone
/// props v2 et, avec `--apply`, retire les entrees concernees.
///
/// ══ POURQUOI CETTE COMMANDE EXISTE ══
///
/// Une entree de propriete `StringRef` / `BytesRef` / `ArrStringRef` ne
/// porte pas ses octets : elle porte un `HeapRef` vers la zone heap, un
/// AUTRE fichier. Une version anterieure rendait durable le POINTEUR avant
/// le POINTE ; un arret brutal dans cette fenetre laissait sur disque des
/// entrees qui referencent des octets heap qui n'ont jamais existe. La
/// lecture rend alors « pas de valeur » sur une propriete qui, de l'avis de
/// la zone props, existe bel et bien.
///
/// L'ordre de flush a ete corrige (le heap d'abord), et la borne de lecture
/// aussi (`free_offset` au lieu de `payload.len()`). Aucun des deux ne
/// repare ce qui est DEJA sur disque : c'est le role de cette commande.
///
/// ══ LE DRY-RUN EST LE DEFAUT ══
///
/// Sans `--apply`, la base est ouverte en LECTURE SEULE et rien n'est
/// ecrit. C'est le mode dans lequel on lance cette commande sur une base de
/// production : elle dit ce qu'elle trouverait, et l'operateur decide.
pub fn run_props(path: &Path, apply: bool, format: OutputFormat, quiet: bool) -> Result<()> {
    use obrain_substrate::repair_props;

    if !apply {
        // LECTURE SEULE : la garantie « ce dry-run ne peut rien abimer » ne
        // repose pas sur la discipline du code ci-dessous mais sur le mode
        // d'ouverture lui-meme.
        let store = obrain_substrate::SubstrateStore::open_read_only(path)
            .map_err(|e| anyhow::anyhow!("repair-props: ouverture de {}: {e}", path.display()))?;
        let rapport = repair_props::scanner(&store, ECHANTILLON_MAX)
            .map_err(|e| anyhow::anyhow!("repair-props: scan: {e}"))?;
        let s = sortie_props(&rapport, path, true, 0);
        raconter_props(&s, quiet);
        if matches!(format, OutputFormat::Json) && !quiet {
            println!("{}", serde_json::to_string_pretty(&s)?);
        }
        return Ok(());
    }

    // ── Chemin mutant ────────────────────────────────────────────────────
    // Passe par `ObrainDB` (donc par le verrou exclusif du store) : le
    // retrait ecrit le WAL, la zone props et les index, et deux ecrivains
    // simultanes sur ces trois etats n'ont aucun sens.
    let db = super::open_existing(path)?;
    let Some(store) = db.substrate_handle() else {
        bail!(
            "repair-props --apply : {} n'est pas une base substrate (aucune zone \
             props v2 a reparer).",
            path.display()
        );
    };
    let rapport = repair_props::scanner(&store, ECHANTILLON_MAX)
        .map_err(|e| anyhow::anyhow!("repair-props: scan: {e}"))?;
    let retirees = repair_props::appliquer(&store, &rapport.cibles)
        .map_err(|e| anyhow::anyhow!("repair-props: retrait: {e}"))?;
    let s = sortie_props(&rapport, path, false, retirees);
    raconter_props(&s, quiet);
    if matches!(format, OutputFormat::Json) && !quiet {
        println!("{}", serde_json::to_string_pretty(&s)?);
    }
    Ok(())
}

/// `obrain repair-edge-chains <path>` — reconstruit les chaines d'adjacence
/// (`next_from` sortant / `next_to` entrant / `first_edge_off`) depuis les
/// `EdgeRecord` VIVANTS, la source de verite.
///
/// POURQUOI. Des ecritures concurrentes d'avant `splice_lock` (incident
/// dedup runtime-dev 2026-09-08) ont rendu CYCLIQUES les chaines d'adjacence
/// de quelques noeuds. La marche d'une chaine cyclique boucle jusqu'a la
/// garde anti-cycle (`next_edge_id + 1`) et re-visite chaque arete des
/// milliers de fois : `MATCH (a)-->(b)` explose (20,9 M) alors que
/// `MATCH ()-[r]->()` (scan des records) reste juste (1,3 M). `repair-degrees`
/// et `compact` n'y remedient pas (l'un recalcule des degres deja justes,
/// l'autre ne touche pas les pointeurs de chaine).
///
/// La reconstruction est un DIFF minimal : elle ne reecrit que les pointeurs
/// qui divergent de l'etat qu'une insertion propre produit (tete = slot le
/// plus haut, ordre decroissant). Une chaine saine reste bit-a-bit
/// identique ; une 2e passe ecrit 0 (idempotent). N'altere NI `src`/`dst`/
/// `edge_type`/poids/props des aretes, NI les degres, NI les compteurs.
///
/// Sans `--apply` : dry-run (diagnostic seul). Requiert un acces EXCLUSIF.
pub fn run_edge_chains(path: &Path, apply: bool, format: OutputFormat, quiet: bool) -> Result<()> {
    if !path.is_dir() {
        bail!("repair-edge-chains: base introuvable: {}", path.display());
    }
    // La reconstruction lit PUIS reecrit : elle n'est PAS atomique face a un
    // ecrivain concurrent. On avertit (comme repair-degrees) sans refuser.
    if apply {
        let maintenant = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if let Ok(entrees) = std::fs::read_dir(path) {
            let mut ecrit_il_y_a: Option<u64> = None;
            for e in entrees.flatten() {
                if let Ok(m) = e.metadata()
                    && let Ok(t) = m.modified()
                    && let Ok(secs) = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs())
                    && secs <= maintenant
                {
                    let age = maintenant - secs;
                    ecrit_il_y_a = Some(ecrit_il_y_a.map_or(age, |a: u64| a.min(age)));
                }
            }
            if let Some(age) = ecrit_il_y_a
                && age < 300
            {
                output::status(
                    &format!(
                        "repair-edge-chains : ATTENTION — cette base a ete ecrite il y a \
                         {age} s. La reconstruction lit puis reecrit et n'est pas atomique \
                         face a un ecrivain concurrent. Arretez d'abord tout ecrivain — le \
                         service, les ponts MCP — ou operez sur une COPIE."
                    ),
                    quiet,
                );
            }
        }
    }

    let db = super::open_existing(path)?;
    let stats = db
        .rebuild_edge_chains(!apply)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let sain =
        stats.next_from_fixed == 0 && stats.next_to_fixed == 0 && stats.first_edge_off_fixed == 0;

    let entete = if !apply {
        "repair-edge-chains (DRY-RUN, aucune ecriture)"
    } else {
        "repair-edge-chains (APPLIQUE)"
    };
    output::status(
        &format!(
            "{entete} : {live} aretes vivantes sur {scanned} slots ; \
             next_from a corriger = {nf}, next_to a corriger = {nt}, \
             first_edge_off a corriger = {fo} ({rew} aretes touchees). {verdict}",
            live = stats.live_edges,
            scanned = stats.edges_scanned,
            nf = stats.next_from_fixed,
            nt = stats.next_to_fixed,
            fo = stats.first_edge_off_fixed,
            rew = stats.edges_rewritten,
            verdict = if sain {
                "Chaines SAINES (rien a reparer)."
            } else if apply {
                "Chaines REPAREES ; relancer la commande doit rendre 0 (idempotence)."
            } else {
                "Chaines CORROMPUES : relancer avec --apply pour reparer."
            },
        ),
        quiet,
    );
    if matches!(format, OutputFormat::Json) && !quiet {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": if sain { "sain" } else if apply { "repare" } else { "corrompu" },
                "dry_run": !apply,
                "edges_scanned": stats.edges_scanned,
                "live_edges": stats.live_edges,
                "nodes_with_out_edges": stats.nodes_with_out_edges,
                "nodes_with_in_edges": stats.nodes_with_in_edges,
                "edges_rewritten": stats.edges_rewritten,
                "next_from_fixed": stats.next_from_fixed,
                "next_to_fixed": stats.next_to_fixed,
                "first_edge_off_fixed": stats.first_edge_off_fixed,
                "path": path.display().to_string(),
            }))?
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;
    use obrain_substrate::slot_remap::COMMANDE_REPARATION;

    /// Le nom que le substrat IMPRIME doit etre celui que le binaire
    /// ACCEPTE.
    ///
    /// ══ LA TROISIEME CONSIGNE MORTE DE LA MEME FAMILLE ══
    ///
    /// Deux avaient deja ete fermees (`OBRAIN_REMAP_REPRISE`,
    /// `ENV_REPRISE_REMAP`). La troisieme etait plus sournoise : le nom
    /// etait correct, mais il n'etait lie a RIEN. `COMMANDE_REPARATION` et
    /// le message d'erreur portaient deux LITTERAUX JUMEAUX, et la
    /// sous-commande clap un troisieme. Renommer l'un des trois laissait
    /// les autres mentir, et le test qui pretendait le couvrir comparait
    /// un litteral a son jumeau — donc ne couvrait rien.
    ///
    /// Ce test-ci lit le nom REEL dans la definition clap. Renommer la
    /// sous-commande, ou renommer la constante, le fait rougir.
    #[test]
    fn le_nom_imprime_est_celui_de_la_sous_commande_reelle() {
        let commande = crate::Cli::command();
        let noms: Vec<String> = commande
            .get_subcommands()
            .map(|s| s.get_name().to_string())
            .collect();
        let attendu = COMMANDE_REPARATION
            .split_whitespace()
            .next_back()
            .expect("COMMANDE_REPARATION ne doit pas etre vide");
        assert!(
            noms.iter().any(|n| n == attendu),
            "`COMMANDE_REPARATION` = {COMMANDE_REPARATION:?} designe la sous-commande \
             {attendu:?}, absente du binaire. Sous-commandes reelles : {noms:?}. \
             Le refus d'ouverture enverrait l'operateur sur `unrecognized subcommand`."
        );
    }

    /// Et le refus du substrat cite bien CETTE constante — pas un jumeau.
    #[test]
    fn le_refus_du_substrat_cite_la_constante_partagee() {
        let err = obrain_substrate::error::SubstrateError::RepermutationIndeterminee {
            epoch: 1,
            base: "/var/lib/obrain/kb".into(),
        };
        let msg = format!("{err}");
        assert!(
            msg.contains(COMMANDE_REPARATION),
            "le refus doit citer {COMMANDE_REPARATION:?}, obtenu : {msg}"
        );
        assert!(
            msg.contains("/var/lib/obrain/kb"),
            "le refus doit nommer LA BASE : {msg}"
        );
    }
}
