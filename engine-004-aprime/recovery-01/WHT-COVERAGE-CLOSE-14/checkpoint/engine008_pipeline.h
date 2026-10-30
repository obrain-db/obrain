// ENGINE-008 -- PREFLIGHT-CONNECT-05 -- interfaces du pipeline commun.
// Wrappers appellables pour : registre d'identite, enumeration, correlation,
// autorisation, contrat d'arret, serialisation. Les implementations internes
// des composants restent `static` derriere ces interfaces.
#pragma once
#include <cmath>
#include <cstdint>
#include <functional>
#include <string>
#include <vector>
#include "engine008_wht_parser.h"

typedef int32_t llama_token_i;

// ---------------------------------------------------------------- arret
enum class StopReason { EOS = 0, MAX_NEW_TOKENS = 1, ERROR = 2, NONE = 3 };
inline const char * stop_reason_name(StopReason r) {
    switch (r) {
        case StopReason::EOS:            return "EOS";
        case StopReason::MAX_NEW_TOKENS: return "MAX_NEW_TOKENS";
        case StopReason::ERROR:          return "ERROR";
        default:                         return "NONE";
    }
}
// GENERATE_STOP_TOKEN_IDS = [248046]  (repris du protocole gele, NON elargi)
static const std::vector<llama_token_i> STOP_TOKEN_IDS = { 248046 };
inline bool is_stop_token(llama_token_i t) {
    for (auto s : STOP_TOKEN_IDS) if (s == t) return true;
    return false;
}
struct GenOutcome { std::vector<llama_token_i> ids; StopReason reason = StopReason::NONE; };
typedef std::function<bool(int, llama_token_i &)> TokenSource;
GenOutcome simulate_generation(const TokenSource & step, int max_new_tokens);
int greedy_argmax_lowest_id(const std::vector<float> & logits);

// ---------------------------------------------------------------- autorisation
enum class AuthVerdict {
    ACCEPTED, REFUSED_NO_DOCUMENT, REFUSED_MALFORMED_DOCUMENT, REFUSED_FLAG_ONLY,
    REFUSED_BINARY_IDENTITY, REFUSED_PLAN_MISMATCH, REFUSED_PROTOCOL_MISMATCH
};
inline const char * auth_name(AuthVerdict v) {
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

// ---------------------------------------------------------------- registre
enum class EntryState {
    ROLE_RESOLVED, ROLE_ABSENT, KEY_AMBIGUOUS, NULL_IDENTITY,
    MULTI_KEY_OBJECT, EXCLUSIVITY_UNESTABLISHED
};
struct RegistryEntry {
    std::string  key;
    void *       tensor = nullptr;
    std::string  role;
    bool         usable_as_positive_witness = false;
    EntryState   state = EntryState::ROLE_ABSENT;
};
struct IdentityRegistry {
    std::vector<RegistryEntry> entries;
    std::vector<std::string> roles_of(const void * t) const {
        std::vector<std::string> o;
        for (const auto & e : entries) if (e.tensor == t && !e.role.empty()) o.push_back(e.role);
        return o;
    }
    bool usable_witness(const void * t) const {
        for (const auto & e : entries) if (e.tensor == t && e.usable_as_positive_witness) return true;
        return false;
    }
};

// ---------------------------------------------------------------- enumeration
struct Enumeration {
    std::string gf;
    int n_declared = 0;
    int n_exported = 0;
    bool malformed = false;
    bool valid = false;
    // Y2 : fingerprint RECALCULE depuis l objet graphe observe, pendant qu il est
    // encore valide. Vide si indisponible.
    std::string fingerprint_recomputed;
    bool        fingerprint_available = false;
};

// ---------------------------------------------------------------- correlation
struct LedgerRow {
    unsigned long long submission_id = 0;
    std::string gf;
    int n_nodes = 0;
    int submission_rc = 0;
    std::string fingerprint;
};
struct CallWindow {
    std::vector<LedgerRow> before, after;          // surface EVENT_LEDGER
    // Surface SUBMITTED_GRAPH_SNAPSHOTS : DISTINCTE du ledger (champs et roles
    // differents). LedgerRow sert ici de porteur commun (submission_id, gf,
    // n_nodes, submission_rc, fingerprint) sans confondre les deux surfaces.
    std::vector<LedgerRow> snap_before, snap_after;
    bool malformed = false;
    bool stale_reuse_detected = false;
};
enum class CorrelationVerdict {
    CORRELATED, ZERO_SUBMISSION, MULTIPLE_SUBMISSIONS_ONLY_LAST_ENUMERATED,
    AMBIGUOUS_MAPPING, FINGERPRINT_CONFRONTATION_NOT_IMPLEMENTABLE,
    SUBMISSION_ERROR, MALFORMED_CAPTURE, STALE_CAPTURE_REUSED, INVALID_WINDOW
};
inline const char * corr_name(CorrelationVerdict v) {
    switch (v) {
        case CorrelationVerdict::CORRELATED: return "CORRELATED";
        case CorrelationVerdict::ZERO_SUBMISSION: return "ZERO_SUBMISSION";
        case CorrelationVerdict::MULTIPLE_SUBMISSIONS_ONLY_LAST_ENUMERATED: return "MULTIPLE_SUBMISSIONS_ONLY_LAST_ENUMERATED";
        case CorrelationVerdict::AMBIGUOUS_MAPPING: return "AMBIGUOUS_MAPPING";
        case CorrelationVerdict::FINGERPRINT_CONFRONTATION_NOT_IMPLEMENTABLE: return "FINGERPRINT_CONFRONTATION_NOT_IMPLEMENTABLE";
        case CorrelationVerdict::SUBMISSION_ERROR: return "SUBMISSION_ERROR";
        case CorrelationVerdict::MALFORMED_CAPTURE: return "MALFORMED_CAPTURE";
        case CorrelationVerdict::STALE_CAPTURE_REUSED: return "STALE_CAPTURE_REUSED";
        default: return "INVALID_WINDOW";
    }
}
enum class IdentityStrength { POINTER_AND_COUNT_ONLY, FINGERPRINT_CONFRONTED };
inline const char * idstr_name(IdentityStrength s) {
    return s == IdentityStrength::FINGERPRINT_CONFRONTED ? "FINGERPRINT_CONFRONTED" : "POINTER_AND_COUNT_ONLY";
}
// Y2 : confrontation du fingerprint. Une concordance est un CONTROLE DE
// COHERENCE DANS LE PROCESSUS -- jamais une preuve d identite complete.
enum class FingerprintVerdict {
    NOT_ATTEMPTED,          // aucune observation
    MATCH,                  // coherence etablie, dans le processus
    MISMATCH,               // discordance : interdit toute conclusion negative
    ABSENT_OR_INVALID,      // valeur absente ou illisible : interdit toute conclusion
    AMBIGUOUS_JOIN,         // plusieurs snapshots candidats, ou aucun attribuable
    UNAVAILABLE             // le fingerprint n a pas pu etre recalcule
};
inline const char * fpverdict_name(FingerprintVerdict v) {
    switch (v) {
        case FingerprintVerdict::MATCH:             return "MATCH";
        case FingerprintVerdict::MISMATCH:          return "MISMATCH";
        case FingerprintVerdict::ABSENT_OR_INVALID: return "ABSENT_OR_INVALID";
        case FingerprintVerdict::AMBIGUOUS_JOIN:    return "AMBIGUOUS_JOIN";
        case FingerprintVerdict::UNAVAILABLE:       return "UNAVAILABLE";
        default:                                    return "NOT_ATTEMPTED";
    }
}
struct CallCorrelation {
    CorrelationVerdict per_submission = CorrelationVerdict::ZERO_SUBMISSION;
    CorrelationVerdict per_call       = CorrelationVerdict::ZERO_SUBMISSION;
    IdentityStrength   identity       = IdentityStrength::POINTER_AND_COUNT_ONLY;
    unsigned long long correlated_id  = 0;
    // --- Y2, exporte vers le rapport serialise
    FingerprintVerdict fp_verdict     = FingerprintVerdict::NOT_ATTEMPTED;
    std::string        fp_read;             // fingerprint lu dans le snapshot
    std::string        fp_recomputed;       // fingerprint recalcule
    unsigned long long fp_submission_id = 0;
    std::string        fp_gf;
    int                fp_n_nodes = -1;
    int                fp_candidates = 0;   // nombre de snapshots candidats
};

// ---------------------------------------------------------------- hypotheses
struct RuntimeAssumptions {
    std::string ctx_type_transmitted = "LLAMA_CONTEXT_TYPE_DEFAULT";
    bool        embeddings_nextn_transmitted = false;
    bool        context_created_ok = false;
    std::string gtype_status = "GTYPE_DERIVED_FROM_VERIFIED_SOURCE_MAPPING";
};

// ---------------------------------------------------------------- requete/rapport
struct PipelineRequest {
    std::string doc_path;
    std::string binary_sha256, plan_sha256, protocol_sha256;
    bool        flag_allow_model_compute = false;
    std::string model_path, template_path;
    std::string capture_root, model_tag, run_tag;
    std::vector<llama_token_i> prompt_tokens;
    int         max_new_tokens = 8;
};

struct PipelineReport {
    AuthVerdict        auth = AuthVerdict::REFUSED_NO_DOCUMENT;
    std::string        capture_dir, capture_refusal;
    bool               load_ok = false, ctx_ok = false;
    IdentityRegistry   registry;
    int                registry_entries = 0, registry_usable_witnesses = 0;
    RuntimeAssumptions assumptions;
    int                prefill_windows = 0, decode_windows = 0;
    Enumeration        enumeration;
    CallCorrelation    correlation;
    std::vector<llama_token_i> generated;
    StopReason         stop_reason = StopReason::NONE;
    int                llama_decode_calls = 0;
    std::vector<std::string> limits;
    std::string        serialized;
    // --- WHT : resultat agreege, sur TOUTES les fenetres
    bool               wht_collected = false;
    WhtVerdict         wht_verdict = WhtVerdict::SURFACE_MISSING_OR_MALFORMED;
    int                wht_windows_observed = 0;
    int                wht_worst_progress = -1;     // le PIRE maillon atteint
    int                wht_targets_expected = 0;
    int                wht_rotation_true = 0, wht_rotation_false = 0;
    int                wht_in_submitted_graph = 0;
    int                wht_role_ok = 0, wht_effect_ok = 0;
    std::string        wht_scope;
    std::vector<std::string> wht_blockers;
};

// ---------------------------------------------------------------- dependances INJECTABLES
// La MEME orchestration sert au chemin reel futur et aux tests mockes.
struct PipelineDeps {
    std::function<AuthVerdict(const PipelineRequest &)>                                  authorize;
    std::function<bool(const std::string &, const std::string &, const std::string &,
                       std::string &, std::string &)>                                    prepare_capture;
    std::function<void *(const std::string &)>                                           load_full;
    std::function<IdentityRegistry(void *)>                                              acquire_registry;
    std::function<RuntimeAssumptions()>                                                  record_assumptions;
    std::function<void *(void *)>                                                        make_ctx;
    std::function<bool(void *, const std::vector<llama_token_i> &)>                      prefill;
    std::function<CallWindow()>                                                          ledger_window;   // releve AVANT
    std::function<std::vector<LedgerRow>()>                                              ledger_snapshot; // releve APRES (ledger)
    std::function<std::vector<LedgerRow>()>                                              snapshot_after;  // releve APRES (snapshots)
    // Collecteur WHT : lit les surfaces reelles et rend une evaluation de portee.
    std::function<WhtAssessment()>                                                       wht_collect;
    std::function<Enumeration()>                                                         enumerate;
    std::function<CallCorrelation(const CallWindow &, const Enumeration &)>              correlate;
    std::function<const std::vector<float> *(void *)>                                    logits;
    std::function<bool(void *, llama_token_i)>                                           decode_step;
    std::function<void(void *, void *)>                                                  release;
    std::vector<std::string> trace;   // trace d'appels, pour les assertions d'ordre
};

PipelineReport run_pipeline(const PipelineRequest & req, PipelineDeps & deps);
std::string serialize_report(const PipelineReport & r);

inline std::string json_esc(const std::string & s) {
    std::string o;
    for (char c : s) {
        if (c == '"' || c == '\\') { o += '\\'; o += c; }
        else if (c == '\n') o += "\\n";
        else o += c;
    }
    return o;
}

// ---------------------------------------------------------------- logique PARTAGEE
// Ces deux fonctions sont LA logique des composants. Le chemin reel ET les
// mocks les appellent : il n'existe pas de seconde implementation pour les tests.
AuthVerdict authorize_doc(bool flag, const std::string & bin_sha, const std::string & plan_sha,
                          const std::string & proto_sha, bool have_doc, bool well_formed,
                          bool doc_allow, const std::string & doc_bin, const std::string & doc_plan,
                          const std::string & doc_proto);
CallCorrelation correlate_call(const CallWindow & w, const Enumeration & e);
