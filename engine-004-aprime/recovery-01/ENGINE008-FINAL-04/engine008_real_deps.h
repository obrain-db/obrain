// ENGINE-008 -- PREFLIGHT-REAL-ADAPTERS-06 : etat partage et compteurs.
// UNE SEULE definition de RealState : deux dispositions differentes seraient
// une violation d'ODR et un comportement indefini.
#pragma once
#include "engine008_pipeline.h"
#include "engine008_capture_parser.h"
#include "llama.h"
#include <string>
#include <vector>

struct RealState {
    llama_model *      model = nullptr;
    llama_context *    ctx   = nullptr;
    std::vector<float> logits_buf;
    std::string        capture_dir;
    WindowCursor       cursor;                 // releve AVANT, arme par prepare_capture
    std::string        ledger_digest_seen;     // detection de troncature/replacement
    const void *       last_gf = nullptr;      // dernier graphe soumis observe (f1a)
    llama_pos          n_past  = 0;            // H1 : position de continuation
    std::vector<NativeTensor> registry_tensors; // I1/I2 : registre natif REEL
    std::string        inventory_path;
    std::string        inventory_sha;  // SHA de l'inventaire REELLEMENT lu         // I2 : OBRAIN_F1A_INVENTORY
    bool               registry_acquired = false;
    bool               p4_applies_role = false;  // P4' : BASELINE seulement
};

PipelineDeps make_real_deps(RealState & s);
std::string inventory_path_from_env();
std::string libllama_sha256_loaded();

extern "C" {
extern long long g_real_load_calls;
extern long long g_real_ctx_calls;
extern long long g_real_prefill_calls;
extern long long g_real_decode_calls;
extern long long g_real_decode_ok;
extern long long g_real_prefill_attempts;
extern long long g_real_prefill_ok;
extern long long g_real_release_calls;
extern long long g_real_f1a_env_calls;
extern long long g_real_enum_calls;
extern long long g_real_registry_calls;
}

// Recalcul de la definition EXACTE du producteur (f1a-capture.cpp:263-281) :
//   FNV-1a 64 bits, depart 1469598103934665603
//   par noeud i : x = (uintptr_t)t ^ ((uint64)t->op << 48) ; fp = (fp ^ x) * 1099511628211
//   puis pour k in [0, GGML_MAX_SRC) : si src[k] != null, fp = (fp ^ (uintptr_t)src[k]) * prime
//   les noeuds nuls et les sources nulles sont IGNORES
unsigned long long recompute_graph_fingerprint(const void * gf);
