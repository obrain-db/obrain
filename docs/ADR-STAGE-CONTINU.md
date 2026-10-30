# ADR — Stage continu : une seule projection à échelles continues

Date : 2026-09-09 · Décision : l'utilisateur confirme « Cosmos = même continuité
de projection à des échelles différentes ».

## Règle (corrigée après retour utilisateur)
**AUCUN changement de page ni de projection.** Tout le multi-projet est
DÉPLIÉ en permanence dans un seul espace continu 3D :
- toutes les bases (projets/runtime) sont visibles simultanément, chacune
  étant un objet spatial (galaxie) déjà ouvert ;
- se déplacer d'une échelle à l'autre = **voyager avec la caméra** dans cet
  univers ; on entre DANS une galaxie pour voir ses items, on continue pour
  voir son sous-graphe — jamais un composant qui remplace l'autre ;
- les échelles ne sont pas des pages : ce sont des DISTANCES de caméra.

## Implémentation visée (stage unique)
1. Une seule `<Scene>` Three.js qui contient, à des coordonnées gigognes :
   - les « galaxies-bases » (positions par similarité réelle) ;
   - chaque galaxie = sphère de points (données `/api/viz/signatures`) ;
   - le sous-graphe courant, placé au centre de sa galaxie ;
2. Zoom continu : rayons, tailles et LOD pilotés par la distance caméra ;
3. Données streamées/composées : les signatures de toutes les bases sont
   gardées en mémoire (une requête), les niveaux suivants chargés à la demande
   (streaming JSON par chunk) → optimise les requêtes à obrain ;
4. Fin de course : une feuille s'affiche transposée dans le Monde sans changer
   de vue (même scène).

## Statut
Accepté → priorité 1 du backlog (remplace « Canvas unifié »).

## Spec v0 (prochaine itération)
1. Une seule `<StageContinu>` Three.js toujours montée (aucun swap) ;
2. toutes les bases rendues en parallèle : galaxie/bulle par base
   (échantillon de points ∝ nœuds, couleur cognitif/code), arcs de similarité
   réels entre bulles, labels ;
3. voyage caméra continu : zoom → entrer dans une bulle (ses items) → sous-graphe
   au centre de la bulle ; bouton « échelle » = marqueur de position, jamais
   un sélecteur d'écran ;
4. l'ancien CosmosView (SVG + swaps) est RETIRÉ.
