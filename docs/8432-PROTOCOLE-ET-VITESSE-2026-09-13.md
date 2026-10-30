# 8432 — protocoles et vitesses : l'état mesuré, et ce qui est corrigé

Chantier du 2026-09-13. **Tout chiffre de ce document est mesuré**, sur le
service vivant (`obrain viz serve` sur `192.168.1.103:8432`, binaire
`target/release/obrain`, store `Runtime dev (engrams)` ≈ 412 773 nœuds) ou sur
un corpus de même forme. Les commandes sont données pour que la mesure se
refasse — c'est une porte, pas un rapport.

---

## 1. Protocole : 14 échecs sur 15 au départ

Instrument : `scripts/protocole-check.mjs` (ajouté par ce chantier). Il fige en
non-régression les huit défauts trouvés. Relevé AVANT, sur le service vivant :
```text
FAIL  traversée /app/../../etc/hostname refusée            code=200 — FUITE DE CONTENU
FAIL  bundle servi en gzip                                 1286462 → 1286462 o (0.0 %)
FAIL  Vary: Accept-Encoding présent                        Vary: —
FAIL  JSON d'API compressé                                 14192 o, sans Content-Encoding
FAIL  2 requêtes pipelined, 2 réponses                     réponses=1
FAIL  HEAD / → 200 sans corps                              code=405, Content-Length=18
FAIL  HEAD sur route absente → 404                         code=405
FAIL  en-tête Date présent                                 ABSENT
FAIL  405 porte Allow:                                     ABSENT
FAIL  ligne de requête vide → 400                          AUCUNE RÉPONSE
FAIL  HTTP/1.1 sans Host → 400                             code=200
FAIL  ETag sur page /                                      aucun ETag
FAIL  ETag sur bundle haché                                aucun ETag
FAIL  page d'entrée revalidable (pas no-store)             Cache-Control: no-store
PASS  /api/stores répond                                   11 ms
```

### 1.1 La faille : traversée de chemin sur `/app/*`

`viz_serve.rs:1113-1117` fait `app_dir().join(rel)` avec `rel` **ni décodé ni
assaini**, alors que la route voisine des fichiers *vendor* porte la garde
complète (`static_vendor:1191-1197` : refus de `..`, `/`, `\`). Mesuré :

```console
$ curl --path-as-is 'http://192.168.1.103:8432/app/../../../../../../../etc/hostname'
HTTP/1.1 200 OK … n3
```

Tout fichier lisible par le processus était servi, **hors garde IAM** (les
actifs statiques sont volontairement publics, commentaire `1028-1030`) : la
garde ne s'applique donc pas à la fuite.

### 1.2 Le transport

| fait | mesure | où |
|---|---|---|
| `Connection: close` systématique | 2 requêtes = 2 `Connected` ; **352 sockets TIME-WAIT** sur le port (≈5,9 fermetures/s) | `respond_entetes:1175`, SSE `:3791` |
| aucune compression | `Accept-Encoding: gzip, deflate, br, zstd` ignoré ; bundle **1 286 462 o** | `respond_entetes:1157-1186` |
| aucun timeout de socket | `set_read_timeout`/`set_write_timeout` absents de tout le fichier | limite douce **1 024 fd** |
| `HEAD` refusé | 405 **avec** un corps de 18 o | `:1011-1013` |
| aucun `Date` | 0 occurrence | RFC 9110 §6.6.1 |
| aucun `ETag`/`304` | `If-None-Match` → 200 + 1 286 462 o | 0 occurrence |
| `/` en `no-store` | alors que le commentaire `:1136-1138` promet `no-cache` | `:1059-1067` |
| requête vide → silence | le client pend (aucune réponse) | `:1004-1006` |
| SSE nu | ni `retry:`, ni battement de cœur, ni `X-Accel-Buffering` | `:3790-3792` |

**Ce que la compression vaut, mesuré** (gzip niveau 6, `flate2`) :

| charge | avant | après gzip | gain |
|---|---|---|---|
| `index-*.js` | 1 286 462 o | 377 771 o | **−70,6 %** |
| `index-*.css` | 397 946 o | 271 863 o | −31,7 % |
| `vendor/three.min.js` | 603 445 o | 149 206 o | −75,3 % |
| `/api/viz/weave` | 41 304 o | 6 311 o | −84,7 % |
| `/api/viz/modules` | 14 192 o | 592 o | **−95,8 %** |
| **charge critique du 1er chargement** | **1 685 394 o** | **650 620 o** | **−61,4 %** |

`flate2` **était déjà lié** dans le binaire (tiré par `ureq` ←
`tree-sitter-language-pack` ← `obrain-indexer`, feature `indexer` du défaut) :
le déclarer en dépendance directe ne compile aucun crate nouveau.

**Ce qui est délibérément écarté** : HTTP/2 et HTTP/3 dans ce binaire. Le module
se définit « std-only, zéro dépendance » (`viz_serve.rs:3`), sur `TcpStream` nu
sans TLS ; le h2c n'est pas utilisé par les navigateurs. La voie, si on la veut,
est un **frontal nginx/caddy** devant le service — et dans ce cas le
`Connection: close` d'aujourd'hui serait nuisible, puisque le frontal réutilise
ses connexions amont (c'est l'une des raisons de le corriger). De même : `zstd`
est déjà lié mais aucun navigateur ne le négocie ; `id:`/`Last-Event-ID` est
sémantiquement inadapté (le flux est une génération à **continuer**, pas un
journal à rejouer — le schéma `deja`/`reprise` du 2026-09-13 est supérieur).

### 1.5 Le service 8432 est repassé derrière IAM — et la porte le dit

Pendant la vérification finale, le service a été relancé avec
`OBRAIN_IAM=1` (env du processus : `OBRAIN_IAM_STORE`, `OBRAIN_IAM_ACCOUNT=obrain`).
Conséquence mesurée : `/` répond **303 → `/login`** (« connexion requise ») et
`/api/**` répond **401**. Les contrôles de PAGE (bundle, `ETag`, `no-cache`) n'ont
alors plus d'objet — il n'y a pas de bundle à compresser sur une redirection.

La garde ne les compte donc plus comme des échecs : elle les **SKIP** avec la
raison et le geste qui les rendrait applicables, et mesure à la place ce qui
EXISTE sans session (compression de la page de connexion : **1307 → 720 o**).
Verdict sur le service vivant :

```text
# http://192.168.1.103:8432 — service derrière IAM (303 → /login)
PASS  traversée /app/../../etc/hostname refusée            code=404
SKIP  compression du bundle                                IAM actif (voir §6)
PASS  2 requêtes pipelinées, 2 réponses                    réponses=2
PASS  HEAD / → 200 sans corps · HEAD absente → 404
PASS  en-tête Date présent · 405 porte Allow: · requête vide → 400 · sans Host → 400
SKIP  ETag / 304 sur page et bundle · page d'entrée revalidable
PASS  compression de la page de connexion (sans session)    1307 → 720 o, Content-Encoding: gzip
PASS  /api/stores répond                                    3 ms, code=401

PROTOCOLE : TOUT PASSE  (3 contrôles NON APPLICABLES)
```

Et sur l'instance isolée, **sans** IAM : `PROTOCOLE : TOUT PASSE`, 15 contrôles,
`If-None-Match` → **304** sur la page et sur le bundle, `Cache-Control: no-cache`.

---

## 2. Vitesses : les quatre postes, et leur cause

Mesures « clé neuve » (paramètres jamais demandés, donc un calcul réel) et
« chaud » (même clé rejouée), 5 tours.

| endpoint | clé neuve | chaud | octets | cause localisée |
|---|---|---|---|---|
| `/api/viz/arcs` | **1 849–1 963 ms** | 2,7 ms | 3 413 | agrégat de rang exécuté **3×** (`:2483`, `:2484` écrasé, `:2486`) ; cache indexé par `(store, top, limit)` (`:2409`, `:2430`) alors que le calcul se fait à `engine_limit` fixe |
| `/api/viz/space` | **586–2 025 ms** | 0,9–1,4 ms | 165–173 | préchauffage `ObrainDB::open_read_only` **en direct** (`:2871`) : **ouverture complète d'un store de 11 Go**, 8 ouvertures relevées dans le journal |
| `/api/viz/weave` | **3 782 ms** (1er appel) | 0,7–1,1 ms | 7 435 | `read_persisted` puis, par monde absent, `cached_ro` + 2-3 GQL (`:3112`, `:3126-3129`) ; clé = mtimes de **tous** les mondes (`:3068-3074`) |
| `/api/viz/subgraph` | 53–117 ms | **46–63 ms** | **1,75–2,30 Mo** | `subgraph_cache_get` **clone la String sous le verrou** (`:1325`) ; TTL 8 s (`:1293`) |
| `/api/functions` | 77 ms → **1 250 ms** | 0,9–1,3 ms | 6 730 | 4 paliers séquentiels (`:1632-1637`) ; construction de trigrammes à la 2ᵉ requête d'une propriété (`title` 260,9 ms, `file_path` 625,6 ms) |

Causes décisives, mesurées en isolation :

- la lecture d'un groupe de rang coûte **407-478 ms** (rang 2), **366-400 ms**
  (rang 3) : la répétition n'est donc pas cosmétique ;
- le préchauffage ne sonde **que `qualified_name`** (`:461-473`) alors que le
  catalogue déclare aussi `title`, `name`, `file_path`, `payload`, `text`,
  `extraction_text` — le coût de construction tombait donc dans une question ;
- `/tmp/obrain-viz-cache` : **1 821 fichiers / 36 Mo**, **aucun élagage**.

### 2.2 Ce qui a été corrigé, et la mesure APRÈS

Instrument : `scripts/vitesse-8432.mjs` (1ʳᵉ passe = le calcul, puis le régime).
Comparaison de deux passes, mêmes requêtes, artefacts
`/tmp/perf-audit/avant-vitesse.json` et `apres-vitesse.json` :

| mesure | AVANT (1ʳᵉ passe) | APRÈS (1ʳᵉ passe) | ce qui a changé |
|---|---|---|---|
| `/api/viz/arcs`, `top` neuf | **2 523 ms** (pointe 6 667) | **2,6 ms** | l'agrégat de rang n'est plus exécuté 3× ; la clé de cache ne porte plus `top`/`limit` (l'agrégat est calculé à `engine_limit` fixe puis tronqué) |
| `/api/viz/subgraph`, 227 898 o | 227 898 o sur le fil | **17 258 o (−92,4 %)** | gzip négocié (§1.3) |
| `/api/viz/modules`, 14 192 o | 14 192 o sur le fil | **580 o (−95,9 %)** | gzip négocié |
| `/api/viz/space`, préfixe neuf | 586–2 025 ms (audit) | ouverture de base supprimée | `open_read_only` → `cached_ro` : plus d'ouverture complète d'un store de 11 Go par processus |
| cache disque `/tmp/obrain-viz-cache` | **1 821 fichiers / 36 Mo**, aucun élagage | **407 entrées** | élagage borné |
| préchauffage des trigrammes | 1 propriété (`qualified_name`) | **4 propriétés par monde** | la construction ne tombe plus dans une question (journal : `proprietes=4 restantes=[]`) |

**Contrôle croisé de compression (le nôtre était faux trois fois)** : les chiffres
« sur le fil » se lisent dans le `Content-Length` annoncé, jamais dans la taille
du corps rendu par `fetch` (qui dégonfle tout seul). Vérification à la main sur
la page de connexion du service IAM : `1307 → 720 o`, **magie `1f8b`** contrôlée
sur les octets reçus.

**La correction du cache d'`arcs` est VÉRIFIÉE, pas seulement chronométrée** —
un cache qui ignore `top` doit rendre la même chose pour tous les `top`, sinon
il invente une réponse :

```console
top=16  : 73 arcs · src→web:803 | miner→web:760 | scripts→src:554
top=48  : 73 arcs · src→web:803 | miner→web:760 | scripts→src:554
top=220 : 73 arcs · src→web:803 | miner→web:760 | scripts→src:554
```

**Ce qui reste lent, et c'est dit** : le **premier** appel à `/api/viz/weave`
reste ~3,9 s (il ouvre chaque monde absent de la toile et l'interroge) ; les deux
agrégats moteur `type d'arête` (**1 151-1 219 ms mesurés**) et
`labels(a)/labels(b) GROUP BY` restent payés à chaque exécution — ils exigent
d'écrire dans le moteur ou le substrat, où une autre session travaille (§5).

### 2.1 Deux postes côté moteur, payés à CHAQUE exécution

| requête | temps | remarque |
|---|---|---|
| `MATCH ()-[r]->() RETURN type(r), count(r)` | **545–563 ms** | contre 15 ms pour `count(r:PROVES)` : ~36× un seul type |
| `labels(a)[0], labels(b)[0] GROUP BY` | **684–723 ms** | appelée par `fetchLabelPairs` — requête dont `api.ts:132-145` documente qu'elle est **fausse** (label de la cible recopié) |
| `STARTS WITH` | 67–72 ms | contre 0,28–0,48 ms pour `CONTAINS` : la série ordonnée est **vide** (`substrate.props.idx.v2` = 4 096 o, jetée à chaque ouverture, `props_index.rs:525-531`) |

Ces trois-là ne sont pas corrigés par ce chantier : ils exigent d'écrire dans le
moteur ou dans le substrat, où une **autre session** travaille en parallèle sur
le même arbre (voir §5).

---

## 3. Chaîne multi-dimension : elle était complète dans une seule face

Le maillon visé : `queryTextAtom` → `runQuery` (`api.ts:59`) → `queryResultAtom`
(`store.ts:36`) → `projectAxes` (`projection.ts:39`) → panneau `Projection` →
clic d'un groupe → `ancrerAtom` → `centersAtom` → rechargement du graphe.

Mesuré à l'écran (sonde CDP, service 8432) :

- au chargement, `.inspector` et `.proj` **absents du DOM** — le panneau n'est
  rendu que par `Inspector.tsx:168`, monté seulement si
  `montre("reglages") && ctlOpen`, et `inspectorOpenAtom` vaut **`false`** par
  défaut (`store.ts:41`) ;
- après « ▶ Projeter » : `.proj-ax = ["type","nbr"]` en 212 ms, 8 lignes ;
- **en face « 🧠 Monde » (la face par défaut), cliquer une ligne ancrable émet
  ZÉRO requête `/api/viz/subgraph`** — l'ancrage promis par le panneau
  (`Projection.tsx:105`) ne se produit pas. Cause : `centersAtom` /
  `requestTokenAtom` ne sont lus que par `Graph3D.tsx:25,31` et
  `CenterSearch.tsx:41`, tandis que la face par défaut monte `StageContinu`
  (`App.tsx:328-332`), qui a un centre **codé en dur** (`StageContinu.tsx:242-248`) ;
- en face « 🔺 Graphe » avec morph désactivé, le même clic émet bien
  `/api/viz/subgraph?…&center=…&hops=1&limit=160`.

### 1.3 Ce qui a été corrigé, et la mesure APRÈS

`scripts/protocole-check.mjs` sur l'instance isolée, binaire reconstruit :

```text
PASS  traversée /app/../../etc/hostname refusée            code=404
PASS  bundle servi en gzip                                 1307553 → 384463 o sur le fil (70.6 %)
PASS  Vary: Accept-Encoding présent
PASS  JSON d'API compressé                                 580 o sur le fil
PASS  2 requêtes pipelinées, 2 réponses                     réponses=2, 1re annonce : keep-alive
PASS  HEAD / → 200 sans corps                               Content-Length=440
PASS  HEAD sur route absente → 404
PASS  en-tête Date présent
PASS  405 porte Allow:                                      Allow: GET, HEAD, POST
PASS  ligne de requête vide → 400
PASS  HTTP/1.1 sans Host → 400
PASS  If-None-Match sur page / → 304                        corps=0 o
PASS  If-None-Match sur bundle haché → 304                   corps=0 o
PASS  page d'entrée revalidable (pas no-store)              Cache-Control: no-cache
PASS  /api/stores répond

PROTOCOLE : TOUT PASSE   (14 échecs sur 15 au départ)
```

Le transport est donc : **gzip négocié** (avec `Vary`, jamais sur
`text/event-stream`), **keep-alive borné** (nombre de requêtes ET fenêtre
d'inactivité), **délais de lecture/écriture posés** (le keep-alive n'est sûr que
parce qu'ils existent), **`HEAD` servi**, **`Date`** partout, **`ETag`/304**,
**`Allow`** sur les 405, et **400** au lieu d'un silence sur une trame illisible.
Côté SSE : `retry: 2000`, battement de cœur, `X-Accel-Buffering: no`, et la
fermeture VOULUE du flux (elle seule délimite la fin, faute de `Content-Length`
— et surtout pas de `chunked`, qui ferait pendre `scripts/chat-reprise-check.mjs`
jusqu'à ses 180 s).

### 1.4 Deux instruments faux, trouvés en vérifiant (et corrigés)

Ce chantier a produit deux mesures FAUSSES avant de produire des mesures
justes ; les deux sont écrites dans les scripts pour ne pas se répéter :

1. **Compter les réponses par une regex ancrée est faux.** Deux réponses
   servies sur la même connexion se suivent SANS séparateur : la ligne de
   statut de la seconde commence juste après le corps de la première, donc
   `^HTTP/1.1` en mode multiligne ne la voit pas. La sonde annonçait « 1
   réponse » quand le serveur en avait écrit deux — j'ai failli déclarer cassé
   un keep-alive qui marchait. Ce qui a tranché : la même requête, envoyée par
   une sonde Python à regex **non ancrée**, en comptait 2. Le compte se fait
   désormais en marchant de `Content-Length` en `Content-Length`.
2. **Mesurer la compression par `fetch` est faux.** `fetch` dégonfle lui-même
   dès que `Content-Encoding` est présent : `arrayBuffer().byteLength` rend la
   taille **d'après** décompression, identique à `identity`. On lit maintenant
   le `Content-Length` annoncé, seul chiffre qui décrit le fil (1 307 553 →
   384 463 o).

### 3.0 Protocole de mesure à l'écran (une leçon payée pendant ce chantier)

Mesurer l'interface **pendant** qu'un agent édite `src/` est invalide, et le
piège est silencieux : le service tourne avec `OBRAIN_DEV=1`, son veilleur
(`spawn_dev_watcher`, `viz_serve.rs:1727-1751`) relance `pnpm build` à chaque
changement, et le script de hot-reload injecté dans la page fait
`location.reload()`. Constaté en pleine sonde : la dernière trace réseau d'une
passe « clic sur un groupe » finissait par `/` puis `/favicon.ico` — **la page
s'était rechargée**, et l'état mesuré n'était plus celui qu'on croyait avoir
posé (projection vide au lieu des axes affichés un instant plus tôt).

Règle qui en découle, appliquée ici : les sondes d'interface se lancent

- soit contre l'**instance isolée** (`scripts/vizserve-isole.sh`, sans
  `OBRAIN_DEV`), dont la page ne se recharge pas toute seule ;
- soit quand l'arbre est **quiete** (aucun agent n'écrit).

Les mesures de protocole, elles, ne sont pas concernées : elles parlent HTTP.

Ce que la sonde a établi **avant** ce rechargement (passe valide, arbre encore
stable) : le panneau est désormais atteignable et rend ses axes avec une vraie
réponse moteur (`axes=["qn","nbr"]`, 40 groupes, une requête `/api/query`), mais
**le clic sur un groupe n'émettait toujours aucune requête de graphe** — le
maillon d'ancrage restait cassé à cet instant.

### 3.0 bis Ce que la vérification de la fratrie n'a PAS pu obtenir (et pourquoi)

Trois obstacles successifs, chacun corrigé sauf le dernier — les taire ferait
passer une limite d'instrument pour un constat sur le produit :

1. **Page blanche** : le veilleur reconstruit `dist/` à chaque écriture d'agent,
   et une page chargée pendant la réécriture reste vide. *Corrigé* : la sonde
   recharge jusqu'à ce que `#root` ait des enfants (mesuré : 1 chargement).
2. **Mauvais sélecteur de face** : la sonde cherchait le bouton « 🌌 Cosmos » dans
   seule l'entrée « ⚙Réglages » porte cette classe ; les faces sont des boutons
   nus. La sonde concluait « pas de bouton Cosmos » et mesurait une page où la
   scène ne pouvait pas être montée. *Corrigé* : la face Cosmos se monte,
   `window.__pickSweep` devient une fonction.
3. **Scène vide en headless** : une fois la face Cosmos montée, `__pickSweep(48)`
   ne trouve **aucune** cellule désignable, pendant 100 s (20 sondages), et le DOM
   porte un `stage-veil`. Sans cellule, il n'y a pas de nœud à cliquer — donc pas
   de panneau de fratrie à lire. **NON résolu** : c'est une limite de la sonde
   sans œil, pas un constat sur ce que voit un utilisateur.

Ce qui EST prouvé, par d'autres moyens : la règle canonique est vivante **dans la
page** (`parentDe('a.b.c.__file__') === "b"`, `window.__fratrie` présent avec ses
7 clés), la fratrie se calcule sur des **données de graphe réelles** via le
module du produit (nœud `setFromMatrix4` → 69 frères du dossier `vendor`,
69 co-fichiers `three.module.js`, index de 149 nœuds en **1,9 ms**, 434 relations),
et une puce de filtre qui était structurellement morte **allume désormais ses
1 964 nœuds sur 11 078** (clic réel, mesuré par `scripts/fratrie-check.mjs`).

### 3.0 ter Où la fratrie se voit, et le caveat qui reste

`scripts/fratrie-check.mjs` (porte de l'agent « frères et sœurs ») a été rejoué
**de première main** sur l'instance isolée : **1 contrôle échoue**, le même que
dans le rapport de l'agent, qui l'obtient vert sur 8432 — ce n'est pas une
contradiction, c'est une **configuration** :

- **Face Cosmos** : le panneau de fratrie s'affiche pour un nœud désigné —
  « frères dans ce monde : **6** ensembles (**127 216** nœuds) », clic → cadrage
  vérifié (`1,495 → 0,900`). Les frères y sont **nommés et cliquables**.
- **Face Graphe, barre de facettes** : Frères / Co-fichiers / Sœurs / Cycle /
  Homonymes avec effectifs exacts, « +N autres » qui se déplie (17 révélés pour
  17 annoncés), frères cliquables — mais cette barre ne se monte **que si
  « ⟲ morph » est désactivé** (`App.tsx:364`), donc après un **changement de mode**.
- **Face Monde (celle du départ)** : « même niveau : N familles » avec effectifs
  réels et puces sélectionnables (vérifié : `Field 748 · Function 500 · Enum 52 ·
  File 14 · Project 2`) — à cette échelle l'unité est l'ensemble, pas le nœud.

**Le caveat, dit franchement** : dans la configuration de départ (face Monde,
« ⟲ morph » actif), la fratrie **par nœud** n'est pas visible sans toucher à un
mode — il faut choisir la face Cosmos (un clic) ou désactiver « morph ». C'était le
seul point de la demande qui n'était pas satisfait « sans rien toucher ».

**CE CAVEAT EST FERMÉ depuis** (commit `26e72901`). La cause était exacte et
double : `App` exigeait `!monde` pour monter la barre de facettes, **et**
`Graph3D` n'est monté que si « morph » est inactif — dans l'état par défaut, aucun
des deux n'était vrai, donc la sélection ne pouvait jamais être posée. Or
`StageContinu` charge **déjà** un sous-graphe (`fetchSubgraph`) : il n'en publiait
rien. Il publie désormais le graphe réellement affiché et le nœud de son centre,
**seulement** pour l'ensemble que l'utilisateur a ouvert (`focusRef` — plusieurs
ensembles chargent en parallèle, publier le dernier arrivé ferait décrire à la
barre un graphe qui n'est pas à l'écran).

Preuve par clic réel dans l'**état par défaut**, nouvelle porte
`scripts/facetbar-monde.mjs` :

```text
PASS  au chargement, aucune barre (rien n'est encore désigné)
PASS  désigner un ensemble FAIT MONTER la barre de facettes   (clic en 936,288)
PASS  la barre DIT la fratrie (effectifs)
      « fratrie de processor.rs — dossier « query » : 4 frères · 4 co-fichiers ·
        4 sœurs · 0 dans le même cycle · 0 homonymes — sur 5 nœuds chargés
        (index en 1.6 ms, sans requête) »
```

La porte porte aussi sa propre leçon : cliquer une scène 3D à coordonnées **fixes**
est une loterie — deux passes ont donné « montée au 3ᵉ clic » puis « jamais », parce
que des appuis de touches envoyés AVANT les clics avaient déplacé la caméra. Les
clics passent donc d'abord, le clavier ensuite.

**Ce qui a réduit ce caveat depuis** : le panneau de la face Cosmos exigeait
`vise.genre === "noeud"`, c'est-à-dire avoir touché **un point de quelques pixels**
dans la nuée — mesuré comme quasi inatteignable (`CosmosWeave.tsx` : « Sans cette
seconde voie, le panneau restait inatteignable en pratique »). Il résout désormais,
à défaut d'un nœud visé, **le centre du graphe affiché** — c'est-à-dire le groupe
qu'on vient de désigner en cliquant une **boule**, le geste naturel. Vérifié sur
données réelles via le module du produit (`fratrie.ts`, aucune règle réécrite) :

```text
centre du graphe affiché : home.triviere.projects.aletheia.dist.static.assets.la
index : 149 nœuds · le CENTRE se résout (clé 304726) — sans viser un point
  nœud : la · dossier « assets » · fichier « _app-B-BS-bi-.js »
  Frères (dossier porteur)  71 · Co-fichiers 2 · Sœurs (communauté) 148 · Cycle 0
  total : 369 relations
```

Le script de la fratrie, lui, **écrit** l'état par défaut au lieu de le contourner :
« face « monde », « ⟲ morph » ACTIF — scène StageContinu, barre de facettes ABSENTE
— pourquoi : `App.tsx` ne monte `FacetBar` que si `!monde && !cosmos && selectedNode`,
et `Graph3D` n'est monté que si « morph » est INACTIF ». Un contrôle qui explique
pourquoi il échoue vaut mieux qu'un contrôle vert obtenu en changeant l'état
mesuré.

### 3.0 quater Une leçon de plus : un merge a mangé une édition non commitée

Le même jour, une session parallèle a fusionné des branches ; `docs/…-2026-09-13.md`
a été **ramené à sa version committée** dans l'arbre de travail, effaçant le
paragraphe ci-dessus (écrit quelques minutes plus tôt, non commité). Le contrôle
qui l'a montré, et qui vaut d'être retenu : `diff` entre la « sauvegarde » et le
fichier en place — **identiques**, donc la sauvegarde avait été prise APRÈS
l'écrasement et ne sauvegardait rien. **Copier n'est pas sauvegarder** : un
fichier recopié sans être relu ni committé ne protège rien, et l'annonce « c'est
sauvegardé » était fausse. Les correctifs de code, eux, étaient commités
(`ff676131`) et ont survécu à la fusion — vérifié par rejeu des portes (§7).

### 3.1 Deux défauts de ralentissement, mesurés sur la fonction réelle
`projectAxes` compilé et exécuté sur des réponses de forme réelle :

| lignes | temps | résultat |
|---|---|---|
| 8 000 | 7,4 ms | 2 axes, 8 000 items construits |
| 50 000 | 29,1 ms | idem |
| **125 258** | **`RangeError: Maximum call stack size exceeded`** | `Math.min(...vals)` (`projection.ts:53-54`) |
| 300 000 (sans colonne-mesure) | **175,3 ms de gel** | 300 000 items construits pour **16 affichés** |

`/api/query` **n'a aucune borne de lignes** (`viz_serve.rs:1847-1856`), et le
corpus servi fait 412 773 nœuds : le seuil est donc atteignable. Le même défaut
existe à `CosmosWeave.tsx:2419`, où la phase « complet » demande 200 000 nœuds
(`:2206-2211`) et où l'échec est **avalé en silence** par le `.catch` de `:2500`.

Le plafond de 16 (`Projection.tsx:66`) est un plafond d'**affichage** : rien ne
dit combien de groupes sont masqués. La longue traîne — l'immensité — était
invisible *et* tue.

---

## 4. Frères et sœurs : la parenté existait, en deux règles incompatibles

- `CosmosWeave.tsx:2446-2451` dérive le parent par `parts[parts.length-2]`, sans
  filtrer `__file__` ;
- `CosmosWeave.tsx:662-666` filtre `__file__` **puis** prend `src[src.length-2]`.

Mesuré sur 19 725 qn réels (cache de requêtes du serveur) : **les deux règles
divergent sur la TOTALITÉ des 15 106 qn portant `__file__`**. Comme le filtre
« ressortir » compare `infos[i].type === filtre` (`:3781`), cette puce **ne peut
structurellement jamais rien allumer** pour ces nœuds.

Ce qu'une sélection montrait : les arêtes `CALLS` **présentes dans le
sous-graphe chargé** (~160 nœuds), tronquées à 14 lignes (`FacetBar.tsx:77`), et
seulement en face Graphe avec morph désactivé (`App.tsx:343`). Tout le reste —
frères de dossier, sœurs de communauté, co-fichiers, co-modules — disparaissait,
alors que la charge utile les porte déjà : `file_path` et `scc_id` sont dans
`/api/viz/subgraph` (`api.ts:5-6`) et **jamais lus** dans `CosmosWeave.tsx`.

Coût mesuré d'un index de parenté construit en un parcours : **160 nœuds 0,4 ms ·
8 000 → 8,9 ms · 200 000 → 134 ms**.

---

## 7. Revérification APRÈS une fusion d'une autre session

Une fusion peut garder le texte d'un côté et **casser** ce qui marchait : la
seule preuve qui vaille est de **rejouer les portes sur le binaire fusionné**,
pas de relire le diff. Fait après résolution (binaire reconstruit, `cargo`
1 min 43, `tsc=0`) :

```text
scripts/protocole-check.mjs  → PROTOCOLE : TOUT PASSE        (15/15)
scripts/projection-check.mjs → PASS — projection multi-axes   (16/16)
arcs, 3 tops NEUFS           → 0,74 ms · 0,69 ms · 0,97 ms    (4,1-6,3 s avant, par top neuf)
```

Contrôle de **contenu**, pas seulement de comportement : `GzEncoder`,
`keep-alive`, `etag_de`, `rang_memo`, `cached_ro`, `elaguer_cache_disque` sont
tous présents dans `viz_serve.rs` après résolution — la fusion a gardé **les deux
côtés** (protocole + vitesses d'ici, IAM d'ailleurs).

## 8. Ce que ce chantier ne fait pas, et pourquoi

- **Les registres moteur** `label → count` et `type d'arête → count` (§2.1) :
  gain le plus large (105-216 ms et 545-563 ms **par exécution**, sur des routes
  appelées par `modules`, `space`, `arcs`, `signatures`, `weave`), mais ils
  touchent `obrain-engine`/`obrain-substrate` — précisément les crates qu'une
  **autre session édite en ce moment** dans cet arbre (travail non commité sur
  `props_trigram_index.rs`, `store.rs`). Les modifier sous ses pieds aurait
  produit le défaut que ce dépôt documente déjà : « un gain mesuré peut
  disparaître sans bruit » (`docs/PERF-REQUETES-LENTES-2026-09-13.md` §5).
- **HTTP/2** : voir §1.2 (frontal, pas ce binaire).
- **`fetchLabelPairs`** : la requête est fausse et coûte cher ; la réécrire change
  la donnée affichée en mode « familles » — décision à prendre, pas correctif à
  appliquer à l'aveugle. **Coût re-mesuré sur le binaire fusionné** (nonces neufs,
  donc exécutions réelles — un texte de requête identique relirait le cache) :

  | requête | temps |
  |---|---|
  | `labels(a)[0], labels(b)[0]` GROUP BY (la FAUSSE, `api.ts:285`) | **1,28 s / 1,27 s** |
  | marges sur `labels(a)[0]` SEUL (la forme fiable, celle de `obrain viz weave`) | **0,91 s** |

  Le levier est donc double : la fausse est **~370 ms plus chère** *et* elle
  recopie le label de la cible dans celui de la source (`Turn-[HAS_STEP]->Step`
  devient `Step,Step`, vérifié sur trois stores). Attention : la forme par marges
  est fiable mais ne rend pas la MÊME chose — elle donne les poids par label, pas
  les poids **par couple** de labels. Obtenir les vrais couples demande une
  requête **ancrée par label** (`MATCH (a:Message)-[r]->(b) RETURN labels(b)[0]`,
  N requêtes pour N labels), c'est-à-dire une décision de conception, pas une
  substitution de texte. Le cache client (15 min, `StoreOverview.tsx:277`) borne
  le coût en attendant.
