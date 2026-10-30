# Optimisation & structure inspirées de la nature (univers/biologique)

**Date** : 2026-09-05 — branche `feat/zorder`, HEAD à jour. L'obrain est déjà
profondément « naturel » ; ce document explicite les correspondances et les
leviers d'optimisation suivants, appliqués ou proposés.

## 1. Ce qu'obrain emprunte déjà au réel (cartes)

| Principe naturel | Mécanisme obrain (réel, dans le code) |
|---|---|
| **Courbure de l'espace** (relativité : masses qui déforment l'espace) | Courbure de Ricci-Ollivier par arête (bottleneck/core), Forman |
| **Chaleur / diffusion** (thermodynamique) | HKS 3 échelles (`compute_hks_descriptors`), `search_by_role` |
| **Mémoire associée au contexte** (hippocampe, co-activation) | Engrammes formés par co-activation gatée par la **surprise** (`prediction_error`), spectres hopfield |
| **Oubli/consolidation** (courbe de l'oubli, Ebbinghaus) | FSRS (stabilité/difficulté), décroissance d'énergie |
| **Réseaux de neurones / synapses** | colonnes synapses, énergie, `cognitive.spread` |
| **Localité cosmique / fil d'ordre spatial** (univers : amas, filaments) | Hilbert/Z-order + fenêtres 2D par communautés (window hubs/énergie) |
| **Rétine : ne transmettre QUE les changements** (détection de contraste) | CDC (capture de changement bornée), `search_delta`, invalidation par mtime |
| **Hiérarchies/échelles** (galaxies→étoiles ; cortex en colonnes) | multi-échelles HKS ; agrégation par label/compteurs O(1) |
| **Sparsité / épargne d'énergie** (cerveau : ~2 % actif) | calculs paresseux (hydratation différée), kill switches, fenêtres bornées |

## 2. Levier appliqué ce jour : empreintes « mémoire » par store (rétine locale)

Constat mesuré : 70 % du coût d'un `compare` = relire la propriété
`qualified_name` de CHAQUE nœud à chaque invocation, même quand rien n'a
changé — comme une rétine qui re-décoderait toute la scène à chaque image.

Correctif (principe « ne recalculer que ce qui change ») : fichier latéral
`substrate.compare-hash[.<prefix>]` = **résumé local** (hash FNV par slot,
en-tête hw/sans/live), **invalidé par mtime du store** (toute réécriture du
store le rend caduc → reconstruction). Résultats (machine n3) :
- paire 35-38 k nœuds/247 k arêtes : **80 ms → 39-40 ms** ;
- gros 443 k/1,67 M : **1,07-1,22 s → 391-432 ms** ;
- coût unitaire ~ **0,7-2 µs/nœud·arête** (masse), première passe = coût froid
  (~1 s gros) puis quasi-gratuit tant que le store ne change pas.
Additif : fichier latéral, aucun changement de format/zone, aucune écriture
dans le store.

## 3. Prochains leviers « nature » proposés (à valider avant code)

1. **Comparaison incrémentale type rétine** : maintenir un delta par version
   (empreinte + événements CDC) → `compare main vs N branches` = O(changements)
   au lieu de O(graphe) ; c'est le saut vers des **ms à répétition** sur
   n'importe quelle taille.
2. **Hiérarchie multi-résolution (pyramide)** pour la boucle N-variantes :
   résumés par label/fichier d'abord (coarse), descente fine seulement si le
   résumé diffère — coût logarithmique typique.
3. **Localité/Hubs** : pré-tri par hubs (degrés/κ) pour échantillonner et
   présenter les différences les plus structurantes en premier.
4. **Mémoire de travail persistante (hippocampe « replay »)** : garder les
   stores ouverts entre deux compares (caches de props hydratés) — le coût
   d'hydratation payé une seule fois pour toute la boucle.
5. **Sparsité d'écriture** : `enrich --ricci` et imports à repasser sur les
   mêmes principes (batchs, verrous uniques, résumés) pour viser la ms à
   l'échelle des variantes typiques.

Chaque levier garde les contraintes du plan : additif, format inchangé,
portes vertes, benchs datés, pas de déploiement prod.
