#!/usr/bin/env bash
# run_server_hip.sh — lancement serveur fork (jspace state_ref) sur GPU AMD via ROCm/HIP
# Usage : bash run_server_hip.sh [model.gguf] [port] [ctx]
set -euo pipefail
MODEL="${1:-/mnt/obrain/models/Qwen3.8-27B-UD-Q3_K_XL.gguf}"
PORT="${2:-8099}"
CTX="${3:-4096}"
BIN="${BIN:-/mnt/obrain/build-hip/bin/llama-server}"
export LD_LIBRARY_PATH="$(dirname "$BIN"):/opt/rocm/lib:${LD_LIBRARY_PATH:-}"

[ -f "$BIN" ] || { echo "BIN introuvable ($BIN) — lancer build_hip.sh"; exit 1; }
[ -f "$MODEL" ] || { echo "MODEL introuvable : $MODEL"; exit 1; }

exec "$BIN" -m "$MODEL" -ngl 99 -c "$CTX" --parallel 3 --port "$PORT" -t 8 \
  --cache-type-k q8_0 --cache-type-v q8_0 --no-warmup
