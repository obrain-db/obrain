# @obrain-db/wasm : obrain dans le navigateur (essai, non publié)

Base de graphes obrain compilée en WebAssembly, pour le navigateur et Node.js.
Une seule liaison WASM dans le dépôt : ce crate (`crates/obrain-web`). Le paquet npm
est construit par `npm/construire.sh`. **Aucune version n'est publiée** : la version
`0.1.0-essai.0` du `package.json` sert à la publication future.

- **en mémoire** : `new Base()`, perdue au rechargement de la page ;
- **persistante (navigateur)** : `await Base.ouvrir(nom)` dans un worker dédié, adossée à
  un journal OPFS, avec `persister()` pour publier l'état courant.

## Construire le paquet

Version de `wasm-pack` épinglée : **0.13.1**. Version de `wasm-bindgen` : celle du
`Cargo.lock` (**0.2.130**), que `wasm-pack` reprend.

```sh
cargo install --locked --version 0.13.1 wasm-pack
crates/obrain-web/npm/construire.sh   # produit crates/obrain-web/pkg-npm/ et liste l'archive
```

`pkg-npm/` n'est pas commité. Le script remplace le `package.json` généré par
`npm/package.json` (nom `@obrain-db/wasm`, `files`, `types`, `exports`). Il n'exécute pas
`npm publish`.

Pour la batterie de tests, la CI compile le profil complet puis génère les liaisons :

```sh
cargo build --target wasm32-unknown-unknown -p obrain-web --release --features complet
wasm-bindgen --target nodejs --out-dir pkg      target/wasm32-unknown-unknown/release/obrain_web.wasm
wasm-bindgen --target web    --out-dir pkg-web  target/wasm32-unknown-unknown/release/obrain_web.wasm
```

## Démarrer

### Navigateur, persistant (OPFS, worker dédié)

OPFS ne donne son accès synchrone que dans un worker dédié. `Base.ouvrir` doit donc être
appelé dans un worker :

```js
// base.worker.js
import init, { Base } from "@obrain-db/wasm";
await init();
const b = await Base.ouvrir("ma-base.journal"); // créée si absente
b.creer_noeud(["Personne"], JSON.stringify({ nom: "Alix" }));
b.persister(); // publie l'instantané : ce qui précède est durable
```

```js
// page principale
const w = new Worker(new URL("./base.worker.js", import.meta.url), { type: "module" });
```

Hors worker, `Base.ouvrir` rejette par une erreur lisible (vérifié par la batterie node
et par `persistance : ouvrir hors worker dédié rejette…`).

### Node.js, en mémoire

Le paquet est en cible `web` : Node charge le `.wasm` à la main, en passant ses octets à `init`.
OPFS n'existe pas dans Node, donc seule la base en mémoire est disponible.

```js
import { readFile } from "node:fs/promises";
import init, { Base } from "@obrain-db/wasm";

await init({ module_or_path: await readFile(new URL("./node_modules/@obrain-db/wasm/obrain_web_bg.wasm", import.meta.url)) });
const b = new Base();
b.executer("INSERT (:Personne {nom: 'Alix'})");
console.log(JSON.parse(b.executer("MATCH (p:Personne) RETURN p.nom AS nom")));
// { colonnes: ["nom"], lignes: [["Alix"]] }
```

Ce chemin a été exécuté avec Node : `executer`, `importer_noeuds`, `executer_langage_avec_parametres`.

## API

Les noms sont en français, comme le reste de `obrain-web`. Il n'y a pas d'alias anglais :
une opération, un nom. La correspondance avec l'API `obrain-wasm` du banc est donnée plus bas.

### Conventions

- **Résultats** : chaîne JSON. Requête : `{"colonnes": [...], "lignes": [[...], ...]}`.
  Recherches : tableau `[{"noeud": id, "score": s}]` (texte, hybride) ou
  `[{"noeud": id, "distance": d}]` (vectoriel). Schéma, mémoire, métriques : objet JSON.
- **Paramètres** : objet JSON `{"nom": valeur}` (chaîne). Une valeur liée n'est jamais
  interprétée comme du langage.
- **Vecteurs** : `Float32Array`.
- **Identifiants de nœud** : nombres (`number`).
- **Erreurs** : chaque méthode faillible lève une erreur JavaScript avec le message du moteur.
  Aucune méthode ne déclenche de panique : une panique wasm abattrait l'instance.

### `Base`

| Méthode | Rend | Rôle |
|---|---|---|
| `new Base()` | `Base` | Base vide, en mémoire. |
| `Base.ouvrir(nom)` | `Promise<Base>` | Base persistante OPFS `nom` (worker dédié). |
| `persister()` | `void` | Publie l'instantané dans le journal (base ouverte avec `ouvrir` uniquement). |
| `executer(requete)` | `string` | GQL. |
| `executer_avec_parametres(requete, parametresJson)` | `string` | GQL avec paramètres. |
| `executer_langage(langage, requete)` | `string` | Langage au choix : `gql`, `cypher`, `sparql`, `gremlin`, `graphql`, `sql`. |
| `executer_langage_avec_parametres(langage, requete, parametresJson)` | `string` | Comme `executer_langage`, avec paramètres. |
| `session()` | `Session` | Session pour les transactions (voir plus bas). |
| `noeuds()` / `aretes()` | `number` | Nombre de nœuds / d'arêtes. |
| `creer_noeud(etiquettes, proprietesJson)` | `number` | Crée un nœud, rend son identifiant. |
| `definir_vecteur(noeud, propriete, vecteur)` | `void` | Pose un `Float32Array` sur un nœud. |
| `importer_noeuds(etiquette, lignesJson)` | `number` | Importe un tableau JSON d'objets comme nœuds d'étiquette `etiquette`. Rend le nombre créé. Tout le lot est validé avant la première écriture : une ligne invalide n'écrit rien. Les arêtes s'écrivent en GQL (`MATCH … CREATE`). |
| `schema()` | `string` | Schéma observé (JSON). |
| `memoire()` | `string` | Empreinte mémoire détaillée (JSON). |
| `vider_cache_plans()` | `void` | Vide le cache des plans. Sans effet sur les résultats. |
| `metriques()` | `string` | `{"requetes": n, "erreurs": n}`. |
| `creer_index_texte(etiquette, propriete)` | `void` | Index plein texte. |
| `supprimer_index_texte(etiquette, propriete)` | `boolean` | Retire l'index ; `true` s'il existait. |
| `reconstruire_index_texte(etiquette, propriete)` | `void` | Reconstruit depuis les données ; crée l'index s'il est absent. |
| `rechercher_texte(etiquette, propriete, requete, k)` | `string` | Recherche plein texte, `k` résultats au plus. |
| `creer_index_vectoriel(etiquette, propriete, dimensions, metrique)` | `void` | Index vectoriel (`metrique` : par exemple `cosine`). |
| `supprimer_index_vectoriel(etiquette, propriete)` | `boolean` | Retire l'index ; `true` s'il existait. |
| `reconstruire_index_vectoriel(etiquette, propriete)` | `void` | Reconstruit en relisant la dimension des données (utile après une migration d'embeddings). Erreur si aucun vecteur ne permet de la déduire. |
| `rechercher_vecteur(etiquette, propriete, vecteur, k)` | `string` | Plus proches voisins. |
| `rechercher_hybride(etiquette, proprieteTexte, proprieteVecteur, texte, vecteur, k)` | `string` | Recherche texte et vecteur combinée. Un `vecteur` vide désactive la partie vectorielle. |

Les méthodes d'index et de recherche existent quand le paquet est construit avec les features
correspondantes ; le paquet publié (`--features complet`) les contient toutes.

### `Session` : transactions

| Méthode | Rôle |
|---|---|
| `session.executer(requete)` | Requête GQL dans la session (`string`, JSON). |
| `session.commencer()` | Ouvre une transaction explicite. |
| `session.valider()` | Valide. Erreur sans transaction ouverte. |
| `session.annuler()` | Annule la transaction en cours. |

```js
const s = b.session();
s.commencer();
s.executer("INSERT (:Personne {nom: 'Gus'})");
s.annuler(); // rien n'est écrit
```

## Persistance

**Décision C (v0.0.1) : pas de WAL sur OPFS.** La durabilité est celle de l'instantané publié
par `persister()`. Un journal écrit à chaque mutation n'existe pas ; sa limite est listée dans
`docs/LIMITES-v0.0.1.md`.

**Persisté : la dernière publication réussie.** `persister()` écrit l'état complet de la base
(nœuds, arêtes, propriétés, dictionnaires) dans le journal OPFS, en deux emplacements logiques
A/B portant un numéro de génération et un CRC. Quand `persister()` rend `Ok`, cet état survit à
la fermeture et au rechargement. Une publication coupée ouvre la précédente, ou une base vide si
aucune n'a abouti : jamais une erreur d'ouverture, jamais un mélange des deux. Verrou :
`obrain-substrate`, `miroir::tests_journal::un_persist_coupe_ouvre_une_generation_complete`.

**Non promis : les écritures faites depuis la dernière publication.** Ce n'est pas un journal.
Une écriture non suivie de `persister()` n'est pas promise. Elle est perdue à la fermeture,
au rechargement ou au crash. Verrou : `tests/navigateur.rs::les_ecritures_apres_la_derniere_publication_sont_perdues`.

**Un import est persisté comme le reste.** `importer_noeuds` n'écrit pas sur OPFS avant
`persister()`. Verrou : `tests/navigateur.rs::un_import_publie_survit_a_une_reouverture`.

**Coût et plafond.** Chaque publication ajoute au journal **deux** images complètes de la base,
et le journal n'est jamais compacté. Le plafond de 2 Gio porte sur la taille totale du journal :
une base dont l'image pèse S octets ne peut être publiée qu'environ 1 Gio / S fois. Au-delà, la
publication est refusée avant toute écriture.

**Non vérifié : les index après réouverture.** Les index texte et vectoriel (`creer_index_*`) ne
sont pas testés à la réouverture. Recréez-les après `Base.ouvrir` au besoin.

**Un seul écrivain par journal.** Un second `Base.ouvrir(nom)` sur le même fichier est refusé tant
que le premier n'est pas libéré (`free()`, ou ramassage par le GC, non déterministe). Test :
`tests/navigateur.rs::un_second_ouvreur_du_meme_fichier_est_refuse`.

## Limites

- **OPFS seulement dans un worker dédié.** Hors worker, `Base.ouvrir` rejette. Node n'a que la
  base en mémoire.
- **Pas de lecture seule sémantique en OPFS.** Il n'existe pas de mode d'ouverture en lecture
  seule : ouvrir un journal lui donne un accès en écriture, et le paquet ne fournit pas de
  garantie de lecture seule.
- **Mono-onglet.** Un journal a un seul écrivain à la fois : OPFS refuse un second accès au même
  fichier (voir *Un seul écrivain par journal*). Une base ouverte dans un autre onglet n'est donc
  pas accessible en parallèle.
- **Pas de threads.** Le paquet s'exécute sur un seul thread wasm : pas de parallélisme
  interne, et les grosses requêtes bloquent le worker qui les exécute.
- **Instantané, pas de journal.** Voir *Persistance*.
- **Taille.** Le paquet n'est pas passé par `wasm-opt` (`wasm-opt = false` dans
  `Cargo.toml`) : le `.wasm` fait environ 8 Mo. À optimiser avant toute publication.
- **Navigateurs.** La batterie navigateur tourne sur Chrome headless épinglé. Firefox et Safari
  ne sont pas testés.
- **Essai non publié.** Aucune version n'existe sur npm et l'API peut changer.

## Correspondance avec `obrain-wasm` (banc n3)

Le banc utilisait un autre paquet, `obrain-wasm`. Ses opérations sont reprises ici sous les noms
de `obrain-web`. Le tableau détaillé, avec les écartements motivés, est dans la description de la
PR.

| `obrain-wasm` | `obrain-web` |
|---|---|
| `new Database()` | `new Base()` |
| `execute` / `executeWithParams` | `executer` / `executer_avec_parametres` |
| `executeWithLanguage` / `executeWithLanguageAndParams` | `executer_langage` / `executer_langage_avec_parametres` |
| `executeCypher`, `executeGremlin`, `executeGraphql`, `executeSparql`, `executeSql` | `executer_langage("cypher" \| "gremlin" \| "graphql" \| "sparql" \| "sql", …)` |
| `nodeCount` / `edgeCount` | `noeuds()` / `aretes()` |
| `createTextIndex` / `dropTextIndex` / `rebuildTextIndex` | `creer_index_texte` / `supprimer_index_texte` / `reconstruire_index_texte` |
| `createVectorIndex` / `dropVectorIndex` / `rebuildVectorIndex` | `creer_index_vectoriel` / `supprimer_index_vectoriel` / `reconstruire_index_vectoriel` |
| `textSearch` / `vectorSearch` / `hybridSearch` | `rechercher_texte` / `rechercher_vecteur` / `rechercher_hybride` |
| `schema` / `memoryUsage` / `clearPlanCache` | `schema()` / `memoire()` / `vider_cache_plans()` |
| `importRows` (mode `nodes`) | `importer_noeuds` |
| (absent de `obrain-wasm`) | `Base.ouvrir`, `persister`, `Session` (transactions), `metriques` |

Non repris : `mmrSearch`, `importLpg`, `importRdf`, `executeRaw` / `executeRawWithLanguage`,
`importRows` en mode `edges`, `version`, `cognitive_demo`. La raison de chaque écartement est
dans la PR.

## Tests

- **Batterie partagée** (`tests/js/suite.mjs`, 44 tests) : exécutée sous node (`tests/js/node.mjs`)
  et dans Chrome headless (`tests/js/navigateur.html`). Les tests dépendant d'une feature
  s'ignorent explicitement quand la méthode est absente.
- **Tests navigateur `wasm-bindgen-test`** (`tests/navigateur.rs`, 8 tests) : persistance OPFS en
  worker dédié, dans Chrome headless (`cargo test --target wasm32-unknown-unknown -p obrain-web --test navigateur`).

Les deux sont exécutés par la CI : job `web-paquet` (node + Chrome si disponible) et job de
persistance navigateur.
