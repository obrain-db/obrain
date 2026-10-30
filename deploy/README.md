# `deploy/` — poser un nœud `selfd` sur une machine

Quatre fichiers, une chaîne. Aucun d'eux ne compile quoi que ce soit, aucun ne
redémarre un service, et aucun n'active une unité sans qu'on le lui demande.

```text
deploy/systemd/selfd.service            l'unité --user du démon local (socket Unix, aucun port)
deploy/systemd/selfd.attacher.service   l'unité --user du rattachement sortant au moteur
docs/ETAT-BINAIRE-SELFD-2026-09-16.md   l'état mesuré du binaire, et ses limites
```

Les deux scripts vivent dans `scripts/` :

```text
scripts/selfd-empaqueter.sh   fait d'un binaire DÉJÀ CONSTRUIT un artefact versionné + manifeste
scripts/selfd-installer.sh    pose cet artefact sur UNE machine, locale ou par ssh
```

## La chaîne, en cinq commandes

```bash
# 1. le binaire est construit EXPLICITEMENT (jamais par l'empaqueteur)
cargo build --release -p obrain-selfd
#   s'il répond « Finished » alors que l'empaqueteur refuse encore, il n'a PAS
#   relinké : voir « la reconstitution du binaire » plus bas.

# 2. l'artefact — refuse un arbre sale (R2) ET une source plus récente que le
#    binaire ; ne compile pas ; écrit hors du dépôt
bash scripts/selfd-empaqueter.sh
#   → ~/.local/share/obrain-selfd/dist/selfd-<commit>-<triple>.tar.gz
#     .manifest.txt  (sha256, commit, describe, rustc, glibc exigée, ldd)
#     .sha256

# 3. mesurer la cible AVANT de copier (l'installateur le refait, et refuse)
ssh <hote> 'uname -m; ldd --version | head -1'

# 4. installer (rien ne démarre : l'activation est un acte séparé)
bash scripts/selfd-installer.sh <hote|local> \
  --artefact <artefact.tar.gz> \
  --racine <repertoire-du-noeud> \
  --moteur http://<moteur>:8432
#   ajouter --allow-exec UNIQUEMENT si le nœud doit exécuter des commandes

# 5. activer, puis rattacher (le jeton est celui de VOTRE compte sur le 8432)
systemctl --user daemon-reload && systemctl --user enable --now selfd
selfd attacher --moteur http://<moteur>:8432 --jeton-moteur <jeton> --machine <nom>
```

## Les règles tenues par ces fichiers

- **R2 — rien ne se déploie qui ne soit commité.** L'empaqueteur refuse un arbre
  sale. L'artefact porte son commit, son sha256 et son `git describe` ; le
  manifeste est copié à côté du binaire installé, donc *le binaire installé porte
  son commit*.
- **R4 — on refuse avant, on ne rattrape pas après.** L'installateur refuse :
  un artefact marqué non déployable, une glibc de cible trop ancienne (en citant
  les deux chiffres), une `libgcc_s.so.1` absente, un transfert dont l'empreinte
  a changé, un nœud qui ne rend pas la politique demandée.
- **R5 — les portes testent la fonction.** Après la pose, le nœud est exécuté
  une fois avec SA racine et SA politique : `exec AUTORISÉ` et `exec refusé` sont
  distingués, et l'installation échoue si ce que le nœud applique n'est pas ce
  qui a été décidé.
- **R6 — aucun outil ne redémarre le service dont il fait partie.** Rien n'est
  activé par défaut : les commandes sont affichées, l'installateur humain décide.
- **R3 — une configuration, une source.** L'unité posée est RENDUE depuis le
  modèle du dépôt ; l'éditer à la main sur une machine est une dérive.

## Ce qui n'est PAS dans cette chaîne, et qu'il faut savoir

- **la glibc** : le binaire actuel exige `GLIBC_2.39` (Ubuntu 24.04). Il ne
  démarre ni sur Debian 12 (2.36), ni sur Ubuntu 22.04 (2.35), ni sur Alpine
  (musl). **Aujourd'hui, la seule machine mesurée qui peut le recevoir est n3
  (Ubuntu 24.04)** ; élargir passe par `x86_64-unknown-linux-musl` — proposée par
  la chaîne d'outils, **non installée** sur n3 — ou par une construction sur la
  plus vieille distribution visée. Mesures, causes et solutions dans
  `docs/ETAT-BINAIRE-SELFD-2026-09-16.md` (§ 1.1 et § 12.5).
- **la reconstitution du binaire — LE point qui bloque aujourd'hui.** Le binaire
  de `target/release/selfd` est **postérieur à ses propres sources**, mais
  `Cargo.lock` a été commité trois fois depuis sa construction (par `obrain-cli`
  et `obrain-indexer`) : l'empaqueteur refuse (R2, exit 2). Piège mesuré : si
  `cargo` n'a rien à refaire, il affiche `Finished` **sans relinker**, le binaire
  garde sa date, et le refus revient à l'identique — forcer un relink RÉEL :

  ```bash
  cargo clean -p obrain-selfd && cargo build --release -p obrain-selfd
  ```

  Jamais sans `-p` : ce serait écraser l'`obrain` que sert le 8432. Le refus de
  l'empaqueteur nomme désormais les commits fautifs et ces deux issues.
- **l'artefact d'épreuve** (`--epreuve-arbre-sale`) est produit malgré une porte,
  et il porte le motif **réel** de son refus : `-ARBRE-SALE` quand l'arbre est
  sale, `-EPREUVE` quand la cause est autre (provenance non établie). Champ
  `epreuve_motif` dans le manifeste, `DEPLOYABLE: non` dans tous les cas, et
  l'installateur le refuse — sauf en cible `local` avec le drapeau d'épreuve.
  Il n'est **jamais** déployable, même quand l'arbre est propre.
- **l'activation** : elle appartient à l'exploitant de la machine. Ces fichiers
  ne l'exécutent pas, même quand `--activer` est passé pour une cible distante —
  c'est la même décision, prise explicitement.

## Vérifier qu'un nœud vit, et qu'il agit

```bash
selfd status                                    # parle à la socket : la politique RÉELLE
selfd --call read '{"path":"<fichier de la racine>"}'   # il agit : contenu rendu
systemctl --user show selfd -p NRestarts        # doit être INCHANGÉ après 60 s
journalctl --user -u selfd -n 20
```

`is-active` répond `active` en plein crash-loop (constat du 2026-09-01 :
`NRestarts` est monté à 9 sans que systemd renonce), et `HTTP 200` ne prouve pas
la fonction. Les deux seuls témoins qui comptent sont le compteur de
redémarrages et un appel qui rend quelque chose.
