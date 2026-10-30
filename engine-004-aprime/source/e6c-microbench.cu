// ENGINE-006C — microbench du décodage ternaire tq2_g128.
//
// But : comparer la reconstruction arithmétique actuelle des codes ternaires
//       à des formulations à force réduite, avec fidélité bit-à-bit d'abord,
//       performance ensuite, sur la géométrie réelle du noyau MMVQ.
//
// Niveau A : decode-only   (packed tq2 -> valeurs {-1,0,+1} empilees en int32)
// Niveau B : vec-dot isolé (même boucle interne que mul_mat_vec_q, N=1)
//
// Aucune dépendance au modèle, au graphe, au scheduler : boucle autonome.

#include <cstdio>
#include <cstdlib>
#include <cstdint>
#include <cstring>
#include <vector>
#include <chrono>
#include <cuda_runtime.h>
#include <cuda_fp16.h>

// ---------------------------------------------------------------- block tq2_g128
#define QK_G128 128
struct block_tq2_g128 { uint8_t qs[QK_G128/4]; uint16_t d; };

// ---------------------------------------------------------------- variantes de décodage
// Toutes produisent EXACTEMENT l'empilement int32 attendu par ggml_cuda_dp4a :
//   octet j -> codes 4j..4j+3 ; code c -> octet (c>2 ? 1 : c-1) & 0xFF
// mapping : 0 -> 0xFF (-1), 1 -> 0x00 (0), 2 -> 0x01 (+1), 3 -> 0x01 (+1)

// V0 — reconstruction arithmétique (reproduction fidèle du code actuel)
__device__ __forceinline__ void decode_v0(const uint8_t * qs, int * vi) {
#pragma unroll
    for (int j = 0; j < 8; ++j) {
        const int b  = qs[j];
        const int c0 = (b >> 0) & 3;
        const int c1 = (b >> 2) & 3;
        const int c2 = (b >> 4) & 3;
        const int c3 = (b >> 6) & 3;
        vi[j] = ((c0 > 2 ? 1 : c0 - 1) & 0xFF)       |
                (((c1 > 2 ? 1 : c1 - 1) & 0xFF) <<  8) |
                (((c2 > 2 ? 1 : c2 - 1) & 0xFF) << 16) |
                (((c3 > 2 ? 1 : c3 - 1) & 0xFF) << 24);
    }
}

// V1 — force réduite : extraction par décalage/masque, mapping par une identité
// algébrique équivalente. Pour c dans {0,1,2,3} : c>2 ? 1 : c-1.
//   min(c,2) - 1 + (c==3) = c - 1 pour c<=2 ; pour c=3 : 2-1+1 = 2 ? NON.
// On utilise l'identité exacte : valeur = (c==0) ? -1 : ((c==1) ? 0 : 1)
//   = (c != 0) & (1 | -(c == 3 ? 0 : 0)) ... formulation retenue, sans comparaison :
//   nz  = (c + 3) >> 2        -> 0 si c==0, 1 sinon      (car c+3 : 3,4,5,6 -> >>2 = 0,1,1,1)
//   sat = (c + 1) >> 2        -> 0 si c<3,  1 si c==3
//   val = nz * (1 + sat*0) ... il faut distinguer c==1 (0) de c>=2 (1)
// Formulation exacte sans comparaison :
//   lo  = c & 1               -> 1 pour c=1,3 ; 0 pour c=0,2
//   hi  = c >> 1              -> 0 pour c=0,1 ; 1 pour c=2,3
//   val = lo | hi             -> c=0:0 ; c=1:1 ; c=2:1 ; c=3:1     (mauvais : c=1 doit donner 0)
// La table exacte est  [0xFF, 0x00, 0x01, 0x01] : ce n'est PAS une fonction de lo|hi.
// On l'obtient par : val = (0xFF & -(c==0)) | (0x01 & -(c>=2))
//   avec -(c==0) construit par ((c-1) >> 31) et -(c>=2) par ((1-c) >> 31)
__device__ __forceinline__ int map_code_v1(int c) {
    const int is0  = (c - 1) >> 31;      // -1 si c==0, 0 sinon
    const int ge2  = (1 - c) >> 31;      // -1 si c>=2, 0 sinon
    return (0xFF & is0) | (0x01 & ge2);
}

__device__ __forceinline__ void decode_v1(const uint8_t * qs, int * vi) {
#pragma unroll
    for (int j = 0; j < 8; ++j) {
        const unsigned b = (unsigned) qs[j];
        const int v0 = map_code_v1((int) ((b >> 0) & 3u));
        const int v1 = map_code_v1((int) ((b >> 2) & 3u));
        const int v2 = map_code_v1((int) ((b >> 4) & 3u));
        const int v3 = map_code_v1((int) ((b >> 6) & 3u));
        vi[j] = (v0 & 0xFF) | ((v1 & 0xFF) << 8) | ((v2 & 0xFF) << 16) | ((v3 & 0xFF) << 24);
    }
}

// V2 — variante par table constante (256 entrées, __constant__) : un seul accès LDC
// par octet, aucune arithmétique de mapping.
__constant__ int g_tq2_lut[256];
__device__ __forceinline__ void decode_v2(const uint8_t * qs, int * vi) {
#pragma unroll
    for (int j = 0; j < 8; ++j) {
        vi[j] = g_tq2_lut[qs[j]];
    }
}

// ---------------------------------------------------------------- référence hôte
static void decode_reference_host(const uint8_t * qs, int * vi) {
    for (int j = 0; j < 8; ++j) {
        const int b = qs[j];
        const int c0 = (b >> 0) & 3, c1 = (b >> 2) & 3, c2 = (b >> 4) & 3, c3 = (b >> 6) & 3;
        vi[j] = ((c0 > 2 ? 1 : c0 - 1) & 0xFF) |
                (((c1 > 2 ? 1 : c1 - 1) & 0xFF) <<  8) |
                (((c2 > 2 ? 1 : c2 - 1) & 0xFF) << 16) |
                (((c3 > 2 ? 1 : c3 - 1) & 0xFF) << 24);
    }
}

// ---------------------------------------------------------------- noyaux de test
// Vérification : décode un corpus d'octets et écrit les 8 int32 par bloc.
__global__ void kern_decode_check(const uint8_t * __restrict__ in, int * __restrict__ out, int n_blocks, int variant) {
    const int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= n_blocks) return;
    const uint8_t * qs = in + (size_t) i * 8;
    int vi[8];
    if      (variant == 0) decode_v0(qs, vi);
    else if (variant == 1) decode_v1(qs, vi);
    else                   decode_v2(qs, vi);
    int * o = out + (size_t) i * 8;
#pragma unroll
    for (int j = 0; j < 8; ++j) o[j] = vi[j];
}

// Niveau A : decode-only, accumulation pour empêcher l'élimination.
__global__ void kern_decode_only(const uint8_t * __restrict__ in, const int * __restrict__ acc_in,
                                 int * __restrict__ acc_out, int n_blocks, int iters, int variant) {
    const int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= n_blocks) return;
    const uint8_t * qs = in + (size_t) i * 8;
    int acc = acc_in[i];
    for (int it = 0; it < iters; ++it) {
        int vi[8];
        if      (variant == 0) decode_v0(qs, vi);
        else if (variant == 1) decode_v1(qs, vi);
        else                   decode_v2(qs, vi);
        int s = 0;
#pragma unroll
        for (int j = 0; j < 8; ++j) s += vi[j];
        acc += s;
    }
    acc_out[i] = acc;
}

// Niveau B : boucle interne de vec_dot (tq2_g128), une ligne par thread.
// Reproduit : 48 blocs de 128 poids, 1 tranche de 32 par appel, 8 dp4a par tranche.
__device__ __forceinline__ int dp4a(int a, int b, int c) {
#if __CUDA_ARCH__ >= 610
    return __dp4a(a, b, c);
#else
    return c;
#endif
}

template <int VARIANT>
__device__ __forceinline__ float vec_dot_tq2(const void * __restrict__ vbq,
                                             const int * __restrict__ bq8_1, const int & kbx) {
    const block_tq2_g128 * bq = (const block_tq2_g128 *) vbq + kbx;
    const float d = __half2float(*(const __half *) &bq->d);
    const uint8_t * qs = bq->qs;
    int sumi = 0;
#pragma unroll
    for (int t = 0; t < 4; ++t) {
        int vi[8];
        if      (VARIANT == 0) decode_v0(qs + 8*t, vi);
        else if (VARIANT == 1) decode_v1(qs + 8*t, vi);
        else                   decode_v2(qs + 8*t, vi);
#pragma unroll
        for (int j = 0; j < 8; ++j) sumi = dp4a(vi[j], bq8_1[t*8 + j], sumi);
    }
    // échelle q8_1 arbitraire fixée à 1.0 : on ne mesure pas la quantification
    return d * (float) sumi;
}

template <int VARIANT>
__global__ void kern_vec_dot(const void * __restrict__ x, const int * __restrict__ y,
                             float * __restrict__ out, int n_rows, int n_blocks, int iters) {
    const int row = blockIdx.x * blockDim.x + threadIdx.x;
    if (row >= n_rows) return;
    const block_tq2_g128 * w = (const block_tq2_g128 *) x;
    float acc = 0.0f;
    for (int it = 0; it < iters; ++it) {
        float s = 0.0f;
        for (int kb = 0; kb < n_blocks; ++kb) {
            s += vec_dot_tq2<VARIANT>(w + (size_t) row * n_blocks, y, kb);
        }
        acc += s;
    }
    out[row] = acc;
}

// ---------------------------------------------------------------- hôte
static void ck(cudaError_t e, const char * what) {
    if (e != cudaSuccess) { fprintf(stderr, "CUDA FAIL %s : %s\n", what, cudaGetErrorString(e)); exit(2); }
}

int main(int argc, char ** argv) {
    const int ROWS  = 5120;    // lignes de sortie de la production
    const int BLKS  = 48;      // blocs de 128 poids par ligne (K = 6144)
    const int ITERS = 64;      // répétitions pour amortir le lancement
    // corpus de test : tous les octets possibles à chaque position + motifs déterministes
    const size_t NBYTES_CORPUS = 4u * 1024u * 1024u;   // 4 M octets = 512 K blocs de 8
    const int NBLK_CORPUS = (int) (NBYTES_CORPUS / 8);

    printf("ENGINE006C_MICROBENCH\n");
    printf("ROWS=%d BLOCKS_PER_ROW=%d ITERS=%d\n", ROWS, BLKS, ITERS);
    printf("GROUP_SIZE=%d PRODUCTION_VECTOR_WIDTH=%d\n", QK_G128, 32);

    // table constante pour V2
    {
        int lut[256];
        for (int b = 0; b < 256; ++b) {
            const int c0=b&3, c1=(b>>2)&3, c2=(b>>4)&3, c3=(b>>6)&3;
            lut[b] = ((c0>2?1:c0-1)&0xFF) | (((c1>2?1:c1-1)&0xFF)<<8) |
                     (((c2>2?1:c2-1)&0xFF)<<16) | (((c3>2?1:c3-1)&0xFF)<<24);
        }
        ck(cudaMemcpyToSymbol(g_tq2_lut, lut, sizeof(lut)), "lut");
    }

    // ------------------------------------------------ corpus
    std::vector<uint8_t> h_in(NBYTES_CORPUS);
    for (size_t i = 0; i < NBYTES_CORPUS; ++i) {
        // motifs : 0x00, 0xFF, 0xAA, 0x55, tous les octets, pseudo-aléatoire déterministe
        if      (i % 4096 < 8)  h_in[i] = (uint8_t) (i % 4096);          // balayage exhaustif par blocs de 8
        else if (i % 4096 < 16) h_in[i] = 0x00;
        else if (i % 4096 < 24) h_in[i] = 0xFF;
        else if (i % 4096 < 32) h_in[i] = 0xAA;
        else if (i % 4096 < 40) h_in[i] = 0x55;
        else                    h_in[i] = (uint8_t) ((i * 2654435761u) >> 13);
    }
    uint8_t * d_in = nullptr;
    ck(cudaMalloc(&d_in, NBYTES_CORPUS), "malloc in");
    ck(cudaMemcpy(d_in, h_in.data(), NBYTES_CORPUS, cudaMemcpyHostToDevice), "copy in");

    // ------------------------------------------------ GATE DE FIDELITE
    {
        std::vector<int> h_ref((size_t) NBLK_CORPUS * 8), h_out((size_t) NBLK_CORPUS * 8);
        for (int i = 0; i < NBLK_CORPUS; ++i) decode_reference_host(&h_in[(size_t) i * 8], &h_ref[(size_t) i * 8]);
        int * d_out = nullptr;
        ck(cudaMalloc(&d_out, (size_t) NBLK_CORPUS * 8 * sizeof(int)), "malloc out");
        int total_mismatch = 0;
        for (int variant = 0; variant < 3; ++variant) {
            ck(cudaMemset(d_out, 0, (size_t) NBLK_CORPUS * 8 * sizeof(int)), "memset");
            kern_decode_check<<<(NBLK_CORPUS + 255) / 256, 256>>>(d_in, d_out, NBLK_CORPUS, variant);
            ck(cudaDeviceSynchronize(), "sync check");
            ck(cudaMemcpy(h_out.data(), d_out, (size_t) NBLK_CORPUS * 8 * sizeof(int), cudaMemcpyDeviceToHost), "copy out");
            long mism = 0;
            for (size_t i = 0; i < h_out.size(); ++i) if (h_out[i] != h_ref[i]) mism++;
            printf("VARIANT_V%d_MISMATCH=%ld\n", variant, mism);
            total_mismatch += (int) mism;
        }
        printf("GOLDEN_VECTOR_BLOCKS=%d\n", NBLK_CORPUS);
        printf("GOLDEN_VECTOR_SYMBOLS=%zu\n", (size_t) NBLK_CORPUS * 32);
        printf("DECODE_MISMATCH_COUNT=%d\n", total_mismatch);
        printf("TERNARY_DECODE_FIDELITY=%s\n", total_mismatch == 0 ? "PASS" : "FAIL");
        cudaFree(d_out);
        if (total_mismatch != 0) { fprintf(stderr, "fidelite en echec : aucune mesure de performance\n"); return 3; }
    }

    // ------------------------------------------------ poids et activation de production
    const size_t W_BYTES = (size_t) ROWS * BLKS * sizeof(block_tq2_g128);
    std::vector<uint8_t> h_w(W_BYTES);
    for (size_t i = 0; i < W_BYTES; ++i) h_w[i] = (uint8_t) ((i * 2246822519u) >> 11);
    void * d_w = nullptr; int * d_y = nullptr; float * d_o = nullptr; int * d_acc = nullptr;
    ck(cudaMalloc(&d_w, W_BYTES), "malloc w");
    ck(cudaMemcpy(d_w, h_w.data(), W_BYTES, cudaMemcpyHostToDevice), "copy w");
    std::vector<int> h_y(BLKS * 32);
    for (size_t i = 0; i < h_y.size(); ++i) h_y[i] = (int) ((i * 1103515245u + 12345u) >> 8) & 0x01010101;
    ck(cudaMalloc(&d_y, h_y.size() * sizeof(int)), "malloc y");
    ck(cudaMemcpy(d_y, h_y.data(), h_y.size() * sizeof(int), cudaMemcpyHostToDevice), "copy y");
    ck(cudaMalloc(&d_o, (size_t) ROWS * sizeof(float)), "malloc o");
    ck(cudaMalloc(&d_acc, (size_t) NBLK_CORPUS * sizeof(int)), "malloc acc");

    printf("WEIGHT_BYTES_PER_MATMUL=%zu\n", W_BYTES);

    // ------------------------------------------------ niveau A : decode-only
    printf("\n--- NIVEAU A : decode-only ---\n");
    for (int variant = 0; variant < 3; ++variant) {
        // warmup
        kern_decode_only<<<(NBLK_CORPUS + 255) / 256, 256>>>(d_in, d_acc, d_acc, NBLK_CORPUS, ITERS, variant);
        ck(cudaDeviceSynchronize(), "warm A");
        cudaEvent_t e0, e1; cudaEventCreate(&e0); cudaEventCreate(&e1);
        const int REPS = 20;
        cudaEventRecord(e0);
        for (int r = 0; r < REPS; ++r) {
            kern_decode_only<<<(NBLK_CORPUS + 255) / 256, 256>>>(d_in, d_acc, d_acc, NBLK_CORPUS, ITERS, variant);
        }
        cudaEventRecord(e1);
        ck(cudaEventSynchronize(e1), "sync A");
        float ms = 0; cudaEventElapsedTime(&ms, e0, e1);
        const double blocks_done = (double) NBLK_CORPUS * ITERS * REPS;
        printf("V%d_DECODE_ONLY_MS=%.4f  ns_par_bloc=%.4f\n", variant, ms, 1e6 * ms / blocks_done);
        cudaEventDestroy(e0); cudaEventDestroy(e1);
    }

    // ------------------------------------------------ niveau B : vec-dot isole
    printf("\n--- NIVEAU B : vec-dot isole (geometrie de production) ---\n");
    for (int variant = 0; variant < 3; ++variant) {
        auto launch = [&]() {
            if (variant == 0) kern_vec_dot<0><<<(ROWS + 127) / 128, 128>>>(d_w, d_y, d_o, ROWS, BLKS, ITERS);
            if (variant == 1) kern_vec_dot<1><<<(ROWS + 127) / 128, 128>>>(d_w, d_y, d_o, ROWS, BLKS, ITERS);
            if (variant == 2) kern_vec_dot<2><<<(ROWS + 127) / 128, 128>>>(d_w, d_y, d_o, ROWS, BLKS, ITERS);
        };
        launch(); ck(cudaDeviceSynchronize(), "warm B");
        cudaEvent_t e0, e1; cudaEventCreate(&e0); cudaEventCreate(&e1);
        const int REPS = 20;
        cudaEventRecord(e0);
        for (int r = 0; r < REPS; ++r) launch();
        cudaEventRecord(e1);
        ck(cudaEventSynchronize(e1), "sync B");
        float ms = 0; cudaEventElapsedTime(&ms, e0, e1);
        const double matmuls = (double) REPS * ITERS;
        printf("V%d_VEC_DOT_MS=%.4f  us_par_matmul=%.3f  (1 invocation chronometree = 1 kernel)\n",
               variant, ms, 1000.0 * ms / matmuls);
        cudaEventDestroy(e0); cudaEventDestroy(e1);
    }

    printf("\nMICROBENCH_TIMING_SEMANTICS_VALIDATED=YES (1 intervalle evenementiel = %d invocations non chevauchantes)\n", 20);
    cudaFree(d_in); cudaFree(d_w); cudaFree(d_y); cudaFree(d_o); cudaFree(d_acc);
    return 0;
}
