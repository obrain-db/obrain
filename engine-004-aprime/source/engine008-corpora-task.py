#!/usr/bin/env python3
"""ENGINE-008-DATA : regles de selection + materialisation des corpus TASK.
Aucune inference. Parsing, canonicalisation, hachage, ecriture uniquement."""
import csv
import glob
import hashlib
import json
import os

Q = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/engine-008-quality-v1")
E = Q + "/external"
C = Q + "/corpora"

def w(rel, text):
    p = os.path.join(Q, rel)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    return p

def canon_json(obj):
    """JSON canonique : cles triees, separateurs compacts, ensure_ascii=false."""
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False)

def sha(s):
    if isinstance(s, str):
        s = s.encode("utf-8")
    return hashlib.sha256(s).hexdigest()

def canon_text(t):
    """Normalisation texte generale : CRLF/CR -> LF. Pas de trim."""
    return t.replace("\r\n", "\n").replace("\r", "\n")

# ============================================================ A/B. REGLES
RULES = """ENGINE008_SELECTION_RULES_v1 -- GELÉES AVANT TOUTE SÉLECTION

=== CANONICALISATION TEXTE ===
encoding=UTF-8
newline_normalization=CRLF_AND_CR_TO_LF
unicode_normalization=NONE
trailing_whitespace=PRESERVE
final_newline_policy=NO_FORCED_FINAL_NEWLINE
code_indentation=PRESERVED
code_blank_lines=PRESERVED
semantic_array_order=PRESERVED (jamais trie)

=== CANONICALISATION JSON (pour hachage) ===
utf8, cles triees lexicographiquement, separateurs compacts ",",":",
arrays preservent l ordre upstream, ensure_ascii=false

=== MMLU-PRO ===
population      : split test, 12032 items, 14 categories
selection_object: {question_id, question, options, answer, answer_index, category, src}
                  cot_content EXCLU (jamais fourni au modele, jamais utilise par le scorer)
selection_hash  : SHA256(canonical JSON object)
regle           : pour CHAQUE categorie, tri ascendant par selection_hash, prendre 100
cible           : 1400 = 14 x 100

=== GSM8K ===
population      : grade_school_math/data/test.jsonl, 1319 items
selection_object: {question, answer}
selection_hash  : SHA256(canonical JSON object)
regle           : tri ascendant, prendre 500

=== MBPP ===
source          : sanitized-mbpp.json au commit pinne
population      : task_id dans [11,510], AUCUNE selection par hash
tri             : task_id ascendant
cible           : le compte REEL (257), jamais force
mbpp.jsonl      : provenance/cross-check uniquement, ne participe pas au score v1

=== IFEVAL ===
population      : toutes les lignes valides apres audit objectif prefreeze
exclusion       : uniquement defaut objectif (id inconnu, constructeur en echec,
                  kwargs invalide, item illisible). JAMAIS "semble difficile".
tri             : ordre upstream preserve

=== CORPUS TOKEN ===
wikitext  : 32768 tokens scores, items non-vides, tri par content_sha256 ascendant
obrain_docs: 16384 tokens scores, docs/*.md, tri par content_sha256 ascendant
obrain_rust: 16384 tokens scores, crates/**/*.rs, tri par content_sha256 ascendant
total     : 65536 tokens SCORES exactement

=== REGLE FRONTIERE DU DERNIER ITEM ===
parcourir dans l ordre hash gele ; ajouter les items COMPLETS tant que possible ;
si le dernier depasse le besoin exact : tokeniser cet item, prendre le PREFIXE
exact requis, le materialiser comme terminal_fragment ; NE PAS passer au suivant.

=== BOS / EOS ===
BOS_POLICY=NOT_ADDED
EOS_POLICY=NOT_ADDED
  Le prompt est rendu puis tokenise tel quel. Aucune politique de bibliotheque
  ne doit differer entre baseline et candidat.

=== SCORED vs CONTEXT ===
Un token servant seulement d amorce au premier target NE COMPTE PAS dans les
65536 scores. Le manifest distingue scored_start_index / scored_end_index.

=== DEDUPLICATION ===
exact canonical content duplicates uniquement. Pas de deduplication floue.

=== IDENTITES GLOBALES ===
mmlu_pro:<question_id>   gsm8k:<stable_hash>   mbpp:<task_id>   ifeval:<key>
"""

p = w("ENGINE008_SELECTION_RULES_v1.txt", RULES)
RULES_SHA = sha(open(p, "rb").read())
w("ENGINE008_SELECTION_RULES_v1.sha256", RULES_SHA + "  ENGINE008_SELECTION_RULES_v1.txt\n")
print("ENGINE008_SELECTION_RULES_SHA256=" + RULES_SHA)

out = ["ENGINE008_SELECTION_RULES_SHA256=" + RULES_SHA, ""]
def P(s=""):
    out.append(s); print(s)

# ============================================================ C/D. IFEVAL AUDIT
P("=== C/D. IFEVAL : AUDIT DE COHERENCE PREFREEZE ===")
import sys
sys.path.insert(0, E + "/mbpp/instruction_following_eval")
ifeval_rows = []
excl = []
reg = None
try:
    import instruction_following_eval.instructions_registry as _r  # noqa
    reg = _r
except Exception:
    try:
        import instructions_registry as _r
        reg = _r
    except Exception as e:
        P("  registry import KO : %s" % str(e)[:120])

inp = E + "/mbpp/instruction_following_eval/data/input_data.jsonl"
raw = [json.loads(l) for l in open(inp) if l.strip()]
P("IFEVAL_RAW_COUNT=%d" % len(raw))
for i, r in enumerate(raw):
    key = r.get("key", "row%d" % i)
    ids = r.get("instruction_id_list") or []
    kw = r.get("kwargs") or []
    parse_ok = bool(r.get("prompt")) and isinstance(ids, list)
    resolved = 0; inst_ok = 0; sch_ok = 0; miss = 0; extra = 0
    reasons = []
    for j, iid in enumerate(ids):
        k = kw[j] if j < len(kw) else {}
        cls = None
        if reg is not None:
            try:
                cls = reg.INSTRUCTION_DICT.get(iid)
            except Exception:
                cls = None
        if cls is None:
            reasons.append("unknown_instruction_id:%s" % iid); continue
        resolved += 1
        try:
            obj = cls(iid)
            inst_ok += 1
        except Exception as e:
            reasons.append("constructor_fail:%s" % str(e)[:40]); continue
        try:
            if hasattr(obj, "build_description"):
                obj.build_description(**k)
            sch_ok += 1
        except TypeError as e:
            reasons.append("kwargs_schema:%s:%s" % (iid, str(e)[:50]))
        except Exception as e:
            reasons.append("kwargs_other:%s:%s" % (iid, str(e)[:50]))
    dup = len(ids) - len(set(ids))
    status = "OK" if (parse_ok and resolved == len(ids) and inst_ok == len(ids)
                      and sch_ok == len(ids) and dup == 0) else "EXCLUDE"
    if status == "EXCLUDE":
        excl.append((key, ";".join(reasons) or "structure"))
    ifeval_rows.append(dict(key=key, parse_ok=parse_ok, instruction_id_count=len(ids),
                            kwargs_count=len(kw), instruction_ids_resolve=resolved,
                            instruction_objects_instantiate=inst_ok,
                            kwargs_schema_compatible=sch_ok,
                            duplicate_instruction_ids=dup,
                            missing_required_kwargs=miss, extra_unexpected_kwargs=extra,
                            objective_consistency_status=status,
                            objective_exclusion_reason=";".join(reasons)))
with open(Q + "/IFEVAL_PREFREEZE_CONSISTENCY_AUDIT.csv", "w", newline="", encoding="utf-8") as f:
    wr = csv.DictWriter(f, fieldnames=list(ifeval_rows[0].keys()))
    wr.writeheader(); wr.writerows(ifeval_rows)
w("IFEVAL_EXCLUSION_LIST.txt",
  "\n".join(["IFEVAL_EXCLUSION_LIST", "",
             "Exclusions UNIQUEMENT pour defaut objectif detecte AVANT inference.", ""] +
            ["%s : %s" % (k, r) for k, r in excl]) + "\n")
P("IFEVAL_EXCLUDED_PREFREEZE_COUNT=%d" % len(excl))
P("IFEVAL_SELECTED_COUNT=%d" % (len(raw) - len(excl)))
P("IFEVAL_CATEGORY_SUFFICIENT=%s" % ("YES" if len(raw) - len(excl) >= 100 else "NO"))
for k, r in excl[:5]:
    P("  exclu %s : %s" % (k, r[:80]))

# ============================================================ E. MMLU-PRO
P()
P("=== E. MMLU-PRO : SELECTION ===")
import pyarrow.parquet as pq
f = sorted(glob.glob(E + "/mmlu-pro/**/*.parquet", recursive=True))[0]
t = pq.read_table(f)
items = t.to_pylist()
bycat = {}
for r in items:
    sel = {"question_id": r["question_id"], "question": r["question"],
           "options": list(r["options"]), "answer": r["answer"],
           "answer_index": r["answer_index"], "category": r["category"],
           "src": r["src"]}
    h = sha(canon_json(sel))
    bycat.setdefault(r["category"], []).append((h, r, sel))
sel_mmlu = []
for cat in sorted(bycat):
    lst = sorted(bycat[cat], key=lambda x: x[0])[:100]
    sel_mmlu.extend(lst)
os.makedirs(C + "/task_eval/mmlu_pro/items", exist_ok=True)
with open(C + "/task_eval/mmlu_pro/MMLU_PRO_ITEMS.jsonl", "w", encoding="utf-8", newline="\n") as fo:
    for h, r, sel in sel_mmlu:
        fo.write(canon_json({"item_id": "mmlu_pro:%s" % r["question_id"],
                             "selection_sha256": h, "category": r["category"],
                             "question": r["question"], "options": list(r["options"]),
                             "answer": r["answer"], "answer_index": r["answer_index"],
                             "src": r["src"], "option_count": len(r["options"])}) + "\n")
with open(C + "/task_eval/mmlu_pro/MMLU_PRO_SELECTION.csv", "w", newline="", encoding="utf-8") as fo:
    wr = csv.writer(fo)
    wr.writerow(["item_id", "question_id", "category", "selection_sha256",
                 "answer_index", "option_count"])
    for h, r, sel in sel_mmlu:
        wr.writerow(["mmlu_pro:%s" % r["question_id"], r["question_id"], r["category"],
                     h, r["answer_index"], len(r["options"])])
cc = {}
for _, r, _s in sel_mmlu:
    cc[r["category"]] = cc.get(r["category"], 0) + 1
P("MMLU_PRO_SELECTED_COUNT=%d" % len(sel_mmlu))
P("MMLU_PRO_SELECTED_CATEGORY_COUNT=%d" % len(cc))
P("MMLU_PRO_SELECTED_PER_CATEGORY_MIN=%d" % min(cc.values()))
P("MMLU_PRO_SELECTED_PER_CATEGORY_MAX=%d" % max(cc.values()))
P("MMLU_PRO_SELECTED_DUPLICATE_QUESTION_ID_COUNT=%d" %
  (len(sel_mmlu) - len({r["question_id"] for _, r, _ in sel_mmlu})))
P("MMLU_PRO_SELECTED_DUPLICATE_CANONICAL_HASH_COUNT=%d" %
  (len(sel_mmlu) - len({h for h, _, _ in sel_mmlu})))

# ============================================================ F. GSM8K
P()
P("=== F. GSM8K : SELECTION ===")
gs = [json.loads(l) for l in open(E + "/gsm8k/grade_school_math/data/test.jsonl") if l.strip()]
scored = sorted(((sha(canon_json({"question": r["question"], "answer": r["answer"]})), r)
                 for r in gs), key=lambda x: x[0])[:500]
os.makedirs(C + "/task_eval/gsm8k/items", exist_ok=True)
with open(C + "/task_eval/gsm8k/GSM8K_ITEMS.jsonl", "w", encoding="utf-8", newline="\n") as fo:
    for h, r in scored:
        fo.write(canon_json({"item_id": "gsm8k:%s" % h[:16], "selection_sha256": h,
                             "question": r["question"], "answer": r["answer"]}) + "\n")
with open(C + "/task_eval/gsm8k/GSM8K_SELECTION.csv", "w", newline="", encoding="utf-8") as fo:
    wr = csv.writer(fo); wr.writerow(["item_id", "selection_sha256"])
    for h, r in scored:
        wr.writerow(["gsm8k:%s" % h[:16], h])
P("GSM8K_SELECTED_COUNT=%d" % len(scored))
P("GSM8K_SELECTED_DUPLICATE_HASH_COUNT=%d" % (len(scored) - len({h for h, _ in scored})))

# ============================================================ G. MBPP
P()
P("=== G. MBPP : MATERIALISATION ===")
mb = json.loads(open(E + "/mbpp/mbpp/sanitized-mbpp.json", "rb").read())
mbsel = sorted([r for r in mb if 11 <= int(r["task_id"]) <= 510], key=lambda r: int(r["task_id"]))
os.makedirs(C + "/task_eval/mbpp/items", exist_ok=True)
with open(C + "/task_eval/mbpp/MBPP_ITEMS.jsonl", "w", encoding="utf-8", newline="\n") as fo:
    for r in mbsel:
        sel = {"task_id": r["task_id"], "prompt": r["prompt"],
               "test_imports": list(r["test_imports"]), "test_list": list(r["test_list"])}
        fo.write(canon_json({"item_id": "mbpp:%s" % r["task_id"],
                             "selection_sha256": sha(canon_json(sel)),
                             "task_id": r["task_id"], "prompt": r["prompt"],
                             "test_imports": list(r["test_imports"]),
                             "test_list": list(r["test_list"]),
                             "source_file": r.get("source_file"),
                             "reference_code": r.get("code"),
                             "reference_code_exposed_to_model": "NO"}) + "\n")
with open(C + "/task_eval/mbpp/MBPP_SELECTION.csv", "w", newline="", encoding="utf-8") as fo:
    wr = csv.writer(fo); wr.writerow(["item_id", "task_id", "test_import_count", "test_count"])
    for r in mbsel:
        wr.writerow(["mbpp:%s" % r["task_id"], r["task_id"], len(r["test_imports"]), len(r["test_list"])])
P("MBPP_SELECTED_COUNT=%d" % len(mbsel))
P("MBPP_SELECTED_DUPLICATE_TASK_ID_COUNT=%d" % (len(mbsel) - len({r["task_id"] for r in mbsel})))

# ============================================================ H. IFEVAL
P()
P("=== H. IFEVAL : MATERIALISATION ===")
excl_keys = {k for k, _ in excl}
ifsel = [r for r in raw if r.get("key") not in excl_keys]
os.makedirs(C + "/task_eval/ifeval/items", exist_ok=True)
with open(C + "/task_eval/ifeval/IFEVAL_ITEMS.jsonl", "w", encoding="utf-8", newline="\n") as fo:
    for r in ifsel:
        fo.write(canon_json({"item_id": "ifeval:%s" % r["key"], "key": r["key"],
                             "prompt": r["prompt"],
                             "instruction_id_list": r["instruction_id_list"],
                             "kwargs": r["kwargs"]}) + "\n")
with open(C + "/task_eval/ifeval/IFEVAL_SELECTION.csv", "w", newline="", encoding="utf-8") as fo:
    wr = csv.writer(fo); wr.writerow(["item_id", "key", "instruction_count"])
    for r in ifsel:
        wr.writerow(["ifeval:%s" % r["key"], r["key"], len(r["instruction_id_list"])])
P("IFEVAL_SELECTED_COUNT=%d" % len(ifsel))

# ============================================================ 40. IDs GLOBAUX
allids = (["mmlu_pro:%s" % r["question_id"] for _, r, _ in sel_mmlu] +
          ["gsm8k:%s" % h[:16] for h, _ in scored] +
          ["mbpp:%s" % r["task_id"] for r in mbsel] +
          ["ifeval:%s" % r["key"] for r in ifsel])
P()
P("TASK_GLOBAL_ITEM_ID_DUPLICATE_COUNT=%d" % (len(allids) - len(set(allids))))
P("TASK_KNOWLEDGE_REASONING_COUNT=%d" % len(sel_mmlu))
P("TASK_MATH_COUNT=%d" % len(scored))
P("TASK_CODE_COUNT=%d" % len(mbsel))
P("TASK_INSTRUCTION_COUNT=%d" % len(ifsel))
P("TASK_EVAL_ITEM_COUNT=%d" % len(allids))
P("TASK_CORPUS_SUFFICIENT=%s" % ("YES" if (len(allids) >= 1000 and len(mbsel) >= 100
                                           and len(scored) >= 100 and len(sel_mmlu) >= 100
                                           and len(ifsel) >= 100) else "NO"))

# tree hash task provisoire
def tree_hash(root):
    h = hashlib.sha256()
    for dp, _dn, fn in sorted(os.walk(root)):
        for f in sorted(fn):
            fp = os.path.join(dp, f)
            rel = os.path.relpath(fp, root)
            h.update(rel.encode()); h.update(b"\x00")
            h.update(open(fp, "rb").read()); h.update(b"\x00")
    return h.hexdigest()
TASK_TREE = tree_hash(C + "/task_eval")
P("TASK_EVAL_DATA_CONTENT_TREE_SHA256=%s" % TASK_TREE)

with open(Q + "/ENGINE008_DATA_STATUS.txt", "w", encoding="utf-8", newline="\n") as f:
    f.write("\n".join(out) + "\n")
print()
print("TASK_DATA_STATUS=" + ("PASS" if len(allids) >= 1000 else "FAIL"))
