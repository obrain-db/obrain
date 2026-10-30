#!/bin/zsh
# Duel de LOAD entre allocateurs à échelle réduite : rondes ALTERNÉES entre
# binaires (sys → jemalloc → mimalloc à chaque ronde), minima du temps réel et
# du RSS pic retenus. Base jetée après chaque load (une seule base à la fois).
# Usage: duel_load_allocateurs.sh <dump> <rondes> <bin1> <bin2> [bin3...]
set -u
DUMP=$1; RONDES=$2; shift 2
BINS=($@)
DB=/tmp/t7-duel-db

typeset -A TBEST RBEST
for b in $BINS; do TBEST[$b]=999999999; RBEST[$b]=999999999999; done

for r in $(seq 1 $RONDES); do
  for b in $BINS; do
    rm -rf $DB
    OUT=$(/usr/bin/time -l $b -q data load $DUMP $DB 2>&1)
    ms=$(echo $OUT | grep -oE '[0-9]+\.[0-9]+ real' | awk '{printf "%d", $1*1000}')
    rss=$(echo $OUT | grep -oE '[0-9]+  maximum resident' | awk '{print $1}')
    echo "# ronde $r $(basename $b): ${ms} ms, RSS $(( rss / 1048576 )) Mo"
    if (( ms < TBEST[$b] )); then TBEST[$b]=$ms; fi
    if (( rss < RBEST[$b] )); then RBEST[$b]=$rss; fi
  done
done
rm -rf $DB

echo "=== duel load $(basename $DUMP) — minima sur $RONDES rondes alternees ==="
for b in $BINS; do
  echo "$(basename $b) load ${TBEST[$b]} ms RSS $(( RBEST[$b] / 1048576 )) Mo"
done
