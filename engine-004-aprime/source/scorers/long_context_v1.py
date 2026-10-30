#!/usr/bin/env python3
"""ENGINE-008 scorer : longueurs long-context (FINAL: <needle>, egalite exacte).

Aucune inference. Egalite exacte apres strip, sensible a la casse.
"""
import re

SCORER_ID = "long_context_v1"
VERSION = "1"
INPUT_SCHEMA = ["item_id", "needle", "response_text"]
OUTPUT_SCHEMA = ["item_id", "parsed_value", "n_final_lines", "correct", "reason"]

FINAL_RE = re.compile(r"^\s*FINAL:\s*(.*?)\s*$", re.MULTILINE)


def score_item(item_id, needle, response_text):
    """Egalite EXACTE apres strip. Pas de lowercase, pas de normalisation
    Unicode, pas de fuzzy, pas de substring."""
    if not isinstance(needle, str) or not needle:
        raise ValueError("%s : needle vide ou non-chaine" % item_id)
    finals = [m.group(1) for m in FINAL_RE.finditer(response_text)]
    if len(finals) == 0:
        return dict(item_id=item_id, parsed_value=None, n_final_lines=0,
                    correct=False, reason="no_final_line")
    if len(finals) > 1:
        return dict(item_id=item_id, parsed_value=None, n_final_lines=len(finals),
                    correct=False, reason="multiple_final_lines")
    got = finals[0]
    return dict(item_id=item_id, parsed_value=got, n_final_lines=1,
                correct=(got == needle), reason="ok" if got == needle else "mismatch")


def score_batch(items):
    """items : [{item_id, needle, response_text}] -> accuracy + detail."""
    rows = [score_item(i["item_id"], i["needle"], i["response_text"]) for i in items]
    n = len(rows)
    acc = (sum(1 for r in rows if r["correct"]) / n) if n else 0.0
    return acc, rows


def selftest():
    out = []
    def chk(name, cond, detail=""):
        out.append("%-52s %s %s" % (name, "PASS" if cond else "FAIL", detail))
        return cond
    ok = True
    N = "RECORD-Z7M4Q9-K2P8"
    cases = [
        ("FINAL: " + N, True, "exact needle"),
        ("FINAL: " + N.lower(), False, "wrong case"),
        ("FINAL: " + N + "X", False, "extra character"),
        ("no final line", False, "missing FINAL"),
        ("FINAL: A\nFINAL: B", False, "two FINAL lines"),
        ("   FINAL: " + N + "   ", True, "outer whitespace tolerated"),
        ("FINAL: RECORD-Z7M4Q9-K2P8 ", True, "trailing space inside field"),
        ("The needle is " + N, False, "substring but no FINAL"),
    ]
    for resp, want, label in cases:
        r = score_item("t", N, resp)
        ok &= chk(label, r["correct"] == want, "(reason=%s)" % r["reason"])
    # pas de substring matching : une reponse plus longue ne passe pas
    r = score_item("t", N, "FINAL: prefix%s" % N)
    ok &= chk("no_substring_matching", not r["correct"])
    # batch
    acc, rows = score_batch([{"item_id": "a", "needle": N, "response_text": "FINAL: " + N},
                             {"item_id": "b", "needle": N, "response_text": "FINAL: nope"}])
    ok &= chk("batch_accuracy", abs(acc - 0.5) < 1e-12 and len(rows) == 2)
    return bool(ok), out


if __name__ == "__main__":
    import sys
    o, lines = selftest()
    print("\n".join(lines))
    print("LONG_CONTEXT_SCORER_SELFTEST=%s" % ("PASS" if o else "FAIL"))
    sys.exit(0 if o else 1)
