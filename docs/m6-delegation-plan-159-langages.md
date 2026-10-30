# Plan de délégation — 159 langages tree-sitter (M6, étape 2)

**Date** : 2026-09-06 — branche `feat/m6-code-graph` (worktree `banc-obrain-m6`).
**HEAD** : `b416c423` (étape 1 : 161 specs générées dans `lang_specs.rs`).

---

## 1. Architecture cible (pourquoi la délégation est sûre)

```
crates/obrain-indexer/src/
├── lang_specs.rs        # DÉJÀ FAIT : 161 LangSpec (types de nœuds par langage)
├── spec_engine.rs       # ÉTAPE 2a — framework d'extraction GÉNÉRIQUE piloté par spec
│                        #   (écrit par l'intégrateur SEUL — définit le contrat)
├── grammars/            # ÉTAPE 2b — UN module par langage, par LOT
│   ├── mod.rs           #   registre central (intégrateur seul)
│   ├── lot1/            #   fichiers d'un agent (rust déjà fait, ruby, php, …)
│   ├── lot2/            #   fichiers d'un autre agent
│   └── …
├── tests/lang/          # tests d'ACCEPTATION réels (fichiers d'exemple par langage)
│   ├── ruby/            #   sample.rb + expected (noms de symboles attendus)
│   └── …
```

**Principe d'isolation** : chaque agent écrit UNIQUEMENT dans son dossier
`grammars/lotN/` + `tests/lang/<lang>/` — JAMAIS dans `mod.rs`, `lang_specs.rs`,
`spec_engine.rs`, `Cargo.toml` (gérés par l'intégrateur). Zéro conflit
d'écriture possible si cette règle est respectée.

## 2. Rôles

| Rôle | Qui | Fichiers |
|---|---|---|
| **Intégrateur** | l'agent parent (moi) | `spec_engine.rs`, `grammars/mod.rs`, `Cargo.toml`, commits |
| **Agents de lot** | sous-agents (1 par lot de 5-8 langages) | `grammars/lotN/*.rs`, `tests/lang/<lang>/*` |
| **Validateur** | sous-agent dédié (après intégration) | exécute la suite, compare au C |

## 3. Découpage en lots (ordre de priorité)

La couverture du corpus obrain guide l'ordre. Lot 0 = déjà fait (rust, python,
go, java, c, cpp, javascript, typescript — modules MANUELS, à migrer vers le
framework ensuite).

| Lot | Langages | Rationale |
|---|---|---|
| 1 | markdown, toml, yaml, json, bash, shell | présent dans le corpus obrain (260 fichiers non couverts) |
| 2 | ruby, php, kotlin, swift, dart, c_sharp, scala | écosystème courant |
| 3 | css, html, sql, hcl, dockerfile, make, cmake | config/infra |
| 4 | elixir, haskell, ocaml, fsharp, clojure, commonlisp, lua | fonctionnel |
| 5 | perl, groovy, erlang, julia, r, zig, nim, crystal | divers |
| 6+ | les ~90 restants (exotiques : ada, cobol, verilog…) | complétude |

Chaque lot = 1 agent. Maximum 3-4 agents en PARALLÈLE (contrainte CPU/disque,
§5).

## 4. Protocole par lot (ce que chaque agent fait)

1. **Lire** : `lang_specs.rs` (la spec de SES langages) + `spec_engine.rs` (le
   framework — l'API exacte) + `grammars/rust.rs` (exemple de module branché).
2. **Écrire** dans `grammars/lotN/` : un fichier par langage qui implémente le
   trait du framework (généralement juste : grammaire tree-sitter + spec →
   l'extraction est générique).
3. **Tests réels** dans `tests/lang/<lang>/` : 2-3 fichiers d'exemple par
   langage (code réel typique) + un test qui vérifie les SYMBOLES EXTRAITS
   (noms de fonctions/classes attendus). PAS des tests triviaux « parse sans
   erreur » — des tests de RICHESSE (le C trouve X symboles dans ce fichier).
4. **Valider LOCALEMENT** : `cargo test -p obrain-indexer -- <lot>` dans SON
   worktree (voir §5 — worktrees par agent).
5. **Rapporter** : liste des fichiers créés + résultats de tests. NE COMMITE PAS.

## 5. Contraintes matérielles (CRITIQUE — à respecter absolument)

Machine : 8 cœurs physiques / 16 threads, 62 Go RAM, **50 Go disque libres**,
charge externe variable (daemon C + bridges obrain).

**RÈGLES** :
1. **Un agent = un worktree git dédié** (`git worktree add`) avec SON target.
   → `cargo build` concurrents ne se marchent pas dessus (target/ séparés).
2. **Jamais plus de 3 agents qui COMPILENT en même temps** (le cargo lock +
   l'espace disque). Les autres agents écrivent du code/tests SANS compiler,
   puis compilent quand un slot se libère.
3. **Chaque grammaire tree-sitter coûte ~100-300 Mo de target** en compilation
   C. Pour 159 langages ≈ 15-45 Go. Le travail se fait PAR LOTS avec
   `cargo clean -p` entre les gros lots si le disque approche 80 %.
4. **Les tests de chaque agent tournent sur SON worktree** — pas de course
   sur le target partagé.
5. Vérifier l'espace AVANT chaque gros build : `df -h /` — si < 20 Go libres,
   nettoyer (`cargo clean` des lots précédents) avant de continuer.

## 6. Validation finale (après tous les lots)

1. Tous les modules branchés dans `grammars/mod.rs` (intégrateur).
2. **Tests réels** : la suite `tests/lang/**` doit passer (richesse minimale
   par langage — chaque fichier d'exemple produit les symboles attendus).
3. **Comparaison au C** (le critère ultime) : sur le corpus de référence
   (arbre obrain), le nombre de fonctions/classes extraites par le framework
   pour chaque langage doit être ≥ celui du C (méthodologie : la base
   `tmp-m6-ref-arbre.db` sert de référence — comparaison par label, déjà
   validée pour Rust : 100 % méthodes, 99,97 % fonctions).
4. Suites vertes complètes, clippy 0, commit par lot.

## 7. Définition de done de l'ÉTAPE 2

- `spec_engine.rs` : extracteur générique testé sur les 8 langages manuels
  (doit reproduire leurs résultats — les specs Rust/Go/Python existent).
- Les 8 modules manuels migrés vers le framework (ou coexistants si la
  richesse du framework est inférieure — décision mesurée).
- Lots 1-3 livrés (langages du corpus + écosystème courant) avec tests réels.
- Lots 4+ en cours (complétude 159).
