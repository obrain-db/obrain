//! # Index de TRIGRAMMES des propriétés textuelles (« PropsTrigram »)
//!
//! ## Le besoin, chiffré sur le banc figé (runtime-dev, 347 970 nœuds)
//!
//! Le chat et le MCP posent des requêtes de SOUS-CHAÎNE — `n.name CONTAINS
//! 'iam'`, `n.qualified_name CONTAINS 'token'` — et aucune structure de
//! propriété existante ne peut y répondre :
//!
//! * `property_indexes` et [`crate::props_eq_index`] tranchent l'ÉGALITÉ :
//!   une sous-chaîne n'est pas une clé ;
//! * [`crate::props_index`] est ORDONNÉ : il sert les intervalles et les
//!   préfixes, pas une occurrence en milieu de valeur ;
//! * l'index TEXTE du moteur (`obrain_core::index::text::InvertedIndex`) est
//!   un BM25 **par MOTS**, qui plus est en mémoire et reconstruit à chaque
//!   processus : il ne peut pas servir de source de candidats pour un
//!   `CONTAINS`, car « iam » y chercherait un *jeton* et manquerait
//!   silencieusement `Miami` — un faux négatif, exactement ce que la doctrine
//!   de tous les index de ce dépôt interdit.
//!
//! Le balayage, lui, coûte ceci (mesuré le 2026-09-13, index ON/OFF,
//! minima sur 3 tours) :
//!
//! ```text
//!   MATCH (n) WHERE n.name CONTAINS 'iam' …                    148 – 190 ms
//!   MATCH (n) WHERE n.name CONTAINS 'auth' OR … 'token' …      ~150 ms
//!   balayage nu, témoin (id(n) < 0)                            25,7 ms
//! ```
//!
//! MESURÉ sur les mêmes 347 970 valeurs, ce que des trigrammes rendraient :
//!
//! ```text
//!   terme     vraies lignes   candidats par trigrammes   sélectivité
//!   iam              386               386                   901×
//!   auth             367               367                   948×
//!   token            757               757                   460×
//!   obrain-iam         2                 2               173 985×
//! ```
//!
//! Autrement dit : la sélectivité n'est pas « meilleure », elle est EXACTE sur
//! ces requêtes — il ne reste à revérifier que ce qui correspond vraiment.
//!
//! ## Ce que cet index est, et ce qu'il n'est JAMAIS
//!
//! **LES VALEURS SONT LA SOURCE DE VÉRITÉ. CET INDEX N'EN EST QU'UNE
//! PROJECTION.** Trois conséquences, tenues par construction :
//!
//! 1. Il énumère des CANDIDATS — un SUR-ENSEMBLE des slots dont la valeur
//!    contient le terme. Il ne répond jamais : l'appelant revérifie par
//!    l'ordre de lecture complet (c'est ce que fait déjà la boucle de
//!    revérification de `find_nodes_in_range`, désormais par LOT).
//! 2. [`PropsTrigramIndex::candidats`] rend `None` — son SEUL silence — quand
//!    il ne sait pas : terme plus court que la fenêtre, ou aucun des grammes du
//!    terme n'est indexé parce que tous sont trop communs. Il rend en revanche
//!    `Some(vec![])` quand il SAIT qu'il n'y a rien : intersection vide, ou
//!    gramme absent du corpus entier. La distinction est vitale dans les deux
//!    sens — un `Some(vec![])` rendu par ignorance affirmerait « zéro
//!    résultat », définitif et muet ; un `None` rendu par certitude ferait
//!    balayer 348 000 lignes pour rien.
//! 3. Il se rebâtit intégralement depuis les seules valeurs. Périmé, corrompu,
//!    d'une autre version : on le jette, et le balayage reprend.
//!
//! ## Le seuil de rareté — mesuré, pas deviné
//!
//! Indexer TOUS les trigrammes coûte 24,4 M de postings sur
//! `qualified_name` — 98 Mo en u32, pour 347 970 nœuds seulement. Or la
//! distribution est très inégale (mesurée) :
//!
//! ```text
//!   trigrammes vus <= 64 fois  : 96 % des grammes DISTINCTS,  9,5 % des postings
//!   trigrammes vus <= 512 fois : 98 % des grammes distincts, 14,3 % des postings
//!   trigrammes vus <= 4096     : 100 % des grammes distincts, 34,9 % des postings
//!   les plus fréquents : « ess » (280 191), « ion » (238 307), « sio » (215 229)
//! ```
//!
//! On n'indexe donc QUE les grammes RARES (`df <= seuil`), ce qui donne :
//!
//! ```text
//!   seuil   grammes   postings   taille   sert les termes de la plainte ?
//!     512    271 753   3,49 M     14 Mo   iam, Auth, obrain-iam — pas « auth »/« token »
//!    1024    273 408   4,68 M     19 Mo   + token/Token
//!    4096    275 314   8,54 M     34 Mo   + auth (tous)
//!   aucun    276 117  24,44 M     98 Mo   tous, mais 3× le prix de 4096
//! ```
//!
//! Conséquence assumée, et c'est une FORCE et non une limite : tout gramme
//! RARE est indexé, donc toute requête sélective trouve au moins un gramme pour
//! la servir ; une requête dont tous les grammes sont communs (`CONTAINS 'ion'`)
//! n'obtient rien de l'index et **retombe sur le balayage** — le chemin lent
//! mais juste, jamais un résultat amputé.
//!
//! ## Pourquoi l'intersection est BORNÉE
//!
//! Parce que le seuil borne CHAQUE liste : toute liste retenue fait au plus
//! `seuil` entrées. L'intersection des grammes d'un terme de longueur `L`
//! coûte donc au pire `L × seuil` — 8 × 4096 = 32 768 pour `obrain-iam` au
//! seuil 4096 — et non la somme des listes d'un index complet, où un seul
//! gramme fréquent peut en porter 280 000.
//!
//! ## Ce qui n'est PAS dans ce module
//!
//! La PERSISTANCE. Ce fichier porte le cœur : comptage, seuil, postings,
//! intersection des grammes, et la garantie de sur-ensemble prouvée par ses
//! tests. La zone persistée (pages, CRC, ancres, reconstruction depuis v2),
//! sur le modèle de [`crate::props_index`], vient s'y brancher sans le changer.

use obrain_common::utils::hash::FxHashMap;

/// Largeur de la fenêtre, en octets. Trois est le compromis usuel : un
/// trigramme est assez sélectif pour borner, assez court pour que toute
/// requête utile en contienne au moins un.
pub const LARGEUR_GRAMME: usize = 3;

/// Seuil de rareté par défaut : un gramme est indexé si sa fréquence
/// documentaire ne dépasse pas cette valeur.
///
/// 1024 est le point mesuré où les requêtes de la plainte sont toutes servies
/// (`iam`, `token`, `Token`, `Auth`, `obrain-iam`) pour 19 Mo sur 347 970
/// nœuds et 4,68 M de postings — contre 98 Mo si l'on indexe tout. Le seuil est
/// un paramètre, pas une constante cachée : [`PropsTrigramIndex::construire`]
/// le prend en argument, pour que le coût se paie en connaissance de cause.
pub const SEUIL_DEFAUT: u32 = 1024;

/// Index de trigrammes en mémoire : gramme rare → slots, triés.
///
/// Les postings portent des SLOTS (`u32`) et non des `NodeId`, comme les
/// autres index de propriétés du substrat ([`crate::props_index`]) : c'est
/// l'unité du stockage, et la conversion en `NodeId` appartient à l'appelant.
#[derive(Debug, Default)]
pub struct PropsTrigramIndex {
    seuil: u32,
    /// Gramme → slots qui le portent, TRIÉS et sans doublon.
    postings: FxHashMap<[u8; LARGEUR_GRAMME], Vec<u32>>,
    /// TOUS les grammes rencontrés au moins une fois dans le corpus examiné,
    /// y compris ceux que le seuil a écartés.
    ///
    /// Sert à distinguer deux silences qui n'ont pas le même sens :
    ///   · gramme CONNU mais écarté (trop commun) → l'index ne peut pas
    ///     trancher avec lui, la réponse se relâche ;
    ///   · gramme INCONNU → il n'apparaît dans AUCUNE valeur du corpus, donc sa
    ///     fréquence est nulle, donc aucune valeur ne peut porter le terme :
    ///     c'est une ABSENCE PROUVÉE, et l'index peut conclure `Some(vec![])`
    ///     au lieu de renvoyer l'appelant balayer 348 000 lignes pour rien.
    connus: FxHashMap<[u8; LARGEUR_GRAMME], ()>,
    /// Nombre de valeurs parcourues (pas forcément indexées : une valeur dont
    /// tous les grammes sont communs ne laisse aucune trace, et c'est voulu).
    valeurs_vues: u32,
}

/// Les trigrammes DISTINCTS d'une valeur, en ordre croissant.
///
/// Distincts : un gramme répété dans la même valeur ne doit produire qu'un
/// posting. Sans cela, « aaaaa » pèserait trois fois dans la liste de « aaa »,
/// et chaque intersection paierait ces doublons pour rien.
///
/// Tri + `dedup` plutôt qu'un `contains` par gramme : sur une valeur de 60
/// octets (la longueur moyenne d'un `qualified_name`), c'est ~58 log 58
/// comparaisons au lieu de ~58 × 29, et cette fonction est appelée deux fois
/// par valeur sur 347 970 valeurs.
fn grammes_distincts(valeur: &[u8]) -> Vec<[u8; LARGEUR_GRAMME]> {
    if valeur.len() < LARGEUR_GRAMME {
        return Vec::new();
    }
    let mut out: Vec<[u8; LARGEUR_GRAMME]> = valeur
        .windows(LARGEUR_GRAMME)
        .map(|f| f.try_into().expect("fenêtre de la largeur exacte"))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

impl PropsTrigramIndex {
    /// Bâtit l'index des valeurs fournies, en n'indexant que les grammes dont
    /// la fréquence documentaire est `<= seuil`.
    ///
    /// DEUX PASSES, et c'est nécessaire : la première compte les fréquences
    /// (seule façon de savoir quels grammes sont rares), la seconde n'écrit
    /// que les postings retenus. Bâtir tous les postings puis les filtrer
    /// coûterait les 98 Mo qu'on cherche justement à ne pas payer.
    ///
    /// `seuil == 0` n'indexe RIEN : l'index rend alors `None` partout, donc le
    /// balayage — c'est l'échappatoire qui permet de mesurer son surcoût sur
    /// le même binaire, comme `OBRAIN_PROPS_INDEX_ORDONNE=0` pour l'index
    /// ordonné.
    #[must_use]
    pub fn construire(valeurs: &[(u32, String)], seuil: u32) -> Self {
        // ── LES DEUX PASSES SONT PARALLÈLES (2026-09-15, mesuré) ─────────
        //
        // La construction coûtait **~3 s** sur 418 000 valeurs (mesuré : 2,94 s
        // pour 347 748 valeurs au premier tour du banc), et c'est le prix du
        // PRÉCHAUFFAGE et de la PREMIÈRE requête d'un processus qui vit. Or les
        // deux passes sont des fonctions pures des valeurs — le découpage en
        // trigrammes d'une valeur ne dépend d'aucune autre.
        //
        // L'ordre de fusion est SANS EFFET : les listes de postings sont TRIÉES
        // en fin de construction (`liste.sort_unstable()`), et c'est cet ordre-là
        // qui sert à l'intersection binaire. Les résultats sont donc identiques
        // — les 18 tests du module le vérifient, dont celui du tri.
        use rayon::prelude::*;
        const LOT: usize = 4096;

        // ── PASSE 1 : fréquence documentaire de chaque gramme ────────────
        let freq: FxHashMap<[u8; LARGEUR_GRAMME], u32> = valeurs
            .par_chunks(LOT)
            .map(|lot| {
                let mut local: FxHashMap<[u8; LARGEUR_GRAMME], u32> = FxHashMap::default();
                for (_, v) in lot {
                    for g in grammes_distincts(v.as_bytes()) {
                        *local.entry(g).or_insert(0) += 1;
                    }
                }
                local
            })
            .reduce(FxHashMap::default, |mut a, b| {
                for (g, n) in b {
                    *a.entry(g).or_insert(0) += n;
                }
                a
            });

        // ── FILTRE : ne garder que les grammes RARES ─────────────────────
        let connus: FxHashMap<[u8; LARGEUR_GRAMME], ()> =
            freq.keys().map(|g| (*g, ())).collect();
        let retenus: FxHashMap<[u8; LARGEUR_GRAMME], ()> = freq
            .iter()
            .filter(|(_, df)| **df <= seuil)
            .map(|(g, _)| (*g, ()))
            .collect();

        // ── PASSE 2 : postings des seuls grammes retenus ────────────────
        let postings: FxHashMap<[u8; LARGEUR_GRAMME], Vec<u32>> = valeurs
            .par_chunks(LOT)
            .map(|lot| {
                let mut local: FxHashMap<[u8; LARGEUR_GRAMME], Vec<u32>> = FxHashMap::default();
                for (slot, v) in lot {
                    for g in grammes_distincts(v.as_bytes()) {
                        if retenus.contains_key(&g) {
                            local.entry(g).or_default().push(*slot);
                        }
                    }
                }
                local
            })
            .reduce(FxHashMap::default, |mut a, b| {
                for (g, mut l) in b {
                    a.entry(g).or_default().append(&mut l);
                }
                a
            });
        let mut postings = postings;
        for liste in postings.values_mut() {
            // Tri : l'intersection par appartenance binaire n'a de sens que
            // sur des listes ordonnées, et l'ordre rend les mesures
            // reproductibles.
            liste.sort_unstable();
        }

        Self {
            seuil,
            postings,
            connus,
            valeurs_vues: valeurs.len() as u32,
        }
    }

    /// Le seuil de rareté appliqué à la construction.
    #[must_use]
    pub fn seuil(&self) -> u32 {
        self.seuil
    }

    /// Nombre de grammes distincts indexés.
    #[must_use]
    pub fn grammes(&self) -> usize {
        self.postings.len()
    }

    /// Nombre total de postings (couples gramme × slot).
    #[must_use]
    pub fn postings(&self) -> usize {
        self.postings.values().map(Vec::len).sum()
    }

    /// Nombre de valeurs parcourues à la construction.
    #[must_use]
    pub fn valeurs_vues(&self) -> u32 {
        self.valeurs_vues
    }

    /// Estimation grossière de la mémoire des postings, en octets.
    #[must_use]
    pub fn memoire_octets(&self) -> usize {
        let listes: usize = self
            .postings
            .values()
            .map(|l| l.capacity() * core::mem::size_of::<u32>())
            .sum();
        listes + self.postings.len() * (LARGEUR_GRAMME + 8)
    }

    /// Les slots CANDIDATS dont la valeur peut contenir `terme`.
    ///
    /// Rend `None` — jamais `Some(vec![])` — quand l'index ne sait pas :
    /// terme plus court que [`LARGEUR_GRAMME`], index vide, ou AUCUN des
    /// grammes du terme n'est indexé (tous trop communs). L'appelant reprend
    /// alors le balayage.
    ///
    /// Rend sinon un SUR-ENSEMBLE : l'intersection des listes des grammes du
    /// terme QUI SONT INDEXÉS. Un gramme manquant n'invalide pas la réponse —
    /// il la rend seulement moins serrée — parce qu'un slot qui contient
    /// vraiment le terme contient TOUS ses grammes, donc figure dans chacune
    /// des listes retenues.
    ///
    /// ── `Some(vec![])` EST CONCLUSIF ICI, ET C'EST UNE DIFFÉRENCE VOULUE ──
    ///
    /// Deux chemins y mènent, et tous deux sont des PREUVES, pas des silences :
    ///
    /// * l'intersection des grammes INDEXÉS est vide — un slot qui contient le
    ///   terme contient tous ses grammes, donc il figurerait dans chaque liste :
    ///   aucune valeur ne le porte ;
    /// * le terme porte un gramme INCONNU du corpus entier — sa fréquence est
    ///   nulle, car la passe de comptage a vu toutes les valeurs.
    ///
    /// Les index d'égalité et ordonné du substrat, eux, interdisent le vecteur
    /// vide (« vide au global = je ne sais pas → `None` »), parce qu'une série
    /// vide y est AMBIGUË : aucun nœud ne porte la propriété, ou la fusion n'a
    /// rien reçu. Cette ambiguïté n'existe pas ici, et la confondre ferait
    /// balayer 348 000 lignes pour rendre zéro.
    #[must_use]
    pub fn candidats(&self, terme: &str) -> Option<Vec<u32>> {
        let octets = terme.as_bytes();
        if octets.len() < LARGEUR_GRAMME {
            // Aucun gramme : l'index n'a rien à dire. « ab » matcherait
            // n'importe quelle valeur contenant « ab », et les trigrammes
            // d'une telle requête n'existent pas.
            return None;
        }

        let mut listes: Vec<&Vec<u32>> = Vec::new();
        for g in grammes_distincts(octets) {
            if let Some(l) = self.postings.get(&g) {
                listes.push(l);
            } else if !self.connus.contains_key(&g) {
                // GRAMME INCONNU DU CORPUS ENTIER : sa fréquence est nulle, et
                // la passe de comptage a vu TOUTES les valeurs — donc aucune ne
                // peut porter le terme. C'est une absence PROUVÉE, pas une
                // ignorance : on conclut au lieu d'envoyer l'appelant balayer
                // 348 000 lignes pour rien. (`CONTAINS 'IAM'` sur un corpus qui
                // n'en contient aucun passe par ici, mesuré.)
                //
                // ── RETIRÉ, PUIS RESTAURÉ (2026-09-16) ─────────────────────────
                //
                // Je l'avais retiré pour tarir un faux négatif de production
                // (`CONTAINS '.crates.'` rendant 0 au lieu de 30 825). C'était un
                // BANDAGE SUR UN SYMPTÔME : le tour suivant a établi la vraie
                // cause — la construction en mémoire n'a pas de FENÊTRE, donc les
                // valeurs écrites après son ancre ne sont couvertes par rien.
                //
                // Or cette preuve-ci est sound DÈS QUE LA VUE EST COMPLÈTE, et
                // c'est exactement ce que garantissent le fichier persisté (ancres
                // validées) et sa fenêtre. La retirer coûtait une optimisation
                // mesurée sur le corpus réel (`CONTAINS` sur un terme absent), et
                // le défaut de production a été traité là où il fallait : en
                // rendant la construction en mémoire OPT-IN, et en documentant
                // qu'elle ne doit pas tourner sur un store écrit par un autre
                // processus tant qu'elle n'a pas sa fenêtre.
                return Some(Vec::new());
            }
            // Sinon : gramme CONNU mais écarté par le seuil. On continue — un
            // gramme manquant relâche la réponse, il ne la fausse jamais.
        }
        if listes.is_empty() {
            // TOUS les grammes du terme sont communs, donc écartés : l'index se
            // TAIT. Rendre un ensemble vide ici ferait croire à « zéro
            // résultat » alors qu'on n'en sait rien du tout — c'est le silence
            // honnête, et l'appelant balaie.
            return None;
        }
        // NOTE (2026-09-16) : une intersection VIDE (`Some(vide)`) reste possible
        // — c'est le cas normal d'un terme réellement absent qu'un gramme retenu
        // a suffi à écarter. Elle est *restrictive*, donc sûre : l'appelant
        // revérifie chaque candidat sur les octets, et une intersection vide n'a
        // aucun candidat à revérifier.

        // De la plus courte à la plus longue : commencer par la plus courte
        // rend l'intersection d'autant moins chère que le terme est sélectif.
        listes.sort_unstable_by_key(|l| l.len());

        let mut courant: Vec<u32> = listes[0].clone();
        for l in &listes[1..] {
            if courant.is_empty() {
                break;
            }
            courant.retain(|slot| l.binary_search(slot).is_ok());
        }
        Some(courant)
    }

    /// La liste des slots d'un gramme, si elle existe — pour que l'état du
    /// store puisse réunir l'index construit et le tampon des écritures.
    #[must_use]
    pub fn liste(&self, gramme: &[u8; LARGEUR_GRAMME]) -> Option<&Vec<u32>> {
        self.postings.get(gramme)
    }

    /// Ce gramme a-t-il été RENCONTRÉ dans le corpus examiné (même s'il a été
    /// écarté par le seuil) ? C'est ce qui distingue « trop commun, je ne peux
    /// pas trancher » de « absent partout, donc aucune valeur ne peut porter le
    /// terme ».
    #[must_use]
    pub fn gramme_connu(&self, gramme: &[u8; LARGEUR_GRAMME]) -> bool {
        self.connus.contains_key(gramme)
    }
}

/// Une liste vide partagée : évite d'allouer pour dire « pas de liste de ce
/// côté-ci » quand l'autre source en a une.
static VIDE: Vec<u32> = Vec::new();

/// Nombre maximal de postings que le tampon d'écritures accepte avant de se
/// déclarer DÉBORDÉ — et de faire taire l'index pour cette propriété.
///
/// Un tampon qui grandit sans fin est une fuite ; un tampon tronqué serait un
/// faux négatif. On ne tronque donc jamais : au-delà du seuil, l'index se TAIT
/// (`None`) et le balayage reprend. Même doctrine que le tampon de l'index
/// ordonné, et même conséquence : lent mais juste.
pub const SEUIL_TAMPON: usize = 4_000_000;

/// L'état des index de trigrammes d'un store : les index CONSTRUITS, et le
/// TAMPON des écritures survenues depuis leur construction.
///
/// ── POURQUOI UN TAMPON, ET POURQUOI IL SUFFIT ────────────────────────────
///
/// Un index construit une fois devient FAUX à la première écriture : un nœud
/// dont la valeur se met à contenir le terme n'y figure pas, et il serait
/// silencieusement absent du résultat — le faux négatif que toute la doctrine
/// de ce dépôt interdit.
///
/// La sortie est celle de l'index ordonné : **les postings sont PUREMENT
/// ADDITIFS**. Le crochet d'écriture pousse les grammes de la NOUVELLE valeur
/// dans le tampon ; il n'a jamais besoin de retirer quoi que ce soit, parce
/// qu'une entrée périmée — valeur réécrite, nœud supprimé — n'est qu'un faux
/// positif, écarté par la revérification qui relit la valeur vivante.
///
/// Conséquence : `index ∪ tampon` est un SUR-ENSEMBLE des couples
/// (slot, valeur) vivants à tout instant, **sans verrou commun entre la
/// construction et les écritures** — la construction peut courir pendant qu'on
/// écrit, le tampon couvre exactement ce que son balayage n'aurait pas vu.
#[derive(Debug)]
pub struct TrigrammesDeStore {
    seuil: u32,
    /// Index construits, par nom de propriété.
    indexes: FxHashMap<String, PropsTrigramIndex>,
    /// Écritures survenues depuis la construction, par propriété puis par
    /// gramme : le seul endroit où un nœud neuf se fait connaître.
    tampon: FxHashMap<String, FxHashMap<[u8; LARGEUR_GRAMME], Vec<u32>>>,
    /// Propriétés dont le tampon a débordé : l'index se TAIT pour elles.
    debordes: FxHashMap<String, ()>,
}

impl TrigrammesDeStore {
    /// Un état vide : aucun index construit, aucun tampon.
    #[must_use]
    pub fn new(seuil: u32) -> Self {
        Self {
            seuil,
            indexes: FxHashMap::default(),
            tampon: FxHashMap::default(),
            debordes: FxHashMap::default(),
        }
    }

    /// Le seuil de rareté appliqué aux constructions.
    #[must_use]
    pub fn seuil(&self) -> u32 {
        self.seuil
    }

    /// L'index de cette propriété est-il construit ?
    #[must_use]
    pub fn est_construit(&self, propriete: &str) -> bool {
        self.indexes.contains_key(propriete)
    }

    /// Le nombre de grammes indexés pour une propriété (0 si non construite).
    #[must_use]
    pub fn grammes(&self, propriete: &str) -> usize {
        self.indexes.get(propriete).map_or(0, PropsTrigramIndex::grammes)
    }

    /// L'index CONSTRUIT d'une propriété, s'il existe — c'est ce que le store
    /// persiste après une construction.
    #[must_use]
    pub fn indexe(&self, propriete: &str) -> Option<&PropsTrigramIndex> {
        self.indexes.get(propriete)
    }

    /// Construit l'index d'une propriété depuis ses valeurs.
    ///
    /// Le tampon n'est PAS vidé : il ne contient que des écritures POSTÉRIEURES
    /// à une construction précédente, et les rejouer dans la nouvelle serait au
    /// pire des doublons, que l'intersection absorbe. Le vider risquerait, lui,
    /// d'oublier une écriture arrivée PENDANT cette construction — c'est-à-dire
    /// de fabriquer un faux négatif.
    pub fn construire(&mut self, propriete: &str, valeurs: &[(u32, String)]) {
        self.indexes.insert(
            propriete.to_string(),
            PropsTrigramIndex::construire(valeurs, self.seuil),
        );
        self.debordes.remove(propriete);
    }

    /// Note une ÉCRITURE : les grammes de la nouvelle valeur entrent au tampon.
    ///
    /// Appelée pour chaque écriture de chaîne d'une propriété dont l'index est
    /// CONSTRUIT. Avant la construction, l'écriture est déjà couverte par le
    /// balayage qui la suit : la noter serait payer des grammes pour rien.
    pub fn noter_ecriture(&mut self, propriete: &str, slot: u32, valeur: &str) {
        if self.debordes.contains_key(propriete) {
            return;
        }
        let seuil = SEUIL_TAMPON;
        let tampon = self.tampon.entry(propriete.to_string()).or_default();
        let avant: usize = tampon.values().map(Vec::len).sum();
        if avant >= seuil {
            // DÉBORDEMENT : on ne tronque pas — ce serait un faux négatif — on
            // se TAIT désormais pour cette propriété.
            self.tampon.remove(propriete);
            self.debordes.insert(propriete.to_string(), ());
            tracing::warn!(
                propriete,
                postings = avant,
                "index de trigrammes : tampon d'écritures DÉBORDÉ — l'index se tait, \
                 le balayage reprend (lent mais juste)"
            );
            return;
        }
        for g in grammes_distincts(valeur.as_bytes()) {
            tampon.entry(g).or_default().push(slot);
        }
    }

    /// Les slots CANDIDATS d'un terme : `index ∪ tampon`, intersecté sur les
    /// grammes du terme. `None` = silence, et l'appelant balaie.
    #[must_use]
    pub fn candidats(&self, propriete: &str, terme: &str) -> Option<Vec<u32>> {
        if self.debordes.contains_key(propriete) {
            return None;
        }
        let index = self.indexes.get(propriete);
        let tampon = self.tampon.get(propriete);
        if index.is_none() && tampon.is_none() {
            return None;
        }
        let octets = terme.as_bytes();
        if octets.len() < LARGEUR_GRAMME {
            return None;
        }

        let mut sources: Vec<(&Vec<u32>, Option<&Vec<u32>>)> = Vec::new();
        for g in grammes_distincts(octets) {
            let de_index = index.and_then(|i| i.liste(&g));
            let du_tampon = tampon.and_then(|t| t.get(&g));
            if de_index.is_none() && du_tampon.is_none() {
                // ABSENCE PROUVÉE : l'index est construit pour cette propriété,
                // le gramme n'y a JAMAIS été rencontré, et le tampon ne le
                // porte pas — donc aucune valeur vivante ni écrite ne le porte,
                // donc aucune ne peut contenir le terme. On CONCLUT.
                if index.is_some_and(|i| !i.gramme_connu(&g)) {
                    return Some(Vec::new());
                }
                // Sinon (gramme commun écarté par le seuil, ou index absent) :
                // on continue — la réponse se relâche, elle ne se fausse pas.
                continue;
            }
            sources.push((de_index.unwrap_or(&VIDE), du_tampon));
        }
        if sources.is_empty() {
            return None;
        }

        // De la plus petite source à la plus grande : commencer court rend
        // l'intersection d'autant moins chère que le terme est sélectif.
        sources.sort_unstable_by_key(|(a, b)| a.len() + b.as_ref().map_or(0, |l| l.len()));

        let (a0, b0) = sources[0];
        let mut courant: Vec<u32> = a0.clone();
        if let Some(b) = b0 {
            courant.extend_from_slice(b);
            courant.sort_unstable();
            courant.dedup();
        }
        for (a, b) in &sources[1..] {
            if courant.is_empty() {
                break;
            }
            courant.retain(|slot| {
                a.binary_search(slot).is_ok() || b.is_some_and(|l| l.binary_search(slot).is_ok())
            });
        }
        Some(courant)
    }

    /// Nombre de postings en tampon pour une propriété — observabilité et tests.
    #[must_use]
    pub fn tampon_postings(&self, propriete: &str) -> usize {
        self.tampon
            .get(propriete)
            .map_or(0, |t| t.values().map(Vec::len).sum())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un corpus déterministe, avec des répétitions voulues : les grammes
    /// communs doivent être écartés par le seuil, les rares gardés.
    fn corpus() -> Vec<(u32, String)> {
        let mut v: Vec<(u32, String)> = Vec::new();
        for i in 0..200u32 {
            v.push((i, format!("function_module_{i:03}")));
        }
        for i in 0..200u32 {
            v.push((1000 + i, format!("Miami_token_{i:03}")));
        }
        v.push((9000, "obrain-iam".into()));
        v.push((9001, "obrain-iam.src".into()));
        v.push((9002, "auth_token".into()));
        v
    }

    /// LA GARANTIE DE LA DOCTRINE : jamais de faux négatif. Tout slot dont la
    /// valeur contient le terme DOIT figurer parmi les candidats — sans quoi
    /// l'appelant, qui ne revérifie que les candidats, sortirait le résultat
    /// amputé, sans erreur ni trace.
    #[test]
    fn le_superset_contient_tous_les_vrais() {
        let valeurs = corpus();
        let idx = PropsTrigramIndex::construire(&valeurs, SEUIL_DEFAUT);
        for terme in [
            "iam",
            "IAM",
            "token",
            "auth",
            "Miami",
            "obrain-iam",
            "module_0",
            "function_module_007",
            "absent-du-corpus",
        ] {
            let vrais: Vec<u32> = valeurs
                .iter()
                .filter(|(_, v)| v.contains(terme))
                .map(|(s, _)| *s)
                .collect();
            if vrais.is_empty() {
                // Même sans vrai match, l'index doit répondre (ou se taire) —
                // mais jamais rendre un candidat qui n'existe pas.
                if let Some(c) = idx.candidats(terme) {
                    assert!(
                        c.iter().all(|s| valeurs.iter().any(|(vs, _)| vs == s)),
                        "candidat inconnu du corpus pour « {terme} »"
                    );
                }
                continue;
            }
            let candidats = idx
                .candidats(terme)
                .unwrap_or_else(|| panic!("l'index doit servir « {terme} »"));
            for s in &vrais {
                assert!(
                    candidats.binary_search(s).is_ok(),
                    "FAUX NÉGATIF : le slot {s} contient « {terme} » et n'est pas candidat"
                );
            }
        }
    }

    /// Toute liste retenue est bornée par le seuil — c'est ce qui rend
    /// l'intersection prévisible, et c'est la propriété dont dépend le coût
    /// annoncé dans l'en-tête du module.
    #[test]
    fn aucune_liste_ne_depasse_le_seuil() {
        let valeurs = corpus();
        for seuil in [1u32, 3, 10, 50] {
            let idx = PropsTrigramIndex::construire(&valeurs, seuil);
            for (g, l) in &idx.postings {
                assert!(
                    l.len() as u32 <= seuil,
                    "gramme {:?} : {} postings pour un seuil de {seuil}",
                    core::str::from_utf8(g).unwrap_or("???"),
                    l.len()
                );
            }
        }
    }

    /// Un seuil nul n'indexe rien : l'index se TAIT partout, et l'appelant
    /// balaie. C'est l'échappatoire de mesure, et elle doit être honnête —
    /// pas un « zéro résultat ».
    #[test]
    fn un_seuil_nul_fait_taire_l_index_au_lieu_de_rendre_vide() {
        let valeurs = corpus();
        let idx = PropsTrigramIndex::construire(&valeurs, 0);
        assert_eq!(idx.postings(), 0);
        assert_eq!(idx.candidats("iam"), None);
        assert_eq!(idx.candidats("token"), None);
    }

    /// Un terme plus court que la fenêtre n'a pas de gramme : `None`, jamais
    /// un ensemble vide qui affirmerait « aucun résultat ».
    #[test]
    fn un_terme_plus_court_que_la_fenetre_rend_None() {
        let idx = PropsTrigramIndex::construire(&corpus(), SEUIL_DEFAUT);
        assert_eq!(idx.candidats("ia"), None);
        assert_eq!(idx.candidats(""), None);
        assert_eq!(idx.candidats("a"), None);
    }

    /// L'intersection est PLUS SERRÉE que le gramme le plus rare : c'est tout
    /// l'intérêt d'intersecter au lieu de rendre la plus petite liste.
    #[test]
    fn l_intersection_est_plus_serree_que_la_liste_la_plus_courte() {
        let valeurs = vec![
            (1u32, "aaa_bbb_ccc".to_string()),
            (2, "aaa_zzz_yyy".to_string()),
            (3, "aaa_www_vvv".to_string()),
        ];
        let idx = PropsTrigramIndex::construire(&valeurs, SEUIL_DEFAUT);
        // « aaa » est porté par les trois ; « ccc » par un seul.
        let c = idx.candidats("aaa_bbb_ccc").expect("servi");
        assert_eq!(c, vec![1], "l'intersection doit rendre le seul vrai match");
    }

    /// L'intersection VIDE est une RÉPONSE, pas un silence : elle prouve
    /// qu'aucune valeur ne porte tous les grammes indexés du terme, donc
    /// qu'aucune ne le contient. C'est la différence assumée avec les index
    /// d'égalité et ordonné, où le vecteur vide est ambigu et interdit.
    #[test]
    fn une_intersection_vide_est_conclusive_et_non_un_silence() {
        let valeurs = vec![(1u32, "abcXXX".to_string()), (2, "YYYxyz".to_string())];
        let idx = PropsTrigramIndex::construire(&valeurs, SEUIL_DEFAUT);
        assert!(idx.candidats("abc").is_some(), "« abc » est servi");
        assert!(idx.candidats("xyz").is_some(), "« xyz » est servi");
        assert_eq!(
            idx.candidats("abcxyz"),
            Some(vec![]),
            "aucune valeur ne peut porter « abcxyz » : l'index le SAIT, il ne se tait pas"
        );
    }

    /// Un gramme ABSENT DU CORPUS ENTIER est une preuve d'absence, pas une
    /// ignorance : la passe de comptage a vu toutes les valeurs. L'index doit
    /// CONCLURE au lieu de faire balayer — mesuré sur le store figé,
    /// `CONTAINS 'IAM'` (zéro vraie ligne) tombait sinon sur 279 084 lignes.
    #[test]
    fn un_gramme_absent_du_corpus_conclut_a_l_absence() {
        let valeurs = vec![(1u32, "alpha".to_string()), (2, "beta".to_string())];
        let idx = PropsTrigramIndex::construire(&valeurs, SEUIL_DEFAUT);
        assert_eq!(idx.candidats("zzz"), Some(vec![]), "« zzz » n'existe nulle part");
        assert_eq!(idx.candidats("IAM"), Some(vec![]), "aucun trigramme « IAM »");
    }

    /// Quand TOUS les grammes du terme sont trop communs pour être indexés,
    /// l'index se TAIT : il ne sait pas si des valeurs le contiennent, et
    /// rendre `Some(vec![])` affirmerait « zéro résultat » à tort.
    #[test]
    fn un_terme_entierement_commun_se_tait_au_lieu_de_conclure() {
        // Douze valeurs contenant « aaa » : au seuil 3, les grammes « xxa »,
        // « xaa » et « aaa » sont tous écartés — et « aaa » est le seul gramme
        // de la requête.
        let valeurs: Vec<(u32, String)> = (0..12u32).map(|i| (i, format!("xxaaa{i:02}"))).collect();
        let idx = PropsTrigramIndex::construire(&valeurs, 3);
        assert_eq!(
            idx.candidats("aaa"),
            None,
            "l'index doit se TAIRE, pas nier : il ignore si le terme est présent"
        );
        // Et il sait toujours répondre dès qu'un gramme rare est disponible.
        assert_eq!(idx.candidats("aaa03"), Some(vec![3]));
    }

    /// Un gramme répété dans une même valeur ne produit QU'UN posting : sinon
    /// chaque intersection paierait des doublons, et le seuil compterait des
    /// occurrences au lieu de documents.
    #[test]
    fn un_gramme_repete_ne_compte_qu_une_fois_par_valeur() {
        let valeurs = vec![(7u32, "aaaaaaaa".to_string())];
        let idx = PropsTrigramIndex::construire(&valeurs, SEUIL_DEFAUT);
        assert_eq!(idx.postings(), 1, "un seul gramme distinct : « aaa »");
        assert_eq!(idx.candidats("aaa"), Some(vec![7]));
    }

    /// Le vocabulaire d'un corpus réel n'a rien à voir avec les chaînes de
    /// test : on vérifie ici que la construction ne s'effondre pas quand une
    /// valeur n'a aucun gramme (moins de trois octets).
    #[test]
    fn les_valeurs_sans_gramme_sont_ignorees_sans_bruit() {
        let valeurs = vec![
            (1u32, "ab".to_string()),
            (2, String::new()),
            (3, "abc".to_string()),
        ];
        let idx = PropsTrigramIndex::construire(&valeurs, SEUIL_DEFAUT);
        assert_eq!(idx.valeurs_vues(), 3);
        assert_eq!(idx.candidats("abc"), Some(vec![3]));
        assert_eq!(idx.candidats("ab"), None);
    }

    /// LA GARANTIE APRÈS ÉCRITURE — celle qui a motivé le tampon.
    ///
    /// Sans lui, ce test échouerait : l'index construit avant l'écriture ne
    /// connaît pas le nœud neuf, et le résultat serait AMPUTÉ sans erreur ni
    /// trace. C'est exactement le faux négatif que la doctrine interdit.
    #[test]
    fn une_ecriture_apres_construction_est_trouvee_par_le_tampon() {
        let mut etat = TrigrammesDeStore::new(SEUIL_DEFAUT);
        etat.construire("nom", &[(1, "alpha".to_string()), (2, "beta".to_string())]);
        assert_eq!(
            etat.candidats("nom", "zzz"),
            Some(vec![]),
            "absence prouvée AVANT l'écriture"
        );
        etat.noter_ecriture("nom", 3, "contient-zzz-maintenant");
        assert_eq!(etat.candidats("nom", "zzz"), Some(vec![3]));
        assert_eq!(etat.candidats("nom", "maintenant"), Some(vec![3]));
    }

    /// Le tampon ne remplace pas l'index : les deux se RÉUNISSENT, sinon
    /// l'écriture ferait disparaître les nœuds d'avant.
    #[test]
    fn l_index_et_le_tampon_se_reunissent() {
        let mut etat = TrigrammesDeStore::new(SEUIL_DEFAUT);
        etat.construire("nom", &[(1, "token-ancien".to_string())]);
        etat.noter_ecriture("nom", 2, "token-nouveau");
        assert_eq!(etat.candidats("nom", "token"), Some(vec![1, 2]));
    }

    /// Une réécriture laisse l'ancien posting : un FAUX POSITIF, écarté par la
    /// revérification. Le contraire — retirer l'ancien sans ajouter le nouveau —
    /// serait un manque définitif.
    #[test]
    fn une_reecriture_laisse_un_faux_positif_et_jamais_un_manque() {
        let mut etat = TrigrammesDeStore::new(SEUIL_DEFAUT);
        etat.construire("nom", &[(1, "ancien-mot".to_string())]);
        etat.noter_ecriture("nom", 1, "nouveau-mot");
        let c = etat.candidats("nom", "nouveau").expect("servi");
        assert!(c.contains(&1), "la VALEUR VIVANTE doit être trouvée");
        let c = etat.candidats("nom", "ancien").expect("servi");
        assert!(c.contains(&1), "faux positif assumé, écarté par revérification");
    }

    /// Sans index construit ni écriture, l'état se TAIT — il ne nie rien.
    #[test]
    fn un_etat_vide_se_tait() {
        let etat = TrigrammesDeStore::new(SEUIL_DEFAUT);
        assert_eq!(etat.candidats("nom", "abc"), None);
        assert!(!etat.est_construit("nom"));
        assert_eq!(etat.tampon_postings("nom"), 0);
    }

    /// LE TOUR COMPLET : écrire, rouvrir par `mmap`, et rendre EXACTEMENT les
    /// mêmes candidats que l'index en mémoire — y compris ses deux silences.
    #[test]
    fn la_persistance_rend_les_memes_candidats() {
        let valeurs = corpus();
        let idx = PropsTrigramIndex::construire(&valeurs, SEUIL_DEFAUT);
        let td = tempfile::tempdir().unwrap();
        let chemin = td.path().join("t.v1");
        idx.ecrire_fichier(&chemin, 1234, 567).unwrap();
        let f = TrigrammesFichier::ouvrir(&chemin, SEUIL_DEFAUT, 1234, 567)
            .expect("ouvrable sous ses propres ancres");
        for terme in ["iam", "IAM", "token", "Auth", "obrain-iam", "zzqq-absent", "ion", "aa"] {
            let mut a = idx.candidats(terme);
            let mut b = f.candidats(terme);
            if let Some(v) = a.as_mut() { v.sort_unstable(); }
            if let Some(v) = b.as_mut() { v.sort_unstable(); }
            assert_eq!(b, a, "« {terme} » : mêmes candidats, mêmes silences");
        }
    }

    /// LES ANCRES SONT LA PREUVE : si le store a été écrit depuis, des valeurs
    /// neuves manquent à l'index, donc répondre rendrait un résultat AMPUTÉ.
    #[test]
    fn des_ancres_differentes_font_refuser_le_fichier() {
        let idx = PropsTrigramIndex::construire(&corpus(), SEUIL_DEFAUT);
        let td = tempfile::tempdir().unwrap();
        let chemin = td.path().join("t.v1");
        idx.ecrire_fichier(&chemin, 1234, 567).unwrap();
        assert!(TrigrammesFichier::ouvrir(&chemin, SEUIL_DEFAUT, 1235, 567).is_none(), "WAL");
        assert!(TrigrammesFichier::ouvrir(&chemin, SEUIL_DEFAUT, 1234, 568).is_none(), "slots");
        assert!(
            TrigrammesFichier::ouvrir(&chemin, SEUIL_DEFAUT + 1, 1234, 567).is_none(),
            "seuil différent : l'index ne décrit pas la même chose"
        );
    }

    /// UN ANNUAIRE CORROMPU EST REFUSÉ — il ne répond pas de travers.
    #[test]
    fn un_annuaire_corrompu_est_refuse() {
        let idx = PropsTrigramIndex::construire(&corpus(), SEUIL_DEFAUT);
        let td = tempfile::tempdir().unwrap();
        let chemin = td.path().join("t.v1");
        idx.ecrire_fichier(&chemin, 1, 1).unwrap();
        let mut octets = std::fs::read(&chemin).unwrap();
        octets[ENTETE + 2] ^= 0xFF; // dans l'annuaire : son CRC doit refuser
        std::fs::write(&chemin, &octets).unwrap();
        assert!(
            TrigrammesFichier::ouvrir(&chemin, SEUIL_DEFAUT, 1, 1).is_none(),
            "annuaire corrompu ⇒ refus"
        );
    }

    /// LE FICHIER SURVIT À UN CYCLE COMPLET, et son annuaire reste trié : c'est
    /// ce qui autorise la recherche binaire — donc le fait de ne PAS charger
    /// 34 Mo pour répondre à une requête d'un seul terme.
    #[test]
    fn l_annuaire_est_trie_pour_la_recherche_binaire() {
        let idx = PropsTrigramIndex::construire(&corpus(), SEUIL_DEFAUT);
        let td = tempfile::tempdir().unwrap();
        let chemin = td.path().join("t.v1");
        idx.ecrire_fichier(&chemin, 1, 1).unwrap();
        let octets = std::fs::read(&chemin).unwrap();
        let nb = u32::from_le_bytes(octets[24..28].try_into().unwrap()) as usize;
        assert!(nb > 0);
        let mut precedent: Option<[u8; 3]> = None;
        for k in 0..nb {
            let o = ENTETE + k * 12;
            let g: [u8; 3] = octets[o..o + 3].try_into().unwrap();
            if let Some(p) = precedent {
                assert!(p < g, "l'annuaire doit être strictement trié");
            }
            precedent = Some(g);
        }
    }
}


// ═══════════════════════════════════════════════════════════════════════════
// PERSISTANCE — l'annuaire trié sur disque, lu par `mmap`
// ═══════════════════════════════════════════════════════════════════════════
//
// ── POURQUOI CE FICHIER EXISTE ────────────────────────────────────────────
//
// MESURÉ : la construction coûte 0,26 s pour `name` (279 084 valeurs) et 1,37 s
// pour `qualified_name` (347 748). Dans un processus qui VIT (pont MCP, `viz
// serve`) elle est amortie ; dans un processus JETABLE (`obrain query`, un
// processus par requête) elle est repayée à chaque appel — et un simple
// balayage de 348 000 lignes coûte ~150 ms, donc construire est une perte.
//
// ── CE QUI EST ÉCRIT, ET POURQUOI C'EST LISIBLE SANS TOUT CHARGER ─────────
//
// Un annuaire TRIÉ sur disque, jamais chargé en entier : la recherche d'un
// gramme est une RECHERCHE BINAIRE dans le `mmap` (une vingtaine de sondes,
// quelques pages touchées), puis la seule liste de postings utile est lue.
// C'est ce qui rend la persistance utile au processus jetable : toucher trois
// pages coûte des microsecondes, là où charger 34 Mo coûterait plus cher que le
// balayage qu'on veut éviter.
//
// Les grammes ÉCARTÉS par le seuil (les plus communs : 8 806 sur 276 117 pour
// `qualified_name` au seuil 4096) sont écrits AUSSI, sans listes : sans eux, on
// ne pourrait plus distinguer « trop commun, je ne sais pas » de « absent du
// corpus, je conclus » — les deux silences que ce module tient à séparer.
//
// ── CORRUPTION : le silence, jamais une réponse fausse ────────────────────
//
// En-tête (magic + version + seuil + ANCORES) et annuaire sont couverts par un
// CRC ; chaque liste de postings porte le sien. Une seule vérification qui
// échoue rend `None` : l'appelant balaie. Un index dont on doute ne répond pas.

/// Préfixe des fichiers persistés, dans le répertoire du store.
pub const FICHIER_PREFIXE: &str = "substrate.trigrammes.";

/// « TRIG » en petit-boutiste — un fichier étranger est rejeté, pas interprété.
pub const FORMAT_MAGIC: u32 = 0x5452_4947; // "TRIG"
/// Version du format. Tout changement d'encodage l'incrémente : un fichier d'une
/// autre version est IGNORÉ (et reconstruit), jamais relu de travers.
pub const FORMAT_VERSION: u32 = 1;
/// Taille de l'en-tête, en octets.
const ENTETE: usize = 96;

/// Le nom de fichier d'une propriété : préfixe + nom assaini.
#[must_use]
pub fn nom_de_fichier(propriete: &str) -> String {
    let assaini: String = propriete
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("{FICHIER_PREFIXE}{assaini}.v{FORMAT_VERSION}")
}

fn lire_u32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn lire_u64(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

/// Annuaire persisté d'UNE propriété, lu par `mmap`.
pub struct TrigrammesFichier {
    mmap: memmap2::Mmap,
    seuil: u32,
    /// L'ancre WAL à laquelle ce fichier a été écrit — c'est elle qui dit de
    /// QUAND il date, donc ce qu'il faut compléter par la fenêtre.
    ancre_wal: u64,
    ancre_slots: u32,
    nb_retenus: u32,
    nb_exclus: u32,
    off_retenus: usize,
    off_exclus: usize,
    off_payload: usize,
}

impl PropsTrigramIndex {
    /// Écrit l'index dans `chemin`, sous les ancres données.
    ///
    /// ÉCRITURE ATOMIQUE (fichier temporaire puis `rename`) : un lecteur
    /// concurrent voit l'ancien fichier complet ou le nouveau, jamais un
    /// demi-fichier. L'écriture est le seul moment où l'on matérialise
    /// l'annuaire — une fois, pas à chaque requête.
    pub fn ecrire_fichier(
        &self,
        chemin: &std::path::Path,
        ancre_wal: u64,
        ancre_slots: u32,
    ) -> std::io::Result<()> {
        let mut entrees: Vec<(&[u8; LARGEUR_GRAMME], &Vec<u32>)> = self.postings.iter().collect();
        entrees.sort_unstable_by_key(|(g, _)| **g);
        let mut exclus: Vec<&[u8; LARGEUR_GRAMME]> = self
            .connus
            .keys()
            .filter(|g| !self.postings.contains_key(*g))
            .collect();
        exclus.sort_unstable();

        // ── Payload : [crc u32][valeurs…] par liste, concaténés ──────────
        let mut payload: Vec<u8> = Vec::new();
        let mut offsets: Vec<(u32, u32)> = Vec::with_capacity(entrees.len()); // (off, n)
        for (_, liste) in &entrees {
            let off = payload.len() as u32;
            payload.extend_from_slice(&crc32fast::hash(bytemuck::cast_slice(liste)).to_le_bytes());
            payload.extend_from_slice(bytemuck::cast_slice(liste));
            offsets.push((off, liste.len() as u32));
        }

        // ── Annuaire : (gramme, off, n) trié, puis les grammes écartés ────
        let mut annuaire: Vec<u8> = Vec::with_capacity(entrees.len() * 12 + exclus.len() * 3);
        for ((g, _), (off, n)) in entrees.iter().zip(offsets.iter()) {
            annuaire.extend_from_slice(*g);
            annuaire.push(0);
            annuaire.extend_from_slice(&off.to_le_bytes());
            annuaire.extend_from_slice(&n.to_le_bytes());
        }
        let off_retenus = ENTETE as u64;
        let off_exclus = off_retenus + annuaire.len() as u64;
        for g in &exclus {
            annuaire.extend_from_slice(*g);
        }
        let off_payload = off_exclus + (exclus.len() * LARGEUR_GRAMME) as u64;

        // ── En-tête ─────────────────────────────────────────────────────
        let mut tete = vec![0u8; ENTETE];
        tete[0..4].copy_from_slice(&FORMAT_MAGIC.to_le_bytes());
        tete[4..8].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        tete[8..12].copy_from_slice(&self.seuil.to_le_bytes());
        tete[12..20].copy_from_slice(&ancre_wal.to_le_bytes());
        tete[20..24].copy_from_slice(&ancre_slots.to_le_bytes());
        tete[24..28].copy_from_slice(&(entrees.len() as u32).to_le_bytes());
        tete[28..32].copy_from_slice(&(exclus.len() as u32).to_le_bytes());
        tete[32..40].copy_from_slice(&off_retenus.to_le_bytes());
        tete[40..48].copy_from_slice(&off_exclus.to_le_bytes());
        tete[48..56].copy_from_slice(&off_payload.to_le_bytes());
        // CRC de l'annuaire, puis CRC de l'en-tête (champ crc à zéro).
        tete[56..60].copy_from_slice(&crc32fast::hash(&annuaire).to_le_bytes());
        tete[60..64].copy_from_slice(&crc32fast::hash(&payload).to_le_bytes());
        let somme_tete = {
            let mut c = tete.clone();
            c[64..68].copy_from_slice(&0u32.to_le_bytes());
            crc32fast::hash(&c)
        };
        tete[64..68].copy_from_slice(&somme_tete.to_le_bytes());

        let tmp = chemin.with_extension("tmp");
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&tete)?;
            f.write_all(&annuaire)?;
            f.write_all(&payload)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, chemin)?;
        Ok(())
    }
}

impl TrigrammesFichier {
    /// Ouvre l'annuaire persisté d'une propriété, **si et seulement si** il est
    /// utilisable : bon format, bon seuil, **ancres identiques** à l'état
    /// courant du store, en-tête et annuaire au CRC juste.
    ///
    /// `None` dans tous les autres cas — et c'est la doctrine du module : un
    /// index dont on ne peut pas prouver qu'il décrit l'état courant ne répond
    /// pas. Les ancres sont ce qui le prouve : si le store a été écrit depuis
    /// l'écriture du fichier, des valeurs neuves manquent à l'index, et répondre
    /// rendrait un résultat AMPUTÉ.
    #[allow(unsafe_code)] // `mmap` : voir la SAFETY juste en dessous.
    pub fn ouvrir(
        chemin: &std::path::Path,
        seuil: u32,
        ancre_wal: u64,
        ancre_slots: u32,
    ) -> Option<Self> {
        let f = Self::lire(chemin, seuil)?;
        (f.ancre_wal == ancre_wal && f.ancre_slots == ancre_slots).then_some(f)
    }

    /// Ouvre un fichier PLUS ANCIEN que l'état courant (`ancre_wal` ≤ courant).
    ///
    /// ── POURQUOI CETTE PORTE EXISTE ──────────────────────────────────────
    ///
    /// Refuser tout fichier dont l'ancre a bougé le rendait INUTILE sur un store
    /// qui vit : le harness écrit en continu, donc l'ancre change à chaque
    /// écriture, donc le fichier était toujours refusé, donc la persistance ne
    /// servait à rien là où elle sert le plus.
    ///
    /// Or un fichier ancien n'est pas faux : il décrit l'état d'AVANT, et ce qui
    /// manque est exactement ce qui a été écrit DEPUIS son ancre — que le store
    /// sait retrouver (`slots_noeuds_de_la_fenetre` + `valeurs_vivantes_des_slots`,
    /// la machinerie dont se servent déjà les index ordonné et d'égalité pour
    /// étendre leur fenêtre). L'appelant complète alors son tampon, et
    /// `fichier ∪ tampon` décrit l'état courant.
    ///
    /// La sûreté tient à trois faits : le WAL est append-only (la fenêtre
    /// `[ancre_wal, maintenant]` est lisible), le fichier porte l'état d'avant,
    /// et le tampon porte le delta. Rien n'est supposé — voir l'appelant.
    pub fn ouvrir_jusqua(
        chemin: &std::path::Path,
        seuil: u32,
        ancre_wal_max: u64,
    ) -> Option<Self> {
        let f = Self::lire(chemin, seuil)?;
        (f.ancre_wal <= ancre_wal_max).then_some(f)
    }

    /// L'ancre WAL du fichier : de quand il date.
    #[must_use]
    pub fn ancre_wal(&self) -> u64 {
        self.ancre_wal
    }

    #[allow(unsafe_code)] // `mmap` : voir la SAFETY de `ouvrir`.
    fn lire(chemin: &std::path::Path, seuil: u32) -> Option<Self> {
        let fichier = std::fs::File::open(chemin).ok()?;
        // SAFETY : lecture seule d'un fichier que nous écrivons par `rename`
        // atomique — le `mmap` voit soit l'ancien, soit le nouveau, jamais un
        // état intermédiaire. Aucun écrivain ne modifie les octets en place.
        let mmap = unsafe { memmap2::Mmap::map(&fichier) }.ok()?;
        let b = &mmap[..];
        if b.len() < ENTETE
            || lire_u32(b, 0)? != FORMAT_MAGIC
            || lire_u32(b, 4)? != FORMAT_VERSION
            || lire_u32(b, 8)? != seuil
        {
            return None;
        }
        let ancre_wal = lire_u64(b, 12)?;
        let ancre_slots = lire_u32(b, 20)?;
        let nb_retenus = lire_u32(b, 24)?;
        let nb_exclus = lire_u32(b, 28)?;
        let off_retenus = lire_u64(b, 32)? as usize;
        let off_exclus = lire_u64(b, 40)? as usize;
        let off_payload = lire_u64(b, 48)? as usize;
        let attendu_annuaire = lire_u32(b, 56)?;
        let attendu_payload = lire_u32(b, 60)?;
        // CRC de l'en-tête (champ à zéro le temps du calcul).
        let mut tete = b[..ENTETE].to_vec();
        let somme = lire_u32(b, 64)?;
        tete[64..68].copy_from_slice(&0u32.to_le_bytes());
        if crc32fast::hash(&tete) != somme {
            return None;
        }
        let fin_annuaire = off_payload;
        if off_retenus != ENTETE
            || off_exclus < off_retenus
            || fin_annuaire > b.len()
            || off_exclus + (nb_exclus as usize) * LARGEUR_GRAMME != off_payload
            || off_retenus + (nb_retenus as usize) * 12 != off_exclus
        {
            return None;
        }
        if crc32fast::hash(&b[off_retenus..fin_annuaire]) != attendu_annuaire {
            return None;
        }
        let _ = attendu_payload; // vérifié liste par liste, à la lecture
        Some(Self {
            mmap,
            seuil,
            ancre_wal,
            ancre_slots,
            nb_retenus,
            nb_exclus,
            off_retenus,
            off_exclus,
            off_payload,
        })
    }

    #[must_use]
    pub fn seuil(&self) -> u32 {
        self.seuil
    }

    /// L'annuaire d'un gramme : `Some(Some(liste))` si le gramme est indexé,
    /// `Some(None)` s'il est connu et ÉCARTÉ (trop commun), `None` s'il est
    /// ABSENT du corpus examiné — les trois cas que la lecture doit distinguer.
    fn gramme(&self, g: &[u8; LARGEUR_GRAMME]) -> Option<Option<Vec<u32>>> {
        let b = &self.mmap[..];
        let (mut lo, mut hi) = (0usize, self.nb_retenus as usize);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let o = self.off_retenus + mid * 12;
            let gramme = b.get(o..o + LARGEUR_GRAMME)?;
            match gramme.cmp(&g[..]) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => {
                    // Les offsets de l'annuaire sont écrits RELATIFS au payload
                    // (c'est la seule chose que l'écrivain connaisse au moment
                    // où il remplit l'annuaire) : on les rend absolus ici.
                    let off = lire_u32(b, o + 4)? as usize + self.off_payload;
                    let n = lire_u32(b, o + 8)? as usize;
                    let valeurs = b.get(off + 4..off + 4 + n * 4)?;
                    if crc32fast::hash(valeurs) != lire_u32(b, off)? {
                        return None; // liste corrompue : on se tait
                    }
                    let liste: Vec<u32> = valeurs
                        .chunks_exact(4)
                        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                        .collect();
                    return Some(Some(liste));
                }
            }
        }
        // Pas dans les indexés : est-il CONNU (écarté par le seuil) ?
        let (mut lo, mut hi) = (0usize, self.nb_exclus as usize);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let o = self.off_exclus + mid * LARGEUR_GRAMME;
            let gramme = b.get(o..o + LARGEUR_GRAMME)?;
            match gramme.cmp(&g[..]) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => return Some(None),
            }
        }
        None
    }

    /// Les slots candidats, avec la MÊME sémantique que l'index en mémoire :
    /// `None` = silence (l'appelant balaie), `Some(vide)` = absence PROUVÉE.
    #[must_use]
    pub fn candidats(&self, terme: &str) -> Option<Vec<u32>> {
        let octets = terme.as_bytes();
        if octets.len() < LARGEUR_GRAMME {
            return None;
        }
        let mut listes: Vec<Vec<u32>> = Vec::new();
        for g in grammes_distincts(octets) {
            match self.gramme(&g) {
                Some(Some(l)) => listes.push(l),
                Some(None) => {}          // connu, écarté : la réponse se relâche
                None => return Some(Vec::new()), // absent du corpus : absence prouvée
            }
        }
        if listes.is_empty() {
            return None;
        }
        listes.sort_unstable_by_key(Vec::len);
        let mut courant = listes.remove(0);
        for l in &listes {
            if courant.is_empty() {
                break;
            }
            courant.retain(|s| l.binary_search(s).is_ok());
        }
        Some(courant)
    }
}
