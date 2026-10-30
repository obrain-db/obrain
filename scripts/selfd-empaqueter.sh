#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# selfd-empaqueter.sh — fait d'un binaire `selfd` DÉJÀ CONSTRUIT un ARTEFACT
# versionné, transférable, et vérifiable sur la machine d'arrivée.
#
# ── POURQUOI IL N'APPELLE JAMAIS `cargo build --release` ────────────────────
# `target/release/` est le répertoire du binaire DÉPLOYÉ, celui que sert le
# service 8432 et celui que `obrain harness-serve` exécute. Un `cargo build
# --release` lancé ici écrase des binaires vivants (POSIX remplace l'inode sous
# un processus en cours — un service à moitié reconstruit est un service qu'on
# ne sait plus décrire). Un empaqueteur n'a donc pas le droit de compiler :
# il CONSTATE. Si le binaire est absent, il le dit et s'arrête — c'est au
# donneur d'ordre de décider quand construire, jamais à un script d'artefact.
#
# ── POURQUOI IL REFUSE UN ARBRE SALE (R2) ───────────────────────────────────
# R2 du protocole de déploiement : « rien ne se déploie qui ne soit commité et
# tagué ». Le constat qui l'a établie : le binaire de prod du 2026-09-01 venait
# d'un arbre à trois fichiers modifiés — irreconstructible, et il contenait le
# panic qui a rendu tout l'historique injoignable. Un artefact dont on ne peut
# pas nommer la source n'est pas un artefact, c'est un objet trouvé.
# L'épreuve locale existe (`--epreuve-arbre-sale`) parce qu'un arbre partagé
# par huit agents parallèles est sale en permanence : elle produit l'artefact,
# mais le NOME, écrit « DEPLOYABLE: non » dans le manifeste, et l'installateur
# refuse de le poser. Un artefact d'épreuve ne peut donc pas être pris pour un
# artefact de déploiement par inadvertance.
#
# ── POURQUOI LE NOM DE L'ÉPREUVE DIT LE VRAI MOTIF (ajouté le 2026-09-16) ───
# Le nom portait `-ARBRE-SALE` dès que `--epreuve-arbre-sale` était passé, et le
# manifeste écrivait « DEPLOYABLE: non — ÉPREUVE (arbre sale, R2) ». Mesuré le
# 2026-09-16 au soir : l'arbre était PROPRE (`git status --porcelain` vide) et
# l'obstacle était ailleurs — `Cargo.lock`, plus récent que le binaire. Le
# manifeste portait donc, sur la même page, `arbre_sale : non` ET « DEPLOYABLE:
# non — ÉPREUVE (arbre sale) » : deux champs qui se contredisent, dans le seul
# document qui existe pour rendre un artefact auditable. Un motif faux n'est pas
# un détail de rédaction : c'est ce qui envoie l'exploitant nettoyer un arbre
# déjà propre pendant que la vraie cause reste en place. Les deux champs sont
# désormais calculés depuis ce qui a été OBSERVÉ, et le nom suit le motif.
#
# ── POURQUOI UNE SOURCE PLUS RÉCENTE NE SE RATTRAPE PAS TOUTE SEULE ─────────
# « Reconstruire le binaire, puis relancer » est le conseil du refus — et il est
# insuffisant quand la source en avance est `Cargo.lock` modifié par un AUTRE
# crate du workspace : cargo recompile alors les crates concernés et laisse
# `selfd` intact (son empreinte de dépendances n'a pas changé). Le binaire garde
# sa date, la garde refuse encore, et l'exploitant recommence. Le refus nomme
# donc les commits qui ont touché ces fichiers, et dit les deux issues
# HONNÊTES : une reconstruction RÉELLE (forcée), ou une décision du donneur
# d'ordre. Le script ne prend ni l'une ni l'autre à sa place.
#
# ── POURQUOI L'ARTEFACT NE SORT PAS DANS LE DÉPÔT ───────────────────────────
# Parce qu'un artefact écrit dans l'arbre le rend SALE, et que le contrôle
# ci-dessus refuserait alors l'empaquetage suivant : le script se bloquerait
# lui-même par son propre produit. La sortie est donc un répertoire HORS du
# dépôt : `${SELFD_DIST:-$HOME/.local/share/obrain-selfd/dist}`. Si l'équipe
# veut un jour `dist-selfd/` dans l'arbre, il faudra l'ignorer dans
# `.gitignore` — ce n'est pas fait ici, ce fichier n'appartient pas à ce
# chantier (voir l'écart consigné dans docs/ETAT-BINAIRE-SELFD-2026-09-16.md).
#
# ── CE QUE LE MANIFESTE PORTE, ET POURQUOI ──────────────────────────────────
# sha256, commit, `git describe`, version de rustc, triple, taille, BuildID,
# édition de liaison, glibc RÉELLEMENT exigée et sortie de `ldd` verbatim.
# Sans lui on ne sait pas d'où l'on vient, donc on ne sait pas où revenir —
# c'est la mécanique anti-retour-en-arrière du protocole, et elle n'existait
# pas pour `selfd` avant ce fichier.
#
# CODES DE SORTIE (un refus est MOTIVÉ : le silence ressemble à une panne)
#   0  artefact produit (ou déjà présent à l'identique)
#   2  refus : arbre sale (R2), ou binaire antérieur à ses propres sources
#   3  refus : binaire absent, non exécutable, ou pas un binaire ELF — ce
#      script ne compile pas, il CONSTATE
#   4  refus : dépendance non résolue localement (`ldd` dit « not found »)
#   5  refus : un artefact DIFFÉRENT porte déjà ce nom (deux binaires ne
#      peuvent pas prétendre au même commit)
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

DEPOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BINAIRE="$DEPOT/target/release/selfd"
# ── LE DÉFAUT EST LE BINAIRE PORTABLE (2026-09-16) ──────────────────────────
# « On doit pouvoir connecter le MÊME partout » : un binaire lié à la glibc de
# la machine de construction ne démarre QUE sur une glibc au moins aussi récente.
# Mesuré en conteneurs : refus sur Debian 11 (2.31), Debian 12 (2.36) et Ubuntu
# 22.04 (2.35) ; le binaire STATIQUE (cible musl) démarre sur les trois, et sur
# Alpine. On le préfère donc dès qu'il existe, et on DIT lequel on a pris : un
# artefact dont on ignore l'édition de liaison est un artefact qu'on croit
# portable à tort.
PREFERENCE="dynamique (glibc) — aucune construction musl trouvée"
if [ -x "$DEPOT/target/x86_64-unknown-linux-musl/release/selfd" ]; then
  BINAIRE="$DEPOT/target/x86_64-unknown-linux-musl/release/selfd"
  PREFERENCE="statique (cible musl) — portatif, aucune dépendance à la glibc"
fi
SORTIE="${SELFD_DIST:-$HOME/.local/share/obrain-selfd/dist}"
EPREUVE=0

usage() {
  # L'aide EST l'en-tête du script : on l'imprime jusqu'à la première ligne qui
  # n'est plus un commentaire. Une aide recopiée à part finirait par diverger du
  # script qu'elle décrit.
  awk 'NR>1 { if ($0 !~ /^#/) exit; sub(/^# ?/, ""); print }' "${BASH_SOURCE[0]}"
  cat <<'AIDE'

USAGE
  scripts/selfd-empaqueter.sh [options]

OPTIONS
  --binaire <chemin>      binaire à empaqueter (défaut : target/release/selfd)
  --sortie <repertoire>   où écrire l'artefact
                          (défaut : $SELFD_DIST, sinon ~/.local/share/obrain-selfd/dist)
  --epreuve-arbre-sale    ÉPREUVE LOCALE : produit l'artefact malgré ce qu'une
                          porte refuse, en le marquant NON DÉPLOYABLE et en
                          nommant le motif RÉEL (arbre sale, provenance non
                          établie, ou les deux) — dans le nom ET le manifeste.
                          Ne jamais utiliser pour poser un binaire sur une
                          machine.
  -h, --help              cette aide
AIDE
  exit 0
}

die() { local code="$1"; shift; echo "selfd-empaqueter : REFUS — $*" >&2; exit "$code"; }

while [ $# -gt 0 ]; do
  case "$1" in
    -h|--help) usage ;;
    --binaire) BINAIRE="${2:?--binaire attend un chemin}"; PREFERENCE="imposé par --binaire" ; shift 2 ;;
    --sortie)  SORTIE="${2:?--sortie attend un répertoire}"; shift 2 ;;
    --epreuve-arbre-sale) EPREUVE=1; shift ;;
    *) echo "selfd-empaqueter : option inconnue : $1 (voir --help)" >&2; exit 2 ;;
  esac
done

# ── 1. LE BINAIRE EXISTE-T-IL ? (on ne compile pas) ─────────────────────────
[ -f "$BINAIRE" ] || die 3 "binaire absent : $BINAIRE
  Ce script EMPAQUETTE, il ne COMPILE pas — et il ne doit pas : un
  \`cargo build --release\` ici écraserait le binaire que sert le service 8432.
  Pour l'obtenir : le construire explicitement, puis relancer ce script."
[ -x "$BINAIRE" ] || die 3 "binaire non exécutable : $BINAIRE"
# AJOUTÉ le 2026-09-16 (« ANNONCER EST DÉCIDER ») : l'artefact ANNONCE un
# exécutable nommé `selfd`. Si ce qu'on met dedans n'est pas un binaire ELF,
# l'artefact est une annonce sans ressource : l'installateur le poserait
# (il ne teste que le sha256 et le démarrage — trop tard), le service
# échouerait, et la cause serait à des kilomètres de l'empaquetage.
case "$(file -b "$BINAIRE")" in
  ELF*) ;;
  *) die 3 "ce n'est pas un binaire ELF : $BINAIRE
    $(file -b "$BINAIRE")
  L'empaqueteur n'emballe pas un fichier qui n'est pas un exécutable : un
  artefact nommé « selfd » doit contenir un selfd." ;;
esac

# ── 2. L'ARBRE EST-IL PROPRE ? (R2) ─────────────────────────────────────────
SALE="$(git -C "$DEPOT" status --porcelain)"
if [ -n "$SALE" ]; then
  echo "selfd-empaqueter : l'arbre de $DEPOT n'est pas propre :" >&2
  echo "$SALE" | sed 's/^/    /' >&2
  if [ "$EPREUVE" -eq 0 ]; then
    die 2 "arbre sale — R2 : « rien ne se déploie qui ne soit commité et tagué ».
  Un artefact produit ici ne serait pas reconstructible depuis un commit : il
  porterait un code que personne ne peut nommer. Commiter (ou remettre de
  côté) d'abord, puis relancer.
  Pour une ÉPREUVE LOCALE seulement : --epreuve-arbre-sale (l'artefact est
  alors marqué NON DÉPLOYABLE et l'installateur refuse de le poser)."
  fi
  echo "selfd-empaqueter : ÉPREUVE — on continue, l'artefact portera ARBRE-SALE" >&2
  echo "selfd-empaqueter : et « DEPLOYABLE: non ». Il ne doit pas être installé." >&2
fi

# ── 3. LE BINAIRE VIENT-IL BIEN DE CES SOURCES ? ────────────────────────────
# Le binaire ne porte pas son commit : `readelf -p .comment` ne rend que la
# version de rustc, et rien ne relie l'exécutable à un arbre (écart consigné
# dans docs/ETAT-BINAIRE-SELFD-2026-09-16.md — un vrai build reproductible ou
# un en-tête de provenance le corrigerait).
# La borne qu'on sait mesurer, elle, est locale et sans réseau : une source de
# `selfd` plus RÉCENTE que le binaire ne peut pas être dedans. C'est exactement
# la forme de l'incident de R2 (binaire produit depuis un arbre modifié, donc
# irreconstructible) — sauf qu'ici on refuse AVANT de produire l'artefact.
# Limite DITE : le périmètre est celui des sources directes et des manifestes ;
# ni la chaîne complète des dépendances de crates.io, ni une garantie de
# reconstruction bit-à-bit. C'est une porte, pas une preuve d'identité.
MTIME_BIN="$(stat -c %Y "$BINAIRE")"

# AJOUTÉ le 2026-09-16 : le périmètre doit EXISTER, et sa mesure ne doit pas
# pouvoir mourir en silence. Constaté sur un banc : `find` sur un chemin absent
# ÉCHOUE, et sous `set -e` + `pipefail` cet échec terminait le script AVANT le
# moindre message — exit 1, aucune ligne. Un refus sans motif est indiscernable
# d'une panne (« le silence ressemble à une panne »), et un périmètre vide ne
# mesure RIEN : il annoncerait « aucune source plus récente que le binaire » sur
# n'importe quoi. Les deux cas sont donc refusés, avec leur motif.
for d in "$DEPOT/crates/obrain-selfd" "$DEPOT/crates/obrain-httpmin"; do
  [ -d "$d" ] || die 3 "périmètre de provenance introuvable : $d
  Sans ce répertoire, la porte de fraîcheur ne mesure RIEN — et une porte qui ne
  mesure rien laisse passer n'importe quel binaire en annonçant « aucune source
  plus récente ». Ce script empaquette un selfd construit dans CE dépôt."
done

RECENTS_CRATES="$( { find "$DEPOT/crates/obrain-selfd" "$DEPOT/crates/obrain-httpmin" \
    -type f -newermt "@${MTIME_BIN}" 2>/dev/null || true; } | sort || true)"
# Les manifestes ne sont pas tous là dans un arbre assemblé à la main
# (`.cargo/config.toml` aujourd'hui suivi, mais rien ne le garantit) : on ne
# passe donc à `find` que ce qui existe. Le périmètre reste NOMMÉ dans le
# message de provenance, pour qu'on sache ce qui a été mesuré.
FICHIERS_MANIFESTES=()
for f in "$DEPOT/Cargo.toml" "$DEPOT/Cargo.lock" "$DEPOT/.cargo/config.toml"; do
  if [ -f "$f" ]; then FICHIERS_MANIFESTES+=("$f"); fi
done
RECENTS_MANIFESTES=""
if [ "${#FICHIERS_MANIFESTES[@]}" -gt 0 ]; then
  RECENTS_MANIFESTES="$( { find "${FICHIERS_MANIFESTES[@]}" -type f -newermt "@${MTIME_BIN}" \
      2>/dev/null || true; } | sort || true)"
fi
PLUS_RECENTS="$(printf '%s\n%s\n' "$RECENTS_CRATES" "$RECENTS_MANIFESTES" | grep -v '^$' || true)"
if [ -n "$PLUS_RECENTS" ]; then
  # Ce qui a CHANGÉ, pas seulement ce qui a une date : c'est la question que se
  # pose l'exploitant devant le refus. `git log` est borné au périmètre nommé,
  # et son échec (dépôt absent, fichier non suivi) ne doit pas masquer le refus.
  COMMITS_APRES="$(git -C "$DEPOT" log --format='      %h %cI %s' \
      --since="@${MTIME_BIN}" -- $(echo "$PLUS_RECENTS" | tr '\n' ' ') 2>/dev/null || true)"
  if [ "$EPREUVE" -eq 0 ]; then
    echo "$PLUS_RECENTS" | sed 's/^/    /' >&2
    [ -n "$COMMITS_APRES" ] && {
      echo "  commits qui ont touché ce périmètre depuis la construction :" >&2
      echo "$COMMITS_APRES" >&2
    }
    die 2 "ces sources sont PLUS RÉCENTES que le binaire (construit
  $(date -u -d "@${MTIME_BIN}" +%Y-%m-%dT%H:%M:%SZ)) : le binaire ne peut pas contenir leur
  code. Un artefact produit ici serait irreconstructible — le mode de panne que
  R2 interdit.
  ATTENTION — « reconstruire, puis relancer » ne suffit pas toujours : si le
  fichier en avance appartient à un AUTRE crate du workspace (c'est le cas de
  Cargo.lock touché par obrain-cli ou obrain-indexer), cargo recompile ce crate
  et laisse \`selfd\` intact — son empreinte de dépendances n'a pas changé. Il
  affiche « Finished » sans avoir rien refait, le binaire garde sa date, et ce
  refus revient à l'identique. Deux issues HONNÊTES, et c'est une décision :
    (a) forcer une reconstruction RÉELLE du binaire :
          cargo clean -p obrain-selfd && cargo build --release -p obrain-selfd
        (le binaire relinké porte alors la date du jour, donc postérieure à
        toutes ses entrées : la porte passe pour de vrai) ;
    (b) décider que le périmètre de cette porte (Cargo.lock ENTIER) est trop
        large et le restreindre au graphe de dépendances de \`selfd\`. Ce
        script ne le fait pas tout seul : ce serait relâcher la porte qui a
        établi R2, et cela se décide, cela ne se subit pas."
  fi
  echo "selfd-empaqueter : ATTENTION — sources plus récentes que le binaire :" >&2
  echo "$PLUS_RECENTS" | sed 's/^/    /' >&2
  echo "selfd-empaqueter : l'épreuve continue, mais la provenance sera écrite NON ÉTABLIE" >&2
  PROVENANCE="NON ÉTABLIE — ces sources sont plus récentes que le binaire :
$(echo "$PLUS_RECENTS" | sed 's/^/  /')"
else
  echo "selfd-empaqueter : provenance — aucune source de selfd plus récente que le binaire"
  echo "                   (périmètre : crates/obrain-selfd, crates/obrain-httpmin,"
  echo "                    Cargo.toml, Cargo.lock ; le binaire ne porte pas son commit)"
  PROVENANCE="établie par CONTENU sur ce périmètre : aucune source de selfd plus récente
que le binaire. Ce n'est pas une preuve de reconstruction bit-à-bit — le binaire
ne porte pas son commit."
fi

# ── 4. LES FAITS DURS DU BINAIRE ────────────────────────────────────────────
SHA="$(sha256sum "$BINAIRE" | cut -d' ' -f1)"
TAILLE="$(stat -c %s "$BINAIRE")"
COMMIT="$(git -C "$DEPOT" rev-parse HEAD)"
COMMIT_COURT="$(git -C "$DEPOT" rev-parse --short HEAD)"
BRANCHE="$(git -C "$DEPOT" rev-parse --abbrev-ref HEAD)"
DESCRIBE="$(git -C "$DEPOT" describe --tags --always 2>/dev/null || echo '(aucun tag)')"
FILETYPE="$(file -b "$BINAIRE")"
BUILDID="$(readelf -n "$BINAIRE" 2>/dev/null | awk '/Build ID:/ {print $3; exit}')"
[ -n "$BUILDID" ] || BUILDID="(absent)"
RUSTC_EMPRINTE="$(readelf -p .comment "$BINAIRE" 2>/dev/null | grep -m1 'rustc version' | sed 's/^ *\[ *[0-9a-f]*\] *//' || true)"
[ -n "$RUSTC_EMPRINTE" ] || RUSTC_EMPRINTE="(le binaire ne porte pas de section .comment)"
RUSTC_LOCAL="$( { rustc -vV 2>/dev/null | head -1; } || true )"
[ -n "$RUSTC_LOCAL" ] || RUSTC_LOCAL="(rustc absent de cette machine)"

# Architecture et édition de liaison : lues, pas supposées.
ARCH_OBJDUMP="$(objdump -f "$BINAIRE" | awk '/architecture:/ {print $2; exit}' | tr -d ',')"
case "$FILETYPE" in
  *"dynamically linked"*) EDITION="dynamique (interpréteur $(file -b "$BINAIRE" | grep -o '/[^,]*ld-linux[^,]*' | head -1))" ; LIBC="gnu" ;;
  # ── `static-pie` EST UN STATIQUE, ET LE MOTIF DOIT LE RECONNAÎTRE ─────────
  # Rust produit un binaire static-PIE pour la cible musl : `file` écrit alors
  # « static-pie linked », que le motif « statically linked » NE MATCHE PAS. Le
  # premier artefact statique a donc reçu le triple `x86_64-unknown-linux-inconnu`
  # — un artefact PORTABLE portant un nom FAUX, et une édition déclarée
  # « inconnue » alors qu'elle est lisible. On reconnaît les deux formes.
  *"static-pie linked"*|*"statically linked"*) EDITION="statique" ; LIBC="musl" ;;
  *)                      EDITION="inconnue" ; LIBC="inconnu" ;;
esac
case "$ARCH_OBJDUMP" in
  i386:x86-64) ARCH="x86_64" ;;
  *)           ARCH="$ARCH_OBJDUMP" ;;
esac
TRIPLE="${ARCH}-unknown-linux-${LIBC}"
if rustup target list --installed 2>/dev/null | grep -qx "$TRIPLE"; then
  CIBLE_INSTALLEE="oui"
else
  CIBLE_INSTALLEE="non (ce triple n'est pas installé ici : l'artefact a été
lu, pas produit — vérifier sur la machine de construction)"
fi

# glibc RÉELLEMENT exigée. Deux chiffres, parce qu'ils ne disent pas la même
# chose : le maximum FORT est ce que le binaire exige par ses symboles réels ;
# le maximum tout court inclut les références FAIBLES (`pidfd_spawnp`,
# `pidfd_getpid` — marquées `w` par `objdump -T`). Mesure du 2026-09-16 : ces
# références faibles suffisent à écrire une exigence GLIBC_2.39 dans l'en-tête
# VERNEED, et `ld.so` refuse le binaire ENTIER sur une glibc plus ancienne,
# même si les symboles sont faibles. Le chiffre qui décide du déploiement est
# donc le maximum tout court.
GLIBC_MAX="$(objdump -T "$BINAIRE" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1 || true)"
GLIBC_FORT="$(objdump -T "$BINAIRE" | grep -vE '^[0-9a-f]+ +w +' | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1 || true)"
# `|| true` N'EST PAS DÉCORATIF : `objdump -T` ÉCHOUE sur un binaire statique (il
# n'y a pas de table dynamique), et sous `set -e` + `pipefail` une substitution
# de commande qui échoue fait SORTIR le script — sans un mot. C'est ce qui est
# arrivé au premier artefact statique : la sortie s'arrêtait net à cette ligne.
# Les deux voisines avaient leur `|| true` ; ces deux-ci ne l'avaient pas.
GLIBC_TOP="$(objdump -T "$BINAIRE" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -3 | tr '\n' ' ' || true)"
GLIBC_FAIBLES="$(objdump -T "$BINAIRE" 2>/dev/null | awk '$2=="w"' | grep -F "($GLIBC_MAX)" | awk '{print $NF}' | sort -u | tr '\n' ' ' || true)"

# ── UN BINAIRE STATIQUE N'EXIGE AUCUNE GLIBC, ET LE CHAMP DOIT LE DIRE ──────
# Sur un binaire statique, `objdump -T` ne rend AUCUN symbole `GLIBC_*` : le
# champ resterait VIDE, et un champ vide se lit comme « on n'a pas mesuré » —
# alors que la mesure, ici, est « aucune dépendance ». On écrit donc le mot, et
# l'installateur saura qu'il n'y a RIEN à comparer (il ne doit surtout pas
# comparer la chaîne « aucune » à un numéro de version : il refuserait à tort).
if [ -z "$GLIBC_MAX" ] && [ "$LIBC" = "musl" ]; then
  GLIBC_MAX="aucune"
  GLIBC_FORT="aucune"
  GLIBC_TOP="aucune (binaire statique : aucune dépendance à la glibc)"
  PORTABLE="oui — statique : démarre sur toute distribution x86-64, sans dépendre de la glibc de la machine de construction"
else
  PORTABLE="NON — exige glibc >= ${GLIBC_MAX:-inconnue} : ne démarrera pas sur une distribution plus ancienne. Pour un artefact portable : cargo build --release --target x86_64-unknown-linux-musl -p obrain-selfd"
fi

LDD="$(ldd "$BINAIRE" 2>&1 || true)"
grep -q 'not found' <<<"$LDD" && die 4 "dépendance non résolue sur CETTE machine :
$(grep 'not found' <<<"$LDD" | sed 's/^/    /')
  Un artefact dont les bibliothèques ne se résolvent pas ici ne se résoudra pas
  mieux ailleurs. Poser la bibliothèque manquante d'abord."

# ── CE QUE LE BINAIRE DÉCLARE AVOIR BESOIN, NOMMÉMENT ───────────────────────
# L'installateur vérifiait `libgcc_s.so.1` EN DUR — une hypothèse de l'ère glibc.
# Un binaire STATIQUE ne déclare AUCUNE bibliothèque partagée : la porte aurait
# donc refusé un artefact portable en affirmant qu'il déclare ce qu'il ne déclare
# pas. Le manifeste porte désormais la liste RÉELLE des NEEDED, et l'installateur
# la lit au lieu de la supposer.
NEEDED="$(readelf -d "$BINAIRE" 2>/dev/null | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p' | sort -u | tr '\n' ' ')"
NEEDED="${NEEDED% }"
# `if` et non `[ ] && …` : sous `set -e`, une condition fausse en fin de ligne
# fait SORTIR le script. C'est exactement le piège dans lequel je suis tombé en
# écrivant ce fichier — la sortie s'arrêtait net, sans message.
if [ -z "$NEEDED" ]; then NEEDED="aucune"; fi

NOM="selfd-${COMMIT_COURT}-${TRIPLE}"
# Le suffixe dit ce qui est VRAI (voir l'en-tête). `-ARBRE-SALE` reste le nom de
# l'épreuve quand l'arbre EST sale ; `-EPREUVE` couvre les autres causes — dont
# la provenance non établie, qui était le cas réel le 2026-09-16 au soir.
MOTIF_EPREUVE=""
if [ "$EPREUVE" -eq 1 ]; then
  [ -n "$SALE" ] && MOTIF_EPREUVE="arbre sale : $(echo "$SALE" | wc -l) fichier(s) non commité(s) (R2)"
  case "$PROVENANCE" in
    NON\ ÉTABLIE*) MOTIF_EPREUVE="${MOTIF_EPREUVE:+$MOTIF_EPREUVE ; }provenance NON ÉTABLIE — sources plus récentes que le binaire" ;;
  esac
  [ -n "$MOTIF_EPREUVE" ] || MOTIF_EPREUVE="épreuve demandée explicitement, aucun refus constaté"
  if [ -n "$SALE" ]; then NOM="${NOM}-ARBRE-SALE"; else NOM="${NOM}-EPREUVE"; fi
fi
ARTEFACT="$SORTIE/$NOM.tar.gz"
MANIFESTE="MANIFEST-${NOM}.txt"
MANIFESTE_SORTIE="$SORTIE/$NOM.manifest.txt"

# ── 5. UN MÊME NOM POUR DEUX BINAIRES DIFFÉRENTS EST UN MENSONGE ────────────
if [ -f "$ARTEFACT" ]; then
  SHA_DEDANS="$(tar -xzOf "$ARTEFACT" selfd 2>/dev/null | sha256sum | cut -d' ' -f1)"
  if [ "$SHA_DEDANS" = "$SHA" ]; then
    echo "selfd-empaqueter : artefact déjà présent et IDENTIQUE : $ARTEFACT"
    echo "selfd-empaqueter : rien à faire (sha256 $SHA)."
    exit 0
  fi
  die 5 "un artefact DIFFÉRENT porte déjà $ARTEFACT
    dedans : $SHA_DEDANS
    ici    : $SHA
  Deux binaires ne peuvent pas prétendre au même nom. Vérifier d'où vient
  l'ancien avant de l'écraser : c'est peut-être celui qui tourne."
fi

mkdir -p "$SORTIE"
ETAPE="$(mktemp -d "${TMPDIR:-/tmp}/selfd-empaqueter.XXXXXX")"
trap 'rm -rf "$ETAPE"' EXIT

cp -p "$BINAIRE" "$ETAPE/selfd"
chmod 0755 "$ETAPE/selfd"

if [ "$EPREUVE" -eq 1 ]; then
  DEPLOYABLE="non — ÉPREUVE : $MOTIF_EPREUVE. Artefact de vérification de la chaîne, pas d'installation"
else
  DEPLOYABLE="oui — arbre propre, artefact versionné « $DESCRIBE »"
fi

{
  echo "# MANIFESTE DE BINAIRE — selfd"
  echo "# Écrit par scripts/selfd-empaqueter.sh. Il dit D'OÙ VIENT ce binaire,"
  echo "# et donc vers quoi on peut revenir (protocole de déploiement, G1/G5)."
  echo "paquet                   : $NOM.tar.gz"
  echo "empaquete_le             : $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "empaquete_sur            : $(cat /proc/sys/kernel/hostname 2>/dev/null || echo inconnu)"
  echo "binaire                  : selfd"
  # ── LE CHEMIN DE LA SOURCE, PAS SEULEMENT SON NOM DANS L'ARCHIVE ──────────
  # `binaire` dit ce que l'archive CONTIENT ; sans le chemin, personne ne peut
  # revérifier l'artefact contre la construction dont il vient — un manifeste
  # qu'on ne peut pas confronter à sa source est une décoration. La porte
  # d'atelier relit ce champ pour vérifier que l'artefact correspond bien à la
  # construction COURANTE (et non à une édition supposée).
  echo "binaire_source           : $BINAIRE"
  echo "binaire_preference       : $PREFERENCE"
  echo "sha256                   : $SHA"
  echo "taille_octets            : $TAILLE"
  echo "commit                   : $COMMIT"
  echo "commit_court             : $COMMIT_COURT"
  echo "branche                  : $BRANCHE"
  echo "git_describe             : $DESCRIBE"
  echo "arbre_sale               : $([ -n "$SALE" ] && echo "oui ($(echo "$SALE" | wc -l) fichier(s))" || echo non)"
  echo "empaquetage              : $([ "$EPREUVE" -eq 1 ] && echo "ÉPREUVE (--epreuve-arbre-sale)" || echo normal)"
  [ "$EPREUVE" -eq 1 ] && echo "epreuve_motif            : $MOTIF_EPREUVE"
  echo "provenance               : $(echo "$PROVENANCE" | head -1)"
  [ "$(echo "$PROVENANCE" | wc -l)" -gt 1 ] && echo "$PROVENANCE" | tail -n +2 | sed 's/^/                           /'
  echo "triple                   : $TRIPLE"
  echo "triple_installe_ici      : $CIBLE_INSTALLEE"
  echo "rustc_empreinte_binaire  : $RUSTC_EMPRINTE"
  echo "rustc_local              : $RUSTC_LOCAL"
  echo "buildid                  : $BUILDID"
  echo "file                     : $FILETYPE"
  echo "edition_liaison          : $EDITION"
  echo "portable                 : $PORTABLE"
  echo "glibc_exigee_max         : $GLIBC_MAX"
  echo "glibc_exigee_forte_max   : $GLIBC_FORT"
  echo "glibc_top3               : $GLIBC_TOP"
  echo "glibc_faibles_max        : ${GLIBC_FAIBLES:-aucun}"
  echo "# glibc_exigee_max est le chiffre qui DÉCIDE du déploiement. Les valeurs"
  echo "# ci-dessus sont lisibles par un script (l'installateur les relit) :"
  echo "# rien après elles sur la ligne, aucune annotation en fin de valeur."
  echo "# glibc_exigee_forte_max ne compte que les symboles NON faibles ; les"
  echo "# références faibles listées ci-dessus suffisent pourtant à écrire"
  echo "# l'exigence dans l'en-tête VERNEED, et ld.so refuse alors le binaire"
  echo "# ENTIER sur une glibc plus ancienne — mesuré le 2026-09-16 dans des"
  echo "# conteneurs (Debian 12 → 2.36 : refus ; Ubuntu 24.04 → 2.39 : démarre)."
  echo "ldd                      : |"
  echo "$LDD" | sed 's/^/  /'
  echo "needed                   : $NEEDED"
  echo "DEPLOYABLE               : $DEPLOYABLE"
} > "$ETAPE/$MANIFESTE"

tar -czf "$ARTEFACT" -C "$ETAPE" selfd "$MANIFESTE"
sha256sum "$ARTEFACT" | sed "s|  .*|  $(basename "$ARTEFACT")|" > "$ARTEFACT.sha256"
cp -p "$ETAPE/$MANIFESTE" "$MANIFESTE_SORTIE"

# ── 6. LA PORTE : L'ARTEFACT SE RELIT-IL COMME LE BINAIRE D'ORIGINE ? ───────
# Une archive corrompue au transfert ne se voit qu'à l'arrivée, sur l'autre
# machine, une fois le binaire posé. On la relit donc ICI, avant de la déclarer
# bonne : `tar -xzO` rend le fichier tel qu'il sera extrait.
SHA_RELU="$(tar -xzOf "$ARTEFACT" selfd | sha256sum | cut -d' ' -f1)"
[ "$SHA_RELU" = "$SHA" ] || die 4 "l'artefact relu ne correspond pas au binaire :
    attendu $SHA
    relu    $SHA_RELU"
tar -tzf "$ARTEFACT" >/dev/null || die 4 "l'archive ne se liste pas : $ARTEFACT"

echo "selfd-empaqueter : artefact produit"
echo "  artefact  : $ARTEFACT"
echo "  manifeste : $MANIFESTE_SORTIE"
echo "  binaire   : $BINAIRE"
echo "              ($PREFERENCE)"
echo "  sha256    : $SHA  ($TAILLE octets)"
echo "  provenance: commit $COMMIT_COURT (« $DESCRIBE »), rustc local : $RUSTC_LOCAL"
echo "  liaison   : $EDITION, glibc exigée $GLIBC_MAX"
echo "  portable  : $PORTABLE"
echo "  relu      : le binaire extrait de l'archive a le même sha256 (porte franchie)"
echo "  DEPLOYABLE: $([ "$EPREUVE" -eq 1 ] && echo NON || echo oui)"
echo
echo "Machine d'arrivée — l'installateur, qui VÉRIFIE la glibc avant de copier :"
echo "  scripts/selfd-installer.sh <hote|local> --artefact $ARTEFACT --racine <repertoire>"
