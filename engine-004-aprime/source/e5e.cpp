// ENGINE-005E — pont mural causal : ARM_LOGITS_ON / ARM_LOGITS_OFF, teacher-forced.
//
// Rejoue la séquence gelée de RUN-003 (token IDs identiques injectes, jamais choisis par les
// logits), dans les deux bras, avec le meme modele, runtime, etat initial, positions.
//
// Mesure : BACKEND_ACTIVE_INTERVAL_SUM (decode->logits), paroi continue, distributions.
// Delta = cout mural causal du chemin de sortie.
//
// Runtime : GELE engine003 V3. Aucun decoupage d'execution.

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
// telemetrie engine-003 V3 : absente du runtime op-timing -> symboles faibles
__attribute__((weak)) void engine002_v3_cuda_telemetry_get(double *);
__attribute__((weak)) void engine002_v3_cuda_telemetry_reset(void);
__attribute__((weak)) void engine002_v3_decode_telemetry_get(double *);
__attribute__((weak)) void engine002_v3_decode_telemetry_reset(void);
}
static bool have_v3() { return engine002_v3_cuda_telemetry_get != nullptr; }
enum { CU_PC = 1, CU_CB = 4, CU_CE = 5, CU_INST = 6, CU_UPD = 7, CU_LCH = 8, CU_NOL = 9 };

// [ENGINE-005G] surface du recensement de dispatch
// [ENGINE-006C] instruments optionnels : symboles FAIBLES, le harnais se lie a tout runtime.
extern "C" __attribute__((weak)) void ggml_census_dump_now(void);
extern "C" __attribute__((weak)) void e6_dup_report(void);
static inline void e6c_dump_optional(void) {
    if (ggml_census_dump_now != nullptr) { ggml_census_dump_now(); }
    if (e6_dup_report        != nullptr) { e6_dup_report(); }
}
extern "C" __attribute__((weak)) int  ggml_census_role_report(void);

static const int MEAS = 256;
static double ms() { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return t.tv_sec*1000.0 + t.tv_nsec/1e6; }
static void die(const char * m) { fprintf(stderr, "E5E FAIL %s\n", m); exit(2); }
static std::string hexb(const unsigned char * h, int n) {
    static const char * x = "0123456789abcdef"; std::string s;
    for (int i = 0; i < n; ++i) { s += x[h[i]>>4]; s += x[h[i]&15]; }
    return s;
}
static void stats(const char * n, std::vector<double> & v) {
    std::vector<double> s = v; std::sort(s.begin(), s.end());
    double m = 0, sd = 0; for (double q : v) m += q; m /= (double) v.size();
    for (double q : v) sd += (q-m)*(q-m); sd = std::sqrt(sd/(double) v.size());
    printf("%s p50=%.4f p95=%.4f mean=%.4f sd=%.4f min=%.4f max=%.4f\n", n,
           s[s.size()/2], s[(size_t)(0.95*(s.size()-1))], m, sd, s[0], s[s.size()-1]);
}
static long g_dbg = 0, g_kept = 0;
static void log_filter(enum ggml_log_level level, const char * text, void *) {
    if (level == GGML_LOG_LEVEL_DEBUG || level == GGML_LOG_LEVEL_CONT) { g_dbg++; return; }
    g_kept++; fputs(text, stderr);
}

int main(int argc, char ** argv) {
    if (argc < 5) die("usage: e5e <model> <prefix805.ids.txt> <baseline-tokens.txt> <outdir> <ON|OFF>");
    const std::string model = argv[1], pfx = argv[2], seqf = argv[3], out = argv[4], arm = argv[5];
    int n_meas = MEAS;
    for (int i = 6; i < argc; ++i) { if (strcmp(argv[i], "--tokens") == 0 && i + 1 < argc) { n_meas = atoi(argv[++i]); } }
    if (n_meas <= 0 || n_meas > MEAS) { n_meas = MEAS; }
    const bool logits_on = (arm == "ON");

    std::vector<int32_t> ids, seq;
    { std::ifstream f(pfx); long t; while (f >> t) ids.push_back((int32_t) t); }
    { std::ifstream f(seqf); long t; while (f >> t) seq.push_back((int32_t) t); }
    if (ids.size() != 805) die("prefix must contain 805 ids");
    if ((int) seq.size() < MEAS) die("sequence too short");

    llama_log_set(log_filter, nullptr);
    ggml_log_set(log_filter, nullptr);

    // [E5G-R2] la provenance est alimentee par le chemin d'instrumentation par couche :
    // les deux variables doivent etre posees ensemble.
    setenv("GGML_LAYER_PROVENANCE", "1", 1);
    setenv("GGML_LAYER_TIMING", "1", 1);
    printf("E5E_ARM=%s\nE5E_TEACHER_FORCED_TOKENS=%d\nE5E_LOGITS_REQUESTED_PER_TOKEN=%s\n",
           arm.c_str(), MEAS, logits_on ? "YES" : "NO");

    llama_backend_init();
    llama_model_params mp = llama_model_default_params();
    llama_model * m = llama_model_load_from_file(model.c_str(), mp);
    if (!m) die("model");
    llama_context_params cp = llama_context_default_params();
    cp.n_ctx = 4096; cp.n_batch = 256; cp.n_ubatch = 256; cp.n_seq_max = 2;
    llama_context * x = llama_init_from_model(m, cp);
    if (!x) die("ctx");

    llama_batch b = llama_batch_init(256, 0, 2);
    { static const int S[4] = {256,256,245,48}; size_t off = 0;
      for (int si = 0; si < 4; ++si) { const int n = S[si];
          for (int i = 0; i < n; ++i) { b.token[i]=ids[off+i]; b.pos[i]=(llama_pos)(off+i); b.n_seq_id[i]=1; b.seq_id[i][0]=0; b.logits[i]=0; }
          b.n_tokens = n; if (llama_decode(x, b) != 0) die("prefix"); off += (size_t) n; } }

    // pre-roll adaptatif (meme critere partout)
    if (have_v3()) { engine002_v3_cuda_telemetry_reset(); engine002_v3_decode_telemetry_reset(); }
    double C[10] = {0};
    int preroll = 0, consec = 0;
    int cur = ids[ids.size()-1];
    llama_pos pos = 805;
    auto one = [&](int tok, llama_pos p, int request_logits) {
        b.token[0]=tok; b.pos[0]=p; b.n_seq_id[0]=1; b.seq_id[0][0]=0;
        b.logits[0] = request_logits; b.n_tokens = 1;
        if (llama_decode(x, b) != 0) die("decode");
    };
    while (preroll < 64) {
        one(cur, pos, 1); pos++; preroll++;
        if (have_v3()) {
            engine002_v3_cuda_telemetry_get(C);
            if (C[CU_NOL] >= 1.0) { consec++; if (consec >= 2) break; } else consec = 0;
        } else if (preroll >= 4) { break; }
    }
    printf("E5E_ADAPTIVE_PREROLL=%d\n", preroll);

    // ------------------------------------------------ fenetre teacher-forced
    // ARM=OFF : saut REELLE de la projection de sortie (variable lue par le builder de graphe).
    // La sonde finale la reactive : les deux bras calculent donc la meme sonde.
    if (!logits_on) { setenv("GGML_E5E_SKIP_HEAD", "1", 1); }
    std::vector<double> host(n_meas), syncw(n_meas), dps(n_meas);
    double sum_host = 0, sum_sync = 0, sum_dps = 0;
    const double w0 = ms();
    for (int i = 0; i < n_meas; ++i) {
        const double t0 = ms();
        one(seq[i], pos, logits_on ? 1 : 0);
        const double t1 = ms();
        const double t2 = t1;
        if (logits_on) {
            const float * lg = llama_get_logits_ith(x, 0);
            if (lg == nullptr) die("logits null avec logits=1");
            (void) lg[0];
            (void) t2;
        }
        const double t2b = ms();
        host[i]  = t1 - t0;
        syncw[i] = t2b - t1;
        dps[i]   = t2b - t0;
        sum_host += host[i]; sum_sync += syncw[i]; sum_dps += dps[i];
        pos++;
    }
    const double w1 = ms();

    double C1[10] = {0}, D1[16] = {0};
    if (have_v3()) { engine002_v3_cuda_telemetry_get(C1); engine002_v3_decode_telemetry_get(D1); }

    printf("E5E_BACKEND_ACTIVE_INTERVAL_SUM_MS=%.4f\n", sum_dps);
    printf("E5E_HOST_SUBMISSION_SUM_MS=%.4f\n", sum_host);
    printf("E5E_SYNC_WAIT_SUM_MS=%.4f\n", sum_sync);
    printf("E5E_WALL_MS=%.4f\n", w1 - w0);
    printf("E5E_BACKEND_MS_PER_TOKEN=%.4f\n", sum_dps / n_meas);
    printf("E5E_WALL_MS_PER_TOKEN=%.4f\n", (w1 - w0) / n_meas);
    printf("E5E_TPS=%.4f\n", (double) n_meas / ((w1 - w0) / 1000.0));
    stats("E5E_DECODE_PLUS_SYNC_MS", dps);
    stats("E5E_HOST_SUBMISSION_MS", host);
    printf("E5E_CUDA[pc=%.0f cb=%.0f ce=%.0f inst=%.0f upd=%.0f lch=%.0f nol=%.0f]\n",
           C1[CU_PC], C1[CU_CB], C1[CU_CE], C1[CU_INST], C1[CU_UPD], C1[CU_LCH], C1[CU_NOL]);
    printf("E5E_WINDOW_LLAMA[miu=%.4f gb=%.4f si=%.4f gc=%.4f]\n", D1[2], D1[4], D1[8], D1[10]);
    printf("E5E_LOG_FILTER_DEBUG_DROPPED=%ld KEPT=%ld\n", g_dbg, g_kept);

    ggml_census_dump_now();   // [ENGINE-005G] lecture unique apres la fenetre
    e6_dup_report();          // [ENGINE-005I] compteurs de sequence native / repetitions

    // ------------------------------------------------ porte d'etat : une sonde finale
    // logits demandes pour un token fixe, apres les memes 256 tokens dans les deux bras.
    unsetenv("GGML_E5E_SKIP_HEAD");   // sonde : tete active dans les DEUX bras
    one(seq[n_meas-1], pos, 1);
    const float * lg = llama_get_logits_ith(x, 0);
    if (lg == nullptr) die("probe logits null");
    unsigned char h[32];
    SHA256((const uint8_t *) lg, (size_t) 248320 * sizeof(float), h);
    printf("E5E_PROBE_TOKEN=%d\nE5E_PROBE_LOGITS_SHA256=%s\n", seq[n_meas-1], hexb(h, 32).c_str());

    std::ofstream f(out + "/e5e-" + arm + "-latencies.csv");
    f << "i,host_submission_ms,sync_wait_ms,decode_plus_sync_ms\n";
    for (int i = 0; i < n_meas; ++i) f << i << "," << host[i] << "," << syncw[i] << "," << dps[i] << "\n";

    ggml_census_dump_now();      // [ENGINE-005G] totaux globaux
    ggml_census_role_report();   // [ENGINE-005G-R2] jointure par role de poids

    llama_batch_free(b); llama_free(x); llama_model_free(m); llama_backend_free();
    return 0;
}
