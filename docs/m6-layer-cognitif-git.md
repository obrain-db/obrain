# Layer cognitif + tissage git de l'indexeur M6 (2026-09-07)

RFC courte : « la base obrain doit être plus rapide et plus riche de sens,
plus utile aux agents IA dans la compréhension du code ».

## Ce qui est branché à l'extraction (commits)

### Volet A — Layer cognitif local par fonction/méthode
1. **Metrics structurelles** (`b81edb7d`, `ce11a4c3`, `6ad13618`,
   `983ba87a`) : complexity (cyclomatique), cognitive (Campbell + pénalité
   d'imbrication), loop_count/loop_depth, max_access_depth — un seul walk
   AST par définition (metrics.rs = équivalent cbm_compute_complexity).
   Branché dans le framework spec (146 langages) + les 8 langages manuels.
   Écrit dans props.extra (clés C).
2. **Qualification** (`15b4156a` is_test, `7e1e9430`/`a036bf3b` docstring,
   `b860a8cf` is_exported) : is_test par fichier (répertoires conventionnels
   + règles par langage) et par nom de fonction ; is_exported par règle de
   langage (Go/Java majuscule, Python non-_) ; docstring = commentaire
   précédent (frère remonté).
3. **Passe interprocédurale** (`1ef37254`) : transitive_loop_depth +
   recursive le long des CALLS (DFS mémoïsé = pass_complexity du C).
4. **Passe TESTS** (`49fe30cf`) : edges TESTS tests → fonctions testées
   (scan des CALLS : caller is_test et callee non-test). 3 916 edges sur le
   worktree M6 (TestOpenInMemory → OpenInMemory/Close/NodeCount).

### Volet B — Tissage git
4. **git_context** (`00d24c5a`) : nœud Branch (HEAD sha, branche, base,
   is_worktree) + edge HAS_BRANCH — seulement si repo git.
5. **Historique + co-évolution** (`d15e32d3`) : git log 1 an → change_count/
   last_modified par File (hotspots) + edges FILE_CHANGES_WITH entre
   fichiers co-évoluants (≥3 co-changements, score ≥0,3, ≤20 fichiers/
   commit — merge ignoré).

## Mesures avant/après (même machine, mode release)

| Corpus | Avant layer | Après layer optimisé | Delta |
|---|---|---|---|
| obrain (1287 f, non-git) | 0,68 s | 0,77 s | +13 % (metrics) |
| worktree M6 (1409 f, git) | — | 1,09 s | layer complet + git |
| kernel drivers/net (5418 f) | 2,9 s | 3,9-4,3 s | +40 % (layer, graphe < C) |
| kernel complet (70 821 f) | 24,7 s | 28,5 s | +15 % (layer complet) |

Le surcoût initial (46,5 s) était dominé par les nœuds Macro des #define
C (237k sur drivers/net, 0 côté C) — retirés (`e8d53a7e`) : graphe divisé
par ~2, temps kernel -39 %. Le layer cognitif pur coûte ~+15-40 % selon
le corpus — le prix pour des metrics/tests/docstrings par fonction.

## Richesse ajoutée (ce que les agents peuvent maintenant interroger)

- `complexity`/`cognitive`/`loop_depth` : repérer les hotspots.
- `transitive_loop_depth` : la fonction au cœur des boucles imbriquées.
- `recursive` : récursion (vraie, pas juste auto-appel).
- `is_test` : filtrer tests vs prod.
- `docstring` : contexte sémantique immédiat.
- Branch + head_sha : savoir QUEL état du code est indexé.
- `change_count`/`last_modified` par File : les fichiers qui bougent.
- `FILE_CHANGES_WITH` : les fichiers qui co-évoluent (refactor impact).

## Non couvert (prochaines passes si besoin)
- SIMILAR_TO (similarité sémantique entre fonctions — coûteux).
- Nœuds Commit individuels (le C ne les crée pas non plus — seulement le
  couplage + temporalité).

## Optimisation de l'indexation (2026-09-07, `c2dbbf1f`)

Profil du kernel (70 821 fichiers, ~28 s utiles) : extraction+parse 16,1 s
(parallèle rayon, parse tree-sitter dominant) + fusion 5,9 s (upserts
1,55 M nœuds) + résolution 1,6 s + passes 2,6 s.

Gains appliqués (payer à l'écriture, comme les requêtes) :
- build_metric_columns mono-passage : 1,58 → 0,65 s (1 itération au lieu
  de 8, has_docstring dérivé en interne).
- tests_pass indexé : 1,17 → 0,44 s (itère les CALLS by_edge_kind au lieu
  des 4,8 M edges; is_test lu depuis la colonne).
- CodeGraph::with_capacity : pré-réserve les Vec (nb définitions connu).

Résultat : drivers/net 4,9 → 4,4 s (-10 %), obrain 1,4 → 1,2 s, kernel
31,8 → ~29 s. Le poste restant (extraction 16 s) est le parse tree-sitter
parallèle — le plancher structurel du travail.
