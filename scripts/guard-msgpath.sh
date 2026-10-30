#!/usr/bin/env bash
# Garde DoD du chemin message obrain (2026-09-09) — voir bench/GUARD-SPEC.md.
#
# Usage :
#   scripts/guard-msgpath.sh --store <copie.obrain>   # garde complète sur copie
#   MSGPATH_BIN=<binaire> scripts/guard-msgpath.sh --store <copie.obrain>
#
# SÉCURITÉ : la garde REFUSE les stores vivants (~/.local/share/obrain-harness/data)
# et ÉCRIT dans la copie fournie (régénération d'un tail WAL) — fournir une
# copie jetable recréée par `cp -a` (voir bench/bench_msg_path.py).
# Sortie : lignes PASS/FAIL ; code de sortie ≠ 0 si une vérification échoue.
set -uo pipefail
cd "$(dirname "$0")/.."
exec python3 scripts/msgpath_guard.py "$@"
