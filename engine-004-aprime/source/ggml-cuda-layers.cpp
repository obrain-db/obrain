// ENGINE-005C : definition unique de l'etat + du vidage (compile dans libggml-cuda).
#include "ggml-cuda-layers.cuh"

ggml_layer_state ggml_layer_state_g;

extern "C" bool ggml_layer_timing_enabled(void) {
    static const bool on = getenv("GGML_LAYER_TIMING") != nullptr;
    return on;
}

extern "C" void ggml_layer_t_dump_now(void) {
    if (!ggml_layer_timing_enabled()) {
        return;
    }
    ggml_layer_set_attention_from_env();

    ggml_layer_state & st = ggml_layer_state_g;
    if (st.layer.empty()) {
        std::fprintf(stderr, "\n[E5C] ETAT VIDE : aucune operation accumulee\n");
        return;
    }

    std::vector<int> keys;
    for (const auto & kv : st.layer) {
        keys.push_back(kv.first);
    }
    std::sort(keys.begin(), keys.end());

    double total = 0.0;
    unsigned long long calls = 0;
    for (int k : keys) {
        total += st.layer[k].ms;
        calls += st.layer[k].n;
    }

    std::fprintf(stderr, "\n[E5C] ATTRIBUTION PAR COUCHE — %.1f ms cumulees, %llu operations, %llu calculs\n",
                 total, (unsigned long long) calls, (unsigned long long) st.tokens);
    std::fprintf(stderr, "  %-6s %-10s %12s %8s %10s %11s %12s\n",
                 "layer", "classe", "ms_total", "%", "appels", "us/op", "MUL_MAT %");

    double rec_ms = 0.0, att_ms = 0.0, rec_mm = 0.0, att_mm = 0.0;
    int n_rec = 0, n_att = 0;

    for (int k : keys) {
        const ggml_layer_rec & r = st.layer[k];
        const bool is_att = (k >= 0) && ggml_layer_is_attention(k);
        const char * cls = (k < 0) ? "hors" : (is_att ? "attention" : "recurrent");
        double mm = 0.0;
        for (const auto & kv : r.op_ms) {
            if (kv.first == "MUL_MAT") {
                mm = kv.second;
            }
        }
        std::fprintf(stderr, "  %-6d %-10s %12.2f %7.2f%% %10llu %11.2f %11.1f%%\n",
                     k, cls, r.ms, 100.0 * r.ms / total, (unsigned long long) r.n,
                     1000.0 * r.ms / (double) r.n, 100.0 * mm / r.ms);
        if (k >= 0) {
            if (is_att) { att_ms += r.ms; att_mm += mm; n_att++; }
            else        { rec_ms += r.ms; rec_mm += mm; n_rec++; }
        }
    }

    std::fprintf(stderr, "\n[E5C] AGREGATION\n");
    std::fprintf(stderr, "RECURRENT_LAYER_COUNT=%d\nATTENTION_LAYER_COUNT=%d\n", n_rec, n_att);
    std::fprintf(stderr, "RECURRENT_LAYER_TOTAL_MS=%.2f\n", rec_ms);
    std::fprintf(stderr, "ATTENTION_LAYER_TOTAL_MS=%.2f\n", att_ms);
    std::fprintf(stderr, "RECURRENT_LAYER_PERCENT=%.2f\n", 100.0 * rec_ms / total);
    std::fprintf(stderr, "ATTENTION_LAYER_PERCENT=%.2f\n", 100.0 * att_ms / total);
    std::fprintf(stderr, "RECURRENT_MUL_MAT_SHARE=%.2f\n", rec_ms > 0 ? 100.0 * rec_mm / rec_ms : 0.0);
    std::fprintf(stderr, "ATTENTION_MUL_MAT_SHARE=%.2f\n", att_ms > 0 ? 100.0 * att_mm / att_ms : 0.0);
    std::fprintf(stderr, "LAYER_SUM_TOTAL_MS=%.2f\n", total);
    std::fprintf(stderr, "HORS_COUCHE_MS=%.2f\n", st.layer.count(-1) ? st.layer[-1].ms : 0.0);

    // [ENGINE-005C bis] repartition par OPERATION dans chaque classe de couche, et couverture
    // du nommage : combien d'operations portent un indice de couche.
    std::fprintf(stderr, "\n[E5C] OPERATIONS PAR CLASSE DE COUCHE\n");
    {
        std::map<std::string, double> att_op, rec_op;
        for (const auto & kv : st.layer) {
            const int k = kv.first;
            if (k < 0) { continue; }
            const bool is_att = ggml_layer_is_attention(k);
            for (const auto & o : kv.second.op_ms) {
                (is_att ? att_op : rec_op)[o.first] += o.second;
            }
        }
        std::fprintf(stderr, "  %-20s %14s %14s\n", "operation", "attention_ms", "recurrent_ms");
        std::vector<std::string> keys;
        for (const auto & kv : att_op) { keys.push_back(kv.first); }
        for (const auto & kv : rec_op) {
            if (att_op.find(kv.first) == att_op.end()) { keys.push_back(kv.first); }
        }
        std::sort(keys.begin(), keys.end());
        for (const auto & k : keys) {
            std::fprintf(stderr, "  %-20s %14.2f %14.2f\n", k.c_str(), att_op[k], rec_op[k]);
        }
    }
    {
        unsigned long long att_n = 0, rec_n = 0, hors_n = 0;
        for (const auto & kv : st.layer) {
            if (kv.first < 0) { hors_n += kv.second.n; }
            else if (ggml_layer_is_attention(kv.first)) { att_n += kv.second.n; }
            else { rec_n += kv.second.n; }
        }
        const unsigned long long tot = att_n + rec_n + hors_n;
        std::fprintf(stderr, "\n[E5C] COUVERTURE DU NOMMAGE\n");
        std::fprintf(stderr, "OPS_TOTAL=%llu OPS_ATTENTION=%llu OPS_RECURRENT=%llu OPS_HORS_COUCHE=%llu\n",
                     (unsigned long long) tot, (unsigned long long) att_n, (unsigned long long) rec_n,
                     (unsigned long long) hors_n);
        std::fprintf(stderr, "NODE_LAYER_TAGGING_COVERAGE_PERCENT=%.2f\n",
                     tot > 0 ? 100.0 * (double) (att_n + rec_n) / (double) tot : 0.0);
    }

    std::fprintf(stderr, "\n[E5C] MUL_MAT PAR COUCHE (top 12)\n");
    std::vector<std::pair<double, int>> v;
    for (int k : keys) {
        auto it = st.layer[k].op_ms.find("MUL_MAT");
        if (it != st.layer[k].op_ms.end()) {
            v.emplace_back(it->second, k);
        }
    }
    std::sort(v.rbegin(), v.rend());
    for (size_t i = 0; i < v.size() && i < 12; ++i) {
        const int k = v[i].second;
        const ggml_layer_rec & r = st.layer[k];
        std::fprintf(stderr, "  layer %-3d %-10s MUL_MAT %10.2f ms  appels %5llu  us/op %8.2f\n",
                     k, (k >= 0 && ggml_layer_is_attention(k)) ? "attention" : "recurrent",
                     v[i].first, (unsigned long long) r.op_n.at("MUL_MAT"),
                     1000.0 * v[i].first / (double) r.op_n.at("MUL_MAT"));
    }
    std::fflush(stderr);
}
