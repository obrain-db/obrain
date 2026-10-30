# ENGINE008_MTP_CAPTURE_DESIGN

**Passe :** PREFLIGHT-BUILD-01 — 2026-10-02
**Statut :** `MTP_CAPTURE_METHOD=Q1_NATIVE_F1A_GRAPH_SUBMISSION_CAPTURE`
**`MTP_CAPTURE_IMPLEMENTED=NOT_IMPLEMENTED_THIS_PASS`**

---

## 1. Mécanisme retenu — le même instrument Q1

`void f1a_mark_compute_submission(struct ggml_cgraph * gf, int submission_rc)`
(`f1a-capture.h:69`) reçoit **le graphe effectivement soumis** et en produit :

- `event_kind=COMPUTE_SUBMIT` dans `EVENT_LEDGER.csv`
- `graph_ptr` et **`graph_fingerprint`** — l'empreinte du graphe soumis
- `capture_epoch_id`, `eligible_target_count`, `captured_target_count`
- `GRAPH_ROLE_RELATIONSHIPS.csv` — relations structurelles des nœuds du graphe soumis

C'est une observation **au niveau du graphe soumis**, pas une recherche de nom
dans des logs. C'est précisément ce que §8 exige.

---

## 2. Signatures MTP/NextN dans les sources Q1

NextN est un concept de premier ordre dans le fork :

| Élément | Emplacement |
|---|---|
| `t_h_nextn` (tenseur d'état NextN du graphe) | `src/llama-graph.cpp:1271`, `:1317-1318` |
| `n_layer_nextn` (hyperparamètre) | `src/llama-graph.cpp:1407` |
| `LLM_TENSOR_NEXTN_EH_PROJ`, `..._SHARED_HEAD_HEAD` | `src/llama-model.cpp:1476-1480`, `:1541-1545` |
| tenseurs du modèle candidat | `blk.64.nextn.{eh_proj,enorm,hnorm,shared_head_norm}` (4 tenseurs, métadonnée mesurée) |

Le harnais ne doit **pas** reconnaître MTP par un nom de log : il doit établir
si le graphe soumis **construit et consomme** ces éléments.

---

## 3. Les six niveaux à distinguer (§8)

| Niveau | Comment l'établir | Statut possible en preflight |
|---|---|---|
| 1. tenseurs **présents** | métadonnée GGUF (`blk.64.nextn.*`, 4 tenseurs) | déjà mesuré : OUI pour le candidat, NON pour la baseline |
| 2. tenseurs **chargés** | inventaire de chargement (`OBRAIN_F1A_INVENTORY`) | observable |
| 3. opérations **construites** | graphe soumis : présence des nœuds dérivés de `t_h_nextn` | observable |
| 4. opérations **appartenant au graphe pertinent** | `graph_fingerprint` du graphe soumis au decode standard | observable |
| 5. opérations **effectivement exécutées** | `COMPUTE_SUBMIT` + `SYNC_COMPLETE` pour ce fingerprint | **partiel** : soumission observée, exécution noyau non |
| 6. **décodage spéculatif** | absence de `llama_set_embeddings`/draft, type de contexte, config | observable côté API harnais |

`RUNTIME_CONFIRMED` ne pourra être déclaré que sur les niveaux effectivement
couverts. Une absence de signal sur un niveau non couvert reste
`NOT_CONFIRMED` — jamais convertie en zéro (§9).

---

## 4. Conditions autorisant une conclusion d'absence

Une conclusion « MTP/NextN non exécuté dans le chemin standard » exige
**cumulativement** :

1. le modèle a été chargé complètement et le graphe standard soumis au moins une fois ;
2. `graph_fingerprint` du graphe soumis est **stable** entre deux runs ;
3. `GRAPH_ROLE_RELATIONSHIPS.csv` de ce graphe **ne contient aucun** nœud dérivé de `t_h_nextn` ;
4. le nombre de soumissions correspond aux appels `llama_decode` réellement comptés ;
5. la couverture est **non nulle** (des nœuds ont bien été capturés) — sinon, absence d'observabilité.

Si la condition 5 échoue : `NOT_CONFIRMED`. Si la 3 échoue : `MTP_EXECUTION_PRESENT`.

---

## 5. Tests synthétiques du collecteur (§8, §9)

Le classificateur doit être validé **avant** tout run, sur des graphes
synthétiques construits en mémoire (aucun forward) :

- **cas positif** : graphe contenant un nœud dérivé de `t_h_nextn` → classé `NEXTN_PRESENT` ;
- **cas négatif** : graphe standard sans NextN → classé `NEXTN_ABSENT` ;
- **cas incomplet** : capture vide / fingerprint absent → `NOT_CONFIRMED`, jamais `ABSENT`.

Ces tests valideront **le code de capture**. Ils ne constituent **pas** une
preuve runtime sur les modèles (`RUNTIME_CAPTURE_STATUS=METHOD_IMPLEMENTED_NOT_EXERCISED`).

---

## 6. Défaut identifié et à couvrir

Le commentaire de `f1a_mark_compute_submission` mentionne `submission_rc` : le
code de retour de la soumission. Un `rc != 0` doit être **compté et rapporté**
comme échec de soumission, et non assimilé à une exécution réussie.

## 7. Limites déclarées

1. L'exécution **noyau** n'est pas observée ; seules la soumission et la
   synchronisation le sont.
2. La couverture sous CUDA graph reste à constater pour cet instrument aussi.
3. Une couverture nulle doit produire `NOT_CONFIRMED`, et **interdire** de
   poursuivre les étapes qui dépendent de cette preuve (§1 de l'instruction).
