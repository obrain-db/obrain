#!/usr/bin/env python3
"""RUN2F-A — ecrit le diff candidat M1-M3 SANS toucher au fork.

Copie les 4 fichiers dans /tmp/q0fwd-cand/, les edite, puis produit un
diff unifie via difflib. Le fork n est jamais modifie.
"""
import difflib, hashlib, os, shutil, sys

W = "/home/triviere/projects/llama-obrain-cuda-f1a"
TMP = "/tmp/q0fwd-cand"
OUT = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
       "qualification/e7b-f1a-q0-forward-v1/run2/RUN2F_A_INSTRUMENTATION_PATCH_CANDIDATE.diff")

FILES = ["src/f1a-capture.h", "src/f1a-capture.cpp", "tools/f1a-eval.cpp",
         "src/llama-context.cpp"]

shutil.rmtree(TMP, ignore_errors=True)
os.makedirs(TMP)
for f in FILES:
    d = os.path.join(TMP, os.path.dirname(f))
    os.makedirs(d, exist_ok=True)
    shutil.copy2(os.path.join(W, f), os.path.join(TMP, f))
print("copies faites dans %s" % TMP)


def edit(rel, old, new, label, count=1):
    p = os.path.join(TMP, rel)
    s = open(p, encoding="utf-8").read()
    n = s.count(old)
    if n != count:
        print("  ANCRE KO %-40s %d occurrences (attendu %d)" % (label, n, count))
        sys.exit(1)
    open(p, "w", encoding="utf-8").write(s.replace(old, new, count))
    print("  ok  %s" % label)


# ============ M1a : champ latched dans la cible ============
edit("src/f1a-capture.h",
"""    // capture d'activation partagee (deduplication)
    int                activation_capture_id;
};""",
"""    // capture d'activation partagee (deduplication)
    int                activation_capture_id;
    // RUN2F-A / M1 : pas scientifique LATCHE au moment de f1a_capture_note.
    // -1  = hors pas scientifique (prefill, ou non renseigne)
    // 0..8 = pas scientifique Q0FWD. C est la SEULE signification de -1.
    // Cette copie est faite pendant que la cible appartient sans ambiguite au
    // pas s. Elle ne doit JAMAIS etre remplacee par une lecture du contexte
    // courant dans f1a_capture_end_epoch.
    int32_t            decode_step_latched;
};""",
"M1a champ decode_step_latched")

# ============ M1b/M3 : API ============
edit("src/f1a-capture.h",
"void f1a_mark_output(struct ggml_tensor * t);",
"""void f1a_mark_output(struct ggml_tensor * t);

// RUN2F-A / M1 : contexte de pas scientifique.
//   -1  = hors pas scientifique (prefill, ou entre deux pas)
//   0..8 = pas scientifique Q0FWD, produisant les logits de OUTPUT_IDS[s]
void f1a_set_decode_step(int32_t step);
int32_t f1a_get_decode_step(void);

// RUN2F-A / M2 : identite de la derniere soumission autoritaire.
// Mis a jour UNIQUEMENT dans f1a_mark_compute_submission, apres l increment
// de f1a_ctr_submission. Le compteur f1a_ctr_submission reste la source
// autoritaire : on ne cree aucun second compteur.
// Sous le threading audite (f1a-eval mono-thread, end_epoch apres sync), la
// derniere soumission enregistree EST celle qui vient de se terminer.
unsigned long long f1a_last_submission_id(void);
int32_t            f1a_last_submission_decode_step(void);

// RUN2F-A / M3 : ledger evenementiel a ecriture immediate.
// f1a_event_current  : lit le contexte courant (boucle scientifique).
// f1a_event_latched  : NE LIT AUCUN contexte mutable ; l identite temporelle
//                      est fournie explicitement. Obligatoire pour CAPTURE_*,
//                      qui surviennent a end_epoch, hors du pas courant.
void f1a_event_current(const char * kind);
void f1a_event_latched(const char * kind,
                       int32_t decode_step,
                       unsigned long long compute_submission_id,
                       unsigned long long capture_epoch_id,
                       int eligible_target_count,
                       int captured_target_count);
void f1a_event_close(void);""",
"M1b/M2/M3 declarations")

# ============ implementations ============
edit("src/f1a-capture.cpp",
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

// ===================== RUN2F-A : instrumentation temporelle =====================
//
// THREADING (audite) : f1a_set_decode_step est appele depuis la boucle
// f1a-eval ; f1a_get_decode_step est lu depuis f1a_capture_note, atteint par
// llama-graph.cpp pendant la construction du graphe du MEME pas, sur le meme
// thread de controle. Un scalaire statique suffit donc.
// ATTENTION : il ne doit JAMAIS servir a attribuer une capture dans
// f1a_capture_end_epoch, ou le pas courant a deja avance. L attribution passe
// par f1a_target_rec::decode_step_latched.

static int32_t g_f1a_decode_step      = -1;
static unsigned long long g_f1a_last_sub_id   = 0;
static unsigned long long g_f1a_last_sub_step = 0;   // 0 = jamais renseigne
static unsigned long long g_f1a_last_sub_fp   = 0;   // fingerprint du graphe soumis

void f1a_set_decode_step(int32_t step) {
    g_f1a_decode_step = step;
}

int32_t f1a_get_decode_step(void) {
    return g_f1a_decode_step;
}

unsigned long long f1a_last_submission_id(void) {
    return g_f1a_last_sub_id;
}

int32_t f1a_last_submission_decode_step(void) {
    return (int32_t) g_f1a_last_sub_step;
}

// ---------- M3 : ledger evenementiel ----------
static const char * const F1A_EVENT_LEDGER_FILE = "EVENT_LEDGER.csv";
static unsigned long long g_f1a_event_seq      = 0;
static FILE *             g_f1a_ledger        = nullptr;
static bool               g_f1a_ledger_hdr    = false;

static void f1a_event_row(const char * kind,
                          int32_t              decode_step,
                          unsigned long long   compute_submission_id,
                          unsigned long long   capture_epoch_id,
                          int                  eligible_target_count,
                          int                  captured_target_count) {
    if (!g_f1a.enabled) return;
    if (g_f1a_ledger == nullptr) {
        g_f1a_ledger = fopen((g_f1a.outdir + "/" + F1A_EVENT_LEDGER_FILE).c_str(), "a");
        if (g_f1a_ledger == nullptr) return;
    }
    if (!g_f1a_ledger_hdr) {
        fprintf(g_f1a_ledger,
            "run_id,event_sequence_id,event_kind,decode_step,compute_submission_id,"
            "graph_ptr,graph_fingerprint,capture_epoch_id,eligible_target_count,"
            "captured_target_count\\n");
        g_f1a_ledger_hdr = true;
    }
    char dsbuf[16];
    if (decode_step < 0) snprintf(dsbuf, sizeof dsbuf, "NA");
    else                 snprintf(dsbuf, sizeof dsbuf, "%d", decode_step);
    char sidbuf[32];
    if (compute_submission_id == 0) snprintf(sidbuf, sizeof sidbuf, "NA");
    else                            snprintf(sidbuf, sizeof sidbuf, "%llu", compute_submission_id);
    char epbuf[32];
    if (capture_epoch_id == 0) snprintf(epbuf, sizeof epbuf, "NA");
    else                       snprintf(epbuf, sizeof epbuf, "%llu", capture_epoch_id);
    char gpbuf[32];
    if (f1a_last_gf == nullptr) snprintf(gpbuf, sizeof gpbuf, "NA");
    else                        snprintf(gpbuf, sizeof gpbuf, "%p", f1a_last_gf);

    const int et = (eligible_target_count < 0) ? 0 : eligible_target_count;
    const int ct = (captured_target_count < 0) ? 0 : captured_target_count;

    fprintf(g_f1a_ledger, "%s,%llu,%s,%s,%s,%s,%016llx,%s,%d,%d\\n",
            g_f1a.run_id.c_str(), ++g_f1a_event_seq, kind, dsbuf, sidbuf,
            gpbuf, g_f1a_last_sub_fp, epbuf, et, ct);
    fflush(g_f1a_ledger);   // ecriture immediate : survit a un arret brutal
}

void f1a_event_current(const char * kind) {
    f1a_event_row(kind, g_f1a_decode_step, g_f1a_last_sub_id, 0, -1, -1);
}

void f1a_event_latched(const char * kind,
                       int32_t decode_step,
                       unsigned long long compute_submission_id,
                       unsigned long long capture_epoch_id,
                       int eligible_target_count,
                       int captured_target_count) {
    f1a_event_row(kind, decode_step, compute_submission_id, capture_epoch_id,
                  eligible_target_count, captured_target_count);
}

void f1a_event_close(void) {
    if (g_f1a_ledger != nullptr) { fclose(g_f1a_ledger); g_f1a_ledger = nullptr; }
}
// =================== fin RUN2F-A instrumentation ===================""",
"M1/M3 implementations")

# latch dans f1a_capture_note
edit("src/f1a-capture.cpp",
"    r.submission_id_at_build = f1a_ctr_submission;",
"""    // RUN2F-A / M1 : LATCH du pas scientifique. Copie AUTORITAIRE, faite ici
    // pendant que la cible appartient sans ambiguite au pas courant.
    r.decode_step_latched = f1a_get_decode_step();
    // NB : submission_id_at_build est DIAGNOSTIC. Il est echantillonne AVANT la
    // soumission et vaut (id reel - 1). Il ne doit JAMAIS servir de
    // compute_submission_id scientifique.
    r.submission_id_at_build = f1a_ctr_submission;""",
"M1 latch dans capture_note")

# M2 : les deux scalaires au hook autoritaire
edit("src/f1a-capture.cpp",
"""    f1a_ctr_submission++;
    f1a_last_gf = (const void *) gf;""",
"""    f1a_ctr_submission++;
    // RUN2F-A / M2 : identite de la soumission qui vient d etre soumise.
    // Mise a jour AU MEME ENDROIT que l increment autoritaire, jamais ailleurs.
    g_f1a_last_sub_id   = f1a_ctr_submission;
    g_f1a_last_sub_step = (unsigned long long) g_f1a_decode_step;
    f1a_last_gf = (const void *) gf;""",
"M2 deux scalaires + COMPUTE_SUBMIT")
print("etape 1 terminee")
