// e8-resume2 -- REPRISE HYBRIDE COMPLETE SANS PREFILL DESTINATION.
// Appelle REELLEMENT llama_state_seq_{get_size,get_data,set_data}_ext, flags=0.
// PARTIAL_ONLY est REFUSE : il ne transporte pas le KV attention.
#include "llama.h"
#include "ggml.h"
#include "llama-context.h"
#include "llama-model.h"
#include "llama-memory-hybrid.h"
#include "llama-memory-recurrent.h"
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cstdint>
#include <cmath>
#include <fstream>
#include <sstream>
#include <string>
#include <vector>

static void die(const char * m) { fprintf(stderr, "FATAL: %s\n", m); exit(2); }
static const char * arg(int c, char ** v, const char * n, const char * d = nullptr) {
    for (int i = 1; i + 1 < c; ++i) if (strcmp(v[i], n) == 0) return v[i+1];
    if (d) return d; fprintf(stderr, "missing %s\n", n); exit(2);
}
static std::string sha256_of(const uint8_t * d, size_t n) {
    std::string p = "/tmp/.e8r2blob";
    { std::ofstream f(p, std::ios::binary); f.write((const char *) d, (std::streamsize) n); }
    std::string c = "sha256sum '" + p + "' | cut -d' ' -f1";
    FILE * fp = popen(c.c_str(), "r"); char b[128] = {0};
    if (fp) { if (!fgets(b, sizeof b, fp)) {} pclose(fp); }
    std::string r(b); while (!r.empty() && (r.back()=='\n'||r.back()=='\r')) r.pop_back();
    return r;
}
static std::vector<int32_t> read_ids(const char * p) {
    std::vector<int32_t> v; std::ifstream f(p); long x;
    if (!f) die("ids illisible"); while (f >> x) v.push_back((int32_t) x); return v;
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
struct Gen { std::vector<llama_token> ids; std::string text, esc; const char * stop = "NONE"; int decodes = 0; };
static Gen generate(llama_context * ctx, llama_model * m, llama_token first, int max_new, llama_pos p0) {
    Gen g; const llama_vocab * v = llama_model_get_vocab(m);
    const int nv = llama_vocab_n_tokens(v);
    std::vector<float> lg(nv);
    const float * p = llama_get_logits_ith(ctx, -1);
    if (!p) { g.stop = "ERROR"; return g; }
    memcpy(lg.data(), p, sizeof(float) * nv);
    g.ids.push_back(first);
    llama_batch b = llama_batch_init(1, 0, 1);
    llama_pos pos = p0;
    for (int step = 0; step < max_new; ++step) {
        if (llama_vocab_is_eog(v, g.ids.back())) { g.stop = "EOS"; break; }
        if ((int) g.ids.size() >= max_new) { g.stop = "MAX_NEW_TOKENS"; break; }
        b.token[0] = g.ids.back(); b.pos[0] = pos++;
        b.n_seq_id[0] = 1; b.seq_id[0][0] = 0; b.logits[0] = 1; b.n_tokens = 1;
        if (llama_decode(ctx, b) != 0) { g.stop = "ERROR"; break; }
        g.decodes++;
        const float * q = llama_get_logits_ith(ctx, -1);
        if (!q) { g.stop = "ERROR"; break; }
        memcpy(lg.data(), q, sizeof(float) * nv);
        int best = 0; for (int i = 1; i < nv; ++i) if (lg[i] > lg[best]) best = i;
        g.ids.push_back((llama_token) best);
    }
    llama_batch_free(b);
    if (strcmp(g.stop, "NONE") == 0) g.stop = "MAX_NEW_TOKENS";
    for (llama_token t : g.ids) {
        char pb[512]; int np = llama_token_to_piece(v, t, pb, sizeof pb, 0, true);
        if (np < 0) { std::vector<char> big(-np); np = llama_token_to_piece(v, t, big.data(), (int) big.size(), 0, true);
                      if (np >= 0) for (int k = 0; k < np; ++k) g.text.push_back(big[k]); }
        else for (int k = 0; k < np; ++k) g.text.push_back(pb[k]);
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
    const int N = 58;

    std::vector<int32_t> ids = read_ids(ids_path);
    if ((int) ids.size() != 59) { fprintf(stderr, "FATAL: %zu IDs, 59 attendus\n", ids.size()); return 3; }
    printf("E8_RESUME2\nINPUT_TOKEN_COUNT=%zu\nMAX_NEW=%d\nPREFIX_TOKENS=%d\nNEXT_POS=%d\n",
           ids.size(), max_new, N, N);

    // --- mode de representation : REFUSE PARTIAL_ONLY, exige flags=0
    const llama_state_seq_flags FLAGS = LLAMA_STATE_SEQ_FLAGS_NONE;
    printf("TRANSFER_FLAGS=%d\n", (int) FLAGS);
    if (FLAGS & LLAMA_STATE_SEQ_FLAGS_PARTIAL_ONLY) { printf("PARTIAL_ONLY_REFUSED=1\n"); return 4; }
    printf("PARTIAL_ONLY_REFUSED=YES\n");
    printf("ENV_OBRAIN_STATE_TYPE=%s\n", getenv("OBRAIN_STATE_TYPE") ? getenv("OBRAIN_STATE_TYPE") : "<ABSENTE>");
    printf("ENV_OBRAIN_STATE_F16=%s\n",    getenv("OBRAIN_STATE_F16")    ? getenv("OBRAIN_STATE_F16")    : "<ABSENTE>");
    printf("ENV_OBRAIN_NO_ROTATION=%s\n",  getenv("OBRAIN_NO_ROTATION")  ? getenv("OBRAIN_NO_ROTATION")  : "<ABSENTE>");

    llama_backend_init();
    llama_model_params mp = llama_model_default_params();
    llama_model * model = llama_model_load_from_file(model_path, mp);
    if (!model) die("modele");
    llama_context_params cp = llama_context_default_params();
    cp.n_ctx = 512; cp.n_batch = 512; cp.n_ubatch = 512; cp.n_seq_max = 1;
    llama_context * ctxA = llama_init_from_model(model, cp);
    llama_context * ctxB = llama_init_from_model(model, cp);
    if (!ctxA || !ctxB) die("contextes");
    printf("KV_TYPE_K=%s\nKV_TYPE_V=%s\nN_CTX=%d\nN_BATCH=%d\nN_UBATCH=%d\n",
           ggml_type_name(llama_kv_cache_type_k(ctxA)), ggml_type_name(llama_kv_cache_type_v(ctxA)),
           512, 512, 512);
    // Le backend est identifie ; les accesseurs internes de la memoire
    // recurrente (get_n_rs / get_size) ne sont PAS atteignables par ce chemin
    // d'inclusion : je ne les invente pas et je le declare.
    { const char * kind = "INCONNU";
      if (dynamic_cast<llama_memory_hybrid *>(ctxA->get_memory())) kind = "HYBRID";
      else if (dynamic_cast<llama_kv_cache *>(ctxA->get_memory())) kind = "PLAIN_KV";
      printf("MEMORY_BACKEND=%s\n", kind);
      printf("N_RS_SEQ=NOT_READ\nRECURRENT_CAPACITY_CELLS=NOT_READ\n"); }

    llama_batch bA = llama_batch_init(512,0,1), bB = llama_batch_init(512,0,1);
    int dst_prefix_decodes = 0, dst_prefix_tokens = 0;

    // --- 3. CONTROL : prefill 0..57, achèvement, MESURE de taille, export
    if (!decode_range(ctxA, ids.data(), N, 0, bA, false)) { printf("PREFILL_CONTROL=FAIL\n"); return 5; }
    llama_synchronize(ctxA);                       // achevement du calcul avant export
    printf("PREFILL_CONTROL=PASS\nCONTROL_PREFIX_DECODE_CALLS=1\nCONTROL_PREFIX_TOKEN_COUNT=%d\n", N);

    const size_t size_measured = llama_state_seq_get_size_ext(ctxA, 0, FLAGS);
    printf("GET_SIZE_EXT=%zu\n", size_measured);
    if (size_measured == 0) { printf("STOP=GET_SIZE_ZERO\n"); return 6; }
    std::vector<uint8_t> blob(size_measured, 0);
    const size_t n_written = llama_state_seq_get_data_ext(ctxA, blob.data(), blob.size(), 0, FLAGS);
    printf("GET_DATA_EXT=%zu\nBLOB_SIZE=%zu\n", n_written, blob.size());
    if (n_written == 0 || n_written > blob.size()) { printf("STOP=EXPORT_INVALID\n"); return 7; }
    blob.resize(n_written);
    const std::string blob_sha = sha256_of(blob.data(), blob.size());
    printf("BLOB_SHA256=%s\nEXPORT_COMPLETE=%s\n", blob_sha.c_str(), (n_written == size_measured) ? "PASS" : "PARTIAL");
    if (n_written != size_measured) { printf("STOP=EXPORT_PARTIAL\n"); return 8; }

    // --- positions CONTROL (observees)
    llama_memory_t mA = llama_get_memory(ctxA);
    printf("CONTROL_POS_ATTN_MIN=%d\nCONTROL_POS_ATTN_MAX=%d\n",
           (int) llama_memory_seq_pos_min_attn(mA, 0), (int) llama_memory_seq_pos_max_attn(mA, 0));
    printf("CONTROL_POS_ALL_MIN=%d\nCONTROL_POS_ALL_MAX=%d\n",
           (int) llama_memory_seq_pos_min(mA, 0), (int) llama_memory_seq_pos_max(mA, 0));

    // --- 4. RESTORED : AUCUN prefixe
    printf("DESTINATION_PREFIX_DECODE_CALLS=%d\nDESTINATION_PREFIX_TOKEN_COUNT=%d\n",
           dst_prefix_decodes, dst_prefix_tokens);
    if (dst_prefix_decodes != 0 || dst_prefix_tokens != 0) { printf("STOP=DESTINATION_NOT_PRISTINE\n"); return 9; }
    const size_t n_read = llama_state_seq_set_data_ext(ctxB, blob.data(), blob.size(), 0, FLAGS);
    printf("SET_DATA_EXT=%zu\n", n_read);
    if (n_read != blob.size()) { printf("IMPORT_COMPLETE=%s\nSTOP=IMPORT_INVALID\n", n_read==0?"FAIL":"PARTIAL"); return 10; }
    printf("IMPORT_COMPLETE=PASS\n");
    llama_synchronize(ctxB);

    llama_memory_t mB = llama_get_memory(ctxB);
    printf("RESTORED_POS_ATTN_MIN=%d\nRESTORED_POS_ATTN_MAX=%d\n",
           (int) llama_memory_seq_pos_min_attn(mB, 0), (int) llama_memory_seq_pos_max_attn(mB, 0));
    printf("RESTORED_POS_ALL_MIN=%d\nRESTORED_POS_ALL_MAX=%d\n",
           (int) llama_memory_seq_pos_min(mB, 0), (int) llama_memory_seq_pos_max(mB, 0));

    // --- reexport depuis RESTORED (taille propre)
    const size_t size_B = llama_state_seq_get_size_ext(ctxB, 0, FLAGS);
    std::vector<uint8_t> blobB(size_B, 0);
    const size_t nB = llama_state_seq_get_data_ext(ctxB, blobB.data(), blobB.size(), 0, FLAGS);
    blobB.resize(nB);
    printf("REEXPORT_SIZE=%zu\nREEXPORT_SHA256=%s\nREEXPORT_IDENTICAL=%s\n",
           nB, sha256_of(blobB.data(), blobB.size()).c_str(),
           (nB == blob.size() && blobB == blob) ? "YES" : "NO");

    // --- 5. continuation : 59e ID en position 58
    if (!decode_range(ctxA, ids.data() + N, 1, N, bA, true)) { printf("DECODE59_CONTROL=FAIL\n"); return 11; }
    if (!decode_range(ctxB, ids.data() + N, 1, N, bB, true)) { printf("DECODE59_RESTORED=FAIL\n"); return 11; }
    printf("DECODE_59TH_CONTROL=PASS\nDECODE_59TH_RESTORED=PASS\n");

    const int nv = (int) llama_vocab_n_tokens(llama_model_get_vocab(model));
    const float * lA = llama_get_logits_ith(ctxA, -1);
    const float * lB2 = llama_get_logits_ith(ctxB, -1);
    int argA = 0, argB = 0; double maxd = 0, l2 = 0; bool fin = true; bool bin = true;
    if (lA && lB2) {
        for (int i = 0; i < nv; ++i) {
            if (!std::isfinite(lA[i]) || !std::isfinite(lB2[i])) fin = false;
            const double d = std::fabs((double) lA[i] - (double) lB2[i]);
            if (d > maxd) maxd = d; l2 += d * d;
            if (lA[i] > lA[argA]) argA = i;
            if (lB2[i] > lB2[argB]) argB = i;
            if (memcmp(&lA[i], &lB2[i], 4) != 0) bin = false;
        }
        printf("START_LOGITS FINITE=%s MAX_ABS_DIFF=%.9g L2_DIFF=%.9g ARGMAX_A=%d ARGMAX_B=%d ARGMAX_MATCH=%s BINARY_IDENTICAL=%s\n",
               fin?"YES":"NO", maxd, std::sqrt(l2), argA, argB, (argA==argB)?"YES":"NO", bin?"YES":"NO");
    } else printf("START_LOGITS_UNAVAILABLE\n");

    Gen gA = generate(ctxA, model, (llama_token) argA, max_new, 59);
    Gen gB = generate(ctxB, model, (llama_token) argB, max_new, 59);
    auto emit = [](const char * tag, const Gen & g) {
        printf("%s_IDS=[", tag); for (size_t i=0;i<g.ids.size();++i) printf("%s%d", i?",":"", g.ids[i]);
        printf("]\n%s_COUNT=%zu\n%s_STOP=%s\n%s_DECODES=%d\n%s_TEXT=[%s]\n%s_TEXT_ESC=%s\n",
               tag, g.ids.size(), tag, g.stop, tag, g.decodes, tag, g.text.c_str(), tag, g.esc.c_str()); };
    emit("CONTROL", gA); emit("RESTORED", gB);
    printf("TOKEN_SEQUENCE_MATCH=%s\n", (gA.ids == gB.ids) ? "YES" : "NO");

    llama_batch_free(bA); llama_batch_free(bB);
    llama_free(ctxA); llama_free(ctxB); llama_model_free(model); llama_backend_free();
    return 0;
}
