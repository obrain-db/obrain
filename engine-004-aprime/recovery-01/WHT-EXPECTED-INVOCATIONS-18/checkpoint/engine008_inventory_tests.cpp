// ENGINE-008 -- WHT-EXPECTED-INVOCATIONS-18 : tests J2 (inventaire) et J1 (invocations).
#include "engine008_inventory.h"
#include <cstdio>
#include <string>

static int failed = 0;
static void ck(const char * n, bool ok) {
    printf("  %-68s %s\n", n, ok ? "PASS" : "FAIL");
    if (!ok) failed++;
}
static NativeTensor T(const char * n, int ty, const void * p) { NativeTensor t; t.name=n; t.type=ty; t.ptr=p; return t; }

int main() {
    printf("ENGINE-008 -- J2 inventaire / J1 invocations\n\n");
    printf("=== J2 : VALIDATION DES CLES CONTRE LE REGISTRE NATIF ===\n");
    { std::vector<std::string> inv = {"blk.8.w", "blk.9.w"};
      std::vector<NativeTensor> reg = { T("blk.8.w",3,(void*)0x1), T("blk.9.w",3,(void*)0x2) };
      auto v = validate_inventory(inv, reg, 3);
      ck("all_keys_valid", v.valid_keys.size()==2 && v.keys.size()==2);
      ck("no_key_in_other_buckets",
         v.absent_keys.empty()&&v.ambiguous_keys.empty()&&v.duplicated_keys.empty()&&
         v.identity_null_keys.empty()&&v.type_not_tq2_keys.empty());
      ck("globally_complete_when_nothing_outside",
         v.inventory_globally_complete && v.tq2_outside_inventory.empty()); }
    { std::vector<std::string> inv = {"blk.8.w", "inconnu"};
      std::vector<NativeTensor> reg = { T("blk.8.w",3,(void*)0x1) };
      auto v = validate_inventory(inv, reg, 3);
      ck("absent_key_detected", v.absent_keys.count("inconnu")==1);
      ck("absent_key_blocks_global_completeness", !v.inventory_globally_complete);
      ck("absent_reason_is_precise",
         v.keys[1].reason.find("registre natif") != std::string::npos); }
    { std::vector<std::string> inv = {"dup"}; std::vector<NativeTensor> reg = { T("dup",3,(void*)0x1) };
      auto v = validate_inventory({"dup","dup"}, reg, 3);
      ck("duplicated_inventory_key_detected", v.duplicated_keys.count("dup")==1); }
    { std::vector<std::string> inv = {"amb"};
      std::vector<NativeTensor> reg = { T("amb",3,(void*)0x1), T("amb",3,(void*)0x9) };
      auto v = validate_inventory(inv, reg, 3);
      ck("ambiguous_key_detected_distinct_identities", v.ambiguous_keys.count("amb")==1); }
    { std::vector<std::string> inv = {"nul"};
      std::vector<NativeTensor> reg = { T("nul",3,nullptr) };
      auto v = validate_inventory(inv, reg, 3);
      ck("null_identity_detected", v.identity_null_keys.count("nul")==1); }
    { std::vector<std::string> inv = {"q4"};
      std::vector<NativeTensor> reg = { T("q4",0,(void*)0x1) };
      auto v = validate_inventory(inv, reg, 3);
      ck("real_type_not_tq2_detected", v.type_not_tq2_keys.count("q4")==1);
      ck("type_carried_in_result", v.keys[0].type == 0); }
    { // poids TQ2_G128 HORS inventaire -> le perimetre declare n est PAS complet
      std::vector<std::string> inv = {"blk.8.w"};
      std::vector<NativeTensor> reg = { T("blk.8.w",3,(void*)0x1), T("blk.9.hors",3,(void*)0x2) };
      auto v = validate_inventory(inv, reg, 3);
      ck("tq2_outside_inventory_detected", v.tq2_outside_inventory.count("blk.9.hors")==1);
      ck("outside_inventory_blocks_global_completeness", !v.inventory_globally_complete);
      ck("incomplete_reason_names_the_cause",
         v.why_incomplete.find("HORS inventaire") != std::string::npos);
      ck("scope_is_declared_perimeter_not_whole_model",
         v.scope.find("DECLARE") != std::string::npos); }

    printf("\n=== J1 : INVOCATIONS ATTENDUES ===\n");
    { std::set<std::string> elig = {"blk.8.w","blk.9.w"};
      std::vector<MatmulOp> ops = { {"blk.8.w",(void*)0x1}, {"blk.9.w",(void*)0x2} };
      auto e = compute_expected_invocations(elig, ops, true);
      ck("expected_invocations_counted", e.known && e.n_invocations==2); }
    { // poids charge mais NON invoque
      std::set<std::string> elig = {"blk.8.w","blk.9.w"};
      std::vector<MatmulOp> ops = { {"blk.8.w",(void*)0x1} };
      auto e = compute_expected_invocations(elig, ops, true);
      ck("loaded_but_not_invoked_distinguished",
         e.known && e.n_invocations==1 && e.invoked_weights.size()==1);
      ck("eligible_weights_still_two", elig.size()==2); }
    { // DEUX invocations du meme poids
      std::set<std::string> elig = {"blk.8.w"};
      std::vector<MatmulOp> ops = { {"blk.8.w",(void*)0x1}, {"blk.8.w",(void*)0x1} };
      auto e = compute_expected_invocations(elig, ops, true);
      ck("multiple_invocations_of_same_weight_detected",
         e.n_invocations==2 && e.weights_invoked_twice.count("blk.8.w")==1); }
    { // poids NON eligible dans le graphe : ignore (nom exact, pas approximatif)
      std::set<std::string> elig = {"blk.8.w"};
      std::vector<MatmulOp> ops = { {"blk.8.w.extra",(void*)0x1}, {"blk.8.w",(void*)0x1} };
      auto e = compute_expected_invocations(elig, ops, true);
      ck("non_eligible_ops_ignored_exact_name_only", e.n_invocations==1); }
    { // enumeration indisponible -> UNKNOWN, jamais une formule
      auto e = compute_expected_invocations({"w"}, {}, false);
      ck("unknown_without_graph_enumeration", !e.known);
      ck("unknown_names_the_missing_link",
         e.why.find("enumeration du graphe soumis indisponible") != std::string::npos);
      ck("no_layer_formula_substituted",
         e.why.find("formule de couches") != std::string::npos); }

    printf("\n=== RECONCILIATION J1 ===\n");
    { std::set<std::string> elig = {"w1","w2"};
      std::vector<MatmulOp> ops = { {"w1",(void*)0x1}, {"w2",(void*)0x2} };
      auto e = compute_expected_invocations(elig, ops, true);
      auto r = reconcile_invocations(e, elig, {"w1"}, 1, 1);
      ck("invocation_without_capture_detected", r.invoked_never_captured.count("w2")==1);
      ck("incomplete_verdict", !r.complete && r.verdict=="INVOCATIONS_WITHOUT_CAPTURE"); }
    { std::set<std::string> elig = {"w1"};
      auto e = compute_expected_invocations(elig, {{"w1",(void*)0x1}}, true);
      auto r = reconcile_invocations(e, elig, {"w1","ZZZ"}, 1, 1);
      ck("capture_unexpected_detected", r.captured_never_invoked.count("ZZZ")==1);
      ck("unexpected_capture_verdict", r.verdict=="CAPTURES_UNEXPECTED"); }
    { std::set<std::string> elig = {"w1"};
      auto e = compute_expected_invocations(elig, {{"w1",(void*)0x1},{"w1",(void*)0x1}}, true);
      auto r = reconcile_invocations(e, elig, {"w1"}, 2, 1);
      ck("ambiguous_reference_detected", r.ambiguous_references.count("w1")==1);
      ck("ambiguous_verdict", r.verdict=="AMBIGUOUS_REFERENCES"); }
    { // reference UNKNOWN INTERDIT COMPLETE
      auto e = compute_expected_invocations({"w"}, {}, false);
      auto r = reconcile_invocations(e, {"w"}, {"w"}, 1, 1);
      ck("unknown_reference_blocks_complete", !r.complete);
      ck("unknown_verdict", r.verdict=="EXPECTED_INVOCATIONS_UNKNOWN_BLOCKS_COMPLETENESS"); }
    { std::set<std::string> elig = {"w1"};
      auto e = compute_expected_invocations(elig, {{"w1",(void*)0x1}}, true);
      auto r = reconcile_invocations(e, elig, {"w1"}, 1, 1);
      ck("complete_when_all_agree", r.complete && r.verdict=="COMPLETE");
      ck("counts_published_separately",
         r.n_eligible_weights==1 && r.n_expected_invocations==1 &&
         r.n_captured_targets==1 && r.n_relations_available==1); }

    printf("\nEXPECTED_INVOCATIONS_SELFTEST=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("INVENTORY_VALIDATION_SELFTEST=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("FAILED_COUNT=%d\n", failed);
    printf("FULL_MODEL_LOAD_COUNT=0\nMODEL_FORWARD_CALL_COUNT=0\n");
    return failed == 0 ? 0 : 1;
}
