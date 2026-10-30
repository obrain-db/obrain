// ENGINE-008 -- WHT-PIPELINE-PROOF-13 : FICHIERS -> lecteur reel -> collecteur
// WHT -> agregation -> serialisation. Les dependances de CALCUL sont mockees ;
// les dependances de LECTURE sont les vraies.
#include "engine008_pipeline.h"
#include "engine008_real_deps.h"
#include <cstdio>
#include <fstream>
#include <sys/stat.h>

static int failed = 0;
static void ck(const char * n, bool ok) {
    printf("  %-64s %s\n", n, ok ? "PASS" : "FAIL");
    if (!ok) failed++;
}
static void wf(const std::string & p, const std::string & c) { std::ofstream f(p); f << c; }

int main() {
    printf("ENGINE-008 -- WHT-PIPELINE-PROOF-13 : fichiers -> pipeline\n\n");
    const std::string D = "/tmp/wp13/cap";
    system(("rm -rf /tmp/wp13 && mkdir -p " + D + "/capon").c_str());

    // --- surfaces SYNTHETIQUES ecrites selon les schemas du producteur
    const std::string MH =
      "run_id,epoch,target_id,capture_id,weight_name,family,expected_K,rotation_applied,"
      "kind,path,dtype,ne0,ne1,ne2,ne3,nb0,nb1,nb2,nb3,logical_elems,bytes,contiguous,"
      "decode_step,compute_submission_id\n";
    const std::string RH =
      "epoch,target_id,weight_name,sub_at_build,cur_op,cur_src0_is_effective,mulmat_op,"
      "mulmat_src0_is_weight,mulmat_src1_is_cur,mmcap_op,mmcap_src0_is_mmraw,downstream_reach,"
      "role_ok,effect_class\n";
    const std::string EH =
      "epoch,target_id,weight_name,role,effect_class,src0_is_weight,src1_is_cur,dst_is_cur,effect_ok\n";
    const std::string SH =
      "target_id,capture_id,role,tensor_ptr,tensor_name,op,type,ne0,nb0,nbytes,contiguous,"
      "buffer_ptr,resident,in_submitted_graph,weight_name\n";
    auto mrow = [](int tid,int cid,const char* k,int rot){
        char b[512]; snprintf(b,sizeof b,"Q1,0,%d,%d,w,5,5120,%d,%s,/x.bin,0,5120,1,1,1,4,20480,20480,20480,5120,20480,1,0,1\n",tid,cid,rot,k); return std::string(b); };
    wf(D+"/capon/GRAPH_CAPTURE_MANIFEST.csv", MH + mrow(1,1,"activation",1) + mrow(1,1,"base_output",1));
    wf(D+"/capon/GRAPH_ROLE_RELATIONSHIPS.csv", RH + "0,1,w,1,1,1,29,1,1,1,1,1,1,OUTPUT_DEPENDENT\n");
    wf(D+"/capon/TARGET_EFFECT_PATH.csv", EH + "0,1,w,MM_CAPTURE,OUTPUT_DEPENDENT,1,1,1,1\n");
    wf(D+"/capon/RESIDENCY_MANIFEST.csv", SH + "1,1,CUR_CAPTURE,0x1,n,1,0,5120,4,20480,1,0x2,1,1,w\n");

    printf("=== LECTEUR REEL + COLLECTEUR WHT ===\n");
    { RealState st; st.capture_dir = D;
      PipelineDeps probe;                       // on isole le collecteur reel
      // on construit les deps reelles puis on n'utilise que wht_collect
      PipelineDeps rd = make_real_deps(st);
      ck("real_wht_collector_present", (bool) rd.wht_collect);
      WhtAssessment a = rd.wht_collect();
      ck("real_reader_finds_surfaces_in_capon_subdir",
         a.coverage.manifest_present && a.coverage.roles_present &&
         a.coverage.effect_present && a.coverage.residency_present);
      ck("real_reader_reports_expected_targets", a.coverage.n_targets_expected == 1);
      ck("real_reader_separates_line_and_target_counts",
         a.coverage.n_rotation_true == 2 && a.coverage.n_targets_expected == 1);
      ck("real_reader_reaches_submitted_graph_scope",
         a.verdict == WhtVerdict::EXECUTION_NOT_ESTABLISHED &&
         a.coverage.n_in_submitted_graph > 0 &&
         a.coverage.n_role_ok > 0 && a.coverage.n_effect_ok > 0);
      ck("execution_scope_still_explicitly_limited",
         a.scope.find("EXECUTION NON ETABLIE") != std::string::npos); }

    printf("=== ABSENCE DE SURFACE -> VERDICT EXPLICITE ===\n");
    { system("rm -rf /tmp/wp13/empty && mkdir -p /tmp/wp13/empty");
      RealState st2; st2.capture_dir = "/tmp/wp13/empty";
      PipelineDeps rd2 = make_real_deps(st2);
      WhtAssessment a2 = rd2.wht_collect();
      ck("missing_surfaces_yield_explicit_verdict",
         a2.verdict == WhtVerdict::SURFACE_MISSING_OR_MALFORMED && !a2.blockers.empty()); }

    printf("\nWHT_FILE_TO_PIPELINE_SELFTEST=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("FAILED_COUNT=%d\n", failed);
    printf("FULL_MODEL_LOAD_COUNT=0\nREAL_CONTEXT_CREATE_COUNT=0\nMODEL_FORWARD_CALL_COUNT=0\n");
    return failed == 0 ? 0 : 1;
}
