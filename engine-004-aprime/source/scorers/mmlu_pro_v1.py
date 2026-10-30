#!/usr/bin/env python3
"""ENGINE-008 scorer : MMLU-Pro (CHOICE_CONDITIONAL_LOGLIKELIHOOD).

Le scorer ne fait AUCUNE inference. Il recoit des log-vraisemblances deja
calculees par le futur harnais et decide correctness.
"""
import math

SCORER_ID = "mmlu_pro_v1"
VERSION = "1"
INPUT_SCHEMA = ["item_id", "answer_index", "option_count", "choice_scores"]
OUTPUT_SCHEMA = ["item_id", "predicted_index", "answer_index", "correct", "tie"]


class MMLUScorerError(Exception):
    pass


def score_item(item_id, answer_index, option_count, choice_scores):
    """Rend dict(item_id, predicted_index, answer_index, correct, tie).

    Tie policy gelee : plus PETIT index parmi les maxima ex aequo.
    Aucune normalisation par longueur : les labels sont des symboles courts.
    """
    if not isinstance(option_count, int) or option_count < 2:
        raise MMLUScorerError("%s : option_count invalide %r" % (item_id, option_count))
    if not isinstance(answer_index, int) or not (0 <= answer_index < option_count):
        raise MMLUScorerError("%s : answer_index hors bornes %r (option_count=%d)"
                              % (item_id, answer_index, option_count))
    if len(choice_scores) != option_count:
        raise MMLUScorerError("%s : %d scores pour %d options"
                              % (item_id, len(choice_scores), option_count))
    for i, s in enumerate(choice_scores):
        if s is None:
            raise MMLUScorerError("%s : score manquant a l index %d" % (item_id, i))
        try:
            f = float(s)
        except (TypeError, ValueError):
            raise MMLUScorerError("%s : score non numerique a l index %d : %r" % (item_id, i, s))
        if math.isnan(f):
            raise MMLUScorerError("%s : NaN a l index %d" % (item_id, i))
        if math.isinf(f):
            raise MMLUScorerError("%s : Inf a l index %d" % (item_id, i))
    best = max(choice_scores)
    tied = [i for i, s in enumerate(choice_scores) if s == best]
    # tie policy : plus petit index
    predicted = min(tied)
    return dict(item_id=item_id, predicted_index=predicted, answer_index=answer_index,
                correct=(predicted == answer_index), tie=(len(tied) > 1))


def selftest():
    """Rend (ok, [lignes]). Fixtures SYNTHETIQUES uniquement."""
    out = []
    def chk(name, cond, detail=""):
        out.append("%-46s %s %s" % (name, "PASS" if cond else "FAIL", detail))
        return cond
    ok = True
    # 3 choix, bon gagnant
    r = score_item("a", 1, 3, [-2.0, -0.1, -3.0])
    ok &= chk("3_choices_correct_winner", r["correct"] and r["predicted_index"] == 1)
    # 3 choix, mauvais gagnant
    r = score_item("b", 1, 3, [-0.1, -2.0, -3.0])
    ok &= chk("3_choices_wrong_winner", (not r["correct"]) and r["predicted_index"] == 0)
    # 10 choix, bon gagnant
    r = score_item("c", 9, 10, [-9.0, -8, -7, -6, -5, -4, -3, -2, -1, -0.05])
    ok &= chk("10_choices_correct_winner", r["correct"] and r["predicted_index"] == 9)
    # 10 choix, mauvais gagnant
    r = score_item("d", 9, 10, [-0.05, -8, -7, -6, -5, -4, -3, -2, -1, -9.0])
    ok &= chk("10_choices_wrong_winner", (not r["correct"]) and r["predicted_index"] == 0)
    # egalite a deux : plus petit index
    r = score_item("e", 3, 4, [-1.0, -1.0, -5.0, -6.0])
    ok &= chk("exact_two_way_tie_smallest_index",
              r["tie"] and r["predicted_index"] == 0 and not r["correct"])
    # egalite multiple
    r = score_item("f", 2, 5, [-2.0, -2.0, -2.0, -9.0, -9.0])
    ok &= chk("exact_multi_way_tie_smallest_index", r["tie"] and r["predicted_index"] == 0)
    # erreurs dures
    for name, fn in (("answer_index_out_of_range", lambda: score_item("g", 5, 3, [-1, -2, -3])),
                     ("missing_choice_score", lambda: score_item("h", 0, 3, [-1, None, -3])),
                     ("nan", lambda: score_item("i", 0, 3, [float("nan"), -2, -3])),
                     ("inf", lambda: score_item("j", 0, 3, [float("inf"), -2, -3])),
                     ("score_count_mismatch", lambda: score_item("k", 0, 4, [-1, -2, -3]))):
        try:
            fn()
            ok &= chk(name, False, "aucune erreur levee")
        except MMLUScorerError as e:
            ok &= chk(name, True, str(e)[:60])
    return bool(ok), out


if __name__ == "__main__":
    import sys
    o, lines = selftest()
    print("\n".join(lines))
    print("MMLU_SCORER_SELFTEST=%s" % ("PASS" if o else "FAIL"))
    sys.exit(0 if o else 1)
