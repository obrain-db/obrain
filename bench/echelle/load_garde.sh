#!/bin/zsh
# Load sous garde-fou disque : tue le load si l'espace libre passe sous SEUIL_GO.
# Usage: load_garde.sh <binaire> <dump> <db> [seuil_go=3]
set -u
OB=$1; DUMP=$2; DB=$3; SEUIL=${4:-3}
rm -rf $DB
/usr/bin/time -l $OB data load $DUMP $DB &
LPID=$!
while kill -0 $LPID 2>/dev/null; do
  AV=$(df -g /tmp | tail -1 | awk '{print $4}')
  if (( AV < SEUIL )); then
    echo "!!! GARDE-FOU: ${AV} Go libres < ${SEUIL} Go — arret du load"
    kill $LPID
    wait $LPID
    exit 42
  fi
  sleep 10
done
wait $LPID
RC=$?
echo "load rc=$RC ; taille base: $(du -sh $DB 2>/dev/null | cut -f1) ; df: $(df -g /tmp | tail -1 | awk '{print $4}') Go libres"
exit $RC
