#!/usr/bin/env bash
# ══════════════════════════════════════════════════════════════════════════════
# PORTE D'ACCEPTATION : CHAQUE COMPTE A SON SELF, ET SA MÉMOIRE VA DANS SA BASE
# ══════════════════════════════════════════════════════════════════════════════
#
# ── POURQUOI CE FICHIER PORTE DEUX PORTES, ET PAS UNE ──────────────────────
#
# Deux lignées ont écrit ce script EN MÊME TEMPS, chacune de son côté, sans se
# voir — et leurs sujets ne sont pas les mêmes :
#
#   · lignée canonique (`b6c6da2c`) — « LA CHAÎNE COMPLÈTE TIENT-ELLE ? » : la
#     conversation journalise une intention pour le COMPTE, la passe d'application
#     (le vrai `scripts/memoire-appliquer-comptes.sh`, isolé) l'applique, le nœud
#     apparaît dans la base DU COMPTE, et cette base est RELUE par la conversation
#     suivante. Y compris les défauts 5.3 (base écrite là où personne ne relit),
#     5.4 (un WARN du moteur n'est pas un refus), et le verrou de relecture ;
#
#   · lignée multi-écrivain (`79d57cf5`) — « DEUX COMPTES SE MÉLANGENT-ILS ? » :
#     l'admin crée deux comptes, chacun voit SON self (`self:<nom>`, pas
#     `self:agent`), les deux selfs sont DISTINCTS, leurs journaux sont DEUX
#     FICHIERS, et un appel SANS identité ne laisse rien passer.
#
# Aucune des deux ne contient l'autre, et une résolution automatique (`rerere`)
# avait retenu la première SEULE en silence : les 170 lignes de la seconde — les
# contrôles d'isolation entre comptes, qui sont exactement la propriété que ce
# chantier doit garantir — avaient disparu. Ce fichier est la RÉUNION des deux :
# chaque assertion des deux versions d'origine est présente, y compris celles qui
# se recouvrent partiellement (elles ne mesurent alors pas la même chose).
#
# ── LE PLAN (chaque titre dit ce qu'il garde) ───────────────────────────────
#   0.     prérequis et ISOLATION vérifiée — aucune base vivante n'est approchée ;
#   1.     serveur armé, cache de mémoire isolé ;
#   2.     deux comptes, chacun sa base (créés par l'admin) ;
#   3.     la conversation appartient au COMPTE (`self:<compte>`) ;
#   3bis.  les deux selfs sont DISTINCTS et leurs journaux sont DEUX FICHIERS ;
#   4.     sans identité, la route REFUSE — et ne rend le self de personne ;
#   4bis.  le self de l'INSTANCE reste intact (lu par le CLI sur la synapse) ;
#   5.     la conversation journalise une intention pour ce compte ;
#   6.     la passe d'application — le VRAI script, isolé par variables ;
#   6bis.  le témoin : la mémoire laisse une trace DATÉE après consommation ;
#   7.     le nœud est dans la base DU COMPTE, rattaché à son Self ;
#   7bis.  la boucle se referme : la mémoire du compte est RELUE ;
#   8.     le journal est CONSOMMÉ (l'intention appliquée n'est pas rejouée) ;
#   9.     la base de l'AUTRE compte est INTACTE (empreinte sha256) ;
#  10.     la passe n'est pas MUETTE quand il n'y a rien à faire ;
#  11.     une panne ne prend pas la forme du repos ;
#  12.     « repos » et « chaîne cassée » ne s'écrivent PAS PAREIL ;
#  13.     la mémoire est écrite là où la conversation la RELIT (défaut 5.3) ;
#  14.     un avertissement du moteur n'est pas un refus (défaut 5.4) ;
#  15.     après une relecture, la passe peut-elle ENCORE appliquer ? (verrou)
#
# ── CE QUE LA PORTE NE PROUVE PAS, ET QUI EST DIT ───────────────────────────
# L'étape 5 dépend d'un MODÈLE : c'est lui qui décide d'appeler l'outil
# `obrain_memoriser`. Par défaut la porte exige donc une conversation RÉELLE
# (elle interroge `OBRAIN_LLM_URL` et échoue si le modèle n'appelle pas l'outil).
# Avec `--sans-llm`, elle écrit à la place une entrée au format EXACT du journal
# (`memoire::Entree::vers_json`, clés triées) et marque l'assertion « conversation
# réelle » comme NON TESTÉE — jamais comme réussie. Une porte qui dégraderait en
# silence serait exactement le défaut qu'elle est censée garder. Le verdict
# distingue donc ÉCHEC et NON TESTÉ, comme les deux versions d'origine.
#
# ── ISOLATION ───────────────────────────────────────────────────────────────
# Aucune base vivante n'est approchée : DATA, cache de mémoire, mondes et
# magasin IAM vivent sous /tmp, dans un répertoire propre à cette exécution, et
# la porte REFUSE de tourner si ces chemins ne sont pas sous /tmp. Le port est
# libre, supérieur à 8400, et jamais 8432 / 8433 / 8455 (services de l'utilisateur).
#
# Usage : scripts/acceptation-self-par-compte.sh [port] [--sans-llm]
#         (port par défaut 8457 ; l'autre lignée proposait 8442)
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 1

PORT="8457"
SANS_LLM=0
for a in "$@"; do
    case "$a" in
        --sans-llm) SANS_LLM=1 ;;
        ''|*[!0-9]*) echo "usage : $0 [port] [--sans-llm]" >&2; exit 2 ;;
        *) PORT="$a" ;;
    esac
done
# ── LES PORTS REFUSÉS SONT CEUX DES DEUX PORTES D'ORIGINE ───────────────────
# 8432 / 8433 (services de l'utilisateur, refusés par les deux) et 8455 (refusé
# par la lignée canonique). Un port ≤ 8400 n'est pas un port d'essai : il tombe
# dans la plage des services, et une porte qui s'y branche mesure autre chose que
# ce qu'elle croit.
if [ "$PORT" -le 8400 ] || [ "$PORT" = "8432" ] || [ "$PORT" = "8433" ] || [ "$PORT" = "8455" ]; then
    echo "refus : $PORT n'est pas un port libre (>8400, hors 8432/8433/8455)." >&2
    exit 2
fi

BIN="${OBRAIN_BIN:-./target/release/obrain}"
TAG="$$"
RACINE="/tmp/acceptation-self-compte-$TAG"
DATA="$RACINE/data"
CACHE="$RACINE/cache"
MONDES="$RACINE/mondes"
IAM="$RACINE/iam.obrain"
ESPACE="acceptation-self-$TAG"
# ── LE STORE PORTE LE NOM DE L'ESPACE, ET CE N'EST PAS UN DÉTAIL ────────────
# Défaut mesuré en écrivant cette porte (et il coûte une heure si on ne le voit
# pas) : la ressource jugée par la politique est le TENANT, dont le nom est tiré
# du STORE — `…/runtime-essai.obrain` ⇒ `orn:obrain:graph:obrain:tenant/runtime-essai`
# — alors que le compte reçoit des droits sur son ESPACE. Nommer l'essai
# « runtime-essai » et l'espace « acceptation-self-<pid> » produit donc un
# `permission denied: action=graph:read resource=… tenant/runtime-essai` sur
# TOUTES les routes, y compris `/api/engine/memoire`, avec une identité
# pourtant valide (`/api/auth/whoami` répond `authentifie: true`). Une porte
# qui ne voit pas ce piège mesure un refus de droits et l'appelle une panne
# de mémoire. Le store prend donc le nom de l'espace.
STORE="$DATA/$ESPACE.obrain"
LOG="$RACINE/serveur.log"
CHAT="$RACINE/chat.log"
ALICE="alice-$TAG"
BOB="bob-$TAG"
MDP_ADMIN="admin-$TAG-$RANDOM"
# ── LES MOTS DE PASSE DES COMPTES SONT ENGENDRÉS, ET PARTAGÉS ───────────────
# L'autre lignée les dérivait du nom (`motdepasse-$nom`) ; les deux écritures
# sont équivalentes pour l'IAM. On garde l'engendrement aléatoire (une porte ne
# doit pas laisser derrière elle des comptes à mot de passe devinable, même sous
# /tmp) et on les définit UNE fois : les étapes 2 et 3 se connectent avec le
# même secret, il n'y a donc qu'un endroit à corriger s'il change.
MDP_A="a-$TAG-$RANDOM"
MDP_B="b-$TAG-$RANDOM"
TMS="$(date +%s)000"
TEMOIN="témoin-de-mémoire-$TAG : mon animal préféré est le loup gris"
# ── DEUX FICHIERS DE COOKIES, ET SURTOUT PAS UN SEUL ───────────────────────
# `$CK_A` porte la session de $ALICE et il est RÉUTILISÉ par les conversations
# des étapes 5, 7bis et 15 : si $BOB s'y connectait, ces trois étapes
# parleraient au nom du mauvais compte — et l'échec apparaîtrait plus loin,
# comme une mémoire perdue. Chaque compte a donc le sien, et ils ne se croisent
# jamais.
CK_A="$RACINE/alice.cookies"
CK_B="$RACINE/bob.cookies"
CK_ADMIN="$RACINE/admin.cookies"
BASE_URL="http://127.0.0.1:$PORT"
SERVEUR=""

echecs=0
non_teste=0
# Contrôles en échec qui portent sur UN SEUL défaut hors de mes fichiers : le
# verrou de relecture (étapes 10 et 15). Comptés à part pour que le verdict dise
# « la chaîne casse ici » plutôt que « douze choses vont mal ».
verrous=0
ok()    { printf '  ✅ %s\n' "$1"; }
ko()    { printf '  ❌ %s\n' "$1"; echecs=$((echecs + 1)); }
skippe(){ printf '  ∅  %s\n' "$1"; non_teste=$((non_teste + 1)); }
info()  { printf '  ·  %s\n' "$1"; }
titre() { printf '\n%s\n' "$1"; }

# ── LE MÉNAGE EST COMMUN AUX DEUX PORTES ────────────────────────────────────
# `kill -9` sans `wait` fait annoncer « Killed » par le shell à la sortie : une
# trace de meurtre à la fin d'un verdict vert, qu'on finit par ignorer. Et les
# DEUX variables de conservation sont honorées (`GARDER_TRACE` pour l'arbre
# complet, `GARDER_JOURNAL` pour le seul journal du serveur), parce que chaque
# lignée en avait une et qu'un relecteur n'a pas à savoir laquelle choisir.
nettoyer() {
    if [ -n "$SERVEUR" ]; then
        kill -9 "$SERVEUR" 2>/dev/null
        wait "$SERVEUR" 2>/dev/null || true
    fi
    if [ "${GARDER_TRACE:-0}" = "1" ]; then
        printf 'trace conservée : %s\n' "$RACINE"
        return
    fi
    if [ "${GARDER_JOURNAL:-0}" = "1" ]; then
        printf 'journal du serveur conservé : %s\n' "$LOG"
        rm -rf "$DATA" "$MONDES" "$IAM" "$CK_A" "$CK_B" "$CK_ADMIN"
        return
    fi
    rm -rf "$RACINE"
}
trap nettoyer EXIT

# ── 0. LES PRÉREQUIS, ET L'ISOLATION VÉRIFIÉE AVANT TOUT ────────────────────
titre "0) prérequis et isolation (aucune base vivante n'est approchée)"
[ -x "$BIN" ] || { echo "  ❌ binaire absent ou non exécutable : $BIN (cargo build --release -p obrain-cli --features ai,iam)" >&2; exit 2; }
case "$DATA" in /tmp/*) ;; *) echo "  ❌ DATA hors /tmp : $DATA" >&2; exit 2 ;; esac
case "$CACHE" in /tmp/*) ;; *) echo "  ❌ cache hors /tmp : $CACHE" >&2; exit 2 ;; esac
mkdir -p "$DATA/selfs" "$CACHE" "$MONDES" || exit 2
ok "binaire : $BIN"
ok "temporaires : $RACINE (DATA, cache de mémoire, mondes, IAM — tous sous /tmp)"
info "port : $PORT (libre, hors 8432/8433/8455)"
[ "$SANS_LLM" = "1" ] && info "mode --sans-llm : la conversation RÉELLE ne sera pas exercée (et ce sera DIT, jamais compté comme réussi)"

"$BIN" init "$STORE" >/dev/null 2>&1 || { echo "  ❌ init du store d'essai" >&2; exit 2; }
"$BIN" init "$IAM" >/dev/null 2>&1 || { echo "  ❌ init du magasin IAM" >&2; exit 2; }
"$BIN" iam amorcer "$IAM" "$ESPACE" --utilisateur admin --mot-de-passe "$MDP_ADMIN" >/dev/null 2>&1 \
    || { echo "  ❌ amorçage IAM (binaire sans la feature iam ?)" >&2; exit 2; }
ok "store d'essai, magasin IAM et compte administrateur en place"

# ── LA BASE DE L'AUTRE COMPTE, ÉCRITE AVANT : C'EST LE TÉMOIN ───────────────
# Elle sert DEUX fois : c'est l'empreinte qui doit rester identique (étape 9), et
# c'est le self « déjà là » dont l'absence de mélange se mesure (étape 3bis).
BASE_BOB="$DATA/selfs/$BOB.obrain"
"$BIN" init "$BASE_BOB" >/dev/null 2>&1 || { echo "  ❌ init base témoin" >&2; exit 2; }
# ── UNE REQUÊTE = UNE INSTRUCTION ──────────────────────────────────────────
# Mesuré en écrivant cette porte : `MERGE … SET … MERGE …` est REFUSÉ
# (« Unexpected token 'MERGE' after the end of the statement ») — la syntaxe du
# moteur n'enchaîne pas deux écritures dans une même requête. Trois appels, donc,
# et les trois doivent réussir : un témoin à moitié écrit ne témoignerait de rien.
"$BIN" query "$BASE_BOB" "MERGE (s:Self {qn: 'self:$BOB'}) SET s.updated_ms = $TMS" >/dev/null 2>&1 \
  && "$BIN" query "$BASE_BOB" "MERGE (n:Note {qn: 'note:temoin-$BOB'}) SET n.text = 'mémoire antérieure de $BOB', n.created_ms = $TMS" >/dev/null 2>&1 \
  && "$BIN" query "$BASE_BOB" "MATCH (s:Self {qn: 'self:$BOB'}), (n:Note {qn: 'note:temoin-$BOB'}) MERGE (s)-[:HAS_NOTE]->(n)" >/dev/null 2>&1 \
  || { echo "  ❌ écriture du témoin dans la base de $BOB" >&2; exit 2; }
EMPREINTE_BOB_AVANT=$(cd "$BASE_BOB" && find . -type f | sort | xargs sha256sum | sha256sum | cut -c1-64)
ok "base témoin de $BOB écrite AVANT (empreinte ${EMPREINTE_BOB_AVANT:0:16}…)"

# ── 1. LE SERVEUR ISOLÉ ─────────────────────────────────────────────────────
titre "1) serveur armé sur $PORT, cache de mémoire ISOLÉ"
OBRAIN_IAM=1 OBRAIN_IAM_ACCOUNT=obrain OBRAIN_IAM_STORE="$IAM" \
OBRAIN_VIZ_CACHE_DIR="$CACHE" OBRAIN_WORLDS_DIR="$MONDES" \
"$BIN" viz serve --host 127.0.0.1 --port "$PORT" \
    --store "$STORE" --enrich-ricci 0 >"$LOG" 2>&1 &
SERVEUR=$!
code="000"
for _ in $(seq 1 60); do
    code=$(curl -s -m 2 -o /dev/null -w '%{http_code}' "$BASE_URL/api/stores" 2>/dev/null)
    [ "$code" != "000" ] && break
    sleep 1
done
if [ "$code" = "000" ]; then
    ko "le serveur n'écoute pas (voir $LOG)"
    sed -n '1,20p' "$LOG"
    exit 2
fi
ok "le serveur écoute ($code sans identité = refus attendu)"
info "cache de mémoire de cette porte : $CACHE"

# ── 2. DEUX COMPTES, CHACUN SA BASE ─────────────────────────────────────────
titre "2) deux comptes, chacun sa base"
curl -s -m 20 -b "$CK_ADMIN" -c "$CK_ADMIN" -o /dev/null -X POST "$BASE_URL/api/auth/login" \
    --data-urlencode "username=admin" --data-urlencode "password=$MDP_ADMIN"
for couple in "$ALICE:$MDP_A" "$BOB:$MDP_B"; do
    nom="${couple%%:*}"; mdp="${couple#*:}"
    r=$(curl -s -m 20 -b "$CK_ADMIN" -c "$CK_ADMIN" -X POST "$BASE_URL/api/iam/utilisateurs" \
        -d "nom=$nom" -d "espace=$ESPACE" -d "mot_de_passe=$mdp")
    if printf '%s' "$r" | grep -q "$nom"; then ok "compte « $nom » créé"
    else ko "création de « $nom » : $(printf '%s' "$r" | head -c 160)"; fi
done
rm -f "$CK_ADMIN"

# ── LA SESSION DE $ALICE, OUVERTE ET VÉRIFIÉE ───────────────────────────────
# `whoami` d'abord : sans cette vérification, un échec de connexion se
# manifesterait plus loin comme un « self absent » — et on chercherait la panne
# dans la mémoire au lieu du cookie.
curl -s -m 20 -c "$CK_A" -o /dev/null -X POST "$BASE_URL/api/auth/login" \
    --data-urlencode "username=$ALICE" --data-urlencode "password=$MDP_A"
qui=$(curl -s -m 20 -b "$CK_A" "$BASE_URL/api/auth/whoami")
printf '%s' "$qui" | grep -q "$ALICE" && ok "session de $ALICE ouverte (whoami)" \
    || ko "whoami inattendu : $(printf '%s' "$qui" | head -c 160)"

# ── 3. LE SELF DE LA CONVERSATION EST CELUI DU COMPTE ───────────────────────
titre "3) la conversation appartient au COMPTE (self:$ALICE, pas self:agent)"
# Un SEUL geste lit la mémoire d'un compte, et il est partagé par toutes les
# étapes qui suivent : deux cookies, deux logins, et la réponse brute rendue à
# l'appelant. Réécrire ce couple `login + GET` à chaque étape serait la façon la
# plus sûre de finir par en oublier un.
memoire_de() { # $1 = nom, $2 = fichier de cookies, $3 = mot de passe
    curl -s -m 20 -c "$2" -o /dev/null -X POST "$BASE_URL/api/auth/login" \
        --data-urlencode "username=$1" --data-urlencode "password=$3"
    curl -s -m 20 -b "$2" "$BASE_URL/api/engine/memoire"
}
MEM=$(memoire_de "$ALICE" "$CK_A" "$MDP_A")
SELF_ANNONCE=$(printf '%s' "$MEM" | sed -n 's/.*"self"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)
JOURNAL=$(printf '%s' "$MEM" | sed -n 's/.*"journal"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)
[ "$SELF_ANNONCE" = "self:$ALICE" ] \
    && ok "le serveur annonce self:$ALICE" \
    || ko "self annoncé : « ${SELF_ANNONCE:-absent} » (attendu self:$ALICE)"
[ -n "$JOURNAL" ] && ok "journal désigné : $(basename "$JOURNAL")" \
    || ko "le serveur ne désigne aucun journal (le nom est un hachage : il ne se devine pas)"
# ── UN REFUS DOIT MONTRER SA RAISON ────────────────────────────────────────
# Sur cette route, un `permission denied: action=graph:read resource=…tenant/…`
# est INDISTINGUABLE d'une absence de self si on n'affiche que le verdict : on
# colle la réponse brute. Un refus muet ressemble à une panne.
case "$SELF_ANNONCE" in
    "self:$ALICE") ;;
    *) info "réponse brute de /api/engine/memoire : $(printf '%s' "$MEM" | head -c 300)" ;;
esac
# La portée dit où en est le self : le nœud Self vit dans la synapse, qui n'est
# pas servie ici — ces selfs neufs sont donc en « journal », ce qui est le
# comportement voulu (le nœud naîtra à l'application de la mémoire).
portee=$(printf '%s' "$MEM" | sed -n 's/.*"portee"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)
[ "$portee" = "journal" ] || [ "$portee" = "compte" ] \
    && ok "portée annoncée : « $portee »" || ko "portée inattendue : « $portee »"

# ── 3bis. DEUX COMPTES, DEUX SELFS, DEUX JOURNAUX ───────────────────────────
# C'est LE point de la demande : « interconnecter chacun sa base ». Deux comptes
# qui partagent un self ne sont pas deux comptes. On exige donc que les deux
# selfs soient DISTINCTS — et que la mémoire de l'un ne soit pas écrite dans le
# fichier de l'autre, ce qui se lit sur le JOURNAL désigné pour chacun.
titre "3bis) deux selfs DISTINCTS, deux journaux distincts (aucun mélange)"
MEM_B=$(memoire_de "$BOB" "$CK_B" "$MDP_B")
self_b=$(printf '%s' "$MEM_B" | sed -n 's/.*"self"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)
jrn_b=$(printf '%s' "$MEM_B" | sed -n 's/.*"journal"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)
[ "$self_b" = "self:$BOB" ] && ok "$BOB → « $self_b »" \
    || ko "$BOB → « $self_b » (attendu self:$BOB)"
[ -n "$SELF_ANNONCE" ] && [ "$SELF_ANNONCE" != "$self_b" ] \
    && ok "les deux selfs sont DISTINCTS (aucun mélange)" \
    || ko "les deux comptes partagent le même self : « $SELF_ANNONCE »"
[ -n "$JOURNAL" ] && [ -n "$jrn_b" ] && [ "$jrn_b" != "$JOURNAL" ] \
    && ok "les journaux de mémoire sont deux FICHIERS distincts ($(basename "$jrn_b"))" \
    || ko "même journal pour les deux comptes : « ${jrn_b:-$JOURNAL} » — deux mémoires se mélangeraient"
# Le journal de $BOB, redemandé par le CLI : la désignation ne doit pas dépendre
# du chemin par lequel on la demande (route HTTP ou sous-commande `memoire`).
if [ -n "$JOURNAL" ]; then
    JOURNAL_BOB=$(OBRAIN_VIZ_CACHE_DIR="$CACHE" "$BIN" memoire --self "self:$BOB" --format json \
        | sed -n 's/.*"journal"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)
    [ -n "$JOURNAL_BOB" ] && [ "$JOURNAL_BOB" != "$JOURNAL" ] \
        && ok "le CLI désigne le même AUTRE fichier pour $BOB ($(basename "$JOURNAL_BOB"))" \
        || ko "le journal de $BOB vu par le CLI est « ${JOURNAL_BOB:-absent} » — la désignation dépend du chemin"
fi

# ── 4. SANS IDENTITÉ, LA ROUTE REFUSE ──────────────────────────────────────
titre "4) sans identité, la route REFUSE (et ne rend le self de personne)"
# ── ERREUR DE MA PREMIÈRE VERSION, GARDÉE ICI PARCE QU'ELLE EST INSTRUCTIVE ──
# J'exigeais qu'un appel ANONYME rende le self de l'instance. C'est faux : quand
# l'IAM est armé, toute route `/api/*` exige une identité — y compris celle-ci.
# Le contrôle correct n'est donc pas « l'anonyme voit le self commun » mais
# « l'anonyme ne voit RIEN, et surtout pas le self d'un compte ». Une porte qui
# exige le contraire pousse à ouvrir une route pour la faire passer — le
# réflexe exact qu'il faut éviter.
ANON="$RACINE/anonyme.json"
code=$(curl -s -m 20 -o "$ANON" -w '%{http_code}' "$BASE_URL/api/engine/memoire")
SANS=$(cat "$ANON" 2>/dev/null)
rm -f "$ANON"
if [ "$code" = "401" ]; then
    ok "appel sans identité → 401 (l'IAM ne laisse rien passer)"
else
    ko "appel sans identité → $code (401 attendu quand l'IAM est armé)"
fi
if printf '%s' "$SANS" | grep -q "$SELF_ANNONCE\|$self_b"; then
    ko "l'appel anonyme expose le self d'un compte !"
else
    ok "aucun self de compte n'est exposé sans identité"
fi

# ── 4bis. LE SELF DE L'INSTANCE RESTE INTACT ───────────────────────────────
# Le self de l'INSTANCE, lui, doit rester intact : on le lit par le CLI, sur la
# base synaptique réelle — c'est ce que l'agent de l'utilisateur porte encore.
titre "4bis) le self de l'instance reste intact (lu par le CLI)"
SYNAPSE="${OBRAIN_SYNAPSE:-/home/triviere/.local/share/obrain-harness/data/agent-synapse-triviere-dsh.obrain}"
if [ -f "$SYNAPSE" ]; then
    SELF_INSTANCE="$("$BIN" query --read-only --format json "$SYNAPSE" \
        "MATCH (s:Self) RETURN s.qn AS qn LIMIT 1" 2>/dev/null \
        | sed -n 's/.*"qn"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)"
    [ -n "$SELF_INSTANCE" ] && ok "le self de l'instance reste « $SELF_INSTANCE » (intact)" \
        || ko "le self de l'instance est introuvable dans la synapse"
else
    printf '  (ignoré) base synaptique absente : %s\n' "$SYNAPSE"
fi

# ── 5. LA CONVERSATION JOURNALISE ───────────────────────────────────────────
titre "5) la conversation journalise une intention pour ce compte"
if [ -z "$JOURNAL" ]; then
    ko "sans journal désigné, la journalisation ne peut pas être vérifiée"
else
    mkdir -p "$(dirname "$JOURNAL")"
    if [ "$SANS_LLM" = "1" ]; then
        skippe "conversation RÉELLE non testée (--sans-llm) : entrée écrite au format exact du journal"
        # ── LE FORMAT EST CELUI DE `memoire::Entree::vers_json` ────────────
        # Clés TRIÉES (serde_json) : la passe consomme le journal en comparant
        # chaque ligne à sa re-sérialisation. Une ligne valide mais NON
        # canonique est appliquée… et laissée en place, donc rejouée à chaque
        # passe (mesuré : un `etat-self` non canonique a produit v1 puis v2 sur
        # deux passes successives). On écrit donc exactement ce que le chat écrit.
        printf '{"intention":{"kind":"note","sujet":"acceptation","texte":"%s"},"provenance":"acceptation-self-par-compte","self":"self:%s","t":%s}\n' \
            "$TEMOIN" "$ALICE" "$TMS" >> "$JOURNAL"
        ok "entrée journalisée pour self:$ALICE"
    else
        LLM_URL="${OBRAIN_LLM_URL:-http://192.168.1.7:9910}"
        llm=$(curl -s -m 5 -o /dev/null -w '%{http_code}' "$LLM_URL/v1/models" 2>/dev/null)
        info "modèle interrogé : $LLM_URL (HTTP $llm)"
        Q="Retiens ceci dans ta mémoire pour de bon, avec l'outil obrain_memoriser : $TEMOIN. Utilise le sujet « acceptation » et le texte exact donné."
        # `store` est transmis : la conversation est ancrée sur la base d'essai,
        # donc le self résolu est celui du compte ET la base visée est celle que
        # la passe relira — c'est l'étape 15 qui en dépend.
        curl -s -N -m 300 -b "$CK_A" -G "$BASE_URL/api/engine/chat" \
            --data-urlencode "q=$Q" --data-urlencode "store=$STORE" \
            --data-urlencode "pensee=courte" --data-urlencode "max_tokens=1200" >"$CHAT" 2>&1
        if [ -f "$JOURNAL" ] && grep -q "\"self\":\"self:$ALICE\"" "$JOURNAL"; then
            ok "le modèle a appelé l'outil, la conversation a journalisé"
        elif [ -f "$JOURNAL" ] && grep -q "self:$ALICE" "$JOURNAL"; then
            ok "la conversation a journalisé pour self:$ALICE"
        else
            ko "aucune entrée de journal après une conversation réelle — voir $CHAT"
            printf '     fin du flux : %s\n' "$(tail -c 300 "$CHAT" | tr '\n' ' ')"
        fi
    fi
fi

# ── 6. LA PASSE D'APPLICATION — LE VRAI SCRIPT, ISOLÉ ───────────────────────
titre "6) la passe d'application (scripts/memoire-appliquer-comptes.sh, isolée)"
PASSE1=$(OBRAIN_HARNESS_DATA="$DATA" OBRAIN_VIZ_CACHE_DIR="$CACHE" OBRAIN_BIN="$BIN" \
    bash scripts/memoire-appliquer-comptes.sh 2>&1)
CODE_PASSE1=$?
printf '%s\n' "$PASSE1" | sed 's/^/     /'
[ "$CODE_PASSE1" = "0" ] && ok "la passe sort en 0" || ko "la passe sort en $CODE_PASSE1"
printf '%s' "$PASSE1" | grep -q "self:$ALICE" \
    && ok "la passe a traité le compte $ALICE (il l'a trouvé par le JOURNAL)" \
    || ko "la passe n'a pas traité $ALICE — le compte n'a pas été découvert"

# ── 6bis. LE TÉMOIN : LA MÉMOIRE LAISSE UNE TRACE QUI SURVIT ────────────────
# Le journal est CONSOMMÉ (c'est voulu : `SelfState` est versionné). Sans le
# témoin, une mémoire appliquée ne laisse donc AUCUNE trace datée, et le seul
# moyen de dire « le self se développe » était une absence de fichier — ce que
# le rapport du 16/09 a dû démonter en six cents lignes. On exige donc la trace.
titre "6bis) le témoin : la mémoire laisse une trace datée après consommation"
TEMOINS="$CACHE/memoire-temoins.jsonl"
BILAN_TEMOIN=$(python3 - "$TEMOINS" "$ALICE" <<'PY'
import sys, json, os
chemin, compte = sys.argv[1], "self:" + sys.argv[2]
j = a = 0
if os.path.exists(chemin):
    for ligne in open(chemin, encoding="utf-8", errors="ignore"):
        try:
            v = json.loads(ligne)
        except Exception:
            continue
        if v.get("self") != compte:
            continue
        if v.get("geste") == "journalise":
            j += 1
        elif v.get("geste") == "applique":
            a += 1
print("%d %d" % (j, a))
PY
)
NJ=$(printf '%s' "$BILAN_TEMOIN" | cut -d' ' -f1)
NA=$(printf '%s' "$BILAN_TEMOIN" | cut -d' ' -f2)
if [ ! -f "$TEMOINS" ]; then
    ko "aucun témoin ($TEMOINS) : la mémoire a beau être appliquée, RIEN ne dit quand elle l'a été — le binaire testé est-il antérieur au témoin (memoire.rs) ?"
else
    ok "le témoin existe : $(basename "$TEMOINS")"
    [ "${NA:-0}" -ge 1 ] \
        && ok "l'application est TÉMOIGNÉE : $NA ligne(s) « applique » pour self:$ALICE" \
        || ko "l'application de self:$ALICE n'est pas témoignée (témoin présent mais muet sur ce compte)"
    if [ "$SANS_LLM" = "1" ]; then
        skippe "journalisation non témoignée testable sans conversation (--sans-llm : l'entrée est écrite à la main)"
    else
        [ "${NJ:-0}" -ge 1 ] \
            && ok "l'appel de l'outil est COMPTÉ : $NJ journalisation(s) pour self:$ALICE" \
            || ko "la conversation a journalisé mais le témoin ne compte aucune journalisation — la mesure du « self qui se développe » manque son point de départ"
    fi
fi

# ── 7. LE NŒUD EST DANS LA BASE DE CE COMPTE ────────────────────────────────
titre "7) le nœud est dans la base DU COMPTE, rattaché à son Self"
BASE_ALICE="$DATA/selfs/$ALICE.obrain"
if [ -d "$BASE_ALICE" ]; then
    ok "la base du compte a été créée par la passe : $(basename "$BASE_ALICE")"
    LIAISON=$("$BIN" query --read-only "$BASE_ALICE" \
        "MATCH (s:Self)-[:HAS_NOTE]->(n:Note) RETURN s.qn AS self, n.qn AS note, n.text AS texte" 2>/dev/null)
    printf '%s' "$LIAISON" | grep -q "self:$ALICE" \
        && ok "self:$ALICE -[:HAS_NOTE]-> :Note présent" \
        || ko "aucune arête HAS_NOTE depuis self:$ALICE : $(printf '%s' "$LIAISON" | tr -d '\n' | head -c 200)"
    printf '%s' "$LIAISON" | grep -q "loup gris" \
        && ok "le texte témoin de la conversation est DANS la base du compte" \
        || ko "le texte témoin est absent (contenu : $(printf '%s' "$LIAISON" | tr -d '\n' | head -c 200))"
else
    ko "la base de $ALICE n'existe pas : rien n'a été appliqué"
fi

# ── 7bis. LA BOUCLE SE REFERME : LA MÉMOIRE EST RELUE ───────────────────────
# Écrire ne suffit pas : une base qui n'est jamais relue ne développe rien. Le
# contexte de conversation injecte le profil du compte (`engine.rs`,
# `profil_du_self`, appelé par `contexte_du_monde`) — et il le DIT sur sa sortie
# d'erreur : `[self] profil injecte : N ligne(s) pour self:<compte>`. On tient
# donc la preuve par le journal du serveur, pas par une impression.
titre "7bis) la mémoire du compte est RELUE dans la conversation suivante"
if [ "$SANS_LLM" = "1" ]; then
    skippe "relecture non testée (--sans-llm) : elle n'a lieu que dans une conversation"
else
    curl -s -N -m 300 -b "$CK_A" -G "$BASE_URL/api/engine/chat" \
        --data-urlencode "q=Que sais-tu de moi ? Réponds en une phrase, sans outil." \
        --data-urlencode "store=$STORE" \
        --data-urlencode "pensee=non" --data-urlencode "max_tokens=300" >>"$CHAT" 2>&1
    INJECTE=$(grep -o "profil injecte : [0-9]* ligne(s) pour self:$ALICE" "$LOG" | tail -1)
    if [ -n "$INJECTE" ]; then
        ok "le serveur a relu la base du compte : $INJECTE"
    else
        ko "aucun « profil injecte » dans le journal du serveur pour self:$ALICE — la base grandit mais n'est pas relue"
    fi
fi

# ── 8. LE JOURNAL EST CONSOMMÉ ──────────────────────────────────────────────
# ── CE QUE CETTE ASSERTION DISAIT, ET POURQUOI ELLE A CHANGÉ (2026-09-16) ───
# Sa version précédente comptait les lignes de `self:$ALICE` restant au journal
# et déclarait « elles seront rejouées » dès qu'il en restait une. Or la
# conversation de l'étape 7bis peut LÉGITIMEMENT journaliser une intention
# NEUVE (le modèle a de nouveau appelé l'outil — c'est arrivé deux fois sur
# trois passages). L'assertion condamnait donc un rejeu inexistant, et le
# message envoyait chercher un défaut de consommation là où il n'y en avait pas.
# Ce qu'elle doit exiger, c'est précisément ceci : l'entrée APPLIQUÉE (celle du
# témoin) ne doit plus être au journal. Une entrée NEUVE reste — et se dit.
titre "8) le journal est consommé (l'intention appliquée n'est pas rejouée)"
if [ -n "$JOURNAL" ] && [ -f "$JOURNAL" ]; then
    if grep -q "$TEMOIN" "$JOURNAL" 2>/dev/null; then
        ko "l'entrée APPLIQUÉE est encore au journal : elle sera rejouée (contenu : $(grep -c "self:$ALICE" "$JOURNAL") ligne(s) pour ce compte)"
    elif grep -q "self:$ALICE" "$JOURNAL" 2>/dev/null; then
        ok "l'entrée appliquée a été retirée ; ce qui reste est NEUF (la conversation de 7bis a journalisé à nouveau) — pas un rejeu"
    else
        ok "le fichier subsiste mais plus rien n'attend pour $ALICE"
    fi
else
    ok "le journal a été supprimé : tout ce qui était en attente a été appliqué"
fi

# ── 9. LA BASE DE L'AUTRE COMPTE EST INTACTE ────────────────────────────────
titre "9) la base de l'AUTRE compte ($BOB) est intacte"
EMPREINTE_BOB_APRES=$(cd "$BASE_BOB" && find . -type f | sort | xargs sha256sum | sha256sum | cut -c1-64)
if [ "$EMPREINTE_BOB_AVANT" = "$EMPREINTE_BOB_APRES" ]; then
    ok "empreinte identique (${EMPREINTE_BOB_APRES:0:16}…) : un octet n'a pas bougé"
else
    ko "la base de $BOB a CHANGÉ : ${EMPREINTE_BOB_AVANT:0:16}… → ${EMPREINTE_BOB_APRES:0:16}…"
fi
TEMOIN_BOB=$("$BIN" query --read-only "$BASE_BOB" \
    "MATCH (n:Note) RETURN n.text AS texte" 2>/dev/null)
printf '%s' "$TEMOIN_BOB" | grep -q "mémoire antérieure de $BOB" \
    && ok "le témoin de $BOB est toujours lisible" \
    || ko "le témoin de $BOB a disparu : $(printf '%s' "$TEMOIN_BOB" | tr -d '\n' | head -c 160)"
printf '%s' "$TEMOIN_BOB" | grep -q "loup gris" \
    && ko "la mémoire de $ALICE a fui dans la base de $BOB" \
    || ok "aucune trace de la mémoire de $ALICE dans la base de $BOB"
if [ -d "$DATA/selfs/$BOB.obrain" ]; then
    ok "la passe n'a PAS créé de base pour $BOB (rien ne l'attendait)"
fi

# ── 10. LA PASSE N'EST PAS MUETTE QUAND IL N'Y A RIEN ───────────────────────
titre "10) la passe DIT ce qu'elle a regardé quand il n'y a rien à appliquer"
PASSE2=$(OBRAIN_HARNESS_DATA="$DATA" OBRAIN_VIZ_CACHE_DIR="$CACHE" OBRAIN_BIN="$BIN" \
    bash scripts/memoire-appliquer-comptes.sh 2>&1)
CODE_PASSE2=$?
printf '%s\n' "$PASSE2" | sed 's/^/     /'
[ -n "$PASSE2" ] \
    && ok "elle parle (une ligne) au lieu de sortir en silence" \
    || ko "elle est MUETTE : journal vide et passe cassée se ressembleraient encore"
printf '%s' "$PASSE2" | grep -q "$CACHE" \
    && ok "elle NOMME le journal qu'elle a regardé" \
    || ko "elle ne dit pas quel journal elle a regardé"
if [ "$CODE_PASSE2" = "0" ]; then
    ok "rien à faire n'est pas une erreur (code 0)"
elif printf '%s' "$PASSE2" | grep -q 'already open'; then
    verrous=$((verrous + 1))
    ko "la mémoire EN ATTENTE n'a pas pu être appliquée : la base du compte est TENUE par un autre processus (verrou exclusif refusé). « La base grandit » s'arrête donc dès qu'une conversation a relu cette base — mesuré par l'étape 15"
else
    ko "code $CODE_PASSE2 inattendu quand il n'y a rien à faire : $(printf '%s' "$PASSE2" | tr '\n' ' ' | head -c 200)"
fi

# ── 11. LA PASSE REFUSE (ET LE DIT) QUAND ELLE NE PEUT PAS TRAVAILLER ───────
titre "11) une panne ne prend pas la forme du repos"
PASSE3=$(OBRAIN_HARNESS_DATA="$DATA" OBRAIN_VIZ_CACHE_DIR="$CACHE" OBRAIN_BIN=/inexistant/obrain \
    bash scripts/memoire-appliquer-comptes.sh 2>&1)
CODE_PASSE3=$?
[ "$CODE_PASSE3" != "0" ] \
    && ok "binaire absent → code $CODE_PASSE3 (et non un « 0 en attente »)" \
    || ko "binaire absent → code 0 : la panne la plus grave prend la forme du repos"
printf '%s' "$PASSE3" | grep -q "INTROUVABLE" \
    && ok "la raison est écrite : $(printf '%s' "$PASSE3" | head -1)" \
    || ko "aucune raison affichée : $(printf '%s' "$PASSE3" | head -c 160)"

# ── 12. REPOS ET CHAÎNE CASSÉE NE S'ÉCRIVENT PAS PAREIL ─────────────────────
# C'est LA confusion qui a duré jusqu'au 16/09 : « 0 compte en attente » se
# lisait aussi bien « la chaîne est au repos » que « la chaîne est morte ». Une
# porte qui ne sait pas distinguer les deux ne garde rien. On exige donc les
# DEUX formulations, et on les exige distinctes.
titre "12) « aucun compte n'a rien à appliquer » ≠ « la chaîne est cassée »"
printf '%s' "$PASSE2" | grep -q 'journalisation(s)' \
    && ok "chaîne VIVANTE : la passe date le dernier appel à l'outil ($(printf '%s' "$PASSE2" | grep -o '[0-9]* journalisation(s)' | tail -1))" \
    || ko "la passe ne date pas le dernier appel : repos et panne restent indiscernables — sortie : $(printf '%s' "$PASSE2" | tr '\n' ' ' | head -c 200)"
R_TEMOIN="$RACINE/sans-temoin"
mkdir -p "$R_TEMOIN/cache/memoire"
PASSE_SANS=$(OBRAIN_HARNESS_DATA="$DATA" OBRAIN_VIZ_CACHE_DIR="$R_TEMOIN/cache" OBRAIN_BIN="$BIN" \
    bash scripts/memoire-appliquer-comptes.sh 2>&1)
printf '%s' "$PASSE_SANS" | grep -q 'ABSENT' \
    && ok "chaîne SANS trace de production : la passe le dit au lieu de le taire" \
    || ko "sans témoin, la passe ne dit rien de l'absence de production : $(printf '%s' "$PASSE_SANS" | tr '\n' ' ' | head -c 200)"
printf '%s' "$PASSE_SANS" | grep -q 'PAS un repos' \
    && ok "elle refuse explicitement d'appeler « repos » une chaîne dont on n'a aucune trace" \
    || ko "elle présente une absence de trace comme un repos — c'est exactement le défaut du 16/09"

# ── 13. LA BASE ÉCRITE EST CELLE QU'UNE CONVERSATION RELIRA (DÉFAUT 5.3) ────
# `engine.rs::profil_du_self` relit `dirname(store ancré)/selfs/<compte>.obrain`
# et rend `None` SANS LE DIRE. Une conversation ancrée sur un monde rangé
# ailleurs écrit donc sa mémoire là où personne ne la lira. Cette porte exige
# que la passe le DISE — et qu'elle applique quand même, parce qu'une intention
# ne se jette pas.
titre "13) la mémoire est écrite là où la conversation la relit (défaut 5.3)"
R_53="$RACINE/defaut53"
mkdir -p "$R_53/data" "$R_53/cache/memoire" "$R_53/mondes"
if "$BIN" init "$R_53/mondes/monde-ailleurs.obrain" >/dev/null 2>&1; then
    J53=$(OBRAIN_VIZ_CACHE_DIR="$R_53/cache" "$BIN" memoire --self self:essai53 --format json \
        | sed -n 's/.*"journal"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
    mkdir -p "$(dirname "$J53")"
    # ── L'ENTRÉE EST ÉCRITE EN DEUX FOIS, ET PAS PAR UNE CONTINUATION ──────
    # Défaut trouvé en écrivant ce bloc : une suite `\` + retour à la ligne
    # DANS une chaîne entre quotes simples n'est PAS une continuation — bash
    # écrit le backslash et la coupure, la ligne JSON est coupée en deux, donc
    # illisible, donc « 0 en attente », donc le cas n'était jamais éprouvé et
    # les trois assertions échouaient pour une raison qui n'avait rien à voir
    # avec le défaut 5.3 qu'elles mesurent.
    {
        printf '{"intention":{"kind":"note","sujet":"essai","texte":"memoire essai53"},'
        printf '"provenance":"monde-ailleurs · %s","self":"self:essai53","t":%s}\n' \
            "$R_53/mondes/monde-ailleurs.obrain" "$TMS"
    } >"$J53"
    PASSE53=$(OBRAIN_HARNESS_DATA="$R_53/data" OBRAIN_VIZ_CACHE_DIR="$R_53/cache" OBRAIN_BIN="$BIN" \
        bash scripts/memoire-appliquer-comptes.sh 2>&1)
    CODE53=$?
    printf '%s' "$PASSE53" | grep -q 'DIVERGENCE' \
        && ok "la passe DIT que la conversation relira $R_53/mondes/selfs/essai53.obrain et non la base écrite" \
        || ko "divergence d'ancrage non signalée : la mémoire serait appliquée là où personne ne la relit — $(printf '%s' "$PASSE53" | tr '\n' ' ' | head -c 200)"
    [ "$CODE53" != "0" ] \
        && ok "elle sort en $CODE53 : un écart qui se répète toutes les 2 minutes se voit" \
        || ko "elle sort en 0 malgré la divergence : le silence ressemblerait à une panne"
    [ -d "$R_53/data/selfs/essai53.obrain" ] \
        && ok "l'intention a été appliquée QUAND MÊME (on ne jette pas une mémoire)" \
        || ko "l'intention a été jetée avec la divergence — une intention ne se perd pas"
else
    ko "impossible de préparer le cas du défaut 5.3 (init d'un store hors `data/`)"
fi

# ── 14. UN AVERTISSEMENT DU MOTEUR N'EST PAS UN REFUS (DÉFAUT 5.4) ──────────
# Le substrat écrit sur stderr, à chaque ouverture, un « post-replay anchor
# audit … husk signature … label_only=1 » — y compris sur une base NEUVE de
# 0 nœud. Lu dans le journal du timer, il ressemble à l'annonce d'une perte de
# propriétés. Défaut mesuré : une version de la passe jugeait « il y a des
# octets sur stderr ⇒ application refusée » et déclarait refusée une passe qui
# avait réussi. On exige donc que stderr soit RELAYÉ, jamais interprété.
titre "14) un avertissement du moteur n'est pas un refus (défaut 5.4)"
printf '%s' "$PASSE1" | grep -q 'ligne(s) de moteur relayée(s)' \
    && ok "la passe relaie ce que dit le moteur, et le compte" \
    || ko "la passe ne dit pas ce qu'elle a relayé du moteur"
printf '%s' "$PASSE1" | grep -q "n'est PAS un refus" \
    && ok "elle dit une fois qu'un WARN n'est pas un verdict (le verdict est le code de sortie)" \
    || ko "rien ne distingue un avertissement d'un refus : c'est le défaut 5.4 tel quel"
NEUVE="$RACINE/neuve.obrain"
if "$BIN" init "$NEUVE" >/dev/null 2>&1; then
    ERR_NEUVE="$RACINE/neuve.err"
    TOT_NEUVE=$("$BIN" query --read-only "$NEUVE" "MATCH (n) RETURN count(*) AS total" 2>"$ERR_NEUVE" \
        | tr -d ' \n' | sed -n 's/.*"total":"\([0-9]*\)".*/\1/p')
    if grep -q 'husk signature' "$ERR_NEUVE"; then
        [ "$TOT_NEUVE" = "0" ] \
            && ok "le husk WARN est émis sur une base NEUVE de $TOT_NEUVE nœud : ce n'est donc PAS une perte de mémoire" \
            || ko "le husk WARN accompagne $TOT_NEUVE nœud(s) : la qualification du défaut 5.4 est à revoir"
    else
        skippe "le husk WARN n'est pas émis par ce binaire (défaut 5.4 peut avoir été corrigé côté substrat) : l'assertion n'a pas pu être exercée"
    fi
else
    ko "impossible de créer une base neuve pour éprouver le défaut 5.4"
fi

# ── 15. APRÈS UNE RELECTURE, LA PASSE PEUT-ELLE ENCORE APPLIQUER ? ──────────
# Le scénario RÉEL de production, que cette porte ne jouait pas : le compte
# converse (le service relit sa base par `engine.rs::profil_du_self`), puis une
# NOUVELLE mémoire arrive et le timer doit l'appliquer. Or une base ouverte en
# lecture prend un `flock` PARTAGÉ et l'écriture exige l'EXCLUSIF (invariant
# mono-écrivain, `obrain-substrate/src/file.rs`) : si le service garde la base du
# compte ouverte, la passe ne peut plus jamais y écrire, et la base du compte
# cesse de grandir AUSSI LONGTEMPS QUE LE SERVICE VIT. C'est en contradiction
# directe avec l'objectif du chantier, et ça ne se voyait pas : la première
# version de cette porte n'écrivait qu'UNE intention, donc le second passage
# n'avait rien à appliquer et le verrou n'était jamais rencontré.
titre "15) après une relecture par la conversation, la passe peut-elle ENCORE appliquer ?"
if [ -z "$JOURNAL" ]; then
    ko "sans journal désigné, la seconde application ne peut pas être éprouvée"
else
    mkdir -p "$(dirname "$JOURNAL")"
    {
        printf '{"intention":{"kind":"note","sujet":"acceptation","texte":"%s (seconde)"}' "$TEMOIN"
        printf ',"provenance":"acceptation-self-par-compte","self":"self:%s","t":%s}\n' \
            "$ALICE" "$((TMS + 1000))"
    } >>"$JOURNAL"
    PASSE4=$(OBRAIN_HARNESS_DATA="$DATA" OBRAIN_VIZ_CACHE_DIR="$CACHE" OBRAIN_BIN="$BIN" \
        bash scripts/memoire-appliquer-comptes.sh 2>&1)
    CODE4=$?
    printf '%s\n' "$PASSE4" | sed 's/^/     /'
    if printf '%s' "$PASSE4" | grep -q "self:$ALICE : 1 appliquee"; then
        ok "la seconde mémoire du compte est appliquée : la base peut grandir service allumé"
    elif printf '%s' "$PASSE4" | grep -q 'already open'; then
        verrous=$((verrous + 1))
        ko "REFUS PAR VERROU : la base du compte est tenue en LECTURE par un autre processus (le service, mesuré : 10 descripteurs ouverts sur la base) ; la mémoire reste au journal et la base NE GRANDIT PAS tant qu'il tourne — la relecture bloque la croissance de la base qu'elle relit. Détenteur hors de mes fichiers : engine.rs / viz_serve.rs / obrain-substrate"
    else
        ko "la seconde application n'a pas eu lieu (code $CODE4) : $(printf '%s' "$PASSE4" | tr '\n' ' ' | head -c 200)"
    fi
    if [ -d "$BASE_ALICE" ]; then
        N4=$("$BIN" query --read-only "$BASE_ALICE" \
            "MATCH (:Self)-[:HAS_NOTE]->(n:Note) RETURN count(n) AS n" 2>/dev/null \
            | tr -d ' \n' | sed -n 's/.*"n":"\([0-9]*\)".*/\1/p')
        [ "${N4:-0}" -ge 2 ] \
            && ok "la base du compte porte $N4 note(s) : elle a RÉELLEMENT grandi" \
            || ko "la base du compte ne porte que ${N4:-?} note(s) : la seconde mémoire n'est jamais arrivée"
    fi
fi

# ── VERDICT ─────────────────────────────────────────────────────────────────
# Trois sorties, comme les deux portes d'origine réunies : tout est vert ; vert
# AVEC des assertions non testées (le mode `--sans-llm`, qui doit le dire au lieu
# de faire passer une porte partielle pour complète) ; et l'échec, qui qualifie à
# part les refus par verrou — un seul défaut, hors de ces fichiers.
titre "verdict"
if [ "$echecs" = "0" ] && [ "$non_teste" = "0" ]; then
    printf '  ✅ chaque compte a SON self et SA base : conversation → journal → passe → base DU COMPTE\n'
    exit 0
fi
if [ "$echecs" = "0" ]; then
    printf '  ⚠  chaîne journal → passe → base DU COMPTE prouvée ; %d assertion(s) NON TESTÉE(S) :\n' "$non_teste"
    printf "     · la conversation réelle (l'outil appelé par le modèle) ;\n"
    printf '     · la relecture de la base du compte dans la conversation suivante.\n'
    printf "     Ces deux-là n'existent que dans une conversation : rejouer sans --sans-llm.\n"
    exit 0
fi
printf '  ❌ %d contrôle(s) en échec\n' "$echecs"
if [ "$verrous" -gt 0 ]; then
    printf '     dont %d sur UN SEUL défaut, et il est réel : la base du compte reste ouverte\n' "$verrous"
    printf '     en LECTURE par le service après que la conversation l'"'"'a relue (verrou exclusif\n'
    printf '     refusé) — la base du compte ne peut plus grandir tant que le service vit.\n'
    printf '     Défaut hors des fichiers de ce chantier : engine.rs / viz_serve.rs / obrain-substrate.\n'
fi
[ "${GARDER_TRACE:-0}" = "1" ] && printf '  trace : %s\n' "$RACINE"
[ "${GARDER_JOURNAL:-0}" = "1" ] && printf '  journal du serveur : %s\n' "$LOG"
exit 1
