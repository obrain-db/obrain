// ── L'op WHT : le contrat, et trois chemins qui doivent tomber d'accord ──────────────────────
//
// Le repli des poids et la transformée de l'activation sont la même opération (orthogonale). Le
// runtime l'obtient par `GGML_OP_WHT` (ggml_wht), un op à part entière que les backends pourront
// implémenter un par un — les noyaux FWHT existent déjà côté CUDA (fwht.cuh) et Vulkan.
//
// Ce test compare TROIS implémentations de la même transformée :
//
//   1. `ggml_wht`                    — l'op, sur le backend CPU, parallélisé
//   2. la matrice dense + le hint     — le chemin historique du fork (GGML_HINT_SRC0_IS_HADAMARD)
//   3. `obrain_rotate`                — le scalaire du repli hors ligne
//
// Les trois doivent donner le même résultat, sinon le repli et le runtime ne sont plus jumeaux et
// le modèle devient du bruit. C'est la seule propriété qui compte, et elle se vérifie ici.

#include "../src/obrain-rotation.h"
#include "../ggml/src/ggml-quants.h" // block_tq2_g128 et les quantifieurs de référence

#include "ggml.h"
#include "ggml-alloc.h"
#include "ggml-backend.h"
#include "ggml-cpu.h"

#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <random>
#include <vector>

static int n_failed = 0;
static int n_passed = 0;

static void check(bool ok, const char * what) {
    if (ok) { n_passed++; } else { n_failed++; printf("  FAIL  %s\n", what); }
}

static double now_ms(void) {
    using clk = std::chrono::steady_clock;
    return std::chrono::duration<double, std::milli>(clk::now().time_since_epoch()).count();
}

// la matrice de Walsh–Hadamard normalisée 1024×1024 (construction de Sylvester)
static void hadamard1024(std::vector<float> & H) {
    H.assign(1024*1024, 0.0f);
    for (int i = 0; i < 1024; ++i) {
        for (int j = 0; j < 1024; ++j) {
            int bits = 0;
            for (int b = 0; b < 10; ++b) {
                if (((i >> b) & 1) && ((j >> b) & 1)) bits++;
            }
            H[(size_t) i*1024 + j] = (bits & 1) ? -1.0f/32.0f : 1.0f/32.0f;
        }
    }
}

int main(void) {
    ggml_backend_t backend = ggml_backend_cpu_init();
    if (!backend) { printf("pas de backend CPU\n"); return 1; }
    ggml_backend_cpu_set_n_threads(backend, 8);

    const int64_t ne_in  = 5120;
    const int64_t ne_out = 8;
    const int64_t n_tok  = 4;
    const int32_t block  = 1024;

    std::mt19937 rng(20260920);
    std::normal_distribution<float> gauss(0.0f, 1.0f);

    std::vector<float> H;
    hadamard1024(H);

    std::vector<float> W((size_t) ne_in*ne_out), W_folded((size_t) ne_in*ne_out);
    for (auto & e : W) e = gauss(rng) * 0.02f;
    for (int64_t j = 0; j < ne_out; ++j) {
        memcpy(&W_folded[(size_t) j*ne_in], &W[(size_t) j*ne_in], (size_t) ne_in*sizeof(float));
        obrain_rotate(&W_folded[(size_t) j*ne_in], ne_in);
    }

    std::vector<float> X((size_t) ne_in*n_tok);
    for (auto & e : X) e = gauss(rng);

    // référence en clair : W·x
    std::vector<double> ref((size_t) ne_out*n_tok, 0.0);
    for (int64_t j = 0; j < ne_out; ++j)
        for (int64_t t = 0; t < n_tok; ++t)
            for (int64_t i = 0; i < ne_in; ++i)
                ref[(size_t) j*n_tok + t] += (double) W[(size_t) j*ne_in + i] * X[(size_t) t*ne_in + i];

    ggml_init_params params = { 512*1024*1024, nullptr, /* no_alloc = */ false };
    ggml_context * ctx = ggml_init(params);

    ggml_tensor * t_x  = ggml_new_tensor_2d(ctx, GGML_TYPE_F32, ne_in, n_tok);
    ggml_tensor * t_H  = ggml_new_tensor_2d(ctx, GGML_TYPE_F32, 1024, 1024);
    ggml_tensor * t_W  = ggml_new_tensor_2d(ctx, GGML_TYPE_F32, ne_in, ne_out);
    ggml_tensor * t_Wf = ggml_new_tensor_2d(ctx, GGML_TYPE_F32, ne_in, ne_out);

    for (int64_t t = 0; t < n_tok; ++t)
        memcpy((char *) t_x->data + t*ne_in*sizeof(float), &X[(size_t) t*ne_in], ne_in*sizeof(float));
    memcpy(t_H->data, H.data(), H.size()*sizeof(float));
    for (int64_t j = 0; j < ne_out; ++j) {
        memcpy((char *) t_W->data  + j*ne_in*sizeof(float), &W[(size_t) j*ne_in],        ne_in*sizeof(float));
        memcpy((char *) t_Wf->data + j*ne_in*sizeof(float), &W_folded[(size_t) j*ne_in], ne_in*sizeof(float));
    }

    printf("=== 1. LE CONTRAT avec l'op ggml_wht (le chemin que le graphe empruntera) ===\n");

    ggml_tensor * x_wht  = ggml_wht(ctx, t_x, block);
    check(ggml_get_wht_block(x_wht) == block, "la taille de bloc se relit depuis l'op");
    check(x_wht->ne[0] == ne_in && x_wht->ne[1] == n_tok, "la forme est conservée");
    ggml_tensor * y_rot  = ggml_mul_mat(ctx, t_Wf, x_wht);
    ggml_tensor * y_ref  = ggml_mul_mat(ctx, t_W, t_x);

    // chemin historique : matrice dense + hint (le fork le fait déjà pour le cache KV)
    ggml_tensor * x_blk  = ggml_reshape_2d(ctx, t_x, 1024, (ne_in/1024)*n_tok);
    ggml_tensor * x_hint = ggml_mul_mat(ctx, t_H, x_blk);
    ggml_mul_mat_set_hint(x_hint, GGML_HINT_SRC0_IS_HADAMARD);
    ggml_tensor * y_hint = ggml_mul_mat(ctx, t_Wf, ggml_reshape_2d(ctx, x_hint, ne_in, n_tok));

    ggml_cgraph * gf = ggml_new_graph(ctx);
    ggml_build_forward_expand(gf, y_rot);
    ggml_build_forward_expand(gf, y_ref);
    ggml_build_forward_expand(gf, y_hint);
    check(ggml_backend_graph_compute(backend, gf) == GGML_STATUS_SUCCESS, "le graphe s'exécute");

    double max_rel = 0.0;
    for (int64_t j = 0; j < ne_out; ++j) {
        for (int64_t t = 0; t < n_tok; ++t) {
            // ggml range ne[0] en premier : (j, t) d'un tenseur [ne_out, n_tok] est à t*ne_out + j
            const double got = ((float *) y_rot->data)[(size_t) t*ne_out + j];
            max_rel = fmax(max_rel, fabs(got - ref[(size_t) j*n_tok + t]) / fabs(ref[(size_t) j*n_tok + t]));
        }
    }
    printf("  poids repliés × op WHT : écart relatif maximal %.3e\n", max_rel);
    check(max_rel < 1e-4, "l'op WHT reproduit le produit d'origine");

    double d_hint = 0.0, d_scalar = 0.0;
    for (size_t i = 0; i < (size_t) ne_out*n_tok; ++i) {
        d_hint = fmax(d_hint, fabs(((float *) y_hint->data)[i] - ((float *) y_rot->data)[i]));
    }
    printf("  op WHT contre matrice dense + hint : écart absolu maximal %.3e\n", d_hint);
    check(d_hint < 1e-5, "les deux chemins ggml donnent le MÊME résultat");

    printf("\n=== 2. l'activent transformée est-elle celle du repli hors ligne ? ===\n");
    {
        // le scalaire `obrain_rotate` appliqué à x doit donner exactement ce que l'op calcule
        std::vector<float> x_s = X;
        for (int64_t t = 0; t < n_tok; ++t) obrain_rotate(&x_s[(size_t) t*ne_in], ne_in);
        double d = 0.0;
        for (int64_t t = 0; t < n_tok; ++t)
            for (int64_t i = 0; i < ne_in; ++i)
                d = fmax(d, fabs(x_s[(size_t) t*ne_in + i] - ((float *) x_wht->data)[(size_t) t*ne_in + i]));
        printf("  op WHT contre obrain_rotate (le repli) : écart absolu maximal %.3e\n", d);
        check(d < 1e-5, "l'op et le repli hors ligne sont bien la MÊME transformée");
    }

    printf("\n=== 3. la transformée est une involution à normalisation près : T(T(x)) = x ===\n");
    {
        ggml_tensor * x2 = ggml_wht(ctx, x_wht, block);
        ggml_cgraph * g2 = ggml_new_graph(ctx);
        ggml_build_forward_expand(g2, x2);
        ggml_backend_graph_compute(backend, g2);
        double d = 0.0, n = 0.0;
        for (size_t i = 0; i < (size_t) ne_in*n_tok; ++i) {
            d = fmax(d, fabs(((float *) x2->data)[i] - X[i]));
            n = fmax(n, fabs(X[i]));
        }
        printf("  T(T(x)) contre x : écart absolu maximal %.3e (|x| max %.3e)\n", d, n);
        check(d/n < 1e-6, "appliquer la transformée deux fois rend l'entrée");
    }

    printf("\n=== 4. CHRONOMÉTRAGE — l'op, le chemin historique, et le scalaire ===\n");
    {
        const int64_t rows = 512;
        std::vector<float> buf((size_t) ne_in*rows);
        for (auto & e : buf) e = gauss(rng);

        ggml_tensor * big = ggml_new_tensor_2d(ctx, GGML_TYPE_F32, ne_in, rows);
        memcpy(big->data, buf.data(), buf.size()*sizeof(float));

        ggml_tensor * o_wht = ggml_wht(ctx, big, block);
        ggml_tensor * o_blk = ggml_reshape_2d(ctx, big, 1024, (ne_in/1024)*rows);
        ggml_tensor * o_hint = ggml_mul_mat(ctx, t_H, o_blk);
        ggml_mul_mat_set_hint(o_hint, GGML_HINT_SRC0_IS_HADAMARD);

        struct { const char * name; ggml_tensor * t; } bench[] = {
            { "op WHT (ggml_wht)",        o_wht  },
            { "matrice dense + hint",     o_hint },
        };
        for (auto & b : bench) {
            ggml_cgraph * g = ggml_new_graph(ctx);
            ggml_build_forward_expand(g, b.t);
            ggml_backend_graph_compute(backend, g); // chauffe
            const int reps = 30;
            const double t0 = now_ms();
            for (int r = 0; r < reps; ++r) ggml_backend_graph_compute(backend, g);
            const double dt = (now_ms() - t0)/reps;
            const double bytes = (double) ne_in*rows*4;
            printf("  %-24s : %7.3f ms pour %lld lignes  →  %5.1f Go/s (aller-retour)\n",
                   b.name, dt, (long long) rows, 2.0*bytes/(dt/1000.0)/1e9);
        }

        const int reps = 30;
        const double t0 = now_ms();
        for (int r = 0; r < reps; ++r) obrain_rotate_rows(buf.data(), rows, ne_in);
        const double dt = (now_ms() - t0)/reps;
        const double bytes = (double) ne_in*rows*4;
        printf("  %-24s : %7.3f ms pour %lld lignes  →  %5.1f Go/s (aller-retour, %d fils)\n",
               "obrain_rotate (scalaire)", dt, (long long) rows, 2.0*bytes/(dt/1000.0)/1e9, 1);
    }

    printf("\n=== 5. CHRONOMÉTRAGE de la ternarisation (type TQ2_G128) ===\n");
    {
        const int64_t n = 5120*64;
        std::vector<float> src((size_t) n);
        for (auto & e : src) e = gauss(rng) * 0.01f;
        std::vector<uint8_t> dst(ggml_row_size(GGML_TYPE_TQ2_G128, 5120)*64);
        std::vector<uint8_t> dst256(ggml_row_size(GGML_TYPE_TQ1_0, 5120)*64);

        const int reps = 10;
        const double t0 = now_ms();
        for (int r = 0; r < reps; ++r) quantize_row_tq2_g128_ref(src.data(), (block_tq2_g128 *) dst.data(), n);
        const double t1 = now_ms();
        for (int r = 0; r < reps; ++r) quantize_row_tq1_0_ref(src.data(), (block_tq1_0 *) dst256.data(), n);
        const double t2 = now_ms();

        const double a = (t1 - t0)/reps, b = (t2 - t1)/reps;
        printf("  TQ2_G128 (groupe 128, équilibré + MC) : %6.2f ms  →  %5.1f M poids/s\n",
               a, n/1e6/(a/1000.0));
        printf("  TQ1_0    (groupe 256, amax)           : %6.2f ms  →  %5.1f M poids/s   [facteur %.2fx]\n",
               b, n/1e6/(b/1000.0), a/b);
        printf("  → pour 27,6 G poids : %.1f min à un fil\n", 27.6e9/(1e6*(n/1e6/(a/1000.0)))/60.0);
    }

    ggml_free(ctx);
    ggml_backend_free(backend);

    printf("\n%d passed, %d failed\n", n_passed, n_failed);
    return n_failed > 0 ? 1 : 0;
}
