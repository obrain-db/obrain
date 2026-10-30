# ENGINE008_EXPECTED_COVERAGE_CONTRACT — N1, N2, N3 et la conservation

## 1. N1 : methode exacte, INDEPENDANTE des captures reussies

```cpp
ExpectedTargets compute_n1(inventory_names, model_tensors, inventory_path, tq2_type_value);
// methode : intersection(inventaire CSV valide,
//                        tenseurs du modele dont le TYPE REEL est TQ2_G128)
```

**`N1_INDEPENDENT_OF_SUCCESSFUL_CAPTURES=YES`.** N1 ne lit **aucune** ligne du
manifeste : il vient de l'**inventaire** (configuration, validee avant le run) et
des **types reels** des tenseurs du modele charge, lus via
`llama_internal_get_tensor_map`. Une capture qui a echoue ne peut donc pas
reduire N1.

**Unites tenues DISTINCTES, jamais confondues :**

| Unite | Ce que c'est |
|---|---|
| poids distinct | un `weight_name` |
| invocation | un appel a `build_lora_mm` (peut se repeter pour le meme poids) |
| capture | une ligne `(run_id, epoch, target_id, capture_id, kind)` |
| couple cible/epoch | `(epoch, target_id)` -- **l'unite de N2** |

**`N1_UNKNOWN` est un etat de premiere classe.** Si l'inventaire est vide/absent,
ou si le registre natif est indisponible, `N1=UNKNOWN` -- et **la completude est
alors INTERDITE**, quels que soient N2 et N3. Aucun nombre n'est invente depuis
168 ou 336.

**`EXPECTED_TARGET_SCOPE`** : les poids portant un nom de l'inventaire **ET** le
type `TQ2_G128`. **Un inventaire partiel ne represente PAS toutes les operations
WHT du modele** : il represente ce que l'inventaire couvre. Si l'inventaire ne
liste qu'une partie des tenseurs `TQ2_G128`, N1 est une **borne inferieure** et
doit etre dit comme tel.

**Deux limites declarees :**
- un poids **charge** n'est pas automatiquement une operation **construite** : les
  branches et la selection d'experts peuvent l'exclure. N1 compte des cibles
  *potentielles*, pas des operations effectives.
- N1 ne peut pas etre etabli avant le chargement du modele (il exige les types
  reels). Avant chargement : **N1=UNKNOWN**, completude interdite.

## 2. Reconciliation implementee

```cpp
CoverageReconciliation reconcile_coverage(ExpectedTargets, WhtManifestFile,
                                          RoleRelFile, EffectPathFile);
```

Exporte : `n1`, `n2`, `n3`, `n_lines`, `n_missing`, `n_unexpected`,
`missing_identities`, `unexpected_identities`, `complete`, `verdict`.

**Regles dures, chacune testee :**

| Interdit | Applique |
|---|---|
| `N2>0 -> COMPLETE` | non : sans N1 connu, `verdict=N1_UNKNOWN_BLOCKS_COMPLETENESS`, `complete=false` |
| `N3==N2 -> COMPLETE` alors que N1 inconnu | non : meme regle |
| additionner plusieurs `kind` comme plusieurs cibles | non : `n_lines` et `n2` sont **separes** et testes distincts |
| une fenetre suivante complete effacant une precedente incomplete | non : `merge_wht` conserve le **pire** maillon et **accumule** les identites |

Ordre des verdicts : `N1 UNKNOWN` -> `MISSING_EXPECTED_TARGETS` ->
`UNEXPECTED_CAPTURES` (ambigu) -> `MISSING_REQUIRED_RELATIONS` ->
`COVERAGE PARTIAL` -> chaine complete.

## 3. Conservation par fenetre -- et sa LIMITE

**Ordre reel trace** : extraction -> fermeture des surfaces (`fopen("w")`) ->
lecture (collecte dans la fenetre) -> conservation dans le rapport
(`merge_wht`) -> calcul suivant.

**Ce qui est conserve** : a chaque fenetre, `merge_wht` accumule
`missing_identities`, `unexpected_identities`, `blockers`, et conserve le pire
`wht_worst_progress`. **L'ecrasement des fichiers entre fenetres ne perd donc
plus ce qui a ete lu** -- teste.

**LIMITE PUBLIEE : `PER_WINDOW_ANALYSES_PRESERVED=PARTIAL`.**
`CAPTURE_EXTRACT` incremente `cc->epoch` (`f1a-capture.cpp:564, 586, 839`) et
ecrit ses fichiers a chaque extraction. **Si UNE fenetre contient PLUSIEURS
extractions, les precedentes sont ecrasees avant toute lecture** : la collecte
par fenetre ne suffit alors pas. Le rapport porte
`wht_max_extractions_per_window` pour rendre ce cas visible, mais **aucune
lecture par extraction n'est implementee** -- ce serait la seule facon de le
resoudre sans modifier Q1.
