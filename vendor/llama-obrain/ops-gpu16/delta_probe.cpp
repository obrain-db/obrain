// delta_probe — faisabilité « Δ-décodage » (Qwen3.8-27B).
// Capture des états cachés par couche à chaque token (préfill token-à-token + continuation
// greedy) et analyse des deltas consécutifs Δ_p = h_{p+1} − h_p :
//   - norme relative moyenne ||Δ||/||h|| par couche ;
//   - énergie cumulée top-k des directions de Δ (Gram ΔΔᵀ, k ∈ 8..256) ;
//   - participation ratio (rang effectif).
// Usage : delta_probe <model.gguf> [n_gen]  (CPU ; sortie : stdout + rien)
#include "llama.h"
#include "common.h"

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

static void jacobi_eigh(double * A, int n, std::vector<double> & evals) {
    for (int sweep = 0; sweep < 300; ++sweep) {
        double off = 0.0;
        for (int i = 0; i < n; ++i) for (int j = i + 1; j < n; ++j) off += A[i * n + j] * A[i * n + j];
        if (off < 1e-26) break;
        for (int p = 0; p < n; ++p) for (int q = p + 1; q < n; ++q) {
            const double apq = A[p * n + q];
            if (std::fabs(apq) < 1e-300) continue;
            const double app = A[p * n + p], aqq = A[q * n + q];
            const double theta = 0.5 * std::atan2(2.0 * apq, aqq - app);
            const double c = std::cos(theta), s = std::sin(theta);
            for (int k = 0; k < n; ++k) {
                const double akp = A[k * n + p], akq = A[k * n + q];
                A[k * n + p] = c * akp - s * akq; A[p * n + k] = A[k * n + p];
                A[k * n + q] = s * akp + c * akq; A[q * n + k] = A[k * n + q];
            }
            A[p * n + p] = c * c * app + s * s * aqq - 2 * s * c * apq;
            A[q * n + q] = s * s * app + c * c * aqq + 2 * s * c * apq;
            A[p * n + q] = 0.0; A[q * n + p] = 0.0;
        }
    }
    evals.assign(n, 0.0);
    for (int i = 0; i < n; ++i) evals[i] = A[i * n + i];
    std::sort(evals.begin(), evals.end(), std::greater<double>());
}

int main(int argc, char ** argv) {
    if (argc < 2) { fprintf(stderr, "usage: %s model.gguf [n_gen]\n", argv[0]); return 2; }
    const char * model_path = argv[1];
    const int n_gen = argc > 2 ? std::atoi(argv[2]) : 96;

    llama_backend_init();
    llama_model_params mp = llama_model_default_params();
    mp.n_gpu_layers = 0;
    llama_model * model = llama_load_model_from_file(model_path, mp);
    if (!model) { fprintf(stderr, "model load failed\n"); return 1; }
    const llama_vocab * vocab = llama_model_get_vocab(model);
    const int n_layer = llama_model_n_layer(model);
    const int n_embd  = llama_model_n_embd(model);
    fprintf(stderr, "model: layers=%d n_embd=%d\n", n_layer, n_embd);

    llama_context_params cp = llama_context_default_params();
    cp.n_ctx = 2048; cp.n_batch = 512; cp.n_ubatch = 512; cp.n_seq_max = 1;
    llama_context * ctx = llama_init_from_model(model, cp);
    if (!ctx) { fprintf(stderr, "ctx failed\n"); return 1; }

    std::vector<int32_t> layers(n_layer);
    for (int i = 0; i < n_layer; ++i) layers[i] = i;
    llama_set_layer_output_capture(ctx, layers.data(), (int32_t) layers.size());

    const std::string prompt =
        "In the Linux kernel, network drivers use NAPI for high-performance packet "
        "reception. The poll method is registered with netif_napi_add and runs in "
        "softirq context. When interrupts arrive, the driver schedules NAPI polling "
        "and disables further interrupts until the budget is exhausted. The e1000 "
        "driver initializes hardware rings, configures descriptor registers, and "
        "starts the transmit and receive paths in e1000_probe.";

    std::vector<llama_token> toks = common_tokenize(vocab, prompt, true);
    const int N = (int) std::min<size_t>(toks.size() + (size_t) n_gen, 256);
    fprintf(stderr, "prompt tokens: %zu ; n_gen cible=%d ; N=%d\n", toks.size(), n_gen, N);

    std::vector<float> store((size_t) N * n_layer * n_embd); // h_l(t), layout [t][layer][i]

    auto decode_one = [&](llama_token id, int t) {
        llama_batch b = llama_batch_get_one(&id, 1);
        if (llama_decode(ctx, b) != 0) return false;
        for (int l = 0; l < n_layer; ++l) {
            const float * out = llama_get_layer_output(ctx, l, 0);
            if (out) std::memcpy(&store[((size_t) t * n_layer + l) * n_embd], out, n_embd * sizeof(float));
        }
        return true;
    };

    int t = 0;
    for (; t < (int) toks.size(); ++t) if (!decode_one(toks[t], t)) return 1;
    for (; t < N; ++t) {
        const float * logits = llama_get_logits_ith(ctx, 0);
        const int nv = llama_vocab_n_tokens(vocab);
        llama_token id = 0; double best = -1e30;
        for (int v = 0; v < nv; ++v) if ((double) logits[v] > best) { best = logits[v]; id = v; }
        if (!decode_one(id, t)) break;
    }
    const int n = t;
    fprintf(stderr, "positions analysées : %d\n", n);

    const int m = n - 1;
    std::vector<double> evals;
    for (int l = 0; l < n_layer; ++l) {
        double sq_sum = 0.0, h_sq_sum = 0.0;
        // Δ_p = h_{p+1} − h_p
        std::vector<double> G((size_t) m * m, 0.0);
        for (int p = 0; p < m; ++p) {
            const float * hp  = &store[((size_t)(p + 1) * n_layer + l) * n_embd];
            const float * hpm = &store[((size_t) p * n_layer + l) * n_embd];
            for (int q = p; q < m; ++q) {
                const float * hq  = &store[((size_t)(q + 1) * n_layer + l) * n_embd];
                const float * hqm = &store[((size_t) q * n_layer + l) * n_embd];
                double s = 0.0;
                for (int i = 0; i < n_embd; ++i) {
                    const double dp = (double) hp[i] - hpm[i];
                    const double dq = (double) hq[i] - hqm[i];
                    s += dp * dq;
                }
                G[p * (size_t) m + q] = s; G[q * (size_t) m + p] = s;
            }
            for (int i = 0; i < n_embd; ++i) {
                const double dp = (double) hp[i] - hpm[i];
                sq_sum += dp * dp;
                h_sq_sum += (double) hp[i] * hp[i];
            }
        }
        if (m < 2) continue;
        std::vector<double> Gc = G;
        jacobi_eigh(Gc.data(), m, evals);
        double total = 0.0; for (double e : evals) if (e > 0) total += e;
        double c8 = 0, c16 = 0, c32 = 0, c64 = 0, c128 = 0, c256 = 0;
        double pn = 0, pd = 0;
        for (int k = 0; k < (int) evals.size(); ++k) {
            const double e = evals[k] > 0 ? evals[k] : 0.0;
            if (k < 8) c8 += e; if (k < 16) c16 += e; if (k < 32) c32 += e;
            if (k < 64) c64 += e; if (k < 128) c128 += e; if (k < 256) c256 += e;
            pn += e * e; pd += e;
        }
        const double to = total > 0 ? total : 1.0;
        const double rel = h_sq_sum > 0 ? std::sqrt(sq_sum / h_sq_sum) : 0.0;
        const double pr = pd > 0 ? pn / (pd * pd) : 0.0;
        printf("layer %3d | rel|Δ|/|h|=%.4f | PR=%.1f | top-k energy: "
               "8=%.3f 16=%.3f 32=%.3f 64=%.3f 128=%.3f 256=%.3f\n",
               l, rel, pr, c8 / to, c16 / to, c32 / to, c64 / to, c128 / to, c256 / to);
    }

    llama_free(ctx);
    llama_free_model(model);
    llama_backend_free();
    return 0;
}
