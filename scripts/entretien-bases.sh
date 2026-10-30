#!/usr/bin/env bash
# ENTRETIEN DES BASES — exécuté PENDANT LA FENÊTRE DU REDÉMARRAGE.
#
# Pourquoi ici et pas dans une commande séparée : les réparations exigent une
# ouverture EXCLUSIVE, donc que plus personne ne tienne la base. Le seul moment
# où c'est vrai sans geste humain, c'est **entre l'arrêt et le lancement du
# service** — `scripts/vizserve.sh` arrête, appelle ce script, puis relance.
#
# Rien ici n'est obligatoire pour servir : tout échec est rapporté et on continue.
# Un entretien qui empêcherait le service de démarrer serait pire que pas
# d'entretien.
#
# Deux passes, de la plus sûre à la plus engageante :
#
#   1. **colonnes de degrés** — reconstruction EN PLACE (jamais par suppression),
#      idempotente, mesurée 23 à 200 ms par base. Toujours exécutée.
#   2. **enregistrements corrompus** (`obrain repair`) — DIAGNOSTIC par défaut ;
#      la réparation n'est appliquée que si `OBRAIN_REPAIR_APPLY=1`, parce
#      qu'elle ÉCRIT dans les bases.
#
# Usage : scripts/entretien-bases.sh [--appliquer]
set -u
cd "$(dirname "$0")/.."

DATA=/home/triviere/.local/share/obrain-harness/data
BIN=./target/release/obrain
APPLIQUER=0
[ "${1:-}" = "--appliquer" ] && APPLIQUER=1
[ "${OBRAIN_REPAIR_APPLY:-0}" = "1" ] && APPLIQUER=1

[ -x "$BIN" ] || { echo "entretien : $BIN absent — ignoré" >&2; exit 0; }

BASES=()
for p in \
  "$DATA/runtime-dev.obrain" "$DATA/runtime-prod.obrain" "$DATA/self-demo.obrain" \
  "$DATA/agent-synapse-triviere-dsh.obrain" \
  /tmp/linux-full3.obrain /tmp/kv-linux-e1000.obrain /tmp/engine_idx.obrain \
  /tmp/kv-linux-driversnet.obrain
do
  [ -d "$p" ] && BASES+=("$p")
done
for p in /tmp/obrain-worlds/*.obrain; do [ -d "$p" ] && BASES+=("$p"); done

detenteur() {
  local cible; cible=$(basename "$1")
  for p in $(ps -eo pid | tail -n +2); do
    if ls -l "/proc/$p/fd" 2>/dev/null | grep -q "/$cible/"; then
      echo "$p:$(tr '\0' ' ' < "/proc/$p/cmdline" 2>/dev/null | cut -c1-50)"
      return
    fi
  done
}

# ── UNE SEULE FENÊTRE, BASE PAR BASE ───────────────────────────────────────
# Défaut mesuré à la première version : les deux passes étaient SÉPARÉES — les
# colonnes de degrés sur toutes les bases, puis le diagnostic des
# enregistrements sur toutes les bases. Entre les deux, un autre serveur (l'autre
# agent relance le sien fréquemment) reprenait les bases, et le diagnostic
# répondait « TENUE » pour 11 bases sur 12 : la fenêtre libre avait été gaspillée.
#
# On traite donc CHAQUE base d'un seul tenant : vérifier qu'elle est libre,
# reconstruire ses colonnes, diagnostiquer ses enregistrements — pendant qu'elle
# l'est encore.
# ── LES DEMANDES ATTACHÉES EN VOL ──────────────────────────────────────────
# Un défaut constaté pendant que la base tournait a été ATTACHÉ (journal
# append-only, comme la mémoire du chat). On les relit ici, on les rejoue, et on
# les ACCORDE quand la réparation a réussi. L'âge de la plus ancienne est dit :
# une réparation due depuis trois jours doit se voir.
FILE=$("$BIN" entretien 2>/dev/null || echo '{}')
EN_ATTENTE=$(echo "$FILE" | grep -oE '"en_attente": *[0-9]+' | grep -oE '[0-9]+' || echo 0)
AGE=$(echo "$FILE" | grep -oE '"age_max_secondes": *[0-9]+' | grep -oE '[0-9]+' || echo 0)
if [ "${EN_ATTENTE:-0}" != "0" ]; then
  echo "entretien · $EN_ATTENTE demande(s) attachée(s) en vol, la plus ancienne il y a ${AGE} s"
  echo "$FILE" | grep -oE '"store": *"[^"]+"' | sed 's/.*: *//' | tr -d '"' | while read -r st; do
    echo "entretien ·   dû : $(basename "$st")"
  done
fi

FAITES=0; TENUES=0; A_REPARER=""
for b in "${BASES[@]}"; do
  nom=$(basename "$b")
  D=$(detenteur "$b")
  if [ -n "$D" ]; then
    printf 'entretien · %-40s TENUE · %s\n' "$nom" "${D:0:58}"
    TENUES=$((TENUES + 1))
    continue
  fi
  # 1 · colonnes de degrés (en place, idempotent, sûr)
  if [ "${OBRAIN_SANS_DEGRES:-0}" != "1" ]; then
    if SORTIE=$(timeout 900 "$BIN" repair-degrees "$b" 2>&1); then
      # La réparation a réussi : la demande est ACQUITTÉE.
      "$BIN" entretien --accorder "$b" --genre degres >/dev/null 2>&1 || true
      printf 'entretien · %-40s degrés : %s\n' "$nom" "$(echo "$SORTIE" | grep -oE 'EN PLACE.*' | cut -c1-40)"
    else
      printf 'entretien · %-40s degrés : ÉCHEC\n' "$nom"
    fi
  fi
  # 2 · enregistrements corrompus — diagnostic (et application si autorisée)
  DIAG=$(timeout 600 "$BIN" repair "$b" 2>&1) || true
  if echo "$DIAG" | grep -qiE "already open|exclusive"; then
    printf 'entretien · %-40s enregistrements : TENUE\n' "$nom"
  elif echo "$DIAG" | grep -qiE "aucun|0 (probleme|anomalie|incoher)|saine|rien a reparer"; then
    printf 'entretien · %-40s enregistrements : saine\n' "$nom"
  else
    printf 'entretien · %-40s enregistrements : À RÉPARER\n' "$nom"
    A_REPARER="$A_REPARER $b"
  fi
  FAITES=$((FAITES + 1))
done

echo "entretien · $FAITES base(s) traitée(s), $TENUES tenue(s) par un autre processus"

if [ -n "$A_REPARER" ] && [ "$APPLIQUER" = "1" ]; then
  for b in $A_REPARER; do
    printf 'entretien · réparation de %s … ' "$(basename "$b")"
    if timeout 1800 "$BIN" repair "$b" --apply >/tmp/entretien-repair.log 2>&1; then
      "$BIN" entretien --accorder "$b" --genre enregistrements >/dev/null 2>&1 || true
      echo "faite"
    else
      echo "ÉCHEC — $(tail -1 /tmp/entretien-repair.log | cut -c1-80)"
    fi
  done
elif [ -n "$A_REPARER" ]; then
  echo "entretien · réparation NON APPLIQUÉE (diagnostic seul)."
  echo "           Pour l'appliquer : OBRAIN_REPAIR_APPLY=1 bash scripts/vizserve.sh"
fi
