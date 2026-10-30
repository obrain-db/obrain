#!/bin/sh
# ============================================================================
# Vérificateur d'intégration du tissage — LECTURE SEULE.
#
# Ce qu'il vérifie : qu'une conversation illimitée reste COHÉRENTE avec une IA
# qui s'auto-tisse. Autrement dit, que la chaîne complète tient, maillon par
# maillon :
#
#   journaux (écrivain vivant) → import dans le graphe → soi → injection servie
#
# Chaque constat est VÉRIFIÉ, pas supposé : un défaut d'intégration ne doit pas
# se découvrir en lisant le code, il doit tomber ici.
#
# Rien n'est écrit : le store est ouvert en verrou PARTAGÉ (le pont ouvre en
# lecture), le démon est en `ro`, et la sentinelle est en lecture pure. Ce
# vérificateur peut donc tourner pendant que la production travaille.
#
# Usage :  verifier-tissage.sh [--store CHEMIN] [--daemon URL] [-q]
#          OBRAIN_BIN (défaut : <dépôt>/target/release/obrain), OBRAIN_STORE,
#          OBRAIN_HARNESS_URL, SESSION_TEST, SEUIL_INJECTION_MS.
# Sortie : code 1 dès qu'un constat est en ÉCHEC (comme le prévol).
# ============================================================================
set -u

STORE="${OBRAIN_STORE:-$HOME/.local/share/obrain-harness/data/runtime-dev.obrain}"
DAEMON="${OBRAIN_HARNESS_URL:-http://192.168.1.103:8090}"
RACINE="$(cd "$(dirname "$0")/.." && pwd)"
OBRAIN="${OBRAIN_BIN:-$RACINE/target/release/obrain}"
SEUIL_MS="${SEUIL_INJECTION_MS:-50}"
SESSION="${SESSION_TEST:-}"
QUIET=0

while [ $# -gt 0 ]; do
  case "$1" in
    --store) STORE="$2"; shift 2 ;;
    --daemon) DAEMON="$2"; shift 2 ;;
    -q) QUIET=1; shift ;;
    *) echo "usage: $0 [--store CHEMIN] [--daemon URL] [-q]" >&2; exit 2 ;;
  esac
done

echecs=0
ligne() { # nom verdict preuve
  if [ "$QUIET" = 0 ]; then
    printf '%-22s %-10s %s\n' "$1" "$2" "$3"
  elif [ "$2" = "ÉCHEC" ]; then
    printf 'ÉCHEC %s : %s\n' "$1" "$3"
  fi
}
ok()   { ligne "$1" OK "$2"; }
ko()   { ligne "$1" ÉCHEC "$2"; echecs=$((echecs + 1)); }
note() { ligne "$1" ATTENTION "$2"; }

if [ "$QUIET" = 0 ]; then
  echo "vérification de l'intégration du tissage"
  printf '%-22s %-10s %s\n' constat verdict preuve
fi

# ── 1. L'écrivain : l'IA tisse-t-elle EN CE MOMENT ? ────────────────────────
# Sans écrivain, plus rien n'entre dans le graphe : la conversation continue
# mais son tissage s'arrête — c'est le mode de panne de l'incident du 05/09.
N=$(pgrep -f "obrain bridge" 2>/dev/null | wc -l)
if [ "$N" -ge 1 ]; then
  ok "écrivain" "$N processus \`obrain bridge\` actifs"
else
  ko "écrivain" "AUCUN \`obrain bridge\` : le tissage est arrêté (la conversation, elle, continue)"
fi

# ── 2. La continuité : le store a-t-il reçu un événement récemment ? ────────
if [ -x "$OBRAIN" ]; then
  if "$OBRAIN" continuity "$STORE" --stale-after-min 30 -q >/dev/null 2>&1; then
    ok "continuité" "store frais (< 30 min) — sentinelle exit 0"
  else
    ko "continuité" "store FIGÉ : aucun événement depuis 30 min (sentinelle exit 1)"
  fi
else
  ko "continuité" "binaire de la sentinelle absent : $OBRAIN"
fi

# ── 3. Le démon natif : les contrats du harness sont-ils servis ? ────────────
SANTE=$(timeout 8 curl -s "$DAEMON/api/harness/health" 2>/dev/null)
case "$SANTE" in
  *'"status":"ok"'*)
    MODE=$(printf '%s' "$SANTE" | sed -n 's/.*"mode":"\([^"]*\)".*/\1/p')
    ok "démon natif" "health ok (mode $MODE) — $DAEMON"
    ;;
  *) ko "démon natif" "pas de santé sur $DAEMON : l'injection native n'est pas servie" ;;
esac

# ── 4. LA SESSION QUI ÉCRIT AVANCE-T-ELLE DANS LE GRAPHE ? ─────────────────
# Ce constat a d'abord été FAUX, deux fois, et les deux erreurs valent d'être
# écrites ici :
#   · `createdAt` d'une session dit sa CRÉATION — une conversation ouverte il y
#     a sept heures et active à la seconde a un `createdAt` de sept heures ;
#   · `max(seq)` GLOBAL est dominé par la plus grosse session du store (3,78 M
#     ici), qui peut être inactive : le graphe paraissait figé alors qu'une
#     autre session avançait de 1 679 messages par minute.
# L'invariant qui compte est donc SESSION-SCOPÉ : la session dont le journal
# vient d'être écrit doit voir ses nœuds avancer. C'est ce qui décide si le
# rappel peut voir ce qui vient d'être dit.
Q_SESSION=$(ls -t "$STORE"/substrate.journal.* 2>/dev/null | head -1 | sed 's/.*substrate\.journal\.//')
if [ -z "$Q_SESSION" ]; then
  ko "session active" "aucune zone de journal dans le store : rien ne tisse"
else
  AGE_J=$(( $(date +%s) - $(stat -c %Y "$STORE/substrate.journal.$Q_SESSION" 2>/dev/null || echo 0) ))
  maxseq_session() {
    { printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"verif","version":"1"}}}\n'; sleep 1; \
      printf '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"query_graph","arguments":{"query":"MATCH (m:Message) WHERE m.qualified_name STARTS WITH \"%s\" RETURN max(m.seq) AS s"}}}\n' "$Q_SESSION"; sleep 4; } \
    | timeout 20 "$OBRAIN" bridge --store "$STORE" 2>/dev/null | tail -1 \
    | python3 -c '
import json,sys
try:
    i=json.loads(json.load(sys.stdin)["result"]["content"][0]["text"])
    print(int((i.get("rows") or [[0]])[0][0] or 0))
except Exception: print(-1)
' 2>/dev/null
  }
  A=$(maxseq_session); sleep 45; Bv=$(maxseq_session)
  if [ "${A:-0}" -lt 0 ] || [ "${Bv:-0}" -lt 0 ]; then
    ko "session active" "le graphe ne répond pas pour $Q_SESSION"
  elif [ "$Bv" -gt "$A" ]; then
    ok "session active" "$Q_SESSION (journal écrit il y a ${AGE_J}s) : +$((Bv - A)) messages en 45 s — le rappel voit ce qui vient d'être dit"
  elif [ "$AGE_J" -gt 300 ]; then
    note "session active" "$Q_SESSION n'écrit plus depuis ${AGE_J}s : rien à mesurer"
  else
    ko "session active" "$Q_SESSION écrit (journal il y a ${AGE_J}s) mais ses nœuds sont FIGÉS à $A pendant 45 s : ce qui vient d'être dit n'est pas rappelable"
  fi
fi

# ── 5. L'injection native : servie, non vide, et DANS SON CONTRAT ───────────
# Cible T1 annoncée : « < 50 ms p95 ». On la mesure au lieu de la croire.
if [ -z "$SESSION" ]; then
  SESSION=$(timeout 10 curl -s "$DAEMON/api/harness/sessions?limit=10" 2>/dev/null | python3 -c '
import json,sys
try:
    d=json.load(sys.stdin); ss=d.get("sessions") or []
    print(ss[0]["qn"] if ss else "")
except Exception: print("")
' 2>/dev/null)
fi
if [ -n "$SESSION" ]; then
  CORPS=$(timeout 10 curl -s -X POST -H 'content-type: application/json' \
    -d "{\"session_id\":\"$SESSION\",\"subject\":\"vérification d'intégration\"}" \
    "$DAEMON/api/harness/context" 2>/dev/null)
  case "$CORPS" in
    *'"status":"ok"'*)
      TAILLE=$(printf '%s' "$CORPS" | wc -c)
      # Une injection NON VIDE : au moins une des quatre sources doit parler.
      VIDE=oui
      for cle in '"goals":[' '"summaries":[' '"weave":[' '"engrams":['; do
        case "$CORPS" in
          *"$cle"*']') ;;
        esac
      done
      printf '%s' "$CORPS" | grep -qE '"(goals|summaries|weave|engrams)":\[\{' && VIDE=non
      if [ "$VIDE" = non ]; then
        ok "injection servie" "$TAILLE octets pour $SESSION"
      else
        note "injection servie" "payload servi mais VIDE pour $SESSION ($TAILLE octets) : le rappel ne rend rien"
      fi
      ;;
    *) ko "injection servie" "le démon ne sert pas de contexte pour $SESSION" ;;
  esac

  # Latence : p95 sur 15 appels, plancher HTTP mesuré à part pour ne pas
  # accuser le démon du coût de `curl`.
  PLANCher=$(for i in 1 2 3 4 5; do curl -s -o /dev/null -w '%{time_total}\n' "$DAEMON/api/harness/health" 2>/dev/null; done | sort -n | sed -n '3p')
  # ÉCHAUFFEMENT : les premières requêtes après un démarrage paient les pages
  # froides du store. Mesurer sans chauffer avait donné 574 ms — proche des
  # 632 ms du respawn de bridge, et donc un chiffre qui condamnait à tort le
  # chemin natif. On mesure ce que le consommateur vivra : à chaud.
  curl -s -o /dev/null -X POST -H 'content-type: application/json' \
    -d "{\"session_id\":\"$SESSION\",\"subject\":\"echauffement\"}" "$DAEMON/api/harness/context" 2>/dev/null
  P95=$(for i in $(seq 1 15); do
      curl -s -o /dev/null -w '%{time_total}\n' -X POST -H 'content-type: application/json' \
        -d "{\"session_id\":\"$SESSION\",\"subject\":\"latence\"}" "$DAEMON/api/harness/context" 2>/dev/null
    done | sort -n | awk '{a[NR]=$1*1000} END {printf "%.0f|%.0f", a[int(NR/2)+1], a[int(NR*0.95)+1]}')
  MED=${P95%%|*}; P95=${P95#*|}
  if [ -n "$P95" ]; then
    if [ "$P95" -le "$SEUIL_MS" ]; then
      ok "latence injection" "médiane ${MED} ms, p95 ${P95} ms (cible ≤ ${SEUIL_MS} ms ; plancher HTTP ${PLANCher}s)"
    else
      ko "latence injection" "médiane ${MED} ms, p95 ${P95} ms > ${SEUIL_MS} ms sur la session ACTIVE : le contrat T1 n'est pas tenu — et à ce niveau, le chemin natif ne vaut plus mieux que les 632 ms du respawn de bridge qu'il remplace"
    fi
  else
    ko "latence injection" "aucune mesure"
  fi
else
  ko "injection servie" "aucune session à interroger"
fi

# ── 5 bis. LE FIL : le tissage se sait-il lui-même ? ───────────────────────
# Deux vérifications, et la seconde est la plus importante : le verdict publié
# par le démon doit S'ACCORDER avec la sentinelle indépendante (le timer ops).
# Ils partagent le même calcul (`continuity::compute`) ; s'ils divergent, c'est
# que le démon tourne un binaire ancien — et un fil qui dit le contraire de la
# garde est pire que pas de fil.
if [ -n "${SESSION:-}" ]; then
  FIL=$(timeout 15 curl -s -X POST -H 'content-type: application/json' \
    -d "{\"session_id\":\"$SESSION\",\"subject\":\"fil\"}" "$DAEMON/api/harness/context" 2>/dev/null \
    | python3 -c '
import json,sys
try:
    d=json.load(sys.stdin); t=d.get("tissage")
    print("ABSENT|-|-|-" if not isinstance(t,dict) else "%s|%s|%s|%s"%(t.get("etat") or str(t.get("stale")).lower(), t.get("age_sec"), t.get("sessions_actives"), t.get("journaux")))
except Exception: print("ERR|-|-")
' 2>/dev/null)
  ETAT_F=${FIL%%|*}; R=${FIL#*|}; AGE_F=${R%%|*}; R2=${R#*|}; ACT=${R2%%|*}; JRN=${R2#*|}
  case "$ETAT_F" in
    ABSENT) ko "fil du tissage" "le contrat d'injection NE PORTE PAS le bloc \`tissage\` : l'IA ne peut pas savoir que son tissage s'est arrêté" ;;
    ERR)    ko "fil du tissage" "contrat d'injection illisible" ;;
    *)
      if [ -x "$OBRAIN" ] && "$OBRAIN" continuity "$STORE" --stale-after-min 30 -q >/dev/null 2>&1; then
        SENTINELLE=frais
      else
        SENTINELLE=fige
      fi
      case "$ETAT_F:$SENTINELLE" in
        frais:frais)    ok "fil du tissage" "contrat : \`etat=frais\` (age ${AGE_F}s, ${ACT} sessions actives sur ${JRN} journaux) et S'ACCORDE avec la sentinelle" ;;
        fige:fige)      ko "fil du tissage" "contrat et sentinelle disent tous deux FIGÉ (age ${AGE_F}s) : le tissage s'est arrêté — la conversation continue sans graphe" ;;
        sans_journal:*) note "fil du tissage" "contrat : \`etat=sans_journal\` (${JRN} journaux) — on ne sait pas si le tissage tourne ; ni frais ni figé, et surtout pas « tout va bien »" ;;
        false:frais)    ok "fil du tissage" "contrat : \`stale=false\` (age ${AGE_F}s, ${JRN} journaux) et S'ACCORDE avec la sentinelle" ;;
        true:fige)      ko "fil du tissage" "contrat et sentinelle disent tous deux FIGÉ (age ${AGE_F}s)" ;;
        *)              ko "fil du tissage" "DÉSACCORD : le contrat dit \`$ETAT_F\` et la sentinelle dit \`$SENTINELLE\` — le démon ne tourne pas la même version que la garde" ;;
      esac ;;
  esac
fi

# ── 5 ter. L'IAM EST-IL PRÉSENT SUR CHAQUE SURFACE ? ───────────────────────
# Une garde qui existe et n'est pas armée ne garde rien : le moteur sait
# exiger une identité (`OBRAIN_IAM=1` + `OBRAIN_IAM_STORE`, jeton par
# `OBRAIN_IAM_TOKEN` pour un pont), mais chaque surface doit l'armer. On lit
# l'environnement RÉEL des processus qui touchent le store : un ÉCRIVAIN sans
# IAM écrit sans identité ni politique — le tissage n'est alors gardé par rien.
# (Le pont du harness hérite de l'environnement de son parent : armer le
# service suffit, aucun code à modifier.)
iam_surface() {
  printf '%-24s %-12s %s\n' "SURFACE" "IMMATRIC." "IAM"
  for pid in $(pgrep -f "obrain bridge|obrain viz|harness-serve|this-rs-gateway" 2>/dev/null); do
    [ "$pid" = "$$" ] && continue
    cmd=$(tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null | cut -c1-22)
    w=$(tr '\0' '\n' < "/proc/$pid/environ" 2>/dev/null | grep -c '^OBRAIN_WRITE=1')
    i=$(tr '\0' '\n' < "/proc/$pid/environ" 2>/dev/null | grep -c '^OBRAIN_IAM=1')
    role=$([ "$w" -ge 1 ] && echo "ÉCRIVAIN" || echo "lecteur")
    printf '%-24s %-12s %s\n' "$cmd" "$role" "$([ "$i" -ge 1 ] && echo ARMÉ || echo ABSENT)"
  done
}
SURFACES=$(iam_surface)
ECRIVAINS_NUS=$(printf '%s\n' "$SURFACES" | awk '$2=="ÉCRIVAIN" && $3=="ABSENT"' | wc -l)
ECRIVAINS=$(printf '%s\n' "$SURFACES" | awk '$2=="ÉCRIVAIN"' | wc -l)
if [ "${ECRIVAINS:-0}" -eq 0 ]; then
  note "IAM des surfaces" "aucun écrivain détecté : rien à juger"
elif [ "${ECRIVAINS_NUS:-0}" -gt 0 ]; then
  ko "IAM des surfaces" "$ECRIVAINS_NUS écrivain(s) sur $ECRIVAINS écrivent SANS IAM (identité non exigée) — le tissage n'est pas gardé"
  printf '%s\n' "$SURFACES" | sed 's/^/    /'
else
  ok "IAM des surfaces" "$ECRIVAINS écrivain(s), tous sous IAM"
fi

# ── 6. Le soi : un seul, et lisible par étiquette ──────────────────────────
# L'IA se lit par étiquette. Si `Self` désigne autre chose que le soi, tout
# consommateur qui demande « qui suis-je » reçoit une roadmap.
# La réponse MCP est échappée deux fois (le résultat est une chaîne JSON dans
# une réponse JSON) : on la décode, sinon on compte zéro et on déclare « un seul
# soi » à tort — un constat faux vaut moins que pas de constat.
if [ -x "$OBRAIN" ]; then
  REP=$( { printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"verif","version":"1"}}}\n'; sleep 1; \
           printf '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"query_graph","arguments":{"query":"MATCH (n:Self) RETURN n.qualified_name AS qn"}}}\n'; sleep 6; } \
         | timeout 20 "$OBRAIN" bridge --store "$STORE" 2>/dev/null | tail -1 )
  BILAN=$(printf '%s' "$REP" | python3 -c '
import json,sys
try:
    r=json.load(sys.stdin)
    inter=json.loads(r["result"]["content"][0]["text"])
    rows=inter.get("rows") or []
    qns=[str(x[0]) for x in rows]
    soi=[q for q in qns if q.startswith("self:")]
    autres=[q for q in qns if not q.startswith("self:")]
    print("%d|%d|%s"%(len(soi), len(autres), ",".join(autres[:3])))
except Exception as e:
    print("ERR|0|%s"%e)
' 2>/dev/null)
  SOI=${BILAN%%|*}; R2=${BILAN#*|}; AUTRES=${R2%%|*}; EXEMPLES=${R2#*|}
  case "$BILAN" in
    ERR*) ko "soi unique" "réponse du graphe illisible ($EXEMPLES)" ;;
    *)
      if [ "$AUTRES" -gt 0 ]; then
        ko "soi unique" "$AUTRES nœud(s) portent l'étiquette \`Self\` sans être le soi ($EXEMPLES) — une IA qui se lit par étiquette se prend pour autre chose"
      elif [ "$SOI" -eq 1 ]; then
        ok "soi unique" "un seul nœud \`Self\` : self:agent"
      else
        ko "soi unique" "soi introuvable ($SOI nœud(s) \`self:*\`)"
      fi ;;
  esac
fi

# ── 7. Durabilité : le graphe signale-t-il des écritures perdues ? ──────────
# À l'ouverture, le moteur audite : des nœuds étiquetés SANS propriétés
# accessibles = des écritures passées après le dernier flush et perdues.
if [ -x "$OBRAIN" ]; then
  HUSK=$( { printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"verif","version":"1"}}}\n'; sleep 5; } \
          | timeout 20 "$OBRAIN" bridge --store "$STORE" 2>&1 >/dev/null \
          | grep -o 'label_only=[0-9]*' | head -1 | cut -d= -f2 )
  if [ -z "$HUSK" ]; then
    note "durabilité" "aucun audit d'ouverture lu (rien à conclure)"
  elif [ "$HUSK" -eq 0 ]; then
    ok "durabilité" "aucun nœud « husk » : rien n'a été perdu au dernier flush"
  else
    note "durabilité" "$HUSK nœud(s) étiquetés sans propriétés accessibles (écritures après le dernier flush) — informatif, la cause est côté écrivain"
  fi
fi

if [ "$QUIET" = 0 ]; then
  echo
  if [ "$echecs" -eq 0 ]; then
    echo "intégration cohérente : aucun constat en échec."
  else
    echo "$echecs constat(s) en ÉCHEC."
  fi
fi
[ "$echecs" -eq 0 ] || exit 1
