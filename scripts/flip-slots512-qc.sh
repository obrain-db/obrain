#!/usr/bin/env bash
# QC/flip slots 512 — à exécuter sur des COPIES de bases (jamais en direct).
# Usage : scripts/flip-slots512-qc.sh <copie1> [copie2 ...]
# Construit l'outil QC depuis le code courant (= « nouveau binaire ») puis, pour
# CHAQUE copie : baseline (counts) → écritures de contrôle packées (env
# OBRAIN_PROPS_SLOTS_512=1) → réouverture + re-QC → parité des counts.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO"
cargo build --release -p obrain-engine --example slots512_qc >/dev/null
BIN="$REPO/target/release/examples/slots512_qc"
for base in "$@"; do
  echo "##### QC slots512 sur copie : $base #####"
  OBRAIN_PROPS_SLOTS_512=1 "$BIN" "$base"
done
