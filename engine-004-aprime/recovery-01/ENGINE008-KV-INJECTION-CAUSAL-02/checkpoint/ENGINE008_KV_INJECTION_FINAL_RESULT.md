# ENGINE-008 — INJECTION KV NATIVE : RÉSULTAT FINAL

**Expérience `ENGINE008-KV-INJECTION-CAUSAL-02`.** Aucune nouvelle exécution
modèle n'a été effectuée pour produire ce document : il consolide les artefacts
bruts déjà obtenus.

---

## Conclusion autorisée

> **L'injection KV native écrit effectivement le cache.**
> **La perturbation de V à la couche 63 produit un effet causal sur les logits.**
> **La restauration retrouve exactement les logits du contrôle.**
> **Les trois générations produisent ici la même séquence de 32 tokens.**

---

## 1. Les trois conditions

| | Contexte | État du KV d'attention | Intervention |
|---|---|---|---|
| **A** | CONTROL | KV **calculé** par le prefill, **jamais touché** | aucune |
| **B** | PERTURBED | KV calculé, puis **couche 63 : V ← zéros f16, K inchangé** | écriture native, positions 0..57 |
| **C** | RESTORED | KV calculé, puis **perturbé sur les 16 couches**, puis **restauré** | écriture + restauration, aucune génération entre les deux |

Configuration commune : modèle candidat, contexte neuf par condition,
`n_ctx=512 / n_batch=512 / n_ubatch=512 / n_seq_max=1`, `ctx_type` défaut,
`embeddings_nextn=false`, argmax direct, arrêt `EOS` ou 32 tokens.

Entrée : **les 59 IDs autoritaires**, régénérés depuis le template qualifié et
**vérifiés par empreinte avant usage** —
`RENDERED_SHA256=92eb0cb3a51bbece69aea29afc87553f6b5b5fdd16a1f30485a31dc38ca1c658`,
`TOKEN_IDS_SHA256=ee1740718fd9fc85bfdec4edac4e561be0336b791d40b2f3514d24d310ece37e`,
`PREFLIGHT_RENDERED_TOKEN_COUNT=59`.

Le **même historique récurrent** a été établi dans les trois contextes par le
prefill des 58 premiers IDs, positions 0..57
(`RECURRENT_HISTORY_PREPARED_IN_ALL_THREE=YES`).

**Couches d'attention, déterminées empiriquement** (sonde `extract_layer` sur
les 64 index de couche, `rc==0` = présente dans le cache KV) :
`ATTENTION_LAYER_COUNT=16`,
`[3,7,11,15,19,23,27,31,35,39,43,47,51,55,59,63]` — une sur quatre, cohérent
avec `qwen35.full_attention_interval=4`. **Les 48 autres couches sont
SSM/Gated DeltaNet et n'ont aucun cache KV.**

Type natif du cache : **f16** pour K et V. `N_EMBD_K_GQA=N_EMBD_V_GQA=1024`,
`K_ROW_BYTES=V_ROW_BYTES=2048`, soit **118784 octets par couche** pour 58
positions.

---

## 2. Les 32 IDs et le texte exact, par condition

Vérifié depuis les artefacts bruts : les trois séquences sont **identiques
élément par élément**, et les trois textes sont **caractère pour caractère
identiques**.

```
CONTROL_IDS = PERTURBED_IDS = RESTORED_IDS =
[760,1156,369,9859,728,310,5707,1132,440,328,3793,1149,1061,369,264,4145,
 1622,13,353,1220,5707,440,1066,328,3793,1149,198,248069,271,3793,13,248046]
```

`32` tokens par condition. `STOP=EOS` dans les trois. `TOKEN_MATCH_A_B=YES`,
`TOKEN_MATCH_A_C=YES`.

Décomposition du texte, **tokens spéciaux conservés** :

| Élément | Valeur |
|---|---|
| Texte assemblé | `The user is asking me to respond only with "OK." This is a simple request. I should respond with just "OK."` |
| … puis fin de raisonnement | `</think>` (ID `248069`) |
| … puis réponse | `OK.` (IDs `271`, `3793`, `13`) |
| … puis fin de tour | `<|im_end|>` (ID `248046`, `EOS`) |
| Échappé exact | `The user is asking me to respond only with \"OK.\" This is a simple request. I should respond with just \"OK.\"\x0a</think>\x0a\x0aOK.<|im_end|>` |

Le token `248046` `<|im_end|>` est **conservé**, et non retiré : c'est le motif
d'arrêt réel. `248069` `</think>` est également conservé.

**L'argmax reste 760** dans les trois conditions, malgré des logits différents
pour B (§5). La divergence **ne franchit pas** la frontière de l'argmax sur ces
32 pas. **Ce n'est pas un échec** et aucune perturbation plus forte n'a été
tentée pour forcer une divergence de texte.

---

## 3. Preuve d'écriture sur les 16 couches — **PASS**

**Distincte du simple retour API.** Le buffer injecté **diffère** de l'original,
donc un readback égal au buffer **modifié** établit que l'**écriture** a eu lieu.
C'est précisément ce qui manquait dans l'expérience précédente, où les deux
contextes partageaient un KV identique et où un readback égal était attendu
même sans écriture.

Ordre imposé, **sans aucun decode entre perturbation et restauration** :
`INJECT_MODIFIED → READBACK_MODIFIED → INJECT_ORIGINAL → READBACK_ORIGINAL`.

Sur chacune des 16 couches, dans le contexte C :

```
DIFFERS_FROM_ORIGINAL=YES   INJECT_RC=0   READBACK_EQ_MODIFIED=YES
RESTORE_RC=0                READBACK_EQ_ORIGINAL=YES
```

`DISCRIMINATING_LAYERS=16`, **`NON_DISCRIMINATING_LAYERS=0`** — aucun V original
n'était déjà tout à zéro, donc le test est discriminant pour **toutes** les
couches et aucune n'est déclarée non discriminante.

Cas de la couche 63, avec empreintes FNV-1a 64 :
```
V_ORIG_FNV = fcf6b04e8b015f6c
V_ZERO_FNV = 2386d0592f82c383     (58 x 2048 octets a 0x00)
V_READ_FNV = 2386d0592f82c383     <- le readback rend EXACTEMENT les zeros injectes
```

`KV_WRITE_DISCRIMINATING_PROOF=PASS`

---

## 4. Preuve de restauration des octets KV — **PASS**

`KV_RESTORATION_BYTE_IDENTITY=PASS`
`C_RESTORED_LAYERS=16/16` — C a retrouvé ses K/V originaux **octet pour octet**.
`A_UNTOUCHED_LAYERS=16/16` — A n'a **jamais** été touché.

Condition préalable vérifiée **avant toute injection** :
`INITIAL_LAYERS_IDENTICAL=16/16` — les trois contextes partageaient bien le même
KV calculé, donc la perturbation était la **seule** différence. Hashes K et V
par couche consignés dans le journal brut.

---

## 5. Les logits — effet causal A/B, identité exacte A/C

```
LOGITS_A_VS_B  FINITE=YES  MAX_ABS_DIFF=2.75086212  L2_DIFF=183.210677
               ARGMAX_X=760  ARGMAX_Y=760  ARGMAX_MATCH=YES
               FNV_X=f4f4260fd33ce5b1   FNV_Y=c09ea86a5843669d
               BYTE_IDENTICAL=NO

LOGITS_A_VS_C  FINITE=YES  MAX_ABS_DIFF=0           L2_DIFF=0
               ARGMAX_X=760  ARGMAX_Y=760  ARGMAX_MATCH=YES
               FNV_X=f4f4260fd33ce5b1   FNV_Y=f4f4260fd33ce5b1
               BYTE_IDENTICAL=YES
```
(empreinte documentée : FNV-1a 64 bits sur les **octets** des f32, dans l'ordre
des indices de vocabulaire.)

**A vs B — effet causal observé.** Perturber **une seule** couche d'attention
(couche 63, V ← zéros, K inchangé) change **réellement** le calcul : les logits
diffèrent là où ils étaient identiques avant l'intervention. Maximum de
différence absolue `2.75`, L2 `183.21`, empreintes d'octets distinctes.

**A vs C — restauration exacte.** Après restauration, les logits sont
**byte-identiques** à ceux du contrôle : `MAX_ABS_DIFF=0`, `L2_DIFF=0`, même
empreinte. La restauration est exacte non seulement en **octets KV** (§4), mais
au niveau du **calcul**.

Les deux contextes sont sains : `FINITE=YES` partout, aucune valeur non finie.

`INJECTION_COMPUTATIONAL_EFFECT=OBSERVED`

---

## 6. Identités complètes

**Modèle :**
```
Qwen3.8-27B-OBRAIN-v3.gguf
sha256 1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937
```

**Pilotes** (instrumentations **distinctes**, non qualifiées par le gel
ENGINE008-Q1) :
```
e8-kv-causal  (CAUSAL-02)    sha256 568668145279652a0a1e8428aed5e4e4555752cd7f3b8429725eb3f5b22dc14b
e8-kv-roundtrip (RUN-01)     sha256 1d6ed9dde172b931e238117052596b991fd08dcf39f1a6f9ca24c022e7337aaa
e8-ids                       sha256 34bcd114a170b09bb19358c145f46cadb15201b769e89c1465d7dfd7f300e4e5
ids59.txt (59 IDs autoritaires) sha256 fc39bb86b0a942c2855d4cd90016f87bf655d4463731631658324e897021dbab
```

**Bibliothèques réellement résolues** (5, toutes dans
`llama-obrain-cuda-f1a/build-cuda/bin`) :
```
libllama.so.0        87c42d5f5dd7ab3c10967b4d3eaac82636b6af11e2c18a7f2798bab2d01bb2b2
libggml-base.so.0    af2c88dd19e87ebf0c902aa07dfcf53a04fbcc7a79c3fdaf19a6b856ab269c01
libggml.so.0         3d7f9ac51cb98936759a2e30acc1a7562e9122d3381d27f3955fff6be9016611
libggml-cpu.so.0     910a04c7af610aa65caf257b1d4ffe589cec608d5dd2a22e6ef5868367218f4f
libggml-cuda.so.0    7fdd87502b93191a0de3f62a9a15989f4fb63211c8cefb64d226b8f4480cdd70
```

**Template qualifié :**
`c3cf9e34abf4f9e36c2d72165aa9c132d3e2a725b6c2586aaa3a8af9d7a81041`

**API native utilisée** (publique uniquement) :
`llama_kv_cache_extract_layer`, `llama_kv_cache_inject_layer`,
`llama_kv_cache_type_k`, `llama_kv_cache_type_v`.
`llama_kv_cache_inject_begin` n'existe pas et n'a pas été appelé.

---

## 7. Les trois essais historiques — l'essai défectueux est conservé

**Aucun essai n'est supprimé. Ils ne sont pas regroupés sous un compteur unique.**

| # | Identité | Statut | Détail |
|---|---|---|---|
| **1** | `ENGINE008-KV-INJECTION-RUN-01`, exécution 1 | **DÉFECTUEUX — CONSERVÉ** | Bug de **mon** pilote : un appel mort `generate(A, 0, …)` décodait un token parasite en position 59, puis la reprise décodait dans une position **déjà occupée** (même classe que le défaut H1). Résultat incohérent : `CONTROL_TOKEN_IDS=[248046]` (EOS immédiat, 1 token) contre `INJECTED` 32 tokens. **La première exécution est conservée telle quelle.** |
| **2** | `ENGINE008-KV-INJECTION-RUN-01`, exécution 2 | VALIDE mais **NON DISCRIMINANT** | Après correction de l'appel mort : `TOKEN_SEQUENCE_MATCH=YES`, 32 tokens identiques. **Mais sa preuve d'écriture n'était pas discriminante** : les deux contextes avaient le même KV calculé, donc un readback identique était attendu **même sans écriture**. Déclaré comme tel, non présenté comme une preuve. |
| **3** | `ENGINE008-KV-INJECTION-CAUSAL-02` | **VALIDE ET DISCRIMINANT** | Écriture (§3), restauration (§4) et effet causal (§5) tous établis. |

Plus un **auto-test sans calcul** (`--selftest`), exécuté **avant** l'essai 3 :
`SELFTEST_RESULT=PASS FAILED=0` — vérifie la production des zéros f16, la
conservation des originaux, l'ordre perturbation/readback/restauration, le
contrat de positions et le contrat d'arrêt.

`ATTEMPT_COUNT = 3` exécutions modèle + 1 auto-test sans calcul.
Ce ne sont **pas** des retries : l'essai 2 est la **correction d'un bug de mon
pilote**, l'essai 3 une **expérience distincte explicitement autorisée**.

---

## Portée — ce qui n'est PAS démontré

- **Injection d'état KV uniquement.** Aucune modification du **graphe** de
  calcul, aucune modification des **poids**, aucun changement du backend.
  L'injection écrit un état d'entrée dans le cache.
- **Aucun transfert des 48 états récurrents SSM/Gated DeltaNet.** Seuls les K/V
  des 16 couches d'attention ont été manipulés. L'état récurrent a été
  **calculé** dans les trois contextes par leur propre prefill.
  `FULL_HYBRID_STATE_RESTORATION_DEMONSTRATED=NO`
- **Aucune élimination du prefill, aucune accélération.**
  Les trois contextes ont effectué le **même** prefill 0..57. Aucune suppression
  de prefill n'a été cherchée ni mesurée.
  `PREFILL_ELIMINATION_DEMONSTRATED=NO`
- **Aucune conclusion de qualité générale.** 32 tokens sur une seule entrée, un
  seul modèle, une seule perturbation. Ce n'est ni un scorer, ni une mesure
  NLL/PPL, ni une évaluation qualité.
- **Aucune recherche supplémentaire de divergence textuelle.** Devant des
  logits différents mais un argmax identique, je n'ai **pas** cherché à forcer
  une divergence : la preuve demandée porte sur les octets écrits et les
  logits, et elle est acquise.
- **La perturbation est une intervention synthétique** (V ← zéros f16)
  explicitement déclarée. Ce n'est **pas** une entrée sémantique qualifiée, et
  elle ne représente aucun état issu du modèle.

```
KV_WRITE_DISCRIMINATING_PROOF=PASS
KV_RESTORATION_BYTE_IDENTITY=PASS
INJECTION_COMPUTATIONAL_EFFECT=OBSERVED
PERTURBED_LAYER=63        INTERVENTION=ZERO_V_ORIGINAL_K
Q1_LIBLLAMA_REBUILT=NO    QUALITY_EXECUTION_ALLOWED=NO
```
