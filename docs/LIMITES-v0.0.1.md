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
| **Écriture partagée : LSN exact sous préemption** | verrouillée (vert sur 3 000 exécutions) | Mesuré le 07/11/2026 sous 32 boucles CPU : `t2b` 3 000/3 000 et `t2c` 3 000/3 000, 0 échec, sur le commit 7f1dd9bb (PR #73, #75). Avant correctif (#64, #66, #73) : 2 échecs sur 40 pour `t2b`, 34 sur 40 pour `t2c`, tous sur le compteur `courses`. | `obrain-substrate/tests/mw_interprocess_append.rs` (`t2b`, `t2c`, `t2d`), `tests/wal_chaine_cas.rs` (`la_creation_du_registre_ne_devance_pas_une_publication`) |
| **Écriture partagée : propriétés et tas** | non garantie v0.0.1 | Deux processus qui posent des propriétés partagent la même page de propriétés et la même queue de tas, dont les curseurs sont par processus (`props_zone.rs`) : une page peut être écrasée. Prévu en v0.0.2 (MP-3). | — |
| **Écriture partagée : transactions concurrentes** | non garantie v0.0.1 | L'identité de transaction est choisie par processus : la transaction d'un processus mort est repérée par une identité hachée sur 31 bits, collision résiduelle d'environ 2^-31 par paire (`open_shared_write`). Un checkpoint qui tombe pendant une transaction d'un pair ne la tronque pas (la troncature est refusée avec un `warn`). Compteur partagé et scellement des abandons prévus en v0.0.2 (MP-10), propriétés et transactions concurrentes en MP-4. | `obrain-substrate/tests/mp8_fenetre_rejeu.rs`, `obrain-engine/tests/ecriture_multi_processus.rs` |
| **Écriture partagée : visibilité et isolation** | non garantie v0.0.1 | Un processus ne voit les écritures d'un pair qu'à sa réouverture. Pas d'isolation ni de conflit d'écriture entre processus (`TransactionManager` et `en_attente_noeuds` sont par processus). | — |
| **Écriture partagée : WAL non tronquable après abandon** | limite connue | Un processus mort qui laisse une transaction non scellée empêche la troncature du WAL jusqu'au prochain redémarrage propre ; un `warn` est journalisé. Croissance de taille, pas perte de données. | `obrain-substrate/tests/mp8_fenetre_rejeu.rs` |
| **Couche simpliciale au crash** : après un `kill -9` pendant l'écriture, un record du répertoire de simplexes peut exister sans ses postings d'incidence. Observé une fois sur le runner GitHub (01/11/2026), 21/21 verts localement : fenêtre rare, dépendante de l'ordonnancement. Hors du chemin nœuds/arêtes/propriétés. CORRIGÉE par #65 : le rejeu complète les postings d'un record vivant déjà présent (`SimplexIncidenceZone::ensure_posted`, `replay.rs`). Le test `rejeu_repose_l_incidence_d_un_record_deja_present` simule cet état post-crash (postings effacés) ; `kill_9_pendant_ecriture_reouverture_saine` n'a pas été modifié et reste une mesure. | corrigée (#65) | `obrain-substrate/tests/simplex_durability.rs` : `rejeu_repose_l_incidence_d_un_record_deja_present`, `kill_9_pendant_ecriture_reouverture_saine` (non ignoré : s'il rougit, c'est une mesure) |
| `schema_crc32` de `substrate.meta` est écrit mais jamais vérifié à la lecture. | mesurée (relu 2026-11-01) | — |
| Les octets `reserved` de `substrate.meta` ne sont pas vérifiés à zéro (la spec le promettait). | mesurée | — |
| Un `substrate.wal` non checkpointé n'est pas lisible par un binaire ANTÉRIEUR qui ignore un opcode (0x54–0x58, ou tout opcode futur) : la lecture n'est pas possible, mais elle est **refusée en nommant l'opcode** (`SubstrateError::WalOpcodeInconnu`, « opcode WAL inconnu 0x7E à l'offset N … »), jamais rendue comme une base vide ou partielle, et le WAL n'est ni rejoué partiellement, ni tronqué, ni modifié par la tentative. Avant le 2026-11-07, l'opcode inconnu était confondu avec une queue déchirée : ouverture sur un préfixe, puis troncature du WAL au checkpoint suivant. La règle d'exploitation « checkpoint avant tout downgrade » reste la façon de rendre la base lisible par l'ancien binaire ; elle n'est plus ce qui protège les données. | garantie, verrouillée par `wal_opcode_inconnu.rs` (substrat) et `format_fige_v3.rs` (moteur, fixture `golden/v3-mini-opcode-futur`) | `obrain-substrate/tests/wal_opcode_inconnu.rs` : `un_opcode_inconnu_refuse_l_ouverture_en_le_nommant_et_laisse_le_wal_intact`, `une_queue_dechiree_avant_le_kind_reste_une_frontiere_de_crash`, `une_frame_inconnue_tronquee_apres_le_kind_reste_une_frontiere_de_crash` ; `obrain-engine/tests/format_fige_v3.rs` : `un_opcode_wal_inconnu_refuse_l_ouverture_par_le_moteur_et_laisse_le_wal_intact`, `la_fixture_v3_avec_un_opcode_futur_est_refusee_en_le_nommant` |
| Aucun fuzzing du décodeur WAL ni du parseur GQL. | mesurée | — |
| Les écritures par l'API directe (`ObrainDB::set_node_property`, `create_node`, …) sont hors transaction (`TransactionId::SYSTEM`) : elles ne participent pas à la détection de conflit write-write des sessions et sont commitées à l'instant. Dans une session, `SET`/`INSERT` passent par le planificateur et sont suivis. | mesurée (relu 2026-11-01, `transaction/manager.rs`, `store.rs` create_node_versioned) | `echec_commit_rembobinage.rs` (conflit détecté via SET en session) |

| **Feature `temporal`** (historique des propriétés par epoch, opt-in, absente du README) : **non fonctionnelle sous substrat**. Mesuré le 2026-11-01 (`cargo test -p obrain-engine --features temporal`) : `temporal_properties.rs` 6 rouges sur 8 — une lecture « at epoch_v1 » rend la valeur courante au lieu de l'historique ; `error_paths::test_property_with_null_value` (branche temporal) rend `Some(Null)` au lieu de `None`. L'historique par epoch n'est pas conservé par le substrat. Ne pas activer. | mesurée | `obrain-engine/tests/temporal_properties.rs` |
| **Feature `rdf`, écritures dans une transaction explicite** (`BEGIN` … `COMMIT`) : **non journalisées**. Les triplets sont visibles dans la session après `commit`, mais absents après réouverture. Mesuré le 2026-11-08 (insert puis commit dans une transaction, réouverture : 0 triplet au lieu de 1). Les écritures en autocommit sont persistantes (`wal_recovery.rs::test_rdf_*`). Les écritures directes par `db.rdf_store()` ne sont pas journalisées non plus. | mesurée | sonde ad hoc, non commitée |
| **Écriture multi-processus** — voir ci-dessous. Et, corrigé par la batterie exhaustive : un second `ObrainDB` **inscriptible** ouvert sur la même base **dans le même processus** était accepté (deux instances, sidecars divergents) ; il est maintenant refusé (`open_substrate_inner`). Une ouverture en lecture seule reste permise. | corrigée | `obrain-engine/src/database/mod.rs` |
| **Retrieval vectoriel par paliers** (L0 128 bits → L1 512 bits Hamming → L2 f16, k0 = 1 000, k1 = 100) : recall@10 **mesuré 0,365** sur 2 000 vecteurs 384-d **aléatoires** (quasi orthogonaux : hors contrat d'une quantification binaire) ; le contrat est mesuré sur vecteurs structurés par `agent_memory_migration::recall_sur_vecteurs_structures_2k` (seuil 0,80). Pas d'index HNSW sous substrat. | mesurée | `obrain-substrate/src/retrieval.rs`, `tiered_scan.rs` |

## Persistance navigateur (obrain-web, OPFS)

| Limite | État | Verrou |
|---|---|---|
| **Décision C : pas de WAL sur OPFS en v0.0.1.** La durabilité navigateur est celle de la dernière publication (`persister()`). Une écriture non publiée n'atteint pas OPFS et disparaît à la fermeture, au rechargement ou au crash. Le WAL est reporté après v0.0.1. | verrouillée | `crates/obrain-web/tests/navigateur.rs` : `les_ecritures_apres_la_derniere_publication_sont_perdues` (Chrome headless, CI « Persistance navigateur ») ; `obrain-substrate/src/miroir.rs` : `les_ecritures_apres_la_publication_n_atteignent_pas_le_journal` |
| Une publication coupée en cours ouvre la génération précédente (une base vide si c'était la première), jamais une erreur ni un mélange. Mesuré sur le format de journal réel, coupures réparties sur toute la publication. | verrouillée | `obrain-substrate/src/miroir.rs` : `un_persist_coupe_ouvre_une_generation_complete` |
| Une publication réussie remplace la précédente ; l'état relu est le dernier publié. | verrouillée | `crates/obrain-web/tests/navigateur.rs` : `une_seconde_publication_remplace_la_premiere` |
| Chaque publication écrit **deux** images complètes dans un journal jamais compacté. Le plafond de 2 Gio porte sur le journal cumulé, pas sur la base : une base dont l'image pèse S octets ne peut être republiée qu'environ 1 Gio / S fois. | mesurée | `obrain-substrate/src/miroir.rs` : `chaque_publication_ajoute_deux_images_au_journal` |
| Une corruption **au milieu** du journal (ce n'est pas une coupure) n'est pas détectée : la reprise tronque le fichier au dernier enregistrement valide, et la publication suivante, intacte, est effacée. Le store rouvert est plus ancien que la dernière publication, sans erreur. | mesurée | `obrain-substrate/src/miroir.rs` : `une_corruption_au_milieu_efface_les_publications_suivantes` |

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
