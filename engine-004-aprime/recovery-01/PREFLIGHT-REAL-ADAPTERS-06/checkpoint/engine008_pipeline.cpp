// ENGINE-008 -- PREFLIGHT-CONNECT-05
// ORCHESTRATION COMMUNE du chemin preflight, dependances injectables.
//
// Ce fichier ne contient AUCUN calcul modele : toute dependance de calcul ou
// d'observation est injectee. La MEME fonction run_pipeline() sert au chemin
// reel futur, au test nominal mocke et aux tests de refus. Il n'existe PAS de
// second pipeline simplifie pour faire passer les tests.
//
// Aucun global de Q1 n'est ecrit. Aucune bibliotheque Q1 n'est requise pour
// compiler ce fichier (types entiers seulement).

#include "engine008_pipeline.h"

#include <algorithm>
#include <cstdio>
#include <cstring>
#include <map>
#include <set>
#include <string>
#include <vector>

// =====================================================================
// SERIALISATION (etape 12) -- fonction PURE, testable seule
// =====================================================================
std::string serialize_report(const PipelineReport & r) {
    std::string o;
    char buf[1024];
    o += "{\"schema\":\"ENGINE008_PIPELINE_REPORT_v1\"";
    snprintf(buf, sizeof buf, ",\"auth\":\"%s\"", auth_name(r.auth)); o += buf;
    o += ",\"capture_dir\":\"" + json_esc(r.capture_dir) + "\"";
    snprintf(buf, sizeof buf, ",\"load_ok\":%d,\"ctx_ok\":%d",
             r.load_ok ? 1 : 0, r.ctx_ok ? 1 : 0); o += buf;
    snprintf(buf, sizeof buf, ",\"registry_entries\":%d,\"registry_usable_witnesses\":%d",
             r.registry_entries, r.registry_usable_witnesses); o += buf;
    o += ",\"ctx_type_transmitted\":\"" + json_esc(r.assumptions.ctx_type_transmitted) + "\"";
    snprintf(buf, sizeof buf, ",\"embeddings_nextn_transmitted\":%d",
             r.assumptions.embeddings_nextn_transmitted ? 1 : 0); o += buf;
    o += ",\"gtype_status\":\"" + json_esc(r.assumptions.gtype_status) + "\"";
    snprintf(buf, sizeof buf, ",\"prefill_windows\":%d,\"decode_windows\":%d",
             r.prefill_windows, r.decode_windows); o += buf;
    snprintf(buf, sizeof buf, ",\"enumeration_declared\":%d,\"enumeration_exported\":%d",
             r.enumeration.n_declared, r.enumeration.n_exported); o += buf;
    snprintf(buf, sizeof buf, ",\"per_submission\":\"%s\",\"per_call\":\"%s\",\"identity\":\"%s\"",
             corr_name(r.correlation.per_submission), corr_name(r.correlation.per_call),
             idstr_name(r.correlation.identity)); o += buf;
    snprintf(buf, sizeof buf, ",\"generated_token_count\":%d,\"llama_decode_calls\":%d",
             (int) r.generated.size(), r.llama_decode_calls); o += buf;
    o += ",\"stop_reason\":\"" + std::string(stop_reason_name(r.stop_reason)) + "\"";
    o += ",\"generated_token_ids\":[";
    for (size_t i = 0; i < r.generated.size(); ++i) {
        if (i) o += ",";
        o += std::to_string(r.generated[i]);
    }
    o += "]";
    o += ",\"limits\":[";
    for (size_t i = 0; i < r.limits.size(); ++i) {
        if (i) o += ",";
        o += "\"" + json_esc(r.limits[i]) + "\"";
    }
    o += "]";
    // Invariant : la serialisation ne porte JAMAIS de conclusion NextN.
    o += ",\"nextn_conclusion\":\"NONE_IN_THIS_REPORT\"";
    o += "}\n";
    return o;
}

// =====================================================================
// CONTRAT D'ARRET (etape 10) -- boucle PURE, source de tokens INJECTEE
// =====================================================================
GenOutcome simulate_generation(const TokenSource & step, int max_new_tokens) {
    GenOutcome o;
    for (int i = 0; i < max_new_tokens; ++i) {
        int32_t t = 0;
        if (!step(i, t)) { o.reason = StopReason::ERROR; return o; }  // erreur rapportee
        o.ids.push_back(t);
        if (is_stop_token(t)) { o.reason = StopReason::EOS; return o; }  // AUCUN decode apres
    }
    o.reason = StopReason::MAX_NEW_TOKENS;
    return o;
}

// argmax direct, egalite exacte -> plus PETIT id
int greedy_argmax_lowest_id(const std::vector<float> & logits) {
    if (logits.empty()) return -1;
    for (float v : logits) if (std::isnan(v) || std::isinf(v)) return -1;  // ligne invalide refusee
    int best = 0;
    for (int i = 1; i < (int) logits.size(); ++i) if (logits[i] > logits[best]) best = i;
    return best;
}

// =====================================================================
// ORCHESTRATION COMMUNE
// =====================================================================
static void trace(PipelineDeps & d, const char * step) { d.trace.push_back(step); }

// Gravite croissante. La couverture d'un APPEL est le PIRE verdict de ses
// fenetres : une fenetre incomplete ne peut pas etre rattrapee par une suivante.
static int corr_severity(CorrelationVerdict v) {
    switch (v) {
        case CorrelationVerdict::CORRELATED: return 0;
        case CorrelationVerdict::ZERO_SUBMISSION: return 1;
        case CorrelationVerdict::MULTIPLE_SUBMISSIONS_ONLY_LAST_ENUMERATED: return 2;
        case CorrelationVerdict::FINGERPRINT_CONFRONTATION_NOT_IMPLEMENTABLE: return 3;
        case CorrelationVerdict::INVALID_WINDOW: return 4;
        case CorrelationVerdict::AMBIGUOUS_MAPPING: return 5;
        case CorrelationVerdict::SUBMISSION_ERROR: return 6;
        case CorrelationVerdict::STALE_CAPTURE_REUSED: return 7;
        default: return 8;   // MALFORMED_CAPTURE
    }
}
static void merge_correlation(CallCorrelation & acc, const CallCorrelation & c) {
    if (corr_severity(c.per_call) > corr_severity(acc.per_call)) acc.per_call = c.per_call;
    if (corr_severity(c.per_submission) > corr_severity(acc.per_submission)) acc.per_submission = c.per_submission;
    if (c.identity == IdentityStrength::POINTER_AND_COUNT_ONLY) acc.identity = IdentityStrength::POINTER_AND_COUNT_ONLY;
    if (c.correlated_id != 0) acc.correlated_id = c.correlated_id;
}

PipelineReport run_pipeline(const PipelineRequest & req, PipelineDeps & d) {
    PipelineReport r;
    bool loaded = false, ctx_made = false;
    void * model = nullptr;
    void * ctx   = nullptr;

    // --- 1. AUTORISATION, AVANT TOUTE OPERATION DE CALCUL
    trace(d, "authorize");
    r.auth = d.authorize(req);
    if (r.auth != AuthVerdict::ACCEPTED) {
        r.limits.push_back("REFUSED_BEFORE_ANY_COMPUTE");
        // Aucune etape de calcul n'est atteinte : aucun nettoyage necessaire.
        trace(d, "serialize"); r.serialized = serialize_report(r);
        return r;
    }

    // --- 2. DESTINATION DE CAPTURE ISOLEE
    trace(d, "prepare_capture");
    if (!d.prepare_capture(req.capture_root, req.model_tag, req.run_tag,
                           r.capture_dir, r.capture_refusal)) {
        r.limits.push_back("CAPTURE_DESTINATION_REFUSED");
        trace(d, "serialize"); r.serialized = serialize_report(r);
        return r;
    }

    // --- 3. CHARGEMENT COMPLET
    trace(d, "load_full");
    model = d.load_full(req.model_path);
    if (model == nullptr) {
        r.limits.push_back("LOAD_FAILED");
        trace(d, "serialize"); r.serialized = serialize_report(r);
        return r;                       // rien a liberer
    }
    loaded = true;
    r.load_ok = true;

    // --- 4. REGISTRE NATIF
    trace(d, "acquire_registry");
    r.registry = d.acquire_registry(model);
    r.registry_entries = (int) r.registry.entries.size();
    for (const auto & e : r.registry.entries) if (e.usable_as_positive_witness) r.registry_usable_witnesses++;

    // --- 5. PARAMETRES TRANSMIS
    trace(d, "record_params");

    // --- 6. CONTEXTE
    trace(d, "make_ctx");
    r.assumptions = d.record_assumptions ? d.record_assumptions() : RuntimeAssumptions{};
    ctx = d.make_ctx(model);
    r.ctx_ok = (ctx != nullptr);
    r.assumptions.context_created_ok = r.ctx_ok;
    if (!r.ctx_ok) {
        r.limits.push_back("CONTEXT_CREATION_FAILED");
        trace(d, "release"); d.release(ctx, model);
        trace(d, "serialize"); r.serialized = serialize_report(r);
        return r;
    }
    ctx_made = true;

    // --- 7. PREFILL DANS UNE FENETRE DE CAPTURE
    trace(d, "window_before_prefill");
    CallWindow pre = d.ledger_window();
    trace(d, "prefill");
    if (!d.prefill(ctx, req.prompt_tokens)) {
        r.limits.push_back("PREFILL_FAILED");
        trace(d, "release"); d.release(ctx, model);
        trace(d, "serialize"); r.serialized = serialize_report(r);
        return r;
    }
    r.llama_decode_calls++;              // le prefill EST un appel a llama_decode
    trace(d, "window_after_prefill");
    pre.after = d.ledger_snapshot();
    r.prefill_windows++;

    // --- 8. COLLECTE / ENUMERATION / CORRELATION, AVANT PEREMPTION
    // f1a_last_gf ne conserve QUE la derniere soumission : on enumere ICI,
    // pas a la fin de la generation.
    trace(d, "enumerate");
    r.enumeration = d.enumerate();
    trace(d, "correlate");
    r.correlation = d.correlate(pre, r.enumeration);   // premier verdict : etat initial

    // --- 9..11. LOGITS, ARGMAX, ARRET, BOUCLE BORNEE
    // Chaque pas de decode a SA PROPRE fenetre d'observation.
    int32_t pending = -1;
    std::vector<llama_token_i> forced;   // vide : la source est le mock de logits
    int produced = 0;
    GenOutcome o;
    o.reason = StopReason::NONE;
    while (produced < req.max_new_tokens) {
        trace(d, "window_before_decode");
        CallWindow w = d.ledger_window();
        trace(d, "read_logits");
        const std::vector<float> * lg = d.logits(ctx);
        if (lg == nullptr) { o.reason = StopReason::ERROR; break; }
        trace(d, "argmax");
        int sel = greedy_argmax_lowest_id(*lg);
        if (sel < 0) { o.reason = StopReason::ERROR; r.limits.push_back("INVALID_LOGIT_ROW"); break; }

        // --- 10. CONTRAT D'ARRET APPLIQUE IMMEDIATEMENT
        o.ids.push_back((int32_t) sel);
        produced++;
        r.decode_windows++;
        if (is_stop_token((int32_t) sel)) { o.reason = StopReason::EOS; break; }
        if (produced >= req.max_new_tokens) { o.reason = StopReason::MAX_NEW_TOKENS; break; }

        trace(d, "decode_step");
        if (!d.decode_step(ctx, (int32_t) sel)) { o.reason = StopReason::ERROR; break; }
        r.llama_decode_calls++;
        trace(d, "window_after_decode");
        w.after = d.ledger_snapshot();
        // enumeration PAR PAS : le graphe observe ne doit pas devenir perime
        r.enumeration = d.enumerate();
        merge_correlation(r.correlation, d.correlate(w, r.enumeration));
    }
    r.generated = o.ids;
    r.stop_reason = o.reason;
    if (o.reason == StopReason::ERROR) r.limits.push_back("DECODE_OR_LOGIT_ERROR");

    // couverture conservee visible
    if (r.correlation.per_call == CorrelationVerdict::MULTIPLE_SUBMISSIONS_ONLY_LAST_ENUMERATED)
        r.limits.push_back("PER_CALL_COVERAGE_INCOMPLETE");
    if (r.correlation.identity == IdentityStrength::POINTER_AND_COUNT_ONLY)
        r.limits.push_back("IDENTITY_POINTER_AND_COUNT_ONLY");
    r.limits.push_back("FINGERPRINT_DEFINITION_UNKNOWN");
    r.limits.push_back("NO_NEXTN_CONCLUSION_FROM_THIS_REPORT");

    // --- 12. SERIALISATION
    trace(d, "serialize");
    r.serialized = serialize_report(r);

    // --- 13. LIBERATION (y compris sur erreur)
    if (ctx_made || loaded) { trace(d, "release"); d.release(ctx, model); }
    return r;
}

// =====================================================================
// LOGIQUE PARTAGEE : autorisation
// =====================================================================
AuthVerdict authorize_doc(bool flag, const std::string & bin_sha, const std::string & plan_sha,
                          const std::string & proto_sha, bool have_doc, bool well_formed,
                          bool doc_allow, const std::string & doc_bin, const std::string & doc_plan,
                          const std::string & doc_proto) {
    if (!have_doc)                   return AuthVerdict::REFUSED_NO_DOCUMENT;
    if (!well_formed)                return AuthVerdict::REFUSED_MALFORMED_DOCUMENT;
    if (!doc_allow)                  return AuthVerdict::REFUSED_FLAG_ONLY;
    // identites : egalite EXACTE de chaines completes, jamais de prefixe
    if (doc_bin   != bin_sha)        return AuthVerdict::REFUSED_BINARY_IDENTITY;
    if (doc_plan  != plan_sha)       return AuthVerdict::REFUSED_PLAN_MISMATCH;
    if (doc_proto != proto_sha)      return AuthVerdict::REFUSED_PROTOCOL_MISMATCH;
    if (!flag)                       return AuthVerdict::REFUSED_FLAG_ONLY;
    return AuthVerdict::ACCEPTED;
}

// =====================================================================
// LOGIQUE PARTAGEE : correlation
// =====================================================================
CallCorrelation correlate_call(const CallWindow & w, const Enumeration & e) {
    CallCorrelation c;
    if (w.malformed)           { c.per_submission = c.per_call = CorrelationVerdict::MALFORMED_CAPTURE; return c; }
    if (w.stale_reuse_detected){ c.per_submission = c.per_call = CorrelationVerdict::STALE_CAPTURE_REUSED; return c; }
    std::set<unsigned long long> seen;
    for (const auto & r : w.before) seen.insert(r.submission_id);
    std::vector<LedgerRow> fresh;
    for (const auto & r : w.after) if (!seen.count(r.submission_id)) fresh.push_back(r);
    if (fresh.empty()) { c.per_submission = c.per_call = CorrelationVerdict::ZERO_SUBMISSION; return c; }
    if (fresh.size() > 1) {
        c.per_call = CorrelationVerdict::MULTIPLE_SUBMISSIONS_ONLY_LAST_ENUMERATED;
        c.per_submission = CorrelationVerdict::CORRELATED;
        return c;
    }
    const LedgerRow & r = fresh[0];
    c.correlated_id = r.submission_id;
    if (r.submission_rc != 0) { c.per_call = c.per_submission = CorrelationVerdict::SUBMISSION_ERROR; return c; }
    if (!e.valid || e.malformed) { c.per_call = c.per_submission = CorrelationVerdict::INVALID_WINDOW; return c; }
    // pointeur + compte : NECESSAIRES mais reutilisables -> non suffisants
    if (e.gf != r.gf || e.n_declared != r.n_nodes) {
        c.per_call = c.per_submission = CorrelationVerdict::AMBIGUOUS_MAPPING; return c; }
    c.per_call = c.per_submission = CorrelationVerdict::CORRELATED;
    // identite : fingerprint NON confrontable (definition inconnue)
    c.identity = IdentityStrength::POINTER_AND_COUNT_ONLY;
    return c;
}
