// e8-kv-roundtrip -- I-B : ROUNDTRIP KV MULTICOUCHE NATIF.
// Pilote DISTINCT, hors gel ENGINE008-Q1. Aucune modification de Q1.
//
// A = controle  : prefill 0..57, KV CALCULE, puis inject_decode du 59e ID
// B = injection : prefill 0..57, puis ECRITURE KV NATIVE depuis A, puis idem
//
// Preuves exigees : retour d'acceptation, readback octet-à-octet, IDs, textes.
#include "llama.h"
#include "ggml.h"
#include "llama-context.h"          // hparams : n_embd_head_k/v (comme pstate-exp0a)
#include "llama-model.h"            // type complet de llama_model
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cstdint>
#include <fstream>
#include <string>
#include <vector>
#include <algorithm>

static void die(const char * m) { fprintf(stderr, "FATAL: %s\n", m); exit(2); }
static const char * arg(int c, char ** v, const char * n, const char * d = nullptr) {
    for (int i = 1; i + 1 < c; ++i) if (strcmp(v[i], n) == 0) return v[i+1];
    if (d) return d; fprintf(stderr, "missing %s\n", n); exit(2);
}

struct Ctx {
    llama_model * model = nullptr;
    llama_context * ctx = nullptr;
    ggml_type type_k = GGML_TYPE_COUNT, type_v = GGML_TYPE_COUNT;
    int n_embd_k = 0, n_embd_v = 0;
    size_t k_row = 0, v_row = 0;
};

static Ctx make_ctx(llama_model * shared, int n_ctx, int n_batch, int n_ubatch) {
    Ctx c; c.model = shared;
    llama_context_params cp = llama_context_default_params();
    cp.n_ctx = n_ctx; cp.n_batch = n_batch; cp.n_ubatch = n_ubatch; cp.n_seq_max = 1;
    c.ctx = llama_init_from_model(c.model, cp);
    if (!c.ctx) die("creation de contexte echouee");
    c.type_k = llama_kv_cache_type_k(c.ctx);
    c.type_v = llama_kv_cache_type_v(c.ctx);
    const int n_head_kv = llama_model_n_head_kv(c.model);
    const int hk = (int) c.ctx->get_model().hparams.n_embd_head_k();
    const int hv = (int) c.ctx->get_model().hparams.n_embd_head_v();
    c.n_embd_k = n_head_kv * hk;
    c.n_embd_v = n_head_kv * hv;
    c.k_row = ggml_row_size(c.type_k, c.n_embd_k);
    c.v_row = ggml_row_size(c.type_v, c.n_embd_v);
    return c;
}

static std::vector<int32_t> read_ids(const char * p) {
    std::vector<int32_t> v; std::ifstream f(p); long x;
    if (!f) die("ids illisible"); while (f >> x) v.push_back((int32_t) x);
    return v;
}

// prefill / decode d'une plage : positions explicites, logits sur le DERNIER token
static bool decode_range(llama_context * ctx, const int32_t * ids, int n, int pos0,
                         llama_batch & b, bool want_logits) {
    for (int i = 0; i < n; ++i) {
        b.token[i] = ids[i]; b.pos[i] = pos0 + i;
        b.n_seq_id[i] = 1; b.seq_id[i][0] = 0;
        b.logits[i] = (want_logits && i + 1 == n) ? 1 : 0;
    }
    b.n_tokens = n;
    return llama_decode(ctx, b) == 0;
}

static std::string sha_of(const std::vector<uint8_t> & d) {
    static int c = 0; char p[64]; snprintf(p, sizeof p, "/tmp/.e8kv_%d", c++);
    { std::ofstream f(p, std::ios::binary); f.write((const char *) d.data(), (std::streamsize) d.size()); }
    std::string cmd = "sha256sum '"; cmd += p; cmd += "' | cut -d' ' -f1";
    FILE * fp = popen(cmd.c_str(), "r"); char buf[128] = {0};
    if (fp) { if (!fgets(buf, sizeof buf, fp)) {} pclose(fp); }
    remove(p);
    std::string r(buf); while (!r.empty() && (r.back()=='\n'||r.back()=='\r')) r.pop_back();
    return r;
}

struct Gen { std::vector<llama_token> ids; std::string text, esc, hex; const char * stop = "NONE"; };

static Gen generate(Ctx & c, llama_token first, int max_new) {
    Gen g;
    const llama_vocab * v = llama_model_get_vocab(c.model);
    const int nv = llama_vocab_n_tokens(v);
    std::vector<float> lg(nv);
    // le premier token vient des logits DEJA lus (dernier decode)
    { const float * p = llama_get_logits_ith(c.ctx, -1); if (!p) { g.stop = "ERROR"; return g; }
      memcpy(lg.data(), p, sizeof(float) * nv); }
    llama_batch b = llama_batch_init(1, 0, 1);
    llama_pos pos_next = 0;
    // position du prochain token : apres le 59e (pos 58)
    { // = 59
      pos_next = 59; }
    g.ids.push_back(first);
    for (int step = 0; step < max_new; ++step) {
        if (llama_vocab_is_eog(v, g.ids.back())) { g.stop = "EOS"; break; }
        if ((int) g.ids.size() >= max_new) { g.stop = "MAX_NEW_TOKENS"; break; }
        b.token[0] = g.ids.back(); b.pos[0] = pos_next++;
        b.n_seq_id[0] = 1; b.seq_id[0][0] = 0; b.logits[0] = 1; b.n_tokens = 1;
        if (llama_decode(c.ctx, b) != 0) { g.stop = "ERROR"; break; }
        const float * p = llama_get_logits_ith(c.ctx, -1);
        if (!p) { g.stop = "ERROR"; break; }
        memcpy(lg.data(), p, sizeof(float) * nv);
        int best = 0; for (int i = 1; i < nv; ++i) if (lg[i] > lg[best]) best = i;
        g.ids.push_back((llama_token) best);
    }
    llama_batch_free(b);
    if (g.stop[0]=='N' && strcmp(g.stop,"NONE")==0) g.stop = "MAX_NEW_TOKENS";
    for (llama_token t : g.ids) {
        char pb[512]; int np = llama_token_to_piece(v, t, pb, sizeof pb, 0, true);
        if (np < 0) { std::vector<char> big(-np); np = llama_token_to_piece(v, t, big.data(), (int) big.size(), 0, true);
                      if (np >= 0) { for (int k=0;k<np;++k){ g.text.push_back(big[k]); char x[8]; snprintf(x,sizeof x,"%02x",(unsigned char)big[k]); g.hex+=x; } } }
        else for (int k = 0; k < np; ++k) { g.text.push_back(pb[k]); char x[8]; snprintf(x,sizeof x,"%02x",(unsigned char)pb[k]); g.hex+=x; }
    }
    for (unsigned char ch : g.text) {
        if (ch=='\\') g.esc += "\\\\"; else if (ch=='"') g.esc += "\\\"";
        else if (ch >= 32 && ch < 127) g.esc += (char) ch;
        else { char x[8]; snprintf(x, sizeof x, "\\x%02x", ch); g.esc += x; } }
    return g;
}

int main(int argc, char ** argv) {
    const char * model_path = arg(argc, argv, "--model");
    const char * ids_path   = arg(argc, argv, "--ids");
    const int max_new       = atoi(arg(argc, argv, "--max-new", "32"));
    const int n_ctx         = atoi(arg(argc, argv, "--n-ctx", "512"));
    const int n_batch       = atoi(arg(argc, argv, "--n-batch", "512"));
    const int n_ubatch      = atoi(arg(argc, argv, "--n-ubatch", "512"));

    std::vector<int32_t> ids = read_ids(ids_path);
    if ((int) ids.size() != 59) { fprintf(stderr, "FATAL: %zu IDs, 59 attendus\n", ids.size()); return 3; }
    printf("E8_KV_ROUNDTRIP\nINPUT_TOKEN_COUNT=%zu\nMAX_NEW=%d\n", ids.size(), max_new);

    llama_backend_init();
    llama_model_params mp = llama_model_default_params();
    llama_model * model = llama_model_load_from_file(model_path, mp);
    if (!model) die("chargement du modele echoue");

    Ctx A = make_ctx(model, n_ctx, n_batch, n_ubatch);
    Ctx B = make_ctx(model, n_ctx, n_batch, n_ubatch);
    printf("HYBRID_CACHE_RESOLUTION=%s\n", (A.ctx && B.ctx) ? "PASS" : "FAIL");
    printf("KV_NATIVE_TYPE_K=%s\nKV_NATIVE_TYPE_V=%s\n",
           ggml_type_name(A.type_k), ggml_type_name(A.type_v));
    printf("N_EMBD_K_GQA=%d\nN_EMBD_V_GQA=%d\nK_ROW_BYTES=%zu\nV_ROW_BYTES=%zu\n",
           A.n_embd_k, A.n_embd_v, A.k_row, A.v_row);
    if (A.type_k != B.type_k || A.type_v != B.type_v) { printf("TYPES_DIVERGENTS=1\n"); return 4; }

    llama_batch bA = llama_batch_init(n_batch, 0, 1);
    llama_batch bB = llama_batch_init(n_batch, 0, 1);

    // --- prefill 0..57 dans A ET B (meme historique recurrent)
    const int N = 58;
    if (!decode_range(A.ctx, ids.data(), N, 0, bA, false)) { printf("PREFILL_A=FAIL\n"); return 5; }
    if (!decode_range(B.ctx, ids.data(), N, 0, bB, false)) { printf("PREFILL_B=FAIL\n"); return 5; }
    printf("PREFILL_0_57_A=PASS\nPREFILL_0_57_B=PASS\nRECURRENT_HISTORY_PREPARED_IN_BOTH_CONTEXTS=YES\n");

    // --- sonde : quels layer_idx le cache KV accepte-t-il ?
    const int NL = (int) llama_model_n_layer(model);
    std::vector<int> kv_layers;
    { std::vector<uint8_t> k(A.k_row, 0), v(A.v_row, 0);
      llama_pos p0 = 0;
      for (int li = 0; li < NL; ++li) {
          std::fill(k.begin(), k.end(), 0); std::fill(v.begin(), v.end(), 0);
          if (llama_kv_cache_extract_layer(A.ctx, 0, &p0, k.data(), v.data(), 1, li) == 0)
              kv_layers.push_back(li);
      } }
    printf("MODEL_LAYER_COUNT=%d\nATTENTION_LAYER_COUNT=%d\nATTENTION_LAYER_INDICES=[",
           NL, (int) kv_layers.size());
    for (size_t i = 0; i < kv_layers.size(); ++i) printf("%s%d", i ? "," : "", kv_layers[i]);
    printf("]\n");
    if (kv_layers.empty()) { printf("NO_KV_LAYER=1\n"); return 6; }

    // --- EXTRAIRE de A, INJECTER dans B, READBACK depuis B
    std::vector<llama_pos> pos(N); for (int i = 0; i < N; ++i) pos[i] = i;
    int acc = 0, rb = 0, layers_done = 0;
    size_t total_bytes = 0;
    std::string src_all, dst_all;
    for (int L : kv_layers) {
        std::vector<uint8_t> kA(N * A.k_row, 0), vA(N * A.v_row, 0);
        if (llama_kv_cache_extract_layer(A.ctx, 0, pos.data(), kA.data(), vA.data(), N, L) != 0) {
            printf("EXTRACT_FAIL_LAYER=%d\n", L); return 7; }
        const int rc = llama_kv_cache_inject_layer(B.ctx, 0, pos.data(), kA.data(), vA.data(), N, L);
        if (rc != 0) { printf("INJECT_FAIL_LAYER=%d RC=%d\n", L, rc); return 8; }
        acc++;
        std::vector<uint8_t> kB(N * B.k_row, 0), vB(N * B.v_row, 0);
        if (llama_kv_cache_extract_layer(B.ctx, 0, pos.data(), kB.data(), vB.data(), N, L) != 0) {
            printf("READBACK_FAIL_LAYER=%d\n", L); return 9; }
        const bool eq = (kA == kB) && (vA == vB);
        if (eq) rb++;
        printf("LAYER=%d INJECT_RC=%d READBACK_BYTE_EQUAL=%s K_BYTES=%zu V_BYTES=%zu\n",
               L, rc, eq ? "YES" : "NO", kA.size(), vA.size());
        src_all += sha_of(kA) + sha_of(vA);
        dst_all += sha_of(kB) + sha_of(vB);
        total_bytes += kA.size() + vA.size();
        layers_done++;
    }
    printf("LAYERS_INJECTED=%d\nLAYERS_READBACK_EQUAL=%d\nTOTAL_KV_BYTES=%zu\n",
           layers_done, rb, total_bytes);
    printf("KV_INJECTION_ACCEPTANCE=%s\n", (acc == layers_done) ? "PASS" : "FAIL");
    printf("KV_READBACK_BYTE_IDENTITY=%s\n", (rb == layers_done) ? "PASS" : "FAIL");
    if (rb != layers_done) { printf("STOP=READBACK_MISMATCH\n"); return 10; }

    // --- dernier token d'entree : ID 58 a la position 58, DANS A ET B
    if (!decode_range(A.ctx, ids.data() + N, 1, N, bA, true)) { printf("DECODE59_A=FAIL\n"); return 11; }
    if (!decode_range(B.ctx, ids.data() + N, 1, N, bB, true)) { printf("DECODE59_B=FAIL\n"); return 11; }
    printf("DECODE_59TH_A=PASS\nDECODE_59TH_B=PASS\n");

    // --- comparaison des logits de depart
    { const int nv = (int) llama_vocab_n_tokens(llama_model_get_vocab(model));
      const float * pa = llama_get_logits_ith(A.ctx, -1);
      const float * pb = llama_get_logits_ith(B.ctx, -1);
      if (pa && pb) {
          int ia = 0, ib = 0;
          for (int i = 1; i < nv; ++i) { if (pa[i] > pa[ia]) ia = i; if (pb[i] > pb[ib]) ib = i; }
          double d = 0; for (int i = 0; i < nv; ++i) d += (double)(pa[i]-pb[i])*(pa[i]-pb[i]);
          printf("START_LOGITS_ARGMAX_A=%d\nSTART_LOGITS_ARGMAX_B=%d\nSTART_LOGITS_L2_DIFF=%.9g\n", ia, ib, d);
      } else printf("START_LOGITS_UNAVAILABLE\n"); }

    // --- generation. UN SEUL appel par contexte : un appel mort prealable
    // DECODE un token parasite en position 59, et la reprise decode alors dans
    // une position DEJA OCCUPEE -- collision, meme classe que le defaut H1.
    const int nv2 = (int) llama_vocab_n_tokens(llama_model_get_vocab(model));
    Gen ga, gb;
    { const float * pa = llama_get_logits_ith(A.ctx, -1);
      int best = 0; if (pa) for (int i = 1; i < nv2; ++i) if (pa[i] > pa[best]) best = i;
      ga = generate(A, (llama_token) best, max_new); }
    { const int nv = (int) llama_vocab_n_tokens(llama_model_get_vocab(model));
      const float * pb = llama_get_logits_ith(B.ctx, -1);
      int best = 0; if (pb) for (int i = 1; i < nv; ++i) if (pb[i] > pb[best]) best = i;
      gb = generate(B, (llama_token) best, max_new); }

    auto emit = [](const char * tag, const Gen & g) {
        printf("%s_TOKEN_IDS=[", tag);
        for (size_t i = 0; i < g.ids.size(); ++i) printf("%s%d", i ? "," : "", g.ids[i]);
        printf("]\n%s_TOKEN_COUNT=%zu\n%s_STOP=%s\n", tag, g.ids.size(), tag, g.stop);
        printf("%s_TEXT=[%s]\n%s_TEXT_ESC=%s\n%s_TEXT_HEX=%s\n", tag, g.text.c_str(), tag, g.esc.c_str(), tag, g.hex.c_str());
    };
    emit("CONTROL", ga); emit("INJECTED", gb);
    printf("TOKEN_SEQUENCE_MATCH=%s\n", (ga.ids == gb.ids) ? "YES" : "NO");

    llama_batch_free(bA); llama_batch_free(bB);
    llama_free(A.ctx); llama_free(B.ctx); llama_model_free(model); llama_backend_free();
    return 0;
}
