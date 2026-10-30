// ENGINE-008 -- PREFLIGHT-REAL-ADAPTERS-06 : etat partage et compteurs.
// UNE SEULE definition de RealState : deux dispositions differentes seraient
// une violation d'ODR et un comportement indefini.
#pragma once
#include "engine008_pipeline.h"
#include "llama.h"
#include <string>
#include <vector>

struct RealState {
    llama_model *      model = nullptr;
    llama_context *    ctx   = nullptr;
    std::vector<float> logits_buf;
    std::string        capture_dir;
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
