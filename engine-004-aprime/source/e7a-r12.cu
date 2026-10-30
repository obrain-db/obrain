// ENGINE-007A-R12 — activation par episode de warp et densite de lanes.
//
// Axe A : densite dans UN episode (G0 = kbx 0..7), 1 a 8 blocs selectionnes.
// Axe B : a kbx logique constant (6) et dynamique constant (24), etaler le travail sur
//         1, 2, 3, 4 ou 6 episodes de warp.
//
// Un episode = (warp, iteration). Le mapping natif donne :
//   G = kbx / 8, warp = G % 4, iteration = G / 4   pour kbx < 48
// Chaque kbx selectionne couvre 4 lanes (tid%4).
//
// Meme noyau runtime valide que R9/R10/R11. Seuls les masques changent.
// Ancres : KBX00_EMPTY, KBX48_NATIVE, K_FULL_NATIVE.

#include <cstdio>
#include <cstdlib>
#include <cstdint>
#include <cstring>
#include <cmath>
#include <string>
#include <vector>
#include <map>
#include <set>
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
    return ((c0 > 2 ? 1 : c0 - 1) & 0xFF) | (((c1 > 2 ? 1 : c1 - 1) & 0xFF) << 8) |
           (((c2 > 2 ? 1 : c2 - 1) & 0xFF) << 16) | (((c3 > 2 ? 1 : c3 - 1) & 0xFF) << 24);
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

__global__ void k_kbx_runtime(const uint8_t * __restrict__ w, const int * __restrict__ y,
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

// qualification : compteurs par episode (warp*2+iteration) et par warp
__global__ void k_episode_qual(const uint8_t * __restrict__ w, unsigned long long kbx_mask,
                               int * __restrict__ epi_lanes, int * __restrict__ warp_lanes,
                               int * __restrict__ kbx_count) {
    const int tid = threadIdx.x, wid = tid / 32;
    for (int kbx = tid / 4; kbx < NBLK; kbx += 32) {
        if ((kbx_mask & (1ull << kbx)) == 0ull) continue;
        const int G = kbx / 8;
        atomicAdd(&epi_lanes[G], 1);
        atomicAdd(&warp_lanes[wid], 1);
        atomicAdd(&kbx_count[kbx], 1);
        if (w == (const uint8_t *) 0x1) epi_lanes[16] += 1;   // jamais pris
    }
}

struct Arm {
    std::string name, kernel, group, config_sha, replicate_of;
    unsigned long long mask;
    std::vector<int> set;
    std::vector<std::pair<int,int>> seq;   // (tid, kbx)
    bool is_replicate;
};

static std::vector<int> set_of(unsigned long long m) {
    std::vector<int> v; for (int b = 0; b < 64; ++b) if (m & (1ull<<b)) v.push_back(b); return v;
}
static unsigned long long mask_from(const std::vector<int> & b) {
    unsigned long long m = 0; for (int x : b) m |= (1ull<<x); return m;
}
static std::string j(const std::vector<int> & v, const char * sep=",") {
    std::string s; char b[16];
    for (size_t i = 0; i < v.size(); ++i) { snprintf(b,16,"%d",v[i]); if(i) s+=sep; s+=b; } return s;
}
static std::string js(const std::set<int> & v, const char * sep=",") {
    std::vector<int> t(v.begin(), v.end()); return j(t, sep);
}

int main(int argc, char ** argv) {
    const std::string mode = (argc > 1) ? argv[1] : "gen";
    const size_t W_BYTES = (size_t) ROWS * NBLK * sizeof(block_tq2_g128);
    const size_t NBLK_TOT = (size_t) ROWS * NBLK;

    std::vector<uint8_t> h(W_BYTES);
    for (size_t i = 0; i < W_BYTES; ++i) h[i] = (uint8_t) ((i * 2246822519u) >> 11);
    long nanS=0, infS=0, subS=0, zeroS=0, finiteS=0;
    for (size_t b = 0; b < NBLK_TOT; ++b) {
        block_tq2_g128 * blk = (block_tq2_g128 *) (h.data() + b * sizeof(block_tq2_g128));
        const unsigned e = 9u + (unsigned) ((b * 2654435761u) % 5u);
        const unsigned m = (unsigned) ((b * 40503u) % 1024u);
        const uint16_t hb = (uint16_t) ((e << 10) | m);
        memcpy(&blk->d, &hb, 2);
        const float fv = __half2float(*(const __half *) &hb);
        if (std::isnan(fv)) nanS++; else if (std::isinf(fv)) infS++;
        else if (fv == 0.0f) zeroS++; else if (std::fabs(fv) < 6.103515625e-05f) subS++; else finiteS++;
    }
    std::vector<int> hy(NBLK * 8);
    for (size_t i = 0; i < hy.size(); ++i) hy[i] = (int) ((i * 1103515245u + 12345u) >> 8) & 0x01010101;
    long actBad = 0;
    for (int b = 0; b < NBLK; ++b) {
        const block_q8_1 * q = (const block_q8_1 *) ((uint8_t *) hy.data() + (size_t) b * sizeof(block_q8_1));
        if (!std::isfinite(__half2float(*(const __half *) &(*(const uint16_t*)&q->ds)))) actBad++;
    }
    const bool input_ok = (nanS==0 && infS==0 && zeroS==0 && actBad==0);

    // ---------- construction de la famille ----------
    std::vector<Arm> arms;
    auto add = [&](const char * nm, const char * grp, unsigned long long mask, const char * repof) {
        Arm a; a.name = nm; a.group = grp; a.mask = mask;
        a.set = set_of(mask);
        for (int tid = 0; tid < TPB; ++tid)
            for (int kbx = tid/4; kbx < NBLK; kbx += 32)
                if (mask & (1ull<<kbx)) a.seq.push_back({tid, kbx});
        a.is_replicate = (repof && *repof); a.replicate_of = repof ? repof : "";
        a.kernel = (std::string(nm) == "K_FULL_NATIVE") ? "_Z13k_full_nativePKhPKiPf"
                                                        : "_Z15k_kbx_runtimePKhPKiPf";
        a.config_sha = a.kernel + "|" + std::to_string(mask) + "|" + j(a.set) + "|" + std::to_string(a.seq.size());
        arms.push_back(a);
    };
    const unsigned long long FULL = 0x0000FFFFFFFFFFFFull;
    auto R = [](std::initializer_list<int> l){ return mask_from(std::vector<int>(l)); };

    // ancres
    add("KBX00_EMPTY",   "ANCHOR", 0ull, "");
    add("K_FULL_NATIVE", "ANCHOR", FULL, "");
    add("KBX48_NATIVE",  "ANCHOR", FULL, "");
    add("KBX06_SET_A",   "ANCHOR", 0x010101010101ull, "");   // historique, doit valoir E6

    // AXE A : densite dans G0 = {0..7}
    add("G0_D1", "AXE_A+EPI_ID", R({0}), "");
    add("G0_D2", "AXE_A", R({0,1}), "");
    add("G0_D4", "AXE_A", R({0,1,2,3}), "");
    add("G0_D6", "AXE_A+AXE_B_E1", R({0,1,2,3,4,5}), "");
    add("G0_D8", "AXE_A", R({0,1,2,3,4,5,6,7}), "");

    // un seul kbx, dans chacun des six episodes
    add("G1_D1_SINGLE", "EPI_ID", R({8}), "");
    add("G2_D1_SINGLE", "EPI_ID", R({16}), "");
    add("G3_D1_SINGLE", "EPI_ID", R({24}), "");
    add("G4_D1_SINGLE", "EPI_ID", R({32}), "");
    add("G5_D1_SINGLE", "EPI_ID", R({40}), "");

    // AXE B : 6 kbx logiques / 24 dynamiques, nombre d episodes variable
    add("E2_SAME_WARP",      "AXE_B", R({0,1,2,32,33,34}), "");           // G0 + G4 (warp 0)
    add("E2_DIFFERENT_WARPS","AXE_B", R({0,1,2,8,9,10}), "");             // G0 + G1 (warps 0,1)
    add("E3",                "AXE_B", R({0,1,8,9,16,17}), "");            // G0,G1,G2
    add("E4",                "AXE_B", R({0,1,8,9,16,24}), "");            // G0..G3
    add("E6",                "AXE_B", R({0,8,16,24,32,40}), "KBX06_SET_A"); // les six

    // ---------- descripteurs ----------
    auto episode_of = [](int kbx){ return kbx / 8; };
    struct D { std::set<int> epi, warps; std::map<int,int> lanes; };
    auto desc = [&](const Arm & a) {
        D d;
        std::map<int,int> lanes;
        for (auto & pr : a.seq) { int t = pr.first, k = pr.second; d.epi.insert(episode_of(k)); d.warps.insert(t/32); lanes[episode_of(k)]++; }
        d.lanes = lanes; return d;
    };

    // gates
    bool gates_ok = true;
    std::map<std::string,std::string> conf; int dup = 0;
    for (auto & a : arms) {
        if ((int) a.set.size() != __builtin_popcountll(a.mask)) { printf("ERREUR_CARDINALITE %s\n", a.name.c_str()); gates_ok = false; }
        if (conf.count(a.config_sha)) {
            if (a.is_replicate && a.replicate_of == conf[a.config_sha]) { /* replicate declare : accepte */ }
            else { dup++; printf("DOUBLON_NON_DECLARE %s vs %s\n", a.name.c_str(), conf[a.config_sha].c_str()); }
        } else conf[a.config_sha] = a.name;
    }

    if (mode == "gen") {
        printf("ARM_TABLE_VERSION=2_FAMILY_R12\n");
        printf("MAPPING=kbx = tid/4 + 32*i ; episode G = kbx/8 ; warp = G%%4 ; iteration = G/4 ; 4 lanes par kbx\n");
        printf("FORMAT=ARM|GROUP|MASK_HEX|KBX_LOGICAL|KBX_DYNAMIC|EPISODES|EPISODE_SET|WARPS|WARP_SET|LANES_PER_EPISODE|IS_REPLICATE|REPLICATE_OF\n");
        for (auto & a : arms) {
            D d = desc(a);
            std::string lp; for (auto & kv : d.lanes) { char b[24]; snprintf(b,24,"G%d:%d ",kv.first,kv.second); lp += b; }
            printf("%s|%s|0x%012llx|%d|%zu|%zu|%s|%zu|%s|%s|%s|%s\n",
                   a.name.c_str(), a.group.c_str(), a.mask, (int) a.set.size(), a.seq.size(),
                   d.epi.size(), js(d.epi).c_str(), d.warps.size(), js(d.warps).c_str(),
                   lp.c_str(), a.is_replicate?"YES":"NO", a.replicate_of.c_str());
        }
        printf("ARM_COUNT=%zu\n", arms.size());
        printf("UNEXPECTED_DUPLICATE_CONFIG_COUNT=%d\n", dup);
        // controle attendu du paragraphe 4
        for (auto & a : arms) if (a.name == "KBX06_SET_A") {
            D d = desc(a);
            printf("KBX06_ACTIVE_WARP_EPISODE_COUNT=%zu\n", d.epi.size());
            printf("KBX06_DID_NOT_REDUCE_WARP_EPISODE_COVERAGE=%s\n", d.epi.size()==6?"YES":"NO");
        }
        printf("INPUT_SANITY_GATE=%s\n", input_ok?"PASS":"FAIL");
        printf("NAN_SCALE_COUNT=%ld\nINF_SCALE_COUNT=%ld\nSUBNORMAL_SCALE_COUNT=%ld\nZERO_SCALE_COUNT=%ld\nFINITE_SCALE_COUNT=%ld\n",
               nanS,infS,subS,zeroS,finiteS);
        fflush(stdout);
        return (gates_ok && dup==0) ? 0 : 3;
    }

    // ---------- qualification GPU ----------
    uint8_t * d_w = nullptr; int * d_y = nullptr; int * d_epi = nullptr; int * d_warp = nullptr; int * d_kc = nullptr;
    ck(cudaMalloc(&d_w, W_BYTES), "mw");
    ck(cudaMalloc(&d_y, (size_t) NBLK*sizeof(block_q8_1)), "my");
    ck(cudaMalloc(&d_epi, 17*sizeof(int)), "me");
    ck(cudaMalloc(&d_warp, 4*sizeof(int)), "mwp");
    ck(cudaMalloc(&d_kc, 65*sizeof(int)), "mk");
    ck(cudaMemcpy(d_w, h.data(), W_BYTES, cudaMemcpyHostToDevice), "cw");
    ck(cudaMemcpy(d_y, hy.data(), hy.size()*sizeof(int), cudaMemcpyHostToDevice), "cy");

    if (mode == "qual") {
        printf("GPU_QUALIFICATION_VERSION=2_R12\nINPUT_SANITY_GATE=%s\n", input_ok?"PASS":"FAIL");
        bool allok = true;
        for (auto & a : arms) {
            if (a.name == "K_FULL_NATIVE") continue;
            ck(cudaMemset(d_epi,0,17*sizeof(int)),"c1"); ck(cudaMemset(d_warp,0,4*sizeof(int)),"c2"); ck(cudaMemset(d_kc,0,65*sizeof(int)),"c3");
            k_episode_qual<<<1,TPB>>>(d_w, a.mask, d_epi, d_warp, d_kc);
            ck(cudaDeviceSynchronize(),"q");
            std::vector<int> epi(17),wp(4),kc(65);
            ck(cudaMemcpy(epi.data(),d_epi,17*sizeof(int),cudaMemcpyDeviceToHost),"h1");
            ck(cudaMemcpy(wp.data(),d_warp,4*sizeof(int),cudaMemcpyDeviceToHost),"h2");
            ck(cudaMemcpy(kc.data(),d_kc,65*sizeof(int),cudaMemcpyDeviceToHost),"h3");
            D d = desc(a);
            std::set<int> gpu_epi; std::string lp; long tot=0;
            for (int g = 0; g < 6; ++g) { if (epi[g]>0) { gpu_epi.insert(g); char b[24]; snprintf(b,24,"G%d:%d ",g,epi[g]); lp+=b; } tot += epi[g]; }
            std::set<int> gpu_w; for (int w2=0;w2<4;++w2) if (wp[w2]>0) gpu_w.insert(w2);
            bool epiok = (gpu_epi == d.epi), cok = (tot == (long) a.seq.size()), wok = (gpu_w == d.warps);
            printf("ARM=%s GPU_DYN=%ld EXPECTED_DYN=%zu GPU_EPISODES=%zu GPU_EPISODE_SET=%s EXPECTED_EPISODE_SET=%s EPI_MATCH=%s DYN_MATCH=%s WARP_MATCH=%s LANES=%s\n",
                   a.name.c_str(), tot, a.seq.size(), gpu_epi.size(), js(gpu_epi).c_str(), js(d.epi).c_str(),
                   epiok?"YES":"NO", cok?"YES":"NO", wok?"YES":"NO", lp.c_str());
            if (!(epiok&&cok&&wok)) allok = false;
        }
        printf("GPU_KBX_QUALIFICATION=%s\n", allok?"PASS":"FAIL");
        printf("GPU_WARP_EPISODE_QUALIFICATION=%s\n", allok?"PASS":"FAIL");
        return allok?0:4;
    }

    // ---------- chronometrage ----------
    if (mode == "time") {
        const int pid = (argc > 2) ? atoi(argv[2]) : 0;
        const int K   = (argc > 3) ? atoi(argv[3]) : 2048;
        const int NC  = (argc > 4) ? atoi(argv[4]) : 18;
        const int NARM= (argc > 5) ? atoi(argv[5]) : (int) arms.size();
        const int NA  = (int) arms.size();
        const int NB  = (NARM > 0 && NARM <= NA) ? NARM : NA;
        if (!input_ok) { printf("ABANDON_INPUT_SANITY_FAIL\n"); return 1; }

        float * d_o = nullptr;
        ck(cudaMalloc(&d_o, (size_t) ROWS*sizeof(float)), "mo");
        printf("R12_TIME_PROCESS_ID=%d K=%d N_CYCLES=%d N_TOTAL=%d N_MEASURED=%d\n", pid,K,NC,NA,NB);
        printf("INPUT_SANITY_GATE=%s\n", input_ok?"PASS":"FAIL");
        for (int i = 0; i < NB; ++i) {
            D d = desc(arms[i]);
            printf("ARMPLAN name=%s mask=0x%012llx kbx=%d dyn=%zu episodes=%zu warps=%zu config=%s\n",
                   arms[i].name.c_str(), arms[i].mask, (int) arms[i].set.size(), arms[i].seq.size(),
                   d.epi.size(), d.warps.size(), arms[i].config_sha.c_str());
        }
        fflush(stdout);

        auto L = [&](int i) {
            if (arms[i].name == "K_FULL_NATIVE") k_full_native<<<ROWS,TPB>>>(d_w,d_y,d_o);
            else k_kbx_runtime<<<ROWS,TPB>>>(d_w,d_y,d_o, arms[i].mask);
        };
        { std::vector<float> chk(ROWS); int tot=0;
          for (int m = 0; m < NB; ++m) {
            for (int r = 0; r < 8; ++r) L(m);
            ck(cudaDeviceSynchronize(),"chk");
            ck(cudaMemcpy(chk.data(), d_o, ROWS*sizeof(float), cudaMemcpyDeviceToHost),"cc");
            int nb=0; for (int i=0;i<ROWS;++i) if(!std::isfinite(chk[i])) nb++;
            printf("OUTPUT_NONFINITE_%-20s=%d\n", arms[m].name.c_str(), nb); tot+=nb;
          }
          printf("OUTPUT_SANITY_GATE=%s\n", tot==0?"PASS":"FAIL"); }
        fflush(stdout);

        for (int w2 = 0; w2 < 3; ++w2) for (int m = 0; m < NB; ++m) for (int i = 0; i < K; ++i) L(m);
        ck(cudaDeviceSynchronize(),"warm");

        for (int c = 0; c < NC; ++c) {
            const int rot = c/2; const bool rev = (c%2)==1;
            for (int pos = 0; pos < NB; ++pos) {
                const int slot = rev ? (NB-1-pos) : pos;
                const int idx = (slot + rot) % NB;
                cudaEvent_t e0,e1; cudaEventCreate(&e0); cudaEventCreate(&e1);
                cudaEventRecord(e0);
                for (int r = 0; r < K; ++r) L(idx);
                cudaEventRecord(e1); ck(cudaEventSynchronize(e1),"sync");
                float ms=0; cudaEventElapsedTime(&ms,e0,e1);
                cudaEventDestroy(e0); cudaEventDestroy(e1);
                printf("OBS arm=%s group=%s config=%s mask=0x%012llx kbx=%d dyn=%zu pid=%d cycle=%d pos=%d us_per_kernel=%.6f\n",
                       arms[idx].name.c_str(), arms[idx].group.c_str(), arms[idx].config_sha.c_str(),
                       arms[idx].mask, (int) arms[idx].set.size(), arms[idx].seq.size(), pid, c, pos, 1000.0*ms/K);
                fflush(stdout);
            }
        }
        cudaFree(d_o);
        return 0;
    }
    return 0;
}
