#!/usr/bin/env bash
# bench-verify — imports & comparaisons obrain (mesurer les gains, machine n3).
# Références (min de runs, n3, 2026-09-05) :
#   native-import corpus.bin (445 493 n / 1 670 005 e) : moteur ~0,26 s (wall ~0,41 s)
#   enrich --ricci code-all3 (1,67 M) : ~3,2 s moteur (wall ~4 s)
#   compare paire 37 k/247 k  : froid ~85 ms · chaud(empreintes) ~40 ms · mémorisé 0 ms
#   compare gros 443 k/1,67 M : froid ~1,1 s · chaud ~0,4 s · mémorisé 0 ms
# Usage : scripts/bench-verify/verify-imports-compare.sh [--strict]
set -uo pipefail
cd "$(dirname "$0")/../.."
BIN=${OBRAIN_BIN:-./target/release/obrain}
[ -x "$BIN" ] || { echo "construire d'abord : cargo build --release -p obrain-cli"; exit 2; }

T="$(mktemp -d)/vb"        # stores temporaires
PAIR_A=/tmp/ve.obrain; PAIR_B=/tmp/vg.obrain
REF_NI=0.6; REF_ENRICH=4.5; REF_CMP_PAIR_MS=100; REF_CMP_BIG_MS=600
STRICT=0; [ "${1:-}" = "--strict" ] && STRICT=1

ms() { /usr/bin/time -f "%e" "$@" 2>&1 >/dev/null | tail -1; }

echo "== 1) native-import (corpus.bin : 445 493 nœuds / 1 670 005 arêtes) =="
if [ -f /tmp/corpus.bin ]; then
  rm -rf "$T-nat.obrain"
  t0=$(date +%s%N); "$BIN" native-import "$T-nat.obrain" /tmp/corpus.bin >/dev/null 2>&1
  t1=$(date +%s%N); sec=$(awk "BEGIN{printf \"%.3f\",($t1-$t0)/1e9}")
  echo "  moteur : ${sec} s   (réf ~0,26 s ; cible ≤ ${REF_NI} s)"
  "$BIN" query "$T-nat.obrain" "MATCH (n) RETURN count(*) AS c" --format json 2>/dev/null | grep '"c"' | head -1
else
  echo "  corpus absent (/tmp/corpus.bin) — ignoré"
fi

echo "== 2) enrich --ricci (copie code-all3 : 445 k nœuds / 1,67 M arêtes) =="
if [ -d target/bench/real/code-all3.obrain ]; then
  rm -rf "$T-enrich.obrain"; cp -a --reflink=auto target/bench/real/code-all3.obrain "$T-enrich.obrain"
  "$BIN" enrich "$T-enrich.obrain" --ricci 2>/dev/null | grep -E "edges_updated|edges_visited" | head -1
  sec=$(ms "$BIN" enrich "$T-enrich.obrain" --ricci 2>/dev/null)
  echo "  wall : ${sec} s   (réf moteur ~3,2 s / wall ~4 s ; cible ≤ ${REF_ENRICH} s)"
else
  echo "  store code-all3 absent — ignoré"
fi

echo "== 3) compare paire réelle (worktrees obrain : 35-38 k nœuds / ~247 k arêtes) =="
wall_ms() { local t0 t1; t0=$(date +%s%N); "$@" >/dev/null 2>&1; t1=$(date +%s%N); awk "BEGIN{printf \"%.1f\", ($t1-$t0)/1e6}"; }
run_pair() { wall_ms "$BIN" compare "$1" "$2" --prefix-a obrain-engine --prefix-b tmp-grafeo-cbm; }
run_big()  { wall_ms "$BIN" compare "$1" "$2"; }
if [ -d "$PAIR_A" ] && [ -d "$PAIR_B" ]; then
  rm -f "$PAIR_A"/substrate.compare-* "$PAIR_B"/substrate.compare-*
  # warm-up (cache OS) — ne compte pas.
  "$BIN" compare "$PAIR_A" "$PAIR_B" --prefix-a obrain-engine --prefix-b tmp-grafeo-cbm >/dev/null 2>&1
  c1=$(run_pair "$PAIR_A" "$PAIR_B")   # recalcule (froid)
  c2=$(run_pair "$PAIR_A" "$PAIR_B")   # empreintes (chaud)
  c3=$(run_pair "$PAIR_A" "$PAIR_B")   # mémorisé attendu ~0
  echo "  froid ${c1} ms · chaud ${c2} ms · 3e run ${c3} ms  (réf moteur ~85/~40/0 ; cible chaud ≤ ${REF_CMP_PAIR_MS} ms)"
else
  echo "  stores variantes absents — créer : node scripts/code-index-to-substrate.mjs $PAIR_A <dbA> …"
fi

echo "== 4) compare gros (code-all vs code-all2 : 443 k nœuds / 1,67 M arêtes) =="
if [ -d target/bench/real/code-all.obrain ] && [ -d target/bench/real/code-all2.obrain ]; then
  "$BIN" compare target/bench/real/code-all.obrain target/bench/real/code-all2.obrain >/dev/null 2>&1 # warm-up
  d1=$(run_big target/bench/real/code-all.obrain target/bench/real/code-all2.obrain)
  d2=$(run_big target/bench/real/code-all.obrain target/bench/real/code-all2.obrain)
  echo "  froid ${d1} ms · chaud ${d2} ms  (réf ~1100/~400 ; cible chaud ≤ ${REF_CMP_BIG_MS} ms)"
else
  echo "  stores code-all/code-all2 absents — ignoré"
fi

rm -rf "$(dirname "$T")"
echo "== fin (nettoie les stores temporaires) =="
