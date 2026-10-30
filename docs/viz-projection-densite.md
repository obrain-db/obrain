# Viz obrain — besoins de projection & sens de représentation

Analyse (2026-09-09) : « densifier à la vitesse d'un slide cursor » =
l'interaction doit être une **loupe multi-échelle continue**, pas une série
de requêtes lourdes. Documente les besoins, le sens des projections, et les
conséquences de conception sur le pipeline serveur + client.

## 1. Les trois échelles et ce que chacune projette

| Échelle | Objet projeté | Projection = quoi ? | Sens |
|---|---|---|---|
| **Macro (Monde/galaxie)** | store entier | *statistique* : 1 point = 1 item, coquilles par type, marqueurs modules | répartition : « où vit quoi » ; pas de topologie lisible à cette échelle |
| **Meso (Graphe)** | voisinage autour de centres | *topologique* : BFS à hops fixes + budget nœuds | proximité structurelle = rayon depuis le centre |
| **Micro (focus)** | un nœud et ses strates | *relationnelle* : parents/enfants, cliques K3/K4, hyperarêtes, ∂ contour | ce qui entoure, appelle, est appelé par la cible |

La densification demandée = glisser entre meso et micro (et de meso vers
macro) **sans cassure** : mêmes centres conservés, le rayon augmente, la
projection s'épaissit (plus de niveaux, de strates, de nœuds).

## 2. Sens de représentation (que veut dire une position/une arête ?)

1. **Directionnalité du code** : une arête CALLS a un sens (appelant →
   appelé). Convention à rendre explicite et stable :
   - *sortantes* = ce que le nœud fait (callees), *entrantes* = qui l'utilise
     (callers). Les deux doivent être lisibles (orientation/contour), pas
     seulement le rayon BFS non dirigé.
   - inbound = « amont/dépendances », outbound = « aval/effets ».
2. **Profondeur ≠ distance euclidienne** : le rayon visuel encode la
   distance structurelle (sauts BFS), jamais la position dans le fichier.
   Un centre = point d'ancrage ; densifier = ajouter des anneaux **autour de
   la même ancre** (pas un re-layout complet).
3. **Flux temporel (stores runtime/cognitifs)** : Session → Message →
   ToolCall a un sens chronologique. La projection doit pouvoir basculer
   d'un « espace des voisinages » à un « axe du temps » (topologie linéaire
   par session) quand le store est cognitif.
4. **Poids et signaux** : energy/synapses = « densité d'intérêt » ; ricci<0
   = « bords » structurels (∂ contour). Ce sont des **méta-projections** qui
   doivent piloter l'échantillonnage (voir §4), pas seulement la couleur.
5. **Légende stable** : couleur = type ; taille/halo = énergie/engram ;
   strates K3/K4/hyper = motifs locaux (faces visibles seulement si le
   budget d'affichage le permet).

## 3. « Slide cursor » = exigences fonctionnelles

- **Incrémentalité** : densifier ne doit pas re-télécharger/re-layout le
  graphe entier. Serveur : endpoint « expansion du frontière » (delta :
  nouveaux nœuds + nouvelles arêtes, annexes du graphe courant) au lieu de
  la replanche complète. Client : merge + layout local conservant les
  positions déjà posées (les anneaux intérieurs ne bougent pas).
- **Réactivité** : un tick de curseur ≤ 100 ms serveur, ≤ 1 frame de rendu ;
  les requêtes identiques (retour arrière du curseur) = cache ~0 ms.
- **Dégradation douce, jamais d'erreur dure** : quand la demande dépasse un
  budget (nœuds/profondeur), répondre **tronqué + `truncated: true`** avec
  un indicateur « continuer ? » — l'utilisateur densifie plus, il ne tombe
  pas sur un refus (corrigé 2026-09-09 : la closure `hops=0` était une
  erreur 400 au-delà de 1500 nœuds ; désormais clamp à 20 k + profondeur
  bornée 16, réponse marquée TRONQUÉ).
- **Pré-chargement** : pendant un survol/hover, pré-expandre le voisinage
  immédiat du nœud survolé (la facette « parents/enfants » devient
  instantanée).

## 4. Échantillonnage : densifier ne veut pas dire tout montrer

Quand le budget nœuds (limit) est atteint, la sélection des nœuds gardés
doit être **structurelle**, pas aléatoire ni « premiers rencontrés » :
1. garder le centre et les hubs (degré/énergie les plus hauts) ;
2. répartir les anneaux : un plafond de nœuds par niveau de profondeur ;
3. conserver les « ponts » (articulations/SCC) qui relient les régions ;
4. jeter en priorité les feuilles redondantes (mêmes voisins, même module).

Conséquence serveur : l'extraction BFS doit produire un *échantillon
conscient* (frontière + priorité par signal), pas seulement une coupure
séquentielle. C'est ce qui rend la vue stable quand on glisse la limite.

## 5. Conséquences techniques (état & prochaines étapes)

Fait (2026-09-09) :
- expansion IN multi-seed indexée (×100-1000) → chaque niveau de densité
  coûte des ms même sur 4,8 M arêtes ;
- résolution des centres par nom indexé (fini les scans de 1,2-3,4 s) ;
- mémo résolution + cache sous-graphe 8 s côté serveur ;
- closure ∞ bornée (20 k nœuds / 16 niveaux, jamais en erreur).

À faire (par ordre de valeur) :
1. **Expansion delta** serveur (`/api/viz/expand` : frontier-only) + merge
   client avec layout incrémental (positions conservées) — le vrai « slide ».
2. Layout BFS client en O(E) (aujourd'hui O(n²) par scan de tous les nœuds
   à chaque étape → illisible au-delà de ~5-8 k nœuds).
3. Échantillonnage structurel (hubs d'abord, ponts, plafond par anneau).
4. LOD « faces » : K3/K4 par distance caméra (le densité LOD actuel ne gère
   que visibilité) + budget triangles.
5. Axe du temps pour les stores cognitifs (topologie linéaire par session).
6. Pré-chargement hover du voisinage.

Le sens de toute cette couche : un modèle 3D où **l'espace = structure,
l'échelle = profondeur explorée, et le mouvement du curseur = la
profondeur de lecture** — pas un scrapbook de nœuds.
