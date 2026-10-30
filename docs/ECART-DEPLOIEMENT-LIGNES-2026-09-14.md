# ÉCART ENTRE LE BINAIRE SERVI ET SA CONFIGURATION — 2026-09-14

*Constat établi en cherchant à débloquer la construction de l'index de
trigrammes sur le store vivant, comme le laissait ouvert le dernier commit du
tronc (`0794f2ac`). Le verrou nommé là — « l'hydratation du registre qui
manque » — n'est pas le bon : le binaire déployé ne lit tout simplement pas les
réglages qu'on lui a armés. Ce document mesure l'écart, nomme sa cause, et
laisse deux décisions ouvertes.*

Toutes les mesures sont prises sans arrêter ni écrire dans les bases servies.
Aucune base de production n'a été ouverte en écriture ; la seule écriture faite
l'a été sur une **copie** (`/tmp`, supprimée depuis).

---

## 1. Le binaire servi n'est pas celui de la lignée du tronc

`md5sum` identique, à l'octet :

| binaire | md5 | construit |
|---|---|---|
| `/home/triviere/.local/share/obrain-harness/bin/obrain` (servi) | `918c6614…` | 2026-09-14 13:24:48 |
| `banc-obrain-j5/target/release/obrain` | `918c6614…` | 2026-09-14 13:22:56 |
| `banc-obrain-j6/target/release/obrain` | `90ce4b70…` | 2026-09-14 14:09:16 |
| `banc-obrain/target/release/obrain` (tronc) | — | 2026-09-14 13:50 |

Le service `obrain-viz.service` tourne donc sur la lignée
`feat/principal-natif-j5`, pas sur le tronc.

Et le tronc ne peut même pas ouvrir les données servies :

```
$ ./target/release/obrain query --read-only <runtime-dev.obrain> "RETURN 1"
OBRAIN-X001: … unsupported format version 3 (expected 1)
```

Les bases ont été basculées en **v3** (colonne `tenant_id`, rupture 32 → 40
octets) le 2026-09-14 à 12:29 par la lignée `j2`→`j6`. Le tronc, lui, est resté
en `SUBSTRATE_FORMAT_VERSION = 1`.

## 2. Les réglages armés sont inertes — six sur huit

`strings` sur le binaire servi, contre l'environnement réel du processus
3621494 :

| réglage armé par l'unité / le drop-in | lu par le binaire servi |
|---|---|
| `OBRAIN_DEV` | ✅ |
| `OBRAIN_LLM_URL` | ✅ |
| `OBRAIN_IAM` | ❌ **inerte** |
| `OBRAIN_IAM_ACCOUNT` | ❌ **inerte** |
| `OBRAIN_IAM_STORE` | ❌ **inerte** |
| `OBRAIN_TRIGRAMMES_DES_LA` | ❌ **inerte** |
| `OBRAIN_TRIGRAMMES_PERSISTER` | ❌ **inerte** |
| `OBRAIN_PREFIXE_INTERVALLE` | ❌ **inerte** |

Trois preuves indépendantes, qui ne reposent pas sur `strings` :

* `nm -C` du binaire servi : **0** symbole `obrain_iam` ; aucune chaîne
  « IAM ARMÉ » ni « IAM NON COMPILÉ ». `grep -c iam` sur le
  `viz_serve.rs` de j5 : **0**. La lignée j5 **n'a pas** de chaîne IAM.
* Comportement : `/api/stores` et `/api/query` répondent **200 sans aucune
  identité** (mesuré le 14/09 à 14:14 et 14:17). Or `scripts/verif-chaine-iam.sh`
  exige **401** sans cookie — c'est le contrat de la chaîne armée.
* Le drop-in `trigrammes.conf` a été écrit le **14/09 à 13:35:38**, soit
  **11 minutes après** la construction du binaire qu'il prétend régler (13:24:48).

Ce que la lignée j5 utilise à la place (présent dans le binaire) :
`OBRAIN_CONTAINS_GARDE`, `OBRAIN_PROPS_INDEX_TRIGRAMMES`,
`OBRAIN_PROPS_INDEX_ORDONNE`, `OBRAIN_PROPS_INDEX_EGALITE`.

## 3. Les deux lignées ont implémenté la même chose, différemment

| | tronc / j4 (non déployé) | j5 / j6 (déployé) |
|---|---|---|
| index de sous-chaînes | `substrate.trigrammes.<prop>.v1`, un fichier par propriété | zone persistée `substrate.props.tri.idx.v2` (428 Mo sur dev) |
| module | `crates/obrain-substrate/src/props_trigram_index.rs` | `props_eq_index.rs` (zone d'égalité + trigrammes) |
| construction | adaptative, `OBRAIN_TRIGRAMMES_DES_LA`, persistance `OBRAIN_TRIGRAMMES_PERSISTER` | fusion à l'écriture (`fusionner_index_trigrammes`) |
| préfixe `STARTS WITH` | réécrit en intervalle, `OBRAIN_PREFIXE_INTERVALLE` | — |
| garde de coût | — | `OBRAIN_CONTAINS_GARDE` (seuil `max(4096, filigrane/8)`) |

Le commit `4ff85c8d` (j4, 12:33) a **fusionné le tronc** : la lignée j4 porte
donc v3 **et** les réglages du tronc. Mais j5 ne descend pas de j4
(`git merge-base --is-ancestor 4ff85c8d feat/principal-natif-j5` → faux ; j4 a
75 commits que j5 n'a pas), et **aucun binaire j4 n'a jamais été construit**
(`banc-obrain-j4/target/release/obrain` : absent).

## 4. Ce que ça donne à l'usure, sur le store servi

Même processus (le service), même code, même forme de requête, alias distincts
pour écarter le cache de requêtes :

| store | requête | mesuré |
|---|---|---|
| `runtime-prod` (statique) | `n.qualified_name CONTAINS 'iam'` | **0,54 / 0,47 ms** |
| `runtime-dev` (**vivant**) | `n.qualified_name CONTAINS 'iam'` | **53,9 / 63,2 ms** |
| `runtime-dev` | `… CONTAINS 'session-'` (277 590 résultats) | 380 / 261 / 256 ms |

Le store statique est servi par la zone de trigrammes ; le store **vivant** ne
l'est pas. C'est la question ouverte du commit `0794f2ac`, mais posée à
l'envers : ce n'est pas la construction qui manque, c'est la **série de
`qualified_name`** dans la zone (le code laisse délibérément une clé dehors
quand il ne peut pas prouver sa complétude — `store.rs`, `fusionner_index_trigrammes`).

Vérifié sur copie statique du store vivant (`/tmp`, 24 s de copie) : la même
requête coûte **~170–300 ms** dans un CLI, et **identique** que la zone de
trigrammes soit active ou non (`OBRAIN_PROPS_INDEX_TRIGRAMMES=0`), garde forcée
ou non (`OBRAIN_CONTAINS_GARDE=1`) — la route indexée **n'est pas empruntée**.

⚠️ **Une mesure à jeter, et je la jette.** Un relevé a affiché 936 µs / 157 µs /
143 µs sur cette copie. Il **ne s'est pas reproduit** (trois tentatives
ultérieures : 302/188/163, 291/283/284/285, 308/177/171 ms). Je le consigne
parce qu'un chiffre de cet ordre aurait suffi à conclure « résolu » : c'est
exactement la fausse alerte du 13/09 (`docs/PERF-REQUETES-LENTES-2026-09-13.md`),
où une régression imaginaire venait d'une requête de mesure modifiée.

⚠️ **Réserve de bruit** : ces mesures ont été prises pendant qu'un `obrain
bridge` d'une autre session occupait un cœur (2,5 Go RSS, ~72 % CPU) et pendant
la copie de 12 Go. Les écarts *entre* les deux stores se sont reproduits (deux
campagnes espacées), les valeurs absolues restent suspectes.

## 5. Pourquoi aucun entretien ne répare ça

`obrain index rebuild` existe et fait exactement l'acte attendu : « Rebatit
depuis les chaines, puis persiste, les index de toutes les proprietes declarees
au catalogue (egalite + trigrammes). L'acte de maintenance qui donne ses series
a une zone d'index introduite apres l'indexation du store ». Sur une copie, il a
rebâti **2 propriétés** en 3,1 s — et `qualified_name` n'en fait visiblement pas
partie (la requête reste lente après).

Or :

* `scripts/entretien-bases.sh` ne contient **aucun** `index rebuild` : ses deux
  passes sont `repair-degrees` et `repair` ;
* il vit dans le **tronc** et pointe sur `./target/release/obrain` — un binaire
  v1, incapable d'ouvrir les bases v3 ;
* il n'est appelé que par `scripts/vizserve.sh` (lancement manuel), **pas** par
  l'unité systemd, qui est le déploiement réel.

Autrement dit : l'acte qui donnerait ses séries au store vivant n'est dans aucun
chemin de redémarrage — et la lignée déployée n'a même pas le script.

## 6. Ce qui a été changé dans ce dépôt

`scripts/verif-env-deployees.sh` — une garde de la même famille que
`verif-chaine-iam.sh` : elle confronte **ce qui est armé** (l'environnement du
processus servi, ou l'unité et ses drop-ins à défaut) à **ce que le binaire
lit** (`strings`, lecture en une passe, sous-chaîne et non égalité — deux noms
collés dans `.rodata` rendent sinon un faux « absent »).

```
$ bash scripts/verif-env-deployees.sh          # binaire servi → 1, 6 inertes
$ bash scripts/verif-env-deployees.sh target/release/obrain   # tronc → 0
```

Elle accuse le binaire déployé et blanchit celui du tronc : c'est la mesure de
l'écart, et de lui seul.

## 7. Décisions ouvertes

1. **Quelle lignée fait référence ?** Le tronc (v1, réglages, non déployable sur
   les bases v3), j4 (v3 + réglages du tronc, jamais construit), ou j5/j6 (v3,
   zone de trigrammes + garde de coût, déployé) ? Tant que ce n'est pas tranché,
   chaque session ajoute du travail à une lignée que personne ne sert.
2. **Que fait-on des six réglages inertes ?** Les faire vivre (construire et
   déployer un binaire qui les lit) ou les retirer du déploiement. Les laisser
   est le seul choix qui ne se défend pas : ils font attribuer au réglage un
   comportement qu'il n'a pas — c'est déjà arrivé pour l'IAM, trois fois.

---

# SUITE (2026-09-14, après-midi)

*Les deux décisions ci-dessus ont été prises : **consolider le tronc DANS la
lignée servie**, et **faire vivre les réglages** (un binaire déployé qui les
lit). Cette section dit où en est la consolidation, et ce qu'elle donne
**mesuré**.*

## 8. La consolidation : branche `fusion/tronc-plus-j6`

Prise sur j6 `cb80e781`, **sans toucher à j6** (une autre session y travaille :
son sommet a bougé deux fois pendant la fusion, `cb80e781` puis `573989a4`).
Commits `073ef065`, `bf97ea2c`, `63e86069`.

**Portes tenues** : `cargo check --release` vert sur substrat (`--all-targets`),
moteur et CLI (`--features ai,iam`) ; `cargo test -p obrain-substrate --lib`
**874 passés / 0 échec** ; `cargo test -p obrain-engine` **758 passés / 0 échec**.

Trois défauts trouvés EN CHEMIN, et pas par relecture — par les tests :

1. **rerere avait perdu la zone de trigrammes de j6** (`props_eq_index.rs`)
   puis, ailleurs, **la route `CONTAINS` de la lignée servie**
   (`try_plan_filter_with_contains`) : la consolidation héritait du planificateur
   du tronc, dont la route est INERTE par défaut (`trigrammes_des_la = u32::MAX`,
   l'index n'est jamais bâti sans réglage). Déployer en l'état aurait régressé le
   store de production de 0,47 ms à un balayage. Les deux routes coexistent
   désormais, celle du tronc d'abord, celle de la zone ensuite.
2. **Le bloc « préfixe → intervalle » du tronc** manquait aussi, avec son
   interrupteur `OBRAIN_PREFIXE_INTERVALLE` — d'où un réglage encore inerte après
   la première passe de fusion. j6 a sa propre réécriture (`prefix_range`, qui
   refuse en plus les préfixes analysables en date-heure) : c'est elle qui sert,
   et l'interrupteur est porté dessus (`OnceLock`, une lecture par processus).
3. **Un contrat de moteur divergeait entre les lignées** :
   `labels(n) CONTAINS 'X'`. j6 : opérateur de chaîne, une liste rend 0 ligne, et
   un test interdit de réécrire la forme en scan d'étiquette. Tronc : ce `None`
   rejetait la ligne EN SILENCE (mesuré : sept requêtes de plus et 181 s de
   conversation), donc `CONTAINS` sur une liste évalue l'APPARTENANCE. La
   consolidation fait entrer le correctif du tronc ; le test de j6 est réécrit
   (pas supprimé) et épingle l'équivalence des trois formes, plus la frontière
   qui reste : `labels(n) CONTAINS 'Tas'` ne retient rien.

**Ce changement de contrat est à l'attention de l'opérateur** : il peut être
défait (revenir à `as_str()?`), au prix du faux négatif silencieux que `79a77fd2`
a corrigé.

## 9. Ce que ça donne, mesuré

Binaire de la fusion (`cargo build --release -p obrain-cli --features ai,iam`),
**copie** du store vivant prise le 14/09 à 15:26 (`/mnt/obrain`, 12 Go, 48 s),
`obrain shell --timing` pour des mesures DANS LE MÊME processus :

| configuration | `qualified_name CONTAINS 'iam'` | lignes |
|---|---|---|
| défaut (routes de la ligne servie) | 532 · 180 · 169 ms | 417 |
| index coupés (`OBRAIN_TRIGRAMMES=0`, `…INDEX_TRIGRAMMES=0`) | 380 · 177 · 181 ms | 417 |
| **tronc armé** (`…DES_LA=1`, `…PERSISTER=1`) | 2,82 s (construction) · **1,0 ms · 0,82 ms** | 417 |
| tronc armé, **2ᵉ processus** (index persisté, 26 Mo) | **2,7 · 1,3 · 1,2 ms** | 417 |

**Les résultats sont identiques dans TOUTES les configurations** — c'est la
clause qui compte : ni faux négatif, ni ligne inventée. Et le gain est là où il
était annoncé : ×130 environ, une fois l'index bâti et persisté.

⚠ **Le motif PEU SÉLECTIF ne gagne rien, et c'est mesuré deux fois** :
`CONTAINS 'session-'` (280 869 lignes) — 195 · 162 ms en **défaut** (route de j6,
garde de coût active), 163 · 164 ms **index armé** (route du tronc). Les deux
routes CONVERGENT : le coût est dominé par la vérification des candidats
(280 869 × ~0,6 µs), pas par la façon de les obtenir.

**Donc la garde de coût n'est PAS étendue à la route du tronc** — non par
principe, mais parce que la mesure ne la justifie pas ici : elle basculerait vers
un balayage qui coûte le même prix. Si un store plus gros (ou un motif encore
plus fréquent) creusait l'écart, c'est ce chiffre-là qu'il faudrait reprendre.

### Le fichier persisté est l'artefact durable — les réglages ne servent qu'à le BÂTIR

Le point opérationnel qui décide de tout, et il a été mesuré après coup : une
fois `substrate.trigrammes.qualified_name.v1` écrit (26 Mo), la configuration
**par défaut** — aucun réglage armé — sert la requête rare en **100,7 ms puis
3,9 ms**, là où elle balayait à 180 ms. Les deux réglages (`…DES_LA`,
`…PERSISTER`) ne sont donc nécessaires qu'à la CONSTRUCTION ; ensuite le fichier
suffit, et tous les processus en profitent (mesuré : 2ᵉ processus armé,
5,7 · 1,4 · 3,6 ms).

⚠ **Mais le fichier n'est utilisable que si sa fenêtre est PROUVÉE** : il est
refusé dès que le WAL a avancé de plus de `PLAFOND_DELTA_FENETRE` (1 Mio) depuis
son ancre — le store vivant écrit en continu, donc le fichier de la copie ne le
servira pas longtemps. C'est précisément pourquoi l'acte de construction doit
être attaché au **redémarrage** (§10) et non fait une fois à la main : il faut le
rafraîchir là où la base est quiescente.

Sur le **store vivant lui-même**, en `--read-only` (jamais ouvert en écriture) :
le binaire de la fusion rend **417 lignes**, les mêmes. Aucun fichier d'index n'a
été écrit à côté de la base servie (0 fichier `substrate.trigrammes.*`, WAL du
store non touché par la mesure).

## 10. Ce qui reste avant de déployer

- **La chaîne IAM PASSE, de bout en bout** (mesuré le 14/09 à 16:08) :
  `scripts/verif-chaine-iam.sh` → **« CHAÎNE COMPLÈTE — tout passe »** —
  binaire contenant l'IAM, compte amorcé, garde armée, connexion par formulaire
  + cookie, requête par la voie du navigateur, `whoami`, piste d'audit,
  renouvellement avec rotation, ancien jeton mort, déconnexion, et le 401 qui
  annonce `WWW-Authenticate: Bearer`. Autrement dit : **le binaire consolidé
  porte une chaîne IAM qui marche**, là où le binaire servi n'en a aucune.
  (Piège rencontré : le binaire d'**exemple** `amorcer_iam` était resté en v1, ce
  qui faisait échouer l'amorçage et, en cascade, huit étapes. Le script échoue
  proprement, mais dit « amorçage » là où la cause est le binaire d'exemple.)
- **Le smoke du dashboard et `acceptation-8432.sh` ne sont pas passés** : la
  consolidation est validée par les tests (substrat 874, moteur 707 + intégration,
  une seule défaillance préexistante et attribuée) et par la chaîne IAM, pas par
  une session de navigateur.
- **j6 a avancé depuis la base de fusion** : rattrapé une fois (`573989a4`
  min/max, `911aa9ba` ORDER BY, mergés dans `e3d70cb6`, un conflit dans
  `aggregate.rs`, résolu par union). Il continue d'avancer — sa session est
  active — donc **toute fusion est périmée dès qu'elle est écrite** : le
  déploiement doit être précédé d'un dernier rattrapage, ou attendre que j6 se
  pose.
- **L'index doit être bâti sur le store servi**, et le moment propre pour cela
  est la **fenêtre de redémarrage** — c'est la doctrine d'`entretien-bases.sh` :
  une base est libre entre l'arrêt et le lancement, et c'est le seul instant où
  l'acte ne coûte rien à personne. `viz serve` préchauffe déjà les index
  textuels en tâche de fond au démarrage : avec le binaire consolidé et
  `OBRAIN_TRIGRAMMES_PERSISTER=1` (déjà armé dans le drop-in), **la construction
  et la persistance se font au démarrage**, et le fichier (26 Mo, écrit par
  temporaire puis `rename` — un CACHE à côté des données, jamais dedans) sert
  ensuite tous les processus.

## 11. La séquence de déploiement, si elle est autorisée

1. **Dernier rattrapage de j6**, recompilation (`cargo build --release -p
   obrain-cli --features ai,iam`), et **les deux portes** : la garde des réglages
   armés (`scripts/verif-env-deployees.sh` → 0) et la chaîne IAM.
2. **Sauvegarde du binaire servi** (`bin/obrain` → `bin/obrain.avant-fusion-<date>`),
   pour un retour en une commande.
3. **Arrêt du service** (`systemctl --user stop obrain-viz`) : c'est la fenêtre
   d'exclusivité, et elle est aussi celle de l'entretien.
4. **Construction et persistance de l'index** sur les stores servis, hors ligne,
   pendant que la base est libre.
5. **Installation du binaire consolidé** et **redémarrage**.
6. **Vérification** : garde verte, `obrain viz serve` répond, dashboard servi,
   puis la mesure qui compte — `qualified_name CONTAINS` sur le store vivant,
   avec le relevé des lignes pour comparaison avant/après.

Le point 6 est la seule preuve qui reste à produire : tout le reste est mesuré,
mais **sur une copie**.
