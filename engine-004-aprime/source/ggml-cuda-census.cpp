// ENGINE-005H : rapport du recensement par SIGNATURE (compile dans libggml-cuda).
#include "ggml-cuda-census.cuh"

#include <string>
#include <vector>
#include <algorithm>

struct row {
    uint64_t     count;
    std::string  name;
    uint32_t     qtype;
    int          pred;
    int          path;
    int64_t      d[6];
};

static std::vector<row> collect() {
    ggml_census_state & st = ggml_census();
    std::vector<row> v;
    for (int i = 0; i < ggml_census_state::CAP; ++i) {
        const uint64_t k = st.key[i].load(std::memory_order_relaxed);
        if (k == 0) {
            continue;
        }
        const uint64_t n = st.count[i].load(std::memory_order_relaxed);
        const uint32_t m = st.meta[i].load(std::memory_order_relaxed);
        row r;
        r.count = n;
        r.name  = st.name[i][0] ? std::string(st.name[i]) : std::string("<sans nom>");
        r.qtype = m & 0xFFFF;
        r.pred  = (m >> 16) & 0xFF;
        r.path  = (m >> 24) & 0xFF;
        for (int j = 0; j < 6; ++j) {
            r.d[j] = st.dims[i][j];
        }
        v.push_back(r);
    }
    return v;
}

extern "C" void ggml_census_dump_now(void) {
    if (!ggml_census_enabled()) {
        return;
    }
    std::vector<row> v = collect();
    std::sort(v.begin(), v.end(), [](const row & a, const row & b) {
        if (a.name != b.name) { return a.name < b.name; }
        return a.count > b.count;
    });

    std::fprintf(stderr, "\n[E5H] DOMAINE DE DISPATCH EXACT (predicats reels + signature)\n");
    std::fprintf(stderr, "SIGNATURE_ROWS=%zu\n", v.size());
    std::fprintf(stderr, "  %-34s %8s %-6s %-5s %-6s %-8s %s\n",
                 "poids", "appels", "qtype", "path", "pred", "ne00xne01", "sig");
    for (const row & r : v) {
        char pred[5];
        pred[0] = (r.pred & 1) ? 'V' : '.';   // can_mmvf
        pred[1] = (r.pred & 2) ? 'F' : '.';   // can_mmf
        pred[2] = (r.pred & 4) ? 'Q' : '.';   // can_mmvq
        pred[3] = (r.pred & 8) ? 'M' : '.';   // can_mmq
        pred[4] = '\0';
        std::fprintf(stderr, "  %-34s %8llu %-6u %-5s %-6s %lldx%lld %lld,%lld,%lld,%lld s1=%lld,%lld\n",
                     r.name.c_str(), (unsigned long long) r.count, r.qtype,
                     ggml_census_path_name(r.path), pred, r.d[0], r.d[1],
                     r.d[0], r.d[1], r.d[2], r.d[3], r.d[4], r.d[5]);
    }
    std::fflush(stderr);
}

// Agregat par CHEMIN (conservé de 005G).
extern "C" void ggml_census_totals(void) {
    if (!ggml_census_enabled()) {
        return;
    }
    std::vector<row> v = collect();
    uint64_t tot[GGML_CENSUS_PATH_N] = {0};
    uint64_t all = 0;
    for (const row & r : v) {
        if (r.path >= 0 && r.path < GGML_CENSUS_PATH_N) { tot[r.path] += r.count; }
        all += r.count;
    }
    std::fprintf(stderr, "CENSUS_DISPATCH_SELECTIONS_TOTAL=%llu\n", (unsigned long long) all);
    for (int p = 0; p < GGML_CENSUS_PATH_N; ++p) {
        std::fprintf(stderr, "CENSUS_%s_SELECTED_TOTAL=%llu\n", ggml_census_path_name(p),
                     (unsigned long long) tot[p]);
    }
    std::fflush(stderr);
}
