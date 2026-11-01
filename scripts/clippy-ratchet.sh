#!/usr/bin/env bash
# CLIQUET CLIPPY (v0.0.1). Le dépôt porte des centaines d'avertissements
# clippy hérités (mesuré le 01/11/2026 : 555 sur `--workspace --all-targets`,
# dont 80 dans obrain-substrate lib). `-D warnings` d'un coup est hors de
# portée sans un chantier dédié ; ne rien faire laisse le nombre monter.
# Ce script fait l'intermédiaire : il COMPTE, compare au plafond commité dans
# `clippy-baseline`, échoue si ça monte, et invite à abaisser le plafond
# quand ça descend. Un plafond ne se relève jamais dans une PR ordinaire.
#
# Usage : scripts/clippy-ratchet.sh [--update]
set -uo pipefail
cd "$(dirname "$0")/.."
plafond_fichier="clippy-baseline"
sortie=$(cargo clippy --workspace --all-targets --message-format=short 2>&1 >/dev/null)
# Une ligne par avertissement « fichier:ligne:col: warning: … » ; les lignes
# de résumé (« `x` (lib) generated N warnings ») ne comptent pas.
total=$(printf '%s\n' "$sortie" | grep -cE '^[^ ]+:[0-9]+:[0-9]+: warning: ' || true)
if [ "${1:-}" = "--update" ]; then
  echo "$total" > "$plafond_fichier"
  echo "plafond clippy mis à $total"
  exit 0
fi
if [ ! -f "$plafond_fichier" ]; then
  echo "clippy-ratchet : pas de plafond ($plafond_fichier absent) — lancez --update" >&2
  exit 2
fi
plafond=$(cat "$plafond_fichier")
echo "clippy : $total avertissements (plafond $plafond)"
if [ "$total" -gt "$plafond" ]; then
  echo "clippy-ratchet : $((total - plafond)) avertissement(s) de PLUS que le plafond — corrigez-les" >&2
  printf '%s\n' "$sortie" | grep -E '^[^ ]+:[0-9]+:[0-9]+: warning: ' | tail -20 >&2
  exit 1
fi
if [ "$total" -lt "$plafond" ]; then
  echo "clippy-ratchet : $((plafond - total)) de moins que le plafond — abaissez-le : scripts/clippy-ratchet.sh --update"
fi
