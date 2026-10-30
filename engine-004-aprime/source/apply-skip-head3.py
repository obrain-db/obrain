#!/usr/bin/env python3
# ENGINE-005F-R2 : retire la PROJECTION DE SORTIE du graphe en conservant un sink valide.
#
# La projection est construite (build_lora_mm) mais son resultat n'est PLUS la cible du graphe :
# un ggml_dup du tenseur d'entree de la projection devient le sink. Le sous-graphe de sortie
# (projection + copie logits) devient alors inatteignable depuis les cibles, donc non planifie.
#
# Sans GGML_E5F_SKIP_OUTPUT_HEAD : comportement strictement inchange.
# Usage : python3 apply-skip-head3.py <src/models/qwen35.cpp>
import sys, os

ANCHOR = """    // LM head
    cur = build_lora_mm(model.output, cur, model.output_s);

    cb(cur, "result_output", -1);
    res->t_logits = cur;

    ggml_build_forward_expand(gf, cur);"""

REPL = """    // LM head
    // [ENGINE-005F-R2] isolation de la PROJECTION DE SORTIE.
    // ON  : le tenseur de projection est la cible du graphe (comportement normal).
    // OFF : la projection est construite mais un ggml_dup du tenseur d'entree devient la cible ;
    //       le sous-graphe de sortie n'est plus atteignable depuis les cibles.
    static const bool e5f_skip_env = (getenv("GGML_E5F_SKIP_OUTPUT_HEAD") != nullptr);

    ggml_tensor * e5f_pre_proj = cur;                 // entree exacte de la projection
    cur = build_lora_mm(model.output, cur, model.output_s);

    cb(cur, "result_output", -1);
    res->t_logits = cur;

    if (!e5f_skip_env) {
        ggml_build_forward_expand(gf, cur);
    } else {
        // sink de remplacement : copie du tenseur PRE-projection (aucun nouveau calcul
        // fonctionnel requis par le modele ; le resultat n'est jamais lu).
        ggml_tensor * e5f_sink = ggml_dup(ctx0, e5f_pre_proj);
        cb(e5f_sink, "e5f_sink_pre_projection", -1);
        ggml_build_forward_expand(gf, e5f_sink);
        fprintf(stderr, "E5F_R2_HEAD_OFF_SINK=dup(pre_projection)\\n");
        res->t_logits = nullptr;
    }"""

def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: apply-skip-head3.py <src/models/qwen35.cpp>")
    path = sys.argv[1]
    text = open(path, encoding="utf-8").read()
    if "[ENGINE-005F-R2]" in text:
        raise SystemExit("DEJA PATCHE")
    n = text.count(ANCHOR)
    if n != 1:
        raise SystemExit("ANCRE INTROUVABLE OU AMBIGUE : %d" % n)
    open(path + ".tmp", "w", encoding="utf-8").write(text.replace(ANCHOR, REPL, 1))
    os.replace(path + ".tmp", path)
    print("ECRIT %s" % path)

if __name__ == "__main__":
    main()
