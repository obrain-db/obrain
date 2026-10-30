# Le chat comme agent sur le graphe — état, mesures, et ce qui reste

Date : 2026-09-11 · Portée : le chat du dashboard (`packages/obrain-dashboard/src/graph/ChatPleinEcran.tsx`)
et sa chaîne serveur (`crates/obrain-cli/src/{viz_serve,engine,ancrage}.rs`).

Ce document dit **ce qui est livré et vérifié**, puis **ce qui reste** — avec, pour
chaque reste, la preuve qui le fonde et la décision d'architecture qu'il exige.
Rien n'y est estimé : chaque chiffre a été mesuré sur le serveur en service
(`obrain viz serve`, 12 mondes, llama-server local).

---

## 1 · Livré et vérifié (2026-09-11)

### 1.1 L'ancrage ne peut plus mentir

Le défaut, reproduit : le chat affichait « ancré sur `this-rs/this` » et lisait
**runtime-dev**. La réponse citait `dsh-runtime`, `session-c254ac35…:msg:7`,
`:step:1:1`, `file:/tmp/codebase-memory-mcp/README.md` — les 24 premiers nœuds de
runtime-dev.

Deux causes, toutes deux côté vue :

1. `CosmosWeave.tsx` comparait `focus.default.world` — un **libellé**
   (« Runtime dev (engrams) ») — au `qn` des mondes (`world:runtime-dev-engrams`) :
   jamais égal, donc la résolution échouait **à tous les coups** ;
2. le repli était `weave.worlds[0].path` : la conversation était câblée sur le
   **premier monde de la liste**.

S'y ajoutait une cause plus profonde : **deux règles de slug divergentes** dans
`viz_serve.rs`. `viz_weave::slug` écrase les tirets consécutifs, la règle en
ligne d'`api_viz_weave` ne les écrasait pas. Pour « this-rs/this (GitHub) » :
`this-rs-this-github` contre `this-rs-this--github` — deux qn pour un seul monde.

Corrigé par `crates/obrain-cli/src/ancrage.rs` : la résolution s'appuie sur la
**base synaptique** (`World.path`), l'autorité du Cosmos, avec repli par clé
canonique pour un monde ajouté à chaud (non tissé). Un monde inconnu ne retombe
sur **aucun autre**. L'ancrage résolu repart vers l'écran et l'en-tête l'affiche.

### 1.2 La densification est un échantillon choisi, et la part lue est dite

Le défaut mesuré : `MATCH (n) RETURN n.qualified_name LIMIT 24` **sans
`ORDER BY`** — l'ordre de stockage. Sur `this-rs/this`, 22 titres de
`.github/ISSUE_TEMPLATE/`, aucune fonction. Sur runtime-dev, 21 des 24 nœuds
appartenaient à une seule session.

Ce qui a été retenu, et **pourquoi** (coûts mesurés) :

| axe | coût | verdict |
|---|---|---|
| compter nœuds/arêtes | 0,04 ms | toujours |
| échantillon par label | 0,3 – 12 ms | retenu |
| arêtes réelles par type | 5 – 70 ms | retenu |
| histogramme des labels | 160 ms (372 k) · 675 ms (1,5 M) | mémorisé |
| histogramme des types d'arêtes | **2,3 s (372 k) · 3,5 s (1,5 M)** | mémorisé |
| hubs par degré | **14,1 s (372 k) · 14,3 s (1,5 M)** | **exclu** |

Le classement par degré est donc **impossible** dans le chemin du chat : on ne
prétend pas montrer le « cœur » du graphe. On montre un échantillon représentatif
par type et des arêtes réelles, et on annonce « 32 / 4685 nœuds (0,7 %) ».

Deux leçons de mesure valent d'être conservées :

- **`n.name` n'est pas indexé** (sur ces stores). `MATCH (n) WHERE n.name = 'x'` coûte 4 981 ms à
  vide sur runtime-dev, et 91 ms quand le nœud est en tête de parcours — un
  `name` qui répond vite ne prouve donc rien sur le `name` qui ne répond pas.
  Seul `qualified_name` (index d'égalité) est bon marché : 0,6 ms.
- **Un store est un instantané, mais son répertoire ne le dit pas.** Le mtime du
  chemin est celui du répertoire : il ne change pas si un fichier existant est
  réécrit. La carte du store est donc validée par le **maximum des mtime des
  fichiers** (`empreinte_store`), pas par `store_mtime`.

### 1.3 Mesures

| cas | avant | après (froid) | après (chaud) |
|---|---|---|---|
| **monde `this-rs/this`** (4 685 nœuds) | **3 141 ms — et le mauvais store** | 253 ms | **0–1 ms** |
| monde tissé `linux-full3` (1,5 M) | — | 5 040 ms | mémorisé |
| **graphe global** (385 k nœuds) | ~1 855 ms | **468 ms** | **1 à 13 ms** |
| **sujet absent du store** | **16 037 ms** | 14 ms | **2 ms** |
| sujet présent, voisinage 1 saut | ~3 400 ms | 1 091 ms | — |

Le store vivant (`runtime-dev`) est **écrit en permanence** par le harness. Lire
sa fraîcheur sur le **manifeste** (`substrate.meta`) — et non sur ses fichiers —
est ce qui permet à sa carte d'être réellement réutilisée : sur le maximum des
mtime des fichiers, le journal d'activation invalidait la carte à chaque
écriture, et le chat repayait son contexte à chaque tour (mesuré : 10 899 ms à
chaque appel au lieu de 468 ms une fois). Une seule règle de fraîcheur dans le
serveur, et c'est la bonne : « ce que j'ai calculé est-il encore vrai ? »

### 1.4 Vérification

`scripts/chat-ancrage-check.mjs` exerce le **geste de l'utilisateur** : Cosmos →
mondes parcourus **au clavier** ←/→ jusqu'à `this-rs/this` (monde ajouté à chaud,
donc absent de la base synaptique — précisément le cas qui échouait) → ouverture
du chat par le bouton flottant → saisie → envoi → lecture de l'ancrage affiché.
Trois passes : en-tête « ancré sur this-rs/this · lu dans this-rs-this.obrain »,
contexte « 32 / 4685 nœuds (0.7 %) · 10 relations · lu en 0 ms », self
`self:triviere-dsh`, **0 erreur console**.

Un contrôle par appel d'API n'aurait pas vu le défaut : sa cause était dans la vue.

---

## 2 · LIVRÉ (2026-09-11, second tour) — le chat interroge la base

### 2.0 La boucle d'outils, et les autres graphes

Six outils exposés au modèle, exécutés contre la base et **visibles à l'écran**
(chaque appel est poussé en SSE : nom, arguments, monde, lignes, durée, erreur) :
`obrain_query`, `obrain_echantillon`, `obrain_voisinage`, `obrain_mondes`,
`obrain_recettes`, `obrain_recette`. Mesuré : `obrain_echantillon` 20 lignes en
0-2 ms ; `obrain_recettes` 29 lignes en 1 ms.

Garde-fous, tous nés d'une mesure : écriture refusée avant envoi (mot entier),
`LIMIT` imposé en l'absence de borne (un `MATCH (n)--(m) …` coûte 14,1 s),
résultats bornés à 2 000 caractères avec la coupe **écrite**, 4 tours maximum
avec arrêt automatique avant de franchir la fenêtre de 8 192 jetons.

**Le référentiel de l'agent n'est pas dans le graphe regardé** (29 recettes dans
`self-demo`, zéro dans runtime-dev / runtime-prod) : l'atlas permet de le
trouver, et l'outil **dit le monde réellement lu**. Vérifié : à une question sur
le tissage posée depuis `this-rs/this`, le référentiel a été lu dans
`Self-demo (engrams)` en 1 ms, et la réponse cite les vraies familles et les
vrais noms.

### 2.1 La conversation STRUCTURE — et le Self conserve (§ axis 5, livré)

Le chat **journalise** ce qu'il structure (`obrain_memoriser`, `obrain_structurer`)
et `scripts/memoire.sh` **applique** en `MERGE` idempotent dans la base du Self.

Pourquoi deux temps : le substrat impose l'invariant mono-écrivain (lecture =
`flock` partagé, écriture = exclusif, promotion refusée). Le serveur tient ses
bases en lecture **par conception** — écrire par lui serait violer l'invariant,
pas le contourner. Le journal est durable immédiatement ; l'application exige
l'exclusif, donc l'arrêt du service, exactement comme `scripts/weave.sh`.

Quatre gestes, sur des entités qui existent DÉJÀ (`Note`+`HAS_NOTE`,
`Goal`+`HAS_GOAL`, `Procedure`+`MASTERS`, `SelfState` versionné+`HAS_STATE`),
chacune avec sa **provenance** (monde + store).

Mesuré : 4 notes écrites par la conversation → appliquées → reliées au Self, avec
provenance ; journal consommé ; **2 entrées identiques → 1 nœud** (idempotence) ;
`SelfState` versionné est la seule écriture non idempotente, et c'est pourquoi le
journal est consommé. À l'écran : « mémorisé (note) : … · journalisé, **en
attente d'application** ».

### 2.1bis Trois défauts de budget, tous mesurés

1. le coût des définitions d'outils (~1 000 jetons/tour) n'était pas réservé →
   « Context size has been exceeded » au tour suivant le premier appel ;
2. ratio d'estimation 3,2 car/jeton **trop optimiste** → ramené à **2,6** ;
3. le contrôle de fenêtre **retirait les outils au lieu de rogner la sortie**, et
   se déclenchait dès le premier tour avec le `max_tokens` par défaut du client
   (4 096) : « retiens ceci » n'appelait donc aucun outil dans le navigateur alors
   qu'un test CLI à `max_tokens=700` passait — même code, deux tailles de sortie.

### 2.2 bis Les deux défauts que la mesure a révélés

1. **Le type manquait sur les nœuds cités.** « Montre-moi 20 Method » → des noms
   réels dont **trois sur six étaient un `Function`, un `Field`, une `Variable`**.
   `labels(a)[0]` est lu avec chaque arête (coût nul) et écrit entre crochets.
2. **Le modèle inventait quand il ne savait pas.** Même question que ci-dessus,
   `tool_choice: auto` : aucun outil appelé, et une réponse affirmant que les
   recettes sont « encapsulées dans `build.main` » — invention pure.
   `exige_un_outil()` impose `tool_choice: "required"` au premier tour dès qu'un
   mot significatif de la question est absent du contexte. Même question rejouée :
   29 recettes réelles, réponse exacte.

---

## 3 · Ce qui reste, avec la preuve qui le fonde

### 3.1 ~~Le chat n'interroge pas la base~~ — **fait** (§ 2.0)

Aujourd'hui : un contexte figé au temps t0, un seul aller-retour vers le modèle.
Le modèle ne **peut pas** demander la suite ; il conclut sur 0,7 % de la matière.
C'est la limite structurelle qui reste après la densification : on a amélioré
*quoi* est lu au premier tour, pas la capacité d'en lire davantage.

**Deux faits établis le 2026-09-11 rendent la suite faisable :**

1. **Le tool-calling natif fonctionne** sur le service local. Requête de contrôle
   envoyée à `POST /v1/chat/completions` avec un outil `obrain_query` :
   réponse `finish_reason: "tool_calls"` et arguments bien formés
   (`{"store":"default","query":"MATCH (n) RETURN count(n)"}`).
2. **Le référentiel existe déjà dans la base.** Le store `self-demo` contient
   **32 `Procedure`** avec un `body` GQL exécutable, classé en familles :

   | famille | exemples réels |
   |---|---|
   | `F1-self` | `ref.self.boot` · `ref.self.goals` · `ref.self.notes` · `ref.self.recettes` · `ref.self.procedures` |
   | `F2-code` | `ref.code.find` · `ref.code.trace` · `ref.code.impact` · `ref.code.arch` · `ref.code.hot` |
   | `F3-runtime` | `ref.runtime.recent` · `ref.runtime.load` · `ref.runtime.touches` · `ref.runtime.weave` · `ref.runtime.signals` · `ref.runtime.goals` · `ref.runtime.self` |

   Exemple de corps réel : `proc:ref.runtime.weave` →
   `MATCH (w:Weave) WHERE w.qualified_name STARTS WITH 'weave:<session_id>' …`.
   Le `SelfState` v8 dit lui-même : « Éprouver le référentiel en conditions
   réelles ».

**Décision proposée** : exposer comme outils `obrain_query` (GQL borné en
lignes), `obrain_echantillon(label, n)`, `obrain_voisinage(qn)`, `obrain_recettes`
/ `obrain_recette(nom)` — et boucler (max ~6 tours) en poussant chaque appel au
client (`event: outil`), pour que le travail soit **visible**. Contrainte dure :
`n_ctx = 8192` **jetons au total** — les définitions d'outils et les résultats
doivent donc être bornés, et la boucle plafonnée.

### 3.2 ~~Le chat ignore les autres mondes et leurs synapses~~ — **partiellement fait** (§ 2.0 : atlas, `obrain_mondes`, monde visé par outil ; reste à faire : traversée automatique d'une synapse)

Il ne connaît qu'un store. Or la base synaptique de l'agent est servie et peuplée :
`agent-synapse-triviere-dsh.obrain`, **12 mondes, 12 fibres, 2 347 ancres**
(`World {qn,label,path}` · `Anchor` · `SYNAPSE` · `Self-[:OWNS]->World`).

Les fibres portent déjà leurs **clés de jointure réelles** :
`world:runtime-dev-engrams ↔ world:self-demo-engrams` a pour `sample`
`["skill:codebase-memory-first", "session-842672aa…", "session-77370046…"]`, et
219 ponts de vécu datés relient `world:obrain-rust ↔ world:runtime-dev-engrams`.

**Limite honnête** : le moteur ne sait pas joindre deux stores. Une traversée
`monde A → fibre → monde B` se fait donc **côté serveur**, en lisant A puis B et
en joignant en mémoire par les `Anchor` partagés. C'est faisable et borné ; ce
n'est pas une requête du moteur, et il ne faut pas le présenter comme telle.

À noter : **6 mondes sur 12 ne sont pas tissés** (les dépôts GitHub ajoutés à
chaud) — ce sont exactement ceux pour lesquels la résolution par slug canonique
est nécessaire (voir § 1.1).

### 3.3 ~~La conversation ne laisse aucune trace~~ — **fait** (§ 2.1bis)

`demander_stream` n'écrit **que** le journal d'activation (`activation::record`) :
la conversation ne produit ni note, ni objectif, ni état. Or les mécanismes
existent déjà côté MCP : `self/consolidate`, `tisser()`, `SelfState` versionné
relié par `HAS_STATE`, `proc:self.remember` (`cls: write`).

Deux obstacles réels, et une décision à prendre :

1. **`viz_serve` ouvre les stores en lecture seule** (`cached_ro`), et le writer
   unique est `import_events` côté MCP. Écrire depuis le dashboard exige soit un
   **journal d'écriture append-only rejoué par le writer** (la même doctrine que
   `activation`), soit un endpoint d'écriture dédié. La première option respecte
   le writer unique et survit aux redémarrages.
2. **Le store est abîmé sur disque** (30 191 décodages de propriétés en échec sur
   241 nœuds, `docs/ETAT-COSMOS-2026-09-10.md` § 3.4) : toute écriture doit être
   `MERGE` idempotent **et vérifiée après coup**.

Enfin, l'identité du self **diverge** aujourd'hui :

| source | valeur |
|---|---|
| base synaptique (`/api/viz/weave`) | `self:triviere-dsh` |
| `self-demo` (nœuds `Self`) | `self:triviere-dsh` |
| défaut du chemin MCP (`OBRAIN_SELF_QN`) | `self:agent` (mcp_server.rs:2678) |
| documentation ancienne | `self:agent:state:v72` |

Trois identités, c'est trois mémoires séparées. L'ancrage du chat porte désormais
`self_qn`, **résolu depuis la base synaptique** (donc `self:triviere-dsh`), et il
est affiché dans le bandeau de contexte. Unifier le défaut MCP est le préalable à
toute écriture de mémoire.

---

## 4 · La fenêtre du modèle — mesurée, et désormais découverte (2026-09-12)

Tous les budgets étaient des constantes calibrées pour **8 192** jetons ; le
service en sert **32 768**. Le chat tronquait donc sa matière d'un facteur quatre,
en silence — « densification » comprise.

Mesures : `/props` → 32 768 · `/slots` → 32 768 · test empirique → **accepté** à
25 258 jetons, **refusé** à 46 205 (« exceeds the available context size »). La
capacité du MODÈLE (256 k annoncés) n'est **pas lisible d'ici** : métadonnées
`/v1/models` vides, pas d'accès SSH, GGUF distant. C'est un réglage du service
(`--ctx-size`), et le chat s'y adaptera seul.

Budgets désormais dérivés : 8 192 → 60 nœuds / 12 000 car. / 4 tours ;
32 768 → 200 nœuds / 34 078 car. / 6 tours / 8 000 car. par résultat d'outil.
La fenêtre et sa provenance sont **affichées** dans le bandeau.

**Vérifié en direct sur les deux services** : le 9 B local → **8 192** (source
« défaut du logiciel », ce service n'annonce pas `n_ctx` — et 8 192 est bien son
`--ctx-size`) ; le 27 B distant → **32 768**, source « annoncée par le service »,
et le contexte passe à **74 nœuds · 21 arêtes · 13 types** (contre 32 · 10 · 8),
en 78 ms.

En chemin, un défaut du **substrat** : quand un autre processus tient une base en
écriture, une ouverture en lecture peut BLOQUER au lieu d'échouer (cinq threads
sur un `futex`, pendant que `/api/query` répondait). Contourné dans mon périmètre
(`cached_ro_si_present`, replis d'`ancrage`, préchauffage de la base synaptique),
pas corrigé — le fichier est partagé.

## 5 · La traversée d'une synapse, automatique (2026-09-12)

`obrain_traverser(cle)` : dit dans quels mondes une clé partagée vit et **ce
qu'elle Y est**, en lisant les `Anchor` et leurs `IN_WORLD` dans la base
synaptique (repli : échantillons des fibres ; la source utilisée est écrite).
Mesuré : `skill:codebase-memory-first` → 3 mondes en 18 ms.

Le self du chat est unifié sur `self:triviere-dsh` (résolu par la base
synaptique), pour le bandeau comme pour l'écriture.

Densité atteinte sur `runtime-dev` (401 216 nœuds) : **112 nœuds · 25 arêtes** en
928 ms (contre 32 · 10 avant).

**CORRECTION de ce paragraphe** : j'avais écrit que l'entretien excluait les
lecteurs. C'est FAUX — il écrit via le **même handle** `cached_ro`, donc il ne les
exclut jamais. Ce qui bloquait était mon propre défaut (une seconde carte de
handles, vide). Vérifié entretien ACTIVÉ : `8/8 monde(s) enrichi(s)` et le chat
lit 112 nœuds · 25 arêtes en 127 ms.

## 6 · L'entretien comme régime (pleine cognition)

La boucle d'entretien faisait un `continue` silencieux sur les mondes qu'elle
n'arrivait pas à ouvrir : ils n'étaient jamais enrichis, et rien ne le disait.
Elle persévère désormais, **nomme ses échecs** et **rend un compte par passe**
(`entretien — 8/8 monde(s) enrichi(s)`), et vérifie en plus la matérialisation des
registres de degrés par type.

Manques pour une cognition pleine, identifiés et non faits :

- **degrés non interrogeables en GQL** (`n.deg_out` = 0 ; registres par type lus
  par l'API Rust seulement) → le chat ne peut pas classer les hubs par degré et
  retombe sur un parcours à 14,1 s qu'il refuse. Addition de moteur.
- **couche sémantique absente** (`vector-index` non compilé, `n.embedding` NULL
  partout) → pas de recherche sémantique. Le plus gros manque.

## 7 · Les hubs : 14,1 s → 0-2 ms, et un refus quand la donnée est fausse

Le classement par degré se faisait par parcours complet : **14,1 s** sur
`runtime-dev`, 14,3 s sur `linux-full3`. Remplacé par une lecture des **registres
de degrés** (`substrate.degrees.node.<TYPE>.u32`, O(1) par nœud), calculée par
l'entretien et **persistée** : `this-rs/this` 40 hubs en **1 ms**,
`project-orchestrator` 40 en **2 ms**.

**Mais** les registres de `runtime-dev` sont faux : **48 253 968** arêtes
sortantes annoncées pour un nœud qui en a **28 368**, dans un graphe qui en compte
**1 243 111**. Un contrôle évident (un degré ≤ le nombre d'arêtes) attrape le cas,
et le classement est alors **REFUSÉ avec sa raison** — persisté et transmis au
chat, à l'outil et à l'interface. Servi : 6 mondes sur 8 ; refusés : `runtime-dev`
(280 nœuds impossibles), `runtime-prod` (8).

**Correctif de fond dans le substrat** : `DegreeColumn::open` ne validait que
l'INTÉGRITÉ (magic/version/CRC), jamais la COHÉRENCE avec le graphe — une colonne
intacte pouvait être fausse. Les colonnes typées avaient leur contrôle depuis le
2026-09-05, la générique non. Ajoutés : `incoherence_colonne_degres` (invariant
« somme des degrés = arêtes vivantes », avec distinction impossible/inexact),
`verifier_coherence_degres_une_fois` (une fois par store et par processus, ~1 ms)
et `reparer_colonne_degres_en_place` (remise à zéro + un rescan). En **lecture
seule**, on ne répare pas (une base lue ne se modifie pas) : on le journalise avec
la commande. Le dashboard est en lecture seule, donc il continue de refuser en le
disant ; la réparation s'applique aux ouvertures inscriptibles et à la demande
(`obrain repair-degrees`, service arrêté).

Trois tests l'ancrent : détection, réparation à la réouverture, et non-réparation
d'une colonne juste.

## 8 · Ce qui reste

Les cinq axes de la demande sont couverts. Restent deux dettes précises :

1. **Unifier le self** : l'écriture est ancrée sur `self:triviere-dsh` (résolu par
   la base synaptique), mais le défaut de l'ancien chemin MCP reste `self:agent`.
   Un seul Self pour un seul agent.
2. **Traversée automatique d'une synapse** : `obrain_mondes` fournit déjà les clés
   partagées ; reste à en faire un outil qui enchaîne la requête dans l'autre
   graphe sans recopier les clés.

Et une limite connue, à ne pas oublier : le modèle **n'est pas constant** sur
l'appel d'outil. `exige_un_outil` (mot significatif absent du contexte ⇒
`tool_choice: "required"`) couvre le cas mesuré de l'invention, mais un modèle qui
répond juste sans lire reste possible sur une question entièrement couverte par
le contexte — ce qui est alors acceptable.
