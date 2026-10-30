#!/usr/bin/env python3
# ENGINE-005E : rend la projection de sortie REELLEMENT evitables pour le bras LOGITS_OFF.
# Aucun effet si la variable GGML_E5E_SKIP_HEAD n'est pas posee : le comportement par defaut est
# inchange (la tete est construite comme avant).
# Usage : python3 apply-skip-head.py <src/models/qwen35.cpp>
import sys, os

ANCHOR = """    // LM head
    cur = build_lora_mm(model.output, cur, model.output_s);

    cb(cur, "result_output", -1);
    res->t_logits = cur;

    ggml_build_forward_expand(gf, cur);"""

REPL = """    // LM head
    // [ENGINE-005E] saut explicite, UNIQUEMENT si la variable est posee ET si aucun token du
    // lot ne demande de logits (inp_out_ids == nullptr). Hors de ces conditions, rien ne change.
    static const bool e5e_skip_env = (getenv("GGML_E5E_SKIP_HEAD") != nullptr);
    const bool e5e_skip_head = e5e_skip_env && (inp_out_ids == nullptr);

    if (!e5e_skip_head) {
        cur = build_lora_mm(model.output, cur, model.output_s);

        cb(cur, "result_output", -1);
        res->t_logits = cur;

        ggml_build_forward_expand(gf, cur);
    } else {
        res->t_logits = nullptr;
    }"""

def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: apply-skip-head.py <src/models/qwen35.cpp>")
    path = sys.argv[1]
    text = open(path, encoding="utf-8").read()
    if "[ENGINE-005E]" in text:
        raise SystemExit("DEJA PATCHE")
    n = text.count(ANCHOR)
    if n != 1:
        raise SystemExit("ANCRE INTROUVABLE OU AMBIGUE : %d" % n)
    open(path + ".tmp", "w", encoding="utf-8").write(text.replace(ANCHOR, REPL, 1))
    os.replace(path + ".tmp", path)
    print("ECRIT %s" % path)

if __name__ == "__main__":
    main()
