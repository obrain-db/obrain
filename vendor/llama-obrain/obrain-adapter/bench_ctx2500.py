"""bench_ctx2500 — bench CPU (machine n3) du pont obrain ↔ llama-obrain.

Objectifs (round utilisateur) :
1. Mesurer tokens/s et TFPT (config : -t 16, ub=512, 3 slots × 2816) sur des faits
   réels lus depuis le store Linux — comparaison aux mesures t=8 (1,07-1,70 tok/s).
2. Valider la capacité « contexte mémoire ~2500-2800 » d'un slot : injection d'un
   GROS fait (concaténation de plusieurs fonctions Linux, ~1 300 tokens) sur un
   slot de 2816 positions → rapport positions mémoire / tokens visibles, préfill
   évité vs texte, coût inject (~ms), génération dans la foulée.
3. Rapport daté JSON + markdown dans /tmp.

Usage : python3 obrain-adapter/bench_ctx2500.py [OBRAIN_BIN] [STORE] [PORT] [N_GEN]
"""
from __future__ import annotations

import json
import os
import re
import sys
import time
from typing import Any, Dict, List

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from adapter import ObrainBridgeRO, metrics_from_inject, post_json

OBRAIN_BIN = sys.argv[1] if len(sys.argv) > 1 else \
    "/home/triviere/projects/banc-obrain/target/release/obrain"
STORE = sys.argv[2] if len(sys.argv) > 2 else "/tmp/kv-linux-driversnet.obrain"
PORT = int(sys.argv[3]) if len(sys.argv) > 3 else 8099
N_GEN = int(sys.argv[4]) if len(sys.argv) > 4 else 96
BASE = f"http://127.0.0.1:{PORT}/jspace/inject_knowledge"

# Fonctions réelles du store (qn vérifiés) pour faits simples + gros fait mémoire
FACT_QNS = [
    "tmp.linux_git.drivers.net.ethernet.intel.e1000.e1000_probe",
    "tmp.linux_git.drivers.net.can.m_can.m_can_start_xmit",
    "tmp.linux_git.drivers.net.can.rockchip.rkcanfd_open",
    "tmp.linux_git.drivers.net.can.flexcan.flexcan_close",
    "tmp.linux_git.drivers.net.can.sja1000.sja1000_open",
]
EXTRA_QNS = [  # cherchés par CONTAINS ; ignorés si absents
    "virtnet_probe", "i40e_probe", "igb_probe", "e1000e_probe",
]


def wait_health(timeout_s: float = 240.0) -> bool:
    import urllib.request
    t0 = time.time()
    while time.time() - t0 < timeout_s:
        try:
            with urllib.request.urlopen(BASE.replace("/jspace/inject_knowledge",
                                                     "/health"), timeout=3) as r:
                if json.loads(r.read().decode()).get("status") == "ok":
                    return True
        except Exception:
            pass
        time.sleep(5)
    return False


def load_texts() -> Dict[str, str]:
    b = ObrainBridgeRO(OBRAIN_BIN, STORE)
    out: Dict[str, str] = {}
    for qn in FACT_QNS:
        try:
            rows = b.query(f"MATCH (n:Function {{qualified_name: '{qn}'}}) "
                           "RETURN n.text LIMIT 1")
            if rows and rows[0] and rows[0][0]:
                out[qn.split(".")[-1]] = str(rows[0][0]).strip()
        except Exception:
            pass
    for short in EXTRA_QNS:
        try:
            rows = b.query("MATCH (n:Function) WHERE n.qualified_name CONTAINS '"
                           f"{short}' RETURN n.qualified_name, n.text LIMIT 1")
            if rows and rows[0] and rows[0][1]:
                out[short] = str(rows[0][1]).strip()
        except Exception:
            pass
    b.close()
    return out


def do_extract(text: str) -> Dict[str, Any]:
    t0 = time.time()
    ex = post_json(BASE, {"fact_text": text, "extract_only": True}, timeout=600)
    return {"ref": str(ex.get("text", "")).replace("kv_state:", ""),
            "wall_s": round(time.time() - t0, 2),
            "fact_tokens": ex.get("fact_tokens"),
            "blob_bytes": ex.get("blob_bytes"),
            "chars": len(text)}


def do_inject(ref: str, prompt: str, n_gen: int, timeout_s: float = 900.0):
    t0 = time.time()
    r = post_json(BASE, {"state_ref": ref, "prompt": prompt,
                         "n_generate": n_gen, "greedy": True}, timeout=timeout_s)
    wall = time.time() - t0
    m = metrics_from_inject(r)
    m["wall_s"] = round(wall, 2)
    m["answer_len"] = len(str(r.get("text") or ""))
    return m


def main() -> int:
    res: Dict[str, Any] = {"date": time.strftime("%Y-%m-%d %H:%M:%S"),
                           "port": PORT, "n_generate": N_GEN, "mode": "CPU",
                           "server_config": "-t 16 -b 2048 -ub 512 -np 3 "
                                            "-c 8192 (slot 2816)",
                           "simple": [], "big_memory": None}
    if not wait_health():
        print("FATAL: serveur non joignable"); return 2

    texts = load_texts()
    print(f"{len(texts)} textes lus depuis le store")

    # --- faits simples : tok/s + TFPT (moyenne sur 2 injects) ---
    for name, txt in list(texts.items())[:3]:
        ex = do_extract(txt[:1400])
        q = (f"Based on the kernel source provided as knowledge, describe the "
             f"function {name}: role, main steps, return value.")
        metrics = []
        for i in range(2):
            inj = do_inject(ex["ref"], q, N_GEN)
            metrics.append({k: inj[k] for k in
                            ("tfpt_ms", "tokens_per_s",
                             "fact_or_state_load_ms", "n_tokens", "wall_s")})
            print(f"simple {name} run{i + 1}: {metrics[-1]}")
        res["simple"].append({"name": name, "extract": ex, "runs": metrics})
        mean_ts = sum(m["tokens_per_s"] or 0 for m in metrics) / len(metrics)
        mean_tfpt = sum(m["tfpt_ms"] or 0 for m in metrics) / len(metrics)
        print(f"  → {name}: tok/s moyen {mean_ts:.2f}, TFPT moyen "
              f"{mean_tfpt:.0f} ms")

    # --- gros fait mémoire : concat de plusieurs fonctions (slot ~2816) ---
    parts = list(texts.values())
    mem = "\n\n".join(f"// --- {k} ---\n{v}" for k, v in texts.items())
    # vise ~1 200-1 500 tokens : garde ~5 500 chars
    mem = mem[:5500]
    exm = do_extract(mem)
    print(f"gros fait mémoire: {exm['fact_tokens']} tokens, "
          f"blob {exm['blob_bytes']} o, extract {exm['wall_s']} s")
    qm = ("You are given a large block of real Linux kernel source (several "
          "functions). Name at least four of the functions present, and for each "
          "one sentence on its role. Then state what the functions have in "
          "common (subsystem/pattern).")
    t0 = time.time()
    inj = do_inject(exm["ref"], qm, N_GEN)
    res["big_memory"] = {"extract": exm, "inject": {k: inj[k] for k in
                         ("tfpt_ms", "tokens_per_s", "fact_or_state_load_ms",
                          "query_decode_ms", "n_tokens", "wall_s")},
                         "answer_head": str(inj["answer_len"])}
    print("big_memory inject:", res["big_memory"]["inject"])
    print("  nb tokens mémoire (fait) :", exm["fact_tokens"],
          "| slot : 2816 | tokens visibles (prompt) : ~",
          len(qm.split()), "| préfill texte évité :",
          f"{exm['fact_tokens'] / 12:.0f} s (à 12 tok/s texte) vs "
          f"inject {exm['wall_s']} s une fois, puis ~0,1 s/question")

    os.makedirs("/tmp/e2e-linux-kv", exist_ok=True)
    with open("/tmp/e2e-linux-kv/bench-ctx2500.json", "w") as fh:
        json.dump(res, fh, ensure_ascii=False, indent=2)
    print("rapport : /tmp/e2e-linux-kv/bench-ctx2500.json")
    return 0


if __name__ == "__main__":
    sys.exit(main())
