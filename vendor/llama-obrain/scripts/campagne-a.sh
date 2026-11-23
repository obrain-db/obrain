#!/usr/bin/env bash
# campagne-a.sh — étape (a) : nom vs indice, sur un instrument FIGÉ et un serveur SURVEILLÉ.
#
# Deux leçons d'exécution, toutes deux MESURÉES ce soir, justifient ce script :
#
#  1. **Un instrument qui bouge ne mesure rien.** `scripts/juge-structurel.py` a changé d'empreinte
#     DEUX fois pendant la campagne (6c63ff58 -> 0fa8b259 -> 57de3f98), parce qu'une autre session
#     le développe en parallèle. Des scores pris avant et après ne sont pas comparables. On fige donc
#     une COPIE, avec son empreinte écrite dans le journal : ce n'est pas un fork du développement,
#     c'est un spécimen étalonné.
#
#  2. **Le serveur peut mourir en pleine campagne.** Constaté : `Received second interrupt` — une
#     interruption externe a tué le serveur une minute après le début, et le juge a attendu 600 s
#     avant d'abandonner. Ce script RELANCE le serveur et REPREND à l'unité suivante : ce qui est
#     mesuré est acquis, ce qui a échoué est refait.
#
# Une unité = (modèle, forme, famille). Unité par unité, jamais tout d'un bloc : une mort ne coûte
# qu'une unité.
#
# Usage : scripts/campagne-a.sh [--n 3] [--port 8120]

set -uo pipefail

RACINE=/home/triviere/projects/llama-obrain
BUILD=$RACINE/build
SORTIE=/mnt/obrain/juge
JOURNAL=$SORTIE/campagne-a.tsv
GELE=$SORTIE/juge-gele.py
MODELES=/mnt/obrain/models

N=3
PORT=8120
while [ $# -gt 0 ]; do
  case "$1" in
    --n) N="$2"; shift 2 ;;
    --port) PORT="$2"; shift 2 ;;
    *) echo "option inconnue : $1"; exit 2 ;;
  esac
done

mkdir -p "$SORTIE"
# ── figer l'instrument (une seule fois : si la copie existe, on la réutilise telle quelle) ──
if [ ! -s "$GELE" ]; then
  cp "$RACINE/scripts/juge-structurel.py" "$GELE"
  echo "  instrument figé : $GELE"
fi
H_GELE=$(sha256sum "$GELE" | cut -c1-16)
H_VIF=$(sha256sum "$RACINE/scripts/juge-structurel.py" | cut -c1-16)
echo "  empreinte FIGÉE  : $H_GELE"
echo "  empreinte VIVANTE: $H_VIF  $( [ "$H_GELE" = "$H_VIF" ] && echo '(identiques pour l instant)' || echo '(differentes : le juge a bouge)' )"
[ -s "$JOURNAL" ] || printf "horodatage\tmodele\tforme\tfamille\tn\tinstrument\tscore\n" > "$JOURNAL"

MODELE_CHARGE=""

demarrer_serveur() {
  local modele="$1"
  # GARDE-FOU : un serveur déjà sain peut servir un AUTRE modèle. Sans cette ligne, la campagne
  # mesurerait v2 contre le serveur de la référence — et produirait des chiffres parfaitement
  # plausibles et parfaitement faux. (Défaut attrapé avant la première mesure, pas après.)
  if [ "$MODELE_CHARGE" != "$modele" ]; then
    arreter_serveur
  fi
  if curl -s -m 2 "http://127.0.0.1:$PORT/health" 2>/dev/null | grep -q '"status":"ok"'; then
    MODELE_CHARGE="$modele"
    return 0
  fi
  nohup "$BUILD/bin/llama-server" -m "$modele" --port "$PORT" -c 4096 -t 8 -ngl 0 \
        --no-warmup --jinja > "$SORTIE/serveur-campagne-$PORT.log" 2>&1 &
  SRV=$!
  for i in $(seq 1 120); do
    if curl -s -m 2 "http://127.0.0.1:$PORT/health" 2>/dev/null | grep -q '"status":"ok"'; then
      MODELE_CHARGE="$modele"
      return 0
    fi
    kill -0 "$SRV" 2>/dev/null || return 1
    sleep 2
  done
  return 1
}

arreter_serveur() {
  if [ -n "${SRV:-}" ]; then kill "$SRV" 2>/dev/null; wait "$SRV" 2>/dev/null; fi
  SRV=""
  MODELE_CHARGE=""
  # le port doit être RÉELLEMENT libre avant de rendre la main, sinon le démarrage suivant se
  # croira servi par un serveur mourant et servirait l'ancien modèle
  for _ in $(seq 1 15); do
    curl -s -m 1 "http://127.0.0.1:$PORT/health" >/dev/null 2>&1 || return 0
    sleep 1
  done
}

for MODELE in "reference:/home/triviere/models/Qwen3.8-27B-Q4_K_M.gguf" \
              "v2:$MODELES/Qwen3.8-27B-OBRAIN-v2.gguf"; do
  NOM="${MODELE%%:*}"; CHEMIN="${MODELE#*:}"
  for FORME in nom indice; do
    for FAM in degre redondance; do
      echo "  ── $NOM / $FORME / $FAM — $(date -Is)"
      OK=0
      for essai in 1 2 3; do
        if demarrer_serveur "$CHEMIN"; then
          T0=$(date +%s)
          timeout 2400 stdbuf -oL python3 "$GELE" "$PORT" --n "$N" --forme "$FORME" \
              --familles "$FAM" > "$SORTIE/a-$NOM-$FORME-$FAM.txt" 2>&1
          T1=$(date +%s)
          LIGNE=$(grep -E "^  $FAM " "$SORTIE/a-$NOM-$FORME-$FAM.txt" | tail -1)
          if [ -n "$LIGNE" ] && ! grep -q "0/  0" <<< "$LIGNE"; then
            echo "     $LIGNE  ($((T1-T0)) s)"
            printf "%s\t%s\t%s\t%s\t%s\t%s\t%s\n" "$(date -Is)" "$NOM" "$FORME" "$FAM" "$N" \
                   "$H_GELE" "$(echo "$LIGNE" | sed 's/^ *//')" >> "$JOURNAL"
            OK=1; break
          fi
          echo "     essai $essai sans résultat (serveur mort ?) — je relance"
          arreter_serveur
        else
          echo "     essai $essai : serveur non démarrable"
        fi
      done
      [ "$OK" = "0" ] && printf "%s\t%s\t%s\t%s\t%s\t%s\t%s\n" "$(date -Is)" "$NOM" "$FORME" "$FAM" \
            "$N" "$H_GELE" "ECHEC (3 essais)" >> "$JOURNAL"
    done
  done
done

arreter_serveur
echo "=== campagne (a) terminée — $(date -Is) ==="
column -t -s$'\t' "$JOURNAL"
