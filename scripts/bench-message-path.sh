#!/usr/bin/env bash
# bench-message-path.sh — banc du chemin d envoi de message (P1/P2 perf).
# Pose par la verification externe du 2026-09-09 (PO). Store de bench fige :
#   ~/bench-stores/runtime-dev-snapshot-20260909.obrain  (copie de runtime-dev,
#   51 863 messages, compteurs PERIMES a dessein — le repro de P1 est dedans).
# Baseline mesuree le 09/09 (binaire deploye) :
#   ouverture RO        : ~886 ms   (recomptage 334k noeuds — P1)
#   runtime_search chaud: ~1100 ms  (P2)
#   query GQL chaude    : ~0.1 ms   (reference moteur)
#   context_build chaud : ~0.1 ms   (OK, cache)
# Cibles DoD : ouverture <= 30 ms SANS compact prealable ; runtime_search < 50 ms chaud.
set -u
S="${VIZ_STORE:-$HOME/bench-stores/runtime-dev-snapshot-20260909.obrain}"
B="${OBRAIN_BIN:-$HOME/.local/share/obrain-harness/bin/obrain}"
echo "store: $S"; echo "bin  : $B"
python3 - "$S" "$B" <<PY
import subprocess,time,json,os,sys
S,B=sys.argv[1],sys.argv[2]
env={**os.environ,"OBRAIN_STORE":S}
# 1) ouverture (3 process frais)
opens=[]
for i in range(3):
    t0=time.time()
    p=subprocess.run([B,"bridge"],input=json.dumps({"tool":"query","args":{"query":"MATCH (n:Session) RETURN count(n) AS c","language":"gql"}})+"\n",capture_output=True,text=True,env=env)
    opens.append((time.time()-t0)*1000)
print(f"ouverture+requete (froid) : min {min(opens):.0f} / max {max(opens):.0f} ms")
# 2) chaud : runtime_search / query / context_build
p=subprocess.Popen([B,"bridge"],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True,env=env)
def call(tool,args):
    t0=time.time(); p.stdin.write(json.dumps({"tool":tool,"args":args})+"\n"); p.stdin.flush()
    p.stdout.readline(); return (time.time()-t0)*1000
call("runtime_search",{"query":"warmup","limit":3})
for tool,args in [("runtime_search",{"query":"perf harness","limit":5}),
                  ("query",{"query":"MATCH (n:Message) RETURN count(n) AS c","language":"gql"}),
                  ("context_build",{"query":"perf harness","budget_tokens":1500})]:
    ts=[call(tool,args) for _ in range(3)]
    print(f"{tool:>16} chaud : med {sorted(ts)[1]:.1f} ms  (cible: <50)")
p.kill()
PY
