//! Détection des fichiers/fonctions de test — équivalent Rust de
//! `cbm_is_test_file` (codebase-memory-mcp internal/cbm/helpers.c).
//!
//! Un fichier est un fichier de test si son chemin contient un répertoire de
//! test conventionnel (tests/, test/, spec/, __tests__/) OU si son basename
//! suit une règle par langage (ex: *_test.go, test_*.py, *.test.js…).

/// Détecte si un chemin relatif de fichier est un fichier de test.
/// `rel_path` : chemin relatif au repo (ex: "crates/foo/tests/bar.rs").
pub fn is_test_file(rel_path: &str) -> bool {
    // Répertoires conventionnels (agnostique au langage).
    let p = rel_path.replace('\\', "/");
    for dir in ["__tests__/", "/tests/", "/test/", "/spec/"] {
        if p.contains(dir) {
            return true;
        }
    }
    for prefix in ["tests/", "test/", "spec/", "__tests__/"] {
        if p.starts_with(prefix) {
            return true;
        }
    }

    let base = p.rsplit('/').next().unwrap_or(&p);
    let ext = base.rsplit('.').next().unwrap_or("");
    let stem = base.strip_suffix(&format!(".{ext}")).unwrap_or(base);

    match ext {
        "go" => base.ends_with("_test.go"),
        "py" => base.starts_with("test_") || base.ends_with("_test.py"),
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => {
            stem.ends_with(".test")
                || stem.ends_with(".spec")
                || stem.ends_with("_test")
                || stem.ends_with("_spec")
                || base.starts_with("test_")
        }
        "java" | "kt" | "scala" => {
            base.ends_with("Test.java")
                || base.ends_with("Tests.java")
                || base.ends_with("Spec.java")
                || base.ends_with("Test.kt")
                || base.ends_with("Spec.kt")
                || base.ends_with("Test.scala")
                || base.ends_with("Spec.scala")
        }
        "rs" => base.ends_with("_test.rs") || base.starts_with("test_"),
        "rb" => {
            base.ends_with("_test.rb") || base.ends_with("_spec.rb") || base.starts_with("test_")
        }
        "php" => base.ends_with("Test.php"),
        "cs" => base.ends_with("Tests.cs") || base.ends_with("Test.cs"),
        "c" | "h" | "cc" | "cpp" | "cxx" | "hh" | "hpp" => {
            base.ends_with("_test.c")
                || base.ends_with("_test.cc")
                || base.ends_with("_test.cpp")
                || base.starts_with("test_")
        }
        _ => false,
    }
}

/// Détecte si un nom de fonction est un test (conventions par langage +
/// attributs). `lang` : nom court du langage (rust/python/go/…).
pub fn is_test_function(name: &str, lang: &str) -> bool {
    match lang {
        "rust" => name == "test" || name.starts_with("test_"),
        "python" => name.starts_with("test_"),
        "go" => {
            name.starts_with("Test") || name.starts_with("Benchmark") || name.starts_with("Example")
        }
        "javascript" | "typescript" => {
            name.starts_with("test") || name.starts_with("it(") || name.starts_with("describe")
        }
        "java" => name.starts_with("test"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repertoires_conventionnels() {
        assert!(is_test_file("crates/foo/tests/helper.rs"));
        assert!(is_test_file("src/test/mod.rs"));
        assert!(is_test_file("tests/integration.rs"));
        assert!(is_test_file("packages/app/__tests__/x.spec.js"));
    }

    #[test]
    fn conventions_par_langage() {
        assert!(is_test_file("math_test.go"));
        assert!(!is_test_file("math.go"));
        assert!(is_test_file("test_math.py"));
        assert!(!is_test_file("math.py"));
        assert!(is_test_file("math.test.js"));
        assert!(is_test_file("math.spec.ts"));
        assert!(is_test_file("MathTest.java"));
        assert!(is_test_file("user_test.rs"));
        assert!(!is_test_file("user.rs"));
    }
}

/// Règle d'export par langage (équivalent cbm_is_exported du C) :
/// Go/Java/C#/Kotlin → exporté si le nom commence par une majuscule ;
/// Python → exporté si ne commence pas par `_` ; défaut → exporté.
pub fn is_exported(name: &str, lang: &str) -> bool {
    let Some(c) = name.chars().next() else {
        return false;
    };
    match lang {
        "go" | "java" | "csharp" | "kotlin" | "c" | "cpp" => c.is_uppercase(),
        "python" | "ruby" => c != '_',
        _ => true,
    }
}

#[cfg(test)]
mod tests_export {
    use super::*;

    #[test]
    fn regles_par_langage() {
        assert!(is_exported("PublicFunc", "go"));
        assert!(!is_exported("privateFunc", "go"));
        assert!(is_exported("MyClass", "java"));
        assert!(is_exported("helper", "python"));
        assert!(!is_exported("_internal", "python"));
        assert!(is_exported("whatever", "rust"));
    }
}
