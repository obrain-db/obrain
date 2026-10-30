# ENGINE-008 — REPRISE SANS PREFILL : MÉTHODE NATIVE

**Phase de PRÉPARATION.** Aucun forward, aucune autorisation de calcul, aucune
modification de Q1. Le dossier `ENGINE008-KV-INJECTION-CAUSAL-02` est clos et
n'est pas réexécuté : sa conclusion (l'injection KV native écrit, l'effet
causal est observé, la restauration est exacte) et ses limites (aucun transfert
des 48 états récurrents, aucune élimination du prefill) sont conservées.

---

## 1. L'interface existe — et elle transporte l'état hybride COMPLET

Le point décisif est dans la composition hybride, lue verbatim
(`src/llama-memory-hybrid.cpp`) :

```cpp
void llama_memory_hybrid::state_write(llama_io_write_i & io, llama_seq_id seq_id, llama_state_seq_flags flags) const {
    if ((flags & LLAMA_STATE_SEQ_FLAGS_PARTIAL_ONLY) == 0) {
        mem_attn->state_write(io, seq_id, flags);   // KV des couches d'ATTENTION
    }
    mem_recr->state_write(io, seq_id, flags);       // etat RECURRENT (SSM), TOUJOURS
}
```

| `flags` | Ce qui est sérialisé |
|---|---|
| **`0`** (`LLAMA_STATE_SEQ_FLAGS_NONE`) | **KV attention (16 couches) + état récurrent (48 couches SSM)** |
| **`1`** (`LLAMA_STATE_SEQ_FLAGS_PARTIAL_ONLY`) | **état récurrent SEUL** — le KV attention est **omis** |

`state_read` suit exactement la même logique.

**Conséquence : `flags = 0` est la seule valeur qui transfère l'état hybride
complet en un appel.** `PARTIAL_ONLY` est **insuffisant** pour une reprise, et le
pilote le **rejette explicitement** plutôt que de le laisser passer.

## 2. Les interfaces, avec leurs références exactes

```
LLAMA_API size_t llama_state_seq_get_size_ext(ctx, seq_id, flags);          include/llama.h:1011
LLAMA_API size_t llama_state_seq_get_data_ext(ctx, dst, size, seq_id, flags); include/llama.h:1016
LLAMA_API size_t llama_state_seq_set_data_ext(ctx, src, size, seq_id, flags); include/llama.h:1023
LLAMA_API size_t llama_state_seq_save_file(ctx, path, seq_id, tokens, n);     include/llama.h
LLAMA_API size_t llama_state_seq_load_file(ctx, path, seq_id, ...);           include/llama.h:989
```
Aucune variante `_attn` / `_recr` publique n'existe pour l'état : les deux sont
transportés par le **même** appel. Les accès séparés existent en revanche pour
les **positions** : `llama_memory_seq_pos_min_attn` / `_max_attn`
(`src/llama-context.cpp:4440`, `:4453`) — utiles pour lire la fenêtre
d'attention sur un hybride, où `seq_pos_min/max` se réduit à
`[last, last]` (commentaire `include/llama.h:839-842`).

**API interne** (atteignable en compilant dans l'arbre du fork) :
`llama_memory_hybrid::get_mem_attn()` / `get_mem_recr()`
(`src/llama-memory-hybrid.h:83-84`), `llama_memory_recurrent::state_write/read`,
`state_write_meta/data`, `state_read_meta/data`
(`src/llama-memory-recurrent.h:73-74`, `:136-140`).

## 3. Le point de transfert

| Étape | Position | Contenu |
|---|---|---|
| 1. Source : prefill des **58 premiers IDs** | `0..57` | établit KV attention **et** état récurrent |
| 2. Export | — | `get_size_ext(ctx_src, 0, flags=0)` puis `get_data_ext(...)` |
| 3. Import dans un contexte **neuf** | `seq_id = 0` | `set_data_ext(ctx_dst, buf, n, 0, flags=0)` |
| 4. Décodage du **59e ID** | **`58`** | dans la destination : produit les **premiers logits de continuation** |
| 5. Suite | `59, 60, …` | génération normale |

Le point 4 est ce qui distingue cette méthode du simple transfert de cache : la
destination n'a **jamais vu** les 58 premiers tokens, et c'est le décodage du
59e qui matérialise les logits.

**Métadonnées nécessaires** : le blob porte `io_magic` **et** le `seq_id`
d'origine. La destination doit être lue avec le `seq_id` **destination** en
argument de `set_data` ; le `seq_id` lu dans l'en-tête sert à retrouver le
`mem_storage` en mode `ON_DEVICE`. Les **positions** des cellules sont dans les
métadonnées de l'état récurrent et dans les cellules du KV : elles sont
restaurées par le blob, pas par un argument.

## 4. Format du blob, tel que le code l'écrit

**En-tête** (`llama_context::state_seq_get_data`, `src/llama-context.cpp:3448`) :
```
io_magic   uint32 = 0xaf143cd9    // constexpr, src/llama-context.cpp:3428
seq_id     llama_seq_id
```
Puis `memory->state_write(io, seq_id, flags)`.

**Section récurrente** (`llama_memory_recurrent::state_write_data`) :
```
s_trans     uint32
n_layer     uint32 = hparams.n_layer()      // nombre de couches recurrentes
k4          uint32 = obrain_state_mode()    // 0=f32, 1=f16, 2=i8
--- tenseurs R, une entree par couche il (les couches nulles sont SAUTEES) :
  r_type_i    int32
  r_size_row  uint64 = ggml_row_size(type, hparams.n_embd_r())
  par plage de cellules : range_size x n_embd_r elements au type du tenseur
--- tenseurs S, une entree par couche :
  s_type_i    int32
  s_size_el   uint32 = ggml_type_size(s_l[il]->type)
  n_embd_s    uint32
  pour chaque ligne j < n_embd_s, par plage de cellules
```

**`OBRAIN_STATE_TYPE=f16|i8`** (alias `OBRAIN_STATE_F16=1` → f16) change le type
du blob récurrent — `obrain_state_mode()` (`src/llama-memory-recurrent.cpp:865`),
valeurs `0` f32 / `1` f16 / `2` i8. **Ce levier est qualifié `[obrain K4]` dans
le code**, et le magic a été **incrémenté en v2** précisément pour ce changement.

## 5. Tailles — formule établie, **mesure à faire à l'exécution**

La taille **ne peut pas** être calculée de façon fiable hors exécution, parce
qu'elle dépend du **nombre de cellules occupées** (`cell_ranges`) et du type
effectif. Elle **doit** être obtenue par `get_size_ext`, puis le nombre d'octets
réellement exportés par le retour de `get_data_ext` :

```
SIZE_ATTENDU = llama_state_seq_get_size_ext(ctx, 0, 0)
N_ECRIT      = llama_state_seq_get_data_ext(ctx, buf, SIZE_ATTENDU, 0, 0)
CONDITION    : N_ECRIT == SIZE_ATTENDU, et N_ECRIT > 8 (l'en-tete seul ne suffit pas)
```

Formule structurelle, pour dimensionner et pour **détecter une incohérence** :
```
n_recr_layers x (4 + 8)                        // en-tetes de couches R
+ N_CELLS x n_embd_r x el_size(r_type)         // tenseurs R
+ n_recr_layers x (4 + 4 + 4)                  // en-tetes de couches S
+ N_CELLS x n_embd_s x el_size(s_type)         // tenseurs S, ecrits par ligne
+ 4 (s_trans) + 4 (n_layer) + 4 (k4) + 8 (magic + seq_id)
```

**Copie complète ou restauration partielle — la distinction :**
- **copie complète** : `flags = 0`, et `N_ECRIT == SIZE_ATTENDU` ;
- **restauration partielle** : `flags = PARTIAL_ONLY` → **le KV attention n'est
  pas transporté**, la destination ne peut pas poursuivre une continuation
  dépendant de l'attention au-delà du préfixe récurrent. **Le pilote refuse ce
  mode.**

## 6. Chemins d'erreur — **la restauration ratée est DESTRUCTIVE**

Lus dans le code, pas déduits :

| Situation | Comportement |
|---|---|
| export : exception | log `error saving state` → **retour 0** |
| import : magic incorrect | `throw "wrong sequence state magic"` → catch → **retour 0** |
| import : `state_read_meta` ou `state_read_data` faux | voir ci-dessous |
| **import : échec de restauration** | **`clear(true)`** si `seq_id == -1`, sinon **`seq_rm(seq_id, -1, -1)`** — **puis** `throw "failed to restore kv cache"` |

```cpp
res = res && state_read_meta(io, cell_count, seq_id);
res = res && state_read_data(io, cell_count);
if (!res) {
    if (seq_id == -1) clear(true); else seq_rm(seq_id, -1, -1);
    throw std::runtime_error("failed to restore kv cache");
}
```

**Conséquences opérationnelles, à porter au contrat :**
1. **Pas d'état résiduel partiel** : en cas d'échec, la séquence est **effacée**,
   pas laissée à moitié écrite. C'est plus sûr qu'une écriture partielle.
2. **Mais l'échec DÉTRUIT l'état destination.** Tenter une restauration sur un
   contexte dont l'état compte le perd. Le pilote doit donc : restaurer dans un
   contexte **neuf**, et ne considérer la restauration acquise que sur un retour
   **égal à la taille attendue**.
3. **Un retour 0 est un échec, jamais un succès vide.** Le code retourne 0
   uniquement par les chemins d'exception.

## 7. Le pilote et ses tests — exécutés, SANS MODÈLE

`e8-resume.cpp` — identité dans `PILOT_IDENTITY.txt`. Compilé contre les
en-têtes du fork (nécessaire pour `llama-context.h` / `llama-model.h`), **sans
aucune modification de Q1**.

**À déclarer, et non à masquer** : le binaire construit pour `--selftest`
**ne référence AUCUN symbole `llama`** (vérifié par `nm -u`) et ne déclare que
`libc` en `NEEDED`. L'éditeur de liens, avec `--as-needed`, a donc **écarté
`libllama` et `libggml`** — parce que la phase de préparation n'appelle
**aucune fonction de la bibliothèque**. Le `RUNPATH` vers
`build-cuda/bin` est bien inscrit, mais **aucune bibliothèque n'est chargée**.
Conséquence : **l'identité des bibliothèques n'est PAS établie pour ce pilote**,
et ce selftest n'exerce **pas** le comportement de la bibliothèque — il vérifie
des **constantes de compilation** (`io_magic`, les deux drapeaux) et des
**contrats lus dans la source**. Il ne prouve rien sur l'exécution du transfert.

`./e8-resume --selftest 1` — **`SELFTEST_RESULT=PASS FAILED=0`** :
```
SELFTEST_MAGIC=0xaf143cd9
SELFTEST_FLAGS_FULL=0 (KV attention + recurrent)
SELFTEST_FLAGS_PARTIAL_ONLY=1 (RECURRENT SEUL -> insuffisant)
SELFTEST_REJECTS_PARTIAL_ONLY=YES
SELFTEST_HEADER_BYTES=8 (magic=4 + seq_id=4)
SELFTEST_ATTN_LAYERS=16   SELFTEST_RECR_LAYERS=48   SELFTEST_TOTAL=64
SELFTEST_PREFIX_TOKENS=58 (positions 0..57)
SELFTEST_NEXT_DECODE_POS=58 (59e ID)
SELFTEST_ERR_EXPORT=exception -> log -> retour 0
SELFTEST_ERR_MAGIC=magic != 0xaf143cd9 -> throw 'wrong sequence state magic' -> retour 0
SELFTEST_ERR_RESTORE=state_read_meta/data faux -> clear() ou seq_rm(seq_id,-1,-1) PUIS throw
SELFTEST_DESTRUCTIVE_ON_FAILURE=YES  (l'etat destination est EFFACE avant l'exception)
SELFTEST_OBRAIN_STATE_TYPE=OBRAIN_STATE_TYPE=f16|i8 (alias OBRAIN_STATE_F16=1) change le blob recurrent
SELFTEST_HEADER_FNV=b9cd84e18b78a77b
```

Ces tests vérifient **la constante de magic**, **la sémantique des deux
drapeaux**, **la décomposition 16 + 48 = 64**, **le point de transfert**, et
**les quatre chemins d'erreur** — sans modèle, sans forward.

Le pilote **s'arrête** sur toute restauration incomplète : `K4_REQUIRED`
(`flags = 0`) et `N_ECRIT == SIZE_ATTENDU` sont des conditions bloquantes.

## 8. Contrat de l'expérience (préparé, non exécuté)

**Deux contextes neufs, même modèle candidat, même configuration**
(`n_ctx=512 / n_batch=512 / n_ubatch=512 / n_seq_max=1`, `ctx_type` défaut,
`embeddings_nextn=false`).

| | CONTROLE | DESTINATION |
|---|---|---|
| 1 | prefill des **58 IDs** aux positions `0..57` | **AUCUN prefill** |
| 2 | — | `set_data_ext(blob, flags=0)` |
| 3 | décodage du **59e ID** en position `58` | idem |
| 4 | génération ≤ 32 tokens | idem |

**Différence unique** : le préfixe est **calculé** (contrôle) ou **transféré**
(destination). Aucun autre écart.

**Ordre des preuves, du plus fort au plus faible :**
1. **Intégrité du transfert** : `N_ECRIT == SIZE_ATTENDU` à l'export, et le
   retour de `set_data_ext` **égal à la taille** (jamais 0).
2. **Re-export depuis la destination** : `get_data_ext(dst, flags=0)` et
   comparaison **octet à octet** avec le blob source. Attention : les
   métadonnées peuvent différer légitimement (par ex. l'index de rollback) —
   comparer **les sections**, pas l'en-tête, et documenter la représentation.
3. **Logits du 59e ID** : finitude, maximum de différence absolue, L2, argmax,
   empreinte d'octets — comme en CAUSAL-02.
4. **Tokens générés** : IDs, fragments, texte, motif d'arrêt.

**Interprétation** : identité des logits → transfert complet observé.
Différence → diagnostiquer **par section** (KV attention vs récurrent) avant
toute conclusion. **Aucun gain de temps ne sera revendiqué** : l'export/import
a un coût, et cette expérience ne mesure aucune accélération.

**Arrêt immédiat** sur : `get_size_ext` nul, `get_data_ext` ≠ taille attendue,
`set_data_ext` nul, re-export non comparable, ou toute exception.

## 9. Points encore indéterminés

1. **Taille réelle du blob** : non mesurée — exige un forward, non autorisé. La
   formule est établie, la **mesure** ne l'est pas.
2. **`n_embd_r()` / `n_embd_s()` effectifs** pour `qwen35` : accessibles par
   `hparams` (`src/llama-hparams.h:349`, `:352`) mais **non relevés** ici, faute
   d'exécution.
3. **Nombre de couches récurrentes portées par `mem_recr`** : je déduis 48
   (64 − 16 mesurées en CAUSAL-02), mais `n_layer` écrit dans le blob est
   `hparams.n_layer()` du **mémoire récurrent** — à confirmer à l'exécution, car
   une couche dont `r_l[il]` est nul est **sautée** à l'écriture.
4. **Nombre de cellules occupées** après un prefill de 58 tokens : attendu 58,
   **à mesurer**.
5. **Interaction avec `n_rs_seq`** (snapshots de rollback) : les tenseurs sont
   élargis à `(1 + n_rs_seq)` groupes et l'écriture utilise `rs_idx`. Si le
   modèle candidat configure `n_rs_seq > 0`, **le périmètre du blob change**.
   Non relevé.
6. **`OBRAIN_STATE_TYPE`** : le blob récurrent peut être en `f16` ou `i8`. La
   comparaison octet à octet n'est valide qu'**à mode constant** entre export et
   import. Non exercé.
7. **Multiplicité des positions** : le décodage du 59e ID en position `58` après
   restauration suppose que les positions restaurées couvrent bien `0..57`.
   Vérifiable par `llama_memory_seq_pos_max_attn` — **non exercé**.

---

```
TRANSFER_API=llama_state_seq_{get_size,get_data,set_data}_ext
TRANSFER_FLAGS=0 (LLAMA_STATE_SEQ_FLAGS_NONE) -> attention KV + recurrent
PARTIAL_ONLY_REJECTED=YES (recurrent seul : insuffisant pour une reprise)
BLOB_MAGIC=0xaf143cd9
RECURRENT_BLOB_TYPE=OBRAIN_STATE_TYPE=f16|i8 (defaut f32)
SELFTEST=self-executed, PASS, no model, no forward
Q1_LIBLLAMA_REBUILT=NO
PREFLIGHT_RELAUNCHED=NO
AUTHORIZATION_CREATED=NO
MODEL_FORWARD_CALL_COUNT=0
```
