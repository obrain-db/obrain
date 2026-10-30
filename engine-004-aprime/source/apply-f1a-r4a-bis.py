#!/usr/bin/env python3
"""ENGINE-007B-F1A-Q0-R4 — R4-a bis : corriger la semantique de F1A_CAPTURE_CONTEXT_ACTIVE_COUNT.

Constat a l'execution, capture desactivee :
    ctx_active=4477  eligible_enter=0  note=0  extract_call=0  payload_write=0  dup_added=0

Les cinq compteurs d inertie sont bien a zero, mais ctx_active valait 4477 alors qu AUCUN
contexte de capture n a ete cree. Defaut de MON compteur : il etait incremente dans les DEUX
branches de f1a_capture_get_ctx(), donc il comptait les VERIFICATIONS, pas les contextes.

C est exactement le travers que le paragraphe 0 de R4 interdit : un compteur qui ne mesure
pas ce que son nom annonce.

Correctif : separer les deux notions.
    F1A_CAPTURE_CONTEXT_ACTIVE_COUNT  = contextes de capture REELLEMENT actifs
    F1A_CAPTURE_PROBE_COUNT           = nombre de verifications du drapeau (diagnostic)
"""
import sys

C = "/home/triviere/projects/llama-obrain-cuda-f1a/src/f1a-capture.cpp"

OLD = """    if (!g_f1a.enabled) { f1a_ctr_ctx_active++; return nullptr; }
    f1a_ctr_ctx_active++;"""
NEW = """    if (!g_f1a.enabled) { f1a_ctr_probe++; return nullptr; }
    f1a_ctr_ctx_active++;      // R4-a : compte les contextes REELLEMENT actifs"""

DECL_OLD = "unsigned long long f1a_ctr_ctx_active = 0;"
DECL_NEW = ("unsigned long long f1a_ctr_ctx_active = 0;\n"
            "unsigned long long f1a_ctr_probe = 0;   // R4-a : verifications du drapeau")

PRINT_OLD = ('    fprintf(stderr, "[f1a-ctr%s] ctx_active=%llu eligible_enter=%llu note=%llu extract_call=%llu payload_write=%llu dup_added=%llu\\n",\n'
             '            tag, f1a_ctr_ctx_active, f1a_ctr_eligible_enter, f1a_ctr_note,\n'
             '            f1a_ctr_extract_call, f1a_ctr_payload_write, f1a_ctr_dup_added);')
PRINT_NEW = ('    fprintf(stderr, "[f1a-ctr%s] ctx_active=%llu probe=%llu eligible_enter=%llu note=%llu extract_call=%llu payload_write=%llu dup_added=%llu\\n",\n'
             '            tag, f1a_ctr_ctx_active, f1a_ctr_probe, f1a_ctr_eligible_enter, f1a_ctr_note,\n'
             '            f1a_ctr_extract_call, f1a_ctr_payload_write, f1a_ctr_dup_added);')


def main():
    s = open(C, encoding="utf-8").read()
    for old, new, name in ((OLD, NEW, "branche du drapeau"),
                           (DECL_OLD, DECL_NEW, "declaration"),
                           (PRINT_OLD, PRINT_NEW, "impression")):
        n = s.count(old)
        if n != 1:
            raise SystemExit("[%s] ancre trouvee %d fois, attendu 1 -- rien n'est ecrit" % (name, n))
        s = s.replace(old, new, 1)
    open(C, "w", encoding="utf-8").write(s)
    print("semantique de ctx_active corrigee ; probe ajoute")


if __name__ == "__main__":
    main()
