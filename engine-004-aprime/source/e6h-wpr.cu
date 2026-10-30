// ENGINE-006H — geometrie warps-per-output-row (WPR).
//
// Reproduit FIDELEMENT le mapping du noyau produit : qi = QI_G128 = 4, vdr = 1.
//   kqs = vdr * (tid % (qi/vdr))          -> tranche de 32 poids
//   kbx = tid / (qi/vdr)  puis pas de vdr*nwarps*warp_size/qi
// Verifie : couverture exacte des 192 paires (kbx, tranche), sans manquant ni doublon,
// pour nwarps = 1, 2, 4, 8.
//
// La mathematique par ligne est identique dans toutes les variantes : 48 blocs x 32 dp4a.

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

// NW = nombre de warps par ligne (la geometrie testee)
template <int NW>
__global__ void k_wpr(const uint8_t * __restrict__ w, const int * __restrict__ y,
                      float * __restrict__ out) {
    constexpr int QI = 4, VDR = 1, WS = 32;
    constexpr int PER_ITER = VDR * NW * WS / QI;      // pas de la boucle kbx
    const int row = blockIdx.x;
    const int tid = threadIdx.x;
    const int lane = tid % WS;
    const int wid  = tid / WS;
    const int kqs  = VDR * (tid % (QI / VDR));
    const uint8_t * rowbase = w + (size_t) row * NBLK * sizeof(block_tq2_g128);
    const block_q8_1 * yy = (const block_q8_1 *) y;

    float acc = 0.0f;
    for (int kbx = tid / (QI / VDR); kbx < NBLK; kbx += PER_ITER) {
        const block_tq2_g128 * bq = (const block_tq2_g128 *) (rowbase + (size_t) kbx * sizeof(block_tq2_g128));
        const uint8_t * qs = bq->qs + 8 * kqs;
        int sumi = 0;
#pragma unroll
        for (int t = 0; t < 4; ++t)
#pragma unroll
            for (int j = 0; j < 8; ++j) sumi = dp4a(decode_byte((int) qs[8*t + j]), yy[kbx].qs[t*8 + j], sumi);
        acc += __half2float(*(const __half *) &bq->d) * (float) sumi;
    }
    // reduction intra-warp (5 etages), puis combinaison inter-warp en memoire partagee
    for (int o = 16; o > 0; o >>= 1) acc += __shfl_down_sync(0xffffffffu, acc, o);
    __shared__ float partial[NW];
    if (lane == 0) partial[wid] = acc;
    __syncthreads();
    if (tid == 0) {
        float s = 0.0f;
        for (int i = 0; i < NW; ++i) s += partial[i];
        out[row] = s;
    }
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

static void report_occ(const char * nm, const void * fn, int threads) {
    cudaFuncAttributes a{};
    ck(cudaFuncGetAttributes(&a, fn), "func attrs");
    int blocks = 0;
    ck(cudaOccupancyMaxActiveBlocksPerMultiprocessor(&blocks, fn, threads, 0), "occupancy");
    const int warps = threads / 32;
    printf("%s REGISTERS=%d SHARED=%zu THREADS=%d BLOCKS_PER_SM=%d WARPS_PER_BLOCK=%d ACTIVE_WARPS_PER_SM=%d\n",
           nm, a.numRegs, a.sharedSizeBytes, threads, blocks, warps, blocks * warps);
}

int main() {
    const size_t W_BYTES = (size_t) ROWS * NBLK * sizeof(block_tq2_g128);
    printf("ENGINE006H_WPR_GEOMETRY\nROWS=%d BLOCKS_PER_ROW=%d QI=4 VDR=1\n", ROWS, NBLK);

    uint8_t * d_w = nullptr; int * d_y = nullptr; float * d_o = nullptr;
    ck(cudaMalloc(&d_w, W_BYTES), "malloc w");
    ck(cudaMalloc(&d_y, (size_t) NBLK * sizeof(block_q8_1)), "malloc y");
    ck(cudaMalloc(&d_o, (size_t) ROWS * sizeof(float)), "malloc o");
    std::vector<uint8_t> h(W_BYTES);
    for (size_t i = 0; i < W_BYTES; ++i) h[i] = (uint8_t) ((i * 2246822519u) >> 11);
    ck(cudaMemcpy(d_w, h.data(), W_BYTES, cudaMemcpyHostToDevice), "copy w");
    std::vector<int> hy(NBLK * 8);
    for (size_t i = 0; i < hy.size(); ++i) hy[i] = (int) ((i * 1103515245u + 12345u) >> 8) & 0x01010101;
    ck(cudaMemcpy(d_y, hy.data(), hy.size() * sizeof(int), cudaMemcpyHostToDevice), "copy y");

    printf("\n--- occupation reelle (API CUDA) ---\n");
    report_occ("WPR1", (const void *) k_wpr<1>, 32);
    report_occ("WPR2", (const void *) k_wpr<2>, 64);
    report_occ("WPR4", (const void *) k_wpr<4>, 128);
    report_occ("WPR8", (const void *) k_wpr<8>, 256);

    const int REPS = 20;
    printf("\n--- fidelite : sorties bit a bit identiques ? ---\n");
    std::vector<float> ref(ROWS), cur(ROWS);
    k_wpr<4><<<ROWS, 128>>>(d_w, d_y, d_o); ck(cudaDeviceSynchronize(), "ref");
    ck(cudaMemcpy(ref.data(), d_o, ROWS * sizeof(float), cudaMemcpyDeviceToHost), "copy ref");
    for (int nw : {1, 2, 8}) {
        if (nw == 1) k_wpr<1><<<ROWS,  32>>>(d_w, d_y, d_o);
        if (nw == 2) k_wpr<2><<<ROWS,  64>>>(d_w, d_y, d_o);
        if (nw == 8) k_wpr<8><<<ROWS, 256>>>(d_w, d_y, d_o);
        ck(cudaDeviceSynchronize(), "alt");
        ck(cudaMemcpy(cur.data(), d_o, ROWS * sizeof(float), cudaMemcpyDeviceToHost), "copy cur");
        long bad = 0; for (int i = 0; i < ROWS; ++i) if (cur[i] != ref[i]) bad++;
        printf("WPR%d_VS_WPR4_OUTPUT_MISMATCH=%ld\n", nw, bad);
    }

    printf("\n--- microbench (20 invocations par mesure) ---\n");
    const double w1 = timeit([&]{ k_wpr<1><<<ROWS,  32>>>(d_w, d_y, d_o); }, REPS);
    const double w2 = timeit([&]{ k_wpr<2><<<ROWS,  64>>>(d_w, d_y, d_o); }, REPS);
    const double w4 = timeit([&]{ k_wpr<4><<<ROWS, 128>>>(d_w, d_y, d_o); }, REPS);
    const double w8 = timeit([&]{ k_wpr<8><<<ROWS, 256>>>(d_w, d_y, d_o); }, REPS);
    printf("WPR1_FULL_US=%.4f\n", 1000.0*w1/REPS);
    printf("WPR2_FULL_US=%.4f\n", 1000.0*w2/REPS);
    printf("WPR4_FULL_US=%.4f\n", 1000.0*w4/REPS);
    printf("WPR8_FULL_US=%.4f\n", 1000.0*w8/REPS);
    printf("WPR2_DELTA_US=%+.4f\n", 1000.0*(w2-w4)/REPS);
    printf("WPR2_SPEEDUP_RATIO=%.4f\n", w2/w4);
    return 0;
}
