# ENGINE008_QUALITY_PROTOCOL_v1

**Passe d'écriture :** PROTOCOL-HARNESS-01 — 2026-10-02
**Hôte de référence :** DGX `spark-c578` (192.168.1.7, alias SSH `dgx`)
**Statut du document :** autorité normative nouvelle, écrite par cette passe.

> **Ce document est une autorité NOUVELLE.** Il ne prétend pas reconstituer à
> l'identique les anciens §1 à §74 du brief HARNESS, dont le texte n'a jamais
> été matérialisé (constat RECOVERY-01 : l'arbre ENGINE-008 ne contient que des
> artefacts DATA S1–S8 et SCORERS). Chaque règle est tracée vers sa source
> réelle dans `ENGINE008_PROTOCOL_SOURCE_MAP_v1.json`. Aucune règle nouvelle
> n'est attribuée à un ancien paragraphe absent.

---

## 0. Correction historique de localisation

La passe HARNESS précédente s'exécutait sur **n3**. Les autorités
DATA/SCORERS/Q1 résidaient sur le **DGX** (`spark-c578`, alias `dgx`,
192.168.1.7). L'absence locale constatée sur n3 ne démontrait **aucune**
suppression. Les autorités ont été retrouvées et vérifiées lors de
RECOVERY-01 ; le checkpoint transféré sur n3 reproduit les contrôles annoncés.

**Aucun résultat qualité n'existe à ce stade.** Aucun calcul modèle n'a eu
lieu, ni pendant RECOVERY-01, ni pendant cette passe.

---

## 1. Objectif et portée des conclusions

**Objectif adopté :** `CONTROLLED_IDENTICAL_INPUTS`.
Comparer baseline et candidat **à entrées bit-identiques**, de sorte que toute
différence mesurée ultérieurement soit imputable aux **modèles**, et non au
texte qui leur est présenté.

**Portée :**
- TASK uniquement : MMLU-Pro (knowledge_reasoning), GSM8K (math), MBPP (code),
  IFEval (instruction) — 2 698 items.
- TOKEN v2 est un corpus **distinct**, gelé séparément ; il n'est pas rouvert ici.
- Le **long contexte** est hors périmètre de cette passe et n'est pas rouvert.

**Ce que ce protocole ne conclut pas :** aucune supériorité, aucune équivalence,
aucune acceptation de modèle. Ces conclusions exigent l'exécution qualité, qui
est interdite par cette passe.

---

## 2. Modèles et autorités de runtime

| Rôle | Autorité | SHA256 |
|---|---|---|
| Baseline | `qwen3.8-27b-gguf/Qwen3.8-27B-Q4_K_M.gguf` | `31629f53165ab6a7dad8c9847dcfd1fdf55829dac1e6e748f4a68581b0033d34` |
| Candidat | `Qwen3.8-27B-OBRAIN-v3.gguf` | `1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937` |
| Runtime Q1 | `llama-obrain-cuda-f1a/build-cuda/bin/libllama.so.0` | `87c42d5f5dd7ab3c10967b4d3eaac82636b6af11e2c18a7f2798bab2d01bb2b2` |

`Q1_PREEXECUTION_BUILD_PROVENANCE.txt` fait autorité pour la provenance.
L'égalité du SHA de `libllama` avec cette autorité a été **mesurée** en
RECOVERY-01. Aucun rebuild de `libllama` n'est autorisé ; son SHA de référence
ne peut pas être modifié.

**Sélection du build :** le fork contient plusieurs constructions aux SHA
différents. **Seul `build-cuda` correspond à Q1**, identifié par sa provenance
et non par son nom. `build-r2-base` et `build-e5c` ont des SHA distincts et
sont écartés.

**Mode de chargement des modèles par le harnais :** `vocab_only = true`.
Aucune pondération n'est chargée, aucun graphe n'est construit : aucun forward
pass n'est structurellement possible.

---

## 3. Renderer et template effectif

**Renderer :** `common_chat_templates_apply` (moteur Jinja natif minja du fork
Q1), bibliothèque `libllama-common.so.0.0.135`.

**Alternative rejetée :** `llama_chat_apply_template`. Mesure : son propre
en-tête (fork Q1, `include/llama.h` l.1593) déclare qu'elle n'utilise **aucun
parseur Jinja** et ne supporte qu'une liste prédéfinie ; aucun template Qwen n'y
figure, et sa détection route tout template contenant `<|im_start|>` vers
`LLM_CHAT_TEMPLATE_CHATML`. Elle n'aurait exécuté ni `reasoning_instructions`,
ni `enable_thinking`, ni `sysns`.

**Template effectif (autorité commune) :**
`ENGINE008_EFFECTIVE_CHAT_TEMPLATE.jinja`,
`sha256 = c3cf9e34abf4f9e36c2d72165aa9c132d3e2a725b6c2586aaa3a8af9d7a81041`,
8 952 octets — octets exacts du template embarqué **baseline**, transmis
**explicitement** via `common_chat_templates_init(model, chat_template_override)`.

**Les templates embarqués ne sont pas utilisés individuellement.**
`EMBEDDED_CHAT_TEMPLATE_MATCH=NO` demeure déclaré :

| Modèle | Template embarqué |
|---|---|
| Baseline | `c3cf9e34abf4f9e36c2d72165aa9c132d3e2a725b6c2586aaa3a8af9d7a81041` |
| Candidat | `12827f24b742ea4e80cdc12dbcf9622227056b9f797252a3149263d4f9aaadce` |

Vérification exigée et **mesurée** : le chemin candidat ne récupère pas son
template embarqué par défaut (`EMBEDDED_TEMPLATE_OVERRIDDEN_BY_EXPLICIT=YES`).

**Aucun GGUF n'est modifié.** Aucun commentaire de template n'est retiré.

---

## 4. Messages TASK et paramètres de rendu

Structure : `SYSTEM_MESSAGE_PRESENT=NO`, un unique message `user`, puis le
préfixe assistant (`add_generation_prompt=true`). Le bloc `system` éventuel du
texte rendu provient du **template**, jamais de l'appelant.

Paramètres normatifs complets : voir `ENGINE008_RENDER_CONFIG_v1.json`.
Valeurs retenues : `use_jinja=true`, `add_generation_prompt=true`,
`enable_thinking=true`, `reasoning_format=NONE`, `add_bos=false`,
`add_eos=false`, `force_pure_content=false`, `tools=[]`,
`parallel_tool_calls=false`, `chat_template_kwargs={}` (vide).

`reasoning_effort` est **absent** du contexte Jinja : le template applique son
propre défaut `'xhigh'`. Cette absence est normative et écrite.

**Les quatre états sont distingués** (variable absente / présente `false` /
présente chaîne vide / présente valeur explicite) et ne sont pas interchangeables.
La fixture `thinking_off` mesure la différence : 81 octets rendus contre 307.

Les prompts sémantiques sont construits par le harnais selon
`TASK_PROMPT_CONTRACTS_v1.txt`
(`sha256 = fa30f16d95a564491e6d02666350ba783d6f44493ea5d04cef024e92ef0c3bc6`),
sans reformulation : MMLU-Pro (options existantes uniquement, 3..10), GSM8K,
MBPP, IFEval (prompt upstream exact).

---

## 5. Tokenisation et politique de tokens spéciaux

`add_special=false`, `parse_special=true` ⇒ `TASK_CHAT_PARSE_SPECIAL=YES`.
Le texte rendu contient des tokens de contrôle du template ; ils doivent être
interprétés comme spéciaux.

**Politique BOS (reprise des preuves recouvrées, non redécidée) :**
`add_bos=false` des deux côtés. La métadonnée baseline vaut `False` ; la clé est
absente côté candidat, et le défaut de branche `qwen35` vaut `false`.
`TASK_DOUBLE_BOS_RISK=NO`. Ni l'un ni l'autre template n'émet de marqueur BOS.

Ces propriétés ne sont pas confondues avec d'autres paramètres d'ajout de tokens
spéciaux : **les arguments effectivement passés sont tracés** dans le rapport de
rendu.

---

## 6. Ordre des items

`knowledge_reasoning`, puis `math`, puis `code`, puis `instruction` ; à
l'intérieur de chaque catégorie, l'ordre DATA déjà gelé. Émis dans
`TASK_GLOBAL_ITEM_ORDER.txt`. Identités uniques exigées.

---

## 7. Modes de scoring

Recopiés de `TASK_PROMPT_CONTRACTS_v1` (autorité SCORERS recouvrée) — **non
reformulés** :

- **MMLU-Pro** — `scoring = CHOICE_CONDITIONAL_LOGLIKELIHOOD`.
  `choice_loglikelihood = sum(log P(candidate_token_j | prefix + previous candidate tokens))`.
  `NORMALISATION_PAR_LONGUEUR=NON` (les labels sont des symboles courts).
  `boundary_safe_tokenization` : tokens(prefix+candidate) moins tokens(prefix),
  **uniquement si** tokens(prefix) est un préfixe exact de la concaténation.
  `tie_policy = plus PETIT index parmi les maxima ex aequo`.
  `max_new_tokens` : sans objet.
- **GSM8K** — `max_new_tokens=1024`, `GREEDY=YES`. Parser `^\s*FINAL:\s*(.*?)\s*$`,
  exactement une ligne `FINAL`. Référence = ce qui suit `####` dans `answer`
  amont. Arithmétique `Fraction`/`Decimal` **exacte** (jamais de float binaire).
- **MBPP** — `max_new_tokens=1024`, `GREEDY=YES`. Extraction : 1 bloc ```python →
  contenu ; 1 bloc générique → contenu ; aucun bloc → réponse entière ; sinon
  `EXTRACTION_ERROR`. Sandbox docker, un seul backend.
  **Jamais exposés au modèle :** `reference_code`, `test_imports`, `test_list`.
- **IFEval** — `max_new_tokens=4096`, `GREEDY=YES`. Scorer officiel pinné,
  révision `e49bbfe381c9c0e564b937f1c4e163a2273c65cc`. Gate principal `STRICT`
  (`follow_all_instructions`) ; diagnostic `LOOSE`.

**Commun :** `GREEDY_REQUIRED=YES`,
`TASK_AGGREGATION_MODE=FOUR_CATEGORY_MACRO_AVERAGE`.

**Décodage prescrit :** `next_token = argmax(logit)`, égalité exacte →
**plus petit token ID**. Aucun sampler de température, aucun tirage aléatoire,
aucun RNG de génération. Traitement des logits invalides et des cas dégénérés
défini explicitement ; aucun comportement dépendant de l'ordre accidentel d'un
conteneur.

---

## 8. Limites de génération, règles d'arrêt

`GSM8K_MAX_NEW_TOKENS=1024`, `MBPP_MAX_NEW_TOKENS=1024`,
`IFEVAL_MAX_NEW_TOKENS=4096` — **repris de l'autorité, non modifiables**.
EOS : `EOS_TOKEN_ID=248046`, présent des deux côtés.
`GENERATE_STOP_TOKEN_IDS=[248046]`.
**Audit du rôle de l'EOS dans le template — RÉSOLU, sur preuve statique :**

| Token de contrôle | ID (baseline) | ID (candidat) |
|---|---|---|
| `<\|im_end\|>` | **248046** | **248046** |
| `<\|im_start\|>` | 248045 | 248045 |
| `<\|endoftext\|>` | 248044 | 248044 |

`EOS_TOKEN_ID=248046` **est** `<\|im_end\|>`, le token que le template emploie
pour clore **chaque** bloc de message, y compris la branche assistant
(`'<\|im_end\|>\n'`). L'audit confirme donc que cet EOS est le stop approprié
pour une réponse assistant, et `GENERATE_STOP_TOKEN_IDS=[248046]` est **gelé**.

Cette confirmation est **structurelle** (identité de token + usage dans le
template), pas une observation runtime : la confirmation runtime reste exigée
par `ENGINE008_RUNTIME_PROOF_CONTRACT_v2.md`.

---

## 9. Seuils et règles statistiques

Recopiés de `QUALITY_THRESHOLDS_v1.txt` (gelé, aucune modification après mesure,
`sha256=08d70467d2dada51a362bfa91694b00d6bd0b1dd020feab2f7bb52af47666bc7`) :

- `MAX_MEAN_NLL_DELTA_NATS=0.0100`
- `MAX_RELATIVE_PERPLEXITY_INCREASE=exp(0.0100)-1` (≈1.005 %)
- `MIN_TOP1_TOKEN_AGREEMENT=0.970`

Bootstrap : `BOOTSTRAP_RESAMPLES=10000`,
`BOOTSTRAP_SEED_U64=5833308329814621762`,
`BOOTSTRAP_SEED_MATERIAL=ENGINE-008-QUALITY-v1|paired-bootstrap|v1`,
algorithme de percentile `empirical_linear_interpolation_k=(n-1)*p`,
stratification `per_category_matched_indices`.

Le harnais ne prend **aucune** décision sémantique supplémentaire : en cas
d'égalité exacte, il émet tous les scores et le scorer gelé applique
« plus petit index ».

---

## 10. Gates de préparation et de gel

**Gates de préparation (mesurées dans cette passe) :** autorités présentes et
vérifiées ; choix A matérialisé ; renderer natif qualifié ; harnais construit
avec le runtime Q1 exact ; selftests passés ; 2 698 entrées produites ; contrôles
de texte, tokens, ordre et reproductibilité tous nuls ; contrats TOKEN et SCORERS
préservés ; paramètres normatifs déterminés ; contrat de preuves runtime future
exploitable ; gardes interdisant réellement le calcul.

**Gates d'égalité (mesurées, aucun rattrapage autorisé) :**

```
TASK_RENDERED_INPUT_COUNT                          = 2698
TASK_RENDERED_TEXT_BASELINE_CANDIDATE_MISMATCH_COUNT = 0
TASK_INPUT_TOKEN_ID_BASELINE_CANDIDATE_MISMATCH_COUNT = 0
TASK_INVALID_TOKEN_ID_COUNT                        = 0
TASK_RENDER_SECOND_PASS_TEXT_MISMATCH_COUNT        = 0
TASK_RENDER_SECOND_PASS_TOKEN_MISMATCH_COUNT       = 0
MMLU_PREFIX_EXTENSION_NONPREFIX_COUNT              = 0
MMLU_CANDIDATE_SUFFIX_TOKEN_MISMATCH_COUNT         = 0
```

**Une différence est un échec de gate, jamais une occasion de retoucher les
entrées jusqu'à obtenir l'égalité.**

`MMLU_PREFIX_STATE_REUSE=NO` (correctness-first, v1) : aucun snapshot KV, aucun
clonage de contexte.

---

## 11. Preuves runtime exigées à l'exécution

Contrat complet : `ENGINE008_RUNTIME_PROOF_CONTRACT_v2.md`.
Quatre statuts distincts et **non interchangeables** : `EXPECTED`,
`METHOD_DEFINED`, `STATICALLY_SUPPORTED`, `RUNTIME_CONFIRMED`.

`RUNTIME_CONFIRMED` ne devient **jamais** `YES` sur la seule base des sources.

Propriétés concernées : décodeur standard effectivement invoqué ; absence de
décodage spéculatif et d'exécution NextN/MTP ; distinction **tenseurs chargés /
couche présente / couche exécutée** ; rotation baseline attendue `NO` ; rotation
candidat attendue `YES` selon l'autorité recouvrée ; variables d'environnement
devant rester `UNSET` (`OBRAIN_NO_ROTATION`, test de **présence** : `0` serait
également incorrect).

Méthode de capture **sans modification du calcul Q1**. Leur échec interdira
l'acceptation des résultats concernés. Aucune gate de préparation n'exige le
calcul modèle tout en l'interdisant.

**Cette passe : `RUNTIME_CONFIRMED=NO` pour toutes — aucun forward pass.**

---

## 12. Formats des résultats

- `TASK_RENDERED_INPUTS.jsonl` — un objet par item : `item_id`, `category`,
  `scorer_id`, `semantic_prompt_sha256`, `chat_template_sha256`,
  `rendered_prompt`, `rendered_prompt_sha256`, `baseline_input_token_ids`,
  `candidate_input_token_ids`, `input_token_count`, `generation_prefix`,
  `max_new_tokens` le cas échéant.
- `MMLU_CHOICES_INPUTS.jsonl` — `item_id`, `answer_index`, `option_count`,
  `prefix_token_ids`, `choices[{label, candidate_token_ids}]`.
- `TASK_GLOBAL_ITEM_ORDER.txt` — `categorie<TAB>item_id`.
- Résultats qualité futurs : schéma défini par la passe d'exécution ; **aucun
  scorer métier dans le harnais**.

---

## 13. Conditions d'arrêt

Arrêt à la fin de PROTOCOL-HARNESS-01. **`ENGINE008_EXECUTION_ALLOWED=NO`**,
même si le gel réussit. Aucun enchaînement automatique vers l'exécution qualité
ni vers une requalification Q1.

Interdits maintenus pour toute la passe : `llama_decode`, `llama_encode`,
prefill, forward pass, logits modèle, génération, NLL/PPL, benchmarks qualité ou
long contexte, essais exploratoires servant à choisir paramètres ou seuils,
requalification implicite de Q1, modification du calcul produit.

Les vecteurs synthétiques et mocks sont autorisés pour tester les algorithmes, et
ne sont **jamais** présentés comme des observations modèle.

---

## 14. Valeurs normatives indéterminées — déclarées

Conformément à la règle « ne pas geler un protocole contenant une valeur
normative indéterminée » :

1. `GENERATE_STOP_CONTRACT` pour un tour assistant — déterminé
   structurellement (§8), **confirmation runtime encore requise**.
2. Statut d'autorité de `libllama-common.so.0.0.135` — **constaté**
   (`a87c31d2e68ebadf3c21b4718b7bf58c34cb97c9a5b1145a7fa30df82f1a4b20`), **non
   qualifié** par une autorité écrite préalable. Le renderer repose donc sur un
   composant dont la provenance est établie mais non qualifiée. Déclaré, non
   masqué.
3. Documentation MTP propre à ENGINE-008 — produite par cette passe à partir des
   sources Q1 et des audits recouvrés ; absence de texte antérieur déclarée.
