//! Aucune requête, si imbriquée soit-elle, ne doit épuiser la pile : elle est
//! refusée par une erreur. Chaque cas tourne dans un thread à pile RÉDUITE (1 Mio,
//! la pile d'une instance wasm32) : un dépassement abandonnerait tout le processus
//! de test, donc l'échec serait bruyant.

use obrain_adapters::query::imbrication::{IMBRICATION_MAX, PROFONDEUR_EXPRESSION_MAX};

const PILE: usize = 1024 * 1024;

fn en_pile_reduite<F: FnOnce() + Send + 'static>(f: F) {
    std::thread::Builder::new()
        .stack_size(PILE)
        .spawn(f)
        .unwrap()
        .join()
        .expect("le thread ne doit pas paniquer");
}

fn parentheses(n: usize, corps: &str) -> String {
    format!("{}{corps}{}", "(".repeat(n), ")".repeat(n))
}

/// Une apostrophe dans un commentaire de ligne ouvre une « chaîne » pour le
/// compteur d'imbrication, qui ne voit plus rien de la suite : c'est le
/// contournement connu, que les compteurs de profondeur des analyseurs doivent
/// couvrir. Chaque langage a sa syntaxe de commentaire et de motif.
fn contournements_gql(n: usize) -> Vec<String> {
    vec![
        format!("-- l'utilisateur\nRETURN {}", parentheses(n, "1")),
        format!("-- l'utilisateur\nMATCH {} RETURN 1", parentheses(n, "x")),
    ]
}
#[allow(dead_code)] // utilisée seulement si la feature du langage est active
fn contournements_cypher(n: usize) -> Vec<String> {
    vec![
        format!("// l'utilisateur\nRETURN {}", parentheses(n, "1")),
        format!("// l'utilisateur\nMATCH {} RETURN 1", parentheses(n, "x")),
    ]
}
#[allow(dead_code)] // utilisée seulement si la feature du langage est active
fn contournements_sql_pgq(n: usize) -> Vec<String> {
    vec![format!("-- l'utilisateur\nSELECT {}", parentheses(n, "1"))]
}
#[allow(dead_code)] // utilisée seulement si la feature du langage est active
fn contournements_sparql(n: usize) -> Vec<String> {
    vec![
        format!(
            "# l'utilisateur\nSELECT * WHERE {{ ?s {} ?o }}",
            parentheses(n, "<http://p>")
        ),
        format!(
            "# l'utilisateur\nSELECT * WHERE {{ ?s ?p ?o FILTER {} }}",
            parentheses(n, "1")
        ),
    ]
}
#[allow(dead_code)] // utilisée seulement si la feature du langage est active
fn aucun_contournement(_n: usize) -> Vec<String> {
    Vec::new()
}

macro_rules! langage {
    ($mod:ident, $feature:literal, $requete:expr, $contournements:expr) => {
        #[cfg(feature = $feature)]
        mod $mod {
            use super::*;
            use obrain_adapters::query::$mod::parse;

            #[test]
            fn au_dela_de_la_limite_est_une_erreur() {
                for n in [IMBRICATION_MAX + 1, 400, 5_000, 50_000] {
                    en_pile_reduite(move || {
                        let q: String = ($requete)(n);
                        let e = parse(&q).expect_err("trop imbriquée : doit être refusée");
                        assert!(e.to_string().contains("trop imbriqu"), "n={n} : {e}");
                    });
                }
            }

            #[test]
            fn la_requete_vide_et_les_requetes_courtes_ne_paniquent_pas() {
                for q in ["", " ", "(", ")", "((((", "))))", "[", "{", "'"] {
                    let _ = parse(q);
                }
            }

            #[test]
            fn le_contournement_par_commentaire_ne_fait_pas_tomber_la_pile() {
                // +2 : dans un motif `((((x))))` le niveau le plus intérieur est un nœud, pas un
                // motif parenthésé — n parenthèses ne font que n − 1 motifs imbriqués.
                for n in [PROFONDEUR_EXPRESSION_MAX + 2, 400, 5_000] {
                    for q in ($contournements)(n) {
                        en_pile_reduite(move || {
                            // Le seul critère : une erreur, jamais un abandon du processus.
                            assert!(parse(&q).is_err(), "n={n}");
                        });
                    }
                }
            }
        }
    };
}

langage!(
    gql,
    "gql",
    |n| format!("RETURN {}", parentheses(n, "1")),
    contournements_gql
);
langage!(
    cypher,
    "cypher",
    |n| format!("RETURN {}", parentheses(n, "1")),
    contournements_cypher
);
langage!(
    sparql,
    "sparql",
    |n| format!(
        "SELECT * WHERE {{ ?s ?p ?o FILTER {} }}",
        parentheses(n, "1")
    ),
    contournements_sparql
);
langage!(
    gremlin,
    "gremlin",
    |n| format!("g.V(){}", ".out(".repeat(n) + &")".repeat(n)),
    aucun_contournement
);
langage!(
    graphql,
    "graphql",
    |n| format!("{}a{}", "{ x ".repeat(n), " }".repeat(n)),
    aucun_contournement
);
langage!(
    sql_pgq,
    "sql-pgq",
    |n| format!("SELECT {}", parentheses(n, "1")),
    contournements_sql_pgq
);

#[cfg(feature = "gql")]
#[test]
fn gql_une_imbrication_raisonnable_reste_acceptee() {
    use obrain_adapters::query::gql::parse;
    for n in [1, 10, 50, IMBRICATION_MAX] {
        en_pile_reduite(move || {
            let q = format!("RETURN {}", parentheses(n, "1"));
            parse(&q).unwrap_or_else(|e| panic!("n={n} doit passer : {e}"));
        });
    }
}
