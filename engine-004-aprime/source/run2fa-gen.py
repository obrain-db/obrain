#!/usr/bin/env python3
"""RUN2F-A : genere le diff candidat M1-M3 SANS toucher au fork.

Aucune chaine triple-quote : les fragments sont des listes de lignes jointes.
Tous les symboles C++ references ont ete verifies dans la source reelle.
"""
import difflib
import hashlib
import os
import shutil
import sys

W = "/home/triviere/projects/llama-obrain-cuda-f1a"
TMP = "/tmp/q0fwd-cand"
D = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/e7b-f1a-q0-forward-v1/run2")
OUT = D + "/RUN2F_A_INSTRUMENTATION_PATCH_CANDIDATE.diff"

FILES = ["src/f1a-capture.h", "src/f1a-capture.cpp",
         "tools/f1a-eval.cpp", "src/llama-context.cpp"]

shutil.rmtree(TMP, ignore_errors=True)
for f in FILES:
    os.makedirs(os.path.join(TMP, os.path.dirname(f)), exist_ok=True)
    shutil.copy2(os.path.join(W, f), os.path.join(TMP, f))
print("copies dans " + TMP)

J = "\n"


def edit(rel, old_lines, new_lines, label, count=1):
    p = os.path.join(TMP, rel)
    s = open(p, encoding="utf-8").read()
    old = J.join(old_lines)
    new = J.join(new_lines)
    n = s.count(old)
    if n != count:
        print("  ANCRE KO %-46s %d occ (attendu %d)" % (label, n, count))
        sys.exit(1)
    open(p, "w", encoding="utf-8").write(s.replace(old, new, count))
    print("  ok  %s" % label)


# ------------------------------------------------ M1a : champ latched
edit("src/f1a-capture.h",
     ["    // capture d'activation partagee (deduplication)",
      "    int                activation_capture_id;",
      "};"],
     ["    // capture d'activation partagee (deduplication)",
      "    int                activation_capture_id;",
      "    // RUN2F-A / M1 : pas scientifique LATCHE au moment de f1a_capture_note.",
      "    //   -1  = hors pas scientifique (prefill, ou non renseigne)",
      "    //   0..8 = pas scientifique Q0FWD. SEULE signification de -1.",
      "    // Copie faite pendant que la cible appartient sans ambiguite au pas s.",
      "    // Ne JAMAIS la remplacer par une lecture du contexte courant a end_epoch.",
      "    int32_t            decode_step_latched;",
      "};"],
     "M1a champ decode_step_latched")

# ------------------------------------------------ declarations
edit("src/f1a-capture.h",
     ["void f1a_mark_output(struct ggml_tensor * t);"],
     ["void f1a_mark_output(struct ggml_tensor * t);",
      "",
      "// RUN2F-A / M1 : contexte de pas scientifique.",
      "//   -1 = hors pas scientifique (prefill, ou entre deux pas)",
      "//  0..8 = pas scientifique Q0FWD, produisant les logits de OUTPUT_IDS[s]",
      "void f1a_set_decode_step(int32_t step);",
      "int32_t f1a_get_decode_step(void);",
      "",
      "// RUN2F-A / M2 : identite de la derniere soumission autoritaire.",
      "// Mise a jour UNIQUEMENT dans f1a_mark_compute_submission, au meme endroit",
      "// que l increment de f1a_ctr_submission. Aucun second compteur.",
      "unsigned long long f1a_last_submission_id(void);",
      "int32_t            f1a_last_submission_decode_step(void);",
      "",
      "// RUN2F-A / M3 : ledger evenementiel, ecriture immediate.",
      "// f1a_event_current : lit le contexte courant (boucle scientifique).",
      "// f1a_event_latched : NE LIT AUCUN contexte mutable ; identite fournie",
      "//                     explicitement. Obligatoire pour CAPTURE_*, emis a",
      "//                     end_epoch donc HORS du pas courant.",
      "void f1a_event_current(const char * kind);",
      "void f1a_event_latched(const char * kind,",
      "                       int32_t            decode_step,",
      "                       unsigned long long compute_submission_id,",
      "                       unsigned long long capture_epoch_id,",
      "                       int                eligible_target_count,",
      "                       int                captured_target_count);",
      "void f1a_event_close(void);"],
     "declarations M1/M2/M3")

# ------------------------------------------------ implementations
impl = [
    "void f1a_mark_output(struct ggml_tensor * t) {",
    "    if (t != nullptr) {",
    "        ggml_set_output(t);",
    "    }",
    "}",
    "",
    "// ============== RUN2F-A : instrumentation temporelle ==============",
    "// THREADING (audite) : setter appele depuis la boucle f1a-eval ; getter lu",
    "// depuis f1a_capture_note, sur le meme thread de controle. Scalaire suffit.",
    "// Ne JAMAIS lire ce contexte a f1a_capture_end_epoch pour attribuer une",
    "// capture : le pas courant y a deja avance. L attribution passe par",
    "// f1a_target_rec::decode_step_latched.",
    "",
    "static int32_t            g_f1a_decode_step   = -1;",
    "static unsigned long long g_f1a_last_sub_id   = 0;",
    "static unsigned long long g_f1a_last_sub_step = 0;",
    "static unsigned long long g_f1a_last_sub_fp   = 0;",
    "",
    "void f1a_set_decode_step(int32_t step) { g_f1a_decode_step = step; }",
    "int32_t f1a_get_decode_step(void)      { return g_f1a_decode_step; }",
    "unsigned long long f1a_last_submission_id(void) { return g_f1a_last_sub_id; }",
    "int32_t f1a_last_submission_decode_step(void)   { return (int32_t) g_f1a_last_sub_step; }",
    "",
    "// -------------------------- M3 : ledger --------------------------",
    'static const char * const F1A_EVENT_LEDGER_FILE = "EVENT_LEDGER.csv";',
    "static unsigned long long g_f1a_event_seq  = 0;",
    "static FILE *             g_f1a_ledger     = nullptr;",
    "static bool               g_f1a_ledger_hdr = false;",
    "",
    "static void f1a_event_row(const char * kind,",
    "                          int32_t              decode_step,",
    "                          unsigned long long   compute_submission_id,",
    "                          unsigned long long   capture_epoch_id,",
    "                          int                  eligible_target_count,",
    "                          int                  captured_target_count) {",
    "    if (!g_f1a.enabled) return;",
    "    if (g_f1a_ledger == nullptr) {",
    '        g_f1a_ledger = fopen((g_f1a.outdir + "/" + F1A_EVENT_LEDGER_FILE).c_str(), "a");',
    "        if (g_f1a_ledger == nullptr) return;",
    "    }",
    "    if (!g_f1a_ledger_hdr) {",
    "        fprintf(g_f1a_ledger,",
    '            "run_id,event_sequence_id,event_kind,decode_step,compute_submission_id,"',
    '            "graph_ptr,graph_fingerprint,capture_epoch_id,eligible_target_count,"',
    '            "captured_target_count' + '\\n' + '");',
    "        g_f1a_ledger_hdr = true;",
    "    }",
    "    char dsbuf[16];",
    '    if (decode_step < 0) snprintf(dsbuf, sizeof dsbuf, "NA");',
    '    else                 snprintf(dsbuf, sizeof dsbuf, "%d", decode_step);',
    "    char sidbuf[32];",
    '    if (compute_submission_id == 0) snprintf(sidbuf, sizeof sidbuf, "NA");',
    '    else                            snprintf(sidbuf, sizeof sidbuf, "%llu", compute_submission_id);',
    "    char epbuf[32];",
    '    if (capture_epoch_id == 0) snprintf(epbuf, sizeof epbuf, "NA");',
    '    else                       snprintf(epbuf, sizeof epbuf, "%llu", capture_epoch_id);',
    "    char gpbuf[32];",
    '    if (f1a_last_gf == nullptr) snprintf(gpbuf, sizeof gpbuf, "NA");',
    '    else                        snprintf(gpbuf, sizeof gpbuf, "%p", f1a_last_gf);',
    "",
    '    fprintf(g_f1a_ledger, "%s,%llu,%s,%s,%s,%s,%016llx,%s,%d,%d' + '\\n' + '",',
    "            g_f1a.run_id.c_str(), ++g_f1a_event_seq, kind, dsbuf, sidbuf,",
    "            gpbuf, g_f1a_last_sub_fp, epbuf,",
    "            (eligible_target_count < 0) ? 0 : eligible_target_count,",
    "            (captured_target_count < 0) ? 0 : captured_target_count);",
    "    fflush(g_f1a_ledger);",
    "}",
    "",
    "void f1a_event_current(const char * kind) {",
    "    f1a_event_row(kind, g_f1a_decode_step, g_f1a_last_sub_id, 0, -1, -1);",
    "}",
    "",
    "void f1a_event_latched(const char * kind,",
    "                       int32_t decode_step,",
    "                       unsigned long long compute_submission_id,",
    "                       unsigned long long capture_epoch_id,",
    "                       int eligible_target_count,",
    "                       int captured_target_count) {",
    "    f1a_event_row(kind, decode_step, compute_submission_id, capture_epoch_id,",
    "                  eligible_target_count, captured_target_count);",
    "}",
    "",
    "void f1a_event_close(void) {",
    "    if (g_f1a_ledger != nullptr) { fclose(g_f1a_ledger); g_f1a_ledger = nullptr; }",
    "}",
    "// =========== fin RUN2F-A instrumentation ===========",
]
edit("src/f1a-capture.cpp",
     ["void f1a_mark_output(struct ggml_tensor * t) {",
      "    if (t != nullptr) {",
      "        ggml_set_output(t);",
      "    }",
      "}"],
     impl, "M1/M3 implementations")

# ------------------------------------------------ M1 latch
edit("src/f1a-capture.cpp",
     ["    r.submission_id_at_build = f1a_ctr_submission;"],
     ["    // RUN2F-A / M1 : LATCH du pas scientifique. Copie AUTORITAIRE, faite ici",
      "    // pendant que la cible appartient sans ambiguite au pas courant.",
      "    r.decode_step_latched = f1a_get_decode_step();",
      "    // NB : submission_id_at_build reste DIAGNOSTIC. Echantillonne AVANT la",
      "    // soumission, il vaut (id reel - 1). Ne JAMAIS l utiliser comme",
      "    // compute_submission_id scientifique.",
      "    r.submission_id_at_build = f1a_ctr_submission;"],
     "M1 latch capture_note")

# ------------------------------------------------ M2 scalaires
edit("src/f1a-capture.cpp",
     ["    f1a_ctr_submission++;",
      "    f1a_last_gf = (const void *) gf;"],
     ["    f1a_ctr_submission++;",
      "    // RUN2F-A / M2 : identite de la soumission qui vient d etre soumise.",
      "    // Mis a jour AU MEME ENDROIT que l increment autoritaire.",
      "    g_f1a_last_sub_id   = f1a_ctr_submission;",
      "    g_f1a_last_sub_step = (unsigned long long) g_f1a_decode_step;",
      "    f1a_last_gf = (const void *) gf;"],
     "M2 deux scalaires")

edit("src/f1a-capture.cpp",
     ["            f1a_ctr_submission, (void *) gf, f1a_last_gf_nodes, submission_rc, s.fingerprint);"],
     ["            f1a_ctr_submission, (void *) gf, f1a_last_gf_nodes, submission_rc, s.fingerprint);",
      "    // RUN2F-A / M2 : fingerprint de la soumission courante, pour le ledger.",
      "    g_f1a_last_sub_fp = s.fingerprint;",
      "    // RUN2F-A / M3 : COMPUTE_SUBMIT, contexte encore courant ici.",
      '    f1a_event_row("COMPUTE_SUBMIT", g_f1a_decode_step, g_f1a_last_sub_id, 0, -1, -1);'],
     "M2 fingerprint + COMPUTE_SUBMIT")

# ------------------------------------------------ M2 manifeste : header + identite lue une fois
edit("src/f1a-capture.cpp",
     ["    if (!s_manifest_header_done) {",
      '        fprintf(m, "run_id,epoch,target_id,capture_id,weight_name,family,expected_K,rotation_applied,"',
      '                   "kind,path,dtype,ne0,ne1,ne2,ne3,nb0,nb1,nb2,nb3,logical_elems,bytes,contiguous' + '\\n' + '");',
      "        s_manifest_header_done = true;",
      "    }"],
     ["    if (!s_manifest_header_done) {",
      '        fprintf(m, "run_id,epoch,target_id,capture_id,weight_name,family,expected_K,rotation_applied,"',
      '                   "kind,path,dtype,ne0,ne1,ne2,ne3,nb0,nb1,nb2,nb3,logical_elems,bytes,contiguous,"',
      '                   "decode_step,compute_submission_id' + '\\n' + '");',
      "        s_manifest_header_done = true;",
      "    }",
      "",
      "    // RUN2F-A / M2 : identite temporelle de CETTE epoque, lue UNE SEULE FOIS.",
      "    // Apres synchronisation ces valeurs sont figees : on ne relit jamais un",
      "    // compteur mutable ni le contexte de pas courant pour ecrire les lignes.",
      "    const unsigned long long epoch_compute_submission_id  = f1a_last_submission_id();",
      "    const int32_t            epoch_submission_decode_step = f1a_last_submission_decode_step();",
      "    const unsigned long long epoch_capture_id             = cc->epoch;",
      "    // Pas de l epoque, derive des cibles LATCHEES (jamais du contexte courant).",
      "    int32_t epoch_decode_step = -1;",
      "    int     n_distinct_steps  = 0;",
      "    int32_t seen_steps[16];",
      "    for (const auto & t : cc->targets) {",
      "        const int32_t d = t.decode_step_latched;",
      "        bool known = false;",
      "        for (int i = 0; i < n_distinct_steps; ++i) if (seen_steps[i] == d) { known = true; break; }",
      "        if (!known && n_distinct_steps < 16) seen_steps[n_distinct_steps++] = d;",
      "    }",
      "    if (n_distinct_steps == 1) {",
      "        epoch_decode_step = seen_steps[0];",
      "    } else {",
      '        fprintf(stderr, "[f1a] EPOCH_DISTINCT_LATCHED_STEP_COUNT=%d (attendu 1) epoch=%llu' + '\\n' + '",',
      "                n_distinct_steps, (unsigned long long) cc->epoch);",
      "    }",
      "    if (epoch_decode_step >= 0 && epoch_submission_decode_step != epoch_decode_step) {",
      '        fprintf(stderr, "[f1a] TARGET_SUBMISSION_STEP_MISMATCH epoch=%llu target=%d submission=%d' + '\\n' + '",',
      "                (unsigned long long) cc->epoch, epoch_decode_step, epoch_submission_decode_step);",
      "    }",
      '    f1a_event_latched("CAPTURE_BEGIN", epoch_decode_step, epoch_compute_submission_id,',
      "                      epoch_capture_id, (int) cc->targets.size(), -1);"],
     "M2 identite lue une fois + CAPTURE_BEGIN")

# ------------------------------------------------ M2 manifeste : ligne +2 colonnes
edit("src/f1a-capture.cpp",
     ['            fprintf(m, "%s,%llu,%d,%d,%s,%d,%lld,%d,%s,%s,%d,%lld,%lld,%lld,%lld,%lld,%lld,%lld,%lld,%lld,%lld,%d' + '\\n' + '",',
      "                    g_f1a.run_id.c_str(), r.epoch, r.target_id, r.activation_capture_id, r.weight_name.c_str(), r.family,",
      "                    (long long) r.expected_K, r.rotation_applied ? 1 : 0, kinds[k], p,",
      "                    (int) t->type,",
      "                    (long long)t->ne[0],(long long)t->ne[1],(long long)t->ne[2],(long long)t->ne[3],",
      "                    (long long)t->nb[0],(long long)t->nb[1],(long long)t->nb[2],(long long)t->nb[3],",
      "                    (long long) ggml_nelements(t), (long long) ggml_nbytes(t),",
      "                    ggml_is_contiguous(t) ? 1 : 0);"],
     ['            fprintf(m, "%s,%llu,%d,%d,%s,%d,%lld,%d,%s,%s,%d,%lld,%lld,%lld,%lld,%lld,%lld,%lld,%lld,%lld,%lld,%d,%d,%llu' + '\\n' + '",',
      "                    g_f1a.run_id.c_str(), r.epoch, r.target_id, r.activation_capture_id, r.weight_name.c_str(), r.family,",
      "                    (long long) r.expected_K, r.rotation_applied ? 1 : 0, kinds[k], p,",
      "                    (int) t->type,",
      "                    (long long)t->ne[0],(long long)t->ne[1],(long long)t->ne[2],(long long)t->ne[3],",
      "                    (long long)t->nb[0],(long long)t->nb[1],(long long)t->nb[2],(long long)t->nb[3],",
      "                    (long long) ggml_nelements(t), (long long) ggml_nbytes(t),",
      "                    ggml_is_contiguous(t) ? 1 : 0,",
      "                    r.decode_step_latched, epoch_compute_submission_id);"],
     "M2 ligne manifeste +2 colonnes")

# ------------------------------------------------ M2 provenance : header +3
edit("src/f1a-capture.cpp",
     ['                    if (ftell(pv) == 0) fprintf(pv, "epoch,submission_id,target_id,capture_id,"',
      '                        "weight_name,role,dtype,ne0,nb0,contiguous,expected_bytes,written_bytes,"',
      '                        "path,hash_algo,payload_hash' + '\\n' + '");'],
     ["                    // RUN2F-A / M2 : submission_id signifie ici compute_submission_id",
      "                    // (soumission terminee), identite autoritaire deja utilisee par le fork.",
      '                    if (ftell(pv) == 0) fprintf(pv, "epoch,submission_id,target_id,capture_id,"',
      '                        "weight_name,role,dtype,ne0,nb0,contiguous,expected_bytes,written_bytes,"',
      '                        "path,hash_algo,payload_hash,run_id,decode_step,capture_epoch_id' + '\\n' + '");'],
     "M2 header provenance +3 colonnes")

# ------------------------------------------------ M2 provenance : ligne +3
edit("src/f1a-capture.cpp",
     ['                    fprintf(pv, "%llu,%llu,%d,%d,%s,%s,%d,%lld,%lld,%d,%zu,%zu,%s,fnv64x4,%s' + '\\n' + '",',
      "                        r.epoch, f1a_ctr_submission, r.target_id, r.activation_capture_id,",
      "                        r.weight_name.c_str(), kinds[k], (int) t->type,",
      "                        (long long) t->ne[0], (long long) t->nb[0],",
      "                        ggml_is_contiguous(t) ? 1 : 0,",
      "                        (size_t) ggml_nbytes(t), (size_t) ggml_nbytes(t),",
      "                        p, hh.c_str());"],
     ['                    fprintf(pv, "%llu,%llu,%d,%d,%s,%s,%d,%lld,%lld,%d,%zu,%zu,%s,fnv64x4,%s,%s,%d,%llu' + '\\n' + '",',
      "                        r.epoch, epoch_compute_submission_id, r.target_id, r.activation_capture_id,",
      "                        r.weight_name.c_str(), kinds[k], (int) t->type,",
      "                        (long long) t->ne[0], (long long) t->nb[0],",
      "                        ggml_is_contiguous(t) ? 1 : 0,",
      "                        (size_t) ggml_nbytes(t), (size_t) ggml_nbytes(t),",
      "                        p, hh.c_str(),",
      "                        g_f1a.run_id.c_str(), r.decode_step_latched, epoch_capture_id);"],
     "M2 ligne provenance +3 colonnes")

print("--- partie 2 OK : manifeste et provenance ---")

# ------------------------------------------------ M3 : SYNC_COMPLETE apres la vraie sync
edit("src/f1a-capture.cpp",
     ["    // 1. SYNCHRONISATION avant toute lecture : le graphe a ete soumis en asynchrone.",
      "    if (backend != nullptr) ggml_backend_synchronize(backend);"],
     ["    // 1. SYNCHRONISATION avant toute lecture : le graphe a ete soumis en asynchrone.",
      "    if (backend != nullptr) ggml_backend_synchronize(backend);",
      "    // RUN2F-A / M3 : SYNC_COMPLETE. Emis APRES ggml_backend_synchronize, pas",
      "    // apres compute_async (qui ne garantit que l enqueue). Le nom dit exactement",
      "    // ce qui est prouve ici.",
      '    f1a_event_row("SYNC_COMPLETE", g_f1a_decode_step, f1a_last_submission_id(),',
      "                  (unsigned long long) cc->epoch, -1, -1);"],
     "M3 SYNC_COMPLETE apres synchronize")

# ------------------------------------------------ M3 : CAPTURE_EXTRACT / CAPTURE_END en fin d epoque
edit("src/f1a-capture.cpp",
     ["    fclose(f);",
      "}",
      "",
      "unsigned long long f1a_compute_submission_count() { return f1a_ctr_submission; }"],
     ["    fclose(f);",
      "}",
      "",
      "unsigned long long f1a_compute_submission_count() { return f1a_ctr_submission; }"],
     "M3 ancre fin end_epoch (noop de verification)")

print("--- partie 3 OK ---")

# ------------------------------------------------ M3 : CAPTURE_EXTRACT / CAPTURE_END (identite LATCHEE)
edit("src/f1a-capture.cpp",
     ["    fclose(m);",
      "    f1a_dump_submitted_snapshots();",
      '    fprintf(stderr, "[f1a-ctr] ctx_active=%llu eligible_enter=%llu note=%llu extract_call=%llu payload_write=%llu' + '\\n' + '",',
      "            f1a_ctr_ctx_active, f1a_ctr_eligible_enter, f1a_ctr_note, f1a_ctr_extract_call, f1a_ctr_payload_write);",
      '    fprintf(stderr, "[f1a] epoque %llu : %zu cibles, %zu payloads activation, %zu sorties, %zu echecs -> %s' + '\\n' + '",',
      "            cc->epoch, cc->targets.size(), n_act, n_out, n_fail, mpath.c_str());"],
     ["    // RUN2F-A / M3 : CAPTURE_EXTRACT et CAPTURE_END utilisent l API LATCHED.",
      "    // Aucune lecture du contexte de pas courant ici : le harness a deja pu",
      "    // avancer. L identite temporelle vient des variables figees plus haut.",
      '    f1a_event_latched("CAPTURE_EXTRACT", epoch_decode_step, epoch_compute_submission_id,',
      "                      epoch_capture_id, (int) cc->targets.size(), (int) (n_act + n_out));",
      "    fclose(m);",
      "    f1a_dump_submitted_snapshots();",
      '    fprintf(stderr, "[f1a-ctr] ctx_active=%llu eligible_enter=%llu note=%llu extract_call=%llu payload_write=%llu' + '\\n' + '",',
      "            f1a_ctr_ctx_active, f1a_ctr_eligible_enter, f1a_ctr_note, f1a_ctr_extract_call, f1a_ctr_payload_write);",
      '    fprintf(stderr, "[f1a] epoque %llu : %zu cibles, %zu payloads activation, %zu sorties, %zu echecs -> %s' + '\\n' + '",',
      "            cc->epoch, cc->targets.size(), n_act, n_out, n_fail, mpath.c_str());",
      '    f1a_event_latched("CAPTURE_END", epoch_decode_step, epoch_compute_submission_id,',
      "                      epoch_capture_id, (int) cc->targets.size(), (int) (n_act + n_out));"],
     "M3 CAPTURE_EXTRACT + CAPTURE_END latched")

# ------------------------------------------------ M1/M3 : f1a-eval
edit("tools/f1a-eval.cpp",
     ["    // --- pas 0 : PREFILL du premier token ---",
      "    // On evalue ensuite UN SEUL token par appel : c est le regime de decodage que 007A mesure.",
      "    {",
      "        llama_batch b = llama_batch_get_one(&toks[0], 1);",
      '        if (llama_decode(ctx, b) != 0) { fprintf(stderr, "ECHEC decode prefill' + '\\n' + '"); return 1; }',
      "    }",
      "    // --- pas suivants : teacher forcing, un token a la fois ---",
      "    for (int32_t s = 0; s < n_steps; ++s) {",
      "        // logits du pas courant",
      "        float * l = llama_get_logits_ith(ctx, -1);",
      '        if (!l) { fprintf(stderr, "logits indisponibles au pas %d' + '\\n' + '", s); return 1; }',
      "        memcpy(logits.data(), l, (size_t) n_vocab * sizeof(float));",
      "        fwrite(logits.data(), sizeof(float), (size_t) n_vocab, fo);",
      "",
      "        if (s + 1 < (int32_t) toks.size()) {",
      "            llama_batch b = llama_batch_get_one(&toks[s + 1], 1);",
      '            if (llama_decode(ctx, b) != 0) { fprintf(stderr, "ECHEC decode pas %d' + '\\n' + '", s); return 1; }',
      "        }",
      '        if ((s % 4) == 0) fprintf(stderr, "[f1a-eval] pas %d/%d' + '\\n' + '", s, n_steps);',
      "    }",
      "    fclose(fo);"],
     ["    // --- pas 0 : PREFILL du premier token ---",
      "    // On evalue ensuite UN SEUL token par appel : c est le regime de decodage que 007A mesure.",
      "    // RUN2F-A / M1 : le prefill est HORS pas scientifique (decode_step = -1).",
      "    f1a_set_decode_step(-1);",
      "    {",
      "        llama_batch b = llama_batch_get_one(&toks[0], 1);",
      '        if (llama_decode(ctx, b) != 0) { fprintf(stderr, "ECHEC decode prefill' + '\\n' + '"); return 1; }',
      "    }",
      "    // --- pas suivants : teacher forcing, un token a la fois ---",
      "    for (int32_t s = 0; s < n_steps; ++s) {",
      "        // RUN2F-A / M1 : pas scientifique s. C est s qui produit les logits",
      "        // de OUTPUT_IDS[s], PAS un compteur d appels a llama_decode.",
      "        f1a_set_decode_step(s);",
      '        f1a_event_current("DECODE_BEGIN");',
      "        // logits du pas courant",
      "        float * l = llama_get_logits_ith(ctx, -1);",
      '        if (!l) { fprintf(stderr, "logits indisponibles au pas %d' + '\\n' + '", s); return 1; }',
      "        memcpy(logits.data(), l, (size_t) n_vocab * sizeof(float));",
      "        fwrite(logits.data(), sizeof(float), (size_t) n_vocab, fo);",
      "        // RUN2F-A / M3 : les logits du pas s viennent d etre lus.",
      '        f1a_event_current("LOGITS_READ");',
      "",
      "        if (s + 1 < (int32_t) toks.size()) {",
      "            llama_batch b = llama_batch_get_one(&toks[s + 1], 1);",
      '            if (llama_decode(ctx, b) != 0) { fprintf(stderr, "ECHEC decode pas %d' + '\\n' + '", s); return 1; }',
      "        }",
      '        if ((s % 4) == 0) fprintf(stderr, "[f1a-eval] pas %d/%d' + '\\n' + '", s, n_steps);',
      '        f1a_event_current("DECODE_END");',
      "    }",
      "    // RUN2F-A / M1 : hors pas scientifique apres la boucle.",
      "    f1a_set_decode_step(-1);",
      "    f1a_event_close();",
      "    fclose(fo);"],
     "M1/M3 f1a-eval : setter + DECODE_BEGIN/LOGITS_READ/DECODE_END")

# ------------------------------------------------ generation du diff
diff_lines = []
for f in FILES:
    a = open(os.path.join(W, f), encoding="utf-8").read().splitlines(keepends=True)
    b = open(os.path.join(TMP, f), encoding="utf-8").read().splitlines(keepends=True)
    d = list(difflib.unified_diff(a, b, fromfile="a/" + f, tofile="b/" + f, n=3))
    if d:
        diff_lines.extend(d)
        print("  diff %-28s %d lignes" % (f, len(d)))

os.makedirs(D, exist_ok=True)
txt = "".join(diff_lines)
# difflib n ajoute pas les en-tetes git ; on les garde tels quels (git apply les accepte)
open(OUT, "w", encoding="utf-8", newline="\n").write(txt)
h = hashlib.sha256(txt.encode()).hexdigest()
open(D + "/RUN2F_A_PATCH_CANDIDATE.sha256", "w").write(h + "  RUN2F_A_INSTRUMENTATION_PATCH_CANDIDATE.diff\n")

added = sum(1 for l in diff_lines if l.startswith("+") and not l.startswith("+++"))
removed = sum(1 for l in diff_lines if l.startswith("-") and not l.startswith("---"))
hunks = sum(1 for l in diff_lines if l.startswith("@@"))
touched = sorted({l.split()[1][2:] for l in diff_lines if l.startswith("+++ ")})

print()
print("CANDIDATE_PATCH_PRODUCED=YES")
print("RUN2F_A_PATCH_CANDIDATE_SHA256=" + h)
print("CANDIDATE_PATCH_FILE_COUNT=%d" % len(touched))
print("CANDIDATE_PATCH_HUNK_COUNT=%d" % hunks)
print("CANDIDATE_PATCH_ADDED_LINES=%d" % added)
print("CANDIDATE_PATCH_REMOVED_LINES=%d" % removed)
print("files: " + ", ".join(touched))

summary = [
    "RUN2F_A_CANDIDATE_CONTENT_SUMMARY",
    "",
    "files touched : " + ", ".join(touched),
    "hunks count : %d" % hunks,
    "added lines : %d" % added,
    "removed lines : %d" % removed,
    "",
    "M1 target latch present : YES (f1a-capture.h champ + f1a-capture.cpp latch)",
    "M1 setter/getter present : YES (f1a_set_decode_step / f1a_get_decode_step)",
    "M2 existing submission counter reused : YES (f1a_ctr_submission, aucun 2e compteur)",
    "M2 two-scalar storage present : YES (g_f1a_last_sub_id / g_f1a_last_sub_step)",
    "M3 event APIs present : YES (f1a_event_current / f1a_event_latched)",
    "M3 ledger writer present : YES (f1a_event_row + F1A_EVENT_LEDGER_FILE)",
    "manifest writer modified : YES (header l.451 + ligne d ecriture, +2 colonnes)",
    "payload provenance writer modified : YES (header + ligne, +3 colonnes)",
    "placeholder count : 0",
    "product compute files touched : 0 (mmvq.cu / vecdotq.cuh / quantize.cu intacts)",
    "",
    "STATIC_MECHANICAL_AUDIT_STATUS=NOT_EXECUTED (appartient a RUN2F-B)",
    "PATCH_APPLICATION_ALLOWED=NO (aucun audit mecanique dans cette passe)",
]
open(D + "/RUN2F_A_CANDIDATE_CONTENT_SUMMARY.txt", "w", encoding="utf-8", newline="\n").write("\n".join(summary) + "\n")
print("resume ecrit")
