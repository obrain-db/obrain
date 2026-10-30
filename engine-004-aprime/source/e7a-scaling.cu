// ENGINE-007A — mise a l'echelle du travail arithmetique utile (IDP.4A).
//
// Famille de microkernels IDENTIQUES en structure, ne differant que par le NOMBRE de sites
// dp4a actifs sur les 32 du chemin tq2_g128 x q8_1, N=1.
//
// KEEPALIVE : les operandes decodes de TOUS les sites sont consommes par une somme XOR qui
// alimente la sortie. Un site dp4a desactive ne supprime donc NI le chargement de poids,
// NI le chargement q8_1, NI le decodage ternaire, NI la generation d'adresse.
//
// ACCUMULATEURS : initialises depuis un zero RUNTIME (argument de noyau) pour empecher le
// repliement de constante quand des groupes entiers sont desactives.
//
// Chaque site est conditionne par un parametre de TEMPLATE (constexpr) : la boucle interne
// est entierement deroulee, donc la selection est resolue a la compilation. Aucun
// branchement runtime par site, aucun commutateur runtime.

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

// MASQUE : 32 sites (4 tranches t x 8 positions j). Un site actif fait le dp4a ;
// tous les sites alimentent le keepalive.
//   A0       : aucun
//   A25_A    : j paire par tranche (4 sites x ... ) -> 8 sites
//   A25_B    : j impaire par tranche
//   A50_EVEN : tranches 0,2 entierement actives -> 16
//   A50_ODD  : tranches 1,3 entierement actives -> 16
//   A75      : toutes sauf la tranche 3 -> 24
//   A100     : les 32
__device__ __forceinline__ bool site_active(int t, int j, int mode) {
    switch (mode) {
        case 0: return false;                                  // A0
        case 1: return (j % 2) == 0;                           // A25_A : 4x2 = 8
        case 2: return (j % 2) == 1;                           // A25_B : 4x2 = 8
        case 3: return (t % 2) == 0;                           // A50_EVEN : 2x8 = 16
        case 4: return (t % 2) == 1;                           // A50_ODD  : 2x8 = 16
        case 5: return t < 3;                                  // A75 : 3x8 = 24
        default: return true;                                  // A100
    }
}

template <int MODE>
__global__ void k_scale(const uint8_t * __restrict__ w, const int * __restrict__ y,
                        float * __restrict__ out, int runtime_zero, int * __restrict__ keep_sink) {
    constexpr int QI = 4, VDR = 1, WS = 32, NW = 4;
    constexpr int PER_ITER = VDR * NW * WS / QI;
    const int tid = threadIdx.x, lane = tid % WS, wid = tid / WS;
    const int kqs = VDR * (tid % (QI / VDR));
    const uint8_t * rowbase = w + (size_t) blockIdx.x * NBLK * sizeof(block_tq2_g128);
    const block_q8_1 * yy = (const block_q8_1 *) y;

    int acc = runtime_zero;           // zero NON connu du compilateur
    int keep = runtime_zero;          // keepalive : consomme TOUS les operandes decodes
    for (int kbx = tid / (QI / VDR); kbx < NBLK; kbx += PER_ITER) {
        const block_tq2_g128 * bq = (const block_tq2_g128 *) (rowbase + (size_t) kbx * sizeof(block_tq2_g128));
        const uint8_t * qs = bq->qs + 8 * kqs;
#pragma unroll
        for (int t = 0; t < 4; ++t) {
#pragma unroll
            for (int j = 0; j < 8; ++j) {
                const int dec = decode_byte((int) qs[8*t + j]);
                // L'activation est chargee EXPLICITEMENT dans tous les cas. Sans cela, le
                // compilateur replie le chargement q8_1 dans l'operande memoire de l'IDP et
                // les activations disparaissent des qu'on desactive un site (constate sur M0 :
                // 0 LDG.E.S8). Cette charge explicite alimente le keepalive, donc elle survit.
                const int act = (int) yy[kbx].qs[t*8 + j];
                keep ^= dec;                                  // garde le decodage vivant
                keep += act;                                  // garde les activations vivantes
                if (site_active(t, j, MODE)) {
                    acc = dp4a(dec, act, acc);
                }
            }
        }
    }
    // reduction : structure conservee dans toutes les variantes
    float f = (float) acc + (float) (keep & 1);               // le keepalive touche la sortie
    for (int o = 16; o > 0; o >>= 1) f += __shfl_down_sync(0xffffffffu, f, o);
    __shared__ float partial[NW];
    if (lane == 0) partial[wid] = f;
    __syncthreads();
    if (tid == 0) {
        float s = 0.0f;
        for (int i = 0; i < NW; ++i) s += partial[i];
        out[(size_t) blockIdx.x] = s;
    }
    if (keep == 0x7fffffff) keep_sink[0] = keep;              // sink supplementaire, jamais pris
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


// ---------------------------------------------------------------------------
// R2 : campagne haute puissance.
//   - une observation = un batch long de K invocations, UNE paire d'evenements
//   - rotation latine : chaque bras apparait une fois a chaque position
//   - interleaving court, ordres alterne (rotation, rotation inversee)
//   - un processus par execution, contexte CUDA neuf
// ---------------------------------------------------------------------------
int main(int argc, char ** argv) {
    const int pid = (argc > 1) ? atoi(argv[1]) : 0;
    const int K   = (argc > 2) ? atoi(argv[2]) : 4096;
    const int NCYC= (argc > 3) ? atoi(argv[3]) : 14;   // 7 rotations x 2 sens
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
    auto L = [&](int mode) {
        switch (mode) {
            case 0: k_scale<0><<<ROWS,128>>>(d_w,d_y,d_o,RZ,d_k); break;
            case 1: k_scale<1><<<ROWS,128>>>(d_w,d_y,d_o,RZ,d_k); break;
            case 2: k_scale<2><<<ROWS,128>>>(d_w,d_y,d_o,RZ,d_k); break;
            case 3: k_scale<3><<<ROWS,128>>>(d_w,d_y,d_o,RZ,d_k); break;
            case 4: k_scale<4><<<ROWS,128>>>(d_w,d_y,d_o,RZ,d_k); break;
            case 5: k_scale<5><<<ROWS,128>>>(d_w,d_y,d_o,RZ,d_k); break;
            default: k_scale<6><<<ROWS,128>>>(d_w,d_y,d_o,RZ,d_k); break;
        }
    };
    const char * nm[7] = {"A0","A25_A","A25_B","A50_EVEN","A50_ODD","A75","A100"};

    // --- warmup global : 3 batches sur chaque bras, jamais comptes ---
    for (int w = 0; w < 3; ++w)
        for (int m = 0; m < 7; ++m) { for (int r = 0; r < K; ++r) L(m); }
    ck(cudaDeviceSynchronize(), "warmup");

    auto observe = [&](int mode) {
        cudaEvent_t e0, e1; cudaEventCreate(&e0); cudaEventCreate(&e1);
        cudaEventRecord(e0);
        for (int r = 0; r < K; ++r) L(mode);
        cudaEventRecord(e1);
        ck(cudaEventSynchronize(e1), "obs sync");
        float ms = 0; cudaEventElapsedTime(&ms, e0, e1);
        cudaEventDestroy(e0); cudaEventDestroy(e1);
        return 1000.0 * ms / K;                     // us par kernel
    };

    printf("R2_PROCESS_ID=%d KERNELS_PER_OBSERVATION=%d N_CYCLES=%d GLOBAL_WARMUP_BATCHES=3\n",
           pid, K, NCYC);
    for (int c = 0; c < NCYC; ++c) {
        const int rot = c / 2;
        const bool rev = (c % 2) == 1;
        for (int pos = 0; pos < 7; ++pos) {
            const int slot = rev ? (6 - pos) : pos;
            const int mode = (slot + rot) % 7;      // rotation latine
            const double us = observe(mode);
            printf("OBS pid=%d cycle=%d pos=%d slot=%d arm=%s us_per_kernel=%.6f\n",
                   pid, c, pos, slot, nm[mode], us);
            fflush(stdout);
        }
    }
    cudaFree(d_w); cudaFree(d_y); cudaFree(d_o); cudaFree(d_k);
    return 0;
}
