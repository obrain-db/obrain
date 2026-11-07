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
| **Écriture multi-processus** sur un même WAL (`open_shared_write`) : **zéro perte et EOF exact** tiennent (6 000 frames × 172 runs), mais l'**exactitude du LSN** (`lsn == ancre + position`) ne tient pas encore sous préemption. Avant correctif, mesuré le 2026-11-01 : 3 échecs sur 172 à 25 ms (dont un sur le runner GitHub), 4 sur 31 avec le délai forcé à 100 µs (`t2c`). Cause : un écrivain préempté plus de `DELAI_TETE_BLOQUEE` (25 ms) était pris pour mort par son successeur, qui ré-ancrait la chaîne ; la frame du préempté était alors posée après celle du successeur avec un LSN inférieur à sa position. CORRECTIF V1-T2B (#64, preuve de vie : pid + heure de démarrage dans un emplacement du registre, `wal_presence.rs`) : sous charge CPU (32 boucles), `t2c` passe de 34 échecs sur 40 à 0 sur 40, `t2b` de 10 sur 40 à 2 sur 40 (échecs résiduels sur le compteur `courses`, cause non isolée). Une seconde cause, un faux positif de troncature (un pair qui écrit entre la mesure de la taille et la lecture de la fin publiée), est corrigée par #66 : sur une sonde de 300 µs, `t1` échoue 20/20 avant et 0/20 après (chiffres du message de commit, non rejoués ici). Le mode partagé n'est PAS encore une garantie de la v0.0.1 : `t2b`/`t2c` restent `#[ignore]` (`mesures.yml`). | mesurée | `obrain-substrate/tests/mw_interprocess_append.rs`, `wal_io.rs::attendre_la_publication`, `wal_io.rs::troncature_averee` (tests `un_pair_qui_ecrit_entre_deux_lectures_n_est_pas_une_troncature`, `une_vraie_troncature_reste_avérée`), `tests/wal_chaine_cas.rs::t1_quatre_processus_lsn_exact_et_ordre_physique` |
| **Couche simpliciale au crash** : après un `kill -9` pendant l'écriture, un record du répertoire de simplexes peut exister sans ses postings d'incidence. Observé une fois sur le runner GitHub (01/11/2026), 21/21 verts localement : fenêtre rare, dépendante de l'ordonnancement. Hors du chemin nœuds/arêtes/propriétés. CORRIGÉE par #65 : le rejeu complète les postings d'un record vivant déjà présent (`SimplexIncidenceZone::ensure_posted`, `replay.rs`). Le test `rejeu_repose_l_incidence_d_un_record_deja_present` simule cet état post-crash (postings effacés) ; `kill_9_pendant_ecriture_reouverture_saine` n'a pas été modifié et reste une mesure. | corrigée (#65) | `obrain-substrate/tests/simplex_durability.rs` : `rejeu_repose_l_incidence_d_un_record_deja_present`, `kill_9_pendant_ecriture_reouverture_saine` (non ignoré : s'il rougit, c'est une mesure) |
| `schema_crc32` de `substrate.meta` est écrit mais jamais vérifié à la lecture. | mesurée (relu 2026-11-01) | — |
| Les octets `reserved` de `substrate.meta` ne sont pas vérifiés à zéro (la spec le promettait). | mesurée | — |
| Un `substrate.wal` non checkpointé n'est pas lisible par un binaire ANTÉRIEUR qui ignore un opcode (0x54–0x58) : **checkpoint forcé avant tout downgrade**. | verrouillée par la règle d'exploitation | `slot_remap.rs` § « Retrograder un binaire » |
| Aucun fuzzing du décodeur WAL ni du parseur GQL. | mesurée | — |
| Les écritures par l'API directe (`ObrainDB::set_node_property`, `create_node`, …) sont hors transaction (`TransactionId::SYSTEM`) : elles ne participent pas à la détection de conflit write-write des sessions et sont commitées à l'instant. Dans une session, `SET`/`INSERT` passent par le planificateur et sont suivis. | mesurée (relu 2026-11-01, `transaction/manager.rs`, `store.rs` create_node_versioned) | `echec_commit_rembobinage.rs` (conflit détecté via SET en session) |

| **Feature `temporal`** (historique des propriétés par epoch, opt-in, absente du README) : **non fonctionnelle sous substrat**. Mesuré le 2026-11-01 (`cargo test -p obrain-engine --features temporal`) : `temporal_properties.rs` 6 rouges sur 8 — une lecture « at epoch_v1 » rend la valeur courante au lieu de l'historique ; `error_paths::test_property_with_null_value` (branche temporal) rend `Some(Null)` au lieu de `None`. L'historique par epoch n'est pas conservé par le substrat. Ne pas activer. | mesurée | `obrain-engine/tests/temporal_properties.rs` |
| **Feature `rdf`** (triplets, `sparql`) : **non persistante**. Mesuré le 2026-11-01 (`--all-features`) : `wal_recovery.rs::wal::test_rdf_{triples,named_graph,delete}_persist_across_restart` rouges — 0 triplet après réouverture au lieu de 2 : le store RDF n'est ni journalisé ni rejoué par le WAL du substrat. En mémoire seulement. | mesurée | `obrain-engine/tests/wal_recovery.rs` |
| **Écriture multi-processus** — voir ci-dessous. Et, corrigé par la batterie exhaustive : un second `ObrainDB` **inscriptible** ouvert sur la même base **dans le même processus** était accepté (deux instances, sidecars divergents) ; il est maintenant refusé (`open_substrate_inner`). Une ouverture en lecture seule reste permise. | corrigée | `obrain-engine/src/database/mod.rs` |
| **Retrieval vectoriel par paliers** (L0 128 bits → L1 512 bits Hamming → L2 f16, k0 = 1 000, k1 = 100) : recall@10 **mesuré 0,365** sur 2 000 vecteurs 384-d **aléatoires** (quasi orthogonaux : hors contrat d'une quantification binaire) ; le contrat est mesuré sur vecteurs structurés par `agent_memory_migration::recall_sur_vecteurs_structures_2k` (seuil 0,80). Pas d'index HNSW sous substrat. | mesurée | `obrain-substrate/src/retrieval.rs`, `tiered_scan.rs` |

## Langage de requête

Trois `#[ignore]` dont la raison était périmée (tests verts sur `main` e11bdd2a le 2026-11-01) sont retirés par la PR « désignorer » : `session_transaction_persists`, `t2b_lsn_exact_par_verrou_d_append`, `foreach_set_on_matched_nodes`. Le second est **ré-ignoré le soir même** avec une mesure neuve (voir « Écriture multi-processus » ci-dessus) : une mesure verte unique n'était pas une preuve.

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
| Aucun binding (Python, Node, Go, C#, Dart) dans ce dépôt. WebAssembly : seul `crates/obrain-web` existe (wrapper `wasm-bindgen`, `publish = false`, paquet d'essai). Seuls `packages/obrain-cli-{npm,python}` sont des wrappers du CLI. | mesurée (relu 2026-11-07) | `crates/obrain-web/Cargo.toml` ; README § Roadmap (ligne 136, à mettre à jour : cite WebAssembly comme absent) |
| `CALL obrain.hybrid_search` n'existe pas comme procédure GQL (les index texte/vecteur vivent dans `ObrainDB`, hors du planificateur de `CALL` ; l'enchaînement `CALL … YIELD … MATCH` n'est pas parsé). La recherche hybride existe en Rust : `ObrainDB::hybrid_search`. | mesurée (décision 5735aa32) | README § Roadmap |
| **Persistance navigateur sur OPFS** : `open_browser` / `persist` passent par le snapshot, **le WAL n'est pas sur OPFS** (décision C, non traitée pour la v0.0.1). Les écritures faites après le dernier `persist()` ne sont pas garanties : comportement non mesuré. | non mesurée | `crates/obrain-web` (suite Chrome headless) |

## Performance

| Limite | État | Verrou |
|---|---|---|
| Les chiffres publiés ne sont reproductibles que par les benchs listés dans `docs/BENCHMARKS.md` ; le tableau LDBC dépend d'un outil externe (`graph-bench`) non versionné ici. | à produire en V1-PERF | `docs/BENCHMARKS.md` |
| **Régression mesurée de l'insertion d'un nœud** (auto-commit, base en mémoire) : **+18 à +21 %** entre `e11bdd2a` et main sur 5 tours sur 6 (11,2 µs → 13,4 µs), **sous charge 2,2 à 5,1**. Non attribuée : le commit fautif n'est pas recherché. Pas une erreur de justesse ni de durabilité. | provisoire (mesure entrelacée, charge > 2) | `docs/BENCHMARKS.md` |
