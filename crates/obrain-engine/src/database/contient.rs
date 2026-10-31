//! # `contient` — l'INTENTION « quels nœuds portent cette sous-chaîne ? »
//!
//! ## Le problème, chiffré
//!
//! Un outil qui veut répondre à « qui contient `obrain-iam` ? » n'a pas
//! d'intention à exprimer : il a un TEXTE à faire avaler au moteur —
//! `MATCH (n) WHERE n.qualified_name CONTAINS 'obrain-iam' RETURN count(n)`.
//! Ce texte repasse, à chaque appel, par toute la chaîne :
//!
//! ```text
//!   analyse (lexer/parseur/traduction GQL)   ── payée au 1er appel d'un texte
//!   planification LOGIQUE (cache)            ── 0 µs si le texte est connu
//!   planification PHYSIQUE (choix d'accès)   ── 13 à 770 µs, JAMAIS en cache
//!   exécution (index de trigrammes)          ── quelques µs
//!   surface (HTTP/MCP, allocation du texte)  ── 0,5 à 1 ms
//! ```
//!
//! Autrement dit : le travail UTILE (l'index, quelques µs) est noyé dans la
//! traduction d'un texte qui ne porte aucune information que l'appelant ne
//! possédait déjà. Cette méthode est le chemin direct : **propriété + aiguille
//! → nœuds**, sans texte, sans analyse, sans planification.
//!
//! ## Contrat de retour — celui de l'existant, à la lettre
//!
//! [`ObrainDB::contient`] rend un [`QueryResult`] ordinaire : `columns` vaut
//! `["n"]`, `rows` une ligne par nœud (un `Value::Int64`, l'identifiant de
//! nœud — exactement ce que rend `id(n)`), `execution_time_ms` la durée de
//! l'appel, `rows_scanned` le nombre de candidats réellement examinés (taille
//! de la liste d'index, ou nombre de slots vivants au balayage). Aucun champ
//! nouveau, aucune forme nouvelle : un appelant qui sait lire une réponse GQL
//! sait lire celle-ci.
//!
//! ## Sémantique — celle de l'évaluateur, pas une approximation
//!
//! `n.prop CONTAINS 'aiguille'` veut dire, dans le moteur : `n.prop` est une
//! CHAÎNE (sinon `as_str()` rend `None`, donc NULL, donc pas de ligne) et
//! `str::contains`. C'est exactement ce que fait le prédicat du substrat
//! ([`PredicatTexte::Contient`]), et cette méthode ne réimplémente rien : elle
//! emprunte **le même chemin que le planificateur**.
//!
//! DEUX ROUTES, et jamais de réponse inventée :
//!
//! 1. **Index** — `noeuds_par_predicat_texte` rend la liste EXACTE des nœuds
//!    (candidats d'index, puis revérification sur les valeurs courantes par le
//!    substrat). La liste est crue parce qu'elle est revérifiée, jamais parce
//!    qu'elle est indexée.
//! 2. **Balayage vérifié** — quand l'index se tait (`None` : pas d'index,
//!    grammes trop communs, delta de fenêtre trop gros) ou quand il rend trop
//!    de candidats pour valoir son coût (la garde adaptative du planificateur,
//!    MÊME seuil, MÊME variable de réglage : `OBRAIN_TEXTE_CANDIDATS_MAX`).
//!    Le balayage lit les valeurs par lots et juge sur les OCTETS
//!    (`verdicts_texte_batch`), sans matérialiser un `ArcStr` par cellule.
//!
//! Les deux routes rendent le même ensemble ; c'est un test, pas une
//! promesse (`tests/contient_intention.rs`, comptes comparés à GQL).
//!
//! ## Ce que cette méthode n'est PAS
//!
//! Elle ne prend qu'une propriété et une aiguille : pas de label, pas de
//! conjonction, pas de transaction, pas de voyage dans le temps. Une intention
//! qui a besoin de cela passe par GQL — le moteur est là pour ça. La
//! visibilité est celle d'un lecteur SANS transaction à l'époque courante,
//! c'est-à-dire exactement ce que voit `MATCH (n)` hors transaction.

use web_time::Instant;

use obrain_common::types::{LogicalType, NodeId, PropertyKey, Value};
use obrain_common::utils::error::Result;
use obrain_core::graph::GraphStore;
use obrain_core::graph::traits::PredicatTexte;

use crate::database::{ObrainDB, QueryResult};

/// Taille des lots du balayage vérifié — la même que la lecture par lot des
/// opérateurs, pour que le coût par ligne reste celui du moteur.
const LOT: usize = 2048;

/// Par quelle route la réponse a été obtenue.
///
/// Ce n'est pas de la décoration : c'est la seule façon, pour un banc ou un
/// lecteur de journal, de savoir si les microsecondes mesurées viennent de
/// l'index ou du balayage — les deux n'ont ni le même coût ni la même
/// signification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteContient {
    /// Liste exacte rendue par l'index de sous-chaînes (trigrammes).
    Index,
    /// L'index s'est tu, ou coûtait plus cher que le balayage : balayage
    /// vérifié sur les octets.
    Balayage,
}

impl RouteContient {
    /// Étiquette stable, pour les journaux et les bancs.
    #[must_use]
    pub fn nom(self) -> &'static str {
        match self {
            Self::Index => "index",
            Self::Balayage => "balayage",
        }
    }
}

/// La réponse, et par où elle est passée.
#[derive(Debug)]
pub struct ReponseContient {
    /// La réponse, au contrat de l'existant.
    pub result: QueryResult,
    /// La route effectivement empruntée.
    pub route: RouteContient,
}

impl ObrainDB {
    /// Les nœuds dont la propriété `propriete` CONTIENT `aiguille`.
    ///
    /// Équivalent exact de
    /// `MATCH (n) WHERE n.<propriete> CONTAINS '<aiguille>' RETURN id(n)`,
    /// sans texte, sans analyse et sans planification.
    ///
    /// # Errors
    ///
    /// Ne rend une erreur que si le magasin refuse la lecture (store fermé,
    /// zone illisible). Une aiguille absente du store rend zéro ligne — une
    /// réponse, pas une erreur.
    pub fn contient(&self, propriete: &str, aiguille: &str) -> Result<QueryResult> {
        Ok(self.contient_avec_route(propriete, aiguille, None)?.result)
    }

    /// [`ObrainDB::contient`], borné aux `limite` premiers nœuds (ordre
    /// croissant des identifiants — donc déterministe, comme tout ordre de ce
    /// moteur).
    ///
    /// # Errors
    ///
    /// Voir [`ObrainDB::contient`].
    pub fn contient_limite(
        &self,
        propriete: &str,
        aiguille: &str,
        limite: usize,
    ) -> Result<QueryResult> {
        Ok(self
            .contient_avec_route(propriete, aiguille, Some(limite))?
            .result)
    }

    /// [`ObrainDB::contient`], avec la route utilisée en clair.
    ///
    /// # Errors
    ///
    /// Voir [`ObrainDB::contient`].
    pub fn contient_avec_route(
        &self,
        propriete: &str,
        aiguille: &str,
        limite: Option<usize>,
    ) -> Result<ReponseContient> {
        let debut = Instant::now();
        let store: &dyn GraphStore = self.store().as_ref();
        let cle = PropertyKey::new(propriete);
        let predicat = PredicatTexte::Contient(aiguille);

        // ── LE PLAFOND DE CANDIDATS, À L'IDENTIQUE DU PLANIFICATEUR ─────────
        //
        // Un index qui rend BEAUCOUP de candidats coûte plus cher que le
        // balayage (chaque candidat est revérifié un par un) : mesuré 790 ms
        // par l'index contre 80 ms au balayage sur `runtime-dev` avec
        // l'aiguille banale `banc_obrain`. Le seuil est RELATIF au store (1 %,
        // plancher 256) pour que la même question se comporte pareil à mille et
        // à dix millions de nœuds, et `OBRAIN_TEXTE_CANDIDATS_MAX` (en pour
        // cent) le règle pour la mesure. Réutiliser la formule du planificateur
        // n'est pas de la symétrie gratuite : deux seuils divergents
        // donneraient deux coûts pour la même intention.
        let total = store.node_count();
        let plafond = std::env::var("OBRAIN_TEXTE_CANDIDATS_MAX")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .map(|pct| total.saturating_mul(pct) / 100)
            .unwrap_or_else(|| (total / 100).max(256));

        let (mut ids, route, examines): (Vec<NodeId>, RouteContient, usize) =
            match store.noeuds_par_predicat_texte(&cle, &predicat) {
                Some(ids) if total == 0 || ids.len() <= plafond => {
                    let n = ids.len();
                    (ids, RouteContient::Index, n)
                }
                _ => {
                    let ids = balayer(store, &cle, &predicat);
                    (ids, RouteContient::Balayage, total)
                }
            };

        // ── VISIBILITÉ : celle du `MATCH (n)` hors transaction ──────────────
        //
        // Le filtre par lot est celui que l'opérateur de balayage applique
        // (`filter_visible_node_ids`), donc les deux routes voient les mêmes
        // nœuds. Sur une base sans transaction ni version en attente, le
        // substrat le sait (`scan_visibility_is_trivial`) et la passe est un
        // simple filtre de vivants — on la garde tout de même : c'est la
        // garantie, pas l'optimisation, qui compte ici.
        let epoch = self.transaction_manager.current_epoch();
        if !store.scan_visibility_is_trivial() {
            ids = store.filter_visible_node_ids(&ids, epoch);
        }

        // Ordre déterministe et unicité : l'index rend une liste par slot, le
        // balayage une liste par slot aussi — mais deux routes qui divergent
        // d'un doublon divergeraient sur le compte, et c'est précisément ce que
        // ce module doit rendre impossible.
        ids.sort_unstable();
        ids.dedup();
        if let Some(k) = limite {
            ids.truncate(k);
        }

        let mut result = QueryResult::with_types(vec!["n".to_string()], vec![LogicalType::Int64]);
        result.rows = ids
            .iter()
            .map(|id| vec![Value::Int64(id.0 as i64)])
            .collect();
        result.rows_scanned = Some(examines as u64);
        result.execution_time_ms = Some(debut.elapsed().as_secs_f64() * 1000.0);
        Ok(ReponseContient { result, route })
    }
}

/// Le balayage vérifié : tous les nœuds vivants, jugés sur les octets de la
/// valeur, par lots.
///
/// C'est le chemin que prend le moteur quand aucun index ne peut servir — la
/// seule différence est qu'ici il n'y a ni opérateur, ni expression à
/// évaluer : le verdict vient de [`GraphStore::verdicts_texte_batch`], dont la
/// documentation fixe la sémantique (« `left.as_str()?` puis `l.contains(r)` »).
/// Un magasin qui ne sait pas répondre sans allouer rend `None` : on retombe
/// alors sur la lecture de valeurs, correcte partout.
fn balayer(store: &dyn GraphStore, cle: &PropertyKey, predicat: &PredicatTexte<'_>) -> Vec<NodeId> {
    let tous = store.all_node_ids();
    let mut out: Vec<NodeId> = Vec::new();
    for lot in tous.chunks(LOT) {
        match store.verdicts_texte_batch(lot, cle, std::slice::from_ref(predicat)) {
            Some(mut verdicts) => {
                if let Some(verdicts) = verdicts.pop() {
                    for (i, id) in lot.iter().enumerate() {
                        if verdicts.get(i).copied().flatten() == Some(true) {
                            out.push(*id);
                        }
                    }
                }
            }
            None => {
                for id in lot {
                    if let Some(Value::String(valeur)) = store.get_node_property(*id, cle)
                        && predicat.verdict(valeur.as_bytes()) == Some(true)
                    {
                        out.push(*id);
                    }
                }
            }
        }
    }
    out
}
