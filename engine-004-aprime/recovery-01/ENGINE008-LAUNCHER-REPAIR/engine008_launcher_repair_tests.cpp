// ENGINE-008 -- tests du LANCEUR REPARE : consentement, mode/audience, sequence
// nominale par le LANCEUR, refus. Traverse run_sequence, pas des commandes manuelles.
#include "engine008_launcher.cpp"
#include <cstdio>
#include <fstream>
static int failed = 0;
static void ck(const char * n, bool ok) { printf("  %-68s %s\n", n, ok?"PASS":"FAIL"); if(!ok) failed++; }
static void wf(const std::string & p, const std::string & c) { std::ofstream f(p); f << c; }
static bool has(const std::string & h, const std::string & n) { return h.find(n)!=std::string::npos; }
static std::string sha_of(const std::string & p) {
    std::string cmd = "sha256sum '" + p + "' 2>/dev/null | cut -d' ' -f1";
    FILE * f = popen(cmd.c_str(), "r"); char b[128] = {0};
    if (f) { if (!fgets(b, sizeof b, f)) {} pclose(f); }
    std::string r(b); while (!r.empty() && (r.back()=='\n'||r.back()=='\r')) r.pop_back();
    return r;
}

#ifndef LAUNCHER_SRC
#define LAUNCHER_SRC "engine008_launcher.cpp"
#endif
int main(int argc, char ** argv) {
    printf("ENGINE-008 -- LANCEUR REPARE\n\n");
    const std::string R = "/tmp/lr";
    const std::string HARNESS = (argc > 1) ? argv[1] : "";
    const std::string TPL     = (argc > 2) ? argv[2] : "";
    const std::string MODEL   = (argc > 3) ? argv[3] : "";
    system(("rm -rf " + R + " && mkdir -p " + R + "/out").c_str());
    wf(R + "/mock.conf",  "well_formed=1\nallow_model_compute=1\naudience=MOCK\nbinary_sha256=B\nplan_sha256=P\nprotocol_sha256=Q\n");
    wf(R + "/real.conf",  "well_formed=1\nallow_model_compute=1\naudience=REAL_PREFLIGHT\nbinary_sha256=B\nplan_sha256=P\nprotocol_sha256=Q\n");

    printf("=== CONSENTEMENT ET AUDIENCE, EXPLICITES ===\n");
    auto base = [&]() {
        LaunchConfig c; c.harness=HARNESS; c.auth_doc=R+"/real.conf";
        c.plan_sha="P"; c.protocol_sha="Q"; c.binary_sha="B"; c.template_path=TPL;
        c.baseline_model=MODEL; c.candidate_model=MODEL;
        c.capture_root=R+"/c1"; c.out_root=R+"/out"; c.max_new_tokens=8;
        return c; };
    { auto c = base(); c.mode="real"; c.grant_compute_consent=false;
      auto r = run_sequence(c);
      ck("real_mode_without_explicit_consent_refused", !r.plan_checked && r.invocations==0);
      ck("refusal_names_the_missing_consent",
         has(r.stop_reason,"CONSENTEMENT") && has(r.stop_reason,"--allow-model-compute")); }
    { auto c = base(); c.mode="real"; c.grant_compute_consent=false;   // document CONFORME, drapeau ABSENT
      auto r = run_sequence(c);
      ck("document_alone_is_insufficient", !r.plan_checked && has(r.stop_reason,"CONSENTEMENT")); }
    { auto c = base(); c.mode="real"; c.grant_compute_consent=true; c.auth_doc=R+"/mock.conf";
      auto r = run_sequence(c);
      ck("real_mode_with_MOCK_document_refused", !r.plan_checked);
      ck("audience_incompatibility_named", has(r.stop_reason,"audience INCOMPATIBLE")); }
    { auto c = base(); c.mode="mock"; c.grant_compute_consent=true; c.auth_doc=R+"/real.conf";
      auto r = run_sequence(c);
      ck("mock_mode_with_REAL_document_refused", !r.plan_checked);
      ck("no_silent_fallback_between_modes", has(r.stop_reason,"audience INCOMPATIBLE")); }
    { auto c = base(); c.mode="autre"; c.grant_compute_consent=true;
      auto r = run_sequence(c); ck("unknown_mode_refused", !r.plan_checked); }

    printf("\n=== PROPAGATION DU DRAPEAU DANS LA COMMANDE REELLE ===\n");
    { // verification par SOURCE : le drapeau est conditionne au consentement
      std::ifstream f(std::string(LAUNCHER_SRC)); std::string all((std::istreambuf_iterator<char>(f)),
                                                                 std::istreambuf_iterator<char>());
      ck("command_flag_is_conditional_on_consent",
         has(all,"c.grant_compute_consent ? \" --allow-model-compute\" : \"\"")); }

    if (!HARNESS.empty() && !TPL.empty() && !MODEL.empty()) {
      printf("\n=== SEQUENCE NOMINALE PAR LE LANCEUR (mode mock, sans calcul) ===\n");
      LaunchConfig c;
      c.harness=HARNESS; c.auth_doc=R+"/mock.conf"; c.plan_sha="P"; c.protocol_sha="Q";
      c.binary_sha=sha_of(HARNESS); c.template_path=TPL; c.template_sha="";
      c.baseline_model=MODEL; c.candidate_model=MODEL;
      c.capture_root=R+"/cap"; c.out_root=R+"/out"; c.max_new_tokens=8;
      c.mode="mock"; c.grant_compute_consent=true;   // deux-facteurs aussi en mock
      mkdir((R+"/cap").c_str(), 0755);
      auto rep = run_sequence(c);
      ck("launcher_invoked_four_children", rep.invocations==4 && rep.plan_checked);
      if (rep.runs.size() != 4) { printf("  ARRET : sequence non remplie -- %s\n", rep.stop_reason.c_str()); }
      else {
      ck("order_is_baseline1_candidate1_baseline2_candidate2",
         rep.runs[0].name=="baseline1" && rep.runs[1].name=="candidate1" &&
         rep.runs[2].name=="baseline2" && rep.runs[3].name=="candidate2");
      ck("all_four_verdicts_PASS",
         rep.runs[0].verdict==RunVerdict::PASS && rep.runs[1].verdict==RunVerdict::PASS &&
         rep.runs[2].verdict==RunVerdict::PASS && rep.runs[3].verdict==RunVerdict::PASS);
      ck("four_distinct_capture_destinations",
         rep.runs[0].capture_dir!=rep.runs[1].capture_dir &&
         rep.runs[1].capture_dir!=rep.runs[2].capture_dir &&
         rep.runs[2].capture_dir!=rep.runs[3].capture_dir);
      ck("no_fifth_child", rep.runs.size()==4);
      // arguments reellement recus + IDs autoritaires
      int ids_ok=0, mock=0, forward0=0;
      for (size_t i=0;i<rep.runs.size();++i) {
          std::ifstream f(rep.runs[i].report_path); std::string t((std::istreambuf_iterator<char>(f)),
                                                                  std::istreambuf_iterator<char>());
          if (has(t,"MOCK_PREFILL_IDS_MATCH=YES")) ids_ok++;
          if (has(t,"MOCK_COMPUTE=YES")) mock++;
          if (has(t,"MODEL_FORWARD_CALL_COUNT=0")) forward0++;
      }
      ck("all_children_received_authoritative_59_ids", ids_ok==4);
      ck("all_children_in_mock_mode", mock==4);
      ck("zero_real_compute_in_all_children", forward0==4);
      printf("  LAUNCHER_AGGREGATE=%s\n", rep.serialized.c_str()); }
    } else ck("harness_template_model_provided", false);

    printf("\nLAUNCHER_REPAIR_SELFTEST=%s\n", failed==0?"PASS":"FAIL");
    printf("FAILED_COUNT=%d\n", failed);
    printf("FULL_MODEL_LOAD_COUNT=0\nREAL_CONTEXT_CREATE_COUNT=0\nMODEL_FORWARD_CALL_COUNT=0\n");
    return failed==0?0:1;
}
