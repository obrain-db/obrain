# Diagnostic — la frontière entre le modèle et le substrat, mesurée

**Date** : 2026-09-21. **Branche** : `integration/ternarisation-tq2g128`.
**Nature** : campagne de mesure sur le 27B ternaire (`Qwen3.8-27B-OBRAIN-v2.gguf`, 11,61 Go,
perplexité 4,74), instrument `scripts/juge-structurel.py` — questions **engendrées**, **oracle**
calculé par le graphe, aucun juge LLM.

**Régime des affirmations** : **MESURÉ** (exécution, commande citée) · **CONSTATÉ** (fichier cité) ·
**CALCULÉ** · **NON VÉRIFIÉ**.

## 1 · Ce que l'instrument demande, et pourquoi il est plus discriminant que la perplexité

Une question structurelle n'est pas une question de culture : la réponse **n'existe que dans
l'invite**, et elle se construit par correspondance de motifs, agrégation et **copie précise**. C'est
exactement la classe de tâches que la quantification abîme en premier — et la perplexité, dominée par
la fluidité, ne la voit pas.

## 2 · Les mesures (27B ternaire 11,61 Go, `n3`, CPU)

| opération demandée au modèle | score | option du juge |
|---|---|---|
| **lire** un voisinage **fourni** par le substrat | **100 %** (reproduit 3×) | `--familles voisins --selection oracle` |
| **sélectionner** un voisinage dans le graphe entier | 75 % | `--selection brute` |
| **borner** à 2 sauts — « exactement 2 » | **0 %** | `--familles atteignabilite --bornage exact` |
| …avec l'exclusion **DITE** (« 2 mais pas 1 », liste du niveau 1 fournie) | **0 %**, 8 nœuds rendus | `--bornage exclu` |
| copier un identifiant (nœuds numérotés au lieu de noms) | **inchangé** | `--forme indice` |
| agréger sur tout le graphe (degré entrant maximal) | 66,7 % | `--familles degre` |

**MESURÉ — le contenu de l'erreur, et il est plus parlant que le taux :** sur `--bornage exact`, la
réponse attendue `{fn_1003, fn_1006}` revient en `{fn_1003, fn_1006, fn_1008}` — **`fn_1008` est
exactement le voisin de 1 saut**. À 2 sauts, le même défaut rend dix nœuds au lieu de deux.

## 3 · Trois hypothèses réfutées, une qui tient

| hypothèse | verdict | ce qui l'a réfutée |
|---|---|---|
| copie exacte d'identifiant | **réfutée** | `--forme indice` ≡ `--forme nom` |
| composition seule (deux sauts en un geste) | **insuffisante** | il réussit **déjà** le 1 saut à 75 % |
| consigne mal comprise | **réfutée** | les deux formulations rendent **le même ensemble faux** |
| **⇒ le BORNAGE d'ensemble** | **tient** | il sur-inclut, et l'erreur **croît avec la profondeur** : +1 nœud à 1 saut, +8 à 2 |

**Ce n'est donc ni la lecture, ni la mémoire des noms, ni la formulation : c'est de DÉLIMITER un
ensemble.** Un modèle qui lit à 100 % ce qu'on lui donne mais ne sait plus retrancher ce qu'on lui
demande de retrancher.

## 4 · Ce que cela impose à l'architecture (et ce n'est pas une préférence)

| qui | quoi | preuve |
|---|---|---|
| **le modèle** | **lire** un voisinage fourni, sélectionner à 1 saut, raisonner en langage | 100 % / 75 % |
| **le substrat** | **borner** : multi-saut, cycles, redondance transitive | 0 % dans le modèle ; le graphe le fait en nanosecondes |
| **l'injection** | livrer le résultat | **198 ms** (mesuré dans `banc-obrain`) contre **98 s** de préfill pour le même contenu en texte (**MESURÉ** ici : 4,38 tok/s de préfill) |

**CALCULÉ : ~500×.** L'injection n'est pas seulement plus juste — elle est la seule voie praticable
sur CPU.

## 5 · Quatre défauts de l'instrument, trouvés en l'exécutant

1. `cycles` : « directement ou indirectement » rendait la phrase ambiguë — **la référence échouait
   4/4** sur cette famille. *Un juge que la référence rate n'est pas un juge.*
2. `redondance` : espérait qu'un chemin de 2 arêtes apparaisse par hasard — **0 question engendrée en
   72 tentatives**. Elle est maintenant **construite** (triangle `a→m→b`).
3. **Coût surestimé d'un ordre de grandeur** : le préfill domine (4,4 tok/s ⇒ ~98 s pour 430
   jetons), pas la génération. Graphes à **8-14 arêtes**, `--n ≤ 4`.
4. **Un `grep` sur la propre sortie du juge masque son erreur** : un `0/0` a été lu comme un défaut
   de générateur alors que les appels échouaient. **Règle : sortie complète, sans filtre.**

## 6 · Deux leçons d'exploitation

- **Un seul modèle lourd à la fois sur `n3`.** Deux agents chargeant des modèles en parallèle : le
  noyau a tué un `llama-server` par **OOM** (`SIGKILL`, deux occurrences dans le journal noyau) —
  et deux bras de mesure ont été perdus. *La machine n'est pas un nœud : personne ne l'arbitre*
  (cf. `docs/MACHINES-CONNUES-*`, `banc-obrain`).
- **Arrêter les serveurs par PID**, jamais `pkill -f` : la ligne de commande de son propre shell
  contient le motif, et le job se tue lui-même.

## 7 · Le maillon non conclu — NON VÉRIFIÉ

Le dernier bras (le substrat compose, le modèle lit **un saut à la fois**, l'union faite par nous) a
été perdu deux fois : une question non comparable au témoin, puis l'OOM. Commande à relancer
proprement (sortie complète, délai ≥ 10 min, **un seul modèle chargé**) :

```bash
cd build
./bin/llama-server -m /mnt/obrain/models/Qwen3.8-27B-OBRAIN-v2.gguf --port 8124 \
  -c 4096 -t 8 -ngl 0 --no-warmup --jinja &
python3 ../scripts/juge-structurel.py 8124 --n 2 --familles voisins --selection oracle    # témoin : 2/2
python3 ../scripts/juge-structurel.py 8124 --n 2 --familles atteignabilite --gestes 2 --selection oracle
```

**Si l'union des sauts atteint 75-100 % sans qu'un tenseur change, le bornage est RESTITUÉ par le
substrat** — et la suite est de donner la composition au graphe plutôt que des bits au modèle.
**NON VÉRIFIÉ à ce jour.**
