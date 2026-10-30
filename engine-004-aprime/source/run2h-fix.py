#!/usr/bin/env python3
"""RUN2H : patch correctif des 2 defauts smoke. Copie -> edition -> diff."""
import difflib, hashlib, os, shutil, sys

W = "/home/triviere/projects/llama-obrain-cuda-f1a"
TMP = "/tmp/q0fwd-fix"
D = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/e7b-f1a-q0-forward-v1/run2")
OUT = D + "/RUN2H_SMOKE_FIX_PATCH.diff"
FILES = ["src/f1a-capture.h", "src/f1a-capture.cpp"]

shutil.rmtree(TMP, ignore_errors=True)
for f in FILES:
    os.makedirs(os.path.join(TMP, os.path.dirname(f)), exist_ok=True)
    shutil.copy2(os.path.join(W, f), os.path.join(TMP, f))
J = "\n"

def edit(rel, old, new, label, count=1):
    p = os.path.join(TMP, rel); s = open(p, encoding="utf-8").read()
    old_s = J.join(old); new_s = J.join(new)
    n = s.count(old_s)
    if n != count:
        print("  ANCRE KO %-46s %d occ" % (label, n)); sys.exit(1)
    open(p, "w", encoding="utf-8").write(s.replace(old_s, new_s, count))
    print("  ok  " + label)

# ============ A1 + A2 : dossier pret, echec I/O visible ============
edit("src/f1a-capture.cpp",
 ["static unsigned long long g_f1a_event_seq  = 0;",
  "static FILE *             g_f1a_ledger     = nullptr;",
  "static bool               g_f1a_ledger_hdr = false;"],
 ["static unsigned long long g_f1a_event_seq  = 0;",
  "static FILE *             g_f1a_ledger     = nullptr;",
  "static bool               g_f1a_ledger_hdr = false;",
  "// RUN2H / A2 : un evenement perdu ne doit JAMAIS ressembler a un run valide.",
  "static unsigned long long g_f1a_event_io_fail = 0;",
  "",
  "unsigned long long f1a_event_io_failure_count(void) { return g_f1a_event_io_fail; }",
  "",
  "// RUN2H / A1 : le repertoire du ledger doit exister AVANT sa premiere ecriture.",
  "// Idempotent. Appele par f1a_event_row, donc SYNC_COMPLETE (emis tot dans",
  "// end_epoch, avant le mkdir historique) trouve toujours sa destination prete.",
  "static void f1a_ensure_outdir(void) {",
  "    if (g_f1a.outdir.empty()) return;",
  "    if (mkdir(g_f1a.outdir.c_str(), 0755) != 0) { /* existe deja : cas normal */ }",
  "}"],
 "A1/A2 : compteur d echec I/O + f1a_ensure_outdir")

edit("src/f1a-capture.cpp",
 ["    if (!g_f1a.enabled) return;",
  "    if (g_f1a_ledger == nullptr) {",
  '        g_f1a_ledger = fopen((g_f1a.outdir + "/" + F1A_EVENT_LEDGER_FILE).c_str(), "a");',
  "        if (g_f1a_ledger == nullptr) return;",
  "    }"],
 ["    if (!g_f1a.enabled) return;",
  "    f1a_ensure_outdir();   // RUN2H / A1 : destination prete AVANT toute ecriture",
  "    if (g_f1a_ledger == nullptr) {",
  '        g_f1a_ledger = fopen((g_f1a.outdir + "/" + F1A_EVENT_LEDGER_FILE).c_str(), "a");',
  "        if (g_f1a_ledger == nullptr) {",
  "            // RUN2H / A2 : echec RENDU OBSERVABLE. Plus de perte silencieuse.",
  "            g_f1a_event_io_fail++;",
  '            fprintf(stderr, "[f1a] EVENT_LEDGER_OPEN_FAILURE kind=%s count=%llu dir=%s\\n",',
  "                    kind, g_f1a_event_io_fail, g_f1a.outdir.c_str());",
  "            return;",
  "        }",
  "    }"],
 "A2 : echec d ouverture observable")

# ============ B : epoch applicable distincte de la valeur 0 ============
edit("src/f1a-capture.cpp",
 ["static void f1a_event_row(const char * kind,",
  "                          int32_t              decode_step,",
  "                          unsigned long long   compute_submission_id,",
  "                          unsigned long long   capture_epoch_id,",
  "                          int                  eligible_target_count,",
  "                          int                  captured_target_count) {"],
 ["// RUN2H / B : capture_epoch_applicable separe l APPLICABILITE de la VALEUR.",
  "// L epoque 0 est une valeur legitime : elle ne doit jamais devenir NA.",
  "static void f1a_event_row(const char * kind,",
  "                          int32_t              decode_step,",
  "                          unsigned long long   compute_submission_id,",
  "                          unsigned long long   capture_epoch_id,",
  "                          int                  capture_epoch_applicable,",
  "                          int                  eligible_target_count,",
  "                          int                  captured_target_count) {"],
 "B : signature + capture_epoch_applicable")

edit("src/f1a-capture.cpp",
 ["    char epbuf[32];",
  '    if (capture_epoch_id == 0) snprintf(epbuf, sizeof epbuf, "NA");',
  '    else                       snprintf(epbuf, sizeof epbuf, "%llu", capture_epoch_id);'],
 ["    char epbuf[32];",
  "    // RUN2H / B : NA seulement si l epoque n est PAS APPLICABLE, jamais parce",
  "    // que sa valeur vaut 0. epoch 0 -> \"0\".",
  "    if (!capture_epoch_applicable) snprintf(epbuf, sizeof epbuf, \"NA\");",
  '    else                           snprintf(epbuf, sizeof epbuf, "%llu", capture_epoch_id);'],
 "B : politique de serialisation de l epoque")

edit("src/f1a-capture.cpp",
 ['    fprintf(g_f1a_ledger, "%s,%llu,%s,%s,%s,%s,%016llx,%s,%d,%d\\n",',
  "            g_f1a.run_id.c_str(), ++g_f1a_event_seq, kind, dsbuf, sidbuf,",
  "            gpbuf, g_f1a_last_sub_fp, epbuf,",
  "            (eligible_target_count < 0) ? 0 : eligible_target_count,",
  "            (captured_target_count < 0) ? 0 : captured_target_count);",
  "    fflush(g_f1a_ledger);"],
 ['    if (fprintf(g_f1a_ledger, "%s,%llu,%s,%s,%s,%s,%016llx,%s,%d,%d\\n",',
  "            g_f1a.run_id.c_str(), ++g_f1a_event_seq, kind, dsbuf, sidbuf,",
  "            gpbuf, g_f1a_last_sub_fp, epbuf,",
  "            (eligible_target_count < 0) ? 0 : eligible_target_count,",
  "            (captured_target_count < 0) ? 0 : captured_target_count) < 0) {",
  "        // RUN2H / A2 : echec d ecriture egalement observable.",
  "        g_f1a_event_io_fail++;",
  '        fprintf(stderr, "[f1a] EVENT_LEDGER_WRITE_FAILURE kind=%s count=%llu\\n",',
  "                kind, g_f1a_event_io_fail);",
  "        return;",
  "    }",
  "    if (fflush(g_f1a_ledger) != 0) {",
  "        g_f1a_event_io_fail++;",
  '        fprintf(stderr, "[f1a] EVENT_LEDGER_FLUSH_FAILURE kind=%s count=%llu\\n",',
  "                kind, g_f1a_event_io_fail);",
  "    }"],
 "A2 : echec d ecriture et de flush observables")

# appelants : current = epoch non applicable ; latched = applicable
edit("src/f1a-capture.cpp",
 ["void f1a_event_current(const char * kind) {",
  "    f1a_event_row(kind, g_f1a_decode_step, g_f1a_last_sub_id, 0, -1, -1);",
  "}"],
 ["void f1a_event_current(const char * kind) {",
  "    // RUN2H / B : pas d epoque dans le contexte courant -> NON APPLICABLE.",
  "    f1a_event_row(kind, g_f1a_decode_step, g_f1a_last_sub_id, 0, 0, -1, -1);",
  "}"],
 "B : f1a_event_current -> epoch non applicable")

edit("src/f1a-capture.cpp",
 ["void f1a_event_latched(const char * kind,",
  "                       int32_t decode_step,",
  "                       unsigned long long compute_submission_id,",
  "                       unsigned long long capture_epoch_id,",
  "                       int eligible_target_count,",
  "                       int captured_target_count) {",
  "    f1a_event_row(kind, decode_step, compute_submission_id, capture_epoch_id,",
  "                  eligible_target_count, captured_target_count);",
  "}"],
 ["void f1a_event_latched(const char * kind,",
  "                       int32_t decode_step,",
  "                       unsigned long long compute_submission_id,",
  "                       unsigned long long capture_epoch_id,",
  "                       int eligible_target_count,",
  "                       int captured_target_count) {",
  "    // RUN2H / B : l epoque est APPLICABLE ici, y compris quand elle vaut 0.",
  "    f1a_event_row(kind, decode_step, compute_submission_id, capture_epoch_id,",
  "                  1, eligible_target_count, captured_target_count);",
  "}"],
 "B : f1a_event_latched -> epoch applicable")

edit("src/f1a-capture.cpp",
 ['    f1a_event_row("SYNC_COMPLETE", g_f1a_decode_step, f1a_last_submission_id(),',
  "                  (unsigned long long) cc->epoch, -1, -1);"],
 ['    // RUN2H / A1 : f1a_event_row garantit desormais le dossier lui-meme',
  "    // (f1a_ensure_outdir), donc cet evenement n est plus perdu sur l epoque 0.",
  '    f1a_event_row("SYNC_COMPLETE", g_f1a_decode_step, f1a_last_submission_id(),',
  "                  (unsigned long long) cc->epoch, 1, -1, -1);"],
 "A1 : SYNC_COMPLETE conserve sa semantique, destination garantie")

edit("src/f1a-capture.cpp",
 ['    f1a_event_row("COMPUTE_SUBMIT", g_f1a_decode_step, g_f1a_last_sub_id, 0, -1, -1);'],
 ['    f1a_event_row("COMPUTE_SUBMIT", g_f1a_decode_step, g_f1a_last_sub_id, 0, 0, -1, -1);'],
 "COMPUTE_SUBMIT : epoch non applicable")

# header
edit("src/f1a-capture.h",
 ["void f1a_event_close(void);"],
 ["void f1a_event_close(void);",
  "// RUN2H / A2 : nombre d echecs d E/S du ledger. Un evenement perdu doit etre",
  "// observable, jamais silencieux.",
  "unsigned long long f1a_event_io_failure_count(void);"],
 "header : f1a_event_io_failure_count")

# ---------------- diff ----------------
lines = []
for f in FILES:
    a = open(os.path.join(W, f), encoding="utf-8").read().splitlines(keepends=True)
    b = open(os.path.join(TMP, f), encoding="utf-8").read().splitlines(keepends=True)
    d = list(difflib.unified_diff(a, b, fromfile="a/" + f, tofile="b/" + f, n=3))
    if d:
        lines.extend(d)
        print("  diff %-24s %d lignes" % (f, len(d)))
txt = "".join(lines)
open(OUT, "w", encoding="utf-8", newline="\n").write(txt)
h = hashlib.sha256(txt.encode()).hexdigest()
open(D + "/RUN2H_SMOKE_FIX_PATCH.sha256", "w").write(h + "  RUN2H_SMOKE_FIX_PATCH.diff\n")
add = sum(1 for l in lines if l.startswith("+") and not l.startswith("+++"))
rem = sum(1 for l in lines if l.startswith("-") and not l.startswith("---"))
hunks = sum(1 for l in lines if l.startswith("@@"))
print()
print("RUN2H_SMOKE_FIX_PATCH_SHA256=" + h)
print("FILES=%d HUNKS=%d ADDED=%d REMOVED=%d" % (len(FILES), hunks, add, rem))
