# Bancs d'essai — savoir quand on avance et quand on régresse

## En deux commandes

```bash
export CARGO_TARGET_DIR=/tmp/bancs/target      # JAMAIS target/release : le binaire de production en sort

# 1. Poser une référence, avant de toucher au code
cargo bench -p obrain-substrate --bench vec_write -- --save-baseline avant

# 2. Après le changement, comparer
cargo bench -p obrain-substrate --bench vec_write -- --baseline avant
```

Criterion imprime alors, par mesure, l'écart en pourcentage, un intervalle de
confiance et une valeur de p, plus une phrase : « Performance has improved »,
« has regressed » ou « No change in performance detected ».

Corpus réduit pour itérer vite :

```bash
VEC_WRITE_BENCH_N=2000 cargo bench -p obrain-substrate --bench vec_write
```

## La règle qui rend ces chiffres lisibles — le contrôle à blanc

**Avant de croire un écart, compare le code à SA PROPRE référence.**

```bash
cargo bench -p obrain-substrate --bench vec_write -- --save-baseline temoin
# sans rien changer :
cargo bench -p obrain-substrate --bench vec_write -- --baseline temoin
```

L'écart attendu est zéro. Toute mesure qui annonce un changement significatif
ici **ne mesure pas** : elle devine. On ne lit alors AUCUN de ses verdicts.

Ce n'est pas de la prudence rituelle. Le 2026-08-21, la première version de ce
banc a annoncé `serie −39,3 % (p = 0,00)` — sur du **code identique**. Une
régression de 39 % avait failli être attribuée à un correctif qui n'y était pour
rien. Le harnais reconstruisait une base de N nœuds à chaque itération :
criterion excluait bien ce montage du chronomètre, mais pas du cache de pages ni
de l'ordonnanceur. Le montage vit désormais hors de `b.iter`.

## Ce qui est mesurable sur cette machine, et ce qui ne l'est pas

Contrôle à blanc du 2026-08-21, hôte à charge ~25-50, service consommant ~10
cœurs sur 20 :

| mesure | écart sur code identique | utilisable |
|---|---|---|
| `vec_write/serie` | −0,37 % (p = 0,34) | **oui** — ±0,4 % |
| `vec_write/lot` | ±1 % | **oui** |
| `vec_write/concurrent/2` | +1,0 % (p = 0,90) | oui, mais intervalle large |
| `vec_write/lecture_pendant_ecriture/4` | +1,4 % (p = 0,37) | oui |
| `vec_write/concurrent/4` | **+59,7 % (p = 0,00)** | **non** |
| `vec_write/concurrent/8` | **+47,9 % (p = 0,00)** | **non** |
| `vec_write/lecture_pendant_ecriture/1` | **−22,7 % (p = 0,00)** | **non** |

Les mesures multi-fils se disputent les cœurs avec `obrain-tmn`. Ce n'est pas
réparable par un réglage de criterion : il leur faut un hôte au repos, ou un
épinglage de cœurs (`taskset`). En attendant, **`serie` et `lot` sont les
garde-fous** — ce sont eux qui couvrent le chemin d'écriture.

## Une référence vieillit

Deux mesures dos-à-dos s'accordent à ±0,4 %. Dix minutes plus tard, la même
mesure sur du code inchangé affichait +4,8 % avec p = 0,00. Un contrôle à blanc
ne vaut **que dans la session où il est pris**. Repose une référence fraîche
avant chaque campagne ; ne compare pas à celle d'hier.

## Ce que mesure `vec_write`

Le chemin `route_vector_write` — `set_node_property` avec une `Value::Vector`,
qui route vers une colonne dense mmappée. C'est le chemin qu'emprunterait la
persistance de `_hilbert_features` : un vecteur par nœud, 626 845 nœuds sur la
base `agent`.

| bras | ce qu'il mesure | pourquoi il existe |
|---|---|---|
| `serie` | écriture unitaire, un fil | garde-fou principal du chemin chaud |
| `lot` | `ecrire_vecteurs_en_lot` | l'écart avec `serie` est le coût de la résolution répétée |
| `concurrent/{2,4,8}` | N fils, même colonne | borne haute du débit ; le `RwLock` par colonne les sérialise légitimement |
| `lecture_pendant_ecriture/{1,4}` | lecteurs pendant qu'un fil écrit | **la forme réelle** : un enrichissement tournant sous un service qui sert des rappels |

Une régression sur `lecture_pendant_ecriture` est grave : elle signifie qu'un
verrou de portée base est revenu dans le chemin d'écriture unitaire. C'est
exactement ce qui existait avant le 2026-08-21 — `route_vector_write` prenait le
`Mutex<SubstrateFile>` global une fois par vecteur, pour protéger une ressource
utilisée seulement à la création de la colonne.

## Repères actuels (2026-08-21, N = 5 000, dim 72)

```
vec_write/serie/5000    2 296 µs    2,18 Melem/s    ~459 ns par écriture
vec_write/lot/5000        476 µs   10,50 Melem/s     ~95 ns par écriture
```

Le `memcpy` utile dans ces 459 ns en coûte une dizaine. Le reste est de la
résolution — allocation `PropertyKey` (c'est un `ArcStr`), verrou du registre de
clés, deux recherches DashMap avec leurs verrous de shard — refaite pour une clé
et une colonne qui ne changent jamais dans un lot.

## Deux pièges de lecture, présents ailleurs dans le dépôt

**Le temps de lot pris pour un temps unitaire.** `hash_index_lookup`
(obrain-core) annonce 27,1 µs, mais son `b.iter` contient `for i in 0..1000`.
Le coût unitaire est 27,1 **ns** — facteur mille, et rien dans la sortie ne le
signale car le banc ne déclare pas de `Throughput`. Même motif dans
`bench_adjacency_lookup` (×100), `bench_adjacency_insert` et
`bench_hash_index_insert` (×1000). Tout banc ajouté ici **doit** déclarer
`Throughput::Elements`.

**Le banc CRUD n'en est pas un.** `bench/substrate/crud.rs`, que son README
présente comme « criterion micro-bench for CRUD », a pour corps entier
`b.iter(|| black_box(0u64))`. Avant `vec_write`, il n'existait aucune mesure
d'écriture dans le dépôt.

**Et `index_bench` ne compile pas sans sa feature :**

```bash
cargo bench -p obrain-core --features vector-index --bench index_bench
```

## `memoire_vs_fichier` — le substrat EN MÉMOIRE contre le substrat FICHIER

```bash
export CARGO_TARGET_DIR=/tmp/bancs/target
MEM_BENCH_N=200 cargo bench -p obrain-substrate --bench memoire_vs_fichier \
    -- --measurement-time 2 --warm-up-time 1 --sample-size 10
```

Né le 2026-09-17 avec le substrat en mémoire
(`docs/ADR-2026-09-17-SUBSTRAT-EN-MEMOIRE.md`) : le mode « en mémoire » du moteur
a cessé d'être un répertoire temporaire mmapé pour devenir des `Vec<u8>`. Ce
changement était nécessaire (wasm32 n'a pas de système de fichiers), mais il
DÉPLACE le stockage — la question « qu'est-ce que ça coûte, qu'est-ce que ça
rapporte » n'avait aucune mesure. Les deux bras partagent la même API
(`SubstrateStore`), seul le support diffère.

**MESURÉ le 2026-09-17 — DEUX runs, deux verdicts. Lire le second.**

Le premier run (`--sample-size 10`, charge 4,09 → 6,75) annonçait la mémoire
**28,6 % plus lente** en lecture unitaire et **52,1 % plus lente** en hydratation.
Le second run (`--sample-size 100`, charge 1,56 → 4,12) donne ceci :

| bras | fichier (temp + mmap) | mémoire | écart |
|---|---|---|---|
| `ouverture` | 6,547 ms | **51,0 µs** | mémoire **128× plus rapide** |
| `ecriture/creation_noeuds` (par 200) | 209,7 µs | **27,3 µs** | mémoire **7,7× plus rapide** |
| `ecriture/pose_propriete` | 991,4 ns | 994,1 ns | **+0,27 % — identiques** |
| `lecture/propriete_unitaire` | 100,39 ns | 101,28 ns | **+0,9 % — identiques** |
| `lecture/hydratation_complete` | 350,31 ns | 350,93 ns | **+0,2 % — identiques** |

**Contrôles à blanc du second run :**

| paire | écart mesuré | verdict |
|---|---|---|
| `ouverture/memoire` vs `memoire_controle` | **+0,60 %** | mesure saine |
| `lecture/propriete_unitaire/memoire` vs `..._controle` | **+0,43 %** | mesure saine — **résolution ~0,5 %** |

### LA LEÇON, ET ELLE EST LE VRAI RÉSULTAT DE CE BANC

Les écarts de lecture du premier run étaient **du bruit**. Ce n'est pas une
hypothèse : c'est ce que disent les contrôles à blanc, passés de **+6,4 %** à
**+0,43 %** de dérive entre les deux runs. Le premier tournait à charge 4-6 avec
10 échantillons ; le second à charge 1,5 avec 100. Exactement le piège décrit en
tête de ce document (« toute mesure qui annonce un changement significatif sur le
contrôle à blanc ne mesure pas : elle devine »).

**Conclusion du banc :** le substrat en mémoire ne coûte RIEN en lecture
(±1 %) et il divise le coût d'ouverture par **128** et la création de nœuds par
**7,7**. La seule vraie réserve est la mémoire physique, en octets — non mesurée.

**Ce qu'il faut retenir pour le prochain qui lance ce banc :** vérifier
`/proc/loadavg` AVANT, garder `--sample-size 100`, et lire les deux paires de
contrôle à blanc avant toute conclusion. Un run à charge 4-6 avec 10 échantillons
produit des écarts de 30 à 50 % sur du code dont la différence réelle est nulle.

---

## AXE APPEND — FIFO multi-écrivains du WAL (2026-10-06)

Banc : `cargo bench -p obrain-substrate --bench wal_append -- --warm-up-time 1
--measurement-time 3 --sample-size 30` (criterion ; `SyncMode::Never`, donc on
mesure le chemin d'écriture, pas le fsync). Record de ~90 octets (une propriété
scalaire inline : clé courte + valeur bincode), la taille d'un write de graphe
courant. Machine : même poste que les bancs ci-dessus, charge notée au moment du
run.

### 1. Append unitaire — un `write` syscall par record

| régime | temps/record | débit | ce que c'est |
|---|---|---|---|
| `seul_ecrivain` (`CoordinationAppend::Aucune`) | **997 ns** | **1,003 M rec/s** | LE CHEMIN CHAUD : `Writer::new` sur un store ouvert en exclusif (verrou de répertoire `LOCK_EX`) |
| `partage_oappend` (défaut de `WalWriter::open`) | **2 899 ns** | 345 k rec/s | WAL partagé : + 2 `fstat` (garde d'append + vérification post-écriture) |
| `partage_verrou` (`VerrouExact`, `flock`) | **4 071 ns** | 246 k rec/s | LSN exact partout, au prix du verrou |

**AVANT / APRÈS (même corps de test, `tests/wal_latence_append.rs`, 200 000
records)** :

| | avant le fix | après (chemin chaud) | écart |
|---|---|---|---|
| latence par record | **1 217 ns** (0,82 M rec/s) | **997 ns** (1,003 M rec/s) | **−18 %** |

Le fix **n'a pas dégradé** le chemin chaud : il l'a légèrement amélioré, parce
qu'un `write` unique coûte moins qu'un `write_all` et que le `fetch_add` de LSN a
disparu. Le surcoût des régimes partagés (2,9 µs / 4,1 µs) est NOMMÉ et ne touche
que les stores en écriture partagée.

### 2. Group commit (`append_batch` : UN `write` pour N records)

| records par lot | temps | débit | ×vs unitaire |
|---|---|---|---|
| 1 | 1 072 ns | 0,93 M rec/s | ×1,0 |
| 8 | 2 140 ns | 3,74 M rec/s | ×4,0 |
| 64 | 11 433 ns | 5,60 M rec/s | ×6,0 |
| 256 | 40 364 ns | 6,34 M rec/s | ×6,8 |
| 1024 | 157 530 ns | **6,50 M rec/s** | **×6,9** |

**Écart avec la RFC** : le §3.1ter du `multi-writer.md` annonçait **25,5 M/s vs
1 M/s (×22)** — mais c'était un PROTO (un `write_all` groupé sur un buffer déjà
rempli, sans encodage). Sur le VRAI `WalWriter`, le group commit vaut **×6,9**
(6,50 M/s). Le levier est réel et c'est le seul du chemin d'écriture, mais le
chiffre de la RFC ne doit pas être cité comme une mesure du produit.

### 2 bis. Re-mesure FINALE (même jour, après la dernière modification)

Dernière passe de `cargo bench -p obrain-substrate --bench wal_append --
--warm-up-time 1 --measurement-time 3 --sample-size 30`, après avoir remis
l'ENCODAGE hors du verrou (prédiction de position vérifiée sous le verrou) :

| bras | temps | débit |
|---|---|---|
| `wal_append_unitaire/seul_ecrivain` | **1 021 ns** | 0,98 M rec/s |
| `wal_append_unitaire/partage_oappend` | 2 820 ns | 355 k rec/s |
| `wal_append_unitaire/partage_verrou` | 4 041 ns | 247 k rec/s |
| `wal_append_batch/64` | 11 431 ns | 5,60 M rec/s |
| `wal_append_batch/1024` | 161 570 ns | **6,34 M rec/s** (×6,3 vs unitaire) |

Les écarts avec la passe précédente (997 ns / 2 899 / 4 071 / 157,5 µs) sont dans
le bruit du banc (±1-2 %), sauf le lot de 1 024 (+2,6 %), qui est le seul bras où
l'encodage est refait quand la prédiction tombe faux — négligeable.

### 3. Débit INTER-PROCESSUS à N écrivains (T2, `mw_interprocess_append.rs`)

6-8 processus, `O_APPEND`, payloads de 0 à 600 octets, motifs distincts par
(pid, seq). Mesuré dans le test (chiffres imprimés à chaque exécution) :

| N processus | records | débit cumulé |
|---|---|---|
| 1 | 20 000 | 204 k rec/s |
| 2 | 40 000 | 377 k rec/s |
| 4 | 80 000 | 689 k rec/s |
| 8 | 160 000 | **1 017 k rec/s** |

**Facteur de scale N=1 → N=8 : ×4,7 à ×5,1** (selon le run), avec **zéro perte** à
chaque point. C'est la preuve que la FIFO SCALE — `O_APPEND` laisse le noyau
sérialiser le placement sans qu'aucun verrou applicatif ne s'interpose. Le régime
`VerrouExact` (LSN exact) donne, lui, **×0,64** : le débit cumulé DIMINUE quand N
monte. C'est pourquoi il n'est pas le défaut.

### 4. La mesure qui compte le plus : la PERTE avant le fix

`tests/mw_interprocess_preuve.rs` (T2 dans sa forme dure, 6 processus × 2 000
frames, sous-ensemble d'API commun aux états pré/post-fix) :

| | frames attendues | frames relues | perdues |
|---|---|---|---|
| **AVANT** (sans `O_APPEND`) | 12 000 | 2 000 | **10 000 — 83,3 %** |
| **APRÈS** | 12 000 | 12 000 | **0** |

C'est le chiffre qui justifie tout le chantier, et il est reproductible : il suffit
de retirer `.append(true)` de `WalWriter::open_avec_ancre` pour le revoir.
(Sonde historique `p1`, qui comparait le sort des frames de DEUX processus :
`de_b = 0` avant, `de_b = M` après.)
