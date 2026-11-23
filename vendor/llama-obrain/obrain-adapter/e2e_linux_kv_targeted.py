"""e2e_linux_kv_targeted — cohérence ciblée du pont obrain ↔ llama-obrain.

Réutilise les refs d'état K/V extraits lors du run e2e (même serveur, cache RAM)
et pose des questions dont la réponse n'est DÉDUCTIBLE QUE du code injecté
(identifiants/étapes précis du corps de la fonction Linux). La cohérence est
mesurée par la présence des marqueurs attendus dans la réponse générée.

Usage :
  python3 obrain-adapter/e2e_linux_kv_targeted.py [OBRAIN_BIN] [STORE] [PORT] [N_GEN]
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
N_GEN = int(sys.argv[4]) if len(sys.argv) > 4 else 140
BASE = f"http://127.0.0.1:{PORT}/jspace/inject_knowledge"

# refs d'états extraits au run e2e (même serveur) → fait = code réel du store Linux
FACTS: List[Dict[str, Any]] = [
    {
        "name": "m_can_start_xmit", "ref": "4cbd155368abfee4",
        "qn": "tmp.linux_git.drivers.net.can.m_can.m_can_start_xmit",
        "markers": ["bus_off", "bus-off", "dropped", "peripheral", "clean",
                    "netdev_tx_ok", "frame_len", "tx_handler"],
        "question": ("You are given the real source code of the Linux function "
                     "m_can_start_xmit as knowledge. Describe precisely: what does "
                     "it return when can_dev_dropped_skb() returns true, what does it "
                     "do when the CAN state is CAN_STATE_BUS_OFF, and which two "
                     "functions does it dispatch to depending on is_peripheral?"),
    },
    {
        "name": "rkcanfd_open", "ref": "009207e89a447b46",
        "qn": "tmp.linux_git.drivers.net.can.rockchip.rkcanfd_open",
        "markers": ["open_candev", "runtime", "chip_start", "offload",
                    "irqf_shared", "shared", "netif_start_queue", "request_irq",
                    "out_close_candev", "goto"],
        "question": ("You are given the real source code of the Linux function "
                     "rkcanfd_open as knowledge. Describe the exact initialization "
                     "sequence it performs in order, which interrupt flag it passes "
                     "to request_irq, and how it unwinds (goto error labels) when "
                     "pm_runtime_resume_and_get or request_irq fails."),
    },
    {
        "name": "flexcan_close", "ref": "642a40bf2defc71f",
        "qn": "tmp.linux_git.drivers.net.can.flexcan.flexcan_close",
        "markers": ["netif_stop_queue", "interrupts_disable", "free_irq", "offload",
                    "transceiver", "close_candev", "quirks", "secondary",
                    "chip_stop", "runtime"],
        "question": ("You are given the real source code of the Linux function "
                     "flexcan_close as knowledge. Describe in order every cleanup "
                     "step it performs: queue stop, interrupt disable, which IRQs are "
                     "freed under FLEXCAN_QUIRK_SECONDARY_MB_IRQ and "
                     "FLEXCAN_QUIRK_NR_IRQ_3, and what happens to the rx offload and "
                     "the transceiver."),
    },
    {
        "name": "sja1000_open", "ref": "09ee7ab894b01953",
        "qn": "tmp.linux_git.drivers.net.can.sja1000.sja1000_open",
        "markers": ["reset", "open_candev", "request_threaded_irq", "threaded",
                    "eagain", "irq_flags", "custom", "sja1000_start",
                    "netif_start_queue"],
        "question": ("You are given the real source code of the Linux function "
                     "sja1000_open as knowledge. Describe: how the chip is put into "
                     "reset mode, in which case request_threaded_irq is skipped "
                     "(SJA1000_CUSTOM_IRQ_HANDLER), what is returned if that irq "
                     "request fails, and which function starts the chip at the end."),
    },
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


def extract_fact_from_store(qn: str) -> str:
    b = ObrainBridgeRO(OBRAIN_BIN, STORE)
    rows = b.query(f"MATCH (n:Function {{qualified_name: '{qn}'}}) "
                   "RETURN n.text LIMIT 1")
    b.close()
    return (rows[0][0] if rows and rows[0] else "")[:700]


def main() -> int:
    results: Dict[str, Any] = {"port": PORT, "n_generate": N_GEN, "mode": "CPU",
                               "date": time.strftime("%Y-%m-%d %H:%M:%S"),
                               "cases": []}
    if not wait_health():
        print("FATAL: serveur non joignable"); return 2

    for c in FACTS:
        out: Dict[str, Any] = {"name": c["name"], "ref": c["ref"]}
        # ref manquant (serveur redémarré) → reconstruction à la demande depuis obrain
        try:
            r0 = post_json(BASE, {"state_ref": c["ref"], "prompt": "ping",
                                  "n_generate": 2, "greedy": True}, timeout=60)
            ref = c["ref"]
        except Exception:
            fact = extract_fact_from_store(c["qn"])
            ex = post_json(BASE, {"fact_text": fact, "extract_only": True},
                           timeout=240)
            ref = str(ex.get("text", "")).replace("kv_state:", "")
            out["reconstructed"] = True
            out["fact_chars"] = len(fact)
        t0 = time.time()
        r = post_json(BASE, {"state_ref": ref, "prompt": c["question"],
                             "n_generate": N_GEN, "greedy": True}, timeout=600)
        wall = time.time() - t0
        m = metrics_from_inject(r)
        m["wall_s"] = round(wall, 2)
        out["metrics"] = {k: m[k] for k in
                          ("tfpt_ms", "tokens_per_s", "fact_or_state_load_ms",
                           "query_decode_ms", "n_tokens", "wall_s")}
        ans = str(r.get("text") or "")
        low = ans.lower()
        hits = [mk for mk in c["markers"] if mk in low]
        out["markers_hits"] = hits
        out["recall"] = round(len(hits) / len(c["markers"]), 3)
        out["answer"] = ans
        print(f"\n=== {c['name']} (ref {ref[:12]}) — rappel {out['recall']} "
              f"hits={hits}")
        print("  metrics:", out["metrics"])
        print("  réponse :", ans[:600].replace("\n", " "))
        results["cases"].append(out)

    os.makedirs("/tmp/e2e-linux-kv", exist_ok=True)
    with open("/tmp/e2e-linux-kv-targeted-report.json", "w") as fh:
        json.dump(results, fh, ensure_ascii=False, indent=2)
    md = ["# Cohérence ciblée — injection KV par ref (Linux git → obrain → llama)",
          "",
          f"- Date : {results['date']} — CPU, n_generate={N_GEN}",
          "- Marqueurs = identifiants/étapes présents UNIQUEMENT dans le code injecté.",
          "",
          "| fonction | ref | state-load (ms) | TFPT (ms) | tok/s | rappel marqueurs |",
          "|----------|-----|-----------------|-----------|-------|------------------|"]
    for c in results["cases"]:
        mt = c["metrics"]
        md.append("| `%s` | `%s` | %s | %s | %s | **%.2f** (%s) |" % (
            c["name"], c["ref"][:12], mt.get("fact_or_state_load_ms"),
            mt.get("tfpt_ms"), mt.get("tokens_per_s"), c["recall"],
            ", ".join(c["markers_hits"])))
    md += ["", "## Réponses complètes", ""]
    for c in results["cases"]:
        md.append(f"### {c['name']}")
        md.append("```"); md.append(c["answer"]); md.append("```"); md.append("")
    with open("/tmp/e2e-linux-kv-targeted-report.md", "w") as fh:
        fh.write("\n".join(md))
    avg = (sum(c["recall"] for c in results["cases"]) / len(results["cases"])
           if results["cases"] else 0.0)
    print(f"\nRAPPEL MOYEN marqueurs ciblés : {avg:.2f} — "
          "rapport /tmp/e2e-linux-kv-targeted-report.md + .json")
    return 0 if avg >= 0.4 else 5


if __name__ == "__main__":
    sys.exit(main())
