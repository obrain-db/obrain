# obrain-adapter — client du pont obrain ↔ llama-server (/jspace)

**Chantier C3 (2026-09-07)** — adapter minimal : sur signal de génération, interroger
obrain (engine `main` v0.0.1, bridge RO) puis injecter de la connaissance dans le
serveur llama.cpp du fork via `/jspace/inject_knowledge` ; détecter le doute/surprise
via `/jspace/compare_generation`.

> Statut : **client de validation (Python, PoC/harness)**. L'adaptateur de production
> sera **C++ dans llama-server** (côté fork) — la logique obrain reste 100 % Rust
> (bridge RO + CALLs LECTURE) ; le Python n'est que du collage de validation.

## Schémas /jspace (lus dans `tools/server/server-context.cpp`)
- `GET /jspace/gram` → `{n_embd, dtype:"f32", layout:"row_major", gram_base64}` (une fois, coûteux).
- `POST /jspace/readout_forward` body `{"h":[n_embd floats]}` → `{n_vocab, logits…}` (tête de lecture sur un vecteur résiduel h).
- `POST /jspace/compare_generation` body `{"prompt":str, "scale"?:0.15, "n_generate"?:24}` → résultat asynchrone (comparaison de générations — signal de divergence/surprise).
- `POST /jspace/inject_knowledge` body `{"fact_text":str, "prompt":str, "n_generate"?:24}` — **KV-splice** (PO plan 4749fb72, P5-T5) : le fait est traité UNE fois sur le slot réservé, son état (attention+recurrent) extrait puis ré-injecté (la couture d'un futur cache), puis `prompt` est décodé APRÈS le fait — les tokens du fait ne sont PAS dans le contexte visible. **Limites par conception** : UN fait par appel (interférence multi-faits) ; réservé à la connaissance NOUVELLE/complémentaire (pas pour contester un fait établi).

## Boucle visée (C5, validée mécaniquement ici)
1. Sur génération en cours → `compare_generation(prompt)` ; si divergence/surprise (seuil à régler),
2. obrain : `obrain.search_surprise(since_ms)` / `search_delta` / `relief` → top entités → textes/facts candidats,
3. `inject_knowledge(fact_text = candidat, prompt = contexte courant)` pour réorienter,
4. captures/readout (étapes C4/C5) → événements vers obrain (engrammes/CDC).

## Fichiers
- `adapter.py` — `ObrainBridgeRO` (spawn `obrain bridge --store …`), sélecteurs de candidats, builders de payloads, POST HTTP (urllib, optionnel).
- `selftest.py` — vérifie : payloads conformes aux schémas, bridge RO fonctionnel (store dédié), sélecteurs déterministes sur petit graphe. Aucun serveur llama requis.
- Lancement : `python3 obrain-adapter/selftest.py [OBRAIN_BIN] [STORE]`
