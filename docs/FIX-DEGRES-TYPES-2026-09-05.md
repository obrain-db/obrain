# Correctif racine — colonnes de degrés typées périmées (« arêtes typées → 0 »)

**Date** : 2026-09-05 — **Branche** : feat/zorder — **HEAD** : b95ca5e1
**Session de reprise** : session-6eabd7a9 (suite de session-e6cde2d1 coupée sur limite de contexte à 14:40:51)
**Périmètre décidé** : code moteur + tests uniquement — AUCUNE manipulation des stores live ni des writers en cours.

---

## 1. Le bug (classe d'incident, reproduite deux fois : prod 2026-09-01 puis runtime-dev 2026-09-05)

Symptôme : `MATCH (a)-[:T]->(b) RETURN count(*)` renvoie **0** silencieusement alors que les
arêtes existent. Cause : le pré-filtre typed-degree (`crates/obrain-core/src/execution/operators/expand.rs`
~l.259-288) saute tout nœud dont `out_degree_by_type(node, T) == 0`. Les colonnes
`substrate.degrees.node.<TYPE>.u32` (sidecars mmap) sont **périmées** — CRC valide, valeurs
fausses — quand des arêtes ont été écrites par un chemin qui ne maintient pas les sidecars
(binaire plus ancien avec mapping de slots différent : incident 2026-09-01, commit `684bb453` ;
écart 48 min sur runtime-dev : `substrate.edges` 14:23 vs colonnes 13:35) → le pré-filtre saute
tous les nœuds → 0.

Preuves (session précédente, ledger) : kill switch `OBRAIN_DISABLE_TYPED_DEGREE_PREFILTER=1`
→ TOUCHES 9 111 / PART_OF 35 990 (correct) ; `obrain repair-degrees` (19 sidecars rebuildés)
→ correct sans kill switch. Probe directe : `TOUCHES scan edges = 9111 | colonne typée = 0`.

## 2. Ce que l'expérience contrôlée de la reprise a établi (2026-09-05 15:00)

Pilote `/tmp/e2e-degrees.py` (store neuf, jamais live) :
- `obrain init` + nœud File ; writer = `obrain bridge` `OBRAIN_WRITE=1` (`open_shared_write`)
  + `import_events` (2 lots) ; lecteur RO = second `obrain bridge` (lecture partagée) ; crash
  SIGKILL du writer puis réouverture (replay WAL).
- **Résultat : chemin actuel 100 % sain.** counts typés RO = 1/1/1/1/1 (HAS_TURN/HAS_STEP/
  PART_OF/INVOKED/TOUCHES) pendant que le writer vit ; colonnes flushées avec `substrate.edges`
  (mêmes mtimes) ; après crash + replay + lot 2 → cohérent.
- **Conclusion : le chemin `import_events` → GQL MERGE → `SubstrateStore::create_edge`
  (store.rs:14718, incréments `incr_typed_out/in_degree`) maintient correctement les sidecars.**
  La dérive 13:35→14:23 de runtime-dev ne peut donc PAS être imputée au writer actuel : c'était
  un writer/binding d'un autre binaire (écosystème de déploiement du 05/09 : bascule PROD,
  binaire partagé, bindings vendors, backups) — non reproductible avec le code HEAD actuel.
- Store live dev (14:49-14:54) : requêtes typées OK (TOUCHES 17 771 / 270 009 arêtes) ; colonnes
  HAS_STEP/PART_OF/PRODUCED/TOUCHES rafraîchies en même cadence que `substrate.edges` ⇒ writer
  actuel sain.

⇒ Le problème « définitivement clos » = le moteur doit **ne plus jamais pouvoir servir un 0
silencieux** quand des sidecars de degrés sont périmés, quelle qu'en soit l'origine amont
(writer étranger, binaire ancien, migration, mapping). Remède actuel = opérateur (`repair-degrees`)
ou kill switch manuel : insuffisant (l'incident 09-01 a récidivé le 09-05).

## 3. Conception du correctif (proposée — à valider à l'implémentation)

### 3.1 Verrouillage : auto-vérification + auto-guérison à l'hydratation (substrate)

Ancrages existants (crus) :
- `SubstrateStore::typed_degrees()` — store.rs:13927 — hydratation **différée** (choix assumé :
  l'ouverture de chaque sidecar à l'open coûtait +85 ms/65 types sur PO) → le point de
  vérification naturel = juste après `ensure_initialized`, **une fois par store** (`OnceLock`),
  au premier usage typed — coût de somme sur pages déjà mappées, invisible.
- Oracle de vérité déjà disponible, O(1) après open : `edge_type_live_counts` (re-comptés et
  corrigés à l'open par `rebuild_counts_only`/`rebuild_live_counters_interne` — store.rs:13621,
  WAL window merges faits avant). Sémantique : arêtes non-tombstonées, `src != 0 && dst != 0`.
- Primitives de rebuild existantes : `rebuild_typed_degrees_into` (store.rs:14004, scan complet
  dans un registre vide), `rebuild_degree_columns` (store.rs:13967, supprime les sidecars puis
  force le lazy rebuild — aujourd'hui réservé à l'opérateur, store fermé).

Vérification (par type T ayant une colonne présente) :
- `sum_out(T)` = Σ slots 1..n_slots de la colonne out de T ; comparer à `edge_type_live_counts[T]`.
- **Périmé si `sum_out(T) < live_count(T)`** (la colonne est EN DESSOUS des arêtes réelles →
  risque exact de 0 silencieux). `sum ≥ live` ⇒ pas de données manquantes (tolère le fringe des
  arêtes 0-extrémité qui ne touchent pas les slots lus) → sain.
- Symétrique pour `sum_in(T)`.

Guérison :
- Store **ouvrable en écriture** (writer/shared-write/RW) : rebuild ciblé du/des type(s) périmé(s)
  (réutiliser le scan : incréments dans des colonnes remises à zéro / fichiers recréés — à caler
  sur l'API interne de `TypedDegreeRegistry` : `get_or_create`, `open_existing_columns`,
  `flush`, typed_degree.rs l.196-475) + `flush()` ; warn journalisé (réparation non muette,
  comme le pattern compteurs « FAUX, corrigés » store.rs:4660-4680).
- Store **lecture seule** (RO, ne peut pas persister) : drapeau en mémoire
  `typed_degrees_unreliable` → le pré-filtre typed-degree tombe sur le chemin générique correct
  pour CE process (cf. 3.2) + warn. (Après le prochain open RW, les colonnes sont guéries pour
  tous.)

### 3.2 Filet de sécurité lecture : pré-filtre jamais silencieusement faux (core)

`expand.rs` (l.259-288) et `typed_degree_rewrite.rs` (`try_plan_typed_degree_topk`) :
- Ajouter une porte de fiabilité par type sur le store (trait ou méthode concrète) consultée
  AVANT le saut `deg == 0` : si la colonne du type demandé est déclarée non fiable (RO périmé),
  ne pas pré-filtrer (scan générique correct). Kill switch conservé tel quel.
- Ne PAS toucher aux fenêtres Z / topk hors pré-filtre de justesse dans ce chantier (les
  colonnes guéries par 3.1 rendent le cas résiduel transitoire).

### 3.3 Contraintes respectées
Additif strict, `SUBSTRATE_FORMAT_VERSION` inchangé (aucun format nouveau : la vérification lit
les sidecars existants et les réécrit via les chemins existants), zéro retour cassé (chemin
nominal : vérification = somme sur pages mappées, une fois par process, aucun changement de
résultats ; heal seulement en cas de péremption réelle), perf : coût différé + borné mesuré
avant/après (open inchangé — la vérif vit à l'hydratation, pas à l'open ; somme ~ node_hw×4B ×
types présents, ms sur stores runtime), empreinte bornée, un commit réversible, chiffres datés
dans le ledger.

## 4. Tests de non-régression à poser (reproduisent la panne et la verrouillent)

1. substrate : sidecar typé remis à ZÉRO (payload + CRC recalculé valide — même méthode que
   `rebuild_degree_columns_heals_stale_sidecars` store.rs:18043) sur store avec arêtes → réouverture
   RW → `typed_degrees()` auto-guérit → `MATCH (a)-[:T]->(b)` ≠ 0 sans kill switch, sans
   `repair-degrees` manuel.
2. substrate : même péremption → ouverture RO → flag non-fiable posé → requête typée rend le
   résultat correct (chemin générique), pas 0.
3. substrate : store sain → vérification = no-op (aucun heal, aucun warn).
4. engine/core : requête typée après écritures `import_events`-style (MERGE GQL) reste
   correcte ; E2E `obrain bridge` writer+RO (rejouer le pilote `/tmp/e2e-degrees.py`).
5. Suites complètes : substrate lib, engine lib (+ cdc,algos), core, + window (window_call_procedure).

## 5. Bench datés à produire
- Avant/après : requête typée nominale (store sain) — aucune régression attendue (chemin
  nominal inchangé) ; coût d'une ouverture + premier usage typed (somme de vérification) sur
  copie du store runtime-dev.
- Gate : pas de nouvelle latence sur open ; heal une seule fois (pas de récurrence au open suivant).

## 6. État / artefacts
- Ledger : rangée « Reprise — diagnostic approfondi » ajoutée (docs/ledger-ameliorations-2026-09-04.md).
- Non commité (précédent) : `scripts/runtime-to-substrate.mjs` (+auto `repair-degrees` post-import).
- Pilote : `/tmp/e2e-degrees.py`.
- Goal actif : goal-766bf973 (30 rounds) — fermeture définitive du bug ; goal-0f5c3b99
  (formes de recherche) en pause, `search_delta` livré, reste intact.

## 7. État d'implémentation (2026-09-05, session-6eabd7a9)

**Correctif livré** (avant commit) :
- `obrain-core/src/graph/traits.rs` : `GraphStore::typed_degree_reliable(&str) -> bool`
  (défaut `true` ; documente le contrat qualité-de-données des sidecars).
- `obrain-core/src/execution/operators/expand.rs` : le pré-filtre typed-degree ne
  saute un nœud que si la colonne du type est FIABLE — sinon scan générique.
- `obrain-engine/.../typed_degree_rewrite.rs` : le rewrite top-K refusé si la
  colonne du type n'est pas fiable.
- `obrain-substrate` :
  - `degree_column.rs` : `zero_out_in()` (reset) + `sum_out_in()` (oracle).
  - `typed_degree.rs` : `zero_type()` / `out_in_sums()` (registre).
  - `store.rs` : champs `read_only`, `typed_degree_suspects`,
    `typed_degree_health_checked` ; `open_read_only` marque le store ;
    `typed_degrees()` déclenche `ensure_typed_degree_health` (une fois) ;
    `detect_stale_typed_degree_types` (somme vs `edge_type_live_counts`),
    `heal_typed_degree_types` (zero + rescan + flush, écrivain seulement),
    `typed_degree_trusted` ; impl `GraphStore::typed_degree_reliable`.
- Tests : 3 neufs (RO non-fiable, sain no-op, écritures vives après heal) +
  test incident 09-01 adapté (auto-guérison) + E2E engine `typed_degree_autoheal_e2e.rs`
  (RW heal → count 1 ; RO fallback → count 1).

**Mesures (machine n3, copie runtime-dev 4,8 Go)** : surcoût correctif ≤ ~10 ms
(steady 0,87 s fix vs 0,87 s HEAD seul) ; 1er usage sur copie sale = heal une fois.
**Constat hors périmètre** : ouverture CLI ~0,81-0,92 s / 1,05 Go au HEAD vs
0,18 s / 35 Mo au build 12:46 — imputable au commit `c935b9d1` (search_delta) ;
chantier dédié à ouvrir (sensibilité latence utilisateur).

**Limites documentées** : les consommateurs directs de colonnes de degrés hors
pré-filtre (fenêtres Z / topk hub, engrams) lisent les colonnes physiques ;
avec un store guéri par le writer elles sont correctes — un store RO isolé sur
une colonne périmée peut rendre fenêtre vide tant qu'aucun écrivain ne l'a
guérie (cas transitoire, hors 0-silencieux des requêtes typées désormais clos).

## 8. Écart fermé après coup — régression d'ouverture CLI/bridge (CDC eager)

Mesure post-livraison (contexte : le CLI a la feature `cdc` obligatoire depuis
`c935b9d1`) : ouvrir un store substrate décodait la zone `substrate.cdc_journal`
à CHAQUE ouverture (`CdcLog::attach_persist` → `load(CDC_RELOAD_CAP)`, jusqu'à
100 k événements en RAM — payloads props lourds sur stores runtime) :
open CLI warm sur copie runtime-dev 4,8 Go = **0,90 s / 1,05 Go** au HEAD vs
**0,06 s / 35 Mo** au build 12:46 (pré-`c935b9d1`).

Correctif (engine) : rechargement **paresseux** — `attach_persist` n'arme que
l'append ; l'historique n'est décodé qu'à la première lecture
(`ensure_history_loaded`, fenêtre bornée, **dédup** (entity_id, epoch,
timestamp) pour le cas écrivain→lecteur même process), et l'early-exit
`max_timestamp` de search_delta n'intervient qu'après ce rechargement.

Mesure post-correctif (même copie, warm) : count(n) **0,07 s / 35 Mo** (parité
pré-régression) ; typed 1re utilisation 0,10-0,11 s / 56 Mo (= parité PRE —
l'hydratation différée des colonnes préexiste ; surcoût de la vérification
santé ≤ ~10 ms).

Bugs internes attrapés au passage : (1) `heal_typed_degree_types` faisait un
`binary_search` sur des suspects NON TRIÉS (itération DashMap) → tri désormais
déterministe dans `detect_stale_typed_degree_types` ; (2) doublon potentiel du
rechargement lazy quand le process a lui-même écrit avant la première lecture →
dédup par triple (entity_id, epoch, timestamp).
