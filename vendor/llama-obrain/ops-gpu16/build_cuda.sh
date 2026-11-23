#!/usr/bin/env bash
# build_cuda.sh — build CUDA de l'engine (llama-obrain-dgx, merge 2026-07 : MTP/Eagle3, jspace)
# Usage : bash build_cuda.sh [repo] [arch]
#   repo : chemin du checkout engine (défaut : la machine GPU a le sien ; sinon pointer ici)
#   arch : 89 (RTX 40xx Ada, défaut) | 90 (Blackwell) | 86 (Ampere) | native
set -euo pipefail
REPO="${1:?usage: build_cuda.sh <repo-llama-obrain-dgx> [arch]}"
ARCH="${2:-89}"

# Prérequis machine GPU : CUDA toolkit >= 12.4 (nvcc), cmake >= 3.24, gcc >= 11
command -v nvcc >/dev/null || { echo "ERREUR: nvcc absent — installer CUDA toolkit (12.4+)."; exit 1; }

cd "$REPO"
cmake -S . -B build-cuda \
  -DCMAKE_BUILD_TYPE=Release \
  -DGGML_CUDA=ON \
  -DGGML_CUDA_FA=ON \
  -DCMAKE_CUDA_ARCHITECTURES="$ARCH"
cmake --build build-cuda --target llama-server llama-bench -j "$(nproc)"

echo "OK — binaires : $REPO/build-cuda/bin/llama-server, llama-bench"
"$REPO/build-cuda/bin/llama-server" --version
