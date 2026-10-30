# ENGINE008_NEXTN_KEYS — cartographie des cles exactes

**Passe :** NEXTN-KEYS-01 — 2026-10-02
**Methode :** lecture des sites de chargement Q1. Aucun chargement de modele.

## 1. Construction du nom complet

`tn(TENSOR, suffix, i)` (`llama-model.cpp:1388+`) combine :
- le **modele de nom** de `LLM_TENSOR_NAMES` (`llama-arch.cpp:492-499`) ;
- un **suffixe** explicite (2e argument) ;
- un **indice de couche** (3e argument).

Le suffixe vide donne le poids principal ; `create_tensor` y ajoute `.weight`.
Les suffixes observes : `"scale"`, `"input_scale"` (compagnons de quantification).

**Ce qui etait juste dans l'adaptateur** : le suffixe `.weight` sur les poids
principaux. **Ce qui etait faux** : l'indice fige a `64`.

## 2. Borne de boucle — `hparams.n_layer_nextn`

- Parametre : `hparams.n_layer_nextn`, journalise en `llama-model.cpp:1981`.
- Accesseur public : `llama_model_n_layer_nextn(model)` (`llama-model.cpp:2426`).
- Cle GGUF : `LLM_KV_NEXTN_PREDICT_LAYERS` = `%s.nextn_predict_layers`
  (`llama-arch.cpp:201`). Candidat mesure : **1**. Baseline : **absente**.

## 3. MECANISME DECISIF : le filtre par type de contexte

`llama-model.cpp:2242-2248` — extrait verbatim :

```cpp
if (arch == LLM_ARCH_STEP35 && hparams.n_layer_nextn > 0) {
    if (params.ctx_type == LLAMA_CONTEXT_TYPE_MTP) {
        filter = [&](uint32_t il) { return il >= hparams.n_layer(); };
    } else {
        filter = [&](uint32_t il) { return il <  hparams.n_layer(); };
    }
}
```

Consequences, etablies par lecture :
1. Les couches NextN sont celles d'indice `>= hparams.n_layer()`.
2. Le chemin **standard** (`ctx_type` par defaut, donc != `MTP`) **filtre** ces
   couches : `il < n_layer()`. Elles ne sont pas chargees dans ce chemin.
3. Atteindre les couches NextN exige **explicitement**
   `LLAMA_CONTEXT_TYPE_MTP`.

C'est une **signature structurelle** du chemin, pas une convention de nom : le
chemin standard et le chemin MTP se distinguent par un **filtre d'indices**
gouverne par un **type de contexte**. C'est le temoin que les passes precedentes
cherchaient.

RESERVE DECLAREE : le filtre ci-dessus est ecrit pour `LLM_ARCH_STEP35`. Son
equivalent pour l'architecture des modeles evalues (`qwen35`) **doit etre
confirme** avant d'en tirer une regle. La STRUCTURE est etablie ; son
APPLICATION a qwen35 reste a verifier.

## 4. Optionalite

`llama_model_loader::TENSOR_NOT_REQUIRED` (`llama-model.cpp:1388`). Les
compagnons de quantification `_s` (`scale`, `input_scale`) sont optionnels :
`llama-model.cpp:1479-1480`, `:1544-1545` pour les roles NextN.
Les poids principaux sont crees **sans** ce drapeau : **obligatoires**.

Un tensor optionnel absent n'est NI un tensor normal, NI une preuve d'absence.

## 5. Etats de resolution (5)

| Etat | Sens |
|---|---|
| `APPLICABLE_PRESENT` | cle applicable et presente dans les metadonnees |
| `APPLICABLE_OPTIONAL_ABSENT` | cle applicable, optionnelle, absente |
| `APPLICABLE_REQUIRED_MISSING` | cle applicable, obligatoire, ABSENTE -> anomalie |
| `NOT_APPLICABLE` | role hors de l'architecture/configuration |
| `UNRESOLVED_CONDITION` | condition de chargement non tranchee |

## 6. Resolution par modele — methode, et limite

`n_layer_nextn` : candidat = 1 (mesure sur les metadonnees GGUF : 4 tenseurs
`blk.64.nextn.*`). Baseline : cle absente -> `n_layer_nextn = 0` -> **aucun role
NextN applicable**. Le baseline ne peut donc pas produire de temoin : c'est un
`NOT_APPLICABLE`, jamais un `ROLE_ABSENT` par defaut.

Indices a developper : `il` de `hparams.n_layer()` a
`hparams.n_layer() + n_layer_nextn - 1`. Pour le candidat : `n_layer()=64`,
`n_layer_nextn=1` -> indice unique **64**. Pour un modele a 2 couches NextN :
indices 64 et 65. **La constante 64 doit donc etre REMPLACEE par cette formule**,
ce qui corrige le defaut de l'adaptateur.

## 7. Ce qui n'a PAS ete fait

Le code de generation/raccord et les tests cibles n'ont pas ete ecrits :
limite de contexte. Les faits ci-dessus sont etablis par lecture des sources ;
ils ne sont pas encore implementes.
