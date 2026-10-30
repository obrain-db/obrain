# ENGINE008_TARGET_RECONCILIATION — les 168 cibles non couvertes

**Source :** `.../e7b-f1a-q1-forward-v1/on/capon/`, mesure sur pieces.

## Identites, verifiees producteur par producteur

**N'ayant PAS adopte `(epoch, target_id)` comme cle universelle sans verifier**,
voici ce que chaque surface identifie reellement :

| Surface | Cle d'identite reelle | Granularite |
|---|---|---|
| `GRAPH_CAPTURE_MANIFEST.csv` | `(run_id, epoch, target_id, capture_id, kind)` = **672** | une LIGNE par (cible, kind) |
| idem, cible | `(epoch, target_id)` = **336** | 168 par epoch |
| `GRAPH_ROLE_RELATIONSHIPS.csv` | `(epoch, target_id)`, **epoch 1 uniquement** | 168 lignes |
| `TARGET_EFFECT_PATH.csv` | `(epoch, target_id)`, **epoch 1 uniquement** | 168 lignes |
| `RESIDENCY_MANIFEST.csv` | `target_id`, 3 roles par cible | 504 = 168 x 3 |

## Tableau de reconciliation

| Grandeur | Valeur |
|---|---|
| lignes du manifeste | **672** |
| captures distinctes `(epoch, target_id, capture_id, kind)` | 672 |
| cibles distinctes `(epoch, target_id)` | **336** |
| associations attendues (cibles x surfaces roles/effet) | **336** |
| associations presentes | **168** |
| associations manquantes | **168** |
| -- dont epoch 0 | **168** |
| -- dont epoch 1 | **0** |

## Motif, fonde sur les sources et les fichiers

**CLASSE : regle de selection du producteur -- portee temporelle des
accumulateurs.**

Les faits mesures :
- le manifeste porte les **deux** epochs : `epoch 0` (336 lignes) et `epoch 1`
  (336 lignes) ;
- les trois surfaces roles/effet/residence ne portent QUE `epoch 1` ;
- les soumissions se repartissent : `epoch 0 -> compute_submission_id=1,
  decode_step=-1` (le PREFILL) ; `epoch 1 -> compute_submission_id=2,
  decode_step=0` (le pas de DECODE) ;
- sur les lignes PRESENTES, tous les statuts producteur sont a 1
  (`role_ok` 168/168, `effect_ok` 168/168, `in_submitted_graph` 504/504).

**Conclusion : les 168 manquantes sont les cibles de l'epoch 0, le prefill.**
L'analyse de role/effet/residence est produite sur le chemin
`CAPTURE_EXTRACT` (epoch 1, pas de decode), tandis que le manifeste accumule
tous les epochs.

**Je ne conclus PAS que « la moitie est normale » depuis le seul ratio** : le
motif est etabli par la correspondance exacte epoch <-> surfaces, pas par
168/336. Aucun run n'a ete fusionne ; aucun fichier deplace.

## Consequence pour le denominateur (voir §3 du statut)

Deux denominateurs sont possibles et **le choix est une decision de protocole,
pas de code** :
- **D1** toutes les cibles du manifeste (336) -> couverture 50 % ->
  `INCOMPLETE_COVERAGE` ;
- **D2** les cibles de l'epoch soumise a l'analyse de role (168) -> couverture
  100 %.

D1 est le **choix conservateur**, et c'est celui que le code applique
desormais. D2 exigerait d'etablir dans la source du producteur QUE l'analyse de
role est par conception restreinte a un epoch -- ce que je n'ai pas lu.
