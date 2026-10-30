# ENGINE008_WHT_CHAIN_AUDIT (B1)

**Methode :** lecture des sources Q1 `87c42d5f…`. Aucun modele charge.

## 1. La chaine exigeer, etat par etat

| Maillon | Etat | Preuve |
|---|---|---|
| Activateurs f1a | **RACCORDE** | `prepare_capture` pose `OBRAIN_F1A_CAPTURE/OUTDIR/RUN_ID/INVENTORY` **apres** l'autorisation |
| Capture native | produite par Q1 | `f1a-capture.cpp` |
| **`EVENT_LEDGER.csv`** | **RACCORDE** | parseur + curseur + correlation, testes (42 controles) |
| **`SUBMITTED_GRAPH_SNAPSHOTS.csv`** | **RACCORDE** | parseur + confrontation de fingerprint, testes |
| **`GRAPH_ROLE_RELATIONSHIPS.csv`** | **NON RACCORDE** | produit (`:671`) mais **aucun parseur** |
| **`TARGET_EFFECT_PATH.csv`** | **NON RACCORDE** | produit (`:675`) mais **aucun parseur** |
| **`RESIDENCY_MANIFEST.csv`** | **NON RACCORDE** | produit (`:628`) mais **aucun parseur** |
| **`rotation_applied`** | **NON RACCORDE** | colonne ecrite (`:590`, valeur `:473`) mais **jamais lue** |
| Couverture / attribution | **RACCORDE** (partiel) | couverture des evenements et des snapshots ; **PAS** la couverture des cibles WHT |
| Rapport serialise | **RACCORDE** | porte les champs de correlation et de fingerprint ; **PAS** de verdict WHT |

**WHT_REAL_COLLECTION_CONNECTED=NO.** La chaine ledger+snapshots est raccordee et
testee ; la chaine WHT **ne l'est pas** : les surfaces qui portent la rotation et
les relations role/effet ne sont ni parsees ni reliees au pipeline, et
`rotation_applied` n'est lu nulle part.

## 2. Criteres d'eligibilite du producteur (verbatim)

```cpp
// f1a-capture.cpp:5
// INVENTAIRE: f1a_eligible() exige le TYPE TQ2_G128 ET le NOM exact present dans le CSV
```

Un tenseur n'est candidat que si **deux** conditions tiennent :
1. son type est **`TQ2_G128`** ;
2. son nom figure **exactement** dans le CSV d'inventaire.

**Consequence pour les deux modeles, et c'est la mise en garde de la consigne :**

| Modele | Type de quantification | Cibles eligibles |
|---|---|---|
| baseline `Qwen3.8-27B-Q4_K_M.gguf` | **Q4_K_M** (pas TQ2_G128) | **AUCUNE** |
| candidat `Qwen3.8-27B-OBRAIN-v3.gguf` | doit porter TQ2_G128 (a confirmer au run) | non nul attendu |

La baseline **ne peut pas** etre capturee comme le candidat. Sa couverture
attendue est **zero**, et **zero cible capturee reste `NOT_CONFIRMED`, jamais une
preuve d'absence**. Donc :

**BASELINE_WHT_NEGATIVE_COVERAGE_ASSESSABLE=NO**

## 3. La rotation est STRUCTURELLEMENT impossible sur cet hote

```cpp
// llama-model.cpp:472,478,481
rotation = get_il_eff(il) % ud->n_devices;
rotation = hparams.n_layer() % ud->n_devices;
```

La rotation est un **modulo par le nombre de devices**. Or l'hote expose :

```
$ nvidia-smi -L
GPU 0: NVIDIA GB10 (UUID: GPU-48000ac5-...)
```

**UN SEUL device.** Donc `n_devices == 1` et **`x % 1 == 0` toujours**. La valeur
de rotation est **toujours nulle**, et `r.rotation_applied = (cur_raw != cur_used)`
(`:473`) **ne peut pas devenir vrai** par ce mecanisme.

**CANDIDATE_WHT_POSITIVE_PROOF_ASSESSABLE=NO**

C'est un fait structurel, etabli par lecture croisee du code de rotation et de la
topologie de la machine -- pas une difficulte d'execution.

## 4. Consequence sur le gel

Le protocole v2 exige P3 (rotation candidate observee) comme **preuve positive** et
P4 (propriete negative sur la baseline) avec **couverture non nulle**. Or :

- P3 : la rotation ne peut pas se produire sur un hote a un seul device ;
- P4 : la baseline n'a aucune cible eligible, donc aucune couverture possible.

Les deux proprietes WHT du contrat **ne sont pas evaluables avec le producteur
existant sur cet hote**. Conformement a la consigne -- « si une propriete est
impossible a etablir avec le producteur existant, nommer le manque et ne pas
emettre le gel de disponibilite » -- **le gel reste REFUSE**, et ce n'est plus un
travail a faire mais une condition de faisabilite a lever.

Deux voies, aucune choisie ici :
- **V-a** : executer les runs sur un hote a **>= 2 devices**, ou la rotation
  devient possible (`% 2`). Le harnais est deja ecrit pour cela.
- **V-b** : **reviser le protocole v2** pour retirer P3/P4 comme preuves requises
  et les remplacer par une propriete evaluable. Cela exige une version v3
  explicite, non un affaiblissement silencieux.
