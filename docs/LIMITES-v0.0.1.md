# Limites connues de la v0.0.1

Chaque ligne est MESURÉE (un test la verrouille, ou une mesure datée la
documente). Une limite sans test n'a pas sa place ici ; une fonctionnalité
absente de cette liste et absente du README n'existe pas en v0.0.1.

Marqueurs : **verrouillée** = un test rouge-si-corrigé ou vert-si-respectée
existe ; **mesurée** = chiffre daté, pas encore de test de non-régression.

## Intégrité des données

| Limite | État | Verrou |
|---|---|---|
| Après un **remap de slots** (`repermuter_par_hilbert`), la première écriture sur un nœud déplacé détache sa chaîne de propriétés v2 (perte des propriétés pré-remap). Constat du 2026-09-06, classé P0, **confirmé rouge le 2026-11-01**. Portée réelle : le moteur n'appelle jamais le remap ; seul le warden du substrat (`warden.rs`) le déclenche, et le moteur ne démarre aucun warden. Hors d'atteinte en usage v0.0.1 ; à corriger AVANT d'activer les wardens. | mesurée | `obrain-substrate/src/store.rs` : `une_prop_ecrite_apres_un_remap_de_slot_ne_perd_pas_la_chaine` (`#[ignore]`) |
| Des **ancres de propriétés divergentes** (index v2 vs chaîne) peuvent rendre un résultat amputé. Dette héritée du 2026-09-13, **confirmée rouge le 2026-11-01**. | mesurée | `store.rs` : `des_ancres_divergentes_ne_rendent_jamais_un_resultat_ampute` (`#[ignore]`) |
| **Écriture multi-processus** sur un même WAL (`open_shared_write`) : fonctionne dans les tests (`t2b` vert le 2026-11-01, son `#[ignore]` « 1 876 écarts de LSN » était périmé), mais le mode partagé n'est PAS une garantie documentée de la v0.0.1. | mesurée | `obrain-substrate/tests/mw_interprocess_append.rs` |
| **Couche simpliciale au crash** : après un `kill -9` pendant l'écriture, un record du répertoire de simplexes peut exister sans ses postings d'incidence. Observé une fois sur le runner GitHub (01/11/2026), 21/21 verts localement : fenêtre rare, dépendante de l'ordonnancement. Hors du chemin nœuds/arêtes/propriétés. | mesurée (note PO 8e369792) | `obrain-substrate/tests/simplex_durability.rs` : `kill_9_pendant_ecriture_reouverture_saine` (non ignoré : s'il rougit, c'est une mesure) |
| `schema_crc32` de `substrate.meta` est écrit mais jamais vérifié à la lecture. | mesurée (relu 2026-11-01) | — |
| Les octets `reserved` de `substrate.meta` ne sont pas vérifiés à zéro (la spec le promettait). | mesurée | — |
| Un `substrate.wal` non checkpointé n'est pas lisible par un binaire ANTÉRIEUR qui ignore un opcode (0x54–0x58) : **checkpoint forcé avant tout downgrade**. | verrouillée par la règle d'exploitation | `slot_remap.rs` § « Retrograder un binaire » |
| Aucun fuzzing du décodeur WAL ni du parseur GQL. | mesurée | — |
| Les écritures par l'API directe (`ObrainDB::set_node_property`, `create_node`, …) sont hors transaction (`TransactionId::SYSTEM`) : elles ne participent pas à la détection de conflit write-write des sessions et sont commitées à l'instant. Dans une session, `SET`/`INSERT` passent par le planificateur et sont suivis. | mesurée (relu 2026-11-01, `transaction/manager.rs`, `store.rs` create_node_versioned) | `echec_commit_rembobinage.rs` (conflit détecté via SET en session) |

## Langage de requête

Trois `#[ignore]` dont la raison était périmée (tests verts sur `main` e11bdd2a le 2026-11-01) sont retirés par la PR « désignorer » : `session_transaction_persists`, `t2b_lsn_exact_par_verrou_d_append`, `foreach_set_on_matched_nodes`.

| Limite | État | Verrou |
|---|---|---|
| Deux `FOREACH` séquentiels dans une requête : le second voit les lignes du premier (6 lignes au lieu de 4). | à trancher en V1-IGNORES | `obrain-engine/tests/spec_compliance.rs` : `gql_foreach_multiple_in_query` |
| `t9_planner_rewrite_correctness::row_correctness_single_direction_no_peer_label` compare le planificateur à une référence lente sur un **jeu de données local** (store PO d'une machine de développement) : rouge hors de cette machine, non reproductible. | mesurée (2026-11-01) | `#[ignore]`, à réécrire sur une fixture |
| Cypher, SPARQL, Gremlin, GraphQL, SQL/PGQ : derrière des features non activées par défaut, non couverts par la CI par défaut. | mesurée (ci.yml) | job `features` (compilation seule) |

## Plateformes

| Limite | État | Verrou |
|---|---|---|
| Un refus de verrou nomme le processus détenteur sur Linux (`/proc`) et macOS (`lsof`) ; ailleurs, pas de nom. | verrouillée | `obrain-substrate/tests/verrou_apres_drop.rs` |
| Windows : aucune CI ; `cfg(windows)` présent dans 5 sites non compilés par la CI. | mesurée | — |

## Surface

| Limite | État | Verrou |
|---|---|---|
| Aucun binding (Python, Node, Go, C#, WASM, Dart) dans ce dépôt : seuls `packages/obrain-cli-{npm,python}` (wrappers du CLI). | mesurée (relu 2026-11-01) | README § Roadmap |
| `CALL obrain.hybrid_search` n'existe pas comme procédure GQL (les index texte/vecteur vivent dans `ObrainDB`, hors du planificateur de `CALL` ; l'enchaînement `CALL … YIELD … MATCH` n'est pas parsé). La recherche hybride existe en Rust : `ObrainDB::hybrid_search`. | mesurée (décision 5735aa32) | README § Roadmap |

## Performance

| Limite | État | Verrou |
|---|---|---|
| Les chiffres publiés ne sont reproductibles que par les benchs listés dans `docs/BENCHMARKS.md` ; le tableau LDBC dépend d'un outil externe (`graph-bench`) non versionné ici. | à produire en V1-PERF | `docs/BENCHMARKS.md` |
