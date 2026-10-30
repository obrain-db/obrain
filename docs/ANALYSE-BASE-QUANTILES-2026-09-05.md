# Analyse — levier « base par quantiles » (fenêtres 2D à localité aplatie)

**Date** : 2026-09-05 — HEAD `3ceeb33b` — branche `feat/zorder`.
**Portée** : ANALYSE CODE (regarde le code réel) + design v1 du levier ① du RFC
`docs/rfc/zorder-hilbert-dimension-base.md` — aucune modification de code.
Prolongement direct du profil de localité mesuré (RFC §5ter : gain zruns ×4-7
dans la queue de l'axe degré, ≈1× dans la masse).

---

## 1. Carte du code (ce qui existe)

Plan 2D **centrality (Q0.16) × out-degree**, découpé par communautés (slots
contigus), **superposition** : Hilbert = ordre physique, Z-order = index logique
(préfixes quadtree). Pièces :

| Pièce | Où | Rôle |
|---|---|---|
| `grid_coords_from_features(centrality_q16, degree, order, max_degree)` | `crates/obrain-substrate/src/hilbert.rs` (~l.95-128) | **Point unique de définition du plan 2D** : rescale linéaire des deux axes vers la grille `[0,n)²` (n = 1<<order). Cité comme invariant : les deux courbes doivent parler de la MÊME grille. |
| `hilbert_key_from_features` / `zorder_key_from_features` | hilbert.rs (~l.70-190) | Clés physique/logique, construites depuis la rescale commune ; `zorder_key_2d` = interleave (l.153). |
| `zorder_window_ranges(cx_lo.., cy_lo.., order)` | hilbert.rs (~l.200+) | Décompose une fenêtre 2D en **runs Z contigus** (bornes de grille inclusives). |
| Zone `Zorder` + sidecar `substrate.zgrid` | store.rs `community_zrows` (~l.8060-8100) | Colonne zkey par slot (écrite au layout) + persistance `(order, max_degree)` du dernier bulk sort ; cache `CommunityZRows` par communauté invalidé par `zrows_gen`. |
| `window_query_2d` (scan exact) | store.rs ~l.8033 | Balaye la communauté, prédicat exact sur les valeurs courantes. |
| `window_query_2d_sorted` (zruns) | store.rs ~l.8134-8240 | Sélecteur ADAPTATIF (`OBRAIN_ZRUNS_MIN_COMMUNITY`, défaut **4500**) → scan si petite communauté ; `sorted_view_valid()` faux (features mutées depuis le tri) → scan ; fenêtre large (> ~⅓) → scan ; sinon runs Z + **re-vérification exacte** (`slot_matches_window`, ~l.7951) → jamais de faux positif. |
| `sorted_view_valid` / `feature_epoch` / `zrows_gen` | store.rs | Garantie de fraîcheur du « layout trié » : toute mutation de features (centralité/arêtes/communautés/tombstones) invalide → repli scan (toujours correct). |
| Entrée moteur/CALL | `crates/obrain-engine/src/database/window.rs` `window_query_2d_sorted` (~l.57) + plan.rs LECTURE | `CALL obrain.window_2d` / `window_2d_energy`. |

**Constat technique (dette utile au levier)** : le rescale fenêtre→grille est
DUPLIQUÉ sous forme de closures inline `gx`/`gy` dans `window_query_2d_sorted`
(l.~8150-8156) alors que le layout passe par `grid_coords_from_features`. Les
deux copies sont linéaires et cohérentes aujourd'hui ; toute transformation de
base (quantiles) doit passer par UNE seule fonction partagée, sous peine de
dérive silencieuse layout≠requête.

## 2. Constat clé (ce qui rend le levier propre)

1. **Monotonie** : toute transformation `degré → rang` (cumulative croissante)
   préserve l'ORDRE des degrés. L'API fenêtre actuelle prend des **bornes de
   degré brut** (`d_lo..d_hi`, ex. hubs d≥50) ; sous une base par quantiles,
   une bande brute se traduit en une bande de RANGS de même population — la
   SÉMANTIQUE des requêtes ne change pas.
2. **Exactitude indépendante de la base** : les zruns re-vérifient chaque
   candidat avec le prédicat exact sur les valeurs courantes, et
   `sorted_view_valid()` replie en scan dès que les features bougent. Une base
   par quantiles ne peut donc PAS rendre de résultat faux — elle ne change que
   le PROFIL DE COÛT (localité des clés). Exactement la philosophie du
   sélecteur adaptatif (exact==sorted vérifié partout).
3. **Le profil mesuré (RFC §5ter) est la spécification du gain** : l'axe brut
   est très asymétrique (0,95× à 50 % de population, ×4,3 à 2 %, ×6,9 à
   0,5 %). En base linéaire, une fenêtre de queue (d≥k) occupe peu de cellules
   `cy` seulement si `max_degree` est bien réglé ; le rescale par rangs rend
   **toute part de population proportionnelle à sa part de cellules** → les
   fenêtres de queue (le cas « hubs » qui intéresse) tombent toujours dans la
   zone à haut gain zruns, les fenêtres larges restent sous le bail scan.

## 3. Design v1 — « base par quantiles » (rangs), opt-in

Principe : remplacer la rescale linéaire de l'axe DEGRÉ par une rescale
**monotone par rangs** (CDF empirique du degré) ; l'axe centralité reste
linéaire dans une v1 (extension triviale ensuite).

- **Fonction de transformation** (nouvelle, dans hilbert.rs) :
  `grid_cy_from_features(degree, order, bins: &DegreeBins) -> u32` où
  `DegreeBins` = histogramme cumulé des degrés (degrés 0..max_effectif) ;
  `cy = F(degree)` mappé sur `[0,n)` par parts égales de population.
  `grid_coords_from_features` gagne une variante/paramètre (enum
  `AxisBase { Linear, Quantiles(DegreeBins) }`) — **une seule définition**,
  partagée par les deux courbes ET par la conversion fenêtre (refactor des
  closures `gx/gy` en helpers du module hilbert : `window_to_grid_*`).
- **Persistance** : nouveau sidecar optionnel `substrate.zquant`
  { epoch, max_degree_effectif, histogramme cumulé compact }. Écrit au moment
  du relayout (bulk sort) si le mode quantiles est demandé
  (`topo hilbert --base quantiles` ou env), lu par layout ET par
  `window_query_2d_sorted` ; **absence du sidecar = chemin linéaire
  (legacy intact)**. Additif, `SUBSTRATE_FORMAT_VERSION` inchangé, anciens
  binaires ignorant la zone inconnue.
- **Coût de la passe stats** : histogramme des degrés = lecture O(nœuds vivants)
  de la colonne degré (T5/T8) — ordre des ms sur 445 k (la re-vérification du
  comptage à l'ouverture est déjà O(n) et mesurée en ms) ; marginal vs le
  relayout (déjà O(n log n)).
- **Fraîcheur** : les bins sont un instantané du relayout ; la dérive post-
  layout est traitée par le mécanisme EXISTANT (`sorted_view_valid` → repli
  scan). Nouveaux nœuds entre deux relayouts : mapping approximatif (comme la
  saturation actuelle à `max_degree`) — correct, seulement moins local.
- **Fenêtres « population » (extension, plus tard)** : avec `F`, une API
  `q_lo/q_hi` (« les 5 % les plus connectés ») devient un simple encodage
  inverse — gratuit une fois `DegreeBins` en place.

## 4. DoD / bench (à exécuter après validation)

1. Propriété **exact==sorted** : 250 fenêtres aléatoires (protocole existant
   `09eb54eb`) rejouées en mode quantiles — égalité parfaite (scan vs zruns).
2. **Protocole §5ter rejoué** sur code-all3 (communauté 19 653) : profil
   fenêtres par fraction de population → attendu : fenêtres 50 % ≈ scan
   (inchangé), fenêtres 2 %/0,5 % dans la zone ×4-7 **dès le premier palier**,
   et surtout fenêtres à population intermédiaire (10-25 %) qui passent sous
   le bail scan moins souvent qu'en linéaire.
3. Comparaison **avant/après** sur les fenêtres hubs réelles (d≥20/50/100 :
   zruns vs scan) — le gain doit être ≥ le tableau linéaire à iso-population.
4. Portes : suites substrate/engine/core vertes, aucun test existant modifié
   (chemin linéaire par défaut), bench daté dans le ledger, commit réversible.
5. Refactor préalable (inclus) : unifier `grid_coords_from_features` et les
   `gx/gy` de `window_query_2d_sorted` — testé par l'égalité existante.

## 5. Risques / limites

- Bins globaux au store vs par communauté : choix v1 = **global** (même degré =
  même rang partout → fenêtres comparables entre communautés) ; une variante
  par communauté (rang intra-commu) est possible mais change la lecture des
  fenêtres — à documenter si jamais retenue.
- Drift distribution avant relayout : performance seulement (jamais exactitude).
- Le mode quantiles change les CLÉS (donc le layout physique) → opt-in + bench
  + rollback = relayout linéaire (zgrid existant). Pas de déploiement prod.
- Centralité en v1 linéaire : le tableau §5ter porte sur le degré ; refaire le
  même profil sur la centralité avant d'étendre (protocole identique).

## 6. Lien avec le levier ② (temps)

La même mécanique (bins + mapping monotone persisteé, invalidation par
relayout) prépare les **fenêtres temporelles** : axe temps = rang de
l'horodatage (CDC) — l'infrastructure `DegreeBins`/sidecar versionné est
réutilisable pour `TimeBins`. Ordre recommandé : ① puis ② (② = « 2D + filtre »
sans courbe 3D, cf. RFC §2-compromis).
