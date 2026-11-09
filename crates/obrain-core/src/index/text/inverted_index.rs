//! BM25-scored inverted index for full-text search.

use super::tokenizer::{SimpleTokenizer, Tokenizer};
use obrain_common::types::NodeId;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

/// Configuration for BM25 scoring.
#[derive(Debug, Clone)]
pub struct BM25Config {
    /// Term frequency saturation parameter (default 1.2).
    ///
    /// Higher values give more weight to term frequency.
    pub k1: f64,
    /// Length normalization parameter (default 0.75).
    ///
    /// 0.0 = no length normalization, 1.0 = full normalization.
    pub b: f64,
}

impl Default for BM25Config {
    fn default() -> Self {
        Self { k1: 1.2, b: 0.75 }
    }
}

/// A posting entry: dense document ID, term frequency, and document length.
///
/// `doc_id` est l'**id dense** du document (u32, attribué une fois à
/// l'insertion, voir [`InvertedIndex`]) — et non le `NodeId` u64 d'origine.
/// C'est le cœur du correctif : l'accumulateur de scores s'indexe directement
/// par `doc_id` dans un `Vec<f64>` (accès mémoire plat), sans hasher un
/// `NodeId` ni insérer dans une `HashMap` — là était le goulot mesuré (13
/// ns/posting contre 2,1 ns pour la seule arithmétique BM25).
///
/// `doc_len` reste porté en ligne (copié à l'insertion) pour que la boucle
/// chaude de scoring ne fasse AUCUN lookup `doc_lengths` par posting visité —
/// coût : +4 B par posting (mesuré sur un corpus 20 k-doc : index ~8 Mo →
/// +~0,8 Mo, voir RFC §14 BM25 axe). Le passage `NodeId`(8 B) → `doc_id`(4 B)
/// fait passer le posting de 16 à 12 octets.
#[derive(Debug, Clone, Copy)]
struct Posting {
    doc_id: u32,
    term_freq: u32,
    doc_len: u32,
}

/// A posting list for a single term.
///
/// Les postings y sont triés par `doc_id` croissant : c'est gratuit à
/// l'insertion (les ids denses sont attribués en ordre monotone croissant,
/// donc l'append suit l'ordre), et cela rend les accès au `Vec` accumulateur
/// séquentiels (cache-friendly).
#[derive(Debug, Clone, Default)]
struct PostingList {
    postings: Vec<Posting>,
}

/// An in-memory inverted index with Okapi BM25 scoring.
///
/// Supports insert, remove, and ranked search operations. Designed
/// for indexing text properties on graph nodes.
///
/// # Example
///
/// ```
/// # #[cfg(feature = "text-index")]
/// # {
/// use obrain_core::index::text::{InvertedIndex, BM25Config};
/// use obrain_common::types::NodeId;
///
/// let mut index = InvertedIndex::new(BM25Config::default());
/// index.insert(NodeId::new(1), "rust graph database");
/// index.insert(NodeId::new(2), "python web framework");
///
/// let results = index.search("graph database", 10);
/// assert_eq!(results[0].0, NodeId::new(1));
/// # }
/// ```
pub struct InvertedIndex {
    /// Term → posting list.
    postings: HashMap<String, PostingList>,
    /// Doc dense → longueur du document (en tokens). `Vec` plat au lieu de la
    /// `HashMap<NodeId, u32>` d'origine : l'id dense EST l'index. Un document
    /// supprimé laisse sa case en l'état (l'id dense n'est jamais réutilisé —
    /// attribué UNE fois), donc la taille du `Vec` est le prochain id à
    /// attribuer, et le nombre de documents VIVANTS est `live_docs`.
    doc_lengths: Vec<u32>,
    /// Doc dense → `NodeId` d'origine. La branche INVERSE du bimap, lue
    /// uniquement à la sortie de `search` pour rendre les résultats (et à
    /// l'export), jamais dans la boucle chaude de scoring.
    doc_ids: Vec<NodeId>,
    /// `NodeId` → doc dense. La branche AVANT du bimap, touchée uniquement par
    /// les opérations d'écriture (`insert`/`remove`/`contains`) — jamais par
    /// `search`. C'est le seul `HashMap` restant, et il est hors du chemin
    /// chaud.
    node_to_doc: HashMap<NodeId, u32>,
    /// Nombre de documents VIVANTS. Diffère de `doc_lengths.len()` qui compte
    /// aussi les trous laissés par les suppressions.
    live_docs: usize,
    /// Sum of all document lengths (for average calculation).
    total_length: u64,
    /// Tokenizer used for indexing and querying.
    tokenizer: Box<dyn Tokenizer>,
    /// BM25 configuration.
    config: BM25Config,
}

/// Mélange déterministe de l'id (splitmix64) — anti-biais ET reproductible.
///
/// `node_id` croissant départagerait les ex-æquo par ordre d'allocation, ce qui
/// regroupe le top-k en un seul cluster quand les ids sont attribués par cluster
/// (le défaut « mono-cluster » que l'ancien commentaire signalait, banc hybride
/// C2). Un mélange pseudo-aléatoire mais DÉTERMINISTE répartit les ex-æquo sans
/// dépendre de l'ordre d'allocation des ids denses.
fn melange(id: NodeId) -> u64 {
    let mut z = id.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Entrée du heap top-k : ordre TOTAL (score décroissant, puis `melange(node_id)`
/// croissant sur les ex-æquo), encodé comme un max-heap où « plus grand » = « pire ».
///
/// La clé secondaire est indispensable : un heap naïf (comparateur score seul)
/// garderait arbitrairement les k premiers postings rencontrés parmi les ex-æquo.
/// `melange(node_id)` — et non `node_id` brut — préserve le « mélange » anti-biais
/// de l'ancien tri (pas de regroupement mono-cluster) tout en restant reproductible.
#[derive(Debug, Clone, Copy)]
struct HeapEntry {
    node_id: NodeId,
    score: f64,
}

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.node_id == other.node_id && self.score.to_bits() == other.score.to_bits()
    }
}

impl Eq for HeapEntry {}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapEntry {
    /// `Less` signifie « self est meilleur » (à conserver) : score plus grand
    /// d'abord, puis `melange(node_id)` plus petit sur les ex-æquo. Le
    /// `BinaryHeap` (max-heap) expose donc en tête l'élément le PLUS MAUVAIS,
    /// celui à éjecter quand le heap dépasse k.
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .score
            .partial_cmp(&self.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| melange(self.node_id).cmp(&melange(other.node_id)))
    }
}

/// ── GRATTOIR DE SCORING RÉUTILISABLE (thread-local) ─────────────────────────
///
/// Le goulot mesuré : `search` reconstruisait un `HashMap<NodeId, f64>` à
/// CHAQUE requête → 13 ns/posting contre 2,1 ns pour la seule arithmétique BM25
/// (le hashing du `NodeId` + l'insertion HashMap coûtent 5,9× le flat).
///
/// Ici l'accumulateur est un `Vec<f64>` indexé par l'id dense : `scores[doc_id]
/// += …` est un accès mémoire direct, sans hashing ni réallocation. Il est
/// RÉUTILISÉ d'une requête à l'autre et remis à zéro via le JOURNAL `touched`
/// des seuls `doc_id` touchés — jamais un `memset` O(N) sur tout le vecteur.
///
/// Pourquoi thread-local plutôt qu'un champ ? `search(&self)` est immuable et
/// l'index est partagé `Arc<RwLock<InvertedIndex>>` sur plusieurs threads : un
/// `RefCell` le rendrait `!Sync` (casserait le partage), un `Mutex`
/// sérialiserait les lectures concurrentes. Le thread-local donne un grattioir
/// par thread, sans contention ni dégradation du type. `search` n'étant pas
/// réentrant (il n'appelle jamais `search`), UN grattioir par thread suffit
/// pour TOUS les index de ce thread.
struct Scratch {
    scores: Vec<f64>,
    touched: Vec<u32>,
}

thread_local! {
    static SCRATCH: std::cell::RefCell<Scratch> =
        const { std::cell::RefCell::new(Scratch { scores: Vec::new(), touched: Vec::new() }) };
}

impl InvertedIndex {
    /// Creates a new inverted index with the given BM25 configuration.
    #[must_use]
    pub fn new(config: BM25Config) -> Self {
        Self {
            postings: HashMap::new(),
            doc_lengths: Vec::new(),
            doc_ids: Vec::new(),
            node_to_doc: HashMap::new(),
            live_docs: 0,
            total_length: 0,
            tokenizer: Box::new(SimpleTokenizer::new()),
            config,
        }
    }

    /// Creates a new inverted index with a custom tokenizer.
    pub fn with_tokenizer(config: BM25Config, tokenizer: Box<dyn Tokenizer>) -> Self {
        Self {
            postings: HashMap::new(),
            doc_lengths: Vec::new(),
            doc_ids: Vec::new(),
            node_to_doc: HashMap::new(),
            live_docs: 0,
            total_length: 0,
            tokenizer,
            config,
        }
    }

    /// Renvoie l'id dense du nœud, en l'attribuant UNE fois s'il est inconnu.
    ///
    /// Utilisé par `insert` et `depuis_postings` : un document apparaît dans
    /// autant de termes qu'il en contient, donc la longueur (et l'id dense) ne
    /// doivent être comptés qu'à la PREMIÈRE rencontre — sinon la longueur
    /// moyenne serait gonflée et BM25 répondrait autre chose.
    fn get_or_assign_doc(&mut self, node_id: NodeId, doc_len: u32) -> u32 {
        if let Some(&doc_id) = self.node_to_doc.get(&node_id) {
            return doc_id;
        }
        // Attribution monotone : l'id dense n'est jamais réutilisé, même après
        // suppression (le trou reste). C'est ce qui rend le bimap stable et le
        // `Vec` accumulateur réutilisable sans réindexation.
        let doc_id = self.doc_lengths.len() as u32;
        self.doc_lengths.push(doc_len);
        self.doc_ids.push(node_id);
        self.node_to_doc.insert(node_id, doc_id);
        self.live_docs += 1;
        self.total_length += u64::from(doc_len);
        doc_id
    }

    /// Indexes a document (node text) into the inverted index.
    ///
    /// If the node was already indexed, it is first removed and re-indexed.
    pub fn insert(&mut self, id: NodeId, text: &str) {
        // Remove existing entry if present
        if self.node_to_doc.contains_key(&id) {
            self.remove(id);
        }

        let tokens = self.tokenizer.tokenize(text);
        let doc_len = tokens.len() as u32;

        if doc_len == 0 {
            return;
        }

        // Attribution UNE fois de l'id dense, AVANT d'écrire les postings.
        let doc_id = self.get_or_assign_doc(id, doc_len);

        // Count term frequencies
        let mut term_freqs: HashMap<&str, u32> = HashMap::new();
        for token in &tokens {
            *term_freqs.entry(token.as_str()).or_insert(0) += 1;
        }

        // Add to posting lists. `doc_id` est monotone croissant et ce `insert`
        // ne produit des postings qu'avec CE `doc_id` : chaque liste est donc
        // naturellement triée par `doc_id` (append dans l'ordre d'insertion),
        // sans tri explicite.
        for (term, freq) in term_freqs {
            self.postings
                .entry(term.to_string())
                .or_default()
                .postings
                .push(Posting {
                    doc_id,
                    term_freq: freq,
                    doc_len,
                });
        }
    }

    /// Removes a document from the index.
    ///
    /// Returns `true` if the document was found and removed.
    pub fn remove(&mut self, id: NodeId) -> bool {
        let Some(doc_id) = self.node_to_doc.remove(&id) else {
            return false;
        };

        let doc_len = self.doc_lengths[doc_id as usize];
        self.total_length -= u64::from(doc_len);
        self.live_docs -= 1;
        // On NE réutilise PAS l'id dense : `doc_lengths[doc_id]` et
        // `doc_ids[doc_id]` restent en l'état (valeur morte), inatteignables
        // puisque plus aucun posting vivant ne porte ce `doc_id`. C'est le prix
        // de la stabilité du bimap, payé en quelques cases mortes au pire.

        // Remove from all posting lists
        self.postings.retain(|_, list| {
            list.postings.retain(|p| p.doc_id != doc_id);
            !list.postings.is_empty()
        });

        true
    }

    /// Searches the index using BM25 scoring.
    ///
    /// Returns up to `k` results sorted by descending BM25 score.
    pub fn search(&self, query: &str, k: usize) -> Vec<(NodeId, f64)> {
        let query_tokens = self.tokenizer.tokenize(query);
        if query_tokens.is_empty() || self.live_docs == 0 {
            return Vec::new();
        }
        if k == 0 {
            return Vec::new();
        }

        let n = self.live_docs as f64;
        let avg_dl = self.total_length as f64 / n;

        SCRATCH.with(|cell| {
            let scratch = &mut *cell.borrow_mut();
            let Scratch { scores, touched } = scratch;

            // Garantir la capacité. `resize` ne met à zéro QUE les nouvelles
            // cases, et seulement quand l'index a grandi depuis la dernière
            // requête : le coût O(grandissement) est amorti, et surtout on
            // n'efface PAS tout le vecteur à chaque requête.
            if scores.len() < self.doc_lengths.len() {
                scores.resize(self.doc_lengths.len(), 0.0);
            }
            touched.clear();

            for token in &query_tokens {
                let Some(posting_list) = self.postings.get(token.as_str()) else {
                    continue;
                };

                // IDF: log((N - df + 0.5) / (df + 0.5) + 1)
                let df = posting_list.postings.len() as f64;
                let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();

                for posting in &posting_list.postings {
                    let tf = f64::from(posting.term_freq);
                    // Longueur du document portée en ligne sur le posting —
                    // aucun lookup `doc_lengths` par posting dans la boucle
                    // chaude.
                    let dl = f64::from(posting.doc_len);

                    // BM25: IDF * (tf * (k1 + 1)) / (tf + k1 * (1 - b + b * dl / avgdl)).
                    // Formule volontairement IDENTIQUE (même ordre d'opérations
                    // flottantes) à la version d'origine : les tests d'égalité
                    // exacte des scores (persistance, top-k) restent bit-à-bit.
                    let tf_component = (tf * (self.config.k1 + 1.0))
                        / (tf
                            + self.config.k1 * (1.0 - self.config.b + self.config.b * dl / avg_dl));

                    let idx = posting.doc_id as usize;
                    // Journal du PREMIER contact : chaque contribution BM25 est
                    // strictement positive (idf > 0, tf_component > 0), donc
                    // `scores[idx] == 0.0` signifie « pas encore touché dans
                    // cette requête ». C'est ce qui évite les doublons dans
                    // `touched` sans hashset — et qui permet le reset par
                    // journal plutôt que par memset O(N).
                    if scores[idx] == 0.0 {
                        touched.push(posting.doc_id);
                    }
                    scores[idx] += idf * tf_component;
                }
            }

            let m = touched.len();
            let results = if m <= k {
                // k >= M : rendre tout. Pas de sélection (rien n'est éjecté),
                // donc le biais de top-k n'existe pas ; on garde le comportement
                // historique (tri complet score seul), moins cher que le heap
                // dont les push O(log k) deviendraient des push O(log M).
                let mut results: Vec<(NodeId, f64)> = touched
                    .iter()
                    .map(|&d| (self.doc_ids[d as usize], scores[d as usize]))
                    .collect();
                results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));
                results
            } else {
                // Sélection top-k BORNÉE : heap min de taille k (O(M log k)).
                // Le comparateur est l'ordre TOTAL de `HeapEntry`, donc la
                // sélection est déterministe et rend exactement le même top-k
                // qu'un tri complet — quel que soit l'ordre d'itération.
                let mut heap: BinaryHeap<HeapEntry> = BinaryHeap::with_capacity(k);
                for &d in touched.iter() {
                    let entry = HeapEntry {
                        node_id: self.doc_ids[d as usize],
                        score: scores[d as usize],
                    };
                    if heap.len() < k {
                        heap.push(entry);
                    } else if entry < *heap.peek().expect("heap de taille k non vide") {
                        heap.pop();
                        heap.push(entry);
                    }
                }
                // `into_sorted_vec` rend l'ordre croissant de l'`Ord` (meilleur
                // d'abord) : score décroissant, node_id croissant.
                heap.into_sorted_vec()
                    .into_iter()
                    .map(|e| (e.node_id, e.score))
                    .collect()
            };

            // Remise à zéro PAR JOURNAL : seuls les `doc_id` touchés sont
            // réécrits à 0.0. Pas de `scores.fill(0.0)` — ce serait un memset
            // O(N) à chaque requête, exactement le coût qu'on élimine.
            for &d in touched.iter() {
                scores[d as usize] = 0.0;
            }

            results
        })
    }

    /// Returns true if the given node is indexed.
    #[must_use]
    pub fn contains(&self, id: NodeId) -> bool {
        self.node_to_doc.contains_key(&id)
    }

    /// ── EXPORT : LES POSTINGS, POUR QUE L'INDEX SURVIVE AU PROCESSUS ────────
    ///
    /// Défaut mesuré le 2026-09-16 : l'index inverti vivait **en RAM par
    /// processus** (`ObrainDB::substrate_text_indexes`), reconstruit par un
    /// balayage complet du label à chaque ouverture. Sur un corpus de plusieurs
    /// centaines de milliers de nœuds, ce balayage est le prix payé à CHAQUE
    /// lecteur — et il est payé pour redécouvrir ce qu'on avait déjà calculé.
    ///
    /// Cette méthode rend l'index SERIALISABLE sans exposer ses structures
    /// internes : une liste de termes, chacun avec ses publications
    /// `(nœud, fréquence, longueur du document)`. La reconstruction
    /// ([`Self::depuis_postings`]) réutilise le MÊME tokenizer et la même
    /// config BM25, donc un index rechargé répond exactement comme celui qui a
    /// été persisté — c'est la condition pour qu'une persistance soit une
    /// optimisation et non un changement de réponse.
    ///
    /// Ce qui n'est PAS exporté : la table inverse des longueurs de documents
    /// (recalculable en une passe sur les publications, et plus compacte ainsi),
    /// ni le bimap NodeId↔id dense (les publications portent le `NodeId`
    /// d'origine, ré-attribué en dense à la reconstruction).
    #[must_use]
    pub fn exporter_postings(&self) -> Vec<(String, Vec<(u64, u32, u32)>)> {
        let mut sortie: Vec<(String, Vec<(u64, u32, u32)>)> = self
            .postings
            .iter()
            .map(|(terme, liste)| {
                let mut pubs: Vec<(u64, u32, u32)> = liste
                    .postings
                    .iter()
                    .map(|p| (self.doc_ids[p.doc_id as usize].0, p.term_freq, p.doc_len))
                    .collect();
                // Ordre STABLE : deux exportations du même index doivent donner
                // les mêmes octets, sinon toute empreinte de vérification
                // changerait sans que rien n'ait changé. On trie par `NodeId`
                // (et non par l'id dense, qui dépend de l'ordre d'insertion),
                // pour que l'export soit indépendant de l'historique d'insertion.
                pubs.sort_unstable();
                (terme.clone(), pubs)
            })
            .collect();
        sortie.sort_by(|a, b| a.0.cmp(&b.0));
        sortie
    }

    /// Reconstruit un index depuis des publications exportées.
    ///
    /// Le tokenizer et la config sont ceux que l'appelant fournit — un index
    /// reconstruit avec une autre config répondrait autrement, donc l'appelant
    /// les fait venir de la MÊME source que l'index d'origine.
    #[must_use]
    pub fn depuis_postings(
        config: BM25Config,
        tokenizer: Box<dyn Tokenizer>,
        postings: Vec<(String, Vec<(u64, u32, u32)>)>,
    ) -> Self {
        let mut index = Self::with_tokenizer(config, tokenizer);
        for (terme, pubs) in postings {
            let mut liste = PostingList::default();
            for (id, freq, doc_len) in pubs {
                let node_id = NodeId(id);
                let doc_id = index.get_or_assign_doc(node_id, doc_len);
                liste.postings.push(Posting {
                    doc_id,
                    term_freq: freq,
                    doc_len,
                });
            }
            // Contrairement à `insert`, l'id dense est ici attribué dans
            // l'ordre de PREMIÈRE rencontre (par terme, par nœud trié), pas
            // dans l'ordre d'insertion d'origine : l'append ne garantit donc
            // plus le tri par `doc_id`. On trie explicitement pour rétablir
            // l'invariant des posting lists.
            liste.postings.sort_unstable_by_key(|p| p.doc_id);
            index.postings.insert(terme, liste);
        }
        index
    }

    /// L'identité de ce qui a PRODUIT cet index : le tokenizer (type + réglages)
    /// et la configuration BM25.
    ///
    /// C'est ce qu'un index persisté doit enregistrer pour qu'on ne puisse pas
    /// le recharger avec un autre vocabulaire : les termes stockés sont la sortie
    /// du tokenizer, et les scores sont ceux de la config. Un fichier sans cette
    /// empreinte est un fichier qu'on ne peut pas juger — et un index périmé qui
    /// répond est pire qu'un index absent.
    #[must_use]
    pub fn identite(&self) -> String {
        format!(
            "{};bm25(k1={},b={})",
            self.tokenizer.identite(),
            self.config.k1,
            self.config.b
        )
    }

    /// Le nombre de documents indexés (surface de vérification).
    #[must_use]
    pub fn doc_count(&self) -> usize {
        self.live_docs
    }

    /// Returns the number of indexed documents.
    #[must_use]
    pub fn len(&self) -> usize {
        self.live_docs
    }

    /// Returns true if the index is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.live_docs == 0
    }

    /// Returns the number of unique terms in the index.
    #[must_use]
    pub fn term_count(&self) -> usize {
        self.postings.len()
    }

    /// Returns estimated heap memory in bytes.
    #[must_use]
    pub fn heap_memory_bytes(&self) -> usize {
        // Postings map: term strings + PostingList vecs
        let postings_overhead = self.postings.capacity()
            * (std::mem::size_of::<String>() + std::mem::size_of::<PostingList>() + 1);
        let postings_data: usize = self
            .postings
            .iter()
            .map(|(term, pl)| term.len() + pl.postings.capacity() * std::mem::size_of::<Posting>())
            .sum();
        // Doc lengths : `Vec<u32>` plat + `Vec<NodeId>` (branche inverse du
        // bimap) + `HashMap<NodeId,u32>` (branche avant, hors chemin chaud).
        let doc_lengths_bytes = self.doc_lengths.capacity() * std::mem::size_of::<u32>()
            + self.doc_ids.capacity() * std::mem::size_of::<NodeId>()
            + self.node_to_doc.capacity()
                * (std::mem::size_of::<NodeId>() + std::mem::size_of::<u32>() + 1);
        postings_overhead + postings_data + doc_lengths_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_and_search() {
        let mut index = InvertedIndex::new(BM25Config::default());
        index.insert(
            NodeId::new(1),
            "the quick brown fox jumps over the lazy dog",
        );
        index.insert(NodeId::new(2), "a fast red car drives on the highway");
        index.insert(NodeId::new(3), "the brown dog sleeps all day");

        let results = index.search("brown dog", 10);
        assert!(!results.is_empty(), "résultat attendu non vide");
        // Node 3 mentions both "brown" and "dog" in a shorter document
        assert_eq!(results[0].0, NodeId::new(3));
    }

    #[test]
    fn test_empty_index_search() {
        let index = InvertedIndex::new(BM25Config::default());
        let results = index.search("anything", 10);
        assert!(results.is_empty(), "résultat attendu vide");
    }

    #[test]
    fn test_empty_query() {
        let mut index = InvertedIndex::new(BM25Config::default());
        index.insert(NodeId::new(1), "hello world");
        let results = index.search("", 10);
        assert!(results.is_empty(), "résultat attendu vide");
    }

    #[test]
    fn test_stop_word_only_query() {
        let mut index = InvertedIndex::new(BM25Config::default());
        index.insert(NodeId::new(1), "hello world");
        let results = index.search("the a an", 10);
        assert!(results.is_empty(), "résultat attendu vide");
    }

    #[test]
    fn test_remove() {
        let mut index = InvertedIndex::new(BM25Config::default());
        index.insert(NodeId::new(1), "hello world");
        index.insert(NodeId::new(2), "hello rust");

        assert_eq!(index.len(), 2);
        assert!(index.remove(NodeId::new(1)));
        assert_eq!(index.len(), 1);

        let results = index.search("hello", 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, NodeId::new(2));
    }

    #[test]
    fn test_remove_nonexistent() {
        let mut index = InvertedIndex::new(BM25Config::default());
        assert!(!index.remove(NodeId::new(999)));
    }

    #[test]
    fn test_reinsert() {
        let mut index = InvertedIndex::new(BM25Config::default());
        index.insert(NodeId::new(1), "old text");
        index.insert(NodeId::new(1), "new text completely different");

        assert_eq!(index.len(), 1);
        let results = index.search("old", 10);
        assert!(results.is_empty(), "résultat attendu vide");

        let results = index.search("completely different", 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, NodeId::new(1));
    }

    #[test]
    fn test_contains() {
        let mut index = InvertedIndex::new(BM25Config::default());
        index.insert(NodeId::new(1), "hello world");

        assert!(index.contains(NodeId::new(1)));
        assert!(!index.contains(NodeId::new(2)));
    }

    #[test]
    fn test_term_count() {
        let mut index = InvertedIndex::new(BM25Config::default());
        index.insert(NodeId::new(1), "hello world");
        index.insert(NodeId::new(2), "hello rust");

        // "hello", "world", "rust" (stop words removed)
        assert_eq!(index.term_count(), 3);
    }

    #[test]
    fn test_k_limit() {
        let mut index = InvertedIndex::new(BM25Config::default());
        for i in 1..=10 {
            index.insert(NodeId::new(i), &format!("document number {}", i));
        }

        let results = index.search("document", 3);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_bm25_scoring_prefers_shorter_docs() {
        let mut index = InvertedIndex::new(BM25Config::default());
        // Short doc with the term
        index.insert(NodeId::new(1), "rust database");
        // Long doc with the same term buried in noise
        index.insert(
            NodeId::new(2),
            "rust programming language systems web server framework database engine query optimizer",
        );

        let results = index.search("rust database", 10);
        assert_eq!(results.len(), 2);
        // Shorter doc should score higher (length normalization)
        assert_eq!(results[0].0, NodeId::new(1));
        assert!(results[0].1 > results[1].1);
    }

    #[test]
    fn test_no_match() {
        let mut index = InvertedIndex::new(BM25Config::default());
        index.insert(NodeId::new(1), "hello world");
        let results = index.search("nonexistent term", 10);
        assert!(results.is_empty(), "résultat attendu vide");
    }

    #[test]
    fn test_search_topk_matches_reference_scoring() {
        // Propriété de la formule de score : `search()` (accumulateur dense +
        // tri stable complet) doit renvoyer le même ensemble de scores top-k
        // qu'un calcul de référence (hasher std, tri stable) — ni perte ni
        // ajout dans le top-k. Le passage HashMap→Vec ne change AUCUNE réponse :
        // c'est la condition même du correctif (une optimisation ne doit pas
        // être un changement de réponse).
        let mut index = InvertedIndex::new(BM25Config::default());
        let n = 500usize;
        let topics = [
            "graph", "vector", "search", "memory", "storage", "wal", "mmap", "spread", "engram",
            "index",
        ];
        for i in 0..n {
            let text = format!(
                "document number {i} about {} with tokens alpha beta gamma delta {}",
                topics[i % topics.len()],
                topics[(i / topics.len()) % topics.len()]
            );
            index.insert(NodeId::new(i as u64), &text);
        }
        let queries = [
            "graph vector tokens gamma",
            "tokens gamma",
            "document about graph",
            "alpha beta gamma",
            "search storage mmap",
            "tokens",
        ];
        for q in &queries {
            for k in [1usize, 5, 20] {
                let opt = index.search(q, k);
                // Calcul de référence : même formule, hasher std.
                let tokens = index.tokenizer.tokenize(q);
                let mut manual: Vec<(NodeId, f64)> = {
                    use std::collections::HashMap as StdMap;
                    let mut scores: StdMap<NodeId, f64> = StdMap::new();
                    let nf = index.live_docs as f64;
                    let avg = index.total_length as f64 / nf;
                    for token in &tokens {
                        let Some(pl) = index.postings.get(token.as_str()) else {
                            continue;
                        };
                        let df = pl.postings.len() as f64;
                        let idf = ((nf - df + 0.5) / (df + 0.5) + 1.0).ln();
                        for p in &pl.postings {
                            let tf = f64::from(p.term_freq);
                            let dl = f64::from(p.doc_len);
                            let tc = (tf * (index.config.k1 + 1.0))
                                / (tf
                                    + index.config.k1
                                        * (1.0 - index.config.b + index.config.b * dl / avg));
                            let node_id = index.doc_ids[p.doc_id as usize];
                            *scores.entry(node_id).or_insert(0.0) += idf * tc;
                        }
                    }
                    scores.into_iter().collect()
                };
                manual.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                manual.truncate(k);

                assert_eq!(opt.len(), manual.len(), "même taille top-{k} pour «{q}»");
                // Comparaison des scores triés (l'ordre des ex-aequo n'est
                // pas contractuel) : mêmes scores aux mêmes rangs.
                let mut os: Vec<f64> = opt.iter().map(|x| x.1).collect();
                let mut ms: Vec<f64> = manual.iter().map(|x| x.1).collect();
                os.sort_by(|a, b| b.partial_cmp(a).unwrap());
                ms.sort_by(|a, b| b.partial_cmp(a).unwrap());
                for (a, b) in os.iter().zip(ms.iter()) {
                    assert!(
                        (a - b).abs() < 1e-9,
                        "score divergent top-{k} «{q}» : {a} vs {b}"
                    );
                }
            }
        }
    }

    #[test]
    fn test_idf_weighting() {
        let mut index = InvertedIndex::new(BM25Config::default());
        // "common" appears in all docs, "rare" only in one
        index.insert(NodeId::new(1), "common rare word");
        index.insert(NodeId::new(2), "common another word");
        index.insert(NodeId::new(3), "common third word");

        let results = index.search("rare", 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, NodeId::new(1));

        // "common" matches all three
        let results = index.search("common", 10);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_search_topk_equals_full_sort() {
        // La sélection top-k BORNÉE (heap min de taille k, O(M log k)) doit
        // rendre EXACTEMENT la même liste ordonnée (id, score) qu'un tri
        // complet O(M log M) avec la même clé secondaire (score décroissant,
        // `melange(node_id)` croissant). Le corpus force les ex-æquo : « common »
        // est dans tous les docs, et les longueurs ne varient que sur 50 valeurs,
        // donc 400 docs partagent chaque score — là où un heap naïf (score
        // seul) biaiserait la composition du top-k selon l'ordre d'itération.
        let mut index = InvertedIndex::new(BM25Config::default());
        let m = 20_000usize;
        for i in 0..m {
            let mut text = String::from("common");
            for _ in 0..(i % 50) {
                text.push_str(" pad");
            }
            index.insert(NodeId::new(i as u64), &text);
        }

        // Référence : tri complet du score + clé secondaire melange(node_id).
        let full_sort_ref = |k: usize| {
            let mut all: Vec<(NodeId, f64)> = Vec::with_capacity(m);
            let n = index.live_docs as f64;
            let avg = index.total_length as f64 / n;
            if let Some(pl) = index.postings.get("common") {
                let df = pl.postings.len() as f64;
                let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
                for p in &pl.postings {
                    let tf = f64::from(p.term_freq);
                    let dl = f64::from(p.doc_len);
                    let tc = (tf * (index.config.k1 + 1.0))
                        / (tf
                            + index.config.k1 * (1.0 - index.config.b + index.config.b * dl / avg));
                    all.push((index.doc_ids[p.doc_id as usize], idf * tc));
                }
            }
            all.sort_by(|a, b| {
                b.1.partial_cmp(&a.1)
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| melange(a.0).cmp(&melange(b.0)))
            });
            all.truncate(k);
            all
        };

        for k in [1usize, 10, 100, 1000] {
            let got = index.search("common", k);
            let want = full_sort_ref(k);
            assert_eq!(got.len(), k, "taille top-{k}");
            assert_eq!(
                got, want,
                "top-{k} (sélection bornée) doit être IDENTIQUE au tri complet"
            );
        }
    }

    /// ── LE GOULOT MESURÉ, DÉMONTÉ ────────────────────────────────────────────
    ///
    /// Une requête commune (« tokens » présent dans TOUS les documents) visitait
    /// ~N postings et faisait une insertion `HashMap` par posting. Mesuré en
    /// release : **~347 µs → ~110-120 µs** après le passage à l'accumulateur
    /// dense (`Vec<f64>` indexé par id dense), soit le 13 → 2,2 ns/posting du
    /// micro-banc (`cargo run --release … --example bench_bm25`).
    ///
    /// Ce test vérifie d'abord la CORRECTITÉ (le flat ne change aucune réponse :
    /// « tokens » matche tous les documents), puis AFFICHE le temps moyen pour
    /// que la régression soit visible avec `--nocapture`. On n'assène pas de
    /// borne dure ici : en profil debug le temps est trop dépendant de la
    /// machine pour être un seuil fiable. La preuve chiffrée est le micro-banc.
    #[test]
    fn test_search_tokens_commun_est_rapide_et_correct() {
        let n = 20_000u64;
        let mut index = InvertedIndex::new(BM25Config::default());
        for i in 0..n {
            index.insert(
                NodeId::new(i),
                &format!("tokens common document number {i}"),
            );
        }

        // Correctitude : « tokens » est dans TOUS les documents.
        let tous = index.search("tokens", n as usize);
        assert_eq!(
            tous.len(),
            n as usize,
            "« tokens » doit matcher tous les documents"
        );
        // Et un top-k borné reste correct.
        let top = index.search("tokens", 10);
        assert_eq!(top.len(), 10);

        // Chauffe (la première requête paie l'allocation du grattioir), puis
        // mesure une moyenne sur plusieurs itérations.
        for _ in 0..200 {
            std::hint::black_box(index.search("tokens", 10));
        }
        let iters = 500usize;
        let start = std::time::Instant::now();
        for _ in 0..iters {
            std::hint::black_box(index.search("tokens", 10));
        }
        let elapsed = start.elapsed();
        let ns = elapsed.as_nanos() as f64 / iters as f64;
        eprintln!(
            "search('tokens') sur {n} docs : {ns:.1} ns/requête ({:.2} ns/posting)",
            ns / n as f64
        );
    }
}

#[cfg(test)]
mod tests_export_import {
    use super::*;
    use obrain_common::types::NodeId;

    /// ── LA PERSISTANCE DOIT RENDRE LES MÊMES RÉPONSES, PAS SEULEMENT DES
    ///    RÉPONSES ─────────────────────────────────────────────────────────────
    /// Un index rechargé qui répondrait « à peu près » la même chose serait pire
    /// qu'un index reconstruit : il serait plus rapide ET faux. Ce test tient
    /// l'égalité EXACTE des scores, sur un corpus où les longueurs de documents
    /// diffèrent (c'est là que se joue la normalisation BM25, donc là qu'une
    /// longueur moyenne mal recomptée se verrait).
    #[test]
    fn exporter_puis_reconstruire_rend_les_memes_scores() {
        let docs = [
            (1u64, "rust graph database engine"),
            (2, "python web framework"),
            (3, "graph graph graph database database"),
            (4, "rust web"),
            (5, "database"),
        ];
        let mut original = InvertedIndex::new(BM25Config::default());
        for (id, texte) in docs {
            original.insert(NodeId(id), texte);
        }

        let export = original.exporter_postings();
        let reconstruit = InvertedIndex::depuis_postings(
            BM25Config::default(),
            Box::new(SimpleTokenizer::new()),
            export,
        );

        assert_eq!(reconstruit.len(), original.len(), "mêmes documents");
        assert_eq!(
            reconstruit.term_count(),
            original.term_count(),
            "mêmes termes"
        );
        assert_eq!(reconstruit.doc_count(), original.doc_count());

        for requete in ["graph", "database rust", "web", "framework", "inexistant"] {
            let a = original.search(requete, 10);
            let b = reconstruit.search(requete, 10);
            assert_eq!(
                a.len(),
                b.len(),
                "la requête « {requete} » ne rend pas le même nombre de résultats"
            );
            for ((id_a, score_a), (id_b, score_b)) in a.iter().zip(b.iter()) {
                assert_eq!(id_a, id_b, "même ORDRE pour « {requete} »");
                assert_eq!(score_a, score_b, "score EXACT pour « {requete} »");
            }
        }
    }

    /// ── L'IDENTITÉ SÉPARE CE QUI DOIT ÊTRE SÉPARÉ ──────────────────────────
    /// Deux tokenizers aux réglages différents produisent des TERMES différents ;
    /// deux configs BM25 produisent des SCORES différents. Si leur identité était
    /// la même, un index persisté pourrait être rechargé par un code qui ne
    /// partage plus son vocabulaire — et répondrait sans rien signaler.
    #[test]
    fn l_identite_distingue_les_reglages_et_la_config() {
        let a =
            InvertedIndex::with_tokenizer(BM25Config::default(), Box::new(SimpleTokenizer::new()));
        let b = InvertedIndex::with_tokenizer(
            BM25Config::default(),
            Box::new(SimpleTokenizer::with_min_length(4)),
        );
        assert_ne!(
            a.identite(),
            b.identite(),
            "deux longueurs minimales = deux vocabulaires : l'identité doit différer"
        );

        let c = InvertedIndex::with_tokenizer(
            BM25Config { k1: 2.0, b: 0.5 },
            Box::new(SimpleTokenizer::new()),
        );
        assert_ne!(
            a.identite(),
            c.identite(),
            "deux configs BM25 = deux barèmes : l'identité doit différer"
        );

        // Et elle est STABLE : la même construction donne la même identité.
        let d =
            InvertedIndex::with_tokenizer(BM25Config::default(), Box::new(SimpleTokenizer::new()));
        assert_eq!(a.identite(), d.identite());
        assert!(
            a.identite().contains("min_length=2"),
            "les réglages doivent être LISIBLES dans l'identité : {}",
            a.identite()
        );
    }

    /// L'export est DÉTERMINISTE (termes et publications triés) : sans cela,
    /// toute empreinte de vérification changerait sans que l'index ait bougé.
    #[test]
    fn l_export_est_deterministe() {
        let mut index = InvertedIndex::new(BM25Config::default());
        index.insert(NodeId(3), "un deux trois");
        index.insert(NodeId(1), "trois deux un");
        let a = index.exporter_postings();
        let b = index.exporter_postings();
        assert_eq!(a, b, "deux exports du même index doivent être identiques");
        assert!(a.windows(2).all(|w| w[0].0 <= w[1].0), "termes triés");
    }
}
