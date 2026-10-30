# Le binaire `selfd` — état livrable et chaîne d'installation par machine

Relevé du **2026-09-16**, sur n3 (192.168.1.103), dépôt
`/home/triviere/projects/banc-obrain-tronc`, branche
`harness/continuite-dans-contrat-2026-09-15`.

Chaque chiffre de cette note vient d'une commande exécutée le jour du relevé, et
la commande est écrite à côté. Les identifiants **stables** du binaire sont son
`sha256` et son `BuildID` : le commit, lui, a bougé pendant la rédaction
(`69421840` → `1f8b2209`, deux commits d'autres chantiers), et un relevé daté
qui prétendrait le contraire serait faux dès le lendemain.

---

## 1. Ce qui est établi, mesure par mesure

| ce qu'on veut savoir | commande | résultat observé (2026-09-16) |
|---|---|---|
| taille | `stat -c %s target/release/selfd` | **1 226 360** octets |
| empreinte | `sha256sum target/release/selfd` | `fbc4bf58fa00e7faf4bc29d497385b45d605bc813ed900e47a801dc62ae308b1` |
| nature | `file target/release/selfd` | `ELF 64-bit LSB pie executable, x86-64, dynamically linked, interpreter /lib64/ld-linux-x86-64.so.2, for GNU/Linux 3.2.0, BuildID[sha1]=31f31ac8…, not stripped` |
| bibliothèques | `ldd target/release/selfd` | `libgcc_s.so.1`, `libc.so.6`, `/lib64/ld-linux-x86-64.so.2` |
| BuildID | `readelf -n … \| grep 'Build ID'` | `31f31ac8c33c1919951046050525e9d46348b7db` |
| compilateur | `readelf -p .comment …` | `rustc version 1.93.0 (254b59607 2026-01-19)`, `Linker: LLD 21.1.8`, `GCC: (Ubuntu 12.4.0-2ubuntu1~24.04.1) 12.4.0` |
| rustc local | `rustc -vV` | `rustc 1.93.0 (254b59607 2026-01-19)` |
| cibles installées | `rustup target list --installed` | `wasm32-unknown-unknown`, `x86_64-unknown-linux-gnu` — **pas de musl** |
| glibc exigée | `objdump -T … \| grep -o 'GLIBC_[0-9.]*' \| sort -uV \| tail -1` | **`GLIBC_2.39`** |
| glibc exigée par les symboles **forts** | `objdump -T … \| grep -vE '^[0-9a-f]+ +w +' \| grep -o 'GLIBC_[0-9.]*' \| sort -uV \| tail -1` | `GLIBC_2.34` |
| symboles faibles portant 2.39 | `objdump -T … \| awk '$2=="w"' \| grep -F '(GLIBC_2.39)'` | `pidfd_getpid`, `pidfd_spawnp` |
| glibc locale | `ldd --version \| head -1` | `ldd (Ubuntu GLIBC 2.39-0ubuntu8.9) 2.39` |

### 1.1 Le point qui décide de tout : `GLIBC_2.39`, et pourquoi

Les deux symboles qui exigent `GLIBC_2.39` sont **faibles** (`w` dans
`objdump -T`) : ce sont des références optionnelles de Rust std (`pidfd_spawnp`,
`pidfd_getpid`). On pourrait en conclure qu'elles ne contraignent rien. **C'est
faux, et la mesure le montre** : l'éditeur de liens inscrit quand même ces
versions dans l'en-tête `VERNEED` de l'objet, et `ld.so` contrôle ces exigences
**avant** toute résolution de symbole, sans distinguer le faible du fort. Le
binaire entier est donc refusé, pas la seule fonction concernée.

Mesuré le 2026-09-16, conteneurs locaux, `--network none`, binaire monté en
lecture seule (`docker run --rm --network none -v …:/n/selfd:ro <image> /n/selfd --call status`) :

| machine (image) | glibc | résultat observé | code |
|---|---|---|---|
| Ubuntu 24.04 | 2.39 | démarre, rend le statut du nœud | **0** |
| Debian 12 | 2.36 | `version 'GLIBC_2.39' not found (required by /n/selfd)` | **1** |
| Ubuntu 22.04 | 2.35 | idem (sortie vide, refus du chargeur) | **1** |
| Debian 11 | 2.31 | `GLIBC_2.32`, `2.33`, `2.34`, `2.39` manquantes | **1** |
| Alpine (redis:7-alpine, musl) | musl | `/n/selfd: not found` (pas d'interpréteur `/lib64/ld-linux-x86-64.so.2`) | **127** |

Autrement dit : **le plancher réel de ce binaire est glibc ≥ 2.39**, c'est-à-dire
Ubuntu 24.04, Debian 13 ou plus récent. Ni Debian 12, ni Ubuntu 22.04, ni
Debian 11, ni aucune distribution musl ne peuvent l'exécuter aujourd'hui.

### 1.2 `« on le copie, il démarre »` : l'affirmation de l'en-tête de `main.rs`

`crates/obrain-selfd/src/main.rs` porte, depuis l'origine : *« `selfd` est un
binaire de ~750 Ko : on le copie, il démarre — en 4 ms. »* La première moitié est
vraie sous une autre forme (**1,23 Mo** et non 750 Ko) et la seconde est vérifiée :
20 lancements de `selfd --call status` mesurés donnent **min 2,8 ms / médiane
3,3 ms / max 5,7 ms** (processus complet, appel inclus). **Ce qui est faux, c'est
« on le copie, il démarre » hors glibc ≥ 2.39 x86-64**, ce que l'en-tête ne dit
pas. C'est exactement le point à corriger dans la documentation du crate (hors de
mon périmètre d'écriture — voir § 8).

Ce qui reste juste, et qui est la vraie propriété à défendre : le binaire **ne
dépend ni d'un arbre `node_modules`, ni du `PATH` d'un shell, ni d'une chaîne de
compilation sur la machine d'arrivée**. Deux bibliothèques système, et rien
d'autre.

### 1.3 La provenance du binaire n'est pas établie par le binaire

`readelf -p .comment` ne contient que la version de rustc : **rien ne relie
l'exécutable à un commit**. `git describe` nomme le commit de l'arbre *au moment
de l'empaquetage*, pas celui du build.

Ce qu'on peut mesurer, et ce qui est implémenté comme porte (voir § 4) :

- les fichiers de `crates/obrain-selfd`, `crates/obrain-httpmin`, `Cargo.toml` et
  `Cargo.lock` étaient **plus anciens** que le binaire au relevé initial
  (binaire `2026-09-15 22:37:10Z`, sources `11:32`/`22:37:05`) : le binaire
  pouvait donc contenir leur code ;
- **au moment de la rédaction, ce n'est plus vrai** : d'autres chantiers ont
  modifié `crates/obrain-selfd/src/{attache,politique,protocole}.rs` et ajouté
  `crates/obrain-selfd/tests/capacites.rs` **après** la construction. Le binaire
  présent dans `target/release/` ne correspond donc plus aux sources présentes, et
  il doit être reconstruit avant toute installation.

Conclusion à retenir : **le binaire à déployer n'est pas encore celui qu'on
croit**. `scripts/selfd-empaqueter.sh` refuse cet état (porte de fraîcheur, § 4)
au lieu de produire un artefact irreconstructible.

---

## 2. Ce qui est déployable aujourd'hui, et ce qui manque

### Déployable aujourd'hui

- **le binaire**, tel quel, sur une machine x86-64 avec glibc ≥ 2.39 ;
- **la chaîne d'installation complète** (empaquetage → artefact versionné →
  installation sur une machine, locale ou par `ssh`) : livrée par ce chantier ;
- **le mode `serve`** (socket Unix locale, aucun port) et **le mode `attacher`**
  (canal sortant vers `/api/remote/*`), tous deux vérifiés en fonctionnement réel
  sur n3.

### Ce qui manquait, et qui est livré ici

| manque constaté le 2026-09-15 | livré le 2026-09-16 |
|---|---|
| aucune unité systemd `selfd` | `deploy/systemd/selfd.service` + `selfd.attacher.service` (`systemd-analyze --user verify` : **exit 0**) |
| aucun script d'installation | `scripts/selfd-installer.sh` (porte glibc, transfert vérifié, unité rendue, commande de rattachement) |
| aucun manifeste (sha/commit/describe) | `scripts/selfd-empaqueter.sh` écrit `MANIFEST-*.txt` + `.sha256`, et refuse un arbre sale (R2) |
| aucune procédure de copie vers une autre machine | § 5 : la procédure exacte, machine par machine |
| rien de `selfd` ne tourne | toujours vrai — **et c'est délibéré** : rien n'a été activé par ce chantier (R6) |

### Ce qui manque encore, et qu'aucun script ne comble

1. ~~**Un binaire reconstruit**~~ — **fait le 2026-09-16 13:19, sur ordre du
   donneur d'ordre** (§ 11). Les sources de `selfd` avaient changé après le build
   (§ 1.3) ; le binaire est désormais **postérieur à ses sources**. Ce qui reste
   n'est plus une reconstruction mais la porte R2 : l'empaqueteur refuse encore
   de produire l'artefact tant que l'arbre porte des fichiers non commités, et
   ces fichiers ne sont pas ceux de ce chantier (§ 11.1).
2. **Une cible devant le nom de la machine** : la base `machine:` n'existe
   toujours pas côté moteur (aucune occurrence de nœud `machine:` dans le code
   Rust — constat du donneur d'ordre). Un nœud qui s'attache est *listé* par
   `/api/remote/clients`, avec le nom qu'il a déclaré, mais rien ne le relie au
   graphe. C'est un autre chantier.
3. **Un jeton IAM** pour l'utilisateur qui s'attache : `/api/remote/clients`
   répond `HTTP 401` sans identité et `HTTP 401` avec un jeton invalide
   (mesuré ; voir § 6).
4. **Une cible joignable** : n1 refuse la clé, n2 répond en ssh mais n'a aucun
   store, dgx et m4 sont hors d'atteinte (`docs/MACHINES-CONNUES-2026-09-15.md`).
   Rien de tout cela n'est un problème de binaire : c'est un problème d'accès.

---

## 3. Pourquoi ce binaire est « le nœud », et ce qu'il fait vraiment

Trois surfaces, **un seul** `protocole::executer_outil` derrière (une capacité
ajoutée une fois est disponible partout) :

| mode | transport | autorité |
|---|---|---|
| `selfd serve` | socket Unix, `0600` sous `%t` ou l'état du nœud | les permissions du fichier |
| `selfd mcp` | stdio | le lanceur (configuration du harness, accès ssh) |
| `selfd --stdio` | stdio (chemin `ssh` direct) | le lanceur |
| `selfd attacher` | HTTP **sortant** vers `/api/remote/*` du moteur | le jeton du compte + la politique locale |

La politique est fixée **au démarrage** et jamais négociée : `--root` borne tous
les chemins (canonisés, donc les `..` et les liens symboliques sont refusés de la
même façon), et sans `--allow-exec` le nœud observe. Vérifié sur n3 :

```text
$ selfd --root /tmp/ch6-selfd-racine --call read '{"path":"/etc/hostname"}'
selfd : chemin refusé : « /etc/hostname » sort de la racine du nœud (/tmp/ch6-selfd-racine).
        C'est une évasion, pas une faute de frappe — elle est refusée, pas normalisée.   (exit 1)

$ selfd --call exec '{"cmd":"id"}'
selfd : exécution NON AUTORISÉE sur ce nœud : il a été démarré sans --allow-exec.
        Un nœud qui n'a pas été autorisé à agir observe ; il n'agit pas.                 (exit 1)

$ selfd --root … --call read '{"path":"temoin.txt"}'
{"bytes": 39, "content": "fichier-temoin: le selfd agit vraiment\n", … }                 (exit 0)
```

Et le démon, qui est le mode que l'unité fait tourner (`Serveur` sur une socket
`0600`, bannière de démarrage, identité persistante) :

```text
$ selfd serve --root /tmp/ch6-selfd-racine --socket /tmp/ch6-selfd-daemon.sock --state-dir /tmp/ch6-selfd-state &
selfd 0.0.1 — nœud node:n3-0f60e9df · socket /tmp/ch6-selfd-daemon.sock · racine … · exec REFUSE
$ selfd --socket /tmp/ch6-selfd-daemon.sock status      # la CLI parle au DÉMON
nœud      : node:n3-0f60e9df
atteint   : socket
racine    : /tmp/ch6-selfd-racine
exec      : refusé (démarrer avec --allow-exec)
$ selfd --socket … read '{"path":"temoin.txt"}'         # il agit : contenu rendu, exit 0
$ selfd --socket … list '{}'                            # il voit la racine
$ selfd --socket … read '{"path":"/etc/hostname"}'      # refus motivé, exit 1
```

---

## 4. La chaîne d'installation livrée

```text
scripts/selfd-empaqueter.sh   →  dist/selfd-<commit>-<triple>.tar.gz
                                 dist/selfd-<commit>-<triple>.manifest.txt
                                 dist/<…>.tar.gz.sha256
scripts/selfd-installer.sh    →  binaire versionné + lien stable, racine,
                                 ~/.config/selfd/selfd.env (0600),
                                 unité systemd --user RENDUE,
                                 commande de rattachement affichée
deploy/systemd/*.service      →  le modèle, et la politique en commentaires
```

L'artefact est écrit **hors du dépôt**
(`${SELFD_DIST:-~/.local/share/obrain-selfd/dist}`) : un artefact déposé dans
l'arbre le rendrait sale, et la porte R2 de l'empaquetage suivant refuserait —
le script se bloquerait par son propre produit. Si l'équipe veut `dist-selfd/`
dans l'arbre, il faut l'ajouter à `.gitignore` (non fait : ce fichier n'est pas
dans mon périmètre).

### Les portes, et ce que chacune refuse

**`selfd-empaqueter.sh`** (codes 0/2/3/4/5) :

| porte | refus motivé |
|---|---|
| le binaire existe | `3` — « ce script EMPAQUETTE, il ne COMPILE pas » : un `cargo build --release` ici écraserait le binaire que sert le service 8432 |
| `git status --porcelain` vide | `2` — R2, avec la liste des fichiers sales |
| sources plus anciennes que le binaire | `2` — le périmètre est nommé (selfd, httpmin, manifests) |
| `ldd` ne dit pas `not found` | `4` — une dépendance non résolue localement ne se résoudra pas mieux ailleurs |
| pas deux binaires sous le même nom | `5` — « c'est peut-être celui qui tourne » |
| l'archive se relit comme le binaire | `4` — porte finale : `tar -xzO` re-haché |

**`selfd-installer.sh`** (codes 0/2/3/4/5) :

| porte | refus motivé |
|---|---|
| cible joignable (`ssh -o BatchMode=yes`) | `2` — rien n'est copié sur une machine qu'on ne joint pas |
| `--racine` fourni, et ≠ `/` | `5`/`2` — pas de racine par défaut, pas de nœud « confiné » dans `/` |
| `sha256` du binaire == manifeste | `3` — « on ne pose pas un binaire que son propre manifeste ne décrit pas » |
| `DEPLOYABLE: oui` | `2` — un artefact d'épreuve (arbre sale) ne se pose pas ; le contournement `--epreuve-arbre-sale` est bridé à la cible `local` |
| glibc de la cible ≥ glibc exigée | `2` — avec les **deux chiffres** cités (voir § 6) |
| `libgcc_s.so.1` présent sur la cible | `2` — le `NEEDED` du binaire, vérifié là-bas et pas ici |
| `sha256` **après transfert** | `4` — le fichier arrivé est re-haché sur la cible |
| le nœud démarre **et rend sa politique** | `4` — sonde `--call status` avec la politique demandée ; `exec AUTORISÉ` et `exec refusé` sont distingués |

Preuve de bout en bout (arbre sale, donc artefact marqué NON DÉPLOYABLE) :

```text
$ scripts/selfd-empaqueter.sh --epreuve-arbre-sale
selfd-empaqueter : artefact produit
  artefact  : ~/.local/share/obrain-selfd/dist/selfd-1f8b2209-x86_64-unknown-linux-gnu-ARBRE-SALE.tar.gz
  manifeste : …-ARBRE-SALE.manifest.txt
  sha256    : fbc4bf58… (1226360 octets)
  provenance: … sources plus récentes que le binaire → NON ÉTABLIE
  liaison   : dynamique (interpréteur /lib64/ld-linux-x86-64.so.2), glibc exigée GLIBC_2.39
  relu      : le binaire extrait de l'archive a le même sha256 (porte franchie)
  DEPLOYABLE: NON

$ scripts/selfd-installer.sh local --artefact <artefact> --racine /tmp/ch6-selfd-racine    # sans le drapeau d'épreuve
selfd-installer : REFUS — cet artefact est marqué NON DÉPLOYABLE …                            (exit 2)

$ scripts/selfd-installer.sh local --artefact <artefact> --racine /tmp/ch6-selfd-racine --epreuve-arbre-sale
   glibc      : 2.39 >= 2.39 — la cible peut charger ce binaire
   libs       : libgcc_s.so.1 présent sur la cible (NEEDED du binaire)
   binaire    : ~/.local/lib/selfd/selfd-1f8b2209-fbc4bf58fa00   (sha256 vérifié APRÈS transfert)
   lien       : ~/.local/lib/selfd/selfd -> selfd-1f8b2209-fbc4bf58fa00
   sonde      : nœud : node:n3-e71f389e
                exec : refusé (démarrer avec --allow-exec) — politique appliquée vérifiée
   unité      : ~/.config/systemd/user/selfd.service
   … Rien n'a été activé (par défaut, l'installateur ne démarre pas un service : R6 …)
```

Et les unités passent la vérification de systemd **sans erreur** :

```text
$ systemd-analyze --user verify deploy/systemd/selfd.service            → exit 0
$ systemd-analyze --user verify deploy/systemd/selfd.attacher.service   → exit 0
$ systemd-analyze --user verify ~/.config/systemd/user/selfd.service    → exit 0
```

`systemd-analyze verify` contrôle que le programme d'`ExecStart` existe : l'unité
du dépôt n'était donc vérifiable **qu'après** une installation. C'est le prix
d'un chemin littéral (`%h/.local/lib/selfd/selfd`), et il est délibéré — un
chemin qu'on peut lire vaut mieux qu'une variable qu'on doit résoudre pour savoir
ce qui tourne.

---

## 5. La procédure exacte, machine par machine

### 5.0 Préalables (sur la machine de construction, n3)

```bash
cd ~/projects/banc-obrain-tronc
cd crates/obrain-selfd && cargo check          # jamais --release : target/release/obrain est servi par 8432
# … puis, quand le donneur d'ordre l'a décidé :
#   cargo build --release -p obrain-selfd
bash scripts/selfd-empaqueter.sh                # refuse un arbre sale (R2)
```

Cibles à mesurer **avant** de copier quoi que ce soit :

```bash
ssh <hote> 'uname -m; ldd --version | head -1'                       # architecture + glibc
ssh <hote> 'ldconfig -p | grep -c libgcc_s.so.1'                     # NEEDED du binaire
```

### 5.1 n3 (192.168.1.103) — la machine locale, glibc 2.39 : **déployable**

```bash
bash scripts/selfd-installer.sh local \
  --artefact ~/.local/share/obrain-selfd/dist/selfd-<commit>-x86_64-unknown-linux-gnu.tar.gz \
  --racine "$HOME/selfd-racine" \
  --moteur http://192.168.1.103:8432
# puis, si — et seulement si — l'on veut que le nœud AGISSE :
#   … --allow-exec
# l'activation est un acte distinct :
systemctl --user daemon-reload && systemctl --user enable --now selfd
```

Puis rattacher (voir § 6 pour le jeton) :

```bash
selfd attacher --moteur http://192.168.1.103:8432 --jeton-moteur <jeton-de-votre-compte> --machine n3
```

### 5.2 n2 (192.168.1.102) — ssh autorisé, aucun store : **à mesurer d'abord**

`docs/MACHINES-CONNUES-2026-09-15.md` : clé autorisée, binaire `obrain` présent
(158 Mo, 9 sept.), **aucun store**, rien qui tourne. La glibc de n2 **n'a pas pu
être mesurée depuis n3** (`ssh n2` : « Could not resolve hostname », `ssh` sans
entrée `Host` dans `~/.ssh/config`) — c'est l'installateur qui la mesurera, et il
refusera si elle est trop ancienne.

```bash
bash scripts/selfd-installer.sh n2 \
  --artefact <artefact> --racine '$HOME/selfd-racine' \
  --moteur http://192.168.1.103:8432
```

Si la porte refuse (glibc < 2.39) : c'est le résultat attendu, pas un incident —
voir § 7 pour ce qu'il faudrait faire.

### 5.3 n1 (.101), dgx (.107), m4 (.54) — **non déployables aujourd'hui**

- **n1** : clé ssh **refusée** (constat du 2026-09-15). Rien à faire tant que
  l'accès n'est pas ouvert ; l'installateur échoue proprement, sans rien copier.
- **dgx** : répond en HTTP (SPA) mais rien n'indique un harness ni un ssh ouvert.
- **m4** : port 80 en **502** — un amont est tombé, ce n'est pas une absence.

Aucune de ces machines n'a été approchée par ce chantier : les preuves sont
locales à n3, et rien n'a été déployé ailleurs.

### 5.4 Retour arrière (une commande, par construction)

```bash
# sur la machine cible, les binaires versionnés restent en place :
ls -1 ~/.local/lib/selfd/
ln -sfn ~/.local/lib/selfd/selfd-<commit-précédent>-<sha12> ~/.local/lib/selfd/selfd
systemctl --user restart selfd        # (l'arrêt/démarrage du service appartient à l'exploitant)
```

Le manifeste installé à côté (`selfd-<commit>-<sha12>.manifest.txt`) dit toujours
d'où vient le binaire qu'on remet.

---

## 6. Le rattachement, et ce qui le garde

Le canal est `/api/remote/*`, servi par **`obrain viz serve`** (8432) — pas par
`harness-serve` (8090). Mesuré sur le déploiement réel, sans identité et avec un
jeton invalide :

```text
$ curl -s -o /dev/null -w '%{http_code}' http://192.168.1.103:8432/api/remote/clients
401   {"error":"aucune identité présentée (… l'IAM est armé …)"}
$ curl -s -o /dev/null -w '%{http_code}' -H 'Authorization: Bearer bidon' http://192.168.1.103:8432/api/remote/clients
401   {"error":"jeton invalide, révoqué ou expiré"}
```

La commande de rattachement, **exacte**, est celle que l'installateur affiche :

```bash
selfd attacher --moteur <url-du-moteur> --jeton-moteur <jeton-de-votre-compte> --machine <nom>
```

- le jeton est celui de **votre compte** sur le 8432 (IAM armé) ; il n'a rien à
  voir avec le jeton de la surface locale du nœud ;
- passer le jeton en argument le rend **lisible dans `ps`** par tout utilisateur
  de la machine — `selfd` le dit lui-même sur la sortie d'erreur. La forme
  recommandée est le fichier `~/.config/selfd/attacher.env` (0600) écrit par
  l'installateur, lu via `$SELFD_MOTEUR_TOKEN` :

```bash
systemctl --user enable --now selfd.attacher      # si --jeton-moteur a été fourni à l'installateur
```

- **`selfd attacher` est une connexion SORTANTE** : rien à ouvrir sur le routeur
  de la machine. Un jeton refusé est une erreur **définitive** : la boucle
  s'arrête (le journal dit « le moteur refuse ce jeton ») et l'unité atterrit en
  `failed` après 6 essais — un service qui bat toutes les 5 s en silence serait
  pire qu'un échec visible.

Le chemin a été exercé **de bout en bout contre le moteur réel**, avec un jeton
volontairement invalide (aucun client n'est enregistré, l'appel est refusé avant
toute écriture) :

```text
$ selfd attacher --moteur http://192.168.1.103:8432 --jeton-moteur bidon --machine n3-test
selfd : le jeton passé en argument est visible dans `ps` par tout utilisateur de cette machine.
        Préférez la variable SELFD_MOTEUR_TOKEN.
selfd : le moteur refuse ce jeton — vérifiez le jeton de VOTRE compte          (exit 1)
```

Ce que cela prouve : le binaire atteint `/api/remote/attacher` sur 8432, l'IAM le
refuse faute d'identité valide, et **le refus remonte avec son motif** au lieu de
tourner en silence. Ce que cela ne prouve pas : le rattachement *réussi* (il
demande un jeton de compte réel, que ce relevé n'a pas).

### 6.1 Une incohérence constatée, et ce qu'elle casse

Le moteur annonce lui-même, dans `GET /api/remote/clients`
(`crates/obrain-cli/src/viz_serve.rs`) :

> « Sur votre machine : `selfd attacher --moteur <url> --jeton <votre jeton>` »

Or `selfd` **n'accepte pas `--jeton`** — mesuré :

```text
$ selfd attacher --moteur http://192.168.1.103:8432 --jeton un-jeton --machine test
selfd : option inconnue : --jeton (voir --help)          (exit 2)
```

Le drapeau est `--jeton-moteur` (ou `$SELFD_MOTEUR_TOKEN`). La seule aide que le
produit donne à l'utilisateur pour se rattacher est donc **fausse**, et elle
envoie l'utilisateur sur une erreur de syntaxe. Correction à faire dans
`viz_serve.rs` (hors de mon périmètre d'écriture — écart § 8) : soit la chaîne
d'aide devient `--jeton-moteur`, soit `selfd` accepte `--jeton` comme alias.

---

## 7. Ce qu'il faudrait pour que « on le copie, il démarre » soit vrai partout

Par ordre de coût croissant :

1. **Construire sur la plus vieille distribution visée** (recommandé, aucun
   nouvel outil) : la compatibilité glibc se gagne en compilant contre la glibc
   la plus ancienne du parc, dans un conteneur (Debian 12 → plancher 2.36,
   Ubuntu 22.04 → 2.35). Le binaire obtenu démarre sur tout ce qui est plus
   récent. Le coût est une chaîne de construction par distribution visée.
2. **Un binaire statique musl** : `rustup target add x86_64-unknown-linux-musl`
   puis `cargo build --release --target x86_64-unknown-linux-musl`. Le résultat
   ne dépend d'aucune glibc (donc démarre aussi sur Alpine). **Non installé ici,
   et non installé par ce chantier** : la cible n'est pas présente
   (`rustup target list --installed` : `wasm32-unknown-unknown`,
   `x86_64-unknown-linux-gnu`) et je ne suppose pas le réseau ni ne modifie la
   chaîne d'outils de la machine de build. À décider par le donneur d'ordre.
   Note : `x86_64-unknown-linux-musl` est une cible *statique par défaut* ; elle
   se compile sans conteneur, mais le résultat est plus gros et ne se lie pas
   dynamiquement à la libc du système.
3. **Un conteneur par machine** : `selfd` s'exécute dans une image figée, on
   copie l'image. Cela résout la glibc et l'architecture, au prix d'un moteur de
   conteneurs sur chaque machine — lourd pour un nœud de 1,2 Mo qui n'a besoin de
   rien.
4. **Un artefact par architecture** : rien n'existe pour `aarch64` aujourd'hui
   (aucune cible installée). Le nom d'artefact porte déjà le triple
   (`selfd-<commit>-<triple>.tar.gz`), donc la chaîne est prête ; il manque la
   cible et un build.

Ce qui n'est **pas** résolu par les quatre points ci-dessus, et qu'il faut savoir :

- **aucune signature** : rien ne prouve que l'artefact vient bien de nous
  (`sha256` + manifeste donnent l'intégrité et l'identité *déclarée*, pas la
  provenance). Un dépôt d'artefacts signé serait la vraie réponse ;
- **l'architecture de la cible n'est vérifiée qu'après transfert** (la porte
  compare `uname -m` avant) ;
- **le binaire ne porte pas son commit** : un build reproductible
  (`--remap-path-prefix`, `SOURCE_DATE_EPOCH`) ou une en-tête de provenance
  ajoutée à l'édition de liens rendrait la reconstruction vérifiable ;
- **`selfd` n'est pas strippé** : 1,2 Mo dont une table de symboles complète.
  `strip` gagnerait de la place, au prix de traces d'appel moins lisibles — à
  décider, pas à faire en silence.

---

## 8. Écarts constatés (fichiers hors de mon périmètre d'écriture)

Aucun n'a été corrigé : je les signale, je ne les fais pas.

1. **`crates/obrain-cli/src/viz_serve.rs`** — l'aide de `/api/remote/clients`
   annonce `selfd attacher --moteur <url> --jeton <votre jeton>` ; le drapeau
   réel est `--jeton-moteur` (§ 6.1, mesuré : `option inconnue : --jeton`).
2. **`crates/obrain-selfd/src/main.rs`** — l'en-tête du module affirme « on le
   copie, il démarre » sans dire la dépendance de glibc (§ 1.2). La note présente
   la mesure ; l'en-tête du crate, lui, reste optimiste.
3. **`.gitignore`** — pas d'entrée pour l'artefact (`dist-selfd/`). C'est la
   raison pour laquelle l'empaqueteur écrit hors du dépôt. Si l'équipe préfère un
   artefact dans l'arbre, il faut ajouter l'entrée *avant* de changer le script.
4. **`Cargo.toml` (profil release)** — `lto = "thin"`, `codegen-units = 1` : rien
   n'y documente la compatibilité glibc visée. Un commentaire (ou un réglage)
   gagnerait à dire quelle distribution minimale est visée, puisque c'est
   désormais une propriété de déploiement.
5. **Aucun nœud `machine:` dans le code Rust** (constat du donneur d'ordre,
   re-vérifié par `grep`) : le rattachement *liste* la machine dans
   `/api/remote/clients`, mais le graphe ne la connaît pas. Le pilotage « par
   machine » reste donc à construire — c'est le sujet d'un autre chantier.

---

## 9. Ce que je n'ai pas fait, et ce que je n'ai pas prouvé

- **Je n'ai rien activé, rien redémarré** : aucun `systemctl daemon-reload`, aucun
  `enable`, aucun `start`. L'unité livrée est un fichier, et l'installation locale
  faite pour la preuve s'est arrêtée avant l'activation (comportement par défaut).
- **Je n'ai construit aucun binaire release** : `cargo build --release` est
  interdit ici (il écraserait le binaire que sert le service 8432). Les faits du
  § 1 portent donc sur le binaire **existant**, pas sur un binaire reconstruit.
- **Je n'ai rien déployé sur une autre machine** : les seules poses réelles ont
  eu lieu sur n3, et la seule cible `ssh` testée conceptuellement est décrite au
  § 5.2 sans avoir été exécutée.
- **Je n'ai pas mesuré la glibc de n1/n2/dgx/m4** : les noms ne se résolvent pas
  depuis n3 (`ssh n2` → « Could not resolve hostname »). Les portes de
  l'installateur sont là pour ça, mais **elles n'ont pas été exercées sur une
  cible distante** : la porte distante est *affirmée, non prouvée sur le parc*.
- **La porte glibc a été éprouvée par un manifeste falsifié** (GLIBC_2.99), pas
  par une vraie machine trop ancienne : je n'ai pas de machine glibc < 2.39
  joignable. Le refus observé est donc réel, mais provoqué.
- **Le mode `mcp` et le mode `--stdio` n'ont pas été exercés** dans ce relevé :
  seul `--call`, `serve` + CLI par socket, et `--help` l'ont été. Le mode `mcp`
  est le chemin par lequel un harness monte le nœud — il mérite sa propre sonde.
- **Le rattachement (`attacher`) n'a été exercé que jusqu'au REFUS** : le chemin
  sortant a été parcouru contre le moteur réel (§ 6), mais avec un jeton
  volontairement invalide. Un rattachement **réussi** — donc un client enregistré
  visible dans `/api/remote/clients` — demanderait un jeton IAM de compte réel,
  que ce relevé n'a pas. C'est la seule marche qui manque pour dire « un self
  agit sur une machine ».

### 9.1 Effets de bord hors du dépôt (pour la preuve, tous réversibles)

L'exécution réelle des deux scripts a laissé, **sur n3 uniquement** :

| chemin | ce que c'est | ce qui l'a écrit |
|---|---|---|
| `~/.local/share/obrain-selfd/dist/selfd-1f8b2209-x86_64-unknown-linux-gnu-ARBRE-SALE.tar.gz` (+ `.manifest.txt`, `.sha256`) | l'artefact d'ÉPREUVE (NON DÉPLOYABLE) | `selfd-empaqueter.sh --epreuve-arbre-sale` |
| `~/.local/lib/selfd/selfd-1f8b2209-fbc4bf58fa00` (+ `.manifest.txt`) et le lien `selfd` | le binaire posé par l'épreuve | `selfd-installer.sh … --epreuve-arbre-sale` |
| `~/.config/selfd/selfd.env` (0600) | politique locale du nœud de test | idem |
| `~/.config/systemd/user/selfd.service` | l'unité RENDUE — **non chargée, non activée** | idem |
| `~/.local/state/selfd/node.json` | l'identité du nœud, créée par la sonde d'installation | idem |
| `/tmp/ch6-*` | racine, état et journaux de test | idem |

**Aucun `systemctl` n'a été exécuté** : l'unité n'est ni chargée par le
gestionnaire systemd (pas de `daemon-reload`), ni activée, ni démarrée. Le
processus `selfd serve` lancé pour la preuve de la socket a été arrêté — c'était
mon propre processus, pas un service.

Pour tout retirer :

```bash
rm -rf ~/.local/share/obrain-selfd ~/.local/lib/selfd ~/.config/selfd \
       ~/.local/state/selfd ~/.config/systemd/user/selfd.service /tmp/ch6-*
```

---

## 10. En une phrase

Le binaire existe, il agit vraiment (lu, listé, refusé, motivé — démontré sur
n3), et la chaîne d'empaquetage, d'installation et d'unité systemd qui manquait
entièrement est livrée et exercée ; il a depuis été **reconstruit** sur ses
sources à jour (§ 11), donc la provenance n'est plus un obstacle ; mais
**« on le copie, il démarre » n'est vrai que sur x86-64 avec glibc ≥ 2.39**, et
**aucun self n'agit encore sur une machine** parce que rien n'a été activé et que
rien ne s'est rattaché : ces deux derniers points sont des décisions, pas des
manques de code.

---

## 11. Reconstruction du 2026-09-16 13:19 (sur ordre du donneur d'ordre)

Ce chapitre est écrit **après** le relevé : il met le document d'accord avec un
fait nouveau, il ne réécrit pas les mesures du § 1 (qui portaient sur le binaire
d'alors, et restent vraies de lui).

### 11.1 Ce qui a été fait, et ce que la porte refuse encore

```bash
# le binaire d'avant est mis de côté — doctrine G4 (jamais de cp sur un vivant)
cp -p target/release/selfd target/release/selfd.avant-reconstruction-20260916-131929

# reconstruction CIBLÉE : obrain-selfd ne dépend que de anyhow, serde_json et
# obrain-httpmin. `-p` garantit que `target/release/obrain` — le binaire que sert
# le service 8432 — n'est ni relu ni réécrit.
nice -n 19 env CARGO_BUILD_JOBS=6 cargo build --release -p obrain-selfd
#   → Finished `release` profile [optimized] target(s) in 11.11s

sha256sum target/release/selfd target/release/obrain
#   d10f6c2d164d6d58ce84c07a5ea305e2f06dff6e37e81a833599435a21987ccb  selfd   (1 249 424 o)
#   bb4ebfa03da9c9e1278c4f3d115945eaa1d8198bd882dcfb536a9c05241691c0  obrain  (INCHANGÉ, 13:11)
```

La provenance est désormais établie — plus aucune source de `selfd` n'est plus
récente que le binaire :

```bash
find crates/obrain-selfd -type f -newermt "@$(stat -c %Y target/release/selfd)"
#   (vide)
```

**L'empaquetage NORMAL refuse toujours, et c'est la porte R2 qui fait son
travail** (exit 2) — mais plus à cause du binaire : à cause de trois fichiers
non commités qui **n'appartiennent pas à ce chantier** et que le donneur d'ordre
a délibérément laissés intacts (travail en cours d'une autre session) :

```text
     M crates/obrain-cli/src/mcp_server.rs
     M scripts/verifier-integration-tissage.sh
    ?? crates/obrain-cli/examples/reparer_etiquettes.rs
```

Les commiter ou les mettre de côté suffit à débloquer l'artefact : la
reconstruction, elle, est faite.

### 11.2 Ce que la reconstruction CORRIGE, mesuré sur le binaire neuf

Le § 2 de `docs/ACCEPTATION-FLOTTE-2026-09-16.md` avait trouvé, **sur l'ancien
binaire**, que « `write` est annoncé par n1 alors que sa racine est en LECTURE
SEULE » — le chantier « annoncer est décider » a corrigé ce défaut dans les
sources, et la reconstruction le rend observable. Deux racines, même binaire :

```bash
# racine inscriptible → write annoncé
selfd --root /tmp/rw --call caps
#   "capacites": ["read","list","write","status"]   ← write présent, raison : la racine est inscriptible
#   "capacites_refusees": [ { "nom": "exec", "motif": "…démarré sans --allow-exec…" } ]

# racine en LECTURE SEULE → write n'est PLUS annoncé
chmod 555 /tmp/ro && selfd --root /tmp/ro --call caps
#   "capacites": ["read","list","status"]           ← write ABSENT
#   "capacites_refusees": [ { "nom": "write",
#       "motif": "écriture impossible à la racine « /tmp/ro »
#                 (Permission denied (os error 13)) : write n'est pas exercable ici,
#                 il n'est donc pas annoncé — l'annoncer ferait proposer un geste voué au refus" } ]
```

Et les **limites** sont désormais annoncées avec les capacités
(`racine`, `delai_ms_par_defaut`, `plafond_octets_par_flux`, `exec` ouvert ou
fermé, `version`), ce qui répond au « que sait faire CHACUNE » de la question de
flotte.

Le binaire neuf agit vraiment, et ses refus restent motivés et non silencieux :

```text
status → nœud node:n3-e97c19a9 · machine n3 · racine /tmp/… · exec refusé
read   → {"bytes": 7, "content": "temoin\n", "path": "/tmp/…/t.txt", "tronque": false}
read /etc/hostname → exit 1, « chemin refusé : « /etc/hostname » sort de la racine
                     du nœud (…). C'est une évasion, pas une faute de frappe —
                     elle est refusée, pas normalisée. »
exec   → exit 1, « exécution NON AUTORISÉE sur ce nœud : il a été démarré sans
                  --allow-exec. Un nœud qui n'a pas été autorisé à agir observe ;
                  il n'agit pas. »
```

Les 11 tests de `selfd` (4 sur `soi`, 7 sur les capacités) passent sur cette
source reconstruite.

### 11.3 Ce que la reconstruction NE change PAS

- **`GLIBC_2.39` est toujours exigée** : `objdump -T` sur le binaire neuf rend
  encore `GLIBC_2.39`. Reconstruire sur n3 ne rend donc pas le binaire portable
  sur Debian 12 ni sur musl — la réponse reste celle du § 7 (construire sur la
  plus vieille distribution visée, ou `x86_64-unknown-linux-musl`).
- **Rien n'a été activé ni déployé** : aucun `systemctl`, aucun rattachement,
  aucune pose sur une machine distante. Le 8432 et ses voisins (8090, 8144) ont
  été vérifiés intacts après coup : mêmes PID, `active`, health `401`.
- **Le binaire est toujours à édition dynamique et non strippé** (1 249 424 o).

---

## 12. 2026-09-16, 18:40 — l'artefact existe, et il n'est PAS déployable : ce qui reste tient en UNE reconstruction

Relevé du soir, même dépôt, même branche, HEAD `9fed71eb`. Le chapitre 11 disait
l'obstacle « trois fichiers non commités qui n'appartiennent pas à ce chantier ».
Ces trois fichiers ont été commités par leur propriétaire (son travail a été
fusionné) : **l'obstacle a donc changé de nature**, et il faut le redire, parce
que le conseil du § 11.1 (« commiter ces trois fichiers ») ne débloque plus rien.

### 12.1 Le refus est-il levé ? NON — et voici les deux portes, dans l'ordre

Mesuré à 18:34, `git status --short` : **vide**, arbre propre. Puis :

```text
$ bash scripts/selfd-empaqueter.sh
selfd-empaqueter : REFUS — ces sources sont PLUS RÉCENTES que le binaire (construit
  2026-09-16T13:19:43Z) : le binaire ne peut pas contenir leur code.               (exit 2)
    Cargo.lock
```

`Cargo.lock` est le **seul** fichier du périmètre en avance sur le binaire :

```text
$ find crates/obrain-selfd crates/obrain-httpmin -type f -newermt "@$(stat -c %Y target/release/selfd)"
(vide)                       # aucune source de selfd plus récente
$ git log --since='2026-09-16T13:19:43Z' --format='%h %cI %s' -- Cargo.lock
0be27697 2026-09-16T15:29:55+00:00 T27 — Cargo.lock : rayon direct dans obrain-cli
c6fbc901 2026-09-16T15:29:55+00:00 indexeur : cache d'extraction par empreinte de contenu (T27)
de83a3cd 2026-09-16T15:34:51+00:00 indexeur : tables internes en foldhash (T27)
$ git diff 8b65a48d HEAD --stat -- Cargo.lock
 Cargo.lock | 4 ++++
```

Quatre lignes ajoutées, dans les listes de dépendances de **`obrain-cli`**
(`rayon`) et **`obrain-indexer`** (`bincode`, `foldhash`, `twox-hash`). Rien dans
la liste de `obrain-selfd`. Et le périmètre propre de `selfd` est **identique**
entre le commit du build (`8b65a48d`, 13:18) et HEAD :

```text
$ git diff --stat 8b65a48d HEAD -- crates/obrain-selfd crates/obrain-httpmin Cargo.toml
(vide)                       # le binaire est reconstructible depuis HEAD sur SA source
```

**Ce que cela veut dire, sans arrondir.** La porte de fraîcheur ne se trompe pas
sur son principe — un `Cargo.lock` modifié peut changer un binaire, et c'est bien
pour cela qu'il est dans son périmètre. Mais son périmètre est le `Cargo.lock`
**entier du workspace**, alors que son énoncé (en-tête du script, § 3) parle des
« sources de `selfd` » : ici, elle refuse un artefact dont la source est à HEAD.
C'est un **faux positif de périmètre**, pas une péremption réelle — **au moment
de cette mesure** : à 18:39, une source de `selfd` a été modifiée, et le refus est
devenu incontestable (§ 12.1.2). Deux issues, et **ce chantier n'a pris ni l'une
ni l'autre** :

1. **forcer une reconstruction RÉELLE** du binaire (11 s, c'est la voie courte) ;
2. **restreindre le périmètre** au graphe de dépendances de `selfd` — ce serait
   relâcher la porte qui a établi R2 : cela se décide, cela ne se subit pas.

### 12.1.1 Le piège : « reconstruire, puis relancer » ne suffit pas toujours

Le message de refus conseillait « Reconstruire le binaire, puis relancer ». Si
`cargo` n'a rien à refaire, il affiche `Finished` **sans relinker**, le binaire
garde sa date, et le refus revient à l'identique — l'exploitant recommence sans
comprendre. Indice fort, mesuré sur cette machine : la construction release de
17:45 (celle qui a produit l'`obrain` du 8432, § 12.8) a réévalué les unités des
autres crates, **pas celle de `selfd`** :

```text
$ ls -l --time-style=+%H:%M target/release/.fingerprint/obrain-selfd-*/invoked.timestamp
13:19  target/release/.fingerprint/obrain-selfd-7fc2d92e8fa218fa/invoked.timestamp
$ find target/release/.fingerprint -name invoked.timestamp -newermt '2026-09-16T13:19:44Z' | head -3
target/release/.fingerprint/obrain-cli-a7e86b0a4ca0d71b/invoked.timestamp
target/release/.fingerprint/obrain-iam-0a9d7275aa5e8b3b/invoked.timestamp
target/release/.fingerprint/obrain-viz-42b3063ef6f8af49/invoked.timestamp
```

C'est un **indice**, pas une preuve : la commande qui trancherait est
`cargo build --release -p obrain-selfd`, et elle est interdite à cet agent (elle
écrirait dans `target/release/`, que sert le service 8432). Le refus le dit
désormais lui-même, et nomme les commits :

```text
  commits qui ont touché ce périmètre depuis la construction :
      0be27697 2026-09-16T15:29:55+00:00 T27 — Cargo.lock : rayon direct dans obrain-cli
      …
  Deux issues HONNÊTES, et c'est une décision :
    (a) forcer une reconstruction RÉELLE du binaire :
          cargo clean -p obrain-selfd && cargo build --release -p obrain-selfd
    (b) décider que le périmètre de cette porte (Cargo.lock ENTIER) est trop
        large et le restreindre au graphe de dépendances de `selfd`.
```

### 12.1.2 Ce qui a bougé PENDANT ce relevé (18:39) : le refus devient incontestable

Un chapitre daté doit dire aussi ce qui a changé sous lui. À **18:39:49**, la
session voisine a modifié **`crates/obrain-selfd/src/attache.rs`** (non commité,
+24/−4) — une source **de `selfd` lui-même**, plus récente que le binaire :

```text
$ stat -c '%y  %n' crates/obrain-selfd/src/attache.rs target/release/selfd
2026-09-16 18:39:49  crates/obrain-selfd/src/attache.rs      ← la SOURCE
2026-09-16 13:19:43  target/release/selfd                    ← le binaire
$ find crates/obrain-selfd crates/obrain-httpmin -type f -newermt "@$(stat -c %Y target/release/selfd)"
crates/obrain-selfd/src/attache.rs
```

Conséquence, et elle est nette : la question du § 12.1.1 (issue 2, « le périmètre
est-il trop large ? ») **devient sans objet pour l'instant**. Même un périmètre
restreint au graphe de `selfd` refuserait : il y a une vraie péremption. À partir
de cette minute, **aucune reconstruction antérieure à ce fichier ne peut être
déployée** — et il faut que le travail de la session voisine soit **commité**
d'abord, sans quoi l'artefact porterait du code non commité (R2). Le manifeste
produit à 18:39 le dit de lui-même : `arbre_sale : oui (22 fichier(s))`, et
`provenance` cite `attache.rs` dans la liste des sources en avance.

### 12.2 L'artefact produit aujourd'hui — ÉPREUVE, donc NON DÉPLOYABLE

L'arbre a été **re-salé par la session voisine** pendant ce relevé (elle commite
en continu le dashboard). C'est exactement ce pour quoi le mode d'épreuve existe :

```text
$ bash scripts/selfd-empaqueter.sh --epreuve-arbre-sale
selfd-empaqueter : artefact produit
  artefact  : ~/.local/share/obrain-selfd/dist/selfd-9fed71eb-x86_64-unknown-linux-gnu-ARBRE-SALE.tar.gz
  manifeste : …-ARBRE-SALE.manifest.txt
  sha256    : d10f6c2d164d6d58ce84c07a5ea305e2f06dff6e37e81a833599435a21987ccb  (1249424 octets)
  provenance: commit 9fed71eb (« version-2026-09-16-20-g9fed71eb »), rustc local : rustc 1.93.0
  liaison   : dynamique (interpréteur /lib64/ld-linux-x86-64.so.2), glibc exigée GLIBC_2.39
  relu      : le binaire extrait de l'archive a le même sha256 (porte franchie)
  DEPLOYABLE: NON                                                            (exit 0)
```

```text
$ ls -l ~/.local/share/obrain-selfd/dist/selfd-9fed71eb-*
-rw-rw-r-- 3123    selfd-9fed71eb-…-ARBRE-SALE.manifest.txt
-rw-rw-r-- 550324  selfd-9fed71eb-…-ARBRE-SALE.tar.gz
-rw-rw-r-- 124     selfd-9fed71eb-…-ARBRE-SALE.tar.gz.sha256
    → 5da4d5af743ac88f43030d1d80c76411f053cb60d4953a15ccc2a8bdf271bb2a  (archive)

$ tar -tzvf …-ARBRE-SALE.tar.gz
-rwxr-xr-x triviere/triviere 1249424 2026-09-16 13:19 selfd
-rw-rw-r-- triviere/triviere    3123 2026-09-16 18:39 MANIFEST-selfd-9fed71eb-…-ARBRE-SALE.txt
```

L'archive a été **régénérée à 18:39**, avec la version finale du script (le
manifeste dit 22 fichiers sales au lieu de 6 : la session voisine a continué de
travailler — c'est le prix, et l'intérêt, d'un manifeste qui dit l'arbre réel).
Le binaire emballé, lui, n'a pas changé.

Les champs qui décident, lus **dans l'archive** (pas seulement à côté) :

```text
commit                   : 9fed71eb02599a7d02a74480d8999dc68ec50e17
arbre_sale               : oui (22 fichier(s))
empaquetage              : ÉPREUVE (--epreuve-arbre-sale)
epreuve_motif            : arbre sale : 22 fichier(s) non commité(s) (R2) ; provenance NON ÉTABLIE — sources plus récentes que le binaire
buildid                  : 93c0301b2e22b0ee1a4e3b7c0ed817d8280fa981
glibc_exigee_max         : GLIBC_2.39
glibc_exigee_forte_max   : GLIBC_2.34
glibc_faibles_max        : pidfd_getpid pidfd_spawnp
DEPLOYABLE               : non — ÉPREUVE : arbre sale : 22 fichier(s) non commité(s) (R2) ; provenance NON ÉTABLIE — … pas d'installation
```

Et l'archive se relit comme le binaire — la porte finale, refaite à la main :

```text
$ tar -xzOf …-ARBRE-SALE.tar.gz selfd | sha256sum
d10f6c2d164d6d58ce84c07a5ea305e2f06dff6e37e81a833599435a21987ccb  -
$ sha256sum target/release/selfd
d10f6c2d164d6d58ce84c07a5ea305e2f06dff6e37e81a833599435a21987ccb  target/release/selfd
```

**Il n'est pas déployable, et il le dit à trois endroits** : dans son nom, dans
son manifeste, et dans la porte de l'installateur. Vérifié, pas supposé :

```text
$ bash scripts/selfd-installer.sh local --artefact …-ARBRE-SALE.tar.gz --racine /tmp/ch-selfd-refus-racine
selfd-installer : REFUS — cet artefact est marqué NON DÉPLOYABLE : …                  (exit 2)
$ ls -ld /tmp/ch-selfd-refus-racine
ls: cannot access '/tmp/ch-selfd-refus-racine': No such file or directory
$ bash scripts/selfd-installer.sh n2 --artefact …-ARBRE-SALE.tar.gz --racine /tmp/x --epreuve-arbre-sale
selfd-installer : REFUS — --epreuve-arbre-sale n'accepte que la cible « local ».      (exit 2)
```

Rien n'a été créé sur la cible : la porte refuse **avant** la racine, avant la
copie, avant tout.

### 12.3 Le binaire agit — revérifié sur CE binaire

Socket, état et racine **tous dans `/tmp`** (`/run/user/1000/selfd.sock` d'un
démon éventuel n'a pas été approché) ; le démon est **mon** processus, arrêté
après la mesure :

```text
$ selfd serve --root /tmp/ch-selfd-…/racine --socket /tmp/ch-selfd-…/selfd.sock --state-dir /tmp/ch-selfd-…/state &
selfd 0.0.1 — nœud node:n3-ea9e9f32 · socket /tmp/ch-selfd-…/selfd.sock · racine /tmp/… · exec REFUSE
$ selfd --socket … --state-dir … status
nœud : node:n3-ea9e9f32   machine : n3   atteint : socket   exec : refusé (démarrer avec --allow-exec)   (exit 0)
$ selfd --call read '{"path":"…/temoin.txt"}' --root … --socket … --state-dir …
{ "bytes": 31, "content": "temoin: le selfd agit vraiment\n", "path": "…", "tronque": false }   (exit 0)
$ selfd --call list '{}' …            → 1 entrée, temoin.txt (31 o)                            (exit 0)
$ selfd --call read '{"path":"/etc/hostname"}' …
selfd : chemin refusé : « /etc/hostname » sort de la racine du nœud (…). C'est une évasion, pas une faute de frappe — elle est refusée, pas normalisée.  (exit 1)
$ selfd --call exec '{"cmd":"id"}' …
selfd : exécution NON AUTORISÉE sur ce nœud : il a été démarré sans --allow-exec. …            (exit 1)
```

### 12.4 Le drapeau de rattachement : vérifié, pas supposé

Le § 6.1 constatait que l'aide du moteur annonçait `--jeton`, inexistant. La
vérification demandée, refaite sur ce binaire :

```text
$ selfd attacher --moteur http://127.0.0.1:8432 --jeton bidon --machine test
selfd : option inconnue : --jeton (voir --help)                                      (exit 2)
$ selfd --help | grep -A1 'attacher'
  attacher            branche cette machine au moteur et attend du travail
                      (--moteur <url> --jeton-moteur <jeton> [--machine <nom>])
```

`--jeton` **n'existe toujours pas**; le drapeau est bien `--jeton-moteur`, et
`$SELFD_MOTEUR_TOKEN` reste la forme recommandée (un argument se lit dans `ps`).
La correction de la chaîne d'aide du moteur (`crates/obrain-cli/src/viz_serve.rs`)
reste donc **à faire** — elle est hors de mon périmètre (§ 8, écart 1).

### 12.5 GLIBC : quelle machine peut le recevoir aujourd'hui, et ce qui élargirait

La reconstruction **n'a rien changé** à la question : mesuré sur le binaire neuf,
`objdump -T | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1` → **`GLIBC_2.39`**,
porté par les deux mêmes références **faibles** `pidfd_getpid`, `pidfd_spawnp`.
`ld.so` refuse le binaire ENTIER sur une glibc plus ancienne (§ 1.1, mesuré en
conteneurs le même jour).

| machine | glibc | peut recevoir ce binaire ? |
|---|---|---|
| **n3** (Ubuntu 24.04, 192.168.1.103) | 2.39 | **OUI** — c'est la seule mesurée qui peut |
| toute distribution à glibc ≥ 2.39 (Ubuntu 24.04+, Debian 13+, Fedora 40+) | ≥ 2.39 | OUI, par construction — **non mesuré sur le parc** |
| Debian 12 | 2.36 | NON (mesuré : `version 'GLIBC_2.39' not found`) |
| Ubuntu 22.04 | 2.35 | NON (mesuré) |
| Debian 11 | 2.31 | NON (mesuré) |
| Alpine / musl | — | NON (mesuré : pas d'interpréteur `ld-linux`) |

Ce qui **élargirait**, et ce qui est mesuré sur cette machine aujourd'hui :

```text
$ rustup target list --installed
wasm32-unknown-unknown
x86_64-unknown-linux-gnu          ← ni musl, ni aarch64
$ rustup target list | grep -E '^x86_64-unknown-linux-musl|^aarch64-unknown-linux-(gnu|musl)'
aarch64-unknown-linux-gnu
aarch64-unknown-linux-musl
x86_64-unknown-linux-musl         ← LISTÉE comme disponible, NON installée
```

1. **`x86_64-unknown-linux-musl`** : la cible est *proposée* par la chaîne
   d'outils, elle n'est **pas installée**. `rustup target add` exigerait le
   réseau ; **je ne l'ai pas fait** (ni le réseau ni l'accord de l'exploitant ne
   se supposent). Le binaire obtenu ne dépendrait d'aucune glibc — donc
   démarrerait aussi sur Alpine et sur Debian 12. C'est la voie la moins chère
   pour couvrir tout le parc x86-64.
2. **Construire sur la plus vieille distribution visée** : aucun nouvel outil,
   au prix d'une chaîne de construction par distribution.
3. **`aarch64`** : rien n'existe (aucune cible installée). Le nom d'artefact
   porte déjà le triple, donc la chaîne est prête ; il manque la cible et un
   build.

**Installable aujourd'hui, donc, seuls les x86-64 glibc ≥ 2.39.** Le parc
n1/n2/dgx/m4 n'a pas été mesuré (§ 5.2/5.3) : les portes de l'installateur le
feront, et refuseront en citant les deux chiffres.

### 12.6 La procédure exacte, machine par machine

**Étape 0 — sur n3, obtenir un artefact DÉPLOYABLE** (la seule marche manquante) :

```bash
cd ~/projects/banc-obrain-tronc
git status --short                       # doit être VIDE (R2) — l'arbre est partagé
#   en particulier : le travail en cours de la session voisine sur
#   crates/obrain-selfd/src/attache.rs DOIT être commité d'abord (§ 12.1.2),
#   sinon l'artefact porterait un code que personne ne peut nommer.
cargo clean -p obrain-selfd              # force un relink RÉEL (voir § 12.1.1)
cargo build --release -p obrain-selfd    # JAMAIS sans -p : il écraserait l'obrain du 8432
bash scripts/selfd-empaqueter.sh         # doit rendre exit 0 et « DEPLOYABLE: oui »
```

Si l'empaqueteur refuse encore en nommant un fichier du périmètre, c'est une
vraie péremption : le dire, ne pas la contourner.

**Étape 1 — mesurer la cible AVANT de copier** (l'installateur le refait) :

```bash
ssh <hote> 'uname -m; ldd --version | head -1; ldconfig -p | grep -c libgcc_s.so.1'
```

**Étape 2 — poser** (rien ne démarre) :

```bash
bash scripts/selfd-installer.sh <hote|local> \
  --artefact ~/.local/share/obrain-selfd/dist/selfd-<commit>-x86_64-unknown-linux-gnu.tar.gz \
  --racine '$HOME/selfd-racine' \
  --moteur http://192.168.1.103:8432
# --allow-exec SEULEMENT si le nœud doit exécuter des commandes — c'est un acte
```

**Étape 3 — activer** (acte de l'exploitant, jamais de l'installateur) :

```bash
systemctl --user daemon-reload && systemctl --user enable --now selfd
```

**Étape 4 — rattacher CETTE machine au moteur** :

```bash
selfd attacher --moteur http://192.168.1.103:8432 \
  --jeton-moteur <jeton-de-VOTRE-compte> --machine <nom>
```

Le drapeau est `--jeton-moteur` (**mesuré** : `--jeton` → `option inconnue`,
exit 2). Le jeton est celui du compte sur le 8432 ; en argument il est lisible
dans `ps`, d'où la forme recommandée, déjà écrite par l'installateur :

```bash
systemctl --user enable --now selfd.attacher      # lit ~/.config/selfd/attacher.env (0600)
```

| machine | état aujourd'hui | ce qui bloque |
|---|---|---|
| **n3** (.103) | déployable : x86-64, glibc 2.39 | rien — sauf l'artefact déployable, § 12.6 étape 0 |
| **n2** (.102) | à mesurer | ssh autorisé, **aucun store**, glibc inconnue |
| **n1** (.101) | non déployable | clé ssh refusée |
| **dgx** (.107) / **m4** (.54) | non déployables | hors d'atteinte / 502 en amont |

### 12.7 Ce que ce chantier a corrigé dans les scripts — et ce qu'il n'a PAS relâché

Quatre défauts, tous constatés en exécutant (aucun n'est une relecture) :

1. **Le nom et le motif de l'épreuve mentaient.** Mesuré : arbre **propre**,
   obstacle = `Cargo.lock`. Or `--epreuve-arbre-sale` nommait l'artefact
   `-ARBRE-SALE` et le manifeste écrivait « DEPLOYABLE: non — ÉPREUVE (arbre
   sale, R2) », sur la même page qu'`arbre_sale : non`. Un motif faux envoie
   nettoyer un arbre déjà propre pendant que la vraie cause reste. Le motif est
   désormais **calculé depuis ce qui a été observé** (champ `epreuve_motif`), et
   le nom suit : `-ARBRE-SALE` quand l'arbre est sale, `-EPREUVE` sinon. Éprouvé
   sur banc : `arbre_sale : non`, `epreuve_motif : provenance NON ÉTABLIE …`,
   artefact `…-EPREUVE.tar.gz`. **La porte, elle, n'a pas bougé** : l'épreuve
   reste non déployable, et l'installateur la refuse toujours (mesuré, exit 2).
2. **L'installateur affirmait la cause à la place du manifeste** (« Il a été
   produit depuis un arbre sale »). Il cite maintenant le motif constaté, et dit
   de ne pas lui substituer une cause supposée.
3. **L'empaqueteur mourait EN SILENCE.** Constaté sur banc : `find` sur un chemin
   absent échoue, et sous `set -e` + `pipefail` cet échec terminait le script
   **exit 1, sans une seule ligne** — indiscernable d'une panne. Deux corrections :
   `|| true` autour des mesures, et une **porte ajoutée** — un périmètre
   introuvable est refusé avec son motif (`exit 3`, mesuré), parce qu'un
   périmètre vide annoncerait « aucune source plus récente » sur n'importe quoi.
4. **Porte ajoutée : le binaire doit être un ELF** (`exit 3`, mesuré sur un
   fichier texte). Sans elle, l'empaqueteur produisait un artefact nommé `selfd`
   contenant n'importe quoi, et la panne n'apparaissait qu'au démarrage du
   service, loin de la cause.

**Aucune porte existante n'a été assouplie** : les quatre corrections ajoutent
une exigence ou rendent un message plus précis. La question du périmètre de la
porte de fraîcheur (§ 12.1, issue 2) est **posée et non tranchée**.

### 12.8 Ce que je n'ai pas fait, et ce que je n'ai pas prouvé

- **Je n'ai produit aucun artefact DÉPLOYABLE** : il faudrait une reconstruction
  release, interdite à cet agent. C'est la seule marche manquante, et elle est
  nommée au § 12.6 étape 0.
- **« Une reconstruction lève la garde » est une DÉDUCTION, pas une mesure** : la
  commande qui le prouverait (`cargo build --release -p obrain-selfd`) est
  interdite ici. Ce qui est mesuré : le binaire est le seul fichier du périmètre
  postérieur à ses sources sauf `Cargo.lock`, et l'empreinte de `obrain-selfd`
  n'a pas été réévaluée par la construction de 17:45.
- **Je n'ai rien redémarré, rien activé, rien déployé** : aucun `systemctl
  start/stop/restart/daemon-reload`, aucun `enable`. Les seuls processus `selfd`
  lancés étaient les miens, sur des socket/état/racine dans `/tmp`, et je les ai
  arrêtés. Le service 8432 et ses voisins n'ont fait l'objet que de **lectures**
  (`systemctl --user show`, `/proc/<pid>/exe`).
- **Je n'ai installé aucune cible rustup** (§ 12.5) et je n'ai pas mesuré la
  glibc de n1/n2/dgx/m4 : la porte distante reste *affirmée, non prouvée sur le
  parc*.
- **Les preuves de banc** (§ 12.7 points 1 et 3) viennent d'un dépôt factice dans
  `/tmp` (`git init`, une copie **octet pour octet** du script, le vrai `selfd`
  en `--binaire`) : c'est un banc pour atteindre des branches que l'arbre réel ne
  permet pas d'atteindre (arbre propre **et** source en avance). Les mesures du
  dépôt réel (§ 12.1, 12.2, 12.3) sont, elles, faites sur `banc-obrain-tronc`.
- **Constaté sans que j'y touche** : `target/release/obrain` a été reconstruit à
  **17:45** (sha256 `ad930298…`) et le service 8432 **a été (re)démarré à
  17:47:37** (`ActiveState=active`, `NRestarts=0`) — il tourne donc sur ce
  binaire-là, et non sur le `70a32f36` du relevé de 14:26. Aucun de ces gestes
  n'est de moi ; je le signale parce que la ligne déployée a changé sous ce
  relevé.

### 12.9 En une phrase

Le binaire existe, il agit (prouvé ce soir sur socket et état dans `/tmp`), la
chaîne d'empaquetage et d'installation tient ses portes — et **ce qui reste entre
l'arbre et une machine n'est plus ni le code ni les scripts : c'est UNE
reconstruction réelle du binaire `selfd`**. Elle était d'abord due à
`Cargo.lock`, commité trois fois après le binaire par `obrain-cli` et
`obrain-indexer` sans que la source de `selfd` bouge d'un octet (refus
défendable, mais discutable sur son périmètre, § 12.1) ; à 18:39, la session
voisine a modifié `crates/obrain-selfd/src/attache.rs`, et le refus est devenu
**incontestable** (§ 12.1.2) : plus aucune reconstruction antérieure à cette
minute ne peut être déployée.

