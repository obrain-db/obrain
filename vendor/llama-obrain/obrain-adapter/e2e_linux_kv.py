"""e2e_linux_kv — validation bout-en-bout du branchement DIRECT obrain ↔ llama-obrain
sur la base Linux (store substrate `/tmp/kv-linux-driversnet.obrain`).

Flux par « fait » (fonction Linux réelle, docstring lue DEPUIS le store obrain) :
  1. extract_only  → état K/V en RAM, ref déterministe (reconstruction à la demande) ;
  2. inject par ref → prompt décodé APRÈS l'état (zéro prefill du fait, zéro token texte
     visible) ; métriques TFPT / tokens/s / state-load ;
  3. cohérence : rappel des termes significatifs du fait dans la réponse générée ;
  4. 2e inject du MÊME ref (autre question) → prouve la réutilisation du cache RAM.

Usage :
  python3 obrain-adapter/e2e_linux_kv.py [OBRAIN_BIN] [STORE] [PORT] [N_GENERATE]

Sorties : /tmp/e2e-linux-kv-report.json + /tmp/e2e-linux-kv-report.md
Validation CPU uniquement ; états maintenus en RAM (fichier disque = cache optionnel).
"""
from __future__ import annotations

import json
import os
import re
import sys
import time
from typing import Any, Dict, List, Optional, Tuple

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from adapter import (ObrainBridgeRO, metrics_from_inject, post_json)

# ---------------------------------------------------------------------------

OBRAIN_BIN = sys.argv[1] if len(sys.argv) > 1 else \
    "/home/triviere/projects/banc-obrain/target/release/obrain"
STORE = sys.argv[2] if len(sys.argv) > 2 else "/tmp/kv-linux-driversnet.obrain"
PORT = int(sys.argv[3]) if len(sys.argv) > 3 else 8099
N_GEN = int(sys.argv[4]) if len(sys.argv) > 4 else 48
BASE = f"http://127.0.0.1:{PORT}/jspace/inject_knowledge"
N_FACTS = 4            # nombre de faits Linux distincts
MAX_FACT_CHARS = 700   # plafond docstring (≈ ≤ 200 tok, slot 768 ok)

STOPWORDS = {
    "the", "and", "for", "with", "this", "that", "from", "into", "upon", "will",
    "function", "struct", "static", "return", "void", "int", "const", "char",
    "unsigned", "long", "short", "device", "driver", "drivers", "null", "linux",
    "kernel", "net", "when", "used", "use", "uses", "using", "must", "should",
    "have", "has", "been", "are", "was", "were", "not", "its", "his", "her",
    "their", "they", "them", "you", "your", "our", "can", "may", "might", "all",
    "any", "each", "both", "also", "only", "over", "under", "more", "most",
    "some", "such", "than", "then", "there", "here", "where", "which", "whose",
    "what", "while", "through", "during", "after", "before", "between",
    "however", "therefore", "other", "another", "one", "two", "first", "second",
    "new", "old", "own", "same", "well", "good", "see", "via", "per", "etc",
    "eg", "ie", "i.e", "e.g", "addr", "data", "size", "value", "values",
}


def top_terms(text: str, k: int = 14) -> List[str]:
    """Termes significatifs d'un texte (identifiants ≥ 4 car., stoplist filtrée),
    triés par fréquence — proxy pour la mesure de cohérence réponse↔fait."""
    toks = re.findall(r"[A-Za-z_][A-Za-z0-9_]{3,}", text)
    seen: Dict[str, int] = {}
    for t in toks:
        tl = t.lower()
        if tl in STOPWORDS or tl.startswith("http"):
            continue
        seen[tl] = seen.get(tl, 0) + 1
    ranked = sorted(seen.items(), key=lambda kv: (-kv[1], kv[0]))
    return [t for t, _ in ranked[:k]]


def term_recall(fact: str, answer: str) -> Tuple[float, List[str], List[str]]:
    """Rappel des termes du fait présents dans la réponse (insensible à la casse)."""
    terms = top_terms(fact)
    low = answer.lower()
    hits = [t for t in terms if t in low]
    return (len(hits) / len(terms) if terms else 0.0), hits, terms


def wait_health(url: str, timeout_s: float = 240.0) -> bool:
    import urllib.request
    t0 = time.time()
    while time.time() - t0 < timeout_s:
        try:
            with urllib.request.urlopen(url.replace("/jspace/inject_knowledge",
                                                    "/health"), timeout=3) as r:
                if json.loads(r.read().decode()).get("status") == "ok":
                    return True
        except Exception:
            pass
        time.sleep(5)
    return False


def store_counts(b: ObrainBridgeRO) -> Dict[str, Any]:
    def one(q: str) -> Any:
        try:
            r = b.query(q, limit=5)
            return r[0][0] if r else None
        except Exception:
            return None
    return {
        "n_nodes": one("MATCH (n) RETURN count(*) AS c"),
        "n_function": one("MATCH (n:Function) RETURN count(*) AS c"),
        "n_file": one("MATCH (n:File) RETURN count(*) AS c"),
        "sample": one("MATCH (n:Function {qualified_name: 'tmp.linux_git.drivers.net."
                      "ethernet.intel.e1000.e1000_probe'}) RETURN n.text LIMIT 1"),
    }


def discover_facts(b: ObrainBridgeRO, n_facts: int = 4) -> List[Dict[str, str]]:
    """Sélectionne n faits réels depuis le store (docstrings kernel riches, familles
    de drivers distinctes) — choix déterministe (tri par longueur de texte)."""
    q = ("MATCH (n:Function) WHERE n.text IS NOT NULL "
         "RETURN n.qualified_name, n.file_path, n.text LIMIT 2500")
    rows = b.query(q, limit=2500)
    cands: List[Dict[str, str]] = []
    for r in rows:
        if len(r) < 3 or not r[2]:
            continue
        txt = str(r[2]).strip()
        if not (300 <= len(txt) <= MAX_FACT_CHARS):
            continue
        qn = str(r[0]); fp = str(r[1])
        name = qn.rsplit(".", 1)[-1] if "." in qn else qn
        if not re.search(r"(probe|_open|_init|_setup|_poll|_xmit|_close)$", name):
            continue
        cands.append({"qn": qn, "name": name, "file": fp, "text": txt})
    # familles = 1er répertoire significatif sous drivers/net (ou basename fichier)
    def family(c: Dict[str, str]) -> str:
        m = re.search(r"drivers/net/([^/]+(?:/[^/]+)?)", c["file"])
        return m.group(1) if m else c["file"]
    cands.sort(key=lambda c: -len(c["text"]))
    picked: List[Dict[str, str]] = []
    fams = set()
    for c in cands:
        fam = family(c)
        if fam in fams:
            continue
        fams.add(fam)
        picked.append(c)
        if len(picked) >= n_facts:
            break
    return picked


def do_extract(fact_text: str, timeout_s: float = 240.0) -> Dict[str, Any]:
    t0 = time.time()
    ex = post_json(BASE, {"fact_text": fact_text, "extract_only": True},
                   timeout=timeout_s)
    wall = time.time() - t0
    ref = str(ex.get("text", "")).replace("kv_state:", "")
    return {"ref": ref, "wall_s": round(wall, 2),
            "fact_tokens": ex.get("fact_tokens"),
            "blob_bytes": ex.get("blob_bytes")}


def do_inject(ref: str, prompt: str, n_gen: int,
              timeout_s: float = 400.0) -> Dict[str, Any]:
    t0 = time.time()
    r = post_json(BASE, {"state_ref": ref, "prompt": prompt,
                         "n_generate": n_gen, "greedy": True},
                  timeout=timeout_s)
    wall = time.time() - t0
    m = metrics_from_inject(r)
    m["wall_s"] = round(wall, 2)
    m["answer"] = str(r.get("text") or "")
    return m


def main() -> int:
    results: Dict[str, Any] = {
        "store": STORE, "obrain_bin": OBRAIN_BIN, "port": PORT,
        "n_generate": N_GEN, "mode": "CPU", "questions": [],
    }
    if not wait_health(BASE):
        print("FATAL: serveur non joignable"); return 2

    b = ObrainBridgeRO(OBRAIN_BIN, STORE)
    results["store_counts"] = store_counts(b)
    print("store (branchement direct) :", json.dumps(results["store_counts"],
          ensure_ascii=False)[:300])

    facts = discover_facts(b, N_FACTS)
    b.close()
    print(f"{len(facts)} faits Linux sélectionnés depuis le store obrain")
    if not facts:
        print("FATAL: aucun fait exploitable dans le store"); return 3

    for i, f in enumerate(facts):
        print(f"\n=== fait {i + 1}/{len(facts)} : {f['name']} "
              f"({f['file']}) — {len(f['text'])} chars ===")
        q: Dict[str, Any] = {"qn": f["qn"], "file": f["file"],
                             "name": f["name"], "fact_chars": len(f["text"])}
        try:
            ex = do_extract(f["text"])
            q["extract"] = ex
            print("  extract wall %.1fs -> ref %s (%s tok, %s o)" % (
                ex["wall_s"], ex["ref"], ex["fact_tokens"], ex["blob_bytes"]))

            q1 = ("Based on the kernel documentation provided as knowledge, explain "
                  f"the role of the function {f['name']}: what it initializes, the "
                  "main steps it performs, and what it returns.")
            inj = do_inject(ex["ref"], q1, N_GEN)
            q["answer_q1"] = inj
            rec, hits, terms = term_recall(f["text"], inj["answer"])
            q["coherence_q1"] = {"recall": round(rec, 3), "hits": hits,
                                 "terms": terms}
            print("  INJECT q1 metrics:", {k: inj[k] for k in
                  ("tfpt_ms", "tokens_per_s", "fact_or_state_load_ms",
                   "query_decode_ms", "n_tokens", "wall_s")})
            print("  cohérence (rappel termes du fait) : %.2f  hits=%s" %
                  (rec, hits[:8]))
            print("  réponse (début) :",
                  inj["answer"][:200].replace("\n", " "))

            # 2e inject du MÊME ref → cache RAM (state_load doit être ~ms)
            q2 = (f"In this function {f['name']}, which error paths or failure "
                  "conditions are documented?")
            inj2 = do_inject(ex["ref"], q2, N_GEN)
            q["answer_q2_reuse"] = inj2
            print("  INJECT q2 (même ref, cache RAM) metrics:", {
                k: inj2[k] for k in
                ("tfpt_ms", "tokens_per_s", "fact_or_state_load_ms",
                 "query_decode_ms", "n_tokens", "wall_s")})
            print("  réponse q2 (début) :",
                  inj2["answer"][:200].replace("\n", " "))
        except Exception as e:
            q["error"] = repr(e)[:400]
            print("  ERREUR:", repr(e)[:300])
        results["questions"].append(q)

    # --- rapport -----------------------------------------------------------
    os.makedirs("/tmp/e2e-linux-kv", exist_ok=True)
    with open("/tmp/e2e-linux-kv-report.json", "w") as fh:
        json.dump(results, fh, ensure_ascii=False, indent=2)

    md = ["# E2E Linux git → obrain → llama-obrain (injection KV-cache par ref)",
          "", f"- Date : {time.strftime('%Y-%m-%d %H:%M:%S')} (CPU, port {PORT})",
          f"- Store : `{STORE}` — {results['store_counts']}",
          f"- n_generate = {N_GEN}, greedy", "",
          "| # | fonction (fait) | extract (s) | ref | tokens | state-load (ms) | "
          "TFPT (ms) | tok/s | rappel |", "|---|-----------------|-------------|-----|--------|"
          "-----------------|----------|-------|--------|"]
    for i, q in enumerate(results["questions"]):
        ex = q.get("extract") or {}
        inj = q.get("answer_q1") or {}
        co = q.get("coherence_q1") or {}
        md.append("| %d | `%s` | %s | `%s` | %s | %s | %s | %s | %.2f |" % (
            i + 1, q.get("name", "?"), ex.get("wall_s"),
            str(ex.get("ref"))[:12], ex.get("fact_tokens"),
            inj.get("fact_or_state_load_ms"), inj.get("tfpt_ms"),
            inj.get("tokens_per_s"), co.get("recall", 0.0)))
    md += ["", "## Réponses (q1, extrait)", ""]
    for q in results["questions"]:
        inj = q.get("answer_q1") or {}
        md.append(f"### {q.get('name')}")
        md.append("```")
        md.append((inj.get("answer") or "(échec)")[:700])
        md.append("```")
        md.append("")
    with open("/tmp/e2e-linux-kv-report.md", "w") as fh:
        fh.write("\n".join(md))

    ok = sum(1 for q in results["questions"] if q.get("answer_q1"))
    print(f"\nRÉSUMÉ : {ok}/{len(facts)} questions répondues — "
          "rapport /tmp/e2e-linux-kv-report.md + .json")
    return 0 if ok >= max(2, len(facts) - 1) else 4


if __name__ == "__main__":
    sys.exit(main())
