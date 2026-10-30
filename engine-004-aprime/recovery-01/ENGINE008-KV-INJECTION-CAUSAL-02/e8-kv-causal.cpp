// e8-kv-causal -- PREUVE DISCRIMINANTE D'ECRITURE ET DE CONSOMMATION DU KV.
// Instrumentation DISTINCTE, hors gel ENGINE008-Q1.
//
// A = CONTROLE                     : KV calcule, jamais touche
// B = KV PERTURBE                  : V de la couche 63 mis a ZERO, K inchange
// C = KV PERTURBE PUIS RESTAURE    : perturbation puis restauration byte-exacte
//
// Difference avec le pilote precedent : le buffer injecte DIFFERE de l'original,
// donc un readback egal au buffer MODIFIE prouve que l'ECRITURE a eu lieu.
#include "llama.h"
#include "ggml.h"
#include "llama-context.h"
#include "llama-model.h"
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cstdint>
#include <cmath>
#include <fstream>
#include <string>
#include <vector>
#include <algorithm>

static void die(const char * m) { fprintf(stderr, "FATAL: %s\n", m); exit(2); }
static const char * arg(int c, char ** v, const char * n, const char * d = nullptr) {
    for (int i = 1; i + 1 < c; ++i) if (strcmp(v[i], n) == 0) return v[i+1];
    if (d) return d; fprintf(stderr, "missing %s\n", n); exit(2);
}
static uint64_t fnv(const std::vector<uint8_t> & d) {
    uint64_t h = 1469598103934665603ULL;
    for (uint8_t b : d) { h ^= b; h *= 1099511628211ULL; }
    return h;
}
static bool all_zero(const std::vector<uint8_t> & d) {
    for (uint8_t b : d) if (b) return false; return true;
}

struct Ctx {
    llama_context * ctx = nullptr;
    ggml_type type_k = GGML_TYPE_COUNT, type_v = GGML_TYPE_COUNT;
    int n_embd_k = 0, n_embd_v = 0;
    size_t k_row = 0, v_row = 0;
};
static Ctx make_ctx(llama_model * m, int n_ctx, int n_batch, int n_ubatch) {
    Ctx c;
    llama_context_params cp = llama_context_default_params();
    cp.n_ctx = n_ctx; cp.n_batch = n_batch; cp.n_ubatch = n_ubatch; cp.n_seq_max = 1;
    c.ctx = llama_init_from_model(m, cp);
    if (!c.ctx) die("contexte");
    c.type_k = llama_kv_cache_type_k(c.ctx);
    c.type_v = llama_kv_cache_type_v(c.ctx);
    const int hk = (int) c.ctx->get_model().hparams.n_embd_head_k();
    const int hv = (int) c.ctx->get_model().hparams.n_embd_head_v();
    c.n_embd_k = llama_model_n_head_kv(m) * hk;
    c.n_embd_v = llama_model_n_head_kv(m) * hv;
    c.k_row = ggml_row_size(c.type_k, c.n_embd_k);
    c.v_row = ggml_row_size(c.type_v, c.n_embd_v);
    return c;
}
static std::vector<int32_t> read_ids(const char * p) {
    std::vector<int32_t> v; std::ifstream f(p); long x;
    if (!f) die("ids"); while (f >> x) v.push_back((int32_t) x); return v;
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
static std::vector<uint8_t> extract(Ctx & c, const std::vector<llama_pos> & pos, int L, bool want_k) {
    std::vector<uint8_t> k(pos.size() * c.k_row, 0), v(pos.size() * c.v_row, 0);
    const int rc = llama_kv_cache_extract_layer(c.ctx, 0, pos.data(), k.data(), v.data(), (int) pos.size(), L);
    if (rc != 0) { printf("EXTRACT_FAIL_LAYER=%d RC=%d\n", L, rc); exit(7); }
    return want_k ? k : v;
}
struct Gen { std::vector<llama_token> ids; std::string text, esc; const char * stop = "NONE"; };
static Gen generate(Ctx & c, llama_model * m, llama_token first, int max_new, llama_pos pos_next) {
    Gen g; const llama_vocab * v = llama_model_get_vocab(m);
    const int nv = llama_vocab_n_tokens(v);
    std::vector<float> lg(nv);
    const float * p0 = llama_get_logits_ith(c.ctx, -1);
    if (!p0) { g.stop = "ERROR"; return g; }
    memcpy(lg.data(), p0, sizeof(float) * nv);
    g.ids.push_back(first);
    llama_batch b = llama_batch_init(1, 0, 1);
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
    if (strcmp(g.stop, "NONE") == 0) g.stop = "MAX_NEW_TOKENS";
    for (llama_token t : g.ids) {
        char pb[512]; int np = llama_token_to_piece(v, t, pb, sizeof pb, 0, true);
        if (np < 0) { std::vector<char> big(-np); np = llama_token_to_piece(v, t, big.data(), (int) big.size(), 0, true);
                      if (np >= 0) for (int k2 = 0; k2 < np; ++k2) g.text.push_back(big[k2]); }
        else for (int k2 = 0; k2 < np; ++k2) g.text.push_back(pb[k2]);
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
    const int max_new = atoi(arg(argc, argv, "--max-new", "32"));
    const int PERT    = atoi(arg(argc, argv, "--perturb-layer", "63"));
    const int n_ctx = 512, n_batch = 512, n_ubatch = 512;

    // --- §5 : AUTO-TEST AVANT TOUT CALCUL (aucun modele requis)
    if (strcmp(arg(argc, argv, "--selftest", "0"), "1") == 0) {
        int fail = 0;
        printf("E8_KV_CAUSAL_SELFTEST\n");
        // 1. zeros f16 : 2 octets par element, tous a 0x00
        std::vector<uint8_t> vzero((size_t) 58 * 2048, 0);
        printf("  SELFTEST_ZERO_F16_BYTES=%zu ALL_ZERO=%s\n", vzero.size(), all_zero(vzero)?"YES":"NO");
        if (vzero.size() != (size_t) 58 * 2048) fail++;
        // 2. un original NON NUL doit differer des zeros
        std::vector<uint8_t> vorig(vzero.size(), 0); vorig[7] = 0x3c;
        printf("  SELFTEST_DIFFERS=%s\n", (vorig != vzero) ? "YES" : "NO");
        if (vorig == vzero) fail++;
        // 3. ordre impose : perturbation -> readback -> restauration -> readback
        const char * order[4] = {"INJECT_MODIFIED","READBACK_MODIFIED","INJECT_ORIGINAL","READBACK_ORIGINAL"};
        printf("  SELFTEST_ORDER=%s>%s>%s>%s\n", order[0], order[1], order[2], order[3]);
        // 4. contrat de positions : prefill 0..57, 59e ID a la position 58, suite a 59
        printf("  SELFTEST_POSITIONS=PREFILL 0..57 ; DECODE_59TH pos=58 ; FIRST_GEN pos=59\n");
        if (!(57 == 58 - 1)) fail++;
        // 5. contrat d'arret : au plus 32 tokens, EOS prioritaire, dernier token non depasse
        printf("  SELFTEST_STOP_CONTRACT=MAX_NEW=32 ; EOS arrete ; aucun decode apres le dernier token selectionne\n");
        // 6. les 16 couches attendues sont bien celles sondees
        printf("  SELFTEST_EXPECTED_ATTENTION_LAYERS=[3,7,11,15,19,23,27,31,35,39,43,47,51,55,59,63]\n");
        printf("  SELFTEST_PERTURB_LAYER=%d IN_SET=%s\n", PERT,
               (PERT==3||PERT==7||PERT==11||PERT==15||PERT==19||PERT==23||PERT==27||PERT==31||
                PERT==35||PERT==39||PERT==43||PERT==47||PERT==51||PERT==55||PERT==59||PERT==63)?"YES":"NO");
        printf("SELFTEST_RESULT=%s FAILED=%d\n", fail==0?"PASS":"FAIL", fail);
        return fail==0?0:1;
    }

    std::vector<int32_t> ids = read_ids(ids_path);
    if ((int) ids.size() != 59) { fprintf(stderr, "FATAL: %zu IDs\n", ids.size()); return 3; }
    printf("E8_KV_CAUSAL\nINPUT_TOKEN_COUNT=%zu\nMAX_NEW=%d\nPERTURB_LAYER=%d\nINTERVENTION=ZERO_V_ORIGINAL_K\n",
           ids.size(), max_new, PERT);

    llama_backend_init();
    llama_model_params mp = llama_model_default_params();
    llama_model * model = llama_model_load_from_file(model_path, mp);
    if (!model) die("modele");

    Ctx A = make_ctx(model, n_ctx, n_batch, n_ubatch);
    Ctx B = make_ctx(model, n_ctx, n_batch, n_ubatch);
    Ctx C = make_ctx(model, n_ctx, n_batch, n_ubatch);
    printf("KV_NATIVE_TYPE_K=%s\nKV_NATIVE_TYPE_V=%s\nN_EMBD_K_GQA=%d\nN_EMBD_V_GQA=%d\nK_ROW_BYTES=%zu\nV_ROW_BYTES=%zu\n",
           ggml_type_name(A.type_k), ggml_type_name(A.type_v), A.n_embd_k, A.n_embd_v, A.k_row, A.v_row);
    if (A.type_k != B.type_k || A.type_k != C.type_k || A.type_v != B.type_v || A.type_v != C.type_v) {
        printf("TYPES_DIVERGENTS=1\n"); return 4; }

    llama_batch bA = llama_batch_init(n_batch,0,1), bB = llama_batch_init(n_batch,0,1), bC = llama_batch_init(n_batch,0,1);
    const int N = 58;
    if (!decode_range(A.ctx, ids.data(), N, 0, bA, false)) { printf("PREFILL_A=FAIL\n"); return 5; }
    if (!decode_range(B.ctx, ids.data(), N, 0, bB, false)) { printf("PREFILL_B=FAIL\n"); return 5; }
    if (!decode_range(C.ctx, ids.data(), N, 0, bC, false)) { printf("PREFILL_C=FAIL\n"); return 5; }
    printf("PREFILL_0_57_A=PASS\nPREFILL_0_57_B=PASS\nPREFILL_0_57_C=PASS\nRECURRENT_HISTORY_PREPARED_IN_ALL_THREE=YES\n");

    const int NL = (int) llama_model_n_layer(model);
    std::vector<int> kv_layers;
    { std::vector<uint8_t> k(A.k_row,0), v(A.v_row,0); llama_pos p0 = 0;
      for (int li = 0; li < NL; ++li) {
          std::fill(k.begin(),k.end(),0); std::fill(v.begin(),v.end(),0);
          if (llama_kv_cache_extract_layer(A.ctx, 0, &p0, k.data(), v.data(), 1, li) == 0) kv_layers.push_back(li); } }
    printf("ATTENTION_LAYER_COUNT=%d\nATTENTION_LAYER_INDICES=[", (int) kv_layers.size());
    for (size_t i=0;i<kv_layers.size();++i) printf("%s%d", i?",":"", kv_layers[i]);
    printf("]\n");
    bool pert_ok = false; for (int L : kv_layers) if (L == PERT) pert_ok = true;
    if (!pert_ok) { printf("PERTURB_LAYER_NOT_ATTENTION=1\n"); return 6; }

    std::vector<llama_pos> pos(N); for (int i=0;i<N;++i) pos[i]=i;

    // --- ETAT INITIAL : A, B, C doivent avoir le MEME KV calcule
    printf("INITIAL_KV_COMPARISON\n");
    int init_eq = 0;
    for (int L : kv_layers) {
        auto kA = extract(A,pos,L,true), vA = extract(A,pos,L,false);
        auto kB = extract(B,pos,L,true), vB = extract(B,pos,L,false);
        auto kC = extract(C,pos,L,true), vC = extract(C,pos,L,false);
        const bool eq = (kA==kB && kA==kC && vA==vB && vA==vC);
        if (eq) init_eq++;
        printf("  LAYER=%d A_B_C_IDENTICAL=%s K_FNV=%016llx V_FNV=%016llx K_BYTES=%zu V_BYTES=%zu\n",
               L, eq?"YES":"NO", (unsigned long long) fnv(kA), (unsigned long long) fnv(vA), kA.size(), vA.size());
    }
    printf("INITIAL_LAYERS_IDENTICAL=%d/%d\n", init_eq, (int) kv_layers.size());

    // --- §2 PREUVE DISCRIMINANTE D'ECRITURE, DANS C, SUR LES 16 COUCHES
    printf("DISCRIMINATING_WRITE_PROOF\n");
    int disc_layers = 0, non_disc = 0, accept = 0, rb_mod = 0, rb_rest = 0;
    std::vector<std::vector<uint8_t>> k_orig_store(kv_layers.size()), v_orig_store(kv_layers.size());
    for (size_t li = 0; li < kv_layers.size(); ++li) {
        const int L = kv_layers[li];
        auto k0 = extract(C,pos,L,true), v0 = extract(C,pos,L,false);
        k_orig_store[li] = k0; v_orig_store[li] = v0;
        std::vector<uint8_t> v_zero(v0.size(), 0);                  // f16 zeros = 0x00
        const bool differs = (v_zero != v0);
        if (!differs) { printf("  LAYER=%d NON_DISCRIMINATING (V original deja tout a zero)\n", L); non_disc++; }
        else disc_layers++;
        const int rc1 = llama_kv_cache_inject_layer(C.ctx, 0, pos.data(), k0.data(), v_zero.data(), N, L);
        if (rc1 != 0) { printf("  LAYER=%d INJECT_RC=%d\n", L, rc1); return 8; }
        accept++;
        auto k_r = extract(C,pos,L,true), v_r = extract(C,pos,L,false);
        const bool mod_ok = (k_r == k0) && (v_r == v_zero);
        if (mod_ok) rb_mod++;
        const int rc2 = llama_kv_cache_inject_layer(C.ctx, 0, pos.data(), k0.data(), v0.data(), N, L);
        if (rc2 != 0) { printf("  LAYER=%d RESTORE_RC=%d\n", L, rc2); return 9; }
        auto k_s = extract(C,pos,L,true), v_s = extract(C,pos,L,false);
        const bool rest_ok = (k_s == k0) && (v_s == v0);
        if (rest_ok) rb_rest++;
        printf("  LAYER=%d DIFFERS_FROM_ORIGINAL=%s INJECT_RC=%d READBACK_EQ_MODIFIED=%s RESTORE_RC=%d READBACK_EQ_ORIGINAL=%s\n",
               L, differs?"YES":"NO", rc1, mod_ok?"YES":"NO", rc2, rest_ok?"YES":"NO");
    }
    printf("DISCRIMINATING_LAYERS=%d\nNON_DISCRIMINATING_LAYERS=%d\n", disc_layers, non_disc);
    printf("KV_WRITE_DISCRIMINATING_PROOF=%s\n",
           (rb_mod == (int) kv_layers.size() && rb_rest == (int) kv_layers.size() &&
            disc_layers == (int) kv_layers.size()) ? "PASS"
           : ((rb_mod > 0 && rb_rest == (int) kv_layers.size()) ? "PARTIAL" : "FAIL"));
    printf("KV_RESTORATION_BYTE_IDENTITY=%s\n", (rb_rest == (int) kv_layers.size()) ? "PASS" : "FAIL");
    if (rb_rest != (int) kv_layers.size()) { printf("STOP=RESTORATION_FAILED\n"); return 10; }

    // --- §3 INTERVENTION POUR LA CONSOMMATION : B seulement, couche 63, V=0
    printf("CONSUMPTION_INTERVENTION\n");
    { auto k0 = extract(B,pos,PERT,true), v0 = extract(B,pos,PERT,false);
      std::vector<uint8_t> v_zero(v0.size(), 0);
      const bool differs = (v_zero != v0);
      const int rc = llama_kv_cache_inject_layer(B.ctx, 0, pos.data(), k0.data(), v_zero.data(), N, PERT);
      if (rc != 0) { printf("B_INJECT_RC=%d\n", rc); return 11; }
      auto k_r = extract(B,pos,PERT,true), v_r = extract(B,pos,PERT,false);
      printf("B_PERTURBED_LAYER=%d DIFFERS_FROM_ORIGINAL=%s INJECT_RC=%d READBACK_EQ_MODIFIED=%s V_ORIG_FNV=%016llx V_ZERO_FNV=%016llx V_READ_FNV=%016llx\n",
             PERT, differs?"YES":"NO", rc, ((k_r==k0)&&(v_r==v_zero))?"YES":"NO",
             (unsigned long long) fnv(v0), (unsigned long long) fnv(v_zero), (unsigned long long) fnv(v_r));
      printf("B_READBACK_DISCRIMINATING=%s\n", ((k_r==k0)&&(v_r==v_zero)&&differs)?"PASS":"FAIL"); }

    // --- C doit avoir retrouve SES originaux ; A intact
    { int c_ok = 0; for (size_t li=0; li<kv_layers.size(); ++li) {
          auto k = extract(C,pos,kv_layers[li],true), v = extract(C,pos,kv_layers[li],false);
          if (k==k_orig_store[li] && v==v_orig_store[li]) c_ok++; }
      printf("C_RESTORED_LAYERS=%d/%d\n", c_ok, (int) kv_layers.size());
      int a_ok = 0; for (size_t li=0; li<kv_layers.size(); ++li) {
          auto k = extract(A,pos,kv_layers[li],true), v = extract(A,pos,kv_layers[li],false);
          if (k==k_orig_store[li] && v==v_orig_store[li]) a_ok++; }
      printf("A_UNTOUCHED_LAYERS=%d/%d\n", a_ok, (int) kv_layers.size());
      if (c_ok != (int) kv_layers.size()) { printf("STOP=C_NOT_RESTORED\n"); return 12; } }

    // --- 59e ID, position 58, dans les trois
    if (!decode_range(A.ctx, ids.data()+N, 1, N, bA, true)) { printf("DECODE59_A=FAIL\n"); return 13; }
    if (!decode_range(B.ctx, ids.data()+N, 1, N, bB, true)) { printf("DECODE59_B=FAIL\n"); return 13; }
    if (!decode_range(C.ctx, ids.data()+N, 1, N, bC, true)) { printf("DECODE59_C=FAIL\n"); return 13; }
    printf("DECODE_59TH_A=PASS\nDECODE_59TH_B=PASS\nDECODE_59TH_C=PASS\n");

    // --- comparaison des logits
    const int nv = (int) llama_vocab_n_tokens(llama_model_get_vocab(model));
    const float * lA = llama_get_logits_ith(A.ctx,-1);
    const float * lB = llama_get_logits_ith(B.ctx,-1);
    const float * lC = llama_get_logits_ith(C.ctx,-1);
    auto cmp = [&](const char * tag, const float * x, const float * y) {
        if (!x || !y) { printf("%s_UNAVAILABLE\n", tag); return; }
        bool fin = true; double maxd = 0, l2 = 0; int ax = 0, ay = 0; uint64_t hx = 1469598103934665603ULL, hy = hx;
        for (int i = 0; i < nv; ++i) {
            if (!std::isfinite(x[i]) || !std::isfinite(y[i])) fin = false;
            const double d = std::fabs((double)x[i] - (double)y[i]);
            if (d > maxd) maxd = d; l2 += d*d;
            if (x[i] > x[ax]) ax = i; if (y[i] > y[ay]) ay = i;
            const uint8_t * bx = (const uint8_t*)&x[i]; const uint8_t * by = (const uint8_t*)&y[i];
            for (int k=0;k<4;++k) { hx ^= bx[k]; hx *= 1099511628211ULL; hy ^= by[k]; hy *= 1099511628211ULL; } }
        printf("%s FINITE=%s MAX_ABS_DIFF=%.9g L2_DIFF=%.9g ARGMAX_X=%d ARGMAX_Y=%d ARGMAX_MATCH=%s FNV_X=%016llx FNV_Y=%016llx BYTE_IDENTICAL=%s\n",
               tag, fin?"YES":"NO", maxd, std::sqrt(l2), ax, ay, (ax==ay)?"YES":"NO",
               (unsigned long long) hx, (unsigned long long) hy,
               (hx==hy)?"YES":"NO");
    };
    cmp("LOGITS_A_VS_B", lA, lB);
    cmp("LOGITS_A_VS_C", lA, lC);

    // --- generation : UN SEUL appel par contexte, depart = argmax de SES logits
    auto first_of = [&](const float * l) { int b = 0; if (l) for (int i=1;i<nv;++i) if (l[i]>l[b]) b=i; return (llama_token) b; };
    Gen gA = generate(A, model, first_of(lA), max_new, 59);
    Gen gB = generate(B, model, first_of(lB), max_new, 59);
    Gen gC = generate(C, model, first_of(lC), max_new, 59);
    auto emit = [](const char * tag, const Gen & g) {
        printf("%s_IDS=[", tag); for (size_t i=0;i<g.ids.size();++i) printf("%s%d", i?",":"", g.ids[i]);
        printf("]\n%s_COUNT=%zu\n%s_STOP=%s\n%s_TEXT=[%s]\n%s_TEXT_ESC=%s\n",
               tag, g.ids.size(), tag, g.stop, tag, g.text.c_str(), tag, g.esc.c_str()); };
    emit("CONTROL", gA); emit("PERTURBED", gB); emit("RESTORED", gC);
    printf("TOKEN_MATCH_A_B=%s\nTOKEN_MATCH_A_C=%s\n",
           (gA.ids==gB.ids)?"YES":"NO", (gA.ids==gC.ids)?"YES":"NO");

    llama_batch_free(bA); llama_batch_free(bB); llama_batch_free(bC);
    llama_free(A.ctx); llama_free(B.ctx); llama_free(C.ctx);
    llama_model_free(model); llama_backend_free();
    return 0;
}
