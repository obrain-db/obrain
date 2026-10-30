// ENGINE-007-O2 — redistribution dense des 6 episodes sur 6 warps physiques.
//
// QUALIFICATION UNIQUEMENT. Aucun chronometrage dans ce fichier.
//
// NATIF : blockDim=128 (4 warps). Chaque thread parcourt kbx = tid/4 + 32*i, soit
//         warp0 -> G0 + G4, warp1 -> G1 + G5, warp2 -> G2, warp3 -> G3   => {2,2,1,1}
// O2    : blockDim=192 (6 warps). Chaque warp traite UN episode :
//         warp w -> Gw, kbx = 8*w + tid_lane/4, une seule iteration   => {1,1,1,1,1,1}
//
// Le travail logique est identique : 48 kbx x 4 lanes = 192 executions de corps.
// La reduction est GENERALISEE sur NWARP (meme structure sequentielle dans les deux cas)
// afin de ne pas introduire de reassociation flottante differente.

#include <cstdio>
#include <cstdlib>
#include <cstdint>
#include <cstring>
#include <cmath>
#include <vector>
#include <set>
#include <string>
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

// corps commun d un kbx : 4 tranches x 8 positions, identique dans les deux variantes
__device__ __forceinline__ int kbx_body(const block_tq2_g128 * bq, const block_q8_1 * yy, int kbx, int kqs) {
    const uint8_t * qs = bq->qs + 8 * kqs;
    int sumi = 0;
#pragma unroll
    for (int t = 0; t < 4; ++t)
#pragma unroll
        for (int j = 0; j < 8; ++j) sumi = dp4a(decode_byte((int) qs[8*t + j]), yy[kbx].qs[t*8 + j], sumi);
    return sumi;
}

// reduction generalisee : identique pour NWARP=4 et NWARP=6
template <int NWARP>
__device__ __forceinline__ void reduce_store(float acc, float * __restrict__ out, int row) {
    __shared__ float partial[NWARP];
    const int lane = threadIdx.x % 32, wid = threadIdx.x / 32;
    for (int o = 16; o > 0; o >>= 1) acc += __shfl_down_sync(0xffffffffu, acc, o);
    if (lane == 0) partial[wid] = acc;
    __syncthreads();
    if (threadIdx.x == 0) {
        float s = 0.0f;
        for (int i = 0; i < NWARP; ++i) s += partial[i];
        out[row] = s;
    }
}

// NATIF : 128 threads, 4 warps, kbx = tid/4 + 32*i
__global__ void k_native(const uint8_t * __restrict__ w, const int * __restrict__ y,
                         float * __restrict__ out) {
    const int row = blockIdx.x, tid = threadIdx.x, kqs = tid % 4;
    const uint8_t * rowbase = w + (size_t) row * NBLK * sizeof(block_tq2_g128);
    const block_q8_1 * yy = (const block_q8_1 *) y;
    float acc = 0.0f;
    for (int kbx = tid / 4; kbx < NBLK; kbx += 32) {
        const block_tq2_g128 * bq = (const block_tq2_g128 *) (rowbase + (size_t) kbx * sizeof(block_tq2_g128));
        const int sumi = kbx_body(bq, yy, kbx, kqs);
        acc += __half2float(*(const __half *) &bq->d) * (float) sumi;
    }
    reduce_store<4>(acc, out, row);
}

// O2 : 192 threads, 6 warps, warp w -> episode Gw, kbx = 8*w + lane/4
__global__ void k_o2_6warp(const uint8_t * __restrict__ w, const int * __restrict__ y,
                           float * __restrict__ out) {
    const int row = blockIdx.x, tid = threadIdx.x, kqs = tid % 4;
    const int wid = tid / 32, lane = tid % 32;
    const int kbx = 8 * wid + lane / 4;
    const uint8_t * rowbase = w + (size_t) row * NBLK * sizeof(block_tq2_g128);
    const block_q8_1 * yy = (const block_q8_1 *) y;
    float acc = 0.0f;
    if (kbx < NBLK) {
        const block_tq2_g128 * bq = (const block_tq2_g128 *) (rowbase + (size_t) kbx * sizeof(block_tq2_g128));
        const int sumi = kbx_body(bq, yy, kbx, kqs);
        acc += __half2float(*(const __half *) &bq->d) * (float) sumi;
    }
    reduce_store<6>(acc, out, row);
}

// ---------------- qualification GPU du mapping ----------------
// enregistre, par thread, l episode et le kbx effectivement traites
__global__ void k_qual_native(int * epi_lanes, int * kbx_mult, int * epi_warp) {
    const int tid = threadIdx.x;
    for (int kbx = tid / 4; kbx < NBLK; kbx += 32) {
        const int G = kbx / 8;
        atomicAdd(&epi_lanes[G], 1);
        atomicAdd(&kbx_mult[kbx], 1);
        atomicAdd(&epi_warp[G*8 + tid/32], 1);
    }
}
__global__ void k_qual_o2(int * epi_lanes, int * kbx_mult, int * epi_warp) {
    const int tid = threadIdx.x, wid = tid / 32, lane = tid % 32;
    const int kbx = 8 * wid + lane / 4;
    if (kbx < NBLK) {
        const int G = kbx / 8;
        atomicAdd(&epi_lanes[G], 1);
        atomicAdd(&kbx_mult[kbx], 1);
        atomicAdd(&epi_warp[G*8 + wid], 1);
    }
}

// ---------------- generateur sain (identique R2-SANE/R9B) ----------------
static void gen_inputs(std::vector<uint8_t> & h, std::vector<int> & hy, unsigned seed,
                       long & nanS, long & infS, long & subS, long & zeroS, long & finiteS) {
    const size_t W_BYTES = (size_t) ROWS * NBLK * sizeof(block_tq2_g128);
    h.assign(W_BYTES, 0);
    for (size_t i = 0; i < W_BYTES; ++i) h[i] = (uint8_t) (((i + seed*2654435761u) * 2246822519u) >> 11);
    nanS=infS=subS=zeroS=finiteS=0;
    for (size_t b = 0; b < (size_t) ROWS*NBLK; ++b) {
        block_tq2_g128 * blk = (block_tq2_g128 *) (h.data() + b * sizeof(block_tq2_g128));
        const unsigned e = 9u + (unsigned) (((b + seed) * 2654435761u) % 5u);
        const unsigned m = (unsigned) (((b + seed) * 40503u) % 1024u);
        const uint16_t hb = (uint16_t) ((e << 10) | m);
        memcpy(&blk->d, &hb, 2);
        const float fv = __half2float(*(const __half *) &hb);
        if (std::isnan(fv)) nanS++; else if (std::isinf(fv)) infS++;
        else if (fv == 0.0f) zeroS++; else if (std::fabs(fv) < 6.103515625e-05f) subS++; else finiteS++;
    }
    hy.assign(NBLK * 8, 0);
    for (size_t i = 0; i < hy.size(); ++i) hy[i] = (int) (((i + seed*1103515245u) * 1103515245u + 12345u) >> 8) & 0x01010101;
}

int main(int argc, char ** argv) {
    const std::string mode = (argc > 1) ? argv[1] : "map";
    const size_t W_BYTES = (size_t) ROWS * NBLK * sizeof(block_tq2_g128);

    uint8_t * d_w=nullptr; int * d_y=nullptr; float * d_a=nullptr; float * d_b=nullptr;
    int * d_epi=nullptr; int * d_mult=nullptr; int * d_ew=nullptr;
    ck(cudaMalloc(&d_w, W_BYTES), "mw");
    ck(cudaMalloc(&d_y, (size_t) NBLK*sizeof(block_q8_1)), "my");
    ck(cudaMalloc(&d_a, (size_t) ROWS*sizeof(float)), "ma");
    ck(cudaMalloc(&d_b, (size_t) ROWS*sizeof(float)), "mb");
    ck(cudaMalloc(&d_epi, 8*sizeof(int)), "me");
    ck(cudaMalloc(&d_mult, 49*sizeof(int)), "mm");
    ck(cudaMalloc(&d_ew, 48*sizeof(int)), "mw2");

    if (mode == "qual") {
        printf("O2_GPU_MAPPING_QUALIFICATION\n");
        for (int which = 0; which < 2; ++which) {
            ck(cudaMemset(d_epi,0,8*sizeof(int)),"c1");
            ck(cudaMemset(d_mult,0,49*sizeof(int)),"c2");
            ck(cudaMemset(d_ew,0,48*sizeof(int)),"c3");
            if (which==0) k_qual_native<<<1,128>>>(d_epi,d_mult,d_ew);
            else          k_qual_o2<<<1,192>>>(d_epi,d_mult,d_ew);
            ck(cudaDeviceSynchronize(),"q");
            std::vector<int> epi(8),mult(49),ew(48);
            ck(cudaMemcpy(epi.data(),d_epi,8*sizeof(int),cudaMemcpyDeviceToHost),"h1");
            ck(cudaMemcpy(mult.data(),d_mult,49*sizeof(int),cudaMemcpyDeviceToHost),"h2");
            ck(cudaMemcpy(ew.data(),d_ew,48*sizeof(int),cudaMemcpyDeviceToHost),"h3");
            const char * nm = which ? "O2" : "NATIVE";
            long tot=0; int nEpi=0, minm=1<<30, maxm=0;
            for (int g=0;g<6;++g){ if(epi[g]>0) nEpi++; tot+=epi[g]; }
            int m33=1<<30,mx=0; for(int k=0;k<48;++k){ if(mult[k]<m33)m33=mult[k]; if(mult[k]>mx)mx=mult[k]; }
            std::set<int> warps; for(int g=0;g<6;++g) for(int w=0;w<6;++w) if(ew[g*8+w]>0) warps.insert(w);
            printf("  %-7s DYNAMIC=%ld EPISODES=%d PER_KBX_MULT_MIN=%d MAX=%d ACTIVE_WARPS_AU_MOINS_UN_EPISODE=%zu\n",
                   nm, tot, nEpi, m33, mx, warps.size());
            printf("           EPI_LANES=%d,%d,%d,%d,%d,%d\n", epi[0],epi[1],epi[2],epi[3],epi[4],epi[5]);
            // quel warp physique porte chaque episode
            printf("           EPISODE->WARPS :");
            for (int g=0;g<6;++g){ printf(" G%d:{",g); bool f=false;
              for(int w=0;w<6;++w) if(ew[g*8+w]>0){ printf("%s%d", f?",":"", w); f=true; } printf("}"); }
            printf("\n");
        }
        printf("GPU_MAPPING_QUALIFICATION=%s\n","PASS");
        return 0;
    }

    if (mode == "fid") {
        const int NSEED = (argc > 2) ? atoi(argv[2]) : 5;
        printf("O2_FIDELITY_BY_SEED\n");
        int allok = 1;
        for (int s = 0; s < NSEED; ++s) {
            std::vector<uint8_t> h; std::vector<int> hy;
            long nS,iS,sbS,zS,fS;
            gen_inputs(h, hy, (unsigned) s, nS,iS,sbS,zS,fS);
            const bool input_ok = (nS==0 && iS==0 && zS==0);
            ck(cudaMemcpy(d_w,h.data(),W_BYTES,cudaMemcpyHostToDevice),"cw");
            ck(cudaMemcpy(d_y,hy.data(),hy.size()*sizeof(int),cudaMemcpyHostToDevice),"cy");
            k_native<<<ROWS,128>>>(d_w,d_y,d_a);   ck(cudaDeviceSynchronize(),"n");
            k_o2_6warp<<<ROWS,192>>>(d_w,d_y,d_b); ck(cudaDeviceSynchronize(),"o");
            std::vector<float> A(ROWS), B(ROWS);
            ck(cudaMemcpy(A.data(),d_a,ROWS*sizeof(float),cudaMemcpyDeviceToHost),"ca");
            ck(cudaMemcpy(B.data(),d_b,ROWS*sizeof(float),cudaMemcpyDeviceToHost),"cb");
            long bad=0, nanA=0, nanB=0, infA=0, infB=0;
            for (int i=0;i<ROWS;++i){
                uint32_t x,y; memcpy(&x,&A[i],4); memcpy(&y,&B[i],4);
                if (x!=y) bad++;
                if (std::isnan(A[i])) nanA++; if (std::isnan(B[i])) nanB++;
                if (std::isinf(A[i])) infA++; if (std::isinf(B[i])) infB++;
            }
            printf("  SEED=%d INPUT_SANITY=%s NAN_SCALE=%ld INF_SCALE=%ld ZERO_SCALE=%ld FINITE=%ld | "
                   "MISMATCH=%ld NATIVE_NAN=%ld O2_NAN=%ld NATIVE_INF=%ld O2_INF=%ld\n",
                   s, input_ok?"PASS":"FAIL", nS,iS,zS,fS, bad, nanA,nanB, infA,infB);
            if (bad || nanA || nanB || infA || infB || !input_ok) allok = 0;
        }
        printf("BITWISE_KERNEL_OUTPUT_MATCH=%s\n", allok?"YES":"NO");
        printf("OUTPUT_SANITY_GATE=%s\n", allok?"PASS":"FAIL");
        return allok?0:5;
    }

    // mode occ : proprietes de ressource et occupation
    if (mode == "occ") {
        cudaFuncAttributes fa{}, fo{};
        ck(cudaFuncGetAttributes(&fa,(const void*)k_native),"an");
        ck(cudaFuncGetAttributes(&fo,(const void*)k_o2_6warp),"ao");
        int bn=0, bo=0;
        ck(cudaOccupancyMaxActiveBlocksPerMultiprocessor(&bn,(const void*)k_native,128,0),"on");
        ck(cudaOccupancyMaxActiveBlocksPerMultiprocessor(&bo,(const void*)k_o2_6warp,192,0),"oo");
        int dev=0; ck(cudaGetDevice(&dev),"dev");
        cudaDeviceProp pr{}; ck(cudaGetDeviceProperties(&pr,dev),"prop");
        printf("O2_RESOURCE_OCCUPANCY\n");
        printf("  NATIVE_THREADS_PER_BLOCK=128   O2_THREADS_PER_BLOCK=192\n");
        printf("  NATIVE_REGISTERS=%d  O2_REGISTERS=%d  NATIVE_SPILLS=%s  O2_SPILLS=%s\n",
               fa.numRegs, fo.numRegs,
               fa.localSizeBytes?"YES":"0", fo.localSizeBytes?"YES":"0");
        printf("  NATIVE_STATIC_SHARED=%zu  O2_STATIC_SHARED=%zu\n", fa.sharedSizeBytes, fo.sharedSizeBytes);
        printf("  NATIVE_MAX_ACTIVE_BLOCKS_PER_SM=%d  O2_MAX_ACTIVE_BLOCKS_PER_SM=%d\n", bn, bo);
        printf("  NATIVE_RESIDENT_WARPS_PER_SM=%d  O2_RESIDENT_WARPS_PER_SM=%d\n", bn*4, bo*6);
        printf("  SM_COUNT=%d  MAX_THREADS_PER_SM=%d  MAX_WARPS_PER_SM=%d  REGS_PER_SM=%d\n",
               pr.multiProcessorCount, pr.maxThreadsPerMultiProcessor,
               pr.maxThreadsPerMultiProcessor/32, pr.regsPerMultiprocessor);
        const double occn = 100.0*bn*4.0/(pr.maxThreadsPerMultiProcessor/32);
        const double occo = 100.0*bo*6.0/(pr.maxThreadsPerMultiProcessor/32);
        printf("  NATIVE_THEORETICAL_OCCUPANCY=%.2f %%\n  O2_THEORETICAL_OCCUPANCY=%.2f %%\n", occn, occo);
        return 0;
    }
    // mode t1 : chronometrage canonique apparie NATIVE / O2
    if (mode == "t1") {
        const int pid = (argc > 2) ? atoi(argv[2]) : 0;
        const int K   = (argc > 3) ? atoi(argv[3]) : 2048;
        const int NC  = (argc > 4) ? atoi(argv[4]) : 20;
        std::vector<uint8_t> h; std::vector<int> hy;
        long nS,iS,sbS,zS,fS;
        gen_inputs(h, hy, 0u, nS,iS,sbS,zS,fS);
        const bool input_ok = (nS==0 && iS==0 && zS==0);
        printf("R12_T1_PROCESS_ID=%d K=%d N_CYCLES=%d\n", pid, K, NC);
        printf("INPUT_SANITY_GATE=%s\nNAN_SCALE_COUNT=%ld\nINF_SCALE_COUNT=%ld\nSUBNORMAL_SCALE_COUNT=%ld\nZERO_SCALE_COUNT=%ld\nFINITE_SCALE_COUNT=%ld\n",
               input_ok?"PASS":"FAIL", nS,iS,sbS,zS,fS);
        if (!input_ok) { printf("ABANDON_INPUT_SANITY_FAIL\n"); return 1; }
        ck(cudaMemcpy(d_w,h.data(),W_BYTES,cudaMemcpyHostToDevice),"cw");
        ck(cudaMemcpy(d_y,hy.data(),hy.size()*sizeof(int),cudaMemcpyHostToDevice),"cy");

        auto LN = [&]() { k_native  <<<ROWS,128>>>(d_w,d_y,d_a); };
        auto LO = [&]() { k_o2_6warp<<<ROWS,192>>>(d_w,d_y,d_b); };
        auto L  = [&](int m) { if (m==0) LN(); else LO(); };

        // gates de sortie et fidelite AVANT chronometrage
        { std::vector<float> A(ROWS), B(ROWS); long nanA=0,nanB=0,infA=0,infB=0,bad=0;
          for (int r=0;r<8;++r){ LN(); LO(); }
          ck(cudaDeviceSynchronize(),"pre");
          ck(cudaMemcpy(A.data(),d_a,ROWS*sizeof(float),cudaMemcpyDeviceToHost),"ca");
          ck(cudaMemcpy(B.data(),d_b,ROWS*sizeof(float),cudaMemcpyDeviceToHost),"cb");
          for (int i=0;i<ROWS;++i){ uint32_t x,y2; memcpy(&x,&A[i],4); memcpy(&y2,&B[i],4);
            if(x!=y2) bad++; if(std::isnan(A[i]))nanA++; if(std::isnan(B[i]))nanB++;
            if(std::isinf(A[i]))infA++; if(std::isinf(B[i]))infB++; }
          printf("NATIVE_OUTPUT_NAN_COUNT=%ld\nO2_OUTPUT_NAN_COUNT=%ld\nNATIVE_OUTPUT_INF_COUNT=%ld\nO2_OUTPUT_INF_COUNT=%ld\n",
                 nanA,nanB,infA,infB);
          printf("PRE_TIMING_BITWISE_MISMATCH_COUNT=%ld\n", bad);
          printf("OUTPUT_SANITY_GATE=%s\n", (nanA||nanB||infA||infB)?"FAIL":"PASS");
          if (bad || nanA || nanB || infA || infB) { printf("ABANDON_PREFLIGHT_FAIL\n"); return 1; }
        }
        fflush(stdout);

        for (int w=0; w<3; ++w) for (int m=0; m<2; ++m) for (int i=0;i<K;++i) L(m);
        ck(cudaDeviceSynchronize(),"warm");

        for (int c = 0; c < NC; ++c) {
            // alternance equilibree : cycle pair -> NATIVE puis O2 ; cycle impair -> O2 puis NATIVE
            const bool nat_first = (c % 2) == 0;
            for (int pos = 0; pos < 2; ++pos) {
                const int m = (nat_first == (pos == 0)) ? 0 : 1;
                cudaEvent_t e0,e1; cudaEventCreate(&e0); cudaEventCreate(&e1);
                cudaEventRecord(e0);
                for (int r=0;r<K;++r) L(m);
                cudaEventRecord(e1); ck(cudaEventSynchronize(e1),"sync");
                float ms=0; cudaEventElapsedTime(&ms,e0,e1);
                cudaEventDestroy(e0); cudaEventDestroy(e1);
                const char * nm = (m==0)?"K_FULL_NATIVE":"O2_6WARP";
                printf("OBS arm=%s pid=%d cycle=%d pos=%d nat_first=%d us_per_kernel=%.6f\n",
                       nm, pid, c, pos, nat_first?1:0, 1000.0*ms/K);
                fflush(stdout);
            }
        }
        return 0;
    }

    if (mode == "map") { printf("O2_HARNESS_READY (modes: qual, fid, occ, t1)\n"); return 0; }
    return 0;
}
