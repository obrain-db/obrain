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

static std::string serialize_inventory(const InventoryValidation & v);
static std::string serialize_invocations(const InvocationReconciliation & r);

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
    // --- Y2 : confrontation du fingerprint, EXPORTEE
    o += ",\"fp_verdict\":\"" + std::string(fpverdict_name(r.correlation.fp_verdict)) + "\"";
    o += ",\"fp_read\":\"" + json_esc(r.correlation.fp_read) + "\"";
    o += ",\"fp_recomputed\":\"" + json_esc(r.correlation.fp_recomputed) + "\"";
    snprintf(buf, sizeof buf, ",\"fp_submission_id\":%llu,\"fp_n_nodes\":%d,\"fp_candidates\":%d",
             r.correlation.fp_submission_id, r.correlation.fp_n_nodes,
             r.correlation.fp_candidates); o += buf;
    o += ",\"fp_gf\":\"" + json_esc(r.correlation.fp_gf) + "\"";
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
    // --- WHT : verdict, comptes SEPARES lignes/cibles, portee et limites
    snprintf(buf, sizeof buf, ",\"wht_collected\":%d,\"wht_windows_observed\":%d",
             r.wht_collected ? 1 : 0, r.wht_windows_observed); o += buf;
    o += ",\"wht_verdict\":\"" + std::string(wht_verdict_name(r.wht_verdict)) + "\"";
    snprintf(buf, sizeof buf, ",\"wht_targets_expected\":%d,\"wht_rotation_true\":%d,"
             "\"wht_rotation_false\":%d,\"wht_in_submitted_graph\":%d,"
             "\"wht_role_ok\":%d,\"wht_effect_ok\":%d",
             r.wht_targets_expected, r.wht_rotation_true, r.wht_rotation_false,
             r.wht_in_submitted_graph, r.wht_role_ok, r.wht_effect_ok); o += buf;
    snprintf(buf, sizeof buf, ",\"extractions_total\":%lld,\"wht_max_extractions_per_window\":%d",
             r.extractions_total, r.wht_max_extractions_per_window); o += buf;
    o += ",\"inv_validation_run\":" + std::string(r.inv_validation_run ? "true" : "false");
    o += ",\"inv_globally_complete\":" + std::string(r.inv_globally_complete ? "true" : "false");
    if (!r.inv_serialized.empty()) o += ",\"inventory_validation\":" + r.inv_serialized;
    snprintf(buf, sizeof buf, ",\"inv_valid\":%d,\"inv_absent\":%d,\"inv_ambiguous\":%d,"
             "\"inv_duplicated\":%d,\"inv_identity_null\":%d,\"inv_type_not_tq2\":%d,"
             "\"inv_tq2_outside\":%d", r.inv_valid, r.inv_absent, r.inv_ambiguous,
             r.inv_duplicated, r.inv_identity_null, r.inv_type_not_tq2, r.inv_tq2_outside); o += buf;
    o += ",\"inv_run\":" + std::string(r.inv_run ? "true" : "false");
    if (!r.invi_serialized.empty()) o += ",\"expected_invocations\":" + r.invi_serialized;
    snprintf(buf, sizeof buf, ",\"invi_eligible\":%d,\"invi_expected\":%d,\"invi_captured\":%d,"
             "\"invi_invoked_never_captured\":%d,\"invi_captured_never_invoked\":%d,"
             "\"invi_ambiguous\":%d", r.invi_eligible, r.invi_expected, r.invi_captured,
             r.invi_invoked_never_captured, r.invi_captured_never_invoked, r.invi_ambiguous); o += buf;
    o += ",\"invi_complete\":" + std::string(r.invi_complete ? "true" : "false");
    o += ",\"invi_verdict\":\"" + json_esc(r.invi_verdict) + "\"";
    snprintf(buf, sizeof buf, ",\"scan_nodes\":%d,\"scan_matmul\":%d,\"scan_unregistered\":%d,"
             "\"scan_nonmatmul\":%d,\"scan_complete\":%d",
             r.scan_nodes, r.scan_matmul, r.scan_unregistered, r.scan_nonmatmul,
             r.scan_complete ? 1 : 0); o += buf;
    o += ",\"p4_checks_run\":" + std::string(r.p4_checks_run ? "true" : "false");
    o += ",\"p4_overall\":\"" + json_esc(r.p4_overall) + "\"";
    o += ",\"p4_adopted\":" + std::string(r.p4_adopted ? "true" : "false");
    o += ",\"p4_applies\":" + std::string(r.p4_applies ? "true" : "false");
    o += ",\"validated_role\":\"" + json_esc(r.validated_role) + "\"";
    o += ",\"p4_gate\":\"" + json_esc(r.p4_gate) + "\"";
    o += ",\"p4_property\":\"STRUCTURAL_WHT_PATH_UNREACHABILITY\"";
    o += ",\"p4_real_model_conditions\":\"NOT_EXERCISED\"";
    if (!r.p4_serialized.empty()) o += ",\"p4_checks\":" + r.p4_serialized;
    o += ",\"wht_scope\":\"" + json_esc(r.wht_scope) + "\"";
    snprintf(buf, sizeof buf, ",\"wht_n1\":%d,\"wht_n2\":%d,\"wht_n3\":%d,\"wht_lines\":%d,"
             "\"wht_n_missing\":%d,\"wht_n_unexpected\":%d,\"wht_n1_known\":%d",
             r.wht_n1, r.wht_n2, r.wht_n3, r.wht_lines,
             r.wht_n_missing, r.wht_n_unexpected, r.wht_n1_known ? 1 : 0); o += buf;
    o += ",\"wht_n1_method\":\"" + json_esc(r.wht_n1_method) + "\"";
    o += ",\"wht_missing_identities\":[";
    for (size_t i = 0; i < r.wht_missing_identities.size(); ++i) { if (i) o += ","; o += "\"" + json_esc(r.wht_missing_identities[i]) + "\""; }
    o += "]";
    o += ",\"wht_unexpected_identities\":[";
    for (size_t i = 0; i < r.wht_unexpected_identities.size(); ++i) { if (i) o += ","; o += "\"" + json_esc(r.wht_unexpected_identities[i]) + "\""; }
    o += "]";
    o += ",\"wht_blockers\":[";
    for (size_t i = 0; i < r.wht_blockers.size(); ++i) {
        if (i) o += ","; o += "\"" + json_esc(r.wht_blockers[i]) + "\"";
    }
    o += "]";
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
// Progression de la chaine WHT. On conserve le PIRE maillon atteint sur
// l'ensemble des fenetres : une fenetre suivante valide n'efface JAMAIS un
// defaut anterieur.
static int wht_progress(WhtVerdict v) {
    switch (v) {
        case WhtVerdict::SURFACE_MISSING_OR_MALFORMED: return 0;
        case WhtVerdict::AMBIGUOUS_ASSOCIATION:        return 1;
        case WhtVerdict::INCOMPLETE_COVERAGE:          return 2;
        case WhtVerdict::WHT_CONSTRUCTED:              return 3;
        case WhtVerdict::WHT_CONSUMED_IN_SUBMITTED:    return 4;
        case WhtVerdict::SUBMISSION_OK:                return 5;
        default:                                       return 6;   // EXECUTION_NOT_ESTABLISHED
    }
}

// Agregation WHT : on conserve le PIRE et on ACCUMULE les identites, pour
// qu'un ecrasement de fichiers entre fenetres ne perde pas ce qui est acquis.
static void merge_wht(PipelineReport & r, const WhtAssessment & w, bool first) {
    r.wht_collected = true;
    r.wht_windows_observed++;
    int prog = wht_progress(w.verdict);
    if (r.wht_worst_progress < 0 || prog < r.wht_worst_progress) {
        r.wht_worst_progress = prog; r.wht_verdict = w.verdict;
    }
    r.wht_targets_expected      = w.coverage.n_targets_expected;
    r.wht_rotation_true         = w.coverage.n_rotation_true;
    r.wht_rotation_false        = w.coverage.n_rotation_false;
    r.wht_in_submitted_graph    = w.coverage.n_in_submitted_graph;
    r.wht_role_ok               = w.coverage.n_role_ok;
    r.wht_effect_ok             = w.coverage.n_effect_ok;
    r.wht_scope                 = w.scope;
    if (first || w.recon.n1 >= 0) { r.wht_n1 = w.recon.n1; r.wht_n1_known = (w.recon.n1 >= 0); }
    r.wht_n2 = w.recon.n2; r.wht_n3 = w.recon.n3; r.wht_lines = w.recon.n_lines;
    r.wht_n_missing = w.recon.n_missing; r.wht_n_unexpected = w.recon.n_unexpected;
    r.wht_n1_method = w.recon.verdict;
    for (const auto & x : w.recon.missing_identities) {
        bool d = false; for (const auto & y : r.wht_missing_identities) if (y == x) d = true;
        if (!d) r.wht_missing_identities.push_back(x);
    }
    for (const auto & x : w.recon.unexpected_identities) {
        bool d = false; for (const auto & y : r.wht_unexpected_identities) if (y == x) d = true;
        if (!d) r.wht_unexpected_identities.push_back(x);
    }
    for (const auto & b : w.blockers) {
        bool d = false; for (const auto & y : r.wht_blockers) if (y == b) d = true;
        if (!d) r.wht_blockers.push_back(b);
    }
    if (prog <= 2) { bool d=false; for (const auto&y:r.limits) if (y=="WHT_CHAIN_INCOMPLETE_IN_AT_LEAST_ONE_WINDOW") d=true; if(!d) r.limits.push_back("WHT_CHAIN_INCOMPLETE_IN_AT_LEAST_ONE_WINDOW"); }
    if (prog <= 0) { bool d=false; for (const auto&y:r.limits) if (y=="WHT_SURFACE_MISSING_OR_MALFORMED") d=true; if(!d) r.limits.push_back("WHT_SURFACE_MISSING_OR_MALFORMED"); }
    if (prog == 1) { bool d=false; for (const auto&y:r.limits) if (y=="WHT_ASSOCIATION_AMBIGUOUS") d=true; if(!d) r.limits.push_back("WHT_ASSOCIATION_AMBIGUOUS"); }
    if (!r.wht_n1_known) { bool d=false; for (const auto&y:r.limits) if (y=="WHT_N1_UNKNOWN_BLOCKS_COMPLETENESS") d=true; if(!d) r.limits.push_back("WHT_N1_UNKNOWN_BLOCKS_COMPLETENESS"); }
    if (false) {}
    if (r.wht_n_unexpected > 0) { bool d=false; for (const auto&y:r.limits) if (y=="WHT_UNEXPECTED_CAPTURES") d=true; if(!d) r.limits.push_back("WHT_UNEXPECTED_CAPTURES"); }
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
    r.p4_applies = req.p4_applies;      // portee declaree par l'appelant
    r.validated_role = req.validated_role;
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
    // --- L1 : valider l'inventaire APRES l'acquisition du registre natif
    if (d.inventory_validation) {
        trace(d, "inventory_validation");
        InventoryValidation v = d.inventory_validation();
        r.inv_validation_run = true;
        r.inv_globally_complete = v.inventory_globally_complete;
        r.inv_valid_key_set = v.valid_keys;
        r.inv_valid = (int) v.valid_keys.size();
        r.inv_absent = (int) v.absent_keys.size();
        r.inv_ambiguous = (int) v.ambiguous_keys.size();
        r.inv_duplicated = (int) v.duplicated_keys.size();
        r.inv_identity_null = (int) v.identity_null_keys.size();
        r.inv_type_not_tq2 = (int) v.type_not_tq2_keys.size();
        r.inv_tq2_outside = (int) v.tq2_outside_inventory.size();
        r.inv_serialized = serialize_inventory(v);
        if (!v.inventory_globally_complete) {
            bool dup=false; for (const auto&y:r.limits) if (y=="INVENTORY_NOT_GLOBALLY_COMPLETE") dup=true;
            if (!dup) r.limits.push_back("INVENTORY_NOT_GLOBALLY_COMPLETE");
        }
    }
    // --- P4' : propriete STRUCTURELLE PROPOSEE. Placee APRES la garde et APRES
    // le chargement : elle inspecte le modele, qui n existe qu ici. Elle ne
    // remplace PAS P4.
    if (d.p4_checks) {
        trace(d, "p4_checks");
        P4CheckReport pc = d.p4_checks();
        r.p4_checks_run = true;
        r.p4_overall = cond_verdict_name(pc.overall);
        r.p4_serialized = serialize_p4_checks(pc);
        r.p4_all_pass = pc.all_pass;
        // --- GATE P4' : adoptee. Six conditions PASS -> OPEN. Tout FAIL ou
        // UNKNOWN -> BLOCKED, et le rapport porte la limite. Un FAIL ou UNKNOWN
        // ne peut JAMAIS devenir PASS (C6 est concu pour cela).
        r.p4_gate = pc.all_pass ? "OPEN" : "BLOCKED";
        // --- I1 : LA GATE EST APPLIQUEE, PAS SEULEMENT RAPPORTEE.
        // Evaluee APRES chargement et acquisition du registre, AVANT le contexte
        // et le prefill. FAIL ou UNKNOWN -> zero contexte, zero prefill, zero decode.
        if (r.p4_adopted && r.p4_applies && r.p4_checks_run && !pc.all_pass) {
            { bool d=false; for (const auto&y:r.limits) if (y=="P4PRIME_GATE_BLOCKED_BEFORE_FORWARD") d=true;
              if (!d) r.limits.push_back("P4PRIME_GATE_BLOCKED_BEFORE_FORWARD"); }
            r.stop_reason = StopReason::NONE;
            if (d.release) { trace(d, "release"); d.release(ctx, model); }
            trace(d, "end");
            return r;                       // AUCUN forward n'est tente
        }
        if (!pc.all_pass) {
            bool dup=false; for (const auto&y:r.limits) if (y=="P4PRIME_GATE_BLOCKED") dup=true;
            if (!dup) r.limits.push_back("P4PRIME_GATE_BLOCKED");
        }
    }
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
    long long ex_before = d.extraction_count ? d.extraction_count() : 0;
    CallWindow pre = d.ledger_window();
    trace(d, "prefill");
    if (!d.prefill(ctx, req.prompt_tokens)) {
        r.limits.push_back("PREFILL_FAILED");
        trace(d, "release"); d.release(ctx, model);
        trace(d, "serialize"); r.serialized = serialize_report(r);
        return r;
    }
    r.llama_decode_calls++;              // le prefill EST un appel a llama_decode
    if (d.extraction_count) {
        long long n = d.extraction_count() - ex_before;
        r.extractions_total += n;
        if (n > r.wht_max_extractions_per_window) r.wht_max_extractions_per_window = (int) n;
        if (n > 1) { bool dup=false; for (const auto&y:r.limits) if (y=="WHT_MULTIPLE_EXTRACTIONS_IN_WINDOW") dup=true;
                     if (!dup) r.limits.push_back("WHT_MULTIPLE_EXTRACTIONS_IN_WINDOW"); }
    }
    trace(d, "window_after_prefill");
    pre.after = d.ledger_snapshot();
    if (d.snapshot_after) pre.snap_after = d.snapshot_after();
    r.prefill_windows++;

    // --- 8. COLLECTE / ENUMERATION / CORRELATION, AVANT PEREMPTION
    // f1a_last_gf ne conserve QUE la derniere soumission : on enumere ICI,
    // pas a la fin de la generation.
    trace(d, "enumerate");
    r.enumeration = d.enumerate();
    trace(d, "correlate");
    r.correlation = d.correlate(pre, r.enumeration);   // premier verdict : etat initial
    // --- L1/L3 : invocations ATTENDUES depuis les operations du graphe soumis.
    if (d.submitted_matmul_ops) {
        trace(d, "expected_invocations");
        bool gavail = d.graph_enumeration_available ? d.graph_enumeration_available() : false;
        MatmulScan scan = d.submitted_matmul_ops();
        std::vector<MatmulOp> ops = scan.ops;
        r.scan_nodes = scan.n_nodes; r.scan_matmul = scan.n_matmul_with_weight;
        r.scan_unregistered = scan.n_unregistered_src0;
        r.scan_nonmatmul = scan.n_nonmatmul_with_weight;
        r.scan_complete = scan.complete;
        // Un src0 NON enregistre est une ambiguite : elle est publiee, et elle
        // interdit la completude tant qu'elle n'est pas levee.
        if (scan.n_unregistered_src0 > 0) {
            bool dup=false; for (const auto&y:r.limits) if (y=="MATMUL_SRC0_NOT_IN_REGISTRY") dup=true;
            if (!dup) r.limits.push_back("MATMUL_SRC0_NOT_IN_REGISTRY");
        }
        // L'ensemble ELIGIBLE vient de l'INVENTAIRE VALIDE (reference
        // INDEPENDANTE des captures), jamais du registre de captures. Un poids
        // attendu mais NON capture doit rester dans l'ensemble attendu.
        std::set<std::string> elig = r.inv_valid_key_set;
        ExpectedInvocations ei = compute_expected_invocations(elig, ops, gavail);
        std::set<std::string> captured;
        for (const auto & e : r.registry.entries) captured.insert(e.key);
        InvocationReconciliation ir = reconcile_invocations(ei, elig, captured, (int) ops.size(), r.registry_entries);
        r.inv_run = true;
        r.invi_expected_known = ir.expected_known;
        r.invi_eligible = ir.n_eligible_weights;
        r.invi_expected = ir.n_expected_invocations;
        r.invi_captured = ir.n_captured_targets;
        r.invi_invoked_never_captured = (int) ir.invoked_never_captured.size();
        r.invi_captured_never_invoked = (int) ir.captured_never_invoked.size();
        r.invi_ambiguous = (int) ir.ambiguous_references.size();
        r.invi_complete = ir.complete;
        r.invi_verdict = ir.verdict;
        if (scan.n_unregistered_src0 > 0) {
            // AMBIGUITE : un matmul dont src0 n est pas resolu au registre ne peut
            // pas etre attribue. La completude est INTERDITE et le verdict change.
            ir.complete = false;
            ir.ambiguous_references.insert("<src0_non_resolu>");
            if (ir.verdict == "COMPLETE") ir.verdict = "AMBIGUOUS_REFERENCES";
            r.invi_ambiguous = (int) ir.ambiguous_references.size();
            r.invi_complete = false;
        }
        r.invi_serialized = serialize_invocations(ir);
        if (!ir.expected_known) {
            bool dup=false; for (const auto&y:r.limits) if (y=="EXPECTED_INVOCATIONS_UNKNOWN_BLOCKS_COMPLETENESS") dup=true;
            if (!dup) r.limits.push_back("EXPECTED_INVOCATIONS_UNKNOWN_BLOCKS_COMPLETENESS");
        }
    }
    // --- WHT : collecte dans la MEME fenetre que l'enumeration, pendant que
    // l'objet observe est encore valide.
    if (d.wht_collect) {
        trace(d, "wht_collect");
        trace(d, "expected_targets");
        ExpectedTargets exp = d.expected_targets ? d.expected_targets() : ExpectedTargets();
        merge_wht(r, d.wht_collect(exp), true);
    }

    // --- 9..11. LOGITS, ARGMAX, ARRET, BOUCLE BORNEE
    // Chaque pas de decode a SA PROPRE fenetre d'observation.
    int32_t pending = -1;
    std::vector<llama_token_i> forced;   // vide : la source est le mock de logits
    int produced = 0;
    GenOutcome o;
    o.reason = StopReason::NONE;
    while (produced < req.max_new_tokens) {
        trace(d, "window_before_decode");
        long long ex_b = d.extraction_count ? d.extraction_count() : 0;
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
        if (d.extraction_count) {
            long long n = d.extraction_count() - ex_b;
            r.extractions_total += n;
            if (n > r.wht_max_extractions_per_window) r.wht_max_extractions_per_window = (int) n;
            if (n > 1) { bool dup=false; for (const auto&y:r.limits) if (y=="WHT_MULTIPLE_EXTRACTIONS_IN_WINDOW") dup=true;
                         if (!dup) r.limits.push_back("WHT_MULTIPLE_EXTRACTIONS_IN_WINDOW"); }
        }
        trace(d, "window_after_decode");
        w.after = d.ledger_snapshot();
        if (d.snapshot_after) w.snap_after = d.snapshot_after();
        // enumeration PAR PAS : le graphe observe ne doit pas devenir perime
        r.enumeration = d.enumerate();
        merge_correlation(r.correlation, d.correlate(w, r.enumeration));
        if (d.wht_collect) {
            ExpectedTargets exp2 = d.expected_targets ? d.expected_targets() : ExpectedTargets();
            merge_wht(r, d.wht_collect(exp2), false);
                }
    }
    r.generated = o.ids;
    r.stop_reason = o.reason;
    if (o.reason == StopReason::ERROR) r.limits.push_back("DECODE_OR_LOGIT_ERROR");

    // couverture conservee visible
    if (r.correlation.per_call == CorrelationVerdict::MULTIPLE_SUBMISSIONS_ONLY_LAST_ENUMERATED)
        r.limits.push_back("PER_CALL_COVERAGE_INCOMPLETE");
    if (r.correlation.identity == IdentityStrength::POINTER_AND_COUNT_ONLY)
        r.limits.push_back("IDENTITY_POINTER_AND_COUNT_ONLY");
    if (r.correlation.fp_verdict == FingerprintVerdict::MISMATCH)
        r.limits.push_back("FINGERPRINT_MISMATCH_BLOCKS_CONCLUSION");
    if (r.correlation.fp_verdict == FingerprintVerdict::AMBIGUOUS_JOIN)
        r.limits.push_back("FINGERPRINT_JOIN_AMBIGUOUS_BLOCKS_CONCLUSION");
    if (r.correlation.fp_verdict == FingerprintVerdict::ABSENT_OR_INVALID)
        r.limits.push_back("FINGERPRINT_ABSENT_BLOCKS_CONCLUSION");
    if (r.correlation.fp_verdict == FingerprintVerdict::UNAVAILABLE)
        r.limits.push_back("FINGERPRINT_NOT_RECOMPUTED");
    if (r.correlation.fp_verdict == FingerprintVerdict::MATCH)
        r.limits.push_back("FINGERPRINT_MATCH_IN_PROCESS_ONLY");
    r.limits.push_back("FINGERPRINT_DEFINITION_UNKNOWN");
    r.limits.push_back("NO_NEXTN_CONCLUSION_FROM_THIS_REPORT");

    // Limite structurelle du denombrement : compter les OPERATIONS presentes
    // dans le graphe n'est PAS compter les EXPERTS effectivement selectionnes au
    // runtime. La fixture T4 ne doit pas etre generalisee a cette propriete.
    { bool dup=false; for (const auto&y:r.limits) if (y=="GRAPH_OPS_NOT_RUNTIME_EXPERT_SELECTION") dup=true;
      if (!dup) r.limits.push_back("GRAPH_OPS_NOT_RUNTIME_EXPERT_SELECTION"); }

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
    c.identity = IdentityStrength::POINTER_AND_COUNT_ONLY;

    // ---- Y2 : confrontation du fingerprint ----------------------------------
    // On identifie le snapshot attribuable a CETTE soumission, puis on compare
    // la valeur LUE a la valeur RECALCULEE pendant que le graphe etait valide.
    // Regle stricte : discordance, valeur absente ou jointure ambigue INTERDIT
    // toute conclusion negative issue de cette observation.
    c.fp_submission_id = r.submission_id;
    c.fp_gf            = r.gf;
    c.fp_n_nodes       = r.n_nodes;
    if (!e.fingerprint_available || e.fingerprint_recomputed.empty()) {
        c.fp_verdict = FingerprintVerdict::UNAVAILABLE;
    } else {
        c.fp_recomputed = e.fingerprint_recomputed;
        // candidats : snapshots dont la soumission ou le gf/n_nodes correspondent
        std::vector<LedgerRow> cand;
        for (const auto & sr : w.snap_after) {
            if (sr.submission_id == r.submission_id) cand.push_back(sr);
        }
        if (cand.empty()) { for (const auto & sr : w.snap_after) cand.push_back(sr); }
        c.fp_candidates = (int) cand.size();
        if (cand.empty()) {
            c.fp_verdict = FingerprintVerdict::ABSENT_OR_INVALID;
        } else if (cand.size() > 1) {
            // plusieurs snapshots candidats : jointure AMBIGUE, aucune conclusion
            c.fp_verdict = FingerprintVerdict::AMBIGUOUS_JOIN;
        } else {
            c.fp_read = cand[0].fingerprint;
            c.fp_n_nodes = cand[0].n_nodes;
            if (c.fp_read.empty())                       c.fp_verdict = FingerprintVerdict::ABSENT_OR_INVALID;
            // Comparaison du compte SEULEMENT s il est connu des deux cotes :
            // le ledger ne porte pas n_nodes, l absence de valeur ne doit pas
            // devenir une discordance.
            else if (cand[0].n_nodes >= 0 && e.n_declared > 0 && cand[0].n_nodes != e.n_declared)
                                                          c.fp_verdict = FingerprintVerdict::MISMATCH;
            else if (c.fp_read == e.fingerprint_recomputed) c.fp_verdict = FingerprintVerdict::MATCH;
            else                                          c.fp_verdict = FingerprintVerdict::MISMATCH;
        }
    }
    // Seule une DISCORDANCE est une CONTRADICTION positive : elle degrade la
    // couverture. Une valeur ABSENTE ou une jointure AMBIGUE signifie seulement
    // que le controle n a pas pu etre fait -- elle est consignee comme limite et
    // interdit une conclusion negative, mais elle n ECASE PAS un verdict de
    // couverture deja etabli. Confondre les deux masquait la couverture reelle.
    if (c.fp_verdict == FingerprintVerdict::MISMATCH) {
        if (corr_severity(c.per_call) < corr_severity(CorrelationVerdict::AMBIGUOUS_MAPPING))
            c.per_call = CorrelationVerdict::AMBIGUOUS_MAPPING;
    }
    return c;
}

// =====================================================================
// L1 : SERIALISATION de l'inventaire et des invocations attendues.
// Definitions placees en fin d'unite ; les declarations sont en tete.
// =====================================================================
static std::string jset(const std::set<std::string> & v) {
    std::string o = "["; size_t i = 0;
    for (const auto & x : v) { if (i++) o += ","; o += "\"" + json_esc(x) + "\""; }
    return o + "]";
}
static std::string serialize_inventory(const InventoryValidation & v) {
    std::string o = "{\"schema\":\"ENGINE008_INVENTORY_VALIDATION_v1\"";
    o += ",\"scope\":\"" + json_esc(v.scope) + "\"";
    o += ",\"why_incomplete\":\"" + json_esc(v.why_incomplete) + "\"";
    o += ",\"globally_complete\":" + std::string(v.inventory_globally_complete ? "true" : "false");
    o += ",\"n_inventory_entries\":" + std::to_string(v.n_inventory_entries);
    o += ",\"n_registry_entries\":" + std::to_string(v.n_registry_entries);
    o += ",\"valid_keys\":" + jset(v.valid_keys);
    o += ",\"absent_keys\":" + jset(v.absent_keys);
    o += ",\"ambiguous_keys\":" + jset(v.ambiguous_keys);
    o += ",\"duplicated_keys\":" + jset(v.duplicated_keys);
    o += ",\"identity_null_keys\":" + jset(v.identity_null_keys);
    o += ",\"type_not_tq2_keys\":" + jset(v.type_not_tq2_keys);
    o += ",\"tq2_outside_inventory\":" + jset(v.tq2_outside_inventory);
    o += "}";
    return o;
}
static std::string serialize_invocations(const InvocationReconciliation & r) {
    std::string o = "{\"schema\":\"ENGINE008_EXPECTED_INVOCATIONS_v1\"";
    o += ",\"expected_known\":" + std::string(r.expected_known ? "true" : "false");
    o += ",\"verdict\":\"" + json_esc(r.verdict) + "\"";
    o += ",\"complete\":" + std::string(r.complete ? "true" : "false");
    o += ",\"n_eligible_weights\":" + std::to_string(r.n_eligible_weights);
    o += ",\"n_expected_invocations\":" + std::to_string(r.n_expected_invocations);
    o += ",\"n_captured_targets\":" + std::to_string(r.n_captured_targets);
    o += ",\"n_relations_available\":" + std::to_string(r.n_relations_available);
    o += ",\"invoked_never_captured\":" + jset(r.invoked_never_captured);
    o += ",\"captured_never_invoked\":" + jset(r.captured_never_invoked);
    o += ",\"ambiguous_references\":" + jset(r.ambiguous_references);
    o += "}";
    return o;
}
