// ENGINE-007A-R4 — replay canonique + decomposition factorielle du plancher.
//
// (1) Replay canonique de la famille 006F : K_STREAM, K_DECODE, K_FULL, repris a l'identique.
// (2) Factoriel 2x2 depuis UN MEME CORPS DECODE :
//       A = chargement des activations q8_1   (OFF / ON)
//       W = volume d'ecriture de sortie       (SMALL 20 Ko / LARGE 2,50 Mo)
//     D00=Aoff,Wsmall  D10=Aon,Wsmall  D01=Aoff,Wlarge  D11=Aon,Wlarge
//
// Corps decode invariant dans les quatre bras : memes chargements de poids, meme decodage
// ternaire, meme geometrie, meme boucle.
//
// Protocole canonique : K=2048, warmup 3 batches, une paire d'evenements par batch,
// rotation latine, processus neuf par campagne.

#include <cstdio>
#include <cstdlib>
#include <cstdint>
#include <cstring>
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

// ---------------- replay 006F, repris a l identique ----------------
__global__ void k_stream(const uint8_t * __restrict__ w, int * __restrict__ sink) {
    const int row = blockIdx.x, tid = threadIdx.x, kqs = tid % 4;
    const uint8_t * rowbase = w + (size_t) row * NBLK * sizeof(block_tq2_g128);
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
    __shared__ float partial[4];
    const int lane = tid % 32, wid = tid / 32;
    if (lane == 0) partial[wid] = acc;
    __syncthreads();
    if (tid == 0) { float s = 0; for (int i = 0; i < 4; ++i) s += partial[i]; out[row] = s; }
}

// ---------------- factoriel : corps decode commun ----------------
// ACT : 1 = charger les 32 activations q8_1 et les garder vivantes ; 0 = ne pas les charger.
// WIDE: 1 = ecrire 1 int32 par thread (5120x128 = 2,50 Mo, semantique exacte de k_decode) ;
//       0 = ecrire 1 int32 par ligne (5120 = 20 Ko).
template <int ACT, int WIDE>
__global__ void k_fact(const uint8_t * __restrict__ w, const int * __restrict__ y,
                       int * __restrict__ sink, int runtime_zero) {
    const int row = blockIdx.x, tid = threadIdx.x, kqs = tid % 4;
    const uint8_t * rowbase = w + (size_t) row * NBLK * sizeof(block_tq2_g128);
    const block_q8_1 * yy = (const block_q8_1 *) y;
    int acc = runtime_zero, keep = runtime_zero;
    for (int kbx = tid / 4; kbx < NBLK; kbx += 32) {
        const uint8_t * qs = rowbase + (size_t) kbx * sizeof(block_tq2_g128) + 8 * kqs;
#pragma unroll
        for (int j = 0; j < 8; ++j) {
            const int dec = decode_byte((int) qs[j]);
            acc += dec;
            if (ACT) {
                const int a = (int) yy[kbx].qs[tid % 4 * 8 + j];   // meme adresse que le chemin produit
                keep += a;
            }
        }
    }
    if (ACT) acc += keep;                       // garde le chargement d activation vivant
    // Stockage par PTX inline : le compilateur ne peut ni replier ni supprimer l ecriture.
    // Sans cela, il reduz les stores par thread a un store conditionnel unique et les bras
    // SMALL/LARGE deviennent indiscernables dans le SASS (constate au premier jet).
    if (WIDE) {
        asm volatile("st.global.cg.s32 [%0], %1;" :: "l"(sink + (size_t) row * TPB + tid), "r"(acc) : "memory");
    } else if (tid == 0) {
        asm volatile("st.global.cg.s32 [%0], %1;" :: "l"(sink + row), "r"(acc) : "memory");
    }
}

int main(int argc, char ** argv) {
    const int  pid = (argc > 1) ? atoi(argv[1]) : 0;
    const int  K   = (argc > 2) ? atoi(argv[2]) : 2048;
    const int  NC  = (argc > 3) ? atoi(argv[3]) : 14;
    const size_t W_BYTES = (size_t) ROWS * NBLK * sizeof(block_tq2_g128);

    uint8_t * d_w = nullptr; int * d_y = nullptr; int * d_sink = nullptr; float * d_o = nullptr;
    ck(cudaMalloc(&d_w, W_BYTES), "mw");
    ck(cudaMalloc(&d_y, (size_t) NBLK * sizeof(block_q8_1)), "my");
    ck(cudaMalloc(&d_sink, (size_t) ROWS * TPB * sizeof(int)), "msink");
    ck(cudaMalloc(&d_o, (size_t) ROWS * sizeof(float)), "mo");
    std::vector<uint8_t> h(W_BYTES);
    for (size_t i = 0; i < W_BYTES; ++i) h[i] = (uint8_t) ((i * 2246822519u) >> 11);
    ck(cudaMemcpy(d_w, h.data(), W_BYTES, cudaMemcpyHostToDevice), "cw");
    std::vector<int> hy(NBLK * 8);
    for (size_t i = 0; i < hy.size(); ++i) hy[i] = (int) ((i * 1103515245u + 12345u) >> 8) & 0x01010101;
    ck(cudaMemcpy(d_y, hy.data(), hy.size()*sizeof(int), cudaMemcpyHostToDevice), "cy");
    volatile int rz = 0; const int RZ = (int) rz;

    const char * nm[6] = {"K_STREAM","K_DECODE","K_FULL","D00","D10","D01"};
    auto L = [&](int m) {
        switch (m) {
            case 0: k_stream<<<ROWS,TPB>>>(d_w, d_sink); break;
            case 1: k_decode<<<ROWS,TPB>>>(d_w, d_sink); break;
            case 2: k_full  <<<ROWS,TPB>>>(d_w, d_y, d_o); break;
            case 3: k_fact<0,0><<<ROWS,TPB>>>(d_w, d_y, d_sink, RZ); break;
            case 4: k_fact<1,0><<<ROWS,TPB>>>(d_w, d_y, d_sink, RZ); break;
            default:k_fact<0,1><<<ROWS,TPB>>>(d_w, d_y, d_sink, RZ); break;
        }
    };
    // D11 traite a part (6e bras)
    auto L11 = [&]() { k_fact<1,1><<<ROWS,TPB>>>(d_w, d_y, d_sink, RZ); };
    const char * nmD11 = "D11";

    printf("R4_PROCESS_ID=%d K=%d N_CYCLES=%d GLOBAL_WARMUP_BATCHES=3\n", pid, K, NC);
    // warmup global, jamais compte
    for (int w = 0; w < 3; ++w) { for (int m = 0; m < 6; ++m) L(m); L11(); for (int i = 0; i < K; ++i) L(2); }

    auto obs = [&](int m) {
        cudaEvent_t e0, e1; cudaEventCreate(&e0); cudaEventCreate(&e1);
        cudaEventRecord(e0);
        for (int r = 0; r < K; ++r) { if (m == 6) L11(); else L(m); }
        cudaEventRecord(e1); ck(cudaEventSynchronize(e1), "sync");
        float ms = 0; cudaEventElapsedTime(&ms, e0, e1);
        cudaEventDestroy(e0); cudaEventDestroy(e1);
        return 1000.0 * ms / K;
    };

    const char * N7[7] = {"K_STREAM","K_DECODE","K_FULL","D00","D10","D01",(const char*)nmD11};
    for (int c = 0; c < NC; ++c) {
        const int rot = c / 2; const bool rev = (c % 2) == 1;
        for (int pos = 0; pos < 7; ++pos) {
            const int slot = rev ? (6 - pos) : pos;
            const int mode = (slot + rot) % 7;
            const double us = obs(mode);
            printf("OBS pid=%d cycle=%d pos=%d arm=%s us_per_kernel=%.6f\n", pid, c, pos, N7[mode], us);
            fflush(stdout);
        }
    }
    cudaFree(d_w); cudaFree(d_y); cudaFree(d_sink); cudaFree(d_o);
    return 0;
}
