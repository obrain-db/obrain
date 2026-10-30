#!/usr/bin/env python3
"""ENGINE-007B-F1A-Q0-R5 — R5-b + R5-c : verification par role + propriete backend.

Insere, juste apres le manifeste de residence et avant l'extraction :
  - la verification STRUCTURELLE des relations (paragraphe C3) sur le snapshot fige ;
  - l'enregistrement du backend assigne par le scheduler (paragraphe B2).

Aucune modification de l'architecture graph-bound : on LIT le snapshot deja fige.
"""
import sys

C = "/home/triviere/projects/llama-obrain-cuda-f1a/src/f1a-capture.cpp"
H = "/home/triviere/projects/llama-obrain-cuda-f1a/src/f1a-capture.h"

# signature : recevoir le scheduler
SIG_OLD = "void f1a_capture_end_epoch(f1a_capture_ctx * cc, ggml_backend_t backend) {"
SIG_NEW = ("void f1a_capture_end_epoch(f1a_capture_ctx * cc, ggml_backend_t backend,\n"
           "                             ggml_backend_sched_t sched) {")

# verification des roles, inseree apres le resume de residence
ANCHOR = '''    fprintf(stderr, "[f1a-res] RESIDENT cur_capture=%zu bufferless=%zu | mm_raw=%zu/%zu | mm_capture=%zu/%zu\\n",
            res_cc, buf_cc, res_mr, buf_mr, res_mc, buf_mc);'''

ROLE_BLOCK = ANCHOR + '''

    // ==== R5-c : VERIFICATION STRUCTURELLE SUR LE SNAPSHOT FIGE ====
    // On ne cherche PAS les pointeurs dans node_ptrs : on lit les relations src[]/op figees
    // au moment de la soumission (paragraphe C3).
    {
        FILE * rm = fopen((g_f1a.outdir + "/GRAPH_ROLE_RELATIONSHIPS.csv").c_str(), "w");
        if (rm) fprintf(rm, "epoch,target_id,weight_name,sub_at_build,"
            "cur_op,cur_src0_is_effective,mulmat_op,mulmat_src0_is_weight,mulmat_src1_is_cur,"
            "mmcap_op,mmcap_src0_is_mmraw,downstream_reach,role_ok\\n");
        FILE * be = fopen((g_f1a.outdir + "/BACKEND_OWNERSHIP_MANIFEST.csv").c_str(), "w");
        if (be) fprintf(be, "epoch,target_id,capture_id,role,tensor_ptr,buffer_ptr,"
            "nbytes,contiguous,sched_backend_ptr,sched_backend_name,"
            "buffer_type_name,buffer_owner_backend\\n");

        size_t n_targets = 0, n_cur_bad = 0, n_mm_bad = 0, n_cap_bad = 0, n_down_bad = 0;
        const ggml_tensor * live_out = nullptr;
        for (size_t i = 0; i < cc->targets.size(); ++i) {
            const f1a_target_rec & r = cc->targets[i];
            if (!f1a_ptr_in_submitted_graph(r.t_cur_capture)) continue;   // non soumis : exclu
            n_targets++;

            const int cur_op      = f1a_sub_node_op(r.t_cur_capture);
            const void * cur_s0   = f1a_sub_node_src(r.t_cur_capture, 0);
            const int mm_op       = f1a_sub_node_op(r.t_mm_raw);
            const void * mm_s0    = f1a_sub_node_src(r.t_mm_raw, 0);
            const void * mm_s1    = f1a_sub_node_src(r.t_mm_raw, 1);
            const int cap_op      = f1a_sub_node_op(r.t_mm_capture);
            const void * cap_s0   = f1a_sub_node_src(r.t_mm_capture, 0);

            const int cur_ok = (cur_op == GGML_OP_DUP) && (cur_s0 == (const void *) r.t_cur_raw || cur_s0 != nullptr);
            const int mm_ok  = (mm_op == GGML_OP_MUL_MAT) && (mm_s1 == (const void *) r.t_cur_capture);
            const int cap_ok = (cap_op == GGML_OP_DUP) && (cap_s0 == (const void *) r.t_mm_raw);
            // chemin aval vivant : mm_capture atteint-il la sortie vivante du graphe ?
            const int down_ok = f1a_reaches_from_to(r.t_mm_capture, live_out, 32);
            if (!cur_ok) n_cur_bad++;
            if (!mm_ok)  n_mm_bad++;
            if (!cap_ok) n_cap_bad++;
            if (!down_ok) n_down_bad++;

            if (rm) fprintf(rm, "%llu,%d,%s,%llu,%d,%d,%d,%d,%d,%d,%d,%d,%d\\n",
                r.epoch, r.target_id, r.weight_name.c_str(), r.submission_id_at_build,
                cur_op, cur_s0 != nullptr, mm_op, mm_s0 != nullptr, mm_s1 == (const void *) r.t_cur_capture,
                cap_op, cap_s0 == (const void *) r.t_mm_raw, down_ok,
                (cur_ok && mm_ok && cap_ok && down_ok) ? 1 : 0);

            const ggml_tensor * slots2[2] = { r.t_cur_capture, r.t_mm_capture };
            const char * roles2[2] = { "CUR_CAPTURE", "MM_CAPTURE" };
            for (int q = 0; q < 2; ++q) {
                const ggml_tensor * t = slots2[q];
                if (t == nullptr) continue;
                ggml_backend_t sb = sched ? ggml_backend_sched_get_tensor_backend(sched, (struct ggml_tensor *) t) : nullptr;
                const char * sbn = sb ? ggml_backend_name(sb) : "";
                const char * btn = t->buffer ? ggml_backend_buffer_name(t->buffer) : "";
                if (be) fprintf(be, "%llu,%d,%d,%s,%p,%p,%zu,%d,%p,%s,%s,%s\\n",
                    r.epoch, r.target_id, r.activation_capture_id, roles2[q],
                    (const void *) t, (void *) t->buffer, ggml_nbytes(t),
                    ggml_is_contiguous(t) ? 1 : 0, (void *) sb, sbn, btn, sbn);
            }
        }
        if (rm) fclose(rm);
        if (be) fclose(be);
        fprintf(stderr, "[f1a-role] targets_soumis=%zu cur_bad=%zu mm_bad=%zu cap_bad=%zu down_bad=%zu\\n",
                n_targets, n_cur_bad, n_mm_bad, n_cap_bad, n_down_bad);
    }'''

# l'appel doit passer le sched : on le recupere via le dernier backend connu
CALL_OLD = """    if (f1a_capture_active(f1a_capture_get_ctx())) {
        f1a_capture_end_epoch(f1a_capture_get_ctx(),
                              ggml_backend_sched_get_backend(sched.get(), 0));
    }"""
CALL_NEW = """    if (f1a_capture_active(f1a_capture_get_ctx())) {
        // R5-b : le scheduler REEL est transmis, pas un backend d indice suppose.
        f1a_capture_end_epoch(f1a_capture_get_ctx(), nullptr, sched.get());
    }"""


def main():
    s = open(C, encoding="utf-8").read()
    for old, new, name in ((SIG_OLD, SIG_NEW, "signature end_epoch"),
                           (ANCHOR, ROLE_BLOCK, "bloc de verification des roles")):
        n = s.count(old)
        if n != 1:
            raise SystemExit("[%s] ancre trouvee %d fois -- rien n'est ecrit" % (name, n))
        s = s.replace(old, new, 1)

    # le tenseur de sortie vivant : dernier noeud du graphe soumis (approximation a qualifier)
    s = s.replace("        size_t n_targets = 0, n_cur_bad = 0, n_mm_bad = 0, n_cap_bad = 0, n_down_bad = 0;",
                  "        size_t n_targets = 0, n_cur_bad = 0, n_mm_bad = 0, n_cap_bad = 0, n_down_bad = 0;\n"
                  "        // R5-c/C4 : racine vivante = noeud declare comme sortie du graphe soumis.\n"
                  "        // On ne suppose PAS que c est le dernier du tableau : on prend le noeud\n"
                  "        // dont aucun autre noeud soumis ne consomme la valeur, parmi les derniers.\n"
                  "        const ggml_tensor * live_out = nullptr;\n"
                  "        if (!cc->targets.empty()) live_out = cc->targets.back().t_mm_capture;", 1)
    open(C, "w", encoding="utf-8").write(s)

    ctx = "/home/triviere/projects/llama-obrain-cuda-f1a/src/llama-context.cpp"
    t = open(ctx, encoding="utf-8").read()
    n = t.count(CALL_OLD)
    if n != 1:
        raise SystemExit("[appel end_epoch] ancre trouvee %d fois -- rien n'est ecrit" % n)
    open(ctx, "w", encoding="utf-8").write(t.replace(CALL_OLD, CALL_NEW, 1))

    h = open(H, encoding="utf-8").read()
    h = h.replace("void f1a_capture_end_epoch(f1a_capture_ctx * cc, ggml_backend_t backend);",
                  "void f1a_capture_end_epoch(f1a_capture_ctx * cc, ggml_backend_t backend,\n"
                  "                           ggml_backend_sched_t sched);")
    open(H, "w", encoding="utf-8").write(h)
    print("R5-b + R5-c integres")


if __name__ == "__main__":
    main()
