# ÉTALONNAGE DU JUGE STRUCTUREL — ce que le juge mesure vraiment

**Date** : 2026-09-20 23:06 → 2026-09-21 00:45 (`n3`, 192.168.1.103). **Nature** : note de mesure.
Elle ne remplace pas `docs/REPRISE-TERNARISATION-2026-09-20.md` (mode d'emploi) ni
`docs/TERNARISATION-2026-09-20.md` (récit technique) : elle répond à **une seule question**, la
première qu'il faut poser à un instrument neuf — *que mesure-t-il réellement ?*

**Régime des affirmations** — celui du dépôt :

| mot | sens |
|---|---|
| **MESURÉ** | relevé par exécution, commande citée |
| **CONSTATÉ** | lu dans un fichier, ligne citée |
| **CALCULÉ** | arithmétique, valeurs données |
| **NON VÉRIFIÉ** | pas de relevé |

Le juge pris comme référence est celui de `scripts/juge-structurel.py`,
**empreinte SHA-256 `6c63ff5871af4dc3`** (16 premiers). Tous les scores ci-dessous sont à cette
empreinte : si le fichier change, **aucun chiffre de cette note n'est plus comparable**.

---

## 1 · La question qui manquait : le plafond

Le principe était déjà écrit dans le dépôt : *« un juge que la référence rate n'est pas un juge »*.
Il n'avait jamais été **vérifié** : ni la note de reprise, ni la doc technique ne portaient de score
de référence. Un score de candidat est illisible sans plafond — 2/3 ne veut pas dire la même chose
selon que la référence fait 3/3 ou 0/3.

**MESURÉ** — référence `Qwen3.8-27B-Q4_K_M.gguf` (18,97 Go, la meilleure dont on dispose, PPL
2,3983), découpage **une famille par invocation** (même graine, donc **mêmes questions** que pour le
candidat), `--n 3 --forme nom` :

```bash
scripts/juge-plafond.sh reference --n 3 --forme nom --port 8120
```

| famille | forme de la réponse attendue | **référence Q4_K_M** | ternaire `OBRAIN-v2` |
|---|---|---|---|
| `degre` | un **seul** nœud | **3/3 = 100 %** | 2/3 = 67 % |
| `redondance` | **OUI / NON** | 2/3 = 67 % | **3/3 = 100 %** |
| `atteignabilite` | énumérer un **ensemble** | 1/3 = 33 % | 0/3 = 0 % |
| `cycles` | énumérer un **ensemble** | **0/3 = 0 %** | 0/3 = 0 % |

**Le meilleur modèle disponible fait 0/3 sur `cycles`.** Ce n'est donc pas le modèle qui échoue :
c'est l'instrument. Et le tableau se lit d'un coup — **les deux familles qui départagent sont
exactement celles qui ne demandent pas d'énumérer** ; les deux qui échouent exigent un ensemble
complet.

**Deuxième constat, immédiat** : le ternaire fait **100 % sur `redondance` là où la référence fait
67 %**. Sur 3 questions, cela n'établit rien sur la qualité — cela établit que **n=3 ne départage
pas un candidat de 11,6 Go d'un modèle de 19 Go**. Voir §5.

---

## 2 · L'instrument était-il juste ? (vérification hors ligne, sans modèle)

Avant de croire un seul score, l'oracle a été recalculé par des méthodes **délibérément
différentes** — c'est le seul moyen d'attraper une vérité terrain fausse.

**MESURÉ** — `scripts/verifier-juge.py`, aucun modèle, aucun réseau :

```bash
python3 scripts/verifier-juge.py --essais 150 --graines 5
```

| ce qui est vérifié | méthode indépendante | résultat |
|---|---|---|
| oracle `cycles` | **Floyd-Warshall** (fermeture transitive) contre le Kosaraju du juge | identique |
| oracle `atteignabilite` | BFS explicite contre ses ensembles de successeurs | identique |
| oracle `degre` | comptage à la main, **type relu dans la question** | identique |
| oracle `redondance` | BFS de longueur ≥ 2 contre sa recherche en deux maillons | identique |
| énoncé | le graphe écrit dans l'invite = exactement l'ensemble d'arêtes de l'oracle | 1 938/1 938 |
| forme `indice` | numéros distincts, aucun `fn_…` survivant, réponse = bon numéro | 1 938/1 938 |

**VERDICT : COHÉRENT.** L'oracle du juge est juste, sa notation n'est pas en cause, et le renommage
en indices ne corrompt rien. Les échecs du §1 sont donc **de vrais échecs de modèle**, pas des
artefacts de mesure.

**Deux constats de rendement d'engendrement** (même exécution, 750 tirages par famille) :

| famille | questions valides | rejetées | pourquoi |
|---|---|---|---|
| `redondance` | **750** | **0** | la réécriture constructive garantit sa question (avant : **0/72 — la famille ne mesurait rien**) |
| `cycles` | 495 | 255 | exige 1 à 6 nœuds en cycle |
| `atteignabilite` | 411 | 339 | exige 1 à 6 nœuds au niveau 2 |
| `degre` | **282** | **468** | exige in-degré ≥ 2 **et** gagnant unique — la plus rare (38 %) |

**CONSTATÉ — un défaut réel, latent, non corrigé** : `q_redondance` retire l'arête directe `(a,t,b)`
puis ajoute le triangle **sans vérifier l'appartenance des deux autres maillons**. 70 graphes sur
750 (**9,3 %**) contiennent donc des arêtes **en double** : l'énoncé écrit deux fois la même ligne.
Sans effet sur la réponse attendue, mais un juge qui teste la précision ne devrait pas mentir sur son
propre compte d'arêtes. **Piste** : construire le triangle dans un `set`. **NON VÉRIFIÉ** que cela
change un seul score.

**MESURÉ — ce défaut n'a PAS touché la campagne du §1** : les 9 questions réellement posées à la
référence ont été régénérées hors ligne, **aucune n'a d'arête en double** (16, 13, 16 / 13, 12, 13 /
13, 14, 9 arêtes, toutes distinctes). Les réponses attendues régénérées correspondent **exactement**
à celles enregistrées par le juge — la reproductibilité est établie par construction.

---

## 3 · Le diagnostic : le modèle fait l'opération, mais rend un ensemble INCOMPLET

Les réponses du modèle sont **bien formées** (`fn_1005, fn_1006, fn_1007`) : la notation n'est pas
en cause. Elles sont **incomplètes** :

| famille | attendu | obtenu |
|---|---|---|
| `cycles` | `fn_1002, fn_1004, fn_1005, fn_1006, fn_1007` (5) | `fn_1005, fn_1006, fn_1007` (3) |
| `cycles` | `fn_1001, fn_1003, fn_1004, fn_1007, fn_1009, fn_1010` (6) | 5 nœuds |
| `atteignabilite` | `fn_1003, fn_1006` (2) | `fn_1006` (1) |
| `atteignabilite` | `fn_1001, fn_1007` (2) | `fn_1001` (1) |

Et **la seule question `atteignabilite` à réponse unique** (`['fn_1009']`) est comprise dans l'unique
réussite de la famille. Le motif est donc précis : **le modèle trouve des membres, il ne garantit pas
l'exhaustivité.**

---

## 4 · Le correctif, et sa validation par la mesure

**Hypothèse** : si l'obstacle est l'énumération et non le raisonnement, alors reposer **la même
question de structure** sous forme **fermée** (OUI/NON sur un nœud nommé) doit faire remonter le
plafond.

**MESURÉ** — `scripts/juge-binaire.py` : mêmes graphes, mêmes oracles (importés du juge, jamais
réécrits), mêmes réglages de requête (`enable_thinking: False`, 64 jetons). **Seule la question
change.**

| famille | forme énumération | **forme fermée** |
|---|---|---|
| `cycles` | 0/3 = **0 %** | **6/9 = 67 %** |
| `atteignabilite` | 1/3 = **33 %** | **4/6 = 67 %** |
| total fermé | — | **10/15 = 67 %** |

L'hypothèse est confirmée : **l'obstacle était la forme de la question.** Le plafond passe de 0 % à
67 % sur `cycles`, et double sur `atteignabilite`.

**Le mode d'échec restant est net et exploitable** : les ratés se concentrent sur les **appâts**. Sur
`atteignabilite`, les distracteurs choisis sont des nœuds atteignables en **1** arête — donc
« proches mais NON » — et **les deux ont reçu `OUI`**. Le modèle confond « ≤ 2 » avec
« **exactement** 2 » : c'est le qualificatif qu'il rate, pas l'atteignabilité. Les items appâts sont
donc **les plus discriminants** du juge, et devraient être posés en nombre, pas par accident.

---

## 5 · Ce que ce juge ne peut PAS encore faire

**MESURÉ, et c'est le constat le plus important pour la suite** : à `n = 3` par famille, avec les
plafonds du §1, **le juge ne départage pas le candidat ternaire du modèle 4× plus gros.** Les deux
seules familles exploitables donnent :

- `degre` (plafond 100 %) : référence **3/3**, ternaire **2/3** — un écart d'**une** question ;
- `redondance` (plafond 67 %) : référence 2/3, ternaire **3/3** — le ternaire **au-dessus**.

Un écart d'une question dans un sens puis dans l'autre n'est pas un résultat. **CALCULÉ** : avec 3
questions et un plafond de 100 %, la résolution est de 33 points ; aucune conclusion sur la
quantification n'est possible à ce régime. `n` doit monter (≥ 10 par famille) **avant** de rejouer le
balayage de politiques — sinon on paiera des heures de préfill pour du bruit.

---

## 6 · La réflexion (« thinking ») : testé, INCONCLUSIF, et coûteux

**Hypothèse testée** : le juge envoie `enable_thinking: False` avec pour justification *« pour ces
questions la réponse est un identifiant, pas un raisonnement »*. Une fermeture transitive **est** un
raisonnement — l'hypothèse méritait l'essai.

**MESURÉ** — `scripts/juge-reflexion.py`, qui **rejoue la boucle du juge à l'identique** pour
retomber sur les mêmes questions. **Équivalence prouvée d'abord** : en `--sans-penser`, il rend la
même question (`['fn_1002','fn_1004','fn_1005','fn_1006','fn_1007']`) et la **même réponse fausse**
(`fn_1005, fn_1006, fn_1007`). La seule variable qui change ensuite est `enable_thinking`.

**Résultat : les deux essais finissent en `finish_reason = "length"` — le budget de 384 jetons a été
entièrement consommé par le raisonnement (765 et 789 caractères), et `content` est VIDE.** Le modèle
a été **coupé en pleine pensée**, pas mis en échec. **Le test ne conclut rien** et n'est pas compté
comme un échec du modèle.

**Deux mesures utiles au passage** :

| | référence Q4_K_M | ternaire OBRAIN-v2 |
|---|---|---|
| préfill | **13,3 tok/s** | 4,4 tok/s |
| décodage (sans réflexion) | 1,45 tok/s | **2,6 tok/s** |
| décodage (avec réflexion) | **0,89-0,95 tok/s** | NON MESURÉ |

**CALCULÉ** : en mode réflexion, une question coûte **434 s et 454 s** — soit **11×** le coût du
réglage du juge (39 s). Le raisonnement part dans `reasoning_content`, **séparé du contenu** : le
harnais doit lire les deux champs. **Le « sans réflexion » du juge est donc un arbitrage de coût
mesuré, pas une négligence** — mais il est pris sur la seule famille qui ne peut pas s'en passer.

---

## 7 · Ce qui change dans le plan

1. **`cycles` et `atteignabilite` doivent être réécrites en questions fermées** (§4 : 0 % → 67 %,
   33 % → 67 %). Le juge n'est pas à abandonner : il est à reformuler.
2. **Les appâts sont l'item de mesure**, pas le remplissage : les poser en nombre (§4).
3. **`degre` et `redondance` sont les seules familles utilisables en l'état** — plafonds 100 % et
   67 %.
4. **Monter `n` à ≥ 10 par famille** avant tout balayage de politiques (§5).
5. Le défaut d'arêtes en double de `q_redondance` est à corriger (**une ligne**), sans effet mesuré
   connu sur les scores.

**Convergence, à ne pas dupliquer** : pendant cette campagne, une **autre session** a fait évoluer le
juge dans la même direction — famille `voisins` (voisinage à **1 saut**, 1 à 4 nœuds) et drapeau
`--selection brute|oracle` (le modèle doit-il *sélectionner* les lignes, ou les reçoit-il ?). C'est un
**second remède au même diagnostic** : réduire la **taille** de l'ensemble plutôt que **fermer** la
question. Les deux sont complémentaires et devraient être mesurés l'un contre l'autre. Son empreinte
est passée à `0fa8b259f0aad40d` — **tous les chiffres de cette note restent épinglés à
`6c63ff5871af4dc3`**.

---

## 8 · Incident d'exécution, consigné parce qu'il est instructif

**MESURÉ** (23:10) : la première campagne a été **détruite** — un autre processus a exécuté
`pgrep -x llama-server | xargs -r kill`, tuant le serveur en pleine 4ᵉ question. Le juge n'imprime son
verdict qu'à la toute fin : **les 3 réponses déjà obtenues ont été perdues**. D'où deux précautions
adoptées ici : **une famille par invocation** (une mort ne coûte qu'une famille) et
**un journal par question, poussé sur le disque** (`juge-reflexion.py`, `juge-binaire.py`).

**Piège reproduit en direct** : `pgrep -f "port 8120"` **matche la ligne de commande de son propre
shell** (le motif figure dans la commande). Le `kill` prévu a donc failli tuer ma propre session.
**Ne jamais tuer par motif — seulement par PID exact.**

**Espace disque** : `/tmp` est sur **`/`** (49 Go libres désormais) tandis que **`/mnt/obrain` est un
volume distinct** (935 Go, 431 Go libres). Le balayage précédent écrivait ses candidats dans `/tmp` —
**c'est la raison exacte pour laquelle les artefacts `attn_qkv`/`ssm_out` ont disparu**. Toute
quantification doit écrire sur `/mnt`.

**CONSTATÉ — deux fichiers de 11 Go dormaient dans `/tmp`**, identifiés par leurs types de tenseurs
(`scripts/identifier-candidat.py`, lecture du seul en-tête GGUF, jamais des 11 Go) :

| fichier | identité | suite |
|---|---|---|
| `/tmp/cand.gguf` | SHA-256 `7e95a6f2…` **identique à `OBRAIN-v2`** | doublon prouvé, supprimé (11,6 Go rendus) |
| `/tmp/c2.gguf` | ne diffère de `v2` **que** par `attn_output` Q4_K sur les 17 couches | rapatrié sur `/mnt` (voir §9) |

**Trois incidents de plus, tous MESURÉS pendant l'étape (a), et leurs correctifs :**

| incident | preuve | correctif adopté |
|---|---|---|
| **le serveur est tué de l'extérieur** en pleine campagne | journal : `Received second interrupt, terminating immediately` — 1 min après le début ; le juge a attendu **600 s** puis abandonné, **0 donnée** | lanceur `scripts/campagne-a.sh` : **relance le serveur et reprend à l'unité suivante** |
| **le juge change pendant la campagne** | empreintes `6c63ff58` → `0fa8b259` → `57de3f98` (une autre session le développe) | **instrument FIGÉ** : copie `juge-gele.py`, empreinte écrite dans le journal — *un instrument qui bouge ne mesure rien* |
| **`pkill -f` tue mon propre shell** (2ᵉ fois) | mon shell a reçu `SIGTERM` : le motif figurait dans sa propre ligne de commande | **plus jamais de `pkill -f`** — uniquement des PID explicites |

**Et un défaut de MA part, attrapé avant la première mesure** : la première version du lanceur
réutilisait un serveur déjà sain **sans vérifier quel modèle il servait**. La campagne aurait donc
mesuré `v2` contre le serveur de la **référence** — des chiffres parfaitement plausibles et
parfaitement faux. Corrigé par un garde-fou (`MODELE_CHARGE`), avant d'avoir payé quoi que ce soit.

**Doctrine retenue** : *unité = (modèle, forme, famille)*, jamais un bloc — une mort ne coûte qu'une
unité ; instrument figé et empreinté ; aucun meurtre par motif.

---

## 9 · Deux points de dossier ouverts, refermés ou bloqués

**Refermé — l'`attn_output` n'est plus perdu.** La variante est préservée à
`/mnt/obrain/models/Qwen3.8-27B-OBRAIN-attn_output.gguf` (11,73 Go). Elle tombe pile sur une question
**ouverte** du §8 de la note de reprise (« sans effet de perplexité, mais sans effet sur les tâches ?
NON TRANCHÉ — une seule mesure ») : le fichier existe désormais pour la trancher avec le juge.

**DÉBLOQUÉ — `qa_obrain.py` : la cause n'était ni l'IAM ni les données, c'était un DÉSACCORD DE
SCHÉMA.** Le diagnostic initial de cette note (« tous les nœuds sont des cosses ») était **trop
large** : il attribuait à *toutes* les copies un avertissement qui n'en concernait qu'une. Corrigé,
et mesuré :

| obstacle | mesure | état |
|---|---|---|
| le store du script est absent | `/tmp/kv-linux-driversnet.obrain` n'existe plus | **2 copies survivent** |
| l'une est abîmée | `-neuf` : `label_only=**333263**`, zones de propriétés = **16 Ko** | à écarter |
| **l'autre est saine** | `v2.obak-avant-v3` : `label_only=**1**`, zones de propriétés = **2,40 Go** | **à utiliser** |
| l'IAM est armé | `OBRAIN_IAM=1`, `OBRAIN_IAM_STORE=~/.local/share/obrain-harness/data/iam.obrain` | **jeton existant trouvé** |
| aucun jeton dans l'environnement | `OBRAIN_IAM_TOKEN` absente de mon shell | `/home/triviere/.config/obrain-harness/iam-token.env` (68 car.) — **la voie documentée** |

Avec l'identité fournie par ce jeton, la lecture passe : **105 546 nœuds `Function`**, sans désarmer
aucune garde. (`OBRAIN_IAM=0` ouvrirait le store aussi — **je ne m'en suis pas servi** : ce serait
contourner une garde délibérément armée.)

**MESURÉ — et voici le vrai obstacle, plus intéressant** :

```
MATCH (n:Function) WHERE n.text IS NOT NULL RETURN count(n)   →   0
```

**Aucun** des 105 546 `Function` ne porte `text` — ni `code`, `body`, `source`, `content`, `doc`. Ce
store est **structurel** : il porte un **pointeur** vers la source (`file_path` + `start_line` +
`end_line`), pas la source. Ce n'est pas une avarie : c'est un choix d'indexation, et le store est
riche par ailleurs (`bt`, `complexity`, `deg_in`, `scc_id`, `param_types`…).

`qa_obrain.py` exige `n.text` : **il ne peut pas fonctionner sur ce store, quelle que soit
l'identité.** Le remède est une **résolution**, pas une réparation :

- préfixe déterminé **empiriquement** (8 `file_path` tirés du store, tous résolus) :
  `/tmp/linux_src/drivers/net/` + `file_path` ;
- la source est **sur la machine** (`/tmp/linux_src`, 31 entrées) ;
- écrit : `ops-gpu16/qa_obrain_source.py` — même flux, `pick_facts` **résout** le pointeur.

**MESURÉ — le côté graphe est validé de bout en bout** (`--dry`, aucun modèle) :

| question | source résolue |
|---|---|
| `e1000e_get_bus_info_pcie` | `ethernet/intel/e1000e/mac.c:14-36` — **719 car.** |
| `ixgbe_probe` | `ethernet/intel/ixgbe/ixgbe_main.c` — **1 400 car.** |
| `e1000e_open` | `ethernet/intel/e1000e/netdev.c` — **1 400 car.** |

**Et un second défaut, indépendant** : le repli de `qa_obrain.py` vise
`…intel.e1000.e1000_probe`, **MESURÉ absent du store** (résultat vide). Le repli ne rattrape donc
jamais rien ; la version réparée vise une fonction qui existe réellement.

**Le danger reste le silence** — `pick_facts` avale l'exception (`except Exception: pass`) : le
script poserait la question **avec un fait vide**, un verdict sans signification qui *ressemble* à un
verdict. C'est ce qui a rendu les trois obstacles invisibles jusqu'ici.

### 9bis · MESURÉ — le flux complet sur le candidat ternaire, et le fait est VÉRIFIÉ

Un seul serveur porte les deux usages (`v3`, `/jspace` **et** `--jinja`, port 8099,
`OBRAIN_STATE_TYPE=i8`) :

```bash
OBRAIN_STATE_TYPE=i8 llama-server -m Qwen3.8-27B-OBRAIN-v3.gguf --port 8099 \
    -c 4096 -t 8 -ngl 0 --no-warmup --jinja --parallel 3 &
. ~/.config/obrain-harness/iam-token.env          # l'identité, la voie documentée
python3 ops-gpu16/qa_obrain_source.py --port 8099 "Que fait la fonction ixgbe_probe ?"
```

```
[obrain] 1 fait(s), 1400 caractères de source RÉSOLUE :
    // tmp-linux-6.6-drivers-net.ethernet.intel.ixgbe.ixgbe_main.ixgbe_probe
[obrain] extract 92.2s | blob 69 MiB | ref e88ede7abb…
RÉPONSE : « La fonction `ixgbe_probe` est utilisée pour initialiser et configurer le pilote réseau
Ethernet Intel ixgbe. […] 1. Vérifie si le périphérique est un périphérique virtuel (VF) et renvoie
une erreur si c'est le cas. »
[métriques wall] TFPT≈546.9 ms | wall 51.7 s | tok/s≈1.81 | state_load 165 ms
```

**Et l'affirmation a été VÉRIFIÉE contre la source, pas seulement trouvée plausible** : `ixgbe_probe`
(ligne 10 758) porte bien `WARN(1, KERN_ERR "%s (%hx:%hx) should not be a VF!\n", …)` suivi de
`return -EINVAL`. L'énoncé du modèle est une **lecture fidèle du code injecté**.

**Ce qui est établi** : le chemin complet *store → sélection → résolution de source → extract i8
(69 MiB) → injection par référence → réponse* **fonctionne avec le modèle ternaire `v3`**, à
**1,81 tok/s** (TFPT 0,55 s, `state_load` 165 ms), sous IAM armé et sans désarmer aucune garde.

**Ce qui n'est PAS établi** : qu'**une** question fasse une validation. C'est une preuve de
**fonctionnement**, pas de **qualité** — pour la qualité il faudrait une série de questions avec
vérité terrain, et le juge structurel du §5 n'est pas encore en état de la fournir.

---

## 10 · Les instruments laissés sur place

| outil | ce qu'il répond | coût |
|---|---|---|
| `scripts/verifier-juge.py` | l'oracle du juge est-il juste ? (hors ligne) | secondes, aucun modèle |
| `scripts/juge-plafond.sh` | quel plafond, famille par famille ? (1 famille/invocation, journalisé) | ~5 min/famille |
| `scripts/juge-binaire.py` | l'énumération est-elle l'obstacle ? (questions fermées) | ~25 s/question |
| `scripts/juge-reflexion.py` | le silence imposé est-il la cause ? (équivalence prouvée) | 7-8 min/question |
| `scripts/identifier-candidat.py` | quelle politique a produit ce GGUF ? | secondes, lit l'en-tête seul |
| `scripts/balayage.sh` | un point de politique : quantifier → PPL → journal TSV | ~9 min/point |

Journaux : `/mnt/obrain/juge/journal.tsv` (plafonds), `reflexion.jsonl`, `binaire.jsonl`.
Corpus de perplexité figé : `/mnt/obrain/corpus/ppl.txt`, SHA-256
`9d16815ecd5c1e1a…` — **le même que celui de la table de descente**, pour que les points restent
comparables.

---

## 11 · Non établi

| question | état |
|---|---|
| La réflexion lève-t-elle le plafond de `cycles` ? | **NON TRANCHÉ** — budget insuffisant, `content` vide (§6). Coût 7-8 min/question |
| Les familles fermées départagent-elles le candidat ? | **NON MESURÉ** — la forme fermée n'a été passée que sur la référence |
| `n ≥ 10` par famille : le juge discrimine-t-il ? | **NON MESURÉ** |
| Le défaut d'arêtes en double change-t-il un score ? | **NON MESURÉ** |
| `qa_obrain.py` : la qualité sur les tâches obrain | **côté graphe DÉBLOQUÉ et validé** (§9) ; **le verdict du modèle reste NON MESURÉ** — le flux `/jspace` complet n'a pas encore été exécuté |

---

## Annexe A — l'axe perplexité : la contre-épreuve, puis le point manquant

Hors sujet du juge, mais ferme deux trous du dossier. Même discipline : **valider l'instrument
avant de croire un chiffre.**

**MESURÉ — contre-épreuve de reproductibilité.** Le point déjà consigné (PPL 4,74 pour `OBRAIN-v2`)
a été remesuré par le pipeline neuf, corpus **identique** (SHA-256 `9d16815ecd5c1e1a…`, celui de la
table de descente) :

```bash
scripts/balayage.sh ref     # → PPL = 4.7407   (233 s)
```

**4,7407 contre 4,74 consigné** : l'instrument de perplexité reproduit le chiffre du dossier. Les
points suivants lui sont donc comparables — c'est la condition pour que quoi que ce soit de nouveau
soit interprétable.

**MESURÉ — le point prédit « ~4,0 » et JAMAIS mesuré** (§2 de la note de reprise) : les deux
meilleurs ajouts **réunis** — `attn_qkv` Q4_K **et** `ssm_out` Q4_K, sur la base bords 8 + `attn_v`
+ `ffn_down` :

```bash
scripts/balayage.sh combo   # → PPL = 4.0825  à 12 510 038 784 octets (12,51 Go), 336 s
scripts/balayage.sh liste   # journal TSV append-only
```

| point de la descente | taille | perplexité | statut |
|---|---|---|---|
| `OBRAIN-v2` (bords 8 + `attn_v` + `ffn_down`) | 11,61 Go | 4,7407 | remesuré ici (§A) |
| + `ssm_out` seul | 11,95 Go | 4,3976 | consigné |
| + `attn_qkv` seul | 12,17 Go | 4,2903 | consigné |
| **+ les deux (`combo`) — nouveau meilleur point** | **12,51 Go** | **4,0825** | **mesuré ici** |
| + `attn_output` | 11,73 Go | 4,7148 | consigné (nul) |
| référence `Q4_K_M` (Unsloth) | 18,97 Go | 2,3983 | consigné |

**CALCULÉ** : +0,90 Go font passer la perplexité de 4,7407 à **4,0825**, soit **−13,9 %**. Le nouveau
meilleur point connu tient dans **12,51 Go contre 18,97** — **34 % plus petit que la référence**,
sans un seul entraînement.

**Artefact promu** : `/mnt/obrain/models/Qwen3.8-27B-OBRAIN-v3.gguf` — **lien matériel** vers
`balayage/combo.gguf` (même volume : **0 octet consommé**, et le fichier de balayage reste en place,
donc le journal garde sa trace).

**MESURÉ — et il raisonne** (critère du §3.3 de la note de reprise ; `--jinja` obligatoire) :

```
raisonnement 582 car. | reponse 297 car. | -> RAISONNE
« Un arbre binaire de recherche est une structure de données où chaque nœud possède au plus deux
  enfants, et où les valeurs situées à gauche sont inférieures à la valeur du nœud, tandis que
  celles situées à droite sont supérieures […] »
```

**v3 est donc le meilleur artefact connu sur les trois critères du dossier** : taille (12,51 Go contre
18,97 pour la référence), perplexité (4,0825) et raisonnement (582 car., contre ≈ 0 pour `v1`).

**Réserve, et elle compte** : la perplexité est mesurée à `-c 512 --chunks 2`, soit **1 024 jetons**.
La comparaison est **appariée** (même corpus, mêmes drapeaux) donc les écarts sont significatifs
*entre points* ; mais **1 024 jetons est un corpus mince**, et rien n'établit que ce classement
tienne sur un corpus plus large. C'est la même classe d'erreur que les « trois erreurs de protocole »
déjà payées dans ce dépôt — signalée ici, **NON VÉRIFIÉE**.

---

## Annexe B — étape (b) : le « bit manquant » est-il sur l'attention ? **NON**

**Question posée par la note de reprise** : *« `attn_qkv`/`attn_v` en Q5_K/Q6_K — localise le bit
manquant sur l'attention »*. C'était l'hypothèse à tester. Elle est **refutée par la mesure**.

**Protocole** : chaque point ne bouge **qu'une variable** depuis le meilleur point connu (`combo`,
4,0825) — sinon un écart ne s'attribue à aucune cause. Corpus et drapeaux figés
(`scripts/balayage.sh`, journal TSV append-only).

```bash
for p in qkv5 qkv6 v5; do scripts/balayage.sh $p; done
scripts/balayage.sh liste
```

| point | la seule variable | taille | perplexité | gain sur `combo` |
|---|---|---|---|---|
| `combo` (référence du balayage) | — | 12,51 Go | 4,0825 | — |
| `v5` | `attn_v` Q4_K → **Q5_K** | 12,52 Go | 4,0803 | **−0,05 %** |
| `qkv5` | `attn_qkv` Q4_K → **Q5_K** | 12,82 Go | 4,0723 | −0,25 % |
| `qkv6` | `attn_qkv` Q4_K → **Q6_K** | 13,16 Go | **4,0443** | −0,94 % |

**Trois lectures, dans l'ordre d'importance :**

1. **`attn_v` ne gagne RIEN.** Q4_K → Q5_K : **−0,05 %** pour +0,01 Go. C'est une famille
   **saturée** dès Q4_K. (Et à 1 024 jetons, un écart de 0,0022 de perplexité n'a de toute façon
   aucune portée de généralisation — cf. la réserve de l'annexe A.)
2. **`attn_qkv` gagne, mais très peu, et tard.** Q4_K → Q5_K vaut 0,25 % ; Q4_K → Q6_K vaut **0,94 %**
   pour **+0,65 Go**. Autrement dit : **payer 65 % d'un gigaoctet pour 1 % de perplexité.**
3. **CALCULÉ — et c'est le point décisif** : l'écart à la référence n'est pas entamé par cet axe.
   `v3` (combo) a une perplexité **70 % plus haute** que la référence `Q4_K_M` (4,0825 contre
   2,3983). Après avoir dépensé **tout** l'axe attention, `qkv6` l'a **69 %** plus haute. Le
   balayage déplace la virgule, pas la question.

**Conclusion** : **la qualité manquante n'est pas sur la précision de l'attention.** Elle est
ailleurs — dans la masse des tenseurs restés ternaires, dans la rotation, ou dans ce que seul un
entraînement apporte. Chercher « le bit manquant » par raffinements successifs a donc un **coût
marginal démesuré** : la piste est close, et c'est un résultat utile, pas un échec.

**Artefact** : `qkv6` est promu en `/mnt/obrain/models/Qwen3.8-27B-OBRAIN-v4-qkv6.gguf` (lien
matériel, 0 octet). **`v3` reste la recommandation** : 4,0825 à 12,51 Go contre 4,0443 à 13,16 Go —
0,94 % de perplexité pour 5,2 % de taille, et `v3` a été validé sur le raisonnement (§Annexe A).

---

## Annexe C — étape (a) : nom contre indice — **la copie d'identifiant n'est PAS le problème**

**Question posée par la note de reprise** : *« `--forme indice` contre `--forme nom` — 0 bit, isole la
*copie d'identifiant* du *raisonnement* »*. Réponse : **mesurée, et c'est non.**

**Protocole, et deux précautions qui ont coûté cher à apprendre :**

1. **Instrument FIGÉ.** Le juge a changé d'empreinte **trois fois** pendant la session
   (`6c63ff58` → `0fa8b259` → `57de3f98`) parce qu'une autre session le développe en parallèle. La
   campagne tourne donc sur une **copie gelée** (`/mnt/obrain/juge/juge-gele.py`, empreinte
   `57de3f98` **écrite dans chaque ligne du journal**). Sans cela, un score « avant » et un score
   « après » ne sont pas comparables — et rien ne le signale.
2. **Unité = (modèle, forme, famille).** Le serveur a été **tué de l'extérieur trois fois** pendant
   cette seule campagne (`Received second interrupt`) ; le lanceur `scripts/campagne-a.sh` relance et
   reprend à l'unité suivante. Les 8 unités ci-dessous sont complètes malgré les trois morts.

```bash
scripts/campagne-a.sh --n 3 --port 8120      # journal : /mnt/obrain/juge/campagne-a.tsv
```

| modèle | famille | forme `nom` | forme `indice` |
|---|---|---|---|
| **référence** `Q4_K_M` | `degre` | 3/3 = **100 %** | 3/3 = **100 %** |
| **référence** `Q4_K_M` | `redondance` | 2/3 = **67 %** | 2/3 = **67 %** |
| **ternaire `v2`** | `degre` | 2/3 = **67 %** | 2/3 = **67 %** |
| **ternaire `v2`** | `redondance` | 3/3 = **100 %** | 3/3 = **100 %** |

**nom ≡ indice, dans les QUATRE paires.** Remplacer les identifiants `fn_1010` par des numéros ne
change **rien**, ni pour la référence ni pour le candidat. **L'échec n'est donc pas la manipulation
du nom** — hypothèse écartée par la mesure, et c'est une réponse nette à la question posée.

**Trois réserves, pour que ce tableau ne dise pas plus qu'il ne dit :**

- `cycles` et `atteignabilite` **sont absentes** de cette campagne : leur plafond est à 0 % et 33 %
  (§1), donc elles ne peuvent rien départager. (a) ne porte donc que sur les **deux familles à
  plafond** — c'est une limite de l'instrument, pas un choix.
- **`n = 3` par case** : chaque case vaut ±33 points. L'égalité nom/indice est frappante par sa
  régularité, mais un écart d'**une** question n'aurait rien prouvé non plus.
- Le candidat testé est **`v2`** (celui que nomme la note de reprise), pas `v3`. Utiliser `v3` est le
  prochain pas naturel.

**Ce que cela laisse debout** : le diagnostic du §3-§4 tient — ce n'est ni le nom des nœuds, ni la
précision de l'attention (Annexe B) ; c'est **l'énumération** (0 % → 67 % en questions fermées).
