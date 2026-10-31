//! `n.prop STARTS WITH 'prefixe'` — la plage ordonnee qui lui correspond.
//!
//! ── POURQUOI UNE PLAGE ──
//!
//! `STARTS WITH` n'etait reconnu que par le traducteur RDF
//! (`planner/rdf/mod.rs`), ou il est evalue en FILTRE apres coup. Sur le
//! chemin LPG il tombait sur le `FilterOperator` generique : un balayage
//! integral, hydratation de chaque noeud comprise, meme quand la requete
//! coupe a trente lignes.
//!
//! Or l'ensemble des chaines commencant par `p` est exactement l'intervalle
//! `[p, haute(p))` pour l'ordre lexicographique, ou `haute(p)` est `p` dont
//! le DERNIER point de code est incremente. Le substrat sait repondre a un
//! intervalle : `find_nodes_in_range`, qui interroge l'index ordonne quand
//! la propriete est declaree et retombe SEUL sur le balayage sinon
//! (`candidats_ordonnes(..).or_else(balayer_v2_par_predicat)`). Il n'y a donc
//! aucun repli a ecrire ici : il existe deja, un etage plus bas.
//!
//! ── POURQUOI CETTE EQUIVALENCE EST VRAIE, ET QUAND ELLE NE L'EST PAS ──
//!
//! `x` commence par `p` ⟺ `p ≤ x < haute(p)`, POURVU que la comparaison soit
//! lexicographique. Les deux sens :
//!
//! · si `x` commence par `p` alors `x ≥ p` (prefixe), et `x < haute(p)` car
//!   `x` et `haute(p)` different au dernier point de code de `p`, ou `x`
//!   porte `c` et `haute(p)` porte `c+1` ;
//! · si `p ≤ x < haute(p)` et que `x` ne commence PAS par `p`, soit `i` le
//!   premier rang ou ils different. `x ≥ p` force `x[i] > p[i]` ; si `i` est
//!   le dernier rang de `p` alors `x[i] ≥ c+1` donc `x ≥ haute(p)`,
//!   contradiction ; si `i` est anterieur, `x > haute(p)` de meme.
//!
//! L'ORDRE DU SUBSTRAT N'EST PAS TOUJOURS LEXICOGRAPHIQUE, et c'est la que
//! l'equivalence casse. `comparer_ordonnable` (substrate/src/store.rs)
//! compare DEUX chaines RFC3339 par INSTANT. Avec `p = '2024-01-01T00:00:00Z'`,
//! la valeur `'2024-01-01T00:00:00.000Z'` a le MEME instant, donc satisfait
//! la borne basse INCLUSIVE — alors qu'elle ne commence pas par `p`. Une
//! ligne de trop, muette. On se DESISTE donc des que `instant_de_chaine`
//! accepte le prefixe ou sa borne haute ; le balayage generique reprend, et
//! il est correct.
//!
//! Les autres desistements sont des impossibilites de construction :
//! prefixe VIDE (aucune borne haute n'existe, et `''` retient tout de toute
//! facon), dernier point de code sans successeur (`char::MAX`), ou dont le
//! successeur tombe dans la zone des SURROGATES (`U+D7FF` → `U+D800`, qui
//! n'est pas un `char`). Dans tous ces cas : `None`, et le chemin generique.
//!
//! ── CE QUI N'EST PAS FAIT ICI ──
//!
//! On ne reconnait que le `STARTS WITH` en position de predicat COMPLET, sur
//! une propriete a gauche et un litteral chaine a droite — la meme prudence
//! qu'`extract_range_predicate`, qui ne descend pas non plus dans les `AND`.
//! `STARTS WITH` n'est pas commutatif : aucune forme retournee.

use crate::query::plan::{BinaryOp, LogicalExpression};
use obrain_common::types::Value;

/// La borne haute EXCLUSIVE de l'ensemble des chaines commencant par
/// `prefixe` : `prefixe` dont le dernier point de code est incremente.
///
/// `None` quand aucune borne n'existe : prefixe vide, dernier point de code
/// egal a `char::MAX`, ou successeur non representable (bord des surrogates).
///
/// On travaille par POINT DE CODE et non par octet : l'ordre lexicographique
/// des octets UTF-8 coincide avec celui des points de code, mais incrementer
/// le dernier OCTET produirait de l'UTF-8 invalide des que le prefixe finit
/// par un caractere multi-octets.
pub(super) fn borne_haute_de_prefixe(prefixe: &str) -> Option<String> {
    let dernier = prefixe.chars().next_back()?;
    let suivant = char::from_u32(u32::from(dernier).checked_add(1)?)?;
    let mut haute = String::with_capacity(prefixe.len() + suivant.len_utf8());
    haute.push_str(&prefixe[..prefixe.len() - dernier.len_utf8()]);
    haute.push(suivant);
    Some(haute)
}

/// L'intervalle `[min, max)` equivalent a `STARTS WITH prefixe`, ou `None`
/// si aucune plage ne peut le representer FIDELEMENT.
///
/// Le desistement datetime est ici, et non chez l'appelant, pour qu'il soit
/// impossible d'obtenir les bornes sans lui.
pub(super) fn plage_de_prefixe(prefixe: &str) -> Option<(String, String)> {
    use obrain_common::types::value_cmp::instant_de_chaine;

    let haute = borne_haute_de_prefixe(prefixe)?;
    // Voir le pave de tete : une borne analysable en datetime fait basculer
    // `comparer_ordonnable` de l'ordre des octets a l'ordre des instants, et
    // l'equivalence prefixe/intervalle ne tient plus.
    if instant_de_chaine(prefixe).is_some() || instant_de_chaine(&haute).is_some() {
        return None;
    }
    Some((prefixe.to_string(), haute))
}

/// Reconnait `n.prop STARTS WITH 'litteral'` et rend `(variable, propriete,
/// prefixe)`.
///
/// Le litteral doit etre une CHAINE : `STARTS WITH` sur autre chose ne
/// s'evalue a vrai pour aucune ligne (l'evaluateur fait `as_str()?` des deux
/// cotes), et le traduire en plage lui donnerait une reponse qu'il n'a pas.
pub(super) fn extraire_prefixe(predicate: &LogicalExpression) -> Option<(String, String, String)> {
    let LogicalExpression::Binary {
        left,
        op: BinaryOp::StartsWith,
        right,
    } = predicate
    else {
        return None;
    };
    let (
        LogicalExpression::Property { variable, property },
        LogicalExpression::Literal(Value::String(prefixe)),
    ) = (left.as_ref(), right.as_ref())
    else {
        return None;
    };
    Some((variable.clone(), property.clone(), prefixe.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// L'equivalence, verifiee EXHAUSTIVEMENT par force brute sur un alphabet
    /// reduit : pour tout prefixe et toute chaine du domaine, appartenir a
    /// l'intervalle et commencer par le prefixe doivent coincider.
    ///
    /// C'est la preuve que le pave de tete enonce ; la faire tourner coute
    /// quelques milliers de comparaisons et ferme la question.
    #[test]
    fn intervalle_et_prefixe_coincident_exhaustivement() {
        let alphabet = ['a', 'b', 'c'];
        let mut domaine: Vec<String> = vec![String::new()];
        for _ in 0..4 {
            let mut suivant = Vec::new();
            for mot in &domaine {
                for c in alphabet {
                    suivant.push(format!("{mot}{c}"));
                }
            }
            domaine.extend(suivant);
        }

        for prefixe in domaine.iter().filter(|p| !p.is_empty()) {
            let (min, max) = plage_de_prefixe(prefixe)
                .unwrap_or_else(|| panic!("plage attendue pour {prefixe:?}"));
            for x in &domaine {
                let par_prefixe = x.starts_with(prefixe.as_str());
                let par_intervalle = x.as_str() >= min.as_str() && x.as_str() < max.as_str();
                assert_eq!(
                    par_prefixe, par_intervalle,
                    "desaccord sur x={x:?} pour le prefixe {prefixe:?} \
                     (intervalle [{min:?}, {max:?}))"
                );
            }
        }
    }

    /// Le voisinage immediat de la borne haute, ecrit a la main : c'est le
    /// cas qui distingue une borne haute juste d'une borne haute naive.
    #[test]
    fn la_borne_haute_exclut_le_voisin_lexicographique() {
        let (min, max) = plage_de_prefixe("self:").expect("plage");
        assert_eq!(min, "self:");
        assert_eq!(max, "self;", "':' + 1 == ';'");
        // 'selg' est SUPERIEUR a 'self:' en octets et doit pourtant sortir.
        assert!(
            "selg" > min.as_str(),
            "temoin : 'selg' passe la borne basse"
        );
        assert!(
            "selg" >= max.as_str(),
            "…et c'est la borne haute qui l'exclut"
        );
    }

    /// Prefixe multi-octets : la borne haute doit rester de l'UTF-8 valide,
    /// ce qu'un increment d'octet ne garantit pas.
    #[test]
    fn borne_haute_d_un_prefixe_multi_octets() {
        let haute = borne_haute_de_prefixe("cafe\u{e9}").expect("borne");
        assert_eq!(haute, "cafe\u{ea}", "e-aigu U+00E9 -> U+00EA");
        assert!(haute.is_char_boundary(haute.len()));
    }

    /// Les desistements de construction.
    #[test]
    fn desistements_de_construction() {
        assert_eq!(borne_haute_de_prefixe(""), None, "prefixe vide");
        assert_eq!(
            borne_haute_de_prefixe(&format!("a{}", char::MAX)),
            None,
            "char::MAX n'a pas de successeur"
        );
        assert_eq!(
            borne_haute_de_prefixe(&format!(
                "k{}",
                char::from_u32(0xD7FF).expect("U+D7FF est valide")
            )),
            None,
            "U+D800 est un surrogate : pas de char"
        );
    }

    /// LE DESISTEMENT DATETIME. La borne haute existe, la plage est refusee
    /// quand meme — parce que l'ordre du substrat cesse d'etre celui des
    /// octets. Voir le pave de tete et le test d'integration
    /// `b_prefixe_datetime_ne_doit_pas_elargir_le_resultat`.
    #[test]
    fn desistement_sur_un_prefixe_datetime() {
        use obrain_common::types::value_cmp::instant_de_chaine;
        let p = "2024-01-01T00:00:00Z";
        assert!(
            instant_de_chaine(p).is_some(),
            "temoin : ce prefixe EST analysable en instant"
        );
        assert!(
            borne_haute_de_prefixe(p).is_some(),
            "temoin : une borne haute existe pourtant"
        );
        assert_eq!(plage_de_prefixe(p), None, "la plage doit etre refusee");
    }

    /// Desistement quand c'est la BORNE HAUTE, et non le prefixe, qui devient
    /// analysable. `'…:00Y'` n'est pas un instant ('Y' n'est pas un fuseau) ;
    /// son successeur `'…:00Z'` en est un.
    #[test]
    fn desistement_quand_seule_la_borne_haute_est_un_instant() {
        use obrain_common::types::value_cmp::instant_de_chaine;
        let p = "2024-01-01T00:00:00Y";
        assert!(instant_de_chaine(p).is_none(), "temoin : le prefixe, non");
        let haute = borne_haute_de_prefixe(p).expect("borne");
        assert_eq!(haute, "2024-01-01T00:00:00Z");
        assert!(
            instant_de_chaine(&haute).is_some(),
            "temoin : la borne haute, si"
        );
        assert_eq!(plage_de_prefixe(p), None, "desistement exige");
    }

    #[test]
    fn extraction_du_motif() {
        let motif =
            |gauche: LogicalExpression, droite: LogicalExpression| LogicalExpression::Binary {
                left: Box::new(gauche),
                op: BinaryOp::StartsWith,
                right: Box::new(droite),
            };
        let prop = LogicalExpression::Property {
            variable: "n".into(),
            property: "nom".into(),
        };

        assert_eq!(
            extraire_prefixe(&motif(
                prop.clone(),
                LogicalExpression::Literal(Value::String("self:".into()))
            )),
            Some(("n".to_string(), "nom".to_string(), "self:".to_string()))
        );

        // Litteral NON-chaine : l'evaluateur ne retient rien, la plage ne
        // doit donc rien retenir non plus.
        assert_eq!(
            extraire_prefixe(&motif(
                prop.clone(),
                LogicalExpression::Literal(Value::Int64(1))
            )),
            None
        );

        // Forme RETOURNEE : `STARTS WITH` n'est pas commutatif.
        assert_eq!(
            extraire_prefixe(&motif(
                LogicalExpression::Literal(Value::String("self:".into())),
                prop.clone()
            )),
            None
        );

        // Un autre operateur.
        assert_eq!(
            extraire_prefixe(&LogicalExpression::Binary {
                left: Box::new(prop),
                op: BinaryOp::Contains,
                right: Box::new(LogicalExpression::Literal(Value::String("self:".into()))),
            }),
            None
        );
    }
}
