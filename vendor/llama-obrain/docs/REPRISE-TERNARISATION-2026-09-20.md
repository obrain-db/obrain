# REPRISE — Ternarisation de Qwen3.8-27B pour obrain

**Date** : 2026-09-20. **Nature** : document de reprise opérationnel. Il doit permettre de continuer
ce travail **avec un contexte vierge** : tout ce qui est nécessaire pour reprendre, redémarrer un
serveur, relancer un essai, et savoir ce qui est établi — est ici.

**Le récit technique complet** (méthode, mesures, ce qui a été rejeté et pourquoi) est dans
`docs/TERNARISATION-2026-09-20.md`. **Ce document-ci est le mode d'emploi** : où sont les choses,
comment les relancer, ce qui reste.

**Régime des affirmations** — celui du dépôt :

| mot | sens |
|---|---|
| **MESURÉ** | relevé par exécution, commande citée |
| **CONSTATÉ** | lu dans un fichier, ligne citée |
| **CALCULÉ** | arithmétique, valeurs données |
| **NON VÉRIFIÉ** | pas de relevé |

---

## 1 · La question d'origine, et où elle a mené

**Question** : PrismML a publié « Ternary Bonsai 2 27B » — un Qwen3.8-27B à 5,9 Go (1,76 bpw) qui
retient 98,2 % des performances. Peut-on appliquer **leurs découvertes** à **notre propre** chaîne
(llama-obrain), sans dépendre de leur fichier ni de leur fork, et **sans entraîner** ?

**Réponse, MESURÉE** : **oui, et c'est fait** — avec des choix différents des leurs, et un point
qu'ils ont et que nous n'aurons pas sans entraînement.

| | leur solution | la nôtre |
|---|---|---|
| format | trits denses `{−1,0,+1}`, groupe 128, id GGUF 143 | **cases de 2 bits**, groupe 128, id ggml **43** |
| rotation | `H∘S` (signes ±1 explicites, 28 672 valeurs embarquées) | **`H` seule** — les signes ont été **mesurés inutiles** (§6) |
| noyaux | CUDA + Metal, fork dédié | **`GGML_OP_WHT`** de premier ordre, dans notre fork |
| entraînement | leurs poids s'écartent de 48 % de la base et fonctionnent → **entraînés** | **quantification après coup**, 4,29 de perplexité à 12,17 Go |

---

## 2 · La descente, mesurée (27B, `-c 512 --chunks 2`, corpus `README.md` + cette ADR)

| variante | taille | perplexité | raisonne ? |
|---|---|---|---|
| tout ternaire, embedding et tête compris | 7,32 Go | **70 143** | bruit |
| `token_embd` Q4_K + `output.weight` Q6_K | 8,40 Go | 83,09 | — |
| + `attn_v` Q4_K | 8,43 Go | 41,71 | — |
| + `ffn_down` Q4_K | 10,12 Go | 34,29 | — |
| + les deux | 10,15 Go | 15,71 | — |
| + **4 couches de bord** Q4_K | 9,43 Go | 7,32 | **non** |
| + **8 couches de bord** | 10,32 Go | 5,70 | **OUI** |
| + 12 couches de bord | 11,22 Go | 4,99 | **OUI** |
| + 8 bords + `attn_v` + `ffn_down` | **11,61 Go** | **4,74** | **OUI** ← *artefact sauvegardé* |
| + `attn_qkv` | 12,17 Go | **4,2903** | **OUI** (756 car. de raisonnement) ← *meilleur, non sauvegardé* |
| + `ssm_out` | 11,95 Go | 4,3976 | OUI ← *non sauvegardé* |
| + `attn_output` | 11,73 Go | 4,7148 (nul) | OUI — **à ne pas payer** |
| **référence `Q4_K_M`** (Unsloth) | **18,97 Go** | **2,3983** | OUI |

**CALCULÉ** : le meilleur point connu (4,29) tient dans **12,17 Go contre 18,97** — **35 % plus
petit**, raisonnement intact, sans un seul entraînement. La combinaison `attn_qkv` + `ssm_out`
devrait tomber vers 4,0 (non mesurée).

---

## 3 · Comment relancer un essai (10 minutes)

### 3.1 Quantifier avec une politique donnée

```bash
cd /home/triviere/projects/llama-obrain/build

# politique la plus avancée connue :
OBRAIN_EDGE_LAYERS=8 OBRAIN_ATTN_V=Q4_K OBRAIN_FFN_DOWN=Q4_K OBRAIN_ATTN_QKV=Q4_K OBRAIN_SSM_OUT=Q4_K \
  ./bin/llama-quantize /mnt/obrain/models/Qwen3.8-27B-BF16.gguf /mnt/obrain/models/CANDIDAT.gguf TQ2_G128
# ~4 min, écrit ~12,5 Go
```

**Les interrupteurs de politique** (aucune recompilation nécessaire) :

| variable | effet | valeur utile |
|---|---|---|
| `OBRAIN_EDGE_LAYERS=N` | N premières et dernières couches en Q4_K | 4 · **8** · 12 |
| `OBRAIN_ATTN_V=Q4_K` | `attn_v` (16 couches pleine-attention) | Q3_K · Q4_K · Q5_K · Q6_K |
| `OBRAIN_FFN_DOWN=Q4_K` | `ffn_down` (64 couches) | idem |
| `OBRAIN_ATTN_QKV=Q4_K` | `attn_qkv` (48 couches DeltaNet) | idem |
| `OBRAIN_SSM_OUT=Q4_K` | `ssm_out` (48 couches DeltaNet) | idem |
| `OBRAIN_ATTN_OUTPUT=Q4_K` | `attn_output` — **mesuré sans effet**, ne pas payer | — |
| `OBRAIN_NO_ROTATION=1` | désarme **le repli ET la transformée** ensemble (contre-épreuve) | diagnostic |
| `OBRAIN_IMATRIX_WEIGHT=1` | active la pondération par l'imatrix — **régression mesurée**, laissée éteinte | diagnostic |

### 3.2 Mesurer

```bash
# perplexité (~5 min pour 2 passes)
./bin/llama-perplexity -m CANDIDAT.gguf -f /tmp/ppl.txt -c 512 --chunks 2 -t 8 -ngl 0 \
  | grep -E "Final estimate"
# /tmp/ppl.txt = cat README.md docs/TERNARISATION-2026-09-20.md docs/GARANTIES-NON-REGRESSION.md
```

### 3.3 Le juge qui compte : **le raisonnement**

La perplexité **ne suffit pas** : elle passe de 7,32 à 5,70 entre 4 et 8 couches de bord, alors que
la capacité de raisonner passe de **non à oui**. Le juge est donc **textuel** :

```bash
# 1) servir — --jinja est OBLIGATOIRE (sinon le gabarit du GGUF n'est pas utilisé et le modèle se tait)
nohup ./bin/llama-server -m CANDIDAT.gguf --port 8105 -c 2048 -t 8 -ngl 0 --no-warmup --jinja >/tmp/j.log 2>&1 &

# 2) juger (30 secondes) : on compte les caractères produits AVANT </think>
python3 - <<'EOF'
import json, urllib.request
Q = "Explique en une phrase ce qu'est un arbre binaire de recherche."
P = "<|im_start|>user\n"+Q+"<|im_end|>\n<|im_start|>assistant\n<think>\n"
r = json.load(urllib.request.urlopen(urllib.request.Request(
    "http://127.0.0.1:8105/completion",
    data=json.dumps({"prompt":P,"n_predict":200,"temperature":0.0}).encode(),
    headers={"Content-Type":"application/json"}), timeout=900))
a, _, b = r["content"].partition("</think>")
print(f"raisonnement {len(a.strip())} car. | reponse {len(b.strip())} car. | -> "
      f"{'RAISONNE' if len(a.strip())>20 else 'NE RAISONNE PAS'}")
print(b.strip()[:200])
EOF
kill %1
```

**Repères MESURÉS** : `raisonnement ≈ 370-760 car.` = sain ; `≈ 0` = le modèle refuse de penser
(signature de la quantification trop agressive) ; réponse vide avec raisonnement long = **budget de
jetons épuisé**, pas un défaut.

---

## 4 · Les pièges déjà payés (ne pas les refaire)

**Réglages de serveur / API — trois erreurs qui m'ont fait conclure faux deux fois :**

| symptôme | cause | correctif |
|---|---|---|
| le modèle émet `</think>` et s'arrête | serveur lancé **sans `--jinja`** → gabarit générique au lieu de celui du GGUF | `--jinja` |
| arrêt au premier jeton | `thinking_budget_tokens: 0` **n'est pas** le commutateur du gabarit Qwen3.8 | `"chat_template_kwargs": {"enable_thinking": false}` |
| `error: invalid argument: --mtp` | le drapeau de ce checkout est autre | `--spec-type draft-mtp --spec-draft-model <draft>` |
| job tué tout seul | `pkill -f "llama-server -m …"` **matche la ligne de commande de son propre shell** | tuer par PID (`$!`) |

**Deux méthodes rejetées par la mesure** (détaillées dans l'ADR §10) :

- **diffusion du résidu** : erreur de produit scalaire **0,0895 → 0,3221 (3,6× pire)**. Annule
  l'erreur seulement si l'activation est lisse sur les 128 canaux — après une rotation, non.
- **imatrix pondérée naïvement** : perplexité **83,09 → 175,65**. L'imatrix est dominée par quelques
  canaux à activation massive, et les 128 canaux d'un groupe partagent **une** échelle : mieux
  ajuster les uns dégrade les autres. *La bonne utilisation d'une imatrix est une compensation
  d'erreur de SORTIE (GPTQ), pas une pondération d'échelle — non essayée.*

**Trois erreurs de protocole de ma part, qui ont produit trois faux verdicts :**

1. **offset GGUF relatif à la section de données**, pas au fichier — ma première lecture a produit
   des statistiques plausibles et fausses (75 % puis 50 % de fenêtres « à magnitude unique », un
   artefact d'octets ASCII).
2. **métrique de produit scalaire rapportée au résultat** : sur 5120 termes elle explose par
   annulation (4,35e-05) là où la même chaîne donne **4,3e-09** rapportée à `Σ|wᵢ·xᵢ|`.
3. **verdict tiré d'un seul échantillon** avec un budget de raisonnement mal honoré.

---

## 5 · Artefacts sur disque

| fichier | taille | quoi |
|---|---|---|
| `/mnt/obrain/models/Qwen3.8-27B-BF16.gguf` | 54,66 Go | **la source** (fusion des 2 shards Unsloth) |
| `/mnt/obrain/models/Qwen3.8-27B-OBRAIN-v2.gguf` | **11,61 Go** | **candidat sauvegardé** : bords 8 + `attn_v` + `ffn_down`, PPL 4,74, raisonne |
| `/mnt/obrain/models/Qwen3.8-27B-OBRAIN-v1.gguf` | 9,43 Go | bords 4 — **ne raisonne pas**, gardé comme témoin |
| `/mnt/obrain/models/Qwen3.8-27B-TQ2G128*.gguf` | 7,3-8,4 Go | les étapes intermédiaires de la descente |
| `/mnt/obrain/models/mtp-Qwen3.8-27B-Q4_0.gguf` | 1,37 Go | draft MTP (`arch qwen35`, `block_count 65`, `nextn_predict_layers 1`) |
| `/mnt/obrain/models/obrain-9b-tq2g128.gguf` | 2,27 Go | cobaye 9B — **a servi à attraper 4 bugs de plomberie**, plus utile maintenant |
| `/tmp/imatrix-27b.dat` | 13,64 Mo | imatrix, **calculée sur 261 Ko de docs llama.cpp — corpus à refaire** |
| `/tmp/ppl.txt` | 58 Ko | corpus de perplexité (à figer dans le dépôt) |
| `/home/triviere/models/Qwen3.8-27B-Q4_K_M.gguf` | 18,97 Go | **la référence** (PPL 2,3983) |

**Aucun candidat `attn_qkv`/`ssm_out` n'est sauvegardé** (le balayage écrivait dans `/tmp`) :
pour les reproduire, relancer la commande du §3.1 avec les variables correspondantes.

---

## 5bis · La provenance exacte de la source (à ne pas redemander)

**MESURÉ.** Le modèle de départ n'est pas un artefact du dépôt : il vient d'un téléchargement, et
sans ces trois commandes un repreneur ne peut pas le reconstruire.

```bash
# 1. les deux shards BF16 (49,99 Go + 4,67 Go) — Unsloth, dépôt public, aucun jeton requis
mkdir -p /mnt/obrain/models/qwen38-bf16 && cd /mnt/obrain/models/qwen38-bf16
for i in 00001 00002; do
  curl -sL -C - -o "Qwen3.8-27B-BF16-$i-of-00002.gguf" \
    "https://huggingface.co/unsloth/Qwen3.8-27B-GGUF/resolve/main/BF16/Qwen3.8-27B-BF16-$i-of-00002.gguf"
done

# 2. fusion en un seul fichier — 54,66 Go (attendu : la somme exacte des deux shards)
/home/triviere/llama-bin/llama-b9222/llama-gguf-split --merge \
  Qwen3.8-27B-BF16-00001-of-00002.gguf /mnt/obrain/models/Qwen3.8-27B-BF16.gguf
```

**Le corpus de perplexité et l'imatrix** sont dans `/tmp` — donc **périssables** — mais reconstruits
par ces deux commandes :

```bash
# corpus (58 743 octets, 989 lignes) — GARANTIES-NON-REGRESSION.md vit dans banc-obrain, pas ici :
# son absence est silencieuse (2>/dev/null), le corpus n'a que deux fichiers
cat README.md docs/TERNARISATION-2026-09-20.md > /tmp/ppl.txt

# imatrix (~9 min 37, 13,64 Mo) — ATTENTION : calculée sur de la doc llama.cpp, un corpus ÉTRANGER
# au trafic obrain. La refaire sur le contenu du graphe est un levier identifié, non essayé.
./bin/llama-imatrix -m /mnt/obrain/models/Qwen3.8-27B-BF16.gguf -f /tmp/calib.txt \
  -c 512 -b 512 --chunks 12 -t 8 -ngl 0 -o /tmp/imatrix-27b.dat
```

**Ce qui n'est PAS versionné, par nécessité** : les modèles (11,61 Go le candidat, 54,66 Go la source)
vivent sur `/mnt/obrain/models/` — un **volume de 935 Go** distinct de la racine. Le dépôt porte la
**recette**, pas les poids.

## 6 · Ce qui a été construit dans le code (tout NON COMMITÉ)

**20+ fichiers dans l'arbre de travail de `llama-obrain`, rien de commité.**

| pièce | fichier | porte |
|---|---|---|
| **Type `GGML_TYPE_TQ2_G128 = 43`** (groupe 128, cases de 2 bits, 2,125 bpw) | `ggml/include/ggml.h`, `ggml/src/ggml-common.h`, `ggml-quants.{c,h}`, `ggml.c`, `ggml-cpu/{quants.c,quants.h,ggml-cpu.c,ops.cpp}` | **`test-quantize-fns` VERT** — erreur 0,006233 (contre 0,008681 pour `TQ1_0`), produit scalaire 0,089486 (contre 0,141345) |
| **`GGML_OP_WHT`** — la transformée, de premier ordre | `ggml.h`, `ggml.c`, `ggml-cpu/ops.{h,cpp}`, `ggml-cpu.c` | **`test-obrain-rotation-ggml` 7/7** — op ≡ matrice dense+hint ≡ repli scalaire : **0.000e+00** |
| **La rotation** `A = H` (signes **abandonnés sur mesure**) | `src/obrain-rotation.{h,cpp}` | **`test-obrain-rotation` 51/51** — orthogonalité, `AᵀA=I`, contrat à 4,3e-09 |
| **Le crochet du graphe** (3 lignes) | `src/llama-graph.cpp` `build_lora_mm` : `if (w->type == GGML_TYPE_TQ2_G128) cur = ggml_wht(...)` | couvre **toutes** les projections linéaires de tous les modèles |
| **Le repli dans le quantifieur** + politique + interrupteurs | `src/llama-quant.cpp`, `include/llama.h`, `src/llama-model-loader.cpp`, `tools/quantize/quantize.cpp`, `tests/test-quant-type-selection.cpp` | `test-quant-type-selection` **33/33** sur 4 architectures |
| **L'op sur un appareil réel** | `tests/test-obrain-wht-device.cpp` | **Vulkan déclare l'op NON supportée** ; le schédulicateur la place sur le CPU → **contrat tenu sur le 780M (1,04e-09)** |
| **Le document technique** | `docs/TERNARISATION-2026-09-20.md` | méthode, descente, rejets, verdict §11 corrigé |

**Les 4 portes, en une commande :**
```bash
cd /home/triviere/projects/llama-obrain/build
for t in test-obrain-rotation test-obrain-rotation-ggml test-obrain-wht-device test-quantize-fns; do
  printf "%-30s " "$t"; ./bin/$t >/dev/null 2>&1 && echo VERT || echo ROUGE; done
```

---

## 7 · Ce qui reste — par ordre d'effet

### 7.1 Le juge structurel (**la synergie avec obrain**, et le prochain artefact)

**La remarque qui commande ce point** : *« le graphe évolue constamment, et c'est surtout sur des
structures logiques de graphe qu'il m'intéresse de mesurer »*. Un corpus figé pourrit ; et ce que le
chat demande au modèle n'est pas de la culture générale, c'est de la **structure**.

**Le juge doit donc être engendré par le graphe, et le graphe doit être l'oracle** : quatre familles,
chacune avec une vérité terrain que le graphe calcule lui-même — aucun juge LLM, aucune annotation.

| famille | question | oracle |
|---|---|---|
| atteignabilité | « depuis X, quels nœuds sont atteignables en ≤ 2 arêtes de type A ? » | BFS borné |
| cycles | « parmi ces arêtes, lesquelles forment un cycle ? » | détection de cycle / SCC |
| degré | « quel nœud a le plus d'arêtes entrantes de type B ? » | comptage |
| redondance transitive | « cette arête est-elle impliquée par les autres ? » | fermeture transitive |

**Pourquoi c'est plus discriminant** : ces familles testent la **précision sur des identifiants et des
relations** — exactement ce que la quantification abîme en premier (PrismML : *« les chaînes de
raisonnement soutenues »*, le tool-driven long horizon). Ma question sur l'arbre binaire ne teste que
la fluidité, ce qui survit le plus longtemps.

**À construire** : `scripts/juge-structurel.mjs` (génération, sérialisation du sous-graphe, oracle,
score) → puis **relancer le balayage du §3.1 contre lui**, et la décision devient : *à partir de
quelle politique le modèle cesse-t-il de se tromper sur la structure ?*

### 7.2 La calibration sur **ton** corpus

L'imatrix a été calculée sur de la documentation llama.cpp — un corpus étranger, et c'est une part
de l'échec de la pondération naïve. L'imatrix mesure « l'importance de chaque canal d'entrée **sur le
texte qu'on va servir** ». **obrain est le sélecteur de corpus qui manque** : les fonctions les plus
interrogées, leur source, les échanges passés. Une commande (`llama-imatrix -f <corpus>`), puis
rejouer la quantification.

### 7.3 Les deux noyaux pour le 780M

**MESURÉ** : le binaire Vulkan sert le modèle ternaire à **2,01 tok/s contre 1,90 en CPU pur** →
**tout reste sur le CPU**. Deux causes :

| manque | conséquence |
|---|---|
| `mul_mat_vec`/`mul_mat` Vulkan pour `TQ2_G128` | les 410 matrices ternaires ne peuvent pas être calculées sur le GPU |
| un shader `GGML_OP_WHT` | chaque rotation ferait un aller-retour hôte↔périphérique |

**CALCULÉ** : une fois écrits, ~5 tok/s attendus sur le 780M (≈ 50 Go/s ÷ 12,17 Go).
**Le même travail est à faire côté CPU** (vec_dot SIMD/AVX2) : mon noyau est **scalaire de
référence**, et c'est pourquoi le ternaire n'est aujourd'hui que **+8 %** plus rapide que la
référence (1,86 contre 1,72 tok/s, même juge) alors que sa taille promettait ~1,6×.

### 7.4 Le MTP — **levier GPU, perte CPU**

**MESURÉ** : `--spec-type draft-mtp` + draft 1,37 Go → **1,41 tok/s contre 1,86 sans : −24 %**.
Sur CPU, chaque pas spéculatif lit *en plus* les 1,37 Go du draft, et la machine est bornée en bande
passante. Les +1,7× documentés dans le dépôt étaient mesurés **sur le 780M**. **À réactiver quand les
shaders existent**, pas avant.

---

## 8 · Ce qui n'est pas établi

| question | état |
|---|---|
| La qualité sur **les tâches obrain** (`qa_obrain.py`) | **NON MESURÉE** — le seul juge final, et il est maintenant possible |
| La combinaison `attn_qkv` + `ssm_out` | **NON MESURÉE** (attendu ~4,0) |
| La calibration sur notre corpus | **NON MESURÉE** |
| La vitesse réelle avec noyaux optimisés | **NON MESURÉE** (+8 % aujourd'hui ; ~1,6× attendu) |
| Le débit GPU du 780M | **NON MESURÉ** (2,01 tok/s = CPU pur ; ~5 tok/s attendu) |
| Les 7,4 Go rendus, traduits en contexte/créneaux | **NON MESURÉS** — c'est pourtant l'axe que l'audit du 19/09 désigne comme le verrou |
| L'`attn_output` : sans effet de perplexité, **mais sans effet sur les tâches ?** | **NON TRANCHÉ** (une seule mesure) |

**Et une réserve qui tient depuis le début** : la perplexité et mon juge textuel ne sont pas un
benchmark. Ce qui est **établi**, c'est que **la voie sans entraînement fonctionne** — pas qu'elle
atteint la référence (4,29 contre 2,40).

---

## 9 · Reprise immédiate — les trois premières commandes

```bash
# 1) les portes (30 s) — elles doivent être vertes avant toute modification
cd /home/triviere/projects/llama-obrain/build
for t in test-obrain-rotation test-obrain-rotation-ggml test-obrain-wht-device test-quantize-fns; do
  printf "%-30s " "$t"; ./bin/$t >/dev/null 2>&1 && echo VERT || echo ROUGE; done

# 2) le meilleur candidat connu, sauvegardé, qui RAISONNE
./bin/llama-server -m /mnt/obrain/models/Qwen3.8-27B-OBRAIN-v2.gguf --port 8105 -c 2048 -t 8 -ngl 0 \
  --no-warmup --jinja &
# puis le juge du §3.3 (compte les caractères avant </think> ; attendu ≈ 370 car.)

# 3) le prochain essai : la combinaison des deux meilleurs ajouts
cd /home/triviere/projects/llama-obrain/build
OBRAIN_EDGE_LAYERS=8 OBRAIN_ATTN_V=Q4_K OBRAIN_FFN_DOWN=Q4_K OBRAIN_ATTN_QKV=Q4_K OBRAIN_SSM_OUT=Q4_K \
  ./bin/llama-quantize /mnt/obrain/models/Qwen3.8-27B-BF16.gguf /mnt/obrain/models/Qwen3.8-27B-OBRAIN-v3.gguf TQ2_G128
# puis §3.2 (perplexité, attendu ≈ 4,0) et §3.3 (raisonnement)
```

---

# ADDENDUM — mesuré le 2026-09-21 (session suivante)

Cet addendum **corrige et complète** ce document. Le détail est dans
`docs/ETALONNAGE-JUGE-2026-09-21.md` (mesures, protocoles, instruments, réserves).
Régime des affirmations inchangé : **MESURÉ** = relevé par exécution.

## A1 · Les trois commandes du §9 ont été exécutées

| ce que le §9 annonçait | ce qui a été MESURÉ |
|---|---|
| §9-3 : `attn_qkv` + `ssm_out` → **attendu ≈ 4,0** | **4,0825** à 12,51 Go — **c'était juste**, et c'est le **meilleur point connu** (battait 4,2903) |
| §8 : « la perplexité reproduit-elle ? » | **4,7407** contre **4,74** consigné — l'instrument reproduit le dossier |
| §3.3 : « le candidat raisonne ? » | **582 car.** de raisonnement — **OUI** |

**Artefacts** : `Qwen3.8-27B-OBRAIN-v3.gguf` (= le `combo` du §9-3, **recommandé** : 12,51 Go, PPL
4,0825, raisonne) et `Qwen3.8-27B-OBRAIN-v4-qkv6.gguf` (13,16 Go, **4,0443** — 0,94 % de mieux pour
5,2 % de taille en plus ; à préférer seulement si la perplexité prime).

**Leçon de méthode, payée cher** : `/tmp` est sur la **racine** (98 % pleine), et **`/mnt/obrain` est
un volume distinct** (935 Go). Le balayage précédent écrivait ses candidats dans `/tmp` — **c'est la
raison exacte** pour laquelle les artefacts `attn_qkv`/`ssm_out` avaient disparu. Deux fichiers de
11 Go y dormaient encore (dont un doublon **prouvé par SHA-256** de `v2`). Écrire sur `/mnt`,
toujours ; `scripts/balayage.sh` le fait, avec un journal TSV append-only.

## A2 · §7.1 — le juge structurel : **il ne peut pas encore juger**

C'était « le prochain artefact » et « l'instrument à relancer en premier ». Il a été relancé, et
**étalonné contre la référence** — ce qui n'avait jamais été fait. Résultat :

| famille | forme de la réponse | **référence Q4_K_M** | ternaire `v2` |
|---|---|---|---|
| `degre` | un seul nœud | **100 %** | 67 % |
| `redondance` | OUI / NON | 67 % | **100 %** |
| `atteignabilite` | **énumérer un ensemble** | 33 % | 0 % |
| `cycles` | **énumérer un ensemble** | **0 %** | 0 % |

**Le meilleur modèle disponible fait 0/3 sur `cycles`** : le défaut est dans la **question**, pas dans
le modèle. Les deux familles qui départagent sont exactement celles qui **ne demandent pas
d'énumérer**. Reposées en questions **fermées** (OUI/NON sur un nœud nommé), les mêmes questions
passent de **0 % → 67 %** (`cycles`) et **33 % → 67 %** (`atteignabilite`).

**Trois conséquences pour la suite :**

1. **`cycles` et `atteignabilite` doivent être réécrites en questions fermées** avant de servir.
   Le juge n'est pas à abandonner, il est à **reformuler**. (Une autre session avance dans la même
   direction avec une famille `voisins` à 1 saut : réduire la **taille** de l'ensemble plutôt que
   **fermer** la question — les deux remèdes sont à comparer.)
2. **Monter `n` à ≥ 10 par famille.** À `n = 3`, une case vaut ±33 points, et le juge a rendu le
   ternaire **au-dessus** de la référence sur `redondance` — ce qui ne prouve rien d'autre que la
   grossièreté de la mesure.
3. La forme `indice` **n'aide pas** : mesurée, elle donne des scores **identiques** à la forme `nom`
   (les 4 paires, les 2 modèles). **L'échec n'est pas la copie d'identifiant.**

## A3 · §7.1 (suite) — deux défauts du juge, constatés

- **`q_redondance` laisse passer des arêtes EN DOUBLE** (9,3 % des graphes) : elle retire l'arête
  directe mais ajoute le triangle sans vérifier l'appartenance. L'énoncé écrit deux fois la même
  ligne. Sans effet mesuré sur les scores, mais un juge qui teste la précision ne devrait pas mentir
  sur son propre compte d'arêtes. **Correctif : une ligne** (construire dans un `set`).
- **`degre` n'engendre que 38 % du temps** (282/750) : « in-degré ≥ 2 **et** gagnant unique » est
  rare. La famille coûte donc plus d'essais que les autres.

## A4 · §7.3 / §8 — deux questions tranchées par la mesure

- **§8 « la combinaison `attn_qkv` + `ssm_out` »** : mesurée, **4,0825** (voir A1).
- **« Le bit manquant est-il sur l'attention ? »** (le prochain essai annoncé) : **NON.** Une variable
  à la fois depuis le meilleur point : `attn_v` Q4_K → Q5_K ne gagne **rien** (−0,05 %) ; `attn_qkv`
  Q4_K → Q6_K gagne **0,94 %** pour **+0,65 Go**. L'attention est **quasi saturée** à Q4_K, alors que
  l'écart à la référence est de **68 %**. **La qualité manquante n'est pas là** — la piste est close,
  et c'est un résultat utile : raffiner l'attention a un coût marginal démesuré.

## A5 · §7.1 / §8 — `qa_obrain.py` : la cause était un DÉSACCORD DE SCHÉMA

Ni l'IAM, ni des données perdues. **Aucun** des 105 546 nœuds `Function` du store ne porte `text`
(compté : `WHERE n.text IS NOT NULL` → **0**), ni `code`/`body`/`source`/`content`. Le store est
**structurel** : il porte un **pointeur** (`file_path` + `start_line` + `end_line`). Le script lisait
le mauvais champ, et **avale l'exception en silence** — il aurait posé la question **avec un fait
vide**, un verdict qui ressemble à un verdict.

**Débloqué et mesuré** (`ops-gpu16/qa_obrain_source.py`) : la source est **sur la machine**
(`/tmp/linux_src`), le préfixe `drivers/net/` est **vérifié**, et le flux complet
*store → résolution → extract i8 (69 MiB) → injection par référence → réponse* **fonctionne avec le
ternaire `v3`** — à **1,81 tok/s**, sous IAM armé et **sans désarmer aucune garde** (l'identité vient
de `~/.config/obrain-harness/iam-token.env`, la voie documentée).

**Et la réponse a été VÉRIFIÉE contre la source** : `ixgbe_probe` porte bien
`WARN(... "should not be a VF!")` puis `return -EINVAL`, et le modèle l'a énoncé correctement.

**Ce qui n'est pas établi** : **une** question n'est pas une validation de qualité — c'est une preuve
de **fonctionnement**. Le reste du §8 (vitesse avec noyaux optimisés, débit GPU, les 7,4 Go traduits
en contexte) reste **NON MESURÉ**.

## A6 · §A2 corrigé — le défaut n'était pas dans la QUESTION, mais dans le RÉGLAGE

**A2 concluait** : « le meilleur modèle disponible fait 0/3 sur `cycles` : le défaut est dans la
**question**, pas dans le modèle ». **C'est faux** — ou plus exactement, ce n'est qu'une des deux
causes, et ce n'est pas celle qui était à l'œuvre.

**L'explication concurrente, qui n'avait jamais été testée** : le juge interroge avec
`chat_template_kwargs: {"enable_thinking": False}`. Les deux familles qui échouent (`cycles`,
`atteignabilite`) demandent d'**énumérer** un ensemble après plusieurs sauts ; les deux qui passent
(`degre`, `redondance`) demandent **un** nœud ou **OUI/NON**. Une variable oubliée explique mieux
cette frontière qu'un défaut de rédaction : **le brouillon**.

**Protocole** (`scripts/juge-raisonnement.py`) — essai **apparié** : les mêmes questions, mot pour
mot, posées deux fois au même modèle, servi **une seule fois**, la seule variable étant
l'autorisation de raisonner. Les questions sont reproduites par le générateur du juge lui-même (même
graine, même consommation du générateur).

**Contrôle intégré, et il passe** : le bras « sans brouillon » **reproduit exactement** l'étalonnage
consigné en A2 — `cycles` 0/3, `atteignabilite` 1/3. Les questions n'ont donc pas bougé, et tout
écart est imputable à la variable testée.

| famille | sans brouillon (A2) | **avec brouillon** | temps / question |
|---|---|---|---|
| `cycles` | **0/3** | **3/3** | 34 s → **533-798 s** |
| `atteignabilite` | **1/3** | **3/3** | 20-24 s → **374-458 s** |

**MESURÉ** : 6 questions sur 6 réparées par le seul ajout du brouillon, raisonnements de 919 à
1 762 caractères. Le plafond du juge passe de **0 % et 33 %** à **100 % et 100 %**.

**Pourquoi la référence échouait — analysé, pas supposé** : ses échecs sont des **sous-ensembles**
exacts. Sur `cycles` q1, elle rend `1005, 1006, 1007` alors que l'oracle attend aussi `1002, 1004`.
Or `{1002,1004}` et `{1005,1007}` sont deux 2-cycles **distincts** et `{1005,1006,1007}` un 3-cycle :
la référence a **trouvé une composante et ignoré entièrement l'autre**. Ce n'est **pas** un filtre par
longueur de cycle (elle inclut le nœud du cycle le plus long) — c'est de l'**énumération incomplète**,
exactement le profil d'un modèle privé de brouillon.

**Ce que cela change, et ce que cela ne change pas** :

1. **Les deux remèdes existent, ils ne sont pas exclusifs.** Fermer la question en OUI/NON (A2 §1)
   *et* laisser raisonner réparent tous deux l'échec. Mais la **cause** désignée en A2 était la
   mauvaise : les questions n'étaient pas défectueuses — elles étaient posées dans un régime qui ne
   permet pas d'y répondre. Corriger la rédaction aurait soigné le symptôme en laissant la mesure
   aveugle à la capacité réelle.
2. **Le coût est le vrai obstacle, pas la validité.** ~28 s/question sans brouillon contre ~520 s
   avec : **dix-huit fois** plus cher, sur un modèle servi au CPU. Un juge à `n ≥ 10` sur 4 familles
   et 2 modèles coûte dès lors ~14 h avec brouillon, contre ~45 min sans. **C'est la raison — et la
   seule — pour laquelle les questions fermées gardent leur intérêt** : elles mesurent la même
   capacité à un coût compatible.
3. **Aucun score de candidat n'est comparable** tant que le régime n'est pas fixé : les colonnes
   « ternaire » d'A2 (0 %, 0 %) ont été relevées **sans** brouillon, donc elles mesurent la même
   incapacité partagée, pas une différence de quantification.
4. **`degre` et `redondance` restent valides** au plafond relevé (100 % et 67 %) : ces deux familles
   sont utilisables en l'état, et le ternaire y est déjà au-dessus de la référence sur `redondance`
   (100 % contre 67 %) — ce qui, à `n = 3`, ne prouve rien (A2 §2).

**Empreinte du juge** : l'étalonnage A2 a tourné sur `6c63ff58`, cet essai sur `1d8aec17` — le juge a
été modifié entre les deux (famille `voisins`, par l'autre session). Le contrôle intégré prouve que
les questions de `cycles` et `atteignabilite` sont **inchangées** ; les scores restent comparables.
Corollaire de méthode : **un juge sans empreinte consignée ne permet pas de savoir si l'on compare
la même chose** — d'où la colonne d'empreinte ajoutée au journal de `scripts/juge-plafond.sh`.

**Écueil rencontré en étendant l'essai au candidat** (MESURÉ, 2026-09-21 10:53). Le premier essai
apparié sur le ternaire `v3` a rendu, pour `cycles` q1, une **réponse VIDE après 2 010 caractères de
raisonnement**. Ce n'est **pas** un échec du modèle : la génération s'est arrêtée sur le plafond de
**1 024** jetons que le script s'était fixé — 1 024 ÷ 754 s = **1,36 tok/s**, exactement le débit du
ternaire. Trois conséquences :

- **Un budget calibré sur la référence ne suffit pas au candidat.** Le ternaire raisonne *plus* que
  la référence (2 010 car. et toujours en train d'écrire, contre 1 602 car. avec réponse rendue). Le
  même plafond qui laisse la référence conclure **tronque** le candidat — et la troncature se lit
  comme un score de 0. Comparer deux modèles sous un budget fixé mesure donc le budget autant que les
  modèles, dès que l'un raisonne plus longuement.
- C'est le **frère des pièges du §4** (« `thinking_budget_tokens: 0` n'est pas le commutateur »,
  « réponse vide avec raisonnement long = budget épuisé »). Il méritait d'être nommé **dans
  l'instrument** : `--n-predict-reflexion` était déclaré mais **jamais utilisé** (corrigé), et un
  budget épuisé est désormais **signalé** au lieu de compter comme une faute du modèle.
- **Coût** : à 1,36 tok/s et plus de 1 024 jetons par question, l'essai apparié sur le ternaire coûte
  **>25 min par question**, soit **>2 h 30** pour les six. C'est la contrainte réelle — pas la
  validité. Elle renforce A6 §2 : sous brouillon, un juge structurel n'est pas un instrument de
  dépannage.

**Un point mesuré — un seul** (MESURÉ, 2026-09-21 11:09, budget 2 048). Sur `cycles` q1, **la même
question** que celle posée à la référence, le ternaire `v3` passe de **✗ (16 s, réponse incomplète :
`1002, 1005, 1006, 1007`, le 2-cycle `{1002,1004}` amputé de `1004`)** à **✓ (952 s, 2 422 car. de
raisonnement, réponse exacte)**. Le brouillon ne répare donc pas la seule référence : il répare aussi
le **candidat**, qui atteint la même réponse en **1,8× plus de temps** et 1,5× plus de raisonnement.

| sur `cycles` q1 | sans brouillon | avec brouillon | raisonnement |
|---|---|---|---|
| référence Q4_K_M | ✗ 34 s | ✓ 533 s | 1 602 car. |
| ternaire `v3` | ✗ 16 s | ✓ 952 s | 2 422 car. |

**Ce qu'on peut en dire** : à ce point, ce qui sépare la référence du ternaire sur cette famille est le
**coût**, pas la capacité — les deux ont besoin d'un brouillon et les deux réussissent avec. **Ce qu'on
ne peut PAS en dire** : rien de général. C'est **une** question ; en tirer un verdict reproduirait
exactement l'erreur n°3 du §4 (« verdict tiré d'un seul échantillon ») que ce document paie déjà.

**Ce que ceci n'établit pas** : `n = 3` reste grossier (±33 points par case) ; l'essai apparié complet
n'a été mené que sur la **référence**, et sur **une** question pour le candidat. Le chiffre qui
manque — *le candidat tient-il le plafond sur les deux familles, à `n` suffisant ?* — n'est **PAS
MESURÉ**, et son coût est désormais connu : **>25 min par question** sur le ternaire.

## A7 · §A5 complété / §8 — `qa_obrain` sur cinq cibles : la plomberie tient, **deux réponses VRAIES, une non ancrée**

A5 a prouvé le fonctionnement sur **une** question, et il a lui-même écrit la réserve : « une
question n'est pas une validation de qualité — c'est une preuve de fonctionnement ». L'essai a été
porté à **cinq cibles distinctes**, donc cinq résolutions de source différentes
(`scripts/qa-campagne.sh`, journal `/mnt/obrain/qa/journal.tsv`). Le résultat a deux parties, et la
seconde **corrige la lecture d'A5**.

**1 · La plomberie : 5/5.** `ixgbe_probe`, `e1000_probe`, `tg3_open`, `rtl8169_init_one`,
`sja1000_open` — la chaîne *store → résolution du pointeur → extract i8 → injection par référence →
réponse* aboutit pour les cinq. Extract 48-94 s, réponse ~50 s, **1,85-1,90 tok/s**, sous IAM armé et
**sans désarmer aucune garde**. La moitié graphe seule (`--dry`, aucun modèle) résout aussi les cinq.
A5 n'était donc pas un coup de chance.

**2 · À 90 jetons, 5/5 TRONQUÉES — aucune n'est complète.** Longueurs **mesurées** : 255, 312, 319,
352, 379 caractères, et **toutes** s'interrompent en pleine phrase ou en pleine pensée. C'est le
budget `N_GEN = 90` : 90 jetons ≈ 315 caractères, et les cinq ont mis **~50 s**, soit 90 jetons à
1,85 tok/s. Deux des cinq (`rtl8169`, `sja1000`) partent même en **raisonnement** — que le chemin
`/jspace/inject_knowledge` **ne ferme pas** — et consomment les 90 jetons avant d'écrire quoi que ce
soit : c'est encore le piège du §4 (« réponse vide avec raisonnement long = budget épuisé »), cette
fois dans la chaîne obrain.

**3 · Le budget relevé à 400 ne corrige pas tout — il révèle le reste.** `n_generate` a été rendu
réglable par l'environnement (`OBRAIN_N_GEN`, une ligne, **défaut inchangé à 90**), et la campagne
rejouée à **400 jetons** : les réponses passent de 255-379 à **1 092-1 656 caractères**, et le débit
tombe de 1,85 à **1,46-1,57 tok/s** — le raisonnement coûte. C'est seulement là que la qualité devient
mesurable, et le résultat n'est pas uniforme :

| cible | fait injecté | à 400 jetons | confronté à la source |
|---|---|---|---|
| `tg3_open` | ✓ `tg3_open` | explication FR, 1 581 car. | **✓ VRAIE** |
| `sja1000_open` | ✓ `sja1000_open` | `<think>` puis explication FR, 1 443 car. | **✓ VRAIE** |
| `e1000_probe` | ✓ `e1000_probe` | explication FR, 1 656 car. | non confrontée |
| `ixgbe_probe` | ✓ `ixgbe_probe` | **continuation du code**, 1 092 car. | sans objet |
| `rtl8169_init_one` | **✗ `e1000e_get_bus_info_pcie`** | prose fluide, 1 539 car. | **✗ NON ANCRÉE** |

**4 · Deux réponses VRAIES, vérifiées par un tiers** — par lecture de la source, pas par le modèle :

- `tg3_open` : la source porte `netdev_err(dev, "Failed to open device. PCI error recovery …")` puis
  `return -EAGAIN;` — exactement ce que la réponse énonce.
- `sja1000_open` : l'ordre `set_reset_mode` → `open_candev` → `request_threaded_irq` →
  `sja1000_start` → `netif_start_queue` est celui de la fonction.

C'est la première vérification de cette chaîne qui ne soit pas une simple preuve de fonctionnement.

**5 · Le défaut grave : `rtl8169_init_one` répond SANS être ancrée.** Le store ne contient **pas**
`rtl8169_init_one` (il contient `rtl8169_irq_mask_and_ack`, `rtl8169_get_wol`, etc. — vérifié), et
`pick_facts` retombe alors sur son **repli**, qui injecte `e1000e_get_bus_info_pcie` : une fonction
d'un **autre pilote**. Le modèle a répondu **de sa mémoire paramétrique** — en français, avec aplomb,
1 539 caractères qui ne doivent **rien** au fait injecté. Le script imprime bien « repli sur une
fonction e1000 connue », mais **la réponse, elle, ressemble à une réponse** : c'est le danger qu'A5
croyait avoir écarté (« un verdict qui ressemble à un verdict »), réapparu sous une autre forme.
**Remède évident, non appliqué** : *refuser* la question quand le store ne porte pas la fonction
demandée, au lieu de lui en substituer une autre en silence.

**6 · `ixgbe_probe` n'est pas une explication.** À 400 jetons comme à 90, le modèle **continue le
code** injecté — le fragment `WARN(… "should not be a VF!") … return -EINVAL`, puis tout le corps de
la fonction. A5 avait lu ces lignes comme une énonciation correcte : elles sont le **début d'une
recopie**. La vérification d'A5 validait une **citation**, pas une compréhension.

**Ce qui n'est PAS établi.** Sur cinq cibles : **deux** vérifiées vraies, **une** continuation de
code, **une** non ancrée, **une** non confrontée. Ce n'est **pas** un score de qualité — c'est un
échantillon, et il désigne surtout ce qui reste à réparer : le **repli silencieux** (§5) et le
**raisonnement non fermé** (§2). Tant que ces deux-là tiennent, tout chiffre de qualité produit par
cette voie mesure la configuration, pas la lecture du ternaire.

