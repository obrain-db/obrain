# INTERFACES NATIVES D'INJECTION REELLEMENT PRESENTES DANS Q1

Perimetre examine : le fork qualifie
`/home/triviere/projects/llama-obrain-cuda-f1a` (include/, src/, tools/, common/).
**Aucun catalogue theorique : uniquement ce qui est declare ET implante.**

## 1. LES TROIS INTERFACES TROUVEES

### I-A -- `llama_kv_cache_inject_layer` -- injection KV, UNE COUCHE
Declare : `include/llama.h:1382`
Implante : `src/llama-context.cpp:4776`

```c
LLAMA_API int32_t llama_kv_cache_inject_layer(
        struct llama_context * ctx,
                llama_seq_id   seq_id,
          const llama_pos    * pos,
          const void         * k_data,
          const void         * v_data,
                    int32_t    n_tokens,
                    int32_t    layer_idx);
```
- **nature** : KV (cache d'attention). Ni activation, ni graphe, ni etat recurrent.
- **entree acceptee** : `k_data` de forme `[n_tokens][n_embd_k_gqa]` et `v_data` de
  forme `[n_tokens][n_embd_v_gqa]`, **dans le type NATIF du cache**
  (`llama_kv_cache_type_k` / `_type_v`), POSITIONNELS.
- **point d'application** : les cellules `(pos, seq_id)` de la couche `layer_idx`
  du cache KV du contexte `ctx`.
- **implante ?** OUI. Elle resout le backend par `resolve_kv_for_inject(ctx)`
  puis appelle `kv->inject_layer(...)`. Code de retour 0 = accepte.

### I-B -- `llama_kv_cache_inject` -- injection KV, MULTICOUCHE
Declare : `include/llama.h:1366` · Implante : `src/llama-context.cpp:4760`
Meme semantique que I-A pour `n_layers` couches d'un coup.

### I-C -- `llama_kv_cache_project_hidden` -- injection ACTIVATION -> KV
Declare : `include/llama.h:1436` · Implante : `src/llama-context.cpp:4853`

```c
LLAMA_API int32_t llama_kv_cache_project_hidden(
        struct llama_context * ctx,
                llama_seq_id   seq_id,
          const llama_pos    * pos,
          const float        * hidden_states,
                    int32_t    n_tokens,
                    int32_t    layer_start,
                    int32_t    layer_end);
```
- **nature** : **ACTIVATION** projetee en KV. C'est l'interface de plus haut
  niveau : elle prend des etats caches f32 et les PROJETTE avec les poids du
  modele pour produire K et V, au lieu d'exiger un K/V deja calcule.
- **entree** : `hidden_states` f32 de forme `[n_tokens][n_embd]`.
- **point d'application** : cellules `(pos, seq_id)`, couches
  `layer_start..layer_end`.
- **dependance** : un cache de poids f32 dequantifies, libere par
  `llama_kv_cache_clear_dequant_cache` (`llama-context.cpp:5417`).

## 2. LE MIROIR, QUI REND L'EXPERIENCE POSSIBLE
`llama_kv_cache_extract_layer` -- declare `include/llama.h:1403`, implante
`src/llama-context.cpp` (via `kv->extract_layer`). Meme signature, buffers en
sortie. **C'est lui qui permet une entree byte-exacte** : extraire le KV d'un
prefill reel, puis l'injecter dans un contexte neuf.

Helpers associes, tous presents :
`llama_kv_cache_type_k`, `llama_kv_cache_type_v` (type natif du cache),
`llama_kv_cache_clear_dequant_cache`.

## 3. CE QUI N'EXISTE PAS -- VERIFIE
`llama_kv_cache_inject_begin` : **N'EXISTE PAS.** Il n'apparait QUE dans un
commentaire de `include/llama.h:1378`, comme precondition annoncee de
`inject_layer`. Aucune declaration, aucune implementation (2 occurrences dans
tout le fork, toutes deux dans ce commentaire).
L'implementation de `inject_layer` **ne l'appelle pas** : elle delegue
directement a `kv->inject_layer(...)`. **La precondition documentee est donc
OBSOLETE, pas bloquante** -- mais je ne le declare pas sans execution : c'est un
point a confirmer au premier appel.

## 4. POINT DE VIGILANCE : LE BACKEND EST HYBRIDE
`resolve_kv_for_inject` (`llama-context.cpp:4747`) gere DEUX cas :
`llama_kv_cache` directement, ou `llama_memory_hybrid` via `get_mem_attn()`.
Il rend `nullptr` pour `hybrid_iswa` / `iswa`, et l'appelant journalise une
erreur et rend -1.
Le commentaire attenant documente un DEFAUT RENCONTRE EN VRAI : la resolution du
type de cache ne traitait pas l'hybride, ce qui a fait echouer la validation de
taille de `inject_layer` (« rejected every correctly-sized buffer »). **Notre
modele est `qwen35`**, et le commentaire cite precisement Qwen3.5/3.6 comme
hybrides (attention + couches recurrentes Gated DeltaNet).
**A VERIFIER A L'EXECUTION** : que `get_memory()` rend bien un
`llama_memory_hybrid` et que `get_mem_attn()` rend un cache non nul. C'est un
prealable, pas une deduction.

## 5. EXEMPLE REPRODUCTIBLE EXISTANT
`tools/obrain/pstate-exp0a.cpp` -- **2565 lignes**, deja ecrit, non modifie.
En-tete verbatim :
```
//   EXP0A.0 : extract_layer idempotence (readback stable)
//   EXP0A.1 : bit reproducibility of the same computation in two independent contexts
//   EXP0A.2 : extract -> inject -> extract identity
//   EXP0A.3 : behavioral probe (K/V + logits of the next target token)
// Public API only: llama_kv_cache_{extract_layer,inject_begin,inject_layer,type_k,type_v}
```
Il prend une **liste d'IDs de tokens en fichier** (`read_ids`), et appelle :
`llama_kv_cache_inject_layer(Q.ctx, 0, pspan.data(), xk[li].data(), xv[li].data(), 48, L)`
Il porte en outre EXP0A.4 a EXP0A.11 (SELF_OVERWRITE, snapshot, stabilite
temporelle, injection de nouvelles cellules, certification de la taille de
rangee, restauration, substitution KV depuis S0).
**ETAT : NON COMPILE.** Aucun binaire `pstate-exp0a` n'existe dans le fork ni
dans `build-cuda/bin/`. Le code source est le seul artefact.

**MODIFICATION DU CHEMIN DE CALCUL : AUCUNE.** L'injection ECRIT dans le cache
KV. Elle ne modifie ni le graphe, ni les poids, ni le backend. C'est un etat
d'entree, pas une reecriture du calcul.

## 6. ENTRES NUMERIQUES DISPONIBLES (captures f1a candidates)
Verifiees fichier par fichier contre `PAYLOAD_PROVENANCE.csv` et
`GRAPH_CAPTURE_MANIFEST.csv` -- **le format est entierement documente par le
producteur**, je ne le deduis pas :

| propriete | `activation` | `base_output` |
|---|---|---|
| exemple | `ep0_sub1_t1177_c109_activation.bin` | `..._base_output.bin` |
| dtype | `0` = **f32** | `0` = f32 |
| forme | `ne0=5120, ne1=59, ne2=1, ne3=1` | `ne0=6144` |
| strides | `nb0=4, nb1=20480, nb2=1208320` | -- |
| elements | 302080 | -- |
| octets | `expected=written=1208320` | 1449984 |
| contigu | `contiguous=1` | 1 |
| **rotation** | **`rotation_applied=1`** | -- |
| cible | `blk.8.attn_gate.weight`, `expected_K=5120` | idem |
| epoch / decode_step | 0 / -1 | 0 / -1 |
| empreinte | `fnv64x4` sur le payload | idem |

**COHERENCE DE FORME, verifiee et non supposee** :
`1208320 / 4 = 302080 = 59 x 5120`. Et `5120` est bien
`qwen35.embedding_length` du modele candidat (metadonnee GGUF lue directement).
`base_output` a `ne0=6144` = `qwen35.ssm.inner_size` : c'est la dimension
**SSM/Gated DeltaNet**, pas l'attention.

**COMPATIBILITE AVEC `project_hidden` : NON DECLAREE.** La forme
`[59, 5120]` f32 correspond a ce qu'attend `hidden_states` (`[n_tokens][n_embd]`),
**mais deux points restent a etablir** :
1. `rotation_applied=1` : cette activation est la version **ROTATIVEE par la WHT**
   de l'entree de `blk.8.attn_gate`. `project_hidden` projette avec les poids
   **non rotatifs**. Injecter une activation rotationnee comme si elle ne l'etait
   pas serait une erreur de sens, pas de taille. **A trancher explicitement.**
2. Ces captures sont les entrees de projections **lineaires de couche**, pas des
   etats caches de sortie de bloc. `project_hidden` attend un etat cache.
   L'identite entre les deux n'est PAS etablie par l'existence du fichier.

**Je ne declare donc AUCUNE compatibilite depuis la seule existence des
fichiers.** Ce qui est verifie est la FORME, les OCTETS, la CONTIGUITE,
l'IDENTITE de modele et la PROVENANCE ; ce qui ne l'est pas est la SEMANTIQUE du
tenseur au regard du point d'injection.

## 7. PLAN CONTROLE / INJECTION (prepare, NON execute)
`GRAPH_INJECTION_MECHANISM=UNRESOLVED` : trois mecanismes natifs existent et
different (KV une couche, KV multicouche, activation projetee). **Le choix
conditionne l'experience** : avec I-A/I-B l'entree est un K/V extrait, avec I-C
c'est une activation f32. Je ne choisis pas silencieusement.

Pour le mecanisme I-A (KV), qui est celui du cas reproductible existant :

1. **CONTROLE** -- contexte NEUF, modele candidat, `prefill` des **59 IDs
   autoritaires** (SHA `ee174071...`), puis `extract_layer` du K/V aux positions
   `0..58` pour TOUTES les couches. Generer **32 tokens** au maximum.
2. **INJECTION** -- contexte NEUF, meme modele, **aucun prefill**.
   `inject_layer` du K/V extrait aux positions `0..58`, couche par couche.
   Generer **32 tokens** au maximum.
3. **Configuration commune** : meme template (`c3cf9e34...`), meme politique
   (`add_special=0`, `parse_special=1`), memes `n_ctx=512 / n_batch=512 /
   n_ubatch=512 / n_seq_max=1`, `ctx_type` defaut, `embeddings_nextn=false`,
   argmax direct avec departage au plus petit ID.
4. **Difference UNIQUE** : le KV est **CALCULE** par un prefill (controle) ou
   **INJECTE** depuis l'extraction du controle (injection). Rien d'autre.
5. **PREUVE D'ACCEPTATION** (exigee, separee du texte) :
   - code retour de `inject_layer` = **0** pour chaque couche ;
   - **readback** : `extract_layer` apres injection rend des octets
     **identiques** a ceux injectes (comparaison octet a octet, et non de texte).
   - `llama_kv_cache_type_k/type_v` releves et concordants avec la taille
     d'octets injectee.
6. **PREUVE DE CONSOMMATION** : les 32 IDs generes et le texte assemble, dans
   les deux conditions. **Une difference de texte seule ne prouve PAS
   l'application** ; une sortie identique ne prouve PAS qu'elle a ete ignoree.
   C'est la conjonction (5) ET (6) qui tranche.
7. **AUCUN retry.**

## 8. CE QUI MANQUE, PRECISEMENT
- **Aucun pilote compile.** `pstate-exp0a.cpp` existe mais n'est pas bati ; il
  faudrait le compiler ou ecrire un pilote minimal. **C'est une NOUVELLE
  INSTRUMENTATION** : elle n'est PAS couverte par le gel Q1 et doit etre
  qualifiee separement. Je ne la presente pas comme qualifiee.
- **`inject_begin` absent** : la precondition documentee n'est pas implementee.
  A confirmer au premier appel ; si l'appel refuse, c'est un blocage a declarer.
- **Backend hybride** : `get_mem_attn()` doit rendre un cache non nul sur
  `qwen35`. Prealable a verifier.
- **`n_embd_k_gqa` / `n_embd_v_gqa`** du modele candidat : necessaires pour
  dimensionner les buffers K/V. Non releves a ce stade.
- **Semantique de l'activation** au regard du point d'injection (rotation,
  nature du tenseur) : non etablie.
