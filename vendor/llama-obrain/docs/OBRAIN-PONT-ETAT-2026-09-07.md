# Pont obrain↔llama — ÉTAT (2026-09-07, goal e9c190dd)

**Reprise en 10 s** : lire ce fichier → `docs/OBRAIN-PONT-INVENTAIRE-2026-09-07.md` (API fork) →
`docs/PROPOSAL-KV-INJECTION-CONTEXTE-100PCT-2026-09-07.md` (architecture).

## Objectif (rappel)
Brancher obrain (engine `main` v0.0.1-m6-zorder) à l'inférence llama.cpp du fork `llama-obrain`
(snapshot `934ec56`), valide Qwen3.8-27B : **bypass total de l'injection texte** — états K/V
pré-calculés, contexte mémoire 100 % disponible, injection ultra-rapide.

## Livré & vérifié (commits fork)
- `c5ce965` Phase 0 (inventaire), `4e8e514` C1/C2 (contrats + obrain-db main/bridge RO),
  `801968a` C3 (adapter /jspace), `90c6f2f` C4 (spike Qwen3.8 inject texte), `52f4fc4` (refonte KV),
  `b24dc29` K1 baseline, `4823a89` K1/K2 code, `fa59db9` K1b disque, `a86145f` K1b cross-restart.
- Serveur (fork) étendu : task `JSPACE_INJECT_KNOWLEDGE` accepte désormais :
  - `{fact_text, extract_only:true}` → `text:"kv_state:<ref>"`, `fact_tokens`, `blob_bytes` ;
    écrit `kv-states/<ref>.bin` (header u32 tokens + payload) — ref FNV-1a déterministe (reproductible) ;
  - `{state_ref, prompt, n_generate, greedy}` → `llama_state_seq_set_data` puis décodage direct du
    prompt (zéro prefill du fait, zéro token texte visible) ; fallback disque si absent du cache.

## Mesures (Qwen3.8-27B Q4, CPU, --parallel 3)
| fait 11-12 tok | avant (texte) | après (état) |
|---|---|---|
| fact → état | 1 288-1 356 ms (prefill répété) | **96-99 ms** (cache) / **505 ms** (disque après reboot) |
| wall/appel (n=4) | 4,8-5,3 s | 3,2-3,6 s |
| extract | — | ~1,7 s, ref stable ×2 ; fichier 157 615 892 o |

## API du pipeline (adapter `obrain-adapter/`)
- `extract_state(text)` / `inject_state(ref, prompt)` (payloads conformes) ; `record_kv_state_ref(qn, ref)`
  (mapping obrain) ; `ObrainBridgeRO` (CALLs LECTURE search_*) ; selftest vert (sans serveur).

## Restant (chantiers, dans l'ordre)
- **K2b (b)** : contrôle fin par **masque d'attention custom multi-têtes** (`custom_attn_mask` +
  `n_head_groups`, llama-graph.h) pour l'accès aux positions mémoire — le causal-dans-seq actuel rend
  déjà le fait visible depuis le prompt (mesuré) ; raffinement = choix des têtes/positions.
- **K3 (c)** : boucle fermée — `compare_generation` calibré (timeout CPU : à re-tester GPU) →
  CALL obrain → inject ; captures readout/GDN/gram → événements/engrammes obrain.
- **K4** : **quant KV** pour maîtriser les blobs (~157 Mo natif/état ; note : TurboQuant KV types
  « dropped in the 2026-07 upstream merge » — à re-caractériser).
- **K1c** : persistance « côté obrain » complète (fichiers états référencés/déplacés avec le store).

## Décisions ouvertes (utilisateur)
1. **Machine GPU** (DGX `llama-obrain-dgx` ?) pour benchs temps réel & calibration compare ;
2. Ordre de priorité K2b vs K3 vs K4 si GPU obtenu.

## Décisions & directives utilisateur (round 11, 07-09) — À EXÉCUTER
1. **Validation CPU obligatoire** (pas GPU-only) : chaque étape vérifiée en mécanique CPU, métriques datées.
2. **RÉVISION STOCKAGE** : ne PAS stocker les blobs KV sur disque (157 Mo natif/état = coût) — l'état est
   **RECONSTRUIT à la demande depuis la base obrain** (contenu/graphe = source de vérité) et maintenu en RAM
   (slot réservé) ; le fichier disque (K1b) devient un cache optionnel.
3. **Métriques à rapporter** : **TFPT**, **tokens/s**, **cohérence de réponse** (questions factuelles du corpus).
4. **Validation fonctionnelle sur base de code RÉELLE** : noyau Linux complet NON dispo local (seulement
   linux-headers) → cible = plus grosse base dispo (aletheia 6,1 G / deepseek-harness 1,7 G / llama-obrain 1,2 G) ;
   indexer (native import) puis questions complexes alimentées par search_*/relief/compare + injection.
   (Un vrai noyau Linux nécessiterait un téléchargement kernel.org — possible si demandé.)
5. Goal recadré rev5 ; K2b (masque, DoD posé) → K3 (compare_generation calibré CPU : réduire n_generate,
   greedy, tolérer long) → K4 (quant KV) après les directives 1-4.

## Base de validation réelle (précision utilisateur round 13)
- **Source Linux complète : `/tmp/linux-6.6`** (construite depuis GitHub, déjà utilisée pour les benchs
  moteur — `banc-obrain/docs/bench/linux-kernel-2026-09-07.md` et `-m6`). AUCUN téléchargement requis.
- K-valid : indexer `/tmp/linux-6.6` en store obrain (native import, engine `main` v0.0.1) → questions
  complexes (architecture, flux, cross-file) alimentées par search_*/relief/compare + injection par état
  reconstruit à la demande ; métriques TFPT/tok-s/cohérence datées (CPU).

## K-valid — construction du store Linux (constat round 14)
- Aucun store obrain/CBM Linux **persistant** trouvé (bench `tmp-linux-6.6-drivers-net` était éphémère ;
  code-all3 absent des `target/bench/real`). La base CBM persistée existe pour les projets listés sous
  `~/.local/share/codebase-memory-mcp/versions/*/cache/home-triviere-projects-*.db` (pas de Linux).
- Recette pour K-valid (à exécuter) :
  1. Réindexer `/tmp/linux-6.6/drivers/net` (ou racine) via l'ingesteur CBM/codebase-memory (comme les
     benchs) → DB projet ; OU réutiliser un gros projet existant (deepseek-harness.db…) comme substitut.
  2. Convertir la DB en store obrain substrate :
     `node scripts/code-index-to-substrate.mjs <out.obrain> <cbm-db>` (binding NAPI `/tmp/rtprobe.node`
     à rebâtir sur engine `main` si périmé) ou `native-import` si corpus binaire disponible.
  3. Harness K-valid : adapter → insights (fonctions + textes) → reconstruction état à la demande →
     injection par ref (RAM) → questions complexes ; métriques TFPT/tok-s/cohérence (CPU).
- Choix utilisateur requis : drivers/net (~5 400 fichiers / ~100 k fonctions) vs racine (~70-81 k fichiers).

## Chantier persistance (décision utilisateur 07-09 : branche dédiée, PAS sur main)
- Banc-obrain : branche `feat/llama-obrain-index-persist` tirée de `main` (2c6c5bab, main laissée
  propre) ; proposition `docs/PROPOSAL-LLAMA-OBRAIN-INDEX-PERSIST-2026-09-07.md` (design+DoD 8) commitée
  `434ccb2f`. Implémentation : exemple/binaire `obrain-indexer persist <repo> <out.obrain>` → store
  substrate (mapping nœuds/arêtes/qn repo.path.name, props texte) → bridge RO. Cible drivers/net par défaut.

## Persist implémenté (banc-obrain feat/llama-obrain-index-persist)
- `obrain-indexer/examples/persist.rs` : M6 → store substrate (engine main). Validé e1000
  (250 Function, text docstring/signature présent, RO count ok). Cible : drivers/net complet.

## BOUT-EN-BOUT VALIDÉ (round, Linux git → obrain → llama) — CPU
- Store : `/tmp/linux-git` (Linux 7.3-rc2, git) drivers/net → `/tmp/kv-linux-driversnet.obrain`
  (117 043 Function / 310 547 nœuds / 769 685 arêtes ; index 5,5 s + persist 2,6 s ; couche git Project/Branch).
- Fait réel (docstring e1000_probe, 93 tok) lu depuis obrain → extract (état 163 Mo en RAM, ref 83ec191618a1dbea)
  → inject par ref : state_load 96,9 ms, **TFPT 619 ms**, **tokens/s 1,69** (CPU), réponse générée.
- Conforme directives : validation CPU, reconstruction d'état à la demande (RAM, pas de blob disque),
  TFPT/tok-s rapportés, base réelle Linux git. Cohérence détaillée = suite (questions multiples).

## VALIDATION DIRECTE multi-faits (suite « cohérence détaillée ») — voir VALIDATION-LINUX-KV-2026-09-07.md
- 4 fonctions Linux réelles lues DEPUIS le store (`/tmp/kv-linux-driversnet.obrain`, git 7.3-rc2) →
  extract (état RAM, 17-19 s CPU) → inject par ref : state-load 97-140 ms, TFPT 0,57-0,94 s,
  1,07-1,70 tok/s, 4/4 réponses (rôles exacts) ; réutilisation RAM 112-128 ms.
- Cohérence ciblée (n=140) : réponses déductibles du seul code injecté — rappel marqueurs 0,40-0,56
  (séquences/quirks/IRQ exacts) sur 3/4 ; `m_can_start_xmit` : artefact slot (0 token au rechargement du ref).
- Harness rejouables : `obrain-adapter/e2e_linux_kv.py` + `e2e_linux_kv_targeted.py`.
