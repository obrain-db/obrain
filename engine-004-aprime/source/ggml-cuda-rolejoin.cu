// ENGINE-005G-R2 : jointure role <-> chemin de dispatch (meme processus, cle = pointeur de poids).
#include "ggml-cuda-census.cuh"
#include "ggml-cuda-provenance.cuh"

#include <map>
#include <string>
#include <vector>
#include <algorithm>

extern "C" int ggml_census_role_report(void) {
    if (!ggml_census_enabled()) {
        return 0;
    }
    ggml_census_state &   cs = ggml_census();
    ggml_prov_state  &   pv = ggml_prov();

    // role -> (chemin -> compteur)
    std::map<std::string, std::map<int, unsigned long long>> by_role;
    unsigned long long total = 0, joined = 0;
    int unjoined_ptrs = 0;

    for (int i = 0; i < ggml_census_state::CAP; ++i) {
        const uint64_t k = cs.key[i].load(std::memory_order_relaxed);
        if (k == 0) {
            continue;
        }
        const int      path = (int) (k & 0xFF) - 1;
        const void *   wptr = (const void *) (uintptr_t) (k >> 8);
        const uint64_t n    = cs.count[i].load(std::memory_order_relaxed);
        total += n;

        auto it = pv.weight_role_of_ptr.find(wptr);
        if (it == pv.weight_role_of_ptr.end()) {
            unjoined_ptrs++;
            by_role["<unjoined>"][path] += n;
            continue;
        }
        joined += n;
        by_role[it->second][path] += n;
    }

    std::fprintf(stderr, "\n[E5G-R2] DISPATCH PAR ROLE DE POIDS (jointure pointeur, meme processus)\n");
    std::fprintf(stderr, "  %-46s %8s %8s %8s %8s %8s %8s\n",
                 "role", "MMVF", "MMF", "MMVQ", "MMQ", "OTHER", "total");
    std::vector<std::pair<unsigned long long, std::string>> ord;
    for (const auto & kv : by_role) {
        unsigned long long t = 0;
        for (const auto & p : kv.second) {
            t += p.second;
        }
        ord.emplace_back(t, kv.first);
    }
    std::sort(ord.rbegin(), ord.rend());
    for (const auto & e : ord) {
        const std::map<int, unsigned long long> & m = by_role[e.second];
        auto g = [&](int p) -> unsigned long long {
            auto it = m.find(p);
            return it == m.end() ? 0ull : it->second;
        };
        std::fprintf(stderr, "  %-46s %8llu %8llu %8llu %8llu %8llu %8llu\n",
                     e.second.c_str(), g(GGML_CENSUS_MMVF), g(GGML_CENSUS_MMF), g(GGML_CENSUS_MMVQ),
                     g(GGML_CENSUS_MMQ), g(GGML_CENSUS_OTHER), e.first);
    }
    std::fprintf(stderr, "ROLE_KEYED_DISPATCH_TOTAL=%llu\n", joined);
    std::fprintf(stderr, "GLOBAL_DISPATCH_TOTAL=%llu\n", total);
    std::fprintf(stderr, "UNJOINED_DISPATCH_COUNT=%llu\n", total - joined);
    std::fprintf(stderr, "UNJOINED_DISPATCH_PERCENT=%.4f\n",
                 total > 0 ? 100.0 * (double) (total - joined) / (double) total : 0.0);
    std::fprintf(stderr, "UNJOINED_WEIGHT_POINTERS=%d\n", unjoined_ptrs);
    std::fflush(stderr);
    return 0;
}
