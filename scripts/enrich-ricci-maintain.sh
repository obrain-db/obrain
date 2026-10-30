#!/usr/bin/env bash
# Maintien de la courbure de Ricci sur toutes les bases servies par le dashboard.
#
# Contrainte REELLE, mesuree le 2026-09-10 : plusieurs processus tiennent les
# stores en verrou EXCLUSIF — le `viz serve` pour les 7 presets, et les ponts MCP
# pour runtime-dev / runtime-prod. Une passe qui tente d'ouvrir un store verrouille
# echoue en 0 s.
#
# Ce script ne tue RIEN. Il enrichit ce qui est libre et journalise ce qu'il saute,
# avec les PID fautifs, pour que le diagnostic soit immediat. Consequence assumee :
# les bases tenues en permanence ne seront jamais rattrapees automatiquement — elles
# exigent une fenetre manuelle (kill par PID -> enrich -> relance du dashboard),
# et c'est ecrit ici pour que personne ne croie a une panne du timer.
#
# Idempotent : relancer ne coute rien, et `edges_updated: 0` est la PREUVE qu'une
# base est deja peuplee (une base non enrichie rapporte des aretes a mettre a jour).
set -uo pipefail

OBAIN="${OBRAIN_BIN:-/home/triviere/projects/banc-obrain/target/release/obrain}"
DATA="/home/triviere/.local/share/obrain-harness/data"
LOG="${OBRAIN_ENRICH_LOG:-/tmp/obrain-enrich-ricci.log}"
LOCK="${OBRAIN_ENRICH_LOCK:-/tmp/obrain-enrich-ricci.lock}"

# une seule instance a la fois : deux passes concurrentes se marcheraient dessus
exec 9>"$LOCK"
flock -n 9 || { echo "[$(date -Is)] deja en cours — abandon" >> "$LOG"; exit 0; }

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

echo "[$(date -Is)] passe — ${#BASES[@]} bases" >> "$LOG"
for B in "${BASES[@]}"; do
  if [ ! -d "$B" ]; then echo "[$(date -Is)] ABSENT $B" >> "$LOG"; continue; fi
  held="$(fuser "$B/substrate.meta" 2>/dev/null | tr -s ' ')"
  if [ -n "$held" ]; then
    echo "[$(date -Is)] SKIP $(basename "$B") — verrou tenu par:$held" >> "$LOG"
    continue
  fi
  s=$(date +%s)
  if "$OBAIN" enrich --ricci "$B" >> "$LOG" 2>&1; then
    echo "[$(date -Is)] OK $(basename "$B") $(( $(date +%s) - s ))s" >> "$LOG"
  else
    echo "[$(date -Is)] ECHEC $(basename "$B")" >> "$LOG"
  fi
done

# rotation douce : on garde les 2000 dernieres lignes au-dela de 5 Mo
if [ -f "$LOG" ] && [ "$(stat -c%s "$LOG")" -gt 5000000 ]; then
  tail -2000 "$LOG" > "$LOG.tmp" && mv "$LOG.tmp" "$LOG"
fi
