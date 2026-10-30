# ENGINE008_RENDERER_QUALIFICATION_CLOSURE_v1

**Passe :** PREFLIGHT-INTEGRATION-FREEZE-02 — 2026-10-02
**Verdict : `RENDERER_QUALIFIED=YES`** — qualification ADDITIONNELLE, distincte de
la qualification historique du calcul Q1.

---

## 1. Composition exacte qualifiee

| Composant | Identite complete |
|---|---|
| `libllama-common.so.0.0.135` | `a87c31d2e68ebadf3c21b4718b7bf58c34cb97c9a5b1145a7fa30df82f1a4b20` |
| `libllama.so.0.0.135` (runtime Q1) | `87c42d5f5dd7ab3c10967b4d3eaac82636b6af11e2c18a7f2798bab2d01bb2b2` |
| `libggml.so.0.16.0` | `3d7f9ac51cb98936759a2e30acc1a7562e9122d3381d27f3955fff6be9016611` |
| `libggml-base.so.0.16.0` | `af2c88dd19e87ebf0c902aa07dfcf53a04fbcc7a79c3fdaf19a6b856ab269c01` |
| `libggml-cpu.so.0.16.0` | `910a04c7af610aa65caf257b1d4ffe589cec608d5dd2a22e6ef5868367218f4f` |
| `libggml-cuda.so.0.16.0` | `7fdd87502b93191a0de3f62a9a15989f4fb63211c8cefb64d226b8f4480cdd70` |
| Template effectif commun | `c3cf9e34abf4f9e36c2d72165aa9c132d3e2a725b6c2586aaa3a8af9d7a81041` (8952 o) |
| Fonction de rendu | `common_chat_templates_apply` (`common/chat.h:324`, `common/chat.cpp:2646`) |
| Fork | `feat/cuda-tq2g128` @ `8447604f7f0c3b48a77be46de2466a5b23e27826` |

**LIMITE HISTORIQUE DECLAREE** : `libllama-common.so` n'a **aucun** SHA d'autorite
ecrite prealable. Cette qualification est **nouvelle** et cree cette autorite ;
elle ne pretend pas restaurer une qualification ancienne. La qualification du
**calcul Q1** reste distincte et inchangee.

## 2. Ce qui est prouve, et ce qui ne suffisait pas

**Ne suffisait pas** : l'egalite des rendus entre baseline et candidat prouve la
**symetrie**, pas la **correction**. Elle est conservee comme un resultat, pas
comme la preuve.

**Attentes SEMANTIQUES independantes**, verifiees sur le rendu REEL de la fixture
`simple_ascii` (307 octets) :

```
<|im_start|>system\n
Reasoning effort is set to xhigh. Please think carefully through the task,
validate key assumptions, consider plausible alternatives, and prioritize
correctness, consistency, and clarity in the final answer.<|im_end|>\n
<|im_start|>user\nWhat is 2+2?<|im_end|>\n
<|im_start|>assistant\n<think>\n
```

| Attente | Mesure |
|---|---|
| bloc `system` emis par le TEMPLATE (non fourni par l'appelant) | **True** |
| instructions de raisonnement par defaut presentes, texte exact | **True** |
| bloc `user` present | **True** |
| ouverture assistant presente | **True** |
| ouverture du raisonnement `<think>` presente | **True** |
| **absence de fallback ChatML** : la 1re ligne est `<|im_start|>system`, non `<|im_start|>user` | **True** |
| `thinking_off` : AUCUNE instruction de raisonnement, 81 o contre 307 | **True** |
| absence de troncature (4000 caracteres, 4295 o rendus, +295 o de cadre) | **True** |
| Unicode preserve | **True** |

**Override du template embarque du candidat** (branche reellement utilisee) :
`MODEL_EMBEDDED_TEMPLATE_SHA256=12827f24…` mais
`RENDERER_TEMPLATE_SOURCE_SHA256=c3cf9e34…` et
`EMBEDDED_TEMPLATE_OVERRIDDEN_BY_EXPLICIT=YES`.

Les six fixtures rendent des SHA256 **identiques** entre les deux modeles —
symetrie conservee, et `graph_mtp` n'est pas en jeu ici.

## 3. Perimetre fonctionnel qualifie

Le renderer qualifie couvre **exactement** : rendu d'un tableau de messages
`[{role:user}]` avec `use_jinja=true`, `add_generation_prompt=true`,
`enable_thinking=true`, `chat_template_kwargs={}`, `add_bos=false`,
`add_eos=false`, sur le template commun impose explicitement.

**Hors perimetre qualifie** : branches outils, `reasoning_format` non NONE,
messages system fournis, variantes multi-tours. Non exercees, non qualifiees.

## 4. Tests utilises

Chemin `vocab_only` uniquement (deja autorise). **Aucun chargement complet,
aucun forward, aucun recompute des 2 698 entrees.** Les fixtures ont ete
re-executees sur le binaire `aa7e4bf45f69854fe92e871d4bcf352b99972a2dc1572fd6af62640c8b988ce9`.
