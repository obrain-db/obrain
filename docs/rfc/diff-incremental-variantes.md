# Diff incrémental O(changements) entre variantes — conception (chantier 3)

**Date** : 2026-09-05 — branche `feat/zorder`. But : quand une variante
(worktree/branche) est ré-importée, comparer `main vs branche` doit coûter
O(changements) et non un rescan du graphe (mesuré aujourd'hui : gros store
froid ~0,78 s ; déjà-mémorisé 0 ms quand RIEN n'a changé — le trou est le cas
« UN store a changé »).

## État actuel (mesuré, safe)

- Nœuds : empreintes `substrate.compare-hash[.<prefix>]` (FNV par slot),
  invalidation mtime `>=` — rejeu exact vérifié par test de parité ;
- Résultat par paire `substrate.compare-result.<clé>` → **0 ms** si les deux
  stores sont intacts ;
- Arêtes : toujours un scan de zone (verrou unique `for_each_live_edge`) à
  chaque recalcul — ~150-300 ms par store (1,67 M).

## Conception — résumé d'arêtes par store (extension symétrique des empreintes)

1. **Fichier latéral** `substrate.compare-edges[.<prefix>]` : en-tête (magic,
   version, hw edges, comptes bruts par type id local) + triplets bruts
   `(src_h:u64, type_h:u64, dst_h:u64)` ordonnés par slot — construit pendant
   le premier scan (même passe que l'empreinte de nœuds), réutilisé ensuite.
   Taille : 24 o × E → 40 Mo pour 1,67 M (lecture ~10-20 ms mmap).
2. **Invalidation** : mêmes règles mtime `>=` que les empreintes de nœuds.
3. **Diff** : si un store est intact et l'autre changé, l'ancien côté est
   reclassé (communs vs propres) et trié **sans rescan zone** (lecture du
   fichier + filtres par membres de l'autre store) ; seul le côté changé est
   scanné. Cas deux stores changés : scans normaux (pas de régression).
4. **Correctness** : mêmes clés `mix3`, même sémantique ENSEMBLE (dédup) et
   même parité cache == recalcul que les empreintes → tests : parité triplets
   cache == scan ; un-côté-changé == diff complet ; invalidation.
5. **Gain attendu** (à mesurer, machine n3) : boucle N-variantes après un
   changement d'une seule branche → ~0,4 s (au lieu de ~0,8 s) pour le gros,
   ~30 ms pour la paire ; objectif daté après implémentation.

## Conception — journal d'import (le vrai O(changements) à terme)

L'import code (binding → store) n'émet pas de journal : pour comparer main vs
branche ré-importée en O(delta), il faut qu'un ré-import écrive un **journal
de changements** (CDC-like, borné) : diff sur la base sqlite CBM (nodes/edges
par `qualified_name`) pendant l'import → événements add/remove node/edge ;
consommés par `compare` comme sur-ensemble du résumé. Chantier à part
(pipeline import + format journal), non inclus ici.

## Plan d'exécution par étapes (2026-09-05)

- ~~**Étape 1**~~ FAIT (88bcd77f) : brique pure-Rust : format du journal
  (`substrate.import-journal`) + fonctions pures `manifest → diff → événements`
  (A/D nœuds & arêtes) — tests unitaires + propriété aléatoire. [démarrée]
- ~~**Étape 2**~~ FAIT (b573ccc1) : captation : pendant l'import code, émettre les événements du
  journal (diff vs manifest précédent) — côté Rust de l'import/engine.
- ~~**Étape 3**~~ FAIT (6d00cdd6) : résumés préconstruits à l'import : `compare` applique le journal pour rafraîchir
  les résumés du store changé (empreintes + compare-edges) sans rescan →
  O(changements) mesuré (bench gros avant/après).
- ~~**Étape 4**~~ FAIT (bench big 211 ms, hook import détaché + retry, ledger/doc).

## DoD (chantier 3, quand exécuté)

1. Résumé arêtes + reclassement sans rescan ; 2. tests (parité, un-côté-
changé, invalidation) ; 3. bench daté avant/après (gros & paire) dans le
ledger ; 4. portes vertes ; 5. commit réversible ; 6. doc + RFC à jour ;
7. aucun format modifié ; 8. zéro retour cassé. — Journal d'import :
RFC séparée.
