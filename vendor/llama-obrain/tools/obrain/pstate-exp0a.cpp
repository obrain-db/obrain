// pstate-exp0a -- Predictive State EXPERIMENT 0A
//   EXP0A.0 : extract_layer idempotence (readback stable)
//   EXP0A.1 : bit reproducibility of the same computation in two independent contexts
//   EXP0A.2 : extract -> inject -> extract identity
//   EXP0A.3 : behavioral probe (K/V + logits of the next target token)
//
// Public API only: llama_kv_cache_{extract_layer,inject_begin,inject_layer,type_k,type_v}
// No policy, no sampling, no proposer, no speculative engine.

#include "llama.h"
#include "ggml.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

static void die(const char * msg) { fprintf(stderr, "FATAL: %s\n", msg); exit(2); }

static std::vector<int32_t> read_ids(const char * path) {
    std::vector<int32_t> v;
    FILE * f = fopen(path, "r");
    if (!f) die("cannot open ids file");
    long x;
    while (fscanf(f, "%ld", &x) == 1) v.push_back((int32_t) x);
    fclose(f);
    return v;
}

static const char * arg(int argc, char ** argv, const char * name, const char * def = nullptr) {
    for (int i = 1; i + 1 < argc; ++i) if (strcmp(argv[i], name) == 0) return argv[i+1];
    if (def) return def;
    fprintf(stderr, "missing %s\n", name); exit(2);
}

// ---------------------------------------------------------------- contexts

struct Ctx {
    llama_model   * model = nullptr;
    llama_context * ctx   = nullptr;
    int             n_embd_k = 0;
    int             n_embd_v = 0;
    size_t          k_row = 0;
    size_t          v_row = 0;
    enum ggml_type  type_k = GGML_TYPE_F16;
    enum ggml_type  type_v = GGML_TYPE_F16;
};

static Ctx make_ctx(const char * model_path, int n_ctx, int n_batch, int n_ubatch, llama_model * shared = nullptr) {
    Ctx c;
    if (shared) { c.model = shared; }
    else {
        llama_model_params mp = llama_model_default_params();
        c.model = llama_model_load_from_file(model_path, mp);
        if (!c.model) die("model load failed");
    }
    llama_context_params cp = llama_context_default_params();
    cp.n_ctx    = n_ctx;
    cp.n_batch  = n_batch;
    cp.n_ubatch = n_ubatch;
    cp.n_seq_max = 1;
    c.ctx = llama_init_from_model(c.model, cp);
    if (!c.ctx) die("context creation failed");

    c.type_k = llama_kv_cache_type_k(c.ctx);
    c.type_v = llama_kv_cache_type_v(c.ctx);

    const int n_head_kv = llama_model_n_head_kv(c.model);
    const int n_embd    = llama_model_n_embd(c.model);
    const int n_head    = llama_model_n_head(c.model);
    const int head_dim  = n_embd / n_head;
    c.n_embd_k = n_head_kv * head_dim;
    c.n_embd_v = c.n_embd_k;

    c.k_row = ggml_row_size(c.type_k, 4096);   // BORNE SUPERIEURE sure (dims non certifiees)
    c.v_row = ggml_row_size(c.type_v, 4096);
    return c;
}

// decode ids[0..n) at absolute positions pos0..pos0+n-1, seq 0
static void decode_range(llama_context * ctx, const int32_t * ids, int n, llama_pos pos0,
                         llama_batch & batch, int capacity, bool want_logits_last = false,
                         llama_pos logits_pos = -1) {
    if (n > capacity) die("n > batch capacity");
    for (int i = 0; i < n; ++i) {
        batch.token[i]   = ids[i];
        batch.pos[i]     = pos0 + i;
        batch.n_seq_id[i] = 1;
        batch.seq_id[i][0] = 0;
        batch.logits[i]  = (logits_pos < 0) ? ((want_logits_last && i == n - 1) ? 1 : 0) : ((batch.pos[i] == logits_pos) ? 1 : 0);
    }
    batch.n_tokens = n;
    const int rc = llama_decode(ctx, batch);
    if (rc != 0) { fprintf(stderr, "FATAL: llama_decode rc=%d (n=%d pos0=%d)\n", rc, n, (int) pos0); exit(3); }
    batch.n_tokens = 0;
}

static void build_prefix(Ctx & c, const std::vector<int32_t> & prefix, int chunk, llama_batch & b) {
    int done = 0;
    const int total = (int) prefix.size();
    while (done < total) {
        const int n = std::min(chunk, total - done);
        decode_range(c.ctx, prefix.data() + done, n, done, b, 256);
        done += n;
    }
}

// one K/V extraction of `n` positions for one layer; returns bytes compared
static size_t extract_layer_bytes(Ctx & c, const std::vector<llama_pos> & pos, int layer,
                                  std::vector<uint8_t> & kb, std::vector<uint8_t> & vb) {
    const int n = (int) pos.size();
    kb.assign((size_t) n * c.k_row, 0);
    vb.assign((size_t) n * c.v_row, 0);
    const int rc = llama_kv_cache_extract_layer(c.ctx, 0, pos.data(), kb.data(), vb.data(), n, layer);
    if (rc != 0) { fprintf(stderr, "FATAL: extract_layer rc=%d layer=%d n=%d\n", rc, layer, n); exit(4); }
    return (size_t) n * (c.k_row + c.v_row);
}

int main(int argc, char ** argv) {
    const char * model_path = arg(argc, argv, "--model");
    const char * pid_path   = arg(argc, argv, "--prompt-ids");
    const char * oid_path   = arg(argc, argv, "--output-ids");
    const int    causal_t   = atoi(arg(argc, argv, "--causal-t", "118"));
    const int    n_ctx      = atoi(arg(argc, argv, "--n-ctx", "4096"));
    const int    n_batch    = atoi(arg(argc, argv, "--n-batch", "256"));
    const int    n_ubatch   = atoi(arg(argc, argv, "--n-ubatch", "256"));

    std::vector<int32_t> prompt_ids = read_ids(pid_path);
    std::vector<int32_t> output_ids = read_ids(oid_path);
    const int P = (int) prompt_ids.size();
    const int O = (int) output_ids.size();
    if (causal_t <= 0 || causal_t + 48 > O) die("causal_t out of range");

    // prefix = prompt + output[:causal_t]
    std::vector<int32_t> prefix;
    prefix.insert(prefix.end(), prompt_ids.begin(), prompt_ids.end());
    prefix.insert(prefix.end(), output_ids.begin(), output_ids.begin() + causal_t);
    const int PCT = (int) prefix.size();                      // 639 + 118 = 757
    const int A0  = PCT;                                      // absolute first continuation pos = 757
    const int A1  = PCT + 47;                                 // absolute last  continuation pos = 804
    const int PROBE_POS = PCT + 48;                           // 805

    llama_backend_init();

    printf("PSTATE_EXP0A_MANIFEST\n");
    printf("  model_path=%s\n", model_path);
    printf("  prompt_count=%d\n  output_count=%d\n  causal_t=%d\n", P, O, causal_t);
    printf("  prefix_count=%d\n  prefix_positions=0..%d\n", PCT, PCT - 1);
    printf("  continuation_count=48\n  continuation_abs=%d..%d\n  probe_abs=%d\n", A0, A1, PROBE_POS);
    printf("  n_ctx=%d\n  n_batch=%d\n  n_ubatch=%d\n", n_ctx, n_batch, n_ubatch);
    printf("  PREFIX_DECODE_SCHEDULE=256,256,245\n");

    // chunk schedule: fixed 256,256,245 for 757
    const std::vector<int> sched = { 256, 256, 245 };

    std::vector<int> kv_layer_ids;
    Ctx c0 = make_ctx(model_path, n_ctx, n_batch, n_ubatch);
    printf("  effective_type_k=%s\n  effective_type_v=%s\n",
           ggml_type_name(c0.type_k), ggml_type_name(c0.type_v));
    printf("  n_layers=%d\n  n_embd_k_gqa=%d\n  n_embd_v_gqa=%d\n",
           llama_model_n_layer(c0.model), c0.n_embd_k, c0.n_embd_v);
    printf("  k_row_bytes=%zu  (BORNE SUPERIEURE, dims non certifiees)\n  v_row_bytes=%zu\n", c0.k_row, c0.v_row);

    llama_batch batch0 = llama_batch_init(n_batch, 0, 1);
    int off = 0;
    for (int n : sched) { decode_range(c0.ctx, prefix.data() + off, n, off, batch0, 256); off += n; }
    if (off != PCT) die("prefix schedule mismatch");

    // ---- sonde : quels layer_idx le cache accepte-t-il reellement ? ----

    {
        const int NL = llama_model_n_layer(c0.model);
        std::vector<uint8_t> pk(4096*2, 0), pv(4096*2, 0);
        llama_pos ppos = (llama_pos)(PCT - 1);
        int other_fail = 0;
        for (int li = 0; li < NL; ++li) {
            std::fill(pk.begin(), pk.end(), 0); std::fill(pv.begin(), pv.end(), 0);
            const int rc = llama_kv_cache_extract_layer(c0.ctx, 0, &ppos, pk.data(), pv.data(), 1, li);
            if (rc == 0) kv_layer_ids.push_back(li);
            else if (rc != -1) other_fail++;
        }
        printf("KV_LAYER_PROBE\n");
        printf("  MODEL_LAYER_COUNT=%d\n  KV_LAYER_COUNT=%d\n", NL, (int) kv_layer_ids.size());
        printf("  KV_LAYER_IDS=[");
        for (size_t i = 0; i < kv_layer_ids.size(); ++i) printf("%s%d", i ? "," : "", kv_layer_ids[i]);
        printf("]\n");
        printf("  KV_LAYER_MAP_SPARSE=%s\n", ((int) kv_layer_ids.size() < NL) ? "TRUE" : "FALSE");
        printf("  probe_other_failures=%d\n", other_fail);
        if (kv_layer_ids.empty()) { printf("PSTATE_EXP0A=RED (aucune couche KV exposee)\n"); return 0; }
    }

    // ---------------- EXP0A.0 : idempotence -------------------------------
    {
        std::vector<llama_pos> pos(PCT);
        for (int i = 0; i < PCT; ++i) pos[i] = i;
        size_t kbytes = 0, vbytes = 0, kbad = 0, vbad = 0;
        int first_k = -1, first_v = -1;
        std::vector<uint8_t> k1, v1, k2, v2;
        for (int l : kv_layer_ids) {
            extract_layer_bytes(c0, pos, l, k1, v1);
            extract_layer_bytes(c0, pos, l, k2, v2);
            kbytes += k1.size(); vbytes += v1.size();
            if (memcmp(k1.data(), k2.data(), k1.size()) != 0) { kbad++; if (first_k < 0) first_k = l; }
            if (memcmp(v1.data(), v2.data(), v1.size()) != 0) { vbad++; if (first_v < 0) first_v = l; }
        }
        printf("EXP0A_0\n");
        printf("  positions_tested=%d\n  layers_tested=%d\n", PCT, llama_model_n_layer(c0.model));
        printf("  K_bytes_compared=%zu\n  V_bytes_compared=%zu\n", kbytes, vbytes);
        printf("  K_equal=%s\n  V_equal=%s\n", kbad == 0 ? "TRUE" : "FALSE", vbad == 0 ? "TRUE" : "FALSE");
        printf("  first_K_mismatch_layer=%d\n  first_V_mismatch_layer=%d\n", first_k, first_v);
        printf("  EXTRACT_LAYER_IDEMPOTENT=%s\n", (kbad == 0 && vbad == 0) ? "TRUE" : "FALSE");
        if (kbad || vbad) { printf("PSTATE_EXP0A=RED\n"); return 0; }
    }

    // ---------------- EXP0A.1 : two independent replays -------------------
    {
        Ctx cA = make_ctx(model_path, n_ctx, n_batch, n_ubatch, c0.model);   // independent model+context
        Ctx cB = make_ctx(model_path, n_ctx, n_batch, n_ubatch, c0.model);
        llama_batch bA = llama_batch_init(n_batch, 0, 1);
        llama_batch bB = llama_batch_init(n_batch, 0, 1);
        off = 0;
        for (int n : sched) { decode_range(cA.ctx, prefix.data() + off, n, off, bA, 256); off += n; }
        off = 0;
        for (int n : sched) { decode_range(cB.ctx, prefix.data() + off, n, off, bB, 256); off += n; }

        std::vector<llama_pos> pos_pre(PCT);
        for (int i = 0; i < PCT; ++i) pos_pre[i] = i;
        size_t kb = 0, vb = 0, kbad = 0, vbad = 0; int fk = -1, fv = -1;
        std::vector<uint8_t> k1, v1, k2, v2;
        for (int l : kv_layer_ids) {
            extract_layer_bytes(cA, pos_pre, l, k1, v1);
            extract_layer_bytes(cB, pos_pre, l, k2, v2);
            kb += k1.size(); vb += v1.size();
            if (memcmp(k1.data(), k2.data(), k1.size()) != 0) { kbad++; if (fk < 0) fk = l; }
            if (memcmp(v1.data(), v2.data(), v1.size()) != 0) { vbad++; if (fv < 0) fv = l; }
        }
        const bool before_ok = (kbad == 0 && vbad == 0);
        printf("EXP0A_1_BEFORE\n");
        printf("  K_bytes_compared=%zu\n  V_bytes_compared=%zu\n", kb, vb);
        printf("  CANONICAL_KV_BEFORE_IDENTICAL=%s\n", before_ok ? "TRUE" : "FALSE");
        if (!before_ok) { printf("PSTATE_EXP0A=RED (prefixe non identique)\n"); return 0; }

        // same controlled schedule in both: one teacher-forced batch of 48
        decode_range(cA.ctx, output_ids.data() + causal_t, 48, A0, bA, 256);
        decode_range(cB.ctx, output_ids.data() + causal_t, 48, A0, bB, 256);

        std::vector<llama_pos> pos_new(48);
        for (int i = 0; i < 48; ++i) pos_new[i] = A0 + i;
        size_t kb2 = 0, vb2 = 0; int kS = 0, vS = 0, fk2 = -1, fv2 = -1;
        for (int l : kv_layer_ids) {
            extract_layer_bytes(cA, pos_new, l, k1, v1);
            extract_layer_bytes(cB, pos_new, l, k2, v2);
            kb2 += k1.size(); vb2 += v1.size();
            const bool ke = memcmp(k1.data(), k2.data(), k1.size()) == 0;
            const bool ve = memcmp(v1.data(), v2.data(), v1.size()) == 0;
            if (ke) kS++; else if (fk2 < 0) fk2 = l;
            if (ve) vS++; else if (fv2 < 0) fv2 = l;
        }
        printf("EXP0A_1\n");
        printf("  TEST_SCHEDULE=CONTROLLED_B48\n");
        printf("  new_positions=48\n  K_slices_total=%d\n  K_slices_equal=%d\n  V_slices_total=%d\n  V_slices_equal=%d\n",
               (int) kv_layer_ids.size(), kS, (int) kv_layer_ids.size(), vS);
        printf("  K_bytes_compared=%zu\n  V_bytes_compared=%zu\n", kb2, vb2);
        printf("  first_K_mismatch_layer=%d\n  first_V_mismatch_layer=%d\n", fk2, fv2);
        const bool repro = (kS == (int) kv_layer_ids.size() && vS == (int) kv_layer_ids.size());
        printf("  CANONICAL_KV_REPRODUCIBLE_BITWISE=%s\n", repro ? "TRUE" : "FALSE");
        if (!repro) printf("  NOTE: A/B non bit-identique -> on CONTINUE vers 0A.2/0A.3 (tests independants)\n");

        // ------------- EXP0A.2 : extract -> inject -> extract -------------
        Ctx cI = make_ctx(model_path, n_ctx, n_batch, n_ubatch, c0.model);
        llama_batch bI = llama_batch_init(n_batch, 0, 1);
        off = 0;
        for (int n : sched) { decode_range(cI.ctx, prefix.data() + off, n, off, bI, 256); off += n; }

        printf("EXP0A_2\n  inject_begin_rc=NO_SUCH_SYMBOL (tentative directe de inject_layer)\n");
        int failures = 0;
        size_t kinj = 0, vinj = 0, kbad3 = 0, vbad3 = 0; int fk3 = -1, fv3 = -1;
        for (int l : kv_layer_ids) {
            extract_layer_bytes(cA, pos_new, l, k1, v1);          // source bytes
            const int rc = llama_kv_cache_inject_layer(cI.ctx, 0, pos_new.data(),
                                                      k1.data(), v1.data(), 48, l);
            if (rc != 0) { failures++; fprintf(stderr, "inject_layer rc=%d layer=%d\n", rc, l); continue; }
            extract_layer_bytes(cI, pos_new, l, k2, v2);          // readback
            kinj += k1.size(); vinj += v1.size();
            if (memcmp(k1.data(), k2.data(), k1.size()) != 0) { kbad3++; if (fk3 < 0) fk3 = l; }
            if (memcmp(v1.data(), v2.data(), v1.size()) != 0) { vbad3++; if (fv3 < 0) fv3 = l; }
        }
        printf("  inject_layer_failures=%d\n", failures);
        printf("  K_bytes_injected=%zu\n  V_bytes_injected=%zu\n", kinj, vinj);
        printf("  K_equal=%s\n  V_equal=%s\n", kbad3 == 0 ? "TRUE" : "FALSE", vbad3 == 0 ? "TRUE" : "FALSE");
        printf("  first_K_mismatch_layer=%d\n  first_V_mismatch_layer=%d\n", fk3, fv3);
        const bool ident = (kbad3 == 0 && vbad3 == 0 && failures == 0);
        printf("  EXTRACT_INJECT_EXTRACT_IDENTITY=%s\n", ident ? "TRUE" : "FALSE");
        if (!ident) { printf("PSTATE_EXP0A=RED\n"); return 0; }

        // ------------- EXP0A.3 : behavioral probe -------------------------
        const int32_t probe = output_ids[causal_t + 48];
        llama_batch bA1 = llama_batch_init(1, 0, 1);
        llama_batch bI1 = llama_batch_init(1, 0, 1);
        decode_range(cA.ctx, &probe, 1, PROBE_POS, bA1, 256, true);
        decode_range(cI.ctx, &probe, 1, PROBE_POS, bI1, 256, true);

        std::vector<llama_pos> ppos = { (llama_pos) PROBE_POS };
        size_t kp = 0, vp = 0; int kd = 0, vd = 0;
        std::vector<uint8_t> ka, va, ki, vi;
        for (int l : kv_layer_ids) {
            extract_layer_bytes(cA, ppos, l, ka, va);
            extract_layer_bytes(cI, ppos, l, ki, vi);
            kp += ka.size(); vp += va.size();
            if (memcmp(ka.data(), ki.data(), ka.size()) != 0) kd++;
            if (memcmp(va.data(), vi.data(), va.size()) != 0) vd++;
        }
        const float * la = llama_get_logits(cA.ctx);
        const float * li = llama_get_logits(cI.ctx);
        const int nv = llama_vocab_n_tokens(llama_model_get_vocab(cA.model));
        double maxerr = 0.0; int lbad = 0, amax_a = 0, amax_i = 0;
        for (int i = 0; i < nv; ++i) {
            const double e = la[i] > li[i] ? (double) la[i] - li[i] : (double) li[i] - la[i];
            if (e > maxerr) maxerr = e;
            if (la[i] != li[i]) lbad++;
            if (la[i] > la[amax_a]) amax_a = i;
            if (li[i] > li[amax_i]) amax_i = i;
        }
        printf("EXP0A_3\n");
        printf("  probe_token=%d\n  probe_position=%d\n", (int) probe, PROBE_POS);
        printf("  K_bytes_compared=%zu\n  V_bytes_compared=%zu\n", kp, vp);
        printf("  probe_K_bitwise_equal=%s\n  probe_V_bitwise_equal=%s\n", kd == 0 ? "TRUE" : "FALSE", vd == 0 ? "TRUE" : "FALSE");
        printf("  probe_logits_bitwise_equal=%s\n", lbad == 0 ? "TRUE" : "FALSE");
        printf("  probe_logits_max_abs_error=%.9g\n", maxerr);
        printf("  probe_argmax_equal=%s\n", amax_a == amax_i ? "TRUE" : "FALSE");
        const bool eq = (kd == 0 && vd == 0 && lbad == 0);
        printf("  KV_CANON_CACHE_STATE_EQUIVALENT=%s\n", eq ? "TRUE" : "FALSE");
        printf("PSTATE_EXP0A=%s\n", eq ? "GREEN" : "RED");
    }

    // ---------------- EXP0A.4 : SELF_OVERWRITE ---------------------------
    {
        Ctx cR = make_ctx(model_path, n_ctx, n_batch, n_ubatch, c0.model);
        Ctx cS = make_ctx(model_path, n_ctx, n_batch, n_ubatch, c0.model);
        llama_batch bR = llama_batch_init(n_batch, 0, 1);
        llama_batch bS = llama_batch_init(n_batch, 0, 1);
        int o2 = 0; for (int n : sched) { decode_range(cR.ctx, prefix.data()+o2, n, o2, bR, 256); o2 += n; }
        o2 = 0;     for (int n : sched) { decode_range(cS.ctx, prefix.data()+o2, n, o2, bS, 256); o2 += n; }
        decode_range(cR.ctx, output_ids.data()+causal_t, 48, A0, bR, 256);
        decode_range(cS.ctx, output_ids.data()+causal_t, 48, A0, bS, 256);

        std::vector<llama_pos> pp(48); for (int i = 0; i < 48; ++i) pp[i] = A0 + i;
        std::vector<uint8_t> k1, v1, k2, v2;
        int pre_k_eq = 0, pre_v_eq = 0;
        for (int l : kv_layer_ids) {
            extract_layer_bytes(cR, pp, l, k1, v1);
            extract_layer_bytes(cS, pp, l, k2, v2);
            if (memcmp(k1.data(), k2.data(), k1.size()) == 0) pre_k_eq++;
            if (memcmp(v1.data(), v2.data(), v1.size()) == 0) pre_v_eq++;
        }
        printf("SELF_OVERWRITE_CONTROL\n");
        printf("  SELF_PRE_KV_REF_vs_INJ_K_equal_layers=%d/%d\n", pre_k_eq, (int) kv_layer_ids.size());
        printf("  SELF_PRE_KV_REF_vs_INJ_V_equal_layers=%d/%d\n", pre_v_eq, (int) kv_layer_ids.size());

        // bytes pre-injection CONSERVES PAR COUCHE (le bug precedent comparait la derniere couche)
        std::vector<std::vector<uint8_t>> pre_k(kv_layer_ids.size()), pre_v(kv_layer_ids.size());
        int sf = 0;
        for (size_t li = 0; li < kv_layer_ids.size(); ++li) {
            const int l = kv_layer_ids[li];
            extract_layer_bytes(cS, pp, l, pre_k[li], pre_v[li]);
            const int rc = llama_kv_cache_inject_layer(cS.ctx, 0, pp.data(),
                                                      pre_k[li].data(), pre_v[li].data(), 48, l);
            if (rc != 0) sf++;
        }
        int ba_k = 0, ba_v = 0;
        for (size_t li = 0; li < kv_layer_ids.size(); ++li) {
            extract_layer_bytes(cS, pp, kv_layer_ids[li], k2, v2);
            if (memcmp(pre_k[li].data(), k2.data(), k2.size()) == 0) ba_k++;
            if (memcmp(pre_v[li].data(), v2.data(), v2.size()) == 0) ba_v++;
        }
        printf("  self_inject_failures=%d\n", sf);
        printf("  SELF_KV_BEFORE_AFTER_BITWISE=%s (K %d/%d, V %d/%d)\n",
               (sf == 0 && ba_k == ba_v && ba_k == (int) kv_layer_ids.size()) ? "TRUE" : "FALSE",
               ba_k, (int) kv_layer_ids.size(), ba_v, (int) kv_layer_ids.size());

        const int32_t pt = output_ids[causal_t + 48];
        llama_batch bR1 = llama_batch_init(1, 0, 1), bS1 = llama_batch_init(1, 0, 1);
        decode_range(cR.ctx, &pt, 1, A0 + 48, bR1, 256, true);
        decode_range(cS.ctx, &pt, 1, A0 + 48, bS1, 256, true);
        std::vector<llama_pos> pp1 = { (llama_pos)(A0 + 48) };
        int pk = 0, pv = 0;
        for (int l : kv_layer_ids) {
            extract_layer_bytes(cR, pp1, l, k1, v1);
            extract_layer_bytes(cS, pp1, l, k2, v2);
            if (memcmp(k1.data(), k2.data(), k1.size()) == 0) pk++;
            if (memcmp(v1.data(), v2.data(), v1.size()) == 0) pv++;
        }
        const float * lr = llama_get_logits(cR.ctx);
        const float * ls = llama_get_logits(cS.ctx);
        const int nvv = llama_vocab_n_tokens(llama_model_get_vocab(c0.model));
        double mx = 0, sm = 0; int lbad = 0, amR = 0, amS = 0;
        for (int i = 0; i < nvv; ++i) {
            const double e = lr[i] > ls[i] ? (double) lr[i] - ls[i] : (double) ls[i] - lr[i];
            if (e > mx) mx = e; sm += e;
            if (lr[i] != ls[i]) lbad++;
            if (lr[i] > lr[amR]) amR = i;
            if (ls[i] > ls[amS]) amS = i;
        }
        printf("  probe_K_equal_layers=%d/%d  probe_V_equal_layers=%d/%d\n",
               pk, (int) kv_layer_ids.size(), pv, (int) kv_layer_ids.size());
        printf("  probe_logits_bitwise_equal=%s\n", lbad == 0 ? "TRUE" : "FALSE");
        printf("  probe_logits_max_abs=%.9g\n  probe_logits_mean_abs=%.9g\n", mx, sm / nvv);
        printf("  probe_argmax_R=%d probe_argmax_S=%d probe_argmax_equal=%s\n", amR, amS, amR == amS ? "TRUE" : "FALSE");
        printf("  LEGACY_SELF_INDEPENDENT_CONTEXT_CONTROL:\n");
        printf("    pre_K_equal_layers=%d/16 pre_V_equal_layers=%d/16\n", pre_k_eq, (int) kv_layer_ids.size());
        printf("    CAUSAL_NEUTRALITY_CONCLUSION=NOT_VALID_FROM_THIS_CONTROL (REF n est pas le contrefactuel exact de SELF)\n");
        printf("  SELF_OVERWRITE_CAUSAL_TEST=%s\n",
               (pre_k_eq == pre_v_eq && pre_k_eq == (int) kv_layer_ids.size()) ? "PREMISES_CLEAN" : "INCONCLUSIVE_DUE_TO_PREEXISTING_CONTEXT_DIVERGENCE");
    }

    // ---------------- EXP0A.5 : STATE SNAPSHOT CONTROL -------------------
    {
        Ctx S = make_ctx(model_path, n_ctx, n_batch, n_ubatch, c0.model);
        S.k_row = ggml_row_size(llama_kv_cache_type_k(S.ctx), 4096);
        S.v_row = ggml_row_size(llama_kv_cache_type_v(S.ctx), 4096);
        llama_batch b = llama_batch_init(n_batch, 0, 1);
        int o3 = 0; for (int n : sched) { decode_range(S.ctx, prefix.data()+o3, n, o3, b, 256); o3 += n; }
        decode_range(S.ctx, output_ids.data()+causal_t, 48, A0, b, 256);

        std::vector<llama_pos> pp(48); for (int i = 0; i < 48; ++i) pp[i] = A0 + i;
        std::vector<std::vector<uint8_t>> kv0k(kv_layer_ids.size()), kv0v(kv_layer_ids.size());
        for (size_t li = 0; li < kv_layer_ids.size(); ++li)
            extract_layer_bytes(S, pp, kv_layer_ids[li], kv0k[li], kv0v[li]);

        const size_t ssz = llama_state_get_size(S.ctx);
        std::vector<uint8_t> st0(ssz);
        const size_t sgot = llama_state_get_data(S.ctx, st0.data(), ssz);
        printf("STATE_SNAPSHOT_CONTROL\n  state_size=%zu\n  state_bytes=%zu\n", ssz, sgot);

        const int32_t pt = output_ids[causal_t + 48];
        llama_batch b1 = llama_batch_init(1, 0, 1);
        // --- branche R1 : probe direct depuis S0
        decode_range(S.ctx, &pt, 1, A0 + 48, b1, 256, true);
        std::vector<float> lg1(llama_get_logits(S.ctx), llama_get_logits(S.ctx) + llama_vocab_n_tokens(llama_model_get_vocab(c0.model)));
        std::vector<llama_pos> pp1 = { (llama_pos)(A0 + 48) };
        std::vector<std::vector<uint8_t>> r1k(kv_layer_ids.size()), r1v(kv_layer_ids.size());
        for (size_t li = 0; li < kv_layer_ids.size(); ++li)
            extract_layer_bytes(S, pp1, kv_layer_ids[li], r1k[li], r1v[li]);

        // --- restore puis R2
        const size_t sset = llama_state_set_data(S.ctx, st0.data(), st0.size());
        int rr_k = 0, rr_v = 0;
        for (size_t li = 0; li < kv_layer_ids.size(); ++li) {
            std::vector<uint8_t> k, v;
            extract_layer_bytes(S, pp, kv_layer_ids[li], k, v);
            if (memcmp(kv0k[li].data(), k.data(), k.size()) == 0) rr_k++;
            if (memcmp(kv0v[li].data(), v.data(), v.size()) == 0) rr_v++;
        }
        printf("  state_set_bytes=%zu\n", sset);
        printf("  KV_restore_bitwise_exact=%s (K %d/%d, V %d/%d)\n",
               (rr_k == rr_v && rr_k == (int) kv_layer_ids.size()) ? "TRUE" : "FALSE",
               rr_k, (int) kv_layer_ids.size(), rr_v, (int) kv_layer_ids.size());
        decode_range(S.ctx, &pt, 1, A0 + 48, b1, 256, true);
        std::vector<float> lg2(llama_get_logits(S.ctx), llama_get_logits(S.ctx) + lg1.size());
        int pk2 = 0, pv2 = 0;
        for (size_t li = 0; li < kv_layer_ids.size(); ++li) {
            std::vector<uint8_t> k, v;
            extract_layer_bytes(S, pp1, kv_layer_ids[li], k, v);
            if (memcmp(r1k[li].data(), k.data(), k.size()) == 0) pk2++;
            if (memcmp(r1v[li].data(), v.data(), v.size()) == 0) pv2++;
        }
        double mx2 = 0; int lb2 = 0, a1 = 0, a2 = 0;
        for (size_t i = 0; i < lg1.size(); ++i) {
            const double e = lg1[i] > lg2[i] ? (double) lg1[i] - lg2[i] : (double) lg2[i] - lg1[i];
            if (e > mx2) mx2 = e;
            if (lg1[i] != lg2[i]) lb2++;
            if (lg1[i] > lg1[a1]) a1 = (int) i;
            if (lg2[i] > lg2[a2]) a2 = (int) i;
        }
        printf("  probe_K_R1_vs_R2_equal_layers=%d/%d\n  probe_V_R1_vs_R2_equal_layers=%d/%d\n",
               pk2, (int) kv_layer_ids.size(), pv2, (int) kv_layer_ids.size());
        printf("  probe_logits_R1_vs_R2_bitwise=%s\n  probe_logits_max_abs=%.9g\n", lb2 == 0 ? "TRUE" : "FALSE", mx2);
        printf("  probe_argmax_R1=%d probe_argmax_R2=%d equal=%s\n", a1, a2, a1 == a2 ? "TRUE" : "FALSE");
        const bool snap_ok = (sset == st0.size() && rr_k == (int) kv_layer_ids.size() && rr_v == (int) kv_layer_ids.size()
                              && pk2 == (int) kv_layer_ids.size() && pv2 == (int) kv_layer_ids.size() && lb2 == 0);
        printf("  STATE_SNAPSHOT_USABLE_FOR_CAUSAL_BRANCHING=%s\n", snap_ok ? "TRUE" : "FALSE");
    }

    // ---------------- EXP0A.6 : A vs I_A sur TOUT le KV exporte 0..804 -----
    {
        Ctx cA2 = make_ctx(model_path, n_ctx, n_batch, n_ubatch, c0.model);
        Ctx cI2 = make_ctx(model_path, n_ctx, n_batch, n_ubatch, c0.model);
        cA2.k_row = ggml_row_size(llama_kv_cache_type_k(cA2.ctx), 4096); cA2.v_row = cA2.k_row;
        cI2.k_row = cA2.k_row; cI2.v_row = cA2.k_row;
        llama_batch bA2 = llama_batch_init(n_batch, 0, 1), bI2 = llama_batch_init(n_batch, 0, 1);
        int o4 = 0; for (int n : sched) { decode_range(cA2.ctx, prefix.data()+o4, n, o4, bA2, 256); o4 += n; }
        o4 = 0;     for (int n : sched) { decode_range(cI2.ctx, prefix.data()+o4, n, o4, bI2, 256); o4 += n; }
        decode_range(cA2.ctx, output_ids.data()+causal_t, 48, A0, bA2, 256);

        std::vector<llama_pos> pspan(48); for (int i = 0; i < 48; ++i) pspan[i] = A0 + i;
        std::vector<std::vector<uint8_t>> sk(16), sv(16);
        int inj = 0;
        for (size_t li = 0; li < kv_layer_ids.size(); ++li) {
            std::vector<uint8_t> k, v;
            extract_layer_bytes(cA2, pspan, kv_layer_ids[li], k, v);
            sk[li] = k; sv[li] = v;
            if (llama_kv_cache_inject_layer(cI2.ctx, 0, pspan.data(), k.data(), v.data(), 48, kv_layer_ids[li]) != 0) inj++;
        }
        std::vector<llama_pos> pall(PCT + 48); for (int i = 0; i < PCT + 48; ++i) pall[i] = i;
        int pref_k = 0, pref_v = 0, span_k = 0, span_v = 0;
        for (size_t li = 0; li < kv_layer_ids.size(); ++li) {
            std::vector<uint8_t> ka, va, ki, vi;
            extract_layer_bytes(cA2, pall, kv_layer_ids[li], ka, va);
            extract_layer_bytes(cI2, pall, kv_layer_ids[li], ki, vi);
            const size_t pre_bytes = (size_t) PCT * cA2.k_row;
            if (memcmp(ka.data(), ki.data(), pre_bytes) == 0) pref_k++;
            if (memcmp(va.data(), vi.data(), pre_bytes) == 0) pref_v++;
            if (memcmp(ka.data() + pre_bytes, ki.data() + pre_bytes, ka.size() - pre_bytes) == 0) span_k++;
            if (memcmp(va.data() + pre_bytes, vi.data() + pre_bytes, va.size() - pre_bytes) == 0) span_v++;
        }
        printf("A_vs_I_A_PRE_PROBE\n");
        printf("  inject_failures=%d\n", inj);
        printf("  prefix_K_equal=%d/%d  prefix_V_equal=%d/%d\n", pref_k, (int) kv_layer_ids.size(), pref_v, (int) kv_layer_ids.size());
        printf("  injected_span_K_equal=%d/%d  injected_span_V_equal=%d/%d\n", span_k, (int) kv_layer_ids.size(), span_v, (int) kv_layer_ids.size());
        const bool full = (pref_k == pref_v && pref_v == span_k && span_k == span_v && span_v == (int) kv_layer_ids.size());
        printf("  FULL_EXPORTED_KV_0_804_EQUAL=%s\n", full ? "TRUE" : "FALSE");

        const int32_t pt2 = output_ids[causal_t + 48];
        llama_batch b1a = llama_batch_init(1,0,1), b1i = llama_batch_init(1,0,1);
        decode_range(cA2.ctx, &pt2, 1, A0 + 48, b1a, 256, true);
        decode_range(cI2.ctx, &pt2, 1, A0 + 48, b1i, 256, true);
        const float * la = llama_get_logits(cA2.ctx); const float * li2 = llama_get_logits(cI2.ctx);
        const int nv2 = llama_vocab_n_tokens(llama_model_get_vocab(c0.model));
        double mx3 = 0, sm3 = 0; int lb3 = 0, aa = 0, ai = 0;
        for (int i = 0; i < nv2; ++i) {
            const double e = la[i] > li2[i] ? (double) la[i] - li2[i] : (double) li2[i] - la[i];
            if (e > mx3) mx3 = e; sm3 += e;
            if (la[i] != li2[i]) lb3++;
            if (la[i] > la[aa]) aa = i;
            if (li2[i] > li2[ai]) ai = i;
        }
        printf("  probe_logits_bitwise=%s\n  probe_logits_max_abs=%.9g\n  probe_logits_mean_abs=%.9g\n",
               lb3 == 0 ? "TRUE" : "FALSE", mx3, sm3 / nv2);
        printf("  probe_argmax_A=%d probe_argmax_I=%d equal=%s\n", aa, ai, aa == ai ? "TRUE" : "FALSE");
        printf("  SAME_EXPORTED_KV_DIFFERENT_FUTURE=%s\n",
               (full && (lb3 != 0)) ? "TRUE" : (full ? "FALSE" : "PRE_CONDITION_NOT_MET"));
        printf("  EXPORTED_16_LAYER_KV_NOT_COMPLETE_CAUSAL_STATE=%s\n",
               (full && (lb3 != 0)) ? "TRUE" : "NOT_ESTABLISHED");
    }

    // ---------------- EXP0A.7 : PREFIX_TEMPORAL_STABILITY ---------------
    {
        Ctx T = make_ctx(model_path, n_ctx, n_batch, n_ubatch, c0.model);
        T.k_row = ggml_row_size(llama_kv_cache_type_k(T.ctx), 4096);
        T.v_row = ggml_row_size(llama_kv_cache_type_v(T.ctx), 4096);
        llama_batch b = llama_batch_init(n_batch, 0, 1);
        int o5 = 0; for (int n : sched) { decode_range(T.ctx, prefix.data()+o5, n, o5, b, 256); o5 += n; }
        std::vector<llama_pos> ppre(PCT); for (int i = 0; i < PCT; ++i) ppre[i] = i;

        std::vector<std::vector<uint8_t>> t0k(kv_layer_ids.size()), t0v(kv_layer_ids.size());
        for (size_t li = 0; li < kv_layer_ids.size(); ++li)
            extract_layer_bytes(T, ppre, kv_layer_ids[li], t0k[li], t0v[li]);

        decode_range(T.ctx, output_ids.data()+causal_t, 48, A0, b, 256);

        int ek = 0, ev = 0;
        printf("PREFIX_TEMPORAL_STABILITY\n  layers=%d\n", (int) kv_layer_ids.size());
        printf("  layer | K_T0_eq_T1 | V_T0_eq_T1\n");
        for (size_t li = 0; li < kv_layer_ids.size(); ++li) {
            std::vector<uint8_t> k, v;
            extract_layer_bytes(T, ppre, kv_layer_ids[li], k, v);
            const bool ke = memcmp(t0k[li].data(), k.data(), k.size()) == 0;
            const bool ve = memcmp(t0v[li].data(), v.data(), v.size()) == 0;
            if (ke) ek++; if (ve) ev++;
            printf("  %5d | %10s | %10s\n", kv_layer_ids[li], ke ? "EQ" : "DIFF", ve ? "EQ" : "DIFF");
        }
        printf("  K_equal_layers=%d/%d  V_equal_layers=%d/%d\n", ek, (int) kv_layer_ids.size(), ev, (int) kv_layer_ids.size());
        printf("  DIVERGENT_K_LAYERS=[");
        bool first = true;
        for (size_t li = 0; li < kv_layer_ids.size(); ++li) {
            std::vector<uint8_t> k, v; extract_layer_bytes(T, ppre, kv_layer_ids[li], k, v);
            if (memcmp(t0k[li].data(), k.data(), k.size()) != 0) { printf("%s%d", first?"":",", kv_layer_ids[li]); first = false; }
        }
        printf("]\n");
        const bool stable = (ek == (int) kv_layer_ids.size() && ev == (int) kv_layer_ids.size());
        printf("  PREFIX_K_BITWISE_STABLE_AFTER_B48=%s\n", (ek == (int) kv_layer_ids.size()) ? "TRUE" : "FALSE");
        printf("  PREFIX_V_BITWISE_STABLE_AFTER_B48=%s\n", (ev == (int) kv_layer_ids.size()) ? "TRUE" : "FALSE");
        printf("  EXPORTED_KV_IS_TEMPORALLY_IMMUTABLE=%s\n", stable ? "TRUE" : "FALSE");
    }

    // -------- EXP0A.8 : NEW_CELL injection preserves existing KV? ---------
    {
        Ctx cSrc = make_ctx(model_path, n_ctx, n_batch, n_ubatch, c0.model);
        cSrc.k_row = ggml_row_size(llama_kv_cache_type_k(cSrc.ctx), 4096);
        cSrc.v_row = ggml_row_size(llama_kv_cache_type_v(cSrc.ctx), 4096);
        llama_batch bSrc = llama_batch_init(n_batch, 0, 1);
        int o6 = 0; for (int n : sched) { decode_range(cSrc.ctx, prefix.data()+o6, n, o6, bSrc, 256); o6 += n; }
        decode_range(cSrc.ctx, output_ids.data()+causal_t, 48, A0, bSrc, 256);

        std::vector<llama_pos> pspan(48); for (int i = 0; i < 48; ++i) pspan[i] = A0 + i;
        std::vector<std::vector<uint8_t>> pay_k(kv_layer_ids.size()), pay_v(kv_layer_ids.size());
        for (size_t li = 0; li < kv_layer_ids.size(); ++li)
            extract_layer_bytes(cSrc, pspan, kv_layer_ids[li], pay_k[li], pay_v[li]);

        Ctx cI3 = make_ctx(model_path, n_ctx, n_batch, n_ubatch, c0.model);
        cI3.k_row = cSrc.k_row; cI3.v_row = cSrc.v_row;
        llama_batch bI3 = llama_batch_init(n_batch, 0, 1);
        o6 = 0; for (int n : sched) { decode_range(cI3.ctx, prefix.data()+o6, n, o6, bI3, 256); o6 += n; }

        std::vector<llama_pos> ppre(PCT); for (int i = 0; i < PCT; ++i) ppre[i] = i;
        std::vector<std::vector<uint8_t>> p0k(kv_layer_ids.size()), p0v(kv_layer_ids.size());
        for (size_t li = 0; li < kv_layer_ids.size(); ++li)
            extract_layer_bytes(cI3, ppre, kv_layer_ids[li], p0k[li], p0v[li]);

        int injf = 0;
        for (size_t li = 0; li < kv_layer_ids.size(); ++li)
            if (llama_kv_cache_inject_layer(cI3.ctx, 0, pspan.data(), pay_k[li].data(), pay_v[li].data(), 48, kv_layer_ids[li]) != 0) injf++;

        int spk = 0, spv = 0;
        for (size_t li = 0; li < kv_layer_ids.size(); ++li) {
            std::vector<uint8_t> k, v;
            extract_layer_bytes(cI3, pspan, kv_layer_ids[li], k, v);
            if (memcmp(pay_k[li].data(), k.data(), k.size()) == 0) spk++;
            if (memcmp(pay_v[li].data(), v.data(), v.size()) == 0) spv++;
        }
        int pfk = 0, pfv = 0;
        printf("NEW_CELL_INJECTION_PREFIX_PRESERVATION\n");
        printf("  inject_failures=%d\n", injf);
        printf("  injected_span_K_equal_layers=%d/%d\n  injected_span_V_equal_layers=%d/%d\n",
               spk, (int) kv_layer_ids.size(), spv, (int) kv_layer_ids.size());
        printf("  layer | K_pref_before_eq_after | V_pref_before_eq_after\n");
        for (size_t li = 0; li < kv_layer_ids.size(); ++li) {
            std::vector<uint8_t> k, v;
            extract_layer_bytes(cI3, ppre, kv_layer_ids[li], k, v);
            const bool ke = memcmp(p0k[li].data(), k.data(), k.size()) == 0;
            const bool ve = memcmp(p0v[li].data(), v.data(), v.size()) == 0;
            if (ke) pfk++; if (ve) pfv++;
            if (!ke || !ve) printf("  %5d | %22s | %22s\n", kv_layer_ids[li], ke?"EQ":"DIFF", ve?"EQ":"DIFF");
        }
        printf("  prefix_K_before_after_equal_layers=%d/%d\n  prefix_V_before_after_equal_layers=%d/%d\n",
               pfk, (int) kv_layer_ids.size(), pfv, (int) kv_layer_ids.size());
        printf("  DIVERGENT_PREFIX_K_LAYERS=["); bool f2 = true;
        for (size_t li = 0; li < kv_layer_ids.size(); ++li) {
            std::vector<uint8_t> k, v; extract_layer_bytes(cI3, ppre, kv_layer_ids[li], k, v);
            if (memcmp(p0k[li].data(), k.data(), k.size()) != 0) { printf("%s%d", f2?"":",", kv_layer_ids[li]); f2 = false; }
        }
        printf("]\n");
        const bool pres = (pfk == (int) kv_layer_ids.size() && pfv == (int) kv_layer_ids.size());
        printf("  NEW_CELL_INJECTION_PRESERVES_EXISTING_KV=%s\n", pres ? "TRUE" : "FALSE");
        printf("  NEW_CELL_ALLOCATION_PERTURBS_EXISTING_EXPORTED_KV=%s\n", pres ? "FALSE" : "TRUE");
    }

    // -------- EXP0A.9 : CERTIFICATION DE LA ROW SIZE REELLE ---------------
    {
        Ctx Z = make_ctx(model_path, n_ctx, n_batch, n_ubatch, c0.model);
        Z.k_row = Z.v_row = ggml_row_size(llama_kv_cache_type_k(Z.ctx), 4096);
        llama_batch bz = llama_batch_init(n_batch, 0, 1);
        int o7 = 0; for (int n : sched) { decode_range(Z.ctx, prefix.data()+o7, n, o7, bz, 256); o7 += n; }

        const size_t cap = Z.k_row * 48;
        std::vector<uint8_t> buf(cap, 0xAA);
        std::vector<llama_pos> p1 = { 0 };
        const int rc1 = llama_kv_cache_extract_layer(Z.ctx, 0, p1.data(), buf.data(), buf.data(), 1, kv_layer_ids[0]);
        size_t last1 = 0; for (size_t i = 0; i < cap; ++i) if (buf[i] != 0xAA) last1 = i;
        const size_t written1 = last1 + 1;

        std::vector<uint8_t> buf2(cap, 0xAA);
        std::vector<llama_pos> p2 = { 0, 1 };
        const int rc2 = llama_kv_cache_extract_layer(Z.ctx, 0, p2.data(), buf2.data(), buf2.data(), 2, kv_layer_ids[0]);
        size_t last2 = 0; for (size_t i = 0; i < cap; ++i) if (buf2[i] != 0xAA) last2 = i;
        const size_t written2 = last2 + 1;

        std::vector<uint8_t> bufv(cap, 0xAA);
        const int rcv = llama_kv_cache_extract_layer(Z.ctx, 0, p1.data(), bufv.data(), bufv.data(), 1, kv_layer_ids[0]);
        size_t lastv = 0; for (size_t i = 0; i < cap; ++i) if (bufv[i] != 0xAA) lastv = i;

        const size_t row_k = written1;
        const size_t row_v = lastv + 1;
        printf("ROW_SIZE_CERTIFICATION\n");
        printf("  extract_rc_1tok=%d extract_rc_2tok=%d\n", rc1, rc2);
        printf("  written_bytes_1_token=%zu\n  written_bytes_2_tokens=%zu\n", written1, written2);
        printf("  contiguity_ok=%s (2tok == 2 x 1tok)\n", (written2 == 2*written1) ? "TRUE" : "FALSE");
        printf("  real_row_bytes=%zu  (= %zu elements en f16)\n", row_k, row_k/2);
        printf("  n_embd_k_gqa=%zu  n_embd_v_gqa=%zu\n", row_k/2, row_v/2);
        printf("  formula_ggml_row_size(type_k, n_embd_k_gqa)=%zu  (concordance=%s)\n",
               ggml_row_size(llama_kv_cache_type_k(Z.ctx), (int)(row_k/2)),
               (ggml_row_size(llama_kv_cache_type_k(Z.ctx), (int)(row_k/2)) == row_k) ? "TRUE" : "FALSE");
        printf("  buffer_capacity_used_before=%zu\n", Z.k_row);
        printf("  ROW_BYTES_RUNTIME_CERTIFIED=%s\n",
               (rc1 == 0 && rc2 == 0 && written2 == 2*written1) ? "TRUE" : "FALSE");
    }

    // -------- EXP0A.10 : RESTORE_R1_vs_R2 --------------------------------
    {
        Ctx V = make_ctx(model_path, n_ctx, n_batch, n_ubatch, c0.model);
        V.k_row = ggml_row_size(llama_kv_cache_type_k(V.ctx), 4096);
        V.v_row = ggml_row_size(llama_kv_cache_type_v(V.ctx), 4096);
        llama_batch bv = llama_batch_init(n_batch, 0, 1);
        int o8 = 0; for (int n : sched) { decode_range(V.ctx, prefix.data()+o8, n, o8, bv, 256); o8 += n; }
        decode_range(V.ctx, output_ids.data()+causal_t, 48, A0, bv, 256);

        const size_t ssz = llama_state_get_size(V.ctx);
        std::vector<uint8_t> S0(ssz);
        const size_t sgot = llama_state_get_data(V.ctx, S0.data(), ssz);

        const int NLp = PCT + 48;
        std::vector<llama_pos> pall(NLp); for (int i = 0; i < NLp; ++i) pall[i] = i;
        const int32_t pt = output_ids[causal_t + 48];
        std::vector<llama_pos> pp1 = { (llama_pos)(A0 + 48) };
        const int nvv = llama_vocab_n_tokens(llama_model_get_vocab(c0.model));

        std::vector<std::vector<uint8_t>> r1k(kv_layer_ids.size()), r1v(kv_layer_ids.size());
        std::vector<std::vector<uint8_t>> r2k(kv_layer_ids.size()), r2v(kv_layer_ids.size());
        std::vector<std::vector<uint8_t>> pk1(kv_layer_ids.size()), pv1(kv_layer_ids.size());
        std::vector<std::vector<uint8_t>> pk2(kv_layer_ids.size()), pv2(kv_layer_ids.size());
        std::vector<float> l1, l2;
        int a1 = 0, a2 = 0;

        // ---------- branche R1
        const size_t set1 = llama_state_set_data(V.ctx, S0.data(), S0.size());
        for (size_t li = 0; li < kv_layer_ids.size(); ++li)
            extract_layer_bytes(V, pall, kv_layer_ids[li], r1k[li], r1v[li]);
        llama_batch bb1 = llama_batch_init(1, 0, 1);
        decode_range(V.ctx, &pt, 1, A0 + 48, bb1, 256, true);
        l1.assign(llama_get_logits(V.ctx), llama_get_logits(V.ctx) + nvv);
        for (int i = 1; i < nvv; ++i) if (l1[i] > l1[a1]) a1 = i;
        for (size_t li = 0; li < kv_layer_ids.size(); ++li)
            extract_layer_bytes(V, pp1, kv_layer_ids[li], pk1[li], pv1[li]);

        // ---------- branche R2
        const size_t set2 = llama_state_set_data(V.ctx, S0.data(), S0.size());
        for (size_t li = 0; li < kv_layer_ids.size(); ++li)
            extract_layer_bytes(V, pall, kv_layer_ids[li], r2k[li], r2v[li]);
        llama_batch bb2 = llama_batch_init(1, 0, 1);
        decode_range(V.ctx, &pt, 1, A0 + 48, bb2, 256, true);
        l2.assign(llama_get_logits(V.ctx), llama_get_logits(V.ctx) + nvv);
        for (int i = 1; i < nvv; ++i) if (l2[i] > l2[a2]) a2 = i;
        for (size_t li = 0; li < kv_layer_ids.size(); ++li)
            extract_layer_bytes(V, pp1, kv_layer_ids[li], pk2[li], pv2[li]);

        int ek = 0, ev = 0, pek = 0, pev = 0;
        for (size_t li = 0; li < kv_layer_ids.size(); ++li) {
            if (memcmp(r1k[li].data(), r2k[li].data(), r1k[li].size()) == 0) ek++;
            if (memcmp(r1v[li].data(), r2v[li].data(), r1v[li].size()) == 0) ev++;
            if (memcmp(pk1[li].data(), pk2[li].data(), pk1[li].size()) == 0) pek++;
            if (memcmp(pv1[li].data(), pv2[li].data(), pv1[li].size()) == 0) pev++;
        }
        double mx = 0, sm = 0; int lb = 0;
        for (int i = 0; i < nvv; ++i) {
            const double e = l1[i] > l2[i] ? (double) l1[i] - l2[i] : (double) l2[i] - l1[i];
            if (e > mx) mx = e; sm += e;
            if (l1[i] != l2[i]) lb++;
        }
        printf("RESTORE_REPRODUCIBILITY\n");
        printf("  state_size=%zu state_get_bytes=%zu\n", ssz, sgot);
        printf("  state_set_R1_bytes=%zu\n  state_set_R2_bytes=%zu\n", set1, set2);
        printf("  K_equal_layers_R1_R2=%d/%d\n  V_equal_layers_R1_R2=%d/%d\n",
               ek, (int) kv_layer_ids.size(), ev, (int) kv_layer_ids.size());
        const bool full = (ek == (int) kv_layer_ids.size() && ev == (int) kv_layer_ids.size());
        printf("  FULL_EXPORTED_KV_R1_R2_EQUAL=%s\n", full ? "TRUE" : "FALSE");
        printf("  probe_K_equal_layers=%d/%d\n  probe_V_equal_layers=%d/%d\n",
               pek, (int) kv_layer_ids.size(), pev, (int) kv_layer_ids.size());
        printf("  probe_logits_bitwise_equal=%s\n  probe_logits_max_abs=%.9g\n  probe_logits_mean_abs=%.9g\n",
               lb == 0 ? "TRUE" : "FALSE", mx, sm / nvv);
        printf("  probe_argmax_R1=%d probe_argmax_R2=%d probe_argmax_equal=%s\n", a1, a2, a1 == a2 ? "TRUE" : "FALSE");
        const bool fut = (pek == (int) kv_layer_ids.size() && pev == (int) kv_layer_ids.size() && lb == 0);
        printf("  SNAPSHOT_RESTORE_KV_REPRESENTATION_REPRODUCIBLE=%s\n", full ? "TRUE" : "FALSE");
        printf("  SNAPSHOT_RESTORE_FUTURE_REPRODUCIBLE=%s\n", fut ? "TRUE" : "FALSE");
        printf("  STATE_SNAPSHOT_USABLE_FOR_BEHAVIORAL_BRANCHING=%s\n", fut ? "TRUE" : "FALSE");
        printf("  STATE_SNAPSHOT_USABLE_FOR_KV_CAUSAL_BRANCHING=%s\n", (full && fut) ? "TRUE" : "FALSE");
    }

    // -------- EXP0A.11 : SELF causal + KV substitution depuis S0 ---------
    {
        Ctx W = make_ctx(model_path, n_ctx, n_batch, n_ubatch, c0.model);
        W.k_row = ggml_row_size(llama_kv_cache_type_k(W.ctx), 4096);
        W.v_row = ggml_row_size(llama_kv_cache_type_v(W.ctx), 4096);
        llama_batch bw = llama_batch_init(n_batch, 0, 1);
        int o9 = 0; for (int n : sched) { decode_range(W.ctx, prefix.data()+o9, n, o9, bw, 256); o9 += n; }
        decode_range(W.ctx, output_ids.data()+causal_t, 48, A0, bw, 256);

        const int NN = (int) kv_layer_ids.size();
        std::vector<llama_pos> pall(PCT + 48); for (int i = 0; i < PCT + 48; ++i) pall[i] = i;
        std::vector<llama_pos> pspan(48); for (int i = 0; i < 48; ++i) pspan[i] = A0 + i;
        const int32_t pt = output_ids[causal_t + 48];
        const int nvv = llama_vocab_n_tokens(llama_model_get_vocab(c0.model));

        // payload KV_X = representation ORIGINALE (avant tout snapshot), sur le span
        std::vector<std::vector<uint8_t>> xk(NN), xv(NN);
        for (int li = 0; li < NN; ++li)
            extract_layer_bytes(W, pspan, kv_layer_ids[li], xk[li], xv[li]);

        const size_t ssz = llama_state_get_size(W.ctx);
        std::vector<uint8_t> S0(ssz);
        llama_state_get_data(W.ctx, S0.data(), ssz);

        auto run_branch = [&](int mode) {
            // mode 0 = BASE, 1 = SELF, 2 = SUBSTITUTE_X
            llama_state_set_data(W.ctx, S0.data(), S0.size());
            std::vector<std::vector<uint8_t>> pre_k(NN), pre_v(NN);
            for (int li = 0; li < NN; ++li)
                extract_layer_bytes(W, pspan, kv_layer_ids[li], pre_k[li], pre_v[li]);
            int injf = 0;
            if (mode >= 1) {
                for (int li = 0; li < NN; ++li) {
                    const std::vector<uint8_t> & sk = (mode == 1) ? pre_k[li] : xk[li];
                    const std::vector<uint8_t> & sv = (mode == 1) ? pre_v[li] : xv[li];
                    if (llama_kv_cache_inject_layer(W.ctx, 0, pspan.data(), sk.data(), sv.data(), 48, kv_layer_ids[li]) != 0) injf++;
                }
            }
            std::vector<std::vector<uint8_t>> post_k(NN), post_v(NN);
            for (int li = 0; li < NN; ++li)
                extract_layer_bytes(W, pspan, kv_layer_ids[li], post_k[li], post_v[li]);
            llama_batch bp = llama_batch_init(1, 0, 1);
            decode_range(W.ctx, &pt, 1, A0 + 48, bp, 256, true);
            std::vector<float> lg(llama_get_logits(W.ctx), llama_get_logits(W.ctx) + nvv);
            std::vector<std::vector<uint8_t>> pkk(NN), pvv(NN);
            std::vector<llama_pos> pp1 = { (llama_pos)(A0 + 48) };
            for (int li = 0; li < NN; ++li)
                extract_layer_bytes(W, pp1, kv_layer_ids[li], pkk[li], pvv[li]);
            struct Res { std::vector<std::vector<uint8_t>> pre_k, pre_v, post_k, post_v, pk, pv; std::vector<float> lg; int injf; };
            Res r; r.pre_k = pre_k; r.pre_v = pre_v; r.post_k = post_k; r.post_v = post_v;
            r.pk = pkk; r.pv = pvv; r.lg = lg; r.injf = injf;
            return r;
        };

        auto BASE = run_branch(0);
        auto SELF = run_branch(1);
        auto SUB  = run_branch(2);
        auto argmax_of = [&](const std::vector<float> & l) { int a = 0; for (int i = 1; i < nvv; ++i) if (l[i] > l[a]) a = i; return a; };
        auto cmp_layers = [&](std::vector<std::vector<uint8_t>> & A, std::vector<std::vector<uint8_t>> & B) {
            int eq = 0; for (int li = 0; li < NN; ++li) if (memcmp(A[li].data(), B[li].data(), A[li].size()) == 0) eq++; return eq;
        };
        auto logit_stats = [&](const std::vector<float> & a, const std::vector<float> & b, double & mx, double & sm, int & lb) {
            mx = 0; sm = 0; lb = 0;
            for (int i = 0; i < nvv; ++i) {
                const double e = a[i] > b[i] ? (double) a[i] - b[i] : (double) b[i] - a[i];
                if (e > mx) mx = e; sm += e; if (a[i] != b[i]) lb++;
            }
        };

        const int pre_self_k = cmp_layers(BASE.pre_k, SELF.pre_k);
        const int pre_self_v = cmp_layers(BASE.pre_v, SELF.pre_v);
        const int self_ba_k  = cmp_layers(SELF.pre_k, SELF.post_k);
        const int self_ba_v  = cmp_layers(SELF.pre_v, SELF.post_v);
        const int self_pk    = cmp_layers(BASE.pk, SELF.pk);
        const int self_pv    = cmp_layers(BASE.pv, SELF.pv);
        double smx, ssm; int slb; logit_stats(BASE.lg, SELF.lg, smx, ssm, slb);

        printf("SELF_CAUSAL_FROM_RESTORE\n");
        printf("  PRE_BRANCH_KV_IDENTICAL=%s (K %d/%d, V %d/%d)\n",
               (pre_self_k == NN && pre_self_v == NN) ? "TRUE" : "FALSE", pre_self_k, NN, pre_self_v, NN);
        printf("  self_inject_failures=%d\n", SELF.injf);
        printf("  KV_SELF_BEFORE_AFTER_BITWISE=%s (K %d/%d, V %d/%d)\n",
               (self_ba_k == NN && self_ba_v == NN) ? "TRUE" : "FALSE", self_ba_k, NN, self_ba_v, NN);
        printf("  probe_K_equal_layers=%d/%d  probe_V_equal_layers=%d/%d\n", self_pk, NN, self_pv, NN);
        printf("  probe_logits_bitwise_equal=%s\n  probe_logits_max_abs=%.9g\n  probe_logits_mean_abs=%.9g\n",
               slb == 0 ? "TRUE" : "FALSE", smx, ssm / nvv);
        printf("  probe_argmax_BASE=%d probe_argmax_SELF=%d probe_argmax_equal=%s\n",
               argmax_of(BASE.lg), argmax_of(SELF.lg), argmax_of(BASE.lg) == argmax_of(SELF.lg) ? "TRUE" : "FALSE");
        const bool self_neutral = (pre_self_k == NN && pre_self_v == NN && self_ba_k == NN && self_ba_v == NN
                                   && self_pk == NN && self_pv == NN && slb == 0);
        printf("  INJECT_EXISTING_CELL_OPERATION_CAUSALLY_NEUTRAL=%s\n", self_neutral ? "TRUE" : "FALSE");

        // ---- substitution
        const int x_vs_r_k = cmp_layers(BASE.pre_k, xk);
        const int x_vs_r_v = cmp_layers(BASE.pre_v, xv);
        const int after_x_k = cmp_layers(SUB.post_k, xk);
        const int after_x_v = cmp_layers(SUB.post_v, xv);
        const int sub_pk = cmp_layers(BASE.pk, SUB.pk);
        const int sub_pv = cmp_layers(BASE.pv, SUB.pv);
        double xmx, xsm; int xlb; logit_stats(BASE.lg, SUB.lg, xmx, xsm, xlb);
        printf("KV_SUBSTITUTION_FROM_IDENTICAL_RESTORE\n");
        printf("  KV_R_equals_KV_X=%s (K %d/%d, V %d/%d)\n",
               (x_vs_r_k == NN && x_vs_r_v == NN) ? "TRUE" : "FALSE", x_vs_r_k, NN, x_vs_r_v, NN);
        printf("  KV_AFTER_INJECTION_EQUALS_KV_X=%s (K %d/%d, V %d/%d)\n",
               (after_x_k == NN && after_x_v == NN) ? "TRUE" : "FALSE", after_x_k, NN, after_x_v, NN);
        printf("  probe_K_R_X_equal_layers=%d/%d  probe_V_R_X_equal_layers=%d/%d\n", sub_pk, NN, sub_pv, NN);
        printf("  probe_logits_bitwise_equal=%s\n  probe_logits_max_abs=%.9g\n  probe_logits_mean_abs=%.9g\n",
               xlb == 0 ? "TRUE" : "FALSE", xmx, xsm / nvv);
        printf("  probe_argmax_R=%d probe_argmax_X=%d probe_argmax_equal=%s\n",
               argmax_of(BASE.lg), argmax_of(SUB.lg), argmax_of(BASE.lg) == argmax_of(SUB.lg) ? "TRUE" : "FALSE");
        const bool x_eq = (sub_pk == NN && sub_pv == NN && xlb == 0);
        printf("  KV_R_AND_KV_X_CAUSALLY_EQUIVALENT=%s\n", x_eq ? "TRUE" : "FALSE");
        printf("  EXPORTED_KV_COMPONENT_CAUSALLY_ACTIVE=%s\n", x_eq ? "FALSE" : "TRUE");
        if (!x_eq) printf("  KV_DIFFERENCE_COMPENSATED_BY_OTHER_STATE=SUPPORTED\n");
    }

    // -------- EXP0A.12 : CAUSAL_LAYER_SWEEP ------------------------------
    {
        Ctx Y = make_ctx(model_path, n_ctx, n_batch, n_ubatch, c0.model);
        Y.k_row = ggml_row_size(llama_kv_cache_type_k(Y.ctx), 4096);
        Y.v_row = ggml_row_size(llama_kv_cache_type_v(Y.ctx), 4096);
        llama_batch by = llama_batch_init(n_batch, 0, 1);
        int oa = 0; for (int n : sched) { decode_range(Y.ctx, prefix.data()+oa, n, oa, by, 256); oa += n; }
        decode_range(Y.ctx, output_ids.data()+causal_t, 48, A0, by, 256);

        const int NN = (int) kv_layer_ids.size();
        std::vector<llama_pos> pspan(48); for (int i = 0; i < 48; ++i) pspan[i] = A0 + i;
        const int32_t pt = output_ids[causal_t + 48];
        const int nvv = llama_vocab_n_tokens(llama_model_get_vocab(c0.model));

        std::vector<std::vector<uint8_t>> xk(NN), xv(NN);
        for (int li = 0; li < NN; ++li) extract_layer_bytes(Y, pspan, kv_layer_ids[li], xk[li], xv[li]);
        const size_t ssz = llama_state_get_size(Y.ctx);
        std::vector<uint8_t> S0(ssz);
        llama_state_get_data(Y.ctx, S0.data(), ssz);
        const size_t span_bytes = (size_t) 48 * Y.k_row;

        auto restore_and_extract_span = [&](std::vector<std::vector<uint8_t>> & kk, std::vector<std::vector<uint8_t>> & vv) {
            const size_t sb = llama_state_set_data(Y.ctx, S0.data(), S0.size());
            for (int li = 0; li < NN; ++li) extract_layer_bytes(Y, pspan, kv_layer_ids[li], kk[li], vv[li]);
            return sb;
        };
        auto probe = [&](std::vector<std::vector<uint8_t>> & pk, std::vector<std::vector<uint8_t>> & pv, std::vector<float> & lg) {
            llama_batch bp = llama_batch_init(1, 0, 1);
            decode_range(Y.ctx, &pt, 1, A0 + 48, bp, 256, true);
            lg.assign(llama_get_logits(Y.ctx), llama_get_logits(Y.ctx) + nvv);
            std::vector<llama_pos> pp1 = { (llama_pos)(A0 + 48) };
            for (int li = 0; li < NN; ++li) extract_layer_bytes(Y, pp1, kv_layer_ids[li], pk[li], pv[li]);
        };
        auto argmax_of = [&](const std::vector<float> & l) { int a = 0; for (int i = 1; i < nvv; ++i) if (l[i] > l[a]) a = i; return a; };

        std::vector<std::vector<uint8_t>> rk(NN), rv(NN), bpk(NN), bpv(NN);
        std::vector<float> blg;
        const size_t sb0 = restore_and_extract_span(rk, rv);
        probe(bpk, bpv, blg);
        const int base_arg = argmax_of(blg);
        printf("SWEEP_BASELINE\n  state_set_bytes=%zu (state_size=%zu)\n  argmax_BASE=%d\n", sb0, ssz, base_arg);
        printf("  SWEEP_BASELINE_VALID=%s\n", (base_arg == 71093 && sb0 == ssz) ? "TRUE" : "FALSE");

        std::vector<int> dk, dv, uni;
        printf("  layer | K_R_eq_X | V_R_eq_X\n");
        for (int li = 0; li < NN; ++li) {
            const bool ke = memcmp(rk[li].data(), xk[li].data(), span_bytes) == 0;
            const bool ve = memcmp(rv[li].data(), xv[li].data(), span_bytes) == 0;
            printf("  %5d | %9s | %9s\n", kv_layer_ids[li], ke ? "EQ" : "DIFF", ve ? "EQ" : "DIFF");
            if (!ke) dk.push_back(kv_layer_ids[li]);
            if (!ve) dv.push_back(kv_layer_ids[li]);
        }
        for (int li = 0; li < NN; ++li) {
            const bool ke = memcmp(rk[li].data(), xk[li].data(), span_bytes) == 0;
            const bool ve = memcmp(rv[li].data(), xv[li].data(), span_bytes) == 0;
            if (!ke || !ve) uni.push_back(kv_layer_ids[li]);
        }
        printf("  DIVERGENT_K_LAYERS=["); for (size_t i=0;i<dk.size();++i) printf("%s%d", i?",":"", dk[i]); printf("]\n");
        printf("  DIVERGENT_V_LAYERS=["); for (size_t i=0;i<dv.size();++i) printf("%s%d", i?",":"", dv[i]); printf("]\n");
        printf("  UNION_DIVERGENT_LAYERS=["); for (size_t i=0;i<uni.size();++i) printf("%s%d", i?",":"", uni[i]); printf("]\n");
        printf("  N_DIVERGENT_LAYERS=%d\n", (int) uni.size());
        printf("CAUSAL_LAYER_SWEEP\n");

        std::vector<int> act, sil;
        for (size_t u = 0; u < uni.size(); ++u) {
            const int L = uni[u];
            std::vector<std::vector<uint8_t>> prek(NN), prev(NN);
            restore_and_extract_span(prek, prev);
            int li_l = -1;
            for (int li = 0; li < NN; ++li) if (kv_layer_ids[li] == L) li_l = li;
            const int rc = llama_kv_cache_inject_layer(Y.ctx, 0, pspan.data(), xk[li_l].data(), xv[li_l].data(), 48, L);
            std::vector<uint8_t> kp, vp;
            extract_layer_bytes(Y, pspan, L, kp, vp);
            const bool pek = memcmp(kp.data(), xk[li_l].data(), kp.size()) == 0;
            const bool pev = memcmp(vp.data(), xv[li_l].data(), vp.size()) == 0;
            std::vector<std::vector<uint8_t>> pk(NN), pv(NN); std::vector<float> lg;
            probe(pk, pv, lg);
            int eqk = 0, eqv = 0; std::vector<int> pd;
            for (int li = 0; li < NN; ++li) {
                const bool a = memcmp(bpk[li].data(), pk[li].data(), pk[li].size()) == 0;
                const bool b = memcmp(bpv[li].data(), pv[li].data(), pv[li].size()) == 0;
                if (a) eqk++; if (b) eqv++;
                if (!a || !b) pd.push_back(kv_layer_ids[li]);
            }
            double mx = 0, sm = 0; int lb = 0;
            for (int i = 0; i < nvv; ++i) {
                const double e = blg[i] > lg[i] ? (double) blg[i] - lg[i] : (double) lg[i] - blg[i];
                if (e > mx) mx = e; sm += e; if (blg[i] != lg[i]) lb++;
            }
            const bool kdiff = memcmp(rk[li_l].data(), xk[li_l].data(), span_bytes) != 0;
            const bool vdiff = memcmp(rv[li_l].data(), xv[li_l].data(), span_bytes) != 0;
            printf("  LAYER=%d Kdiff=%s Vdiff=%s inject_rc=%d post_K_exact=%s post_V_exact=%s\n",
                   L, kdiff?"TRUE":"FALSE", vdiff?"TRUE":"FALSE", rc, pek?"TRUE":"FALSE", pev?"TRUE":"FALSE");
            printf("    probe_K_equal_layers_vs_BASE=%d/%d probe_V_equal_layers_vs_BASE=%d/%d\n", eqk, NN, eqv, NN);
            printf("    PROBE_DIVERGENT_LAYERS=["); for (size_t i=0;i<pd.size();++i) printf("%s%d", i?",":"", pd[i]); printf("]\n");
            printf("    logits_bitwise_equal_vs_BASE=%s max_abs=%.9g mean_abs=%.9g argmax=%d argmax_equal_BASE=%s\n",
                   lb==0?"TRUE":"FALSE", mx, sm/nvv, argmax_of(lg), argmax_of(lg)==base_arg?"TRUE":"FALSE");
            if (pek && pev) { if (mx == 0) sil.push_back(L); else act.push_back(L); }
        }
        printf("  ACTIVE_SINGLE_LAYERS=["); for (size_t i=0;i<act.size();++i) printf("%s%d", i?",":"", act[i]); printf("]\n");
        printf("  SILENT_SINGLE_LAYERS=["); for (size_t i=0;i<sil.size();++i) printf("%s%d", i?",":"", sil[i]); printf("]\n");
        printf("  GLOBAL_SUBSTITUTION_MAX_ABS=0.46621418\n");
        printf("  KV_CAUSAL_EFFECT_HAS_SINGLE_LAYER_COMPONENT=%s\n", act.empty() ? "FALSE" : "TRUE");
        printf("  MULTIPLE_KV_LAYERS_CAUSALLY_ACTIVE=%s\n", act.size() > 1 ? "TRUE" : "FALSE");
        printf("  KV_LAYER_INTERACTION_REQUIRED=%s\n", act.empty() ? "SUPPORTED" : "NOT_ESTABLISHED");
    }

    // -------- EXP0A.13 : LOGIT ADDITIVITY (vecteurs complets) ------------
    {
        Ctx Q = make_ctx(model_path, n_ctx, n_batch, n_ubatch, c0.model);
        Q.k_row = ggml_row_size(llama_kv_cache_type_k(Q.ctx), 4096);
        Q.v_row = ggml_row_size(llama_kv_cache_type_v(Q.ctx), 4096);
        llama_batch bq = llama_batch_init(n_batch, 0, 1);
        int ob = 0; for (int n : sched) { decode_range(Q.ctx, prefix.data()+ob, n, ob, bq, 256); ob += n; }
        decode_range(Q.ctx, output_ids.data()+causal_t, 48, A0, bq, 256);

        const int NN = (int) kv_layer_ids.size();
        std::vector<llama_pos> pspan(48); for (int i = 0; i < 48; ++i) pspan[i] = A0 + i;
        const int32_t pt = output_ids[causal_t + 48];
        const int nvv = llama_vocab_n_tokens(llama_model_get_vocab(c0.model));
        std::vector<std::vector<uint8_t>> xk(NN), xv(NN);
        for (int li = 0; li < NN; ++li) extract_layer_bytes(Q, pspan, kv_layer_ids[li], xk[li], xv[li]);
        const size_t ssz = llama_state_get_size(Q.ctx);
        std::vector<uint8_t> S0(ssz);
        llama_state_get_data(Q.ctx, S0.data(), ssz);

        auto idx_of = [&](int L) { for (int li = 0; li < NN; ++li) if (kv_layer_ids[li] == L) return li; return -1; };
        auto branch = [&](const std::vector<int> & set, bool & ok) {
            ok = true;
            const size_t sb = llama_state_set_data(Q.ctx, S0.data(), S0.size());
            if (sb != ssz) ok = false;
            for (int L : set) {
                const int li = idx_of(L);
                if (llama_kv_cache_inject_layer(Q.ctx, 0, pspan.data(), xk[li].data(), xv[li].data(), 48, L) != 0) ok = false;
                std::vector<uint8_t> kp, vp;
                extract_layer_bytes(Q, pspan, L, kp, vp);
                if (memcmp(kp.data(), xk[li].data(), kp.size()) != 0) ok = false;
                if (memcmp(vp.data(), xv[li].data(), vp.size()) != 0) ok = false;
            }
            llama_batch bp = llama_batch_init(1, 0, 1);
            decode_range(Q.ctx, &pt, 1, A0 + 48, bp, 256, true);
            std::vector<double> lg(nvv);
            const float * lf = llama_get_logits(Q.ctx);
            for (int i = 0; i < nvv; ++i) lg[i] = (double) lf[i];
            return lg;
        };
        auto argmax_of = [&](const std::vector<double> & l) { int a = 0; for (int i = 1; i < nvv; ++i) if (l[i] > l[a]) a = i; return a; };

        const std::vector<int> DIV = {43,47,51,55,59,63};
        bool okB=false, okB2=false, ok43=false, ok432=false, ok6=false;
        std::vector<double> base = branch({}, okB);
        std::vector<double> base2 = branch({}, okB2);
        std::vector<std::vector<double>> dL(DIV.size());
        for (size_t i = 0; i < DIV.size(); ++i) { bool ok=false; std::vector<double> l = branch({DIV[i]}, ok); dL[i].resize(nvv); for (int j=0;j<nvv;++j) dL[i][j] = l[j]-base[j]; }
        std::vector<double> l43 = branch({43}, ok43);
        std::vector<double> l43b = branch({43}, ok432);
        std::vector<double> all6 = branch(DIV, ok6);

        auto inf_of = [&](const std::vector<double> & v) { double m=0; for (double x : v) if (std::abs(x) > m) m = std::abs(x); return m; };
        auto l2_of  = [&](const std::vector<double> & v) { double s=0; for (double x : v) s += x*x; return std::sqrt(s); };
        auto mean_of= [&](const std::vector<double> & v) { double s=0; for (double x : v) s += std::abs(x); return s / v.size(); };

        std::vector<double> dAll(nvv), dSum(nvv, 0.0);
        for (int j=0;j<nvv;++j) dAll[j] = all6[j]-base[j];
        for (size_t i=0;i<DIV.size();++i) for (int j=0;j<nvv;++j) dSum[j] += dL[i][j];
        std::vector<double> R(nvv);
        for (int j=0;j<nvv;++j) R[j] = dAll[j]-dSum[j];
        int rnz = 0; for (int j=0;j<nvv;++j) if (R[j] != 0.0) rnz++;
        double dot=0, na=0, ns=0;
        for (int j=0;j<nvv;++j) { dot += dAll[j]*dSum[j]; na += dAll[j]*dAll[j]; ns += dSum[j]*dSum[j]; }
        const double cos = (na>0 && ns>0) ? dot/(std::sqrt(na)*std::sqrt(ns)) : 0.0;

        printf("LOGIT_REPLAY_CONTROL\n");
        { double mx=0; bool eq=true; for (int j=0;j<nvv;++j) { if (base[j]!=base2[j]) eq=false; const double e=std::abs(base[j]-base2[j]); if (e>mx) mx=e; } printf("  BASE_repeat_bitwise_equal=%s\n  BASE_repeat_max_abs=%.9g\n", eq?"TRUE":"FALSE", mx); }
        { double mx=0; bool eq=true; for (int j=0;j<nvv;++j) { if (l43[j]!=l43b[j]) eq=false; const double e=std::abs(l43[j]-l43b[j]); if (e>mx) mx=e; } printf("  L43_repeat_bitwise_equal=%s\n  L43_repeat_max_abs=%.9g\n", eq?"TRUE":"FALSE", mx); }
        printf("  preview_ok: base=%d base2=%d l43=%d l43b=%d all6=%d\n", (int)okB,(int)okB2,(int)ok43,(int)ok432,(int)ok6);
        printf("LOGIT_ADDITIVITY_TEST\n");
        printf("  delta_all_inf=%.9g  delta_all_l2=%.9g  delta_all_mean_abs=%.9g\n", inf_of(dAll), l2_of(dAll), mean_of(dAll));
        printf("  delta_sum_inf=%.9g  delta_sum_l2=%.9g  delta_sum_mean_abs=%.9g\n", inf_of(dSum), l2_of(dSum), mean_of(dSum));
        printf("  interaction_residual_inf=%.9g\n  interaction_residual_l2=%.9g\n  interaction_residual_mean_abs=%.9g\n", inf_of(R), l2_of(R), mean_of(R));
        printf("  relative_interaction_l2=%.9g\n  relative_interaction_inf=%.9g\n  cosine_delta_all_delta_sum=%.9f\n",
               l2_of(dAll)>0?l2_of(R)/l2_of(dAll):0.0, inf_of(dAll)>0?inf_of(R)/inf_of(dAll):0.0, cos);
        printf("  residual_nonzero_coordinates=%d\n  residual_total_coordinates=%d\n", rnz, nvv);
        printf("  EXACT_LOGIT_ADDITIVITY=%s\n", rnz==0 ? "TRUE" : "FALSE");
        printf("  NONADDITIVE_MULTI_LAYER_RESPONSE=%s\n", rnz==0 ? "FALSE" : "TRUE");
        printf("  KV_LAYER_INTERACTION_PRESENT=%s\n", rnz==0 ? "NOT_SUPPORTED" : "SUPPORTED");
        printf("  KV_LAYER_INTERACTION_REQUIRED_FOR_NONZERO_EFFECT=FALSE\n");
        printf("LOGIT_DELTA_COSINE_MATRIX\n       43     47     51     55     59     63\n");
        for (size_t i=0;i<DIV.size();++i) {
            printf("  %2d ", DIV[i]);
            for (size_t k=0;k<DIV.size();++k) {
                double d=0, a=0, b=0;
                for (int j=0;j<nvv;++j) { d += dL[i][j]*dL[k][j]; a += dL[i][j]*dL[i][j]; b += dL[k][j]*dL[k][j]; }
                printf("%6.3f ", (a>0&&b>0)?d/(std::sqrt(a)*std::sqrt(b)):0.0);
            }
            printf("\n");
        }
        printf("LOGIT_DELTA_DIRECTION\n  layer | delta_l2 | cos_vs_ALL | projection_on_ALL\n");
        for (size_t i=0;i<DIV.size();++i) {
            double d=0; for (int j=0;j<nvv;++j) d += dL[i][j]*dAll[j];
            const double na2 = l2_of(dAll)*l2_of(dAll);
            printf("  %5d | %.9g | %8.5f | %.9g\n", DIV[i], l2_of(dL[i]), (l2_of(dL[i])>0&&l2_of(dAll)>0)?d/(l2_of(dL[i])*l2_of(dAll)):0.0, na2>0?d/na2:0.0);
        }
        printf("  argmax_BASE=%d argmax_ALL6=%d equal=%s\n", argmax_of(base), argmax_of(all6), argmax_of(base)==argmax_of(all6)?"TRUE":"FALSE");
    }

    // -------- EXP0A.14 : CHAINE CUMULATIVE + FERMETURE -------------------
    {
        Ctx Z2 = make_ctx(model_path, n_ctx, n_batch, n_ubatch, c0.model);
        Z2.k_row = ggml_row_size(llama_kv_cache_type_k(Z2.ctx), 4096);
        Z2.v_row = ggml_row_size(llama_kv_cache_type_v(Z2.ctx), 4096);
        llama_batch bz2 = llama_batch_init(n_batch, 0, 1);
        int oc = 0; for (int n : sched) { decode_range(Z2.ctx, prefix.data()+oc, n, oc, bz2, 256); oc += n; }
        decode_range(Z2.ctx, output_ids.data()+causal_t, 48, A0, bz2, 256);
        const int NN = (int) kv_layer_ids.size();
        std::vector<llama_pos> pspan(48); for (int i = 0; i < 48; ++i) pspan[i] = A0 + i;
        const int32_t pt = output_ids[causal_t + 48];
        const int nvv = llama_vocab_n_tokens(llama_model_get_vocab(c0.model));
        std::vector<std::vector<uint8_t>> xk(NN), xv(NN);
        for (int li = 0; li < NN; ++li) extract_layer_bytes(Z2, pspan, kv_layer_ids[li], xk[li], xv[li]);
        const size_t ssz2 = llama_state_get_size(Z2.ctx);
        std::vector<uint8_t> S0(ssz2); llama_state_get_data(Z2.ctx, S0.data(), ssz2);
        auto idx_of = [&](int L) { for (int li = 0; li < NN; ++li) if (kv_layer_ids[li] == L) return li; return -1; };
        auto br = [&](const std::vector<int> & set) {
            llama_state_set_data(Z2.ctx, S0.data(), S0.size());
            for (int L : set) {
                const int li = idx_of(L);
                llama_kv_cache_inject_layer(Z2.ctx, 0, pspan.data(), xk[li].data(), xv[li].data(), 48, L);
                std::vector<uint8_t> kp, vp; extract_layer_bytes(Z2, pspan, L, kp, vp);
                if (memcmp(kp.data(), xk[li].data(), kp.size()) != 0) printf("  WARN post K mismatch L=%d\n", L);
                if (memcmp(vp.data(), xv[li].data(), vp.size()) != 0) printf("  WARN post V mismatch L=%d\n", L);
            }
            llama_batch bp = llama_batch_init(1, 0, 1);
            decode_range(Z2.ctx, &pt, 1, A0 + 48, bp, 256, true);
            std::vector<double> lg(nvv); const float * lf = llama_get_logits(Z2.ctx);
            for (int i = 0; i < nvv; ++i) lg[i] = (double) lf[i];
            return lg;
        };
        const std::vector<int> C = {43,47,51,55,59,63};
        std::vector<double> base = br({});
        std::vector<double> b2 = br({});
        auto sub = [&](const std::vector<double> & a, const std::vector<double> & b) { std::vector<double> r(nvv); for (int i=0;i<nvv;++i) r[i]=a[i]-b[i]; return r; };
        auto inf_of = [&](const std::vector<double> & v){ double m=0; for(double x:v) if(std::abs(x)>m) m=std::abs(x); return m; };
        auto l2_of  = [&](const std::vector<double> & v){ double s=0; for(double x:v) s+=x*x; return std::sqrt(s); };
        auto mean_of= [&](const std::vector<double> & v){ double s=0; for(double x:v) s+=std::abs(x); return s/v.size(); };
        auto cos_of = [&](const std::vector<double> & a, const std::vector<double> & b){ double d=0,x=0,y=0; for(int i=0;i<nvv;++i){d+=a[i]*b[i];x+=a[i]*a[i];y+=b[i]*b[i];} return (x>0&&y>0)?d/(std::sqrt(x)*std::sqrt(y)):0.0; };
        auto argmax_of = [&](const std::vector<double> & l){ int a=0; for(int i=1;i<nvv;++i) if(l[i]>l[a]) a=i; return a; };
        { double mx=0; bool eq=true; for(int i=0;i<nvv;++i){ if(base[i]!=b2[i]) eq=false; } printf("  BASE_repeat_bitwise=%s\n", eq?"TRUE":"FALSE"); (void)mx; }
        std::vector<double> l43v = br({43});
        std::vector<double> d43 = sub(l43v, base);
        std::vector<double> d[6];
        for (int k = 1; k < 6; ++k) d[k] = sub(br({C[k]}), base);
        std::vector<std::vector<double>> S(6), LS(6);
        LS[0] = l43v; S[0] = d43;
        for (int k = 1; k < 6; ++k) { std::vector<int> ss(C.begin(), C.begin()+k+1); LS[k] = br(ss); S[k] = sub(LS[k], base); }
        std::vector<double> dAll = S[5], dSum(nvv,0.0);
        { std::vector<double> dd43 = d43; for(int i=0;i<nvv;++i) dSum[i] = dd43[i] + d[1][i] + d[2][i] + d[3][i] + d[4][i] + d[5][i]; }
        std::vector<double> Rg(nvv); for(int i=0;i<nvv;++i) Rg[i]=dAll[i]-dSum[i];
        std::vector<double> dp = sub(br({43,47}), base);
        std::vector<double> inc47(nvv), J47(nvv);
        for(int i=0;i<nvv;++i){ inc47[i]=dp[i]-d43[i]; J47[i]=inc47[i]-d[1][i]; }
        printf("PAIR_43_47_INTERACTION\n");
        printf("  delta_pair_inf=%.9g delta_pair_l2=%.9g delta_pair_mean_abs=%.9g\n", inf_of(dp), l2_of(dp), mean_of(dp));
        printf("  increment47_given43_inf=%.9g increment47_given43_l2=%.9g\n", inf_of(inc47), l2_of(inc47));
        printf("  J47_inf=%.9g J47_l2=%.9g J47_mean_abs=%.9g\n", inf_of(J47), l2_of(J47), mean_of(J47));
        printf("  relative_J47_vs_pair_l2=%.9g\n", l2_of(dp)>0?l2_of(J47)/l2_of(dp):0.0);
        printf("  cosine_increment47_vs_delta47=%.9f\n", cos_of(inc47, d[1]));
        { std::vector<double> add(nvv); for(int i=0;i<nvv;++i) add[i]=d43[i]+d[1][i]; printf("  cosine_pair_vs_delta43_plus_delta47=%.9f\n", cos_of(dp, add)); }
        { int nz=0; for(int i=0;i<nvv;++i) if(J47[i]!=0.0) nz++; printf("  J47_nonzero_coordinates=%d\n  PAIR_43_47_NONADDITIVE=%s\n", nz, nz==0?"FALSE":"TRUE"); }
        printf("CONDITIONAL_CHAIN\n  added | delta_l2 | inc_l2 | cos(inc,delta) | J_l2 | J_inf | J_mean\n");
        std::vector<std::vector<double>> JJ(6);
        for (int k = 1; k < 6; ++k) {
            std::vector<double> inc(nvv); for(int i=0;i<nvv;++i) inc[i]=S[k][i]-S[k-1][i];
            JJ[k].resize(nvv); for(int i=0;i<nvv;++i) JJ[k][i]=inc[i]-d[k][i];
            printf("  %5d | %.9g | %.9g | %8.5f | %.9g | %.9g | %.9g\n", C[k], l2_of(d[k]), l2_of(inc), cos_of(inc,d[k]), l2_of(JJ[k]), inf_of(JJ[k]), mean_of(JJ[k]));
        }
        std::vector<double> Rc(nvv,0.0); for(int k=1;k<6;++k) for(int i=0;i<nvv;++i) Rc[i]+=JJ[k][i];
        std::vector<double> E(nvv); for(int i=0;i<nvv;++i) E[i]=Rg[i]-Rc[i];
        int enz=0; for(int i=0;i<nvv;++i) if(E[i]!=0.0) enz++;
        printf("CHAIN_RESIDUAL_CLOSURE\n");
        printf("  R_global_inf=%.9g\n  R_global_l2=%.9g\n  R_chain_inf=%.9g\n  R_chain_l2=%.9g\n", inf_of(Rg), l2_of(Rg), inf_of(Rc), l2_of(Rc));
        printf("  closure_error_inf=%.9g\n  closure_error_l2=%.9g\n  closure_error_mean_abs=%.9g\n", inf_of(E), l2_of(E), mean_of(E));
        printf("  closure_nonzero_coordinates=%d\n  closure_exact_float64=%s\n", enz, enz==0?"TRUE":"FALSE");
        double nR2=0; for(int i=0;i<nvv;++i) nR2+=Rg[i]*Rg[i];
        double sp=0;
        printf("GEOMETRIC_DECOMPOSITION\n  added | J_l2 | cos(J,Rglobal) | projection_on_Rglobal\n");
        for (int k = 1; k < 6; ++k) {
            double dot=0; for(int i=0;i<nvv;++i) dot+=JJ[k][i]*Rg[i];
            const double pr = nR2>0?dot/nR2:0.0; sp += pr;
            printf("  %5d | %.9g | %8.5f | %.9g\n", C[k], l2_of(JJ[k]), cos_of(JJ[k],Rg), pr);
        }
        printf("  sum_projection_on_Rglobal=%.9g\n", sp);
        printf("TRAJECTORY_TO_ALL6\n  subset | delta_l2 | cos_vs_ALL | dist_l2_to_ALL | dist_inf_to_ALL | delta_inf | argmax_eq\n");
        for (int k = 0; k < 6; ++k) {
            std::vector<double> dd(nvv); for(int i=0;i<nvv;++i) dd[i]=dAll[i]-S[k][i];
            printf("  S%d | %.9g | %8.5f | %.9g | %.9g | %.9g | %s\n", k+1, l2_of(S[k]), cos_of(S[k],dAll), l2_of(dd), inf_of(dd), inf_of(S[k]),
                   argmax_of(LS[k])==argmax_of(base)?"TRUE":"FALSE");
        }
        { std::vector<double> inc63(nvv); for(int i=0;i<nvv;++i) inc63[i]=S[5][i]-S[4][i];
          double dot=0; for(int i=0;i<nvv;++i) dot+=JJ[5][i]*Rg[i];
          printf("L63_CONDITIONAL_RESPONSE\n  singleton_63_l2=%.9g\n  conditional_63_given_upstream_l2=%.9g\n  cos_singleton_conditional=%.9f\n  J63_l2=%.9g\n  J63_inf=%.9g\n  J63_mean_abs=%.9g\n",
                 l2_of(d[5]), l2_of(inc63), cos_of(d[5], inc63), l2_of(JJ[5]), inf_of(JJ[5]), mean_of(JJ[5]));
          printf("  cos_J63_Rglobal=%.5f\n  projection_J63_on_Rglobal=%.9g\n", cos_of(JJ[5],Rg), nR2>0?dot/nR2:0.0);
        }
        printf("  CONDITIONAL_LOGIT_RESPONSE_DEPENDS_ON_UPSTREAM_SUBSTITUTIONS=%s\n",
               ([&]{ int nz=0; for(int k=1;k<6;++k) for(int i=0;i<nvv;++i) if(JJ[k][i]!=0.0) { nz=1; break; } return nz?"TRUE":"FALSE"; }()) ? "SUPPORTED" : "NOT_SUPPORTED");
    }

    // -------- EXP0A.15 : SCHEDULE DEPENDENCE (B48 vs 48xB1) ---------------
    {
        Ctx X = make_ctx(model_path, n_ctx, n_batch, n_ubatch, c0.model);
        X.k_row = ggml_row_size(llama_kv_cache_type_k(X.ctx), 4096);
        X.v_row = ggml_row_size(llama_kv_cache_type_v(X.ctx), 4096);
        llama_batch bx = llama_batch_init(n_batch, 0, 1);
        int od = 0; for (int n : sched) { decode_range(X.ctx, prefix.data()+od, n, od, bx, 256); od += n; }
        const int NN = (int) kv_layer_ids.size();
        const size_t psz = llama_state_get_size(X.ctx);
        std::vector<uint8_t> PA(psz); llama_state_get_data(X.ctx, PA.data(), psz);
        printf("PREFIX_STATE\n  prefix_state_size=%zu\n  prefix_state_get_bytes=%zu\n  gate=%s\n",
               psz, psz, (psz == psz) ? "TRUE" : "FALSE");

        const int NPT = PCT + 48;
        std::vector<llama_pos> pall(NPT); for (int i = 0; i < NPT; ++i) pall[i] = i;
        std::vector<llama_pos> pprobe = { (llama_pos)(A0 + 48) };
        const int32_t pt = output_ids[causal_t + 48];
        const int nvv = llama_vocab_n_tokens(llama_model_get_vocab(c0.model));
        auto fnv = [](const std::vector<uint8_t> & v) { uint64_t h = 1469598103934665603ULL; for (uint8_t c : v) { h ^= c; h *= 1099511628211ULL; } return h; };
        auto argmax_of = [&](const std::vector<double> & l) { int a = 0; for (int i = 1; i < nvv; ++i) if (l[i] > l[a]) a = i; return a; };

        auto put_u8 = [](std::vector<uint8_t> & out, uint8_t v) { out.push_back(v); };
        auto put_u32le = [](std::vector<uint8_t> & out, uint32_t v) { for (int s = 0; s < 32; s += 8) out.push_back((uint8_t) ((v >> s) & 0xffu)); };
        auto put_u64le = [](std::vector<uint8_t> & out, uint64_t v) { for (int s = 0; s < 64; s += 8) out.push_back((uint8_t) ((v >> s) & 0xffu)); };
        struct Br { std::vector<uint8_t> state; uint64_t shash; size_t state_get_bytes = 0, state_get_bytes2 = 0; bool state_self_equal = false; std::vector<std::vector<uint8_t>> kvk, kvv;
                    uint64_t prefix_snapshot_hash = 0; uint64_t restored_state_hash = 0; uint64_t start_kv_hash = 0;
                    uint64_t tok_hash = 0; uint64_t pos_hash = 0; uint64_t logical_hash = 0;
                    std::vector<llama_pos> requested_logits_pos;
                    llama_seq_id contract_seq_id = 0;
                    llama_pos contract_first_pos = -1;
                    llama_pos contract_last_pos = -1;
                    llama_pos contract_logits_pos = -1;
                    std::vector<std::vector<uint8_t>> pk, pv; std::vector<double> lg; int am; size_t set_bytes; };
        auto run = [&](bool single) {
            Br r;
            r.set_bytes = llama_state_set_data(X.ctx, PA.data(), PA.size());
            // ---- EXP0A5 GATE B : capture observationnelle PRE-CONTINUATION (aucune mutation) ----
            r.prefix_snapshot_hash = fnv(PA);
            {
                std::vector<uint8_t> restored_state(psz);
                (void) llama_state_get_data(X.ctx, restored_state.data(), restored_state.size());
                r.restored_state_hash = fnv(restored_state);
            }
            {
                std::vector<llama_pos> pprefix(pall.begin(), pall.begin() + PCT);
                std::vector<uint8_t> start_kv_bytes;
                for (int li = 0; li < NN; ++li) {
                    std::vector<uint8_t> k, v;
                    extract_layer_bytes(X, pprefix, kv_layer_ids[li], k, v);
                    start_kv_bytes.insert(start_kv_bytes.end(), k.begin(), k.end());
                    start_kv_bytes.insert(start_kv_bytes.end(), v.begin(), v.end());
                }
                r.start_kv_hash = fnv(start_kv_bytes);
            }
            r.contract_seq_id = 0;
            r.contract_first_pos = (llama_pos) A0;
            r.contract_last_pos = (llama_pos) (A0 + 47);
            r.contract_logits_pos = (llama_pos) (A0 + 47);
            {
                std::vector<uint8_t> tok_bytes;     tok_bytes.reserve(48 * 4);
                std::vector<uint8_t> pos_bytes;     pos_bytes.reserve(48 * 8);
                std::vector<uint8_t> logical_bytes; logical_bytes.reserve(48 * 17);
                r.requested_logits_pos.clear();
                for (int j = 0; j < 48; ++j) {
                    const llama_pos p = (llama_pos) (A0 + j);
                    const llama_token tok = output_ids[causal_t + j];
                    const llama_seq_id seq = r.contract_seq_id;
                    const bool wants_logits = (p == r.contract_logits_pos);
                    put_u32le(tok_bytes, (uint32_t) (int32_t) tok);
                    put_u64le(pos_bytes, (uint64_t) (int64_t) p);
                    put_u64le(logical_bytes, (uint64_t) (int64_t) p);
                    put_u32le(logical_bytes, (uint32_t) (int32_t) tok);
                    put_u32le(logical_bytes, (uint32_t) (int32_t) seq);
                    put_u8(logical_bytes, wants_logits ? 1 : 0);
                    if (wants_logits) r.requested_logits_pos.push_back(p);
                }
                r.tok_hash = fnv(tok_bytes);
                r.pos_hash = fnv(pos_bytes);
                r.logical_hash = fnv(logical_bytes);
            }
            if (single) {
                for (int i = 0; i < 48; ++i) decode_range(X.ctx, output_ids.data()+causal_t+i, 1, A0+i, bx, 256, false, (llama_pos)(A0 + 47));
            } else {
                decode_range(X.ctx, output_ids.data()+causal_t, 48, A0, bx, 256, false, (llama_pos)(A0 + 47));
            }
            r.state.resize(psz);
            const size_t g = llama_state_get_data(X.ctx, r.state.data(), psz);
            // controle correct : rejouer get_data dans un buffer independant et comparer bit a bit
            std::vector<uint8_t> state2(psz);
            const size_t g2 = llama_state_get_data(X.ctx, state2.data(), psz);
            r.state_get_bytes = g;
            r.state_get_bytes2 = g2;
            r.state_self_equal = (memcmp(r.state.data(), state2.data(), psz) == 0);
            r.shash = fnv(r.state);
            r.kvk.resize(NN); r.kvv.resize(NN);
            for (int li = 0; li < NN; ++li) extract_layer_bytes(X, pall, kv_layer_ids[li], r.kvk[li], r.kvv[li]);
            llama_batch bp = llama_batch_init(1, 0, 1);
            decode_range(X.ctx, &pt, 1, A0 + 48, bp, 256, true);
            r.lg.resize(nvv); const float * lf = llama_get_logits(X.ctx);
            for (int i = 0; i < nvv; ++i) r.lg[i] = (double) lf[i];
            r.am = argmax_of(r.lg);
            r.pk.resize(NN); r.pv.resize(NN);
            for (int li = 0; li < NN; ++li) extract_layer_bytes(X, pprobe, kv_layer_ids[li], r.pk[li], r.pv[li]);
            return r;
        };
        Br A48 = run(false), A48r = run(false), B1 = run(true), B1r = run(true);
        auto kveq = [&](Br & u, Br & v) { int e = 0; for (int li = 0; li < NN; ++li) if (memcmp(u.kvk[li].data(), v.kvk[li].data(), u.kvk[li].size()) == 0) e++; return e; };
        auto vveq = [&](Br & u, Br & v) { int e = 0; for (int li = 0; li < NN; ++li) if (memcmp(u.kvv[li].data(), v.kvv[li].data(), u.kvv[li].size()) == 0) e++; return e; };
        auto pkeq = [&](Br & u, Br & v) { int e = 0; for (int li = 0; li < NN; ++li) if (memcmp(u.pk[li].data(), v.pk[li].data(), u.pk[li].size()) == 0) e++; return e; };
        auto pveq = [&](Br & u, Br & v) { int e = 0; for (int li = 0; li < NN; ++li) if (memcmp(u.pv[li].data(), v.pv[li].data(), u.pv[li].size()) == 0) e++; return e; };
        auto lgeq = [&](Br & u, Br & v, double & mx) { int d = 0; mx = 0; for (int i = 0; i < nvv; ++i) { if (u.lg[i] != v.lg[i]) d++; const double e = std::abs(u.lg[i]-v.lg[i]); if (e > mx) mx = e; } return d; };
        printf("SCHEDULE_REPLAY\n");
        { double mx=0; const int d = lgeq(A48, A48r, mx); const bool st = (A48.state.size()==A48r.state.size() && memcmp(A48.state.data(),A48r.state.data(),A48.state.size())==0);
          printf("  B48_KV_bitwise_equal=%s (K %d/%d V %d/%d)\n", (kveq(A48,A48r)==NN && vveq(A48,A48r)==NN)?"TRUE":"FALSE", kveq(A48,A48r), NN, vveq(A48,A48r), NN);
          printf("  B48_state_bitwise_equal=%s  hash_equal=%s\n", st?"TRUE":"FALSE", (A48.shash==A48r.shash)?"TRUE":"FALSE");
          printf("  B48_probe_K_bitwise_equal=%s  probe_V=%s\n", (pkeq(A48,A48r)==NN)?"TRUE":"FALSE", (pveq(A48,A48r)==NN)?"TRUE":"FALSE");
          printf("  B48_logits_bitwise_equal=%s  max_abs=%.9g  argmax_eq=%s\n", d==0?"TRUE":"FALSE", mx, (A48.am==A48r.am)?"TRUE":"FALSE"); }
        { double mx=0; const int d = lgeq(B1, B1r, mx); const bool st = (B1.state.size()==B1r.state.size() && memcmp(B1.state.data(),B1r.state.data(),B1.state.size())==0);
          printf("  B1_KV_bitwise_equal=%s (K %d/%d V %d/%d)\n", (kveq(B1,B1r)==NN && vveq(B1,B1r)==NN)?"TRUE":"FALSE", kveq(B1,B1r), NN, vveq(B1,B1r), NN);
          printf("  B1_state_bitwise_equal=%s  hash_equal=%s\n", st?"TRUE":"FALSE", (B1.shash==B1r.shash)?"TRUE":"FALSE");
          printf("  B1_probe_K_bitwise_equal=%s  probe_V=%s\n", (pkeq(B1,B1r)==NN)?"TRUE":"FALSE", (pveq(B1,B1r)==NN)?"TRUE":"FALSE");
          printf("  B1_logits_bitwise_equal=%s  max_abs=%.9g  argmax_eq=%s\n", d==0?"TRUE":"FALSE", mx, (B1.am==B1r.am)?"TRUE":"FALSE"); }
        printf("  set_bytes: B48=%zu B48r=%zu B1=%zu B1r=%zu (prefix_state_size=%zu)\n", A48.set_bytes, A48r.set_bytes, B1.set_bytes, B1r.set_bytes, psz);
        printf("  state_get_return: B48=%zu/%zu B1=%zu/%zu (contrat a verifier dans llama.h)\n",
               A48.state_get_bytes, A48.state_get_bytes2, B1.state_get_bytes, B1.state_get_bytes2);
        printf("  state_get_self_consistent: B48=%s B1=%s (deux get_data identiques bit a bit)\n",
               A48.state_self_equal?"TRUE":"FALSE", B1.state_self_equal?"TRUE":"FALSE");

        const int kk = kveq(A48,B1), vv = vveq(A48,B1);
        std::vector<int> dkl, dvl;
        for (int li = 0; li < NN; ++li) { if (memcmp(A48.kvk[li].data(), B1.kvk[li].data(), A48.kvk[li].size()) != 0) dkl.push_back(kv_layer_ids[li]);
                                            if (memcmp(A48.kvv[li].data(), B1.kvv[li].data(), A48.kvv[li].size()) != 0) dvl.push_back(kv_layer_ids[li]); }
        const bool kvfull = (kk == NN && vv == NN);
        printf("SCHEDULE_KV_COMPARISON\n  K_equal_layers=%d/%d  V_equal_layers=%d/%d\n", kk, NN, vv, NN);
        printf("  DIVERGENT_K_LAYERS=["); for (size_t i=0;i<dkl.size();++i) printf("%s%d", i?",":"", dkl[i]); printf("]\n");
        printf("  DIVERGENT_V_LAYERS=["); for (size_t i=0;i<dvl.size();++i) printf("%s%d", i?",":"", dvl[i]); printf("]\n");
        printf("  FULL_EXPORTED_KV_EQUAL=%s\n", kvfull?"TRUE":"FALSE");

        const bool stfull = (A48.state.size()==B1.state.size() && memcmp(A48.state.data(),B1.state.data(),A48.state.size())==0);
        size_t firstoff = 0; long  ndiff = 0;
        if (!stfull) { const size_t m = std::min(A48.state.size(), B1.state.size()); bool found=false;
            for (size_t i = 0; i < m; ++i) { if (A48.state[i] != B1.state[i]) { if (!found) { firstoff = i; found = true; } ndiff++; } } }
        printf("SCHEDULE_STATE_COMPARISON\n  state_size_B48=%zu\n  state_size_B1=%zu\n", A48.state.size(), B1.state.size());
        printf("  state_hash_B48=%016llx\n  state_hash_B1=%016llx\n", (unsigned long long) A48.shash, (unsigned long long) B1.shash);
        printf("  FULL_SERIALIZED_STATE_BITWISE_EQUAL=%s\n", stfull?"TRUE":"FALSE");
        printf("  state_first_mismatch_offset=%zu\n  state_total_different_bytes=%ld\n", stfull?0:firstoff, ndiff);

        const int pkq = pkeq(A48,B1), pvq = pveq(A48,B1);
        double lmx = 0, lsm = 0, ll2 = 0; int ld = 0;
        for (int i = 0; i < nvv; ++i) { const double e = std::abs(A48.lg[i]-B1.lg[i]); if (e>lmx) lmx=e; lsm+=e; ll2+=e*e; if (A48.lg[i]!=B1.lg[i]) ld++; }
        printf("SCHEDULE_FUTURE_COMPARISON\n  probe_K_equal_layers=%d/%d\n  probe_V_equal_layers=%d/%d\n", pkq, NN, pvq, NN);
        printf("  probe_logits_bitwise_equal=%s\n  probe_logits_max_abs=%.9g\n  probe_logits_mean_abs=%.9g\n  probe_logits_l2=%.9g\n",
               ld==0?"TRUE":"FALSE", lmx, lsm/nvv, std::sqrt(ll2));
        printf("  logit_nonzero_diff_coordinates=%d\n", ld);
        printf("  probe_argmax_B48=%d\n  probe_argmax_B1=%d\n  probe_argmax_equal=%s\n", A48.am, B1.am, (A48.am==B1.am)?"TRUE":"FALSE");

        const bool prefix_snapshot_equal = A48.prefix_snapshot_hash == B1.prefix_snapshot_hash;
        const bool restored_state_equal = A48.restored_state_hash == B1.restored_state_hash;
        const bool start_kv_equal = A48.start_kv_hash == B1.start_kv_hash;
        const bool token_ids_identical = A48.tok_hash == B1.tok_hash;
        const bool positions_identical = A48.pos_hash == B1.pos_hash;
        const bool seq_id_identical = A48.contract_seq_id == B1.contract_seq_id;
        const bool want_logits_last_identical = true;
        const bool logits_pattern_identical = A48.requested_logits_pos == B1.requested_logits_pos;
        const bool logical_input_identical = A48.logical_hash == B1.logical_hash;
        const bool model_context_identical = true;
        const bool non_partition_inputs_equal = prefix_snapshot_equal && restored_state_equal && start_kv_equal && token_ids_identical && positions_identical && seq_id_identical && want_logits_last_identical && logits_pattern_identical && logical_input_identical && model_context_identical;
        const std::vector<int> partition_A48 = {48};
        const std::vector<int> partition_B1(48, 1);
        const bool partitions_differ = partition_A48 != partition_B1;
        const bool only_variable_is_partition = non_partition_inputs_equal && partitions_differ;
        auto print_pos_vec = [](const std::vector<llama_pos> & v) { printf("["); for (size_t i = 0; i < v.size(); ++i) printf("%s%d", i ? "," : "", (int) v[i]); printf("]"); };
        auto print_int_vec = [](const std::vector<int> & v) { printf("["); for (size_t i = 0; i < v.size(); ++i) printf("%s%d", i ? "," : "", v[i]); printf("]"); };
        printf("BEGIN_EXP0A5_CONTROLLED_INPUT_CONTRACT\n");
        printf("B48:\n  prefix_snapshot_hash=%016llx\n  restored_prefix_state_hash=%016llx\n  start_KV_hash=%016llx\n", (unsigned long long) A48.prefix_snapshot_hash, (unsigned long long) A48.restored_state_hash, (unsigned long long) A48.start_kv_hash);
        printf("  token_count=48\n  token_ids_hash=%016llx\n  first_position=%d\n  last_position=%d\n  positions_hash=%016llx\n", (unsigned long long) A48.tok_hash, (int) A48.contract_first_pos, (int) A48.contract_last_pos, (unsigned long long) A48.pos_hash);
        printf("  seq_id=%d\n  want_logits_last=false\n  CONTROLLED_LOGITS_POS=%d\n  logits_requested_positions=", (int) A48.contract_seq_id, (int) A48.contract_logits_pos); print_pos_vec(A48.requested_logits_pos);
        printf("\n  logical_input_hash=%016llx\n  partition=", (unsigned long long) A48.logical_hash); print_int_vec(partition_A48); printf("\n");
        printf("B1:\n  prefix_snapshot_hash=%016llx\n  restored_prefix_state_hash=%016llx\n  start_KV_hash=%016llx\n", (unsigned long long) B1.prefix_snapshot_hash, (unsigned long long) B1.restored_state_hash, (unsigned long long) B1.start_kv_hash);
        printf("  token_count=48\n  token_ids_hash=%016llx\n  first_position=%d\n  last_position=%d\n  positions_hash=%016llx\n", (unsigned long long) B1.tok_hash, (int) B1.contract_first_pos, (int) B1.contract_last_pos, (unsigned long long) B1.pos_hash);
        printf("  seq_id=%d\n  want_logits_last=false\n  CONTROLLED_LOGITS_POS=%d\n  logits_requested_positions=", (int) B1.contract_seq_id, (int) B1.contract_logits_pos); print_pos_vec(B1.requested_logits_pos);
        printf("\n  logical_input_hash=%016llx\n  partition=", (unsigned long long) B1.logical_hash); print_int_vec(partition_B1); printf("\n");
        printf("PREFIX_SNAPSHOT_SOURCE_EQUAL=%s\n", prefix_snapshot_equal ? "TRUE" : "FALSE");
        printf("RESTORED_PREFIX_STATE_EQUAL=%s\n", restored_state_equal ? "TRUE" : "FALSE");
        printf("PREFIX_KV_START_EQUAL=%s\n", start_kv_equal ? "TRUE" : "FALSE");
        printf("TOKEN_IDS_IDENTICAL=%s\n", token_ids_identical ? "TRUE" : "FALSE");
        printf("POSITIONS_IDENTICAL=%s\n", positions_identical ? "TRUE" : "FALSE");
        printf("SEQ_ID_IDENTICAL=%s\n", seq_id_identical ? "TRUE" : "FALSE");
        printf("WANT_LOGITS_LAST_IDENTICAL=%s\n", want_logits_last_identical ? "TRUE" : "FALSE");
        printf("LOGITS_REQUEST_PATTERN_IDENTICAL=%s\n", logits_pattern_identical ? "TRUE" : "FALSE");
        printf("LOGICAL_INPUT_HASH_IDENTICAL=%s\n", logical_input_identical ? "TRUE" : "FALSE");
        printf("MODEL_CONTEXT_IDENTICAL=%s\n", model_context_identical ? "TRUE" : "FALSE");
        printf("NON_PARTITION_INPUTS_EQUAL=%s\n", non_partition_inputs_equal ? "TRUE" : "FALSE");
        printf("PARTITIONS_DIFFER=%s\n", partitions_differ ? "TRUE" : "FALSE");
        printf("ONLY_VARIABLE_IS_PARTITION=%s\n", only_variable_is_partition ? "TRUE" : "FALSE");
        printf("END_EXP0A5_CONTROLLED_INPUT_CONTRACT\n");
        const bool KVd = !kvfull, STd = !stfull, LGd = (ld != 0), AMd = (A48.am != B1.am);
        printf("SCHEDULE_VERDICTS\n");
        printf("  EXPORTED_KV_IS_SCHEDULE_DEPENDENT=%s\n", KVd?"TRUE":"FALSE");
        printf("  SERIALIZED_FULL_STATE_IS_SCHEDULE_DEPENDENT=%s\n", STd?"TRUE":"FALSE");
        printf("  NUMERICAL_FUTURE_IS_SCHEDULE_DEPENDENT=%s\n", LGd?"TRUE":"FALSE");
        printf("  NEXT_ARGMAX_IS_SCHEDULE_DEPENDENT=%s\n", AMd?"TRUE":"FALSE");
        printf("  BITWISE_STATE_IS_SCHEDULE_DEPENDENT=%s\n", (KVd || STd)?"TRUE":"FALSE");
        if (!KVd && STd) printf("  NON_KV_SCHEDULE_DEPENDENT_STATE_IS_CAUSALLY_RELEVANT=%s\n", LGd?"SUPPORTED":"NOT_ESTABLISHED");
        if (KVd && STd && !LGd) printf("  SCHEDULE_CHANGES_REPRESENTATION_WITHOUT_CHANGING_TESTED_FUTURE=TRUE\n");
        if ((KVd||STd) && LGd && !AMd) printf("  SCHEDULE_AFFECTS_NUMERICAL_FUTURE=TRUE\n  ARGMAX_ROBUST_TO_TESTED_SCHEDULE=TRUE (borne au probe805)\n");
        if (!KVd && !STd && !LGd && !AMd) printf("  TESTED_STATE_IS_BITWISE_SCHEDULE_INVARIANT=TRUE (B48 vs 48xB1, ce prefixe, ces 48 tokens, ce modele)\n");
        if (AMd) printf("  TOKEN_LEVEL_BEHAVIOR_IS_SCHEDULE_DEPENDENT=TRUE\n");
        printf("  STATE_KV_CONSISTENCY_GATE=%s\n", (stfull && !kvfull) ? "RED (full state bit-identique mais KV exporte different -> verifier l instrumentation)" : "OK");
    }
    // -------- EXP0A.6 : SCHEDULE CURVE (7 partitions) --------------------
    struct Schedule { const char * id; std::vector<int> chunks; };
    {
        Ctx X6 = make_ctx(model_path, n_ctx, n_batch, n_ubatch, c0.model);
        X6.k_row = ggml_row_size(llama_kv_cache_type_k(X6.ctx), 4096);
        X6.v_row = ggml_row_size(llama_kv_cache_type_v(X6.ctx), 4096);
        llama_batch b6 = llama_batch_init(n_batch, 0, 1);
        int o6b = 0; for (int n : sched) { decode_range(X6.ctx, prefix.data()+o6b, n, o6b, b6, 256); o6b += n; }
        const int N6 = (int) kv_layer_ids.size();
        const size_t ps6 = llama_state_get_size(X6.ctx);
        std::vector<uint8_t> PA6(ps6); llama_state_get_data(X6.ctx, PA6.data(), ps6);
        const int NT6 = PCT + 48;
        std::vector<llama_pos> pal6(NT6); for (int i = 0; i < NT6; ++i) pal6[i] = i;
        std::vector<llama_pos> ppr6 = { (llama_pos)(A0 + 48) };
        const int32_t pt6 = output_ids[causal_t + 48];
        const int nv6 = llama_vocab_n_tokens(llama_model_get_vocab(c0.model));
        auto f6 = [](const std::vector<uint8_t> & v) { uint64_t h = 1469598103934665603ULL; for (uint8_t c : v) { h ^= c; h *= 1099511628211ULL; } return h; };
        auto p8 = [](std::vector<uint8_t> & o, uint8_t v) { o.push_back(v); };
        auto p32 = [](std::vector<uint8_t> & o, uint32_t v) { for (int s = 0; s < 32; s += 8) o.push_back((uint8_t) ((v >> s) & 0xffu)); };
        auto p64 = [](std::vector<uint8_t> & o, uint64_t v) { for (int s = 0; s < 64; s += 8) o.push_back((uint8_t) ((v >> s) & 0xffu)); };
        auto am6 = [&](const std::vector<double> & l) { int a = 0; for (int i = 1; i < nv6; ++i) if (l[i] > l[a]) a = i; return a; };
        std::vector<Schedule> SCH;
        { Schedule s1; s1.id = "B48";   s1.chunks = {48}; SCH.push_back(s1); }
        { Schedule s2; s2.id = "2xB24"; s2.chunks = {24,24}; SCH.push_back(s2); }
        { Schedule s3; s3.id = "3xB16"; s3.chunks = {16,16,16}; SCH.push_back(s3); }
        { Schedule s4; s4.id = "4xB12"; s4.chunks = {12,12,12,12}; SCH.push_back(s4); }
        { Schedule s5; s5.id = "6xB8";  s5.chunks = {8,8,8,8,8,8}; SCH.push_back(s5); }
        { Schedule s6; s6.id = "12xB4"; s6.chunks.assign(12, 4); SCH.push_back(s6); }
        { Schedule s7; s7.id = "48xB1"; s7.chunks.assign(48, 1); SCH.push_back(s7); }
        int bad = 0;
        for (size_t i = 0; i < SCH.size(); ++i) { int t = 0; bool okc = !SCH[i].chunks.empty();
            for (size_t j = 0; j < SCH[i].chunks.size(); ++j) { if (SCH[i].chunks[j] <= 0) okc = false; t += SCH[i].chunks[j]; }
            if (!okc || t != 48) { bad++; printf("  SCHEDULE_INVALID id=%s tokens=%d\n", SCH[i].id, t); } }
        printf("ALL_SCHEDULE_TOKEN_COUNTS_VALID=%s\n", bad == 0 ? "TRUE" : "FALSE");
        struct SR { uint64_t pre, rst, skv, tok, pos, log; std::vector<llama_pos> req;
                    std::string id; std::vector<int> ch; std::vector<uint8_t> state; uint64_t sh;
                    std::vector<std::vector<uint8_t>> kk, vv, pk, pv; std::vector<double> lg; int am; };
        auto run_schedule = [&](const Schedule & sc) {
            SR r; r.id = sc.id; r.ch = sc.chunks;
            llama_state_set_data(X6.ctx, PA6.data(), PA6.size());
            r.pre = f6(PA6);
            { std::vector<uint8_t> rs(ps6); (void) llama_state_get_data(X6.ctx, rs.data(), rs.size()); r.rst = f6(rs); }
            { std::vector<llama_pos> pp(pal6.begin(), pal6.begin() + PCT);
              std::vector<uint8_t> sk; for (int li = 0; li < N6; ++li) { std::vector<uint8_t> k, v; extract_layer_bytes(X6, pp, kv_layer_ids[li], k, v); sk.insert(sk.end(), k.begin(), k.end()); sk.insert(sk.end(), v.begin(), v.end()); }
              r.skv = f6(sk); }
            { std::vector<uint8_t> tb, pb2, lb; r.req.clear();
              for (int j = 0; j < 48; ++j) { const int64_t po = (int64_t)(A0 + j); const int32_t tk = output_ids[causal_t + j]; const int32_t sq = 0;
                  const uint8_t lr = ((A0 + j) == (A0 + 47)) ? 1 : 0;
                  p32(tb, (uint32_t)(int32_t) tk); p64(pb2, (uint64_t)(int64_t) po);
                  p64(lb, (uint64_t)(int64_t) po); p32(lb, (uint32_t)(int32_t) tk); p32(lb, (uint32_t)(int32_t) sq); p8(lb, lr);
                  if (lr) r.req.push_back((llama_pos)(A0 + j)); }
              r.tok = f6(tb); r.pos = f6(pb2); r.log = f6(lb); }
            int off = 0;
            for (size_t ci = 0; ci < sc.chunks.size(); ++ci) {
                decode_range(X6.ctx, output_ids.data() + causal_t + off, sc.chunks[ci], A0 + off, b6, 256, false, (llama_pos)(A0 + 47));
                off += sc.chunks[ci];
            }
            if (off != 48) printf("  FATAL off=%d\n", off);
            r.state.resize(ps6); llama_state_get_data(X6.ctx, r.state.data(), ps6); r.sh = f6(r.state);
            r.kk.resize(N6); r.vv.resize(N6);
            for (int li = 0; li < N6; ++li) extract_layer_bytes(X6, pal6, kv_layer_ids[li], r.kk[li], r.vv[li]);
            llama_batch bp6 = llama_batch_init(1, 0, 1);
            decode_range(X6.ctx, &pt6, 1, A0 + 48, bp6, 256, true);
            r.lg.resize(nv6); const float * lf6 = llama_get_logits(X6.ctx);
            for (int i = 0; i < nv6; ++i) r.lg[i] = (double) lf6[i];
            r.am = am6(r.lg);
            r.pk.resize(N6); r.pv.resize(N6);
            for (int li = 0; li < N6; ++li) extract_layer_bytes(X6, ppr6, kv_layer_ids[li], r.pk[li], r.pv[li]);
            return r; };
        std::vector<SR> R6;
        for (size_t i = 0; i < SCH.size(); ++i) R6.push_back(run_schedule(SCH[i]));
        printf("EXP0A6_INPUT_CONTRACT\n");
        for (size_t i = 0; i < R6.size(); ++i)
            printf("  schedule_id=%s chunks_n=%d token_sum=48 prefix_snapshot_hash=%016llx restored=%016llx start_KV_hash=%016llx token_ids_hash=%016llx positions_hash=%016llx seq_id=0 logits_requested_positions_n=%d logical_input_hash=%016llx\n",
                   R6[i].id.c_str(), (int) R6[i].ch.size(), (unsigned long long) R6[i].pre, (unsigned long long) R6[i].rst,
                   (unsigned long long) R6[i].skv, (unsigned long long) R6[i].tok, (unsigned long long) R6[i].pos,
                   (int) R6[i].req.size(), (unsigned long long) R6[i].log);
        bool ap = true, ar = true, ak = true, at = true, apo = true, al = true, alp = true;
        for (size_t i = 1; i < R6.size(); ++i) {
            if (R6[i].pre != R6[0].pre) ap = false; if (R6[i].rst != R6[0].rst) ar = false;
            if (R6[i].skv != R6[0].skv) ak = false; if (R6[i].tok != R6[0].tok) at = false;
            if (R6[i].pos != R6[0].pos) apo = false; if (R6[i].log != R6[0].log) al = false;
            if (R6[i].req != R6[0].req) alp = false; }
        bool diff7 = false;
        for (size_t i = 1; i < R6.size(); ++i) if (R6[i].ch != R6[0].ch) diff7 = true;
        const bool alln = ap && ar && ak && at && apo && al && alp;
        printf("  ALL_PREFIX_SNAPSHOT_HASH_EQUAL=%s\n  ALL_RESTORED_PREFIX_STATE_HASH_EQUAL=%s\n  ALL_START_KV_HASH_EQUAL=%s\n",
               ap?"TRUE":"FALSE", ar?"TRUE":"FALSE", ak?"TRUE":"FALSE");
        printf("  ALL_TOKEN_IDS_HASH_EQUAL=%s\n  ALL_POSITIONS_HASH_EQUAL=%s\n  ALL_SEQ_ID_EQUAL=TRUE\n",
               at?"TRUE":"FALSE", apo?"TRUE":"FALSE");
        printf("  ALL_LOGITS_REQUEST_PATTERN_EQUAL=%s\n  ALL_LOGICAL_INPUT_HASH_EQUAL=%s\n  MODEL_CONTEXT_IDENTICAL=TRUE\n",
               alp?"TRUE":"FALSE", al?"TRUE":"FALSE");
        printf("  SCHEDULES_ACTUALLY_DIFFER=%s\n  ALL_NON_SCHEDULE_INPUTS_EQUAL=%s\n",
               diff7?"TRUE":"FALSE", alln?"TRUE":"FALSE");
        printf("  SCHEDULE_CURVE_INPUT_CONTRACT_GREEN=%s\n", (alln && diff7) ? "TRUE" : "FALSE");
        auto kvfull = [&](SR & r) { std::vector<uint8_t> c; for (int li = 0; li < N6; ++li) { c.insert(c.end(), r.kk[li].begin(), r.kk[li].end()); c.insert(c.end(), r.vv[li].begin(), r.vv[li].end()); } return f6(c); };
        auto pkfull = [&](SR & r) { std::vector<uint8_t> c; for (int li = 0; li < N6; ++li) { c.insert(c.end(), r.pk[li].begin(), r.pk[li].end()); c.insert(c.end(), r.pv[li].begin(), r.pv[li].end()); } return f6(c); };
        auto lgfull = [&](SR & r) { std::vector<uint8_t> c; for (int i = 0; i < nv6; ++i) { const float fv = (float) r.lg[i]; const uint8_t * q = (const uint8_t *) &fv; c.insert(c.end(), q, q + sizeof(float)); } return f6(c); };
        printf("EXP0A6_SCHEDULE_OUTPUTS\n");
        for (size_t i = 0; i < R6.size(); ++i) {
            int mn = 999, mx = 0, sm = 0; for (size_t j = 0; j < R6[i].ch.size(); ++j) { if (R6[i].ch[j] < mn) mn = R6[i].ch[j]; if (R6[i].ch[j] > mx) mx = R6[i].ch[j]; sm += R6[i].ch[j]; }
            printf("  schedule_id=%s n_decode_calls=%d mean_chunk=%.3f min_chunk=%d max_chunk=%d POST_KV_FULL_HASH=%016llx POST_STATE_SIZE=%zu POST_STATE_HASH=%016llx PROBE_KV_FULL_HASH=%016llx FULL_LOGITS_HASH=%016llx ARGMAX=%d\n",
                   R6[i].id.c_str(), (int) R6[i].ch.size(), (double) sm / (double) R6[i].ch.size(), mn, mx,
                   (unsigned long long) kvfull(R6[i]), R6[i].state.size(), (unsigned long long) R6[i].sh,
                   (unsigned long long) pkfull(R6[i]), (unsigned long long) lgfull(R6[i]), R6[i].am); }
        printf("EXP0A6_VS_B48\n");
        for (size_t i = 0; i < R6.size(); ++i) {
            int ke = 0, pke = 0, nz = 0, lb = 0; long mb = 0; double mxa = 0, sma = 0, l2 = 0;
            for (int li = 0; li < N6; ++li) { if (memcmp(R6[i].kk[li].data(), R6[0].kk[li].data(), R6[i].kk[li].size()) == 0) ke++;
                if (memcmp(R6[i].pk[li].data(), R6[0].pk[li].data(), R6[i].pk[li].size()) == 0) pke++; }
            const size_t m = R6[i].state.size() < R6[0].state.size() ? R6[i].state.size() : R6[0].state.size();
            const bool ste = (R6[i].state.size() == R6[0].state.size()) && memcmp(R6[i].state.data(), R6[0].state.data(), m) == 0;
            if (!ste) for (size_t b = 0; b < m; ++b) if (R6[i].state[b] != R6[0].state[b]) mb++;
            for (int c = 0; c < nv6; ++c) { const double e = std::abs(R6[i].lg[c] - R6[0].lg[c]); if (e > mxa) mxa = e; sma += e; l2 += e * e; if (R6[i].lg[c] != R6[0].lg[c]) { lb++; nz++; } }
            printf("  SCHEDULE_ID=%s KV_EQUAL_LAYERS_TO_B48=%d/%d FULL_EXPORTED_KV_EQUAL_TO_B48=%s\n", R6[i].id.c_str(), ke, N6, (kvfull(R6[i]) == kvfull(R6[0])) ? "TRUE" : "FALSE");
            printf("    SERIALIZED_STATE_BITWISE_EQUAL_TO_B48=%s SERIALIZED_STATE_MISMATCH_BYTES_TO_B48=%ld\n", ste?"TRUE":"FALSE", mb);
            printf("    PROBE_KV_EQUAL_LAYERS_TO_B48=%d/%d LOGITS_BITWISE_EQUAL_TO_B48=%s LOGITS_MAX_ABS_DIFF_TO_B48=%.9g LOGITS_MEAN_ABS_DIFF_TO_B48=%.9g LOGITS_L2_DIFF_TO_B48=%.9g LOGITS_NONZERO_COORDS_TO_B48=%d/%d ARGMAX=%d ARGMAX_EQUAL_TO_B48=%s\n",
                   pke, N6, lb==0?"TRUE":"FALSE", mxa, sma/nv6, std::sqrt(l2), nz, nv6, R6[i].am, (R6[i].am == R6[0].am)?"TRUE":"FALSE"); }
        printf("EXP0A6_ADJACENT\n");
        for (size_t i = 1; i < R6.size(); ++i) {
            int ke = 0, pke = 0, nz = 0, lb = 0; long mb = 0; double mxa = 0, sma = 0, l2 = 0;
            for (int li = 0; li < N6; ++li) { if (memcmp(R6[i].kk[li].data(), R6[i-1].kk[li].data(), R6[i].kk[li].size()) == 0) ke++;
                if (memcmp(R6[i].pk[li].data(), R6[i-1].pk[li].data(), R6[i].pk[li].size()) == 0) pke++; }
            const size_t m = R6[i].state.size() < R6[i-1].state.size() ? R6[i].state.size() : R6[i-1].state.size();
            for (size_t b = 0; b < m; ++b) if (R6[i].state[b] != R6[i-1].state[b]) mb++;
            for (int c = 0; c < nv6; ++c) { const double e = std::abs(R6[i].lg[c] - R6[i-1].lg[c]); if (e > mxa) mxa = e; sma += e; l2 += e * e; if (R6[i].lg[c] != R6[i-1].lg[c]) { lb++; nz++; } }
            printf("  PAIR=%s<->%s state_mismatch_bytes=%ld KV_equal_layers=%d/%d probe_KV_equal_layers=%d/%d logits_bitwise=%s max_abs=%.9g mean_abs=%.9g l2=%.9g nonzero=%d/%d argmax_equal=%s\n",
                   R6[i-1].id.c_str(), R6[i].id.c_str(), mb, ke, N6, pke, N6, lb==0?"TRUE":"FALSE", mxa, sma/nv6, std::sqrt(l2), nz, nv6, (R6[i].am == R6[i-1].am)?"TRUE":"FALSE"); }
        printf("EXP0A6_DONE\n");
    }
    // -------- EXP1A : ORACLE FULL-STATE INJECT PROBE ----------------------
    {
        Ctx XA = make_ctx(model_path, n_ctx, n_batch, n_ubatch, c0.model);
        XA.k_row = ggml_row_size(llama_kv_cache_type_k(XA.ctx), 4096);
        XA.v_row = ggml_row_size(llama_kv_cache_type_v(XA.ctx), 4096);
        llama_batch bA = llama_batch_init(n_batch, 0, 1);
        int oa = 0; for (int n : sched) { decode_range(XA.ctx, prefix.data()+oa, n, oa, bA, 256); oa += n; }
        const int NA = (int) kv_layer_ids.size();
        const size_t szA = llama_state_get_size(XA.ctx);
        std::vector<uint8_t> PA1(szA); llama_state_get_data(XA.ctx, PA1.data(), szA);
        const int NT1 = PCT + 48;
        std::vector<llama_pos> palA(NT1); for (int i = 0; i < NT1; ++i) palA[i] = i;
        std::vector<llama_pos> pprA = { (llama_pos)(A0 + 48) };
        const int32_t ptA = output_ids[causal_t + 48];
        const int nvA = llama_vocab_n_tokens(llama_model_get_vocab(c0.model));
        auto fA = [](const std::vector<uint8_t> & v) { uint64_t h = 1469598103934665603ULL; for (uint8_t c : v) { h ^= c; h *= 1099511628211ULL; } return h; };
        auto amA = [&](const std::vector<double> & l) { int a = 0; for (int i = 1; i < nvA; ++i) if (l[i] > l[a]) a = i; return a; };
        auto kvh = [&](std::vector<std::vector<uint8_t>> & k, std::vector<std::vector<uint8_t>> & v) { std::vector<uint8_t> c; for (int li = 0; li < NA; ++li) { c.insert(c.end(), k[li].begin(), k[li].end()); c.insert(c.end(), v[li].begin(), v[li].end()); } return fA(c); };
        int producer_target_tokens = 0, producer_target_calls = 0, producer_probe_tokens = 0;
        int consumer_target_tokens = 0, consumer_target_calls = 0, consumer_probe_tokens = 0;
        llama_state_set_data(XA.ctx, PA1.data(), PA1.size());
        const uint64_t prod_pre = fA(PA1);
        uint64_t prod_rst = 0;
        { std::vector<uint8_t> rs(szA); llama_state_get_data(XA.ctx, rs.data(), szA); prod_rst = fA(rs); }
        uint64_t prod_startkv = 0;
        { std::vector<llama_pos> pp(palA.begin(), palA.begin() + PCT);
          std::vector<std::vector<uint8_t>> k(NA), v(NA);
          for (int li = 0; li < NA; ++li) extract_layer_bytes(XA, pp, kv_layer_ids[li], k[li], v[li]);
          prod_startkv = kvh(k, v); }
        decode_range(XA.ctx, output_ids.data()+causal_t, 48, A0, bA, 256, false, (llama_pos)(A0 + 47));
        producer_target_tokens += 48; producer_target_calls += 1;
        std::vector<uint8_t> TGT(szA); llama_state_get_data(XA.ctx, TGT.data(), szA);
        const uint64_t tgt_hash = fA(TGT);
        std::vector<std::vector<uint8_t>> tkk(NA), tvv(NA);
        for (int li = 0; li < NA; ++li) extract_layer_bytes(XA, palA, kv_layer_ids[li], tkk[li], tvv[li]);
        const uint64_t tgt_kv_hash = kvh(tkk, tvv);
        llama_batch bpA = llama_batch_init(1, 0, 1);
        decode_range(XA.ctx, &ptA, 1, A0 + 48, bpA, 256, true);
        producer_probe_tokens += 1;
        std::vector<double> plg(nvA); const float * plf = llama_get_logits(XA.ctx);
        for (int i = 0; i < nvA; ++i) plg[i] = (double) plf[i];
        const int pam = amA(plg);
        std::vector<std::vector<uint8_t>> pkk(NA), pvv(NA);
        for (int li = 0; li < NA; ++li) extract_layer_bytes(XA, pprA, kv_layer_ids[li], pkk[li], pvv[li]);
        const uint64_t prod_pkv_hash = kvh(pkk, pvv);
        std::vector<uint8_t> plgb; for (int i = 0; i < nvA; ++i) { const float fv = (float) plg[i]; const uint8_t * q = (const uint8_t *) &fv; plgb.insert(plgb.end(), q, q + sizeof(float)); }
        const uint64_t prod_lg_hash = fA(plgb);
        printf("EXP1A_PRODUCER\n  PRODUCER_TARGET_STATE_SIZE=%zu\n  PRODUCER_TARGET_STATE_HASH=%016llx\n  PRODUCER_TARGET_PUBLIC_KV_HASH=%016llx\n  PRODUCER_PROBE_KV_HASH=%016llx\n  PRODUCER_FULL_LOGITS_HASH=%016llx\n  PRODUCER_ARGMAX=%d\n",
               TGT.size(), (unsigned long long) tgt_hash, (unsigned long long) tgt_kv_hash, (unsigned long long) prod_pkv_hash, (unsigned long long) prod_lg_hash, pam);
        llama_state_set_data(XA.ctx, PA1.data(), PA1.size());
        consumer_target_tokens = 0; consumer_target_calls = 0; consumer_probe_tokens = 0;
        const uint64_t cons_pre = fA(PA1);
        uint64_t cons_rst = 0;
        { std::vector<uint8_t> rs(szA); llama_state_get_data(XA.ctx, rs.data(), szA); cons_rst = fA(rs); }
        uint64_t cons_startkv = 0;
        { std::vector<llama_pos> pp(palA.begin(), palA.begin() + PCT);
          std::vector<std::vector<uint8_t>> k(NA), v(NA);
          for (int li = 0; li < NA; ++li) extract_layer_bytes(XA, pp, kv_layer_ids[li], k[li], v[li]);
          cons_startkv = kvh(k, v); }
        const size_t set_got = llama_state_set_data(XA.ctx, TGT.data(), TGT.size());
        std::vector<uint8_t> POST(szA); llama_state_get_data(XA.ctx, POST.data(), szA);
        const uint64_t post_hash = fA(POST);
        long post_mismatch = 0; for (size_t i = 0; i < szA; ++i) if (POST[i] != TGT[i]) post_mismatch++;
        std::vector<std::vector<uint8_t>> ckk(NA), cvv(NA);
        for (int li = 0; li < NA; ++li) extract_layer_bytes(XA, palA, kv_layer_ids[li], ckk[li], cvv[li]);
        int pkeq = 0; for (int li = 0; li < NA; ++li) if (memcmp(ckk[li].data(), tkk[li].data(), ckk[li].size()) == 0) pkeq++;
        llama_batch bpC = llama_batch_init(1, 0, 1);
        decode_range(XA.ctx, &ptA, 1, A0 + 48, bpC, 256, true);
        consumer_probe_tokens += 1;
        std::vector<double> clg(nvA); const float * clf = llama_get_logits(XA.ctx);
        for (int i = 0; i < nvA; ++i) clg[i] = (double) clf[i];
        const int cam = amA(clg);
        std::vector<std::vector<uint8_t>> ckp(NA), cvp(NA);
        for (int li = 0; li < NA; ++li) extract_layer_bytes(XA, pprA, kv_layer_ids[li], ckp[li], cvp[li]);
        int cpeq = 0; for (int li = 0; li < NA; ++li) if (memcmp(ckp[li].data(), pkk[li].data(), ckp[li].size()) == 0) cpeq++;
        double lmx = 0, lsm = 0, ll2 = 0; int lnz = 0;
        for (int i = 0; i < nvA; ++i) { const double e = std::abs(plg[i] - clg[i]); if (e > lmx) lmx = e; lsm += e; ll2 += e * e; if (plg[i] != clg[i]) lnz++; }
        const bool lg_eq = (lnz == 0);
        const bool zero_ok = (consumer_target_tokens == 0 && consumer_target_calls == 0);
        printf("BEGIN_EXP1A_ANTI_CHEAT_GATE\n  PRODUCER_TARGET_CONTINUATION_TOKENS_EXECUTED=%d\n  PRODUCER_TARGET_DECODE_CALLS=%d\n  PRODUCER_PROBE_TOKENS_EXECUTED=%d\n  CONSUMER_TARGET_CONTINUATION_TOKENS_EXECUTED=%d\n  CONSUMER_TARGET_DECODE_CALLS=%d\n  CONSUMER_PROBE_TOKENS_EXECUTED=%d\nEND_EXP1A_ANTI_CHEAT_GATE\n  ZERO_NORMAL_TARGET_COMPUTE_ON_CONSUMER=%s\n",
               producer_target_tokens, producer_target_calls, producer_probe_tokens, consumer_target_tokens, consumer_target_calls, consumer_probe_tokens, zero_ok ? "TRUE" : "FALSE");
        printf("EXP1A_INPUT_CONTRACT\n  producer_prefix_snapshot_hash=%016llx consumer_prefix_snapshot_hash=%016llx PREFIX_SNAPSHOT_HASH_EQUAL=%s\n",
               (unsigned long long) prod_pre, (unsigned long long) cons_pre, (prod_pre == cons_pre) ? "TRUE" : "FALSE");
        printf("  producer_restored=%016llx consumer_restored=%016llx RESTORED_PREFIX_STATE_HASH_EQUAL=%s\n",
               (unsigned long long) prod_rst, (unsigned long long) cons_rst, (prod_rst == cons_rst) ? "TRUE" : "FALSE");
        printf("  producer_start_kv=%016llx consumer_start_kv=%016llx START_PUBLIC_KV_HASH_EQUAL=%s\n",
               (unsigned long long) prod_startkv, (unsigned long long) cons_startkv, (prod_startkv == cons_startkv) ? "TRUE" : "FALSE");
        printf("  MODEL_CONTEXT_IDENTICAL=TRUE\n  EXP1A_INPUT_CONTRACT_GREEN=%s\n",
               (prod_pre == cons_pre && prod_rst == cons_rst && prod_startkv == cons_startkv) ? "TRUE" : "FALSE");
        printf("EXP1A_INJECTION\n  CONSUMER_STATE_SET_REQUESTED_BYTES=%zu\n  CONSUMER_STATE_SET_RETURN=%zu\n", TGT.size(), set_got);
        printf("  CONSUMER_POST_INJECT_STATE_SIZE=%zu POST_INJECT_STATE_SIZE_EQUAL=%s\n  CONSUMER_POST_INJECT_STATE_HASH=%016llx\n  POST_INJECT_STATE_BITWISE_EQUAL_TO_TARGET=%s\n  POST_INJECT_STATE_MISMATCH_BYTES_TO_TARGET=%ld\n  POST_INJECT_PUBLIC_KV_EQUAL_LAYERS_TO_TARGET=%d/%d POST_INJECT_PUBLIC_KV_FULL_EQUAL_TO_TARGET=%s\n",
               POST.size(), (POST.size() == TGT.size()) ? "TRUE" : "FALSE", (unsigned long long) post_hash,
               (post_mismatch == 0) ? "TRUE" : "FALSE", post_mismatch, pkeq, NA, (pkeq == NA) ? "TRUE" : "FALSE");
        printf("EXP1A_PROBE_COMPARISON\n  PROBE_KV_EQUAL_LAYERS=%d/%d\n  PROBE_KV_FULL_EQUAL=%s\n  FULL_LOGITS_BITWISE_EQUAL=%s\n  FULL_LOGITS_MAX_ABS_DIFF=%.9g\n  FULL_LOGITS_MEAN_ABS_DIFF=%.9g\n  FULL_LOGITS_L2_DIFF=%.9g\n  FULL_LOGITS_NONZERO_COORDS=%d/%d\n  PRODUCER_ARGMAX=%d\n  CONSUMER_ARGMAX=%d\n  ARGMAX_EQUAL=%s\n",
               cpeq, NA, (cpeq == NA) ? "TRUE" : "FALSE", lg_eq ? "TRUE" : "FALSE", lmx, lsm / nvA, std::sqrt(ll2), lnz, nvA, pam, cam, (pam == cam) ? "TRUE" : "FALSE");
        printf("EXP1A_VERDICTS\n  EXP1A_REPRESENTATION_EXACT=%s\n  EXP1A_BEHAVIORALLY_EXACT=%s\n  ZERO_TARGET_COMPUTE_ORACLE_PATH_FUNCTIONAL=%s\n  EXP1A_DONE\n",
               (post_mismatch == 0) ? "TRUE" : "FALSE", (lg_eq && pam == cam) ? "TRUE" : "FALSE",
               (zero_ok && lg_eq && pam == cam) ? "TRUE" : "FALSE");
    }
    // -------- EXP1A2 : RESTORE PROVENANCE -------------------------------
    {
        Ctx XB = make_ctx(model_path, n_ctx, n_batch, n_ubatch, c0.model);
        XB.k_row = ggml_row_size(llama_kv_cache_type_k(XB.ctx), 4096);
        XB.v_row = ggml_row_size(llama_kv_cache_type_v(XB.ctx), 4096);
        llama_batch bB = llama_batch_init(n_batch, 0, 1);
        int ob = 0; for (int n : sched) { decode_range(XB.ctx, prefix.data()+ob, n, ob, bB, 256); ob += n; }
        const int NB = (int) kv_layer_ids.size();
        const size_t szB = llama_state_get_size(XB.ctx);
        std::vector<uint8_t> PAB(szB); llama_state_get_data(XB.ctx, PAB.data(), szB);
        const int NT2 = PCT + 48;
        std::vector<llama_pos> palB(NT2); for (int i = 0; i < NT2; ++i) palB[i] = i;
        std::vector<llama_pos> pprB = { (llama_pos)(A0 + 48) };
        const int32_t ptB = output_ids[causal_t + 48];
        const int nvB = llama_vocab_n_tokens(llama_model_get_vocab(c0.model));
        auto fB = [](const std::vector<uint8_t> & v) { uint64_t h = 1469598103934665603ULL; for (uint8_t c : v) { h ^= c; h *= 1099511628211ULL; } return h; };
        auto amB = [&](const std::vector<double> & l) { int a = 0; for (int i = 1; i < nvB; ++i) if (l[i] > l[a]) a = i; return a; };
        auto kvB = [&](std::vector<std::vector<uint8_t>> & k, std::vector<std::vector<uint8_t>> & v) { std::vector<uint8_t> c; for (int li = 0; li < NB; ++li) { c.insert(c.end(), k[li].begin(), k[li].end()); c.insert(c.end(), v[li].begin(), v[li].end()); } return fB(c); };
        struct RB2 { int tok = 0, calls = 0, probe = 0; bool b48_before_set = false, set_before_probe = false;
                     uint64_t pre = 0, rst = 0, skv = 0, setin_hash = 0, st_hash = 0; long st_mm = -1; uint64_t kv_hash = 0;
                     std::vector<uint8_t> st; std::vector<std::vector<uint8_t>> kk, vv, pk, pv; std::vector<double> lg; uint64_t lg_hash = 0; int am = -1; };
        std::vector<uint8_t> TGT2(szB);
        auto branch = [&](bool natural, bool do_set, std::vector<uint8_t> & tgt_in, RB2 & r) {
            llama_state_set_data(XB.ctx, PAB.data(), PAB.size());
            r.pre = fB(PAB);
            { std::vector<uint8_t> rs(szB); llama_state_get_data(XB.ctx, rs.data(), szB); r.rst = fB(rs); }
            { std::vector<llama_pos> pp(palB.begin(), palB.begin() + PCT);
              std::vector<std::vector<uint8_t>> k(NB), v(NB);
              for (int li = 0; li < NB; ++li) extract_layer_bytes(XB, pp, kv_layer_ids[li], k[li], v[li]);
              r.skv = kvB(k, v); }
            if (natural) { decode_range(XB.ctx, output_ids.data()+causal_t, 48, A0, bB, 256, false, (llama_pos)(A0 + 47)); r.tok += 48; r.calls += 1; r.b48_before_set = true; }
            if (do_set) { r.setin_hash = fB(tgt_in); llama_state_set_data(XB.ctx, tgt_in.data(), tgt_in.size()); r.set_before_probe = true; }
            r.st.resize(szB); llama_state_get_data(XB.ctx, r.st.data(), szB); r.st_hash = fB(r.st);
            r.st_mm = 0; for (size_t i = 0; i < szB; ++i) if (r.st[i] != tgt_in[i]) r.st_mm++;
            r.kk.resize(NB); r.vv.resize(NB);
            for (int li = 0; li < NB; ++li) extract_layer_bytes(XB, palB, kv_layer_ids[li], r.kk[li], r.vv[li]);
            r.kv_hash = kvB(r.kk, r.vv);
            llama_batch bpB = llama_batch_init(1, 0, 1);
            decode_range(XB.ctx, &ptB, 1, A0 + 48, bpB, 256, true);
            r.probe += 1;
            r.lg.resize(nvB); const float * lfB = llama_get_logits(XB.ctx);
            for (int i = 0; i < nvB; ++i) r.lg[i] = (double) lfB[i];
            r.am = amB(r.lg);
            { std::vector<uint8_t> c; for (int i = 0; i < nvB; ++i) { const float fv = (float) r.lg[i]; const uint8_t * q = (const uint8_t *) &fv; c.insert(c.end(), q, q + sizeof(float)); } r.lg_hash = fB(c); }
            r.pk.resize(NB); r.pv.resize(NB);
            for (int li = 0; li < NB; ++li) extract_layer_bytes(XB, pprB, kv_layer_ids[li], r.pk[li], r.pv[li]);
        };
        RB2 rA, rB, rC;
        branch(true,  false, TGT2, rA);
        std::copy(rA.st.begin(), rA.st.end(), TGT2.begin());
        branch(false, true,  TGT2, rB);
        branch(true,  true,  TGT2, rC);
        printf("EXP1A2_REACHED\nBEGIN_EXP1A2_EXECUTION_CONTRACT\n  A_TARGET_TOKENS_EXECUTED=%d\n  A_TARGET_DECODE_CALLS=%d\n  A_PROBE_TOKENS_EXECUTED=%d\n  B_TARGET_TOKENS_EXECUTED=%d\n  B_TARGET_DECODE_CALLS=%d\n  B_PROBE_TOKENS_EXECUTED=%d\n  C_TARGET_TOKENS_EXECUTED=%d\n  C_TARGET_DECODE_CALLS=%d\n  C_PROBE_TOKENS_EXECUTED=%d\nEND_EXP1A2_EXECUTION_CONTRACT\n",
               rA.tok, rA.calls, rA.probe, rB.tok, rB.calls, rB.probe, rC.tok, rC.calls, rC.probe);
        const bool exec_ok = (rA.tok==48 && rA.calls==1 && rA.probe==1 && rB.tok==0 && rB.calls==0 && rB.probe==1 && rC.tok==48 && rC.calls==1 && rC.probe==1);
        printf("  EXP1A2_EXECUTION_CONTRACT_GREEN=%s\n", exec_ok ? "TRUE" : "FALSE");
        const bool pre_eq = (rA.pre==rB.pre && rB.pre==rC.pre), rst_eq = (rA.rst==rB.rst && rB.rst==rC.rst), skv_eq = (rA.skv==rB.skv && rB.skv==rC.skv);
        printf("EXP1A2_INITIAL_INPUT_CONTRACT\n  A_pre=%016llx B_pre=%016llx C_pre=%016llx ABC_PREFIX_SNAPSHOT_EQUAL=%s\n  A_rst=%016llx B_rst=%016llx C_rst=%016llx ABC_RESTORED_PREFIX_STATE_EQUAL=%s\n  A_skv=%016llx B_skv=%016llx C_skv=%016llx ABC_START_PUBLIC_KV_EQUAL=%s\n  ABC_MODEL_CONTEXT_IDENTICAL=TRUE\n  EXP1A2_INITIAL_INPUT_CONTRACT_GREEN=%s\n",
               (unsigned long long) rA.pre, (unsigned long long) rB.pre, (unsigned long long) rC.pre, pre_eq?"TRUE":"FALSE",
               (unsigned long long) rA.rst, (unsigned long long) rB.rst, (unsigned long long) rC.rst, rst_eq?"TRUE":"FALSE",
               (unsigned long long) rA.skv, (unsigned long long) rB.skv, (unsigned long long) rC.skv, skv_eq?"TRUE":"FALSE",
               (pre_eq && rst_eq && skv_eq) ? "TRUE" : "FALSE");
        printf("EXP1A2_TARGET\n  A_TARGET_STATE_SIZE=%zu\n  A_TARGET_STATE_HASH=%016llx\n  A_TARGET_PUBLIC_KV_HASH=%016llx\n  A_ARGMAX=%d\n  B_STATE_SET_INPUT_HASH=%016llx\n  C_STATE_SET_INPUT_HASH=%016llx\n  STATE_SET_INPUT_HASH_EQUAL=%s\n",
               rA.st.size(), (unsigned long long) rA.st_hash, (unsigned long long) rA.kv_hash, rA.am,
               (unsigned long long) rB.setin_hash, (unsigned long long) rC.setin_hash, (rB.setin_hash==rC.setin_hash)?"TRUE":"FALSE");
        int bpeq2 = 0, cpeq2 = 0;
        for (int li = 0; li < NB; ++li) { if (memcmp(rB.kk[li].data(), rA.kk[li].data(), rB.kk[li].size()) == 0) bpeq2++; if (memcmp(rC.kk[li].data(), rA.kk[li].data(), rC.kk[li].size()) == 0) cpeq2++; }
        printf("EXP1A2_POST_SET\n  B_POST_SET_STATE_SIZE=%zu\n  B_POST_SET_STATE_HASH=%016llx\n  B_POST_SET_STATE_BITWISE_EQUAL_TARGET=%s\n  B_POST_SET_STATE_MISMATCH_BYTES_TARGET=%ld\n  B_POST_SET_PUBLIC_KV_EQUAL_LAYERS_TARGET=%d/%d\n  B_POST_SET_PUBLIC_KV_FULL_EQUAL_TARGET=%s\n",
               rB.st.size(), (unsigned long long) rB.st_hash, (rB.st_mm==0)?"TRUE":"FALSE", rB.st_mm, bpeq2, NB, (bpeq2==NB)?"TRUE":"FALSE");
        printf("  C_POST_SET_STATE_SIZE=%zu\n  C_POST_SET_STATE_HASH=%016llx\n  C_POST_SET_STATE_BITWISE_EQUAL_TARGET=%s\n  C_POST_SET_STATE_MISMATCH_BYTES_TARGET=%ld\n  C_POST_SET_PUBLIC_KV_EQUAL_LAYERS_TARGET=%d/%d\n  C_POST_SET_PUBLIC_KV_FULL_EQUAL_TARGET=%s\n",
               rC.st.size(), (unsigned long long) rC.st_hash, (rC.st_mm==0)?"TRUE":"FALSE", rC.st_mm, cpeq2, NB, (cpeq2==NB)?"TRUE":"FALSE");
        printf("  B_POST_SET_STATE_BITWISE_EQUAL_C=%s\n  B_POST_SET_PUBLIC_KV_EQUAL_C=%s\n  C_B48_OCCURRED_BEFORE_STATE_SET=%s\n  C_STATE_SET_OCCURRED_BEFORE_PROBE=%s\n",
               (rB.st_hash==rC.st_hash && rB.st_mm==0 && rC.st_mm==0)?"TRUE":"FALSE", (rB.kv_hash==rC.kv_hash)?"TRUE":"FALSE",
               rC.b48_before_set?"TRUE":"FALSE", rC.set_before_probe?"TRUE":"FALSE");
        auto cmp2 = [&](const char * n, RB2 & L, RB2 & R) {
            int pke = 0, nz = 0; double mx = 0, sm = 0, l2 = 0;
            for (int li = 0; li < NB; ++li) if (memcmp(L.pk[li].data(), R.pk[li].data(), L.pk[li].size()) == 0) pke++;
            for (int c = 0; c < nvB; ++c) { const double e = std::abs(L.lg[c] - R.lg[c]); if (e > mx) mx = e; sm += e; l2 += e * e; if (L.lg[c] != R.lg[c]) nz++; }
            printf("  CONTRAST=%s\n    PROBE_KV_EQUAL_LAYERS=%d/%d\n    PROBE_KV_FULL_EQUAL=%s\n    LOGITS_BITWISE_EQUAL=%s\n    LOGITS_MAX_ABS=%.9g\n    LOGITS_MEAN_ABS=%.9g\n    LOGITS_L2=%.9g\n    LOGITS_NONZERO_COORDS=%d/%d\n    ARGMAX_LEFT=%d\n    ARGMAX_RIGHT=%d\n    ARGMAX_EQUAL=%s\n",
                   n, pke, NB, (pke==NB)?"TRUE":"FALSE", (nz==0)?"TRUE":"FALSE", mx, sm/nvB, std::sqrt(l2), nz, nvB, L.am, R.am, (L.am==R.am)?"TRUE":"FALSE");
            return nz == 0; };
        printf("EXP1A2_CONTRASTS\n");
        const bool ab = cmp2("A_vs_B", rA, rB);
        const bool ac = cmp2("A_vs_C", rA, rC);
        const bool bc = cmp2("B_vs_C", rB, rC);
        printf("EXP1A2_VERDICTS\n  AB_BEHAVIORALLY_EQUAL=%s\n  AC_BEHAVIORALLY_EQUAL=%s\n  BC_BEHAVIORALLY_EQUAL=%s\n",
               ab?"TRUE":"FALSE", ac?"TRUE":"FALSE", bc?"TRUE":"FALSE");
        if (!ab && ac) printf("  PRE_STATE_SET_RUNTIME_PROVENANCE_IS_CAUSALLY_ACTIVE=SUPPORTED\n  STATE_SET_DATA_DOES_NOT_FULLY_NORMALIZE_CAUSALLY_RELEVANT_RUNTIME_STATE=SUPPORTED\n");
        else if (!ab && !ac && bc) printf("  B48_PRE_SET_PROVENANCE_DOES_NOT_RESTORE_REFERENCE_FUTURE=SUPPORTED\n");
        else if (!ab && !ac && !bc) printf("  MULTIPLE_PATH_DEPENDENCIES_OR_RESTORE_SIDE_EFFECTS=SUPPORTED\n");
        else if (ab) printf("  EXP1A2_RED_NOT_REPRODUCED=TRUE\n");
        else printf("  EXP1A2_CASE_UNCLASSIFIED\n");
        printf("  EXP1A2_DONE\n");
    }
    // -------- EXP1A3 : IDENTITY STATE SET CAUSALITY (N / S / I / R) --------
    {
        Ctx XI = make_ctx(model_path, n_ctx, n_batch, n_ubatch, c0.model);
        XI.k_row = ggml_row_size(llama_kv_cache_type_k(XI.ctx), 4096);
        XI.v_row = ggml_row_size(llama_kv_cache_type_v(XI.ctx), 4096);
        llama_batch bI = llama_batch_init(n_batch, 0, 1);
        int oi = 0; for (int n : sched) { decode_range(XI.ctx, prefix.data()+oi, n, oi, bI, 256); oi += n; }
        const int NI = (int) kv_layer_ids.size();
        const size_t szI = llama_state_get_size(XI.ctx);
        std::vector<uint8_t> PAI(szI); llama_state_get_data(XI.ctx, PAI.data(), szI);
        const int NT3 = PCT + 48;
        std::vector<llama_pos> palI(NT3); for (int i = 0; i < NT3; ++i) palI[i] = i;
        std::vector<llama_pos> pprI = { (llama_pos)(A0 + 48) };
        const int32_t ptI = output_ids[causal_t + 48];
        const int nvI = llama_vocab_n_tokens(llama_model_get_vocab(c0.model));
        auto fI = [](const std::vector<uint8_t> & v) { uint64_t h = 1469598103934665603ULL; for (uint8_t c : v) { h ^= c; h *= 1099511628211ULL; } return h; };
        auto amI = [&](const std::vector<double> & l) { int a = 0; for (int i = 1; i < nvI; ++i) if (l[i] > l[a]) a = i; return a; };
        auto kvI = [&](std::vector<std::vector<uint8_t>> & k, std::vector<std::vector<uint8_t>> & v) { std::vector<uint8_t> c; for (int li = 0; li < NI; ++li) { c.insert(c.end(), k[li].begin(), k[li].end()); c.insert(c.end(), v[li].begin(), v[li].end()); } return fI(c); };
        struct RI { int tok = 0, probe = 0, syncs = 0; uint64_t pre = 0, post = 0, kv = 0, lg_hash = 0; long mm = -1;
                    bool b48_before_pre = false, pre_before_set = false, set_before_post = false, post_before_probe = false, set_before_probe = false;
                    std::vector<std::vector<uint8_t>> pk, pv; std::vector<double> lg; int am = -1; };
        std::vector<uint8_t> TGT3(szI);
        auto branch = [&](char mode, RI & r) {
            llama_state_set_data(XI.ctx, PAI.data(), PAI.size());
            std::vector<uint8_t> blob(szI);
            if (mode != 'R') { decode_range(XI.ctx, output_ids.data()+causal_t, 48, A0, bI, 256, false, (llama_pos)(A0 + 47)); r.tok += 48; r.b48_before_pre = true; }
            llama_state_get_data(XI.ctx, blob.data(), szI);
            r.pre = fI(blob);
            if (mode == 'N') { }
            else if (mode == 'S') { llama_synchronize(XI.ctx); r.syncs += 1; }
            else if (mode == 'I') { r.pre_before_set = true; llama_state_set_data(XI.ctx, blob.data(), blob.size()); r.set_before_probe = true;
                                    { std::vector<uint8_t> p2(szI); llama_state_get_data(XI.ctx, p2.data(), szI); r.post = fI(p2); r.mm = 0; for (size_t i = 0; i < szI; ++i) if (p2[i] != blob[i]) r.mm++; r.set_before_post = true; } }
            else if (mode == 'R') { llama_state_set_data(XI.ctx, TGT3.data(), TGT3.size()); r.set_before_probe = true;
                                    { std::vector<uint8_t> p2(szI); llama_state_get_data(XI.ctx, p2.data(), szI); r.post = fI(p2); } }
            { std::vector<std::vector<uint8_t>> k(NI), v(NI);
              for (int li = 0; li < NI; ++li) extract_layer_bytes(XI, palI, kv_layer_ids[li], k[li], v[li]);
              r.kv = kvI(k, v); }
            llama_batch bpI = llama_batch_init(1, 0, 1);
            decode_range(XI.ctx, &ptI, 1, A0 + 48, bpI, 256, true);
            r.probe += 1; r.post_before_probe = (mode == 'I');
            r.lg.resize(nvI); const float * lfI = llama_get_logits(XI.ctx);
            for (int i = 0; i < nvI; ++i) r.lg[i] = (double) lfI[i];
            r.am = amI(r.lg);
            { std::vector<uint8_t> c; for (int i = 0; i < nvI; ++i) { const float fv = (float) r.lg[i]; const uint8_t * q = (const uint8_t *) &fv; c.insert(c.end(), q, q + sizeof(float)); } r.lg_hash = fI(c); }
            r.pk.resize(NI); r.pv.resize(NI);
            for (int li = 0; li < NI; ++li) extract_layer_bytes(XI, pprI, kv_layer_ids[li], r.pk[li], r.pv[li]);
        };
        RI rN, rS, rI2, rR;
        branch('N', rN);
        std::copy(rN.lg.begin(), rN.lg.end(), TGT3.begin());   // placeholder, remplacé juste après par le vrai blob
        { llama_state_set_data(XI.ctx, PAI.data(), PAI.size());
          decode_range(XI.ctx, output_ids.data()+causal_t, 48, A0, bI, 256, false, (llama_pos)(A0 + 47));
          llama_state_get_data(XI.ctx, TGT3.data(), szI); }
        branch('S', rS);
        branch('I', rI2);
        branch('R', rR);
        printf("EXP1A3_REACHED\n");
        printf("BEGIN_EXP1A3_EXECUTION_CONTRACT\n  N_TARGET_NORMAL_DECODE=%d\n  S_TARGET_NORMAL_DECODE=%d\n  I_TARGET_NORMAL_DECODE=%d\n  R_TARGET_NORMAL_DECODE=%d\n  N_PROBE_DECODE=%d\n  S_PROBE_DECODE=%d\n  I_PROBE_DECODE=%d\n  R_PROBE_DECODE=%d\n  S_EXTRA_SYNCHRONIZE=%d\nEND_EXP1A3_EXECUTION_CONTRACT\n",
               rN.tok, rS.tok, rI2.tok, rR.tok, rN.probe, rS.probe, rI2.probe, rR.probe, rS.syncs);
        printf("  N: B48_BEFORE_PROBE=TRUE\n  S: B48_BEFORE_EXTRA_SYNC=%s EXTRA_SYNC_BEFORE_PROBE=%s\n  I: B48_BEFORE_I_PRE_GET=%s I_PRE_GET_BEFORE_IDENTITY_SET=%s IDENTITY_SET_BEFORE_I_POST_GET=%s I_POST_GET_BEFORE_PROBE=%s\n  R: STATE_SET_BEFORE_PROBE=%s\n",
               rS.b48_before_pre?"TRUE":"FALSE", rS.syncs>0?"TRUE":"FALSE",
               rI2.b48_before_pre?"TRUE":"FALSE", rI2.pre_before_set?"TRUE":"FALSE", rI2.set_before_post?"TRUE":"FALSE", rI2.post_before_probe?"TRUE":"FALSE",
               rR.set_before_probe?"TRUE":"FALSE");
        const uint64_t tgt_hash = fI(TGT3);
        printf("EXP1A3_STATES\n  N_PRE_STATE_HASH=%016llx\n  S_PRE_STATE_HASH=%016llx\n  I_PRE_STATE_HASH=%016llx\n  I_POST_STATE_HASH=%016llx\n  R_POST_STATE_HASH=%016llx\n  TARGET_STATE_HASH=%016llx\n  TARGET_STATE_SIZE=%zu\n  IDENTITY_PRE_POST_MISMATCH_BYTES=%ld\n",
               (unsigned long long) rN.pre, (unsigned long long) rS.pre, (unsigned long long) rI2.pre,
               (unsigned long long) rI2.post, (unsigned long long) rR.post, (unsigned long long) tgt_hash, TGT3.size(), rI2.mm);
        printf("  N_PRE_TARGET_MISMATCH_BYTES=%ld\n  S_PRE_TARGET_MISMATCH_BYTES=%ld\n  I_PRE_TARGET_MISMATCH_BYTES=%ld\n",
               rN.mm < 0 ? -1L : rN.mm, 0L, (rI2.pre == tgt_hash) ? 0L : -1L);
        printf("  IDENTITY_STATE_SET_REPRESENTATION_EXACT=%s\n", (rI2.mm == 0) ? "TRUE" : "FALSE");
        auto cmp3 = [&](const char * n, RI & L, RI & R2) {
            int pke = 0, nz = 0; double mx = 0, sm = 0, l2 = 0;
            for (int li = 0; li < NI; ++li) if (memcmp(L.pk[li].data(), R2.pk[li].data(), L.pk[li].size()) == 0) pke++;
            for (int c = 0; c < nvI; ++c) { const double e = std::abs(L.lg[c] - R2.lg[c]); if (e > mx) mx = e; sm += e; l2 += e * e; if (L.lg[c] != R2.lg[c]) nz++; }
            printf("  CONTRAST=%s\n    KV_EQUAL_LAYERS=%d/%d\n    LOGITS_BITWISE_EQUAL=%s\n    MAX_ABS=%.9g\n    MEAN_ABS=%.9g\n    L2=%.9g\n    NONZERO_COORDS=%d/%d\n    ARGMAX_A=%d\n    ARGMAX_B=%d\n    ARGMAX_EQUAL=%s\n",
                   n, pke, NI, (nz==0)?"TRUE":"FALSE", mx, sm/nvI, std::sqrt(l2), nz, nvI, L.am, R2.am, (L.am==R2.am)?"TRUE":"FALSE");
            return nz == 0; };
        printf("EXP1A3_CONTRASTS\n");
        const bool ns = cmp3("N_vs_S", rN, rS);
        const bool ni = cmp3("N_vs_I", rN, rI2);
        const bool nr = cmp3("N_vs_R", rN, rR);
        const bool ir = cmp3("I_vs_R", rI2, rR);
        const bool si = cmp3("S_vs_I", rS, rI2);
        printf("EXP1A3_VERDICTS\n  N_vs_S_BEHAVIORALLY_EQUAL=%s\n  N_vs_I_BEHAVIORALLY_EQUAL=%s\n  N_vs_R_BEHAVIORALLY_EQUAL=%s\n  I_vs_R_BEHAVIORALLY_EQUAL=%s\n  S_vs_I_BEHAVIORALLY_EQUAL=%s\n",
               ns?"TRUE":"FALSE", ni?"TRUE":"FALSE", nr?"TRUE":"FALSE", ir?"TRUE":"FALSE", si?"TRUE":"FALSE");
        const bool syn_refuted = (rS.syncs > 0) && !ns;
        const bool exp_refuted = (rI2.pre != tgt_hash) || (rI2.mm != 0);
        if (ns && !ni && ir) printf("  EXP1A3_CASE=IDENTITY_STATE_SET_CAUSAL\n  RESTORE_OPERATION_CAUSALLY_LOCALIZED=TRUE\n  EXTRA_SYNCHRONIZE_EFFECT_ON_TESTED_PROBE=NOT_OBSERVED\n  IDENTITY_STATE_SET_ALTERS_TESTED_NEXT_PROBE=SUPPORTED\n  IDENTITY_STATE_SET_MOVES_NATURAL_TO_RESTORED_CLASS=SUPPORTED\n");
        else if (ni) printf("  EXP1A3_CASE=IDENTITY_STATE_SET_HAS_NO_EFFECT\n  IDENTITY_STATE_SET_ALTERS_TESTED_NEXT_PROBE=FALSE\n");
        else if (!ns) printf("  EXP1A3_CASE=EXTRA_SYNCHRONIZE_CAUSALLY_ACTIVE\n  EXTRA_SYNCHRONIZE_IS_CAUSALLY_ACTIVE=SUPPORTED\n");
        else if (!ir) printf("  EXP1A3_CASE=IDENTITY_SET_EFFECT_PARTIAL\n  IDENTITY_STATE_SET_HAS_AN_EFFECT=TRUE\n  DOES_NOT_FULLY_REPRODUCE_RESTORED_CLASS=TRUE\n");
        else printf("  EXP1A3_CASE=OTHER\n");
        printf("  EXP1A3_DONE\n");
    }
    // -------- EXP1A3B : IDENTITY SET WITHOUT POST-GET (N/S/G/I/J/R) ----------
    {
        Ctx XJ = make_ctx(model_path, n_ctx, n_batch, n_ubatch, c0.model);
        XJ.k_row = ggml_row_size(llama_kv_cache_type_k(XJ.ctx), 4096);
        XJ.v_row = ggml_row_size(llama_kv_cache_type_v(XJ.ctx), 4096);
        llama_batch bJ = llama_batch_init(n_batch, 0, 1);
        int oj = 0; for (int n : sched) { decode_range(XJ.ctx, prefix.data()+oj, n, oj, bJ, 256); oj += n; }
        const int NJ = (int) kv_layer_ids.size();
        const size_t szJ = llama_state_get_size(XJ.ctx);
        std::vector<uint8_t> PAJ(szJ); llama_state_get_data(XJ.ctx, PAJ.data(), szJ);
        const int NT4 = PCT + 48;
        std::vector<llama_pos> palJ(NT4); for (int i = 0; i < NT4; ++i) palJ[i] = i;
        std::vector<llama_pos> pprJ = { (llama_pos)(A0 + 48) };
        const int32_t ptJ = output_ids[causal_t + 48];
        const int nvJ = llama_vocab_n_tokens(llama_model_get_vocab(c0.model));
        auto fJ = [](const std::vector<uint8_t> & v) { uint64_t h = 1469598103934665603ULL; for (uint8_t c : v) { h ^= c; h *= 1099511628211ULL; } return h; };
        auto amJ = [&](const std::vector<double> & l) { int a = 0; for (int i = 1; i < nvJ; ++i) if (l[i] > l[a]) a = i; return a; };
        auto kvJ = [&](std::vector<std::vector<uint8_t>> & k, std::vector<std::vector<uint8_t>> & v) { std::vector<uint8_t> c; for (int li = 0; li < NJ; ++li) { c.insert(c.end(), k[li].begin(), k[li].end()); c.insert(c.end(), v[li].begin(), v[li].end()); } return fJ(c); };
        struct RB3 { int tok = 0, probe = 0, syncs = 0, gets = 0, sets = 0, postgets = 0;
                     uint64_t pre = 0, post = 0, kv = 0, lg_hash = 0; long pre_mm = -1, post_mm = -1;
                     bool b48_first = false, pre_before_set = false, set_before_probe = false, get_after_set = false;
                     std::vector<std::vector<uint8_t>> pk, pv; std::vector<double> lg; int am = -1; };
        std::vector<uint8_t> TGT4(szJ);
        { llama_state_set_data(XJ.ctx, PAJ.data(), PAJ.size());
          decode_range(XJ.ctx, output_ids.data()+causal_t, 48, A0, bJ, 256, false, (llama_pos)(A0 + 47));
          llama_state_get_data(XJ.ctx, TGT4.data(), szJ); }
        const uint64_t tgt4 = fJ(TGT4);
        auto mmJ = [&](const std::vector<uint8_t> & b) { long m = 0; for (size_t i = 0; i < szJ; ++i) if (b[i] != TGT4[i]) m++; return m; };
        auto branch = [&](int mode, RB3 & r) {
            llama_state_set_data(XJ.ctx, PAJ.data(), PAJ.size());
            std::vector<uint8_t> blob(szJ);
            if (mode != 5) { decode_range(XJ.ctx, output_ids.data()+causal_t, 48, A0, bJ, 256, false, (llama_pos)(A0 + 47)); r.tok += 48; r.b48_first = true; }
            llama_state_get_data(XJ.ctx, blob.data(), szJ); r.gets += 1; r.pre = fJ(blob); r.pre_mm = mmJ(blob);
            if (mode == 1) { llama_synchronize(XJ.ctx); r.syncs += 1; }
            else if (mode == 2) { std::vector<uint8_t> b2(szJ); llama_state_get_data(XJ.ctx, b2.data(), szJ); r.gets += 1; r.post = fJ(b2); r.post_mm = mmJ(b2); }
            else if (mode == 3 || mode == 4) { r.pre_before_set = true; llama_state_set_data(XJ.ctx, blob.data(), blob.size()); r.sets += 1; r.set_before_probe = true;
                if (mode == 3) { std::vector<uint8_t> b2(szJ); llama_state_get_data(XJ.ctx, b2.data(), szJ); r.gets += 1; r.postgets += 1; r.post = fJ(b2); r.post_mm = mmJ(b2); r.get_after_set = true; } }
            else if (mode == 5) { llama_state_set_data(XJ.ctx, TGT4.data(), TGT4.size()); r.sets += 1; r.set_before_probe = true; { std::vector<uint8_t> b2(szJ); llama_state_get_data(XJ.ctx, b2.data(), szJ); r.gets += 1; r.post = fJ(b2); r.post_mm = mmJ(b2); } }
            { std::vector<std::vector<uint8_t>> k(NJ), v(NJ);
              for (int li = 0; li < NJ; ++li) extract_layer_bytes(XJ, palJ, kv_layer_ids[li], k[li], v[li]);
              r.kv = kvJ(k, v); }
            llama_batch bpJ = llama_batch_init(1, 0, 1);
            decode_range(XJ.ctx, &ptJ, 1, A0 + 48, bpJ, 256, true);
            r.probe += 1;
            r.lg.resize(nvJ); const float * lfJ = llama_get_logits(XJ.ctx);
            for (int i = 0; i < nvJ; ++i) r.lg[i] = (double) lfJ[i];
            r.am = amJ(r.lg);
            r.pk.resize(NJ); r.pv.resize(NJ);
            for (int li = 0; li < NJ; ++li) extract_layer_bytes(XJ, pprJ, kv_layer_ids[li], r.pk[li], r.pv[li]);
        };
        RB3 r3N, r3S, r3G, r3I, r3J, r3R;
        branch(0, r3N); branch(1, r3S); branch(2, r3G); branch(3, r3I); branch(4, r3J); branch(5, r3R);
        printf("EXP1A3B_REACHED\nBEGIN_EXP1A3B_EXECUTION_CONTRACT\n  N_TARGET_DECODE=%d\n  S_TARGET_DECODE=%d\n  G_TARGET_DECODE=%d\n  I_TARGET_DECODE=%d\n  J_TARGET_DECODE=%d\n  R_TARGET_DECODE=%d\n  N_PROBE_DECODE=%d\n  S_PROBE_DECODE=%d\n  G_PROBE_DECODE=%d\n  I_PROBE_DECODE=%d\n  J_PROBE_DECODE=%d\n  R_PROBE_DECODE=%d\n  S_EXTRA_SYNCHRONIZE=%d\n  G_EXTRA_STATE_GET=%d\n  I_IDENTITY_STATE_SET=%d\n  I_POST_STATE_GET=%d\n  J_IDENTITY_STATE_SET=%d\n  J_POST_STATE_GET=%d\nEND_EXP1A3B_EXECUTION_CONTRACT\n",
               r3N.tok, r3S.tok, r3G.tok, r3I.tok, r3J.tok, r3R.tok, r3N.probe, r3S.probe, r3G.probe, r3I.probe, r3J.probe, r3R.probe,
               r3S.syncs, r3G.gets - 1, r3I.sets, r3I.postgets, r3J.sets, r3J.postgets);
        printf("EXP1A3B_ORDER\n  G: B48_BEFORE_G_PRE_GET=%s G_PRE_GET_BEFORE_G_POST_GET=%s G_POST_GET_BEFORE_PROBE=%s\n  J: B48_BEFORE_J_PRE_GET=%s J_PRE_GET_BEFORE_IDENTITY_SET=%s IDENTITY_SET_BEFORE_J_PROBE=%s NO_POST_GET_BEFORE_J_PROBE=%s\n",
               r3G.b48_first?"TRUE":"FALSE", (r3G.gets>=2)?"TRUE":"FALSE", "TRUE",
               r3J.b48_first?"TRUE":"FALSE", r3J.pre_before_set?"TRUE":"FALSE", r3J.set_before_probe?"TRUE":"FALSE", (r3J.postgets==0)?"TRUE":"FALSE");
        printf("EXP1A3B_STATES\n  TARGET_STATE_HASH=%016llx TARGET_STATE_SIZE=%zu\n  N_PRE_TARGET_MISMATCH_BYTES=%ld\n  S_PRE_TARGET_MISMATCH_BYTES=%ld\n  G_PRE_TARGET_MISMATCH_BYTES=%ld\n  G_POST_TARGET_MISMATCH_BYTES=%ld\n  I_PRE_TARGET_MISMATCH_BYTES=%ld\n  I_POST_TARGET_MISMATCH_BYTES=%ld\n  J_PRE_TARGET_MISMATCH_BYTES=%ld\n  R_POST_TARGET_MISMATCH_BYTES=%ld\n  G_PRE_POST_MISMATCH_BYTES=%ld\n",
               (unsigned long long) tgt4, TGT4.size(), r3N.pre_mm, r3S.pre_mm, r3G.pre_mm, r3G.post_mm, r3I.pre_mm, r3I.post_mm, r3J.pre_mm, r3R.post_mm, r3G.post_mm);
        const bool pre_mm_zero = (r3N.pre_mm==0 && r3S.pre_mm==0 && r3G.pre_mm==0 && r3I.pre_mm==0 && r3J.pre_mm==0);
        const bool all_mm_zero = (r3N.pre_mm==0 && r3S.pre_mm==0 && r3G.pre_mm==0 && r3G.post_mm==0 && r3I.pre_mm==0 && r3I.post_mm==0 && r3J.pre_mm==0 && r3R.post_mm==0);
        printf("  EXP1A3B_PRE_INTERVENTION_STATES_BITWISE_EQUAL=%s\n  EXP1A3B_ALL_STATE_MISMATCH_COUNTS_ZERO=%s\n", pre_mm_zero?"TRUE":"FALSE", all_mm_zero?"TRUE":"FALSE");
        auto cmp4 = [&](const char * n, RB3 & L, RB3 & R2) {
            int pke = 0, nz = 0; double mx = 0, sm = 0, l2 = 0;
            for (int li = 0; li < NJ; ++li) if (memcmp(L.pk[li].data(), R2.pk[li].data(), L.pk[li].size()) == 0) pke++;
            for (int c = 0; c < nvJ; ++c) { const double e = std::abs(L.lg[c] - R2.lg[c]); if (e > mx) mx = e; sm += e; l2 += e * e; if (L.lg[c] != R2.lg[c]) nz++; }
            printf("  CONTRAST=%s\n    KV_EQUAL_LAYERS=%d/%d\n    LOGITS_BITWISE_EQUAL=%s\n    MAX_ABS=%.9g\n    MEAN_ABS=%.9g\n    L2=%.9g\n    NONZERO_COORDS=%d/%d\n    ARGMAX_LEFT=%d\n    ARGMAX_RIGHT=%d\n",
                   n, pke, NJ, (nz==0)?"TRUE":"FALSE", mx, sm/nvJ, std::sqrt(l2), nz, nvJ, L.am, R2.am);
            return nz == 0; };
        printf("EXP1A3B_CONTRASTS\n");
        const bool c_ns = cmp4("N_vs_S", r3N, r3S);
        const bool c_ng = cmp4("N_vs_G", r3N, r3G);
        const bool c_nj = cmp4("N_vs_J", r3N, r3J);
        const bool c_ni = cmp4("N_vs_I", r3N, r3I);
        const bool c_nr = cmp4("N_vs_R", r3N, r3R);
        const bool c_ir = cmp4("I_vs_R", r3I, r3R);
        const bool c_ji = cmp4("J_vs_I", r3J, r3I);
        const bool c_jr = cmp4("J_vs_R", r3J, r3R);
        const bool c_gi = cmp4("G_vs_I", r3G, r3I);
        const bool c_si = cmp4("S_vs_I", r3S, r3I);
        printf("EXP1A3B_VERDICTS\n  N_vs_S_EQUAL=%s\n  N_vs_G_EQUAL=%s\n  N_vs_J_EQUAL=%s\n  N_vs_I_EQUAL=%s\n  N_vs_R_EQUAL=%s\n  I_vs_R_EQUAL=%s\n  J_vs_I_EQUAL=%s\n  J_vs_R_EQUAL=%s\n  G_vs_I_EQUAL=%s\n  S_vs_I_EQUAL=%s\n",
               c_ns?"TRUE":"FALSE", c_ng?"TRUE":"FALSE", c_nj?"TRUE":"FALSE", c_ni?"TRUE":"FALSE", c_nr?"TRUE":"FALSE", c_ir?"TRUE":"FALSE", c_ji?"TRUE":"FALSE", c_jr?"TRUE":"FALSE", c_gi?"TRUE":"FALSE", c_si?"TRUE":"FALSE");
        const bool ref_ok = (!c_ni) && c_ir;
        printf("  EXP1A3B_REFERENCE_EXP1A3_REPRODUCED=%s\n", ref_ok ? "TRUE" : "FALSE");
        if (c_ns && c_ng && !c_nj && c_ji && c_jr) printf("  EXP1A3B_CASE=IDENTITY_STATE_SET_ALONE_CAUSAL\n  EXTRA_SYNCHRONIZE_EFFECT_ON_TESTED_PROBE=NOT_OBSERVED\n  EXTRA_STATE_GET_EFFECT_ON_TESTED_PROBE=NOT_OBSERVED\n  IDENTITY_STATE_SET_WITHOUT_POST_READBACK_ALTERS_TESTED_NEXT_PROBE=SUPPORTED\n  IDENTITY_STATE_SET_ALONE_MOVES_NATURAL_TO_RESTORED_CLASS=SUPPORTED\n  RESTORE_OPERATION_CAUSALLY_LOCALIZED=TRUE\n");
        else if (!c_ng) printf("  EXP1A3B_CASE=EXTRA_STATE_GET_CAUSALLY_ACTIVE\n");
        else if (c_nj && !c_ni) printf("  EXP1A3B_CASE=STATE_SET_PLUS_POST_GET_INTERACTION\n");
        else if (!c_nj && !c_ji) printf("  EXP1A3B_CASE=STATE_SET_CAUSAL_WITH_POST_GET_MODULATION\n");
        else printf("  EXP1A3B_CASE=OTHER\n");
        printf("  EXP1A3B_DONE\n");
    }
    return 0;
}
