#!/usr/bin/env bash
# index-code-projects.sh — insère les projets CODE dans un store runtime (mono-store).
# Usage : --plan | --apply --store <runtime-prod|runtime-dev|path> [--repos "a b c"]
# Hors ligne recommandé (stores verrouillés par les webs) : géré via detach+stop si --apply.
set -u
DATA=$HOME/.local/share/obrain-harness/data
BIN=$HOME/.local/share/obrain-harness/bin/obrain
STORES="${STORES:-runtime-prod.obrain runtime-dev.obrain}"
REPOS="${REPOS:-/home/triviere/projects/banc-obrain /home/triviere/projects/deepseek-harness /home/triviere/projects/aletheia}"
MODE="${1:---plan}"
case "$MODE" in --plan|--apply) ;; *) echo "usage: $0 [--plan|--apply]"; exit 2;; esac
# detach (l'exécuteur vit dans dsh-dev-3083) — identique au sync
if [ -z "${IDX_DETACHED:-}" ] && [ "$MODE" = "--apply" ]; then
  exec systemd-run --user --collect --wait --pipe --unit=index-code-projects-$$ \
       --setenv=IDX_DETACHED=1 --setenv=STORES="$STORES" --setenv=REPOS="$REPOS" "$0" "$@"
fi
echo "== indexation code -> [$STORES] ($MODE) =="
for st in $STORES; do [ -d "$DATA/$st" ] || { echo "store absent: $st"; exit 2; }; done
for r in $REPOS; do [ -d "$r" ] && echo "repo: $r" || echo "repo ABSENT: $r"; done
if [ "$MODE" = "--plan" ]; then echo "PLAN : arrêt webs (detach), index_repository par repo dans CHAQUE store, restart, validation list_projects par store."; exit 0; fi
systemctl --user stop dsh-web.service dsh-dev-3083.service
for st in $STORES; do
  ST="$DATA/$st"
  for r in $REPOS; do
    [ -d "$r" ] || continue
    echo "-- [$st] index $r"
    echo "{\"tool\":\"index_repository\",\"args\":{\"path\":\"$r\"}}" \
      | env OBRAIN_WRITE=1 OBRAIN_STORE="$ST" "$BIN" bridge 2>/dev/null | python3 -c 'import sys,json;d=json.loads(sys.stdin.read());c=d.get("structuredContent");print(c or d.get("content","?"))'
  done
  echo "-- [$st] list_projects:"
  printf '{"tool":"list_projects","args":{}}\n' | env OBRAIN_STORE="$ST" "$BIN" bridge 2>/dev/null | python3 -c 'import sys,json;print([p["name"][:60] for p in json.loads(sys.stdin.read()).get("structuredContent",{}).get("projects",[])])'
done
systemctl --user start dsh-web.service && sleep 4 && systemctl --user start dsh-dev-3083.service
echo "ROLLBACK : les stores indexés restent lisibles ; aucune suppression (indexation additive)."
