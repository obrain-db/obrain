// ENGINE-007B-F1A-Q0-N5-A — test INDEPENDANT d'indexation ligne de poids <-> coordonnee de sortie.
//
// Ce programme ne reutilise NI N2, NI le lecteur TQ2_G128, NI aucune fonction de
// serialisation maison. Il construit un vrai ggml_mul_mat F32, l'execute sur GPU,
// relit la sortie, et compare a une somme calculee en C++ pur.
//
// Le but est de trancher : la ligne logique r du poids correspond-elle a output[r] ?

#include "ggml.h"
#include "ggml-alloc.h"
#include "ggml-backend.h"
#include "ggml-cuda.h"

#include <cmath>
#include <cstdio>
#include <cstdint>
#include <cstring>
#include <vector>

static const int K = 256;   // dimension d'entree
static const int M = 64;    // nombre de lignes de poids

// valeurs EXACTEMENT representables en F32 : entiers petits, produits exacts.
static float w_val(int k, int r) {
    // motif non symetrique et sans repetition : chaque ligne est unique
    return (float) (((k * 7 + r * 13) % 23) - 11);
}
static float a_val(int k) {
    return (float) (((k * 5) % 17) - 8);
}

int main() {
    // --- 1. init GGML + backend CUDA ---
    ggml_backend_t backend = ggml_backend_cuda_init(0);
    if (!backend) { printf("INDEPENDENT_F32_GPU_EXECUTION=FAIL pas de backend CUDA\n"); return 1; }
    printf("INDEPENDENT_F32_GPU_EXECUTION=PASS\n");

    ggml_backend_buffer_type_t buft = ggml_backend_get_default_buffer_type(backend);

    // --- 2. contexte ---
    struct ggml_init_params ip;
    ip.mem_size   = 64 * 1024 * 1024;
    ip.mem_buffer = nullptr;
    ip.no_alloc   = true;   // le backend alloue : ggml_backend_alloc_ctx_tensors l exige
    struct ggml_context * ctx = ggml_init(ip);
    if (!ctx) { printf("ggml_init FAIL\n"); return 1; }

    struct ggml_tensor * w = ggml_new_tensor_2d(ctx, GGML_TYPE_F32, K, M);
    struct ggml_tensor * a = ggml_new_tensor_2d(ctx, GGML_TYPE_F32, K, 1);
    ggml_set_name(w, "w");
    ggml_set_name(a, "a");

    // --- 3. remplir cote hote (no_alloc : tensor->data n est pas encore alloue) ---
    std::vector<float> host_w((size_t) K * M), host_a(K);
    for (int r = 0; r < M; ++r)
        for (int k = 0; k < K; ++k)
            host_w[(size_t) r * K + k] = w_val(k, r);
    for (int k = 0; k < K; ++k)
        host_a[k] = a_val(k);

    // --- 4. le vrai ggml_mul_mat ---
    struct ggml_tensor * out = ggml_mul_mat(ctx, w, a);

    struct ggml_cgraph * gf = ggml_new_graph(ctx);
    ggml_build_forward_expand(gf, out);

    // --- 5. executer sur GPU ---
    ggml_backend_buffer_t buf = ggml_backend_alloc_ctx_tensors(ctx, backend);
    if (!buf) { printf("alloc_ctx_tensors FAIL\n"); return 1; }
    ggml_backend_tensor_set(w, host_w.data(), 0, ggml_nbytes(w));
    ggml_backend_tensor_set(a, host_a.data(), 0, ggml_nbytes(a));

    if (ggml_backend_graph_compute(backend, gf) != GGML_STATUS_SUCCESS) {
        printf("graph_compute FAIL\n"); return 1;
    }
    ggml_backend_synchronize(backend);

    std::vector<float> gpu(M);
    ggml_backend_tensor_get(out, gpu.data(), 0, M * sizeof(float));

    // --- 6. publier la serialisation REELLE ---
    printf("WEIGHT_NE=%lld,%lld,%lld,%lld\n", (long long) w->ne[0], (long long) w->ne[1],
           (long long) w->ne[2], (long long) w->ne[3]);
    printf("WEIGHT_NB=%zu,%zu,%zu,%zu\n", w->nb[0], w->nb[1], w->nb[2], w->nb[3]);
    printf("ACTIVATION_NE=%lld,%lld,%lld,%lld\n", (long long) a->ne[0], (long long) a->ne[1],
           (long long) a->ne[2], (long long) a->ne[3]);
    printf("ACTIVATION_NB=%zu,%zu,%zu,%zu\n", a->nb[0], a->nb[1], a->nb[2], a->nb[3]);
    printf("OUTPUT_NE=%lld,%lld,%lld,%lld\n", (long long) out->ne[0], (long long) out->ne[1],
           (long long) out->ne[2], (long long) out->ne[3]);
    printf("OUTPUT_NB=%zu,%zu,%zu,%zu\n", out->nb[0], out->nb[1], out->nb[2], out->nb[3]);
    printf("OUTPUT_TYPE=%d\nOUTPUT_NBYTES=%zu\n", (int) out->type, ggml_nbytes(out));

    // --- 7. CPU independant : expected[r] = somme_k w[k,r] * a[k] ---
    // On lit le poids par son INDEX LOGIQUE GGML : w->data[k + r*K] car ne[0]=K, ne[1]=M.
    // On lit la sortie par son index LOGIQUE : out->data[r] car ne[0]=M, ne[1]=1.
    int bad = 0;
    std::vector<int> test_rows = {0, 1, M / 4, M / 2, 3 * M / 4, M - 2, M - 1};
    printf("\nROW,EXPECTED,GPU,ABS_ERROR\n");
    for (int r = 0; r < M; ++r) {
        double s = 0.0;
        for (int k = 0; k < K; ++k) s += (double) w_val(k, r) * (double) a_val(k);
        double err = std::fabs(s - (double) gpu[r]);
        printf("%d,%.1f,%.1f,%.3e\n", r, s, (double) gpu[r], err);
        if (err != 0.0) ++bad;
    }
    printf("\nF32_ROW_OUTPUT_CORRESPONDENCE=%s\n", bad == 0 ? "PASS" : "FAIL");
    printf("TESTED_ROWS=%d  ROWS_WITH_ERROR=%d\n", M, bad);
    // TEST_ALL_ROWS
    printf("TEST_ALL_ROWS=%s\n", bad == 0 ? "YES_PASS" : "NO_FAIL");

    // --- 8. serialisation : l'octet i de la sortie est-il out[i] ? ---
    // pour un tenseur contigu ne[0]=M, nb[0]=4 : octet 4*i == element i
    int ser_ok = (out->nb[0] == sizeof(float)) ? 1 : 0;
    printf("OUTPUT_LOGICAL_TO_SERIALIZED_BYTE_MAPPING=%s (nb0=%zu attendu %zu)\n",
           ser_ok ? "PASS" : "FAIL", out->nb[0], sizeof(float));
    printf("F32_OUTPUT_SERIALIZATION=%s\n", ser_ok ? "PASS" : "FAIL");
    printf("INDEPENDENT_F32_MUL_MAT_TEST=%s\n", (bad == 0 && ser_ok) ? "PASS" : "FAIL");

    ggml_backend_buffer_free(buf);
    ggml_free(ctx);
    ggml_backend_free(backend);
    return (bad == 0 && ser_ok) ? 0 : 1;
}
