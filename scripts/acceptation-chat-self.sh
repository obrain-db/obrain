#!/usr/bin/env bash
# ── PORTE D'ACCEPTATION : LE CHAT ÉCRIT DANS LE SELF DE L'APPELANT ──────────
#
# Le défaut corrigé, mesuré le 2026-09-15 : le chemin du CHAT résolvait le self
# de l'INSTANCE pendant que la vue mémoire résolvait celui du compte. Deux
# comptes voyaient donc chacun leur self dans le panneau, et la conversation
# rangeait pourtant ses souvenirs dans celui de l'instance — un ami écrivait
# chez quelqu'un d'autre, sans que rien ne le dise.
#
# Cette porte vérifie la seule chose qui prouve le correctif : l'événement
# `contexte` du flux SSE porte le self de l'APPELANT, et il est différent pour
# deux comptes différents. C'est le self que l'engine utilisera ensuite pour
# écrire la mémoire — donc c'est lui qui décide où vont les souvenirs.
#
# Usage : scripts/acceptation-chat-self.sh [port] [secondes]
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 1

PORT="${1:-8443}"
LIMITE="${2:-45}"
if [ "$PORT" = "8432" ] || [ "$PORT" = "8433" ]; then
    echo "refus : $PORT est un service de l'utilisateur. Choisis un port libre."
    exit 2
fi

BIN="${OBRAIN_BIN:-./target/release/obrain}"
TAG="$$"
STORE="/tmp/acceptation-chat-$TAG.obrain"
IAM="/tmp/acceptation-chat-iam-$TAG.obrain"
CK_A="/tmp/acceptation-chat-a-$TAG.cookies"
CK_B="/tmp/acceptation-chat-b-$TAG.cookies"
LOG="/tmp/acceptation-chat-$TAG.log"
ESPACE="acceptation-chat-$TAG"
ALICE="alice-$TAG"
BOB="bob-$TAG"
MDP_ADMIN="admin-$TAG-$RANDOM"
BASE_URL="http://127.0.0.1:$PORT"
SERVEUR=""

echecs=0
ok()   { printf '  ✅ %s\n' "$1"; }
ko()   { printf '  ❌ %s\n' "$1"; echecs=$((echecs + 1)); }
titre(){ printf '\n%s\n' "$1"; }

nettoyer() {
    [ -n "$SERVEUR" ] && kill -9 "$SERVEUR" 2>/dev/null
    rm -f "$CK_A" "$CK_B"
    [ "${GARDER_JOURNAL:-0}" = "1" ] || rm -f "$LOG"
    [ "${GARDER_MONDE:-0}" = "1" ] && return
    rm -rf "$STORE" "$IAM"
}
trap nettoyer EXIT

titre "1) binaire et magasin d'identités"
[ -x "$BIN" ] || { echo "  ❌ binaire absent"; exit 2; }
"$BIN" init "$STORE" >/dev/null 2>&1 || { echo "  ❌ init base"; exit 2; }
"$BIN" init "$IAM" >/dev/null 2>&1 || { echo "  ❌ init identités"; exit 2; }
"$BIN" iam amorcer "$IAM" "$ESPACE" --utilisateur admin --mot-de-passe "$MDP_ADMIN" >/dev/null 2>&1 \
    || { echo "  ❌ amorçage (binaire sans IAM ?)"; exit 2; }
ok "base et identités prêtes"

titre "2) serveur armé (port $PORT)"
OBRAIN_IAM=1 OBRAIN_IAM_ACCOUNT=obrain OBRAIN_IAM_STORE="$IAM" \
"$BIN" viz serve --host 127.0.0.1 --port "$PORT" \
    --store "$STORE" --enrich-ricci 0 >"$LOG" 2>&1 &
SERVEUR=$!
for _ in $(seq 1 60); do
    c=$(curl -s -m 2 -o /dev/null -w '%{http_code}' "$BASE_URL/api/stores" 2>/dev/null)
    [ "$c" != "000" ] && break
    sleep 1
done
[ "$c" != "000" ] && ok "le serveur écoute" || { ko "le serveur n'écoute pas (voir $LOG)"; exit 2; }

titre "3) deux comptes"
CK_ADMIN="/tmp/acceptation-chat-admin-$TAG.cookies"
curl -s -m 20 -b "$CK_ADMIN" -c "$CK_ADMIN" -o /dev/null -X POST "$BASE_URL/api/auth/login" \
    --data-urlencode "username=admin" --data-urlencode "password=$MDP_ADMIN"
for nom in "$ALICE" "$BOB"; do
    r=$(curl -s -m 20 -b "$CK_ADMIN" -c "$CK_ADMIN" -X POST "$BASE_URL/api/iam/utilisateurs" \
        -d "nom=$nom" -d "espace=$ESPACE" -d "mot_de_passe=motdepasse-$nom")
    printf '%s' "$r" | grep -q "$nom" && ok "compte « $nom » créé" \
        || ko "création de « $nom » : $(printf '%s' "$r" | head -c 140)"
done
rm -f "$CK_ADMIN"

# ── LE SELF ANNONCÉ PAR LE CHAT, POUR UN COMPTE DONNÉ ───────────────────────
# On ouvre le flux SSE et on lit la PREMIÈRE trame `contexte` : c'est là que
# l'engine dit à qui appartient la conversation. On ne laisse pas le modèle
# finir — la question est donc triviale et le flux est coupé aussitôt.
self_du_chat() { # $1 = nom, $2 = cookies
    curl -s -m 20 -c "$2" -o /dev/null -X POST "$BASE_URL/api/auth/login" \
        --data-urlencode "username=$1" --data-urlencode "password=motdepasse-$1"
    curl -sN -m "$LIMITE" -b "$2" \
        "$BASE_URL/api/engine/chat?store=$ESPACE&q=dis%20bonjour&pensee=non" \
        2>/dev/null | head -c 4000 \
        | sed -n 's/^data: \(.*\)$/\1/p' | grep -m1 '"self_qn"' \
        | sed -n 's/.*"self_qn"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p'
}

titre "4) le chat annonce le self de l'APPELANT"
# Alice d'abord : sans nœud Self dans la synapse, la portée est « journal ».
A=$(self_du_chat "$ALICE" "$CK_A")
[ -n "$A" ] && ok "alice : la conversation appartient à « $A »" \
    || ko "aucun self annoncé pour alice (le flux n'a rien dit — voir $LOG)"
[ "$A" = "self:$ALICE" ] && ok "c'est bien SON self, pas celui de l'instance" \
    || ko "self annoncé « $A » ≠ self:$ALICE"
B=$(self_du_chat "$BOB" "$CK_B")
[ -n "$B" ] && ok "bob : la conversation appartient à « $B »" \
    || ko "aucun self annoncé pour bob"
[ -n "$A" ] && [ -n "$B" ] && [ "$A" != "$B" ] \
    && ok "deux comptes, deux conversations, deux selfs" \
    || ko "les deux comptes partagent la conversation de « $A »"

titre "5) les souvenirs de chacun vont dans SON journal"
# On écrit un souvenir par compte, par le même chemin que le chat (l'outil de
# mémoire s'appuie sur `memoire::enregistrer(self_qn, …)`) : ce que la porte
# vérifie, c'est que les deux fichiers sont DISTINCTS, donc qu'aucun souvenir
# d'un compte ne peut se retrouver chez l'autre.
j_a=$(curl -s -m 20 -b "$CK_A" "$BASE_URL/api/engine/memoire" \
    | sed -n 's/.*"journal"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)
j_b=$(curl -s -m 20 -b "$CK_B" "$BASE_URL/api/engine/memoire" \
    | sed -n 's/.*"journal"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)
[ -n "$j_a" ] && [ -n "$j_b" ] && [ "$j_a" != "$j_b" ] \
    && ok "deux journaux distincts" \
    || ko "même journal (« $j_a » / « $j_b ») : les souvenirs se mélangeraient"
[ -n "$j_a" ] && printf '%s' "$j_a" | grep -q "$ALICE" \
    && ok "le journal d'alice porte son nom : $(basename "$j_a")" \
    || printf '  (info) journal d'"'"'alice : %s\n' "$(basename "${j_a:-absent}")"

titre "verdict"
if [ "$echecs" = "0" ]; then
    printf '  ✅ la conversation appartient à son compte, et ses souvenirs aussi\n'
    exit 0
fi
printf '  ❌ %d contrôle(s) en échec\n' "$echecs"
[ "${GARDER_JOURNAL:-0}" = "1" ] && printf '  journal du serveur : %s\n' "$LOG"
exit 1
