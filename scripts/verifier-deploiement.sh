#!/usr/bin/env bash
# =============================================================================
# Portes fonctionnelles d'un déploiement — obrain + harness (protocole G3).
# Réf : docs/PROTOCOLE-DEPLOIEMENT.md
#
# Usage : verifier-deploiement.sh --env {dev|prod} [--seuil-sessions N]
#
# --env est OBLIGATOIRE et sans valeur par défaut : un outil qui devine son
# environnement est un outil qui se trompe d'environnement (règle R6 / RFC
# « identité d'environnement »). Les chemins découlent de --env, jamais
# l'inverse.
#
# Sortie : 0 = toutes les portes passent. 1 = au moins une porte échoue.
# Chaque porte imprime PASS/FAIL avec la preuve, jamais un simple verdict.
# =============================================================================
set -uo pipefail

ENVI=""; SEUIL=""
while [ $# -gt 0 ]; do
  case "$1" in
    --env) ENVI="${2:-}"; shift 2 ;;
    --seuil-sessions) SEUIL="${2:-}"; shift 2 ;;
    *) echo "argument inconnu : $1"; exit 2 ;;
  esac
done
[ -z "$ENVI" ] && { echo "--env {dev|prod} est obligatoire (aucun défaut)"; exit 2; }

RACINE="$HOME/.local/share/obrain-harness"
OBRAIN="$RACINE/bin/obrain"
case "$ENVI" in
  prod) UNITE=dsh-web;      PORT=3082; STORE="$RACINE/data/runtime-prod.obrain";     HOME_DSH="$HOME/.dsh" ;;
  dev)  UNITE=dsh-dev-3083; PORT=3083; STORE="$RACINE/data/runtime-dev.obrain"; HOME_DSH="$HOME/.dsh-dev" ;;
  *) echo "env inconnu : $ENVI (dev|prod)"; exit 2 ;;
esac
SEUIL="${SEUIL:-60}"

ECHECS=0
ok()   { printf "  \033[32mPASS\033[0m  %-22s %s\n" "$1" "$2"; }
ko()   { printf "  \033[31mFAIL\033[0m  %-22s %s\n" "$1" "$2"; ECHECS=$((ECHECS+1)); }

echo "== $ENVI ($UNITE :$PORT) — $(date +%H:%M:%S)"

# --- Porte 0 : le binaire est celui du manifeste ------------------------------
MAN=$(ls -t "$RACINE"/etc/MANIFEST-*.txt 2>/dev/null | head -1)
if [ -n "$MAN" ]; then
  ATTENDU=$(grep -m1 binary_sha256 "$MAN" | awk "{print \$3}")
  COURANT=$(sha256sum "$OBRAIN" | cut -d" " -f1)
  [ "$ATTENDU" = "$COURANT" ] && ok "binaire" "$(basename "$MAN") — ${COURANT:0:16}" \
                              || ko "binaire" "attendu ${ATTENDU:0:16}, trouvé ${COURANT:0:16}"
else
  ko "binaire" "aucun manifeste dans $RACINE/etc"
fi

# --- Porte 1 : ÉCOUTE — boot() a rendu la main, plugins montés ----------------
# HTTP 200 ne prouve rien : le bundle client statique répond même quand
# l'arbre de plugins serveur est amputé. La ligne d'écoute, elle, n'est
# émise qu'APRÈS le retour de boot().
# La fenêtre couvre TOUT le boot : le wrapper de démarrage émet la ligne
# d'écoute AVANT qu'ActiveEnterTimestamp ne soit posé (écart mesuré ~10 s
# sur dsh-dev-3083, 2026-09-06 — ligne à 02:11:22, active à 02:11:32).
# On prend la borne la PLUS TÔT : ExecMainStartTimestamp (début réel du
# processus principal), sinon ActiveEnterTimestamp.
DEPUIS=$(systemctl --user show "$UNITE" -p ExecMainStartTimestamp -p ActiveEnterTimestamp --value \
         | grep -E '[0-9]{4}-[0-9]{2}-[0-9]{2}' \
         | sort | head -1 | sed -E 's/^[A-Za-z]+ +//; s/ +UTC$//')
if [ -n "$DEPUIS" ] && journalctl --user -u "$UNITE" --since "$DEPUIS" --no-pager 2>/dev/null | grep -q "dsh web: http"; then
  ok "écoute" "ligne d'écoute présente depuis $DEPUIS"
else
  ko "écoute" "AUCUNE ligne d'écoute depuis $DEPUIS — arbre de plugins suspect"
fi
FATALES=$(journalctl --user -u "$UNITE" --since "$DEPUIS" --no-pager 2>/dev/null \
          | grep -cE "plugin tree failed|duplicate loader entry|timeout bridge")
[ "$FATALES" = "0" ] && ok "journal" "0 erreur fatale de boot" \
                     || ko "journal" "$FATALES erreur(s) fatale(s) de boot"

# --- Porte 2 : CONTENU — le graphe répond, les degrés ne sont pas périmés -----
# Des sidecars de degrés périmés font renvoyer 0 aux expansions typées : la
# liste revient VIDE pendant que le service répond 200. `obrain validate`
# étant un no-op sur substrat, cette porte est le seul détecteur.
N=""
for i in $(seq 1 25); do
  printf '%s\n' '{"tool":"sessions_list","args":{}}' \
    | timeout 30 "$OBRAIN" mcp --store "$STORE" 2>/dev/null > /tmp/_g2-$ENVI.json
  N=$(python3 - "$ENVI" <<'PY' 2>/dev/null
import json,sys
d=json.load(open(f"/tmp/_g2-{sys.argv[1]}.json"))
print(len(json.loads(d["content"][0]["text"])["sessions"]))
PY
)
  [ -n "$N" ] && break
  sleep 2
done
if [ -z "$N" ]; then
  ko "contenu" "aucune fenêtre de lecture en 25 tentatives (verrou d'écriture)"
elif [ "$N" -ge "$SEUIL" ]; then
  ok "contenu" "$N sessions (seuil $SEUIL)"
else
  ko "contenu" "$N sessions < seuil $SEUIL — degrés périmés ? lancer repair-degrees"
fi

# --- Porte 3 : HISTORIQUE — le journal se lit, pas de panic -------------------
SID=$(ls "$STORE" 2>/dev/null | grep -m1 "^substrate.journal.session-" | sed "s/substrate.journal.//")
if [ -z "$SID" ]; then
  ko "historique" "aucune zone de journal dans le store"
else
  NE=""
  for i in $(seq 1 25); do
    printf '%s\n' "{\"tool\":\"session_load\",\"args\":{\"session_id\":\"$SID\",\"events_only\":true,\"events_limit\":3}}" \
      | timeout 45 "$OBRAIN" mcp --store "$STORE" 2>/tmp/_g3err-$ENVI > /tmp/_g3-$ENVI.json
    grep -q panicked /tmp/_g3err-$ENVI && { ko "historique" "PANIC du bridge sur $SID"; NE=panic; break; }
    NE=$(python3 - "$ENVI" <<'PY' 2>/dev/null
import json,sys
d=json.load(open(f"/tmp/_g3-{sys.argv[1]}.json"))
print(len((d.get("structuredContent") or json.loads(d["content"][0]["text"])).get("events",[])))
PY
)
    [ -n "$NE" ] && break
    sleep 2
  done
  if [ "$NE" = "panic" ]; then :
  elif [ -z "$NE" ]; then ko "historique" "aucune fenêtre de lecture (verrou)"
  elif [ "$NE" -ge 1 ]; then ok "historique" "$NE events lus sur ${SID:0:24}…"
  else ko "historique" "0 event — journal illisible"
  fi
fi

# --- Porte 3bis : COHÉRENCE — toute session listée doit être ouvrable --------
# L'invariant que le déploiement doit GARANTIR, pas réparer après coup.
# Le 2026-09-01 : 77 sessions listées en prod, 13 seulement résolvables. Les
# 48 archives de subagent sont nommées <uuid> nu sur le disque, alors que
# import_events force le préfixe « session- » sur tout identifiant : le nom du
# graphe ne peut structurellement jamais retrouver l'archive. Une conversation
# listée mais non ouvrable, c'est une conversation perdue du point de vue de
# l'utilisateur, même si l'archive est intacte.
# (2026-09-06) Résolution en 3 preuves, ordonnées par autorité :
#   1. ZONE JOURNAL dans le STORE (`substrate.journal.session-<id>`) — la
#      preuve d'ouvrabilité du monde store-backed : les sessions importées
#      sont lues via leur zone (mesuré 2026-09-06 : 122/135 en prod et
#      124/137 en dev n'ont QUE cette preuve, archives absentes) ;
#   2. `source_path` enregistré dans le graphe (croisements de homes :
#      sessions du store prod dont source_path pointe ~/.dsh-dev) ;
#   3. archive `session.jsonl*` sous le home de l'environnement (repli
#      historique — archives nommées <uuid> nu ou session-<uuid>).
ORPH=$(python3 - "$ENVI" "$HOME_DSH" "$STORE" <<'PY' 2>/dev/null
import json, sys, os, glob
envi, home, store = sys.argv[1], sys.argv[2], sys.argv[3]
try:
    d = json.load(open(f"/tmp/_g2-{envi}.json"))
    items = json.loads(d["content"][0]["text"])["sessions"]
except Exception:
    print("NA"); raise SystemExit
zones = set()
try:
    for f in os.listdir(store):
        if f.startswith("substrate.journal.session-"):
            zones.add("session-" + f[len("substrate.journal.session-"):])
except Exception:
    pass
def resolvable(item):
    sid = item.get("qn") or item.get("name") or ""
    # 1) zone journal du store (autoritatif en régime store-backed).
    if sid in zones:
        return True
    # 2) source_path du graphe (autoritatif — croisements de homes inclus).
    p = item.get("properties")
    try:
        props = json.loads(p) if isinstance(p, str) else (p or {})
    except Exception:
        props = {}
    sp = props.get("source_path")
    if sp and os.path.exists(sp):
        return True
    # 3) repli : archive dans le home de l'environnement.
    nu = sid[len("session-"):] if sid.startswith("session-") else sid
    for nom in (sid, nu):
        if glob.glob(os.path.join(home, "sessions", "*", nom, "session.jsonl*")):
            return True
    return False
orph = [i for i in items if not resolvable(i)]
print(f"{len(orph)}/{len(items)}")
PY
)
if [ "$ORPH" = "NA" ] || [ -z "$ORPH" ]; then
  ko "cohérence" "impossible de croiser le graphe et les archives"
else
  NB=${ORPH%%/*}; TOT=${ORPH##*/}
  if [ "$NB" = "0" ]; then
    ok "cohérence" "$TOT sessions listées, toutes ouvrables"
  else
    ko "cohérence" "$NB session(s) listée(s) sur $TOT SANS archive — réconciliation requise"
  fi
fi

# --- Porte 4 : STABILITÉ — pas de crash-loop ---------------------------------
# La porte qu'on oublie. Une sonde ponctuelle réussit dès le premier essai et
# déclare bon un service qui bat toutes les 5 s. is-active répond "active" en
# plein crash-loop, et StartLimitBurst ne peut jamais se déclencher.
R1=$(systemctl --user show "$UNITE" -p NRestarts --value)
echo "  ....  stabilité             observation 60 s (NRestarts=$R1)…"
sleep 60
R2=$(systemctl --user show "$UNITE" -p NRestarts --value)
A=$(systemctl --user is-active "$UNITE")
H=$(curl -s -o /dev/null -w "%{http_code}" --max-time 10 "http://127.0.0.1:$PORT/" || echo 000)
if [ "$R1" = "$R2" ] && [ "$A" = "active" ] && [ "$H" = "200" ]; then
  ok "stabilité" "NRestarts stable à $R2, actif, HTTP 200 après 60 s"
else
  ko "stabilité" "NRestarts $R1->$R2, état=$A, HTTP=$H"
fi

# --- Porte 5 : CONTINUITÉ D'ÉCRITURE (writer vivant + journal frais) --------
# « Que ça n'arrive plus » (2026-09-05) : après une bascule, le writer doit
# être vivant et la base ne doit pas être figée. Rejouable ; seuil par env
# (prod tolérant à l'inactivité : 60 min ; dev strict : 5 min).
SEUIL_CONT="${SEUIL_CONT:-$([ "$ENVI" = prod ] && echo 60 || echo 5)}"
W=$(pgrep -f "obrain bridge" 2>/dev/null | wc -l)
CONT="$OBRAIN continuity "$STORE" --stale-after-min "$SEUIL_CONT" --format json 2>/dev/null"
CONT_OUT=$("$OBRAIN" continuity "$STORE" --stale-after-min "$SEUIL_CONT" --format json 2>/dev/null)
CONT_RC=$?
if [ "$W" -ge 1 ] && [ "$CONT_RC" = "0" ]; then
  ok "continuité" "$W writer(s) obrain actifs ; store $STORE frais (< ${SEUIL_CONT} min) — $(printf '%s' "$CONT_OUT" | tr -d '\n' | cut -c1-140)"
else
  ko "continuité" "$W writer(s) obrain ; continuity exit=$CONT_RC (seuil ${SEUIL_CONT} min) : $(printf '%s' "$CONT_OUT" | tr -d '\n' | cut -c1-200)"
fi

echo
[ "$ECHECS" = "0" ] && { echo "== $ENVI : TOUTES LES PORTES PASSENT"; exit 0; }
echo "== $ENVI : $ECHECS PORTE(S) EN ÉCHEC"; exit 1
