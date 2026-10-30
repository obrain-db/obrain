# Instantané et « forme » — deux artefacts qu'il ne faut PAS confondre

**Date** : 2026-09-17. **Statut** : **l'étape 1 du §5 est FAITE ET MESURÉE**
(`crates/obrain-substrate/src/instantane.rs`). Les étapes 2 à 4 restent un plan.

**Régime des affirmations.** **CONSTATÉ** = lu dans un fichier cité.
**MESURÉ** = relevé par exécution. **PLAUSIBLE** = mécanisme identifiable dans ce
dépôt, non mesuré. **SPÉCULATIF** = rien ici ne l'étaye ; c'est dit pour que
personne ne le cite comme s'il l'était.

---

## 0. Mise en phase avec l'avancement du projet (2026-09-17)

**CONSTATÉ**, en relisant ce qui a bougé pendant ce chantier :

- **Aucune duplication.** L'avancement courant porte sur le **tissage**
  (le savoir du chat et de l'engine), sur **l'échelle multi-niveaux** du
  dashboard (`packages/obrain-dashboard/src/graph/echelle.ts`, `palierDe(d, R)`,
  mesuré sous chromium CDP) et sur les **index de propriété substrat**. Le présent
  chantier (instantané, substrat en mémoire, binding wasm) est un fil ORTHOGONAL :
  personne ne travaille dessus, et rien ici n'empiète sur ce qui avance ailleurs.
- **Attention au vocabulaire : « échelle », « relief », « crans » sont PRIS.**
  Ils désignent le rendu multi-niveaux du dashboard. Le résumé fidèle décrit au
  §4 ci-dessous ne doit PAS s'appeler « échelle » ni « relief » : ici on parle
  d'**instantané exact** et de **résumé fidèle**, sans plus.
- **`T17b` ne veut plus dire « snapshot » dans le code d'aujourd'hui.** Le
  commit `b0f5bd11` annonçait « a substrate-native snapshot API lands as T17b »,
  mais les occurrences actuelles de `T17b` désignent l'**index de propriété natif
  substrat** (`store.rs:17486`, `20898`, `21011`…). La référence du §1 est
  HISTORIQUE et exacte ; elle n'est plus la bonne étiquette pour parler
  d'instantané. `T17d`, lui, est encore la bonne étiquette pour la **compaction
  des pages de propriétés** (cité mot pour mot dans `props_zone.rs:147-150`).
- **La méthode est déjà la bonne.** Les documents de reprise du projet exigent
  que « chaque affirmation porte un fichier, une ligne, une commande ou un
  test » — c'est exactement ce que font les §4bis à §4quater, et le régime
  CONSTATÉ / MESURÉ / PLAUSIBLE / SPÉCULATIF du §6.

## 1. Le fait qu'il faut poser d'abord : la compacité est DÉJÀ là

**CONSTATÉ.** Le substrat range ses entités en tableaux binaires à pas fixe :

| enregistrement | taille | fichier |
|---|---|---|
| `NodeRecord` | **40 B** | `crates/obrain-substrate/src/record.rs:188` |
| (2ᵉ type) | **36 B** | `record.rs:437` |
| `SimplexRecord` | **32 B** | `record.rs:556` |

Pas de pointeurs, pas d'en-tête par objet, pas de padding par enregistrement :
`slot × taille` est l'adresse. Les chaînes vivent dans un *string heap* commun,
les clés dans un dictionnaire. L'en-tête méta
(`crates/obrain-substrate/src/meta.rs:40-55`) porte déjà tout ce qu'un manifeste
demanderait : `magic`, `format_version`, `flags`, `node_count`, `edge_count`,
`property_page_count`, `string_heap_size`, `created_at`, `last_checkpoint`,
`last_wal_offset`, `hilbert_order`, `embedding_matrix_seed`.

**Conséquence, et c'est le point qui change tout le raisonnement :** un
instantané **exact** (sans perte) n'a rien à « inventer » en compacité. Il a à
**recopier** une forme déjà dense. Chercher une « forme parfaite » pour
l'exactitude, c'est chercher ce qui est écrit noir sur blanc dans `record.rs`.

Ce qui manque n'est donc pas la compacité, c'est **le geste de copie** : il
n'existe aucune sortie d'instantané.

**CONSTATÉ — ce qui existe aujourd'hui, et ce que ce n'est pas :**

| surface | nature | ce qu'elle ne fait pas |
|---|---|---|
| `obrain data export/import` (CLI) | JSON lines, implémenté **dans le CLI** | pas natif : re-sérialise en texte, dix fois plus gros |
| `native-import` (`crates/obrain-cli/src/commands/native_import.rs`) | corpus binaire *ad hoc* : `n, m`, noms de types, **un label par nœud**, arêtes `(src,dst,type)` | **pas de propriétés**, pas de dictionnaire, pas de colonnes latérales — c'est un format d'INGESTION, pas une image de store |
| `importLpg` (binding wasm) | JSON | idem |
| snapshot bincode | **retiré** par `b0f5bd11` | — |

## 2. Deux artefacts, deux cahiers des charges opposés

La demande — « une forme si parfaite qu'elle représente tout, extrêmement
compacte » — recouvre deux choses que tout oppose. Les confondre est la manière
la plus sûre de produire soit un faux sauvegardé, soit un vrai inutilisable.

| | **(A) Instantané exact** | **(B) « Forme » — résumé fidèle** |
|---|---|---|
| garantie | **restauration bit-à-bit** des zones | les **requêtes** répondent ≈ pareil |
| perte | aucune, jamais | assumée, **mesurée** |
| où vit la compacité | déjà dans le layout (§1) | dans l'invariant qu'on choisit de garder |
| usage | sauvegarde, migration, transfert | mémoire longue, partage entre selves, « relief » |
| physique / maths / bio | **n'ont rien à y faire** | c'est là, et seulement là, qu'ils servent |

**Dire ceci est le cœur de cette note :** si on cherche la physique, la
mathématique et la biologie pour fabriquer l'artefact **(A)**, on fabrique un
instantané qui mentira tôt ou tard (une courbe spectrale, une décimation par
courbure, une consolidation par force d'engramme NE RESTITUENT PAS un nœud
supprimé). Les trois domaines appartiennent à **(B)**.

## 3. (A) L'instantané exact — le plan, et ses vrais problèmes

Les vrais problèmes ne sont pas la taille, ce sont **la cohérence** et
**l'identité**.

1. **Cohérence pendant la copie.** Le store vit : WAL en append, zones mmapées,
   colonnes latérales paresseuses. Un dump naïf capture un état de Schrödinger
   (pages à moitié écrites, colonne indexée à un ancrage WAL périmé). Deux
   stratégies, dans cet ordre de préférence :
   - **barrière** : verrou d'écriture + `flush()` (le substrat sait déjà le faire,
     `store.rs:7499`) puis copie — c'est court et vérifiable ;
   - **fenêtre WAL** : copie sans arrêter les écritures, avec l'offset WAL
     capturé avant/après, plus les enregistrements de la fenêtre joints à part.
     Plus ambitieux ; c'est ce que `dernier_offset_flush` sait déjà exprimer.
2. **Identité du store restauré.** Le manifeste doit porter ce qui rend les
   octets relisibles : `format_version`, les **strides par zone** (la zone nodes
   se décrit elle-même depuis « jalon 2 » via `NodeRecord::sentinelle_zone_v3`),
   les compteurs, et un CRC par zone. Sinon on restaure un store que le binaire
   suivant refusera — ou, pire, qu'il acceptera de travers.
3. **Les colonnes latérales, qui sont la vraie difficulté.** Un instantané qui
   n'emporte que `nodes`/`edges`/`props` perd : dictionnaire (`substrate.dict`),
   degrés (générique + par type), Hilbert/Z-order, communautés, simplexes
   (`SimplexDir`, `SimplexIncidence`, heaps), engrammes (membres + bitset), et
   l'état MVCC/époques. Deux choix défendables — **tout** (image fidèle, gros) ou
   **le minimum + reconstruction** (petit, mais il faut prouver que les
   reconstructions donnent le même graphe). Le second est le seul qui soit
   « compact » au sens voulu, et il déplace le risque vers les reconstructeurs.
4. **Ce qui est déjà écrit quelque part.** Le WAL post-troncature et le
   dictionnaire se recouvrent partiellement ; `native_import` montre qu'on sait
   écrire en zone-direct sans WAL par opération (`Writer::bulk_write`) — le
   pendant en lecture est le même chemin, à l'envers.

**Compacité disponible SANS perte, si on la cherche vraiment :** le string heap
et le dictionnaire sont les seuls endroits non denses (texte répété) ; l'ordre
Hilbert/Z-order, **déjà calculé**, donne des deltas courts sur les arêtes qui se
ressemblent — c'est de l'entropie gagnée gratuitement, sans toucher à
l'exactitude.

**Critères d'acceptation (falsifiables) :** restauration → zones **bit-à-bit
identiques** ; `bytes/nœud` annoncé et comparé ; un store de 10⁶ nœuds restauré
puis interrogé par une batterie de requêtes identique donne **les mêmes
résultats**.

## 4. (B) La « forme » — là où les trois domaines servent vraiment

Règle que je m'impose ici : chaque idée doit s'ancrer sur une structure **que ce
dépôt calcule déjà**. Sinon c'est une intuition décorative.

### 4.1 Mathématiques

| levier | ancrage existant | ce qu'il promet | statut |
|---|---|---|---|
| **Codage par l'entropie** (dictionnaire, varint, deltas en ordre de courbe) | `substrate.dict`, zones Hilbert/Z-order | −30 à −60 % sur le texte et les arêtes répétées, **sans perte** | PLAUSIBLE |
| **Bas rang / spectral** | « spectral signature » des engrammes (`engram/types.rs`), `embedding_matrix_seed` | représenter un voisinage par ses premiers modes plutôt que par ses arêtes : compact, et **stable aux petites perturbations** | PLAUSIBLE |
| **Coarse-graining multi-échelle (RG)** | communautés Leiden, hiérarchie de tiers (`Tier0/1/2`) | garder un graphe grossier + les résidus : la « forme » à plusieurs résolutions | PLAUSIBLE |
| **Complexes simpliciaux** | zones `SimplexDir` / `SimplexIncidence` | l'information des hyperarêtes est déjà séparée du graphe : la compresser à part, c'est un gain structurel, pas un tour | CONSTATÉ (la structure existe) |

### 4.2 Physique

| levier | ancrage existant | ce qu'il promet | statut |
|---|---|---|---|
| **Géométrie comme invariant** — garder ce qui préserve courbure et diffusion de la chaleur, jeter le reste | **Ollivier-Ricci est déjà calculée et persistée** (`enrich --ricci`, `node_curvature`), `heat_diffusion` existe | choisir les arêtes à garder par leur contribution géométrique, pas par leur poids : « même forme, moins de maillage » | PLAUSIBLE |
| **Coarse-graining à la RG** (intégrer les degrés de liberté courts) | les tiers | une hiérarchie où chaque niveau conserve les observables du niveau fin | PLAUSIBLE |
| **Holographie / AdS-CFT** (le volume encodé sur la frontière) | — | rien de falsifiable ici sur un graphe de propriétés | **SPÉCULATIF** |

### 4.3 Biologie

| levier | ancrage existant | ce qu'il promet | statut |
|---|---|---|---|
| **Consolidation** : ne garder que ce qui a été consolidé, laisser le reste s'effacer | engrammes + FSRS (`strength`, décroissance), activation/rappel | l'instantané « biologique » n'est pas une copie, c'est **ce qui a survécu** : compact parce que sélectif | PLAUSIBLE |
| **Élagage synaptique** : couper sous un seuil d'usage | degrés, compteurs d'accès | réduit le maillage en gardant la topologie utile | PLAUSIBLE |
| **Compression « codons »** (alphabet quaternaire, cadre de lecture) | — | sur des enregistrements déjà à 40 octets et dominés par des entiers, un alphabet plus petit ne gagne pas : l'entropie réelle n'est pas dans l'alphabet | **SPÉCULATIF / probablement faux ici** |

Le point qui compte : **(B) se mesure en fidélité comportementale**, pas en
octets. Deux courbes, une par artefact — et si la seconde ne tient pas la
première, ce n'est pas un instantané.

## 4bis. ÉTAPE 1 FAITE — l'export zone-direct, et ce qu'il a révélé

**CONSTATÉ + MESURÉ.** `crates/obrain-substrate/src/instantane.rs` fournit
`exporter` / `restaurer` / `stats`, format `OBRSNAP1` (méta brute + zones
verbatim, nom/longueur/**CRC32** par zone, ordre trié pour que deux instantanés
d'un même store soient comparables octet à octet).

- **Portable par construction** : la copie passe par l'abstraction `ZoneFile`,
  jamais par le système de fichiers — le même code sert un store fichier ET un
  store en mémoire (nouveau `SubstrateFile::zone_names()`).
- **Round-trip EXACT prouvé** : `round_trip_bit_a_bit_sur_les_zones` compare
  chaque zone OCTET À OCTET après restauration. Un instantané **corrompu** (CRC
  faux) et un instantané **tronqué** sont REFUSÉS — testés, pas supposés.
- 5 tests, 5 verts.

**MESURÉ — version 1 (copie verbatim des zones) :**

```
14 078 652 octets — 26 315 slots nœuds / 29 239 slots arêtes, 20 zones
→ 535,0 octets/slot, rembourrage ×13,16 (pour 2 000 nœuds vivants créés)
```

**Lecture :** une copie naïve hérite du **rembourrage ×1,5** de la pré-allocation
(`ZoneFile::grow_to`) — ici **13 fois** le contenu vivant.

**ÉTAPE 2 FAITE — format v2 : le rembourrage n'est plus transporté.** Le
rembourrage de `grow_to` est, par construction, **des zéros de queue**. Le format
v2 transporte donc, par zone, la longueur LOGIQUE et la charge utile **débarrassée
de ses zéros de queue**, et la restauration appelle `grow_to(len_logique)` — qui
re-zérofile exactement comme la croissance d'origine — avant de réécrire la charge
utile. Ce n'est pas une heuristique de compression : c'est la reconnaissance de ce
que la pré-allocation écrit, donc **sans perte**, et c'est prouvé par le test
bit-à-bit, qui compare toujours les zones ENTIÈRES, rembourrage compris.

**MESURÉ — version 2 :**

```
8 408 183 octets (contre 14 078 652 en v1) → −40,3 %
319,5 octets/slot (contre 535,0)
5 tests, 5 verts, round-trip bit-à-bit inchangé
```

**ÉTAPE 3 — DIAGNOSTIC FAIT : la cible est UNE zone.** Plutôt que d'optimiser à
l'aveugle (leçon de `benches/BANCS.md`), la répartition par zone de l'image
exportée a été mesurée (`instantane::repartition`), sur le même corpus de 2 000
nœuds / 1 999 arêtes / 4 000 propriétés :

```
substrate.props.v2        9 818 112 → 8 192 046   (97,4 %)
substrate.nodes           1 052 672 →    80 031   ( 1,0 %)   ← compactée 13×
substrate.edges           1 052 672 →    71 970   ( 0,9 %)   ← compactée 15×
substrate.props.heap.v2      32 768 →    27 042   ( 0,3 %)
substrate.degrees.node.u32 1 052 672 →    16 069   ( 0,2 %)
TOTAL des zones : 8 403 227 octets (manifeste : 4 956)
```

Trois lectures, et la troisième est la vraie :

1. La découpe des zéros de queue **marche exactement comme prévu** : les zones à
   pas fixe tombent d'un facteur **13 à 15** (nœuds, arêtes, degrés).
2. **Il ne reste qu'une cible** : `substrate.props.v2` pèse à lui seul **97,4 %**
   de l'instantané. Optimiser ailleurs serait du bruit.
3. **Sa taille est le résultat, pas le problème du format** : 9,8 Mio logiques
   pour 4 000 propriétés, soit ≈ **2,4 Kio par propriété**. Une zone de props v2
   rangée en pages de 4 Kio n'a aucune raison de coûter 2,4 Kio par valeur — sauf
   si l'allocation est proche d'**une page par propriété**. C'est une HYPOTHÈSE
   (PLAUSIBLE, **non vérifiée** : la vérifier demande de lire l'allocation de
   pages de `props_zone.rs`), et elle déplacerait le sujet de l'instantané vers le
   **rangement des propriétés** — un gain qui profiterait à tous les stores, pas
   seulement aux instantanés.

**Ce que la v2 laisse intact, dit franchement :** les zéros **internes** aux zones
ne sont pas coupés — seuls les zéros de QUEUE le sont. C'est pourquoi `props.v2`
passe de 9,8 à 8,2 Mio (17 % gagnés) là où les zones à pas fixe gagnent 13 à 15×.

**Prochaine cible, donc, et elle est unique :** `substrate.props.v2`. Deux voies —
(1) re-ranger les pages VIVANTES dans l'image exportée (le substrat sait déjà le
faire en place : `SubstrateStore::shrink_props_zone`, « truncating the zero-init
padding reclaims disk » ; l'export devrait le faire SANS muter le store vivant),
ou (2) d'abord comprendre pourquoi 4 000 propriétés occupent ≈ 2 400 pages.

**Deuxième découverte, à ne pas oublier :** sur un store EN MÉMOIRE, les
compteurs `node_count` / `edge_count` de la méta valent **0** — ils ne sont
écrits qu'au flush/checkpoint, or le mode mémoire ne flushe pas. L'instantané
compte donc depuis les **zones**, et il compte des **SLOTS**, pas des entités
vivantes (2 000 nœuds → 26 315 slots). Champs nommés `slots_noeuds` /
`slots_aretes` pour que personne ne confonde les deux.

## 4ter. ÉTAPE 3 — la cause est trouvée, et elle n'est PAS dans l'instantané

**CONSTATÉ (lu dans le code).** `crates/obrain-substrate/src/props_zone.rs` range
les propriétés d'un nœud en **chaîne de pages liées** (`first_prop_off` du
`NodeRecord` porte la tête ; `next_page` remonte la chaîne, du plus récent au plus
ancien). `PAGE_SIZE = 4096` (`page.rs:63`). Et le curseur d'allocation est
**monotone, jamais réutilisé** :

> « Cached `next_page` allocator cursor (monotonic). We never reuse page_idx values
> — tombstone-and-rewrite is handled in-page via the `ENTRY_FLAG_TOMBSTONE` flag,
> and wholesale page compaction is a T17d concern. » — `props_zone.rs:147-150`

**Conséquence, cohérente avec la mesure :** un nœud qui porte des propriétés coûte
**au moins une page de 4 Kio**, qu'il porte deux propriétés de 30 octets ou vingt.
Sur le corpus mesuré (2 000 nœuds × 2 propriétés) : ≈ 2 000 pages × 4 Kio ≈ 8 Mio —
soit exactement les 8,2 Mio observés. Le nœud lui-même, lui, coûte 40 octets.

**Donc :** les 97,4 % qui restent ne sont pas du rembourrage, ni un défaut de
format d'instantané — c'est la **granularité du rangement des propriétés**
(≈ 4 Kio par nœud porteur), et le code dit lui-même que la compaction de pages est
**différée à T17d**.

**Deux voies, et c'est un arbitrage, pas une évidence :**

1. **Accepter** : l'instantané est exact et 40 % plus compact que la copie naïve.
   Coût : 4 Kio par nœud porteur dans l'image.
2. **Ouvrir T17d** (compaction des pages de propriétés) : re-ranger les entrées
   vivantes en pages pleines. Gain pour **tous les stores**, pas seulement les
   instantanés — mais c'est un chantier sur un format central, avec la sémantique
   de chaîne/versionnement à préserver.

Faire semblant de résoudre ça *dans* `instantane.rs` serait exactement l'erreur que
ce module refuse : transformer un problème de rangement en tour de passe-passe de
sérialisation.

## 4quater. ÉTAPE 3bis — le dictionnaire est raccordé (format v3)

**CE QUI A ÉTÉ FAIT.** Le format passe en **v3** : après l'en-tête meta, une
section **dictionnaire** (`[4] longueur, [4] CRC32, [n] octets`) transportant le
`DictSnapshot` du store — registres (labels, types d'arête, clés de propriété,
colonnes vectorielles) et compteurs d'allocation.

- `SubstrateStore::octets_dictionnaire()` : les octets du dictionnaire courant
  (pendant de `build_dict_snapshot`, qui les écrit — ici on les REND).
- `SubstrateStore::instantane()` : l'image complète, dictionnaire + zones + méta.
- `instantane::dict_du_flux(image)` : relit le dictionnaire transporté — la voie
  d'accès pour les supports **sans répertoire** (un store en mémoire ne peut pas
  recevoir un fichier).
- À la restauration, le dictionnaire est écrit dans le répertoire du store, là où
  `DictSnapshot::load` le cherchera — ou récupéré par `dict_du_flux` si le support
  n'a pas de répertoire.

**MESURÉ.** 6 tests verts, dont `le_dictionnaire_voyage_et_se_recharge` : le label,
le type d'arête et la clé de propriété se relisent depuis le flux, et un instantané
produit SANS dictionnaire en est **absent** (et non « présent et vide », ce qui
laisserait croire à un store sans aucun label).

**ET LA BOUCLE EST FERMÉE (même jour, §4quinquies ci-dessous) :** le dictionnaire
atterrit désormais aussi en MÉMOIRE — il est écrit comme une **zone**
(`substrate.dict`), que `from_substrate` relit quand `is_memory()`. C'était le
dernier morceau d'exactitude.

## 4quinquies. L'INSTANTANÉ EXACT EST COMPLET — boucle fermée

**CE QUI A ÉTÉ FAIT.** Le dictionnaire devient une **ZONE** (`substrate.dict`) au
lieu d'un fichier écrit à la main. Cette unification n'est pas cosmétique : c'est
elle qui rend la restauration possible dans les DEUX modes.

- En mode **FICHIER**, `open_named_zone` ouvre/crée `<dir>/substrate.dict` et y
  écrit les octets — exactement le fichier que `DictSnapshot::load` relit.
- En mode **MÉMOIRE**, la même écriture dépose la zone dans le registre du
  substrat, et `from_substrate` la relit (`if sub.is_memory()`). Avant, le
  dictionnaire voyageait mais ne pouvait pas atterrir.

Et `SubstrateStore::depuis_instantane(image)` restaure **dans le bon ordre** : un
substrat neuf, on y restaure zones + méta + dictionnaire, et **seulement alors** on
construit le store — qui recharge ses registres en lisant ce substrat. On ne
rafistole jamais un store vivant : ses couches (cache de zones, colonnes, index)
naissent sur les octets restaurés.

**MESURÉ — 7 tests verts**, dont
`un_store_restaure_en_memoire_sait_encore_nommer_ses_labels`, qui exige les DEUX :
les octets (le nœud 1 se relit) **et** le sens (le label « Personne » et le type
d'arête « CONNAIT » se relisent depuis les registres du store restauré, pas depuis
l'image).

**L'ARTEFACT ① EST DONC COMPLET :**

| garantie | état |
|---|---|
| zones restaurées **bit-à-bit** | PROUVÉ (`round_trip_bit_a_bit_sur_les_zones`) |
| rembourrage non transporté (−40 %) | MESURÉ |
| dictionnaire transporté ET appliqué | PROUVÉ, en fichier comme en mémoire |
| instantané **corrompu** refusé (CRC) | PROUVÉ |
| instantané **tronqué** refusé (sans panique) | PROUVÉ |
| restauration **en mémoire** (wasm) | PROUVÉ (`depuis_instantane`) |

Ce qui reste n'est PLUS dans ① : c'est le **WAL** (journal de rejeu, non transporté)
et la compacité des propriétés (artefact ②, cf. §4ter).

## 5. Ce que je ferais, dans cet ordre

1. **L'export zone-direct (le jumeau manquant de `native-import`)** — manifeste
   (méta + strides + CRC par zone) + copie des zones sous barrière. Petit,
   borné, et c'est **la** brique dont tout le reste dépend. Livrable : un
   round-trip bit-à-bit testé, `bytes/nœud` mesuré.
2. **Le dictionnaire et le string heap en codage entropique** — premier gain
   réel de compacité, sans perte, mesurable seul.
3. **Le manifeste des colonnes latérales** : décider « tout » contre
   « minimum reconstructible », et **prouver** les reconstructeurs (degrés,
   Hilbert, communautés) par égalité de requêtes.
4. **Alors seulement (B)** : mesurer d'abord combien d'octets on gagne à jeter
   sous un seuil (élagage par degré, puis par courbure), et **à quel prix en
   fidélité** — sur un corpus réel du dépôt, pas sur un graphe jouet.

## 6. Non mesuré, et ce que je refuse d'affirmer

- **NON MESURÉ** : aucun `bytes/nœud` d'instantané n'existe, puisqu'aucun
  instantané n'existe. Le seul chiffre de compacité établi ici est celui des
  enregistrements (40/36/32 B, `record.rs`).
- **NON MESURÉ** : le coût des reconstructeurs de colonnes latérales.
- La correspondance « courbure ↔ fidélité des requêtes » n'est **pas** un
  résultat de ce dépôt : c'est une hypothèse de travail, à falsifier en
  comparant des réponses de requêtes avant/après élagage.
- Le mot « parfait » n'a pas de sens mesurable ici. Ce qui en a un :
  **exact** (restauration bit-à-bit) et **fidèle** (accord de requêtes ≥ seuil à
  X % d'octets). C'est sous ces deux mots-là que je peux travailler.
