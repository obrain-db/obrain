#!/usr/bin/env python3
"""ENGINE-007B-F1A-Q0-R3 — appartenance au graphe soumis, en une seule edition verifiee."""
import sys

C = "/home/triviere/projects/llama-obrain-cuda-f1a/src/f1a-capture.cpp"
s = open(C, encoding="utf-8").read()

edits = [
    # 1. en-tete du manifeste
    ('"buffer_ptr,resident,sched_backend_ptr,sched_backend_name,weight_name\\n");',
     '"buffer_ptr,resident,in_submitted_graph,weight_name\\n");'),
    # 2. compteurs
    ("    size_t res_cc = 0, buf_cc = 0, res_mr = 0, buf_mr = 0, res_mc = 0, buf_mc = 0;",
     "    size_t res_cc = 0, buf_cc = 0, res_mr = 0, buf_mr = 0, res_mc = 0, buf_mc = 0;\n"
     "    size_t n_res_insub = 0, n_null_insub = 0;"),
    # 3. ligne du manifeste (remplacee entierement)
    ("""                fprintf(rman, "%d,%d,%s,%p,%s,%d,%d,%lld,%lld,%zu,%d,%p,%d,%p,%s,%s\\n",
                        r.target_id, r.activation_capture_id, slots[k].role, (const void*)t,
                        t->name, (int)t->op, (int)t->type,
                        (long long)t->ne[0], (long long)t->nb[0], ggml_nbytes(t),
                        ggml_is_contiguous(t) ? 1 : 0, (void*)t->buffer, resident ? 1 : 0,
                        (void*)sb, sbn, r.weight_name.c_str());""",
     """                const int insub = f1a_ptr_in_submitted_graph((const void *) t) ? 1 : 0;
                if (resident && insub) n_res_insub++;
                if (!resident && insub) n_null_insub++;
                fprintf(rman, "%d,%d,%s,%p,%s,%d,%d,%lld,%lld,%zu,%d,%p,%d,%d,%s\\n",
                        r.target_id, r.activation_capture_id, slots[k].role, (const void*)t,
                        t->name, (int)t->op, (int)t->type,
                        (long long)t->ne[0], (long long)t->nb[0], ggml_nbytes(t),
                        ggml_is_contiguous(t) ? 1 : 0, (void*)t->buffer, resident ? 1 : 0,
                        insub, r.weight_name.c_str());"""),
    # 4. log de provenance
    ('    fprintf(stderr, "[f1a-cls] epoch=%llu submissions=%llu sid_at_build=%llu class=%s\\n",',
     '    fprintf(stderr, "[f1a-prov] RESIDENT_ET_SOUMIS=%zu  BUFFERLESS_ET_SOUMIS=%zu\\n",\n'
     '            n_res_insub, n_null_insub);\n'
     '    fprintf(stderr, "[f1a-cls] epoch=%llu submissions=%llu sid_at_build=%llu class=%s\\n",'),
]

for i, (a, b) in enumerate(edits, 1):
    n = s.count(a)
    if n != 1:
        raise SystemExit("EDITION %d : ancre trouvee %d fois, attendu 1 -- rien n'est ecrit" % (i, n))
    s = s.replace(a, b, 1)

open(C, "w", encoding="utf-8").write(s)
print("4 editions appliquees et verifiees")
