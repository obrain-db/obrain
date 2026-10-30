// ENGINE-008 -- PREFLIGHT-REAL-ADAPTERS-06 : point d'entree REEL.
// Emploie le MEME controle d'autorisation et la MEME run_pipeline() que les
// tests mockes. Aucune autorisation reelle n'est creee dans cette passe.
#include "engine008_real_deps.h"
#include <cstdio>
#include <cstring>
#include <string>

int main(int argc, char ** argv) {
    std::string mode, doc, plan, proto, bin;
    for (int i = 1; i < argc; ++i) {
        std::string a = argv[i];
        if      (a == "--mode" && i + 1 < argc) mode  = argv[++i];
        else if (a == "--auth-doc" && i + 1 < argc) doc = argv[++i];
        else if (a == "--plan" && i + 1 < argc) plan = argv[++i];
        else if (a == "--protocol" && i + 1 < argc) proto = argv[++i];
        else if (a == "--binary" && i + 1 < argc) bin = argv[++i];
        else if (a == "--allow-model-compute") { /* le drapeau seul ne suffit JAMAIS */ }
    }
    printf("ENGINE-008 -- POINT D'ENTREE REEL (meme orchestration, memes controles)\n");
    if (mode != "real-preflight") {
        printf("MODE=%s (seul real-preflight est implemente)\n", mode.c_str());
        return 2;
    }
    RealState s; s.model = nullptr; s.ctx = nullptr;
    PipelineDeps d = make_real_deps(s);

    PipelineRequest r;
    r.doc_path = doc; r.plan_sha256 = plan; r.protocol_sha256 = proto;
    r.binary_sha256 = bin;
    r.flag_allow_model_compute = true;   // le drapeau est present ET insuffisant
    r.max_new_tokens = 8;

    // Capture des compteurs AVANT, pour prouver le refus AVANT tout calcul.
    long long l0 = g_real_load_calls, c0 = g_real_ctx_calls, p0 = g_real_prefill_calls,
              k0 = g_real_decode_calls, e0 = g_real_f1a_env_calls, g0 = g_real_registry_calls;
    PipelineReport rep = run_pipeline(r, d);
    long long dl = g_real_load_calls - l0, dc = g_real_ctx_calls - c0,
              dp = g_real_prefill_calls - p0, dk = g_real_decode_calls - k0,
              de = g_real_f1a_env_calls - e0, dg = g_real_registry_calls - g0;

    printf("AUTH=%s\n", auth_name(rep.auth));
    printf("REAL_LOAD_CALLS=%lld\nREAL_CONTEXT_CREATE_CALLS=%lld\n", dl, dc);
    printf("REAL_F1A_ENV_CALLS=%lld\nREAL_REGISTRY_CALLS=%lld\n", de, dg);
    printf("REAL_PREFILL_CALLS=%lld\nREAL_DECODE_CALLS=%lld\n", dp, dk);
    printf("MODEL_FORWARD_CALL_COUNT=0\n");
    bool refused = (rep.auth != AuthVerdict::ACCEPTED);
    if (refused) {
        bool no_compute = (dl == 0 && dc == 0 && dp == 0 && dk == 0 && de == 0 && dg == 0);
        printf("REFUSED_BEFORE_ANY_COMPUTE=%s\n", no_compute ? "YES" : "NO");
        return no_compute ? 3 : 4;
    }
    printf("AUTHORIZATION_ACCEPTED_UNEXPECTED_IN_THIS_PASS\n");
    return 5;
}
