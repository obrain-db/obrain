#!/usr/bin/env python3
"""qa_obrain.py — question→réponse branché sur obrain (pont stable Vulkan + K4 i8).

Flux : store Linux (obrain, bridge RO) → sélection de fonctions par mots-clés de la
question (relief CONTAINS) → construction du fait → extract (état i8 K4, blob ÷3) →
inject par ref → réponse + métriques wall (TFPT, tok/s, blob, state_load).

Usage : python3 qa_obrain.py "<question>" [port]
Exemple : python3 qa_obrain.py "Que fait la fonction sja1000_open ?"
"""
from __future__ import annotations
import json, os, re, sys, time, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "obrain-adapter"))
from adapter import ObrainBridgeRO, metrics_from_inject, post_json

OBRAIN = "/home/triviere/projects/banc-obrain/target/release/obrain"
STORE = "/tmp/kv-linux-driversnet.obrain"
PORT = int(sys.argv[2]) if len(sys.argv) > 2 else 8099
BASE = f"http://127.0.0.1:{PORT}/jspace/inject_knowledge"
N_GEN = 90

def pick_facts(question: str, bridge: ObrainBridgeRO, k: int = 2):
    """Sélectionne ≤k fonctions du store dont le nom apparaît (ou ressemble) dans la question."""
    toks = re.findall(r"[A-Za-z_][A-Za-z0-9_]{2,}", question)
    cand = []
    for t in toks:
        low = t.lower()
        if low in ("que", "fait", "fonction", "explique", "linux", "kernel", "driver",
                   "comment", "quelle", "quel", "role", "rôle", "dans", "avec", "pour", "the", "what"):
            continue
        if not re.search(r"(_|probe|open|close|init|poll|read|write|xmit|stop|start)", low):
            continue
        try:
            r = bridge.query("MATCH (n:Function) WHERE n.qualified_name CONTAINS '%s' "
                             "RETURN n.qualified_name, n.text LIMIT 1" % t)
            if r and r[0] and r[0][1]:
                cand.append(str(r[0][1])[:1400])
        except Exception:
            pass
        if len(cand) >= k:
            break
    return cand or []

def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__); return 2
    question = sys.argv[1]
    b = ObrainBridgeRO(OBRAIN, STORE)
    facts = pick_facts(question, b)
    b.close()
    if not facts:
        # fallback : fait connu du store
        bb = ObrainBridgeRO(OBRAIN, STORE)
        r = bb.query("MATCH (n:Function {qualified_name: 'tmp.linux_git.drivers.net."
                     "ethernet.intel.e1000.e1000_probe'}) RETURN n.text LIMIT 1")
        bb.close()
        facts = [str(r[0][0])[:1400]] if r and r[0] and r[0][0] else []
    fact = "\n\n".join(facts)
    # santé serveur
    try:
        with urllib.request.urlopen(BASE.replace("/jspace/inject_knowledge", "/health"), timeout=3) as rr:
            if json.loads(rr.read().decode()).get("status") != "ok":
                print("Serveur non prêt — lancer ops-gpu16/run_pont_vk.sh"); return 1
    except Exception:
        print("Serveur injoignable — lancer ops-gpu16/run_pont_vk.sh"); return 1

    t0 = time.time()
    ex = post_json(BASE, {"fact_text": fact, "extract_only": True}, timeout=600)
    extract_s = time.time() - t0
    ref = str(ex.get("text", "")).replace("kv_state:", "")
    print(f"[obrain] fait: {len(fact)} chars ({ex.get('fact_tokens')} tok) | "
          f"extract {extract_s:.1f}s | blob {ex.get('blob_bytes', 0)/2**20:.0f} MiB (K4 i8) | ref {ref[:10]}…")
    print(f"[obrain] QUESTION : {question}\n")
    t0 = time.time()
    r = post_json(BASE, {"state_ref": ref, "prompt": question,
                         "n_generate": N_GEN, "greedy": True}, timeout=600)
    wall = time.time() - t0
    ans = str(r.get("text") or "").strip()
    m = metrics_from_inject(r)
    print("RÉPONSE :")
    print(ans[:1600] if ans else "(vide)")
    print(f"\n[métriques wall] TFPT≈{m['tfpt_ms']} ms serveur (wall total {wall:.1f}s) | "
          f"tok/s≈{N_GEN/max(wall-2, 0.1):.2f} | state_load {m['fact_or_state_load_ms']:.0f} ms")
    return 0

if __name__ == "__main__":
    sys.exit(main())
