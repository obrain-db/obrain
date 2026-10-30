// ENGINE-006F — plancher empirique de streaming pour le noyau tq2_g128 / q8_1 / N=1.
//
// Trois kernels SEPARES, memes lectures de poids, meme geometrie de production :
//   S0 STREAM : lit tous les octets de poids du target ; sink minimal observable ;
//               aucun decodage, aucun dp4a, aucune reduction de ligne.
//   S1 DECODE : S0 + decodage ternaire exact ; pas de produit scalaire q8_1.
//   S2 FULL   : calcul MMVQ complet de production.
//
// Geometrie : 5120 lignes x 48 blocs de 128 poids, 34 octets par bloc.
// Layout physique reel (32 octets de codes + 2 octets d'echelle), aucun buffer synthetique.

#include <cstdio>
#include <cstdlib>
#include <cstdint>
#include <vector>
#include <cuda_runtime.h>
#include <cuda_fp16.h>

#define QK_G128 128
struct block_tq2_g128 { uint8_t qs[QK_G128/4]; uint16_t d; };
struct block_q8_1 { uint32_t ds; int8_t qs[32]; };

static const int ROWS = 5120;
static const int NBLK = 48;
static const int TPB  = 128;

static void ck(cudaError_t e, const char * w) {
    if (e != cudaSuccess) { fprintf(stderr, "CUDA_FAIL %s : %s\n", w, cudaGetErrorString(e)); exit(2); }
}

__device__ __forceinline__ int decode_byte(int b) {
    const int c0 = (b >> 0) & 3, c1 = (b >> 2) & 3, c2 = (b >> 4) & 3, c3 = (b >> 6) & 3;
    return ((c0 > 2 ? 1 : c0 - 1) & 0xFF) | (((c1 > 2 ? 1 : c1 - 1) & 0xFF) << 8) |
           (((c2 > 2 ? 1 : c2 - 1) & 0xFF) << 16) | (((c3 > 2 ? 1 : c3 - 1) & 0xFF) << 24);
}
__device__ __forceinline__ int dp4a(int a, int b, int c) { return __dp4a(a, b, c); }

__global__ void k_stream(const uint8_t * __restrict__ w, int * __restrict__ sink) {
    const int row = blockIdx.x, tid = threadIdx.x, kqs = tid % 4;
    const uint8_t * rowbase = w + (size_t) row * NBLK * sizeof(block_tq2_g128);
    // NOTE : u32 est INTERDIT ici. L'adresse vaut base + 34*kbx + 8*kqs et 34 mod 4 = 2 :
    // pour kbx impair l'adresse est 2 mod 4 -> "misaligned address" a l'execution (verifie).
    // L'alignement garanti par le format est 2 octets : on lit donc en u16 (4 lectures).
    uint32_t acc = 0;
    for (int kbx = tid / 4; kbx < NBLK; kbx += 32) {
        const uint16_t * qw = (const uint16_t *) (rowbase + (size_t) kbx * sizeof(block_tq2_g128) + 8 * kqs);
        acc ^= ((uint32_t) qw[0]) | (((uint32_t) qw[1]) << 16);
        acc ^= ((uint32_t) qw[2]) | (((uint32_t) qw[3]) << 16);
    }
    sink[(size_t) row * TPB + tid] = (int) acc;
}

__global__ void k_decode(const uint8_t * __restrict__ w, int * __restrict__ sink) {
    const int row = blockIdx.x, tid = threadIdx.x, kqs = tid % 4;
    const uint8_t * rowbase = w + (size_t) row * NBLK * sizeof(block_tq2_g128);
    int acc = 0;
    for (int kbx = tid / 4; kbx < NBLK; kbx += 32) {
        const uint8_t * qs = rowbase + (size_t) kbx * sizeof(block_tq2_g128) + 8 * kqs;
#pragma unroll
        for (int j = 0; j < 8; ++j) acc += decode_byte((int) qs[j]);
    }
    sink[(size_t) row * TPB + tid] = acc;
}

__global__ void k_full(const uint8_t * __restrict__ w, const int * __restrict__ y,
                       float * __restrict__ out) {
    const int row = blockIdx.x, tid = threadIdx.x, kqs = tid % 4;
    const uint8_t * rowbase = w + (size_t) row * NBLK * sizeof(block_tq2_g128);
    const block_q8_1 * yy = (const block_q8_1 *) y;
    float acc = 0.0f;
    for (int kbx = tid / 4; kbx < NBLK; kbx += 32) {
        const block_tq2_g128 * bq = (const block_tq2_g128 *) (rowbase + (size_t) kbx * sizeof(block_tq2_g128));
        const uint8_t * qs = bq->qs + 8 * kqs;
        int sumi = 0;
#pragma unroll
        for (int t = 0; t < 4; ++t)
#pragma unroll
            for (int j = 0; j < 8; ++j) sumi = dp4a(decode_byte((int) qs[8*t + j]), yy[kbx].qs[t*8 + j], sumi);
        acc += __half2float(*(const __half *) &bq->d) * (float) sumi;
    }
    for (int o = 16; o > 0; o >>= 1) acc += __shfl_down_sync(0xffffffffu, acc, o);
    if ((tid % 32) == 0) out[(size_t) row * 4 + tid / 32] = acc;
}

// G2 : variante a 2 accumulateurs independants, puis somme finale. Meme nombre de dp4a,
// meme decodage, meme chargements : seule la structure de dependance change.
__global__ void k_full_acc2(const uint8_t * __restrict__ w, const int * __restrict__ y,
                            float * __restrict__ out) {
    const int row = blockIdx.x, tid = threadIdx.x, kqs = tid % 4;
    const uint8_t * rowbase = w + (size_t) row * NBLK * sizeof(block_tq2_g128);
    const block_q8_1 * yy = (const block_q8_1 *) y;
    float acc = 0.0f;
    for (int kbx = tid / 4; kbx < NBLK; kbx += 32) {
        const block_tq2_g128 * bq = (const block_tq2_g128 *) (rowbase + (size_t) kbx * sizeof(block_tq2_g128));
        const uint8_t * qs = bq->qs + 8 * kqs;
        int s0 = 0, s1 = 0;
#pragma unroll
        for (int t = 0; t < 2; ++t) {
#pragma unroll
            for (int j = 0; j < 8; ++j) s0 = dp4a(decode_byte((int) qs[8*t + j]), yy[kbx].qs[t*8 + j], s0);
        }
#pragma unroll
        for (int t = 2; t < 4; ++t) {
#pragma unroll
            for (int j = 0; j < 8; ++j) s1 = dp4a(decode_byte((int) qs[8*t + j]), yy[kbx].qs[t*8 + j], s1);
        }
        acc += __half2float(*(const __half *) &bq->d) * (float) (s0 + s1);
    }
    for (int o = 16; o > 0; o >>= 1) acc += __shfl_down_sync(0xffffffffu, acc, o);
    if ((tid % 32) == 0) out[(size_t) row * 4 + tid / 32] = acc;
}

template <typename F>
static double timeit(F launch, int reps) {
    launch(); ck(cudaDeviceSynchronize(), "sync warm");
    cudaEvent_t e0, e1; cudaEventCreate(&e0); cudaEventCreate(&e1);
    cudaEventRecord(e0);
    for (int r = 0; r < reps; ++r) launch();
    cudaEventRecord(e1); ck(cudaEventSynchronize(e1), "sync time");
    float ms = 0; cudaEventElapsedTime(&ms, e0, e1);
    cudaEventDestroy(e0); cudaEventDestroy(e1);
    return ms;
}

int main() {
    const size_t W_BYTES = (size_t) ROWS * NBLK * sizeof(block_tq2_g128);
    printf("ENGINE006F_LOWER_BOUND\n");
    printf("ROWS=%d BLOCKS_PER_ROW=%d BLOCK_BYTES=%zu THREADS_PER_BLOCK=%d\n",
           ROWS, NBLK, sizeof(block_tq2_g128), TPB);
    printf("TARGET_WEIGHT_BYTES_READ=%zu\n", W_BYTES);
    printf("TARGET_Q8_1_BYTES_READ=%zu\n", (size_t) NBLK * sizeof(block_q8_1));

    uint8_t * d_w = nullptr; int * d_y = nullptr; float * d_o = nullptr; int * d_sink = nullptr;
    ck(cudaMalloc(&d_w, W_BYTES), "malloc w");
    ck(cudaMalloc(&d_y, (size_t) NBLK * sizeof(block_q8_1)), "malloc y");
    ck(cudaMalloc(&d_o, (size_t) ROWS * 4 * sizeof(float)), "malloc o");
    ck(cudaMalloc(&d_sink, (size_t) ROWS * TPB * sizeof(int)), "malloc sink");

    std::vector<uint8_t> h(W_BYTES);
    for (size_t i = 0; i < W_BYTES; ++i) h[i] = (uint8_t) ((i * 2246822519u) >> 11);
    ck(cudaMemcpy(d_w, h.data(), W_BYTES, cudaMemcpyHostToDevice), "copy w");
    std::vector<int> hy(NBLK * 8);
    for (size_t i = 0; i < hy.size(); ++i) hy[i] = (int) ((i * 1103515245u + 12345u) >> 8) & 0x01010101;
    ck(cudaMemcpy(d_y, hy.data(), hy.size() * sizeof(int), cudaMemcpyHostToDevice), "copy y");

    const int REPS = 20;
    const double stream_ms = timeit([&]{ k_stream<<<ROWS, TPB>>>(d_w, d_sink); }, REPS);
    const double decode_ms = timeit([&]{ k_decode<<<ROWS, TPB>>>(d_w, d_sink); }, REPS);
    const double full_ms   = timeit([&]{ k_full  <<<ROWS, TPB>>>(d_w, d_y, d_o); }, REPS);
    const double acc2_ms   = timeit([&]{ k_full_acc2<<<ROWS, TPB>>>(d_w, d_y, d_o); }, REPS);

    const double stream_us = 1000.0 * stream_ms / REPS;
    const double decode_us = 1000.0 * decode_ms / REPS;
    const double full_us   = 1000.0 * full_ms   / REPS;

    printf("\nSTREAM_US=%.4f\nDECODE_US=%.4f\nFULL_MMVQ_US=%.4f\n", stream_us, decode_us, full_us);
    printf("DECODE_INCREMENT_US=%.4f\n", decode_us - stream_us);
    printf("DOT_REDUCTION_INCREMENT_US=%.4f\n", full_us - decode_us);
    printf("STREAM_EFFECTIVE_GB_S=%.2f\n", (W_BYTES / 1e9) / (stream_us / 1e6));
    printf("FULL_EFFECTIVE_GB_S=%.2f\n",   (W_BYTES / 1e9) / (full_us   / 1e6));
    printf("FULL_OVER_STREAM_RATIO=%.3f\n", full_us / stream_us);
    const double acc2_us = 1000.0 * acc2_ms / REPS;
    printf("\nG2_FULL_ACC2_US=%.4f\n", acc2_us);
    printf("G2_ACC2_GAIN_US=%.4f\n", full_us - acc2_us);
    printf("G2_ACC2_RATIO=%.4f\n", acc2_us / full_us);

    // --- controle HOT/COLD : jeu de travail >> 2 x L2 (25,2 Mo mesurage 006B) ---
    // On fait tourner NMAT matrices distinctes de 8,36 Mo avant chaque mesure du target.
    const int NMAT = 8;                        // 8 x 8,36 Mo = 66,9 Mo > 2 x L2
    const size_t WS = W_BYTES * NMAT;
    uint8_t * d_ws = nullptr; int * d_wsink = nullptr; float * d_wo = nullptr;
    ck(cudaMalloc(&d_ws, WS), "malloc ws");
    ck(cudaMalloc(&d_wsink, (size_t) ROWS * TPB * sizeof(int)), "malloc wsink");
    ck(cudaMalloc(&d_wo, (size_t) ROWS * 4 * sizeof(float)), "malloc wo");
    std::vector<uint8_t> hws(WS);
    for (size_t i = 0; i < WS; ++i) hws[i] = (uint8_t) ((i * 2654435761u) >> 13);
    ck(cudaMemcpy(d_ws, hws.data(), WS, cudaMemcpyHostToDevice), "copy ws");

    auto flush = [&]() {                        // parcourt tout le jeu de travail avant le target.
        // Purement en lecture (k_stream) : chaque matrice du jeu de travail a la geometrie exacte
        // d'un tenseur cible. On n'utilise PAS k_full ici : le buffer plat n'a pas de sortie
        // dimensionnee pour lui (erreur d'acces illegale rencontree, cf. HARNESS_DEFECT_24).
        for (int k = 0; k < NMAT; ++k) k_stream<<<ROWS, TPB>>>(d_ws + (size_t) k * W_BYTES, d_wsink);
    };

    // HOT : le target reste resident (mesures precedentes repetees)
    cudaEvent_t h0, h1; cudaEventCreate(&h0); cudaEventCreate(&h1);
    k_stream<<<ROWS, TPB>>>(d_w, d_sink); ck(cudaDeviceSynchronize(), "warm hot");
    cudaEventRecord(h0);
    for (int r = 0; r < REPS; ++r) k_stream<<<ROWS, TPB>>>(d_w, d_sink);
    cudaEventRecord(h1); ck(cudaEventSynchronize(h1), "sync hot");
    float hot_ms_s = 0; cudaEventElapsedTime(&hot_ms_s, h0, h1);

    // COLD : on vide le cache avec le jeu de travail avant CHAQUE invocation du target
    cudaEventRecord(h0);
    for (int r = 0; r < REPS; ++r) { flush(); k_stream<<<ROWS, TPB>>>(d_w, d_sink); }
    cudaEventRecord(h1); ck(cudaEventSynchronize(h1), "sync cold");
    float cold_ms_s = 0; cudaEventElapsedTime(&cold_ms_s, h0, h1);

    k_full<<<ROWS, TPB>>>(d_w, d_y, d_o); ck(cudaDeviceSynchronize(), "warm hot f");
    cudaEventRecord(h0);
    for (int r = 0; r < REPS; ++r) k_full<<<ROWS, TPB>>>(d_w, d_y, d_o);
    cudaEventRecord(h1); ck(cudaEventSynchronize(h1), "sync hot f");
    float hot_ms_f = 0; cudaEventElapsedTime(&hot_ms_f, h0, h1);

    cudaEventRecord(h0);
    for (int r = 0; r < REPS; ++r) { flush(); k_full<<<ROWS, TPB>>>(d_w, d_y, d_o); }
    cudaEventRecord(h1); ck(cudaEventSynchronize(h1), "sync cold f");
    float cold_ms_f = 0; cudaEventElapsedTime(&cold_ms_f, h0, h1);

    // le flush lui-meme, mesure seul, pour retrancher son cout des mesures froides
    cudaEventRecord(h0);
    for (int r = 0; r < REPS; ++r) flush();
    cudaEventRecord(h1); ck(cudaEventSynchronize(h1), "sync flush");
    float flush_ms = 0; cudaEventElapsedTime(&flush_ms, h0, h1);

    const double sh = 1000.0*hot_ms_s/REPS,  sc = 1000.0*(cold_ms_s-flush_ms)/REPS;
    const double fh = 1000.0*hot_ms_f/REPS,  fc = 1000.0*(cold_ms_f-flush_ms)/REPS;
    printf("\n--- CONTROLE HOT/COLD (jeu de travail %d x 8,36 Mo = %.1f Mo) ---\n", NMAT, WS/1024.0/1024.0);
    printf("STREAM_HOT_US=%.3f\nSTREAM_COLD_US=%.3f\n", sh, sc);
    printf("FULL_HOT_US=%.3f\nFULL_COLD_US=%.3f\n", fh, fc);
    printf("STREAM_CACHE_SENSITIVITY=%.3f\n", sc/sh);
    printf("FULL_CACHE_SENSITIVITY=%.3f\n", fc/fh);
    cudaEventDestroy(h0); cudaEventDestroy(h1);
    cudaFree(d_ws); cudaFree(d_wsink); cudaFree(d_wo);
    cudaFree(d_w); cudaFree(d_y); cudaFree(d_o); cudaFree(d_sink);
    return 0;
}
