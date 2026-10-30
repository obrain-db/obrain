#!/usr/bin/env bash
# ── BASCULE DÉLIBÉRÉE VERS UNE VERSION DÉJÀ PROMUE ──────────────────────────
#
# POURQUOI CE SCRIPT EXISTE (2026-09-15)
# Pendant un A/B, `current` a été basculé à la main avec un chemin de version
# codé en dur. Une AUTRE session a promu sa version entre-temps (10:43:56) : le
# retour arrière du test a écrasé sa promotion, et `current` a divergé de
# `etat.json`. Ce script est la seule façon correcte de changer la version
# servie : il prend le VERROU, remet `etat.json` d'accord avec la version
# visée, bascule le lien, journalise, puis vérifie.
#
# Usage :
#   scripts/ui-bascule.sh <version-ou-prefixe>        # version la plus récente si omis
#   scripts/ui-bascule.sh --liste
set -uo pipefail
cd "$(dirname "$0")/.."

UI_DIR="${OBRAIN_UI_DIR:-$HOME/.local/share/obrain-harness/ui}"
BASE="http://192.168.1.103:8432"
CIBLE="${1:-}"
[ "${2:-}" = "--base" ] && BASE="${3:-$BASE}"

if [ "$CIBLE" = "--liste" ] || [ -z "$CIBLE" ]; then
  echo "Versions promues (de la plus ancienne à la plus récente) :"
  ls -1 "$UI_DIR/releases" 2>/dev/null | sed 's/^/  /'
  echo
  echo "Version courante : $(basename "$(readlink "$UI_DIR/current" 2>/dev/null || echo aucune)")"
  [ "$CIBLE" = "--liste" ] && exit 0
  exit 2
fi

# Résolution : nom exact, ou préfixe unique.
TROUVES=$(ls -1 "$UI_DIR/releases" | grep -E "^${CIBLE}" || true)
NB=$(printf '%s\n' "$TROUVES" | grep -c . || true)
if [ "$NB" -eq 0 ]; then echo "aucune version ne commence par « $CIBLE »" >&2; exit 1; fi
if [ "$NB" -gt 1 ]; then echo "« $CIBLE » désigne $NB versions — préciser :" >&2; printf '%s\n' "$TROUVES" | sed 's/^/  /' >&2; exit 1; fi
VERSION="$TROUVES"
RELEASE="$UI_DIR/releases/$VERSION"
[ -f "$RELEASE/index.html" ] || { echo "$RELEASE n'a pas d'index.html" >&2; exit 1; }

exec 9>"$UI_DIR/.verrou"
if ! flock -w 600 9; then echo "verrou occupé — réessayer" >&2; exit 1; fi

# `etat.json` DÉCRIT LA VERSION SERVIE : on le réécrit depuis la version visée,
# sinon le vérificateur signalerait (à juste titre) une divergence.
node -e '
const fs=require("fs");
const [etat,release,version,base]=process.argv.slice(1);
const caps=JSON.parse(fs.readFileSync(`${release}/capacites.json`,"utf8"));
let prec=null; try{ prec=JSON.parse(fs.readFileSync(etat,"utf8")).version; }catch(e){}
fs.writeFileSync(etat, JSON.stringify({
  version, source: caps.dir, bascule_deliberee: true, bascule_le: new Date().toISOString(),
  version_precedente: prec, capacites: caps.capacites,
  index_html_sha256: caps.index_html_sha256, assets_servis: caps.servis,
}, null, 2)+"\n");
' "$UI_DIR/etat.json" "$RELEASE" "$VERSION" "$BASE"

ln -sfn "$RELEASE" "$UI_DIR/current.new" && mv -T "$UI_DIR/current.new" "$UI_DIR/current"
printf '%s  BASCULE  %s  (précédente : %s)\n' "$(date -Is)" "$VERSION" \
  "$(node -e 'try{console.log(JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).version_precedente||"aucune")}catch(e){console.log("aucune")}' "$UI_DIR/etat.json")" >> "$UI_DIR/journal.log"
echo "current → $VERSION"

bash "$(dirname "$0")/ui-verifier.sh" "$BASE"
