// ENGINE-006C-R1 — harnais produit minimal, derive du protocole RUN-003.
//
// AUCUN crochet d'instrumentation historique : pas de telemetrie V3, pas de census, pas de
// compteur de provenance, pas d'enregistreur de graphe. Seulement l'API publique llama/ggml.
//
// Trois modes, selectionnes par variable d'environnement (un processus neuf par run) :
//   E6CR1_MODE=greedy   (defaut) : decode autoregressif greedy, 256 jetons mesures
//   E6CR1_MODE=teacher           : rejoue la sequence gelee (fidelite par sonde)
//
// Le harnais REFUSE de produire un SHA si le tampon correspondant est vide ou incomplet.

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

static const int MEASURED_TOKENS = 256;
static const int PREROLL_MIN     = 1;
static const int PREROLL_MAX     = 64;
static const int PREROLL_STABLE  = 2;      // runs consecutifs chauds exiges
static const double PREROLL_SYNC_THRESHOLD_MS = 1.0;

static double ms() { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return t.tv_sec*1000.0 + t.tv_nsec/1e6; }
static void die(const char * m) { fprintf(stderr, "E6CR1_FAIL %s\n", m); exit(2); }
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
static void stats(const char * n, std::vector<double> & v) {
    if (v.empty()) { printf("%s p50=NA (tampon vide)\n", n); return; }
    std::vector<double> s = v; std::sort(s.begin(), s.end());
    double m = 0, sd = 0;
    for (double q : v) m += q; m /= (double) v.size();
    for (double q : v) sd += (q-m)*(q-m); sd = std::sqrt(sd/(double) v.size());
    printf("%s p50=%.4f p90=%.4f p95=%.4f p99=%.4f mean=%.4f sd=%.4f min=%.4f max=%.4f\n", n,
           s[s.size()/2], s[(size_t)(0.90*(s.size()-1))], s[(size_t)(0.95*(s.size()-1))],
           s[(size_t)(0.99*(s.size()-1))], m, sd, s[0], s[s.size()-1]);
}

int main(int argc, char ** argv) {
    if (argc < 5) die("usage: e6cr1 <model> <prefix805.ids.txt> <teacher-tokens.txt> <bindir>");
    const std::string model = argv[1], pfx = argv[2], seqf = argv[3];
    const char * mode_env = getenv("E6CR1_MODE");
    const std::string mode = mode_env ? mode_env : "greedy";
    const bool teacher = (mode == "teacher");

    std::vector<int32_t> ids, seq;
    { std::ifstream f(pfx); long t; while (f >> t) ids.push_back((int32_t) t); }
    { std::ifstream f(seqf); long t; while (f >> t) seq.push_back((int32_t) t); }
    if (ids.size() != 805)  die("prefix doit contenir 805 ids");
    if (seq.size() < MEASURED_TOKENS) die("sequence teacher trop courte");

    llama_log_set(log_filter, nullptr);
    ggml_log_set(log_filter, nullptr);

    printf("E6CR1_MODE=%s\nE6CR1_MODEL=%s\nE6CR1_PREFIX_IDS=%zu\nE6CR1_MEASURED_TOKEN_COUNT=%d\n",
           mode.c_str(), model.c_str(), ids.size(), MEASURED_TOKENS);
    fflush(stdout);

    llama_backend_init();
    llama_model_params mp = llama_model_default_params();
    llama_model * m = llama_model_load_from_file(model.c_str(), mp);
    if (!m) die("chargement du modele");
    const int n_vocab = llama_vocab_n_tokens(llama_model_get_vocab(m));
    printf("E6CR1_N_VOCAB=%d\n", n_vocab);

    llama_context_params cp = llama_context_default_params();
    cp.n_ctx = 4096; cp.n_batch = 256; cp.n_ubatch = 256; cp.n_seq_max = 2;
    llama_context * x = llama_init_from_model(m, cp);
    if (!x) die("creation du contexte");

    llama_batch b = llama_batch_init(256, 0, 2);
    { static const int S[4] = {256,256,245,48}; size_t off = 0;
      for (int si = 0; si < 4; ++si) { const int n = S[si];
          for (int i = 0; i < n; ++i) { b.token[i]=ids[off+i]; b.pos[i]=(llama_pos)(off+i); b.n_seq_id[i]=1; b.seq_id[i][0]=0; b.logits[i]=0; }
          b.n_tokens = n; if (llama_decode(x, b) != 0) die("decode du prefixe"); off += (size_t) n; } }

    int      cur = ids[ids.size()-1];
    llama_pos pos = 805;
    auto step = [&](int tok, int want_logits) {
        b.token[0]=tok; b.pos[0]=pos; b.n_seq_id[0]=1; b.seq_id[0][0]=0;
        b.logits[0]=want_logits; b.n_tokens=1;
        if (llama_decode(x, b) != 0) die("decode");
    };

    // pre-roll adaptatif : RUN-003 s'arrete apres 4 runs, critere de chauffe
    int preroll = 0, consec = 0;
    while (preroll < PREROLL_MAX) {
        step(cur, 1); pos++; preroll++;
        const float * lg = llama_get_logits_ith(x, 0);
        if (lg == nullptr) die("logits nuls pendant le pre-roll");
        int best = 0; for (int i = 1; i < n_vocab; ++i) if (lg[i] > lg[best]) best = i;
        cur = best;
        if (preroll >= PREROLL_MIN) { consec++; if (consec >= PREROLL_STABLE) break; }
    }
    printf("E6CR1_ADAPTIVE_PREROLL=%d\n", preroll);

    // ------------------------------------------------ fenetre mesuree
    std::vector<double> host(MEASURED_TOKENS), syncw(MEASURED_TOKENS), argmx(MEASURED_TOKENS), dps(MEASURED_TOKENS);
    std::vector<int32_t> out_tokens;      // 256 jetons produits/consommes
    out_tokens.reserve(MEASURED_TOKENS);
    double sum_host = 0, sum_sync = 0, sum_argm = 0, sum_dps = 0;
    const double w0 = ms();
    for (int i = 0; i < MEASURED_TOKENS; ++i) {
        const int32_t tok = teacher ? seq[i] : cur;
        const double t0 = ms();
        step(tok, 1);
        const double t1 = ms();
        const float * lg = llama_get_logits_ith(x, 0);
        if (lg == nullptr) die("logits nuls pendant la fenetre");
        const double t2 = ms();
        int best = 0; for (int j = 1; j < n_vocab; ++j) if (lg[j] > lg[best]) best = j;
        const double t3 = ms();
        out_tokens.push_back(teacher ? (int32_t) best : (int32_t) best);
        if (!teacher) { cur = best; }
        else          { cur = seq[i]; }
        pos++;
        host[i] = t1-t0; syncw[i] = t2-t1; argmx[i] = t3-t2; dps[i] = t3-t0;
        sum_host += host[i]; sum_sync += syncw[i]; sum_argm += argmx[i]; sum_dps += dps[i];
    }
    const double w1 = ms();

    // ------------------------------------------------ garde anti-vide
    if ((int) out_tokens.size() != MEASURED_TOKENS) die("tampon de jetons incomplet");
    const size_t tok_bytes = out_tokens.size() * sizeof(int32_t);
    if (tok_bytes == 0) die("tampon de jetons vide");
    unsigned char th[32];
    SHA256((const uint8_t *) out_tokens.data(), tok_bytes, th);
    printf("E6CR1_TOKEN_BUFFER_BYTES=%zu\n", tok_bytes);
    printf("E6CR1_OUTPUT_TOKEN_SEQUENCE_SHA256=%s\n", hexb(th,32).c_str());

    const size_t lg_bytes = (size_t) n_vocab * sizeof(float);
    if (lg_bytes == 0) die("tampon de logits vide");
    const float * last = llama_get_logits_ith(x, 0);
    if (last == nullptr) die("logits finaux nuls");
    unsigned char ph[32];
    SHA256((const uint8_t *) last, lg_bytes, ph);
    printf("E6CR1_PROBE_LOGIT_COUNT=%d\nE6CR1_PROBE_LOGIT_BUFFER_BYTES=%zu\n", n_vocab, lg_bytes);
    printf("E6CR1_PROBE_LOGITS_SHA256=%s\n", hexb(ph,32).c_str());

    printf("E6CR1_BACKEND_ACTIVE_INTERVAL_SUM_MS=%.4f\n", sum_dps);
    printf("E6CR1_GREEDY_CONTINUOUS_WINDOW_MS=%.4f\n", w1-w0);
    printf("E6CR1_BACKEND_EQUIVALENT_TPS=%.4f\n", (double) MEASURED_TOKENS / (sum_dps/1000.0));
    printf("E6CR1_END_TO_END_GREEDY_TPS=%.4f\n", (double) MEASURED_TOKENS / ((w1-w0)/1000.0));
    printf("E6CR1_MS_PER_TOKEN=%.4f\n", (w1-w0)/MEASURED_TOKENS);
    stats("E6CR1_HOST_DECODE_SUBMISSION_MS", host);
    stats("E6CR1_BACKEND_SYNC_WAIT_MS", syncw);
    stats("E6CR1_ARGMAX_MS", argmx);
    stats("E6CR1_DECODE_PLUS_ARGMAX_MS", dps);
    printf("E6CR1_LOG_FILTER_DEBUG_DROPPED=%ld KEPT=%ld\n", g_dbg, g_kept);

    // sequence complete ecrite pour comparaison inter-bras
    { std::ofstream f(std::string(argv[4]) + "/tokens-" + mode + ".txt");
      for (int32_t t : out_tokens) f << t << "\n"; }

    llama_batch_free(b); llama_free(x); llama_model_free(m); llama_backend_free();
    return 0;
}
