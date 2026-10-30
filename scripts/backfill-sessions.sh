#!/usr/bin/env bash
# =============================================================================
# Backfill des métadonnées de session perdues à l'import.
# Réf : docs/PROTOCOLE-DEPLOIEMENT.md — note « v0.0.1 se désorganise ».
#
# Usage : backfill-sessions.sh --env {dev|prod} [--appliquer]
#
# Sans --appliquer : simulation, aucune écriture (défaut délibéré).
#
# CE QUE ÇA RÉPARE
# Les en-têtes des session.jsonl.zstd portent origin="subagent" et
# parentSession sur 48 sessions ; le graphe les a perdus (0/77). L'UI masque
# les subagents via origin === 'subagent' — sans la propriété, aucun n'est
# masqué et la liste principale est polluée à 80 %.
# source_path, lui, n'est JAMAIS écrit par import_events (le code ne fait que
# le lire) : sans lui, l'ouverture d'une conversation ne trouve pas son
# archive.
#
# COMMENT
# On rejoue l'événement d'en-tête via import_events — le chemin d'import
# normal, qui sait déjà écrire origin et parentSession. source_path est posé
# par une requête GQL séparée, puisque aucun chemin d'import ne l'écrit.
#
# Le service de l'environnement visé DOIT être arrêté : l'écriture prend un
# verrou exclusif, et le disputer à un bridge vivant donne un résultat
# non déterministe.
# =============================================================================
set -uo pipefail

ENVI=""; APPLIQUER=0
while [ $# -gt 0 ]; do
  case "$1" in
    --env) ENVI="${2:-}"; shift 2 ;;
    --appliquer) APPLIQUER=1; shift ;;
    *) echo "argument inconnu : $1"; exit 2 ;;
  esac
done
[ -z "$ENVI" ] && { echo "--env {dev|prod} est obligatoire (aucun défaut)"; exit 2; }

RACINE="$HOME/.local/share/obrain-harness"
OBRAIN="$RACINE/bin/obrain"
case "$ENVI" in
  prod) UNITE=dsh-web;      HOME_DSH="$HOME/.dsh";     STORE="$RACINE/data/runtime.obrain" ;;
  dev)  UNITE=dsh-dev-3083; HOME_DSH="$HOME/.dsh-dev"; STORE="$RACINE/data/runtime-dev.obrain" ;;
  *) echo "env inconnu : $ENVI"; exit 2 ;;
esac

if [ "$APPLIQUER" = "1" ] && systemctl --user is-active "$UNITE" >/dev/null 2>&1; then
  echo "REFUS : $UNITE tourne encore. L'écriture exige le verrou exclusif."
  echo "        systemctl --user stop $UNITE   puis relancer."
  exit 1
fi

echo "== backfill $ENVI — archives: $HOME_DSH/sessions  store: $(basename "$STORE")"
[ "$APPLIQUER" = "0" ] && echo "   (SIMULATION — ajouter --appliquer pour écrire)"

vus=0; sub=0; ecrits=0; echecs=0
while IFS= read -r f; do
  vus=$((vus+1))
  H=$(zstd -dc "$f" 2>/dev/null | head -1)
  [ -z "$H" ] && continue
  LIGNE=$(printf '%s' "$H" | python3 -c '
import json,sys
try: h=json.load(sys.stdin)
except Exception: sys.exit(0)
if h.get("type")!="session" or not h.get("id"): sys.exit(0)
print(json.dumps({"id":h["id"],"origin":h.get("origin",""),"parent":h.get("parentSession","")}))
')
  [ -z "$LIGNE" ] && continue
  ID=$(printf '%s' "$LIGNE" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')
  ORI=$(printf '%s' "$LIGNE" | python3 -c 'import json,sys; print(json.load(sys.stdin)["origin"])')
  [ "$ORI" = "subagent" ] && sub=$((sub+1))

  if [ "$APPLIQUER" = "1" ]; then
    # 1. rejoue l'en-tête : import_events écrit origin (si "subagent") et parentSession
    printf '%s\n' "{\"tool\":\"import_events\",\"args\":{\"project\":\"dsh-runtime\",\"session_id\":\"$ID\",\"events\":[$H]}}" \
      | OBRAIN_WRITE=1 timeout 120 "$OBRAIN" mcp --store "$STORE" >/dev/null 2>&1 \
      && ecrits=$((ecrits+1)) || echecs=$((echecs+1))
    # 2. source_path : aucun chemin d'import ne l'écrit, on le pose en GQL
    ECH=$(printf '%s' "$f" | sed "s/'/''/g")
    OBRAIN_WRITE=1 timeout 120 "$OBRAIN" query "$STORE" \
      "MATCH (s:Session {qualified_name: '$ID'}) SET s.source_path = '$ECH' RETURN 1" >/dev/null 2>&1
  fi
done < <(find "$HOME_DSH/sessions" -name "session.jsonl.zstd" 2>/dev/null)

echo "   en-têtes lus        : $vus"
echo "   marqués subagent    : $sub"
[ "$APPLIQUER" = "1" ] && echo "   import_events OK    : $ecrits (échecs: $echecs)"
echo
echo "== contrôle après coup"
for i in $(seq 1 20); do
  printf '%s\n' '{"tool":"sessions_list","args":{}}' | timeout 30 "$OBRAIN" mcp --store "$STORE" 2>/dev/null > /tmp/_bf.json
  grep -q sessions /tmp/_bf.json && break; sleep 2
done
python3 - <<'PY'
import json, collections
try: d=json.load(open("/tmp/_bf.json"))
except Exception: print("   (lecture impossible)"); raise SystemExit
s=json.loads(d["content"][0]["text"])["sessions"]
c=collections.Counter()
for x in s:
    try: p=json.loads(x["properties"])
    except Exception: p={}
    for k in ("origin","parentSession","source_path"):
        if p.get(k): c[k]+=1
print(f"   sessions        : {len(s)}")
for k in ("origin","parentSession","source_path"):
    print(f"   {k:15} : {c[k]}/{len(s)}")
PY
