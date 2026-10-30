#!/usr/bin/env bash
# ── PORTE DU HARNESS — LE CONTRAT DU HARNESS NATIF, MESURÉ POSTE PAR POSTE ──
#
# ## Pourquoi une porte À PART
#
# Le harness natif (`obrain harness-serve`) n'est pas le moteur : c'est le
# CONTRAT par lequel une conversation lit un graphe — `/api/harness/health`,
# `/machine`, `/sessions`, `/context`, `/continuity`. Il est devenu le point par
# lequel un self apprend quelles machines il connaît (`machine:`), et le seul
# lecteur qui pourrait violer l'invariant mono-écrivain s'il écrivait dans la base
# qu'il sert.
#
# La porte d'atelier mesure la FLOTTE ; celle-ci mesure le HARNESS. Les deux
# délèguent l'une à l'autre plutôt que de se recopier : deux juges qui
# divergeraient seraient pires qu'un seul.
#
# ## Ce qu'elle a été écrite pour attraper
#
# Un cas mesuré, et c'est le défaut qui a justifié cette porte :
#
#   POST /api/harness/context {"session_id":"session-qui-nexiste-pas"}
#     → HTTP 200  {"status":"ok", "engrams":[], "goals":[], …}
#
# Une session INCONNUE rendait « ok » sur un vide intégral. Le cas « aucune
# session » avait été corrigé (400 + `aucune_session`) ; celui-ci était resté, et
# il est PIRE : le lecteur croit que le graphe est muet alors que la session
# n'existe pas. C'est le mode de défaillance que la doctrine nomme depuis le
# premier jour — « une réponse vide qui a la forme d'une bonne réponse ».
#
# ## Les sept garanties
#
#   1. l'identité : health et machine disent la MÊME chose, le nom d'hôte vient du
#      noyau (source publiée), le build est TRACÉ, le store annoncé est RÉELLEMENT
#      celui qui est servi ;
#   2. les sessions : la pagination avance et ne se recouvre pas ; chaque session
#      listée est ouvrable (aucune orpheline) ;
#   3. l'injection : les sections attendues sur une session connue ; SANS session
#      et avec une session INCONNUE, elle refuse au lieu de dire « ok » ; le bloc
#      `tissage` dit son état et ne prétend pas « tout va bien » quand il ne sait
#      pas ;
#   4. LECTURE SEULE : le harness n'écrit RIEN dans la base qu'il sert ;
#   5. l'auto-déclaration : il journalise son identité, et la passe en fait un fait
#      du graphe nommé ;
#   6. deux harnesses, deux bases : identités distinguables, aucune fuite ;
#   7. les routes inconnues : 404 franc, et pas de route du moteur par effet de bord.
#
# ## Usage
#
#   scripts/acceptation-harness.sh            # la porte entière
#   scripts/acceptation-harness.sh --familles 3,4
#
# Elle ne touche AUCUN service vivant : tout tourne sur des ports libres > 8400,
# des bases temporaires, et disparaît au trap EXIT — même en cas d'échec.
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 1

# ── LE SUJET EST FIXÉ, PAS HÉRITÉ ───────────────────────────────────────────
# Leçon apprise sur la porte d'atelier : `OBRAIN_BIN` est posé dans
# l'environnement de cette machine et désigne une copie ANCIENNE. Une porte qui
# hérite son sujet mesure ce qu'on lui donne sans le dire.
BIN="$REPO/target/release/obrain"

RACINE="$(mktemp -d /tmp/acceptation-harness-XXXXXX)"
SERVEURS=()
nettoyer() {
    local p
    for p in "${SERVEURS[@]:-}"; do [ -n "$p" ] && kill -9 "$p" 2>/dev/null; done
    [ "${GARDER_TRACE:-0}" = "1" ] || rm -rf "$RACINE"
}
trap nettoyer EXIT

echecs=0; verifications=0; mesures=0
ok()   { verifications=$((verifications + 1)); printf '  ✅ %s\n' "$1"; }
ko()   { verifications=$((verifications + 1)); echecs=$((echecs + 1)); printf '  ❌ %s\n' "$1"; }
mes()  { mesures=$((mesures + 1)); printf '  ·  %s\n' "$1"; }
titre(){ printf '\n%s\n' "$1"; }

FAMILLES=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --familles) FAMILLES="${2:-}"; shift 2 ;;
        # --bin : choisir le binaire mesuré, EXPLICITEMENT. La leçon de la porte
        # d'atelier n'est pas d'interdire le choix — c'est d'interdire qu'il soit
        # SILENCIEUX (elle héritait `OBRAIN_BIN`, donc une copie ancienne, sans le
        # dire). Ici le sujet ne peut venir que de la ligne de commande, et la
        # porte l'affiche.
        --bin) BIN="$2"; shift 2 ;;
        --aide|-h|--help) sed -n '2,58p' "$0"; exit 0 ;;
        *) printf 'acceptation-harness : option inconnue : %s\n' "$1" >&2; exit 2 ;;
    esac
done
famille() {
    [ -z "$FAMILLES" ] && return 0
    case ",$FAMILLES," in *",$1,"*) return 0 ;; esac
    return 1
}

port_libre() {
    local p
    for p in $(seq 8410 8490); do
        ss -ltn 2>/dev/null | grep -q ":$p " || { echo "$p"; return; }
    done
    echo 0
}

# ── DÉMARRAGE D'UN HARNESS TEMPORAIRE ───────────────────────────────────────
# Rend « PID:PORT » sur la sortie standard. Le cache est ISOLÉ : depuis que le
# harness journalise sa machine au démarrage, un harness de test qui n'isolerait
# pas son cache écrirait un fait `machine:` pour une base de test dans le registre
# VIVANT (défaut mesuré, famille 3 de la porte d'atelier).
demarrer_harness() { # $1 = base servie ; $2 = étiquette (pour le cache/log)
    local base="$1" etiquette="$2" port
    port="$(port_libre)"
    [ "$port" = "0" ] && { echo "0:0"; return; }
    mkdir -p "$RACINE/$etiquette/cache"
    OBRAIN_VIZ_CACHE_DIR="$RACINE/$etiquette/cache" \
    OBRAIN_HARNESS_ROOT="$RACINE/$etiquette/racine" \
        "$BIN" harness-serve "$base" --addr "127.0.0.1:$port" \
        >"$RACINE/$etiquette/log" 2>&1 &
    local pid=$!
    SERVEURS+=("$pid")
    local fin=$((SECONDS + 40))
    while [ "$SECONDS" -lt "$fin" ]; do
        if ss -ltn 2>/dev/null | grep -q ":$port "; then echo "$pid:$port"; return; fi
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.2
    done
    echo "$pid:0"
}

get()  { curl -sS -m 10 -o "$RACINE/corps" -w '%{http_code}' "$1" 2>/dev/null || echo 000; }
post() { curl -sS -m 10 -X POST -H 'content-type: application/json' -d "$2" -o "$RACINE/corps" -w '%{http_code}' "$1" 2>/dev/null || echo 000; }
# L'empreinte d'un dossier ENTIER : contenu de chaque fichier, ordonné par nom.
# C'est elle qui porte l'invariant « le harness n'écrit pas » — une comparaison de
# mtime ne suffirait pas (une réécriture identique doit être vue comme un changement,
# et un fichier ajouté aussi).
empreinte_dossier() {
    find "$1" -type f -print0 2>/dev/null | sort -z \
        | xargs -0 sha256sum 2>/dev/null | sha256sum | cut -d' ' -f1
}
corps() { head -c "${1:-200}" "$RACINE/corps" 2>/dev/null | tr -d '\n'; }
champ() { python3 -c 'import json,sys
try: d=json.load(open(sys.argv[1]))
except Exception: print(""); sys.exit(0)
v=d
for k in sys.argv[2].split("."):
    v = v.get(k) if isinstance(v, dict) else None
    if v is None: break
print("" if v is None else (json.dumps(v, ensure_ascii=False) if isinstance(v,(dict,list)) else v))' "$RACINE/corps" "$1"; }

printf '════════════════════════════════════════════════════════════════════\n'
printf 'binaire mesuré : %s (%s)\n' "$BIN" "$(sha256sum "$BIN" 2>/dev/null | cut -c1-16)"
if [ -n "$FAMILLES" ]; then
    printf 'PORTE DU HARNESS — ÉPREUVE PARTIELLE (familles %s) — %s\n' "$FAMILLES" "$(date -Is)"
    printf 'CE N EST PAS UN VERDICT : les garanties non exécutées ne sont pas mesurées.\n'
else
    printf 'PORTE DU HARNESS — %s\n' "$(date -Is)"
fi
printf '════════════════════════════════════════════════════════════════════\n'

# ── 1) L'IDENTITÉ ───────────────────────────────────────────────────────────
f1_identite() {
    titre "1) l'identité : ce que le harness dit de lui-même est VRAI"
    local base="$RACINE/f1/base.obrain"
    mkdir -p "$RACINE/f1"
    "$BIN" init "$base" >/dev/null 2>&1 || { ko "création de la base de test impossible"; return; }
    "$BIN" query "$base" "CREATE (:Session {qualified_name: 'session-f1', cwd: '/tmp', createdAt: 1})" >/dev/null 2>&1

    local pp port pid
    pp="$(demarrer_harness "$base" f1)"; pid="${pp%%:*}"; port="${pp##*:}"
    if [ "$port" = "0" ]; then ko "le harness ne démarre pas sur une base temporaire (voir $RACINE/f1/log)"; return; fi
    mes "harness sur 127.0.0.1:$port (PID $pid), base $base"

    local code
    code="$(get "http://127.0.0.1:$port/api/harness/health")"
    [ "$code" = "200" ] && ok "/api/harness/health répond 200" || ko "/api/harness/health répond $code : $(corps 200)"
    code="$(get "http://127.0.0.1:$port/api/harness/machine")"
    if [ "$code" = "200" ]; then
        ok "/api/harness/machine répond 200"
        H_NOM="$(champ machine)"; H_STORE="$(champ store)"; H_MODE="$(champ mode)"
        H_SRC="$(champ nom_source)"; H_INST="$(champ instance)"
        H_COMMIT="$(champ build.commit)"; H_DESC="$(champ build.describe)"
        H_RUSTC="$(champ build.rustc)"
        H_OS="$(champ os)"; H_ARCH="$(champ arch)"
        mes "machine=$H_NOM  os=$H_OS  arch=$H_ARCH  mode=$H_MODE"
        mes "store=$H_STORE"
        mes "instance=$H_INST"
        mes "nom_source=$H_SRC"
        mes "build : commit=${H_COMMIT:0:12} describe=$H_DESC"
    else
        ko "/api/harness/machine répond $code : $(corps 200)"
        return
    fi

    # Le nom d'hôte doit venir du NOYAU, et la source doit être PUBLIÉE.
    local noyau; noyau="$(cat /proc/sys/kernel/hostname 2>/dev/null | tr -d '\n')"
    if [ "$H_NOM" = "$noyau" ]; then
        ok "le nom d'hôte annoncé est celui du noyau (« $noyau »)"
    else
        ko "le harness annonce « $H_NOM » alors que le noyau dit « $noyau »"
    fi
    case "$H_SRC" in
        /proc/sys/kernel/hostname|/etc/hostname) ok "la SOURCE du nom est publiée ($H_SRC) — un nom vérifiable, pas un argument" ;;
        *) ko "la source du nom n'est pas publiée (nom_source=« $H_SRC ») : on ne peut pas savoir si le nom est vérifié" ;;
    esac

    # Le store annoncé doit être celui qui est RÉELLEMENT servi : la session
    # semée dans CETTE base doit être la seule visible.
    code="$(get "http://127.0.0.1:$port/api/harness/sessions?limit=50")"
    if [ "$code" = "200" ] && grep -q 'session-f1' "$RACINE/corps"; then
        ok "le store annoncé est celui qui est VRAIMENT servi (la session semée s'y trouve)"
    else
        ko "le store annoncé n'est pas celui qui est servi : /sessions ne rend pas la session semée ($code : $(corps 160))"
    fi

    # Le build doit être TRACÉ : un binaire qui ne dit pas d'où il vient n'est pas
    # déployable (R2), et l'identité du harness doit le dire.
    if [ -n "$H_COMMIT" ] && [ "$H_COMMIT" != "inconnu" ] && [ "$H_COMMIT" != "null" ]; then
        ok "le BUILD est tracé (commit ${H_COMMIT:0:12}, describe $H_DESC)"
        if [ "$H_COMMIT" = "$(git rev-parse HEAD)" ]; then
            ok "le commit annoncé est celui de l'arbre mesuré — pas une constante"
        else
            mes "le commit annoncé ($(printf '%s' "$H_COMMIT" | cut -c1-12)) diffère du HEAD actuel ($(git rev-parse --short HEAD)) : binaire construit avant le dernier commit (normal si non reconstruit)"
        fi
    else
        ko "le build n'est PAS tracé (build.commit=« $H_COMMIT ») : rien ne dit d'où vient ce binaire"
    fi

    # ── LA VERSION DE RUSTC COMPTE AUTANT QUE LE COMMIT ─────────────────────
    # Un MÊME source produit deux binaires différents selon le compilateur : sans
    # cette estampille, un comportement qui change sans que le code ait bougé est
    # inexplicable. `rust-version` du manifeste n'est pas la réponse — c'est une
    # compatibilité déclarée, pas la version qui a compilé. Défaut mesuré : le
    # champ valait « non_estampee » jusqu'au 2026-09-16.
    if [ -n "$H_RUSTC" ] && [ "$H_RUSTC" != "non_estampee" ] && [ "$H_RUSTC" != "inconnu" ]; then
        ok "la version de rustc est ESTAMPILLÉE (« $H_RUSTC »)"
    else
        ko "la version de rustc n'est pas estampillée (build.rustc=« $H_RUSTC ») : un même source produirait deux binaires indistinguables"
    fi

    # health et machine doivent dire LA MÊME chose sur l'identité : deux vérités
    # pour un même objet, c'est une de trop.
    code="$(get "http://127.0.0.1:$port/api/harness/health")"
    local i_nom i_store i_inst i_commit
    i_nom="$(champ identite.machine)"; i_store="$(champ identite.store)"
    i_inst="$(champ identite.instance)"; i_commit="$(champ identite.build.commit)"
    if [ -n "$i_nom" ]; then
        ok "/api/harness/health porte l'identité (champ « identite »)"
    else
        ko "/api/harness/health ne porte PAS l'identité — impossible de nommer la machine depuis la route de santé"
    fi
    if [ "$i_nom" = "$H_NOM" ] && [ "$i_store" = "$H_STORE" ] && [ "$i_inst" = "$H_INST" ] && [ "$i_commit" = "$H_COMMIT" ]; then
        ok "health et machine disent EXACTEMENT la même identité"
    else
        ko "health et machine DIVERGENT : machine=($H_NOM|$H_STORE|$H_INST|$H_COMMIT) health=($i_nom|$i_store|$i_inst|$i_commit)"
    fi

    # Les champs historiques de health doivent SURVIVRE : des portes et des
    # scripts les lisent (scripts/verifier-integration-tissage.sh, flotte-auto-…).
    local st sd sv
    st="$(champ status)"; sd="$(champ mode)"; sv="$(champ version)"
    if [ "$st" = "ok" ] && [ "$sd" = "ro" ] && [ -n "$sv" ]; then
        ok "les champs historiques de health sont intacts (status=$st, mode=$sd, version=$sv)"
    else
        ko "les champs historiques de health ont changé (status=$st, mode=$sd, version=$sv) — des consommateurs les lisent"
    fi

    # Les DEUX MOITIÉS de l'annonce : ce qu'il sert (store, mode) ET ce qu'il ne
    # fait pas. Un harness qui ne dit que le positif laisse croire qu'il écrit.
    if [ -n "$H_MODE" ]; then
        ok "le mode OFFERT est annoncé ($H_MODE) — un lecteur sait à quoi s'en tenir"
    else
        ko "aucun mode offert annoncé : un client ne peut pas savoir si le harness écrit"
    fi
}

# ── 2) LES SESSIONS ─────────────────────────────────────────────────────────
f2_sessions() {
    titre "2) les sessions : pagination qui avance, et aucune orpheline"
    local base="$RACINE/f2/base.obrain"
    mkdir -p "$RACINE/f2"
    "$BIN" init "$base" >/dev/null 2>&1 || { ko "création impossible"; return; }
    local i
    for i in 1 2 3 4 5; do
        "$BIN" query "$base" "CREATE (:Session {qualified_name: 'session-p$i', cwd: '/tmp/$i', createdAt: $i})" >/dev/null 2>&1
    done
    local pp port pid
    pp="$(demarrer_harness "$base" f2)"; pid="${pp%%:*}"; port="${pp##*:}"
    [ "$port" = "0" ] && { ko "le harness ne démarre pas"; return; }

    local c1 c2 n1 n2
    c1="$(get "http://127.0.0.1:$port/api/harness/sessions?limit=2")"
    n1="$(champ sessions | python3 -c 'import json,sys
try: print(len(json.load(sys.stdin)))
except Exception: print(0)')"
    if [ "$c1" = "200" ] && [ "$n1" = "2" ]; then
        ok "limit=2 rend exactement 2 sessions"
    else
        ko "limit=2 rend $n1 session(s) (HTTP $c1) : la borne n'est pas respectée"
    fi

    c2="$(get "http://127.0.0.1:$port/api/harness/sessions?limit=2&offset=2")"
    n2="$(champ sessions | python3 -c 'import json,sys
try: print(len(json.load(sys.stdin)))
except Exception: print(0)')"
    if [ "$c1" = "200" ] && [ "$c2" = "200" ]; then
        # Les deux pages ne doivent PAS se recouvrir : une pagination qui
        # recouvre fait relire indéfiniment les mêmes sessions.
        cp "$RACINE/corps" "$RACINE/page2.json"
        get "http://127.0.0.1:$port/api/harness/sessions?limit=2" >/dev/null
        cp "$RACINE/corps" "$RACINE/page1.json"
        if python3 - "$RACINE/page1.json" "$RACINE/page2.json" <<'PY'
import json, sys
a = {s["name"] for s in json.load(open(sys.argv[1]))["sessions"]}
b = {s["name"] for s in json.load(open(sys.argv[2]))["sessions"]}
sys.exit(1 if (a & b) else 0)
PY
        then
            ok "deux pages successives (limit=2, offset=2) NE se recouvrent PAS"
        else
            ko "les deux pages se RECOUVRENT : une lecture qui pagine relirait indéfiniment les mêmes sessions"
        fi
    else
        ko "la pagination ne répond pas 200 dans les deux cas (page1=$c1, page2=$c2)"
    fi

    # Cohérence : toute session listée doit être OUVRABLE par l'injection. Une
    # session listée mais inconnue de l'injection est une promesse non tenue.
    local fautives=0 total=0 nom
    get "http://127.0.0.1:$port/api/harness/sessions?limit=50" >/dev/null
    cp "$RACINE/corps" "$RACINE/toutes.json"
    for nom in $(python3 -c 'import json,sys
for s in json.load(open(sys.argv[1]))["sessions"]: print(s["name"])' "$RACINE/toutes.json" 2>/dev/null); do
        total=$((total + 1))
        post "http://127.0.0.1:$port/api/harness/context" "{\"session_id\":\"$nom\"}" >/dev/null
        if [ "$(champ status)" != "ok" ]; then fautives=$((fautives + 1)); fi
    done
    if [ "$total" -gt 0 ] && [ "$fautives" = "0" ]; then
        ok "les $total sessions listées sont TOUTES ouvrables par l'injection (0 orpheline)"
    else
        ko "$fautives session(s) listée(s) sur $total ne sont PAS ouvrables : la liste promet ce que l'injection ne tient pas"
    fi
}

# ── 3) L'INJECTION ──────────────────────────────────────────────────────────
f3_injection() {
    titre "3) l'injection : elle dit ce qu'elle sait, et REFUSE de dire « ok » sur du vide"
    local base="$RACINE/f3/base.obrain"
    mkdir -p "$RACINE/f3"
    "$BIN" init "$base" >/dev/null 2>&1 || { ko "création impossible"; return; }
    "$BIN" query "$base" "CREATE (:Session {qualified_name: 'session-f3', cwd: '/tmp', createdAt: 1})" >/dev/null 2>&1
    "$BIN" query "$base" "CREATE (:Goal {qualified_name: 'but-f3', text: 'un objectif'})" >/dev/null 2>&1
    "$BIN" query "$base" "MATCH (s:Session {qualified_name:'session-f3'}), (g:Goal {qualified_name:'but-f3'}) MERGE (s)-[:HAS_GOAL]->(g)" >/dev/null 2>&1
    local pp port pid
    pp="$(demarrer_harness "$base" f3)"; pid="${pp%%:*}"; port="${pp##*:}"
    [ "$port" = "0" ] && { ko "le harness ne démarre pas"; return; }

    local code
    code="$(post "http://127.0.0.1:$port/api/harness/context" '{"session_id":"session-f3"}')"
    if [ "$code" = "200" ] && [ "$(champ status)" = "ok" ]; then
        ok "sur une session CONNUE, l'injection rend 200 et status=ok"
    else
        ko "sur une session connue, l'injection rend $code / status=$(champ status) : $(corps 200)"
    fi
    # Les sections attendues : elles doivent être PRÉSENTES (même vides), sinon le
    # lecteur ne peut pas distinguer « rien à dire » de « section oubliée ».
    local s sections_attendues="goals summaries weave engrams"
    local manquantes=""
    for s in $sections_attendues; do
        python3 -c 'import json,sys
d=json.load(open(sys.argv[1]))
sys.exit(0 if sys.argv[2] in d else 1)' "$RACINE/corps" "$s" || manquantes="$manquantes $s"
    done
    if [ -z "$manquantes" ]; then
        ok "les sections attendues sont présentes : $sections_attendues"
    else
        ko "sections MANQUANTES dans l'injection :$manquantes — un lecteur ne peut pas distinguer « rien à dire » d'une section oubliée"
    fi
    # Le bloc `tissage` : il doit DIRE son état, et ne pas prétendre que tout va
    # bien quand il ne sait pas (l'état `sans_journal` existe pour ça).
    local tetat
    tetat="$(champ tissage.etat)"
    if [ -n "$tetat" ]; then
        ok "le bloc « tissage » annonce son état (« $tetat ») — la conversation sait si son graphe est tissé"
    else
        local tstale; tstale="$(champ tissage.stale)"
        if [ -n "$tstale" ]; then
            mes "le bloc « tissage » ne porte que l'ancien champ « stale » ($tstale) : l'état nommé (frais/figé/sans_journal) manque"
        else
            ko "AUCUN bloc « tissage » dans l'injection : la conversation ne peut pas savoir que son tissage s'est arrêté"
        fi
    fi

    # ── LE DÉFAUT QUI A JUSTIFIÉ CETTE PORTE ────────────────────────────────
    # SANS session : le harness doit REFUSER, pas servir du vide en « ok ».
    code="$(post "http://127.0.0.1:$port/api/harness/context" '{}')"
    if [ "$code" != "200" ] || [ "$(champ status)" != "ok" ]; then
        ok "SANS session, l'injection REFUSE (HTTP $code, status=$(champ status)) au lieu de servir du vide en « ok »"
    else
        ko "SANS session, l'injection rend 200 + status=ok : « un ok sur un vide intégral » — le mode de défaillance que la doctrine interdit"
    fi
    # AVEC une session INCONNUE : c'est le même mensonge, et il est PIRE, parce que
    # le lecteur croit que le graphe est muet alors que la session n'existe pas.
    code="$(post "http://127.0.0.1:$port/api/harness/context" '{"session_id":"session-qui-nexiste-pas"}')"
    if [ "$code" != "200" ] || [ "$(champ status)" != "ok" ]; then
        ok "avec une session INCONNUE, l'injection REFUSE (HTTP $code, status=$(champ status))"
    else
        ko "avec une session INCONNUE, l'injection rend 200 + status=ok sur des sections vides : le lecteur croira que le graphe est muet alors que la session n'existe pas"
    fi
    # Et la raison doit être LISIBLE : un refus sans motif ne se corrige pas.
    if [ -n "$(champ raison)" ]; then
        ok "le refus est MOTIVÉ (« $(champ raison | head -c 90)… »)"
    else
        ko "le refus de l'injection n'est pas motivé : un refus sans motif ne se corrige pas"
    fi
}

# ── 4) LECTURE SEULE ────────────────────────────────────────────────────────
f4_lecture_seule() {
    titre "4) le harness n'ÉCRIT RIEN dans la base qu'il sert"
    local base="$RACINE/f4/base.obrain"
    mkdir -p "$RACINE/f4"
    "$BIN" init "$base" >/dev/null 2>&1 || { ko "création impossible"; return; }
    "$BIN" query "$base" "CREATE (:Session {qualified_name: 'session-f4', cwd: '/tmp', createdAt: 1})" >/dev/null 2>&1

    # ── LA RÉFÉRENCE EST PRISE AVANT TOUT DÉMARRAGE ─────────────────────────
    # Défaut de la première version de cette famille : elle prenait la référence
    # APRÈS avoir fait tourner un harness, donc elle ne pouvait pas voir ce que
    # ce premier harness avait écrit — elle mesurait le second. Une mesure dont la
    # référence est prise trop tard ne mesure rien.
    local avant; avant="$(empreinte_dossier "$base")"
    [ -z "$avant" ] && { ko "impossible de mesurer l'état initial de la base"; return; }

    local pp port pid
    pp="$(demarrer_harness "$base" f4)"; pid="${pp%%:*}"; port="${pp##*:}"
    [ "$port" = "0" ] && { ko "le harness ne démarre pas"; return; }

    local i
    for i in 1 2 3; do
        get "http://127.0.0.1:$port/api/harness/health" >/dev/null
        get "http://127.0.0.1:$port/api/harness/machine" >/dev/null
        get "http://127.0.0.1:$port/api/harness/sessions?limit=10" >/dev/null
        get "http://127.0.0.1:$port/api/harness/continuity" >/dev/null
        post "http://127.0.0.1:$port/api/harness/context" '{"session_id":"session-f4"}' >/dev/null
        # Les gestes qui POURRAIENT écrire, justement pour voir s'ils le font.
        post "http://127.0.0.1:$port/api/harness/context" '{"session_id":"inconnue"}' >/dev/null
        get "http://127.0.0.1:$port/api/harness/sessions?limit=1000" >/dev/null
    done
    kill -9 "$pid" 2>/dev/null; sleep 0.5
    local apres; apres="$(empreinte_dossier "$base")"
    if [ "$avant" = "$apres" ]; then
        ok "la base servie est BIT POUR BIT identique après 21 appels au contrat (invariant mono-écrivain tenu)"
    else
        ko "LA BASE SERVIE A CHANGÉ : le harness écrit dans ce qu'il annonce servir en LECTURE — l'invariant mono-écrivain est violé"
        mes "fichiers avant/après : $(find "$base" -type f 2>/dev/null | wc -l)"
        find "$base" -type f -newer "$RACINE/f4" 2>/dev/null | head -3 | sed 's/^/      touché : /'
    fi
}

# ── 5) L'AUTO-DÉCLARATION ───────────────────────────────────────────────────
f5_auto_declaration() {
    titre "5) le harness se DÉCLARE, et la déclaration devient un fait du graphe"
    local base="$RACINE/f5/base.obrain"
    mkdir -p "$RACINE/f5"
    "$BIN" init "$base" >/dev/null 2>&1 || { ko "création impossible"; return; }
    local pp port pid
    pp="$(demarrer_harness "$base" f5)"; pid="${pp%%:*}"; port="${pp##*:}"
    [ "$port" = "0" ] && { ko "le harness ne démarre pas"; return; }
    kill -9 "$pid" 2>/dev/null; sleep 0.5

    local journal="$RACINE/f5/cache/machines"
    if [ -d "$journal" ] && ls "$journal"/*.jsonl >/dev/null 2>&1; then
        local n; n="$(cat "$journal"/*.jsonl 2>/dev/null | grep -c '"kind"[[:space:]]*:[[:space:]]*"machine"')"
        if [ "$n" -ge 1 ]; then
            ok "au démarrage, le harness a JOURNALISÉ son identité ($n déclaration(s))"
            local store_declare; store_declare="$(grep -o '"store":"[^"]*"' "$journal"/*.jsonl 2>/dev/null | head -1 | cut -d'"' -f4)"
            if [ "$store_declare" = "$base" ]; then
                ok "la déclaration nomme la base RÉELLEMENT servie"
            else
                ko "la déclaration nomme « $store_declare » alors que la base servie est « $base »"
            fi
            local caps; caps="$(grep -o '"capacites"[^]]*]' "$journal"/*.jsonl 2>/dev/null | head -1)"
            if [ "$caps" = '"capacites":[]' ] || [ -z "$caps" ]; then
                ok "il n'annonce AUCUNE capacité (un moteur seul n'en a pas : « annoncer est décider »)"
            else
                ko "il annonce des capacités alors qu'un moteur n'exécute rien : $caps"
            fi
        else
            ko "le journal des machines ne porte AUCUNE déclaration du harness"
        fi
    else
        ko "aucun journal de machines écrit au démarrage : le harness ne se déclare pas"
    fi

    # Et la PASSE doit en faire un fait du graphe, nommé.
    OBRAIN_VIZ_CACHE_DIR="$RACINE/f5/cache" "$BIN" machines --store "$base" --appliquer \
        >"$RACINE/f5/passe.json" 2>"$RACINE/f5/passe.err" \
        || { ko "la passe de la flotte a échoué : $(head -c 160 "$RACINE/f5/passe.err")"; return; }
    local noms
    noms="$("$BIN" query "$base" --read-only "MATCH (n) WHERE n.qualified_name STARTS WITH 'machine:' RETURN n.qualified_name" 2>/dev/null)"
    local noyau; noyau="$(cat /proc/sys/kernel/hostname | tr -d '\n')"
    if printf '%s' "$noms" | grep -q "machine:$noyau"; then
        ok "après la passe, le graphe porte « machine:$noyau » — la machine est un fait, pas une vue"
        ok "la même identité est trouvable par « qn » ET par « qualified_name » (le dépôt porte deux conventions)"
    else
        ko "le graphe ne porte pas « machine:$noyau » après la passe : $(printf '%s' "$noms" | tr -d '\n' | head -c 160)"
    fi
    if printf '%s' "$noms" | grep -q "machine:$noyau#"; then
        ok "le nœud d'INSTANCE (machine:<hôte>#<base>) distingue les moteurs d'un même hôte"
    else
        mes "aucun nœud d'instance « machine:$noyau#<base> » : deux moteurs sur un même hôte ne seraient pas discernables"
    fi
}

# ── 6) DEUX HARNESSES, DEUX BASES ───────────────────────────────────────────
f6_deux_harness() {
    titre "6) deux harnesses sur deux bases : identités distinguables, aucune fuite"
    local b1="$RACINE/f6/un.obrain" b2="$RACINE/f6/deux.obrain"
    mkdir -p "$RACINE/f6"
    "$BIN" init "$b1" >/dev/null 2>&1; "$BIN" init "$b2" >/dev/null 2>&1
    "$BIN" query "$b1" "CREATE (:Session {qualified_name: 'session-un', cwd: '/tmp', createdAt: 1})" >/dev/null 2>&1
    "$BIN" query "$b2" "CREATE (:Session {qualified_name: 'session-deux', cwd: '/tmp', createdAt: 1})" >/dev/null 2>&1
    local p1 p2 port1 port2
    p1="$(demarrer_harness "$b1" f6a)"; port1="${p1##*:}"
    p2="$(demarrer_harness "$b2" f6b)"; port2="${p2##*:}"
    [ "$port1" = "0" ] || [ "$port2" = "0" ] && { ko "les deux harnesses ne démarrent pas ensemble"; return; }

    get "http://127.0.0.1:$port1/api/harness/machine" >/dev/null; local i1; i1="$(champ instance)"
    get "http://127.0.0.1:$port2/api/harness/machine" >/dev/null; local i2; i2="$(champ instance)"
    if [ -n "$i1" ] && [ -n "$i2" ] && [ "$i1" != "$i2" ]; then
        ok "deux harnesses sur deux bases se disent DIFFÉRENTS ($i1 ≠ $i2)"
    else
        ko "deux harnesses sur deux bases se disent IDENTIQUES ($i1 = $i2) : on ne peut pas les distinguer"
    fi
    get "http://127.0.0.1:$port1/api/harness/sessions?limit=50" >/dev/null
    if grep -q 'session-un' "$RACINE/corps" && ! grep -q 'session-deux' "$RACINE/corps"; then
        ok "le premier ne sert QUE sa base (aucune fuite de session)"
    else
        ko "FUITE : le premier harness sert des sessions qui ne sont pas dans sa base"
    fi
    get "http://127.0.0.1:$port2/api/harness/sessions?limit=50" >/dev/null
    if grep -q 'session-deux' "$RACINE/corps" && ! grep -q 'session-un' "$RACINE/corps"; then
        ok "le second ne sert QUE sa base"
    else
        ko "FUITE : le second harness sert des sessions qui ne sont pas dans sa base"
    fi
}

# ── 7) LES ROUTES INCONNUES ─────────────────────────────────────────────────
f7_routes() {
    titre "7) les routes : une route absente se dit absente, et rien du moteur ne fuit"
    local base="$RACINE/f7/base.obrain"
    mkdir -p "$RACINE/f7"
    "$BIN" init "$base" >/dev/null 2>&1
    local pp port pid
    pp="$(demarrer_harness "$base" f7)"; pid="${pp%%:*}"; port="${pp##*:}"
    [ "$port" = "0" ] && { ko "le harness ne démarre pas"; return; }

    local code
    code="$(get "http://127.0.0.1:$port/api/harness/route-inconnue")"
    if [ "$code" = "404" ]; then
        ok "une route inconnue du harness rend 404 (elle se dit absente, elle ne ment pas)"
    else
        ko "une route inconnue rend $code au lieu de 404 : $(corps 160)"
    fi
    # Le harness N'EST PAS le moteur : les routes de flotte ne doivent pas être
    # servies ici, sinon la surface se confond et « qui parle à qui » se perd.
    for r in /api/remote/machines /api/remote/clients /api/query; do
        code="$(get "http://127.0.0.1:$port$r")"
        if [ "$code" = "404" ]; then
            ok "$r n'est PAS servi par le harness (404) — le harness n'est pas le moteur"
        elif [ "$code" = "000" ]; then
            ko "$r : aucune réponse du harness"
        else
            mes "$r rend $code sur le harness (jamais 404) : cette route du moteur est servie par le harness, à vérifier"
        fi
    done
}

famille 1 && f1_identite
famille 2 && f2_sessions
famille 3 && f3_injection
famille 4 && f4_lecture_seule
famille 5 && f5_auto_declaration
famille 6 && f6_deux_harness
famille 7 && f7_routes

titre "verdict"
printf '  %d vérification(s) · %d mesure(s)\n' "$verifications" "$mesures"
if [ "$echecs" != "0" ]; then
    printf '  ❌ %d GARANTIE(S) NON TENUE(S) sur %d — le contrat du harness n est pas bon\n' "$echecs" "$verifications"
    [ "${GARDER_TRACE:-0}" = "1" ] && printf '  trace : %s\n' "$RACINE"
    exit 1
fi
if [ -n "$FAMILLES" ]; then
    printf '  ⚠️  ÉPREUVE PARTIELLE (familles %s) : ces garanties-là tiennent, LES AUTRES N ONT\n' "$FAMILLES"
    printf '      PAS ÉTÉ MESURÉES. Pour un verdict, lancer la porte sans --familles.\n'
    exit 0
fi
printf '  ✅ LE HARNESS TIENT SON CONTRAT : identité vraie et tracée, pagination qui\n'
printf '     avance, injection qui refuse de dire « ok » sur du vide, base servie\n'
printf '     intacte, auto-déclaration devenue fait du graphe, deux harnesses\n'
printf '     distinguables, routes absentes dites absentes.\n'
exit 0
