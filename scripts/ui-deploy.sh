#!/usr/bin/env bash
# ── DÉPLOIEMENT DE L'UI DU 8432 — UN GESTE, ET JAMAIS EN ARRIÈRE ────────────
#
# POURQUOI CE SCRIPT EXISTE (2026-09-15)
# Le 14/09 à 20:03, un `pnpm build` dans le worktree servi a réécrit
# `dist/index.html` vers un bundle bâti depuis une lignée SANS rendu markdown,
# et le redémarrage de 21:50 a rendu la substitution effective. L'utilisateur a
# perdu la vue markdown du chat sans qu'aucune porte ne s'ouvre : le build avait
# simplement écrasé la page servie.
#
# La règle, désormais : **on ne sert plus un dossier de build, on sert une
# VERSION**. Une version est créée ici, nommée, gelée en lecture seule, et
# promue en un geste atomique. Un build qui perd une capacité que la version
# servie avait est REFUSÉ (sauf `--accepter-perte`, explicite et journalisé).
#
# Usage :
#   scripts/ui-deploy.sh --depuis <dist>            # promouvoir un dist déjà bâti
#   scripts/ui-deploy.sh --worktree <worktree>      # bâtir (tsc + vite) puis promouvoir
#   scripts/ui-deploy.sh --lier <dist-du-worktree>  # une fois : découpler le dist servi
#   options : --id <etiquette> · --base <url> · --accepter-perte <capacite> · --sans-verif
set -uo pipefail
cd "$(dirname "$0")/.."
ROOT="$(pwd)"

UI_DIR="${OBRAIN_UI_DIR:-$HOME/.local/share/obrain-harness/ui}"
CONTRAT="$ROOT/scripts/ui-contrat.json"
BASE_DEFAUT="http://192.168.1.103:8432"

DEPUIS="" ; WORKTREE="" ; LIER="" ; ID="" ; BASE="$BASE_DEFAUT" ; VERIF=1
PERTES=()

KO=0
ok()   { printf 'PASS  %s\n' "$1"; }
non()  { printf 'FAIL  %s\n' "$1"; KO=$((KO+1)); }
note() { printf '      %s\n' "$1"; }
titre(){ printf '\n── %s\n' "$1"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --depuis)          DEPUIS="$2"; shift 2 ;;
    --worktree)        WORKTREE="$2"; shift 2 ;;
    --lier)            LIER="$2"; shift 2 ;;
    --id)              ID="$2"; shift 2 ;;
    --base)            BASE="$2"; shift 2 ;;
    --accepter-perte)  PERTES+=("$2"); shift 2 ;;
    --sans-verif)      VERIF=0; shift ;;
    *) echo "option inconnue : $1" >&2; exit 2 ;;
  esac
done

mkdir -p "$UI_DIR/releases"

# ── VERROU — LE MAGASIN EST UN ÉTAT PARTAGÉ ─────────────────────────────────
# CONSTATÉ LE 2026-09-15 : pendant qu'un A/B basculait la version courante à la
# main, une AUTRE session a promu sa propre version (10:43:56). Le retour
# arrière du test, écrit avec un chemin de version codé en dur, a écrasé la
# promotion de l'autre session — `current` et `etat.json` ont divergé, et c'est
# le vérificateur qui l'a vu. Toute mutation du magasin se fait donc SOUS
# VERROU, et personne ne bascule `current` à la main (voir `ui-bascule.sh`).
exec 9>"$UI_DIR/.verrou"
if ! flock -w 600 9; then
  echo "verrou occupé depuis 10 min — une autre promotion est en cours ; réessayer" >&2
  exit 1
fi
note "verrou pris sur $UI_DIR/.verrou (pid $$)"

# ── 0. DÉCOUPLAGE (une fois par worktree servi) ─────────────────────────────
# Le binaire résout le dist par un chemin FIGÉ À LA COMPILATION
# (`CARGO_MANIFEST_DIR/../../packages/obrain-dashboard/dist`, cf.
# docs/plan-j5-bascule.md §« PIÈGE dist du viz »). On ne peut pas changer ce
# chemin sans recompiler — mais on peut y mettre un LIEN vers la version
# courante. Alors un build dans ce worktree n'écrit plus jamais la page servie.
if [ -n "$LIER" ]; then
  titre "Découplage du dist servi"
  if [ ! -e "$LIER" ]; then non "chemin introuvable : $LIER"; exit 1; fi
  if [ -L "$LIER" ]; then
    cible="$(readlink -f "$LIER")"
    note "déjà un lien → $cible (rien à faire)"
  else
    # RENOMMER (pas copier) : un `cp -a` de 40 Mo suivi d'un `rm -rf` laisserait
    # /app en 404 pendant plusieurs secondes. Le renommage est instantané, la
    # fenêtre où le chemin n'existe pas se compte en microsecondes.
    sauve="$LIER.avant-decouplage-$(date +%Y%m%d-%H%M%S)"
    mv "$LIER" "$sauve" || { non "renommage impossible"; exit 1; }
    ln -s "$UI_DIR/current" "$LIER"
    ok "dist découplé : $LIER → $UI_DIR/current"
    note "contenu précédent conservé (renommé) : $sauve"
  fi
  [ -e "$UI_DIR/current/index.html" ] || note "⚠ aucune version courante encore : promouvoir avant que /app ne serve"
  exit $KO
fi

# ── 1. SOURCE ───────────────────────────────────────────────────────────────
titre "Source du build"
TMP_BUILD=""
if [ -n "$WORKTREE" ]; then
  pkg="$WORKTREE/packages/obrain-dashboard"
  [ -d "$pkg" ] || { non "pas de dashboard dans $WORKTREE"; exit 1; }
  TMP_BUILD="$(mktemp -d /tmp/ui-build-XXXXXX)"
  note "build dans $pkg → $TMP_BUILD"
  if (cd "$pkg" && npx tsc -b >/tmp/ui-tsc.log 2>&1); then ok "tsc -b (porte de types)"; else non "tsc -b — voir /tmp/ui-tsc.log"; tail -15 /tmp/ui-tsc.log | sed 's/^/      /'; exit 3; fi
  if (cd "$pkg" && npx vite build --outDir "$TMP_BUILD" --emptyOutDir >/tmp/ui-vite.log 2>&1); then ok "vite build"; else non "vite build — voir /tmp/ui-vite.log"; tail -15 /tmp/ui-vite.log | sed 's/^/      /'; exit 3; fi
  DEPUIS="$TMP_BUILD"
fi
[ -n "$DEPUIS" ] || { echo "usage: ui-deploy.sh --depuis <dist> | --worktree <worktree> | --lier <dist>" >&2; exit 2; }
[ -f "$DEPUIS/index.html" ] || { non "pas d'index.html dans $DEPUIS"; exit 1; }
note "dist source : $DEPUIS"

# ── 2. CAPACITÉS DE LA SOURCE ───────────────────────────────────────────────
titre "Capacités mesurées (fermeture atteignable depuis index.html)"
CAPS_JSON="$(node "$ROOT/scripts/ui-capacites.mjs" "$DEPUIS")" || { non "mesure impossible"; exit 1; }
lire_cap() { printf '%s' "$CAPS_JSON" | python3 -c "import json,sys;print(json.load(sys.stdin)['capacites'].get('$1'))"; }
ETAT="$UI_DIR/etat.json"
cap_avant() { [ -f "$ETAT" ] && python3 -c "import json;print(json.load(open('$ETAT'))['capacites'].get('$1'))" 2>/dev/null || echo "None"; }

# ── 3. LA PORTE ─────────────────────────────────────────────────────────────
titre "Porte : le contrat, puis l'invariant « jamais en arrière »"
BLOQ="$(python3 -c "import json;print(' '.join(json.load(open('$CONTRAT'))['bloquantes']))")"
SUIV="$(python3 -c "import json;print(' '.join(json.load(open('$CONTRAT'))['suivies']))")"
for c in $BLOQ; do
  v="$(lire_cap "$c")"
  if [ "$v" = "True" ]; then ok "bloquante « $c » présente"; else non "bloquante « $c » ABSENTE — version refusée"; fi
done
for c in $SUIV; do
  v="$(lire_cap "$c")"; av="$(cap_avant "$c")"
  if [ "$v" = "True" ]; then ok "suivie « $c » présente"
  elif [ "$av" = "True" ]; then
    if printf '%s\n' "${PERTES[@]:-}" | grep -qx "$c"; then
      note "suivie « $c » : perte ACCEPTÉE explicitement (journalisée)"
    else
      non "suivie « $c » : la version servie l'a, la nouvelle NON — refus (--accepter-perte $c pour forcer)"
    fi
  else note "suivie « $c » : absente des deux (pas une régression)"
  fi
done
if [ $KO -gt 0 ]; then
  printf '\nDÉPLOIEMENT REFUSÉ — %d contrôle(s) en échec. La version servie n'"'"'a pas bougé.\n' "$KO"
  [ -n "$TMP_BUILD" ] && rm -rf "$TMP_BUILD"
  exit 3
fi

# ── 4. LA VERSION : GELÉE, NOMMÉE ───────────────────────────────────────────
titre "Création de la version"
GIT_SHA="$(git -C "${WORKTREE:-$ROOT}" rev-parse --short HEAD 2>/dev/null || echo inconnu)"
GIT_BRANCHE="$(git -C "${WORKTREE:-$ROOT}" rev-parse --abbrev-ref HEAD 2>/dev/null || echo inconnue)"
ETIQUETTE="${ID:-$(basename "${WORKTREE:-$DEPUIS}")}"
VERSION="$(date +%Y%m%d-%H%M%S)-${GIT_SHA}-${ETIQUETTE}"
CIBLE="$UI_DIR/releases/$VERSION"
mkdir -p "$CIBLE/assets"
cp "$DEPUIS/index.html" "$CIBLE/index.html"
# On ne copie QUE la fermeture servie : le cimetière de bundles périmés ne part
# pas en production (c'est lui qui, le 15/09, a fait mentir une mesure).
printf '%s' "$CAPS_JSON" | python3 -c "import json,sys;print('\n'.join(json.load(sys.stdin)['assets_atteints']))" | while read -r a; do
  [ -n "$a" ] && [ -f "$DEPUIS/assets/$a" ] && cp "$DEPUIS/assets/$a" "$CIBLE/assets/$a"
done
node "$ROOT/scripts/ui-capacites.mjs" "$CIBLE" > "$CIBLE/capacites.json" || { non "la version copiée ne se mesure pas"; exit 1; }
chmod -R a-w "$CIBLE"
ok "version $VERSION ($(du -sh "$CIBLE" | cut -f1)) gelée en lecture seule"

# ── 5. BASCULE ATOMIQUE ─────────────────────────────────────────────────────
titre "Bascule de la version courante"
ln -sfn "$UI_DIR/releases/$VERSION" "$UI_DIR/current.new"
mv -T "$UI_DIR/current.new" "$UI_DIR/current"
ok "current → releases/$VERSION"

node -e '
const fs=require("fs");
const [etat,vers,source,sha,branche,capsFile,pertes]=process.argv.slice(1);
const caps=JSON.parse(fs.readFileSync(capsFile,"utf8"));
const json={version:vers,source,hors_ligne:sha,branche,promue:new Date().toISOString(),
  capacites:caps.capacites,perte_acceptee:pertes?pertes.split(",").filter(Boolean):[],
  index_html_sha256:caps.index_html_sha256,assets_servis:caps.servis};
fs.writeFileSync(etat,JSON.stringify(json,null,2)+"\n");
' "$ETAT" "$VERSION" "$DEPUIS" "$GIT_SHA" "$GIT_BRANCHE" "$CIBLE/capacites.json" "$(IFS=,; echo "${PERTES[*]:-}")"
printf '%s  %s  %s  %s\n' "$(date -Is)" "$VERSION" "$GIT_BRANCHE@$GIT_SHA" "capacités $(lire_cap markdown)/$(lire_cap katex)/$(lire_cap fratrie)/$(lire_cap ws-client)" >> "$UI_DIR/journal.log"
ok "manifeste écrit ($ETAT) et journal tenu ($UI_DIR/journal.log)"

# ── 6. VÉRIFICATION PAR HTTP ────────────────────────────────────────────────
if [ "$VERIF" = 1 ]; then
  titre "Vérification : ce que le serveur SERT est bien cette version"
  if bash "$ROOT/scripts/ui-verifier.sh" "$BASE"; then ok "l'UI servie est la version promue"; else non "l'UI servie DIVERGE de la version promue"; fi
fi

[ -n "$TMP_BUILD" ] && rm -rf "$TMP_BUILD"
printf '\n══════════════════════════════════════════════════════════════════\n'
if [ $KO -eq 0 ]; then printf 'UI PROMUE : %s\n' "$VERSION"; else printf 'UI PROMUE mais %d ALERTE(S) — voir ci-dessus\n' "$KO"; fi
printf '══════════════════════════════════════════════════════════════════\n'
exit $(( KO > 0 ? 1 : 0 ))
