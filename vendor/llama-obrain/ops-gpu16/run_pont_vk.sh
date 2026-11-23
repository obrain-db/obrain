#!/usr/bin/env bash
# Pont obrain↔llama consolidé — machine n3, backend Vulkan (RADV, stable).
# Usage : bash run_pont_vk.sh [model] [port] [ctx]
set -euo pipefail
MODEL="${1:-/mnt/obrain/models/Qwen3.8-27B-UD-Q2_K_XL.gguf}"  # défaut qualité ; IQ1_M = vitesse
PORT="${2:-8099}"
CTX="${3:-4096}"
BIN="${BIN:-/mnt/obrain/build-vk/bin/llama-server}"
export LD_LIBRARY_PATH="/mnt/obrain/build-vk/bin:${LD_LIBRARY_PATH:-}"
export GGML_VK_VISIBLE_DEVICES=0
export OBRAIN_STATE_TYPE=i8          # K4 : états récurrents compressés (blob ÷3, cohérence OK)
exec "$BIN" -m "$MODEL" -ngl 99 -c "$CTX" --parallel 3 --port "$PORT" -t 8 --no-warmup
