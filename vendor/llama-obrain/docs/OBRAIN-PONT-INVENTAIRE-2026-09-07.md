# Pont obrain ↔ llama.cpp — Inventaire Phase 0 (2026-09-07)

**Fork** : `llama-obrain` @ `934ec56` (snapshot sécurisé « avant poursuite »), branche `master`, dépôt séparé (aucun remote).
**Build** : `build/bin/llama-server` présent (+ `libllama-server-impl.so`) ; `llama-server --version` → « 0 (unknown) » (snapshot sans tag git) ; un `jspace-server.log` non suivi prouve que le serveur `/jspace` a déjà été lancé.
**Cible modèle** : Qwen3.8-27B (GGUF) — noyaux `qwen35.cpp` / `qwen35moe.cpp` présents dans `src/models/`.
**Objectif** : brancher obrain (engine `main` v0.0.1-m6-zorder) à l'inférence : injection KV directe + bypass prefill, multi-têtes/masques d'attention custom, boucle doute/surprise → requête obrain → injection ; obrain-db du pipeline mis sur la version `main`.

---

## 1. Contrats d'API confirmés (lecture du code)

### 1.1 Injection / extraction KV-cache directe (bypass prefill) — `src/llama-kv-cache.h`
- `inject(...)` (l.~346) : « Direct KV cache injection — write pre-computed K/V data without llama_decode » — signature `[n_tokens][n_embd_k_gqa] / [n_embd_v_gqa]`.
- `inject_layer(seq_id, pos, k_data, v_data, n_tokens, layer_idx)` (l.~355) : injection par couche sur allocation de cellule existante.
- `extract_layer(...)` (l.~369) : miroir (lecture par couche aux positions) — doc explicite : « to demonstrate **prefill-bypass** via inject of a previously-extracted K/V state into a fresh seq ».
→ Mécanisme (a) du goal : **présent et utilisable** (extract sur une seq → inject sur une seq neuve → décodage sans re-prefill).

### 1.2 Masque d'attention custom multi-têtes — `src/llama-graph.h` / `src/llama-context.cpp`
- `custom_attn_mask` (+ `custom_attn_mask_pos`, `custom_attn_mask_n_pos`, `custom_attn_mask_n_head_groups`, `custom_attn_mask_sorted_pos`, `hier_mask`) dans `llm_graph_context` (llama-graph.h ~l.315-332) ; câblés au build du graphe (llama-graph.cpp:496) et alimentés depuis le contexte (`llama-context.cpp:2902-2906`, accesseurs `get_attn_mask*`).
→ Mécanisme (b) : **présent** (masques par positions + groupes de têtes) ; reste à exposer le réglage depuis le serveur/adaptateur obrain.

### 1.3 Surface serveur `[obrain]` (routes HTTP) — `tools/server/server.cpp`
- `GET  /jspace/gram` — `POST /jspace/readout_forward` — `POST /jspace/compare_generation` — `POST /jspace/inject_knowledge` — marqués « J-space geometric analysis [obrain] — PO plan 4749fb72 (cognitive-canvas) » ; « Custom attention mask [obrain] » à proximité.
- Tâches internes `[obrain]` : `SERVER_TASK_TYPE_JSPACE_COMPARE_GENERATION`, `SERVER_TASK_TYPE_JSPACE_INJECT_KNOWLEDGE`, `SERVER_TASK_TYPE_SLOT_COMPACT` (LONG-RESUME, compaction phantom-prefix).
→ Mécanisme (c) : le serveur expose déjà les points d'entrée HTTP de comparaison de génération et d'injection de connaissance ; c'est là que l'adaptateur obrain se branchera (client → /jspace/…).

### 1.4 Capture / steering multi-couches `[obrain]` (86 marqueurs, 16 fichiers)
- `set_layer_output_capture / get_layer_output / layer_output_n_layers` (llama-context.{h,cpp}) ; override multi-couches « geometry-of-meaning » par `t_layer_override_inp` rempli par l'appelant (llama-context.cpp ~1746) ; capture GDN « HC-SPLICE-01 étape 2.5 » ; post-attn-norm capture [B1a] ; hooks modèle dans `qwen35.cpp`/`qwen35moe.cpp`/`qwen3.cpp`/`delta-net-base.cpp`.
→ C'est le canal de **steering** complémentaire (si l'injection se fait aussi au niveau des activations, pas seulement du KV).

## 2. Écart confirmé (l'autre moitié du pont)
- **Le fork ne référence aucun store obrain** (grep `OBRAIN_STORE|obrain-db|.obrain` : vide) → l'obrain-db « utilisé par le pipeline » n'existe pas encore côté fork ; il reste à :
  1. définir le store cible (voir §4, décision requise) et le mettre à jour sur l'engine `main` (binaire `v0.0.1-m6-zorder`, portes 683/735/823/870 vertes) ;
  2. construire l'adaptateur : états obrain (engrammes/fenêtres/search_surprise/relief) → (KV extrait/injecté, masques, override multi-couches) ; captures (readout/compare_generation) → événements/engrammes côté obrain.

## 3. État build / rejouabilité
- `llama-server` compilé (build/bin) ; les hooks `[obrain]` sont dans le source compilé de cette build (version « 0 (unknown) » = snapshot) — à confirmer par un run de smoke (sans modèle : `--help` OK) avant toute modification.
- `git status` : propre, seul `jspace-server.log` non suivi (artefact de run précédent — à gitignorer ou supprimer).

## 4. Décisions requises avant Phase 1 (spike bout-en-bout Qwen3.8)
1. **Store obrain cible** : (A) un store dédié « mémoire inférence » créé sur engine `main` (recommandé — propre, isolé) ; (B) réutiliser un store runtime existant (dev/prod, copie) ; choix à valider.
2. **Modèle Qwen3.8-27B** : provenance du GGUF + machine (GPU local vs DGX `llama-obrain-dgx`) — dépendance externe à fournir/confirmer avant le spike.
3. **Ordre** : (a) d'abord « obrain-db main » + adaptateur minimal de lecture (CALLs accessibles depuis le process serveur via bridge/CLI RO) ; (b) spike injection KV bypass prefill sur Qwen3.8 ; (c) boucle surprise.

## 5. Prochaines étapes proposées (chantiers séparés, DoD 8 cases avant exécution)
- **C1 — Inventaire diff `[obrain]` complet & re-base** (parcourir le diff du snapshot vs llama.cpp amont, lister chaque hook : contrat, entrées/sorties, backend GPU OK) → doc `OBRAIN-PONT-DIFF-*.md`.
- **C2 — obrain-db `main` + bridge RO** : artefacts engine main (CLI/binding), petit store de référence, vérifier CALLs LECTURE depuis un client externe (search_delta/surprise/relief/engrams/window).
- **C3 — Adaptateur minimal** : module dans le fork (ou client externe) qui appelle `/jspace/inject_knowledge` & `/jspace/compare_generation` avec du contenu obrain, + smoke sur un petit modèle pour valider la boucle mécanique (pas encore Qwen3.8).
- **C4 — Spike Qwen3.8-27B** : injection KV directe (extract→inject, bypass prefill) alimentée par le store ; bench datés (prefill évité vs re-prefill, latence decode).
- **C5 — Boucle surprise/incertitude** : détection (readout/compare_generation, entropie logits) → CALL `search_surprise`/`relief` → injection corrective (KV ou override) ; tests + bench.
