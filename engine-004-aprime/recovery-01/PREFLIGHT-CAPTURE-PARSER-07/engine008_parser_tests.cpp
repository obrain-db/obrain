// ENGINE-008 -- PREFLIGHT-CAPTURE-PARSER-07 : tests SANS MODELE.
// Categorie 1 : captures historiques Q1 conservees brutes (lecture seule).
// Categorie 2 : fixtures SYNTHETIQUES, explicitement identifiees comme telles.
#include "engine008_capture_parser.h"
#include "engine008_real_deps.h"
#include "ggml.h"
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

static int failed = 0;
static void ck(const char * n, bool ok) {
    printf("  %-68s %s\n", n, ok ? "PASS" : "FAIL");
    if (!ok) failed++;
}

static const char * HDR =
    "run_id,event_sequence_id,event_kind,decode_step,compute_submission_id,"
    "graph_ptr,graph_fingerprint,capture_epoch_id,eligible_target_count,"
    "captured_target_count\n";

int main(int argc, char ** argv) {
    printf("ENGINE-008 -- PREFLIGHT-CAPTURE-PARSER-07 : parseurs et fenetres\n\n");
    const std::string hist = (argc > 1) ? argv[1] : "";

    // =============================================== CATEGORIE 1 : HISTORIQUES
    printf("=== CATEGORIE 1 : CAPTURES HISTORIQUES Q1 (lecture seule) ===\n");
    if (!hist.empty()) {
        LedgerFile l = read_ledger_file(hist + "/EVENT_LEDGER.csv");
        printf("  [source: %s]\n", hist.c_str());
        ck("historical_ledger_valid", l.state == FileState::VALID);
        ck("historical_ledger_rows_present", l.rows.size() > 0);
        ck("historical_event_kinds_preserved", l.kinds.size() >= 1);
        { bool all_seq = true, all_fp = true;
          for (const auto & e : l.rows) { if (e.seq == 0) all_seq = false; if (e.fingerprint.empty()) all_fp = false; }
          ck("historical_sequential_ids_nonzero", all_seq);
          ck("historical_fingerprints_present", all_fp); }
        ck("historical_no_duplicate_seq", l.duplicate_seq == 0);
        SnapshotFile s = read_snapshots_file(hist + "/SUBMITTED_GRAPH_SNAPSHOTS.csv");
        printf("  snapshot state=%s rows=%zu\n", file_state_name(s.state), s.rows.size());
        ck("historical_snapshot_schema_recognised", s.state == FileState::VALID || s.state == FileState::ABSENT);
        if (s.state == FileState::VALID) {
            bool all = true;
            for (const auto & r : s.rows) if (r.n_nodes <= 0 || r.fingerprint.size() != 16) all = false;
            ck("historical_snapshot_values_wellformed", all);
        }
    } else {
        ck("historical_path_provided", false);
    }

    // =============================================== CATEGORIE 2 : SYNTHETIQUES
    printf("\n=== CATEGORIE 2 : FIXTURES SYNTHETIQUES (identifiees comme telles) ===\n");
    { auto f = parse_ledger("", false);
      ck("absent_file_detected", f.state == FileState::ABSENT); }
    { auto f = parse_ledger("", true);
      ck("empty_file_detected", f.state == FileState::EMPTY); }
    { auto f = parse_ledger(HDR, true);
      ck("header_only_is_empty_not_valid", f.state == FileState::EMPTY); }
    { auto f = parse_ledger("a,b,c\n1,2,3\n", true);
      ck("wrong_header_malformed", f.state == FileState::MALFORMED); }
    { std::string b = HDR; b += "R,1,COMPUTE_SUBMIT,0,1,0x7f00,abc,NA,10,10\n";
      auto f = parse_ledger(b, true);
      ck("single_submission_valid", f.state == FileState::VALID && f.rows.size() == 1); }
    { std::string b = HDR;
      b += "R,1,COMPUTE_SUBMIT,0,1,0x7f00,aaa,NA,10,10\n";
      b += "R,2,COMPUTE_SUBMIT,1,2,0x7f00,bbb,NA,10,10\n";
      auto f = parse_ledger(b, true);
      ck("multiple_submissions_all_preserved", f.state == FileState::VALID && f.rows.size() == 2); }
    { std::string b = HDR; b += "R,1,COMPUTE_SUBMIT,0,1,0x7f00,aaa,NA,10\n";  // 9 champs
      auto f = parse_ledger(b, true);
      ck("partial_line_is_partial_not_valid", f.state == FileState::PARTIAL); }
    { std::string b = HDR; b += "R,X,COMPUTE_SUBMIT,0,1,0x7f00,aaa,NA,10,10\n";
      auto f = parse_ledger(b, true);
      ck("non_numeric_sequence_malformed", f.state == FileState::MALFORMED); }
    { std::string b = HDR; b += "R,1,COMPUTE_SUBMIT,0,1,NOTAPTR,aaa,NA,10,10\n";
      auto f = parse_ledger(b, true);
      ck("bad_pointer_malformed_no_silent_conversion", f.state == FileState::MALFORMED); }
    { std::string b = HDR;
      b += "R,1,COMPUTE_SUBMIT,0,1,0x7f00,aaa,NA,10,10\n";
      b += "R,1,COMPUTE_SUBMIT,1,2,0x7f00,bbb,NA,10,10\n";
      auto f = parse_ledger(b, true);
      ck("duplicate_sequence_detected", f.state == FileState::VALID && f.duplicate_seq == 1); }
    { std::string b = HDR; b += "R,1,CAPTURE_TARGET,0,1,0x7f00,abc,NA,10,3\n";
      auto f = parse_ledger(b, true);
      ck("event_kind_preserved_not_filtered", f.kinds.size() == 1 && f.kinds[0] == "CAPTURE_TARGET"); }
    { std::string b = "submission_id,gf,n_nodes,submission_rc,fingerprint\n1,0x7f00,10,0,0123456789abcdef\n";
      auto f = parse_snapshots(b, true);
      ck("snapshot_valid", f.state == FileState::VALID && f.rows.size() == 1); }
    { std::string b = "submission_id,gf,n_nodes,submission_rc,fingerprint\n1,0x7f00,10,7,0123456789abcdef\n";
      auto f = parse_snapshots(b, true);
      ck("snapshot_submission_error_preserved", f.rows.size() == 1 && f.rows[0].submission_rc == 7); }
    { std::string b = "submission_id,gf\n1,0x7f00\n";
      auto f = parse_snapshots(b, true);
      ck("snapshot_wrong_header_malformed", f.state == FileState::MALFORMED); }
    { std::string b = "submission_id,gf,n_nodes,submission_rc,fingerprint\n1,0x7f00,10,0,aa\n1,0x7f00,10,0,bb\n";
      auto f = parse_snapshots(b, true);
      ck("snapshot_duplicate_id_detected", f.duplicate_submission_id == 1); }

    // =============================================== FENETRES
    printf("\n=== FENETRES AVANT/APRES ===\n");
    const std::string D = "/tmp/win07";
    auto write_file = [](const std::string & p, const std::string & c) {
        FILE * f = fopen(p.c_str(), "w"); if (f) { fwrite(c.data(), 1, c.size(), f); fclose(f); } };
    { std::string lp = D + "/EVENT_LEDGER.csv", sp = D + "/SUB.csv";
      system(("mkdir -p " + D).c_str());
      write_file(lp, HDR); write_file(sp, "submission_id,gf,n_nodes,submission_rc,fingerprint\n");
      WindowCursor c = arm_window(lp, sp);
      auto o = observe_window(c, lp, sp);
      ck("zero_new_submission", o.n_new_submissions == 0); }
    { std::string lp = D + "/EVENT_LEDGER.csv", sp = D + "/SUB.csv";
      write_file(lp, HDR); write_file(sp, "submission_id,gf,n_nodes,submission_rc,fingerprint\n");
      WindowCursor c = arm_window(lp, sp);
      write_file(lp, std::string(HDR) + "R,1,COMPUTE_SUBMIT,0,1,0x7f00,aa,NA,10,10\n");
      write_file(sp, "submission_id,gf,n_nodes,submission_rc,fingerprint\n1,0x7f00,10,0,aa\n");
      auto o = observe_window(c, lp, sp);
      ck("one_new_submission_detected", o.n_new_submissions == 1);
      ck("new_event_content_available", o.new_events.size() == 1 && o.new_events[0].fingerprint == "aa"); }
    { std::string lp = D + "/EVENT_LEDGER.csv", sp = D + "/SUB.csv";
      write_file(lp, HDR); write_file(sp, "submission_id,gf,n_nodes,submission_rc,fingerprint\n");
      WindowCursor c = arm_window(lp, sp);
      write_file(lp, std::string(HDR)
        + "R,1,COMPUTE_SUBMIT,0,1,0x7f00,aa,NA,10,10\nR,2,COMPUTE_SUBMIT,1,2,0x7f00,bb,NA,10,10\n");
      auto o = observe_window(c, lp, sp);
      ck("multiple_new_submissions_detected", o.n_new_submissions == 2); }
    { std::string lp = D + "/EVENT_LEDGER.csv", sp = D + "/SUB.csv";
      write_file(lp, std::string(HDR) + "R,1,COMPUTE_SUBMIT,0,1,0x7f00,aa,NA,10,10\n"
                                       "R,2,COMPUTE_SUBMIT,1,2,0x7f00,bb,NA,10,10\n");
      write_file(sp, "submission_id,gf,n_nodes,submission_rc,fingerprint\n");
      WindowCursor c = arm_window(lp, sp);
      write_file(lp, HDR);   // TRONCATURE / REMPLACEMENT
      auto o = observe_window(c, lp, sp);
      ck("truncation_or_replacement_detected", o.truncated_or_replaced); }
    { std::string lp = D + "/EVENT_LEDGER.csv", sp = D + "/SUB.csv";
      write_file(lp, HDR); write_file(sp, "submission_id,gf,n_nodes,submission_rc,fingerprint\n");
      WindowCursor c = arm_window(lp, sp);
      write_file(lp, "corrompu\n");
      auto o = observe_window(c, lp, sp);
      ck("malformed_capture_blocks_conclusion", o.ledger_state == FileState::MALFORMED); }
    { std::string lp = D + "/EVENT_LEDGER.csv", sp = D + "/SUB.csv";
      write_file(lp, HDR); write_file(sp, "submission_id,gf,n_nodes,submission_rc,fingerprint\n");
      WindowCursor c = arm_window(lp, sp);
      write_file(sp, "submission_id,gf,n_nodes,submission_rc,fingerprint\n9,0x7f00,10,5,zz\n");
      auto o = observe_window(c, lp, sp);
      ck("submission_error_flagged", o.submission_error_present); }
    { std::string lp = D + "/EVENT_LEDGER.csv", sp = D + "/SUB.csv";
      write_file(lp, HDR); write_file(sp, "submission_id,gf,n_nodes,submission_rc,fingerprint\n");
      WindowCursor c = arm_window(lp, sp);
      write_file(lp, std::string(HDR) + "R,1,K,0,1,0x7f00,aa,NA,10,10\n"
                                       "R,1,K,1,1,0x7f00,bb,NA,10,10\n");
      auto o = observe_window(c, lp, sp);
      ck("duplicate_ids_detected_in_window", o.duplicated_ids); }

    // =============================================== FINGERPRINT (definition exacte)
    printf("\n=== FINGERPRINT : definition du producteur, recalcul ===\n");
    { struct ggml_init_params p = { 1u << 20, nullptr, true };
      struct ggml_context * ctx = ggml_init(p);
      struct ggml_tensor * a = ggml_new_tensor_1d(ctx, GGML_TYPE_F32, 4);
      struct ggml_tensor * b = ggml_new_tensor_1d(ctx, GGML_TYPE_F32, 4);
      struct ggml_tensor * c = ggml_add(ctx, a, b);
      struct ggml_cgraph * g = ggml_new_graph(ctx);
      ggml_build_forward_expand(g, c);
      unsigned long long f1 = recompute_graph_fingerprint((const void *) g);
      unsigned long long f2 = recompute_graph_fingerprint((const void *) g);
      ck("fingerprint_deterministic_same_object", f1 == f2 && f1 != 0);
      ck("fingerprint_nonzero_offset_applied", f1 != 1469598103934665603ULL);
      // un graphe DIFFERENT (autre objet) donne une empreinte differente :
      // le fingerprint depend des ADRESSES.
      struct ggml_tensor * d = ggml_new_tensor_1d(ctx, GGML_TYPE_F32, 4);
      struct ggml_cgraph * g2 = ggml_new_graph(ctx);
      ggml_build_forward_expand(g2, d);
      unsigned long long f3 = recompute_graph_fingerprint((const void *) g2);
      ck("fingerprint_differs_for_other_graph", f3 != f1);
      ck("fingerprint_null_graph_zero", recompute_graph_fingerprint(nullptr) == 0);
      ggml_free(ctx); }

    printf("\nHISTORICAL_CAPTURE_PARSER_VALIDATION=%s\n",
           (hist.empty() ? "NOT_RUN" : (failed == 0 ? "PASS" : "FAIL")));
    printf("FILE_TO_CORRELATION_SELFTEST=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("FAILED_COUNT=%d\n", failed);
    printf("FULL_MODEL_LOAD_COUNT=0\nREAL_CONTEXT_CREATE_COUNT=0\nMODEL_FORWARD_CALL_COUNT=0\n");
    return failed == 0 ? 0 : 1;
}
