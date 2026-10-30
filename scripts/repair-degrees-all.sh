#!/usr/bin/env bash
# MET TOUTES LES BASES À NIVEAU — colonnes de degrés reconstruites.
#
# Pourquoi une séquence ATOMIQUE et non des réparations à l'unité :
# `repair-degrees` ouvre la base en EXCLUSIF (il supprime les sidecars et
# rescanne les arêtes), donc il exige que TOUS les lecteurs aient relâché. Or le
# serveur du dashboard est relancé par d'autres processus : constaté, une
# première passe a vu ses 11 bases reprises entre deux commandes, et tout a
# échoué en « already open by another process ».
#
# D'où l'ordre, identique à `scripts/weave.sh` (la doctrine du dépôt) :
#   1. arrêt PROPRE du serveur (SIGTERM, attente de sortie effective) ;
#   2. réparation de toutes les bases, sans aucun temps mort ;
#   3. relance du dashboard.
#
# Les bases tenues par un AUTRE processus (ponts MCP du harnais, synchro
# externe) ne sont pas touchées : on nomme le détenteur et on continue. Les tuer
# serait un effet de bord sur une session qui n'est pas la nôtre.
#
# Usage : scripts/repair-degrees-all.sh [--sans-relance]
set -e
cd "$(dirname "$0")/.."

DATA=/home/triviere/.local/share/obrain-harness/data
BIN=./target/release/obrain
RELANCE=1
[ "$1" = "--sans-relance" ] && RELANCE=0

# ── 1 · LA LISTE DES BASES QUI COMPTENT ────────────────────────────────────
# Les presets servis + la base synaptique de l'agent + les mondes ajoutés à
# chaud. On ne parcourt PAS tout `/tmp/*.obrain` : la plupart sont des artefacts
# de banc jetables, et « tout réparer » veut dire « tout ce qui est servi ».
BASES=()
for p in \
  "$DATA/runtime-dev.obrain" \
  "$DATA/runtime-prod.obrain" \
  "$DATA/self-demo.obrain" \
  "$DATA/agent-synapse-triviere-dsh.obrain" \
  /tmp/linux-full3.obrain \
  /tmp/kv-linux-e1000.obrain \
  /tmp/engine_idx.obrain \
  /tmp/kv-linux-driversnet.obrain
do
  [ -d "$p" ] && BASES+=("$p")
done
for p in /tmp/obrain-worlds/*.obrain; do
  [ -d "$p" ] && BASES+=("$p")
done

# ── 2 · QUI TIENT UNE BASE ? (pour nommer, jamais pour tuer) ───────────────
detenteur() {
  local cible
  cible=$(basename "$1")
  for p in $(ps -eo pid | tail -n +2); do
    if ls -l "/proc/$p/fd" 2>/dev/null | grep -q "/$cible/"; then
      echo "$p:$(tr '\0' ' ' < "/proc/$p/cmdline" 2>/dev/null | cut -c1-70)"
      return
    fi
  done
}

# ── 3 · ARRÊT PROPRE DU SERVEUR ────────────────────────────────────────────
SERVER_WAS_UP=0
if ss -ltnp 2>/dev/null | grep -qE ':843[0-9] '; then
  SERVER_WAS_UP=1
  for p in $(ss -ltnp 2>/dev/null | grep -E ':843[0-9] ' | grep -oP 'pid=\K[0-9]+' | sort -u); do
    kill -TERM "$p" 2>/dev/null || true
  done
  for _ in $(seq 1 60); do
    ss -ltnp 2>/dev/null | grep -qE ':843[0-9] ' || break
    sleep 0.5
  done
  if ss -ltnp 2>/dev/null | grep -qE ':843[0-9] '; then
    echo "réparation : le serveur ne rend pas la main — abandon (aucune écriture)" >&2
    exit 1
  fi
  echo "réparation : serveur arrêté proprement"
fi

# ── 4 · RÉPARATION, SANS TEMPS MORT ────────────────────────────────────────
OK=0; ECHECS=0
for b in "${BASES[@]}"; do
  nom=$(basename "$b")
  printf '%-46s' "$nom"
  DEB=$(date +%s%N)
  if SORTIE=$(timeout 900 "$BIN" repair-degrees "$b" 2>&1); then
    FIN=$(date +%s%N)
    MS=$(( (FIN - DEB) / 1000000 ))
    N=$(echo "$SORTIE" | grep -oE '^repair-degrees : [0-9]+' | grep -oE '[0-9]+' || echo 0)
    printf 'OK    %6s ms · %s sidecar(s)\n' "$MS" "$N"
    OK=$((OK + 1))
  else
    D=$(detenteur "$b")
    if [ -n "$D" ]; then
      printf 'TENUE · %s\n' "${D:0:80}"
    else
      printf 'ÉCHEC · %s\n' "$(echo "$SORTIE" | grep -oE 'Caused by.*' | cut -c1-70)"
    fi
    ECHECS=$((ECHECS + 1))
  fi
done

# ── 5 · RELANCE DU DASHBOARD ───────────────────────────────────────────────
if [ "$RELANCE" = "1" ] && [ "$SERVER_WAS_UP" = "1" ]; then
  ./scripts/vizserve.sh >/dev/null 2>&1 || true
  echo "réparation : dashboard relancé"
fi

echo "réparation : $OK base(s) à niveau, $ECHECS non réparée(s) (tenues par un autre processus)"
