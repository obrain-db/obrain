#!/usr/bin/env python3
"""ENGINE-008 DATA -- S1 : reaudit IFEval (identite POSITIONNELLE) + reconstruction TASK.

Discipline :
  - ecritures atomiques (tmp + rename)
  - exceptions non masquees (exit non nul)
  - aucun modele
"""
import csv
import hashlib
import json
import os
import sys

Q = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/engine-008-quality-v1")
E = Q + "/external"
C = Q + "/corpora"
VOCAB = 248320


def sha_bytes(b):
    return hashlib.sha256(b).hexdigest()


def atomic_write(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8", newline="\n") as f:
        f.write(data)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, path)


def canon_json(o):
    return json.dumps(o, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


# ===================================================== S1 : IFEval
sys.path.insert(0, E + "/mbpp")
from instruction_following_eval import instructions_registry as REG

raw = [json.loads(l) for l in open(E + "/mbpp/instruction_following_eval/data/input_data.jsonl")
       if l.strip()]

ROWS, EXCL = [], []
TOTAL_POS_FAIL = 0
for r in raw:
    key = str(r["key"])
    ids = r["instruction_id_list"]
    kw = r["kwargs"]
    codes, detail = [], []

    if not r.get("prompt"):
        codes.append("UNPARSEABLE_ITEM"); detail.append("prompt_vide")
    if len(kw) != len(ids):
        codes.append("KWARGS_CARDINALITY_MISMATCH")
        detail.append("kwargs=%d ids=%d" % (len(kw), len(ids)))

    # --- validation POSITIONNELLE : (index, instruction_id, kwargs[index]) ---
    n = min(len(ids), len(kw))
    ctor_ok = build_ok = 0
    for index in range(n):
        iid = ids[index]
        k = kw[index]
        cls = REG.INSTRUCTION_DICT.get(iid)
        if cls is None:
            codes.append("UNKNOWN_INSTRUCTION_ID"); detail.append("pos%d:%s" % (index, iid)); continue
        try:
            obj = cls(iid)
            ctor_ok += 1
        except Exception as e:
            codes.append("INSTRUCTION_CONSTRUCTOR_FAILURE")
            detail.append("pos%d:%s:%s" % (index, iid, str(e)[:40])); continue
        try:
            obj.build_description(**k)
            build_ok += 1
        except TypeError as e:
            codes.append("MISSING_REQUIRED_KWARG")
            detail.append("pos%d:%s:%s" % (index, iid, str(e)[:50]))
        except Exception as e:
            codes.append("INVALID_KWARG_TYPE")
            detail.append("pos%d:%s:%s" % (index, iid, str(e)[:50]))

    total_pos_fail = (n - build_ok)
    TOTAL_POS_FAIL += total_pos_fail
    if total_pos_fail and not codes:
        codes.append("OFFICIAL_SCORER_INSTANTIATION_FAILURE")

    dup_ids = sorted({i for i in ids if ids.count(i) > 1})
    dup_count = sum(ids.count(i) - 1 for i in set(ids))
    if dup_ids:
        # IDENTITE POSITIONNELLE : un doublon est VALIDE si chaque occurrence
        # s'instancie et construit sa description avec SON kwargs[index].
        dup_sem = ("VALID_POSITIONAL_REPETITION"
                   if not codes else "INVALID_POSITIONAL_REPETITION")
    else:
        dup_sem = "NO_DUPLICATE"

    cs = sorted(set(codes))
    ok = not cs
    if not ok:
        EXCL.append((key, ";".join(cs), ";".join(detail[:6]),
                     sha_bytes(canon_json({"key": key, "ids": ids, "kwargs": kw}).encode())))
    ROWS.append({
        "key": key,
        "prompt_sha256": sha_bytes((r.get("prompt") or "").encode()),
        "instruction_count": len(ids), "kwargs_count": len(kw),
        "count_alignment": len(kw) == len(ids),
        "instruction_ids_resolve": n,
        "per_position_constructor_success": ctor_ok,
        "per_position_build_description_success": build_ok,
        "missing_required_kwarg_count": sum(1 for c in codes if c == "MISSING_REQUIRED_KWARG"),
        "unexpected_kwarg_count": sum(1 for c in codes if c == "UNEXPECTED_KWARG"),
        "invalid_kwarg_type_count": sum(1 for c in codes if c == "INVALID_KWARG_TYPE"),
        "official_scorer_instantiation_success": ok,
        "duplicate_instruction_id_count": dup_count,
        "duplicate_instruction_id_semantics": dup_sem,
        "objective_status": "OK" if ok else "EXCLUDE",
        "objective_exclusion_code": ";".join(cs),
        "objective_exclusion_detail": ";".join(detail[:6]),
    })

with open(Q + "/IFEVAL_PREFREEZE_CONSISTENCY_AUDIT.csv.tmp", "w", newline="", encoding="utf-8") as f:
    w = csv.DictWriter(f, fieldnames=list(ROWS[0].keys()))
    w.writeheader(); w.writerows(ROWS)
    f.flush(); os.fsync(f.fileno())
os.replace(Q + "/IFEVAL_PREFREEZE_CONSISTENCY_AUDIT.csv.tmp",
           Q + "/IFEVAL_PREFREEZE_CONSISTENCY_AUDIT.csv")

dup_items = sum(1 for r in ROWS if r["duplicate_instruction_id_count"] > 0)
if EXCL:
    body = ["IFEVAL_EXCLUSION_LIST", "",
            "EXCLUSION_COUNT=%d" % len(EXCL), "",
            "Chaque exclusion porte un code objectif et sa position fautive.", ""]
    for k, c, d, h in EXCL:
        body += ["key=%s" % k, "objective_exclusion_code=%s" % c,
                 "objective_exclusion_detail=%s" % d, "source_fields_sha256=%s" % h, ""]
else:
    body = ["IFEVAL_EXCLUSION_LIST", "",
            "EXCLUSION_COUNT=0",
            "",
            "Aucune exclusion prefreeze. La regle a ete evaluee sur les 541 items.",
            "Les %d items a instruction_id duplique sont des repetitions" % dup_items,
            "POSITIONNELLES valides : chaque occurrence (index, instruction_id,",
            "kwargs[index]) s'instancie et construit sa description.",
            ""]
atomic_write(Q + "/IFEVAL_EXCLUSION_LIST.txt", "\n".join(body) + "\n")

ek = {k for k, _, _, _ in EXCL}
ifsel = [r for r in raw if str(r["key"]) not in ek]

lines = []
for r in ifsel:
    lines.append(canon_json({"item_id": "ifeval:%s" % r["key"], "key": r["key"],
                             "prompt": r["prompt"],
                             "instruction_id_list": r["instruction_id_list"],
                             "kwargs": r["kwargs"]}))
atomic_write(C + "/task_eval/ifeval/IFEVAL_ITEMS.jsonl", "\n".join(lines) + "\n")

csvl = ["item_id,key,instruction_count"]
for r in ifsel:
    csvl.append("ifeval:%s,%s,%d" % (r["key"], r["key"], len(r["instruction_id_list"])))
atomic_write(C + "/task_eval/ifeval/IFEVAL_SELECTION.csv", "\n".join(csvl) + "\n")

print("IFEVAL_RAW_COUNT=%d" % len(raw))
print("IFEVAL_EXCLUDED_PREFREEZE_COUNT=%d" % len(EXCL))
print("IFEVAL_SELECTED_COUNT=%d" % len(ifsel))
print("IFEVAL_CATEGORY_SUFFICIENT=%s" % ("YES" if len(ifsel) >= 100 else "NO"))
print("IFEVAL_DUPLICATE_ID_ITEM_COUNT=%d" % dup_items)
print("IFEVAL_DUPLICATE_ID_EXCLUSION_COUNT=%d" % len(EXCL))
print("IFEVAL_VALID_ITEM_SCORER_INSTANTIATION_FAILURE_COUNT=%d" % TOTAL_POS_FAIL)
if EXCL:
    print("EXCLUSIONS :")
    for k, c, d, _ in EXCL[:10]:
        print("  %s : %s | %s" % (k, c, d[:90]))
