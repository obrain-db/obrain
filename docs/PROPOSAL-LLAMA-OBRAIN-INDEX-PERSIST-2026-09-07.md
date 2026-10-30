# Proposition — persistance obrain-indexer → store substrate (couplage obrain↔llama-obrain)

**Date** : 2026-09-07 — branche dédiée `feat/llama-obrain-index-persist` (tirée de `main`, main laissée propre).
**But** : le chaînon manquant du couplage obrain↔llama-obrain sans CBM : `obrain-indexer` (M6)
produit un graphe EN MÉMOIRE (`IndexOutput.graph`, `index.rs` : « l'écriture substrate est une étape
ultérieure — RFC M6 ») ; il faut le **persister dans un store substrate obrain** consultable par
`obrain bridge` (RO) pour alimenter search_*/relief/facts → reconstruction d'état KV → injection.

## Design
1. Nouvel exemple/binaire dans `crates/obrain-indexer` : `persist <repo> <out.obrain>`
   (`IndexOptions::default` + écriture du graphe).
2. Mapping graphe indexé → substrate (engine `main`) :
   - chaque nœud symbole → nœud substrate avec label par `NodeKind` (Function/File/…), `qualified_name`
     « <repo>.<path>.<name> » (cf. bench_queries `linux-6.6.kernel.kthread.cleaner_kthread`), propriétés
     texte utile à l'extraction d'état : `file_path`, `signature`/docstring, body (`text`) ;
   - arêtes : CALLS/… par `EdgeKind` (mêmes types que l'index) ;
   - volume drivers/net attendu : ~100 k fonctions / 5 400 fichiers → écriture bulk (WAL group,
     checkpoint) — pas un nœud/s par commit.
3. Vérification : `obrain bridge --store <out.obrain>` RO — counts par label, `search_graph`/BM25 sur
   `Function`, fenêtre/relief exploitables ; puis adapter llama-obrain (insights → textes → état KV RAM).
4. Cible d'index par défaut : `/tmp/linux-6.6/drivers/net` (rapide, représentatif) ; racine possible ensuite.

## DoD (8 cases)
1. `persist` produit un store ouvrable (bridge RO OK) ; counts == graphe en mémoire (parité nœuds/arêtes).
2. `qualified_name` stable & conforme (« repo.path.name ») ; props texte présentes pour extraction d'état.
3. Test de parité : index en mémoire vs store — mêmes comptes NodeKind/EdgeKind (≥ un sous-ensemble vérifié).
4. Bench daté : temps index vs persist (drivers/net, CPU) ; taille store.
5. Store réel sur drivers/net construit et interrogé (search_graph « kthread »/« net_device » etc.).
6. Aucune régression engine (suites 683/735/823/870 vertes sur main inchangée) ; commits réversibles.
7. Docs (cette proposition + état pont) + graphe.
8. Couplage de bout en bout (question complexe → insights obrain → injection état → réponse + TFPT/tok-s/cohérence) sur CPU.
