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
# Garde-fous (méthode de docs/BENCHMARKS.md, appliquée ici) :
#   - charge : la charge 1 min (`uptime`) est lue avant et après chaque passe
#     save/compare. Charge >= 2 au départ : refus, sortie 3, rien n'est compilé.
#     Charge >= 2 à l'arrivée : ligne « ATTENTION charge » et sortie 4, pour qu'un
#     relevé pris pendant une charge haute ne passe jamais pour une ligne de base
#     propre. En mode run, la charge est seulement notée au début et à la fin.
#   - compilation hors fenêtre : chaque bench est compilé (--no-run) avant la
#     boucle mesurée, la compilation ne tombe donc pas dans le temps mesuré.
#   - échecs : toute commande cargo en échec arrête le script (sortie 1).
#   - arbre de build : CARGO_TARGET_DIR est honoré s'il est défini, jamais
#     positionné ici. Les lignes de base vivent dans <target>/criterion : elles ne
#     sont comparables qu'entre arbres qui partagent le même CARGO_TARGET_DIR.
#
# Pas de retry, pas de relance jusqu'au vert : un rouge est un rouge, on le lit.
set -euo pipefail

MODE="${1:-run}"
NOM="${2:-$(git rev-parse --short HEAD)}"
SEUIL="${BENCH_SEUIL:-0.10}"
CHARGE_MAX=2
BENCHES="${BENCHES:-obrain-substrate:tiered_scan obrain-substrate:wal_append obrain-engine:query_bench obrain-engine:phase_a_write_goulot}"
OPTS="${BENCH_OPTS:---warm-up-time 1 --measurement-time 5}"

case "$MODE" in
  save)    extra="--save-baseline $NOM" ;;
  compare) extra="--baseline $NOM --noise-threshold $SEUIL" ;;
  run)     extra="" ;;
  *) echo "usage : $0 save|compare|run [nom]" >&2; exit 2 ;;
esac

# Charge 1 min : premier nombre de `uptime`, virgule décimale ramenée au point.
charge() {
  uptime | sed -E 's/.*load averages?: *//' | awk '{ v=$1; sub(/,$/, "", v); gsub(/,/, ".", v); print v }'
}
# Lit la charge dans CHARGE ; sort 1 si elle est illisible (garde inopérant sinon).
lire_charge() {
  CHARGE="$(charge)"
  [[ "$CHARGE" =~ ^[0-9]+(\.[0-9]+)?$ ]] || { echo "bench-v001 : charge illisible dans uptime (« $CHARGE »)" >&2; exit 1; }
}
# Vrai (0) si la charge CHARGE atteint CHARGE_MAX.
trop_charge() {
  awk -v l="$CHARGE" -v m="$CHARGE_MAX" 'BEGIN { exit !(l + 0 >= m) }'
}
# Filtre de la sortie Criterion : un nom nu (ligne seule avant « time: ») ou un verdict.
filtre_criterion() {
  awk '/^[A-Za-z_][A-Za-z0-9_\/]* *$/ || /time:|change:|Performance|No change|within noise|error/'
}

lire_charge
echo "bench-v001 : mode=$MODE nom=$NOM seuil=$SEUIL charge=$CHARGE target=${CARGO_TARGET_DIR:-défaut}"
if [ "$MODE" != run ] && trop_charge; then
  echo "bench-v001 : refus, charge $CHARGE >= $CHARGE_MAX : machine pas calme, ce ne serait pas un relevé. Relancer quand la charge sera retombée." >&2
  exit 3
fi

# Compilation hors fenêtre : aucune compilation ne doit tomber dans la boucle mesurée.
for b in $BENCHES; do
  crate="${b%%:*}"; bench="${b##*:}"
  echo "── compilation $crate / $bench"
  cargo bench -p "$crate" --bench "$bench" --no-run || { echo "bench-v001 : échec de compilation $crate / $bench" >&2; exit 1; }
done

journal="$(mktemp -t bench-v001.XXXXXX)"
for b in $BENCHES; do
  crate="${b%%:*}"; bench="${b##*:}"
  echo "── $crate / $bench"
  # Pas de `|| true` : pipefail fait remonter l'échec de cargo, le script sort 1.
  # shellcheck disable=SC2086
  cargo bench -p "$crate" --bench "$bench" -- $OPTS $extra 2>&1 | tee -a "$journal" | filtre_criterion \
    || { echo "bench-v001 : échec de $crate / $bench" >&2; exit 1; }
done

regressions=0
if [ "$MODE" = compare ]; then
  regressions=$(grep -c "Performance has regressed" "$journal" || true)
  ameliorations=$(grep -c "Performance has improved" "$journal" || true)
  echo "bench-v001 : $regressions régression(s), $ameliorations amélioration(s) au seuil $SEUIL (journal : $journal)"
fi

lire_charge
echo "bench-v001 : charge en fin de passe = $CHARGE"
if [ "$MODE" != run ] && trop_charge; then
  echo "ATTENTION charge $CHARGE >= $CHARGE_MAX pendant la passe : relevé provisoire, ne pas le prendre pour une ligne de base (journal : $journal)" >&2
  exit 4
fi

[ "$regressions" -eq 0 ] || exit 1
