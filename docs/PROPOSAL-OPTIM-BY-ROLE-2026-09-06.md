# Proposition — optimisation `search_by_role` (776 ms → cible < 100 ms)

**Date** : 2026-09-06 — HEAD `481dabb4` — constat des tests bout-en-bout :
`search_by_role(2,5)` = **776 ms** sur le store dev réel (RO) vs 0,2-16 ms pour
les autres formes. C'est le seul appel non interactif.

## 1. Où va le temps (analyse de code)

`search_by_role` → `compute_hks_descriptors` (geometric.rs) :
1. `CsrAdjacency::build` — O(V+E) mmap, ~ms sur ce graphe (~145 k nœuds) ;
2. **3 × `heat_kernel_signature_with_csr`** (t_local, t_meso, t_global=10) —
   chacune = `HKS_HUTCHINSON_PROBES = 32` vecteurs Rademacher, chaque vecteur
   diffusé en `n_steps = t/HKS_DT` pas sur O(E) — **séquentiel** ;
3. coût ≈ 32 × (pas_local+pas_meso+pas_global) × E. Avec t_global dominant
   (~200 pas si HKS_DT≈0,05) : ≈ 32 × ~230 × 270 k ≈ **~1,7-2 G opérations** →
   ~0,7-0,8 s mesuré. La distance 3D + re-rank clustering est négligeable.

## 2. Leviers (par ordre de gain / risque)

- **A. Paralléliser les 32 probes (rayon)** — les diffusions par probe sont
  INDÉPENDANTES (lectures CSR partagées). Machine 16 cœurs → ×8-16 attendu :
  **776 ms → ~50-100 ms**. Déterminisme : chaque probe garde son seed Xorshift
  fixe, accumulation dans des slots par index puis somme séquentielle dans
  l'ordre (résultat bit-identique quel que soit l'ordonnancement rayon).
  Vérifier la dépendance rayon dans obrain-substrate (déjà évoqué en commentaire).
- **B. Réutiliser le châînage des échelles PAR probe** : diffuser une seule
  fois par probe en enregistrant la diagonale à t_local puis t_meso puis
  t_global (au lieu de 3 diffusions indépendantes) — économise les redémarrages
  (gain réel seulement si les petites échelles ne dominent pas ; à mesurer).
- **C. Réduire les probes pour meso/global** (32 → 12-16) : bruit Hutchinson
  ↑ (±0,18 → ~±0,3) — les tests by_role (marge twin vs étoile 0,02) doivent
  être re-calibrés si ce levier est pris seul ; à coupler avec A.
- **D. Cache par (store, mtime)** des descripteurs HKS : premier appel ~cout,
  répétitions « même rôle » (usage agent) → **< 5 ms** ; invalidation mtime
  (même patron que les empreintes `compare`).
- **E. Filtre `label` / scope** déjà disponible pour restreindre les candidats.

## 3. DoD (8 cases)

1. Bench de décomposition daté : CSR vs diffusion (par échelle) au HEAD —
   la cible 100 ms suppose A validé mesuré ;
2. A (parallélisation probes déterministe) implémenté dans geometric.rs —
   résultats bit-identiques au séquentiel (test d'égalité stricte) ;
3. B si rentable mesuré ; C seulement avec re-calibrage des tests ;
4. D (cache par store+mtime) si le budget le permet (gain usage répété) ;
5. Suites : engine 680/724, core 870, substrate 807, topk/remap, formes —
   0 rouge ;
6. Bench daté : by_role sur runtime-dev **avant 776 ms → après cible < 100 ms**
   (+ répété < 5 ms si D) ; zéro impact autres chemins (aucun hook) ;
7. Commit unique réversible + ledger (chiffres) ;
8. Pas de déploiement prod sans demande (binaire déjà à jour — ce chantier
   enrichira le prochain build).

## 4. Risques

- Déterminisme flottant sous rayon (résolu par slots ordonnés — à prouver par
  un test d'égalité stricte vs séquentiel sur plusieurs seeds) ;
- rayon absent de obrain-substrate → ajout de dépendance (à confirmer) ou
  threads std manuels ;
- la variance Hutchinson ne change pas (mêmes probes, même ordre) → aucune
  re-calibration des seuils de test nécessaire pour A.

## 5. Ordre d'exécution (après validation)

1. bench de décomposition → 2. A + test d'égalité stricte → 3. mesurer →
   B/C/D selon résultats → 4. suites + bench daté → 5. ledger + commit →
   6. (option) re-bascule binaire prod/dev avec toi.

## 6. RÉVISION — cible **~1 ms**, pas 100 ms (architecture enrich-then-scan)

Le parallélisme (A) plafonne ~50-100 ms : ×50 du compte. Pour ~1 ms sur un
graphe de 145 k nœuds, il faut SORTIR le HKS du chemin de requête :

- **`obrain enrich --hks`** (CLI Rust, additif — miroir du pattern `enrich
  --ricci` puis `compare` à empreintes) : calcule `compute_hks_descriptors`
  UNE fois par store et persiste les 3 flottants par nœud (3 propriétés
  scalaires `hks_local/meso/global`, ou colonne f32 dédiée) ; rejouable par
  mtime/epoch (invalidation comme les empreintes `compare`).
- **`search_by_role` redevient un scan de colonnes** : lire l'ancre (O(1)) +
  balayer les descripteurs persistés (145 k × 3 f32 ≈ 1,7 Mo séquentiel mmap
  → **< 1 ms**) + top-k → **cible ≤ 2 ms** sur runtime-dev ; le filtre
  `label` restreint le scan.
- Aucun changement de format : les descripteurs sont des propriétés (comme
  energy/ricci) ; le HKS lourd ne s'exécute plus qu'à l'enrichissement
  (tolérable : 776 ms une fois + I/O).
- Généralisation « ~1 ms partout » : appliquer le même principe aux autres
  formes chaudes (delta/surprise/relief déjà en ms à chaud via by_ts ; delta
  à froid 114 ms = décodage de fenêtre → pagination/watermark + limite par
  défaut pour l'interactif).

## 7. DoD révisée (par-dessus §3)
1. `obrain enrich --hks` (persistance additive + invalidation mtime) ;
2. `search_by_role` = scan colonnes (élimination du HKS du chemin query) ;
3. Bench : by_role runtime-dev **776 ms → ≤ 2 ms** (froid après enrich) ;
   enrich --hks daté (~1 s) ; suites 0 rouge ; tests by_role inchangés
   (résultats identiques aux descripteurs HKS pré-calculés — test d'égalité
   vs calcul à la volée) ;
4. delta interactif : limite par défaut + watermark (à la suite).
