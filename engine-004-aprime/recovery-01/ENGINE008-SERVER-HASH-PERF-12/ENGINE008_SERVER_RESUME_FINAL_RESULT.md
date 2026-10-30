# ENGINE-008 — RÉSULTAT FINAL SERVEUR : RESTAURATION D'ÉTAT HYBRIDE

**Dossier consolidé.** Aucune nouvelle requête modèle, aucun rebuild, aucune
optimisation supplémentaire n'a été produite pour ce document : **tout provient
des artefacts déjà mesurés**, dans `ENGINE008-SERVER-HASH-PERF-12/`.
Les dossiers historiques et leurs défauts sont conservés (§7).

---

## 1. Route exacte et identités de l'état final

### Route

```
POST http://127.0.0.1:8480/jspace/inject_knowledge
  tâche serveur : SERVER_TASK_TYPE_JSPACE_INJECT_KNOWLEDGE
  implémentation : tools/server/server-context.cpp (switch case)
  contrat JSON :
    A  { "fact_token_ids": [58 ids], "extract_only": true, "verify": true }
    B  { "fact_token_ids": [58 ids], "prompt": "OK", "n_generate": 32,
         "greedy": true, "verify": true }
    C  { "state_ref": "<ref>",      "prompt": "OK", "n_generate": 32,
         "greedy": true, "verify": true }
```

### Identités

| Élément | SHA256 |
|---|---|
| `llama-server` (exécutable) | `690bc45a54528bda9313af578e3ff5a15fa9fabc2867168580373c20d03815e3` |
| `libllama-server-impl.so` | `1d41dee2ffab28027f30f16b2480523b61f80827f413a635b468a6e18c9d65ce` |
| **Q1** `libllama.so.0` | `87c42d5f5dd7ab3c10967b4d3eaac82636b6af11e2c18a7f2798bab2d01bb2b2` |
| **libcrypto** `libcrypto.so.3` | `6facd87aa596f6d10c2029802756c56cef8761576a3e8894c42a7295a5a89d75` |
| Modèle `Qwen3.8-27B-OBRAIN-v3.gguf` | `1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937` |
| 59 IDs autoritaires `ids59.txt` | `fc39bb86b0a942c2855d4cd90016f87bf655d4463731631658324e897021dbab` |
| Rendu | `92eb0cb3a51bbece69aea29afc87553f6b5b5fdd16a1f30485a31dc38ca1c658` |
| IDs | `ee1740718fd9fc85bfdec4edac4e561be0336b791d40b2f3514d24d310ece37e` |

`libcrypto.so.3 => /lib/aarch64-linux-gnu/libcrypto.so.3` — **linkage vérifié
dans `ldd libllama-server-impl.so`**. OpenSSL 3.0.13 (30 Jan 2024).

**Sources modifiées** (identités complètes dans `IDENTITES_FINALES.txt`) :
`obrain-sha256.h` `5e26e7bb…` · `obrain-kv-cache.h` `7d24bd60…` ·
`server-context.cpp` `70c94958…` · `server-task.h` `2e9c4bcc…` ·
`server-task.cpp` `ba29159b…` · `CMakeLists.txt` `aeea20bb…`

**Configuration effective** : `n_ctx=1024` (`--ctx-size 1024`, arrondi serveur à
1536, `n_ctx_slot=512`), `--n-gpu-layers 0`, `--parallel 3`, `--jinja`,
`OBRAIN_KV_STATES_MAX_BYTES=2147483648`. Transfert : **`flags=0`**
(`LLAMA_STATE_SEQ_FLAGS_NONE`) — attention **et** récurrent.

---

## 2. Les trois paires CONTROL/RESTORED

Ordre préenregistré : **B/C, C/B, B/C**. Chaque requête HTTP a été **acceptée**
(`http=200`). Blob : **160 697 024 octets**, ref `046f5945a8dc6fc9`.

### Durées de préparation (`ms_preparation_total`, ms)

| Paire | Ordre | CONTROL | RESTORED | Ratio C/R |
|---|---|---|---|---|
| 1 | B/C | **874.322** (B1) | **144.557** (C1) | **6.0483** |
| 2 | C/B | **824.855** (B2) | **143.503** (C2) | **5.7480** |
| 3 | B/C | **804.153** (B3) | **143.289** (C3) | **5.6121** |
| | **médiane** | **824.855** | **143.503** | **5.7480** |

*La troisième durée CONTROL (B3 = 804.153 ms) a été retrouvée dans
`B3.json` : elle n'était pas consignée dans le journal de l'orchestrateur.*

**Ratio des médianes = 824.855 / 143.503 = 5.7480.** Calculé **uniquement sur
les trois paires complètes de cette expérience serveur**. *Aucun ratio du
pilote natif (`e8-resume2`, `e8-bench`) n'est transposé ici.*

### Durées HTTP totales (s)

| Paire | CONTROL | RESTORED |
|---|---|---|
| 1 | 11.095 | 10.480 |
| 2 | 11.224 | 10.490 |
| 3 | 11.351 | 10.026 |

Les totaux HTTP sont **quasi identiques** : la génération des 6 tokens domine
dans les deux conditions. **Le gain porte sur la préparation, pas sur le total.**

### Premiers logits et verdicts numériques

Les six conditions rapportent `first_logits_n = 248320` logits natifs.

| Paire | MAX_ABS_DIFF | L2 | ARGMAX | Identité binaire |
|---|---|---|---|---|
| 1 (B/C) | **0** | **0** | **13 / 13** | **YES** |
| 2 (C/B) | **0** | **0** | **13 / 13** | **YES** |
| 3 (B/C) | **0** | **0** | **13 / 13** | **YES** |

`FINITE=YES` des deux côtés. **`START_LOGITS_BINARY_IDENTICAL=YES` sur les
trois paires.**

### Compteurs du préfixe

| | B1 | C1 | B2 | C2 | B3 | C3 |
|---|---|---|---|---|---|---|
| `prefix_decode_calls` | 1 | **0** | 1 | **0** | 1 | **0** |
| `prefix_token_count` | 58 | **0** | 58 | **0** | 58 | **0** |
| `state_imported` | false | **true** | false | **true** | false | **true** |

**`DESTINATION_PREFIX_TOKEN_COUNT=0` sur les trois paires** : la destination ne
décode aucun token du préfixe et poursuit sur l'état importé.

---

## 3. IDs et textes recueillis — convention EOS

**IDs identiques pour les six conditions :**

```
[13, 198, 248069, 271, 3793, 13]
```

Positions : `[59, 60, 61, 62, 63, 64]` — **pas de 1**.
`selected_ids_count = emitted_ids_count = 6`.
`n_generate_requested = 32`.
`stop_reason = EOS`.

**Texte identique pour les six conditions :**

```
.\n</think>\n\nOK.
```

(un point, saut de ligne, `</think>` = **248069**, saut de ligne, saut de ligne,
`OK.`)

### Convention explicite concernant EOS

**Deux EOS distincts, à ne pas confondre :**

1. **EOS SÉLECTIONNÉ** — le sampler désigne `<|im_end|>` = **248046** comme
   prochain token. La boucle de production teste `if (tok == eos) break;`
   **avant** l'émission : **cet ID n'est donc NI émis NI enregistré** dans
   `decoded_ids_json`.
2. **EOS ÉMIS** — **aucun**. Dans ces six conditions, **aucun token EOS n'a été
   émis** : l'arrêt a lieu sur le test de tête de boucle.

`stop_reason = "EOS"` signale qu'un **EOS a été sélectionné** et a provoqué
l'arrêt. Les listes `decoded_ids_json` ne contiennent que les **6 tokens
émis**. `n_generate_requested = 32` est le **plafond**, jamais le nombre produit
— une sortie de 6 tokens terminée par EOS est normale.

---

## 4. Banc SHA256 — dix mesures, ordre alterné préenregistré

Ordre préenregistré : **R/N, N/R, R/N, N/R, R/N** (R = référence autoportante,
N = OpenSSL), sur **le même blob immutable de 160 697 016 octets**.

| Paire | Ordre | R (ms) | N (ms) |
|---|---|---|---|
| 1 | R/N | 798.3 | 71.2 |
| 2 | N/R | 798.5 | 70.9 |
| 3 | R/N | 798.8 | 70.9 |
| 4 | N/R | 798.6 | 70.8 |
| 5 | R/N | 798.6 | 70.8 |

```
OLD_BLOB_SHA256_MEDIAN_MS = 798.6
NEW_BLOB_SHA256_MEDIAN_MS =  70.9
GAIN_MEDIAN = x11.27   (recalcul depuis les médianes publiées : 798.6/70.9 = 11.26)
```

**Les dix empreintes sont identiques entre elles** et valent
`7a40fc6b9f270a87053ca6a352b68a2cbf83401b945b62f396eb70147cf34832` — la valeur
consignée au journal de BENCH-03. Débits **apparents** : 201 → 2267 Mo/s
(taille/durée de hachage ; **le blob est en mémoire, aucune lecture disque
n'est impliquée** — ce n'est pas une mesure de bande passante physique).

**Modèle** : `ref 64014.1 ms → new 5945.1 ms`, **gain ×10.77**. Mesure serveur
au chargement : **`MODEL_IDENTITY_MS=5978.6`** (contre 64 832.7 avant), et le
serveur est passé de **84 s à 27 s** de démarrage.

> ### TROIS NATURES DE COMPARAISON, À NE PAS CONFONDRE
>
> **(a) Banc SHA256 — COMPARAISON CONTRÔLÉE.** Les deux implémentations
> (référence autoportante et OpenSSL) sont exécutées **sur le même blob
> immutable**, sur **la même machine**, dans **le même processus**, selon un
> **ordre alterné préenregistré** (R/N, N/R, R/N, N/R, R/N), avec **cinq mesures
> chacune** et **vérification que les dix empreintes sont identiques**. C'est
> une comparaison expérimentale contrôlée : la seule variable est
> l'implémentation du hachage. → **gain ×11.27**.
>
> **(b) Trois paires serveur — COMPARAISON APPARIÉE, SÉQUENTIELLE, À ORDRE
> ALTERNÉ, DANS LA MÊME EXPÉRIENCE.** CONTROL et RESTORED sont appariés par
> numéro de paire (B1↔C1, B2↔C2, B3↔C3), exécutés **dans la même session
> serveur**, **séquentiellement** (jamais en parallèle), avec un **ordre
> alterné préenregistré** (B/C, C/B, B/C) et **une paire par condition**. Les
> deux conditions diffèrent par le chemin (recalcul vs import), **pas par
> l'implémentation**. Ce n'est **ni** un contrôle d'implémentation **ni** une
> comparaison avec un état antérieur du code. → **ratios appariés 6.0483 /
> 5.7480 / 5.6121, ratio des médianes 5.7480**.
> *Réserve : une seule mesure par condition et par paire (trois mesures par
> condition au total) ; aucune répétition intra-condition.*
>
> **(c) Anciennes versions serveur — COMPARAISONS HISTORIQUES.** Les durées
> citées au §7 et dans les dossiers antérieurs (par ex. `ms_preparation_total`
> RESTORED = 851.897 ms en DEADLOCK-11 contre 143.503 ms ici) proviennent
> d'**exécutions différentes, à des états de code différents, sur des
> configurations différentes** (DEADLOCK-11 : SHA référence, blob 160 697 024 ;
> ici : SHA OpenSSL). Elles sont **indicatives et non appariées** : elles
> situent un ordre de grandeur, **elles ne mesurent pas un effet**. Aucun de ces
> écarts ne doit être présenté comme un résultat contrôlé.

---

## 5. Coûts serveur séparés — et pourquoi ils ne s'additionnent pas

Mesures **directes** sur RESTORED (C1 / C2 / C3), en ms :

| Composante | C1 | C2 | C3 | médiane |
|---|---|---|---|---|
| `ms_state_lookup` | 50.757 | 49.750 | 49.502 | **49.750** |
| `ms_compatibility` | 0.012 | 0.012 | 0.012 | **0.012** |
| `ms_blob_sha256` | 64.518 | 64.502 | 64.505 | **64.505** |
| `ms_state_import` | 6.891 | 6.903 | 6.846 | **6.891** |
| `ms_preparation_total` | 144.557 | 143.503 | 143.289 | **143.503** |

CONTROL, pour la même expérience : `ms_prefix_compute = ms_preparation_total`
= 874.322 / 824.855 / 804.153. `ms_model_sha256 = -2.0` (NON MESURÉ sur ce
chemin : l'identité est établie **au chargement**, pas par requête), et
`ms_state_import = -1.0` / `ms_blob_sha256 = -2.0` (non applicables au
recalcul). `ms_prefix_compute = -1.0` sur RESTORED, où aucun préfixe n'est
calculé.

> **AVERTISSEMENT DE NON-ADDITIVITÉ.** La somme brute des quatre composantes
> (49.750 + 0.012 + 64.505 + 6.891 = **121.158 ms**) est **inférieure** à
> `ms_preparation_total` (**143.503 ms**), écart **22.345 ms**. Ces phases sont
> **imbriquées, pas disjointes** : `ms_compatibility` couvre la construction du
> manifeste, `ms_prefix_compute` et `ms_preparation_total` partagent la même
> horloge, et une part du travail (remise à zéro de séquence, résolution de
> l'identité modèle, contrôles d'intégrité de taille) n'est couverte par aucune
> des quatre. **Ne pas les additionner comme si elles étaient disjointes.**

`MS_STATE_IMPORT = [6.891, 6.903, 6.846] ms`, mesuré **autour de l'appel réel**
à `llama_state_seq_set_data` sur 160 697 024 octets.

---

## 6. Protections testées

Toutes exécutées sur le code de production — `obrain-kv-cache.h` est **inclus
par le serveur ET par les tests**, les règles testées sont donc celles du
serveur et non une copie.

| Protection | Suite | Résultat |
|---|---|---|
| **Compatibilité** (refus avant `set_data`) | `obrain-kv-cache-tests.cpp` | **PASS** — manifeste absent / incomplet / incompatible refusés ; **SHA256 modèle différent avec description ET topologie identiques refusé** ; Q1, types KV, mode, configuration, séquence, format |
| **Intégrité SHA256** | idem | **PASS** — blob tronqué, altéré à taille constante, SHA stocké falsifié |
| **Persistance / rechargement** | idem | **PASS** — aller-retour blob+manifeste, **préfixe exact restauré**, positions et configuration ; préfixe effacé → refus |
| **LRU et borne mémoire** | idem | **PASS** — ordre d'éviction, comptabilité après insertion/remplacement/éviction, refus d'une entrée trop grosse |
| **Concurrence du hachage** | `obrain-sha256-concurrency-tests.cpp` | **PASS** — 32 threads × 2000 hachages, **aucun sous-processus**, aucun fichier temporaire |
| **Régression du double verrouillage** | `obrain-import-deadlock-tests.cpp` | **PASS** — détecte l'ancien motif bloquant et valide le corrigé |
| **Substitution SHA256** | `obrain-sha256-substitution-tests.cpp` | **PASS** — 22 assertions, empreintes strictement identiques |
| **Vecteurs SHA256** | `obrain-sha256-tests.cpp` | **PASS** — 11 assertions |
| **Cache (total)** | | **49 PASSED / 0 FAILED** |

**Aucun contrôle n'a été affaibli** : `INTEGRITY_CHECK_REMOVED=NO`. La
vérification SHA256 à l'import est intacte et s'exécute toujours ; elle est
seulement plus rapide.

---

## 7. Expériences historiques conservées

| Dossier | Résultat | Défaut / correction |
|---|---|---|
| `ENGINE008-RESUME-NOPREFILL-01` | méthode établie | pilote **non lié** (`--as-needed` avait écarté les bibliothèques) — déclaré |
| `ENGINE008-RESUME-NOPREFILL-RUN-02` | `TOKEN_SEQUENCE_MATCH=YES` | linkage réel vérifié |
| `ENGINE008-RESUME-BENCH-03` | ratio médian **5.24×** (pilote natif) | mesure en mémoire, pas de bout en bout |
| `ENGINE008-ENGINE-RESUME-INTEGRATION-04` | mécanisme K1/K2 cartographié | « signal suspect » **faux** (erreur de lecture de ma part), corrigé |
| `ENGINE008-SERVER-CACHE-PROOF-05` | hit pur, 12.24× | logits non exposés |
| `ENGINE008-SERVER-CACHE-CLOSURE-06` | manifeste raccordé | refus non exercés |
| `ENGINE008-SERVER-CACHE-FINAL-07` | **C INTERROMPU** | cause UNKNOWN |
| `ENGINE008-SERVER-CACHE-COMPLETE-08` | C bloqué | `popen`/`fork` **réfuté** ensuite |
| `ENGINE008-SERVER-CACHE-RESOLVE-09` | SHA en processus (0 sous-processus) | C bloquait encore |
| `ENGINE008-SERVER-CACHE-ENDTOEND-10` | A/B acceptés, **C bloqué** | orchestrateur fiabilisé |
| `ENGINE008-SERVER-IMPORT-DEADLOCK-11` | **CAUSE RACINE ÉTABLIE** | auto-interblocage : `g_kv_states_mtx` **est** `g_kv_cache.mtx`, repris par le même thread |
| `ENGINE008-SERVER-HASH-PERF-12` | substitution SHA256 | présent dossier |

---

## Conclusion

**La route serveur restaure l'état hybride complet et poursuit sans recalculer
le préfixe en destination.** Sur les trois paires testées, `prefix_token_count=0`
et `state_imported=true` côté RESTORED, `58` et `false` côté CONTROL, et **les
premiers logits sont binairement identiques au recalcul** (`MAX_ABS_DIFF=0`,
`L2=0`, argmax 13/13 sur les trois paires).

**La substitution SHA256 conserve les empreintes et les contrôles**, tout en
réduisant le coût mesuré du hachage : **×11.27** sur le blob de l'expérience
(798.6 → 70.9 ms) et **×10.77** sur le fichier de modèle.

### Limites

- **Le préfixe source est calculé une fois** : le blob doit être produit, et son
  coût n'est pas absorbé. C'est un **transfert d'état après un prefill unique**,
  pas une élimination du prefill.
- **Le résultat porte sur le modèle, l'entrée et la configuration testés** —
  `Qwen3.8-27B-OBRAIN-v3`, 59 IDs autoritaires, `n_ctx=1024`, `flags=0`. Il
  n'est pas généralisé à d'autres préfixes, modèles ou réglages.
- **Aucune conclusion de qualité générale**, aucune évaluation qualité n'a été
  lancée.
- **Aucune conclusion de charge concurrente** : les requêtes ont été émises
  séquentiellement, une à la fois.
- **Aucun gain réseau ni de bout en bout** : le blob est resté **en mémoire**,
  et le coût disque/réseau **n'a pas été mesuré**.
- Le gain SHA256 est un **gain de coût de hachage**, pas un gain de bande
  passante physique.
