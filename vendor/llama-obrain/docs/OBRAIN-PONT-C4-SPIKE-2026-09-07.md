# Pont obrain↔llama — C4 : spike Qwen3.8-27B (2026-09-07)

## Environnement
- Machine : **CPU seul** (pas de nvidia-smi/GPU), 62 G RAM, ~56 G dispo.
- Modèle : `/home/triviere/models/Qwen3.8-27B-Q4_K_M.gguf` (18,97 Go), serveur fork `llama-obrain` snapshot `934ec56` (`build/bin/llama-server -c 2048 -t 8 --port 8099`).
- Débit mesuré : ~1,7 tok/s (~580-620 ms/token, CPU).

## Résultats de bout en bout (chronométrés)
### 1. `/jspace/inject_knowledge` (KV-splice, PO P5-T5) — VALIDÉ
- Body `{fact_text, prompt, n_generate}` ; réponse : `{text, fact_tokens, blob_bytes, per_token_ms, fact_decode_ms, query_decode_ms}`.
- Premier essai : fact « Paris est la capitale de la France. » (8 tokens) → **blob état extrait/ré-injecté = 157 419 212 octets** (~157 Mo pour 8 tokens — la couture du futur cache), fact_decode 1062 ms, query_decode 1649 ms, total 14,6 s (20 tokens générés). Texte généré cohérent (raisonnement `<think>`).

### 2. Boucle complète OBRAIN → injection — VALIDÉ (10,2 s)
- Store obrain engine `main` (`/tmp/infmem.obrain`) : création d'un nœud fait (`K {text: "Le Pacifique est le plus grand océan du monde."}`) → CDC.
- `adapter.py` : `search_surprise(0)` remonte le nœud (activity CDC réelle), `fact_from_node` extrait le texte → `POST /jspace/inject_knowledge` (12 fact_tokens, blob 157 681 436 octets) → réponse générée.
=> Le chemin « doute/surprise → requête obrain → fait → injection KV » est **mécaniquement prouvé** sur Qwen3.8.

### 3. `/jspace/compare_generation` (signal divergence) — NON concluant sur CPU
- Deux essais (n_generate 24 puis 4) : **timeout > 60 s puis > 300 s** (curl rc=28). Hypothèses : file de tâches serveur sérialisée (le 1er essai abandonné client-side a pu rester en exécution serveur) et/ou génération `<think>` longue ; à re-tester sur GPU et en inspectant le handler/task (server-context.cpp) pour comprendre la durée attendue.

## Bench daté (CPU, 27B Q4)
| Mesure | Valeur |
|---|---|
| tokens/s (decode) | ~1,7 (580-620 ms/tok) |
| inject_knowledge 8 tok (wall) | 14,6 s |
| boucle obrain→inject (wall) | 10,2 s |
| blob KV par fait | ~157 Mo (8-12 tokens) |
| fact_decode / query_decode | 1,06 s / 1,65 s |

## Notes / suite
- Le blob 157 Mo/fait impose la gestion d'un **cache d'états** (extract→réinject = la couture prévue) : à mesurer sur faits plus longs et sur GPU.
- GPU absent : les perfs CPU suffisent à valider la mécanique ; les benchs de production (latence tok/s, comparaisons) devront être rejoués sur GPU (DGX dispo ?) — à confirmer.
- C5 : seuil sur le signal `compare_generation` → déclenchement automatique obrain→inject (boucle fermée) + écriture des événements (captures/readout) vers obrain.
