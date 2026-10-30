# MISSION PERF — chemin d envoi de message (P1/P2/P3/P4)

> Origine : vérification externe PO, 2026-09-09. Déposée en fichier car le store
> runtime-dev est verrouillé par le harness (mono-écrivain — comportement correct) :
> à importer dans le graphe (manage_adr / import_docs) au prochain round.

Le moteur est sain (query GQL 0,1 ms sur 334k nœuds / 1,14M arêtes). Deux chemins périphériques dégradent l envoi de message.

## P1 — compteurs d instantané non persistés (≈600-900 ms par ouverture)
Chaque ouverture qui suit une écriture RW recompte les 334k nœuds
(« compteurs d instantané FAUX, corrigés depuis les zones »).
- Fix : persister le snapshot corrigé au flush/checkpoint, ou compteurs incrémentaux à l écriture.
- Cible : ouverture ≤ 30 ms SANS compact préalable (29 ms mesurés juste après compact — la cible est atteignable).
- REPRO figé : `~/bench-stores/runtime-dev-snapshot-20260909.obrain` (51 863 messages, compteurs périmés à dessein).
- Banc prêt : `scripts/bench-message-path.sh`. Baseline 09/09 : ouverture 660-795 ms.

## P2 — runtime_search ≈1100 ms chaud SOUS ÉCRIVAIN CONCURRENT
Sur snapshot figé : 0,8 ms chaud. Sur runtime-dev vivant (harness RW actif) : ≈1100 ms CHAQUE appel.
Le coût vient de l invalidation/resynchronisation face aux écritures concurrentes.
- PROFILER SOUS ÉCRIVAIN ACTIF (le banc statique ne le reproduit pas — ajouter un mode --with-writer).
- Cible : < 50 ms chaud sous écrivain. C est l outil du chemin message (session-persistence-graph, 5 sites d appel).
- Référence : context_build et search_graph s amortissent (0,1 / 36 ms chauds) — runtime_search ne s amortit jamais.

## P3 — obrain-live-sync : curseur incrémental
627 091 lignes rescannées toutes les 3 s, 854 min CPU cumulées, « 0 événements » la plupart des passes.
Persister l offset par fichier journal et seek. Clarifier le doublon avec cbm-runtime-sync.timer (15 min).

## P4 — viz serve : porter en release
N existe qu en build debug (10 Go RSS). Aussi : le preset `/tmp/linux-full3.obrain` (7,3G) vit en /tmp — à déplacer.

## Contexte WAL (acquis à préserver)
- Corruption crash-boundary ÉLIMINÉE le 09/09 (`compact --prune-wal`, les 2 stores).
- Ne pas re-corrompre : jamais de kill brutal d un bridge en écriture ; ENOSPC est un vecteur réel
  (disque à 99 % — 16G libres après ménage du 09/09 : wal-backup, pre/bak-migrate, deploy-stages 01/09 supprimés, source `runtime.obrain` conservée).
- Reliquat cosmétique : 3 nœuds coquilles (labels sans props) dans dev et prod.

## DoD global
Étape « chemin message » dans guard.sh : ouverture ≤ 30 ms après écriture + runtime_search < 50 ms chaud sous écrivain + context_build < 50 ms, benchés sur le snapshot + un écrivain synthétique.
