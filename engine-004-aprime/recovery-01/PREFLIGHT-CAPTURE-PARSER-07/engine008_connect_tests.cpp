// ENGINE-008 -- PREFLIGHT-CONNECT-05 : tests de la chaine complete, mocks.
// Les mocks FOURNISSENT des sorties CONSOMMEES par l'etape suivante : table de
// tenseurs -> registre, graphe synthetique -> enumeration, evenements -> ledger,
// logits -> argmax -> contrat d'arret.
#include "engine008_pipeline.h"
#include <cstdio>
#include <cstring>
#include <map>
#include <set>

static int failed = 0;
static void ck(const char * n, bool ok) {
    printf("  %-66s %s\n", n, ok ? "PASS" : "FAIL");
    if (!ok) failed++;
}

// ------------------------------------------------------------------ MOCK ENV
struct MockEnv {
    std::vector<void *> tensors;
    std::map<void *, std::string> names;
    std::map<std::string, void *> by_key;
    std::vector<LedgerRow> ledger;
    std::string gf = "0xAAA";
    int n_nodes = 100;
    std::vector<float> logits_row;
    bool fail_load = false, fail_ctx = false, fail_prefill = false, fail_decode = false;
    bool malformed_window = false, stale = false, multi_submit = false;
    bool empty_logits = false;
    long long load_calls = 0, ctx_calls = 0, prefill_calls = 0, decode_calls = 0;
    long long enum_calls = 0, corr_calls = 0, release_calls = 0, logits_calls = 0;
    std::vector<std::string> order;

    void * tensor(const char * key) {
        void * t = (void *) ((uintptr_t) tensors.size() + 1);   // identite opaque
        tensors.push_back(t);
        names[t] = key;
        by_key[key] = t;
        return t;
    }
};

// Table de tenseurs MOCKEE : un poids NextN exclusif, un poids partage, un
// poids ordinaire, plus une cle hors contrat.
static IdentityRegistry mock_registry(MockEnv & e) {
    IdentityRegistry r;
    void * w_nextn = e.tensor("blk.64.nextn.eh_proj.weight");
    void * w_shared = e.tensor("blk.64.nextn.shared_head_head.weight");
    void * w_ord    = e.tensor("blk.0.ffn_gate.weight");
    RegistryEntry a; a.key = "blk.64.nextn.eh_proj.weight"; a.tensor = w_nextn;
    a.role = "NEXTN_EH_PROJ"; a.usable_as_positive_witness = true; a.state = EntryState::ROLE_RESOLVED;
    RegistryEntry b; b.key = "blk.64.nextn.shared_head_head.weight"; b.tensor = w_shared;
    b.role = "NEXTN_SHARED_HEAD_HEAD"; b.usable_as_positive_witness = false;
    b.state = EntryState::EXCLUSIVITY_UNESTABLISHED;      // hors temoins positifs
    RegistryEntry c; c.key = "blk.0.ffn_gate.weight"; c.tensor = w_ord; c.state = EntryState::ROLE_ABSENT;
    r.entries = { a, b, c };
    return r;
}

static PipelineDeps make_mock_deps(MockEnv & e) {
    PipelineDeps d;
    d.authorize = [&e](const PipelineRequest & r) {
        bool have = !r.doc_path.empty();
        bool allow = !e.fail_ctx;   // permet de tester un refus via un doc non autorisant
        return authorize_doc(r.flag_allow_model_compute, r.binary_sha256, r.plan_sha256,
                             r.protocol_sha256, have, true, allow,
                             r.binary_sha256, r.plan_sha256, r.protocol_sha256);
    };
    d.prepare_capture = [&e](const std::string &, const std::string & m, const std::string & r,
                             std::string & out, std::string & refusal) {
        e.order.push_back("prepare_capture");
        out = "/mock/" + m + "/" + r;
        if (e.stale) { refusal = "destination non vide"; return false; }
        return true;
    };
    d.load_full = [&e](const std::string &) -> void * {
        e.order.push_back("load_full"); e.load_calls++;
        return e.fail_load ? nullptr : (void *) 0x1000;
    };
    d.acquire_registry = [&e](void *) { e.order.push_back("acquire_registry"); return mock_registry(e); };
    d.record_assumptions = [&e]() { e.order.push_back("record_params"); RuntimeAssumptions a; return a; };
    d.make_ctx = [&e](void *) -> void * {
        e.order.push_back("make_ctx"); e.ctx_calls++;
        return e.fail_ctx ? nullptr : (void *) 0x2000;
    };
    d.prefill = [&e](void *, const std::vector<llama_token_i> &) {
        e.order.push_back("prefill"); e.prefill_calls++;
        if (e.fail_prefill) return false;
        // la soumission est enregistree : elle sera vue par le releve APRES
        LedgerRow r; r.submission_id = e.ledger.size() + 1; r.gf = e.gf;
        r.n_nodes = e.n_nodes; r.submission_rc = 0; r.fingerprint = "fpMOCK";
        e.ledger.push_back(r);
        if (e.multi_submit) { LedgerRow r2 = r; r2.submission_id++; r2.gf = "0xBBB"; r2.n_nodes = 200; e.ledger.push_back(r2); }
        return true;
    };
    d.ledger_window = [&e]() { e.order.push_back("window_before"); CallWindow w; w.before = e.ledger; return w; };
    d.ledger_snapshot = [&e]() { e.order.push_back("window_after"); return e.ledger; };
    d.enumerate = [&e]() {
        e.order.push_back("enumerate"); e.enum_calls++;
        Enumeration n;
        if (e.malformed_window) { n.malformed = true; n.valid = false; return n; }
        n.gf = e.gf; n.n_declared = e.n_nodes; n.n_exported = e.n_nodes; n.valid = true;
        return n;
    };
    d.correlate = [&e](const CallWindow & w, const Enumeration & n) {
        e.order.push_back("correlate"); e.corr_calls++;
        CallWindow ww = w; ww.malformed = e.malformed_window;
        return correlate_call(ww, n);
    };
    d.logits = [&e](void *) -> const std::vector<float> * {
        e.order.push_back("read_logits"); e.logits_calls++;
        if (e.empty_logits) { e.logits_row.clear(); return &e.logits_row; }
        return &e.logits_row;
    };
    d.decode_step = [&e](void *, llama_token_i) {
        e.order.push_back("decode_step"); e.decode_calls++;
        if (e.fail_decode) return false;
        LedgerRow r; r.submission_id = e.ledger.size() + 1; r.gf = e.gf;
        r.n_nodes = e.n_nodes; r.submission_rc = 0; r.fingerprint = "fpMOCK2";
        e.ledger.push_back(r);
        return true;
    };
    d.release = [&e](void *, void *) { e.order.push_back("release"); e.release_calls++; };
    return d;
}

static PipelineRequest base_request() {
    PipelineRequest r;
    r.doc_path = "auth.json"; r.flag_allow_model_compute = true;
    r.binary_sha256 = "BINSHA"; r.plan_sha256 = "PLANSHA"; r.protocol_sha256 = "PROTOSHA";
    r.model_path = "/mock/model.gguf"; r.template_path = "/mock/tpl";
    r.capture_root = "/mock"; r.model_tag = "baseline"; r.run_tag = "run1";
    r.prompt_tokens = { 1, 2, 3 };
    r.max_new_tokens = 8;
    return r;
}
static bool contains(const std::vector<std::string> & v, const char * s) {
    for (const auto & x : v) if (x == s) return true;
    return false;
}

int main() {
    printf("ENGINE-008 -- PREFLIGHT-CONNECT-05 : CHAINE COMPLETE (mocks)\n\n");
    const std::string OK_ID = "188523132d85ccb942b84951535deab35db98212bfdf5f500c04b236c02de6c7";

    printf("=== CHEMIN NOMINAL MOCKE ===\n");
    {
        MockEnv e; e.logits_row = { 0.f, 5.f };          // argmax -> id 1
        PipelineDeps d = make_mock_deps(e);
        PipelineReport r = run_pipeline(base_request(), d);
        ck("nominal_accepted", r.auth == AuthVerdict::ACCEPTED);
        ck("nominal_load_ok", r.load_ok);
        ck("nominal_ctx_ok", r.ctx_ok);
        ck("ordering_gate_before_load", !d.trace.empty() && d.trace[0] == "authorize" &&
           d.trace.size() > 1 && d.trace[1] == "prepare_capture");
        ck("ordering_full_chain_traversed",
           contains(d.trace, "prepare_capture") && contains(d.trace, "load_full") &&
           contains(d.trace, "acquire_registry") && contains(d.trace, "record_params") &&
           contains(d.trace, "make_ctx") && contains(d.trace, "prefill") &&
           contains(d.trace, "enumerate") && contains(d.trace, "correlate") &&
           contains(d.trace, "read_logits") && contains(d.trace, "serialize"));
        // ordre RELATIF impose
        { int i_load = -1, i_reg = -1, i_ctx = -1, i_pre = -1, i_enum = -1, i_corr = -1;
          for (int i = 0; i < (int) d.trace.size(); ++i) {
            if (i_load < 0 && d.trace[i] == "load_full") i_load = i;
            if (i_reg  < 0 && d.trace[i] == "acquire_registry") i_reg = i;
            if (i_ctx  < 0 && d.trace[i] == "make_ctx") i_ctx = i;
            if (i_pre  < 0 && d.trace[i] == "prefill") i_pre = i;
            if (i_enum < 0 && d.trace[i] == "enumerate") i_enum = i;
            if (i_corr < 0 && d.trace[i] == "correlate") i_corr = i;
          }
          ck("relative_order_load_registry_ctx_prefill_enum_correlate",
             i_load < i_reg && i_reg < i_ctx && i_ctx < i_pre && i_pre < i_enum && i_enum < i_corr); }
        ck("registry_built_and_used", r.registry_entries == 3 && r.registry_usable_witnesses == 1);
        ck("shared_head_not_a_witness", r.registry_usable_witnesses == 1);
        ck("enumeration_declared_matches_capture", r.enumeration.n_declared == 100);
        ck("correlation_reached_correlator", e.corr_calls >= 1);
        ck("identity_pointer_and_count_only",
           r.correlation.identity == IdentityStrength::POINTER_AND_COUNT_ONLY);
        ck("coverage_incomplete_visible", contains(r.limits, "PER_CALL_COVERAGE_INCOMPLETE") ||
                                          contains(r.limits, "IDENTITY_POINTER_AND_COUNT_ONLY"));
        ck("allocation_released", e.release_calls == 1);
    }

    printf("=== REFUS AVANT TOUT CALCUL ===\n");
    {
        MockEnv e; PipelineDeps d = make_mock_deps(e);
        PipelineRequest q = base_request(); q.doc_path = "";
        PipelineReport r = run_pipeline(q, d);
        ck("refused", r.auth != AuthVerdict::ACCEPTED);
        ck("no_load_call", e.load_calls == 0);
        ck("no_ctx_call", e.ctx_calls == 0);
        ck("no_prefill_call", e.prefill_calls == 0);
        ck("no_decode_call", e.decode_calls == 0);
        ck("no_enumeration_call", e.enum_calls == 0);
        ck("refusal_marked_in_limits", contains(r.limits, "REFUSED_BEFORE_ANY_COMPUTE"));
    }

    printf("=== ERREURS ET NETTOYAGE ===\n");
    {
        MockEnv e; e.fail_ctx = true; PipelineDeps d = make_mock_deps(e);
        PipelineRequest q = base_request();
        PipelineReport r = run_pipeline(q, d);
        ck("ctx_failure_no_prefill", e.prefill_calls == 0);
        ck("ctx_failure_no_decode", e.decode_calls == 0);
    }
    {
        MockEnv e; e.fail_prefill = true; PipelineDeps d = make_mock_deps(e);
        PipelineReport r = run_pipeline(base_request(), d);
        ck("prefill_failure_releases", e.release_calls == 1);
        ck("prefill_failure_no_decode", e.decode_calls == 0);
        ck("prefill_failure_marked", contains(r.limits, "PREFILL_FAILED"));
    }
    {
        MockEnv e; e.fail_load = true; PipelineDeps d = make_mock_deps(e);
        PipelineReport r = run_pipeline(base_request(), d);
        ck("load_failure_marked", contains(r.limits, "LOAD_FAILED"));
        ck("load_failure_no_release_needed", e.release_calls == 0);
    }

    printf("=== COUVERTURE ET ATTRIBUTION ===\n");
    {
        MockEnv e; e.logits_row = { 0.f, 5.f }; e.multi_submit = true;
        PipelineDeps d = make_mock_deps(e);
        PipelineReport r = run_pipeline(base_request(), d);
        ck("multiple_submissions_per_call_incomplete",
           r.correlation.per_call == CorrelationVerdict::MULTIPLE_SUBMISSIONS_ONLY_LAST_ENUMERATED);
        ck("multiple_submissions_limit_visible", contains(r.limits, "PER_CALL_COVERAGE_INCOMPLETE"));
    }
    {
        MockEnv e; e.logits_row = { 0.f, 5.f }; e.malformed_window = true;
        PipelineDeps d = make_mock_deps(e);
        PipelineReport r = run_pipeline(base_request(), d);
        ck("malformed_capture_no_conclusion",
           r.correlation.per_call == CorrelationVerdict::MALFORMED_CAPTURE);
    }
    {
        MockEnv e; e.logits_row = { 0.f, 5.f };
        PipelineDeps d = make_mock_deps(e);
        PipelineReport r = run_pipeline(base_request(), d);
        bool claims_absence = r.serialized.find("NEXTN_NOT_DETECTED") != std::string::npos ||
                              r.serialized.find("ABSENT") != std::string::npos;
        ck("serialized_report_never_claims_nextn_absence", !claims_absence);
        ck("serialized_report_states_no_conclusion",
           r.serialized.find("NONE_IN_THIS_REPORT") != std::string::npos);
        ck("serialized_report_holds_registry_and_correlation",
           r.serialized.find("registry_entries") != std::string::npos &&
           r.serialized.find("per_call") != std::string::npos);
        ck("serialized_report_holds_stop_reason",
           r.serialized.find("stop_reason") != std::string::npos);
    }

    printf("=== CONTRAT D ARRET REJOUE DANS CE BINAIRE ===\n");
    { std::vector<llama_token_i> seq = { 248046 };
      auto o = simulate_generation([&](int i, llama_token_i & t){ t = seq[i]; return true; }, 8);
      ck("stop_eos_first_token", o.reason == StopReason::EOS && o.ids.size() == 1); }
    { std::vector<llama_token_i> seq(8, 100);
      auto o = simulate_generation([&](int i, llama_token_i & t){ t = seq[i]; return true; }, 8);
      ck("stop_max_new_tokens_at_8", o.reason == StopReason::MAX_NEW_TOKENS && o.ids.size() == 8); }
    { std::vector<llama_token_i> seq = {1,2,3,4,5,6,7,248046};
      auto o = simulate_generation([&](int i, llama_token_i & t){ t = seq[i]; return true; }, 8);
      ck("stop_eos_at_position_8_no_off_by_one", o.reason == StopReason::EOS && o.ids.size() == 8); }
    { int calls = 0;
      auto o = simulate_generation([&](int, llama_token_i & t){ calls++; t = 248046; return true; }, 8);
      ck("no_decode_after_stop", calls == 1); }
    { auto o = simulate_generation([](int i, llama_token_i & t){ t = 1; return i < 2; }, 8);
      ck("decode_error_reported", o.reason == StopReason::ERROR && o.ids.size() == 2); }
    { std::vector<float> l = {3.f, 9.f, 9.f, 1.f};
      ck("argmax_tie_lowest_id", greedy_argmax_lowest_id(l) == 1); }
    { std::vector<float> bad = {0.f, NAN};
      ck("invalid_logits_rejected", greedy_argmax_lowest_id(bad) == -1); }

    printf("\nINTEGRATED_MOCK_PATH_SELFTEST=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("REJECTION_GUARD_SELFTEST=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("STOP_CONTRACT_SELFTEST=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("SERIALIZED_PIPELINE_RESULT_SELFTEST=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("FAILED_COUNT=%d\n", failed);
    printf("FULL_MODEL_LOAD_COUNT=0\nMODEL_FORWARD_CALL_COUNT=0\n");
    return failed == 0 ? 0 : 1;
}
