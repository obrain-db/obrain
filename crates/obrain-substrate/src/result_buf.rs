//! # `ResultBuf` — l'arène réutilisable des chemins qui DOIVENT copier
//!
//! Les vues du pont mémoire ([`crate::view`]) suppriment la copie quand un
//! `&str` emprunté suffit. Mais certains chemins matérialisent par nature
//! (agrégats, résultats qui survivent à la garde, transferts entre
//! threads) : là, le coût évitable n'est pas LA copie — ce sont les
//! ALLOCATIONS répétées. `ResultBuf` est une arène possédée par
//! l'appelant : les textes sont copiés bout à bout dans UNE `String`, les
//! lignes sont des offsets, et [`ResultBuf::clear`] recycle les deux
//! capacités sans dealloc — amorti sur des milliers de requêtes
//! (design O1 §4, « Buffer de résultat réutilisable »).
//!
//! Indépendant des vues : aucune garde, aucun lifetime de store — le
//! buffer possède ses octets et peut survivre au store qui les a fournis.
//! Zéro `unsafe` : les tranches sont des `Range` d'une `String` ordinaire.

/// Une ligne de résultat : l'identifiant brut de l'entité (`NodeId.0` ou
/// `EdgeId.0`) et la tranche `[debut, fin)` de son texte dans l'arène.
#[derive(Debug, Clone, Copy)]
struct Ligne {
    id: u64,
    debut: usize,
    fin: usize,
}

/// Arène de résultats `(id, texte)` réutilisable entre requêtes.
///
/// ```
/// use obrain_substrate::ResultBuf;
/// let mut buf = ResultBuf::new();
/// buf.push(7, "premier");
/// buf.push(9, "second");
/// assert_eq!(buf.len(), 2);
/// assert_eq!(buf.get(1), Some((9, "second")));
/// let capa = buf.capacite_octets();
/// buf.clear(); // longueur 0, capacités CONSERVÉES : la requête suivante n'alloue pas
/// assert!(buf.is_empty());
/// assert_eq!(buf.capacite_octets(), capa);
/// ```
#[derive(Debug, Default)]
pub struct ResultBuf {
    texte: String,
    lignes: Vec<Ligne>,
}

impl ResultBuf {
    /// Arène vide (aucune allocation avant le premier [`Self::push`]).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Arène pré-dimensionnée : `octets` d'arène de texte, `lignes` lignes.
    #[must_use]
    pub fn with_capacity(octets: usize, lignes: usize) -> Self {
        Self {
            texte: String::with_capacity(octets),
            lignes: Vec::with_capacity(lignes),
        }
    }

    /// Vide l'arène SANS rendre la mémoire : les capacités (octets et
    /// lignes) restent acquises pour la requête suivante — c'est tout
    /// l'apport mesuré du buffer face aux allocations répétées.
    pub fn clear(&mut self) {
        self.texte.clear();
        self.lignes.clear();
    }

    /// Copie `s` en fin d'arène et enregistre la ligne `(id, s)`.
    pub fn push(&mut self, id: u64, s: &str) {
        let debut = self.texte.len();
        self.texte.push_str(s);
        self.lignes.push(Ligne {
            id,
            debut,
            fin: self.texte.len(),
        });
    }

    /// Nombre de lignes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lignes.len()
    }

    /// Vrai si aucune ligne.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lignes.is_empty()
    }

    /// La ligne `i` : `(id, texte)`.
    #[must_use]
    pub fn get(&self, i: usize) -> Option<(u64, &str)> {
        let l = self.lignes.get(i)?;
        Some((l.id, &self.texte[l.debut..l.fin]))
    }

    /// Toutes les lignes, dans l'ordre d'insertion.
    pub fn iter(&self) -> impl Iterator<Item = (u64, &str)> + '_ {
        self.lignes
            .iter()
            .map(|l| (l.id, &self.texte[l.debut..l.fin]))
    }

    /// Octets de texte actuellement stockés.
    #[must_use]
    pub fn octets(&self) -> usize {
        self.texte.len()
    }

    /// Capacité acquise de l'arène de texte (survit à [`Self::clear`]).
    #[must_use]
    pub fn capacite_octets(&self) -> usize {
        self.texte.capacity()
    }

    /// Capacité acquise du tableau de lignes (survit à [`Self::clear`]).
    #[must_use]
    pub fn capacite_lignes(&self) -> usize {
        self.lignes.capacity()
    }
}
