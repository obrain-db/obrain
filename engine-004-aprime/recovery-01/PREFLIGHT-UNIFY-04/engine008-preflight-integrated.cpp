// ENGINE-008 -- PREFLIGHT-INTEGRATE-CODE-03
// S2 correlation des soumissions + S3 enregistrement des hypotheses + S4
// autorisation nominale mockee.
//
// TOUT est mocke : aucun chargement de modele, aucun forward, aucun acces aux
// globals de Q1. Les callbacks de calcul sont des mocks ; ils ne sont atteints
// que si l'autorisation est valide.

#include "ggml.h"

#include <algorithm>
#include <cstdio>
#include <cstring>
#include <map>
#include <set>
#include <string>
#include <vector>

static int failed = 0;
static void ck(const char * n, bool ok) {
    printf("  %-64s %s\n", n, ok ? "PASS" : "FAIL");
    if (!ok) failed++;
}

// ===================================================================== S4
// AUTORISATION : le binaire verifie un DOCUMENT EXTERNE dont l'identite lui est
// FOURNIE explicitement. Aucune circularite : le document reference l'identite
// du binaire, pas l'inverse.
struct AuthorizationDoc {
    std::string binary_sha256;     // identite du binaire autorise
    std::string plan_sha256;       // identite du plan gele
    std::string protocol_sha256;   // identite du protocole recevable
    bool        allow_model_compute = false;
    bool        well_formed = false;
};

struct AuthorizationRequest {
    bool        flag_allow_model_compute = false;
    std::string binary_sha256;     // identite REELLE du binaire en cours
    std::string plan_sha256;
    std::string protocol_sha256;
    std::string doc_path;          // document externe a lire
};

enum class AuthVerdict {
    ACCEPTED,
    REFUSED_NO_DOCUMENT,
    REFUSED_MALFORMED_DOCUMENT,
    REFUSED_FLAG_ONLY,
    REFUSED_BINARY_IDENTITY,
    REFUSED_PLAN_MISMATCH,
    REFUSED_PROTOCOL_MISMATCH
};
static const char * auth_name(AuthVerdict v) {
    switch (v) {
        case AuthVerdict::ACCEPTED:                   return "ACCEPTED";
        case AuthVerdict::REFUSED_NO_DOCUMENT:        return "REFUSED_NO_DOCUMENT";
        case AuthVerdict::REFUSED_MALFORMED_DOCUMENT: return "REFUSED_MALFORMED_DOCUMENT";
        case AuthVerdict::REFUSED_FLAG_ONLY:          return "REFUSED_FLAG_ONLY";
        case AuthVerdict::REFUSED_BINARY_IDENTITY:    return "REFUSED_BINARY_IDENTITY";
        case AuthVerdict::REFUSED_PLAN_MISMATCH:      return "REFUSED_PLAN_MISMATCH";
        default:                                      return "REFUSED_PROTOCOL_MISMATCH";
    }
}

// Callbacks MOCKES : ils comptent leurs appels et n'appellent RIEN.
struct MockCallbacks {
    long long load = 0, ctx = 0, prefill = 0, decode = 0;
    std::vector<std::string> order;
};

// Le document est fourni par une source injectable (test = mock, reel = fichier).
struct AuthDocSource {
    virtual bool read(const std::string & path, AuthorizationDoc & out) = 0;
    virtual ~AuthDocSource() {}
};

static AuthVerdict authorize(const AuthorizationRequest & req,
                             AuthDocSource & docs,
                             MockCallbacks & cb,
                             void * = nullptr) {
    if (req.doc_path.empty()) return AuthVerdict::REFUSED_NO_DOCUMENT;
    AuthorizationDoc d;
    if (!docs.read(req.doc_path, d)) return AuthVerdict::REFUSED_NO_DOCUMENT;
    if (!d.well_formed)              return AuthVerdict::REFUSED_MALFORMED_DOCUMENT;
    // Le drapeau seul ne suffit JAMAIS : il faut un document valide en plus.
    if (!d.allow_model_compute)      return AuthVerdict::REFUSED_FLAG_ONLY;
    // Identites : comparaison EXACTE de chaines completes, jamais de prefixe.
    if (d.binary_sha256   != req.binary_sha256)   return AuthVerdict::REFUSED_BINARY_IDENTITY;
    if (d.plan_sha256     != req.plan_sha256)     return AuthVerdict::REFUSED_PLAN_MISMATCH;
    if (d.protocol_sha256 != req.protocol_sha256) return AuthVerdict::REFUSED_PROTOCOL_MISMATCH;
    if (!req.flag_allow_model_compute)            return AuthVerdict::REFUSED_FLAG_ONLY;
    return AuthVerdict::ACCEPTED;
}

// Chemin runtime : les callbacks ne sont atteints QUE si authorize() accepte.
static void runtime_path(AuthVerdict v, MockCallbacks & cb) {
    if (v != AuthVerdict::ACCEPTED) return;   // aucun callback apres refus
    cb.load++;    cb.order.push_back("load");
    cb.ctx++;     cb.order.push_back("ctx");
    cb.prefill++; cb.order.push_back("prefill");
    cb.decode++;  cb.order.push_back("decode");
}

// ===================================================================== S2
// CORRELATION DES SOUMISSIONS
// Champs reellement disponibles au producteur (f1a-capture.cpp:301-310) :
//   submission_id, gf, n_nodes, submission_rc, fingerprint
// Le pointeur et le nombre de noeuds PEUVENT etre reutilises : ils ne suffisent
// pas. Le fingerprint est l'identite ; sa definition exacte au producteur n'a
// PAS ete etablie -> la confrontation est declaree NON IMPLEMENTABLE en l'etat.

struct LedgerRow {
    unsigned long long submission_id = 0;
    std::string gf;
    int n_nodes = 0;
    int submission_rc = 0;
    std::string fingerprint;
};

struct Enumeration {
    std::string gf;              // issu de la capture
    int n_declared = 0;
    std::string fingerprint;     // NON derive par nous : voir limite
    bool valid = false;
};

enum class CorrelationVerdict {
    CORRELATED,
    ZERO_SUBMISSION,
    MULTIPLE_SUBMISSIONS_ONLY_LAST_ENUMERATED,
    AMBIGUOUS_MAPPING,
    FINGERPRINT_CONFRONTATION_NOT_IMPLEMENTABLE,
    SUBMISSION_ERROR,
    MALFORMED_CAPTURE,
    STALE_CAPTURE_REUSED,
    INVALID_WINDOW
};
static const char * corr_name(CorrelationVerdict v) {
    switch (v) {
        case CorrelationVerdict::CORRELATED: return "CORRELATED";
        case CorrelationVerdict::ZERO_SUBMISSION: return "ZERO_SUBMISSION";
        case CorrelationVerdict::MULTIPLE_SUBMISSIONS_ONLY_LAST_ENUMERATED:
            return "MULTIPLE_SUBMISSIONS_ONLY_LAST_ENUMERATED";
        case CorrelationVerdict::AMBIGUOUS_MAPPING: return "AMBIGUOUS_MAPPING";
        case CorrelationVerdict::FINGERPRINT_CONFRONTATION_NOT_IMPLEMENTABLE:
            return "FINGERPRINT_CONFRONTATION_NOT_IMPLEMENTABLE";
        case CorrelationVerdict::SUBMISSION_ERROR: return "SUBMISSION_ERROR";
        case CorrelationVerdict::MALFORMED_CAPTURE: return "MALFORMED_CAPTURE";
        case CorrelationVerdict::STALE_CAPTURE_REUSED: return "STALE_CAPTURE_REUSED";
        default: return "INVALID_WINDOW";
    }
}

// Fenetre d'observation : releve du ledger AVANT et APRES l'appel.
struct CallWindow {
    std::vector<LedgerRow> before;
    std::vector<LedgerRow> after;
    bool malformed = false;
    bool stale_reuse_detected = false;   // une capture anterieure a ete reutilisee
};

// Soumissions NOUVELLES apparues dans la fenetre (par submission_id).
static std::vector<LedgerRow> new_submissions(const CallWindow & w) {
    std::set<unsigned long long> seen;
    for (const auto & r : w.before) seen.insert(r.submission_id);
    std::vector<LedgerRow> out;
    for (const auto & r : w.after) if (!seen.count(r.submission_id)) out.push_back(r);
    return out;
}

// Force de l'identite : la couverture peut etre etablie sur gf+n_nodes (tous
// deux NECESSAIRES mais reutilisables), tandis que le fingerprint -- seule
// identite -- n'est PAS confrontable faute de definition etablie au producteur.
enum class IdentityStrength {
    POINTER_AND_COUNT_ONLY,
    FINGERPRINT_CONFRONTED
};
static const char * idstr_name(IdentityStrength s) {
    return s == IdentityStrength::FINGERPRINT_CONFRONTED ? "FINGERPRINT_CONFRONTED"
                                                         : "POINTER_AND_COUNT_ONLY";
}

struct CallCorrelation {
    CorrelationVerdict per_submission = CorrelationVerdict::ZERO_SUBMISSION;
    CorrelationVerdict per_call       = CorrelationVerdict::ZERO_SUBMISSION;
    IdentityStrength   identity       = IdentityStrength::POINTER_AND_COUNT_ONLY;
    unsigned long long correlated_id  = 0;
};

// `enumerated` = ce que l'enumerateur a pu capturer (au plus UNE soumission).
static CallCorrelation correlate(const CallWindow & w, const Enumeration & e) {
    CallCorrelation c;
    if (w.malformed) { c.per_submission = c.per_call = CorrelationVerdict::MALFORMED_CAPTURE; return c; }
    if (w.stale_reuse_detected) {
        c.per_submission = c.per_call = CorrelationVerdict::STALE_CAPTURE_REUSED; return c;
    }
    std::vector<LedgerRow> fresh = new_submissions(w);
    if (fresh.empty()) {
        // AUCUNE nouvelle soumission : aucune observation attribuable.
        c.per_submission = c.per_call = CorrelationVerdict::ZERO_SUBMISSION; return c;
    }
    if (fresh.size() > 1) {
        // Plusieurs soumissions, mais l'enumerateur n'en couvre qu'une.
        c.per_call = CorrelationVerdict::MULTIPLE_SUBMISSIONS_ONLY_LAST_ENUMERATED;
        c.per_submission = CorrelationVerdict::CORRELATED;   // par soumission : OK
        return c;
    }
    const LedgerRow & r = fresh[0];
    if (r.submission_rc != 0) {
        c.per_call = c.per_submission = CorrelationVerdict::SUBMISSION_ERROR;
        c.correlated_id = r.submission_id; return c;
    }
    if (!e.valid) { c.per_call = c.per_submission = CorrelationVerdict::INVALID_WINDOW; return c; }
    // Le pointeur et le compte de noeuds ne suffisent PAS (reutilisation possible).
    // On les utilise comme CONTRÔLE NECESSAIRE, jamais comme identite.
    if (e.gf != r.gf || e.n_declared != r.n_nodes) {
        c.per_call = c.per_submission = CorrelationVerdict::AMBIGUOUS_MAPPING;
        c.correlated_id = r.submission_id; return c;
    }
    // La COUVERTURE est etablie : une seule soumission nouvelle, enumeration
    // valide, pointeur et compte de noeuds concordants (necessaires, non
    // suffisants). L'IDENTITE, elle, reste a la force du pointeur et du compte :
    // la definition du fingerprint au producteur n'est PAS etablie et
    // l'enumerateur ne le derive pas -> NON CONFRONTABLE. Les deux sont
    // rapportes SEPAREMENT ; confondre les deux transformerait un caveat
    // d'identite en echec de couverture.
    c.per_call = c.per_submission = CorrelationVerdict::CORRELATED;
    c.identity = IdentityStrength::POINTER_AND_COUNT_ONLY;
    c.correlated_id = r.submission_id;
    return c;
}

// ===================================================================== S3
// HYPOTHESES DU CHEMIN STANDARD : on enregistre ce qui est TRANSMIS, pas ce que
// le contexte contient en interne.
struct RuntimeAssumptions {
    std::string ctx_type_transmitted = "LLAMA_CONTEXT_TYPE_DEFAULT";
    bool        embeddings_nextn_transmitted = false;
    bool        context_created_ok = false;
    std::string gtype_status = "GTYPE_DERIVED_FROM_VERIFIED_SOURCE_MAPPING";
};
static RuntimeAssumptions record_assumptions(const std::string & ctx_type,
                                             bool emb_nextn, bool ctx_ok) {
    RuntimeAssumptions a;
    a.ctx_type_transmitted = ctx_type;
    a.embeddings_nextn_transmitted = emb_nextn;
    a.context_created_ok = ctx_ok;
    return a;
}

// ===================================================================== TESTS
struct MockDocs : AuthDocSource {
    std::map<std::string, AuthorizationDoc> docs;
    bool read(const std::string & p, AuthorizationDoc & out) override {
        auto it = docs.find(p);
        if (it == docs.end()) return false;
        out = it->second; return true;
    }
};

static LedgerRow row(unsigned long long id, const char * gf, int n, int rc, const char * fp) {
    LedgerRow r; r.submission_id = id; r.gf = gf; r.n_nodes = n; r.submission_rc = rc; r.fingerprint = fp; return r;
}

int main() {
    printf("ENGINE-008 -- PREFLIGHT-INTEGRATE-CODE-03 (tout mocke, aucun modele)\n\n");
    const std::string BIN = "aa7e4bf45f69854fe92e871d4bcf352b99972a2dc1572fd6af62640c8b988ce9";
    const std::string PLAN = "b1821f335d93464106d8cc7d13d5f167edd1e95d7e1af9cc15a8f747b56ade06";
    const std::string PROTO = "c42f485d41214fd9a87b9398c0df5a82965f68eb35e2ee69e17c4db76252e1a3";

    // ---------------- S4 : autorisation nominale et refus
    printf("=== S4 AUTORISATION (callbacks mockes) ===\n");
    MockDocs docs;
    AuthorizationDoc good; good.binary_sha256 = BIN; good.plan_sha256 = PLAN;
    good.protocol_sha256 = PROTO; good.allow_model_compute = true; good.well_formed = true;
    docs.docs["auth.json"] = good;
    AuthorizationDoc malformed = good; malformed.well_formed = false;
    docs.docs["malformed.json"] = malformed;

    AuthorizationRequest req; req.flag_allow_model_compute = true;
    req.binary_sha256 = BIN; req.plan_sha256 = PLAN; req.protocol_sha256 = PROTO;
    req.doc_path = "auth.json";

    { MockCallbacks cb; auto v = authorize(req, docs, cb);
      ck("nominal_authorization_accepted", v == AuthVerdict::ACCEPTED);
      runtime_path(v, cb);
      ck("callbacks_reached_in_order",
         cb.load == 1 && cb.ctx == 1 && cb.prefill == 1 && cb.decode == 1 &&
         cb.order.size() == 4 && cb.order[0] == "load" && cb.order[3] == "decode"); }

    auto rejection = [&](const char * name, AuthorizationRequest r, AuthVerdict expect) {
        MockCallbacks cb;
        auto v = authorize(r, docs, cb);
        runtime_path(v, cb);
        ck(name, v == expect && cb.load == 0 && cb.ctx == 0 && cb.prefill == 0 && cb.decode == 0);
    };

    { AuthorizationRequest r = req; r.flag_allow_model_compute = false;
      rejection("flag_absent_refused", r, AuthVerdict::REFUSED_FLAG_ONLY); }
    { AuthorizationRequest r = req; r.doc_path = "";
      rejection("absent_document_refused", r, AuthVerdict::REFUSED_NO_DOCUMENT); }
    { AuthorizationRequest r = req; r.doc_path = "malformed.json";
      rejection("malformed_document_refused", r, AuthVerdict::REFUSED_MALFORMED_DOCUMENT); }
    { AuthorizationRequest r = req; r.binary_sha256 = "deadbeef";
      rejection("wrong_binary_identity_refused", r, AuthVerdict::REFUSED_BINARY_IDENTITY); }
    { AuthorizationRequest r = req; r.plan_sha256 = "deadbeef";
      rejection("plan_mismatch_refused", r, AuthVerdict::REFUSED_PLAN_MISMATCH); }
    { AuthorizationRequest r = req; r.protocol_sha256 = "deadbeef";
      rejection("protocol_mismatch_refused", r, AuthVerdict::REFUSED_PROTOCOL_MISMATCH); }
    { // le drapeau seul, sans document : refuse
      AuthorizationRequest r = req; r.doc_path = ""; r.flag_allow_model_compute = true;
      rejection("flag_alone_without_document_refused", r, AuthVerdict::REFUSED_NO_DOCUMENT); }

    // ---------------- S3 : hypotheses du chemin standard
    printf("=== S3 HYPOTHESES DU CHEMIN STANDARD ===\n");
    { auto a = record_assumptions("LLAMA_CONTEXT_TYPE_DEFAULT", false, true);
      ck("assumptions_record_transmitted_values",
         a.ctx_type_transmitted == "LLAMA_CONTEXT_TYPE_DEFAULT" &&
         a.embeddings_nextn_transmitted == false && a.context_created_ok);
      ck("gtype_status_is_derived_not_observed",
         a.gtype_status == "GTYPE_DERIVED_FROM_VERIFIED_SOURCE_MAPPING"); }

    // ---------------- S2 : correlation des soumissions
    printf("=== S2 CORRELATION DES SOUMISSIONS ===\n");
    Enumeration en; en.gf = "0xAAA"; en.n_declared = 100; en.valid = true;

    { CallWindow w; w.before = { row(1,"0xAAA",100,0,"fp1") };
      w.after  = { row(1,"0xAAA",100,0,"fp1") };
      auto c = correlate(w, en);
      ck("zero_new_submission_no_attribution", c.per_call == CorrelationVerdict::ZERO_SUBMISSION); }

    { CallWindow w; w.before = {};
      w.after  = { row(1,"0xAAA",100,0,"fp1") };
      auto c = correlate(w, en);
      ck("single_submission_correlated_per_submission", c.per_submission == CorrelationVerdict::CORRELATED);
      ck("coverage_established_for_single_submission",
         c.per_call == CorrelationVerdict::CORRELATED);
      ck("identity_strength_pointer_and_count_only",
         c.identity == IdentityStrength::POINTER_AND_COUNT_ONLY);
      ck("fingerprint_confrontation_not_claimed",
         std::string(idstr_name(c.identity)) == "POINTER_AND_COUNT_ONLY"); }

    { CallWindow w; w.before = {};
      w.after  = { row(1,"0xAAA",100,0,"fp1"), row(2,"0xBBB",200,0,"fp2") };
      auto c = correlate(w, en);
      ck("multiple_submissions_per_call_incomplete",
         c.per_call == CorrelationVerdict::MULTIPLE_SUBMISSIONS_ONLY_LAST_ENUMERATED);
      ck("per_submission_still_ok_when_multiple",
         c.per_submission == CorrelationVerdict::CORRELATED); }

    { CallWindow w; w.before = {};
      w.after  = { row(1,"0xOTHER",100,0,"fp1") };
      auto c = correlate(w, en);
      ck("pointer_mismatch_ambiguous", c.per_call == CorrelationVerdict::AMBIGUOUS_MAPPING); }

    { CallWindow w; w.before = {};
      w.after  = { row(1,"0xAAA",999,0,"fp1") };
      auto c = correlate(w, en);
      ck("node_count_mismatch_ambiguous", c.per_call == CorrelationVerdict::AMBIGUOUS_MAPPING); }

    { CallWindow w; w.before = {};
      w.after  = { row(1,"0xAAA",100,7,"fp1") };
      auto c = correlate(w, en);
      ck("failed_submission_explicit_status", c.per_call == CorrelationVerdict::SUBMISSION_ERROR); }

    { CallWindow w; w.malformed = true;
      auto c = correlate(w, en);
      ck("malformed_capture_explicit_status", c.per_call == CorrelationVerdict::MALFORMED_CAPTURE); }

    { CallWindow w; w.before = {}; w.after = { row(1,"0xAAA",100,0,"fp1") };
      w.stale_reuse_detected = true;
      auto c = correlate(w, en);
      ck("stale_capture_reuse_refused", c.per_call == CorrelationVerdict::STALE_CAPTURE_REUSED); }

    { CallWindow w; w.before = {}; w.after = { row(1,"0xAAA",100,0,"fp1") };
      Enumeration bad; bad.gf = "0xAAA"; bad.n_declared = 100; bad.valid = false;
      auto c = correlate(w, bad);
      ck("invalid_enumeration_no_conclusion", c.per_call == CorrelationVerdict::INVALID_WINDOW); }

    // Aucun verdict de correlation ne conclut une ABSENCE NextN.
    { const char * names[] = {"ZERO_SUBMISSION","MULTIPLE_SUBMISSIONS_ONLY_LAST_ENUMERATED",
        "AMBIGUOUS_MAPPING","FINGERPRINT_CONFRONTATION_NOT_IMPLEMENTABLE","SUBMISSION_ERROR",
        "MALFORMED_CAPTURE","STALE_CAPTURE_REUSED","INVALID_WINDOW"};
      bool none_claims_absence = true;
      for (const char * n : names) if (strstr(n, "ABSENT") || strstr(n, "NO_NEXTN")) none_claims_absence = false;
      ck("no_correlation_verdict_claims_nextn_absence", none_claims_absence); }

    printf("\nNOMINAL_AUTHORIZATION_MOCK_SELFTEST=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("REJECTION_GUARD_SELFTEST=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("INTEGRATION_SELFTEST=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("FAILED_COUNT=%d\n", failed);
    printf("MODEL_FORWARD_CALL_COUNT=0\nFULL_MODEL_LOAD_COUNT=0\n");
    return failed == 0 ? 0 : 1;
}
