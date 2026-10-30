# CONSOLIDATION DES CHANTIERS — 2026-09-13

16 chantiers actifs, un worktree chacun. Ce document dit **ce qui est déjà
consolidé, ce qui manque, et dans quel ordre y aller** — les chiffres sont calculés
par `git`, pas recopiés.

## 1. Le socle déjà consolidé

`perf/chemin-nanoseconde` (`8c980863`) contient **7 chantiers** empilés :

- `feat/m6-code-graph` — docs(indexer M6): bench complet queries + RAM (kernel/obrain/drivers) + exemple bench_queries
- `feat/zero-copy-suite` — bench zero_copy_suite : balayage 10k nœuds (GQL vs boucle vs for_each), arène, vues d'arêtes
- `feat/zero-copy-views` — doc : tableau comparatif n3 x86_64 vs DGX GB10 aarch64 des benchs zero-copy
- `integration/zero-copy` — docs: consolidation obrain 2026-09-10 (lignees, shas, production, tests)
- `perf/heap-ultra` — repair-props : l echantillon montre d abord les valeurs courantes
- `perf/gql-projection` — projection : consommer le lot de proprietes au lieu de le recopier
- `perf/agregats-o1` — compteurs : boucher deux fuites, prouver l invariant, epingler ce qui reste

C'est la référence d'intégration : les autres branches devraient partir de là, pas du tronc `e9826f3b`/`8eb2aec1`.

## 2. Ce que la consolidation du jour y ajoute

Branche **`consolidation/2026-09-13`** = socle + `feat/mcp-next-step-hints` :

- 2 conflits seulement, **documentaires** (`blob_column_registry.rs` : union des deux
  jeux de fonctions ; `props_zone.rs` : leur doc rafraîchie + leurs nouveaux tests retenus) ;
- `store.rs`, `execution/operators/filter.rs`, `planner/lpg/filter.rs`, `page.rs`,
  `props_codec.rs`, `wal_store.rs`, les deux bancs : **fusionnés seuls**, sans conflit.

## 3. Les chantiers ABSENTS du socle (à intégrer, un par un)

| branche | dernier commit | fichiers touchés (crates/) |
|---|---|---|
| `feat/recherche-couplee` | obrain_recherche : la recherche couplee du moteur devient  | 40 |
| `perf/voisinage-entrant` | voisinage entrant : re-ancrage sur la cible indexee, 375 0 | 37 |
| `perf/scan-plafonne` | scan plafonne : node_ids_capped, et la garde qui l'empeche | 36 |
| `feat/mcp-next-step-hints` | UN PREFIXE EST UN INTERVALLE : `STARTS WITH` PASSE ENFIN P | 31 |
| `perf/cout-proportionnel` | cout : le banc de B, concu pour etre lu sous charge — et N | 26 |
| `docs/principal-natif` | RFC isolation native : le principal descend dans le substr | 22 |
| `docs/principal-natif-v2` | RFC principal-natif v2 : arbitrages verrouillés, terrain r | 22 |
| `feat/orbites` | ORBITES : le graphe dit quelles echelles il permet, on ces | 22 |

## 4. Surface de conflit : les fichiers chauds

Un fichier touché par beaucoup de branches est un point de collision :

| fichier | branches qui le touchent |
|---|---|
| `crates/obrain-substrate/src/props_zone.rs` | 15 |
| `crates/obrain-cli/src/main.rs` | 12 |
| `crates/obrain-substrate/src/store.rs` | 11 |
| `crates/obrain-cli/src/viz_serve.rs` | 9 |
| `crates/obrain-substrate/src/lib.rs` | 9 |
| `crates/obrain-cli/src/activation.rs` | 8 |
| `crates/obrain-cli/src/commands/compare.rs` | 8 |
| `crates/obrain-cli/src/commands/continuity.rs` | 8 |

Ces trois-là dominent : `store.rs`, `props_zone.rs`, `blob_column_registry.rs` —
c'est le coeur du substrat (propriétés + heap), donc le lieu où toute
consolidation devra arbitrer.

## 5. Protocole de vérification (l'arbitre)

1. `cargo check -p obrain-engine --release` — la compilation seule, dans un `target` dédié ;
2. suites sémantiques : `gql_05_chaines`, `gql_02_comparaison`, `gql_04_nullite`,
   `gql_12_types`, `expression_and_projection`, `filter_pushdown`, `obrain-core --lib` ;
3. les deux bancs : `scripts/bench-filtre-propriete.sh` (fixture figée) et
   `scripts/bench-serveur.sh` (store vivant, par le serveur) ;
4. toute régression mesurée se traite avec un **interrupteur** du type
   `OBRAIN_*`=0/1 pour prouver l'équivalence AVANT de conclure au gain.

## 6. Contraintes matérielles (à connaître avant d'empiler)

- Disque : `43G` libres (96% utilisé). Les autres worktrees
  n'ont pas de `target/` : chaque vérification complète coûte plusieurs Go, donc on
  vérifie par `cargo check` dans un `target` partagé, et on ne construit le binaire
  complet que pour la version retenue.
- `/home/triviere/projects/banc-obrain` est utilisé par **plusieurs agents en même temps**
  (mon travail y est commité ; un autre y laisse en permanence un chantier non commité).
  Toute consolidation doit donc passer par des **worktrees dédiés**, jamais par un
  `checkout` sur ce répertoire.
- Charge de la machine : plusieurs agents mesurent en parallèle. Toute mesure doit
  être **entrelacée** (A/B dans la même fenêtre) ou en **temps CPU**, sinon elle ne veut rien dire.
## 7. VERDICT DE VÉRIFICATION (consolidation/2026-09-13)

- `cargo check -p obrain-engine --release` sur la branche consolidée : **vert**
  (`Finished` en 23 s, `obrain-common`, `core`, `substrate`, `adapters`, `engine`
  tous vérifiés). Les deux conflits résolus ne cassent rien.
- Campagne `cargo test --release -p obrain-engine --no-fail-fast` sur cette branche :
  **toutes les suites vertes sauf deux tests de CONCURRENCE**, qui échouent
  par intermittence quand toute la campagne tourne en parallèle sous une charge
  de 10 à 25 (machine partagée par plusieurs agents). Relancés seuls, trois fois
  chacun : `unicite_merge` 4/4 ×3, `multi_writer_invariants` 9/9 ×3 — donc des
  **scintillements de charge**, pas des défauts de la fusion. La seule suite
  durablement rouge (`annulation_bout_en_bout`) l'était déjà avant cette fusion
  (A/B prouvé plus tôt).
- Conclusion : la consolidation est **vérifiée** (compilation + suites), et les
  deux seuls scintillements sont identifiés avec leur cause.

## 8. Ordre d'intégration proposé pour les 7 chantiers restants

Du plus sûr au plus risqué, **une fusion à la fois, vérifiée avant la suivante** :

1. `docs/principal-natif` puis `docs/principal-natif-v2` — étiquetés doc, mais ils
   touchent 22 fichiers de `crates/` : à vérifier comme du code, pas comme du texte ;
2. `perf/scan-plafonne` — borné et muni de sa propre garde (`node_ids_capped`) ;
3. `perf/voisinage-entrant` — ré-ancrage sur la cible indexée ;
4. `perf/cout-proportionnel` — apporte un banc, conçu pour être lu sous charge ;
5. `feat/orbites` — échelles déduites du graphe ;
6. `feat/recherche-couplee` — le plus gros (72 fichiers), donc en dernier, quand le
   socle ne bougera plus.

Règle : la branche consolidée est le seul endroit où l'on empile ; les chantiers
repartent **d'elle** (rebase) pour éviter de rejouer les mêmes conflits.

## 9. Sauvegarde du chantier non commité du worktree principal

Le répertoire `/home/triviere/projects/banc-obrain` porte un chantier EN COURS qui
n'était que dans l'arbre de travail : index de trigrammes, colonnes de degrés,
détection de cycles, paliers de journal, réparation, dashboard (126 entrées,
dont 76 fichiers non suivis). Il est désormais figé dans

    sauvegarde/worktree-principal-2026-09-13   (92449e8c, 226 fichiers, 30 784 insertions)

Produit sans `checkout`, sans `stash`, sans modifier un seul fichier : l'index a
été écrit puis rendu à son état d'avant, et l'arbre de travail est resté
strictement identique (126 entrées avant et après). Cette branche sert à **ne
rien perdre** ; elle contient aussi des fichiers de store parasites présents à la
racine du dépôt (`substrate.*`), ajoutés parce qu'un instantané prend tout.

Un chantier ne doit plus jamais vivre seulement dans un arbre sale : chacun de ces
16 agents a un worktree et une branche, et le seul qui ne commitait pas est celui
qui partageait le répertoire principal.

## 10. Surface de conflit RÉELLE, par chantier absent

Pour chacun des 8 chantiers absents : commits depuis sa base, fichiers touchés, et
surtout **fichiers où lui ET le socle ont écrit** — c'est là que se jouera la fusion.

| chantier | commits | fichiers | fichiers déjà touchés par le socle |
|---|---|---|---|
| `feat/recherche-couplee` | 86 | 72 | **8** |
| `perf/scan-plafonne` | 86 | 68 | 6 |
| `perf/voisinage-entrant` | 88 | 69 | 6 |
| `feat/mcp-next-step-hints` *(fusionné)* | 86 | 65 | 6 |
| `docs/principal-natif` | 84 | 55 | 4 |
| `docs/principal-natif-v2` | 84 | 55 | 4 |
| `perf/cout-proportionnel` | 85 | 58 | 4 |
| `feat/orbites` | 82 | 54 | 3 |

Les fichiers en cause sont toujours les mêmes : `main.rs`, `viz_serve.rs`,
`Cargo.toml` (structurel, donc facile) et `props_zone.rs`, `store.rs`, `lib.rs`,
`props_eq_index.rs`, `index.rs`, `repair.rs` (le cœur du substrat, donc coûteux).

### Redondances repérées (à arbitrer pendant la fusion, pas après)

- `incoming_heads` est présent **des deux côtés** (socle *et* `perf/voisinage-entrant`),
  qui porte par ailleurs un test que le socle n'a pas (`voisinage_entrant.rs`) :
  probable recouvrement partiel, à trancher par la mesure, pas par le nom.
- **Trois** chantiers (`perf/scan-plafonne`, `perf/voisinage-entrant`,
  `perf/cout-proportionnel`) ajoutent le MÊME banc `[[bench]] name = "plage_prefixe"` :
  conflit textuel garanti sur `Cargo.toml`, contenu identique — trivial à résoudre,
  mais à résoudre UNE fois, pas trois.

## 11. LA CAUSE RACINE : deux lignées, et un tronc qui dépend d'un chantier non commité

En voulant construire le binaire consolidé, le CLI a échoué — et la raison n'est
pas la fusion, c'est l'état du dépôt :

1. **Le tronc `342a61a1` (HEAD de la branche `feat/mcp-next-step-hints`) n'est pas
   auto-cohérent.** Son `crates/obrain-cli/src/viz_serve.rs` (COMMITÉ) appelle
   `crate::ancrage::self_qn`, `crate::engine::Ancrage`, `crate::viz_weave::slug` —
   or `ancrage.rs` est un fichier **NON SUIVI**, et il manque aussi
   `commands/entretien.rs`, `commands/hubs.rs`, `commands/memoire.rs`,
   `commands/outils.rs`, `viz_serve::cached_ro_si_present`, `viz_extract::PhasesUs`.
   Autrement dit : **ce commit ne se construit que dans le worktree principal**,
   là où ces fichiers existent à l'état non suivi. Un `git clone` du dépôt à ce
   commit ne compile pas.
2. **Les branches sont sur DEUX lignées.** Le socle d'intégration
   (`perf/chemin-nanoseconde`) descend de `e9826f3b`, un tronc ANTÉRIEUR : son CLI
   n'a ni `engine.rs`, ni `viz_weave.rs`, ni `ancrage.rs`, et son `viz_serve.rs`
   ne connaît pas `ancrage` (0 occurrence). Il est donc cohérent *de son côté*,
   mais il ignore tout le CLI récent.
3. **Conséquence** : fusionner « socle + travail récent » donne un hybride — la
   structure du CLI d'un côté, les appels de l'autre. Ce n'est PAS un conflit git
   (git n'a rien signalé) : c'est un conflit de LIGNÉES. Importé depuis la
   sauvegarde, le CLI redemande alors `props_decode_failures` (compteur du
   chantier) et une fonction à 12 arguments — donc le **substrat du chantier**.

**Ce que ça impose.** Le chantier non commité n'est pas un chantier « en plus » :
c'est la **moitié manquante du tronc**. Tant qu'il n'est pas commité sur sa propre
branche, aucune consolidation n'est possible — ni la mienne, ni une autre.

**Ordre correct, à partir d'ici :**

1. **committer le chantier** (il est déjà figé dans
   `sauvegarde/worktree-principal-2026-09-13`, 226 fichiers) sur une branche à lui,
   `feat/…-tronc-complet` par exemple, après l'avoir découpé si possible ;
2. **choisir la lignée** : le tronc récent + chantier devient la base ;
3. y **fusionner le socle** `perf/chemin-nanoseconde` (7 chantiers de perf) —
   c'est là que se joueront les vrais conflits (`store.rs`, `props_zone.rs`,
   `blob_column_registry.rs`) ;
4. puis les 7 chantiers restants, un par un, avec le protocole du §5.

Ma consolidation (`consolidation/2026-09-13`) reste utile comme **preuve que la
fusion est faisable** : moteur vérifié, 2 conflits documentaires, et la liste
exacte de ce qui manque est désormais écrite noir sur blanc.
