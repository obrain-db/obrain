#!/usr/bin/env bash
# Migration slots 512 — test multi-phase sur COPIES de bases (jamais en direct).
# Usage : scripts/migrate-slots512.sh <copie1> [copie2 ...]
# Phases : baseline → mises à jour d'entités EXISTANTES (legacy) → créations
# packées (nœuds+arêtes, têtes slot vérifiées) → suppressions → 3 cycles de
# réouverture avec parité exacte des comptes.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO"
cargo build --release -p obrain-engine --example mig_slots512 >/dev/null
BIN="$REPO/target/release/examples/mig_slots512"
for base in "$@"; do
  echo "######## MIGRATION slots512 : $base ########"
  OBRAIN_PROPS_SLOTS_512=1 "$BIN" "$base"
done
