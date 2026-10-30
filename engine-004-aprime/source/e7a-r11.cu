// ENGINE-007A-R10 — couche de qualification de la table d armes kbx.
//
// AUCUN TIMING DANS CE FICHIER. Deux modes :
//   mode "qual"  : noyau instrumentalise qui enregistre l ENSEMBLE kbx reellement execute
//   mode "gen"   : generation de la table d armes + simulation hote + auto-verification
//
// Le noyau mathematique est exactement celui valide en R9A/R9B (corps natif complet, boucle
// runtime, aucune garde a valeur nulle, aucun keepalive).

#include <cstdio>
#include <cstdlib>
#include <cstdint>
#include <cstring>
#include <cmath>
#include <string>
#include <vector>
#include <set>
#include <map>
#include <algorithm>
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

// ---- noyau naturel (reference et bras kbx) ----
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

// noyau de QUALIFICATION : enregistre (thread, kbx) reellement executes. Jamais chronometre.
__global__ void k_kbx_qual(const uint8_t * __restrict__ w, const int * __restrict__ y,
                           unsigned long long kbx_mask, int * __restrict__ exec_count) {
    const int tid = threadIdx.x;
    for (int kbx = tid / 4; kbx < NBLK; kbx += 32) {
        if ((kbx_mask & (1ull << kbx)) == 0ull) continue;
        atomicAdd(&exec_count[kbx], 1);           // une incrementation par thread executant ce bloc
        // consommation triviale pour empecher l elimination, sans modifier la semantique
        if (w == (const uint8_t *) 0x1) exec_count[64] += (int) y[0];
    }
}

// ---------------------------------------------------------------------------
// simulation hote du mapping natif
// ---------------------------------------------------------------------------
struct Arm {
    std::string name;
    std::string kernel;
    unsigned long long mask;
    std::vector<int> expected_set;
    std::vector<std::pair<int,int>> per_thread;   // (tid, kbx) dans l ordre natif
    bool is_replicate;
    std::string replicate_of;
    std::string config_sha;                       // hash canonique de configuration
};

static std::vector<int> native_sequence_for_thread(int tid) {
    std::vector<int> v;
    for (int kbx = tid / 4; kbx < NBLK; kbx += 32) v.push_back(kbx);
    return v;
}

static std::vector<int> mask_set(unsigned long long mask) {
    std::vector<int> v;
    for (int b = 0; b < 64; ++b) if (mask & (1ull << b)) v.push_back(b);
    return v;
}

// simulation : pour chaque thread, la sous-sequence de sa sequence native retenue par le masque
static std::vector<std::pair<int,int>> host_sequence(unsigned long long mask) {
    std::vector<std::pair<int,int>> seq;
    for (int tid = 0; tid < TPB; ++tid)
        for (int kbx : native_sequence_for_thread(tid))
            if (mask & (1ull << kbx)) seq.push_back({tid, kbx});
    return seq;
}

// repartition par warp : nombre de couples (thread, kbx) executes par warp
static std::vector<int> per_warp_counts(unsigned long long mask) {
    std::vector<int> w(TPB/32, 0);
    for (auto & p : host_sequence(mask)) w[p.first/32]++;
    return w;
}

// sous-ensembles : repartis sur le domaine natif, equilibres entre warps
static unsigned long long mask_from(const std::vector<int> & kbxs) {
    unsigned long long m = 0; for (int b : kbxs) m |= (1ull << b); return m;
}

static std::string set_str(const std::vector<int> & v) {
    std::string s; char buf[16];
    for (size_t i = 0; i < v.size(); ++i) { snprintf(buf,16,"%d",v[i]); if(i) s += ","; s += buf; }
    return s;
}
static std::string warp_str(const std::vector<int> & v) {
    std::string s; char buf[16];
    for (size_t i = 0; i < v.size(); ++i) { snprintf(buf,16,"%d",v[i]); if(i) s += "/"; s += buf; }
    return s;
}

int main(int argc, char ** argv) {
    const std::string mode = (argc > 1) ? argv[1] : "gen";
    const std::string table_tag = (argc > 5) ? argv[5] : "R10";
    const size_t W_BYTES = (size_t) ROWS * NBLK * sizeof(block_tq2_g128);
    const size_t NBLK_TOT = (size_t) ROWS * NBLK;

    // ---- generation saine (identique a R9B) ----
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
        const uint16_t sl = (uint16_t) (q->ds & 0xffffu);
        if (!std::isfinite(__half2float(*(const __half *) &sl))) actBad++;
    }
    const bool input_ok = (nanS==0 && infS==0 && zeroS==0 && actBad==0);

    // ---- table d armes ----
    std::vector<Arm> arms;
    auto add = [&](const char * nm, const char * kern, unsigned long long mask, bool rep, const char * repof) {
        Arm a; a.name = nm; a.kernel = kern; a.mask = mask;
        a.expected_set = mask_set(mask);
        a.per_thread = host_sequence(mask);
        a.is_replicate = rep; a.replicate_of = repof ? repof : "";
        arms.push_back(a);
    };

    const unsigned long long FULL = 0x0000FFFFFFFFFFFFull;         // 48 blocs = ensemble natif
    // KBX36 : 36 blocs repartis (on retire 12 blocs, un sur quatre dans la premiere moitie)
    std::vector<int> s36; for (int b = 0; b < 48; ++b) if (!(b < 12 && (b % 4) == 0)) s36.push_back(b);
    // KBX24_A : blocs pairs ; KBX24_B : blocs impairs
    std::vector<int> s24a, s24b; for (int b = 0; b < 48; ++b) { if (b % 2 == 0) s24a.push_back(b); else s24b.push_back(b); }
    // KBX12_A : b % 4 == 0 ; KBX12_B : b % 4 == 1
    std::vector<int> s12a, s12b, s12c; for (int b = 0; b < 48; ++b) { if (b%4==0) s12a.push_back(b); if (b%4==1) s12b.push_back(b); if (b%4==2) s12c.push_back(b); }
    // KBX08_A / KBX08_B : 8 blocs, repartis, equilibres
    std::vector<int> s08a, s08b; for (int b = 0; b < 48; ++b) { if (b%6==0) s08a.push_back(b); if (b%6==3) s08b.push_back(b); }
    // KBX06_A / KBX06_B
    std::vector<int> s06a, s06b; for (int b = 0; b < 48; ++b) { if (b%8==0) s06a.push_back(b); if (b%8==4) s06b.push_back(b); }

    // -- famille R11 en tete : KBX00, natif, plein runtime, puis cardinalites decroissantes --
    add("KBX00_EMPTY",   "_Z15k_kbx_runtimePKhPKiPf",        0ull,              false, "");
    add("K_FULL_NATIVE", "_Z13k_full_nativePKhPKiPf",        FULL,              false, "");
    add("KBX48_NATIVE",  "_Z15k_kbx_runtimePKhPKiPf",        FULL,              false, "");
    add("KBX36_SET_A",   "_Z15k_kbx_runtimePKhPKiPf",        mask_from(s36),    false, "");
    add("KBX24_SET_A",   "_Z15k_kbx_runtimePKhPKiPf",        mask_from(s24a),   false, "");
    add("KBX12_SET_A",   "_Z15k_kbx_runtimePKhPKiPf",        mask_from(s12a),   false, "");
    add("KBX06_SET_A",   "_Z15k_kbx_runtimePKhPKiPf",        mask_from(s06a),   false, "");
    // -- famille R10 complete, conservee pour le mode gen du harnais R10 --
    add("KBX24_SET_B",   "_Z15k_kbx_runtimePKhPKiPf",        mask_from(s24b),   false, "");
    add("KBX12_SET_A",   "_Z15k_kbx_runtimePKhPKiPf",        mask_from(s12a),   false, "");
    add("KBX12_SET_B",   "_Z15k_kbx_runtimePKhPKiPf",        mask_from(s12b),   false, "");
    add("KBX12_SET_C",   "_Z15k_kbx_runtimePKhPKiPf",        mask_from(s12c),   false, "");
    add("KBX08_SET_A",   "_Z15k_kbx_runtimePKhPKiPf",        mask_from(s08a),   false, "");
    add("KBX08_SET_B",   "_Z15k_kbx_runtimePKhPKiPf",        mask_from(s08b),   false, "");
    add("KBX06_SET_A",   "_Z15k_kbx_runtimePKhPKiPf",        mask_from(s06a),   false, "");
    add("KBX06_SET_B",   "_Z15k_kbx_runtimePKhPKiPf",        mask_from(s06b),   false, "");

    // ---- restriction a la famille R11 si demandee, AVANT tout calcul de gate ----
    if (table_tag == "R11") {
        std::vector<Arm> keep;
        const char * want[] = {"KBX00_EMPTY","K_FULL_NATIVE","KBX48_NATIVE","KBX36_SET_A",
                               "KBX24_SET_A","KBX12_SET_A","KBX06_SET_A"};
        for (const char * w : want) for (auto & a : arms) if (a.name == w) keep.push_back(a);
        arms = keep;
    }

    // ---- auto-verification : cardinalite, doublons, equilibre ----
    bool gates_ok = true;
    std::map<std::string, std::string> conf;      // config canonique -> nom d arme
    int dup = 0;
    for (auto & a : arms) {
        const int pc = __builtin_popcountll(a.mask);
        if ((int) a.expected_set.size() != pc) { printf("ERREUR cardinalite arme %s\n", a.name.c_str()); gates_ok = false; }
        std::string cfg = a.kernel + "|" + std::to_string(a.mask) + "|" + set_str(a.expected_set)
                        + "|" + std::to_string(a.per_thread.size());
        a.config_sha = cfg;
        if (conf.count(cfg)) { dup++; printf("DOUBLON_CONFIG arme=%s deja=%s\n", a.name.c_str(), conf[cfg].c_str()); }
        else conf[cfg] = a.name;
    }

    if (mode == "gen") {
        printf("ARM_TABLE_VERSION=1\n");
        printf("ARM_TABLE_KERNEL_MAPPING=native kbx = tid/4 + 32*i, i=0,1 ; 128 threads ; 48 blocs ; grid=(5120,1,1)\n");
        printf("ARM_TABLE_FORMAT=ARM_NAME|KERNEL_SYMBOL|MASK_HEX|KBX_COUNT|EXPECTED_KBX_SET|EXPECTED_DYNAMIC_KBX_COUNT|KBX_COUNT_PER_WARP|IS_REPLICATE|REPLICATE_OF\n");
        for (auto & a : arms) {
            printf("%s|%s|0x%012llx|%d|%s|%zu|%s|%s|%s\n",
                   a.name.c_str(), a.kernel.c_str(), a.mask, (int) a.expected_set.size(),
                   set_str(a.expected_set).c_str(), a.per_thread.size(),
                   warp_str(per_warp_counts(a.mask)).c_str(),
                   a.is_replicate ? "YES" : "NO", a.replicate_of.c_str());
        }
        printf("ARM_COUNT=%zu\n", arms.size());
        printf("UNEXPECTED_DUPLICATE_CONFIG_COUNT=%d\n", dup);
        printf("INPUT_SANITY_GATE=%s\n", input_ok ? "PASS" : "FAIL");
        printf("NAN_SCALE_COUNT=%ld\nINF_SCALE_COUNT=%ld\nSUBNORMAL_SCALE_COUNT=%ld\nZERO_SCALE_COUNT=%ld\nFINITE_SCALE_COUNT=%ld\n",
               nanS, infS, subS, zeroS, finiteS);
        fflush(stdout);
        return (gates_ok && dup==0) ? 0 : 3;
    }

    // ---- mode timing : campagne canonique ----
    if (mode == "time") {
        const int pid = (argc > 2) ? atoi(argv[2]) : 0;
        const int K   = (argc > 3) ? atoi(argv[3]) : 2048;
        const int NC  = (argc > 4) ? atoi(argv[4]) : 24;
        const int NARM = (argc > 6) ? atoi(argv[6]) : (int) arms.size();
        const int NB = (NARM > 0 && NARM <= (int) arms.size()) ? NARM : (int) arms.size();
        if (!input_ok) { printf("ABANDON_INPUT_SANITY_FAIL\n"); return 1; }

        uint8_t * d_w = nullptr; int * d_y = nullptr; float * d_o = nullptr;
        ck(cudaMalloc(&d_w, W_BYTES), "mw");
        ck(cudaMalloc(&d_y, (size_t) NBLK * sizeof(block_q8_1)), "my");
        ck(cudaMalloc(&d_o, (size_t) ROWS * sizeof(float)), "mo");
        ck(cudaMemcpy(d_w, h.data(), W_BYTES, cudaMemcpyHostToDevice), "cw");
        ck(cudaMemcpy(d_y, hy.data(), hy.size()*sizeof(int), cudaMemcpyHostToDevice), "cy");

        printf("R10_TIME_PROCESS_ID=%d K=%d N_CYCLES=%d N_ARMS=%zu N_MEASURED=%d\n", pid, K, NC, arms.size(), NB);
        printf("TQ2_BLOCK_COUNT=%zu\nFINITE_SCALE_COUNT=%ld\nNAN_SCALE_COUNT=%ld\nINF_SCALE_COUNT=%ld\nSUBNORMAL_SCALE_COUNT=%ld\nZERO_SCALE_COUNT=%ld\n",
               NBLK_TOT, finiteS, nanS, infS, subS, zeroS);
        printf("INPUT_SANITY_GATE=%s\n", input_ok ? "PASS" : "FAIL");
        for (auto & a : arms) printf("ARMCFG name=%s mask=0x%012llx kbx=%d dyn=%zu warps=%s\n",
               a.name.c_str(), a.mask, (int) a.expected_set.size(), a.per_thread.size(),
               warp_str(per_warp_counts(a.mask)).c_str());
        fflush(stdout);

        auto L = [&](size_t i) {
            if (arms[i].name == "K_FULL_NATIVE") k_full_native<<<ROWS,TPB>>>(d_w, d_y, d_o);
            else k_kbx_runtime<<<ROWS,TPB>>>(d_w, d_y, d_o, arms[i].mask);
        };

        // gate de sortie sur chaque bras
        { std::vector<float> chk(ROWS); int tot=0;
          for (size_t m = 0; m < arms.size(); ++m) {
            for (int r = 0; r < 8; ++r) L(m);
            ck(cudaDeviceSynchronize(), "chk");
            ck(cudaMemcpy(chk.data(), d_o, ROWS*sizeof(float), cudaMemcpyDeviceToHost), "cc");
            int nb=0; for (int i=0;i<ROWS;++i) if (!std::isfinite(chk[i])) nb++;
            printf("OUTPUT_NONFINITE_%-15s=%d\n", arms[m].name.c_str(), nb); tot += nb;
          }
          printf("OUTPUT_SANITY_GATE=%s\n", tot==0 ? "PASS" : "FAIL"); }
        fflush(stdout);

        for (int w = 0; w < 3; ++w) for (int m = 0; m < NB; ++m) for (int i = 0; i < K; ++i) L((size_t) m);
        ck(cudaDeviceSynchronize(), "warm");

        const int NA = (int) arms.size();
        for (int c = 0; c < NC; ++c) {
            const int rot = c / 2; const bool rev = (c % 2) == 1;
            for (int pos = 0; pos < NB; ++pos) {
                const int slot = rev ? (NB - 1 - pos) : pos;
                const int idx = (slot + rot) % NB;
                cudaEvent_t e0, e1; cudaEventCreate(&e0); cudaEventCreate(&e1);
                cudaEventRecord(e0);
                for (int r = 0; r < K; ++r) L(idx);
                cudaEventRecord(e1); ck(cudaEventSynchronize(e1), "sync");
                float ms = 0; cudaEventElapsedTime(&ms, e0, e1);
                cudaEventDestroy(e0); cudaEventDestroy(e1);
                const double us = 1000.0 * ms / K;
                printf("OBS arm=%s config=%s mask=0x%012llx kbx=%d dyn=%zu pid=%d cycle=%d pos=%d us_per_kernel=%.6f\n",
                       arms[idx].name.c_str(), arms[idx].config_sha.c_str(), arms[idx].mask,
                       (int) arms[idx].expected_set.size(), arms[idx].per_thread.size(), pid, c, pos, us);
                fflush(stdout);
            }
        }
        cudaFree(d_w); cudaFree(d_y); cudaFree(d_o);
        return 0;
    }

    // ---- mode qual : execution GPU instrumentee, sans chronometrage ----
    uint8_t * d_w = nullptr; int * d_y = nullptr; int * d_cnt = nullptr;
    ck(cudaMalloc(&d_w, W_BYTES), "mw");
    ck(cudaMalloc(&d_y, (size_t) NBLK * sizeof(block_q8_1)), "my");
    ck(cudaMalloc(&d_cnt, 65 * sizeof(int)), "mc");
    ck(cudaMemcpy(d_w, h.data(), W_BYTES, cudaMemcpyHostToDevice), "cw");
    ck(cudaMemcpy(d_y, hy.data(), hy.size()*sizeof(int), cudaMemcpyHostToDevice), "cy");

    printf("GPU_QUALIFICATION_VERSION=1\nINPUT_SANITY_GATE=%s\n", input_ok ? "PASS" : "FAIL");
    bool allok = true;
    for (auto & a : arms) {
        if (a.name == "K_FULL_NATIVE") continue;                 // pas de masque a verifier
        ck(cudaMemset(d_cnt, 0, 65*sizeof(int)), "clr");
        k_kbx_qual<<<1, TPB>>>(d_w, d_y, a.mask, d_cnt);         // un seul bloc : le mapping ne
        ck(cudaDeviceSynchronize(), "qual");                     // depend pas de la ligne
        std::vector<int> cnt(65);
        ck(cudaMemcpy(cnt.data(), d_cnt, 65*sizeof(int), cudaMemcpyDeviceToHost), "cc");
        std::vector<int> observed;
        for (int b = 0; b < 48; ++b) if (cnt[b] > 0) observed.push_back(b);
        long total = 0; for (int b = 0; b < 48; ++b) total += cnt[b];
        const bool setok = (observed == a.expected_set);
        const bool cntok = (total == (long) a.per_thread.size());
        // verification plus fine : le compte par bloc doit egaler le nombre de threads natifs l utilisant
        bool perblockok = true;
        for (int b = 0; b < 48; ++b) {
            const int native_threads = (b < 32) ? 4 : (b < 48 ? 4 : 0);
            if ((a.mask & (1ull<<b)) && cnt[b] != native_threads) perblockok = false;
            if (!(a.mask & (1ull<<b)) && cnt[b] != 0) perblockok = false;
        }
        printf("ARM=%s GPU_OBSERVED_KBX_SET=%s GPU_OBSERVED_DYNAMIC_KBX_COUNT=%ld SET_MATCH=%s COUNT_MATCH=%s PERBLOCK_MATCH=%s\n",
               a.name.c_str(), set_str(observed).c_str(), total, setok?"YES":"NO", cntok?"YES":"NO", perblockok?"YES":"NO");
        if (!(setok && cntok && perblockok)) allok = false;
    }
    printf("GPU_KBX_QUALIFICATION=%s\n", allok ? "PASS" : "FAIL");
    cudaFree(d_w); cudaFree(d_y); cudaFree(d_cnt);
    return allok ? 0 : 4;
}
