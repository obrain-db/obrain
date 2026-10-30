//! ── LES FONCTIONS FOURNIES PAR L'HÔTE ───────────────────────────────────────
//!
//! Une requête peut appeler une fonction que le moteur ne connaît pas :
//! `obrain.energy(n)`, `mon_algo(x)`, une métrique cognitive branchée par un
//! embedder. Jusqu'ici, l'évaluateur répondait — en toute honnêteté, mais
//! sèchement — « fonction inconnue : <nom> », et **rien** ne pouvait étendre le
//! vocabulaire : le registre de plugins d'`obrain-adapters`
//! (`PluginRegistry::register_udf`) existait, était thread-safe, documenté
//! comme « résolu au moment du planning »… et n'était lu par personne.
//!
//! ── POURQUOI UN CROCHET ICI, ET PAS UNE DÉPENDANCE ──────────────────────────
//! `obrain-core` est la couche basse : elle ne peut pas dépendre
//! d'`obrain-adapters` (c'est l'inverse). Le vocabulaire des fonctions
//! inconnues est donc un **point d'extension** : l'hôte (le binaire, un
//! embedder, les bindings) installe UNE fonction qui sait les résoudre, et le
//! noyau l'appelle avant de conclure à une faute de frappe.
//!
//! Le crochet est unique et se pose une fois par processus
//! ([`installer`] rend `false` si un autre hôte l'a déjà pris) : deux
//! résolveurs concurrents rendraient la réponse dépendante de l'ordre
//! d'initialisation, ce qui est exactement le genre de flottement qu'un
//! vocabulaire ne doit pas avoir.
//!
//! Vide par défaut : sans hôte, le comportement est INCHANGÉ (l'erreur
//! « fonction inconnue » reste, avec son message).

use std::sync::OnceLock;

use obrain_common::types::Value;

/// La fonction d'un hôte : `(nom, arguments) -> Some(résultat)` quand elle
/// connaît ce nom, `None` sinon.
///
/// `Some(Ok(valeur))` = la fonction a répondu ; `Some(Err(message))` = elle la
/// connaît mais a échoué (le message part tel quel dans l'erreur de requête) ;
/// `None` = ce nom n'est pas le sien, le noyau garde son message.
pub type ResolveurFonctions = dyn Fn(&str, &[Value]) -> Option<std::result::Result<Value, String>>
    + Send
    + Sync;

static RESOLVEUR: OnceLock<&'static ResolveurFonctions> = OnceLock::new();

/// Installe le résolveur de l'hôte. Rend `false` si un résolveur est déjà posé
/// (le premier gagne : un vocabulaire installé deux fois serait un flottement).
pub fn installer(resolveur: &'static ResolveurFonctions) -> bool {
    RESOLVEUR.set(resolveur).is_ok()
}

/// Y a-t-il un résolveur installé ? (Sert aux tests et à l'introspection.)
#[must_use]
pub fn installe() -> bool {
    RESOLVEUR.get().is_some()
}

/// Demande à l'hôte s'il connaît cette fonction.
///
/// Rend `None` quand aucun hôte n'est installé ou quand le nom n'est pas le
/// sien — dans les deux cas l'appelant garde sa conduite d'origine.
pub fn appeler(nom: &str, args: &[Value]) -> Option<std::result::Result<Value, String>> {
    RESOLVEUR.get().and_then(|r| r(nom, args))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Un test ne peut pas installer le résolveur global sans polluer les autres
    // (il est unique par processus) : on vérifie donc la PORTÉE du contrat —
    // l'absence d'hôte ne fait rien, et `appeler` ne répond pas d'un nom qu'il
    // ne connaît pas. L'installation réelle est éprouvée en bout de chaîne
    // (`obrain-engine/tests/fonction_hote.rs`), là où elle a un sens.
    #[test]
    fn sans_hote_aucune_reponse() {
        assert!(!installe(), "aucun hôte ne doit être installé dans ce test");
        assert!(appeler("mon_algo", &[Value::Int64(1)]).is_none());
    }
}
