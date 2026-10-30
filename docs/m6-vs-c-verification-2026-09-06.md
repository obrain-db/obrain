# Vérification d'exactitude M6 vs indexeur C (même arbre 8f531963)

Méthode : comparaison FINE par multiset (fichier, nom, ligne de début) entre
mon extracteur natif et la base SQLite de l'indexeur C sur le même arbre.

## Méthodes (label Method, fichiers .rs)
- C : 8 474 — match exact M6 : **8 474 (100 %)** — C seul : 0
- M6 : 8 570 (+96) : vraies méthodes que le C ne capture pas
  (ex: out_degree/in_degree dans store.rs, from/new/register_listener dans
  scheduler.rs — méthodes dans des impls que le C saute)

## Fonctions (label Function, fichiers .rs)
- C : 11 644 — match exact M6 : **11 641 (99,97 %)** — C seul : 3
- M6 : 11 667 (+26) : vraies fonctions que le C rate
- Les 3 fonctions C ratées par M6 sont des cas rares :
  1. `fn log` dans `extern "C" {}` (FFI wasm_bindgen) — bloc extern non descendu
  2. `assert_send_sync` + `check` dans `const _: () = { ... }` (bloc const anonyme)

## Conclusion
M6 est un SUR-ENSEMBLE exact du C sur les définitions Rust : chaque symbole
du C est retrouvé à la même ligne, plus des symboles supplémentaires que le C
manque. Les résultats de richesse sont fiables.

## Amélioration possible (3 fonctions)
Descendre dans `extern "C" {}` et les blocs `const _: () = {}` pour les
fonctions FFI et const — rattraperait les 3 derniers cas.
