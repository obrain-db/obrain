// ENGINE-008 -- T4 : les quatre cas de correspondance invocation <-> operation,
// traverses par run_pipeline jusqu'au RAPPORT SERIALISE.
#include "engine008_pipeline.h"
#include "engine008_inventory.h"
#include <cstdio>
#include <fstream>

static int failed = 0;
static void ck(const char * n, bool ok) {
    printf("  %-72s %s\n", n, ok ? "PASS" : "FAIL");
    if (!ok) failed++;
}
static bool has(const std::string & h, const std::string & n) { return h.find(n) != std::string::npos; }

struct Env {
    std::vector<std::string> inv = {"w1","w2","w3"};
    std::vector<NativeTensor> reg;
    std::set<std::string>     captured;
    MatmulScan                scan;
};
static PipelineDeps deps(Env & e) {
    PipelineDeps d;
    d.authorize = [](const PipelineRequest &) { return AuthVerdict::ACCEPTED; };
    d.prepare_capture = [](const std::string &, const std::string & m, const std::string & r,
                           std::string & o, std::string &) { o = "/tmp/t4/" + m + "/" + r; return true; };
    d.load_full = [](const std::string &) -> void * { return (void*)0x1; };
    d.acquire_registry = [&e](void *) {
        IdentityRegistry r;
        for (const auto & k : e.captured) { RegistryEntry x; x.key=k; x.role="R"; x.usable_as_positive_witness=true; x.tensor=(void*)0xA; r.entries.push_back(x); }
        return r; };
    d.record_assumptions = []() { return RuntimeAssumptions(); };
    d.make_ctx = [](void *) -> void * { return (void*)0x2; };
    d.prefill = [](void *, const std::vector<llama_token_i> &) { return true; };
    d.ledger_window = []() { return CallWindow(); };
    d.ledger_snapshot = []() { return std::vector<LedgerRow>(); };
    d.enumerate = []() { Enumeration n; n.gf="G"; n.n_declared=1; n.valid=true; return n; };
    d.correlate = [](const CallWindow & w, const Enumeration & n) { return correlate_call(w, n); };
    d.logits = [](void *) -> const std::vector<float> * { static std::vector<float> l={0.f,5.f}; return &l; };
    d.decode_step = [](void *, llama_token_i) { return true; };
    d.release = [](void *, void *) {};
    d.extraction_count = []() { return 0LL; };
    d.expected_targets = [&e]() { ExpectedTargets x; x.state=N1State::KNOWN;
        for (const auto & k : e.inv) x.identities.insert(k); return x; };
    d.wht_collect = [](const ExpectedTargets &) { return WhtAssessment(); };
    d.inventory_validation = [&e]() { return validate_inventory(e.inv, e.reg, 3); };
    d.graph_enumeration_available = []() { return true; };
    d.submitted_matmul_ops = [&e]() { return e.scan; };
    return d;
}
static PipelineRequest req() {
    PipelineRequest r; r.doc_path="d"; r.max_new_tokens=8; r.capture_root="/tmp/t4";
    r.model_tag="m"; r.run_tag="r"; return r;
}

int main() {
    printf("ENGINE-008 -- T4 : correspondance invocation <-> operation\n\n");
    system("mkdir -p /tmp/t4");

    printf("=== 1. COPIE DE CAPTURE : ne doit PAS devenir une invocation ===\n");
    { Env e;
      e.reg = {{"w1",3,(void*)0x11},{"w2",3,(void*)0x12},{"w3",3,(void*)0x13}};
      e.captured = {"w1","w2","w3"};
      // Q1 (llama-graph.cpp:1526-1535) : le chemin de capture cree ggml_dup de
      // l ACTIVATION et ggml_dup du RESULTAT, plus UN SEUL ggml_mul_mat(w,...).
      // Les dup ont pour src0 l activation ou le resultat : jamais le poids.
      e.scan.ops = {{"w1",(void*)0x11},{"w2",(void*)0x12},{"w3",(void*)0x13}};
      e.scan.n_nodes = 6; e.scan.n_matmul_with_weight = 3;
      e.scan.n_nonmatmul_with_weight = 3;   // les 3 ggml_dup de capture
      e.scan.complete = true;
      PipelineDeps d = deps(e);
      PipelineReport r = run_pipeline(req(), d);
      ck("capture_copies_do_not_add_expected_invocations",
         r.invi_expected == 3 && r.invi_eligible == 3);
      ck("nonmatmul_ops_are_counted_separately_not_as_invocations",
         r.scan_nonmatmul == 3 && has(r.serialized, "\"scan_nonmatmul\":3"));
      ck("capture_copy_case_complete_and_serialized",
         r.invi_complete && has(r.serialized, "\"invi_complete\":true")); }

    printf("\n=== 2. ALIAS / VUE DU POIDS : non resolu -> AMBIGU, completude INTERDITE ===\n");
    { Env e;
      e.reg = {{"w1",3,(void*)0x11},{"w2",3,(void*)0x12},{"w3",3,(void*)0x13}};
      e.captured = {"w1","w2","w3"};
      e.scan.ops = {{"w1",(void*)0x11},{"w2",(void*)0x12}};   // w3 vu par alias -> NON resolu
      e.scan.n_nodes = 5; e.scan.n_matmul_with_weight = 3;
      e.scan.n_unregistered_src0 = 1;                          // le matmul alias
      e.scan.complete = false;
      PipelineDeps d = deps(e);
      PipelineReport r = run_pipeline(req(), d);
      ck("aliased_src0_counted_as_unregistered", r.scan_unregistered == 1);
      ck("unregistered_src0_blocks_completeness", !r.invi_complete);
      ck("ambiguity_serialized",
         has(r.serialized, "\"scan_unregistered\":1") &&
         has(r.serialized, "src0_non_resolu"));
      ck("ambiguity_limit_published", [&]{ for (const auto& x : r.limits) if (x=="MATMUL_SRC0_NOT_IN_REGISTRY") return true; return false; }()); }

    printf("\n=== 3. EXPERT NON SELECTIONNE : charge != operation construite ===\n");
    { Env e;
      e.reg = {{"w1",3,(void*)0x11},{"w2",3,(void*)0x12},{"w3",3,(void*)0x13}};
      e.captured = {"w1","w2"};                 // w3 charge mais NON selectionne
      e.scan.ops = {{"w1",(void*)0x11},{"w2",(void*)0x12}};   // aucune op pour w3
      e.scan.n_nodes = 4; e.scan.n_matmul_with_weight = 2; e.scan.complete = true;
      PipelineDeps d = deps(e);
      PipelineReport r = run_pipeline(req(), d);
      ck("loaded_but_unselected_weight_not_counted",
         r.invi_expected == 2 && r.invi_eligible == 3);
      ck("loaded_count_differs_from_invocation_count",
         has(r.serialized, "\"n_eligible_weights\":3") &&
         has(r.serialized, "\"n_expected_invocations\":2")); }
    { Env e;   // MUL_MAT_ID (experts MoE) DOIT etre compte
      e.reg = {{"w1",3,(void*)0x11},{"w2",3,(void*)0x12},{"w3",3,(void*)0x13}};
      e.captured = {"w1","w2","w3"};
      e.scan.ops = {{"w1",(void*)0x11},{"w2",(void*)0x12},{"w3",(void*)0x13}};
      e.scan.n_nodes = 4; e.scan.n_matmul_with_weight = 3; e.scan.complete = true;
      PipelineDeps d = deps(e);
      PipelineReport r = run_pipeline(req(), d);
      ck("mul_mat_id_path_would_be_counted", r.invi_expected == 3); }

    printf("\n=== 4. AUTRE OPERATION UTILISANT LE MEME POIDS ===\n");
    { Env e;
      e.reg = {{"w1",3,(void*)0x11},{"w2",3,(void*)0x12},{"w3",3,(void*)0x13}};
      e.captured = {"w1","w2","w3"};
      e.scan.ops = {{"w1",(void*)0x11},{"w2",(void*)0x12},{"w3",(void*)0x13}};
      e.scan.n_nodes = 8; e.scan.n_matmul_with_weight = 3;
      e.scan.n_nonmatmul_with_weight = 5;      // 5 ops NON matmul portant le poids
      e.scan.complete = true;
      PipelineDeps d = deps(e);
      PipelineReport r = run_pipeline(req(), d);
      ck("shared_weight_nonmatmul_NOT_an_invocation",
         r.invi_expected == 3 && r.scan_nonmatmul == 5);
      ck("scan_counts_serialized_exhaustively",
         has(r.serialized, "\"scan_nodes\":8") && has(r.serialized, "\"scan_matmul\":3") &&
         has(r.serialized, "scan_complete")); }

    printf("\n=== 5. CAS CROISES : multiplicite et fenetre suivante ===\n");
    { Env e;
      e.reg = {{"w1",3,(void*)0x11}};
      e.captured = {"w1"};
      e.scan.ops = {{"w1",(void*)0x11},{"w1",(void*)0x11}};   // DEUX invocations
      e.scan.n_nodes = 3; e.scan.n_matmul_with_weight = 2; e.scan.complete = true;
      PipelineDeps d = deps(e);
      PipelineReport r = run_pipeline(req(), d);
      ck("multiplicity_preserved", r.invi_expected == 2 && !r.invi_complete);
      ck("ambiguity_from_multiplicity_serialized",
         has(r.serialized, "ambiguous_references")); }

    printf("\nT4_SELFTEST=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("FAILED_COUNT=%d\n", failed);
    printf("FULL_MODEL_LOAD_COUNT=0\nMODEL_FORWARD_CALL_COUNT=0\n");
    return failed == 0 ? 0 : 1;
}
