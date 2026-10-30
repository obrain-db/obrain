#!/usr/bin/env bash
# ── VÉRIFICATION : CE QUI EST SERVI EST BIEN LA VERSION PROMUE ──────────────
#
# Comparaison par HTTP des octets RÉELLEMENT servis à ceux de la version
# courante. C'est la garde qui manquait le 14/09 : elle aurait vu, à 21:50, que
# la page servie n'était plus celle qu'on croyait.
#
# Elle détecte trois dérives, chacune avec son nom :
#   1. la page servie n'est pas celle de `current` (index.html substitué) ;
#   2. un asset servi ne correspond pas à celui de `current` (bundle remplacé) ;
#   3. le dist du worktree servi n'est plus un LIEN vers la version courante
#      (quelqu'un a rebâti par-dessus — exactement l'accident du 14/09).
#
# Usage : scripts/ui-verifier.sh [base]      (défaut : http://192.168.1.103:8432)
# Sortie : PASS/FAIL par contrôle, code non nul dès la première dérive.
set -uo pipefail
cd "$(dirname "$0")/.."

BASE="${1:-http://192.168.1.103:8432}"
ATTENDRE=0
[ "${2:-}" = "--attendre" ] && ATTENDRE=30
UI_DIR="${OBRAIN_UI_DIR:-$HOME/.local/share/obrain-harness/ui}"
ETAT="$UI_DIR/etat.json"

# Au DÉMARRAGE du service (ExecStartPost), le socket peut ne pas écouter encore :
# sans cette attente, la porte crierait sur une course, pas sur une dérive.
if [ "$ATTENDRE" != 0 ]; then
  for _ in $(seq 1 "$ATTENDRE"); do
    curl -s -o /dev/null -m 2 "$BASE/api/stores" && break
    sleep 1
  done
fi
# Worktrees dont le dist est censé être un lien vers la version courante :
# s'ajoute ici ce qui sert un jour (le chemin est figé dans chaque binaire).
LIENS_ATTENDUS=(
  "/home/triviere/projects/banc-obrain-j5/packages/obrain-dashboard/dist"
)

KO=0
ok()  { printf 'PASS  %s\n' "$1"; }
non() { printf 'FAIL  %s\n' "$1"; KO=$((KO+1)); }

if [ ! -f "$ETAT" ]; then
  non "aucune version promue ($ETAT absent) — promouvoir avec scripts/ui-deploy.sh"
  exit 1
fi

sha_flux() { curl -s --compressed -m 20 "$1" | sha256sum | cut -d' ' -f1; }

# ── 1. La page servie == la version courante ────────────────────────────────
locales="$(sha256sum "$UI_DIR/current/index.html" | cut -d' ' -f1)"
# /app : la page shell — IAM arme, « / » redirige vers la connexion.
servie="$(sha_flux "$BASE/app")"
if [ "$servie" = "$locales" ]; then ok "index.html servi = version courante"
else non "index.html SERVI ≠ version courante (servi ${servie:0:16}, courant ${locales:0:16})"; fi

# ── 2. Chaque asset servi == celui de la version courante ───────────────────
python3 -c "
import json,sys
d=json.load(open('$ETAT'))
print('\n'.join(a['nom']+' '+a['sha256'] for a in d['assets_servis']))
" | while read -r nom attendu; do
  if [ ! -f "$UI_DIR/current/assets/$nom" ]; then
    echo "FAIL  $nom : absent de la version courante"; echo KO >> /tmp/ui-verifier-ko
    continue
  fi
  obtenu="$(sha_flux "$BASE/app/assets/$nom")"
  if [ "$obtenu" = "$attendu" ]; then printf 'PASS  %s servi à l'"'"'identique\n' "$nom"
  else printf 'FAIL  %s DIVERGE (servi %s, attendu %s)\n' "$nom" "${obtenu:0:16}" "${attendu:0:16}"; echo KO >> /tmp/ui-verifier-ko; fi
done
[ -f /tmp/ui-verifier-ko ] && { KO=$((KO + $(wc -l < /tmp/ui-verifier-ko))); rm -f /tmp/ui-verifier-ko; }

# ── 3. Le dist servi est toujours découplé ──────────────────────────────────
for d in "${LIENS_ATTENDUS[@]}"; do
  [ -e "$d" ] || continue
  if [ -L "$d" ]; then
    cible="$(readlink -f "$d")"
    if [ "$cible" = "$(readlink -f "$UI_DIR/current")" ]; then ok "dist découplé : $d → version courante"
    else non "dist $d pointe ailleurs ($cible)"; fi
  else
    non "dist $d n'est plus un lien — un build a pu écraser la page servie"
  fi
done

printf '\n'
if [ "$KO" -eq 0 ]; then printf 'UI SERVIE CONFORME (%s)\n' "$BASE"; else printf 'UI SERVIE DIVERGENTE — %d contrôle(s) en échec\n' "$KO"; fi
exit $(( KO > 0 ? 1 : 0 ))
