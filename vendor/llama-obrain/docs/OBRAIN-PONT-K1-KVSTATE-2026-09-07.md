# Pont obrain↔llama — K1 : build d'états K/V + baseline (2026-09-07)

## Constats (lecture du task P5-T5, server-context.cpp ~3040-3235)
- Le mécanisme existe déjà dans `/jspace/inject_knowledge` : fact décodé sur le slot réservé
  (`seq_id = jspace_reserved_slot_id()`, exige `--parallel >= 3`), puis prompt décodé APRÈS sur le même
  seq (le KV du fait reste résident pendant l'appel) ; le round-trip extract→réinject (blob) a été
  **retiré comme pur overhead** (2026-07-13) — la note de code dit explicitement que le **cache
  d'états cross-requêtes (extract une fois, réutiliser sur plusieurs prompts) n'est pas implémenté** :
  c'est exactement K1/K2.
- APIs de la couture : `llama_state_seq_get_data / llama_state_seq_set_data` (blob GPU/CPU ~65-70 Mo
  commenté, 157 Mo mesurés ici pour 12 tokens attention+recurrent) ; par couche : `extract_layer` /
  `inject_layer` (0 caller pipeline aujourd'hui).

## Baseline datée (CPU 27B Q4, --parallel 3, même fait 12 tokens, 2 prompts différents)
| appel | fact_tokens | blob_bytes | fact_decode_ms | query_decode_ms | wall |
|---|---|---|---|---|---|
| prompt A | 12 | 157 681 444 | **1287,9** | 1270,9 | 4,81 s |
| prompt B | 12 | 157 681 444 | **1356,1** | 1548,5 | 5,25 s |

=> **~1,3-1,4 s de prefill du fait répétés à CHAQUE appel** (CPU) : le gain K1 = ne le payer qu'une
fois (état persisté), puis injection = copie (set_data) µs-ms.

## Plan d'implémentation K1/K2 (routes serveur, dans le fork)
1. `POST /kv/extract_state {text}` (slot réservé) : tokenize+decode du texte (fact) → `llama_state_seq_get_data(seq)`
   → blob écrit sur disque (store dir `--obrain-states` ou défaut) → réponse `{ref: sha256, bytes, tokens, n_layers, dims}`.
   Reproductibilité : même texte → même sha (extract déterministe, seed fixe hors sampling).
2. `POST /kv/inject_state {ref, prompt, n_generate}` : lecture blob → `llama_state_seq_set_data(seq)` (après
   `llama_memory_seq_rm`) → decode(prompt) sur le même seq → génération (boucle sampling existante) →
   réponse `{text, inject_ms, per_token_ms}`. **Zéro prefill du fait.**
3. Stockage « côté obrain » : l'adapter écrit `{qn → ref}` + métadonnées dans le store obrain (propriété
   `kv_state_ref` du nœud) — K1b ; le disque local du serveur reste le cache d'états (réf par sha).
4. Bench K2 : inject_ms vs fact_decode_ms (~1,3 s baseline) ; continuité de réponse avec/sans texte visible.

## DoD (K1)
1. extract d'un texte → blob fichier + ref sha ; re-extract → **sha identique** (test reproductibilité).
2. Taille/dims documentés ; gestion `--parallel < 3` (erreur explicite, comme le task actuel).
3. Tests : 2 textes distincts → 2 refs ≠ ; même texte → même ref ; blob non vide ; token count exact.
4. Bench daté (extract wall ; taille blob) dans ce doc/ledger.
5. Aucune régression (build fork vert ; moteur main intact — worktree séparé).
6. Commit réversible ; docs + graph.

## K2 — implémentation & bench (round 6, 07-09) — VALIDÉ
Code : champs `state_ref`/`extract_only` sur le task JSPACE_INJECT_KNOWLEDGE ; statiques `g_kv_states`
(cache process, ref FNV-1a du blob) ; route `/jspace/inject_knowledge` accepte `{extract_only}` (→
répond `kv_state:<ref>`) et `{state_ref, prompt}` (→ `llama_state_seq_set_data` puis décodage du prompt,
zéro prefill du fait). Serveur recompilé.

Bench (Qwen3.8-27B Q4, CPU, --parallel 3, greedy, n_generate=4) :
| étape | fact→état | wall |
|---|---|---|
| extract (2x, reproductibilité) | ref `eed87f05a1aeb5c8` / 11 tok / 157 615 888 o | 1,68 / 1,75 s |
| baseline text path (fait 12 tok) | fact_decode 1 288-1 356 ms | 4,81-5,25 s |
| **inject par ref** (2 prompts) | chargement état **96-99 ms** (fact_decode_ms) | 3,15-3,53 s |

=> injection par état : fact→contexte **~0,10 s au lieu de ~1,3 s (×13)** ; wall −1,3 à −1,7 s/appel.
Notes : cache process (persistance disque/obrain = K1b) ; 157 Mo natif par état → K4 (quant KV).

## K1b — persistance disque + ref côté obrain (round 7, 07-09)
- États écrits sur disque (`kv-states/<ref>.bin`, header u32 tokens + payload) en plus du cache
  mémoire ; fallback disque à l'injection (survit au restart). Vérifié : extract → fichier
  `eed87f05a1aeb5c8.bin` (157 615 892 o), même ref que la version mémoire (déterministe).
- Adapter : `record_kv_state_ref(bin, store, qn, ref)` → SET `n.kv_state_ref` sur le nœud obrain
  (CLI RW) ; selftest vérifie l'aller-retour (mini store). Mapping qn→ref = K1b côté obrain.
- (b) masque mémoire multi-têtes : le décodage sur la même seq rend les positions du fait
  causalement visibles depuis le prompt (déjà mesuré) ; le raffinement par `custom_attn_mask` +
  `n_head_groups` est disponible (llama-graph.h) — chantier K2b.

## K1b — cross-restart (round 8, 07-09) — VALIDÉ
Après redémarrage complet du serveur (état absent du cache mémoire), `{state_ref:
eed87f05a1aeb5c8, prompt}` répond depuis le disque : fact_tokens 11, blob 157 615 888 o,
chargement+set_data 505 ms (lecture disque + copie), wall 3,62 s (4 tok CPU).
=> les états persistent entre sessions : contexte mémoire 100 % disponible, inject sans
re-prefill même après restart.
