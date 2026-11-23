#!/usr/bin/env bash
# qa-campagne.sh — faire porter (c) sur PLUSIEURS questions, et consigner de quoi VÉRIFIER.
#
# Pourquoi ce script existe : A5 a débloqué `qa_obrain.py` et l'a prouvé sur **une** question — et il
# l'écrit lui-même : « une question n'est pas une validation de qualité, c'est une preuve de
# fonctionnement ». Un seul succès ne distingue pas « le ternaire sait lire la source » de « il a eu
# de la chance sur ixgbe_probe ».
#
# Ce que ce script ajoute, et rien de plus :
#   1. plusieurs questions, chacune avec sa propre fonction cible (donc sa propre résolution de source) ;
#   2. la SOURCE RÉSOLUE conservée À CÔTÉ de la réponse, pour que la réponse puisse être confrontée
#      au fait — sinon « ça répond » reste invérifiable ;
#   3. un journal TSV append-only, avec les métriques de temps (extract, TFPT, tok/s).
#
# Il n'écrit rien dans le graphe et n'arme ni ne désarme aucune garde : c'est le script de A5, appelé
# en boucle, avec sa sortie conservée.
#
# Usage :
#   export OBRAIN_IAM_TOKEN=$(... ~/.config/obrain-harness/iam-token.env)
#   scripts/qa-campagne.sh <port> [--n-gen 90]

set -uo pipefail

OPS=/home/triviere/projects/llama-obrain/ops-gpu16
SORTIE=/mnt/obrain/qa
JOURNAL=$SORTIE/journal.tsv
mkdir -p "$SORTIE"

PORT="${1:?usage: qa-campagne.sh <port>}"
shift || true

if [ -z "${OBRAIN_IAM_TOKEN:-}" ]; then
  echo "  ✗ OBRAIN_IAM_TOKEN absente. Charger ~/.config/obrain-harness/iam-token.env :"
  echo "    set -a; . ~/.config/obrain-harness/iam-token.env; set +a"
  exit 1
fi

# les cibles sont des fonctions RÉELLES du corpus, choisies pour que chaque question exige une
# résolution de source DIFFÉRENTE (fichier et plage de lignes distincts).
QUESTIONS=(
  "Que fait la fonction ixgbe_probe ?"
  "Que fait la fonction e1000_probe ?"
  "Que fait la fonction tg3_open ?"
  "Que fait la fonction rtl8169_init_one ?"
  "Que fait la fonction sja1000_open ?"
)

if ! [ -s "$JOURNAL" ]; then
  printf "horodatage\tquestion\tstatut\tcar_fait\textract_s\twall_s\ttok_s\tfichier\n" > "$JOURNAL"
fi

for q in "${QUESTIONS[@]}"; do
  echo "── $q   ($(date -Is))"
  FICHIER="$SORTIE/$(echo "$q" | tr -c 'A-Za-z0-9' '_').txt"
  T0=$(date +%s)
  ( cd "$OPS" && timeout 1800 python3 qa_obrain_source.py --port "$PORT" "$q" ) > "$FICHIER" 2>&1
  rc=$?
  T1=$(date +%s)

  if [ $rc -ne 0 ]; then
    echo "   ✗ échec (rc=$rc) — voir $FICHIER"
    printf "%s\t%s\tECHEC_rc%s\t-\t-\t%s\t-\t%s\n" "$(date -Is)" "$q" "$rc" "$((T1-T0))" "$FICHIER" >> "$JOURNAL"
    continue
  fi

  # attention : « 1 fait(s), 1400 caractères » — on veut le SECOND nombre. Prendre le PREMIER
  # inscrivait le nombre de faits (toujours 1) dans une colonne nommée « caractères ».
  CAR=$(grep -oE "[0-9]+ fait\(s\), [0-9]+ caractères" "$FICHIER" | grep -oE "[0-9]+ caractères" | grep -oE "^[0-9]+" | head -1)
  EX=$(grep -oE "extract [0-9.]+s" "$FICHIER" | grep -oE "[0-9.]+" | head -1)
  WALL=$(grep -oE "wall [0-9.]+s" "$FICHIER" | grep -oE "[0-9.]+" | head -1)
  TOKS=$(grep -oE "tok/s≈[0-9.]+" "$FICHIER" | grep -oE "[0-9.]+" | head -1)
  printf "   faits %s car. | extract %ss | wall %ss | %s tok/s\n" "${CAR:-?}" "${EX:-?}" "${WALL:-?}" "${TOKS:-?}"
  # l'aperçu de la réponse, pour lecture immédiate
  sed -n '/^RÉPONSE :/,/^\[métriques/p' "$FICHIER" | sed '1d;$d' | head -4 | sed 's/^/   │ /'
  printf "%s\t%s\tOK\t%s\t%s\t%s\t%s\t%s\n" \
    "$(date -Is)" "$q" "${CAR:-?}" "${EX:-?}" "${WALL:-?}" "${TOKS:-?}" "$FICHIER" >> "$JOURNAL"
done

echo
echo "=== campagne terminée — journal : $JOURNAL ==="
echo "    Les sources résolues sont dans chaque fichier : la réponse peut être CONFRONTÉE au fait."
