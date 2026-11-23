#!/usr/bin/env bash
# run_dgx_test.sh - end-to-end deploy + test of the obrain llama engine on the DGX
# (llama-obrain-dgx engine, Qwen3.8-27B-UD-Q3_K_XL + MTP draft, jspace slot).
# Usage: bash run_dgx_test.sh [arch]   (arch: 89 Ada default | 90 Blackwell | 86 Ampere | native)
set -euo pipefail

N3="${N3:-triviere@192.168.1.103}"
MODELS_DIR="${MODELS_DIR:-/mnt/obrain/models}"
REPO="${REPO:-/home/triviere/projects/llama-obrain-dgx}"
PORT="${PORT:-8099}"
ARCH="${1:-89}"
MODEL="$MODELS_DIR/Qwen3.8-27B-UD-Q3_K_XL.gguf"
DRAFT="$MODELS_DIR/mtp-Qwen3.8-27B-Q4_0.gguf"
BIN="$REPO/build-cuda/bin/llama-server"

echo "== 0. preflight =="
nvidia-smi -L || { echo "ERR: no NVIDIA GPU on this host"; exit 1; }
command -v nvcc >/dev/null || { echo "ERR: nvcc absent - install CUDA toolkit >= 12.4"; exit 1; }
command -v cmake >/dev/null || { echo "ERR: cmake absent"; exit 1; }
command -v rsync >/dev/null || { echo "ERR: rsync absent"; exit 1; }

# models: copy from n3 if missing
if [ ! -f "$MODEL" ] || [ ! -f "$DRAFT" ]; then
  echo "== models missing -> fetch from n3 =="
  ssh -o BatchMode=yes "$N3" true || { echo "ERR: n3 not reachable - copy the models manually"; exit 1; }
  mkdir -p "$MODELS_DIR"
  scp -o BatchMode=yes \
    "$N3:/mnt/obrain/models/Qwen3.8-27B-UD-Q3_K_XL.gguf" \
    "$N3:/mnt/obrain/models/mtp-Qwen3.8-27B-Q4_0.gguf" \
    "$MODELS_DIR/"
fi
[ -f "$MODEL" ] || { echo "ERR: $MODEL missing"; exit 1; }
[ -f "$DRAFT" ] || { echo "ERR: $DRAFT missing"; exit 1; }
echo "models OK ($(du -h "$MODEL" | cut -f1) + $(du -h "$DRAFT" | cut -f1))"

# engine source: fetch from n3 if missing
if [ ! -d "$REPO" ]; then
  echo "== engine missing -> rsync from n3 (source only) =="
  mkdir -p "$(dirname "$REPO")"
  rsync -a --exclude .git --exclude build "$N3:/home/triviere/projects/llama-obrain-dgx/" "$REPO/" || exit 1
fi

# build the CUDA engine if needed
if [ ! -x "$BIN" ]; then
  echo "== build CUDA engine (arch $ARCH, FlashAttention) =="
  cmake -S "$REPO" -B "$REPO/build-cuda" \
    -DCMAKE_BUILD_TYPE=Release \
    -DGGML_CUDA=ON \
    -DGGML_CUDA_FA=ON \
    -DCMAKE_CUDA_ARCHITECTURES="$ARCH"
  cmake --build "$REPO/build-cuda" --target llama-server -j "$(nproc)"
fi
echo "== engine =="
"$BIN" --version 2>&1 | head -2

# launch: Q3_K_XL full offload + KV q8 + MTP, parallel 3 (jspace reserved slot)
pkill -f "$BIN.*$PORT" 2>/dev/null || true
sleep 1
"$BIN" -m "$MODEL" \
  --host 127.0.0.1 --port "$PORT" \
  -c 8192 --parallel 3 \
  -ngl 99 \
  --cache-type-k q8_0 --cache-type-v q8_0 \
  -t 8 --no-warmup \
  --mtp --spec-draft-model "$DRAFT" --spec-draft-n-max 8 \
  > /tmp/llama-dgx.log 2>&1 &
SRV=$!
echo "== server pid $SRV on :$PORT (log /tmp/llama-dgx.log) =="

ok=0
for i in $(seq 1 150); do
  if curl -s -m 2 "http://127.0.0.1:$PORT/health" 2>/dev/null | grep -q ok; then ok=1; break; fi
  sleep 2
done
if [ "$ok" != 1 ]; then
  echo "ERR: server not healthy after 300s:"
  tail -25 /tmp/llama-dgx.log
  exit 1
fi
echo "== server healthy =="
curl -s "http://127.0.0.1:$PORT/v1/models" | head -c 260; echo

echo "== bench: real wall tok/s (32 tokens) =="
t0=$(date +%s%N)
R=$(curl -s -m 900 "http://127.0.0.1:$PORT/v1/chat/completions" \
  -H "Content-Type: application/json" \
  -d "{\"model\":\"$MODEL\",\"messages\":[{\"role\":\"user\",\"content\":\"Write a short haiku about obrain.\"}],\"max_tokens\":32,\"temperature\":0.2}")
t1=$(date +%s%N)
echo "$R" | head -c 700; echo
CT=$(echo "$R" | grep -o '"completion_tokens":[0-9]*' | cut -d: -f2)
EL=$(( (t1 - t0) / 1000000 ))
if [ -n "$CT" ] && [ "$EL" -gt 0 ]; then
  echo "wall: $CT tokens in ${EL} ms -> $(awk "BEGIN{printf \"%.1f\", $CT*1000/$EL}") tok/s"
fi
echo "== done - server still running on :$PORT (stop: kill $SRV) =="
