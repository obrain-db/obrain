#!/usr/bin/env python3
# ENGINE-005F : saut REEL de la tete de sortie (construction ET execution du graphe).
# Aucun effet si GGML_E5F_SKIP_OUTPUT_HEAD n'est pas posee.
# Usage : python3 apply-skip-head2.py <src/models/qwen35.cpp>
import sys, os

ANCHOR = """    // LM head
    cur = build_lora_mm(model.output, cur, model.output_s);

    cb(cur, "result_output", -1);
    res->t_logits = cur;

    ggml_build_forward_expand(gf, cur);"""

REPL = """    // LM head
    // [ENGINE-005F] saut EXPLICITE de la tete de sortie : le noeud n'est ni construit ni ajoute
    // comme cible du graphe. Sans la variable, comportement strictement inchange.
    static const bool e5f_skip_env = (getenv("GGML_E5F_SKIP_OUTPUT_HEAD") != nullptr);
    const bool e5f_skip = e5f_skip_env;

    if (!e5f_skip) {
        cur = build_lora_mm(model.output, cur, model.output_s);

        cb(cur, "result_output", -1);
        res->t_logits = cur;

        ggml_build_forward_expand(gf, cur);
    } else {
        res->t_logits = nullptr;
        fprintf(stderr, "E5F_HEAD_SKIPPED_BUILD=1\\n");
    }"""

def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: apply-skip-head2.py <src/models/qwen35.cpp>")
    path = sys.argv[1]
    text = open(path, encoding="utf-8").read()
    if "[ENGINE-005F]" in text:
        raise SystemExit("DEJA PATCHE")
    n = text.count(ANCHOR)
    if n != 1:
        raise SystemExit("ANCRE INTROUVABLE OU AMBIGUE : %d" % n)
    open(path + ".tmp", "w", encoding="utf-8").write(text.replace(ANCHOR, REPL, 1))
    os.replace(path + ".tmp", path)
    print("ECRIT %s" % path)

if __name__ == "__main__":
    main()
