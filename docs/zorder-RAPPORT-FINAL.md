# Rapport final — superposition « ordre Z » + chantiers de relief (contrôle qualité/performance)

**Branche** : `feat/zorder` (depuis `62b9fe99`, HEAD consolidé de `fix/ch8b-engrams`) — **26 commits**, arbre propre.
**Contexte** : moteur graph obrain (workspace `/home/triviere/projects/banc-obrain`), services consommateurs : aletheia (binding NAPI `obrain-node`) et DSH harness graph-writer (moteur = binaire `obrain` en `bridge`/`mcp`).

---

## 1. Chaîne « ordre Z » — livrée et mesurée

### Analyse & conception
- RFC vivante : `docs/rfc/substrate/zorder-superposition.md` (v11) — propriétés comparées Hilbert vs Z, capacités C1 (fenêtres 2D) / C2 (clustering hiérarchique) / C3 (ordre physique Z), contraintes (stocks réels, runbook déploiement §9.4, constats §9.3/§9.5).

### Implémentation (moteur → binding)
- **D1 — Primitives** (`39b64211`) : `grid_coords_from_features` (grille partagée), `zorder_key_2d`/`zorder_key_from_features` (Morton), `zorder_window_ranges` (fenêtre → plages Z) + 20 tests unitaires.
- **D2 — Zone `Zorder`** (`36c3137f`) : `substrate.zorder` u32/slot, remplie aux passes de tri (bulk/compact), replay miroir, enregistrée (ALL_MMAPPED/checkpoint/advise) — additive, `SUBSTRATE_FORMAT_VERSION` inchangé, E2E étendu.
- **D4 — `SubstrateStore::window_query_2d`** (`6b2dd100`) : scan EXACT (valeurs courantes, toujours correct), tombstones/zéro-sentinelles exclus — baseline de référence.
- **D3b — Accélération zruns** (`7461bdec` + `e2bd535f`) : sidecar `substrate.zgrid` (persistance `order`+`max_degree` du tri), `window_query_2d_sorted` (index (zkey,slot) par communauté en cache, plages Z, prédicat exact, repli scan si layout non trié / grille absente / fenêtre large >⅓) — tests bornes/filtres/saturation.
- **Exposition** (`d1388681`) : `ObrainDB::window_query_2d` (engine) + NAPI `windowQuery2d` + `index.d.ts` + tests moteur.

### Benchmarks (machine n3, criterion ×30, seed fixe — consignés RFC §5.1)
- **B1 fill** : Z = **41–46 % du coût du hkey** (6,5–9 vs 14–21 ns/clé) → superposition quasi gratuite dans la passe existante.
- **B2 fenêtres (66 k)** : zruns gagne **1,7× (10 % d'aire) → 131× (0,01 %)** ; scan gagne au-delà (~crossover 10–50 %) ; runs/fenêtre 88→1 (quadtree conforme).
- **B3 localité** : Z **non dégradé** vs Hilbert sur écarts 4-voisins (mean 20,6 vs 28,3 ; max 1366 vs 3413).
- Ordres de grandeur cohérents avec les benches de référence du dépôt (spread depth-3 33 µs@100 k ; fenêtre-scan 22–32 µs@66 k ; gates ≤ 1 ms @10⁶ respectés en projection).

### Intégration services
- **Aletheia** : code d'intégration prêt (`windowDegreeHubs` + route `GET /api/aletheia/obrain?hubs=K`, feature-detecté), `tsc --noEmit` vert ; preuve runtime binding (store frais : fenêtres exactes) ; **constat données** : colonne degrés générique de `council.obrain` à 0 (watermark ~131 k slots / 937 vivants — fragmentation, jamais compacté) → `repair-degrees`/compaction avant usage réel des fenêtres.
- **Harness** : binaire `obrain` rebuildé (feat/zorder) — smoke bridge RO sur `runtime-dev.obrain` OK (sessions_list servi, compat ascendante prouvée).
- **Bascule des services réels** = **TODO opérateur documenté** (RFC §9.4 : dev-3083 (héberge cette session), aletheia (hôte/unité à confirmer), prod :3082 jamais pour du dev — R7).

## 2. Chantiers de relief (contrôle qualité/performance)

1. **Test préexistant corrigé** (`677e8416`) : `operators_expand::test_expand_edge_type_case_insensitive` — pré-filtre typed-degree sensible à la casse → résolution canonique ; 10/10.
2. **Degré réel en compaction** (`9629dc0e`) : fin du `degree=0` dégradé (scan d'arêtes restreint aux membres) ; colonnes Hilbert/Zorder justes post-compaction ; test dédié ; helper replay robuste (mapping par label) ; lib 795/795.
3. **Époque de mutations** (`bb632569`) : `SubstrateFile.feature_epoch` (bump writer : nœuds/arêtes/centralité/communautés — jamais layout/replay), snapshot fin de tri, `sorted_view_valid()` exigée par zruns AVANT cache → **fast paths « layout trié » prouvablement sains** (mutation post-tri → repli scan) ; lib 796/796.
4. **Préfetch adaptatif** (`5a2eb9e3`) : API additive `prefetch_community_centered` (sous-plage centrée bornée) + test ; **non branché sur le hot path — décision QC mesurée** (préfetch communauté entière ≈ ±5 % à 100 k balayé ; expérience d'activation partielle documentée, RFC §11).

**QC cumulé** : lib obrain-substrate **797/797**, suites integration vertes (window, hilbert_bulk_sort, community_crash_safety, operators_expand, zgrid/zruns/prefetch), engine 2/2, checks + build release OK. Échec résiduel : aucun (le seul historique `operators_expand` est corrigé).

## 3. État déployé & recommandations
- **Moteur** : `feat/zorder` prête à merger/tagger (aucun remote configuré sur banc-obrain — G5 du protocole : pousser hors machine).
- **Prochaines actions opérateur** (runbook RFC §9.4) : merge/tag → rebuild `obrain`/`obrain-node` → swap dev-3083 (fenêtre sûre) → sync binding aletheia → portes G3 → prod si validé.
- **Suite recommandée** (quand un workload l'exige) : activer la voie `window_query_2d_sorted` depuis l'engine/NAPI sur des données saines (council réparé + LDleiden/pagerank), expérience B4 (activation partielle) pour arbitrer le branchement du préfetch centré.

## 4. Addendum — validation à l'échelle (2026-09-04)
Voir RFC §12 : spread depth-3 @10⁶ ≪ 1 ms (gate T11), fenêtres 2D @300 k (zruns 10×→325× sous le crossover ~10-50  0'aire), fill Z ≈ 46  0u hkey, localité Z non dégradée — chiffres complets et recommandations (préfetche borné à l'échelle, ordre de grille ≥ 10⁶, datasets réels mégalaw/PO en attente).
