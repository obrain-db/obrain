#!/usr/bin/env bash
# Garde DoD de la couche viz (P0-v06) — voir docs/FULL-OBRAIN-VIZ-ROADMAP §5.
#
# Usage :
#   scripts/guard-viz.sh                 # garde sur corpus généré (jetable)
#   scripts/guard-viz.sh --bench [store] # + bench chiffré (défaut : corpus ou $VIZ_STORE)
#   VIZ_STORE=<chemin store> scripts/guard-viz.sh
#
# Sortie : lignes PASS/FAIL ; code de sortie ≠ 0 si une vérification échoue.
# La logique vit dans scripts/viz_guard.py (python3) — ce wrapper garde le
# point d'entrée shell documenté dans le plan (pre-push : scripts/pre-push.sh).
set -uo pipefail
cd "$(dirname "$0")/.."
exec python3 scripts/viz_guard.py "$@"
