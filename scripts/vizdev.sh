#!/usr/bin/env bash
# Dashboard obrain en MODE DEV (hot reload + rebuild auto) — voir vite-rs-like
set -e
fuser -k 8432/tcp 2>/dev/null || true
sleep 1
cd "$(dirname "$0")/.."
OBRAIN_DEV=1 nohup ./target/release/obrain viz serve \
  --host 192.168.1.103 --port 8432 \
  --preset-default "Runtime dev (engrams)" \
  --preset "Runtime dev (engrams)=/home/triviere/.local/share/obrain-harness/data/runtime-dev.obrain" \
  --preset "Runtime prod=/home/triviere/.local/share/obrain-harness/data/runtime-prod.obrain" \
  --preset "Self-demo (engrams)=/home/triviere/.local/share/obrain-harness/data/self-demo.obrain" \
  --preset "Linux complet (v6.6)=/tmp/linux-full3.obrain" \
  --preset "Linux e1000=/tmp/kv-linux-e1000.obrain" \
  --preset "Obrain (rust)=/tmp/engine_idx.obrain" \
  > /tmp/vizserve.log 2>&1 < /dev/null &
echo "viz serve DEV lancé (pid $!) — hot reload actif (log /tmp/vizserve.log)"
sleep 1.5
curl -s -o /dev/null -w 'racine: %{http_code}\n' http://192.168.1.103:8432/ || true
