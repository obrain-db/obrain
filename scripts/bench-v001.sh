#!/usr/bin/env bash
# Les quatre benchs de référence de la v0.0.1 (docs/BENCHMARKS.md), avec une
# ligne de base Criterion et un seuil de non-régression.
#
#   scripts/bench-v001.sh save [nom]         enregistre la ligne de base `nom` (défaut : sha court)
#   scripts/bench-v001.sh compare <nom>      compare l'arbre courant à `nom` ; sort 1 si Criterion
#                                            juge qu'une mesure a RÉGRESSÉ au-delà du seuil
#   scripts/bench-v001.sh run                lance sans ligne de base (= make bench-v001)
#
# Seuil : BENCH_SEUIL (défaut 0.10 = 10 %), passé à Criterion en --noise-threshold ;
# en deçà, une variation est du bruit. Un runner partagé ne mesure pas : ce script
# est fait pour une machine calme (charge < 2, notée dans le relevé).
#
# Pas de retry, pas de relance jusqu'au vert : un rouge est un rouge, on le lit.
set -euo pipefail

MODE="${1:-run}"
NOM="${2:-$(git rev-parse --short HEAD)}"
SEUIL="${BENCH_SEUIL:-0.10}"
BENCHES="${BENCHES:-obrain-substrate:tiered_scan obrain-substrate:wal_append obrain-engine:query_bench obrain-engine:phase_a_write_goulot}"
OPTS="${BENCH_OPTS:---warm-up-time 1 --measurement-time 5}"

case "$MODE" in
  save)    extra="--save-baseline $NOM" ;;
  compare) extra="--baseline $NOM --noise-threshold $SEUIL" ;;
  run)     extra="" ;;
  *) echo "usage : $0 save|compare|run [nom]" >&2; exit 2 ;;
esac

echo "bench-v001 : mode=$MODE nom=$NOM seuil=$SEUIL charge=$(uptime | sed 's/.*load average[s]*: //')"
journal="$(mktemp -t bench-v001.XXXXXX)"
for b in $BENCHES; do
  crate="${b%%:*}"; bench="${b##*:}"
  echo "── $crate / $bench"
  # shellcheck disable=SC2086
  cargo bench -p "$crate" --bench "$bench" -- $OPTS $extra 2>&1 | tee -a "$journal" | grep -E "time:|change:|Performance|No change|within noise|error" || true
done

if [ "$MODE" = compare ]; then
  regressions=$(grep -c "Performance has regressed" "$journal" || true)
  ameliorations=$(grep -c "Performance has improved" "$journal" || true)
  echo "bench-v001 : $regressions régression(s), $ameliorations amélioration(s) au seuil $SEUIL (journal : $journal)"
  [ "$regressions" -eq 0 ] || exit 1
fi
