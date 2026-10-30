# Consolidation obrain — 2026-09-10

`harness-CONSO-O` · chantier Ergaster `CONSOLIDATION-OBRAIN` · terrain n3.

Consolidation des deux dépôts obrain : le graphe (`banc-obrain`) et le fork
d'inférence (`llama-obrain`). Toutes les empreintes ci-dessous ont été relevées
avec `git rev-parse` / `git merge-base` sur n3 ; tous les comptes de tests sont
recopiés de la sortie `cargo`.

---

## 1. `banc-obrain` — branche `integration/zero-copy`

### Lignée réelle (l'hypothèse de départ était fausse)

Le brief supposait que `feat/zero-copy-suite` contenait déjà tout
`feat/zero-copy-views`, et donc que la consolidation serait acquise par
construction. **Ce n'est pas le cas.** Vérification :

```
git merge-base feat/zero-copy-views feat/zero-copy-suite
  -> e6ea350e3d7cc3a63c208b7cea007e3d7a40e5bb

git merge-base --is-ancestor feat/zero-copy-views feat/zero-copy-suite
  -> faux
```

`suite` a été branchée sur `views` **au 3ᵉ commit** (`e6ea350e`), pas sur sa
pointe. Le 4ᵉ commit de `views` manquait donc à `suite` :

| Branche | Pointe | Commits |
|---|---|---|
| `feat/zero-copy-views` (O1) | `83c53b28` | `8ac6d8ca` → `0004b13a` → `e6ea350e` → `83c53b28` |
| `feat/zero-copy-suite` (VWS) | `0b29991d` | *(sur `e6ea350e`)* `29696871` → `c68e9d3e` → `0b29991d` |

Commit orphelin : `83c53b28` — *« doc : tableau comparatif n3 x86_64 vs DGX GB10
aarch64 des benchs zero-copy »*, **doc seule** (`docs/DESIGN-PONT-MEMOIRE-ZERO-COPY-2026-09-10.md`,
+32 lignes, 1 fichier).

### Le merge (réel, pas fabriqué)

Un vrai merge était donc nécessaire — mais il ne porte que de la documentation.

```
commit de merge = 4889f5a712ee640833ee137ce4eac3891e6dfc97
  merge --no-ff de feat/zero-copy-views (83c53b28) dans feat/zero-copy-suite (0b29991d)
```

La pointe de `integration/zero-copy` est ensuite le commit du présent document,
posé directement au-dessus de ce merge (`git log -1 integration/zero-copy` pour
l'empreinte exacte — elle ne peut pas se citer elle-même ici).

Sans conflit. Contenance vérifiée : `git merge-base --is-ancestor` est vrai pour
`feat/zero-copy-views`, `feat/zero-copy-suite` **et** `main` (`2c6c5bab`).

```
git diff --stat feat/zero-copy-suite integration/zero-copy
  docs/DESIGN-PONT-MEMOIRE-ZERO-COPY-2026-09-10.md | 32 ++++++
  1 file changed, 32 insertions(+)
```

> **L'arbre de code est identique à `feat/zero-copy-suite`.** Le seul écart est
> le fichier de doc. Les mesures de perf de VWS restent donc valables mot pour
> mot sur la branche d'intégration.

Réalisé dans un worktree neuf `~/projects/banc-obrain-integ` : le checkout
principal de `banc-obrain` est sur `feat/mcp-next-step-hints` avec un arbre sale
(~20 fichiers modifiés) et n'a pas été touché.

### Validation

`cargo test -p obrain-substrate --release --no-fail-fast` (n3, x86_64).
Comptes = somme des lignes `test result` de la sortie cargo.

| | `integration/zero-copy` (`4889f5a7`) | base d'origine `main` (`2c6c5bab`) |
|---|---|---|
| cibles de test | 47 (46 `Running` + 1 `Doc-tests`) | 45 (44 + 1) |
| **passed** | **1 224** | 1 200 |
| **failed** | **3** | **3** |
| ignored | 8 | 8 |
| lib `obrain-substrate` | 824 passed / 0 failed / 1 ignored | 823 / 0 / 1 |
| code de sortie | `EXIT=101` | `EXIT=101` |

Ces chiffres recoupent exactement le rapport VWS (1 224 verts, 8 ignorés,
3 échecs, lib 824).

**Les 3 échecs sont préexistants — c'est prouvé, pas supposé.** Suite
`operators_filter`, `105 passed; 3 failed` **des deux côtés** :

- `test_cross_type_string_float_equality`
- `test_cross_type_string_int_equality`
- `test_cross_type_string_numeric_ordering`

Ils échouent **à l'identique sur `main` (`2c6c5bab`)**, soit ~150 commits *avant*
le moindre commit zero-copy. Ils ne sont donc imputables ni à O1, ni à VWS, ni à
la consolidation. (VWS avait fait la même démonstration sur `e6ea350e` ; la
refaire depuis `main` élargit la preuve.)

**Apport net de la consolidation :** +24 tests verts, +1 test de lib,
+2 suites d'intégration (`zero_copy_suite`, `zero_copy_views`), **0 nouvel échec**.

### Divergence avec `main`

Aucune dans le sens gênant : `git log integration/zero-copy..main` est **vide** —
`main` ne porte rien qui manque à la branche d'intégration. En revanche
`integration/zero-copy` est ~150 commits devant `main`, car la lignée zero-copy
est bâtie sur toute la chaîne dashboard/viz (`e9826f3b`, `af863a7f`, atlas/morph,
Cosmos/Monde/Graphe…) qui n'a jamais été fusionnée dans `main`.

---

## 2. `llama-obrain` — branche `integration/inject-2026-09-10`

### Lignée : pile strictement linéaire

```
integration/inject-2026-09-10 = 87c279b3e29d8e4223e434ac90f702133be89080
```

Créée **sans checkout** (`git branch`) : la branche courante `master` et son
arbre (2 fichiers ops modifiés) sont intacts.

```
git merge-base master fix/inject-batch-split
  -> 24e01e124bef... (= la pointe de master elle-même)
```

`master` est donc un **ancêtre direct** de la branche déployée. La pile, du plus
ancien au plus récent :

| # | sha | branche à cette pointe | objet |
|---|---|---|---|
| 0 | `24e01e1` | `master` | pont stable consolidé (`run_pont_vk.sh`, `qa_obrain.py`) |
| 1 | `227ddfa` | `feat/prompt-lookup-server` | portabilité aarch64 (gardes x86 F16C/AVX2) |
| 2 | `6e72f5d` | | J3 seeding ngram-map par texte brut |
| 3 | `fdc60b7` | `feat/j2-j3-couplage` | J2 sonde de confiance read-only (entropie top-k) |
| 4 | `2229c6a` | `feat/spec-inject-consumer` (SPX) | consommateur spéculatif de `/jspace/inject_knowledge` |
| 5 | `604c83e` | | **correctif DoS** : fractionnement du décodage J-space en lots de `n_batch` |
| 6 | `87c279b` | `fix/inject-batch-split` (FIX) | garde `OBRAIN_REC_TYPE` × `OBRAIN_STATE_TYPE` (refus au démarrage) |

Les **cinq** pointes de branches du dépôt (`master`, `feat/prompt-lookup-server`,
`feat/j2-j3-couplage`, `feat/spec-inject-consumer`, `fix/inject-batch-split`)
sont toutes contenues dans `integration/inject-2026-09-10` (`--is-ancestor`
vérifié pour chacune).

> **Aucun merge n'a été fabriqué ici : la consolidation était déjà acquise par
> construction.** `fix/inject-batch-split` *est* la réunion de tout le dépôt.
> `integration/inject-2026-09-10` ne fait que lui donner un nom stable.

### Ce qui tourne en production

Confirmé par l'événement de déploiement du chantier `FIX-inject-dos` dans le
store Ergaster (lecture seule — **la DGX et `:9910` n'ont pas été touchés**) :

- build dans un arbre isolé `llama-obrain-deploy`, branche `fix-deploy` sur
  `FETCH_HEAD` de `fix/inject-batch-split`, **HEAD `87c279b`**, `EXIT=0` ;
- `libllama-server-impl.so` 6 973 824 octets, horodatée 22:15 ;
- post-bascule : pid 44088, `readlink /proc/pid/exe` → binaire de déploiement,
  `/proc/pid/maps` confirme le `.so` de 22:15, `NRestarts=0` ;
- test qui tuait le service (fait de 10 601 caractères, bien au-delà des 2 048
  tokens de `n_batch`) → HTTP 200, service vivant, `health ok` ;
- non-régression chat OK, **`system_fingerprint b48-87c279b`**.

`87c279b` est bien la pointe de la lignée **et** le sha en production. Cohérent.

### Divergence avec `master` — le signal est inversé

Le brief anticipait que `master` puisse porter des travaux absents de la branche
déployée. **Ce n'est pas le cas** :

```
git log --oneline master ^fix/inject-batch-split   -> vide
git log --oneline fix/inject-batch-split ^master   -> 6 commits
```

`master` ne contient **rien** que la production n'ait. Le signal réel est
l'inverse et mérite attention :

> **`master` est 6 commits en retard sur la production.** Le correctif de déni de
> service (`604c83e`), la garde de types (`87c279b`), le consommateur spéculatif
> (`2229c6a`), la sonde de confiance J2 (`fdc60b7`), le seeding J3 (`6e72f5d`) et
> la portabilité aarch64 (`227ddfa`) tournent sur `:9910` **sans avoir jamais été
> fusionnés dans `master`**. `master` n'est donc pas représentatif de ce qui est
> servi : quiconque partirait de `master` réintroduirait le déni de service.

Aucune action forcée : rien n'a été fusionné dans `master`, la décision revient
au superviseur. La remontée de `integration/inject-2026-09-10` dans `master`
serait un fast-forward pur (aucun conflit possible, `master` étant un ancêtre).

### Compilation

Non recompilé sur n3 : gros build CUDA, sans bénéfice ici. La validation existe
déjà en deux endroits — sur Mac par FIX, et en production par le superviseur
(preuves de bascule ci-dessus).

---

## 3. Récapitulatif des livrables

| Dépôt | Branche | Sha | Nature |
|---|---|---|---|
| `banc-obrain` | `integration/zero-copy` | merge `4889f5a7` (+ ce doc au-dessus) | merge réel (doc seule) de `views` dans `suite` |
| `llama-obrain` | `integration/inject-2026-09-10` | `87c279b` | nommage d'une pile déjà linéaire — **= production** |

Worktrees créés sur n3 : `~/projects/banc-obrain-integ` (branche d'intégration),
`~/projects/banc-obrain-base` (base `main` jetable, pour la preuve de
préexistence des échecs). Aucun checkout existant n'a été modifié, aucune
branche courante cassée, la DGX n'a reçu que des lectures et `:9910` n'a pas été
redémarré.
