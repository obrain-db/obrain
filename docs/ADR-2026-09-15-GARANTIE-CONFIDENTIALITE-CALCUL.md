# ADR — Garantie de confidentialité du calcul : « personne n'écoute, pas même celui qui exécute l'engine »

**Date** : 2026-09-15. **Statut** : **PROPOSÉ** — la variante du self (§6) reste à
trancher ; le §5 (état actuel) et le §7 (étape 0) ne dépendent pas de ce choix.

**Nature** : architecture decision record. Tissé dans le graphe par le canal
`docs/` → `import_docs --plans_dir` → nœud `Plan` (l'import appartient au
writer — voir l'ADR du 2026-09-13).

**Régime des affirmations.** Chaque affirmation sur l'état actuel porte son
fichier. Le mot **CONSTATÉ** signifie « relevé par lecture du code, avec le
fichier cité » ; **MESURÉ** n'est employé nulle part ici, faute de relevé. Aucun
chiffre de performance TEE, FHE ou MPC de ce document n'a été mesuré dans ce
dépôt : ils sont explicitement marqués **NON MESURÉ** et devront être relevés
avant décision s'ils deviennent déterminants.

---

## 1. Contexte

La demande, telle qu'elle a été posée :

> Comment s'assurer que les calculs lancés par des utilisateurs ne puissent pas
> être écoutés par d'autres — **même par celui qui exécute l'engine** ?

Puis, sur le modèle de menace : **« tout le monde » doit être couvert, et cela
doit être une des garanties du système** — pas un réglage, pas une politique, pas
une promesse.

Ce document ne décrit donc pas un durcissement. Il décrit ce qu'il faudrait
changer pour que la confidentialité d'un calcul soit une **propriété structurelle
du système**, et il compare les trois architectures qui peuvent y prétendre.

### 1.1 Le fait qui commande tout le reste

**Aucune mesure logicielle ne peut cacher un calcul à celui qui exécute le
moteur.** Ce n'est pas une difficulté d'ingénierie, c'est une impossibilité, pour
trois raisons cumulatives :

1. **Il contrôle le code.** Toute isolation écrite dans ce dépôt est *exécutée
   par lui*. Il peut modifier `obrain-engine`, rebrancher un `tracing::info!`,
   interposer un allocateur, instrumenter `memcpy`. La garantie porterait sur
   « ce binaire-là », or c'est lui qui choisit le binaire.
2. **Le clair doit exister en RAM.** Pour qu'un calcul sur `x` rende `y`, `x` et
   `y` traversent ses registres. Ce qui est lisible est copiable : vidage de
   mémoire, swap, `/proc/<pid>/mem`, compteurs de performance.
3. **Même sans tricher, il reste les à-côtés** : le texte de la requête, la table
   des sessions, le WAL, la durée, la taille du résultat.

**Conséquence directe : la demande n'admet que trois sorties.**

| Sortie | Principe | Nom |
|---|---|---|
| **A** | ne jamais lui donner le clair | chiffrement de bout en bout, FHE, MPC, calcul local |
| **B** | le clair dans une boîte scellée par le silicium | VM confidentielle + attestation vérifiée par le client |
| **C** | le clair ne quitte pas la machine de l'utilisateur | calcul chez lui ; l'engine ne voit que des enveloppes |

Tout le reste — permissions, journalisation, bonnes pratiques — relève de la
**dissuasion et de la détection**. C'est utile, souvent suffisant, mais cela ne
répond pas à la phrase « même celui qui exécute ». Le §3 nomme cette distinction
au lieu de la masquer.

### 1.2 Le constat mesuré le plus dur, et il est immédiat

**CONSTATÉ** : il n'existe **aucun TLS dans tout le workspace**. Un `grep` sur
`rustls`, `native-tls`, `openssl` et `tls` dans l'ensemble des `Cargo.toml` ne
rend **rien** ; aucune occurrence de `TlsAcceptor`, `rustls::` ou
`ServerConfig::builder` dans les `crates/*.rs`.

Le serveur est un **HTTP/1.1 écrit à la main** sur `std::net::TcpListener`
(`crates/obrain-cli/src/viz_serve.rs`, `fn handle`, lecture d'en-têtes à la main
avec les refus 400 documentés en tests).

Autrement dit : **aujourd'hui, la garantie ne tient pas même contre un
observateur passif du réseau.** Un `tcpdump` sur le LAN rend les jetons porteurs,
les requêtes et les résultats. C'est le point de départ réel, et il ne dépend
d'aucun des choix architecturaux qui suivent.

---

## 2. Ce qu'on entend par « garantie »

Sans définition, « garantie » veut dire « promesse ». Quatre critères, à opposer
à chaque affirmation du système :

1. **Vérifiable par la partie qui la subit** — l'utilisateur contrôle la preuve,
   pas le serveur qui l'annonce.
2. **Vraie même si tous les nœuds sont hostiles** — y compris le concepteur du
   système.
3. **Sans point de défaillance unique** — ni clé maîtresse, ni administrateur de
   confiance, ni porte de récupération.
4. **Sa défaillance est bruyante** — si la garantie tombe, cela se voit.

### 2.1 Le principe directeur : remplacer le contrôle par l'impossibilité

Une politique est un **contrôle** : un défaut de code la contourne. Un chiffrement
dont la clé est absente est une **impossibilité** : il n'y a rien à contourner.

Ce dépôt a déjà payé pour l'apprendre. **CONSTATÉ**, et documenté dans
`docs/ADMINISTRATION-IAM-2026-09-13.md` : `delete_policy` et `delete_role`
marquaient `iam_status = "deleted"` depuis toujours, mais `node_to_policy` et
`node_to_role` ne lisaient jamais ce champ — or `evaluate_access` collecte les
politiques **en passant par les rôles**. Une politique « supprimée » continuait
donc d'accorder ses droits, en silence. La règle existait, la décision était
fausse, et rien ne le disait.

La leçon est transférable telle quelle : **là où la confidentialité repose sur une
décision, elle repose sur une décision qui peut être fausse sans que personne ne
le sache.** Là où elle repose sur une clé absente ou une mesure de code
non conforme, elle ne le peut pas.

### 2.2 Les cinq invariants à tenir

| # | Invariant | Ce qu'il garantit | Moyen |
|---|---|---|---|
| **G1** | **Contenu** | personne ne lit les données, ni au repos ni en cours de calcul | AEAD par espace, clé jamais utilisable seule (seuil) |
| **G2** | **Calcul** | personne ne voit *ce que* le calcul fait : requête, plan, paramètres | exécution scellée attestée, ou MPC, ou local |
| **G3** | **Métadonnées** | personne ne déduit qui calcule quoi, quand, combien | enveloppes de taille fixe, regroupement, bourrage, trafic de couverture |
| **G4** | **Intégrité** | personne ne substitue un résultat ni un binaire sans être détecté | mesure de code attestée + reçus signés |
| **G5** | **Inférence** | aucun résultat ne révèle les données d'un autre | isolation structurelle (ne jamais croiser), plancher d'agrégation, budget DP |

**G5 est celui qu'on oublie, et il annule tous les autres.** Un enclave parfait
qui laisse une requête lire les nœuds d'autrui les divulgue **par le résultat**.
La confidentialité du calcul a deux moitiés : celle du calcul, et celle des
données co-résidentes qu'il touche.

---

## 3. L'échelle honnête des forces

Il n'existe pas « une » garantie : il existe une échelle, et le système doit dire
à l'utilisateur sur quelle marche il se trouve — sinon il vend la plus haute en
livrant la plus basse.

| Niveau | Nature de la garantie | Tient contre « tout le monde » ? | Coût dominant |
|---|---|---|---|
| **N1 — Calcul chez l'utilisateur** | **mathématique** : rien ne sort, il n'y a rien à écouter | **oui, totalement** | ergonomie (l'utilisateur exécute du code) |
| **N2 — MPC / FHE** | **cryptographique** : aucune partie ne détient le clair | oui, sous réserve de la correction du protocole | perf et communication — **NON MESURÉ** ici |
| **N3 — TEE attesté (SEV-SNP / TDX)** | **technique** : silicium + micrologiciel ; pas une preuve mathématique | en pratique oui, en théorie non | dépendance à AMD/Intel, complexité d'attestation |
| **N4 — Chiffrement au repos + audit chaîné** | **détective** : le clair est visible pendant le calcul | **non** — détecte, ne protège pas | faible |

La cible « tout le monde » impose donc : **N1 par défaut, N2 pour le croisement
de données, N3 seulement pour ce qui ne peut être ni local ni cryptographique, et
N4 comme filet — jamais comme garantie.**

---

## 4. Ce que le système fait déjà — et contre qui

| Pièce | Fichier | Ce que ça protège réellement |
|---|---|---|
| Garde d'entrée, décision unique, **fail-closed quand armée**, état publié sur `/api/info` | `crates/obrain-iam/src/garde.rs` | les **autres utilisateurs** — **à condition** que `OBRAIN_IAM=1` (non armée par défaut) et que le binaire soit compilé `--features iam` |
| Espaces, rôles, politiques bornées à `("tenant", espace)`, actions `graph:read` / `graph:write` / `iam:audit` / `iam:admin` | `crates/obrain-iam/src/admin.rs` | cloisonnement **logique** entre espaces |
| Isolation cognitive par tenant : DashMap distinctes par tenant, « isolation is structural, not a property filter » | `crates/obrain-cognitive/src/tenant.rs` | les autres tenants — **en mémoire cognitive seulement** |
| Sic transit : mots de passe **bcrypt** (`set_user_password`, `DEFAULT_COST`) ; jetons **256 bits tirés du système**, empreinte SHA-256 hex | `crates/obrain-iam/src/provider.rs` | vol de base d'identités |
| Journal d'audit : `principal`, `action`, `resource`, `result`, `timestamp` | `crates/obrain-iam/src/model.rs` (`AuditEvent`) | **détection**, à une réserve près (ci-dessous) |
| CRC32 par page (`seal_crc32`) | `crates/obrain-substrate/src/page.rs`, `props_zone.rs` | **l'intégrité seulement** — aucune confidentialité |
| Primitives cryptographiques disponibles dans le workspace | `Cargo.toml` racine : `bcrypt`, `sha2` | — |
| Chiffrement au repos, AEAD, asymétrique, attestation, TEE, FHE, MPC | *(rien)* | **rien** |

### 4.1 Les réserves, nommées

1. **« Immuable » est une intention, pas une propriété.** `model.rs` décrit
   `IamAuditEvent` comme une « immutable audit trail entries ». Mais **CONSTATÉ** :
   il n'y a ni chaînage par hachage, ni signature, et le magasin appartient à
   l'opérateur. Il peut supprimer ou réécrire un événement sans que rien ne le
   signale — ce qui disqualifie l'audit comme garantie (il reste utile comme
   hygiène).
2. **Les jetons sont hachés en SHA-256 sans sel** (`hash_token`). Le commentaire
   du code l'assume : « in production this would use bcrypt/argon2 with salt ».
   Cela reste un progrès par rapport au `DefaultHasher` qu'il remplace, pas une
   cible.
3. **Le transport est en clair** (§1.2) : ni TLS, ni authentification du serveur,
   ni confidentialité du canal. C'est le trou le plus large et le moins cher à
   fermer.
4. **Le clair est sur le disque.** Aucun chiffrement au repos : les pages du
   substrat portent un CRC32, qui détecte la corruption et ne cache rien. Un
   `hexdump` du `.obrain` rend le graphe, sans même patcher le moteur.

### 4.2 Conclusion du constat

| Adversaire | Couvert aujourd'hui ? |
|---|---|
| un autre utilisateur du même engine | **partiellement** — via l'IAM, quand la garde est armée et le binaire compilé pour |
| un observateur passif du réseau | **non** |
| celui qui exécute l'engine (lecture du fichier, du WAL, de la RAM) | **non** |
| celui qui exécute l'engine (lecture du résultat d'un calcul légitime) | **non** |

---

## 5. Le conflit à trancher : un self qui comprend voit

C'est la décision qui fixe toute l'architecture, et elle n'a pas de contournement.

**Comprendre une demande, c'est la lire.** Un self qui interprète « range-moi ces
nœuds par communauté et dis-moi ce qui a bougé » doit accéder au sens de la
demande, donc aux données qu'elle désigne. Il n'existe pas de variante où le self
comprend et où, simultanément, rien de lisible ne le traverse.

Trois architectures en découlent.

### V-A — Self scellé (TEE attesté)

**Principe.** Le self et la cellule de calcul tournent dans une VM confidentielle
(AMD SEV-SNP, Intel TDX) ou un conteneur confidentiel. Les clés sont descellées
**dans** l'enclave, et **seulement après** que le client a vérifié un rapport
d'attestation dont la mesure correspond au binaire publié. L'opérateur
(l'hyperviseur) ne lit pas la RAM, et ne peut pas substituer le code sans que le
client le voie et refuse la clé.

**Qui détient quoi.** Le client détient la clé racine et le vérificateur
d'attestation. L'enclave détient le clair, mais scellé. L'engine, hors enclave, ne
détient que du chiffré.

| Invariant | Verdict |
|---|---|
| G1 contenu | ✅ tenu contre l'opérateur ; la confiance se déplace vers AMD/Intel |
| G2 calcul | ✅ tenu — le plan et les paramètres restent dans l'enclave |
| G3 métadonnées | ⚠️ partiel — l'opérateur voit les tailles, la durée, la fréquence ; bourrage nécessaire |
| G4 intégrité | ✅ **le mieux tenu** — c'est la seule variante où la substitution de code est détectée par le client |
| G5 inférence | ❌ non traité par le TEE : un calcul qui croise des données les divulgue |

**Ce qui reste fuyant.** Les canaux auxiliaires du silicium (motifs d'accès
mémoire, historique d'attaques sur SGX, incidents micrologiciels AMD) — c'est
précisément pourquoi N3 est « technique » et non « mathématique ». Et les
métadonnées, que le TEE ne masque pas.

**Coût.** Complexité opérationnelle : provisionnement d'attestation, registre des
mesures de build, builds reproductibles, rotation de clés scellées. Surcoût de
performance **NON MESURÉ** ici (les ordres de grandeur publics vont de quelques
pour cent à ~30 % selon la charge mémoire — à relever avant décision).

**Ce que ça casse.** Rien du produit : le self garde toute son intelligence. Mais
le système devient **dépendant d'un fondeur** : si l'attestation n'est plus
disponible ou plus crue, il ne reste aucun plancher.

**Réversibilité.** Faible. Une fois les clés scellées et les flux bâtis sur
l'attestation, redescendre demande de reprendre l'architecture.

### V-B — Self aveugle par construction

**Principe.** Le self ne voit que des **enveloppes opaques** et des **jetons de
capacité**. Il route, ordonnance, facture — il ne comprend pas. La compréhension
vit dans le **micro-harness de l'utilisateur**, là où sont déjà la clé et la
machine. Le self est un facteur, pas un lecteur.

**Qui détient quoi.** L'utilisateur détient tout le clair et toute
l'interprétation. L'engine détient du chiffré et des enveloppes de taille fixe.
Rien à desceller : il n'y a pas de clé à protéger côté engine, donc pas de clé à
voler.

| Invariant | Verdict |
|---|---|
| G1 contenu | ✅ tenu **mathématiquement** — l'engine n'a jamais la clé |
| G2 calcul | ✅ tenu par construction — la requête ne quitte pas le micro-harness |
| G3 métadonnées | ⚠️ c'est **le** point faible de V-B : le self voit *quand*, *combien*, et avec qui ; enveloppes de taille fixe et regroupement obligatoires |
| G4 intégrité | ⚠️ l'utilisateur vérifie ses propres calculs ; la conformité d'un calcul délégué demande des reçus signés côté enclave |
| G5 inférence | ✅ **le mieux tenu** — ne jamais croiser est la seule parade complète à l'inférence |

**Ce qui reste fuyant.** Le graphe des métadonnées : qui parle à qui, à quelle
fréquence. Un adversaire qui observe longtemps peut reconstruire beaucoup d'un
graphe social sans jamais lire un octet de contenu. Le bourrage et le regroupement
réduisent ce canal, ils ne l'annulent pas.

**Coût.** Faible en infrastructure, réel en conception : il faut définir le
vocabulaire des jetons de capacité, la forme des enveloppes, et le protocole par
lequel un micro-harness demande une action sur une machine.

**Ce que ça casse.** Le self perd l'intelligence sémantique — **au sens où elle
change de lieu**. L'expérience « je parle à un self » peut être intégralement
préservée, parce que le self est une **interface**, pas un lieu : l'utilisateur
parle toujours au self, mais la partie qui comprend tourne chez lui.

**Réversibilité.** Élevée. C'est un plancher : on peut toujours ajouter une
enclave au-dessus sans le défaire.

### V-C — Hybride : self scellé, plans sans données

**Principe.** Le self tourne dans l'enclave et comprend, mais **seuls des plans
sans données** en sortent : des programmes de calcul, à forme constante, que
l'engine exécute sur du chiffré. Le self voit le sens, l'engine ne voit qu'une
forme.

| Invariant | Verdict |
|---|---|
| G1 contenu | ✅ tenu si le plan ne porte pas de données |
| G2 calcul | ⚠️ **le point de rupture** : un plan *est* une description du calcul |
| G3 métadonnées | ❌ le plan doit être à forme constante, sinon sa forme trahit son objet |
| G4 intégrité | ✅ comme V-A |
| G5 inférence | ❌ non traité |

**Ce qui reste fuyant, et c'est décisif.** Le plan fuit. « Lis le nœud X »
révèle X, quelle que soit la manière de l'encoder ; « compte les voisins de X »
révèle X. Pour ne rien révéler, le plan doit être **indépendant de la demande** —
c'est-à-dire un parcours exhaustif, dont le coût ne dépend pas de la question. On
retrouve alors le domaine de l'ORAM et du calcul oblivious, dont le prix est
précisément de rendre toute lecture indiscernable : **NON MESURÉ** ici, mais
structurellement coûteux.

**Coût.** Le cumul de V-A et de la difficulté d'ORAM — c'est-à-dire le plus élevé
des trois.

**Ce que ça casse.** Rien au niveau produit ; beaucoup au niveau performance et
simplicité, pour une garantie G2 qui reste partielle.

---

## 6. Matrice de décision

Critères pondérés selon la demande : *« tout le monde » doit être couvert, et cela
doit être une garantie du système*.

| Critère | V-A Self scellé | V-B Self aveugle | V-C Hybride |
|---|---|---|---|
| Tient contre l'opérateur de l'engine | ✅ (technique) | ✅ (mathématique) | ✅ (technique) |
| Tient contre un observateur réseau | à faire dans les trois (§7) | idem | idem |
| G2 — le calcul n'est pas visible | ✅ | ✅ | ⚠️ partiel |
| G3 — métadonnées | ⚠️ | ⚠️ | ❌ |
| G5 — inférence | ❌ | ✅ | ❌ |
| Le self **comprend** la demande | ✅ | ❌ *en tant que lieu*, ✅ *comme interface* | ✅ |
| Disponible **aujourd'hui**, sans matériel | ❌ | ✅ | ❌ |
| Dépendance à un tiers (fondeur) | forte | **aucune** | forte |
| Coût d'infrastructure | élevé | faible | le plus élevé |
| Réversibilité | faible | **élevée** | faible |
| Force de la garantie | technique | **mathématique** | technique |

---

## 7. Décision proposée

**Proposition de l'auteur de cette ADR, la décision restant à l'humain :**

> **V-B est le plancher. V-A est un plafond optionnel, ajouté plus tard, et
> seulement là où un calcul ne peut être ni local ni cryptographique.**

Cinq raisons, dans l'ordre de poids :

1. **La demande dit « garantie », et « tout le monde ».** Or une garantie qui
   dépend d'AMD et d'Intel n'est pas mathématique : c'est une garantie
   technique (N3). V-B est la seule des trois à produire une garantie **N1** —
   l'engine ne peut pas lire ce qu'il n'a pas.
2. **V-B est la seule disponible aujourd'hui.** Elle ne demande ni matériel
   particulier, ni registre de mesures, ni builds reproductibles. V-A exige les
   trois.
3. **V-B ne coûte pas la compréhension, elle la déplace.** « Je parle à un self »
   reste vrai : le self est une interface. Ce qui change est que la partie qui
   interprète tourne sur la machine de l'utilisateur — là où sont déjà la clé, la
   confiance et l'intérêt.
4. **V-B est réversible, V-A ne l'est pas.** Un plancher se recouvre ; un
   plancher qu'on n'a pas ne se rattrape pas. Si l'attestation devient
   indisponible, inutilisable ou décrue, V-A n'a plus rien en dessous.
5. **V-C ne satisfait pas G2**, et c'est son objet même. Construire une
   architecture dont la pièce centrale fuit est un mauvais échange ; elle
   n'existe ici que pour être écartée en connaissance de cause.

**Ce que V-B exige en contrepartie**, et qui doit être assumé :

* des **enveloppes de taille fixe** et du **regroupement**, sinon G3 est perdu et
  le graphe de métadonnées se reconstitue à l'observation ;
* l'acceptation explicite que le self ne fait plus de sémantique — tout ce qui
  demande de comprendre vit dans le micro-harness ;
* un **jeton de capacité** comme unique monnaie d'action du self : il délègue une
  autorisation, jamais une donnée.

**Si la décision devait être V-A**, alors la condition non négociable est que le
client **vérifie l'attestation avant de livrer la clé** — une enclave dont
l'attestation n'est pas vérifiée par l'utilisateur n'est pas une garantie, c'est
une promesse avec du matériel autour.

---

## 8. Étapes, avec le critère de vérification de chacune

Chaque étape est indépendante du choix V-A / V-B : **on peut commencer la 0, la 1
et la 2 sans avoir tranché le §7.**

| # | Étape | Critère de vérification (falsifiable) |
|---|---|---|
| **0** | **TLS sur toutes les surfaces d'entrée** — `viz serve`, pont MCP, CLI distant | `tcpdump -A` sur une requête `/api/query` ne montre **ni** le jeton **ni** la requête **ni** le résultat. Aujourd'hui il montre les trois (§1.2) |
| **1** | **Un espace = un magasin**, clé par espace, armé par défaut | retirer la clé d'un espace rend ses données illisibles **même avec tous les droits IAM** |
| **2** | **AEAD par page** à la place du CRC32 seul ; clé descellée hors du substrat | `hexdump` du `.obrain` ne rend aucun littéral du graphe |
| **3** | **Enveloppes opaques + jetons de capacité** (le cœur de V-B) | l'engine reçoit une enveloppe de taille fixe et ne peut en extraire ni la requête ni la cible |
| **4** | **Mesure de code + reçus signés + registre chaîné** | un binaire patché **n'obtient jamais la clé** ; omettre un calcul est détectable par l'utilisateur |
| **5** | **Croisement de données en 2-of-2** (ou FHE sur prédicats étroits) | aucun nœud seul ne reconstitue le clair d'un croisement |

**Pourquoi commencer par 0 et non par 3.** L'étape 3 est la plus intéressante et
la moins urgente : tant que le transport est en clair, tout ce qu'on édifie
au-dessus est lisible par le premier venu sur le réseau. **L'étape 0 est la seule
qui protège immédiatement, contre tout le monde, sans rien décider.**

---

## 9. Ce que cette ADR ne décide pas

* **La variante du self** (§7) — c'est la décision qui reste ouverte.
* **Perte de clé.** Si l'utilisateur perd sa clé, qui peut l'aider ? Toute
  réponse qui dit « le système » recrée un point de défaillance unique et annule
  le critère 3 du §2. La seule réponse cohérente avec la garantie est : personne,
  et l'utilisateur doit le savoir avant.
* **Multi-appareils.** La clé doit voyager entre les machines de l'utilisateur
  sans traverser l'engine.
* **Coût chiffré de TEE, FHE et MPC sur les charges réelles de ce dépôt.** Les
  trois sont **NON MESURÉS** ici, et les chiffres publics ne sont pas
  transposables : un parcours de graphe à degré élevé n'a pas le profil d'un
  circuit bootstrappé.
* **Ce que « tout le monde » inclut au-delà du réseau** : un adversaire qui
  contrôle le fondeur du CPU de l'utilisateur, ou son système d'exploitation,
  sort du périmètre de cette ADR.

## 10. Règles transférables

* **Comprendre, c'est lire.** Aucune architecture ne fait comprendre un self sans
  qu'il voie la demande. Choisir, c'est choisir *où* la compréhension vit.
* **Une politique qui a tort ne le dit pas ; une clé absente ne peut pas avoir
  tort.** Préférer l'impossibilité au contrôle, partout où c'est possible.
* **« Immuable » sans chaînage ni signature est un souhait.** Le magasin d'audit
  appartient à l'opérateur : il peut réécrire l'histoire.
* **Le résultat divulgue autant que la requête.** Un enclave qui croise des
  données les publie — G5 se traite à la conception, pas après.
* **Un chiffrement sans transport chiffré ne protège rien.** Le trou le plus
  large de ce dépôt (`grep` sans résultat sur TLS) est le moins cher à fermer.
* **Dire sur quelle marche de l'échelle on se trouve.** Vendre N1 et livrer N4
  est le seul défaut de ce document qui serait irréparable, parce qu'il ne se
  verrait que chez l'utilisateur.

---

## 11. État de l'étape 0, au 2026-09-15

**La brique existe, et le critère du §8 s'exécute sans me croire.**
`crates/obrain-tls` porte le transport chiffré ; `scripts/acceptation-tls.sh`
exécute le critère avec des outils qui ne savent rien de ce dépôt — `curl` comme
client, `openssl s_client` pour lire le protocole négocié, `grep` pour interroger
la capture, `od` pour le premier octet. **16 vérifications, 0 échec** : TLS 1.3
négocié, une requête aboutit, un client qui ne connaît pas l'autorité est
refusé, du HTTP en clair sur le port chiffré n'obtient rien, le jeton reste exigé
*dans* le tunnel, et la capture des deux sens ne rend ni le jeton, ni la requête,
ni le résultat.

`tcpdump` demande des droits que cet utilisateur n'a pas : la capture est produite
par un relais (`examples/serveur_tls.rs relais`), qui enregistre tous les octets
passant dans les deux sens. **Le témoin, sans lequel la capture ne prouverait
rien** : le même montage devant un serveur en clair (`--clair`) retrouve le jeton
et le secret. C'est la seule chose qui distingue une preuve d'une décoration.

**Le seul point encore ouvert : le branchement sur `run_serve()`**
(`crates/obrain-cli/src/viz_serve.rs`). La crate vit pour l'instant **à côté** du
serveur : tant que ce branchement n'est pas posé, le serveur réel sert toujours en
clair, et le §1.2 de ce document reste vrai mot pour mot. Le contrat est prêt —
`FluxTls` implémente `Read + Write`, et les trois réglages de session que `handle`
pose déjà (`set_read_timeout`, `set_write_timeout`, `set_nodelay`) sont exposés,
précisément pour que la migration ne perde pas en silence la protection
anti-slowloris.

**Limites assumées de la crate**, à ne pas découvrir à l'usage : pas
d'authentification de client (le jeton porteur reste la seule identité, et un
jeton volé reste un jeton volé), pas de rotation ni d'ACME ni de révocation
(certificat rechargé au démarrage), pas d'échange de clés post-quantique avec le
fournisseur `ring`, et les métadonnées — tailles, durées, fréquences — restent
visibles : **G3 n'est pas traité par le transport.**
