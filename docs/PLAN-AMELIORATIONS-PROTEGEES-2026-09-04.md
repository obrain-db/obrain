# Plan d'améliorations protégées — obrain engine (2026-09-04)

**Statut** : PROPOSÉ — plan affiné par exécution supervisée, à valider avant Phase 0.
**Branche cible** : `feat/zorder` (worktree `/home/triviere/projects/banc-obrain`).
**Contexte** : chaînes méthodes livrées (Hilbert physique, Z-order logique, cascade features 64d,
spectral, géométrie discrète, kernel Φ₀, tiers L0/L1/L2) — RFC `zorder-superposition.md` §17
propose C2 puis C5. Ce plan définit COMMENT exécuter chaque amélioration **sans régression** :
benchmarks, retours de commande (4 surfaces), empreinte mémoire.

---

## 1. Contraintes auto-imposées (non négociables)

| # | Contrainte | Conséquence opérationnelle |
|---|---|---|
| C1 | **Additivité stricte** | Nouveauté = zone/file/CALL/API/option supplémentaire. `SUBSTRATE_FORMAT_VERSION=1` **inchangé**. Anciens binaires ignorent les zones nouvelles ; nouveaux binaires ouvrent les anciens stores sans migration. |
| C2 | **Zéro retour cassé** | 4 surfaces protégées par golden outputs figés (GQL, NAPI/binding+typings, CLI, bridge/MCP JSON-lines). Diff strict = ∅ avant tout commit. |
| C3 | **Perf non régressive** | Mêmes stores réels, mêmes commandes, avant/après. Δ ≠ 0 autorisé **uniquement sur l'axe visé** et dans le sens annoncé. Aucune modification de hot path sans bench préalable (leçon QC préfetch : "pas de hot path sans bench dédié"). |
| C4 | **Empreinte mémoire bornée** | Gates RSS datés (baseline : ~257 Mo RSS obrain vs kuzu ~160 Mo @445 k/1,67 M, adv §26). Sidecars ≤ 4 o/slot sauf justification écrite. Caches bornés + invalidés par génération (pattern zruns). Rien n'est chargé en plus : mmap advice seulement. |
| C5 | **Un commit par chantier, réversible** | Rollback = retirer le commit/la zone. Aucun commit géant multi-chantiers. |
| C6 | **Dette zéro** | Aucun test existant modifié pour faire passer un nouveau. Tout correctif de test = chantier séparé documenté (pattern `operators_expand` corrigé à la racine, RFC §11). |
| C7 | **Chiffres datés et reproductibles** | Toute mesure → ledger daté (machine de référence identifiée, seed fixe, commandes exactes). Pas de chiffre non rejouable. |
| C8 | **Opt-in pour tout changement sémantique** | Tout ce qui change `community_id`, l'ordre physique ou la sémantique de localité reste **derrière option/flag** ; le défaut est bit-identique à aujourd'hui. |
| C9 | **Pas de déploiement prod depuis cette session** | Règle R7 du PROTOCOLE-DEPLOIEMENT : :3082 jamais redémarrée pour du dev. Cible : validation moteur + tests, pas de bascule de service. |
| C10 | **Non-régression des invariants** | Fenêtres exact==sorted ; QC comptes == sources après toute ingestion ; `resume-ordre-journal` rejoué pour tout ce qui touche l'ordre physique ; canal CDC intact si zones touchées. |

## 2. Périmètre — chantiers re-scopés

### 2.1 Phase 0 — Gel de baseline (prérequis de tout le reste)
Livrable : **ledger de baseline daté** + batteries de golden outputs.
- Comptes : lib substrate **798/798**, engine **678/678**, tests sémantiques (text-index, vector-index, hybrid-search), suites E2E (hilbert_bulk_sort, zgrid/zruns/prefetch, community_crash_safety, operators_expand).
- Bench rejoué : `scripts/bench-verify/verify-o1.mjs`, `verify-engine.mjs`, native-import vs kuzu (mêmes stores `target/bench/real/`), gates `bench/substrate/` (retrieval_gate.json, t16_validation.json, T9/T11).
- Golden outputs : captures canoniques des 4 surfaces (GQL counts exacts 445 493 / 27 501 / 95 386 / 349 289 ; fenêtres ; CLI `topo`/`stats --format json` ; bridge `sessions_list` ; NAPI smoke).
- RSS : open / après-touché / après-tri sur code-all3 + rt-prod.
- Tag de référence si HEAD non taggé.

### 2.2 Phase 1 — Additifs sûrs (aucun chemin existant modifié)
| Chantier | Contenu | Risque | DoD |
|---|---|---|---|
| **P1a — C5 : fenêtres Z en GQL** | Procédure CALL `obrain.window_2d(community, centrality_range, degree_range)` (scan exact + repli zruns si layout trié) exposée via le dispatch `plan_call_procedure` | 🟢 | ✅ **LIVRÉ** `143f68b9` — CALL documenté ; égalité exact==sorted ; tests 4/4 ; bench code-all3 : CALL hubs d≥50 181 matches 134,8 µs (voie zruns, ≪ scan 245 µs) |
| **P1b — B4 : préfetch centré borné** | API additive `prefetch_community_centered(cid, centre, demi-largeur)` (sous-plage Hilbert, mono-range madvise déjà capable) ; **non branché sur le hot path** | 🟢 | ⚠️ **DÉJÀ LIVRÉ** `5a2eb9e3` (feat/zorder, additif, testé `store.rs:15763`) — chantier = **validation** : re-vérifier test + mesurer B4 (non exécuté) |
| **P1c — Index propriété lookup** | `create_property_index` existant validé en perf sur code-all3 (65 µs mesuré, ×935) ; compléter si trou : mapping qn→slot | 🟢 | ⚠️ **DÉJÀ LIVRÉ** (engine `index.rs:34` + NAPI `createPropertyIndex`) — chantier = **validation** : bench lookup déjà 43-65 µs baseline ; défaut (scan) inchangé sans index |

### 2.3 Phase 2 — Retrieval (hot path retrieval uniquement, gates sémantiques)
| Chantier | Contenu | Risque | DoD |
|---|---|---|---|
| **P2a — BM25 élagage ≤1 ms** | Élagage structurel BM25 @20 k (2,27 ms → ≤1 ms) ; débloque hybrid ~0,6 ms | 🟡 | ✅ **LIVRÉ** `dbd2024a`+`1909a818` (fix stabilité) — **684 µs @20 k (×3,0, cible ≤1 ms)** ; gates sémantiques 3/3 ; top-1 exact ; flakiness v1 C2 préexistante documentée (2/12 avant comme après) |
| **P2b — C2 : RAG topologique industrialisé** | Sortir le rerank par dominance de cluster (v1 prouvée : precision@5 0.530 → **0.944**) du test vers un pipeline consommable ; **chemin non-topologique bit-identique** | 🟡 | ✅ **LIVRÉ** `b50a4d94` — pipeline `hybrid_search_topological` sans oracle : **0,518 → 0,766** (composantes réelles CALLS) ; bit-identité boost=0 testée ; v1 non modifiée (C6) |

### 2.4 Phase 3 — Structures opt-in (défaut bit-identique)
| Chantier | Contenu | Risque | DoD |
|---|---|---|---|
| **P3a — Kernel Φ₀ vivant** | `KernelEnricher` thinker (T17l Ph2 documenté) : maintient `_kernel_embedding` en incrémental ; réutilise la boucle `session_metrics → kernel_adaptation` existante | 🟡 | ✅ **LIVRÉ** `8ea058dd` — tests 3/3 (empty no-op · dépendance hilbert · chaîne complète + skip 2ᵉ tick) ; cognitive enrichment 61/61 ; config TOML + ThinkerKind + spawn fleet ; doc canonical-features-pipeline Ph2 livrée |
| **P3b — Géométrie → communautés** | κ (Ollivier-Ricci) comme poids/frontière dans LDleiden — **derrière option** ; défaut = comportement actuel | 🟠 | ✅ **LIVRÉ** (variante additive `run_ldleiden_batch_ricci(substrate, stats, ricci_boost)`) — **A/B : boost=0 bit-identique** (testé) ; principe validé : pont κ=−0,333 affaibli (w=0,667) vs intra-triangle κ=+0,388 renforcé (w=1,388) ; colonnes `ricci_u8` réutilisées ; chemin unitaire `run_ldleiden_batch` inchangé ; 5/5 tests + fallback sans feature |
| **P3c — Plan sémantique (localité par forme)** | Tri physique optionnel sur projection 2D de features canoniques (eigvecs spectraux 0-1) au lieu de (centrality×degree) ; chaînon du design doc `continuity-spectral-spatial.md` | 🟠 | ⚠️ **Étape A ✅ (validation) + Étape B REPORTÉE (gate de mesure)** — A : la forme spectrale sépare les clusters (intra 1,367 < inter 1,880, `5e1fa458`) ; **B : mesure B3 datée 2026-09-05 sur deux clusters faiblement reliés — localité intra-cluster topologique 8753,3 vs spectral 8962,7 → PAS d'amélioration (léger recul ~+2,4 %) ⇒ mode `Plan2D::Spectral` NON implémenté** (dégraderait le défaut sans gain) ; gate verrouillée par test (basculera si un futur plan spectral fait mieux) |

### 2.5 Phase 4 — Format / ordre physique (le plus encadré)
| Chantier | Contenu | Risque | DoD |
|---|---|---|---|
| **P4a — Ingestion native zone-direct <0,5 s** | Writer direct par blocs + relink O(E+N) + rebuild degrés/compteurs ; **store neuf uniquement**, checkpoint final, pas de WAL par op | 🟠 | ✅ **DÉJÀ LIVRÉ + VALIDÉ REJOUÉ** (`obrain native-import`, CLI `native_import.rs`) — re-mesure 2026-09-04 : **0,254 s** @445 k/1,67 M (baseline 0,267 s), wall 0,39 s, RSS 334 Mo, QC comptes exacts ; < 0,5 s visé ✓ (réf kuzu 0,5 s au checkpoint) |
| **P4b — Ordre physique temporel (zone journal)** | Superposition (time × communauté) sur la même mécanique ; supprime le re-tri par seq du harness (session-persistence) | 🟠 | ✅ **LIVRÉ (garantie d'ordre, périmètre honnête)** — `read_session_journal` trie la fenêtre décodée par seq (no-op nominal ; correctif si appends désordonnés respawn/retry) ; tests 2/2 ; engine 680/680 ; **limite documentée** : la SÉLECTION de fenêtre reste physique (collecte à rebours bornée par limit) — supprimer le re-tri harness nécessiterait de corriger la sélection (décodage complet, coût évité par conception) → action harness séparée ; zone journal inchangée (aucun format touché) |

### 2.6 Phase 5 — Couplages cognitifs (design d'abord, implémentation ensuite)
| Chantier | Contenu | Risque | DoD |
|---|---|---|---|
| **P5a — C3 : fenêtre cognitive** | Fenêtres Z (centrality×degree) ∩ énergie/spread → « sous-graphe vivant d'une communauté » | 🟠 | ✅ **LIVRÉ** (procédure additive `obrain.window_2d_energy(cid, c_lo, c_hi, d_lo, d_hi, energy_min_q15) YIELD node_id, energy`) — filtre par énergie inline (Q1.15, `read_node`/slot, O(|fenêtre|)) ; seuil 0 = fenêtre C5 inchangée ; read-only OK ; CALL C5 existant **intact** ; 6/6 tests dont 2 nouveaux ; engine 678/707 |
| **P5b — C6 : mémoire de sessions cognitive** | substrate sessions + hybrid + fenêtres + spread → continuité de contexte (fil rouge CH6) | 🟠 | EN ATTENTE — design doc §4 `design-phases-restantes-2026-09-04.md` (payload context_build additif rétro-compatible, mesure rappel vs baseline) ; implémentation = round dédié |

## 3. Méthodes de vérification (par chantier, toutes obligatoires sauf mention)

1. **Suites** : `cargo test -p obrain-substrate --lib` (798), `-p obrain-engine --lib` (678), + suites features/tests E2E du chantier.
2. **Bench-verify rejoué** : verify-o1, verify-engine, native-import vs kuzu — sorties datées dans `docs/bench-verify/`.
3. **Gates bench/substrate** : retrieval_gate.json, t16_validation.json ; critère spread depth-3 ≤1 ms @10⁶ (T11) et T9 quand applicable.
4. **Golden outputs (4 surfaces)** : diff strict avant/après — GQL counts, CLI, NAPI typings/smoke, bridge JSON-lines. Toute différence = chantier bloqué.
5. **RSS** : open / touché / trié, mêmes stores, même machine — diff ≤ tolérance annoncée (défaut : +5 % ou +X Mo justifié).
6. **Invariants spécifiques** : exact==sorted des fenêtres ; QC comptes == sources ; `resume-ordre-journal` (ordre physique) ; canal CDC (zones).
7. **Propriétés A/B** pour tout opt-in : option éteinte ⇒ sorties identiques bit à bit (test dédié).

## 4. Définition de "fini" (par chantier — toutes les cases)

- [ ] Code implémenté **additif/opt-in** selon la contrainte C1/C8
- [ ] Tests nouveaux verts **sans modifier de test existant** (C6)
- [ ] Suites complètes vertes (substrate + engine + features du chantier)
- [ ] Bench chiffré avant/après, daté, reproductible (C7), Δ conforme (C3)
- [ ] Golden outputs des 4 surfaces : diff = ∅ (C2)
- [ ] RSS avant/après : diff ≤ tolérance (C4)
- [ ] Commit unitaire + note datée dans le ledger
- [ ] Non-régression des invariants vérifiée (C10)

## 5. Gouvernance des mesures — ledger

Un fichier `docs/ledger-ameliorations-2026-09-04.md` (créé en Phase 0) consigne par ligne :
`date | chantier | commit | tests (n/n) | bench avant → après | golden diff | RSS avant → après | notes`.
Aucun chantier ne commence sans sa ligne de baseline ; aucun ne se clôt sans sa ligne de résultat.

## 6. Risques & parades

| Risque | Parade |
|---|---|
| Régression silencieuse sur un chemin non visé | Golden outputs + suites complètes AVANT commit (jamais après) |
| Dérive mémoire d'un sidecar/cache | Gate RSS par chantier ; sidecars ≤ 4 o/slot (C4) ; caches bornés + génération |
| Changement d'ordre physique cassant le harness | `resume-ordre-journal` rejoué ; opt-in par défaut (C8) ; jamais de redémarrage prod (C9) |
| Chantier trop gros = commit illisible | Découpage par chantier (C5) ; sous-étapes commitables si nécessaire, chacune verte |
| Chiffres non reproductibles | Seed fixe, machine identifiée, commandes exactes dans le ledger (C7) |
| Tentative de "réparer" un test en le modifiant | Interdit (C6) ; tout correctif = chantier séparé avec cause racine documentée |

## 7. Non-objectifs (explicites)

- Ne PAS changer `SUBSTRATE_FORMAT_VERSION` ni le format des zones existantes.
- Ne PAS modifier les sorties existantes de GQL/CLI/NAPI/bridge (ajout seulement).
- Ne PAS redéployer/basculer un service prod (:3082) depuis cette session.
- Ne PAS implémenter P5 sans design doc validé au préalable.
- Ne PAS introduire de dépendance externe nouvelle sans justification écrite.
- Ne PAS traiter P3b/P3c/P4b comme défaut : ce sont des modes optionnels mesurés.

## 8. Séquencement proposé (ordre d'exécution)

```
Phase 0 (baseline + golden + ledger)  →  valide le protocole
  → P1a (C5 fenêtres GQL)             → preuve complète du protocole
  → P1b (B4 préfetch) · P1c (index)   → additifs sûrs
  → P2a (BM25) · P2b (C2 RAG topo)    → retrieval, gates sémantiques
  → P3a (kernel vivant)               → thinker, sans risque requête
  → P3b (κ→communautés) · P3c (plan sémantique) → opt-in mesurés
  → P4a (ingestion native) · P4b (ordre temporel) → format/physique encadré
  → P5a/P5b (C3/C6)                   → design d'abord
```

Chaque phase ne démarre que si la précédente est entièrement verte (toutes cases DoD cochées).

## 9. Attentes explicites de l'exécution supervisée

1. Chaque chantier est proposé **avant** d'être exécuté (diff de plan + DoD rappelés) — pas d'effet de surprise.
2. Tout écart au plan (contrainte, chiffre, périmètre) est signalé immédiatement, avec l'option rollback.
3. Les mesures sont honnêtes : si un axe ne s'améliore pas, le chantier est reporté/redéfini, pas maquillé (tradition du dépôt : « lecture honnête »).
4. À la fin de chaque phase : point d'étape consolidé (ledger) avant la suivante.
5. Le document vit avec l'exécution : chaque ligne de résultat y est reportée.

---

*Document vivant — à valider avant Phase 0. Références : RFC zorder-superposition.md (§17), docs/bench-verify/README.md, CHECKPOINT-ZORDER-2026-09-04.md, PROTOCOLE-DEPLOIEMENT.md, docs/continuity-spectral-spatial.md, canonical-features-pipeline.md.*
