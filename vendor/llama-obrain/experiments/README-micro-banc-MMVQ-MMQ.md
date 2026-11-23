# Micro-banc MMVQ / MMQ — trois builds, protocole et pieges

But : savoir si la frontiere `MMVQ_MAX_BATCH_SIZE = 8` est une limite de **performance** ou de
**validite** du chemin vectoriel, aux **formes reelles de verification** et a **lots fixes**
(n = 7, 8, 9, 10, 12, 16), sans dependre du decodage speculatif pour produire les tailles.

Le bord est etabli end-to-end (voir §15 du document d'etat) : a dm=7 (lot 8) la verification coute
**30,15 ms** par jeton commis, a dm=9 (lot 10) elle en coute **67,04** — et le debit speculateur
retombe a R ≈ 1,00. Ce micro-banc cherche le pourquoi, kernel par kernel.

## Les trois binaires, et ce qui les distingue vraiment

| build | source | drapeau de compilation | ce qu'il donne |
|---|---|---|---|
| **A** nominal | arbre courant | aucun | MMVQ pour n <= 8, MMQ au-dela |
| **B** MMQ force | + `experiments/force-mmq.py` | `-DGGML_CUDA_FORCE_MMQ=ON` | MMQ pour n = 7..16 |
| **C** MMVQ etendu | + `experiments/mmvq-wide.py` | aucun | MMVQ pour n = 7..16 |

**Deux pieges verifies dans le code, pas supposes :**

1. **`GGML_CUDA_FORCE_MMQ` seul ne force pas MMQ a n <= 8.** Le dispatch teste d'abord
   `ne2 <= MMVQ_MAX_BATCH_SIZE` et la branche vectorielle rend la main avant d'atteindre
   `ggml_cuda_should_use_mmq`, seule fonction ou ce drapeau agit (`mmq.cu`). Un build « FORCE_MMQ »
   non modifie serait donc **identique a A** dans la plage mesuree. D'ou l'abaissement de la
   constante a 6 dans B.
2. **Relever la constante ne suffit pas a etendre MMVQ.** Trois bornes supplementaires :
   `static_assert(MMVQ_MAX_BATCH_SIZE == MMVF_MAX_BATCH_SIZE)` (ggml-cuda.cu:1794) oblige `mmvf.cuh`
   a suivre ; et les dispatchs `mul_mat_vec_q_switch_ncols_dst` (mmvq.cu) et
   `mul_mat_vec_f_cuda_switch_ncols_dst` (mmvf.cu) ont `case 1..8` puis `default: GGML_ABORT` —
   relever la constante sans ajouter les cas ferait **planter** le binaire. C les ajoute, en les
   restreignant aux types que la verification utilise reellement (inventaire du GGUF v3 :
   TQ2_G128 168 tenseurs, Q4_K 176, Q5_K 65, Q6_K 1) pour ne pas instancier 8 cas de plus pour les
   ~25 autres types.

**Les trois builds doivent vivre dans trois repertoires separes**, jamais recycler le meme avec des
drapeaux differents : une recompilation partielle ou un objet en cache est exactement le genre de
contamination silencieuse que ce protocole cherche a eliminer. A consigner pour chacun : commit
source, drapeaux, **hash du binaire**, horodatage, GPU/pilote, et etat prod/load au moment de la
mesure.

## Ce qu'on mesure

Pour chaque couple **forme x n** (formes : `gate/up 17408x5120`, `down 5120x17408`, `q_proj`,
`k/v`, `o_proj`) :

* `T_MMVQ`, `T_MMQ`, rapport `MMQ/MMVQ`, debit effectif (Go/s) ;
* ressources de l'instantiation MMVQ : registres/fil, shared statique et dynamique, local memory,
  spills, fils/bloc, occupancy theorique et obtenue si accessible (`ptxas -v` + calcul d'occupancy) ;
* **justesse AVANT performance** : pour n = 9, 10, 12, 16, memes entrees dans les deux chemins,
  `max |Y_MMVQ - Y_MMQ|` et `||Y_MMVQ - Y_MMQ||_2 / ||Y_MMQ||_2`. Une petite difference compatible
  avec des noyaux quantifies est acceptable ; une anomalie qui apparait **brutalement apres n = 8**
  ne l'est pas. Le mode `test` de `test-backend-ops` compare deja chaque chemin a la reference CPU et
  rapporte l'erreur — c'est la borne superieure honnete de l'ecart entre les deux chemins.

Les cas de perf du micro-banc sont dans `tests/test-backend-ops.cpp` (bloc `MICRO-BANC MMVQ / MMQ`),
a lots fixes : `-o MUL_MAT -p tq2_g128` en mode `perf` pour le temps, en mode `test` pour la justesse.

## Decision attendue

| cas | observation | suite |
|---|---|---|
| **A** | MMVQ correct et nettement plus rapide jusqu'a 16 | extension du domaine : patch candidat |
| **B** | MMVQ gagne a 9/10 ou 9/12, puis perd | **le meilleur resultat** : domaine intermediaire, MMQ pour les gros lots |
| **C** | MMVQ s'effondre des 9, spills/occupancy mauvais, ou justesse cassee | ne pas relever la constante : chercher un troisieme noyau pour 9-16 |

**Validation end-to-end seulement apres** : si C montre un domaine gagnant, comparer A et C dans une
fenetre propre sur base / dm7 / dm9 / dm10 (idealement dm12), et verifier que les ~67-69 ms de
verification redescendent vers ~30 ms **et** que R repasse franchement au-dessus de 1. Amelioration
micro-kernel et amelioration du moteur se demontrent **separement**.

Le n-gram attend : il produira des fenetres plus longues, mais il faut d'abord savoir si le moteur
sait les verifier.
