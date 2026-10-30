// ENGINE-005F — pont mural de la TETE DE SORTIE, sans confondeur de sortie hote.
//
// Les DEUX bras : logits hote = OFF (aucune copie vers l'hote, aucune extraction).
//   ARM_HEAD_ON  : tete de sortie construite et executee
//   ARM_HEAD_OFF : tete de sortie absente du graphe (bascule GGML_E5F_SKIP_OUTPUT_HEAD)
// Meme binaire, memes DSO, meme modele, memes tokens injectes.
//
// Frontiere de completion IDENTIQUE dans les deux bras : llama_synchronize(ctx) explicite.
// Aucune mesure ne depend de get_logits.

#include "llama.h"
#define E5F_TRACE 1
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
#include <strings.h>

// [ENGINE-005G] surface du recensement de dispatch
extern "C" void ggml_census_dump_now(void);
extern "C" int  ggml_census_path_of(const void * wptr, unsigned long long * out_count);
extern "C" int  ggml_census_role_report(void);

static const int MEAS = 256;
static double ms() { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return t.tv_sec*1000.0 + t.tv_nsec/1e6; }
static void die(const char * m) { fprintf(stderr, "E5F FAIL %s\n", m); exit(2); }
static std::string hexb(const unsigned char * h, int n) {
    static const char * x = "0123456789abcdef"; std::string s;
    for (int i = 0; i < n; ++i) { s += x[h[i]>>4]; s += x[h[i]&15]; }
    return s;
}
static long g_dbg = 0, g_kept = 0;
static void log_filter(enum ggml_log_level level, const char * text, void *) {
    if (level == GGML_LOG_LEVEL_DEBUG || level == GGML_LOG_LEVEL_CONT) { g_dbg++; return; }
    g_kept++; fputs(text, stderr);
}

int main(int argc, char ** argv) {
    if (argc < 5) die("usage: e5f <model> <prefix805.ids.txt> <baseline-tokens.txt> <outdir> <ON|OFF> [--probe]");
    const std::string model = argv[1], pfx = argv[2], seqf = argv[3], out = argv[4], arm = argv[5];
    const bool head_on = (arm == "ON");
    // --probe : rejoue les MEMES tokens puis fait la sonde. La bascule reste inchangee pendant
    // tout le processus (un changement de construction en cours de contexte casse l'allocation).
    bool probe_only = false;
    for (int i = 6; i < argc; ++i) { if (strcmp(argv[i], "--probe") == 0) { probe_only = true; } }

    std::vector<int32_t> ids, seq;
    { std::ifstream f(pfx); long t; while (f >> t) ids.push_back((int32_t) t); }
    { std::ifstream f(seqf); long t; while (f >> t) seq.push_back((int32_t) t); }
    if (ids.size() != 805) die("prefix 805");
    if ((int) seq.size() < MEAS) die("sequence trop courte");

    llama_log_set(log_filter, nullptr);
    ggml_log_set(log_filter, nullptr);

    // Bascule AVANT toute construction de graphe : elle agit des le premier decode.
    if (!head_on) { setenv("GGML_E5F_SKIP_OUTPUT_HEAD", "1", 1); }
    else          { unsetenv("GGML_E5F_SKIP_OUTPUT_HEAD"); }

    setenv("GGML_LAYER_PROVENANCE", "1", 1);   // [E5G-R2] active la provenance (table pointeur -> role)
    printf("E5F_ARM=%s\nE5F_HOST_LOGITS_OUTPUT=%s\nE5F_HEAD_COMPUTE=%s\nE5F_SYNC_PRIMITIVE=llama_synchronize_explicit\n",
           arm.c_str(), "OFF", head_on ? "ON" : "OFF");

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
          b.n_tokens = n; if (llama_decode(x, b) != 0) die("prefix"); llama_synchronize(x); off += (size_t) n; } }

    int cur = ids[ids.size()-1];
    llama_pos pos = 805;

    // [E5F-R2] trace de divergence : sonde de logits identique a plusieurs instants.
    auto probe_hash = [&](const char * label) {
        llama_batch pb = llama_batch_init(1, 0, 1);
        pb.token[0]=cur; pb.pos[0]=pos; pb.n_seq_id[0]=1; pb.seq_id[0][0]=0; pb.logits[0]=1; pb.n_tokens=1;
        if (llama_decode(x, pb) != 0) { fprintf(stderr, "E5F probe fail\n"); return; }
        const float * lg = llama_get_logits_ith(x, 0);
        unsigned char hh[32];
        SHA256((const uint8_t *) lg, (size_t) 248320 * sizeof(float), hh);
        fprintf(stderr, "E5F_TRACE_%s=%s\n", label, hexb(hh,32).c_str());
        llama_batch_free(pb);
    };
    probe_hash("AFTER_PREFIX");
    probe_hash("AFTER_PREFIX_REPEAT");
    auto step = [&](int tok, llama_pos p) {
        b.token[0]=tok; b.pos[0]=p; b.n_seq_id[0]=1; b.seq_id[0][0]=0;
        b.logits[0]=0;                       // AUCUN logits hote, dans les DEUX bras
        b.n_tokens=1;
        if (llama_decode(x, b) != 0) die("decode");
        llama_synchronize(x);                // frontiere de completion IDENTIQUE
    };
    for (int k = 0; k < 4; ++k) { step(cur, pos); pos++; }   // pre-roll fixe, identique

    // ------------------------------------------------ fenetre teacher-forced
    std::vector<double> per(MEAS);
    const double w0 = ms();
    for (int i = 0; i < MEAS; ++i) {
        const double t0 = ms();
        step(seq[i], pos);
        const double t1 = ms();
        per[i] = t1 - t0;
        pos++;
    }
    const double w1 = ms();

    std::vector<double> s = per; std::sort(s.begin(), s.end());
    double mean = 0, sd = 0;
    for (double q : per) mean += q; mean /= MEAS;
    for (double q : per) sd += (q-mean)*(q-mean); sd = std::sqrt(sd/MEAS);

    printf("E5F_PROBE_ONLY=%s\n", probe_only ? "YES" : "NO");
    printf("E5F_SYNCHRONIZED_WINDOW_MS=%.4f\n", w1 - w0);
    printf("E5F_HEAD_SKIPPED_IN_THIS_PROCESS=%s\n", head_on ? "NO" : "YES");
    printf("E5F_MS_PER_TOKEN=%.4f\n", (w1 - w0) / MEAS);
    printf("E5F_TPS=%.4f\n", (double) MEAS / ((w1 - w0) / 1000.0));
    printf("E5F_TOKEN_MS p50=%.4f p95=%.4f mean=%.4f sd=%.4f min=%.4f max=%.4f\n",
           s[MEAS/2], s[(size_t)(0.95*(MEAS-1))], mean, sd, s[0], s[MEAS-1]);

    // ------------------------------------------------ porte d'etat : sonde commune
    // La bascule n'est pas modifiee : la sonde s'execute avec la meme configuration que la fenetre.
    // Si la fenetre a tourne sans tete, la sonde est faite dans un PROCESSUS separe avec la tete.
    b.token[0]=seq[MEAS-1]; b.pos[0]=pos; b.n_seq_id[0]=1; b.seq_id[0][0]=0; b.logits[0]=1; b.n_tokens=1;
    if (llama_decode(x, b) != 0) die("probe decode");
    const float * lg = llama_get_logits_ith(x, 0);
    if (lg == nullptr) die("probe logits null");
    unsigned char h[32];
    SHA256((const uint8_t *) lg, (size_t) 248320 * sizeof(float), h);
    printf("E5F_PROBE_TOKEN=%d\nE5F_PROBE_LOGITS_SHA256=%s\n", seq[MEAS-1], hexb(h,32).c_str());

    std::ofstream f(out + "/e5f-" + arm + "-latencies.csv");
    f << "i,token_ms\n";
    for (int i = 0; i < MEAS; ++i) f << i << "," << per[i] << "\n";

    printf("E5F_LOG_FILTER_DEBUG_DROPPED=%ld KEPT=%ld\n", g_dbg, g_kept);
    printf("E5F_SYNC_CALL_COUNT_PER_TOKEN=1\n");
    ggml_census_dump_now();      // [ENGINE-005G] totaux globaux
    ggml_census_role_report();   // [ENGINE-005G-R2] jointure par role
    printf("E5F_HOST_LOGTIS_OUTPUT=%s\n", "OFF");
    llama_batch_free(b); llama_free(x); llama_model_free(m); llama_backend_free();
    return 0;
}
