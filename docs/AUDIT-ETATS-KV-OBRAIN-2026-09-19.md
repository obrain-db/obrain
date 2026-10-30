# Audit — « KV cache persistant / états adressables » confronté à l'architecture obrain en place

**Date** : 2026-09-19. **Nature** : audit critique section par section d'une
proposition reçue (KV cache persistant, Merkle-DAG d'états, mutualisation H200),
confrontée à ce qui tourne réellement et à ce que le dépôt contient.

**Régime des affirmations** — celui du dépôt, tenu jusqu'au bout :

| mot | sens |
|---|---|
| **MESURÉ** | relevé par exécution pendant cet audit, commande citée |
| **CONSTATÉ** | lu dans un fichier cité, avec la ligne |
| **CALCULÉ** | arithmétique, formule et valeurs données |
| **EXTERNE** | source publique citée (URL) |
| **DÉDUIT** | conséquence d'une mesure, non observée directement |
| **NON VÉRIFIÉ** | pas de relevé — et je le dis plutôt que de le combler |

---

## 0 · Le socle : ce qui tourne, mesuré — pas supposé

Tout le reste de cet audit en dépend, donc il vient d'abord.

### 0.1 La chaîne réelle

**CONSTATÉ.** Le dashboard vivant (`viz serve`, pid 1781125, `:8432`) porte dans
son environnement (`/proc/1781125/environ`) :

```
OBRAIN_LLM_URL=http://192.168.1.7:9910
OBRAIN_IAM=1
```

**MESURÉ** (`GET /props`, `GET /slots`) — le service visé est :

| fait | valeur |
|---|---|
| modèle | `Qwen3.8-27B-Q4_K_M.gguf` (`model_ftype: Q4_K - Medium`) |
| moteur | `llama-server`, build `b53-39cbbb7` |
| `n_ctx` par créneau | **262 144** |
| créneaux | **4**, tous `is_processing: false` pendant la mesure |
| décodage spéculatif | **`speculative: none`** |
| gabarit de chat | présent (`Content-only`) |

Donc : **le raisonnement sur « Qwen3.8-27B » n'est pas hypothétique — c'est le
modèle servi.** Mais il l'est servi **sans DFlash2**, alors que la table de
benchmark citée est étiquetée `Qwen3.8-27B/DFlash2`.

### 0.2 Le débit réel de ce service — mesuré, et il commande tout

**MESURÉ.** Une requête par taille, `POST /v1/chat/completions`, `stream: false`,
prompt unique (aucune réutilisation), lecture de `timings` :

| invite (jetons) | prefill | sortie | decode |
|---|---|---|---|
| 210 | 57,8 tok/s | 128 | 12,80 tok/s |
| 1 532 | 737,0 tok/s | 32 | 11,10 tok/s |
| 2 226 | 526,1 tok/s | 128 | 9,78 tok/s |

**CALCULÉ.** Ce que ces chiffres impliquent sur la machine qui sert :

```
decode 11,1 tok/s × 16,2 Go (poids Q4_K_M d'un 27B) = 180 Go/s de bande passante effective
prefill 737 tok/s × 2 × 27e9                              = 40 TFLOPS effectifs
```

**DÉDUIT.** Un décodeur à ~180 Go/s et ~40 TFLOPS n'est pas un H200 (4,8 To/s,
≈990 TFLOPS BF16 dense) : c'est un profil de type **DGX Spark / GB10** (mémoire
unifiée ~273 Go/s), ce qui explique d'ailleurs le choix `n_ctx 262144 × 4`
(16 Gio de KV en BF16 tiennent dans une mémoire unifiée de 128 Go, pas dans une
HBM de 80 Go). **Le SKU exact n'est pas vérifié** — je n'ai pas d'accès à l'hôte.

**Le fait qui compte** : la table citée annonce **184–236 tok/s** en c=1. Le
service réel fait **9,8–12,8 tok/s**. Il y a un facteur ~20 entre l'analyse et le
système qu'elle prétend optimiser. Ce n'est pas un détail : cela signifie que
**la décision dominante n'est pas le KV cache, c'est le choix de la machine.**

### 0.3 Le lien qui porte les requêtes — 100 Mb/s

**MESURÉ.**

```
$ ip route get 192.168.1.7
192.168.1.7 dev enp2s0 src 192.168.1.103
$ cat /sys/class/net/enp2s0/speed   → 100
$ ethtool enp2s0 | grep "Supported link modes"
    Supported link modes: 10baseT/Half 10baseT/Full
```

Ce n'est pas un câble dégradé : **la carte ne sait faire que du 10/100**. Lien
négocié 100 Mb/s full, compteurs d'erreur à zéro (`rx_crc_errors 0`,
`rx_errors 0`, `tx_errors 0`).

**CALCULÉ** — restaurer un état de contexte de 100k jetons (6,10 Gio en BF16) par
ce lien :

```
6,10 Gio / 12,5 Mo/s = 524 s
```

à comparer au **prefill recalculé** sur la même machine : `100 000 / 737 = 136 s`.

**Conséquence, et elle est structurelle** : sur la topologie actuelle, **injecter
un état est 4× plus lent que recalculer le prefill**. Le texte à traverser fait
quelques centaines de kilo-octets (32 ms) — le réseau est dimensionné pour du
texte, et il est catastrophique pour des tenseurs. Toute conception qui fait
transiter des tenseurs entre le graphe et le moteur est morte sur ce lien.

### 0.4 L'ordre de l'invite, dans le code — le point que personne ne regarde

**CONSTATÉ.** `crates/obrain-cli/src/engine.rs:2110-2113` :

```rust
let mut messages: Vec<serde_json::Value> = vec![serde_json::json!({
    "role": "system", "content": format!("{systeme}\n\nCONTEXTE DU GRAPHE\n{texte_contexte}")
})];
messages.extend(messages_du_tour(historique, question, reprise));
```

L'extrait de graphe — **re-sélectionné à chaque tour** — est placé **dans le
message système, donc au premier jeton de l'invite**. Tout ce qui suit (l'invite
outil, l'historique entier, la question) est invalidé dès que l'extrait change.

**CONSTATÉ** de surcroît, `engine.rs:346-366` (`contexte_ajuste`) : quand le
quota manque, l'extrait est coupé **par la fin** et remplacé par un compteur qui
contient **deux nombres variables** :

```rust
format!("{}\n\n[CONTEXTE TRONQUÉ : {} caractères sur {} — la fenêtre ...]", …)
```

**CONSTATÉ**, `engine.rs:2083-2097` : `pour_contexte = fenêtre − (invite +
historique + réponse réservée + marge)`. **L'historique grandit, donc le quota
de l'extrait rétrécit à chaque tour** — le point de coupe se déplace, et avec lui
les deux nombres du compteur. La partie réutilisable de l'invite devient donc
**plus courte à chaque tour**, pendant que la partie à recalculer grandit.

**CALCULÉ.** L'invite est ré-évaluée depuis le point de coupe à chaque tour. Sur
le service réel, à 737 tok/s au mieux : un tour dont l'invite fait 100k jetons
coûte **≥136 s de prefill**, à chaque tour, pour un historique qui n'a pas changé.

C'est le défaut le plus rentable du système, et il est **gratuit à corriger** :
l'ordre des messages. Ordre stable d'abord (système, outils, historique), matière
volatile ensuite (extrait de graphe, question). Rien à écrire côté tenseurs.

### 0.5 Les DEUX modèles de la flotte sont des hybrides `qwen35` — et c'est le
nœud de tout

**MESURÉ.** J'ai lu l'en-tête GGUF du modèle local
(`/home/triviere/Qwen3.5-9B-…-KL.Q4_K_M.gguf`), sans intermédiaire :

```
general.architecture              = qwen35
qwen35.full_attention_interval    = 4        → 32 blocs = 24 DeltaNet + 8 attention
qwen35.block_count                = 32
qwen35.ssm.state_size             = 128
qwen35.ssm.group_count            = 16
qwen35.ssm.inner_size             = 4096
qwen35.ssm.conv_kernel            = 4
qwen35.attention.head_count_kv    = 4
qwen35.attention.key_length       = 256
qwen35.context_length             = 262144
```

Et le service distant est `Qwen3.8-27B`, **même architecture `qwen35`**
(EXTERNE, llama.cpp #27211 : « On Qwen3.8-27B (arch `qwen35`, 65 blocks, SSM
state size 128 across 16 groups, inner size 6144) »).

**Donc la chaîne entière — petit modèle local *et* 27B de production — est
faite de modèles hybrides à état récurrent.** Ce n'est pas un détail de
configuration : c'est **la classe d'architecture où llama.cpp documente
lui-même que la réutilisation d'état ne fonctionne pas.**

### 0.6 Incident à déclarer : la réutilisation de préfixe a tué un llama-server
— et c'est un défaut documenté, pas une fausse manœuvre

**MESURÉ, et je le déclare.** En testant la réutilisation de préfixe (deux
requêtes identiques sur le même créneau, `id_slot: 0`, `cache_prompt: true`) sur
le **service local** `:9901` (llama-server `b9222`, `qwen35` 9B Q4_K_M, iGPU
Radeon 780M, `n_parallel 4`, `kv_unified=true`, `n_ctx 8192`) :

```
la 1ʳᵉ requête aboutit : 1454 jetons évalués en 5352 ms (272 tok/s)
la 2ᵉ requête (identique) : le service meurt
journal : llama-server[3383214]: common.cpp:1489: failed to remove sequence 0
                                    with p0=1454, p1=-1
          systemd: Main process exited, code=dumped, status=6/ABRT (core-dump)
```

`p0=1454` **est** la longueur de mon invite. Le service a été **redémarré
automatiquement par systemd en 10 s** (`restart counter is at 1`) et il répond.

**EXTERNE — l'attribution, désormais solide.** Deux sources décrivent exactement
ce scénario :

- **llama.cpp PR #24003** (fermée, **non mergée**) : « For recurrent/hybrid (FULL
  seq-rm) models, disk slot save/restore currently doesn't deliver KV reuse, and
  the exact-prompt "regenerate" case **crashes the server (`GGML_ABORT`)** … This
  makes save/restore unusable for the growing class of recurrent/hybrid models
  (Mamba, Falcon-H, Jamba, Qwen3-Next, etc.). »
- **llama.cpp issue #21831** (OUVERTE depuis 2026-04-13) : « Server forces full
  prompt re-processing on subsequent requests (SWA/recurrent memory error) » —
  A/B sur build identique, **`qwen35moe` échoue (contexte perdu)**, un
  transformeur pur passe.

**Le modèle local que j'ai tué est `qwen35`, le chemin est la réutilisation d'un
prompt identique, le signal est `SIGABRT`.** L'attribution ne laisse plus de
place raisonnable au doute : ce n'est pas mon format de requête, c'est **le
défaut hybride/récurrent connu de llama.cpp**.

**Ce que cela signifie pour la proposition — c'est le point le plus important de
tout l'audit.** Toute l'économie du texte repose sur la réutilisation de
préfixe. Or, sur cette pile précise :

| fait | statut |
|---|---|
| la réutilisation de préfixe est la base de l'économie du texte | l'auteur le dit lui-même |
| llama.cpp ne la fournit **pas** de façon fiable pour `qwen35` | **EXTERNE**, #21831 ouverte, reproduite |
| le cas « prompt identique rejoué » **avorte le serveur** | **EXTERNE** (#24003) + **MESURÉ** ici |
| un système externe ne peut **pas** injecter d'état par HTTP | **EXTERNE** : #23817 fermée — « server slot save/restore feature already covered such use case » |

**Je n'ai pas reproduit sur le service 27B** : c'est un service partagé, et
l'expérience décisive (deux requêtes de préfixe identique) est **précisément
celle qui a tué le service local**. **La mesure qui déciderait de la valeur
réelle de la proposition n'a donc pas été faite** — c'est un trou assumé, pas un
oubli.

---

## 1 · Verdict global

**La thèse est juste sur son cœur, et je l'ai vérifiée chiffre par chiffre.** Le
principe — « à contexte long, le préfill domine et un état injecté le supprime » —
est **CALCULÉ et confirmé** : sur H200, un préfill de 110k jetons coûte 12–30 s de
GPU ; restaurer 6,25 Gio depuis la RAM hôte coûte **0,12 s** (mesuré : dernier
saut PCIe à 55 Go/s), depuis un SSD Gen5 **0,46 s**. Deux ordres de grandeur. **Ce
n'est pas une lubie, c'est la bonne optimisation** — et vos chiffres de modèle
(KV 64 KiB/jeton, état 144 Mio, décomposition `State(N)`) sont **exacts au byte
près**, y compris l'état de convolution que vous aviez listé.

**Mais cinq choses le bornent ou le contredisent, et aucune n'est un détail :**

1. **La dédup économise du stockage, pas de la bande passante.** C'est la
   correction qui coûte le plus cher à votre raisonnement. Le KV d'une séquence
   est **relu par cette séquence** à chaque jeton : le trafic vaut
   `B × contexte × 64 KiB`, partage ou pas. Donc 32 contextes de 100k donnent
   **21 tok/s/utilisateur, avec ou sans 80k partagé**. Le partage supprime du
   **prefill**, pas du **trafic de décodage**. Vous ne confondez pas dédup de
   stockage et dédup de calcul — vous les fusionnez.
2. **Il y a un plafond dur, et votre table le franchit.** À 100k de contexte, les
   lectures KV seules plafonnent à **732 tok/s agrégés, quel que soit le lot**.
   Votre ligne c=32 (1 525–1 952 tok/s) exigerait **2,08× à 2,67× la bande
   passante HBM d'un H200, à 100k de contexte** : ce n'est pas optimisable, c'est
   impossible. Cette ligne décrit **≈35k jetons par séquence** — ce que la table
   ne dit pas.
3. **La « machine » de l'analyse n'est pas celle qui sert.** MESURÉ : ~180 Go/s
   et ~40 TFLOPS, soit **~27× moins de bande passante** que le H200 supposé, avec
   **11 tok/s en décodage** contre les 184–236 annoncés. Et l'interconnexion fait
   **100 Mb/s**, ce qui **invalide** l'injection d'état sur la topologie actuelle
   (537 s contre 136 s de recalcul).
4. **Le mécanisme dont dépend toute l'économie est cassé pour votre modèle.**
   Les deux modèles de la flotte sont des hybrides `qwen35`, et llama.cpp
   documente lui-même que la réutilisation de préfixe y échoue (#21831, ouverte)
   et que le cas « prompt identique rejoué » **avorte le serveur** (#24003) — ce
   que j'ai **reproduit** (§0.6). Avant de concevoir un store, il faut mesurer si
   le cache existant rend quelque chose.
5. **Le défaut le plus rentable n'est pas dans la proposition, il est déjà dans
   le code** : l'extrait de graphe est au premier jeton de l'invite (§0.4). Tant
   qu'il y est, la réutilisation est au mieux fragile — et construire un state
   store pour un cache qu'on sabote gratuitement serait payer très cher pour ne
   rien voir. **Corriger l'ordre des messages est gratuit.**

---

## 2 · Section par section

### 2.1 « Ce que montre déjà la H200 » — table c1/c8/c32

**Ce qui tient.** L'arithmétique du coût est exacte :

**CALCULÉ.** `1700 tok/s × 3600 = 6,12 M jetons/h` ; `4,59 $/h ÷ 6,12 M = 0,75 $/M`
brut ; à 60 % d'occupation `1,25 $/M` ; à 40 % `1,88 $/M`. Vos trois lignes sont
bonnes au centime.

**Ce qui ne tient pas — et c'est la correction la plus lourde de cet audit.** La
table n'a **pas d'axe de longueur de contexte**, et c'est l'axe dominant. Le
décodage est borné par `trafic/étape ÷ 4,8 To/s`, et **le trafic KV se compte par
séquence, pas par bloc physique** : chaque requête relit tout son contexte à
chaque jeton (les noyaux d'attention lancent un bloc de threads par
séquence × tête, et chacun parcourt **son** KV).

**CALCULÉ**, avec les chiffres vérifiés du modèle (§2.3), `W = 16,2 Go` (poids
Q4_K_M) et `KV = 64 KiB/jeton` :

| scénario (H200, 27B Q4) | trafic/étape | ms/étape | tok/s aggr. | tok/s/util |
|---|---|---|---|---|
| 8 × 35k | 34,6 Go | 7,2 | 1111 | 138,9 |
| 16 × 35k | 52,9 Go | 11,0 | 1452 | 90,7 |
| **32 × 35k** | 89,6 Go | 18,7 | **1714** | **53,6** |
| 12 × 100k | 94,8 Go | 19,8 | 607 | 50,6 |
| **32 × 100k** | 225,9 Go | 47,1 | **680** | **21,2** |
| plafond **dur**, 100k, lectures KV seules | — | — | **732** | 0 |

La ligne 32 × 35k **reproduit votre ligne c=32** (1525–1952 tok/s agrégés,
48–61 tok/s/utilisateur). Trois conséquences, dont une qui contredit votre thèse :

1. **Votre table décrit ≈35k jetons par séquence.** C'est une information utile,
   pas un défaut — mais elle la cachait, et c'est elle qui rend la table
   inutilisable hors de ce régime.
2. **Le plafond est dur et il ne s'améliore pas en batchant.** À 100k de
   contexte, les lectures KV seules plafonnent à
   `4,8e12 ÷ (100 000 × 65 536) = 732 tok/s agrégés`, quel que soit le lot. Donc
   **1 525 tok/s à 100k exigerait 9,99 To/s, soit 2,08× la bande passante HBM
   d'un H200** (1 952 tok/s → 2,67×) : **physiquement impossible**, pas
   « optimisable ».
3. **Et le partage de préfixe n'augmente PAS le débit.** C'est le point que
   votre texte affirme (§2.5, §2.14) et que la mesure du noyau contredit : à
   100k de contexte, 32 séquences partageant 80k donnent **le même
   21,2 tok/s/utilisateur** que 32 séquences sans rien partager. La dédup
   économise de la **mémoire**, pas de la **bande passante** — parce que les
   octets sont partagés en stockage mais **relus par chaque séquence**.

**Ce que votre idée achète, exactement.** La dédup divise l'empreinte (4,44×),
donc elle permet de **garder plus de contextes résidents** — et plus de
concurrence fait monter le débit **agrégé** vers l'asymptote (680 → 732 tok/s)
en faisant **baisser** la vitesse par utilisateur. Autrement dit : **la dédup
achète de la capacité, pas de la vitesse.** C'est un vrai gain — 32 contextes de
100k ne tiennent pas dans 141 Go sans elle — mais ce n'est pas celui qui est
annoncé. Pour 50 tok/s/utilisateur à 100k de contexte, la limite est **12
séquences** par H200, partage ou pas.

**Ce qui manque.** La variable qui décide n'est pas la concurrence, c'est
`(longueur de contexte, débit par utilisateur visé)`. Toute table de tarification
construite sur la seule concurrence sera fausse dès que les contextes grandissent.

**Note sur la table elle-même.** EXTERNE : le triplet 184–236 / 1 090–1 368 /
1 525–1 952 **ne se rattache à aucune source publique identifiable**, et sans la
quantification il est ininterprétable : en BF16 un 27B dense plafonne à 88,9
tok/s, donc 184–236 tok/s est **incompatible avec du BF16** et n'est cohérent
qu'avec du ≤4 bits, un MoE, ou du décodage spéculatif. Bonne nouvelle : puisque
le modèle servi est **Q4_K_M** (plafond 355,6 tok/s), des valeurs de 184–236
tok/s sont **cohérentes** (52–66 % du plafond). **Il faut donc écrire la
quantification et la longueur de contexte à côté de ces chiffres — sans quoi ils
ne veulent rien dire.**

### 2.2 « Le truc qui m'intéresse vraiment » — l'état persisté

**Ce qui tient, et c'est le point fort de la proposition.** **CALCULÉ** :

| | valeur |
|---|---|
| prefill 110k jetons, H200, 50 % MFU | **12,0 s** |
| prefill 110k jetons, H200, 20 % MFU | 30,0 s |
| restauration de 6,10 Gio depuis la RAM hôte (~50 Go/s) | **0,1 s** |
| restauration depuis NVMe (1 SSD Gen5, 14,5 Go/s mesuré) | **0,46 s** |
| recalcul du préfixe sur le **service réel** (737 tok/s) | **136 s** |

Le rapport « contexte recalculé / contexte restauré » est bien de deux ordres de
grandeur, et le fait que le préfill soit **compute-bound** alors que le decode
est **bandwidth-bound** est correctement identifié. La phrase « ce n'est pas
qu'une optimisation de latence, c'est une optimisation de capacité GPU » est
juste : du préfill supprimé, ce sont des secondes de GPU rendues au decode
batché. **La borne, en revanche, doit être dite** : au-delà de ~2 300 jetons de
contexte le decode est lui-même limité par la bande passante KV (§2.1), donc le
gain de capacité est **borné par la part du préfill dans le temps GPU total** —
à 110k d'entrée pour 500 de sortie, c'est environ la moitié, pas un facteur libre.

**Ce qui manque.** Le raisonnement omet **la bande passante de l'axe de
restauration**, qui est l'axe réel. **EXTERNE** (mesures publiées, pas des
estimations) pour 6,25 Gio :

| chemin de restauration | débit | temps |
|---|---|---|
| RAM hôte → HBM, dernier saut PCIe Gen5 d'**un** GPU | **55 Go/s mesuré** (63 Go/s brut) | **0,12 s** |
| 1 SSD Gen5 entreprise (Solidigm D7-PS1010, mesuré) | 14,5 Go/s | **0,46 s** |
| nœud client classe MLPerf Storage v3.0 | 41,2 Go/s | 0,16 s |
| baie 8 disques effective | ~58 Go/s | 0,12 s |
| 10 GbE | 1,25 Go/s | 5,4 s |
| 2,5 GbE | 0,31 Go/s | 21,0 s |
| 1 GbE | 0,125 Go/s | 53,7 s |
| **100 Mb/s (MESURÉ ici)** | **0,0125 Go/s** | **536,9 s** |

**Deux faits à retenir, tous deux contraires à l'intuition du schéma :**

1. **Pour un seul GPU, la baie de disques cesse d'être le goulot dès 2–3
   SSD** : le dernier saut PCIe est un plancher incompressible de **~122 ms**
   pour 6,25 Gio. Ajouter du NVMe n'accélère plus rien au-delà.
2. **Le NVMe peut être *plus rapide* que la RAM hôte** — 0,46 s contre 0,12 s
   reste dans le même ordre. Autrement dit, dans le régime qui nous intéresse,
   les paliers « chaud/froid » se ressemblent **beaucoup plus** que le schéma à
   trois températures ne le suggère, et le vrai décrochage n'arrive qu'au
   **réseau**.

**Règle qui en découle : le state store doit vivre sur la machine du modèle, pas
sur celle du graphe.** Chez vous, aujourd'hui, il vivrait à 100 Mb/s de distance,
et l'injection serait **4× plus lente que le recalcul du prefill**.

### 2.3 « Pourquoi Qwen3.8 est particulièrement adapté » — les chiffres du modèle

**C'est la section la mieux vérifiée, et elle est exacte.** **EXTERNE** (config.json
officiels) :

- `Qwen/Qwen3.8-27B` **existe** (créé 2026-08-05, apache-2.0, 27B **dense**, VL).
- 64 couches, `layer_types` = 48 × `linear_attention` + 16 × `full_attention`,
  `full_attention_interval 4` → bien `16 × [D, D, D, A]`.
- `num_key_value_heads 4`, `head_dim 256`, `num_attention_heads 24`,
  `partial_rotary_factor 0.25`, `hidden_size 5120`, contexte natif **262 144**.
- Gated DeltaNet : `linear_num_key_heads 16`, `linear_num_value_heads 48`,
  `linear_key_head_dim 128`, `linear_value_head_dim 128`, `linear_conv_kernel_dim 4`.
- **`mamba_ssm_dtype: "float32"`** — confirmé, dans tous les configs Qwen3.5/3.6/3.8
  (absent de Qwen3-Next, contrairement à ce qu'une lecture rapide suggérerait).
- Le nom des couches vient bien de **Gated Delta Networks** (Yang, Kautz,
  Hatamizadeh, NVIDIA, arXiv:2412.06464, ICLR 2025).

**Et vos deux formules sont justes au bit près. CALCULÉ** :

```
KV   = 16 couches × 4 têtes KV × 256 dim × (K+V) × 2 o = 65 536 o = 64 KiB/jeton ✅
état = 48 couches × 48 têtes V × 128 × 128 × 4 o       = 144 MiB                ✅
croisement : l'état domine le KV en dessous de 2 304 jetons
à 262 144 jetons : KV = 16,00 Gio, soit 114× l'état
```

Vos tables de volume (10k → 625 Mio, 100k → 6,10 Gio, 262k → 16,0 Gio, FP8 la
moitié) sont exactes.

**Et la décomposition de votre état est exacte au byte près — vérifiée contre une
mesure indépendante.** **EXTERNE** (llama.cpp #27211, `qwen35` 27B) : « one
checkpoint's recurrent plane is **149,6 MiB** ». Or :

```
état récurrent (matrice Delta) = 48 × 128 × 6144 × 4     = 144,00 MiB   ← votre chiffre
état de convolution            = 48 × 3 × (6144+4096) × 4 =   5,63 MiB   ← votre « convolution_state »
                                                    total = 149,63 MiB   ← celui de llama.cpp
```

**Votre `State(N) = {KV, DeltaNet state, conv state, position, …}` est donc
exact, et l'état de convolution n'est pas un détail : il pèse 3,9 % et il est
indispensable** (un état DeltaNet sans son état de convolution ne se reprend
pas). C'est la meilleure partie de votre analyse.

**Conséquence pratique, en revanche, votre chiffre de « ~250 Mio par slot » est
faux, et dans le sens qui coûte de l'argent :** llama.cpp garde **32 checkpoints
par créneau** par défaut (`--ctx-checkpoints 32`). La vraie grandeur n'est donc
pas ~150–250 Mio **par checkpoint**, mais **≈ 4,7 Gio par créneau** (EXTERNE,
#27211 : « a single slot at the default limit holds about 4.7 GiB, and that
scales linearly with slots »). Sur 4 créneaux, ~19 Gio de checkpoints.

**Une seule erreur de lecture, mais elle compte** :
« DFlash2 » n'est **pas** une quantification.

**EXTERNE** : DFlash 2 est un **modèle brouillon de décodage spéculatif** par
diffusion de blocs — 5 couches, 1,92 G, rattaché au 27B (arXiv:2602.06036, ICML
2026 ; intégrations llama.cpp #27342, vLLM #52816). Les dépôts `…-DFlash2-Q4_K_M.gguf`
quantifient **le brouillon**, pas le 27B. Or **le service déployé est
`speculative: none`** (§0.1) : votre table de référence décrit donc un
**mécanisme absent du système réel**. Comparer les deux n'a pas de sens tant que
ce n'est pas aligné.

### 2.4 « Voilà où ton système pourrait devenir puissant » (110k + 500)

**Le rapport 221× est un rapport de travail, pas de temps — vous le dites, et
c'est honnête.** Ce que la section ne dit pas : le préfill est **massivement
parallèle** et le decode non. **CALCULÉ**, dense 27B :

```
prefill 110k : 5,94 PFLOP ÷ 495 TFLOPS (50 % MFU) = 12,0 s
decode  500  : 500 ÷ 50 tok/s                     = 10,0 s
```

Le rapport en temps mural est donc **≈1:1**, pas 221:1. Cela ne détruit pas la
thèse — au contraire, cela la **renforce** : si le préfill coûte autant que la
réponse entière, alors le supprimer est l'optimisation la plus rentable du
système. Mais l'argument doit être présenté ainsi, pas avec 221×.

### 2.5 « Ça rejoint ta mutualisation H200 » (32 agents, P et D se battent)

**Le diagnostic est bon, la solution proposée n'est pas la bonne pour ce
problème.** « Prefill et decode se battent pour le GPU » a déjà deux réponses
déployées en production, qui ne demandent **aucun état persisté** — et la
distinction entre les deux est décisive :

- le **prefill chunké** (Sarathi-Serve, arXiv:2403.02310, OSDI'24) découpe le
  prefill en morceaux et produit des ordonnancements « stall-free » qui
  « ajoutent de nouvelles requêtes à un lot **sans interrompre les décodes en
  cours** ». **C'est une mitigation, pas une élimination** : prefill et decode
  restent sur le **même GPU**, dans le même lot ; la compétition pour les SM et
  la bande passante demeure. Gains mesurés : 2,6× (Mistral-7B/1×A100) à 5,6×
  (Falcon-180B).
- la **désagrégation prefill/decode** (DistServe, arXiv:2401.09670 ; Splitwise,
  arXiv:2311.18677 ; Mooncake, arXiv:2407.00079 en production chez Kimi) :
  « assigns prefill and decoding computation to **different GPUs**, hence
  **eliminating** prefill-decoding interferences ». **C'est celle qui élimine**,
  au prix du transfert de KV sur le réseau, d'un pool de ressources séparé, de
  deux plans de parallélisme et d'un moteur de transfert (Mooncake, NIXL).
  Livrée : SGLang (`--disaggregation-mode prefill|decode`, avec un **canal dédié
  pour l'état hybride** conv/SSM/SWA, transféré « atomically »), vLLM
  (chunked prefill toujours actif en V1 + Disaggregated Serving), NVIDIA Dynamo.

L'état persisté apporte autre chose (TTFT, capacité, coût), mais il ne faut pas
le vendre comme la correction de l'interférence P/D : **elle est corrigée, et
gratuitement, par des mécanismes que votre runtime connaît déjà** — et si l'on
veut vraiment éliminer la compétition, la désagrégation est le chemin connu,
livré, et documenté.

### 2.6 « Un deuxième niveau » — Merkle-DAG d'états adressables

**La direction est la bonne ; l'un des mécanismes est faux, et le faux est un
piège de correction.**

**Le piège. CONSTATÉ/CALCULÉ.** Le KV d'un bloc de jetons `[a, b)` **dépend de
tout le préfixe `[0, b)`**, pas des jetons du bloc :

- l'attention est causale : l'état caché du jeton `t` dépend de tous les jetons `≤ t` ;
- RoPE est **absolu** : la même suite de jetons à une position différente donne
  un `K` différent.

Donc un bloc KV **n'est pas adressable par son propre contenu**. La clé ne peut
être que le **chemin de Merkle** (le préfixe entier). Conséquence directe : **la
dédup se réduit exactement au partage de préfixe identique depuis la position 0**,
ni plus ni moins. Le « content-addressed block store » n'apporte alors **aucune
dédup supplémentaire** — il apporte de la **granularité de transfert et
d'éviction**, ce qui est utile, mais ce n'est pas ce qui est annoncé. La
`StateRef` que vous esquissez (avec `parent_state`) est correcte ; c'est le
schéma `KVBlock #a83f tokens 0..63` qui induit en erreur.

**Ce qui en découle pour « le graphe comme index d'exécution ».**
L'identité utilisable par un ordonnanceur est la **suite de jetons exacte**.
Deux nœuds « sémantiquement proches » dans obrain ne partagent **aucun** KV. Le
graphe ne peut donc pas être l'index d'exécution du GPU : cet index est un
radix/trie sur les identifiants de jetons. Ce qu'obrain apporte légitimement :
le **cycle de vie**, la **propriété**, l'**ACL**, le **nommage**, la **politique
d'éviction**. C'est beaucoup — mais ce n'est pas l'ordonnancement.

**Le point qui rend le reste vrai.** Votre phrase « plus obrain trouve de
préfixes communs, moins le stockage augmente avec le nombre d'utilisateurs » est
juste — **à condition de fabriquer ces préfixes communs**. Ils ne se trouvent
pas, ils se **conçoivent** : invite système canonique, schémas d'outils
canoniques, sérialisation de graphe canonique, ordre stable. C'est un problème
de **politique**, pas de stockage — et c'est précisément là qu'obrain a un actif
que les autres n'ont pas.

### 2.7 « Cela peut modifier ton modèle tarifaire »

**La décomposition `C_request = C_prefill_miss + C_state_restore + C_decode +
C_memory` est saine, et l'idée `B << A` (entrée en cache bien moins chère) est
la bonne.**

**Le piège de marge, qu'il faut voir avant de publier un tarif.** Un préfixe en
cache coûte ~0 de **prefill** — mais il coûte **sa traversée à chaque étape de
decode**. **CALCULÉ** : 100k jetons en cache à 32 séquences, c'est **47 Go de
trafic HBM par étape**, soit 13 ms, soit 75 tok/s/utilisateur. Un tarif qui ne
distingue que « miss » et « hit » facture au même prix un hit de 2k et un hit de
100k, alors que le second coûte **50× plus cher en bande passante**. Le prix doit
dépendre de **longueur × étapes**, pas seulement de hit/miss. C'est le genre
d'erreur qui ne se voit pas au lancement et qui vide la marge trois mois plus tard.

### 2.8 « Exemple économique H200 »

**Exact, à une réserve près.** `1700 tok/s → 6,12 M tok/h → 0,75 $/M` ✅, et le
raisonnement d'occupation (60 % → 1,25 ; 40 % → 1,88) ✅. La réserve : ces trois
chiffres supposent implicitement un contexte de **≈35k par séquence** (§2.1). À
100k, le débit tombe à **680 tok/s agrégés** quel que soit le partage, soit
`4,59 / 2,448 = ` **1,88 $/M brut** au lieu de 0,75 — et le plafond dur (732 tok/s)
interdit d'espérer mieux par lots. **Le coût par million ne baisse donc pas quand
le contexte grandit : il monte.** C'est l'inverse de l'argument de vente, et il
faut le savoir avant de publier une grille tarifaire.

### 2.9 « Il y a même un sweet spot » — paliers FAST/INTERACTIVE/ECONOMY/BACKGROUND

**Solide et directement réalisable — c'est la partie la plus opérationnelle du
texte.** Un ordonnanceur qui choisit le lot en fonction du SLO annoncé est une
vraie différenciation, et elle ne dépend d'aucune recherche : llama-server expose
déjà `n_parallel`, SGLang/vLLM exposent la concurrence maximale.

**Une réserve honnête** : le palier « 40–60 tok/s » que vous appelez ECONOMY est
aujourd'hui le **meilleur cas mesuré** de votre service réel (9,8–12,8 tok/s).
Les paliers se dessinent sur H200, pas sur l'existant.

### 2.10 « Et llama.cpp dans tout ça ? »

**Votre instinct est bon, votre justification d'unicité ne l'est pas, et la
difficulté réelle est ailleurs que là où vous la placez.**

- **C'est déjà votre moteur** : `OBRAIN_LLM_URL` → llama-server (§0.1). Il n'y a
  pas de choix à faire, il est fait.
- **Les primitives existent déjà, et sous des noms précis** (EXTERNE, `llama.h`
  et `tools/server/README.md`) : `llama_state_seq_get_data`,
  `llama_state_seq_set_data`, `llama_state_seq_save_file`,
  `llama_state_seq_load_file`, plus le drapeau
  `LLAMA_STATE_SEQ_FLAGS_PARTIAL_ONLY` (« SWA KV cache or **recurrent cache
  (e.g. Mamba)** »). Côté serveur : `--slot-save-path` + `POST
  /slots/{id}?action=save|restore|erase`, `--cache-ram` (défaut 8 Gio),
  `--cache-prompt` (activé par défaut), `--cache-reuse N` (**désactivé par
  défaut**), `--slot-prompt-similarity` (0,10), `--ctx-checkpoints` (32).
  « Tester ce que la plupart des moteurs ne proposent pas comme abstraction
  publique » est donc **inexact** : c'est exposé, documenté, et en partie activé.
  À noter : **`--prompt-cache` n'existe pas** dans llama-server (il ne concerne
  que l'exemple `completion`), ce qui est une source d'erreur fréquente.
- **Ce qui manque vraiment, et ce n'est pas un hook** : un **endpoint HTTP pour
  injecter un état arbitraire** dans un créneau vivant. Il n'existe pas — et la
  demande a été **explicitement refusée** par le mainteneur (EXTERNE, #23817
  fermée : « don't send us slop issues … **server slot save/restore feature
  already covered such use case** »). Donc « Obrain appelle `state_restore(id)` »
  n'est pas une intégration : c'est **un patch de llama.cpp**, donc un fork à
  maintenir. C'est possible — c'est même l'argument « tu contrôles toute la
  stack » — mais il faut l'assumer comme tel.
- **Et le hook est cassé pour votre architecture** : voir §0.6. La réutilisation
  de préfixe **avorte** un `b9222` sur `qwen35`, et llama.cpp documente que le
  save/restore disque ne rend pas le KV pour les modèles hybrides (#24003,
  #26004, #28074 — aucune mergée). **Le mécanisme sur lequel repose toute
  l'économie de la proposition est, pour votre modèle, non pas lent : non
  fonctionnel.**

### 2.11 « Un agent crée 20 hypothèses » — le fork

**Cet exemple est à l'envers, et c'est le point le plus utile de tout l'audit.**

**CALCULÉ**, 20 branches depuis un préfixe de 80k :

```
par branche : KV(80k) 4,88 Gio + état récurrent 144 Mio = 5,02 Gio
20 branches : 100,5 Gio   (+ poids 16,2 Go)
```

Sur 141 Go de HBM, **le fork matériel est impossible** — et il est impossible
*par nature* : les branches divergent immédiatement, donc leurs KV divergent
immédiatement, donc rien n'est partageable après le point de fork.

**Et « copy-on-write » est un mot impropre ici — la meilleure preuve vient de
SGLang, qui a implémenté exactement cette primitive.** EXTERNE (blog LMSYS
Unified Radix Cache, 2026-08-11) : « MAMBA provides checkpoint reuse. It requires
one recurrent checkpoint at the reusable frontier and **copies shared state into
a private request slot before mutation**. » Le blog PyTorch dit de même : « It
needs to **fork a checkpoint** of Mamba state from a request. » **Il y a bien
copie, pas partage** : l'état récurrent est mutable, donc un fork paie
immédiatement la copie complète. Seul le KV d'attention est réellement
copy-on-write (blocs immuables indexés par position).

**Or, si les 20 hypothèses sont générées en même temps, le cache radix partage
le préfixe automatiquement et rien n'a besoin d'être matérialisé.** 4,88 Gio une
seule fois, plus 20 queues courtes.

**Et la conclusion de SGLang borne sérieusement le scénario « 10 000 agents » :**
EXTERNE (PR #28185, mergée) : le pool d'états récurrents est dimensionné sur
l'ensemble *en cours*, pas sur le cache ; mesuré sur Qwen3.5-35B-A3B, 4×H20,
`--max-mamba-cache-size 256` → `reuse_frac` **0,940 à K=256 préfixes distincts,
0,020 à K=384, 0,000 à K=640**. Autrement dit : **le cache d'états récurrents
s'effondre dès quelques centaines de préfixes distincts**, ce qui est trois
ordres de grandeur sous les « 100 M jetons » de votre §13. L'état récurrent
n'est pas du KV : il ne se met pas en cache à l'échelle comme du KV.

**Donc** : le fork est une primitive **temporelle** (reprendre demain, changer de
machine, redémarrer un moteur), **pas** une primitive de concurrence. Votre
exemple illustre le cas où l'état persisté est **inutile**, alors qu'il est
présenté comme son meilleur argument. Savoir trier ces deux cas vaut plus que
tout le reste de la proposition.

### 2.12 « Il y a toutefois un verrou technique important »

**Cette section est juste, c'est la bonne intuition, et le verrou est encore
plus serré que vous ne le dites.** Vous nommez : capturer/restaurer de façon
déterministe le KV des 16 couches d'attention **+** les états récurrents des 48
couches DeltaNet **+** les positions, avec compatibilité version / quantification
/ noyaux. Quatre précisions, toutes sourcées :

1. **Le déterminisme bit-à-bit n'est pas garanti, et llama.cpp le dit.** EXTERNE,
   doc serveur de `cache_prompt` : « Because (depending on the backend) the
   logits are **not** guaranteed to be bit-for-bit identical for different batch
   sizes (prompt processing vs. token generation) enabling this option can cause
   nondeterministic results. » Confirmé par #23212 (mêmes modèle, graine, prompt
   → sorties différentes entre deux générations de matériel Apple ; réponse de
   ggerganov : déterministe **sur le même matériel**, pas entre machines). Votre
   exigence « parfaitement déterministe » est donc un **travail à faire**, pas une
   propriété acquise — et c'est exactement ce qui rend `state_merge` impossible,
   comme vous l'avez vu.
2. **L'état récurrent ne se rembobine pas** — c'est structurel, pas un bug :
   EXTERNE, source llama.cpp pour DSV4 : « the compressed state **cannot be
   rolled back** within that window. Report only the current boundary so
   server-context uses checkpoints for rollback. » Conséquence : **les
   checkpoints ne sont pas reconstructibles après coup — ils doivent voyager
   avec le fichier d'état** (PR #26004, ouverte). Votre intuition « checkpoints
   clairsemés » est donc la seule architecture possible, pas une optimisation.
3. **Toute mise à jour du modèle invalide 100 % du store.** La clé contient
   `model_version` — donc re-quantifier ou changer de noyau jette l'intégralité
   du cache. Ce n'est pas un cache, c'est un **cache à durée de vie courte** : sa
   valeur espérée doit être actualisée par le rythme de changement de version.
4. **Le fork d'un état récurrent n'est pas un partage** (§2.11), et **le lien**
   (§0.3) est le verrou que je mesurerais **en premier** : il peut annuler tout
   le reste sur la topologie actuelle.

**Et le verrou que vous placez le plus haut — « garantir la compatibilité avec
version du modèle, quantification et kernels » — est déjà en échec sur votre
pile, aujourd'hui.** Ce n'est pas un risque de conception : c'est un bug ouvert
(#21831) et un abandon de serveur reproduit (#0.6). **La première action de ce
chantier n'est pas de concevoir le store, c'est de mesurer `timings.cached_tokens`
sur deux tours réels du chat obrain.**

### 2.13 « Séparer la taille du graphe logique de celle du state store »

**Exact, et c'est la meilleure section du texte.** Le tableau KV (10k → 625 Mio …
100 M → 5,96 Tio), l'absurdité du snapshot par jeton (100 000 × 144 Mio =
13,73 Tio), et le refus de `Node → tenseur` sont justes. **CALCULÉ** : 424 247
nœuds × 144 Mio = **58,3 Tio** ; × 250 Mio = **101,1 Tio**. Conclusion correcte :
`sessions → StateManifest → hash → blob`.

**Un point à rectifier, et il simplifie tout** : la cardinalité réelle du state
store n'est **pas** celle du graphe. C'est `O(sessions × points de fork)`. Le
424k nœuds n'ont jamais été le risque — l'exemple est un épouvantail qui se
réfute tout seul. **Les deux échelles sont découplées par construction**, pas par
dédup, et c'est encore mieux que ce qui est annoncé.

### 2.14 « 32 utilisateurs × 100k » — le calcul central

**L'arithmétique est exacte de bout en bout. CALCULÉ** :

```
sans partage : 3,2 M jetons × 64 KiB = 195,3 GiB   ✅ (votre 195,3)
80k partagé  : 720k jetons × 64 KiB  =  43,9 GiB   ✅ (votre 43,9)
facteur de dédup                     =  4,44×      ✅
+ 32 états DeltaNet (4,5 Gio) = ~48,4 Gio          ✅
poids 51 Go + 48,4 = ~100 Go sur 141 Go            ✅ (arithmétique)
```

**Ce qui manque est le débit, pas la place.** « Le scénario devient concevable »
sur le plan **mémoire** est vrai : 43,9 Gio de KV dédupliqué **tient** dans
141 Go là où 195,3 Gio ne tiennent pas. Mais sur le plan **bande passante**, la
même configuration donne **21,2 tok/s/utilisateur, partage ou pas** (§2.1) — parce
que la dédup économise du **stockage**, et que le **trafic** reste
`B × contexte × 64 KiB` par étape. Votre conclusion « ~100 Go sur 141 Go, donc
c'est jouable » est une conclusion de **capacité** ; elle ne dit rien de la
vitesse, qui est la contrainte réellement active. **Le plafond dur à 100k de
contexte est 732 tok/s agrégés pour toute la carte.**

**Et un point de topologie** : chez vous, ces 43,9 Gio ne résident pas sur la
machine du graphe. Ils doivent vivre sur celle du modèle, sinon c'est 100 Mb/s
(§0.3).

### 2.15 « On peut faire encore beaucoup mieux » — HBM / RAM / NVMe

**La hiérarchie à trois températures est la bonne, et c'est la direction de
l'industrie.** Deux précisions :

- **Le NVMe est un palier de capacité, pas de débit.** Un seul disque Gen5
  (14,5 Go/s mesuré) sert **2,2 contextes de 100k par seconde** et rien de plus ;
  et **pour un seul GPU, la baie cesse d'être le goulot dès 2–3 disques** — le
  dernier saut PCIe impose un plancher de **122 ms** pour 6,25 Gio. La
  restauration froide reste très rentable par rapport au préfill (**0,46 s**
  contre 12–136 s), mais elle **plafonne le débit** que le stockage peut
  soutenir. Cette asymétrie (latence ✅ / débit ❌) n'apparaît pas dans le schéma.
- **Le chiffre de 98 % de réutilisation de préfixe est mal attribué — je l'ai
  vérifié.** EXTERNE : il vient du blog LMSYS *Unified Radix Cache* du
  2026-08-11, où il désigne un **taux de succès du palier L3 de HiCache sur un
  multi-tour synthétique** (« On DeepSeek-V4-Flash, **L3 keeps the hit rate near
  98%** »), sur `bench_multiturn.py` — 4×H200 TP4, 48 clients, 60 tours,
  4096 jetons d'entrée par tour. **Ce n'est ni un agent de code, ni un chiffre
  SGLang sur agents de code.**
  Les mesures SGLang sur de vrais agents de code disent l'inverse : EXTERNE,
  issue #35129 (trafic réel **Claude Code**, 8×H20, HiCache) — les longues
  sessions agentiques obtiennent **`#cached-token: 0` à chaque tour**, le taux
  pondéré par jeton sur 5 h de trafic réel est **83,8 %**, et c'est le taux des
  **requêtes courtes** qui atteint ~98 %. Le blog HiCache original donne, sur un
  agent de code Qwen3-Coder-480B, **40 % → 80 %**. Les chiffres « 85–97 %,
  97,2 % en équipe » qui ressemblent à la formulation citée viennent de **NVIDIA
  Dynamo**, et **mesurés sur de l'infrastructure d'API managée** (donc Anthropic),
  pas de SGLang.
  **À remplacer par votre propre taux, mesuré — et il est probablement bas, pour
  la raison du §0.6.**

### 2.16 « Là où je modifierais Obrain » — blocs 16–64 jetons, checkpoints clairsemés

**Les tailles sont exactes. CALCULÉ** : 16 jetons = 1 Mio (1 MiB) en BF16,
512 Kio en FP8 ; 64 jetons = 4 Mio / 2 Mio. ✅

**Deux réserves de conception :**

1. **Ne pas confondre granularité de pagination et identité de cache.** Le
   découpage en blocs sert à **gérer/évincer** la mémoire ; l'**identité** d'un
   cache est un chemin de préfixe (§2.6). Un bloc partagé doit partir à la même
   position et être plein — un bloc partiel casse le partage. C'est vrai dans
   vLLM comme dans SGLang, et cela mérite d'être écrit dans la conception.
2. **Checkpoints clairsemés : votre liste de points est la bonne** (tours,
   frontières d'outils, forks, frontières de préfixe partagé, reprises
   probables). Ajoutez `HEAD` comme **seul point obligatoire**, et traitez les
   autres comme une politique d'éviction, pas comme une structure.

### 2.17 « Et avec la taille actuelle d'Obrain »

**Le calcul est exact (58,3 Tio / 101,1 Tio) et la conclusion est la bonne.**
Voir §2.13 : le problème posé n'existe pas, parce que la cardinalité est celle
des sessions, pas celle des nœuds.

### 2.18 « Mon ordre de grandeur pour une vraie installation »

**Raisonnable en ordre de grandeur, avec une correction et une omission.**

| poste | votre chiffre | remarque |
|---|---|---|
| graphe/index | 5–20 Go | **CONSTATÉ** : votre `runtime-dev` est à ~400k nœuds, l'ordre tient |
| Merkle/radix | < 1–10 Go | ✅ (1 M de nœuds × 150–250 o = 150–250 Mo) |
| états chauds H200 | 30–70 Go | ⚠️ dépend du modèle : Q4_K_M fait 16,2 Go, un BF16 en ferait 54 |
| RAM chaude | 256–512 Go | ✅ |
| NVMe | 2–8 To | ✅ |

**CALCULÉ** : 4 To à 32 Kio/jeton ≈ **122 M jetons** (votre 130 M est proche,
l'écart vient de l'arrondi de 32 vs 34 Kio). ✅

**L'omission** : la ligne **réseau** n'est pas dans le tableau, et c'est mesuré
chez vous à **100 Mb/s** (§0.3). Un tableau de dimensionnement qui oublie l'axe
par lequel passent les octets est incomplet.

### 2.19 « Le DAG mondial » et « le scheduler piloté par le graphe »

**La première moitié est juste ; la seconde est à retirer.**

- ✅ « Plus obrain trouve de préfixes communs, moins le stockage augmente avec
  les utilisateurs » — vrai, et c'est le cœur de la valeur.
- ❌ « Le scheduler regrouperait les requêtes selon leur proximité de state/prefix
  *du graphe* pour maximiser le cache-hit » : **la proximité dans le graphe
  n'augmente pas les cache-hits.** Le regroupement doit se faire sur le
  **préfixe de jetons** — ce que fait déjà un ordonnanceur radix-aware. Le graphe
  n'apporte rien ici.
- ✅ La version *correcte* de cette idée est plus forte et plus simple :
  **obrain choisit ce qu'il met dans la fenêtre, donc obrain peut fabriquer les
  préfixes communs.** C'est un levier réel, gratuit, et que personne d'autre ne
  peut actionner — parce que personne d'autre ne possède la sémantique du contexte.
- ⚠️ **Le partage entre locataires est une surface d'attaque mesurée, pas
  théorique — et c'est la contrainte qui borne le plus durement votre facteur
  4,44×.** Voir §2.20 : elle mérite sa propre section.

### 2.20 Le partage de cache est un canal auxiliaire — et la parade officielle
détruit la dédup

**Ce n'est pas une précaution de principe : c'est un domaine publié, avec des
attaques nommées et une CVE.** EXTERNE :

- **PROMPTPEEK** — « I Know What You Asked: **Prompt Leakage via KV-Cache Sharing
  in Multi-Tenant LLM Serving** » (SUSTech + ByteDance, **NDSS 2026**) :
  reconstruction des invites d'autres utilisateurs en sondant le cache KV partagé
  de SGLang/vLLM.
- **InputSnatch** (arXiv:2411.18191) — vol des entrées par canal temporel sur le
  partage de cache.
- **« Auditing Prompt Caching in Language Model APIs »** (ICML 2025,
  arXiv:2502.07776) — la simple variation de latence suffit à inférer les invites
  d'autrui ; mitigation recommandée : **isolation du cache par utilisateur**.
- **« Governing the KV Cache »** (arXiv:2608.09225) énumère PROMPTPEEK,
  EarlyBird et InputSnatch, « jusqu'à 100 % de succès contre vLLM et SGLang non
  protégés ».

**Et la documentation officielle de vLLM confirme, en citant sa propre CVE**
(EXTERNE, page Security, section « Prefix Cache Timing Side-Channel Mitigation
(Cache Salting) ») :

> « Prefix caching reuses KV cache blocks across requests that share a common
> prompt prefix. In multi-tenant deployments, that reuse is a timing side channel
> (**CVE-2025-46570**). »
> « Research has shown this signal is nearly perfectly distinguishable
> (**ROC AUC of 0.99**) at prefix lengths of just **8 tokens**. »
> « **Multi-tenant deployments**: Set `cache_salt` on every request, using a
> secret scoped to the tenant boundary you want to enforce. »
> « **Single-tenant deployments**: Cache salting is unnecessary and can be
> omitted to maximize cache hit rates. »

**Traduction en décision d'architecture, et c'est sans appel :**

1. **Le sel est mélangé au hachage du premier bloc KV** : deux requêtes de sels
   différents **ne partagent aucun bloc**. Donc **la parade officielle au canal
   auxiliaire supprime exactement la dédup inter-locataires.** Votre « DAG
   mondial d'états de calcul » n'est pas interdit — il est **borné au périmètre
   d'un locataire**, et cette borne est décidée par un mécanisme de sécurité, pas
   par un choix de stockage.
2. **Le signal est distinguable dès 8 jetons de préfixe partagé** (ROC AUC 0,99).
   Autrement dit : **le simple fait qu'un cache hit soit plus rapide qu'un miss
   fuit de l'information.** Un préfixe commun de 80 000 jetons n'est pas
   nécessaire pour fuiter — 8 suffisent. La « localité HBM » que vous voulez
   optimiser **est** le canal.
3. **Chez vous, la question n'est pas hypothétique.** obrain porte déjà des
   tenants (`OBRAIN_IAM_ACCOUNT`, acteurs, politiques) et le dépôt a fait de la
   confidentialité du calcul une propriété structurelle
   (`docs/ADR-2026-09-15-GARANTIE-CONFIDENTIALITE-CALCUL.md`). Un state store
   partagé entre agents de locataires différents **contredirait un ADR existant**
   s'il n'est pas salé. À l'inverse, en **mono-locataire** (vos propres agents,
   votre flotte), le salage est explicitement inutile et **la dédup est
   légitime** — c'est le régime où votre idée fonctionne.
4. **Ce que cela implique pour le chiffre de 4,44×** : il est atteignable
   **à l'intérieur d'un locataire**, jamais entre locataires. Le multi-locataire
   n'améliore pas la dédup : il la **plafonne**.

---

## 3 · Ce qui est mieux, par ordre de rentabilité

1. **Mesurer la réutilisation réellement obtenue, avant toute conception.**
   Deux tours réels du chat obrain, lecture de `timings.cached_tokens` /
   `prompt_n`. C'est la seule mesure qui décide de la valeur de tout le reste —
   et l'on sait déjà, par ailleurs, que cette réutilisation est **documentée
   défaillante pour `qwen35`** (#21831) avec un **abandon de serveur** sur le cas
   « prompt identique » (#24003, reproduit ici). **Coût : deux requêtes.**
2. **Corriger l'ordre de l'invite (§0.4).** Gratuit, une poignée de lignes :
   système stable + outils + historique d'abord, extrait de graphe et question
   ensuite. Sans cela, tout ce qu'on construira au-dessus travaillera contre un
   cache qu'on invalide soi-même. **Coût : quelques heures.**
3. **Réparer le lien (§0.3).** 100 Mb/s → 1/2,5/10 GbE. Ce n'est pas une
   optimisation de confort : c'est ce qui décide si l'injection d'état est
   **possible du tout** (537 s → 54 s → 5 s). Et c'est indépendant de tout le
   reste : **la carte ne fait que du 10/100**, ce qui pénalise toute la flotte.
   **Coût : quelques dizaines d'euros.**
4. **Compresser l'état récurrent avant de le persister.** EXTERNE : SGLang livre
   déjà `--enable-int8-mamba-checkpoint` ; llama.cpp a une proposition équivalente
   (`--rs-checkpoint-codec`, RHT-INT16 ×2, **RHT-INT8 ×4** : 149,6 Mio → 74,8 →
   **37,4 Mio**). Sur 48 couches et plusieurs créneaux, c'est un facteur 4
   **linéaire** sur le poste qui domine le stockage des checkpoints. **Rapport
   valeur/effort sans comparaison avec un state store.**
5. **Quantifier le KV (FP8, voire 4 bits).** Le KV domine le stockage au-delà de
   2 304 jetons (114× à 262k) — et à contexte long, c'est lui qui fixe le plafond
   dur de débit (732 tok/s à 100k). Le diviser par deux **repousse le plafond**.
   **Coût : un drapeau** (quand le noyau le supporte).
6. **Décider le périmètre de partage, et l'écrire (§2.20).** La parade officielle
   au canal auxiliaire (`cache_salt`) **supprime la dédup inter-locataires**. Le
   facteur 4,44× est atteignable **dans** un locataire, jamais **entre**. Cette
   décision appartient au même endroit que l'IAM d'obrain, pas au state store.
7. **Décider la machine avant d'optimiser le logiciel.** 180 Go/s mesurés contre
   4,8 To/s supposés : ~27×. Aucune optimisation de cache ne rattrape un facteur
   27 de bande passante. La location d'un H200 (4,31–6,31 $/h on-demand, 2,19–2,62 $
   en spot — votre 4,59 $/h est réel) est une décision d'infrastructure qui
   **domine** toutes les autres.
8. **Si la réutilisation est cassée : changer de moteur, ou assumer le fork.**
   SGLang a la primitive exacte (**`MambaRadixCache`**, composant `MAMBA` de
   `UnifiedRadixCache` : chemin FULL partagé + checkpoint récurrent copié dans un
   créneau privé), mais elle exige `--mamba-radix-cache-strategy extra_buffer` et
   CUDA/MUSA/NPU, et son taux de réutilisation **s'effondre au-delà de quelques
   centaines de préfixes distincts** (0,94 → 0,02 à K=384). llama.cpp n'expose
   **aucun endpoint d'injection** et le mainteneur a explicitement refusé d'en
   ajouter un (#23817). Les deux chemins sont donc réels, et aucun n'est gratuit.
9. **Fabriquer les préfixes communs** : invite système canonique, schémas d'outils
   canoniques, sérialisation de graphe canonique (même ordre, mêmes séparateurs,
   même troncature), ordre stable. C'est **le** multiplicateur qui fait passer le
   facteur de partage de ~1× à 4× — et c'est du travail de **politique de
   contexte**, pas de tenseurs. C'est aussi la seule chose de cette liste que
   **personne d'autre qu'obrain ne peut faire**.
10. **Alors, et seulement alors, le state store** — d'abord avec les primitives
    existantes (`--slot-save-path`, `/slots`), en **co-localisé** avec le modèle,
    avec la clé de Merkle complète (préfixe entier, positions, version de modèle,
    quantification, noyaux), une politique d'éviction à trois paliers dont le NVMe
    est la **capacité** et non le débit (plancher PCIe 122 ms pour 6,25 Gio), et un
    sel par locataire.

**Reformulation stratégique, qui est ma réponse à « n'y a-t-il pas mieux » :**
le stockage d'états KV est en train de devenir une **commodité** (llama.cpp,
SGLang HiCache, vLLM, Mooncake, Dynamo — tous livrés). Ce qu'aucun de ces moteurs
ne peut construire, c'est **la politique de sélection et de canonicalisation du
contexte** — et c'est exactement ce qu'est obrain. **Construisez la politique,
louez le stockage.** Et avant la politique : **mesurez la réutilisation**, parce
que tant qu'elle vaut zéro, elle est le seul chiffre qui compte.

---

## 4 · Ce que je ne tranche pas

| question | état | ce qu'il faudrait |
|---|---|---|
| Le build `b53` du service 27B réutilise-t-il un préfixe ? | **RÉSOLU** : la mesure n'a pas été faite (elle a tué le service local `b9222`, §0.6). On sait par ailleurs que c'est **documenté cassé pour `qwen35`** (#21831) | 2 requêtes identiques sur `id_slot` fixe, lecture de `timings.cached_tokens` |
| Le SKU exact de la machine qui sert | **NON VÉRIFIÉ** — profil **DÉDUIT** de 180 Go/s et 40 TFLOPS (type DGX Spark/GB10) | `nvidia-smi` / fiche de la machine |
| La table c1/c8/c32 a-t-elle une source citable ? | **RÉSOLU** : aucune source publique identifiable ; ininterprétable sans quantification ni longueur de contexte. Cohérente avec du Q4 (le modèle servi), incompatible avec du BF16 | — |
| Le taux de 98 % de réutilisation | **RÉSOLU** : mal attribué — c'est un hit rate L3 HiCache sur un multi-tour **synthétique** (DeepSeek-V4-Flash) ; sur du vrai trafic Claude Code, SGLang mesure 83,8 % pondéré, et **0 jeton caché par tour** sur les sessions agentiques longues | — |
| La vitesse réelle du lien vers le service | **PARTIELLEMENT RÉSOLU** : `ethtool` donne la **négociation** (100 Mb/s, modes supportés 10/100 seulement, zéro erreur) ; le débit en charge n'a pas été mesuré | un transfert volumineux mesuré |
| `192.168.1.7` n'est pas dans l'inventaire de la flotte | **NON RÉSOLU** : `docs/MACHINES-CONNUES-2026-09-15.md:14` documente la DGX en **`.107`** (qui ne répond pas sur `:9910`), or le dashboard vivant vise **`.7`**. L'écart est réel, quel qu'en soit le sens | réconcilier l'inventaire |
| L'état récurrent en INT8/RHT est-il utilisable en production ? | **PROPOSÉ, non mergé** côté llama.cpp (`--rs-checkpoint-codec`) ; **livré** côté SGLang (`--enable-int8-mamba-checkpoint`) | un essai sur le 27B |
| Le fork d'état récurrent tient-il à l'échelle de milliers de préfixes ? | **NON** d'après la mesure SGLang (effondrement à K≈384) — mais c'est une mesure SGLang, pas la vôtre | votre charge |

---

## 5 · Conclusion en une phrase

**La proposition est juste dans son principe et exacte dans presque tous ses
chiffres de modèle — mais le levier qu'elle croit tenir (la dédup) achète de la
mémoire et non du débit, sa table de référence décrit un régime de contexte
qu'elle ne nomme pas et franchit un plafond physique à 100k, le mécanisme dont
elle dépend est documenté cassé pour le modèle réellement servi, et le défaut le
plus rentable à corriger n'est pas dans la proposition : il est déjà dans l'ordre
de l'invite, où il rend gratuitement inutile le cache que la proposition veut
persister.**

### Les cinq chiffres à retenir

| | |
|---|---|
| **732 tok/s** | plafond **dur** de sortie à 100k de contexte sur un H200, **invariant au lot** |
| **≈35k jetons** | la longueur de contexte implicite de la table c1/c8/c32 citée |
| **21,2 tok/s/util** | ce que donne réellement 32 × 100k — **partage ou pas** |
| **536 s vs 136 s** | injection d'état contre recalcul du prefill, **sur votre lien actuel** |
| **11 tok/s vs 184–236** | débit mesuré du service réel contre la table de référence |

### Une dernière remarque, qui n'est pas technique

La proposition est écrite comme si obrain était un moteur d'inférence. obrain
n'est pas cela : c'est **un graphe qui choisit ce que le modèle voit**. La
conséquence n'est pas une critique — c'est une promotion. Le state store, tout le
monde va le livrer ; SGLang, vLLM, Moonlight, Dynamo et llama.cpp l'ont déjà
livré en partie. **Ce que personne ne peut livrer à votre place, c'est de savoir
quel contexte mérite d'être mis dans la fenêtre, et sous quelle forme canonique.**
C'est là qu'est votre actif, et c'est là que le prochain chantier devrait
commencer — pas dans les tenseurs.
