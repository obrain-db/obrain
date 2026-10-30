# PROCESSUS UI — le 8432 ne perd plus une capacité en silence

*Écrit le 2026-09-15, après la régression du 14/09 : la vue markdown des
réponses de l'agent a disparu du dashboard entre 20:03 et 21:50, sans qu'aucune
porte ne s'ouvre.*

La règle, en une phrase : **on ne sert plus un dossier de build, on sert une
VERSION — nommée, gelée, promue en un geste, et vérifiée après coup.**

---

## 1. Ce qui s'est passé (mesuré, pas supposé)

`viz_serve.rs::app_dir()` résout le dashboard par un chemin **figé à la
compilation** : `CARGO_MANIFEST_DIR/../../packages/obrain-dashboard/dist`
(documenté dans `docs/plan-j5-bascule.md` §« PIÈGE dist du viz »). Pour servir le
dashboard du tronc sur le binaire j5, on avait donc **copié** ce dist bâti à cet
endroit — c'est le remède appliqué à l'époque.

| Quand | Ce qui a été fait | Conséquence |
|---|---|---|
| 14/09 ~16:45 | copie du dist du tronc dans `banc-obrain-j5/.../dist` | le 8432 sert le dashboard du tronc : markdown, KaTeX, chat plein écran, fratrie |
| 14/09 **20:03** | `pnpm build` dans le worktree j5 (commit WS `bfad86ff`) | `dist/index.html` est **réécrit** vers un bundle bâti depuis j5, lignée **sans** markdown |
| 14/09 **21:50** | redémarrage du service | la substitution devient effective — markdown perdu |

Preuves, à l'octet : les 16 bundles datés 16:45 dans le dist servi sont
**identiques** à ceux du tronc (`index-BbONi8wC.js` sha `1727390f…`,
`index-BwJ1RCzW.css` sha `ea1c60bd…`) et portent `react-markdown` + `katex` avec
un CSS de 399 479 o ; le bundle de 20:03 (`index-5yMkCKfg.js`) n'en porte aucune
trace et son CSS fait 25 968 o. Dans la lignée j5, `graph/RenduRiche.tsx` est
absent et `react-markdown`, `remark-gfm`, `remark-math`, `rehype-katex`, `katex`
ne sont **pas** dans `package.json` : ce bundle ne *pouvait pas* rendre le
markdown.

Aucune de ces deux étapes n'était fautive en soi. Ce qui manquait, c'est une
**définition de ce que la page servie doit savoir faire**, et un refus quand un
build ne la respecte pas.

## 2. Comment ça marche maintenant

```
~/.local/share/obrain-harness/ui/
├── releases/<horodatage>-<sha>-<etiquette>/   ← versions gelées (chmod a-w)
│   ├── index.html · assets/…                  ← LA FERMETURE SERVIE, rien d'autre
│   └── capacites.json                         ← ce que cette version sait faire
├── current → releases/<…>                     ← la version servie (lien)
├── etat.json                                  ← manifeste de la version courante
└── journal.log                                ← historique des promotions
```

Dans le worktree **servi**, `packages/obrain-dashboard/dist` n'est plus un
dossier mais un **lien** vers `…/ui/current`. C'est ce qui rend l'accident
impossible : un `pnpm build` à cet endroit écrit dans une version gelée, donc
**échoue bruyamment** au lieu d'écraser la page servie.

Trois pièces :

| Pièce | Rôle |
|---|---|
| `scripts/ui-capacites.mjs` | mesure ce qu'un dist sait faire, dans la **fermeture atteignable** depuis `index.html` |
| `scripts/ui-contrat.json` | ce que l'UI servie doit avoir (`bloquantes`) et ce qui ne doit pas disparaître (`suivies`) |
| `scripts/ui-deploy.sh` | bâtit, **gate**, gèle une version, bascule, vérifie |
| `scripts/ui-verifier.sh` | compare les octets SERVIS à la version courante ; branché sur `ExecStartPost` |

⚠ **Leçon d'instrument, apprise ici même** : la première version du mesureur
balayait *tous* les `.js/.css` du dossier d'assets et concluait « markdown
présent » sur un dist qui ne le servait pas — parce que le dossier d'assets est
un cimetière qui accumule les bundles de tous les builds passés. **Un dossier de
build n'est pas ce qui est servi ; la fermeture de `index.html` l'est.**

## 3. Les gestes du quotidien

**Servir une nouvelle UI** (bâtir dans un worktree, gate, promotion, vérification) :

```bash
bash scripts/ui-deploy.sh --worktree /home/triviere/projects/banc-obrain-j5
```

**Promouvoir un dist déjà bâti :**

```bash
bash scripts/ui-deploy.sh --depuis <chemin-du-dist> --id <etiquette>
```

**Savoir si la page servie est bien celle qu'on croit :**

```bash
bash scripts/ui-verifier.sh                     # PASS/FAIL, détaille chaque dérive
```

**Voir l'histoire :** `cat ~/.local/share/obrain-harness/ui/journal.log`

**Si la porte refuse** : elle nomme la capacité perdue et rend la main sans rien
changer. Deux issues honnêtes — corriger le build, ou **accepter** la perte
explicitement (`--accepter-perte <capacite>`), auquel cas elle est écrite dans
`etat.json` et dans le journal. Ce qui est interdit, c'est la disparition
silencieuse.

## 4. Ce qui est branché sur le service

Un drop-in `~/.config/systemd/user/obrain-viz.service.d/verif-ui.conf` exécute
`ui-verifier.sh http://192.168.1.103:8432 --attendre` à chaque démarrage. Le
préfixe `-` est délibéré : une dérive est **dite** dans le journal, elle ne fait
pas échouer l'unité (`Restart=always` en ferait une boucle de redémarrage).
L'adresse est celle du LAN : le service n'écoute pas sur la boucle locale.

## 5. État au 2026-09-15 — lignées réunies

Version servie : la **plus récente promue** — au 2026-09-15
`20260915-104356-3bf9c301-echelle-multi-niveaux` (lignée `fusion/tronc-plus-j6`
@ `3bf9c301`, promue par une autre session sur la même lignée, **sans aucune
perte acceptée**). `scripts/ui-bascule.sh --liste` donne l'état réel ; `etat.json`
décrit toujours la version courante.

Historique de la journée : `…-markdown-retabli` (10:29, markdown restauré,
`ws-client` perdu et accepté) → `…-reunion-markdown-ws` (10:33, réunification,
5/5) → `…-echelle-multi-niveaux` (10:43, 5/5).

| Capacité | État | Note |
|---|---|---|
| `markdown` | ✅ bloquante | rendu markdown des réponses de l'agent |
| `katex` | ✅ bloquante | formules + polices (CSS 399 479 o) |
| `fratrie` | ✅ suivie | frères/sœurs d'un élément sélectionné |
| `projection` | ✅ suivie | panneau de projection multi-axes |
| `ws-client` | ✅ suivie | transport WS streaming **récupéré** par la réunification |

### La réunification (2026-09-15)

Les deux lignées avaient chacune un morceau que l'autre n'avait pas. Elle est
faite **dans le sens qui n'enlève rien** : on a porté le client WS dans la
lignée qui portait déjà le markdown et la fratrie (l'inverse aurait fait perdre
la fratrie, refusée par la porte).

Le portage se fait en **un seul point**, `packages/obrain-dashboard/src/lib/api.ts` :

1. `getCached` accepte un **chargeur de remplacement** — c'est ce qui permet
   d'interposer un autre transport sous le même cache sans dupliquer la logique
   de session/single-flight ;
2. `streamSubgraph(...)` (repris de `bfad86ff`) accumule les chunks
   `begin/nodes*/edges*/end` ; `subgraphParWsOuHttp(...)` tente le WS puis
   **retombe sur HTTP+gzip**, et une annulation volontaire ne déclenche pas le
   repli ;
3. `fetchSubgraph` garde sa signature et son **cache de session 30 s**.

⚠ **Piège évité, à retenir** : le `fetchSubgraph` du tronc passait par
`getCached(..., 30_000)` alors que celui de j5 au moment du commit WS passait par
un `get` nu. Un portage brut du diff aurait donc **supprimé le cache de session**
sans que rien ne le dise — le fichier aurait été « à jour » et le dashboard plus
lent. Le porte-à-faux se voit seulement quand on lit les DEUX versions du point
de passage.

Preuves au 2026-09-15, sur le service vivant :

| Contrôle | Mesure |
|---|---|
| Bundle servi (`index-BdQIpy7a.js`, version la plus récente) | `react-markdown` · `katex` · `KaTeX` · `__fratrie` · `subgraph/stream` · `proj-row` — les six marqueurs |
| WS ↔ HTTP, même requête (`center=3&hops=1&limit=50`) | frames `begin nodes edges end` · 1ʳᵉ frame à **7 ms** · schéma, stats, **nœuds (15) et arêtes (14) IDENTIQUES** |
| Porte | 5/5 capacités, aucune perte acceptée · `UI SERVIE CONFORME` |

### ⚠ Le pionnier à ne pas refaire : `projection-check.mjs` ne voyait pas le WS

Sa sonde n'enveloppait que `window.fetch`. Depuis la réunification, le
sous-graphe part par **WebSocket** : les contrôles 4 et 4bis annonçaient
« 0 requête(s) » alors que la requête était émise — un **angle mort de
l'instrument**, pas une absence. Le constructeur `WebSocket` est désormais
enveloppé lui aussi (constantes `OPEN`/`CLOSED` recopiées : jamais remplacé).
**Quand le transport change, l'instrument doit changer avec lui** — c'est la
deuxième fois dans cette affaire qu'une mesure décrit autre chose que le vivant.

## 5 bis. Deux sessions ont promu en même temps — et c'est le vérificateur qui l'a vu

Le 2026-09-15, pendant qu'un A/B basculait `current` à la main (chemin de version
codé en dur), une **autre session a promu sa version** à 10:43:56. Le retour
arrière du test a écrasé cette promotion : `current` pointait sur une version, et
`etat.json` décrivait l'autre.

`ui-verifier.sh` l'a dit immédiatement :

```
FAIL  index-36nyX3nG.css : absent de la version courante
FAIL  index-BdQIpy7a.js : absent de la version courante
UI SERVIE DIVERGENTE — 2 contrôle(s) en échec
```

Deux corrections de processus en sont sorties :

1. **`ui-deploy.sh` prend un VERROU** (`flock` sur `$UI_DIR/.verrou`) : deux
   promotions ne peuvent plus s'entrelacer, et la mesure de « ce qui est servi
   avant » ne peut plus porter sur un état qu'une autre session est en train de
   changer.
2. **`ui-bascule.sh`** : la SEULE façon correcte de changer la version servie.
   Il prend le verrou, **réécrit `etat.json` d'accord avec la version visée**
   (sinon le vérificateur signalerait, à juste titre, une divergence), bascule le
   lien, journalise (`BASCULE … (précédente : …)`) et vérifie. On ne bascule plus
   `current` à la main.

Réparé dans la foulée : la promotion de l'autre session est rétablie, le
manifeste et la version courante concordent, `UI SERVIE CONFORME`.

## 5 ter. Le défaut que le contrôle de la fratrie a mis au jour (2026-09-15)

`fratrie-check.mjs` échouait sur le 8432 depuis des jours. Ce n'était **ni** la
fratrie, **ni** la réunification (prouvé par A/B sur le vivant). La chaîne :

1. Le contrôle ne trouvait **aucune cible** (`cible : null`) et **mesurait
   quand même** — URL `?store=null`, donc store par défaut (un store « runtime »,
   face MONDE sans `FacetBar`). Ses essais de sélection étaient collectés et
   **jamais imprimés**. C'est ce silence qui a coûté le plus de temps.
2. La raison des essais : ses sondes interrogent les entrées par défaut de
   l'application (`e1000_probe`, `tcp_v4_rcv`, `substitute_in_operator`) —
   des **noms nus**.
3. Or les deux lignées ne résolvent pas un centre de la même façon :
   - **servie (j5)** : `resolve_center_strict` — id numérique ou `qualified_name`
     **exact** seulement, sinon `400 {"error":"centre introuvable: e1000_probe"}`
     (décision NATIF-J5C : le repli par suffixe résolvait un centre absent sur un
     nœud arbitraire, puis lançait une expansion coûteuse sur le mauvais nœud) ;
   - **tronc** : `resolve_node_opts(&db, c, true)` — repli par suffixe/nom.
4. Conséquence **visible pour l'utilisateur** : sur le 8432, choisir
   « Linux e1000 », « Linux complet » ou « Obrain (rust) » laissait la **scène
   vide**, sans rien dire — l'application envoyait `e1000_probe`, le serveur
   répondait 400. Mesuré : 0 nœud ; la même ancre en qn complet rend 18 nœuds.

**Correction appliquée, côté client** (le serveur reste strict, l'ancre devient
exacte) : `resoudreAncres(store, noms)` dans `lib/api.ts` résout le nom nu en qn
via `/api/functions` (dont les paliers sont qn exact → nom → suffixe → contient),
et `App.tsx` s'en sert pour poser ses ancres par défaut. En cas d'échec de
résolution, le nom est rendu inchangé : un binaire au repli permissif le résout
encore — le correctif ne casse donc rien sur la lignée du tronc.

**Effet mesuré** (contrôle de la fratrie sur le 8432, cible choisie seule) :
`cible : Linux e1000 / e1000_probe`, 18 nœuds chargés, barre de facettes
présente, **frères 17 · sœurs 17 annoncés**, plafond 14 respecté, troncature
annoncée (« +3 autres »), dépliage exact (17), clic sur un frère qui change la
cible, fratrie qui reste ouverte après l'ancrage.

**Ce qui reste en échec, et qui n'est pas la fratrie** (3 contrôles) :
- « les deux anciennes règles divergeaient sur 0 qn » — le contrôle échantillonne
  les 4 premiers stores (des stores « runtime ») et exige des qn portant
  `__file__` ; l'échantillon n'en contient aucun sur le 8432 (115 sur 8433).
  Attente dépendante des données.
- « aucune fratrie affichée après un clic sur une boule » et « aucune puce
  “ressortir” » — phase **Cosmos sur le store par défaut**, c'est-à-dire un store
  « runtime », que le contrôle lui-même documente comme ouvrant la face MONDE.
- « clic “yml 3” : cadrage sans effet » — **instable** : passé 3 fois sur 5 sur le
  même bundle.


Ancien dossier, conservé (renommé, pas supprimé) :
`banc-obrain-j5/packages/obrain-dashboard/dist.avant-decouplage-20260915-102954`.

## 6. Ce qui reste ouvert, hors de ce processus

- ~~**Lecture de fichier arbitraire sur le 8432 vivant**~~ : **FERMÉE le
  2026-09-15** — la garde de la lignée consolidée (`chemin_sous` : décodage
  pourcent, refus du composant `..`, `canonicalize` + `starts_with(racine)`) a
  été portée dans la lignée servie et déployée. Vérifié sur le vivant : **404 à
  toutes les profondeurs 7→20**, y compris `%2e%2e` et par lien symbolique ;
  l'UI se sert normalement. Détail : `banc-obrain-j5/docs/CONFINEMENT-STATIQUE-2026-09-15.md`.
  ⚠ **Le contrôle de protocole a menti entre-temps** : il envoyait sept `../`,
  calibrés sur l'ancienne racine ; le `dist` étant devenu un lien vers un chemin
  plus profond, sept niveaux n'atteignaient plus `/etc` et il a affiché PASS sur
  une fuite intacte. Il teste désormais l'échappement de 7 à 20 niveaux.
- **10 autres manquements de protocole** sur le binaire servi (HEAD, `Date`,
  `ETag`/304, keep-alive, 400 sans Host…) — 15/15 sur le binaire consolidé.
- **`scripts/acceptation-8432.sh` ne peut pas démarrer** dans son mode par défaut :
  `ISOLÉ_PORT=8433` (ligne 31) n'est pas un nom de variable bash valide ; sous
  `set -u`, `$ISOLÉ_PORT` se lit `${ISOL}É_PORT` → « ISOL: unbound variable ».
  Correctif : renommer `ISOLE_PORT` (lignes 31, 79, 90).
- **`fratrie-check.mjs` ne lance aucun navigateur** : il se connecte à un CDP
  (`argv[3] || http://127.0.0.1:9248`). La porte doit démarrer ce chrome, sinon
  le contrôle meurt en `ECONNREFUSED` et le verdict « fratrie non prouvée » est
  un artefact du harnais, pas un verdict produit.
- **La séquence de déploiement du binaire consolidé** (§11 de
  `docs/ECART-DEPLOIEMENT-LIGNES-2026-09-14.md`) reste non exécutée.

## 7. Échelle multi-niveaux du canevas — audit et correctifs (2026-09-15, 11:15)

Version servie : `20260915-111551-5a6f25de-descendre-ne-recule-pas` (branche
`fusion/tronc-plus-j6`), promue sans aucune perte acceptée, porte 5/5.

Audit de la page SERVIE (CDP local, fenêtre 1440×757), puis cinq correctifs —
tous dans `CosmosWeave.tsx`, `App.tsx`, `_layout.scss` :

| Défaut mesuré | Cause | Correctif |
|---|---|---|
| Les trois boutons de vue ne faisaient RIEN (`t` = 0, caméra 1141 → 1131, bouton actif toujours « Cosmos ») | `niveau` lu dans la boucle de rendu, absent des dépendances de l'effet → valeur figée au montage ; `niveauDemandeRef` déclarée et jamais utilisée | la demande voyage par la référence |
| « ↓ entrer » ne descendait pas (12 % → 12 %, caméra 369,2 → 369,2) | `down` appelait `onOpen` = CHANGEMENT DE BASE | `down` appelle `plonger()` |
| Le seul bouton de descente hors écran (y = 805 pour 757 px) puis inerte | il vivait au bas du panneau qui défile (contenu 1251 px / 588 visibles) ; dans `reperes` il héritait de `pointer-events: none` | barre `.echelle-bar` toujours visible, hors du panneau, en `pointer-events: auto` |
| Fond de la descente VIDE en silence (« graphe ✓ 16ms » mais aucune ligne « Graphe : ») | l'entrée était un nom de FAMILLE (`deepseek_harness`), pas un `qualified_name` → 400 sur les deux phases ; et le repli vivait dans un `.then` qu'un rejet saute | résolution AVANT l'appel (pas de séparateur ⇒ `/api/functions`), repli atteignable en cas d'échec, qn résolu retenu pour la 2ᵉ phase |
| Crans et libellés se contredisaient (cran « Graphe » à `t` = 0,63 → bande « monde ») | cran exprimé en RAYONS vs échelle affichée en CURSEUR | une seule autorité : bandes + crans en `t`, distance déduite |
| « ⤓ descendre » RECULAIT (85 % → 63 %) | destination calculée par formule (`CADRAGE_POUPEE`) sans regarder où l'on est | le geste n'accepte qu'une destination plus profonde |

Contrôles : `scripts/echelle-multi-niveaux-check.mjs` (nouveau — les cinq
exigences) ; `scripts/continuum-check.mjs` repasse au vert (ses échecs réseau sont
désormais jugés PAR URL, la `404 /ui/state/*` documentée reste affichée).

Côté données, le budget de la phase « motifs » passe de 200 000 à 20 000 : mesuré,
`hops=2 limit=200000` rend 75 210 nœuds / **88 667 arêtes** / 38 Mo contre
20 000 nœuds / **les mêmes 88 667 arêtes** / 16 Mo. Les motifs se comptent sur les
arêtes — rien n'est perdu. Le panneau annonce « N sur M » : la troncature est dite.

Reste ouvert (mesuré, NON corrigé, aucun fichier touché) : la vue par défaut
(`morph`/`StageContinu`) et `StoreOverview`/`Graph3D` gardent leurs propres
impasses — `⌂` du `.zoomctl` inerte (et il désactive les trois boutons du
panneau), filtres « Tout/Cognitif/Code » écrasés dans la même frame, mode
« familles » sans descente à la souris, `↑` ≡ `⌂` dans `Graph3D`.

## 8. Profondeur et polymorphisme de la navigation (2026-09-15, 11:46)

Version servie : `20260915-114646-1b1fda11-rapport-garde-monde-vise`, porte 5/5.

### Profondeur — un quatrième cran, et un étage qui était mort

`gr.posGraphe` était **lu deux fois et jamais assigné** : la « migration vers
l'espace des signaux » (nœuds ET arêtes qui voyagent ensemble vers une disposition
où les axes sont courbure · énergie · degré) ne pouvait donc **pas s'exécuter**.
Corrigé : `gr.posGraphe = pos.slice()` (une COPIE — `pos` est le tampon vivant que
la migration écrit, la prendre pour origine ferait dériver le graphe sans retour)
et `gr.posSignaux = posSig`.

`NiveauCosmos` passe à `0|1|2|3` et le sélecteur à quatre crans : **espace · monde ·
groupe · signaux**. Mesuré : camDist 1126,9 → 127,9 → 25,7 → 4,5 px, monotone,
voyant qui suit ; `__etages.monde.signaux` = `{tS: 1, origineGraphe: true,
origineSignaux: true}`.

### Polymorphisme — l'axe devient un geste

Six axes (`projet · parent · extension · communauté · strate · bords ∂`) sont
offerts dans la barre toujours visible. Mesuré sur un graphe de 20 000 nœuds :
1 / 2 / 3 / 1 / 2 / 2 groupes — six découpages **différents**, quatre qui
segmentent, et les deux muets l'annoncent. « ⤓ descendre » traverse l'axe : au
dernier cran il entre dans le plus gros groupe de l'axe par son nœud le plus
important (ordre serveur `importance:degree`), et le nouveau niveau se re-découpe
avec le même axe.

### Deux pannes trouvées en route

| Défaut | Mesure | Cause |
|---|---|---|
| Le voyant des crans restait sur « 🌌 Cosmos » aux quatre crans | compteur de rapports `{n: 300, dernier: 0}` | le rapport n'était pas sous le garde `on` : la boucle passe sur tous les mondes, c'est le **dernier** qui parlait (même piège que `.interieur` et `.monde`, déjà documenté deux fois) |
| Les axes « communauté » et « ∂ » auraient été vides dans la boucle | — | `etiquetteDimension` lisait des **états** React (cartes vides dans la fermeture du montage) ; ils passent par des références, comme `fichiersRef` |

### L'instrument aussi

Le contrôle de fluidité relevait toutes les 500 ms pendant qu'un cran de molette
passait toutes les 500 ms : deux crans pouvaient tomber dans le même intervalle et
la trace montrait un saut de 29 points **qui n'existait pas**. À cadence fine
(relevé 120 ms, un cran toutes les 900 ms) : 5 → 20 → 34 → 48 → 63 → 77 → 91 → 100,
aucun saut. Le contrôle a été corrigé, pas la scène.

## 9. Deux lignées ont failli se reprendre la page (2026-09-15, 11:56)

**Ce qui s'est passé, mesuré.** À 11:46 j'ai promu `…-rapport-garde-monde-vise`
(branche `fusion/tronc-plus-j6`, commit `1b1fda11`). À 11:53, une autre lignée a
promu `20260915-115305-51f332c2-consolidee` (branche
`fusion/tronc-plus-j6-mes-correctifs`). La page servie est passée **de 6 à 0** sur
le marqueur `eb-axe` : le travail de profondeur et de polymorphisme avait disparu
de ce que l'utilisateur voit, sans qu'aucune porte ne s'ouvre — les 5 capacités du
contrat étaient pourtant intactes, donc la porte a dit PASS.

**Pourquoi la porte n'a rien vu.** Elle vérifie des CAPACITÉS (markdown, katex,
fratrie, ws-client, projection). Un build qui perd une fonctionnalité *non listée*
passe la porte : c'est exactement la même leçon qu'au §1, un cran plus loin.

**Ce que le diagnostic a montré, et qui a évité une perte.**
`git merge-base --is-ancestor` sur les trois commits : la lignée concurrente
**contenait déjà** mes deux premiers commits et n'en manquait qu'un (le troisième).
Son seul apport dashboard était `lib/api.ts` (+125 lignes : client WS et
`resoudreAncres`), **déjà présent dans l'arbre de travail partagé**. Les deux
ensembles ne touchant aucun fichier en commun, la construction depuis l'arbre de
travail donne leur **union** — c'est ce qui a été promu :

```
20260915-115637-0aecec01-union-profondeur-polymorphie-et-api
bundle index-DrjULkH5.js — marqueurs : eb-axe ✓ Signaux ✓ non segmenté ✓
                                        resoudreAncres ✓ streamSubgraph ✓
```

**La règle à en tirer.** Deux lignées qui servent la MÊME page ne doivent pas
promouvoir à tour de rôle : chacune doit être construite avec l'autre
(fusion, ou construction depuis l'arbre qui porte les deux), sinon la dernière
promotion gagne et l'autre travail disparaît en silence. Vérification en trois
secondes, à faire AVANT de promouvoir quand on n'est pas seul :

```bash
git merge-base --is-ancestor <mon-commit> <leur-commit>   # sinon : fusionner d'abord
grep -c <mon-marqueur> <bundle-servi>                     # sinon : ma fonctionnalité n'est pas servie
grep -c <leur-marqueur> <bundle-servi>                     # et ne pas perdre la leur
```

## 10. Profondeur le long de l'axe, axe partagé, axe qui segmente (2026-09-15, 13:04)

Version servie : `20260915-130437-5eb329d4-plus-de-sentinelle` (build depuis
l'arbre de travail partagé, qui porte les deux lignées), porte 5/5.

### La profondeur le long de l'axe

Au dernier cran, « descendre » entre dans le plus gros groupe de l'axe par son nœud
le plus important, et le moteur rend le voisinage de ce nœud. Cette profondeur est
désormais **visible** (une poupée `🎯` par entrée dans le fil, cliquable) et
**mesurée** (`__etages.monde.axe.chemin`).

Le fil est **honnête** : une poupée n'est ajoutée que si le graphe a réellement
changé (identité `entrée|nœuds` avant/après) ; sinon le substitut est NOMMÉ
(« aucune profondeur prise : le graphe n'a pas changé … affiché : X »).
Mesuré : 20 000 → 5 nœuds, `chemin` = 1 ; au pas suivant `chemin` reste 1.

### L'axe devient partagé

`axeNavigationAtom` (atome d'autorité) + transport dans le document d'état
(`axe`), avec UNE table de noms dans les deux sens ; un axe inconnu ne s'applique
pas.

### L'axe segmente le cran « monde »

Pour un axe de relation (communauté Leiden, ∂), les groupes de l'intérieur sont
ceux du GRAPHE, et le panneau le dit (« effectifs du graphe chargé, pas du monde
entier »). Mesuré quand le geste aboutit :
`Intérieur (axe « ∂ ») : 2 ensembles — bord ∂ (5801) · intérieur (5277)`.

### Cinq défauts trouvés, dont trois de mon fait

| Défaut | Mesure | Cause |
|---|---|---|
| Blocage mutuel au chargement | rien ne se chargeait | l'événement « graphe » est gaté sur l'intérieur (`it?.pret`) : mon réordonnancement était impossible → re-segmentation en 3ᵉ événement |
| Rechargement impossible | — | `gr.charge` n'était jamais remis à faux (garde posée sur un drapeau permanent) |
| **Recentrage sur un nœud impossible** | clic sur un nœud = aucun effet | `if (gr.charge \|\| gr.pret) return` refusait tout rechargement : le garde porte maintenant sur le CENTRE |
| Re-segmentation automatique | « 1 ensembles — intel (1378) » au lieu de 6 | l'axe auto-choisi pilotait la source → `sourceGroupes()` exige un axe de relation explicitement désigné |
| Un cran qui RECULAIT | 100 % → 0 % sur un clic de « descendre » | `niveauAppliqueRef = -1` : la boucle y voyait une demande neuve et renvolait à l'espace → la descente d'axe ne déplace plus la caméra |
| 400 réapparu | `center=bord ∂` | les groupes d'un axe de relation ne sont pas des nœuds ; le court-circuit ne couvrait que la phase « rapide » |

### Réserve mesurée, non taire

La re-segmentation du cran « monde » par un axe de relation ne prend effet que si
le graphe est **prêt** au moment où l'intérieur se reconstruit ; sinon l'étage reste
sur les familles (et le dit : `source: "familles"`). L'enchaînement
graphe → organisation n'est pas encore garanti à tous les coups.

## 11. La descente fractale, et le serveur qui ne servait plus la bonne version (2026-09-15, 13:43)

Version servie : `20260915-134325-c4254fa7-clavier-hierarchie-corrige`, porte 5/5,
`UI SERVIE CONFORME`.

### Le gain de navigation : des sous-groupes qui étaient inatteignables

Mesuré sur ce store : **3 racines → 200 enfants sous `deepseek_harness` → 60
petits-enfants sous `…packages`**. L'étage intérieur du Cosmos ne montrait que les
racines (6) : la loi existait (`fractale.ts` : `enfantsDe`, `rayons` en ∛part,
`directions`, `estFeuille`, `tokenValide`), servie par `/api/viz/space`, mais elle
n'était pas branchée dans le Cosmos.

| Geste | Effet |
|---|---|
| chip `⤵` par groupe | entre dans SES sous-groupes (agrégés par le serveur, tout le store) |
| `Page ↓` | entre dans le plus gros sous-groupe du niveau |
| `Page ↑` / `Retour arrière` | remonte d'un niveau |
| `⌂ racines › A › B (rang n)` | chaque maillon ramène DIRECTEMENT à son ancêtre |
| niveau feuille | le chip `⤵` disparaît et le niveau est ANNONCÉ comme feuille |

Vérifié : `6 → 24 → 24 → 24` groupes sur quatre niveaux, clavier et maillons
compris, aucune exception.

### Le défaut le plus grave de la journée : le serveur servait une AUTRE version

`scripts/ui-verifier.sh` a répondu **`UI SERVIE DIVERGENTE`** : le serveur de 8432
avait été démarré depuis un TROISIÈME checkout (`banc-obrain-tronc`), qui sert son
propre `packages/obrain-dashboard/dist` — un dossier GELÉ, sans les quatre crans,
sans les six axes, sans les correctifs précédents. Aucune des versions promues dans
le magasin n'était donc visible, et le paquet servi n'existait même pas dans le
magasin (`find` sur tout le disque : introuvable).

**Règle qui manquait au processus** : promouvoir ne suffit pas — il faut que le
worktree SERVI soit découplé (`ui-deploy.sh --lier <dist-du-worktree>`). Un
`obrain viz serve` démarré depuis un checkout dont `dist` est un vrai dossier
resservira ce dossier pour toujours, et `ui-verifier.sh` est le SEUL contrôle qui
le voit (le journal des promotions, lui, reste vert).

```bash
# à faire après TOUT démarrage de serveur, ou après un changement de service :
bash scripts/ui-verifier.sh                     # doit dire UI SERVIE CONFORME
bash scripts/ui-deploy.sh --lier <dist-du-worktree-servi>   # si ce n'est pas le cas
```

### Deux défauts de ma part, trouvés par la mesure

- `Page ↓` inerte : le gestionnaire exigeait `focusW >= 0` (une visée au CLAVIER),
  alors qu'au cran « monde » la visée vient de la caméra → condition toujours
  fausse. Corrigé en retombant sur la cible de la caméra, comme `plonger`.
- Des relevés concluant « page cassée » (`organisation: undefined`, panneau vide)
  alors que c'était le **navigateur de mesure** qui était mort. Vérifier l'état du
  navigateur AVANT de conclure sur l'application.

### Reste à faire, mesuré

Le GRAPHE ne suit pas encore le niveau : entrer dans `packages` change l'intérieur,
mais le graphe reste celui du niveau précédent. Le combler demande de résoudre un
préfixe en nœuds (requête) puis un sous-graphe **multi-centres** — l'API l'accepte
déjà (`center=a,b,c`).

## 12. Le graphe suit le niveau (2026-09-15, 13:49)

Version servie : `20260915-134957-073111cd-graphe-suit-le-niveau`, porte 5/5.

Un groupe de la hiérarchie n'est pas UN nœud mais un ENSEMBLE, désigné par un chemin
de tokens — et le moteur ne prend pas un chemin comme centre (« centre introuvable »).
La voie qui manquait :

| Pièce | Rôle |
|---|---|
| `noeudsDuPrefixe(qnMonde, prefixe)` | les qn du graphe CHARGÉ qui portent ce chemin, dans l'ordre d'importance du serveur, bornés à 32 |
| `chargerGraphe(wd, centre, mode, centres[])` | le moteur accepte plusieurs centres (`center=a,b,c` — vérifié : 4 centres → 200, 21 nœuds) et expanse leurs voisinages |
| `entreeRacine` (par monde) | le retour aux racines REPREND le graphe du monde au lieu de rester sur le sous-groupe quitté |

Le relevé des qn se fait **avant** de vider l'étage (le graphe courant est la source).
Quand aucun nœud du chemin n'est dans le graphe chargé, on le DIT — le graphe ne
prétend pas avoir suivi.

Vérifié sur la page servie, mêmes gestes, mesure après mesure :

    racines ......................... 11 078 nœuds · 11 077 arêtes
    entré `deepseek_harness` ........ 11 171 nœuds · 11 231 arêtes
    entré `deepseek_harness.packages`  350 nœuds ·  333 arêtes · 24 triangles
    retour aux racines .............. 11 078 nœuds · 11 077 arêtes (restauré)
    exceptions ...................... aucune

La navigation va donc du général au particulier sans rupture : le sous-groupe, ses
sous-groupes, **et le graphe de chacun**.

## 12. Le serveur passe derrière un IAM — et le graphe suit le niveau (2026-09-15, 14:31)

Version servie : `20260915-143142-b11c2c5a-chemin-de-graphe` (dans le magasin de
versions ; **derrière une identité**, voir plus bas).

### Le graphe suit le niveau

Entrer dans un sous-groupe changeait l'intérieur mais laissait le graphe du niveau
précédent : la navigation ne suivait qu'à moitié. Trois pièces :

1. `noeudsDuPrefixe(qnMonde, prefixe)` — les qn du graphe chargé qui portent le
   CHEMIN dans leur nom (ordre d'importance du serveur, bornés à 32) ;
2. `chargerGraphe(wd, centre, mode, centres[])` — le moteur accepte plusieurs
   centres (`center=a,b,c`), donc chaque sous-groupe a SON graphe ;
3. l'entrée du monde est retenue (`entreeRacine`) pour que le retour aux racines
   reprenne le graphe du monde.

Mesuré : `11 078` (racines) → `11 171` (`deepseek_harness`) → `350` nœuds /
`333` arêtes / `24` triangles (`…packages`) → retour `11 078`, aucune exception.

### Le chemin des entrées de graphe

Chaque clic sur un nœud recentre le graphe ET s'ajoute au chemin
(`graphe : entrée1 › entrée2 …`) ; chaque maillon y revient avec les MÊMES centres
qu'à l'époque (reprise exacte, pas approchée) ; `Page ↑` remonte d'abord la
hiérarchie, puis le chemin de graphe — l'ordre où on les a prises.

### Le serveur sert derrière un IAM (changement de la lignée voisine)

`OBRAIN_IAM=1` : `/` répond `303 → /login`, `/api/*` répond
`401 {"error":"aucune identité présentée (en-tête Authorization: Bearer <jeton>
…)"}`. Les **fichiers statiques** (`/app/assets/*`) restent servis.

Conséquence pour les contrôles — et c'est un piège à connaître :

> Un contrôle qui ne présente pas d'identité lit l'écran de connexion et conclut
> « le canevas ne marche plus ». C'est un verdict FAUX.

`echelle-multi-niveaux-check.mjs` sait donc faire les deux :

```bash
OBRAIN_TOKEN=<jeton> node scripts/echelle-multi-niveaux-check.mjs   # mesure l'app réelle
node scripts/echelle-multi-niveaux-check.mjs                        # dit qu'il ne peut pas, et sort en 2
```

`ui-verifier.sh` signale en revanche une divergence d'`index.html` dès que `/`
redirige : la comparaison octet à octet avec la version publiée ne vaut que si `/`
sert réellement la page.

### Reste ouvert

- Rejouer les sections 1 à 8 du contrôle derrière une identité (les vérifications
  de la descente hiérarchique et du graphe par niveau ont été faites AVANT la
  bascule IAM).
- Le 400 résiduel quand un groupe d'axe (« bord ∂ ») sert de centre : traité, mais
  à surveiller dès qu'un axe de relation est choisi.

## 13. L'union des deux lignées, et trois défauts qui masquaient la navigation (2026-09-15, 14:59)

Version servie sur 8432 : `20260915-145902-3f304484-union-chat-replie`, bundle
`index-DNDN7g-q.js` — construite depuis un worktree d'UNION (`/tmp/union-nav`,
branche `union/nav-et-rendu-20260915`) : leur moteur de rendu (`json-graph@2`,
arêtes en colonnes) **plus** mes quatre commits de navigation.

### Pourquoi une union

Le 14:42, la lignée voisine a promu `9146bce4` (branche
`consolidation/tronc-release-2026-09-15`) : elle contenait mes commits jusqu'à
`c4254fa7` mais **pas** la descente fractale, le graphe par niveau, le chemin
d'entrées ni le contrôle. Ses apports à elle (moteur de rendu) n'étaient pas dans
la mienne. Aucune des deux branches n'était donc servable seule.

```bash
git worktree add -b union/nav-et-rendu-20260915 /tmp/union-nav 9146bce4
cd /tmp/union-nav && git cherry-pick 073111cd b11c2c5a ff8b4ffb 3c4831fc   # DANS L'ORDRE
cd packages/obrain-dashboard && pnpm install --prefer-offline && ./node_modules/.bin/tsc -b
bash scripts/ui-deploy.sh --worktree /tmp/union-nav --id union-nav-et-moteur-rendu
```

**Ordre des cherry-picks** : les prendre un par un dans l'ordre chronologique.
`b11c2c5a` seul entre en conflit sur `CosmosWeave.tsx` — non par désaccord, mais
parce que son parent logique (`073111cd`) manquait : l'union se fait dans l'ordre.

### Trois défauts trouvés, dont deux invisibles sur le serveur habituel

| Défaut | Mesure | Cause |
|---|---|---|
| **Le chat plein écran couvrait le canevas** | `.chat-plein` = 1094×696 en z-60, `elementFromPoint(720,420)` → `DIV.chat-corps` ; molette 0 % → 0 %, bouton « ⤓ descendre » recouvert | `const [chatOuvert, setChatOuvert] = useState(true)` dans la lignée voisine. Replié par défaut : le canevas reçoit enfin les clics (`elementFromPoint` → `CANVAS`) |
| **Vue entière en erreur sur le repli « toile non tissée »** | écran « Cosmos — erreur », React #300, 0 canevas | `useAtomValue(montreAtom)` déclaré APRÈS le `return` du repli → nombre de hooks variable. Invisible sur un serveur dont la toile est tissée |
| **Le paquet servi n'était pas celui déployé** | `ui-verifier.sh` : `UI SERVIE DIVERGENTE` | un troisième checkout servait son `dist` gelé (voir §11) |

### Trois leçons d'instrument

1. **Le contrôle doit voir les `console.error`** : une frontière d'erreur React
   signalait #300 par `console.error`, que `Runtime.exceptionThrown` ne voit pas.
   Le contrôle annonçait « 23 exigences non tenues » sans dire pourquoi.
2. **Le contrôle doit pouvoir s'identifier** : derrière l'IAM, `OBRAIN_TOKEN=<jeton>`
   est présenté à toutes les requêtes ; sinon il DIT qu'il ne peut pas mesurer.
3. **Le contrôle ne conclut pas sur un instant** : le bloc du chemin n'est rendu
   que si l'étage a plus d'un groupe — un relevé sec tombait pendant la
   re-segmentation et annonçait « ABSENT » à tort. Il attend, borné.

### Vérifier sans l'IAM : une instance isolée

`obrain viz serve` armé (`OBRAIN_IAM=1`) protège `/api/*`. Pour vérifier le canevas
sans identité, on sert **le même binaire** sur un port local, sans IAM, **avec la
base synaptique** (sans elle, `weave.available = false` et le Cosmos affiche le
repli — ce qui a provoqué un faux « la vue est cassée ») :

```bash
cd <worktree-du-binaire>
./target/release/obrain viz serve --host 127.0.0.1 --port 8433 --enrich-ricci 0 \
  --preset-default "Runtime dev (engrams)" \
  --preset "Runtime dev (engrams)=…/data/runtime-dev.obrain" \
  --synapse-store …/data/agent-synapse-triviere-dsh.obrain
node scripts/echelle-multi-niveaux-check.mjs http://127.0.0.1:8433
```

**Résultat sur le build d'union : `✓ TOUT EST TENU`** — quatre crans monotones,
descente continue et sans recul, groupes auto-segmentés, étage des signaux réel,
six axes polymorphes, descente hiérarchique (sous-groupes de sous-groupes), chemin
affiché (`chemin : ⌂ racines › deepseek_harness (rang 4) ↥ remonter`), graphe qui
suit le niveau (11 078 → 11 171 → 350 → 11 078 nœuds).

### Reste ouvert

- Les deux lignées doivent être FUSIONNÉES proprement (pas seulement construites
  ensemble) : `union/nav-et-rendu-20260915` est un worktree de travail, à reporter
  dans la branche de consolidation.
- `ui-verifier.sh` compare `index.html` octet à octet : sous IAM, `/` répond `303`,
  donc il signale une divergence qui n'en est pas une. À faire évoluer.

## 14. Densifier ou approfondir : deux gestes pour un seul mot (2026-09-15, 16:05)

Demande : « Dans la navigation multi-échelle tu dois savoir quand densifier call avec
pagination de quand approfondir, allez voir les surfaces autres qui se branchent
mécaniquement à la suite. »

### Le défaut mesuré, et pourquoi la question était INDÉCIDABLE

`/api/viz/space` et `/api/viz/modules` servent jusqu'à **200 ensembles**
(`TOP_EFF = 200`, triés par effectif décroissant). Le front, lui, **coupait à 24
avant de lire la longueur** :

```ts
groupes: (m.modules || []).filter((x) => x.count > 1).slice(0, FRACTALE.groupesMax)
setInterieur({ n: modules.length, total: tousModules.length, … })   // 24 ET 24
```

Le panneau possédait la phrase « N sur total » : elle ne pouvait **pas** s'afficher,
faute d'un total réel. Résultat : 176 groupes invisibles, sans un mot, et **aucun
geste** pour les voir — le seul geste de « plus » était `⤵`, qui DESCEND d'un cran
et ne répond donc pas à « montre-moi ce qui manque ICI ». Un plafond silencieux ne se
distingue pas d'une donnée absente.

### La loi, écrite une fois et testée sans navigateur

`packages/obrain-dashboard/src/graph/densite.ts` (module **pur**, aucun import) :

| Règle | Situation | Geste |
|---|---|---|
| 1 | un groupe est DÉSIGNÉ (`intention: "dedans"`) | **approfondir** (une direction désignée est une direction) |
| 2 | `total > montres` (matière reçue non dessinée) | **densifier**, sans appel — le niveau NE BOUGE PAS |
| 3 | tout est montré, mais le SERVEUR a coupé (`tronque`) | **densifier par appel** (`offset` suivant), et on le DIT |
| 4 | niveau FEUILLE entièrement montré | **rien** (ni ici, ni en dessous) |
| 5 | tout est montré, des sous-groupes existent | **approfondir** |

`densite.test.mjs` : **13/13**, dont l'invariant qui sépare les deux gestes —
`bougeLeNiveau === (geste === "approfondir")` sur 2 646 combinaisons d'état.

### La pagination du call, côté serveur

`/api/viz/space` accepte désormais `offset` : `ORDER BY c DESC SKIP offset LIMIT 200`,
`offset` **dans la clé de cache** (sans quoi la page 2 se servirait la page 1 depuis le
cache — un mensonge silencieux) et dans le `extra` du cache disque ; la réponse publie
`offset` et `tronque` (page pleine). Vérifié sur une copie de store que `SKIP` s'applique
**APRÈS l'agrégat** : sur 6 587 nœuds tenant dans UN groupe, `SKIP 1` rend `[]` et non le
même groupe amputé — un `SKIP` avant agrégation aurait rendu `c = 6586`.

### Les surfaces qui se branchent mécaniquement (toutes alignées)

| Surface | Ce qui a changé |
|---|---|
| **Cosmos** (`CosmosWeave.tsx`) | l'étage GARDE la liste complète (`it.tous`), dessine `it.page` ; geste `＋ densifier` dans la barre et le panneau ; `d` au clavier ; `estFeuille` porte sur le NIVEAU |
| **Atlas / Monde** (`StageContinu.tsx`) | `NoeudFractal.tous`/`page` ; `＋ densifier` dans le panneau ; `d` ; la feuille se juge sur le niveau |
| **Monde tissé** (`StoreOverview.tsx`) | sa pagination (+48, réglage propre) rejoint `pageSuivante`/`PAGE.pas` — une seule loi, un seul pas |
| **Grammaire** (`grammar.ts`) | action `densifier` (touche `d`), libellé `VIEW_KEYS_HINT` mis à jour |
| **État publié** (`__etages.organisation`, `__fractale`) | `tous`, `montres`, `page`, `geste`, `densifiable`, `plafondAtteint` — la décision est MESURABLE |
| **Contrôle** (`echelle-multi-niveaux-check.mjs`) | §9 : densifier augmente `montres` **sans** toucher `prefixe`/`rang` et **sans appel** ; approfondir les change ; compte des `/api/viz/space` émis |

### Deux défauts trouvés PAR LE CONTRÔLE, puis corrigés

Le §9 a d'abord ÉCHOUÉ, et c'est la mesure qui a parlé — pas la relecture :

| Mesure du contrôle | Cause | Correctif |
|---|---|---|
| « 24 sur 56 » → **« 8 sur 8 »** au même préfixe et au même rang | `chargerInterieur` recalcule la SOURCE du découpage à chaque appel (agrégat serveur ↔ échantillon du graphe) : le redessin pouvait changer de découpage sous le geste | la source est RETENUE sur l'étage (`it.sourceAxe`), et un redessin redessine le même |
| **1 appel** `/api/viz/space` émis par « densifier » | le geste rappelait le chargeur complet, qui réémet la requête du niveau | mode `redessin` : source = `it.tous`, fibres = `it.arcs`, **zéro requête** |

### Vérifié, mesuré

Localement : `tsc -b` vert · `vite build` vert · `cargo check -p obrain-cli` vert ·
`densite.test.mjs` **13/13** · `SKIP` après agrégat prouvé sur deux stores COPIES (jamais sur
un store servi : `obrain query` prend un verrou EXCLUSIF — constaté sur `kv-linux-e1000`).

Bout en bout, sur une instance ISOLÉE (binaire de ce checkout sur 8433, `dist` de ce
checkout, chromium Playwright en CDP) :

```
node scripts/echelle-multi-niveaux-check.mjs http://127.0.0.1:8433
  ── 9. DENSIFIER OU APPROFONDIR
  niveau courant : préfixe="deepseek_harness.packages" rang=5 dessinés=24 reçus=56
  ✓ le niveau ANNONCE plus qu'il ne dessine — 24 dessinés sur 56 reçus
  après « d densifier » : préfixe="deepseek_harness.packages" rang=5 dessinés=48 reçus=56
                         appels /api/viz/space émis : 0
  ✓ densifier DESSINE plus — 24 → 48
  ✓ densifier NE CHANGE PAS de niveau (préfixe et rang identiques)
  ✓ densifier un niveau déjà reçu n'émet AUCUN appel — 0 appel(s)
  après « Page ↓ approfondir » : préfixe="deepseek_harness.packages.client" rang=6
  ✓ approfondir CHANGE de niveau (préfixe allongé ou rang +1)
  geste offert à l'écran : ＋ densifier 18
✓ TOUT EST TENU
```

Fragilité rencontrée, NOTÉE et non corrigée (hors périmètre) : `viz serve` meurt sur
`Broken pipe (os error 32)` quand un client se déconnecte pendant une grosse réponse — un
`Page.navigate` du contrôle pendant un sous-graphe de 20 k nœuds suffit. La vérification a
donc été relancée sous une boucle de redémarrage. Une instance de service ne devrait pas
mourir parce qu'un navigateur a fermé un onglet.

### Reste ouvert

- **Au-delà de 200** : densifier devient un APPEL (`offset`). Le chemin existe
  (`pageServeur`, règle 3 de la loi) mais n'a pas été exercé en vrai : il demande un store
  dont un niveau porte plus de 200 groupes. À mesurer avant de le considérer tenu.
- Cette lignée n'est pas déployée : le 8432 sert toujours `20260915-153730-…`, et le
  `dist` de ce checkout n'est lié à aucun magasin d'UI. Promotion =
  `scripts/ui-deploy.sh --depuis packages/obrain-dashboard/dist --id densifier-ou-approfondir`.

## 14. Vérification indépendante : trois défauts trouvés par la mesure, et la portée RÉELLE de la règle 3 (2026-09-15, 16:45)

Reprise du §13 : la loi (`densite.ts`) est écrite et testée, le câblage des surfaces existe.
Ce passage l'a **mesuré de bout en bout**, a corrigé trois défauts que la relecture ne voyait
pas, et **borne** ce que la règle 3 couvre vraiment.

### Trois défauts, chacun trouvé par une mesure

| Défaut | Mesure | Correctif |
|---|---|---|
| **La règle 3 ne s'armait plus** dès que nos filtres retiraient une ligne | `/api/viz/space?store=runtime-dev&rank=4&prefix=deepseek_harness&top=200` → `tronque: true`, 200 lignes, dont un groupe au nom **VIDE** portant 8 438 nœuds, écarté par `tokenValide`. Le total filtré retombe sous 200, or la règle exigeait `total >= 200` : `decisionDensite({total:150, montres:150, plafondAtteint:true})` répondait **approfondir** | la règle 3 (et `densifiable`, et `suiteChezLeServeur`) ne dépend plus que de `plafondAtteint` — **ce que le serveur dit**, que le total filtré ne peut pas contredire. Test de régression ajouté (`densite.test.mjs` : 13 → **15/15**) |
| **Le `SKIP` comptait des groupes filtrés** | `offset = iv.tous.length` (filtré) alors que `SKIP` porte sur des lignes BRUTES → ~50 lignes de recouvrement par page sur le store ci-dessus | `NiveauFractal.bruts` (lignes reçues) + `it.offsetBrut` sur l'étage ; le geste avance désormais du compte BRUT |
| **L'Atlas offrait un geste qu'il refuse** | `densifiable` était publié vrai dès que la loi disait `densifier` — y compris `requiertAppel` — alors que `densifierEchelleRef` sort justement sur ce cas : une puce « ＋ densifier » qui ne fait RIEN | l'OFFRE est bornée à ce que le geste sait faire (`geste === "densifier" && !requiertAppel`) ; la loi continue de publier son verdict dans `geste` |

### La portée réelle de la règle 3 — le point qu'il faut retenir

Descente mesurée sur `runtime-dev` (`Page ↓`, état publié `__etages.organisation`) :

```
prefixe=""                             rang=3  tous=6   plafondAtteint=false
prefixe="deepseek_harness"             rang=4  tous=17  plafondAtteint=false
prefixe="deepseek_harness.packages"    rang=5  tous=56  plafondAtteint=false
prefixe="…packages.client"             rang=6  tous=42  plafondAtteint=false
prefixe="…client.runtime"              rang=7  tous=4   plafondAtteint=false
```

**Aucun niveau atteint par la descente n'a `plafondAtteint`.** La cause est structurelle :
à un niveau descendu, les groupes viennent soit de l'AXE, soit de `suivantsDuNiveau` — les
deux calculés sur l'**échantillon du graphe**. `enfantsDe` (l'agrégat du serveur, donc la
seule source paginable par `offset`) n'est la source que quand l'échantillon du graphe ne
rend pas deux tokens. Conséquences, à traiter dans cet ordre :

1. **« Densifier » sur un niveau issu du GRAPHE n'est pas une pagination `offset`** : le
   « plus » y est un budget plus large (`limit`/`hops` du sous-graphe). Tant que ce chemin
   n'existe pas, la règle 3 reste **théorique** dans la navigation de ce store, et
   « densifier » y est toujours un redessin (règle 2) — ce qui marche, mais ne couvre pas
   le cas que le §13 laissait ouvert.
2. **Le cran « racines » ne peut pas paginer du tout** : `/api/viz/modules` n'a pas
   d'`offset` (vérifié : le paramètre est ignoré, la réponse ne publie ni `offset` ni
   `tronque`). Asymétrie assumée, mais elle doit être dite au panneau plutôt que découverte.

### Vérifié par ce passage (mesures, pas relecture)

- `tsc -b` vert · `vite build` vert · `cargo check -p obrain-cli` vert · `densite.test.mjs` **15/15**.
- **Serveur, par `curl`** (instance isolée, binaire de ce checkout) : `offset=1` change la
  TÊTE de la liste (preuve que `SKIP` s'applique après l'agrégat), `offset=200` rend une page
  **différente** de `offset=0` (`allowNullish`, `allowNumberAndString`… au lieu de
  `packages`, `""`, `examples`…), et `tronque: true` est publié. La pagination au-delà de
  200 est donc **réelle** — seul son déclenchement par l'UI reste à porter (ci-dessus).
- **Contrôle complet, instance isolée** (binaire + `dist` de ce checkout sur 8455, chromium
  Playwright en CDP, sans IAM) : `✓ TOUT EST TENU — quatre crans, descente continue et sans
  recul, groupes auto-segmentés, étage des signaux réel, six axes polymorphes`, §8 et §9
  compris (`24 → 48 sur 56`, préfixe et rang **identiques**, `0` appel `/api/viz/space`).
  Mesuré avant les trois correctifs sur le bundle `index-8uVgesZD` ; relancé après.
- 19 `console.error` non bloquantes, **toutes expliquées** : `/ui/state/*` (connu, §« CONNU »)
  et `/api/viz/subgraph/stream` en **404**. Ce dernier est un écart de LIGNÉE, pas un défaut
  neuf : la route et `crates/obrain-cli/src/viz_ws.rs` existent sur
  `consolidation/tronc-release-2026-09-15`, `fusion/tronc-plus-j6-mes-correctifs` et
  `union/nav-et-rendu-20260915` — **pas** sur cette branche. Le client WS tente d'abord,
  échoue, retombe en HTTP : invisible à l'écran, mais `ws-client` est une capacité **suivie**
  du contrat (`scripts/ui-contrat.json`) et n'est donc tenue que par le client.

### La surface non couverte : l'appel d'OUTIL a le même aiguillage, et ne le distingue pas

C'est la surface « à la suite » que la navigation n'atteint pas, et elle est mesurée :

- `outils.rs` (`rendre`) écrit au modèle : `[COUPÉ au plafond de N caractères : 17 lignes sur
  29 affichées — affine la requête.]` — le total est DIT (bien), mais le seul remède proposé
  est « affine », jamais « demande la page suivante » (`SKIP`/`LIMIT`) ni « descends » ;
- `engine.rs` (`POUR DENSIFIER`) propose trois gestes — un type précis, le voisinage d'un
  nœud cité, les modules du projet — qui sont tous des **approfondissements** ; la
  pagination n'y est pas nommée ;
- `obrain_echantillon` prend `n` (≤ 30) et **pas** d'`offset` : densifier un type au-delà de
  30 n'est pas offert. Seul `mcp_server.rs` (`session_load`) expose `limit` **et** `offset`.

Autrement dit : le même choix « densifier par pagination / approfondir » existe côté agent,
et y est tranché par défaut en faveur de l'approfondissement.

## 15. La pagination exercée POUR DE VRAI, et quatre défauts de plus (2026-09-15, 20:40)

Le §14 laissait la règle 3 (densifier au-delà de la page serveur) « réelle mais non
atteignable ». Ce passage l'a **exercée de bout en bout**, et il a fallu corriger quatre
défauts pour que le geste cesse de mentir. Mesure finale, instance isolée, `runtime-dev`,
niveau `deepseek_harness` (rang 4, 75 groupes reçus après filtres) :

```
densification 1 : montres=48  · 0 appel   (en mémoire)
densification 2 : montres=72  · 0 appel   (en mémoire)
densification 3 : montres=75  · 0 appel   (tout est dessiné)
densification 4 : offsetBrut=400 pagesVides=1 · 1 appel  ...&offset=200&links=0
densification 5 : offsetBrut=600 pagesVides=2 · 1 appel  ...&offset=400&links=0
                  → plafondAtteint=false, geste=approfondir, densifiable=false
densifications 6-8 : PLUS RIEN (aucun appel émis)
  raison publiée : « les pages suivantes du niveau n'apportent aucun groupe d'effectif > 1 »
```

Le niveau n'a **jamais** bougé (préfixe et rang identiques du premier au dernier geste) :
densifier paie des appels, approfondir change de cran — la distinction demandée est
maintenant mesurable dans les deux sens.

### Quatre défauts trouvés, tous par la mesure

| Défaut | Mesure qui l'a révélé | Correctif |
|---|---|---|
| **Le redessin ÉTEIGNAIT le verdict du serveur** | `plafondAtteint: true` sur un niveau à page pleine, `false` APRÈS une densification **en mémoire** — la page 2 n'était donc plus jamais demandée | le verdict est RETENU au redessin (`if (!redessin)`), comme `tous`, `arcs`, `sourceAxe` |
| **« page sans groupe » confondu avec « plus rien »** | `offset=200` rend 200 lignes dont l'effectif **maximal est 1** → 0 groupe après `count > 1` ; le code sortait sur `!p.tous.length` et **réémettait la même page** (5 appels identiques, zéro état changé) | deux cas distincts : `p.bruts === 0` → fini ; `p.bruts > 0` sans groupe gardé → page vide comptée |
| **Le geste tournait à vide, sans fin** | 8 gestes → 5 appels identiques, l'offre ne s'éteignait jamais | au-delà de **2** pages vides, l'offre s'éteint **et le dit** (`pagesVides`, `offsetBrut` publiés) |
| **L'offre survivait à l'état** | état interne éteint (plus aucun appel) mais `__etages` publiait encore `plafondAtteint: true` → puce offerte qui ne fait rien | le verdict est republié au moment où il change |

### Ce que la mesure apprend sur la donnée elle-même

L'agrégat est trié par effectif **décroissant** : les pages 2..N sont donc le **petit
reste**, et après le filtre `count > 1` de l'interface elles ne contiennent plus rien
d'exploitable (mesuré : effectif maximal 1 sur 200 lignes). La pagination est donc RÉELLE
(le `SKIP` s'applique, l'`offset` part, les pages diffèrent) mais son **rendement** est
faible par construction : deux pages suffisent à conclure, et le geste le dit au lieu de
laisser croire qu'il reste du sens à aller chercher.

### Deux corrections de mes propres conclusions du §14

1. **« Aucun niveau atteint n'a `plafondAtteint` »** est FAUX tel quel. Mesuré depuis :
   le niveau `deepseek_harness` (rang 4) arrive avec `plafondAtteint: true` et
   `source: "sous-groupes"` — donc par l'AGRÉGAT serveur, pas par le graphe. La source d'un
   niveau descendu **alterne** : `sous-groupes` (agrégat serveur → page `offset`) quand
   l'échantillon du graphe ne rend pas deux tokens, `axe`/`suivants` (échantillon → budget
   de centres) sinon. Les deux chemins existent donc, et la règle 3 bis n'est pas un
   pis-aller : c'est l'autre moitié du même choix.
2. **Le budget de centres était un 32 EN DUR** (`rechargerInterieur`) et le garde de
   `chargerGraphe` refusait tout rechargement au même centre : une densification par
   échantillon aurait été annoncée puis silencieusement ignorée. Le budget vit désormais
   sur l'étage (`centresGraphe`, 32 → 64 → … → 256) et le garde distingue « même centre »
   de « même centre AVEC plus de centres » — ce dernier étant une densification, au même
   titre qu'un centre différent est un recentrage.

### Reste ouvert (mis à jour)

- **L'échantillon du graphe** (`source: "graphe"`) : le chemin existe (budget de centres,
   règle 3 bis, garde corrigé) mais n'a pas encore été exercé en vrai sur un niveau où la
  source EST le graphe — il demande une trajectoire où `suivantsDuNiveau` réussit.
- **`/api/viz/modules` n'a pas d'`offset`** : le cran « racines » ne peut pas paginer chez
  le serveur (vérifié : paramètre ignoré, ni `offset` ni `tronque` publiés).
- **L'Atlas** (`StageContinu`) ne paie pas la page serveur (`requiertAppel` → refus, et
  l'offre est bornée en conséquence) : c'est dit, pas caché.
- La fragilité `Broken pipe` de `viz serve` reste : elle a tué une instance pendant ces
  mesures (relancée depuis).

## 16. Le blocage des crans : le vol est PARFAIT, c'est le MONDE qui n'est pas le même (2026-09-15, 22:20)

Les quatre boutons de crân ne produisaient rien : `t` figé vers 0,12, voyant resté
« 🌌 Cosmos », aucun graphe jamais demandé. Instrumenté autorité par autorité — jamais supposé.

### Ce que chaque instrument a établi (et écarté)

| Mesure | Verdict |
|---|---|
| 23,6 images/s, **zéro exception** | la boucle vit : ni boucle morte, ni plantage |
| `crans = {demandes:2, appliques:2, sansMonde:0}` | la demande **est appliquée** (ce n'est pas un refus) |
| `vol = {on:false, k:1, vers:0, total:903}` | **le vol ARRIVE exactement sur sa cible** — 0,42 s, jamais annulé |
| `ctrl.minDistance` = `frac 0,08 × R` ≈ 4,5 | le plancher de molette n'est **pas** la butée |
| `espaces = {d_locale:748, d_monde:748, R_local:56,4, echelle_parent:1,8}` | les deux espaces **coïncident** : ce n'était pas une confusion local/monde |
| `crans.mondeVise = world:aletheia-chat-locale-n3-gelee-12-09 (R 25,99)` | le cran vise **un** monde |
| `espaces.R_local = 56,4` | le palier — donc le voyant — se mesure sur **un AUTRE** |

**La cause est là : deux mondes pour un seul cran.** Le vol porte la caméra exactement où
il l'a décidé (`vers: 0`), et la question « suis-je arrivé ? » est posée à un autre monde,
situé 748 unités plus loin. Le cran ne peut donc JAMAIS s'allumer : il a raison, et on lui
répond sur autre chose.

### Le piège que j'ai posé moi-même, et qui ne se voyait pas

J'ai d'abord fait adopter le monde du cran par le suivi (`focusQnRef.current = cibleN`) :
**aucun effet mesuré**. Raison : `focusQnRef.current = focusQn` est réécrit à CHAQUE rendu
(l. 693) — une écriture dans la boucle est donc effacée au rendu suivant. Un `ref` qui
re-synchronise un état React n'est pas une autorité : c'est un cache. Il faut écrire
l'ÉTAT (`setFocusQn`), pas la référence.

### Les instruments ajoutés (ils restent : c'est ce qui a permis de trouver)

- `__etages.crans` : `demandes`, `appliques`, `sansMonde`, `dernier`, et l'UNITÉ du vol
  (`mondeVise`, `rayonVise`, `distanceVisee`, `distanceMonde`) ;
- `__etages.espaces` : `d_locale`, `d_monde`, `R_local`, `R_monde`, `echelle_parent` —
  une longueur sans son espace est un chiffre qu'on croit à tort ;
- `__etages.vol` : `on`, `k`, `de`, `vers`, `total`, `cible`, `cam` — l'état du vol
  lui-même, la dernière autorité qui n'était pas mesurée ;
- **la tenue de cran** : quand le palier MESURÉ retombe sous le cran demandé, la distance
  est rétablie (direction conservée), **bornée à 3 reprises** et comptée (`crans.reprises`,
  `-1` = « ne tient pas ici »). Elle fonctionne (`reprises` 1 → 3 observés) — mais elle ne
  peut rien tant que la cible et la mesure ne parlent pas du même monde.

### Le correctif, précis (une seule autorité de monde)

Le cran doit cadrer **le monde que le palier mesure** — ou le palier doit mesurer **le monde
que le cran a cadré**. Concrètement : `setFocusQn(cibleN)` après le vol (l'état, pas la
référence), et faire lire `cible`/`node` de la boucle sur ce même `focusQn`. Une ligne
d'écriture, une ligne de lecture, et le voyant ne peut plus contredire le vol.

### Reste ouvert

- Le disque de la machine a atteint **0 octet libre** pendant ces mesures (100 %) : le
  serveur `viz serve` mourait en « Broken pipe », chromium disparaissait en pleine passe
  (`Target : closing`, deux fois), et `npx tsc` a échoué en `ENOSPC`. Une partie des
  instabilités de §1/§8 vient de là, pas du produit. Le `/tmp` porte ~100 G de copies de
  stores d'autres passages, et `target/` 47 G — un nettoyage est à décider par le détenteur.
- `scripts/echelle-multi-niveaux-check.mjs` doit être relancé **entier** (sans `head` :
  deux mesures ont été tronquées par mon propre instrument) après le correctif ci-dessus.


## 17. REPRENDRE L'ÉCHELLE À N'IMPORTE QUEL MOMENT

Tout le savoir du multi-échelle — la loi des quatre crans et leurs distances, la loi
« densifier ou approfondir » (`densite.ts`), le curseur vertical et ses deux pièges,
les six défauts corrigés et les **deux défauts encore ouverts** (deux distances pour une
seule échelle ; `tS` qui ne s'engage pas au cran « signaux »), les instruments à lire
(`__etages.*`), les commandes exactes pour relancer une instance, un chrome de mesure et
les contrôles, et les cinq pièges d'instrument déjà payés — est rassemblé dans :

    docs/ECHELLE-MULTI-NIVEAUX-REPRISE-2026-09-16.md

C'est le document à ouvrir en premier : il dit ce qui est vrai, ce qui est cassé, où le
lire, et comment tout relancer.

Le savoir du **chat, des conversations et de l'engine** (découplage du graphe, structuration
des échanges, graphe vide à la première connexion, coupe-circuit de répétition, arrêt quand
le client part, budget par question) a son propre volet :

    docs/CHAT-ET-ENGINE-REPRISE-2026-09-16.md

Et les deux volets ont un **résumé importable** dans le graphe, prêt pour le processus qui
détient l'écriture :

    docs/ADR-2026-09-16-ECHELLE-MULTI-NIVEAUX.md

**Tentative d'écriture directe, mesurée le 2026-09-16** : `manage_adr` répond
`écriture impossible (store en lecture seule ?)` et `import_docs` ferme la connexion —
c'est l'invariant qui protège le WAL (un seul processus détient `OBRAIN_WRITE=1`), et c'est
pourquoi le véhicule du tissage est le DOCUMENT (voir `ba9b018a`).
