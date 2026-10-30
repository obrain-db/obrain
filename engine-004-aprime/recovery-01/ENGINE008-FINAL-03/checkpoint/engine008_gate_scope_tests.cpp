// ENGINE-008 -- PORTEE DE LA GATE P4' : baseline seulement, et elle EMPECHE le calcul.
#include "engine008_pipeline.h"
#include "engine008_p4_checks.h"
#include <cstdio>
static int failed = 0;
static void ck(const char * n, bool ok) { printf("  %-70s %s\n", n, ok?"PASS":"FAIL"); if(!ok) failed++; }
static P4Inputs six_pass() {
    P4Inputs in;
    in.model_loaded=true; in.registry_acquired=true; in.registry_complete=true; in.n_tensors=851;
    in.types_read_from_tensors=true; in.n_types_read=851; in.type_inferred_from_name=false;
    in.n_relevant_weights=168; in.n_relevant_tq2g128=0;
    in.wht_sites_audited=true; in.n_wht_creation_sites=1; in.wht_sites={"llama-graph.cpp:1497"};
    in.libllama_sha256="87c42d5f5dd7ab3c10967b4d3eaac82636b6af11e2c18a7f2798bab2d01bb2b2";
    in.expected_libllama_sha256="87c42d5f5dd7ab3c10967b4d3eaac82636b6af11e2c18a7f2798bab2d01bb2b2";
    in.obrain_no_rotation_present=false; in.ctx_type="LLAMA_CONTEXT_TYPE_DEFAULT";
    in.gtype_status="GTYPE_DERIVED_FROM_VERIFIED_SOURCE_MAPPING";
    return in;
}
struct Counters { int prefill=0, decode=0, ctx=0; };
static PipelineDeps deps(P4Inputs in, Counters & c, bool with_checks = true) {
    PipelineDeps d;
    d.authorize = [](const PipelineRequest &) { return AuthVerdict::ACCEPTED; };
    d.prepare_capture = [](const std::string & r, const std::string &, const std::string &,
                           std::string & o, std::string &) { o = r; return true; };
    d.load_full = [](const std::string &) -> void * { return (void*)0x1; };
    d.acquire_registry = [](void *) { return IdentityRegistry(); };
    d.record_assumptions = []() { return RuntimeAssumptions(); };
    d.make_ctx = [&c](void *) -> void * { c.ctx++; return (void*)0x2; };
    d.prefill = [&c](void *, const std::vector<llama_token_i> &) { c.prefill++; return true; };
    d.ledger_window = []() { return CallWindow(); };
    d.ledger_snapshot = []() { return std::vector<LedgerRow>(); };
    d.enumerate = []() { Enumeration n; n.gf="G"; n.n_declared=1; n.valid=true; return n; };
    d.correlate = [](const CallWindow & w, const Enumeration & n) { return correlate_call(w, n); };
    d.logits = [](void *) -> const std::vector<float> * { static std::vector<float> l={0.f,5.f}; return &l; };
    d.decode_step = [&c](void *, llama_token_i) { c.decode++; return true; };
    d.release = [](void *, void *) {};
    d.extraction_count = []() { return 0LL; };
    d.expected_targets = []() { return ExpectedTargets(); };
    d.wht_collect = [](const ExpectedTargets &) { return WhtAssessment(); };
    if (with_checks) d.p4_checks = [in]() { return check_p4_structural(in); };
    return d;
}
static PipelineRequest req(const char * role) {
    PipelineRequest r; r.doc_path="d"; r.max_new_tokens=8; r.capture_root="/tmp/gs";
    r.prompt_tokens = {1,2,3}; r.validated_role = role;
    r.p4_applies = (std::string(role) == "baseline");
    return r;
}
int main() {
    printf("ENGINE-008 -- portee de la gate P4'\n\n");
    system("mkdir -p /tmp/gs");
    printf("=== BASELINE : six PASS -> poursuite ===\n");
    { Counters c; PipelineDeps d = deps(six_pass(), c);
      PipelineReport r = run_pipeline(req("baseline"), d);
      ck("baseline_six_pass_reaches_prefill", c.prefill == 1 && c.ctx == 1);
      ck("gate_open_serialized", r.p4_gate == "OPEN" && r.p4_applies); }
    printf("\n=== BASELINE : UNKNOWN -> AUCUN forward ===\n");
    { P4Inputs in = six_pass(); in.model_loaded=false; in.registry_acquired=false;
      Counters c; PipelineDeps d = deps(in, c);
      PipelineReport r = run_pipeline(req("baseline"), d);
      ck("blocked_gate_prevents_context", c.ctx == 0);
      ck("blocked_gate_prevents_prefill", c.prefill == 0);
      ck("blocked_gate_prevents_decode", c.decode == 0);
      ck("blocking_limit_published",
         [&]{ for (const auto& x : r.limits) if (x=="P4PRIME_GATE_BLOCKED_BEFORE_FORWARD") return true; return false; }()); }
    printf("\n=== BASELINE : FAIL (type TQ2 pertinent) -> AUCUN forward ===\n");
    { P4Inputs in = six_pass(); in.n_relevant_tq2g128=1; in.tq2_offenders={"blk.8.w"};
      Counters c; PipelineDeps d = deps(in, c);
      PipelineReport r = run_pipeline(req("baseline"), d);
      ck("fail_gate_prevents_prefill", c.prefill == 0 && c.ctx == 0); }
    printf("\n=== CANDIDAT : P4' baseline NON APPLICABLE ===\n");
    { P4Inputs in = six_pass(); in.model_loaded=false; in.registry_acquired=false;  // UNKNOWN
      Counters c; PipelineDeps d = deps(in, c);
      PipelineReport r = run_pipeline(req("candidate"), d);
      ck("candidate_not_blocked_by_baseline_gate", c.prefill == 1 && c.ctx == 1);
      ck("p4_applies_false_in_report", !r.p4_applies);
      ck("validated_role_serialized", r.validated_role == "candidate"); }
    printf("\n=== SANS EVALUATION P4' : pas de gate a appliquer ===\n");
    { Counters c; PipelineDeps d = deps(six_pass(), c, /*with_checks*/ false);
      PipelineReport r = run_pipeline(req("baseline"), d);
      ck("no_checks_no_default_block", c.prefill == 1 && !r.p4_checks_run); }
    printf("\nGATE_SCOPE_SELFTEST=%s\nFAILED_COUNT=%d\n", failed==0?"PASS":"FAIL", failed);
    printf("FULL_MODEL_LOAD_COUNT=0\nMODEL_FORWARD_CALL_COUNT=0\n");
    return failed==0?0:1;
}
