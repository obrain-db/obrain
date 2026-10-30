#!/bin/zsh
# Vérification de l'index d'égalité persisté — avant/après, sur une base
# portant un catalogue d'index déclaré (le régime tmn).
#
#   AVANT (binaire témoin, sans zone persistée fiable) : chaque ouverture
#   repaie batir_index_depuis_v2 pour toutes les clés du catalogue.
#   APRÈS (binaire T7 + une fusion via wal checkpoint) : l'ouverture
#   confronte les ancres et NE reconstruit RIEN (tracé).
#
# Usage: verif_index_persiste.sh <bin_temoin> <bin_t7> <db> <passes>
set -u
TEMOIN=$1; T7BIN=$2; DB=$3; PASSES=${4:-5}
REQ="MATCH (n:Function) WHERE n.name = 'compare_values' RETURN count(*)"

chrono() { # chrono <etiquette> <binaire> — minima sur PASSES passes
  local best=99999999
  for p in $(seq 1 $PASSES); do
    local t0=$(python3 -c 'import time;print(time.monotonic_ns())')
    $2 --format json query $DB $REQ >/dev/null 2>&1
    local t1=$(python3 -c 'import time;print(time.monotonic_ns())')
    local ms=$(( (t1 - t0) / 1000000 ))
    (( ms < best )) && best=$ms
  done
  echo "$1 : ${best} ms (minima $PASSES passes)"
}

echo "== catalogue déclaré =="
cat $DB/substrate.indexcat 2>/dev/null || echo "(absent)"

echo "== AVANT (témoin : reconstruction par processus à chaque ouverture) =="
chrono "avant" $TEMOIN

echo "== fusion : une ouverture T7 (paie la passe UNE fois) + checkpoint =="
time $T7BIN -q wal checkpoint $DB
ls -lh $DB/substrate.props.eq.idx.v2 2>/dev/null || echo "ZONE ABSENTE — echec"

echo "== APRÈS (T7 : ancres confrontées, zéro reconstruction) =="
chrono "apres" $T7BIN

echo "== preuve tracée (RUST_LOG=info, une ouverture) =="
RUST_LOG=obrain_substrate=info $T7BIN --format json query $DB $REQ 2>&1 >/dev/null \
  | grep -E "égalité persisté FIABLE|index de propriété bâtis|elapsed_ms" | head -5
