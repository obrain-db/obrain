//! Vérification A POSTERIORI des contraintes déclarées : « tiennent-elles sur
//! la base, là, maintenant ? »
//!
//! ── POURQUOI CE MODULE EXISTE ───────────────────────────────────────────────
//! Les contraintes sont appliquées par les opérateurs du langage (`INSERT`,
//! `SET`, `MERGE`, ajout de label). Deux familles d'écritures n'y passent pas,
//! PAR CONTRAT :
//!
//! - l'API directe du store (`create_node_with_props`, `set_node_property`…) ;
//! - l'import en masse (`bulk_import`, `bulk_import_props`).
//!
//! C'est délibéré : une migration de plusieurs millions de nœuds ne peut pas
//! payer une recherche d'index et un verrou par nœud. La contrepartie est ce
//! module : après l'import, [`super::ObrainDB::verifier_contraintes`] dit si
//! les contraintes tiennent, et nomme ce qui les viole.
//!
//! Il sert aussi à `CREATE CONSTRAINT` : une contrainte déclarée sur des
//! données qui la violent déjà est refusée, en nommant un doublon.
//!
//! ── COÛT ────────────────────────────────────────────────────────────────────
//! Une passe par label contraint : les colonnes contraintes sont lues par lots,
//! et chaque tuple est rangé dans une table de hachage. O(nœuds du label) en
//! temps, 16 octets par nœud en mémoire, AUCUNE recherche d'index — donc aucun
//! coût quadratique, et un résultat qui ne dépend pas de l'état des index.
//!
//! ── L'ÉGALITÉ EST CELLE DU LANGAGE ──────────────────────────────────────────
//! Deux tuples sont le même tuple quand leurs membres sont égaux au sens de
//! `=` (`egalite_structurelle`) : `5` et `5.0`, ou deux chaînes datetime du
//! même instant. Le hachage est construit pour ranger ces valeurs égales dans
//! le même seau ; l'égalité est ensuite CONFIRMÉE sur les valeurs elles-mêmes,
//! de sorte qu'une collision de hachage ne fabrique jamais une violation.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hash, Hasher};

use obrain_common::types::value_cmp::{egalite_structurelle, instant_de_chaine};
use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::GraphStore;

use crate::catalog::{NodeTypeDefinition, TypeConstraint};

/// Nombre de nœuds dont les colonnes sont lues d'un coup.
const TAILLE_DE_LOT: usize = 8192;

/// Plafond par défaut du nombre de violations DÉTAILLÉES dans un rapport
/// (elles sont toutes comptées ; seules les premières sont décrites).
pub const VIOLATIONS_DETAILLEES_PAR_DEFAUT: usize = 100;

/// Une violation de contrainte constatée sur les données.
#[derive(Debug, Clone, PartialEq)]
pub struct ViolationContrainte {
    /// Nom de la contrainte (celui de `SHOW CONSTRAINTS` / `DROP CONSTRAINT`).
    pub contrainte: String,
    /// Nature : `unique`, `node_key` ou `not_null`.
    pub nature: &'static str,
    /// Label portant la contrainte.
    pub label: String,
    /// Propriétés liées par la contrainte, dans l'ordre déclaré.
    pub proprietes: Vec<String>,
    /// Le nœud en violation.
    pub noeud: NodeId,
    /// Pour un doublon : le premier nœud rencontré qui porte le même tuple.
    pub premier_porteur: Option<NodeId>,
    /// Pour un doublon : les valeurs du tuple répété.
    pub valeurs: Vec<Value>,
    /// Pour une propriété requise absente (`NOT NULL`, membre d'un
    /// `NODE KEY`) : son nom.
    pub propriete_absente: Option<String>,
}

impl std::fmt::Display for ViolationContrainte {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            contrainte,
            nature,
            label,
            proprietes,
            noeud,
            ..
        } = self;
        let colonnes = proprietes.join(", ");
        match (&self.propriete_absente, self.premier_porteur) {
            (Some(absente), _) => write!(
                f,
                "{nature} constraint '{contrainte}' on :{label} ({colonnes}): \
                 node {} has no value for '{absente}'",
                noeud.0
            ),
            (None, Some(premier)) => {
                let tuple: Vec<String> = self.valeurs.iter().map(|v| format!("{v}")).collect();
                write!(
                    f,
                    "{nature} constraint '{contrainte}' on :{label} ({colonnes}): \
                     nodes {} and {} both carry ({})",
                    premier.0,
                    noeud.0,
                    tuple.join(", ")
                )
            }
            (None, None) => write!(
                f,
                "{nature} constraint '{contrainte}' on :{label} ({colonnes}): \
                 node {} violates it",
                noeud.0
            ),
        }
    }
}

/// Résultat d'une vérification des contraintes sur les données.
#[derive(Debug, Clone, Default)]
pub struct RapportContraintes {
    /// Les violations décrites (au plus le plafond demandé).
    pub violations: Vec<ViolationContrainte>,
    /// Le nombre TOTAL de violations constatées, décrites ou non.
    pub violations_totales: usize,
    /// Nombre de contraintes examinées.
    pub contraintes_examinees: usize,
    /// Nombre de nœuds lus, compté par label contraint (un nœud portant deux
    /// labels contraints est lu deux fois).
    pub noeuds_examines: usize,
    /// Violations déjà signalées (contrainte, nœud) : une contrainte héritée est
    /// examinée sous le label parent ET sous chaque label enfant, la même
    /// violation ne doit être comptée qu'une fois.
    vues: std::collections::HashSet<(String, NodeId)>,
}

impl RapportContraintes {
    /// Vrai si aucune contrainte n'est violée.
    #[must_use]
    pub fn tient(&self) -> bool {
        self.violations_totales == 0
    }

    fn signaler(&mut self, plafond: usize, violation: impl FnOnce() -> ViolationContrainte) {
        let v = violation();
        if !self.vues.insert((v.contrainte.clone(), v.noeud)) {
            return;
        }
        self.violations_totales += 1;
        if self.violations.len() < plafond {
            self.violations.push(v);
        }
    }
}

/// Hacheur identité : les clés sont déjà des hachages de tuple.
#[derive(Default)]
struct Identite(u64);

impl Hasher for Identite {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, octets: &[u8]) {
        for o in octets {
            self.0 = self.0.rotate_left(8) ^ u64::from(*o);
        }
    }
    fn write_u64(&mut self, n: u64) {
        self.0 = n;
    }
}

type TableDeTuples = HashMap<u64, NodeId, BuildHasherDefault<Identite>>;

/// Bits canoniques d'un nombre : `-0.0` et `0.0` confondus, un seul NaN.
fn bits_canoniques(f: f64) -> u64 {
    if f == 0.0 {
        0
    } else if f.is_nan() {
        f64::NAN.to_bits()
    } else {
        f.to_bits()
    }
}

/// Hache une valeur de sorte que deux valeurs ÉGALES au sens de
/// `egalite_structurelle` reçoivent le même hachage. L'inverse n'est pas
/// requis : l'égalité est confirmée ensuite sur les valeurs.
fn hacher_valeur(v: &Value, h: &mut impl Hasher) {
    match v {
        Value::Null => 0u8.hash(h),
        Value::Bool(b) => {
            1u8.hash(h);
            b.hash(h);
        }
        // UNE classe numérique : `5` et `5.0` sont égaux. Un entier égal à un
        // flottant est exactement représentable, donc `i as f64` le rejoint.
        #[allow(clippy::cast_precision_loss)]
        Value::Int64(i) => {
            2u8.hash(h);
            bits_canoniques(*i as f64).hash(h);
        }
        Value::Float64(f) => {
            2u8.hash(h);
            bits_canoniques(*f).hash(h);
        }
        // Deux chaînes datetime du même instant sont égales.
        Value::String(s) => match instant_de_chaine(s) {
            Some(instant) => {
                3u8.hash(h);
                instant.hash(h);
            }
            None => {
                4u8.hash(h);
                s.as_str().hash(h);
            }
        },
        Value::List(elements) => {
            5u8.hash(h);
            elements.len().hash(h);
            for e in elements.iter() {
                hacher_valeur(e, h);
            }
        }
        Value::Map(carte) => {
            6u8.hash(h);
            carte.len().hash(h);
            for (k, e) in carte.iter() {
                k.as_str().hash(h);
                hacher_valeur(e, h);
            }
        }
        // Les autres variantes ne sont égales qu'à elles-mêmes, à
        // représentation identique : leur rendu les distingue.
        autre => {
            7u8.hash(h);
            format!("{autre:?}").hash(h);
        }
    }
}

fn hacher_tuple(tuple: &[&Value]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for v in tuple {
        hacher_valeur(v, &mut h);
    }
    h.finish()
}

fn tuples_egaux(a: &[&Value], b: &[Value]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| egalite_structurelle(x, y))
}

/// Une contrainte en cours de vérification sur un label.
struct Examen<'a> {
    contrainte: &'a TypeConstraint,
    nom: String,
    label: &'a str,
    /// Propriétés de la contrainte, dans l'ordre déclaré.
    proprietes: Vec<String>,
    /// Rang, dans les colonnes lues, de chaque propriété de la contrainte.
    rangs: Vec<usize>,
    /// Tuples déjà vus (contraintes d'unicité seulement).
    vus: TableDeTuples,
}

impl Examen<'_> {
    fn violation(
        &self,
        noeud: NodeId,
        premier_porteur: Option<NodeId>,
        valeurs: Vec<Value>,
        propriete_absente: Option<String>,
    ) -> ViolationContrainte {
        ViolationContrainte {
            contrainte: self.nom.clone(),
            nature: self.contrainte.kind_str(),
            label: self.label.to_string(),
            proprietes: self.proprietes.clone(),
            noeud,
            premier_porteur,
            valeurs,
            propriete_absente,
        }
    }
}

/// Vérifie les contraintes d'UN type de nœud sur les données du store.
///
/// `seulement` restreint l'examen à ces contraintes (c'est ce que fait
/// `CREATE CONSTRAINT` pour la contrainte qu'il s'apprête à déclarer) ;
/// `None` examine toutes celles du type. `plafond` borne le nombre de
/// violations DÉCRITES ; `arret_a_la_premiere` rend la main dès la première.
pub(crate) fn verifier_type(
    store: &dyn GraphStore,
    def: &NodeTypeDefinition,
    seulement: Option<&[TypeConstraint]>,
    plafond: usize,
    arret_a_la_premiere: bool,
    rapport: &mut RapportContraintes,
) {
    let label = def.name.as_str();
    let mut colonnes: Vec<String> = Vec::new();
    let mut examens: Vec<Examen<'_>> = Vec::new();
    for contrainte in &def.constraints {
        if matches!(contrainte, TypeConstraint::Check { .. })
            || seulement.is_some_and(|s| !s.contains(contrainte))
        {
            continue;
        }
        let proprietes = contrainte.properties();
        let rangs = proprietes
            .iter()
            .map(|p| {
                colonnes.iter().position(|c| c == p).unwrap_or_else(|| {
                    colonnes.push(p.clone());
                    colonnes.len() - 1
                })
            })
            .collect();
        let nom = def.constraint_name(contrainte).map_or_else(
            || format!("{label}_{}", contrainte.kind_str()),
            str::to_string,
        );
        examens.push(Examen {
            contrainte,
            nom,
            label,
            proprietes,
            rangs,
            vus: TableDeTuples::default(),
        });
    }
    if examens.is_empty() {
        return;
    }
    rapport.contraintes_examinees += examens.len();

    let cles: Vec<PropertyKey> = colonnes
        .iter()
        .map(|c| PropertyKey::new(c.as_str()))
        .collect();
    let noeuds = store.nodes_by_label(label);
    rapport.noeuds_examines += noeuds.len();

    for lot in noeuds.chunks(TAILLE_DE_LOT) {
        // Une lecture par colonne et par lot : `valeurs[colonne][i]`.
        let valeurs: Vec<Vec<Option<Value>>> = cles
            .iter()
            .map(|cle| store.get_node_property_batch(lot, cle))
            .collect();
        for (i, &noeud) in lot.iter().enumerate() {
            let lue = |rang: usize| -> Option<&Value> {
                valeurs[rang]
                    .get(i)
                    .and_then(Option::as_ref)
                    .filter(|v| !matches!(v, Value::Null))
            };
            for examen in &mut examens {
                // Présence : NOT NULL, et chaque membre d'un NODE KEY.
                let absente = examen
                    .rangs
                    .iter()
                    .position(|&r| lue(r).is_none())
                    .map(|k| examen.proprietes[k].clone());
                if let Some(absente) = absente {
                    if matches!(
                        examen.contrainte,
                        TypeConstraint::NotNull(_) | TypeConstraint::PrimaryKey(_)
                    ) {
                        rapport.signaler(plafond, || {
                            examen.violation(noeud, None, Vec::new(), Some(absente))
                        });
                        if arret_a_la_premiere {
                            return;
                        }
                    }
                    // Un tuple incomplet n'est jamais un doublon.
                    continue;
                }
                if matches!(examen.contrainte, TypeConstraint::NotNull(_)) {
                    continue;
                }

                // Unicité : le tuple a-t-il déjà été vu ?
                let tuple: Vec<&Value> = examen.rangs.iter().filter_map(|&r| lue(r)).collect();
                let mut hachage = hacher_tuple(&tuple);
                loop {
                    let Some(&premier) = examen.vus.get(&hachage) else {
                        examen.vus.insert(hachage, noeud);
                        break;
                    };
                    // Confirmation sur les valeurs : le premier porteur est
                    // relu (chemin rare — il n'est pris que sur un doublon
                    // ou une collision de hachage).
                    let du_premier: Vec<Value> = examen
                        .rangs
                        .iter()
                        .filter_map(|&r| store.get_node_property(premier, &cles[r]))
                        .collect();
                    if tuples_egaux(&tuple, &du_premier) {
                        let repete: Vec<Value> = tuple.iter().map(|v| (*v).clone()).collect();
                        rapport.signaler(plafond, || {
                            examen.violation(noeud, Some(premier), repete, None)
                        });
                        if arret_a_la_premiere {
                            return;
                        }
                        break;
                    }
                    // Collision : même seau, tuples différents. On sonde le
                    // seau suivant.
                    hachage = hachage.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
                }
            }
        }
    }
}

impl super::ObrainDB {
    /// Vérifie que les contraintes déclarées (`UNIQUE`, `NODE KEY`,
    /// `NOT NULL`) tiennent sur les données de la base.
    ///
    /// À appeler après une écriture qui ne passe pas par les opérateurs du
    /// langage — API directe du store, import en masse — et qui, par contrat,
    /// n'applique AUCUNE contrainte. Le rapport décrit au plus
    /// [`VIOLATIONS_DETAILLEES_PAR_DEFAUT`] violations et les compte toutes.
    ///
    /// Coût : une passe sur les nœuds de chaque label contraint, sans
    /// recherche d'index (voir la documentation du module).
    #[must_use]
    pub fn verifier_contraintes(&self) -> RapportContraintes {
        self.verifier_contraintes_plafonnees(VIOLATIONS_DETAILLEES_PAR_DEFAUT)
    }

    /// [`Self::verifier_contraintes`] en choisissant le nombre de violations
    /// décrites dans le rapport.
    #[must_use]
    pub fn verifier_contraintes_plafonnees(&self, plafond: usize) -> RapportContraintes {
        let mut rapport = RapportContraintes::default();
        let store = self.graph_store();
        let mut labels = self.catalog.all_node_type_names();
        labels.sort();
        for label in labels {
            // Le type RÉSOLU : les contraintes héritées des types parents
            // s'appliquent aux nœuds de ce label.
            let Some(mut def) = self.catalog.resolved_node_type(&label) else {
                continue;
            };
            def.normalize_constraints();
            verifier_type(
                store.as_ref() as &dyn GraphStore,
                &def,
                None,
                plafond,
                false,
                &mut rapport,
            );
        }
        rapport
    }
}

impl super::ObrainDB {
    /// Liste les violations qu'aurait une contrainte `UNIQUE (propriétés)` sur
    /// `label` SANS la déclarer : le diagnostic d'une migration (« que faut-il
    /// nettoyer avant de pouvoir déclarer cette contrainte ? »), là où
    /// `CREATE CONSTRAINT` s'arrête à la première violation.
    ///
    /// Au plus `plafond` violations sont décrites, toutes sont comptées.
    #[must_use]
    pub fn verifier_contrainte_hypothetique(
        &self,
        label: &str,
        proprietes: &[&str],
        plafond: usize,
    ) -> RapportContraintes {
        let def = NodeTypeDefinition {
            name: label.to_string(),
            properties: Vec::new(),
            constraints: vec![TypeConstraint::Unique(
                proprietes.iter().map(|p| (*p).to_string()).collect(),
            )],
            constraint_names: Vec::new(),
            parent_types: Vec::new(),
            implicit: false,
        };
        let mut rapport = RapportContraintes::default();
        let store = self.graph_store();
        verifier_type(
            store.as_ref() as &dyn GraphStore,
            &def,
            None,
            plafond,
            false,
            &mut rapport,
        );
        rapport
    }
}
