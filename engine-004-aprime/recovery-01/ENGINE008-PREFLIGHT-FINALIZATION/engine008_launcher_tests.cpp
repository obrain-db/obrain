// ENGINE-008 -- PREFLIGHT-FINALIZE-10 : tests du lanceur, processus SYNTHETIQUES.
// Aucun modele. Aucun calcul. Les enfants sont des scripts qui ecrivent un
// rapport synthetique explicitement identifie comme tel.
#include "engine008_launcher.cpp"   // meme unite de traduction : acces aux types
#include <cstdio>

static int failed = 0;
static void ck(const char * n, bool ok) {
    printf("  %-64s %s\n", n, ok ? "PASS" : "FAIL");
    if (!ok) failed++;
}
static void write_file(const std::string & p, const std::string & c) {
    std::ofstream f(p); f << c;
}
static LaunchConfig base_cfg(const std::string & root) {
    LaunchConfig c;
    c.child_override = root + "/child.sh";
    c.auth_doc       = root + "/auth.conf";
    c.capture_root   = root + "/cap";
    c.out_root       = root + "/out";
    c.baseline_model  = "/synth/baseline.gguf";
    c.candidate_model = "/synth/candidate.gguf";
    c.template_path   = "/synth/tpl";
    c.max_new_tokens  = 8;
    return c;
}

int main() {
    printf("ENGINE-008 -- PREFLIGHT-FINALIZE-10 : lanceur (processus synthetiques)\n\n");
    const std::string R = "/tmp/l10";
    system(("rm -rf " + R + " && mkdir -p " + R + "/out").c_str());
    // enfant synthetique : ecrit un rapport, et peut echouer au run demande
    write_file(R + "/child.sh",
        "#!/bin/sh\n"
        "MT=$1; RT=$2; CAP=$3; MODEL=$4; MAX=$5\n"
        "# parametres reellement recus, pour verifier la propagation\n"
        "echo \"$MT|$RT|$CAP|$MODEL|$MAX\" >> /tmp/l10/params.log\n"
        "echo \"$MT\" >> /tmp/l10/instances.log\n"
        "if [ -f /tmp/l10/fail_at ] && [ \"$(cat /tmp/l10/instances.log | wc -l)\" = \"$(cat /tmp/l10/fail_at)\" ]; then\n"
        "  echo 'stop_reason=ERROR' ; exit 7\n"
        "fi\n"
        "if [ -f /tmp/l10/no_report ]; then exit 0; fi\n"
        "if [ -f /tmp/l10/malformed ]; then echo 'nimportequoi' ; exit 0; fi\n"
        "echo 'stop_reason=EOS'\n"
        "echo 'generated_token_count=1'\n"
        "echo 'fp_verdict=MATCH'\n"
        "echo 'per_call=CORRELATED'\n"
        "exit 0\n");
    system(("chmod +x " + R + "/child.sh").c_str());
    write_file(R + "/auth.conf", "audience=MOCK\n");

    printf("=== ORDRE NOMINAL ET QUATRE PROCESSUS DISTINCTS ===\n");
    { system(("rm -f " + R + "/params.log " + R + "/instances.log").c_str());
      auto rep = run_sequence(base_cfg(R));
      ck("plan_checked_before_first_launch", rep.plan_checked);
      ck("exactly_four_invocations", rep.invocations == 4);
      ck("four_runs_reported", rep.runs.size() == 4);
      ck("order_is_baseline1_candidate1_baseline2_candidate2",
         rep.runs[0].name == "baseline1" && rep.runs[1].name == "candidate1" &&
         rep.runs[2].name == "baseline2" && rep.runs[3].name == "candidate2");
      ck("each_process_is_distinct_and_fresh",
         rep.runs[0].capture_dir != rep.runs[1].capture_dir &&
         rep.runs[1].capture_dir != rep.runs[2].capture_dir &&
         rep.runs[2].capture_dir != rep.runs[3].capture_dir);
      ck("all_runs_pass", rep.runs[0].verdict == RunVerdict::PASS &&
                          rep.runs[1].verdict == RunVerdict::PASS &&
                          rep.runs[2].verdict == RunVerdict::PASS &&
                          rep.runs[3].verdict == RunVerdict::PASS);
      ck("aggregated_report_serialized",
         rep.serialized.find("ENGINE008_LAUNCH_REPORT_v1") != std::string::npos &&
         rep.serialized.find("\"invocations\":4") != std::string::npos);
      // propagation des parametres
      std::ifstream pf(R + "/params.log"); std::string l1, l2;
      std::getline(pf, l1); std::getline(pf, l2);
      ck("parameters_propagated_model_tag", l1.substr(0, 8) == "baseline" && l2.substr(0, 9) == "candidate");
      ck("parameters_propagated_max_tokens",
         l1.find("|8") != std::string::npos && l2.find("|8") != std::string::npos);
      ck("capture_dir_unique_per_run", l1.find("/baseline/run1") != std::string::npos &&
                                       l2.find("/candidate/run1") != std::string::npos);
      ck("no_fifth_invocation_clause", rep.runs.size() == 4); }

    printf("=== ARRET AU PREMIER ECHEC ===\n");
    { system(("rm -f " + R + "/params.log " + R + "/instances.log").c_str());
      write_file(R + "/fail_at", "2");                       // le 2e run echoue
      auto rep = run_sequence(base_cfg(R));
      ck("stops_after_first_failure", rep.invocations == 2 && rep.stopped_early);
      ck("failed_run_marked_FAIL", rep.runs[1].verdict == RunVerdict::FAIL);
      ck("no_retry_of_failed_run", rep.invocations == 2);
      ck("remaining_runs_marked_NOT_RUN",
         rep.runs[2].verdict == RunVerdict::NOT_RUN && rep.runs[3].verdict == RunVerdict::NOT_RUN);
      ck("failure_artifacts_kept", access((R + "/out/candidate1.report.txt").c_str(), R_OK) == 0);
      system(("rm -f " + R + "/fail_at").c_str()); }

    printf("=== RAPPORT ENFANT ABSENT OU MALFORME ===\n");
    { system(("rm -f " + R + "/params.log " + R + "/instances.log").c_str());
      write_file(R + "/no_report", "1");
      auto rep = run_sequence(base_cfg(R));
      ck("missing_child_report_is_FAIL", rep.runs[0].verdict == RunVerdict::FAIL &&
                                         !rep.runs[0].report_seen);
      system(("rm -f " + R + "/no_report").c_str()); }
    { system(("rm -f " + R + "/params.log " + R + "/instances.log").c_str());
      write_file(R + "/malformed", "1");
      auto rep = run_sequence(base_cfg(R));
      ck("malformed_child_report_is_FAIL", rep.runs[0].verdict == RunVerdict::FAIL &&
                                           rep.runs[0].report_malformed);
      ck("exit_code_alone_is_not_the_evaluation", rep.runs[0].exit_code == 0);
      system(("rm -f " + R + "/malformed").c_str()); }

    printf("=== DESTINATION REUTILISEE ===\n");
    { system(("rm -f " + R + "/params.log " + R + "/instances.log").c_str());
      system(("mkdir -p " + R + "/cap/baseline/run1 && touch " + R + "/cap/baseline/run1/ancien.csv").c_str());
      auto rep = run_sequence(base_cfg(R));
      ck("reused_capture_dir_refused", rep.runs[0].verdict == RunVerdict::FAIL &&
                                       rep.runs[0].reason.find("non vide") != std::string::npos);
      ck("no_invocation_when_capture_refused", rep.invocations == 0);
      system(("rm -rf " + R + "/cap").c_str()); }

    printf("=== GARDE-FOUS DU PLAN ===\n");
    { auto c = base_cfg(R); c.max_new_tokens = 16;
      auto rep = run_sequence(c);
      ck("max_tokens_other_than_8_refused", !rep.plan_checked && rep.invocations == 0); }
    { auto c = base_cfg(R); c.auth_doc = R + "/absent.conf";
      auto rep = run_sequence(c);
      ck("missing_authorization_refused_before_launch", !rep.plan_checked && rep.invocations == 0); }

    printf("\nFOUR_RUN_LAUNCHER_SELFTEST=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("FAILED_COUNT=%d\n", failed);
    printf("FULL_MODEL_LOAD_COUNT=0\nREAL_CONTEXT_CREATE_COUNT=0\nMODEL_FORWARD_CALL_COUNT=0\n");
    return failed == 0 ? 0 : 1;
}
