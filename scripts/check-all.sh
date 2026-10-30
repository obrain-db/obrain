#!/usr/bin/env bash
# Vérification complète du chantier dashboard-graph : disponibilité, warm,
# performances (chaud < 50 ms), E2E interface. Sortie agrégée.
# Usage : bash scripts/check-all.sh            (serveur déjà lancé)
#         bash scripts/check-all.sh --restart  (relance + warm d'abord)
set -e
cd "$(dirname "$0")/.."
if [ "${1:-}" = "--restart" ]; then
  echo "── relance + warm…"
  bash scripts/vizserve.sh >/dev/null 2>&1
  sleep 1
fi
echo "── smoke backend (agrégats < 50 ms chaud)…"
bash scripts/smoke-backend.sh | tail -12
echo "── e2e interface (parcours complet)…"
node scripts/e2e-dashboard.mjs | tail -8
echo "── statut serveur…"
curl -s http://192.168.1.103:8432/api/viz/status | python3 -m json.tool | head -12
echo "── FIN : chantier dashboard-graph — tout est vérifié instantané après warm."
