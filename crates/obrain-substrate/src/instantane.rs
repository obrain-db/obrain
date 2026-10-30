//! Instantané EXACT d'un substrat — l'arche jumelle manquante de `native-import`.
//!
//! POURQUOI CE MODULE EXISTE, ET CE QU'IL N'EST PAS. Le dépôt savait INGÉRER un
//! corpus binaire (`native-import`, RFC §16.1) mais n'avait **aucune sortie** :
//! ni image de store, ni restauration. Or la compacité n'était pas le problème —
//! les enregistrements font déjà 40 B (nœud), 36 B, 32 B (simplexe), sans
//! pointeur ni en-tête par objet (`crate::record`). Ce qui manquait était le
//! GESTE de copie.
//!
//! Ce module copie donc les ZONES, et rien d'autre : c'est la forme déjà dense
//! du substrat, transportée telle quelle. Aucune ré-écriture, aucune
//! transformation, aucun encodage « intelligent » — parce qu'un instantané qui
//! transforme n'est plus un instantané : il devient un format de plus, avec sa
//! propre classe de bugs. La compacité n'est pas gagnée ici, elle est INHÉRÉE.
//!
//! PORTABLE PAR CONSTRUCTION. La copie passe par l'abstraction `ZoneFile`
//! (`open_named_zone`), jamais par le système de fichiers : le même code marche
//! donc pour un store fichier ET pour un store en mémoire — ce dernier n'ayant,
//! par définition, aucun répertoire à archiver. C'est la raison d'être de
//! `SubstrateFile::zone_names()`.
//!
//! FORMAT (little-endian) :
//!
//! ```text
//!   [8]  magic "OBRSNAP1"
//!   [4]  version de format = 1
//!   [4]  longueur de l'en-tête meta (META_FILE_SIZE)
//!   [n]  octets de l'en-tête meta (zone `substrate.meta`, tel quel)
//!   [4]  longueur du dictionnaire (0 si absent)
//!   [4]  CRC32 du dictionnaire
//!   [n]  octets du dictionnaire (`DictSnapshot::to_bytes`)
//!   [4]  nombre de zones
//!   par zone :
//!     [2]  longueur du nom
//!     [n]  nom (UTF-8, nom de fichier du substrat)
//!     [8]  longueur LOGIQUE de la zone (celle d'origine, rembourrage inclus)
//!     [8]  longueur de la charge utile (= logique, moins les zéros de queue)
//!     [4]  CRC32 de la charge utile
//!     [n]  charge utile = les octets de la zone, MOINS ses zéros de queue
//! ```
//!
//! POURQUOI LES ZÉROS DE QUEUE SONT RETIRÉS — et pourquoi c'est SANS PERTE.
//! Les zones sont pré-allouées par croissance exponentielle ×1,5
//! (`ZoneFile::grow_to`) : leur longueur dépasse largement l'étendue vivante, et
//! le surplus est du REMBOURRAGE À ZÉROS (mesuré : ×13 sur un graphe de 2 000
//! nœuds). Copier ce rembourrage, c'est transporter du vide. La restauration
//! appelle `grow_to(len_logique)`, qui RE-ZÉROFILLE — c'est exactement ce que
//! fait la croissance d'origine — puis réécrit la charge utile : la zone
//! restaurée est **identique octet à octet** à l'originale, rembourrage compris.
//!
//! CE QUE L'INSTANTANÉ NE CONTIENT PAS (v1, et il faut le dire) :
//! le WAL (journal de rejeu, non rejoué ici), le dictionnaire (`substrate.dict`,
//! qui a déjà son codec — `DictSnapshot::to_bytes` — et qui reste à raccorder),
//! et les colonnes latérales qui ne sont pas des zones du répertoire. Un store
//! restauré depuis cet instantané a donc ses zones exactes ; ce qui vit HORS des
//! zones est à raccorder avant de crier victoire.

use std::io::Write as _;

use crate::error::SubstrateResult;
use crate::file::SubstrateFile;

/// Marqueur du format, 8 octets — refus franc sur tout autre.
const MAGIC: &[u8; 8] = b"OBRSNAP1";

/// Version du format d'instantané. À incrémenter dès que la disposition change.
/// v2 (2026-09-17) : longueur logique + charge utile sans les zéros de queue.
/// v3 (2026-09-17) : section DICTIONNAIRE — sans elle, une image restaurait des
/// zones dont plus personne ne sait lire les labels ni les clés.
const FORMAT_VERSION: u32 = 3;

/// Une zone telle qu'elle apparaît dans l'instantané.
#[derive(Debug)]
struct ZoneCaptee {
    nom: String,
    /// Longueur d'origine de la zone (rembourrage compris).
    len_logique: u64,
    /// Octets utiles : la zone moins ses zéros de queue.
    charge: Vec<u8>,
    /// CRC32 de la charge utile (les zéros de queue sont implicites).
    crc: u32,
}

/// Longueur utile d'une zone : on coupe les zéros de QUEUE.
///
/// Ce n'est pas une heuristique de compression, c'est la reconnaissance de ce que
/// la croissance du substrat écrit — `grow_to` remplit de zéros. Ce qui est coupé
/// est donc, par construction, régénéré à l'identique par le `grow_to` de la
/// restauration.
fn longueur_utile(octets: &[u8]) -> usize {
    octets.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1)
}

/// Statistiques d'un instantané — ce qu'on veut pouvoir citer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatsInstantane {
    /// Octets de l'instantané complet (manifeste inclus).
    pub octets_total: u64,
    /// Nombre de zones emportées.
    pub zones: u32,
    /// SLOTS de nœuds réservés dans la zone — et NON le nombre de nœuds vivants.
    ///
    /// (2026-09-17, MESURÉ) La zone `nodes` est pré-allouée par croissance
    /// exponentielle ×1,5 (cf. `ZoneFile::grow_to`) : un graphe de **2 000**
    /// nœuds occupe **26 315 slots**. Confondre les deux ferait annoncer un
    /// instantané 13× plus gros qu'il ne l'est par entité — c'est exactement le
    /// genre de chiffre faux que ce module existe pour éviter.
    ///
    /// Le nombre VIVANT, lui, n'est disponible qu'auprès de l'allocateur du
    /// store (`next_node_id`) ou de la méta une fois flushée — le substrat ne le
    /// déduit pas de la longueur, et ne doit pas.
    pub slots_noeuds: u64,
    /// SLOTS d'arêtes réservés (même remarque que `slots_noeuds`).
    pub slots_aretes: u64,
}

impl StatsInstantane {
    /// Octets par SLOT de nœud réservé. `None` si aucun slot.
    ///
    /// Ce n'est PAS « par nœud vivant » : voir `slots_noeuds`. Pour un graphe
    /// compacté (après `shrink`/`compact`), les deux coïncident ; en cours de
    /// croissance, l'écart est celui du rembourrage ×1,5.
    #[must_use]
    pub fn octets_par_slot(&self) -> Option<f64> {
        (self.slots_noeuds > 0).then(|| self.octets_total as f64 / self.slots_noeuds as f64)
    }
}

/// Capture toutes les zones d'un substrat, VERBATIM, sous forme d'octets.
///
/// Les noms viennent de [`SubstrateFile::zone_names`], qui interroge le support
/// réel (répertoire ou registre mémoire) — jamais une liste figée : une colonne
/// latérale créée à la volée doit partir avec le reste.
pub fn exporter(sub: &SubstrateFile) -> SubstrateResult<Vec<u8>> {
    exporter_avec_dict(sub, None)
}

/// Export avec dictionnaire — la forme COMPLÈTE, celle qu'utilise le store
/// (`SubstrateStore::instantane`).
pub fn exporter_avec_dict(sub: &SubstrateFile, dict: Option<&[u8]>) -> SubstrateResult<Vec<u8>> {
    let meta_octets = meta_bytes(sub);

    let mut zones: Vec<ZoneCaptee> = Vec::new();
    for nom in sub.zone_names()? {
        let zf = sub.open_named_zone(&nom)?;
        let len_logique = zf.len();
        let utile = longueur_utile(zf.as_slice());
        let charge = zf.as_slice()[..utile].to_vec();
        let crc = crc32fast::hash(&charge);
        zones.push(ZoneCaptee {
            nom,
            len_logique,
            charge,
            crc,
        });
    }
    // Ordre STABLE : deux instantanés du même store doivent être comparables
    // octet à octet (tests, déduplication, transport). L'ordre du système de
    // fichiers, lui, ne l'est pas.
    zones.sort_by(|a, b| a.nom.cmp(&b.nom));

    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&(meta_octets.len() as u32).to_le_bytes());
    out.extend_from_slice(&meta_octets);

    let dict = dict.unwrap_or(&[]);
    out.extend_from_slice(&(dict.len() as u32).to_le_bytes());
    out.extend_from_slice(&crc32fast::hash(dict).to_le_bytes());
    out.extend_from_slice(dict);

    out.extend_from_slice(&(zones.len() as u32).to_le_bytes());

    for z in &zones {
        let nom = z.nom.as_bytes();
        out.extend_from_slice(&(nom.len() as u16).to_le_bytes());
        out.extend_from_slice(nom);
        out.extend_from_slice(&z.len_logique.to_le_bytes());
        out.extend_from_slice(&(z.charge.len() as u64).to_le_bytes());
        out.extend_from_slice(&z.crc.to_le_bytes());
        out.extend_from_slice(&z.charge);
    }

    Ok(out)
}

/// Les octets BRUTS de l'en-tête meta (pas la structure) : l'instantané transporte
/// le support tel quel, pas une re-sérialisation.
fn meta_bytes(sub: &SubstrateFile) -> Vec<u8> {
    sub.meta_bytes()
}

/// Restaure un instantané DANS un substrat fourni (souvent neuf).
///
/// Chaque zone est écrite VERBATIM, après vérification de son CRC : un
/// instantané corrompu est REFUSÉ au lieu de produire un store silencieusement
/// faux. C'est la seule garantie qui compte ici — « restaurer » un store
/// différent de l'original sans le dire serait le pire résultat possible.
pub fn restaurer(sub: &mut SubstrateFile, octets: &[u8]) -> SubstrateResult<StatsInstantane> {
    let mut cur = Lecteur::nouveau(octets);
    let magic = cur.lire(8)?;
    if magic != MAGIC {
        return Err(crate::error::SubstrateError::BadMagic);
    }
    let version = cur.u32()?;
    if version != FORMAT_VERSION {
        return Err(crate::error::SubstrateError::UnsupportedVersion(
            version,
            FORMAT_VERSION,
        ));
    }

    let meta_len = cur.u32()? as usize;
    let meta_octets = cur.lire(meta_len)?.to_vec();

    let dict_len = cur.u32()? as usize;
    let dict_crc = cur.u32()?;
    let dict = cur.lire(dict_len)?.to_vec();
    if crc32fast::hash(&dict) != dict_crc {
        return Err(crate::error::SubstrateError::Internal(
            "instantané corrompu : dictionnaire — CRC invalide".into(),
        ));
    }

    let n_zones = cur.u32()?;

    // 1. Les ZONES d'abord : la méta décrit des compteurs que les zones doivent
    //    pouvoir satisfaire. (L'ordre inverse marcherait aussi ici, mais celui-ci
    //    laisse la méta en dernier mot — celui qui décide si le store est sain.)
    for _ in 0..n_zones {
        let nom_len = cur.u16()? as usize;
        let nom = String::from_utf8_lossy(cur.lire(nom_len)?).into_owned();
        let len_logique = cur.u64()?;
        let len_charge = cur.u64()?;
        let crc_attendu = cur.u32()?;
        let charge = cur.lire(len_charge as usize)?;
        let crc = crc32fast::hash(charge);
        if crc != crc_attendu {
            return Err(crate::error::SubstrateError::Internal(format!(
                "instantané corrompu : zone {nom} — CRC {crc:#010x} au lieu de {crc_attendu:#010x}"
            )));
        }
        if len_charge > len_logique {
            return Err(crate::error::SubstrateError::Internal(format!(
                "instantané incohérent : zone {nom} — charge utile ({len_charge}) \
                 plus longue que la zone ({len_logique})"
            )));
        }
        let mut zf = sub.open_named_zone(&nom)?;
        // La croissance RE-ZÉROFILLE : c'est elle qui régénère le rembourrage non
        // transporté. On zérofie explicitement la queue pour que la garantie ne
        // dépende pas de l'état de la cible (restauration dans un substrat qui
        // porterait déjà des octets).
        zf.grow_to(len_logique)?;
        let dst = zf.as_slice_mut();
        dst[..len_charge as usize].copy_from_slice(charge);
        dst[len_charge as usize..].fill(0);
    }

    // 2. La méta, qui porte l'identité du store.
    sub.write_meta_from_bytes(&meta_octets)?;

    // 3. Le dictionnaire, s'il a été transporté : écrit comme une ZONE nommée
    //    `substrate.dict`. Cette unification n'est pas cosmétique — c'est ELLE qui
    //    rend le dictionnaire restaurable dans les DEUX modes :
    //      · en mode FICHIER, `open_named_zone` ouvre/crée `<dir>/substrate.dict`
    //        et y écrit ces octets : c'est exactement le fichier que
    //        `DictSnapshot::load` relira à l'ouverture ;
    //      · en mode MÉMOIRE, la même écriture dépose la zone dans le registre du
    //        substrat, et `from_substrate` la relit (`is_memory()`).
    //    Avant, la voie mémoire n'existait pas : le dictionnaire voyageait mais
    //    ne pouvait pas atterrir.
    if !dict.is_empty() {
        let mut zf = sub.open_named_zone(crate::store::DICT_FILENAME_PUB)?;
        zf.grow_to(dict.len() as u64)?;
        zf.as_slice_mut().copy_from_slice(&dict);
    }

    let (slots_noeuds, slots_aretes) = compter_depuis_zones(sub)?;
    Ok(StatsInstantane {
        octets_total: octets.len() as u64,
        zones: n_zones,
        slots_noeuds,
        slots_aretes,
    })
}

/// Statistiques d'un instantané SANS le restaurer (lecture du manifeste seul).
///
/// Sert à répondre à « combien ça pèse, pour combien de nœuds » sans toucher à
/// un store — c'est le chiffre qu'on veut citer et comparer.
pub fn stats(octets: &[u8]) -> SubstrateResult<StatsInstantane> {
    let mut cur = Lecteur::nouveau(octets);
    let magic = cur.lire(8)?;
    if magic != MAGIC {
        return Err(crate::error::SubstrateError::BadMagic);
    }
    let _version = cur.u32()?;
    let meta_len = cur.u32()? as usize;
    let _meta_octets = cur.lire(meta_len)?;
    let dict_len = cur.u32()? as usize;
    let _dict_crc = cur.u32()?;
    let _dict = cur.lire(dict_len)?;
    let n_zones = cur.u32()?;

    // LES COMPTEURS VIENNENT DES ZONES, PAS DE LA MÉTA — et c'est important.
    // (2026-09-17, MESURÉ) Sur un store EN MÉMOIRE, `node_count` et `edge_count`
    // de l'en-tête meta valent 0 : ces compteurs ne sont écrits qu'au
    // flush/checkpoint, or le mode mémoire ne flushe pas (voir `store::flush`).
    // Un manifeste qui les recopierait annoncerait « 0 nœud » pour un graphe
    // plein — le genre de mensonge qu'un instantané ne doit jamais porter.
    let (mut slots_noeuds, mut slots_aretes) = (0u64, 0u64);
    for _ in 0..n_zones {
        let nom_len = cur.u16()? as usize;
        let nom = String::from_utf8_lossy(cur.lire(nom_len)?).into_owned();
        let len_logique = cur.u64()?;
        let len_charge = cur.u64()?;
        let _crc = cur.u32()?;
        if nom == crate::file::zone::NODES {
            slots_noeuds = slots_pleins(
                len_logique,
                core::mem::size_of::<crate::record::NodeRecord>(),
            );
        } else if nom == crate::file::zone::EDGES {
            slots_aretes = slots_pleins(
                len_logique,
                core::mem::size_of::<crate::record::EdgeRecord>(),
            );
        }
        cur.sauter(len_charge as usize)?;
    }
    Ok(StatsInstantane {
        octets_total: octets.len() as u64,
        zones: n_zones,
        slots_noeuds,
        slots_aretes,
    })
}

/// Nombre d'entités VIVANTES dans une zone à pas fixe : le slot 0 est un
/// sentinelle (nœud nul, arête nulle), on le retranche.
fn slots_pleins(octets_zone: u64, taille_record: usize) -> u64 {
    (octets_zone / taille_record as u64).saturating_sub(1)
}

/// Compte les nœuds et arêtes d'un substrat en lisant ses ZONES.
fn compter_depuis_zones(sub: &SubstrateFile) -> SubstrateResult<(u64, u64)> {
    let nodes = sub.open_zone(crate::file::Zone::Nodes)?;
    let slots_noeuds = slots_pleins(
        nodes.len(),
        core::mem::size_of::<crate::record::NodeRecord>(),
    );
    let edges = sub.open_zone(crate::file::Zone::Edges)?;
    let slots_aretes = slots_pleins(
        edges.len(),
        core::mem::size_of::<crate::record::EdgeRecord>(),
    );
    Ok((slots_noeuds, slots_aretes))
}

/// Le dictionnaire transporté par un instantané, s'il y en a un.
///
/// Rend `None` si l'image n'en porte pas (instantané produit par `exporter`
/// seul). Sert aux supports SANS répertoire — un store en mémoire ne peut pas
/// recevoir le dictionnaire par un fichier : l'appelant le récupère ici.
pub fn dict_du_flux(octets: &[u8]) -> SubstrateResult<Option<Vec<u8>>> {
    let mut cur = Lecteur::nouveau(octets);
    if cur.lire(8)? != MAGIC {
        return Err(crate::error::SubstrateError::BadMagic);
    }
    let _version = cur.u32()?;
    let meta_len = cur.u32()? as usize;
    let _meta = cur.lire(meta_len)?;
    let dict_len = cur.u32()? as usize;
    let _crc = cur.u32()?;
    let dict = cur.lire(dict_len)?.to_vec();
    Ok((!dict.is_empty()).then_some(dict))
}

/// Répartition par zone d'un instantané : `(nom, longueur logique, charge utile)`.
///
/// POURQUOI CE DIAGNOSTIC EXISTE. Après la v2, il restait 8,4 Mio pour un store
/// dont le contenu vivant tient en ~1 Mio. « Optimiser » sans savoir OÙ sont les
/// octets, c'est deviner — et ce dépôt a déjà payé pour apprendre à ne pas le
/// faire (cf. `benches/BANCS.md`). Cette fonction lit l'image EXPORTÉE, donc elle
/// décrit ce qui voyage réellement, pas ce qu'on croit voyager.
pub fn repartition(octets: &[u8]) -> SubstrateResult<Vec<(String, u64, u64)>> {
    let mut cur = Lecteur::nouveau(octets);
    let magic = cur.lire(8)?;
    if magic != MAGIC {
        return Err(crate::error::SubstrateError::BadMagic);
    }
    let _version = cur.u32()?;
    let meta_len = cur.u32()? as usize;
    let _meta = cur.lire(meta_len)?;
    let dict_len = cur.u32()? as usize;
    let _dict_crc = cur.u32()?;
    let _dict = cur.lire(dict_len)?;
    let n_zones = cur.u32()?;
    let mut out = Vec::with_capacity(n_zones as usize);
    for _ in 0..n_zones {
        let nom_len = cur.u16()? as usize;
        let nom = String::from_utf8_lossy(cur.lire(nom_len)?).into_owned();
        let len_logique = cur.u64()?;
        let len_charge = cur.u64()?;
        let _crc = cur.u32()?;
        cur.sauter(len_charge as usize)?;
        out.push((nom, len_logique, len_charge));
    }
    out.sort_by(|a, b| b.2.cmp(&a.2));
    Ok(out)
}

/// Écrit un instantané sur un support `Write` (fichier, tampon, socket).
pub fn ecrire_dans(w: &mut impl std::io::Write, octets: &[u8]) -> SubstrateResult<()> {
    w.write_all(octets)?;
    Ok(())
}

/// Lecteur borné — refuse de lire au-delà des octets fournis au lieu de paniquer.
struct Lecteur<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Lecteur<'a> {
    fn nouveau(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn lire(&mut self, n: usize) -> SubstrateResult<&'a [u8]> {
        let fin = self.pos.checked_add(n).ok_or_else(|| {
            crate::error::SubstrateError::Internal("instantané : longueur débordante".into())
        })?;
        if fin > self.buf.len() {
            return Err(crate::error::SubstrateError::Internal(format!(
                "instantané tronqué : {} octets demandés à l'offset {}, il en reste {}",
                n,
                self.pos,
                self.buf.len() - self.pos
            )));
        }
        let out = &self.buf[self.pos..fin];
        self.pos = fin;
        Ok(out)
    }

    fn sauter(&mut self, n: usize) -> SubstrateResult<()> {
        self.lire(n).map(|_| ())
    }

    fn u16(&mut self) -> SubstrateResult<u16> {
        Ok(u16::from_le_bytes(self.lire(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> SubstrateResult<u32> {
        Ok(u32::from_le_bytes(self.lire(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> SubstrateResult<u64> {
        Ok(u64::from_le_bytes(self.lire(8)?.try_into().unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obrain_core::graph::{GraphStore as _, GraphStoreMut as _};

    /// Construit un petit store avec des nœuds, des propriétés et des arêtes.
    fn terrain() -> crate::store::SubstrateStore {
        let store = crate::store::SubstrateStore::open_memory().expect("store en mémoire");
        let a = store.create_node(&["Personne"]);
        let b = store.create_node(&["Personne"]);
        store.set_node_property(a, "nom", obrain_common::types::Value::String("Alix".into()));
        store.set_node_property(b, "nom", obrain_common::types::Value::String("Gus".into()));
        store.set_node_property(a, "age", obrain_common::types::Value::Int64(30));
        store.create_edge(a, b, "CONNAIT");
        store
    }

    /// LE test qui définit « instantané exact » : après export puis restauration,
    /// CHAQUE zone est identique OCTET À OCTET. Pas « équivalent », pas « les
    /// compteurs collent » : identique. C'est le critère falsifiable, et c'est le
    /// seul qui autorise à appeler ça une sauvegarde.
    #[test]
    fn round_trip_bit_a_bit_sur_les_zones() {
        let source = terrain();
        let image = source.avec_substrate(|sub| exporter(sub)).expect("export");
        assert!(image.len() > 8, "une image non vide");

        let mut cible = SubstrateFile::open_memory().expect("cible en mémoire");
        let stats = restaurer(&mut cible, &image).expect("restauration");
        let noms_src = source.avec_substrate(|sub| sub.zone_names().unwrap());

        // Comparaison zone par zone, octet à octet — dans les deux sens : aucune
        // zone de la source ne manque, et aucune zone en trop n'a été inventée.
        assert_eq!(stats.zones as usize, noms_src.len());
        let noms_cible = cible.zone_names().unwrap();
        assert_eq!(noms_src, noms_cible, "mêmes zones des deux côtés");

        for nom in &noms_src {
            let a = source.avec_substrate(|sub| sub.open_named_zone(nom).unwrap());
            let b = cible.open_named_zone(nom).unwrap();
            assert_eq!(a.len(), b.len(), "longueur de {nom}");
            assert_eq!(
                a.as_slice(),
                b.as_slice(),
                "la zone {nom} doit être identique OCTET À OCTET"
            );
        }

        // Et la méta restaurée porte le même node_count que la source.
        let (nc, ec) = source
            .avec_substrate(|sub| (sub.meta_header().node_count, sub.meta_header().edge_count));
        assert_eq!(cible.meta_header().node_count, nc);
        assert_eq!(cible.meta_header().edge_count, ec);
    }

    /// LE DICTIONNAIRE VOYAGE, ET IL SE RECHARGE.
    ///
    /// C'est ce qui sépare « des octets exacts » de « un store qui veut encore
    /// dire quelque chose » : sans les registres (labels, types d'arête, clés de
    /// propriété, compteurs), une image restaurée contient des données que
    /// personne ne sait plus nommer.
    #[test]
    fn le_dictionnaire_voyage_et_se_recharge() {
        let source = terrain();
        let image = source.instantane().expect("instantané complet");

        let dict = dict_du_flux(&image)
            .expect("lecture du flux")
            .expect("le dictionnaire doit être transporté");
        let snap = crate::dict::DictSnapshot::from_bytes(&dict).expect("décodage");

        assert!(
            snap.labels.iter().any(|l| l == "Personne"),
            "le label doit être dans le dictionnaire : {:?}",
            snap.labels
        );
        assert!(
            snap.edge_types.iter().any(|t| t == "CONNAIT"),
            "le type d'arête doit y être : {:?}",
            snap.edge_types
        );
        assert!(
            snap.prop_keys.iter().any(|k| k == "nom"),
            "la clé de propriété doit y être : {:?}",
            snap.prop_keys
        );

        // Et un instantané SANS dictionnaire en est ABSENT — pas « présent et
        // vide », ce qui laisserait croire à un store sans aucun label.
        let nu = source
            .avec_substrate(|sub| exporter(sub))
            .expect("export nu");
        assert!(
            dict_du_flux(&nu).expect("lecture").is_none(),
            "un export nu ne porte pas de dictionnaire"
        );
    }

    /// LE BOUT EN BOUT, EN MÉMOIRE : un store restauré doit encore savoir
    /// NOMMER ses labels.
    ///
    /// C'est la preuve que le dictionnaire a ATTERRI, et pas seulement voyagé :
    /// sans lui, les zones seraient exactes et les registres VIDES — un store qui
    /// contient des données que personne ne sait plus nommer. Le test exige les
    /// deux : les octets (un nœud se relit) ET le sens (le label se relit).
    #[test]
    fn un_store_restaure_en_memoire_sait_encore_nommer_ses_labels() {
        use obrain_core::graph::GraphStore as _;

        let source = terrain();
        let image = source.instantane().expect("instantané complet");

        let restaure = crate::store::SubstrateStore::depuis_instantane(&image)
            .expect("restauration en mémoire");

        // 1. Les OCTETS : le nœud 1 est là (les zones ont bien atterri).
        assert!(
            restaure.get_node(obrain_common::types::NodeId(1)).is_some(),
            "le nœud restauré doit se relire"
        );

        // 2. Le SENS : le dictionnaire du store RESTAURÉ porte encore ses labels.
        let dict = restaure
            .octets_dictionnaire()
            .expect("dict du store restauré");
        let snap = crate::dict::DictSnapshot::from_bytes(&dict).expect("décodage");
        assert!(
            snap.labels.iter().any(|l| l == "Personne"),
            "le store restauré doit encore connaître son label : {:?}",
            snap.labels
        );
        assert!(
            snap.edge_types.iter().any(|t| t == "CONNAIT"),
            "et son type d'arête : {:?}",
            snap.edge_types
        );
    }

    /// Un instantané CORROMPU doit être REFUSÉ, pas restauré de travers : c'est la
    /// différence entre un format et un pari.
    #[test]
    fn un_instantane_corrompu_est_refuse() {
        let source = terrain();
        let image = source.avec_substrate(|sub| exporter(sub)).expect("export");
        // On abîme un octet au MILIEU des données d'une zone (pas l'en-tête, qui
        // n'est pas couvert par un CRC : le manifeste d'abord, puis les zones).
        let mut abime = image.clone();
        let n = abime.len();
        abime[n - 3] ^= 0xFF;

        let mut cible = SubstrateFile::open_memory().expect("cible");
        let res = restaurer(&mut cible, &abime);
        assert!(
            res.is_err(),
            "un CRC faux doit faire échouer la restauration"
        );
    }

    /// Un instantané tronqué est refusé proprement (pas de panique, pas de
    /// lecture hors bornes).
    #[test]
    fn un_instantane_tronque_est_refuse_sans_panique() {
        let source = terrain();
        let image = source.avec_substrate(|sub| exporter(sub)).expect("export");
        let mut cible = SubstrateFile::open_memory().expect("cible");
        for coupe in [4usize, 9, 20, image.len() - 1] {
            let res = restaurer(&mut cible, &image[..coupe]);
            assert!(res.is_err(), "tronqué à {coupe} doit échouer");
        }
    }

    /// MESURE — combien pèse un instantané, par nœud, sur un corpus réaliste.
    ///
    /// Lancement qui AFFICHE le chiffre :
    /// `cargo test -p obrain-substrate --lib instantane::tests::mesure_octets_par_noeud -- --nocapture`
    ///
    /// Le chiffre n'est pas figé dans une assertion (il dépend du contenu), mais
    /// la borne haute l'est : un instantané qui dépasse 1 Kio par nœud sur un
    /// graphe aussi simple serait un signal d'alarme, pas une mesure.
    #[test]
    fn mesure_octets_par_noeud() {
        const N: usize = 2_000;
        let store = crate::store::SubstrateStore::open_memory().expect("store");
        let mut ids = Vec::with_capacity(N);
        for i in 0..N {
            let id = store.create_node(&["Mesure"]);
            store.set_node_property(id, "rang", obrain_common::types::Value::Int64(i as i64));
            store.set_node_property(
                id,
                "nom",
                obrain_common::types::Value::String(format!("noeud-{i}").into()),
            );
            ids.push(id);
        }
        for i in 1..N {
            store.create_edge(ids[i - 1], ids[i], "SUIT");
        }

        let image = store.avec_substrate(|sub| exporter(sub)).expect("export");
        let s = stats(&image).expect("stats");
        let par_slot = s.octets_par_slot().expect("des slots");
        let rembourrage = s.slots_noeuds as f64 / N as f64;
        println!(
            "INSTANTANE: {} octets — {} slots nœuds / {} slots arêtes, {} zones —              {:.1} octets/slot, rembourrage ×{:.2} ({} nœuds vivants créés)",
            s.octets_total, s.slots_noeuds, s.slots_aretes, s.zones, par_slot, rembourrage, N
        );
        // DIAGNOSTIC — où sont les octets ? (les 12 plus grosses zones)
        println!("--- répartition (logique → charge utile) ---");
        for (nom, lg, lc) in repartition(&image).unwrap().iter().take(12) {
            let pct = 100.0 * *lc as f64 / s.octets_total as f64;
            println!("  {nom:38} {lg:>12} → {lc:>12}  ({pct:5.1} %)");
        }
        let total_charge: u64 = repartition(&image).unwrap().iter().map(|z| z.2).sum();
        println!(
            "  {:38} {:>12} → {:>12}  (dont manifeste : {} octets)",
            "TOTAL des zones",
            "",
            total_charge,
            s.octets_total - total_charge
        );
        assert!(
            s.slots_noeuds >= N as u64,
            "les slots réservés couvrent au moins les nœuds créés"
        );
        assert!(
            par_slot < 1024.0,
            "un instantané à {par_slot:.1} octets/slot dépasse la borne d'alerte"
        );
    }

    /// Le manifeste seul suffit à citer un chiffre : combien d'octets, pour
    /// combien de nœuds. Sans restaurer quoi que ce soit.
    #[test]
    fn les_stats_se_lisent_sans_restaurer() {
        let source = terrain();
        let image = source.avec_substrate(|sub| exporter(sub)).expect("export");
        let s = stats(&image).expect("stats");
        // Le socle (ALL_MMAPPED) compte 13 zones, mais l'instantané en emporte
        // PLUS : les side-cars créés par la construction du store voyagent aussi
        // (mesuré : 20). On ne fige donc pas le nombre — l'égalité exacte des deux
        // côtés est vérifiée par `round_trip_bit_a_bit_sur_les_zones`.
        assert!(s.zones >= 13, "au moins le socle : {}", s.zones);
        // Les zones sont PRÉ-ALLOUÉES (×1,5) : leurs longueurs donnent des SLOTS,
        // pas le nombre d'entités vivantes. On n'assère donc pas `2` et `1`, qui
        // seraient faux — seulement des minorants et la cohérence du ratio.
        assert!(s.slots_noeuds >= 2, "au moins les nœuds créés");
        assert!(s.slots_aretes >= 1, "au moins les arêtes créées");
        assert!(s.octets_par_slot().is_some());
    }
}
