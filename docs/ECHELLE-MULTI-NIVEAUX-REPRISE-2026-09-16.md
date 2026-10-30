# L'ÉCHELLE MULTI-NIVEAUX — document de reprise

> But de ce document : **pouvoir reprendre le travail sur l'échelle à n'importe quel
> moment**, sans rejouer l'histoire. Il dit ce qui est vrai (mesuré), ce qui est
> cassé (mesuré aussi), où le lire, et comment tout relancer.

Écrit le 2026-09-16, branche `consolidation/tronc-2026-09-15-plus-fusion`.

---

## 1. Ce qu'est l'échelle, en une page

L'échelle est **une seule scène**, parcourue à **quatre crans** — et la descente entre
eux est **continue**, pas une commutation (mesuré : 9 paliers sans saut ≥ 20 points).

| cran | nom | ce que la scène montre | distance visée (pour un monde de rayon `R`) |
|---|---|---|---|
| 0 | **Espace** | toutes les bases déployées, la toile de l'univers | `14 R` |
| 1 | **Monde** | l'organisation d'un monde : ses groupes, comptés par le serveur | `distancePourT(0,45, R)` |
| 2 | **Groupe** | le graphe réel DANS son groupe (K3/K4/∂/synapses) | `distancePourT(0,85, R)` |
| 3 | **Signaux** | la même matière **réordonnée** par courbure · énergie · degré | `0,08 R` |

Le **palier** est l'autorité unique d'échelle : c'est le même calcul qui décide le mot
affiché, le cran allumé, l'ouverture des étages et le mélange vers les signaux.

> **Où vit la loi, au 2026-09-16** : dans le module **`packages/obrain-dashboard/src/graph/echelle.ts`**
> (`palierDe`, `distancePourT`, `distancePourPalier`, `viserCran`, `bandeDe`, `milieuBande`,
> `nomPalier`, `PALIERS`), **testé par `echelle.test.mjs` — 22 cas** (commit `d5efa94c` :
> « DES TESTS POUR LES COINS — et deux d'entre eux ont trouvé un défaut réel »).
> `CosmosWeave.tsx` l'IMPORTE ; il ne la redéfinit plus. Un document qui situait la loi
> « en tête de `CosmosWeave.tsx` » désignait l'ancien emplacement — corrigé ici.

Les bandes :

```
BANDE_MONDE = 0,34      d > distancePourT(0,34,R)              → palier 0 (espace)
BANDE_DEDANS = 0,72     d > distancePourT(0,72,R)              → palier 1 (monde)
PALIER_DEDANS_SIGNAUX = 0,25                                → palier 2 (groupe)
PALIER_SIGNAUX = 0,08   sinon                                  → palier 3 (signaux)
distancePourT(t,R) = 14R · exp(−t · ln(56))     (échelle LOGARITHMIQUE : chaque
                                                 doublement de distance prend la
                                                 même part de course qu'une carte)
```

**Entrer dans un monde n'est pas s'approcher, c'est franchir sa paroi** — et la coquille
ne se ferme qu'au franchissement, jamais « en approchant » (correction mesurée).

---

## 2. Le contrôle : un CURSEUR vertical, plus quatre boutons (2026-09-16)

Quatre boutons disaient « on est à un cran », pas « où l'on est **entre** eux », alors
que la descente est continue. L'échelle est donc un **`input[type=range]` vertical**
(`writing-mode: vertical-lr`, à `z-index: 42`, à gauche, centré verticalement), et
**chaque cran porte sa matière**, lue sur les mesures que la scène publie déjà :

```
🌌 Espace   →  « 17 espaces · 450 662 nœuds ici »
🧠 Monde    →  « 450 662 nœuds · 1 349 794 arêtes »   (ou « N groupes (M dessinés) · axe »)
🔺 Groupe   →  « graphe ouvert à 100 % »
🧬 Signaux  →  « mélange à … % (courbure · énergie · degré) »
```

**Sens de l'axe** : le minimum (Espace) est **en haut**, le maximum (Signaux) en bas —
comme une coupe, on descend pour entrer. Vérifié au clic : haut 10 % → 0, milieu 35 % → 1,
bas 90 % → 2.

**Deux pièges déjà payés** (ne pas les repayer) :
- `z-index: 6` laissait la **barre de scène** (`.echelle-bar`) recouvrir le curseur :
  `elementFromPoint` sur un cran répondait `DIV.echelle-bar` et le clic ne faisait RIEN.
- `direction: rtl` **inverse l'axe** en écriture verticale : ma documentation disait
  « espace en haut », la mesure donnait le contraire.

Les libellés des crans portent un cran par ligne : **le contrôle ne doit lire que la
PREMIÈRE ligne** de `.ec-cran.on` (sinon il compare le libellé **et** les données).

---

## 3. La loi « densifier ou approfondir » (module PUR `densite.ts`)

Le défaut d'origine, mesuré : un niveau recevait jusqu'à **200 groupes** du serveur,
n'en **dessinait que 24**, et lisait `total` **dans la liste déjà coupée** — donc le
panneau annonçait « 24 sur 24 » et les 176 autres n'étaient atteignables par **aucun**
geste. Le seul « plus » était `⤵`, qui **change de cran**.

La loi, écrite une fois (`packages/obrain-dashboard/src/graph/densite.ts`, testée par
`densite.test.mjs`, **18 cas**) :

| règle | situation | geste |
|---|---|---|
| 1 | un groupe est DÉSIGNÉ (`intention: "dedans"`) | **approfondir** |
| 2 | `total > montres` (matière reçue non dessinée) | **densifier**, sans appel |
| 3 | tout est montré, le SERVEUR a coupé (`plafondAtteint`) | **densifier par appel** (`offset`) |
| 3 bis | niveau découpé sur un ÉCHANTILLON du graphe (`couverture < 50 %`) | **densifier** = élargir l'échantillon (plus de centres) |
| 4 | niveau FEUILLE entièrement montré | **rien** |
| 5 | tout est montré, des sous-groupes existent | **approfondir** |

Invariant qui sépare les deux gestes : `bougeLeNiveau === (geste === "approfondir")`.

**Ce que la mesure a appris depuis** :
- l'agrégat est trié par effectif DÉCROISSANT : les pages 2..N sont le **petit reste**
  (mesuré : 200 lignes d'effectif maximal **1**), donc densifier par `offset` paie un
  appel et ne rapporte souvent rien — d'où l'arrêt **après 2 pages vides**, qui se dit ;
- `/api/viz/modules` (cran « racines ») **n'a pas d'`offset`** : le cran racine ne peut
  pas paginer côté serveur ;
- un niveau descendu est le plus souvent découpé sur un **échantillon** (`couverture`
  publiée) ; la règle 3 bis est la densification qui s'y applique.

---

## 4. Ce qui a été corrigé sur les crans (et pourquoi)

| défaut mesuré | correctif |
|---|---|
| Le cran « monde » ne s'allumait jamais : le vol visait **un** monde, le palier en mesurait **un autre** (748 unités plus loin) | `setFocusQn(String(cibleN))` après le vol — **l'ÉTAT, pas la référence** (`focusQnRef` est réécrite à chaque rendu : y écrire est effacé) |
| L'offre « densifier » restait affichée alors que le geste ne faisait rien | l'OFFRE est bornée à ce que le geste sait faire ; le verdict continue d'être publié dans `geste` |
| À l'Atlas, une puce « ＋ densifier » ne faisait rien (`requiertAppel` refusé) | l'offre exige `!requiertAppel` |
| Le redessin ÉTEIGNAIT le verdict du serveur (`plafondAtteint` vrai → faux) | le verdict est RETENU au redessin, comme `tous`, `arcs`, `sourceAxe` |
| « page sans groupe » confondu avec « plus rien » → la MÊME page était réémise (5 appels identiques) | deux cas distincts : `p.bruts === 0` (fini) vs page vide comptée |
| Le `SKIP` comptait des groupes **filtrés** (recouvrement) | `NiveauFractal.bruts` + `it.offsetBrut` (lignes brutes) |

---

## 5. Ce qui est CASSÉ aujourd'hui, mesuré, et où c'est

### 5.1 Deux distances pour une seule échelle (le défaut principal)

Mesuré au curseur, cran par cran (`__etages.crans.tenue`) :

```
cran 1 (Monde)   {niv:1, d:62,54, R:27,56, palier:1, reprises:0}   visée 63,05 → ATTEINTE
cran 2 (Groupe)  {niv:2, d:12,53, R:27,56, palier:2, reprises:0}   visée 12,60 → ATTEINTE
cran 3 (Signaux) {niv:3, d: 4,51, R:27,56, palier:3, reprises:0}   visée  2,20 → palier 3 ATTEINT
```

**Le vol arrive, le palier demandé est atteint.** Mais `t` et `camDist` — donc **la
barre, le % du curseur et le contrôle d'acceptation** — mesurent une **autre** distance
(`camera ↔ ctrl.target` : 338 / 270 / 225) que celle du palier
(`camera ↔ cible.mesh.position` : 62,5 / 12,5 / 4,5). Le contrôle juge donc la descente
sur la mauvaise des deux, et croit à un **recul** au cran « signaux ».

→ **Pas suivant** : faire mesurer `t`/`camDist` sur la **même** distance que le palier
(une seule autorité de distance, comme on a unifié l'autorité de monde).

### 5.2 `tS` reste à 0 au cran « signaux »

Le palier 3 est atteint, mais le **mélange** vers l'espace des signaux ne s'engage pas.
Or le niveau annoncé exige `p >= 3 && tS > 0,02 && nGraph > 0,5` : le cran est donc
honoré par la **distance** et pas par le **mélange**, d'où un voyant qui reste « 🔺 Groupe ».
(Le cran 4 a DÉJÀ fonctionné : mesuré `tS = 1` avec `origineGraphe` et `origineSignaux`
vrais — donc la question est « pourquoi pas ici », pas « est-ce que ça marche ».)

### 5.3 Flakiness connue de l'instrument, pas du produit

- `§8`/`§9` du contrôle échouent **en fin de passe** (pages fraîches où l'étage ne se
  charge pas) alors que `§4` — même type de page, plus tôt — passe : re-mesurer avec un
  chrome NEUF avant de conclure ;
- le chrome de mesure **meurt** après plusieurs scènes WebGL lourdes (le fichier du
  contrôle le dit : relancer un chrome neuf) ;
- le disque de la machine a atteint **0 octet** pendant ces mesures : serveurs morts en
  « Broken pipe », chromium disparu, `npx tsc` en `ENOSPC`. Ce n'étaient **pas** des
  défauts du produit.

---

## 6. Les instruments : où lire quoi

Tout est publié par la scène dans `window.__etages` (le contrôle et le curseur s'en
nourrissent — un seul calcul, deux lecteurs) :

| clé | ce qu'elle dit |
|---|---|
| `crans` | `demandes` / `appliques` / `sansMonde` / `dernier`, l'UNITÉ du vol (`mondeVise`, `rayonVise`, `distanceVisee`) et le **verdict de la tenue** (`tenue.{niv,d,R,palier,reprises}`) |
| `vol` | l'état du vol : `on`, `k`, `de`, `vers`, `total`, `cible`, `cam` (`vers: 0` = la caméra est SUR sa cible) |
| `espaces` | `d_locale` / `d_monde`, `R_local` / `R_monde`, `echelle_parent` — une longueur sans son espace est un chiffre qu'on croit à tort |
| `organisation` | le niveau : `prefixe`, `rang`, `tous`, `montres`, `page`, `plafondAtteint`, `sourceDensite`, `couverture`, `geste`, `densifiable`, `offsetBrut`, `pagesVides` |
| `monde` | `t`, `camDist`, `fermeture`, `verrou`, `cage`, `axe{axe,groupes,plus,n}`, `signaux{tS, melange, origineGraphe, origineSignaux}` |
| `rapport` | les entrées du verdict de niveau : `palier`, `nInter`, `nGraph`, `tS`, `reel` |

**Côté moteur** (Rust) :
- `engine : client parti — génération arrêtée au tour N` (niveau `WARN`) : le client a
  fermé, la génération cesse ;
- `repetition.rs` : coupe-circuit de répétition (bloc ≥ 40 car. × ≥ 3, ou ligne ≥ 8 × ≥ 6,
  texte ≥ 600 car.), avec la phrase affichée à l'utilisateur ;
- budget par question : `BUDGET_QUESTION_S = 300` (la somme des appels au modèle).

---

## 7. Reprendre : les commandes exactes

```bash
# 0. Les portes (elles arrêtent, elles ne rapportent pas)
cd packages/obrain-dashboard && ./node_modules/.bin/tsc -b          # TypeScript
cd ../.. && cargo test -p obrain-cli --bin obrain                   # 228 tests
npx tsc src/graph/densite.ts --outDir /tmp/dens-test --module esnext \
  --target es2020 --moduleResolution bundler --strict \
  && node src/graph/densite.test.mjs                                # 18 cas (loi densifier/approfondir)
npx tsc src/graph/conversations.ts --outDir /tmp/conv-test --module esnext \
  --target es2020 --moduleResolution bundler --strict \
  && node src/graph/conversations.test.mjs                          # 17 cas (structurer les conversations)
npx tsc src/graph/echelle.ts --outDir /tmp/ech-test --module esnext \
  --target es2020 --moduleResolution bundler --strict \
  && node src/graph/echelle.test.mjs                                # 22 cas (la loi d'échelle)

# 1. Une instance ISOLÉE (jamais le 8432 servi, qui est une autre lignée)
cargo build --release -p obrain-cli --bin obrain
./target/release/obrain viz serve --host 127.0.0.1 --port 8455 --enrich-ricci 0 \
  --preset-default "Runtime dev (engrams)" \
  --preset "Runtime dev (engrams)=/home/triviere/.local/share/obrain-harness/data/runtime-dev.obrain" \
  --synapse-store /home/triviere/.local/share/obrain-harness/data/agent-synapse-triviere-dsh.obrain

# 2. Un chrome NEUF pour la mesure (CDP) — le bundle servi est celui de CE checkout
pkill -f 'remote-debugging-port=9248'
~/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome --headless=new \
  --remote-debugging-port=9248 --remote-allow-origins='*' --no-sandbox \
  --disable-dev-shm-usage --enable-unsafe-swiftshader \
  --user-data-dir=/tmp/cdp-reprise about:blank &

# 3. Le contrôle d'échelle — TOUJOURS dans un fichier, jamais dans un `head`
node scripts/echelle-multi-niveaux-check.mjs http://127.0.0.1:8455 > /tmp/rapport.txt 2>&1
grep -E "^  ✗|exigence|TOUT EST TENU" /tmp/rapport.txt
```

**Autres contrôles de la même famille** : `scripts/fractale-check.mjs` (descente
récursive), `scripts/continuum-check.mjs`, `scripts/views-controls-check.mjs`,
`scripts/fratrie-check.mjs`, `scripts/cosmos-capture.mjs` — tous pilotent désormais
`.ec-cran` / `.echelle-curseur`.

### Pièges d'instrument DÉJÀ payés (ne pas les repayer)

1. **`head`/`tail` sur un contrôle** : le flux fermé tue le script (SIGPIPE) — on écrit
   dans un fichier, puis on filtre.
2. **`offsetParent === null`** ne veut PAS dire « invisible » : c'est aussi le cas de tout
   élément `position: fixed`. La visibilité se juge par `getBoundingClientRect` **et**
   `elementFromPoint`.
3. **Un `<input type=range>` piloté en JS** (`r.value = '3'`) ne déclenche PAS React :
   il faut un geste réel (clic/molette) ou le setter natif du prototype.
4. **`tsc` est une PORTE** : il a arrêté deux constructions fautives de cette lignée
   (champ de journal non déclaré, paramètre documenté par `/** */`).
5. **Le champ publié doit être DÉCLARÉ** dans le type du journal, sinon la construction
   échoue — et le bundle servi reste l'ancien (les mesures mentent alors en silence).

---

## 8. Les commits de cette lignée

| commit | ce qu'il pose |
|---|---|
| `c4254fa7` | profondeur le long de l'axe, axe partagé, crân « monde » segmenté |
| `0aecec01` | un quatrième cran RÉEL + six axes de descente |
| `5a6f25de` | les trois crans de vue ne faisaient plus rien — mesuré, puis corrigé |
| `073111cd` | la descente fractale (`fractale.ts`) : 3 → 200 → 60 → … |
| `b11c2c5a` | le graphe suit le niveau : chaque sous-groupe a son propre graphe |
| `3c4831fc` | le chemin des entrées de graphe |
| `48fd59c2` | le cran cadre enfin le monde qu'il mesure (le vol était parfait) |
| `8c76266c` | la génération qui boucle s'arrête (coupe-circuit de répétition) |
| `4cbbf229` | le client parti arrête la génération + budget par question |
| `346c93a2` | l'échelle devient un curseur vertical porteur de données |
| `4179e1c3` | le contrôle lit le LIBELLÉ du cran, pas la donnée qu'il porte |
| `799d3c74` | la tenue de cran publie son verdict (il renverse le diagnostic) |

---

## 9. En trois phrases, pour reprendre demain

1. **L'échelle est une scène continue à quatre paliers** ; l'autorité est `palierDe(d, R)`,
   et tout affichage (mot, cran allumé, ouverture) doit s'y référer.
2. **Le vol, la visée et la tenue sont justes** (mesurés) ; l'incohérence restante est
   qu'une **seconde distance** (`caméra ↔ ctrl.target`) alimente `t`/`camDist`, la barre
   et le contrôle — à unifier sur `cible.mesh.position`.
3. **`tS` ne s'engage pas** au cran « signaux » ici, alors qu'il a déjà valu 1 ailleurs :
   c'est le prochain défaut à mesurer, et `__etages.monde.signaux` porte tout ce qu'il
   faut pour le faire.
