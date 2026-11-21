//! Index spatiaux vus par la SESSION — DDL, procédures `CALL`, planificateur.
//!
//! ── POURQUOI UNE POIGNÉE, ET PAS UN ACCÈS À `ObrainDB` ─────────────────────
//!
//! Le DDL GQL, les procédures `CALL` et le planificateur vivent dans `Session`,
//! qui ne détient pas d'`Arc<ObrainDB>` — et ne doit pas en détenir : la base
//! tient le store, la session tient le store, une poignée forte vers la base
//! fermerait le cycle et empêcherait la libération. La poignée porte donc les
//! CLONES `Arc` dont elle a besoin, exactement comme `PoigneeRechercheHybride`
//! et `PoigneeIndexVectoriels`.
//!
//! ── CE QU'ELLE EXPOSE, ET CE QU'ELLE N'EXPOSE PAS ──────────────────────────
//!
//! Six opérations, pas l'API entière : créer, supprimer, rayon, kNN, lister,
//! résoudre. Une poignée qui exposerait `ObrainDB` entière rendrait la
//! frontière décorative — et la frontière EST le sujet : la session ne peut
//! toucher que ce qu'un index spatial sait faire.

use std::collections::HashMap;
use std::sync::Arc;

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_common::utils::error::{Error, Result};
use parking_lot::RwLock;

use crate::query::plan::LogicalExpression;
use obrain_core::graph::GraphStore as _;
use obrain_core::index::geo::point_lon_lat;
use obrain_core::index::spatial::SpatialIndex;

/// Le registre partagé entre la base, l'observateur et les poignées.
///
/// ── POURQUOI UN CACHE DE NOMS ──────────────────────────────────────────────
///
/// Le catalogue durable est un FICHIER : `SubstrateStore::noms_index()` appelle
/// `lire_catalogue()`, qui fait un `std::fs::read_to_string` et reparse toutes
/// les déclarations. Résoudre un nom d'index par cette voie à chaque requête
/// coûterait une lecture disque — l'exact contraire de la cible
/// (microsecondes). Le cache ne vit que tant que rien ne change : il est
/// invalidé à chaque création, renommage et suppression.
#[derive(Default)]
pub(crate) struct RegistreSpatial {
    index: RwLock<HashMap<String, Arc<RwLock<SpatialIndex>>>>,
    noms: RwLock<Option<Vec<(String, String, String)>>>,
}

impl RegistreSpatial {
    pub(crate) fn nouveau() -> Self {
        Self::default()
    }

    /// L'index d'une clé `label:propriété`, s'il est chargé.
    pub(crate) fn get(&self, cle: &str) -> Option<Arc<RwLock<SpatialIndex>>> {
        self.index.read().get(cle).cloned()
    }

    /// L'index d'une clé, construit par `construire` s'il n'est pas chargé.
    /// Voir [`Self::sous_verrou`].
    pub(crate) fn construire_sous_verrou(
        &self,
        cle: &str,
        construire: impl FnOnce() -> SpatialIndex,
    ) -> Arc<RwLock<SpatialIndex>> {
        self.sous_verrou(cle, false, construire)
    }

    /// Construit par `construire` et REMPLACE l'index d'une clé (un
    /// `CREATE INDEX` explicite rebâtit). Voir [`Self::sous_verrou`].
    pub(crate) fn rebatir_sous_verrou(
        &self,
        cle: &str,
        construire: impl FnOnce() -> SpatialIndex,
    ) -> Arc<RwLock<SpatialIndex>> {
        self.sous_verrou(cle, true, construire)
    }

    /// Construction et publication SOUS LE VERROU D'ÉCRITURE du registre, du
    /// balayage à la publication.
    ///
    /// ── POURQUOI SOUS VERROU (revue de #282, 21/11/2026) ──────────────────
    ///
    /// Construire hors verrou, puis publier, laisse une fenêtre : une mutation
    /// dont le balayage a lu l'ANCIENNE valeur, et dont la notification arrive
    /// AVANT la publication, ne trouve aucun index à mettre à jour
    /// (`tous_avec_cle()` ne le voit pas encore). L'index publié porte alors
    /// l'ancienne valeur, jusqu'au prochain rebâti.
    ///
    /// Le substrat appelle ses observateurs HORS de ses propres verrous, après
    /// l'écriture (`SubstrateStore::observateurs`). Tenir ce verrou pendant la
    /// construction ne peut donc pas interbloquer : la mutation s'écrit, puis
    /// sa notification ATTEND la publication, puis relit la valeur vivante et
    /// l'applique. Si la notification a lu le registre avant notre prise du
    /// verrou, l'écriture la précède, donc notre balayage (postérieur à la
    /// prise) la voit. Dans les deux cas l'index publié est au niveau du
    /// store. Test : `une_mutation_pendant_la_construction_nest_pas_perdue`.
    ///
    /// Coût assumé : pendant la construction (648 ms à 10⁶ points au banc),
    /// toute requête spatiale et toute notification de mutation attendent.
    /// C'est le prix d'un DDL ou d'une première lecture après réouverture, pas
    /// d'une requête ordinaire.
    fn sous_verrou(
        &self,
        cle: &str,
        remplacer: bool,
        construire: impl FnOnce() -> SpatialIndex,
    ) -> Arc<RwLock<SpatialIndex>> {
        let mut index = self.index.write();
        if !remplacer && let Some(existant) = index.get(cle) {
            // Un autre constructeur a pu passer pendant qu'on attendait.
            return Arc::clone(existant);
        }
        let construit = Arc::new(RwLock::new(construire()));
        #[cfg(test)]
        crochet_avant_publication(cle);
        index.insert(cle.to_string(), Arc::clone(&construit));
        drop(index);
        self.invalider_noms();
        construit
    }

    /// Retire l'index d'une clé. Rend `true` s'il était chargé.
    pub(crate) fn retirer(&self, cle: &str) -> bool {
        let retire = self.index.write().remove(cle).is_some();
        self.invalider_noms();
        retire
    }

    /// Les index dont la clé commence par `prefixe` (typiquement `label:`),
    /// avec leur clé. Copie des `Arc` : l'appelant travaille hors du verrou.
    pub(crate) fn pour_label(&self, prefixe: &str) -> Vec<(String, Arc<RwLock<SpatialIndex>>)> {
        self.index
            .read()
            .iter()
            .filter(|(cle, _)| cle.starts_with(prefixe))
            .map(|(cle, i)| (cle.clone(), Arc::clone(i)))
            .collect()
    }

    /// Tous les index chargés, avec leur clé (copie des `Arc`).
    pub(crate) fn tous_avec_cle(&self) -> Vec<(String, Arc<RwLock<SpatialIndex>>)> {
        self.index
            .read()
            .iter()
            .map(|(cle, i)| (cle.clone(), Arc::clone(i)))
            .collect()
    }

    /// Tous les index chargés (copie des `Arc`).
    pub(crate) fn tous(&self) -> Vec<Arc<RwLock<SpatialIndex>>> {
        self.index.read().values().map(Arc::clone).collect()
    }

    /// Les noms d'index spatiaux, `(nom, label, propriété)`, triés.
    ///
    /// Le PREMIER appel lit le catalogue (fichier) ; les suivants lisent le
    /// cache, jusqu'à la prochaine création, renommage ou suppression.
    pub(crate) fn noms(
        &self,
        sub: Option<&obrain_substrate::SubstrateStore>,
    ) -> Vec<(String, String, String)> {
        if let Some(cache) = self.noms.read().as_ref() {
            return cache.clone();
        }
        let Some(sub) = sub else {
            return Vec::new();
        };
        let mut noms: Vec<(String, String, String)> = sub
            .noms_index()
            .into_iter()
            .filter(|(_, d)| d.genre == "spatial")
            .map(|(n, d)| (n, d.label, d.propriete))
            .collect();
        noms.sort();
        *self.noms.write() = Some(noms.clone());
        noms
    }

    /// Oublie le cache de noms (appelé à chaque changement du catalogue).
    pub(crate) fn invalider_noms(&self) {
        *self.noms.write() = None;
    }
}

/// Ce que la session peut faire d'un index spatial.
///
/// Tout est en MICROSECONDES côté index (1,8 µs mesurés à 10⁶ points) ; ce qui
/// peut coûter cher est le premier appel après une ouverture, qui reconstruit
/// l'arbre (648 ms à 10⁶) — une fois, paresseusement.
pub trait IndexSpatiaux: Send + Sync {
    /// Construit l'index `:label(propriété)` et le nomme. Rend le nombre de
    /// points indexés.
    ///
    /// # Errors
    ///
    /// Nom déjà porté par un autre index spatial, ou catalogue illisible.
    fn creer(&self, nom: &str, label: &str, propriete: &str) -> Result<usize>;

    /// Retire un index (par son nom ou `label:propriété`).
    ///
    /// # Errors
    ///
    /// Nom inconnu — une erreur qui le nomme, jamais un `false` muet.
    fn supprimer(&self, nom_ou_ident: &str) -> Result<bool>;

    /// Les nœuds à moins de `rayon_m` mètres, par distance croissante.
    ///
    /// # Errors
    ///
    /// Index inconnu.
    fn rayon(
        &self,
        nom_ou_ident: &str,
        lon: f64,
        lat: f64,
        rayon_m: f64,
    ) -> Result<Vec<(NodeId, f64)>>;

    /// Les `k` nœuds les plus proches, par distance croissante.
    ///
    /// # Errors
    ///
    /// Index inconnu.
    fn plus_proches(
        &self,
        nom_ou_ident: &str,
        lon: f64,
        lat: f64,
        k: usize,
    ) -> Result<Vec<(NodeId, f64)>>;

    /// Nombre de nœuds du label dont la propriété porte une valeur non nulle
    /// qui N'EST PAS un point. Tant qu'il n'est pas nul, le balayage GQL
    /// refuserait `distance()` : le planificateur doit se replier sur lui.
    ///
    /// # Errors
    ///
    /// Index inconnu.
    fn non_points(&self, nom_ou_ident: &str) -> Result<usize>;

    /// Les index nommés, `(nom, label, propriété)`, triés par nom.
    fn noms(&self) -> Vec<(String, String, String)>;

    /// `(label, propriété)` d'un nom ou d'un identifiant `label:propriété`.
    ///
    /// # Errors
    ///
    /// Ni un nom connu ni l'identifiant d'un index déclaré.
    fn resoudre(&self, nom_ou_ident: &str) -> Result<(String, String)>;
}

/// Ce qu'un balayage du label trouve sur la propriété : les points, et les
/// identifiants dont la valeur n'est PAS un point.
pub(crate) struct Collecte {
    pub(crate) points: Vec<(u32, f64, f64)>,
    /// Valeur NON NULLE qui n'est pas un point (`"oops"`, trois nombres…).
    /// Le balayage GQL refuse `distance()` dessus ; l'index doit le savoir.
    pub(crate) non_points: Vec<u32>,
}

/// Balaie un label sur une propriété.
///
/// UNE seule définition, utilisée par la base ET par la poignée : deux scans
/// divergeraient sur ce qu'est un point indexable, et l'index contiendrait
/// alors autre chose que ce que la requête interroge. `Null` et l'absence ne
/// sont ni un point ni une faute (le balayage les écarte en silence) ; toute
/// autre valeur non point est une faute que le balayage REFUSE.
pub(crate) fn collecter_points(
    store: &dyn obrain_core::graph::GraphStoreMut,
    label: &str,
    property: &str,
) -> Collecte {
    let pk = PropertyKey::new(property);
    let mut points: Vec<(u32, f64, f64)> = Vec::new();
    let mut non_points: Vec<u32> = Vec::new();
    for &node_id in &store.nodes_by_label(label) {
        let Some(v) = store.get_node_property(node_id, &pk) else {
            continue;
        };
        if matches!(v, Value::Null) {
            continue;
        }
        match point_lon_lat(&v) {
            Some((lon, lat)) => points.push((node_id.0 as u32, lon, lat)),
            None => non_points.push(node_id.0 as u32),
        }
    }
    Collecte { points, non_points }
}

/// Construit l'index d'un label sur une propriété, non-points compris.
pub(crate) fn construire_index(
    store: &dyn obrain_core::graph::GraphStoreMut,
    label: &str,
    property: &str,
) -> SpatialIndex {
    let Collecte { points, non_points } = collecter_points(store, label, property);
    let mut index = SpatialIndex::build(points);
    for id in non_points {
        index.marquer_non_point(id);
    }
    index
}

/// Applique la valeur VIVANTE d'une propriété à un index : point → inséré ;
/// absent ou `Null` → retiré (le balayage l'écarte en silence) ; toute autre
/// valeur → marquée non-point (le balayage la REFUSE, l'index doit le savoir).
fn appliquer_valeur(index: &mut SpatialIndex, id: u32, valeur: Option<&Value>) {
    match valeur {
        None | Some(Value::Null) => {
            index.retirer(id);
        }
        Some(v) => match point_lon_lat(v) {
            Some((lon, lat)) => {
                index.inserer(id, lon, lat);
            }
            None => index.marquer_non_point(id),
        },
    }
}

/// Crochet de test : appelé par `RegistreSpatial::sous_verrou` entre la
/// construction et la publication, verrou tenu. Le test qui en a besoin le
/// pose pour UNE clé ; les autres clés ne le déclenchent pas (les tests
/// tournent en parallèle dans le même binaire).
#[cfg(test)]
#[allow(clippy::type_complexity)]
static CROCHET_AVANT_PUBLICATION: parking_lot::Mutex<Option<(String, Box<dyn FnOnce() + Send>)>> =
    parking_lot::Mutex::new(None);

#[cfg(test)]
fn crochet_avant_publication(cle: &str) {
    let pris = {
        let mut slot = CROCHET_AVANT_PUBLICATION.lock();
        match slot.as_ref() {
            Some((c, _)) if c == cle => slot.take(),
            _ => None,
        }
    };
    if let Some((_, f)) = pris {
        f();
    }
}

/// La poignée elle-même : les `Arc` de la base, sans la base.
pub(crate) struct PoigneeIndexSpatiaux {
    substrate_store: Option<Arc<obrain_substrate::SubstrateStore>>,
    store: Arc<dyn obrain_core::graph::GraphStoreMut>,
    spatiaux: Arc<RegistreSpatial>,
}

impl PoigneeIndexSpatiaux {
    pub(crate) fn nouvelle(
        substrate_store: Option<Arc<obrain_substrate::SubstrateStore>>,
        store: Arc<dyn obrain_core::graph::GraphStoreMut>,
        spatiaux: Arc<RegistreSpatial>,
    ) -> Self {
        Self {
            substrate_store,
            store,
            spatiaux,
        }
    }

    /// L'index d'un nom, chargé ou reconstruit À LA DEMANDE.
    fn index(&self, nom_ou_ident: &str) -> Result<Arc<RwLock<SpatialIndex>>> {
        let (label, propriete) = self.resoudre(nom_ou_ident)?;
        let cle = format!("{label}:{propriete}");
        if let Some(idx) = self.spatiaux.get(&cle) {
            return Ok(idx);
        }
        Ok(self
            .spatiaux
            .construire_sous_verrou(&cle, || construire_index(&*self.store, &label, &propriete)))
    }
}

impl IndexSpatiaux for PoigneeIndexSpatiaux {
    fn creer(&self, nom: &str, label: &str, propriete: &str) -> Result<usize> {
        if let Some((l, p)) = self
            .noms()
            .into_iter()
            .find(|(n, _, _)| n == nom)
            .map(|(_, l, p)| (l, p))
            && !(l == label && p == propriete)
        {
            return Err(Error::Internal(format!(
                "CREATE INDEX '{nom}' : ce nom désigne déjà l'index spatial :{l}({p})"
            )));
        }
        // Refuser AVANT de poser quoi que ce soit en mémoire : un refus qui
        // laisse un index chargé n'est pas un refus.
        let Some(ref sub) = self.substrate_store else {
            return Err(Error::Internal(
                "CREATE INDEX ... SPATIAL : un index n'existe que sur une base substrat \
                 (une base externe n'a pas de catalogue durable)."
                    .to_string(),
            ));
        };
        let idx = self
            .spatiaux
            .rebatir_sous_verrou(&format!("{label}:{propriete}"), || {
                construire_index(&*self.store, label, propriete)
            });
        let n = idx.read().len();
        // Déclaration DURABLE, puis NOM : l'ordre compte — `nommer_index` refuse
        // un nom sur une déclaration absente, et c'est ce qu'on veut (un nom ne
        // désigne jamais un index qui n'existe pas).
        sub.declarer_index(obrain_substrate::DeclarationIndex {
            genre: "spatial".to_string(),
            label: label.to_string(),
            propriete: propriete.to_string(),
            dimension: Some(2),
            metrique: Some("haversine".to_string()),
        })
        .map_err(|e| {
            Error::Internal(format!("CREATE INDEX '{nom}' : déclaration refusée : {e}"))
        })?;
        sub.nommer_index("spatial", label, propriete, nom)
            .map_err(|e| Error::Internal(format!("CREATE INDEX '{nom}' : nom refusé : {e}")))?;
        Ok(n)
    }

    fn supprimer(&self, nom_ou_ident: &str) -> Result<bool> {
        let (label, propriete) = self.resoudre(nom_ou_ident)?;
        let retire = self.spatiaux.retirer(&format!("{label}:{propriete}"));
        if let Some(ref sub) = self.substrate_store {
            // La déclaration part MÊME si l'index n'était pas en mémoire : sur
            // une base rouverte il ne l'est pas encore, et un DROP doit valoir.
            if let Err(e) = sub.retirer_declaration_index("spatial", &label, &propriete) {
                tracing::warn!(erreur = %e, "DROP INDEX : retrait de déclaration non persisté");
            }
        }
        Ok(retire)
    }

    fn rayon(
        &self,
        nom_ou_ident: &str,
        lon: f64,
        lat: f64,
        rayon_m: f64,
    ) -> Result<Vec<(NodeId, f64)>> {
        let idx = self.index(nom_ou_ident)?;
        let garde = idx.read();
        Ok(garde
            .dans_rayon(lon, lat, rayon_m)
            .into_iter()
            .map(|(id, d)| (NodeId(u64::from(id)), d))
            .collect())
    }

    fn plus_proches(
        &self,
        nom_ou_ident: &str,
        lon: f64,
        lat: f64,
        k: usize,
    ) -> Result<Vec<(NodeId, f64)>> {
        let idx = self.index(nom_ou_ident)?;
        let garde = idx.read();
        Ok(garde
            .plus_proches(lon, lat, k)
            .into_iter()
            .map(|(id, d)| (NodeId(u64::from(id)), d))
            .collect())
    }

    fn non_points(&self, nom_ou_ident: &str) -> Result<usize> {
        Ok(self.index(nom_ou_ident)?.read().nombre_non_points())
    }

    fn noms(&self) -> Vec<(String, String, String)> {
        self.spatiaux.noms(self.substrate_store.as_deref())
    }

    fn resoudre(&self, nom_ou_ident: &str) -> Result<(String, String)> {
        if let Some((_, l, p)) = self.noms().into_iter().find(|(n, _, _)| n == nom_ou_ident) {
            return Ok((l, p));
        }
        // ── UN IDENTIFIANT N'EST ACCEPTÉ QUE S'IL EST DÉCLARÉ (2026-11-21) ──
        //
        // AVANT : tout `label:propriété` lisible était accepté, et `index()`
        // bâtissait alors un index en mémoire, silencieusement, au premier
        // appel. Le planificateur interroge par identifiant : un simple
        // `WHERE distance(n.pos, point(..)) <= r` sur une base SANS
        // `CREATE INDEX` bâtissait donc un index implicite (136 ms mesurés à
        // 10⁵ nœuds), non durable, invisible de `SHOW INDEXES`, et faussait
        // toute mesure « sans index » (30 µs au lieu de 28 ms). Un index
        // existe parce qu'un DDL l'a déclaré, jamais parce qu'une lecture
        // l'a frôlé. La construction paresseuse reste légitime pour UN cas :
        // un index déclaré mais pas encore chargé (réouverture).
        if let Some((l, p)) = nom_ou_ident.split_once(':')
            && self
                .noms()
                .into_iter()
                .any(|(_, dl, dp)| dl == l && dp == p)
        {
            return Ok((l.to_string(), p.to_string()));
        }
        Err(Error::Internal(format!(
            "index spatial inconnu : '{nom_ou_ident}' — index déclarés : [{}]",
            self.noms()
                .into_iter()
                .map(|(n, l, p)| format!("{n} = :{l}({p})"))
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }
}

/// ── L'OBSERVATEUR D'INDEX SPATIAUX ─────────────────────────────────────────
///
/// Installé sur le store à l'ouverture, comme celui du vectoriel, et pour la
/// même raison : le store prévient depuis ses points UNIQUES d'écriture, donc
/// TOUS les chemins — API Rust, GQL `SET`/`CREATE`/`MERGE` — entretiennent
/// l'index, sans crochet dans les opérateurs.
///
/// Il ne détient qu'un `Weak` du store : le store le détient déjà, et un `Arc`
/// ferait un cycle qui empêcherait la libération (la même leçon que
/// `ObservateurIndexVectoriel`).
///
/// Une ÉCRITURE QUI N'EST PLUS UN POINT retire le nœud de l'index. Sans cela,
/// il resterait à son ancienne position : il répondrait à une requête sur un
/// lieu où il n'est plus, ce qui est pire que de l'oublier.
pub(crate) struct ObservateurIndexSpatial {
    registre: Arc<RegistreSpatial>,
    store: std::sync::Weak<obrain_substrate::SubstrateStore>,
}

impl ObservateurIndexSpatial {
    /// Construit l'observateur (le store est pris en `Weak`).
    #[must_use]
    pub(crate) fn nouveau(
        registre: Arc<RegistreSpatial>,
        store: &Arc<obrain_substrate::SubstrateStore>,
    ) -> Self {
        Self {
            registre,
            store: Arc::downgrade(store),
        }
    }

    /// Les index concernés par la propriété `key` : `(label, index)`.
    fn concernes_par_propriete(&self, key: &str) -> Vec<(String, Arc<RwLock<SpatialIndex>>)> {
        let suffixe = format!(":{key}");
        self.registre
            .tous_avec_cle()
            .into_iter()
            .filter(|(cle, _)| cle.ends_with(&suffixe))
            .filter_map(|(cle, idx)| {
                let label = cle.split_once(':').map(|(l, _)| l.to_string())?;
                Some((label, idx))
            })
            .collect()
    }

    /// Met à jour l'index pour `(id, propriété)` depuis la valeur VIVANTE.
    fn rafraichir(&self, id: obrain_common::types::NodeId, key: &str) {
        let Some(store) = self.store.upgrade() else {
            return;
        };
        let concernes = self.concernes_par_propriete(key);
        if concernes.is_empty() {
            return;
        }
        let Some(noeud) = store.get_node(id) else {
            return;
        };
        let valeur = store.get_node_property(id, &PropertyKey::new(key));
        for (label, idx) in concernes {
            // Le nœud doit porter le LABEL de l'index : `:Lieu(pos)` ne doit pas
            // recevoir un `:Batiment(pos)` simplement parce que la propriété
            // s'appelle pareil.
            if !noeud.labels.iter().any(|l| l.as_str() == label) {
                continue;
            }
            appliquer_valeur(&mut idx.write(), id.0 as u32, valeur.as_ref());
        }
    }
}

impl obrain_substrate::StoreMutationObserver for ObservateurIndexSpatial {
    fn node_created(&self, _id: obrain_common::types::NodeId, _epoch: u64) {}

    fn edge_created(
        &self,
        _id: obrain_common::types::EdgeId,
        _src: obrain_common::types::NodeId,
        _dst: obrain_common::types::NodeId,
        _epoch: u64,
    ) {
    }

    fn node_property_set(&self, id: obrain_common::types::NodeId, key: &str) {
        self.rafraichir(id, key);
    }

    fn node_property_removed(&self, id: obrain_common::types::NodeId, key: &str) {
        for (_, idx) in self.concernes_par_propriete(key) {
            idx.write().retirer(id.0 as u32);
        }
    }

    fn node_deleted(&self, id: obrain_common::types::NodeId) {
        for idx in self.registre.tous() {
            idx.write().retirer(id.0 as u32);
        }
    }

    /// `SET n:Lieu` sur un nœud qui porte déjà `pos` : il entre dans
    /// `:Lieu(pos)` avec la valeur qu'il porte (revue de #282 : l'index
    /// l'ignorait, le balayage le rendait).
    fn node_label_added(&self, id: obrain_common::types::NodeId, label: &str) {
        let Some(store) = self.store.upgrade() else {
            return;
        };
        for (cle, idx) in self.registre.pour_label(&format!("{label}:")) {
            let Some((_, propriete)) = cle.split_once(':') else {
                continue;
            };
            let valeur = store.get_node_property(id, &PropertyKey::new(propriete));
            appliquer_valeur(&mut idx.write(), id.0 as u32, valeur.as_ref());
        }
    }

    /// `REMOVE n:Lieu` : le nœud sort des index de ce label, quelle que soit
    /// sa position.
    fn node_label_removed(&self, id: obrain_common::types::NodeId, label: &str) {
        for (_, idx) in self.registre.pour_label(&format!("{label}:")) {
            idx.write().retirer(id.0 as u32);
        }
    }
}

// ── PROCÉDURES `CALL` ──────────────────────────────────────────────────────
//
// Deux procédures, pas une : l'index sait faire deux choses, et les cacher
// derrière un argument optionnel obligerait l'appelant à se souvenir de l'ordre
// des paramètres pour dire laquelle. `YIELD node, distance` dans les deux cas —
// `distance` est en MÈTRES, jamais un score : un utilisateur peut la vérifier
// à la règle sur une carte.

/// `CALL obrain.spatial.nearest(index, lon, lat, k) YIELD node, distance`.
pub const NOM_NEAREST: &str = "obrain.spatial.nearest";

/// `CALL obrain.spatial.within(index, lon, lat, rayon_m) YIELD node, distance`.
pub const NOM_WITHIN: &str = "obrain.spatial.within";

/// Colonnes de sortie : `node` (un nœud) et `distance` (Float64, en mètres).
pub const COLONNES: [&str; 2] = ["node", "distance"];

/// Colonnes qui portent un NŒUD (et non un scalaire) : `YIELD node AS n
/// WHERE n.status = …` doit y lire des propriétés.
pub const COLONNES_NOEUD: [&str; 1] = ["node"];

/// Ce nom désigne-t-il une de nos procédures, avec ou sans le préfixe
/// `obrain.` ? Correspondance EXACTE, comme `db.index.vector.queryNodes`.
#[must_use]
pub fn est_procedure_spatiale(resolved_name: &str) -> bool {
    resolved_name == NOM_NEAREST
        || resolved_name == NOM_WITHIN
        || resolved_name == "spatial.nearest"
        || resolved_name == "spatial.within"
}

/// Exécute `CALL obrain.spatial.{nearest,within}(...)` si le nom matche.
///
/// `Ok(None)` si le nom n'est pas une des deux.
///
/// # Errors
///
/// Poignée absente, arguments mal formés ou en mauvais nombre, index inconnu
/// (erreur qui le nomme), ou point invalide.
pub fn try_execute_spatial_procedure(
    resolved_name: &str,
    arguments: &[LogicalExpression],
    poignee: Option<&Arc<dyn IndexSpatiaux>>,
) -> Result<Option<obrain_adapters::plugins::AlgorithmResult>> {
    use obrain_adapters::plugins::AlgorithmResult;

    let fenetre = resolved_name.ends_with("within");
    let nom = if fenetre { NOM_WITHIN } else { NOM_NEAREST };
    if !est_procedure_spatiale(resolved_name) {
        return Ok(None);
    }
    let Some(poignee) = poignee else {
        return Err(Error::Internal(format!(
            "{nom} : aucune base attachée à cette session (poignée d'index spatiaux absente)"
        )));
    };
    if arguments.len() != 4 {
        return Err(Error::Internal(format!(
            "{nom} attend 4 arguments : (index_name, lon, lat, {}), reçu {}",
            if fenetre { "rayon_m" } else { "k" },
            arguments.len()
        )));
    }
    let valeurs = arguments
        .iter()
        .map(|a| constante(nom, a))
        .collect::<Result<Vec<Value>>>()?;
    let index = match &valeurs[0] {
        Value::String(s) => s.to_string(),
        autre => {
            return Err(Error::Internal(format!(
                "{nom} : argument 0 (index_name) doit être une chaîne, reçu {autre:?}"
            )));
        }
    };
    let lon = nombre(nom, &valeurs[1], "lon")?;
    let lat = nombre(nom, &valeurs[2], "lat")?;
    if !obrain_core::index::geo::latitude_valide(lat) {
        return Err(Error::Internal(format!(
            "{nom} : latitude {lat} hors de [-90, 90]"
        )));
    }
    let dernier = nombre(nom, &valeurs[3], if fenetre { "rayon_m" } else { "k" })?;

    let lignes = if fenetre {
        poignee.rayon(&index, lon, lat, dernier)?
    } else {
        if dernier < 0.0 || dernier.fract() != 0.0 {
            return Err(Error::Internal(format!(
                "{nom} : k doit être un entier positif, reçu {dernier}"
            )));
        }
        poignee.plus_proches(&index, lon, lat, dernier as usize)?
    };

    let mut result = AlgorithmResult::new(COLONNES.iter().map(|c| (*c).to_string()).collect());
    for (id, distance) in lignes {
        result.add_row(vec![Value::Int64(id.0 as i64), Value::Float64(distance)]);
    }
    Ok(Some(result))
}

/// Les lignes de `obrain.procedures()` pour les deux procédures.
#[must_use]
pub fn lignes_procedures() -> Vec<Vec<Value>> {
    vec![
        vec![
            Value::from(NOM_NEAREST),
            Value::from(
                "Spatial kNN: the k nearest nodes of a point in the named spatial index, by \
                 great-circle distance (metres, ascending)",
            ),
            Value::from("index_name (String), lon (Float64), lat (Float64), k (Int64)"),
            Value::from("node, distance"),
        ],
        vec![
            Value::from(NOM_WITHIN),
            Value::from(
                "Spatial window: every node within radius_m metres of a point in the named \
                 spatial index (great-circle distance, metres, ascending)",
            ),
            Value::from("index_name (String), lon (Float64), lat (Float64), rayon_m (Float64)"),
            Value::from("node, distance"),
        ],
    ]
}

/// Évalue un argument constant (les paramètres `$x` sont déjà substitués).
///
/// On n'appelle PAS `eval_constant_expression` du planificateur : il est gaté
/// sur une feature absente de cette compilation, et les arguments d'une
/// procédure spatiale sont des littéraux (nom, deux coordonnées, un nombre).
/// Un argument plus exotique rend une erreur qui dit quoi écrire, pas un
/// « fonction inconnue » à trois couches de distance.
fn constante(nom: &str, expr: &LogicalExpression) -> Result<Value> {
    match expr {
        LogicalExpression::Literal(v) => Ok(v.clone()),
        LogicalExpression::Unary {
            op: crate::query::plan::UnaryOp::Neg,
            operand,
        } => match constante(nom, operand)? {
            Value::Float64(f) => Ok(Value::Float64(-f)),
            Value::Int64(i) => Ok(Value::Int64(-i)),
            autre => Err(Error::Internal(format!(
                "{nom} : un moins unaire ne s'applique pas à {autre:?}"
            ))),
        },
        LogicalExpression::Parameter(p) => Err(Error::Internal(format!(
            "{nom} : paramètre ${p} non fourni"
        ))),
        autre => Err(Error::Internal(format!(
            "{nom} : les arguments doivent être des constantes littérales \
             (index_name, lon, lat, k|rayon_m), reçu {autre:?}"
        ))),
    }
}

/// Un argument numérique, avec un message qui nomme le paramètre fautif.
fn nombre(nom: &str, v: &Value, quel: &str) -> Result<f64> {
    obrain_core::index::geo::nombre_f64(v).ok_or_else(|| {
        Error::Internal(format!(
            "{nom} : argument « {quel} » doit être un nombre, reçu {v:?}"
        ))
    })
}

#[cfg(test)]
mod tests {
    //! La course « balayage, puis publication » (hypothèse de la revue de
    //! #282), rendue DÉTERMINISTE par `CROCHET_AVANT_PUBLICATION`.
    use super::*;
    use crate::ObrainDB;

    fn point(lon: f64, lat: f64) -> Value {
        Value::List(vec![Value::Float64(lon), Value::Float64(lat)].into())
    }

    /// Scénario : le DDL a balayé (ancienne position), tient le verrou du
    /// registre, et n'a pas encore publié. Un écrivain déplace le nœud à cet
    /// instant. Sa notification doit ATTENDRE la publication, puis
    /// s'appliquer : l'index publié rend la NOUVELLE position. Sans le verrou,
    /// la notification ne trouvait aucun index et l'ancienne position restait
    /// (vu rouge sur la variante « construire hors verrou »).
    #[test]
    fn une_mutation_pendant_la_construction_nest_pas_perdue() {
        let db = Arc::new(ObrainDB::new_in_memory());
        let id = db.create_node_with_props(&["LieuCourse"], [("pos", point(2.0, 48.0))]);
        let (tx, rx) = std::sync::mpsc::channel::<std::thread::JoinHandle<()>>();
        {
            let db = Arc::clone(&db);
            *CROCHET_AVANT_PUBLICATION.lock() = Some((
                "LieuCourse:pos".to_string(),
                Box::new(move || {
                    // L'écrivain : il écrit (le substrat n'est pas verrouillé),
                    // puis sa notification bloque sur le registre jusqu'à la
                    // publication. On ne le JOINT pas ici : ce serait
                    // l'interblocage que le verrou rend impossible à ignorer.
                    let ecrivain = {
                        let db = Arc::clone(&db);
                        std::thread::spawn(move || {
                            db.set_node_property(id, "pos", point(3.0, 49.0));
                        })
                    };
                    // Attendre que l'ÉCRITURE soit visible dans le store : dès
                    // lors, l'écrivain est au plus tard dans sa notification,
                    // et le balayage du DDL (déjà fait) a lu l'ancienne valeur.
                    let pk = PropertyKey::new("pos");
                    for _ in 0..1_000_000 {
                        let vivante = db
                            .data_store()
                            .get_node_property(id, &pk)
                            .as_ref()
                            .and_then(point_lon_lat);
                        if vivante == Some((3.0, 49.0)) {
                            break;
                        }
                        std::thread::yield_now();
                    }
                    tx.send(ecrivain).expect("canal");
                }),
            ));
        }
        db.execute("CREATE INDEX course_pos FOR (n:LieuCourse) ON (n.pos) USING SPATIAL")
            .expect("DDL");
        let ecrivain = rx.recv().expect("le crochet a tourné");
        ecrivain.join().expect("écrivain");
        // La vérité : le balayage (point en premier n'est pas routé).
        let scan = db
            .execute("MATCH (n:LieuCourse) WHERE distance(point(3.0, 49.0), n.pos) <= 1 RETURN n")
            .expect("balayage");
        let index = db
            .execute("MATCH (n:LieuCourse) WHERE distance(n.pos, point(3.0, 49.0)) <= 1 RETURN n")
            .expect("index");
        assert_eq!(scan.rows.len(), 1);
        assert_eq!(
            index.rows.len(),
            1,
            "la position écrite pendant la construction est perdue"
        );
        let ancienne = db
            .execute("MATCH (n:LieuCourse) WHERE distance(n.pos, point(2.0, 48.0)) <= 1 RETURN n")
            .expect("index");
        assert_eq!(
            ancienne.rows.len(),
            0,
            "l'ancienne position ne doit plus être servie"
        );
    }
}
