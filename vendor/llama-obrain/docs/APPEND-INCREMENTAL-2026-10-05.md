# Import incrémental d'état KV — P0 « 1M jetons par client » (2026-10-05)

Worktree : `~/projects/llama-obrain-cuda-f1a` — branche **`feat/kv-splice-chat`** (base `293f02e`).
Build de vérification : `~/build-append-cpu` (CPU, **jamais** `build-cuda` : c'est le binaire du
service 9910, qui tournait pendant tout le chantier — `build-cuda/bin/llama-server`, mtime 14:52,
non touché ; 9910 et 9911 sont restés à HTTP 200).

## 1. Le verrou qu'on lève

`llama_kv_cache::state_read_meta` (`src/llama-kv-cache.cpp:2398`) commençait par
`seq_rm(dest_seq_id, -1, -1)` **avant** de lire la moindre position : tout import d'état était un
**REMPLACEMENT TOTAL**. Idem `llama_memory_recurrent::state_read_meta`. Recharger un contexte de
1M jetons (65 556 o/jeton de KV en f16 + 156 894 360 o d'état récurrent ⇒ ~65,7 Go/client) par
pages obligeait donc à recopier tout le préfixe à chaque page ajoutée — alors que seule la dernière
page change.

## 2. Ce qui a été implémenté

### 2.1 Moteur — `include/llama.h`, `src/llama-kv-cache.*`, `src/llama-memory-recurrent.*`, `src/llama-memory*.h`

- **`LLAMA_STATE_SEQ_FLAGS_APPEND` (valeur 4)** : nouveau drapeau. `state_read` le lit désormais
  (il était `GGML_UNUSED`).
- **KV** : en mode APPEND, `seq_rm` est **sauté** ; les positions de la page sont lues d'abord, puis
  **vérifiées** :
  - contiguïté **interne** (`pos == pos_min + i`) — une page trouée est refusée, jamais « posée au
    plus près » ;
  - contiguïté **avec l'existant** (`pos_min == seq_pos_max(dest) + 1`, ou `0` si la séquence est
    vide) ;
  puis `find_slot` + `apply_ubatch` positionnent la page. Les cellules des pages 0..k-1 ne sont ni
  lues ni écrites.
  - **Refus explicite sur cache SWA** (`n_swa > 0 && swa_type != NONE`) : un cache à fenêtre
    glissante recycle ses cellules (`find_slot` puis purge dans `apply_ubatch`), l'invariant
    « toutes les positions [pos_min, pos_max] sont présentes » n'y existe pas — promettre une
    préservation qu'on ne peut pas tenir serait un mensonge.
- **Récurrent** (couches à attention linéaire) : l'état est un **pli** (résumé) de toutes les
  positions vues, il n'y a rien à ajouter cellule par cellule — l'état entrant remplace celui de la
  séquence, et c'est correct parce qu'il contient déjà le pli des pages précédentes. `seq_rm` est
  sauté, et la **progression est exigée** (position entrante strictement supérieure au pli courant).
- **Extraction bornée par positions** : `llama_state_seq_get_size_ext_range` /
  `llama_state_seq_get_data_ext_range` (+ `llama_memory_i::state_write_range`, implémentation par
  défaut = comportement historique). Sans cette borne, extraire une page rendrait **tout** l'état
  accumulé de la séquence : impossible de fabriquer un delta.
- **Correctif** : sur un import refusé **en mode APPEND**, la séquence n'est **plus purgée**
  (`state_read` KV et récurrent). Le code d'origine purgeait à chaque échec — en mode ajout, un
  refus (page non contiguë, cellules insuffisantes) effaçait exactement les pages qu'on voulait
  préserver. C'est testé (`SURVIE : les octets de la séquence cible sont INTACTS après le refus`).

### 2.2 Route — `tools/server/server-context.cpp`, `server-task.*`, `obrain-kv-cache.h`

`POST /jspace/inject_knowledge` accepte désormais :

| champ | sens |
|---|---|
| `state_append: true` | **AJOUTE** la page (drapeau APPEND) au lieu de remplacer. Ni question, ni génération : la séquence réservée devient le contexte assemblé, page après page. |
| `pos_min`, `pos_max` | plage de la page, **exigée** en mode ajout : comparée au manifeste du blob ET à la séquence cible **avant** tout appel au moteur (HTTP 400 lisible au lieu d'un refus profond). |
| `state_reset: true` | remise à zéro **explicite** de la séquence avant l'ajout (la séquence J-space est partagée : une extraction y laisse le préfixe qu'elle décode). |
| `pos_start: N` | extraction : le préfixe est décodé en `[N, N+n)`, donc l'état extrait est la **page** `[N, N+n)`. |
| `prerequis_refs: [refs]` | extraction : les pages sont **ajoutées** à une séquence propre avant de décoder la nouvelle page, pour que le pli récurrent couvre bien `[0, pos_max]`. Avec des prérequis (et sans `fact_text`), la route pose la question **sur le contexte assemblé** (mode K4). |

Réponse JSON enrichie : `state_append`, `page_pos_min/max`, `seq_pos_min/max`, `attn_pos_min/max`,
`seq_cells`, **`import_bytes`** (les octets réellement importés = la page, jamais le préfixe),
`ms_state_import`, `recr_from`.

Garde-fou **`recr_from`** (manifeste) : une page dont la partie récurrente ne couvre pas depuis 0,
ajoutée à une séquence commençant à 0, produirait un état récurrent **qui ignore les pages déjà en
place** — réponses fausses, sans erreur, sans log. La route **refuse** et dit comment corriger
(`prerequis_refs`). `recr_from` est écrit **par construction** à l'extraction (0 avec prérequis,
`pos_start` sinon), et vaut 0 par défaut pour les manifestes antérieurs — ce qui est exact, puisque
l'ancienne extraction décodait toujours à partir de 0.

## 3. Mesure bout en bout (2026-10-05 22:14, moteur temporaire port 9999, hors 9910/9911)

`python3 experiments/mesure-append-e2e.py 9999 2048 512` — Qwen3-1.7B-Q8_0 (KV f16), `--ctx-size
8192 --parallel 3`, 3 répétitions.

```
[extract] page0   ref=6ebcc175f742f791 page=[0, 1971]    recr_from=0  1972 jetons  226 189 104 o
[extract] page1   ref=1e9f3c47d54b481b page=[1972, 2455] recr_from=0   484 jetons   55 515 504 o
[extract] complet ref=cfc6b838232b6ff5 page=[0, 2455]    recr_from=0  2456 jetons  281 703 904 o

AJOUT    page0 : 226 189 104 o en 8,5 / 8,5 / 8,5 ms   -> seq=[0, 1971]
AJOUT    page1 :  55 515 504 o en 1,9 / 1,9 / 1,9 ms   -> seq=[0, 2455]   <-- le DELTA
BASELINE complet : 281 703 904 o en 10,5 / 10,6 / 10,5 ms (remplacement total du même contexte)

GAIN octets : x5,07   (268,7 Mo -> 52,9 Mo)
GAIN temps  : x5,63   (10,5 ms  ->  1,9 ms)

QUESTION (réponse dans la PAGE 0, après ajout de la page 1)
  contexte ASSEMBLE (2 pages) : « Le code secret est ROUGE-7. »   <-- page 0 INTACTE
  contexte COMPLET (remplace) : « Le code secret est ROUGE-7. »
```

Lecture : `import_bytes` de l'ajout de page 1 vaut **exactement** les octets de la page 1
(55 515 504 o = 484 × 114 700 + en-têtes) — les 1972 jetons déjà en place **n'ont pas été
recopiés**, et la réponse reste juste. La répétition 3/3 (1,9 ms vs 10,5 ms) écarte le bruit.

Note : 114 700 o/jeton **pour ce modèle** (Qwen3-1.7B, 28 couches, KV f16). Les 65 556 o/jeton du
design concernent le 27B de production — le rapport entre les deux mesures est ce qui compte ici.

## 4. Tests

| test | résultat |
|---|---|
| `tests/test-state-append.cpp` (**nouveau**, enregistré dans `tests/CMakeLists.txt`) | **SUCCES (0 échec)** — ajout contigu, non-recopie octet à octet, refus non contigu + survie, refus page trouée, équivalence d'argmax avec un prefill complet |
| `tools/server/obrain-kv-cache-tests.cpp` | **74/74 PASS** (dont 8 nouveaux cas multi-flux) |
| `test-save-load-state` | PASS (chemin REMPLACEMENT historique intact) |
| `test-state-restore-fragmented` | SUCCESS |
| `cargo test` (chat `wt-chat-kv-splice`) | 197 passés, 0 échec |
| build CPU complet (`llama-server` + tests) | EXIT=0 |

## 5. Bugs trouvés et corrigés pendant la vérification

1. **Purge de la séquence sur un import refusé** (moteur, KV + récurrent) — en mode ajout, un refus
   effaçait les pages 0..k-1. Corrigé, testé.
2. **`obrain-kv-cache.h :: marche` ne gérait pas les flux VIDES** : l'écrivain, pour un flux vide,
   n'écrit que son nombre de cellules puis passe au suivant ; le lecteur attendait une section de
   données après *chaque* flux, se désalignait, et **refusait un blob valide** en annonçant
   « topologie d'attention invraisemblable ». Invisible sur un serveur mono-flux (`--parallel 1`),
   systématique en multi-flux. Trouvé en mesurant sur `--parallel 3` (HTTP 500 sur un modèle sain).
   Corrigé + 8 cas de test ajoutés.
3. **Assemblage par prérequis sans purge** (route, ma propre erreur) : « ne pas purger entre les
   ajouts » avait été confondu avec « ne pas purger du tout » ⇒ le premier prérequis était refusé
   (« la page commence à 0 alors que la séquence attend 232 »). Corrigé : purge **puis** ajouts.
4. **`SRV_INF` sans argument** : `SRV_INF("texte\n")` ne compile pas dans ce fork (le macro exige au
   moins un argument après le format) — corrigé en `SRV_INF("%s", "...")`.
5. **Mode question sur contexte assemblé impossible** : la garde « `fact_text` doit tokeniser au
   moins 1 jeton » rendait toute question sur un contexte assemblé impossible — c'est-à-dire l'usage
   même de la fonction. Corrigé (`(or pass 'prerequis_refs')`).

## 6. Limites dites

- **SWA** : l'ajout incrémental est refusé (voir 2.1), par construction.
- **Modèles hybrides** : une page doit avoir été extraite d'une session portant déjà les pages
  précédentes (`prerequis_refs`), sinon la route refuse. C'est l'invariant du design par pages, mais
  il doit être tenu par l'appelant.
- **Modèles purement récurrents** : seule la progression du pli est vérifiable dans le blob (la
  contiguïté exacte vient de la partie attention, absente dans ce cas).
- La route **chat** garde la sémantique de remplacement ; le P0 expose l'ajout sur la route J-space.
