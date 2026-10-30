# ADR — Cosmos : système solaire, toile de l'univers, base synaptique durable de l'agent

Date : 2026-09-10 · Statut : **accepté**, implémenté (serveur + dashboard)

## Décision

La vue **Cosmos** du dashboard n'est plus un semis de bulles reliées par des
similarités de types. Elle devient un **système solaire** — un foyer central
autour duquel des mondes orbitent — doublé d'une **toile de l'univers qui se
tisse** entre ces mondes. Trois exigences de l'utilisateur gouvernent le tout :

1. « des attractions, des orbites, le sol n'est pas fixe, il s'affaisse à la masse » ;
2. « un tissage synaptique qui reproduit **fidèlement** la densification
   synaptique pendant qu'il se développe pour connecter des neurones » ;
3. « chaque monde est une base de connaissance qui s'inter-connecte via les
   synapses ; cette base synaptique est **celle de l'agent**, elle doit être
   **durable et persistante** ».

## 1 · Le centre n'est pas fixe

Le « Soleil » n'est pas une identité figée (ni le Self, ni le moteur, ni la
base active) : c'est le **foyer d'attention**, qui **dépend du contexte** —
l'instant, la conversation, l'agent, la question posée, le sujet, l'objectif,
l'utilisateur. C'est une réponse de l'utilisateur, et elle est implémentée telle
quelle : `/api/viz/weave` renvoie des **foyers candidats réels**, lus dans les
mondes (pas de liste décorative) :

| Foyer | Source réelle mesurée |
|---|---|
| `conversation` | `Session` de `createdAt` maximal (ex. « Boot + auto-structuration du Self ») |
| `agent` | nœud `Self` dont le `qualified_name` commence par `self:` |
| `utilisateur` | `Account` + `Session-[:OWNED_BY]->Account` |
| `objectif` | `Goal` portant un nom lisible (« Structuration continue du Self ») |
| `instant` | l'horloge courante — la gravité suit la récence |
| `sujet` | sujet/question libre (recherche) |

Changer de foyer change **ce qui attire** : si le foyer est un monde, les autres
mondes se rapprochent selon le **poids réel de leur fibre** avec lui
(`CosmosWeave.tsx::layout`). L'attraction n'est jamais décorative.

## 2 · Le sol s'affaisse à la masse

La nappe d'espace-temps est une grille (`PlaneGeometry` 46×46) dont chaque
sommet descend selon la somme des puits gravitationnels :

```
sag(x,z) = − Σ  (150·masse_i + 40) / (1 + d²/(200²·(0.5 + masse_i)))
masse_i = log10(1 + nœuds_i) / max(log10(1 + nœuds))
```

La profondeur du puits colore la nappe (bleu → rouge) : la courbure se voit.
Les orbites sont **képlériennes** (ω ∝ a^−1.5) et chaque monde a son plan
incliné propre. Un monde **sans aucun lien** n'est pas caché : il reste en
périphérie, massif mais non tissé — c'est une information, pas un défaut.

## 3 · La toile se densifie **fidèlement**

Chaque fibre est fondée sur une **preuve réelle**, jamais sur une similarité
inventée. **Quatre preuves**, dans `crates/obrain-cli/src/viz_weave.rs` — et
elles ne sont jamais confondues à l'écran :

- **P1 · entité partagée** — même `qualified_name` dans ≥ 2 mondes.
- **P2 · pont de vécu** — `Session.cwd` tombe sous la racine d'un monde : une
  session **réellement exécutée** dans ce territoire tisse le monde qui l'a
  vécue au monde qui le décrit. **Datée** par `Session.createdAt`.
- **P3 · plan traversant** — entité du plan `self` (`self:`) ou `utilisateur`
  (`account:`) présente dans un monde de connaissance.
- **P4 · flux de types** *(ajouté après mesure)* — les deux mondes portent
  réellement **le même flux** (mêmes types reliés par la même relation), chacun
  avec son volume. C'est une **parenté de structure** : la preuve la plus
  faible. Elle est donc rendue **autrement** (trait fin, discontinu, froid) et
  comptée à part (`counts.flow_ties`) : une ressemblance de forme ne doit
  jamais se lire comme une connexion vécue.

Le **poids** d'une fibre = P1 + P2 (preuves fortes) ; P4 est exposé séparément
en `flows` et ne gonfle pas le poids. Un couple de mondes sans P1/P2 mais avec
P4 apparaît comme **parenté seule** — c'est ce qui relie enfin Linux et e1000
(6 flux communs) sans mentir sur leur nature.

La **densification** n'est pas une animation arbitraire : chaque fibre porte
`events: Vec<i64>` — les dates réelles de naissance de ses synapses. La courbe
(`densification.steps`, 48 paliers) rejoue le développement sur l'horloge réelle
de l'agent, et le curseur du dashboard interroge cette courbe.

### Mesures réelles du 2026-09-10 (6 mondes, `--per-label 400`)

```
5147 ancres lues · 2347 ancres tissées · 12 fibres · horloge 605 sessions
  dev ↔ prod              poids 2347  (2347 entités partagées)  258 datées · 19 flux
  obrain(rust) ↔ dev      poids  218  (218 ponts de vécu)       218 datées ·  5 flux
  obrain(rust) ↔ prod     poids  134  (134 ponts de vécu)       134 datées ·  6 flux
  dev ↔ self-demo         poids    3                             2 datées
  prod ↔ self-demo        poids    3                             2 datées
  7 fibres de PARENTÉ seule (linux↔prod 10 flux, linux↔dev 9, linux↔e1000 6, …)
densification : 0 % → 1 synapse · 25 % → 118 · 50 % → 241 · 100 % → 614

Signature de flux du self (marges réelles) :
  (Self)-[MASTERS]->(Procedure)   src=34 dst=32
  (Self)-[HAS_STATE]->(SelfState) src=8  dst=8
  (Self)-[HAS_GOAL]->(Goal)       src=3  dst=3
```

Deux faits mesurés ont orienté la conception :

- **le `cwd` des sessions est le seul pont réel** entre mondes vécus et mondes
  de code (dev ∩ obrain(rust) = **0** `qualified_name` commun, mais **218**
  sessions exécutées dans `banc-obrain`). Sans P2, Linux et Obrain(rust)
  restaient des masses isolées ;
- **`self:agent:state:v72` vit dans dev, prod *et* self** : le Self est déjà
  tissé aux mondes avant toute intervention.

## 3 bis · Ce que le moteur ne sait pas encore lire (bug mesuré)

La preuve P4 a failli être construite sur une **lecture fausse**. Trois formes
ont été comparées sur trois stores (dev, self-demo, linux) :

| forme | résultat |
|---|---|
| `RETURN labels(a)[0], count(r)` — **une seule** expression de label | **FIABLE partout** |
| `RETURN labels(a)[0], labels(b)[0], count(r)` — deux expressions | **FAUX** : la cible est recopiée dans la source |
| `MATCH (a:Label)-[r]->(b)` — ancre explicite | correct sur dev/linux, **vide** sur self-demo |

Preuves chiffrées (2026-09-10) :

```
dev   : réel Turn -[HAS_STEP]-> Step        agrégé 2 labels → Step,Step 49726
dev   : réel Message -[PROVES]-> Task       agrégé 2 labels → Task,Task 2360734
linux : réel Function -[CALLS]-> Function   agrégé 2 labels → Function,Function
self  : count(a:Procedure) = 32  MAIS  (a:Procedure)-[r]->(b) = ∅
```

Conséquences assumées :

1. le compte **joint** d'une paire de labels n'est pas lisible aujourd'hui. P4
   conserve donc les **marges** (`src_count`, `dst_count`) et une borne
   inférieure (`bound = min`), **nommées comme telles** — aucune paire n'est
   présentée comme mesurée conjointement ;
2. cette lecture par marges a **corrigé une conclusion antérieure** : le flux
   réel du self est `Self -[MASTERS]-> Procedure` (le Self tisse vers ses
   connaissances), et non `Procedure→Procedure` ;
3. **le dashboard est touché** : `fetchLabelPairs` (vue Monde) emploie la forme
   à deux expressions. `same[s]` y capte donc presque tout et `cross` est quasi
   vide. Avertissement posé en tête de fonction ; reprise à faire.

Rapport de bug à ouvrir côté moteur : agrégation `GROUP BY` sur deux expressions
`labels(...)` — les deux colonnes se lient au même emplacement ; et jointure
`(a:Label)-[r]->(b)` vide sur un store pourtant peuplé (self-demo), alors que le
même motif non ancré trouve les arêtes.

## 4 · La base synaptique est celle de l'agent : durable et persistante

Store obrain dédié, produit par `obrain viz weave` (et `scripts/weave.sh`) :

```text
(:Self {qn:'self:triviere-dsh'})
  -[:OWNS]->        (:World {qn,label,path,nature,nodes,edges,mass,gravity,orbit,types,flows})
  -[:WEAVES]->      (:Anchor {qn,label,first_seen,is_self,world_count})
(:Anchor)-[:IN_WORLD]->(:World)
(:World)-[:SYNAPSE {weight,anchors,shared,bridged,flows,flow_sample,dated,sample,
                    reinforcement,last_spike}]->(:World)
(:Densification {steps,t0,t1,sessions})
```

Durabilité **vérifiée** : la base survit aux redémarrages ; le serveur la relit
en RO (`source: "persisted"`, 2347 ancres, 5 fibres, horloge 605 sessions) et le
**renforcement s'accumule** à travers les processus (×7 après sept tissages —
`MERGE` + `SET`, jamais de réécriture).

Chemin : `~/.local/share/obrain-harness/data/agent-synapse-triviere-dsh.obrain`.

## 5 · Contrat HTTP

`GET /api/viz/weave?focus=<spec>` →
`{schema, available, source, self, worlds[{…,types,flows}],
fibers[{…,shared,bridged,flows,flow_sample}], anchors[], foci[],
focus, clock{t0,t1,sessions}, densification{steps[]}, counts, persisted}`.

`available:false` + `reason` quand la base n'est pas encore tissée : le
dashboard se replie alors sur l'ancienne projection (`CosmosWeave` rend
`StageContinu`) avec la raison affichée — jamais d'écran muet.

## 6 · Points durs rencontrés (mesurés, à ne pas refaire)

1. **`SET` : l'ordre compte.** Lire `r.reinforcement` *après* d'autres `SET` de
   la même clause renvoie `null` (binding périmé) : le compteur restait figé à 1.
   Isolé sur store neuf (1 → 2 → 3 uniquement si l'incrément est **en premier**).
   L'incrément se fait donc en tête de `SET`.
2. **`open_shared_write` ne suffit pas ici.** Les propriétés d'arête écrites
   après le dernier flush ne survivent pas à la réouverture (le moteur
   l'annonce : « husk signature — properties written after the last flush were
   lost »), donc le renforcement ne s'incrémentait pas. Tissage en ouverture
   **exclusive**, orchestré par `scripts/weave.sh` (arrêt propre → tissage →
   relance).
3. **`kill -9` sur le serveur ⇒ SIGBUS intermittent au tissage suivant**
   (reproduit : rc=135 au 1er passage, OK au suivant). Cause non élucidée.
   Atténué : `SIGTERM` + attente de sortie effective, `sync`, et **une reprise**
   si le tissage meurt par signal (le tissage est idempotent par `MERGE`).
4. **La propriété est `qualified_name`, pas `qn`.** Les foyers lisaient `qn` :
   ils étaient tous vides. Même piège pour `label` (les nœuds portent `name`).
5. **Lire un flux de types demande la bonne forme de requête.** Deux expressions
   `labels(...)` dans un `GROUP BY` se lient au même emplacement et recopient la
   cible dans la source : la première version de P4 rapportait
   `Function→Function` partout et un self en `Procedure→Procedure`. Voir §3 bis
   pour le tableau des trois formes — la seule sûre partout est **une seule**
   expression de label, agrégée seule.

## 7 · Vérification visuelle : possible, et elle a payé

L'outil navigateur de l'agent refuse les IP privées (`192.168.1.103`). Mais un
**Chrome local écoute en CDP** sur `127.0.0.1:9248` : `scripts/cosmos-capture.mjs`
ouvre un onglet, laisse la scène se construire, collecte console + exceptions +
WebGL, capture l'écran — puis une **seconde capture panneaux masqués**, seule
façon de mesurer le contenu 3D sans que le chrome d'interface ne contamine la
mesure (constaté : une boîte englobante « qui touche le bord bas » n'était que
du texte de panneau).

Ce que cette vérification a immédiatement attrapé, et qu'aucun contrôle
serveur ne pouvait voir :

1. **La vue Cosmos était inatteignable.** `⟲ morph` est actif par défaut et sa
   branche était évaluée AVANT celle du cosmos : l'utilisateur voyait l'ancienne
   projection « même substrat ». Le Cosmos passe désormais en premier.
2. **La toile était sous-pixel.** Avec `sizeAttenuation`, la taille d'un point
   est en unités-monde : à ~1600 unités de recul, les 2,6 unités des synapses
   valaient **1,2 px**. Mesuré après correction : pixels lumineux **2,16 % →
   6,02 %**, couronne d'orbites 24,5 → 39,0 de luminance, nappe 24,0 → 31,4.
3. **Le brouillard noyait les mondes** (plage 1400→3400 pour des objets à
   1500-2200 du point de vue) : repoussé à 2600→5400.

État vérifié après correction : contenu de la scène nue **18,7 %**, pixels
lumineux **4,18 %**, centre de lumière à **0,7 % / 13 %** du centre du cadre,
aucune erreur console ni WebGL (`SwiftShader`). Le script échoue si la scène
devient quasi vide (< 6 % de contenu), si plus rien n'est lumineux, ou si le
foyer sort du cadre — une scène blanche ne produit aucune erreur JS et passerait
tous les autres contrôles sans cette mesure.

## 7 bis · Limites assumées

- **Le jugement esthétique reste à l'utilisateur** : ce qui est vérifié est
  « la scène se construit, elle est peuplée, le foyer est centré, rien ne
  plante » — pas « c'est beau ni conforme à l'intention ».
- **P2 dépend de la racine déclarée** (`--world "Nom=chemin#racine"`). Sans
  racine, un monde ne reçoit que les preuves P1/P3/P4.
- **P4 est une parenté de forme, pas une connexion.** Elle relie désormais
  Linux/e1000 au reste (7 fibres de parenté) — c'est réel, c'est faible, et
  c'est rendu comme tel.
- **Le compte joint d'une paire de labels reste illisible** (bug §3 bis) : P4
  expose des marges et une borne, jamais un compte de paire.
- `reinforcement` compte les **tissages**, pas une pertinence sémantique.
- Coût mesuré : tissage complet ≈ 55 s (les 10 relations principales de chaque
  monde sont interrogées deux fois), contre 36 s sans P4.

## 8 · Fichiers

- `crates/obrain-cli/src/viz_weave.rs` — preuves, agrégation, persistance, relecture.
- `crates/obrain-cli/src/viz_serve.rs` — `/api/viz/weave`, foyers contextuels.
- `crates/obrain-cli/src/commands/viz.rs` — sous-commande `viz weave`, `--synapse-store`.
- `packages/obrain-dashboard/src/graph/CosmosWeave.tsx` — système solaire, nappe, toile.
- `packages/obrain-dashboard/src/lib/api.ts` — `fetchWeave` (+ avertissement sur `fetchLabelPairs`).
- `scripts/weave.sh` · `scripts/vizserve.sh`.
- `scripts/cosmos-capture.mjs` — capture réelle via le Chrome local (CDP) + mesure
  de scène (contenu, luminance, centrage du foyer) : la vérification visuelle.
- `scripts/check-weave-contract.mjs` — garde-fou du contrat sans navigateur :
  champs lus par le composant, `NaN` des maths (orbites, taille, affaissement de
  la nappe), monotonie du curseur de densification. C'est ce qui remplace la
  capture d'écran que l'agent ne peut pas produire ; à lancer après tout
  changement de payload.
