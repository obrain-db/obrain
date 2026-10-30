#!/usr/bin/env bash
# Smoke backend « bases » : disponibilité + chaud < 50 ms + cohérence.
# Usage : bash scripts/smoke-backend.sh [BASE]
set -e
BASE="${1:-http://192.168.1.103:8432}"
RT="/home/triviere/.local/share/obrain-harness/data/runtime-dev.obrain"
LIN="/tmp/linux-full3.obrain"
FAIL=0
fast(){ python3 -c "import sys; sys.exit(0 if float('$1') < 0.05 else 1)" 2>/dev/null; }
t(){ printf '%-38s %s\n' "$1" "$2"; }
ck(){ [ "$2" = "$3" ] && t "$1" "OK ($3)" || { t "$1" "ÉCHEC attendu $3 obtenu $2"; FAIL=1; }; }

c=$(curl -s -o /dev/null -w '%{http_code}' "$BASE/"); ck "racine" "$c" 200
c=$(curl -s -o /dev/null -w '%{http_code}' "$BASE/sitemap.xml"); ck "sitemap.xml" "$c" 200
python3 -c "import xml.dom.minidom,sys;xml.dom.minidom.parse(sys.stdin)" < <(curl -s "$BASE/sitemap.xml") \
  && t "sitemap XML bien formé" "OK" || { t "sitemap XML bien formé" "ÉCHEC"; FAIL=1; }
c=$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/viz/status"); ck "status" "$c" 200

for ep in "modules?top=200" "space?prefix=deepseek_harness&rank=4&top=60&links=0" "overview"; do
  curl -s -o /dev/null --get --data-urlencode "store=$RT" "$BASE/api/viz/$ep" >/dev/null 2>&1 || true
  time=$(curl -s -o /dev/null -w '%{time_total}' --get --data-urlencode "store=$RT" "$BASE/api/viz/$ep")
  if fast "$time"; then t "$ep runtime (chaud $time s)" "OK"; else t "$ep runtime (chaud $time s)" "LENT"; FAIL=1; fi
done
for ep in "arcs?top=48&limit=320" "overview" "space?prefix=drivers&rank=3&top=14&links=0"; do
  curl -s -o /dev/null --get --data-urlencode "store=$LIN" "$BASE/api/viz/$ep" >/dev/null 2>&1 || true
  time=$(curl -s -o /dev/null -w '%{time_total}' --get --data-urlencode "store=$LIN" "$BASE/api/viz/$ep")
  if fast "$time"; then t "$ep linux (chaud $time s)" "OK"; else t "$ep linux (chaud $time s)" "LENT"; FAIL=1; fi
done
Q="MATCH ()-[r]->() RETURN type(r) AS t, count(r) AS n ORDER BY n DESC LIMIT 5"
curl -s -o /dev/null --get --data-urlencode "store=$RT" --data-urlencode "query=$Q" --data-urlencode "language=gql" "$BASE/api/query" >/dev/null 2>&1 || true
time=$(curl -s -o /dev/null -w '%{time_total}' --get --data-urlencode "store=$RT" --data-urlencode "query=$Q" --data-urlencode "language=gql" "$BASE/api/query")
fast "$time" && t "query cachée (chaud $time s)" "OK" || { t "query cachée (chaud $time s)" "LENT"; FAIL=1; }

[ "$FAIL" = 0 ] && t "RÉSULTAT" "SMOKE OK" || t "RÉSULTAT" "ÉCHECS DÉTECTÉS"
exit "$FAIL"
