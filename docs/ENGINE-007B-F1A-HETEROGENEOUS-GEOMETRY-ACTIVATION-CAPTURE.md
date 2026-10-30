# ENGINE-007B-F1A — GÉOMÉTRIE HÉTÉROGÈNE 5/6 ÉPISODES ET QUALIFICATION DE LA CAPTURE

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a/`

---

## 0 — Résultat

**Les gates de géométrie et d'inventaire passent intégralement. Le gate de capture ne passe pas.**

```
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
```

| gate du §41 | état |
|---|---|
| `AUTHORITATIVE_FULL_MODEL_IDENTIFIED` | **YES** |
| `STANDALONE_GGUF_READER_CROSS_VALIDATED` | **YES** — 866 tenseurs, 0 écart |
| `E5_GEOMETRY_SOURCE_PROVEN` | **YES** |
| `E6_GEOMETRY_SOURCE_PROVEN` | **YES** |
| `ELIGIBLE_TENSOR_INVENTORY_FROZEN` | **YES** — 156 E5 + 12 E6 |
| `TARGET_MATMUL_OPERAND_MAPPING_PROVEN` | **NON** — voir §7 |
| `READ_ONLY_ACTIVATION_CAPTURE_HOOK` | **NON QUALIFIÉ** — voir §8 |
| `CAPTURED_ACTIVATION_K_MATCH_WEIGHT_K` | non mesuré |
| `CAPTURE_COMPLETENESS_GATE` | non mesuré |
| `CAPTURED_ACTIVATION_NAN/INF_COUNT` | non mesuré |
| `EPISODE_REFERENCE_RECONSTRUCTS_FULL_OPERATOR` | non mesuré |

**Aucune donnée de saillance n'a été produite** (§32 respecté). Le §42 prévoit exactement cet état :
`BLOCKED_ACTIVATION_CAPTURE_GATE`.

---

## 1 — Freeze de F1 (§1)

```
ENGINE007B_F1_STATUS=BLOCKED_PREMISE_GEOMETRY_MISMATCH_AND_MISSING_ACTIVATION_HARNESS
EPISODE_ALIGNED_STRUCTURED_ELIMINATION=NOT_DETERMINED
   -> aucun verdict de qualite du paragraphe 41 ancien n est selectionne.
```

## 2 — Modèle autoritatif (§2)

```
MODEL_PATH=/home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf
MODEL_SHA256=1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937
MODEL_BYTES=12844231424          FULL_MODEL_TENSOR_COUNT=866
GGUF_VERSION=3                   n_kv=47
BLOCK_COUNT=65   EMBEDDING_DIM=5120   FFN_DIM=17408   general.architecture=qwen35

REPARTITION DES TYPES : F32=360  Q4_K=176  TQ2_G128=168  BF16=96  Q5_K=65  Q6_K=1

NOT_THE_MODEL=mtp-Qwen3.8-27B-TQ2G128.gguf IS_THE_MTP_NEXTN_HEAD_ONLY
   (799 Mo, 18 tenseurs, blk.64 seul — ecarte)

Q4N_MODEL_FAMILY=Qwen3.8   Q4N_MODEL_VARIANT=27B
Q4N_LLAMA_CPP_ARCH_ENUM=LLM_ARCH_QWEN35   Q4N_LLAMA_CPP_ARCH_ID=qwen35
LLAMA_CPP_ARCH_ID_IS_MODEL_VERSION=NO     ACTIVE_MODEL_IS_QWEN3_8_27B=YES
QWEN3_5_USED_FOR_EXECUTION=NO
```

## 3 — Validation croisée du lecteur GGUF (§3) : **PASS**

Le §3 demande de ne pas se fier à mon lecteur autonome pour le type 43, hors de sa table standard.
J'ai donc écrit un second lecteur qui passe par **le chemin de code du fork obrain lui-même** :

```
f1a-gguf-crosscheck.c
   gguf_init_from_file(no_alloc=true)   -> libggml-base.so du fork
   gguf_get_n_tensors / _name / _type / _offset
   + relecture independante de la table de dimensions
   + ggml_type_name / ggml_type_size / ggml_blck_size du fork
```

Résultat :

```
lecteur libggml + table : 866 tenseurs
lecteur python autonome : 866 tenseurs
ensembles de noms identiques : OUI
ecarts type=0   dimensions=0   offset=0
MISMATCH_LIBGGML (desaccord interne entre libggml et la table) = 0 sur 866

STANDALONE_GGUF_READER_CROSS_VALIDATED=YES
```

Les deux chemins confirment indépendamment : `type 43 = TQ2_G128`, `ggml_size=34` octets par bloc,
`ggml_blck_size=128`. L'inventaire est fiable.

## 4 — Géométrie prouvée par la source (§4, §5)

```
vecdotq.cuh:114   VDR_TQ2_G128_Q8_1_MMVQ = 1
mmvq.cu:42        get_vdr_mmvq(TQ2_G128) = 1        (qi = 128/32 = 4)
mmvq.cu:518-519   blocks_per_iter = 1*4*32/4 = 32 ; blocks_per_row_x = ncols_x/128
mmvq.cu:606       kbx = tid/4 + 32*i
mmvq.cu:732       bq = (block_tq2_g128 *) vbq + kbx      <- indexation LINEAIRE

EPISODE_GEOMETRY_SOURCE_PROVEN=YES
EPISODE_LOGICAL_WEIGHT_GRANULARITY=1024_CONTIGUOUS_WEIGHTS
   aucun stride, padding ni packing intermediaire
```

## 5 — Les deux familles de géométrie (§6) : **confirmées sur le modèle réel**

### E5 — K=5120, **5 épisodes** (famille dominante)

```
kbx = 40 ; episodes = 40/8 = 5
  E0 = kbx  0.. 7  = K [   0 , 1024)      E3 = kbx 24..31 = K [3072 , 4096)
  E1 = kbx  8..15  = K [1024 , 2048)      E4 = kbx 32..39 = K [4096 , 5120)
  E2 = kbx 16..23  = K [2048 , 3072)

  i=0 : kbx 0..31  -> 128 threads actifs
  i=1 : kbx 32..39 -> tid/4 < 8 -> tid < 32 -> warp 0 SEUL

  E0 -> warp 0    E1 -> warp 1    E2 -> warp 2    E3 -> warp 3    E4 -> warp 0
  EPISODES_PER_PHYSICAL_WARP={2,1,1,1}      ACTIVE_EPISODES=5

  BASELINE=KEEP5   ECHELLE=KEEP5/KEEP4/KEEP3/KEEP2
  MASQUE=bits 0..4, plage 0x00..0x1f
  CARDINAUX C(5,4)=5  C(5,3)=10  C(5,2)=10
```

### E6 — K=6144, **6 épisodes** (secondaire)

```
kbx = 48 ; episodes = 6
  E0..E5 = kbx 8g..8g+7  = K [1024g , 1024g+1024)
  E0->w0  E1->w1  E2->w2  E3->w3  E4->w0  E5->w1
  EPISODES_PER_PHYSICAL_WARP={2,2,1,1}      ACTIVE_EPISODES=6

  BASELINE=KEEP6   ECHELLE=KEEP6/KEEP5/KEEP4/KEEP3
  MASQUE=bits 0..5, plage 0x00..0x3f
  CARDINAUX C(6,5)=6  C(6,4)=15  C(6,3)=20
```

## 6 — Inventaire gelé (§7, §8, §12)

```
FAMILLE   RÔLE              n    K      N        LIGNES calculees
E5        ffn_up            48   5120   17408     835 584
E5        ffn_gate          48   5120   17408     835 584
E5        attn_gate         36   5120    6144     221 184
E5        attn_q            12   5120   12288     147 456
E5        attn_k            12   5120    1024      12 288
E6        attn_output       12   6144    5120      61 440

TOTAL_E5_TENSORS=156      TOTAL_E6_TENSORS=12
ELIGIBLE_ROW_COUNT=2113536   (valeur mesuree, cf. TQ2_TENSOR_INVENTORY.csv)
E5_ROW_COUNT=2007040         E6_ROW_COUNT=61440
PRIMARY_F1_GEOMETRY=E5    (~95,0 % des lignes eligibles : 2 007 040 / 2 113 536)
SECONDARY_F1_GEOMETRY=E6
BLOCS_CONCERNES=48 (8..55)
```

*Note* : la somme du tableau par rôle (2 052 096) diffère de `E5_ROW_COUNT` (2 007 040) parce que
mon comptage par rôle multiplie `N` par le nombre de tenseurs du rôle sans vérifier que tous les
blocs du rôle portent le même `N` ni le même nombre d'occurrences. La valeur **faisant autorité** est
celle du CSV, calculée tenseur par tenseur : `E5_ROW_COUNT=2 007 040`. Je signale l'écart plutôt que
de le lisser.

Rôles **non éligibles**, confirmés sur l'artefact courant (§8) :

```
ffn_down  -> Q4_K       (PAS TQ2_G128)
attn_v    -> inexistant ; la valeur passe par attn_qkv fusionne, en Q5_K
token_embd, output -> Q4_K / Q6_K
```

```
SALIENCY_MEASURED=NO      QUALITY_ABLATION_MEASURED=NO
```

## 7 — Ce que j'ai établi sur le mapping des opérandes, et ce qui manque (§14)

Le §14 demande de **prouver** quel opérande est le poids et lequel est l'activation, et d'interdire
de présumer `src0`/`src1`. Voici ce que la lecture de source donne :

```
llama-graph.cpp:1476   ggml_tensor * llm_graph_context::build_lora_mm(
llama-graph.cpp:1487     // commentaire source : « build_lora_mm est le goulot unique de
                         //  toutes les projections lineaires de tous les ... »
llama-graph.cpp:1518     ggml_tensor * res = ggml_mul_mat(ctx0, w, cur);
```

Donc, pour tout matmul linéaire du modèle :
`src0 = w` (le tenseur de poids quantifié), `src1 = cur` (l'activation courante).

**Mais ce n'est pas une preuve au sens du §14**, qui exige de l'établir **sur le nœud de graphe
réellement exécuté**, avec les identités runtime des tenseurs :

```
TARGET_MATMUL_OPERAND_MAPPING_PROVEN=NO
   -> identifie par LECTURE DE SOURCE, pas encore par inspection du noeud de graphe
      en execution. Il manque : le nom du tenseur de poids vu par le nœud, l'identite
      du tenseur d activation, sa forme et son dtype au moment de l appel.
   -> c est un travail de la meme capture que le paragraphe 13, et je ne le declara pas
      acquis sans la mesure.
```

## 8 — Ce qui bloque le gate de capture (§17 à §30)

Le §18 demande d'inspecter le dépôt pour un mécanisme d'observation **existant** et de le préférer
à un patch des couches du modèle. Ce que j'ai trouvé :

```
1. ggml/src/ggml-cuda/ggml-cuda-observe.cuh : ABSENT de l arbre courant
   -> le harnais d observation ENGINE-004 est une GRIFFE DECLAREE dans mes notes de travail,
      pas un fichier present sur disque. Je ne peux donc pas m appuyer sur lui.

2. Aucun callback de graphe in-tree :
   grep cb_eval / ggml_callback / eval_cb dans ggml.h et ggml.c -> AUCUN RESULTAT
   -> il n existe pas de point d accroche d evaluation pret a l emploi.

3. Point d injection candidat identifie, mais NON IMPLEMENTE :
   llama-graph.cpp:1476 build_lora_mm   -> goulot unique de TOUTES les projections lineaires,
   avec ggml_mul_mat(ctx0, w, cur) a la ligne 1518.
   C est l endroit le moins invasif : un seul site, conditionne par le nom du tenseur
   de poids et son type TQ2_G128, sans toucher mmvq.cu ni aucun noyau.
```

**Je n'ai pas écrit ce patch dans cette passe.** Le §23 demande qu'une première capture soit
minuscule (1 séquence, 8-16 tokens, sous-ensemble représentatif) ; je préfère livrer ce patch avec
son mode de qualification plutôt que de le bâcler en fin de passe et de produire une capture dont
je ne pourrais pas garantir la complétude (§24) ni la cohérence de forme (§25).

```
READ_ONLY_ACTIVATION_CAPTURE_HOOK=NOT_QUALIFIED
CAPTURE_MANIFEST_EMPTY=YES
CAPTURED_ACTIVATION_NAN_COUNT=NOT_MEASURED
CAPTURED_ACTIVATION_INF_COUNT=NOT_MEASURED
EXPECTED_TARGET_INVOCATIONS=NOT_COMPUTED
OBSERVED_TARGET_INVOCATIONS=0
ACTIVATION_CAPTURE_RUN_PERFORMANCE_DATA=INVALID_NOT_MEASURED   (declare, paragraphe 19)
```

## 9 — Contrôle de la référence opérateur (§29, §30)

Le §29 exige une implémentation de référence exacte de `TQ2_G128 x Q8_1` **avant** toute saillance.
Je n'en ai pas construit, faute de la matière première qu'elle doit valider (les activations).
Je note seulement que les éléments nécessaires sont identifiés et disponibles :

```
qtype 43, block_tq2_g128 = { uint8_t qs[32] ; ggml_half d }  (34 octets, blck=128)
decode ternaire : 2 bits par poids, 4 codes -> {-1, 0, +1, +1} (a confirmer sur le fork)
q8_1 : block_q8_1 = { uint32_t ds ; int8_t qs[32] ; ggml_half d ; ggml_half s }
   -> la reconstruction q8_1 depuis l activation logique est faisable hors GPU,
      ce qui est la strategie preferee du paragraphe 28 (etape 2).
EPISODE_REFERENCE_RECONSTRUCTS_FULL_OPERATOR=NOT_MEASURED
EPISODE_REFERENCE_MODE=NOT_BUILT
```

## 10 — Aucune mutation (§17, §38, §39)

```
PRODUCT_KERNEL_MODIFIED=NO     SHA256 mmvq.cu=2bf9a27028207120f476ab543264f0f5ec40ceceb4058989efcf1a84fdeee037
                               SHA256 vecdotq.cuh=a8ddd2223d81b20fcc09ba461a659890f70ee7e808fc149d21e1d50bdf3e50d8
MODEL_MODIFIED=NO              GGUF_OVERWRITTEN=NO
F1A_CAPTURE.patch=NON_PRODUIT  (aucune modification de source dans cette passe, donc aucun patch a declarer)
```

Le §40 est respecté : l'absence de hook de capture est une **lacune de capacité**, déclarée comme
telle et non comptée comme `PLUMBING_FAILURE_29`.

```
PLUMBING_FAILURE_COUNT=28      PLUMBING_FAILURE_29=NOT_ASSIGNED
```

## 11 — Livrables

Produits dans `qualification/e7b-f1a/` :

```
FULL_MODEL_PROVENANCE.txt      modele, sha, comptages, type erroné ecarte
GGUF_READER_CROSSCHECK.txt     866 tenseurs lus par le chemin libggml du fork (869 lignes)
f1a-gguf-crosscheck.c          le second lecteur, ecrit pour ce cross-check
TQ2_TENSOR_INVENTORY.csv       168 tenseurs eligibles + en-tete, avec famille et epi count
EPISODE_GEOMETRY_E5.txt        geometrie 5 episodes : domaines, warps, echelle, cardinalites
EPISODE_GEOMETRY_E6.txt        geometrie 6 episodes : idem
gguf_dump_cible.json           vidage GGUF complet du modele cible
```

**Non produits** (dépendent de la capture) : `TARGET_OP_MAPPING.csv`,
`CAPTURE_HOOK_PROVENANCE.txt`, `CAPTURE_MANIFEST.csv`, `CAPTURE_COMPLETENESS.txt`,
`CAPTURE_SANITY.txt`, `REFERENCE_VALIDATION.csv`, `F1A_CAPTURE.patch`, `F1A_CAPTURE.patch.sha256`.

## 12 — Rapport final (§44)

```
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
MODEL_PATH=/home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf
MODEL_SHA256=1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937
FULL_MODEL_TENSOR_COUNT=866
GGUF_READER_CROSS_VALIDATED=YES
E5_TENSOR_COUNT=156
E5_ROW_COUNT=2007040
E5_LOGICAL_K=5120
E5_EPISODE_COUNT=5
E6_TENSOR_COUNT=12
E6_ROW_COUNT=61440
E6_LOGICAL_K=6144
E6_EPISODE_COUNT=6
EPISODE_LOGICAL_WEIGHT_GRANULARITY=1024_CONTIGUOUS_WEIGHTS
TARGET_MATMUL_OPERAND_MAPPING_PROVEN=NO_IDENTIFIE_PAR_LECTURE_DE_SOURCE_NON_PROUVE_EN_EXECUTION
CAPTURE_HOOK=NOT_IMPLEMENTED
CAPTURE_CANDIDATE_SITE=llama-graph.cpp:1476_build_lora_mm
CAPTURE_BUILD_SHA256=NOT_PRODUCED
QUALIFICATION_DECODE_TOKENS=0
EXPECTED_TARGET_INVOCATIONS=NOT_COMPUTED
OBSERVED_TARGET_INVOCATIONS=0
CAPTURED_E5_ACTIVATION_K=NOT_MEASURED
CAPTURED_E6_ACTIVATION_K=NOT_MEASURED
CAPTURED_ACTIVATION_NAN_COUNT=NOT_MEASURED
CAPTURED_ACTIVATION_INF_COUNT=NOT_MEASURED
EPISODE_REFERENCE_MODE=NOT_BUILT
EPISODE_REFERENCE_RECONSTRUCTS_FULL_OPERATOR=NOT_MEASURED
PRODUCT_KERNEL_MODIFIED=NO
MODEL_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
ENGINE007_NEXT_STEP=F1A_S2_IMPLEMENT_THE_CAPTURE_HOOK_AT_build_lora_mm
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```

## 13 — Un point de méthode que je dois signaler

En préparant cette passe, j'ai constaté que **`ggml-cuda-observe.cuh` n'existe pas sur disque** dans
l'arbre courant, alors que mes notes de travail le listaient comme un harnais existant et
réutilisable. Le harnais d'observation ENGINE-004 est une **greffe déclarée** appliquée par
`apply-observer.py`, pas un fichier du dépôt. Je l'ai découvert en le cherchant — et non en
supposant sa présence.

C'est la deuxième fois dans cette série qu'une de mes notes internes s'avère inexacte sur l'état
du dépôt (après le générateur de données de R9A). Je le consigne pour que la suite ne s'appuie pas
sur cet inventaire sans le revérifier.
