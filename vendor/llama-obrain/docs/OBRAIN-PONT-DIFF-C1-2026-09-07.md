# Pont obrain↔llama — C1 : contrats des hooks [obrain] (2026-09-07)

Source : headers publics (`llama-context.h`, `llama-graph.h`) + points d'ancrage cpp. V1 (contrats), à compléter pendant C3.

## Capture (lecture de l'état interne par couche, par token)
- **Sortie de couche** : `set_layer_output_capture(layer_indices, n)` / `get_layer_output(layer, token)` / `layer_output_n_layers()` — buffers `[n_tokens × n_embd]` (copies anti-recyclage GPU, `t_layer_out` au build, llama-graph.cpp:501).
- **Post-attn-norm [B1a]** : `set_attn_norm_capture(layers)` / `get_attn_norm(layer, i)` — capture `h_normed` (entrée de W_Q/W_K/W_V).
- **GDN (DeltaNet) [HC-SPLICE-01 étape 2.5]** : `set_gdn_input_capture(layers)` / `get_gdn_captured(layer, what∈{0=k,1=v,2=g,3=beta}, dims_out[3]={ne0, ne1=têtes, n_tokens_total})` — coefficients de transition récurrents par token (modèle delta-net) ; accumulateurs `gdn_data/gdn_dims`, tensors `t_gdn_in`.

## Steering (injection dans le flux, pendant un forward)
- **Override mono/multi-couche « geometry-of-meaning »** : `set_layer_output_override(layer, data, n_floats)` (mono, usage « finite-difference probe ») et variante multi-couches simultanées (`override_layer_indices` trié + `override_layer_data_by_layer`), câblée en entrées de graphe `t_layer_override_inp` (remplies après allocation, avant compute) — le résidu de chaque couche remplacée devient l'entrée de la suivante.
- Modèles couverts : hooks de capture dans `qwen35.cpp:204`, `qwen35moe.cpp:189/240/253` (override), `qwen3.cpp:140`, `models/llama.cpp:226`, `delta-net-base.cpp:435` (GDN).

## Interfaces serveur (`[obrain]`, PO plan 4749fb72)
- Routes : `GET /jspace/gram` · `POST /jspace/readout_forward` · `POST /jspace/compare_generation` · `POST /jspace/inject_knowledge` (server.cpp:263-269) ; helpers `tools/server/server-context.cpp:44+` ; réservation de slot jspace (server-context.cpp:2871, 3044 tâche P5-T5 « production »).

## Ce que ça implique pour l'adaptateur obrain (design note)
- L'appelant (futur adaptateur côté obrain engine `main`) doit : (1) connaître la topologie par token/couche (`n_embd`, n_tokens du batch) pour produire `data` ; (2) fournir K/V déjà projetés pour `inject_layer` (c.-à-d. produire du KV hors llama_decode — ex. à partir d'un pré-calcul obrain : encodage mémoire) ; (3) consommer les captures (sorties/attn-norm/GDN) comme **mesures réelles d'activation** → boucle surprise (C5).
- Les `search_*` obrain ne sont PAS listés par `obrain.procedures()` (dispatch dédié LECTURE) : l'adaptateur doit cibler les noms exacts (`obrain.search_delta/surprise/by_role/conflicts/causes/effects`, `obrain.relief`), pas les découvrir par catalogue.
