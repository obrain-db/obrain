//! Garde d'imbrication : refuse une requête dont les crochets s'imbriquent trop
//! profond, AVANT toute analyse.
//!
//! ## Pourquoi
//!
//! Les six analyseurs (GQL, Cypher, SPARQL, Gremlin, GraphQL, SQL/PGQ) sont
//! récursifs. Une requête `RETURN ((((…1…))))` ou `MATCH ((((…` assez profonde
//! épuise la pile et **abandonne le processus** (`SIGABRT`) — sans message, sans
//! erreur rattrapable. Mesuré le 05/11/2026 :
//!
//! | Cible | Pile | Effondrement |
//! |---|---|---|
//! | natif, thread de 2 Mio | 2 Mio | entre 200 et 400 niveaux (`RETURN`), entre 400 et 800 (`MATCH`) |
//! | natif, GQL, pile de 1 Mio | 1 Mio | 112 niveaux tiennent, 128 abandonnent (build de test optimisé) |
//! | `wasm32-unknown-unknown` | 1 Mio | entre 200 et 400 niveaux |
//!
//! Sur wasm32 la panique est un `abort` : une seule requête tue toute l'instance
//! du navigateur. Une requête écrite à la main ne dépasse jamais une vingtaine de
//! niveaux ; seule une requête générée ou hostile atteint ces profondeurs.
//!
//! ## Ce que fait la garde, et ce qu'elle ne fait pas
//!
//! Elle compte l'imbrication maximale de `(`, `[` et `{` hors des chaînes
//! (`'…'`, `"…"`, `` `…` ``, antislash échappant) et des commentaires bloc
//! (`/* … */`). La limite est la MÊME sur toutes les cibles, pour qu'une requête
//! acceptée en natif le soit aussi dans le navigateur.
//!
//! Les commentaires de ligne ne sont PAS interprétés : `//` est un morceau d'IRI
//! en SPARQL et `--` est une arête non orientée en GQL. Une apostrophe dans un
//! commentaire de ligne peut donc masquer le reste de la requête au comptage : la
//! garde protège des requêtes accidentelles ou naïvement hostiles, pas d'un
//! adversaire qui connaît cette limite. Les analyseurs d'expressions portent leur
//! propre compteur de profondeur pour ce cas (voir `PROFONDEUR_EXPRESSION_MAX`).

use obrain_common::utils::error::{Error, QueryError, QueryErrorKind, Result};

/// Imbrication maximale de `(`, `[`, `{` acceptée dans une requête.
///
/// MESURÉ (05/11/2026, natif, pile de 1 Mio) : l'analyseur GQL tient 112 niveaux
/// de parenthèses et s'effondre à 128, soit environ 8,7 Kio de pile par niveau
/// (chaque niveau traverse une douzaine de fonctions de précédence). 64 laisse un
/// facteur 2 de marge sur la plus petite pile visée (wasm32, 1 Mio), et reste très
/// au-dessus de ce qu'une requête écrite à la main atteint (une vingtaine).
pub const IMBRICATION_MAX: usize = 64;

/// Profondeur maximale de récursion d'une expression, dans les analyseurs qui
/// la comptent eux-mêmes. Un peu plus haute que [`IMBRICATION_MAX`] : n niveaux de
/// parenthèses traversent n + 1 expressions primaires, et un appel de fonction en
/// ajoute une de plus. Toute requête qui passe la garde en amont passe donc aussi
/// ce compteur ; il ne sert que si la garde en amont a été contournée.
pub const PROFONDEUR_EXPRESSION_MAX: usize = IMBRICATION_MAX + 16;

/// Message d'erreur commun, pour que les six langages disent la même chose.
fn trop_profond(octet: usize) -> Error {
    Error::Query(
        QueryError::new(
            QueryErrorKind::Syntax,
            format!(
                "requête trop imbriquée : plus de {IMBRICATION_MAX} niveaux de parenthèses, \
                 crochets ou accolades (à l'octet {octet})"
            ),
        )
        .with_hint(
            "découpez la requête, ou passez les valeurs en paramètres plutôt qu'en littéraux imbriqués",
        ),
    )
}

/// Vérifie l'imbrication de `source`.
///
/// # Errors
///
/// Une erreur de syntaxe si l'imbrication dépasse [`IMBRICATION_MAX`].
pub fn verifier(source: &str) -> Result<()> {
    let mut profondeur = 0usize;
    let mut it = source.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        match c {
            // Chaîne : on saute jusqu'au délimiteur fermant. Un délimiteur doublé
            // (`''`) ferme puis rouvre : le comptage reste juste.
            '\'' | '"' | '`' => {
                while let Some((_, d)) = it.next() {
                    if d == '\\' {
                        it.next();
                    } else if d == c {
                        break;
                    }
                }
            }
            // Commentaire bloc.
            '/' if matches!(it.peek(), Some((_, '*'))) => {
                it.next();
                let mut precedent = '\0';
                for (_, d) in it.by_ref() {
                    if precedent == '*' && d == '/' {
                        break;
                    }
                    precedent = d;
                }
            }
            '(' | '[' | '{' => {
                profondeur += 1;
                if profondeur > IMBRICATION_MAX {
                    return Err(trop_profond(i));
                }
            }
            ')' | ']' | '}' => profondeur = profondeur.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nid(n: usize) -> String {
        format!("RETURN {}1{}", "(".repeat(n), ")".repeat(n))
    }

    #[test]
    fn la_limite_exacte_passe_et_le_niveau_suivant_est_refuse() {
        assert!(verifier(&nid(IMBRICATION_MAX)).is_ok());
        let e = verifier(&nid(IMBRICATION_MAX + 1)).unwrap_err();
        assert!(e.to_string().contains("trop imbriquée"), "{e}");
    }

    #[test]
    fn les_trois_sortes_de_crochets_comptent_ensemble() {
        let triples = IMBRICATION_MAX / 3;
        let reste = IMBRICATION_MAX - triples * 3;
        let plein = format!("{}{}", "([{".repeat(triples), "(".repeat(reste));
        assert!(verifier(&plein).is_ok(), "exactement la limite");
        assert!(
            verifier(&format!("{plein}[")).is_err(),
            "un niveau de plus, d'une autre sorte"
        );
    }

    #[test]
    fn les_parentheses_en_sequence_ne_s_empilent_pas() {
        assert!(verifier(&"()".repeat(10_000)).is_ok());
    }

    #[test]
    fn les_chaines_ne_comptent_pas() {
        for d in ['\'', '"', '`'] {
            let q = format!("RETURN {d}{}{d}", "(".repeat(5_000));
            assert!(verifier(&q).is_ok(), "délimiteur {d}");
        }
        // antislash : le délimiteur échappé ne ferme pas la chaîne
        let q = format!("RETURN 'a\\'{}'", "(".repeat(5_000));
        assert!(verifier(&q).is_ok());
    }

    #[test]
    fn une_chaine_ne_remet_pas_le_compteur_a_zero() {
        // 100 ouvertes, une chaîne pleine de `)`, puis 100 ouvertes : 200 réelles.
        let q = format!(
            "{}'{}'{}",
            "(".repeat(100),
            ")".repeat(500),
            "(".repeat(100)
        );
        assert!(verifier(&q).is_err());
    }

    #[test]
    fn un_commentaire_bloc_ne_compte_pas() {
        let q = format!("RETURN /* {} */ 1", "(".repeat(5_000));
        assert!(verifier(&q).is_ok());
    }

    #[test]
    fn requete_vide_et_unicode() {
        assert!(verifier("").is_ok());
        assert!(verifier("RETURN 'é 日本 😀'").is_ok());
    }
}
