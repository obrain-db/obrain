# Pont obrain↔llama — C2 : obrain-db sur version `main` + bridge RO (2026-09-07)

## Artefacts engine `main`
- Binaire : `banc-obrain/target/release/obrain` buildé depuis l'arbre `main` (= HEAD `adc11a68`, code identique) — `obrain --version` → `0.0.1`. Portes moteur vertes : engine 683/735, substrate 823, core 870 (rejouées le 07-09).
- Store dédié pipeline : `/tmp/infmem.obrain` (créé `obrain init`) — chemin provisoire ; décision de localisation finale requise (§ décisions).

## Bridge RO externe — vérifié (processus séparé)
```
printf '{"tool":"query",...}' | obrain bridge --store <store>
```
- `MATCH (n) RETURN count(*)` → 0 (OK).
- `CALL obrain.search_delta(0)` → status ok, 0 rows (store vide).
- `CALL obrain.search_surprise(0,5,false)` → status ok, 0 rows.
- `CALL obrain.search_by_role(1,3)` → erreur explicite (nœud hors registre — comportement attendu, cf. tests engine).
- `obrain.procedures()` → 28 entrées, **sans** les `search_*` (dispatch LECTURE dédié, cf. C1) — l'adaptateur utilisera les noms exacts.

=> Le canal « client externe → obrain engine main en lecture seule » fonctionne : c'est la voie que l'adaptateur d'inférence empruntera (process `obrain bridge` RO persistant par pipeline, requêtes CALL par événement surprise/doute).

## Étapes suivantes (C3+)
- C3 : petit module adaptateur (dans le fork ou client) qui, sur un signal « compare_generation », appelle `search_delta/surprise/relief` sur le store puis POST `/jspace/inject_knowledge` — smoke sur petit modèle.
- Décisions en attente : (1) store cible définitif (dédié « mémoire inférence » recommandé vs copie runtime) ; (2) GGUF Qwen3.8-27B + machine.
