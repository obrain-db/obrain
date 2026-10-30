#!/bin/zsh
# Banc des 5 formes de requêtes — minima sur P passes (machine partagée : jamais de moyenne).
# Usage: bench_requetes.sh <binaire obrain> <db> [passes=7] [etiquette]
set -u
OB=$1
DB=$2
PASSES=${3:-7}
TAG=${4:-bench}

typeset -A Q
Q[q1_lookup_nom]="MATCH (n:Function) WHERE n.name = 'compare_values' RETURN n.qualified_name"
Q[q2_appelants_1saut]="MATCH (a)-[:CALLS]->(b:Function) WHERE b.name = 'compare_values' RETURN count(*)"
Q[q3_count_label]="MATCH (n:Function) RETURN count(*)"
Q[q4_orderby_desc_limit]="MATCH (a)-[:CALLS]->(b:Function) RETURN b.name, count(*) AS c ORDER BY c DESC LIMIT 5"
Q[q5_scan_filtre]="MATCH (n:Function) WHERE n.start_line > 200 RETURN count(*)"

ORDER=(q1_lookup_nom q2_appelants_1saut q3_count_label q4_orderby_desc_limit q5_scan_filtre)

typeset -A BEST
for k in $ORDER; do BEST[$k]=99999999; done

# Passes alternées : on boucle sur les formes à chaque passe (pas 7x la même d'affilée)
for p in $(seq 1 $PASSES); do
  for k in $ORDER; do
    t0=$(python3 -c 'import time;print(time.monotonic_ns())')
    OUT=$($OB --format json query $DB ${Q[$k]} 2>&1)
    t1=$(python3 -c 'import time;print(time.monotonic_ns())')
    ms=$(( (t1 - t0) / 1000000 ))
    if (( ms < BEST[$k] )); then BEST[$k]=$ms; fi
    if (( p == 1 )); then
      echo "## $TAG $k passe1=${ms}ms resultat: $(echo $OUT | head -c 200)"
    fi
  done
done

echo "=== $TAG minima sur $PASSES passes (ms, CLI complet = ouverture+requete) ==="
for k in $ORDER; do
  echo "$TAG $k ${BEST[$k]} ms"
done
