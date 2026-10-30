# IAM branché — ce qui est appliqué, où, et sous quelle condition

Trace de revue, 2026-09-13. Tout ce qui suit a été **mesuré**, pas déduit : les
matrices de refus ont été rejouées sur le serveur HTTP et sur le pont MCP.

## Le constat de départ

L'audit a trouvé deux IAM, et **aucun des deux n'était appliqué là où le graphe
se sert** :

* `obrain-iam` — la bibliothèque du moteur — était **complète et testée**
  (5 395 lignes, 9 modules : identités, sessions, politiques, ORN, RGPD, store,
  95 tests verts) et n'avait **aucun appelant hors d'elle-même** : zéro point
  d'application. `session_ext.rs` décrivait son intégration en *pseudo-code*.
  La feature `iam` n'était ni dans `default` ni dans `ai` : **le build déployé ne
  la compilait même pas**.
* `obrain viz serve` servait le graphe sur **192.168.1.103:8432** sans demander
  la moindre identité — et rien ne le disait. Le pont MCP (le chemin du harness,
  celui qui écrit par le GraphWriter) non plus.

Autrement dit : la règle existait, personne ne la posait.

## Ce qui est branché

La règle vit **une seule fois**, dans `obrain_iam::Garde` (bibliothèque) :
`authentifier(AuthRequest::SessionToken)` puis
`check_graph_access(action, "tenant", <base>)`. Les points d'entrée ne font que
l'appeler — une correction de politique ne se réplique pas trois fois.

| point d'entrée | où la garde passe | couverture |
|---|---|---|
| `viz serve` | `handle()`, **avant** le `match` des routes | toutes les routes, y compris celles ajoutées demain |
| pont MCP | `call_tool()`, au **point de dispatch unique** | les 30+ outils exposés |
| `obrain query` / `shell` | note d'état sur `stderr` | **n'applique rien** — voir « La frontière locale » |

La portée est `("tenant", <base>)` : une politique autorise **une base**, pas les
autres (le nom d'espace est le radical du fichier, le même des deux côtés).

## Ce qui est vérifié

### Le serveur HTTP (matrice rejouée)

| | non armé (défaut) | armé |
|---|---|---|
| annonce au démarrage | `⚠ IAM NON ARMÉ — … quiconque atteint ce port lit et écrit tout le graphe` | `IAM ARMÉ — toute requête doit présenter une identité…` |
| `/api/info` | `"iam_armee": false` + l'état en clair | `"iam_armee": true` |
| requête **sans jeton** | **200** (rien ne change) | **401** + quoi présenter |
| jeton valide, base autorisée | — | **200** |
| jeton valide, **autre** base | — | **403** `resource=orn:obrain:graph:obrain:tenant/runtime-dev` |

Refus, tel qu'un client le reçoit :

```json
{"acces":"lecture","error":"permission denied: action=graph:read resource=orn:obrain:graph:obrain:tenant/runtime-dev","espace":"runtime-dev","http":403,"iam":"IAM ARMÉ — …"}
```

### Le pont MCP (JSON-RPC sur stdio)

| cas | résultat |
|---|---|
| non armé, sans jeton | **OK** — comportement inchangé |
| armé, sans `OBRAIN_IAM_TOKEN` | **refus** : `aucune identité présentée …` (`http: 401`) |
| armé + jeton, base autorisée | **OK** |
| armé + jeton, **autre** base | **refus** : `permission denied … tenant/runtime-dev` |

L'état est **publié dans la réponse `initialize`** (le pont n'a pas de terminal :
c'est le seul endroit où il peut dire sous quelle condition il sert le graphe).

### La garde elle-même (6 tests verts, `obrain-iam`)

`non_armee_laisse_passer_sans_identite` ·
`armee_refuse_sans_identite` · `armee_refuse_un_jeton_inconnu` ·
`armee_autorise_ce_que_la_politique_autorise` ·
`la_politique_ne_deborde_pas_sur_une_autre_base` ·
`une_ecriture_exige_le_droit_d_ecrire` (une politique `graph:read` **n'ouvre pas**
l'écriture sur la même base).

La bibliothèque est **fail-closed** : `PolicyDecision` n'a pas d'« abstention »
— pas de politique qui matche ⇒ `ImplicitDeny` ⇒ refus.

## Comment armer

```bash
# 1) Fabriquer le premier jeton (hors bande — aucun chemin ne l'émettait)
cargo run --release -p obrain-cli --features iam --example amorcer_iam -- \
    <base.obrain> <compte> <espace> [utilisateur]
#    → pose utilisateur + rôle + politique bornée à CET espace, imprime le jeton

# 2) Servir, armé
OBRAIN_IAM=1 OBRAIN_IAM_ACCOUNT=obrain OBRAIN_IAM_STORE=<base.obrain> \
    ./target/release/obrain viz serve …          # clients : Authorization: Bearer <jeton>

# 3) Pont MCP : le jeton voyage dans l'environnement du harness
OBRAIN_IAM_TOKEN=<jeton>
```

Construction : **`cargo build --release -p obrain-cli --features ai,iam`** — sans
`iam`, la garde n'est pas compilée et l'état le dit (`IAM NON COMPILÉ`).

## Pourquoi le défaut n'est PAS « refuser »

Le jour où la garde a été écrite, aucun client n'en présentait : le dashboard
(browser, sur `192.168.1.103`) et le pont du harness. Armer d'office aurait coupé
les deux instantanément. Le défaut est donc **« passer, mais le dire »** : l'état
est annoncé au démarrage du serveur, publié par `/api/info` et par
`initialize` du pont. Une porte non protégée ne doit jamais être silencieuse —
c'est exactement ce qui a laissé ce serveur exposé sans que personne ne le voie.

## Ce qui reste ouvert

* **Aucune route de remise de jeton.** Le serveur ne sait pas authentifier un
  utilisateur (pas de login, pas de `/auth`) : les jetons se créent hors bande
  avec l'exemple ci-dessus. Tant que ce chemin n'existe pas, armer suppose de
  fabriquer le premier jeton à la main — et un dashboard navigateur ne peut pas
  porter un en-tête `Authorization` sans un proxy qui l'injecte (ou un cookie).
* **La frontière locale est le système, pas l'IAM** : `obrain query` lit le
  fichier en direct. Exiger un jeton là ne protégerait rien et casserait des
  scripts ; la commande **dit** donc sa frontière au lieu de la taire. Défaut
  trouvé en la branchant : appeler la garde y ouvrait le store en lecture seule,
  ce qui faisait échouer `open_existing` (« already open read-only in this
  process »). La note se lit maintenant dans l'environnement, sans ouvrir aucun
  store.
* **`WWW-Authenticate: Bearer`** n'est pas envoyé avec le 401 (le `respond()`
  du serveur ne pose pas d'en-tête libre) : le corps dit quoi présenter, l'en-tête
  manque.
* Les autres IAM du parc (le middleware du framework `this`, ses 72 tests verts)
  ne sont pas touchés : ils restent la référence pour un serveur applicatif.
  Reste chez eux un défaut **assumé** : une route sans politique déclarée
  *passe* (`Abstain`) — l'inverse du fail-closed de cette garde-ci.

---

# Mise à jour du soir — la CHAÎNE COMPLÈTE, vérifiée

## Le parcours, maillon par maillon

Avec un magasin d'identités **dédié et ouvrable en écriture** :

| # | geste | résultat mesuré |
|---|---|---|
| 1 | `GET /api/auth/whoami` sans identité | `authentifie: false` |
| 2 | `POST /api/auth/login` mot de passe **faux** | **401** |
| 3 | `POST /api/auth/login` correct | **200** + jeton au corps **et** cookie `obrain_session` |
| 4 | `GET /api/query` **avec le cookie** (voie du navigateur) | **200** |
| 5 | `whoami` avec le cookie | `admin@obrain` |
| 6 | `POST /api/auth/logout` | `revoked: true` |
| 7 | `whoami` **après** la déconnexion | `false` |
| 8 | la même requête, même jeton, **après** | **401** |

Sans identité, tout `/api/*` répond **401** avec `WWW-Authenticate: Bearer realm="obrain"` ;
avec une identité que la politique n'autorise pas sur cette base, **403** nommant la ressource.

## Ce qu'il a fallu corriger pour la FERMER

| défaut mesuré | correction |
|---|---|
| **Jeton déterministe** (`DefaultHasher` + compteur : deux processus émettaient `obt_d32bf2e05a925e5f…0001`) | 256 bits tirés du système (`rand`) |
| **Révocation décorative** : la garde ouvrait le magasin en **lecture seule** — `revoked: true` mais 4 credentials avant / 4 après, session encore valide | ouverture en **écriture partagée** ; repli lecture seule **déclaré** (l'état et les routes le disent) |
| **Expiration jamais vérifiée** : le message disait « invalid or expired », le code ne testait rien | `iam_expires_ts` (époque en secondes) écrit **et** vérifié |
| **Horodatage fictif** : `now_iso()` était un compteur formaté en date (`2026-01-01T00:00:00Z`, `…:01Z`) | horloge réelle (`SystemTime`) |
| **Empreinte de jeton faible** (SipHash-64, sans sel : une collision suffit à faire passer un jeton pour un autre) | **SHA-256** — gratuit aujourd'hui, impossible demain sans casser des sessions |

## Deux exigences de déploiement, apprises à la mesure

1. **Construire avec `iam`** : `cargo build --release -p obrain-cli --features ai,iam`.
   La commande de `scripts/vizserve.sh` (`--features ai`) **ne l'inclut pas** : le
   binaire déployé répondait `IAM NON COMPILÉ` et `OBRAIN_IAM=1` restait sans effet.
   L'état est publié partout (démarrage, `/api/info`, `whoami`, `initialize` du pont).
2. **Un magasin d'identités DÉDIÉ et ÉCRIVABLE** (`OBRAIN_IAM_STORE=…/iam.obrain`).
   Pointé sur une base servie — donc ouverte en lecture seule — la connexion et la
   déconnexion deviennent décoratives ; la garde l'annonce alors en clair.

## Deux incidents d'atelier, notés parce qu'ils ont coûté du temps

* **Ce fichier et le manifeste ont été réécrits sous mes pieds** (écriture en masse
  d'une autre session, 14:46 puis 14:52) : la feature `iam` a perdu `dep:obrain-iam`,
  `viz_serve.rs` a perdu toute la garde — alors que `mcp_server.rs` la référençait
  toujours, donc **l'arbre ne compilait plus**. Vérifier le **contenu**, pas
  l'horodatage, avant de conclure qu'un correctif est en place.
* **Le disque s'est rempli** (680 K libres sur 936 G), à cause de `target/debug`
  gonflé par les allers-retours de compilation. J'ai libéré ce seul cache de test
  (62 G) ; rien de ce qui appartient aux autres dans `/tmp` (58 G) n'a été touché.

---

# Maillons ajoutés — renouvellement, piste d'audit, page de connexion

Trois manques restaient pour que la chaîne vive sans bricolage côté client.

## 1. Renouvellement de session (`POST /api/auth/refresh`)

`Garde::renouveler(jeton, ttl)` **tourne** la session : nouveau jeton, **et l'ancien
révoqué**. Un renouvellement qui n'invalide pas l'ancien laisse une clé valable
derrière chaque renouvellement — la session s'accumule au lieu de tourner.
Vérifié par test : `le_renouvellement_remplace_le_jeton_et_tue_l_ancien` (le nouveau
ouvre, **l'ancien est mort**).

## 2. Piste d'audit (`GET /api/auth/audit?limit=`)

La bibliothèque **savait** journaliser (`IamStore::log_audit_event`, utilisée par
l'export RGPD) et **aucune décision de sécurité n'y était écrite** : ni les refus,
ni les connexions, ni les révocations. Un IAM sans trace ne répond pas à « qui a
été refusé, et quand ». Désormais tracés : connexion refusée, connexion acceptée,
ouverture/fermeture de session, renouvellement, et **chaque verdict** d'accès
(`Allow`/`Deny`). Vérifié par test : `les_decisions_sont_tracees`.

La route est gardée par une action **nommée** (`iam:audit`) : une politique peut
accorder la lecture de la piste **sans** accorder celle du graphe — et l'inverse.

## 3. Page de connexion (`/login`) et redirection de l'accueil

Armer la garde rendait le **dashboard** inutilisable : la page se chargeait (les
actifs statiques restent publics, exprès) mais ses appels `fetch` recevaient 401
sans qu'aucun écran permette d'obtenir un cookie — `fetch` n'envoie pas
d'identifiants qu'on ne lui a pas donnés. Le serveur sert donc une page de
connexion autonome (formulaire `form-urlencoded`, lisible par le serveur), pose le
cookie et **renvoie le navigateur vers l'accueil**. L'accueil, quand la garde est
armée et l'identité absente, redirige vers `/login` (303).

## Ce qui bloque la vérification *en direct* de ces trois maillons

Le fichier `crates/obrain-cli/src/viz_serve.rs` est **en cours d'édition par une
autre session** (préchauffage des trigrammes) et **ne compile pas** à l'instant de
cette note (`PROPS_TEXTUELLES` introuvable, puis `s.property_keys` ne vit pas assez
longtemps). Mes apports y sont présents (`GardeIam`, `api_auth_refresh`,
`api_auth_audit`, `PAGE_CONNEXION`), mais **rien de neuf ne peut être construit ni
essayé** tant que ce fichier reste cassé. S'y ajoute une reconstruction du binaire
**sans `iam`** à 16:46 (`nm` : 0 symbole `obrain_iam`) : la commande documentée par
`scripts/vizserve.sh` reste `--features ai`, qui **n'embarque pas la chaîne IAM**.

Les trois maillons sont **prouvés au niveau bibliothèque** (11/11, dont rotation et
audit) ; leur câblage HTTP suit exactement le motif déjà vérifié en direct pour la
connexion, le cookie, la requête et la déconnexion.

---

# ÉTAT STABILISÉ — et comment le revérifier toi-même

Le blocage précédent (fichier de travail cassé, binaire reconstruit sans `iam`) est
levé : l'arbre compile, et la chaîne entière a été **revérifiée de bout en bout**
par un script qui ne dépend plus d'une vérification « à la main ».

## Le script

```bash
scripts/verif-chaine-iam.sh [base-de-donnees.obrain] [port] [magasin-iam.obrain]
```

Par défaut : la base **libre** `runtime.obrain` (jamais une base servie), le port
**8435** (jamais 8432/8433, qui sont à toi), et `iam-verif.obrain` comme magasin
d'identités. Il rend **0 si tout passe**, 2 si le binaire n'embarque pas l'IAM, et
le nombre d'échecs sinon. Il arrête toujours son serveur (`trap`).

Ce qu'il vérifie, et qui est passé le 2026-09-13 à 17:32 :

```
1) binaire ......................... IAM compilé (79 symboles obrain_iam)
2) magasin dédié ................... utilisateur amorcé, politiques bornées à l'espace
3) serveur armé (port 8435) ........ garde ARMÉE
4) la chaîne
   accueil sans session ............ 303 → /login
   page /login ..................... 200, formulaire présent
   connexion refusée ............... 401
   connexion par formulaire ........ 303 + cookie posé
   requête AVEC le cookie .......... 200        ← la voie du navigateur
   whoami .......................... verif@obrain
   piste d'audit ................... lisible (5 événements)
   renouvellement .................. 200, jeton tourné
   ANCIEN jeton après rotation ..... 401        ← la rotation tue l'ancien
   nouveau jeton ................... 200
   déconnexion ..................... 200
   session fermée .................. 401
   401 ............................. WWW-Authenticate: Bearer présent
5) verdict ......................... CHAÎNE COMPLÈTE — tout passe
```

## Les deux seules choses qui dépendent de toi

1. **Construire avec l'IAM** : `cargo build --release -p obrain-cli --features ai,iam`.
   `scripts/vizserve.sh` lance `./target/release/obrain` **tel quel** : si le
   dernier build a été fait sans `iam`, le binaire servi répond `IAM NON COMPILÉ`
   — c'est annoncé au démarrage, dans `/api/info`, dans `whoami` et dans
   `initialize` du pont, et le script ci-dessus le détecte en une seconde (étape 1).
2. **Un magasin d'identités dédié et écrivable** (`OBRAIN_IAM_STORE=…/iam.obrain`),
   et `OBRAIN_IAM=1` pour armer. Sur une base servie — donc ouverte en lecture
   seule — connexion et déconnexion redeviennent décoratives ; la garde le dit.

## Ce qui reste hors de la chaîne

Pas de MFA (le seul HMAC disponible viendrait d'une dépendance absente du
workspace : l'improviser serait pire que l'absence) · pas de renouvellement
automatique côté client · le CLI local reste borné par le système, et le dit ·
l'énumération des sessions d'une identité n'existe pas (pas de « mes appareils »).

---

# ARMÉ EN SERVICE SUR 8432 (2026-09-13, 18:47)

La chaîne ne fait plus que passer les tests : **elle sert**. `obrain viz serve`
sur `192.168.1.103:8432` tourne avec `OBRAIN_IAM=1`, et voici ce que ça donne,
mesuré sur ce port :

| geste | résultat |
|---|---|
| requête **sans** identité | **401** (+ `WWW-Authenticate: Bearer`) |
| accueil `/` sans session | **303** → `/login` |
| connexion par formulaire | **303** + cookie `obrain_session` |
| requête **avec** le cookie | **200** — le dashboard fonctionne |
| `whoami` | `admin@obrain` |
| base **hors des 7 presets** | **403** — la portée tient |
| renouvellement | **200**, ancien jeton mort |
| piste d'audit | `{"action":"graph:read","principal":"…user/admin","quand":"2026-09-13T18:47:48Z","ressource":"…tenant/runtime-dev","verdict":"Allow"}` |

L'horodatage de la piste est **réel** (`18:47:48Z`) — c'est le correctif le plus
visible de la journée : avant, `now_iso()` était un compteur, donc l'audit
n'aurait daté aucun incident.

## Ce qu'il faut savoir pour travailler avec cette garde armée

* **Se connecter** : `http://192.168.1.103:8432/login` — utilisateur `admin`,
  mot de passe engendré et déposé dans
  `~/.local/share/obrain-harness/data/iam.obrain.identifiants.txt` (droits 600).
  Le cookie dure 8 h (`OBRAIN_IAM_TTL_SECS`).
* **Un script** qui interroge 8432 doit présenter un jeton :
  `Authorization: Bearer <jeton>`, ou `?token=` (le flux SSE d'`EventSource`).
* **Le pont MCP n'est PAS armé** : c'est un autre processus, lancé par le
  harness, et il ne reçoit `OBRAIN_IAM=1` que si le harness le pose. Les outils
  graphe du harness fonctionnent donc comme avant. Pour l'armer aussi :
  `OBRAIN_IAM=1 OBRAIN_IAM_STORE=…/iam.obrain OBRAIN_IAM_TOKEN=<jeton>` dans
  l'environnement du harness.
* **Désarmer** : relancer 8432 sans `OBRAIN_IAM=1` (la garde reste compilée,
  l'état est annoncé : « ⚠ IAM NON ARMÉ »).
* **Espaces autorisés** : les sept presets, chacun par sa propre politique
  (`runtime-dev`, `runtime-prod`, `self-demo`, `linux-full3`, `kv-linux-e1000`,
  `engine_idx`, `kv-linux-driversnet`), actions `graph:read`, `graph:write`,
  `iam:audit`. Une base hors de cette liste est refusée — c'est le cas mesuré
  en 403.

## La cause racine, corrigée

`scripts/vizserve.sh` documentait `cargo build --release -p obrain-cli
--features ai` : la chaîne IAM est derrière sa **propre** feature, donc un
binaire ainsi construit annonce `IAM NON COMPILÉ` et `OBRAIN_IAM=1` reste sans
effet. Un `cargo build` ordinaire lancé pendant un chantier remplaçait le
binaire complet — **trois fois aujourd'hui**. La ligne documentée est désormais
`--features ai,iam`, et `scripts/verif-chaine-iam.sh` le contrôle en une seconde.
