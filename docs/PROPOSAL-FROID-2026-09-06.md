# Proposition — « ~1 ms même à froid » : deux chantiers (ouverture & CDC)

**Date** : 2026-09-06 — HEAD `41cd17f9` — après by_role (cache chaud 3,3 ms ;
sidecar froid `substrate.hks_cache`, parité ok). Décomposition du « froid »
(process neuf, runtime-dev) : **ouverture substrate ~70-80 ms** (recount/
replay/props) + **chargement fenêtre CDC ~20-30 ms** → les DEUX à traiter.

## Chantier A — Ouverture à froid < 10 ms (empreintes de comptage persistées)

**Problème** : `from_substrate` refait à CHAQUE open : `rebuild_counts_only`
(recount O(E) nœuds+arêtes, histograms) + audit WAL replay (props) — ~70-80 ms
sur runtime-dev, payé par TOUTE requête d'un process neuf (même `count(n)`).

**Design (additif, format inchangé)** :
- Sidecar `substrate.counts_cache` (comme `substrate.hks_cache`) écrit par le
  premier open RW après recount : magic `CNT1`, empreinte = (node_hw, edge_hw,
  wal_tail_off), puis (total_nodes, total_edges, label_live_counts[64],
  edge_type_live_counts par id, epoch courante).
- Open RO : si l'empreinte correspond (pas de nouveau flush/WAL) → **charger
  les compteurs depuis le sidecar** au lieu du recount ; sinon recount puis
  réécriture du sidecar (atomic tmp+rename ; RO qui trouve empreinte ≠ la
  sienne → recount sans écriture).
- Correctness : le recount sert à corriger des compteurs snapshot périmés ; le
  sidecar ne fait que transporter le résultat RÉCENT d'un recount — mêmes
  valeurs ; invalidation stricte par (node_hw, edge_hw, wal_tail) pour ne
  jamais servir de compteurs périmés. L'audit replay (husk) reste un point à
  mesurer séparément (peut être sauté en RO quand WAL vide ? à valider).
- Cible : open RO ~70-80 ms → **< 10 ms** ; suites 0 rouge ; test = deux
  opens successifs sur même copie → second sans recount (instrumentation
  compteur) + égalité counts.

## Chantier B — cdc_tail (index de queue) : delta froid récent < ~10 ms

**Problème** : premier `changes_since_timestamp` charge la fenêtre (~20-30 ms
de décodage) même pour un `since` récent.

**Design (additif)** :
- Zone `substrate.cdc_tail` (magic `CDCT`, fixed 20 B/frame : `ts u64,
  payload_off u64, frame_len u32`) tenue à l'append CDC (sous le même lock) ;
  fenêtre glissante bornée (50 k → ~1 Mo, trim par réécriture périodique).
- `cdc_persist::last_ts()` (lecture dernière frame → early-exit « rien de
  neuf » en µs) et `frames_since(since, cap)` (entrées depuis la fin, ts ≥
  since → offsets).
- `CdcLog::changes_since_timestamp` chemin FROID : si persistor et
  `since ≥ ts_première_entrée_tail` → décoder UNIQUEMENT les frames ≥ since
  (seek) + fusion événements mémoire de la session ; sinon (since très vieux)
  → ensure_history_loaded inchangé (fallback). Pas de résident 10 k.
- Cible : `delta(60 s)` froid de ~90-100 ms (avec open) → **~open + ms** ;
  `delta(0)` froid conserve le fallback borné.

## DoD commune
1. bench de décomposition daté (open vs cdc vs query) au HEAD (fait §-ci-dessus,
   à figer chiffré) ; 2. A puis B, chacun : tests + suites 0 rouge (engine
   680/724, substrate 807, core 870, formes search_*) ; 3. bench avant/après
   daté sur runtime-dev (copie) ; 4. commit unique réversible par chantier ;
   5. re-bascule binaire prod/dev avec l'utilisateur ; 6. ledger.

## Ordre
A (ouverture — levier dominant ×toutes requêtes) → B (cdc_tail). Chacun exécuté
complet avant le suivant.
