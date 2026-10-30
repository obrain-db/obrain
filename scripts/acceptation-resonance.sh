#!/usr/bin/env bash
# ── PORTE D'ACCEPTATION : LA RÉSONANCE ──────────────────────────────────────
#
# « Interconnecter chacun sa base » — la porte vérifie que l'interconnexion est
# un GESTE, qu'elle est RÉVOCABLE, et surtout qu'elle ne fait pas fuir la
# mémoire privée. Le contrat, dans l'ordre où il compte :
#
#   1. aucun partage par défaut : un lecteur qui n'a pas de lien ne voit RIEN ;
#   2. le geste CLI (`obrain resonance --lier`) ouvre un lien, et la route HTTP
#      le voit tout de suite — sans redémarrer le service ;
#   3. ce qui devient lisible est NOMMÉ sur les deux faces (le publieur voit qui
#      le lit, le lecteur voit à qui il est abonné) ;
#   4. LE TEXTE PRIVÉ NE TRAVERSE JAMAIS : un secret écrit dans une `note` et un
#      `etat-self` du publieur est cherché dans TOUTE la réponse HTTP, et ne doit
#      apparaître nulle part ;
#   5. `--delier` referme pour de bon : le lecteur ne voit plus rien ;
#   6. un self ne peut pas s'abonner à lui-même (cela n'apprend rien et
#      masquerait une faute de frappe dans un nom).
#
# Usage : scripts/acceptation-resonance.sh [port]
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 1

PORT="${1:-8445}"
if [ "$PORT" = "8432" ] || [ "$PORT" = "8433" ]; then
    echo "refus : $PORT est un service de l'utilisateur. Choisis un port libre."
    exit 2
fi

BIN="${OBRAIN_BIN:-./target/release/obrain}"
TAG="$$"
STORE="/tmp/acceptation-res-$TAG.obrain"
IAM="/tmp/acceptation-res-iam-$TAG.obrain"
CACHE="/tmp/acceptation-res-cache-$TAG"
CK="/tmp/acceptation-res-$TAG.cookies"
LOG="/tmp/acceptation-res-$TAG.log"
ESPACE="acceptation-res-$TAG"
LECTEUR="lecteur-$TAG"
PUBLIEUR="publieur-$TAG"
SECRET="secret-prive-${TAG}-ne-doit-jamais-sortir"
MDP_ADMIN="admin-$TAG-$RANDOM"
MDP_READ="read-$TAG-$RANDOM"
BASE_URL="http://127.0.0.1:$PORT"
SERVEUR=""

echecs=0
ok()   { printf '  ✅ %s\n' "$1"; }
ko()   { printf '  ❌ %s\n' "$1"; echecs=$((echecs + 1)); }
titre(){ printf '\n%s\n' "$1"; }

nettoyer() {
    [ -n "$SERVEUR" ] && kill -9 "$SERVEUR" 2>/dev/null
    rm -f "$CK"
    [ "${GARDER_JOURNAL:-0}" = "1" ] || rm -f "$LOG"
    [ "${GARDER_MONDE:-0}" = "1" ] && return
    rm -rf "$STORE" "$IAM" "$CACHE"
}
trap nettoyer EXIT

titre "1) binaire, base, identités"
[ -x "$BIN" ] || { echo "  ❌ binaire absent"; exit 2; }
"$BIN" init "$STORE" >/dev/null 2>&1 || { echo "  ❌ init base"; exit 2; }
"$BIN" init "$IAM" >/dev/null 2>&1 || { echo "  ❌ init identités"; exit 2; }
"$BIN" iam amorcer "$IAM" "$ESPACE" --utilisateur admin --mot-de-passe "$MDP_ADMIN" >/dev/null 2>&1 \
    || { echo "  ❌ amorçage (binaire sans IAM ?)"; exit 2; }
ok "prêt"

titre "2) serveur armé, cache de résonance isolé (port $PORT)"
# Le cache est ISOLÉ : la résonance lit les journaux de mémoire réels, et lire
# ceux de l'utilisateur dans un test serait une indiscrétion.
OBRAIN_IAM=1 OBRAIN_IAM_ACCOUNT=obrain OBRAIN_IAM_STORE="$IAM" \
OBRAIN_VIZ_CACHE_DIR="$CACHE" \
"$BIN" viz serve --host 127.0.0.1 --port "$PORT" \
    --store "$STORE" --enrich-ricci 0 >"$LOG" 2>&1 &
SERVEUR=$!
for _ in $(seq 1 60); do
    c=$(curl -s -m 2 -o /dev/null -w '%{http_code}' "$BASE_URL/api/stores" 2>/dev/null)
    [ "$c" != "000" ] && break
    sleep 1
done
[ "$c" != "000" ] && ok "le serveur écoute" || { ko "le serveur n'écoute pas (voir $LOG)"; exit 2; }

titre "3) deux comptes, et le journal du publieur"
CK_ADMIN="/tmp/acceptation-res-admin-$TAG.cookies"
curl -s -m 20 -b "$CK_ADMIN" -c "$CK_ADMIN" -o /dev/null -X POST "$BASE_URL/api/auth/login" \
    --data-urlencode "username=admin" --data-urlencode "password=$MDP_ADMIN"
for nom in "$LECTEUR" "$PUBLIEUR"; do
    r=$(curl -s -m 20 -b "$CK_ADMIN" -c "$CK_ADMIN" -X POST "$BASE_URL/api/iam/utilisateurs" \
        -d "nom=$nom" -d "espace=$ESPACE" -d "mot_de_passe=motdepasse-$nom")
    printf '%s' "$r" | grep -q "$nom" && ok "compte « $nom » créé" \
        || ko "création de « $nom » : $(printf '%s' "$r" | head -c 140)"
done
rm -f "$CK_ADMIN"

# Le chemin du journal se DEMANDE (il est haché) : on interroge la route mémoire
# du publieur, puis on écrit dans le fichier qu'elle désigne.
jrn_pour() { # $1 = nom
    local ck="/tmp/acceptation-res-$1.cookies"
    curl -s -m 20 -c "$ck" -o /dev/null -X POST "$BASE_URL/api/auth/login" \
        --data-urlencode "username=$1" --data-urlencode "password=motdepasse-$1"
    curl -s -m 20 -b "$ck" "$BASE_URL/api/engine/memoire" \
        | sed -n 's/.*"journal"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1
}
JP=$(jrn_pour "$PUBLIEUR")
JL=$(jrn_pour "$LECTEUR")
if [ -n "$JP" ] && [ -n "$JL" ]; then
    ok "journaux désignés par le serveur"
else
    ko "le serveur ne désigne pas les journaux"; exit 2
fi
mkdir -p "$(dirname "$JP")"
# Le journal du publieur contient les QUATRE genres : deux privés (avec le
# secret), deux publics (des noms).
cat > "$JP" <<JSONL
{"t":10,"self":"self:$PUBLIEUR","provenance":"porte","intention":{"kind":"note","texte":"$SECRET","sujet":"assurance"}}
{"t":20,"self":"self:$PUBLIEUR","provenance":"porte","intention":{"kind":"etat-self","epithet":"v1","objectif":"$SECRET","prochaine_action":"$SECRET","note":"$SECRET"}}
{"t":30,"self":"self:$PUBLIEUR","provenance":"porte","intention":{"kind":"objectif","nom":"publicer-l-atlas","texte":"$SECRET"}}
{"t":40,"self":"self:$PUBLIEUR","provenance":"porte","intention":{"kind":"procedure","nom":"compter-par-label","description":"$SECRET","corps":"$SECRET"}}
JSONL
ok "journal du publieur écrit (2 genres privés, 2 publics)"

# ── LA ROUTE, VUE PAR LE LECTEUR ────────────────────────────────────────────
res_de() { # $1 = nom → le corps JSON de /api/engine/resonance
    local ck="/tmp/acceptation-res-$1.cookies"
    curl -s -m 20 -b "$ck" "$BASE_URL/api/engine/resonance"
}

# ── DEUX LISTES, ET C'EST LA DISTINCTION QUI COMPTE ─────────────────────────
# Erreur de ma première version, corrigée ici : je cherchais le nom du publieur
# dans TOUTE la réponse. Or la route distingue délibérément deux choses :
#   · `candidats` — QUI EXISTE (nom, volume, dernière activité) : on ne peut pas
#     proposer de relier deux selfs sans montrer lesquels existent, et un nom de
#     self n'est pas une mémoire ;
#   · `je_vois` — CE QUE JE LIS, donc les facettes : vide tant qu'aucun lien
#     n'est posé, et c'est ÇA le contrat « aucun partage par défaut ».
# Le contrôle juste porte donc sur `je_vois`, pas sur la présence d'un nom.
voit() { # $1 = corps JSON → la partie `je_vois`
    printf '%s' "$1" | sed -n 's/.*"je_vois":\[\([^]]*\)\].*/\1/p'
}

titre "4) SANS LIEN, le lecteur ne VOIT rien (aucun partage par défaut)"
AVANT=$(res_de "$LECTEUR")
[ -z "$(voit "$AVANT")" ] \
    && ok "« je_vois » est VIDE tant qu'aucun lien n'est posé" \
    || ko "des facettes sont lisibles SANS lien : $(printf '%s' "$(voit "$AVANT")" | head -c 200)"
printf '%s' "$AVANT" | grep -q "self:$PUBLIEUR" \
    && ok "le publieur est proposé comme CANDIDAT (le découvrir n'est pas le lire)" \
    || printf '  (info) le publieur n'"'"'est pas proposé comme candidat\n'
if printf '%s' "$AVANT" | grep -q "publicer-l-atlas"; then
    ko "un NOM PUBLIC du publieur fuit alors qu'aucun lien n'existe"
else
    ok "aucun nom public du publieur ne fuit sans lien"
fi

titre "5) le GESTE : le CLI ouvre le lien, le service le voit à chaud"
GESTE=$(OBRAIN_VIZ_CACHE_DIR="$CACHE" "$BIN" resonance --publieur "self:$PUBLIEUR" --lecteur "self:$LECTEUR" 2>&1)
printf '%s' "$GESTE" | grep -qi "résonance" && ok "le geste s'annonce (il dit ce qui devient lisible)" \
    || ko "le geste ne dit rien : $(printf '%s' "$GESTE" | head -c 160)"

APRES=$(res_de "$LECTEUR")
VU=$(voit "$APRES")
[ -n "$VU" ] && ok "« je_vois » est désormais rempli (sans redémarrage du service)" \
    || ko "le lien n'est pas vu par le service : $(printf '%s' "$APRES" | head -c 240)"
printf '%s' "$(res_de "$PUBLIEUR")" | grep -q "$LECTEUR" \
    && ok "le publieur VOIT QUI LE LIT (l'abonné est nommé chez lui)" \
    || ko "le publieur ne voit pas qui le lit"

titre "6) LE CONTRAT : le texte privé ne traverse jamais"
if printf '%s' "$APRES" | grep -q "$SECRET"; then
    ko "LE SECRET A TRAVERSÉ la réponse HTTP — c'est la seule faute qui compte"
else
    ok "le secret (note + etat-self) n'apparaît NULLE PART dans la réponse"
fi
printf '%s' "$APRES" | grep -q "publicer-l-atlas" \
    && ok "le nom d'objectif public, lui, EST rendu" \
    || ko "le nom d'objectif n'est pas rendu : le partage ne sert à rien"
printf '%s' "$APRES" | grep -q "compter-par-label" \
    && ok "le nom de procédure est rendu" \
    || ko "le nom de procédure n'est pas rendu"
printf '%s' "$APRES" | grep -qiE '"exclu"|prive_exclu' \
    && ok "les genres exclus sont DITS (un partage partiel qui se tairait mentirait)" \
    || printf '  (info) les genres exclus ne sont pas annoncés\n'

titre "7) le lien est RÉVOCABLE, et le sens compte"
# Le MÊME cache que le serveur : sinon le CLI écrirait dans les liens RÉELS de
# l'utilisateur — un test ne doit pas toucher à ce qui existe hors de lui.
OBRAIN_VIZ_CACHE_DIR="$CACHE" "$BIN" resonance \
    --publieur "self:$PUBLIEUR" --lecteur "self:$LECTEUR" --delier >/dev/null 2>&1
FERME=$(res_de "$LECTEUR")
[ -z "$(voit "$FERME")" ] \
    && ok "après « --delier », « je_vois » est de nouveau VIDE" \
    || ko "le lien refermé laisse encore lire : $(printf '%s' "$(voit "$FERME")" | head -c 200)"
if printf '%s' "$FERME" | grep -q "publicer-l-atlas"; then
    ko "un nom public reste lisible après révocation"
else
    ok "plus aucun nom public après révocation"
fi

# Se lier à soi-même : refusé.
if OBRAIN_VIZ_CACHE_DIR="$CACHE" "$BIN" resonance \
    --publieur "self:$PUBLIEUR" --lecteur "self:$PUBLIEUR" >/dev/null 2>&1; then
    ko "un self a pu s'abonner à lui-même"
else
    ok "un self ne peut pas s'abonner à lui-même (refusé)"
fi

titre "8) le consentement n'est pas publié en clair"
# Le cache tourne sous `umask 0002` : un `fs::write` banal créerait ces fichiers
# en 664, donc lisibles par tout le monde — or ils disent QUI s'intéresse à QUEL
# self. On mesure les droits RÉELLEMENT posés.
droits=$(stat -c '%a' "$CACHE/resonance" 2>/dev/null)
[ "$droits" = "700" ] && ok "répertoire des liens en 0700" \
    || ko "répertoire des liens en « ${droits:-absent} » (0700 attendu)"
f=$(ls "$CACHE/resonance"/resonance-*.json 2>/dev/null | head -1)
if [ -n "$f" ]; then
    df=$(stat -c '%a' "$f")
    [ "$df" = "600" ] && ok "fichier de consentement en 0600" \
        || ko "fichier de consentement en « $df » (0600 attendu) : lisible par le groupe"
else
    printf '  (info) aucun fichier de consentement à mesurer (tous les liens refermés)\n'
fi

titre "verdict"
if [ "$echecs" = "0" ]; then
    printf '  ✅ le partage est un geste, il se referme, et le privé ne sort pas\n'
    exit 0
fi
printf '  ❌ %d contrôle(s) en échec\n' "$echecs"
[ "${GARDER_JOURNAL:-0}" = "1" ] && printf '  journal du serveur : %s\n' "$LOG"
exit 1
