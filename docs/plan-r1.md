# NATIF-R1 — Reparation de la donnee corrompue R1 (aretes cycliques)

Chantier `fix/r1-dedup-aretes` (base `dcb6592d`). Store cible :
`~/.local/share/obrain-harness/data/runtime-dev.obrain` (VIVANT — jamais
touche en ecriture par ce chantier). Preuve conduite sur COPIE a froid
`~/tmp/harness-r1/copie.obrain`.

> Regle d'or : ce chantier ne travaille QUE sur la copie. L'orchestrateur
> applique la recette au vivant lui-meme, en fenetre, avec sauvegarde.

---

## 1. Diagnostic exact

### 1.1 Le symptome

| Mesure | Valeur (copie, avant) |
|---|---|
| Compteur d'aretes `MATCH ()-[r]->() RETURN count(r)` | **1 306 807** |
| Traversee sortante `MATCH (a)-->(b) RETURN count(a)` | **20 895 577** (x16) |
| Traversee entrante `MATCH (a)<--(b) RETURN count(b)` | **6 536 966** (x5) |
| Noeuds `count(n)` | 434 066 |

Le COMPTEUR d'enregistrements est juste ; c'est la TRAVERSEE qui explose.

### 1.2 La cause racine — chaines d'adjacence CYCLIQUES

Le substrat (`crates/obrain-substrate/src`) stocke une arete comme un
`EdgeRecord` de 36 o (`record.rs`) portant `src`, `dst`, `edge_type`, et
DEUX pointeurs de liste chainee :

- `next_from` — arete suivante partageant le meme `src` (chaine SORTANTE,
  entree `NodeRecord.first_edge_off`).
- `next_to` — arete suivante partageant le meme `dst` (chaine ENTRANTE,
  entree `incoming_heads[dst]`, une map memoire relue depuis les records a
  chaque ouverture).

`MATCH ()-[r]->()` compte les ENREGISTREMENTS (scan lineaire de la zone
edges). `MATCH (a)-->(b)` MARCHE la chaine `next_from` de chaque noeud
(`walk_outgoing_chain`, store.rs).

Des ecritures concurrentes d'AVANT l'introduction du `splice_lock`
(incident dedup runtime-dev, 2026-09-08) ont rendu CYCLIQUES les chaines
`next_from` (et quelques `next_to`) de 15 noeuds. `walk_outgoing_chain`
possede une garde anti-cycle `garde_max = next_edge_id + 1` (~1,3 M) : sur
un cycle, la marche re-visite les memes records jusqu'a la garde puis
abandonne. D'ou l'explosion FINIE mais massive : chaque noeud corrompu
rend ~1,3 M lignes (garde_max) au lieu de ses quelques aretes reelles.

**Preuve que les ENREGISTREMENTS sont intacts** (aucune duplication de
record) : le type `PROVES` ne compte que **3** aretes au total
(`MATCH ()-[r:PROVES]->()`), alors que les noeuds 317553 ET 318414
"emettent" chacun 1 306 818 aretes PROVES en traversee. Idem `IN_SESSION`
= 59 909 records tandis que 175745/6/7 en "emettent" 1,3 M chacun. Seuls
les POINTEURS de chaine sont faux ; `src`/`dst`/`edge_type`/poids/props des
records sont corrects. C'est pourquoi `repair-degrees` (degres recalcules
depuis les records, deja justes) et `compact` (ne touche pas les pointeurs
de chaine) n'ont eu AUCUN effet en J5.

### 1.3 Les 15 noeuds — caracterisation des sortantes (avant reparation)

Chaque noeud rend ~garde_max lignes en traversee ; forme du cycle variable.
`n` = nombre de fois que la traversee re-visite l'arete (revisites du cycle).

| id | label | type | cible(s) du cycle x revisites | aretes reelles (apres fix) |
|---|---|---|---|---|
| 175745 | SelfState | IN_SESSION | 401737 x1 306 816 | **7** (401737, 415800, 416102, 398525, 364795, 189087, 175556) |
| 175746 | SelfState | IN_SESSION | 401737 x1 306 816 | reelles restaurees (dont 401737, 416102, 409581...) |
| 175747 | SelfState | IN_SESSION | 401737 x1 306 816 | reelles restaurees |
| 317553 | (Probe/Proof) | PROVES | 316854 x1 306 818 | 1 (316854) |
| 318414 | (Probe/Proof) | PROVES | 316854 x1 306 818 | 1 (316854) |
| 319094 | (StateHub) | HAS_STATE | 326510 x650247 + 326509 x650247 | ~33 090 HAS_STATE (24 444 cibles) + EMITTED_BY + WOVEN_BY |
| 325167 | Step-ish | HAS_STEP | ~29 cibles x45 063 | reelles restaurees |
| 325447 | Step-ish | HAS_STEP | ~29 cibles x45 063 | reelles restaurees |
| 325819 | Step-ish | HAS_STEP | ~29 cibles x45 063 | reelles restaurees |
| 326114 | Step-ish | HAS_STEP | ~29 cibles x45 063 | reelles restaurees |
| 326140 | Step-ish | HAS_STEP | ~29 cibles x45 063 | reelles restaurees |
| 326241 | Step-ish | HAS_STEP | ~29 cibles x45 063 | reelles restaurees |
| 326349 | Step-ish | HAS_STEP | ~29 cibles x45 063 | reelles restaurees |
| 326438 | Step-ish | HAS_STEP | ~29 cibles x45 063 | reelles restaurees |
| 326679 | Step-ish | HAS_STEP | ~29 cibles x45 063 | reelles restaurees |

Verification de coherence du multiplicateur : 29 x 45 063 = 1 306 827 ~
garde_max ; 2 x 650 247 = 1 300 494 ~ garde_max ; 1 x 1 306 816 ~ garde_max.

**Decouverte importante** : sur 175745, le cycle etait un self-loop sur
l'arete vers 401737, place EN TETE de chaine. La garde se declenchait AVANT
d'atteindre les aretes reelles situees plus loin dans la chaine (398525,
364795, 189087, 175556) : ces aretes reelles etaient donc INVISIBLES a la
traversee, bien que leurs records existent. Une reparation par simple
"coupe de cycle" ne les aurait pas retrouvees. La reconstruction depuis les
records (voir sec. 2) les RESTAURE.

Les chaines ENTRANTES (`next_to`) etaient corrompues plus legerement
(traversee entrante 6,5 M vs 1,3 M) — d'ou la necessite de reparer les DEUX
sens.

---

## 2. Mecanisme de reparation

### 2.1 Principe : reconstruire les chaines depuis la source de verite

Les `EdgeRecord` sont la SOURCE DE VERITE (intacts). Les chaines sont des
pointeurs DERIVES. La reparation la plus sure = RECONSTRUIRE
deterministiquement les chaines depuis les records vivants :

- pour chaque `src`, re-lier `next_from` sur ses records vivants, une seule
  fois chacun ; poser `NodeRecord.first_edge_off` ;
- pour chaque `dst`, re-lier `next_to` ; `incoming_heads` est de toute facon
  reconstruite depuis les records a chaque ouverture (map memoire) — aucune
  action disque requise dessus.

**Invariant reconstruit** : tete = slot le PLUS HAUT, ordre DECROISSANT —
l'etat EXACT qu'une insertion propre (`splice_edge_at_head`, LIFO, slot
croissant) produit. Une chaine SAINE est donc deja dans cet etat : le
nombre d'ecritures egale le nombre de pointeurs REELLEMENT faux. Empreinte
minimale, IDEMPOTENT (2e passe -> 0 ecriture). Ne touche NI records
(`src`/`dst`/`type`/poids/props), NI degres, NI compteurs.

Mesure sur la copie : sur 1 306 807 aretes, seuls **1 348** records et
**761** tetes de noeud divergeaient (0,1 %) — la corruption est bien
localisee aux ~15 chaines.

### 2.2 Implementation (commits sur `fix/r1-dedup-aretes`)

- `SubstrateStore::rebuild_edge_chains(dry_run)` — `crates/obrain-substrate/src/store.rs`.
  Scan des records vivants, calcul des pointeurs voulus, ecriture des SEULS
  diffs (via `writer.update_edge` / `update_node`, donc journalise WAL). Pris
  sous `splice_lock` (acces exclusif). Test unitaire
  `store::tests::rebuild_edge_chains_casse_un_cycle_et_est_idempotent`.
- `ObrainDB::rebuild_edge_chains(dry_run)` — `crates/obrain-engine/src/database/mod.rs`
  (delegation via `substrate_handle`).
- Commande CLI `obrain repair-edge-chains <PATH> [--apply]` —
  `crates/obrain-cli/src/main.rs` + `crates/obrain-cli/src/commands/repair.rs`.
  Sans `--apply` : DRY-RUN (diagnostic seul, aucune ecriture). Avertit si le
  store a ete ecrit recemment (non atomique face a un ecrivain concurrent).

> Note de nommage : la tache suggerait `obrain repair dedup-edges`. Le nom
> retenu est `repair-edge-chains` (frere de `repair-degrees`) car l'operation
> ne DEDUPLIQUE pas des records (il n'y en a pas en double) : elle RECONSTRUIT
> les chaines d'adjacence. Le nom dit ce que fait la commande.

### 2.3 RECETTE pour l'orchestrateur (a executer sur le VIVANT en fenetre)

Binaire de reparation (worktree, contient la commande) :
`~/projects/banc-obrain-r1/target/release/obrain`. NE PAS remplacer le
binaire deploye. Le vivant s'appelle
`~/.local/share/obrain-harness/data/runtime-dev.obrain`.

```
# 0. FENETRE : arreter TOUT ecrivain du vivant (service chat/viz, ponts MCP).
#    La reconstruction lit puis reecrit : NON atomique face a un ecrivain.
# 1. SAUVEGARDE du vivant (cp -r a froid, writers arretes).
# 2. DIAGNOSTIC (lecture seule, aucune ecriture) :
BIN=~/projects/banc-obrain-r1/target/release/obrain
V=~/.local/share/obrain-harness/data/runtime-dev.obrain
$BIN repair-edge-chains "$V" --format json          # attend status "corrompu", next_from_fixed>0
# 3. APPLIQUER :
$BIN repair-edge-chains "$V" --apply --format json   # status "repare"
# 4. IDEMPOTENCE (preuve) :
$BIN repair-edge-chains "$V" --apply --format json   # tous *_fixed = 0, status "sain"
# 5. Reconstruire les index (ils se lisent depuis les chaines) :
$BIN index rebuild "$V"
# 6. Verifier compteur == traversee :
$BIN query "$V" "MATCH ()-[r]->() RETURN count(r)"
$BIN query "$V" "MATCH (a)-->(b) RETURN count(a)"    # doit egaler le compteur
$BIN query "$V" "MATCH (a)<--(b) RETURN count(b)"    # doit egaler le compteur
# 7. Redemarrer les services.
```

Alternative sans nouveau binaire : rejouer la recette sur la copie deja
reparee `~/tmp/harness-r1/copie.obrain` (reference), ou remplacer le vivant
par cette copie APRES avoir verifie qu'aucune ecriture vivante n'a diverge
depuis la copie (la copie a ete prise a chaud : privilegier la commande sur
le vivant lui-meme).

---

## 3. Preuves (sur la copie reparee)

| Preuve | Avant | Apres |
|---|---|---|
| Compteur `count(r)` | 1 306 807 | **1 306 807** (inchange) |
| Traversee sortante `count(a)` | 20 895 577 | **1 306 807** (== compteur) |
| Traversee entrante `count(b)` | 6 536 966 | **1 306 807** (== compteur) |
| Noeuds `count(n)` | 434 066 | **434 066** (inchange) |
| CALLS | 618 776 | 618 776 |
| DEFINES | 225 418 | 225 418 |
| IN_SESSION | 59 909 | 59 909 |
| HAS_STEP | 64 513 | 64 513 |
| HAS_STATE | 60 824 | 60 824 |
| PROVES | 3 | 3 |

- **Compteur == traversee** dans les DEUX sens (1 306 807). Explosion
  eliminee.
- **Types temoins inchanges** : aucune arete typee (CALLS, DEFINES, ...)
  gagnee ou perdue.
- **Noeuds et props intacts** : `count(n)` inchange ; props de 175745
  intactes (`qualified_name: self:agent:state:v312`, `version: 312`).
- **Aretes reelles preservees et RESTAUREES** : 175745 rend ses 7 aretes
  IN_SESSION reelles (dont 4 qui etaient masquees par le cycle) ; 317553 rend
  sa seule arete PROVES ; 319094 rend ses ~33 090 HAS_STATE reelles.
- **Index reconstructibles** : `obrain index rebuild` OK (2 index persistes,
  5 s), lus depuis les chaines reparees.
- **Idempotence** : 2e passe `--apply` -> 0 pointeur corrige, status "sain".
- **Test unitaire** : `rebuild_edge_chains_casse_un_cycle_et_est_idempotent`
  ... ok (casse un cycle, preserve les aretes reelles, idempotent).

## 4. Livrables

- Store repare : `~/tmp/harness-r1/copie.obrain` (reference orchestrateur).
- Binaire de reparation : `~/projects/banc-obrain-r1/target/release/obrain`.
- Code : commits sur `fix/r1-dedup-aretes`.
- Ce document : `docs/plan-r1.md`.

## 5. Note sur la copie a chaud

La copie a ete prise sur un store CHAUD. A l'ouverture, l'audit signale 30
noeuds "husk" (label sans propriete joignable) — props ecrites apres le
dernier flush, perdues a la copie. Ces 30 noeuds sont sans rapport avec les
15 noeuds corrompus et n'affectent pas la preuve du mecanisme. Pour la
donnee vivante, l'orchestrateur travaille sur le vivant lui-meme (recette
sec. 2.3) apres sauvegarde a froid : aucun husk induit par la copie.
