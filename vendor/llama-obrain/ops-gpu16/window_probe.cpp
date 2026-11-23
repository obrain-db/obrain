// window_probe — batterie « rang local de fenêtre » (protocole utilisateur corrigé).
// Capture h_l(t) par couche (préfill + génération greedy), puis pour L ∈ {4,8,16,32},
// fenêtres glissantes du PASSÉ [t−L..t−1] uniquement :
//   - ERREUR DE REPRÉSENTABILITÉ du token suivant : min_α ‖h_t − H_pastᵀα‖/‖h_t‖ (base passée) ;
//   - fraction de fenêtres où err > ε (0,01/0,05/0,10) → fréquence de rafraîchissement ;
//   - évolution de l'erreur médiane par profondeur (dérive).
// Sortie : /tmp/window_report.csv
// Usage : window_probe <model.gguf> [n_gen]
#include "llama.h"
#include "common.h"
#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

static void solve_lin(double * A, double * b, int n) { // Gauss, A n×n en place
    for (int i = 0; i < n; ++i) {
        int p = i; for (int k = i + 1; k < n; ++k) if (std::fabs(A[k * n + i]) > std::fabs(A[p * n + i])) p = k;
        if (p != i) { for (int k = 0; k < n; ++k) std::swap(A[i * n + k], A[p * n + k]); std::swap(b[i], b[p]); }
        const double piv = A[i * n + i]; if (std::fabs(piv) < 1e-30) continue;
        for (int k = 0; k < n; ++k) A[i * n + k] /= piv; b[i] /= piv;
        for (int j = 0; j < n; ++j) if (j != i) {
            const double f = A[j * n + i]; if (std::fabs(f) < 1e-30) continue;
            for (int k = 0; k < n; ++k) A[j * n + k] -= f * A[i * n + k];
            b[j] -= f * b[i];
        }
    }
}
static void jacobi_eigh(double * A, int n, std::vector<double> & ev) {
    for (int s = 0; s < 500; ++s) {
        double off = 0; for (int i = 0; i < n; ++i) for (int j = i + 1; j < n; ++j) off += A[i * n + j] * A[i * n + j];
        if (off < 1e-28) break;
        for (int p = 0; p < n; ++p) for (int q = p + 1; q < n; ++q) {
            const double apq = A[p * n + q]; if (std::fabs(apq) < 1e-300) continue;
            const double th = 0.5 * std::atan2(2 * apq, A[q * n + q] - A[p * n + p]);
            const double c = std::cos(th), s = std::sin(th);
            for (int k = 0; k < n; ++k) {
                const double a1 = A[k * n + p], a2 = A[k * n + q];
                A[k * n + p] = c * a1 - s * a2; A[p * n + k] = A[k * n + p];
                A[k * n + q] = s * a1 + c * a2; A[q * n + k] = A[k * n + q];
            }
            const double app = A[p * n + p], aqq = A[q * n + q];
            A[p * n + p] = c * c * app - 2 * s * c * apq + s * s * aqq;
            A[q * n + q] = s * s * app + 2 * s * c * apq + c * c * aqq;
            A[p * n + q] = 0; A[q * n + p] = 0;
        }
    }
    ev.assign(n, 0); for (int i = 0; i < n; ++i) ev[i] = A[i * n + i];
    std::sort(ev.begin(), ev.end(), std::greater<double>());
    for (auto & e : ev) if (e < 0) e = 0;
}

int main(int argc, char ** argv) {
    if (argc < 2) { fprintf(stderr, "usage: %s model.gguf [n_gen]\n", argv[0]); return 2; }
    const int n_gen = argc > 2 ? std::atoi(argv[2]) : 96;
    llama_backend_init();
    llama_model_params mp = llama_model_default_params(); mp.n_gpu_layers = 0;
    llama_model * model = llama_load_model_from_file(argv[1], mp);
    if (!model) return 1;
    const llama_vocab * vocab = llama_model_get_vocab(model);
    const int n_layer = llama_model_n_layer(model), n_embd = llama_model_n_embd(model);
    llama_context_params cp = llama_context_default_params();
    cp.n_ctx = 2048; cp.n_batch = 512; cp.n_ubatch = 512; cp.n_seq_max = 1;
    llama_context * ctx = llama_init_from_model(model, cp);
    std::vector<int32_t> layers(n_layer); for (int i = 0; i < n_layer; ++i) layers[i] = i;
    llama_set_layer_output_capture(ctx, layers.data(), n_layer);
    const std::string prompt =
        "In the Linux kernel, network drivers use NAPI for high-performance packet reception. "
        "The poll method is registered with netif_napi_add and runs in softirq context. "
        "When interrupts arrive, the driver schedules NAPI polling and disables further "
        "interrupts until the budget is exhausted. The e1000 driver initializes hardware "
        "rings, configures descriptor registers, and starts the transmit and receive paths "
        "in e1000_probe.";
    std::vector<llama_token> toks = common_tokenize(vocab, prompt, true);
    const int N = (int) std::min<size_t>(toks.size() + (size_t) n_gen, 240);
    std::vector<float> store((size_t) N * n_layer * n_embd);
    auto cap = [&](int t) { for (int l = 0; l < n_layer; ++l) { const float * o = llama_get_layer_output(ctx, l, 0); if (o) memcpy(&store[((size_t) t * n_layer + l) * n_embd], o, n_embd * sizeof(float)); } };
    auto dec = [&](llama_token id, int t) { llama_batch b = llama_batch_get_one(&id, 1); if (llama_decode(ctx, b)) return false; cap(t); return true; };
    int t = 0; for (; t < (int) toks.size(); ++t) if (!dec(toks[t], t)) return 1;
    for (; t < N; ++t) { const float * lo = llama_get_logits_ith(ctx, 0); const int nv = llama_vocab_n_tokens(vocab);
        llama_token id = 0; double best = -1e30; for (int v = 0; v < nv; ++v) if ((double) lo[v] > best) { best = lo[v]; id = v; }
        if (!dec(id, t)) break; }
    const int n = t; fprintf(stderr, "positions: %d (n_embd=%d)\n", n, n_embd);

    FILE * csv = fopen("/tmp/window_report.csv", "w");
    fprintf(csv, "layer,L,med_err,frac_1e-2,frac_5e-2,frac_1e-1,rel_gain_1e-1\n");
    const int Ls[4] = {4, 8, 16, 32};
    for (int l = 0; l < n_layer; ++l) {
        for (int li = 0; li < 4; ++li) {
            const int L = Ls[li];
            std::vector<double> errs; int cnt = 0; double f1 = 0, f5 = 0, f10 = 0;
            for (int t0 = L; t0 < n; ++t0) {
                const float * hc = &store[((size_t) t0 * n_layer + l) * n_embd];
                std::vector<double> G(L * L, 0.0), rhs(L, 0.0);
                for (int i = 0; i < L; ++i) {
                    const float * hi = &store[((size_t)(t0 - L + i) * n_layer + l) * n_embd];
                    double ri = 0; for (int k = 0; k < n_embd; ++k) ri += (double) hi[k] * hc[k];
                    rhs[i] = ri;
                    for (int j = 0; j <= i; ++j) { const float * hj = &store[((size_t)(t0 - L + j) * n_layer + l) * n_embd];
                        double s = 0; for (int k = 0; k < n_embd; ++k) s += (double) hi[k] * hj[k];
                        G[i * L + j] = s; G[j * L + i] = s; }
                }
                std::vector<double> A = G, b = rhs; solve_lin(A.data(), b.data(), L);
                double err2 = 0, hn2 = 0;
                for (int k = 0; k < n_embd; ++k) {
                    double proj = 0; for (int i = 0; i < L; ++i) proj += b[i] * (double) store[((size_t)(t0 - L + i) * n_layer + l) * n_embd + k];
                    const double hh = (double) hc[k]; err2 += (hh - proj) * (hh - proj); hn2 += hh * hh;
                }
                const double e = hn2 > 0 ? std::sqrt(err2 / hn2) : 1.0;
                errs.push_back(e); cnt++;
                if (e > 0.01) f1++; if (e > 0.05) f5++; if (e > 0.10) f10++;
            }
            if (!errs.empty()) {
                std::sort(errs.begin(), errs.end());
                const double med = errs[errs.size() / 2];
                const double q = cnt ? f10 / cnt : 0;
                const double rg = q < 0.99 ? 1.0 / std::max(1e-6, 1.0 - q) : 99.0;
                fprintf(csv, "%d,%d,%.5f,%.3f,%.3f,%.3f,%.2f\n", l, L, med, cnt ? f1 / cnt : 0, cnt ? f5 / cnt : 0, q, rg);
            }
        }
    }
    fclose(csv);
    printf("rapport écrit : /tmp/window_report.csv\n");
    llama_free(ctx); llama_free_model(model); llama_backend_free();
    return 0;
}
