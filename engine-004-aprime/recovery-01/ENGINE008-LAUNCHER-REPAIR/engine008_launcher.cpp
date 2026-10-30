// ENGINE-008 -- PREFLIGHT-FINALIZE-10 : lanceur des QUATRE runs.
// Un processus de harnais PAR run. Aucun modele charge ici. Aucun calcul.
//
// Le lanceur n execute que ce que le plan autorise : 4 runs, dans l ordre,
// sans retry, sans cinquieme invocation, et il s arrete au premier echec.
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>
#include <fstream>
#include <sstream>

// ---------------------------------------------------------------- PLAN
struct RunSpec { const char * tag; const char * model_tag; bool is_baseline; };
static const RunSpec PLAN[4] = {
    { "run1", "baseline", true  },
    { "run1", "candidate", false },
    { "run2", "baseline", true  },
    { "run2", "candidate", false },
};
static const char * ORDER[4] = { "baseline1", "candidate1", "baseline2", "candidate2" };

enum class RunVerdict { PASS, FAIL, NOT_CONFIRMED, NOT_RUN };
const char * verdict_name(RunVerdict v) {
    switch (v) {
        case RunVerdict::PASS:           return "PASS";
        case RunVerdict::FAIL:           return "FAIL";
        case RunVerdict::NOT_CONFIRMED:  return "NOT_CONFIRMED";
        default:                         return "NOT_RUN";
    }
}

struct RunResult {
    std::string name, tag, model_tag;
    RunVerdict  verdict = RunVerdict::NOT_RUN;
    int         exit_code = -1;
    std::string reason;
    std::string report_path, capture_dir, stop_reason;
    int         generated = -1;
    std::string fp_verdict, per_call;
    bool        report_seen = false, report_malformed = false;
};

// ---------------------------------------------------------------- UTILITAIRES
static bool dir_empty_or_absent(const std::string & p) {
    struct stat st;
    if (stat(p.c_str(), &st) != 0) return true;          // absente : acceptable
    std::string cmd = "ls -A '" + p + "' 2>/dev/null | head -1";
    FILE * f = popen(cmd.c_str(), "r"); if (!f) return false;
    char buf[512] = {0}; if (!fgets(buf, sizeof buf, f)) buf[0] = '\0';
    pclose(f);
    return buf[0] == '\0';
}
static bool file_sha_ok(const std::string & path, const std::string & expect) {
    if (expect.empty()) return true;
    std::string cmd = "sha256sum '" + path + "' 2>/dev/null | cut -d' ' -f1";
    FILE * f = popen(cmd.c_str(), "r"); if (!f) return false;
    char buf[256] = {0}; if (!fgets(buf, sizeof buf, f)) buf[0] = '\0';
    pclose(f);
    std::string got(buf); while (!got.empty() && (got.back() == '\n' || got.back() == '\r')) got.pop_back();
    return got == expect;
}
static std::string kv(const std::string & text, const std::string & key) {
    std::istringstream is(text); std::string line;
    while (std::getline(is, line)) {
        auto e = line.find('=');
        if (e != std::string::npos && line.substr(0, e) == key) return line.substr(e + 1);
    }
    return "";
}

// ---------------------------------------------------------------- CONFIG
struct LaunchConfig {
    std::string harness;        // binaire du harnais (1 processus par run)
    std::string auth_doc;       // autorisation EXTERNE (jamais creee ici)
    std::string plan_sha, protocol_sha, binary_sha;
    std::string template_path, template_sha;
    std::string baseline_model, baseline_sha;
    std::string candidate_model, candidate_sha;
    std::string capture_root;
    std::string out_root;
    int         max_new_tokens = 8;
    // point d injection pour les tests : programme enfant synthetique
    std::string child_override;
    // --- MODE ET CONSENTEMENT, EXPLICITES (jamais deduits)
    std::string mode = "real";
    bool grant_compute_consent = false;

};

struct LaunchReport {
    std::vector<RunResult> runs;
    bool plan_checked = false, stopped_early = false;
    std::string stop_reason;
    int  invocations = 0;
    std::string serialized;
};

// Audience declaree par le document. Lecture STRICTE.
static std::string doc_audience(const std::string & path) {
    std::ifstream f(path); std::string line, aud;
    while (std::getline(f, line)) {
        auto e = line.find('=');
        if (e != std::string::npos && line.substr(0, e) == "audience") aud = line.substr(e + 1);
    }
    return aud;
}

// ---------------------------------------------------------------- ENFANT
// Construit la commande du run. L environnement est celui du plan :
// OBRAIN_NO_ROTATION doit etre ABSENT (test de PRESENCE, pas de valeur).
static std::string build_command(const LaunchConfig & c, const RunSpec & s,
                                 const std::string & model, const std::string & capture,
                                 const std::string & report) {
    std::string prog = c.child_override.empty() ? c.harness : c.child_override;
    std::string cmd = "env -u OBRAIN_NO_ROTATION ";
    if (c.child_override.empty()) {
        cmd += "'" + prog + "' --mode " + (c.mode == "mock" ? std::string("verify-nominal") : std::string("real-preflight"))
             + " --auth-doc '" + c.auth_doc + "'"
             + " --plan '"     + c.plan_sha + "'"
             + " --protocol '" + c.protocol_sha + "'"
             + " --binary '"   + c.binary_sha + "'"
             + " --model '"    + model + "'"
             + " --template '" + c.template_path + "'"
             + " --capture-root '" + capture + "'"
             + " --model-tag '" + s.model_tag + "' --run-tag '" + s.tag + "'"
             + " --max-new-tokens " + std::to_string(c.max_new_tokens)
             // Transmis SEULEMENT si accorde ET valide. Jamais inconditionnel.
             + (c.grant_compute_consent ? " --allow-model-compute" : "");
    } else {
        // enfant synthetique : recoit les memes parametres, par variables
        cmd += "'" + prog + "'"
             + " '" + s.model_tag + "' '" + s.tag + "' '" + capture + "'"
             + " '" + model + "' " + std::to_string(c.max_new_tokens);
    }
    cmd += " > '" + report + "' 2>&1";
    return cmd;
}

// Un rapport enfant est lu et VALIDE : le seul code de sortie ne suffit pas.
static RunVerdict judge(const RunResult & r) {
    if (r.exit_code != 0) return RunVerdict::FAIL;
    if (!r.report_seen) return RunVerdict::FAIL;             // rapport enfant absent
    if (r.report_malformed) return RunVerdict::FAIL;         // rapport malforme
    if (r.stop_reason.empty()) return RunVerdict::NOT_CONFIRMED;
    return RunVerdict::PASS;
}

LaunchReport run_sequence(const LaunchConfig & c) {
    LaunchReport R;
    // --- verification des autorites et du plan AVANT le premier lancement
    if (c.harness.empty() && c.child_override.empty()) { R.stop_reason = "harnais non specifie"; return R; }
    if (!c.child_override.empty() && c.harness.empty()) {
        // chemin synthetique : on verifie seulement l existence du programme
        if (access(c.child_override.c_str(), X_OK) != 0) { R.stop_reason = "enfant introuvable"; return R; }
    } else if (access(c.harness.c_str(), X_OK) != 0) { R.stop_reason = "harnais introuvable"; return R; }
    if (c.auth_doc.empty() || access(c.auth_doc.c_str(), R_OK) != 0) { R.stop_reason = "autorisation absente"; return R; }
    // --- MODE ET AUDIENCE : explicites, sans fallback
    if (c.mode != "real" && c.mode != "mock") { R.stop_reason = "mode inconnu : " + c.mode; return R; }
    { const std::string aud = doc_audience(c.auth_doc);
      const std::string want = (c.mode == "real") ? "REAL_PREFLIGHT" : "MOCK";
      if (aud != want) { R.stop_reason = "audience INCOMPATIBLE : mode=" + c.mode +
          " exige audience=" + want + ", document=" + (aud.empty() ? "<aucune>" : aud); return R; } }
    // --- CONSENTEMENT : exige EXPLICITEMENT en mode reel. Le document SEUL ne
    // suffit pas ; le drapeau SEUL non plus.
    if (!c.grant_compute_consent) {
        R.stop_reason = "CONSENTEMENT EXPLICITE ABSENT : --allow-model-compute est exige, "
                        "dans les DEUX modes (le document seul ne suffit pas)"; return R; }
    if (!c.binary_sha.empty() && !file_sha_ok(c.harness, c.binary_sha) && c.child_override.empty()) {
        R.stop_reason = "identite du harnais non conforme"; return R; }
    if (!c.baseline_sha.empty()  && !file_sha_ok(c.baseline_model,  c.baseline_sha))  { R.stop_reason = "identite baseline non conforme"; return R; }
    if (!c.candidate_sha.empty() && !file_sha_ok(c.candidate_model, c.candidate_sha)) { R.stop_reason = "identite candidat non conforme"; return R; }
    if (!c.template_sha.empty()  && !file_sha_ok(c.template_path,   c.template_sha))  { R.stop_reason = "identite template non conforme"; return R; }
    if (c.max_new_tokens != 8) { R.stop_reason = "maximum de tokens different du plan (8)"; return R; }
    R.plan_checked = true;

    for (int i = 0; i < 4; ++i) {
        const RunSpec & s = PLAN[i];
        RunResult r; r.name = ORDER[i]; r.tag = s.tag; r.model_tag = s.model_tag;
        const std::string model = s.is_baseline ? c.baseline_model : c.candidate_model;
        r.capture_dir = c.capture_root + "/" + s.model_tag + "/" + s.tag;
        r.report_path = c.out_root + "/" + std::string(ORDER[i]) + ".report.txt";

        // destination unique ET initialement vide
        if (!dir_empty_or_absent(r.capture_dir)) {
            r.verdict = RunVerdict::FAIL; r.reason = "destination de capture non vide";
            R.runs.push_back(r); R.stopped_early = true; R.stop_reason = r.reason; return R;
        }
        mkdir(r.capture_dir.c_str(), 0755);

        std::string cmd = build_command(c, s, model, r.capture_dir, r.report_path);
        int rc = system(cmd.c_str());
        R.invocations++;
        r.exit_code = (rc == -1) ? -1 : WEXITSTATUS(rc);

        // lecture du rapport enfant
        std::ifstream f(r.report_path);
        if (f) {
            std::ostringstream ss; ss << f.rdbuf();
            std::string text = ss.str();
            r.report_seen = !text.empty();
            r.stop_reason  = kv(text, "stop_reason");
            std::string g  = kv(text, "generated_token_count");
            if (!g.empty()) r.generated = atoi(g.c_str());
            r.fp_verdict   = kv(text, "fp_verdict");
            r.per_call     = kv(text, "per_call");
            // un rapport doit au moins porter stop_reason ou un verdict explicite
            if (r.report_seen && r.stop_reason.empty() && text.find("AUTH=") == std::string::npos)
                r.report_malformed = true;
        }
        r.verdict = judge(r);
        if (r.verdict != RunVerdict::PASS && r.reason.empty()) r.reason = "verdict non PASS";

        bool stop = (r.verdict == RunVerdict::FAIL);
        R.runs.push_back(r);
        if (stop) { R.stopped_early = true; R.stop_reason = "arret au premier echec : " + r.name; break; }
    }
    // marquer explicitement les runs non lances
    for (int i = (int) R.runs.size(); i < 4; ++i) {
        RunResult r; r.name = ORDER[i]; r.verdict = RunVerdict::NOT_RUN;
        r.reason = "non lance : sequence arretee"; R.runs.push_back(r);
    }
    // rapport agrege
    std::string o = "{\"schema\":\"ENGINE008_LAUNCH_REPORT_v1\",\"runs\":[";
    for (size_t i = 0; i < R.runs.size(); ++i) {
        const RunResult & r = R.runs[i];
        if (i) o += ",";
        o += "{\"name\":\"" + r.name + "\",\"model_tag\":\"" + r.model_tag + "\","
             "\"verdict\":\"" + verdict_name(r.verdict) + "\","
             "\"exit_code\":" + std::to_string(r.exit_code) + ","
             "\"report_seen\":" + (r.report_seen ? "true" : "false") + ","
             "\"report_malformed\":" + (r.report_malformed ? "true" : "false") + ","
             "\"stop_reason\":\"" + r.stop_reason + "\","
             "\"generated_token_count\":" + std::to_string(r.generated) + ","
             "\"fp_verdict\":\"" + r.fp_verdict + "\","
             "\"per_call\":\"" + r.per_call + "\","
             "\"reason\":\"" + r.reason + "\"}";
    }
    o += "],\"invocations\":" + std::to_string(R.invocations)
       + ",\"stopped_early\":" + (R.stopped_early ? "true" : "false")
       + ",\"plan_checked\":" + (R.plan_checked ? "true" : "false")
       + ",\"stop_reason\":\"" + R.stop_reason + "\"}\n";
    R.serialized = o;
    return R;
}
