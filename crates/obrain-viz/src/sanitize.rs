//! Échappement et sanitisation **partagés** entre les writers de formats
//! (DOT, Mermaid, plus tard GraphML). Une seule source de vérité pour les
//! caractères hostiles (`"`, `\`, `\n`, `#`, non-ASCII…) — c'est ce qui
//! garantit qu'un label hostile rend encore dans chaque format.

/// FNV-1a 64 bits — hash déterministe **stable entre versions de Rust**
/// (contrairement à `DefaultHasher`), utilisé pour dériver des ids Mermaid
/// sûrs quand l'id source n'est pas un identifiant `[A-Za-z0-9_]+`.
#[must_use]
pub fn fnv1a64(input: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Échappe une chaîne pour un contexte DOT entre guillemets :
/// `\` → `\\`, `"` → `\"`, sauts de ligne → séquences d'échappement.
#[must_use]
pub fn dot_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

/// Encadre une chaîne de guillemets DOT en échappant son contenu
/// (utilisable comme id de nœud ou comme valeur d'attribut).
#[must_use]
pub fn dot_quote(input: &str) -> String {
    format!("\"{}\"", dot_escape(input))
}

/// Id Mermaid sûr et déterministe : l'id source est conservé s'il est déjà
/// `[A-Za-z0-9_]+` non vide et ne commence pas par un chiffre ; sinon il est
/// remplacé par `n` + hash FNV-1a hexadécimal. Les collisions sont détectées
/// par le writer (qui maintient la bijection id → id Mermaid).
#[must_use]
pub fn mermaid_node_id(id: &str) -> String {
    let ascii_ok = !id.is_empty()
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !id.as_bytes()[0].is_ascii_digit();
    if ascii_ok {
        return id.to_string();
    }
    format!("n{:016x}", fnv1a64(id))
}

/// Échappe un libellé Mermaid (contenu d'un nœud ou texte d'une arête) avec
/// les codes d'entité Mermaid : `&` → `#amp;`, `"` → `#quot;`, `#` → `#35;`,
/// `<`/`>` → `#lt;`/`#gt;`, saut de ligne → espace.
#[must_use]
pub fn mermaid_label(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '&' => out.push_str("#amp;"),
            '"' => out.push_str("#quot;"),
            '#' => out.push_str("#35;"),
            '<' => out.push_str("#lt;"),
            '>' => out.push_str("#gt;"),
            '\n' | '\r' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{dot_escape, dot_quote, fnv1a64, mermaid_label, mermaid_node_id};

    #[test]
    fn fnv1a64_empty_known_vector() {
        assert_eq!(fnv1a64(""), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn fnv1a64_is_stable() {
        assert_eq!(fnv1a64("fn:analyze"), fnv1a64("fn:analyze"));
        assert_ne!(fnv1a64("a"), fnv1a64("b"));
    }

    #[test]
    fn dot_escape_covers_hostile_chars() {
        let raw = "a\"b\\c\nd\re\tf";
        let esc = dot_escape(raw);
        assert_eq!(esc, "a\\\"b\\\\c\\nd\\re\\tf");
        // Chaque guillemet restant est échappé (précédé d'un backslash).
        assert!(esc.contains("\\\""));
        assert!(!esc.contains('\n'));
        assert!(!esc.contains('\r'));
        assert!(!esc.contains('\t'));
    }

    #[test]
    fn dot_quote_wraps_and_escapes() {
        assert_eq!(dot_quote("fn:parse"), "\"fn:parse\"");
        assert_eq!(dot_quote("a\"b"), "\"a\\\"b\"");
    }

    #[test]
    fn mermaid_node_id_keeps_plain_identifiers() {
        assert_eq!(mermaid_node_id("analyze_2"), "analyze_2");
        assert_ne!(mermaid_node_id("2analyze"), "2analyze"); // chiffre initial → hash
    }

    #[test]
    fn mermaid_node_id_is_deterministic_and_prefixed() {
        let a = mermaid_node_id("fn:/usr/deps");
        let b = mermaid_node_id("fn:/usr/deps");
        assert_eq!(a, b);
        assert!(a.starts_with('n'), "{a}");
    }

    #[test]
    fn mermaid_label_escapes_entities() {
        assert_eq!(
            mermaid_label("a\"b#c&d<e>f\ng"),
            "a#quot;b#35;c#amp;d#lt;e#gt;f g"
        );
    }
}
