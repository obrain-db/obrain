// ENGINE-008 -- PREFLIGHT-REMAINING-CHECKS-17 : tests des SIX conditions de P4'.
// P4' est une PROPOSITION NON ADOPTEE : aucun de ces resultats ne remplace P4.
#include "engine008_p4_checks.h"
#include <cstdio>
#include <string>

static int failed = 0;
static void ck(const char * n, bool ok) {
    printf("  %-66s %s\n", n, ok ? "PASS" : "FAIL");
    if (!ok) failed++;
}
static const ConditionResult * get(const P4CheckReport & r, const char * id) {
    for (const auto & c : r.conditions) if (c.id == id) return &c;
    return nullptr;
}
static P4Inputs full_ok() {
    P4Inputs in;
    in.model_loaded = true; in.registry_acquired = true; in.registry_complete = true;
    in.n_tensors = 500;
    in.types_read_from_tensors = true; in.n_types_read = 500;
    in.n_relevant_weights = 168; in.n_relevant_tq2g128 = 0;
    in.wht_sites_audited = true; in.n_wht_creation_sites = 1;
    in.wht_sites = {"llama-graph.cpp:1497"};
    in.libllama_sha256 = "87c42d5f";
    in.expected_libllama_sha256 = "87c42d5f";
    in.obrain_no_rotation_present = false;
    in.ctx_type = "LLAMA_CONTEXT_TYPE_DEFAULT";
    in.gtype_status = "GTYPE_DERIVED_FROM_VERIFIED_SOURCE_MAPPING";
    return in;
}

int main() {
    printf("ENGINE-008 -- P4' : six conditions (PROPOSITION NON ADOPTEE)\n\n");
    { auto r = check_p4_structural(full_ok());
      ck("six_conditions_reported", r.conditions.size() == 6);
      ck("all_pass_when_everything_verified", r.all_pass && r.overall == CondVerdict::PASS);
      ck("each_condition_has_proof_scope_reason",
         get(r,"C1")->proof.size()>0 && get(r,"C1")->scope.size()>0 &&
         get(r,"C3")->proof.size()>0 && get(r,"C5")->scope.size()>0);
      ck("note_states_not_adopted", r.note.find("NON ADOPTEE") != std::string::npos);
      ck("serialized_marks_adopted_false",
         serialize_p4_checks(r).find("\"adopted\":false") != std::string::npos); }

    printf("\n=== C3 : presence d un poids TQ2_G128 pertinent -> FAIL ===\n");
    { auto in = full_ok(); in.n_relevant_tq2g128 = 1; in.tq2_offenders = {"blk.8.attn_gate.weight"};
      auto r = check_p4_structural(in);
      ck("c3_fails_on_tq2_present", get(r,"C3")->verdict == CondVerdict::FAIL);
      ck("c3_names_the_offender", get(r,"C3")->proof.find("blk.8.attn_gate.weight") != std::string::npos);
      ck("overall_fails", r.overall == CondVerdict::FAIL && !r.all_pass); }

    printf("\n=== registre ABSENT ou INCOMPLET -> UNKNOWN / FAIL ===\n");
    { auto in = full_ok(); in.model_loaded = false; in.registry_acquired = false;
      auto r = check_p4_structural(in);
      ck("c1_unknown_without_model", get(r,"C1")->verdict == CondVerdict::UNKNOWN);
      ck("c1_reason_names_not_exercised",
         get(r,"C1")->reason.find("NOT_EXERCISED") != std::string::npos); }
    { auto in = full_ok(); in.registry_complete = false;
      auto r = check_p4_structural(in);
      ck("c1_fails_on_partial_registry", get(r,"C1")->verdict == CondVerdict::FAIL); }
    { auto in = full_ok(); in.types_read_from_tensors = false; in.n_types_read = 0;
      auto r = check_p4_structural(in);
      ck("c2_unknown_without_real_types", get(r,"C2")->verdict == CondVerdict::UNKNOWN); }
    { auto in = full_ok(); in.type_inferred_from_name = true;
      auto r = check_p4_structural(in);
      ck("c2_fails_when_type_inferred_from_name", get(r,"C2")->verdict == CondVerdict::FAIL);
      ck("c2_reason_cites_the_name_rule",
         get(r,"C2")->reason.find("Q4_K_M") != std::string::npos); }

    printf("\n=== identite AMBIGUE et conditions de chemin ===\n");
    { auto in = full_ok(); in.n_relevant_weights = 0;
      auto r = check_p4_structural(in);
      ck("c3_unknown_when_nothing_enumerated", get(r,"C3")->verdict == CondVerdict::UNKNOWN); }
    { auto in = full_ok(); in.libllama_sha256 = "deadbeef";
      auto r = check_p4_structural(in);
      ck("c5_unknown_on_sha_mismatch", get(r,"C5")->verdict == CondVerdict::UNKNOWN); }
    { auto in = full_ok(); in.obrain_no_rotation_present = true;
      auto r = check_p4_structural(in);
      ck("c5_unknown_when_no_rotation_present",
         get(r,"C5")->verdict == CondVerdict::UNKNOWN &&
         get(r,"C5")->reason.find("OBRAIN_NO_ROTATION") != std::string::npos); }
    { auto in = full_ok(); in.wht_sites_audited = false;
      auto r = check_p4_structural(in);
      ck("c4_unknown_without_audit", get(r,"C4")->verdict == CondVerdict::UNKNOWN); }
    { auto in = full_ok(); in.libllama_sha256.clear();
      auto r = check_p4_structural(in);
      ck("c5_unknown_without_sha", get(r,"C5")->verdict == CondVerdict::UNKNOWN); }

    printf("\n=== C6 ne peut JAMAIS transformer un FAIL en PASS ===\n");
    { auto in = full_ok(); in.n_relevant_tq2g128 = 3;
      auto r = check_p4_structural(in);
      ck("c6_always_pass_by_design", get(r,"C6")->verdict == CondVerdict::PASS);
      ck("c6_does_not_mask_fail", r.overall == CondVerdict::FAIL);
      ck("c6_enumerates_limits",
         get(r,"C6")->proof.find("STRUCTURELLE") != std::string::npos &&
         get(r,"C6")->proof.find("execution") != std::string::npos); }

    printf("\n=== SERIALISATION ===\n");
    { auto r = check_p4_structural(full_ok());
      std::string s = serialize_p4_checks(r);
      ck("serialized_has_schema", s.find("ENGINE008_P4PRIME_CHECKS_v1") != std::string::npos);
      ck("serialized_has_all_ids",
         s.find("\"C1\"") != std::string::npos && s.find("\"C6\"") != std::string::npos);
      ck("serialized_has_verdicts_and_proofs",
         s.find("\"verdict\"") != std::string::npos && s.find("\"proof\"") != std::string::npos &&
         s.find("\"scope\"") != std::string::npos && s.find("\"reason\"") != std::string::npos); }

    printf("\nP4_STRUCTURAL_CHECKS_SELFTEST=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("FAILED_COUNT=%d\n", failed);
    printf("FULL_MODEL_LOAD_COUNT=0\nMODEL_FORWARD_CALL_COUNT=0\n");
    return failed == 0 ? 0 : 1;
}
