# Design — chantiers restants (P3c, P4b, P5a, P5b) — 2026-09-04

**Statut** : DESIGN DOC — cadre les chantiers restants du plan
`PLAN-AMELIORATIONS-PROTEGEES-2026-09-04.md` avant toute implémentation.
**Branche** : `feat/zorder`. **État vérifié** : faits ancrés par lecture du code
à HEAD `cff0e200`.

Règle : chaque chantier ci-dessous exige (a) ce design, (b) un proto mesuré,
(c) l'accord avant implémentation sur le substrate (contraintes C8 opt-in,
C10 invariants). Aucun ne touche le chemin par défaut.

---

## 1. P3c — Plan sémantique (localité par forme)

### Objectif
Trier physiquement les nœuds par une projection 2D de features canoniques
(eigvecs spectraux 0-1) au lieu du plan `(centrality_cached × out-degree)`,
**en option**, pour que la localité physique suive la *forme* (signature
spectrale) plutôt que le rôle topologique local. Chaînon du design doc
`continuity-spectral-spatial.md` (reconstruction par forme + localité).

### État vérifié (ancres)
- Point de définition du plan : `hilbert.rs::grid_coords_from_features`
  (rescale partagé) → `hilbert_key_from_features` / `zorder_key_from_features`.
  Une seule grille : Hilbert (physique) et Z (logique) parlent du même plan.
- Consommation : `writer.rs::bulk_sort_by_hilbert` Phase 6 (l.1667 : la même
  boucle remplit les colonnes Hilbert/Zorder/Community depuis
  `centrality[old]`/`degrees[old]`) ; entrée store `store.rs::repermuter_par_hilbert`
  (l.7620) ; permutation `hilbert.rs::compute_hilbert_permutation[_page_aligned]`.
- Features spectrales disponibles : facettes 0-1 de `hilbert_features` =
  eigvecs 1-4 du Laplacien normalisé (via `adapters/.../spectral.rs::spectral_embedding`),
  déjà persistées en `_hilbert_features` (64d) par HilbertEnricher/neo4j2obrain.
- Harness : `docs/continuity-spectral-spatial.md` — rappel par signature
  spectrale + localité Hilbert ; hooks candidats 1-3 (context_build, persistance
  à l'import_events, validation numérique).

### Conception proposée (opt-in strict)
1. **Étape A — validation numérique (hook 3, additif, zéro risque)** : test de
   cohérence entre les facettes spectrales 0-1 de la cascade `hilbert_features`
   et `spectral_embedding` indépendant sur un graphe réel (code-all3) — les
   facettes 0-1 doivent corréler avec les eigvecs 1-2. Livrable : chiffres datés.
2. **Étape B — mode de tri alternatif** : `bulk_sort_by_hilbert` accepte un
   résolveur de coordonnées injecté (`Plan2D::Topological` défaut |
   `Plan2D::Spectral { key }`) ; en mode spectral, Phase 6 lit la projection
   2D des eigvecs au lieu de `(centrality, degree)`. Persistance : flag méta
   `PLAN_SPECTRAL` (bits méta restants) ; replay/warden = même sémantique hint
   que Hilbert (tolérant à la dégradation). Colonne Zorder suit le même plan
   (même grille).
3. **Mesures obligatoires avant décision** : B3 (localité — gaps 4-voisins
   hilbert vs spectral) + spread depth-3 (faults/latence) + fenêtres Z, sur
   code-all3 et rt-prod. **Gate : pas d'amélioration ⇒ chantier reporté** (pas
   de dégradation acceptée sur le défaut).

### Risques
- L'ordre physique change (opt-in) → toutes les surfaces qui dépendent de
  `community_slot_range`/préfetch restent correctes (hints) mais la localité
  mesurée peut être pire. D'où la gate de mesure.
- Le plan spectral exige `_hilbert_features` présentes (dépendance amont
  HilbertEnricher) — mode spectral sur base non enrichie = repli topologique.
- `community_id` n'est PAS touché par P3c (seul l'ordre intra-communauté
  change) — c'est la différence clé avec P3b.

---

## 2. P4b — Ordre physique temporel (zone journal)

### Objectif
Donner aux sessions un ordre physique contigu par `seq` (zone journal
`substrate.journal.<session>`) pour supprimer le re-tri par seq que le harness
fait à la lecture (`session-persistence/src/index.ts:352-371`).

### État vérifié (ancres)
- `engine/src/journal_zone.rs` : journal brut `{type,seq,time,data}` par
  session, zone dédiée `substrate.journal.<session_id>` (7 M lignes séparées du
  graphe). Header OBJ1.
- Le harness re-trie par seq à la lecture — porte de régression :
  `resume-ordre-journal.spec.ts` (externe, graph-writer).

### Conception proposée
1. **Étape A — mesure** : quantifier le coût du re-tri harness (lignes/session,
  temps) sur rt-prod pour justifier ou non le chantier.
2. **Étape B — écriture ordonnée** : les append du journal sont déjà
  séquentiels par construction (WAL-like) ; vérifier que la lecture zone est
  déjà en ordre d'append et que le re-tri harness est un vestige — si oui,
  chantier = documentation + suppression du re-tri côté harness (pas de code
  substrate). Si non (compaction/rewrite réordonne), introduire une colonne
  seq contiguë par session.
3. **Validation** : `resume-ordre-journal.spec.ts` vert ; lecture zone contiguë
  par session ; défaut (ordre actuel) conservé en cas de doute.

### Risques
- Touche l'ordre physique → surfaces larges ; la solution préférée est celle
  qui ne change AUCUN format (le re-tri est probablement déjà un no-op coûteux).
- CDC actif sur certains stores (council) — portes de régression CDC.

---

## 3. P5a — C3 : fenêtre cognitive

### Objectif
Fenêtres Z (centrality×degree) **∩ énergie/spread** — « le sous-graphe vivant
d'une communauté » : sélection par bande d'énergie pendant l'inférence.

### Conception proposée
- Réutilise la capacité C5 livrée (`CALL obrain.window_2d`) + lecture énergie
  par slot (colonne inline substrate) ou UDF `obrain.energy(node_id)`.
- Surface : procédure `obrain.window_2d` étendue d'un paramètre `energy_min`
  (filtre post-fenêtre) OU composition côté appelant (2 appels). Préférer la
  composition (aucun changement du CALL existant — C2/C5 inchangés).
- Proto : sur code-all3 après un spread, fenêtre par communauté + filtre
  énergie ; mesurer le coût du filtre vs la sélectivité.

### Risques
- Faible : purement additif (composition). Le risque est conceptuel (que veut
  dire « vivant » — seuil d'énergie à calibrer).

---

## 4. P5b — C6 : mémoire de sessions cognitive

### Objectif
Substrate sessions + hybrid + fenêtres + spread → continuité de contexte
(« rappel par communauté + énergie ») — le fil rouge CH6 du design doc
`continuity-spectral-spatial.md`.

### Conception proposée
1. **Squelette existant** : `context_build` (injection composée par session,
   bridge/mcp) + `memory_recall` (engrams) — mesurer l'état actuel
   (rappel ~0,48 ms, confiance 0,7 — chiffres du design doc).
2. **Chaînon à enrichir** : fusion du rappel spectral-ensembles dans le
   payload injecté, cadré par maxTokens — sans changer le format du payload
   existant (additif).
3. **Validation** : session_load (messages+texte) inchangé ; rappel par
   communauté+énergie mesuré vs baseline ; doc 11 §9.9 (aletheia).

### Risques
- Surface harness/bridge (MCP JSON-lines) — toute évolution du payload doit
  rester rétro-compatible (protocole RO par défaut, R7).

---

## Non-objectifs communs
- Aucun bump de `SUBSTRATE_FORMAT_VERSION`.
- Aucun changement du chemin par défaut sans gate de mesure positive.
- Aucune modification des sorties GQL/CLI/NAPI/bridge existantes.
- P3c/P4b/P5 : implémentation seulement après validation de ce design + proto.

*Document vivant — à valider chantier par chantier avant implémentation.*
