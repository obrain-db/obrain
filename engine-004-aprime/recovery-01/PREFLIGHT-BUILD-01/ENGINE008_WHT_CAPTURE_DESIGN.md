# ENGINE008_WHT_CAPTURE_DESIGN

**Passe :** PREFLIGHT-BUILD-01 — 2026-10-02
**Statut :** `WHT_CAPTURE_METHOD=Q1_NATIVE_F1A_INSTRUMENTATION`
**`WHT_CAPTURE_IMPLEMENTED=NOT_IMPLEMENTED_THIS_PASS`** (voir statut)

---

## 1. Le constat qui débloque B2

Le site de décision (`src/llama-graph.cpp:1497`) **n'émet aucun log** — je l'avais
établi. Mais ce site est **déjà instrumenté** par l'instrumentation f1a qui fait
partie de la construction Q1 elle-même.

`src/f1a-capture.cpp:473` :

```cpp
r.rotation_applied = (cur_raw != cur_used);
```

La rotation n'est pas *déduite d'un message* : elle est **observée comme une
identité de tenseur**. `cur_raw_id` est l'activation telle qu'elle **arrive**,
`cur_eff_id` l'activation **effectivement consommée** par `build_lora_mm`. Si la
rotation a eu lieu, les deux diffèrent — et le registre le note par cible.

C'est exactement l'observation que §7 exige, et elle est prise **au moment de la
construction du graphe**, donc indépendante du mode d'exécution.

---

## 2. Mécanisme d'activation — natif, inerte par défaut

En-tête `src/f1a-capture.h:3` :
> « INERTE par défaut : `f1a_capture_get_ctx()` retourne nullptr tant que
> `OBRAIN_F1A_CAPTURE=1` n'est pas posé. Aucun dup, aucun registre, aucune E/S
> sur le chemin normal. »

Variables, lues dans `f1a-capture.cpp:13-15` :

| Variable | Rôle |
|---|---|
| `OBRAIN_F1A_CAPTURE` | active la capture (sinon `nullptr`, coût nul) |
| `OBRAIN_F1A_INVENTORY` | inventaire |
| `OBRAIN_F1A_OUTDIR` | répertoire de sortie |
| `OBRAIN_F1A_RUN_ID` | identifiant du run |

**Aucune modification du calcul n'est requise.** L'instrument est déjà compilé
dans `libllama.so.0.0.135` (SHA Q1 `87c42d5f…`). L'activer ne change pas le
chemin de calcul : il ajoute un registre scopé à une époque de graphe.

C'est **le même instrument** qui a produit `Q1_ROTATION_APPLIED_TRUE_COUNT=672`,
`FALSE=0`, `MISSING=0` dans `e7b-f1a-q1-forward-v1/on/WHT_RUNTIME_PROOF_AUDIT.txt`.

---

## 3. Surface de capture

### `EVENT_LEDGER.csv` — colonnes exactes (`f1a-capture.cpp:184`)

```
run_id, event_sequence_id, event_kind, decode_step, compute_submission_id,
graph_ptr, graph_fingerprint, capture_epoch_id, eligible_target_count,
captured_target_count
```

Types d'événements écrits dans les sources : `COMPUTE_SUBMIT`
(`f1a-capture.cpp:289`), `SYNC_COMPLETE` (`:573`).

### `GRAPH_CAPTURE_MANIFEST.csv` (`:579`) — par cible

```
run_id, epoch, target_id, capture_id, weight_name, family, expected_K,
rotation_applied, ...
```

### `GRAPH_ROLE_RELATIONSHIPS.csv` (`:671`) — relations structurelles des nœuds

---

## 4. Chaîne de preuve — les quatre maillons exigés par §7

| Maillon | Source d'observation |
|---|---|
| 1. décision de rotation | `rotation_applied = (cur_raw != cur_used)`, `f1a-capture.cpp:473` |
| 2. transformation / représentation obtenue | `cur_eff_id`, `eff_ne`, `eff_nb`, `eff_bytes` (activation **post-WHT**) |
| 3. appartenance au chemin de sortie | `weight_name` (projection linéaire identifiée) + `GRAPH_ROLE_RELATIONSHIPS.csv` |
| 4. exécution effectivement observée | `COMPUTE_SUBMIT` + `SYNC_COMPLETE` + `graph_fingerprint` |

Le maillon 4 est le plus faible et doit être **déclaré comme tel** : la
soumission et la synchronisation sont observées, **pas l'exécution du noyau**.
La conclusion devra être formulée en conséquence.

---

## 5. Propriété négative de la baseline — couverture explicite

Le point délicat : détecter une rotation *qui n'a pas lieu*.

`build_lora_mm` est, d'après le commentaire du fork, « le goulot unique de toutes
les projections linéaires de tous les modèles ». L'instrument enregistre une
cible **par appel**, avec `rotation_applied` faux ou vrai.

Pour la baseline, la couverture est donc **positive et mesurable** :
`eligible_target_count` et `captured_target_count` doivent être **non nuls**.
Si la baseline produit des cibles avec `rotation_applied=false` **partout**, la
propriété négative est **détectée**, pas simplement non observée.

**Critère d'échec à consigner** : `captured_target_count == 0` sur la baseline
signifierait une absence de couverture, donc `NOT_CONFIRMED` — jamais « absence
de rotation prouvée ». C'est la distinction que §7 impose explicitement.

---

## 6. Ce que l'observateur change — et ce qu'il ne change pas

- **Ne change pas** : le chemin de calcul, la décision de rotation, le graphe
  construit, les tenseurs consommés. L'instrument lit des identités de pointeurs
  et des métadonnées de tenseurs.
- **Change** : des E/S disque (`fopen`/`fprintf` par époque de graphe) et une
  consommation mémoire pour le registre.
- **À documenter par mesure** : ne pas désactiver CUDA graphs, ne pas changer
  `OBRAIN_NO_ROTATION`. Si l'instrument forçait une synchronisation, cela doit
  être **observé et déclaré**, car un chemin modifié n'est pas silencieusement
  équivalent (§7).

**Point ouvert à trancher à l'exécution** : la couverture de l'instrument sous
CUDA graph. `f1a_capture_get_ctx()` est scopé par époque de graphe, ce qui
suggère une capture à la construction — mais cela doit être **constaté**, pas
supposé.

---

## 7. Limites déclarées

1. Le maillon 4 observe **soumission + synchronisation**, pas l'exécution du
   noyau. Une conclusion d'exécution effective serait sur-interprétée.
2. La couverture de l'instrument sous CUDA graph reste à constater.
3. `rotation_applied` prouve ce que son emplacement permet : la substitution
   d'activation dans `build_lora_mm`. Il ne prouve pas la justesse numérique de
   la transformée — cela relève du §8 du présent contrat (WHT déjà qualifiée
   par ailleurs : 672/672).
4. Cet instrument est **celui de Q1** ; il ne remplace pas Q1 et ne le modifie pas.
