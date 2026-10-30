// ENGINE-007A-R9B — replication de R2 sur donnees SAINES.
//
// Les noyaux A0/A25_A/A25_B/A50_EVEN/A50_ODD/A75/A100 sont repris SANS AUCUNE MODIFICATION.
// Seule la generation des entrees change : le champ d echelle fp16 des blocs tq2_g128 est
// desormais engendre explicitement comme une valeur FINIE, NORMALE et NON NULLE.
//
// Gates imposes avant toute mesure :
//   INPUT_SANITY_GATE  : comptage des echelles NaN/Inf/subnormales/nulles
//   OUTPUT_SANITY_GATE : comptage des sorties non finies, pour chaque bras

#include <cstdio>
#include <cstdlib>
#include <cstdint>
#include <cstring>
#include <cmath>
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
__device__ __forceinline__ bool site_active(int t, int j, int mode) {
    switch (mode) {
        case 0: return false;
        case 1: return (j % 2) == 0;
        case 2: return (j % 2) == 1;
        case 3: return (t % 2) == 0;
        case 4: return (t % 2) == 1;
        case 5: return t < 3;
        default: return true;
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
    int acc = runtime_zero, keep = runtime_zero;
    for (int kbx = tid / (QI / VDR); kbx < NBLK; kbx += PER_ITER) {
        const block_tq2_g128 * bq = (const block_tq2_g128 *) (rowbase + (size_t) kbx * sizeof(block_tq2_g128));
        const uint8_t * qs = bq->qs + 8 * kqs;
#pragma unroll
        for (int t = 0; t < 4; ++t) {
#pragma unroll
            for (int j = 0; j < 8; ++j) {
                const int dec = decode_byte((int) qs[8*t + j]);
                const int act = (int) yy[kbx].qs[t*8 + j];
                keep ^= dec; keep += act;
                if (site_active(t, j, MODE)) acc = dp4a(dec, act, acc);
            }
        }
    }
    float f = (float) acc + (float) (keep & 1);
    for (int o = 16; o > 0; o >>= 1) f += __shfl_down_sync(0xffffffffu, f, o);
    __shared__ float partial[NW];
    if (lane == 0) partial[wid] = f;
    __syncthreads();
    if (tid == 0) {
        float s = 0.0f;
        for (int i = 0; i < NW; ++i) s += partial[i];
        out[(size_t) blockIdx.x] = s;
    }
    if (keep == 0x7fffffff) keep_sink[0] = keep;
}

int main(int argc, char ** argv) {
    const int pid = (argc > 1) ? atoi(argv[1]) : 0;
    const int K   = (argc > 2) ? atoi(argv[2]) : 2048;
    const int NC  = (argc > 3) ? atoi(argv[3]) : 16;
    const size_t W_BYTES = (size_t) ROWS * NBLK * sizeof(block_tq2_g128);
    const size_t NBLK_TOT = (size_t) ROWS * NBLK;

    uint8_t * d_w = nullptr; int * d_y = nullptr; float * d_o = nullptr; int * d_k = nullptr;
    ck(cudaMalloc(&d_w, W_BYTES), "mw");
    ck(cudaMalloc(&d_y, (size_t) NBLK * sizeof(block_q8_1)), "my");
    ck(cudaMalloc(&d_o, (size_t) ROWS * sizeof(float)), "mo");
    ck(cudaMalloc(&d_k, sizeof(int)), "mk");

    // ---- generation saine, champ par champ ----
    std::vector<uint8_t> h(W_BYTES);
    for (size_t i = 0; i < W_BYTES; ++i) h[i] = (uint8_t) ((i * 2246822519u) >> 11);   // charges utiles
    long nanS=0, infS=0, subS=0, zeroS=0, finiteS=0;
    for (size_t b = 0; b < NBLK_TOT; ++b) {
        block_tq2_g128 * blk = (block_tq2_g128 *) (h.data() + b * sizeof(block_tq2_g128));
        // echelle fp16 explicite : exposant 9..13, mantisse derivee — valeur normale, finie, non nulle
        const unsigned e = 9u + (unsigned) ((b * 2654435761u) % 5u);
        const unsigned m = (unsigned) ((b * 40503u) % 1024u);
        const uint16_t hb = (uint16_t) ((e << 10) | m);
        memcpy(&blk->d, &hb, 2);
        const __half hv = *(__half *) &hb;
        const float  fv = __half2float(hv);
        if (std::isnan(fv)) nanS++;
        else if (std::isinf(fv)) infS++;
        else if (fv == 0.0f) zeroS++;
        else if (std::fabs(fv) < 6.103515625e-05f) subS++;
        else finiteS++;
    }
    ck(cudaMemcpy(d_w, h.data(), W_BYTES, cudaMemcpyHostToDevice), "cw");

    std::vector<int> hy(NBLK * 8);
    for (size_t i = 0; i < hy.size(); ++i) hy[i] = (int) ((i * 1103515245u + 12345u) >> 8) & 0x01010101;
    long actBad = 0;
    for (int b = 0; b < NBLK; ++b) {
        const block_q8_1 * q = (const block_q8_1 *) ((uint8_t *) hy.data() + (size_t) b * sizeof(block_q8_1));
        const uint16_t sl = (uint16_t) (q->ds & 0xffffu);
        const __half hv = *(__half *) &sl; const float fv = __half2float(hv);
        if (!std::isfinite(fv)) actBad++;
    }
    ck(cudaMemcpy(d_y, hy.data(), hy.size()*sizeof(int), cudaMemcpyHostToDevice), "cy");
    volatile int rz = 0; const int RZ = (int) rz;

    printf("R9B_PROCESS_ID=%d K=%d N_CYCLES=%d\n", pid, K, NC);
    printf("TQ2_BLOCK_COUNT=%zu\n", NBLK_TOT);
    printf("FINITE_SCALE_COUNT=%ld\nNAN_SCALE_COUNT=%ld\nINF_SCALE_COUNT=%ld\nSUBNORMAL_SCALE_COUNT=%ld\nZERO_SCALE_COUNT=%ld\n",
           finiteS, nanS, infS, subS, zeroS);
    // etendue des echelles utilisees
    { float mn=1e30f, mx=-1e30f;
      for (size_t b = 0; b < NBLK_TOT; b += 97) {
        const block_tq2_g128 * blk = (const block_tq2_g128 *) (h.data() + b * sizeof(block_tq2_g128));
        const float fv = __half2float(*(const __half *) &blk->d);
        if (fv<mn) mn=fv; if (fv>mx) mx=fv; }
      printf("HALF_SCALE_MIN_APPROX=%a\nHALF_SCALE_MAX_APPROX=%a\n", mn, mx); }
    printf("ACTIVATION_METADATA_NONFINITE_COUNT=%ld\n", actBad);
    printf("INPUT_SANITY_GATE=%s\n", (nanS==0 && infS==0 && zeroS==0 && actBad==0) ? "PASS" : "FAIL");
    fflush(stdout);
    if (!(nanS==0 && infS==0 && zeroS==0 && actBad==0)) { printf("ABANDON_INPUT_SANITY_FAIL\n"); return 1; }

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

    // ---- gate de sortie : chaque bras doit produire des sorties finies ----
    { std::vector<float> chk(ROWS); int tot=0;
      for (int m = 0; m < 7; ++m) {
        for (int r = 0; r < 8; ++r) L(m);
        ck(cudaDeviceSynchronize(), "chk");
        ck(cudaMemcpy(chk.data(), d_o, ROWS*sizeof(float), cudaMemcpyDeviceToHost), "cc");
        int nb=0; for (int i=0;i<ROWS;++i) if (!std::isfinite(chk[i])) nb++;
        printf("OUTPUT_NONFINITE_%-9s=%d\n", nm[m], nb); tot += nb;
      }
      printf("OUTPUT_SANITY_GATE=%s\n", tot==0 ? "PASS" : "FAIL"); }
    fflush(stdout);

    // ---- warmup ----
    for (int w = 0; w < 3; ++w) for (int m = 0; m < 7; ++m) for (int i = 0; i < K; ++i) L(m);
    ck(cudaDeviceSynchronize(), "warm");

    auto obs = [&](int m) {
        cudaEvent_t e0, e1; cudaEventCreate(&e0); cudaEventCreate(&e1);
        cudaEventRecord(e0);
        for (int r = 0; r < K; ++r) L(m);
        cudaEventRecord(e1); ck(cudaEventSynchronize(e1), "sync");
        float ms = 0; cudaEventElapsedTime(&ms, e0, e1);
        cudaEventDestroy(e0); cudaEventDestroy(e1);
        return 1000.0 * ms / K;
    };

    for (int c = 0; c < NC; ++c) {
        const int rot = c / 2; const bool rev = (c % 2) == 1;
        for (int pos = 0; pos < 7; ++pos) {
            const int slot = rev ? (6 - pos) : pos;
            const int mode = (slot + rot) % 7;
            const double us = obs(mode);
            printf("OBS pid=%d cycle=%d pos=%d arm=%s us_per_kernel=%.6f\n", pid, c, pos, nm[mode], us);
            fflush(stdout);
        }
    }
    cudaFree(d_w); cudaFree(d_y); cudaFree(d_o); cudaFree(d_k);
    return 0;
}
