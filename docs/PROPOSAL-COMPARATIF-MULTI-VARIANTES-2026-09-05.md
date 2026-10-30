# Proposition — comparatif multi-variantes & intégration MCP du relief

**Date** : 2026-09-05 — branche `feat/zorder`, HEAD `f5f7f95f`.
**Statut** : EXÉCUTÉ (rounds 1-2, commits du chantier) — `compare` v1 + `--prefix-a/--prefix-b` (normalisation du préfixe projet CBM dans les qn) ; démo réelle sur 2 worktrees d'obrain : Jaccard nœuds 0,888 / arêtes 0,921 (493 ms), diff par type ; enchaînement import par variante → enrich → comparatifs consigné. Nœuds sans qn = « sans identité » ; qn absolus/prefixés → normalisation documentée.

---

## 1. Contexte / constat (reconnaissance du réel)

- L'index de code **codebase-memory MCP** produit **une base par projet**
  (`~/.cache/codebase-memory-mcp/*.db`) — et les variantes d'un même dépôt
  (branches, worktrees, écarts) existent DÉJÀ comme projets/bases séparés :
  ex. famille obrain : `obrain-engine.db`, `home-triviere-projects-obrain.db`
  (1,4 Go), `tmp-grafeo-cbm.db`, `this-rs.db`… ; famille harness :
  `home-triviere-projects-deepseek-harness.db`, `harness-graph-writer.db`,
  `bench-inject-harness.db`.
- L'import obrain d'aujourd'hui FUSIONNE ces bases (corpus.bin = 15 dbs
  concaténés, ids remapés, sans props → variantes indiscernables) : une
  comparaison binaire n'est donc pas seulement insuffisante, elle est
  IMPOSSIBLE sur un store fusionné.
- L'enrichissement (`obrain enrich --ricci`, livré) s'applique déjà à un
  store par variante ; il manque l'orchestration « par variante » et le
  comparatif.

## 2. Architecture proposée (intégration MCP + relief + comparatif)

**Principe : 1 store par variante, comparatif explicite par paires/matrice.**
Enchaînement (documenté + commandes Rust, zéro script JS pour la logique) :

1. **Import par variante** (séparé) : pour chaque projet/branche/worktree CBM,
   `obrain data/…` ou `native-import <variant.obrain> <variant-corpus.bin>`
   (corpus généré DEPUIS UN SEUL db) — le nom de variante = nom du db
   (`<project>@<branch>` si dispo) persisté en PROPRIÉTÉ d'un nœud racine ou
   fichier d'état `variant` (additif, sans format).
2. **Relief par variante** : `obrain enrich <variant.obrain> --ricci` (déjà
   livré) — étendu plus tard (HKS à la demande, communautés).
3. **Comparatif N-variantes** : nouvelle sous-commande
   **`obrain compare <A.obrain> <B.obrain> [--labels …] [--limit …]`** (v1) :
   identité des nœuds par **(label, `qualified_name`)** (les deux stores étant
   des index du MÊME dépôt à des états différents, les qn sont stables) ;
   sortie :
   - nœuds ajoutés / retirés / modifiés (attributs `file_path`, `start_line`,
     `end_line`, `name` ou présence/absence) par label — avec échantillons ;
   - delta arêtes **par type** (ajoutées/retirées, comptes + échantillons
     `srcQn -[T]-> dstQn`) — le « qui appelle/importe quoi en plus/en moins » ;
   - résumé daté (Jaccard structurel nœuds/arêtes, similarité globale).
   Vue **N-variantes** : comparer `main` vs chaque worktree/branche (boucle
   `compare` par paires) ou matrice — pas de nouvelle primitive ; les CALLs
   existants (`search_delta`, `search_by_role`, `search_surprise`) restent
   utilisables PAR variante (chaque store vivant).

## 3. DoD (8 cases) — v1 (`compare` + enchaînement)

1. Sous-commandes Rust `obrain compare A B [--labels] [--limit]` et (doc)
   boucle N-variantes ; aucun format modifié ; RO sur les deux stores.
2. Identité (label, qualified_name) ; nœuds sans qn = comptés à part
   (« sans identité ») ; attributs comparés listés ci-dessus.
3. Tests synthétiques : 2 stores construits à partir d'un même squelette puis
   divergés (ajout/retrait de nœuds et d'arêtes CALLS/IMPORTS, modification
   de `start_line`) → diff exact (ajoutés/retirés/modifiés/deltas par type) ;
   cas identiques → diff vide ; labels filtrés.
4. Suites : substrate 804, engine 680/724, core 870 — aucune modif existante.
5. Bench daté : `compare` entre deux stores de variantes réelles (ex. petits
   stores construits depuis 2 db CBM de la même famille) — coût O(N+E),
   RSS, rapport ; single big-pair mesuré si corpus dispo.
6. Commit unique réversible + chiffres dans le ledger.
7. Doc : cette proposition (statut) + section « workflow variantes » dans
   `docs/APPORT-OBRAIN-USAGES.md` (usages code multi-branches) + ledger.
8. Pas de déploiement prod ; pas de fusion des variantes dans `compare`.

## 4. Limites / ouvertures

- Identité = (label, qn) : présuppose des index du même dépôt (qn stables) ;
  rename de symboles = retrait+ajout (documenté).
- Comparaison sémantique de code (déplacement de code) = hors v1.
- N-variantes : pas de primitive multi-store unique en v1 — boucle par paires
  + agrégation simple côté CLI (matrice de similarité) à évaluer ensuite.
- L'« intégration MCP » = câbler l'enchaînement dans le pipeline d'import
  (script/procédure d'orchestration) et documenter ; le moteur d'indexation
  MCP lui-même vit hors de ce dépôt.
