#!/usr/bin/env python3
"""ENGINE-008 HARNESS -- recouvrement d une autorite gelee depuis un script
survivant, avec VERIFICATION contre le SHA publie.

Le texte de TASK_PROMPT_CONTRACTS_v1 etait litteralement embarque dans
source/e8-scorers-artifacts.py (litteral assigne a la variable `contracts`,
ligne 127). On l extrait par AST -- jamais a la main -- on le materialise,
et on verifie que son SHA256 egale l autorite publiee :

  TASK_PROMPT_CONTRACTS_V1_SHA256=
  fa30f16d95a564491e6d02666350ba783d6f44493ea5d04cef024e92ef0c3bc6

Un recouvrement qui ne retombe PAS sur le SHA publie n est pas un
recouvrement : il est rejete.
"""
import ast
import hashlib
import os

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
SRC = os.path.join(ROOT, "source", "e8-scorers-artifacts.py")
OUT = HERE

AUTHORITY = "fa30f16d95a564491e6d02666350ba783d6f44493ea5d04cef024e92ef0c3bc6"


def extract_literal(path, varname):
    tree = ast.parse(open(path, encoding="utf-8").read())
    for node in ast.walk(tree):
        if isinstance(node, ast.Assign):
            for t in node.targets:
                if isinstance(t, ast.Name) and t.id == varname:
                    return ast.literal_eval(node.value)
    raise SystemExit("litteral %r introuvable dans %s" % (varname, path))


def atomic(path, text):
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, path)


def main():
    txt = extract_literal(SRC, "contracts")
    sha = hashlib.sha256(txt.encode("utf-8")).hexdigest()
    ok = (sha == AUTHORITY)
    lines = [
        "ENGINE-008 HARNESS -- RECOUVREMENT D AUTORITE GELEE",
        "=" * 72,
        "RECOVERY_METHOD=AST_LITERAL_EXTRACTION",
        "RECOVERY_SOURCE=%s" % SRC,
        "RECOVERY_SOURCE_VAR=contracts",
        "ARTIFACT=TASK_PROMPT_CONTRACTS_v1.txt",
        "ARTIFACT_BYTES=%d" % len(txt.encode("utf-8")),
        "ARTIFACT_SHA256=%s" % sha,
        "AUTHORITY_SHA256=%s" % AUTHORITY,
        "TASK_PROMPT_CONTRACTS_V1_RECOVERED=%s" % ("YES" if ok else "NO"),
        "VERIFICATION=%s" % ("SHA256_EXACT_MATCH" if ok else "MISMATCH_REJECTED"),
        "",
    ]
    if ok:
        lines += [
            "Le texte recouvre est BIT-IDENTIQUE a l autorite gelee publiee.",
            "L autorite TASK_PROMPT_CONTRACTS_v1 n est donc plus une entree",
            "manquante : elle est restauree et verifiee.",
            "",
            "RAPPEL DES CONTRAINTES QU ELLE FIXE (extrait) :",
            "  MMLU-PRO : scoring = CHOICE_CONDITIONAL_LOGLIKELIHOOD",
            "             NORMALISATION_PAR_LONGUEUR=NON",
            "             tie_policy = plus PETIT index parmi les maxima ex aequo",
            "  GSM8K    : max_new_tokens=1024, GREEDY=YES",
            "             parser = ^\\s*FINAL:\\s*(.*?)\\s*$",
            "  MBPP     : max_new_tokens=1024, GREEDY=YES",
            "  IFEVAL   : max_new_tokens=4096, GREEDY=YES",
            "  COMMUN   : GREEDY_REQUIRED=YES",
            "             TASK_AGGREGATION_MODE=FOUR_CATEGORY_MACRO_AVERAGE",
            "             BOOTSTRAP_RESAMPLES=10000",
            "",
            "=> les limites de generation exigees par §38 sont CONFIRMEES ici :",
            "   GSM8K_MAX_NEW_TOKENS=1024, MBPP_MAX_NEW_TOKENS=1024,",
            "   IFEVAL_MAX_NEW_TOKENS=4096.",
            "",
        ]
    else:
        lines += ["REJET : le texte recouvre ne correspond PAS a l autorite.", ""]

    atomic(os.path.join(OUT, "TASK_PROMPT_CONTRACTS_v1_RECOVERED.txt"),
           "\n".join(lines))
    if ok:
        atomic(os.path.join(OUT, "TASK_PROMPT_CONTRACTS_v1.txt"), txt)
        atomic(os.path.join(OUT, "TASK_PROMPT_CONTRACTS_v1.sha256"),
               sha + "  TASK_PROMPT_CONTRACTS_v1.txt\n")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
