# ENGINE008_DENOMINATORS — trois comptes distincts

## Sémantique des compteurs du producteur, verifiee AVANT usage

| Compteur / champ | Ce qu'il compte | Piege evite |
|---|---|---|
| `eligible_target_count` | cibles **eligibles** entrees dans le collecteur | **n'est PAS `n_nodes`** (0 contre 4315) -- deja corrige en CLOSE-08 |
| `captured_target_count` | cibles effectivement capturees | -- |
| lignes du manifeste | **lignes**, 2 par cible (`activation` + `base_output`) | confondre lignes et cibles **double** le compte |
| `n_distinct_targets` | `(epoch, target_id)` | `target_id` est **PER-EPOCH**, pas global |
| `n_distinct_identities` | `(run_id, epoch, target_id, capture_id, kind)` | sans `kind`, 336 faux doublons |

## Les trois denominateurs, materiaslises

| # | Denominateur | Valeur historique | Ce qu'il etablit |
|---|---|---|---|
| **N1** | cibles **attendues/eligibles** dans le perimetre demande | **NON CONNU** -- aucune surface ne le publie pour un run complet | exige le registre natif + l'inventaire TQ2 |
| **N2** | cibles **capturees** | **336** (`(epoch,target)` distincts) | ce qui a ete observe |
| **N3** | cibles disposant des **relations requises** (roles/effet) | **168** | ce qui a ete analyse |

**`EXPECTED_CAPTURED_JOINED_COUNTS_DISTINGUISHED=YES`.**

**Point cle, et il est explicite dans la consigne :** le manifeste sert de
denominateur pour la **couverture des relations PARMI les captures** (N3/N2). Il
**ne prouve pas** que toutes les cibles attendues ont ete capturees (N2/N1). Le
code applique le denominateur conservateur N2 (manifeste), jamais N3 -- ce qui
serait circulaire.

## D1 vs D2, compares SANS adopter D2

**D1 -- toutes les cibles du perimetre complet demande.**
- Denominateur : N1 (ou, a defaut, N2 comme borne inferieure explicite).
- Sur le run historique : 168/336 = **50 %** -> `INCOMPLETE_COVERAGE`.
- **D1_REQUIRED_COVERAGE_OBTAINABLE=YES PAR LA COLLECTE PAR FENETRE.** Le
  producteur ecrase ses fichiers a chaque extraction, donc une lecture unique en
  fin de run ne peut pas obtenir D1. Mais le harnais collecte **dans chaque
  fenetre** (PROOF-13), donc il lit chaque analyse **avant** l'ecrasement
  suivant. D1 devient obtenable -- sous reserve que chaque extraction soit
  terminee dans sa fenetre, ce qui est une **confirmation runtime**.
- Limite : N1 reste non publie ; D1 est donc applique sur N2, ce qui est une
  **borne inferieure** et doit etre dit comme tel.

**D2 -- cibles d'un epoch explicitement defini et effectivement analyse.**
- **PROPRIETE DEMONTREE** : sur l'epoch choisi, toutes les cibles capturees de cet
  epoch ont une analyse de role/effet concordante (`role_ok=1`, `effect_ok=1`,
  `in_submitted_graph=1` sur 168/168, 504/504).
- **CE QUI RESTERAIT NON COUVERT** : tous les autres epochs, **en particulier le
  PREFILL** (epoch 0). Sur le run historique, D2 sur epoch 1 laisserait 168
  cibles -- la totalite du prefill -- hors couverture.
- **Compatibilite avec P3** : le texte exact de v2 exige « rotation detectee,
  couverture suffisante ». « Suffisante » n'est pas defini par epoch. Adopter D2
  **choisirait** le perimetre **apres** avoir vu quels epochs reussissent -- ce
  que la consigne interdit : « Ne pas selectionner apres coup uniquement les
  epochs dont les associations reussissent. »
- **D2_ADOPTION_STATUS=NOT_ADOPTED.**

**Choix retenu par le code : D1 sur N2 (manifeste), conservateur.**
