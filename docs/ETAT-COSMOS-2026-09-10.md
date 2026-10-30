# État du travail — Cosmos, chat et mesures (2026-09-10)

Document de reprise. Il dit **ce qui est livré**, **ce qui est vérifié et comment**,
**ce qui reste ouvert** — et **ce qui n'est pas de moi**, pour qu'aucun travail ne
soit perdu ni attribué à tort.

---

## 1. Ce qui est livré, et la preuve associée

Tout est committé sur `feat/mcp-next-step-hints`. Chaque ligne porte la mesure qui
la justifie ; aucune affirmation n'est laissée sans chiffre.

### Navigation continue dans le Cosmos

| commit | ce qui a été fait | vérification |
|---|---|---|
| `c9de67c7` | La molette seule traverse espace → monde → dedans. `pres` passe de 2,2 à **0,25 rayon** (la caméra était DEHORS à 100 %), course **logarithmique** (93 % du trajet se passait dans le vide), le monde regardé devient le centre d'orbite et **cesse d'orbiter** | molette seule : `espace 11 % → 32 % → monde 54 % → dedans 75 % → 97 % → 100 %` |
| `3624d772` | **Régression de moi, corrigée** : la molette demandait **78 crans** pour traverser (three.js applique `0,95^zoomSpeed`) | avant : 10 crans → 18 %. après : **8 crans → 100 %** |
| `4c673703` | **L'échelle du monde multipliait toutes les distances de l'intérieur par 4,5** — ma contraction était dix fois trop faible | composition intérieure : 17,5 % → **32-37 %** de contenu, luminance 13,5 → 45 |
| `3b0478cf` | Diagnostic par palier : la scène **se vidait** à mesure qu'on entrait (81 % → 53 % → 19 %) | compte de contenu par palier |

### Les étages et leur fidélité

| commit | ce qui a été fait | vérification |
|---|---|---|
| `4a30e252` | **Timeline d'événements** (un chargement lourd à la fois), **volumétrie répartie au prorata** des effectifs réels (l'ancienne formule `√n` écrasait les rapports et le plafond de 1600 les annulait), couleurs issues du **nom réel** au lieu de l'index | composition intérieure **33 % → 75,4 %** |
| `d68fdd0e` | **Tissu : 1 % de la substance était représentée** (2 347 ancres → 24) et les **flux n'étaient pas tracés** | ancres envoyées 24 → **160** ; flux 8 → 24, tracés en faisceaux ; carte 39,4 % → 41,4 % |
| `31c1133b` | **Taille d'un monde ajouté = constante `0.5`** : 310 547 et 1 378 nœuds avaient la même taille | `drivers-net` passe de masse 0.500 à **0.887** |
| `274efd97` | Tétraèdres **K4** ajoutés aux triangles K3 | `300 nœuds · 14 509 arêtes · 120 triangles · 40 tétraèdres` |
| `65d5274f` | `setSignaux` **n'était jamais appelé** (étage invisible sur tous les stores) + entrée des mondes de code résolue par recherche | sur `linux` : `courbure −0,81 → −0,42 (3 distinctes)` |
| `0a2968cc` | Axe **temps** : l'histoire réelle d'un monde | `344 sessions, 26/08 → 10/09/2026 · 1:53 2:43 3:53 4:43 5:49 6:102` |
| `7f5d7de6` | Axe **∂** (contour structurel) | sur `drivers-net` : `bord ∂ 97 · intérieur 23` |
| `d8eb7598` | Étage **sémantique** déclaré indisponible, **raison mesurée** | `vector-index` non compilé, `n.embedding/vector/vec/emb` → NULL partout |

### Chat

| commit | ce qui a été fait | vérification |
|---|---|---|
| `69d63402` | **Flux SSE** (`http_min::post_sse`, lecteur incrémental), engine en conversation (contexte obrain → think → texte), multi-tours, ancrage sur le **graphe global**, vue plein écran + bouton flottant | contexte annoncé à **2,5 s** ; think 24 → 266 car. ; texte 208 → 698 car. ; 2 questions → 2 tours ; plein écran `0,0,390×844` |
| `9b0a814d` | **Les clics traversaient le chat** (`.ov-panneaux` porte `pointer-events: none`) : champ inutilisable, fermeture impossible | avant : élément au point du champ = un en-tête de panneau derrière. après : focus **OUI**, saisie OK, fermeture **OUI** |
| `8d4ee9c4` | **Une réponse coupée n'est plus présentée comme complète** : `finish_reason` capté et affiché, plafond 900 → 4096 | `fin` reçue : `{'raison': 'length'}` → l'interface l'annonce |

### Mesure

| commit | ce qui a été fait |
|---|---|
| `90cdc56e` | **Instrument consolidé** `scripts/perf-endpoints.mjs` : chaque route **et** chaque opération du moteur, premier appel vs suivants, artefacts horodatés `docs/perf/` |
| `83d9fe0b` | **Rejeu du journal du 8432** : les anomalies du serveur mesurées à la source |

---

## 2. Ce qui est vérifié, et comment

Sept contrôles pilotent le vrai navigateur (**CDP sur `127.0.0.1:9248`**) — ils font
le **geste réel**, pas un appel d'API. État à la dernière passe, tous ✓ :

| contrôle | ce qu'il garantit |
|---|---|
| `continuum-check.mjs` | descente continue, **aucun trou**, étages peuplés — **et depuis ce soir une passe à la MOLETTE SEULE** (`92 % en 6 crans`) |
| `emphasis-check.mjs` | la sélection change le **rendu** : bruit 0,00 %, luminance −24 % |
| `mobile-panels-check.mjs` | panneaux repliables, repliés par défaut, canvas majoritaire |
| `views-controls-check.mjs` | les trois vues partagent grammaire, chrome et sujet |
| `cosmos-capture.mjs` | scène nue **41,6 %** de contenu · 11,95 % de lumineux · foyer centré · 0 erreur |
| `activation-live-check.mjs` | l'allumage vient du journal réel (**2 347 ancres**) |
| `engine-ask-check.mjs` | l'engine répond ancré sur des nœuds réels |

**Pourquoi la passe à la molette a été ajoutée** : le contrôle pilotait la descente
**au clavier**. Il a donc validé pendant plusieurs rounds un trajet que
l'utilisateur n'emprunte pas — et il n'a pas vu la régression des 78 crans.
Un contrôle qui n'exerce pas le geste de l'utilisateur ne teste rien.

---

## 3. Ce qui reste ouvert, avec le chiffre qui le bloque

1. **`/api/viz/subgraph hops=2` coûte 26 s au premier appel** (20 ms ensuite).
   C'est le point noir de la descente. La phase dite « rapide » (hops=1, **802 ms**)
   peuple l'étage tout de suite et les motifs arrivent après — c'est le palliatif
   en place. Le correctif est **moteur**.
2. **`count(r)` = 30 µs contre `count(r:PROVES)` = 20 ms** — même question, filtre
   de type, **666×**. Il manque un **compteur par type d'arête**. Idem un étage
   au-dessus : agréger par label = 232 ms contre 29 µs pour un comptage simple.
3. **4 nœuds cycliques** font payer **124 ms** par expansion contre 0,10 ms pour
   leur voisin (**×1245**). Ce sont des messages d'une même session
   (`session-5ac886fb…`). Le parcours détecte le cycle et le refait.
4. **La base est abîmée sur disque** : 30 191 décodements de propriétés en échec
   sur **241 nœuds** (`WAL frame invalid: StringRef out of range`), récupérés **en
   mémoire** au chargement — donc lisibles aujourd'hui, mais avec un risque de
   perte au redémarrage. Les compteurs d'instantané ont dû être **corrigés**
   (371 258 → 372 019 nœuds).
5. **6 mondes sur 12 n'ont aucune fibre** : ils ne partagent aucun
   `qualified_name`. La moitié de la carte est vide de trajectoires. Piste :
   tracer la **parenté** (7 liens) et les **258 ponts de vécu datés**, déjà calculés.
6. **Le modèle est lent** : **140 s** pour une question ouverte, 19 s pour une
   courte. Le raisonnement consomme les deux tiers du temps *et* la moitié du
   plafond. Levier : consigne plus directive ou plafond de raisonnement séparé.
7. **Rien n'est en nanosecondes.** Plancher observé : quelques dizaines de
   **microsecondes**. Plafond : 26 s. Six ordres de grandeur, dans le même
   instrument (`docs/perf/ENDPOINTS.md`).

---

## 4. Ce qui n'est PAS de moi (à ne pas attribuer, à ne pas écraser)

L'arbre de travail contient des modifications **non committées d'un autre agent**
travaillant sur le même dépôt :

- `packages/obrain-dashboard/src/graph/CosmosWeave.tsx` — désignabilité des nœuds
  du graphe au raycast (`ray.params.Points.threshold`, `gr.points`, `noeudsInfo`) ;
- `packages/obrain-dashboard/src/styles/_layout.scss` — panneau des axes sur petit
  écran (`@media (max-width: 640px) .monde-focus`) ;
- `scripts/vizserve.sh` — arrêt du préchauffage `arcs` (trois agrégats concurrents
  provoquaient un OOM au lancement) ;
- ~20 fichiers Rust (`commands/*`, `engram/*`, `substrate/*`, `indexer/*`) ;
- scripts non suivis : `check-all.sh`, `e2e-dashboard.mjs`, `smoke-backend.sh`,
  `enrich-ricci-*.sh`, `guard-msgpath.sh`, `msgpath_guard.py`, `bench-message-path.sh`.

**Je n'ai rien committé de ce travail et rien écrasé.** Mes commits ne touchent que
mes propres fichiers, ajoutés nommément.

---

## 5. Environnement, pour rejouer

- **Dashboard** : `http://192.168.1.103:8432` (lanceur `scripts/vizserve.sh`,
  journal `/tmp/vizserve.log`). 12 mondes servis.
- **Navigateur de vérification** : Chromium *headless* Playwright sur le port
  **9248**, relancé en tâche de fond ce soir (il avait été tué par un `pkill`
  imprudent de ma part). Commande dans `/tmp/chrome-cdp.log`.
- **Modèle** : `llama-server` sur `127.0.0.1:9901`, diffusion SSE (think + texte).
- **Instrument de coûts** : `node scripts/perf-endpoints.mjs` (~2 min, 3 tours) →
  `docs/perf/ENDPOINTS.md` + `docs/perf/endpoints.json`.

### Piège à ne pas reproduire

`pkill -f "<motif>"` **matche ma propre ligne de commande** quand le motif y figure
en clair : je me suis tué deux fois, et j'ai tué le navigateur de vérification une
fois. Tuer **par PID**, jamais par motif.

---

## 6. Ma recommandation d'ordre

1. **Compteur par type d'arête** (point 2) : le gain le plus large pour l'effort le
   plus faible, et `docs/perf/` sait le mesurer avant/après.
2. **Parcours cyclique** (point 3) : quatre nœuds identifiés, 124 ms chacun.
3. **Timer `enrich --ricci`** : la courbure n'est maintenue que par des passes
   manuelles ; les nouvelles arêtes arrivent sans courbure.
4. **Ponts de vécu** (point 5) : les trajectoires les plus lisibles qui manquent.

Les points 1 à 3 sont **moteur**. Le point 4 est **vue**, donc à ma portée.
