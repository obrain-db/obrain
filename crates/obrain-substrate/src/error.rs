//! Error types for the substrate crate.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SubstrateError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid magic — not a substrate file")]
    BadMagic,

    #[error("unsupported format version {0} (expected {1})")]
    UnsupportedVersion(u32, u32),

    #[error("schema CRC mismatch: claimed {claimed:#x}, actual {actual:#x}")]
    SchemaCrcMismatch { claimed: u32, actual: u32 },

    #[error("WAL encode failed: {0}")]
    WalEncode(String),

    #[error("WAL decode failed: {0}")]
    WalDecode(String),

    #[error("WAL short read: needed {needed} bytes, got {got}")]
    WalShortRead { needed: usize, got: usize },

    #[error("WAL frame invalid: {0}")]
    WalBadFrame(String),

    #[error("WAL CRC mismatch: claimed {claimed:#x}, actual {actual:#x}")]
    WalCrcMismatch { claimed: u32, actual: u32 },

    /// ── UN TROU DE ZÉROS N'EST NI UNE FIN NI UNE FRONTIÈRE DE CRASH ───────
    ///
    /// Une plage d'octets NULS suivie de DONNÉES, au milieu du WAL. Le lecteur
    /// s'arrêtait dessus en la confondant avec une fin de fichier (pré-allocation)
    /// ou avec une queue déchirée (frontière de crash) : dans les deux cas le
    /// rejeu s'arrêtait SANS LE DIRE, et tout ce qui suivait le trou était perdu
    /// en silence — indiscernable d'une absence.
    ///
    /// Cette variante existe pour que le fait soit NOMMÉ, avec ses bornes, et
    /// pour qu'aucune réparation ne puisse tronquer à l'aveugle des frames
    /// valides situées APRÈS le trou.
    #[error(
        "WAL : TROU de {longueur} octets nuls à l'offset {offset}, suivi de données — \
         ce n'est ni une fin de fichier pré-allouée ni une frontière de crash : le \
         rejeu s'arrête ICI et le dit (ne tronquez pas, resynchronisez)"
    )]
    WalTrou { offset: u64, longueur: u64 },

    /// ── UNE FRAME QU'UN SEUL `write` NE PEUT PAS POSER D'UN BLOC ──────────
    ///
    /// Au-delà de `MAX_RW_COUNT` (~1 Gio sur Linux), le noyau écrit une frame en
    /// PLUSIEURS fois. Un `write_all` rejoué après un `EINTR`/`ENOSPC` partiel
    /// fabriquerait donc une frame HYBRIDE (en-tête d'un record, corps d'un
    /// autre) : indécodable, et pire, indiscernable d'une frontière de crash. On
    /// refuse à l'encodage plutôt que d'espérer.
    #[error(
        "frame WAL de {taille} octets > {max} : au-delà, un seul write ne suffit plus \
         et la frame deviendrait HYBRIDE (déchirée au milieu). Refus à l'encodage"
    )]
    WalFrameTropGrande { taille: usize, max: usize },

    /// ── ÉCRITURE COURTE : LA FRAME A ÉTÉ DÉCHIRÉE, ET ON L'A ANNULÉE ────
    ///
    /// Le disque a accepté `ecrits` octets sur `demandes` (quota atteint en plein
    /// milieu de la frame). La frame partielle a été RETIRÉE par troncature à
    /// `position` : la queue du WAL est propre, et l'append a échoué proprement
    /// plutôt que de laisser une frame hybride.
    #[error(
        "écriture WAL COURTE : {ecrits} octets écrits sur {demandes} (disque plein ?) — \
         frame partielle ANNULÉE par troncature à l'offset {position}, la queue est propre"
    )]
    WalEcritureCourte {
        ecrits: usize,
        demandes: usize,
        position: u64,
    },

    /// ── LA QUEUE EST DÉCHIRÉE ET N'A PAS PU ÊTRE RÉPARÉE ─────────────────
    ///
    /// L'annulation d'une écriture courte a elle-même échoué (ou une queue
    /// déchirée a été trouvée à l'ouverture) : écrire à la suite produirait des
    /// frames que plus aucun lecteur ne verrait, puisque le rejeu s'arrête sur la
    /// déchirure. On refuse donc TOUT append, en le disant, jusqu'à
    /// [`crate::wal_io::WalWriter::reparer_queue`].
    #[error(
        "queue du WAL déchirée ({raison}) : append REFUSÉ tant qu'elle n'est pas \
         réparée — appelez `WalWriter::reparer_queue()` (le rejeu s'arrête sur la \
         déchirure, donc tout append ultérieur serait invisible à la relecture)"
    )]
    WalQueueAPreparer { raison: String },

    /// ── PHASE B, PIÈCE 1 : LE CHECKPOINTEUR EST ÉLU, ET CE N'EST PAS NOUS ──
    ///
    /// Un checkpoint inter-processus n'est sûr que s'il est UNIQUE : l'avance
    /// d'ancre et la troncature ne sont pas atomiques l'une par rapport à
    /// l'autre, et deux checkpointeurs concurrents feraient recouvrir les LSN du
    /// nouveau segment sur ceux du précédent (chacun avançant l'ancre de SA
    /// mesure). Le fence `<wal>.chk` (flock exclusif non bloquant) désigne LE
    /// checkpointeur ; ce refus dit qu'un pair le détient.
    ///
    /// Ce n'est PAS une panne : le WAL n'a rien perdu, il n'a simplement pas
    /// été tronqué. L'appelant automatique (`checkpoint_si_wal_trop_gros`) le
    /// traite comme un « rien à faire » ; l'appelant EXPLICITE le lit comme
    /// « un autre checkpointe, réessayez ».
    #[error(
        "checkpoint NON ÉLU : un autre processus détient le fence {fence} — rien n'a été \
         tronqué (l'avance d'ancre et la troncature ne sont pas atomiques l'une par \
         rapport à l'autre : deux checkpointeurs feraient recouvrir les LSN du nouveau \
         segment sur ceux du précédent)"
    )]
    CheckpointNonElu { fence: String },

    /// ── PHASE B, PIÈCE 1 : LA FRAME A ÉTÉ DÉTRUITE, PUIS ENCORE ───────────
    ///
    /// Un checkpoint d'un AUTRE processus a tronqué le WAL entre notre lecture de
    /// la fin et notre `write` : la frame a été emportée. C'est RÉCUPÉRABLE (on
    /// relit l'ancre, on recale la position, on réécrit) — mais seulement un
    /// nombre BORNÉ de fois. Cette variante nomme l'épuisement des reprises :
    /// sans elle, on boucherait indéfiniment sur un store où un pair checkpoint à
    /// chaque append, et l'appelant croirait à un blocage.
    #[error(
        "frame WAL détruite {essais} fois de suite par une troncature CONCURRENTE \
         (dernière position {position}) : reprises épuisées — le WAL est sain, \
         l'enregistrement n'a pas été posé"
    )]
    WalTroncatureConcurrente { essais: u32, position: u64 },

    #[error("substrate file out of space (array capacity exhausted)")]
    OutOfSpace,

    #[error("node id out of bounds: {0}")]
    NodeOutOfBounds(u32),

    #[error("edge id out of bounds: {0}")]
    EdgeOutOfBounds(u64),

    /// Chantier SlotRemappable (revue adversariale #2, 2026-08-24) — une
    /// repermutation de slots a ete ANNONCEE (`pending`) sans jamais etre
    /// CONFIRMEE (`committed`). Le processus est mort entre la premiere
    /// mutation de zone et la barriere de durabilite : les zones noeuds /
    /// aretes peuvent etre intactes, a moitie permutees ou entierement
    /// permutees, et RIEN de l'exterieur ne les distingue.
    ///
    /// Cette variante existe pour une seule raison : le canal `Err` de
    /// `replay_from` signifie historiquement « le WAL a ete tronque par une
    /// panne, c'est normal, on ouvre quand meme ». Y injecter « les donnees
    /// sont indeterminees, n'ouvrez pas » sans variante distincte, c'est
    /// garantir que le refus soit avale un etage plus haut. Voir
    /// `SubstrateStore::from_substrate`.
    /// ── LE MESSAGE FAIT PARTIE DU MECANISME DE SURETE ──
    ///
    /// Il ne dit pas seulement « je refuse » : il ACHEMINE l'operateur
    /// vers le seul chemin sur. La version precedente renvoyait vers une
    /// variable d'environnement qui effacait l'annonce a l'aveugle — et
    /// la revue a prouve que cela corrompt quand la barriere avait ete
    /// franchie. Un refus qui oriente vaut infiniment mieux qu'un refus
    /// qui bloque.
    ///
    /// La consigne vit dans le `Display` et non dans un `tracing::error!`
    /// precisement pour qu'une assertion puisse l'exiger.
    ///
    /// (Revue #5) Le nom de la commande n'est plus un LITTERAL JUMEAU : il
    /// est interpole depuis `slot_remap::COMMANDE_REPARATION`, et un test
    /// du CLI verifie que cette constante designe bien la sous-commande
    /// clap REELLE. Avant, renommer la sous-commande laissait ce message
    /// envoyer l'operateur vers `error: unrecognized subcommand` — la
    /// troisieme consigne morte de la meme famille.
    #[error(
        "repermutation epoch {epoch} annoncee sans confirmation : l'etat des zones \
         noeuds/aretes est INDETERMINE (intact, a moitie permute, ou entierement \
         permute — rien ne les distingue). Ouvrir la base servirait des reponses \
         potentiellement fausses, avec des CRC valides. NE DEVINEZ PAS : lancez \
         `{commande} {base} --dry-run`, qui etablit ce qui est observable et dit ce \
         qu'il ferait SANS RIEN MODIFIER ; puis, seulement si le diagnostic est \
         concluant, `{commande} --apply` sur ce meme diagnostic.",
        commande = crate::slot_remap::COMMANDE_REPARATION
    )]
    RepermutationIndeterminee {
        epoch: u32,
        /// Chemin de la base concernee — sans lui, la consigne ne serait
        /// pas actionnable sur un processus qui monte plusieurs bases
        /// (les graphes nommes enfants sont montes dans le meme
        /// processus, chacun avec son compteur d'epoch reparti a 1).
        base: String,
    },

    /// Revue #3 (2026-08-24) — la zone `substrate.remap.epoch` est
    /// PRESENTE mais ne se relit pas : magic, version ou CRC.
    ///
    /// Cette zone est l'ORACLE de tout le mecanisme de repermutation.
    /// Jusqu'ici elle echouait EN MODE OUVERT : la moindre anomalie la
    /// faisait relire comme `default()`, donc `committed = 0`, donc le
    /// cas 3 du bras `HilbertRepermuteV2` re-appliquait la permutation
    /// sur des zones DEJA permutees, avec des CRC valides et sans un mot.
    /// Un octet suffisait.
    ///
    /// Le vecteur n'est PAS une corruption de bit improbable : la
    /// constante `REMAP_EPOCH_VERSION` vaut deja 2, donc une version 1 a
    /// existe. Toute montee de version ferait relire TOUTE base comme
    /// `default()`. C'est un evenement PLANIFIE, pas un accident.
    ///
    /// Distinguer ABSENTE (legitime : base jamais repermutee) de
    /// PRESENTE-MAIS-ILLISIBLE (jamais legitime) est exactement ce que
    /// fait `slot_remap::lire_etat_epoch`.
    #[error(
        "zone d'epoch de repermutation illisible ({raison}) : c'est l'ORACLE dont depend \
         toute la protection contre la double application d'une permutation. La lire \
         comme neuve ferait re-permuter des zones deja permutees, avec des CRC valides \
         et sans aucun symptome. La base n'est PAS ouverte. Restaurez la zone \
         substrate.remap.epoch depuis une sauvegarde, ou reconstruisez la base."
    )]
    EpoqueRemapIllisible { raison: String },

    /// Catch-all for in-crate invariants that don't deserve a bespoke
    /// variant. Used by higher-level components (e.g. the label registry)
    /// when they need to surface a structured error without widening the
    /// public error API.
    #[error("substrate invariant violated: {0}")]
    Internal(String),

    /// (jalon 3) Un principal est DEJA pose sur ce handle de store. Le
    /// principal est immuable une fois pose — pas de re-pose, pas
    /// d'effacement (la lecon du `access_context` mutable : une identite
    /// qui peut changer apres coup n'est pas une garantie). Pour lire sous
    /// une autre identite, ouvrez un autre handle.
    #[error(
        "principal deja pose sur ce store (tenant {existant}) : le principal est \
         immuable une fois pose — ouvrez un autre handle pour une autre identite"
    )]
    PrincipalDejaPose { existant: u32 },

    /// (jalon 3) La valeur ne peut pas devenir une portee de lecture :
    /// `u32::MAX` est la sentinelle A5 « non migre », invisible de tous.
    /// L'accepter comme principal la rendrait visible d'elle-meme —
    /// l'inverse exact du fail-closed.
    #[error("tenant {0} irrecevable comme principal (sentinelle non-migre reservee)")]
    PrincipalIrrecevable(u32),

    /// Le parcours a ete INTERROMPU par une annulation cooperative.
    ///
    /// N'est PAS une erreur de stockage : rien n'est corrompu, rien n'a
    /// echoue. C'est le seul moyen pour un balayage interruptible de dire
    /// « je n'ai pas fini » — sans cette variante, un parcours tronque
    /// rendrait un resultat PARTIEL indiscernable d'un resultat complet,
    /// ce qui transformerait une annulation en fausse reponse.
    #[error("scan cancelled")]
    Cancelled,
}

impl From<obrain_common::utils::cancel::Cancelled> for SubstrateError {
    fn from(_: obrain_common::utils::cancel::Cancelled) -> Self {
        SubstrateError::Cancelled
    }
}

pub type SubstrateResult<T> = Result<T, SubstrateError>;
