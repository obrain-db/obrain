# Plan de tissage — périmètre consolidé (multi-writer + contrat bridge)

**Date** : 2026-09-06 — branche `feat/zorder` — HEAD `15fb56a0` (worktree propre).
**Rôle** : la carte UNIQUE du périmètre courant — à relire en premier après toute
perte de contexte (protocole L0 de `docs/RECONSTRUCTION-AGENT-2026-09-05.md`).
Elle tisse : le contexte utilisateur (obrain = vraie base cognitive), les
chantiers livrés, les invariants gelés, les découvertes, les liens vers le code
et les docs, et les prochaines actions. **Tenir à jour à chaque chantier.**

---

## 1. État réel en 3 lignes (le moment zéro)

1. **État** : obrain `feat/zorder`, HEAD `15fb56a0` propre — le goal
   « multi-writer à scale linéaire » (8 rounds) est **complete** ; le chantier
   « double journalisation WAL des propriétés » (option C) a été **livré par un
   agent et vérifié par bench indépendant** ce jour. Contrat bridge
   `runtime_search`/`search_graph` toujours **déployé** (sha `151e0f65`).
2. **Vérification livraison option C (2026-09-06, benchs indépendants A/B sur
   le parent `945feb65`, même sonde des 2 côtés)** : suites vertes (substrate
   **810** = 808+2 tests preuve kill, engine 683, core 870, invariants 9/9,
   I12 vert) ; gains mesurés — prop neuve 7,11 → 6,38 µs (−10 %), prop chaude
   3,75 → 2,67 µs (−29 %), CREATE GQL mono 13,87 → 12,20 µs (−12 %), @8 ~296k
   (au plafond, pas de régression). Preuve de non-perte : 2 tests kill sans
   flush (WAL à 0 record inline → état identique après réouverture), verts.
3. **Prochaine action** : rien d'engagé — prochain chantier au choix (volet
   M6 code→graph natif, ou approfondissement moteur GQL documenté en
   « pistes écartées » du plan).

## 2. Contexte utilisateur (décisions actées — verbatim résumé)

| Décision | Contenu |
|---|---|
| « obrain ne doit pas se limiter à son cas d'usage » | Une vraie base : N writers concurrents sur le même graphe, propriété du moteur (pas une topologie de contournement). |
| « tout doit être obrain » | Graphe de code, sessions, mémoire agent : même moteur, même catalogue, identité commune `qualified_name`. |
| « l'export code→graph doit être un module d'obrain » | Chantier M6 : l'indexeur de code devient natif substrate (remplace le rôle d'indexeur de codebase-memory). |
| « obrain doit être une db cognitive » | Best case + archi fixe : boucle code ↔ sessions ↔ mémoire agent (énergie, engrams, formes de recherche) — pas un GDB générique. |

## 3. Chantiers livrés (avec ancrages)

| Chantier | Commits | Artefacts | Statut |
|---|---|---|---|
| **RFC multi-writer** | `8e67cfe7` → enrichie `3a6dd361` | `docs/rfc/substrate/multi-writer.md` (~750 l, toutes les mesures) | Livré — constats mesurés, décisions, design v3 documenté |
| **Fix contrat bridge (brain_search)** | `91a86b70` (code), `abd70153` (docs), `aeb8638c` (bascule) | `crates/obrain-cli/src/mcp_server.rs` — `prop_str`, `runtime_search`, `search_graph` | **Déployé** prod+dev (sha `151e0f65`), brain_search vert |
| **B1 — scope par graphe (faux conflits éliminés)** | `b8933a48` | `graph_scope()` + write/read sets scopés + invariant I11 | Livré — faux conflits inter-graphes **75 % → 0 %** (prérequis du scale) |
| **I12 — filet import bulk** | `92d6b029` | `multi_writer_invariants.rs` `import_bulk_400k_1600k_non_regresse` | Livré — 400k/1,6M en ~0,25 s, testé à chaque `cargo test` |
| **Cache-parse GQL** | `f183550d` | `session/mod.rs` `execute_optimized_plan` | Livré — plan optimisé cherché AVANT le parse : +5,5 % mono, +5 % @8 |
| **THP hors zones props v2** | `a60b45d7` | `file.rs` `est_zone_props_v2_nom` | Livré — prop page neuve **8,09 → 7,1 µs (−12 %)** (Levier 2) |
| **Double journalisation WAL props (option C)** | `8d710b99` (+ RFC `945feb65`, revue `ed8e5bd5`, ledger `2cd35082`/`15fb56a0`) | `store.rs` gating `PropSetInline`/`PropRemoveInline` (± arêtes) à `!v2_a_pris` + 2 tests preuve kill | **Livré par agent, vérifié** — substrate 810/810 ; prop neuve −10 à −17 %, chaude −29 à −61 %, CREATE mono −12 % ; preuve non-perte (kill sans flush) verte |

## 4. Invariants gelés (RFC §4) — la matrice vivante

| # | Invariant | Test | Fichier |
|---|---|---|---|
| I1 | Verrou exclusif refusé si part partagée vit (process) | unit | `file.rs` |
| I2 | Deux ouvertures RO partagent la part | unit | `file.rs` |
| I3 | shared-write + RO coexistent ; le RO voit les commits | `shared_write_puis_lecture_ro_voit_les_commits` | engine `multi_writer_invariants.rs` |
| I4 | 2ᵉ writer exclusif refusé pendant shared-write | `second_writer_exclusif_refuse_pendant_shared_write` | idem |
| I5 | Append WAL multithread : ensemble LSN 1..=N, zéro perte | `append_multithread_lsn_contigus_zero_perte` + `threads_independants_substrate_zero_perte` | substrate `multi_writer_wal.rs` + engine |
| I6 | Rejeu RO à l'ouverture : idempotent, zéro perte | `reouverture_ro_idempotente_apres_rejeu` | engine |
| I7 | Write-write conflict sur store substrate persistant | `conflit_write_write_sur_store_substrate` | engine |
| I8 | `TransactionWriteTracker` → `WriteConflict` remonté | 3 tests unitaires | `write_tracker.rs` |
| I9 | ParallelExecutor peut driver de vraies écritures substrate | `parallel_executor_batch_reel_substrate` | engine |
| I10 | Lecture RO pendant writer actif : pas de blocage | `lecture_ro_pendant_writer_thread_sans_blocage` + WAL reader concurrent | engine + substrate |
| I11 | PAS de faux conflit inter-graphes (scope B1) | `deux_graphes_disjoints_sans_faux_conflit` | engine (ajouté `b8933a48`) |
| I12 | Import bulk 400k/1,6M non régressé (repère ~0,22-0,26 s) | `import_bulk_400k_1600k_non_regresse` | engine (ajouté `92d6b029`) |

## 5. Découvertes du tissage (à ne pas reperdre)

1. **`open_shared_write` = `LOCK_SH`** : l'unicité du writer est une CONVENTION
   appelant (`OBRAIN_WRITE=1`), pas une garantie du verrou — deux writers
   partagés ne se refusent pas (RFC §1.1, I4).
2. **Ordre physique WAL ≠ ordre LSN sous appends concurrents** : le LSN est
   réservé par `fetch_add` AVANT le mutex d'écriture → l'invariant garanti est
   ENSEMBLISTE (1..=N sans trou), pas un ordre. À trancher pour Phase A/B2
   (RFC §4.1).
3. **Contrat bridge = contrat canal cbm (M4)** : depuis `CBM_BRIDGE_BIN=obrain`,
   chaque endpoint du bridge obrain doit imiter EXACTEMENT le format
   codebase-memory qu'il remplace. Le bug brain_search (lossless JSON) venait
   d'un contrat de champ (`qn`/`rank`/`text` vs `id`/`score`/`snippet`), pas
   d'un `rank` non-fini — le fix `jsonRank` du 05/09 était insuffisant.
4. **`ParallelExecutor` existe mais n'est branché nulle part** (mort) — levier
   écarté par la mesure (Phase A rejetée, RFC §3.1bis).
5. **Plafond matériel scale = ×4,7** (8 cœurs physiques + turbo mono) : le
   substrate multi-graphe scale ×6,1 (dépasse le SELECT), le GQL B1 ×4,0-4,5 =
   95 % du plafond. Le scale linéaire « parfait ×8 » est physiquement
   impossible sur cette machine — mesuré, pas supposé.
6. **Les faux conflits inter-graphes étaient LE blocage B1** : write_set non
   scopé → deux graphes écrivant NodeId(1) chacun = 75 % d'échec. Le scope par
   graphe (I11) a tout débloqué.
7. **Le write lock du registry de clés est pris à chaque internement** même si
   la clé existe (2×/prop) — mesuré SANS gain à corriger (props B1 ×5,7
   identique avec fast-path → reverté).

## 6. Liens graphe (brain_search retrouve ces artefacts)

Le tissage est vérifié par la recherche : `brain_search "multi-writer"`,
`"contrat bridge"`, `"I5 ordre LSN"` doivent remonter les messages/sessions de
ce périmètre (weave + engrams). Le graphe de code (`obrain-engine` projet)
contient les 3 fichiers d'artefacts ci-dessus (vérifié par `graph_query`).

## 7. Prochaines actions (ordre recommandé)

1. **Double journalisation WAL des propriétés — LIVRÉ** (`8d710b99`, option C :
   `PropSetInline`/`PropRemoveInline` ± arêtes conditionnels à `v2_a_pris`).
   Vérifié indépendamment (benchs A/B sur parent, preuve kill verte). Chantier
   clos — voir §3.
2. **Prochain chantier au choix de l'utilisateur** : volet M6 code→graph natif
   (indexeur de code en module obrain), ou approfondissement moteur GQL
   (documenté en « pistes écartées » §5 — faible retour attendu).
3. Tenir ce document + le ledger (`ledger-ameliorations-2026-09-04.md`) à jour
   à chaque chantier.
3. Pistes écartées après mesure (ne pas ré-explorer) : refactor
   manager-par-graphe (échecs rounds 2-4), group commit fsync (0 %), ownership
   map (−58 % mono), cache de plans shardé/Arc (workload réel = cache-miss),
   requêtes paramétrées (changement d'API), internement fast-path (0 gain).

## 8. Rappels opérateur / déploiement

- Binaire partagé : `~/.local/share/obrain-harness/bin/obrain` (sha actuel
  `151e0f65`), backup `obrain.bak-brainfix-20260906`, manifeste
  `etc/MANIFEST-hermes-v0.0.1.txt` (+ `.bak-pre-brainfix-20260906`).
- Rollback 1 cmd : mv retour du backup + restauration du manifeste.
- Règle repo : pas de déploiement prod sans demande ; R6 (restart dev = user).
