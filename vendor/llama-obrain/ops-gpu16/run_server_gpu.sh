#!/usr/bin/env bash
# run_server_gpu.sh — lancement llama-server GPU 16 Go (jspace + injection KV obrain)
# Modèle : Qwen3.8-27B-UD-Q3_K_XL (13,1 Go — tient full-offload en 16 Go VRAM avec KV q8)
# Usage : bash run_server_gpu.sh [model.gguf] [port] [ctx]
set -euo pipefail
MODEL="${1:-/mnt/obrain/models/Qwen3.8-27B-UD-Q3_K_XL.gguf}"
PORT="${2:-8099}"
CTX="${3:-8192}"
MTP_DRAFT="${MTP_DRAFT:-/mnt/obrain/models/mtp-Qwen3.8-27B-Q4_0.gguf}"

BIN="${BIN:-/home/triviere/projects/llama-obrain-dgx/build-cuda/bin/llama-server}"
[ -f "$BIN" ] || { echo "BIN introuvable — lancer build_cuda.sh d'abord"; exit 1; }
[ -f "$MODEL" ] || { echo "MODEL introuvable : $MODEL"; exit 1; }

ARGS=(
  -m "$MODEL"
  --host 127.0.0.1 --port "$PORT"
  -c "$CTX"
  --parallel 3            # exigé par le slot réservé /jspace (>=3)
  -ngl 99                 # full offload GPU (Q3_K_XL 12,24 GiB + KV q8 tient en 16 Go)
  --cache-type-k q8_0 --cache-type-v q8_0   # ÷2 VRAM KV (f16 si besoin de qualité : retirer)
  -t 8
  --no-warmup
)
# Décodage spéculatif MTP (draft Q4_0 1,28 Go — moteur merge 2026-07). Désactivable : USE_MTP=0
#
# CORRECTIF 2026-09-20 — `--mtp` NE VA PAS AU SERVEUR. Mesuré :
#     $ llama-server --mtp -m /x.gguf
#     error: invalid argument: --mtp
# `--mtp` est un drapeau de TÉLÉCHARGEMENT : common/arg.cpp:2805, il porte
# `.set_examples({LLAMA_EXAMPLE_DOWNLOAD})` (même chose dans le worktree -dgx). Le serveur
# attend `--spec-type draft-mtp` (common/speculative.cpp:35 le nomme, et il l'accepte).
# Tel qu'il était écrit, ce bloc faisait ÉCHOUER le lancement, ou tournait sans spéculation.
#
# EFFET DE BORD, et il est BON : `draft-mtp` est l'un des trois types qui alimentent
# `cparams.n_rs_seq` (common/common.h:392, `need_n_rs_seq()` → `draft.n_max`). Or
# `llm_arch_supports_rs_rollback()` est vrai pour **qwen35 et qwen35moe seulement**
# (src/llama-arch.cpp:966-974). Donc activer draft-mtp fait basculer la mémoire du modèle de
# FULL (retour arrière par CHECKPOINTS, où le rejeu d'un prompt identique ABANDONNE le serveur —
# c'est l'incident `common.cpp: failed to remove sequence 0 with p0=1454, p1=-1` de l'audit)
# vers RS (retour arrière BORNÉ EXACT). À vérifier dans le journal au démarrage :
#     « the context supports bounded partial sequence removal »   (RS — ce qu'on veut)
#   contre
#     « speculative decoding will use checkpoints »               (FULL — le mode fragile)
#
# γ : `--spec-draft-n-max` DÉFINIT AUSSI la borne de retour arrière n_rs_seq. γ=3 est le
# meilleur mesuré (IQ1_M : 5,42 → 8,44 tok/s) ; γ=4 PERD (7,48) — le draft coûte plus qu'il
# ne rapporte. Ne pas remettre 8 sans mesurer `draft_n_accepted / draft_n` dans `timings`.
if [ "${USE_MTP:-1}" = "1" ] && [ -f "$MTP_DRAFT" ]; then
  ARGS+=(--spec-type draft-mtp --spec-draft-model "$MTP_DRAFT" --spec-draft-n-max "${SPEC_N_MAX:-3}")
  echo "MTP spéculatif ACTIF (draft : $MTP_DRAFT, n_max=${SPEC_N_MAX:-3})"
fi

echo "lancement : $BIN ${ARGS[*]}"
exec "$BIN" "${ARGS[@]}"
