#!/usr/bin/env python3
# Orchestrateur ENGINE008 GPU 9910 -- configuration de VALIDATION (nouvelle).
import base64, json, math, os, signal, struct, subprocess, sys, time, urllib.request, urllib.error

F    = "/home/triviere/projects/llama-obrain-cuda-f1a"
MC   = "/home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf"
D    = "/tmp/gpu9910"
# [§3] repertoire de travail NEUF : `jspace_kv_dir` vaut "kv-states" RELATIF au
# CWD, donc ce CWD isole les nouveaux etats. Aucun ancien etat n'est touche.
WORK = D + "/work"
PORT = 9910
BASE = f"http://127.0.0.1:{PORT}"
STARTUP_MAX_S = 900          # preenregistre
REQUEST_MAX_S = 300          # preenregistre
READY = 0

def log(m):
    line = f"[{time.strftime('%H:%M:%S')}] {m}"
    print(line, flush=True)
    with open(f"{D}/orchestrator.log", "a") as f: f.write(line + "\n")

def probe(tag):
    global READY
    READY += 1
    try:
        with urllib.request.urlopen(BASE + "/health", timeout=4) as r:
            return r.status, r.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e: return e.code, e.read().decode("utf-8", "replace")
    except Exception as e: return 0, f"{type(e).__name__}: {e}"

def port_free():
    out = subprocess.run(["ss","-tln"], capture_output=True, text=True).stdout
    return f":{PORT} " not in out and f":{PORT}\n" not in out

def dump_stacks(pid):
    log("--- CAPTURE DES PILES (processus encore vivant) ---")
    try:
        ts = os.listdir(f"/proc/{pid}/task")
        log(f"  {len(ts)} threads")
        for t in ts:
            try: st = open(f"/proc/{pid}/task/{t}/stat").read().split()[2]
            except Exception: st = "?"
            try: wc = open(f"/proc/{pid}/task/{t}/wchan").read().strip()
            except Exception: wc = "?"
            try:
                sysc = open(f"/proc/{pid}/task/{t}/syscall").read().split()[0]
            except Exception: sysc = "?"
            log(f"  thread {t} etat={st} wchan={wc} syscall={sysc}")
    except Exception as e: log(f"  (piles indisponibles: {e})")

def post(name, body, tag=None):
    t0 = time.time(); n = tag or name
    req = urllib.request.Request(BASE + "/jspace/inject_knowledge",
        data=json.dumps(body).encode(), headers={"Content-Type":"application/json"})
    code, raw, err = 0, None, None
    try:
        with urllib.request.urlopen(req, timeout=REQUEST_MAX_S) as r:
            code = r.status; raw = r.read().decode("utf-8","replace")
    except urllib.error.HTTPError as e:
        code = e.code; raw = e.read().decode("utf-8","replace")
    except Exception as e: err = f"{type(e).__name__}: {e}"
    dt = time.time() - t0
    log(f"[{n}] http={code} duree={dt:.3f}s err={err}")
    with open(f"{D}/{n}.meta","w") as f: f.write(f"http={code} duration_s={dt:.6f} error={err}\n")
    if raw is not None:
        with open(f"{D}/{n}.json","w") as f: f.write(raw)
    with open(f"{D}/phases.log","a") as f: f.write(f"{n} t0={t0:.3f} t1={time.time():.3f} http={code} d={dt:.3f} err={err}\n")
    try: return code, (json.loads(raw) if raw else None)
    except Exception: return code, None

def logits(r):
    b = base64.b64decode(r.get("first_logits_base64",""))
    return list(struct.unpack(f"<{len(b)//4}f", b)) if b else []

def compare(b, c, label):
    lB, lC = logits(b), logits(c)
    if not lB or not lC or len(lB) != len(lC):
        log(f"  {label}: logits indisponibles ({len(lB)}/{len(lC)})"); return False
    maxd = 0.0; l2 = 0.0; ident = True; aB = aC = 0
    for k,(x,y) in enumerate(zip(lB,lC)):
        if math.isfinite(x) and math.isfinite(y):
            d = abs(x-y)
            if d > maxd: maxd = d
            l2 += d*d
            if struct.pack("<f",x) != struct.pack("<f",y): ident = False
        if x > lB[aB]: aB = k
        if y > lC[aC]: aC = k
    log(f"  {label}: FINITE=YES MAX_ABS_DIFF={maxd:.9g} L2={math.sqrt(l2):.9g} "
        f"ARGMAX={aB}/{aC} BINARY_IDENTICAL={'YES' if ident else 'NO'}")
    return ident

def main():
    os.makedirs(D, exist_ok=True); os.makedirs(WORK, exist_ok=True)
    for f in ("orchestrator.log","phases.log"): open(f"{D}/{f}","w").close()
    log("=== ORCHESTRATEUR GPU 9910 -- CONFIGURATION DE VALIDATION (NOUVELLE) ===")
    log("Cette configuration n'est PAS une restauration de la config historique :")
    log("  historique : Qwen3.8-27B-Q4_K_M.gguf, --host 0.0.0.0, ctx 32768, ngl 99")
    log("  validation : Qwen3.8-27B-OBRAIN-v3.gguf, 127.0.0.1, ctx 1024, ngl 99")
    if not port_free():
        log(f"ABANDON : le port {PORT} est DEJA OCCUPE."); return 40
    log(f"port {PORT} verifie LIBRE avant lancement")
    os.makedirs(WORK + "/kv-states", exist_ok=True)

    cmd = [f"{F}/build-cuda/bin/llama-server", "-m", MC,
           "--host","127.0.0.1","--port",str(PORT),
           "-ngl","99","--ctx-size","1024","--parallel","3","--jinja"]
    log("COMMANDE : " + " ".join(cmd))
    log(f"CWD (isole les nouveaux etats) : {WORK}")
    slog = open(f"{D}/server.log","w")
    p = subprocess.Popen(cmd, stdout=slog, stderr=subprocess.STDOUT, cwd=WORK,
        env={**os.environ, "OBRAIN_KV_STATES_MAX_BYTES": str(2*1024**3),
             "CUDA_VISIBLE_DEVICES":"0"}, preexec_fn=os.setsid)
    log(f"SERVER_PID={p.pid}")
    open(f"{D}/pid.txt","w").write(str(p.pid))

    t0 = time.time(); ready = False
    while time.time() - t0 < STARTUP_MAX_S:
        if p.poll() is not None:
            log(f"SERVEUR TERMINE rc={p.returncode}"); 
            for l in open(f"{D}/server.log").read().splitlines()[-25:]: log("  "+l)
            return 41
        code, body = probe("startup")
        if code == 200 and '"ok"' in body:
            ready = True; log(f"PRET apres {time.time()-t0:.1f}s (probe#{READY})"); break
        time.sleep(3)
    if not ready: log(f"NON PRET apres {STARTUP_MAX_S}s -- aucun modele requis"); return 42

    # [§2] PLACEMENT GPU REEL, lu dans le journal
    slog.flush()
    txt = open(f"{D}/server.log").read()
    log("--- PLACEMENT GPU (lu dans le journal du serveur) ---")
    for l in txt.splitlines():
        ll = l.lower()
        if any(k in ll for k in ("offload","cuda","backend","device","buffer size","recurrent","attn")):
            log("  " + l.strip()[:200])
    log("--- fin du placement ---")

    ids = [int(x) for x in open("/tmp/kv02/ids59.txt").read().split()]
    P58, ID59 = ids[:58], ids[58]
    log(f"59 IDs autoritaires, prefixe={len(P58)}, 59e ID={ID59}")

    codeA, A = post("A", {"fact_token_ids":P58,"extract_only":True,"verify":True})
    if codeA != 200: dump_stacks(p.pid); return 43
    ref = (A.get("text","") or "").replace("kv_state:","")
    log(f"A: ref={ref} blob_bytes={A.get('blob_bytes')} fact_tokens={A.get('fact_tokens')}")

    codeB, B = post("B", {"fact_token_ids":P58,"prompt":"OK","n_generate":32,"greedy":True,"verify":True})
    if codeB != 200: dump_stacks(p.pid); return 44
    codeC, C = post("C", {"state_ref":ref,"prompt":"OK","n_generate":32,"greedy":True,"verify":True})
    if codeC != 200: dump_stacks(p.pid); return 45
    json.dump({"A":A,"B":B,"C":C}, open(f"{D}/sequential.json","w"), indent=1)

    log("=== VALIDATION GPU SEQUENTIELLE ===")
    ident = compare(B, C, "GPU seq B/C")
    log(f"  B: prefix_tok={B.get('prefix_token_count')} imported={B.get('state_imported')} "
        f"prep_ms={B.get('ms_preparation_total')} ids={B.get('decoded_ids_json')} pos={B.get('decoded_pos_json')} stop={B.get('stop_reason')}")
    log(f"  C: prefix_tok={C.get('prefix_token_count')} imported={C.get('state_imported')} "
        f"prep_ms={C.get('ms_preparation_total')} ids={C.get('decoded_ids_json')} pos={C.get('decoded_pos_json')} stop={C.get('stop_reason')}")
    log(f"  C ms: lookup={C.get('ms_state_lookup')} compat={C.get('ms_compatibility')} "
        f"blob_sha={C.get('ms_blob_sha256')} import={C.get('ms_state_import')}")
    log(f"  TOKEN_IDS_MATCH={'YES' if B.get('decoded_ids_json')==C.get('decoded_ids_json') else 'NO'}"
        f"  TEXT_MATCH={'YES' if B.get('text')==C.get('text') else 'NO'}")
    log(f"  TEXT B={B.get('text','')!r}")
    log(f"  TEXT C={C.get('text','')!r}")
    log(f"GPU_SEQUENTIAL_RESUME={'PASS' if ident and C.get('prefix_token_count')==0 else 'FAIL'}")
    log("=== FIN SEQUENTIEL ===")
    p.terminate()
    return 0

if __name__ == "__main__":
    rc = 1
    try: rc = main()
    finally:
        try:
            pid = int(open(f"{D}/pid.txt").read().strip())
            os.killpg(os.getpgid(pid), signal.SIGKILL); print(f"[cleanup] {pid} arrete", flush=True)
        except Exception as e: print(f"[cleanup] {e}", flush=True)
    sys.exit(rc)
