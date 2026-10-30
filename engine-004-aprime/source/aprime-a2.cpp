// ENGINE-004 / A'' : meme chemin que A', mais runtime OBSERVATEUR (anneau d'evenements date
// par ordinal de calcul). Question : A2_EVENT_COUNT_MEASURED_WINDOW, et quel champ invalide le graphe.
//
// Discipline de fenetre :
//   NO_CSV_WRITES_INSIDE_WINDOW=YES        -> tous les fichiers ecrits apres les 256 tokens
//   NO_TEXT_LOGGING_INSIDE_WINDOW=YES      -> observeur fige dans la fenetre (toute tentative
//                                             d'ecriture est comptee comme violation)
//   NO_DYNAMIC_DIAGNOSTIC_ALLOCATION=YES   -> buffers reserves avant la fenetre
//   OBSERVER_RING_PREALLOCATED=YES         -> anneau statique dans la bibliotheque
//   aucune lecture de compteur par token   -> bornes posees par ordinal, pas par lecture
//
// Defauts 4/5 conserves : t0..t3 identiques a A'.

#include "llama.h"
#include <openssl/sha.h>

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cstdint>
#include <cmath>
#include <algorithm>
#include <string>
#include <vector>
#include <fstream>
#include <time.h>

extern "C" {
// chemin A' : telemetrie engine-003 V3 (absente du runtime observateur) -> declaree faiblement
__attribute__((weak)) void engine002_v3_cuda_telemetry_get(double *);
__attribute__((weak)) void engine002_v3_decode_telemetry_get(double *);
__attribute__((weak)) void engine002_v3_cuda_telemetry_reset(void);
__attribute__((weak)) void engine002_v3_decode_telemetry_reset(void);
}

// observateur ENGINE-004 : surface stable exportee par la bibliotheque.
// Declarations HORS extern "C" : la bibliotheque exporte ces fonctions avec le mangleage C++
// (verifie avec nm -D : _Z29ggml_obrain_obs_driver_window_beginv).
void ggml_obrain_obs_driver_window_begin(void);
void ggml_obrain_obs_driver_window_end(void);
void ggml_obrain_obs_driver_begin_warmup(void);
void ggml_obrain_obs_driver_end_warmup(void);
void ggml_obrain_obs_driver_freeze(bool);
void ggml_obrain_obs_driver_reset(void);
void ggml_obrain_obs_driver_counters(double *);
int  ggml_obrain_obs_driver_dump(const char *);

static const long LF     = 248320;
static const int  MEAS   = 256;
static const int  MAXPRE = 64;

static const char * APRIME_SHA =
    "210b7eb55ff64a10c1d51af5391edc21b8d4545d297817c332d5635842590231";

static double ms() {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (double) t.tv_sec * 1000.0 + (double) t.tv_nsec / 1e6;
}

static void die(const char * m) {
    fprintf(stderr, "A2 FAIL %s\n", m);
    exit(2);
}

static std::string hexb(const unsigned char * h, int n) {
    static const char * x = "0123456789abcdef";
    std::string s;
    for (int i = 0; i < n; ++i) { s += x[h[i] >> 4]; s += x[h[i] & 15]; }
    return s;
}

int main(int argc, char ** argv) {
    if (argc < 4) die("usage: aprime-a2 <model.gguf> <prefix805.ids.txt> <outdir>");
    const std::string model = argv[1], pfx = argv[2], out = argv[3];

    std::vector<int32_t> ids;
    { std::ifstream f(pfx); long t; while (f >> t) ids.push_back((int32_t) t); }
    if (ids.size() != 805) die("prefix must contain 805 ids");

    printf("A2_MODEL=%s\nA2_PREFIX_IDS=%zu\n", model.c_str(), ids.size());

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

    // ------------------------------------------------ warmup adaptatif (meme critere que A')
    if (engine002_v3_cuda_telemetry_reset) { engine002_v3_cuda_telemetry_reset(); engine002_v3_decode_telemetry_reset(); }
    ggml_obrain_obs_driver_reset();
    ggml_obrain_obs_driver_begin_warmup();

    double C[10] = { 0 };
    int    preroll = 0, consec = 0;
    while (preroll < MAXPRE) {
        int nt = 0;
        step(&nt);
        cur = nt; pos++; preroll++;
        if (engine002_v3_cuda_telemetry_get) {
            engine002_v3_cuda_telemetry_get(C);
            if (C[9] >= 1.0) { consec++; if (consec >= 2) break; } else consec = 0;
        } else {
            double O[16]; ggml_obrain_obs_driver_counters(O);
            if (O[9] >= 1.0) { consec++; if (consec >= 2) break; } else consec = 0;
        }
    }
    ggml_obrain_obs_driver_end_warmup();
    printf("A2_ADAPTIVE_PREROLL=%d\n", preroll);

    // ------------------------------------------------ buffers (alloues AVANT la fenetre)
    std::vector<double>  tb(MEAS), td(MEAS), tg(MEAS), ta(MEAS);
    std::vector<int32_t> toks(MEAS);

    // reset des compteurs observateur juste avant la fenetre, puis bornes par ordinal
    ggml_obrain_obs_driver_reset();
    double O0[16]; ggml_obrain_obs_driver_counters(O0);
    ggml_obrain_obs_driver_window_begin();

    // ------------------------------------------------ FENETRE
    ggml_obrain_obs_driver_freeze(true);   // toute I/O d'observation est refusee et comptee
    const double w0 = ms();
    for (int i = 0; i < MEAS; ++i) {
        int nt = 0;
        step(&nt);
        toks[i] = nt; tb[i] = tt[0]; td[i] = tt[1]; tg[i] = tt[2]; ta[i] = tt[3];
        cur = nt; pos++;
    }
    const double w1 = ms();
    ggml_obrain_obs_driver_window_end();
    ggml_obrain_obs_driver_freeze(false);

    double O1[16]; ggml_obrain_obs_driver_counters(O1);

    // ------------------------------------------------ restitution (hors fenetre)
    unsigned char h[32];
    SHA256((const uint8_t *) toks.data(), toks.size() * sizeof(int32_t), h);
    const std::string sha = hexb(h, 32);

    printf("A2_GENERATED_TOKEN_COUNT=%d\n", MEAS);
    printf("A2_OUTPUT_TOKEN_SEQUENCE_SHA256=%s\n", sha.c_str());
    printf("A2_TOKEN_SEQUENCE_MATCH_APRIME=%s\n", sha == APRIME_SHA ? "YES" : "NO");
    printf("A2_WINDOW_BEGIN_COMPUTE_ORDINAL=%.0f\n", O0[14] + 1.0);
    printf("A2_WINDOW_END_COMPUTE_ORDINAL=%.0f\n", O1[14] + 1.0);
    printf("A2_COMPUTE_ORDINAL_TOTAL=%.0f\n", O1[14]);
    printf("A2_PROPERTIES_CHANGED_TRUE=%.0f\n", O1[0]);
    printf("A2_UPDATE_REQUIRED_TRUE=%.0f\n", O1[2]);
    printf("A2_CAPTURE_BEGIN=%.0f\n", O1[3]);
    printf("A2_CAPTURE_END=%.0f\n", O1[4]);
    printf("A2_INSTANTIATE=%.0f\n", O1[5]);
    printf("A2_EXEC_UPDATE=%.0f\n", O1[6]);
    printf("A2_LAUNCH=%.0f\n", O1[8]);
    printf("A2_NO_UPDATE_LAUNCH=%.0f\n", O1[9]);
    printf("A2_LAUNCH_NO_GRAPH=%.0f\n", O1[10]);
    printf("A2_WARMUP_RESET=%.0f\n", O1[11]);
    printf("A2_UID_REUSED=%.0f\n", O1[12]);
    printf("A2_FROZEN_WRITES=%.0f\n", O1[13]);
    printf("A2_OBSERVER_EVENTS_TOTAL=%.0f\n", O1[15]);

    // ------------------------------------------------ latences (defauts 4/5)
    std::vector<double> host(MEAS), syncw(MEAS), dps(MEAS), argm(MEAS);
    double s_host = 0, s_sync = 0, s_dps = 0, s_argm = 0;
    for (int i = 0; i < MEAS; ++i) {
        host[i]  = td[i] - tb[i];
        syncw[i] = tg[i] - td[i];
        dps[i]   = tg[i] - tb[i];
        argm[i]  = ta[i] - tg[i];
        s_host += host[i]; s_sync += syncw[i]; s_dps += dps[i]; s_argm += argm[i];
    }
    printf("A2_HOST_DECODE_SUBMISSION_MS_total=%.4f\n", s_host);
    printf("A2_BACKEND_SYNC_WAIT_MS_total=%.4f\n", s_sync);
    printf("A2_DECODE_PLUS_SYNC_MS_total=%.4f\n", s_dps);
    printf("A2_ARGMAX_MS_total=%.4f\n", s_argm);
    printf("A2_GREEDY_WINDOW_TOTAL_S=%.6f\n", (w1 - w0) / 1000.0);
    printf("A2_END_TO_END_GREEDY_TPS=%.4f\n", (double) MEAS / ((w1 - w0) / 1000.0));
    printf("A2_BACKEND_EQUIVALENT_TPS=%.4f\n", (double) MEAS / (s_dps / 1000.0));

    // ------------------------------------------------ fichiers (apres la fenetre)
    { std::ofstream f(out + "/a2-tokens.txt");
      for (int i = 0; i < MEAS; ++i) f << toks[i] << "\n"; }
    { std::ofstream f(out + "/a2-latencies.csv");
      f << "i,host_decode_submission_ms,backend_sync_wait_ms,decode_plus_sync_ms,argmax_ms,token\n";
      for (int i = 0; i < MEAS; ++i)
          f << i << "," << host[i] << "," << syncw[i] << "," << dps[i] << "," << argm[i] << "," << toks[i] << "\n"; }

    const int dumped = ggml_obrain_obs_driver_dump((out + "/a2-observer-dump.txt").c_str());
    printf("A2_OBSERVER_DUMP_WRITTEN=%s\n", dumped ? "YES" : "NO");

    llama_batch_free(b);
    llama_free(x);
    llama_model_free(m);
    llama_backend_free();
    return 0;
}
