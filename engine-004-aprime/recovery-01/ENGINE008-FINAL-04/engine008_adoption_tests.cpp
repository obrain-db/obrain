// ENGINE-008 -- tests d'ADOPTION de P4' et de sa GATE.
// P4' est ADOPTEE : la gate consomme les six conditions. Un FAIL ou un UNKNOWN
// BLOQUE. Aucun test ne transforme UNKNOWN en PASS.
#include "engine008_pipeline.h"
#include "engine008_p4_checks.h"
#include <cstdio>

static int failed = 0;
static void ck(const char * n, bool ok) { printf("  %-66s %s\n", n, ok?"PASS":"FAIL"); if(!ok) failed++; }
static bool has(const std::string & h, const std::string & n) { return h.find(n)!=std::string::npos; }

static P4Inputs all_pass_inputs() {
    P4Inputs in;
    in.model_loaded=true; in.registry_acquired=true; in.registry_complete=true; in.n_tensors=500;
    in.types_read_from_tensors=true; in.n_types_read=500; in.type_inferred_from_name=false;
    in.n_relevant_weights=168; in.n_relevant_tq2g128=0;
    in.wht_sites_audited=true; in.n_wht_creation_sites=1; in.wht_sites={"llama-graph.cpp:1497"};
    in.libllama_sha256="87c42d5f"; in.expected_libllama_sha256="87c42d5f";
    in.obrain_no_rotation_present=false; in.ctx_type="LLAMA_CONTEXT_TYPE_DEFAULT";
    in.gtype_status="GTYPE_DERIVED_FROM_VERIFIED_SOURCE_MAPPING";
    return in;
}
static PipelineDeps deps(P4Inputs in) {
    PipelineDeps d;
    d.authorize = [](const PipelineRequest &) { return AuthVerdict::ACCEPTED; };
    d.prepare_capture = [](const std::string &, const std::string & m, const std::string & r,
                           std::string & o, std::string &) { o="/tmp/ad/"+m+"/"+r; return true; };
    d.load_full = [](const std::string &) -> void * { return (void*)0x1; };
    d.acquire_registry = [](void *) { return IdentityRegistry(); };
    d.record_assumptions = []() { return RuntimeAssumptions(); };
    d.make_ctx = [](void *) -> void * { return (void*)0x2; };
    d.prefill = [](void *, const std::vector<llama_token_i> &) { return true; };
    d.ledger_window = []() { return CallWindow(); };
    d.ledger_snapshot = []() { return std::vector<LedgerRow>(); };
    d.enumerate = []() { Enumeration n; n.gf="G"; n.n_declared=1; n.valid=true; return n; };
    d.correlate = [](const CallWindow & w, const Enumeration & n) { return correlate_call(w,n); };
    d.logits = [](void *) -> const std::vector<float> * { static std::vector<float> l={0.f,5.f}; return &l; };
    d.decode_step = [](void *, llama_token_i) { return true; };
    d.release = [](void *, void *) {};
    d.extraction_count = []() { return 0LL; };
    d.expected_targets = []() { return ExpectedTargets(); };
    d.wht_collect = [](const ExpectedTargets &) { return WhtAssessment(); };
    d.p4_checks = [in]() { return check_p4_structural(in); };
    return d;
}
static PipelineRequest req() { PipelineRequest r; r.doc_path="d"; r.max_new_tokens=8; r.capture_root="/tmp/ad"; return r; }

int main() {
    printf("ENGINE-008 -- ADOPTION de P4' et gate\n\n");
    system("mkdir -p /tmp/ad");
    printf("=== P4' ADOPTEE, SIX CONDITIONS PASS (mocks) ===\n");
    { PipelineDeps d0 = deps(all_pass_inputs());
      PipelineReport r = run_pipeline(req(), d0);
      ck("p4_adopted_in_report", r.p4_adopted && has(r.serialized, "\"p4_adopted\":true"));
      ck("property_serialized",
         has(r.serialized, "STRUCTURAL_WHT_PATH_UNREACHABILITY"));
      ck("real_model_conditions_not_exercised",
         has(r.serialized, "NOT_EXERCISED"));
      ck("gate_open_on_six_pass", r.p4_gate == "OPEN" && r.p4_all_pass);
      ck("no_blocked_limit", !has(r.serialized, "P4PRIME_GATE_BLOCKED"));
      ck("per_condition_results_serialized",
         has(r.serialized, "\"C1\"") && has(r.serialized, "\"C6\"") &&
         has(r.serialized, "\"proof\"") && has(r.serialized, "\"scope\"")); }
    printf("\n=== CONDITION FAIL -> GATE BLOQUEE ===\n");
    { auto in = all_pass_inputs(); in.n_relevant_tq2g128 = 1; in.tq2_offenders={"blk.8.w"};
      PipelineDeps dd = deps(in);
      PipelineReport r = run_pipeline(req(), dd);
      ck("gate_blocked_on_fail", r.p4_gate == "BLOCKED" && !r.p4_all_pass);
      ck("blocked_limit_published", has(r.serialized, "P4PRIME_GATE_BLOCKED"));
      ck("overall_fail_serialized", has(r.serialized, "\"p4_overall\":\"FAIL\"")); }
    printf("\n=== CONDITION UNKNOWN -> GATE BLOQUEE (jamais PASS) ===\n");
    { auto in = all_pass_inputs(); in.model_loaded=false; in.registry_acquired=false;
      PipelineDeps dd = deps(in);
      PipelineReport r = run_pipeline(req(), dd);
      ck("gate_blocked_on_unknown", r.p4_gate == "BLOCKED");
      ck("unknown_never_becomes_pass", !r.p4_all_pass &&
         has(r.serialized, "\"p4_overall\":\"UNKNOWN\"")); }
    { auto in = all_pass_inputs(); in.type_inferred_from_name = true;   // C2 FAIL
      PipelineDeps dd = deps(in);
      PipelineReport r = run_pipeline(req(), dd);
      ck("inferred_type_blocks_gate", r.p4_gate == "BLOCKED" &&
         has(r.serialized, "FAIL")); }
    printf("\n=== LIMITE EXPLICITE : operations du graphe != experts au runtime ===\n");
    { PipelineDeps d0 = deps(all_pass_inputs());
      PipelineReport r = run_pipeline(req(), d0);
      ck("graph_ops_vs_runtime_expert_selection_limit_published",
         has(r.serialized, "GRAPH_OPS_NOT_RUNTIME_EXPERT_SELECTION")); }

    printf("\nADOPTION_SELFTEST=%s\n", failed==0?"PASS":"FAIL");
    printf("FAILED_COUNT=%d\n", failed);
    printf("FULL_MODEL_LOAD_COUNT=0\nMODEL_FORWARD_CALL_COUNT=0\n");
    return failed==0?0:1;
}
