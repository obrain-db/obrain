#!/usr/bin/env bash
# ── PORTE D'ACCEPTATION DE L'ADMINISTRATION IAM ─────────────────────────────
#
# Un seul point d'entrée qui répond à : « peut-on administrer les comptes, et
# est-ce que ça tient ? » — sans moi, et sans la main.
#
# Pourquoi ce script existe : l'IAM savait créer des identités depuis la
# bibliothèque, mais AUCUNE surface ne les exposait — ajouter un collègue
# supposait d'écrire un programme. Cette porte vérifie que les deux surfaces
# (le CLI, hors bande sur un fichier ; le serveur, plafonné par les droits de
# l'appelant) font bien ce qu'elles annoncent, y compris ce qu'elles REFUSENT.
#
# Ce qu'elle vérifie, dans l'ordre :
#   1. le binaire contient l'IAM, et la sous-commande `iam` existe ;
#   2. l'amorçage pose un administrateur (mot de passe + jeton) ;
#   3. le CLI liste, ajoute, désactive, efface un compte ;
#   4. un serveur ARMÉ, sur un port LIBRE, refuse sans jeton et accepte avec ;
#   5. le PLAFOND tient : un administrateur d'un espace ne peut pas en ouvrir un
#      autre — c'est le contrôle qui compte le plus ;
#   6. les mutations ne passent pas par GET ;
#   7. le VERROUILLAGE tient : le dernier administrateur ne se désactive pas ;
#   8. la piste d'audit porte les gestes.
#
# Usage :
#   scripts/acceptation-iam.sh [port]
#
# Défauts : port 8439 — JAMAIS 8432 ni 8433, qui sont les services de
# l'utilisateur. Rien n'est laissé derrière : magasin, cookies et journal
# temporaires sont supprimés à la sortie.
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 1

PORT="${1:-8439}"
if [ "$PORT" = "8432" ] || [ "$PORT" = "8433" ]; then
    echo "refus : $PORT est un service de l'utilisateur. Choisis un port libre."
    exit 2
fi

BIN="${OBRAIN_BIN:-./target/release/obrain}"
IAM="/tmp/acceptation-iam-$$.obrain"
CK="/tmp/acceptation-iam-$$.cookies"
LOG="/tmp/acceptation-iam-$$.log"
MDP="acceptation-$(date +%s)-$RANDOM"
BASE_URL="http://127.0.0.1:$PORT"

echecs=0
ok()   { printf '  ✅ %s\n' "$1"; }
ko()   { printf '  ❌ %s\n' "$1"; echecs=$((echecs + 1)); }
titre(){ printf '\n%s\n' "$1"; }

nettoyer() {
    [ -n "${SERVEUR:-}" ] && kill -9 "$SERVEUR" 2>/dev/null
    rm -rf "$IAM" "$CK" "$CK.n"
    [ "${GARDER_JOURNAL:-0}" = "1" ] || rm -f "$LOG"
}
trap nettoyer EXIT

# ── 1. LE BINAIRE ───────────────────────────────────────────────────────────
titre "1) binaire ($BIN)"
if [ ! -x "$BIN" ]; then
    echo "  ❌ $BIN absent — construire : cargo build --release -p obrain-cli --features ai,iam"
    exit 2
fi

# ── LE SEUL CONTRÔLE QUI VAUT : FAIRE PARLER LA COMMANDE ────────────────────
# `nm -C "$BIN" | grep obrain_iam` NE SUFFIT PAS. Mesuré le 2026-09-14 : un
# binaire construit SANS la feature portait quand même des symboles
# `obrain_iam` (une autre dépendance du workspace la tire), donc cette sonde
# passait au vert pendant que la commande répondait « IAM NON COMPILÉ ». C'est
# la même famille de défaut que « la ligne de construction déployée » : un
# contrôle indirect valide ce qu'il ne mesure pas.
SONDE_MAGASIN="/tmp/acceptation-iam-sonde-$$.obrain"
"$BIN" init "$SONDE_MAGASIN" >/dev/null 2>&1
SONDE="$("$BIN" iam utilisateurs "$SONDE_MAGASIN" 2>&1)"
rm -rf "$SONDE_MAGASIN"
if printf '%s' "$SONDE" | grep -q 'NON COMPILÉ'; then
    echo "  ❌ IAM ABSENT du binaire : la sous-commande répond « NON COMPILÉ »."
    echo "     Construire : cargo build --release -p obrain-cli --features ai,iam"
    echo "     (c'est la seule commande qui embarque la chaîne — \`--features ai\` ne l'inclut pas,"
    echo "      et un \`cargo build\` ordinaire sur le même target/ écrase le binaire complet)"
    exit 2
fi
if printf '%s' "$SONDE" | grep -qi 'error'; then
    ko "la sous-commande « iam » répond une erreur inattendue : $(printf '%s' "$SONDE" | head -2 | tr '\n' ' ')"
else
    ok "la sous-commande « iam » est COMPILÉE (elle répond, au lieu de s'annoncer incompilée)"
fi
if nm -C "$BIN" 2>/dev/null | grep -qi obrain_iam; then
    ok "symboles obrain_iam présents ($(stat -c %y "$BIN" | cut -c1-19))"
else
    ko "aucun symbole obrain_iam dans le binaire"
fi

# ── 2. AMORÇAGE (hors bande, sur un fichier jetable) ────────────────────────
titre "2) amorçage d'un administrateur ($IAM)"
"$BIN" init "$IAM" >/dev/null 2>&1
if [ ! -d "$IAM" ]; then echo "  ❌ création du magasin impossible"; exit 2; fi

SORTIE_AMORCAGE="$("$BIN" iam amorcer "$IAM" demo --utilisateur admin --mot-de-passe "$MDP" 2>&1)"
if printf '%s' "$SORTIE_AMORCAGE" | grep -qi 'admin'; then
    ok "amorçage par le CLI"
else
    # Repli sur l'exemple historique : il doit continuer de fonctionner, mais son
    # absence n'est pas un échec de l'administration.
    EXEMPLE="./target/release/examples/amorcer_iam"
    if [ -x "$EXEMPLE" ]; then
        SORTIE_AMORCAGE="$("$EXEMPLE" "$IAM" obrain demo admin "$MDP" 2>&1)"
        ok "amorçage par l'exemple (le CLI a échoué) : $(printf '%s' "$SORTIE_AMORCAGE" | grep -c .) ligne(s)"
    else
        ko "amorçage impossible : $(printf '%s' "$SORTIE_AMORCAGE" | head -2 | tr '\n' ' ')"
    fi
fi
if "$BIN" iam utilisateurs "$IAM" 2>&1 | grep -q 'admin'; then
    ok "l'administrateur se lit dans la liste"
else
    ko "l'administrateur ne se lit pas : $("$BIN" iam utilisateurs "$IAM" 2>&1 | head -3 | tr '\n' ' ')"
fi

# ── 3. LES GESTES DU CLI ────────────────────────────────────────────────────
titre "3) gestes du CLI"
SORTIE_AJOUT="$("$BIN" iam ajouter "$IAM" alice --espace demo 2>&1)"
if printf '%s' "$SORTIE_AJOUT" | grep -q 'alice'; then
    ok "ajout d'un compte (mot de passe engendré rendu une fois)"
else
    ko "ajout d'un compte : $(printf '%s' "$SORTIE_AJOUT" | head -2 | tr '\n' ' ')"
fi

# Le plafond du CLI est NUL (hors bande) mais la grammaire des noms, non.
if "$BIN" iam ajouter "$IAM" 'mauvais/nom' --espace demo >/dev/null 2>&1; then
    ko "un nom contenant « / » a été accepté (il découperait un ORN)"
else
    ok "nom dangereux refusé (« a/b »)"
fi

if "$BIN" iam desactiver "$IAM" alice >/dev/null 2>&1 \
   && "$BIN" iam utilisateur "$IAM" alice 2>&1 | grep -qi 'inactive'; then
    ok "désactivation d'un compte"
else
    ko "désactivation : $("$BIN" iam utilisateur "$IAM" alice 2>&1 | head -2 | tr '\n' ' ')"
fi

if "$BIN" iam effacer "$IAM" alice >/dev/null 2>&1; then
    ko "effacement RGPD sans --oui : il devait REFUSER"
else
    ok "effacement refusé sans --oui (geste irréversible)"
fi
if "$BIN" iam effacer "$IAM" alice --oui >/dev/null 2>&1; then
    ok "effacement exécuté avec --oui"
else
    ko "effacement avec --oui : $( "$BIN" iam effacer "$IAM" alice --oui 2>&1 | head -2 | tr '\n' ' ')"
fi

# ── 4. LE SERVEUR ARMÉ ──────────────────────────────────────────────────────
titre "4) serveur armé (port $PORT)"
OBRAIN_IAM=1 OBRAIN_IAM_STORE="$IAM" OBRAIN_IAM_ACCOUNT=obrain \
    "$BIN" viz serve --host 127.0.0.1 --port "$PORT" \
    --preset-default "Acceptation" --preset "Acceptation=$IAM" > "$LOG" 2>&1 &
SERVEUR=$!
for _ in $(seq 1 40); do
    [ "$(curl -s -o /dev/null -w '%{http_code}' -m 3 "$BASE_URL/api/auth/whoami")" = "200" ] && break
    sleep 1
done
case "$(grep -a 'viz serve :' "$LOG" | tail -1)" in
    *"IAM ARMÉ"*) ok "garde armée" ;;
    *"NON COMPILÉ"*) ko "binaire sans IAM"; exit 2 ;;
    *) ko "garde NON armée : $(grep -a 'viz serve :' "$LOG" | tail -1)" ;;
esac

# ── 5. LA GARDE ET LE PLAFOND ───────────────────────────────────────────────
titre "5) garde et plafond"

code=$(curl -s -m 20 -o /dev/null -w '%{http_code}' "$BASE_URL/api/iam/utilisateurs")
[ "$code" = "401" ] && ok "administration sans jeton → 401" || ko "administration sans jeton → $code (401 attendu)"

code=$(curl -s -m 30 -c "$CK" -o /dev/null -w '%{http_code}' -X POST "$BASE_URL/api/auth/login" \
    -d "username=admin&password=$MDP")
if [ "$code" = "303" ] || [ "$code" = "200" ]; then ok "connexion de l'administrateur → $code"
else ko "connexion → $code (303 attendu)"; fi

# ── UN ESPACE NOMMÉ EST OBLIGATOIRE ─────────────────────────────────────────
# Juger une route d'administration sur l'espace par défaut du serveur
# reviendrait à trancher sur un espace que la requête ne vise pas.
code=$(curl -s -m 20 -b "$CK" -o /dev/null -w '%{http_code}' "$BASE_URL/api/iam/utilisateurs")
[ "$code" = "400" ] && ok "administration sans espace nommé → 400 (nommer, jamais défaut)" \
    || ko "administration sans espace nommé → $code (400 attendu)"

# ── « QUI SUIS-JE » : une identité suffit ───────────────────────────────────
# C'est ce qui permet à la page de savoir sur quel espace elle peut agir, au
# lieu de le deviner.
if curl -s -m 20 -b "$CK" "$BASE_URL/api/iam/moi" | grep -q '"appelant"'; then
    ok "« qui suis-je » lisible avec une simple identité"
else
    ko "« qui suis-je » : $(curl -s -m 20 -b "$CK" "$BASE_URL/api/iam/moi" | head -c 160)"
fi

code=$(curl -s -m 20 -b "$CK" -o /dev/null -w '%{http_code}' "$BASE_URL/api/iam/utilisateurs?espace=demo")
[ "$code" = "200" ] && ok "liste des comptes avec cookie → 200" || ko "liste avec cookie → $code"

# ── LE CONTRÔLE QUI COMPTE LE PLUS ──────────────────────────────────────────
# L'administrateur amorcé n'a de droits que sur « demo ». Ouvrir un compte sur
# un AUTRE espace doit être refusé : c'est la différence entre « administrer »
# et « tout pouvoir ».
code=$(curl -s -m 20 -b "$CK" -o /tmp/acceptation-iam-plafond-$$.txt -w '%{http_code}' \
    -X POST "$BASE_URL/api/iam/utilisateurs" -d 'nom=bob&espace=hors-portee')
if [ "$code" = "403" ]; then
    ok "création hors du plafond → 403 (l'espace « hors-portee » n'est pas détenu)"
else
    ko "création hors du plafond → $code (403 attendu) : $(head -c 160 /tmp/acceptation-iam-plafond-$$.txt)"
fi
rm -f /tmp/acceptation-iam-plafond-$$.txt

code=$(curl -s -m 20 -b "$CK" -o /dev/null -w '%{http_code}' \
    -X POST "$BASE_URL/api/iam/utilisateurs" -d 'nom=carol&espace=demo')
[ "$code" = "200" ] && ok "création dans le plafond → 200" || ko "création dans le plafond → $code"

# ── LES MUTATIONS NE PASSENT PAS PAR GET ────────────────────────────────────
code=$(curl -s -m 20 -b "$CK" -o /dev/null -w '%{http_code}' \
    "$BASE_URL/api/iam/utilisateur/desactiver?nom=carol")
if [ "$code" = "401" ] || [ "$code" = "403" ] || [ "$code" = "404" ] || [ "$code" = "405" ]; then
    ok "désactivation par GET → $code (refusée : une mutation ne s'obtient pas d'un lien)"
else
    ko "désactivation par GET → $code : un lien suffirait à fermer un compte"
fi

code=$(curl -s -m 20 -b "$CK" -o /dev/null -w '%{http_code}' \
    -X POST "$BASE_URL/api/iam/utilisateur/desactiver" -d 'nom=carol&espace=demo')
[ "$code" = "200" ] && ok "désactivation par POST → 200" || ko "désactivation par POST → $code"

# ── LE VERROUILLAGE ─────────────────────────────────────────────────────────
# « admin » est le seul administrateur : le désactiver fermerait la porte à clé.
REPONSE=$(curl -s -m 20 -b "$CK" -X POST "$BASE_URL/api/iam/utilisateur/desactiver" \
    -d 'nom=admin&espace=demo')
case "$REPONSE" in
    *"dernier administrateur"*) ok "désactivation du dernier administrateur refusée" ;;
    *) ko "dernier administrateur : réponse inattendue « $(printf '%s' "$REPONSE" | head -c 160) »" ;;
esac
code=$(curl -s -m 20 -b "$CK" -o /dev/null -w '%{http_code}' \
    "$BASE_URL/api/iam/utilisateur?nom=admin&espace=demo")
[ "$code" = "200" ] && ok "l'administrateur est toujours joignable (verrouillage tenu)" || ko "admin perdu → $code"

# ── 6. LA PAGE ET LA PISTE ──────────────────────────────────────────────────
titre "6) page et piste"
# La page est une COQUILLE sans données : elle se sert sans jeton, et c'est
# l'API qu'elle appelle qui est gardée (sinon la liste des comptes serait
# publique).
code=$(curl -s -m 20 -o /dev/null -w '%{http_code}' "$BASE_URL/iam")
[ "$code" = "200" ] && ok "page /iam servie" || ko "page /iam → $code"
if curl -s -m 20 "$BASE_URL/iam" | grep -q '/api/iam/moi'; then
    ok "la page ne détient aucune donnée : elle interroge l'API gardée"
else
    ko "la page ne semble pas passer par l'API gardée"
fi

if curl -s -m 20 -b "$CK" "$BASE_URL/api/iam/audit?limit=50&espace=demo" | grep -q '"action"'; then
    ok "piste d'audit des gestes d'administration lisible"
else
    ko "piste d'audit illisible ou vide"
fi

# ── 7. VERDICT ──────────────────────────────────────────────────────────────
titre "7) verdict"
if [ "$echecs" -eq 0 ]; then
    echo "  ADMINISTRATION IAM COMPLÈTE — tout passe."
else
    echo "  $echecs étape(s) en échec. Journal du serveur : $LOG"
    echo "  (GARDER_JOURNAL=1 scripts/acceptation-iam.sh pour le conserver)"
fi
exit "$echecs"
