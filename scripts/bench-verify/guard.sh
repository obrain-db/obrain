#!/usr/bin/env bash
# Gardien d'autonomie : re-mesure + suites ; exit≠0 si un chiffre annoncé dévie.
set -u
cd "$(dirname "$0")/../.."
FAIL=0
cp target/release/libobrain_node.so /tmp/rtprobe.node
echo "== suites =="
cargo test -p obrain-substrate --lib -q 2>&1 | grep -E 'test result' | tail -1
cargo test -p obrain-engine --lib -q 2>&1 | grep -E 'test result' | tail -1
cargo test -p obrain-cli --features cypher,indexer --bin obrain -- mcp_server 2>&1 | grep -E 'test result' | tail -1   # surface MCP bridge (26 outils, §31)
echo "== re-mesures =="
echo "== fraîcheur index code (relief du code à jour) =="
cargo build --release -p obrain-indexer --example persist 2>&1 | grep -E '^error|Finished' | tail -1
FRESH=$(mktemp -d)/fresh.obrain
./target/release/examples/persist crates/obrain-engine "$FRESH" >/dev/null 2>&1
FH=$(printf '{"tool":"search_graph","args":{"query":"reachable_count","limit":5}}' | env OBRAIN_STORE="$FRESH" ./target/release/obrain bridge 2>/dev/null | python3 -c 'import sys,json
try:
    d=json.loads(sys.stdin.read()); c=d.get("structuredContent") or {}
    rows=c.get("rows") or []
    print(sum(1 for r in rows if "reachable_count" in str(r)))
except Exception: print(-1)')
echo "hits reachable_count (code frais) : $FH"
[ "$FH" -ge 1 ] 2>/dev/null || { echo "RÉGRESSION fraîcheur index code (persist échoué ou symbole absent)"; FAIL=1; }
O1=$(node scripts/bench-verify/verify-o1.mjs 2>&1 | grep -v Experimental)
echo "$O1"
echo "$O1" | grep -qE 'O\(1\) 0\.[0-3][0-9] µs' || { echo "RÉGRESSION O(1)"; FAIL=1; }
E=$(node scripts/bench-verify/verify-engine.mjs 2>&1 | grep -v Experimental)
echo "$E" | grep -q 'exact==sorted : true' || { echo "RÉGRESSION fenêtres"; FAIL=1; }
echo "$E" | grep -qE 'GQL count\(n\)\s+= 445493' || { echo "RÉGRESSION count"; FAIL=1; }
echo "== ingestion native =="
rm -rf /tmp/store-guard.obrain
I=$(target/release/obrain native-import /tmp/store-guard.obrain /tmp/corpus.bin 2>&1 | tail -1)
echo "$I"
T=$(echo "$I" | sed -nE 's/.*en ([0-9.]+) s.*/\1/p')
ok=$(awk -v t="$T" 'BEGIN{ print (t+0 < 0.6) ? 1 : 0 }')
[ "$ok" = "1" ] || { echo "RÉGRESSION ingestion ($T s ≥ 0,6)"; FAIL=1; }
echo "== résolution sémantique (M6 frais vs C, seuils 50/50) =="
SEM=$(SEMANTIC_MIN_PRECISION=0.60 SEMANTIC_MIN_RECALL=0.70 SEMANTIC_STORE="$FRESH" bash scripts/bench-verify/semantic-resolution.sh 2>&1); SEXIT=$?
echo "$SEM" | grep -E 'RÉSUMÉ|GARDIEN' | head -4
if [ "$SEXIT" -ne 0 ]; then
    echo "RÉGRESSION résolution sémantique (exit=$SEXIT : metrics sous seuil precision/recall ≥ 50 % ou bench inopérant — db/store/binaire absents)"
    FAIL=1
fi
if [ "$FAIL" -eq 0 ]; then echo "GARDIEN : OK — aucun chiffre annoncé ne dévie"; else echo "GARDIEN : ÉCHEC"; fi
exit $FAIL
