#!/usr/bin/env python3
# Test du chemin DISQUE : extraire, ARRETER, REDEMARRER, reinjecter par ref.
import base64, json, math, os, signal, struct, subprocess, sys, time
import urllib.request, urllib.error
F="/home/triviere/projects/llama-obrain-cuda-f1a"
MC="/home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf"
D="/tmp/disktest"; WORK="/home/triviere/obrain-9910/work"; PORT=9910; BASE=f"http://127.0.0.1:{PORT}"
def log(m):
    l=f"[{time.strftime('%H:%M:%S')}] {m}"; print(l,flush=True); open(f"{D}/test.log","a").write(l+"\n")
def probe():
    try:
        with urllib.request.urlopen(BASE+"/health",timeout=5) as r: return r.status,r.read().decode()
    except urllib.error.HTTPError as e: return e.code,e.read().decode()
    except Exception as e: return 0,str(e)
def post(name,body):
    t0=time.time()
    req=urllib.request.Request(BASE+"/jspace/inject_knowledge",data=json.dumps(body).encode(),
                               headers={"Content-Type":"application/json"})
    code,raw,err=0,None,None
    try:
        with urllib.request.urlopen(req,timeout=300) as r: code=r.status; raw=r.read().decode("utf-8","replace")
    except urllib.error.HTTPError as e: code=e.code; raw=e.read().decode("utf-8","replace")
    except Exception as e: err=f"{type(e).__name__}: {e}"
    dt=time.time()-t0; log(f"[{name}] http={code} duree={dt:.3f}s err={err}")
    if raw is not None: open(f"{D}/{name}.json","w").write(raw)
    if code!=200 and raw: log(f"   {raw[:250]}")
    try: return code,(json.loads(raw) if raw else None)
    except Exception: return code,None
def start():
    cmd=[f"{F}/build-cuda/bin/llama-server","-m",MC,"--host","127.0.0.1","--port",str(PORT),
         "-ngl","99","--ctx-size","262144","--parallel","3","--jinja"]
    p=subprocess.Popen(cmd,stdout=open("/home/triviere/obrain-9910/logs/server.log","w"),
        stderr=subprocess.STDOUT,cwd=WORK,
        env={**os.environ,"OBRAIN_KV_STATES_MAX_BYTES":str(2*1024**3),"CUDA_VISIBLE_DEVICES":"0"},
        preexec_fn=os.setsid)
    open("/home/triviere/obrain-9910/server.pid","w").write(str(p.pid))
    t0=time.time()
    while time.time()-t0<900:
        if p.poll() is not None: return p,False
        c,b=probe()
        if c==200 and '"ok"' in b: return p,True
        time.sleep(3)
    return p,False
def port_owner():
    """PID qui ecoute REELLEMENT sur le port, ou None."""
    try:
        o=subprocess.run(["ss","-tlnp"],capture_output=True,text=True).stdout
        for l in o.splitlines():
            if f":{PORT} " in l:
                i=l.find("pid=")
                if i>=0: return int(l[i+4:].split(",")[0])
    except Exception: pass
    return None
def stop(p):
    """Arret VERIFIE : le port doit etre libre ET le PID disparu."""
    try: os.killpg(os.getpgid(p.pid), signal.SIGTERM)
    except Exception: pass
    for _ in range(40):
        if p.poll() is not None and port_owner() is None: break
        time.sleep(1)
    if p.poll() is None or port_owner() is not None:
        try: os.killpg(os.getpgid(p.pid), signal.SIGKILL)
        except Exception: pass
        time.sleep(4)
    owner=port_owner()
    log(f"  [stop] exit={p.poll()} proprietaire_du_port={owner}")
    if owner is not None:
        log(f"  [stop] ECHEC : le port {PORT} est encore tenu par PID {owner}")
        return False
    return True
def lg(r):
    b=base64.b64decode(r.get("first_logits_base64",""))
    return list(struct.unpack(f"<{len(b)//4}f",b)) if b else []
def main():
    os.makedirs(D,exist_ok=True); open(f"{D}/test.log","w").close()
    log("=== TEST DU CHEMIN DISQUE (reinjection SANS texte, SANS recalcul) ===")
    ids=[int(x) for x in open("/tmp/kv02/ids59.txt").read().split()]; P58=ids[:58]
    kvd=WORK+"/kv-states"
    log(f"repertoire d'etats : {kvd}")
    before=set(os.listdir(kvd)) if os.path.isdir(kvd) else set()
    log(f"fichiers presents avant : {len(before)} -> {sorted(before)}")

    log("--- INSTANCE 1 ---")
    p1,ok=start()
    if not ok: log("instance 1 NON PRETE"); return 41
    log(f"instance 1 PID={p1.pid} prete")
    c,Ctrl=post("CTRL",{"fact_token_ids":P58,"prompt":"OK","n_generate":32,"greedy":True,"verify":True})
    if c!=200: stop(p1); return 42
    log(f"  CTRL ids={Ctrl.get('decoded_ids_json')} prefix_tok={Ctrl.get('prefix_token_count')} text={Ctrl.get('text','')!r}")
    c,A=post("EXPORT",{"fact_token_ids":P58,"extract_only":True,"verify":True})
    if c!=200: stop(p1); return 43
    ref=(A.get("text","") or "").replace("kv_state:","")
    log(f"  EXPORT ref={ref} blob_bytes={A.get('blob_bytes')}")
    # ecriture sur disque ? (asynchrone cote handler, laissons un instant)
    time.sleep(2)
    after=set(os.listdir(kvd)) if os.path.isdir(kvd) else set()
    newf=sorted(after-before)
    log(f"  nouveaux fichiers sur disque : {newf}")
    fpath=kvd+"/"+ref+".bin"
    if os.path.exists(fpath):
        sz=os.path.getsize(fpath)
        log(f"  fichier {fpath} : {sz} octets")
        with open(fpath,"rb") as f: head=f.read(8)
        log(f"  8 premiers octets : {head!r}  (attendu: b'OBKV' + version)")
    else:
        log(f"  ATTENTION : {fpath} ABSENT -- le chemin disque ne sera pas testable"); stop(p1); return 44

    log("--- ARRET DE L'INSTANCE 1 (la memoire cache disparait) ---")
    if not stop(p1):
        log("ABANDON : impossible d'arreter l'instance 1, le test disque serait FAUX"); return 47
    log(f"instance 1 arretee ; port {PORT} LIBRE (verifie)")

    log("--- INSTANCE 2 (processus NEUF, cache memoire VIDE) ---")
    p2,ok=start()
    if not ok: log("instance 2 NON PRETE"); return 45
    owner=port_owner()
    if owner != p2.pid:
        log(f"ABANDON : le port est tenu par PID {owner}, pas par l'instance 2 ({p2.pid}) -- test FAUX")
        return 48
    log(f"instance 2 PID={p2.pid} prete ET proprietaire du port {PORT} -- seule source : LE DISQUE")
    c,R=post("REST_DISK",{"state_ref":ref,"prompt":"OK","n_generate":32,"greedy":True,"verify":True})
    if c!=200:
        log("REINJECTION DISQUE ECHOUEE"); stop(p2); return 46
    log(f"  REST ids={R.get('decoded_ids_json')} prefix_tok={R.get('prefix_token_count')} "
        f"imported={R.get('state_imported')} text={R.get('text','')!r}")
    log(f"  REST ms: lookup={R.get('ms_state_lookup')} compat={R.get('ms_compatibility')} "
        f"blob_sha={R.get('ms_blob_sha256')} import={R.get('ms_state_import')} total={R.get('ms_preparation_total')}")
    l1,l2=lg(Ctrl),lg(R); md=0.0;l2d=0.0;idn=True;a1=a2=0
    for k,(x,y) in enumerate(zip(l1,l2)):
        if math.isfinite(x) and math.isfinite(y):
            d=abs(x-y); md=max(md,d); l2d+=d*d
            if struct.pack("<f",x)!=struct.pack("<f",y): idn=False
        if x>l1[a1]: a1=k
        if y>l2[a2]: a2=k
    log("=== COMPARAISON : reinjection DISQUE vs CONTROLE de l'instance 1 ===")
    log(f"  MAX_ABS_DIFF={md:.9g} L2={math.sqrt(l2d):.9g} ARGMAX={a1}/{a2} BINARY_IDENTICAL={'YES' if idn else 'NO'}")
    log(f"  prefix_token_count={R.get('prefix_token_count')}  (0 = aucun token de prefixe recalcule)")
    log(f"  TOKEN_IDS_MATCH={'YES' if Ctrl.get('decoded_ids_json')==R.get('decoded_ids_json') else 'NO'}"
        f"  TEXT_MATCH={'YES' if Ctrl.get('text')==R.get('text') else 'NO'}")
    ok_all = idn and R.get('prefix_token_count')==0 and Ctrl.get('decoded_ids_json')==R.get('decoded_ids_json')
    log(f"DISK_REINJECTION_WITHOUT_TEXT={'PASS' if ok_all else 'FAIL'}")
    log("=== instance 2 laissee EN SERVICE sur 127.0.0.1:9910 (bind local pendant le test) ===")
    return 0
if __name__=="__main__": sys.exit(main())
