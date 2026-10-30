#!/usr/bin/env bash
# ── PORTE D'ACCEPTATION : QUI EXISTE (le catalogue des selfs) ───────────────
#
# Le préalable de la résonance : on ne relie pas des selfs sans pouvoir dire
# lesquels existent. Cette porte vérifie que le catalogue les LIT sur ce qui est
# déjà là — journaux de mémoire, registre des mondes, nœuds `Self` de la
# synaptique — et qu'il ne ment pas quand il ne peut pas lire :
#
#   1. deux comptes existent, chacun avec sa mémoire ;
#   2. le catalogue les montre TOUS LES DEUX, nommés par leur utilisateur ;
#   3. il dit le journal de chacun et le nombre d'entrées (donc il a LECTURE de
#      la mémoire, pas seulement des noms) ;
#   4. le monde ajouté par un compte lui est attribué dans le catalogue ;
#   5. sans identité, la route REFUSE (on ne lit pas la population sans entrer) ;
#   6. le catalogue n'écrit rien : le nombre d'entrées ne bouge pas après appel.
#
# Usage : scripts/acceptation-selfs-catalogue.sh [port]
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 1

PORT="${1:-8444}"
if [ "$PORT" = "8432" ] || [ "$PORT" = "8433" ]; then
    echo "refus : $PORT est un service de l'utilisateur. Choisis un port libre."
    exit 2
fi

BIN="${OBRAIN_BIN:-./target/release/obrain}"
TAG="$$"
STORE="/tmp/acceptation-selfs-$TAG.obrain"
IAM="/tmp/acceptation-selfs-iam-$TAG.obrain"
CACHE="/tmp/acceptation-selfs-cache-$TAG"
MONDES="/tmp/acceptation-selfs-mondes-$TAG"
CK_A="/tmp/acceptation-selfs-a-$TAG.cookies"
LOG="/tmp/acceptation-selfs-$TAG.log"
ESPACE="acceptation-selfs-$TAG"
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
    rm -f "$CK_A"
    [ "${GARDER_JOURNAL:-0}" = "1" ] || rm -f "$LOG"
    [ "${GARDER_MONDE:-0}" = "1" ] && return
    rm -rf "$STORE" "$IAM" "$CACHE" "$MONDES"
}
trap nettoyer EXIT

titre "1) binaire, base, identités, cache de mémoire dédié"
[ -x "$BIN" ] || { echo "  ❌ binaire absent"; exit 2; }
"$BIN" init "$STORE" >/dev/null 2>&1 || { echo "  ❌ init base"; exit 2; }
"$BIN" init "$IAM" >/dev/null 2>&1 || { echo "  ❌ init identités"; exit 2; }
"$BIN" iam amorcer "$IAM" "$ESPACE" --utilisateur admin --mot-de-passe "$MDP_ADMIN" >/dev/null 2>&1 \
    || { echo "  ❌ amorçage (binaire sans IAM ?)"; exit 2; }
ok "prêt (cache de mémoire isolé : $CACHE)"

titre "2) serveur armé (port $PORT)"
# Le cache est ISOLÉ pour cette porte : le catalogue lit les journaux de
# mémoire, et lire ceux de l'utilisateur dans un test serait une indiscrétion.
OBRAIN_IAM=1 OBRAIN_IAM_ACCOUNT=obrain OBRAIN_IAM_STORE="$IAM" \
OBRAIN_VIZ_CACHE_DIR="$CACHE" OBRAIN_WORLDS_DIR="$MONDES" \
"$BIN" viz serve --host 127.0.0.1 --port "$PORT" \
    --store "$STORE" --enrich-ricci 0 >"$LOG" 2>&1 &
SERVEUR=$!
for _ in $(seq 1 60); do
    c=$(curl -s -m 2 -o /dev/null -w '%{http_code}' "$BASE_URL/api/stores" 2>/dev/null)
    [ "$c" != "000" ] && break
    sleep 1
done
[ "$c" != "000" ] && ok "le serveur écoute" || { ko "le serveur n'écoute pas (voir $LOG)"; exit 2; }

titre "3) deux comptes, chacun sa mémoire"
CK_ADMIN="/tmp/acceptation-selfs-admin-$TAG.cookies"
curl -s -m 20 -b "$CK_ADMIN" -c "$CK_ADMIN" -o /dev/null -X POST "$BASE_URL/api/auth/login" \
    --data-urlencode "username=admin" --data-urlencode "password=$MDP_ADMIN"
for nom in "$ALICE" "$BOB"; do
    r=$(curl -s -m 20 -b "$CK_ADMIN" -c "$CK_ADMIN" -X POST "$BASE_URL/api/iam/utilisateurs" \
        -d "nom=$nom" -d "espace=$ESPACE" -d "mot_de_passe=motdepasse-$nom")
    printf '%s' "$r" | grep -q "$nom" && ok "compte « $nom » créé" \
        || ko "création de « $nom » : $(printf '%s' "$r" | head -c 140)"
done
rm -f "$CK_ADMIN"

# ── UNE MÉMOIRE PAR COMPTE : on écrit une entrée dans le journal de CHACUN ──
# ── LE CHEMIN DU JOURNAL SE DEMANDE, IL NE SE DEVINE PAS ────────────────────
# Défaut mesuré en écrivant cette porte : le journal est nommé par un HACHAGE du
# self (`memoire::journal`), donc aucun test ne peut prédire son nom — et un
# test qui le prédirait mesurerait sa propre formule, pas le chemin réel. La
# route `/api/engine/memoire` REND le chemin : on l'interroge par compte, et on
# écrit dans LE fichier que le serveur vient de désigner.
journal_pour() { # $1 = nom → chemin du journal de ce compte
    local ck="/tmp/acceptation-selfs-$1.cookies"
    curl -s -m 20 -c "$ck" -o /dev/null -X POST "$BASE_URL/api/auth/login" \
        --data-urlencode "username=$1" --data-urlencode "password=motdepasse-$1"
    curl -s -m 20 -b "$ck" "$BASE_URL/api/engine/memoire" \
        | sed -n 's/.*"journal"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1
}

for nom in "$ALICE" "$BOB"; do
    j=$(journal_pour "$nom")
    if [ -z "$j" ]; then
        ko "le serveur ne donne pas le journal de « $nom »"
        continue
    fi
    mkdir -p "$(dirname "$j")"
    # Le format du journal est celui de `memoire::Entree::vers_json` : un objet
    # JSON par ligne, avec `self`, `provenance` et `intention`.
    printf '{"t":%s,"self":"self:%s","provenance":"porte-catalogue","intention":{"genre":"note","texte":"souvenir de %s"}}\n' \
        "$(date +%s)000" "$nom" "$nom" >> "$j"
    ok "une entrée de mémoire écrite pour « $nom »"
done

# ── LE CATALOGUE ────────────────────────────────────────────────────────────
# Il est gardé : on l'interroge avec le cookie d'un compte (identité valide).
curl -s -m 20 -c "$CK_A" -o /dev/null -X POST "$BASE_URL/api/auth/login" \
    --data-urlencode "username=$ALICE" --data-urlencode "password=motdepasse-$ALICE"
CAT=$(curl -s -m 20 -b "$CK_A" "$BASE_URL/api/engine/selfs")

titre "4) le catalogue montre les deux selfs, nommés"
printf '%s' "$CAT" | grep -q "self:$ALICE" && ok "« self:$ALICE » présent" \
    || ko "« self:$ALICE » ABSENT : $(printf '%s' "$CAT" | head -c 200)"
printf '%s' "$CAT" | grep -q "self:$BOB" && ok "« self:$BOB » présent" \
    || ko "« self:$BOB » ABSENT : $(printf '%s' "$CAT" | head -c 200)"
nombre=$(printf '%s' "$CAT" | sed -n 's/.*"nombre"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p' | head -1)
[ "${nombre:-0}" -ge 2 ] && ok "le catalogue annonce $nombre self(s)" \
    || ko "nombre de selfs inattendu : ${nombre:-absent}"

titre "5) il a LECTURE de leur mémoire (journal et nombre d'entrées)"
jrn=$(printf '%s' "$CAT" | sed -n 's/.*"journal"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)
[ -n "$jrn" ] && ok "un journal est annoncé : $(basename "$jrn")" \
    || ko "aucun journal annoncé (le catalogue ne lit pas la mémoire)"
printf '%s' "$CAT" | grep -q '"entrees":[[:space:]]*1' \
    && ok "une entrée lue dans un journal (donc le contenu est lu, pas deviné)" \
    || printf '  (info) entrées annoncées : %s\n' "$(printf '%s' "$CAT" | grep -o '"entrees":[0-9]*' | head -3 | tr '\n' ' ')"

titre "6) le monde apporté est attribué à son auteur"
# On ajoute un monde par le compte d'alice (le geste de la demande), puis on
# relit le catalogue : le monde doit apparaître DU CÔTÉ D'ALICE.
LANC=$(curl -s -m 30 -b "$CK_A" -X POST "$BASE_URL/api/worlds" \
    -H 'Content-Type: application/json' -d '{"url":"https://github.com/sindresorhus/is-npm"}')
JOB=$(printf '%s' "$LANC" | sed -n 's/.*"job"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
if [ -n "$JOB" ]; then
    fin=$(( $(date +%s) + 180 ))
    while [ "$(date +%s)" -lt "$fin" ]; do
        e=$(curl -s -m 20 -b "$CK_A" "$BASE_URL/api/worlds/status?job=$JOB" \
            | sed -n 's/.*"etat"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
        [ "$e" = "fini" ] || [ "$e" = "erreur" ] && break
        sleep 3
    done
    [ "$e" = "fini" ] && ok "monde ajouté par alice (état « $e »)" \
        || ko "ajout de monde en état « $e »"
else
    ko "ajout refusé : $(printf '%s' "$LANC" | head -c 180)"
fi

CAT2=$(curl -s -m 20 -b "$CK_A" "$BASE_URL/api/engine/selfs")
# Le monde doit être listé DANS la fiche d'alice. On isole la fiche pour ne pas
# valider la présence du monde « quelque part » dans le JSON.
fiche_a=$(printf '%s' "$CAT2" | tr '}' '\n' | grep -m1 "self:$ALICE")
printf '%s' "$fiche_a" | grep -qi "is-npm" \
    && ok "le monde « is-npm » est attribué à self:$ALICE" \
    || ko "le monde n'est pas attribué à alice : $(printf '%s' "$fiche_a" | head -c 220)"

titre "7) la route ne relie rien et ne ment pas"
avant=$(printf '%s' "$CAT2" | sed -n 's/.*"entrees_total"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p' | head -1)
curl -s -m 20 -b "$CK_A" "$BASE_URL/api/engine/selfs" >/dev/null
apres=$(curl -s -m 20 -b "$CK_A" "$BASE_URL/api/engine/selfs" \
    | sed -n 's/.*"entrees_total"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p' | head -1)
[ "$avant" = "$apres" ] && ok "appeler le catalogue n'écrit RIEN ($avant entrées)" \
    || ko "le total d'entrées a bougé : $avant → $apres"

code=$(curl -s -m 20 -o /dev/null -w '%{http_code}' "$BASE_URL/api/engine/selfs")
[ "$code" = "401" ] && ok "sans identité → 401 (on ne lit pas la population sans entrer)" \
    || ko "sans identité → $code (401 attendu)"
illisible=$(printf '%s' "$CAT2" | sed -n 's/.*"synapse_illisible"[[:space:]]*:[[:space:]]*\(true\|false\).*/\1/p' | head -1)
[ -n "$illisible" ] && ok "la lisibilité de la synaptique est DITE (synapse_illisible=$illisible)" \
    || ko "la route ne dit pas si la synaptique a été lue (lacune = fait)"

titre "verdict"
if [ "$echecs" = "0" ]; then
    printf '  ✅ qui existe est lisible, sans rien relier ni rien écrire\n'
    exit 0
fi
printf '  ❌ %d contrôle(s) en échec\n' "$echecs"
[ "${GARDER_JOURNAL:-0}" = "1" ] && printf '  journal du serveur : %s\n' "$LOG"
exit 1
