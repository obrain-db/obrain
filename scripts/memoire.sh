#!/usr/bin/env bash
# APPLIQUE la mémoire du chat dans la base du Self.
#
# Pourquoi une passe séparée, et pourquoi elle arrête le service :
#
# Le substrat impose l'invariant mono-écrivain. Une base ouverte en LECTURE prend
# un `flock` PARTAGÉ ; une écriture exige le verrou EXCLUSIF, et le registre
# refuse la promotion (« already open read-only in this process — cannot upgrade
# to an exclusive (write) lock »). Le serveur du dashboard tient ses bases en
# lecture : il ne peut donc pas écrire dedans — et ce n'est pas un défaut à
# contourner, c'est ce qui protège les données.
#
# D'où les deux temps, exactement comme `scripts/weave.sh` :
#   1. le chat JOURNALISE ses intentions (fichier append-only, aucun verrou,
#      durable immédiatement) ;
#   2. cette passe les APPLIQUE en MERGE idempotent, puis consomme le journal.
#
# L'arrêt est PROPRE (SIGTERM puis attente) : un `kill -9` sur un serveur qui
# tenait les stores mmap'és a déjà provoqué un SIGBUS au passage suivant
# (constaté sur le tissage — on ne reproduit pas).
#
# Usage : scripts/memoire.sh [--self self:triviere-dsh] [--store <chemin>] [--etat]
set -e
cd "$(dirname "$0")/.."

DATA=/home/triviere/.local/share/obrain-harness/data
SELF_QN="self:triviere-dsh"
STORE="$DATA/self-demo.obrain"
ETAT_SEUL=0

while [ $# -gt 0 ]; do
  case "$1" in
    --self) SELF_QN="$2"; shift 2 ;;
    --store) STORE="$2"; shift 2 ;;
    --etat) ETAT_SEUL=1; shift ;;
    *) echo "option inconnue : $1" >&2; exit 2 ;;
  esac
done

BIN=./target/release/obrain

if [ "$ETAT_SEUL" = "1" ]; then
  "$BIN" memoire --self "$SELF_QN"
  exit 0
fi

# Ce qui est en attente AVANT : sans ce chiffre, on ne peut pas dire ce que la
# passe a réellement changé.
if ! ETAT=$("$BIN" memoire --self "$SELF_QN"); then
  echo "mémoire : lecture du journal impossible — $BIN memoire a échoué" >&2
  exit 1
fi
AVANT=$(printf '%s' "$ETAT" | grep -oP '"en_attente":\s*\K[0-9]+' || echo 0)
echo "mémoire : $AVANT entrée(s) en attente pour $SELF_QN"
if [ "$AVANT" = "0" ]; then
  echo "mémoire : rien à appliquer."
  exit 0
fi

PORT=8432
HOST=192.168.1.103
SERVER_WAS_UP=0
if pgrep -f 'obrain vi[z] serve' >/dev/null 2>&1; then
  SERVER_WAS_UP=1
  for p in $(pgrep -f 'obrain vi[z] serve' 2>/dev/null); do kill -TERM "$p" 2>/dev/null || true; done
  for _ in $(seq 1 40); do
    pgrep -f 'obrain vi[z] serve' >/dev/null 2>&1 || break
    sleep 0.5
  done
  pgrep -f 'obrain vi[z] serve' >/dev/null 2>&1 && {
    echo "mémoire : le serveur ne rend pas la main — abandon (aucune écriture)" >&2
    exit 1
  }
  echo "mémoire : serveur arrêté proprement"
fi

# ── AUCUN DÉTENTEUR, SINON ON LE DIT ────────────────────────────────────────
# Constaté : l'ouverture exclusive a échoué alors que le serveur était arrêté —
# un autre processus tenait la base en partage (les serveurs MCP `obrain bridge`
# et une synchro externe tiennent des stores obrain en permanence). Sans ce
# contrôle, l'échec arrive en message brut ; avec, on nomme le fautif.
TENANTS=""
for p in $(ps -eo pid | tail -n +2); do
  if ls -l "/proc/$p/fd" 2>/dev/null | grep -q "$(basename "$STORE")"; then
    TENANTS="$TENANTS $p"
  fi
done
if [ -n "$TENANTS" ]; then
  echo "mémoire : la base est encore tenue par :$TENANTS" >&2
  echo "  (l'écriture exige le verrou EXCLUSIF ; ces processus la tiennent en partage)" >&2
  for p in $TENANTS; do echo "    PID $p : $(tr '\0' ' ' < /proc/$p/cmdline 2>/dev/null | cut -c1-90)" >&2; done
  if [ "$SERVER_WAS_UP" = "1" ]; then ./scripts/vizserve.sh >/dev/null 2>&1 || true; fi
  exit 1
fi

set +e
"$BIN" memoire --self "$SELF_QN" --store "$STORE" --appliquer
RC=$?
set -e

if [ "$SERVER_WAS_UP" = "1" ]; then
  ./scripts/vizserve.sh >/dev/null 2>&1 || true
  sleep 2
  echo "mémoire : dashboard relancé sur http://$HOST:$PORT"
fi
exit $RC
