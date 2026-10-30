#!/usr/bin/env python3
# orchestrateur ENGINE008 -- possede le serveur, son PID, ses logs, le port,
# les requetes et le nettoyage. Sondes horodatees, verification de survie.
import json, os, signal, subprocess, sys, time, urllib.request, urllib.error

F    = "/home/triviere/projects/llama-obrain-cuda-f1a"
MC   = "/home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf"
D    = "/tmp/exp11"
PORT = 8480
BASE = f"http://127.0.0.1:{PORT}"
# DELAIS PREENREGISTRES. Le hachage du modele coute 64.8 s (mesure RESOLVE-09),
# le chargement ~20 s : la fenetre de demarrage est fixee a 600 s, LARGEMENT
# au-dela, et n'est pas ajustee apres observation.
STARTUP_MAX_S = 600
REQUEST_MAX_S = 240
READY = None

def log(m):
    line = f"[{time.strftime('%H:%M:%S')}] {m}"
    print(line, flush=True)
    with open(f"{D}/orchestrator.log", "a") as f: f.write(line + "\n")

def probe(tag):
    """Sonde de disponibilite SANS CALCUL. Rend (http_code, body_ou_erreur)."""
    global READY
    READY = (READY or 0) + 1
    try:
        with urllib.request.urlopen(BASE + "/health", timeout=4) as r:
            b = r.read().decode("utf-8", "replace"); code = r.status
    except urllib.error.HTTPError as e:
        code = e.code; b = e.read().decode("utf-8", "replace")
    except Exception as e:
        code = 0; b = f"{type(e).__name__}: {e}"
    with open(f"{D}/readiness_probes.log", "a") as f:
        f.write(f"probe#{READY} t={time.time():.3f} {tag} http={code} body={b[:200]}\n")
    return code, b

def alive(p):
    return p.poll() is None

def identify(p):
    """Recense PID vivant, etat, socket d'ecoute, port."""
    pid = p.pid
    st = "VIVANT" if alive(p) else f"TERMINE rc={p.returncode}"
    listen = ""
    try:
        out = subprocess.run(["ss","-tlnp"], capture_output=True, text=True, timeout=10).stdout
        for l in out.splitlines():
            if f":{PORT}" in l: listen = l.strip()
    except Exception as e: listen = f"(ss indisponible: {e})"
    log(f"  PID={pid} {st} | ecoute: {listen or 'AUCUNE'}")
    return pid, st, listen

def post(name, body):
    t0 = time.time()
    data = json.dumps(body).encode()
    req = urllib.request.Request(BASE + "/jspace/inject_knowledge", data=data,
                                 headers={"Content-Type": "application/json"})
    code, raw, err = 0, None, None
    try:
        with urllib.request.urlopen(req, timeout=REQUEST_MAX_S) as r:
            code = r.status; raw = r.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        code = e.code; raw = e.read().decode("utf-8", "replace")
    except Exception as e:
        err = f"{type(e).__name__}: {e}"
    dt = time.time() - t0
    log(f"[{name}] http={code} duree={dt:.3f}s err={err}")
    with open(f"{D}/{name}.meta","w") as f:
        f.write(f"http={code} duration_s={dt:.6f} error={err}\n")
    if raw is not None:
        with open(f"{D}/{name}.json","w") as f: f.write(raw)
    with open(f"{D}/phases.log","a") as f:
        f.write(f"{name} t0={t0:.3f} t1={time.time():.3f} http={code} d={dt:.3f} err={err}\n")
    try: return code, (json.loads(raw) if raw else None)
    except Exception: return code, None

def main():
    os.makedirs(D, exist_ok=True)
    for f in ("orchestrator.log","readiness_probes.log","phases.log"):
        open(f"{D}/{f}","w").close()
    log("=== ORCHESTRATEUR ENGINE008 ENDTOEND-10 ===")
    log(f"hote=127.0.0.1 port={PORT} (meme hote, meme espace reseau)")
    log(f"DELAIS PREENREGISTRES: demarrage<={STARTUP_MAX_S}s requete<={REQUEST_MAX_S}s")

    slog = open(f"{D}/server.log","w")
    p = subprocess.Popen(
        [f"{F}/build-cuda/bin/llama-server","--model",MC,"--host","127.0.0.1",
         "--port",str(PORT),"--ctx-size","1024","--n-gpu-layers","0","--parallel","3","--jinja"],
        stdout=slog, stderr=subprocess.STDOUT, cwd=F,
        env={**os.environ, "OBRAIN_KV_STATES_MAX_BYTES": str(2*1024**3)},
        preexec_fn=os.setsid)          # groupe de processus propre : rien d'externe ne le tuera
    log(f"SERVER_PID={p.pid} (groupe propre, cwd={F})")
    with open(f"{D}/pid.txt","w") as f: f.write(str(p.pid))

    # ---- attente de disponibilite REELLE, avec verification de survie
    t_start = time.time(); ready = False
    while time.time() - t_start < STARTUP_MAX_S:
        if not alive(p):
            log(f"SERVEUR TERMINE PENDANT LE DEMARRAGE rc={p.returncode}")
            log("--- 30 dernieres lignes du journal serveur ---")
            for l in open(f"{D}/server.log").read().splitlines()[-30:]: log("  " + l)
            return 30
        code, body = probe("startup")
        if code == 200 and '"ok"' in body:
            ready = True
            log(f"PRET apres {time.time()-t_start:.1f}s (probe#{READY}) http=200 body={body}")
            break
        time.sleep(3)
    if not ready:
        log(f"NON PRET apres {STARTUP_MAX_S}s -- ABANDON (aucune requete modele envoyee)")
        return 31

    log("--- ETAT AVANT A (horodate) ---")
    pid, st, listen = identify(p)
    code, body = probe("avant_A")
    log(f"  sonde HTTP: http={code} body={body}")
    log(f"  URL CLIENT EXACTE: POST {BASE}/jspace/inject_knowledge")
    if code != 200:
        log("ABANDON: la sonde de disponibilite ne repond pas 200"); return 32

    ids = [int(x) for x in open("/tmp/kv02/ids59.txt").read().split()]
    assert len(ids) == 59, len(ids)
    P58, ID59 = ids[:58], ids[58]
    log(f"IDS AUTORITAIRES: 59 lus, prefixe={len(P58)}, 59e ID={ID59}")

    # ---- A. EXPORT
    codeA, A = post("A", {"fact_token_ids": P58, "extract_only": True, "verify": True})
    if codeA != 200:
        log("ARRET APRES A (defaut). Diagnostic conserve, aucun retry."); identify(p); return 33
    ref = (A.get("text","") or "").replace("kv_state:","")
    log(f"A: ref={ref} blob_bytes={A.get('blob_bytes')} fact_tokens={A.get('fact_tokens')}")

    # ---- B. CONTROL
    codeB, B = post("B", {"fact_token_ids": P58, "prompt": "OK", "n_generate": 32,
                          "greedy": True, "verify": True})
    if codeB != 200:
        log("ARRET APRES B (defaut). Aucune requete suivante."); identify(p); return 34

    # ---- C. RESTORED
    codeC, C = post("C", {"state_ref": ref, "prompt": "OK", "n_generate": 32,
                          "greedy": True, "verify": True})
    if codeC != 200:
        log("ARRET APRES C (defaut).")
        log("--- DIAGNOSTIC : dernieres phases + etat des threads ---")
        identify(p)
        for t in os.listdir(f"/proc/{p.pid}/task"):
            try: stt = open(f"/proc/{p.pid}/task/{t}/stat").read().split()[2]
            except Exception: stt = "?"
            log(f"  thread {t} etat={stt}")
        try:
            for l in open(f"/proc/{p.pid}/wchan").read().splitlines(): log(f"  wchan={l}")
        except Exception: pass
        return 35

    # ---- comparaison
    import base64, math, struct
    def logits(r):
        b = base64.b64decode(r.get("first_logits_base64",""))
        return list(struct.unpack(f"<{len(b)//4}f", b)) if b else []
    lB, lC = logits(B), logits(C)
    res = {"A": A, "B": B, "C": C}
    with open(f"{D}/comparison.json","w") as f: json.dump(res, f, indent=1)
    log(f"LOGITS: |B|={len(lB)} |C|={len(lC)}")
    if lB and lC and len(lB) == len(lC):
        maxd = 0.0; l2 = 0.0; ident = True; argB = argC = 0
        for i,(x,y) in enumerate(zip(lB,lC)):
            if math.isfinite(x) and math.isfinite(y):
                d = abs(x-y)
                if d > maxd: maxd = d
                l2 += d*d
                if struct.pack("<f",x) != struct.pack("<f",y): ident = False
            if x > lB[argB]: argB = i
            if y > lC[argC]: argC = i
        log(f"START_LOGITS FINITE=YES MAX_ABS_DIFF={maxd:.9g} L2={math.sqrt(l2):.9g} "
            f"ARGMAX={argB}/{argC} BINARY_IDENTICAL={'YES' if ident else 'NO'}")
    log(f"B: ids={B.get('decoded_ids_json')} pos={B.get('decoded_pos_json')} "
        f"sel={B.get('selected_ids_count')} emis={B.get('emitted_ids_count')} "
        f"stop={B.get('stop_reason')} prefix_tok={B.get('prefix_token_count')}")
    log(f"C: ids={C.get('decoded_ids_json')} pos={C.get('decoded_pos_json')} "
        f"sel={C.get('selected_ids_count')} emis={C.get('emitted_ids_count')} "
        f"stop={C.get('stop_reason')} prefix_tok={C.get('prefix_token_count')} "
        f"imported={C.get('state_imported')}")
    log(f"BLOB_SHA256_MS: A={A.get('ms_blob_sha256')} C={C.get('ms_blob_sha256')}")
    log(f"MODEL_SHA256_MS: A={A.get('ms_model_sha256')} B={B.get('ms_model_sha256')}")
    log(f"ms_preparation_total: B={B.get('ms_preparation_total')} C={C.get('ms_preparation_total')}")
    log(f"TEXT B=[{B.get('text','')}]")
    log(f"TEXT C=[{C.get('text','')}]")
    log(f"TOKEN_IDS_MATCH={'YES' if B.get('decoded_ids_json')==C.get('decoded_ids_json') else 'NO'}")
    log(f"TEXT_MATCH={'YES' if B.get('text')==C.get('text') else 'NO'}")
    log("=== FIN ===")
    return 0

if __name__ == "__main__":
    rc = 1
    p = None
    try:
        rc = main()
    finally:
        # nettoyage : le serveur appartient a l'orchestrateur
        try:
            pid = int(open(f"{D}/pid.txt").read().strip())
            os.killpg(os.getpgid(pid), signal.SIGKILL)
            print(f"[cleanup] serveur {pid} arrete", flush=True)
        except Exception as e:
            print(f"[cleanup] {e}", flush=True)
    sys.exit(rc)
