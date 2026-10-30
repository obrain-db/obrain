#!/usr/bin/env bash
# ── PORTE D'ACCEPTATION : UN COMPTE NON-ADMIN AJOUTE SON MONDE, ENRICHI ─────
#
# La demande, mot pour mot : « dans un premier temps je crée les comptes via le
# compte admin, les users doivent pouvoir ajouter un repos github avec un
# enrichissement complet ».
#
# Ce script la vérifie de bout en bout, sans la main, sur un port LIBRE et des
# fichiers TEMPORAIRES — jamais sur 8432/8433, qui sont les services de
# l'utilisateur.
#
# ── CE QU'IL PROUVE, ET QUI N'ÉTAIT PAS PROUVÉ AVANT ────────────────────────
#   1. le binaire porte la chaîne IAM (sinon il le dit et sort en 2) ;
#   2. un administrateur CRÉE un compte, avec un espace et un mot de passe ;
#   3. ce compte SE CONNECTE, et n'a AUCUN droit d'administration (le plafond
#      du compte neuf est vérifié, pas supposé) ;
#   4. avec ses seuls droits, il AJOUTE un dépôt GitHub (`POST /api/worlds`) ;
#   5. le travail traverse clone → indexation → ENRICHISSEMENT ;
#   6. la base produite PORTE le relief : la courbure est sur les arêtes ;
#   7. le monde est ATTRIBUÉ à ce compte (le registre porte son identité).
#
# Le point 6 lit la BASE, pas le rapport HTTP : un rapport peut mentir, une
# arête qui porte sa courbure non.
#
# Usage :
#   scripts/acceptation-monde-utilisateur.sh [port] [dépôt-github] [secondes]
# Défauts : port 8441, dépôt sindresorhus/is-npm (petit, mais avec de vrais
# appels entre fonctions — sans quoi il n'y a pas de relief à mesurer).
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 1

PORT="${1:-8441}"
DEPOT="${2:-https://github.com/sindresorhus/is-npm}"
LIMITE="${3:-180}"

if [ "$PORT" = "8432" ] || [ "$PORT" = "8433" ]; then
    echo "refus : $PORT est un service de l'utilisateur. Choisis un port libre."
    exit 2
fi

BIN="${OBRAIN_BIN:-./target/release/obrain}"
TAG="$$"
STORE="/tmp/acceptation-monde-$TAG.obrain"
IAM="/tmp/acceptation-monde-iam-$TAG.obrain"
MONDES="/tmp/acceptation-monde-worlds-$TAG"
CK="/tmp/acceptation-monde-$TAG.cookies"
LOG="/tmp/acceptation-monde-$TAG.log"
# L'ESPACE est le nom de la base servie : la politique s'y juge (voir
# `espace_vise`), donc le compte doit y avoir `graph:write`.
ESPACE="acceptation-monde-$TAG"
MDP_ADMIN="admin-$TAG-$RANDOM"
MDP_USER="user-$TAG-$RANDOM"
USER="alice-$TAG"
BASE_URL="http://127.0.0.1:$PORT"
SERVEUR=""

echecs=0
ok()   { printf '  ✅ %s\n' "$1"; }
ko()   { printf '  ❌ %s\n' "$1"; echecs=$((echecs + 1)); }
titre(){ printf '\n%s\n' "$1"; }

nettoyer() {
    [ -n "$SERVEUR" ] && kill -9 "$SERVEUR" 2>/dev/null
    rm -f "$CK" "$CK.n"
    [ "${GARDER_JOURNAL:-0}" = "1" ] || rm -f "$LOG"
    # ── LE MONDE EST CONSERVÉ SUR DEMANDE ──────────────────────────────────
    # Un contrôle qu'on ne peut pas INSPECTER après coup oblige à le relancer
    # pour comprendre un échec. `GARDER_MONDE=1` laisse la base produite, son
    # registre et le magasin d'identités : on peut alors rejouer la lecture
    # du relief à la main, sur exactement ce que la porte a mesuré.
    if [ "${GARDER_MONDE:-0}" = "1" ]; then
        printf '\n  (conservé) monde : %s\n' "$MONDES"
        printf '  (conservé) base servie : %s · identités : %s\n' "$STORE" "$IAM"
        return
    fi
    rm -rf "$STORE" "$IAM" "$MONDES"
}
trap nettoyer EXIT

# ── 1. LE BINAIRE, ET LA PREUVE QU'IL PORTE L'IAM ───────────────────────────
titre "1) binaire ($BIN)"
if [ ! -x "$BIN" ]; then
    echo "  ❌ $BIN absent — construire : cargo build --release -p obrain-cli --features ai,iam"
    exit 2
fi
SONDE="/tmp/acceptation-monde-sonde-$TAG.obrain"
"$BIN" init "$SONDE" >/dev/null 2>&1
if "$BIN" iam utilisateurs "$SONDE" >/dev/null 2>&1; then
    ok "la chaîne IAM répond (le binaire la porte)"
else
    ko "IAM NON COMPILÉ dans ce binaire — relancer : cargo build --release -p obrain-cli --features ai,iam"
    rm -rf "$SONDE"
    exit 2
fi
rm -rf "$SONDE"

# ── 2. LA BASE SERVIE ET LE MAGASIN D'IDENTITÉS ─────────────────────────────
titre "2) base servie + magasin d'identités"
"$BIN" init "$STORE" >/dev/null 2>&1 || { echo "  ❌ init de la base a échoué"; exit 2; }
# Le magasin d'identités est une base obrain comme une autre : il s'initialise
# AVANT de recevoir un administrateur (défaut trouvé en écrivant cette porte :
# `iam amorcer` sur un magasin inexistant rend « Database not found »).
"$BIN" init "$IAM" >/dev/null 2>&1 || { echo "  ❌ init du magasin d'identités a échoué"; exit 2; }
AMORCAGE="$("$BIN" iam amorcer "$IAM" "$ESPACE" --utilisateur admin --mot-de-passe "$MDP_ADMIN" 2>&1)"
if printf '%s' "$AMORCAGE" | grep -qi admin; then
    ok "administrateur amorcé sur l'espace « $ESPACE »"
else
    ko "amorçage impossible : $(printf '%s' "$AMORCAGE" | head -2 | tr '\n' ' ')"
    exit 2
fi

# ── 3. LE SERVEUR ARMÉ, SUR UN PORT LIBRE ───────────────────────────────────
titre "3) serveur armé (port $PORT)"
OBRAIN_IAM=1 \
OBRAIN_IAM_ACCOUNT=obrain \
OBRAIN_IAM_STORE="$IAM" \
OBRAIN_WORLDS_DIR="$MONDES" \
"$BIN" viz serve --host 127.0.0.1 --port "$PORT" \
    --store "$STORE" --enrich-ricci 0 >"$LOG" 2>&1 &
SERVEUR=$!
for _ in $(seq 1 60); do
    code=$(curl -s -m 2 -o /dev/null -w '%{http_code}' "$BASE_URL/api/stores" 2>/dev/null)
    [ "$code" != "000" ] && break
    sleep 1
done
if [ "$code" = "000" ]; then
    ko "le serveur n'écoute pas (voir $LOG)"
    exit 2
fi
ok "le serveur écoute et REFUSE sans identité (code $code)"

# ── 4. L'ADMIN CRÉE LE COMPTE ───────────────────────────────────────────────
titre "4) l'administrateur crée le compte"
curl -s -m 20 -c "$CK" -o /dev/null "$BASE_URL/login" || true
# ── LE SUCCÈS D'UN FORMULAIRE EST UNE REDIRECTION, PAS UN 200 ──────────────
# Défaut mesuré en écrivant cette porte : `curl -d` envoie du
# `content-type: application/x-www-form-urlencoded`, donc le serveur prend la
# branche FORMULAIRE (celle du navigateur) et répond **303** vers l'accueil avec
# le cookie de session — c'est le comportement VOULU (une page HTML ne sait pas
# lire du JSON). Exiger 200 faisait échouer le contrôle sur un succès parfait.
code=$(curl -s -m 20 -b "$CK" -c "$CK" -o /dev/null -w '%{http_code}' \
    -X POST "$BASE_URL/api/auth/login" \
    --data-urlencode "username=admin" --data-urlencode "password=$MDP_ADMIN")
{ [ "$code" = "200" ] || [ "$code" = "303" ]; } \
    && ok "connexion administrateur ($code)" || ko "connexion administrateur → $code"

REPONSE=$(curl -s -m 20 -b "$CK" -c "$CK" -X POST "$BASE_URL/api/iam/utilisateurs" \
    -d "nom=$USER" -d "espace=$ESPACE" -d "mot_de_passe=$MDP_USER")
if printf '%s' "$REPONSE" | grep -q "$USER"; then
    ok "compte « $USER » créé, avec l'espace « $ESPACE »"
else
    ko "création du compte : $(printf '%s' "$REPONSE" | head -c 200)"
fi

# ── 5. LE COMPTE NE SE CONNECTE PAS... SANS SON MOT DE PASSE ────────────────
# Le contrôle NÉGATIF d'abord : un compte neuf qui s'ouvrirait sans mot de passe
# serait une porte ouverte, et ce serait plus grave que le reste.
UCOOK="/tmp/acceptation-monde-user-$TAG.cookies"
code=$(curl -s -m 20 -o /dev/null -w '%{http_code}' -X POST "$BASE_URL/api/auth/login" \
    --data-urlencode "username=$USER" --data-urlencode "password=mauvais-mot-de-passe")
[ "$code" = "401" ] && ok "mot de passe faux → 401" || ko "mot de passe faux → $code (401 attendu)"

code=$(curl -s -m 20 -c "$UCOOK" -o /dev/null -w '%{http_code}' -X POST "$BASE_URL/api/auth/login" \
    --data-urlencode "username=$USER" --data-urlencode "password=$MDP_USER")
{ [ "$code" = "200" ] || [ "$code" = "303" ]; } \
    && ok "connexion du compte utilisateur ($code)" || ko "connexion utilisateur → $code"

# ── 6. LE PLAFOND : CE COMPTE N'ADMINISTRE PAS ──────────────────────────────
titre "5) le compte n'a aucun droit d'administration"
code=$(curl -s -m 20 -b "$UCOOK" -o /dev/null -w '%{http_code}' \
    "$BASE_URL/api/iam/utilisateurs?espace=$ESPACE")
{ [ "$code" = "403" ] || [ "$code" = "401" ]; } \
    && ok "liste des comptes → $code (refusé)" \
    || ko "liste des comptes → $code : un compte neuf peut administrer !"

REPONSE=$(curl -s -m 20 -b "$UCOOK" -X POST "$BASE_URL/api/iam/utilisateurs" \
    -d "nom=intrus-$TAG" -d "espace=$ESPACE")
code=$(printf '%s' "$REPONSE" | head -c 3)
if printf '%s' "$REPONSE" | grep -qiE "refus|admin|interdit"; then
    ok "créer un compte → refusé"
else
    ko "créer un compte → « $(printf '%s' "$REPONSE" | head -c 160) »"
fi

# ── 7. LE GESTE : AJOUTER SON MONDE ─────────────────────────────────────────
titre "6) l'utilisateur ajoute un dépôt GitHub"
LANCEMENT=$(curl -s -m 30 -b "$UCOOK" -X POST "$BASE_URL/api/worlds" \
    -H 'Content-Type: application/json' -d "{\"url\":\"$DEPOT\"}")
JOB=$(printf '%s' "$LANCEMENT" | sed -n 's/.*"job"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
if [ -n "$JOB" ]; then
    ok "ajout accepté — travail « $JOB »"
else
    ko "ajout refusé : $(printf '%s' "$LANCEMENT" | head -c 220)"
fi

ETAT=""; ETAPES=""
if [ -n "$JOB" ]; then
    fin=$(( $(date +%s) + LIMITE ))
    while [ "$(date +%s)" -lt "$fin" ]; do
        ETAT=$(curl -s -m 20 -b "$UCOOK" "$BASE_URL/api/worlds/status?job=$JOB")
        e=$(printf '%s' "$ETAT" | sed -n 's/.*"etat"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
        case "$e" in *enrichissement*) case "$ETAPES" in *enrichissement*) ;; *) ETAPES="$ETAPES → enrichissement" ;; esac ;; esac
        [ "$e" = "fini" ] || [ "$e" = "erreur" ] && break
        sleep 3
    done
fi

e=$(printf '%s' "$ETAT" | sed -n 's/.*"etat"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
[ "$e" = "fini" ] && ok "travail terminé (étapes$ETAPES)" \
    || ko "travail en état « $e » : $(printf '%s' "$ETAT" | head -c 240)"

# Le rapport d'enrichissement : ce que l'écran affichera.
enrich_ms=$(printf '%s' "$ETAT" | sed -n 's/.*"enrich_ms"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p')
visitees=$(printf '%s' "$ETAT" | sed -n 's/.*"aretes_visitees"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p')
erreurs=$(printf '%s' "$ETAT" | sed -n 's/.*"enrich_erreurs"[[:space:]]*:\[\([^]]*\)\].*/\1/p')
[ -n "$enrich_ms" ] && ok "enrichissement rapporté : ${enrich_ms} ms, ${visitees:-0} arête(s) visitée(s)" \
    || ko "AUCUN rapport d'enrichissement (enrich_ms absent)"
[ -z "$erreurs" ] && ok "aucune erreur d'enrichissement nommée" \
    || ko "enrichissement PARTIEL : $erreurs"

STORE_MONDE=$(printf '%s' "$ETAT" | sed -n 's/.*"store"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
attrib=$(printf '%s' "$ETAT" | sed -n 's/.*"proprietaire"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')

# ── 8. LA PREUVE PAR LA BASE, PAS PAR LE RAPPORT ────────────────────────────
titre "7) la base produite porte le relief (lecture directe)"
if [ -n "$STORE_MONDE" ] && [ -d "$STORE_MONDE" ]; then
    # On ARRÊTE le serveur : il tient le verrou exclusif du monde qu'il a créé,
    # et une lecture concurrente échouerait — pas parce que la base est vide.
    kill -9 "$SERVEUR" 2>/dev/null; SERVEUR=""
    sleep 2
    INFO="$("$BIN" info "$STORE_MONDE" 2>/dev/null)"
    # ── ON LIT LE JSON, PAS DU TEXTE HUMAIN ────────────────────────────────
    # Défaut mesuré en écrivant cette porte : `info` écrit aussi des WARN du
    # substrat, et une expression qui cherchait le mot « Nodes » dans la sortie
    # tombait sur ces avertissements — la porte annonçait « le monde ne se lit
    # pas » sur une base parfaitement lisible. Un contrôle qui analyse de la
    # prose casse au premier changement de prose : on lit les champs.
    noeuds=$(printf '%s' "$INFO" | sed -n 's/.*"node_count"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p' | head -1)
    aretes=$(printf '%s' "$INFO" | sed -n 's/.*"edge_count"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p' | head -1)
    if [ -n "$noeuds" ]; then
        ok "le monde se lit : $noeuds nœuds, ${aretes:-?} arêtes"
    else
        ko "le monde ne se lit pas ($STORE_MONDE) : $(printf '%s' "$INFO" | head -c 200)"
    fi

    # ── LA COURBURE SE LIT PAR L'OVERLAY, PAS PAR GQL ───────────────────────
    # Défaut mesuré en écrivant cette porte : `MATCH ()-[r]->() WHERE
    # r.ricci_f32 IS NOT NULL` rend 0 même sur un monde parfaitement enrichi —
    # la courbure vit dans l'`EdgeRecord` du substrat, que GQL n'expose pas
    # comme propriété d'arête. La lecture qui MARCHE est l'overlay `ricci`, qui
    # annonce lui-même combien d'arêtes il a stylées. C'est cette mesure-là
    # qu'on exige : un monde brut en stylerait ZÉRO, et ne pourrait pas mentir.
    CENTRE=$("$BIN" query --read-only --format json "$STORE_MONDE" \
        "MATCH (n) RETURN n.qualified_name AS qn LIMIT 1" 2>/dev/null \
        | sed -n 's/.*"qn"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)
    if [ -z "$CENTRE" ]; then
        ko "aucun nœud lisible dans le monde : la base est vide"
    else
        RELIEF=$("$BIN" viz subgraph --format json --overlay ricci "$STORE_MONDE" "$CENTRE" 2>&1 | head -3 | tr '\n' ' ')
        stylees=$(printf '%s' "$RELIEF" | sed -n 's/.*\/[[:space:]]*\([0-9][0-9]*\)[[:space:]]*arêtes stylées.*/\1/p')
        if [ -n "$stylees" ] && [ "$stylees" -gt 0 ]; then
            ok "courbure persistée : $stylees arête(s) stylée(s) sur $aretes"
        else
            ko "aucune courbure lisible — le monde est arrivé BRUT : $(printf '%s' "$RELIEF" | head -c 200)"
        fi
    fi
else
    ko "aucun store de monde dans le rapport (store absent)"
fi

titre "verdict"
if [ "$echecs" = "0" ]; then
    printf '  ✅ un compte non-admin ajoute son monde, et il arrive ENRICHI\n'
    exit 0
fi
printf '  ❌ %d contrôle(s) en échec\n' "$echecs"
[ "${GARDER_JOURNAL:-0}" = "1" ] && printf '  journal du serveur : %s\n' "$LOG"
exit 1
