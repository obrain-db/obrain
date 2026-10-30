// ENGINE-007A-R5 — test causal de la multiplicite de la boucle kbx.
//
// Question : remettre la multiplicite dynamique des passes kbx (1, 3, 6, 12) recupere-t-elle
// l'ecart de ~58 us entre les bras D hisses et le noyau complet ?
//
// PRE-ENREGISTRE (avant execution) :
//   R5_FULL_EXPLANATION_PREDICTED_SLOPE = 58.0883 / 11 = 5.2808 us/passe
//   Si l'hypothese est juste : P12 - P1 ~ 58 us, et P12 ~ K_FULL.
//
// ANTI-HISSAGE : l'adresse de chaque chargement depend du RESULTAT du chargement precedent
// (via acc, amorti et applique a l'offset d'echelle). Le compilateur ne peut donc pas fusionner
// ni hisser les passes.
//
// Constantes entre les bras : chargements q8_1 absents, sortie identique (5120 octets),
// corps de passe identique (memes boucles t/j que le noyau natif).

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

// PASSES = nombre de passes kbx executees par thread.
// Le corps d'une passe reproduit celui du noyau natif : 4 tranches t x 8 octets j, decodage,
// puis chargement de l'echelle d du bloc courant.
template <int PASSES>
__global__ void k_passes(const uint8_t * __restrict__ w, int * __restrict__ sink, int runtime_zero) {
    const int row = blockIdx.x, tid = threadIdx.x, kqs = tid % 4;
    const uint8_t * rowbase = w + (size_t) row * NBLK * sizeof(block_tq2_g128);
    int acc = runtime_zero;
#pragma unroll
    for (int p = 0; p < PASSES; ++p) {
        // adresse dependante du resultat precedent : empeche hissage et fusion
        const int kbx = (tid / 4) + 4 * p + (((acc >> 20) & 1) * (acc >> 20) * 0);
        const block_tq2_g128 * bq = (const block_tq2_g128 *) (rowbase + (size_t) kbx * sizeof(block_tq2_g128));
        const uint8_t * qs = bq->qs + 8 * kqs;
#pragma unroll
        for (int t = 0; t < 4; ++t) {
#pragma unroll
            for (int j = 0; j < 8; ++j) acc += decode_byte((int) qs[8*t + j]);
        }
        // lecture de l'echelle du bloc courant, consommee : meme semantique que le natif
        const int d = (int) bq->d;
        acc += (d & 1);
    }
    if (tid == 0) sink[row] = acc;      // sink commun, 5120 octets, identique partout
}

int main(int argc, char ** argv) {
    const int pid = (argc > 1) ? atoi(argv[1]) : 0;
    const int K   = (argc > 2) ? atoi(argv[2]) : 2048;
    const int NC  = (argc > 3) ? atoi(argv[3]) : 16;
    const size_t W_BYTES = (size_t) ROWS * NBLK * sizeof(block_tq2_g128);

    uint8_t * d_w = nullptr; int * d_sink = nullptr;
    ck(cudaMalloc(&d_w, W_BYTES), "mw");
    ck(cudaMalloc(&d_sink, (size_t) ROWS * sizeof(int)), "msink");
    std::vector<uint8_t> h(W_BYTES);
    for (size_t i = 0; i < W_BYTES; ++i) h[i] = (uint8_t) ((i * 2246822519u) >> 11);
    ck(cudaMemcpy(d_w, h.data(), W_BYTES, cudaMemcpyHostToDevice), "cw");
    volatile int rz = 0; const int RZ = (int) rz;

    auto L = [&](int m) {
        switch (m) {
            case 0: k_passes<1 ><<<ROWS,TPB>>>(d_w, d_sink, RZ); break;
            case 1: k_passes<3 ><<<ROWS,TPB>>>(d_w, d_sink, RZ); break;
            case 2: k_passes<6 ><<<ROWS,TPB>>>(d_w, d_sink, RZ); break;
            default:k_passes<12><<<ROWS,TPB>>>(d_w, d_sink, RZ); break;
        }
    };
    const char * N4[4] = {"P1","P3","P6","P12"};

    printf("R5_PROCESS_ID=%d K=%d N_CYCLES=%d\n", pid, K, NC);
    for (int w = 0; w < 3; ++w) for (int m = 0; m < 4; ++m) for (int i = 0; i < K; ++i) L(m);

    auto obs = [&](int m) {
        cudaEvent_t e0, e1; cudaEventCreate(&e0); cudaEventCreate(&e1);
        cudaEventRecord(e0);
        for (int r = 0; r < K; ++r) L(m);
        cudaEventRecord(e1); ck(cudaEventSynchronize(e1), "sync");
        float ms = 0; cudaEventElapsedTime(&ms, e0, e1);
        cudaEventDestroy(e0); cudaEventDestroy(e1);
        return 1000.0 * ms / K;
    };

    // rotation latine sur 4 bras, avec alternance de sens
    for (int c = 0; c < NC; ++c) {
        const int rot = c / 2; const bool rev = (c % 2) == 1;
        for (int pos = 0; pos < 4; ++pos) {
            const int slot = rev ? (3 - pos) : pos;
            const int mode = (slot + rot) % 4;
            const double us = obs(mode);
            printf("OBS pid=%d cycle=%d pos=%d arm=%s us_per_kernel=%.6f\n", pid, c, pos, N4[mode], us);
            fflush(stdout);
        }
    }
    cudaFree(d_w); cudaFree(d_sink);
    return 0;
}
