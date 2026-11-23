"""Self-test C3 (aucun serveur llama requis) : payloads conformes aux schémas
/jspace + bridge RO fonctionnel sur le store dédié + sélecteurs déterministes.
Usage : python3 obrain-adapter/selftest.py [OBRAIN_BIN] [STORE]
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from adapter import (ObrainBridgeRO, fact_from_node, metrics_from_inject,
                     payload_compare_generation, record_kv_state_ref,
                     payload_extract_state, payload_inject_knowledge, payload_inject_state,
                     payload_readout_forward, top_candidates_surprise)


def fail(msg: str) -> None:
    print("FAIL:", msg)
    sys.exit(1)


def main() -> int:
    obrain_bin = sys.argv[1] if len(sys.argv) > 1 else \
        "/home/triviere/projects/banc-obrain/target/release/obrain"
    store = sys.argv[2] if len(sys.argv) > 2 else "/tmp/infmem.obrain"
    if not os.path.exists(store):
        subprocess.run([obrain_bin, "init", store], check=True, capture_output=True)

    # 0. Métriques TFPT/tok-s depuis une réponse typique (directives utilisateur).
    m = metrics_from_inject({"per_token_ms": [500.0, 520.0, 480.0], "query_decode_ms": 900.0, "fact_decode_ms": 97.0})
    assert m["tfpt_ms"] == 500.0 and abs(m["tokens_per_s"] - 2.0) < 1e-6, m
    assert metrics_from_inject({})["tfpt_ms"] is None

    # 1. Payloads conformes aux schémas documentés (read depuis server-context.cpp).
    cmp = payload_compare_generation("Bonjour", scale=0.15, n_generate=24)
    assert set(cmp) == {"prompt", "scale", "n_generate"} and cmp["scale"] == 0.15, cmp
    inj = payload_inject_knowledge("Fait nouveau", "prompt", n_generate=24)
    assert set(inj) == {"fact_text", "prompt", "n_generate"}, inj
    ro = payload_readout_forward([0.0] * 128, 128)
    assert set(ro) == {"h"} and len(ro["h"]) == 128
    ex = payload_extract_state("fait")
    assert ex == {"fact_text": "fait", "extract_only": True}, ex
    inj_r = payload_inject_state("abc", "prompt")
    assert set(inj_r) == {"state_ref", "prompt", "n_generate", "greedy"} and inj_r["state_ref"] == "abc"
    try:
        payload_readout_forward([0.0] * 4, 128)
        fail("readout_forward doit exiger len(h)==n_embd")
    except ValueError:
        pass

    # 2. Bridge RO externe (process séparé) sur le store dédié.
    b = ObrainBridgeRO(obrain_bin, store)
    rows = b.query("MATCH (n) RETURN count(*) AS c")
    print("bridge RO count(n):", rows)
    try:
        sr = b.query("CALL obrain.search_surprise(0, 5, false) "
                     "YIELD node_id, surprise RETURN node_id, surprise")
        print("search_surprise(0):", sr)
    except RuntimeError as e:
        print("search_surprise note:", e)

    # 3. Sélecteurs déterministes sur un mini graphe (store éphémère).
    with tempfile.TemporaryDirectory() as td:
        s2 = os.path.join(td, "mini.obrain")
        subprocess.run([obrain_bin, "init", s2], check=True, capture_output=True)
        mb = ObrainBridgeRO(obrain_bin, s2)
        # Création de nœuds via un writer one-shot (CLI RW sur store éphémère).
        for label, qn in (("L", "fact:alpha"), ("L", "fact:beta")):
            subprocess.run([obrain_bin, "query", s2,
                            f"CREATE (:L {{qualified_name: '{qn}', text: 'insight {qn}'}})"],
                           check=True, capture_output=True)
        record_kv_state_ref(obrain_bin, s2, "fact:alpha", "eed87f05a1aeb5c8")
        rows = mb.query("MATCH (n {qualified_name: 'fact:alpha'}) RETURN n.kv_state_ref LIMIT 1")
        assert rows and rows[0] and rows[0][0] == "eed87f05a1aeb5c8", rows
        cand = top_candidates_surprise(mb, 0, 5)  # fenêtre 0 = tout (vide sans CDC)
        assert isinstance(cand, list)
        txt = fact_from_node(mb, 1, ("text", "name"))
        print("candidats (0 attendu, store sans CDC):", cand)
        print("fact_from_node(1):", txt)
        assert txt is None or isinstance(txt, str)
        mb.close()

    b.close()
    print("SELFTEST C3 OK (payloads + bridge RO + sélecteurs)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
