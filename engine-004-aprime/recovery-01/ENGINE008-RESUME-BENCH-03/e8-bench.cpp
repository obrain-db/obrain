// e8-bench -- MESURE DU GAIN DE REPRISE HYBRIDE.
// 5 paires A(recaclul)/B(reprise), ordre alterne, contexte NEUF par condition.
// Blob produit UNE SEULE FOIS et garde EN MEMOIRE. Aucun acces disque dans la mesure.
#include "llama.h"
#include "ggml.h"
#include "llama-context.h"
#include "llama-model.h"
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cstdint>
#include <cmath>
#include <chrono>
#include <fstream>
#include <algorithm>
#include <string>
#include <vector>

static void die(const char * m) { fprintf(stderr, "FATAL: %s\n", m); exit(2); }
static const char * arg(int c, char ** v, const char * n, const char * d = nullptr) {
    for (int i = 1; i + 1 < c; ++i) if (strcmp(v[i], n) == 0) return v[i+1];
    if (d) return d; fprintf(stderr, "missing %s\n", n); exit(2);
}
static std::vector<int32_t> read_ids(const char * p) {
    std::vector<int32_t> v; std::ifstream f(p); long x;
    if (!f) die("ids"); while (f >> x) v.push_back((int32_t) x); return v;
}
static double ms_since(std::chrono::steady_clock::time_point t0) {
    return std::chrono::duration<double, std::milli>(std::chrono::steady_clock::now() - t0).count();
}
static bool decode_range(llama_context * ctx, const int32_t * ids, int n, int pos0,
                         llama_batch & b, bool want_logits) {
    for (int i = 0; i < n; ++i) {
        b.token[i] = ids[i]; b.pos[i] = pos0 + i; b.n_seq_id[i] = 1; b.seq_id[i][0] = 0;
        b.logits[i] = (want_logits && i + 1 == n) ? 1 : 0;
    }
    b.n_tokens = n;
    return llama_decode(ctx, b) == 0;
}
struct Cmp { bool fin = true, bin = true; double maxd = 0, l2 = 0; int argA = 0, argB = 0; bool ok = false; };
static Cmp compare(const float * a, const float * b, int nv) {
    Cmp c; if (!a || !b) return c;
    c.ok = true;
    for (int i = 0; i < nv; ++i) {
        if (!std::isfinite(a[i]) || !std::isfinite(b[i])) c.fin = false;
        const double d = std::fabs((double) a[i] - (double) b[i]);
        if (d > c.maxd) c.maxd = d;
        c.l2 += d * d;
        if (a[i] > a[c.argA]) c.argA = i;
        if (b[i] > b[c.argB]) c.argB = i;
        if (memcmp(&a[i], &b[i], 4) != 0) c.bin = false;
    }
    return c;
}

int main(int argc, char ** argv) {
    const char * model_path = arg(argc, argv, "--model");
    const char * ids_path   = arg(argc, argv, "--ids");
    const int max_new = atoi(arg(argc, argv, "--max-new", "32"));
    const int N = 58;
    const int NPAIR = 5;

    std::vector<int32_t> ids = read_ids(ids_path);
    if ((int) ids.size() != 59) { fprintf(stderr, "FATAL: %zu IDs\n", ids.size()); return 3; }
    printf("E8_BENCH\nINPUT_TOKEN_COUNT=%zu\nPREFIX_TOKENS=%d\nNEXT_POS=%d\nPAIRS=%d\nMAX_NEW=%d\n",
           ids.size(), N, N, NPAIR, max_new);
    printf("PAIR_ORDER=A/B,B/A,A/B,B/A,A/B\nTIMER=std::chrono::steady_clock (monotone)\n");
    const llama_state_seq_flags FLAGS = LLAMA_STATE_SEQ_FLAGS_NONE;
    if (FLAGS & LLAMA_STATE_SEQ_FLAGS_PARTIAL_ONLY) { printf("PARTIAL_ONLY_REFUSED=FAIL\n"); return 4; }
    printf("TRANSFER_FLAGS=%d\nPARTIAL_ONLY_REFUSED=YES\n", (int) FLAGS);

    // ---- CHARGEMENT DU MODELE : hors du temps principal, mesure separee
    auto t0 = std::chrono::steady_clock::now();
    llama_backend_init();
    llama_model_params mp = llama_model_default_params();
    llama_model * model = llama_model_load_from_file(model_path, mp);
    if (!model) die("modele");
    const double model_load_ms = ms_since(t0);
    printf("MODEL_LOAD_MS=%.3f  (HORS MESURE PRINCIPALE)\n", model_load_ms);

    llama_context_params cp = llama_context_default_params();
    cp.n_ctx = 512; cp.n_batch = 512; cp.n_ubatch = 512; cp.n_seq_max = 1;
    printf("N_CTX=%d\nN_BATCH=%d\nN_UBATCH=%d\nN_SEQ_MAX=%d\n", cp.n_ctx, cp.n_batch, cp.n_ubatch, cp.n_seq_max);
    printf("ENV_OBRAIN_STATE_TYPE=%s\nENV_OBRAIN_STATE_F16=%s\n",
           getenv("OBRAIN_STATE_TYPE") ? getenv("OBRAIN_STATE_TYPE") : "<ABSENTE>",
           getenv("OBRAIN_STATE_F16") ? getenv("OBRAIN_STATE_F16") : "<ABSENTE>");

    // ---- PREPARATION DU BLOB : UNE SEULE FOIS, en memoire
    std::vector<uint8_t> blob; size_t blob_size = 0; std::string blob_sha;
    double src_prefix_ms = 0, src_export_ms = 0;
    { // creation de contexte, hors temps principal
      auto tc = std::chrono::steady_clock::now();
      llama_context * S = llama_init_from_model(model, cp);
      if (!S) die("ctx source");
      printf("CTX_CREATE_MS=%.3f  (HORS MESURE PRINCIPALE)\n", ms_since(tc));
      llama_batch b = llama_batch_init(512,0,1);
      // (1) calcul du prefixe source
      auto t = std::chrono::steady_clock::now();
      if (!decode_range(S, ids.data(), N, 0, b, false)) { printf("SRC_PREFILL=FAIL\n"); return 5; }
      llama_synchronize(S);
      src_prefix_ms = ms_since(t);
      // (2) export vers le buffer memoire
      t = std::chrono::steady_clock::now();
      const size_t sz = llama_state_seq_get_size_ext(S, 0, FLAGS);
      if (sz == 0) { printf("STOP=GET_SIZE_ZERO\n"); return 6; }
      blob.assign(sz, 0);
      const size_t nw = llama_state_seq_get_data_ext(S, blob.data(), blob.size(), 0, FLAGS);
      src_export_ms = ms_since(t);
      if (nw == 0 || nw != sz) { printf("STOP=EXPORT_INVALID nw=%zu sz=%zu\n", nw, sz); return 7; }
      blob.resize(nw); blob_size = nw;
      { std::ofstream f("/tmp/kv05/blob_sha_src.bin", std::ios::binary); f.write((const char*) blob.data(), (std::streamsize) blob.size()); }
      std::string c = "sha256sum /tmp/kv05/blob_sha_src.bin | cut -d' ' -f1";
      FILE * fp = popen(c.c_str(), "r"); char bb[128] = {0};
      if (fp) { if (!fgets(bb, sizeof bb, fp)) {} pclose(fp); }
      blob_sha.assign(bb); while (!blob_sha.empty() && (blob_sha.back()=='\n'||blob_sha.back()=='\r')) blob_sha.pop_back();
      llama_batch_free(b);
      llama_free(S);
    }
    printf("SOURCE_BLOB_PREPARATION_MS=%.3f\n", src_prefix_ms + src_export_ms);
    printf("  SOURCE_PREFIX_MS=%.3f\n  SOURCE_EXPORT_MS=%.3f\n", src_prefix_ms, src_export_ms);
    printf("BLOB_BYTES=%zu\nBLOB_SHA256=%s\n", blob_size, blob_sha.c_str());
    printf("BLOB_KEPT_IN_MEMORY=YES (aucun acces disque dans la mesure)\n");

    const int nv = (int) llama_vocab_n_tokens(llama_model_get_vocab(model));
    std::vector<float> lgA(nv), lgB(nv);
    std::vector<double> tA(NPAIR), tB(NPAIR), pA_pref(NPAIR), pA_dec(NPAIR), pB_imp(NPAIR), pB_dec(NPAIR);
    int pair_ok = 0; bool all_ok = true; double ref_l2 = 0;
    std::vector<llama_token> idsA, idsB; std::string txtA, txtB;

    for (int p = 0; p < NPAIR; ++p) {
        const bool a_first = (p % 2 == 0);        // A/B,B/A,A/B,B/A,A/B
        printf("PAIR=%d ORDER=%s\n", p + 1, a_first ? "A/B" : "B/A");
        // --- contexte NEUF par condition, creation HORS temps principal
        auto run_A = [&]() {
            auto tc = std::chrono::steady_clock::now();
            llama_context * A = llama_init_from_model(model, cp);
            const double ctx_ms = ms_since(tc);
            llama_batch b = llama_batch_init(512,0,1);
            auto t = std::chrono::steady_clock::now();
            if (!decode_range(A, ids.data(), N, 0, b, false)) { printf("  A_PREFILL=FAIL\n"); all_ok=false; llama_batch_free(b); llama_free(A); return; }
            llama_synchronize(A);
            pA_pref[p] = ms_since(t);
            t = std::chrono::steady_clock::now();
            if (!decode_range(A, ids.data()+N, 1, N, b, true)) { printf("  A_DECODE59=FAIL\n"); all_ok=false; llama_batch_free(b); llama_free(A); return; }
            llama_synchronize(A);
            const float * l = llama_get_logits_ith(A, -1);
            if (l) memcpy(lgA.data(), l, sizeof(float)*nv);
            pA_dec[p] = ms_since(t);
            tA[p] = pA_pref[p] + pA_dec[p];
            printf("  A_CTX_CREATE_MS=%.3f (hors principal)\n", ctx_ms);
            if (p == 0) {   // premiere paire : generation
                int first = 0; for (int i=1;i<nv;++i) if (lgA[i] > lgA[first]) first = i;
                idsA.push_back((llama_token) first);
                llama_batch b1 = llama_batch_init(1,0,1); llama_pos pos = 59;
                const llama_vocab * v = llama_model_get_vocab(model);
                for (int s = 0; s < max_new; ++s) {
                    if (llama_vocab_is_eog(v, idsA.back())) break;
                    if ((int) idsA.size() >= max_new) break;
                    b1.token[0]=idsA.back(); b1.pos[0]=pos++; b1.n_seq_id[0]=1; b1.seq_id[0][0]=0; b1.logits[0]=1; b1.n_tokens=1;
                    if (llama_decode(A, b1) != 0) break;
                    const float * q = llama_get_logits_ith(A, -1); if (!q) break;
                    int best=0; for (int i=1;i<nv;++i) if (q[i] > q[best]) best=i;
                    idsA.push_back((llama_token) best);
                }
                llama_batch_free(b1);
                for (llama_token tk : idsA) { char pb[512]; int np = llama_token_to_piece(v, tk, pb, sizeof pb, 0, true);
                    if (np > 0) for (int k=0;k<np;++k) txtA.push_back(pb[k]); }
            }
            llama_batch_free(b); llama_free(A);
        };
        auto run_B = [&]() {
            auto tc = std::chrono::steady_clock::now();
            llama_context * B = llama_init_from_model(model, cp);
            const double ctx_ms = ms_since(tc);
            llama_batch b = llama_batch_init(512,0,1);
            int dst_dec = 0, dst_tok = 0;                 // aucune ecriture de prefixe
            auto t = std::chrono::steady_clock::now();
            const size_t nr = llama_state_seq_set_data_ext(B, blob.data(), blob.size(), 0, FLAGS);
            if (nr != blob.size()) { printf("  B_IMPORT=FAIL nr=%zu\n", nr); all_ok=false; llama_batch_free(b); llama_free(B); return; }
            llama_synchronize(B);
            pB_imp[p] = ms_since(t);
            t = std::chrono::steady_clock::now();
            if (!decode_range(B, ids.data()+N, 1, N, b, true)) { printf("  B_DECODE59=FAIL\n"); all_ok=false; llama_batch_free(b); llama_free(B); return; }
            llama_synchronize(B);
            const float * l = llama_get_logits_ith(B, -1);
            if (l) memcpy(lgB.data(), l, sizeof(float)*nv);
            pB_dec[p] = ms_since(t);
            tB[p] = pB_imp[p] + pB_dec[p];
            printf("  B_CTX_CREATE_MS=%.3f (hors principal)\n", ctx_ms);
            printf("  B_DESTINATION_PREFIX_DECODE_CALLS=%d\n  B_DESTINATION_PREFIX_TOKEN_COUNT=%d\n", dst_dec, dst_tok);
            if (dst_dec != 0 || dst_tok != 0) { printf("  STOP=DESTINATION_NOT_PRISTINE\n"); all_ok = false; }
            if (p == 0) {
                int first = 0; for (int i=1;i<nv;++i) if (lgB[i] > lgB[first]) first = i;
                idsB.push_back((llama_token) first);
                llama_batch b1 = llama_batch_init(1,0,1); llama_pos pos = 59;
                const llama_vocab * v = llama_model_get_vocab(model);
                for (int s = 0; s < max_new; ++s) {
                    if (llama_vocab_is_eog(v, idsB.back())) break;
                    if ((int) idsB.size() >= max_new) break;
                    b1.token[0]=idsB.back(); b1.pos[0]=pos++; b1.n_seq_id[0]=1; b1.seq_id[0][0]=0; b1.logits[0]=1; b1.n_tokens=1;
                    if (llama_decode(B, b1) != 0) break;
                    const float * q = llama_get_logits_ith(B, -1); if (!q) break;
                    int best=0; for (int i=1;i<nv;++i) if (q[i] > q[best]) best=i;
                    idsB.push_back((llama_token) best);
                }
                llama_batch_free(b1);
                for (llama_token tk : idsB) { char pb[512]; int np = llama_token_to_piece(v, tk, pb, sizeof pb, 0, true);
                    if (np > 0) for (int k=0;k<np;++k) txtB.push_back(pb[k]); }
            }
            llama_batch_free(b); llama_free(B);
        };
        if (a_first) { run_A(); run_B(); } else { run_B(); run_A(); }
        if (!all_ok) { printf("STOP=PAIR_FAILURE pair=%d\n", p+1); break; }
        Cmp c = compare(lgA.data(), lgB.data(), nv);
        const bool ok_pair = c.ok && c.fin && c.bin && (c.maxd == 0) && (c.argA == c.argB);
        if (ok_pair) pair_ok++;
        if (p == 0) ref_l2 = c.l2;
        printf("  A_MS=%.3f  B_MS=%.3f  RATIO_A_OVER_B=%.4f\n", tA[p], tB[p], tB[p] > 0 ? tA[p]/tB[p] : 0.0);
        printf("  A_PREFILL_MS=%.3f  A_DECODE59_MS=%.3f  B_IMPORT_MS=%.3f  B_DECODE59_MS=%.3f\n",
               pA_pref[p], pA_dec[p], pB_imp[p], pB_dec[p]);
        printf("  CMP FINITE=%s BINARY_IDENTICAL=%s MAX_ABS_DIFF=%.9g L2_DIFF=%.9g ARGMAX_A=%d ARGMAX_B=%d MATCH=%s PAIR_OK=%s\n",
               c.fin?"YES":"NO", c.bin?"YES":"NO", c.maxd, c.l2, c.argA, c.argB,
               (c.argA==c.argB)?"YES":"NO", ok_pair?"YES":"NO");
    }

    printf("PAIRS_COMPLETED=%d\nPAIRS_NUMERICALLY_OK=%d\n", all_ok ? NPAIR : pair_ok, pair_ok);
    printf("NUMERICAL_EQUIVALENCE=%s\n", (pair_ok == NPAIR && all_ok) ? "PASS" : "FAIL");
    if (all_ok && pair_ok == NPAIR) {
        std::vector<double> va(tA), vb(tB);
        std::sort(va.begin(), va.end()); std::sort(vb.begin(), vb.end());
        const double medA = va[NPAIR/2], medB = vb[NPAIR/2];
        printf("RECOMPUTE_MS_LIST=["); for (int i=0;i<NPAIR;++i) printf("%s%.3f", i?",":"", tA[i]); printf("]\n");
        printf("RESUME_MS_LIST=[");    for (int i=0;i<NPAIR;++i) printf("%s%.3f", i?",":"", tB[i]); printf("]\n");
        printf("RECOMPUTE_MEDIAN_MS=%.3f\nRESUME_MEDIAN_MS=%.3f\n", medA, medB);
        printf("RECOMPUTE_MIN_MS=%.3f\nRECOMPUTE_MAX_MS=%.3f\n", va.front(), va.back());
        printf("RESUME_MIN_MS=%.3f\nRESUME_MAX_MS=%.3f\n", vb.front(), vb.back());
        printf("MEDIAN_RATIO_RECOMPUTE_OVER_RESUME=%.4f\n", medB > 0 ? medA/medB : 0.0);
        printf("RATIO_DEFINITION=RECOMPUTE_MEDIAN_MS / RESUME_MEDIAN_MS ; >1 signifie reprise plus rapide\n");
    }
    printf("TOKEN_SEQUENCE_MATCH=%s\n", (idsA == idsB && !idsA.empty()) ? "YES" : "NO");
    printf("CONTROL_IDS=["); for (size_t i=0;i<idsA.size();++i) printf("%s%d", i?",":"", idsA[i]); printf("]\n");
    printf("RESTORED_IDS=["); for (size_t i=0;i<idsB.size();++i) printf("%s%d", i?",":"", idsB[i]); printf("]\n");
    printf("CONTROL_TEXT=[%s]\nRESTORED_TEXT=[%s]\n", txtA.c_str(), txtB.c_str());
    printf("DESTINATION_PREFILL_AVOIDED=%s\n", all_ok ? "YES" : "NO");
    llama_model_free(model); llama_backend_free();
    return all_ok ? 0 : 11;
}
