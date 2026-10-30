# ADR — Chaîne IAM complète, et contrôle de l'explosion des liens

**Date** : 2026-09-13. **Nature** : architecture decision record, tissé dans le
graphe par le canal `docs/` → `import_docs` → nœud `Plan` (voir « Le canal » plus
bas). Tout ce qui suit est **mesuré** sur `runtime-dev` (≈ 408 000 nœuds,
1 255 000 arêtes) et sur des bases libres : les chiffres sont des relevés, pas des
ordres de grandeur.

## Contexte

Deux défauts systémiques, découverts en cherchant pourquoi « lire 4 nœuds »
coûtait 900 ms, puis pourquoi l'IAM existait sans être appliqué.

**1. La lecture du graphe était dominée par UNE phase non indexée.** Quand
l'identifiant de la **cible** est contraint (`MATCH (a)-[r]->(b) WHERE id(b)
IN […]`), le filtre ne peut pas descendre sous l'expansion : tout le motif
retombait sur un **scan complet de la table des arêtes**, 216 à 923 ms,
**constant quel que soit le nœud** — ce qui interdisait de l'attribuer à une
requête ou à une donnée.

**2. L'IAM était une bibliothèque sans appelant.** 5 395 lignes, 95 tests verts,
**zéro point d'application** : `session_ext.rs` décrivait son intégration en
pseudo-code, la feature `iam` n'était ni dans `default` ni dans `ai`. Le serveur
exposé sur le réseau servait tout le monde sans identité, et **rien ne le disait**.

## Décisions

### D1 — Retourner le motif plutôt que balayer

La **cible devient la graine**, la traversée se fait **en entrant**
(`edges_from(b, Incoming)` → `incoming_heads` + chaîne `next_to`) : **O(degré)**
au lieu de **O(E)**. Préconditions conservatrices (saut unique ou longueur
variable plafonnée, sens sortant, mode `Walk`, aucun alias de chemin, entrée =
`NodeScan` nu) : hors de ces formes, **le plan d'origine reste** — jamais un
résultat modifié.

*Mesures* : phase d'expansion **905 027 µs → 39-282 µs** ; moteur d'une question
**878-891 ms → 0 ms** ; requête entrante seule **216-923 ms → 6,8-47 ms** ;
`/api/viz/subgraph?inbound=1` **851,7 ms → 0,5 ms**. Résultats **identiques**
(11 cas de référence : ensembles de lignes égaux ; seul l'ordre des ex æquo sous
`ORDER BY` bouge).

### D2 — Pousser le filtre dans les DEUX côtés d'une jointure

Deux écritures de la même requête différaient de **154×** : en chaîne, le filtre
tombe entre les expansions (88 ms) ; à plat, il restait **au-dessus** de la
jointure, donc les deux côtés scannaient tout avant d'apparier (**31 724 ms**).
Le prédicat est poussé dans chaque côté qui porte ses variables ; chaque côté
devient un motif à cible contrainte et D1 s'y applique. Refus explicites :
jointures non-`Inner`, produits croisés, **toute forme de prédicat non
répertoriée** (oublier une variable ferait évaluer le prédicat là où elle n'est
pas liée).

*Mesures* : **31 724 → 194-206 ms**, mêmes 19 968 lignes ; 3 sauts : 608 ms pour
1 070 976 lignes.

### D3 — Le produit cartésien est borné, pas subi

`MATCH (a)-[r]->(b), (c) WHERE id(b) IN [3]` **faisait mourir le serveur**, sans
message : le plan n'était pas une jointure mais un **scan imbriqué**
(1 255 485 arêtes × 407 677 nœuds ≈ 5·10¹¹ paires). L'engin a un budget mémoire
(`memory_limit`, `spill_path`) et **aucun opérateur d'exécution ne le consulte**.

Garde au **passage unique de la planification**, à partir des seuls compteurs du
store (O(1), bornes prouvées) : HTTP 400 qui **dit** les bornes, le plafond et
trois remèdes. Précision vérifiée : une jointure par **égalité corrélée** (que le
moteur sait faire par sonde d'index) **n'est pas refusée** ; une jointure modeste
passe.

### D4 — Le pouvoir d'arrêt est armé aux TROIS points d'entrée

L'échéance de requête existait, était testée, et le code le dit lui-même :
« *Complète, testée… et jamais armée en production* ». `MATCH (a)-[*1..2]->(b)
WHERE id(b) IN [315447]` tournait **encore après 150 s**. Armée depuis : `viz
serve` (30 s), `obrain query` (60 s, code de sortie 1), **pont MCP** (30 s — le
chemin du harness, donc d'un appel d'outil d'agent qui ne rendait jamais la
main). Le chemin d'**écriture** du pont n'est **pas** armé : une indexation
longue est légitime.

### D5 — La garde IAM : une règle, tous les points d'entrée

`obrain_iam::Garde` : `authentifier` puis `check_graph_access(action, "tenant",
<base>)`. Portée **par base**. La bibliothèque est **fail-closed**
(`ImplicitDeny` : pas de politique ⇒ refus) — l'inverse du middleware du
framework `this`, où une route sans politique déclarée **passe** (`Abstain`).

Chaîne fermée : **connexion** (mot de passe bcrypt → jeton + cookie `HttpOnly`),
**identification**, **renouvellement AVEC rotation** (l'ancien jeton meurt),
**déconnexion qui révoque pour de bon**, **piste d'audit** alimentée.

### D6 — Un défaut non armé doit être BRUYANT

La garde non armée **passe** — mais l'état est annoncé au démarrage, publié par
`/api/info`, par `whoami` et par `initialize` du pont : « IAM NON ARMÉ — aucune
identité n'est exigée : quiconque atteint ce point d'entrée lit et écrit tout le
graphe ». C'est exactement ce qui manquait : une porte nue que personne ne voyait.

### D7 — La capacité est CONSTATÉE, pas affirmée

`has_backward_adjacency()` renvoyait `true` **en dur**, et tout D1 repose dessus :
sur une base à chaîne entrante incomplète, le retournement rendrait **moins** de
lignes que le plan générique, **en silence**. Le substrat vérifie donc, par
échantillon déterministe (64 destinations, une fois par instance), que la chaîne
rend ce que le balayage compte ; le planificateur ne retourne que sur une capacité
**constatée**. Mesuré : `[flip] RETOURNÉ` / 0,4-0,6 ms contre `[flip] adjacence
entrante non vérifiée` / 373-418 ms (plan générique) — **mêmes lignes**.

## Les cinq défauts IAM corrigés (chacun mesuré)

| défaut | mesure | correction |
|---|---|---|
| **Jeton déterministe** — `DefaultHasher` + compteur | deux processus distincts émettaient `obt_d32bf2e05a925e5f…0001` | 256 bits tirés du système |
| **Révocation décorative** — magasin ouvert en **lecture seule** | `revoked: true` mais **4 credentials avant / 4 après**, session encore valide | ouverture en **écriture partagée** ; repli lecture seule **déclaré** |
| **Expiration jamais vérifiée** | le message disait « invalid or expired », le code ne testait rien | `iam_expires_ts` écrit **et** contrôlé |
| **Horodatage fictif** — `now_iso()` = compteur formaté en date | `2026-01-01T00:00:00Z`, `…:01Z` | horloge réelle (`SystemTime`) |
| **Empreinte faible** — SipHash-64 sans sel | une collision suffit à faire passer un jeton pour un autre | **SHA-256** |

## Conséquences (non négociables)

1. **Construire avec `--features ai,iam`.** `scripts/vizserve.sh` lance
   `./target/release/obrain` tel quel ; un `cargo build` ordinaire — ou
   `--features ai` — produit un binaire qui annonce `IAM NON COMPILÉ` et où
   `OBRAIN_IAM=1` **n'a aucun effet**. Constaté **trois fois** dans la journée.
2. **Un magasin d'identités DÉDIÉ et ÉCRIVABLE** (`OBRAIN_IAM_STORE=…/iam.obrain`).
   Pointé sur une base servie — donc ouverte en lecture seule — connexion et
   déconnexion redeviennent décoratives ; la garde le dit.
3. **Vérifier le CONTENU, pas l'horodatage.** Trois fichiers emportés par une
   écriture en masse d'une autre session (manifeste, `viz_serve.rs`,
   `mcp_server.rs`) : l'arbre ne compilait plus, et l'horodatage ne le disait pas.
4. **Une chaîne qu'on ne peut pas revérifier n'est pas stabilisée** :
   `scripts/verif-chaine-iam.sh` (13 étapes, port 8435, verdict et code de sortie).

## Le canal — comment cette connaissance est entrée dans le graphe

**Un seul processus écrit** (`OBRAIN_WRITE=1`) et il détient
`runtime-prod.obrain` ; tout autre pont est **lecteur** — c'est l'invariant qui
protège le WAL. Conséquence directe, mesurée : `manage_adr` et `import_docs`
répondent *« écriture impossible (store en lecture seule ?) »* depuis une session
qui n'est pas l'écrivain. Le tissage passe donc par les **documents** :

```
docs/<un-projet>/*.md  →  import_docs --plans_dir <ce répertoire>  →  nœud Plan
```

Preuve que le canal fonctionne : le graphe contient déjà
`plan:/home/triviere/projects/banc-obrain/docs/FULL-OBRAIN-VIZ-ROADMAP-2026-09-08`.
Ce document-ci suit exactement la même forme, et porte la règle : **pour tisser,
écrire le document ; l'import appartient au writer.**

## Règles transférables

* **Un coût constant n'est pas un coût de requête** : 849-850 ms pour quatre nœuds
  différents désignaient une phase, pas une donnée.
* **Ne jamais confondre la place RÉSERVÉE et le plafond RÉEL** — c'est ainsi que
  `?max_tokens=24` coûtait 50 128 ms au lieu de 2 400 ms.
* **Ce qui est annoncé sans être vérifié finit par mentir** : `has_backward_adjacency`,
  l'expiration d'un jeton, l'horodatage, un budget mémoire que nul n'interroge.
* **Mesurer avant de conclure sur un plan** : `EXPLAIN` a montré que le produit
  cartésien n'était pas une `Join` mais un scan imbriqué — mon premier garde
  visait la mauvaise forme.
* **Une sonde témoin peut être repliée par l'optimiseur** : `id(b) + 0 = N` était
  simplifiée puis retournée ; le témoin doit être une forme que le moteur ne
  reconnaît pas (`NOT (id(b) <> N)`).
* **Un état non protégé doit être annoncé**, pas découvrable : c'est la règle qui
  a transformé « le serveur est ouvert » en « le serveur le dit ».
