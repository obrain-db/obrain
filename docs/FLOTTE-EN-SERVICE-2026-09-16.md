# LA FLOTTE EN SERVICE — la passe tourne toute seule (2026-09-16)

> `obrain machines --appliquer` fait de la flotte un fait du graphe (un nœud
> `machine:<hôte>` par machine connue, avec son compte, ses capacités annoncées,
> sa première attache et sa dernière présence — `crates/obrain-cli/src/machines.rs`).
> Jusqu'ici, **rien ne la déclenchait en service** : elle n'avait été prouvée que
> par `scripts/acceptation-flotte.sh`, qui la lance elle-même. Une fonctionnalité
> qu'aucun service n'appelle n'existe pas pour l'utilisateur.

Ce document dit ce qui est livré, ce qu'il faut installer, ce qui se passe quand
la base visée est tenue en lecture — et ce qui se perd si l'on ne fait rien.

---

## 1. L'état mesuré (2026-09-16, 18:36 → 18:50 UTC)

| Fait | Mesure |
|---|---|
| `viz serve` (8432) | actif, PID 1530420, démarré 17:47:37 |
| `OBRAIN_VIZ_CACHE_DIR` du viz | **absente** de `/proc/1530420/environ` (seuls `OBRAIN_IAM`, `OBRAIN_IAM_ACCOUNT`, `OBRAIN_IAM_STORE`, `OBRAIN_LLM_URL`, `OBRAIN_WORLD*`, `OBRAIN_UI_DIR`, `OBRAIN_INDEX_CACHE_DIR`) |
| Registre des machines | `/tmp/obrain-viz-cache/machines` **n'existe pas** — aucune machine ne s'est encore rattachée |
| `/tmp` | **pas un tmpfs** (aucun montage distinct : il est sur la racine) — un reboot ne l'efface donc pas |
| Ce qui l'efface vraiment | `D /tmp 1777 root root 30d` (`/usr/lib/tmpfiles.d/tmp.conf`), armé par `systemd-tmpfiles-clean.timer` (actif, prochain déclenchement 19:09) |
| Déclencheur en service | **aucun** : `systemctl --user list-timers --all \| grep machines` ne rend rien, et il n'y a aucune unité `machines` dans `~/.config/systemd/user/` |
| Base servie | `/mnt/obrain/harness-data/runtime-dev.obrain` (alias `~/.local/share/obrain-harness/data/runtime-dev.obrain`), tenue par **6 verrous partagés** (voir §5) |

---

## 2. Ce qui est livré

| Fichier | Rôle |
|---|---|
| `scripts/machines-appliquer.sh` | la passe, avec son verdict : dit ce qu'elle a regardé, refuse en motivant, ne se tait jamais quand il y avait des machines |
| `deploy/systemd/obrain-machines.service` | l'unité qui l'exécute (oneshot, `Nice=10`, aucun redémarrage de service) |
| `deploy/systemd/obrain-machines.timer` | le déclencheur : `OnBootSec=3min`, `OnUnitActiveSec=2min`, `Persistent=true` |
| `deploy/systemd/obrain-viz.service.d-cache-persistant.conf` | le drop-in qui sort le registre de `/tmp` (à copier par l'exploitant) |
| `docs/FLOTTE-EN-SERVICE-2026-09-16.md` | ce document |

**Rien n'est activé.** Une unité livrée dans le dépôt n'est pas une unité en
service : l'activation est un geste d'exploitant (R7).

### Deux déclencheurs coexistent — et c'est voulu

Le dépôt porte déjà `deploy/systemd/obrain-machines-appliquer.{service,timer}` +
`scripts/machines-appliquer-comptes.sh` (commit `e2a30f07`), qui écrivent la
flotte de **chaque compte** dans **sa** base (`selfs/<compte>.obrain`), toutes
les 5 minutes. Les deux passes ne visent pas la même base, pour une raison
mesurable :

| | cible | cadence | quand l'utiliser |
|---|---|---|---|
| `obrain-machines.timer` (ce document) | **une** base : celle que le harness sert (`runtime-dev.obrain` par défaut) | 2 min | quand on veut la flotte dans LE graphe qu'on interroge, sur une base que nul service ne tient |
| `obrain-machines-appliquer.timer` (dépôt) | la base de **chaque** compte (`selfs/<compte>.obrain`) | 5 min | quand on veut que chaque compte apprenne ses machines sans disputer aucun verrou |

La cadence diffère parce que le coût diffère : la passe par compte ouvre en
exclusif la base de chaque compte (autant de verrous que de comptes), là où la
passe d'une base n'en prend qu'un. **On ne les active pas toutes les deux sur la
même cible.**

---

## 3. Installation (geste d'exploitant)

```bash
cd /home/triviere/projects/banc-obrain-tronc

# 1. le déclencheur, dans le foyer systemd de l'utilisateur
cp deploy/systemd/obrain-machines.service ~/.config/systemd/user/
cp deploy/systemd/obrain-machines.timer   ~/.config/systemd/user/

# 2. (recommandé) le registre hors de /tmp — voir §4
mkdir -p ~/.config/systemd/user/obrain-viz.service.d
cp deploy/systemd/obrain-viz.service.d-cache-persistant.conf \
   ~/.config/systemd/user/obrain-viz.service.d/cache-persistant.conf

# 3. recharger systemd, puis ARMER le timer (c'est le seul geste qui « met en service »)
systemctl --user daemon-reload
systemctl --user enable --now obrain-machines.timer
```

Le drop-in ne prend effet qu'au **prochain démarrage du viz** — et ce
redémarrage n'appartient qu'à l'exploitant (R6/R7) : aucun script de ce dépôt ne
l'arrête ni ne le redémarre. Le lien entre les deux : si vous déplacez le
registre, **posez la même variable pour les passes** (§4.3), sinon elles
regardent `/tmp` pendant que l'écrivain écrit ailleurs.

Contrôles utiles :

```bash
systemd-analyze --user verify ~/.config/systemd/user/obrain-machines.service   # exit 0
systemd-analyze --user verify ~/.config/systemd/user/obrain-machines.timer     # exit 0
systemctl --user list-timers obrain-machines.timer
journalctl --user -u obrain-machines.service -n 20
```

---

## 4. Le registre dans `/tmp` : ce qui se perd, et comment le sortir de là

### 4.1 Ce qui se perd, et par quel mécanisme exact

Le registre des machines est un **journal append-only** écrit par `viz serve`
(`remote.rs` : `…/machines/<hachage du compte>.jsonl`) — pas un cache
recalculable : il porte ce qui a été annoncé, par quel compte, avec quelles
capacités, et quand. Le défaut d'`OBRAIN_VIZ_CACHE_DIR` est
`/tmp/obrain-viz-cache`.

Correction d'une croyance répandue, mesurée : `/tmp` **n'est pas un tmpfs** sur
cette machine (aucun montage distinct), donc **un reboot ne suffit pas** à
effacer le registre. Ce qui l'efface :

* le nettoyage quotidien de systemd, `D /tmp 1777 root root 30d`
  (`/usr/lib/tmpfiles.d/tmp.conf`) : une machine qui s'est rattachée une fois et
  n'a plus rien écrit depuis **30 jours** perd son journal sans un mot, et la
  passe suivante conclut « 0 machine connue » ;
* un `rm -rf` humain quand `/tmp` dérange — et là, c'est immédiat.

Ce qui **survit** à la perte du registre : les nœuds `machine:` déjà écrits dans
les bases (la passe n'efface jamais un fait passé). Ce qui est perdu : la flotte
**connaissable** — la prochaine passe n'a plus rien à appliquer, et le graphe ne
s'enrichit plus des machines qui se rattacheront sans que personne ne le voie.

### 4.2 Le remède : `~/.local/share/obrain-harness/viz-cache`

Le drop-in `deploy/systemd/obrain-viz.service.d-cache-persistant.conf` pose :

```
OBRAIN_VIZ_CACHE_DIR=%h/.local/share/obrain-harness/viz-cache
```

Pourquoi ce chemin : le foyer **durable** de l'utilisateur, à côté de `mondes/`
(que le drop-in `durable.conf` a déjà sorti de `/tmp` pour la même raison) — et
**pas** sous `/mnt/obrain/caches/`, dont le nom invite un futur nettoyage à vider
ce qui doit survivre. Même nom de feuille que l'ancien dossier : l'écart est un
préfixe, pas une nouvelle convention.

### 4.3 Changer ce chemin DÉPLACE le registre existant

```bash
cp -a /tmp/obrain-viz-cache "$HOME/.local/share/obrain-harness/viz-cache"
```

Sans cette copie, la flotte déjà connue n'est plus lue. (Le 2026-09-16 18:45,
`/tmp/obrain-viz-cache/machines` n'existe pas : la copie est sans effet
aujourd'hui — elle reste la règle, parce que rien ne le dira le jour où le
dossier existera.)

**Deux lectures suivent ce dossier depuis leur propre service**, avec leur propre
environnement : elles ne suivent pas le viz toutes seules.

* `obrain-machines.service` → poser la variable dans
  `~/.config/obrain/machines.env` (son `EnvironmentFile`) :
  `OBRAIN_VIZ_CACHE_DIR=/home/triviere/.local/share/obrain-harness/viz-cache`.
  Sans quoi le script **refuse** (code 4) : « registre introuvable là où
  l'écrivain l'écrit » — c'est la garde qui existe pour ça.
* `obrain-self-appliquer.service` (hors dépôt) → y ajouter la même
  `Environment=`, sinon `memoire-appliquer-comptes.sh` regarde `/tmp`, n'y trouve
  plus de journal et annonce « 0 compte » toutes les 2 minutes : la mémoire des
  comptes cesse d'être appliquée sans que rien ne soit cassé. (Ce script DIT le
  chemin qu'il regarde — c'est ce qui permet de le voir dans le journal.)

---

## 5. Ce qui se passe si la base est tenue en lecture

C'est le cas **normal** de la base servie. Mesuré le 2026-09-16 à 18:47 :

```
$ M=/mnt/obrain/harness-data/runtime-dev.obrain/substrate.meta
$ stat -c %i "$M"
59769730
$ grep 59769730 /proc/locks
49: FLOCK  ADVISORY  READ 1543449 fc:00:59769730 0 EOF
51: FLOCK  ADVISORY  READ 1529140 fc:00:59769730 0 EOF   # harness-serve (8090)
74: FLOCK  ADVISORY  READ 1540963 fc:00:59769730 0 EOF
132: FLOCK ADVISORY  READ 1530145 fc:00:59769730 0 EOF
266: FLOCK ADVISORY  READ 1529142 fc:00:59769730 0 EOF   # this-rs-gateway (8144)
297: FLOCK ADVISORY  READ 1530420 fc:00:59769730 0 EOF   # obrain viz serve (8432)
```

Six verrous **partagés** : l'ouverture exclusive de la passe est refusée. Le
verrou est un `try_lock` **non bloquant** (`obrain-substrate/src/file.rs`,
`fs2::FileExt::try_lock_exclusive`) — la passe ne patiente pas, elle refuse
immédiatement. Le script le dit, avec ses remèdes, et sort en **1** :

```
machines : moteur — OBRAIN-X001: Internal error: substrate: failed to open
  existing store at …: I/O error: substrate … is already open by another process
  (exclusive lock on …/substrate.meta refused: Resource temporarily unavailable
  (os error 11)). Close the other holder first — note: read-only opens use a
  shared lock and can coexist.
machines : /tmp/…/store.obrain — la passe a ÉCHOUÉ (code 1) ; le registre (3 machine(s)) est intact
           LA BASE EST TENUE EN LECTURE par un service (dashboard/harness).
           Ce n'est PAS une panne : c'est l'invariant mono-écrivain.
           Remèdes, dans l'ordre du moins intrusif :
             1. viser une base que nul service ne tient :
                scripts/machines-appliquer-comptes.sh  (selfs/<compte>.obrain)
             2. viser la base d'un compte précis :
                scripts/machines-appliquer.sh --compte <compte> --store …/selfs/<compte>.obrain
             3. arrêter VOUS-MÊME le service le temps d'une passe (R6 : ce script ne le fait jamais).
```

**Conséquence pratique, à savoir avant d'armer le timer** : viser la base servie
revient à écrire un refus toutes les 2 minutes dans le journal tant que le
dashboard tourne. C'est un refus DATÉ et MOTIVÉ (et non un succès muet), mais si
l'on veut la flotte dans le graphe sans rien disputer, la cible juste est la base
d'un compte — c'est-à-dire l'autre déclencheur (§2). Un refus qui se répète
n'est acceptable que parce qu'il dit pourquoi ; il ne remplace pas une cible
libre.

### Codes de sortie du script

| code | sens |
|---|---|
| 0 | appliqué — ou rien à appliquer, et il le DIT (registre vide, base non ouverte) |
| 1 | passe refusée : verrou tenu, refus du moteur, 0 nœud écrit alors que le registre portait des machines, ou rapport illisible |
| 2 | binaire introuvable, ou argument inconnu |
| 3 | le binaire ne connaît pas la sous-commande `machines` (déploiement en retard) — avec la commande de reconstruction |
| 4 | registre introuvable là où l'écrivain l'écrit (écrivain et passe ne pointent pas le même dossier) |
| 5 | la base cible n'existe pas — la passe ne la crée pas, alors que `open_substrate` saurait le faire : écrire la flotte dans une base neuve que personne ne sert aurait la forme d'un succès |

---

## 6. Ce qui se perd si l'on ne fait rien

| Si l'on n'installe rien | Conséquence |
|---|---|
| le timer n'est pas armé | la passe ne tourne **toujours pas** en service : la flotte n'entre dans le graphe que si un humain lance `scripts/machines-appliquer.sh` ou la porte d'acceptation |
| le drop-in n'est pas posé | le registre reste dans `/tmp` : effacé par le nettoyage à 30 jours, ou par un `rm -rf` — la passe conclut alors « 0 machine connue » et le graphe cesse de s'enrichir, sans erreur. **Les nœuds déjà écrits survivent** ; c'est la connaissance des machines à venir qui se perd |
| la variable n'est posée que d'un côté | l'écrivain (viz) et la passe ne regardent pas le même dossier : le script REFUSE (code 4) au lieu d'annoncer 0 machine — et si c'est la passe du Self qui est restée sur `/tmp`, la mémoire des comptes cesse d'être appliquée en silence (elle dit « 0 compte », sur le chemin qu'elle regarde) |

---

## 7. Preuves exécutées (2026-09-16, 18:38 → 18:52 UTC)

Toutes sur des stores et des caches **temporaires** (`/tmp/ch6-final`,
`/tmp/ch6-flotte`), jamais sur une base vivante.

| Commande | Sortie observée |
|---|---|
| `bash -n scripts/machines-appliquer.sh` | exit 0 |
| `systemd-analyze --user verify deploy/systemd/obrain-machines.service` | exit 0, aucune sortie |
| `systemd-analyze --user verify deploy/systemd/obrain-machines.timer` | exit 0, aucune sortie |
| `SYSTEMD_UNIT_PATH=/tmp/ch6-flotte/units: systemd-analyze --user verify obrain-viz.service` (unité réelle, sans puis avec le drop-in copié en `obrain-viz.service.d/cache-persistant.conf`) | exit 0 dans les deux cas : le drop-in ne casse rien et `OBRAIN_VIZ_CACHE_DIR` s'y trouve bien |
| `obrain query /tmp/ch6-final/store.obrain --read-only "MATCH (m:Machine) RETURN m.qn"` **avant** la passe | `nœuds machine: = 0` |
| 1ʳᵉ passe (registre : 3 machines, 2 comptes, 2 fichiers de journal) | `3 nœud(s) « machine: » écrit(s) sur 3 connue(s) · registre …/cache/machines (2 fichier(s))`, exit 0 |
| même requête **après** la 1ʳᵉ passe | `nœuds machine: = 3` — `machine:n3`, `machine:n7`, `machine:poste-commun`, chacun avec son `compte` et ses `capacites_annoncees` |
| **2ᵉ passe** sur la même base, puis même requête | `3 nœud(s) … écrit(s)`, exit 0 ; `nœuds machine: = 3` — **idempotence** : même nombre, même ensemble de `qn` (et une 3ᵉ passe ne change rien non plus) |
| journal MIXTE : 3 rattachements + 1 déclaration de moteur (`{"kind":"machine","machine":"n9-sans-selfd",…}`) | le script compte 4 machines vues, la passe en écrit 3 (`connues: 3`) et sort 0 : le genre « déclaration » est LU par le script, et la passe n'écrit que ce que son binaire sait écrire |
| journal de déclarations SEULES, binaire antérieur aux déclarations | « le registre nomme 1 machine(s) (n9-sans-selfd) mais la passe n'en connaît AUCUNE », avec les deux causes possibles (binaire en retard / lignes incomplètes), **exit 1** — au lieu d'un « 0 nœud écrit sur 0 connue » qui aurait la forme d'un repos |
| base tenue par un `flock -s` (simulation exacte du verrou partagé du dashboard) | refus du moteur relayé + « LA BASE EST TENUE EN LECTURE… » + les 3 remèdes, **exit 1** |
| `OBRAIN_VIZ_CACHE_DIR` pointant à côté du registre | « registre INTROUVABLE …, mais un registre existe en … », **exit 4** |
| aucun registre nulle part | « 0 machine connue ; normal tant qu'aucune machine ne s'est rattachée », **exit 0** |
| base cible inexistante | **exit 5**, avec le geste qui crée une base (`obrain init`) |
| binaire sans la sous-commande `machines` | **exit 3**, avec `cargo build --release -p obrain-cli` |
| argument inconnu | **exit 2** ; `--help` rend l'en-tête du script et sort 0 |
| avant/après : `ss -ltnp \| grep 8432`, `systemctl --user is-active obrain-viz`, `systemctl --user show obrain-viz -p MainPID` | **même PID (1530420)**, `active` — aucun service touché, ni arrêté ni redémarré |
| avant/après : `systemctl --user list-timers --all \| grep machines`, `ls ~/.config/systemd/user/ \| grep machines` | rien, avant comme après : **le chantier n'arme rien** |

---

## 8. Écarts connus, et ce qui reste à faire

* **La porte d'acceptation ne teste pas le déclencheur.**
  `scripts/acceptation-flotte.sh` §8 exécute la passe elle-même puis constate —
  elle a raison de le faire (une instance d'acceptation n'a pas de timer), mais
  elle ne vérifie pas qu'un service la déclenche. Ajouter cette exigence est un
  gain net (« toute modification de la porte AJOUTE des exigences »), et cela
  n'appartient pas à ce chantier : la porte n'est pas dans ses fichiers.
* **`deploy/systemd/obrain-machines-appliquer.{service,timer}` et
  `scripts/machines-appliquer-comptes.sh` ne sont pas touchés** (commit
  antérieur `e2a30f07`) : deux déclencheurs coexistent, et c'est le document
  ci-dessus qui dit lequel armer. Si l'on veut un seul nom, c'est une décision
  d'exploitant, pas une édition de ce chantier.
* **Rien n'est armé.** Après cette livraison, l'état en service est inchangé :
  aucun timer `machines`. Tant que `systemctl --user enable --now
  obrain-machines.timer` n'est pas lancé, « la passe tourne toute seule » reste
  une intention.
* **La base servie restera refusée** tant que le dashboard, le harness et la
  passerelle la tiennent en lecture (6 verrous partagés, §5). C'est l'invariant
  mono-écrivain, pas un défaut : la flotte entre dans le graphe par la base
  d'un compte, ou pendant une fenêtre où plus personne ne sert la base.
* **Le journal porte désormais deux genres, et le binaire déployé en connaît
  un.** Un chantier parallèle (en cours dans ce même arbre, non commité) fait
  journaliser au harness sa propre identité (`harness_serve.rs`) sous
  `{"kind":"machine",…}` : une machine sans `selfd` devient nommable dans le
  graphe. `scripts/machines-appliquer.sh` compte donc TOUT nom de machine du
  journal (les deux genres), et refuse de conclure « rien à appliquer » quand la
  passe n'en a reconnu aucune — sinon il serait devenu aveugle au second genre,
  c'est-à-dire précisément à l'apport de ce chantier-là. Le binaire déployé au
  18:41 (sha256 `ad930298…`) est antérieur : il ignore encore les déclarations.
