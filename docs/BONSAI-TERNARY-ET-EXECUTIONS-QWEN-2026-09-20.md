# Bonsai 2 27B (ternaire) confronté à nos exécutions Qwen3.8-27B — et à ce qui a bougé dans obrain

**Date** : 2026-09-20. **Nature** : lecture des modifications non commitées d'obrain et de
l'engine, lecture de première main de la release PrismML « Ternary Bonsai 2 27B », et verdict
chiffré sur la question « peut-on faire la même chose chez nous ».

**Régime des affirmations** — celui du dépôt :

| mot | sens |
|---|---|
| **MESURÉ** | relevé par exécution pendant cette session, commande citée |
| **CONSTATÉ** | lu dans un fichier cité (dépôt local ou source publique), ligne/URL donnée |
| **CALCULÉ** | arithmétique, formule et valeurs données |
| **ANNONCE VENDEUR** | chiffre de PrismML, non reproduit indépendamment |
| **NON VÉRIFIÉ** | pas de relevé — et je le dis plutôt que de le combler |

---

## 1 · Ce qui a bougé dans obrain et dans l'engine

**CONSTATÉ.** Dernier commit `456d3794` du **2026-09-16 15:02**. Au-dessus :
**211 fichiers modifiés** (`git diff --stat` : +5 989 / −953) et **21 fichiers non suivis**
(5 821 lignes : 2 954 de code, 2 714 de docs, 153 de script).

| domaine | fichiers | + | − |
|---|---:|---:|---:|
| `crates/obrain-substrate` | 32 | +1799 | −329 |
| `crates/obrain-cli` | 7 | +1268 | −86 |
| `crates/obrain-engine` | 81 | +1043 | −243 |
| `crates/obrain-selfd` | 5 | +528 | −89 |
| `crates/obrain-core` | 10 | +319 | −14 |
| `crates/obrain-httpmin` | 1 | +240 | −6 |
| `crates/obrain-cognitive` | 49 | +189 | −84 |
| `packages/obrain-dashboard` | 2 | +136 | −1 |
| reste (adapters, common, garde, wasm, docs, scripts…) | 24 | +467 | −93 |

### 1.1 Les quatre chantiers de fond

1. **Le substrat sans système de fichiers** (pour wasm). `ZoneBacking::{Mmap, Ram}` dans
   `crates/obrain-substrate/src/file.rs` (+788), `RamZone`/`RamDir`, `SubstrateStore::open_memory()`,
   `WalMedium::Memoire` (wal_io.rs, +270), horloge `web-time` sur **187 lignes / 142 fichiers**
   (`std::time::SystemTime::now()` **panique** sur `wasm32-unknown-unknown`), garde anti-panic dans
   `scripts/build-wasm.sh` (refus de livrer un module qui **lie** ce panic).
   Mesures consignées : ouverture **6,547 ms → 51,0 µs (128×)**, création de nœuds par 200
   **209,7 µs → 27,3 µs (7,7×)**, suite wasm du binding **1/21 → 22/22**.
   Limites déclarées : le mode RAM **ne persiste rien et n'est pas checkpointable** (le saut du flush
   a été tenté puis **reverté**, la ré-ancrage nommant ses fichiers avec `process::id()`), et il est
   prouvé **en Node, pas dans un navigateur**.
2. **Le format d'instantané `OBRSNAP1`** — `crates/obrain-substrate/src/instantane.rs` (neuf, 707 l.,
   7 tests) : zones + méta + dictionnaire, CRC32 par zone, zéro de queue retiré. Mesuré sur le store
   réel : v1 14 078 652 o (535,0 o/slot, rembourrage ×13,16) → **v2 8 408 183 o (−40,3 %)**.
   Répartition : `substrate.props.v2` = **97,4 %** du total ; `nodes` compactée ×13, `edges` ×15.
   **v1 ne transporte ni le WAL ni les colonnes latérales.**
3. **`obrain-garde`** — crate neuf, **zéro dépendance**, portant la garde déjà commitée le 16/09
   (client parti, **budget par question 300 s**, détection de répétition) et traversant désormais
   **trois** boucles de modèle : `obrain-cli/src/engine.rs`, `obrain-selfd/src/chat.rs`,
   `obrain-selfd/src/agent.rs`. `crates/obrain-httpmin` gagne `post_sse_supervise`, dont le rappel
   peut **cesser de lire** (avant : `Result<()>`, donc aucun appelant ne pouvait arrêter).
4. **Les 25 défauts numérotés** du document `docs/GARDE-TOUR-PLAFOND-REPRISE-2026-09-17.md` — dont,
   parmi les plus lourds : le PID CDC qui **retenait le `flock` à vie** (`Weak<CdcLog>`), la **65ᵉ
   étiquette qui tuait le démon** (`.expect("label registry overflow (>64 labels)")` → garde
   `valider_etiquettes`), le DDL de types non durable (`substrate.typecat`), `wal_status()` qui
   répondait `enabled: false` sur une base qui écrit, l'agrégat dans un `WITH` GQL, la dédup CDC
   **quadratique** (16,4 / 59,0 / 228,1 ms → 4,7 / 9,9 / 18,1 ms), l'index BM25 reconstruit à chaque
   ouverture (**5 930 ms des 7,8 s** de vie d'un pont → 477 ms), et le lecteur WAL qui criait
   « corruption » sur une écriture en vol.

### 1.2 L'engine et le pont LLM

**CONSTATÉ.** Commit `4cbbf229` : `client_parti` (drapeau posé par `viz_serve` à la première écriture
SSE refusée) + `BUDGET_QUESTION_S = 300` (borne la **somme** des appels). Commit `8c76266c` :
détection de répétition + « la génération qui boucle s'arrête ». Non commité : `mcp_server.rs`
**+1064** — veilleur de requête (> 10 s → alerte), **échéance sur tout appel d'outil** (30 s),
**plafond de réponse 32 Mo**, **budget de jetons d'injection** (`OBRAIN_CONTEXT_TOKEN_BUDGET`, avec un
correctif mesuré : `estimate_tokens` plafonne chaque propriété à 500 caractères, donc 3 000 caractères
étaient comptés **135 jetons au lieu de ~750**), et la lecture des **deux conventions d'identité**
(`qn` *et* `qualified_name`) dans `graph_link`, `payload_get`, `viz_extract::resolve_node_opts`.

### 1.3 Ce qui reste hors du vert — et qui compte pour la suite

- **Le binaire déployé n'a pas les correctifs.** `~/.local/share/obrain-harness/bin/obrain`
  (16/09 17:45) ne contient **ni** `OBRAIN_REPONSE_MAX_MO` **ni** `OBRAIN_REQUETE_ALERTE_MS`
  (0 occurrence par `strings`) ⇒ **rien ne borne l'emballement en production**.
- **Dérive de compteurs du store de production** : `80 377 → 87 918` (**7 541 nœuds d'écart**), plus
  `label_only=31` nœuds « coquilles » et un `props zone v2 decode failed`. Consignés, non traités.
- **Quatre assertions de performance sont devenues opt-in** (`OBRAIN_PERF_GATE=1`) et **ne jugent
  plus par défaut**.
- **Le lot non commité ne compile pas contre `HEAD`** : `declarer_type`, `open_memory`,
  `installer_udfs_dans_le_moteur`, `valider_etiquettes`, `cancel::frontiere(...)` n'existent pas à
  `HEAD`. C'est **un seul ensemble cohérent** à committer, pas fichier par fichier.
- Flake de verrou **inexpliqué** (2 passes sur 6), `annulation_bout_en_bout` rouge sous charge.

### 1.4 L'audit KV du 19/09 — le socle de la question de vitesse

**CONSTATÉ** (`docs/AUDIT-ETATS-KV-OBRAIN-2026-09-19.md`) :

| fait | valeur mesurée |
|---|---|
| service réel | `Qwen3.8-27B-Q4_K_M.gguf`, `llama-server b53-39cbbb7`, 4 créneaux × `n_ctx 262 144` |
| décodage | **9,8 – 12,8 tok/s** · prefill **737 tok/s** au mieux |
| décodage spéculatif | **`speculative: none`** |
| bande passante effective déduite | **180–210 Go/s** |
| lien `enp2s0` | **100 Mb/s** ⇒ injecter 6,10 Gio = **524 s** contre **136 s** de prefill recalculé |

Et le défaut le plus rentable, **encore présent** : `crates/obrain-cli/src/engine.rs:2111` place
l'extrait de graphe **dans le message système, donc au premier jeton**, et le réextrait **à chaque
tour** — tout ce qui suit est invalidé. **Gratuit à corriger.**

---

## 2 · Ce que Bonsai 2 27B est réellement

**CONSTATÉ** de première main (API Hugging Face + carte modèle + lecture des métadonnées publiées) :

| fichier | taille | bits/poids réels |
|---|---:|---:|
| `Ternary-Bonsai-2-27B-PTQ1_0.gguf` | **5,947 Go** | **1,75** |
| `Ternary-Bonsai-2-27B-PQ2_0.gguf` | 7,206 Go | 2,13 |
| `Ternary-Bonsai-2-27B-F16.gguf` | 53,808 Go | 16 |
| `mmproj-Q8_0` / `BF16` | 0,63 / 0,93 Go | tour de vision, optionnelle |

### 2.1 La représentation — c'est là qu'est la valeur

1. **Ternary g128** : `w_i = s_g · t_i`, `t_i ∈ {−1, 0, +1}`, une échelle FP16 par **groupe de 128**.
2. **Base tournée** : rotation de Hadamard **par blocs de 1024**, `R = (1/√n)·H_n·S` avec `S` diagonale
   de signes ±1 **explicite et stockée**. La rotation est **repliée dans les poids offline** ; le runtime
   applique la transformée inverse **aux activations**. Contrat déclaré dans le fichier
   (`prism.hadamard.*` : `normalized-sylvester-walsh-hadamard`, `block_size = 1024`, 401 tenseurs,
   `token_embd` en inverse).
3. **Le chemin d'état récurrent n'est PAS ternarisé** : **26,2 M paramètres (0,0976 %)** gardent la
   haute précision — `in_proj_a`, `in_proj_b`, `conv1d`, `A_log`, `dt_bias`, les normalisations,
   `q_norm`/`k_norm` des couches linéaires. **C'est la réponse au point sensible de l'architecture
   hybride `qwen35`** (Gated DeltaNet + attention pleine).

### 2.2 Le runtime : un fork obligatoire

**CONSTATÉ.** Stock llama.cpp **ne charge pas** ces fichiers : `PTQ1_0`/`PQ2_0` sont des **types
privés** (ids **142/143**, ≥ `GGML_TYPE_COUNT` upstream = 43) ; et la variante `Q2_0` groupe-64,
elle, **se charge sans avertissement et sort du charabia** faute de transformée d'activation.
Carte modèle : « *Stock llama.cpp will not run these files.* »

Binaires publiés (release `prism-b10709-9a9394a`, 2026-09-18) : **linux-cuda 12.4/12.8/13.3 x64**,
**ubuntu-vulkan x64 et arm64**, **rocm 7.2 x64**, ubuntu-arm64 (CPU), macOS, Windows.
**Pas de binaire Linux ARM64 CUDA** (cas DGX Spark/GB10 : à construire soi-même).

Deux réserves sur ce fork, qui nous concernent directement :
- **Bug reproductible sur MTP + poids repliés Hadamard** (la spéculation est refusée), toujours
  présent dans `prism-b10709` ; correctif communautaire de 14 lignes (PR `PrismML-Eng/llama.cpp#205`),
  gain ~1,34× après correction. **Or le MTP est justement ce que nous voulons activer.**
- La transformée d'activation n'existe pas dans les métadonnées standard : le fichier **refuse de se
  charger** si le runtime ne sait pas appliquer la rotation — c'est une compatibilité par fork, pas par
  format.

**La recette de fabrication n'est pas publiée** : le whitepaper ne contient **aucune** section sur la
compression (ni PTQ/QAT, ni calibration, ni budget de calcul) ; ce qui est publié est le **conteneur**
(contrat Hadamard + layout binaire) et le **runtime**, pas le procédé d'assignation des trits et des
échelles. PrismML revendique une PI Caltech, et l'upstreaming de la FWHT est **en cours**
(PR `ggml-org/llama.cpp#27779` : CPU F16 seule, **largeur 1024 pas encore**).

### 2.3 La qualité — le seul chiffre qui intéresserait, et il est vendeur

**ANNONCE VENDEUR** (14 benchmarks, mode *thinking*) :

| variante | bpw | taille | moyenne |
|---|---:|---:|---:|
| Qwen3.8-27B FP16 | 16,0 | 54 Go | 86,32 |
| Qwen3.8-27B UD-Q4_K_XL | 5,2 | 17,6 Go | **85,18** |
| Qwen3.8-27B **IQ2_XXS** | 2,8 | 9,4 Go | **72,59** |
| **Bonsai 2 27B** | **1,72** | **5,9 Go** | **84,78** |

Et le détail qui compte pour un agent : **l'effondrement du 2-bit conventionnel est sélectif** —
AIME26 **57,5** (Bonsai 95,83), LiveCodeBench **56,4** (Bonsai 90,07), alors que MMLU-Redux tient
**88,93**. « *Which is why casual testing misses the collapse.* »

**Le détail qui décide pour nous est dans les annexes, pas dans le titre** : toujours selon leurs
propres tableaux, **SWE-bench Verified 60,8 contre 80,6** (≈ 75 % de rétention) et **Terminal-Bench
52,8 contre 69,7** — c'est-à-dire **exactement l'agentique long horizon**, l'usage d'obrain.

**Contre-signaux, à porter au dossier** (mesures et témoignages tiers) : `tool_bench` mesuré à
**0,4200** pour Bonsai 2 PQ2_0 (effort *medium*) contre **0,4706** pour `UD-Q4_K_XL` (*xhigh*) ;
boucles de raisonnement en *xhigh* (l'un des deux tests tiers n'a pas pu se terminer) ;
**embeddings ternarisés ⇒ corruption multilingue** (« practically unusable for languages other than
english »). Pour obrain, dont l'engine est **un client à appels d'outils** et dont le corpus est
**en français**, ces trois points sont les plus sensibles — et le français n'est pas testé par eux.

---

## 3 · Peut-on faire la même chose pour nos exécutions ? — le verdict chiffré

### 3.1 Le fait que la question suppose faux : nous sommes DÉJÀ au même budget de bits

**CALCULÉ** (`octets × 8 / 27e9`) — les modèles sont **déjà sur `/mnt/obrain/models/`** :

| modèle servi | taille | bits/poids |
|---|---:|---:|
| `Qwen3.8-27B-Q4_K_M.gguf` (production) | 18,97 Go | 5,62 |
| `UD-Q3_K_XL` | 13,15 Go | 3,90 |
| `UD-Q2_K_XL` | 9,83 Go | 2,91 |
| `UD-IQ2_XXS` | 7,27 Go | 2,15 |
| `UD-IQ1_M` | 6,73 Go | 1,99 |
| **`UD-IQ1_S`** | **6,19 Go** | **1,83** |
| **Bonsai 2 PTQ1_0** | **5,95 Go** | **1,75** |

**Bonsai est 4,7 % plus petit que notre `IQ1_S` et 14 % plus petit que notre `IQ1_M`.**
Le levier « octets par jeton » est **déjà tiré**. Ce que Bonsai apporte, ce n'est pas la taille —
c'est **la qualité à cette taille** (84,78 contre 72,59 pour un IQ2_XXS selon eux).
**Toute la question se déplace donc de « compresser plus » à « tenir la qualité ».**

### 3.2 Ce que ça donnerait, machine par machine

| machine | aujourd'hui (MESURÉ) | avec Bonsai PTQ1_0 | verdict |
|---|---|---|---|
| **prod `192.168.1.7:9910`** (18,97 Go, **11 tok/s**, `spec none`) | 5,62 bpw | **5,95 Go** ⇒ si la bande passante effective tient : **~30–35 tok/s**, **×2,7 à 3** | **le vrai gain** |
| **carte CUDA 16 Go** (plan : Q3_K_XL 13,15 Go + MTP ≥ 50 tok/s) | 6,2 Gio de marge | 5,95 Go, et **PTQ1_0 est le pack le plus rapide sur Ada** (91,1 tok/s mesuré sur 4090) | **gain net** |
| **n3 / Radeon 780M** (IQ1_M 6,73 Go, 8,7 tok/s) | mur de bande passante **~50 Go/s** | 5,95 Go ⇒ **~9–10 tok/s au mieux**, et **aucun kernel ternaire Vulkan dans le fork PrismML** | **gain nul** |

Le mur de la 780M avait déjà été chiffré chez nous le 2026-09-07 : « 50 tok/s exigerait
**≤ 1 Go/token (≈ 0,3 bit/paramètre)** » (`docs/RAISONNEMENT-OPTIM-QWEN38-780M-2026-09-07.md`).
**Bonsai ne le franchit pas : il est à 1,75 bit.**

### 3.3 Les trois voies, et ce qu'elles coûtent

**(a) Consommer Bonsai tel quel — faisable sans écrire une ligne.** Un binaire du fork
(`linux-cuda` ou `ubuntu-vulkan`) + le GGUF, pointé par `OBRAIN_LLM_URL`. Pas de portage, pas de
conversion. **Le prix : on perd les primitives de `llama-obrain`** — `/jspace/inject_knowledge`,
`OBRAIN_STATE_TYPE=i8`, MTP, captures GDN, garde de type. Le fork PrismML ne les a pas. Donc : deux
moteurs, ou portage. À noter aussi que leur décodage spéculatif (dspark, 1,8–2,4× sur CUDA) **force
`-np 1` et casse la réutilisation de préfixe** — moins grave chez nous, cette réutilisation étant
déjà documentée cassée pour `qwen35` (llama.cpp #21831).

**(b) Reproduire la méthode sur NOS poids — non, pas aujourd'hui.** Trois obstacles **CONSTATÉS** :
1. **Le procédé n'est pas publié** (pas de section compression dans le whitepaper) : on a le
   conteneur, pas l'assignation des trits et des échelles. Le contrat *public* est complet sur tout le
   reste : `hadamard.json` (401 tenseurs, `sign_widths = [5120, 6144, 17408]`, `sign_values` de
   28 672 entiers ±1, `token_embd` en transformée inverse), et le layout binaire est révélé par le
   codec MLX publié (`PTQ1_0` = **28 octets par 128 poids** : 26 o de trits + 2 o d'échelle FP16).
   **C'est bien la procédure d'assignation qui manque, et elle seule.**
2. **Notre fork n'a AUCUN kernel ternaire** — vérifié : `TQ1_0`/`TQ2_0` = **0 fichier** en
   `ggml-cuda/` *et* en `ggml-vulkan/` ; `Q2_0` = **CPU seulement**. (Nuance : upstream *a* des
   mentions Vulkan pour `TQ1_0`/`TQ2_0` — notre fork est en retard sur ce point, pas en avance.) Même
   `llama-quantize --tq1_0` ne tournerait donc pas sur GPU chez nous — alors que la release PrismML,
   elle, tourne.
3. **Le ternaire PTQ naïf n'est pas Bonsai** : le quantifieur `TQ1_0` de llama.cpp fait un arrondi
   par **bloc de 256** avec **une** échelle FP16, **sans rotation**, sans échelle par groupe. Or la
   rotation **est** le cœur de la méthode — et la littérature SSM est explicite : les corrections
   post-hoc qui marchent pour un transformer **échouent sur les SSM** (accumulation d'erreur dans la
   récurrence). PrismML garde d'ailleurs tout le chemin d'état en FP.

*Seule voie « fais-le toi-même » outillée et publiée* : **SASORI** (ternaire multi-plans, `torch`-only,
sans ré-entraînement) — mais elle vise **~6 bpw** (`K=3, g=256`), donc **plus lourde et plus lente
qu'un Q4_K_M**, et son propre README conseille d'utiliser un 4-bit si ça rentre. **Elle ne répond
pas à notre objectif de taille.**

**(c) Le chiffre qui manque, et qui est le moins cher à obtenir.**
Nos propres documents disent que **la qualité réelle de `IQ1_M` n'a jamais été validée sur questions
longues** (`docs/TEST-IQ1M-2026-09-07.md`), et l'audit KV décrit des symptômes de génération
pathologique que nous avons traités par une **garde** (arrêt sur répétition) — c'est-à-dire un
**pansement sur un symptôme que le bas-bit peut produire**. Or le seul chiffre extérieur disponible
dit que le 2-bit conventionnel s'effondre **précisément** sur le raisonnement long (AIME26 57,5) et
le code (LiveCodeBench 56,4). **C'est notre cas d'usage exact.**

---

## 4 · Ordre de travail proposé

| # | geste | coût | ce qu'il faut lire ensuite |
|---|---|---|---|
| 0 | **Activer le MTP sur le service de production** (`speculative: none` aujourd'hui, 11 tok/s). Le draft (1,28 Go) et le code MTP sont **déjà chez nous** | une ligne | `timings` avant/après sur 3 tailles d'invite |
| 0 bis | **Corriger l'ordre de l'invite** (`engine.rs:2111`) : système + outils + historique stables d'abord, extrait de graphe et question ensuite | quelques heures | `timings.cached_tokens` sur deux tours réels |
| 1 | **Mesurer la qualité de nos bas-bits sur NOS tâches** : `Q4_K_M` vs `Q3_K_XL` vs `IQ2_XXS` vs `IQ1_M`, mode *thinking*, questions réelles de l'engine **avec appels d'outils**. Modèles déjà présents, 780M suffisante (8,7 tok/s) | 1 journée | taux d'appels d'outils corrects + justesse |
| 2 | **Essayer Bonsai 2 sans portage** : binaire PrismML `linux-cuda` + `PTQ1_0.gguf` (5,95 Go), `OBRAIN_LLM_URL` pointé dessus, **les mêmes questions qu'en 1** + `tg128`/`pp512` | 1 journée | est-ce que les 98,2 % tiennent sur nos tâches ? |
| 3 | **Décider** : porter les kernels ternaires + la FWHT dans `llama-obrain` (garde jspace/MTP/état i8), ou vivre avec deux moteurs, ou rebaser quand la série FWHT atterrit upstream (largeur 1024 manquante) | semaines | `docs/` |

**Ce que je ne recommande pas** : ternariser nos propres poids maintenant (procédé non publié +
kernels à écrire + SSM = mur documenté), et se servir des types `TQ1_0`/`TQ2_0` de llama.cpp sur nos
poids (aucun kernel GPU chez nous, pas de rotation, replis `Q4_0`).

**Une phrase** : Bonsai ne nous apprend pas à compresser plus fort — nous sommes déjà à 1,83 bit ;
il nous montre qu'à 1,75 bit **on peut ne pas perdre le raisonnement**, et cette partie-là s'achète
aujourd'hui en binaire précompilé, pas en recherche. Mais avant de payer le prix (un fork de plus),
le chiffre à obtenir est **la qualité de nos propres quants sur nos propres tâches** — il n'existe
pas, et il coûte une journée.
