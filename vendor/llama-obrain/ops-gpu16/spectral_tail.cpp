// spectral_tail.cpp — énergie de queue spectrale de matrices réelles d'un GGUF Q8_0 pur.
// 1) parse GGUF (header kv + tensor info) ; 2) charge les tenseurs demandés (déquant Q8_0) ;
// 3) top-256 valeurs propres de AᵀA par Lanczos avec réorthogonalisation complète ;
// 4) rapport : ||W||_F², énergie cumulée top-k (64/128/256), queue, PR.
// Usage : spectral_tail <model-q8.gguf> tensor... [> out]
#include "ggml.h"
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

struct TensorInfo { std::string name; uint32_t n_dims; uint64_t ne[4]; uint32_t type; uint64_t offset; };

static std::string rd_str(FILE * f) {
    uint64_t n; if (fread(&n, 8, 1, f) != 1) return "";
    std::string s((size_t) n, '\0'); if (n) fread(&s[0], 1, (size_t) n, f);
    return s;
}
static void skip_val(FILE * f, uint32_t t) {
    switch (t) {
        case 0: case 1: case 7: fseek(f, 1, SEEK_CUR); break;
        case 2: case 3: fseek(f, 2, SEEK_CUR); break;
        case 4: case 5: case 6: fseek(f, 4, SEEK_CUR); break;
        case 8: { uint64_t n; fread(&n, 8, 1, f); fseek(f, (long) n, SEEK_CUR); break; }
        case 10: case 11: case 12: fseek(f, 8, SEEK_CUR); break;
        case 9: { uint32_t et; fread(&et, 4, 1, f); uint64_t n; fread(&n, 8, 1, f);
                  for (uint64_t i = 0; i < n && i < 100000; ++i) skip_val(f, et); break; }
        default: break;
    }
}
static bool parse_gguf(const char * path, std::vector<TensorInfo> & out) {
    FILE * f = fopen(path, "rb"); if (!f) return false;
    char magic[4]; if (fread(magic, 1, 4, f) != 4 || memcmp(magic, "GGUF", 4)) { fclose(f); return false; }
    uint32_t ver; int64_t n_t, n_kv;
    fread(&ver, 4, 1, f); fread(&n_t, 8, 1, f); fread(&n_kv, 8, 1, f);
    for (int64_t i = 0; i < n_kv; ++i) { rd_str(f); uint32_t t; fread(&t, 4, 1, f); skip_val(f, t); }
    for (int64_t i = 0; i < n_t; ++i) {
        TensorInfo ti; ti.name = rd_str(f);
        fread(&ti.n_dims, 4, 1, f);
        for (uint32_t d = 0; d < ti.n_dims; ++d) fread(&ti.ne[d], 8, 1, f);
        fread(&ti.type, 4, 1, f); fread(&ti.offset, 8, 1, f);
        out.push_back(ti);
    }
    fclose(f); return true;
}
static inline float fp16_to_fp32(uint16_t h) {
    const uint32_t sign = (uint32_t)(h & 0x8000u) << 16;
    const unsigned e = (h >> 10) & 0x1f, m = h & 0x3ffu;
    uint32_t f;
    if (e == 0) {
        if (m == 0) f = sign;
        else { f = sign | 0x3f800000u; float base; memcpy(&base, &f, 4);
               return base * (float) m * 5.960464477539063e-8f; } // m * 2^-24
    } else if (e == 31) { f = sign | 0x7f800000u | (m << 13); }
    else { f = sign | ((e + 112u) << 23) | (m << 13); }
    float out; memcpy(&out, &f, 4); return out;
}
struct Mat { size_t n_rows, n_cols; std::vector<float> data; const float * row(size_t i) const { return &data[i * n_cols]; } };
static bool load_q8(FILE * f, const TensorInfo & ti, Mat & m) {
    m.n_cols = (size_t) ti.ne[0]; m.n_rows = (size_t) ti.ne[1];
    m.data.resize(m.n_rows * m.n_cols);
    fseek(f, (long) ti.offset, SEEK_SET);
    for (size_t r = 0; r < m.n_rows; ++r)
        for (size_t c0 = 0; c0 < m.n_cols; c0 += 32) {
            uint16_t d16; if (fread(&d16, 2, 1, f) != 1) return false;
            const float d = fp16_to_fp32(d16);
            int8_t qs[32]; if (fread(qs, 1, 32, f) != 32) return false;
            for (size_t j = 0; j < 32; ++j) m.data[r * m.n_cols + c0 + j] = d * (float) qs[j];
        }
    return true;
}

// Jacobi dense (validé dans la sonde Δ)
static void jacobi_eigh(double * A, int n, std::vector<double> & evals) {
    for (int sweep = 0; sweep < 500; ++sweep) {
        double off = 0.0;
        for (int i = 0; i < n; ++i) for (int j = i + 1; j < n; ++j) off += A[i * n + j] * A[i * n + j];
        if (off < 1e-28) break;
        for (int p = 0; p < n; ++p) for (int q = p + 1; q < n; ++q) {
            const double apq = A[p * n + q];
            if (std::fabs(apq) < 1e-300) continue;
            const double app = A[p * n + p], aqq = A[q * n + q];
            const double theta = 0.5 * std::atan2(2.0 * apq, aqq - app);
            const double c = std::cos(theta), s = std::sin(theta);
            for (int k = 0; k < n; ++k) {
                const double a1 = A[k * n + p], a2 = A[k * n + q];
                A[k * n + p] = c * a1 - s * a2; A[p * n + k] = A[k * n + p];
                A[k * n + q] = s * a1 + c * a2; A[q * n + k] = A[k * n + q];
            }
            A[p * n + p] = c * c * app - 2 * s * c * apq + s * s * aqq;
            A[q * n + q] = s * s * app + 2 * s * c * apq + c * c * aqq;
            A[p * n + q] = 0.0; A[q * n + p] = 0.0;
        }
    }
    evals.assign(n, 0.0);
    for (int i = 0; i < n; ++i) evals[i] = A[i * n + i];
    std::sort(evals.begin(), evals.end(), std::greater<double>());
    for (auto & e : evals) if (e < 0) e = 0;
}

int main(int argc, char ** argv) {
    if (argc < 3) { fprintf(stderr, "usage: %s model-q8.gguf manifest.tsv\n", argv[0]); return 2; }
    FILE * man = fopen(argv[2], "r");
    if (!man) { fprintf(stderr, "manifest introuvable\n"); return 1; }
    FILE * f = fopen(argv[1], "rb");
    char line[1024];
    while (fgets(line, sizeof line, man)) {
        char name[512]; unsigned long long rows, cols, off;
        if (sscanf(line, "%511s %llu %llu %llu", name, &rows, &cols, &off) != 4) continue;
        Mat m; m.n_rows = rows; m.n_cols = cols; m.data.resize(rows * cols);
        fseek(f, (long) off, SEEK_SET);
        bool ok = true;
        for (size_t r = 0; ok && r < rows; ++r)
            for (size_t c0 = 0; c0 < cols; c0 += 32) {
                uint16_t d16; if (fread(&d16, 2, 1, f) != 1) { ok = false; break; }
                const float d = fp16_to_fp32(d16);
                int8_t qs[32]; if (fread(qs, 1, 32, f) != 32) { ok = false; break; }
                for (size_t j = 0; j < 32; ++j) m.data[r * cols + c0 + j] = d * (float) qs[j];
            }
        if (!ok) { printf("%s: lecture échouée\n", name); continue; }
        const size_t n = m.n_cols, rr = m.n_rows;
        double frob2 = 0.0; for (float v : m.data) frob2 += (double) v * v;
        const int K = 256;
        std::vector<double> al(K), be(K);
        std::vector<std::vector<double>> V(K, std::vector<double>(n, 0.0));
        std::vector<double> v(n), w(n), vold(n);
        uint64_t seed = 0x9e3779b97f4a7c15ull;
        for (size_t i = 0; i < n; ++i) { seed = seed * 6364136223846793005ull + 1442695040888963407ull; v[i] = ((double)(seed >> 11) / 9007199254740992.0) - 0.5; }
        double nv = 0; for (double x : v) nv += x * x; nv = std::sqrt(nv); for (double & x : v) x /= nv;
        int Kk = 0;
        for (int k = 0; k < K; ++k) {
            std::vector<float> t(rr);
            for (size_t i = 0; i < rr; ++i) { const float * row = m.row(i); double s = 0; for (size_t j = 0; j < n; ++j) s += (double) row[j] * v[j]; t[i] = (float) s; }
            std::fill(w.begin(), w.end(), 0.0);
            for (size_t i = 0; i < rr; ++i) { const float * row = m.row(i); const double ti2 = (double) t[i]; for (size_t j = 0; j < n; ++j) w[j] += (double) row[j] * ti2; }
            for (int j = 0; j < k; ++j) { double d = 0; for (size_t i = 0; i < n; ++i) d += w[i] * V[j][i]; for (size_t i = 0; i < n; ++i) w[i] -= d * V[j][i]; }
            if (k > 0) { double d = 0; for (size_t i = 0; i < n; ++i) d += w[i] * vold[i]; for (size_t i = 0; i < n; ++i) w[i] -= d * vold[i]; }
            al[k] = 0; for (size_t i = 0; i < n; ++i) al[k] += w[i] * v[i];
            for (size_t i = 0; i < n; ++i) w[i] -= al[k] * v[i];
            double nw = 0; for (double x : w) nw += x * x; nw = std::sqrt(nw);
            be[k] = nw; V[k] = v; vold = v;
            if (nw < 1e-12) { Kk = k + 1; break; }
            for (size_t i = 0; i < n; ++i) v[i] /= nw;
            Kk = k + 1;
        }
        std::vector<double> T((size_t) Kk * Kk, 0.0);
        for (int i = 0; i < Kk; ++i) { T[i * Kk + i] = al[i]; if (i + 1 < Kk) T[i * Kk + i + 1] = T[(i + 1) * Kk + i] = be[i]; }
        std::vector<double> ev; jacobi_eigh(T.data(), Kk, ev);
        double cum64 = 0, cum128 = 0, cum256 = 0, cum = 0, pn = 0;
        for (int k = 0; k < (int) ev.size(); ++k) { cum += ev[k]; if (k < 64) cum64 = cum; if (k < 128) cum128 = cum; if (k < 256) cum256 = cum; pn += ev[k] * ev[k]; }
        auto fr = [&](double e) { return frob2 > 0 ? e / frob2 : 0.0; };
        printf("== %s (%zux%zu) ==\n", name, rr, n);
        printf("   ||W||_F²=%.6e | Σσ²(top%d)=%.6e (%.4f) | cum k64=%.4f k128=%.4f | queue(>128)=%.4f\n",
               frob2, Kk, cum, fr(cum), fr(cum64), fr(cum128), 1.0 - fr(cum128));
        printf("   σ1=%.4e σ32=%.4e σ64=%.4e σ128=%.4e | PR=%.2f\n",
               std::sqrt(ev[0]), std::sqrt(ev[31]), std::sqrt(ev[63]), std::sqrt(ev[127]),
               pn > 0 ? pn / (cum * cum) : 0);
        fflush(stdout);
    }
    fclose(man); fclose(f); return 0;
}
