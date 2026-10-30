#!/usr/bin/env python3
"""ENGINE-008 HARNESS -- audit §15 : autorite du chat template.

Extrait la CHAINE EXACTE du chat template des DEUX modeles, la materialise,
calcule son SHA256, et caracterise la divergence.

AUCUNE inference. Aucun decode. Aucune reimplementation du template en Python :
on n APPLIQUE pas le template (cela exige llama_chat_apply_template, §16) ;
on constate seulement l identite ou la difference des chaines source.
"""
import difflib
import hashlib
import json
import os
import sys

OUT = os.path.dirname(os.path.abspath(__file__))
AUTH = os.path.join(OUT, "engine008_authority.json")


def load():
    d = json.load(open(AUTH))
    return d["baseline"], d["candidate"]


def main():
    b, c = load()
    tb, tc = b["chat_template"], c["chat_template"]
    lines = []
    def P(s=""):
        lines.append(s)
        print(s)

    P("ENGINE-008 -- AUDIT DU CHAT TEMPLATE (§15)")
    P("=" * 72)
    P("MODE=STATIC_STRING_EXTRACTION_ONLY")
    P("TEMPLATE_APPLIED=NO")
    P("LLAMA_CHAT_APPLY_TEMPLATE_CALLED=NO")
    P("PYTHON_TEMPLATE_REIMPLEMENTATION=NO")
    P()
    P("BASELINE_MODEL_PATH=%s" % b["path"])
    P("BASELINE_MODEL_SHA256=%s" % b["model_sha256"])
    P("BASELINE_CHAT_TEMPLATE_SHA256=%s" % b["chat_template_sha256"])
    P("BASELINE_CHAT_TEMPLATE_BYTES=%d" % len(tb.encode("utf-8")))
    P()
    P("CANDIDATE_MODEL_PATH=%s" % c["path"])
    P("CANDIDATE_MODEL_SHA256=%s" % c["model_sha256"])
    P("CANDIDATE_CHAT_TEMPLATE_SHA256=%s" % c["chat_template_sha256"])
    P("CANDIDATE_CHAT_TEMPLATE_BYTES=%d" % len(tc.encode("utf-8")))
    P()
    match = "YES" if tb == tc else "NO"
    P("CHAT_TEMPLATE_BASELINE_CANDIDATE_MATCH=%s" % match)
    P("CHAT_TEMPLATE_REQUIRED_BY_PROTOCOL=YES")
    P("CHAT_TEMPLATE_GATE=%s" % ("PASS" if tb == tc else "FAIL"))
    P()
    if tb != tc:
        P("=== CARACTERISATION DE LA DIVERGENCE ===")
        i = 0
        while i < min(len(tb), len(tc)) and tb[i] == tc[i]:
            i += 1
        P("FIRST_DIVERGENCE_CHAR_INDEX=%d" % i)
        common_prefix = tb[:i]
        P("COMMON_PREFIX_SHA256=%s"
          % hashlib.sha256(common_prefix.encode("utf-8")).hexdigest())
        P("COMMON_PREFIX_BYTES=%d" % len(common_prefix.encode("utf-8")))
        pfx_lines = common_prefix.count("\n") + 1
        P("COMMON_PREFIX_LINE_COUNT=%d" % pfx_lines)
        P()
        P("DIVERGENCE_REGION = traitement des messages system/developer")
        P("  baseline : gere UN message system (messages[0].role == 'system')")
        P("  candidat : FUSIONNE les messages system/developer consecutifs")
        P("             via un namespace sysns(count, text)")
        P()
        P("--- EXTRAIT BASELINE (au point de divergence) ---")
        P(tb[max(0, i - 120):i + 520])
        P("--- EXTRAIT CANDIDAT (au point de divergence) ---")
        P(tc[max(0, i - 120):i + 520])
        P()
        d = list(difflib.unified_diff(tb.splitlines(), tc.splitlines(),
                                      "baseline_chat_template",
                                      "candidate_chat_template", lineterm="", n=2))
        P("UNIFIED_DIFF_LINE_COUNT=%d" % len(d))
        P("UNIFIED_DIFF_ADDED_LINE_COUNT=%d"
          % sum(1 for l in d if l.startswith("+") and not l.startswith("+++")))
        P("UNIFIED_DIFF_REMOVED_LINE_COUNT=%d"
          % sum(1 for l in d if l.startswith("-") and not l.startswith("---")))
        diff_path = os.path.join(OUT, "CHAT_TEMPLATE_BASELINE_CANDIDATE.diff")
        with open(diff_path, "w", encoding="utf-8", newline="\n") as f:
            f.write("\n".join(d) + "\n")
        P("UNIFIED_DIFF_PATH=%s" % diff_path)
    P()
    P("=== CONSIGNE PROTOCOLE (§15) ===")
    if tb == tc:
        P("ENGINE008_CHAT_TEMPLATE_SHA256=%s" % b["chat_template_sha256"])
    else:
        P("ENGINE008_CHAT_TEMPLATE_SHA256=UNDEFINED")
        P("  Le protocole exige CHAT_TEMPLATE_BASELINE_CANDIDATE_MATCH=YES.")
        P("  Les deux modeles portent des revisions DIFFERENTES du template.")
        P("  Aucune autorite commune ne peut etre definie sans decision humaine :")
        P("  rendre TASK avec deux templates differents violerait l exigence")
        P("  d inputs baseline/candidat bit-identiques (§20).")
        P("  GATE=CHAT_TEMPLATE_BASELINE_CANDIDATE_MISMATCH")

    txt = "\n".join(lines) + "\n"
    with open(os.path.join(OUT, "CHAT_TEMPLATE_AUDIT.txt"), "w",
              encoding="utf-8", newline="\n") as f:
        f.write(txt)
    with open(os.path.join(OUT, "BASELINE_CHAT_TEMPLATE.jinja"), "w",
              encoding="utf-8", newline="") as f:
        f.write(tb)
    with open(os.path.join(OUT, "CANDIDATE_CHAT_TEMPLATE.jinja"), "w",
              encoding="utf-8", newline="") as f:
        f.write(tc)
    print()
    print("ECRIT: CHAT_TEMPLATE_AUDIT.txt, BASELINE_CHAT_TEMPLATE.jinja, "
          "CANDIDATE_CHAT_TEMPLATE.jinja")


if __name__ == "__main__":
    main()
