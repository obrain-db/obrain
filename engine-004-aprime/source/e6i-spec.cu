// ENGINE-006I — specialisation de la MACHINERIE GENERIQUE a la geometrie exacte.
//
// Ce qui change : uniquement le controle et l'adressage. La machinerie generique du noyau
// produit calcule, a chaque lancement, des ratios de canaux et d'echantillons (fastdiv/fastmodulo)
// et un adressage derive de parametres de noyau. Pour la geometrie de production ces quantites
// valent l'identite (nchannels_dst=1, nsamples_dst=1, pas de canaux multiples).
//
// Ce qui ne change PAS : format de poids, chargements, decodage ternaire, chargements q8_1,
// nombre et ordre des dp4a, partition des warps, ordre de reduction, ecriture de sortie.

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
// BASE GENERIQUE : reproduit la machinerie du noyau produit. Chaque quantite
// derivee de la geometrie arrive par parametre de noyau et est recombinee ici,
// y compris les decompositions de canaux/echantillons (fastdiv/fastmodulo).
// ---------------------------------------------------------------------------
struct generic_geom {
    unsigned ncols_x;      // 48 blocs par ligne
    unsigned stride_row_x; // 48
    unsigned stride_col_y; // 1
    unsigned stride_col_dst; // 1
    unsigned nchannels_y, nchannels_dst, channel_ratio;
    unsigned nsamples_dst, sample_ratio;
    unsigned stride_sample_x, stride_sample_y, stride_sample_dst;
    unsigned stride_channel_x, stride_channel_y, stride_channel_dst;
    unsigned row0_limit;   // nrows_x
};

__device__ __forceinline__ unsigned fastdiv_u32(unsigned a, unsigned divisor) { return a / divisor; }
__device__ __forceinline__ unsigned fastmod_u32(unsigned a, unsigned divisor) { return a % divisor; }

__global__ void k_base(const uint8_t * __restrict__ w, const int * __restrict__ y,
                       float * __restrict__ out, generic_geom g) {
    constexpr int QI = 4, VDR = 1, WS = 32, NW = 4;
    constexpr int PER_ITER = VDR * NW * WS / QI;
    const int tid = threadIdx.x, lane = tid % WS, wid = tid / WS;
    const int kqs = VDR * (tid % (QI / VDR));

    // machinerie generique : decomposition canal / echantillon a partir des parametres
    const unsigned channel_dst = blockIdx.y;
    const unsigned sample_dst  = blockIdx.z;
    const unsigned channel_x = fastmod_u32(channel_dst, g.channel_ratio);
    const unsigned sample_x  = fastdiv_u32(sample_dst, g.sample_ratio);
    const unsigned channel_y = fastmod_u32(channel_dst, g.nchannels_y);
    const unsigned row0 = 0;
    const unsigned kbx_off = sample_x*g.stride_sample_x + channel_x*g.stride_channel_x + row0*g.stride_row_x;

    const unsigned nrows_x = g.row0_limit;
    if (row0 + blockIdx.x >= nrows_x) return;                 // garde generique

    const uint8_t * rowbase = w + (size_t) blockIdx.x * g.stride_row_x * sizeof(block_tq2_g128);
    const block_q8_1 * yy = (const block_q8_1 *) y
                          + sample_x*g.stride_sample_y + channel_y*g.stride_channel_y;
    float acc = 0.0f;
    for (int kbx = tid / (QI / VDR); kbx < (int) g.ncols_x; kbx += PER_ITER) {
        const block_tq2_g128 * bq = (const block_tq2_g128 *) (rowbase + (size_t) (kbx_off + kbx) * sizeof(block_tq2_g128));
        const uint8_t * qs = bq->qs + 8 * kqs;
        int sumi = 0;
#pragma unroll
        for (int t = 0; t < 4; ++t)
#pragma unroll
            for (int j = 0; j < 8; ++j) sumi = dp4a(decode_byte((int) qs[8*t + j]), yy[kbx].qs[t*8 + j], sumi);
        acc += __half2float(*(const __half *) &bq->d) * (float) sumi;
    }
    for (int o = 16; o > 0; o >>= 1) acc += __shfl_down_sync(0xffffffffu, acc, o);
    __shared__ float partial[NW];
    if (lane == 0) partial[wid] = acc;
    __syncthreads();
    if (tid == 0) {
        float s = 0.0f;
        for (int i = 0; i < NW; ++i) s += partial[i];
        out[(size_t) sample_dst*g.stride_sample_dst + channel_dst*g.stride_channel_dst + blockIdx.x*g.stride_col_dst] = s;
    }
}

// ---------------------------------------------------------------------------
// SPEC : meme corps mathematique, machinerie remplacee par des constantes exactes.
// Les seules valeurs figees sont celles de la geometrie de production mesuree.
// ---------------------------------------------------------------------------
__global__ void k_spec(const uint8_t * __restrict__ w, const int * __restrict__ y,
                       float * __restrict__ out) {
    constexpr int QI = 4, VDR = 1, WS = 32, NW = 4;
    constexpr int PER_ITER = VDR * NW * WS / QI;
    // --- constantes exactes de la geometrie de production ---
    constexpr unsigned NCOLS_X       = 48;
    constexpr unsigned STRIDE_ROW_X  = 48;
    constexpr unsigned STRIDE_COL_DST= 1;
    constexpr unsigned CHANNEL_RATIO = 1;
    constexpr unsigned SAMPLE_RATIO  = 1;
    constexpr unsigned NCHANNELS_Y   = 1;
    constexpr unsigned NROWS_X       = 5120;

    const int tid = threadIdx.x, lane = tid % WS, wid = tid / WS;
    const int kqs = VDR * (tid % (QI / VDR));

    // machinerie supprimee : channel_x/sample_x = 0, kbx_off = 0, ratios = identite
    const uint8_t * rowbase = w + (size_t) blockIdx.x * STRIDE_ROW_X * sizeof(block_tq2_g128);
    const block_q8_1 * yy = (const block_q8_1 *) y;
    static_assert(NROWS_X == 5120, "grille exacte");
    float acc = 0.0f;
    // Aucun pragma d'unroll ici : la boucle doit rester IDENTIQUE a celle de BASE, sinon on
    // mesure l'effet du deroulement et non celui de la specialisation (erreur commise au 1er jet).
    for (int kbx = tid / (QI / VDR); kbx < (int) NCOLS_X; kbx += PER_ITER) {
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
    __shared__ float partial[NW];
    if (lane == 0) partial[wid] = acc;
    __syncthreads();
    if (tid == 0) {
        float s = 0.0f;
        for (int i = 0; i < NW; ++i) s += partial[i];
        out[(size_t) blockIdx.x * STRIDE_COL_DST] = s;
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

int main() {
    const size_t W_BYTES = (size_t) ROWS * NBLK * sizeof(block_tq2_g128);
    generic_geom g{};
    g.ncols_x = NBLK; g.stride_row_x = NBLK; g.stride_col_y = 1; g.stride_col_dst = 1;
    g.nchannels_y = 1; g.nchannels_dst = 1; g.channel_ratio = 1;
    g.nsamples_dst = 1; g.sample_ratio = 1;
    g.stride_sample_x = 0; g.stride_sample_y = 0; g.stride_sample_dst = 0;
    g.stride_channel_x = 0; g.stride_channel_y = 0; g.stride_channel_dst = 0;
    g.row0_limit = ROWS;

    printf("ENGINE006I_SPEC\n");
    printf("ENGINE006I_EXACT_GEOMETRY_MANIFEST=ncols_x=%u,stride_row_x=%u,stride_col_y=%u,"
           "stride_col_dst=%u,channel_ratio=%u,sample_ratio=%u,nchannels_y=%u,nrows_x=%u,"
           "nwarps=4,rows_per_block=1,grid=(%d,1,1),block=(128,1,1)\n",
           g.ncols_x, g.stride_row_x, g.stride_col_y, g.stride_col_dst,
           g.channel_ratio, g.sample_ratio, g.nchannels_y, g.row0_limit, ROWS);

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

    // --- fidelite bit a bit (exigee par le paragraphe 5 : l'ordre mathematique est identique) ---
    std::vector<float> a(ROWS), b(ROWS);
    k_base<<<dim3(ROWS,1,1), 128>>>(d_w, d_y, d_o, g); ck(cudaDeviceSynchronize(), "base");
    ck(cudaMemcpy(a.data(), d_o, ROWS*sizeof(float), cudaMemcpyDeviceToHost), "cp a");
    k_spec<<<dim3(ROWS,1,1), 128>>>(d_w, d_y, d_o);    ck(cudaDeviceSynchronize(), "spec");
    ck(cudaMemcpy(b.data(), d_o, ROWS*sizeof(float), cudaMemcpyDeviceToHost), "cp b");
    long bad = 0; for (int i = 0; i < ROWS; ++i) if (a[i] != b[i]) bad++;
    printf("KERNEL_OUTPUT_MISMATCH_COUNT=%ld\nBITWISE_KERNEL_OUTPUT_MATCH=%s\n", bad, bad ? "NO" : "YES");

    // --- occupation ---
    cudaFuncAttributes fa{};
    ck(cudaFuncGetAttributes(&fa, (const void*) k_base), "attr base");
    int bl0 = 0; ck(cudaOccupancyMaxActiveBlocksPerMultiprocessor(&bl0, (const void*) k_base, 128, 0), "occ base");
    printf("BASE_REGISTERS=%d BASE_SHARED=%zu BASE_BLOCKS_PER_SM=%d\n", fa.numRegs, fa.sharedSizeBytes, bl0);
    cudaFuncAttributes fs{};
    ck(cudaFuncGetAttributes(&fs, (const void*) k_spec), "attr spec");
    int bl1 = 0; ck(cudaOccupancyMaxActiveBlocksPerMultiprocessor(&bl1, (const void*) k_spec, 128, 0), "occ spec");
    printf("SPEC_REGISTERS=%d SPEC_SHARED=%zu SPEC_BLOCKS_PER_SM=%d\n", fs.numRegs, fs.sharedSizeBytes, bl1);

    // --- microbench, ordre pre-enregistre ---
    const int REPS = 20;
    double tb[7], ts[7]; int nb = 0, ns = 0;
    const char * order[6] = {"BASE","SPEC","SPEC","BASE","BASE","SPEC"};
    for (int i = 0; i < 6; ++i) {
        if (order[i][0] == 'B') tb[nb++] = timeit([&]{ k_base<<<dim3(ROWS,1,1),128>>>(d_w, d_y, d_o, g); }, REPS);
        else                    ts[ns++] = timeit([&]{ k_spec<<<dim3(ROWS,1,1),128>>>(d_w, d_y, d_o); }, REPS);
    }
    printf("\n--- ordre pre-enregistre BASE SPEC SPEC BASE BASE SPEC ---\n");
    double mb = 0, ms_ = 0;
    for (int i = 0; i < 3; ++i) { printf("BASE_run%d_US=%.4f\n", i+1, 1000.0*tb[i]/REPS); mb += 1000.0*tb[i]/REPS; }
    for (int i = 0; i < 3; ++i) { printf("SPEC_run%d_US=%.4f\n", i+1, 1000.0*ts[i]/REPS); ms_ += 1000.0*ts[i]/REPS; }
    mb /= 3; ms_ /= 3;
    printf("BASE_FULL_US=%.4f\nSPEC_FULL_US=%.4f\n", mb, ms_);
    printf("DELTA_US=%+.4f\nSPEEDUP_RATIO=%.4f\n", ms_ - mb, ms_ / mb);
    return 0;
}
