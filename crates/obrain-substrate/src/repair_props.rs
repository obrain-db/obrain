//! Audit des references heap PENDANTES de la zone props v2 — le moteur de
//! `obrain repair-props`.
//!
//! ══ CE QU'EST UNE REFERENCE PENDANTE, ET POURQUOI ELLE EXISTE ══
//!
//! Une entree de propriete dont la valeur est `StringRef` / `BytesRef` /
//! `ArrStringRef` ne porte PAS les octets : elle porte un `HeapRef
//! {page_id, offset}` vers la zone heap. Le pointeur et le pointe vivent
//! dans deux fichiers distincts, rendus durables separement. Un arret
//! brutal entre les deux laisse donc sur disque un pointeur sans pointe —
//! `PropsZone::read_heap_view` rend alors `None`, et la lecture de la
//! propriete rend « valeur absente » alors que l'entree, elle, est bien la.
//!
//! L'ordre de flush (heap avant props, voir `PropsZone::flush`) ferme la
//! fenetre POUR L'AVENIR. Il ne repare rien de ce qu'une version anterieure
//! a laisse derriere elle : ce module sert a constater ces degats-la, et a
//! les retirer.
//!
//! ══ POURQUOI `read_heap_view` EST LE SEUL JUGE ══
//!
//! Trois defauts distincts produisent une reference illisible : une page
//! au-dela de l'extension allouee, un offset au-dela de `free_offset`, et
//! une longueur aberrante. Les trois sont deja couverts par le meme appel
//! (`heap_page_slice` pour le premier, `HeapPage::read_at` pour les deux
//! autres). Redupliquer ces bornes ici creerait une SECONDE definition de
//! « resoluble », qui deriverait de celle du chemin de lecture — et un
//! outil de reparation qui ne juge pas comme le lecteur est pire qu'aucun
//! outil.
//!
//! ══ POURQUOI LE RETRAIT NE VISE QUE LA VALEUR COURANTE ══
//!
//! La chaine d'un proprietaire est un journal append-only : une cle peut y
//! apparaitre plusieurs fois, et seule la DERNIERE (semantique LWW, voir
//! `PropsZone::get_latest_for_key`) est servie au lecteur. Une entree
//! pendante ENSEVELIE sous une reecriture saine n'est donc lue par
//! personne.
//!
//! Or la seule primitive de retrait sure du substrat
//! (`GraphStoreMut::remove_*_property`) ne sait pas viser une entree
//! precise : elle pose une tombstone sur la CLE, ce qui masque la valeur
//! courante. L'appliquer a cause d'une entree ensevelie DETRUIRAIT la
//! valeur saine qui l'ensevelit. Le scan compte donc toutes les entrees
//! pendantes (c'est l'etat reel du disque, et il doit etre rapporte), mais
//! ne rend RETIRABLES que celles qui sont la valeur courante de leur cle.
//! L'ecart entre les deux nombres est lui-meme rapporte : c'est du
//! gaspillage, pas une perte.

use crate::error::SubstrateResult;
use crate::page::{HeapRef, PropertyValue};
use crate::props_zone::PropsZone;
use crate::store::SubstrateStore;
use obrain_common::utils::hash::{FxHashMap, FxHashSet};

/// Nature du porteur d'une chaine de proprietes.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GenreProprietaire {
    Noeud,
    Arete,
}

impl GenreProprietaire {
    /// Etiquette stable, destinee a la sortie JSON — elle ne doit pas
    /// changer au gre des renommages internes.
    pub fn etiquette(self) -> &'static str {
        match self {
            Self::Noeud => "node",
            Self::Arete => "edge",
        }
    }
}

/// Une entree de propriete dont la valeur heap ne se resout pas.
#[derive(Clone, Debug)]
pub struct RefPendante {
    pub genre: GenreProprietaire,
    /// Slot du proprietaire (`NodeId` / `EdgeId` bruts).
    pub proprietaire: u64,
    pub cle: u16,
    /// Nom de la cle, quand le registre sait la resoudre.
    pub nom_cle: Option<String>,
    pub page_id: u32,
    pub offset: u32,
    /// L'entree est-elle la valeur COURANTE de sa cle (gagnante LWW) ?
    /// Seules celles-la sont retirables — voir le pave de tete.
    pub courante: bool,
}

/// Cible de retrait : un couple (proprietaire, nom de cle) dont la valeur
/// courante est pendante. Le nom est obligatoire — `remove_*_property` ne
/// sait viser qu'un nom, jamais un identifiant de cle.
#[derive(Clone, Debug)]
pub struct CibleRetrait {
    pub genre: GenreProprietaire,
    pub proprietaire: u64,
    pub nom_cle: String,
}

/// Ce qu'une seule chaine a livre. Sert d'unite au scan complet, et c'est
/// aussi l'unite que le test eprouve : la boucle de `scanner` ne fait que
/// sommer ces analyses, elle n'ajoute aucune regle.
#[derive(Clone, Debug, Default)]
pub struct AnalyseChaine {
    pub entrees_scannees: u64,
    pub entrees_heap: u64,
    /// `(cle, reference fautive, l'entree est-elle la valeur courante)`.
    pub pendantes: Vec<(u16, HeapRef, bool)>,
}

/// Tout ce que le scan a etabli. Les compteurs sont des FAITS observes, pas
/// des estimations : aucun champ n'est extrapole.
#[derive(Clone, Debug, Default)]
pub struct RapportRefsPendantes {
    /// Chaines de proprietes traversees (nœuds + aretes).
    pub chaines_scannees: u64,
    /// Entrees vivantes (non-tombstone) traversees, tous types confondus.
    pub entrees_scannees: u64,
    /// Parmi elles, celles dont la valeur est adossee au heap.
    pub entrees_heap: u64,
    /// Parmi celles-la, celles dont au moins une reference ne resout pas.
    pub entrees_pendantes: u64,
    /// Entrees pendantes qui sont la valeur COURANTE de leur cle.
    pub entrees_pendantes_courantes: u64,
    pub noeuds_touches: usize,
    pub aretes_touchees: usize,
    /// Extension VIVANTE du heap, en pages de 4 KiB.
    pub heap_pages_vivantes: u32,
    pub heap_pages_crc_examinees: u32,
    pub heap_pages_crc_invalide: u32,
    /// Entrees pendantes courantes dont la cle est absente du registre :
    /// elles ne peuvent pas etre retirees par nom, donc le rapport les
    /// isole au lieu de les compter comme retirables.
    pub pendantes_sans_nom_de_cle: u64,
    pub echantillon: Vec<RefPendante>,
    pub cibles: Vec<CibleRetrait>,
}

/// Toutes les references heap portees par une valeur — une pour les
/// scalaires, N pour un tableau de chaines, aucune pour les autres tags.
pub(crate) fn refs_heap(v: &PropertyValue) -> &[HeapRef] {
    match v {
        PropertyValue::StringRef(h) | PropertyValue::BytesRef(h) => std::slice::from_ref(h),
        PropertyValue::ArrStringRef(v) => v.as_slice(),
        _ => &[],
    }
}

/// Une entree est pendante des qu'UNE de ses references ne resout pas : un
/// tableau de chaines dont un element manque n'est pas « partiellement
/// lisible », il est faux. Rend la premiere reference fautive.
fn premiere_ref_pendante(zone: &PropsZone, v: &PropertyValue) -> Option<HeapRef> {
    refs_heap(v)
        .iter()
        .find(|h| zone.read_heap_view(**h).is_none())
        .copied()
}

/// Analyse UNE chaine de proprietes. C'est ici que vit toute la regle de
/// classement ; `scanner` n'en est que la boucle.
///
/// Deux passes, et la seconde n'a lieu que si la premiere a trouve quelque
/// chose : le cout de `collect_live_props_by_key_unite` (une seconde
/// traversee) n'est paye que sur les rares chaines touchees.
pub(crate) fn analyser_chaine(zone: &PropsZone, tete: u64) -> SubstrateResult<AnalyseChaine> {
    let mut analyse = AnalyseChaine::default();

    // Passe 1 — toute la chaine, y compris les entrees ensevelies : c'est
    // l'etat REEL du disque, et un rapport qui ne compterait que le visible
    // sous-estimerait les degats.
    let mut brutes: Vec<(u16, HeapRef)> = Vec::new();
    zone.walk_chain_unite(Some(tete), |e| {
        analyse.entrees_scannees += 1;
        if refs_heap(&e.value).is_empty() {
            return;
        }
        analyse.entrees_heap += 1;
        if let Some(h) = premiere_ref_pendante(zone, &e.value) {
            brutes.push((e.prop_key, h));
        }
    })?;
    if brutes.is_empty() {
        return Ok(analyse);
    }

    // Passe 2 — qui gagne le LWW ? C'est ce qui distingue une valeur
    // reellement perdue d'une entree ensevelie sous une reecriture saine.
    let mut vivantes = FxHashMap::default();
    let mut effacees = FxHashSet::default();
    zone.collect_live_props_by_key_unite(Some(tete), &mut vivantes, &mut effacees)?;
    let courantes: std::collections::HashSet<u16> = vivantes
        .iter()
        .filter(|(_, e)| premiere_ref_pendante(zone, &e.value).is_some())
        .map(|(k, _)| *k)
        .collect();

    analyse.pendantes = brutes
        .into_iter()
        .map(|(cle, href)| (cle, href, courantes.contains(&cle)))
        .collect();
    Ok(analyse)
}

/// Scanne toute la zone props v2 et rend l'etat des references heap.
///
/// `taille_echantillon` borne le nombre d'entrees pendantes detaillees ;
/// les COMPTEURS, eux, portent sur la totalite. Les `cibles` ne sont pas
/// bornees : `--apply` en a besoin en entier, et leur nombre est par
/// construction inferieur ou egal a celui des entrees pendantes.
///
/// Rend un rapport vide (tous compteurs a zero) sur une base sans zone v2 :
/// il n'y a alors aucun `HeapRef` sur disque, donc rien a auditer.
pub fn scanner(
    store: &SubstrateStore,
    taille_echantillon: usize,
) -> SubstrateResult<RapportRefsPendantes> {
    let mut rapport = RapportRefsPendantes::default();
    let Some(verrou) = store.zone_props_v2() else {
        return Ok(rapport);
    };

    // ── Releve des tetes de chaine AVANT toute prise de `props_zone` ─────
    //
    // `tetes_de_chaines_v2` prend le verrou de zone des nœuds ; l'ordre
    // inverse (props_zone puis zone de nœuds) est celui qui a deja
    // interbloque le store. Les tetes d'aretes sont relevees de meme, par
    // `for_each_live_edge`, avant d'ouvrir la zone props.
    let tetes_noeuds = store.tetes_de_chaines_v2();
    let mut tetes_aretes: Vec<(u64, u64)> = Vec::new();
    store.for_each_live_edge(|slot, rec| {
        let tete = rec.first_prop_off.to_u64();
        if tete != 0 {
            tetes_aretes.push((slot, tete));
        }
    })?;

    let zone = verrou.read();
    rapport.heap_pages_vivantes = zone.allocated_heap_page_count();
    let (examinees, en_echec) = zone.verifier_crc_pages_heap();
    rapport.heap_pages_crc_examinees = examinees;
    rapport.heap_pages_crc_invalide = en_echec;

    let chaines = tetes_noeuds
        .into_iter()
        .map(|(slot, tete)| (GenreProprietaire::Noeud, u64::from(slot), tete))
        .chain(
            tetes_aretes
                .into_iter()
                .map(|(slot, tete)| (GenreProprietaire::Arete, slot, tete)),
        );

    for (genre, proprietaire, tete) in chaines {
        let analyse = analyser_chaine(&zone, tete)?;
        rapport.chaines_scannees += 1;
        rapport.entrees_scannees += analyse.entrees_scannees;
        rapport.entrees_heap += analyse.entrees_heap;
        if analyse.pendantes.is_empty() {
            continue;
        }
        match genre {
            GenreProprietaire::Noeud => rapport.noeuds_touches += 1,
            GenreProprietaire::Arete => rapport.aretes_touchees += 1,
        }
        for (cle, href, courante) in analyse.pendantes {
            rapport.entrees_pendantes += 1;
            let nom = store.nom_de_cle_prop(cle).map(|a| a.to_string());
            if courante {
                rapport.entrees_pendantes_courantes += 1;
                match &nom {
                    Some(n) => {
                        // Une cle peut etre pendante plusieurs fois sur la
                        // meme chaine (reecritures successives, toutes
                        // perdues) : une SEULE tombstone suffit a la
                        // masquer, donc une seule cible.
                        let deja = rapport.cibles.iter().any(|c| {
                            c.genre == genre && c.proprietaire == proprietaire && c.nom_cle == *n
                        });
                        if !deja {
                            rapport.cibles.push(CibleRetrait {
                                genre,
                                proprietaire,
                                nom_cle: n.clone(),
                            });
                        }
                    }
                    None => rapport.pendantes_sans_nom_de_cle += 1,
                }
            }
            // ── L'ECHANTILLON PRIVILEGIE LES ENTREES COURANTES ──
            //
            // Un simple « les N premieres rencontrees » remplit
            // l'echantillon avec des entrees ENSEVELIES des que le premier
            // proprietaire touche en porte — et l'operateur se retrouve
            // avec vingt lignes qu'il ne peut ni verifier ni retirer, alors
            // que ce sont les valeurs COURANTES qui sont perdues et qui
            // l'interessent. Une courante evince donc une ensevelie quand
            // l'echantillon est plein ; entre deux du meme rang, l'ordre de
            // rencontre tranche.
            let temoin = RefPendante {
                genre,
                proprietaire,
                cle,
                nom_cle: nom,
                page_id: href.page_id,
                offset: href.offset,
                courante,
            };
            if rapport.echantillon.len() < taille_echantillon {
                rapport.echantillon.push(temoin);
            } else if courante
                && let Some(pos) = rapport.echantillon.iter().position(|p| !p.courante)
            {
                rapport.echantillon[pos] = temoin;
            }
        }
    }

    Ok(rapport)
}

/// Retire les cibles etablies par [`scanner`] et rend durable.
///
/// ══ POURQUOI PASSER PAR `remove_*_property` ══
///
/// La tentation serait d'aller retourner le drapeau `ENTRY_FLAG_TOMBSTONE`
/// dans la page. Ce serait une SECONDE ecriture de la zone props, hors du
/// WAL, hors des index, hors des colonnes blob — trois etats que le retrait
/// officiel maintient et qu'une mutation de page laisserait incoherents.
/// `remove_node_property` / `remove_edge_property` sont le chemin que le
/// reste du substrat connait ; c'est donc celui-ci, meme s'il fait un peu
/// plus de travail que strictement necessaire.
///
/// Rend le nombre de tombstones demandees. La valeur RENDUE par le retrait
/// n'est pas un temoin utilisable ici : elle vaut `None` precisement quand
/// la valeur etait illisible — c'est-a-dire dans tous nos cas. Le seul
/// verdict qui fasse foi est un RE-SCAN, que l'appelant fait.
pub fn appliquer(store: &SubstrateStore, cibles: &[CibleRetrait]) -> SubstrateResult<usize> {
    use obrain_common::types::{EdgeId, NodeId};
    use obrain_core::graph::traits::GraphStoreMut;

    for cible in cibles {
        match cible.genre {
            GenreProprietaire::Noeud => {
                store.remove_node_property(NodeId(cible.proprietaire), &cible.nom_cle);
            }
            GenreProprietaire::Arete => {
                store.remove_edge_property(EdgeId(cible.proprietaire), &cible.nom_cle);
            }
        }
    }
    store.flush()?;
    Ok(cibles.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::SubstrateFile;
    use crate::page::PropertyEntry;
    use crate::props_zone::PROPS_HEAP_V2_FILENAME;

    fn copier_repertoire(src: &std::path::Path, dst: &std::path::Path) {
        std::fs::create_dir_all(dst).unwrap();
        for e in std::fs::read_dir(src).unwrap() {
            let e = e.unwrap();
            if e.file_type().unwrap().is_file() {
                std::fs::copy(e.path(), dst.join(e.file_name())).unwrap();
            }
        }
    }

    /// L'analyse doit trouver EXACTEMENT la reference fabriquee pendante —
    /// ni plus (aucun faux positif sur les entrees saines qui l'entourent),
    /// ni moins.
    ///
    /// La panne est fabriquee par le MECANISME REEL, pas simulee : un
    /// premier lot rendu pleinement durable, puis un second dont seule la
    /// zone props atteint le disque (`flush_partiel_pour_test`), et le
    /// fichier heap restaure depuis l'image durable du lot 1. A la
    /// reouverture, l'entree du lot 2 pointe vers une page heap qui
    /// n'existe pas — exactement ce que `repair-props` doit trouver sur une
    /// base abimee.
    #[test]
    fn l_analyse_trouve_exactement_la_reference_fabriquee_pendante() {
        let sf = SubstrateFile::open_tempfile().unwrap();
        let mut pz = PropsZone::open(&sf).unwrap();

        // ---- lot 1 : deux entrees saines, pleinement durables.
        let href_a = pz.intern_bytes(b"valeur-saine-a").unwrap();
        let href_b = pz.intern_bytes(b"valeur-saine-b").unwrap();
        let mut tete = Some(
            pz.append_entry(
                1,
                None,
                &PropertyEntry::new(10, PropertyValue::StringRef(href_a)),
            )
            .unwrap(),
        );
        tete = Some(
            pz.append_entry(
                1,
                tete,
                &PropertyEntry::new(11, PropertyValue::StringRef(href_b)),
            )
            .unwrap(),
        );
        pz.flush().unwrap();
        let dir_durable = tempfile::tempdir().unwrap();
        copier_repertoire(sf.path(), dir_durable.path());

        // ---- lot 2 : une entree dont les octets occupent une page heap
        // NEUVE — c'est cette page que la coupure va emporter.
        let charge = vec![b'x'; crate::heap::HEAP_PAGE_SIZE - 16 - 4];
        let href_pendante = pz.intern_bytes(&charge).unwrap();
        assert_eq!(
            href_pendante.page_id, 1,
            "le lot 2 doit occuper une page heap neuve, sinon la coupure \
             n'emporte rien"
        );
        tete = Some(
            pz.append_entry(
                1,
                tete,
                &PropertyEntry::new(12, PropertyValue::StringRef(href_pendante)),
            )
            .unwrap(),
        );
        pz.flush_partiel_pour_test().unwrap();

        // Image d'apres-coupure : tout l'etat courant, sauf le heap,
        // restaure a son dernier etat durable (celui du lot 1).
        let dir_crash = tempfile::tempdir().unwrap();
        copier_repertoire(sf.path(), dir_crash.path());
        std::fs::copy(
            dir_durable.path().join(PROPS_HEAP_V2_FILENAME),
            dir_crash.path().join(PROPS_HEAP_V2_FILENAME),
        )
        .unwrap();
        let tete_brute = u64::from(tete.unwrap());
        drop(pz);

        // ---- audit : c'est le code de production qui juge, pas une copie.
        let sf2 = SubstrateFile::open(dir_crash.path()).unwrap();
        let pz2 = PropsZone::open(&sf2).unwrap();
        let analyse = analyser_chaine(&pz2, tete_brute).unwrap();

        assert_eq!(
            analyse.entrees_scannees, 3,
            "les trois entrees doivent avoir survecu a la coupure"
        );
        assert_eq!(analyse.entrees_heap, 3, "les trois sont adossees au heap");
        assert_eq!(
            analyse.pendantes.len(),
            1,
            "exactement UNE reference doit etre pendante : {:?}",
            analyse.pendantes
        );
        let (cle, href, courante) = analyse.pendantes[0];
        assert_eq!(cle, 12, "la pendante est l'entree du lot 2, pas une saine");
        assert_eq!(
            href, href_pendante,
            "et elle porte la reference exacte fabriquee"
        );
        assert!(
            courante,
            "aucune reecriture ne l'ensevelit : elle est la valeur courante, \
             donc retirable"
        );

        // Le contre-test : sans lui, un scan qui refuserait TOUT passerait.
        assert_eq!(
            pz2.read_heap_view(href_a),
            Some(&b"valeur-saine-a"[..]),
            "les entrees du lot 1 doivent rester resolubles"
        );
        assert_eq!(pz2.read_heap_view(href_b), Some(&b"valeur-saine-b"[..]));
    }

    /// Une entree pendante ENSEVELIE sous une reecriture saine ne doit PAS
    /// etre declaree courante : poser une tombstone sur sa cle detruirait
    /// la valeur saine qui l'ensevelit.
    #[test]
    fn une_pendante_ensevelie_sous_une_reecriture_saine_n_est_pas_courante() {
        let sf = SubstrateFile::open_tempfile().unwrap();
        let mut pz = PropsZone::open(&sf).unwrap();

        // Une reference volontairement impossible : page tres au-dela de
        // l'extension allouee. `read_heap_view` la refuse par le meme
        // chemin qu'une page emportee par une coupure.
        let fantome = HeapRef {
            page_id: 9_999,
            offset: 0,
        };
        let mut tete = Some(
            pz.append_entry(
                1,
                None,
                &PropertyEntry::new(7, PropertyValue::StringRef(fantome)),
            )
            .unwrap(),
        );
        // Reecriture SAINE de la MEME cle : elle ensevelit la precedente.
        let href_ok = pz.intern_bytes(b"reecriture-saine").unwrap();
        tete = Some(
            pz.append_entry(
                1,
                tete,
                &PropertyEntry::new(7, PropertyValue::StringRef(href_ok)),
            )
            .unwrap(),
        );

        let analyse = analyser_chaine(&pz, u64::from(tete.unwrap())).unwrap();
        assert_eq!(
            analyse.pendantes.len(),
            1,
            "l'entree ensevelie doit etre COMPTEE — c'est l'etat du disque"
        );
        assert!(
            !analyse.pendantes[0].2,
            "mais elle ne doit PAS etre courante : la valeur servie au \
             lecteur est la reecriture saine, et une tombstone la detruirait"
        );
    }

    /// Le CRC n'est verifie que sur l'extension VIVANTE, et une page
    /// fraichement scellee doit s'y verifier. Sans cela, le rapport
    /// afficherait des echecs de CRC sur du rembourrage jamais ecrit.
    #[test]
    fn le_crc_des_pages_heap_vivantes_se_verifie() {
        let sf = SubstrateFile::open_tempfile().unwrap();
        let mut pz = PropsZone::open(&sf).unwrap();
        for i in 0..3 {
            pz.intern_bytes(format!("charge-{i}").as_bytes()).unwrap();
        }
        let (examinees, en_echec) = pz.verifier_crc_pages_heap();
        assert!(examinees >= 1, "au moins une page vivante attendue");
        assert_eq!(
            examinees,
            pz.allocated_heap_page_count(),
            "l'examen doit porter sur toute l'extension vivante"
        );
        assert_eq!(
            en_echec, 0,
            "aucune page fraichement scellee ne doit echouer"
        );
    }
}
