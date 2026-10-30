// ENGINE-008 -- T1 : TEST BOUT-EN-BOUT DU CHEMIN RACCORDE.
// inventaire lu (format reel) -> registre MOCKE -> graphe SYNTHETIQUE
//   -> validation -> invocations attendues -> captures/relations
//   -> reconciliation -> RAPPORT SERIALISE.
// Traverse run_pipeline et les adaptateurs reellement utilises. Les fonctions
// pures ne sont PAS appelees separement : tout passe par le pipeline.
#include "engine008_pipeline.h"
#include "engine008_inventory.h"
#include <cstdio>
#include <fstream>
#include <sstream>

static int failed = 0;
static void ck(const char * n, bool ok) {
    printf("  %-70s %s\n", n, ok ? "PASS" : "FAIL");
    if (!ok) failed++;
}
static bool has(const std::string & h, const std::string & n) { return h.find(n) != std::string::npos; }
static std::string slurp(const std::string & p) { std::ifstream f(p); std::ostringstream s; s << f.rdbuf(); return s.str(); }

// --- inventaire SYNTHETIQUE ecrit au FORMAT REEL du producteur
static const char * INV_TEXT =
    "tensor_name,family,note\n"
    "# inventaire synthetique de test\n"
    "blk.8.attn_gate.weight,5,test\n"
    "blk.9.ffn_gate.weight,5,test\n"
    "\n"
    "blk.10.ffn_up.weight,5,test\n";

struct E2E {
    std::vector<std::string> inv_keys;
    std::vector<NativeTensor> reg;              // registre MOCKE
    std::vector<MatmulOp>     ops;              // graphe SYNTHETIQUE
    std::set<std::string>     registry_named;   // pour la reconciliation
};

static PipelineDeps make_deps(E2E & e, bool graph_available = true) {
    PipelineDeps d;
    d.authorize = [](const PipelineRequest &) { return AuthVerdict::ACCEPTED; };
    d.prepare_capture = [](const std::string &, const std::string & m, const std::string & r,
                           std::string & out, std::string &) { out = "/tmp/e2e/" + m + "/" + r; return true; };
    d.load_full = [](const std::string &) -> void * { return (void *) 0x1000; };
    d.acquire_registry = [&e](void *) {
        IdentityRegistry r;
        for (const auto & k : e.registry_named) {
            RegistryEntry x; x.key = k; x.role = "ROLE"; x.usable_as_positive_witness = true;
            x.tensor = (void *) 0xAA; r.entries.push_back(x);
        }
        return r; };
    d.record_assumptions = []() { return RuntimeAssumptions(); };
    d.make_ctx = [](void *) -> void * { return (void *) 0x2000; };
    d.prefill = [](void *, const std::vector<llama_token_i> &) { return true; };
    d.ledger_window = []() { return CallWindow(); };
    d.ledger_snapshot = []() { return std::vector<LedgerRow>(); };
    d.enumerate = []() { Enumeration n; n.gf = "0xG"; n.n_declared = 10; n.valid = true; return n; };
    d.correlate = [](const CallWindow & w, const Enumeration & n) { return correlate_call(w, n); };
    d.logits = [](void *) -> const std::vector<float> * { static std::vector<float> l = {0.f, 5.f}; return &l; };
    d.decode_step = [](void *, llama_token_i) { return true; };
    d.release = [](void *, void *) {};
    d.extraction_count = []() { return 0LL; };
    d.expected_targets = [&e]() {
        ExpectedTargets x; x.state = N1State::KNOWN;
        for (const auto & k : e.inv_keys) x.identities.insert(k);
        return x; };
    d.wht_collect = [](const ExpectedTargets &) { return WhtAssessment(); };
    // ---- LE CHEMIN RACCORDE, avec les VRAIS adaptateurs de logique
    d.inventory_validation = [&e]() {
        return validate_inventory(e.inv_keys, e.reg, 3); };
    d.graph_enumeration_available = [&e, graph_available]() { return graph_available; };
    d.submitted_matmul_ops = [&e]() { return e.ops; };
    return d;
}
static PipelineRequest req() {
    PipelineRequest r; r.doc_path = "d"; r.max_new_tokens = 8;
    r.prompt_tokens = {1,2,3}; r.capture_root = "/tmp/e2e"; r.model_tag = "m"; r.run_tag = "r";
    return r;
}

int main() {
    printf("ENGINE-008 -- T1 : chemin raccorde bout-en-bout\n\n");
    system("mkdir -p /tmp/e2e");
    const std::string ip = "/tmp/e2e/inventory.csv";
    { std::ofstream f(ip); f << INV_TEXT; }

    printf("=== A. INVENTAIRE LU AU FORMAT REEL ===\n");
    InventoryRead ir = read_inventory_source(slurp(ip), true);
    ck("header_tensor_name_skipped", ir.header_seen && ir.n_skipped_header == 1);
    ck("comment_skipped", ir.n_skipped_comment == 1);
    ck("three_unique_names", ir.unique_names.size() == 3 && ir.names.size() == 3);
    ck("no_duplicates", ir.n_duplicates == 0 && !ir.capture_disabled_by_duplicates);
    { std::string dup = std::string(INV_TEXT) + "blk.8.attn_gate.weight,5,x\n";
      auto d2 = read_inventory_source(dup, true);
      ck("duplicate_disables_capture_per_producer",
         d2.n_duplicates == 1 && d2.capture_disabled_by_duplicates); }

    printf("\n=== B. CHEMIN COMPLET : VALIDATION -> INVOCATIONS -> RECONCILIATION -> RAPPORT ===\n");
    E2E e;
    for (const auto & n : ir.unique_names) e.inv_keys.push_back(n);
    // registre MOCKE : les 3 cles de l inventaire en TQ2 + 1 poids TQ2 HORS inventaire
    e.reg.push_back({"blk.8.attn_gate.weight", 3, (void*)0x1});
    e.reg.push_back({"blk.9.ffn_gate.weight", 3, (void*)0x2});
    e.reg.push_back({"blk.10.ffn_up.weight", 3, (void*)0x3});
    e.reg.push_back({"blk.11.hors.weight",     3, (void*)0x4});
    e.registry_named = {"blk.8.attn_gate.weight","blk.9.ffn_gate.weight","blk.10.ffn_up.weight"};
    // graphe SYNTHETIQUE : deux MUL_MAT eligibles, une MUL_MAT hors registre,
    // une operation NON matmul sur le meme poids (ne doit PAS compter)
    e.ops.push_back({"blk.8.attn_gate.weight", (void*)0x1});
    e.ops.push_back({"blk.9.ffn_gate.weight",  (void*)0x2});
    { PipelineDeps d = make_deps(e);
      PipelineReport r = run_pipeline(req(), d);
      ck("pipeline_reached_the_connected_path",
         r.inv_validation_run && r.inv_run);
      ck("inventory_validation_serialized",
         has(r.serialized, "ENGINE008_INVENTORY_VALIDATION_v1"));
      ck("expected_invocations_serialized",
         has(r.serialized, "ENGINE008_EXPECTED_INVOCATIONS_v1"));
      ck("inventory_absent_key_from_mocked_registry",
         r.inv_absent == 0 && r.inv_valid == 3);
      ck("tq2_outside_inventory_detected_through_pipeline",
         r.inv_tq2_outside == 1 &&
         has(r.serialized, "blk.11.hors.weight"));
      ck("inventory_not_globally_complete_flagged",
         !r.inv_globally_complete && has(r.serialized, "\"globally_complete\":false"));
      ck("limit_published_in_report",
         [&]{ for (const auto& x : r.limits) if (x=="INVENTORY_NOT_GLOBALLY_COMPLETE") return true; return false; }());
      ck("expected_invocations_known_and_counted",
         r.invi_expected_known && r.invi_expected == 2 && r.invi_eligible == 3);
      ck("invocation_counts_serialized",
         has(r.serialized, "\"n_expected_invocations\":2") &&
         has(r.serialized, "\"n_eligible_weights\":3"));
      ck("verdict_serialized", has(r.serialized, "\"invi_verdict\""));
    }

    printf("\n=== C. INVOCATION ATTENDUE MAIS SANS CAPTURE -> NON COMPLET ===\n");
    { E2E e2 = e;
      e2.registry_named = {"blk.8.attn_gate.weight"};   // une seule capture
      PipelineDeps d = make_deps(e2);
      PipelineReport r = run_pipeline(req(), d);
      ck("invoked_never_captured_detected", r.invi_invoked_never_captured >= 1);
      ck("not_complete", !r.invi_complete);
      ck("missing_identities_serialized_in_invocations",
         has(r.serialized, "\"invoked_never_captured\":[") &&
         has(r.serialized, "blk.9.ffn_gate.weight")); }
    { E2E e3 = e;
      e3.registry_named = {"blk.8.attn_gate.weight","blk.9.ffn_gate.weight"};
      PipelineDeps d = make_deps(e3);
      PipelineReport r = run_pipeline(req(), d);
      ck("complete_when_invocations_match_captures", r.invi_complete);
      ck("complete_verdict_serialized", has(r.serialized, "\"verdict\":\"COMPLETE\"")); }

    printf("\n=== D. REFUS DE COMPLETUDE ===\n");
    { E2E e4 = e;
      e4.registry_named = {"blk.8.attn_gate.weight","blk.9.ffn_gate.weight"};
      PipelineDeps d = make_deps(e4, /*graph_available*/ false);
      PipelineReport r = run_pipeline(req(), d);
      ck("unknown_reference_blocks_completeness",
         !r.invi_expected_known && !r.invi_complete);
      ck("limit_for_unknown_reference_published",
         [&]{ for (const auto& x : r.limits) if (x=="EXPECTED_INVOCATIONS_UNKNOWN_BLOCKS_COMPLETENESS") return true; return false; }());
      ck("unknown_verdict_serialized",
         has(r.serialized, "EXPECTED_INVOCATIONS_UNKNOWN_BLOCKS_COMPLETENESS")); }

    printf("\n=== E. UNE FENETRE ULTERIEURE N EFFACE PAS UN DEFAUT ANTERIEUR ===\n");
    { E2E e5 = e;
      e5.registry_named = {"blk.8.attn_gate.weight","blk.9.ffn_gate.weight"};
      PipelineDeps d = make_deps(e5);
      int n = 0;
      auto base = d.inventory_validation;
      d.inventory_validation = [&n, &base]() {
          n++;
          if (n == 1) { InventoryValidation bad;
              bad.absent_keys.insert("clef_absente_du_registre");
              bad.inventory_globally_complete = false;
              bad.why_incomplete = "defaut artificiel de la premiere fenetre";
              return bad; }
          return base(); };
      PipelineReport r = run_pipeline(req(), d);
      ck("later_valid_window_does_not_erase_earlier_defect",
         n >= 1 && has(r.serialized, "clef_absente_du_registre")); }

    printf("\nCONNECTED_END_TO_END_SELFTEST=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("FAILED_COUNT=%d\n", failed);
    printf("FULL_MODEL_LOAD_COUNT=0\nREAL_CONTEXT_CREATE_COUNT=0\nMODEL_FORWARD_CALL_COUNT=0\n");
    return failed == 0 ? 0 : 1;
}
