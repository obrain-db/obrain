//! Les erreurs du journal — aucune n'est silencieuse, aucune n'est ambiguë.
//!
//! Un journal inviolable n'a qu'une façon d'échouer honnêtement : **dire où** la
//! chaîne s'est rompue. Une erreur qui se contenterait d'un « vérification
//! échouée » obligerait l'appelant à re-vérifier lui-même pour savoir ce qui a
//! bougé — c'est-à-dire à réimplémenter la vérification, donc à diverger.

/// Résultat des opérations de la crate.
pub type Resultat<T> = core::result::Result<T, ErreurJournal>;

/// Toutes les erreurs de `obrain-journal`.
#[derive(Debug, thiserror::Error)]
pub enum ErreurJournal {
    /// Le système d'exploitation n'a pas rendu d'entropie pour la clé.
    #[error("entropie indisponible : {0}")]
    Entropie(String),

    /// La signature d'une entrée ne vaut pas pour la clé publique annoncée.
    ///
    /// C'est le seul contrôle qu'un opérateur qui détient la clé de signature
    /// **ne peut pas** contourner en recalculant la chaîne : resigner une autre
    /// histoire est possible, mais pas sans que les reçus détenus par les
    /// clients cessent de s'accorder.
    #[error("signature invalide à la séquence {sequence}")]
    SignatureInvalide {
        /// Séquence de l'entrée fautive.
        sequence: u64,
    },

    /// La chaîne ne se tient plus à partir de cette séquence.
    #[error("chaîne rompue à la séquence {sequence} : {motif}")]
    ChaineRompee {
        /// Première séquence incohérente.
        sequence: u64,
        /// Ce qui ne se tient pas, en clair.
        motif: String,
    },

    /// La tête lue n'est pas celle qu'un client avait souvenue : le journal a
    /// été tronqué, ou remplacé par un autre.
    #[error("journal tronqué ou remplacé : tête souvenue {attendue}, lue {lue}")]
    TeteDifferente {
        /// Tête que le client avait conservée.
        attendue: String,
        /// Tête réellement lue.
        lue: String,
    },

    /// Un reçu porte l'empreinte d'un autre journal : il ne s'accorde avec
    /// celui-ci, et ne doit jamais y être versé.
    #[error("reçu d'un autre journal (empreinte {0})")]
    JournalEtranger(String),

    /// Aucune entrée à cette séquence — le journal est plus court que le reçu.
    #[error("aucune entrée à la séquence {sequence} : le journal est plus court que le reçu")]
    SequenceAbsente {
        /// Séquence demandée.
        sequence: u64,
    },

    /// L'élément présenté ne correspond pas à ce qui a été inscrit à cette
    /// séquence.
    ///
    /// C'est la variante de la **comparaison** : le reçu d'un client confronté au
    /// journal, ou l'événement montré confronté au reçu. Elle est distincte de
    /// [`ErreurJournal::ChaineRompee`], qui décrit une incohérence *interne* au
    /// journal : ici, le journal peut être parfaitement cohérent, et c'est ce
    /// qu'on lui oppose qui ne l'est pas.
    #[error("ce qui est présenté ne correspond pas à l'entrée {sequence} du journal")]
    ElementDifferent {
        /// Séquence de l'entrée confrontée.
        sequence: u64,
    },

    /// L'identifiant fourni ne peut pas désigner un journal.
    #[error("référence invalide : {0}")]
    Reference(String),
}
