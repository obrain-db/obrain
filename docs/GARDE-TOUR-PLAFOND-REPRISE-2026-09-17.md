# LA GARDE, LE TOUR ET LE PLAFOND — ce qui a été corrigé le 2026-09-17

> Document de reprise du chantier ouvert le 2026-09-16 par la question :
> *« l'engine est-il suffisamment couvert pour garantir de ne pas avoir de
> boucle, prendre en compte tout le contexte de la conversation et des
> connaissances diverses, et laisser la structure évoluer dans le harness voire
> au-delà ? »*
>
> Réponse courte, mesurée : **non sur les trois points**, avec des causes
> précises. Ce document dit ce qui a été cherché, ce qui a été trouvé, ce qui a
> été corrigé, ce qui reste — et **comment le vérifier** (chaque affirmation
> porte un fichier, une ligne, une commande ou un test).

Écrit le 2026-09-17 · branche `consolidation/tronc-2026-09-15-plus-fusion`.

---

## 0. En trois paragraphes

1. **La boucle.** L'arrêt d'une génération vivait dans **un seul** des endroits
   qui parlent à un modèle (`obrain-cli/src/engine.rs`). Les deux boucles de
   `obrain-selfd` — le micro-harness qui AGIT sur la machine — n'en avaient
   **aucune** des trois garanties (répétition, budget, client parti), et le
   client SSE qu'elles utilisent (`obrain-httpmin`) avait un rappel qui rendait
   `()`, donc **incapable d'arrêter la lecture**. Un crate partagé,
   `obrain-garde` (pur, sans dépendance), porte désormais la garantie, et **les
   trois boucles de modèle la traversent** (`obrain-cli/engine.rs`,
   `obrain-selfd/chat.rs`, `obrain-selfd/agent.rs`).
   *L'audit en supposait une QUATRIÈME dans `obrain-selflink` : elle n'existe pas
   — vérifié par `grep`, ce crate est un client HTTP de LECTURE, sans aucun appel
   à un modèle (voir §2.5 bis). La garde y était inutile, et l'écrire était une
   erreur de ce document.*
2. **Le contexte.** La requête de rappel était **une constante de
   configuration** : la mémoire était interrogée avec les mêmes mots à chaque
   tour, quelle que soit la question. Et le bloc injecté, ingéré comme un
   message ordinaire, **ressortait comme « dernier message » des sessions
   rappelées** — le graphe se rappelait sa propre sortie. Les deux sont
   corrigés, et le second est vérifié sur un store réel.
3. **La structure.** L'évolution par la donnée est réelle et outillée (labels,
   types d'arêtes, propriétés, colonnes dynamiques, index et procédures
   déclarés en texte plat) ; l'évolution du substrat PHYSIQUE (signal persisté,
   zone, opcode WAL, dialecte) exige un chantier moteur. Un point dur n'était
   pas un chantier mais un **défaut** : la 65ᵉ étiquette **faisait abandonner le
   processus**, et une simple requête suffisait à le déclencher.

---

## 1. Chantier 1 — l'arrêt devient systémique

### Ce qui existait, et où

| pièce | avant | après |
|---|---|---|
| détection de répétition | `obrain-cli/src/repetition.rs` (binaire) | `obrain-garde/src/repetition.rs` (crate partagé, 7 cas inchangés) |
| budget par question, client parti, plafond d'appels d'outil | `engine.rs` uniquement | `obrain-garde/src/garde.rs` (`Garde`, `Veille`, `Reglages`, `Motif`) |
| arrêt d'une lecture SSE | impossible (`post_sse` rendait `()`) | `obrain-httpmin::post_sse_supervise` rend `Ok(true)` = « arrêté par nous » |

### Les boucles de modèle, après

| boucle | garde |
|---|---|
| `crates/obrain-cli/src/engine.rs` (chat du moteur) | `Garde::avec_client` + `veille` (le budget borne aussi la lecture), prose par tour (`nouvelle_page`) |
| `crates/obrain-selfd/src/chat.rs` (`converser`) | idem, et le tour rend `Tour { texte, arret }` — un arrêt se DIT |
| `crates/obrain-selfd/src/agent.rs` (`derouler`) | garde avant chaque tour, `index_appel_valide` (index d'appel aberrant), `Evenement::Arret` |
| — | **`obrain-selflink` n'est PAS une boucle de modèle** : c'est un client HTTP de LECTURE du serveur viz (son unique POST est une authentification, `engine.rs:216`), et ses seules boucles parcourent des tableaux JSON déjà reçus. Rien à y garder. L'audit l'avait supposé ; la vérification l'a démenti (§2.5 bis). |

### Preuves (le critère que l'audit reprochait au dépôt : « la garde tombe-t-elle si on la retire ? »)

| test | mutation jouée | résultat |
|---|---|---|
| `obrain-garde` : 18 cas (répétition ×7, budget, client parti, premier motif, clamp, veille, page) | — | verts |
| `obrain-httpmin` : 5 cas (flux nominal, arrêt par le rappel, arrêt par la commande, 500, délai) | `sur_ligne` ignoré | **FAILED** ✅ |
| `selfd` : `un_modele_qui_se_repete_est_arrete_et_le_dit` | détection retirée | **FAILED** (`fragments lus : 20`) ✅ |
| `selfd` : `un_index_d_appel_aberrant_est_ignore_au_lieu_d_allouer` | clamp retiré | **FAILED** ✅ |

> Le test de répétition a d'abord été **trop faible** : il passait encore sans la
> détection (le motif était posé, mais le flux était lu jusqu'au bout). Il
> compte maintenant les fragments lus — « lire jusqu'au bout puis dire qu'on a
> bouclé » n'est pas la garantie, c'est le défaut d'origine.

### Ce qui reste ouvert (chantier 1)

- `obrain-selflink` reçoit `arret` mais **ne le montre pas** (surface minimale).
- La détection reste **littérale** : ping-pong de lignes courtes (motif < 40
  caractères, moins de 6 lignes identiques) et boucles sémantiques non couverts —
  c'est un choix prudent assumé (`MIN_TEXTE`, seuils nommés), à ne pas relâcher
  sans mesure de faux positifs.

---

## 2. Chantier 2 — la porte de tests : hermétique, puis honnête

### 2.1 Le défaut structurant : la porte dépendait de la machine

Mesuré le 2026-09-16 : `cargo test --workspace` donnait **45 échoués / 18 cibles
rouges** sur un poste où `OBRAIN_IAM*` et `OBRAIN_STORE` étaient armés, et
**26 échoués / 14 cibles** dans un environnement propre. **19 échecs étaient
donc le fait de l'environnement du développeur**, pas du code.

Corrigé, par cause :

| cause | correctif | vérification |
|---|---|---|
| IAM armé dans l'environnement → `401` sur 9 tests du pont | `GardeIam::desarmer_pour_les_tests()` (garde INERTE posée une fois, `OnceLock`) + appel dans `make_db()` | les 9 passent **avec** `OBRAIN_IAM=1` |
| `viz serve` héritait l'IAM de la machine → 6 tests « le serveur n'écoute pas » | isolement de l'ENFANT (`env_remove` sur IAM) dans `tests/viz_space_pagination.rs` | 6/6 verts avec l'env pollué |
| un test écrivait `OBRAIN_METRICS_FILE` dans l'environnement du PROCESSUS (mutation concurrente interdite) | forçage de test (`OVERRIDE_METRIQUES`, `Mutex`) au lieu de `set_var` | plus de mutation globale ; 221 tests verts |
| deux portes de performance échouaient selon la machine (569 ms pour un seuil de 250 ms) | armement EXPLICITE `OBRAIN_PERF_GATE=1` ; non-armé, les chiffres restent mesurés, écrits au rapport, et le non-armement **se dit** | non armé = vert ; `OBRAIN_PERF_GATE=1` = **FAILED** (la porte existe toujours) |

### 2.2 Défauts réels corrigés (et ce qui les prouve)

| # | défaut | fichier | preuve |
|---|---|---|---|
| 1 | **Cycle de références qui fuit le store et son verrou** : `store → CdcObserver → CdcLog → CdcPersist → store`. Après la chute d'une base, **tous ses fichiers restaient ouverts** et son `flock` exclusif tenu à vie ; le registre partageait donc cette part exclusive, un « shared-write » redevenait exclusif et l'invariant I4 (2ᵉ écrivain refusé) tombait | `obrain-engine/src/database/cdc_observer.rs` (`Weak<CdcLog>`) | `multi_writer_invariants` : 9/9 avec `--features cdc,algos` ; `/proc/self/fd` vide après la chute |
| 2 | **Série de propriété muette** : `find_nodes_with_property` exigeait une couverture DISQUE et rendait `None` alors que l'index de SESSION portait les porteurs (visible seulement quand la zone n'était pas encore fiable) | `obrain-substrate/src/store.rs` (union des 3 sources) | `existence_par_serie_de_propriete_…` verts (888 tests substrate) |
| 3 | **`create_property_index` ne marquait pas la clé couverte** après avoir bâti et persisté la série | `store.rs` (`confronter_index_egalite()` après construction) | idem |
| 4 | Deux doctests cassaient la porte `--doc` (prose indentée compilée comme du Rust) | `obrain-common/src/utils/cancel.rs`, `obrain-substrate/src/store.rs` | `cargo test -p obrain-common --doc`, `-p obrain-substrate --doc` verts |
| 5 | Le cycle de chaîne n'explosait plus (garde de visite), mais le test affirmait encore l'explosion | `store.rs` (assertion = 1, motivée) | vert |
| 6 | Trois tests de comparaison inter-types et trois tests `CREATE/DROP INDEX` affirmaient des contrats **retirés par doctrine** (hack RDF ; cutover T17) | `operators_filter.rs`, `spec_compliance.rs` | verts, avec témoins de même classe pour garder des dents |
| 7 | **La 65ᵉ étiquette tuait le processus** (mutation refaite le 17/09 : **défaut 12** et §2.5) | `traits.rs`, `mutation.rs`, `store.rs` | `plafond_etiquettes.rs` — **vérifié par mutation** |

### 2.3 Ce qui reste rouge (mesuré, pas caché)

*(Ce tableau a porté l'état « rouge » ; tout ce qu'il listait est corrigé. Il est
conservé comme INVENTAIRE daté, parce qu'un inventaire qui disparaît ne laisse
aucune trace de ce qui était cassé — et parce que la ligne `spec_compliance` a
d'abord été attribuée à une mauvaise cause : voir la note sous le tableau.)*

| cible | test | cause mesurée | état |
|---|---|---|---|
| `wal_recovery` ×4 | DDL de types | `CREATE NODE TYPE` écrivait un `WalRecord` que **personne ne rejoue** → le type disparaissait au redémarrage ; et `wal_status()` annonçait `enabled: false` sur un store qui a bien un WAL | **corrigé** |
| `property_index_po_bench` ×3 | index de propriété | les tests passaient par le GQL `CREATE INDEX`, **refusé** depuis le cutover T17 | **corrigés** (API programmatique) |
| `coverage_rebrand` / `coverage_session` | agrégat dans `WITH` ; `nodes_written` | le `WITH` de GQL traduisait chaque item en **projection scalaire** (l'agrégat arrivait intact à l'évaluateur → « fonction inconnue : sum ») ; un test affirmait `0` quand la doc du module affirmait `1` | **corrigés** |
| `search_relief` | contradiction invisible | la déduplication CDC prenait `(entité, epoch, timestamp)` pour clé : cinq écritures dans la **même milliseconde** étaient effondrées en une → plus aucune contradiction détectée | **corrigé** (clé = événement entier) |
| `spec_compliance` | `parse_foreach_after_with` | même cause que `coverage_rebrand` : l'agrégat dans un `WITH` de GQL n'était pas planifié (il arrivait intact à l'évaluateur). **La cause du `collect` était AUTRE, et trouvée plus tard** (4ᵉ passe, §2.7) : le typage des projections, pas la planification de l'agrégat | **corrigé** |
| `obrain-cognitive --lib` ×2 | caches `synapse` | les tests affirmaient « le cache chaud démarre vide » — vrai jusqu'au 2026-08-31, date où le boot l'a réhydraté | **corrigés** (épreuve renforcée : cache vidé pour prouver la vue native) |

> Cette liste remplace celle de `GARANTIES-NON-REGRESSION.md` §4, qui datait du
> 2026-09-05. **Rien n'y reste rouge** : la porte complète est à **8 838 verts,
> 0 échec** — chiffre obtenu sur la passe finale, après le correctif du défaut 23.
>
> Un échec est apparu sur une passe **intermédiaire**, et un seul :
> `simplex_durability::drop_sans_flush_rejeu_reconstruit`, sur un **refus de verrou
> exclusif** (`substrate … is already open by another process`). C'est un flake de
> charge, pas un défaut : la cible passe 6/6 seule, 8/8 en quatre instances
> simultanées, et vert à la passe finale. Il était déjà nommé comme tel au
> chantier 2 — un flake nommé n'est pas un flake caché.

### 2.4 L'état mesuré, les trois temps du chantier

| mesure | avant (2026-09-16) | 1re passe | 2de passe | **finale** |
|---|---|---|---|---|
| environnement PROPRE | 8 769 passés · **26 échoués** · 14 cibles | 8 799 · 13 · 7 cibles | 8 811 · 1 · 1 cible | **8 838 · 0 échec** |
| environnement du POSTE (IAM armé) | 8 750 · **45 échoués** · 18 cibles | 8 796 · 16 · 10 cibles | hermétique | hermétique |

Le dernier échec de la passe précédente
(`multi_writer_wal::append_multithread_contre_reader_concurrent`, WAL sous
threads) ne se reproduit plus : il ne tombait que sous la charge d'une suite
complète, et l'élimination des causes de contention restantes l'a ramené dans le
vert. Deux autres tests sensibles à la charge ont demandé, eux, une vraie
correction — et c'étaient **mes** coutures de test, pas le produit :

* `mcp_server::tests::metrics_logger_jsonl_quand_env_absent_rien` : le forçage du
  chemin de métriques était **global** (`Mutex<Option<…>>`), donc TOUS les tests
  du binaire écrivaient dans le fichier que ce test observait ; sous charge, les
  appends de plusieurs threads s'entrelaçaient et la relecture donnait un
  enregistrement illisible. Le forçage est désormais **par thread** : il ne
  concerne que les appels du test qui l'a posé.
* `viz_extract::tests::expansion_entrante_ne_balaie_pas_la_zone` : vert seul et en
  cible isolée, rouge sous la charge d'une suite complète (mesure de balayage).

### 2.5 Les défauts corrigés dans la seconde passe (2026-09-17, soir)

#### 2.5 bis CORRECTION D'UNE AFFIRMATION DE CE DOCUMENT (même jour)

Le chantier 1 était décrit comme devant traverser « les QUATRE boucles de
modèle », `obrain-selflink` comprise (c'est ce que supposait l'audit). Vérifié :

```
$ grep -rn "post_sse|chat/completions|Garde" crates/obrain-selflink/src/*.rs
crates/obrain-selflink/src/engine.rs:216: http::post_json(&self.url("/api/auth/login"), …)
```

`obrain-selflink` est un **client HTTP de LECTURE** du serveur viz : il n'appelle
aucun modèle (`son unique POST` est une authentification), et ses seules boucles
parcourent des tableaux JSON déjà reçus. **Il n'y a donc rien à garder là** — la
garde couvre TROIS sites de boucle de modèle (`obrain-cli/engine.rs`,
`obrain-selfd/chat.rs`, `obrain-selfd/agent.rs`) plus le drapeau client de
`viz_serve`. Écrire « quatre » était faux, et un document de garanties qui
exagère son périmètre ne garantit plus rien.

| # | défaut | ce qui le prouvait | où |
|---|---|---|---|
| 8 | **Le DDL de types n'était pas durable** : `CREATE NODE TYPE Vehicle` disparaissait à la fermeture — le `WalRecord` était écrit, jamais rejoué | `wal_recovery::test_create_node_type_persists` (« Node type Vehicle should survive WAL replay, got: [] ») | `substrate.typecat` (catalogue déclaratif, patron de `substrate.proccat`) + `database/schema_catalog.rs` (hydratation au boot) |
| 9 | **`wal_status()` mentait en mode substrate** : `enabled: false` sur une base qui écrit et rejoue `substrate.wal` | `wal_recovery::test_wal_status_persistent` | `database/admin.rs` (lit le WAL du substrat, et avoue `last_checkpoint: None` au lieu d'inventer 0) |
| 10 | **Un agrégat dans `WITH` n'était pas agrégé** (GQL) : `WITH sum(n.v) AS total` rendait « fonction inconnue : sum » | `coverage_rebrand::query_with_aliased_aggregation`, `spec_compliance::parse_foreach_after_with` | `translators/gql/mod.rs` (branche agrégat, miroir du `RETURN` et de `cypher.rs::translate_with`) |
| 11 | **La déduplication CDC effaçait des événements** : clé `(entité, epoch, timestamp)` → 5 écritures dans la même milliseconde = 1 seule retenue | `search_relief::relief_domine_le_nouveau_hub…` (contradiction invisible) | `cdc.rs` (`DedupCdc` : l'événement ENTIER fait la clé) |
| 12 | **La 65ᵉ étiquette tuait le processus** (une requête suffisait) | `plafond_etiquettes.rs`, **vérifié par mutation** | `graph/traits.rs` + `operators/mutation.rs` + `substrate/store.rs` |
| 13 | **Cycle de références qui fuitait le store et son verrou** | `multi_writer_invariants` (I4), `/proc/self/fd` | `database/cdc_observer.rs` |

### 2.6 Les défauts corrigés dans la TROISIÈME passe (2026-09-17, nuit)

Chantier 4 (budget en jetons) et chantier 5 (registre de plugins) :

| # | défaut | ce qui le prouvait | où |
|---|---|---|---|
| 14 | **L'injection n'avait aucun budget d'ensemble** : plafonds par section (goals 5, summaries N, weave 8, engrams 10), aucun chiffre total ; et les résumés de compaction traversaient la frontière **sans troncature** | audit + `le_budget_de_jetons_borne_l_injection_et_le_dit` | `obrain-rag` ENFIN branché : `RagConfig::token_budget` + `budget::estimate_tokens` décident, le pont s'y range (`mcp_server.rs`), et la coupe SE DIT (`troncature`) |
| 15 | **`obrain-rag` n'était branché nulle part** (budget, MMR, ranking, feedback : tout le crate) | audit §« existe dans le moteur » vs « utilisé » | `obrain-cli` en dépend ; le budget en jetons est le premier usage réel |
| 16 | **Le `PluginRegistry` était orphelin** : `register_udf` existait, documenté « résolu au moment du planning », et aucune requête ne pouvait appeler une UDF | `RETURN mon_algo(n)` → « fonction inconnue » | point d'extension du noyau (`execution::fonction_hote`) + pont installé par les deux constructeurs de base + `registre_global()` |
| 17 | **L'index BM25 était reconstruit à chaque ouverture** (~1,6 s par lecteur : le BM25 était de fait interdit en chemin chaud) | audit + mesure dans le test | `substrate.textidx.<label>.<propriété>` (écriture atomique, **empreinte** à deux signaux), hydratation au boot |

#### Les deux signaux de l'empreinte — mesurés, pas devinés

| moment | offset du WAL | époque | nœuds |
|---|---|---|---|
| après construction de l'index | 551 | 5 | 5 |
| après `close()` | **583** | **5** | 5 |
| après réouverture | 583 | 0 | 5 |
| après un `CREATE` | 675 | 1 | 6 |
| après un `SET` de propriété | 675 | 2 | 6 |

Aucun des deux ne suffit seul : **l'offset** bouge à la fermeture (551 → 583) donc
il ne peut pas dire « l'index est à jour » ; **l'époque** est remise à zéro à
chaque réouverture donc elle ne peut pas valider un fichier d'une session
précédente. Chacun répond donc à la question qu'il sait trancher : l'époque
décide **réécrire ou effacer** à la fermeture, l'offset décide **charger ou
reconstruire** au réveil. Et les compteurs de nœuds/arêtes, eux, ne bougent PAS
sur un `SET` — un index validé par les seuls compteurs chargerait un texte
périmé.

### 2.7 La quatrième passe (2026-09-17, lendemain matin)

Une amélioration demandée (« continue à améliorer ») a commencé par vérifier une
affirmation de ce document — et l'a trouvée fausse (selflink, voir §2.5 bis) —
puis a suivi une observation : un `obrain bridge` qui tournait à vide.

| # | défaut | ce qui le prouvait | où |
|---|---|---|---|
| 18 | **Un agrégat de LISTE ne traversait pas un `WITH`** : `collect(v)` rendait `Int64(0)` et `size(vals)` rendait `null`, alors que la même requête en Cypher rendait `[10, 20]` | `coverage_rebrand::une_liste_traverse_un_with_et_reste_utilisable` (ex-test ignoré, réactivé) | `planner/lpg/project.rs` : toute variable projetée était typée `LogicalType::Node`, donc la liste était recopiée dans un **vecteur de nœuds**, où elle disparaît. Le type vient désormais du planificateur (scalaire / arête / nœud) |
| 19 | **Un pont qui ne rend pas la main était invisible** : mesuré 83 % d'un cœur pendant plus de dix minutes, 234 Mo écrits sur le socket, aucune ligne de journal | mesure `/proc/<pid>/io` + `/proc/<pid>/task/*/stat` (voir ci-dessous) | veilleur de requête (`alerte_requete`, seuil `OBRAIN_REQUETE_ALERTE_MS`, défaut 10 s) : il NOMME l'outil et son âge |
| 20 | **Une réponse sans plafond était streamée** : le même pont a écrit 234 Mo en dix minutes (134 écritures/s), pendant que le harness consommait le flux — ni erreur, ni fin | mesure : `syscw` 158 419 → 230 799, `wchar` 161 Mo → 234 Mo | `plafond_reponse_octets` (`OBRAIN_REPONSE_MAX_MO`, défaut 32 Mo) : au-delà, un **refus explicite** qui nomme l'outil, la taille et le remède (`LIMIT`) |
| 21 | **Une sortie cassée n'arrêtait pas la boucle** : `let _ = writeln!(stdout, …)` avalait toutes les erreurs d'écriture — un correspondant parti laissait le pont empiler des réponses en tenant le verrou du store | `mcp_server::tests::une_sortie_cassee_remonte_au_lieu_d_etre_avalee` | la boucle s'arrête sur `Err` et le DIT (`ecrire_reponse`) |
| 22 | **Une empreinte d'index BM25 ne disait pas qui l'avait produit** : seul un `MAGIC` que l'auteur doit penser à incrémenter | `index_texte_persiste::un_index_produit_par_un_autre_tokenizer_est_refuse` (mutation : refus retiré → chargement d'un index étranger) | l'identité du tokenizer (type **et** réglages) et celle de la config BM25 sont écrites dans le fichier et vérifiées au chargement |
| 22 bis | **Un test lisait un régime POSÉ PAR UN AUTRE TEST** : `sonde_v3_replay::un_noeud_v3_sans_flush_survit_au_rejeu` tombait sur un refus légitime (`OBRAIN_PROPS_V2=0 REFUSE …`) parce qu'un test voisin du même processus avait posé ce régime sous `props_v2_env_lock` — verrou que 131 sites prennent et que ce test-ci ignorait | la suite `obrain-substrate --lib` en parallèle (888 tests) | le test prend le même verrou : on ne peut pas dépendre d'un régime sans entrer dans la section critique qui le pose |
| 23 | **LE CORRECTIF DU DÉFAUT 11 ÉTAIT QUADRATIQUE** : la déduplication CDC comparait l'événement à tous ses prédécesseurs du même groupe de clé — et un groupe, c'est un import (des milliers d'écritures sur le MÊME nœud de session dans la MÊME milliseconde). Mesuré sur le chemin froid : **×3,9 par doublement** (16,4 ms → 59,0 → 228,1 ms pour 1k/2k/4k). Sur la zone du store réel, cela fait des MINUTES à 100 % d'un cœur — l'emballement de pont observé trois fois (85 % de CPU, 2 Go de RSS, aucune E/S) | `cdc_cout_dedup.rs` : mesure des deux chemins, garde de complexité à 16 000 événements, et compte rendu exact | `cdc.rs` — `DedupCdc` indexe les événements par un **hachage stable** (clés de propriétés triées) et ne compare par `==` que les collisions. Linéaire après correction (×1,8/×2,0), 13× plus rapide à 4 000 |

#### Deux gardes de temps ramenées derrière la porte opt-in

La même passe a rencontré deux assertions de **temps mural** qui tombaient sous
la charge (machine partagée, charge 7 à 17 pour 16 cœurs) alors que le code
n'avait pas changé :

* `multi_writer_invariants::import_bulk_400k_1600k_non_regresse` : 2,2 s mesuré
  contre un repère historique de 0,26 s (garde à 0,80 s) — mesuré **vert** une
  fois la machine rendue au calme, et **vert** également porte armée ;
* `index_concurrency::deux_index_ne_se_genent_pas` : sa garde secondaire
  (`parallele < sequentiel`) avait une marge de **1,5 %** (241,79 ms contre
  245,32 ms), donc plus petite que le bruit de la machine. Sa garde principale —
  `parallele < temoin * 2`, marge 100 %, celle qui attrape le défaut visé — reste
  active en toutes circonstances.

Les deux suivent désormais la même règle que les portes de performance de
`obrain-cognitive` : **la mesure est toujours faite et toujours imprimée**,
l'assertion de bruit ne tombe que si `OBRAIN_PERF_GATE=1`. C'est la doctrine
écrite au chantier 2, appliquée là où elle manquait encore.

#### Comment l'emballement du pont a été mesuré (sans débogueur)

`ptrace` est refusé sur cette machine (`yama/ptrace_scope = 1`, processus frère),
donc pas de pile d'appels. Ce qui a suffi, sans rien interrompre :

```bash
ps -o pid,etime,pcpu,rss -p <pid>            # 71-83 % d'un cœur, 2,2 Go résidents
awk '{print $14+$15}' /proc/<pid>/stat       # ← 10 s plus tard : 800 ticks = 8 s de CPU
for t in /proc/<pid>/task/*; do awk '{print $14+$15}' $t/stat; cat $t/comm; done
                                             # tout le CPU dans le fil PRINCIPAL, 17 fils au repos
grep -E 'wchar|syscw' /proc/<pid>/io         # 161 → 234 Mo écrits, 158 419 → 230 799 écritures
ls -l /proc/<pid>/fd                         # fd 0/1/2 = sockets, + les zones du store
```

Ce que ces mesures disent, et ce qu'elles ne disent pas : le pont **écrivait**
(134 écritures/s) vers le harness, donc la boucle n'était pas un simple
attente-active ; mais la requête qui a produit ces 234 Mo n'est pas identifiée —
`OBRAIN_DEBUG_LOG` n'était pas armé sur ce processus, et il n'est pas possible de
l'armer après coup. **C'est précisément ce que le veilleur et le plafond
corrigent** : à la prochaine occurrence, le journal nomme l'outil et la taille.

#### DÉFAUT 24 : UN RÉGLAGE MORT, ET UN TEST IGNORÉ POUR LA MAUVAISE RAISON

La vérification d'un test **ignoré** (`contrat_j5b_planificateur::contains_garde_
fou_de_cout`, ignoré le 2026-09-15) a mis au jour un défaut d'une autre nature :
un bouton d'exploitation qui ne règle plus rien.

| ce qui était affirmé | ce qui est mesuré |
|---|---|
| « la preuve 0-slot exigeait la zone trigrammes armée dès le create » | **faux** : la précondition se CONSTRUIT (base sur fichier + index déclaré + fermeture + réouverture, le remède déjà appliqué à son jumeau `existence_indexe`) — les deux preuves 0-slot passent, mesuré |
| la variable `OBRAIN_CONTAINS_GARDE` règle la garde de coût de la route CONTAINS | **faux depuis un décrochage silencieux** : `seuil_garde_contains`, la fonction qui la lit, n'est **appelée par personne** (le compilateur le disait : `function seuil_garde_contains is never used`). Le commit qui l'avait branchée (8105b739, 2026-09-14) est le seul que l'historique attribue — l'appel a disparu sans être attribuable |
| la garde existait donc encore | **la garde vivante est ailleurs** : `trop_de_candidats` (`planner/lpg/filter.rs`, pilotée par `OBRAIN_TEXTE_CANDIDATS_MAX`, un pourcentage du store au lieu d'un seuil absolu), prouvée par `contient_intention::la_garde_de_cout_rend_la_main_au_balayage` |

**Décision** : le réglage mort est **retiré** (une fonction morte qui lit une
variable d'environnement est pire qu'une fonction absente : elle donne l'illusion
d'un levier), l'emplacement dit où vit la garde vivante, et le test est
**réécrit et RÉACTIVÉ** — il ne garde que ce qui est vrai et vérifiable (la route
indexée sert sans balayer, 0 slot, et rend exactement les mêmes lignes). La cible
passe de « 1 test vert + 1 ignoré » à **« 2 verts, 0 ignoré »**.

**Deux autres fonctions mortes** sont signalées par le compilateur dans le même
fichier — `union_candidats` et `borne_slots_aretes` — mais **ce ne sont pas des
boutons** (aucune variable d'environnement, aucune surface de réglage) : du code
laissé en place, sans effet trompeur. Nommées ici plutôt que supprimées.

#### 2.7 bis RETRACTATION : LE DÉFAUT 23 N'EXPLIQUE PAS L'EMBALLEMENT DU PONT

Ce document a affirmé — avec des mesures — que la déduplication CDC quadratique
était « très exactement l'emballement de pont observé ». **C'est faux, et la
vérification le démontre** :

| mesure sur une COPIE du store réel (sonde `cout_consolidation`) | valeur |
|---|---|
| fenêtre CDC chargée | 20 000 événements |
| groupes de clé (entité, époque, milliseconde) | 2 263 |
| **plus gros groupe** | **15** |
| lecture de la fenêtre | 295 ms |
| les trois lectures d'une consolidation (`search_surprise` + `search_conflicts` + `search_relief`) | **30 ms** |

Avec un groupe maximal de 15, le coût quadratique vaut 15²/2 ≈ 112 comparaisons —
**quelques dixièmes de milliseconde**. Le défaut 23 est réel (mesuré : ×3,9 par
doublement, et catastrophique sur un store dont les événements se concentrent sur
une seule entité — un import massif, par exemple), mais il **ne peut pas**
expliquer 85 % d'un cœur pendant vingt minutes sur ce store-ci : ses événements
sont répartis (≈ 9 par groupe).

**Ce que la même sonde établit à la place**, et qui est le vrai chemin :

* le harness écrit **toutes les 500 ms** (`GraphWriter.flushIntervalMs`) ;
* chaque `import_events` déclenche `auto_consolider` → `tisser` → **trois lectures
  complètes de la fenêtre CDC** (20 000 événements, 295 ms pour la seule lecture) ;
* soit ≥ 325 ms de travail par import mesuré **avec** le correctif — et davantage
  avec les payloads d'une rafale réelle ;
* face à une cadence de 500 ms **sans contre-pression** : si un import dépasse la
  cadence, la file grossit et le pont ne rattrape jamais son retard. C'est le
  profil exact observé (85 % de CPU pendant des minutes, 2,0-2,6 Go de RSS — le
  mmap du store, 468 Mo de journal CDC qui continue de croître).

**Conclusion honnête** : le défaut 23 était une piste fausse (mais un vrai
défaut, corrigé).

#### 2.7 septies LE LECTEUR WAL CRIAIT « CORRUPTION » SUR UNE ÉCRITURE EN VOL

Défaut 25, trouvé en mesurant le taux d'échec d'un test que j'avais classé — à tort
— « flake de charge » :

```text
multi_writer_wal::append_multithread_contre_reader_concurrent
panicked at multi_writer_wal.rs:110: aucun record corrompu visible:
WalShortRead { needed: 44, got: 32 }
```

**2 échecs sur 12 exécutions**, EN ISOLATION (et vert avec `--test-threads=1`) —
donc ni un flake de charge, ni un conflit de fichiers (les deux tests ont des
dossiers temporaires distincts) : un défaut réel, intermittent par nature.

Le commentaire du test dit mot pour mot ce que l'itérateur doit faire :

> une écriture en vol peut laisser une queue partielle, que l'itérateur arrête
> proprement (comportement crash-boundary), mais jamais de record corrompu au milieu

Or `WalIter::next` rendait `Some(Err(WalShortRead))` : il **criait corruption** sur
une queue incomplète. `decode` reçoit `&bytes[cursor..]` — « il manque des octets »
veut donc dire, **par construction**, que le fichier s'arrête avant la fin de
l'enregistrement : une écriture en vol, ou un arrêt brutal. C'est la frontière de
crash. **Le test avait raison ; le code ne tenait pas son contrat.** Corrigé : ce cas
rend `None` (fin propre), une vraie corruption (CRC invalide, octets réservés non
nuls, type inconnu) reste une erreur. **15/15 vertes** ensuite, là où 2 sur 12
échouaient.

**Et un test unitaire a dit NON, à raison.** `wal_io::reader_stops_at_crc_corruption`
est tombé sur ma correction : il corrompait **un octet au milieu du fichier**, et si
cet octet tombe dans le champ LONGUEUR, l'enregistrement réclame plus d'octets qu'il
n'en existe — **indiscernable d'une queue tronquée**, par construction du format (le
CRC ne vérifie que des octets présents). Le test mesurait donc autre chose que son
nom. Il corrompt maintenant le **CRC** d'un enregistrement du milieu (tous les octets
sont là, c'est bien la somme de contrôle qui doit faire échouer), et un **second test**
fixe explicitement le cas que le format ne sait pas trancher, pour que la décision
soit écrite plutôt que tacite.

#### 2.7 sexies UNE RÉGRESSION QUE J'AI INTRODUITE, ET QUE LA MESURE A DÉMASQUÉE

La persistance de l'index texte (défaut 14 du §2.6) **reconstruisait à chaque
ouverture** tout index déclaré dont le fichier manquait ou ne correspondait plus.
Mesuré sur le store réel (484 000 nœuds, **12 index texte déclarés**), par les
phases déjà instrumentées de l'ouverture :

```text
index texte hydrates (fichiers substrate.textidx.*) charges=0 reconstruits=12
obrain-mcp: store ouvert (/tmp/…/runtime-dev.obrain, RW, 5930 ms)
```

**5,9 s des 7,8 s de la vie d'un pont** — douze balayages complets de label, **à
chaque ouverture**, alors que le harness respawn un pont par requête. Une
« optimisation » qui déplace un coût PARESSEUX (l'index se construisait à la
première recherche qui en avait besoin) vers un coût SYSTÉMATIQUE n'optimise rien.

Corrigé : l'hydratation n'installe plus que ce qu'elle a pu **lire** du disque, et
**diffère** le reste à la première recherche (le chemin paresseux, inchangé).

| mesure (release, store réel, cycle de vie complet d'un pont RW) | avant | après |
|---|---|---|
| ouverture annoncée par le pont | **5 930 ms** | **477 ms** |
| cycle complet (mural · CPU) | 7,77 s · 3,29 s | **3,72 s · 1,61 s** |

**Et cette régression n'explique PAS les emballements observés** : le binaire que
le harness lance (13:04) ne contient aucune trace de ce code — `strings` y trouve
**0 occurrence** de `substrate.textidx.`. Les ponts emballés tournent une version
antérieure. Ce que cette passe corrige, c'est ce qui les aurait remplacés : sans ce
correctif, **déployer** mon travail aurait rendu chaque requête du harness 12 fois
plus lente à l'ouverture.

#### 2.7 quinquies LA GARANTIE QUI MANQUAIT : AUCUN OUTIL NE DÉPASSE SON BUDGET

Au-delà de la cause, le défaut de FOND est celui que le chantier 1 avait traité
pour les boucles de modèle, et qui restait entier pour le moteur : **rien ne
pouvait arrêter un appel d'outil**. L'échéance du pont
(`OBRAIN_QUERY_TIMEOUT_MS`, 30 s) n'était armée que pour le texte GQL ; les appels
d'OUTILS — `import_events`, `auto_consolider` → `search_surprise`,
`search_conflicts`, `search_relief`, chacune relisant toute la fenêtre CDC —
tournaient **hors de toute portée d'annulation**. `cancel::check()` n'y voyait
jamais d'échéance : un travail parti ne pouvait plus être arrêté, et c'est
exactement ce qu'on observe (un cœur brûlé pendant des minutes).

Corrigé, et vérifié :

| maillon | où | preuve |
|---|---|---|
| l'échéance est armée pour **tout appel d'outil** (pas seulement GQL) | `mcp_server::run` — portée `cancel::frontiere` autour de la dispatch entière | `le_tissage_refuse_quand_le_budget_est_depasse` |
| les trois lectures de la consolidation **consultent** l'échéance avant leur travail | `search_surprise`, `search_conflicts`, `search_relief` | `echeance_outils.rs` — 4 tests, dont « les trois » une par une |
| le refus est **explicite** et nomme l'étape | `verifier_echeance` | mutation : contrôle retiré de `search_surprise` → 2 tests tombent |

Ce qui reste hors d'atteinte, et il faut le dire : `changes_since_timestamp`
rend un `Vec` (pas un `Result`), donc une **lecture** de fenêtre déjà engagée ne
peut pas être interrompue au milieu — seule la prochaine frontière la coupe. Le
budget borne donc l'enchaînement, pas chaque lecture élémentaire.

#### 2.7 quater SECONDE MESURE : UN BINAIRE DEBUG N'EST PAS UN BINAIRE DE PRODUCTION

Le §2.7 ter ci-dessous a été écrit avec des mesures prises sur **`target/debug/obrain`**
(537 Mo, non optimisé) : 1,70 s de CPU et 2,44 Go de RSS pour une ouverture. Refaites
avec le binaire **release** — celui qui tourne — les chiffres changent du tout au
tout :

| ouverture d'un pont (release, store réel de 14 Go, copie) | temps d'ouverture annoncé | cycle complet | RSS |
|---|---|---|---|
| lecture seule | 486 ms | 0,53 s mural · **0,24 s CPU** | 279 Mo |
| écriture | 342 ms | 4,79 s mural · **1,47 s CPU** | **2 441 Mo** |

**Leçon, et elle vaut d'être écrite** : mesurer sur un binaire debug puis conclure
sur la production est une faute de méthode — le coût du binaire non optimisé
(chargement, relocations, absence d'optimisation) était attribué au store. Les
chiffres de production sont ceux du **release** : une ouverture RW coûte
**1,47 s de CPU et 2,4 Go** (l'hydratation des zones et le chargement des
propriétés dominent, pas le « recount » des compteurs qui vaut 17 ms).

**Ce qui tient, en revanche, et qui explique le profil observé** : les ponts
emballés mesuraient **2,0-2,6 Go de RSS** — exactement le profil d'un pont en
ÉCRITURE (2 441 Mo mesurés), pas d'un pont en lecture seule (279 Mo). Et le
harness **respawn un pont par requête** (son propre code le dit), à une cadence
d'écriture de **500 ms**. Une ouverture RW de 1,47 s de CPU toutes les 500 ms
sature un cœur — c'est le mécanisme, et il ne demande pas de mystère
supplémentaire.

#### 2.7 ter CE QUI EXPLIQUE VRAIMENT L'EMBALLEMENT — MESURÉ

Deux mesures sur une COPIE du store réel ferment l'enquête :

| ouverture d'un pont sur le store réel | temps mural | CPU | RSS |
|---|---|---|---|
| **lecture seule** (`OBRAIN_WRITE` absent) | 0,44 s | **0,33 s** | **283 Mo** |
| **écriture** (`OBRAIN_WRITE=1`) | 2,30 s | **1,70 s** | **2 444 Mo** |

L'ouverture en ÉCRITURE coûte **5× le CPU et 8,6× la mémoire** de l'ouverture en
lecture : elle rejoue le WAL, audite les ancres, **recompte les compteurs** et
mappe tout le store (2,4 Go).

Face à cela, le modèle du harness — écrit noir sur blanc dans son propre code
(`mcp_server.rs` : « respawn `obrain bridge` après CHAQUE requête (dispose) ») —
et sa cadence d'écriture de **500 ms** (`GraphWriter.flushIntervalMs`) :

> chaque requête paie une ouverture RW de 1,70 s de CPU et 2,4 Go, alors qu'une
> nouvelle arrive toutes les 500 ms ⇒ **un cœur saturé en permanence** dès que la
> session est active.

Ce que cela explique, et que la piste CDC n'expliquait pas :

| observation (mesurée sur les ponts emballés) | explication |
|---|---|
| 85-90 % d'un cœur pendant 3 à 24 minutes | enchaînement d'ouvertures RW de 1,70 s à cadence de 500 ms |
| RSS 2,0-2,6 Go | le mapping du store en mode écriture (2 444 Mo mesurés) |
| écritures continues (centaines de Mo) | journal CDC appendé + corrections de compteurs + ancres de rejeu |
| réapparition systématique (4 occurrences) | la session est active : le harness écrit sans arrêt |

**Et pourquoi le recomptage est nécessaire** : le premier open RW de la copie a
signalé `compteurs d'instantane FAUX, corriges depuis les zones noeuds_avant=80377
noeuds_apres=87918` — **7 541 nœuds d'écart**. Après correction, les ouvertures
suivantes sont propres : la dérive vient donc d'un processus **tué en pleine
écriture** (le harness `dispose` son pont ; j'en ai tué un moi-même), qui laisse
les compteurs en arrière — et l'ouverture suivante paie le recomptage complet.
Le recomptage est le filet ; **la dérive est le défaut**, et sa cause (un chemin
d'écriture qui ne met pas les compteurs à jour, ou l'arrêt brutal) reste à traiter.

**Deux signaux d'intégrité observés au passage** sur le store réel, et nommés
plutôt que tus :

* `post-replay anchor audit: nodes with labels but no reachable properties
  (husk signature — properties written after the last flush were lost)
  label_only=31` — 31 nœuds « coquilles » (étiquetés, sans propriétés), la
  signature exacte d'écritures perdues par un arrêt brutal ;
* `props zone v2 decode failed … StringRef {page_id=20801, offset=291} out of
  range` — une référence de chaîne invalide dans la zone de propriétés v2, sur
  laquelle le moteur retombe proprement (DashMap), mais qui dit qu'une page est
  abîmée.

Ces deux-là sont des signaux de DONNÉES du store de production, pas des défauts du
code de cette passe : ils sont consignés ici pour que quelqu'un les regarde.

#### LA PISTE QUE CE DOCUMENT AVAIT RETENUE, ET QUE LA MESURE A ÉCARTÉE

Défaut **23**. Après la troisième occurrence (85 % de CPU, 2,0 Go, échantillonnée
trois fois), la piste « boucle de calcul sans E/S » a été suivie jusqu'au bout par
la mesure, et elle mène à la déduplication CDC corrigée la veille.

**L'hypothèse.** La déduplication de `changes_since_*` compare désormais
l'événement ENTIER (défaut 11 : la clé grossière effaçait les écritures d'une même
milliseconde). Mais elle le faisait par un **parcours linéaire du groupe** — et un
groupe, c'est exactement ce que produit un import : des milliers d'écritures sur
la MÊME entité (le nœud de session) dans la MÊME milliseconde.

**La mesure** (`crates/obrain-engine/tests/cdc_cout_dedup.rs`, chemin froid, celui
d'un pont qui lit une zone) :

| n événements (même clé) | avant | après |
|---|---|---|
| 1 000 | 16,4 ms | 4,7 ms |
| 2 000 | 59,0 ms (**×3,6**) | 9,9 ms (×2,1) |
| 4 000 | 228,1 ms (**×3,9**) | 18,1 ms (×1,8) |
| 16 000 | ≈ 3,6 s (extrapolé) | 80,0 ms (×4,4 pour ×4) |

×3,9 par doublement : **quadratique**, puis **linéaire** après correction. Sur la
zone du store réel (des dizaines de milliers d'événements sur la même entité),
cela fait des minutes à 100 % d'un cœur — soit exactement l'emballement de pont
observé trois fois, avec ses 85 % de CPU, ses 2 Go de RSS et sa quasi-absence
d'E/S. **Ce n'était donc pas un défaut du pont : c'était le mien, et il était
invisible dans les tests** (à 4 000 événements, 228 ms ne se voit pas).

**Le correctif** (`cdc.rs`, `DedupCdc`) conserve l'exactitude et supprime le coût :
la clé grossière porte désormais un index de **hachage stable** des champs qui
décident de l'égalité, et seuls les événements qui hachent pareil sont comparés par
`==`. Le hachage trie les clés de propriétés avant de les hacher — sans ce tri,
l'ordre d'itération aléatoire d'un `HashMap` donnerait deux hachages différents
pour deux événements égaux, et la déduplication ne dédupliquerait plus.

**Le filet** : le même fichier porte une garde de **complexité** (et non de temps
mural) — 16 000 événements de même clé sous 1 s, seuil 14× au-dessus du linéaire et
3,6× sous le quadratique — plus l'assertion que le compte rendu vaut exactement `n`
(plus grand = des doublons rendus, plus petit = des événements distincts effacés,
c'est-à-dire le défaut d'origine).

**Ce que cela change pour les gardes du pont** : le veilleur, le plafond de réponse
et l'arrêt sur sortie cassée restent utiles (ils rendent visible et borné ce qui
était muet), mais ils ne sont plus la correction du problème — la correction est
dans `cdc.rs`, et elle est déployable.

#### L'emballement S'EST REPRODUIT — et le binaire déployé ne porte pas encore les gardes

Après l'arrêt du premier pont (le registre en respawn un au prochain appel, comme
le documente le code), un **second** emballement a été mesuré, même profil :

| | 1ᵉʳ pont | 2ᵈ pont (respawn) |
|---|---|---|
| âge observé | 9 min 48 s | 14 min 04 s |
| CPU | 71-83 % d'un cœur | 88,8 % |
| RSS | 2,2 Go | 2,0 Go |
| écrit sur le socket | 161 → 234 Mo | 276 Mo |

Deux occurrences au même profil, ce n'est pas un accident : c'est un défaut qui
se reproduit à chaque sollicitation de ce chemin.

**Et la garde n'est pas active dans le système vivant** : le binaire que le
harness lance (`~/.local/share/obrain-harness/bin/obrain`) date du **16/09 17:45**
et ne contient ni `OBRAIN_REPONSE_MAX_MO` ni `OBRAIN_REQUETE_ALERTE_MS` — vérifié
par `strings` sur le binaire déployé (0 occurrence, là où le binaire local les
porte). Tant qu'il n'est pas reconstruit et redéployé, **rien ne borne
l'emballement** : le correctif est dans les sources et éprouvé, pas dans la
machine.

Pour nommer la requête à la prochaine occurrence, deux leviers existent déjà :
`OBRAIN_DEBUG_LOG=<chemin>` (le pont y écrit chaque requête et chaque réponse) et
le veilleur (seuil `OBRAIN_REQUETE_ALERTE_MS`, défaut 10 s), qui publie l'outil en
cours et son âge sur la sortie d'erreur — que le harness capture et journalise.

#### Ce que la seconde passe a laissé, nommé

- `collect()` dans un `WITH` de GQL rendait `0` au lieu de la liste : il a été
  d'abord **écrit en test ignoré** (pour rester visible et exécutable plutôt que
  caché), puis **corrigé** à la 4ᵉ passe — la cause était le typage des
  projections, pas l'agrégat (§2.7, défaut 18). Le test est désormais ACTIF.
- `property_index_po_bench` couvre moins qu'avant : ces tests mesuraient aussi la
  latence du **DDL** ; l'API programmatique n'a pas d'équivalent chronométré.
- **Fait depuis** (3ᵉ passe, §2.6) : le budget en jetons (`obrain-rag` branché),
  la persistance de l'index BM25, le `PluginRegistry` branché.
- **Reste ouvert, et nommé** : la restitution de la **session courante** depuis le
  graphe (la moitié « couvrir tout le contexte » qui manque — le chemin existe,
  `session_load events_only`, il n'est branché que sur la reprise) ; le tokenizer
  français (stopwords anglais, pas de stemming) ; `obrain-rag::ranking`/`feedback`
  (branchés par personne) ; la latence du DDL d'index, que `property_index_po_bench`
  mesurait et que l'API programmatique ne mesure plus.

---

## 5. Comment vérifier tout ce document

```bash
# La porte complète, EN ENVIRONNEMENT PROPRE (c'est la règle : la porte ne doit
# pas dépendre de l'IAM ni du store du poste)
env -u OBRAIN_IAM -u OBRAIN_IAM_STORE -u OBRAIN_IAM_ACCOUNT -u OBRAIN_IAM_TOKEN \
    -u OBRAIN_STORE -u OBRAIN_SESSION_ENV -u OBRAIN_SESSION_OWNER -u OBRAIN_LLM_URL \
    cargo test --workspace --no-fail-fast

# Les pièces du chantier 1
cargo test -p obrain-garde                       # 18 cas
cargo test -p obrain-httpmin                     # 5 cas (arrêt de la lecture)
cargo test -p obrain-selfd                       # 2 cas (répétition, index aberrant)

# Le plafond d'étiquettes
cargo test -p obrain-engine --test plafond_etiquettes

# Les portes de performance, quand on veut les ARMER
OBRAIN_PERF_GATE=1 cargo test -p obrain-cognitive --features synapse,substrate \
    --test feedback_baseline --test synapse_reinforce_micro_bench

# Le sujet de rappel (côté harness)
npx vitest run packages/context/graph-injection/tests/sujet.spec.ts
```

---

## 6. En une phrase, pour reprendre demain

**La garantie d'arrêt existe maintenant là où elle manquait (les micro-harness),
la porte de tests est verte (8 838, 0 échec) et ne dépend plus de la machine qui
la lance, le rappel parle de la question posée au lieu d'une constante, la mémoire
ne se rappelle plus sa propre sortie, le DDL de types et l'index BM25 survivent au
redémarrage, l'injection a un budget en jetons qui vient d'`obrain-rag`, le
`PluginRegistry` n'est plus orphelin — et la 65ᵉ étiquette ne tue plus le
processus.**

Ce qui reste est nommé, et **vérifié comme non fait** : la restitution de la
session courante depuis le graphe ; le tokenizer français (mots vides anglais,
pas de radicalisation) ; `obrain-rag::ranking` et `::feedback` (branchés par
personne) ; la latence du DDL d'index, que `property_index_po_bench` ne mesure
plus ; et l'emballement du pont, **corrigé dans les sources mais pas dans le
binaire déployé** (§2.7).
