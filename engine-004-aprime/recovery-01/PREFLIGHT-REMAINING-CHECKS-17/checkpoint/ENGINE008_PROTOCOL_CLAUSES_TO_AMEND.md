# CLAUSES EXACTES A MODIFIER — pour v4 (ou v3)

Seules les clauses qui changent sont listees, avec leur reference. **Aucune
proposition complete n'est reecrite.** Statut : `PROPOSED_NOT_ADOPTED`.

## Clause A — perimetre WHT (v3 §1, v4 §1)

**Remplacer** « le perimetre WHT reellement demontrable » par :

> Le perimetre demontrable est celui des **poids du tronc atteignant
> `build_lora_mm`**, etablis par la source (`qwen35.cpp:239-241` : racine des
> logits = `cur` apres `output_norm` et `get_rows`) et par l'inventaire mesure
> (168 poids, `blk.8` a `blk.55`, aucun NextN).
> **Interdit** : etendre ce constat a tout poids du tronc comme preuve
> universelle de dependance aux logits. La reference est l'operation et le chemin
> **effectivement etablis**.

## Clause B — N1 (v4 §2)

**Ajouter** :

> N1 est l'ensemble des **poids eligibles du perimetre configure** :
> `intersection(inventaire, tenseurs dont le TYPE REEL est TQ2_G128)`.
> **Distinction obligatoire** des quatre ensembles :
> (i) poids eligibles, (ii) **invocations/cibles attendues pour l'epoch**,
> (iii) captures observees, (iv) relations disponibles.
> **Un poids charge n'est PAS automatiquement une invocation attendue** : les
> branches et la selection d'experts peuvent l'exclure. N1 compte (i), pas (ii).
> **Perimetre exact** : N1 couvre le **perimetre configure** de l'inventaire, et
> non la WHT du modele entier. Un inventaire seulement **valide** n'est pas
> **complet**.
> `N1=UNKNOWN` -> **completude interdite**.

## Clause C — extractions par fenetre (v4 §5, point 7)

**Remplacer** « une seule extraction par fenetre -- si plusieurs, la conservation
par fenetre est insuffisante » par :

> Le harnais **compte** les extractions par fenetre via les evenements
> `CAPTURE_EXTRACT` du ledger (mode ajout, `fflush` par ligne), et publie
> `wht_max_extractions_per_window`. **Une extraction par `process_ubatch`**
> (`llama-context.cpp:1783-1788`).
> Si une fenetre contient **plus d'une** extraction, les analyses anterieures sont
> ecrasees avant lecture : la couverture D1 est **declaree non obtenable** pour
> cette fenetre, et le run s'arrete.

## Clause D — P4' : les six conditions (v4 §4)

**Ajouter** :

> Les six conditions sont verifiees par `check_p4_structural()` (PASS/FAIL/UNKNOWN,
> avec preuve, perimetre et motif), et le resultat est **serialise** sous
> `ENGINE008_P4PRIME_CHECKS_v1` avec `"adopted":false`.
> **C1** registre natif complet ; **C2** types REELS lus sur les tenseurs
> (**jamais** deduits du nom `Q4_K_M`) ; **C3** aucun poids pertinent TQ2_G128 ;
> **C4** audit des sites de creation WHT ; **C5** identite du code et conditions
> du chemin ; **C6** limites enoncees.
> **C6 ne peut JAMAIS transformer un FAIL en PASS** (teste).
> Le verdict global est le **PIRE** des six.

## Clause E — gate (nouvelle)

> Les verifications P4' sont executees **apres la garde d'autorisation et apres le
> chargement du modele**, puisque C1-C3 inspectent le modele. Elles ne precedent
> **jamais** la garde.
> Leur resultat **ne remplace pas P4** dans les gates tant que l'amendement n'est
> pas adopte.
