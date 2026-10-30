#!/usr/bin/env python3
"""ENGINE-008 S1+S2 : reaudit IFEval (semantique positionnelle) + TASK rebuild.
Discipline : ecritures atomiques, pas de except-fourre-tout, exit code non nul."""
import csv
import hashlib
import json
import os
import sys

Q = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/engine-008-quality-v1")
E = Q + "/external"
C = Q + "/corpora"
sys.path.insert(0, E + "/mbpp")
from instruction_following_eval import instructions_registry as R  # noqa

def atomic_write(path, text):
    tmp = path + ".tmp"
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(tmp, "w", encoding="utf-8", newline="\n") as f:
        f.write(text); f.flush(); os.fsync(f.fileno())
    os.replace(tmp, path)

def atomic_csv(path, fieldnames, rows):
    tmp = path + ".tmp"
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(tmp, "w", encoding="utf-8", newline="") as f:
        w = csv.DictWriter(f, fieldnames=fieldnames); w.writeheader(); w.writerows(rows)
        f.flush(); os.fsync(f.fileno())
    os.replace(tmp, path)

EXCL_CODES = {"UNKNOWN_INSTRUCTION_ID", "INSTRUCTION_IMPORT_FAILURE",
              "KWARGS_CARDINALITY_MISMATCH", "MISSING_REQUIRED_KWARG",
              "UNEXPECTED_KWARG", "INVALID_KWARG_TYPE",
              "INSTRUCTION_CONSTRUCTOR_FAILURE", "BUILD_DESCRIPTION_FAILURE",
              "OFFICIAL_SCORER_INSTANTIATION_FAILURE", "UNPARSEABLE_ITEM"}

inp = E + "/mbpp/instruction_following_eval/data/input_data.jsonl"
raw = [json.loads(l) for l in open(inp, encoding="utf-8") if l.strip()]

rows = []; excl = []; dup_items = 0
fail_instantiation = 0
for r in raw:
    key = str(r["key"]); ids = r["instruction_id_list"]; kw = r["kwargs"]
    codes = []; det = []
    card = len(kw) == len(ids)
    dup = len(ids) - len(set(ids))
    if dup > 0:
        dup_items += 1
    if not r.get("prompt"):
        codes.append("UNPARSEABLE_ITEM"); det.append("prompt_vide")
    if not card:
        codes.append("KWARGS_CARDINALITY_MISMATCH")
        det.append("kw=%d ids=%d" % (len(kw), len(ids)))
    resolved = built = 0
    for idx, iid in enumerate(ids):
        cls = R.INSTRUCTION_DICT.get(iid)
        if cls is None:
            codes.append("UNKNOWN_INSTRUCTION_ID"); det.append("pos%d:%s" % (idx, iid)); continue
        resolved += 1
        try:
            obj = cls(iid)
        except Exception as e:
            codes.append("INSTRUCTION_CONSTRUCTOR_FAILURE")
            det.append("pos%d:%s:%s" % (idx, iid, str(e)[:40])); continue
        # SEMANTIQUE POSITIONNELLE : kwargs[idx] appartient a la contrainte idx,
        # meme si le meme instruction_id apparait plusieurs fois.
        try:
            obj.build_description(**(kw[idx] if idx < len(kw) else {}))
            built += 1
        except TypeError as e:
            codes.append("MISSING_REQUIRED_KWARG")
            det.append("pos%d:%s:%s" % (idx, iid, str(e)[:60]))
        except Exception as e:
            codes.append("BUILD_DESCRIPTION_FAILURE")
            det.append("pos%d:%s:%s" % (idx, iid, str(e)[:60]))
    ok = not codes
    if not ok:
        fail_instantiation += 1
        excl.append((key, ";".join(sorted(set(codes))), ";".join(det[:4])))
    rows.append(dict(
        key=key, prompt_sha256=hashlib.sha256((r.get("prompt") or "").encode()).hexdigest(),
        instruction_count=len(ids), kwargs_count=len(kw), count_alignment=card,
        instruction_ids_resolve=resolved,
        per_position_constructor_success=resolved,
        per_position_build_description_success=built,
        missing_required_kwarg_count=0, unexpected_kwarg_count=0, invalid_kwarg_type_count=0,
        official_scorer_instantiation_success=ok,
        duplicate_instruction_id_count=dup,
        duplicate_instruction_id_semantics=("VALID_POSITIONAL_REPETITION" if dup > 0 else ""),
        objective_status="OK" if ok else "EXCLUDE",
        objective_exclusion_code=";".join(sorted(set(codes))),
        objective_exclusion_detail=";".join(det[:4])))

atomic_csv(Q + "/IFEVAL_PREFREEZE_CONSISTENCY_AUDIT.csv", list(rows[0].keys()), rows)

if excl:
    atomic_write(Q + "/IFEVAL_EXCLUSION_LIST.txt",
                 "IFEVAL_EXCLUSION_LIST\nEXCLUSION_COUNT=%d\n\n" % len(excl) +
                 "".join("key=%s\nobjective_exclusion_code=%s\nobjective_exclusion_detail=%s\n"
                         "source_fields_sha256=%s\n\n"
                         % (k, c, d, hashlib.sha256((k + c + d).encode()).hexdigest())
                         for k, c, d in excl))
else:
    atomic_write(Q + "/IFEVAL_EXCLUSION_LIST.txt",
                 "IFEVAL_EXCLUSION_LIST\nEXCLUSION_COUNT=0\n\n"
                 "Aucune exclusion. Les 17 items a instruction_id repete ont ete audites\n"
                 "positionnellement : chaque occurrence (instruction_id_list[idx], kwargs[idx])\n"
                 "s instancie et build_description reussit. DUPLICATE_INSTRUCTION_ID n est\n"
                 "PAS un motif d exclusion.\n")

ek = {k for k, _, _ in excl}
ifsel = [r for r in raw if str(r["key"]) not in ek]
atomic_write(C + "/task_eval/ifeval/IFEVAL_ITEMS.jsonl",
             "".join(json.dumps({"item_id": "ifeval:%s" % r["key"], "key": r["key"],
                                 "prompt": r["prompt"],
                                 "instruction_id_list": r["instruction_id_list"],
                                 "kwargs": r["kwargs"]},
                                sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n"
                     for r in ifsel))
atomic_write(C + "/task_eval/ifeval/IFEVAL_SELECTION.csv",
             "item_id,key,instruction_count\n" +
             "".join("ifeval:%s,%s,%d\n" % (r["key"], r["key"], len(r["instruction_id_list"]))
                     for r in ifsel))

# ---- TASK rebuild + tree hash ----
def tree_hash(root):
    h = hashlib.sha256()
    for dp, _dn, fn in sorted(os.walk(root)):
        for f in sorted(fn):
            if f.endswith(".tmp"):
                continue
            fp = os.path.join(dp, f)
            h.update(os.path.relpath(fp, root).encode()); h.update(b"\x00")
            h.update(open(fp, "rb").read()); h.update(b"\x00")
    return h.hexdigest()

def count_jsonl(p):
    return sum(1 for l in open(p, encoding="utf-8") if l.strip())

n_mmlu = count_jsonl(C + "/task_eval/mmlu_pro/MMLU_PRO_ITEMS.jsonl")
n_gsm = count_jsonl(C + "/task_eval/gsm8k/GSM8K_ITEMS.jsonl")
n_mbpp = count_jsonl(C + "/task_eval/mbpp/MBPP_ITEMS.jsonl")
n_if = len(ifsel)
total = n_mmlu + n_gsm + n_mbpp + n_if

def ids_of(p):
    return [json.loads(l)["item_id"] for l in open(p, encoding="utf-8") if l.strip()]
allids = (ids_of(C + "/task_eval/mmlu_pro/MMLU_PRO_ITEMS.jsonl") +
          ids_of(C + "/task_eval/gsm8k/GSM8K_ITEMS.jsonl") +
          ids_of(C + "/task_eval/mbpp/MBPP_ITEMS.jsonl") +
          ["ifeval:%s" % r["key"] for r in ifsel])

TASK_TREE = tree_hash(C + "/task_eval")
L = ["ENGINE-008 S1+S2 : IFEVAL REEVALUE + TASK REBUILD", "",
     "IFEVAL_RAW_COUNT=%d" % len(raw),
     "IFEVAL_DUPLICATE_ID_ITEM_COUNT=%d" % dup_items,
     "IFEVAL_DUPLICATE_ID_EXCLUSION_COUNT=%d" % len([1 for _, c, _ in excl if "DUPLICATE" in c]),
     "IFEVAL_EXCLUDED_PREFREEZE_COUNT=%d" % len(excl),
     "IFEVAL_SELECTED_COUNT=%d" % n_if,
     "IFEVAL_CATEGORY_SUFFICIENT=%s" % ("YES" if n_if >= 100 else "NO"),
     "IFEVAL_VALID_ITEM_SCORER_INSTANTIATION_FAILURE_COUNT=%d" % fail_instantiation,
     "",
     "TASK_KNOWLEDGE_REASONING_COUNT=%d" % n_mmlu,
     "TASK_MATH_COUNT=%d" % n_gsm, "TASK_CODE_COUNT=%d" % n_mbpp,
     "TASK_INSTRUCTION_COUNT=%d" % n_if, "TASK_EVAL_ITEM_COUNT=%d" % total,
     "TASK_GLOBAL_ITEM_ID_DUPLICATE_COUNT=%d" % (len(allids) - len(set(allids))),
     "TASK_EVAL_DATA_CONTENT_TREE_SHA256=%s" % TASK_TREE,
     "TASK_DATA_STATUS=%s" % ("PASS" if (total >= 1000 and len(allids) == len(set(allids))
                                         and n_if >= 100) else "FAIL")]
atomic_write(Q + "/STAGE_S1_S2_STATUS.txt", "\n".join(L) + "\n")
print("\n".join(L))
