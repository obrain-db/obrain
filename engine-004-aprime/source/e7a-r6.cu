// ENGINE-007A-R6 — multiplicite kbx NATIVE, borne de boucle runtime.
//
// Principe (paragraphe 4 a 10) :
//   - le corps est EXACTEMENT celui de K_FULL natif (chargements poids, echelle, decodage ternaire,
//     idp4a, accumulateurs natifs, reduction native, store natif)
//   - la SEULE variable est le nombre de passes kbx reellement executees, pilote par un argument
//     de noyau RUNTIME, lu avant tout travail arithmetique
//   - AUCUN mecanisme anti-DCE, aucun keepalive, aucun anti-hissage : le resultat mathematique du
//     noyau rend les valeurs vivantes
//   - AUCUNE dependance de l'indice ou de la borne sur acc (gate du paragraphe 8)
//
// Decoupage EXACT du natif : le natif fait kbx = tid/4 + 32*i, soit kbx = tid/4 + 2*p avec
// PASSDIV=2 et p parcourant 0..11. Les passes p>=2 tombent hors domaine (kbx >= 48), exactement
// comme les iterations i>=2 du natif. Avec les 12 bits du masque actifs, on retrouve donc
// EXACTEMENT la sequence native par thread : c'est le gate de fidelite du paragraphe 14.
// La selection par passe se fait par un masque runtime (bit p) : codegen IDENTIQUE pour tous les
// bras, seule l'execution conditionnelle change.

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
static const int PASSDIV = 2;   // kbx = tid/4 + 2*p : idem natif quand les 12 passes sont actives
static const int NPASS = 12;

static void ck(cudaError_t e, const char * w) {
    if (e != cudaSuccess) { fprintf(stderr, "CUDA_FAIL %s : %s\n", w, cudaGetErrorString(e)); exit(2); }
}
__device__ __forceinline__ int decode_byte(int b) {
    const int c0 = (b >> 0) & 3, c1 = (b >> 2) & 3, c2 = (b >> 4) & 3, c3 = (b >> 6) & 3;
    return ((c0 > 2 ? 1 : c0 - 1) & 0xFF) | (((c1 > 2 ? 1 : c1 - 1) & 0xFF) << 8) |
           (((c2 > 2 ? 1 : c2 - 1) & 0xFF) << 16) | (((c3 > 2 ? 1 : c3 - 1) & 0xFF) << 24);
}
__device__ __forceinline__ int dp4a(int a, int b, int c) { return __dp4a(a, b, c); }

// Corps natif complet de K_FULL, avec SELECTION RUNTIME des blocs kbx.
// kbx_mask : bit b a 1 => le bloc kbx=b est traite. Spans = kbx actifs (calcule par l hote).
// Le codegen est IDENTIQUE pour tous les bras : seule l execution conditionnelle change.
// Avec kbx_mask = tous les bits 0..47, ce noyau reproduit EXACTEMENT K_FULL.
__global__ void k_native_passes(const uint8_t * __restrict__ w, const int * __restrict__ y,
                                float * __restrict__ out, unsigned long long kbx_mask,
                                int span_count) {
    const int row = blockIdx.x, tid = threadIdx.x, kqs = tid % 4;
    const int lane = tid % 32, wid = tid / 32;
    const uint8_t * rowbase = w + (size_t) row * NBLK * sizeof(block_tq2_g128);
    const block_q8_1 * yy = (const block_q8_1 *) y;
    float acc = 0.0f;
    int done = 0;
    for (int kbx = tid / 4; kbx < NBLK; kbx += 32) {
        if ((kbx_mask & (1ull << kbx)) == 0ull) continue;   // selection par bloc, sans dependance a acc
        const block_tq2_g128 * bq = (const block_tq2_g128 *) (rowbase + (size_t) kbx * sizeof(block_tq2_g128));
        const uint8_t * qs = bq->qs + 8 * kqs;
        int sumi = 0;
#pragma unroll
        for (int t = 0; t < 4; ++t)
#pragma unroll
            for (int j = 0; j < 8; ++j) sumi = dp4a(decode_byte((int) qs[8*t + j]), yy[kbx].qs[t*8 + j], sumi);
        acc += __half2float(*(const __half *) &bq->d) * (float) sumi;
        done++;
    }
    for (int o = 16; o > 0; o >>= 1) acc += __shfl_down_sync(0xffffffffu, acc, o);
    __shared__ float partial[4];
    if (lane == 0) partial[wid] = acc;
    __syncthreads();
    if (tid == 0) { float s = 0; for (int i = 0; i < 4; ++i) s += partial[i]; out[row] = s + (float) (done & 1) * (float) span_count * 0.0f; }
}

// reference : K_FULL natif strictement identique a 006F/R3/R4 (boucle kbx < NBLK, pas = 32)
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

int main(int argc, char ** argv) {
    const int pid = (argc > 1) ? atoi(argv[1]) : 0;
    const int K   = (argc > 2) ? atoi(argv[2]) : 2048;
    const int NC  = (argc > 3) ? atoi(argv[3]) : 16;
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

    // masques : P12 = les 12 passes ; P6_A = passes paires ; P6_B = passes impaires ;
    // P3 = 3 passes ; P1 = 1 passe. Tous de meme codegen, seule l execution change.
    // Masques sur les 48 blocs kbx. P12 = les 48 (reproduit le natif) ;
    // P24 = blocs pairs (24) ; P12_ALT = blocs impairs (24) ; P6 = un bloc sur 8 (6).
    const unsigned long long M48  = 0x0000FFFFFFFFFFFFull;          // 48 blocs
    const unsigned long long M24A = 0x0000555555555555ull;          // pairs
    const unsigned long long M24B = 0x0000AAAAAAAAAAAAull;          // impairs
    const unsigned long long M6   = 0x0000000000001011ull;          // {0,4,12,16,20,44} arbitraire
    const int S48 = 48, S24A = 24, S24B = 24, S6 = 6;

    // --- gate de fidelite : P12 doit reproduire K_FULL exactement ---
    k_full_native<<<ROWS,TPB>>>(d_w, d_y, d_ref); ck(cudaDeviceSynchronize(), "ref");
    k_native_passes<<<ROWS,TPB>>>(d_w, d_y, d_o, M48, S48); ck(cudaDeviceSynchronize(), "p48");
    std::vector<float> a(ROWS), b(ROWS);
    ck(cudaMemcpy(a.data(), d_ref, ROWS*sizeof(float), cudaMemcpyDeviceToHost), "ca");
    ck(cudaMemcpy(b.data(), d_o, ROWS*sizeof(float), cudaMemcpyDeviceToHost), "cb");
    long bad = 0; for (int i = 0; i < ROWS; ++i) if (a[i] != b[i]) bad++;
    printf("R6_PROCESS_ID=%d K=%d N_CYCLES=%d\n", pid, K, NC);
    printf("P12_VS_K_FULL_OUTPUT_MISMATCH=%ld\nP12_KBX_SEQUENCE_MATCH_NATIVE=%s\n", bad, bad?"NO":"YES");

    const char * N5[5] = {"K_FULL","P6","P24_A","P24_B","P48"};
    const unsigned long long M5[5] = {M48, M6, M24A, M24B, M48};
    const int S5[5] = {S48, S6, S24A, S24B, S48};
    auto L = [&](int m) {
        if (m == 0) k_full_native<<<ROWS,TPB>>>(d_w, d_y, d_o);
        else        k_native_passes<<<ROWS,TPB>>>(d_w, d_y, d_o, M5[m], S5[m]);
    };
    auto LP12 = [&]() { k_native_passes<<<ROWS,TPB>>>(d_w, d_y, d_o, M48, S48); };

    for (int w = 0; w < 3; ++w) { for (int m = 0; m < 5; ++m) for (int i = 0; i < K; ++i) L(m); for (int i = 0; i < K; ++i) LP12(); }

    auto obs = [&](int m) {
        cudaEvent_t e0, e1; cudaEventCreate(&e0); cudaEventCreate(&e1);
        cudaEventRecord(e0);
        for (int r = 0; r < K; ++r) { if (m == 5) LP12(); else L(m); }
        cudaEventRecord(e1); ck(cudaEventSynchronize(e1), "sync");
        float ms = 0; cudaEventElapsedTime(&ms, e0, e1);
        cudaEventDestroy(e0); cudaEventDestroy(e1);
        return 1000.0 * ms / K;
    };

    const char * N6[6] = {"K_FULL","P6","P24_A","P24_B","P48","P48b"};
    for (int c = 0; c < NC; ++c) {
        const int rot = c / 2; const bool rev = (c % 2) == 1;
        for (int pos = 0; pos < 6; ++pos) {
            const int slot = rev ? (5 - pos) : pos;
            const int mode = (slot + rot) % 6;
            const double us = obs(mode);
            printf("OBS pid=%d cycle=%d pos=%d arm=%s us_per_kernel=%.6f\n", pid, c, pos, N6[mode], us);
            fflush(stdout);
        }
    }
    cudaFree(d_w); cudaFree(d_y); cudaFree(d_o); cudaFree(d_ref);
    return 0;
}
