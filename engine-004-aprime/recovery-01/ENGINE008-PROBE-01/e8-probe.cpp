// Sonde ISOLANT D2 (lecteur d'inventaire) et D3 (registre natif).
// Aucun forward : le registre exige le modele charge, pas de contexte.
#include "engine008_inventory.h"
#include "llama.h"
#include <cstdio>
#include <fstream>
#include <sstream>
#include <set>
#include <string>
#include <vector>
#include <utility>

// Meme declaration EXTERNE que le harnais (engine008_real_deps.cpp:47) : la
// fonction vit dans src/llama-model.h, pas dans llama.h.
extern const std::vector<std::pair<std::string, ggml_tensor *>> &
llama_internal_get_tensor_map(const llama_model * model);

int main(int argc, char ** argv) {
    if (argc < 3) { printf("usage: e8-probe <modele> <inventaire>\n"); return 2; }

    printf("=== D2 : LECTEUR D'INVENTAIRE, EN ISOLATION ===\n");
    printf("CHEMIN=%s\n", argv[2]);
    std::ifstream f(argv[2]);
    if (!f) { printf("FICHIER_NON_OUVERT=1\n"); }
    else {
        std::ostringstream ss; ss << f.rdbuf();
        const std::string bytes = ss.str();
        printf("OCTETS_LUS=%zu\n", bytes.size());
        InventoryRead ir = read_inventory_source(bytes, true);
        printf("N_LINES=%d HEADER_SEEN=%d SKIPPED_HEADER=%d SKIPPED_COMMENT=%d\n",
               ir.n_lines, (int) ir.header_seen, ir.n_skipped_header, ir.n_skipped_comment);
        printf("UNIQUE_NAMES=%zu NAMES=%zu DUPLICATES=%d CAPTURE_DISABLED=%d\n",
               ir.unique_names.size(), ir.names.size(), ir.n_duplicates,
               (int) ir.capture_disabled_by_duplicates);
        printf("WHY=%s\n", ir.why.empty() ? "<vide>" : ir.why.c_str());
        int i = 0; for (const auto & n : ir.unique_names) { if (i++ < 3) printf("  EXEMPLE=%s\n", n.c_str()); }
    }

    printf("\n=== MODELE : charge (sans contexte, sans forward) ===\n");
    llama_model_params mp = llama_model_default_params();
    llama_model * m = llama_model_load_from_file(argv[1], mp);
    if (m == nullptr) { printf("MODELE_NON_CHARGE=1\n"); return 3; }
    printf("MODELE_CHARGE=1\n");
    const auto & table = llama_internal_get_tensor_map(m);
    printf("TENSOR_MAP_SIZE=%zu\n", table.size());
    const int n_all   = (int) llama_model_n_layer(m);
    const int n_nextn = (int) llama_model_n_layer_nextn(m);
    printf("N_LAYER_ALL=%d N_LAYER_NEXTN=%d N_LAYER=%d\n", n_all, n_nextn, n_all - n_nextn);
    int shown = 0, n_blk = 0, n_tq2 = 0;
    for (const auto & kv : table) {
        if (kv.first.compare(0, 4, "blk.") == 0) n_blk++;
        if (kv.second != nullptr && kv.second->type == GGML_TYPE_TQ2_G128) n_tq2++;
        if (shown++ < 4) printf("  TENSOR=%s TYPE=%d\n", kv.first.c_str(),
                                (int) (kv.second ? kv.second->type : -1));
    }
    printf("TENSORS_blk=%d TENSORS_TQ2_G128=%d (sur %zu)\n", n_blk, n_tq2, table.size());
    printf("TQ2_G128_ENUM=%d\n", (int) GGML_TYPE_TQ2_G128);
    llama_model_free(m);
    return 0;
}
