# L'acceptation de la flotte — la question posée pour de vrai

`docs/MACHINES-CONNUES-2026-09-15.md` §5 réclame un test et le nomme :

> Deux harnesses sur deux stores […] et **une seule question au serveur global** :
> « quelles machines connais-tu, et que sait faire chacune ? ». Aujourd'hui cette
> question n'a aucune réponse : elle échoue par le vide, pas par l'erreur.

Ce document est le compte rendu de ce test, écrit et **exécuté** le 2026-09-16.
La porte est `scripts/acceptation-flotte.sh` : un moteur armé en IAM sur un port
libre (8600), **deux comptes**, **trois nœuds `selfd`** aux capacités différentes,
tout sous `/tmp`, rien de déployé, aucun service de l'utilisateur touché.

**Verdict : 19 vérifications passent, 9 échouent sur 28.** L'échec n'est pas un
accident de la porte : c'est l'état de la ligne déployée. La question de la flotte
n'a toujours pas de réponse — mais elle a maintenant un garde-fou qui la
réclamera, et qui échoue bruyamment, avec le motif exact, tant qu'elle n'en aura
pas.

---

## 1. Ce qui a été mesuré, et avec quel binaire

La porte mesure **le binaire**, pas la source : une correction non construite
n'est pas une correction déployée. Trois exécutions complètes ont été faites (et
une contre-épreuve, plus bas), sur les deux constructions présentes sur la
machine :

| exécution | moteur | sha256 | verdict |
|---|---|---|---|
| A | `~/.local/share/obrain-harness/bin/obrain` (celui que `OBRAIN_BIN` désigne dans l'environnement) | `eea3b9aa3058b36dae6918d2562624f9416d820bbcbb5b9848abc00741e8c389` | 19 ✅ / 9 ❌ |
| B | `./target/release/obrain` (celui du dépôt, la ligne du contrat) | `eeef5062c68d7b021bde245e898c95c6c66478650ed1a949d80360a070e7f467` | 19 ✅ / 9 ❌ |
| **C** (celle citée en §2) | idem B — la porte imprime en plus le **motif** du refus mesuré | idem B | 19 ✅ / 9 ❌ |

Nœud, dans tous les cas : `./target/release/selfd`,
`fbc4bf58fa00e7faf4bc29d497385b45d605bc813ed900e47a801dc62ae308b1` (celui des faits
établis).

A et B rendent des verdicts **identiques au caractère près** une fois
normalisés les noms de tag, les identifiants de client et les horodatages :

```text
$ sed -e 's/<tag A>/TAG/g' … sortie-flotte.txt      | grep -E '✅|❌' > a.txt
$ sed -e 's/<tag B>/TAG/g' … sortie-flotte-tronc.txt | grep -E '✅|❌' > b.txt
$ diff a.txt b.txt && echo IDENTIQUES
IDENTIQUES (hors noms de tag, identifiants et horodatages)
```

L'exécution C — celle dont la sortie est reproduite en §2 — ajoute deux choses :
le vocabulaire d'absence accepté par l'assertion 7.3 s'est élargi (voir §5), et
la porte **imprime désormais le motif** du refus qu'elle mesure (§2, assertion
2.4), pour qu'on ne soit pas obligé de croire l'auteur du test sur parole. Elle
rend le même verdict que A et B : **19 ✅ / 9 ❌ sur 28**.

**Révision de référence.** Les numéros de ligne cités dans ce rapport sont ceux de
la révision commitée : `69421840` (base de ce chantier) et `1f8b2209` (le commit
qui l'a suivi, 2026-09-16 12:05:59, qui ne touche que
`crates/obrain-substrate/src/wal_io.rs` — `git diff --stat 69421840 1f8b2209 --
crates/obrain-cli/src/remote.rs crates/obrain-cli/src/viz_serve.rs` est vide). Ce
qui a bougé **non commité** sous ces lignes est traité en §5.

Le reste de ce document cite l'exécution B.

---

## 2. La sortie brute, complète

Commande : `env -u OBRAIN_BIN ./scripts/acceptation-flotte.sh` (port 8600, pris
parce qu'il est libre — le script refuse 8432/8433/8455/8090/8144/3082/3083 et
tout port ≤ 8400). Durée : quelques secondes. Sortie intégrale :

```text

1) le dispositif : deux comptes, trois machines, un port libre (le 8600)
  ·  moteur mesuré : eeef5062c68d7b021bde245e898c95c6c66478650ed1a949d80360a070e7f467
  ·                   ./target/release/obrain
  ·  nœud mesuré   : fbc4bf58fa00e7faf4bc29d497385b45d605bc813ed900e47a801dc62ae308b1
  ·                   ./target/release/selfd
  ✅ trois identités distinctes (admin, alice-1096944-19765, bob-1096944-19765), deux comptes porteurs de machines
  ✅ le moteur écoute sur 127.0.0.1:8600 (pid 1097073, stores et cache sous /tmp)

2) chaque machine DIT ce qu'elle sait faire — et c'est VRAI
  ✅ « n3-porte-1096944-19765 » est vue par son compte (jalon : le canal sortant fonctionne)
  ✅ « n2-porte-1096944-19765 » est vue par son compte
  ✅ « n1-porte-1096944-19765 » (racine en lecture seule) est vue par son compte
  ·  annonces : exec=[,{"attache_ms":1789561156191,"capacites":["read","list","write","status","exec"],"derniere_vue_ms":1789561156191,"id":"cl-1a0aa28325f-1","machine":"n3-porte-1096944-19765","silence_ms":40]
  ·              ro  =[{"aide":"Sur votre machine : `selfd attacher --moteur <url> --jeton <votre jeton>`","clients":[{"attache_ms":1789561156192,"capacites":["read","list","write","status"],"derniere_vue_ms":1789561156192,"id":"cl-1a0aa283260-3","machine":"n1-porte-1096944-19765","silence_ms":39]
  ·              obs =[{"aide":"Sur votre machine : `selfd attacher --moteur <url> --jeton <votre jeton>`","clients":[{"attache_ms":1789561156192,"capacites":["read","list","write","status"],"derniere_vue_ms":1789561156192,"id":"cl-1a0aa283260-2","machine":"n2-porte-1096944-19765","silence_ms":45]
  ✅ « exec » annoncé par n3-porte-1096944-19765, absent de n2-porte-1096944-19765 : les capacités NE SONT PAS les mêmes
  ✅ l'appel « exec » sur n3-porte-1096944-19765 a RÉELLEMENT tourné (stdout rendu par la machine)
  ✅ l'appel « exec » sur n2-porte-1096944-19765 est REFUSÉ, et le refus est motivé : « exécution NON AUTORISÉE sur ce nœud : il a été démarré sans --allow-exec. Un nœud qui n'a pas été autorisé à agir observe ; il n'agit pas." »
  ❌ « write » est annoncé par n1-porte-1096944-19765 alors que sa racine (/tmp/acceptation-flotte-1096944-19765/racine-ro) est en LECTURE SEULE : la capacité annoncée n'est pas exercable (annoncer n'est pas décider)

3) « vos machines » — la seule route qui existe : GET /api/remote/clients
  ✅ avec l'identité de alice-1096944-19765, la route NOMME n3-porte-1096944-19765 et ses capacités
  ✅ la route d'alice-1096944-19765 ne nomme AUCUNE machine d'un autre compte
  ✅ sans identité → 401 : le canal ne parle qu'à VOS machines (jamais de liste anonyme)

4) LA QUESTION : « quelles machines connais-tu, et que sait faire chacune ? »
  ·  GET /api/remote/machines → 404  {"error":"route inconnue du canal"}
  ·  GET /api/machines → 404  not found
  ·  GET /api/flotte → 404  not found
  ·  GET /api/fleet → 404  not found
  ·  GET /api/harness/machines → 404  not found
  ·  GET /api/engine/machines → 404  not found
  ❌ AUCUNE route ne répond à la question de la flotte (voir les codes ci-dessus) : la réponse est le vide, pas l'erreur
  ❌ impossible de nommer n3-porte-1096944-19765 : aucune route ne répond (l'assertion échoue par absence de réponse, pas par contenu)
  ❌ impossible de nommer n2-porte-1096944-19765 : aucune route ne répond
  ❌ impossible de dire ce que sait faire chacune : aucune route ne répond

5) le vide n'a pas la forme d'une bonne réponse (§2 du document)
  ✅ sans identité, la question est REFUSÉE (401) — pas servie vide
  ❌ route absente : rien ne peut dire « je ne connais aucune machine » — l'absence de route et l'absence de machine sont indistinguables
  ✅ le refus est MOTIVÉ (« {"acces":"lecture","error":"aucune identité présentée (en-tête `Authorization: Bearer  »)

6) cloisonnement : chaque compte ne voit que ses machines
  ✅ bob-1096944-19765 ne voit jamais n3-porte-1096944-19765
  ✅ alice-1096944-19765 ne voit jamais n2-porte-1096944-19765
  ✅ le compte admin, qui n'a attaché aucune machine, n'en voit aucune : le registre est PAR COMPTE
  ✅ bob-1096944-19765, qui CONNAÎT l'identifiant cl-1a0aa28325f-1, ne peut pas l'appeler (409 : {"error":"ce client n'est pas à vous"})

7) une machine CONNUE mais ABSENTE reste nommée (sinon « connaître » veut dire « voir »)
  ·  « n2-porte-1096944-19765 » a été arrêtée ; le moteur redémarre (le registre est-il un fait, ou une vue ?)
  ✅ le moteur est redémarré sur le même port, avec les mêmes stores
  ✅ n3-porte-1096944-19765 s'est RÉATTACHÉE seule après le redémarrage (le canal sortant tient sa promesse)
  ❌ bob-1096944-19765 ne retrouve plus n2-porte-1096944-19765 après le redémarrage du moteur : la flotte était une vue en mémoire, pas un fait ({"aide":"Sur votre machine : `selfd attacher --moteur <url> --jeton <votre jeton>`","clients":[],"compte":"bob-1096944-19765"})
  ❌ (corollaire) rien ne distingue « jamais vue » de « connue mais absente »

8) la machine comme NŒUD du graphe (§3 et §4.1 du document)
  ·  moteur arrêté : le verrou exclusif du store est rendu, on interroge la base elle-même
  ❌ aucun nœud « machine: » dans le store servi (réponse : []) — la flotte n'est pas un fait du graphe

verdict
  ❌ 9 vérification(s) en échec sur 28
  (chaque ❌ nomme ce qui manque ; docs/ACCEPTATION-FLOTTE-2026-09-16.md les ordonne)
code=1
```

### Contre-épreuve : la porte ne dit pas toujours non

Une porte qui échoue toujours ne prouve rien. On l'a donc fait tourner en
désignant une route qui **répond** (`ROUTE_FLOTTE=/api/remote/clients`) : la
première assertion de §4 passe au vert — la porte distingue « aucune route » de
« une route qui répond » — et l'assertion 5.2 attrape alors le mode de défaillance nommé en §2
du document, *un ok sur un vide intégral* :

```text
$ env -u OBRAIN_BIN ROUTE_FLOTTE=/api/remote/clients ./scripts/acceptation-flotte.sh
4) LA QUESTION : « quelles machines connais-tu, et que sait faire chacune ? »
  ·  GET /api/remote/clients → 200  {"aide":"…","clients":[],"compte":"admin"}
  ✅ une route répond à la question : /api/remote/clients
  ❌ la réponse ne nomme pas n3-porte-1082106-20708 : {"aide":"…","clients":[],"compte":"admin"}
  ❌ la réponse ne nomme pas n2-porte-1082106-20708 : {"aide":"…","clients":[],"compte":"admin"}
  ❌ la réponse ne distingue pas les capacités des deux machines (exec=[] obs=[])
  ❌ la réponse nomme les machines sans dire leurs capacités
5) le vide n'a pas la forme d'une bonne réponse (§2 du document)
  ✅ sans identité, la question est REFUSÉE (401) — pas servie vide
  ❌ la flotte répond 200 avec une liste VIDE alors que deux machines sont attachées
…
  ❌ 9 vérification(s) en échec sur 29
```

---

## 3. Assertion par assertion

| # | Ce que la porte exige | Ce qu'elle a observé | Verdict |
|---|---|---|---|
| 1.1 | Deux comptes distincts et leurs jetons existent | `admin`, `alice-…`, `bob-…` amorcés hors bande (l'administration IAM **précède** le démarrage : voir §6) | ✅ |
| 1.2 | Un moteur armé écoute sur un port **libre**, stores et cache sous `/tmp` | écoute sur `127.0.0.1:8600`, aucun port de service approché | ✅ |
| 2.1 | Trois machines aux noms différents sont vues par leur compte | `n3-porte-…`, `n2-porte-…`, `n1-porte-…` vues | ✅ |
| 2.2 | Les capacités annoncées **diffèrent** d'une machine à l'autre | `n3` = `[read,list,write,status,exec]`, `n2` = `[read,list,write,status]` | ✅ |
| 2.3 | `exec` annoncé par `n3` est **réellement** exercable | appel `POST /api/remote/appel` → la commande tourne, `stdout` = `flotte-<tag>` | ✅ |
| 2.4 | L'absence d'`exec` sur `n2` est **réellement** un refus, et motivé | `{"ok":false,"refus":"exécution NON AUTORISÉE sur ce nœud : il a été démarré sans --allow-exec. Un nœud qui n'a pas été autorisé à agir observe ; il n'agit pas."}` — motif imprimé par la porte, pas cité de la source | ✅ |
| 2.5 | Une capacité **non exercable** n'est pas annoncée | `n1`, dont la racine est en lecture seule (`chmod 555`), annonce quand même `write` | ❌ |
| 3.1 | La route par compte nomme la machine et ses capacités | `/api/remote/clients` avec le jeton d'`alice` rend `n3-porte-…` et `["read","list","write","status","exec"]` | ✅ |
| 3.2 | Elle ne nomme **aucune** machine d'un autre compte | le corps d'`alice` ne contient jamais `n2-porte-…` | ✅ |
| 3.3 | Sans identité, la route refuse | `401` + motif `aucune identité présentée` | ✅ |
| 4.1 | **Une route répond à la question de la flotte** | 6 candidates interrogées : `/api/remote/machines` → 404 « route inconnue du canal », `/api/machines`, `/api/flotte`, `/api/fleet`, `/api/harness/machines`, `/api/engine/machines` → 404 `not found` | ❌ |
| 4.2 | La réponse **NOMME** `n3` | aucune réponse à examiner | ❌ |
| 4.3 | La réponse **NOMME** `n2` | aucune réponse à examiner | ❌ |
| 4.4 | Elle dit ce que sait faire **chacune**, dans le même enregistrement, distinct | aucune réponse à examiner | ❌ |
| 5.1 | Sans identité, la question est refusée — pas servie vide | `401` : la garde IAM couvre tout `/api/remote/*` **avant** le routage. La propriété est acquise, mais elle ne prouve pas qu'une route existe (voir §4, manque 6) | ✅ |
| 5.2 | Avec deux machines attachées, une liste vide serait un mensonge | route absente : rien ne peut dire « je ne connais aucune machine » ; l'absence de route et l'absence de machine sont indistinguables | ❌ |
| 5.3 | Un refus porte un **motif** | le corps du 401 porte `error` et le texte de la garde | ✅ |
| 6.1 | `bob` ne voit jamais la machine d'`alice` | vrai, y compris en connaissant l'identifiant | ✅ |
| 6.2 | `alice` ne voit jamais la machine de `bob` | vrai | ✅ |
| 6.3 | Un compte sans machine n'en voit aucune (le registre est **par compte**) | le corps d'`admin` est `clients: []` — et c'est ce qui interdit de répondre à la question globale par `/api/remote/clients` | ✅ |
| 6.4 | Connaître un `client_id` n'ouvre aucun droit | appel croisé de `bob` sur le client d'`alice` → `409 {"error":"ce client n'est pas à vous"}` | ✅ |
| 7.1 | Après redémarrage du moteur, la machine vivante se réattache seule | `n3` revient sans intervention (le canal sortant tient sa promesse) | ✅ |
| 7.2 | Une machine **connue mais absente** reste nommée | le corps de `bob` après redémarrage est `clients: []` | ❌ |
| 7.3 | L'absence est **dite** (elle n'est pas confondue avec une connexion) | sans objet : la machine n'est même plus nommée | ❌ |
| 8.1 | La machine est un **nœud du graphe** (`machine:<nom>`) | `obrain query <store> --read-only "MATCH (n) WHERE n.qualified_name STARTS WITH 'machine:' RETURN n.qualified_name"` → `[]` | ❌ |

---

## 4. Les manques, du plus bloquant au moins bloquant

### Manque 1. (bloquant) Aucune route ne répond à la question de la flotte

C'est le manque nommé en §5 de `MACHINES-CONNUES-2026-09-15.md`, confirmé tel quel : la question n'a
**aucune** réponse, et l'échec est un 404, pas un vide. Six orthographes
plausibles ont été essayées : toutes muettes.

- **Où le code devrait changer** : `crates/obrain-cli/src/viz_serve.rs:2896`
  (le dispatch `p if p.starts_with("/api/remote/")`) et la table de routes de
  `api_remote`, `crates/obrain-cli/src/viz_serve.rs:8337-8435`. Le repli de ce
  module est `_ => erreur(stream, 404, "route inconnue du canal")` (8435) : c'est
  là qu'ajouter `("/api/remote/machines", "GET")`.
- **La forme exigée par la porte** : pour chaque machine, **dans le même
  enregistrement**, son nom et ses capacités. Un document qui sépare la liste des
  noms et la liste des capacités ne répond pas à « que sait faire CHACUNE » : il
  rend au lecteur le soin de recoudre, et c'est précisément ce qui casse
  (« le modèle propose des gestes qu'il ne peut pas prendre », §4 point 2 du document).
- **Décision de conception que la porte encode, et qu'il faut assumer** : elle
  pose la question avec l'identité **administrateur** (un opérateur de
  l'instance), parce que la flotte traverse deux comptes ; et elle exige que le
  cloisonnement par compte reste intact (les assertions 6.1 à 6.3 passent aujourd'hui). Une route par
  compte ne peut pas répondre à la question — c'est mesuré : le compte `admin`,
  qui n'a attaché aucune machine, voit `clients: []`.
- **Doctrine** : « annoncer est décider ». La future route ne doit nommer que des
  machines dont la **joignabilité est vérifiée** (le canal les a vues dans la
  fenêtre de silence), et doit dire les autres comme absentes plutôt que de les
  taire.

### Manque 2. (bloquant) Le registre de la flotte n'est pas un fait : il vit dans la mémoire du processus

Mesuré : `bob` retrouve `clients: []` après redémarrage, alors que sa machine
avait été vue avant.

- **Où le code devrait changer** : `crates/obrain-cli/src/remote.rs:88-107`
  (`struct Etat`, `struct Registre`, `fn registre()` — un `OnceLock` statique),
  et `attacher()` (`remote.rs:136-157`) qui se contente d'insérer dans la
  `HashMap`. Rien n'est écrit nulle part, rien n'est relu au démarrage.
- **Effet secondaire lu dans le code** (lecture, non mesuré au runtime) :
  `SILENCE_MAX_MS = 90_000` (`remote.rs:31-35`) et `oublier_les_muets()`
  (`remote.rs:121-133`) font disparaître une machine muette depuis plus de 90 s —
  donc un portable qui dort sort de « quelles machines connais-tu » **sans même
  un redémarrage**.
- **Contrainte à respecter** : l'invariant mono-écrivain. `viz serve` sert ses
  stores en **lecture** et n'y écrit jamais ; le motif autorisé est journal
  append-only puis passe d'application (`crates/obrain-cli/src/memoire.rs`). Une
  flotte qui se souvient doit donc se ranger **là où le moteur a le droit
  d'écrire**, ou être écrite par un autre (voir manque 3) et seulement lue ici.
- **État au moment d'écrire** : ce manque est en cours de comblement dans l'arbre
  — un journal append-only par compte, ajouté à `remote.rs` le 2026-09-16 à
  **12:14:42**, soit *après* la mesure qui fonde ce rapport (voir §5). Le binaire
  jugé ici ne le porte pas.

### Manque 3. (bloquant) Le graphe ne porte aucun nœud `machine:`

Mesuré : la requête GQL sur le store servi rend `[]`. §3 du document est donc
toujours vrai : le graphe connaît les sessions, les appels d'outils et l'état du
Self, **pas la flotte**.

- **Qui doit écrire** : §4 point 1 du document tranche déjà — « écrite par le **harness
  lui-même** au démarrage ». Côté nœud, le point d'écriture est l'annonce :
  `crates/obrain-selfd/src/attache.rs:61` (la fonction `attacher`, qui compose
  déjà `machine` + `capacites`). Côté moteur, la réception est
  `crates/obrain-cli/src/viz_serve.rs:8338-8358` — qui ne touche aujourd'hui
  aucun graphe (« `/api/remote/*` ne touche à aucun graphe », commentaire de
  `viz_serve.rs:2642`).
- **Ce que le nœud devrait porter** (§4 point 1 du document) : `machine:<hostname>`,
  l'OS, le store servi, le mode offert, la **dernière présence**.
- **Contrainte** : même invariant mono-écrivain que le manque 2. Un `viz serve`
  en lecture ne peut pas écrire ce nœud dans le store qu'il sert ; il lui faut
  soit un store dédié ouvert en écriture, soit le journal puis la passe
  d'application.

### Manque 4. (important) L'annonce perd la moitié qui dit ce que la machine **ne** fait **pas**

Le nœud dit aujourd'hui ce qu'il accepte ; le motif de ses refus est calculé par
sa politique (mesuré : le refus d'`exec` revient **motivé** — c'est une force du
dispositif), mais il n'arrive pas jusqu'au pilotage.

- **Où le code devrait changer** : `crates/obrain-cli/src/viz_serve.rs:8343-8348`
  ne lit que `capacites` dans le corps d'attachement ; `struct Client`
  (`crates/obrain-cli/src/remote.rs:60-72`) n'a de champ ni pour les capacités
  refusées, ni pour les limites. Or la source de `selfd` dans le dépôt envoie
  désormais `capacites_refusees` et `limites` (`crates/obrain-selfd/src/attache.rs`,
  fonction `attacher`, modifiée le 2026-09-16 à 12:02 par un autre chantier) :
  **le moteur les ignore silencieusement**.
- **Pourquoi ça compte** : §4 point 2 du document — « sinon le modèle propose des
  gestes qu'il ne peut pas prendre et l'échec arrive après coup ».

### Manque 5. (important) Les capacités annoncées par le binaire `selfd` **déployé** ne sont pas mesurées

Mesuré, et c'est le seul échec de la section 2 : `n1`, dont la racine est
`chmod 555`, annonce `["read","list","write","status"]`. Le `write` y est
inexerçable — la sonde d'écriture de la politique échouerait, et une écriture
demandée échouera après coup.

- **Ce que la porte a mesuré** : le binaire `./target/release/selfd`,
  `sha256 fbc4bf58…`, mtime `2026-09-15 22:37` — les faits établis. Dans ce
  binaire, l'annonce est une liste **en dur** (`read, list, write, status`, plus
  `exec` selon la politique).
- **Ce que le dépôt contient au moment d'écrire ce rapport** :
  `crates/obrain-selfd/src/attache.rs` (mtime `2026-09-16 12:02:26`, **modifié
  non commité**, par un autre chantier) passe désormais par
  `etat.politique.capacites()`, qui **mesure** chaque capacité
  (`crates/obrain-selfd/src/politique.rs:159-165`).
- **Le manque restant est donc un manque de LIGNE, pas de code** : reconstruire
  `selfd` et le redéployer (règle R7 : le dev d'abord, la prod ne redémarre
  jamais), puis relancer cette porte — c'est elle qui dira si le correctif est
  bien arrivé jusqu'au fil. Tant que la porte mesure le binaire déployé, elle
  doit échouer : c'est exactement son rôle.

### Manque 6. (moyen) Une route `/api` inconnue peut répondre « permission refusée » au lieu de « n'existe pas »

Constaté en écrivant la porte, avec un espace IAM qui ne portait pas le nom du
store servi :

```text
GET /api/machines → 403 {"acces":"lecture","error":"permission denied: action=graph:read resource=orn:obrain:graph:obrain:tenant/store"}
```

La garde IAM s'exécute **avant** le routage
(`crates/obrain-cli/src/viz_serve.rs:2618-2650`) : une route qui n'existe pas se
déguise en refus de droit. Pour un opérateur, « vous n'avez pas le droit » et
« cette route n'existe pas » ne se corrigent pas de la même façon — c'est la même
famille de défaut que le silence pris pour une panne. Deux conséquences pour
cette porte : elle nomme l'espace IAM d'après le store servi (sinon toutes les
routes inconnues répondent 403 et masquent les 404), et elle **dit** ce que
chaque candidate a répondu plutôt que de compter les échecs.

### Manque 7. (moyen) Le repli du module `remote` masque l'absence de route sous un autre nom

`/api/remote/machines` répond `404 {"error":"route inconnue du canal"}`
(`crates/obrain-cli/src/viz_serve.rs:8435`), là où les autres chemins inconnus
répondent `404 not found`. Le motif est honnête, mais il faudra se rappeler que
la question de la flotte se range **dans ce module** : une route ajoutée ailleurs
sera invisible à la porte tant que `ROUTE_FLOTTE` ne la nommera pas.

### Manque 8. (mineur) Une liste vide ne se dénonce pas

Même sur la route qui existe, une flotte vide est un tableau vide :
`{"aide":"…","clients":[],"compte":"admin"}`. Rien n'y distingue « ce compte n'a
attaché aucune machine » de « le registre a été perdu au redémarrage » — et c'est
exactement ce que les assertions 7.2 et 7.3 mesurent : les deux corps sont identiques avant et après. Tant
que le manque 2 n'est pas comblé, cette indistinction est structurelle ; une fois
la flotte persistée, elle devra être **dite** (`absente`, `dernière présence`)
plutôt que laissée à l'interprétation. Le champ `vivant` en cours d'ajout dans
l'arbre (`remote.rs`, voir §5) est exactement cette réponse ; c'est pourquoi
l'assertion 7.3 l'accepte comme marqueur d'absence.

---

## 5. L'arbre de travail a bougé pendant que j'écrivais — et ce que cela change

Huit chantiers travaillent en parallèle dans le même arbre. Ce que j'ai observé,
horodaté, pour que la lecture de ce rapport ne soit pas trompée :

- **La tête de branche a avancé** : `1f8b2209` (2026-09-16 **12:05:59**, « LE WAL
  S'OUVRAIT SANS O_APPEND… »), là où le contrat de ce chantier fixe la base à
  `69421840`. Ce commit ne touche que `crates/obrain-substrate/src/wal_io.rs` :
  `git diff --stat 69421840 1f8b2209 -- crates/obrain-cli/src/remote.rs
  crates/obrain-cli/src/viz_serve.rs` **est vide**. Les lignes citées en §4
  restent donc exactes pour la révision commitée.
- **`crates/obrain-cli/src/remote.rs`** (mtime **12:14:42**, +1001 lignes non
  commitées) : le registre est désormais **journalisé** — un fichier append-only
  par compte sous `OBRAIN_VIZ_CACHE_DIR/machines`, relu au démarrage — et
  `Client` porte un champ `vivant` (« connue du journal, mais pas joignable »,
  `remote.rs:132-136` dans la version en cours d'écriture). C'est **le manque 2
  en cours de comblement**, et il l'est dans le sens de la doctrine : l'écriture
  va dans le cache du moteur, jamais dans un store servi en lecture.
- **`crates/obrain-selfd/src/attache.rs`** (mtime **12:02:26**, non commité) :
  l'annonce passe par `Politique::capacites()`, donc par des capacités
  **mesurées** — le manque 5 en cours de comblement. Mais **rien n'est construit** :
  le binaire mesuré est du 2026-09-15 22:37, et c'est lui que la porte juge.
- **Fichiers neufs non suivis** apparus pendant la session, du fait d'autres
  chantiers : `packages/obrain-dashboard/src/graph/flotte.ts`,
  `…/PanneauFlotte.tsx`, `crates/obrain-cli/tests/remote_registre.rs`,
  `remote_memoire.rs`, `harness_machine.rs`, `scripts/selfd-installer.sh`,
  `scripts/selfd-empaqueter.sh`.
- **Ce qui n'a PAS bougé, et c'est décisif** : les routes du canal sont encore
  `attacher`, `ordres`, `resultat`, `detacher`, `clients`, `memoire`, `appel` —
  vérifié dans l'arbre au moment d'écrire

  ```text
  $ grep -n '("/api/remote/' crates/obrain-cli/src/viz_serve.rs
  8340: ("/api/remote/attacher", "POST")   8362: ("/api/remote/ordres", "GET")
  8384: ("/api/remote/resultat", "POST")   8393: ("/api/remote/detacher", "POST")
  8398: ("/api/remote/clients", "GET")     8417: ("/api/remote/memoire", "POST")
  8424: ("/api/remote/appel", "POST")
  ```

  **Aucune route de flotte.** Le manque 1 n'est donc pas un retard de
  construction : il est dans le code. De même, la lecture du corps d'attachement
  ne retient toujours que `capacites` (manque 4).
- **Conséquence de lecture** : tout ce qui est mesuré ici l'est **contre les
  binaires** (§1). Les manques 2 et 5 sont en cours de traitement dans l'arbre,
  non construits ; les manques 1, 3, 4, 6, 7 et 8 y sont encore. La porte ne
  passera au vert que sur un binaire reconstruit **et redéployé** — et c'est bien
  ce qu'on veut d'un garde-fou : il juge la ligne, pas l'intention.
- **Note de méthode** : l'assertion 7.3 (« l'absence est DITE ») accepte
  désormais le vocabulaire de l'arbre qui arrive (`"vivant": false`) au même
  titre que `absent`, `hors ligne` ou `injoignable`. La porte n'impose pas un
  mot — elle exige que l'absence se dise. C'est la seule assertion que j'ai
  élargie, et je le dis : élargir un **vocabulaire** n'est pas relâcher une
  **exigence**, mais la nuance mérite d'être écrite noir sur blanc.

---

## 6. Ce que la porte a appris en étant écrite

- **L'administration IAM hors bande doit précéder le démarrage du moteur.**
  `obrain iam jeton <iam> <nom>` pendant que `viz serve` tourne échoue sur le
  verrou exclusif : `substrate /tmp/… is already open by another process
  (exclusive lock on …/substrate.meta refused)`. Un test qui créerait ses comptes
  après le démarrage mesurerait cet accident d'ordonnancement au lieu de la
  flotte. Les trois identités sont donc posées **avant**.
- **L'espace IAM doit porter le nom du store servi.** La politique indexe la
  ressource sur `("tenant", <base>)` : un espace nommé autrement fait répondre
  403 à toute lecture de graphe, et masque les 404 (manque 6). Constaté, puis
  corrigé dans la porte.
- **`pkill -f` tue le shell qui l'exécute** quand le motif cherché figure dans sa
  propre ligne de commande. La porte tue **par PID**, jamais par motif — et le
  dit, parce que l'incident a coûté une exécution.
- **Le nœud se réattache seul** après un redémarrage du moteur, sans
  intervention : c'est mesuré (§7.1) et c'est la propriété sur laquelle un
  pilotage pourra s'appuyer.

---

## 7. Les services vivants n'ont pas été touchés

Mesures encadrant l'exécution C — mêmes valeurs pour A, B et la contre-épreuve :

```text
$ ss -ltnp | grep ':8432'                     # avant
LISTEN 0 128 192.168.1.103:8432 0.0.0.0:* users:(("obrain",pid=1053321,fd=3))
$ systemctl --user is-active obrain-viz       # avant
active
$ env -u OBRAIN_BIN ./scripts/acceptation-flotte.sh     # la porte
$ ss -ltnp | grep ':8432'                     # après
LISTEN 0 128 192.168.1.103:8432 0.0.0.0:* users:(("obrain",pid=1053321,fd=3))
$ systemctl --user is-active obrain-viz       # après
active
$ systemctl --user show obrain-viz -p MainPID -p ActiveEnterTimestamp
MainPID=1053321
ActiveEnterTimestamp=Wed 2026-09-16 12:04:54 UTC
```

Même PID avant et après, service `active`, aucune unité rechargée, aucun port de
service approché (la porte refuse 8432/8433/8455/8090/8144/3082/3083 et tout port
≤ 8400 ; elle a pris 8600). Après les exécutions, aucun résidu : ni port `86xx` en
écoute, ni processus `selfd`, ni répertoire `acceptation-flotte-*` sous `/tmp`.

**Une réserve d'honnêteté sur cette preuve.** Ma toute première mesure, à
`12:01:08`, montrait un autre PID (`1033684`) ; à `12:04:54` le service a été
redémarré par un **autre acteur** (son `ActiveEnterTimestamp` le date), avant que
je ne lance la porte. Je n'ai jamais invoqué `systemctl`, ni tué un PID qui ne
soit pas le mien (mon moteur de prototypage sur le port 8601 et son nœud `selfd`).
La preuve valable est donc celle-ci : **pendant l'exécution de la porte, le PID et
l'état sont inchangés**, et ils le restent. Ce redémarrage extérieur concerne
l'ordonnanceur du chantier, pas cette porte.

---

## 8. Ce qui reste à faire

1. **Compléter la porte** (elle est le garde-fou, pas le remède) : le jour où une
   route de flotte existera, la nommer dans `routes_flotte()` /
   `ROUTE_FLOTTE`, et vérifier qu'elle **n'annonce pas** une machine dont le
   canal ne l'a pas vue vivante (joignabilité vérifiée) — l'assertion n'est pas
   encore écrite, faute de route à mesurer.
2. **Combler les manques 1 à 3** (`viz_serve.rs:2896` + `api_remote` pour la
   route ; `remote.rs:88-107` pour la persistance ; l'écriture du nœud
   `machine:` par le harness pour le graphe) : ce sont eux qui font échouer **8
   des 9** vérifications (5 pour la route absente, 2 pour la mémoire perdue, 1
   pour le nœud de graphe). Au moment d'écrire, le manque 2 est **en cours** dans
   l'arbre (journal par compte) et le manque 1 **ne l'est pas**.
3. **Reconstruire et redéployer `selfd`** (manque 5) : le correctif des capacités
   mesurées est déjà dans l'arbre de travail, pas dans le binaire déployé. Sans
   construction ni redéploiement, la porte continuera d'avoir raison contre la
   ligne — et c'est exactement son office (R7 : le dev d'abord, la prod ne
   redémarre jamais).
4. **Porter les refus et les limites jusqu'au moteur** (manque 4) : c'est la
   moitié de la réponse à « que sait faire chacune » qui manque encore.
5. **Relancer la porte à chaque étape** — et accepter qu'elle reste rouge tant que
   la question n'a pas de réponse. Une porte qu'on assouplit pour obtenir du vert
   ne garde plus rien.

---

## 9. Note du 2026-09-16 (après la porte) — la route existe, et une CONTRADICTION est apparue

Mesures refaites après les chantiers, avec `OBRAIN_BIN` forcé sur la construction
du dépôt. Ce forçage n'est pas cosmétique : par défaut la variable désigne une
**copie ancienne**, si bien que la porte mesurait un autre moteur que celui du
dépôt — fidèle à son contrat (elle mesure la ligne déployée), mais il faut le
savoir pour lire son verdict.

| moteur mesuré | nœud mesuré | verdict |
|---|---|---|
| `eea3b9aa` (la copie que `OBRAIN_BIN` désigne) | `d10f6c2d` | 20 ✅ / 8 ❌ |
| `b8d1d30f` (`./target/release/obrain`, le dépôt) | `d10f6c2d` | **22 ✅ / 6 ❌** |

Deux échecs ont **disparu** avec le moteur du dépôt, et ce sont exactement ceux
que le manque 2 annonçait : « bob ne retrouve plus n2 après le redémarrage du
moteur » et « rien ne distingue jamais vue de connue mais absente ». La porte
**confirme donc indépendamment** la persistance du registre des machines — que le
chantier correspondant ne pouvait prouver que par ses propres tests.

Un troisième a disparu parce que le **nœud** mesuré est le `selfd` reconstruit :
« write annoncé sur une racine en lecture seule » est passé au vert côté n1.

Les 6 échecs restants se réduisent à deux manques : **la question de la flotte**
(5 assertions) et **le graphe ne porte aucun nœud `machine:`** (1 assertion).

### 9.1 La route existe désormais — mais elle répond par COMPTE, et la porte attend une vue globale

`GET /api/remote/machines` est implémentée (voir `viz_serve.rs`, `api_remote`) :
elle nomme les machines du compte appelant, dit les capacités telles qu'elles ont
été **annoncées**, sépare `connues` de `joignables`, et surtout **dit quand elle
ne sait rien** (`"aucune": true` plus une phrase en toutes lettres) — donc « aucune
machine » ne se confond plus avec « route absente ». La porte passe de 28 à
**29 assertions**, et celle qui exigeait une route répondant à la question est
désormais verte.

Mais la porte interroge avec le jeton **admin** (ligne 398 :
`http GET "$r" "$JETON_ADMIN"`) et attend que la réponse **nomme les machines
d'alice et de bob**. La route répond pour le compte appelant — donc admin, qui n'a
rien attaché, reçoit honnêtement « ce compte ne connaît AUCUNE machine ».

**Les deux sections de cette porte se contredisent**, et ce n'est pas un détail
d'implémentation :

* § 4 exige qu'**admin voie toutes les machines** (vue d'ensemble de la flotte) ;
* § 6 **valide** que « le compte admin, qui n'a attaché aucune machine, n'en voit
  aucune : le registre est PAR COMPTE ».

Deux lois du dépôt s'opposent ici, et le choix est une **décision de sécurité**,
pas une inférence :

* **A — par compte** (ce qui est implémenté) : chacun ne voit que ses machines.
  Lecture stricte de « une liste ne doit jamais contenir une ressource dont on n'a
  pas vérifié le droit » (`docs/MACHINES-CONNUES-2026-09-15.md` §4). Aucune fuite
  possible ; en revanche la question n'a de réponse que compte par compte, et un
  opérateur ne peut pas voir la flotte entière.
* **B — globale pour qui administre** : la question de
  `docs/MACHINES-CONNUES-2026-09-15.md` §5 (« une seule question au serveur
  global ») reçoit une réponse d'ensemble, chaque machine portant son compte. Il
  faut alors décider explicitement **quel droit** ouvre cette vue, et le vérifier —
  aucun helper de privilège évident n'existe aujourd'hui dans `viz_serve.rs`,
  c'est donc ce qu'il faudrait écrire et éprouver.

**La porte n'a PAS été assouplie** pour obtenir du vert : ses ❌ sont restées
bruyantes, et la contradiction a été consignée ici plutôt que contournée.

### 9.2 La décision : **A — strictement par compte** (2026-09-16)

Le donneur d'ordre a délégué le choix. Il est tranché ainsi, et les raisons sont
tirées du dépôt, pas d'une préférence :

1. **La loi apprise du bug** est « une liste ne doit jamais contenir une ressource
   dont on n'a pas vérifié le droit » (`MACHINES-CONNUES-2026-09-15.md` §4). C'est
   une loi **par identité** — elle ne souffre pas d'exception « pour l'admin ».
2. **L'énoncé d'origine** est « un self qui arrive à tous les manager » : *mon*
   self gérant *mes* machines. Il ne dit nulle part qu'un compte voit celles d'un
   autre.
3. **L'IAM de ce dépôt scope déjà la mémoire par compte**, et le cloisonnement est
   une qualité que cette porte **vérifie**. Une vue inter-comptes serait une
   régression de confidentialité déguisée en confort d'exploitation, et elle
   supposerait d'inventer un droit d'administration qui n'existe pas encore
   (`viz_serve.rs` n'a aucun helper de privilège).

Le § 4 de la porte a donc été **réécrit, et durci** — jamais affaibli :

* il interroge le **compte propriétaire** de chaque machine (alice pour n3 et n1,
  bob pour n2), et exige que chacune soit nommée **avec ses capacités** ;
* il vérifie que les capacités sont **distinctes** (n3 annonce `exec`, n1 non) ;
* il ajoute une assertion que la version précédente **ne pouvait pas porter** : la
  **non-fuite par cette route-là** — alice ne voit aucune machine de bob, et
  réciproquement. Le cloisonnement n'était mesuré que sur
  `/api/remote/clients` ; il l'est désormais sur `GET /api/remote/machines` aussi ;
* un compte **sans** machine doit recevoir `"aucune": true` **explicitement** — un
  fait énoncé, pas un silence ;
* le § 5 regarde la réponse du **propriétaire** (une liste vide y serait un
  mensonge) et non celle d'admin, pour qui une liste vide est la vérité.

### 9.3 Verdict après la décision et la route

| étape | moteur | verdict |
|---|---|---|
| porte d'origine | `eea3b9aa` + `selfd` d'alors | 19 ✅ / 9 ❌ (28) |
| après les chantiers | `b8d1d30f` | 22 ✅ / 6 ❌ (28) |
| après la route + § 4 durci | `target/debug/obrain` (route incluse) | **32 ✅ / 1 ❌ (33)** |

Mesuré en clair, la question reçoit enfin une réponse **par compte** :

```text
GET /api/remote/machines (admin) → 200  {"aucune":true,"compte":"admin","connues":0,…}
alice → {"aucune":false,"connues":2,"joignables":2,"machines":[n3 (exec…), n1 (ro…)]}
bob   → {"aucune":false,"connues":1,"joignables":1,"machines":[n2 (sans exec)]}
✅ alice, qui la possède, se voit nommer n3 — et n1
✅ bob, qui la possède, se voit nommer n2
✅ les capacités sont DISTINCTES : n3 annonce exec, n1 non
✅ alice ne voit AUCUNE machine de bob par cette route
✅ bob ne voit AUCUNE machine d'alice par cette route
✅ admin, qui n'a rien attaché, se voit répondre « aucune machine » — EXPLICITEMENT
```

**Il ne reste alors qu'UN échec** : `aucun nœud « machine: » dans le store servi`
— le dernier manque bloquant de ce document (§3, manque 3). Ce n'est pas un défaut
de route : la flotte n'était pas un **fait du graphe**. Le registre la connaissait,
la route la disait, mais une IA qui interroge le graphe ne trouvait rien. C'est
l'objet du § 9.4.

### 9.4 La flotte devient un FAIT DU GRAPHE — la porte passe au VERT

`obrain machines` (module `crates/obrain-cli/src/machines.rs`, commande
`commands/machines.rs`) ferme le manque 3, en respectant la contrainte qui
l'imposait : le moteur sert ses bases en **LECTURE**, donc il ne peut pas écrire
ces nœuds lui-même — c'est l'invariant mono-écrivain, pas un défaut. Faire de la
flotte un fait du graphe est donc un geste **séparé**, comme
`obrain memoire --appliquer` :

```text
obrain machines                                  # ce que le registre connaît
obrain machines --store <base> --appliquer        # ce qui devient un fait du graphe
```

Le registre journalise déjà (append-only, sans verrou) ; la passe ouvre la base en
**exclusif** et écrit en `MERGE` idempotent. En service, un timer la déclenche.

**Ce qui est écrit, et ce qui ne l'est pas.** Un nœud `machine:<hôte>` par machine
connue, portant des **faits datés** : compte, capacités **annoncées**, première
attache, dernière présence, et quelle machine a écrit le fait. Il n'y a
**aucune propriété « joignable »** — un booléen écrit à l'application serait
périmé une minute plus tard, et un état recopié qui ment est ce que la doctrine
interdit. Le lecteur compare `derniere_presence_ms` à l'heure ; le registre, lui,
garde `vivant` en mémoire, là où l'information est fraîche.

**Deux propriétés d'identité, et c'est mesuré.** `qn` **et** `qualified_name`. Le
dépôt porte deux conventions : la mémoire et le registre nomment `qn`
(`memoire.rs`, `remote.rs`), le graphe de code et la surface MCP qui parle à une IA
nomment `qualified_name` (`obrain-indexer`, `mcp_server.rs`). Une machine écrite
avec une seule des deux serait **invisible** à l'autre lecteur — et c'est
exactement ce que la première mesure a montré, `n.qualified_name` rendant vide sur
un nœud écrit avec `qn` seul. L'objectif du module étant qu'on trouve la machine,
on écrit les deux.

**La porte a été durcie, une fois de plus.** Son § 8 ne se contente plus de
constater qu'« un nœud `machine:` existe » : il vérifie que les **trois** machines
sont nommées, et que chaque nœud porte ses **capacités et son compte**, pas
seulement son nom — un nœud muet sur les capacités ne répondrait pas à « et que
sait faire chacune ? ». Et il déclenche la passe RÉELLE avant de constater, en
disant pourquoi : une instance d'acceptation n'a pas de timer. La porte n'invente
aucun nœud — si la passe échoue, l'assertion qui suit échoue.

**Verdict final : 37 ✅ / 0 ❌.** De 19 ✅ / 9 ❌ sur 28 vérifications, la porte
n'a jamais été assouplie : elle a été **durcie cinq fois** (une assertion ajoutée
par le registre durable, la non-fuite sur la route de flotte, les capacités
distinctes, les trois machines nommées, les capacités portées par chaque nœud) et
elle est aujourd'hui entièrement verte.
