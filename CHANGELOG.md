# Obrain Changelog

All notable changes to Obrain will be documented in this file.

This project was forked from [GrafeoDB/grafeo](https://github.com/GrafeoDB/grafeo) and rebranded as **Obrain** ([obrain.dev](https://obrain.dev)). The upstream Grafeo changelog is preserved in `CHANGELOG-grafeo.md` for historical reference.

## [0.0.1] — en préparation (branche de release, 2026-11)

### Durabilité des valeurs longues (obrain#120)

- Une valeur de plus de 256 octets (chaîne, octets, routée en colonne blob) n'était
  pas couverte par le WAL : un commit acquitté ne la protégeait pas d'un `kill -9`
  (mesuré : 0/40 relues), et une réécriture non flushée suivie d'un ajout faisait
  relire à un nœud les octets d'un autre (mesuré : 40/40 contaminés).
- **Perte, corrigée pour l'écrivain exclusif** : deux trames WAL `BlobColumnWrite`
  (valeur comprise) et `BlobColumnClear`, rejouées à la réouverture sur la fenêtre
  post-flush par un écrivain exclusif. Un écrivain en écriture PARTAGÉE ne rejoue
  pas (l'arène est à ses pairs) : une valeur longue qu'il a acquittée se perd
  encore au `kill -9` avant flush (limite, `docs/LIMITES-v0.0.1.md`).
- **Contamination à la REPRISE après un arrêt, corrigée** : l'écrivain qui rouvre une
  colonne recalcule la fin d'arène comme le maximum de `offset + len` de l'index
  (`BlobColumnWriter::create`). Une écriture de colonne restée sans trame — kill
  entre la colonne et la trame, valeur au-delà du plafond, écriture partagée — ne
  se fait plus écraser par l'ajout suivant. Trouvé par la relecture de #123. Ne
  couvre PAS deux écrivains VIVANTS en écriture partagée, qui gardent chacun leur
  fin d'arène en cache et s'écrasent toujours (limite ouverte, `docs/LIMITES-v0.0.1.md`,
  ligne « arène des blobs »). Une entrée d'index écrite au-delà de `n_slots` sans
  sync est protégée mais pas effacée : si un slot plus haut est écrit, elle
  redevient lisible avec sa dernière valeur (testé).
- Tests : `obrain-engine/tests/blob_hors_wal_kill9.rs` (6 scénarios `kill -9` et 2
  limites verrouillées), `blob_column::tests` (arrêt sans sync puis ajout).
- Plafond : une valeur de plus de 32 Mio n'est pas journalisée, donc perdue au
  `kill -9` avant flush ; elle ne contamine plus aucun autre nœud (limite
  verrouillée).
- Compatibilité descendante, décision : **pas de bump de version** du format. Un
  binaire qui connaît le refus d'opcode inconnu (depuis be927bd5, 07/11/2026)
  REFUSE d'ouvrir une base portant des trames 0x29/0x2A en nommant l'opcode
  (`WalOpcodeInconnu`), sans rien modifier ; un binaire antérieur au 07/11
  s'arrête à la première comme sur une frontière de crash. Ne pas rétrograder
  après écriture.

### Contraintes d'unicité composites (PR #99) — changements visibles

- **`UNIQUE (a, b)` porte sur le tuple**, pas sur chaque colonne (jusqu'ici : « a unique ET b
  unique », qui refusait des lignes légitimes). `NODE KEY` idem.
- **Les contraintes sont persistées** (`substrate.typecat`) et rechargées à l'ouverture ; elles
  ne se rejouaient pas au démarrage. Nouveau champ JSON `constraint_names` (lecture compatible
  dans les deux sens, voir la description de la PR), nouveau champ `implicit` (idem).
- **`DROP CONSTRAINT <nom>` retire réellement la contrainte** (c'était un no-op) ; `SHOW
  CONSTRAINTS` liste les contraintes (nom, nature, label, propriétés) au lieu d'une table vide.
- **Redéclaration idempotente** ; sans `IF NOT EXISTS`, redéclarer une contrainte équivalente est
  maintenant une erreur : `Constraint already exists: an equivalent unique constraint '<nom>' is
  declared on :<label> (<colonnes>)` (auparavant : « Created » et une copie de plus).
  Le nom d'une contrainte déjà prise par une AUTRE : `Constraint already exists: the name '<nom>'
  designates another constraint`. `DROP CONSTRAINT` d'un nom inconnu : `Constraint not found:
  <nom>` (sauf `IF EXISTS`).
- **Nom par défaut** d'une contrainte sans nom : `<label>_<nature>_<colonnes>` (ex. `T_unique_a_b`),
  le même à la création et à la réparation d'une base ; `EXISTS` et `NOT NULL` ont la nature
  `not_null`. Si une colonne contient `_`, une empreinte des colonnes est ajoutée
  (`T_unique_a_b_3f0c1a`) pour que `UNIQUE (a, b)` et `UNIQUE (a_b)` ne partagent pas un nom.
- **`CREATE CONSTRAINT` qui ne peut pas être persistée échoue** (au lieu de « Created » puis une
  contrainte qui disparaît au redémarrage) ; le DDL est refusé en lecture seule ; un
  `substrate.typecat` illisible fait échouer la déclaration au lieu d'être réécrit.
- **`CREATE [OR REPLACE] NODE TYPE` conserve les contraintes** du type ; sur un type créé
  implicitement par `CREATE CONSTRAINT`, il le complète.
- **`CREATE CONSTRAINT` sur des données qui violent déjà la contrainte est refusé**, en nommant
  un doublon (deux nœuds et leurs valeurs) ou le membre absent ; auparavant « Created » et les
  doublons restaient. Liste complète des doublons d'une contrainte envisagée :
  `ObrainDB::verifier_contrainte_hypothetique` ; état de toutes les contraintes déclarées sur les
  données : `ObrainDB::verifier_contraintes`.
- **L'unicité reconnaît les doublons que le langage tient pour égaux** : `5` et `5.0`, deux chaînes
  date-heure du même instant dans deux fuseaux, une chaîne de plus de 256 octets (rangée en colonne
  blob, jamais dans l'index d'égalité), sur une colonne seule comme dans un tuple, dans les deux
  ordres d'insertion et de déclaration. La règle est en un seul endroit
  (`SubstrateStore::index_borne_les_porteurs`) : l'index ne borne les candidats que si la classe
  d'égalité est la clé, que la valeur est de celles que l'index reçoit, que la clé est enregistrée et
  que l'index est bâti ; sinon balayage du label (O(label) : mesuré ×800 à 100 000 nœuds pour un tuple
  tout entiers, voir `docs/LIMITES-v0.0.1.md`). L'import en lot, qui n'alimente pas les index, retire
  leurs marqueurs : ils sont rebâtis au prochain usage. Nouvelle méthode de trait
  `GraphStore::find_nodes_by_properties_for_label` (défaut : balayage du label).
- **Coût du contrôle d'un tuple indépendant de l'ordre des colonnes** : la recherche par
  propriétés résout par la condition indexée la plus sélective, plus par la première déclarée
  (`UNIQUE (a, b)` avec `a` à deux valeurs coûtait 30 fois `UNIQUE (b, a)`).
- **NON couvert par cette version** (chemins traités par la PR empilée « unicité sur tous les
  chemins ») : l'ajout de label `SET n:T` n'est pas contrôlé ; l'API directe du store et
  `bulk_import*` n'appliquent aucune contrainte ; deux `INSERT` concurrents du même tuple peuvent
  passer tous les deux. Sur une base construite par `ObrainDB::with_store` (store externe), une contrainte
  n'est pas durable (journalisé). Voir `docs/LIMITES-v0.0.1.md`.
- **Rupture d'API (crate `obrain-core`)** : le trait public `ConstraintValidator` perd
  `check_unique_node_property(labels, key, value)` au profit de
  `check_unique_node_properties(labels, properties, exclude)` (jeu de propriétés effectivement
  écrit, nœud mis à jour à exclure) ; nouvelle méthode `unique_constrained_properties(labels)`
  (défaut : aucune). Tout implémenteur externe doit s'adapter.

### Batterie exhaustive (V1-EXHAUSTIF) — ce que `--all-features` a révélé

- Interblocage déterministe de `CALL` cognitif `bootstrap` (`create_synapse_slots_batch`
  gardait le verrou du substrat que `write_edge` reprend) : corrigé.
- Un second `ObrainDB` inscriptible sur la même base dans le même processus était
  accepté : refusé désormais (lecture seule toujours permise).
- Feature `temporal` non fonctionnelle sous substrat, feature `rdf` non persistante :
  documentées dans `docs/LIMITES-v0.0.1.md`, non corrigées.
- Les tests de défauts connus passent de la feature `defauts-connus` au cfg
  `defauts_connus` (`RUSTFLAGS="--cfg defauts_connus"`), pour que `--all-features`
  ne les joue plus ; la CI joue maintenant `cargo test --workspace --all-features`.
- `detailed_stats().edge_type_count` comptait un type de plus que ceux créés (tests
  `database_api`, morts depuis que `cypher` n'est plus par défaut) : voir ci-dessous.

### Limite mesurée (V1-T2B)

- Écriture multi-processus : le LSN d'un écrivain préempté plus de 25 ms peut
  être inexact (frame posée après celle du successeur qui l'a pris pour mort) ;
  zéro perte, EOF exact. `t2b`/`t2c` ré-ignorés avec la mesure (3/172 à 25 ms,
  4/31 à 100 µs), exécutés par `mesures.yml`. Détail : `docs/LIMITES-v0.0.1.md`.

Première version destinée à être publiée. Le numéro du workspace est `0.0.1`
depuis des mois ; les tags `v0.5.x` du dépôt viennent de l'amont GrafeoDB et
l'entrée `[0.2.0]` plus bas est un héritage du fork — aucune des deux n'est une
release d'Obrain. Cette entrée est la première qui en soit une.

### Intégrité des données (mesuré, test rouge sur `main` e11bdd2a puis vert)
- **Transactions en vol au rejeu** : un processus qui disparaissait (`kill -9`)
  avec une transaction OUVERTE laissait ses nœuds et arêtes publiés à la
  réouverture (50/50 mesurés). Le WAL porte désormais des frontières de
  transaction (`TxNoeudEnAttente` 0x56, `TxAreteEnAttente` 0x57, `TxCommit`
  0x58) et le rejeu écarte ce qui n'a jamais été scellé. Test :
  `obrain-engine/tests/durabilite_commit_kill9.rs`.
- **Commit scellé et `fsync`** : `Session::commit()` pose un marqueur de commit
  et, en `SyncMode::EveryCommit` (le défaut), attend le `fsync` — un commit
  acquitté survit à une coupure de courant, pas seulement à un `kill -9`.
  `SyncMode::Group` reste opt-in et sans minuterie (doc corrigée).
- **Commit refusé** : un conflit de validation laissait les entités de la
  transaction allouées sur disque (invisibles en mémoire, de retour après
  réouverture). Le rembobinage a lieu avant le vidage des empreintes, comme
  pour `ROLLBACK`. Test : `obrain-engine/tests/echec_commit_rembobinage.rs`.
- **Sidecar de propriétés** : un `substrate.props` d'une autre version ou
  corrompu était lu comme VIDE (warn) puis écrasé au flush suivant. C'est une
  erreur d'ouverture, désormais.

### Fonctionnalités rendues vraies (mesurées par `crates/obrain/tests/readme_examples.rs`)
- Procédures nouvelles : `astar(source, target)`, `triangle_count`, `k_core` (alias de
  `kcore`), `jaccard`, `cosine_similarity`, `adamic_adar` (raccourcis de `similarity`).
- Sept algorithmes implémentés mais jamais enregistrés deviennent appelables :
  `personalized_pagerank`, `stable_communities`, `irreducible_kernel`,
  `spectral_embedding`, `heat_kernel_diffusion`, `hilbert_features`,
  `subgraph_contraction` (clé du registre normalisée sans le préfixe `obrain.`).
- Features `cognitive` / `cognitive-full` sur le crate `obrain` ; les UDF
  `obrain.energy`, `obrain.risk`, `obrain.synapses` sont enregistrées à la
  construction du moteur cognitif ; le parseur GQL accepte les noms de fonction
  qualifiés (`obrain.energy(id(n))`).

### Format on-disk
- Format du répertoire substrat figé à la version 3 ; `docs/rfc/substrate/
  format-spec.md` § 0 décrit l'état réel ; fixture de référence
  `obrain-engine/tests/golden/v3-mini` verrouillée par `format_fige_v3.rs`.
- Suppression du module orphelin (non compilé) du format mono-fichier
  `.obrain` (`obrain-adapters/src/storage/file`).

### Plateformes
- macOS : un refus de verrou nomme le processus détenteur (`lsof`), comme sur
  Linux (`/proc`).

### CI
- Actions épinglées par SHA, `permissions: contents: read`, jobs : lint
  (cliquet clippy), tests, `--all-features` (compilation), MSRV, `cargo deny`,
  `cargo package` dans l'ordre de publication, macOS (tests d'intégrité).
- Chemins Codecov, Dependabot, Makefile et `ci-local.sh` débarrassés des
  références à `lab/grafeo` et à des bindings absents.

### Documentation
- `NOTICE` (attribution GrafeoDB, Apache-2.0) ; `repository` du manifeste
  pointe vers `obrain-db/obrain`.
- `docs/LIMITES-v0.0.1.md` : chaque limite connue, avec le test qui la
  verrouille. `scripts/check-links.sh` : plus aucun lien relatif cassé.
- README : voir la PR V1-README (fonctionnalités = celles que
  `crates/obrain/tests/readme_examples.rs` exécute).

## 2026-09-15 — LE FROID D'`/api/viz/arcs` CESSE D'ÊTRE PAYÉ PAR UNE REQUÊTE

*Constat de départ : « cette requête met plus de 3 s, toutes les requêtes de ce
type devraient être instantanées ». Le chiffre était exact, mais il ne désignait
pas ce qu'on croyait. Mesures sur le service 8432, `runtime-dev` (439 688 nœuds
/ 1 324 105 arêtes).*

### Le régime chaud était déjà instantané — c'est le froid qui coûtait

| régime | mesuré |
|---|---|
| cache chaud (clé rejouée) | **0,3–1,2 ms** |
| cache vide, store déjà ouvert | **2 050 ms** |
| cache vide **et** cache disque vide | **6 240 ms** ← les « >3 s » |

Deux causes, cumulées, et aucune n'était l'agrégat :

1. **Le cache disque vit dans `/tmp/obrain-viz-cache`**, effacé à chaque reboot —
   le service servi n'arme pas `OBRAIN_VIZ_CACHE_DIR`. La première visite après
   un redémarrage recalcule donc tout. *Action d'exploitation, aucun code :*
   pointer `OBRAIN_VIZ_CACHE_DIR` vers un chemin persistant.
2. **Le calcul était DANS le gestionnaire de requête.** Sur les 2 050 ms :
   agrégat utile ~700 ms, choix du rang ~400 ms, sondes d'existence des rangs
   ~800 ms, artefacts de build ~200 ms. Les quatre cinquièmes du coût étaient des
   **sondes** ne décrivant qu'une structure immuable.

### Ce qui change

- **`arcs_corps`** : le corps est extrait de `api_viz_arcs` en une fonction
  appelable HORS requête. La route ne fait plus que le tronquer (`tronquer_arcs`).
- **`prechauffer_les_arcs`** : l'agrégat du monde par défaut se pose au démarrage,
  en tâche de fond (`OBRAIN_PRECHO_ARCS_MS`, défaut 30 s ; `0` désarme), sur la
  forme exacte que le dashboard demande (palier `limit ≤ 1024`, rang automatique).
  Écrit dans les **mêmes** caches que la route, sans quoi il ne servirait à rien.
- **Moteur — `split(s, d)[k]` ne matérialise plus la liste.** L'idiome
  d'indexation de chemin de tout le dépôt construisait la `Vec<Value>` entière
  (une chaîne allouée par token) pour n'en lire qu'une position. Décomposition sur
  `runtime-dev`, par étage :

  | étage | avant | après |
  |---|---|---|
  | balayage des arêtes CALLS, `count(r)` | 80 ms | 80 ms |
  | + `a/b.qualified_name` | 340 ms | 340 ms |
  | + `split(a.qn,'.')[4]` | 640 ms | **470 ms** |
  | + `split(b.qn,'.')[4]` | 1 000 ms | **680 ms** |
  | + `GROUP BY sa, sb, count(r)` | 1 050 ms | **710 ms** |

  Le groupement — ce qu'on croit toujours être le coût — pèse **50 ms**. Ce qui
  pesait, c'est de payer 2,6 M découpages pour 439 688 nœuds distincts.

### Vérifications

- **Première visite, cache neuf** : 0,67 ms avec préchauffage contre 2,23 s sans,
  à code identique par ailleurs (deux instances isolées, même store).
- **Charge utile identique** : md5 `c2e4b973…` pour la production, l'instance
  préchauffée et l'instance froide — l'optimisation moteur ne change pas la
  réponse, à l'octet.
- **Sémantique de `split`** : `split_indexe_suit_la_voie_generale_sur_les_cas_limites`
  (séparateur vide, séparateur doublé, index négatif, hors bornes, argument non
  textuel, index calculé, non entier) — **56/56** sur `gql_05_chaines`, **870/870**
  sur `obrain-core`.

### Ce qui reste ouvert

- Les **autres paliers** (`limit > 1024`) et les **autres mondes** ne sont pas
  préchauffés : ils se paient à la demande, une fois.
- Le **premier `/api/viz/weave`** (~3,9 s) et les agrégats `type d'arête` /
  `labels(a)/labels(b) GROUP BY` restent lents — ils exigent d'écrire dans le
  moteur ou le substrat.
- Le **cache disque en `/tmp`** reste à déplacer côté exploitation (point 1).

## 2026-09-14 — SIX RÉGLAGES ARMÉS QUE LE BINAIRE SERVI NE LIT PAS, ET UNE GARDE POUR QUE ÇA SE VOIE

*Constat établi en tentant de débloquer la construction de l'index de trigrammes
sur le store vivant, comme le laissait ouvert `0794f2ac`. Le verrou nommé là
n'était pas le bon. Détail, mesures et réserves :
`docs/ECART-DEPLOIEMENT-LIGNES-2026-09-14.md`.*

### Le binaire servi n'est pas celui de ce tronc

- `md5sum` identique, à l'octet : le binaire du harness est celui de la lignée
  `feat/principal-natif-j5` (construit 13:22:56), pas celui du tronc. Le tronc
  ne peut même pas **ouvrir** les bases servies — elles sont en v3 depuis le
  14/09 12:29, il est resté en `SUBSTRATE_FORMAT_VERSION = 1`
  (`unsupported format version 3 (expected 1)`).
- La lignée j4 (12:33) a bien **fusionné** ce tronc — v3 **et** les réglages —
  mais j5 ne descend pas de j4 (75 commits d'écart), et **aucun binaire j4 n'a
  jamais été construit**.

### Ce qui est armé n'est pas ce qui est lu

- `strings` sur le binaire servi, contre l'environnement réel du processus :
  `OBRAIN_IAM`, `OBRAIN_IAM_ACCOUNT`, `OBRAIN_IAM_STORE`,
  `OBRAIN_TRIGRAMMES_DES_LA`, `OBRAIN_TRIGRAMMES_PERSISTER`,
  `OBRAIN_PREFIXE_INTERVALLE` — **six réglages armés, six inertes**.
- Trois preuves qui ne passent pas par `strings` : `nm` ne trouve **aucun**
  symbole `obrain_iam` (la lignée j5 n'a pas de chaîne IAM — `grep -c iam` sur
  son `viz_serve.rs` : 0) ; `/api/query` répond **200 sans identité**, là où
  `verif-chaine-iam.sh` exige 401 ; et le drop-in `trigrammes.conf` a été écrit
  **11 minutes après** la construction du binaire qu'il prétend régler.
- Le comportement mesuré n'a donc rien à voir avec ces réglages : sur le store
  **vivant**, `qualified_name CONTAINS` coûte 53,9 / 63,2 ms ; sur le store
  **statique** de la même famille, 0,54 / 0,47 ms — même processus, même code,
  même forme de requête. La zone de trigrammes sert l'un, pas l'autre.
- L'acte qui donnerait ses séries au store vivant (`obrain index rebuild`)
  n'est dans **aucun** chemin de redémarrage : absent de
  `scripts/entretien-bases.sh`, et ce script pointe sur un binaire v1.

### La garde

- **`scripts/verif-env-deployees.sh`** confronte ce qui est armé (environnement
  du processus servi, ou unité et drop-ins à défaut) à ce que le binaire lit.
  Elle accuse le binaire déployé (6 inertes, sortie 1) et blanchit celui du
  tronc (sortie 0) — c'est la mesure de l'écart, et de lui seul.

### Une mesure jetée, et pourquoi elle est écrite ici

Un relevé a affiché 936 µs / 157 µs / 143 µs là où les autres donnaient
170–300 ms. Il **ne s'est pas reproduit** (trois tentatives). Consigné parce
qu'un chiffre de cet ordre aurait suffi à conclure « résolu » — même famille que
la fausse alerte du 13/09, où une régression imaginaire venait d'une requête de
mesure modifiée entre deux campagnes.

## 2026-09-13 — ADMINISTRATION IAM : LES COMPTES DEVIENNENT ADMINISTRABLES, ET « SUPPRIMER » SE MET ENFIN À RETIRER

*Chantier mesuré sur des magasins d'identités jetables (`/tmp`) et un serveur
armé sur port libre — jamais sur 8432/8433 (vérifiés intacts après coup). Portes :
`scripts/acceptation-iam.sh` (bout en bout CLI + HTTP — **tout passe**) et
`cargo test -p obrain-iam --all-features` (**139 tests**). Détail complet :
`docs/ADMINISTRATION-IAM-2026-09-13.md`.*

### Le défaut trouvé en chemin : supprimer ne retirait aucun droit

- `delete_policy` et `delete_role` marquent `iam_status = "deleted"` depuis
  toujours, mais `node_to_policy` / `node_to_role` **ne lisaient jamais ce
  champ**. Or `evaluate_access` collecte les politiques **en passant par les
  rôles** : une politique ou un rôle « supprimé » **continuait d'accorder ses
  droits**. Mesuré par un test d'administration — après un retrait d'espace, la
  session dont l'espace venait d'être fermé **passait toujours**.
- Correctif : les deux fonctions rendent `None` sur `status == "deleted"` ;
  **statut absent ⇒ actif**, sinon les identités écrites avant le correctif
  seraient toutes invalidées. Deux tests de régression le tiennent
  (`store::tests::supprimer_une_politique_retire_le_droit`,
  `supprimer_un_role_retire_le_droit`).

### Une seule surface manquait, et elle manquait entièrement

- La bibliothèque portait déjà tout le CRUD ; **rien ne l'exposait** : l'exemple
  `amorcer_iam` créait un compte sans en permettre un second, le serveur ne
  servait que `login`/`logout`/`whoami`/`refresh`/`audit`, et aucune
  sous-commande `obrain iam` n'existait. Ajouter un collègue supposait d'écrire
  un programme.
- **Un noyau, trois surfaces** : `obrain-iam::admin` (opérations + garanties),
  `obrain iam` (hors bande, sur un fichier), et des routes HTTP + une page
  `/iam` (plafonnées par les droits de l'appelant).

### Trois garanties, chacune tenue par des tests

- **Le plafond se LIT, il ne se déclare pas.** `plafonnee_par_l_acteur` déduit le
  plafond des politiques de l'acteur dans le magasin — une surface qui le
  déclarerait elle-même pourrait se décrire plus large que ses droits. Défaut
  **fermé** : un acteur sans politique n'accorde rien.
- **Aucun verrouillage possible.** Le dernier administrateur actif ne se
  désactive pas, ne se dégrade pas, ne s'efface pas, et sa politique
  d'administration ne se supprime pas — trois chemins, tous gardés, et le verrou
  se débloque en promouvant un second compte.
- **Tout geste est tracé sur l'action qu'il a faite** (`iam:user:create`,
  `iam:user:disable`, …), avec l'**acteur** en principal et le **compte visé** en
  ressource.

### Le serveur : deux barrières, et un espace NOMMÉ

- Treize routes d'administration, toutes gardées par l'action `iam:admin` **au
  point de contrôle unique** qui couvrait déjà les routes de données — une route
  ajoutée demain est gardée sans qu'on y pense — **et** plafonnées sur les droits
  réels de l'appelant dans chaque gestionnaire. La seconde barrière ne fait pas
  confiance à la première.
- **Un espace nommé, jamais un défaut** : sans `espace=`, la route rend **400**.
  La juger sur l'espace par défaut du serveur reviendrait à trancher sur un
  espace que la requête ne vise pas. Mesuré : `401` sans jeton, `400` sans espace,
  `403` hors du plafond, `200` dedans.
- **`/api/iam/moi`** exige une identité mais aucun droit : on n'y lit que soi.
  C'est ce qui permet à la page `/iam` de savoir sur quel espace se juger au lieu
  de le deviner. La page est une **coquille sans données** — `/iam` n'étant pas
  sous `/api/`, une page qui rendrait la liste côté serveur serait **publique**.
- Les mutations ne passent pas par `GET` (mesuré : `405`), et **refusent**
  d'écrire sur un magasin d'identités ouvert en lecture seule plutôt que de
  répondre « fait » sans rien changer.

### On n'administre que ce qu'on détient

Le plafond limitait ce qu'on peut ACCORDER, pas ce qu'on peut ATTEINDRE : un
administrateur du seul espace `e1` pouvait désactiver ou effacer un compte qui ne
vit que dans `e2` — il n'obtenait pas `e2`, mais il en fermait l'accès. La portée
se vérifie désormais sur la **cible**, à un seul guichet par lequel passent
toutes les opérations publiques.

### Refus explicites, parce qu'ils comptent autant que les gestes

- `emettre_jeton` et `amorcer` **refusent** sur une administration plafonnée :
  fabriquer un jeton pour autrui contournerait la connexion, et l'amorçage pose
  `iam:admin`.
- Les noms suivent une **grammaire fermée** : `a/b` est refusé (« il sépare ou
  découpe un ORN ») plutôt que de courir après une liste noire.
- L'effacement RGPD exige `--oui` et refuse sans lui ; un jeton ou un mot de
  passe ne sont jamais relisibles — les listes rendent identifiant et échéance,
  jamais la valeur.

## 2026-09-13 — Le 8432 : LE TRANSPORT EST AU GOÛT DU JOUR, LA CHAÎNE MULTI-DIMENSION ABOUTIT, LA TRAÎNE SE VOIT

*Chantier mesuré sur le service vivant (`obrain viz serve`, store `runtime-dev`
≈ 412 773 nœuds) et sur instances isolées. Portes : `scripts/protocole-check.mjs`,
`scripts/vitesse-8432.mjs`, `scripts/projection-check.mjs`,
`scripts/fratrie-check.mjs`, `scripts/acceptation-8432.sh`. Détail complet et
chiffres traçables : `docs/8432-PROTOCOLE-ET-VITESSE-2026-09-13.md`.*

### Protocole : 14 échecs sur 15 → tout passe

- **Faille fermée** : `/app/../../…` servait **n'importe quel fichier lisible**
  (`200` + contenu), hors garde IAM, parce que le chemin était joint sans être
  assaini alors que la route *vendor* voisine portait la garde complète.
  Désormais **404**.
- **Compression négociée** (`gzip`, `Vary: Accept-Encoding`, jamais sur
  `text/event-stream`) : bundle **1 307 553 → 384 463 o sur le fil (−70,6 %)**,
  JSON d'API **580 o** au lieu de 14 192.
- **Keep-alive borné** (nombre de requêtes ET fenêtre d'inactivité), avec les
  **délais de lecture/écriture** posés — sans eux, garder une connexion ouverte
  transformerait un client immobile en fil et descripteur perdus. Les 352 sockets
  TIME-WAIT relevées au départ venaient de là.
- **`HEAD`** servi sans corps, **`Date`** sur toute réponse (RFC 9110 §6.6.1),
  **`ETag`/304**, `Allow:` sur les 405, **400** au lieu d'un silence qui fait
  pendre le client, `413`/`431`/`505` nommés.
- **SSE** : `retry: 2000`, battement de cœur, `X-Accel-Buffering: no` — et la
  fermeture VOULUE du flux, seule à délimiter sa fin (surtout pas de `chunked`,
  qui ferait pendre `scripts/chat-reprise-check.mjs` jusqu'à ses 180 s).
- HTTP/2 reste **écarté** dans ce binaire (« std-only », `TcpStream` nu, sans
  TLS ; h2c n'existe pas côté navigateur). La voie, si on la veut, est un frontal
  — et ce keep-alive est ce qui le rend possible.

### Vitesses : les postes mesurés, et ce qui tombe

| poste | avant | après |
|---|---|---|
| `/api/viz/arcs`, `top` neuf | **2 523-6 667 ms** pour 3 413 o | **~3 ms** |
| `/api/viz/space`, préfixe neuf | 586-2 025 ms | ouverture de base supprimée (`cached_ro`) |
| `/api/viz/subgraph` (2,2 Mo) | clone sous verrou | `Arc<str>` |
| cache disque | **1 821 fichiers / 36 Mo**, sans élagage | **407 entrées** |
| préchauffage de trigrammes | 1 propriété | **4 propriétés par monde** (visible en `warn!`) |

`arcs` payait l'agrégat de rang **trois fois** et indexait son cache par
`(store, top, limit)` : un `top` neuf repayait tout. L'agrégat est calculé à
`engine_limit` fixe puis tronqué, donc `top` ne change pas la réponse — la clé ne
le porte plus, **et la correction est vérifiée** : `top=16`, `48`, `220` rendent
les mêmes 73 arcs, dans le même ordre.

Restent lents, et c'est dit : le **premier** `/api/viz/weave` (~3,9 s) et les
agrégats moteur `type d'arête` (1 151-1 219 ms) et `labels(a)/labels(b) GROUP BY`
— ils exigent d'écrire dans le moteur ou le substrat.

### Chaîne multi-dimension : elle était complète dans UNE seule face

- **Le maillon d'ancrage ne relançait rien** dans la face par défaut : cliquer un
  groupe de la projection émettait **zéro** requête de graphe (`StageContinu`
  avait un centre codé en dur et ne lisait ni `centersAtom` ni
  `requestTokenAtom`). Réparé, avec coalescence de rafale : **6 clics → 1 requête**.
- **`projectAxes` levait `RangeError` vers 125 000 lignes** (`Math.min(...vals)`,
  dans le rendu, sur un endpoint sans borne) et construisait 8 000 groupes pour
  en afficher 16. Un parcours unique remplace les deux passes ; la construction
  est bornée à 64 groupes, avec `total`/`groupCount`/`hidden` **exacts** pour que
  les pourcentages ne mentent pas.
- **La traîne se voit et se parcourt** : « N groupes · K affichés · M masqués »,
  « tout voir » en liste **fenêtrée** (20 lignes DOM pour 157 groupes), filtre
  texte, et un clic rend la part (rang/part/effectif) en plus d'ancrer.
- Au-delà de 20 000 lignes, la projection **ne se lance plus d'office** : elle dit
  le nombre de lignes et propose « ajouter LIMIT 20000 » ou « projeter quand même ».
- Réseau : cache de résultats de requête (20 s), `fetchSubgraph` mémoïsé,
  recherche de fonctions débouncée (**2 requêtes pour 6 frappes**), activation à
  TTL réel (sondage toujours vivant à 1,20 s).
- Défaut trouvé en chemin : `?store=<X>` posait les ancres du store par DÉFAUT du
  serveur — un symbole Rust dans un store Linux, donc un `400` et une scène vide
  sans rien dire.

### Frères et sœurs : une seule règle de parenté, et elle est réparée

- La parenté se dérivait **deux fois**, par deux règles incompatibles qui
  divergeaient sur **la totalité des 15 106 qn portant `__file__`** : la puce de
  filtre comparait donc des étiquettes qu'elle ne pouvait structurellement jamais
  allumer. Une fonction canonique remplace les deux, et la puce **allume
  réellement** ses nœuds (mesuré : 1 964 nœuds sur 11 078).
- `Math.min(...a)` sur la phase « complet » (200 000 nœuds) levait `RangeError`
  **avalée en silence** : un seul parcours désormais (10,7 ms mesurés).
- L'index de fratrie (frères de parent, sœurs de communauté, co-fichiers,
  co-modules) dérive du graphe **déjà chargé**, sans requête supplémentaire.

### Le protocole de mesure fait partie du résultat

Trois instruments de ce chantier ont rendu des chiffres FAUX avant d'être
justes, et la leçon est écrite dans les scripts : **(1)** deux réponses HTTP se
suivent sans séparateur — une regex ancrée `^HTTP/1.1` en comptait une seule, et
j'ai failli déclarer cassé un keep-alive qui marchait ; **(2)** `fetch` dégonfle
lui-même, donc mesurer la compression par la taille du corps rend « aucun gain »
là où il est de 70 % ; **(3)** un nonce de cache en espaces de fin ne force une
exécution qu'**une fois** — relancer avec le même texte relit le cache et fait
passer 1 079 ms pour 1,2 ms. Enfin : mesurer l'interface pendant qu'un agent
édite `src/` est invalide, le veilleur (re)construit le bundle et la page se
recharge sous la sonde.

## 2026-09-13 — Le chat : une coupure est REPRISE, un `tool_call` n'est pas une réponse

*Défauts refusés par l'utilisateur, mesurés sur le chat du port 8432.*

### Le flux coupé est repris au lieu d'être annoncé incomplet

- **Cause mesurée** : `viz serve` est TUÉ en pleine réponse par un déploiement
  (journal systemd : `status=9/KILL`, 1 248 redémarrages comptés). La connexion
  SSE meurt, et la vue n'avait qu'un mot à écrire — « flux interrompu avant la fin
  — ce qui précède est incomplet ». Une panne de TRANSPORT devenait une réponse
  perdue.
- **Serveur** : `demander_stream` accepte une `reprise` (le texte déjà produit) ;
  le paramètre HTTP `deja` la transporte, borné à 8 000 caractères et par la QUEUE
  (le raccord se fait en fin de texte). Le tour rejoué n'offre AUCUN outil : il
  continue une prose, il ne rouvre pas une exploration.
- **Seconde cause, silencieuse et pire** : `post_sse` rendait `Ok` sur une
  fermeture de connexion — un flux de modèle coupé passait donc pour une lecture
  réussie, sans `finish_reason`, et le texte partiel se lisait comme une réponse
  complète. Ce cas est désormais nommé `flux_coupe`.
- **Vue** : toute coupure rouvre le flux avec le texte déjà affiché (quatre
  reprises au plus, le service redémarrant en ~2 s), et la reprise SE DIT. Au-delà,
  la cause est vérifiée auprès du serveur et un bouton « reprendre la réponse »
  relance à la main. La bannière d'inachevé a disparu du fichier.
- **Le raccord est déterministe** : le modèle tantôt CONTINUE (« …17, 1 » puis
  « 8, 19, 20 »), tantôt RÉGÉNÈRE (« 1, 2, 3, … 20 »). `raccord.ts` retire le
  préfixe déjà affiché (recouvrement ≥ 12 caractères) et, si la suite repart du
  début, la réponse entière REMPLACE le texte affiché — et la vue le dit.

### Un `tool_call` écrit dans le texte n'est jamais la réponse

- **Défaut mesuré** : après huit appels d'outils, un tour a produit ZÉRO appel dans
  le canal `tool_calls` et, à la place, le markup du gabarit écrit dans son
  raisonnement. Le moteur n'y voyait que du texte : il partait à l'écran comme
  « raisonnement », aucun outil n'était appelé, et la fin annoncée était `stop`.
- **Correctif** : `engine::ExtracteurAppels` sépare prose et markup morceau par
  morceau (le flux coupe les balises, y compris en leur milieu) et
  `outils::appels_du_texte` récupère les appels — deux syntaxes, fermeture
  manquante tolérée, jamais de prose prise pour un appel. Les appels récupérés sont
  EXÉCUTÉS, et si le cas tombe sur le tour de conclusion, un tour leur est accordé
  pour conclure avec le résultat.

### Deux défauts trouvés en mesurant

- `labels(n) CONTAINS 'Class'` rendait **0 ligne en 147 ms** alors que 15 004
  nœuds portent ce label : `labels(n)` est une LISTE, `as_str()` rendait `None`,
  et la ligne était silencieusement rejetée. `CONTAINS` évalue désormais
  l'appartenance sur une liste.
- `obrain_hubs` sans `monde`, dans un chat ancré au graphe global, échouait en
  « le monde visé n'a pas pu être résolu » alors que le store lu était connu.

### Latences d'outils — état mesuré, et le piège du build de trigrammes

Mesures réelles (store `runtime-dev`, 408 832 nœuds, clés de cache neuves à chaque
mesure, service redémarré) :

| requête | avant ce chantier | après |
|---|---|---|
| égalité `qualified_name` | 0,08 ms | **0,0 ms** ✅ |
| count par label (compteur O(1)) | 0,04 ms | **0,0 ms** ✅ |
| scan de label | 0,5 ms | **0,4 ms** ✅ |
| `CONTAINS` | 13 ms | **6,7–23,6 ms** ✅ |
| préfixe (graphe entier / dans un label) | **344 / 3 027–16 315 ms** | **7–60 ms**, pointe 8 007 ms au rattrapage |
| préfixe + label en prédicat | **1 442 ms** (et 0 ligne, faux négatif) | **0–190 ms**, 15 004 lignes rendues |
| agrégation `split`+count | 109–148 ms | **60–155 ms** |

Trois causes trouvées et corrigées : (1) la réécriture de préfixe **construisait**
l'index qu'elle interrogeait (un balayage complet, 3 à 16 s, une fois par question) ;
(2) la clé de l'index ordonné était **tronquée à 11 octets** — inutilisable pour un
préfixe de chemin, et deux lectures de production figeaient la largeur d'entrée à
16 octets (elles lisaient une entrée sur trois) ; (3) sur un store VIVANT, la garde
reconstruisait la série **à chaque question** (mesuré 2 277 ms) — borné depuis à
une reconstruction par propriété et par 30 s.

**Le défaut dominant qui reste** : la construction de l'index de **trigrammes** est
adaptative (`OBRAIN_TRIGRAMMES_DES_LA`, défaut 2) et se déclenche donc à la
DEUXIÈME requête sur une propriété. Sur un serveur qui vit, elle tombe **en plein
milieu d'une question** : mesuré **8 007 ms** pour un préfixe. Mesure comparative,
instance isolée sur le même store, `OBRAIN_TRIGRAMMES=0` :

```
préfixe 1/2/3/4 : 25,8 · 7,8 · 7,0 · 12,8 ms   (aucun blocage)
CONTAINS        : 151,1 ms                     (le balayage reprend)
```

Autrement dit : le build de trigrammes coûte 8 s pour ne RIEN rendre sur un préfixe
de chemin (tous ses grammes sont communs, l'index se tait), alors qu'il sert à
`CONTAINS`. La sortie propre n'est donc pas de l'éteindre mais de **le construire
hors du chemin de la question** (au démarrage, ou par une passe d'entretien) —
c'est la cible du prochain round, avec les ~37 ms résiduels et l'agrégation à 60 ms.

### L'agrégation à forte cardinalité — mesurée sur un store QUIESCÉ

La machine est partagée (charge 22 à 33 sur 16 cœurs) et le store `runtime-dev` est
écrit en continu : la MÊME requête y rend 60, 250, 745 ou 6 804 ms selon le moment.
Une mesure unique n'y prouve donc rien — c'est ce bruit qui a fait garder puis
jeter une micro-optimisation des clés de groupe. Deux outils pour cela :
`scripts/requetes-latence-check.mjs --repetitions 5` (médiane + étendue), et un
store **quiescé** servi par une instance isolée.

Sur `/tmp/linux-full3-v2.obrain` (~1,5 M de nœuds, non écrit depuis 14:28), min de
4 essais, même binaire :

| requête | mesuré |
|---|---|
| `MATCH (n) RETURN count(n)` (compteur) | **0 ms** |
| groupe par `labels(n)` (13 groupes) | **938 ms** — coût de projection |
| groupe par `id(n)` (clé entière) | **1 401 ms** |
| groupe par `n.qualified_name` (clé **chaîne**) | **2 527 ms** |

**La clé chaîne coûte ~1,1 s de plus que la clé entière** sur 1,5 M de lignes
(~0,75 µs/ligne) : c'est la matérialisation d'une `ArcStr` par ligne. Le remède
est identifié et déjà à disposition : `Column::get_string(index) -> Option<&str>`
(`execution/vector.rs:278`) rend la chaîne EMPRUNTÉE, donc on peut sonder la table
de groupes sans allouer, et n'allouer que pour un groupe NOUVEAU.

CE QUE ÇA EXIGE, ET POURQUOI CE N'EST PAS FAIT ICI : une vue empruntée de la clé
(`GroupKeyRef<'_>`) avec `indexmap::Equivalent<GroupKey>` et un `Hash` STRICTEMENT
cohérent entre les deux représentations — une inconsistance ne casse pas la
compilation, elle crée des groupes en double, c'est-à-dire un résultat FAUX. Il
faut donc un test qui vérifie l'égalité de hachage des deux formes avant de
toucher à l'opérateur, et un A/B (médianes, store quiescé) pour prouver le gain.

### Trois optimisations REFUSÉES par la mesure (à ne pas retenter)

Ce qui suit n'est pas du travail perdu : c'est ce qu'il ne faut PAS refaire. Chaque
essai a été mesuré par A/B **en alternance**, deux binaires servis en parallèle sur
le MÊME store, au même moment — la seule méthode qui tienne sur une machine
partagée (charge 22 à 33 sur 16 cœurs, store écrit en continu par le harness).

| essai | hypothèse | A/B mesuré | verdict |
|---|---|---|---|
| clés de groupe en `SmallVec` | l'allocation par ligne coûte | sans gain (mesures alors non fiables) | retiré |
| vue EMPRUNTÉE de la clé de groupe (`get_string`, `Equivalent`, sondage sans allocation) | la matérialisation d'une `ArcStr` par ligne coûte 1,1 s sur 1,5 M de lignes | médiane **114 → 116 ms** sur la requête réelle ; **2 362 → 2 580 ms** sur le store quiescé | **retiré** |
| chemin d'index pour un préfixe SOUS AGRÉGAT (drapeau `consomme_tout` posé par `plan_aggregate`) | sous un `GROUP BY` toutes les lignes sont lues, donc rassembler les candidats ne gaspille rien | médiane **202 → 251 ms (−24 %)** | **retiré** |

Les deux dernières conclusions sont contre-intuitives et valent d'être gardées :

1. **Grouper par une clé quasi unique** (autant de groupes que de lignes) rend le
   coût INHÉRENT : ce sont les 1,5 M de clés qu'il faut stocker, pas un gaspillage
   par ligne. Aucun sondage sans allocation n'y change rien.
2. **Le balayage séquentiel avec lectures par LOT bat l'index** sur un préfixe de
   chemin, même sous agrégat : lire 408 832 chaînes en lot coûte moins cher que de
   poursuivre 30 824 candidats dispersés dans les pages, puis de les revérifier.
   L'index ne paie que sur une clé SÉLECTIVE (l'auteur mesurait ×250 sur `code`
   court) — pas sur les préfixes de chemin qui sortent des LLM.

Il reste donc À FAIRE, et sans ambiguïté : **paralléliser** le balayage/l'agrégat
(le dépôt a déjà le moteur morsel/rayon), seule voie qui divise un coût par le
nombre de cœurs au lieu de chercher un chemin plus court.

### Vérifications

`scripts/chat-reprise-check.mjs --tuer` (coupe le service pour de vrai et exige
que la réponse CONTINUE), `scripts/chat-reprise-ui-check.mjs` (vraie interface),
`packages/obrain-dashboard/src/graph/raccord.test.mjs` (les deux raccords mesurés)
et `scripts/requetes-latence-check.mjs` (latences, budget 50 ms par défaut).

## 2026-09-11 — Chat : ancrage véridique & densification honnête

*Deux défauts mesurés commandaient ce chantier. Le premier faisait produire des
réponses confiantes sur la mauvaise base ; le second faisait lire 0,5 % d'un
monde en croyant l'avoir décrit.*

### Ancrage — le chat lit le store du SUJET, pas le premier de la liste

- **Défaut reproduit** : avec le sujet `world:this-rs-this--github`, le chat
  annonçait « ancré sur this-rs/this » et lisait **runtime-dev**. La réponse
  citait `dsh-runtime`, `session-c254ac35…:msg:7`, `:step:1:1`,
  `file:/tmp/codebase-memory-mcp/README.md` — exactement les 24 premiers nœuds de
  runtime-dev — alors que `this-rs/this` contient 1 500 `Function` et 1 175 `Class`.
- **Deux causes cumulées, toutes deux côté vue** : `focus.world` porte un
  **libellé** (« Runtime dev (engrams) ») et était comparé au `qn` des mondes
  (`world:runtime-dev-engrams`) — jamais égal ; puis le repli `worlds[0].path`
  câblait la conversation sur le **premier monde**.
- **Deux règles de slug divergentes** ont été trouvées dans le même fichier :
  `viz_weave::slug` (écrase les tirets consécutifs) et une règle en ligne dans
  `api_viz_weave` (ne les écrase pas). Pour « this-rs/this (GitHub) » :
  `this-rs-this-github` contre `this-rs-this--github` — **deux qn pour un seul
  monde**. Une règle unique fait désormais foi, et la résolution compare des clés
  canoniques : un monde qui existe se résout, quelle que soit la forme du qn.
- **Nouveau module** `crates/obrain-cli/src/ancrage.rs` : la résolution vit dans
  le serveur et s'appuie sur la **base synaptique** (`World.path`), l'autorité du
  Cosmos, avec repli par slug pour un monde ajouté à chaud (donc pas encore
  tissé). Un monde inconnu **ne retombe sur aucun autre** : l'absence d'ancrage
  est une information, un ancrage inventé est un mensonge.
- Ce qui a été ouvert est **renvoyé à l'écran** (`ancrage` dans l'événement
  `contexte`) et affiché : « ancré sur this-rs/this · **lu dans
  this-rs-this.obrain** ». L'interface ne peut plus nommer autre chose que ce qui
  a été lu.

### Densification — un échantillon représentatif, et la part lue annoncée

- **Défaut mesuré** : `MATCH (n) RETURN n.qualified_name LIMIT 24`, **sans
  `ORDER BY`** — donc l'ordre de stockage. Sur `this-rs/this`, cela donnait
  **22 titres de `.github/ISSUE_TEMPLATE/`** et **aucune** fonction. Sur
  runtime-dev, **21 des 24 nœuds appartenaient à une seule session**.
- **La densification retenue est contrainte par la mesure**, pas par l'intuition :

  | axe | coût mesuré | verdict |
  |---|---|---|
  | compter nœuds/arêtes | 0,04 ms | toujours |
  | échantillon par label (`MATCH (n:Function) … LIMIT n`) | 0,3 – 12 ms | ✅ retenu |
  | arêtes réelles par type (`MATCH (a)-[r:CALLS]->(b) …`) | 5 – 70 ms | ✅ retenu |
  | histogramme des labels | 160 ms (372 k) · 675 ms (1,5 M) | ✅ mémorisé |
  | histogramme des types d'arêtes | 2,3 s (372 k) · 3,5 s (1,5 M) | ✅ mémorisé |
  | **hubs par degré** (`MATCH (n)--(m) … ORDER BY count(m) DESC`) | **14,1 s (372 k) · 14,3 s (1,5 M)** | ❌ **exclu** |

  Conséquence assumée et écrite dans le contexte : on ne prétend pas montrer le
  « cœur » du graphe, on montre un échantillon **représentatif par type** et des
  **arêtes réelles**, avec le pourcentage lu. Un plafond silencieux fait croire
  que la donnée n'existe pas.
- **Carte du store mémorisée** (`engcarte`), validée par
  `viz_serve::store_mtime` — c'est-à-dire par le **manifeste** du store
  (`substrate.meta`), qui change quand les DONNÉES changent, pas quand le journal
  d'activation écrit une ligne. **Une seule règle de fraîcheur dans tout le
  serveur.**

  La première version de cette carte la validait sur le *maximum des mtime des
  fichiers* : cela inclut le WAL et le journal d'activation, donc la carte du
  store **vivant** (`runtime-dev`, écrit en permanence) n'était jamais
  réutilisée. Deux règles pour une même chose, c'est un défaut — et ici c'était
  une régression de performance déguisée en prudence. Mesuré après alignement :
  `runtime-dev` passe de **10 899 ms** à **468 ms** à froid et **1 à 13 ms** à
  chaud.
- **Le sujet est vérifié avant d'être cherché** : un sujet ABSENT faisait payer
  **14 040 ms** (parcours complet à la recherche d'un suffixe inexistant). Les
  index d'égalité coûtent 0,6 ms — et `n.name` n'est **pas** indexé (mesuré
  4 981 ms à vide), donc il n'est tenté que sur un store assez petit, et la
  limite est **dite** dans le contexte.
- **Décomposition du temps** exposée (`decompose`), parce qu'une latence
  inexpliquée se répare mal.

### Mesures (serveur `obrain viz serve`, 12 mondes)

| cas | avant | après (à froid) | après (à chaud) |
|---|---|---|---|
| **monde `this-rs/this`** (4 685 nœuds) | **3 141 ms — et le mauvais store** | 253 ms | **0–1 ms** |
| monde tissé `linux-full3` (1,5 M) | — | 5 040 ms | mémorisé |
| **graphe global** (385 k nœuds) | ~1 855 ms | **468 ms** | **1 à 13 ms** |
| **sujet absent du store** | **16 037 ms** | 14 ms | **2 ms** |
| sujet présent, voisinage 1 saut | ~3 400 ms | 1 091 ms | — |

Le store vivant (`runtime-dev`) est **écrit en permanence** par le harness : lire
sa fraîcheur sur le manifeste — et non sur ses fichiers — est ce qui permet à sa
carte d'être réellement réutilisée. Sans cela, le chat repayait son contexte à
chaque tour.

### Vérification dans le navigateur réel

- `scripts/chat-ancrage-check.mjs` (nouveau) fait le **geste de l'utilisateur** :
  Cosmos → parcours des mondes **au clavier** (←/→) jusqu'à `this-rs/this` — un
  monde ajouté à chaud, donc absent de la base synaptique, précisément le cas qui
  échouait — puis ouverture du chat par le bouton flottant, saisie, envoi, et
  lecture de l'ancrage affiché. Un contrôle par appel d'API n'aurait pas vu le
  défaut : sa cause était dans la vue.
- Résultat (3 passes) : en-tête « ancré sur this-rs/this · lu dans
  this-rs-this.obrain », contexte « 32 / 4685 nœuds (0.7 %) · 10 relations ·
  lu en 0 ms », self `self:UTILISATEUR`, **0 erreur console**. Capture :
  `docs/perf/chat-ancrage.png`.
- La répartition par type est affichée **ouverte** par défaut : « Function — 1500
  dans la base · 4 lus ». C'est ce que veut dire « densification » : montrer ce
  qui n'a PAS été lu.

### Tests unitaires ajoutés

- `ancrage` : 7 tests, dont `les_deux_formes_de_slug_resolvent_le_meme_monde`
  (le cas exact du défaut), `monde_inconnu_ne_retombe_pas_sur_un_autre`,
  `store_absent_du_disque_ne_resout_pas`.
- `engine` : `type_interpolable` refuse tout type d'arête non identifiant (une
  donnée de la base ne doit pas pouvoir écrire du GQL), `part_lue_chiffree`,
  `part_inconnue_si_total_nul`.

### Le chat INTERROGE la base — et le référentiel de l'agent est atteignable

Le contexte était figé au temps t0 : un seul aller-retour, le modèle ne pouvait
pas demander la suite. Six outils lui sont désormais exposés, exécutés
**contre la base** et **visibles à l'écran** (chaque appel est poussé en SSE :
nom, arguments, monde, lignes, durée, erreur).

| outil | ce qu'il donne | coût mesuré |
|---|---|---|
| `obrain_query` | une requête GQL bornée (le modèle en écrit lui-même) | 0-2 ms |
| `obrain_echantillon` | n nœuds d'un TYPE | 0-2 ms |
| `obrain_voisinage` | le voisinage réel d'un nœud | — |
| `obrain_mondes` | les 12 mondes + leurs synapses **et les clés partagées** | 0 ms |
| `obrain_recettes` / `obrain_recette` | le référentiel de l'agent (29 recettes `proc:ref*`) | 1 ms |

**Garde-fous, tous nés d'une mesure** : lecture seule refusée *avant* envoi
(`CREATE`, `MERGE`, `DETACH`… détectés par mot entier) ; `LIMIT` **imposé** quand
la requête n'en porte pas (un `MATCH (n)--(m) …` coûte 14,1 s) ; résultats bornés
à 2 000 caractères et la coupe est **écrite** dans le résultat ; 4 tours maximum,
et la boucle s'arrête d'elle-même avant de franchir la fenêtre de 8 192 jetons.

**Le référentiel n'est PAS dans le graphe regardé** — mesuré : 29 recettes dans
`self-demo`, **zéro** dans runtime-dev et runtime-prod. Les outils le cherchent
donc dans les mondes du self (`obrain_mondes` fournit la carte), et **disent le
monde réellement lu**.

### Deux défauts corrigés, mesurés

1. **Le type était absent des nœuds cités.** À la question « montre-moi 20
   Method », le modèle a listé des noms RÉELS dont **trois sur six étaient un
   `Function`, un `Field` et une `Variable`** : il attribuait le type *demandé* à
   des nœuds qu'il ne pouvait pas typer. `labels(a)[0]` est maintenant lu **avec
   chaque arête** (coût nul, l'expansion tourne déjà) et écrit entre crochets.
2. **Le modèle INVENTAIT quand il ne savait pas.** À la question « quelles
   recettes d'interrogation existent, et que fait celle du tissage ? », avec
   `tool_choice: auto`, il n'a appelé aucun outil et a répondu que les recettes
   seraient « encapsulées dans `build.main` » et `InvoiceDescriptor` — pure
   invention ; le contexte ne contenait ni « recette », ni « tissage », ni
   « weave ». Un 9 B ne s'impose pas à lui-même la consigne « dis que tu ne sais
   pas » : `exige_un_outil()` la lui **impose** — un mot significatif de la
   question absent du contexte ⇒ `tool_choice: "required"` au premier tour, et
   l'écran l'annonce.

   **Vérification, même question** : le modèle a appelé `obrain_recettes` →
   **29 lignes en 1 ms, lues dans `Self-demo (engrams)`**, et la réponse cite les
   vraies familles (`F1-self`, `F2-code`, `F3-runtime`), les vrais noms
   (`self.boot`, `code.hot`, `runtime.weave`) et les vraies descriptions. Avant :
   inventé. Après : exact.

### Connexion aux autres graphes

Le chat ne connaît plus un seul store : l'**atlas** (12 mondes + 12 fibres, lu
dans la base synaptique) lui donne les mondes, leurs preuves (`entités
partagées`, `sessions vécues`, `flux`, `datées`) et un **échantillon des clés
partagées** — de quoi continuer une interrogation dans l'autre graphe au lieu de
le nommer. La jointure entre deux stores est faite **côté serveur** ; le moteur ne
sait pas joindre deux bases, et rien ne le prétend.

`scripts/chat-ancrage-check.mjs` (geste réel : Cosmos → clavier → chat) confirme
l'ensemble : ancre correcte, part lue annoncée, appels d'outils **visibles**,
**0 erreur console**. 16 tests unitaires couvrent l'ancrage, la densification,
les garde-fous des outils et l'exigence d'outil.

### Ce qui reste ouvert

- **Auto-structuration et SelfState** : la conversation n'écrit toujours rien.
  Le canal d'écriture reste à trancher (`viz_serve` ouvre en lecture seule, le
  writer unique est côté MCP) ; `self_qn` est désormais résolu par la base
  synaptique et affiché, ce qui est le préalable.
- **Unification du self** : `self:UTILISATEUR` (toile, self-demo) contre
  `self:agent` (défaut de l'ancien chemin MCP).

### La conversation STRUCTURE ce qu'elle apprend — et le Self le conserve

Dernier axe de la demande : `demander_stream` n'écrivait **que** le journal
d'activation. Une conversation ne produisait ni note, ni objectif, ni état.

**Pourquoi un journal, et pas une écriture directe** — la contrainte est dans le
substrat (`file.rs`) : une base ouverte en lecture prend un `flock` **partagé**,
une écriture exige l'**exclusif**, et le registre refuse la promotion
(« already open read-only in this process — cannot upgrade to an exclusive
(write) lock »). Le serveur tient ses bases en lecture : il ne peut pas écrire
dedans, et ce n'est pas un défaut à contourner — c'est l'invariant mono-écrivain.

D'où deux temps, **la doctrine exacte du tissage** (`scripts/weave.sh` arrête,
écrit, relance) :

1. **le chat JOURNALISE** (outils `obrain_memoriser` et `obrain_structurer`) —
   fichier append-only, un par Self, aucun verrou, **durable immédiatement** :
   l'intention survit à un arrêt, à un crash, à un redémarrage ;
2. **`scripts/memoire.sh` APPLIQUE** — arrêt propre du service, ouverture
   exclusive, écriture en `MERGE` idempotent, journal **consommé**.

Quatre gestes, chacun sur une entité qui **existe déjà** dans la base du Self
(inventer de nouvelles formes ferait diverger la mémoire du modèle de celle qui
est tissée) : `Note` (+ `HAS_NOTE`), `Goal` (+ `HAS_GOAL`), `Procedure`
(+ `MASTERS`), et `SelfState` **versionné** (+ `HAS_STATE`). Chaque nœud porte sa
**provenance** (monde + store) : une mémoire sans provenance n'est pas
vérifiable.

**Vérifié de bout en bout** : 4 notes écrites par la conversation, appliquées,
reliées au Self par `HAS_NOTE`, avec leur provenance ; journal consommé (0 en
attente) ; **idempotence mesurée** — 2 entrées identiques appliquées donnent
**1 nœud** (`MERGE` sur un `qn` dérivé du texte) ; la seule écriture non
idempotente est le `SelfState`, versionné exprès, et c'est pourquoi le journal est
consommé.

À l'écran, une écriture n'est jamais présentée comme faite : « **mémorisé**
(note) : … · journalisé, **en attente d'application** dans `self:UTILISATEUR` ».
Annoncer une écriture faite alors qu'elle attend serait le mensonge qu'on corrige
partout ailleurs dans ce chat.

### Trois défauts de budget, tous mesurés

Le passage aux outils a révélé trois erreurs de dimensionnement de la fenêtre de
8 192 jetons, chacune invisible jusqu'à ce qu'un cas réel la déclenche :

1. **le coût des définitions d'outils n'était pas réservé** — il repart à CHAQUE
   tour (~1 000 jetons) : « Context size has been exceeded » au tour suivant le
   premier appel d'outil. Les descriptions ont aussi été resserrées ;
2. **le ratio d'estimation était trop optimiste** — 3,2 caractères/jeton
   sous-estime le français accentué et le GQL ponctué. Ramené à **2,6**,
   volontairement pessimiste : trop réserver coûte un peu de contexte, trop peu
   coûte la conversation ;
3. **le contrôle de fenêtre retirait les OUTILS au lieu de rogner la SORTIE** — et
   il se déclenchait dès le premier tour avec le `max_tokens` par défaut du
   client (4 096). Conséquence mesurée : « retiens ceci dans ta mémoire »
   n'appelait aucun outil dans le navigateur, alors qu'un test en ligne de
   commande à `max_tokens=700` passait — **même code, deux tailles de sortie**.
   La sortie est un confort, les outils sont une capacité : on rogne la première.

Un quatrième garde a été **retiré** : il coupait la boucle après un tour d'outils
dès que la fenêtre devenait serrante, donc la conversation s'arrêtait sur
l'écriture **sans réponse**. Le contrôle de budget en tête de tour suffit et fait
mieux.

### Ce qui reste ouvert

- **Unification du self** : l'écriture est ancrée sur `self:UTILISATEUR` (résolu
  par la base synaptique), mais le défaut de l'ancien chemin MCP reste
  `self:agent`. Un seul Self pour un seul agent reste à faire.
- **Traversée automatique d'une synapse** : `obrain_mondes` fournit déjà les clés
  partagées ; reste à en faire un outil qui enchaîne la requête dans l'autre
  graphe sans recopier les clés.

### La fenêtre du modèle est DÉCOUVERTE, plus supposée

**Le constat** : tous les budgets du chat — taille du contexte, nombre de nœuds
cités, plafond des résultats d'outils, nombre de tours d'exploration — étaient
des **constantes calibrées pour 8 192 jetons**. Or le moteur avait changé pour un
modèle distant qui en sert **32 768**. Le chat tronquait donc sa matière d'un
facteur QUATRE, en silence, et rien ne le disait.

**Les specs, mesurées** (et non supposées) :

| question | réponse mesurée |
|---|---|
| modèle servi | `Qwen3.8-27B-Q4_K_M.gguf`, sur `IP-INTERNE:9910` (hôte distant) |
| fenêtre annoncée | `/props` → `default_generation_settings.n_ctx = 32768` · `/slots` → `n_ctx: 32768` |
| fenêtre RÉELLEMENT acceptée | **accepté** à 25 258 jetons · **refusé** à 46 205 (« request (46205 tokens) exceeds the available context size ») |
| capacité du modèle | **non lisible d'ici** : `/v1/models` expose des métadonnées vides, aucun accès SSH (publickey/password refusés), le GGUF n'est pas local |
| `--ctx-size` du service local (9 B) | 8 192 — c'est un **paramètre de lancement**, pas une limite du modèle |

Conséquence : la fenêtre servie est un réglage du **service**, pas une propriété
du logiciel ni du modèle. Pour servir 256 k, il faut relancer le `llama-server`
distant avec un `--ctx-size` plus grand ; le chat s'y adaptera **tout seul**,
puisqu'il lit désormais la valeur annoncée.

**Ce qui change dans le code** — les budgets sont dérivés de la fenêtre :

| | 8 192 (avant) | 32 768 (découvert) |
|---|---|---|
| nœuds cités | 60 | 200 |
| caractères de contexte | 12 000 | 34 078 |
| labels échantillonnés × nœuds | 8 × 4 | 14 × 8 |
| types d'arêtes × arêtes | 4 × 3 | 6 × 5 |
| tours d'outils | 4 | 6 |
| plafond d'un résultat d'outil | 2 000 car. | 8 000 car. |
| lignes par appel d'outil | 22 | 60 |

- `fenetre_jetons()` : `OBRAIN_LLM_CTX` si posé, sinon **`/props` du service**
  (mémorisé), sinon 8 192 — et **jamais en dessous** : une fenêtre surestimée
  ferait refuser la requête, une fenêtre sous-estimée ne coûte que de la place.
- La fenêtre est **affichée** dans le bandeau du contexte, **avec sa provenance**
  (« annoncée par le service … » / « défaut du logiciel » / « env ») : un plafond
  invisible se lit comme une limite du logiciel alors qu'il vient du service.
- **Défaut que j'ai introduit et corrigé** : la carte du store était mémorisée sur
  le seul mtime du store. Changer de modèle aurait donc réutilisé une carte
  MAIGRE — la densification gagnée ne serait jamais apparue, sans aucun signe. La
  fenêtre fait désormais partie de la clé de cache.

### Vérification

Trois tests unitaires nouveaux, sur des fonctions **pures** (donc sans réseau ni
variable d'environnement, les tests s'exécutant en parallèle dans un même
processus) : valeurs exactes à 8 192 et 32 768, **monotonie** sur sept fenêtres
(une fenêtre plus large ne donne jamais un budget plus petit), et bornes hautes
(200 nœuds, 8 000 caractères, 60 lignes au maximum). 25 tests passent.

`scripts/vizserve-isole.sh` (nouveau) : une seconde instance sur le port 8433
pour vérifier sans disputer le service de l'utilisateur — deux agents relancent
`scripts/vizserve.sh` en parallèle, l'un échoue en « Address already in use » et
c'est l'ANCIEN binaire qui répond. Constaté deux fois : des mesures « après
correctif » qui étaient en fait « avant ».

### Vérifié en direct, contre les deux services

La découverte est mesurée sur les DEUX moteurs, avec le même code :

| service | fenêtre découverte | provenance affichée | effet sur le contexte |
|---|---|---|---|
| 9 B local (`127.0.0.1:9901`, lancé `--ctx-size 8192`) | **8 192** | « défaut du logiciel (le service … n'a rien annoncé) » — ce service n'expose pas `n_ctx` | 60 nœuds au plafond |
| 27 B distant (`IP-INTERNE:9910`) | **32 768** | « annoncée par le service http://IP-INTERNE:9910 » | **74 nœuds · 21 arêtes · 13 types** contre 32 · 10 · 8 avant, **en 78 ms** |

Le premier cas est instructif : `llama-b9222` n'annonce pas `n_ctx` dans
`/props` — la découverte retombe alors sur 8 192, ce qui est **exactement la
valeur de son `--ctx-size`**. Elle ne se trompe donc pas, elle avoue son repli.

### Un défaut du substrat, rencontré en chemin (et contourné, pas corrigé)

Pour obtenir cette mesure il a fallu écarter deux blocages, tous deux dus à
**l'invariace mono-écrivain mal vécue par les lecteurs** : quand un autre
processus tient une base en écriture, une ouverture en lecture peut **BLOQUER**
au lieu d'échouer. Relevé des threads du serveur pendant une requête : **cinq
threads figés sur un `futex`**, pendant que `/api/query` répondait en 0,9 ms. Le
chat, `/api/viz/weave` et `/api/engine/memoire` étaient morts ; `/api/query`
vivait. La différence était les bases qu'ils ouvrent.

Contournement dans mon périmètre :

- `viz_serve::cached_ro_si_present` — rend le handle **s'il est déjà ouvert**,
  sans jamais en ouvrir un ;
- `ancrage` (self, résolution de monde, atlas) n'ouvre plus rien sur le chemin
  d'une requête : il se rabat sur ses replis, qui sont documentés et testés ;
- `prechauffer_synapse` — la base **synaptique** était la seule à ne pas être
  préchauffée : elle était donc ouverte paresseusement, à la première requête qui
  en a besoin. Elle l'est désormais en tâche de fond comme les mondes.

Le défaut de fond reste dans le **substrat** (une ouverture qui bloque au lieu
de rendre « déjà ouvert en écriture par un autre processus »). Je ne l'ai pas
corrigé : le fichier est partagé avec l'autre agent et le risque de le modifier
sans pouvoir vérifier est plus grand que le bénéfice.
### La traversée d'une synapse, enfin automatique (axe 4 achevé)

Jusqu'ici le modèle apprenait qu'un lien existait (`obrain_mondes` lui donnait les
preuves et un échantillon des clés partagées) puis devait **recopier la clé
lui-même**. Nouvel outil `obrain_traverser(cle)` : il dit dans QUELS mondes la clé
vit et **ce qu'elle Y EST**.

Source de vérité : les `Anchor` de la base synaptique et leurs `IN_WORLD` — la
vérité complète du tissage — avec repli sur les échantillons de clés portés par
les fibres, et la source utilisée est **écrite dans le résultat**.

Mesuré : `skill:codebase-memory-first` → **3 mondes en 18 ms**, source « base
synaptique, via `Anchor → IN_WORLD` », et pour chaque monde le nœud réel
(`[Skill] codebase-memory-first`) et ses arêtes sortantes. Le modèle en a fait un
tableau juste : la même clé est une `Skill` dans les trois.

### Le self du chat est unifié

`/api/engine/memoire` et le bandeau affichent `self:UTILISATEUR` — **résolu par
la base synaptique**, pas par le défaut `self:agent` de l'ancien chemin MCP. La
mémoire du chat s'ancre donc sur le même Self que celui du tissage. Vérifié en
direct.

### Deux défauts que j'ai introduits, et que la mesure a fait sortir

1. **Une carte de handles en double.** Pour éviter une ouverture bloquante,
   `cached_ro_si_present` déclarait sa PROPRE carte statique, distincte de celle
   de `cached_ro` : elle était donc toujours vide, et le chat répondait « base non
   ouverte » alors que la base ÉTAIT ouverte (vérifié : présente dans les
   descripteurs du processus). C'est le même défaut que les deux règles de slug —
   deux registres pour une même chose — sauf qu'ici il rendait le chat muet.
   Corrigé : **une seule carte**, partagée, avec repli sur le chemin canonique.

2. **Une prudence qui cassait un service qui marchait.** En exigeant un handle
   déjà présent, le chat refusait de lire au lieu d'ouvrir. Le raisonnement était
   faux : le risque de blocage est réel, mais un refus systématique est PIRE qu'un
   blocage rare. Revenu au contrat « peek d'abord, ouverture ensuite », qui est
   celui qui fonctionnait — et qui lit désormais **112 nœuds · 25 arêtes** sur
   `runtime-dev` (401 216 nœuds) en 928 ms, contre 32 · 10 avant le chantier de
   densification.

### Ce que je n'ai pas pu faire, et pourquoi

### CORRECTION — la passe d'entretien n'exclut PAS les lecteurs

J'avais écrit ici que `--enrich-ricci` « ouvre les bases en écriture » et empêchait
les lecteurs de les ouvrir, et j'avais mesuré avec `--enrich-ricci 0` à cause de
ça. **C'était faux.** La passe prend **le même handle `cached_ro`** que les
lecteurs (elle écrit *via* lui) : elle ne les exclut jamais.

Ce qui bloquait réellement, c'était **mon propre défaut** : une seconde carte de
handles, vide, dans `cached_ro_si_present` — plus, à un moment, un binaire
périmé. Vérifié depuis, entretien ACTIVÉ : `8/8 monde(s) enrichi(s)` et le chat
lit `112 nœuds · 25 arêtes` en **127 ms**, self `self:UTILISATEUR`. Les deux
coexistent.

La leçon vaut d'être écrite : j'ai attribué à l'infrastructure un symptôme qui
venait de mon code, et j'ai « prudemment » désactivé une fonctionnalité sur cette
base. Un diagnostic qu'on ne vérifie pas en désactivant le suspect coûte deux
fois — une fois en erreur, une fois en capacité perdue.
### L'ENTRETIEN EST LE RÉGIME, pas une option (pleine cognition)

Consigne de l'utilisateur : « tous les enrichissements doivent être de base, on
vise une pleine cognition ». Une base servie sans ses signaux est un graphe brut.

**Ce qui n'allait pas** : la boucle d'entretien faisait
`let Ok(db) = cached_ro(&path) else { continue }` — un `continue` **silencieux**.
Un monde qui ne s'ouvrait pas n'était **jamais** enrichi, et rien ne le disait :
la base restait brute indéfiniment en donnant l'impression que l'entretien
passait. C'est l'inverse d'un enrichissement de base.

**Ce qui change** : l'entretien persévère pour ouvrir chaque monde (peek puis
ouverture), **nomme** ce qu'il n'a pas pu enrichir et pourquoi, et **rend un
compte par passe** — une passe qui ne dit pas ce qu'elle a fait ne se distingue
pas d'une passe qui n'a rien fait. Il vérifie aussi que les registres de **degrés
par type** se matérialisent (`substrate.degrees.node.<TYPE>.u32`) en plus de la
courbure, et la désactivation devient bruyante :

```
viz serve : entretien Runtime dev (engrams) — courbure 202 arêtes mises à jour (1 241 937 visitées)
viz serve : entretien — 8/8 monde(s) enrichi(s)
```

Mesuré : **8/8 mondes enrichis**, 40 registres de degrés présents sur
`runtime-dev`, et le chat lit pendant la passe (127 ms).

**Ce qui manque encore pour la « pleine cognition »**, et que je n'ai pas fait :

- **Les degrés ne sont pas interrogeables en GQL.** Ils existent en registres par
  type, lus par l'API Rust (`substrate.out_degree`), mais `n.deg_out` reste à 0 :
  aucune surface `CALL obrain.*` ne les expose. Conséquence directe : le chat
  **ne peut pas** classer les hubs par degré (il retombe sur un parcours à
  14,1 s, qu'il refuse). C'est une addition de moteur, hors de mon périmètre.
- **La couche sémantique est absente** : d'après `docs/ETAT-COSMOS-2026-09-10.md`,
  `vector-index` n'est pas compilé et aucune propriété d'embedding n'existe
  (`n.embedding` → NULL partout). Sans elle, pas de recherche sémantique dans le
  chat — c'est le plus gros manque pour une cognition pleine.

### 14,1 s : REFUSÉ. Les hubs se lisent dans les registres — et on refuse de mentir

Consigne : « on ne doit pas accepter des temps de 14,1 s ». Le classement par
degré se faisait par un **parcours complet** :

```text
MATCH (n)--(m) RETURN n.qualified_name, count(m) AS d ORDER BY d DESC LIMIT 12
```

**14,1 s** sur `runtime-dev` (383 k nœuds, 1,2 M arêtes), **14,3 s** sur
`linux-full3`. Ce n'est pas une lenteur à optimiser : c'est un parcours pour une
réponse déjà écrite ailleurs. Le chat ne pouvait donc pas montrer le « cœur »
d'un graphe — il refusait la requête.

**Le remplacement** : les registres de degrés du substrat
(`substrate.degrees.node.<TYPE>.u32`) répondent en **O(1) par nœud**. L'entretien
calcule donc le classement pendant sa passe et le **persiste** ; le chat le
relit. Une seule passe linéaire, jamais sur le chemin d'une question.

| store | hubs produits | temps |
|---|---|---|
| `this-rs/this` (4 685 nœuds) | 40 | **1 ms** |
| `Obrain (rust)` (6 587) | 40 | **1 ms** |
| `rust-lang/log` (698) | 40 | **0 ms** |
| `this-rs/project-orchestrator` (26 633) | 40 | **2 ms** |
| `Self-demo` (52) | 40 | **0 ms** |
| `octocat/Spoon-Knife` (36) | 6 | **0 ms** |

Au passage, une correction de fond : la liste contenait **quatre fois le même
`qualified_name`** (la base admet des noms non uniques) — quatre copies d'une
identité évincaient trois vrais hubs. Un seul hub par `qn` désormais.

### Et surtout : les registres de `runtime-dev` sont FAUX — on refuse de classer

Premier essai sur `runtime-dev` : le registre annonçait **48 253 968** arêtes
sortantes pour un nœud. Vérification par comptage réel : **28 368**. Le graphe en
compte **1 243 111** au total. **Incohérent d'un facteur 1 700.**

Un classement tiré de là n'aurait pas été lent : il aurait été **faux**, et il
aurait envoyé le modèle et l'utilisateur vers les mauvais nœuds — le contraire
de ce qu'on cherche. Un contrôle évident et gratuit (un degré ne peut pas dépasser
le nombre d'arêtes) attrape ces registres, et le classement est alors **REFUSÉ**,
avec sa raison :

```
Runtime dev (engrams) — classement REFUSÉ : les registres de degrés de ce store
sont INCOHÉRENTS : 280 nœud(s) annoncent un degré supérieur au nombre d'arêtes du
graphe (pire cas 48 253 968 pour 1 243 111 arêtes au total).
```

Mesuré sur les huit mondes servis : **6 classements produits** (0-2 ms),
**2 refusés** — `runtime-dev` (280 nœuds impossibles) et `runtime-prod` (8). Le
refus est **persisté et transmis** : le chat, l'outil et l'interface le disent au
lieu d'afficher une liste vide qui se lirait comme « aucun nœud connecté ».

Réparation possible : `obrain repair-degrees <store>` — qui exige une ouverture
**exclusive**, donc le service arrêté pour ce store. C'est un geste d'exploitation,
pas une correction de code.

### Vérifié

- 29 tests unitaires (4 nouveaux sur les hubs : total, lisibilité, plafond,
  **péremption** — un classement calculé sur une autre version du store n'est pas
  servi) ;
- en direct : `this-rs/this` → 8 hubs affichés en 1 ms, noms et degrés réels ;
  `runtime-dev` → le refus est transmis dans l'événement de contexte.

### Le substrat ne sert plus une colonne de degrés FAUSSE (correctif de fond)

Le refus du classement sur `runtime-dev` n'était pas un symptôme à contourner
côté chat : il venait du substrat. **`DegreeColumn::open` valide l'INTÉGRITÉ du
fichier** — magic, version, taille, CRC — **jamais sa COHÉRENCE avec le graphe**.
Une colonne écrite avant une coupure, relue après une réparation de compteurs, est
parfaitement intacte et parfaitement fausse : elle était donc servie telle quelle,
sans un mot.

Les colonnes **typées** avaient déjà leur contrôle (`ensure_typed_degree_health`,
2026-09-05). La colonne **générique** — celle que lit `out_degree`, donc celle
dont dépendent les classements de hubs — n'en avait aucun. C'est ce trou qui est
fermé.

**Ce qui a été ajouté** (`crates/obrain-substrate/src/store.rs`) :

- `incoherence_colonne_degres` — le contrôle, avec le MÊME invariant que les
  colonnes typées : la somme des degrés vaut le nombre d'arêtes vivantes. Il
  distingue l'**impossible** (un nœud ne peut pas avoir plus d'arêtes que le
  graphe n'en contient — le cas mesuré : 48 253 968 contre 1 243 111) de
  l'**inexact** (dérive infime tolérée et journalisée, pour ne pas payer un scan
  à chaque ouverture) ;
- `verifier_coherence_degres_une_fois` — vérification **une fois par store et par
  processus** (~1 ms pour 400 k nœuds), au premier accès réel ;
- `reparer_colonne_degres_en_place` — remise à zéro puis **un seul** rescan des
  arêtes, dans le fichier existant.

**Trois pièges rencontrés en l'écrivant**, tous mesurés :

1. **Le contrôle placé dans le `get_or_init` de `degrees()` ne servait à rien** :
   à la réouverture, il tourne avant que les compteurs d'arêtes soient établis
   (`total_live_edges == 0`), la cellule se verrouille, et la colonne corrompue
   est servie pour toute la session. Le drapeau n'est donc consommé que
   **lorsqu'on peut juger** ;
2. **La reconstruction existante ne pouvait pas être réutilisée** :
   `rebuild_degrees_from_scan` **crée un fichier neuf**, or la zone de degrés est
   déjà ouverte à ce moment-là — la réparation échouait silencieusement et la
   colonne fausse restait servie. D'où la réparation **en place** ;
3. **En lecture seule, on ne répare pas** — la réparation écrit, et une base
   ouverte en lecture ne doit pas être modifiée (même discipline que
   l'auto-guérison des colonnes typées, qui désactive son pré-filtre dans ce
   mode). On le journalise avec la commande de réparation.

**Conséquence pour le dashboard** : le service ouvre les bases en **lecture
seule**, donc il ne répare pas — il continue de dire la vérité (classement refusé,
avec la raison et la commande). La réparation s'applique aux ouvertures
**inscriptibles** (CLI, pont MCP, passes d'enrichissement), et reste disponible à
la demande : `obrain repair-degrees <store>` (service arrêté pour ce store).

**Vérifié** : trois tests dans `obrain-substrate` — la détection d'une colonne
impossible, sa **réparation à la réouverture** (corruption, flush, fermeture,
réouverture, degrés justes), et le fait qu'une colonne **juste n'est PAS**
reconstruite (sinon on paierait un scan à chaque ouverture). 841 tests du
substrat passent ; **un échec ne vient pas de ce correctif** —
`des_ancres_divergentes_ne_rendent_jamais_un_resultat_ampute` porte sur l'index
ordonné, ne touche ni `out_degree` ni `degrees`, et son commentaire est daté du
2026-09-11 (chantier en cours d'un autre agent).

### Mise à niveau de TOUTES les bases — et ce que la vérification a démenti

Consigne : « répare toutes les bases, il faut que tout soit à niveau ».

**L'outil** : `scripts/repair-degrees-all.sh` (nouveau) — arrêt **propre** du
serveur, réparation de toutes les bases **sans temps mort**, relance. La séquence
atomique n'est pas un confort : une première passe à l'unité a vu ses 11 bases
reprises entre deux commandes par un serveur relancé par un autre agent, et tout a
échoué en « already open by another process ».

**11 bases réparées en 23 à 204 ms.** Les deux plus grosses (`runtime-dev`,
`runtime-prod`) étaient tenues par des **ponts MCP du harnais** — deux d'entre eux
vivants depuis 1 et 2 jours. Je n'ai rien tué : elles se sont **guéries seules**,
le correctif du substrat détectant et réparant leur colonne via une ouverture
inscriptible. Preuve croisée : le top hub de `runtime-prod` annonce désormais
**28 368** arêtes sortantes — exactement le comptage réel mesuré à la main, là où
le registre disait **48 253 968**.

**Vérification indépendante, par les FICHIERS** (l'en-tête `DEGR` est lisible :
`n_slots` puis des paires u32), et non par ce que les outils rapportent :

| base | Σ degrés | arêtes | verdict |
|---|---|---|---|
| runtime-prod · kv-linux-e1000 · engine_idx · octocat ×2 · rust-lang/log · this-rs/this · this-rs/project-orchestrator | **= arêtes** | — | **8 exactement cohérentes** |
| runtime-dev | 1 243 981 | 1 243 111 | dérive +870 (0,07 %, inoffensive) |
| self-demo | 52 | 47 | dérive +5 |
| **linux-full3** | **0** | 4 779 754 | **colonne inutilisable** |
| kv-linux-driversnet | 0 | **0** | store **VIDE** — la colonne est juste |

**Deux déments, tirés de la mesure :**

1. **`kv-linux-driversnet` n'est pas une base abîmée : elle est VIDE.** 0 nœud,
   0 arête, par compteur **et** par parcours. La toile annonçait 310 547 nœuds et
   769 685 arêtes — des chiffres **enregistrés au tissage** et plus vrais depuis.
   Un monde est donc servi sans données. Ma première passe l'avait compté comme
   « colonne incohérente » : c'était faux, et c'est la vérification par fichier
   qui l'a montré.
2. **`linux-full3` reste cassée, et sa cause est plus profonde.** Sa colonne
   générique fait **4 Ko — un seul emplacement** — pour 1 553 071 nœuds, alors que
   ses colonnes **typées** en font 12 Mo chacune. Elle a été recréée à un moment
   où le compteur de nœuds n'était pas encore chargé. Et `rebuild_degrees_from_scan`
   ne peut pas la refaire : il parcourt la zone d'arêtes **classique**, qui est
   **vide (0 octet)** sur cette base — ses enregistrements vivent dans un autre
   régime (`substrate.blobcol.*`, `substrate.tier0/1/2`), comme le montre un
   `substrate.edges` de 0 octet pour 4,78 M d'arêtes.

**Ce qui change dans le comportement, et c'est le point important** : ces deux
mondes ne produisent plus de classement **en silence**. L'entretien le DIT,
avec la cause :

```
Linux complet (v6.6)   — classement REFUSÉ : aucun degré non nul alors que le
                         graphe compte 4779754 arêtes
Linux drivers-net      — classement REFUSÉ : le graphe ne compte aucune arête :
                         il n'y a aucun nœud connecté à classer (le store est vide
                         — ce n'est pas un défaut de la colonne)
```

Deux garde-fous ajoutés : `hubs::calculer` refuse explicitement quand **aucun
degré n'est lu alors que le graphe a des arêtes** (c'était le mode de panne
silencieux), et distingue le **graphe vide**, qui n'est pas un défaut. Côté
substrat, `incoherence_colonne_degres` vérifie désormais la **taille** de la
colonne (au moins un emplacement par nœud vivant) : une colonne plus petite que
le graphe lit des zéros au-delà du premier emplacement.

**État final mesuré** : **10 mondes sur 12** produisent leur classement en
**0 à 13 ms** (40 hubs chacun, sauf les deux micro-dépôts GitHub : 2 et 6).
30 tests CLI, 3 tests substrat.

**Ce qui reste ouvert, précisément** : `rebuild_degrees_from_scan` doit
énumérer les arêtes **à travers les régimes de stockage** (zone classique, blob
columns, zones étagées) au lieu de ne lire que la zone classique ; et
`repair-degrees` doit **flusher** sa recréation avant de rendre la main (vérifié :
sur une copie valide, il supprime la colonne et ne la recrée pas). Tant que ce
n'est pas fait, `linux-full3` restera refusée — mais **en le disant**.

### Le vrai coupable : des STORES INCOMPLETS, pas des colonnes abîmées

Suite de la mise à niveau — et troisième dément de mes propres conclusions, tiré
d'une copie **fidèle** de `linux-full3` (1 553 071 nœuds, 4 779 754 arêtes
annoncés) :

```text
MATCH (n) RETURN count(n)          →  1553071     (compteur)
MATCH (n) RETURN n LIMIT 3         →  []          (AUCUN enregistrement lisible)
MATCH ()-[r]->() RETURN count(r)   →  4779754     (compteur)
MATCH (a)-[r]->(b) … LIMIT 3       →  []          (AUCUNE arête lisible)
```

**Ces deux mondes ne sont pas des colonnes de degrés abîmées : ce sont des stores
dont les ZONES DE DONNÉES SONT VIDES (0 octet) alors que les compteurs du
manifeste ont survécu.** La colonne de degrés vide était donc *correcte*, et mes
deux messages précédents accusaient la mauvaise pièce — d'abord « registres
incohérents », puis « colonne inutilisable ».

**Ce qui est corrigé — la chaîne de diagnostic distingue trois états**, chacun
avec son message et sa gravité :

| état | message | gravité |
|---|---|---|
| graphe vide (0 arête) | « le graphe est vide : il n'y a rien à classer » | aucune — ce n'est pas un défaut |
| compteur qui ment | « **AUCUN enregistrement lisible** : `MATCH (n) RETURN n LIMIT 1` ne rend rien alors que le compteur annonce N arêtes. Les zones de nœuds et d'arêtes sont VIDES — le classement est impossible parce qu'il n'y a plus de données à classer, **pas parce que la colonne serait abîmée** » | **store incomplet** |
| colonne inutilisable | « aucun degré non nul alors que des nœuds se lisent — reconstruisez par `repair-degrees` » | colonne |

La sonde qui tranche coûte **O(1)** : une lecture (`MATCH (n) RETURN n LIMIT 1`).

**Et le substrat le dit maintenant à la source.** La vérification des compteurs
ne pouvait pas le voir : elle est **sautée** quand la fenêtre WAL ne porte aucun
record structurel (optimisation P1 du 2026-09-09) — exactement le cas d'un store
vidé hors WAL. Une sonde de **lecture** bornée (8 emplacements) a donc été ajoutée
à l'ouverture : elle ne répare rien — un re-comptage par les zones classiques
rendrait 0 et **détruirait** les compteurs — mais elle le DIT :

```
WARN substrate::counters: STORE INCOMPLET : des nœuds sont annonces mais AUCUN
enregistrement n'est lisible (8 emplacements sondes) — les zones de donnees sont
vides, seuls les compteurs du manifeste ont survecu. Les comptes, les degres et
les classements qui s'appuient dessus sont FAUX.
```

Vérifié en service : la sonde se déclenche sur `linux-full3` (et se tait sur
`kv-linux-driversnet`, dont les compteurs sont à 0 — il n'y a rien à dénoncer).

**État après ce tour** : 10 mondes sur 12 produisent leur classement en 0-13 ms ;
les 2 autres sont diagnostiqués **exactement** — un store vide, un store
incomplet — au lieu d'être accusés à tort ou silencieux. 6 tests `hubs`,
3 tests substrat, 841 tests substrat au total (un échec préexistant sur l'index
ordonné, sans rapport).

**Reste ouvert** : la réparation propre de stores incomplets (ré-indexation) et
l'énumération des arêtes à travers les régimes de stockage (`blobcol`, `tier0/1/2`)
pour que `rebuild_degrees_from_scan` fonctionne sur ces bases — deux chantiers
distincts, tous deux à valider sur copie.

### Balayage COMPLET des endpoints — et deux faux positifs de l'instrument

« Vérifie que tout est bon sur tous les endpoints. » Les instruments existants
en couvraient une partie : `smoke-backend.sh` (agrégats chauds < 50 ms) et
`perf-endpoints.mjs` (coûts de 10 routes). **Dix routes n'étaient mesurées par
aucun des deux** — `/api/viz/subgraph`, `/api/viz/status`, `/api/info`,
`/api/stores`, `/api/engine/ask`, `/api/engine/chat`, `/api/engine/memoire`,
`/api/worlds/status`, `/__dev/version` et les statiques.

Nouvel instrument : **`scripts/endpoints-check.mjs`** — les 26 routes, avec des
paramètres VALIDES (un endpoint qui répond 400 faute de paramètre ne prouve rien),
à froid **puis** à chaud, et un verdict par route : `OK` · `VIDE` · `ERREUR` ·
`ABSENT` · `LENT`. Il dérive aussi les **actifs réellement référencés par la
page** et les demande : vérifier « la page répond » ne dit pas que le bundle se
sert, et une page qui pointe un fichier absent donne une application blanche avec
un 200.

**Résultat, sur les DEUX serveurs** — le dashboard de l'utilisateur (8432) et une
instance du binaire courant (8433) : **26/26 endpoints OK**.

Tout répond en **1 à 9 ms à chaud** (subgraph 1-13 ms, activation 5 ms), sauf les
deux routes qui appellent le modèle : `engine/ask` **39 à 83 s** (le temps du LLM,
pas du serveur) et `engine/chat` en flux, dont le **premier événement** arrive en
**13-15 ms** — c'est le gain du chantier d'ancrage.

**Deux faux positifs de l'instrument, trouvés et corrigés :**

1. **`/api/worlds/status` interrogé avec un identifiant inexistant** était compté
   comme une panne — alors que répondre « travail inconnu » est le comportement
   CORRECT d'une route dont le métier est de valider un paramètre. Le verdict
   accepte désormais une erreur ATTENDUE, explicitement déclarée par route.
2. **`actif js` en erreur sur les deux serveurs à la fois** : mon heuristique
   cherchait la chaîne `"error"` dans le corps — et un bundle JS en contient
   forcément (gestion d'erreurs, clés de messages). L'heuristique ne s'applique
   plus qu'aux réponses **JSON** (`^\s*[\[{]`). Sans ce garde-fou, j'aurais
   annoncé deux pannes inexistantes.

**Un point d'exploitation** : le serveur du dashboard tourne un binaire de
**20:39**, antérieur de 14 minutes à mes deux derniers correctifs (le diagnostic à
trois états des hubs et la sonde d'intégrité du substrat). Les endpoints sont tous
présents et sains, mais **ces correctifs ne sont pas actifs tant que le service
n'est pas relancé**.

### Réparer sans détruire — et la cause de la corruption à 45×

**Le défaut grave, dans l'outil de réparation lui-même.** `repair-degrees`
supprimait les fichiers de colonnes et comptait sur le chemin paresseux pour les
recréer. Or **supprimer un fichier déjà mmapé ne le recrée pas** : l'inode survit
dans le processus, les écritures de la « reconstruction » partent dedans, et le
chemin reste **vide**. Vérifié sur une copie : après la passe, la colonne
générique avait disparu ; une seconde passe ne la recréait pas ; seule une
**nouvelle ouverture** la recréait (et correctement). Un outil qui détruit ce
qu'il prétend réparer — **et qui l'annonce** (« colonnes reconstruites ») — est le
pire des deux mondes.

La reconstruction se fait désormais **en place** : remise à zéro puis un seul
rescan, sur la colonne existante, sans jamais supprimer un fichier. Vérifié de
bout en bout : `n_slots=37 · Σ=30` avant, `n_slots=37 · Σ=30` après, et le
message le dit honnêtement (« aucun fichier supprimé »).

### Puis j'ai cassé quelque chose, et je l'ai réparé

En activant la réparation **automatique** à chaque détection d'incohérence,
`runtime-dev` est passé de **Σ = 1 243 981** (juste à 0,07 %) à **Σ = 56 087 046**
— **45 fois** le nombre réel d'arêtes. J'avais validé ce chemin sur une copie de
252 Ko et je l'ai déployé sur une base de 11 Go : une faute de méthode, pas
seulement un bug.

**La cause est maintenant établie par la mesure** :

- **l'algorithme n'est pas en défaut** : trois passes successives sur une copie
  immobile donnent **exactement** le même résultat (`Σ=30` → `30` → `30`) ;
- **la concurrence l'est** : la reconstruction en place remet à zéro puis
  rescanne, et une arête créée **entre** les deux est comptée deux fois. La
  corruption est apparue sur la seule base **écrite en continu** par le harnais,
  et sur elle seule, pendant que ce chemin tournait automatiquement.

Trois décisions :

1. **Réparation automatique désactivée.** La **détection** reste, et avec elle le
   **refus** : le chat ne publie jamais un classement tiré d'une colonne
   incohérente, et il dit pourquoi. Ce filet n'écrit rien.
2. **`runtime-dev` restauré** : colonne corrompue retirée (sauvegardée), reconstruite
   par le chemin éprouvé → `Σ = 1 247 728`, max `33 155`, classement de nouveau
   servi en **14 ms**.
3. **Tolérance portée de 0,1 % à 2 %** : sur une base vivante, le compteur et le
   scan ne sont pas pris au même instant. À 0,1 %, le plus grand monde se faisait
   refuser pour une dérive **normale** de +0,37 %. La corruption reste attrapée
   (le cas mesuré était de 45×, une colonne vide en est à 100 %).

Et une garde nouvelle dans `repair-degrees` : si la base **a été écrite il y a
moins de cinq minutes**, la commande le dit — chiffre à l'appui — parce que la
reconstruction en place n'est pas atomique face à un écrivain concurrent.
Vérifié : **silencieuse** sur une base au repos (fichiers vieux de deux heures),
**bavarde** sur une base écrite à l'instant.

**État après ce tour** : 10 mondes sur 12 produisent leur classement en 0-14 ms ;
les 2 autres — `Linux complet` (store incomplet) et `Linux drivers-net` (store
vide) — sont refusés avec le **bon** diagnostic. 24/24 endpoints OK, 841 tests
substrat, 31 tests CLI, 6 tests hubs.

### Un journal qu'on ne peut pas lire ne sert à rien (6 039 lignes → 33)

Constat en vérifiant : au démarrage du service, le journal recevait **6 039
avertissements** `T17k: PropsZone v2 decode failed — skipping key` — un par nœud
et par propriété dont le décodage échoue sur une base abîmée. Ils **noyaient tout
le reste**, y compris les lignes d'entretien que je cherchais : ma première
vérification a conclu « journal vide » alors que le service fonctionnait.

Le message est désormais **agrégé sur une échelle logarithmique** : on journalise
les 1er, 2e et 3e cas (avec nœud, clé et erreur — de quoi diagnostiquer), puis
10, 20, 50, 100, 200, 500, 1 000… avec le **total** qui avance. Les paliers serrés
au début sont voulus : deux ou trois exemples disent s'il s'agit d'un défaut
systématique ou d'un cas isolé. Le message voisin (`bulk collect failed`) reçoit
le même traitement.

**Mesuré après relance : 33 lignes au lieu de 6 039.** Et l'entretien devient
enfin visible :

```
entretien Runtime dev (engrams) — courbure 7832 arêtes mises à jour (1 247 797 visitées)
entretien Runtime dev (engrams) — 40 hub(s) en 37 ms (registres)
entretien Runtime prod          — 40 hub(s) en 126 ms (registres)
entretien Linux complet (v6.6)  — classement REFUSÉ : AUCUN enregistrement lisible
entretien Linux drivers-net     — classement REFUSÉ : le graphe est vide
entretien — 12/12 monde(s) enrichi(s)
```

On y voit aussi, en production, la **sonde d'intégrité du substrat** ajoutée plus
tôt : `STORE INCOMPLET : des nœuds sont annonces mais AUCUN enregistrement n'est
lisible` — le défaut qui, sans elle, restait invisible.

**État** : 12/12 mondes enrichis ; 10 produisent leur classement de hubs en 0-126 ms,
2 sont refusés avec le bon diagnostic. 841 tests substrat (1 échec préexistant sur
l'index ordonné, sans rapport), 35 tests CLI, 24/24 endpoints.

### DEUX CARTES POUR UN MÊME REGISTRE — la cause commune de quatre pannes silencieuses

Je cherchais pourquoi la sonde de lisibilité ne se déclenchait jamais. La cause
est unique, et c'est la même famille de défaut que les deux règles de slug :

**`cached_ro` remplissait SA carte statique locale** (avec horodatage d'usage et
éviction LRU), **tandis que la lecture « sans ouvrir » interrogeait une AUTRE
carte**, au niveau du module, que **personne ne remplissait jamais**. La lecture
ne trouvait donc rien — toujours — et **quatre fonctionnalités se rabattaient en
silence sur leur repli** :

| usage | ce qui se passait |
|---|---|
| ancrage du chat | le handle n'était pas trouvé → « base non ouverte » alors que la base **était** ouverte |
| ouvreur de mondes (`db_du_monde`) | chaque outil rouvrait une base au lieu de réutiliser le handle chaud |
| handle synaptique de `obrain_traverser` | la traversée se rabattait sur les échantillons des fibres |
| sonde de lisibilité de l'atlas | jamais exécutée → « volumétrie NON VÉRIFIÉE (store non sondé) » pour **tous** les mondes |

**Corrigé : une seule carte**, au niveau du module, avec l'horodatage d'usage —
`cached_ro` et `cached_ro_si_present` lisent et écrivent la même.

**Vérifié de bout en bout**, en citant le modèle mot pour mot :

```
world:runtime-dev-engrams [self] Runtime dev (engrams) — 372019 nœuds · 1188294 arêtes
  · volumétrie VÉRIFIÉE par lecture
world:self-demo-engrams  [self] Self-demo (engrams)  — 52 nœuds · 47 arêtes
  · volumétrie VÉRIFIÉE par lecture
world:linux-complet-v6-6 [connaissance] Linux complet (v6.6) —
  ⚠ INCOMPLET — le compteur annonce 1553071 nœuds et 4779754 arêtes, mais AUCUN
    enregistrement n'est lisible : ce monde n'a plus de données.
```

Le chat **cesse enfin de présenter un store vide comme un monde plein** — c'était
le défaut signalé, et il tenait à une carte fantôme.

C'est la quatrième fois dans cette session qu'un symptôme attribué à
l'infrastructure venait d'un **registre dédoublé** : les deux règles de slug, les
deux cartes de handles, la carte du store mémorisée sans la fenêtre, et les
compteurs du manifeste face aux zones vides. Le motif est constant — **deux
sources pour une même vérité, et c'est la plus faible qui gagne en silence**.

### L'ENTRETIEN EST DÉLÉGUÉ AU REDÉMARRAGE — il n'a plus à être demandé

Demande de l'utilisateur : « tu peux déléguer ça au processus de redémarrage ? »
C'est la bonne architecture, et pour une raison de fond : **les réparations
exigent une ouverture EXCLUSIVE, donc que plus personne ne tienne les bases. Le
seul moment où c'est vrai sans geste humain, c'est entre l'arrêt et le lancement
du service.**

`scripts/vizserve.sh` arrête, appelle **`scripts/entretien-bases.sh`**, puis
relance. L'entretien fait, **base par base dans une même fenêtre** :

1. **colonnes de degrés** — reconstruction EN PLACE, idempotente, 21 à 192 ms ;
2. **enregistrements corrompus** (`obrain repair`) — **diagnostic** par défaut,
   application seulement si `OBRAIN_REPAIR_APPLY=1` (elle ÉCRIT dans les bases).

Rien n'est obligatoire pour servir : tout échec est rapporté et le service
démarre quand même — un entretien qui empêcherait le démarrage serait pire que
pas d'entretien. `OBRAIN_SANS_ENTRETIEN=1` contourne le tout.

**Deux défauts de ma première version, mesurés et corrigés :**

1. **Les deux passes étaient séparées** (« toutes les colonnes », puis « tous les
   diagnostics »). Entre les deux, un autre serveur reprenait les bases et le
   diagnostic répondait « TENUE » pour 11 bases sur 12 : la fenêtre libre était
   gaspillée. On traite désormais chaque base d'un seul tenant — vérifier qu'elle
   est libre, reconstruire, diagnostiquer, pendant qu'elle l'est encore.
2. **L'arrêt n'attendait qu'une seconde.** L'entretien trouvait encore 8 bases
   « tenues » par le serveur PRÉCÉDENT, en train de mourir — un processus qui
   rend douze bases mmapées met plus d'une seconde. Le redémarrage attend
   maintenant la libération **effective** : port rendu ET plus aucun processus
   (avec le crochet dans le motif `pgrep`, le piège documenté du dépôt).

**Vérifié** : un `bash scripts/vizserve.sh` ordinaire affiche désormais

```
entretien · self-demo.obrain                    enregistrements : saine
entretien · linux-full3.obrain                  enregistrements : saine
entretien · runtime-dev.obrain                  TENUE · …/obrain bridge
entretien · 5 base(s) traitée(s), 8 tenue(s) par un autre processus
viz serve lancé (pid …) — racine: 200
```

**Ce qui reste hors d'atteinte, et pourquoi** : `runtime-dev` et `runtime-prod`
sont tenus par les **ponts MCP du harnais**, en permanence. Tant qu'ils tournent,
leur réparation d'enregistrements est impossible — l'entretien le **dit** à chaque
redémarrage au lieu d'échouer en silence. Les autres bases sont traitées dès
qu'un redémarrage les trouve libres ; un autre agent relançant le même service sur
le même port, la fenêtre est contestée, et c'est assumé : ce qui n'est pas fait
cette fois sera fait à la suivante.

### LES DEMANDES EN VOL SONT ATTACHÉES — plus aucune réparation ne se perd

Demande de l'utilisateur : « tu devrais avoir un protocole qui te permettrait
d'attacher des demandes en vol pendant que la base tourne ». C'était le chaînon
manquant, et il manquait **précisément là où ça compte**.

**Le défaut** : une base qu'on ne pouvait pas réparer (tenue par le service, par
les ponts MCP, par un autre agent) était **abandonnée** à ce passage. Le compte
rendu disait « TENUE » et **l'information mourait là** — au redémarrage suivant,
il fallait que quelqu'un se souvienne qu'elle était à réparer. Personne ne s'en
souvient.

**Le protocole** (`crates/obrain-cli/src/entretien.rs`) est le même que celui de
la mémoire du chat, pour la même raison — le substrat impose l'invariant
mono-écrivain, donc on **journalise l'intention** quand on ne peut pas agir, et on
**l'applique** quand on peut :

1. **en vol** — un producteur constate un défaut et **attache une demande** :
   la base, le genre de réparation, la raison **mesurée**, la source, l'heure.
   Fichier append-only, aucun verrou : rien ne bloque la lecture en cours ;
2. **à la fenêtre** — l'entretien de redémarrage relit la file, répare les bases
   libérées, et **accorde** ce qui a réussi. Ce qui n'a pas pu être fait **reste**,
   avec son **âge**.

**Idempotent** : attacher dix fois la même demande n'en crée qu'une (la dernière
mesure est gardée). Sans cela, un défaut constaté à chaque requête remplirait le
journal — le défaut des 6 039 lignes, transposé. **Un test l'ancre.**

**Premier producteur branché** : le calcul des hubs, quand il refuse pour un motif
**réparable** (colonne de degrés inutilisable). Il constate le défaut en pleine
lecture, ne peut pas le corriger — l'exclusivité est impossible pendant qu'on
lit — et l'attache.

**Vérifié de bout en bout**, sur les deux cas :

```
entretien · 2 demande(s) attachée(s) en vol, la plus ancienne il y a 9 s
entretien ·   dû : runtime-dev.obrain
entretien ·   dû : self-demo.obrain
entretien · runtime-dev.obrain   TENUE · …/obrain bridge       ← demande CONSERVÉE
entretien · self-demo.obrain     degrés : EN PLACE (…rescan…)  ← réparée ET ACQUITTÉE
```

```
état de la file après : 1 en attente · âge max 158 s
  · runtime-dev.obrain / degres · « colonne corrompue (mesure : 45x le réel) »
```

Une base libre est **réparée et acquittée** ; une base tenue **garde sa demande et
son âge**. Le jour où les ponts s'arrêtent, la réparation se fait au redémarrage
suivant — sans que personne ait eu à s'en souvenir, et sans qu'aucune mesure ne
soit perdue.

**CLI** : `obrain entretien` (ce qui est dû, et depuis quand),
`--attacher <store> --genre <g> --raison <mesure>`, `--accorder <store> --genre <g>`.

### LA COLONNE DE DEGRÉS SE CORROMPT TOUTE SEULE — le chemin d'écriture est en cause

En nettoyant ce que je croyais être une demande de test, j'ai vérifié la colonne
de `runtime-dev` avant de l'acquitter. Elle était **incohérente** :

```
n_slots = 407 005 · Σ degrés = 197 476 156 · max = 5 238 490 · arêtes ≈ 1 243 111
→ 159 fois le nombre réel d'arêtes
```

**Je me trompais sur deux choses, et la mesure m'a reprise :**

1. **La demande n'était pas un test.** Je l'avais attachée avec une raison
   fabriquée (« 45x le réel ») pour éprouver le protocole — mais le producteur
   `hubs` avait entre-temps **remplacé** cette raison par une mesure réelle (mon
   attachement est idempotent : la dernière mesure gagne). En l'acquittant, j'ai
   effacé une réparation due. Elle est **ré-attachée**, avec sa vraie mesure.
2. **La corruption n'est pas de moi.** La réparation automatique est **désactivée**
   (vérifié : `let _ = raison;`), et l'outil opérateur **ne pouvait pas tourner** —
   les ponts MCP tiennent `runtime-dev` en **exclusif**, toute tentative échoue en
   « already open by another process ». Pourtant la colonne a dérivé jusqu'à 159×
   **pendant l'exploitation**.

**Le coupable est donc le chemin d'ÉCRITURE du harnais.** `runtime-dev` est la
capture vivante, écrite en continu ; `runtime-prod`, écrit bien moins, a une
colonne **exacte** (Σ = 1 118 170 = son nombre d'arêtes). La piste est nette : un
chemin d'écriture (ou de rejeu WAL à l'ouverture) **incrémente les compteurs de
degrés sans contrepartie** — la colonne monte, et rien ne la fait redescendre.

**Ce que la chaîne a fait, et c'est exactement ce pour quoi elle est faite** : le
calcul des hubs a **détecté** l'incohérence, a **refusé** de publier un classement
faux, et a **attaché** une demande mesurée. Le protocole des demandes en vol a
donc servi **le jour même de son écriture**, sur un défaut qui n'existait pas
quand je l'ai conçu.

**Restauré** (colonne retirée, reconstruite par une ouverture neuve) :
`Σ = 1 252 664 · max = 33 155` → cohérente, à 0,77 % de la dérive normale d'une
base vivante. Sauvegarde de la colonne corrompue : `/tmp/colonne-159x.bak`.

**Ce qui reste ouvert, et qui est un vrai sujet moteur** : trouver le chemin
d'écriture qui incrémente sans contrepartie. Tant qu'il existe, la colonne de
`runtime-dev` dérivera de nouveau, le classement se refusera de nouveau, et une
demande s'attachera de nouveau — le système le dit, mais il ne le répare pas.

### LA CAUSE RACINE : une reconstruction qui ADDITIONNAIT au lieu de REMPLACER

« À toi de trouver et de tisser. » C'est trouvé, et c'est le défaut le plus
coûteux de cette base — parce qu'il était **silencieux** et qu'il **empirait tout
seul**.

**Le raisonnement.** `runtime-dev` annonçait 159 fois le nombre réel de ses arêtes
en degrés. J'avais : (1) désactivé ma réparation automatique — vérifié dans le
code ; (2) constaté que l'outil opérateur **ne pouvait pas tourner** — les ponts
MCP tiennent la base en exclusif. Donc **aucun outil de réparation n'avait pu
s'exécuter**, et la colonne avait pourtant dérivé. Il fallait chercher dans le
**chemin de lecture/écriture lui-même**.

**Le coupable**, dans `crates/obrain-substrate/src/degree_column.rs` :

```rust
let mut zf = sub.open_named_zone(filename)?;   // ← open_or_create : RÉUTILISE
ensure_room(&mut zf, total_bytes, 1 << 20)?;
// Zero-init payload (the underlying `ensure_room` grow_to already
// zero-fills new pages).          ← LE COMMENTAIRE EST FAUX EN RÉUTILISATION
```

`ensure_room` ne met à zéro que les pages **neuves**. Sur un fichier **existant**,
le payload garde ses anciennes valeurs. Or quand la CRC d'une colonne est
invalidée — une écriture tronquée sur une base **vivante** suffit — la
réouverture appelle `rebuild_degrees_from_scan`, qui **crée** la colonne
(réutilise le fichier) puis **ajoute** le résultat du scan aux anciennes valeurs.
**La colonne ne se reconstruisait pas : elle s'additionnait.**

159 × ≈ **158 reconstructions accumulées** — autant d'ouvertures d'une base
écrite en continu. `runtime-prod`, ouvert bien moins souvent, avait une colonne
**exacte** : c'est ce contraste qui a désigné le chemin.

**Le correctif** — une ligne, et elle remplace une supposition par un fait :

```rust
zf.as_slice_mut()[HEADER_SIZE..HEADER_SIZE + payload_bytes].fill(0);
```

**Le test qui l'ancre** (`degree_column::tests::recreer_sur_un_fichier_existant_remet_a_zero`) :
une colonne portant 3 et 5, recréée sur le même fichier, doit rendre **0** — pas 3
et 5. Avant le correctif, ce test échoue : c'est exactement l'accumulation mesurée.

**Ce que la chaîne avait fait, en attendant** : détecter (159× > tolérance),
**refuser** de publier un classement faux, et **attacher** une demande mesurée.
Le protocole des demandes en vol — écrit quelques heures plus tôt sur ta
suggestion — a servi **le jour même**, sur un défaut qui n'existait pas quand je
l'ai conçu.

**État** : `runtime-dev` restauré et **stable** (`Σ = 1 252 664 · +0,77 %`, la
dérive normale d'une base vivante) ; la demande est **acquittée**, cause traitée à
la racine. 842 tests substrat (1 échec préexistant, index ordonné), 40 tests CLI,
24/24 endpoints.

**Le motif, une cinquième fois** : un **commentaire qui affirme une propriété
fausse** (« ensure_room zero-fills ») a tenu lieu de vérification pendant que le
code faisait autre chose. Après les deux règles de slug, les deux cartes de
handles, la carte du store sans la fenêtre et les compteurs du manifeste face aux
zones vides — c'est toujours **une source de vérité affirmée plutôt que mesurée**.

### Vérification de bout en bout du correctif — sur fichiers réels, pas en unité

Le correctif est dans le binaire servi, et la vérification a été faite **sur une
copie réelle**, pas seulement par un test unitaire :

```
AVANT                          : n_slots=37 · Σ degrés = 30   (arêtes 30)
CRC invalidée (octet de payload abîmé — l'état d'une écriture tronquée)
APRÈS ouverture (reconstruction) : n_slots=37 · Σ degrés = 30   → REMPLACE
```

Avant le correctif, la même manipulation donnait **60** (30 conservés + 30
rescanés) : c'est exactement l'accumulation qui a porté `runtime-dev` à 159 fois
son nombre d'arêtes en 158 reconstructions.

**Portée vérifiée du correctif** — les autres colonnes héritent ou sont hors de
cause :

| colonne / zone | verdict |
|---|---|
| colonne **générique** de degrés | **corrigée** — `create_by_filename` remet à zéro explicitement |
| colonnes **typées** (par type d'arête) | **hérite** : elles n'appellent `create_by_filename` que si `open` a rendu `None` |
| auto-guérison typée (`zero_type`) | déjà correcte : elle remet à zéro en place |
| `cdc_persist` | sûr : n'écrit son en-tête **que si la zone est vide** |
| `journal_zone` | sûr : il **remplace** son contenu (`shrink_to` + recopie complète) |

Aucun appelant ne comptait sur l'ancien comportement (relecture faite : les trois
seuls appelants sont la reconstruction générique, la voie typée, et les tests).

**État après déploiement** : colonne de `runtime-dev` **stable** (`Σ = 1 252 664`,
+0,77 % — la dérive d'une base vivante, **pas** une accumulation), file
d'entretien **vide** (0 en attente), **24/24 endpoints**, 856 tests substrat
(1 échec préexistant sur l'index ordonné, chantier d'un autre agent).

**Le motif, sixième occurrence** — et celle-ci est la plus instructive : ni un
registre dédoublé, ni une mesure périmée, mais **un commentaire qui affirmait une
propriété du code** (« `ensure_room` zero-fills ») et qui a tenu lieu de
vérification. Le correctif tient en une ligne ; c'est de l'avoir cru sur parole
qui a coûté 158 reconstructions silencieuses.

### LE PROTOCOLE EST VISIBLE — un troisième temps qui manquait

Le protocole des demandes en vol avait un **producteur** (il attache quand la
réparation est impossible) et un **consommateur** (l'entretien de redémarrage
applique et accorde). Il manquait le troisième temps : **l'interface**. Une
réparation due n'existait que dans la sortie d'une commande que personne ne
lance — et une attente silencieuse ne se distingue pas d'un oubli.

**Ajouté** :

- `GET /api/entretien` — les réparations dues, avec leur **âge** et la commande
  qui les applique ;
- une **pastille dans l'en-tête du chat** : `⚠ 1 réparation due · 1 min`, dont
  l'infobulle nomme la base, le genre, la raison mesurée et l'ancienneté.

**Vérifié dans le navigateur** (geste réel, CDP) :

```
pastilles de l'en-tête : ⚠ 1 réparation due · 1 min | think visible | ✕
infobulle : Réparations attachées en vol, en attente de la fenêtre d'exclusivité
            (elles s'appliquent au prochain redémarrage) :
            • kv-linux-e1000.obrain / enregistrements — controle de visibilite…
```

Le contrôle a été fait avec une demande **réelle mais bénigne** (un contrôle de
visibilité que j'ai attaché et acquitté moi-même, raison explicite) : la pastille
apparaît, l'infobulle explique, et l'acquittement la fait disparaître.

**Le retenu** : un protocole se juge à ce qu'on peut **voir**, pas à ce qu'on peut
taper. Les deux premiers temps étaient vérifiables en ligne de commande ; le
troisième ne l'était pas — et c'est celui qui décide si une dette technique se
paie ou s'oublie.

---

## 2026-09-10 — chantier « Dashboard graph : big picture tissée & instantanéité »

### Représentation (Monde)
- Cartographie auto-adaptée : projets+branches (2 sauts), tissu organique (liens denses), familles.
- Cascade multi-hop au clic (3 sauts), survol sans doublon, épinglage multi, vue du dessus, carte-mère big picture (0 requête réseau), aide clavier (H/?).
- Grammaire de contexte unifiée `↰ / ↳ / ·` ; historique de navigation réel (pushState/popstate) ; deep-links `?store=&dive=` + titres.

### Performance & backend
- Agrégats durables disque+mémoire (mtime), `< 1 ms` aux revisites, survie aux redémarrages ; warm au déploiement (`vizserve.sh`).
- `/api/viz/space` allégé (`links=0`) + clé unique ; préchauffage auto des enfants et petits-enfants (cascade serveur) ; requêtes GQL cachées.
- Monitoring `GET /api/viz/status`, pages `/sitemap.xml` générées à la volée.

### Qualité & livrables
- `docs/dashboard-viz/GRAPH.md` (base graph : décisions, matrice, backlog).
- Vérifications versionnées : `scripts/smoke-backend.sh`, `scripts/e2e-dashboard.mjs`, `scripts/check-all.sh`.
- Refactor partiel (Étape F) : primitives pures extraites dans `src/graph/monde/util.ts`.

---

All notable changes to Obrain will be documented in this file.

This project was forked from [GrafeoDB/grafeo](https://github.com/GrafeoDB/grafeo) and rebranded as **Obrain** ([obrain.dev](https://obrain.dev)). The upstream Grafeo changelog is preserved in `CHANGELOG-grafeo.md` for historical reference.

## [0.0.1-m6-zorder] - 2026-09-07

Consolidation sur `main` de la vague zorder/M6 (feat/release-m6-zorder, HEAD `adc11a68`). Détail daté et rejouable : `docs/ledger-ameliorations-2026-09-04.md`, RFCs `docs/rfc/`.

### Moteur / substrate
- Fenêtres 2D Z-order : `CALL obrain.window_2d` (C5) et `obrain.window_2d_energy` (C3, sous-graphe vivant) — scans/plages Z, lecture seule.
- BM25 ≤ 1 ms (684 µs mesuré), RAG topologique industrialisé, kernel vivant, courbure Ricci-Ollivier (Jost-Liu, par arête + `node_curvature`) et HKS 3 échelles par nœud.
- Reverse-adjacence `PART_OF` O(1) (compteur µs) + accesseurs `sessions_part_of`.
- CDC : journal zone `substrate.cdc_journal` persisté ; rechargement **paresseux** (fix régression d'ouverture CLI/bridge ~0,9 s/1 Go → 0,07 s/35 Mo).
- Auto-guérison des colonnes de degrés typées périmées (fini le `MATCH (a)-[:T]->(b)` = 0 silencieux) + `SubstrateStore::edge_endpoints_by_id`.
- Indexer M6 : indexation optimisée (colonnes mono-passage, caches top-N/compteurs, requêtes en µs), `tests_pass` indexé.

### Recherche (formes pour l'IA) — 5/5 + relief
- `obrain.search_delta` — diff CDC graphe depuis `since_ms`.
- `obrain.search_surprise` — surprise d'activité (activité vs attente ∝ degré).
- `obrain.search_by_role` — rôles structurels invariants au contenu (HKS + clustering ; cache sidecar < 1 ms).
- `obrain.search_conflicts` — dissonance temporelle d'états.
- `obrain.search_causes` / `obrain.search_effects` — spread directionnel causal.
- `obrain.relief` — projection contextuelle ; procédures toutes classées LECTURE.

### CLI / outils
- `obrain compare <A> <B>` — comparatif multi-variantes (identité (label, qn), mode structurel rapide ~80 ms/37 k, `--attrs` profond, résumés précalculés par journal d'import) ; vue N-variantes par paires.
- `obrain enrich [--ricci]` — passes additifs/idempotentes post-import ; `obrain repair-degrees`, `journal migrate`, `native-import`.
- Bridge : sessions_list/session_load optimisés (memo-cache, requêtes ancrées, 4-17 ms), support OR-EXISTS côté planner (fini les faux silencieux).


## [0.2.0] - 2026-04-22

### 🌋 Substrate cutover — topology-as-storage

Obrain's primary storage backend is now **`SubstrateStore`** — a single
mmap'd directory where the topology *is* the storage. `LpgStore` (the
legacy in-memory graph + bincode snapshots) is deprecated and will be
removed in a subsequent release.

See [RFC-SUBSTRATE](docs/rfc/substrate/format-spec.md) and the
[Migration guide](docs/MIGRATION.md).

### Breaking

- **File format**: new `substrate.obrain/` directory layout (32 B node
  records, 30 B edge records, 4 KiB property pages, string heap with
  `(offset, len)`). Existing `.obrain` files must be migrated via
  `obrain-migrate --upgrade`. A 7-day rollback window is supported
  using the `legacy-v0.1` release binary.
- **Cognitive stores** (`Energy`, `Scar`, `Utility`, `Affinity`,
  `Synapse`, `Engram`, `Coactivation`) are now thin views over
  substrate columns; direct writers that targeted the old monolithic
  stores must move to `SubstrateStore::*` column-view APIs.
- **HNSW index replaced** by the L0 (128-bit) / L1 (512-bit) / L2 (f16
  cosine) tier cascade. The public retrieval API (`vector_search`)
  keeps the same signature, but the implementation no longer builds
  an in-RAM HNSW graph.

### Added

- `obrain-substrate` crate — single mmap'd store with WAL-native
  durability, index-free adjacency, column-inline cognitive state.
- `obrain-migrate` CLI — one-shot offline migrator from `.obrain`
  legacy files to the substrate directory layout (dry-run,
  upgrade, force modes).
- **Self-maintaining Thinkers** — auto-started on `open()`:
  `Consolidator` (energy decay), `CommunityWarden` (LDleiden + Hilbert
  compaction), `Predictor` (retrieval warm-up), `Dreamer`
  (cross-community synapse proposals).
- **Geometric inference columns** — `ricci` (Ricci-Ollivier curvature
  per edge), `curvature` (per node), heat-kernel diffusion,
  geodesics over the f16 embedding manifold.
- **LDleiden incremental community detection** — online, `O(|ΔE|)`
  on edge mutations; drives `community_id` and Hilbert page
  ordering.
- `neo4j2obrain` rewrite — writes substrate directly with cognitive
  by-construction (embeddings, tiers, communities, centrality,
  Ricci, COACT edges) in one import.

### Performance (measured on 10⁶-node synthetic + megalaw)

| Metric | 0.1.x (LpgStore) | 0.2.0 (Substrate) | Gain |
|---|---|---|---|
| RSS total (10 users + 5 KBs) | ~110 GiB | ≤ 18 GiB | ÷ 6-10 |
| RSS anonymous incompressible | 5–10 GiB | ≤ 1 GiB | ÷ 10 |
| Startup (10⁶ nodes) | 5–15 s | ≤ 100 ms | ÷ 50-150 |
| Retrieval p95 (10⁶ nodes) | 30–100 ms | ≤ 1 ms | ÷ 30-100 |
| Recall@10 vs HNSW f32 baseline | 100 % | ≥ 99 % | preserved |

### Fixed (resolved by construction)

- Cognitive stores bypass-WAL gotcha — every reinforce / energy
  update / scar insert is now WAL-logged synchronously before the
  column write.
- `AssetRegistry` / `RoomStore` / `SpaceStore` / `IntegrationStore`
  direct-mutation paths — all routed through `HubWalStore` + substrate
  WAL-native columns.
- Dichotomy `ObrainDB::open` (full RAM) vs `ObrainDB::open_overlay`
  (mmap + delta LpgStore) — single `open` path.
- Cognitive state WAL vs full-snapshot inconsistency — single source
  of truth (substrate).

## [0.0.1] - 2026-03-27

### 🎉 Initial Release — Clean Break from Grafeo

This is the first release of **Obrain**, a complete rebrand of the Grafeo graph database.

### Summary

- Full rebrand: all references to "grafeo" replaced with "obrain" across 15 crates, 7 language bindings, CI/CD, and documentation
- New file extension: `.obrain` (replaces `.grafeo`)
- New npm scope: `@obrain`
- Version reset to `0.0.1` — clean break, no backward compatibility with previous Grafeo releases

### Inherited Features

- **Multi-model graph database**: LPG (Labeled Property Graph) + RDF dual-model storage
- **6 query languages**: GQL (ISO), Cypher, Gremlin, GraphQL, SPARQL, SQL/PGQ
- **ACID transactions** with MVCC epoch-based concurrency
- **Vector search**: HNSW index for approximate nearest-neighbor queries
- **Full-text search**: BM25-based text indexing
- **Temporal properties**: opt-in append-only versioning with point-in-time queries
- **7 language bindings**: Python, Node.js, C, WASM, Go, Dart, C#
- **Snapshot persistence**: binary snapshot format with WAL (Write-Ahead Log)
- **Observability**: Prometheus metrics export, structured tracing spans
- **Graph Data Science**: community detection, centrality, pathfinding algorithms
- **Read-only mode**: shared file lock for concurrent readers

### Breaking

- No backward compatibility with previous Obrain releases
- File format uses `.obrain` extension exclusively
- All crate names changed from `obrain-*` to `obrain-*`
- Python package: `obrain` (was `obrain`)
- npm package: `@obrain/obrain` (was `@obrain/obrain`)
