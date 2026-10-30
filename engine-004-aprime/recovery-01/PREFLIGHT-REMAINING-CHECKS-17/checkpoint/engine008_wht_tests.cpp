// ENGINE-008 -- WHT-CAPTURE-CONNECT-12 : tests SANS MODELE.
// Fixtures SYNTHETIQUES explicitement declarees + captures historiques brutes.
#include "engine008_wht_parser.h"
#include <cstdio>
#include <fstream>
#include <sstream>

static int failed = 0;
static void ck(const char * n, bool ok) {
    printf("  %-66s %s\n", n, ok ? "PASS" : "FAIL");
    if (!ok) failed++;
}

static const char * MH =
  "run_id,epoch,target_id,capture_id,weight_name,family,expected_K,rotation_applied,"
  "kind,path,dtype,ne0,ne1,ne2,ne3,nb0,nb1,nb2,nb3,logical_elems,bytes,contiguous,"
  "decode_step,compute_submission_id\n";
static const char * RH =
  "epoch,target_id,weight_name,sub_at_build,cur_op,cur_src0_is_effective,mulmat_op,"
  "mulmat_src0_is_weight,mulmat_src1_is_cur,mmcap_op,mmcap_src0_is_mmraw,downstream_reach,"
  "role_ok,effect_class\n";
static const char * EH =
  "epoch,target_id,weight_name,role,effect_class,src0_is_weight,src1_is_cur,dst_is_cur,effect_ok\n";
static const char * SH =
  "target_id,capture_id,role,tensor_ptr,tensor_name,op,type,ne0,nb0,nbytes,contiguous,"
  "buffer_ptr,resident,in_submitted_graph,weight_name\n";

static std::string mrow(int tid,int cid,const char*wn,int rot,const char*kind,int ds,int sub){
    char b[512]; snprintf(b,sizeof b,
      "Q1,0,%d,%d,%s,5,5120,%d,%s,/cap/x.bin,0,5120,1,1,1,4,20480,20480,20480,5120,20480,1,%d,%d\n",
      tid,cid,wn,rot,kind,ds,sub); return b; }
static std::string rrow(int tid,const char*wn,int sub_at_build,int role_ok,int reach){
    char b[512]; snprintf(b,sizeof b,"0,%d,%s,%d,1,1,29,1,1,1,1,%d,%d,OUTPUT_DEPENDENT\n",
      tid,wn,sub_at_build,reach,role_ok); return b; }
static std::string erow(int tid,const char*wn,int ok){
    char b[512]; snprintf(b,sizeof b,"0,%d,%s,MM_CAPTURE,OUTPUT_DEPENDENT,1,1,1,%d\n",tid,wn,ok); return b; }
static std::string srow(int tid,int cid,int insub,int resident){
    char b[512]; snprintf(b,sizeof b,"%d,%d,CUR_CAPTURE,0x7f00,node_1,1,0,5120,4,20480,1,0xbe00,%d,%d,w\n",
      tid,cid,resident,insub); return b; }

int main(int argc, char ** argv) {
    printf("ENGINE-008 -- WHT-CAPTURE-CONNECT-12 : surfaces WHT\n\n");
    const std::string hist = (argc > 1) ? argv[1] : "";

    printf("=== CATEGORIE 1 : CAPTURES HISTORIQUES Q1 (lecture seule) ===\n");
    if (!hist.empty()) {
        auto m = read_wht_manifest(hist);
        printf("  [%s]\n", hist.c_str());
        ck("historical_manifest_valid", m.state == FileState::VALID);
        ck("historical_rows_672", m.rows.size() == 672);
        ck("historical_distinct_identities_672_includes_kind", m.n_distinct_identities == 672);
        ck("historical_distinct_targets_336_not_672", m.n_distinct_targets == 336);
        ck("historical_kinds_are_activation_and_base_output",
           m.kinds.size() == 2);
        ck("historical_rotation_true_nonzero", m.rotation_true > 0);
        ck("historical_rotation_true_rows_672", m.rotation_true == 672 && m.rotation_false == 0);
        ck("historical_rotation_true_distinct_targets_336", m.rotation_true_targets == 336);
        ck("historical_rotation_field_never_missing", m.missing_rotation_field == 0);
        ck("historical_no_duplicate_on_full_identity",
         m.duplicate_target_id == 0);   // (run, epoch, target_id, capture_id)
      ck("historical_target_id_is_per_epoch_not_global",
         m.n_distinct_targets <= (int) m.rows.size());
        ck("historical_kind_present", !m.rows.empty() && !m.rows[0].kind.empty());
        ck("historical_submission_id_present", !m.rows.empty() && m.rows[0].compute_submission_id > 0);
        printf("  (ces captures ne sont PAS des observations du nouveau binaire)\n");
    } else ck("historical_path_provided", false);

    printf("\n=== CATEGORIE 2 : FIXTURES SYNTHETIQUES ===\n");
    { auto m = parse_wht_manifest("", false);
      ck("manifest_absent_detected", m.state == FileState::ABSENT); }
    { auto m = parse_wht_manifest("a,b\n1,2\n", true);
      ck("manifest_wrong_header_malformed", m.state == FileState::MALFORMED); }
    { std::string b = MH; b += mrow(1,1,"blk.8.attn_gate.weight",1,"activation",0,1);
      b += "Q1,0,2,2,w,5,5120,1,activation,/x,0,5120,1,1,1,4,20480,20480,20480,5120,20480,1,0\n"; // 23 champs
      auto m = parse_wht_manifest(b, true);
      ck("manifest_partial_line_detected", m.state == FileState::PARTIAL); }
    { std::string b = MH; b += "Q1,0,X,1,w,5,5120,1,activation,/x,0,5120,1,1,1,4,20480,20480,20480,5120,20480,1,0,1\n";
      auto m = parse_wht_manifest(b, true);
      ck("manifest_non_numeric_target_malformed", m.state == FileState::MALFORMED); }
    { std::string b = MH; b += mrow(1,1,"w",9,"activation",0,1);   // rotation invalide
      auto m = parse_wht_manifest(b, true);
      ck("manifest_unusable_rotation_counted_not_assumed",
         m.state == FileState::VALID && m.missing_rotation_field == 1 &&
         m.rows[0].rotation_applied == -1); }
    { std::string b = MH; b += mrow(1,1,"w",1,"activation",0,1) + mrow(1,1,"w",1,"activation",0,1);
      auto m = parse_wht_manifest(b, true);
      ck("manifest_duplicate_identical_identity_detected", m.duplicate_target_id == 1);
      ck("same_target_two_kinds_is_not_duplicate",
         parse_wht_manifest(std::string(MH) + mrow(1,1,"w",1,"activation",0,1)
                                           + mrow(1,1,"w",1,"base_output",0,1), true).duplicate_target_id == 0);
      ck("same_target_different_capture_is_not_duplicate",
         parse_wht_manifest(std::string(MH) + mrow(1,1,"w",1,"activation",0,1) + mrow(1,2,"w",1,"activation",0,1), true).duplicate_target_id == 0); }

    printf("\n=== CHAINE CANDIDATE COMPLETE ===\n");
    { std::string mb = MH; mb += mrow(1,1,"blk.8.attn_gate.weight",1,"activation",0,7);
      std::string rb = RH; rb += rrow(1,"blk.8.attn_gate.weight",1,1,1);
      std::string eb = EH; eb += erow(1,"blk.8.attn_gate.weight",1);
      std::string sb = SH; sb += srow(1,1,1,1);
      auto n1 = compute_n1({"blk.8.attn_gate.weight"}, {{"blk.8.attn_gate.weight",3}}, "/i", 3);   // N1 CONNU
      auto a = assess_wht(parse_wht_manifest(mb,true), parse_role_relations(rb,true),
                          parse_effect_path(eb,true), parse_residency(sb,true), n1);
      ck("coverage_denominator_is_manifest_not_joined_rows",
         a.coverage.n_targets_expected == 1 && a.coverage.n_targets_with_roles == 1);
      ck("candidate_chain_constructed_and_consumed",
         a.verdict == WhtVerdict::EXECUTION_NOT_ESTABLISHED &&
         a.scope.find("CONSOMME") != std::string::npos);
      ck("execution_never_claimed_from_submission_success",
         a.scope.find("EXECUTION NON ETABLIE") != std::string::npos);
      ck("coverage_reports_targets_and_submitted_graph",
         a.coverage.n_targets_expected == 1 && a.coverage.n_in_submitted_graph == 1 &&
         a.coverage.n_role_ok == 1 && a.coverage.n_effect_ok == 1); }

    printf("\n=== COUVERTURE : DENOMINATEUR ET PARTIALITE ===\n");
    { // A. cible ATTENDUE non capturee -> MISSING_EXPECTED_TARGETS, identite nommee
      std::string mb = MH;
      mb += "Q1,0,1,1,w,5,5120,1,activation,/x,0,5120,1,1,1,4,20480,20480,20480,5120,20480,1,0,1\n";
      std::string rb = RH; rb += rrow(1,"w",1,1,1);
      std::string eb = EH; eb += erow(1,"w",1);
      std::string sb = SH; sb += srow(1,1,1,1);
      auto n1p = compute_n1({"w","w2"}, {{"w",3},{"w2",3}}, "/i", 3);
      auto a = assess_wht(parse_wht_manifest(mb,true), parse_role_relations(rb,true),
                          parse_effect_path(eb,true), parse_residency(sb,true), n1p);
      ck("missing_expected_target_blocks_before_relations",
         a.verdict == WhtVerdict::INCOMPLETE_COVERAGE && a.recon.n_missing == 1);
      ck("missing_expected_identity_named",
         a.recon.missing_identities.size() == 1 && a.recon.missing_identities[0] == "w2");
      ck("n1_n2_n3_kept_separate", a.recon.n1 == 2 && a.recon.n2 == 1); }
    { // B. TOUTES les cibles attendues capturees, mais relations INCOMPLETES
      std::string mb = MH;
      mb += "Q1,0,1,1,w,5,5120,1,activation,/x,0,5120,1,1,1,4,20480,20480,20480,5120,20480,1,0,1\n";
      mb += "Q1,1,2,2,w,5,5120,1,activation,/x,0,5120,1,1,1,4,20480,20480,20480,5120,20480,1,0,1\n";
      std::string rb = RH; rb += rrow(1,"w",1,1,1);          // epoch 0 seul
      std::string eb = EH; eb += erow(1,"w",1);
      std::string sb = SH; sb += srow(1,1,1,1);
      auto n1b = compute_n1({"w"}, {{"w",3}}, "/i", 3);
      auto a = assess_wht(parse_wht_manifest(mb,true), parse_role_relations(rb,true),
                          parse_effect_path(eb,true), parse_residency(sb,true), n1b);
      ck("relations_partial_when_all_expected_captured",
         a.verdict == WhtVerdict::INCOMPLETE_COVERAGE && a.recon.n2 == 2 && a.recon.n3 == 1);
      ck("partial_coverage_blocker_names_the_ratio",
         !a.blockers.empty() && a.blockers.back().find("cibles couvertes sur") != std::string::npos);
      ck("partial_coverage_does_not_claim_full_chain",
         a.scope.find("COUVERTURE PARTIELLE") != std::string::npos); }
    { // ligne SUPPLEMENTAIRE d un autre kind SANS nouvelle cible : ne doit pas
      // gonfler le denominateur ni la couverture
      std::string mb = MH;
      mb += "Q1,0,1,1,w,5,5120,1,activation,/x,0,5120,1,1,1,4,20480,20480,20480,5120,20480,1,0,1\n";
      mb += "Q1,0,1,1,w,5,5120,1,base_output,/y,0,5120,1,1,1,4,20480,20480,20480,5120,20480,1,0,1\n";
      std::string rb = RH; rb += rrow(1,"w",1,1,1);
      std::string eb = EH; eb += erow(1,"w",1);
      std::string sb = SH; sb += srow(1,1,1,1);
      auto n1k = compute_n1({"w"}, {{"w",3}}, "/i", 3);
      auto a = assess_wht(parse_wht_manifest(mb,true), parse_role_relations(rb,true),
                          parse_effect_path(eb,true), parse_residency(sb,true), n1k);
      ck("extra_kind_line_does_not_change_target_denominator",
         a.coverage.n_targets_expected == 1 && a.coverage.n_rotation_true == 2); }
    { // capture REPETEE de la meme cible : meme identite -> detectee
      std::string mb = MH;
      mb += "Q1,0,1,1,w,5,5120,1,activation,/x,0,5120,1,1,1,4,20480,20480,20480,5120,20480,1,0,1\n";
      mb += "Q1,0,1,1,w,5,5120,1,activation,/x,0,5120,1,1,1,4,20480,20480,20480,5120,20480,1,0,1\n";
      auto m = parse_wht_manifest(mb,true);
      ck("repeated_identical_capture_detected", m.duplicate_target_id == 1); }

    printf("\n=== MAILLONS MANQUANTS, CHACUN NOMME ===\n");
    { std::string mb = MH; mb += mrow(1,1,"w",1,"activation",0,7);
      auto a = assess_wht(parse_wht_manifest(mb,true), parse_role_relations("",false),
                          parse_effect_path("",false), parse_residency("",false));
      ck("missing_surfaces_block_submission_link",
         a.verdict == WhtVerdict::INCOMPLETE_COVERAGE &&
         a.blockers.size() >= 3); }
    { std::string mb = MH; mb += mrow(1,1,"w",1,"activation",0,7);
      std::string rb = RH; rb += rrow(1,"w",0,1,1);          // PAS dans le graphe soumis
      std::string eb = EH; eb += erow(1,"w",1);
      std::string sb = SH; sb += srow(1,1,0,1);              // in_submitted_graph=0
      auto a = assess_wht(parse_wht_manifest(mb,true), parse_role_relations(rb,true),
                          parse_effect_path(eb,true), parse_residency(sb,true));
      ck("not_in_submitted_graph_blocks", a.verdict == WhtVerdict::INCOMPLETE_COVERAGE); }
    { std::string mb = MH; mb += mrow(1,1,"w",0,"activation",0,7);   // rotation FAUSSE
      auto a = assess_wht(parse_wht_manifest(mb,true), parse_role_relations("",false),
                          parse_effect_path("",false), parse_residency("",false));
      ck("rotation_false_is_incomplete_not_absence",
         a.verdict == WhtVerdict::INCOMPLETE_COVERAGE &&
         a.scope.find("jamais une preuve d absence") != std::string::npos); }
    { auto a = assess_wht(parse_wht_manifest("",false), parse_role_relations("",false),
                          parse_effect_path("",false), parse_residency("",false));
      ck("missing_rotation_surface_is_explicit", a.verdict == WhtVerdict::SURFACE_MISSING_OR_MALFORMED); }
    { std::string mb = MH; mb += mrow(1,1,"w",1,"activation",0,7) + mrow(1,1,"w",1,"activation",0,7);
      auto a = assess_wht(parse_wht_manifest(mb,true), parse_role_relations("",false),
                          parse_effect_path("",false), parse_residency("",false));
      ck("ambiguous_association_blocks_conclusion", a.verdict == WhtVerdict::AMBIGUOUS_ASSOCIATION); }
    { std::string mb = MH; mb += mrow(1,1,"w",1,"cache",0,7);
      std::string rb = RH; rb += rrow(1,"w",1,1,1);
      std::string eb = EH; eb += erow(1,"w",1);
      std::string sb = SH; sb += srow(1,1,1,1);
      auto a = assess_wht(parse_wht_manifest(mb,true), parse_role_relations(rb,true),
                          parse_effect_path(eb,true), parse_residency(sb,true));
      ck("cache_origin_kind_accepted_as_construction", a.scope.find("CONSTRUITE") != std::string::npos); }
    { std::string mb = MH; mb += mrow(1,1,"w",1,"activation",0,0);   // soumission 0
      auto m = parse_wht_manifest(mb,true);
      ck("submission_error_visible_in_join_field",
         m.state == FileState::VALID && m.rows[0].compute_submission_id == 0); }

    printf("\n=== N1 : CIBLES ATTENDUES, INDEPENDANTES DES CAPTURES ===\n");
    { std::vector<std::string> inv = {"blk.8.attn_gate.weight","blk.9.attn_gate.weight"};
      std::vector<std::pair<std::string,int>> model = {
        {"blk.8.attn_gate.weight",3},{"blk.9.attn_gate.weight",3},{"blk.10.ffn_gate.weight",0}};
      auto e = compute_n1(inv, model, "/inv.csv", 3);
      ck("n1_known_from_inventory_and_types", e.state == N1State::KNOWN && e.identities.size() == 2);
      ck("n1_ignores_weights_without_tq2_type", e.n_model_tq2g128 == 2 && e.identities.count("blk.10.ffn_gate.weight") == 0);
      ck("n1_counts_inventory_and_model_separately", e.n_inventory_names == 2 && e.n_model_tq2g128 == 2); }
    { auto e = compute_n1({}, {{"a",3}}, "/inv.csv", 3);
      ck("n1_unknown_without_inventory", e.state == N1State::UNKNOWN);
      auto r = reconcile_coverage(e, WhtManifestFile(), RoleRelFile(), EffectPathFile());
      ck("n1_unknown_blocks_completeness", !r.complete && r.n1 == -1 &&
           r.verdict == "N1_UNKNOWN_BLOCKS_COMPLETENESS"); }
    { auto e = compute_n1({"a"}, {}, "/inv.csv", 3);
      ck("n1_unknown_without_native_registry", e.state == N1State::UNKNOWN); }
    { // N2>0 et N3==N2 NE SUFFISENT PAS si N1 est inconnu
      std::string mb = MH; mb += mrow(1,1,"w",1,"activation",0,1);
      std::string rb = RH; rb += rrow(1,"w",1,1,1);
      std::string eb = EH; eb += erow(1,"w",1);
      std::string sb = SH; sb += srow(1,1,1,1);
      auto m = parse_wht_manifest(mb,true); auto rr = parse_role_relations(rb,true);
      auto ee = parse_effect_path(eb,true); auto ss = parse_residency(sb,true);
      auto a = assess_wht(m, rr, ee, ss, ExpectedTargets());
      ck("n2_positive_and_n3_equal_n2_never_complete_without_n1",
         a.verdict == WhtVerdict::INCOMPLETE_COVERAGE &&
         a.scope.find("N1 INCONNU") != std::string::npos); }
    { // cible ATTENDUE absente -> incomplete, identite nommee
      std::vector<std::string> inv = {"w","w2","w3"};
      std::vector<std::pair<std::string,int>> model = {{"w",3},{"w2",3},{"w3",3}};
      auto e = compute_n1(inv, model, "/i", 3);
      std::string mb = MH; mb += mrow(1,1,"w",1,"activation",0,1);
      std::string rb = RH; rb += rrow(1,"w",1,1,1);
      std::string eb = EH; eb += erow(1,"w",1);
      std::string sb = SH; sb += srow(1,1,1,1);
      auto a = assess_wht(parse_wht_manifest(mb,true), parse_role_relations(rb,true),
                          parse_effect_path(eb,true), parse_residency(sb,true), e);
      ck("missing_expected_targets_detected",
         a.recon.n1 == 3 && a.recon.n_missing == 2 && a.verdict == WhtVerdict::INCOMPLETE_COVERAGE);
      ck("missing_identities_named",
         a.recon.missing_identities.size() == 2); }
    { // capture INATTENDUE -> ambigu, aucune conclusion
      std::vector<std::string> inv = {"w"};
      auto e = compute_n1(inv, {{"w",3}}, "/i", 3);
      std::string mb = MH; mb += mrow(1,1,"w",1,"activation",0,1) + mrow(1,2,"ZZZ",1,"activation",0,1);
      auto a = assess_wht(parse_wht_manifest(mb,true), parse_role_relations("",false),
                          parse_effect_path("",false), parse_residency("",false), e);
      ck("unexpected_capture_is_ambiguous",
         a.recon.n_unexpected == 1 && a.verdict == WhtVerdict::AMBIGUOUS_ASSOCIATION); }
    { // journalisation complete : N1=N2=N3
      std::vector<std::string> inv = {"w"};
      auto e = compute_n1(inv, {{"w",3}}, "/i", 3);
      std::string mb = MH; mb += mrow(1,1,"w",1,"activation",0,1) + mrow(1,1,"w",1,"base_output",0,1);
      std::string rb = RH; rb += rrow(1,"w",1,1,1);
      std::string eb = EH; eb += erow(1,"w",1);
      std::string sb = SH; sb += srow(1,1,1,1);
      auto a = assess_wht(parse_wht_manifest(mb,true), parse_role_relations(rb,true),
                          parse_effect_path(eb,true), parse_residency(sb,true), e);
      ck("complete_coverage_when_n1_known_and_all_present",
         a.recon.complete && a.recon.n1 == 1 && a.recon.n2 == 1 && a.recon.n3 == 1);
      ck("kinds_not_summed_as_targets", a.recon.n_lines == 2 && a.recon.n2 == 1);
      ck("complete_does_not_skip_chain_checks",
         a.verdict != WhtVerdict::SURFACE_MISSING_OR_MALFORMED); }
    { // PLUSIEURS EPOCHS : N2 compte les couples, pas les lignes
      std::string mb = MH;
      mb += mrow(1,1,"w",1,"activation",0,1) + mrow(1,1,"w",1,"base_output",0,1);
      mb += "Q1,1,1,2,w,5,5120,1,activation,/x,0,5120,1,1,1,4,20480,20480,20480,5120,20480,1,0,2\n";
      mb += "Q1,1,1,2,w,5,5120,1,base_output,/y,0,5120,1,1,1,4,20480,20480,20480,5120,20480,1,0,2\n";
      auto r = reconcile_coverage(ExpectedTargets(), parse_wht_manifest(mb,true), RoleRelFile(), EffectPathFile());
      ck("multi_epoch_counts_pairs_not_lines", r.n_lines == 4 && r.n2 == 2); }

    printf("\nWHT_CONNECTED_SELFTEST=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("FAILED_COUNT=%d\n", failed);
    printf("FULL_MODEL_LOAD_COUNT=0\nREAL_CONTEXT_CREATE_COUNT=0\nMODEL_FORWARD_CALL_COUNT=0\n");
    return failed == 0 ? 0 : 1;
}
