// ENGINE-007A-R3 — reconciliation des niveaux absolus entre instruments.
//
// Matrice 2x2 : deux NOYAUX (006F k_full sans keepalive / R2 A100 avec keepalive explicite)
// x deux PROTOCOLES de chronometrage (batch court type 006F / batch long type R2).
// Separe KERNEL_EFFECT, HARNESS_EFFECT et leur interaction.
//
// Balayage de longueur de batch et de warmup inclus.

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

static void ck(cudaError_t e, const char * w) {
    if (e != cudaSuccess) { fprintf(stderr, "CUDA_FAIL %s : %s\n", w, cudaGetErrorString(e)); exit(2); }
}
__device__ __forceinline__ int decode_byte(int b) {
    const int c0 = (b >> 0) & 3, c1 = (b >> 2) & 3, c2 = (b >> 4) & 3, c3 = (b >> 6) & 3;
    return ((c0 > 2 ? 1 : c0 - 1) & 0xFF) | (((c1 > 2 ? 1 : c1 - 1) & 0xFF) << 8) |
           (((c2 > 2 ? 1 : c2 - 1) & 0xFF) << 16) | (((c3 > 2 ? 1 : c3 - 1) & 0xFF) << 24);
}
__device__ __forceinline__ int dp4a(int a, int b, int c) { return __dp4a(a, b, c); }

// ---------------------------------------------------------------------------
// KERNEL A = version 006F (sans keepalive) : l'activation est lue dans l'appel
// vec_dot, sans etre conservee hors de l'IDP.
// ---------------------------------------------------------------------------
__global__ void k_006f(const uint8_t * __restrict__ w, const int * __restrict__ y,
                       float * __restrict__ out) {
    const int tid = threadIdx.x, lane = tid % 32, wid = tid / 32, kqs = tid % 4;
    const uint8_t * rowbase = w + (size_t) blockIdx.x * NBLK * sizeof(block_tq2_g128);
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
    if (tid == 0) { float s = 0; for (int i = 0; i < 4; ++i) s += partial[i]; out[blockIdx.x] = s; }
}

// ---------------------------------------------------------------------------
// KERNEL B = version 007A-R2 A100 (avec keepalive : les DEUX operandes sont
// charges explicitement et conserves).
// ---------------------------------------------------------------------------
__global__ void k_r2(const uint8_t * __restrict__ w, const int * __restrict__ y,
                     float * __restrict__ out, int runtime_zero, int * __restrict__ keep_sink) {
    const int tid = threadIdx.x, lane = tid % 32, wid = tid / 32, kqs = tid % 4;
    const uint8_t * rowbase = w + (size_t) blockIdx.x * NBLK * sizeof(block_tq2_g128);
    const block_q8_1 * yy = (const block_q8_1 *) y;
    int acc = runtime_zero, keep = runtime_zero;
    for (int kbx = tid / 4; kbx < NBLK; kbx += 32) {
        const block_tq2_g128 * bq = (const block_tq2_g128 *) (rowbase + (size_t) kbx * sizeof(block_tq2_g128));
        const uint8_t * qs = bq->qs + 8 * kqs;
#pragma unroll
        for (int t = 0; t < 4; ++t)
#pragma unroll
            for (int j = 0; j < 8; ++j) {
                const int dec = decode_byte((int) qs[8*t + j]);
                const int act = (int) yy[kbx].qs[t*8 + j];
                keep ^= dec; keep += act;
                acc = dp4a(dec, act, acc);
            }
    }
    float f = (float) acc + (float) (keep & 1);
    for (int o = 16; o > 0; o >>= 1) f += __shfl_down_sync(0xffffffffu, f, o);
    __shared__ float partial[4];
    if (lane == 0) partial[wid] = f;
    __syncthreads();
    if (tid == 0) { float s = 0; for (int i = 0; i < 4; ++i) s += partial[i]; out[blockIdx.x] = s; }
    if (keep == 0x7fffffff) keep_sink[0] = keep;
}

// protocole de chronometrage parametrable
static double measure(void (*dummy)(), int which, int K, int warmup, const uint8_t * w,
                      const int * y, float * o, int rz, int * sink) {
    for (int i = 0; i < warmup; ++i) {
        if (which == 0) k_006f<<<ROWS,128>>>(w, y, o);
        else            k_r2  <<<ROWS,128>>>(w, y, o, rz, sink);
    }
    ck(cudaDeviceSynchronize(), "warm");
    cudaEvent_t e0, e1; cudaEventCreate(&e0); cudaEventCreate(&e1);
    cudaEventRecord(e0);
    for (int i = 0; i < K; ++i) {
        if (which == 0) k_006f<<<ROWS,128>>>(w, y, o);
        else            k_r2  <<<ROWS,128>>>(w, y, o, rz, sink);
    }
    cudaEventRecord(e1); ck(cudaEventSynchronize(e1), "sync");
    float ms = 0; cudaEventElapsedTime(&ms, e0, e1);
    cudaEventDestroy(e0); cudaEventDestroy(e1);
    return 1000.0 * ms / K;
}

int main() {
    const size_t W_BYTES = (size_t) ROWS * NBLK * sizeof(block_tq2_g128);
    uint8_t * d_w = nullptr; int * d_y = nullptr; float * d_o = nullptr; int * d_k = nullptr;
    ck(cudaMalloc(&d_w, W_BYTES), "mw");
    ck(cudaMalloc(&d_y, (size_t) NBLK * sizeof(block_q8_1)), "my");
    ck(cudaMalloc(&d_o, (size_t) ROWS * sizeof(float)), "mo");
    ck(cudaMalloc(&d_k, sizeof(int)), "mk");
    std::vector<uint8_t> h(W_BYTES);
    for (size_t i = 0; i < W_BYTES; ++i) h[i] = (uint8_t) ((i * 2246822519u) >> 11);
    ck(cudaMemcpy(d_w, h.data(), W_BYTES, cudaMemcpyHostToDevice), "cw");
    std::vector<int> hy(NBLK * 8);
    for (size_t i = 0; i < hy.size(); ++i) hy[i] = (int) ((i * 1103515245u + 12345u) >> 8) & 0x01010101;
    ck(cudaMemcpy(d_y, hy.data(), hy.size()*sizeof(int), cudaMemcpyHostToDevice), "cy");
    volatile int rz = 0; const int RZ = (int) rz;

    printf("ENGINE007A_R3_ABSOLUTE_LEVEL_RECONCILIATION\n");
    printf("KERNEL_A=006F_k_full_sans_keepalive   KERNEL_B=007A_A100_avec_keepalive\n\n");

    // --- matrice 2x2 : chaque noyau sous les deux protocoles ---
    printf("=== MATRICE 2x2 (2048 kernels/mesure, warmup 3) ===\n");
    for (int which = 0; which < 2; ++which) {
        const char * kn = which ? "K_B_A100_keepalive" : "K_A_006F_no_keepalive";
        for (int proto = 0; proto < 2; ++proto) {
            const int K = proto ? 2048 : 20;          // protocole 006F : lot de 20 ; protocole R2 : 2048
            const int W = proto ? 3 : 1;
            double best = 1e9, sum = 0;
            for (int rep = 0; rep < 5; ++rep) {
                const double v = measure(nullptr, which, K, W, d_w, d_y, d_o, RZ, d_k);
                if (v < best) best = v; sum += v;
            }
            printf("  %-24s proto=%-8s K=%-5d warmup=%d  mean=%.4f  min=%.4f  us/kernel\n",
                   kn, proto ? "R2_long" : "006F_short", K, W, sum/5, best);
        }
    }

    // --- balayage de longueur de batch, sur les deux noyaux ---
    printf("\n=== BALAYAGE DE LONGUEUR DE BATCH (warmup 3, une mesure) ===\n");
    const int Ks[7] = {1, 4, 20, 64, 256, 2048, 8192};
    for (int which = 0; which < 2; ++which) {
        printf("  noyau %s :\n", which ? "B_A100" : "A_006F");
        for (int i = 0; i < 7; ++i) {
            const double v = measure(nullptr, which, Ks[i], 3, d_w, d_y, d_o, RZ, d_k);
            printf("    K=%-5d  us_per_kernel=%.4f\n", Ks[i], v);
        }
    }

    // --- balayage de warmup ---
    printf("\n=== BALAYAGE DE WARMUP (K=2048) ===\n");
    const int Ws[5] = {0, 1, 10, 100, 1000};
    for (int which = 0; which < 2; ++which) {
        printf("  noyau %s :\n", which ? "B_A100" : "A_006F");
        for (int i = 0; i < 5; ++i) {
            const double v = measure(nullptr, which, 2048, Ws[i], d_w, d_y, d_o, RZ, d_k);
            printf("    warmup=%-5d  us_per_kernel=%.4f\n", Ws[i], v);
        }
    }
    cudaFree(d_w); cudaFree(d_y); cudaFree(d_o); cudaFree(d_k);
    return 0;
}
