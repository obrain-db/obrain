#!/usr/bin/env bash
# perf-snapshot.sh — re-mesure périodique anti-régression (µs/ms) sur stores réels.
# Usage: bash scripts/bench-verify/perf-snapshot.sh   (sortie stable, à logger à chaque jalon)
set -u
cd "$(dirname "$0")/../.."
BIN=./target/release/obrain
[ -x "$BIN" ] || { echo "build d'abord: cargo build --release -p obrain-cli"; exit 2; }
ENG=/tmp/engine.obrain
IDX=/tmp/engine_idx.obrain
[ -d "$IDX" ] || cp -a "$ENG" "$IDX"
meas() { # meas <store> <json-line> -> engine µs (médiane sur N)
  local store="$1" line="$2" n=30 ems=0 ok=0
  for i in $(seq 1 $n); do
    r=$(printf '%s\n' "$line" | env OBRAIN_STORE="$store" "$BIN" bridge 2>/dev/null | python3 -c 'import sys,json
try:
 d=json.loads(sys.stdin.read()); c=d.get("structuredContent") or {}
 print(c.get("status",""), c.get("execution_time_ms",-1))
except: print("err -1")')
    st=$(echo "$r"|cut -d' ' -f1); t=$(echo "$r"|cut -d' ' -f2)
    if [ "$st" = "ok" ] && [ "${t%%.*}" != "-1" ]; then ok=$((ok+1)); ems=$(python3 -c "print($ems+$t)"); fi
  done
  [ "$ok" -gt 0 ] && python3 -c "print(f'{$ems/$ok*1000:.0f} µs')" || echo "n/a"
}
echo "== perf snapshot $(date -u +%FT%TZ) =="
echo -n "count (4k Function)            : "; meas "$ENG" '{"tool":"query","args":{"query":"MATCH (n:Function) RETURN count(n) AS n","language":"gql"}}'
echo -n "lookup indexé (qualified_name) : "; meas "$IDX" '{"tool":"query","args":{"query":"MATCH (n) WHERE n.qualified_name = '\''home.triviere.projects.banc_obrain.crates.obrain_engine.src.query.test_language'\'' RETURN n.qualified_name","language":"gql"}}'
echo -n "pushdown 3-sauts (count DIST)  : "; meas "$ENG" '{"tool":"query","args":{"query":"MATCH (a:Function)-[:CALLS]->(b:Function)-[:CALLS]->(c:Function)-[:CALLS]->(d:Function) RETURN count(DISTINCT d) AS d","language":"gql"}}'
echo -n "reachable_count(1..2)          : "; meas "$ENG" '{"tool":"query","args":{"query":"CALL obrain.reachable_count({from_label:'\''Function'\'', edge_type:'\''CALLS'\'', min_hops:1, max_hops:2}) YIELD hop_count, distinct_count RETURN hop_count, distinct_count","language":"gql"}}'
