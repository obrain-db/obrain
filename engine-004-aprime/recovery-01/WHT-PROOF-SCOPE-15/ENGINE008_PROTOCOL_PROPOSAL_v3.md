# ENGINE008_PROTOCOL_PROPOSAL_v3 — `PROPOSED_NOT_ADOPTED`

**Statut : PROPOSE, NON ADOPTE.** v2 reste
`822db99f6c288e346e1dca06c7405ab13a4a06d69bc05beccbe92e333fe5fa3d`.
Ce texte est pret a etre **adopte, amende ou rejete** par arbitrage.

## 1. Perimetre WHT reellement demontrable

**Demontrable, avec chaine source :**
- la racine des logits est `cur` apres `output_norm` et apres
  `ggml_get_rows(..., inp_out_ids)` (`qwen35.cpp:239-241`) ;
- les WHT observees portent sur des **poids du tronc** -- mesure : 168 noms
  distincts, couches `blk.8` a `blk.55`, **aucun** `blk.>=64`, aucun hors `blk.` ;
- ces WHT sont donc dans la **cloture de la racine des logits** ;
- `llama_get_logits_ith` **appelle `ctx->synchronize()`** : la lecture des logits
  acheve le travail soumis.

**Ce qui reste indémontrable statiquement :**
- le deroule du backend operation par operation -- **aucune surface ne l'observe** ;
- `downstream_reach` et `effect_class` etablissent « atteint **une** sortie
  declaree », pas « atteint **la** sortie des logits » : le constructeur declare
  `t_logits`, `t_embd` et `t_h_nextn`. **Ces champs ne distinguent pas les
  racines** ; la distinction vient de la source, pas d'eux.

## 2. D1 et D2 -- consequences, sans adopter D2

| | D1 (retenu) | D2 |
|---|---|---|
| Denominateur | toutes les cibles du perimetre (manifeste, borne inferieure) | cibles d'un epoch explicitement defini |
| Sur le run historique | 168/336 = 50 % -> `INCOMPLETE_COVERAGE` | 168/168 = 100 % sur epoch 1 |
| Ce qui resterait non couvert | rien de choisi arbitrairement | **tout le PREFILL** (epoch 0) |
| Compatibilite avec P3 | P3 exige « couverture suffisante » ; D1 garde l'exigence entiere | **choisit le perimetre apres avoir vu quels epochs reussissent** -- interdit |

**D1 est retenu** : conservateur, et il ne selectionne pas apres coup.
**`D2_ADOPTION_STATUS=NOT_ADOPTED`.**

**`D1_REQUIRED_COVERAGE_OBTAINABLE=YES` PAR LA COLLECTE PAR FENETRE.** Le
producteur ouvre `GRAPH_ROLE_RELATIONSHIPS.csv`, `TARGET_EFFECT_PATH.csv` et
`BACKEND_OWNERSHIP_MANIFEST.csv` en **`"w"`** : chaque extraction **ecrase** la
precedente. Une lecture unique en fin de run ne verrait que la derniere.
Le harnais collecte **dans chaque fenetre** (PROOF-13), donc il lit chaque
analyse **avant** l'ecrasement suivant -- ce que le producteur ne conserve pas.
**Sous reserve** que chaque extraction soit terminee dans sa fenetre : c'est une
**confirmation runtime**, pas un acquis statique.

## 3. Texte P4 -- inchange

`ENGINE008_P4_AMENDMENT_TEXT.md` (WHT-COVERAGE-CLOSE-14) reste le texte de
reference : P4' remplace une **propriete** par une **autre propriete**
(inatteignabilite structurelle au lieu d'absence observee), avec six conditions
**non verifiees sur un modele reel**. **`P4_AMENDMENT_STATUS=PROPOSED_NOT_ADOPTED`.**

## 4. Limites qui subsistent

1. **N1 non publie** : aucune surface ne publie le nombre de cibles
   attendues/eligibles d'un run complet. D1 s'applique donc sur N2 (manifeste),
   qui est une **borne inferieure** -- et doit etre dit comme tel.
2. **Execution non tracee par operation** : la preuve reste une **deduction**
   (soumission observee + synchronisation par la lecture + appartenance a la
   cloture de la racine des logits).
3. **Cache `obrain_rot`** : `rotation_applied=1` marque que le graphe contient la
   transformee ; cela n'etablit pas une **nouvelle** execution a chaque pas.
4. **P4' non verifiee** : six conditions, aucune etablie sur un modele reel.

## 5. Confirmations ATTENDUES pendant les quatre runs

1. `submission_rc == GGML_STATUS_SUCCESS` par pas -- **observe** ;
2. `llama_get_logits_ith` retourne un pointeur non nul par pas -- **observe**, et
   implique la synchronisation ;
3. l'`epoch` et le `decode_step` de chaque extraction s'alignent sur la fenetre
   correspondante -- **observe**, et c'est ce qui autorise la collecte par fenetre ;
4. le verdict WHT n'est **pas** partiel sur chaque fenetre ;
5. `ctx_type` et `embeddings_nextn` transmis et acceptes ;
6. `gtype` reste `GTYPE_DERIVED_FROM_VERIFIED_SOURCE_MAPPING` (jamais observe).

Ces confirmations sont des **criteres de run**, pas des prerequis.
`RUNTIME_OBSERVER_VALIDATION=NOT_EXERCISED`.
