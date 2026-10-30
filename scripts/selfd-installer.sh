#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# selfd-installer.sh — pose le nœud `selfd` sur UNE machine, à partir d'un
# artefact produit par `scripts/selfd-empaqueter.sh`, et rend la commande
# EXACTE qui rattache cette machine au moteur.
#
# ── CE QUE CE SCRIPT REFUSE DE FAIRE, ET POURQUOI ───────────────────────────
#
# 1. IL REFUSE UN ARTEFACT MARQUÉ NON DÉPLOYABLE.
#    Un artefact produit sur un arbre sale (R2) porte « DEPLOYABLE: non » dans
#    son manifeste, et l'installateur le lit AVANT de copier quoi que ce soit.
#    Le contournement existe — `--epreuve-arbre-sale` — mais il est bridé : il
#    n'accepte QUE la cible `local`. Un artefact d'épreuve ne peut donc pas
#    partir vers une autre machine, même par inadvertance.
#
# 2. IL VÉRIFIE LA GLIBC DE LA CIBLE AVANT DE COPIER.
#    Mesure du 2026-09-16 : le binaire de n3 exige GLIBC_2.39 et ne démarre NI
#    sur Debian 12 (2.36), NI sur Ubuntu 22.04 (2.35), NI sur Debian 11 (2.31),
#    NI sur Alpine (musl) — `/n/selfd: version 'GLIBC_2.39' not found`. Un
#    binaire copié qui ne démarre pas laisse un service en échec et un
#    opérateur qui cherche une panne de configuration : c'est exactement ce
#    qu'une porte doit empêcher AVANT (R4 : « on refuse avant, on ne rattrape
#    pas après »). L'exigence est lue dans le manifeste, comparée à la glibc
#    réellement installée sur la cible, et le refus CITE les deux chiffres.
#
# 3. IL NE DÉMARRE RIEN PAR DÉFAUT.
#    Il pose le binaire, la racine, l'unité et la politique — puis il AFFICHE
#    les commandes qui activent, et s'arrête. `--activer` est un acte explicite
#    de l'installateur humain. R6 du protocole : aucun outil ne redémarre le
#    service dont il fait partie — et ici, aucun outil ne démarre un service
#    qu'un humain n'a pas décidé de démarrer.
#
# 4. IL N'ÉLARGIT AUCUNE POLITIQUE PAR DÉFAUT.
#    `--racine` est OBLIGATOIRE : sans choix explicite, il n'y a pas de racine.
#    `--allow-exec` est ABSENT par défaut : sans lui le nœud observe et n'agit
#    pas. Rien de ce qu'un client enverra ne pourra l'élargir — c'est la
#    propriété que toute la chaîne existe pour tenir (politique.rs : « l'absence
#    de décision vaut refus »).
#
# 5. IL NE MET AUCUN JETON DANS UNE LIGNE DE COMMANDE.
#    Le jeton du moteur est écrit dans `~/.config/selfd/attacher.env` (0600) et
#    lu par `selfd` depuis `$SELFD_MOTEUR_TOKEN` : un argument se lit dans `ps`
#    par tout utilisateur de la machine. La commande de rattachement affichée
#    est la forme littérale demandée, à côté de la forme recommandée.
#
# CODES DE SORTIE
#   0  installé (ou déjà à l'identique) ; 2 refus motivé (artefact, cible) ;
#   3 empreinte non conforme ; 4 transfert non vérifié ; 5 argument manquant
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

DEPOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODELE_UNITE="$DEPOT/deploy/systemd/selfd.service"
MODELE_ATTACHE="$DEPOT/deploy/systemd/selfd.attacher.service"

CIBLE=""
ARTEFACT=""
RACINE=""
PREFIX=""
UNITE_DIR=""
MOTEUR=""
JETON=""
MACHINE_NOM=""
ALLOW_EXEC=0
ACTIVER=0
EPREUVE=0

usage() {
  # L'aide EST l'en-tête du script : on l'imprime jusqu'à la première ligne qui
  # n'est plus un commentaire (une aide recopiée à part divergerait).
  awk 'NR>1 { if ($0 !~ /^#/) exit; sub(/^# ?/, ""); print }' "${BASH_SOURCE[0]}"
  cat <<'AIDE'

USAGE
  scripts/selfd-installer.sh <hote|local> --artefact <fichier.tar.gz> --racine <repertoire> [options]

  <hote>   nom ssh de la machine cible (doit accepter une clé : BatchMode=yes),
           ou `local` pour la machine où l'on se trouve.

OPTIONS
  --artefact <f>        artefact produit par selfd-empaqueter.sh   (obligatoire)
  --racine <dir>        racine du nœud sur la cible : aucun chemin n'en sort
                        (obligatoire — pas de racine par défaut, c'est une
                        décision, pas un héritage)
  --allow-exec          AUTORISE l'exécution de commandes sur la cible.
                        Sans ce drapeau, le nœud observe (status/read/list) et
                        n'exécute RIEN. C'est un acte : il se voit dans l'unité.
  --moteur <url>        moteur obrain à servir (défaut : http://127.0.0.1:8432).
                        Pour une autre machine : http://<ip-du-moteur>:8432
  --jeton-moteur <j>    jeton IAM de VOTRE compte, écrit dans attacher.env (0600)
  --machine <nom>       nom de cette machine chez votre compte (défaut : hostname)
  --prefix <dir>        préfixe d'installation sur la cible (défaut : ~/.local)
  --unite-dir <dir>     où poser l'unité (défaut : ~/.config/systemd/user)
  --activer             lance `systemctl --user daemon-reload` et
                        `enable --now` sur la CIBLE. Par défaut : rien n'est
                        activé, les commandes sont affichées.
  --epreuve-arbre-sale  accepte un artefact marqué NON DÉPLOYABLE (ÉPREUVE).
                        Refusé pour toute cible autre que `local`.
  -h, --help            cette aide
AIDE
  exit 0
}

die() { local code="$1"; shift; echo "selfd-installer : REFUS — $*" >&2; exit "$code"; }
etape() { echo; echo "── $*"; }

while [ $# -gt 0 ]; do
  case "$1" in
    -h|--help) usage ;;
    --artefact) ARTEFACT="${2:?--artefact attend un fichier}"; shift 2 ;;
    --racine)   RACINE="${2:?--racine attend un répertoire}"; shift 2 ;;
    --prefix)   PREFIX="${2:?--prefix attend un répertoire}"; shift 2 ;;
    --unite-dir) UNITE_DIR="${2:?--unite-dir attend un répertoire}"; shift 2 ;;
    --moteur)   MOTEUR="${2:?--moteur attend une URL}"; shift 2 ;;
    --jeton-moteur) JETON="${2:?--jeton-moteur attend un secret}"; shift 2 ;;
    --machine)  MACHINE_NOM="${2:?--machine attend un nom}"; shift 2 ;;
    --allow-exec) ALLOW_EXEC=1; shift ;;
    --activer)  ACTIVER=1; shift ;;
    --epreuve-arbre-sale) EPREUVE=1; shift ;;
    -*) echo "selfd-installer : option inconnue : $1 (voir --help)" >&2; exit 2 ;;
    *)  if [ -z "$CIBLE" ]; then CIBLE="$1"; else echo "selfd-installer : cible déjà donnée ($CIBLE)" >&2; exit 2; fi; shift ;;
  esac
done

[ -n "$CIBLE" ] || die 5 "la cible manque : « local » ou un nom ssh.
  Exemple : scripts/selfd-installer.sh n2 --artefact … --racine ~/nœud"
[ -n "$ARTEFACT" ] || die 5 "--artefact manque : ce script n'invente pas de binaire."
[ -n "$RACINE" ] || die 5 "--racine manque. Il n'y a PAS de racine par défaut : un nœud
  dont la racine est un héritage est un nœud qui observe ce que personne n'a
  choisi. Décider, puis relancer (ex. --racine ~/selfd-racine)."
case "$RACINE" in
  /) die 2 "--racine / : ce serait la machine entière. Un nœud confiné dans / n'est
  pas un nœud confiné — c'est un accès root déguisé en politique." ;;
esac
[ "$EPREUVE" -eq 1 ] && [ "$CIBLE" != "local" ] && die 2 "--epreuve-arbre-sale n'accepte que la cible « local ».
  Un artefact d'épreuve ne doit pas partir vers une autre machine : c'est
  précisément ce que le marquage « DEPLOYABLE: non » existe pour empêcher."

# ── L'ACCÈS À LA CIBLE, EN UN SEUL ENDROIT ──────────────────────────────────
# `BatchMode=yes` : deux fois plutôt qu'une. Sans lui, un `ssh` sans clé attend
# une saisie de mot de passe dans un script non interactif — et un déploiement
# qui attend un clavier ressemble à un déploiement qui a échoué, sans le dire.
si_cible() {
  if [ "$CIBLE" = "local" ]; then bash -c "$1"; else
    ssh -o BatchMode=yes -o ConnectTimeout=10 "$CIBLE" "$1"
  fi
}
vers_cible() { # envoie sur stdin vers un chemin
  if [ "$CIBLE" = "local" ]; then cat > "$1"; else
    ssh -o BatchMode=yes -o ConnectTimeout=10 "$CIBLE" "cat > '$1'"
  fi
}
MSGFILE="$(mktemp "${TMPDIR:-/tmp}/selfd-installer.XXXXXX")"
trap 'rm -f "$MSGFILE"' EXIT

# ── 0. LA CIBLE EXISTE-T-ELLE, ET EST-ELLE JOIGNABLE ? ──────────────────────
# « ANNONCER EST DÉCIDER » : on ne pose rien sur une machine qu'on n'a pas
# jointe. Un `ssh` qui échoue doit arrêter ici, pas au milieu d'une copie.
etape "0. joignabilité de la cible « $CIBLE »"
if ! si_cible 'uname -m' > "$MSGFILE" 2>&1; then
  cat "$MSGFILE" >&2
  die 2 "la cible « $CIBLE » n'est pas joignable (ou la clé est refusée).
  Rien n'a été copié, rien n'a été modifié. C'est l'état constaté pour n1 le
  2026-09-15 (clé refusée) : on ne déploie pas ce qu'on ne peut pas joindre."
fi
echo "   machine      : $CIBLE"
ARCH_CIBLE="$(cat "$MSGFILE" | tr -d '\r\n')"
echo "   architecture : $ARCH_CIBLE"
GLIBC_CIBLE="$(si_cible 'ldd --version 2>/dev/null | head -1' | grep -oE '[0-9]+\.[0-9]+$' | head -1 || true)"
[ -n "$GLIBC_CIBLE" ] || GLIBC_CIBLE="(inconnue)"
echo "   glibc        : $GLIBC_CIBLE"

# ── 1. L'ARTEFACT : CE QU'IL EST, ET S'IL A LE DROIT D'ÊTRE POSÉ ────────────
etape "1. artefact"
[ -f "$ARTEFACT" ] || die 5 "artefact introuvable : $ARTEFACT"
SHA_ARCHIVE="$(sha256sum "$ARTEFACT" | cut -d' ' -f1)"
echo "   fichier    : $ARTEFACT"
echo "   sha256     : $SHA_ARCHIVE"
if [ -f "$ARTEFACT.sha256" ]; then
  SHA_ATTENDU="$(cut -d' ' -f1 < "$ARTEFACT.sha256")"
  [ "$SHA_ATTENDU" = "$SHA_ARCHIVE" ] || die 3 "l'empreinte de l'archive ne correspond pas :
    annoncée : $SHA_ATTENDU
    mesurée  : $SHA_ARCHIVE
  L'archive a été modifiée ou corrompue après empaquetage. On ne l'installe pas."
  echo "   empreinte  : conforme à $ARTEFACT.sha256"
else
  echo "   AVERTISSEMENT : pas de .sha256 à côté de l'archive — l'empreinte de"
  echo "   l'archive n'est pas vérifiable. Le binaire, lui, le sera contre le manifeste."
fi

TRAVAIL="$(mktemp -d "${TMPDIR:-/tmp}/selfd-artefact.XXXXXX")"
trap 'rm -f "$MSGFILE"; rm -rf "$TRAVAIL"' EXIT
tar -xzf "$ARTEFACT" -C "$TRAVAIL" || die 3 "l'archive ne s'extrait pas : $ARTEFACT"
BIN_SRC="$TRAVAIL/selfd"
[ -f "$BIN_SRC" ] || die 3 "l'archive ne contient pas de fichier « selfd »."
MANIFESTE="$(find "$TRAVAIL" -maxdepth 1 -name 'MANIFEST-*.txt' | head -1)"
[ -n "$MANIFESTE" ] || die 3 "l'archive ne contient pas de manifeste (MANIFEST-*.txt).
  Sans manifeste on ne sait pas ce qu'on pose ni d'où cela vient : l'artefact
  n'est pas un artefact, c'est un objet trouvé. R2."

# Les valeurs du manifeste sont relues par un SCRIPT : on ne garde que le
# premier mot de la ligne, pour qu'une annotation en fin de valeur (il y en a eu
# une, et elle est entrée dans une comparaison de version) ne puisse plus
# tromper cette porte.
lire_manifeste() { sed -n "s/^$1 *: *//p" "$MANIFESTE" | head -1 | awk '{print $1}'; }
# Pour les champs de TEXTE (motif du refus, provenance), on garde la ligne
# entière : c'est le motif qui rend le refus actionnable, tronqué il ne l'est plus.
lire_manifeste_texte() { sed -n "s/^$1 *: *//p" "$MANIFESTE" | head -1; }
SHA_MANIF="$(lire_manifeste sha256)"
SHA_MESURE="$(sha256sum "$BIN_SRC" | cut -d' ' -f1)"
[ "$SHA_MANIF" = "$SHA_MESURE" ] || die 3 "le binaire de l'archive ne correspond pas à son manifeste :
    manifeste : $SHA_MANIF
    mesuré    : $SHA_MESURE
  On ne pose pas un binaire que son propre manifeste ne décrit pas."
COMMIT="$(lire_manifeste commit)"
COMMIT_COURT="$(lire_manifeste commit_court)"
TRIPLE="$(lire_manifeste triple)"
GLIBC_REQUISE="$(lire_manifeste glibc_exigee_max)"
DEPLOYABLE="$(lire_manifeste_texte DEPLOYABLE)"
PROVENANCE="$(lire_manifeste_texte provenance)"
echo "   commit     : $COMMIT_COURT ($COMMIT)"
echo "   triple     : $TRIPLE"
echo "   glibc      : exige $GLIBC_REQUISE"
echo "   provenance : $PROVENANCE"
case "$DEPLOYABLE" in
  oui*) echo "   déployable : oui" ;;
  *)    if [ "$EPREUVE" -eq 1 ] && [ "$CIBLE" = "local" ]; then
          echo "   déployable : NON (artefact d'épreuve) — accepté car --epreuve-arbre-sale ET cible locale"
        else
          die 2 "cet artefact est marqué NON DÉPLOYABLE :
    $DEPLOYABLE
  Le motif est celui que l'empaqueteur a CONSTATÉ et écrit ci-dessus (arbre
  sale, provenance non établie, ou les deux) : ne pas lui substituer une cause
  supposée. Un artefact qui ne peut pas être rattaché à un commit n'est pas
  reconstructible, donc pas déployable (R2). Pour une épreuve de la chaîne, sur
  la machine locale seulement : --epreuve-arbre-sale. Mesuré le 2026-09-16 au
  soir : l'arbre était PROPRE et la cause était la fraîcheur de Cargo.lock —
  cette phrase annonçait « produit depuis un arbre sale », et elle envoyait
  nettoyer un arbre déjà propre."
        fi ;;
esac

# ── 2. LA CIBLE PEUT-ELLE SEULEMENT EXÉCUTER CE BINAIRE ? ───────────────────
etape "2. la cible peut-elle exécuter ce binaire ? (R4 : on refuse avant)"
case "$TRIPLE" in
  x86_64-*) [ "$ARCH_CIBLE" = "x86_64" ] || die 2 "le binaire est x86_64, la cible est $ARCH_CIBLE.
  Un binaire par architecture : c'est une limite connue de la chaîne, pas un
  détail — il faudra un artefact par architecture." ;;
  *) echo "   AVERTISSEMENT : triple $TRIPLE non vérifié par ce script." ;;
esac
# La comparaison de versions se fait sur les chiffres, pas sur les chaînes :
# « 2.9 » > « 2.39 » serait vrai en comparaison textuelle et faux en réalité.
#
# ── MAIS ON NE COMPARE QUE CE QUI EST COMPARABLE (2026-09-16) ───────────────
# Un artefact STATIQUE porte `glibc_exigee_max : aucune`. Le passer à la
# comparaison ci-dessous le ferait déclarer « TROP ANCIEN » — la porte refuserait
# un artefact PORTABLE, exactement l'inverse de son office. Le mot « aucune » est
# donc traité AVANT toute comparaison.
if [ "$GLIBC_REQUISE" = "aucune" ]; then
  echo "   glibc      : AUCUNE dépendance — binaire STATIQUE : la cible peut le charger, quelle que soit sa glibc"
  echo "                (c'est la réponse à « connecter le même partout » : Debian 11, Debian 12, Ubuntu 22.04, Alpine)"
elif [ "$GLIBC_CIBLE" != "(inconnue)" ] && [ -n "$GLIBC_REQUISE" ]; then
  REQ="${GLIBC_REQUISE#GLIBC_}"
  PLUS_BASSE="$(printf '%s\n%s\n' "$REQ" "$GLIBC_CIBLE" | sort -V | head -1)"
  if [ "$PLUS_BASSE" != "$REQ" ]; then
    die 2 "la cible a une glibc TROP ANCIENNE :
    exigée par le binaire : $REQ
    installée sur $CIBLE : $GLIBC_CIBLE
  Le binaire ne démarrerait pas — l'éditeur de liens refuse à ce niveau
  (« version GLIBC_${REQ} not found »), mesuré le 2026-09-16 sur Debian 12
  (2.36), Ubuntu 22.04 (2.35) et Debian 11 (2.31). Rien n'a été copié.
  Solution : empaqueter l'artefact STATIQUE (le défaut de l'empaqueteur depuis
  le 2026-09-16) — il n'a aucune dépendance à la glibc et démarre partout.
    cargo build --release --target x86_64-unknown-linux-musl -p obrain-selfd
    scripts/selfd-empaqueter.sh   # préfère automatiquement le statique"
  else
    echo "   glibc      : $GLIBC_CIBLE >= $REQ — la cible peut charger ce binaire"
  fi
else
  echo "   AVERTISSEMENT : glibc de la cible non établie — porte non franchie par mesure."
fi
# ── LES BIBLIOTHÈQUES : CELLES QUE LE BINAIRE DÉCLARE, PAS CELLES QU'ON SUPPOSE ─
# Ce contrôle cherchait `libgcc_s.so.1` EN DUR. C'est une hypothèse de l'ère
# glibc : un binaire STATIQUE ne déclare AUCUNE bibliothèque partagée, et la porte
# aurait donc refusé un artefact portable en affirmant qu'il déclare ce qu'il ne
# déclare pas. On lit désormais la liste `needed` du manifeste — celle que
# l'empaqueteur a mesurée sur le binaire — et on vérifie CHACUNE sur la cible.
NEEDED="$(lire_manifeste_texte needed)"
if [ -z "$NEEDED" ] || [ "$NEEDED" = "aucune" ]; then
  echo "   libs       : AUCUNE bibliothèque partagée déclarée (binaire statique) — rien à résoudre sur la cible"
else
  ABSENTES=""
  for lib in $NEEDED; do
    si_cible "ldconfig -p 2>/dev/null | grep -q -- '$lib'" || ABSENTES="$ABSENTES $lib"
  done
  if [ -n "$ABSENTES" ]; then
    die 2 "bibliothèque(s) déclarée(s) par le binaire et ABSENTE(S) de $CIBLE :$ABSENTES
  Le binaire ne démarrerait pas. Rien n'a été copié.
  (Liste lue dans le manifeste — les NEEDED réels du binaire — et non supposée.)"
  else
    echo "   libs       : toutes les bibliothèques DÉCLARÉES sont présentes sur la cible ($NEEDED)"
  fi
fi

# ── 3. LA RACINE DU NŒUD ────────────────────────────────────────────────────
etape "3. racine du nœud"
si_cible "mkdir -p '$RACINE'" || die 2 "impossible de créer la racine $RACINE sur $CIBLE."
RACINE_REELLE="$(si_cible "cd '$RACINE' && pwd -P")"
echo "   racine     : $RACINE_REELLE"
echo "   politique  : $([ "$ALLOW_EXEC" -eq 1 ] && echo "EXÉCUTION AUTORISÉE (--allow-exec)" || echo "observation seule (aucun --allow-exec : le nœud n'exécute rien)")"
[ "$ALLOW_EXEC" -eq 1 ] && cat >&2 <<AVERT
   ────────────────────────────────────────────────────────────────────────
   AVERTISSEMENT : --allow-exec a été demandé. Cette machine acceptera des
   commandes exécutées par le nœud, confinées à $RACINE_REELLE.
   C'est un acte, et il est visible dans l'unité posée. Sans lui, le nœud
   observe. Le refus de sortir de la racine reste, lui, inconditionnel.
   ────────────────────────────────────────────────────────────────────────
AVERT

# ── 4. POSER LE BINAIRE, VERSIONNÉ, ET VÉRIFIER CE QUI EST ARRIVÉ ───────────
etape "4. pose du binaire (transfert vérifié par empreinte, pas supposé)"
# Les chemins par défaut sont exprimés en `$HOME` de la CIBLE (guillemets
# simples : c'est le shell distant qui les étend, pas celui-ci — sans quoi on
# installerait dans le home de n3 en croyant installer chez le voisin).
if [ -z "$PREFIX" ]; then
  PREFIX_REELLE="$(si_cible 'mkdir -p "$HOME/.local/lib/selfd" && cd "$HOME/.local" && pwd -P')"
  PREFIX_MODELE="%h/.local"
else
  PREFIX_REELLE="$(si_cible "mkdir -p '$PREFIX/lib/selfd' && cd '$PREFIX' && pwd -P")"
  PREFIX_MODELE="$PREFIX"
fi
BIN_NOM="selfd-${COMMIT_COURT}-$(echo "$SHA_MESURE" | cut -c1-12)"
DEST_DIR="$PREFIX_REELLE/lib/selfd"
DEST="$DEST_DIR/$BIN_NOM"
TMP_DEST="$DEST.incomplet"
si_cible "rm -f '$TMP_DEST'" || true
vers_cible "$TMP_DEST" < "$BIN_SRC" || die 4 "le transfert vers $CIBLE a échoué."
SHA_DISTANT="$(si_cible "sha256sum '$TMP_DEST'" | cut -d' ' -f1)"
[ "$SHA_DISTANT" = "$SHA_MESURE" ] || die 4 "le binaire ARRIVÉ ne correspond pas à celui envoyé :
    envoyé  : $SHA_MESURE
    arrivé  : $SHA_DISTANT
  Le transfert l'a altéré. Rien n'est mis en place (fichier laissé : $TMP_DEST)."
# Le remplacement est ATOMIQUE, et le nom stable est un lien : le retour
# arrière tient en une commande (R2, « vers quoi revenir »).
si_cible "chmod 0755 '$TMP_DEST' && mv -f '$TMP_DEST' '$DEST' && ln -sfn '$DEST' '$DEST_DIR/selfd'" \
  || die 4 "la mise en place a échoué sur $CIBLE."
cp -p "$MANIFESTE" "$TRAVAIL/manifeste.txt"
vers_cible "$DEST.manifest.txt" < "$TRAVAIL/manifeste.txt" || die 4 "le manifeste n'a pas pu être déposé."
echo "   binaire    : $DEST   (sha256 vérifié APRÈS transfert)"
echo "   lien       : $DEST_DIR/selfd -> $DEST   (retour arrière : ln -sfn <autre> selfd)"
echo "   manifeste  : $DEST.manifest.txt   (R2 : le binaire installé porte son commit)"
VERSION_SUR_PLACE="$(si_cible "'$DEST_DIR/selfd' --help 2>&1 | head -1" || true)"
echo "   exécution  : $VERSION_SUR_PLACE"

# ── 4bis. LA PORTE : LE NŒUD DÉMARRE-T-IL VRAIMENT ICI ? ─────────────────────
# R5 : une porte teste la FONCTION, pas la vie. La glibc et les bibliothèques
# ont été vérifiées par leurs versions — c'est une déduction. On exécute donc
# le nœud UNE fois, avec SA racine et SA politique, et on refuse si ce qu'il
# rend ne dit pas ce qu'on vient d'installer. La sonde crée son identité dans
# `~/.local/state/selfd/` : c'est la même que celle du démon, donc la vérifier
# ici la rend durable au lieu de la déplacer.
# La sonde porte la MÊME politique que l'unité : sans quoi elle prouverait
# qu'un nœud démarre, pas que CELUI-CI démarre — et elle afficherait « exec
# refusé » juste après une installation avec --allow-exec.
SONDE_FLAGS=""
[ "$ALLOW_EXEC" -eq 1 ] && SONDE_FLAGS=" --allow-exec"
SONDE="$(si_cible "'$DEST_DIR/selfd' --root '$RACINE_REELLE'$SONDE_FLAGS --call status 2>&1" || true)"
if ! grep -q 'exec *:' <<<"$SONDE"; then
  echo "$SONDE" | sed 's/^/    /' >&2
  die 4 "le binaire installé ne démarre pas sur $CIBLE (la sonde \`--call status\`
  n'a pas rendu un statut de nœud). Motif ci-dessus — c'est exactement ce que la
  vérification de glibc cherchait à prévenir, et elle n'a pas suffi : ne pas
  activer l'unité tant que ce n'est pas compris. Le binaire est resté en place
  ($DEST), rien n'a été activé."
fi
# Et la politique rendue est VÉRIFIÉE, pas supposée : ce que l'installateur a
# décidé doit être ce que le nœud applique vraiment.
if [ "$ALLOW_EXEC" -eq 1 ]; then
  grep -q 'exec *: *AUTORISÉ' <<<"$SONDE" || die 4 "l'installateur a demandé --allow-exec, mais le nœud
  installé rend une autre politique :
$(grep 'exec *:' <<<"$SONDE" | sed 's/^/    /')
  Ne pas activer : la politique appliquée ne serait pas celle qui a été décidée."
else
  grep -q 'exec *: *refusé' <<<"$SONDE" || die 4 "le nœud installé n'a PAS la politique d'observation
  demandée par défaut :
$(grep 'exec *:' <<<"$SONDE" | sed 's/^/    /')
  Ne pas activer : un nœud qui exécute sans que personne ne l'ait décidé est
  précisément ce que cette chaîne existe pour empêcher."
fi
echo "   sonde      : $(grep -m1 'nœud *:' <<<"$SONDE" | sed 's/  */ /g')"
echo "                $(grep -m1 'exec *:' <<<"$SONDE" | sed 's/  */ /g') — politique appliquée vérifiée"

# ── 5. LA POLITIQUE, DANS UN FICHIER À PART (pas dans l'unité) ──────────────
etape "5. politique locale et socle (fichiers 0600, jamais dans une ligne de commande)"
CONF_DIR="$(si_cible 'mkdir -p "$HOME/.config/selfd" && cd "$HOME/.config/selfd" && pwd -P')"
ENV_SELFD="$CONF_DIR/selfd.env"
{
  echo "# Politique locale du nœud selfd — écrite par scripts/selfd-installer.sh le $(date -u +%Y-%m-%dT%H:%M:%SZ)."
  echo "# Ce fichier est la SOURCE de la politique : l'unité y lit SELFD_RACINE."
  echo "# Le modifier est une dérive ; relancer l'installateur, c'est le corriger."
  echo "SELFD_RACINE=$RACINE_REELLE"
  echo "SELFD_MOTEUR=${MOTEUR:-http://127.0.0.1:8432}"
} > "$TRAVAIL/selfd.env"
vers_cible "$ENV_SELFD" < "$TRAVAIL/selfd.env" || die 4 "écriture de $ENV_SELFD impossible"
si_cible "chmod 0600 '$ENV_SELFD'"
echo "   $ENV_SELFD (0600)"

if [ -n "$JETON" ]; then
  MACHINE_NOM="${MACHINE_NOM:-$(si_cible 'cat /proc/sys/kernel/hostname 2>/dev/null || hostname')}"
  ATT_ENV="$CONF_DIR/attacher.env"
  {
    echo "# Jeton du moteur pour CE compte — écrit par selfd-installer.sh le $(date -u +%Y-%m-%dT%H:%M:%SZ)."
    echo "# `selfd` le lit dans l'environnement ($SELFD_MOTEUR_TOKEN) : il n'apparaît"
    echo "# donc NI dans l'unité, NI dans une ligne de commande (donc pas dans \`ps\`)."
    echo "SELFD_MOTEUR=${MOTEUR:-http://127.0.0.1:8432}"
    echo "SELFD_MOTEUR_TOKEN=$JETON"
    echo "SELFD_MACHINE=$MACHINE_NOM"
    echo "SELFD_RACINE=$RACINE_REELLE"
  } > "$TRAVAIL/attacher.env"
  vers_cible "$ATT_ENV" < "$TRAVAIL/attacher.env" || die 4 "écriture de $ATT_ENV impossible"
  si_cible "chmod 0600 '$ATT_ENV'"
  echo "   $ATT_ENV (0600) — machine « $MACHINE_NOM »"
else
  MACHINE_NOM="${MACHINE_NOM:-<nom-de-la-machine>}"
  echo "   (pas de --jeton-moteur : la commande de rattachement est affichée à la fin)"
fi

# ── 6. L'UNITÉ, RENDUE DEPUIS LE MODÈLE DU DÉPÔT ────────────────────────────
etape "6. unité systemd --user (rendue depuis deploy/systemd/selfd.service)"
[ -f "$MODELE_UNITE" ] || die 5 "modèle d'unité introuvable : $MODELE_UNITE"
if [ "$ALLOW_EXEC" -eq 1 ]; then
  POLITIQUE="# Politique rendue par selfd-installer.sh le $(date -u +%Y-%m-%dT%H:%M:%SZ) :
#   EXÉCUTION AUTORISÉE sur cette machine (--allow-exec), confinée à
#   $RACINE_REELLE. Le nœud exécute ce que le moteur lui demande, dans
#   cette racine, et rien en dehors."
else
  POLITIQUE="# Politique rendue par selfd-installer.sh le $(date -u +%Y-%m-%dT%H:%M:%SZ) :
#   OBSERVATION SEULE (aucun --allow-exec). Le nœud lit, liste, rend son état ;
#   il n'exécute rien. Pour l'autoriser à agir, réinstaller avec --allow-exec —
#   et le faire sciemment."
fi
UNITE_RENDUE="$TRAVAIL/selfd.service"
awk -v pol="$POLITIQUE" '{ if ($0 == "#@EXEC_POLITIQUE@") { print pol } else { print } }' \
  "$MODELE_UNITE" > "$UNITE_RENDUE"
if [ "$ALLOW_EXEC" -eq 1 ]; then
  sed -i 's|^  --socket %t/selfd.sock$|  --socket %t/selfd.sock --allow-exec|' "$UNITE_RENDUE"
  grep -q -- '--allow-exec' "$UNITE_RENDUE" || die 5 "la politique n'a pas pu être inscrite dans l'unité (marqueur introuvable)."
fi
PREFIX_UNITE="${PREFIX_MODELE}"
if [ "$PREFIX_UNITE" != "%h/.local" ]; then
  # Le modèle porte `%h/.local/lib/selfd/selfd` : si le préfixe diffère, l'unité
  # doit le dire, sinon elle pointerait sur un chemin qui n'existe pas.
  sed -i "s|%h/\.local/lib/selfd/selfd|${PREFIX_UNITE}/lib/selfd/selfd|" "$UNITE_RENDUE"
fi
ENTETE="# ── UNITÉ POSÉE — ne pas éditer à la main (R3 : une configuration, une source) ──
# Rendue par scripts/selfd-installer.sh le $(date -u +%Y-%m-%dT%H:%M:%SZ) sur $CIBLE
# artefact : $(basename "$ARTEFACT")
# commit   : $COMMIT
# sha256   : $SHA_MESURE
# politique: racine=$RACINE_REELLE exec=$([ "$ALLOW_EXEC" -eq 1 ] && echo AUTORISÉ || echo refusé)
# Régénérer : scripts/selfd-installer.sh $CIBLE --artefact <artefact> --racine $RACINE_REELLE$([ "$ALLOW_EXEC" -eq 1 ] && echo ' --allow-exec' || true)"
# Le défaut est le `$HOME` de la CIBLE (guillemets simples : c'est le shell
# distant qui l'étend). Un `--unite-dir` explicite est pris tel quel.
if [ -z "$UNITE_DIR" ]; then
  UNITE_DIR_REELLE="$(si_cible 'mkdir -p "$HOME/.config/systemd/user" && cd "$HOME/.config/systemd/user" && pwd -P')"
else
  UNITE_DIR_REELLE="$(si_cible "mkdir -p '$UNITE_DIR' && cd '$UNITE_DIR' && pwd -P")"
fi
UNITE_CIBLE="$UNITE_DIR_REELLE/selfd.service"
{ echo "$ENTETE"; cat "$UNITE_RENDUE"; } > "$TRAVAIL/unite.service"
vers_cible "$UNITE_CIBLE" < "$TRAVAIL/unite.service" || die 4 "écriture de $UNITE_CIBLE impossible"
echo "   unité      : $UNITE_CIBLE"
if [ -f "$MODELE_ATTACHE" ] && [ -n "$JETON" ]; then
  ATT_CIBLE="$UNITE_DIR_REELLE/selfd.attacher.service"
  vers_cible "$ATT_CIBLE" < "$MODELE_ATTACHE" || die 4 "écriture de $ATT_CIBLE impossible"
  echo "   rattachement: $ATT_CIBLE (jeton lu dans attacher.env)"
fi

# ── 7. ACTIVER (un acte explicite) OU AFFICHER ──────────────────────────────
etape "7. activation"
if [ "$ACTIVER" -eq 1 ]; then
  si_cible 'systemctl --user daemon-reload' || die 4 "daemon-reload a échoué sur $CIBLE"
  si_cible 'systemctl --user enable --now selfd' || die 4 "enable --now a échoué sur $CIBLE"
  echo "   selfd activé sur $CIBLE"
  si_cible 'systemctl --user show selfd -p NRestarts -p ActiveState' || true
  [ -f "$MODELE_ATTACHE" ] && [ -n "$JETON" ] && si_cible 'systemctl --user enable --now selfd.attacher' || true
else
  cat <<FIN
   Rien n'a été activé (par défaut, l'installateur ne démarre pas un service :
   R6 — aucun outil ne redémarre le service dont il fait partie). Sur $CIBLE :

     systemctl --user daemon-reload
     systemctl --user enable --now selfd
FIN
  [ -f "$MODELE_ATTACHE" ] && [ -n "$JETON" ] && echo "     systemctl --user enable --now selfd.attacher"
fi

# ── 8. LA COMMANDE DE RATTACHEMENT, EXACTE ──────────────────────────────────
# C'est le geste qui fait passer d'« un binaire est posé » à « le self agit sur
# CETTE machine ». Sans lui, tout ce qui précède est un objet posé.
cat <<FIN

── 8. RATTACHER CETTE MACHINE AU MOTEUR
   Le canal est \`/api/remote/*\`, servi par le MOTEUR (viz serve, 8432) : c'est
   une connexion SORTANTE de la machine, rien n'est à ouvrir sur son routeur.

   Commande exacte (forme littérale — le jeton est alors visible dans \`ps\`) :

     selfd attacher --moteur ${MOTEUR:-http://127.0.0.1:8432} --jeton-moteur <jeton-de-votre-compte> --machine $MACHINE_NOM

   Forme recommandée (le jeton reste dans un fichier 0600) :

     $( [ -n "$JETON" ] && echo "systemctl --user enable --now selfd.attacher" || echo "set -a; . $CONF_DIR/attacher.env; set +a; selfd attacher" )
FIN

# ── 9. PROUVER QUE LE NŒUD VIT ET QU'IL AGIT (R5) ───────────────────────────
cat <<FIN

── 9. VÉRIFIER — la fonction, pas la vie
   Sur $CIBLE :

     selfd status                       # parle à la socket : la politique RÉELLE
     selfd --call read '{"path":"<un fichier de $RACINE_REELLE>"}'
     systemctl --user show selfd -p NRestarts    # INCHANGÉ après 60 s
     journalctl --user -u selfd -n 20

   Un « active » ne prouve rien : le 2026-09-01, NRestarts est monté à 9 sans
   que systemd renonce. Et \`HTTP 200\` ne prouve pas davantage la fonction.
FIN

echo
echo "selfd-installer : terminé sur « $CIBLE » — rien n'a été démarré sans --activer."
