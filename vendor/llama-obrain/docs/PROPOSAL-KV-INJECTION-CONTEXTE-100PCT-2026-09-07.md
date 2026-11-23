# Proposition — injection KV ultra-rapide, contexte 100 % disponible (bypass total du texte)

**Date** : 2026-09-07 — objectif goal e9c190dd — fork `llama-obrain` (934ec56), engine obrain `main`.
**Statut** : PROPOSITION (design + DoD) — à valider avant implémentation.

## 1. Pourquoi (constats mesurés)
- `/jspace/inject_knowledge` fait **prefill du texte du fait à CHAQUE appel** (fact_decode ~1,0 s sur CPU 27B) puis round-trip de l'état (blob ~157 Mo/8 tokens) ; le texte reste « non visible » mais le coût de préfill demeure.
- Objectif : **by-passer totalement l'injection texte** — le contexte mémoire doit être **à 100 % disponible** (états présents en permanence) et l'**injection ultra-rapide** (copie d'état, pas de calcul).
- APIs prouvées présentes dans le fork et **sans appelant pipeline** (0 caller) : `llama_kv_cache::inject` / `inject_layer` / `extract_layer` (+ wrappers `llama_kv_cache_inject_layer`/`extract_layer` llama-context.cpp:4761-4791) — exactement la couture « prefill-bypass » documentée dans llama-kv-cache.h.

## 2. Architecture proposée

### 2.1 Hors ligne (une fois par insight)
1. Choisir un insight/nœud obrain (fact, doc, résumé) — requête obrain (`search_*`/`relief`/engrams).
2. Générer son état K/V **une seule fois** : prefill des tokens de l'insight sur un slot dédié puis `extract_layer` par couche → sérialiser `{k_data, v_data}` par couche (native type du cache) → **blob d'état**.
3. **Stocker le blob dans/à côté d'obrain** : clé = qn du nœud (propriété `kv_state_ref` + fichier binaire annexe, ou zone substrate) ; version = hash du contenu.

### 2.2 En ligne (par décodage)
1. Sur signal (surprise/doute, `compare_generation` calibré, ou directive) → CALL obrain → qn(s) cible(s).
2. Charger le/les blob(s) (local, ms) → `llama_kv_cache::inject` aux **positions mémoire réservées** (positions fixes hors tokens visibles), couche par couche (`inject_layer`).
3. **Masque d'attention custom** (`custom_attn_mask` + `n_head_groups`) : les tokens courants voient les positions mémoire (lecture) — contrôle fin des têtes qui y accèdent.
4. Décodage continue : **zéro prefill du fait, zéro token texte du fait dans le contexte visible**, le « contexte » mémoire est disponible en permanence et l'injection = copie mémoire → µs-ms.

### 2.3 Retour (boucle complète)
Captures/readout (GDN/post-attn/gram) pendant le décodage → événements vers obrain (CDC/engrammes) → futur `search_surprise`/`relief` — la boucle « l'inférence écrit dans le graphe, le graphe pilote l'inférence ».

## 3. DoD (8 cases)
1. Outil de build d'états (CLI ou route) : insight → blob stable (extract byte-identique entre runs) → stocké, clé = qn ; test de reproductibilité.
2. Route serveur d'injection d'état par ref (`POST /kv/inject_state {refs:[qn…], prompt, n_generate}` ou extension task) — pas de `fact_text`.
3. Masque mémoire : tokens courants → positions mémoire via custom mask (groupes de têtes configurables) ; test de contrôle (causalité des positions mémoire).
4. **Bench daté** : inject wall (µs/ms, memcpy) vs fact_decode actuel (1,0 s CPU) ; continuité de génération avec/sans état (même insight) ; coût blob compressé.
5. Boucle obrain : surprise → CALL → refs → injection → réponse ; tests côté adapter (payloads `{refs}`).
6. Suites fork (build) + aucune régression engine main (683/735/823/870) ; commits réversibles par étape.
7. Docs (cette proposition + README adapter) + graph.
8. GPU pour les chiffres temps réel (le CPU valide la mécanique uniquement).

## 4. Risques / décisions
- **Taille des blobs** (157 Mo/8 tok natifs) → KV quantisé (note : « TurboQuant KV types dropped in 2026-07 upstream merge ») et/ou compression — chantier dédié ; le format d'état doit coller au type de cache du slot.
- Positions mémoire réservées fixes vs slots dynamiques : simplicité d'abord (positions réservées contiguës, masque dédié).
- Multi-états simultanés : limites d'interférence constatées côté `inject_knowledge` (un fait/appel) → à re-caractériser pour l'injection directe (les états sont disjoints par positions → moins d'interférence attendue, à mesurer).
- GPU requis pour bench temps réel (CPU seul actuellement).

## 5. Étapes proposées (chantiers séparés)
- K1 : **build d'états + stockage obrain** (outil extract→blob, ref=qn, reproductibilité).
- K2 : **injection directe serveur** (route refs, masque mémoire, multi-têtes) + bench (µs vs ms, avec/sans état).
- K3 : **boucle fermée** obrain↔décodage (surprise→refs→inject ; captures→obrain) + tests.
- K4 : **format/quant KV** pour maîtriser la taille des blobs (échelle).
