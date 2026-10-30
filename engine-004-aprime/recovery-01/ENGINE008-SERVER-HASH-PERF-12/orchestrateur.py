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

    # ---- A. EXPORT (une seule fois)
    codeA, A = post("A", {"fact_token_ids": P58, "extract_only": True, "verify": True})
    if codeA != 200:
        log("ARRET APRES A (defaut). Aucun retry."); identify(p); return 33
    ref = (A.get("text","") or "").replace("kv_state:","")
    log(f"A: ref={ref} blob_bytes={A.get('blob_bytes')} fact_tokens={A.get('fact_tokens')}")

    # ---- 3 PAIRES CONTROL/RESTORED, ORDRE PREENREGISTRE B/C, C/B, B/C
    PAIRS = [("B","C"), ("C","B"), ("B","C")]
    log(f"ORDRE PREENREGISTRE DES PAIRES : {PAIRS}")
    res = {}
    gens = {}
    for i, (first, second) in enumerate(PAIRS):
        log(f"--- PAIRE {i+1} : {first} puis {second} ---")
        for name in (first, second):
            body = ({"fact_token_ids": P58, "prompt": "OK", "n_generate": 32,
                     "greedy": True, "verify": True} if name == "B" else
                    {"state_ref": ref, "prompt": "OK", "n_generate": 32,
                     "greedy": True, "verify": True})
            code, r = post(f"{name}{i+1}", body)
            if code != 200:
                log(f"ARRET : {name}{i+1} a echoue (defaut). Diagnostic conserve.")
                identify(p); return 35
            res[f"{name}{i+1}"] = r
        b, c = res[f"B{i+1}"], res[f"C{i+1}"]
        if b.get("prefix_token_count") != 58 or b.get("state_imported"):
            log(f"  ANOMALIE B{i+1}: prefix={b.get('prefix_token_count')} imported={b.get('state_imported')}")
        if c.get("prefix_token_count") != 0 or not c.get("state_imported"):
            log(f"  ANOMALIE C{i+1}: prefix={c.get('prefix_token_count')} imported={c.get('state_imported')}")
        log(f"  B{i+1}: prefix_tok={b.get('prefix_token_count')} imported={b.get('state_imported')} "
            f"prep_ms={b.get('ms_preparation_total')} ids={b.get('decoded_ids_json')} stop={b.get('stop_reason')}")
        log(f"  C{i+1}: prefix_tok={c.get('prefix_token_count')} imported={c.get('state_imported')} "
            f"prep_ms={c.get('ms_preparation_total')} ids={c.get('decoded_ids_json')} stop={c.get('stop_reason')}")
        log(f"  C{i+1} ms: lookup={c.get('ms_state_lookup')} compat={c.get('ms_compatibility')} "
            f"blob_sha={c.get('ms_blob_sha256')} import={c.get('ms_state_import')}")
        gens[i] = (b, c)
    with open(f"{D}/comparison.json","w") as f: json.dump(res, f, indent=1)

    # ---- comparaison
    import base64, math, struct
    def logits(r):
        bb = base64.b64decode(r.get("first_logits_base64",""))
        return list(struct.unpack(f"<{len(bb)//4}f", bb)) if bb else []
    log("=== COMPARAISON DES PREMIERS LOGITS SUR LES 3 PAIRES ===")
    allident = True
    for i in range(3):
        b, c = gens[i]
        lB, lC = logits(b), logits(c)
        if not lB or not lC or len(lB) != len(lC):
            log(f"  paire {i+1}: logits indisponibles ({len(lB)}/{len(lC)})"); allident = False; continue
        maxd = 0.0; l2 = 0.0; ident = True; aB = aC = 0
        for k,(x,y) in enumerate(zip(lB,lC)):
            if math.isfinite(x) and math.isfinite(y):
                dd = abs(x-y)
                if dd > maxd: maxd = dd
                l2 += dd*dd
                if struct.pack("<f",x) != struct.pack("<f",y): ident = False
            if x > lB[aB]: aB = k
            if y > lC[aC]: aC = k
        if not ident: allident = False
        log(f"  paire {i+1} ({PAIRS[i][0]}/{PAIRS[i][1]}): MAX_ABS_DIFF={maxd:.9g} L2={math.sqrt(l2):.9g} "
            f"ARGMAX={aB}/{aC} BINARY_IDENTICAL={'YES' if ident else 'NO'}")
        log(f"    ids B={b.get('decoded_ids_json')}  C={c.get('decoded_ids_json')}  "
            f"match={'YES' if b.get('decoded_ids_json')==c.get('decoded_ids_json') else 'NO'}")
        log(f"    text B={b.get('text','')!r}")
        log(f"    text C={c.get('text','')!r}")
    log(f"START_LOGITS_BINARY_IDENTICAL_3_PAIRES={'YES' if allident else 'NO'}")
    log("=== FIN ===")
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
