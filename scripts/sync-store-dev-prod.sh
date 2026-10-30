#!/usr/bin/env bash
# sync-store-dev-prod.sh — rendre PROD « tout présent » (sessions/tissage de dev).
# Mode sûr : --plan (lecture seule) | --apply (nécessite confirmation explicite).
# Principe : dev est le superset vivant (threads récents) → prod prend une copie
# reflink de dev ; l'ancien store prod est archivé (rollback immédiat possible).
set -u
DATA=$HOME/.local/share/obrain-harness/data
PROD="$DATA/runtime-prod.obrain"; DEV="$DATA/runtime-dev.obrain"
BIN=$HOME/.local/share/obrain-harness/bin/obrain
MODE="${1:---plan}"
case "$MODE" in --plan|--apply) ;; *) echo "usage: $0 [--plan|--apply]"; exit 2;; esac

# DÉTACHEMENT : l'exécuteur vit dans le cgroup du service qui va être stoppé.
# Sans cela, `--apply` se suicide à l'étape 1. Relance via systemd-run (hors cgroup).
if [ -z "${SYNC_DETACHED:-}" ] && [ "$MODE" = "--apply" ]; then
  echo "détachement (systemd-run) pour survivre à l'arrêt des services…"
  exec systemd-run --user --collect --wait --pipe        --unit=sync-store-dev-prod-$$ --setenv=SYNC_DETACHED=1 "$0" "$@"
fi

echo "== sync dev -> prod ($MODE) =="
[ -d "$DEV" ] && [ -d "$PROD" ] || { echo "stores absents"; exit 2; }
for n in prod dev; do
  s=$([ $n = prod ] && echo "$PROD" || echo "$DEV")
  echo -n "$n sessions: "; printf '{"tool":"query","args":{"query":"MATCH (n:Session) RETURN count(n) AS c","language":"gql"}}\n' | env OBRAIN_STORE="$s" "$BIN" bridge 2>/dev/null | python3 -c 'import sys,json;print(json.load(sys.stdin).get("structuredContent",{}).get("rows"))' 2>/dev/null || echo '?'
done
echo -n "dev contient le thread récent ? "; printf '{"tool":"query","args":{"query":"MATCH (n:Message) WHERE n.qualified_name CONTAINS '\''89ff3444'\'' RETURN count(n) AS c","language":"gql"}}\n' | env OBRAIN_STORE="$DEV" "$BIN" bridge 2>/dev/null | python3 -c 'import sys,json;print(json.load(sys.stdin).get("structuredContent",{}).get("rows"))' 2>/dev/null

if [ "$MODE" = "--plan" ]; then
  echo "PLAN : archive prod -> runtime-prod.obrain.bak-sync ; cp --reflink dev -> prod ; restart dsh-web puis dsh-dev-3083 ; validation counts ; rollback = swap .bak."
  echo "(--apply : arrêt ~1 min des deux webs (dont dev). Exécuté DÉTACHÉ pour ne pas se tuer avec le service ; ne pas lancer depuis une session hébergée par dev sans ce détachement.)"
  exit 0
fi
# --- --apply : exécution (explicite) ---
TS=$(date +%Y%m%d-%H%M%S)
echo "[1/5] arrêt services"; systemctl --user stop dsh-web.service dsh-dev-3083.service
echo "[2/5] archive prod"; mv "$PROD" "$PROD.bak-sync-$TS"
echo "[3/5] copie dev -> prod"; cp -a --reflink=auto "$DEV" "$PROD"
echo "[4/5] démarrage prod puis dev"; systemctl --user start dsh-web.service && sleep 5 && systemctl --user start dsh-dev-3083.service
echo "[5/5] validation"; for i in 1 2 3 4 5; do curl -s -o /dev/null -w "3082:%{http_code} " http://127.0.0.1:3082/; curl -s -o /dev/null -w "3083:%{http_code}\n" http://127.0.0.1:3083/; sleep 3; done
echo "ROLLBACK : systemctl stop dsh-web dsh-dev-3083 ; mv $PROD /tmp ; mv $PROD.bak-sync-$TS $PROD ; systemctl start dsh-web dsh-dev-3083"
