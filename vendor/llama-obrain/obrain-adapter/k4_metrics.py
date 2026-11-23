"""k4_metrics — métriques chiffrées (TFPT, tok/s, cohérence) des modes de blob K4.

Mesure sur le pont jspace (extract → inject par ref) pour chaque mode
(OBRAIN_STATE_TYPE absent=f32 | f16 | i8) :
- TFPT réel (time-to-first-token) = wall d'un inject n_generate=1
  (state_load + préfill du prompt + 1er token), sans stream ;
- tok/s stable = (n-1)/(wall_n − wall_1) sur n_generate=n ; + champ serveur (CPU : fiable) ;
- cohérence : question ciblée (réponse non déductible du nom, code injecté) —
  rappel de marqueurs + similarité de la réponse vs le mode f32 de référence.

Usage :
  python3 obrain-adapter/k4_metrics.py [llama-server-bin] [model.gguf]
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import time
import urllib.request
from difflib import SequenceMatcher

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from adapter import ObrainBridgeRO, metrics_from_inject, post_json

BIN = sys.argv[1] if len(sys.argv) > 1 else \
    "/home/triviere/projects/llama-obrain/build/bin/llama-server"
MODEL = sys.argv[2] if len(sys.argv) > 2 else \
    "/home/triviere/models/Qwen3.8-27B-Q4_K_M.gguf"
OBRAIN = "/home/triviere/projects/banc-obrain/target/release/obrain"
STORE = "/tmp/kv-linux-driversnet.obrain"
PORT = 8099
BASE = f"http://127.0.0.1:{PORT}/jspace/inject_knowledge"

FACTS = {
    "sja1000_open": ("tmp.linux_git.drivers.net.can.sja1000.sja1000_open",
        ["reset", "open_candev", "request_threaded_irq", "threaded", "custom",
         "eagain", "irq_flags", "sja1000_start", "netif_start_queue"],
        "Given the real source of sja1000_open: how is the chip put into reset mode, "
        "when is request_threaded_irq skipped, what is returned if it fails, and which "
        "function starts the chip at the end?"),
    "flexcan_close": ("tmp.linux_git.drivers.net.can.flexcan.flexcan_close",
        ["netif_stop_queue", "interrupts_disable", "free_irq", "offload",
         "transceiver", "close_candev", "quirks"],
        "Given the real source of flexcan_close: list in order the cleanup steps "
        "(queue, interrupts, IRQs under FLEXCAN_QUIRK_SECONDARY_MB_IRQ and NR_IRQ_3, "
        "rx offload, transceiver)."),
}


def q1(qn: str) -> str:
    b = ObrainBridgeRO(OBRAIN, STORE)
    r = b.query(f"MATCH (n:Function) WHERE n.qualified_name CONTAINS '{qn}' RETURN n.text LIMIT 1")
    b.close()
    return str(r[0][0]) if r and r[0] and r[0][0] else ""


def wait_health(timeout_s: float = 200.0) -> bool:
    t0 = time.time()
    while time.time() - t0 < timeout_s:
        try:
            with urllib.request.urlopen(BASE.replace("/jspace/inject_knowledge", "/health"),
                                        timeout=2) as r:
                if json.loads(r.read().decode()).get("status") == "ok":
                    return True
        except Exception:
            pass
        time.sleep(2)
    return False


def boot(env: dict | None, tag: str):
    import subprocess as _sp
    try:
        out = _sp.run(["lsof", "-ti", f"tcp:{PORT}"], capture_output=True, text=True)
        for pid in out.stdout.split():
            _sp.run(["kill", "-9", pid.strip()], capture_output=True)
    except Exception:
        pass
    import time as _t
    _t.sleep(1)
    d = f"/tmp/k4m/{tag}"
    os.makedirs(d, exist_ok=True)
    e = dict(os.environ)
    e.update(env or {})
    log = open(f"{d}/s.log", "w")
    p = subprocess.Popen([BIN, "-m", MODEL, "-c", "4096", "--parallel", "3",
                          "--port", str(PORT), "-t", "8", "--no-warmup"],
                         cwd=d, env=e, stdout=log, stderr=subprocess.STDOUT)
    if not wait_health():
        print(tag, "BOOT FAIL")
        print(open(f"{d}/s.log").read()[-300:])
        p.kill()
        return None
    return p


def run() -> int:
    res = {"date": time.strftime("%Y-%m-%d %H:%M:%S"), "modes": {}}
    facts = {k: q1(qn) for k, (qn, _, _) in FACTS.items()}
    refs = {}
    for mode in ("f32", "f16", "i8"):
        env = None if mode == "f32" else {"OBRAIN_STATE_TYPE": mode}
        p = boot(env, mode)
        if p is None:
            continue
        md = {"facts": {}}
        for name, txt in facts.items():
            ex = post_json(BASE, {"fact_text": txt, "extract_only": True}, timeout=900)
            ref = str(ex.get("text", "")).replace("kv_state:", "")
            refs[name] = ref
            f = {"blob_MiB": round(ex.get("blob_bytes", 0) / 2 ** 20, 1),
                 "tokens": ex.get("fact_tokens")}
            # TFPT réel (n=1) puis tok/s stable (n=48)
            r1 = post_json(BASE, {"state_ref": ref, "prompt": "Explain it briefly.",
                                  "n_generate": 1, "greedy": True}, timeout=400)
            w1 = r1.get("wall_ms")  # absent; on mesure côté client via timing externe -> cf. ci-dessous
            t0 = time.time()
            post_json(BASE, {"state_ref": ref, "prompt": "Explain it briefly.",
                             "n_generate": 1, "greedy": True}, timeout=400)
            tfpt_wall = time.time() - t0
            m1 = metrics_from_inject(r1)
            n = 48
            t0 = time.time()
            rn = post_json(BASE, {"state_ref": ref, "prompt": "Explain it briefly.",
                                  "n_generate": n, "greedy": True}, timeout=600)
            wall_n = time.time() - t0
            mn = metrics_from_inject(rn)
            tok_s = (n - 1) / max(wall_n - tfpt_wall, 0.05)
            f.update({
                "state_load_ms": m1.get("fact_or_state_load_ms"),
                "tfpt_s_wall": round(tfpt_wall, 2),
                "tok_s_steady": round(tok_s, 2),
                "tok_s_server": mn.get("tokens_per_s"),
            })
            # cohérence sur n=96 pour f32 & i8 (référence f32)
            print(f"[{mode}] {name}: blob={f['blob_MiB']} MiB tok={f['tokens']} "
                  f"TFPT={f['tfpt_s_wall']}s tok/s={f['tok_s_steady']} (steady) "
                  f"server={f['tok_s_server']} state_load={f['state_load_ms']}ms")
            md["facts"][name] = f
        # cohérence ciblée f32/i8
        if mode in ("f32", "i8"):
            for name, (qn, markers, question) in FACTS.items():
                t0 = time.time()
                r = post_json(BASE, {"state_ref": refs[name], "prompt": question,
                                     "n_generate": 96, "greedy": True}, timeout=600)
                wall = time.time() - t0
                ans = str(r.get("text") or "")
                low = ans.lower()
                hits = [mk for mk in markers if mk in low]
                md.setdefault("coherence", {})[name] = {
                    "hits": hits, "recall": round(len(hits) / len(markers), 2),
                    "wall_s": round(wall, 1), "ans_head": ans[:160]}
                print(f"[{mode}] cohérence {name}: rappel {len(hits)}/{len(markers)} "
                      f"hits={hits}")
        p.kill()
        p.wait()
        res["modes"][mode] = md
    # similarité f32 vs i8
    if "f32" in res["modes"] and "i8" in res["modes"]:
        sim = {}
        for name in FACTS:
            a = res["modes"]["f32"]["coherence"][name]["ans_head"]
            b = res["modes"]["i8"]["coherence"][name]["ans_head"]
            sim[name] = round(SequenceMatcher(None, a, b).ratio(), 3)
        res["f32_vs_i8_similarity"] = sim
        print("similarité f32 vs i8 (début de réponse) :", sim)
    with open("/tmp/e2e-linux-kv/k4-metrics.json", "w") as fh:
        json.dump(res, fh, ensure_ascii=False, indent=2)
    print("rapport : /tmp/e2e-linux-kv/k4-metrics.json")
    return 0


if __name__ == "__main__":
    sys.exit(run())
