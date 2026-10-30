# ENGINE-008 — TRANSMISSION D'IMPLÉMENTATION

**État transmis** : restauration d'état hybride complet par la route serveur,
validée ; substitution SHA256 validée. Arbre :
`/home/triviere/projects/llama-obrain-cuda-f1a`.

---

## 1. Chemin de production et interfaces réellement utilisées

### Route

```
POST /jspace/inject_knowledge
  handler : tools/server/server-context.cpp
            case SERVER_TASK_TYPE_JSPACE_INJECT_KNOWLEDGE
```

| Usage | Corps JSON |
|---|---|
| **A — EXPORT** | `{ "fact_token_ids": [ids...], "extract_only": true, "verify": true }` |
| **B — CONTROL** | `{ "fact_token_ids": [ids...], "prompt": "...", "n_generate": 32, "greedy": true, "verify": true }` |
| **C — RESTORED** | `{ "state_ref": "<ref hex>", "prompt": "...", "n_generate": 32, "greedy": true, "verify": true }` |

`fact_token_ids` et `fact_text` sont **mutuellement exclusifs** (erreur 400).
Borne : 1..4096 IDs, chaque ID vérifié contre `llama_vocab_n_tokens`.
Le chemin textuel historique (`fact_text`) est **conservé et inchangé**.

### Interfaces de transfert — appelées réellement

```cpp
llama_state_seq_get_size_ext(ctx, seq_id, flags)              // taille
llama_state_seq_get_data_ext(ctx, dst, size, seq_id, flags)   // export
llama_state_seq_set_data_ext(ctx, src, size, seq_id, flags)   // import
```

**`flags = LLAMA_STATE_SEQ_FLAGS_NONE` (0)** — seule valeur qui transporte
l'état hybride **complet** : `llama_memory_hybrid::state_write` écrit
`mem_attn` (16 couches d'attention) **puis** `mem_recr` (48 couches SSM).
`LLAMA_STATE_SEQ_FLAGS_PARTIAL_ONLY` (=1) **omet le KV attention** : il est
**refusé** par le pilote et ne doit pas être utilisé pour une reprise.

Avant l'import, la séquence destination est remise à zéro :
`llama_memory_seq_rm(llama_get_memory(ctx), seq_id, -1, -1)`.

---

## 2. Fichiers modifiés et identités complètes

| Fichier | SHA256 |
|---|---|
| `tools/server/obrain-sha256.h` | `5e26e7bb33a353af89d7a98034f3c040a2ae15e376604be1a69603747ec81e2f` |
| `tools/server/obrain-kv-cache.h` | `7d24bd605441be77c1444180848aea318f14aa730486a5592cbc666048aa1873` |
| `tools/server/server-context.cpp` | `70c94958f4b5aa49dc35227ccf69317bf2b5ab479214f4da2f94817afd1dab91` |
| `tools/server/server-task.h` | `2e9c4bcc2464616e68f779eb3efb66ab53cda54775c73ce0d3512e27239e518c` |
| `tools/server/server-task.cpp` | `ba29159b9bf174f310a9112fd3e342c0946f16b30598b47a263daf492008cca1` |
| `tools/server/CMakeLists.txt` | `aeea20bb2b8aa59184efa38b3af6a50ec4b76fa7fd2d555162418c0c2c1e5953` |

**Artefacts construits**

| Artefact | SHA256 |
|---|---|
| `build-cuda/bin/llama-server` | `690bc45a54528bda9313af578e3ff5a15fa9fabc2867168580373c20d03815e3` |
| `build-cuda/bin/libllama-server-impl.so` | `1d41dee2ffab28027f30f16b2480523b61f80827f413a635b468a6e18c9d65ce` |
| **`build-cuda/bin/libllama.so.0` (Q1)** | `87c42d5f5dd7ab3c10967b4d3eaac82636b6af11e2c18a7f2798bab2d01bb2b2` |
| `libcrypto.so.3` (dépendance ajoutée) | `6facd87aa596f6d10c2029802756c56cef8761576a3e8894c42a7295a5a89d75` |

**Q1 n'a JAMAIS été reconstruit** : `libllama.so.0` est identique avant et après
chaque build. Seuls `server-context` et `libllama-server-impl.so` changent.

**Dépendance ajoutée** : OpenSSL 3.0.13, résolue en
`/lib/aarch64-linux-gnu/libcrypto.so.3` — **vérifié présent dans
`ldd libllama-server-impl.so`**.

---

## 3. Commandes reproductibles

### Build

```bash
cd /home/triviere/projects/llama-obrain-cuda-f1a
cmake -S . -B build-cuda
cmake --build build-cuda --target llama-server -j 8
sha256sum build-cuda/bin/libllama.so.0     # DOIT rester 87c42d5f...
```

### Tests (tous sans modèle)

```bash
cd /home/triviere/projects/llama-obrain-cuda-f1a
INC="-std=c++17 -O2 -w -I tools/server"

g++ $INC tools/server/obrain-kv-cache-tests.cpp              -o /tmp/t_cache
g++ $INC -pthread tools/server/obrain-import-deadlock-tests.cpp -o /tmp/t_dead
g++ $INC -pthread tools/server/obrain-sha256-concurrency-tests.cpp -o /tmp/t_conc
g++ $INC -DOBRAIN_USE_OPENSSL_SHA256 tools/server/obrain-sha256-tests.cpp -o /tmp/t_sha -lcrypto
g++ $INC -DOBRAIN_USE_OPENSSL_SHA256 tools/server/obrain-sha256-substitution-tests.cpp -o /tmp/t_sub -lcrypto

/tmp/t_cache   # 49 PASSED / 0 FAILED
/tmp/t_dead    #  5 PASSED / 0 FAILED
/tmp/t_conc    # PASS, aucun sous-processus
/tmp/t_sha     # 11 PASSED / 0 FAILED
/tmp/t_sub     # 22 PASSED / 0 FAILED
```

Toutes les suites **incluent `obrain-kv-cache.h` / `obrain-sha256.h`, les mêmes
en-têtes que le serveur** : elles exercent le code de production, pas une copie.

### Lancement serveur (configuration validée)

```bash
export OBRAIN_KV_STATES_MAX_BYTES=2147483648
build-cuda/bin/llama-server \
  --model /home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf \
  --host 127.0.0.1 --port <PORT> \
  --ctx-size 1024 --n-gpu-layers 0 --parallel 3 --jinja
```

Attendre `GET /health` → `{"status":"ok"}` **en vérifiant que le PID est
vivant** ; le démarrage mesuré est de **27 s**.

---

## 4. Contrat du manifeste, de compatibilité et d'intégrité

`struct kv_state_manifest` (`obrain-kv-cache.h`) porte :

```
model_desc, model_sha256, model_bytes, model_params,
n_vocab, n_layer, n_embd,
q1_sha256,                       // identité de la libllama RÉELLEMENT chargée
format_magic (0xaf143cd9),       // format du blob
blob_bytes, blob_sha256,         // intégrité cryptographique
n_tokens,
type_k, type_v,                  // types KV effectifs
n_ctx, n_batch, n_ubatch, n_seq_max,
state_mode,                      // OBRAIN_STATE_TYPE effectif
seq_id,
prefix_ids,                      // PREFIXE EXACT
pos_min, pos_max
```

**Règles de refus**, appliquées **AVANT `set_data`** :

1. `complete()` faux → `REFUSE_NO_MANIFEST` / `REFUSE_INCOMPATIBLE`. **Toute
   donnée nécessaire inconnue refuse l'import** — jamais « compatible par
   défaut ».
2. `model_sha256` différent → refusé **en premier**. Description, taille,
   paramètres et topologie sont des conditions **supplémentaires** : elles ne
   remplacent pas l'identité exacte (testé : deux modèles partageant description
   **et** topologie sont refusés).
3. `q1_sha256`, types KV, `state_mode`, configuration de contexte, `seq_id`,
   `format_magic` → chacun refusé s'il diffère.
4. `blob_bytes` incohérent → `REFUSE_BAD_SIZE`.
5. **SHA256 des octets** recalculé et comparé → `REFUSE_BAD_SHA256`.

**L'identité du modèle est établie UNE FOIS, AU CHARGEMENT**, et **liée à
l'instance** : `kv_model_instance_new()` invalide l'empreinte précédente, et
`kv_model_sha256(path, token)` ne réutilise le cache que si le **jeton
d'instance** ET le chemin correspondent. **Une nouvelle instance de modèle ne
réutilise jamais aveuglément l'identité précédente.** Le hachage est effectué
**hors du verrou** de l'identité.

**Un hash de contenu ne remplace pas un contrôle de compatibilité** : la
référence FNV-1a 64 bits sert uniquement d'**index** ; SHA256 sert
d'**intégrité** ; le manifeste sert de **compatibilité**. Les trois rôles sont
distincts et aucun n'est substitué à un autre.

---

## 5. Borne mémoire, éviction et persistance

```
OBRAIN_KV_STATES_MAX_BYTES   défaut 2 x 1024^3 = 2 147 483 648 OCTETS
```

- **Borne explicite**, compteur d'octets tenu à jour, **éviction LRU**
  (`g_kv_states_lru` + index de position).
- **Refus d'une entrée plus grosse que la borne** : aucune entrée n'est créée,
  un message d'erreur explicite est renvoyé (`state too large for the bounded
  cache (not stored)`).
- **Le remplacement d'une clé existante retire d'abord sa propre taille**, puis
  évince si nécessaire — sans quoi un simple remplacement évincait une entrée
  **non liée** (défaut trouvé par les tests, corrigé).
- **Persistance disque** (`save_state_file` / `load_state_file`) : magic `OBKV`
  + version + manifeste complet (**dont le préfixe exact**) + taille + octets.
  Un fichier tronqué, sans manifeste, ou dont l'association blob/manifeste est
  discordante est **refusé**. **Aucun manifeste rétroactif n'est fabriqué** : un
  ancien blob sans manifeste complet reste refusé.
- **Isolation** : la clé reste la référence de contenu et `seq_id` fait partie
  du manifeste comparé ; aucun partage implicite entre sessions.

---

## 6. Règle de verrouillage corrigée et test de régression

**CONTRAT** : `kv_states_insert` et `kv_states_get` (`server-context.cpp`)
**possèdent** `g_kv_cache.mtx`. **L'appelant ne doit PAS le reprendre.**

```cpp
// ligne ~72 : c'est LE MEME mutex, par référence
static std::mutex & g_kv_states_mtx = g_kv_cache.mtx;

// les wrappers prennent le verrou
static bool kv_states_get(const std::string & ref, kv_state_blob & out) {
    std::lock_guard<std::mutex> lk(g_kv_cache.mtx);
    return g_kv_cache.get(ref, out);
}
// MAUVAIS (ancien code) : verrou externe + wrapper -> auto-interblocage
{ std::lock_guard<std::mutex> lk(g_kv_states_mtx); kv_states_get(ref, blob); }
// CORRECT
kv_states_get(ref, blob);
```

**`std::mutex` n'est pas récursif** : reprendre le même mutex sur le même
thread bloque ce thread sur lui-même (`futex_do_wait`, tous threads en `S`).
Symptôme observé : la requête C n'obtenait **aucune réponse** et expirait à
240 s, alors que A et B réussissaient.

**TEST DE RÉGRESSION** `obrain-import-deadlock-tests.cpp` (5 assertions) : il
exerce les **deux motifs** avec une **surveillance bornée** — l'ancien motif
**doit** bloquer (`« NON -> detecte »`), le corrigé doit terminer (16 threads ×
300 insert/get). Il utilise un **cache dédié** pour l'ancien motif, car le
thread abandonné garde le verrou.

**Règle générale à respecter** : ne jamais tenir un verrou du cache pendant une
opération longue (hachage, import) ; les hachages s'exécutent **hors verrou**.

---

## 7. Conventions IDs, EOS, positions et métriques

### IDs

- Les IDs proviennent de **`llama_sampler_sample`**, le sampler de
  **production**. **Aucun ID n'est reconstruit depuis le texte.**
- `selected_ids_count` et `emitted_ids_count` sont distincts et doivent être
  égaux en fonctionnement normal.
- `fact_token_ids` reçus **tels quels**, sans tokenisation : `fact_tokens=58`
  exactement, aucun template ajouté.

### EOS — deux notions distinctes

- **EOS SÉLECTIONNÉ** : le sampler désigne `<|im_end|>` = **248046** comme
  prochain token. La boucle teste `if (tok == eos) break;` **avant** l'émission
  → **cet ID n'est NI émis NI enregistré**.
- **EOS ÉMIS** : **aucun** dans les conditions validées.
- `stop_reason="EOS"` signale la **sélection**. `decoded_ids_json` ne contient
  que les IDs **émis**.
- `n_generate_requested` est le **PLAFOND**, jamais le nombre produit : une
  sortie de 6 tokens terminée par EOS est normale.

### Positions

- La position enregistrée est **`cur_pos` seul** — exactement la position du
  batch transmis à `llama_decode` (`gen_batch.pos[0] = cur_pos`).
- **Ne jamais calculer `cur_pos + t`** : `cur_pos` avance déjà, la formule
  produisait un **pas de 2**.
- Mesuré : préfixe 0..57, 59e ID décodé en **58**, continuations en
  **59..64** (pas de 1).

### Métriques — convention explicite

```
>= 0  : mesure réelle
 -1   : NON APPLICABLE sur ce chemin
 -2   : NON MESURÉ
```

Champs : `ms_prefix_compute`, `ms_state_lookup`, `ms_model_sha256`,
`ms_blob_sha256`, `ms_compatibility`, `ms_state_import`,
`ms_preparation_total`.

> **NON-ADDITIVITÉ** : ces phases sont **imbriquées, pas disjointes**. Mesuré :
> `49.750 + 0.012 + 64.505 + 6.891 = 121.158 ms` contre
> `ms_preparation_total = 143.503 ms` — écart **22.345 ms** couvert par aucune
> des quatre. **Ne pas les additionner.**

`fact_decode_ms` est **conservé pour compatibilité** mais son nom est trompeur :
sur le chemin texte il couvre le décodage du fait, sur `state_ref` il couvre
l'import. Utiliser les champs séparés. Sur `extract_only`, les métriques sont
désormais finalisées **avant le retour** (auparavant elles restaient à `0.0`).

---

## 8. Résultat validé

**Trois paires CONTROL/RESTORED**, ordre préenregistré **B/C, C/B, B/C**,
blob de **160 697 024 octets** (ref `046f5945a8dc6fc9`). Les 7 requêtes HTTP
ont été acceptées.

| Paire | CONTROL (ms) | RESTORED (ms) | Ratio |
|---|---|---|---|
| 1 (B/C) | 874.322 | 144.557 | 6.0483 |
| 2 (C/B) | 824.855 | 143.503 | 5.7480 |
| 3 (B/C) | 804.153 | 143.289 | 5.6121 |
| **médiane** | **824.855** | **143.503** | **5.7480** |

```
DESTINATION_PREFIX_TOKEN_COUNT = 0        (prefix_decode_calls=0, state_imported=true)
CONTROL                        = 58       (prefix_token_count=58, state_imported=false)
START_LOGITS_BINARY_IDENTICAL  = YES 3/3  (MAX_ABS_DIFF=0, L2=0, ARGMAX=13/13)
TOKEN_IDS_MATCH                = YES      IDs [13,198,248069,271,3793,13]
TEXT                           = '.\n</think>\n\nOK.'  (identique)
```

**Zéro préfixe recalculé en destination**, logits **binairement identiques 3/3**,
**ratio des médianes 5.748×**.

**Substitution SHA256** — comparaison contrôlée, ordre alterné préenregistré,
mêmes empreintes :

```
OLD_BLOB_SHA256_MEDIAN_MS = 798.6      MODEL: ref 64014.1 -> new 5945.1 (x10.77)
NEW_BLOB_SHA256_MEDIAN_MS =  70.9      serveur au chargement : 5978.6 ms
GAIN_MEDIAN = x11.27                   empreintes identiques et == 7a40fc6b...
```

**Aucun contrôle affaibli** : `INTEGRITY_CHECK_REMOVED=NO`.

---

## 9. Limites

- **Périmètre testé** : modèle `Qwen3.8-27B-OBRAIN-v3` (`1d7c9409…`), entrée
  **59 IDs autoritaires**, `--n-gpu-layers 0` (**calcul CPU**), `n_ctx=1024`
  (arrondi serveur 1536, `n_ctx_slot=512`), `--parallel 3`, `--jinja`,
  `flags=0`. **Rien n'est généralisé** hors de ce périmètre — notamment pas à
  d'autres modèles, d'autres préfixes, ni à des `n_ctx` différents.
- **Le préfixe source est calculé une fois** : le blob doit être produit. C'est
  un **transfert d'état après un prefill unique**, **pas** une élimination du
  prefill. Le coût de production du blob n'est pas absorbé.
- **Aucune validation de charge concurrente** : les requêtes ont été émises
  **séquentiellement**, une à la fois. Le comportement sous concurrence réelle
  (plusieurs requêtes simultanées, éviction sous charge, contention) **n'a pas
  été testé**.
- **Aucune conclusion de qualité générale** : aucune évaluation qualité n'a été
  lancée.
- **Aucun gain réseau ni de bout en bout** : le blob est resté **en mémoire** ;
  le coût disque/réseau **n'a pas été mesuré**.
- **Le gain SHA256 est un coût de hachage**, pas une mesure de bande passante
  physique.
- **GPU non validé** : toutes les mesures ont été faites avec
  `--n-gpu-layers 0`. Le chemin de reprise **n'a pas été exercé avec des
  couches sur GPU**.

---

## 10. Après DEADLOCK-11 : démarrer le serveur corrigé à la place du port 9910

**Réponse : NON en l'état, et je ne le fais pas maintenant.** Motifs vérifiés :

1. **Rien n'écoute sur 9910** (vérifié : `ss -tlnp` ne montre ni 9910 ni
   voisins). Il n'y a donc **aucune configuration de référence à remplacer**, et
   je ne connais ni le modèle, ni les paramètres, ni le service qui l'occupait.
2. Les serveurs actifs tournent depuis **d'autres arbres** :
   `llama-obrain/build/bin/llama-server` (`0385e939…`) pour 9906 et 9908, et
   `llama-obrain-fix/build-fix/bin/llama-server` pour 9907. **Le binaire corrigé
   est `llama-obrain-cuda-f1a/build-cuda/bin/llama-server` (`690bc45a…`)** — un
   arbre différent, avec une dépendance nouvelle (`libcrypto.so.3`) et une
   sémantique nouvelle (manifeste obligatoire à l'import).
3. **La configuration validée est CPU** (`--n-gpu-layers 0`) et bornée à un
   modèle et un `n_ctx`. **Je n'ai pas validé le chemin de reprise sur GPU**, ni
   sous concurrence.
4. **Conséquence du manifeste** : le chemin disque **refuse** désormais tout
   blob antérieur sans manifeste complet. Des états persistés existent
   (`/home/triviere/kv-states/*.bin`) : **ils ne seraient pas rechargeables**.
5. **L'instruction de cette mission interdit tout nouveau forward.** Démarrer un
   serveur de production en est un.

**Ce qu'il faudrait avant de basculer**, dans l'ordre :

1. Établir la configuration de référence du port 9910 (modèle, `n_ctx`,
   `n-gpu-layers`, `--parallel`, service de démarrage) — **information
   manquante**.
2. Relancer `cmake --build build-cuda --target llama-server` dans l'arbre de
   production retenu, et **vérifier que `libllama.so.0` y reste inchangé**.
3. **Valider le chemin de reprise avec la configuration cible** (GPU et
   `--parallel` réels), puis **sous concurrence**, avant toute exposition.
4. Décider du sort des états disque existants (re-génération avec manifeste).

Le lancement lui-même est une opération courte et réversible **une fois 1 à 3
faits** ; il ne doit pas être présenté comme acquis aujourd'hui.

```
FUNCTIONAL_SERVER_RESUME_VALIDATED=YES
VALIDATED_SCOPE=Qwen3.8-27B-OBRAIN-v3 (sha256 1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937),
  59 IDs autoritaires (ids59.txt sha256 fc39bb86b0a942c2855d4cd90016f87bf655d4463731631658324e897021dbab),
  flags=0 (LLAMA_STATE_SEQ_FLAGS_NONE), --ctx-size 1024, --n-gpu-layers 0, --parallel 3, --jinja,
  OBRAIN_KV_STATES_MAX_BYTES=2147483648, Q1 libllama.so.0 = 87c42d5f...
NEXT_EXPERIMENT_AUTHORIZED=NO
REMAINING_WORK_FOR_THIS_SCOPE=NONE
```
