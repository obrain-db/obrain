# ENGINE-007B-F1A — CLÔTURE DU DÉFAUT 56 (durée de vie des captures)

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../qualification/e7b-f1a-capture-defect56-closeout/`

---

## 0 — Résultat

**Clôture matérialisée. Le défaut est fermé, et le détecteur est prouvé discriminant.**

```
ENGINE007B_F1A_CAPTURE_STATUS=QUALIFIED_POST_FIX
HARNESS_DEFECT_56_STATUS=RESOLVED_VERIFIED_BY_TARGETED_PRESERVATION_AND_BITWISE_TRANSPARENCY

POST_FIX_ZERO_INPUT_NONZERO_OUTPUT_CONTRADICTION_COUNT=0
CAPTURE_CONTENT_NEGATIVE_CONTROL=PASS
```

## 1 — Vérification préalable des 672 payloads corrigés (§2)

```
MISSING_PAYLOAD_COUNT=0
SHA256_MISMATCH_COUNT=0
DIMENSION_MISMATCH_COUNT=0
NONFINITE_PAYLOAD_COUNT=0
```

**Les tailles réelles ont été lues, pas supposées :**

| K | N | paires | rôle |
|---|---|---|---|
| 5120 | 17408 | 192 | `ffn_up`, `ffn_gate` |
| 5120 | 6144 | 72 | `attn_gate` |
| 6144 | 5120 | 24 | `attn_output` (famille E6) |
| 5120 | 12288 | 24 | — |
| 5120 | 1024 | 24 | `attn_q` / `attn_k` (8 têtes × 128) |

Mon premier contrôle de dimension **échouait à tort** : j'avais codé en dur une liste de valeurs
attendues, et `N=1024` / `N=12288` n'y figuraient pas. Je l'ai remplacé par un contrôle de
**cohérence avec `K_logical` de l'inventaire** — factuel au lieu d'heuristique.

## 2 — Inventaires (§3, §4)

```
EPOCH0_TARGET_CONTENT_INVENTORY.csv   168 lignes
EPOCH1_TARGET_CONTENT_INVENTORY.csv   168 lignes
```

23 colonnes par cible : identifiants, chemin, SHA256, nombre d'éléments, compte de zéros, compte de
non-nuls, `amax`, comptes NaN et Inf pour l'activation **et** la sortie de base, plus le verdict de
contradiction.

## 3 — Comparaison des époques (§5)

```
COMMON_WEIGHT_NAME_COUNT=168
UNMATCHED_EPOCH0_WEIGHT_COUNT=0
UNMATCHED_EPOCH1_WEIGHT_COUNT=0
```

Appariement **par nom de poids**, pas par `TARGET_ID`. Les valeurs diffèrent entre époques — ce sont
deux calculs différents, ce n'était pas l'attendu.

## 4 — Table des contradictions (§6)

```
ZERO_INPUT_NONZERO_OUTPUT_CONTRADICTIONS.csv   header present, 0 ligne de donnees
POST_FIX_ZERO_INPUT_NONZERO_OUTPUT_CONTRADICTION_COUNT=0
```

## 5 — Contrôle négatif : le seul artefact vraiment nouveau (§7)

Le détecteur testé est **exactement** celui des inventaires :

```python
activation_all_zero == true AND base_output_any_nonzero == true
```

| cas | attendu | observé |
|---|---|---|
| `NEGATIVE_CASE_1` — activation nulle, sortie non nulle | **détecté** | **détecté** |
| `NEGATIVE_CASE_2` — activation nulle, sortie nulle | non | non |
| `NEGATIVE_CASE_3` — activation non nulle, sortie non nulle | non | non |
| `NEGATIVE_CASE_4` — NaN | signalé par finitude, **pas** contradiction | signalé=OUI, contradiction=NON |

```
EXPECTED_CONTRADICTION_COUNT=1
OBSERVED_CONTRADICTION_COUNT=1
FALSE_POSITIVE_COUNT=0
FALSE_NEGATIVE_COUNT=0
CAPTURE_CONTENT_NEGATIVE_CONTROL=PASS
```

Les payloads synthétiques sont dans un dossier séparé (`negative_control_payloads/`). **Aucun
payload original n'a été modifié.**

**C'est la preuve qui manquait** : sans elle, « 0 contradiction » pouvait aussi bien signifier « le
détecteur ne détecte rien ». Maintenant on sait qu'il détecte le cas 1 et pas les autres.

## 6 — Trace source, positions relues (§8)

J'ai **relu** les positions dans le commit qualifié avant de les citer :

```c
// ggml/src/ggml-alloc.c:690   [VERIFIE]
static void ggml_gallocr_free_node(ggml_gallocr_t galloc, struct ggml_tensor * node) {
    // graph outputs are never freed
    if (node->flags & GGML_TENSOR_FLAG_OUTPUT) {
        AT_PRINTF("not freeing output %s\n", node->name);
        return;

// ggml/src/ggml-backend.cpp:1342 et :1361   [VERIFIE]
ggml_set_output(tensor_copy); // prevent ggml-alloc from overwriting the tensor
```

Les SHA256 des trois fichiers du correctif, du binaire diagnostique et du modèle sont archivés.

## 7 — Statut de la causalité (§9)

```
ALLOCATOR_LIFETIME_MECHANISM=PROVEN
   regle lue dans la source (ggml-alloc.c:690), relue dans le commit qualifie
PRESERVATION_FIX_EFFECT=PROVEN
   correctif applique, 25 contradictions -> 0, 336 captures changees
BITWISE_MODEL_TRANSPARENCY_AFTER_FIX=PROVEN
   logits OFF/ON identiques au bit pres, re-mesure sur le binaire corrige
EXACT_OVERWRITING_WRITER_FOR_EACH_CAPTURE=NOT_IDENTIFIED
```

**Je ne déclare pas `EXACT_WRITER_IDENTIFIED=YES`** : aucune cartographie mémoire n'a été faite.
Votre §9 dit que cette absence **ne rouvre pas** le défaut 56 — c'est aussi ma lecture.

## 8 — Portée historique (§10)

```
PRE_FIX_ZERO_INPUT_NONZERO_OUTPUT_CONTRADICTIONS=25
POST_FIX_ZERO_INPUT_NONZERO_OUTPUT_CONTRADICTIONS=0
PRE_VS_POST_ACTIVATION_CONTENT_CHANGED=336/336
ALL_PRE_FIX_CAPTURE_CONTENT=NOT_SCIENTIFICALLY_QUALIFIED
```

**Je ne déduis aucune propriété bitwise précise de chaque ancien payload** du seul fait qu'il diffère
du nouveau. Ce qui est établi : le mécanisme de l'allocateur rendait la libération **possible**, et
les 336 contenus ont effectivement changé après correctif.

## 9 — Rapport final (§14)

```
ENGINE007B_F1A_CAPTURE_STATUS=QUALIFIED_POST_FIX
HARNESS_DEFECT_56=CAPTURE_DUP_TENSORS_NOT_MARKED_AS_OUTPUT_SO_ALLOCATOR_FREES_THEIR_STORAGE
HARNESS_DEFECT_56_STATUS=RESOLVED_VERIFIED_BY_TARGETED_PRESERVATION_AND_BITWISE_TRANSPARENCY
CORRECTED_CAPTURE_SOURCE=qualification/e7b-f1a-q0-n3nonies/capon/
EPOCH0_TARGET_COUNT=168
EPOCH1_TARGET_COUNT=168
COMMON_WEIGHT_NAME_COUNT=168
MISSING_PAYLOAD_COUNT=0
SHA256_MISMATCH_COUNT=0
DIMENSION_MISMATCH_COUNT=0
NONFINITE_PAYLOAD_COUNT=0
POST_FIX_ZERO_INPUT_NONZERO_OUTPUT_CONTRADICTION_COUNT=0
CAPTURE_CONTENT_NEGATIVE_CONTROL=PASS
ALLOCATOR_LIFETIME_MECHANISM=PROVEN
PRESERVATION_FIX_EFFECT=PROVEN
BITWISE_MODEL_TRANSPARENCY_AFTER_FIX=PROVEN
EXACT_OVERWRITING_WRITER_FOR_EACH_CAPTURE=NOT_IDENTIFIED
PRE_PRESERVATION_CAPTURE_SET=INVALID_FOR_SCIENTIFIC_REFERENCE
EPOCH1_NATIVE_REFERENCE_CONTROL=PASS
INDEPENDENT_ROW_OUTPUT_INDEXING_TEST=PASS
Q0_FORWARD_PROTOCOL_VERSION=v1
Q0_FORWARD_STATUS=NOT_EXECUTED
Q1_FORWARD_ALLOWED=NO
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
ENGINE007_NEXT_STEP=Q0FWD_V1_FREEZE_TRAJECTORY_THEN_EXECUTE
```

## 10 — Livrables réellement créés

```
EPOCH0_TARGET_CONTENT_INVENTORY.csv           168 lignes, 23 colonnes
EPOCH1_TARGET_CONTENT_INVENTORY.csv           168 lignes
EPOCH0_EPOCH1_TARGET_COMPARISON.csv           168 noms apparies
ZERO_INPUT_NONZERO_OUTPUT_CONTRADICTIONS.csv  header seul, 0 donnee
CAPTURE_CONTENT_NEGATIVE_CONTROL.txt          PASS, 4 cas
CAPTURE_BUFFER_LIFETIME_SOURCE_TRACE.txt      positions relues et verifiees
EPOCH1_NATIVE_REFERENCE_CONTROL.txt           indexe N5, ne le recalcule pas
DEFECT56_SOURCE_PROVENANCE.txt                SHA256 sources + binaire + modele
DEFECT56_DATA_PROVENANCE.txt                  populations, pre-check, tailles reelles
DEFECT56_STATUS.txt                           synthese et ledger
negative_control_payloads/                    4 paires synthetiques, isolees
```

**Non créés, volontairement (§11)** : `POST_COMPUTE_CONTENT_SNAPSHOTS.csv`,
`END_EPOCH_CONTENT_SNAPSHOTS.csv` (instrumentation runtime devenue inutile),
`CAPTURE_BUFFER_REGION_MANIFEST.csv` (exigerait la cartographie mémoire, hors gate Q0FWD-v1).

## 11 — Retour au gate prospectif

Le closeout est fait. Le vrai gate reste :

```
Q0FWD_TOKEN_TRAJECTORY   <- non fige
puis run OFF/ON Q0FWD-v1
```

`Q0_FORWARD_STATUS=NOT_EXECUTED`, `Q1_FORWARD_ALLOWED=NO`. Aucune instrumentation GPU lancée dans
cette passe, le protocole Q0FWD-v1 n'a pas été touché.
