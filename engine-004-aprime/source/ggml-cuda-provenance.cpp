// ENGINE-005D : rapports de provenance (compile dans libggml-cuda).
#include "ggml-cuda-provenance.cuh"

static bool prov_is_attention(int layer) {
    // architecture : full_attention_interval = 4 sur 64 couches -> attention si il % 4 == 3
    return layer >= 0 && (layer % 4) == 3;
}

extern "C" void ggml_prov_dump_now(void) {
    if (!ggml_prov_enabled()) {
        return;
    }
    ggml_prov_state & st = ggml_prov();

    std::fprintf(stderr, "\n[E5D] PROVENANCE DE CONSTRUCTION\n");
    std::fprintf(stderr, "LAYER_OUTPUT_RECORDED_COUNT=%zu\n", st.layer_out.size());
    std::fprintf(stderr, "LAYER_OUTPUT_ANCHOR_FOUND_COUNT=%zu/%zu\n",
                 st.layer_out_node_index.size(), st.layer_out.size());

    bool mono = true;
    int  prev = -1;
    for (const auto & kv : st.layer_out_node_index) {
        if (kv.second <= prev) { mono = false; }
        prev = kv.second;
    }
    std::fprintf(stderr, "LAYER_OUTPUT_TOPOLOGICAL_ORDER=%s\n", mono ? "PASS" : "FAIL");

    // attribution du graphe complet
    unsigned long long attributed = 0;
    for (int v : st.node_layer) {
        if (v >= 0) { attributed++; }
    }
    std::fprintf(stderr, "GRAPH_NODE_COUNT=%zu\n", st.node_layer.size());
    std::fprintf(stderr, "GRAPH_NODE_LAYER_ATTRIBUTED_COUNT=%llu\n", (unsigned long long) attributed);
    std::fflush(stderr);
}

extern "C" void ggml_prov_layer_report(void) {
    if (!ggml_prov_enabled()) {
        return;
    }
    ggml_prov_state & st = ggml_prov();
    if (st.layer_ms.empty()) {
        std::fprintf(stderr, "[E5D] aucun temps par couche accumule\n");
        return;
    }

    double total = 0.0, att = 0.0, rec = 0.0, post = 0.0;
    double att_mm = 0.0, rec_mm = 0.0;
    for (const auto & kv : st.layer_ms) {
        total += kv.second;
        if (kv.first < 0) { post += kv.second; continue; }
        const bool is_att = prov_is_attention(kv.first);
        if (is_att) { att += kv.second; } else { rec += kv.second; }
        auto it = st.layer_op_ms.find(std::to_string(kv.first) + "|MUL_MAT");
        if (it != st.layer_op_ms.end()) {
            if (is_att) { att_mm += it->second; } else { rec_mm += it->second; }
        }
    }

    std::fprintf(stderr, "\n[E5D] TEMPS PAR COUCHE — provenance de construction\n");
    std::fprintf(stderr, "LAYER_SUM_TOTAL_MS=%.2f\n", total);
    std::fprintf(stderr, "RECURRENT_TOTAL_MS=%.2f\n", rec);
    std::fprintf(stderr, "ATTENTION_TOTAL_MS=%.2f\n", att);
    std::fprintf(stderr, "POST_MODEL_TOTAL_MS=%.2f\n", post);
    std::fprintf(stderr, "RECURRENT_TOTAL_PERCENT=%.2f\n", 100.0 * rec / total);
    std::fprintf(stderr, "ATTENTION_TOTAL_PERCENT=%.2f\n", 100.0 * att / total);
    std::fprintf(stderr, "RECURRENT_MUL_MAT_MS=%.2f\n", rec_mm);
    std::fprintf(stderr, "ATTENTION_MUL_MAT_MS=%.2f\n", att_mm);
    std::fprintf(stderr, "RECURRENT_MUL_MAT_SHARE=%.2f\n", rec > 0 ? 100.0 * rec_mm / rec : 0.0);
    std::fprintf(stderr, "ATTENTION_MUL_MAT_SHARE=%.2f\n", att > 0 ? 100.0 * att_mm / att : 0.0);

    // verification de couverture des MUL_MAT executes
    double mm_att = 0.0, mm_rec = 0.0, mm_post = 0.0, mm_unknown = 0.0;
    for (const auto & kv : st.layer_op_ms) {
        const size_t bar = kv.first.find('|');
        const int layer = atoi(kv.first.substr(0, bar).c_str());
        if (kv.first.substr(bar + 1) != "MUL_MAT") { continue; }
        if (layer == -1) { mm_unknown += kv.second; }
        else if (layer == -2) { mm_post += kv.second; }
        else if (prov_is_attention(layer)) { mm_att += kv.second; }
        else { mm_rec += kv.second; }
    }
    const double mm_tot = mm_att + mm_rec + mm_post + mm_unknown;
    std::fprintf(stderr, "EXECUTED_MUL_MAT_MS_TOTAL=%.2f\n", mm_tot);
    std::fprintf(stderr, "EXECUTED_MUL_MAT_LAYER_COVERAGE_PERCENT=%.2f\n",
                 mm_tot > 0 ? 100.0 * (mm_att + mm_rec + mm_post) / mm_tot : 0.0);
    std::fprintf(stderr, "EXECUTED_MUL_MAT_UNKNOWN_LAYER_MS=%.2f\n", mm_unknown);
    std::fflush(stderr);
}

extern "C" void ggml_prov_role_report(void) {
    if (!ggml_prov_enabled()) {
        return;
    }
    ggml_prov_state & st = ggml_prov();
    std::vector<std::pair<double, std::string>> v;
    double tot = 0.0;
    for (const auto & kv : st.mm_role) {
        v.emplace_back(kv.second.ms, kv.first);
        tot += kv.second.ms;
    }
    std::sort(v.rbegin(), v.rend());
    std::fprintf(stderr, "\n[E5D] MUL_MAT PAR ROLE DE POIDS (temps cumule, tous tokens)\n");
    std::fprintf(stderr, "  %-62s %10s %10s %9s %9s\n", "classe|role|type", "ms", "part", "appels", "M|N|K");
    double cum = 0.0;
    for (const auto & e : v) {
        const ggml_prov_state::mm_rec & r = st.mm_role[e.second];
        cum += e.first;
        std::fprintf(stderr, "  %-62s %10.1f %8.2f%% %9llu %lld|%lld|%lld\n", e.second.c_str(), e.first,
                     tot > 0 ? 100.0 * e.first / tot : 0.0, (unsigned long long) r.calls, r.M, r.N, r.K);
        if (cum / (tot > 0 ? tot : 1.0) > 0.90) {
            std::fprintf(stderr, "  --- couverture 90%% atteinte a cette ligne ---\n");
        }
    }
    std::fprintf(stderr, "MUL_MAT_ROLE_TOTAL_MS=%.2f\n", tot);
    std::fflush(stderr);
}

extern "C" void ggml_prov_signatures_top(int n) {
    if (!ggml_prov_enabled()) {
        return;
    }
    ggml_prov_state & st = ggml_prov();
    std::vector<std::pair<unsigned long long, std::string>> v;
    for (const auto & kv : st.mm_sig) {
        v.emplace_back(kv.second.calls, kv.first);
    }
    std::sort(v.rbegin(), v.rend());
    std::fprintf(stderr, "\n[E5D] TOP SIGNATURES MUL_MAT (par nombre d'appels)\n");
    std::fprintf(stderr, "  %-70s %10s %10s %12s\n", "couche|role|type|M|N|K", "appels", "M", "N");
    for (size_t i = 0; i < v.size() && (int) i < n; ++i) {
        const ggml_prov_state::mm_rec & r = st.mm_sig[v[i].second];
        std::fprintf(stderr, "  %-70s %10llu %10lld %12lld\n", v[i].second.c_str(),
                     (unsigned long long) r.calls, r.M, r.N);
    }
    std::fflush(stderr);
}
