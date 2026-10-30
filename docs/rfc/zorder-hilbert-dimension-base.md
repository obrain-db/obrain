# Z-order ↔ Hilbert : que ferait « changer la base de dimension » ?

**Date** : 2026-09-05 — branche `feat/zorder`. Document de conception — réponse
à « dans la logique du lien Z↔Hilbert, qu'est-ce qui changerait la base de
dimension ? ». État réel : plan **2D (centrality × out-degree)** (Q0.16/Q1.15),
découpé **par communautés** (ranges), clés par interleave Z
(`hilbert::zorder_key_2d(x,y,order)`), « superposition » = empilement de
résolutions/localités (zgrid/zruns, remap des slots, fenêtres 2D exact==sorted
et hubs en zruns vs scan).

« Changer la base de dimension » = changer les AXES et/ou la TRANSFORMÉE qui
projettent les nœuds dans l'espace avant encodage par la courbe. Tout ce qui
suit garde les contraintes du plan (additif, opt-in, format stable, benchs).

## 1. Changer les AXES (le plus simple, gros effet)

| Proposition | Concept | Nature | Coût/format |
|---|---|---|---|
| Axe **énergie** (C3) à la place/à côté de la centralité | fenêtres « activité récente × degré » | énergie = mémoire à décroissance | colonnes déjà là ; nouvel appel fenêtre (additif) |
| Axe **courbure Ricci κ** (par nœud) | « degré × κ » : localité vs couplage | la courbure « définit la métrique » (comme en relativité : la masse plie l'espace) | κ persisté (enrich) ; axes continus quantifiables |
| Axe **temps** (3e dimension) | (temps × degré) : fenêtres glissantes | univers : « événement » = point d'espace-temps | courbes 3D (Hilbert/Morton 3D) — colonnes temps déjà (CDC) ; opt-in |
| Axe **spectral/HKS** (une échelle) | rôles structurels proches dans l'espace-courbe | ondes : changer vers la base de Fourier locale | HKS à la demande ; axes réels |
| Axe **file_path / profondeur** (code) | voisinage par modules dans la courbe | hiérarchie | prop existante |

## 2. Changer la TRANSFORMÉE des coordonnées (changement de base au sens propre)

1. **Quantiles égaux par axe (bases adaptatives)** : au lieu de Q0.16 brut,
   mapper chaque axe sur ses quantiles (binning équiprobable) → la courbe code
   le RANG, pas la valeur : localité robuste aux distributions en loi de
   puissance (degrés). Nécessite une passe stats par store (additive, stable
   par version du store). Nature : perception (la rétine code des contrastes,
   pas des intensités absolues).
2. **Axes issus d'un plongement local (eigenmaps/PCA par communauté)** :
   « frame field » tourné par communauté (comme un repère mobile en géométrie
   différentielle) : chaque communauté code ses nœuds dans SA base propre →
   fenêtres intra-communauté plus denses. Coût : une base de rotation par
   communauté (petite matrice), range stable.
3. **Base spectrale multi-échelles (style ondelettes)** : la « superposition »
   actuelle Z-sur-Hilbert est déjà une vue multi-résolution ; la généraliser en
   **pyramide** (résumés coarse par niveau) permettrait coarse-to-fine (lié au
   comparatif N-variantes multi-résolution déjà proposé).

## 3. Changer la COURBE (encodage 1D de la même base)

- **Hilbert vs Z** : Z (interleave simple) est rapide mais moins « lisse » ;
  Hilbert minimise les sauts de localité mais coûte des tables d'ordre. Notre
  superposition Z-sur-Hilbert cherche à cumuler les deux. Alternatives qui
  « changent la base » : **Peano (base 3)** (anisotropie), **Gray-code**,
  **twin curves** (deux courbes orthogonales entrecroisées, comme un damier)
  pour réduire l'anisotropie directionnelle.
- **Débits multi-échelles** : entrelacement par bitplanes inversés (BDD) pour
  des préfixes = granularités.

## 4. Les limites physiques à garder en tête

- La localité d'une courbe se dégrade avec la dimension d (locality gap ∝
  dimension ; « malédiction de la dimension ») : privilégier 2-3 AXES
  SÉMANTIQUES forts plutôt que d'augmenter naïvement la dimension ;
- courbes n-D (Hilbert 3D+) = coût d'état exponentiel en d ; Morton n-D est
  bon marché mais moins local ; compromis : **une courbe par paires d'axes**
  (2D × 2D) + clés composites (comme des dimensions « éclatées ») ;
- tout changement d'axes transforme les clés → **versionner** le schéma
  (chiffres/bench avant-après, opt-in, pas de déploiement prod), et garder les
  anciens chemins lisibles.

## 5. Expérience réalisée 2026-09-05 (code-all3 réel, n3)

**Plancher zruns ≈ 40-50 µs** (indépendant du résultat) ; coût scan ∝ taille
de communauté (+filtre). Grande communauté (19 653 nœuds, cid 43292) :

| d_lo ≥ | résultat | scan | zruns | gain |
|---|---|---|---|---|
| 1 | 15 331 | 1112,8 µs | 1069,6 µs | 1,04× |
| 10 | 5 745 | 547,7 | 447,1 | 1,23× |
| 20 | 1 539 | 314,5 | 153,0 | **2,06×** |
| 50 | 181 | 239,0 | 70,4 | **3,40×** |
| 100 | 39 | 276,7 | 55,4 | **5,00×** |
| 300+ | ≤3 | ~230 | ~50 | ~4,7× |

Petites communautés (≤ ~1 500 nœuds) : le SCAN bat zruns (2-17 µs vs ~40 µs
de plancher, exact==sorted ✓ partout). → **croisement** quand scan ≳ plancher
(communautés de quelques milliers de nœuds).

## 5bis. Chantier issu de la mesure : sélecteur ADAPTATIF scan/zruns

Décision O(1) par communauté (taille ou scan estimé) → scan sous le seuil,
zruns au-dessus : meilleur des deux régimes (analogie : saccades — test
bon marché puis branche). Sémantique inchangée (les deux voies sont
exact==sorted vérifié) — purement un choix de chemin. DoD : bench daté du
balayage (dont tableaux ci-dessus), seuil calibré, tests exact==sorted.

Les autres « changements de base » (axes énergie/κ/temps/quantiles, frames
par communauté) restent à prototyper ensuite — cette mesure établit la
métrique de gain (gap zruns-vs-scan) qui départagera chaque axe candidat.


## 5ter. Protocole « axes candidats » — profil de localité par QUANTILE (mesure)

Protocole rejouable : grande communauté, pour chaque fraction f de la queue
supérieure on calcule le seuil degré d* (binaire sur la longueur) puis on
mesure scan vs zruns — profil de là où la localité paie le long de l'axe.
Résultat (code-all3, communauté 19 653 nœuds, n3) :

| fraction | d* | résultat | scan | zruns | gain zruns |
|---|---|---|---|---|---|
| 50 % | ≥6 | 9 665 | 806 µs | 848 µs | 0,95× |
| 25 % | ≥12 | 4 347 | 489 | 353 | 1,38× |
| 10 % | ≥19 | 1 747 | 376 | 157 | 2,39× |
| 5 % | ≥24 | 970 | 297 | 120 | 2,48× |
| 2 % | ≥35 | 393 | 260 | 60 | **4,31×** |
| 0,5 % | ≥71 | 95 | 241 | 35 | **6,91×** |

Lecture : l'axe degré brut est très asymétrique — à bas seuil les fenêtres
couvrent la communauté (peu de localité, scan≈zruns) ; la localité explose
dans la queue (×4-7 au-delà de 2 %). Un axe en QUANTILES (fenêtres = parts
égales de la population) maintiendrait les fenêtres dans la zone à haut gain
zruns et aplatirait le profil. Axes alternatifs réels (énergie, κ) :
nécessitent colonne + relayout du plan (chantier stockage) — ce profil sert
de métrique de départage avant tout chantier d'axe.
## 6. Lien avec les chantiers en cours

- L'axe temps/CDC et le diff incrémental « rétine » : courbes 2D + horodatage
  = fenêtres temporelles (window_since) — premier usage « base de dimension
  espace-temps » ;
- `search_by_role` (HKS) et la courbure donnent des axes « naturels » déjà
  calculables → le changement de base le plus « physique » serait **localité
  définie par la métrique κ** (comme en relativité générale : la géométrie
  locale définit le repère).
