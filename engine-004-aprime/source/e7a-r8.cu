// ENGINE-007A-R8 — retrait de l artefact d instrumentation, retest de la fidelite P48.
//
// R7B a montre que le garde `span_count * 0.0f` (numeriquement nul) creait une SECONDE racine
// entiere, une conversion supplementaire, un FMUL et une region de convergence en plus.
//   SOURCE_LEVEL_NOOP != BINARY_LEVEL_NOOP
//
// R8 retire ce garde SANS le remplacer par aucun autre keepalive : le noyau possede deja un
// resultat IDP reel, une reduction reelle et un store reel, qui rendent le corps vivant.
//
// Aucun timing dans cette passe. Objectif unique : P48 vs K_FULL, bit a bit.

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

// R8 : masque runtime sur les blocs kbx, AUCUN artefact, AUCUN keepalive ajoute.
// Le parametre `span_count` a ete entierement supprime.
__global__ void k_r8_passes(const uint8_t * __restrict__ w, const int * __restrict__ y,
                            float * __restrict__ out, unsigned long long kbx_mask) {
    const int row = blockIdx.x, tid = threadIdx.x, kqs = tid % 4;
    const int lane = tid % 32, wid = tid / 32;
    const uint8_t * rowbase = w + (size_t) row * NBLK * sizeof(block_tq2_g128);
    const block_q8_1 * yy = (const block_q8_1 *) y;
    float acc = 0.0f;
    for (int kbx = tid / 4; kbx < NBLK; kbx += 32) {
        if ((kbx_mask & (1ull << kbx)) == 0ull) continue;   // selection par bloc
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
    __shared__ float partial[4];
    if (lane == 0) partial[wid] = acc;
    __syncthreads();
    if (tid == 0) { float s = 0; for (int i = 0; i < 4; ++i) s += partial[i]; out[row] = s; }
}

// reference native, identique a 006F/R3/R4/R6
__global__ void k_full_native(const uint8_t * __restrict__ w, const int * __restrict__ y,
                              float * __restrict__ out) {
    const int row = blockIdx.x, tid = threadIdx.x, kqs = tid % 4;
    const int lane = tid % 32, wid = tid / 32;
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
    __shared__ float partial[4];
    if (lane == 0) partial[wid] = acc;
    __syncthreads();
    if (tid == 0) { float s = 0; for (int i = 0; i < 4; ++i) s += partial[i]; out[row] = s; }
}

int main() {
    const size_t W_BYTES = (size_t) ROWS * NBLK * sizeof(block_tq2_g128);
    uint8_t * d_w = nullptr; int * d_y = nullptr; float * d_o = nullptr; float * d_ref = nullptr;
    ck(cudaMalloc(&d_w, W_BYTES), "mw");
    ck(cudaMalloc(&d_y, (size_t) NBLK * sizeof(block_q8_1)), "my");
    ck(cudaMalloc(&d_o, (size_t) ROWS * sizeof(float)), "mo");
    ck(cudaMalloc(&d_ref, (size_t) ROWS * sizeof(float)), "mref");
    std::vector<uint8_t> h(W_BYTES);
    for (size_t i = 0; i < W_BYTES; ++i) h[i] = (uint8_t) ((i * 2246822519u) >> 11);
    ck(cudaMemcpy(d_w, h.data(), W_BYTES, cudaMemcpyHostToDevice), "cw");
    std::vector<int> hy(NBLK * 8);
    for (size_t i = 0; i < hy.size(); ++i) hy[i] = (int) ((i * 1103515245u + 12345u) >> 8) & 0x01010101;
    ck(cudaMemcpy(d_y, hy.data(), hy.size()*sizeof(int), cudaMemcpyHostToDevice), "cy");

    const unsigned long long M48 = 0x0000FFFFFFFFFFFFull;
    printf("ENGINE007A_R8_REMOVE_INSTRUMENTATION_ARTIFACT\n");
    printf("ARTEFACT_RETIRE=span_count_Et_done_compteur\nADDITIONAL_KEEPALIVE_ADDED=NONE\n\n");

    k_full_native<<<ROWS,TPB>>>(d_w, d_y, d_ref); ck(cudaDeviceSynchronize(), "ref");
    k_r8_passes  <<<ROWS,TPB>>>(d_w, d_y, d_o, M48); ck(cudaDeviceSynchronize(), "r8");
    std::vector<float> a(ROWS), b(ROWS);
    ck(cudaMemcpy(a.data(), d_ref, ROWS*sizeof(float), cudaMemcpyDeviceToHost), "ca");
    ck(cudaMemcpy(b.data(), d_o, ROWS*sizeof(float), cudaMemcpyDeviceToHost), "cb");
    long bad = 0; double maxabs = 0;
    for (int i = 0; i < ROWS; ++i) {
        if (a[i] != b[i]) bad++;
        double d = a[i] > b[i] ? a[i]-b[i] : b[i]-a[i];
        if (d > maxabs) maxabs = d;
    }
    printf("R8_P48_VS_K_FULL_OUTPUT_MISMATCH=%ld\n", bad);
    printf("BITWISE_KERNEL_OUTPUT_MATCH=%s\n", bad ? "NO" : "YES");
    printf("MAX_ABS_OUTPUT_ERROR=%.9g\n", maxabs);
    printf("R8_EXTRA_ROOT_B=%s\n", "ABSENT_A_VERIFIER_EN_SASS");
    cudaFree(d_w); cudaFree(d_y); cudaFree(d_o); cudaFree(d_ref);
    return 0;
}
