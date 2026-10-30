# Bench de gains — imports & comparaisons (rejouable)

**Machine n3 · dates 2026-09-05 · branche feat/zorder.** Kit :
`bash scripts/bench-verify/verify-imports-compare.sh [--strict]`
(les stores temporaires sont créés/nettoyés par le script ; les références
ci-dessous viennent de mesures datées du ledger).

## Références et commandes exactes

| Opération | Commande | Réf (min, n3) | Échelle |
|---|---|---|---|
| Ingestion native (445 493 n / 1 670 005 e) | `obrain native-import <frais> /tmp/corpus.bin` | **0,26 s moteur** (wall ~0,4 s, RSS 334 Mo) | 0,2 µs/arête |
| Enrich ricci (1,67 M arêtes) | `obrain enrich <copie code-all3> --ricci` | **~3,2 s moteur** (wall 2-4 s selon cache) | ~2 µs/arête |
| Compare paire worktrees (35-38 k n / ~247 k e) | `obrain compare A B --prefix-a obrain-engine --prefix-b tmp-grafeo-cbm` | froid **~85 ms** · empreintes **~40 ms** · mémorisé **0 ms** | ~1-2 µs/nœud·arête |
| Compare gros (443 661 n / 1,67 M e, props) | `obrain compare code-all code-all2` | froid **~0,9 s** · caches nœuds+arêtes **~195 ms** · un côté changé ~0,7 s · mémorisé **0 ms** | ~1-2 µs/nœud·arête |
| Compare mode profond | `obrain compare A B --attrs` | ~5× le mode structurel | — |

Autres surfaces (verify-o1/engine, rejoués le 2026-09-05) :
edgeCountByType(CALLS) **0,16 µs** · nodeCountByLabel(File) **0,13 µs** ·
count(n) **41 µs** · count(File) 35,5 µs · count(CALLS) 20 867 µs ·
window large **exact==sorted** · hubs d≥50 zruns **71 µs** vs scan 238 µs ·
lookup qualified_name **63 µs**.

## Angles morts à connaître (vérifiés)

1. **Cache OS à froid** : juste après un import/rebuild, les premières
   mesures gonflent (page cache). Le kit fait un warm-up avant les mesures
   « chaud » ; comparer sur machine calme.
2. **Stores quiescents** : `compare`/`enrich` présupposent un store stable
   (pas d'écrivain concurrent) ; sur store vivant, empreintes/mémo invalident
   dès qu'un fichier du store change (invalidation `>=`, testée) — le
   résultat rejoué ne peut pas être périmé, au pire on recalcule.
3. **Empreintes/mémo = fichiers latéraux** `substrate.compare-hash[.<prefix>]`
   et `substrate.compare-result.<clé>` dans le répertoire du store — à
   supprimer si on veut forcer le recalcul (`rm -f <store>/substrate.compare-*`).
4. **Enrich déjà rafraîchi** : relancer `enrich --ricci` sur un store déjà
   enrichi recalcule (idempotent) mais peut être plus rapide ; mesurer sur
   une copie fraîche pour le chiffre « avant/après ».
5. **Multiplicités vs ensembles** : `compare` (structurel et complet) raisonne
   en ENSEMBLES pour les arêtes (les parallèles ne comptent qu'une fois) ;
   les totaux affichés par type restent les comptes bruts.
6. **Limites sémantiques connues** : identité = qn relatif (préfixes CBM
   gérés via `--prefix-a/--prefix-b`) ; renommage de symbole = retrait+ajout ;
   labels/attributs = mode `--attrs` (profond, plus lent) ; nœuds sans qn =
   « sans identité ».
