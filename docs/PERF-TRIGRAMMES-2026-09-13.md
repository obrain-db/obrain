# PERF — Index de trigrammes pour les requêtes `CONTAINS`

**Date** : 2026-09-13 · **Banc** : `~/bench-stores/runtime-dev-snapshot-20260909.obrain`
(347 970 nœuds, aucun écrivain concurrent)

| pièce | fichier |
|---|---|
| cœur de l'index (postings, seuil, silences, tampon d'écritures) | `crates/obrain-substrate/src/props_trigram_index.rs` |
| câblage substrat (trait, crochet d'écriture, construction à la demande) | `crates/obrain-substrate/src/store.rs`, `crates/obrain-core/src/graph/traits.rs` |
| branchement planificateur (`CONTAINS`, disjonctions) | `crates/obrain-engine/src/query/planner/lpg/filter.rs` |
| banc du prédicat | `crates/obrain-substrate/examples/bench_trigrammes.rs` |
| banc de la REQUÊTE | `crates/obrain-engine/examples/bench_texte_gql.rs` |
| preuves de bout en bout | `crates/obrain-substrate/tests/trigrammes_bout_en_bout.rs` |

---

## 1. Le grief, chiffré

Les requêtes qui ont ouvert le chantier perf viennent du journal MCP du
2026-09-12 :

```text
MATCH (n) WHERE n.name CONTAINS 'iam' OR n.name CONTAINS 'IAM'
              OR n.qualified_name CONTAINS 'iam' RETURN … LIMIT 30   → 148 – 190 ms
MATCH (n) WHERE n.name CONTAINS 'auth' OR … 'token' …                → ~150 ms
```

Ce sont des balayages : 347 970 lignes parcourues, une lecture de propriété
par ligne, un test de sous-chaîne par ligne. Le témoin « balayage nu, zéro
cellule » coûte **25,7 ms** : tout le reste est la cellule et le prédicat.

## 2. Aucun index existant ne peut y répondre

| structure | ce qu'elle tranche | pourquoi elle ne peut pas servir `CONTAINS` |
|---|---|---|
| `property_indexes`, `props_eq_index` | l'ÉGALITÉ | une sous-chaîne n'est pas une clé |
| `props_index` (ordonné) | intervalles, préfixes | une occurrence **en milieu** de valeur n'a pas de rang |
| `InvertedIndex` (BM25) | des MOTS, en mémoire, reconstruit par processus | « iam » y chercherait un *jeton* et manquerait `Miami` : **faux négatif** |

## 3. Ce que des trigrammes achètent — mesuré, pas estimé

`bench_trigrammes`, seuil **4096**, minima sur 3 tours :

### `name` — 279 084 valeurs, 3,1 Mo de texte

| terme | vraies lignes | candidats | sur-ensemble | recherche | sur-coût |
|---|---|---|---|---|---|
| `iam` | 12 | 12 | OUI | 0,1 µs | **1,0×** |
| `auth` | 215 | 215 | OUI | 3,6 µs | **1,0×** |
| `Auth` | 150 | 150 | OUI | 1,4 µs | **1,0×** |
| `token` | 257 | 257 | OUI | 3,7 µs | **1,0×** |
| `Token` | 489 | 489 | OUI | 6,6 µs | **1,0×** |
| `obrain-iam` | 2 | 2 | OUI | 0,3 µs | **1,0×** |
| `IAM` | 0 | 0 | OUI | 0,0 µs | conclusif : zéro ligne, **sans balayer** |
| `ion` | 14 278 | — | — | — | l'index se TAIT → balayage (juste, lent) |

### `qualified_name` — 347 748 valeurs, 26,2 Mo de texte

| terme | vraies lignes | candidats | sur-ensemble | recherche | sur-coût |
|---|---|---|---|---|---|
| `iam` | 386 | 386 | OUI | 0,1 µs | **1,0×** |
| `IAM` | 10 | 10 | OUI | 0,1 µs | **1,0×** |
| `auth` | 1 226 | 1 226 | OUI | 11,5 µs | **1,0×** |
| `Auth` | 208 | 208 | OUI | 2,4 µs | **1,0×** |
| `token` | 491 | 491 | OUI | 8,0 µs | **1,0×** |
| `Token` | 545 | 545 | OUI | 8,6 µs | **1,0×** |
| `obrain-iam` | 371 | 371 | OUI | 5,5 µs | **1,0×** |
| `ion` | 238 307 | — | — | — | l'index se TAIT → balayage |

La sélectivité n'est pas « meilleure » : elle est **exacte** sur ces requêtes.
Il ne reste à revérifier que ce qui correspond vraiment.

## 4. Ce que ça coûte

| propriété | seuil | grammes | postings | mémoire | construction |
|---|---|---|---|---|---|
| `name` | 1024 | 47 612 | 1,15 M | 7,2 Mo | 0,16 s |
| `name` | **4096** | 47 976 | 1,80 M | **11,0 Mo** | 0,11 s |
| `qualified_name` | 1024 | 265 404 | 4,74 M | 28,8 Mo | 1,91 s |
| `qualified_name` | **4096** | 267 311 | 8,60 M | **50,9 Mo** | 1,34 s |
| `qualified_name` | aucun (tout indexer) | 276 117 | 24,44 M | 97,7 Mo | — |

**Le seuil de rareté est ce qui rend l'index abordable**, et il est mesuré :

```text
trigrammes vus <= 64 fois   :  96 % des grammes DISTINCTS,  9,5 % des postings
trigrammes vus <= 512 fois  :  98 % des grammes distincts, 14,3 % des postings
trigrammes vus <= 4096      : 100 % des grammes distincts, 34,9 % des postings
les plus fréquents : « ess » (280 191), « ion » (238 307), « sio » (215 229)
```

On n'indexe donc que les grammes `df <= seuil`. **Seuil 4096 recommandé** : il
sert *tous* les termes de la plainte, là où 1024 laisse `auth` (1 226 vraies
lignes sur `qn`) retomber sur le balayage — le surcoût de 22 Mo est le prix de
cette couverture.

**Effet de bord précieux** : une requête dont tous les grammes sont communs
(`CONTAINS 'ion'`) n'obtient rien de l'index et **retombe sur le balayage** —
lent mais juste, jamais amputé.

## 5. La doctrine, tenue par construction

1. **Sur-ensemble seulement.** `candidats()` rend des slots candidats ;
   l'appelant revérifie par l'ordre de lecture complet (la boucle de
   revérification de `find_nodes_in_range`, désormais **par lot**).
2. **Deux silences à ne pas confondre** — et c'est la subtilité du module :
   * `None` = **je ne sais pas** (terme plus court que la fenêtre, ou tous ses
     grammes trop communs) → l'appelant balaie ;
   * `Some(vec![])` = **je sais qu'il n'y a rien** (intersection vide, ou
     gramme **absent du corpus entier** — la passe de comptage a vu toutes les
     valeurs) → zéro ligne, sans balayer.
   Confondre les deux coûte, dans un sens, 348 000 lignes balayées pour rien ;
   dans l'autre, un « zéro résultat » définitif et faux.
3. **L'intersection est BORNÉE** : le seuil borne chaque liste à `seuil`
   entrées, donc un terme de longueur `L` coûte au pire `L × seuil`
   (8 × 4096 = 32 768 pour `obrain-iam`) — là où un index complet laisse un
   seul gramme fréquent en porter 280 000.
4. **Il se rebâtit depuis les valeurs.** Périmé ou corrompu : on le jette, le
   balayage reprend.

## 6. Les deux chemins, mesurés par la MÊME primitive — et les mêmes lignes

`verdicts_texte_batch` (`GraphStore`) rend un verdict par ligne **sur les
octets mmap, sans allouer d'`ArcStr`** : c'est à la fois le PRÉDICAT du chemin
balayé et la REVÉRIFICATION du chemin indexé. Comparer les deux avec le même
outil est la seule comparaison honnête — et elle porte une vérification.

`bench_trigrammes`, seuil 4096, store figé :

### `name` (279 084 valeurs)

| terme | balayage | index + revérif | gain | lignes |
|---|---|---|---|---|
| `iam` | 164,32 ms | **0,033 ms** | **5 042×** | 12 = 12 ✓ |
| `auth` | 187,81 ms | **0,702 ms** | 268× | 215 = 215 ✓ |
| `Auth` | 240,11 ms | 0,150 ms | 1 598× | 150 = 150 ✓ |
| `token` | 174,18 ms | 0,301 ms | 579× | 257 = 257 ✓ |
| `Token` | 259,17 ms | 0,504 ms | 514× | 489 = 489 ✓ |
| `obrain-iam` | 142,58 ms | **0,023 ms** | 6 124× | 2 = 2 ✓ |
| `IAM` | 134,55 ms | 0,010 ms | 13 761× | 0 = 0 ✓ |
| `zzqq-absent` | 224,41 ms | 0,011 ms | 21 151× | 0 = 0 ✓ |
| `ion` | 144,04 ms | — | — | l'index se TAIT → on balaie |

### `qualified_name` (347 748 valeurs)

| terme | balayage | index + revérif | gain | lignes |
|---|---|---|---|---|
| `iam` | 844,40 ms | **0,948 ms** | 891× | 386 = 386 ✓ |
| `IAM` | 420,35 ms | 0,033 ms | 12 630× | 10 = 10 ✓ |
| `auth` | 384,24 ms | **1,138 ms** | 338× | 1 226 = 1 226 ✓ |
| `Auth` | 262,17 ms | 0,323 ms | 811× | 208 = 208 ✓ |
| `token` | 350,65 ms | 0,576 ms | 609× | 491 = 491 ✓ |
| `Token` | 263,36 ms | 0,775 ms | 340× | 545 = 545 ✓ |
| `obrain-iam` | 220,40 ms | 0,342 ms | 645× | 371 = 371 ✓ |
| `zzqq-absent` | 288,50 ms | 0,017 ms | 17 151× | 0 = 0 ✓ |
| `ion` | 288,69 ms | — | — | l'index se TAIT → on balaie |

**16 vérifications, 16 fois MÊMES LIGNES, aucune divergence, aucun faux
négatif.** C'est la partie « les bons résultats » : le gain n'est pas acheté
par une approximation.

## 6 bis. Et à la REQUÊTE — `bench_texte_gql`, même binaire, même store

Le prédicat n'est pas la requête. `crates/obrain-engine/examples/bench_texte_gql.rs`
exécute les requêtes GQL **réelles** de la plainte dans un processus qui vit,
avec (`défaut`) et sans (`OBRAIN_TRIGRAMMES=0`) l'index :

| requête | sans index | avec index | gain | lignes |
|---|---|---|---|---|
| `iam` (3 termes, 2 props) | 108,39 ms | **0,21 ms** | **516×** | 30 = 30 ✓ |
| `auth/token` (4 termes) | 81,71 ms | **0,73 ms** | 112× | 30 = 30 ✓ |
| `count qn CONTAINS 'iam'` | 80,93 ms | **0,04 ms** | **2 023×** | 1 = 1 ✓ |
| `count name CONTAINS 'token'` | 77,33 ms | **0,03 ms** | 2 578× | 1 = 1 ✓ |
| témoin commun `ion` | 78,59 ms | 75,17 ms | — | index muet → balayage, inchangé |
| témoin absent `zzqq-absent` | 78,70 ms | **0,00 ms** | — | absence prouvée, sans balayer |

### ⚠ La réserve qui décide de tout : la DURÉE DE VIE DU PROCESSUS

L'index est **construit à la demande, une fois par processus et par
propriété**. Mesuré au CLI (`obrain query`, **un processus par requête**) :
la requête du 12/09 passe de 1,10 s à **2,32 s** — la construction (0,26 s pour
`name`, 2,94 s pour `qualified_name`) est repayée à CHAQUE appel. Les chiffres
du tableau ci-dessus valent donc pour un processus qui vit : **le pont MCP, le
`viz serve`, un banc** — pas pour le CLI jetable.

## 6 ter. La GARDE ADAPTATIVE — le remède ne doit pas être pire que le mal

Puisque le CLI est un processus par requête, la construction ne doit pas être
payée pour une seule requête. `noeuds_par_predicat_texte` compte donc les
demandes **par propriété et par processus** : la première rend `None` (donc le
balayage, coût connu), la **deuxième** construit, les suivantes coûtent des
microsecondes. Réglable par `OBRAIN_TRIGRAMMES_DES_LA` (1 = ancien
comportement, qui ne convient qu'aux bancs et aux processus qu'on sait durables).

**A/B ENTRELACÉ**, la même requête, le cache chaud pour toutes les mesures :

| configuration | tour 1 | tour 2 | tour 3 | lignes |
|---|---|---|---|---|
| sans index (témoin) | 154,0 ms | 160,0 ms | 156,8 ms | 30 |
| **garde par défaut (2)** | **149,7 ms** | **155,0 ms** | **155,8 ms** | 30 |
| `DES_LA=1` (construction à la 1re) | 2,06 s | 2,06 s | 1,92 s | 30 |

**Zéro régression** : avec la garde, le CLI se comporte exactement comme avant
l'index, à la milliseconde près. Et le gain du processus qui vit est intact —
banc GQL, garde par défaut : 1er tour **147,3 ms** (il balaie), puis
**0,22 ms** au régime établi, contre 87,16 ms sans index.



### Ce que ces chiffres ne disent PAS — et qu'il ne faut pas leur faire dire

* La colonne « balayage » est le coût du **prédicat seul**, sur toutes les
  lignes de la propriété, mesuré par la même primitive. La requête GQL
  complète mesurée au CLI coûte 148–190 ms : le plan et la projection
  s'ajoutent **aux deux** colonnes.
* La colonne « index » **n'inclut pas la construction** de l'index
  (0,26 s pour `name`, 2,94 s pour `qualified_name`, **par processus**) : la
  première requête la paie. C'est exactement le grief que la zone persistée a
  déjà résolu pour l'index d'égalité — d'où l'étape 1 de la suite.
* Le coût du **plan** après branchement (ouverture de l'index, passage des
  candidats dans l'opérateur) n'est pas mesuré ici : c'est ce que le
  branchement devra établir.


## 7. Ce qui reste, et ce qui ne reste plus

1. 🔶 **PERSISTANCE — utile, mais PLUS bloquante, et la mesure l'a montré.**
   La garde adaptative (§6 ter) a résolu le cas qui motivait la persistance :
   un processus jetable ne construit plus rien, donc ne paie plus rien. Ce qui
   reste à gagner est étroit : un processus qui **vit mais redémarre souvent**
   (relancer un pont MCP toutes les quelques minutes) paierait 0,26–2,94 s par
   redémarrage. La zone persistée (pages, CRC, ancres, reconstruction depuis
   v2 — sur le modèle de `props_index` / `props_eq_index`) le supprimerait.
   À faire quand ce cas se présentera, pas avant.
2. ✅ **Branchement dans le planificateur — FAIT.** `try_plan_filter_with_text_index`
   (`crates/obrain-engine/src/query/planner/lpg/filter.rs`) sert `CONTAINS` et
   les disjonctions de `CONTAINS` sur la même variable, avec le balayage
   conservé comme filet dès qu'une condition n'est pas servie. Il ne prend QUE
   si tout le prédicat est couvert : servir une partie rendrait des lignes
   fausses, en silence.
3. ✅ **Mesure de bout en bout de la REQUÊTE — FAITE** (§6 bis), sur le même
   binaire et le même store, avec le nombre de lignes comparé à chaque fois.
4. ✅ **Aucune régression en processus court — FAITE ET MESURÉE** (§6 ter).

### État des preuves

| preuve | où | résultat |
|---|---|---|
| cœur de l'index (sur-ensemble, silences, seuil, tampon) | `props_trigram_index.rs` | **14 tests verts** |
| exactitude, écriture après construction, réécriture, silences, garde | `tests/trigrammes_bout_en_bout.rs` | **5 tests verts** |
| prédicat : index vs balayage, ligne par ligne | `examples/bench_trigrammes.rs` | 16 × mêmes lignes |
| requête GQL : avec/sans index, même binaire | `examples/bench_texte_gql.rs` | 6 requêtes, lignes identiques |
| CLI en processus court : aucune régression | A/B entrelacé, §6 ter | 149,7 / 155,0 / 155,8 ms contre 154,0 / 160,0 / 156,8 ms |
| suite moteur complète | `cargo test -p obrain-engine --no-fail-fast` | **2 036 passés, 0 échec** (149 binaires) |

**Ce qui n'est PAS prouvé** : qu'un processus qui redémarre souvent y gagne —
aujourd'hui il ne construit pas, donc il balaie. C'est le seul cas restant, il
est identifié, et son remède (la persistance) est chiffré.



## 8. Leviers voisins, mesurés séparément

* **Chemin d'intervalle** (`>=`, `<`, `STARTS WITH`) : l'index ordonné sert
  déjà (18,3 ms contre 1 430 ms pour un intervalle vide en espace de clé), mais
  sa clé est **tronquée à 11 octets** : sur des valeurs à long préfixe commun
  il énumère 127 200 candidats pour 368 lignes attendues. Élargir la clé est une
  **course perdue** (mesuré : le genou est à 50-51 octets *pour cette borne*,
  soit 55 octets par entrée, et il se déplace avec chaque borne plus longue).
* **Le coût fixe de 17 ms** du balayage blob, sur ce chemin, est plus rentable à
  attaquer que la finesse de l'index : il parcourt 347 970 créneaux pour
  2 valeurs présentes.
* **`STARTS WITH` n'est pas poussé** vers l'index ordonné aujourd'hui :
  152 ms avec ou sans l'index — la poussée n'existe pas.
