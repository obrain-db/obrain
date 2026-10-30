// ENGINE-007A-R9A — taxonomie des mismatches bit a bit.
//
// Aucun kernel n est modifie : on rejoue K_FULL et R8 (R8A, sans artefact) puis on classe
// CHAQUE divergence en comparant les uint32 bruts, sans passer par la comparaison flottante.
//
// Resout l incoherence : 1875 mismatches bitwise avec max_abs_error == 0 n est possible que
// pour des cas speciaux (+0/-0, NaN, Inf).

#include <cstdio>
#include <cstdlib>
#include <cstdint>
#include <cstring>
#include <cmath>
#include <vector>
#include <algorithm>
#include <numeric>
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
    return ((c0 > 0 ? 0 : 0) | ((c0 > 2 ? 1 : c0 - 1) & 0xFF)) |
           ((((c1 > 2 ? 1 : c1 - 1) & 0xFF)) << 8) |
           ((((c2 > 2 ? 1 : c2 - 1) & 0xFF)) << 16) |
           ((((c3 > 2 ? 1 : c3 - 1) & 0xFF)) << 24);
}
__device__ __forceinline__ int dp4a(int a, int b, int c) { return __dp4a(a, b, c); }

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

__global__ void k_r8_passes(const uint8_t * __restrict__ w, const int * __restrict__ y,
                            float * __restrict__ out, unsigned long long kbx_mask) {
    const int row = blockIdx.x, tid = threadIdx.x, kqs = tid % 4;
    const int lane = tid % 32, wid = tid / 32;
    const uint8_t * rowbase = w + (size_t) row * NBLK * sizeof(block_tq2_g128);
    const block_q8_1 * yy = (const block_q8_1 *) y;
    float acc = 0.0f;
    for (int kbx = tid / 4; kbx < NBLK; kbx += 32) {
        if ((kbx_mask & (1ull << kbx)) == 0ull) continue;
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

static int32_t ord_ulp(float f) {           // cle d ordre total pour distance ULP
    int32_t i; memcpy(&i, &f, 4);
    return (i < 0) ? (int32_t) 0x80000000u - i : i;
}

int main() {
    const size_t W_BYTES = (size_t) ROWS * NBLK * sizeof(block_tq2_g128);
    uint8_t * d_w = nullptr; int * d_y = nullptr; float * d_a = nullptr; float * d_b = nullptr;
    ck(cudaMalloc(&d_w, W_BYTES), "mw");
    ck(cudaMalloc(&d_y, (size_t) NBLK * sizeof(block_q8_1)), "my");
    ck(cudaMalloc(&d_a, (size_t) ROWS * sizeof(float)), "ma");
    ck(cudaMalloc(&d_b, (size_t) ROWS * sizeof(float)), "mb");
    std::vector<uint8_t> h(W_BYTES);
    for (size_t i = 0; i < W_BYTES; ++i) h[i] = (uint8_t) ((i * 2246822519u) >> 11);
    // CORRECTION : l ancien generateur laissait des bits arbitraires dans le champ d echelle
    // (uint16_t d) des blocs tq2_g128, produisant des demi-flottants NaN/Inf. On force une
    // echelle half FINIE et non nulle sur chaque bloc.
    for (int r = 0; r < ROWS; ++r)
        for (int b = 0; b < NBLK; ++b) {
            block_tq2_g128 * blk = (block_tq2_g128 *) (h.data() + ((size_t) r * NBLK + b) * sizeof(block_tq2_g128));
            const int16_t hd = (int16_t) (0x2000 | (int16_t) ((r + b) & 0x03ff));  // exposant 8, mantisse
            memcpy(&blk->d, &hd, 2);
        }
    ck(cudaMemcpy(d_w, h.data(), W_BYTES, cudaMemcpyHostToDevice), "cw");
    std::vector<int> hy(NBLK * 8);
    for (size_t i = 0; i < hy.size(); ++i) hy[i] = (int) ((i * 1103515245u + 12345u) >> 8) & 0x01010101;
    ck(cudaMemcpy(d_y, hy.data(), hy.size()*sizeof(int), cudaMemcpyHostToDevice), "cy");

    k_full_native<<<ROWS,TPB>>>(d_w, d_y, d_a); ck(cudaDeviceSynchronize(), "nat");
    k_r8_passes  <<<ROWS,TPB>>>(d_w, d_y, d_b, 0x0000FFFFFFFFFFFFull); ck(cudaDeviceSynchronize(), "r8");

    std::vector<float> A(ROWS), B(ROWS);
    ck(cudaMemcpy(A.data(), d_a, ROWS*sizeof(float), cudaMemcpyDeviceToHost), "ca");
    ck(cudaMemcpy(B.data(), d_b, ROWS*sizeof(float), cudaMemcpyDeviceToHost), "cb");

    printf("ENGINE007A_R9A_BITWISE_MISMATCH_TAXONOMY\n\n");
    long total=0, szero=0, fin=0, nanp=0, nanv=0, inf=0, oth=0;
    long nanA=0, nanB=0;
    std::vector<double> ulps; std::vector<double> abserr; std::vector<double> relerr;
    std::vector<int> firstidx;
    printf("%-6s %-12s %-12s %-16s %-16s %-6s %s\n","idx","native_bits","r8_bits","native_float","r8_float","ulp","class");
    for (int i = 0; i < ROWS; ++i) {
        uint32_t ba, bb; memcpy(&ba, &A[i], 4); memcpy(&bb, &B[i], 4);
        if (std::isnan(A[i])) nanA++;
        if (std::isnan(B[i])) nanB++;
        if (ba == bb) continue;
        total++;
        const char * cls;
        uint32_t xa = ba & 0x7fffffffu, xb = bb & 0x7fffffffu;
        bool za = (xa == 0), zb = (xb == 0);
        if (za && zb) { cls = "SIGNED_ZERO_ONLY"; szero++; }
        else if (std::isnan(A[i]) || std::isnan(B[i])) {
            if (std::isnan(A[i]) && std::isnan(B[i])) { cls="NAN_PAYLOAD"; nanp++; }
            else { cls="NAN_VS_NUMBER"; nanv++; }
        }
        else if (std::isinf(A[i]) || std::isinf(B[i])) { cls="INF"; inf++; }
        else if (xa > 0x7f800000u || xb > 0x7f800000u) { cls="OTHER_SPECIAL"; oth++; }
        else {
            cls = "FINITE_NONZERO"; fin++;
            double d = std::fabs((double)A[i]-(double)B[i]);
            abserr.push_back(d);
            relerr.push_back(d / std::max(1e-30,(double)std::fabs(A[i])));
            ulps.push_back(std::fabs((double)ord_ulp(A[i])-(double)ord_ulp(B[i])));
        }
        if ((int)firstidx.size() < 32) {
            firstidx.push_back(i);
            char fa[64], fb[64];
            snprintf(fa,64,"%a",A[i]); snprintf(fb,64,"%a",B[i]);
            double u = (za&&zb)?0.0:std::fabs((double)ord_ulp(A[i])-(double)ord_ulp(B[i]));
            printf("%-6d 0x%08x   0x%08x   %-16s %-16s %-6.0f %s\n", i, ba, bb, fa, fb, u, cls);
        }
    }
    printf("\n=== TAXONOMIE ===\n");
    printf("MISMATCH_TOTAL=%ld\n", total);
    printf("SIGNED_ZERO_MISMATCH_COUNT=%ld\n", szero);
    printf("FINITE_NONZERO_MISMATCH_COUNT=%ld\n", fin);
    printf("NAN_PAYLOAD_MISMATCH_COUNT=%ld\n", nanp);
    printf("NAN_VS_NUMBER_COUNT=%ld\n", nanv);
    printf("INF_MISMATCH_COUNT=%ld\n", inf);
    printf("OTHER_SPECIAL_COUNT=%ld\n", oth);
    printf("SUM_CHECK=%ld\n", szero+fin+nanp+nanv+inf+oth);
    printf("MISMATCH_CLASSIFICATION_ACCOUNTING_MATCH=%s\n", (szero+fin+nanp+nanv+inf+oth)==total?"YES":"NO");
    printf("MISMATCH_FRACTION=%.6f\n", (double)total/ROWS);
    printf("\nNATIVE_NAN_COUNT=%ld\nR8_NAN_COUNT=%ld\n", nanA, nanB);
    // stats sur la population totale des sorties
    double maxabs=0, maxrel=0;
    for (int i=0;i<ROWS;++i){ double d=std::fabs((double)A[i]-(double)B[i]); if(d>maxabs)maxabs=d;
        double r=d/std::max(1e-30,(double)std::fabs(A[i])); if(r>maxrel)maxrel=r; }
    printf("\nMAX_ABS_OUTPUT_ERROR_HEX=%a\n", maxabs);
    printf("MAX_ABS_OUTPUT_ERROR_SCIENTIFIC_17DIGITS=%.17e\n", maxabs);
    printf("MAX_REL_OUTPUT_ERROR_SCIENTIFIC_17DIGITS=%.17e\n", maxrel);
    if (!ulps.empty()) {
        std::sort(ulps.begin(), ulps.end());
        auto q=[&](double p){ return ulps[(size_t)(p*(ulps.size()-1))]; };
        printf("\n=== FINITE_NONZERO : %zu elements ===\n", ulps.size());
        std::sort(abserr.begin(), abserr.end()); std::sort(relerr.begin(), relerr.end());
        printf("FINITE_MISMATCH_MAX_ABS=%.17e\nFINITE_MISMATCH_MEAN_ABS=%.17e\n", abserr.back(),
               std::accumulate(abserr.begin(),abserr.end(),0.0)/abserr.size());
        printf("FINITE_MISMATCH_MAX_REL=%.17e\nFINITE_MISMATCH_MEAN_REL=%.17e\n", relerr.back(),
               std::accumulate(relerr.begin(),relerr.end(),0.0)/relerr.size());
        printf("ULP_DISTANCE_MIN=%.0f\nULP_DISTANCE_P50=%.0f\nULP_DISTANCE_P95=%.0f\nULP_DISTANCE_P99=%.0f\nULP_DISTANCE_MAX=%.0f\n",
               ulps.front(), q(0.50), q(0.95), q(0.99), ulps.back());
    }
    cudaFree(d_w); cudaFree(d_y); cudaFree(d_a); cudaFree(d_b);
    return 0;
}
