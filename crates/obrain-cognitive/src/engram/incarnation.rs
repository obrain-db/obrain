//! Liaison durable ENGRAMME → incarnation (nœud substrat).
//!
//! Un identifiant d'engramme (`EngramId`, slot `u16`) n'identifie rien par
//! lui-même : l'ancien compteur local pouvait le réutiliser après suppression
//! et réouverture. Seul l'allocateur NATIF du substrat (monotone, persisté,
//! borne partagée) ne rend jamais deux fois le même identifiant. Cette liaison
//! en porte la PREUVE : à l'allocation native, un nœud `__engram_incarnation__`
//! est créé avec la propriété `_engram_id`. La ressource d'observation
//! `E:<id>` n'est alors acceptée par le registre durable que si ce nœud existe ;
//! les révisions et effets vivent sur ce nœud, comme sur tout nœud ressource.
//!
//! * Pas de nœud (engramme legacy, formé par le compteur local) → refus sûr :
//!   aucune provenance, jamais de correspondance numérique.
//! * Nœud RETIRÉ (engramme supprimé) → refus nommé : une observation obsolète
//!   ne s'applique pas à une incarnation éteinte.
//!
//! L'attestation n'est écrite qu'APRÈS la publication vérifiée de l'état natif
//! de l'engramme (membres + bitset) : une réservation d'identifiant seule, ou
//! un échec de miroir, ne laisse AUCUNE incarnation lisible comme vivante. Une
//! vie illisible ou mal typée (`_retiree` non booléen, `_engram_id` non entier)
//! est REFUSÉE (fail-closed), jamais lue comme « vivante ».
//!
//! Deux nœuds portant le même `_engram_id` (course de création, rejeu) sont
//! interdits par construction de [`attester`] (recherche avant création) ;
//! s'il en existe plusieurs malgré tout, [`lier`] REFUSE (ambiguïté) plutôt que
//! de choisir.

use obrain_common::types::{NodeId, Value};
use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::{EtatPropriete, IssuePublication, ModePublication, SubstrateStore};

/// Label RÉSERVÉ du nœud d'incarnation.
pub(crate) const LABEL_INCARNATION: &str = "__engram_incarnation__";
/// Identifiant d'engramme porté par le nœud.
const CLE_ID: &str = "_engram_id";
/// Marque d'extinction (engramme supprimé).
const CLE_RETIREE: &str = "_retiree";

/// État de la liaison d'un engramme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Liaison {
    /// Aucune incarnation attestée (legacy, réservation orpheline, jamais formé
    /// nativement).
    Absente,
    /// Incarnation vivante, portée par ce nœud.
    Vivante(NodeId),
    /// Incarnation éteinte (engramme supprimé).
    Retiree(NodeId),
    /// Plusieurs nœuds revendiquent l'identifiant : refus, jamais de choix.
    Ambigue,
    /// Vie ou identité illisible / mal typée : refus (fail-closed).
    Illisible(String),
}

/// Interprète la valeur CAPTURÉE de `_retiree` : absente ou `false` = vivante,
/// `true` = éteinte ; tout autre type est illisible (refus).
pub(crate) fn vie_depuis(valeur: Option<&Value>) -> Result<bool, String> {
    match valeur {
        None => Ok(true),
        Some(v) => match v.as_bool() {
            Some(retiree) => Ok(!retiree),
            None => Err("vie d'incarnation mal typée (`_retiree` non booléen) : refus".to_string()),
        },
    }
}

/// Clés à capturer dans la précondition STRICTE d'une publication sur le nœud
/// d'incarnation : identité et vie.
pub(crate) const CLES_IDENTITE_VIE: [&str; 2] = [CLE_ID, CLE_RETIREE];

/// Vérifie, sur les valeurs CAPTURÉES, que le nœud est bien l'incarnation
/// VIVANTE de `engram_id`. Rend `Err` (refus nommé) sinon.
pub(crate) fn verifier_capture(
    id_capture: Option<&Value>,
    retiree_capture: Option<&Value>,
    engram_id: u64,
) -> Result<(), String> {
    match id_capture.and_then(Value::as_int64) {
        Some(v) if v as u64 == engram_id => {}
        _ => {
            return Err(
                "identité d'incarnation absente, mal typée ou différente : refus sûr".to_string(),
            );
        }
    }
    if vie_depuis(retiree_capture)? {
        Ok(())
    } else {
        Err("incarnation d'engramme éteinte : observation obsolète refusée".to_string())
    }
}

/// `Ok(None)` : nœud ORPHELIN (créé, arrêt avant l'écriture de `_engram_id`) —
/// il ne revendique aucun engramme et n'empoisonne pas la lecture.
fn id_du_noeud(store: &SubstrateStore, n: NodeId) -> Result<Option<u64>, String> {
    match store.lire_etat_propriete(n, CLE_ID) {
        Ok(EtatPropriete::Presente { valeur, .. }) => valeur
            .as_int64()
            .map(|v| Some(v as u64))
            .ok_or_else(|| "`_engram_id` mal typé".to_string()),
        Ok(_) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

fn vie_du_noeud(store: &SubstrateStore, n: NodeId) -> Result<bool, String> {
    match store.lire_etat_propriete(n, CLE_RETIREE) {
        Ok(EtatPropriete::Presente { valeur, .. }) => vie_depuis(Some(&valeur)),
        Ok(_) => Ok(true),
        Err(e) => Err(e.to_string()),
    }
}

/// Lit la liaison d'un engramme. Tout nœud d'incarnation à identité illisible
/// rend la lecture entière ILLISIBLE : on ne devine pas à quel engramme il
/// appartient.
pub(crate) fn lier(store: &SubstrateStore, engram_id: u64) -> Liaison {
    let mut trouves: Vec<NodeId> = Vec::new();
    for n in store.nodes_by_label_balayage(LABEL_INCARNATION) {
        match id_du_noeud(store, n) {
            Ok(Some(id)) if id == engram_id => trouves.push(n),
            Ok(_) => {}
            Err(e) => return Liaison::Illisible(format!("nœud d'incarnation {n:?} : {e}")),
        }
    }
    match trouves.as_slice() {
        [] => Liaison::Absente,
        [n] => match vie_du_noeud(store, *n) {
            Ok(true) => Liaison::Vivante(*n),
            Ok(false) => Liaison::Retiree(*n),
            Err(e) => Liaison::Illisible(e),
        },
        _ => Liaison::Ambigue,
    }
}

/// Ensemble des identifiants d'engrammes dont l'incarnation est ÉTEINTE, en UN
/// balayage. Un nœud illisible est ignoré ici (il n'éteint rien) ; la lecture
/// individuelle ([`lier`]) reste la seule autorité pour écrire.
pub(crate) fn retirees(store: &SubstrateStore) -> std::collections::HashSet<u64> {
    store
        .nodes_by_label_balayage(LABEL_INCARNATION)
        .into_iter()
        .filter_map(|n| match (id_du_noeud(store, n), vie_du_noeud(store, n)) {
            (Ok(Some(id)), Ok(false)) => Some(id),
            _ => None,
        })
        .collect()
}

/// Nombre maximal de ré-évaluations d'une publication en conflit.
const NB_REESSAIS: usize = 8;

/// Publie `cle = valeur` sur `n` par le chemin CANONIQUE faillible : capture de
/// précondition, transformation préparée en mode strict, publication
/// conditionnée. Seul un résultat `PublieIntegrale` est un succès ; un conflit
/// est ré-évalué (borné), toute autre issue est une ERREUR — jamais le repli
/// silencieux d'une écriture non persistée.
///
/// `garde` reçoit les valeurs capturées de la précondition (relues dans la même
/// capture que la publication) et peut refuser.
fn publier_strict(
    store: &SubstrateStore,
    n: NodeId,
    cle: &str,
    valeur: Value,
    cles_gardees: &[&str],
    garde: impl Fn(&obrain_substrate::Precondition) -> Result<(), String>,
) -> Result<(), String> {
    let mut cles: Vec<&str> = vec![cle];
    cles.extend(cles_gardees.iter().copied().filter(|c| *c != cle));
    for _ in 0..NB_REESSAIS {
        let pre = store
            .capturer_precondition(n, &cles)
            .map_err(|e| e.to_string())?;
        garde(&pre)?;
        let prep = store
            .preparer_transformation(n, &[(cle, valeur.clone())], &pre, ModePublication::Strict)
            .map_err(|e| e.to_string())?;
        match store
            .publier_si_precondition_intacte(prep)
            .map_err(|e| e.to_string())?
        {
            IssuePublication::PublieIntegrale => return Ok(()),
            IssuePublication::PubliePartielle => {
                return Err(format!(
                    "publication partielle de `{cle}` sur {n:?} : état non attesté"
                ));
            }
            IssuePublication::ConflitPrecondition => continue,
        }
    }
    Err(format!(
        "`{cle}` sur {n:?} : conflits de précondition répétés"
    ))
}

/// Attestation IDEMPOTENTE : rend le nœud d'incarnation de `engram_id`, en le
/// créant s'il manque. À n'appeler QU'APRÈS la publication vérifiée de l'état
/// natif de l'engramme, pour un identifiant rendu par l'allocateur natif. Refuse
/// (sans créer) si la liaison existante est ambiguë ou illisible.
///
/// L'identité est publiée par le chemin canonique faillible : si elle ne l'est
/// pas intégralement, l'attestation ÉCHOUE (le nœud créé reste un orphelin sans
/// identité, ignoré par [`lier`]) et l'appelant garde sa réservation à retenter.
pub(crate) fn attester(store: &SubstrateStore, engram_id: u64) -> Result<NodeId, String> {
    match lier(store, engram_id) {
        Liaison::Vivante(n) | Liaison::Retiree(n) => Ok(n),
        Liaison::Absente => {
            let n = store.create_node(&[LABEL_INCARNATION]);
            publier_strict(store, n, CLE_ID, Value::from(engram_id as i64), &[], |_| {
                Ok(())
            })?;
            Ok(n)
        }
        Liaison::Ambigue => Err("liaison d'incarnation ambiguë : attestation refusée".to_string()),
        Liaison::Illisible(e) => Err(format!("liaison d'incarnation illisible : {e}")),
    }
}

/// Éteint l'incarnation (engramme supprimé) par le chemin canonique faillible,
/// la précondition portant l'identité ET la vie. Idempotent si déjà éteinte ou
/// jamais attestée (legacy). ÉCHOUE — sans rien avoir écrit — si la liaison est
/// ambiguë/illisible ou si l'écriture canonique n'est pas intégrale : l'appelant
/// ne doit alors PAS vider l'état de l'engramme.
pub(crate) fn retirer(store: &SubstrateStore, engram_id: u64) -> Result<(), String> {
    match lier(store, engram_id) {
        Liaison::Absente | Liaison::Retiree(_) => Ok(()),
        Liaison::Ambigue => Err("liaison d'incarnation ambiguë : retrait refusé".to_string()),
        Liaison::Illisible(e) => Err(format!(
            "liaison d'incarnation illisible : retrait refusé ({e})"
        )),
        Liaison::Vivante(n) => {
            publier_strict(store, n, CLE_RETIREE, Value::from(true), &[CLE_ID], |pre| {
                verifier_capture(
                    pre.valeur_capturee(CLE_ID),
                    pre.valeur_capturee(CLE_RETIREE),
                    engram_id,
                )
            })
            .or_else(|e| {
                // Éteinte par un concurrent : l'objectif est atteint.
                if matches!(lier(store, engram_id), Liaison::Retiree(_)) {
                    Ok(())
                } else {
                    Err(e)
                }
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engram::store::EngramStore;
    use crate::engram::types::EngramId;
    use std::sync::Arc;

    fn natif_disque(chemin: &std::path::Path, creer: bool) -> (Arc<SubstrateStore>, EngramStore) {
        let st = Arc::new(if creer {
            SubstrateStore::create(chemin).unwrap()
        } else {
            SubstrateStore::open(chemin).unwrap()
        });
        let es = EngramStore::new(None);
        es.attach_substrate_store(st.clone());
        (st, es)
    }

    fn fermer(st: Arc<SubstrateStore>) {
        st.flush().unwrap();
        st.close().unwrap();
    }

    /// COUPURE entre le marqueur de retrait et l'effacement du slot : à la
    /// réouverture, le slot éteint ne redevient PAS un engramme (le retrait est
    /// achevé), l'id reste réservé, et c'est stable à la réouverture suivante.
    #[test]
    fn coupure_entre_marqueur_et_effacement_du_slot() {
        let td = tempfile::tempdir().unwrap();
        let chemin = td.path().join("kb");
        let a = {
            let (st, es) = natif_disque(&chemin, true);
            for _ in 0..3 {
                st.create_node(&["membre"]);
            }
            let a = es.form(&[NodeId(1), NodeId(2)]).unwrap();
            assert!(matches!(lier(&st, a.0), Liaison::Vivante(_)));
            // Marqueur écrit par le chemin canonique, puis « coupure » : le slot
            // n'est PAS vidé.
            retirer(&st, a.0).unwrap();
            assert!(matches!(lier(&st, a.0), Liaison::Retiree(_)));
            assert!(st.writer().read_engram_state(a.0 as u16).unwrap().is_some());
            fermer(st);
            a
        };
        let slot_vide = |st: &SubstrateStore, id: EngramId| {
            st.writer()
                .read_engram_state(id.0 as u16)
                .unwrap()
                .is_none_or(|(membres, _)| membres.is_empty())
        };
        // Passe 0 : le retrait interrompu est ACHEVÉ ; A ne revient pas.
        let b = {
            let (st, es) = natif_disque(&chemin, false);
            let rapport = es.load_from_substrate();
            assert_eq!(rapport.retraits_acheves, vec![a], "retrait réparé");
            assert_eq!(rapport.coherent, 0);
            assert!(es.get(a).is_none(), "passe 0 : A absent du cache");
            assert!(slot_vide(&st, a), "passe 0 : slot de A vide");
            assert_eq!(es.count(), 0);
            // L'id éteint n'est jamais réattribué ; B est un engramme VIVANT.
            let b = es.form(&[NodeId(1), NodeId(3)]).unwrap();
            assert!(b.0 > a.0, "B ({b}) > A ({a})");
            assert!(matches!(lier(&st, b.0), Liaison::Vivante(_)));
            fermer(st);
            b
        };
        // Passe 1 : A reste absent, rien à réparer, B est présent et UNIQUE.
        let (st, es) = natif_disque(&chemin, false);
        let rapport = es.load_from_substrate();
        assert!(rapport.retraits_acheves.is_empty(), "plus rien à réparer");
        assert!(es.get(a).is_none(), "passe 1 : A absent du cache");
        assert!(slot_vide(&st, a), "passe 1 : slot de A vide");
        assert!(es.get(b).is_some(), "passe 1 : B présent");
        assert_eq!(es.count(), 1, "passe 1 : B seul (A n'est pas revenu)");
        assert_eq!(rapport.coherent, 1);
    }

    /// ÉCHEC du chemin canonique de retrait (liaison ambiguë) : le retrait est
    /// REFUSÉ avant de vider quoi que ce soit — cache, slot et réservation
    /// restent ; la réouverture retrouve l'engramme.
    #[test]
    fn retrait_refuse_conserve_cache_et_slot() {
        let td = tempfile::tempdir().unwrap();
        let chemin = td.path().join("kb");
        let a = {
            let (st, es) = natif_disque(&chemin, true);
            for _ in 0..3 {
                st.create_node(&["membre"]);
            }
            let a = es.form(&[NodeId(1), NodeId(2)]).unwrap();
            // Faute : un second nœud revendique la même identité.
            let intrus = st.create_node(&[LABEL_INCARNATION]);
            publier_strict(
                &st,
                intrus,
                CLE_ID,
                Value::from(a.0 as i64),
                &[],
                |_| Ok(()),
            )
            .unwrap();
            assert!(matches!(lier(&st, a.0), Liaison::Ambigue));
            assert!(es.try_remove(a).is_err(), "retrait refusé");
            assert!(es.remove(a).is_none(), "remove ne vide rien");
            assert!(es.get(a).is_some(), "cache conservé");
            assert!(
                st.writer()
                    .read_engram_state(a.0 as u16)
                    .unwrap()
                    .is_some_and(|(m, _)| !m.is_empty()),
                "slot conservé"
            );
            fermer(st);
            a
        };
        let (_st, es) = natif_disque(&chemin, false);
        let r = es.load_from_substrate();
        assert_eq!(r.coherent, 1, "{r:?}");
        assert!(es.get(a).is_some());
    }

    /// Attestation : l'identité est publiée par le chemin strict ; un nœud
    /// ORPHELIN (créé sans identité) n'est pas une incarnation et n'empoisonne
    /// pas la lecture ; la seconde attestation est idempotente.
    #[test]
    fn attestation_stricte_idempotente_et_orphelin_ignore() {
        let st = SubstrateStore::open_memory().unwrap();
        st.create_node(&[LABEL_INCARNATION]); // orphelin
        assert!(matches!(lier(&st, 3), Liaison::Absente));
        let n = attester(&st, 3).unwrap();
        assert_eq!(attester(&st, 3).unwrap(), n);
        assert!(matches!(lier(&st, 3), Liaison::Vivante(m) if m == n));
        retirer(&st, 3).unwrap();
        retirer(&st, 3).unwrap(); // idempotent
        assert!(matches!(lier(&st, 3), Liaison::Retiree(_)));
    }
}
