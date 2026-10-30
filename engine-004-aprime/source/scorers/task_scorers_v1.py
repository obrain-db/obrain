#!/usr/bin/env python3
"""ENGINE-008 scorers : structured-validity, agregation 4 categories, bootstrap paire.

Aucune inference. Aucun alea non deterministe.
"""
import hashlib
import json
import random

# ---------------------------------------------------------------- structured validity
STRUCTURED_ID = "structured_validity_v1"
STRUCTURED_VERSION = "1"

# Namespaces inclus : contraintes de FORME (format, structure, casse, ponctuation,
# debut/fin, combinaison de forme). Exclus : contenu semantique, longueur seule,
# langue/sujet.
INCLUDED_NAMESPACES = ["detectable_format", "change_case", "startend",
                       "punctuation", "combination"]
EXCLUDED_NAMESPACES = ["keywords", "length_constraints", "detectable_content", "language"]
EXCLUSION_REASONS = {
    "keywords": "contrainte de CONTENU semantique (presence/absence/frequence de mots)",
    "length_constraints": "contrainte de LONGUEUR/COMPTAGE seule",
    "detectable_content": "contrainte de CONTENU (placeholders, postscript)",
    "language": "contrainte de LANGUE / sujet",
}
INCLUSION_REASONS = {
    "detectable_format": "FORMAT de reponse (json, sections, listes, titre, reponse contrainte)",
    "change_case": "CASSE de la reponse",
    "startend": "FORME de debut/fin (citation, terminaison exacte)",
    "punctuation": "PONCTUATION",
    "combination": "STRUCTURE de reponse (repetition du prompt, deux reponses)",
}


def matching_instructions(instruction_id_list):
    return [i for i in instruction_id_list if i.split(":")[0] in INCLUDED_NAMESPACES]


def matching_namespaces(instruction_id_list):
    return sorted({i.split(":")[0] for i in instruction_id_list
                   if i.split(":")[0] in INCLUDED_NAMESPACES})


def in_structured_subset(instruction_id_list):
    return len(matching_instructions(instruction_id_list)) > 0


def structured_score(strict_follow_all_instructions):
    """Le score structured-validity EST le STRICT officiel, restreint au sous-ensemble."""
    return 1 if strict_follow_all_instructions else 0


def structured_selftest():
    out = []
    def chk(n, c, d=""):
        out.append("%-56s %s %s" % (n, "PASS" if c else "FAIL", d)); return c
    ok = True
    ok &= chk("format_only_is_included",
              in_structured_subset(["detectable_format:json_format"]))
    ok &= chk("punctuation_is_included",
              in_structured_subset(["punctuation:no_comma"]))
    ok &= chk("combination_is_included",
              in_structured_subset(["combination:two_responses"]))
    ok &= chk("keywords_only_is_excluded",
              not in_structured_subset(["keywords:existence"]))
    ok &= chk("length_only_is_excluded",
              not in_structured_subset(["length_constraints:number_words"]))
    ok &= chk("language_only_is_excluded",
              not in_structured_subset(["language:response_language"]))
    ok &= chk("mixed_is_included",
              in_structured_subset(["keywords:existence", "detectable_format:title"]))
    ok &= chk("score_maps_strict", structured_score(True) == 1 and structured_score(False) == 0)
    return bool(ok), out


# ---------------------------------------------------------------- task aggregate
AGG_ID = "task_aggregate_v1"
AGG_VERSION = "1"
MAIN_CATEGORIES = ["knowledge_reasoning", "math", "code", "instruction"]
TASK_AGGREGATION_MODE = "FOUR_CATEGORY_MACRO_AVERAGE"


class AggregateError(Exception):
    pass


def aggregate(rows):
    """rows : [{item_id, category, baseline_score, candidate_score}].

    Rend dict avec les accuracies par categorie, la macro et le delta macro.
    Valide : memes IDs des deux cotes, pas de doublon, pas de score manquant,
    score dans {0,1}, les 4 categories presentes.
    """
    seen = set()
    per = {c: {"n": 0, "b": 0, "c": 0} for c in MAIN_CATEGORIES}
    for r in rows:
        iid = r["item_id"]
        if iid in seen:
            raise AggregateError("item duplique : %s" % iid)
        seen.add(iid)
        cat = r["category"]
        if cat not in per:
            raise AggregateError("categorie inconnue : %r" % cat)
        for k in ("baseline_score", "candidate_score"):
            if k not in r or r[k] is None:
                raise AggregateError("%s : %s manquant" % (iid, k))
            if r[k] not in (0, 1):
                raise AggregateError("%s : %s hors {0,1} : %r" % (iid, k, r[k]))
        per[cat]["n"] += 1
        per[cat]["b"] += r["baseline_score"]
        per[cat]["c"] += r["candidate_score"]
    for c in MAIN_CATEGORIES:
        if per[c]["n"] == 0:
            raise AggregateError("categorie absente ou vide : %s" % c)
    acc = {}
    for c in MAIN_CATEGORIES:
        n = per[c]["n"]
        acc[c] = dict(n=n, baseline=per[c]["b"] / n, candidate=per[c]["c"] / n,
                      delta=(per[c]["c"] - per[c]["b"]) / n)
    baseline_macro = sum(acc[c]["baseline"] for c in MAIN_CATEGORIES) / len(MAIN_CATEGORIES)
    candidate_macro = sum(acc[c]["candidate"] for c in MAIN_CATEGORIES) / len(MAIN_CATEGORIES)
    return dict(per_category=acc, baseline_macro=baseline_macro,
                candidate_macro=candidate_macro,
                macro_delta=candidate_macro - baseline_macro,
                total_items=len(rows), mode=TASK_AGGREGATION_MODE)


def aggregate_selftest():
    out = []
    def chk(n, c, d=""):
        out.append("%-56s %s %s" % (n, "PASS" if c else "FAIL", d)); return c
    ok = True
    def mk(spec):
        rows = []
        for cat, n, bs, cs in spec:
            for i in range(n):
                rows.append(dict(item_id="%s:%d" % (cat, i), category=cat,
                                 baseline_score=bs[i] if isinstance(bs, list) else bs,
                                 candidate_score=cs[i] if isinstance(cs, list) else cs))
        return rows
    base = [("knowledge_reasoning", 10, 1, 1), ("math", 10, 1, 1),
            ("code", 10, 1, 1), ("instruction", 10, 1, 1)]
    r = aggregate(mk(base))
    ok &= chk("identical_scores_delta_zero", abs(r["macro_delta"]) < 1e-12)
    # candidat +1 item (des categories inegales)
    rows = mk([("knowledge_reasoning", 10, 1, 1), ("math", 10, 1, 1),
               ("code", 10, 1, 1), ("instruction", 10, [1]*10, [0]*9 + [1])])
    r = aggregate(rows)
    ok &= chk("unbalanced_item_counts_ok", r["per_category"]["instruction"]["n"] == 10)
    # candidat strictement pire sur une categorie
    rows = mk([("knowledge_reasoning", 10, 1, 0), ("math", 10, 1, 1),
               ("code", 10, 1, 1), ("instruction", 10, 1, 1)])
    r = aggregate(rows)
    ok &= chk("candidate_worse_detected",
              abs(r["per_category"]["knowledge_reasoning"]["delta"] + 1.0) < 1e-12
              and r["macro_delta"] < 0)
    for name, spec, frag in (
            ("missing_category", [("knowledge_reasoning", 1, 1, 1)], "absente"),
            ("invalid_score", [("knowledge_reasoning", 1, 1, 2), ("math", 1, 1, 1),
                               ("code", 1, 1, 1), ("instruction", 1, 1, 1)], "hors"),
            ("missing_score", [("knowledge_reasoning", 1, None, 1), ("math", 1, 1, 1),
                               ("code", 1, 1, 1), ("instruction", 1, 1, 1)], "manquant")):
        try:
            aggregate(mk(spec)); ok &= chk(name, False, "aucune erreur levee")
        except AggregateError as e:
            ok &= chk(name, frag in str(e), str(e)[:50])
    # item duplique
    rows = mk(base); rows.append(dict(rows[0]))
    try:
        aggregate(rows); ok &= chk("duplicate_item", False)
    except AggregateError as e:
        ok &= chk("duplicate_item", "duplique" in str(e))
    return bool(ok), out


# ---------------------------------------------------------------- bootstrap paire
BOOT_ID = "bootstrap_paired_v1"
BOOT_VERSION = "1"
BOOTSTRAP_RESAMPLES = 10000
_SEED_MATERIAL = "ENGINE-008-QUALITY-v1|paired-bootstrap|v1"
BOOTSTRAP_SEED_U64 = int.from_bytes(
    hashlib.sha256(_SEED_MATERIAL.encode("utf-8")).digest()[0:8], "big")


def bootstrap_paired(rows, resamples=BOOTSTRAP_RESAMPLES, seed=BOOTSTRAP_SEED_U64):
    """Bootstrap PAIRÉ, stratifié par categorie.

    A chaque replication, pour chaque categorie, on tire N_category indices
    AVEC remise, et on utilise EXACTEMENT les memes indices pour baseline et
    candidat. Le delta macro est la moyenne des 4 deltas de categorie.
    """
    by = {c: [] for c in MAIN_CATEGORIES}
    for r in rows:
        if r["category"] in by:
            by[r["category"]].append((r["baseline_score"], r["candidate_score"]))
    for c in MAIN_CATEGORIES:
        if not by[c]:
            raise AggregateError("categorie vide pour bootstrap : %s" % c)
    rng = random.Random(seed)
    deltas = []
    for _ in range(resamples):
        cat_deltas = []
        for c in MAIN_CATEGORIES:
            arr = by[c]; n = len(arr)
            sb = sc = 0
            for _i in range(n):
                j = rng.randrange(n)          # MEME indice pour les deux cotes
                sb += arr[j][0]; sc += arr[j][1]
            cat_deltas.append((sc - sb) / n)
        deltas.append(sum(cat_deltas) / len(cat_deltas))
    deltas.sort()
    def pct(p):
        # percentile empirique, interpolation lineaire (algorithme documente)
        if not deltas:
            return 0.0
        k = (len(deltas) - 1) * p
        f = int(k); c = min(f + 1, len(deltas) - 1)
        return deltas[f] + (deltas[c] - deltas[f]) * (k - f)
    return dict(n_resamples=len(deltas), seed=seed,
                ci95_low=pct(0.025), ci95_high=pct(0.975),
                mean=sum(deltas) / len(deltas),
                percentile_algorithm="empirical_linear_interpolation_k=(n-1)*p",
                stratification="per_category_matched_indices")


def bootstrap_selftest():
    out = []
    def chk(n, c, d=""):
        out.append("%-56s %s %s" % (n, "PASS" if c else "FAIL", d)); return c
    ok = True
    def mk(cat_spec):
        rows = []
        for cat, n, b, c in cat_spec:
            for i in range(n):
                rows.append(dict(item_id="%s:%d" % (cat, i), category=cat,
                                 baseline_score=b, candidate_score=c))
        return rows
    N = 20
    z = mk([(c, N, 1, 1) for c in MAIN_CATEGORIES])
    r = bootstrap_paired(z, resamples=200)
    ok &= chk("identical_arrays_zero_delta",
              abs(r["mean"]) < 1e-12 and abs(r["ci95_low"]) < 1e-12
              and abs(r["ci95_high"]) < 1e-12)
    better = mk([(c, N, 0, 1) for c in MAIN_CATEGORIES])
    r = bootstrap_paired(better, resamples=200)
    ok &= chk("candidate_always_better", abs(r["ci95_low"] - 1.0) < 1e-12)
    worse = mk([(c, N, 1, 0) for c in MAIN_CATEGORIES])
    r = bootstrap_paired(worse, resamples=200)
    ok &= chk("candidate_always_worse", abs(r["ci95_high"] + 1.0) < 1e-12)
    # tailles inegales + categorie d un seul item
    mixed = mk([("knowledge_reasoning", 30, 1, 1), ("math", 1, 1, 0),
                ("code", 5, 1, 1), ("instruction", 12, 1, 1)])
    r = bootstrap_paired(mixed, resamples=200)
    ok &= chk("unequal_and_single_item_category", r["n_resamples"] == 200)
    # repetabilite : meme graine deux fois
    a = bootstrap_paired(mixed, resamples=500)
    b = bootstrap_paired(mixed, resamples=500)
    ok &= chk("same_seed_repeatable", a == b)
    return bool(ok), out


if __name__ == "__main__":
    import sys
    allok = True
    for name, fn, gate in (("structured", structured_selftest, "STRUCTURED_VALIDITY_SCORER_SELFTEST"),
                           ("aggregate", aggregate_selftest, "TASK_AGGREGATE_SELFTEST"),
                           ("bootstrap", bootstrap_selftest, "BOOTSTRAP_SELFTEST")):
        o, lines = fn()
        print("=== %s ===" % name); print("\n".join(lines)); print()
        print("%s=%s" % (gate, "PASS" if o else "FAIL")); print()
        allok &= bool(o)
    print("BOOTSTRAP_SEED_U64=%d" % BOOTSTRAP_SEED_U64)
    sys.exit(0 if allok else 1)
