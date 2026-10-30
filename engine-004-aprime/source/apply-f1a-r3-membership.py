#!/usr/bin/env python3
"""ENGINE-007B-F1A-Q0-R3 — ajoute l'appartenance au graphe soumis dans le manifeste.

Pour chaque tenseur enregistre, ecrit in_submitted_graph : le pointeur appartient-il
a un graphe effectivement soumis ? C'est la definition operationnelle du paragraphe 4.
"""
import sys

C = "/home/triviere/projects/llama-obrain-cuda-f1a/src/f1a-capture.cpp"

A_HDR = '"buffer_ptr,resident,sched_backend_ptr,sched_backend_name,weight_name\\n");'
B_HDR = '"buffer_ptr,resident,in_submitted_graph,weight_name\\n");'

A_ROW = """                fprintf(rman, "%d,%d,%s,%p,%s,%d,%d,%lld,%lld,%zu,%d,%p,%d,%p,%s,%s\\n",
                        r.target_id, r.activation_capture_id, slots[k].role, (const void*)t,
                        t->name, (int)t->op, (int)t->type,
                        (long long)t->ne[0], (long long)t->nb[0], ggml_nbytes(t),
                        ggml_is_contiguous(t) ? 1 : 0, (void*)t->buffer, resident ? 1 : 0,
                        (void*)sb, sbn, r.weight_name.c_str());"""

B_ROW = """                const int insub = f1a_ptr_in_submitted_graph((const void *) t) ? 1 : 0;
                if (resident && insub) n_res_insub++;
                if (!resident && insub) n_null_insub++;
                fprintf(rman, "%d,%d,%s,%p,%s,%d,%d,%lld,%lld,%zu,%d,%p,%d,%d,%s\\n",
                        r.target_id, r.activation_capture_id, slots[k].role, (const void*)t,
                        t->name, (int)t->op, (int)t->type,
                        (long long)t->ne[0], (long long)t->nb[0], ggml_nbytes(t),
                        ggml_is_contiguous(t) ? 1 : 0, (void*)t->buffer, resident ? 1 : 0,
                        insub, r.weight_name.c_str());"""

A_CNT = "    size_t res_cc = 0, buf_cc = 0, res_mr = 0, buf_mr = 0, res_mc = 0, buf_mc = 0;"
B_CNT = A_CNT + "\n    size_t n_res_insub = 0, n_null_insub = 0;"

A_LOG = '    fprintf(stderr, "[f1a-cls] epoch=%llu submissions=%llu sid_at_build=%llu class=%s\\n",'
B_LOG = ('    fprintf(stderr, "[f1a-prov] RESIDENT_ET_SOUMIS=%zu  BUFFERLESS_ET_SOUMIS=%zu\\n",\n'
         '            n_res_insub, n_null_insub);\n') + A_LOG


def main():
    s = open(C, encoding="utf-8").read()
    if "in_submitted_graph" in s:
        print("deja applique"); return
    for anchor, name in ((A_HDR, "en-tete manifeste"), (A_ROW, "ligne manifeste"),
                         (A_CNT, "compteurs"), (A_LOG, "log")):
        n = s.count(anchor)
        if n != 1:
            raise SystemExit("ANCRE '%s' : %d occurrence(s), attendu 1 -- rien n'est ecrit" % (name, n))
    s = s.replace(A_HDR, B_HDR, 1)
    s = s.replace(A_ROW, B_ROW, 1)
    s = s.replace(A_CNT, B_CNT, 1)
    s = s.replace(A_LOG, B_LOG, 1)
    open(C, "w", encoding="utf-8").write(s)
    print("appartenance au graphe soumis integree")


if __name__ == "__main__":
    main()
