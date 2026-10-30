// ENGINE-004 / RUN-003 — BASELINE sur le runtime GELÉ engine003 V3.
//
// Mode : NATURAL_LONG_RUN_STATIONARY. Une transition de capacité KV naturelle est ATTENDUE
// dans la fenêtre et ne l'invalide pas.
//
// Runtime : libllama 3db83675… / libggml-cuda abafbf0d… (gelées, AUCUN observateur).
// Le logging texte par token de la bibliothèque est coupé SANS repatcher les DSO, via le
// callback de log public (llama_log_set) : le filtre vit dans le harnais, pas dans le runtime.
//
// Frontières temporelles par token (§12) :
//   t0 avant llama_decode ; t1 juste après llama_decode ; t2 juste après llama_get_logits_ith ;
//   t3 juste après argmax.
//   HOST_DECODE_SUBMISSION_MS = t1-t0 ; BACKEND_SYNC_WAIT_MS = t2-t1 ;
//   DECODE_PLUS_SYNC_MS = t2-t0 ; ARGMAX_MS = t3-t2 ; GREEDY_TOKEN_MS = t3-t0.
//
// Compteurs : télémétrie engine-003 V3 (elles ont autorité), lues UNE fois avant et UNE fois
// après la fenêtre — aucune lecture par token.

#include "llama.h"
#include <openssl/sha.h>

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cstdint>
#include <cstdarg>
#include <cmath>
#include <algorithm>
#include <string>
#include <vector>
#include <fstream>
#include <time.h>

extern "C" {
void engine002_v3_cuda_telemetry_get(double *);
void engine002_v3_cuda_telemetry_reset(void);
void engine002_v3_decode_telemetry_get(double *);
void engine002_v3_decode_telemetry_reset(void);
}

// indices CUDA V3 : 0 instance_null_true, 1 pc, 2 urT, 3 urF, 4 cb, 5 ce, 6 inst, 7 upd, 8 lch, 9 nol
enum { CU_NULL = 0, CU_PC = 1, CU_URT = 2, CU_URF = 3, CU_CB = 4, CU_CE = 5, CU_INST = 6, CU_UPD = 7, CU_LCH = 8, CU_NOL = 9 };
enum { DEC_URT = 2, DEC_GB = 4, DEC_SI = 8, DEC_GC = 10 };

static const long LF     = 248320;
static const int  MEAS   = 256;
static const int  MAXPRE = 64;

static double ms() {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (double) t.tv_sec * 1000.0 + (double) t.tv_nsec / 1e6;
}

static void die(const char * m) { fprintf(stderr, "RUN003 FAIL %s\n", m); exit(2); }

static std::string hexb(const unsigned char * h, int n) {
    static const char * x = "0123456789abcdef";
    std::string s;
    for (int i = 0; i < n; ++i) { s += x[h[i] >> 4]; s += x[h[i] & 15]; }
    return s;
}

// ------------------------------------------------------------------ filtre de log (harnais)
// PER_TOKEN_LIBRARY_TEXT_LOGGING=0 sans toucher aux DSO : on garde WARN/ERROR, on jette DEBUG.
static long g_log_debug_dropped = 0;
static long g_log_kept          = 0;

static void log_filter(enum ggml_log_level level, const char * text, void * /*user_data*/) {
    if (level == GGML_LOG_LEVEL_DEBUG || level == GGML_LOG_LEVEL_CONT) {
        g_log_debug_dropped++;
        return;
    }
    g_log_kept++;
    fputs(text, stderr);
}

static void stats(const char * name, const std::vector<double> & v) {
    std::vector<double> s = v;
    std::sort(s.begin(), s.end());
    double mn = 0, sd = 0;
    for (double q : v) mn += q;
    mn /= (double) v.size();
    for (double q : v) sd += (q - mn) * (q - mn);
    sd = std::sqrt(sd / (double) v.size());
    printf("%s p50=%.4f p90=%.4f p95=%.4f p99=%.4f mean=%.4f sd=%.4f min=%.4f max=%.4f\n",
           name, s[s.size() / 2], s[(size_t) (0.90 * (s.size() - 1))], s[(size_t) (0.95 * (s.size() - 1))],
           s[(size_t) (0.99 * (s.size() - 1))], mn, sd, s[0], s[s.size() - 1]);
}

int main(int argc, char ** argv) {
    if (argc < 4) die("usage: run003 <model.gguf> <prefix805.ids.txt> <outdir>");
    const std::string model = argv[1], pfx = argv[2], out = argv[3];

    std::vector<int32_t> ids;
    { std::ifstream f(pfx); long t; while (f >> t) ids.push_back((int32_t) t); }
    if (ids.size() != 805) die("prefix must contain 805 ids");

    // filtre de log AVANT toute initialisation
    llama_log_set(log_filter, nullptr);
    ggml_log_set(log_filter, nullptr);

    printf("RUN003_MODEL=%s\nRUN003_PREFIX_IDS=%zu\nRUN003_MEASURED_TOKEN_COUNT=%d\n",
           model.c_str(), ids.size(), MEAS);

    llama_backend_init();
    llama_model_params mp = llama_model_default_params();
    llama_model * m = llama_model_load_from_file(model.c_str(), mp);
    if (!m) die("model load");

    llama_context_params cp = llama_context_default_params();
    cp.n_ctx = 4096; cp.n_batch = 256; cp.n_ubatch = 256; cp.n_seq_max = 2;
    llama_context * x = llama_init_from_model(m, cp);
    if (!x) die("context");

    llama_batch b = llama_batch_init(256, 0, 2);
    { static const int S[4] = { 256, 256, 245, 48 };
      size_t off = 0;
      for (int si = 0; si < 4; ++si) {
          const int n = S[si];
          for (int i = 0; i < n; ++i) {
              b.token[i] = ids[off + i]; b.pos[i] = (llama_pos) (off + i);
              b.n_seq_id[i] = 1; b.seq_id[i][0] = 0; b.logits[i] = 0;
          }
          b.n_tokens = n;
          if (llama_decode(x, b) != 0) die("prefix decode");
          off += (size_t) n;
      } }

    int       cur = ids[ids.size() - 1];
    llama_pos pos = 805;
    double    tt[4] = { 0, 0, 0, 0 };

    auto step = [&](int * nt) {
        const double t0 = ms();
        b.token[0] = cur; b.pos[0] = pos; b.n_seq_id[0] = 1; b.seq_id[0][0] = 0;
        b.logits[0] = 1; b.n_tokens = 1;
        if (llama_decode(x, b) != 0) die("decode");
        const double t1 = ms();
        const float * lg = llama_get_logits_ith(x, 0);
        const double t2 = ms();
        long am = 0;
        for (long i = 1; i < LF; ++i) if (lg[i] > lg[am]) am = i;
        const double t3 = ms();
        tt[0] = t0; tt[1] = t1; tt[2] = t2; tt[3] = t3;
        *nt = (int) am;
    };

    // ------------------------------------------------ pré-roll adaptatif (même critère que A/B/C/A')
    engine002_v3_cuda_telemetry_reset(); engine002_v3_decode_telemetry_reset();
    double C[10];
    int preroll = 0, consec = 0;
    while (preroll < MAXPRE) {
        int nt = 0; step(&nt); cur = nt; pos++; preroll++;
        engine002_v3_cuda_telemetry_get(C);
        if (C[CU_NOL] >= 1.0) { consec++; if (consec >= 2) break; } else consec = 0;
    }
    engine002_v3_cuda_telemetry_get(C);
    printf("RUN003_ADAPTIVE_PREROLL=%d\n", preroll);
    printf("PREROLL_CUDA[cb=%.0f ce=%.0f inst=%.0f upd=%.0f pc=%.0f urT=%.0f urF=%.0f lch=%.0f nol=%.0f]\n",
           C[CU_CB], C[CU_CE], C[CU_INST], C[CU_UPD], C[CU_PC], C[CU_URT], C[CU_URF], C[CU_LCH], C[CU_NOL]);
    if (!(C[CU_CB] >= 1.0 && C[CU_CE] >= 1.0 && C[CU_CB] == C[CU_CE])) die("warmup capture invariant");
    if (C[CU_NOL] < 1.0) die("warmup regime not reached");

    // ------------------------------------------------ buffers avant la fenêtre
    std::vector<double>  tb(MEAS), td(MEAS), tg(MEAS), ta(MEAS);
    std::vector<int32_t> toks(MEAS);

    engine002_v3_cuda_telemetry_reset(); engine002_v3_decode_telemetry_reset();
    double C0[10]; engine002_v3_cuda_telemetry_get(C0);

    // ------------------------------------------------ FENÊTRE (aucune I/O)
    const double w0 = ms();
    for (int i = 0; i < MEAS; ++i) {
        int nt = 0;
        step(&nt);
        toks[i] = nt; tb[i] = tt[0]; td[i] = tt[1]; tg[i] = tt[2]; ta[i] = tt[3];
        cur = nt; pos++;
    }
    const double w1 = ms();

    double C1[10], D1[16];
    engine002_v3_cuda_telemetry_get(C1);
    engine002_v3_decode_telemetry_get(D1);

    // ------------------------------------------------ restitution
    unsigned char h[32];
    SHA256((const uint8_t *) toks.data(), toks.size() * sizeof(int32_t), h);
    const std::string sha = hexb(h, 32);

    printf("RUN003_OUTPUT_TOKEN_SEQUENCE_SHA256=%s\n", sha.c_str());
    printf("RUN003_GREEDY_CONTINUOUS_WINDOW_MS=%.4f\n", w1 - w0);
    printf("LOG_FILTER_DEBUG_DROPPED=%ld\nLOG_FILTER_KEPT=%ld\n", g_log_debug_dropped, g_log_kept);

    printf("PROPERTIES_CHANGED_TRUE=%.0f\n", C1[CU_PC]);
    printf("UPDATE_REQUIRED_TRUE=%.0f\n",     C1[CU_URT]);
    printf("UPDATE_REQUIRED_FALSE=%.0f\n",    C1[CU_URF]);
    printf("CAPTURE_BEGIN=%.0f\n",            C1[CU_CB]);
    printf("CAPTURE_END=%.0f\n",              C1[CU_CE]);
    printf("INSTANTIATE=%.0f\n",              C1[CU_INST]);
    printf("EXEC_UPDATE=%.0f\n",              C1[CU_UPD]);
    printf("LAUNCH=%.0f\n",                   C1[CU_LCH]);
    printf("NO_UPDATE_LAUNCH=%.0f\n",         C1[CU_NOL]);
    printf("INSTANCE_NULL_TRUE=%.0f\n",       C1[CU_NULL]);
    printf("WINDOW_LLAMA[miu=%.4f gb=%.4f si=%.4f gc=%.4f]\n", D1[DEC_URT], D1[DEC_GB], D1[DEC_SI], D1[DEC_GC]);

    std::vector<double> host(MEAS), syncw(MEAS), dps(MEAS), argm(MEAS), greedy(MEAS);
    double sum_dps = 0, sum_host = 0, sum_argm = 0;
    for (int i = 0; i < MEAS; ++i) {
        host[i]   = td[i] - tb[i];
        syncw[i]  = tg[i] - td[i];
        dps[i]    = tg[i] - tb[i];
        argm[i]   = ta[i] - tg[i];
        greedy[i] = ta[i] - tb[i];
        sum_dps += dps[i]; sum_host += host[i]; sum_argm += argm[i];
    }

    stats("HOST_DECODE_SUBMISSION_MS", host);
    stats("BACKEND_SYNC_WAIT_MS", syncw);
    stats("DECODE_PLUS_SYNC_MS", dps);
    stats("ARGMAX_MS", argm);
    stats("GREEDY_TOKEN_MS", greedy);

    printf("BACKEND_ACTIVE_INTERVAL_SUM_MS=%.4f\n", sum_dps);
    printf("ENGINE004_BASELINE_BACKEND_EQUIVALENT_TPS=%.4f\n", (double) MEAS / (sum_dps / 1000.0));
    printf("ENGINE004_BASELINE_END_TO_END_GREEDY_TPS=%.4f\n", (double) MEAS / ((w1 - w0) / 1000.0));

    { std::ofstream f(out + "/run003-tokens.txt");
      for (int i = 0; i < MEAS; ++i) f << toks[i] << "\n"; }
    { std::ofstream f(out + "/run003-latencies.csv");
      f << "i,host_decode_submission_ms,backend_sync_wait_ms,decode_plus_sync_ms,argmax_ms,greedy_token_ms,token\n";
      for (int i = 0; i < MEAS; ++i)
          f << i << "," << host[i] << "," << syncw[i] << "," << dps[i] << "," << argm[i] << "," << greedy[i] << "," << toks[i] << "\n"; }
    { std::ofstream f(out + "/run003-counters.txt");
      f << "PROPERTIES_CHANGED_TRUE=" << C1[CU_PC] << "\nUPDATE_REQUIRED_TRUE=" << C1[CU_URT]
        << "\nCAPTURE_BEGIN=" << C1[CU_CB] << "\nCAPTURE_END=" << C1[CU_CE]
        << "\nINSTANTIATE=" << C1[CU_INST] << "\nEXEC_UPDATE=" << C1[CU_UPD]
        << "\nLAUNCH=" << C1[CU_LCH] << "\nNO_UPDATE_LAUNCH=" << C1[CU_NOL] << "\n"; }

    llama_batch_free(b); llama_free(x); llama_model_free(m); llama_backend_free();
    return 0;
}
