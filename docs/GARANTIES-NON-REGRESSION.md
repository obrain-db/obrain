# Garantir la non-régression — matrice de portes (bench / features / exactitude de requête)

**Date** : 2026-09-05 — branche `feat/zorder`. Réponse opérationnelle à
« comment garantir la non-régression ? » : chaque niveau a UNE commande
rejouable et des compteurs datés au ledger.

## 1. Matrice de portes (commandes)

| Niveau | Commande | Couvre |
|---|---|---|
| Unités par crate (rapides) | `cargo test -p obrain-core --lib` · `-p obrain-substrate --lib` · `-p obrain-engine --lib` · `-p obrain-cli --bin obrain` | logique interne, formats, caches |
| Engine — surfaces IA/RAG | `cargo test -p obrain-engine --tests --features cdc,algos` | CDC, algorithmes, nos formes (search_*), fenêtres |
| Engine — TOUTES les cibles | `cargo test -p obrain-engine --tests --no-fail-fast` (défaut puis `--features cdc,algos,ai`) | inventaire COMPLET : compile-échecs et tests rouges visibles en une passe (cargo test est fail-fast par défaut → toujours `--no-fail-fast` pour l'inventaire) |
| CI dépôt (canon) | `cargo test --all-features --workspace` (cf. `.github/workflows/ci.yml`) | toutes langues, index, RDF, CDC, algos ensemble |
| Bench/gains | `bash scripts/bench-verify/verify-imports-compare.sh` + verify-o1/engine | micro/milli secondes par op, égalité exacte zruns==scan |

## 2. Exactitude de requête — filets

- Conformité GQL : `gql_spec_compliance.rs` + fichiers `gql_*` (défauts
  connus = liste ROUGE légitime sous `--features defauts-connus`, documentés
  fichier:ligne — « la liste vivante des défauts », pas une régression) ;
- **Propriétés vs oracles indépendants** (filets ajoutés 2026-09-05) :
  `compare` structurel==complet (40 graines) ; `search_conflicts` (30) ;
  `search_surprise` (10, égalité exacte <1e-6) ; fenêtres 2D zruns==scan==
  oracle (250 requêtes aléatoires) ; parités de caches (empreintes, arêtes) ;
- Trie/queue exacts : fenêtres `exact==sorted` + tri ascendant vérifié ;
  tri |surprise| décroissant vérifié ; order/limit/distinct GQL couverts par
  la conformité.

## 3. Règle maison (safe / fast / bench)

Pas de changement sans (a) portes du niveau concerné vertes, (b) bench daté
avant/après quand il y a un coût, (c) un test qui aurait attrapé la
régression. Chaque chantier = commit réversible + ligne ledger.

## 3 bis. Règle d'HERMÉTISME (2026-09-17) — la porte ne dépend pas du poste

Défaut mesuré le 2026-09-16 : `cargo test --workspace` donnait **45 échoués /
18 cibles rouges** sur un poste où `OBRAIN_IAM*` et `OBRAIN_STORE` étaient armés,
et **26 échoués / 14 cibles** en environnement propre. **19 échecs étaient le
fait de la machine**, pas du code — un test qui dépend de l'environnement de
celui qui le lance ne prouve rien.

Trois règles, désormais tenues par du code (et non par la discipline) :

1. **L'IAM est désarmé dans les tests unitaires** : `GardeIam::desarmer_pour_les_tests()`
   (garde inerte posée une fois, `OnceLock`) — les tests du pont passent avec
   `OBRAIN_IAM=1` dans l'environnement.
2. **Un processus lancé par un test n'hérite PAS de l'IAM du poste**
   (`env_remove` sur l'enfant, jamais sur le test).
3. **Aucune variable d'environnement du processus n'est mutée par un test** pour
   se donner un comportement : on passe par un forçage local
   (`OVERRIDE_METRIQUES`) — la mutation d'environnement concurrente n'est pas
   définie.
4. **Les portes de performance sont ARMÉES explicitement** (`OBRAIN_PERF_GATE=1`) :
   non armées, elles mesurent, écrivent leur rapport et **disent** qu'elles ne
   jugent pas (un temps mural mesure la machine autant que le code) ; armées,
   elles échouent normalement.

### 3 ter. Audit d'hermétisme, refait avec le bon motif (2026-09-17)

> **Premier audit FAUX, et pourquoi** : il comptait `set_var|remove_var`, motif qui
> attrape aussi `offset_var` et `remove_variable` — il a donc désigné cinq fichiers
> de traduction/planification comme « muteurs d'environnement » qui n'en étaient
> pas. Le motif exact est `env::set_var` / `env::remove_var`. Un audit qui
> désigne des innocents coûte plus cher qu'aucun audit : on corrige du code sain.

Inventaire exact (`grep -rE "env::(set_var|remove_var)"`) :

| fichier | mutations | état |
|---|---|---|
| `obrain-substrate/src/store.rs` | 151 | verrou pris (393 occurrences) ; le test `sonde_v3_replay`, qui lisait un régime posé par un autre, prend le verrou depuis le 17/09 |
| `obrain-substrate/src/file.rs` | 15 | verrou pris (62) — la proportion de l'audit précédent (5) était fausse |
| `obrain-migrate/src/converter.rs` | 6 | verrou pris (21) — crate séparé, pas de partage de processus |
| `obrain-engine/tests/contient_intention.rs` | 4 | **corrigé (17/09)** : verrou local ; la justification « test mono-thread » du fichier était FAUSSE |
| `obrain-engine/tests/existence_indexe.rs` | 2 | **corrigé (17/09)** : verrou local — un test affirmait `slots == 0` pendant qu'un autre posait le plafond à 0 |
| `obrain-engine/tests/t9_planner_rewrite_correctness.rs` | 2 | **corrigé (17/09)** : verrou dans `run_twice` ; « no concurrent env access » était faux |
| `obrain-engine/tests/contrat_j5b_planificateur.rs` | 0 | **plus aucune mutation** depuis que le bouton mort a été retiré (défaut 24) et le test réécrit |
| `obrain-substrate/tests/t17k_get_node_bulk_hydrate.rs` | 1 | **corrigé (17/09)** : verrou local |
| `obrain-engine/tests/t17k_cypher_where_synthetic.rs` | 1 | **corrigé (17/09)** : verrou local |
| `obrain-cli/src/main.rs` | 2 | **faux positif, vérifié** : `NO_COLOR` est posé au DÉMARRAGE, avant tout thread — sa justification « called once at startup before any threads are spawned » est CORRECTE. Aucun verrou n'est nécessaire, et en ajouter un serait du bruit |

**Ce qui reste, nommé** : aucun fichier ne mute plus l'environnement sans
sérialisation. La forme cible **n'est pas le verrou mais le seam** — comme
`chemin_metriques()` dans `obrain-cli` (forçage par thread, jamais par
l'environnement) : il laisse la variable pour la production et retire aux tests le
pouvoir de se nuire. Le verrou est le remède proportionné en attendant.

**Trois justifications « single-threaded » fausses** ont été corrigées dans le
même mouvement : `cargo test` exécute les tests d'un MÊME binaire en threads
parallèles, ce que ces commentaires niaient. Un commentaire faux dans le code est
un défaut : il convainc le prochain lecteur que le test est hermétique.

### 3 quater. Le flake de verrou, nommé et borné (2026-09-17)

Deux tests de `simplex_durability` sont tombés, **en suite complète seulement**, sur
un refus de verrou exclusif quelques lignes après avoir fermé le store :

```text
substrate /tmp/.tmp…/kb is already open by another process (exclusive lock on
…/substrate.meta refused: Resource temporarily unavailable)
```

`drop_sans_flush_rejeu_reconstruit` (une passe) puis `wal_dechire_arret_propre`
(une autre). Ce qui a été **tenté** pour le reproduire, et le résultat :

| essai | résultat |
|---|---|
| la cible seule | 6/6 vert |
| la cible × 4 instances en parallèle (2 tours) | 6/6 × 8 |
| sonde dédiée « le verrou est-il rendu au drop ? » — 60 cycles create/écriture/drop/open | **0 refus**, pire attente 72 µs |
| la même sonde × 4 instances en parallèle | **0 refus**, pire attente 810 µs |

Conclusion honnête : **inexpliqué, rare (2 passes sur 6), jamais reproduit hors
suite complète**. Ce n'est donc pas un défaut prouvé — c'est un flake nommé, et
la sonde devenue garde (`crates/obrain-substrate/tests/verrou_apres_drop.rs`)
tient l'invariant qui compte : la libération n'est pas *différée*, elle est
SYNCHRONE (une régression « le mmap retient la description de fichier, donc le
`flock` » ferait tomber cette assertion immédiatement).

**Piste pour la prochaine enquête** : les deux tests fautifs font un `wipe()`
(troncature de fichiers) ENTRE le drop et la réouverture — c'est la seule
différence avec la sonde, qui n'écrit rien entre les deux. À vérifier en premier.

**Une seconde famille de flake, corrigée dans son DIAGNOSTIC (pas masquée).**
`orderby_epoque_j8::instant_quand_a_jour` est tombé sur `ORDER BY n.v DESC LIMIT 1` :
5 slots lus au lieu de 0, c'est-à-dire un balayage au lieu de la route indexée.
Le document l'avait classé « test sensible au temps » et j'ai failli le mettre
derrière la porte de performance — **c'était faux et cela aurait MASQUÉ un défaut
réel** : ce test n'asserte aucun temps mural, il asserte un COMPTEUR DE SLOTS. Une
assertion déterministe qui échoue dit que l'ÉTAT diffère, pas que la machine est
lente.

Ce qui a été exclu par la mesure, et ce qui reste :

| hypothèse | verdict |
|---|---|
| fuite de descripteur / mapping qui retient le `flock` | **exclue** — sonde `verrou_apres_drop` : 0 descripteur après le drop, 0 refus sur 60 cycles |
| collision de noms de dossiers temporaires entre processus | **exclue** — 62⁶ noms, ~0,01 % sur une suite complète |
| test sensible au temps | **exclue** — l'assertion est un compteur, pas un chronomètre |
| route refusée parce que l'index ordonné n'est pas jugé `fiable()` | **écartée par la mesure** : la sonde posée dans `index_ordonne_disponible` n'a jamais été atteinte — le refus vient d'ailleurs |
| **ancres d'index ≠ état courant, au bit près** | **CAUSE TROUVÉE ET CORRIGÉE** — voir ci-dessous |

**LA CAUSE, trouvée par lecture puis REPRODUITE.** Le raccourci `min`/`max` et la
route `ORDER BY … LIMIT k` n'acceptent de servir l'index ordonné que si ses ancres
sont **identiques à l'état courant, au bit près** :

```rust
g.fiable() && g.ancre_wal() == self.writer.wal().offset()
           && g.ancre_pages() == props_zone.allocated_page_count()
```

C'est une exigence SAINE — un `max()` servi par un index en retard serait faux —
mais elle fait de la disponibilité de la route un état **instantané**, invalidé par
la moindre écriture de service (métrique, CDC, époque) entre la construction de
l'index et la requête. Les tests mesuraient plusieurs routes d'affilée après un
seul `rebuild` : la deuxième tombait sur un balayage.

Reproduction : **sous charge CPU** (`yes > /dev/null` × 6), `orderby_epoque_j8`
échoue en isolation — `ORDER BY v DESC LIMIT 1` lisait 5 slots, `max(v)` en lisait 6
(le store porte 5 nœuds). Corrigé en **ré-ancrant l'index juste avant chaque
mesure** (`mesurer_route`), l'assertion restant « 0 slot » : **5/5 vertes sous la
même charge**, là où elles échouaient.

Le jumeau (`voisinage_entrant::preuve_comptable_seek_contre_balayage`) avait la
même faiblesse sous une autre forme : sa « chauffe hors comptage » visait la
structure paresseuse `OnceLock incoming_heads`, mais l'exécution de chauffe n'était
pas garantie d'être celle qui la construit — le compte mesurait alors le balayage
d'INITIALISATION. Corrigé en mesurant le **régime établi** (une exécution hors
comptage, puis la mesure) : mêmes dents, plus d'échauffement compté. 5/5 sous charge.

**Le refus de verrou est désormais INSTRUISIBLE** : quand le `flock` est refusé, le
message nomme le détenteur lu dans `/proc` (`detenteur : pid 1234 (obrain)`), et
`verrou_apres_drop::un_refus_de_verrou_nomme_le_detenteur` prouve la détection avec
un processus enfant qui tient réellement le verrou. Un échec qu'on ne peut pas
instruire coûte une enquête à chaque occurrence ; il en coûtera une de moins.

### 3 quinquies. La non-régression de la passe du 17/09 (17 h)

Trois changements de SÉMANTIQUE ont été faits le 17/09 en fin de journée. Chacun a
été traité comme un risque de régression, pas comme une correction :

| changement | risque | ce qui le couvre |
|---|---|---|
| hydratation des index texte **différée** au lieu d'être reconstruite à l'ouverture | une recherche pourrait ne rien rendre après réouverture | `index_texte_persiste` : 4 tests, dont « après une écriture, l'index est RÉCONSTRUIT » et « l'index chargé et l'index reconstruit donnent le même classement » — la reconstruction paresseuse est exercée par le test d'empreinte invalidée, et il trouve bien le document ajouté |
| le lecteur WAL **s'arrête proprement** sur une queue incomplète | une troncature pourrait devenir SILENCIEUSE — `ReplayStats::decode_errors` documente « 1 si la queue a été déchirée » | le fait est conservé par un **drapeau** (`WalIter::queue_incomplete`) que le rejeu lit pour compter, exactement comme avant ; les tests existants disaient `<= 1`, ce qui ne distinguait PAS le signal préservé du signal perdu → **nouveau test** `crash_safety::une_queue_coupee_au_milieu_d_un_enregistrement_est_comptee` qui exige **exactement 1** sur une coupure construite |
| une queue de **zéros** (pré-allocation) ne lève pas ce drapeau | un faux positif de panne à chaque rejeu d'une base pré-allouée | `wal_io::une_queue_de_zeros_ne_leve_pas_le_drapeau`, avec sa **portée écrite** (reste < 32 octets ; au-delà, un reste de zéros prend un autre chemin, antérieur à cette passe) |

**Preuve de non-régression** : `cargo test --workspace --no-fail-fast` en
environnement propre, quatre passes consécutives → **8 846 · 0**, **8 852 · 0**,
**8 853 · 0**, puis **8 852 · 1** dont l'unique échec n'est PAS de cette passe (voir
ci-dessous). Et l'audit du diff : aucun reste de diagnostic, aucune neutralisation
oubliée, aucun test affaibli — les deux tests de route ont été corrigés **sans**
toucher à leurs assertions (« 0 slot » reste exigé).

**L'unique échec de la quatrième passe est un test d'un AUTRE chantier, et c'est une
garde de temps.** `annulation_bout_en_bout::une_requete_annulee_en_vol_s_arrete_vraiment`
(fichier modifié à 15:57:29 par le balayage `web_time` en cours, jamais touché par
cette passe) compare deux durées murales pour prouver la non-vacuité du mécanisme :
« contre-épreuve » 1,153 s, « annulée en vol » 189 ms, « anti-vacuité » 130 ms — et
exige que la neutralisation RAMÈNE le coût à celui d'une exécution complète. Sous la
charge d'une suite complète, les deux mesures s'inversent (130 ms < 189 ms) et il
tombe. **5 exécutions sur 5 vertes en isolation**, dont une sous charge CPU
(`yes` × 6). Même famille que les autres gardes de temps de ce dépôt : la mesure est
utile, l'assertion ne devrait mordre que si elle est ARMÉE (voir §3 bis). Ce n'est
pas à cette passe de modifier le test d'un chantier en cours ; c'est signalé ici.

## 4. Inventaire rouge — état de référence (à remplir après chaque passe)

- 2026-09-05 (avant correctifs, --no-fail-fast) : rouge = `coverage_memory`,
  `unicite_merge`, `seam_pattern_edge_cases` (cdc,algos) + `call_procedures`
  (gate algos manquant), `error_paths` (défaut).
- 2026-09-05 (après correctifs `c8335340`, --no-fail-fast, 141 binaires) :
  **défaut = 0 rouge · cdc,algos = 0 rouge** (RC 0, les deux passes).
  Correctifs : préfiltre degré typé réservé aux expands OUT (BUG RÉEL :
  MATCH non dirigé sautait les nœuds destination), gates `algos` pour les
  tests CALL, memory_usage honnête (0 revendiqué, pas inventé).
- **FLAKY corrigé** (`ced09dba`) : `unicite_merge::merge_concurrent_avec_
  index_reste_unique` — verrou du composé MERGE find→create→re-query
  (`GraphStoreMut::merge_guard`) : sonde 0/12 (avant 1-2/10).
- 2026-09-05 (après correctif MERGE, --no-fail-fast) : **défaut 141/141 RC 0 ·
  cdc,algos 141/141 RC 0** — inventaire 0 rouge.
- **2026-09-16 (constat, poste de développement)** : `29/08` n'était plus l'état
  réel — **45 échoués / 18 cibles** avec l'environnement du poste, 26/14 en
  environnement propre. Défauts réels trouvés : fuite du store par cycle de
  références CDC (verrou exclusif tenu à vie), série de propriété muette, DDL de
  types non durable, 64 étiquettes qui tuaient le processus, trois tests de
  comparaison inter-types et trois tests `CREATE/DROP INDEX` restés sur des
  contrats retirés par doctrine.
- **2026-09-17 (après correctifs)** — inventaire daté, détail et preuves dans
  `docs/GARDE-TOUR-PLAFOND-REPRISE-2026-09-17.md`. Ce qui était rouge ce
  jour-là, et qui est **corrigé** depuis : `wal_recovery` (4 : DDL de types non
  durable), `property_index_po_bench` (3 : doctrine T17), `coverage_rebrand` /
  `coverage_session` (2 : agrégat dans `WITH` ; `nodes_written`), `search_relief`
  (1 : déduplication CDC qui effaçait des événements), `spec_compliance::parse_
  foreach_after_with` (1), `obrain-cognitive --lib` (2 : caches `synapse`),
  `database_api::test_wal_status_in_memory` (1), plus le défaut de LISTE
  `collect()` dans un `WITH` (typage des projections, 4ᵉ passe).
  **Hermétisme tenu** : les mêmes commandes sont vertes avec l'environnement du
  poste armé.
- **2026-09-17 (fin de journée, état mesuré)** : `cargo test --workspace
  --no-fail-fast` en environnement propre → **8 838 passés · 0 échoué · 0 cible
  rouge** (départ de la journée : 8 769 · 26 · 14). Deux gardes de temps mural
  sont passées derrière `OBRAIN_PERF_GATE=1` (`import_bulk_400k_1600k_non_
  regresse`, la garde de recouvrement d'`index_concurrency`) : mesurées et
  imprimées toujours, exigées seulement porte armée.
  **Un seul point reste hors du vert, et il n'est pas dans les tests** :
  l'emballement d'un `obrain bridge` (§2.7 du document de reprise) — borné par
  un plafond de réponse et nommé par un veilleur, mais le binaire DÉPLOYÉ
  (16/09) ne les porte pas encore.

> NOTE cargo : `cargo test` est FAIL-FAST entre binaires (s'arrête au premier
> rouge) → inventaire toujours en `--no-fail-fast` ; le rouge « 0 défaut »
> inclut aussi la liste vivante `--features defauts-connus` (défauts de
> conformité documentés fichier:ligne — à distinguer des régressions).
