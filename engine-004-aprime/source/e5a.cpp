// ENGINE-005A — corpus d'attribution GPU, runtime GELÉ engine003 V3 (aucun patch bibliothèque).
//
// Modes :
//   A) régime naturel (graphes ACTIFS) : mesure le régime du produit, CPU-side ; nécessite
//      --skip pour sortir de la classe B (transition de capacité KV), et --snapshot pour
//      garantir 0 transition dans la fenêtre (mêmes tampons à chaque token => propriétés
//      de graphe identiques, donc pas de franchissement de palier).
//   B) attributs de nœuds (graphes DÉSACTIVÉS par GGML_CUDA_DISABLE_GRAPHS=1) : chaque nœud
//      est lancé individuellement, donc la timeline devient attribuable par nom de kernel.
//
// Aucun flag spécial côté bibliothèque : le compteur engine002_v3 ne voit que les graphes,
// désactiver les graphes le rend donc mécaniquement nul (et c'est déclaré).
//
// Fenêtre : --skip K tokens décodés avant, puis --profile N tokens mesurés et profilés.

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
#include <dlfcn.h>

// [ENGINE-005C] surface de l'instrumentation par couche (fournie par la bibliotheque).
// Liaison C : ces deux fonctions sont exportees par la bibliotheque avec un nom non mangue
// (le harnais n'inclut PAS l'en-tete, pour ne pas dupliquer l'etat statique).
extern "C" void ggml_layer_t_dump_now(void);
extern "C" bool ggml_layer_timing_enabled(void);
// [ENGINE-005D] provenance de construction
extern "C" void ggml_prov_dump_now(void);
extern "C" void ggml_prov_layer_report(void);
extern "C" void ggml_prov_signatures_top(int n);
extern "C" void ggml_prov_role_report(void);

extern "C" {
// telemetrie engine-003 V3 : absente du runtime diagnostique E5C -> symboles faibles
__attribute__((weak)) void engine002_v3_cuda_telemetry_get(double *);
__attribute__((weak)) void engine002_v3_cuda_telemetry_reset(void);
__attribute__((weak)) void engine002_v3_decode_telemetry_get(double *);
__attribute__((weak)) void engine002_v3_decode_telemetry_reset(void);
}
static bool have_v3() { return engine002_v3_cuda_telemetry_get != nullptr; }

enum { CU_NULL = 0, CU_PC = 1, CU_URT = 2, CU_URF = 3, CU_CB = 4, CU_CE = 5, CU_INST = 6, CU_UPD = 7, CU_LCH = 8, CU_NOL = 9 };

static const long LF     = 248320;
static const int  MAXPRE = 64;

static double ms() {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (double) t.tv_sec * 1000.0 + (double) t.tv_nsec / 1e6;
}
static void die(const char * m) { fprintf(stderr, "E5A FAIL %s\n", m); exit(2); }

static std::string hexb(const unsigned char * h, int n) {
    static const char * x = "0123456789abcdef";
    std::string s;
    for (int i = 0; i < n; ++i) { s += x[h[i] >> 4]; s += x[h[i] & 15]; }
    return s;
}

// ---------------------------------------------------------------- NVTX (chargement paresseux)
typedef void (*nvtx_range_push_t)(const char *);
typedef void (*nvtx_range_pop_t)(void);
static nvtx_range_push_t p_nvtx_push = nullptr;
static nvtx_range_pop_t  p_nvtx_pop  = nullptr;

static void nvtx_init() {
    void * h = dlopen("libnvtx3interop.so.1", RTLD_NOW | RTLD_GLOBAL);
    if (!h) h = dlopen("libnvToolsExt.so.1", RTLD_NOW | RTLD_GLOBAL);
    if (h) {
        p_nvtx_push = (nvtx_range_push_t) dlsym(h, "nvtxRangePushA");
        p_nvtx_pop  = (nvtx_range_pop_t)  dlsym(h, "nvtxRangePop");
    }
    printf("NVTX_AVAILABLE=%s\n", p_nvtx_push ? "YES" : "NO");
}
static inline void nvtx_push(const char * n) { if (p_nvtx_push) p_nvtx_push(n); }
static inline void nvtx_pop()                { if (p_nvtx_pop)  p_nvtx_pop(); }

static void stats(const char * name, const std::vector<double> & v) {
    std::vector<double> s = v;
    std::sort(s.begin(), s.end());
    double mn = 0, sd = 0;
    for (double q : v) mn += q;
    mn /= (double) v.size();
    for (double q : v) sd += (q - mn) * (q - mn);
    sd = std::sqrt(sd / (double) v.size());
    printf("%s p50=%.4f p95=%.4f mean=%.4f sd=%.4f min=%.4f max=%.4f sum=%.4f\n",
           name, s[s.size() / 2], s[(size_t) (0.95 * (s.size() - 1))], mn, sd, s[0], s[s.size() - 1], mn * v.size());
}

// filtre de log (harnais) : PER_TOKEN_LIBRARY_TEXT_LOGGING=0 sans toucher aux DSO
static long g_dbg_dropped = 0, g_kept = 0;
static void log_filter(enum ggml_log_level level, const char * text, void *) {
    if (level == GGML_LOG_LEVEL_DEBUG || level == GGML_LOG_LEVEL_CONT) { g_dbg_dropped++; return; }
    g_kept++;
    fputs(text, stderr);
}

int main(int argc, char ** argv) {
    if (argc < 4) die("usage: e5a <model.gguf> <prefix805.ids.txt> <outdir> [--skip K] [--profile N] [--snapshot] [--tag NAME]");

    const std::string model = argv[1], pfx = argv[2], out = argv[3];
    int  skip = 0, prof = 0;
    bool snapshot = false;
    std::string tag = "e5a";
    for (int i = 4; i < argc; ++i) {
        if (!strcmp(argv[i], "--skip") && i + 1 < argc)    { skip = atoi(argv[++i]); }
        else if (!strcmp(argv[i], "--profile") && i + 1 < argc) { prof = atoi(argv[++i]); }
        else if (!strcmp(argv[i], "--snapshot"))           { snapshot = true; }
        else if (!strcmp(argv[i], "--tag") && i + 1 < argc){ tag = argv[++i]; }
    }
    if (prof <= 0) die("--profile N requis");

    std::vector<int32_t> ids;
    { std::ifstream f(pfx); long t; while (f >> t) ids.push_back((int32_t) t); }
    if (ids.size() != 805) die("prefix must contain 805 ids");

    llama_log_set(log_filter, nullptr);
    ggml_log_set(log_filter, nullptr);
    nvtx_init();

    const char * graphs_disabled = getenv("GGML_CUDA_DISABLE_GRAPHS");
    printf("E5A_TAG=%s\nE5A_GRAPHS_ENABLED=%s\nE5A_SKIP_TOKENS=%d\nE5A_PROFILE_TOKENS=%d\nE5A_SNAPSHOT_BATCH=%s\n",
           tag.c_str(), graphs_disabled ? "NO" : "YES", skip, prof, snapshot ? "YES" : "NO");

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
    long      gpu_calls = 0;

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
        gpu_calls++;
    };

    // pré-roll adaptatif (même critère que partout)
    if (have_v3()) { engine002_v3_cuda_telemetry_reset(); engine002_v3_decode_telemetry_reset(); }
    double C[10] = {0};
    int preroll = 0, consec = 0;
    if (have_v3()) {
        while (preroll < MAXPRE) {
            int nt = 0; step(&nt); cur = nt; pos++; preroll++;
            engine002_v3_cuda_telemetry_get(C);
            if (C[CU_NOL] >= 1.0) { consec++; if (consec >= 2) break; } else consec = 0;
        }
    } else {
        // sans telemetrie V3 : pre-roll fixe equivalent (2 tokens apres le warmup observe)
        for (int k = 0; k < 4; ++k) { int nt = 0; step(&nt); cur = nt; pos++; preroll++; }
    }
    printf("E5A_ADAPTIVE_PREROLL=%d\n", preroll);

    // ------------------------------------------------ tokens sautés (hors mesure)
    for (int i = 0; i < skip; ++i) { int nt = 0; step(&nt); cur = nt; pos++; }

    size_t kv_pos = (size_t) pos;   // position KV au début de la fenêtre

    // ------------------------------------------------ fenêtre mesurée + profilée
    std::vector<double>  tb(prof), td(prof), tg(prof), ta(prof);
    std::vector<int32_t> toks(prof);

    if (have_v3()) { engine002_v3_cuda_telemetry_reset(); engine002_v3_decode_telemetry_reset(); }
    double C0[10] = {0};
    if (have_v3()) { engine002_v3_cuda_telemetry_get(C0); }

    nvtx_push("E5A_PROFILE_WINDOW");
    const double w0 = ms();
    for (int i = 0; i < prof; ++i) {
        char r[64];
        snprintf(r, sizeof(r), "TOKEN_%03d", i);
        nvtx_push(r);
        int nt = 0;
        step(&nt);
        nvtx_pop();
        toks[i] = nt; tb[i] = tt[0]; td[i] = tt[1]; tg[i] = tt[2]; ta[i] = tt[3];
        cur = nt; pos++;
        if (snapshot) { b.pos[0] = 805; }   // position figée : mêmes tampons à chaque token
    }
    const double w1 = ms();
    nvtx_pop();

    double C1[10] = {0}, D1[16] = {0};
    if (have_v3()) { engine002_v3_cuda_telemetry_get(C1); engine002_v3_decode_telemetry_get(D1); }
    printf("V3_TELEMETRY_PRESENT=%s\n", have_v3() ? "YES" : "NO");

    unsigned char h[32];
    SHA256((const uint8_t *) toks.data(), toks.size() * sizeof(int32_t), h);
    printf("E5A_OUTPUT_TOKEN_SEQUENCE_SHA256=%s\n", hexb(h, 32).c_str());
    printf("E5A_KV_POSITION_AT_WINDOW_START=%zu\n", kv_pos);

    printf("PROPERTIES_CHANGED_TRUE=%.0f\nUPDATE_REQUIRED_TRUE=%.0f\nCAPTURE_BEGIN=%.0f\nCAPTURE_END=%.0f\n",
           C1[CU_PC], C1[CU_URT], C1[CU_CB], C1[CU_CE]);
    printf("INSTANTIATE=%.0f\nEXEC_UPDATE=%.0f\nLAUNCH=%.0f\nNO_UPDATE_LAUNCH=%.0f\nINSTANCE_NULL_TRUE=%.0f\n",
           C1[CU_INST], C1[CU_UPD], C1[CU_LCH], C1[CU_NOL], C1[CU_NULL]);
    printf("WINDOW_LLAMA[miu=%.4f gb=%.4f si=%.4f gc=%.4f]\n", D1[2], D1[4], D1[8], D1[10]);
    printf("LOG_FILTER_DEBUG_DROPPED=%ld KEPT=%ld\n", g_dbg_dropped, g_kept);
    printf("HOST_LLAMA_DECODE_CALLS=%ld\n", gpu_calls);

    std::vector<double> host(prof), syncw(prof), dps(prof), argm(prof);
    for (int i = 0; i < prof; ++i) {
        host[i]  = td[i] - tb[i];
        syncw[i] = tg[i] - td[i];
        dps[i]   = tg[i] - tb[i];
        argm[i]  = ta[i] - tg[i];
    }
    stats("HOST_DECODE_SUBMISSION_MS", host);
    stats("BACKEND_SYNC_WAIT_MS", syncw);
    stats("DECODE_PLUS_SYNC_MS", dps);
    stats("ARGMAX_MS", argm);
    printf("E5A_WINDOW_WALL_MS=%.4f\n", w1 - w0);
    printf("E5A_WINDOW_TPS=%.4f\n", (double) prof / ((w1 - w0) / 1000.0));

    { std::ofstream f(out + "/" + tag + "-tokens.txt");
      for (int i = 0; i < prof; ++i) f << toks[i] << "\n"; }
    { std::ofstream f(out + "/" + tag + "-latencies.csv");
      f << "i,host_decode_submission_ms,backend_sync_wait_ms,decode_plus_sync_ms,argmax_ms,token\n";
      for (int i = 0; i < prof; ++i)
          f << i << "," << host[i] << "," << syncw[i] << "," << dps[i] << "," << argm[i] << "," << toks[i] << "\n"; }
    { std::ofstream f(out + "/" + tag + "-counters.txt");
      f << "PROPERTIES_CHANGED_TRUE=" << C1[CU_PC] << "\nUPDATE_REQUIRED_TRUE=" << C1[CU_URT]
        << "\nCAPTURE_BEGIN=" << C1[CU_CB] << "\nCAPTURE_END=" << C1[CU_CE]
        << "\nINSTANTIATE=" << C1[CU_INST] << "\nEXEC_UPDATE=" << C1[CU_UPD]
        << "\nLAUNCH=" << C1[CU_LCH] << "\nNO_UPDATE_LAUNCH=" << C1[CU_NOL]
        << "\nKV_POSITION_AT_WINDOW_START=" << kv_pos << "\n"; }

    // attribution par couche : vidage APRES la fenetre (§5 : le harnais porte la declaration)
    ggml_layer_t_dump_now();
    ggml_prov_dump_now();
    ggml_prov_layer_report();
    ggml_prov_role_report();
    ggml_prov_signatures_top(24);
    if (ggml_layer_timing_enabled()) {
        printf("LAYER_TIMING_ENABLED=YES\n");
    } else {
        printf("LAYER_TIMING_ENABLED=NO\n");
    }

    llama_batch_free(b); llama_free(x); llama_model_free(m); llama_backend_free();
    return 0;
}
