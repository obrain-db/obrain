# Limites connues de la v0.0.1

Chaque ligne est MESURÉE (un test la verrouille, ou une mesure datée la
documente). Une limite sans test n'a pas sa place ici ; une fonctionnalité
absente de cette liste et absente du README n'existe pas en v0.0.1.

Marqueurs : **verrouillée** = un test rouge-si-corrigé ou vert-si-respectée
existe ; **mesurée** = chiffre daté, pas encore de test de non-régression.

## Intégrité des données

| Limite | État | Verrou |
|---|---|---|
| Après un **remap de slots** (repermutation Hilbert), la première écriture sur un nœud déplacé peut détacher sa chaîne de propriétés v2 (perte des propriétés pré-remap). Constat du 2026-09-06, classé P0. | à trancher en V1-IGNORES (corriger ou retirer la repermutation du chemin v0.0.1) | `obrain-substrate/src/store.rs` : `une_prop_ecrite_apres_un_remap_de_slot_ne_perd_pas_la_chaine` (`#[ignore]`) |
| **Écriture multi-processus** sur un même WAL (`open_shared_write`) : le LSN n'est pas exactement la position (1 876 écarts sur 6 000 trames mesurés). Le mode partagé n'est PAS une garantie de la v0.0.1. | mesurée | `obrain-substrate/tests/mw_interprocess_append.rs` : `t2b_lsn_exact_par_verrou_d_append` (`#[ignore]`) |
| `schema_crc32` de `substrate.meta` est écrit mais jamais vérifié à la lecture. | mesurée (relu 2026-11-01) | — |
| Les octets `reserved` de `substrate.meta` ne sont pas vérifiés à zéro (la spec le promettait). | mesurée | — |
| Un `substrate.wal` non checkpointé n'est pas lisible par un binaire ANTÉRIEUR qui ignore un opcode (0x54–0x58) : **checkpoint forcé avant tout downgrade**. | verrouillée par la règle d'exploitation | `slot_remap.rs` § « Retrograder un binaire » |
| Aucun fuzzing du décodeur WAL ni du parseur GQL. | mesurée | — |
| Les écritures par l'API directe (`ObrainDB::set_node_property`, `create_node`, …) sont hors transaction (`TransactionId::SYSTEM`) : elles ne participent pas à la détection de conflit write-write des sessions et sont commitées à l'instant. Dans une session, `SET`/`INSERT` passent par le planificateur et sont suivis. | mesurée (relu 2026-11-01, `transaction/manager.rs`, `store.rs` create_node_versioned) | `echec_commit_rembobinage.rs` (conflit détecté via SET en session) |

## Langage de requête

| Limite | État | Verrou |
|---|---|---|
| Deux `FOREACH` séquentiels dans une requête : le second voit les lignes du premier (6 lignes au lieu de 4). | à trancher en V1-IGNORES | `obrain-engine/tests/spec_compliance.rs` : `gql_foreach_multiple_in_query` |
| `FOREACH … SET` sur des valeurs d'`UNWIND` ne persiste pas (identité de nœud perdue). | à trancher en V1-IGNORES | `spec_compliance.rs` : `foreach_set_on_matched_nodes` |
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
| La façade `obrain` n'expose pas de feature `cognitive` : les procédures `obrain.cognitive.*` et les UDF `obrain.energy/risk/synapses` ne sont accessibles que depuis `obrain-engine` avec `--features cognitive`. | à confirmer par `crates/obrain/tests/readme_examples.rs` | harnais README |

## Performance

| Limite | État | Verrou |
|---|---|---|
| Les chiffres publiés ne sont reproductibles que par les benchs listés dans `docs/BENCHMARKS.md` ; le tableau LDBC dépend d'un outil externe (`graph-bench`) non versionné ici. | à produire en V1-PERF | `docs/BENCHMARKS.md` |
