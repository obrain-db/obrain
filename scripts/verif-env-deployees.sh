#!/usr/bin/env bash
# VÉRIFICATION DES RÉGLAGES ARMÉS — « ce qui est armé est-il lu par le binaire servi ? »
#
# ── POURQUOI CE SCRIPT EXISTE ───────────────────────────────────────────────
#
# Défaut constaté le 2026-09-14, et il ne se voyait nulle part. L'unité systemd
# `obrain-viz.service` et son drop-in `trigrammes.conf` armaient QUATRE
# variables — `OBRAIN_IAM`, `OBRAIN_TRIGRAMMES_DES_LA`,
# `OBRAIN_TRIGRAMMES_PERSISTER`, `OBRAIN_PREFIXE_INTERVALLE` — qu'AUCUNE ligne
# du binaire DÉPLOYÉ ne lit. Le binaire servi était celui de la lignée
# `feat/principal-natif-j5` ; les quatre réglages, eux, venaient du tronc
# (`feat/tronc-complet-2026-09-13`) et de la consolidation qui a suivi (j4),
# jamais construite ni déployée.
#
# Rien ne le disait : le service avait l'air configuré. C'est exactement la
# classe du « ⚠ IAM NON COMPILÉ » que ce dépôt a déjà documentée trois fois —
# un réglage qui vit dans un fichier de déploiement que le binaire ignore.
#
# ── COMMENT, ET SANS RIEN SUPPOSER ──────────────────────────────────────────
#
# Un nom de variable d'environnement lu par Rust vit dans `.rodata` sous forme
# de chaîne littérale : `strings` le voit, ou ne le voit pas, et c'est la
# réponse. Le contrôle ne suppose donc RIEN des features compilées ni de la
# lignée dont vient le binaire — c'est précisément le point qu'on veut mesurer.
#
# La comparaison porte sur ce qui est RÉELLEMENT armé, pris dans cet ordre :
#   1. l'environnement du processus servi (`/proc/<pid>/environ`) — la vérité
#      de terrain, drop-ins, unité et exports de shell confondus ;
#   2. à défaut (service arrêté), l'unité et ses drop-ins (`systemctl cat`).
#
# Usage :
#   scripts/verif-env-deployees.sh [binaire] [unité]
#
# Défauts : le binaire déployé du harness et `obrain-viz.service`.
# Sortie : 0 si tout ce qui est armé est lu ; 1 s'il reste un réglage inerte.
set -u

BIN="${1:-${OBRAIN_BIN:-/home/triviere/.local/share/obrain-harness/bin/obrain}}"
UNITE="${2:-${OBRAIN_UNITE:-obrain-viz.service}}"
MOTIF_SERVICE="${OBRAIN_MOTIF_SERVICE:-obrain vi[z] serve}"

echecs=0
inertes=""

titre() { printf '\n%s\n' "$1"; }

if [ ! -x "$BIN" ]; then
  echo "❌ binaire introuvable : $BIN" >&2
  exit 2
fi

# ── 1. CE QUI EST ARMÉ ──────────────────────────────────────────────────────
titre "1) réglages armés"
PID="$(pgrep -f "$MOTIF_SERVICE" 2>/dev/null | head -1 || true)"
ARMES=""
SOURCE=""
if [ -n "$PID" ] && [ -r "/proc/$PID/environ" ]; then
  ARMES="$(tr '\0' '\n' < "/proc/$PID/environ" | sed -n 's/^\(OBRAIN_[A-Z0-9_]*\)=.*/\1/p' | sort -u)"
  SOURCE="environnement du processus $PID (service en marche)"
else
  if systemctl --user cat "$UNITE" >/dev/null 2>&1; then
    ARMES="$(systemctl --user cat "$UNITE" 2>/dev/null \
      | sed -n 's/^Environment=\(OBRAIN_[A-Z0-9_]*\)=.*/\1/p' | sort -u)"
    SOURCE="unité $UNITE et ses drop-ins (service arrêté)"
  else
    echo "  ⚠ ni processus servi ni unité « $UNITE » lisibles — rien à vérifier"
    exit 0
  fi
fi
if [ -z "$ARMES" ]; then
  echo "  aucun réglage OBRAIN_* armé ($SOURCE)"
  echo "  … rien à confronter au binaire."
  exit 0
fi
echo "  source : $SOURCE"
printf '  binaire : %s (%s)\n' "$BIN" "$(stat -c %y "$BIN" | cut -c1-19)"

# ── 2. CE QUE LE BINAIRE LIT ────────────────────────────────────────────────
# Lecture en une passe : 160 Mo de chaînes se parcourent une fois, pas N fois.
CHAINES="$(mktemp)"
trap 'rm -f "$CHAINES"' EXIT
strings -a "$BIN" > "$CHAINES" 2>/dev/null || true

# ── 3. LA CONFRONTATION ─────────────────────────────────────────────────────
titre "2) confrontation"
# Sous-chaîne, et non égalité : deux noms collés dans .rodata (constaté :
# « OBRAIN_STOREOBRAIN_OPEN_RETRY_MSRORW ») rendraient un faux « absent ».
for v in $ARMES; do
  if grep -qF -- "$v" "$CHAINES"; then
    printf '  ✅ %-32s lu par le binaire\n' "$v"
  else
    printf '  ❌ %-32s ARMÉ MAIS INERTE — le binaire ne connaît pas ce nom\n' "$v"
    inertes="$inertes $v"
    echecs=$((echecs + 1))
  fi
done

# ── 4. VERDICT ──────────────────────────────────────────────────────────────
titre "3) verdict"
if [ "$echecs" -eq 0 ]; then
  echo "  Tout ce qui est armé est lu par le binaire servi."
else
  echo "  $echecs réglage(s) armé(s) sans effet :"
  for v in $inertes; do echo "    · $v"; done
  echo "  Un réglage inerte est pire qu'un réglage absent : il fait croire que"
  echo "  le comportement mesuré vient de lui. Soit le binaire servi doit porter"
  echo "  le code qui le lit, soit le réglage doit être retiré du déploiement."
fi
exit $(( echecs > 0 ? 1 : 0 ))
