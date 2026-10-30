# CONSOLIDATION DES LIGNÉES — 2026-09-16

*Ce document dit ce qui a été commité, ce qui a été rapatrié, ce qui reste dehors,
et **pourquoi**. La seconde et la dernière liste comptent autant que la première :
un inventaire qui ne dit que ses succès laisse croire qu'il n'y a plus rien.*

Branche de consolidation : **`consolidation/tronc-2026-09-15-plus-fusion`**
(partie de `consolidation/tronc-release-2026-09-15`, qui est la lignée du binaire
**servi** au 8432, et qui reçoit la lignée de fusion).

---

## 1. Ce qui n'était commité nulle part, et l'est maintenant

Douze arbres de travail étaient **sales** — c'est-à-dire portaient du travail qui
n'existait que là. Aucun n'est plus sale (`git status --porcelain` vide dans les
35 worktrees) :

| Worktree | Ce qui traînait | Commit |
|---|---|---|
| `banc-obrain` | 2 644 insertions (CLI, moteur, dashboard, tests) + 4 crates neuves + 1 ADR en `??` | 4 commits |
| `banc-forme` | `contient.rs` (l'INTENTION au lieu du texte) + son contrat + son banc | `8274f67e` |
| `banc-index` | index d'EXISTENCE de propriété + la garde « jamais pire qu'un balayage » | `4f353fba` |
| `banc-obrain-heap` | message de `repair` : pourquoi `--apply` épargne les ensevelies | `ce76e9d0` |
| `banc-obrain-j4` | quatre sondes J4 (instruments de mesure) | `a8df6fd7` |
| `banc-obrain-j5` | confinement des chemins statiques + 2 comptes rendus | `0d67c097`, `9a3f954b` |
| `banc-obrain-provenance` | politique d'exclusion d'indexation, **avec sa raison par exclusion** | `78b2e542` |
| `banc-obrain-tronc` | sonde d'auto-connaissance de la flotte | `20fc3d38` |
| `banc-repli` | trigrammes : ne plus payer le rejeu d'un fichier MUET | `5dd22fd7` |
| `banc-surface` | ventilation d'un appel HTTP + date du store en mémoire | `30114840` |
| `tpc-j6` | le binaire de TESTS de la CLI ne compilait plus (donc zéro test) | `80a225c6` |
| `union-nav` | le chat ne s'ouvre plus par-dessus le canevas au chargement | `12dfc0d2` |

Plus une hygiène transverse : les **stores égarés** (dix-huit chemins non suivis,
dont `kv-states/` à 153 Mo) et le **`dist` lié** des worktrees servis sont
désormais ignorés (`7954c0b5`) — un lien n'est pas un dossier, donc `dist/` ne le
couvrait pas et quatre worktrees apparaissaient sales en permanence.

---

## 2. Ce qui a été rapatrié

Parti de 34 branches non fusionnées ; **il n'en reste que 2** — un instantané de
sauvetage et un filet technique (voir §3). Fusions réalisées, dans cet ordre :

`consolidation/tronc-release-2026-09-15` (129 commits : résidence, selfs,
`harness_serve`, chaîne IAM, ramifications, repli petit écran du chat) ·
`perf/heap-ultra` · `perf/repli-balayage` · `perf/surface-http` ·
`perf/plan-froid` · `perf/forme-naturelle` · `perf/index-toutes-formes` ·
`fusion/tronc-plus-j6-mes-correctifs` · `feat/provenance-aretes` ·
`feat/slots512-wiring` · `docs/principal-natif` · `docs/principal-natif-v2` ·
`consolidation/2026-09-13` · `fix/r1-dedup-aretes` · `feat/principal-natif-j5` ·
`union/nav-et-rendu-20260915` · `feat/orbites` · `feat/principal-natif-j4`
(voir §2 bis), plus cinq branches **fermées sur preuve** (§3).

### Ce que la fusion a RÉVÉLÉ, et qui n'était pas le but

**Deux régressions silencieuses dans la lignée servie.** Elles ont été trouvées
par `git merge-base --is-ancestor` et par `cargo check`, pas par relecture :

1. **La clé du cache de requêtes avait perdu la GÉNÉRATION du moteur.**
   `c768b0ec` (« le cache cesse de servir des mesures périmées », un ANCÊTRE de
   la lignée servie) l'avait ajoutée ; la lignée est revenue à
   `(store, langage, texte)`. Un binaire réinstallé ou un réglage changé ne
   changeaient donc ni la clé ni la réponse — on relisait un corps produit par un
   AUTRE moteur. C'est la faute qui avait coûté « trois mesures fausses en une
   soirée » (cf. `docs/ECART-DEPLOIEMENT-LIGNES-2026-09-14.md`).
2. **`sans_mesure` / `from_cache` avaient disparu** (0 occurrence mesurée, contre
   2 dans l'ancêtre). Le serveur réannonçait donc une mesure fraîche pour un corps
   resservi par le cache — le MÊME flottant trois fois de suite, signature d'une
   valeur stockée.

Restaurées toutes les deux (`ff3f463a`). **Ce qui vaut d'être retenu au-delà de
ces deux cas : une lignée qui « avance » de 129 commits peut aussi perdre un
correctif que sa propre histoire contenait.** Une branche n'est pas un progrès
monotone, et rien dans `git log` ne le signale.

### Résolutions de conflits, et leur nature

| Fichier | Décision | Pourquoi |
|---|---|---|
| `viz_serve.rs` | lignée servie, **plus** les 8 hunks de pagination de niveau | la pagination (`offset`, `tronque`) n'existait que côté fusion, et `densite.ts` en dépend |
| `worlds.rs` | lignée servie | sur-ensemble (`fusionner_du_disque` + `REGISTRE_MTIME`) |
| `outils.rs`, `admin.rs` | lignée servie | même correctif des deux côtés |
| `ChatPleinEcran.tsx` | lignée servie | sur-ensemble (repli des réglages sous 400 px) |
| `api.ts` | fusion **plus** `Ramifications` de la lignée servie | garder l'un OU l'autre cassait `SubjectLine.tsx` ou `fetchSpacePage` |
| `store.rs` (`index`) | **les deux** | l'index d'existence et l'index ordonné par valeur ne s'opposent pas |
| `repair.rs`, `main.rs`, `store.rs` (`r1`) | **les deux** | ajouts additifs : `RepairProps` d'un côté, reconstruction R1 de l'autre |

Après résolution : `cargo check --workspace` **0 erreur**, `npx tsc -b` **0
erreur**, `densite.ts` importé par les trois vues.

### Un incident de méthode, consigné

Un `git commit --amend` a visé le mauvais commit : le message de la fusion
`tpc-j6` a temporairement porté celui du commit d'index. Corrigé par
`git filter-branch --msg-filter` (arbre **identique** vérifié avant/après), et la
branche `sauvegarde/avant-correction-message` garde la version d'avant. Rien
n'est perdu ; c'est écrit ici parce qu'un message faux est un faux témoignage,
même réparé.

---

## 2 bis. Trois fusions de plus, et une leçon sur l'index

`union/nav-et-rendu-20260915` · `feat/orbites` · `feat/principal-natif-j4`.

**`union/nav-et-rendu`** : neuf conflits dans `CosmosWeave.tsx`, et aucun n'était
une divergence — dans les neuf, la lignée servie est le **descendant** du travail
de la branche. Vérifié identifiant par identifiant (`revenirEntreeRef`,
`suivantsDuNiveau`, `entreeRacine`, `allerClassesRef`, `entrerPlusGrosRef`,
`groupesSource`, `it.feuille` : tous présents). Ce qui manquait vraiment, c'est
que **le chat s'ouvrait par-dessus le canevas** : `.chat-plein` en z-index 60
recevait tous les clics (`elementFromPoint(720,420)` → `DIV.chat-corps`), la
molette ne descendait plus, « ⤓ descendre » était recouvert.

Piège évité de justesse : entre-temps l'état d'ouverture avait quitté le
composant pour un atome (`chatOuvertAtom`). Reprendre la branche telle quelle
recréait un `useState` local — **deux `chatOuvert` dans un même composant** — et
la ligne fautive serait revenue. Le correctif est porté sur l'atome.

**`feat/orbites`** : `orbites.rs` (26 Ko) et `/api/viz/orbites` entrent ; la
constante `KNOWN_ROOTS` est **restaurée** au-dessus, parce que la mémoïsation de
l'agrégat de rang du tronc en dépend. Les deux mécanismes cohabitent — décision
explicite, écrite dans le code à l'endroit de la constante.

**L'incident, et il faut le dire** : le commit de fusion de `feat/orbites` a été
enregistré avec l'état **cassé**. Les deux correctifs avaient été écrits dans
l'arbre de travail *après* le `git add` : l'index, donc le commit, portaient
encore le doublon de route et la constante manquante. Le `cargo check` qui suivait
mesurait l'ARBRE, pas l'index — il disait vrai sur le fichier et faux sur ce qui
était commité. Réparé par `2b983001`, avec la leçon écrite : *après un `git add`,
une correction postérieure doit être re-stagée*.

## 3. Ce qui reste DEHORS, et pourquoi

Il ne reste que **deux branches**, et aucune n'est du travail perdu :

| Branche | Ce que c'est | Pourquoi elle reste dehors |
|---|---|---|
| `sauvegarde/worktree-principal-2026-09-13` | Un **instantané de sauvetage** (226 fichiers) du chantier non commité du 13/09, pris quand il a été décidé qu'« aucun travail ne doit vivre seulement dans un arbre sale ». | C'est une photographie, pas une lignée. Mesuré : ses 126 fichiers absents du tronc sont **des stores** (`Runtime/substrate.*`, `engine_idx/substrate.*`) et un `CosmosWeave.tsx.bak` de 4 477 lignes — c'est-à-dire exactement ce qui ne doit PAS entrer (cf. l'hygiène `7954c0b5`). Le reste de son code est une copie plus ancienne. **Archive : ne jamais fusionner.** |
| `sauvegarde/avant-correction-message` | Filet technique : l'état d'avant la correction d'un message par `filter-branch` (arbre vérifié identique). | À supprimer quand la consolidation est validée. |

### Les quatre branches FERMÉES, et la preuve qui l'autorise

Elles ne sont pas fusionnées : elles sont **déclarées absorbées**, sur preuve
mesurée, et fermées par `git merge -s ours` (ce qui les fait apparaître comme
fusionnées sans rien changer à l'arbre).

| Branche | Preuve |
|---|---|
| `fusion/j4-final` | `git cherry HEAD <branche>` : **0 commit non équivalent** — ses 2 commits sont déjà dans le tronc par `patch-id`. |
| `fusion/j4-consolide` | idem : 0 non équivalent. |
| `feat/principal-natif-j4` | 2 commits équivalents ; le troisième (`a8df6fd7`) est **la sonde J4 commitée dans cette session** → fusionnée pour de vrai, pas fermée. |
| `feat/recherche-couplee` | La recherche couplée **est** dans le tronc (`fuse_results` 3×, `hybrid_search` 2×, RRF 4×, feature `hybrid-search` câblée dans `obrain-cli/Cargo.toml` avec le commentaire « chantier RECHERCHE-COUPLEE »). Ses 3 fichiers propres sont des **vues mortes retirées exprès** par `328233bd` (« retrait des vues mortes »). Et son commit `17af9a63` est un dépôt WIP qui se déclare lui-même : « fichiers WIP wami (copie de build, **ne pas reprendre tel quel**) ». |
| `feat/principal-natif-j5` | Garde de confinement et nano vérifiés présents ; les 2 documents manquants rapatriés par `b5b4e508`. |

---

## 4. Ce qui n'a PAS été vérifié

Le dire est plus utile que de le taire :

* les **portes d'acceptation** (`scripts/acceptation-*.sh`, douze scripts) n'ont
  pas été relancées : elles demandent un service armé, et le service tourne ;
* la **mesure dans un navigateur** du dashboard consolidé n'a pas été faite — or
  c'est elle qui dirait si le portage du chat sur l'atome tient, et si la
  navigation fractale se comporte comme dans la lignée d'origine ;
* le **déploiement** du binaire consolidé au 8432 n'a pas été fait : le service
  en cours est celui de `consolidation/tronc-release-2026-09-15`, et le tronc
  consolidé n'a **pas** été construit en `--release`.

### Ce qui EST vérifié, et sur quoi

| Contrôle | Résultat |
|---|---|
| `cargo check --workspace` | 0 erreur |
| `cargo check --workspace --all-targets` | 0 erreur (tests, exemples et bancs compilent — ce qui compte, puisque la fusion a fait entrer des tests et des sondes) |
| `npx tsc -b` (dashboard) | 0 erreur |
| `node --test densite.test.mjs segments.test.mjs` | 2/2 |
| marqueurs de conflit dans l'arbre | aucun (`git grep '^<<<<<<<'`) |
| `git status --porcelain` | vide, dans les 35 arbres de travail |

Le contrôle des **cibles de test** n'est pas décoratif : c'est `tsc` qui a attrapé
le doublon de hook `montre` produit par la fusion `union/nav-et-rendu`, et
`cargo check` qui a attrapé la constante manquante et la route dédoublée de
`feat/orbites`. Les deux défauts étaient invisibles à la lecture des conflits.

---

## 4 bis. La campagne de portes, sur le binaire consolidé

**Le build.** `cargo build --release -p obrain-cli` (features par défaut : `gql`,
`cypher`, `indexer`, `ai`, `iam`, `algos`, `parallel` — c'est la configuration de
production). Terminé en 1 min 42 s, binaire de 161 600 296 octets, et il porte
bien la sous-commande `viz monde` rapatriée dans cette session.

**La campagne.** Les dix portes lancées par `scripts/acceptation-toutes.sh`
(chacune crée ses stores dans `/tmp`, prend son port, lance son propre
`viz serve` — **aucune ne touche le service de production**) :

| Porte | Verdict | Vérifications |
|---|---|---|
| `acceptation-coffre.sh` | PASS | 25 |
| `acceptation-journal.sh` | PASS | 14 |
| `acceptation-tls.sh` | PASS | 16 |
| `acceptation-iam.sh` | PASS | 24 |
| `acceptation-selfs-catalogue.sh` | PASS | 17 |
| `acceptation-self-par-compte.sh` | PASS | 12 |
| `acceptation-chat-self.sh` | PASS | 10 |
| `acceptation-resonance.sh` | PASS | 22 |
| `acceptation-monde-utilisateur.sh` | PASS | 16 |
| `acceptation-8432.sh` | **ÉCHEC sur UN contrôle** | 3/4 |

`acceptation-8432.sh` passe ses quatre premiers contrôles — les deux builds
(binaire **et** `tsc -b` + `vite build` du dashboard), le protocole (dont la
traversée de chemin refusée à **7 profondeurs sur 7**), et le balayage des
endpoints — et échoue sur le dernier, `fratrie-check.mjs`.

**Ce que la campagne a corrigé dans les portes elles-mêmes**

1. **`scripts/acceptation-coffre.sh` était commité sans le bit exécutable** : la
   porte existait, était documentée, et aucun lanceur ne pouvait l'exécuter.
   C'est le lanceur qui l'a dit, en la comptant « ABSENTE ».
2. **`endpoints-check.mjs` comptait un 401 comme une panne.** `/api/worlds` et
   `/api/worlds/status` exigent une identité **par conception** (`45dd9212`) ;
   interrogées sans identité, elles répondent 401 même IAM désarmé. Le contrôle
   présente désormais l'identité si l'environnement en fournit une, et sinon il
   les compte à part : **23/23 mesurés OK · 2 NON mesurés (identité requise)**.
3. **`fratrie-check.mjs` mourait sans dire pourquoi** (`HTTP 401` + pile
   d'appels). Il dit maintenant l'état de son identité.

**Le seul échec qui reste, et il n'accuse pas le produit.** `fratrie-check.mjs`
ne trouve pas de cible exploitable sur l'instance isolée (les mondes qu'il essaie
sont absents ou n'ouvrent pas la face Graphe), et sur le service **armé** sa
connexion est refusée : `iam.obrain.identifiants.txt` est daté du **13/09 18:37**
alors que le magasin d'identités a été **reconstruit le 14/09 12:28**
(`iam.obrain.bak-avant-v3-20260914`). Le fichier est antérieur au magasin qu'il
prétend ouvrir. La porte se rendra mesurable par `OBRAIN_IAM_TOKEN=<jeton>`, ou
par `FRATRIE_STORE=… FRATRIE_CENTRE=…` — deux issues que le script écrit
lui-même.

**Ce qui n'a PAS été fait :** le service de production n'a **pas** été redémarré.
Le 8432 tourne toujours sur le binaire de la lignée d'avant la consolidation.
Déployer est un geste du détenteur, et il se fait en deux temps —
`systemctl --user restart obrain-viz.service`, puis les contrôles à vif écrits
par la porte elle-même.

1. **Construire et déployer le tronc consolidé en `--release`**, puis relancer les
   douze portes d'acceptation : sans ça, la consolidation n'est qu'une
   compilation. C'est la seule étape qui reste entre l'état actuel et un service
   qui porte tout le travail.
2. **Mesurer dans un navigateur** le chat (porté sur l'atome) et la navigation
   fractale : ce sont les deux endroits où une fusion a changé un comportement,
   pas seulement un fichier.
3. **Trancher le choix de rang** : `/api/viz/orbites` dérive les échelles, la
   mémoïsation du tronc reste sur `KNOWN_ROOTS`. Les deux cohabitent parce que les
   fusionner à l'aveugle aurait perdu la correction des 1,2 s ; la question
   « lequel garde-t-on » reste ouverte, et elle se mesure.
4. **Décider du sort de `sauvegarde/worktree-principal-2026-09-13`** : archive
   permanente, ou suppression, sachant que son contenu propre est fait de stores.
