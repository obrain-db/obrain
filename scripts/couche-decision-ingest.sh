#!/usr/bin/env bash
#
# couche-decision-ingest.sh — projette le plan de la couche de décision dans un graphe.
#
# ── POURQUOI CE PILOTE EXISTE ───────────────────────────────────────────────
#
# MESURÉ : le parseur GQL d'obrain exige qu'UNE SEULE instruction soit
# consommée par invocation — « the entire query must be consumed ». Un fichier
# de plusieurs instructions séparées par `;` passé à `obrain query --file`
# échoue à la deuxième. Les instructions de `couche-decision.gql` sont donc
# jouées une par une, une invocation par instruction.
#
# ── CE QUI REND LE REJEU SANS DANGER ────────────────────────────────────────
#
# MESURÉ, et c'est la propriété qui compte :
#   · nœuds  : `MERGE ... ON CREATE SET` → un `status` déjà changé SURVIT au
#              rejeu. Rejouer le script ne détruit pas l'avancement.
#   · arêtes : `MERGE` sur motif d'arête → rejouable sans doublon.
#              `INSERT` d'arête N'EST PAS idempotent (rejeu = arête en double).
#              Ne pas remplacer les MERGE d'arêtes par des INSERT.
#
# ── USAGE ───────────────────────────────────────────────────────────────────
#
#   scripts/couche-decision-ingest.sh --store /chemin/ma.obrain
#   scripts/couche-decision-ingest.sh --store /chemin/ma.obrain --dry-run
#   scripts/couche-decision-ingest.sh --store /chemin/ma.obrain --obrain ./target/release/obrain
#
# Le store est OBLIGATOIRE et explicite : aucun défaut, pour qu'aucune
# ingestion ne puisse viser une base vivante par inadvertance.
#
# ── PRÉREQUIS ───────────────────────────────────────────────────────────────
#
# Le store visé ne doit pas être tenu en écriture par un autre processus.
# `viz serve` en est l'écrivain unique sur runtime-dev : le CLI ne peut alors
# pas l'ouvrir en écriture (« already open by another process »). Ingérer dans
# une base de chantier, ou par l'écrivain autorisé.

set -euo pipefail

STORE=""
OBRAIN=""
DRY_RUN=0
GQL="$(dirname "$0")/couche-decision.gql"

while [ $# -gt 0 ]; do
  case "$1" in
    --store)   STORE="${2:-}"; shift 2 ;;
    --obrain)  OBRAIN="${2:-}"; shift 2 ;;
    --gql)     GQL="${2:-}"; shift 2 ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) sed -n '2,45p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "option inconnue : $1" >&2; exit 2 ;;
  esac
done

if [ -z "$STORE" ]; then
  echo "ERREUR : --store est obligatoire (aucun défaut, par conception)." >&2
  exit 2
fi

if [ ! -e "$STORE" ]; then
  echo "ERREUR : le store '$STORE' n'existe pas." >&2
  exit 2
fi

if [ -z "$OBRAIN" ]; then
  if command -v obrain >/dev/null 2>&1; then
    OBRAIN="$(command -v obrain)"
  else
    echo "ERREUR : binaire obrain introuvable ; utiliser --obrain <chemin>." >&2
    exit 2
  fi
fi

if [ ! -f "$GQL" ]; then
  echo "ERREUR : fichier d'instructions introuvable : $GQL" >&2
  exit 2
fi

echo "store    : $STORE"
echo "binaire  : $OBRAIN"
echo "gql      : $GQL"
[ "$DRY_RUN" = 1 ] && echo "mode     : SIMULATION (aucune écriture)"

n=0
echecs=0

# Une instruction par ligne ; `#` et lignes vides ignorés.
while IFS= read -r instruction || [ -n "$instruction" ]; do
  case "$instruction" in
    ''|'#'*) continue ;;
  esac

  n=$((n + 1))

  if [ "$DRY_RUN" = 1 ]; then
    printf '%4d  %s\n' "$n" "$(printf '%s' "$instruction" | cut -c1-100)"
    continue
  fi

  if ! sortie="$("$OBRAIN" query "$STORE" "$instruction" 2>&1)"; then
    echecs=$((echecs + 1))
    echo "ÉCHEC instruction $n :" >&2
    printf '%s\n' "$instruction" >&2
    printf '%s\n' "$sortie" >&2
  fi
done < "$GQL"

if [ "$DRY_RUN" = 1 ]; then
  echo "simulation : $n instructions seraient jouées."
  exit 0
fi

if [ "$echecs" -gt 0 ]; then
  echo "TERMINÉ AVEC ÉCHECS : $echecs sur $n instructions." >&2
  exit 1
fi

echo "OK : $n instructions jouées, aucune erreur."

# ── Contrôle de forme ───────────────────────────────────────────────────────
# Le plan doit être lisible et son état modifiable immédiatement.
echo
echo "Avancement (toutes les unités sont 'todo' à la première ingestion) :"
"$OBRAIN" query "$STORE" \
  "MATCH (p:Phase)-[:HAS_STEP]->(s:Step) RETURN p.id, p.seq, s.id, s.status ORDER BY p.seq, s.seq"
