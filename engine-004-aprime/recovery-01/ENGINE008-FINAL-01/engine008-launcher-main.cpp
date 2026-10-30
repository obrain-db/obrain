// ENGINE-008 -- E3 : LANCEUR DE PRODUCTION. Executable distinct de t-launcher.
#include "engine008_launcher.cpp"   // definitions completes : LaunchConfig, LaunchReport
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>
int main(int argc, char ** argv) {
    LaunchConfig c;
    c.max_new_tokens = 8;
    for (int i = 1; i < argc; ++i) {
        std::string a = argv[i];
        auto next = [&]() -> std::string { return (i + 1 < argc) ? argv[++i] : std::string(); };
        if      (a == "--harness") c.harness = next();
        else if (a == "--auth-doc") c.auth_doc = next();
        else if (a == "--plan") c.plan_sha = next();
        else if (a == "--protocol") c.protocol_sha = next();
        else if (a == "--binary") c.binary_sha = next();
        else if (a == "--template") c.template_path = next();
        else if (a == "--template-sha") c.template_sha = next();
        else if (a == "--baseline") c.baseline_model = next();
        else if (a == "--baseline-sha") c.baseline_sha = next();
        else if (a == "--candidate") c.candidate_model = next();
        else if (a == "--candidate-sha") c.candidate_sha = next();
        else if (a == "--capture-root") c.capture_root = next();
        else if (a == "--out") c.out_root = next();
        else if (a == "--mode") c.mode = next();
        else if (a == "--allow-model-compute") c.grant_compute_consent = true;
        else { printf("LAUNCHER_REFUSED=argument inconnu : %s\n", a.c_str()); return 2; }
    }
    if (c.harness.empty() || c.auth_doc.empty() || c.capture_root.empty() || c.out_root.empty()
        || c.baseline_model.empty() || c.candidate_model.empty()) {
        printf("LAUNCHER_REFUSED=parametres obligatoires manquants\n"); return 2; }
    printf("LAUNCHER_MODE=%s\nLAUNCHER_CONSENT=%s\n", c.mode.c_str(),
           c.grant_compute_consent ? "GRANTED" : "ABSENT");
    LaunchReport R = run_sequence(c);
    printf("LAUNCHER_INVOCATIONS=%d\nLAUNCHER_STOPPED_EARLY=%s\nLAUNCHER_PLAN_CHECKED=%s\n",
           R.invocations, R.stopped_early ? "YES" : "NO", R.plan_checked ? "YES" : "NO");
    printf("LAUNCHER_STOP_REASON=%s\n", R.stop_reason.c_str());
    for (const auto & r : R.runs)
        printf("RUN %-11s model=%-9s verdict=%-14s exit=%d report_seen=%s malformed=%s reason=%s\n",
               r.name.c_str(), r.model_tag.c_str(), verdict_name(r.verdict), r.exit_code,
               r.report_seen ? "Y" : "N", r.report_malformed ? "Y" : "N", r.reason.c_str());
    printf("LAUNCHER_REPORT=%s", R.serialized.c_str());
    if (!R.plan_checked) return 3;
    for (const auto & r : R.runs) if (r.verdict == RunVerdict::FAIL) return 4;
    return 0;
}
