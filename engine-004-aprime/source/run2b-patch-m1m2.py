#!/usr/bin/env python3
"""RUN2B — patch M1-M3 : contexte de pas, latch a capture_note, ledger runtime."""
import sys

W = "/home/triviere/projects/llama-obrain-cuda-f1a"

H = W + "/src/f1a-capture.h"
C = W + "/src/f1a-capture.cpp"
E = W + "/tools/f1a-eval.cpp"


def patch(path, old, new, label, count=1):
    s = open(path, encoding="utf-8").read()
    n = s.count(old)
    if n != count:
        print("  %-46s ANCRE %d occurrences (attendu %d)" % (label, n, count))
        sys.exit(1)
    open(path, "w", encoding="utf-8").write(s.replace(old, new, count))
    print("  %-46s PATCHED" % label)


print("=== M1 : contexte de pas ===")

# --- 1. struct : latch dans l enregistrement de cible ---
patch(H,
"""    // capture d'activation partagee (deduplication)
    int                activation_capture_id;
};""",
"""    // capture d'activation partagee (deduplication)
    int                activation_capture_id;
    // [RUN2B] M1 : pas scientifique LATCHE au moment de f1a_capture_note.
    // Copie pendant que la cible appartient sans ambiguite au pas s.
    int                decode_step_latched;
};""",
"f1a-capture.h : champ decode_step_latched")

# --- 2. API du contexte de pas ---
patch(H,
"void f1a_mark_output(struct ggml_tensor * t);",
"""void f1a_mark_output(struct ggml_tensor * t);

// [RUN2B] M1 : contexte de pas scientifique.
//   -1 = hors pas scientifique (prefill, ou entre deux pas)
//  0..8 = pas scientifique Q0FWD (produit les logits de OUTPUT_IDS[s])
void f1a_set_decode_step(int32_t step);
int32_t f1a_get_decode_step(void);

// [RUN2B] M3 : ledger evenementiel runtime.
// Ecrit une ligne IMMEDIATEMENT (flush a chaque evenement). event_sequence_id
// est monotone et fait foi comme ordre logique. Les champs non applicables
// valent NA, jamais une valeur inventee.
void f1a_event(const char * kind,
               unsigned long long submission_id,
               const void * graph_ptr,
               unsigned long long graph_fingerprint,
               int eligible_targets,
               int captured_targets);""",
"f1a-capture.h : API M1 + M3")

print("=== M1/M3 : implementations ===")

patch(C,
"""void f1a_mark_output(struct ggml_tensor * t) {
    if (t != nullptr) {
        ggml_set_output(t);
    }
}""",
"""void f1a_mark_output(struct ggml_tensor * t) {
    if (t != nullptr) {
        ggml_set_output(t);
    }
}

// [RUN2B] M1 : contexte de pas scientifique.
// THREADING : la boucle f1a-eval, la construction de graphe (llama-graph.cpp:1534)
// et la soumission (llama-context.cpp:2955) s executent sur le thread de controle.
// f1a_capture_end_epoch (llama-context.cpp:1784) s execute aussi sur ce thread,
// APRES synchronisation. Un global simple suffit DONC ici -- mais il ne doit
// JAMAIS servir a attribuer une capture a end_epoch : le pas courant a alors
// deja avance. L attribution passe par le latch de f1a_capture_note.
static int32_t g_f1a_decode_step = -1;

void f1a_set_decode_step(int32_t step) {
    g_f1a_decode_step = step;
}

int32_t f1a_get_decode_step(void) {
    return g_f1a_decode_step;
}

// [RUN2B] M3 : ledger evenementiel. Ecrit pendant l execution, jamais
// reconstruit apres coup depuis les payloads ou leur ordre d ecriture.
static unsigned long long g_f1a_event_seq = 0;
static FILE *             g_f1a_ledger    = nullptr;
static bool               g_f1a_ledger_hdr = false;

void f1a_event(const char * kind,
               unsigned long long submission_id,
               const void * graph_ptr,
               unsigned long long graph_fingerprint,
               int eligible_targets,
               int captured_targets) {
    if (!g_f1a.enabled) return;
    if (g_f1a_ledger == nullptr) {
        g_f1a_ledger = fopen((g_f1a.outdir + "/" + F1A_EVENT_LEDGER_NAME).c_str(), "a");
        if (g_f1a_ledger == nullptr) return;
    }
    if (!g_f1a_ledger_hdr) {
        fprintf(g_f1a_ledger,
            "run_id,event_sequence_id,event_kind,decode_step,compute_submission_id,"
            "graph_ptr,graph_fingerprint,capture_epoch_id,eligible_target_count,"
            "captured_target_count\\n");
        g_f1a_ledger_hdr = true;
    }
    const int32_t ds = g_f1a_decode_step;
    char dsbuf[16];
    if (ds < 0) snprintf(dsbuf, sizeof dsbuf, "NA");
    else        snprintf(dsbuf, sizeof dsbuf, "%d", ds);
    char sidbuf[32];
    if (submission_id == 0) snprintf(sidbuf, sizeof sidbuf, "NA");
    else                    snprintf(sidbuf, sizeof sidbuf, "%llu", submission_id);
    char gpbuf[32];
    if (graph_ptr == nullptr) snprintf(gpbuf, sizeof gpbuf, "NA");
    else                      snprintf(gpbuf, sizeof gpbuf, "%p", graph_ptr);
    char gfbuf[32];
    if (graph_fingerprint == 0) snprintf(gfbuf, sizeof gfbuf, "NA");
    else                        snprintf(gfbuf, sizeof gfbuf, "%016llx", graph_fingerprint);

    fprintf(g_f1a_ledger, "%s,%llu,%s,%s,%s,%s,%s,%llu,%d,%d\\n",
            g_f1a.run_id.c_str(), ++g_f1a_event_seq, kind, dsbuf, sidbuf, gpbuf, gfbuf,
            (unsigned long long) g_f1a.epoch_cur, eligible_targets, captured_targets);
    fflush(g_f1a_ledger);   // ecriture immediate : survit a un arret brutal
}""",
"f1a-capture.cpp : f1a_set/get_decode_step + f1a_event")

print("=== M1 : latch au moment de f1a_capture_note ===")

patch(C,
"    r.submission_id_at_build = f1a_ctr_submission;",
"""    // [RUN2B] M1 : LATCH du pas scientifique. C est la copie AUTORITAIRE.
    // Lue ici, pendant que la cible appartient sans ambiguite au pas courant.
    r.decode_step_latched = g_f1a_decode_step;
    // [RUN2B] M2 : occurrence de soumission latchee au build (deja existante).
    r.submission_id_at_build = f1a_ctr_submission;""",
"f1a-capture.cpp : latch dans f1a_capture_note")

print("=== M2 : propagation dans le manifeste ===")

patch(C,
'''        fprintf(m, "run_id,epoch,target_id,capture_id,weight_name,family,expected_K,rotation_applied,"
                   "kind,path,dtype,ne0,ne1,ne2,ne3,nb0,nb1,nb2,nb3,logical_elems,bytes,contiguous\\n");''',
'''        fprintf(m, "run_id,epoch,target_id,capture_id,weight_name,family,expected_K,rotation_applied,"
                   "kind,path,dtype,ne0,ne1,ne2,ne3,nb0,nb1,nb2,nb3,logical_elems,bytes,contiguous,"
                   "decode_step,compute_submission_id\\n");''',
"f1a-capture.cpp : header manifeste +2 colonnes")

print()
print("PATCH M1-M2 APPLIQUE. M3 (appels d evenements) reste a cabler.")
print("Fichiers modifies : f1a-capture.h, f1a-capture.cpp")
