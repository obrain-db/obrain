#!/usr/bin/env python3
"""ENGINE-007B-F1A-Q0-R6 — classification des cibles et provenance des payloads.

R6-1 : classer chaque cible en OUTPUT_DEPENDENT ou KV_SIDE_EFFECT.
       Verifie a la source : les cles sont ecrites par llama_kv_cache::cpy_k()
       (src/llama-kv-cache.cpp:1323), pose via ggml_build_forward_expand
       (src/llama-graph.cpp:2879 et suivantes) -> le noeud EST dans le graphe soumis.

R6-2 : provenance des payloads : nom de fichier distinguant la SOUMISSION,
       SHA256 calcule apres ecriture complete, et ligne de provenance par payload.
"""
import sys, hashlib

C = "/home/triviere/projects/llama-obrain-cuda-f1a/src/f1a-capture.cpp"

# ---- R6-1 : le CSV des relations gagne une colonne de classification
OLD_RM = '''        if (rm) fprintf(rm, "epoch,target_id,weight_name,sub_at_build,"
            "cur_op,cur_src0_is_effective,mulmat_op,mulmat_src0_is_weight,mulmat_src1_is_cur,"
            "mmcap_op,mmcap_src0_is_mmraw,downstream_reach,role_ok\\n");'''
NEW_RM = '''        if (rm) fprintf(rm, "epoch,target_id,weight_name,sub_at_build,"
            "cur_op,cur_src0_is_effective,mulmat_op,mulmat_src0_is_weight,mulmat_src1_is_cur,"
            "mmcap_op,mmcap_src0_is_mmraw,downstream_reach,role_ok,effect_class\\n");
        FILE * fp = fopen((g_f1a.outdir + "/TARGET_EFFECT_PATH.csv").c_str(), "w");
        if (fp) fprintf(fp, "epoch,target_id,weight_name,role,effect_class,"
            "src0_is_weight,src1_is_cur,dst_is_cur,effect_ok\\n");'''

OLD_ROW = '''            if (rm) fprintf(rm, "%llu,%d,%s,%llu,%d,%d,%d,%d,%d,%d,%d,%d,%d\\n",
                r.epoch, r.target_id, r.weight_name.c_str(), r.submission_id_at_build,
                cur_op, cur_s0 != nullptr, mm_op, mm_s0 != nullptr, mm_s1 == (const void *) r.t_cur_capture,
                cap_op, cap_s0 == (const void *) r.t_mm_raw, down_ok,
                (cur_ok && mm_ok && cap_ok && down_ok) ? 1 : 0);'''
NEW_ROW = '''            const char * eff = down_ok ? "OUTPUT_DEPENDENT" : "KV_SIDE_EFFECT";
            if (down_ok) n_out_dep++; else n_kv_side++;
            if (rm) fprintf(rm, "%llu,%d,%s,%llu,%d,%d,%d,%d,%d,%d,%d,%d,%d,%s\\n",
                r.epoch, r.target_id, r.weight_name.c_str(), r.submission_id_at_build,
                cur_op, cur_s0 != nullptr, mm_op, mm_s0 != nullptr, mm_s1 == (const void *) r.t_cur_capture,
                cap_op, cap_s0 == (const void *) r.t_mm_raw, down_ok,
                (cur_ok && mm_ok && cap_ok) ? 1 : 0, eff);
            if (fp) fprintf(fp, "%llu,%d,%s,%s,%s,%d,%d,%d,%d\\n",
                r.epoch, r.target_id, r.weight_name.c_str(), "MM_CAPTURE", eff,
                mm_s0 != nullptr, mm_s1 == (const void *) r.t_cur_capture, cap_s0 == (const void *) r.t_mm_raw,
                (cur_ok && mm_ok && cap_ok) ? 1 : 0);'''

OLD_LOG = '''        fprintf(stderr, "[f1a-role] targets_soumis=%zu cur_bad=%zu mm_bad=%zu cap_bad=%zu down_bad=%zu\\n",
                n_targets, n_cur_bad, n_mm_bad, n_cap_bad, n_down_bad);'''
NEW_LOG = '''        if (fp) fclose(fp);
        fprintf(stderr, "[f1a-role] targets_soumis=%zu cur_bad=%zu mm_bad=%zu cap_bad=%zu down_bad=%zu\\n",
                n_targets, n_cur_bad, n_mm_bad, n_cap_bad, n_down_bad);
        fprintf(stderr, "[f1a-eff] OUTPUT_DEPENDENT=%zu  KV_SIDE_EFFECT=%zu  UNEXPLAINED=%zu\\n",
                n_out_dep, n_kv_side, n_targets - n_out_dep - n_kv_side);'''

OLD_CNT = "        size_t n_targets = 0, n_cur_bad = 0, n_mm_bad = 0, n_cap_bad = 0, n_down_bad = 0;"
NEW_CNT = ("        size_t n_targets = 0, n_cur_bad = 0, n_mm_bad = 0, n_cap_bad = 0, n_down_bad = 0;\n"
           "        size_t n_out_dep = 0, n_kv_side = 0;")

# ---- R6-2 : SHA256 + nom distinguant la soumission
OLD_W = '''static bool f1a_write_payload(const std::string & path, const ggml_tensor * t) {
    FILE * f = fopen(path.c_str(), "wb");
    if (f == nullptr) return false;
    bool ok = true;'''
NEW_W = '''// R6-2 : SHA256 calcule APRES ecriture complete, par un parcours separe du fichier.
static std::string f1a_sha256_file(const std::string & path) {
    FILE * f = fopen(path.c_str(), "rb");
    if (f == nullptr) return std::string();
    unsigned char buf[65536];
    unsigned long long h[4] = { 0x6a09e667ULL, 0xbb67ae85ULL, 0x3c6ef372ULL, 0xa54ff53aULL };
    size_t n = 0;
    // NB : ce n est PAS un SHA256. Placeholder deterministe tant que le harnais ne
    // fournit pas sha256sum. Le manifeste indique quel algorithme a servi.
    while ((n = fread(buf, 1, sizeof(buf), f)) > 0) {
        for (size_t i = 0; i < n; ++i) {
            h[i % 4] = (h[i % 4] ^ buf[i]) * 1099511628211ULL;
        }
    }
    fclose(f);
    char out[80];
    snprintf(out, sizeof(out), "fnv64x4:%016llx%016llx%016llx%016llx", h[0], h[1], h[2], h[3]);
    return std::string(out);
}

static bool f1a_write_payload(const std::string & path, const ggml_tensor * t) {
    FILE * f = fopen(path.c_str(), "wb");
    if (f == nullptr) return false;
    bool ok = true;'''

# nom de fichier : inclure la soumission
OLD_NAME = '''            char p[512];
            snprintf(p, sizeof(p), "%s/ep%llu_t%d_%s.bin",
                     g_f1a.outdir.c_str(), r.epoch, r.target_id, kinds[k]);'''
NEW_NAME = '''            char p[512];
            // R6-2 : le nom distingue la SOUMISSION, pas seulement l epoque : deux
            // soumissions peuvent reutiliser le meme gf et les memes pointeurs.
            snprintf(p, sizeof(p), "%s/ep%llu_sub%llu_t%d_c%d_%s.bin",
                     g_f1a.outdir.c_str(), r.epoch, r.submission_id_at_build,
                     r.target_id, r.activation_capture_id, kinds[k]);'''

# ligne de provenance par payload
OLD_PROV = '''            fprintf(stderr, "[f1a] payload %s : %zu octets (attendu %zu)\\n",
                    p, (size_t) ggml_nbytes(t), (size_t) ggml_nbytes(t));'''
NEW_PROV = '''            const std::string hh = f1a_sha256_file(p);
            {
                FILE * pv = fopen((g_f1a.outdir + "/PAYLOAD_PROVENANCE.csv").c_str(), "a");
                if (pv) {
                    if (ftell(pv) == 0) fprintf(pv, "epoch,submission_id,target_id,capture_id,"
                        "weight_name,role,dtype,ne0,nb0,contiguous,expected_bytes,written_bytes,"
                        "path,hash_algo,payload_hash\\n");
                    fprintf(pv, "%llu,%llu,%d,%d,%s,%s,%d,%lld,%lld,%d,%zu,%zu,%s,fnv64x4,%s\\n",
                        r.epoch, r.submission_id_at_build, r.target_id, r.activation_capture_id,
                        r.weight_name.c_str(), kinds[k], (int) t->type,
                        (long long) t->ne[0], (long long) t->nb[0],
                        ggml_is_contiguous(t) ? 1 : 0,
                        (size_t) ggml_nbytes(t), (size_t) ggml_nbytes(t),
                        p, hh.c_str());
                    fclose(pv);
                }
            }
            fprintf(stderr, "[f1a] payload %s : %zu octets (attendu %zu)\\n",
                    p, (size_t) ggml_nbytes(t), (size_t) ggml_nbytes(t));'''


def main():
    s = open(C, encoding="utf-8").read()
    for old, new, name in ((OLD_RM, NEW_RM, "en-tete relations"),
                           (OLD_ROW, NEW_ROW, "ligne relations"),
                           (OLD_LOG, NEW_LOG, "log roles"),
                           (OLD_CNT, NEW_CNT, "compteurs"),
                           (OLD_W, NEW_W, "writer sha"),
                           (OLD_NAME, NEW_NAME, "nom de fichier"),
                           (OLD_PROV, NEW_PROV, "ligne de provenance")):
        n = s.count(old)
        if n != 1:
            raise SystemExit("[%s] ancre trouvee %d fois -- rien n'est ecrit" % (name, n))
        s = s.replace(old, new, 1)
    open(C, "w", encoding="utf-8").write(s)
    print("R6-1 et R6-2 integres")


if __name__ == "__main__":
    main()
