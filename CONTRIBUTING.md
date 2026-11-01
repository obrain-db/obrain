# Contribuer à Obrain

## Mise en place

```bash
git clone https://github.com/obrain-db/obrain.git
cd obrain
cargo build --workspace
```

Rust **1.91.1** au minimum (`rust-version` du manifeste, vérifié par la CI). Aucun autre outil n'est requis : ce dépôt ne contient ni bindings (Python, Node, Go, WASM…) ni CLI — le CLI vit dans `obrain-db/obrain-cli`.

## Les crates

| Crate | Rôle |
| ----- | ---- |
| `obrain` | Façade : `ObrainDB`, sessions, profils de features |
| `obrain-common` | Types de base, erreurs, utilitaires |
| `obrain-core` | Modèle de graphe, index, exécution |
| `obrain-substrate` | Stockage « topology-as-storage » : zones mmap, WAL, rejeu, checkpoints |
| `obrain-adapters` | Parseurs (GQL par défaut ; Cypher, SPARQL, Gremlin, GraphQL, SQL/PGQ derrière des features), algorithmes, plugins |
| `obrain-reactive` | Bus de mutations, événements |
| `obrain-cognitive` | Couche cognitive (énergie, synapses, tissu, mémoire) — aussi extraite dans `obrain-db/obrain-cognitive` |
| `obrain-engine` | Moteur : base de données, sessions, transactions, procédures, UDF |

Ordre de publication (chaque crate dépend des précédents) : common → core → substrate → adapters, reactive → cognitive → engine → obrain.

## Faire un changement

1. Une branche par changement, coupée de `main` ; jamais de push sur `main` : tout passe par une PR.
2. Avant de pousser : `./scripts/ci-local.sh --quick` (format, clippy, doc, tests), ou au minimum `cargo fmt --all` et `scripts/clippy-ratchet.sh`.
3. La CI (`.github/workflows/ci.yml`) joue : format ; clippy en **cliquet** (`clippy-baseline`, le nombre d'avertissements ne monte jamais dans une PR ; s'il descend, abaissez le plafond avec `scripts/clippy-ratchet.sh --update`) ; tests en features par défaut ; compilation `--all-features` ; MSRV ; `cargo deny` ; métadonnées de publication ; tests d'intégrité sur macOS.
4. Fusion en *squash* ; une PR = un sujet.

Ce que la CI ne joue pas et qu'il faut lancer à la main avant une version : `cargo test --workspace --all-features`, `cargo test --workspace --release`, les tests `#[ignore]` (`mesures.yml`, à la demande), les benchs (`make bench-v001`, voir `docs/BENCHMARKS.md`).

## Règles du dépôt

- **Un chiffre sans commande, matériel et charge n'est pas un chiffre** (`docs/BENCHMARKS.md`).
- **Une feature annoncée dans le README est une feature exercée** par `crates/obrain/tests/readme_examples.rs` ; sinon elle va dans `docs/LIMITES-v0.0.1.md`.
- Un test `#[ignore]` porte sa raison **et sa mesure** ; on ne masque pas un échec par un retry, `--test-threads=1` ou une assertion retirée.
- Pas de retrait d'historique, de force-push ni de modification de `main` hors PR.
- Rien de personnel dans le dépôt : ni chemin de poste, ni hôte, ni IP, ni clé.

## Style

- `rustfmt` et `clippy` (pédantique, avec les `allow` du manifeste de l'espace de travail) ; `thiserror` pour les erreurs ; `unsafe` justifié par un commentaire `SAFETY`.
- Les tests unitaires vivent dans le fichier sous `#[cfg(test)]`, les tests d'intégration dans `crates/<crate>/tests/`.
- Messages de commit conventionnels : `feat:`, `fix:`, `docs:`, `test:`, `refactor:`, `perf:`, `ci:`, `chore:`.

## Licence

Apache-2.0 (`LICENSE`). Obrain dérive de GrafeoDB ; l'attribution est dans `NOTICE`.
