#!/usr/bin/env bash
# ── PORTE D'ACCEPTATION : LA FLOTTE ─────────────────────────────────────────
# « Quelles machines connais-tu, et que sait faire chacune ? »
#
# docs/MACHINES-CONNUES-2026-09-15.md §5 réclame ce test et le nomme :
#
#   « Une seule question au serveur global : “quelles machines connais-tu, et que
#     sait faire chacune ?”. Aujourd'hui cette question n'a aucune réponse : elle
#     échoue par le vide, pas par l'erreur. »
#
# Cette porte POSE la question, pour de vrai, sur un dispositif ISOLÉ (stores dans
# /tmp, cache dans /tmp, IAM temporaire, port libre) : un moteur armé, deux
# comptes, et trois nœuds `selfd` aux noms ET aux capacités DIFFÉRENTES — c'est ce
# qui rend la question intéressante. Elle vérifie, dans cet ordre :
#
#   1. le dispositif (deux comptes, trois machines aux capacités distinctes) ;
#   2. que ce que CHAQUE machine annonce savoir faire est VRAI — mesuré par un
#      appel réel, pas cru sur parole (une capacité annoncée et refusée à
#      l'exécution est exactement ce que la doctrine « annoncer est décider »
#      interdit) ;
#   3. ce que le serveur dit des machines D'UN compte (`/api/remote/clients`,
#      la seule route qui existe aujourd'hui) ;
#   4. LA QUESTION, POSÉE À CHAQUE COMPTE PROPRIÉTAIRE : elle doit NOMMER ses
#      machines, donner les capacités de CHACUNE (distinctes), et n'en montrer
#      AUCUNE d'un autre compte — le cloisonnement est mesuré ici aussi, et pas
#      seulement en 6 ;
#   5. qu'une réponse VIDE n'a pas la forme d'une bonne réponse (le mode de
#      défaillance nommé en §2 du document : « un ok sur un vide intégral ») ;
#   6. le cloisonnement (le compte A ne voit pas la machine du compte B, et
#      connaître un identifiant de client n'ouvre aucun droit) ;
#   7. qu'une machine CONNUE mais ABSENTE reste nommée après un redémarrage du
#      moteur — « connaître » une machine n'est pas « la voir connectée » ;
#   8. que la flotte est un FAIT DU GRAPHE (§3 et §4.1 du document : la machine
#      comme nœud de premier ordre).
#
# La porte MESURE LE BINAIRE DÉPLOYÉ (`target/release/obrain`,
# `target/release/selfd`), pas la source du dépôt : c'est la ligne servie qui
# compte, et une correction non construite n'est pas une correction déployée.
# `OBRAIN_BIN` / `SELFD_BIN` permettent de mesurer une autre construction.
#
# ── CE QU'ELLE NE FAIT PAS, ET POURQUOI ─────────────────────────────────────
# Elle ne déploie RIEN sur une autre machine, elle ne touche AUCUN service de
# l'utilisateur (les ports 8432/8433/8455/8090/8144/3082/3083 sont refusés
# explicitement, et tout port ≤ 8400 avec eux), elle n'écrit dans AUCUNE base
# vivante : tout vit sous /tmp/acceptation-flotte-<pid> et disparaît au trap EXIT,
# y compris en cas d'échec.
#
# ── L'INCIDENT QUI A FIXÉ UNE ÉTAPE ─────────────────────────────────────────
# L'administration hors bande de l'IAM doit PRÉCÉDER le démarrage du moteur.
# Constaté en écrivant cette porte : `obrain iam jeton` sur le magasin d'identités
# pendant que `viz serve` tourne échoue sur le verrou exclusif —
# « substrate … is already open by another process (exclusive lock … refused) ».
# Les comptes et leurs jetons sont donc posés AVANT que le moteur n'ouvre le
# magasin ; un test qui les créerait après mesurerait cet accident d'ordonnancement
# au lieu de la flotte.
#
# Usage : scripts/acceptation-flotte.sh [port]
#   ROUTE_FLOTTE=/api/...   nomme LA route à interroger pour la question globale
#                           (défaut : la liste des candidates, dans l'ordre)
#   GARDER_TRACE=1          conserve stores et journaux après la porte
#   SELF_IDLE=1             passe les nœuds (diagnostic : rien ne s'attache)

set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 1

BIN="${OBRAIN_BIN:-./target/release/obrain}"
SELFD="${SELFD_BIN:-./target/release/selfd}"

# ── LES PORTS DE L'UTILISATEUR SONT REFUSÉS, PAS SEULEMENT ÉVITÉS ───────────
# 8432 sert un utilisateur en ce moment même. Un test qui « tomberait par
# hasard » sur ce port ne serait pas un incident de test : ce serait une panne
# d'exploitation. On refuse AVANT de choisir.
refuser_port() {
    case "$1" in
        8432|8433|8455|8090|8144|3082|3083)
            echo "refus : $1 est un service de l'utilisateur (ou réservé). Choisis un port libre."
            return 1
            ;;
    esac
    [ "$1" -gt 8400 ] 2>/dev/null || {
        echo "refus : $1 ≤ 8400 — la porte ne prend que des ports hauts, jamais ceux du service."
        return 1
    }
    return 0
}

# Un port RÉELLEMENT libre : on ne devine pas, on lit la table d'écoute.
port_libre() {
    p=8600
    while [ "$p" -lt 8800 ]; do
        if ! ss -ltn 2>/dev/null | grep -q ":$p "; then
            printf '%s' "$p"
            return 0
        fi
        p=$((p + 1))
    done
    return 1
}

if [ "${1:-}" != "" ]; then
    PORT="$1"
    refuser_port "$PORT" || exit 2
else
    PORT="$(port_libre)" || { echo "aucun port libre trouvé entre 8600 et 8800"; exit 2; }
fi
refuser_port "$PORT" || exit 2

TAG="$$-$RANDOM"
RACINE="/tmp/acceptation-flotte-$TAG"
STORE="/tmp/acceptation-flotte-$TAG.obrain"
IAM="/tmp/acceptation-flotte-iam-$TAG.obrain"
CACHE="$RACINE/cache"
MONDES="$RACINE/mondes"
LOG="$RACINE/moteur.log"
REP="$RACINE/reponse.txt"          # dernier corps HTTP, sur disque (pas de quoting)
# L'espace IAM doit porter le NOM DU STORE servi : la politique du moteur est
# indexée sur la ressource (« tenant », <base>) — un espace nommé autrement fait
# répondre 403 à toute route de lecture, et la porte mesurerait alors une faute
# d'amorçage. Constaté en écrivant cette porte (mesure sur /api/machines).
ESPACE="acceptation-flotte-$TAG"
ALICE="alice-$TAG"
BOB="bob-$TAG"
MDP_ADMIN="admin-$TAG-$RANDOM"

# Trois machines, trois politiques — c'est le cœur du sujet :
#   n3 : exécute (--allow-exec)                → sait FAIRE
#   n2 : observe (sans --allow-exec)           → ne fait pas exécuter
#   n1 : racine en LECTURE SEULE               → n'écrit pas, quoi qu'elle dise
MACHINE_EXEC="n3-porte-$TAG"
MACHINE_OBS="n2-porte-$TAG"
MACHINE_RO="n1-porte-$TAG"

BASE_URL="http://127.0.0.1:$PORT"
SERVEUR=""
NOEUD_EXEC=""
NOEUD_OBS=""
NOEUD_RO=""

echecs=0
verifications=0
ok()   { printf '  ✅ %s\n' "$1"; verifications=$((verifications + 1)); }
ko()   { printf '  ❌ %s\n' "$1"; echecs=$((echecs + 1)); verifications=$((verifications + 1)); }
info() { printf '  ·  %s\n' "$1"; }
titre(){ printf '\n%s\n' "$1"; }

# ── LE NETTOYAGE EST UNE ASSERTION DE PLUS ──────────────────────────────────
# Un test qui laisse un moteur et trois nœuds derrière lui a échoué même s'il
# affiche du vert : il a fabriqué la panne qu'il prétendait mesurer. On tue par
# PID (jamais `pkill -f`, qui a tué le shell de l'auteur pendant l'écriture de
# cette porte : le motif cherché est dans sa propre ligne de commande).
nettoyer() {
    for pid in "$NOEUD_EXEC" "$NOEUD_OBS" "$NOEUD_RO" "$SERVEUR"; do
        [ -n "$pid" ] && kill -9 "$pid" 2>/dev/null
    done
    SERVEUR=""; NOEUD_EXEC=""; NOEUD_OBS=""; NOEUD_RO=""
    [ "${GARDER_TRACE:-0}" = "1" ] && { printf '\n(trace conservée : %s)\n' "$RACINE"; return; }
    rm -rf "$RACINE" "$STORE" "$IAM"
}
trap nettoyer EXIT INT TERM

# ── LE PETIT MATÉRIEL HTTP ──────────────────────────────────────────────────
CODE=""
# http <méthode> <chemin> [jeton] [corps] → pose CODE et REP
http() {
    local methode="$1" chemin="$2" jeton="${3:-}" corps="${4:-}"
    local args=(-s -m 20 -o "$REP" -w '%{http_code}' -X "$methode")
    [ -n "$jeton" ] && args+=(-H "Authorization: Bearer $jeton")
    [ -n "$corps" ] && args+=(-H 'Content-Type: application/json' -d "$corps")
    CODE="$(curl "${args[@]}" "$BASE_URL$chemin" 2>/dev/null)"
    [ -n "$CODE" ] || CODE="000"
}
corps() { head -c "${1:-400}" "$REP" 2>/dev/null | tr -d '\n'; }

attendre_ecoute() { # $1 = secondes
    local fin=$(( $(date +%s) + $1 )) c=""
    while [ "$(date +%s)" -lt "$fin" ]; do
        c="$(curl -s -m 2 -o /dev/null -w '%{http_code}' "$BASE_URL/api/stores" 2>/dev/null)"
        [ "$c" != "000" ] && return 0
        sleep 1
    done
    return 1
}

# La fiche d'UNE machine dans un corps JSON, quelle que soit la forme exacte du
# document : on coupe aux accolades et on garde le fragment qui porte le nom.
# C'est volontairement tolérant sur le SCHÉMA, et strict sur le FOND : le nom de
# la machine et ses capacités doivent être dans le MÊME enregistrement. Un
# document qui les sépare ne répond pas à « que sait faire CHACUNE » — il répond
# « quelles machines » et « quelles capacités », à charge pour le lecteur de
# recoudre. On l'exige quand même, et on le dit.
# ── LIRE UNE FICHE, ET CE QU'ELLE ANNONCE VRAIMENT ──────────────────────────
#
# `fiche` coupait le corps au premier `}`. Cela suffisait tant que
# l'enregistrement d'une machine était PLAT ; depuis que l'annonce est complète
# (chantier « l'annonce porte aussi ce qu'une machine ne fait pas »), la fiche
# contient un objet `annonce` imbriqué, et la coupe s'arrêtait AVANT `capacites`
# et `id`. La porte ne mesurait donc plus rien : elle échouait sur une extraction,
# pas sur un comportement. Réécrite sur du JSON VRAI.
#
# Et les assertions lisent désormais le tableau `capacites` LUI-MÊME au lieu de
# chercher un mot dans toute la fiche. C'est un DURCISSEMENT, pas un ajustement :
# depuis que la fiche porte aussi ce que la machine REFUSE, un `grep '"exec"'` sur
# la fiche entière trouve « exec » dans les refus d'une machine qui ne l'annonce
# PAS en capacité — l'assertion aurait été fausse dans les deux sens.
fiche() { # $1 = corps JSON, $2 = nom de machine  -> l'enregistrement ENTIER, en JSON
    printf '%s' "$1" | FICHE_NOM="$2" python3 -c '
import json, os, sys
nom = os.environ["FICHE_NOM"]
def chercher(o):
    if isinstance(o, dict):
        if o.get("machine") == nom:
            return o
        for v in o.values():
            r = chercher(v)
            if r is not None:
                return r
    elif isinstance(o, list):
        for v in o:
            r = chercher(v)
            if r is not None:
                return r
    return None
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(0)
r = chercher(d)
if r is not None:
    print(json.dumps(r, ensure_ascii=False))
'
}

champ_liste() { # $1 = fiche JSON, $2 = nom du tableau -> une valeur par ligne
    printf '%s' "$1" | CHAMP="$2" python3 -c '
import json, os, sys
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(0)
v = (d or {}).get(os.environ["CHAMP"])
if isinstance(v, list):
    for x in v:
        print(x.get("nom") if isinstance(x, dict) else x)
'
}

champ_id() { # $1 = corps JSON, $2 = nom de machine -> son identifiant
    fiche "$1" "$2" | python3 -c '
import json, sys
try:
    print(json.load(sys.stdin).get("id") or "")
except Exception:
    print("")
'
}

# ── 1) LE DISPOSITIF ────────────────────────────────────────────────────────
titre "1) le dispositif : deux comptes, trois machines, un port libre (le $PORT)"
# Le binaire mesuré est NOMMÉ en entier : `OBRAIN_BIN` peut désigner la copie
# installée par le harness plutôt que le `target/release` du dépôt, et un rapport
# qui ne dit pas ce qu'il a mesuré ne prouve rien.
info "moteur mesuré : $(sha256sum "$BIN" 2>/dev/null | cut -d' ' -f1)"
info "                 $BIN"
info "nœud mesuré   : $(sha256sum "$SELFD" 2>/dev/null | cut -d' ' -f1)"
info "                 $SELFD"
[ -x "$BIN" ]   || { echo "  ❌ moteur absent : $BIN"; exit 2; }
[ -x "$SELFD" ] || { echo "  ❌ nœud absent : $SELFD"; exit 2; }

mkdir -p "$RACINE" "$CACHE" "$MONDES" "$RACINE/racine-exec" "$RACINE/racine-obs" \
         "$RACINE/racine-ro" "$RACINE/etat-exec" "$RACINE/etat-obs" "$RACINE/etat-ro"
"$BIN" init "$STORE" >/dev/null 2>&1 || { echo "  ❌ init du store"; exit 2; }
"$BIN" init "$IAM"   >/dev/null 2>&1 || { echo "  ❌ init du magasin d'identités"; exit 2; }

# ── L'ADMINISTRATION HORS BANDE AVANT LE DÉMARRAGE (voir l'incident en tête) ─
"$BIN" iam amorcer "$IAM" "$ESPACE" --utilisateur admin --mot-de-passe "$MDP_ADMIN" --json \
    >"$RACINE/iam-admin.json" 2>/dev/null \
    || { echo "  ❌ amorçage IAM (binaire sans IAM ?)"; exit 2; }
JETON_ADMIN="$(sed -n 's/.*"jeton"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$RACINE/iam-admin.json" | head -1)"

jeton_de() { # $1 = nom de compte → jeton (le magasin est encore libre à ce stade)
    "$BIN" iam jeton "$IAM" "$1" --json 2>/dev/null \
        | sed -n 's/.*"jeton"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1
}
for nom in "$ALICE" "$BOB"; do
    "$BIN" iam ajouter "$IAM" "$nom" --espace "$ESPACE" --mot-de-passe "mdp-$nom" \
        >/dev/null 2>&1 || ko "création du compte « $nom »"
done
JETON_A="$(jeton_de "$ALICE")"
JETON_B="$(jeton_de "$BOB")"
if [ -n "$JETON_ADMIN" ] && [ -n "$JETON_A" ] && [ -n "$JETON_B" ]; then
    ok "trois identités distinctes (admin, $ALICE, $BOB), deux comptes porteurs de machines"
else
    ko "identités incomplètes (admin=${#JETON_ADMIN} alice=${#JETON_A} bob=${#JETON_B} octets)"
    exit 2
fi

# La racine de n1 est en LECTURE SEULE : le nœud ne PEUT pas écrire, et il ne
# doit donc pas l'annoncer.
chmod 555 "$RACINE/racine-ro"

demarrer_moteur() {
    OBRAIN_IAM=1 OBRAIN_IAM_ACCOUNT=obrain OBRAIN_IAM_STORE="$IAM" \
    OBRAIN_VIZ_CACHE_DIR="$CACHE" OBRAIN_WORLDS_DIR="$MONDES" \
    "$BIN" viz serve --host 127.0.0.1 --port "$PORT" --store "$STORE" --enrich-ricci 0 \
        >>"$LOG" 2>&1 &
    SERVEUR=$!
}
demarrer_moteur
if attendre_ecoute 60; then
    ok "le moteur écoute sur 127.0.0.1:$PORT (pid $SERVEUR, stores et cache sous /tmp)"
else
    ko "le moteur n'écoute pas (voir $LOG)"; exit 2
fi

attacher() { # $1=jeton $2=machine $3=racine $4=état $5=journal [--allow-exec]
    local extra=""
    [ "${6:-}" = "--allow-exec" ] && extra="--allow-exec"
    SELFD_MOTEUR_TOKEN="$1" "$SELFD" attacher --moteur "$BASE_URL" --machine "$2" \
        --root "$3" --state-dir "$4" $extra >>"$5" 2>&1 &
    printf '%s' $!
}
if [ "${SELF_IDLE:-0}" != "1" ]; then
    NOEUD_EXEC="$(attacher "$JETON_A" "$MACHINE_EXEC" "$RACINE/racine-exec" "$RACINE/etat-exec" "$RACINE/noeud-exec.log" --allow-exec)"
    NOEUD_OBS="$(attacher "$JETON_B" "$MACHINE_OBS" "$RACINE/racine-obs" "$RACINE/etat-obs" "$RACINE/noeud-obs.log")"
    NOEUD_RO="$(attacher "$JETON_A" "$MACHINE_RO" "$RACINE/racine-ro" "$RACINE/etat-ro" "$RACINE/noeud-ro.log")"
fi

# On attend que le moteur ait VU les machines, pas que les processus existent :
# c'est la vue du serveur qui est jugée ici.
attendre_machine() { # $1=jeton $2=machine $3=secondes → 0 si le compte voit la machine
    local fin=$(( $(date +%s) + $3 )) c=""
    while [ "$(date +%s)" -lt "$fin" ]; do
        http GET /api/remote/clients "$1"
        c="$(cat "$REP" 2>/dev/null)"
        printf '%s' "$c" | grep -q -F -- "$2" && return 0
        sleep 1
    done
    printf '%s' "$c" >"$RACINE/derniere-vue.txt"
    return 1
}

titre "2) chaque machine DIT ce qu'elle sait faire — et c'est VRAI"
if attendre_machine "$JETON_A" "$MACHINE_EXEC" 45; then
    ok "« $MACHINE_EXEC » est vue par son compte (jalon : le canal sortant fonctionne)"
else
    ko "« $MACHINE_EXEC » jamais vue : $(head -c 200 "$RACINE/derniere-vue.txt" 2>/dev/null)"
    exit 2
fi
if attendre_machine "$JETON_B" "$MACHINE_OBS" 45; then
    ok "« $MACHINE_OBS » est vue par son compte"
else
    ko "« $MACHINE_OBS » jamais vue : $(head -c 200 "$RACINE/derniere-vue.txt" 2>/dev/null)"
fi
if [ "${SELF_IDLE:-0}" != "1" ]; then
    if attendre_machine "$JETON_A" "$MACHINE_RO" 30; then
        ok "« $MACHINE_RO » (racine en lecture seule) est vue par son compte"
    else
        ko "« $MACHINE_RO » jamais vue : $(head -c 200 "$RACINE/derniere-vue.txt" 2>/dev/null)"
    fi
fi

http GET /api/remote/clients "$JETON_A"; VUE_A="$(cat "$REP")"
http GET /api/remote/clients "$JETON_B"; VUE_B="$(cat "$REP")"

# 2.1 — les capacités annoncées doivent DIFFÉRER : si les trois machines disaient
# la même chose, la question « que sait faire CHACUNE » n'aurait pas d'objet.
FICHE_EXEC="$(fiche "$VUE_A" "$MACHINE_EXEC")"
FICHE_RO="$(fiche "$VUE_A" "$MACHINE_RO")"
FICHE_OBS="$(fiche "$VUE_B" "$MACHINE_OBS")"
info "annonces : exec=[$FICHE_EXEC]"
info "            ro  =[$FICHE_RO]"
info "            obs =[$FICHE_OBS]"

CAPS_EXEC="$(champ_liste "$FICHE_EXEC" capacites)"
CAPS_OBS="$(champ_liste "$FICHE_OBS" capacites)"
if printf '%s\n' "$CAPS_EXEC" | grep -qx 'exec' \
   && ! printf '%s\n' "$CAPS_OBS" | grep -qx 'exec'; then
    ok "« exec » annoncé par $MACHINE_EXEC, absent de $MACHINE_OBS : les capacités NE SONT PAS les mêmes"
else
    ko "les capacités ANNONCÉES ne distinguent pas les nœuds (exec=[$(printf '%s' "$CAPS_EXEC" | tr '\n' ',')] obs=[$(printf '%s' "$CAPS_OBS" | tr '\n' ',')])"
fi

# 2.2 — la capacité annoncée est RÉELLE : on la fait exercer, pour de vrai.
ID_EXEC="$(champ_id "$VUE_A" "$MACHINE_EXEC")"
if [ -n "$ID_EXEC" ]; then
    http POST /api/remote/appel "$JETON_A" \
        "{\"client_id\":\"$ID_EXEC\",\"outil\":\"exec\",\"arguments\":{\"cmd\":\"echo\",\"args\":[\"flotte-$TAG\"]}}"
    if [ "$CODE" = "200" ] && grep -q "flotte-$TAG" "$REP"; then
        ok "l'appel « exec » sur $MACHINE_EXEC a RÉELLEMENT tourné (stdout rendu par la machine)"
    else
        ko "« exec » annoncé par $MACHINE_EXEC mais l'appel rend $CODE : $(corps 200)"
    fi
else
    ko "aucun identifiant pour $MACHINE_EXEC — impossible de mesurer sa capacité"
fi

# 2.3 — l'ABSENCE est réelle, elle aussi : le refus doit être MOTIVÉ.
ID_OBS="$(champ_id "$VUE_B" "$MACHINE_OBS")"
if [ -n "$ID_OBS" ]; then
    http POST /api/remote/appel "$JETON_B" \
        "{\"client_id\":\"$ID_OBS\",\"outil\":\"exec\",\"arguments\":{\"cmd\":\"echo\",\"args\":[\"interdit\"]}}"
    if [ "$CODE" = "200" ] && grep -q '"ok":false' "$REP" && grep -q '"refus"' "$REP" \
       && grep -q 'NON AUTORISÉE' "$REP"; then
        # Le motif est IMPRIMÉ : c'est lui qui rend le refus actionnable, et une
        # porte qui se contente de « c'est refusé » ne prouve pas que le refus
        # dise POURQUOI (doctrine : le refus remonte, motivé).
        ok "l'appel « exec » sur $MACHINE_OBS est PORTÉ au nœud, qui refuse avec SON motif : « $(grep -o '"refus":"[^"]*"' "$REP" | head -1 | cut -c10-160) »"
    elif [ "$CODE" = "409" ] && grep -q 'NON AUTORISÉE' "$REP" && grep -q 'allow-exec' "$REP"; then
        # ── LE MOTEUR PEUT REFUSER SANS ENVOYER L'APPEL, ET C'EST MIEUX ──────
        # Depuis que l'annonce porte ce que la machine REFUSE, le moteur SAIT que
        # ce nœud n'a pas `exec` : il répond 409 sans faire le voyage, et il CITE
        # le motif du nœud. L'assertion accepte donc les deux formes — mais elle
        # exige dans les deux le motif DU NŒUD (« NON AUTORISÉE », « allow-exec »),
        # jamais un refus générique : un refus sans motif ne se corrige pas.
        ok "l'appel « exec » sur $MACHINE_OBS est refusé SANS être envoyé (409), et le moteur CITE le motif du nœud : « $(grep -o '"error":"[^"]*"' "$REP" | head -1 | cut -c10-200) »"
    else
        ko "l'absence d'« exec » sur $MACHINE_OBS n'est pas un refus MOTIVÉ ($CODE) : $(corps 240)"
    fi
else
    ko "aucun identifiant pour $MACHINE_OBS — impossible de mesurer son refus"
fi

# 2.4 — une capacité NON exercable ne doit pas être annoncée. La racine de n1 est
# en lecture seule : si « write » y figure, le moteur proposera une écriture
# vouée à l'échec — le défaut que la doctrine interdit, mesuré ici.
if [ "${SELF_IDLE:-0}" != "1" ]; then
    CAPS_RO="$(champ_liste "$FICHE_RO" capacites)"
    REF_RO="$(champ_liste "$FICHE_RO" capacites_refusees)"
    info "n1 (racine en lecture seule) annonce : capacites=[$(printf '%s' "$CAPS_RO" | tr '\n' ',')] refusees=[$(printf '%s' "$REF_RO" | tr '\n' ',')]"
    if printf '%s\n' "$CAPS_RO" | grep -qx 'write'; then
        ko "« write » est annoncé par $MACHINE_RO alors que sa racine ($RACINE/racine-ro) est en LECTURE SEULE : la capacité annoncée n'est pas exercable (annoncer n'est pas décider)"
    else
        ok "aucune capacité non exercable annoncée par $MACHINE_RO (racine en lecture seule respectée)"
    fi
    # ── ET LA MOITIÉ QUI MANQUAIT : LE REFUS DOIT ÊTRE DIT ──────────────────
    # « write » absent de `capacites` ne suffit pas : un client doit pouvoir LIRE
    # ce que la machine REFUSE, sinon il propose un geste voué à l'échec et
    # l'échec arrive après coup. C'est le manque 4 du document, mesuré ici — et non
    # plus seulement affirmé.
    if printf '%s\n' "$REF_RO" | grep -qx 'write'; then
        ok "son refus de « write » est ANNONCÉ (capacites_refusees) — ce qu'elle ne fait pas est DIT, pas seulement absent"
    else
        ko "« write » n'est pas annoncé en capacité par $MACHINE_RO, mais son refus n'est PAS dit (capacites_refusees=[$(printf '%s' "$REF_RO" | tr '\n' ',')]) : la moitié « ce qu'elle ne fait pas » manque"
    fi
fi

# ── 3) CE QUE LE SERVEUR DIT DE VOS MACHINES ────────────────────────────────
titre "3) « vos machines » — la seule route qui existe : GET /api/remote/clients"
if [ -n "$FICHE_EXEC" ] && printf '%s' "$FICHE_EXEC" | grep -q -F -- "$MACHINE_EXEC"; then
    ok "avec l'identité de $ALICE, la route NOMME $MACHINE_EXEC et ses capacités"
else
    ko "la route ne nomme pas $MACHINE_EXEC : $(printf '%s' "$VUE_A" | head -c 200)"
fi
printf '%s' "$VUE_A" | grep -q -F -- "$MACHINE_OBS" \
    && ko "la route d'$ALICE nomme $MACHINE_OBS : elle ne devrait voir que les siennes" \
    || ok "la route d'$ALICE ne nomme AUCUNE machine d'un autre compte"

http GET /api/remote/clients ""
if [ "$CODE" = "401" ]; then
    ok "sans identité → 401 : le canal ne parle qu'à VOS machines (jamais de liste anonyme)"
else
    ko "sans identité → $CODE, et le corps : $(corps 160)"
fi

# ── 4) LA QUESTION — LE SUJET DE CETTE PORTE ────────────────────────────────
titre "4) LA QUESTION : « quelles machines connais-tu, et que sait faire chacune ? »"
# On ne présume pas du nom de la route : on interroge les candidates et on DIT ce
# que chacune a répondu. Une route qui répond 200 sans nommer les machines ne
# répond pas à la question ; une route absente se dit absente.
routes_flotte() {
    if [ -n "${ROUTE_FLOTTE:-}" ]; then
        printf '%s\n' "$ROUTE_FLOTTE"
        return
    fi
    printf '%s\n' /api/remote/machines /api/machines /api/flotte /api/fleet \
                  /api/harness/machines /api/engine/machines
}

# ── LA QUESTION SE POSE À UNE IDENTITÉ, PAS « À L'ADMIN » ───────────────────
#
# Cette section interrogeait avec le jeton ADMIN et exigeait qu'on lui nomme les
# machines d'alice et de bob — alors que sa propre section 6 valide que « le
# compte admin, qui n'a attaché aucune machine, n'en voit aucune ». Les deux ne
# peuvent pas être vrais ensemble : la porte se contredisait.
#
# La contradiction a été tranchée le 2026-09-16 : la question se répond PAR
# COMPTE. Trois raisons, toutes tirées du dépôt plutôt que d'une préférence :
#
#   1. la loi apprise du bug est « une liste ne doit jamais contenir une ressource
#      dont on n'a pas vérifié le droit » (docs/MACHINES-CONNUES-2026-09-15.md §4),
#      et c'est une loi PAR IDENTITÉ ;
#   2. l'énoncé d'origine est « un self qui arrive à tous les manager » — MON self
#      gérant MES machines — jamais « un compte voit celles des autres » ;
#   3. l'IAM de ce dépôt scope déjà la mémoire par compte, et le cloisonnement est
#      une qualité que cette porte VÉRIFIE. Une vue inter-comptes serait une
#      régression de confidentialité, pas une fonctionnalité.
#
# Cette section n'a donc pas été ASSOUPLIE : elle est devenue plus exigeante. Elle
# interroge le compte PROPRIÉTAIRE de chaque machine, et elle ajoute une
# assertion que la version précédente ne portait pas — la NON-FUITE par cette
# route-là, la section 6 ne la vérifiant que sur /api/remote/clients.
ROUTE_OK=""
for r in $(routes_flotte); do
    http GET "$r" "$JETON_ADMIN"
    info "GET $r (admin) → $CODE  $(corps 120)"
    if [ "$CODE" = "200" ] && [ -z "$ROUTE_OK" ]; then
        ROUTE_OK="$r"
    fi
done

if [ -n "$ROUTE_OK" ]; then
    ok "une route répond à la question : $ROUTE_OK"
else
    ko "AUCUNE route ne répond à la question de la flotte (voir les codes ci-dessus) : la réponse est le vide, pas l'erreur"
fi

FLOTTE_A=""; FLOTTE_B=""; FLOTTE_ADMIN=""
if [ -n "$ROUTE_OK" ]; then
    http GET "$ROUTE_OK" "$JETON_A";   FLOTTE_A="$(cat "$REP")"
    info "alice → $(corps 200)"
    http GET "$ROUTE_OK" "$JETON_B";   FLOTTE_B="$(cat "$REP")"
    info "bob   → $(corps 200)"
    http GET "$ROUTE_OK" "$JETON_ADMIN"; FLOTTE_ADMIN="$(cat "$REP")"

    # (a) LE COMPTE PROPRIÉTAIRE VOIT LES SIENNES, NOMMÉES.
    for m in "$MACHINE_EXEC" "$MACHINE_RO"; do
        printf '%s' "$FLOTTE_A" | grep -q -F -- "$m" \
            && ok "alice, qui la possède, se voit nommer $m" \
            || ko "la réponse d'alice ne nomme pas SA machine $m : $(printf '%s' "$FLOTTE_A" | head -c 200)"
    done
    if printf '%s' "$FLOTTE_B" | grep -q -F -- "$MACHINE_OBS"; then
        ok "bob, qui la possède, se voit nommer $MACHINE_OBS"
    else
        ko "la réponse de bob ne nomme pas SA machine $MACHINE_OBS : $(printf '%s' "$FLOTTE_B" | head -c 200)"
    fi

    # (b) CE QUE SAIT FAIRE CHACUNE — et que ce n'est pas la même chose.
    F_EXEC="$(fiche "$FLOTTE_A" "$MACHINE_EXEC")"
    F_RO="$(fiche "$FLOTTE_A" "$MACHINE_RO")"
    # Le tableau `capacites` LUI-MÊME, et non un mot cherché dans la fiche : la
    # route sert le même enregistrement complet, où « exec » figure aussi dans les
    # capacités REFUSÉES de la machine qui ne l'annonce pas.
    CAPS_F_EXEC="$(champ_liste "$F_EXEC" capacites)"
    CAPS_F_RO="$(champ_liste "$F_RO" capacites)"
    if printf '%s\n' "$CAPS_F_EXEC" | grep -qx 'exec' \
       && ! printf '%s\n' "$CAPS_F_RO" | grep -qx 'exec'; then
        ok "les capacités sont DISTINCTES : « $MACHINE_EXEC » annonce exec, « $MACHINE_RO » non"
    else
        ko "la réponse ne distingue pas les capacités (exec=[$(printf '%s' "$CAPS_F_EXEC" | tr '\n' ',')] ro=[$(printf '%s' "$CAPS_F_RO" | tr '\n' ',')])"
    fi
    # Une réponse qui nomme les machines mais tait leurs capacités est le défaut
    # que §4.2 du document décrit : « le modèle propose des gestes qu'il ne peut
    # pas prendre, et l'échec arrive après coup ».
    if [ -n "$CAPS_F_EXEC" ]; then
        ok "les capacités sont DITES, pas seulement les noms"
    else
        ko "la réponse nomme les machines sans dire leurs capacités (fiche : $(printf '%s' "$F_EXEC" | head -c 140))"
    fi
    # … et la moitié qui manquait (manque 4) : ce que la machine REFUSE doit être
    # LISIBLE par un client, sinon il propose un geste voué à l'échec.
    REF_F_RO="$(champ_liste "$F_RO" capacites_refusees)"
    if printf '%s\n' "$REF_F_RO" | grep -qx 'write'; then
        ok "les refus sont DITS aussi par cette route : « $MACHINE_RO » annonce refuser write"
    else
        ko "les refus ne sont pas dits par cette route (refusees=[$(printf '%s' "$REF_F_RO" | tr '\n' ',')]) — « write » n'est pas annoncé en capacité, mais son refus devrait être LISIBLE"
    fi

    # (c) AUCUNE FUITE PAR CETTE ROUTE. C'est la raison d'être du choix « par
    #     compte », donc c'est elle qui doit être mesurée, et pas seulement
    #     affirmée.
    if printf '%s' "$FLOTTE_A" | grep -q -F -- "$MACHINE_OBS"; then
        ko "FUITE : la réponse d'alice nomme $MACHINE_OBS, qui appartient à bob"
    else
        ok "alice ne voit AUCUNE machine de bob par cette route"
    fi
    FUITES_B=0
    for m in "$MACHINE_EXEC" "$MACHINE_RO"; do
        printf '%s' "$FLOTTE_B" | grep -q -F -- "$m" && FUITES_B=$((FUITES_B + 1))
    done
    if [ "$FUITES_B" -eq 0 ]; then
        ok "bob ne voit AUCUNE machine d'alice par cette route"
    else
        ko "FUITE : la réponse de bob nomme $FUITES_B machine(s) d'alice"
    fi

    # (d) UN COMPTE SANS MACHINE LE DIT — un fait, pas un silence.
    printf '%s' "$FLOTTE_ADMIN" | grep -q '"aucune"[[:space:]]*:[[:space:]]*true' \
        && ok "admin, qui n'a rien attaché, se voit répondre « aucune machine » — EXPLICITEMENT" \
        || ko "la réponse à admin ne dit pas explicitement qu'il ne connaît aucune machine : $(printf '%s' "$FLOTTE_ADMIN" | head -c 200)"
else
    ko "impossible de nommer $MACHINE_EXEC : aucune route ne répond (l'assertion échoue par absence de réponse, pas par contenu)"
    ko "impossible de nommer $MACHINE_OBS : aucune route ne répond"
    ko "impossible de dire ce que sait faire chacune : aucune route ne répond"
fi

# ── 5) LE VIDE N'A PAS LA FORME D'UNE BONNE RÉPONSE ─────────────────────────
titre "5) le vide n'a pas la forme d'une bonne réponse (§2 du document)"
http GET "${ROUTE_OK:-/api/remote/machines}" ""
if [ "$CODE" = "401" ] || [ "$CODE" = "403" ]; then
    ok "sans identité, la question est REFUSÉE ($CODE) — pas servie vide"
else
    ko "sans identité, la question rend $CODE : $(corps 160)"
fi
if [ -n "$ROUTE_OK" ]; then
    # Le vide est un FAIT quand il est DIT (admin, couvert en 4(d)) et un MENSONGE
    # quand il cache des machines : pour le compte qui en possède, une liste vide
    # serait le « ok sur un vide intégral » que le document interdit. C'est donc
    # la réponse du PROPRIÉTAIRE qu'il faut regarder ici, pas celle d'admin.
    if printf '%s' "$FLOTTE_A" | grep -qE '"machines"[[:space:]]*:[[:space:]]*\[\]'; then
        ko "la réponse d'alice est une liste VIDE alors qu'elle possède deux machines"
    else
        ok "le compte qui possède des machines ne reçoit PAS une liste vide"
    fi
else
    ko "route absente : rien ne peut dire « je ne connais aucune machine » — l'absence de route et l'absence de machine sont indistinguables"
fi
# Le refus, quand il y en a un, doit porter un MOTIF (le silence ressemble à une
# panne). On vérifie la forme du corps, pas seulement le code.
http GET /api/remote/clients ""
if [ "$CODE" = "401" ] && grep -qi 'error\|refus\|identité' "$REP"; then
    ok "le refus est MOTIVÉ (« $(corps 90) »)"
else
    ko "le refus ($CODE) n'est pas motivé : $(corps 120)"
fi

# ── 6) CLOISONNEMENT ────────────────────────────────────────────────────────
titre "6) cloisonnement : chaque compte ne voit que ses machines"
printf '%s' "$VUE_B" | grep -q -F -- "$MACHINE_EXEC" \
    && ko "$BOB voit $MACHINE_EXEC (machine d'$ALICE)" \
    || ok "$BOB ne voit jamais $MACHINE_EXEC"
printf '%s' "$VUE_A" | grep -q -F -- "$MACHINE_OBS" \
    && ko "$ALICE voit $MACHINE_OBS (machine de $BOB)" \
    || ok "$ALICE ne voit jamais $MACHINE_OBS"
http GET /api/remote/clients "$JETON_ADMIN"
printf '%s' "$(cat "$REP")" | grep -qE '"clients"[[:space:]]*:[[:space:]]*\[\]' \
    && ok "le compte admin, qui n'a attaché aucune machine, n'en voit aucune : le registre est PAR COMPTE" \
    || ko "le compte admin voit des machines qu'il n'a pas attachées : $(corps 200)"
# Connaître un identifiant ne donne aucun droit : c'est la propriété qui rend un
# `client_id` publiable sans danger (décision 2 en tête de `remote.rs`).
if [ -n "$ID_EXEC" ]; then
    http POST /api/remote/appel "$JETON_B" \
        "{\"client_id\":\"$ID_EXEC\",\"outil\":\"status\",\"arguments\":{}}"
    if [ "$CODE" != "200" ]; then
        ok "$BOB, qui CONNAÎT l'identifiant $ID_EXEC, ne peut pas l'appeler ($CODE : $(corps 90))"
    else
        ko "$BOB a pu appeler la machine d'$ALICE avec son seul identifiant"
    fi
fi

# ── 7) CONNUE ≠ CONNECTÉE : la machine partie reste nommée ──────────────────
titre "7) une machine CONNUE mais ABSENTE reste nommée (sinon « connaître » veut dire « voir »)"
[ -n "$NOEUD_OBS" ] && kill -9 "$NOEUD_OBS" 2>/dev/null; NOEUD_OBS=""
sleep 1
info "« $MACHINE_OBS » a été arrêtée ; le moteur redémarre (le registre est-il un fait, ou une vue ?)"
kill -9 "$SERVEUR" 2>/dev/null; wait "$SERVEUR" 2>/dev/null; SERVEUR=""
demarrer_moteur
if attendre_ecoute 60; then
    ok "le moteur est redémarré sur le même port, avec les mêmes stores"
else
    ko "le moteur n'est pas revenu (voir $LOG)"
fi
if attendre_machine "$JETON_A" "$MACHINE_EXEC" 45; then
    ok "$MACHINE_EXEC s'est RÉATTACHÉE seule après le redémarrage (le canal sortant tient sa promesse)"
else
    ko "$MACHINE_EXEC ne s'est pas réattachée après le redémarrage : $(head -c 200 "$RACINE/derniere-vue.txt" 2>/dev/null)"
fi
http GET /api/remote/clients "$JETON_B"; APRES="$(cat "$REP")"
if printf '%s' "$APRES" | grep -q -F -- "$MACHINE_OBS"; then
    ok "$BOB retrouve $MACHINE_OBS NOMMÉE après le redémarrage, alors qu'elle n'est plus connectée"
    # Le marqueur d'absence est cherché LARGE : la porte n'impose pas un
    # vocabulaire, elle exige que l'absence soit DITE. `vivant: false`,
    # `absent`, `hors ligne`, `injoignable` sont quatre façons également bonnes
    # de le dire — refuser l'une d'elles ferait échouer une implémentation juste.
    printf '%s' "$APRES" | grep -qiE '"vivant"[[:space:]]*:[[:space:]]*false|absent|hors ligne|injoignable|derniere_vue|dernière_vue|derniere_presence|dernière_présence|vu pour la derniere' \
        && ok "et la réponse DIT qu'elle est absente (elle ne la fait pas passer pour connectée)" \
        || ko "la machine est nommée mais rien ne dit qu'elle est ABSENTE (une machine éteinte et une machine vivante se ressemblent)"
else
    ko "$BOB ne retrouve plus $MACHINE_OBS après le redémarrage du moteur : la flotte était une vue en mémoire, pas un fait ($(printf '%s' "$APRES" | head -c 160))"
    ko "(corollaire) rien ne distingue « jamais vue » de « connue mais absente »"
fi

# ── 8) LA FLOTTE EST UN FAIT DU GRAPHE ──────────────────────────────────────
titre "8) la machine comme NŒUD du graphe (§3 et §4.1 du document)"
kill -9 "$SERVEUR" 2>/dev/null; wait "$SERVEUR" 2>/dev/null; SERVEUR=""
info "moteur arrêté : le verrou exclusif du store est rendu"

# ── POURQUOI LA PORTE DÉCLENCHE LA PASSE ELLE-MÊME ──────────────────────────
# Le moteur sert ses bases en LECTURE (flock partagé) : il ne peut PAS écrire les
# nœuds `machine:` lui-même — ce n'est pas un défaut à contourner, c'est
# l'invariant mono-écrivain qui protège les données. Faire de la flotte un fait du
# graphe est donc un geste SÉPARÉ, exactement comme `obrain memoire --appliquer` :
#
#     obrain machines --store <base> --appliquer
#
# En service, c'est un timer qui le déclenche. Une instance d'acceptation n'a pas
# de timer : la porte exécute donc le MÉCANISME RÉEL, puis constate. Elle
# n'INVENTE aucun nœud — si la passe échoue, l'assertion qui suit échoue aussi.
info "passe de la flotte : « obrain machines --appliquer » sur le store servi"
if OBRAIN_VIZ_CACHE_DIR="$CACHE" "$BIN" machines --store "$STORE" --appliquer \
        >>"$RACINE/passe-flotte.log" 2>&1; then
    ok "la passe de la flotte s'exécute sans erreur sur le store servi"
else
    ko "la passe de la flotte a échoué : $(tail -c 200 "$RACINE/passe-flotte.log" | tr -d '\n')"
fi

# La requête est celle du RÉCIT, et la propriété n'est pas choisie au hasard :
# on cherche par `qualified_name` parce que c'est ce que lit la surface dont se
# sert une IA. Un nœud écrit avec `qn` seul serait INVISIBLE au lecteur qu'on veut
# servir — c'est ce que la première version de cette assertion avait révélé.
REQ="MATCH (n) WHERE n.qualified_name STARTS WITH 'machine:' RETURN n.qualified_name"
if "$BIN" query "$STORE" --read-only "$REQ" >"$RACINE/requete.txt" 2>"$RACINE/requete.err"; then
    for m in "$MACHINE_EXEC" "$MACHINE_OBS" "$MACHINE_RO"; do
        if grep -q -F -- "machine:$m" "$RACINE/requete.txt"; then
            ok "le store servi porte « machine:$m »"
        else
            ko "le store servi ne porte pas « machine:$m » (réponse : $(tr -d '\n' <"$RACINE/requete.txt" | head -c 100))"
        fi
    done
    # Un nœud qui porte le NOM mais tait ce que la machine sait faire ne répond
    # pas à la question posée au graphe : « … et que sait faire CHACUNE ? ».
    if "$BIN" query "$STORE" --read-only \
            "MATCH (m:Machine) RETURN m.qn, m.capacites_annoncees, m.compte" \
            >"$RACINE/requete2.txt" 2>/dev/null \
        && grep -q 'capacites_annoncees' "$RACINE/requete2.txt"; then
        ok "chaque nœud machine porte ses CAPACITÉS et son compte, pas seulement son nom"
    else
        ko "les nœuds machine: ne portent pas leurs capacités : $(tr -d '\n' <"$RACINE/requete2.txt" 2>/dev/null | head -c 140)"
    fi
else
    ko "la requête sur le store a échoué : $(head -c 160 "$RACINE/requete.err")"
fi

# ── 9) LA PASSE TOURNE TOUTE SEULE ──────────────────────────────────────────
#
# Le manque que cette section ferme, et il était écrit noir sur blanc dans le
# compte rendu du chantier : « AJOUTER une exigence qui vérifie qu'un déclencheur
# existe, pour que « la passe tourne toute seule » devienne un fait MESURÉ par la
# porte et non une intention ».
#
# Une passe que personne ne déclenche n'existe pas pour l'utilisateur : elle n'a
# été prouvée que par cette porte, qui la lance à la main. On exige donc quatre
# choses, et chacune attrape un mode de défaillance différent :
#
#   9.1 le dépôt PORTE le déclencheur — sinon le mécanisme n'est pas versionné,
#       il vit dans un shell qui mourra avec lui ;
#   9.2 il est INSTALLÉ et ACTIF — un fichier dans le dépôt ne déclenche rien ;
#   9.3 il a TOURNÉ et la passe a RÉUSSI — un timer qui n'a jamais sonné, ou qui
#       échoue à chaque passage, est un timer qu'on apprend à ignorer (le motif de
#       défaillance que ce dépôt refuse sous toutes ses formes) ;
#   9.4 il VISE la bonne cible — la base SERVIE est tenue en lecture par le
#       moteur : un timer qui la viserait échouerait à chaque passage. La cible
#       juste est la base de chaque compte, que nul service ne tient en exclusif.
titre "9) la passe de la flotte tourne TOUTE SEULE (déclencheur mesuré)"
UNIT_TIMER="obrain-machines-appliquer.timer"
UNIT_SERVICE="obrain-machines-appliquer.service"

# 9.1 — le mécanisme est-il dans le dépôt ?
if [ -f deploy/systemd/$UNIT_SERVICE ] && [ -f deploy/systemd/$UNIT_TIMER ]; then
    ok "le dépôt PORTE le déclencheur (deploy/systemd/$UNIT_SERVICE + $UNIT_TIMER) — le mécanisme est versionné"
else
    ko "le dépôt ne porte PAS de déclencheur ($UNIT_SERVICE / $UNIT_TIMER) : la passe ne peut tourner que si quelqu'un la lance à la main"
fi

# 9.2 — est-il installé et armé ?
if systemctl --user cat "$UNIT_TIMER" >/dev/null 2>&1; then
    ETAT_TIMER="$(systemctl --user is-active "$UNIT_TIMER" 2>/dev/null)"
    if [ "$ETAT_TIMER" = "active" ]; then
        ok "$UNIT_TIMER est INSTALLÉ et actif"
    else
        ko "$UNIT_TIMER est installé mais « ${ETAT_TIMER:-inconnu} » — le poser et l'armer : install -m 644 deploy/systemd/$UNIT_TIMER ~/.config/systemd/user/ && systemctl --user daemon-reload && systemctl --user enable --now $UNIT_TIMER"
    fi
else
    ko "$UNIT_TIMER n'est PAS INSTALLÉ : install -m 644 deploy/systemd/$UNIT_SERVICE deploy/systemd/$UNIT_TIMER ~/.config/systemd/user/ && systemctl --user daemon-reload && systemctl --user enable --now $UNIT_TIMER"
fi

# 9.3 — a-t-il tourné, et la passe a-t-elle réussi ?
if systemctl --user cat "$UNIT_SERVICE" >/dev/null 2>&1; then
    DERNIERE="$(systemctl --user show "$UNIT_TIMER" -p LastTriggerUSec --value 2>/dev/null)"
    CODE_PASSE="$(systemctl --user show "$UNIT_SERVICE" -p ExecMainStatus --value 2>/dev/null)"
    if [ -n "$DERNIERE" ] && [ "$DERNIERE" != "n/a" ] && [ "$DERNIERE" != "" ]; then
        ok "le déclencheur A SONNÉ (dernier déclenchement : ${DERNIERE%%.*})"
        if [ "$CODE_PASSE" = "0" ]; then
            ok "la passe a RÉUSSI (ExecMainStatus=0) — elle ne « tourne » pas en échouant"
        else
            ko "la passe a échoué au dernier passage (ExecMainStatus=${CODE_PASSE:-inconnu}) : un timer qui échoue toujours est un timer qu'on apprend à ignorer"
        fi
    else
        ko "le déclencheur n'a JAMAIS sonné : rien ne prouve que la passe tourne en service"
    fi

    # 9.4 — la cible : le script PAR COMPTE, jamais la base servie.
    EXEC="$(systemctl --user show "$UNIT_SERVICE" -p ExecStart --value 2>/dev/null)"
    if printf '%s' "$EXEC" | grep -q 'machines-appliquer-comptes.sh'; then
        ok "il vise la base de CHAQUE COMPTE (machines-appliquer-comptes.sh) — que nul service ne tient en exclusif"
    elif printf '%s' "$EXEC" | grep -q 'machines-appliquer.sh'; then
        ko "il vise machines-appliquer.sh, donc la base SERVIE : celle que le moteur tient en lecture pendant qu'il la sert, et la passe y échouera à chaque passage"
    else
        mes "cible non reconnue dans ExecStart : $(printf '%s' "$EXEC" | head -c 160)"
    fi
else
    ko "$UNIT_SERVICE n'est pas installé : le déclencheur n'a pas de charge à lancer"
fi

# ── VERDICT ─────────────────────────────────────────────────────────────────
titre "verdict"
if [ "$echecs" = "0" ]; then
    printf '  ✅ la flotte est nommée, ses capacités sont dites, mesurées et cloisonnées (%d vérifications)\n' "$verifications"
    exit 0
fi
printf '  ❌ %d vérification(s) en échec sur %d\n' "$echecs" "$verifications"
printf '  (chaque ❌ nomme ce qui manque ; docs/ACCEPTATION-FLOTTE-2026-09-16.md les ordonne)\n'
[ "${GARDER_TRACE:-0}" = "1" ] && printf '  trace : %s\n' "$RACINE"
exit 1
