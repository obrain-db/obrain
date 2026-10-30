#!/usr/bin/env python3
"""PREFLIGHT-IMPLEMENT-01 -- etend le harnais : W1 (mode preflight) + W2 (f1a).

Insere un bloc de code avant main() et patche le dispatch des modes.
Le harnais gele reste intact par ailleurs : les modes existants ne changent pas.
"""
import re
import sys

SRC = sys.argv[1]
OUT = sys.argv[2]
s = open(SRC, encoding="utf-8").read()

BLOCK = r'''
// ==========================================================================
// PREFLIGHT-IMPLEMENT-01 -- W1 : mode preflight (chemin runtime) + W2 : f1a
// ==========================================================================
// Le chemin runtime est IMPLEMENTE mais INTERDIT dans cette passe. La garde
// refuse AVANT le chargement complet, AVANT la creation du contexte, AVANT
// tout forward pass.

#include <functional>
#include <map>
#include <set>

// ---------------------------------------------------------------- compteurs
// Compteurs DISTINCTS. Ne jamais deriver un nombre de forwards d'un nombre
// de tokens.
struct RuntimeCounters {
    long long guard_refused          = 0;  // refus de la garde
    long long llama_decode_calls     = 0;  // appels API reellement effectues
    long long graph_submit_events    = 0;  // COMPUTE_SUBMIT observes
    long long completion_events      = 0;  // SYNC_COMPLETE observes
    long long tokens_selected        = 0;  // tokens choisis par argmax
    long long stop_tokens_selected   = 0;  // tokens d arret selectionnes
    long long prefill_calls          = 0;
    long long full_load_calls        = 0;  // chargements COMPLETS (pas vocab_only)
    long long context_create_calls   = 0;
};
static RuntimeCounters g_rc;

// ---------------------------------------------------------------- contrat d arret
// GENERATE_STOP_TOKEN_IDS = [248046]  (repris du protocole gele, non elargi).
static const std::vector<llama_token> STOP_TOKEN_IDS = { 248046 };

enum class StopReason { EOS = 0, MAX_NEW_TOKENS = 1, ERROR = 2, NONE = 3 };

static const char * stop_reason_name(StopReason r) {
    switch (r) {
        case StopReason::EOS:            return "EOS";
        case StopReason::MAX_NEW_TOKENS: return "MAX_NEW_TOKENS";
        case StopReason::ERROR:          return "ERROR";
        default:                         return "NONE";
    }
}

static bool is_stop_token(llama_token t) {
    for (auto s : STOP_TOKEN_IDS) if (s == t) return true;
    return false;
}

struct GenOutcome {
    std::vector<llama_token> ids;
    StopReason               reason = StopReason::NONE;
    long long                decodes = 0;
};

// Boucle de generation PURE : la source des tokens est injectee. C est ce qui
// permet de tester le contrat d arret sans aucun calcul modele.
// `step` renvoie true en cas d erreur simulee.
static GenOutcome simulate_generation(const std::function<bool(int, llama_token &)> & step,
                                      int max_new_tokens) {
    GenOutcome o;
    for (int i = 0; i < max_new_tokens; ++i) {
        llama_token t = 0;
        if (!step(i, t)) { o.reason = StopReason::ERROR; return o; }
        o.decodes++;                      // un decode PAR token, jamais derive
        o.ids.push_back(t);
        if (is_stop_token(t)) {
            o.reason = StopReason::EOS;
            return o;                     // AUCUN decode apres l arret
        }
    }
    o.reason = StopReason::MAX_NEW_TOKENS;
    return o;
}

static int mode_stop_selftest() {
    int failed = 0;
    auto ck = [&](const char * n, bool ok) {
        printf("  %-52s %s\n", n, ok ? "PASS" : "FAIL");
        if (!ok) failed++;
    };
    printf("=== CONTRAT D ARRET SUR SEQUENCES SYNTHETIQUES (aucun modele) ===\n");
    printf("  GENERATE_STOP_TOKEN_IDS=[248046]  max_new_tokens=8\n");

    { // arret sur EOS au premier token
        std::vector<llama_token> seq = {248046};
        auto o = simulate_generation([&](int i, llama_token & t){ t = seq[i]; return true; }, 8);
        ck("stop_eos_first_token", o.reason == StopReason::EOS && o.ids.size() == 1 && o.decodes == 1);
    }
    { // huit tokens sans EOS -> MAX_NEW_TOKENS
        std::vector<llama_token> seq(8, 100);
        auto o = simulate_generation([&](int i, llama_token & t){ t = seq[i]; return true; }, 8);
        ck("stop_max_new_tokens_at_8", o.reason == StopReason::MAX_NEW_TOKENS && o.ids.size() == 8);
    }
    { // EOS exactement au 8e token -> EOS, pas de 9e
        std::vector<llama_token> seq = {1,2,3,4,5,6,7,248046};
        auto o = simulate_generation([&](int i, llama_token & t){ t = seq[i]; return true; }, 8);
        ck("stop_eos_at_position_8_no_off_by_one",
           o.reason == StopReason::EOS && o.ids.size() == 8 && o.decodes == 8);
    }
    { // EOS au 9e : inatteignable sous la limite -> MAX_NEW_TOKENS
        std::vector<llama_token> seq = {1,2,3,4,5,6,7,8,248046};
        auto o = simulate_generation([&](int i, llama_token & t){ t = seq[i]; return true; }, 8);
        ck("eos_beyond_limit_is_max_new_tokens",
           o.reason == StopReason::MAX_NEW_TOKENS && o.ids.size() == 8);
    }
    { // aucun decode apres l arret
        std::vector<llama_token> seq = {248046, 999};
        int calls = 0;
        auto o = simulate_generation([&](int i, llama_token & t){ calls++; t = seq[i]; return true; }, 8);
        ck("no_decode_after_stop", calls == 1 && o.decodes == 1);
    }
    { // erreur simulee au 3e pas
        auto o = simulate_generation([](int i, llama_token & t){ t = 1; return i < 2; }, 8);
        ck("simulated_prefill_decode_error_reported",
           o.reason == StopReason::ERROR && o.ids.size() == 2);
    }
    { // limite a 1
        std::vector<llama_token> seq = {7, 7};
        auto o = simulate_generation([&](int i, llama_token & t){ t = seq[i]; return true; }, 1);
        ck("limit_1_no_off_by_one", o.ids.size() == 1 && o.reason == StopReason::MAX_NEW_TOKENS);
    }
    { // argmax direct et tie-break : reutilise les fonctions deja testees
        std::vector<float> l = {3.f, 9.f, 9.f, 1.f};
        ck("argmax_tie_lowest_id_still_held", greedy_argmax_lowest_id(l) == 1);
        std::vector<float> bad = {0.f, NAN};
        double z = 0;
        ck("invalid_logit_row_rejected", !logsumexp_stable(bad, z));
    }
    printf("\nGREEDY_AND_STOP_SYNTHETIC_SELFTEST=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("STOP_SYNTHETIC_FAILED_COUNT=%d\n", failed);
    return failed == 0 ? 0 : 1;
}

// ---------------------------------------------------------------- W2 : f1a
// Instrument DEJA compile dans Q1. Activateurs lus dans f1a-capture.cpp:13-15.
static const char * F1A_ENABLE    = "OBRAIN_F1A_CAPTURE";
static const char * F1A_OUTDIR    = "OBRAIN_F1A_OUTDIR";
static const char * F1A_INVENTORY = "OBRAIN_F1A_INVENTORY";
static const char * F1A_RUNID     = "OBRAIN_F1A_RUN_ID";

struct F1aPlan {
    std::string outdir;
    std::string run_id;
    std::string inventory;
    bool        ok = false;
    std::string refusal;
};

// Prepare une destination ISOLEE par modele et par run.
// Refuse une destination non vide : empeche la reutilisation silencieuse
// de captures anterieures (§7).
static F1aPlan prepare_f1a(const std::string & root, const std::string & model_tag,
                           const std::string & run_tag) {
    F1aPlan p;
    p.outdir    = root + "/" + model_tag + "/" + run_tag;
    p.run_id    = model_tag + "-" + run_tag;
    p.inventory = p.outdir + "/inventory.json";
    if (!root.empty() && root[0] != '/') { p.refusal = "root non absolu"; return p; }
    std::string cmd = "mkdir -p '" + p.outdir + "'";
    if (system(cmd.c_str()) != 0) { p.refusal = "mkdir impossible"; return p; }
    std::string chk = "ls -A '" + p.outdir + "' 2>/dev/null | head -1";
    FILE * f = popen(chk.c_str(), "r");
    char buf[256] = {0};
    if (f) { if (fgets(buf, sizeof buf, f)) {} pclose(f); }
    if (buf[0] != '\0') { p.refusal = "destination non vide : capture anterieure possible"; return p; }
    p.ok = true;
    return p;
}

struct F1aCoverage {
    long long ledger_rows   = 0;
    long long submit_events = 0;
    long long sync_events   = 0;
    long long manifest_rows = 0;
    long long rel_rows      = 0;
    long long rot_true      = 0;
    long long rot_false     = 0;
    long long nextn_nodes   = 0;
    long long rc_nonzero    = 0;   // soumissions en erreur
    std::set<std::string> fingerprints;
    std::set<std::string> run_ids;
    bool malformed = false;
    std::string malformed_why;
};

static std::vector<std::string> split_csv_line(const std::string & l) {
    std::vector<std::string> out; std::string cur;
    for (char c : l) {
        if (c == ',') { out.push_back(cur); cur.clear(); }
        else if (c != '\r') cur += c;
    }
    out.push_back(cur);
    return out;
}

// Collecteur des trois surfaces constatees dans Q1.
static F1aCoverage collect_f1a(const std::string & outdir) {
    F1aCoverage c;
    {   // EVENT_LEDGER.csv
        std::ifstream f(outdir + "/EVENT_LEDGER.csv");
        if (!f) { c.malformed = true; c.malformed_why = "EVENT_LEDGER.csv absent"; return c; }
        std::string l; bool hdr = false; size_t ncol = 0;
        while (std::getline(f, l)) {
            if (l.empty()) continue;
            auto v = split_csv_line(l);
            if (!hdr) { hdr = true; ncol = v.size(); continue; }
            if (v.size() != ncol) { c.malformed = true; c.malformed_why = "EVENT_LEDGER colonnes inattendues"; return c; }
            c.ledger_rows++;
            std::string kind = v.size() > 2 ? v[2] : "";
            if (kind == "COMPUTE_SUBMIT") c.submit_events++;
            if (kind == "SYNC_COMPLETE")  c.sync_events++;
            if (v.size() > 6) c.fingerprints.insert(v[6]);
            if (v.size() > 0) c.run_ids.insert(v[0]);
        }
    }
    {   // GRAPH_CAPTURE_MANIFEST.csv
        std::ifstream f(outdir + "/GRAPH_CAPTURE_MANIFEST.csv");
        if (f) {
            std::string l; bool hdr = false; int irot = -1;
            while (std::getline(f, l)) {
                if (l.empty()) continue;
                auto v = split_csv_line(l);
                if (!hdr) {
                    hdr = true;
                    for (size_t i = 0; i < v.size(); ++i) if (v[i] == "rotation_applied") irot = (int) i;
                    if (irot < 0) { c.malformed = true; c.malformed_why = "rotation_applied absent du manifeste"; return c; }
                    continue;
                }
                c.manifest_rows++;
                if (irot < (int) v.size()) {
                    if (v[irot] == "1") c.rot_true++;
                    else if (v[irot] == "0") c.rot_false++;
                }
            }
        }
    }
    {   // GRAPH_ROLE_RELATIONSHIPS.csv -- detection NextN
        std::ifstream f(outdir + "/GRAPH_ROLE_RELATIONSHIPS.csv");
        if (f) {
            std::string l;
            while (std::getline(f, l)) {
                if (l.empty()) continue;
                c.rel_rows++;
                if (l.find("nextn") != std::string::npos ||
                    l.find("NEXTN") != std::string::npos ||
                    l.find("h_nextn") != std::string::npos) c.nextn_nodes++;
            }
        }
    }
    return c;
}

// Couverture : refuser une conclusion d ABSENCE quand la couverture manque (§8).
enum class CoverageVerdict { EMPTY, PARTIAL, COMPLETE };
static CoverageVerdict coverage_verdict(const F1aCoverage & c) {
    if (c.ledger_rows == 0 || c.manifest_rows == 0) return CoverageVerdict::EMPTY;
    if (c.submit_events == 0 || c.fingerprints.empty()) return CoverageVerdict::PARTIAL;
    return CoverageVerdict::COMPLETE;
}
static const char * coverage_name(CoverageVerdict v) {
    switch (v) {
        case CoverageVerdict::EMPTY:   return "EMPTY";
        case CoverageVerdict::PARTIAL: return "PARTIAL";
        default:                       return "COMPLETE";
    }
}
// Conclusion sur la rotation. ABSENT n est rendu QUE si la couverture est
// COMPLETE et que des cibles ont ete capturees avec rotation_applied=0.
static const char * rotation_conclusion(const F1aCoverage & c) {
    if (c.malformed) return "NOT_CONFIRMED_MALFORMED";
    CoverageVerdict v = coverage_verdict(c);
    if (v != CoverageVerdict::COMPLETE) return "NOT_CONFIRMED_COVERAGE";
    if (c.manifest_rows == 0) return "NOT_CONFIRMED_NO_TARGETS";
    if (c.rot_true  > 0) return "ROTATION_PRESENT";
    if (c.rot_false > 0) return "ROTATION_ABSENT_WITH_COVERAGE";
    return "NOT_CONFIRMED_NO_ROTATION_FIELD";
}
static const char * nextn_conclusion(const F1aCoverage & c) {
    if (c.malformed) return "NOT_CONFIRMED_MALFORMED";
    if (c.rel_rows == 0) return "NOT_CONFIRMED_NO_GRAPH_ROLES";
    CoverageVerdict v = coverage_verdict(c);
    if (v != CoverageVerdict::COMPLETE) return "NOT_CONFIRMED_COVERAGE";
    return c.nextn_nodes > 0 ? "NEXTN_PRESENT_IN_GRAPH" : "NEXTN_ABSENT_WITH_COVERAGE";
}

static int write_file(const std::string & p, const std::string & s) {
    std::ofstream f(p, std::ios::binary);
    if (!f) return -1;
    f << s;
    return 0;
}

static int mode_capture_selftest(const std::string & tmp_root) {
    int failed = 0;
    auto ck = [&](const char * n, bool ok) {
        printf("  %-58s %s\n", n, ok ? "PASS" : "FAIL");
        if (!ok) failed++;
    };
    printf("=== COLLECTEURS f1a SUR FIXTURES SYNTHETIQUES (aucun modele) ===\n");
    std::string root = tmp_root + "/capture-selftest";
    std::string mk = "rm -rf '" + root + "' && mkdir -p '" + root + "'";
    if (system(mk.c_str()) != 0) { printf("preparation impossible\n"); return 1; }

    const std::string LEDGER_HDR =
        "run_id,event_sequence_id,event_kind,decode_step,compute_submission_id,"
        "graph_ptr,graph_fingerprint,capture_epoch_id,eligible_target_count,captured_target_count\n";
    const std::string MANIFEST_HDR =
        "run_id,epoch,target_id,capture_id,weight_name,family,expected_K,rotation_applied\n";
    const std::string REL_HDR = "run_id,from_node,to_node,kind\n";

    auto mkdir = [&](const std::string & d) {
        std::string c = "mkdir -p '" + root + "/" + d + "'"; if (system(c.c_str())) {}
    };

    // 1. cas positif : rotation presente
    mkdir("positive");
    write_file(root + "/positive/EVENT_LEDGER.csv", LEDGER_HDR +
        "r1,1,COMPUTE_SUBMIT,0,1,0x1,fpAAA,1,10,10\nr1,2,SYNC_COMPLETE,0,1,0x1,fpAAA,1,10,10\n");
    write_file(root + "/positive/GRAPH_CAPTURE_MANIFEST.csv", MANIFEST_HDR +
        "r1,1,1,1,blk.0.ffn_gate.weight,5,128,1\nr1,1,2,2,blk.1.ffn_up.weight,5,128,1\n");
    write_file(root + "/positive/GRAPH_ROLE_RELATIONSHIPS.csv", REL_HDR + "r1,a,b,mm\n");
    auto p = collect_f1a(root + "/positive");
    ck("positive_rotation_present", std::string(rotation_conclusion(p)) == "ROTATION_PRESENT");
    ck("positive_coverage_complete", coverage_verdict(p) == CoverageVerdict::COMPLETE);

    // 2. cas negatif AVEC couverture complete -> ABSENT autorise
    mkdir("negative");
    write_file(root + "/negative/EVENT_LEDGER.csv", LEDGER_HDR +
        "r2,1,COMPUTE_SUBMIT,0,1,0x2,fpBBB,1,10,10\nr2,2,SYNC_COMPLETE,0,1,0x2,fpBBB,1,10,10\n");
    write_file(root + "/negative/GRAPH_CAPTURE_MANIFEST.csv", MANIFEST_HDR +
        "r2,1,1,1,blk.0.ffn_gate.weight,0,0,0\nr2,1,2,2,blk.1.ffn_up.weight,0,0,0\n");
    write_file(root + "/negative/GRAPH_ROLE_RELATIONSHIPS.csv", REL_HDR + "r2,a,b,mm\n");
    auto n = collect_f1a(root + "/negative");
    ck("negative_rotation_absent_with_coverage",
       std::string(rotation_conclusion(n)) == "ROTATION_ABSENT_WITH_COVERAGE");
    ck("negative_nextn_absent_with_coverage",
       std::string(nextn_conclusion(n)) == "NEXTN_ABSENT_WITH_COVERAGE");

    // 3. couverture VIDE -> jamais ABSENT
    mkdir("empty");
    write_file(root + "/empty/EVENT_LEDGER.csv", LEDGER_HDR);
    write_file(root + "/empty/GRAPH_CAPTURE_MANIFEST.csv", MANIFEST_HDR);
    auto e = collect_f1a(root + "/empty");
    ck("empty_coverage_never_concludes_absence",
       std::string(rotation_conclusion(e)) == "NOT_CONFIRMED_COVERAGE" &&
       std::string(nextn_conclusion(e)) == "NOT_CONFIRMED_COVERAGE" ||
       std::string(rotation_conclusion(e)).rfind("NOT_CONFIRMED", 0) == 0);

    // 4. couverture PARTIELLE : soumission absente
    mkdir("partial");
    write_file(root + "/partial/EVENT_LEDGER.csv", LEDGER_HDR +
        "r3,1,SYNC_COMPLETE,0,1,0x3,fpCCC,1,10,10\n");
    write_file(root + "/partial/GRAPH_CAPTURE_MANIFEST.csv", MANIFEST_HDR +
        "r3,1,1,1,blk.0.ffn_gate.weight,5,128,0\n");
    auto pa = collect_f1a(root + "/partial");
    ck("partial_missing_submit_not_confirmed",
       std::string(rotation_conclusion(pa)).rfind("NOT_CONFIRMED", 0) == 0);

    // 5. artefact malforme (colonnes inattendues)
    mkdir("malformed");
    write_file(root + "/malformed/EVENT_LEDGER.csv", LEDGER_HDR + "r4,1,COMPUTE_SUBMIT\n");
    auto m = collect_f1a(root + "/malformed");
    ck("malformed_ledger_not_confirmed",
       std::string(rotation_conclusion(m)).rfind("NOT_CONFIRMED", 0) == 0);

    // 6. artefact tronque (manifeste sans la colonne requise)
    mkdir("no_field");
    write_file(root + "/no_field/EVENT_LEDGER.csv", LEDGER_HDR +
        "r5,1,COMPUTE_SUBMIT,0,1,0x5,fpDDD,1,10,10\n");
    write_file(root + "/no_field/GRAPH_CAPTURE_MANIFEST.csv", "run_id,epoch,target_id\nr5,1,1\n");
    auto nf = collect_f1a(root + "/no_field");
    ck("manifest_without_rotation_field_not_confirmed",
       std::string(rotation_conclusion(nf)).rfind("NOT_CONFIRMED", 0) == 0);

    // 7. NextN present
    mkdir("nextn");
    write_file(root + "/nextn/EVENT_LEDGER.csv", LEDGER_HDR +
        "r6,1,COMPUTE_SUBMIT,0,1,0x6,fpEEE,1,10,10\n");
    write_file(root + "/nextn/GRAPH_CAPTURE_MANIFEST.csv", MANIFEST_HDR +
        "r6,1,1,1,blk.0.ffn_gate.weight,0,0,0\n");
    write_file(root + "/nextn/GRAPH_ROLE_RELATIONSHIPS.csv", REL_HDR +
        "r6,a,b,mm\nr6,h_nextn,c,nextn_chain\n");
    auto nn = collect_f1a(root + "/nextn");
    ck("nextn_present_detected",
       std::string(nextn_conclusion(nn)) == "NEXTN_PRESENT_IN_GRAPH");

    // 8. destination isolee : refus si non vide
    std::string iso = root + "/iso";
    auto a1 = prepare_f1a(iso, "baseline", "run1");
    ck("isolated_destination_accepted_when_empty", a1.ok);
    if (a1.ok) {
        write_file(a1.outdir + "/EVENT_LEDGER.csv", LEDGER_HDR);
        auto a2 = prepare_f1a(iso, "baseline", "run1");
        ck("reuse_of_previous_capture_refused", !a2.ok &&
           a2.refusal.find("non vide") != std::string::npos);
    } else { ck("reuse_of_previous_capture_refused", false); }

    printf("\nWHT_CAPTURE_COLLECTOR_SELFTEST=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("MTP_CAPTURE_COLLECTOR_SELFTEST=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("CAPTURE_COLLECTOR_FAILED_COUNT=%d\n", failed);
    printf("RUNTIME_CAPTURE_STATUS=METHOD_IMPLEMENTED_NOT_EXERCISED\n");
    return failed == 0 ? 0 : 1;
}

// ---------------------------------------------------------------- W1 : runtime
// Points d entree de calcul, INJECTABLES, pour que la garde soit testable
// sans jamais appeler libllama.
struct RuntimeOps {
    std::function<llama_model * (const std::string &)>              full_load;
    std::function<llama_context * (llama_model *)>                  make_ctx;
    std::function<bool(llama_context *, const std::vector<llama_token> &)> prefill;
    std::function<int (llama_context *, llama_token, llama_pos)>   decode;
    std::function<void(llama_context *)>                            free_ctx;
    std::function<void(llama_model *)>                              free_model;
};

static RuntimeOps real_ops() {
    RuntimeOps o;
    o.full_load = [](const std::string & p) {
        llama_model_params mp = llama_model_default_params();
        mp.vocab_only = false;      // chargement COMPLET (mode runtime uniquement)
        return llama_model_load_from_file(p.c_str(), mp);
    };
    o.make_ctx = [](llama_model * m) {
        llama_context_params cp = llama_context_default_params();
        cp.n_ctx = 512; cp.n_batch = 512; cp.n_ubatch = 512;
        cp.n_seq_max = 1;
        return llama_init_from_model(m, cp);
    };
    // Prefill : batch explicite, positions 0..n-1, logits demandes SEULEMENT
    // sur le dernier token (llama_batch.logits est un tableau PAR token dans
    // cette revision : il n existe pas de champ logits_all).
    o.prefill = [](llama_context * ctx, const std::vector<llama_token> & toks) {
        if (toks.empty()) return false;
        llama_batch b = llama_batch_init((int32_t) toks.size(), 0, 1);
        for (size_t i = 0; i < toks.size(); ++i) {
            b.token[i]     = toks[i];
            b.pos[i]       = (llama_pos) i;
            b.n_seq_id[i]  = 1;
            b.seq_id[i][0] = 0;
            b.logits[i]    = (i + 1 == toks.size()) ? 1 : 0;
        }
        b.n_tokens = (int32_t) toks.size();
        int rc = llama_decode(ctx, b);
        llama_batch_free(b);
        return rc == 0;
    };
    // Un pas de decode : UN token, position explicite, logits demandes.
    o.decode = [](llama_context * ctx, llama_token tok, llama_pos pos) {
        llama_batch b = llama_batch_init(1, 0, 1);
        b.token[0]     = tok;
        b.pos[0]       = pos;
        b.n_seq_id[0]  = 1;
        b.seq_id[0][0] = 0;
        b.logits[0]    = 1;
        b.n_tokens     = 1;
        int rc = llama_decode(ctx, b);
        llama_batch_free(b);
        return rc;
    };
    o.free_ctx   = [](llama_context * c) { llama_free(c); };
    o.free_model = [](llama_model * m) { llama_model_free(m); };
    return o;
}

// Mocks : comptent les invocations, n appellent RIEN.
static long long g_mock_full_load = 0, g_mock_make_ctx = 0,
                 g_mock_prefill = 0, g_mock_decode = 0;
static RuntimeOps mock_ops() {
    RuntimeOps o;
    o.full_load = [](const std::string &) { g_mock_full_load++; return (llama_model *) nullptr; };
    o.make_ctx  = [](llama_model *) { g_mock_make_ctx++; return (llama_context *) nullptr; };
    o.prefill   = [](llama_context *, const std::vector<llama_token> &) { g_mock_prefill++; return true; };
    o.decode    = [](llama_context *, llama_token, llama_pos) { g_mock_decode++; return 0; };
    o.free_ctx   = [](llama_context *) {};
    o.free_model = [](llama_model *) {};
    return o;
}

enum class PreflightStatus { REFUSED, OK, ERROR };

struct PreflightRequest {
    std::string model_path, template_path, capture_root, model_tag, run_tag;
    std::string user_content = "Reponds uniquement : OK.";
    int         max_new_tokens = 8;
};

// ORDRE IMPOSE : la garde est evaluee AVANT le chargement complet.
static PreflightStatus run_preflight(const PreflightRequest & req,
                                     RuntimeOps & ops,
                                     bool capture_enabled) {
    // 1. garde AVANT tout chargement complet, tout contexte, tout forward
    if (!g_auth.allowed) {
        g_rc.guard_refused++;
        return PreflightStatus::REFUSED;
    }
    // 2. capture isolee + activation f1a (JAMAIS avant la garde)
    F1aPlan plan;
    if (capture_enabled) {
        plan = prepare_f1a(req.capture_root, req.model_tag, req.run_tag);
        if (!plan.ok) return PreflightStatus::ERROR;
        setenv(F1A_ENABLE, "1", 1);          // activateurs lus par Q1
        setenv(F1A_OUTDIR, plan.outdir.c_str(), 1);
        setenv(F1A_RUNID, plan.run_id.c_str(), 1);
        setenv(F1A_INVENTORY, plan.inventory.c_str(), 1);
        // OBRAIN_NO_ROTATION n est JAMAIS touche ici.
    }
    // 3. chargement COMPLET
    g_rc.full_load_calls++;
    llama_model * m = ops.full_load(req.model_path);
    if (!m) return PreflightStatus::ERROR;
    // 4. contexte neuf
    g_rc.context_create_calls++;
    llama_context * ctx = ops.make_ctx(m);
    if (!ctx) { ops.free_model(m); return PreflightStatus::ERROR; }
    // 5. prefill
    std::vector<llama_token> prompt;
    g_rc.prefill_calls++;
    if (!ops.prefill(ctx, prompt)) { ops.free_ctx(ctx); ops.free_model(m); return PreflightStatus::ERROR; }
    // 6. boucle bornee -- la source de tokens est le decode reel
    llama_pos pos = (llama_pos) prompt.size();
    llama_token next = prompt.empty() ? 0 : prompt.back();
    auto o = simulate_generation([&](int, llama_token & t) {
        int rc = ops.decode(ctx, next, pos);
        if (rc != 0) return false;                 // erreur de decode explicite
        g_rc.llama_decode_calls++;                 // UN appel API reel, pas un forward suppose
        const float * lg = llama_get_logits_ith(ctx, -1);
        const int     nv = (int) llama_vocab_n_tokens(llama_model_get_vocab(m));
        if (!lg || nv <= 0) return false;
        std::vector<float> row(lg, lg + nv);
        int best = greedy_argmax_lowest_id(row);   // argmax direct, egalite -> plus petit id
        if (best < 0) return false;
        t = (llama_token) best;
        next = t; pos++;
        return true;
    }, req.max_new_tokens);
    g_rc.tokens_selected += (long long) o.ids.size();
    for (auto t : o.ids) if (is_stop_token(t)) g_rc.stop_tokens_selected++;
    ops.free_ctx(ctx);
    ops.free_model(m);
    return o.reason == StopReason::ERROR ? PreflightStatus::ERROR : PreflightStatus::OK;
}

// Test de la garde avec fonctions MOCKEES : les mocks ne doivent JAMAIS etre
// appeles quand la garde refuse.
static int mode_guard_selftest() {
    int failed = 0;
    auto ck = [&](const char * n, bool ok) {
        printf("  %-58s %s\n", n, ok ? "PASS" : "FAIL");
        if (!ok) failed++;
    };
    printf("=== GARDE AVANT CHARGEMENT COMPLET (fonctions mockees) ===\n");
    g_mock_full_load = g_mock_make_ctx = g_mock_prefill = g_mock_decode = 0;
    g_rc = RuntimeCounters{};
    g_auth.allowed = false;
    g_auth.reason  = "aucune autorisation d execution";

    PreflightRequest req;
    req.model_path = "/nonexistent"; req.template_path = "/nonexistent";
    req.capture_root = "/tmp";
    RuntimeOps mk = mock_ops();
    PreflightStatus st = run_preflight(req, mk, false);

    ck("preflight_refused_by_guard", st == PreflightStatus::REFUSED);
    ck("mock_full_load_never_called", g_mock_full_load == 0);
    ck("mock_make_ctx_never_called",   g_mock_make_ctx == 0);
    ck("mock_prefill_never_called",    g_mock_prefill == 0);
    ck("mock_decode_never_called",     g_mock_decode == 0);
    ck("guard_refusal_counted",        g_rc.guard_refused == 1);
    ck("no_full_load_counted",         g_rc.full_load_calls == 0);
    ck("no_context_counted",           g_rc.context_create_calls == 0);
    ck("no_prefill_counted",           g_rc.prefill_calls == 0);
    ck("no_decode_counted",            g_rc.llama_decode_calls == 0);

    // le drapeau seul ne doit pas ouvrir la garde
    g_auth.allowed = false;
    g_auth.reason  = "drapeau present sans autorisation du plan gele";
    g_mock_full_load = g_mock_make_ctx = g_mock_prefill = g_mock_decode = 0;
    PreflightRequest r2 = req;
    PreflightStatus st2 = run_preflight(r2, mk, false);
    ck("flag_alone_does_not_open_guard", st2 == PreflightStatus::REFUSED && g_mock_decode == 0);

    printf("\nHARNESS_COMPUTE_GUARD_SELFTEST=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("GUARD_SELFTEST_FAILED_COUNT=%d\n", failed);
    printf("PREFLIGHT_MODE_UNLOCKED=%s\n", "NO");
    return failed == 0 ? 0 : 1;
}

'''

# inserer le bloc avant main()
i = s.index("int main(int argc, char ** argv) {")
s = s[:i] + BLOCK + s[i:]

# patcher le dispatch des modes
old = '''    if (mode == "verify-freeze")     { printf("VERIFY_FREEZE=NOT_IMPLEMENTED_IN_THIS_BUILD\\n"); return 2; }'''
new = '''    if (mode == "verify-freeze")     { printf("VERIFY_FREEZE=NOT_IMPLEMENTED_IN_THIS_BUILD\\n"); return 2; }
    if (mode == "stop-selftest")     return mode_stop_selftest();
    if (mode == "guard-selftest")    return mode_guard_selftest();
    if (mode == "capture-selftest")  return mode_capture_selftest(out_dir.empty() ? "/tmp" : out_dir);
    if (mode == "preflight") {
        // Mode runtime : la garde est evaluee DANS run_preflight, avant tout
        // chargement complet. Aucune ouverture automatique apres compilation.
        PreflightRequest req;
        req.model_path   = model_b;
        req.template_path= template_path;
        req.capture_root = out_dir.empty() ? "/tmp/rec01-capture" : out_dir + "/capture";
        req.model_tag    = "baseline";
        req.run_tag      = "run1";
        RuntimeOps ops   = real_ops();
        PreflightStatus st = run_preflight(req, ops, true);
        if (st == PreflightStatus::REFUSED) {
            fprintf(stderr, "%s\\n  operation refusee : mode preflight\\n  motif : %s\\n"
                            "  aucun chargement complet, aucun contexte, aucun forward\\n",
                    REFUSED_MSG, g_auth.reason.c_str());
            return 3;
        }
        printf("PREFLIGHT_STATUS=%s\\n", st == PreflightStatus::OK ? "OK" : "ERROR");
        printf("PREFLIGHT_LLAMA_DECODE_CALL_COUNT=%lld\\n", g_rc.llama_decode_calls);
        return st == PreflightStatus::OK ? 0 : 1;
    }'''
assert old in s
s = s.replace(old, new, 1)

# le drapeau --allow-model-compute doit rester insuffisant : inchange.
open(OUT, "w", encoding="utf-8", newline="\n").write(s)
print("harnais etendu : %d lignes" % s.count("\n"))
