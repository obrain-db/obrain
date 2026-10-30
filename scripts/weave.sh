#!/usr/bin/env bash
# Tisse la TOILE DE L'UNIVERS et persiste la base synaptique de l'agent
# (vue Cosmos). Chaque synapse est fondée sur une preuve RÉELLE :
#   · entité partagée   — même `qualified_name` dans ≥ 2 mondes ;
#   · pont de vécu      — session réellement exécutée dans le territoire d'un
#                         monde (`Session.cwd` sous la racine du monde), datée ;
#   · plan traversant   — entité `self:`/`account:` présente dans un monde.
# La base produite est DURABLE (store obrain, WAL + flush) et se RENFORCE à
# chaque tissage au lieu d'être réécrite.
#
# Usage : scripts/weave.sh [--per-label N]
set -e
cd "$(dirname "$0")/.."
DATA=/home/triviere/.local/share/obrain-harness/data
OUT="$DATA/agent-synapse-triviere-dsh.obrain"
SELF_QN="self:triviere-dsh"
PER_LABEL=400

while [ $# -gt 0 ]; do
  case "$1" in
    --per-label) PER_LABEL="$2"; shift 2 ;;
    *) echo "option inconnue : $1" >&2; exit 2 ;;
  esac
done

# Le tissage ouvre la base synaptique en EXCLUSIF : c'est la seule ouverture
# où le RENFORCEMENT des synapses survit (mesuré — en shared-write, les
# propriétés écrites après le dernier flush sont perdues et le compteur reste
# figé). Le service viz tient la base en lecture seule → on l'arrête, on
# tisse, puis on le relance pour que les lecteurs voient la toile fraîche.
PORT=8432
HOST=192.168.1.103
SERVER_WAS_UP=0
if pgrep -f 'obrain viz serve' >/dev/null 2>&1; then
  SERVER_WAS_UP=1
  # Arrêt PROPRE (SIGTERM) puis attente de sortie effective : un `kill -9` sur
  # un serveur qui tenait les stores mmap'és provoquait un SIGBUS intermittent
  # au tissage suivant (reproduit : rc=135 au 1er passage après un -9, OK au
  # suivant). On laisse donc le processus rendre ses mappings.
  for p in $(pgrep -f 'obrain viz serve' 2>/dev/null); do kill -TERM "$p" 2>/dev/null || true; done
  for _ in $(seq 1 40); do
    pgrep -f 'obrain viz serve' >/dev/null 2>&1 || break
    sleep 0.25
  done
  # Dernier recours seulement : le service ne répond pas au SIGTERM.
  for p in $(pgrep -f 'obrain viz serve' 2>/dev/null); do kill -9 "$p" 2>/dev/null || true; done
  fuser -k ${PORT}/tcp 2>/dev/null || true
  sleep 1
  sync
fi

run_weave() {
  ./target/release/obrain viz weave \
    --world "Runtime dev (engrams)=$DATA/runtime-dev.obrain" \
    --world "Runtime prod=$DATA/runtime-prod.obrain" \
    --world "Self-demo (engrams)=$DATA/self-demo.obrain" \
    --world "Linux complet (v6.6)=/tmp/linux-full3.obrain" \
    --world "Linux e1000=/tmp/kv-linux-e1000.obrain" \
    --world "Obrain (rust)=/tmp/engine_idx.obrain#/home/triviere/projects/banc-obrain" \
    --self "$SELF_QN" \
    --per-label "$PER_LABEL" \
    --out "$OUT"
}

run_weave
STATUS=$?
# Un tissage tué par un signal (SIGBUS au premier passage après un arrêt
# brutal) est repris UNE fois : le tissage est idempotent par construction
# (MERGE), seul le compteur de renforcement s'incrémente une fois de plus.
if [ "$STATUS" -ge 128 ]; then
  echo "weave : tissage interrompu par un signal (rc=$STATUS) — reprise" >&2
  sleep 2
  run_weave
  STATUS=$?
fi

if [ "$SERVER_WAS_UP" = "1" ]; then
  bash scripts/vizserve.sh >/dev/null 2>&1 || true
  echo "service viz relancé sur http://$HOST:$PORT/ (toile à jour)" >&2
fi
exit $STATUS
