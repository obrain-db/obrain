// ENGINE-004 APRIME — A' : long-run naturel minimal, regime greedy autoregressif.
//
// Question : DO_GRAPH_TRANSITIONS_OCCUR_NATURALLY_OVER_A_LONG_AUTOREGRESSIVE_RUN ?
//
// Contrat de la fenetre (256 tokens) :
//   NO_CSV_WRITES=YES            -> aucun fichier ecrit pendant la fenetre
//   NO_PER_TOKEN_TEXT_LOGGING=YES -> aucun stdout/stderr par token dans la fenetre
//   NO_DYNAMIC_ALLOCATION=YES    -> tous les buffers reserves avant la fenetre
//   NO_RUNTIME_PATCH=YES         -> aucune modification d'etat de la lib pendant la fenetre
//   telemetrie : reset UNE fois avant la fenetre, lecture UNE fois apres (aucune lecture par token).
//
// Defauts 4/5 (attribution de latence), mesures dans la boucle :
//   t0 avant llama_decode / t1 apres llama_decode / t2 apres llama_get_logits_ith / t3 apres argmax
//   HOST_DECODE_SUBMISSION_MS = t1-t0     (ce que coute l'appel llama_decode cote hote)
//   BACKEND_SYNC_WAIT_MS      = t2-t1     (ce que coute la recuperation des logits)
//   DECODE_PLUS_SYNC_MS       = t2-t0
//   ARGMAX_MS                 = t3-t2
//   GREEDY_TOKEN_MS           = t3-t0
//
// Aucun patch runtime : on n'utilise que l'API publique + les compteurs engine-003.

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
#include <unistd.h>

extern "C" {
void engine002_v3_decode_telemetry_get(double *);
void engine002_v3_cuda_telemetry_get(double *);
void engine002_v3_decode_telemetry_reset(void);
void engine002_v3_cuda_telemetry_reset(void);
}

// V3 CUDA telemetry indices (source : build-source/ggml/src/ggml-cuda/ggml-cuda.cu)
enum {
    CU_NULL_TRUE = 0,  // instance_null_true
    CU_PC        = 1,  // properties_changed_true
    CU_URT       = 2,  // update_required_true
    CU_URF       = 3,  // update_required_false
    CU_CB        = 4,  // capture_begin
    CU_CE        = 5,  // capture_end
    CU_INST      = 6,  // instantiate
    CU_UPD       = 7,  // exec_update
    CU_LCH       = 8,  // launch
    CU_NOL       = 9,  // no_update_launch
};

enum { DEC_URT = 2, DEC_GB = 4, DEC_SI = 8, DEC_GC = 10 };  // V3 decode telemetry indices

static const long LF     = 248320;      // taille du vocabulaire (argmax complet, meme chemin que RUN-002)
static const int  MEAS   = 256;         // fenetre A'
static const int  MAXPRE = 64;          // borne du warmup adaptatif

static double ms() {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (double) t.tv_sec * 1000.0 + (double) t.tv_nsec / 1e6;
}

static void die(const char * m) {
    fprintf(stderr, "APRIME FAIL %s\n", m);
    exit(2);
}

static std::string hexb(const unsigned char * h, int n) {
    static const char * x = "0123456789abcdef";
    std::string s;
    for (int i = 0; i < n; ++i) {
        s += x[h[i] >> 4];
        s += x[h[i] & 15];
    }
    return s;
}

static void stats(const char * name, const std::vector<double> & v) {
    std::vector<double> s = v;
    std::sort(s.begin(), s.end());
    double mn = 0.0, sd = 0.0;
    for (double q : v) mn += q;
    mn /= (double) v.size();
    for (double q : v) sd += (q - mn) * (q - mn);
    sd = std::sqrt(sd / (double) v.size());
    printf("%s p50=%.3f p95=%.3f mean=%.3f sd=%.3f min=%.3f max=%.3f\n", name,
           s[s.size() / 2], s[(size_t) (0.95 * (double) (s.size() - 1))], mn, sd, s[0], s[s.size() - 1]);
}

int main(int argc, char ** argv) {
    if (argc < 4) {
        die("usage: aprime2 <model.gguf> <prefix805.ids.txt> <outdir>");
    }
    const std::string model = argv[1];
    const std::string pfx   = argv[2];
    const std::string out   = argv[3];

    // [DATEUR] borne l'evenement silencieux (warmup reset) par un COMPTE DE CALCULS,
    // jamais par le temps : AUCUN appel d'horloge n'est fait dans step() quand arm=0.
    const char * budget_env = getenv("APRIME_BUDGET");
    const long budget = budget_env ? atol(budget_env) : -1;
    long       computes = 0;
    bool       armed    = false;
    auto dateur = [&]() {
        if (budget > 0) {
            computes++;
            if (!armed && computes >= budget) {
                armed = true;
                fprintf(stderr, "DATEUR_FIRED_COMPUTES=%ld\n", computes);
            } else if (armed) {
                fprintf(stderr, "DATEUR_POST_FIRED_COMPUTES=%ld\n", computes);
            }
        }
    };

    std::vector<int32_t> ids;
    {
        std::ifstream f(pfx);
        long t;
        while (f >> t) ids.push_back((int32_t) t);
    }
    if (ids.size() != 805) die("prefix must contain 805 ids");

    // ------------------------------------------------ provenance
    printf("APRIME_MODEL=%s\nAPRIME_PREFIX=%s\nAPRIME_PREFIX_IDS=%zu\n", model.c_str(), pfx.c_str(), ids.size());

    llama_backend_init();
    llama_model_params mp = llama_model_default_params();
    llama_model * m = llama_model_load_from_file(model.c_str(), mp);
    if (!m) die("model load");

    llama_context_params cp = llama_context_default_params();
    cp.n_ctx     = 4096;
    cp.n_batch   = 256;
    cp.n_ubatch  = 256;
    cp.n_seq_max = 2;
    llama_context * x = llama_init_from_model(m, cp);
    if (!x) die("context");

    llama_batch b = llama_batch_init(256, 0, 2);

    // prefixe identique a RUN-001/RUN-002 : 4 decodes de {256,256,245,48}
    {
        static const int S[4] = { 256, 256, 245, 48 };
        size_t off = 0;
        for (int si = 0; si < 4; ++si) {
            const int n = S[si];
            for (int i = 0; i < n; ++i) {
                b.token[i]     = ids[off + i];
                b.pos[i]       = (llama_pos) (off + i);
                b.n_seq_id[i]  = 1;
                b.seq_id[i][0] = 0;
                b.logits[i]    = 0;
            }
            b.n_tokens = n;
            if (llama_decode(x, b) != 0) die("prefix decode");
            off += (size_t) n;
        }
    }

    int         cur = ids[ids.size() - 1];
    llama_pos   pos = 805;
    double      next_t[4] = { 0, 0, 0, 0 };   // t3 du token precedent -> t0 du suivant

    // un pas greedy complet, horodate aux quatre points (defauts 4/5)
    auto step = [&](int * next_tok) {
        dateur();
        const double t0 = ms();
        b.token[0]     = cur;
        b.pos[0]       = pos;
        b.n_seq_id[0]  = 1;
        b.seq_id[0][0] = 0;
        b.logits[0]    = 1;
        b.n_tokens     = 1;
        if (llama_decode(x, b) != 0) die("decode");
        const double t1 = ms();
        const float * lg = llama_get_logits_ith(x, 0);
        const double t2 = ms();
        long am = 0;
        for (long i = 1; i < LF; ++i) {
            if (lg[i] > lg[am]) am = i;
        }
        const double t3 = ms();
        next_t[0] = t0;
        next_t[1] = t1;
        next_t[2] = t2;
        next_t[3] = t3;
        *next_tok = (int) am;
    };

    // ------------------------------------------------ warmup adaptatif
    // Identique A/B/C et RUN-002 : jusqu'a deux no_update_launch consecutifs.
    engine002_v3_cuda_telemetry_reset();
    engine002_v3_decode_telemetry_reset();

    double C[10];
    int    preroll = 0, consec = 0;
    while (preroll < MAXPRE) {
        int nt = 0;
        step(&nt);
        cur = nt;
        pos++;
        preroll++;
        engine002_v3_cuda_telemetry_get(C);
        if (C[CU_NOL] >= 1.0) {
            consec++;
            if (consec >= 2) break;
        } else {
            consec = 0;
        }
    }
    engine002_v3_cuda_telemetry_get(C);
    printf("APRIME_ADAPTIVE_PREROLL=%d\n", preroll);
    printf("PREROLL_CUDA[cb=%.0f ce=%.0f inst=%.0f upd=%.0f pc=%.0f urT=%.0f urF=%.0f lch=%.0f nol=%.0f nul=%.0f]\n",
           C[CU_CB], C[CU_CE], C[CU_INST], C[CU_UPD], C[CU_PC], C[CU_URT], C[CU_URF], C[CU_LCH], C[CU_NOL], C[CU_NULL_TRUE]);
    printf("WARMUP_CAPTURE_INVARIANT=%s\n", (C[CU_CB] == C[CU_CE]) ? "PASS" : "FAIL");
    if (!(C[CU_CB] >= 1.0 && C[CU_CE] >= 1.0 && C[CU_INST] >= 1.0 && C[CU_NOL] >= 1.0)) {
        die("warmup regime not reached");
    }

    // ------------------------------------------------ buffers de la fenetre (allocation AVANT)
    std::vector<double>  t_begin(MEAS), t_dec(MEAS), t_gl(MEAS), t_am(MEAS);
    std::vector<int32_t> toks(MEAS);

    // reset UNIQUE, juste avant la fenetre : plus aucune lecture telemetrie avant la fin
    engine002_v3_decode_telemetry_reset();
    engine002_v3_cuda_telemetry_reset();

    // ------------------------------------------------ FENETRE 256 TOKENS
    const double w0 = ms();
    for (int i = 0; i < MEAS; ++i) {
        int nt = 0;
        step(&nt);
        toks[i]  = nt;
        t_begin[i] = next_t[0];
        t_dec[i] = next_t[1];
        t_gl[i]  = next_t[2];
        t_am[i]  = next_t[3];
        cur      = nt;
        pos++;
    }
    const double w1 = ms();

    // lecture UNIQUE de la telemetrie, apres la fenetre
    double C2[10], D2[16];
    engine002_v3_cuda_telemetry_get(C2);
    engine002_v3_decode_telemetry_get(D2);
    const double w_read = ms();

    // ------------------------------------------------ restitution (apres la fenetre)
    unsigned char h[32];
    SHA256((const uint8_t *) toks.data(), toks.size() * sizeof(int32_t), h);
    printf("APRIME_GENERATED_TOKEN_COUNT=%d\n", MEAS);
    printf("APRIME_OUTPUT_TOKEN_SEQUENCE_SHA256=%s\n", hexb(h, 32).c_str());
    printf("APRIME_WINDOW_WALL_S=%.6f\n", (w1 - w0) / 1000.0);
    printf("APRIME_TELEMETRY_READ_MS=%.4f\n", w_read - w1);

    printf("APRIME_PROPERTIES_CHANGED_TRUE=%.0f\n", C2[CU_PC]);
    printf("APRIME_UPDATE_REQUIRED_TRUE=%.0f\n",    C2[CU_URT]);
    printf("APRIME_UPDATE_REQUIRED_FALSE=%.0f\n",   C2[CU_URF]);
    printf("APRIME_INSTANCE_NULL_TRUE=%.0f\n",      C2[CU_NULL_TRUE]);
    printf("APRIME_CAPTURE_BEGIN=%.0f\n",           C2[CU_CB]);
    printf("APRIME_CAPTURE_END=%.0f\n",             C2[CU_CE]);
    printf("APRIME_INSTANTIATE=%.0f\n",             C2[CU_INST]);
    printf("APRIME_EXEC_UPDATE=%.0f\n",             C2[CU_UPD]);
    printf("APRIME_LAUNCH=%.0f\n",                  C2[CU_LCH]);
    printf("APRIME_NO_UPDATE_LAUNCH=%.0f\n",        C2[CU_NOL]);
    printf("APRIME_CAPTURE_BEGIN_EQ_CAPTURE_END=%s\n", (C2[CU_CB] == C2[CU_CE]) ? "PASS" : "FAIL");

    // ------------------------------------------------ latences (defauts 4/5)
    {
        std::vector<double> host(MEAS), syncw(MEAS), dps(MEAS), argm(MEAS), greedy(MEAS);
        double sum_host = 0, sum_sync = 0, sum_dps = 0, sum_argm = 0;
        for (int i = 0; i < MEAS; ++i) {
            host[i]   = t_dec[i] - t_begin[i];   // HOST_DECODE_SUBMISSION_MS
            syncw[i]  = t_gl[i] - t_dec[i];      // BACKEND_SYNC_WAIT_MS
            dps[i]    = t_gl[i] - t_begin[i];    // DECODE_PLUS_SYNC_MS
            argm[i]   = t_am[i] - t_gl[i];       // ARGMAX_MS
            greedy[i] = t_am[i] - t_begin[i];    // GREEDY_TOKEN_MS
            sum_host += host[i];
            sum_sync += syncw[i];
            sum_dps  += dps[i];
            sum_argm += argm[i];
        }
        printf("HOST_DECODE_SUBMISSION_MS mean=%.4f total=%.4f\n", sum_host / MEAS, sum_host);
        printf("BACKEND_SYNC_WAIT_MS mean=%.4f total=%.4f\n", sum_sync / MEAS, sum_sync);
        printf("DECODE_PLUS_SYNC_MS mean=%.4f total=%.4f\n", sum_dps / MEAS, sum_dps);
        printf("ARGMAX_MS mean=%.4f total=%.4f\n", sum_argm / MEAS, sum_argm);
        stats("HOST_DECODE_SUBMISSION_MS_DIST", host);
        stats("BACKEND_SYNC_WAIT_MS_DIST", syncw);
        stats("DECODE_PLUS_SYNC_MS_DIST", dps);
        stats("ARGMAX_MS_DIST", argm);

        // backend equivalent = somme des intervalles ou le backend travaille (decode -> logits)
        const double backend_s = sum_dps / 1000.0;
        const double greedy_s  = (w1 - w0) / 1000.0;
        printf("BACKEND_ACTIVE_INTERVAL_SUM_MS=%.4f\n", sum_dps);
        printf("BACKEND_EQUIVALENT_TPS=%.4f\n", (double) MEAS / backend_s);
        printf("GREEDY_CONTINUOUS_WALL_SECONDS=%.6f\n", greedy_s);
        printf("END_TO_END_GREEDY_TPS=%.4f\n", (double) MEAS / greedy_s);
        printf("AMORTIZED_GRAPH_MAINTENANCE_COST_PER_TOKEN_MS=NOT_MEASURED_IN_APRIME\n");

        // ---------------------------------------------------- fichiers (apres la fenetre)
        {
            std::ofstream f(out + "/aprime-latencies.csv");
            f << "i,t0_ms,t1_ms,t2_ms,t3_ms,host_decode_submission_ms,backend_sync_wait_ms,decode_plus_sync_ms,argmax_ms,greedy_token_ms,token\n";
            for (int i = 0; i < MEAS; ++i) {
                f << i << "," << t_begin[i] << "," << t_dec[i] << "," << t_gl[i] << "," << t_am[i] << ","
                  << host[i] << "," << syncw[i] << "," << dps[i] << "," << argm[i] << "," << greedy[i] << ","
                  << toks[i] << "\n";
            }
        }
        {
            std::ofstream f(out + "/aprime-tokens.txt");
            for (int i = 0; i < MEAS; ++i) f << toks[i] << "\n";
        }
    }
    {
        std::ofstream f(out + "/aprime-counters.txt");
        f << "APRIME_ADAPTIVE_PREROLL=" << preroll << "\n";
        f << "APRIME_PROPERTIES_CHANGED_TRUE=" << C2[CU_PC] << "\n";
        f << "APRIME_UPDATE_REQUIRED_TRUE=" << C2[CU_URT] << "\n";
        f << "APRIME_UPDATE_REQUIRED_FALSE=" << C2[CU_URF] << "\n";
        f << "APRIME_INSTANCE_NULL_TRUE=" << C2[CU_NULL_TRUE] << "\n";
        f << "APRIME_CAPTURE_BEGIN=" << C2[CU_CB] << "\n";
        f << "APRIME_CAPTURE_END=" << C2[CU_CE] << "\n";
        f << "APRIME_INSTANTIATE=" << C2[CU_INST] << "\n";
        f << "APRIME_EXEC_UPDATE=" << C2[CU_UPD] << "\n";
        f << "APRIME_LAUNCH=" << C2[CU_LCH] << "\n";
        f << "APRIME_NO_UPDATE_LAUNCH=" << C2[CU_NOL] << "\n";
        f << "WINDOW_LLAMA[miu=" << D2[DEC_URT] << " gb=" << D2[DEC_GB] << " si=" << D2[DEC_SI] << " gc=" << D2[DEC_GC] << "]\n";
    }

    llama_batch_free(b);
    llama_free(x);
    llama_model_free(m);
    llama_backend_free();
    return 0;
}
