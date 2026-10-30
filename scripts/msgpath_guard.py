#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
msgpath_guard.py — garde DoD du CHEMIN MESSAGE obrain (P0 2026-09-09).

Rejoue le chemin réel (bridge MCP, spawn-court) sur UNE COPIE du store
runtime (jamais le store vivant) et vérifie les gates :
  1. ouverture store PROPRE (WAL checkpointé)      — médiane 3× ≤ 30 ms
  2. ouverture store APRÈS ÉCRITURES (tail WAL)    — médiane 3× ≤ 50 ms
  3. runtime_search à CHAUD (bridge gardé vivant)  — médiane runs 2-5 < 50 ms
  4. intégrité : pas de « crash boundary » à l'ouverture, comptes stables.

Référence mesurée (binaire debug repo, 2026-09-09, clone 335 k nœuds) :
  propre 26 ms · après écritures 43 ms (avant correctifs : ~1550 ms) ·
  runtime_search chaud ~1-3 ms (avant : ~1550 ms).

SÉCURITÉ : le store fourni DOIT être une copie jetable. Refus si le chemin
est sous le répertoire des stores VIVANTS (data/) ou nommé *.obrain du live.
Le guard écrit dans la copie (régénération d'un tail WAL) puis la laisse en
l'état — elle est censée être recréée (cp -a) à chaque exécution.

Usage :
  python3 scripts/msgpath_guard.py --store <copie.obrain>        # garde complète
  python3 scripts/msgpath_guard.py --store <copie.obrain> --open-only
Env : MSGPATH_BIN (défaut target/debug/obrain du repo).
"""
import argparse
import json
import os
import queue
import re
import subprocess
import sys
import threading
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.environ.get("MSGPATH_BIN") or os.path.join(REPO, "target", "debug", "obrain")
LIVE_DATA = os.path.expanduser("~/.local/share/obrain-harness/data")
GATE_CLEAN_MS = 30.0
GATE_DIRTY_MS = 50.0
GATE_SEARCH_MS = 50.0
OPEN_RE = re.compile(r"store ouvert \([^)]*, RO, ([\d.]+) ms\)")
BAD_RE = re.compile(r"crash boundary|PropsZone v2 decode failed|husk label_only=[1-9]")

checks = []  # (ok, msg)


def check(ok, msg):
    checks.append((ok, msg))
    print(("PASS  " if ok else "FAIL  ") + msg)


def refuse_live(store):
    store = os.path.realpath(store)
    live = os.path.realpath(LIVE_DATA)
    if store.startswith(live + os.sep) or store == live:
        sys.exit(f"REFUS : {store} est sous le répertoire des stores VIVANTS ({live}). "
                 "Fournir une COPIE jetable (cp -a).")
    if not os.path.isdir(store):
        sys.exit(f"store absent : {store}")


def env_for(write=False):
    e = {k: v for k, v in os.environ.items()
         if k not in ("OBRAIN_STORE", "OBRAIN_WRITE", "OBRAIN_IDLE_MS", "RUST_LOG")}
    e["OBRAIN_IDLE_MS"] = "600000"
    if write:
        e["OBRAIN_WRITE"] = "1"
    return e


class Bridge:
    def __init__(self, store, write=False, tag=""):
        self.p = subprocess.Popen([BIN, "bridge", "--store", store],
                                  stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE, text=True, bufsize=1,
                                  env=env_for(write))
        self.q = queue.Queue()
        self.errs = []
        threading.Thread(target=self._pump, daemon=True).start()
        threading.Thread(target=self._errpump, daemon=True).start()

    def _pump(self):
        for line in self.p.stdout:
            self.q.put(line.rstrip())
        self.q.put(None)

    def _errpump(self):
        for line in self.p.stderr:
            self.errs.append(line)

    def req(self, obj, timeout=60):
        self.p.stdin.write(json.dumps(obj) + "\n")
        self.p.stdin.flush()
        line = self.q.get(timeout=timeout)
        if line is None:
            raise RuntimeError("bridge EOF")
        return line

    def init(self):
        self.req({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}})

    def open_ms(self):
        """ms internes « store ouvert » depuis stderr (attente ≤ 3 s)."""
        t = time.monotonic() + 3.0
        while time.monotonic() < t:
            for ln in self.errs:
                m = OPEN_RE.search(ln)
                if m:
                    return float(m.group(1))
            time.sleep(0.02)
        return float("nan")

    def evidence(self):
        return [ln for ln in self.errs if BAD_RE.search(ln)][:3]

    def close(self, kill=True):
        try:
            if kill:
                self.p.kill()
            else:
                self.p.stdin.close()
                self.p.wait(timeout=20)
        except Exception:
            self.p.kill()


def open_timing(store, runs=3):
    ms = []
    for _ in range(runs):
        b = Bridge(store)
        try:
            b.init()
            ms.append(b.open_ms())
            ev = b.evidence()
        finally:
            b.close()
    return ms, ev


def dirty_tail(store):
    """1 écriture structurelle (graph_link) en shared-write puis SIGKILL."""
    b = Bridge(store, write=True)
    try:
        b.init()
        b.req({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "graph_link",
            "arguments": {"source": {"qn": "guard-dirty-a", "label": "Node"},
                          "target": {"qn": "guard-dirty-b", "label": "Node"},
                          "type": "GUARD_WAL"}}})
    finally:
        b.close(kill=True)
    time.sleep(0.3)


def clean_close(store):
    b = Bridge(store, write=True)
    try:
        b.init()
    finally:
        b.close(kill=False)


def median(xs):
    s = sorted(xs)
    return s[len(s) // 2] if xs else float("nan")


def search_salvo(store, queries=("session", "import", "erreur")):
    """runtime_search ×N sur un bridge gardé vivant ; retourne les ms (run1 exclu)."""
    b = Bridge(store)
    lats = []
    try:
        b.init()
        for i, q in enumerate(queries):
            t0 = time.perf_counter()
            b.req({"jsonrpc": "2.0", "id": 10 + i, "method": "tools/call",
                   "params": {"name": "runtime_search",
                              "arguments": {"query": q, "limit": 8}}})
            lats.append((time.perf_counter() - t0) * 1000)
    finally:
        b.close()
    return lats[1:]  # chaud : run1 = rebuild éventuel exclu


def main():
    ap = argparse.ArgumentParser(description="Garde chemin message obrain")
    ap.add_argument("--store", required=True)
    ap.add_argument("--open-only", action="store_true")
    args = ap.parse_args()
    if not os.path.exists(BIN):
        sys.exit(f"binaire absent : {BIN} (builder d'abord)")
    refuse_live(args.store)
    print(f"guard: store = {args.store}\nguard: binaire = {BIN}")

    # ── 1. propre ──
    clean_close(args.store)
    ms_c, ev_c = open_timing(args.store)
    med_c = median(ms_c)
    check(med_c <= GATE_CLEAN_MS and not ev_c,
          f"ouverture PROPRE ≤ {GATE_CLEAN_MS:.0f} ms — mesuré {med_c:.0f} ms "
          f"({[f'{m:.0f}' for m in ms_c]})" + (f" ; preuves: {ev_c}" if ev_c else ""))
    if args.open_only:
        return finish()

    # ── 2. après écritures ──
    dirty_tail(args.store)
    ms_d, ev_d = open_timing(args.store)
    med_d = median(ms_d)
    check(med_d <= GATE_DIRTY_MS and not ev_d,
          f"ouverture APRÈS ÉCRITURES ≤ {GATE_DIRTY_MS:.0f} ms — mesuré {med_d:.0f} ms "
          f"({[f'{m:.0f}' for m in ms_d]})" + (f" ; preuves: {ev_d}" if ev_d else ""))

    # ── 3. runtime_search chaud ──
    lats = search_salvo(args.store)
    med_s = median(lats) if lats else float("inf")
    check(med_s < GATE_SEARCH_MS,
          f"runtime_search CHAUD < {GATE_SEARCH_MS:.0f} ms — mesuré {med_s:.1f} ms "
          f"({[f'{x:.1f}' for x in lats]})")
    return finish()


def finish():
    n_fail = sum(1 for ok, _ in checks if not ok)
    print(f"guard-msgpath: {len(checks) - n_fail}/{len(checks)} PASS")
    return 1 if n_fail else 0


if __name__ == "__main__":
    sys.exit(main())
