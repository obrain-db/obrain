#!/bin/zsh
# Banc allocateurs dos à dos sur la MÊME base : passes alternées ENTRE binaires
# (sys → jemalloc → mimalloc à chaque passe) pour que le bruit machine se
# répartisse ; minima retenus. RSS pic relevé à chaque invocation, minimum
# retenu (le RSS d'une requête est stable ; le min écarte les passes où le
# cache disque a gonflé la résidence).
# Usage: bench_allocateurs.sh <db> <passes> <bin1> <bin2> [bin3...]
set -u
DB=$1; PASSES=$2; shift 2
BINS=($@)

typeset -A Q
Q[q1_lookup_nom]="MATCH (n:Function) WHERE n.name = 'compare_values' RETURN n.qualified_name"
Q[q2_appelants_1saut]="MATCH (a)-[:CALLS]->(b:Function) WHERE b.name = 'compare_values' RETURN count(*)"
Q[q3_count_label]="MATCH (n:Function) RETURN count(*)"
Q[q4_orderby_desc_limit]="MATCH (a)-[:CALLS]->(b:Function) RETURN b.name, count(*) AS c ORDER BY c DESC LIMIT 5"
Q[q5_scan_filtre]="MATCH (n:Function) WHERE n.start_line > 200 RETURN count(*)"
ORDER=(q1_lookup_nom q2_appelants_1saut q3_count_label q4_orderby_desc_limit q5_scan_filtre)

typeset -A BEST RSS
for b in $BINS; do for k in $ORDER; do BEST[$b:$k]=999999999; RSS[$b:$k]=999999999999; done; done

for p in $(seq 1 $PASSES); do
  for b in $BINS; do
    for k in $ORDER; do
      t0=$(python3 -c 'import time;print(time.monotonic_ns())')
      OUT=$(/usr/bin/time -l $b --format json query $DB ${Q[$k]} 2>&1 >/dev/null)
      t1=$(python3 -c 'import time;print(time.monotonic_ns())')
      ms=$(( (t1 - t0) / 1000000 ))
      rss=$(echo $OUT | grep -oE '[0-9]+  maximum resident' | awk '{print $1}')
      [ -z "$rss" ] && rss=0
      if (( ms < BEST[$b:$k] )); then BEST[$b:$k]=$ms; fi
      if (( rss > 0 && rss < RSS[$b:$k] )); then RSS[$b:$k]=$rss; fi
    done
  done
  echo "# passe $p faite"
done

echo "=== minima sur $PASSES passes alternees (ms / RSS pic Mo) ==="
for b in $BINS; do
  for k in $ORDER; do
    echo "$(basename $b) $k ${BEST[$b:$k]} ms $(( RSS[$b:$k] / 1048576 )) Mo"
  done
done
