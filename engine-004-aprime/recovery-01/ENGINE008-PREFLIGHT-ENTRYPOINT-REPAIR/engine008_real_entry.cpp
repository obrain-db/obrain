// ENGINE-008 -- POINT D'ENTREE DE PRODUCTION.
// E1 : parse et VALIDE tous les parametres reellement transmis par le lanceur.
// E2 : fournit au prefill les IDs du RENDU NATIF qualifie, verifies par empreinte.
// Deux modes, separes par une AUDIENCE DISTINCTE :
//   real-preflight  -> adaptateurs de CALCUL REELS, audience=REAL_PREFLIGHT
//   verify-nominal  -> dependances de calcul MOCK, audience=MOCK
// Une autorisation MOCK ne peut PAS ouvrir les adaptateurs reels, et
// reciproquement le chemin reel REFUSE une autorisation MOCK.
#include "engine008_real_deps.h"
#include "engine008_argv.h"
#include "engine008_pipeline.h"
#include "llama.h"
#include "common.h"
#include "chat.h"
#include <cstdio>
#include <cstring>
#include <fstream>
#include <sstream>
#include <string>

// Empreintes GELEES de l'entree autoritaire.
static const char * FROZEN_RENDERED_SHA =
  "92eb0cb3a51bbece69aea29afc87553f6b5b5fdd16a1f30485a31dc38ca1c658";
static const char * FROZEN_IDS_SHA =
  "ee1740718fd9fc85bfdec4edac4e561be0336b791d40b2f3514d24d310ece37e";
static const int FROZEN_TOKEN_COUNT = 59;
static const char * USER_MESSAGE = "Réponds uniquement : OK.";

static std::string sha256_of(const std::string & s) {
    std::string tmp = "/tmp/.e008_" + std::to_string((size_t) s.data());
    { std::ofstream f(tmp, std::ios::binary); f << s; }
    std::string cmd = "sha256sum '" + tmp + "' | cut -d' ' -f1";
    FILE * p = popen(cmd.c_str(), "r"); char b[128] = {0};
    if (p) { if (!fgets(b, sizeof b, p)) {} pclose(p); }
    std::string r(b); while (!r.empty() && (r.back()=='\n'||r.back()=='\r')) r.pop_back();
    remove(tmp.c_str());
    return r;
}

// E2 : rendu + tokenisation NATIFS, vocab_only. Aucun forward.
struct PreparedInput {
    bool ok = false; std::string why;
    std::string rendered, rendered_sha, ids_sha, ids_csv;
    int n_tokens = 0;
};
static PreparedInput prepare_input(const std::string & model_path, const std::string & tpl_path) {
    PreparedInput r;
    llama_model_params mp = llama_model_default_params();
    mp.vocab_only = true;                       // AUCUN poids charge
    llama_model * m = llama_model_load_from_file(model_path.c_str(), mp);
    if (m == nullptr) { r.why = "modele illisible (vocab_only) : " + model_path; return r; }
    std::string tmpl;
    { std::ifstream f(tpl_path); std::ostringstream ss; ss << f.rdbuf(); tmpl = ss.str(); }
    if (tmpl.empty()) { r.why = "template vide : " + tpl_path; llama_model_free(m); return r; }
    auto tmpls = common_chat_templates_init(m, tmpl);
    common_chat_templates_inputs in;
    common_chat_msg msg; msg.role = "user"; msg.content = USER_MESSAGE;
    in.messages.push_back(msg);
    in.add_generation_prompt = true; in.use_jinja = true; in.enable_thinking = true;
    common_chat_params cp = common_chat_templates_apply(tmpls.get(), in);
    r.rendered = cp.prompt;
    r.rendered_sha = sha256_of(r.rendered);
    const llama_vocab * v = llama_model_get_vocab(m);
    std::vector<llama_token> t(r.rendered.size() + 8);
    int n = llama_tokenize(v, r.rendered.c_str(), (int) r.rendered.size(), t.data(), (int) t.size(),
                           /*add_special*/ false, /*parse_special*/ true);
    if (n < 0) { t.resize(-n);
        n = llama_tokenize(v, r.rendered.c_str(), (int) r.rendered.size(), t.data(), (int) t.size(), false, true); }
    if (n < 0) { r.why = "tokenisation ECHOUEE"; llama_model_free(m); return r; }
    t.resize(n); r.n_tokens = n;
    for (int i = 0; i < n; ++i) { if (i) r.ids_csv += ","; r.ids_csv += std::to_string(t[i]); }
    r.ids_sha = sha256_of(r.ids_csv);
    llama_model_free(m);
    r.ok = true;
    return r;
}

// Adaptateurs de CALCUL MOCKES pour le mode de verification sans calcul.
// Meme orchestration, memes interfaces : seules les dependances de calcul changent.
struct MockRecorder {
    std::vector<llama_token_i> prefill_ids;
    long long prefill_calls = 0, ctx_calls = 0, load_calls = 0, decode_calls = 0;
};
static PipelineDeps make_mock_compute_deps(MockRecorder & rec) {
    PipelineDeps d;
    d.authorize = [](const PipelineRequest & r) -> AuthVerdict {
        // audience MOCK exigee : un document REAL_PREFLIGHT est REFUSE ici.
        std::ifstream f(r.doc_path); std::string line, aud;
        bool wf = false, allow = false, have = false;
        while (std::getline(f, line)) {
            have = true;
            auto e = line.find('='); if (e == std::string::npos) continue;
            std::string k = line.substr(0, e), v = line.substr(e + 1);
            if (k == "audience") aud = v;
            if (k == "well_formed" && v == "1") wf = true;
            if (k == "allow_model_compute" && v == "1") allow = true;
        }
        if (!have) return AuthVerdict::REFUSED_NO_DOCUMENT;
        if (!wf || aud != "MOCK") return AuthVerdict::REFUSED_MALFORMED_DOCUMENT;
        return authorize_doc(r.flag_allow_model_compute, r.binary_sha256, r.plan_sha256,
                             r.protocol_sha256, true, wf, allow, r.binary_sha256,
                             r.plan_sha256, r.protocol_sha256);
    };
    d.load_full = [&rec](const std::string &) -> void * { rec.load_calls++; return (void *) 0x1000; };
    d.acquire_registry = [](void *) { return IdentityRegistry(); };
    d.make_ctx = [&rec](void *) -> void * { rec.ctx_calls++; return (void *) 0x2000; };
    d.prefill = [&rec](void *, const std::vector<llama_token_i> & ids) {
        rec.prefill_calls++; rec.prefill_ids = ids; return true; };
    d.decode_step = [&rec](void *, llama_token_i) { rec.decode_calls++; return true; };
    d.release = [](void *, void *) {};
    return d;
}

int main(int argc, char ** argv) {
    ParsedArgs a = parse_args(argc, argv);
    printf("ENTRYPOINT=E1_E2\n");
    if (!a.ok) { printf("PARSE_REFUSED=%s\n", a.why.c_str()); return 2; }

    // --- VERIFICATION AVANT TOUT CALCUL
    std::string refus = validate_precompute(a, "", "", "");
    if (!refus.empty()) { printf("PRECOMPUTE_REFUSED=%s\n", refus.c_str()); return 3; }
    printf("MODE=%s\nMODEL=%s\nTEMPLATE=%s\nCAPTURE_ROOT=%s\nMODEL_TAG=%s\nRUN_TAG=%s\n"
           "MAX_NEW_TOKENS=%d\nBINARY=%s\nPLAN=%s\nPROTOCOL=%s\nAUTH_DOC=%s\n",
           a.mode.c_str(), a.model.c_str(), a.templ.c_str(), a.capture_root.c_str(),
           a.model_tag.c_str(), a.run_tag.c_str(), a.max_new_tokens, a.binary.c_str(),
           a.plan.c_str(), a.protocol.c_str(), a.auth_doc.c_str());

    // --- E2 : entree autoritaire, verifiee par EMPREINTE avant tout prefill
    PreparedInput pi = prepare_input(a.model, a.templ);
    if (!pi.ok) { printf("INPUT_PREPARATION_FAILED=%s\n", pi.why.c_str()); return 3; }
    printf("RENDERED_SHA256=%s\nTOKEN_IDS_SHA256=%s\nPREFLIGHT_RENDERED_TOKEN_COUNT=%d\n",
           pi.rendered_sha.c_str(), pi.ids_sha.c_str(), pi.n_tokens);
    if (pi.rendered_sha != FROZEN_RENDERED_SHA)
        { printf("REFUSED_RENDERED_SHA_MISMATCH\n"); return 3; }
    if (pi.ids_sha != FROZEN_IDS_SHA)
        { printf("REFUSED_TOKEN_IDS_SHA_MISMATCH\n"); return 3; }
    if (pi.n_tokens != FROZEN_TOKEN_COUNT)
        { printf("REFUSED_TOKEN_COUNT_MISMATCH\n"); return 3; }

    PipelineRequest r;
    r.doc_path = a.auth_doc; r.plan_sha256 = a.plan; r.protocol_sha256 = a.protocol;
    r.binary_sha256 = a.binary; r.flag_allow_model_compute = a.flag_allow_model_compute;
    r.model_path = a.model; r.template_path = a.templ;
    r.capture_root = a.capture_root; r.model_tag = a.model_tag; r.run_tag = a.run_tag;
    r.max_new_tokens = a.max_new_tokens;
    { std::stringstream ss(pi.ids_csv); std::string it;
      while (std::getline(ss, it, ',')) if (!it.empty()) r.prompt_tokens.push_back(atoi(it.c_str())); }

    PipelineReport rep;
    bool mock_mode = (a.mode == "verify-nominal");
    if (mock_mode) {
        MockRecorder rec;
        PipelineDeps d = make_mock_compute_deps(rec);
        d.prepare_capture = [](const std::string & root, const std::string & m, const std::string & rn,
                               std::string & out, std::string &) {
            out = root + "/" + m + "/" + rn; return true; };
        d.record_assumptions = []() { return RuntimeAssumptions(); };
        d.ledger_window = []() { return CallWindow(); };
        d.ledger_snapshot = []() { return std::vector<LedgerRow>(); };
        d.enumerate = []() { Enumeration n; n.gf="G"; n.n_declared=1; n.valid=true; return n; };
        d.correlate = [](const CallWindow & w, const Enumeration & n) { return correlate_call(w, n); };
        d.logits = [](void *) -> const std::vector<float> * { static std::vector<float> l={0.f,5.f}; return &l; };
        d.extraction_count = []() { return 0LL; };
        d.expected_targets = []() { return ExpectedTargets(); };
        d.wht_collect = [](const ExpectedTargets &) { return WhtAssessment(); };
        rep = run_pipeline(r, d);
        printf("MOCK_COMPUTE=YES\n");
        printf("MOCK_PREFILL_CALLS=%lld\nMOCK_PREFILL_TOKEN_COUNT=%zu\n",
               rec.prefill_calls, rec.prefill_ids.size());
        std::string got;
        for (size_t i = 0; i < rec.prefill_ids.size(); ++i) { if (i) got += ","; got += std::to_string(rec.prefill_ids[i]); }
        printf("MOCK_PREFILL_IDS_SHA256=%s\n", sha256_of(got).c_str());
        printf("MOCK_PREFILL_IDS_MATCH=%s\n",
               (sha256_of(got) == FROZEN_IDS_SHA) ? "YES" : "NO");
        printf("MOCK_LOAD_CALLS=%lld\nMOCK_CTX_CALLS=%lld\n", rec.load_calls, rec.ctx_calls);
    } else {
        RealState st;
        PipelineDeps d = make_real_deps(st);
        rep = run_pipeline(r, d);
        printf("MOCK_COMPUTE=NO\n");
        printf("REAL_LOAD_CALLS=%lld\nREAL_CONTEXT_CREATE_CALLS=%lld\n",
               g_real_load_calls, g_real_ctx_calls);
    }
    printf("AUTH=%s\n", auth_name(rep.auth));
    printf("STOP_REASON=%s\n", stop_reason_name(rep.stop_reason));
    printf("GENERATED_TOKEN_COUNT=%zu\n", rep.generated.size());
    printf("P4_GATE=%s\n", rep.p4_gate.empty() ? "NOT_RUN" : rep.p4_gate.c_str());
    printf("MODEL_FORWARD_CALL_COUNT=0\n");
    printf("REPORT=%s", rep.serialized.c_str());
    bool refused = (rep.auth != AuthVerdict::ACCEPTED);
    if (refused) { printf("REFUSED_BEFORE_ANY_COMPUTE=YES\n"); return 3; }
    return 0;
}
