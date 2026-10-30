#!/usr/bin/env bash
# FENETRE DE MAINTENANCE — courbure de Ricci sur les 8 bases servies.
#
# Pourquoi une fenetre : le `viz serve` tient les 8 stores en verrou EXCLUSIF en
# permanence (mesure du 2026-09-10 : pid unique detenteur des huit). Tant qu'il
# tourne, `enrich --ricci` echoue en 0 s sur chaque base. Il n'y a donc pas
# d'entretien possible « a cote » : il faut une fenetre.
#
# Deroulement :
#   1. enregistrer de quoi relancer le dashboard (pid, cmdline, cwd) ;
#   2. arreter les detenteurs PAR PID, lus depuis fuser — JAMAIS par motif :
#      un `pkill -f "obrain viz serve"` matche sa propre ligne de commande quand
#      le motif y figure, et le script se tire dessus (constate deux fois le
#      2026-09-10, mon shell est mort) ;
#   3. enrichir les 8 bases, avec reprises : les ponts MCP sont respawnes par le
#      harness et rattrapent le store par defaut en quelques secondes ;
#   4. RELANCER le dashboard. C'est garanti par `trap ... EXIT` : quel que soit le
#      chemin de sortie — succes, erreur, SIGTERM, Ctrl-C — le dashboard remonte.
#      Ne jamais laisser la fenetre se refermer sans lui.
#
# Idempotent : `edges_updated: 0` signifie « deja peuplee », pas « echec ».
set -uo pipefail

OBAIN="/home/triviere/projects/banc-obrain/target/release/obrain"
REPO="/home/triviere/projects/banc-obrain"
DATA="/home/triviere/.local/share/obrain-harness/data"
LOG="${OBRAIN_ENRICH_LOG:-/tmp/obrain-enrich-window.log}"
LOCK="/tmp/obrain-enrich-window.lock"
PORT=8432
HOST=192.168.1.103

BASES=(
  "$DATA/agent-synapse-triviere-dsh.obrain"
  "$DATA/self-demo.obrain"
  "$DATA/runtime-prod.obrain"
  "$DATA/runtime-dev.obrain"
  "/tmp/linux-full3.obrain"
  "/tmp/kv-linux-e1000.obrain"
  "/tmp/engine_idx.obrain"
  "/tmp/kv-linux-driversnet.obrain"
)

say(){ echo "[$(date -Is)] $*" >> "$LOG"; }
log(){ echo "$*" >> "$LOG"; }

# --- une seule fenetre a la fois ---
exec 9>"$LOCK"
flock -n 9 || { say "une autre passe est en cours — abandon"; exit 0; }

# --- 1. enregistrer le dashboard AVANT de l'arreter ---
VIZ_PID=""
for B in "${BASES[@]}"; do
  h="$(fuser "$B/substrate.meta" 2>/dev/null | tr ' ' '\n' | grep -E '^[0-9]+$')"
  for pid in $h; do
    # /proc/<pid>/cmdline separe les arguments par des NUL : le motif litteral
    # "viz serve" (avec espace) ne peut PAS y matcher. On traduit d'abord.
    if tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null | grep -q "viz serve"; then VIZ_PID="$pid"; fi
  done
done
if [ -n "$VIZ_PID" ]; then
  say "dashboard detecte : pid $VIZ_PID"
  tr '\0' ' ' < "/proc/$VIZ_PID/cmdline" > /tmp/obrain-viz-cmdline.txt 2>/dev/null
  readlink "/proc/$VIZ_PID/cwd" > /tmp/obrain-viz-cwd.txt 2>/dev/null
else
  say "aucun dashboard en cours d'execution"
fi

# --- 4. relance garantie, sur tout chemin de sortie ---
restart_viz(){
  if ss -lnt 2>/dev/null | grep -q ":$PORT"; then say "dashboard deja en ligne"; return; fi
  say "relance du dashboard"
  cd "$REPO" || return
  setsid nohup bash scripts/vizserve.sh >> /tmp/vizserve-window.log 2>&1 < /dev/null &
  for _ in $(seq 1 30); do
    ss -lnt 2>/dev/null | grep -q ":$PORT" && break
    sleep 1
  done
  if ss -lnt 2>/dev/null | grep -q ":$PORT"; then say "dashboard EN LIGNE sur $PORT"; else say "!! dashboard NON remonte"; fi
}
trap restart_viz EXIT INT TERM

# --- 2. arreter les detenteurs, par PID ---
say "ouverture de la fenetre"
for essai in 1 2 3 4 5 6 7 8; do
  pids=""
  for B in "${BASES[@]}"; do
    [ -e "$B/substrate.meta" ] || continue
    h="$(fuser "$B/substrate.meta" 2>/dev/null | tr ' ' '\n' | grep -E '^[0-9]+$' | grep -v "^$$\$")"
    pids="$pids $h"
  done
  pids="$(echo "$pids" | tr ' ' '\n' | sort -u | grep -E '^[0-9]+$' | tr '\n' ' ')"
  [ -z "$pids" ] && { say "essai $essai : verrous LIBRES"; break; }
  say "essai $essai : arrestation de$pids"
  for pid in $pids; do kill -TERM "$pid" 2>/dev/null; done
  sleep 3
done

# --- 3. enrichir, avec reprises serrees sur les bases tenues ---
for B in "${BASES[@]}"; do
  [ -d "$B" ] || { say "ABSENT $B"; continue; }
  rc=1
  for essai in $(seq 1 12); do
    h="$(fuser "$B/substrate.meta" 2>/dev/null | tr ' ' '\n' | grep -E '^[0-9]+$' | grep -v "^$$\$")"
    if [ -n "$h" ]; then for pid in $h; do kill -TERM "$pid" 2>/dev/null; done; sleep 1; fi
    s=$(date +%s)
    if "$OBAIN" enrich --ricci "$B" >> "$LOG" 2>&1; then
      ar=$(grep -o '"edges_updated": "[0-9]*"' "$LOG" | tail -1 | grep -o '[0-9]*')
      say "OK $(basename "$B") $(( $(date +%s) - s ))s arêtes=$ar"
      rc=0; break
    fi
    sleep 1
  done
  [ "$rc" -ne 0 ] && say "ECHEC $(basename "$B") — verrou non obtenu"
done

say "fermeture de la fenetre"
# rotation douce
if [ -f "$LOG" ] && [ "$(stat -c%s "$LOG")" -gt 5000000 ]; then tail -2000 "$LOG" > "$LOG.tmp" && mv "$LOG.tmp" "$LOG"; fi
exit 0
