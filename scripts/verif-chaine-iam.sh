#!/usr/bin/env bash
# VÉRIFICATION DE LA CHAÎNE IAM — reproductible, et sans moi.
#
# Pourquoi ce script existe : la chaîne d'authentification a été vérifiée « à la
# main », plusieurs fois, et deux de ces vérifications ont été perdues — une
# reconstruction concurrente du binaire (sans `iam`) et une édition en cours d'un
# fichier partagé. Une chaîne qu'on ne peut pas REVÉRIFIER n'est pas stabilisée.
#
# Ce qu'il fait, dans l'ordre, et sans rien laisser derrière lui :
#   1. vérifie que le binaire CONTIENT l'IAM (sinon il le dit et sort en 2) ;
#   2. amorce un magasin d'identités dédié (utilisateur + mot de passe + politiques) ;
#   3. lance un serveur ARMÉ sur un port libre (8435 par défaut, jamais 8432/8433) ;
#   4. déroule la chaîne : accueil → /login → connexion → cookie → requête →
#      whoami → renouvellement → rotation → piste d'audit → déconnexion → refus ;
#   5. arrête le serveur, et rend un verdict (code 0 si tout passe).
#
# Usage :
#   scripts/verif-chaine-iam.sh [base-de-donnees.obrain] [port] [magasin-iam.obrain]
#
# Par défaut : la base LIBRE `runtime.obrain` (jamais une base servie), le port
# 8435, et `iam-verif.obrain` comme magasin d'identités.
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 1

DATA_DIR="${OBRAIN_HARNESS_DATA:-/home/triviere/.local/share/obrain-harness/data}"
BASE="${1:-$DATA_DIR/runtime.obrain}"
PORT="${2:-8435}"
IAM="${3:-$DATA_DIR/iam-verif.obrain}"
UTILISATEUR="verif"
MOT_DE_PASSE="verif-$(date +%s)-$RANDOM"
ESPACE="$(basename "$BASE" .obrain)"
BIN="./target/release/obrain"
EXEMPLE="./target/release/examples/amorcer_iam"
LOG="/tmp/verif-chaine-iam-$$.log"
CK="/tmp/verif-chaine-iam-$$.cookies"

echecs=0
ok()   { printf '  ✅ %s\n' "$1"; }
ko()   { printf '  ❌ %s\n' "$1"; echecs=$((echecs + 1)); }
titre(){ printf '\n%s\n' "$1"; }

nettoyer() {
    [ -n "${SERVEUR:-}" ] && kill -9 "$SERVEUR" 2>/dev/null
    rm -f "$CK"
}
trap nettoyer EXIT

# ── 1. LE BINAIRE CONTIENT-IL L'IAM ? ───────────────────────────────────────
titre "1) binaire"
if [ ! -x "$BIN" ]; then
    echo "  ❌ $BIN absent — construire : cargo build --release -p obrain-cli --features ai,iam"
    exit 2
fi
if nm -C "$BIN" 2>/dev/null | grep -qi obrain_iam; then
    ok "IAM compilé dans le binaire ($(stat -c %y "$BIN" | cut -c1-19))"
else
    echo "  ❌ IAM ABSENT du binaire : il a été reconstruit sans la feature."
    echo "     Construire : cargo build --release -p obrain-cli --features ai,iam"
    echo "     (c'est la seule commande qui embarque la chaîne — \`--features ai\` ne l'inclut pas)"
    exit 2
fi

# ── 2. MAGASIN D'IDENTITÉS DÉDIÉ ────────────────────────────────────────────
titre "2) magasin d'identités ($IAM)"
[ -d "$IAM" ] || "$BIN" init "$IAM" >/dev/null 2>&1
if [ ! -d "$IAM" ]; then echo "  ❌ création impossible"; exit 2; fi
if [ -x "$EXEMPLE" ]; then
    SORTIE="$("$EXEMPLE" "$IAM" obrain "$ESPACE" "$UTILISATEUR" "$MOT_DE_PASSE" 2>&1)"
else
    SORTIE="$(cargo run --release -q -p obrain-cli --features ai,iam --example amorcer_iam -- \
        "$IAM" obrain "$ESPACE" "$UTILISATEUR" "$MOT_DE_PASSE" 2>&1)"
fi
if printf '%s' "$SORTIE" | grep -q "Allow graph:read"; then
    ok "utilisateur amorcé, politiques bornées à l'espace « $ESPACE »"
else
    ko "amorçage : $(printf '%s' "$SORTIE" | head -2 | tr '\n' ' ')"
fi

# ── 3. SERVEUR ARMÉ, PORT LIBRE ─────────────────────────────────────────────
titre "3) serveur armé (port $PORT)"
OBRAIN_IAM=1 OBRAIN_IAM_STORE="$IAM" OBRAIN_IAM_ACCOUNT=obrain \
    "$BIN" viz serve --host 127.0.0.1 --port "$PORT" \
    --preset-default "Verif" --preset "Verif=$BASE" > "$LOG" 2>&1 &
SERVEUR=$!
B="http://127.0.0.1:$PORT"
for _ in $(seq 1 40); do
    [ "$(curl -s -o /dev/null -w '%{http_code}' -m 3 "$B/api/auth/whoami")" = "200" ] && break
    sleep 1
done
ETAT="$(grep -a 'viz serve :' "$LOG" | tail -1)"
case "$ETAT" in
    *"IAM ARMÉ"*) ok "garde armée" ;;
    *"NON COMPILÉ"*) ko "binaire sans IAM"; exit 2 ;;
    *) ko "garde NON armée : $ETAT" ;;
esac

# ── 4. LA CHAÎNE ────────────────────────────────────────────────────────────
titre "4) la chaîne"

code=$(curl -s -o /dev/null -w '%{http_code}' "$B/")
[ "$code" = "303" ] && ok "accueil sans session → 303 vers /login" || ko "accueil sans session → $code (303 attendu)"

code=$(curl -s -o /dev/null -w '%{http_code}' "$B/login")
champs=$(curl -s "$B/login" | grep -c 'name="password"')
[ "$code" = "200" ] && [ "$champs" = "1" ] && ok "page /login servie (formulaire)" || ko "page /login → $code, champs=$champs"

code=$(curl -s -m 30 -o /dev/null -w '%{http_code}' -X POST "$B/api/auth/login" \
    -d "username=$UTILISATEUR&password=mauvais")
[ "$code" = "401" ] && ok "connexion refusée (mauvais mot de passe) → 401" || ko "mauvais mot de passe → $code (401 attendu)"

code=$(curl -s -m 30 -c "$CK" -o /dev/null -w '%{http_code}' -X POST "$B/api/auth/login" \
    -d "username=$UTILISATEUR&password=$MOT_DE_PASSE")
J1="$(grep obrain_session "$CK" 2>/dev/null | awk '{print $7}')"
if [ "$code" = "303" ] && [ -n "$J1" ]; then
    ok "connexion par formulaire → 303 + cookie posé"
else
    ko "connexion par formulaire → $code, cookie=${J1:0:12}"
fi

code=$(curl -s -m 30 -o /dev/null -w '%{http_code}' -b "$CK" \
    --get --data-urlencode "store=$BASE" --data-urlencode "query=MATCH (n) RETURN count(n)" "$B/api/query")
[ "$code" = "200" ] && ok "requête AVEC le cookie → 200 (voie du navigateur)" || ko "requête avec cookie → $code (200 attendu)"

ident="$(curl -s -m 20 -b "$CK" "$B/api/auth/whoami" | sed -n 's/.*"identite":"\([^"]*\)".*/\1/p')"
[ -n "$ident" ] && ok "whoami → $ident" || ko "whoami sans identité"

audit="$(curl -s -m 20 -b "$CK" "$B/api/auth/audit?limit=5")"
case "$audit" in
    *'"verdict"'*) ok "piste d'audit lisible ($(printf '%s' "$audit" | grep -o '"action"' | wc -l) événement(s))" ;;
    *'"error"'*)   ko "piste d'audit refusée : $(printf '%s' "$audit" | head -c 120)" ;;
    *)             ko "piste d'audit illisible : $(printf '%s' "$audit" | head -c 120)" ;;
esac

code=$(curl -s -m 30 -c "$CK.n" -o /dev/null -w '%{http_code}' -X POST -b "$CK" "$B/api/auth/refresh")
J2="$(grep obrain_session "$CK.n" 2>/dev/null | awk '{print $7}')"
if [ "$code" = "200" ] && [ -n "$J2" ] && [ "$J1" != "$J2" ]; then
    ok "renouvellement → 200 et jeton tourné"
else
    ko "renouvellement → $code, rotation=$([ "$J1" != "$J2" ] && echo oui || echo non)"
fi
rm -f "$CK.n"

code=$(curl -s -m 20 -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $J1" \
    --get --data-urlencode "store=$BASE" --data-urlencode "query=MATCH (n) RETURN count(n)" "$B/api/query")
[ "$code" = "401" ] && ok "ANCIEN jeton après rotation → 401 (mort)" || ko "ancien jeton → $code (401 attendu)"

code=$(curl -s -m 20 -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $J2" \
    --get --data-urlencode "store=$BASE" --data-urlencode "query=MATCH (n) RETURN count(n)" "$B/api/query")
[ "$code" = "200" ] && ok "nouveau jeton → 200" || ko "nouveau jeton → $code (200 attendu)"

code=$(curl -s -m 20 -o /dev/null -w '%{http_code}' -X POST -b "$CK" "$B/api/auth/logout")
[ "$code" = "200" ] && ok "déconnexion → 200" || ko "déconnexion → $code"

code=$(curl -s -m 20 -o /dev/null -w '%{http_code}' -b "$CK" \
    --get --data-urlencode "store=$BASE" --data-urlencode "query=MATCH (n) RETURN count(n)" "$B/api/query")
[ "$code" = "401" ] && ok "session fermée → 401" || ko "après déconnexion → $code (401 attendu)"

en_tete=$(curl -s -m 20 -D - -o /dev/null "$B/api/query?store=$BASE&query=MATCH%20(n)%20RETURN%20count(n)" \
    | grep -ci '^www-authenticate: bearer')
[ "$en_tete" -ge 1 ] && ok "le 401 annonce WWW-Authenticate: Bearer" || ko "WWW-Authenticate absent du 401"

# ── 5. VERDICT ──────────────────────────────────────────────────────────────
titre "5) verdict"
if [ "$echecs" -eq 0 ]; then
    echo "  CHAÎNE COMPLÈTE — tout passe."
else
    echo "  $echecs étape(s) en échec. Journal du serveur : $LOG"
fi
exit "$echecs"
