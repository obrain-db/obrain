#!/usr/bin/env bash
# SECONDE INSTANCE, PORT DISTINCT — pour VÉRIFIER sans dépendre du service de
# l'utilisateur (8432).
#
# Pourquoi ce script existe : deux agents travaillent dans ce dépôt en même
# temps. `scripts/vizserve.sh` tue par motif et reprend le port 8432 ; quand les
# deux le relancent, l'un échoue en « Address already in use » et c'est
# l'ANCIEN binaire qui continue de répondre — on croit mesurer le nouveau code,
# on mesure l'ancien (constaté deux fois : des mesures « après correctif » qui
# étaient en fait « avant »).
#
# Cette instance-ci ne touche à rien : port 8433, écoute locale, aucun arrêt.
#
# Usage : scripts/vizserve-isole.sh [--stop] [--port N]
set -e
cd "$(dirname "$0")/.."

DATA=/home/triviere/.local/share/obrain-harness/data
PORT=8433
STOP=0
while [ $# -gt 0 ]; do
  case "$1" in
    --stop) STOP=1; shift ;;
    --port) PORT="$2"; shift 2 ;;
    *) echo "option inconnue : $1" >&2; exit 2 ;;
  esac
done

# Arrêt PAR PID DU PORT, jamais par motif : `pgrep -f` matche la ligne de
# commande de l'appelant (constaté trois fois dans cette session, dont un
# `kill -9` qui a tué le shell qui l'exécutait).
arreter() {
  local pids
  pids=$(ss -ltnp 2>/dev/null | grep ":$PORT " | grep -oP 'pid=\K[0-9]+' | sort -u || true)
  for p in $pids; do
    kill -TERM "$p" 2>/dev/null || true
  done
  sleep 2
}

if [ "$STOP" = "1" ]; then
  arreter
  echo "instance $PORT : arrêtée"
  exit 0
fi

if ss -ltnp 2>/dev/null | grep -q ":$PORT "; then
  echo "instance $PORT : déjà en service — rien à faire"
  exit 0
fi

# Le moteur peut être distant : `OBRAIN_LLM_URL` est repris de l'environnement
# s'il est posé, sinon le défaut du logiciel s'applique. C'est ce qui permet de
# VÉRIFIER la découverte de fenêtre contre un autre service que celui du
# dashboard (mesuré : le 9 B local sert 8 192, le 27 B distant 32 768).
if [ -n "$OBRAIN_LLM_URL" ]; then
  echo "moteur : $OBRAIN_LLM_URL (repris de l'environnement)"
fi
nohup ./target/release/obrain viz serve \
  --host 127.0.0.1 --port "$PORT" \
  --preset-default "Runtime dev (engrams)" \
  --preset "Runtime dev (engrams)=$DATA/runtime-dev.obrain" \
  --preset "Runtime prod=$DATA/runtime-prod.obrain" \
  --preset "Self-demo (engrams)=$DATA/self-demo.obrain" \
  --preset "Obrain (rust)=/tmp/engine_idx.obrain" \
  --synapse-store "$DATA/agent-synapse-triviere-dsh.obrain" \
  > "/tmp/vizserve-$PORT.log" 2>&1 &

sleep 3
if ss -ltnp 2>/dev/null | grep -q ":$PORT "; then
  echo "instance de vérification : http://127.0.0.1:$PORT  (journal /tmp/vizserve-$PORT.log)"
else
  echo "instance $PORT : échec du lancement — voir /tmp/vizserve-$PORT.log" >&2
  tail -5 "/tmp/vizserve-$PORT.log" >&2
  exit 1
fi
