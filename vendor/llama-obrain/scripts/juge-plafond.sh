#!/usr/bin/env bash
# juge-plafond.sh — ÉTALONNER le juge : quel score la RÉFÉRENCE obtient-elle ?
#
# Pourquoi c'est la mesure qui manque : un score de candidat n'est lisible que rapporté à un
# plafond. Si la référence Q4_K_M (PPL 2,3983 — le meilleur modèle dont on dispose) rate une
# famille, ce n'est pas le candidat qui est en cause, c'est la question. Le principe est déjà posé
# dans les documents : « un juge que la référence rate n'est pas un juge ». Il n'avait jamais été
# VÉRIFIÉ : ni la doc technique ni la note de reprise ne portent de score de référence.
#
# Deux précautions d'ingénierie, tirées d'un incident MESURÉ (2026-09-20 23:10, un `pkill` d'une
# autre session a tué le serveur en pleine 4e question et tout perdu) :
#
#   1. **Une famille par invocation.** Le juge n'imprime son verdict qu'à la toute fin ; tué, il ne
#      rend rien. En découpant par famille, une mort ne coûte que LA famille en cours.
#   2. **Aucun `pkill`.** On note le PID et on ne tue que le sien. Le motif `pkill -f llama-server`
#      est un piège documenté du dépôt (il matche la ligne de commande de son propre shell) et il
#      détruit le travail des autres.
#
# Usage : scripts/juge-plafond.sh <reference|v2|v1|attn_output> [--n 3] [--forme nom] [--port 8120]
# Sortie : un fichier de résultat par (modèle, famille, forme) dans /mnt/obrain/juge/

set -uo pipefail

RACINE=/home/triviere/projects/llama-obrain
BUILD=$RACINE/build
JUGE=$RACINE/scripts/juge-structurel.py
SORTIE=/mnt/obrain/juge
JOURNAL=$SORTIE/journal.tsv

mkdir -p "$SORTIE"

CIBLE="${1:?usage: juge-plafond.sh <reference|v2|v1|attn_output> [--n 3] [--forme nom] [--port 8120]}"
shift || true

case "$CIBLE" in
  reference)   MODELE=/home/triviere/models/Qwen3.8-27B-Q4_K_M.gguf ;;
  v2)          MODELE=/mnt/obrain/models/Qwen3.8-27B-OBRAIN-v2.gguf ;;
  v1)          MODELE=/mnt/obrain/models/Qwen3.8-27B-OBRAIN-v1.gguf ;;
  attn_output) MODELE=/mnt/obrain/models/Qwen3.8-27B-OBRAIN-attn_output.gguf ;;
  *) echo "cible inconnue : $CIBLE"; exit 2 ;;
esac

N=3
FORME=nom
PORT=8120
THREADS=6            # on laisse des fils à l'autre session : la machine est partagée
while [ $# -gt 0 ]; do
  case "$1" in
    --n) N="$2"; shift 2 ;;
    --forme) FORME="$2"; shift 2 ;;
    --port) PORT="$2"; shift 2 ;;
    --threads) THREADS="$2"; shift 2 ;;
    *) echo "option inconnue : $1"; exit 2 ;;
  esac
done

if ! ss -ltn 2>/dev/null | grep -q ":$PORT "; then
  echo "  démarrage du serveur ($CIBLE) sur :$PORT, -t $THREADS — $(date -Is)"
  nohup "$BUILD/bin/llama-server" -m "$MODELE" --port "$PORT" -c 4096 -t "$THREADS" -ngl 0 \
        --no-warmup --jinja > "$SORTIE/serveur-$CIBLE-$PORT.log" 2>&1 &
  SRV=$!
  echo "  PID serveur = $SRV (je ne tuerai QUE celui-là)"
  for i in $(seq 1 180); do
    if curl -s -m 2 "http://127.0.0.1:$PORT/health" 2>/dev/null | grep -q '"status":"ok"'; then
      echo "  serveur prêt après $((i*2)) s"; break
    fi
    # si le serveur est mort entre-temps (tué par ailleurs), on le dit au lieu d'attendre 6 min
    if ! kill -0 "$SRV" 2>/dev/null; then
      echo "  ✗ le serveur est MORT pendant le chargement (tué par un tiers ?) — voir le journal"
      tail -3 "$SORTIE/serveur-$CIBLE-$PORT.log"
      exit 3
    fi
    sleep 2
  done
else
  SRV=""
  echo "  un serveur écoute déjà sur :$PORT — je l'utilise tel quel"
fi

H_JUGE=$(sha256sum "$JUGE" | cut -c1-16)
echo "  empreinte du juge : $H_JUGE (si elle change en cours de route, les scores ne sont plus comparables)"

for FAM in degre atteignabilite cycles redondance; do
  FICHIER="$SORTIE/$CIBLE-$FORME-$FAM.txt"
  echo "  ── $CIBLE / $FORME / $FAM (n=$N) — $(date -Is)"
  T0=$(date +%s)
  # stdbuf force l'affichage au fil de l'eau : on voit la progression au lieu de deviner
  timeout 3600 stdbuf -oL -eL python3 "$JUGE" "$PORT" --n "$N" --forme "$FORME" \
      --familles "$FAM" > "$FICHIER" 2>&1
  rc=$?
  T1=$(date +%s)
  LIGNE=$(grep -E "^  $FAM " "$FICHIER" | tail -1)
  [ -z "$LIGNE" ] && LIGNE="  $FAM  —/— (échec rc=$rc)"
  echo "     $LIGNE   ($(( T1 - T0 )) s, rc=$rc)"
  printf "%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n" \
    "$(date -Is)" "$CIBLE" "$FORME" "$FAM" "$N" "$H_JUGE" "$(( T1 - T0 ))" "$(echo "$LIGNE" | sed 's/^ *//')" \
    >> "$JOURNAL"
done

if [ -n "${SRV:-}" ]; then
  echo "  arrêt de MON serveur (PID $SRV) — $(date -Is)"
  kill "$SRV" 2>/dev/null
  wait "$SRV" 2>/dev/null
fi
echo "=== étalonnage $CIBLE terminé — $(date -Is) ==="
echo "    journal : $JOURNAL"
