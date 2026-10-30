# Checkpoint & plan de déploiement obrain — dev & prod deepseek-harness

**Date** : 2026-09-05 — branche `feat/zorder`, worktree PROPRE. HEAD : voir git log.
Artefacts dispo : `target/release/obrain` (CLI, compare/enrich/journal-import/
réparations Rust) · `target/release/libobrain_node.so` (binding NAPI).

## État verrouillé (checkpoint)
- Comparatif multi-variantes : compare 7,2 s → ~0,4 s (caches) / 0 ms (mémorisé),
  résumés préconstruits à l'import (journal d'import, étapes 1-4).
- Fenêtres : sélecteur adaptatif scan/zruns (seuil calibré 4500, env
  OBRAIN_ZRUNS_MIN_COMMUNITY) ; résultats identiques quelle que soit la sélection.
- Inventaire moteur : 0 rouge (141/141 ×2 passes) ; core 870 · substrate 807 ·
  engine 680/724 · CLI 70.
- Correctifs : préfiltre degré typé (undirected correct), verrou MERGE (0/12).

## Règles du déploiement (exigences utilisateur)
1. Tout ce qui concerne obrain, à jour et fonctionnel (binaire + binding).
2. **PRÉSERVER les bases existantes** (runtime*.obrain, dbs) — aucune
   migration/écriture sur les stores pendant le déploiement.
3. Prod AVANT dev.
4. Restart : à la discrétion de l'exécutant, proprement (unités/services).

## Cibles découvertes (recon, à confirmer)
- Stores : `~/.local/share/obrain-harness/data/` (runtime-dev.obrain,
  runtime-full*.obrain, runtime.obrain) — stores ACTIFS (WAL/CDC récents) :
  ne jamais écrire dedans pendant la bascule.
- Binding historique attendu sous `.../obrain-harness/vendor/libobrain_node.node`
  (dossier actuellement vide ?).
- Aletheia : `~/projects/aletheia` + unités systemd `aletheia.service`,
  `aletheia-install.service`, `aletheia-nim-wallet.service`, `aletheia-docs.service`.
- Pas d'unités systemd `deepseek-harness*`/`obrain*` trouvées → dev/prod tournent
  probablement autrement (pnpm dev / tmux / wrapper) — À CONFIRMER.

## Étapes prévues (après confirmation des 2 points ci-dessous)
1. Rebuild release complets : `cargo build --release -p obrain-cli -p obrain-node`.
2. Copie en sécurité des binaires/binding actuellement en place (backup horodaté).
3. Prod : arrêt propre des services cibles → remplacement binaire+binding →
  démarrage → smoke (info store, session) → validation.
4. Dev : même séquence.
5. Aletheia : selon sa consommation d'obrain (à préciser).
6. Rollback immédiat possible (backup) ; aucun store touché.
