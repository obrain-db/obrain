# ENGINE008_PROTOCOL_PROPOSAL_v4 — `PROPOSED_NOT_ADOPTED`

**Statut : PROPOSE, NON ADOPTE.** v2 reste
`822db99f6c288e346e1dca06c7405ab13a4a06d69bc05beccbe92e333fe5fa3d`.
Cette version **complete v3** ; elle ne la remplace pas.

## 1. D1 : couverture de TOUTES les fenetres du perimetre declare

D1 = toutes les cibles attendues (N1), sur **toutes** les fenetres du run, sans
selection d'epoch. Conservateur. D2 reste **NON ADOPTE** : il laisserait le
prefill hors couverture et choisirait le perimetre apres coup.

## 2. N1 / N2 / N3 et criteres exacts

| Grandeur | Definition | Critere |
|---|---|---|
| **N1** | `intersection(inventaire, tenseurs de type REEL TQ2_G128)` | **UNKNOWN** avant chargement ; si UNKNOWN, **completude INTERDITE** |
| **N2** | couples `(epoch, target_id)` distincts du manifeste | jamais la somme des `kind` |
| **N3** | cibles ayant `sub_at_build=1 & role_ok=1` **et** `effect_ok=1` | -- |

**Verdict de couverture COMPLETE** exige : N1 connu, `n_missing == 0`,
`n_unexpected == 0`, `n3 >= n2 > 0`, et le maillon de chaine atteint.

## 3. Preuve candidate : observation vs deduction

**Directement observe** : `submission_rc` du ledger ; retour non nul de
`llama_get_logits_ith` (qui **appelle `ctx->synchronize()`**) ; `epoch` et
`decode_step` de chaque extraction.

**Deduit du contrat source** : que la lecture des logits acheve le travail
soumis ; que la WHT est dans la cloture de la racine des logits
(`qwen35.cpp:239-241`).

**Jamais observe** : le deroule du backend operation par operation.

**INTERDIT** : promouvoir l'appartenance d'un poids au tronc en preuve
**universelle** de dependance aux logits. La reference est l'operation et le
chemin **effectivement etablis** : `t_logits = cur` puis
`ggml_build_forward_expand(gf, cur)` sur le graphe qwen35 standard, et
l'inventaire mesure (168 poids, `blk.8` a `blk.55`, **aucun** NextN).

## 4. P4' structurelle, avec ses six conditions

Texte de reference : `ENGINE008_P4_AMENDMENT_TEXT.md` (COVERAGE-14). Six
conditions, **aucune verifiee sur un modele reel**. La propriete change
(inatteignabilite structurelle au lieu d'absence observee) et cela doit etre dit
a l'arbitrage.

## 5. Confirmations requises pendant les runs

1. `submission_rc == GGML_STATUS_SUCCESS` par pas -- observe ;
2. `llama_get_logits_ith` non nul par pas -- observe ;
3. `epoch`/`decode_step` alignes sur la fenetre -- observe ;
4. **verdict WHT non partiel sur chaque fenetre** -- observe ;
5. `ctx_type` et `embeddings_nextn` transmis et acceptes ;
6. `gtype` : `GTYPE_DERIVED_FROM_VERIFIED_SOURCE_MAPPING` (jamais observe) ;
7. **une seule extraction par fenetre** -- si plusieurs, la conservation par
   fenetre est insuffisante et le run doit le signaler.

## 6. Arret au premier defaut, aucun retry

Repris du plan : arret immediat au premier echec, artefacts conserves, runs
suivants `NOT_RUN`, **aucun retry, aucune cinquieme invocation**. Le lanceur
l'applique deja (21 tests).

## 7. Conditions a verifier lors du preflight

Les verificateurs de P4' **doivent etre implementes et testes avant le gel** ;
leurs **resultats reels restent `NOT_EXERCISED`**. Etat actuel :
`P4_STRUCTURAL_CHECKS_IMPLEMENTED=NO` -- **c'est un manque a combler.**
