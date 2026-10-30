#!/usr/bin/env python3
"""ENGINE-008 scorer : GSM8K (parsing FINAL: <number>, arithmetique EXACTE).

Aucune inference. Aucun float binaire pour decider une egalite.
"""
import re
from decimal import Decimal, InvalidOperation
from fractions import Fraction

SCORER_ID = "gsm8k_v1"
VERSION = "1"
INPUT_SCHEMA = ["item_id", "reference_raw", "response_text"]
OUTPUT_SCHEMA = ["item_id", "reference_canonical", "parsed_value", "n_final_lines", "correct", "reason"]

FINAL_RE = re.compile(r"^\s*FINAL:\s*(.*?)\s*$", re.MULTILINE)
REF_SPLIT = "####"


class GSM8KScorerError(Exception):
    pass


def reference_from_upstream(answer_field):
    """Reference canonique = ce qui suit '####' dans le champ upstream."""
    if REF_SPLIT not in answer_field:
        raise GSM8KScorerError("champ upstream sans '####' : %r" % answer_field[:80])
    ref = answer_field.split(REF_SPLIT)[-1].strip()
    if not ref:
        raise GSM8KScorerError("reference vide apres '####'")
    return ref


_NUM_CLEAN = re.compile(r"^[+-]?[\d,]*\.?\d+(?:[eE][+-]?\d+)?$")
_FRAC = re.compile(r"^([+-]?\d+)\s*/\s*(\d+)$")


def canonical_number(text):
    """Rend une valeur EXACTE (Fraction ou Decimal) ou leve.

    Supporte : entiers signes, decimaux, fractions a/b, notation scientifique,
    separateurs de milliers quand non ambigus.
    """
    s = text.strip()
    if not s:
        raise GSM8KScorerError("champ numerique vide")
    # separateurs de milliers : uniquement si bien formes (1,234 / 1,234,567)
    if "," in s:
        if not re.match(r"^[+-]?\d{1,3}(,\d{3})+(\.\d+)?$", s):
            raise GSM8KScorerError("virgules ambigues : %r" % s)
        s = s.replace(",", "")
    m = _FRAC.match(s)
    if m:
        num = int(m.group(1)); den = int(m.group(2))
        if den == 0:
            raise GSM8KScorerError("denominateur nul : %r" % text)
        return Fraction(num, den)
    if not _NUM_CLEAN.match(s):
        raise GSM8KScorerError("nombre mal forme : %r" % text)
    try:
        if "e" in s.lower() or "." in s:
            # Decimal -> Fraction : arithmetique exacte, aucun float binaire
            return Fraction(Decimal(s))
        return Fraction(int(s), 1)
    except (InvalidOperation, ValueError) as e:
        raise GSM8KScorerError("nombre illisible : %r (%s)" % (text, e))


def extract_final_lines(response_text):
    return [m.group(1) for m in FINAL_RE.finditer(response_text)]


def score_item(item_id, reference_raw, response_text):
    try:
        ref = canonical_number(reference_from_upstream(reference_raw))
    except GSM8KScorerError as e:
        raise GSM8KScorerError("%s : reference : %s" % (item_id, e))
    finals = extract_final_lines(response_text)
    if len(finals) == 0:
        return dict(item_id=item_id, reference_canonical=str(ref), parsed_value=None,
                    n_final_lines=0, correct=False, reason="no_final_line")
    if len(finals) > 1:
        return dict(item_id=item_id, reference_canonical=str(ref), parsed_value=None,
                    n_final_lines=len(finals), correct=False, reason="multiple_final_lines")
    try:
        got = canonical_number(finals[0])
    except GSM8KScorerError as e:
        return dict(item_id=item_id, reference_canonical=str(ref), parsed_value=None,
                    n_final_lines=1, correct=False, reason="malformed_number:%s" % e)
    return dict(item_id=item_id, reference_canonical=str(ref), parsed_value=str(got),
                n_final_lines=1, correct=(got == ref), reason="ok" if got == ref else "wrong_number")


def selftest():
    out = []
    def chk(name, cond, detail=""):
        out.append("%-52s %s %s" % (name, "PASS" if cond else "FAIL", detail))
        return cond
    ok = True
    REF = "reasoning here\n#### 42"
    cases = [
        ("FINAL: 42", REF, True, "exact integer"),
        ("FINAL: -42", "x #### -42", True, "negative"),
        ("FINAL: 1,234", "x #### 1234", True, "thousands separator"),
        ("FINAL: 0.5", "x #### 1/2", True, "decimal vs fraction"),
        ("FINAL: 2/4", "x #### 1/2", True, "fraction reduction"),
        ("FINAL: 5e2", "x #### 500", True, "scientific notation"),
        ("no final here", REF, False, "missing FINAL"),
        ("FINAL: 1\nFINAL: 2", REF, False, "two FINAL lines"),
        ("FINAL: forty-two", REF, False, "non numeric"),
        ("FINAL: 42 apples", REF, False, "trailing semantic junk"),
        ("FINAL: 41", REF, False, "wrong number"),
    ]
    for resp, ref, want, label in cases:
        r = score_item("t", ref, resp)
        ok &= chk(label, r["correct"] == want, "(reason=%s)" % r["reason"])
    # separateurs ambigus -> erreur dure a l extraction, score 0 propre
    r = score_item("t", "x #### 1234", "FINAL: 1,23")
    ok &= chk("ambiguous_separator_rejected", (not r["correct"]) and r["reason"].startswith("malformed"))
    # reference sans #### -> erreur dure
    try:
        score_item("t", "42", "FINAL: 42")
        ok &= chk("reference_without_marker", False, "aucune erreur levee")
    except GSM8KScorerError as e:
        ok &= chk("reference_without_marker", True, str(e)[:50])
    # arithmetique exacte : 1/3 != 0.333
    r = score_item("t", "x #### 1/3", "FINAL: 0.333")
    ok &= chk("exact_arithmetic_1_3_vs_0.333", not r["correct"])
    return bool(ok), out


if __name__ == "__main__":
    import sys
    o, lines = selftest()
    print("\n".join(lines))
    print("GSM8K_SCORER_SELFTEST=%s" % ("PASS" if o else "FAIL"))
    sys.exit(0 if o else 1)
