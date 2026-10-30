#!/usr/bin/env bash
# Pré-vol prod-first (full-obrain) — MODE --check : LECTURE SEULE, aucune action.
# Exécuter juste avant la vague : valide que tout est présent pour PROD d'abord, DEV ensuite.
set -u
cd "$(dirname "$0")/.."
echo "== pré-vol prod-first — vérifications (aucune action) =="
FAIL=0
BIN=./target/release/obrain
[ -x "$BIN" ] || { echo "✗ binaire absent (cargo build --release -p obrain-cli)"; FAIL=1; }
echo -n "outils bridge : "; printf '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}\n' | env OBRAIN_STORE=/tmp/engine.obrain "$BIN" bridge 2>/dev/null | python3 -c 'import sys,json;print(len(json.load(sys.stdin)["result"]["tools"]),"outils")' 2>/dev/null || { echo "✗ bridge injoignable"; FAIL=1; }
for p in ~/.config/systemd/user/dsh-web.service.d/99-obrain-harness.conf ~/.dsh/profiles/web/cordis.patch.yml ~/.dsh-dev/profiles/web/cordis.patch.yml; do [ -f "$p" ] && echo "✓ présent: $p" || { echo "✗ absent: $p"; FAIL=1; }; done
echo -n "stores prod/dev : "; for s in ~/.local/share/obrain-harness/data/runtime-prod.obrain ~/.local/share/obrain-harness/data/runtime-dev.obrain; do [ -d "$s" ] && echo -n "ok($(basename $s)) " || { echo -n "MANQUANT($s) "; FAIL=1; }; done; echo
echo -n "git (jalons) : "; git log --oneline -1
[ "$FAIL" -eq 0 ] && echo "PRÉ-VOL : OK — tout est présent pour prod d'abord, dev ensuite" || { echo "PRÉ-VOL : ÉCHEC"; exit 1; }
