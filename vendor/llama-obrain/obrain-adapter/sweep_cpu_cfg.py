"""sweep_cpu_cfg — sweep CPU de configurations serveur fork (machine n3).

Pour chaque config (-t, -c) : boot serveur → extract d'un fait réel (store Linux)
→ 2 injects (n=32) → métriques (tok/s, TFPT, state-load, wall) → arrêt serveur.

Usage : python3 obrain-adapter/sweep_cpu_cfg.py [OBRAIN_BIN] [STORE]
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import time
import urllib.request
from typing import Any, Dict, List

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from adapter import ObrainBridgeRO, metrics_from_inject, post_json

OBRAIN_BIN = sys.argv[1] if len(sys.argv) > 1 else \
    "/home/triviere/projects/banc-obrain/target/release/obrain"
STORE = sys.argv[2] if len(sys.argv) > 2 else "/tmp/kv-linux-driversnet.obrain"
SERVER = "/home/triviere/projects/llama-obrain/build/bin/llama-server"
MODEL = "/home/triviere/models/Qwen3.8-27B-Q4_K_M.gguf"
PORT = 8099
BASE = f"http://127.0.0.1:{PORT}/jspace/inject_knowledge"

CONFIGS: List[Dict[str, Any]] = [
    {"tag": "t8-c2048", "args": ["-t", "8", "-c", "2048"]},    # config validée d'origine
    {"tag": "t8-c8192", "args": ["-t", "8", "-c", "8192"]},    # slot ~2816, t=8
    {"tag": "t16-c8192", "args": ["-t", "16", "-c", "8192"]},  # slot ~2816, t=16
    {"tag": "t12-c8192", "args": ["-t", "12", "-c", "8192"]},
    {"tag": "t8-c6144", "args": ["-t", "8", "-c", "6144"]},    # slot ~2048
]


def wait_health(timeout_s: float = 200.0) -> bool:
    t0 = time.time()
    while time.time() - t0 < timeout_s:
        try:
            with urllib.request.urlopen(BASE.replace("/jspace/inject_knowledge",
                                                     "/health"), timeout=3) as r:
                if json.loads(r.read().decode()).get("status") == "ok":
                    return True
        except Exception:
            pass
        time.sleep(3)
    return False


def load_fact() -> str:
    b = ObrainBridgeRO(OBRAIN_BIN, STORE)
    rows = b.query("MATCH (n:Function {qualified_name: 'tmp.linux_git.drivers.net."
                   "ethernet.intel.e1000.e1000_probe'}) RETURN n.text LIMIT 1")
    b.close()
    return (rows[0][0] if rows and rows[0] else "")[:1200]


def run_config(tag: str, extra: List[str], fact: str) -> Dict[str, Any]:
    os.makedirs("/tmp/sweep-kv", exist_ok=True)
    log = open(f"/tmp/sweep-kv/{tag}.log", "w")
    args = [SERVER, "-m", MODEL, "--parallel", "3", "--port", str(PORT),
            "-b", "2048", "-ub", "512", "--no-warmup"] + extra
    p = subprocess.Popen(args, cwd="/tmp/sweep-kv", stdout=log, stderr=subprocess.STDOUT)
    out: Dict[str, Any] = {"tag": tag, "args": extra}
    try:
        if not wait_health():
            out["error"] = "health timeout"
            return out
        # slot info
        try:
            with urllib.request.urlopen(
                    BASE.replace("/jspace/inject_knowledge", "/slots"),
                    timeout=5) as r:
                slots = json.loads(r.read().decode())
                out["n_ctx_slot"] = slots[0]["n_ctx"] if slots else None
        except Exception:
            pass
        t0 = time.time()
        ex = post_json(BASE, {"fact_text": fact, "extract_only": True},
                       timeout=300)
        out["extract_s"] = round(time.time() - t0, 2)
        out["fact_tokens"] = ex.get("fact_tokens")
        ref = str(ex.get("text", "")).replace("kv_state:", "")
        q = ("Based on the kernel source provided as knowledge, describe the "
             "function e1000_probe: role, main steps, return value.")
        runs = []
        for i in range(2):
            t0 = time.time()
            r = post_json(BASE, {"state_ref": ref, "prompt": q,
                                 "n_generate": 32, "greedy": True}, timeout=400)
            m = metrics_from_inject(r)
            m["wall_s"] = round(time.time() - t0, 2)
            runs.append(m)
            print(f"  [{tag}] run{i + 1}: tok/s={m['tokens_per_s']} "
                  f"tfpt={m['tfpt_ms']} state_load={m['fact_or_state_load_ms']} "
                  f"n={m['n_tokens']} wall={m['wall_s']}s")
        good = [m for m in runs if m["tokens_per_s"]]
        out["tok_per_s_mean"] = (sum(m["tokens_per_s"] for m in good) / len(good)
                                 if good else None)
        out["tfpt_ms_mean"] = (sum(m["tfpt_ms"] for m in good) / len(good)
                               if good else None)
        out["state_load_ms_mean"] = (sum(m["fact_or_state_load_ms"] for m in good)
                                     / len(good) if good else None)
        out["runs"] = runs
    except Exception as e:
        out["error"] = repr(e)[:200]
    finally:
        try:
            p.kill()
        except Exception:
            pass
        p.wait(timeout=15)
        log.close()
    return out


def main() -> int:
    fact = load_fact()
    print(f"fait : e1000_probe, {len(fact)} chars")
    results = []
    for cfg in CONFIGS:
        print(f"\n=== config {cfg['tag']} ({cfg['args']}) ===")
        r = run_config(cfg["tag"], cfg["args"], fact)
        results.append(r)
        print(json.dumps({k: v for k, v in r.items() if k != "runs"},
                         ensure_ascii=False))
    with open("/tmp/e2e-linux-kv/sweep-cpu.json", "w") as fh:
        json.dump(results, fh, ensure_ascii=False, indent=2)
    print("\n=== SYNTHÈSE ===")
    for r in results:
        print(f"{r['tag']:14s} slot={r.get('n_ctx_slot')} "
              f"tok/s={r.get('tok_per_s_mean')} tfpt={r.get('tfpt_ms_mean')} "
              f"state_load={r.get('state_load_ms_mean')} extract={r.get('extract_s')} "
              f"{'ERR ' + r.get('error', '') if 'error' in r else ''}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
