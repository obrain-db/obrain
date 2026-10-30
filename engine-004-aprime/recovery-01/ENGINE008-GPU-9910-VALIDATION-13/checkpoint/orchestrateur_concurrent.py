#!/usr/bin/env python3
# [§2] placement GPU + [§6] validation concurrente bornee (2 requetes).
import base64, json, math, os, signal, struct, subprocess, sys, threading, time
import urllib.request, urllib.error
F="/home/triviere/projects/llama-obrain-cuda-f1a"
MC="/home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf"
D="/tmp/gpu9910b"; WORK=D+"/work"; PORT=9910; BASE=f"http://127.0.0.1:{PORT}"
STARTUP_MAX_S=900; REQUEST_MAX_S=300; READY=0
def log(m):
    l=f"[{time.strftime('%H:%M:%S')}] {m}"; print(l,flush=True)
    open(f"{D}/orchestrator.log","a").write(l+"\n")
def probe():
    global READY; READY+=1
    try:
        with urllib.request.urlopen(BASE+"/health",timeout=4) as r: return r.status,r.read().decode()
    except urllib.error.HTTPError as e: return e.code,e.read().decode()
    except Exception as e: return 0,f"{type(e).__name__}: {e}"
def port_free():
    o=subprocess.run(["ss","-tln"],capture_output=True,text=True).stdout
    return f":{PORT} " not in o
def stacks(pid):
    log("--- PILES ---")
    try:
        for t in os.listdir(f"/proc/{pid}/task"):
            try: st=open(f"/proc/{pid}/task/{t}/stat").read().split()[2]
            except Exception: st="?"
            try: wc=open(f"/proc/{pid}/task/{t}/wchan").read().strip()
            except Exception: wc="?"
            log(f"  thread {t} etat={st} wchan={wc}")
    except Exception as e: log(f"  indisponible: {e}")
def post(name, body):
    t0=time.time()
    req=urllib.request.Request(BASE+"/jspace/inject_knowledge",data=json.dumps(body).encode(),
                               headers={"Content-Type":"application/json"})
    code,raw,err=0,None,None
    try:
        with urllib.request.urlopen(req,timeout=REQUEST_MAX_S) as r: code=r.status; raw=r.read().decode("utf-8","replace")
    except urllib.error.HTTPError as e: code=e.code; raw=e.read().decode("utf-8","replace")
    except Exception as e: err=f"{type(e).__name__}: {e}"
    dt=time.time()-t0
    log(f"[{name}] http={code} duree={dt:.3f}s err={err}")
    open(f"{D}/{name}.meta","w").write(f"http={code} duration_s={dt:.6f} error={err}\n")
    if raw is not None: open(f"{D}/{name}.json","w").write(raw)
    open(f"{D}/phases.log","a").write(f"{name} t0={t0:.3f} t1={time.time():.3f} http={code} d={dt:.3f} err={err}\n")
    try: return code,(json.loads(raw) if raw else None)
    except Exception: return code,None
def logits(r):
    b=base64.b64decode(r.get("first_logits_base64",""))
    return list(struct.unpack(f"<{len(b)//4}f",b)) if b else []
def cmp(b,c,label):
    lB,lC=logits(b),logits(c)
    if not lB or not lC or len(lB)!=len(lC):
        log(f"  {label}: logits indisponibles ({len(lB)}/{len(lC)})"); return False
    md=0.0;l2=0.0;idn=True;aB=aC=0
    for k,(x,y) in enumerate(zip(lB,lC)):
        if math.isfinite(x) and math.isfinite(y):
            d=abs(x-y); md=max(md,d); l2+=d*d
            if struct.pack("<f",x)!=struct.pack("<f",y): idn=False
        if x>lB[aB]: aB=k
        if y>lC[aC]: aC=k
    log(f"  {label}: MAX_ABS_DIFF={md:.9g} L2={math.sqrt(l2):.9g} ARGMAX={aB}/{aC} BINARY_IDENTICAL={'YES' if idn else 'NO'}")
    return idn

def main():
    os.makedirs(D,exist_ok=True); os.makedirs(WORK+"/kv-states",exist_ok=True)
    for f in ("orchestrator.log","phases.log"): open(f"{D}/{f}","w").close()
    log("=== GPU 9910 : PLACEMENT + VALIDATION CONCURRENTE ===")
    if not port_free(): log(f"ABANDON : {PORT} occupe"); return 40
    log(f"port {PORT} LIBRE")
    # -lv 4 pour faire apparaitre le placement
    cmd=[f"{F}/build-cuda/bin/llama-server","-m",MC,"--host","127.0.0.1","--port",str(PORT),
         "-ngl","99","--ctx-size","1024","--parallel","3","--jinja","-lv","4"]
    log("COMMANDE : "+" ".join(cmd))
    p=subprocess.Popen(cmd,stdout=open(f"{D}/server.log","w"),stderr=subprocess.STDOUT,
        cwd=WORK,env={**os.environ,"OBRAIN_KV_STATES_MAX_BYTES":str(2*1024**3),
        "CUDA_VISIBLE_DEVICES":"0"},preexec_fn=os.setsid)
    log(f"SERVER_PID={p.pid}"); open(f"{D}/pid.txt","w").write(str(p.pid))
    t0=time.time(); ready=False
    while time.time()-t0<STARTUP_MAX_S:
        if p.poll() is not None:
            log(f"SERVEUR TERMINE rc={p.returncode}")
            for l in open(f"{D}/server.log").read().splitlines()[-20:]: log("  "+l)
            return 41
        c,b=probe()
        if c==200 and '"ok"' in b: ready=True; log(f"PRET apres {time.time()-t0:.1f}s"); break
        time.sleep(3)
    if not ready: log("NON PRET"); return 42
    log("--- PLACEMENT GPU (recherche large dans le journal) ---")
    txt=open(f"{D}/server.log").read()
    kept=0
    for l in txt.splitlines():
        ll=l.lower()
        if any(k in ll for k in ("offload","cuda","backend","buffer size","assigned to device",
                                 "cpu buffer","kv cache","recurrent","attn","tensor","vram","graph")):
            log("  "+l.strip()[:220]); kept+=1
    log(f"--- {kept} lignes de placement ---")
    # ecrire le recapitulatif de placement a part
    open(f"{D}/gpu_placement.log","w").write("\n".join(
        l for l in txt.splitlines() if any(k in l.lower() for k in
        ("offload","cuda","backend","buffer size","assigned to device","cpu buffer","kv cache","recurrent"))))

    ids=[int(x) for x in open("/tmp/kv02/ids59.txt").read().split()]
    P58=ids[:58]
    # ---- ENTREE 1 : autoritaire
    E1={"kind":"ids","body":P58}
    # ---- ENTREE 2 : texte natif
    E2_TEXT="Réponds uniquement : BONJOUR."
    log(f"ENTREE 1 : 59 IDs autoritaires (prefixe {len(P58)})")
    log(f"ENTREE 2 : fact_text={E2_TEXT!r} (rendu et tokenise NATIVEMENT par le serveur)")
    codes,refs,ctrls={},{},{}
    def run_entry(tag,body):
        c,r=post(f"{tag}_A",{**body,"extract_only":True,"verify":True}); codes[tag+"_A"]=c
        if c!=200: return False
        refs[tag]=(r.get("text","") or "").replace("kv_state:","")
        log(f"  {tag} A: ref={refs[tag]} blob_bytes={r.get('blob_bytes')} fact_tokens={r.get('fact_tokens')}")
        c,r=post(f"{tag}_CTRL",{**body,"prompt":"OK","n_generate":32,"greedy":True,"verify":True}); codes[tag+"_CTRL"]=c
        if c!=200: return False
        ctrls[tag]=r
        log(f"  {tag} CTRL: ids={r.get('decoded_ids_json')} prefix_tok={r.get('prefix_token_count')} text={r.get('text','')!r}")
        return True
    if not run_entry("E1",{"fact_token_ids":P58}): stacks(p.pid); return 43
    if not run_entry("E2",{"fact_text":E2_TEXT}): stacks(p.pid); return 44
    log("=== RESTAURATIONS SIMULTANEES (2 requetes en parallele) ===")
    res={}
    def restore(tag):
        c,r=post(f"{tag}_REST",{"state_ref":refs[tag],"prompt":"OK","n_generate":32,"greedy":True,"verify":True})
        res[tag]=(c,r)
    th=[threading.Thread(target=restore,args=(t,)) for t in ("E1","E2")]
    t0=time.time()
    for t in th: t.start()
    for t in th: t.join()
    log(f"les deux restaurations ont dure {time.time()-t0:.3f}s au total")
    ok=True
    for t in ("E1","E2"):
        c,r=res[t]
        if c!=200: log(f"  {t} REST: http={c} ECHEC"); ok=False; continue
        same=cmp(ctrls[t],r,f"{t} REST vs SON controle")
        log(f"  {t} REST: prefix_tok={r.get('prefix_token_count')} imported={r.get('state_imported')} "
            f"ids={r.get('decoded_ids_json')} text={r.get('text','')!r}")
        log(f"  {t} TOKEN_IDS_MATCH={'YES' if ctrls[t].get('decoded_ids_json')==r.get('decoded_ids_json') else 'NO'}"
            f"  TEXT_MATCH={'YES' if ctrls[t].get('text')==r.get('text') else 'NO'}")
        log(f"  {t} prep_ms={r.get('ms_preparation_total')} lookup={r.get('ms_state_lookup')} "
            f"import={r.get('ms_state_import')} blob_sha={r.get('ms_blob_sha256')}")
        if r.get('prefix_token_count')!=0: ok=False
        if not same: ok=False
    # isolation croisee : E1 REST ne doit correspondre qu'a SON controle
    r1,r2=res["E1"][1],res["E2"][1]
    log("=== ISOLATION CROISEE ===")
    log(f"  E1_REST ids={r1.get('decoded_ids_json')}  E2_REST ids={r2.get('decoded_ids_json')}")
    log(f"  E2_CTRL ids={ctrls['E2'].get('decoded_ids_json')}")
    log(f"  E1_REST identique a E2_CTRL ? {'OUI (MELANGE !)' if r1.get('decoded_ids_json')==ctrls['E2'].get('decoded_ids_json') and ctrls['E1'].get('decoded_ids_json')!=ctrls['E2'].get('decoded_ids_json') else 'NON'}")
    log(f"  les deux entrees ont-elles des controles DIFFERENTS ? "
        f"{'OUI' if ctrls['E1'].get('decoded_ids_json')!=ctrls['E2'].get('decoded_ids_json') else 'NON (indiscernables)'}")
    json.dump({"E1":ctrls["E1"],"E2":ctrls["E2"],"E1_REST":r1,"E2_REST":r2,
               "refs":refs},open(f"{D}/concurrent.json","w"),indent=1)
    log(f"TWO_REQUEST_CONCURRENT_RESUME={'PASS' if ok else 'FAIL'}")
    log("=== FIN ===")
    p.terminate(); return 0
if __name__=="__main__":
    rc=1
    try: rc=main()
    finally:
        try:
            pid=int(open(f"{D}/pid.txt").read().strip()); os.killpg(os.getpgid(pid),signal.SIGKILL)
            print(f"[cleanup] {pid} arrete",flush=True)
        except Exception as e: print(f"[cleanup] {e}",flush=True)
    sys.exit(rc)
