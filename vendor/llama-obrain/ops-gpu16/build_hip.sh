#!/usr/bin/env bash
# build_hip.sh — build HIP (ROCm) de l'engine fork llama-obrain (jspace K1/K2 state_ref)
# Machine : AMD iGPU/dGPU via ROCm. ROCm 6.2 → appliquer d'abord le shim fp8 (patch ci-joint).
# Usage : bash build_hip.sh [repo] [gfx]
set -euo pipefail
REPO="${1:?usage: build_hip.sh <repo-llama-obrain> [gfx]}"   # fork avec API state_ref
GFX="${2:-gfx1103}"                                           # gfx1103 = Radeon 780M (Zen4 APU)
export PATH=/opt/rocm/bin:$PATH

command -v hipcc >/dev/null || { echo "ERREUR: hipcc absent (installer ROCm)"; exit 1; }

# Shim ROCm 6.2 : __hip_fp8_e4m3 n'existe qu'en 6.3+ (6.2 = __hip_fp8_e4m3_fnuz)
H="$REPO/ggml/src/ggml-cuda/vendors/hip.h"
grep -q "ROCM 6.2 n'exporte que la variante fnuz" "$H" || python3 - <<PY
old='''#if HIP_VERSION >= 60200000
#include <hip/hip_fp8.h>
typedef __hip_fp8_e4m3 __nv_fp8_e4m3;
#define FP8_AVAILABLE
#endif // HIP_VERSION >= 60200000'''
new='''#if HIP_VERSION >= 60200000
#include <hip/hip_fp8.h>
// ROCm 6.2 n'exporte que la variante fnuz (__hip_fp8_e4m3 non-fnuz : ROCm >= 6.3).
#if HIP_VERSION < 60300000
typedef __hip_fp8_e4m3_fnuz __hip_fp8_e4m3;
#endif
typedef __hip_fp8_e4m3 __nv_fp8_e4m3;
#define FP8_AVAILABLE
#endif // HIP_VERSION >= 60200000'''
s=open("$H").read(); assert old in s; open("$H","w").write(s.replace(old,new,1))
PY

cd "$REPO"
cmake -S . -B build-hip \
  -DCMAKE_BUILD_TYPE=Release \
  -DGGML_HIP=ON \
  -DGGML_HIP_GRAPHS=OFF \
  -DAMDGPU_TARGETS="$GFX" \
  -DCMAKE_PREFIX_PATH=/opt/rocm
cmake --build build-hip --target llama-server llama-bench -j "$(nproc)"
echo "OK — $REPO/build-hip/bin/llama-server (HIP $GFX)"
