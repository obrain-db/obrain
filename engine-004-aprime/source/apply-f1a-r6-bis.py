#!/usr/bin/env python3
"""ENGINE-007B-F1A-Q0-R6 (reprise) — attribution de soumission, SHA256 reel.

SEQUENCE REELLE OBSERVEE (journal on.txt) :
    sub=1  -> end_epoch #1 { 168 residentes, 1176 bufferless }  -> 336 payloads
    sub=2  -> end_epoch #2 { 168 residentes,    0 bufferless }  -> 336 payloads

Mon etiquette precedente utilisait f1a_ctr_submission, lu a l extraction. Or ce compteur
est incremente a la soumission du pas SUIVANT, donc au moment de l extraction il vaut deja
1 de trop : les payloads de l epoque cloturee sous sub=1 etaient etiquetes sub=2.

C est exactement le « decalage d une soumission » que le paragraphe R5-d/D1 interdit.

CORRECTIF : figer, a la CREATION de l epoque de capture, le numero de la soumission a
laquelle cette epoque appartient. La valeur ne bouge plus ensuite.
    - si aucune soumission n a encore eu lieu      -> soumission 0 (prefill / preparation)
    - sinon                                        -> la derniere soumission effectuee
Chaque cible enregistree porte ce numero, et l extraction l utilise tel quel.
"""
import sys

C = "/home/triviere/projects/llama-obrain-cuda-f1a/src/f1a-capture.cpp"

# 1. champ dedie dans le contexte d epoque
A1 = """struct f1a_capture_ctx {
    unsigned long long epoch = 0;"""
B1 = """struct f1a_capture_ctx {
    unsigned long long epoch = 0;
    // R6 : soumission a laquelle appartient CETTE epoque, figee a la creation.
    unsigned long long owning_submission = 0;"""

# 2. le figer a la creation de l epoque
A2 = """f1a_capture_ctx * f1a_capture_get_ctx() {
    f1a_load_inventory();
    if (!g_f1a.enabled) { f1a_ctr_probe++; return nullptr; }
    f1a_ctr_ctx_active++;"""
B2 = """f1a_capture_ctx * f1a_capture_get_ctx() {
    f1a_load_inventory();
    if (!g_f1a.enabled) { f1a_ctr_probe++; return nullptr; }
    f1a_ctr_ctx_active++;
    if (f1a_capture_ctx * c = f1a_capture_current_ctx()) {
        // R6 : numero de la soumission EN COURS, fige pour toute la duree de l epoque.
        c->owning_submission = f1a_ctr_submission;
        return c;
    }"""

# 3. la cible porte l identifiant de sa soumission proprietaire
A3 = "    r.submission_id_at_build = f1a_ctr_submission;"
B3 = "    r.submission_id_at_build = cc->owning_submission;"

# 4. extraction : utiliser l identifiant de l epoque, plus le compteur global
A4 = """                    fprintf(pv, "%llu,%llu,%d,%d,%s,%s,%d,%lld,%lld,%d,%zu,%zu,%s,fnv64x4,%s\\n",
                        r.epoch, f1a_ctr_submission, r.target_id, r.activation_capture_id,"""
B4 = """                    fprintf(pv, "%llu,%llu,%d,%d,%s,%s,%d,%lld,%lld,%d,%zu,%zu,%s,fnv64x4,%s\\n",
                        r.epoch, r.submission_id_at_build, r.target_id, r.activation_capture_id,"""

A5 = """            snprintf(p, sizeof(p), "%s/ep%llu_sub%llu_t%d_c%d_%s.bin",
                     g_f1a.outdir.c_str(), r.epoch, f1a_ctr_submission,
                     r.target_id, r.activation_capture_id, kinds[k]);"""
B5 = """            snprintf(p, sizeof(p), "%s/ep%llu_sub%llu_t%d_c%d_%s.bin",
                     g_f1a.outdir.c_str(), r.epoch, r.submission_id_at_build,
                     r.target_id, r.activation_capture_id, kinds[k]);"""

# 5. un accesseur statique pour le contexte courant (declaration locale a la fonction)
A6 = """f1a_capture_ctx * f1a_capture_get_ctx() {"""
B6 = """// R6 : contexte courant, sans repasser par le controle d activation (evite la recursion).
static f1a_capture_ctx * f1a_capture_current_ctx() {
    static thread_local f1a_capture_ctx ctx;
    return &ctx;
}

f1a_capture_ctx * f1a_capture_get_ctx() {"""

# retirer l ancienne static locale devenue inutile
A7 = """    // Une epoque par execution de graphe : creee ici, finalisee par f1a_capture_end_epoch().
    static thread_local f1a_capture_ctx ctx;
    return &ctx;
}"""
B7 = """    // Une epoque par execution de graphe : creee ici, finalisee par f1a_capture_end_epoch().
    return f1a_capture_current_ctx();
}"""


def main():
    s = open(C, encoding="utf-8").read()
    for old, new, name in ((A1, B1, "champ owning_submission"),
                           (A2, B2, "figeage a la creation"),
                           (A3, B3, "cible porte sa soumission"),
                           (A4, B4, "provenance"),
                           (A5, B5, "nom de fichier"),
                           (A6, B6, "accesseur courant"),
                           (A7, B7, "retrait static locale")):
        n = s.count(old)
        if n != 1:
            raise SystemExit("[%s] ancre trouvee %d fois -- rien n'est ecrit" % (name, n))
        s = s.replace(old, new, 1)
    open(C, "w", encoding="utf-8").write(s)
    print("attribution de soumission corrigee")


if __name__ == "__main__":
    main()
