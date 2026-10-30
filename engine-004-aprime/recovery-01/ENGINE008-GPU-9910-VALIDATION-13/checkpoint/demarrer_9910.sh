#!/bin/bash
# Demarrage du service 9910. Ce script ne se tue jamais lui-meme : l'arret se
# fait par PID, jamais par motif de ligne de commande.
set -u
F=/home/triviere/projects/llama-obrain-cuda-f1a
MC=/home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf
BASE=/home/triviere/obrain-9910
OLDPID=$(cat $BASE/server.pid 2>/dev/null || echo "")
if [ -n "$OLDPID" ] && kill -0 "$OLDPID" 2>/dev/null; then
  echo "arret de l'ancien serveur PID=$OLDPID"; kill "$OLDPID" 2>/dev/null; sleep 5
  kill -9 "$OLDPID" 2>/dev/null; sleep 2
fi
if ss -tln | grep -q ":9910 "; then echo "ATTENTION: 9910 encore occupe"; ss -tlnp | grep ":9910"; fi
mkdir -p $BASE/work $BASE/logs
cd $BASE/work
OBRAIN_KV_STATES_MAX_BYTES=2147483648 CUDA_VISIBLE_DEVICES=0 setsid nohup \
  $F/build-cuda/bin/llama-server -m $MC \
  --host 0.0.0.0 --port 9910 -ngl 99 --ctx-size 262144 --parallel 3 --jinja \
  > $BASE/logs/server.log 2>&1 < /dev/null &
NEWPID=$!
echo $NEWPID > $BASE/server.pid
echo "SERVER_PID=$NEWPID"
for i in $(seq 1 150); do
  if curl -s -m 3 http://127.0.0.1:9910/health 2>/dev/null | grep -q '"ok"'; then
    echo "DISPONIBLE a ${i}s"; break
  fi
  kill -0 "$NEWPID" 2>/dev/null || { echo "PROCESSUS MORT"; tail -8 $BASE/logs/server.log; exit 1; }
  sleep 1
done
echo "--- etat ---"
echo "  PID=$NEWPID  bind=$(ss -tln | awk '$4 ~ /:9910$/ {print $4}' | head -1)"
echo "  health=$(curl -s -m 5 http://127.0.0.1:9910/health)"
echo "  n_ctx_slot=$(grep -ao 'n_ctx_slot = [0-9]*' $BASE/logs/server.log | head -1 | cut -d= -f2 | tr -d ' ')"
echo "  arret : kill \$(cat $BASE/server.pid)"
