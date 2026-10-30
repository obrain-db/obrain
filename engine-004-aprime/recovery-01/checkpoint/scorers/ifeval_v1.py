#!/usr/bin/env python3
"""ENGINE-008 scorer : IFEval (wrapper du scorer OFFICIEL piné).

Ne reimplemente AUCUNE semantique des 25 instructions.
Gate principal = STRICT (follow_all_instructions). LOOSE = diagnostic.
"""
import os
import sys

SCORER_ID = "ifeval_v1"
VERSION = "1"
OFFICIAL_REVISION = "e49bbfe381c9c0e564b937f1c4e163a2273c65cc"
INPUT_SCHEMA = ["item_id", "key", "prompt", "instruction_id_list", "kwargs", "response_text"]
OUTPUT_SCHEMA = ["item_id", "strict", "loose", "n_instructions", "reason"]

# Le paquet officiel s'importe depuis le PARENT du dossier instruction_following_eval.
OFFICIAL_ROOT = os.environ.get("ENGINE008_IFEVAL_ROOT", "")


def _load_official():
    if OFFICIAL_ROOT and OFFICIAL_ROOT not in sys.path:
        sys.path.insert(0, OFFICIAL_ROOT)
    from instruction_following_eval import evaluation_lib  # noqa
    return evaluation_lib


def score_item(item_id, key, prompt, instruction_id_list, kwargs, response_text,
               evaluation_lib=None):
    """Rend dict(item_id, strict, loose, n_instructions, reason).

    Construit l objet InputExample officiel puis appelle le scorer upstream.
    Aucune semantique d instruction n est reecrite ici.
    """
    el = evaluation_lib or _load_official()
    inp = el.InputExample(key=key, instruction_id_list=instruction_id_list,
                          prompt=prompt, kwargs=kwargs)
    # API OFFICIELLE : le scorer indexe la reponse PAR LE PROMPT.
    # Une collision de prompt est donc un defaut dur, verifie en amont.
    prompt_to_response = {prompt: response_text}
    strict = el.test_instruction_following_strict(inp, prompt_to_response)
    loose = el.test_instruction_following_loose(inp, prompt_to_response)
    n = len(instruction_id_list)
    return dict(item_id=item_id, strict=bool(strict.follow_all_instructions),
                loose=bool(loose.follow_all_instructions), n_instructions=n,
                reason="ok")


def selftest():
    """Fixtures locales SANS modele. Le scorer officiel est appele reellement."""
    out = []
    def chk(name, cond, detail=""):
        out.append("%-56s %s %s" % (name, "PASS" if cond else "FAIL", detail))
        return cond
    ok = True
    el = _load_official()
    # 1. contrainte satisfaite : la reponse commence par le mot demande
    ids = ["startend:end_checker"]
    kw = [{"end_phrase": "DONE"}]
    r = score_item("a", "1", "Say hi.", ids, kw, "Hello there.\nDONE", el)
    ok &= chk("strict_pass_end_checker", r["strict"] is True, str(r))
    # 2. contrainte violee
    r = score_item("b", "2", "Say hi.", ids, kw, "Hello there.", el)
    ok &= chk("strict_fail_end_checker", r["strict"] is False, str(r))
    # 3. plusieurs contraintes simultanees, une seule violee -> FAIL
    ids2 = ["startend:end_checker", "punctuation:no_comma"]
    kw2 = [{"end_phrase": "DONE"}, {}]
    r = score_item("c", "3", "x", ids2, kw2, "a, b\nDONE", el)
    ok &= chk("multiple_constraints_one_violated", r["strict"] is False)
    r = score_item("d", "4", "x", ids2, kw2, "a b\nDONE", el)
    ok &= chk("multiple_constraints_all_satisfied", r["strict"] is True)
    # 4. fixture POSITIONNELLE : meme instruction_id deux fois, kwargs distincts
    #    (le motif qui avait ete exclu a tort en DATA)
    ids3 = ["length_constraints:number_words", "length_constraints:number_words"]
    kw3 = [{"relation": "at least", "num_words": 3}, {"relation": "less than", "num_words": 6}]
    r = score_item("e", "5", "x", ids3, kw3, "one two three four", el)
    ok &= chk("positional_duplicate_ids_instantiate", r["n_instructions"] == 2, str(r))
    ok &= chk("positional_duplicate_ids_strict_true", r["strict"] is True, str(r))
    r = score_item("f", "6", "x", ids3, kw3, "one two three four five six seven", el)
    ok &= chk("positional_duplicate_ids_strict_false_when_too_long", r["strict"] is False)
    # 5. LOOSE diagnostic expose
    r = score_item("g", "7", "x", ids, kw, "Hello there.\nDONE", el)
    ok &= chk("loose_field_exposed", r["loose"] is True)
    return bool(ok), out


if __name__ == "__main__":
    import sys as _s
    o, lines = selftest()
    print("\n".join(lines))
    print("IFEVAL_SCORER_SELFTEST=%s" % ("PASS" if o else "FAIL"))
    _s.exit(0 if o else 1)
