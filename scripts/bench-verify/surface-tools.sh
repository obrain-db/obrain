#!/usr/bin/env bash
# Batterie de surface du bridge MCP Rust (`obrain mcp`, crates/obrain-cli).
#
# Principe (parité avec l'ancienne batterie C, en plus strict) : sur un store
# substrate jetable, CHAQUE outil du catalogue `tools/list` est appelé avec des
# arguments REPRÉSENTATIFS et doit répondre `structuredContent.status == "ok"`
# (isError false). L'ancienne batterie tolérait des erreurs métier pour les
# appels sans contexte (journal_import sans args, context_build sans session…) ;
# ici ces outils sont exercés AVEC des arguments complets → ils doivent PASSER
# comme les autres. Un mécanisme d'exceptions reste disponible
# (ACCEPT_ERRORS = "tool|sous-chaîne d'erreur attendue") pour les cas
# environnementaux légitimes — vide par défaut.
#
#  1. cargo build --release -p obrain-cli (features par défaut : gql,cypher,indexer)
#  2. store jetable : obrain init + INSERTs (fonctions fixture + arête CALLS)
#  3. `obrain mcp` (OBRAIN_WRITE=1) : un appel tools/call par outil de tools/list
#  4. exit≠0 si un outil manque du catalogue attendu ou échoue
# Résumé final : « N/M PASS ».
#
# Usage : scripts/bench-verify/surface-tools.sh
set -u
cd "$(dirname "$0")/../.." || exit 2
ROOT="$(pwd)"
BIN="$ROOT/target/release/obrain"

command -v python3 >/dev/null 2>&1 || { echo "ÉCHEC : python3 requis (parsing JSON des réponses)"; exit 2; }

echo "== [surface-tools] build release =="
cargo build --release -p obrain-cli || { echo "ÉCHEC : build"; exit 1; }

# Catalogue ATTENDU complet (31 outils) — un outil manquant = échec de la
# batterie (« manque »), même s'il n'apparaît pas dans tools/list.
EXPECTED="search_graph runtime_search sessions_list context_build checkpoint_material
import_events session_load journal_import memory_register memory_recall get_graph_schema
query query_graph get_code_snippet trace_path structural_roles list_projects search_code
index_status graph_link payload_get list_docs ingest_traces impact_set index_repository
create_property_index import_owners import_workspace import_docs import_sessions manage_adr"

# Erreurs métier acceptables, une par ligne : "<outil>|<sous-chaîne de l'erreur>".
# (Vide : la batterie exige "status":"ok" pour chaque outil exercé avec ses
# arguments représentatifs. Ajouter ici un couple quand un environnement
# légitime ne peut pas servir un outil — ex. feature absente du build.)
ACCEPT_ERRORS=""

TMP="$(mktemp -d)"
MCP_PID=""
cleanup() {
    [ -n "$MCP_PID" ] && kill "$MCP_PID" 2>/dev/null
    rm -rf "$TMP"
}
trap cleanup EXIT INT TERM

STORE="$TMP/t.obrain"
echo "== [surface-tools] store jetable : $STORE =="
"$BIN" init "$STORE" >/dev/null 2>&1 || { echo "ÉCHEC : obrain init"; exit 1; }

# Fixtures : fichier code (get_code_snippet), registres (imports), archives
# jsonl (import_sessions), skills (import_docs).
mkdir -p "$TMP/fixture" "$TMP/sessions" "$TMP/docs"
printf 'fn alpha() {}\nfn beta() {}\n' >"$TMP/fixture/code.rs"
printf '{"session-t1": "alice", "t2": "bob"}\n' >"$TMP/owners.json"
printf '{"w1": {"path": "%s", "title": "Projet W1", "sessionIds": ["session-t1", "t2"]}}\n' "$TMP" >"$TMP/workspaces.json"
printf '# Skill demo\n\n## Usage\nLa batterie lit ce contenu.\n' >"$TMP/docs/SKILL.md"
printf '%s\n' \
    '{"type":"session","id":"arch1","seq":0,"cwd":"/tmp","time":0,"data":{}}' \
    '{"type":"user/message","seq":1,"time":1,"data":{"role":"user","content":"Bonjour"}}' \
    '{"type":"tool/call","seq":2,"time":2,"data":{"callId":"c1","name":"read"}}' \
    '{"type":"tool/result","seq":3,"time":3,"data":{"message":{"content":[{"type":"text","toolCallId":"c1","text":"ok"}]}}}' \
    >"$TMP/sessions/s1.jsonl"

# INSERTs du mini-graphe (fonctions + CALLS + props file pour get_code_snippet).
"$BIN" query "$STORE" "INSERT (:Function {qualified_name: 'bench.repo.alpha', name: 'alpha_fn', file_path: '$TMP/fixture/code.rs', start_line: 1, end_line: 2})" >/dev/null 2>&1
"$BIN" query "$STORE" "INSERT (:Function {qualified_name: 'bench.repo.beta', name: 'beta_fn'})" >/dev/null 2>&1
"$BIN" query "$STORE" "MATCH (a:Function {qualified_name: 'bench.repo.alpha'}), (b:Function {qualified_name: 'bench.repo.beta'}) CREATE (a)-[:CALLS]->(b)" >/dev/null 2>&1
"$BIN" query "$STORE" "CREATE PROPERTY INDEX qualified_name IF NOT EXISTS" >/dev/null 2>&1 || true

# ── serveur MCP en écriture (un seul process pour toute la batterie) ──
coproc MCP { env OBRAIN_WRITE=1 OBRAIN_IDLE_MS=600000 "$BIN" mcp --store "$STORE" 2>"$TMP/mcp.err"; }
MCP_PID="$MCP_PID"

# Requête JSON-RPC : $1 = requête ; réponse complète sur stdout (1 ligne).
mcp_req() {
    printf '%s\n' "$1" >&"${MCP[1]}"
    local line=""
    IFS= read -r -t 120 line <&"${MCP[0]}" || { echo "__NO_RESPONSE__"; return 1; }
    printf '%s\n' "$line"
}

call_tool() { # $1 = name ; $2 = arguments JSON → verdict + détail
    local req resp
    req="{\"jsonrpc\":\"2.0\",\"id\":$RID,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}"
    resp="$(mcp_req "$req")" || { VERDICT="NO_RESPONSE"; DETAIL="timeout/EOF sur le bridge"; return; }
    IFS='|' read -r VERDICT DETAIL <<EOF
$(printf '%s' "$resp" | python3 -c '
import sys, json
try:
    d = json.loads(sys.stdin.read())
except Exception as e:
    print("PARSE_FAIL|réponse non JSON : %s" % e); sys.exit()
if "error" in d and "result" not in d:
    print("RPC_ERROR|" + str(d["error"].get("message", d["error"]))[:200]); sys.exit()
res = d.get("result") or {}
sc = res.get("structuredContent") or {}
if res.get("isError"):
    print("TOOL_ERROR|" + str((sc or {}).get("error", ""))[:300])
elif (sc or {}).get("status") == "ok":
    print("OK|")
else:
    print("BAD_STATUS|" + json.dumps(sc)[:200])
')
EOF
}

RID=1
# initialize (protocole) puis fixture runtime : session + ids des fonctions.
mcp_req '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}' >/dev/null
RID=$((RID + 1))
import_req="{\"jsonrpc\":\"2.0\",\"id\":$RID,\"method\":\"tools/call\",\"params\":{\"name\":\"import_events\",\"arguments\":{\"session_id\":\"fixture\",\"events\":[{\"type\":\"session\",\"id\":\"fixture\",\"seq\":0,\"time\":0,\"data\":{}},{\"type\":\"user/message\",\"seq\":1,\"time\":1,\"data\":{\"role\":\"user\",\"content\":\"Bonjour\"}}]}}}"
mcp_req "$import_req" >/dev/null
RID=$((RID + 1))
# ids réels des fonctions fixture (memory_register/recall exigent des ids).
# NB : ne JAMAIS exécuter mcp_req dans un pipeline — le sous-shell du pipeline
# ferme les fds du coproc (EBADF) — capture puis parse en deux temps.
ID_REQ="{\"jsonrpc\":\"2.0\",\"id\":$RID,\"method\":\"tools/call\",\"params\":{\"name\":\"query\",\"arguments\":{\"query\":\"MATCH (n:Function {name:'alpha_fn'}) RETURN id(n)\"}}}"
ID1_RESP="$(mcp_req "$ID_REQ")"
ID1="$(printf '%s' "$ID1_RESP" | python3 -c '
import sys, json
try:
    d = json.loads(sys.stdin.read())
    rows = (d.get("result") or {}).get("structuredContent", {}).get("rows") or []
    print(int(rows[0][0]) if rows and rows[0] else "")
except Exception:
    print("")
')"
RID=$((RID + 1))
ID_REQ="{\"jsonrpc\":\"2.0\",\"id\":$RID,\"method\":\"tools/call\",\"params\":{\"name\":\"query\",\"arguments\":{\"query\":\"MATCH (n:Function {name:'beta_fn'}) RETURN id(n)\"}}}"
ID2_RESP="$(mcp_req "$ID_REQ")"
ID2="$(printf '%s' "$ID2_RESP" | python3 -c '
import sys, json
try:
    d = json.loads(sys.stdin.read())
    rows = (d.get("result") or {}).get("structuredContent", {}).get("rows") or []
    print(int(rows[0][0]) if rows and rows[0] else "")
except Exception:
    print("")
')"
RID=$((RID + 1))
[ -n "$ID1" ] && [ -n "$ID2" ] || { echo "ÉCHEC : ids des fonctions fixture introuvables (ID1=$ID1 ID2=$ID2)"; cat "$TMP/mcp.err" | tail -5; exit 1; }

# ── catalogue : chaque outil reçoit des arguments représentatifs ──
# (remplacements __TMP__/__ID1__/__ID2__ après le case)
tool_args() {
    case "$1" in
        search_graph)        echo '{"query":"alpha_fn","limit":5}' ;;
        runtime_search)      echo '{"query":"Bonjour","limit":3}' ;;
        sessions_list)       echo '{}' ;;
        context_build)       echo '{"session_id":"session-fixture","limit":3,"subject":"alpha"}' ;;
        checkpoint_material) echo '{"session_id":"session-fixture"}' ;;
        import_events)       echo '{"session_id":"fixture","events":[{"type":"session","id":"fixture","seq":0,"time":0,"data":{}},{"type":"user/message","seq":1,"time":1,"data":{"role":"user","content":"Bonjour"}}]}' ;;
        session_load)        echo '{"session_id":"session-fixture","events_limit":10}' ;;
        journal_import)      echo '{"session_id":"session-fixture","lines":["{\"type\":\"user/message\",\"seq\":9,\"time\":9,\"data\":{\"role\":\"user\",\"content\":\"ligne batterie\"}}"]}' ;;
        memory_register)     echo '{"nodes":[__ID1__,__ID2__],"prediction_error":0.3}' ;;
        memory_recall)       echo '{"cues":[__ID1__],"limit":3}' ;;
        get_graph_schema)    echo '{}' ;;
        query)               echo '{"query":"MATCH (n) RETURN count(n) AS total","language":"gql","limit":20}' ;;
        query_graph)         echo '{"query":"MATCH (n) RETURN count(n) AS n","max_rows":10,"format":"json"}' ;;
        get_code_snippet)    echo '{"qualified_name":"bench.repo.alpha"}' ;;
        trace_path)          echo '{"qualified_name":"bench.repo.beta","direction":"inbound","depth":1}' ;;
        structural_roles)    echo '{"node":"bench.repo.beta"}' ;;
        list_projects)       echo '{}' ;;
        search_code)         echo '{"pattern":"alpha","limit":5}' ;;
        index_status)        echo '{}' ;;
        graph_link)          echo '{"source":{"qn":"bench.repo.gamma","label":"Function"},"target":{"qn":"bench.repo.delta","label":"Function"},"type":"CALLS"}' ;;
        payload_get)         echo '{"qn":"bench.repo.alpha"}' ;;
        list_docs)           echo '{}' ;;
        ingest_traces)       echo '{"traces":[{"caller":"bench.repo.gamma","callee":"bench.repo.delta","count":1}]}' ;;
        impact_set)          echo '{"node":"bench.repo.beta","direction":"inbound","depth":1}' ;;
        index_repository)    echo '{"path":"__TMP__/fixture"}' ;;
        create_property_index) echo '{"property":"qualified_name"}' ;;
        import_owners)       echo '{"registry":"__TMP__/owners.json"}' ;;
        import_workspace)    echo '{"registry":"__TMP__/workspaces.json"}' ;;
        import_docs)         echo '{"file":"__TMP__/docs/SKILL.md","kind":"Skill"}' ;;
        import_sessions)     echo '{"dir":"__TMP__/sessions"}' ;;
        manage_adr)          echo '{"mode":"update","project":"bench","path":"001-bridge","content":"# ADR 001\n\n## Contexte\nContexte batterie.\n\n## Décision\nBridge Rust.\n"}' ;;
        *)                   echo '' ;;
    esac
}

# Vérifie que le catalogue attendu est présent dans tools/list.
TL="$(mcp_req '{"jsonrpc":"2.0","id":'$RID',"method":"tools/list","params":{}}')"
RID=$((RID + 1))
LISTED="$(printf '%s' "$TL" | python3 -c '
import sys, json
try:
    print("\n".join(t["name"] for t in json.load(sys.stdin)["result"]["tools"]))
except Exception as e:
    print(""); import sys as s; s.stderr.write("tools/list illisible: %s\n" % e)
')"
MISSING=0
for want in $EXPECTED; do
    if ! printf '%s\n' "$LISTED" | grep -qx "$want"; then
        echo "MANQUE au tools/list : $want"
        MISSING=1
    fi
done
[ "$MISSING" -eq 0 ] || { echo "ÉCHEC : catalogue incomplet"; exit 1; }

echo "== [surface-tools] exécution outil par outil =="
M=0
PASS=0
FAILED_TOOLS=""
for tool in $LISTED; do
    M=$((M + 1))
    args="$(tool_args "$tool")"
    if [ -z "$args" ]; then
        echo "SKIP  $tool (pas d'arguments représentatifs définis)"
        FAILED_TOOLS="$FAILED_TOOLS $tool"
        continue
    fi
    args="${args//__TMP__/$TMP}"
    args="${args//__ID1__/$ID1}"
    args="${args//__ID2__/$ID2}"
    call_tool "$tool" "$args"
    if [ "$VERDICT" = "OK" ]; then
        PASS=$((PASS + 1))
        echo "PASS  $tool"
    else
        # Erreur métier acceptée ? (liste ACCEPT_ERRORS "tool|sous-chaîne")
        accepted=""
        while IFS='|' read -r atool asub; do
            [ -n "$atool" ] || continue
            if [ "$atool" = "$tool" ] && printf '%s' "$DETAIL" | grep -qF "$asub"; then
                accepted="1"
                break
            fi
        done <<EOF
$ACCEPT_ERRORS
EOF
        if [ -n "$accepted" ]; then
            PASS=$((PASS + 1))
            echo "PASS  $tool (erreur métier acceptée : $DETAIL)"
        else
            FAILED_TOOLS="$FAILED_TOOLS $tool"
            echo "FAIL  $tool ($VERDICT : $DETAIL)"
        fi
    fi
done

echo "== [surface-tools] résumé : $PASS/$M PASS =="
if [ "$PASS" -eq "$M" ]; then
    echo "SURFACE OK — chaque outil du catalogue répond status ok sur store jetable."
    exit 0
else
    echo "SURFACE ÉCHEC — outils en défaut :$FAILED_TOOLS"
    echo "--- stderr du bridge (fin) ---"
    tail -10 "$TMP/mcp.err" 2>/dev/null || true
    exit 1
fi
