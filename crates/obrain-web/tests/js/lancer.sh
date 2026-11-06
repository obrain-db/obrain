#!/usr/bin/env bash
# Lance la batterie du paquet obrain-web : node PUIS Chrome headless.
# Usage : tests/js/lancer.sh <dossier racine contenant pkg/ et pkg-web/>
set -euo pipefail
RACINE="${1:-.}"
cd "$RACINE"
echo "=== node ==="
node crates/obrain-web/tests/js/node.mjs pkg | tail -4
echo "=== Chrome headless ==="
CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
PORT="${PORT:-8765}"
python3 -m http.server "$PORT" --bind 127.0.0.1 > /dev/null 2>&1 &
SERVEUR=$!
trap 'kill $SERVEUR 2>/dev/null || true' EXIT
sleep 1
SORTIE=$("$CHROME" --headless=new --no-sandbox --disable-gpu --virtual-time-budget=180000 \
  --dump-dom "http://127.0.0.1:$PORT/crates/obrain-web/tests/js/navigateur.html" 2>/dev/null \
  | grep -o '<pre id="sortie">[^<]*' | sed 's/<pre id="sortie">//; s/&quot;/"/g; s/&amp;/\&/g')
python3 -I - "$SORTIE" <<'PY'
import json, sys
t = sys.argv[1]
if not t.startswith("RAPPORT "):
    print("ECHEC navigateur :", t[:400]); sys.exit(1)
r = json.loads(t[len("RAPPORT "):])
for l in r["journal"]: print(l)
print(f"\n{r['ok']}/{r['total']} tests passés, {len(r['ignores'])} ignorés, {len(r['echecs'])} échecs, {r['duree_ms']} ms")
sys.exit(0 if not r["echecs"] else 1)
PY
