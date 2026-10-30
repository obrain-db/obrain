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
};

PipelineDeps make_real_deps(RealState & s);

extern "C" {
extern long long g_real_load_calls;
extern long long g_real_ctx_calls;
extern long long g_real_prefill_calls;
extern long long g_real_decode_calls;
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
