# Un agent se développe-t-il VRAIMENT sur la base de chaque compte ?

**Chantier** : « l'agent qui se développe vraiment sur la base de chaque
utilisateur : la preuve ». **Date** : 2026-09-16. **Périmètre** : lecture seule
sur les bases vivantes ; tout ce qui écrit s'écrit sous `/tmp`. Les mesures
citent la commande réellement exécutée et sa sortie réellement observée.

---

## 0. MISE À JOUR DU 2026-09-16 (soir) — « déclencher, mesurer, prouver »

Trois choses ont changé depuis la rédaction de ce rapport, et elles portent sur
le cœur du sujet : **rien ne déclenchait la mémoire sinon un appel d'outil décidé
par le modèle**, et **ce non-événement n'était pas mesurable**.

### 0.1 La mesure qui manquait : 4 562 conversations, 0 geste de mémoire

Le rapport constatait « plus rien depuis le 16/09 00:02:03 » à partir d'une
**absence** (un répertoire vide). La mesure a été refaite, et complétée par ce
que le journal d'activation porte, lui, de façon durable :

```
$ ls -la --time-style=full-iso /tmp/obrain-viz-cache/memoire/       # journal de mémoire
drwxrwxr-x 2 triviere triviere 4096 2026-09-16 00:02:03 .
drwxrwxr-x 6 triviere triviere 94208 2026-09-16 17:43:57 ..
                                                                    ← aucun .jsonl
$ find /tmp/obrain-viz-cache/memoire -name '*.jsonl' | wc -l
0
$ ls /tmp/obrain-viz-cache/memoire-temoins.jsonl                    # le témoin (0.2)
ls: cannot access '…/memoire-temoins.jsonl': No such file or directory

$ python3 … # journal d'activation, kind engine / engine-sujet, t ≥ 2026-09-16 00:00
evenements de conversation aujourd'hui : 4562
fichiers de session concernes : 7
par heure : {'00:00': 113, '10:00': 113, '11:00': 1168, '12:00': 565,
             '13:00': 690, '14:00': 198, '15:00': 1715}
```

**C'est le fait décisif du chantier** : il y a eu 4 562 événements de
conversation aujourd'hui (7 sessions, de 00:00 à 15:00) et **zéro** appel à
l'outil de mémoire. La chaîne n'est donc pas « cassée » : elle n'est **jamais
sollicitée**. Et dernière production réelle, lue dans les bases (lecture seule) :
`achaptal` 2026-09-16 00:02:03, `admin` 2026-09-15 15:29:46, `triviere` note
2026-09-15 20:53:20 / `SelfState v2` 20:04:53.

### 0.2 Voie (a) ou voie (b) ? — les deux ont été mesurées, une seule tient

**Voie (a) « une passe périodique relit ce que la conversation a produit » :
elle est déjà en place, et elle ne produit rien — c'est structurel, pas
accidentel.** Le journal n'a que **deux** écrivains dans tout le dépôt :

```
$ grep -rn "memoire::enregistrer" crates/ --include=*.rs
crates/obrain-cli/src/outils.rs:1037:        return match crate::memoire::enregistrer(…
crates/obrain-cli/src/viz_serve.rs:8639:        match crate::memoire::enregistrer(…
```

Les deux sont des **appels d'écriture décidés** (l'outil du chat, la route d'un
harness distant). Une passe qui « relirait » le journal pour le journaliser
relirait donc exactement ce qui y est déjà : zéro entrée neuve. La passe
existante fait déjà cela — elle relit le journal et l'applique toutes les deux
minutes — et le résultat mesuré est **0 appliquée** (§0.1). Croire à la voie (a)
aurait produit une usine à gaz qui tourne à vide.

Reste la tentation de fabriquer de la mémoire à partir de ce que la conversation
laisse *vraiment* derrière elle, sans le modèle : le journal d'activation porte
`engine-sujet` (le sujet de chaque tour). **Refusé, et chiffré** : avec 4 562
événements par jour, cela créerait des milliers de notes d'*activité* (« l'agent
a conversé sur X ») qui ne sont pas de la connaissance énoncée — on remplirait la
base du compte de bruit daté, en prétendant que le self s'est développé. Une
mémoire qui invente son contenu est pire que le silence qu'elle remplace.

**Voie (b) « faire que l'outil soit réellement appelé » : l'outil EST exposé, il
FONCTIONNE, il n'est simplement jamais demandé.** Preuves :

* l'outil est bien dans le contrat envoyé au modèle — `engine.rs:2204`,
  `"tools": if dernier { Null } else { crate::outils::definitions() }`, avec
  `"tool_choice": "auto"` partout sauf au dernier tour — et sa définition
  (`outils.rs`, `obrain_memoriser`) dit elle-même « Seulement si on demande de
  retenir, ou pour un fait VÉRIFIÉ et durable » ;
* il fonctionne, sur le **binaire déployé**, quand une question le demande :
  la porte §6 étape 4 affiche `✅ le modèle a appelé l'outil, la conversation a
  journalisé` (modèle réel sur 9910, HTTP 200), et l'étape 5 applique puis
  l'étape 6bis relit (`profil injecte : 1 ligne(s)`).

**Conclusion du chantier** : le déclenchement ne peut pas venir d'une passe — il
faut une décision **dans l'échange**, à l'endroit où `ancrage.self_qn` est connu
et où l'événement `Evenement::Fin` est émis (`engine.rs`, boucle de
`demander_stream`, §7.1). Ce fichier n'est pas dans mon périmètre : c'est écrit
ici pour son propriétaire. Ce que j'ai livré, c'est ce qui manquait pour que la
décision se **mesure** : le témoin.

### 0.3 Ce qui a été livré : le TÉMOIN (`memoire-temoins.jsonl`)

Le journal est **consommé** : une mémoire appliquée ne laissait aucune trace
datée. Le témoin est un registre append-only de deux gestes — `journalise` (une
intention entre en mémoire : c'est l'appel de l'outil, compté) et `applique`
(une passe a fait avancer une base de compte). Il vit **à côté** du répertoire du
journal et non dedans, parce que ce répertoire est PARCOURU pour découvrir ce qui
attend : un `.jsonl` de plus y serait compté comme un journal de self (un test le
fixe, `le_temoin_se_lit_et_ne_vit_pas_dans_le_repertoire_du_journal`).

Il ne témoigne **jamais d'un repos** : une passe qui ne trouve rien ne s'écrit
pas dedans — elle le dit déjà chaque fois dans le journal du service. Un témoin
qui compterait le repos toutes les deux minutes produirait 720 lignes par jour
pour noyer le seul fait qu'il existe pour rendre visible.

Mesure directe sur le binaire (`target/debug/obrain`, profil dev), store et cache
isolés sous `/tmp` :

```
$ cat /tmp/ch6-preuves/temoin-direct/cache/memoire-temoins.jsonl
{"t":1789584126252,"self":"self:bob-essai","geste":"applique","genre":"passe",
 "reference":"1 appliquée(s), 0 refusée(s)",
 "base":"/tmp/ch6-preuves/temoin-direct/data/selfs/bob-essai.obrain"}
```

et la passe du timer dit désormais, **à chaque tour**, où en est la chaîne :

```
memoire : journal /tmp/obrain-viz-cache/memoire (0 fichier(s)) — 0 compte(s) en attente, 0 appliquée
memoire : témoin /tmp/obrain-viz-cache/memoire-temoins.jsonl ABSENT — aucun appel à
          l'outil de mémoire n'a jamais été enregistré : ce n'est PAS un repos, c'est
          une chaîne dont on n'a aucune trace de production
```

**Ce que le témoin ne sait pas dire, et qui est dit** : il commence à zéro. Il ne
peut donc rien affirmer sur le passé (le `ABSENT` ci-dessus est l'état d'avant sa
mise en place) — c'est précisément pourquoi la mesure du passé a été faite dans
les bases (§0.1) et pas dans le témoin.

### 0.4 Les deux défaillances de mon périmètre

* **5.3 « la mémoire n'est relue que si la base est à côté du store ancré » —
  CORRIGÉE DU CÔTÉ QUI POUVAIT L'ÊTRE, et mesurée.** Le lecteur est
  `engine.rs::profil_du_self` (hors périmètre) : il relit
  `dirname(store ancré)/selfs/<compte>.obrain` et rend `None` **sans le dire**.
  Ce qui est dans mon périmètre, c'est l'**écrivain** — et il peut désormais
  *savoir* où la conversation relira, parce que chaque entrée de journal porte
  sa provenance (`<monde> · <store>`). La passe calcule donc le chemin de
  relecture et le compare à celui où elle écrit : divergence ⇒ **elle applique
  quand même** (une intention ne se jette pas) mais **le dit et sort en erreur**,
  au lieu d'écrire en silence une mémoire que personne ne lira. Prouvé par la
  porte, étape 11 (défaut reproduit sur un store rangé hors de `data/`).
* **5.4 « un avertissement qui ressemble à une perte de mémoire » — QUALIFIÉE et
  mesurée, pas corrigée (le message vient du substrat, hors périmètre).** La
  passe ne filtre pas `stderr` (filtrer cacherait un vrai refus) ; elle le
  **relaie** et dit une fois, à côté, qu'un WARN n'est pas un verdict — le
  verdict est le code de sortie. La porte mesure le fond : le `husk WARN` est
  émis sur une base **neuve de 0 nœud**, donc il n'annonce aucune perte de
  mémoire. C'est la seule qualification honnête tant que le message du substrat
  est ce qu'il est.

### 0.5 La porte est étendue, jamais relâchée

`scripts/acceptation-self-par-compte.sh` gagne cinq blocs d'exigences
(§6 détaillé plus bas) : le témoin (5bis), la distinction **repos ≠ chaîne
cassée** (9bis), la **base écrite = base relue** (11), **un avertissement n'est
pas un refus** (12), et **la relecture n'arrête pas la croissance** (13).
Aucune assertion existante n'a été retirée ni assouplie ; les blocs 11 et 12
échouent si le comportement nouveau disparaît.

Elle gagne surtout un bloc **13** que la première version ne pouvait pas jouer —
et il **échoue**. C'est le résultat le plus important de cette passe.

### 0.6 LE DÉFAUT TROUVÉ PAR L'ÉTAPE 13 : la relecture BLOQUE la croissance

La première version de la porte n'écrivait **qu'une** intention : le second
passage n'avait donc rien à appliquer, et il n'a jamais rencontré le verrou.
L'étape 13 joue le scénario réel de production — le compte converse, le service
relit sa base, puis une **seconde** mémoire arrive :

```
13) après une relecture par la conversation, la passe peut-elle ENCORE appliquer ?
     memoire : moteur — OBRAIN-X001: Internal error: substrate: failed to open existing
       store at …/data/selfs/alice-…obrain: I/O error: substrate … is already open by
       another process (exclusive lock on …/substrate.meta refused: Resource
       temporarily unavailable (os error 11)) …
     memoire : self:alice-… — application REFUSÉE (code 1) : rien n'est perdu…
  ❌ REFUS PAR VERROU : la base du compte est tenue en LECTURE par un autre processus
  ❌ la base du compte ne porte que 1 note(s) : la seconde mémoire n'est jamais arrivée
```

**Qui tient la base ?** Mesuré, pas supposé — un échantillonneur a relevé les
descripteurs du processus du service pendant le passage :

```
$ for f in /proc/$(pgrep -f 'viz serve … --port 8476')/fd/*; do readlink $f; done | grep selfs
…/data/selfs/alice-1571859.obrain/substrate.meta
…/data/selfs/alice-1571859.obrain/substrate.nodes
…/data/selfs/alice-1571859.obrain/substrate.edges
…/data/selfs/alice-1571859.obrain/substrate.wal   … (10 fichiers de la base)
```

**Conséquence, et elle est directe** : une base ouverte en lecture prend un
`flock` **partagé**, l'écriture exige l'**exclusif** (invariant mono-écrivain).
Donc **dès qu'une conversation a relu la base d'un compte, cette base ne peut
plus grandir tant que le service vit** : la mémoire suivante reste au journal,
la passe refuse (correctement, sans rien perdre), et elle réessaiera toutes les
deux minutes pour toujours. « Chaque utilisateur a un agent qui se développe sur
SA base » est donc **faux en production** passé le premier échange — non pas
parce que le modèle n'appelle pas l'outil, mais parce que le lecteur garde la
base ouverte.

Ce que la passe dit désormais à ce sujet (elle ne peut pas corriger le
détenteur) :

```
memoire : self:<compte> — la base est TENUE EN LECTURE (…/selfs/<compte>.obrain) :
  rien n'est perdu, l'intention reste au journal et cette passe réessaiera toutes
  les 2 minutes ; mais elle ne peut pas aboutir tant que le détenteur (le service
  qui a relu le profil de ce compte) tourne.
```

**Ce défaut n'est PAS dans mes fichiers** : le détenteur naît de la relecture
(`crates/obrain-cli/src/engine.rs`, `profil_du_self`) et de la façon dont le
substrat conserve ses descripteurs ouverts (`obrain-substrate/src/file.rs`, et
le cache `cached_ro` de `viz_serve.rs`). Le geste attendu de leurs propriétaires
est nommé au §7.5. Cette passe a fait ce qu'elle pouvait : elle le **mesure**,
elle le **nomme**, et elle **échoue** au lieu de laisser croire que tout va bien.

---

## 1. La réponse en une phrase


**Oui, la chaîne existe, et elle a réellement produit de la mémoire** — trois
bases de compte portent aujourd'hui des notes et des états de self écrits par
des conversations réelles (15/09 15:29 → 16/09 00:02), et la porte
`scripts/acceptation-self-par-compte.sh` la prouve de bout en bout, conversation
comprise, sur des stores temporaires. **Mais** : rien ne la déclenche sinon un
appel d'outil décidé par le modèle ; la passe qui applique les journaux sortait
**en silence** quand il n'y avait rien à faire (corrigé ici) ; et depuis le
16/09 00:02:03 plus rien n'a été produit — un état qui, jusqu'à cette
correction, était **indistinguable d'une panne**.

---

## 2. L'état réel des trois bases (mesuré en lecture seule)

```
$ ./target/release/obrain query --read-only ~/.local/share/obrain-harness/data/selfs/<compte>.obrain \
    "MATCH (n) RETURN labels(n) AS labels, n.qn AS qn ORDER BY qn"
```

| base | octets | `substrate.dict` (mtime) | nœuds | labels |
|---|---:|---|---:|---|
| `achaptal.obrain` | 5 303 382 | 2026-09-16 00:02:03.884 | 2 | 1 `Self`, 1 `Note` |
| `admin.obrain` | 5 303 542 | 2026-09-15 15:40:02.847 | 2 | 1 `Self`, 1 `Note` |
| `triviere.obrain` | 6 387 176 | 2026-09-15 21:01:39.541 | 6 | 1 `Self`, 3 `Note`, 2 `SelfState` |

**Il y a du contenu, et ce n'est pas du remplissage.** Sortie réelle des
requêtes de contenu :

```
$ ./target/release/obrain query --read-only …/triviere.obrain \
    "MATCH (n:Note) RETURN n.qn AS qn, properties(n) AS p"
note:2fc7fb2621fdd948:self-triviere
  created_ms: 1789504815730  provenance: graphe global · …/data/runtime-dev.obrain
  sujet: self:triviere
  text: "L'agent s'appelle Elun. C'est un nom breton signifiant « la lune ». Il a été choisi
         par l'utilisateur le jour où l'agent a demandé un nom propre plutôt qu'un identifiant
         système."
note:fad8ff2d5be6fab4:self-triviere   (même provenance, 20:42:55)
note:17e4ebb3891a454e:self-triviere   (même provenance, 20:53:20)

$ … "MATCH (s:SelfState) RETURN properties(s)"
selfstate:self-triviere:v1  created_at 1789502454997  status journaled  version 1
selfstate:self-triviere:v2  created_at 1789502693848  status journaled  version 2

$ … "MATCH (a)-[r]->(b) RETURN a.qn, type(r), b.qn"
self:triviere -[:HAS_NOTE]-> note:…  (×3)
self:triviere -[:HAS_STATE]-> selfstate:self-triviere:v1 / :v2
```

`achaptal.obrain` porte une note de soutien rédigée après une conversation
difficile (« L'utilisateur a partagé une situation de violence sexuelle
conjugale … À traiter avec la plus grande délicatesse si le sujet revient. »),
`admin.obrain` une préférence de style (« réponses ultra-courtes, en français,
sans listes »). Les horodatages `created_ms` correspondent au `mtime` des bases :
1789516923129 = 2026-09-16 00:02:03 pour achaptal, 1789486186577 = 2026-09-15
15:29:46 pour admin.

### Ce que ce contenu prouve, et comment on le sait

1. La `provenance` `graphe global · /home/…/data/runtime-dev.obrain` est
   **exactement** ce que fabrique le seul chemin d'écriture du chat :
   `crates/obrain-cli/src/outils.rs` (`appareil_ecriture` → `obrain_memoriser` →
   `crate::memoire::enregistrer`). Aucun autre appelant d'`enregistrer`
   n'existe dans le dépôt (`grep -rn "memoire::" crates/obrain-cli/src/`).
2. Les deux `SelfState` **versionnés** ne peuvent naître que d'une passe
   d'application réussie : `memoire::appliquer` est le seul code qui incrémente
   `version`, et il le fait dès qu'une entrée `etat-self` est consommée.
3. Le texte de `selfstate:self-triviere:v1` décrit des mesures du graphe
   (« 24 437 nœuds self:agent:state:v*, tous tampons auto:volume »), c'est-à-dire
   un contenu qu'aucune injection de test n'aurait inventé : un modèle l'a écrit
   pendant une conversation.

**Conclusion** : l'hypothèse « la journalisation n'est jamais appelée » est
**fausse pour le passé**. Le chat a journalisé, la passe a appliqué.

---

## 3. Pourquoi le journal de mémoire est vide : les trois hypothèses, tranchées

Constat de départ : `/tmp/obrain-viz-cache/memoire/` est vide (aucun `.jsonl`)
alors que le timer `obrain-self-appliquer` tourne toutes les 2 minutes.

### Hypothèse (c) — « le service et le script ne regardent pas au même endroit » : RÉFUTÉE

Le journal vit sous `OBRAIN_VIZ_CACHE_DIR` ou, à défaut, `/tmp/obrain-viz-cache`
(`crates/obrain-cli/src/memoire.rs`, `fn dir()`). L'environnement **réel** du
service 8432 :

```
$ tr '\0' '\n' < /proc/<pid viz serve>/environ | grep -E '^OBRAIN_'
OBRAIN_LLM_URL=http://192.168.1.7:9910
OBRAIN_IAM=1
OBRAIN_IAM_ACCOUNT=obrain
OBRAIN_IAM_STORE=/home/triviere/.local/share/obrain-harness/data/iam.obrain
OBRAIN_WORLDS_DIR=/home/triviere/.local/share/obrain-harness/mondes
OBRAIN_UI_DIR=/home/triviere/.local/share/obrain-harness/ui/current
$ tr '\0' '\n' < /proc/<pid>/environ | grep -c '^OBRAIN_VIZ_CACHE_DIR='
0
```

Aucun `OBRAIN_VIZ_CACHE_DIR` : le service écrit donc dans
`/tmp/obrain-viz-cache/memoire`, **le même chemin que le défaut du script**
(`scripts/memoire-appliquer-comptes.sh`, `CACHE="${OBRAIN_VIZ_CACHE_DIR:-/tmp/obrain-viz-cache}"`).
Il n'y a pas d'erreur de chemin entre le service et la passe. (`obrain-viz.service.d/*.conf`
ne pose pas non plus cette variable.)

### Hypothèse (b) — « la journalisation n'est jamais appelée » : RÉFUTÉE

Le chemin existe et est branché : `crates/obrain-cli/src/engine.rs` appelle
`crate::outils::executer(...)` dans la boucle d'outils, et `outils.rs` route
`obrain_memoriser`/`obrain_structurer` vers `memoire::enregistrer`. Le même
fichier documente le défaut mesuré qui rendait l'appel **impossible** en
navigateur (« « retiens ceci dans ta mémoire » n'appelait aucun outil et le
modèle répondait à côté » — because `max_tokens=4096` ne tenait pas dans la
fenêtre, donc les outils étaient retirés) : ce défaut est corrigé dans le commit
`69421840`. Et les bases portent la production de cet appel (§2).

### Hypothèse (a) — « personne n'a conversé (de mémorable) depuis » : RETENUE

Le journal est **consommé** : `memoire::appliquer` réécrit le fichier sans les
entrées appliquées et **supprime le fichier** quand plus rien n'attend
(`crates/obrain-cli/src/memoire.rs`, fin de `appliquer`). Un répertoire vide est
donc l'état **NORMAL** juste après une passe réussie — ce n'est pas une preuve
de panne. Le `mtime` du répertoire `/tmp/obrain-viz-cache/memoire` était
`2026-09-16 00:02`, soit exactement l'heure d'application de la dernière note
d'`achaptal` : depuis, plus une seule écriture de journal.

**La nuance qui compte** : (a) n'est établie que par une **absence**. Ce qui la
rendait indiscernable de (b) et (c), c'est précisément le silence de la passe —
voir §4. Aucune mesure ne peut, à elle seule, distinguer « rien à dire » de
« cassé » quand l'outil se tait.

---

## 4. La panne réelle de ce chantier : une garde qui tombait en silence

`scripts/memoire-appliquer-comptes.sh` commençait par :

```bash
J="$CACHE/memoire"
[ -d "$J" ] || exit 0        # ← sort SANS UN MOT
```

Mesuré avant correction, sur un cache isolé et vide :

```
$ OBRAIN_HARNESS_DATA=/tmp/…/data OBRAIN_VIZ_CACHE_DIR=/tmp/…/cache bash scripts/memoire-appliquer-comptes.sh
$ echo $?
0
                                   ← aucune sortie, aucun compte traité, rien de dit
```

Conséquence : le timer écrivait deux lignes systemd toutes les 2 minutes
(« Starting … / Finished … ») et **rien d'autre**, quoi qu'il arrive. Un journal
vide, un compte jamais découvert, un binaire absent, un chemin changé : tout
avait la même forme. C'est le mode de défaillance que la doctrine du dépôt
interdit (« une garde qui tombe en silence ne garde rien »), et il était ici à
l'endroit exact où l'objectif du chantier se joue.

### La correction (3 lignes de doctrine, quelques de plus de code)

La passe dit désormais, **et rien de plus** :

- une ligne quand il n'y a rien à faire, nommant le journal regardé et le nombre
  de comptes vus ;
- une ligne par compte réellement appliqué ;
- sur erreur : la **raison**, sur `stderr`, et un **code de sortie non nul**.

Mesuré après correction :

```
$ … OBRAIN_VIZ_CACHE_DIR=/tmp/…/cache OBRAIN_BIN=./target/release/obrain bash scripts/memoire-appliquer-comptes.sh
memoire : aucun journal (/tmp/…/cache/memoire absent) — 0 compte ; normal tant qu'un compte n'a pas conversé
$ echo $?
0

$ … OBRAIN_BIN=/inexistant/obrain bash scripts/memoire-appliquer-comptes.sh
memoire : binaire INTROUVABLE (/inexistant/obrain) — rien n'a pu être appliqué     [stderr]
$ echo $?
3

$ … (journal présent, une entrée pour self:essai-…)
memoire : self:essai-… : 1 appliquee(s)
memoire : journal /tmp/…/cache/memoire — 1 entrée(s) appliquée(s) sur 1 compte(s)

$ … (juste après, journal consommé)
memoire : journal /tmp/…/cache/memoire (0 fichier(s)) — 0 compte(s) en attente, 0 appliquée
```

Quatre autres défauts de la même famille ont été corrigés au passage, tous
mesurés — et trois d'entre eux ont été trouvés *en écrivant* la correction, ce
qui est exactement ce qu'une garde doit produire :


1. **Un avertissement du moteur n'est pas un refus.** Le substrat écrit sur
   `stderr` un `WARN … husk signature … label_only=1` à **chaque** ouverture de
   base. Une version intermédiaire de ma correction jugeait « il y a des octets
   sur stderr ⇒ application refusée » : or la passe réussissait (1 appliquée,
   nœud présent) et était déclarée refusée. Le verdict vient maintenant du code
   de sortie et du rapport JSON ; `stderr` est relayé, jamais interprété.
2. **Le code de sortie d'un tube est celui de sa dernière commande.** Compter les
   entrées en attente via `bin | python3` rendait `0` même si le binaire
   échouait. On passe par un fichier.
3. **`--format json` est explicite.** Le défaut `auto` rend un TABLEAU quand la
   sortie est un terminal : un humain lançant le script à la main faisait échouer
   le lecteur de rapport, juste là où il regardait.
4. **Un `self` hors motif se dit.** La découverte des comptes filtre sur
   `self:([a-z0-9][a-z0-9_-]{0,63})` ; un compte à majuscules était **sauté sans
   un mot** — sa mémoire serait restée au journal pour toujours. Elle est
   désormais rapportée (`memoire : self ignoré (hors motif de compte) : …`),
   sans changer le filtrage.

Et **la correction est déjà vivante** : `obrain-self-appliquer.service` exécute
le script de ce dépôt, donc le timer dit maintenant ce qu'il regarde, tout seul,
toutes les 2 minutes —

```
$ journalctl --user -u obrain-self-appliquer.service -n 6 --no-pager
Sep 16 12:17:02 … Starting obrain-self-appliquer.service …
Sep 16 12:17:02 … memoire-appliquer-comptes.sh[1092542]: memoire : journal
   /tmp/obrain-viz-cache/memoire (0 fichier(s)) — 0 compte(s) en attente, 0 appliquée
Sep 16 12:17:02 … Finished obrain-self-appliquer.service …
Sep 16 12:19:15 … memoire-appliquer-comptes.sh[1096945]: memoire : journal
   /tmp/obrain-viz-cache/memoire (0 fichier(s)) — 0 compte(s) en attente, 0 appliquée
```

(avant : seules les deux lignes systemd `Starting`/`Finished` apparaissaient —
un timer qui tourne et ne dit rien, pendant des heures.)

---

## 5. Quatre autres défaillances constatées — dont une CORRIGÉE depuis

Le constat de ce chantier était : « quatre défaillances hors de mes fichiers ». La
5.1 a été corrigée le 2026-09-16 (§ 9) ; les trois autres restent ouvertes, et
elles sont toujours décrites ici avec l'endroit exact où le code devrait changer.

### 5.1 Une ligne de journal non canonique est appliquée **et rejouée** — CORRIGÉ

La consommation compare chaque ligne brute du journal à sa **re-sérialisation**
JSON (`connues.contains(l)`, `memoire.rs`) : une ligne valide mais écrite dans un
autre ordre de clés (clés non triées) n'est jamais reconnue comme appliquée.
Mesuré sur un store temporaire, avec une intention `etat-self` (le geste **non
idempotent**, versionné exprès) :

```
PASSE 1 : appliquees=1  journal_encore_present=oui
PASSE 2 : appliquees=1  journal_encore_present=oui
SelfState créés : selfstate:self-essai6-28291:v1 , selfstate:self-essai6-28291:v2
```

Deux passes ⇒ deux versions d'état. Avec le timer de 2 minutes et une telle
ligne au journal, la base du compte grossirait d'un `SelfState` toutes les deux
minutes, indéfiniment.

> **CORRIGÉ le 2026-09-16** (§ 9). Le correctif est bien allé là où ce constat le
> désignait — `crates/obrain-cli/src/memoire.rs` — et la consommation y est
> désormais `journal_a_reecrire`, **fonction pure** qui compare la **valeur**
> (`serde_json::Value`, insensible à l'ordre des clés) et non le texte.
> Rejoué sur les **deux binaires réels**, décor vérifié (`en_attente: 1`) :
> `eea3b9aa` (avant) rend `appliquees=1` **puis `appliquees=1`**, journal encore
> présent, **deux** versions `SelfState` ; `9bfc0a5d` (après) rend
> `appliquees=1` puis **`appliquees=0`**, journal **consommé**, **une seule**
> version.

### 5.2 Le nom du « tenant » décide du droit de lire la mémoire du compte

Constaté en écrivant la porte : la ressource jugée par la politique IAM est le
**tenant**, dont le nom est tiré du **store**, alors que le compte reçoit des
droits sur son **espace**.

```
$ curl -s -b <cookie alice> http://127.0.0.1:8464/api/auth/whoami
{"authentifie":true,…}                                  ← identité VALIDE
$ curl -s -b <cookie alice> http://127.0.0.1:8464/api/engine/memoire
{"acces":"lecture","error":"permission denied: action=graph:read
  resource=orn:obrain:graph:obrain:tenant/runtime-essai", …}   ← refusé
```
(store `…/runtime-essai.obrain`, espace du compte `esp-…`)

Conséquence directe pour l'objectif « chaque utilisateur a un agent sur SA
base » : **une base `selfs/<compte>.obrain` n'est pas forcément lisible par son
compte** si l'espace IAM ne porte pas le même nom que la base. La porte de ce
chantier contourne le piège en nommant son espace comme son store — détail
commenté dans le script pour que personne ne reperde une heure dessus. **Le
correctif n'est pas dans mes fichiers** : il est dans la construction de la
ressource jugée (garde IAM de `crates/obrain-cli/src/viz_serve.rs` et
`crates/obrain-iam/`).

### 5.3 La mémoire n'est relue que si la base est à côté du store ancré — CORRIGÉ DU CÔTÉ ÉCRIVAIN, MESURÉ

`engine.rs`, `profil_du_self` : la base lue est
`dirname(ancrage.store)/selfs/<compte>.obrain`, en lecture seule, et le profil
est injecté en tête du contexte. Donc une conversation **ancrée sur un monde
rangé ailleurs** (par exemple `~/.local/share/obrain-harness/mondes/…`) ne
relit **pas** la mémoire du compte : `profil_du_self` rend `None` sans le dire.
*Affirmé par lecture de code (`engine.rs:857-935`).*

> **2026-09-16 (soir) — la moitié qui pouvait l'être est corrigée, et le défaut
> n'est plus seulement affirmé : il est REPRODUIT.** Le lecteur est hors de mon
> périmètre (il faudrait y décider d'un chemin canonique plutôt que d'un chemin
> dérivé : §7.4). L'ÉCRIVAIN, lui, peut savoir où la conversation relira — la
> provenance portée par chaque entrée de journal est `<monde> · <store>`, donc
> `dirname(store)/selfs/<compte>.obrain` se calcule. La passe compare désormais
> ce chemin à celui où elle écrit :

```
memoire : self:essai53 — DIVERGENCE : la conversation s'est ancrée sur un store hors de
/tmp/…/defaut53/data, donc elle RELIRA /tmp/…/defaut53/mondes/selfs/essai53.obrain, et
non /tmp/…/defaut53/data/selfs/essai53.obrain où cette passe écrit. La mémoire serait
appliquée là où aucune conversation ne la relit (engine.rs, profil_du_self :
dirname(store ancré)/selfs/<compte>.obrain).
memoire : 1 compte(s) dont la mémoire est écrite là où la conversation ne la relit PAS
```

> Elle **applique quand même** (jeter une intention serait pire) et **sort en
> erreur** : un écart qui se répète toutes les deux minutes doit se voir. Le
> défaut 5.3 est ainsi devenu *observable en production* au lieu d'être une
> ligne de rapport que personne ne lit — et la porte l'exige (étape 11).

### 5.4 Un avertissement qui ressemble à une perte de mémoire — QUALIFIÉ, MESURÉ, PAS CORRIGÉ

`WARN obrain_substrate::replay: post-replay anchor audit: nodes with labels but
no reachable properties (husk signature …) label_only=1` apparaît à l'ouverture
**de toute base**, y compris une base fraîche de **0 nœud** :

```
$ ./target/release/obrain init /tmp/…/neuve.obrain && \
  ./target/release/obrain query --read-only /tmp/…/neuve.obrain "MATCH (n) RETURN count(*) AS total"
WARN … husk signature … label_only=1 …
{ "total": "0" }
```

Ce n'est donc pas la mémoire du self qui est perdue — mais quiconque lit le
journal du timer ou du chat y voit, toutes les deux minutes, l'annonce d'une
perte de propriétés. À qualifier côté substrat.

> **2026-09-16 (soir) — le message est hors périmètre (substrat), la LECTURE
> qu'on en fait est dans le mien, et elle est corrigée.** Deux gestes :
> (1) la passe **relaie** `stderr` sans jamais le filtrer (filtrer cacherait un
> vrai refus, qui est justement ce qu'on veut voir) et dit **une fois**, à côté,
> ce qu'un WARN n'est pas : `… ligne(s) de moteur relayée(s) : un WARN du
> substrat (husk signature, émis même sur une base de 0 nœud) n'est PAS un
> refus ; le verdict est le code de sortie` ; (2) la porte essaie d'exercer le
> fond (§6, étape 12) : sur une base **neuve**, le husk WARN accompagne
> `total = 0`, ce qui établit qu'il n'annonce aucune perte. Constat honnête du
> dernier passage : le binaire testé **n'émet plus** ce WARN (quelqu'un l'a
> corrigé côté substrat entre-temps), donc l'assertion est marquée `∅ NON
> TESTÉE` au lieu d'être comptée comme un succès — une porte qui dégraderait en
> silence serait exactement le défaut qu'elle garde.

---

## 6. La porte : `scripts/acceptation-self-par-compte.sh`

```bash
bash scripts/acceptation-self-par-compte.sh [port] [--sans-llm]
```

Tout vit sous `/tmp/acceptation-self-compte-<pid>/` : stores, cache de mémoire,
mondes, magasin IAM. La porte **refuse** de tourner si ces chemins ne sont pas
sous `/tmp`, prend un port libre > 8400 (jamais 8432/8433/8455), et n'approche
aucune base vivante. Elle sort 0 si tout passe, non nul sinon, et affiche une
ligne `✅ / ❌ / ∅` par assertion.

Ce qu'elle prouve, dans l'ordre :

1. le serveur annonce `self:<compte>` (et non `self:agent`) comme Self de la
   conversation, et désigne **le** journal — dont le nom est un hachage, donc
   non devinable ;
2. le journal de `bob` est un **autre** fichier que celui d'`alice` ;
3. la **conversation réelle** (modèle interrogé, appel d'outil) journalise une
   intention pour ce compte ;
4. la passe — **le vrai script**, isolé par variables d'environnement — applique,
   et elle a **découvert le compte par son journal** (pas par l'annuaire) ;
5. le nœud est dans la base **de ce compte**, avec `self:<compte> -[:HAS_NOTE]->`,
   et le texte témoin est celui de la conversation ;
6. **la boucle se referme** : la conversation suivante relit la base du compte
   (journal du serveur : `[self] profil injecte : N ligne(s) pour self:<compte>`) ;
7. le journal est **consommé** (l'intention n'est pas rejouée à chaque passe) ;
8. la base de l'**autre** compte est **intacte** : empreinte `sha256` du
   répertoire **identique**, témoin toujours lisible, aucune fuite de la mémoire
   d'`alice` chez `bob`, et aucune base créée pour lui ;
9. la passe **n'est pas muette** quand il n'y a rien à faire, et elle **nomme le
   journal qu'elle a regardé** ;
10. une panne ne prend pas la forme du repos : binaire absent ⇒ code 3 et la
    raison écrite.
11. ── **AJOUTÉ le 2026-09-16** ── **le TÉMOIN** : la mémoire a été journalisée
    (une ligne `journalise` pour ce compte, donc l'appel de l'outil est COMPTÉ)
    et appliquée (une ligne `applique`), et ces traces survivent à la
    consommation du journal ;
12. ── **AJOUTÉ** ── **repos ≠ chaîne cassée** : avec un témoin, la passe date
    le dernier appel à l'outil ; sans témoin, elle refuse explicitement
    d'appeler « repos » une chaîne dont on n'a aucune trace de production ;
13. ── **AJOUTÉ** ── **la base écrite est celle qui sera relue (5.3)** : une
    conversation ancrée sur un store rangé ailleurs est DITE, la passe applique
    quand même et sort en erreur ;
14. ── **AJOUTÉ** ── **un avertissement n'est pas un refus (5.4)** : `stderr` est
    relayé et compté, la doctrine est dite une fois, et le `husk WARN` — émis
    même sur une base neuve — est rapporté à une base de 0 nœud (ou marqué
    `∅ NON TESTÉE` s'il n'est plus émis) ;
15. ── **AJOUTÉ** ── **la relecture n'arrête pas la croissance** : une SECONDE
    mémoire, arrivée après qu'une conversation a relu la base du compte, doit
    encore pouvoir être appliquée. **Cette assertion ÉCHOUE aujourd'hui** : le
    service garde la base ouverte (§0.6). Elle est là pour rester rouge tant que
    le défaut existe — une porte qui ne le dirait pas serait complice du
    silence.

Avec `--sans-llm`, les assertions qui dépendent du modèle (3 et 6) sont marquées
`∅ NON TESTÉE` — l'entrée de journal est alors écrite au format **exact** de
`memoire::Entree::vers_json` (clés triées : voir §5.1) — et le verdict final
**dit** ce qui n'a pas été exercé. Une porte qui dégraderait en silence serait
exactement le défaut qu'elle est censée garder.

### Sortie réelle (2026-09-16 18:51, `target/debug/obrain` — profil dev, pas le release)

Les blocs 0 à 6bis sont ceux de la sortie du 12:18 ci-dessus et passent de la
même façon ; voici les blocs AJOUTÉS, verbatim, plus le verdict :

```
$ OBRAIN_BIN=./target/debug/obrain bash scripts/acceptation-self-par-compte.sh 8477
…
5bis) le témoin : la mémoire laisse une trace datée après consommation
  ✅ le témoin existe : memoire-temoins.jsonl
  ✅ l'application est TÉMOIGNÉE : 1 ligne(s) « applique » pour self:alice-1594001
  ✅ l'appel de l'outil est COMPTÉ : 1 journalisation(s) pour self:alice-1594001
…
7) le journal est consommé (l'intention appliquée n'est pas rejouée)
  ✅ le journal a été supprimé : tout ce qui était en attente a été appliqué
…
9bis) « aucun compte n'a rien à appliquer » ≠ « la chaîne est cassée »
  ✅ chaîne VIVANTE : la passe date le dernier appel à l'outil (2 journalisation(s))
  ✅ chaîne SANS trace de production : la passe le dit au lieu de le taire
  ✅ elle refuse explicitement d'appeler « repos » une chaîne dont on n'a aucune trace

11) la mémoire est écrite là où la conversation la relit (défaut 5.3)
  ✅ la passe DIT que la conversation relira …/defaut53/mondes/selfs/essai53.obrain
     et non la base écrite
  ✅ elle sort en 1 : un écart qui se répète toutes les 2 minutes se voit
  ✅ l'intention a été appliquée QUAND MÊME (on ne jette pas une mémoire)

12) un avertissement du moteur n'est pas un refus (défaut 5.4)
  ✅ la passe relaie ce que dit le moteur, et le compte
  ✅ elle dit une fois qu'un WARN n'est pas un verdict (le verdict est le code de sortie)
  ∅  le husk WARN n'est pas émis par ce binaire (défaut 5.4 peut avoir été corrigé
     côté substrat) : l'assertion n'a pas pu être exercée

13) après une relecture par la conversation, la passe peut-elle ENCORE appliquer ?
     memoire : self:alice-1594001 — la base est TENUE EN LECTURE (…/selfs/alice-1594001.obrain) :
       rien n'est perdu, l'intention reste au journal et cette passe réessaiera toutes les
       2 minutes ; mais elle ne peut pas aboutir tant que le détenteur (le service qui a relu
       le profil de ce compte) tourne.
  ❌ REFUS PAR VERROU : la base du compte est tenue en LECTURE par un autre processus
     (le service, mesuré : 10 descripteurs ouverts sur la base) ; la mémoire reste au
     journal et la base NE GRANDIT PAS tant qu'il tourne
  ❌ la base du compte ne porte que 1 note(s) : la seconde mémoire n'est jamais arrivée

verdict
  ❌ 2 contrôle(s) en échec
     dont 1 sur UN SEUL défaut, et il est réel : la base du compte reste ouverte
     en LECTURE par le service après que la conversation l'a relue (verrou exclusif
     refusé) — la base du compte ne peut plus grandir tant que le service vit.
     Défaut hors des fichiers de ce chantier : engine.rs / viz_serve.rs / obrain-substrate.
```

**Bilan du passage : 39 ✅, 2 ❌, 1 ∅.** Les deux ❌ sont le MÊME défaut, réel et
mesuré (§0.6) : ce n'est pas un défaut de la porte, c'est un défaut de la chaîne
que la porte a cessé de cacher. Le ∅ est un défaut corrigé ailleurs entre-temps
(le `husk WARN` n'est plus émis) : il est marqué NON TESTÉ et non « réussi ».

> **À LIRE AVANT DE REJOUER LA PORTE** : ce passage a été fait avec
> `OBRAIN_BIN=./target/debug/obrain` (profil dev), parce que le binaire DÉPLOYÉ
> ne contient pas encore le témoin (il a été reconstruit en release à 17:45,
> avant cette passe). Lancée sur le déployé, la porte rend **35 ✅ / 4 ❌ / 1 ∅**
> au lieu de 39 ✅ / 2 ❌ / 1 ∅ — mesuré, et les deux ❌ de plus sont exactement
> ceux qui exigent le binaire neuf (5bis « le témoin existe » et 9bis « la passe
> date le dernier appel »). C'est voulu : la porte est en avance sur le
> déploiement, jamais en retard.

### Sortie réelle (2026-09-16 12:18, binaire déployé, modèle `qwen3.8-27b` sur 9910) — bloc historique, conservé tel quel

```
$ bash scripts/acceptation-self-par-compte.sh 8467
0) prérequis et isolation (aucune base vivante n'est approchée)
  ✅ binaire : /home/triviere/.local/share/obrain-harness/bin/obrain
  ✅ temporaires : /tmp/acceptation-self-compte-1095334 (tous sous /tmp)
1) serveur armé sur 8467, cache de mémoire ISOLÉ            ✅ 401 sans identité
2) deux comptes, chacun sa base                             ✅ alice / bob
3) la conversation appartient au COMPTE
  ✅ le serveur annonce self:alice-1095334
  ✅ journal désigné : 4035d1a84bd7a77d.jsonl
  ✅ le journal de bob-1095334 est un AUTRE fichier (ed69cb0bd314e1ad.jsonl)
4) la conversation journalise une intention pour ce compte
  ·  modèle interrogé : http://192.168.1.7:9910 (HTTP 200)
  ✅ le modèle a appelé l'outil, la conversation a journalisé
5) la passe d'application (le vrai script, isolé)
     memoire : self:alice-1095334 : 1 appliquee(s)
     memoire : journal /tmp/…/cache/memoire — 1 entrée(s) appliquée(s) sur 1 compte(s)
  ✅ la passe a traité alice-1095334 (il l'a trouvé par le JOURNAL)
6) le nœud est dans la base DU COMPTE, rattaché à son Self
  ✅ self:alice-1095334 -[:HAS_NOTE]-> :Note présent
  ✅ le texte témoin de la conversation est DANS la base du compte
6bis) la mémoire du compte est RELUE dans la conversation suivante
  ✅ le serveur a relu la base : profil injecte : 1 ligne(s) pour self:alice-1095334
7) le journal est consommé (l'intention n'est pas rejouée)     ✅
8) la base de l'AUTRE compte (bob-1095334) est intacte
  ✅ empreinte identique (507c0ea5fd8f2dc6…) : un octet n'a pas bougé
  ✅ le témoin de bob-1095334 est toujours lisible
  ✅ aucune trace de la mémoire de alice-1095334 dans la base de bob-1095334
9) la passe DIT ce qu'elle a regardé quand il n'y a rien à faire   ✅ (3 contrôles)
10) une panne ne prend pas la forme du repos  ✅ binaire absent → code 3, raison écrite

verdict
  ✅ la chaîne complète tient : conversation → journal → passe → base DU COMPTE
```

---

## 7. Ce qui reste impossible sans un changement en Rust

Aucun de ces changements n'est dans mes fichiers de propriété ; ils sont écrits
ici pour être faits par ceux qui les possèdent.

1. **La journalisation dépend entièrement du modèle.** Aucun code ne
   journalise l'échange lui-même : `memoire::enregistrer` n'a que **deux**
   appelants, tous deux des appels d'écriture DÉCIDÉS — le chemin d'outil
   (`outils.rs:1037`) et la route d'un harness distant (`viz_serve.rs:8639`).
   Une conversation qui ne « demande » pas de retenir quelque chose ne laisse
   **rien** dans la base du compte. **Mesuré le 2026-09-16 (§0.1) : 4 562
   événements de conversation en une journée, 0 appel à l'outil ; dernière
   production réelle 2026-09-16 00:02:03.** Si l'objectif est « chaque
   conversation développe la base », il faut un appel à la fin de l'échange dans
   `crates/obrain-cli/src/engine.rs`, boucle de `demander_stream`, au moment où
   l'événement `Evenement::Fin` est émis (l'`ancrage.self_qn` y est connu) :
   c'est une décision produit, pas une correction de bogue. **C'est le seul
   verrou qui reste entre « la chaîne marche » et « le self se développe ».**
2. ~~**La consommation du journal repose sur une comparaison textuelle**~~ —
   **FAIT** (§5.1, corrigé le 2026-09-16) : `journal_a_reecrire` compare la
   **valeur** et non le texte. Un témoin ne dira jamais qu'une entrée a été
   appliquée si elle ne l'a pas été ; la correction se prouve sur les deux
   binaires réels.
3. **Le droit de lire une base de compte dépend du nom du tenant** (§5.2) :
   garde IAM appelée par `crates/obrain-cli/src/viz_serve.rs` (construction de la
   ressource `…tenant/<nom>`) et `crates/obrain-iam/`. Tant que c'est le nom du
   store qui est jugé, un compte dont l'espace ne s'appelle pas comme la base
   sera refusé en `graph:read` — y compris sur sa propre mémoire.
4. **Le chemin de relecture est figé** sur `dirname(store ancré)/selfs/<compte>.obrain`
   (`engine.rs`, `profil_du_self`). Une conversation ancrée hors du répertoire
   `data/` ne relit pas la mémoire du compte, et le silence est total. Ce n'est
   désormais plus **silencieux** côté écrivain (§5.3 : la passe le dit et sort en
   erreur), mais la mémoire concernée reste non relue tant que le lecteur ne
   prend pas la racine canonique plutôt que le voisinage du store ancré.
5. **LA RELECTURE EMPÊCHE LA CROISSANCE (trouvé et mesuré le 2026-09-16,
   §0.6)** — le défaut le plus grave pour l'objectif du chantier, et il n'était
   visible nulle part : le service qui relit la base d'un compte
   (`engine.rs::profil_du_self`, appelé par `contexte_du_monde`) **garde cette
   base ouverte** (10 descripteurs relevés dans `/proc/<pid>/fd`). Un lecteur
   tient le `flock` **partagé**, l'écriture exige l'**exclusif**
   (`obrain-substrate/src/file.rs`) : la passe d'application ne peut donc plus
   JAMAIS écrire dans la base de ce compte tant que le service vit, et la base
   cesse de grandir. Ce que ça implique, sans détour : **« l'agent se développe
   sur SA base » est vrai une fois — la première — puis faux**, pour tout compte
   dont la conversation a relu sa mémoire. Les gestes possibles, par ordre de
   préférence, appartiennent aux propriétaires de `engine.rs` / `viz_serve.rs` /
   `obrain-substrate` :
   * **ne pas garder la base du self ouverte** entre deux requêtes (ouvrir,
     lire, fermer) — c'est le geste minimal, et il laisse l'invariant
     mono-écrivain intact ;
   * ou faire de l'application une **écriture coopérative** dans le processus du
     service (une passe qui passe par lui plutôt que par un second processus) ;
   * en attendant, `scripts/memoire.sh` (arrêt, application, relance) reste le
     seul remède — c'est un arrêt de production, donc une décision, pas une
     routine. **Mesure associée** : la porte, étape 13, échoue tant que le
     défaut est là (le journal garde l'intention : rien n'est perdu).

---

## 8. Ce qui reste à faire (au-delà du périmètre de ce chantier)

- **Le seul geste qui manque pour que le self se développe : un appel à la fin
  de l'échange** (§7.1). Tant qu'il n'existe pas, la seule mesure honnête
  possible est celle du témoin : « N journalisations, dernier appel il y a X
  heures » — et la porte l'exige désormais (§6, 5bis et 9bis).
- Rejouer la porte **après chaque déploiement** : c'est elle qui transforme
  « l'agent se développe » d'une intention en une mesure. Elle est étendue
  (4 blocs d'exigences en plus) et **échoue** si le témoin, la distinction
  repos/panne, la garde d'ancrage ou la doctrine du WARN disparaissent.
- ~~Un témoin de croissance par compte~~ — **livré sous la forme minimale qui
  sert** : le témoin daté de la mémoire (§0.3). Ce qui reste : le lire côté
  interface (un tableau « la base de `<compte>` a-t-elle grossi depuis 7 jours ? »
  avec la date du dernier nœud, les notes portant déjà `created_ms` et les états
  `created_at`). La passe, elle, sait déjà dire **cassé ≠ repos**.
- Traiter §5.2 (tenant) avec son propriétaire.
- **Une incohérence trouvée en éprouvant la passe, et pas encore corrigée** : la
  découverte des comptes lit les **fichiers** du répertoire de journal, alors que
  le compte des ententes en attente lit le journal **au nom du self** (un
  hachage). Un journal rangé sous un autre nom est donc découvert… puis ignoré :
  la passe annonçait « 1 compte en attente » et appliquait 0 sans dire pourquoi.
  Le compte rendu est devenu honnête (« N repéré(s) au journal, M en attente
  vérifiée »), mais l'écart de fond reste : les deux lectures devraient porter
  sur la même chose. Reproduit sur `/tmp` : un fichier `journal.jsonl` portant
  une entrée pour `self:bob-essai` → `1 compte(s) repéré(s)`, `0 appliquée`.
- Rien de tout ceci ne dépend du « micro-harness par machine » : la chaîne
  prouvée ici est celle du **chat d'un compte** sur l'interface 8432. Ce que la
  porte ne couvre pas : un `self` par machine, l'unité `selfd`, la publication de
  ces bases vers un harness distant — hors de mon chantier.

---

## 9. Journal des corrections de cette passe (2026-09-16, soir)

| Fichier | Ce qui change | Comment c'est prouvé |
|---|---|---|
| `crates/obrain-cli/src/memoire.rs` | **Le témoin** : `chemin_temoins()`, `Temoin`, `temoigner()`, branché sur `enregistrer` (après le journal, jamais avant) et sur `appliquer` (seulement si la passe a avancé ou refusé). Deux tests : la ligne se relit en JSON, et le témoin ne vit PAS dans le répertoire parcouru par la découverte. | `cargo test -p obrain-cli --bin obrain memoire::` → **11 passed; 0 failed** (dont `le_temoin_se_lit_et_ne_vit_pas_dans_le_repertoire_du_journal`) ; et le binaire réel écrit la ligne : cf. §0.3 |
| `scripts/memoire-appliquer-comptes.sh` | Lit le témoin et **date la chaîne** ; détecte la **divergence d'ancrage** (5.3) ; détecte **témoin ↔ disque** incohérent (« CHAÎNE CASSÉE ») ; relaie `stderr` en disant qu'un WARN n'est pas un refus (5.4). | Trois cas rejoués sur `/tmp` : cache absent, cache vide, journal ancré ailleurs (cf. §5.3) |
| `scripts/acceptation-self-par-compte.sh` | **5 blocs d'exigences AJOUTÉS** (5bis témoin, 9bis repos ≠ cassé, 11 base écrite = base relue, 12 WARN ≠ refus, 13 la relecture n'arrête pas la croissance) ; l'assertion 7 est **rectifiée** (elle condamnait un rejeu inexistant dès que la conversation de 6bis journalisait une intention NEUVE — elle exige désormais ce qu'elle doit exiger : l'entrée APPLIQUÉE n'est plus au journal). Aucune assertion retirée ni assouplie. | 39 ✅ / 2 ❌ / 1 ∅, sortie en §6 ; les 2 ❌ sont le défaut §0.6 |
| `docs/DEVELOPPEMENT-SELF-PAR-COMPTE-2026-09-16.md` | §0 (mise à jour), §5.3 et §5.4 requalifiés, §7.2 marqué FAIT, §8 mis à jour, ce journal. | le présent document |

**Deux choses que cette passe NE fait pas, et qui sont dites pour ne pas être
confondues avec un oubli** : elle n'ajoute aucun déclencheur dans l'échange
(`engine.rs` n'est pas dans son périmètre — c'est le §7.1, la décision qui
manque), et elle ne corrige pas le message du substrat (§5.4 : hors périmètre,
qualifié et mesuré à la place).
