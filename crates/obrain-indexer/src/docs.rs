//! Extraction de docstring — équivalent Rust d'extract_docstring
//! (codebase-memory-mcp internal/cbm/extract_defs.c).
//!
//! Règle générique : le docstring d'une définition est le commentaire qui la
//! précède directement (frère précédent). Si plusieurs commentaires se
//! succèdent, on remonte au plus haut (le bloc doc). Texte brut tronqué à
//! MAX_DOC_LEN (comme le C — pas de nettoyage des marqueurs).

use tree_sitter::Node;

/// Longueur max du docstring stocké (le C : MAX_COMMENT_LEN).
pub const MAX_DOC_LEN: usize = 500;

fn is_comment_node(kind: &str) -> bool {
    matches!(
        kind,
        "comment" | "block_comment" | "line_comment" | "multiline_comment"
    )
}

/// Extrait le docstring d'un nœud de définition : remonte les commentaires
/// frères précédents consécutifs et retourne le plus haut (tronqué).
pub fn docstring_of<'a>(node: Node<'a>, source: &str) -> Option<String> {
    // Commentaire(s) frère(s) précédent(s) — remonte jusqu'à 2 consécutifs.
    let mut prev = node.prev_sibling();
    let mut doc: Option<String> = None;
    let mut depth = 0;
    while let Some(p) = prev {
        if !is_comment_node(p.kind()) {
            break;
        }
        let t = text_of(p, source);
        if !t.is_empty() {
            doc = Some(t);
        }
        depth += 1;
        if depth >= 3 {
            break; // ne remonte pas au-delà de 2-3 commentaires consécutifs
        }
        prev = p.prev_sibling();
    }
    doc.map(|d| {
        if d.chars().count() > MAX_DOC_LEN {
            d.chars().take(MAX_DOC_LEN).collect()
        } else {
            d
        }
    })
}

fn text_of<'a>(node: Node<'a>, source: &'a str) -> String {
    source[node.byte_range()].trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    fn first_fn(src: &str) -> Option<String> {
        let mut p = Parser::new();
        p.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
        let tree = p.parse(src, None).unwrap();
        fn find<'a>(n: tree_sitter::Node<'a>, out: &mut Option<tree_sitter::Node<'a>>) {
            if n.kind() == "function_item" {
                *out = Some(n);
                return;
            }
            let mut cur = n.walk();
            for c in n.children(&mut cur) {
                find(c, out);
                if out.is_some() {
                    return;
                }
            }
        }
        let mut found = None;
        find(tree.root_node(), &mut found);
        found.map(|f| docstring_of(f, src).unwrap_or_default())
    }

    #[test]
    fn docstring_devant_fn() {
        let src =
            "/// Calcule la somme.\n/// Deux lignes.\nfn sum(a: i32, b: i32) -> i32 { a + b }\n";
        let doc = first_fn(src).unwrap();
        assert!(doc.contains("somme"), "doc: {doc}");
    }

    #[test]
    fn pas_de_docstring_sans_commentaire() {
        let src = "fn sum(a: i32, b: i32) -> i32 { a + b }\n";
        let doc = first_fn(src).unwrap();
        assert!(doc.is_empty(), "pas de doc attendu: {doc}");
    }

    #[test]
    fn docstring_commentaire_normal() {
        let src = "// addition simple\nfn sum(a: i32, b: i32) -> i32 { a + b }\n";
        let doc = first_fn(src).unwrap();
        assert!(doc.contains("addition"), "doc: {doc}");
    }
}
