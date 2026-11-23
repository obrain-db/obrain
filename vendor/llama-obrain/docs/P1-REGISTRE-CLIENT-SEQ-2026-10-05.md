# Registre client → séquence + budget du cache d'états — P1 « 1M jetons par client » (2026-10-05)

Worktree : `~/projects/llama-obrain-cuda-f1a` — branche **`feat/kv-splice-chat`**, base **`9fcd772`**
(import incrémental P0 déjà commité). Le worktree contenait **déjà** des modifications non commitées
qui ne sont pas de ce chantier (`tools/obrain/pstate-exp0a.cpp`, 24/09 ; `tools/server/CMakeLists.txt`,
02/10) : elles n'ont pas été touchées, et rien n'a été commité (voir §7).

Build de vérification : **`~/build-p1`** (CPU, `-DGGML_CUDA=OFF -DLLAMA_CURL=OFF`, `Release`).
`build-cuda/bin/llama-server` — le binaire du service 9910 — est resté **intact**
(mtime `2026-10-05 14:52:32`, inchangé) ; 9911 a répondu `{"status":"ok"}` à la fin du chantier.
Moteur temporaire : ports **9996-9999** uniquement, lancé et arrêté par le script de mesure.

---

## 0. Les deux prémisses, confrontées au code de la base

| Prémisse | Ce que le code dit à `9fcd772` |
|---|---|
| `jspace_reserved_slot_id()` = `n_parallel-1` « à `server-context.cpp:1908-1910` » | **Vrai, lignes déplacées** : `server-context.cpp:2152-2154` (`n_parallel >= 3 ? n_parallel - 1 : -1`). |
| « un client qui demande le slot réservé l'obtient » | **Vrai, et c'est le défaut** : l'exclusion n'existe que dans les **deux boucles automatiques** de `get_available_slot` (`slot.id == reserved_id` → `2268` et `2318`). Le chemin `task.id_slot != -1` (`2254`) appelait `get_slot_by_id(task.id_slot)` **sans** la connaître. |
| « le client envoie `id_slot = hash(cid) % total_slots` » | **Vrai** : `wt-chat-kv-splice`, `api/src/chat/inference.rs` (`ChatSlot`), et `total_slots` était publié comme `params.n_parallel`. |
| « l'extraction fait `seq_rm` qui efface le KV du fil » | **Vrai** : `llama_memory_seq_rm(mem, seq_id, -1, -1)` aux deux entrées J-space (avant patch : `3679`, `3726`, `3775`, `4156`, `4076`). |
| « `g_kv_states` = `std::unordered_map` non borné (`server-context.cpp:61-63`), la borne `OBRAIN_KV_STATES_MAX_BYTES` **n'existe pas** » | **FAUX À CETTE BASE.** Le cache d'états a été extrait dans **`tools/server/obrain-kv-cache.h`** (commit `7faac3d`, retouché par `9fcd772`) : `server-context.cpp:71-72` construit `obrain_kv::Cache g_kv_cache(obrain_kv::Cache::default_max_bytes())`, et `Cache` **a déjà** un budget d'octets + éviction LRU + compteurs (`obrain-kv-cache.h`, `struct Cache::insert/get`). `default_max_bytes()` lit bien `OBRAIN_KV_STATES_MAX_BYTES` **en octets**, défaut 2 Gio. **Le verrou n°2 était donc déjà levé** — il n'était ni *vérifié*, ni *mesuré*, ni *observable* : c'est ce que ce chantier ajoute (§2, §4). |
| « réutilise la politique déjà écrite de `server_prompt_cache::update` (`server-task.cpp:1850-1881`) » | **Lignes déplacées** : `1850-1881` est aujourd'hui `server_prompt_cache::load`. La politique visée est `server_prompt_cache::update` (`1892-1923`) : `while (size() > limit_size) states.pop_front()`, budget en octets, `limit_size` initialisé en **MiB** par le constructeur `server_prompt_cache(int32_t limit_size_mib, …)` (`server-task.h:958`). `obrain_kv::Cache` a la même forme mais est un **vrai LRU** (touch sur `get`) là où le cache de prompts est FIFO-avec-effacement-à-l'usage. Le nom du paramètre `Cache(size_t mb)` (valeur en **octets**) venait de cette confusion MiB/octets : corrigé (§5, bug 7). |

---

## 1. Le registre client → séquence (livrable 1)

**Où il vit.** Nouveau fichier **`tools/server/obrain-client-seq.h`** (367 lignes) — même doctrine que
`obrain-kv-cache.h` : le serveur **et** les tests incluent le **même** code, donc la politique testée
est celle qui tourne. Il est instancié comme membre de `server_context_impl` :
`obrain_seq::Registre registre_client;` (`server-context.cpp:1561-1567`), configuré **après** la
création des slots — la règle dépend de `n_parallel` :

```cpp
registre_client.configurer((int32_t) slots.size(), jspace_reserved_slot_id());   // server-context.cpp:1967
```

**Le mapping est tenu par trois structures** (dans `Registre`) :

| structure | rôle | pourquoi elle est nécessaire |
|---|---|---|
| `attr_ : unordered_map<string,int32_t>` | **clé client → séquence** | le mapping lui-même (stable : c'est ce qui préserve l'épinglage du préfixe) |
| `pris_ : vector<string>` | **séquence → clé** (`""` = libre) | sans l'inverse, deux clients pourraient viser la même séquence sans que le registre le voie ; c'est aussi ce qui répond à « la séquence réservée appartient-elle à un client ? » |
| `lru_ : list<string>` | ordre d'usage (front = plus récent) | saturation : savoir **quelle** clé inactive peut céder sa séquence |

**Clé du client** (`cle_client_de`, `server-context.cpp:2247`), par ordre de préférence :

1. `"cid:" + client_id` — champ **`client_id`** facultatif ajouté au corps de requête
   (`server-context.cpp:6985`, `server_task::client_id`, `server-task.h:183`) : deux conversations qui
   fournissent deux identifiants distincts obtiennent **deux séquences distinctes**, même si leurs
   `id_slot` hachés sont identiques ;
2. à défaut `"slot:" + id_slot` — la seule identité dont le serveur dispose aujourd'hui, et elle est
   déjà stable par conversation (le client la dérive de son cid). **Aucun client existant n'a besoin
   d'être modifié** pour bénéficier du correctif ;
3. clé vide (ni `client_id` ni `id_slot`) → `-1` : sélection automatique du serveur, **inchangée**.

**Où le serveur s'en sert** :

| site | ce qui change |
|---|---|
| `get_available_slot` (`2466`) | `const int32_t id_slot_effectif = seq_client(task);` remplace `task.id_slot` dans les **trois** usages (sélection par id, filtre de similarité, message LRU) |
| `get_free_slots` (`3344`) | la séquence réservée est exclue : les tâches **filles** (n_cmpl>1, spéculation) ne peuvent plus y installer leur KV — même défaut, autre porte |
| `jspace_reserved_slot_id()` (`2227`) + `/props` (`7438`) | délèguent à `jspace_reserved_slot_id_of` / `jspace_slots_offerts_of` (`67-76`) : **une seule définition** du calcul, partagée par la classe et la route |
| `reserver_sequence_pour_jspace` (`2315`), appelé aux **deux** entrées J-space | garde-fou **avant tout `seq_rm`** |
| `seq_client` (`2255`) | journalise le mapping **au moment où il est établi** (`[obrain-p1] client '…' -> sequence N`) |

**Politique d'attribution** (le « pourquoi » est dans l'en-tête du fichier) :

1. jamais la séquence réservée ; jamais une séquence hors du pool offert ;
2. **stable** : la même clé garde la même séquence ;
3. un client qui visait la séquence réservée est **re-mappé**, pas ignoré : l'ignorer le renverrait
   dans la sélection LRU du serveur, donc lui ferait perdre son préfixe à chaque étape — le correctif
   coûterait le bénéfice de la fonctionnalité ;
4. **pool saturé** (plus de clients distincts que de séquences offertes) : on reprend la séquence de
   la clé **la plus froide** — mais **seulement si elle est inactive** ; sinon on rend `-1` (sélection
   automatique du serveur, qui exclut déjà la réservée). L'état vivant est fourni par un **oracle
   injecté** (`Attribution attribuer(cle, demande, OccupationFn)`) que le serveur branche sur
   `!slots[s].is_processing()` — le registre ne dépend pas de `llama.h`, et une seule politique est exercée ;
5. `/props` publie `slots_offerts = n_parallel - 1`, donc l'espace de hachage du client
   (`hash(cid) % total_slots`) **coïncide avec le pool réel** : le cas de saturation ne se produit plus
   pour un client conforme. **C'est le second volet du correctif**, sans toucher au client.

---

## 2. Budget + éviction LRU du cache d'états (livrable 2)

**Déjà écrit, désormais vérifié.** `obrain_kv::Cache` (`obrain-kv-cache.h`) :
`insert()` refuse un blob plus gros que le budget, évince la **plus ancienne** jusqu'à tenir dans
`max_bytes`, tient `bytes` de façon exacte ; `get()` **touche** l'entrée (LRU) ; compteurs
`n_insert/n_replace/n_evict/n_refuse_too_big`. Budget : `OBRAIN_KV_STATES_MAX_BYTES` **en octets**,
défaut `2 Gio`. C'est le chat qui a raison (`api/src/chat/mod.rs:1228`), et la valeur est bien en octets.

Ce qui a été **ajouté** ici :

- **Observabilité** : ligne `[kv-cache] apres ref=… : cache_bytes=… cache_entrees=… cache_budget=…
  cache_evictions=…` après **chaque** insertion (`server-context.cpp:4629`), et renommage des champs
  de `[kv-extract]` en `cache_avant_*` (cette ligne est émise **avant** l'insertion du blob, donc ses
  chiffres décrivent l'état d'avant — les laisser nommés `cache_*` faisait lire un décompte décalé).
  Une borne qu'on ne peut pas lire ne se vérifie pas.
- **Tests** : sémantique de la variable d'environnement (unités, valeur invalide, défaut) et budget
  **sous charge** (64 insertions, borne jamais dépassée, évictions comptées) — §3.
- **Correction d'un piège** : le paramètre s'appelait `Cache(size_t mb)` alors que la valeur est en
  **octets** (`Cache(4096)` = 4096 octets) — renommé `max_bytes_octets`, l'unité est dite à l'endroit
  où on la lit (§5, bug 7).

---

## 3. Tests

| test | résultat |
|---|---|
| `tools/server/obrain-client-seq-tests.cpp` (**nouveau**, 258 lignes) | **71/71 PASS** |
| `tools/server/obrain-kv-cache-tests.cpp` | **86/86 PASS** (74 avant + 12 nouveaux) |
| Build CPU `~/build-p1` (base puis patché, deux fois) | **EXIT=0** |

Couverture du registre (sections A-I du fichier de test) : configuration (`slots_offerts`, réservation
absente/hors bornes/pool vide) · **deux clients → deux séquences distinctes, aucune sur la réservée** ·
stabilité de l'épinglage (1 attribution + 2 réutilisations) · **le client qui vise la séquence réservée
est re-mappé, et le re-mappage est stable** · **50 attributions sur un pool de 3 : jamais la réservée,
toujours une séquence offerte, la réservée reste libre, le registre ne détient jamais plus de clés que
de séquences** · saturation : la plus froide cède, **une plus froide mais ACTIVE ne cède pas**, toutes
actives → `-1` et personne ne perd rien · 20 conversations successives → séquences recyclées, pas de
fuite d'épinglage · sans réservation le comportement historique est intact (demande rendue telle
quelle, clé vide → `-1`) · demande hors intervalle (`id_slot=7` → 3 modulo 4, la réservée → re-mappée).

Le garde-fou « une extraction n'efface pas le KV d'un client » est **vérifiable** au niveau du
registre, et c'est ce que les tests affirment : l'ensemble des séquences attribuées aux clients et la
séquence réservée sont **disjoints** (`occupant(reserve)` reste vide, même après 50 attributions).
La mesure §4 le vérifie au niveau HTTP.

---

## 4. Mesure bout en bout (2026-10-05, 22:28 → 22:35, moteur temporaire 9996-9999)

`experiments/mesure-p1-clients-seq.py` (nouveau) — Qwen3-1.7B-Q8_0 (KV f16), `--ctx-size 4096
--parallel 4` (⇒ `n_ctx_slot = 1024`, séquence réservée = **3**, `total_slots` annoncé = 3 après
correctif), `-lv 3`. Chaque exécution lance et arrête son moteur ; le répertoire des états persistés
est relatif au CWD du moteur (donc temporaire, rien n'est écrit dans le `kv-states` de production).

### 4.1 Le défaut, reproduit sur le binaire BASE (`~/build-append-cpu`, 0 occurrence de `obrain-p1`)

Fil A = même conversation, `id_slot=3` (la séquence réservée) ; son préfixe dit
« le code secret du projet est **ROUGE-7** ». Le fait extrait dit « **VERT-3** ».

```
[fil] fil A, 1er appel           prompt_n=568  cache_n=0    -> 'ROUGE-7 … le code secret … est ROUGE-7'
[fil] fil A, 2e (KV chaud)       prompt_n=1    cache_n=567  -> 'ROUGE-7 …'        (préfixe RÉUTILISÉ)
[extract] fait VERT-3   ref=da7678368254c639  591 jetons  67 788 408 o  (2,8 s)
[fil] fil A, 3e (apres extraction) prompt_n=1  cache_n=567  -> 'Le dossier 77 contient des notes de travail…'
```

Lecture : la 3ᵉ réponse **n'a rien re-décodé** (`prompt_n=1`, le serveur croit le préfixe en cache) et
elle lit pourtant le KV du **fait extrait** — le fil a perdu son contexte et **voit le contenu d'une
autre séquence**. Aucune erreur, aucun HTTP non-200, aucune ligne de journal : c'est le défaut annoncé,
et il est reproductible. Journal : `selected slot by id (3)` (le fil est bien sur la séquence réservée).

### 4.2 Le correctif (binaire `~/build-p1`)

```
[obrain-p1] registre client -> sequence : 3 sequence(s) offerte(s), reservee = 3 (slots = 4)
[obrain-p1] client 'slot:3'    -> sequence 0 (demande 3, reservee 3, offertes 3) [RE-MAPPE : la sequence demandee etait INUTILISABLE]
[obrain-p1] client 'slot:1'    -> sequence 1 (demande 1, reservee 3, offertes 3)
[obrain-p1] client 'cid:fil-1' -> sequence 2 (demande 3, …) [RE-MAPPE]
[obrain-p1] client 'cid:fil-2' -> sequence 1 (demande 3, …) [RE-MAPPE] [reprise de la sequence d'un fil INACTIF]
```

| vérification | BASE | P1 |
|---|---|---|
| `total_slots` annoncé par `/props` | 4 | **3** |
| fil A (id_slot=3), 1er appel correct | OUI | OUI |
| fil A, 2ᵉ appel : préfixe réutilisé (`prompt_n=1`) | OUI | OUI |
| **après une extraction : le KV du fil est INTACT (ROUGE-7 rendu)** | **NON** | **OUI** |
| `/slots` : quels slots portent le prompt du fil | slots 1 et **3** | slots **0**, 1, 2 (jamais 3) |
| `client_id` : deux conversations → deux séquences distinctes, hors réservée | NON (champ inconnu) | **OUI** |

`RESULTAT_P1_CLIENTS` : `FAIL` sur la base (défaut reproduit), **`PASS`** sur le binaire corrigé.

### 4.3 La RAM ne croît pas de façon monotone (8 extractions successives, même binaire)

Faits distincts de 854 jetons ⇒ **97 954 508 octets** (93,4 Mo) par blob ; 8 extractions ⇒ **747 Mo**
extraits au total. Seul le budget change entre les deux exécutions :

| | budget **250 000 000 o** | budget **4 000 000 000 o** (témoin) |
|---|---|---|
| octets en cache (max) | **195 909 016** (186,8 Mo ≤ budget) | **783 636 064** (747 Mo) |
| entrées | **2** | **8** |
| **évictions** | **6** | **0** |
| RSS du moteur | 2263 Mo → 2485…2583 Mo, **oscille** (non monotone) | 2263 Mo → **3125 Mo** (monotone, +862 Mo) |
| `RESULTAT_P1_RAM` | **`BORNE`** | `TEMOIN` |

Preuve ligne à ligne (budget 250 Mo) : `cache_bytes` reste à `195909016` de la 2ᵉ à la 8ᵉ extraction
pendant que `cache_evictions` monte de 0 à 6 — le cache **évince** au lieu de croître. Sans budget
contraignant, le cache suit exactement les extractions (`747 Mo` pour `747 Mo`) et la RSS croît de
façon monotone : c'est le budget, et lui seul, qui borne la mémoire.

---

## 5. Bugs trouvés et corrigés en me vérifiant

1. **Le défaut principal, reproduit puis fermé** (§4.1) : un fil épinglé sur la séquence réservée
   voyait son KV effacé par la première extraction, et lisait ensuite le KV d'une **autre** séquence
   sans re-décoder. Corrigé par le registre (+ garde-fou + `/props`).
2. **`get_free_slots` offrait aussi la séquence réservée** aux tâches **filles** (`n_cmpl>1`,
   spéculation) : un KV installé là serait effacé par l'extraction suivante — le même défaut par une
   autre porte, non cité dans l'analyse. Corrigé (`server-context.cpp:3344`).
3. **MA première politique de saturation faisait ROTATION** — trouvé en traçant mon propre code, pas
   dans le cahier des charges. Reprendre la séquence la plus froide **sans regarder si son client
   travaille** donne, avec N clients actifs et N-1 séquences, exactement une rotation : *tous* les
   clients perdent leur séquence à chaque tour et re-préfillent tout leur contexte à chaque étape —
   y compris ceux qui n'avaient rien demandé. C'est pire que de n'épingler personne. Corrigé par
   l'**oracle d'occupation** (§1, point 4) : un client qui génère ne cède jamais sa séquence ; s'il n'y
   en a plus de libre, le client supplémentaire n'est pas épinglé. Le piège symétrique (ne jamais
   reprendre une séquence à une clé inactive ⇒ le registre resterait plein de fils morts et plus
   aucun fil vivant ne serait épinglé) est couvert par le même oracle.
4. **Mon garde-fou masquait le refus d'origine** : placé avant le test `seq_id < 0`, il remplaçait, sur
   un serveur `--parallel < 3`, le message « requires --parallel 3 … current --parallel is too low »
   et son type `ERROR_TYPE_NOT_SUPPORTED` par un message générique `ERROR_TYPE_SERVER` — une régression
   de contrat pour un cas sans rapport avec le P1. Déplacé **après** : le refus d'origine est rendu mot
   pour mot.
5. **`-lv 2` n'écrit pas les lignes INFO** (`common/log.h` : `LOG_LEVEL_INFO = 3`) : la première
   campagne de mesure ne montrait **aucune** ligne `[obrain-p1]` alors que le correctif fonctionnait.
   Un piège de mesure, pas de code — mais il aurait fait conclure « le registre ne journalise rien ».
   Le script de mesure force `-lv 3` et le documente.
6. **Compteurs de cache ambigus** : la ligne `[kv-extract]` est émise **avant** l'insertion du blob,
   donc ses champs `cache_bytes/cache_entrees` décrivaient l'état d'**avant** (la 1ʳᵉ extraction
   annonçait « entrees=0 »). Renommés `cache_avant_*`, et une ligne `[kv-cache] apres ref=…` publiée
   après insertion : c'est elle qui prouve l'éviction.
7. **`Cache(size_t mb)` : un paramètre nommé « Mo » pour une valeur en octets** — piège hérité du
   constructeur `server_prompt_cache(int32_t limit_size_mib, …)` qui, lui, prend bien des MiB.
   `Cache(4096)` = 4096 **octets**, donc un budget qu'on croit de 4 Gio refuse **toute** insertion avec
   un message qui n'accuse pas l'unité. Renommé + commenté + testé sur la variable d'environnement.
8. **Bug d'unité dans mon propre script de mesure** : la comparaison « somme des blobs > budget »
   mélangeait MiB et octets, et classait la campagne bornée en « TEMOIN ». Trouvé en relisant la
   sortie (un run borné qui s'annonce non borné). Le verdict repose désormais sur les compteurs du
   serveur (octets/évictions), pas sur la RSS.
9. **Worktree non propre** : `tools/obrain/pstate-exp0a.cpp` et `tools/server/CMakeLists.txt` portaient
   des modifications non commitées **antérieures** (24/09 et 02/10). Signalé pour que personne ne les
   attribue au P1 ; elles n'ont pas été touchées.

---

## 6. Limites dites

- **Principe des tiroirs** : avec `n_parallel` séquences, **une** est réservée aux tâches J-space, donc
  au plus `n_parallel - 1` clients peuvent être épinglés simultanément. Au-delà, les clients
  supplémentaires retombent sur la sélection automatique du serveur (comportement d'avant `id_slot`) —
  ils ne sont **pas** corrompus, ils perdent leur épinglage, et le serveur le dit en `SRV_WRN` avec le
  remède (`augmenter --parallel`). Aucun correctif serveur ne peut lever cette limite : c'est une
  capacité. `/props` publiant le pool réel, un client conforme ne la rencontre plus.
- **Sans `client_id`**, l'identité du client est son `id_slot` haché : deux cid qui hachètent sur le
  même slot sont **indistinguables** par le serveur et partagent une séquence (comme aujourd'hui). Le
  champ `client_id` est l'API qui permet au chat de lever cela — **changement côté client, hors
  périmètre** (« ne touche qu'au serveur »).
- **Blob plus gros que le budget** : l'insertion est refusée **et** la persistance disque est sautée
  (comportement d'origine, inchangé) : un état dont le blob dépasse le budget n'est pas extractible.
- **Copie à la lecture** : `Cache::get` **copie** le blob (`out = it->second`, un memcpy de la taille
  du blob — 93 Mo ici, 157 Mo sur le 27B) **sous le verrou global** du cache. C'est un coût par lecture
  sur le chemin `state_ref` du chat, pas une corruption ; non modifié (hors périmètre), signalé.
- Les deux branches du garde-fou (séquence réservée **occupée**, préfixe **résiduel**) ne sont **pas**
  atteignables par les chemins normaux après le registre : c'est le but. Elles protègent l'avenir
  (restauration de slot sur la séquence réservée, chemin de sélection qui l'oublierait) et refusent
  explicitement au lieu d'effacer.
- **9911 tourne `--parallel 1`** : `jspace_reserved_slot_id()` y vaut `-1`, donc le P1 y est **inerte**
  (aucune réservation, aucun J-space). Rien n'a été déployé ; le P1 s'active sur les instances
  `--parallel >= 3`.

---

## 7. État du dépôt

Modifié (serveur uniquement) : `tools/server/server-context.cpp`, `tools/server/server-task.h`,
`tools/server/obrain-kv-cache.h`, `tools/server/obrain-kv-cache-tests.cpp`.
Ajouté : `tools/server/obrain-client-seq.h`, `tools/server/obrain-client-seq-tests.cpp`,
`experiments/mesure-p1-clients-seq.py`, `docs/P1-REGISTRE-CLIENT-SEQ-2026-10-05.md`.

**Rien n'a été commité** : le worktree porte des modifications antérieures sans rapport
(`pstate-exp0a.cpp`, `CMakeLists.txt`), et un `git commit -a` les aurait embarquées. Commit ciblé
possible :

```sh
cd ~/projects/llama-obrain-cuda-f1a && git add \
  tools/server/obrain-client-seq.h tools/server/obrain-client-seq-tests.cpp \
  tools/server/server-context.cpp tools/server/server-task.h tools/server/obrain-kv-cache.h \
  tools/server/obrain-kv-cache-tests.cpp experiments/mesure-p1-clients-seq.py \
  docs/P1-REGISTRE-CLIENT-SEQ-2026-10-05.md && git commit -m "P1 : registre client -> sequence (jamais la reservee) + budget LRU du cache d etats verifie et observable"
```

Reproduire la mesure :

```sh
python3 experiments/mesure-p1-clients-seq.py --bin ~/build-p1/bin/llama-server clients      --port 9997
python3 experiments/mesure-p1-clients-seq.py --bin ~/build-p1/bin/llama-server extractions  --budget 250000000 --n 8 --port 9998
```
