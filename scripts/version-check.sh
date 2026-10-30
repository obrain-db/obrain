#!/usr/bin/env bash
# ── LA PORTE DE VERSION : CE QUI TOURNE EST-IL CE QUI EST VALIDÉ ? ──────────
#
# Demande : « uniformiser et gérer une bonne version qui ne décale pas de ce qui
# est validé et fonctionnel ».
#
# Le problème n'était pas l'absence de version — il y en avait une (une branche,
# un binaire, une UI promue) — mais l'absence de MOYEN DE LA COMPARER. Mesuré le
# 2026-09-16 : l'UI promue annonçait `hors_ligne 0e3367a4` alors que les sources
# étaient en `990a8911`. Cause : `ui-deploy.sh` CONSTRUIT depuis l'arbre de
# travail mais ENREGISTRE `git HEAD` — des fichiers non commités produisent une
# version servie plus récente que la référence, et rien ne le disait.
#
# Cette porte compare QUATRE choses, et nomme chaque écart :
#
#   1. l'arbre de service est sur la branche attendue, et À ce commit ;
#   2. l'UI servie nomme ce commit (`hors_ligne`) ;
#   3. le service tourne, et sur le binaire attendu ;
#   4. l'UI servie est bien celle de la version promue (l'`index.html` mis à part :
#      avec l'IAM armé, `GET /` rend la page de connexion, pas la page de l'app).
#
# Elle ne MODIFIE rien : elle constate. La réparation est un geste explicite
# (`--reparer`), parce qu'une porte qui répare en silence ne dit plus la vérité.
#
# Usage : scripts/version-check.sh [--complet] [--reparer]

set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
ARBRE="$(pwd)"
REF="$ARBRE/VERSION.json"

COMPLET=0
REPARER=0
ECRITURE_EN_COURS=0
for a in "$@"; do
  case "$a" in
    --complet) COMPLET=1 ;;
    --reparer) REPARER=1 ;;
    --ecriture-en-cours) ECRITURE_EN_COURS=1 ;;
    *) echo "option inconnue : $a" >&2; exit 2 ;;
  esac
done

[ -f "$REF" ] || { echo "❌ $REF absent : aucune référence de version" >&2; exit 2; }
lire() { python3 -c "import json,sys;d=json.load(open('$REF'));print(eval('d'+sys.argv[1]))" "$1"; }

BRANCHE_ATTENDUE="$(lire "['branche_de_service']")"
# ── LE TAG EST LA RÉFÉRENCE STABLE ──────────────────────────────────────────
# Un fichier ne peut pas figer le hash du commit qui le contient (l'amender
# change ce hash) : on référence donc un TAG, et la porte vérifie qu'il pointe le
# commit courant. `commit_valide` reste un indice lisible, mais il ne décide plus.
TAG_ATTENDU="$(lire "['tag_valide']" 2>/dev/null || echo '')"
COMMIT_TAG="$(git -C "$(lire "['arbre_de_service']")" rev-list -n1 "$TAG_ATTENDU" 2>/dev/null | cut -c1-8)"
COMMIT_ATTENDU="${COMMIT_TAG:-$(lire "['commit_valide']")}"
ARBRE_SERVICE="$(lire "['arbre_de_service']")"
ETAT_UI="$(lire "['reference_ui']")"
UNITE="$(lire "['service']['unite']")"
PORT="$(lire "['service']['port']")"
BINAIRE="$(lire "['service']['binaire']")"

echecs=0
ok()   { printf '  ✅ %s\n' "$1"; }
ko()   { printf '  ❌ %s\n' "$1"; echecs=$((echecs + 1)); }
info() { printf '      %s\n' "$1"; }
titre(){ printf '\n── %s\n' "$1"; }

echo "PORTE DE VERSION — $(date -Is)"
info "référence : $REF"

# ── 1. L'ARBRE DE SERVICE ───────────────────────────────────────────────────
titre "1) l'arbre de service"
BRANCHE_REELLE="$(git -C "$ARBRE_SERVICE" rev-parse --abbrev-ref HEAD 2>/dev/null || echo inconnue)"
COMMIT_REEL="$(git -C "$ARBRE_SERVICE" rev-parse --short HEAD 2>/dev/null || echo inconnu)"
if [ -n "$TAG_ATTENDU" ]; then
  if [ -z "$COMMIT_TAG" ]; then
    ko "le tag de référence « $TAG_ATTENDU » n'existe pas dans le dépôt"
  elif [ "$COMMIT_TAG" = "$COMMIT_REEL" ]; then
    ok "tag « $TAG_ATTENDU » → $COMMIT_TAG (le commit courant : la version est celle qui est validée)"
  else
    ko "tag « $TAG_ATTENDU » → $COMMIT_TAG ≠ commit courant « $COMMIT_REEL » — la version a bougé depuis la validation"
    info "soit on valide ce nouvel état (tag à déplacer), soit on revient au commit du tag"
  fi
fi
[ "$BRANCHE_REELLE" = "$BRANCHE_ATTENDUE" ] \
  && ok "branche : $BRANCHE_REELLE" \
  || ko "branche « $BRANCHE_REELLE » ≠ attendue « $BRANCHE_ATTENDUE »"
if [ "$COMMIT_REEL" = "$COMMIT_ATTENDU" ]; then
  ok "commit : $COMMIT_REEL (le commit validé)"
else
  ko "commit « $COMMIT_REEL » ≠ validé « $COMMIT_ATTENDU »"
  info "$(git -C "$ARBRE_SERVICE" log --oneline -1 "$COMMIT_REEL" 2>/dev/null | head -c 120)"
  info "validé : $(git -C "$ARBRE_SERVICE" log --oneline -1 "$COMMIT_ATTENDU" 2>/dev/null | head -c 120)"
fi
# ── ET RIEN D'INCOMMITÉ : c'est la cause de la dérive ───────────────────────
SALE="$(git -C "$ARBRE_SERVICE" status --porcelain 2>/dev/null | wc -l)"
if [ "$SALE" = "0" ]; then
  ok "aucune modification non commitée"
else
  # ── UNE ÉCRITURE EN COURS N'EST PAS UNE DÉRIVE SILENCIEUSE ────────────────
  # Défaut mesuré : d'autres sessions écrivent dans cet arbre PENDANT qu'on
  # vérifie. La porte doit le dire sans confondre deux choses très différentes :
  #   · la version SERVIE a dérivé (grave : la page n'est plus celle qui est
  #     validée) ;
  #   · quelqu'un travaille (normal, mais il faut le SAVOIR, parce que la
  #     prochaine promotion emportera son travail s'il n'est pas commité).
  # Le second cas s'écarte EXPLICITEMENT par `--ecriture-en-cours`, et cela reste
  # une exception nommée dans le verdict — jamais un silence.
  if [ "${ECRITURE_EN_COURS:-0}" = "1" ]; then
    printf '  ⚠ %s fichier(s) en cours de modification par un autre chantier (exception nommée --ecriture-en-cours)\n' "$SALE"
  else
    ko "$SALE fichier(s) modifié(s) NON commité(s) — c'est ce qui fait diverger la version SERVIE de la référence"
  fi
  git -C "$ARBRE_SERVICE" status --porcelain | head -5 | while read -r l; do info "$l"; done
  [ "$REPARER" = "1" ] && info "(--reparer ne commit pas à ta place : un commit est un message, pas un rebase automatique)"
fi

# ── 2. L'UI SERVIE ──────────────────────────────────────────────────────────
titre "2) l'UI servie"
if [ ! -f "$ETAT_UI" ]; then
  ko "manifeste absent : $ETAT_UI"
else
  VERSION_UI="$(python3 -c "import json;print(json.load(open('$ETAT_UI')).get('version',''))")"
  HORS_LIGNE="$(python3 -c "import json;print(json.load(open('$ETAT_UI')).get('hors_ligne',''))")"
  info "version promue : $VERSION_UI"
  [ "$HORS_LIGNE" = "$COMMIT_ATTENDU" ] \
    && ok "elle nomme le commit validé ($HORS_LIGNE)" \
    || ko "elle nomme « $HORS_LIGNE » ≠ validé « $COMMIT_ATTENDU » — la page servie n'est pas exactement la version validée"
fi

# ── 3. LE SERVICE ───────────────────────────────────────────────────────────
titre "3) le service"
ETAT="$(systemctl --user is-active "$UNITE" 2>/dev/null || echo inactif)"
[ "$ETAT" = "active" ] && ok "$UNITE est actif" || ko "$UNITE est « $ETAT »"
EXE_SERVICE="$(systemctl --user show "$UNITE" -p ExecStart --value 2>/dev/null | grep -oE '/[^ ]*target/release/obrain' | head -1)"
[ "$EXE_SERVICE" = "$BINAIRE" ] \
  && ok "il tourne sur le binaire attendu" \
  || ko "binaire « ${EXE_SERVICE:-?} » ≠ attendu « $BINAIRE »"
[ -x "$BINAIRE" ] && info "binaire : $(stat -c '%y' "$BINAIRE" | cut -d. -f1)" || ko "binaire introuvable"
CODE="$(curl -s -m 8 -o /dev/null -w '%{http_code}' "http://192.168.1.103:$PORT/api/stores" 2>/dev/null)"
[ "$CODE" != "000" ] && ok "le port $PORT répond (code $CODE)" || ko "le port $PORT ne répond pas"

# ── 4. CE QUE LE SERVEUR SERT EST LA VERSION PROMUE ─────────────────────────
titre "4) ce qui est SERVI"
if [ -f "$ETAT_UI" ]; then
  BASE="$(lire "['interface_base']")"
  ASSETS_ATTENDUS="$(python3 -c "
import json;d=json.load(open('$ETAT_UI'))
print(' '.join(a['nom'] for a in d.get('assets_servis',[])))")"
  if [ -n "$ASSETS_ATTENDUS" ]; then
    # On interroge le SERVICE (derrière l'IAM, les actifs statiques restent
    # publics — c'est ce qui rend ce contrôle possible sans identité).
    for a in $ASSETS_ATTENDUS; do
      attendu="$(python3 -c "
import json;d=json.load(open('$ETAT_UI'))
print(next((x['sha256'] for x in d.get('assets_servis',[]) if x['nom']=='$a'),''))")"
      servi="$(curl -s -m 20 "http://192.168.1.103:$PORT/app/assets/$a" 2>/dev/null | sha256sum | cut -d' ' -f1)"
      [ "$servi" = "$attendu" ] && ok "actif servi à l'identique : $a" \
        || ko "actif DIVERGENT : $a (servi ${servi:0:12}… ≠ promu ${attendu:0:12}…)"
    done
  else
    info "aucun actif déclaré dans le manifeste"
  fi
fi

# ── 5. LES PORTES, SUR DEMANDE ──────────────────────────────────────────────
if [ "$COMPLET" = "1" ]; then
  titre "5) les portes de la version (--complet)"
  run_porte() {
    local cmd="$1" nom="$2"
    printf '  … %s\n' "$nom"
    if bash -c "$cmd" >/tmp/version-porte.log 2>&1; then ok "$nom"
    else ko "$nom — voir /tmp/version-porte.log"; tail -3 /tmp/version-porte.log | while read -r l; do info "$l"; done; fi
  }
  for m in ancrage memoire resonance; do
    run_porte "cd '$ARBRE_SERVICE' && cargo test --release -p obrain-cli --features ai,iam --bin obrain ${m}::" "tests $m"
  done
  info "les portes d'acceptation et d'interface coûtent des minutes : elles sont listées dans VERSION.json §portes"
else
  titre "5) les portes"
  info "non exécutées — relancer avec --complet (elles démarrent des serveurs jetables)"
fi

titre "verdict"
if [ "$echecs" = "0" ]; then
  printf '  ✅ ce qui tourne EST la version validée (%s)\n' "$COMMIT_ATTENDU"
  exit 0
fi
printf '  ❌ %d écart(s) entre ce qui tourne et la version validée\n' "$echecs"
printf '     la référence est %s — corriger, ou y reporter la version validée\n' "$REF"
exit 1
