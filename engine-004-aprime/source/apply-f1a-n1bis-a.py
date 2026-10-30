#!/usr/bin/env python3
"""ENGINE-007B-F1A-Q0-N1bis — A : corriger HARNESS_DEFECT_52.

DEF AUT : chaque epoque rouvre GRAPH_CAPTURE_MANIFEST.csv en "w" et ecrase la precedente.
672 payloads sur disque, un manifeste qui ne decrit que la derniere epoque.

CORRECTIF (contrat du paragraphe 1) :
    DEBUT D EXECUTION  creer ou tronquer, ecrire UN SEUL en-tete
    CHAQUE EPOQUE      ajouter les lignes, sans effacer, sans repeter l en-tete
    FIN D EXECUTION    fermer et verifier

Un simple mode "a" ne suffit pas : il melangerait les executions successives. On ajoute
donc un drapeau statique qui declenche la troncature UNE fois par execution.
"""
import sys

C = "/home/triviere/projects/llama-obrain-cuda-f1a/src/f1a-capture.cpp"

OLD = '''    const std::string mpath = g_f1a.outdir + "/GRAPH_CAPTURE_MANIFEST.csv";
    FILE * m = fopen(mpath.c_str(), "w");
    if (m == nullptr) {
        fprintf(stderr, "[f1a] manifeste non ouvrable : %s\\n", mpath.c_str());
        cc->targets.clear(); cc->act_key_to_id.clear(); cc->epoch++;
        return;
    }
    fprintf(m, "epoch,target_id,capture_id,weight_name,family,expected_K,rotation_applied,"
               "kind,path,dtype,ne0,ne1,ne2,ne3,nb0,nb1,nb2,nb3,logical_elems,bytes,contiguous\\n");'''

NEW = '''    const std::string mpath = g_f1a.outdir + "/GRAPH_CAPTURE_MANIFEST.csv";
    // N1bis : HARNESS_DEFECT_52. La premiere epoque TRONQUE et ecrit l en-tete ;
    // les suivantes AJOUTENT, sans repeter l en-tete et sans effacer.
    static bool s_manifest_header_done = false;
    FILE * m = fopen(mpath.c_str(), s_manifest_header_done ? "a" : "w");
    if (m == nullptr) {
        fprintf(stderr, "[f1a] manifeste non ouvrable : %s\\n", mpath.c_str());
        cc->targets.clear(); cc->act_key_to_id.clear(); cc->epoch++;
        return;
    }
    if (!s_manifest_header_done) {
        fprintf(m, "run_id,epoch,target_id,capture_id,weight_name,family,expected_K,rotation_applied,"
                   "kind,path,dtype,ne0,ne1,ne2,ne3,nb0,nb1,nb2,nb3,logical_elems,bytes,contiguous\\n");
        s_manifest_header_done = true;
    }'''

OLD_ROW = '''            fprintf(m, "%llu,%d,%d,%s,%d,%lld,%d,%s,%s,%d,%lld,%lld,%lld,%lld,%lld,%lld,%lld,%lld,%lld,%lld,%d\\n",
                    r.epoch, r.target_id, r.activation_capture_id, r.weight_name.c_str(), r.family,'''
NEW_ROW = '''            fprintf(m, "%s,%llu,%d,%d,%s,%d,%lld,%d,%s,%s,%d,%lld,%lld,%lld,%lld,%lld,%lld,%lld,%lld,%lld,%lld,%d\\n",
                    g_f1a.run_id.c_str(), r.epoch, r.target_id, r.activation_capture_id, r.weight_name.c_str(), r.family,'''

# run_id : identifie l execution, pour qu un mode "a" ne melange jamais deux runs
OLD_STATE = '''struct f1a_state {
    bool inited   = false;'''
NEW_STATE = '''struct f1a_state {
    bool inited   = false;
    std::string run_id;      // N1bis : identifie l execution, jamais melangee a une autre'''

OLD_INIT = '''    g_f1a.outdir         = out ? out : "/tmp/f1a-out";'''
NEW_INIT = '''    g_f1a.outdir         = out ? out : "/tmp/f1a-out";
    {   // N1bis : identifiant d execution, pose une seule fois par processus
        const char * rid = getenv("OBRAIN_F1A_RUN_ID");
        g_f1a.run_id = rid ? rid : "run0";
    }'''


def main():
    s = open(C, encoding="utf-8").read()
    for old, new, name in ((OLD, NEW, "ouverture du manifeste"),
                           (OLD_ROW, NEW_ROW, "ligne du manifeste"),
                           (OLD_STATE, NEW_STATE, "champ run_id"),
                           (OLD_INIT, NEW_INIT, "initialisation run_id")):
        n = s.count(old)
        if n != 1:
            raise SystemExit("[%s] ancre trouvee %d fois -- rien n'est ecrit" % (name, n))
        s = s.replace(old, new, 1)
    open(C, "w", encoding="utf-8").write(s)
    print("HARNESS_DEFECT_52 : manifeste corrige (troncature une fois + ajout ensuite + run_id)")


if __name__ == "__main__":
    main()
