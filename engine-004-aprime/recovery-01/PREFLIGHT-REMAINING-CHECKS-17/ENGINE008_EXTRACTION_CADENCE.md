# ENGINE008_EXTRACTION_CADENCE — combien d'extractions par appel

## Le site unique d'extraction, verbatim

`src/llama-context.cpp:1783-1788`, dans `process_ubatch` :

```cpp
const auto status = graph_compute(res->get_gf(), ubatch.n_tokens > 1);
// [F1A-CAPTURE] fenetre de duree de vie : synchronisation puis extraction, AVANT
// toute reutilisation possible des buffers du planificateur par le tour suivant.
if (f1a_capture_active(f1a_capture_get_ctx())) {
    // R5-b : le scheduler REEL est transmis, pas un backend d indice suppose.
    f1a_capture_end_epoch(f1a_capture_get_ctx(), nullptr, sched.get());
}
```

**Une extraction par appel a `process_ubatch`, donc par UBATCH** -- et non par
appel de `llama_decode`. Et `f1a_capture_end_epoch` incremente `cc->epoch`
(`f1a-capture.cpp:839`) et ecrit ses trois fichiers en `"w"`.

## La borne, depuis la source

`MAX_EXTRACTIONS_PER_CALL_SOURCE_BOUND` = **le nombre d'ubatches traites par
l'appel**, chacun donnant exactement une extraction. Ce n'est pas une constante :
c'est une fonction de la segmentation.

Pour la configuration EXACTE du plan (`n_ubatch=512`, prompt court, decode d'un
token) : le prompt tient dans **1 ubatch** et chaque pas de decode est **1
ubatch** -> **1 extraction par appel**. C'est l'attendu, mais ce n'est pas une
garantie : la segmentation depend de `ubatch` construit par `llama_decode_impl`.

**Je ne change NI le batching, NI les CUDA graphs, NI la configuration** pour
faire passer cette preuve.

## L'observateur existe deja, sans modifier le calcul

**`PER_EXTRACTION_OBSERVER_AVAILABLE_WITHOUT_COMPUTE_CHANGE=YES`.**

`f1a_capture_end_epoch` emet les evenements `CAPTURE_EXTRACT` et `CAPTURE_END`
(`f1a-capture.cpp:816-825`). Le ledger :
- est ouvert en **mode AJOUT** (`fopen(...,"a")`, `:173`) ;
- est **`fflush` apres CHAQUE evenement** (`:220`) ;
- est donc **lisible immediatement**, en ajout, sans rien changer au calcul.

**Compter les lignes `CAPTURE_EXTRACT` du ledger donne le nombre d'extractions.**
Le harnais le fait par fenetre (`d.extraction_count`, delta avant/apres) et
publie `wht_max_extractions_per_window`.

**Ce n'est PAS un polling temporel** : c'est un compteur d'evenements ecrits par
le producteur lui-meme, dans un fichier en ajout avec flush. Et ce n'est pas un
callback : **aucun chemin CUDA n'est touche**.

## D1 : faisabilite, et sa condition

`D1_REQUIRED_COVERAGE_OBTAINABLE=YES` -- **sous condition observable**.

- Si **chaque fenetre contient exactement 1 extraction**, la collecte par fenetre
  lit l'analyse de cette extraction **avant** que la suivante ne l'ecrase -> D1
  est obtenable.
- Si une fenetre contient **plusieurs** extractions, les precedentes sont
  ecrasees avant lecture -> **D1 n'est PAS obtenable** pour cette fenetre.

Le harnais **detecte** le cas (delta du compteur d'evenements) et publie la limite
`WHT_MULTIPLE_EXTRACTIONS_IN_WINDOW`. Il ne suppose donc pas la borne : il la
**verifie** a chaque fenetre.

**Ce qui reste hors de portee sans modifier Q1** : *recuperer* l'analyse d'une
extraction ecrasee. Le ledger dit qu'elle a eu lieu, il ne dit pas ce qu'elle
contenait. Si un run montre >1 extraction par fenetre, D1 devra etre declare non
obtenable -- et **c'est un resultat, pas un echec**.
