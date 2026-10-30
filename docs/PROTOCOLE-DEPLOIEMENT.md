# Protocole de déploiement — obrain + harness sur n3

Autorité : ce document fait foi pour le **déploiement**.
Pour la définition des **environnements** (DSH_HOME, runtime dirs, cohortes),
l'autorité reste `harness-worktrees/graph-writer/docs/protocol-multi-env.md`.
Les deux se citent, ne se recopient pas.

Écrit le 2026-09-01 après une journée où trois déploiements successifs ont
défait le travail des précédents. Chaque règle ci-dessous répond à un fait
constaté ce jour-là, cité en regard.

---

## Les sept règles

**R1 — Un seul chemin.**
`dsh-deploy.sh` pour le harness, le module OpenTofu pour la configuration
d'obrain. Tout autre script qui déploie est supprimé.
*Constat : le `deploy.log` du 2026-09-01 09:13 a été écrit par un script
introuvable dans tout `~/projects`, sans smoke, sans healthcheck, décidant
seul de « skip restart ».*

**R2 — Rien ne se déploie qui ne soit commité et tagué.**
Le build refuse un arbre sale. Le binaire installé porte son commit.
*Constat : le binaire de prod du matin venait d'un arbre avec trois fichiers
modifiés non commités. Il était irreconstructible — et il contenait le panic
qui a rendu tout l'historique injoignable.*

**R3 — Une configuration, une source.**
Le drop-in systemd est **rendu** depuis le module tofu. On ne l'édite jamais
à la main. Une modification manuelle est une dérive, pas un correctif.
*Constat : le drop-in chargé n'était pas celui de tofu. Un `tofu apply`
aurait retiré `DSH_GRAPH_WRITER`, `OBRAIN_IDLE_MS` et `OBRAIN_DEBUG_LOG`, et
redémarré la prod sans prévenir.*

**R4 — On refuse avant, on ne rattrape pas après.**
Une porte qui échoue arrête le déploiement. Le rollback est le filet, pas le
plan.
*Constat : le « rollback » du module tofu est un `echo` suivi d'un `exit 1`.
Il ne restaure rien.*

**R5 — Les portes testent la fonction, pas la vie.**
`HTTP 200` ne prouve rien : le mode de défaillance le plus fréquent de cette
pile est un service vivant qui répond 200 avec un arbre de plugins amputé ou
un graphe qui renvoie 0.
*Constat : le marqueur `dsh-client-ui-` appartient au bundle client statique.
Il est servi à l'identique quand le serveur est amputé.*

**R6 — Aucun outil ne redémarre le service dont il fait partie.**
Un processus lit `/proc/self/cgroup` et refuse, ou se ré-exécute en unité
transitoire (`systemd-run --user --scope`).
*Constat : `dsh-deploy.sh` a dû adopter ce motif après s'être tué lui-même.
Le module tofu, lui, redémarre encore `dsh-web` depuis un `local-exec`.*

**R7 — Le dev d'abord, toujours. La prod ne redémarre jamais pour une action
de dev.**
*Invariant n°1 de `protocol-multi-env.md`, seul point de la chaîne qui n'a
jamais été pris en défaut.*

---

## Le déroulé

### G0 — Préflight (bloquant)

```bash
git -C ~/projects/banc-obrain status --porcelain   # doit être vide      (R2)
git -C ~/projects/banc-obrain describe --tags      # doit nommer un tag  (R2)
df -h ~ | tail -1                                  # > 15 Go libres
curl -s -o /dev/null -w '%{http_code}' localhost:3082/   # prod 200 AVANT (R7)
```
Un seul échec arrête tout. On ne déploie pas sur une prod déjà malade : on la
soigne d'abord, sinon on ne saura pas ce qu'on a cassé.

### G1 — Build tracé

```bash
cd ~/projects/banc-obrain
choom -n 300 -- nice -n 19 env CARGO_BUILD_JOBS=6 cargo build --release
```
`nice 19` + 6 jobs sur 16 threads : le compilateur ne dispute jamais le CPU au
llama-server de 15 Go. Deux gels machine ont établi ce réglage.

Puis on écrit le manifeste — sha256, commit, `describe`, versions du harness,
état des stores. Sans lui, on ne sait pas d'où l'on vient, donc on ne sait pas
où revenir. C'est la mécanique anti-retour-en-arrière.

### G2 — Dev, avec préchauffage

```bash
printf '%s\n' '{"tool":"sessions_list","args":{}}' \
  | obrain mcp --store <store-dev> > /dev/null    # store CHAUD
systemctl --user restart dsh-dev-3083
```
Le préchauffage n'est pas un confort. Un store froid fait dépasser le timeout
de 5 s du bridge, l'entrée de loader `dsh-workspace` échoue, et **l'arbre de
plugins s'effondre en silence**. C'est le crash du 2026-09-01 00:04.

### G3 — Les portes fonctionnelles (R5)

Les quatre, dans cet ordre. Aucune n'est facultative.

| porte | ce qu'elle prouve | comment |
|---|---|---|
| **écoute** | `boot()` a rendu la main, les plugins sont montés | la ligne `dsh web: http://` apparaît dans le journal APRÈS l'horodatage du démarrage |
| **contenu** | le graphe répond, les degrés ne sont pas périmés | `sessions_list` renvoie ≥ N sessions (seuil = dernier compte connu − 10 %) |
| **historique** | le journal se lit, pas de panic | `session_load` `events_only` sur une session connue renvoie ≥ 1 event |
| **cohérence** | toute session listée est ouvrable | croiser les ids du graphe avec les archives de `$DSH_HOME/sessions` — 0 orpheline |
| **stabilité** | pas de crash-loop | `NRestarts` inchangé après **60 s** |

La dernière est celle qu'on oublie. Une sonde ponctuelle réussit dès la
première tentative et déclare le déploiement bon — alors que le service bat
toutes les 5 s. `is-active` répond `active` en plein crash-loop, et
`StartLimitBurst=5` sur une fenêtre de 10 s avec `RestartSec=5` ne se
déclenche **jamais** : au mieux 2 démarrages par fenêtre contre un seuil de 5.
Le 2026-09-01, le compteur est monté à 9 sans que systemd renonce.

### G3bis — Réconciliation (à chaque démarrage, pas une réparation ponctuelle)

Avant d'exposer l'UI, l'environnement rescanne ses propres archives et
réaligne le graphe : `source_path` du `DSH_HOME` courant, `origin` et
`parentSession` relus dans l'en-tête. Quelques millisecondes par projet.

Sans cette étape, la porte « cohérence » échouera au déploiement suivant —
c'est délibéré. L'invariant n'est pas garanti par une réparation qu'on se
souvient de lancer, il l'est par une étape qui s'exécute toujours.

*Constat du 2026-09-01 : 77 sessions listées en prod, 13 ouvrables. Les 48
archives de subagent sont nommées `<uuid>` nu sur le disque tandis que
`import_events` force le préfixe `session-` : l'identifiant du graphe ne peut
structurellement jamais retrouver l'archive. Le dev était dans le même état —
67 orphelines sur 77 — sans que personne le voie, faute de sonde.*

### G4 — Prod

Mêmes gestes, dans une fenêtre choisie, jamais pendant qu'une conversation
travaille. Sauvegarde du binaire par `mv` horodaté avant copie — jamais `cp`
sur un binaire en cours d'exécution (`Text file busy`). Échec d'une porte →
restauration automatique, puis on rejoue les portes sur l'état restauré.

### G5 — Scellement

Tag annoté, manifeste écrit, **et le dépôt poussé hors de la machine**.
*Constat : `banc-obrain` n'avait aucun remote. L'unique exemplaire du moteur
qui fait tourner la prod vivait sur un seul disque.*

---

## Ce qui empêche de revenir en arrière

Cinq mécanismes, tous absents avant le 2026-09-01 :

1. **Un manifeste par version** — on sait toujours ce qui tourne et vers quoi
   revenir. Le moteur C l'avait (`MANIFEST.txt` + cohorte), obrain non.
2. **Le tag obligatoire** — un binaire non tagué ne s'installe pas.
3. **La configuration rendue depuis une source unique** — une édition
   manuelle devient visible comme une dérive, au lieu de disparaître au
   prochain apply.
4. **Un test de non-régression par incident** — chaque panne corrigée laisse
   un test qui la détecterait. Le panic du cadrage de journal en a un.
5. **La sonde de dérive des degrés** — comparer les mtimes de
   `substrate.degrees.*` avec `substrate.edges`. Ne demande aucun verrou,
   coûte une milliseconde, et détecte le seul mode de défaillance silencieuse
   du store : les expansions typées qui renvoient 0 pendant que le service
   répond 200.

---

## Les interdits

- `--no-smoke` en production. Les deux unités qui l'ont utilisé le
  2026-09-01 sont aujourd'hui en échec.
- `cp` sur un binaire en cours d'exécution.
- Éditer un drop-in à la main.
- Déployer depuis le cgroup de la cible.
- Activer un canal d'écriture exclusif sans avoir vérifié que l'outillage de
  lecture y survit. `OBRAIN_WRITE=1` prend un verrou exclusif : sauvegarde,
  `monitor-obrain.sh`, `repair-degrees` et toute restauration perdent alors
  leur accès de façon non déterministe.
