#!/usr/bin/env python3
"""ENGINE-008 PROTOCOL-HARNESS-01 -- manifeste canonique du perimetre TASK.

DEFINITION NOUVELLE (l'ancienne n'a jamais ete materialisee). Elle est donc
declaree comme telle dans ENGINE008_PROTOCOL_SOURCE_MAP_v1.json.

-- perimetre ------------------------------------------------------------
Inclus : fichiers DATA TASK faisant autorite ; contrats de prompts ; sources de
preparation des entrees (template effectif, config de rendu, source du harnais) ;
autorites des scorers et des seuils pertinentes ; textes rendus et sequences
tokenisees ; ordre global des items.

Exclus (exigence §9) : le manifeste lui-meme ; le rapport de gel qui contient son
hash ; les resultats qualite futurs ; les fichiers temporaires et logs non
normatifs ; les grands modeles ; le corpus long contexte non prepare.

-- serialisation --------------------------------------------------------
JSON, cles triees, separateurs compacts, ensure_ascii=false, sans saut de ligne
final. Les entrees sont ordonnees par chemin logique (ordre des octets).

  manifest_bytes = canonical_json({"schema","entries"})
  TASK_EVAL_CONTENT_TREE_SHA256 = sha256(manifest_bytes)

Aucune dependance circulaire : le manifeste ne contient jamais son propre hash.
"""
import hashlib
import json
import os
import sys

W = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "protocol-harness-01")
Q = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/engine-008-quality-v1")
FORK = "/home/triviere/projects/llama-obrain-cuda-f1a"

# (chemin logique, chemin physique)
ENTRIES = [
    # --- DATA TASK (autorite) ---
    ("data/task_eval/mmlu_pro/MMLU_PRO_ITEMS.jsonl",      Q + "/corpora/task_eval/mmlu_pro/MMLU_PRO_ITEMS.jsonl"),
    ("data/task_eval/mmlu_pro/MMLU_PRO_SELECTION.csv",    Q + "/corpora/task_eval/mmlu_pro/MMLU_PRO_SELECTION.csv"),
    ("data/task_eval/gsm8k/GSM8K_ITEMS.jsonl",            Q + "/corpora/task_eval/gsm8k/GSM8K_ITEMS.jsonl"),
    ("data/task_eval/gsm8k/GSM8K_SELECTION.csv",          Q + "/corpora/task_eval/gsm8k/GSM8K_SELECTION.csv"),
    ("data/task_eval/mbpp/MBPP_ITEMS.jsonl",              Q + "/corpora/task_eval/mbpp/MBPP_ITEMS.jsonl"),
    ("data/task_eval/mbpp/MBPP_SELECTION.csv",            Q + "/corpora/task_eval/mbpp/MBPP_SELECTION.csv"),
    ("data/task_eval/ifeval/IFEVAL_ITEMS.jsonl",          Q + "/corpora/task_eval/ifeval/IFEVAL_ITEMS.jsonl"),
    ("data/task_eval/ifeval/IFEVAL_SELECTION.csv",        Q + "/corpora/task_eval/ifeval/IFEVAL_SELECTION.csv"),
    # --- contrats de prompts et regles de selection ---
    ("contracts/TASK_PROMPT_CONTRACTS_v1.txt",            Q + "/TASK_PROMPT_CONTRACTS_v1.txt"),
    ("contracts/ENGINE008_SELECTION_RULES_v2.txt",        Q + "/ENGINE008_SELECTION_RULES_v2.txt"),
    ("contracts/QUALITY_THRESHOLDS_v1.txt",               Q + "/QUALITY_THRESHOLDS_v1.txt"),
    ("contracts/STRUCTURED_VALIDITY_CONFIG_v1.txt",       Q + "/STRUCTURED_VALIDITY_CONFIG_v1.txt"),
    # --- autorites des scorers ---
    ("scorers/QUALITY_SCORER_PROVENANCE.txt",             Q + "/QUALITY_SCORER_PROVENANCE.txt"),
    ("scorers/gsm8k_v1.py",                               Q + "/scorers/gsm8k_v1.py"),
    ("scorers/ifeval_v1.py",                              Q + "/scorers/ifeval_v1.py"),
    ("scorers/long_context_v1.py",                        Q + "/scorers/long_context_v1.py"),
    ("scorers/mbpp_v1.py",                                Q + "/scorers/mbpp_v1.py"),
    ("scorers/mmlu_pro_v1.py",                            Q + "/scorers/mmlu_pro_v1.py"),
    ("scorers/task_scorers_v1.py",                        Q + "/scorers/task_scorers_v1.py"),
    # --- sources de preparation des entrees ---
    ("prep/ENGINE008_EFFECTIVE_CHAT_TEMPLATE.jinja",      W + "/authority/ENGINE008_EFFECTIVE_CHAT_TEMPLATE.jinja"),
    ("prep/ENGINE008_RENDER_CONFIG_v1.json",              W + "/docs/ENGINE008_RENDER_CONFIG_v1.json"),
    ("prep/engine008-quality-eval.cpp",                   FORK + "/tools/engine008-quality-eval/engine008-quality-eval.cpp"),
    # --- entrees produites ---
    ("out/MMLU_CHOICES_INPUTS.jsonl",                     W + "/out/MMLU_CHOICES_INPUTS.jsonl"),
    ("out/TASK_GLOBAL_ITEM_ORDER.txt",                    W + "/out/TASK_GLOBAL_ITEM_ORDER.txt"),
    ("out/TASK_RENDERED_INPUTS.jsonl",                    W + "/out/TASK_RENDERED_INPUTS.jsonl"),
]


def sha256_file(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def main():
    entries = []
    missing = []
    for logical, physical in ENTRIES:
        if not os.path.exists(physical):
            missing.append((logical, physical))
            continue
        entries.append({"path": logical,
                        "bytes": os.path.getsize(physical),
                        "sha256": sha256_file(physical)})
    if missing:
        print("MANQUANTS :")
        for l, p in missing:
            print("  %-52s %s" % (l, p))
        return 2
    entries.sort(key=lambda e: e["path"].encode())
    manifest = {"schema": "ENGINE008_TASK_SCOPE_v1", "entries": entries}
    blob = json.dumps(manifest, sort_keys=True, separators=(",", ":"),
                      ensure_ascii=False).encode("utf-8")
    tree = hashlib.sha256(blob).hexdigest()

    out_manifest = os.path.join(W, "out", "ENGINE008_TASK_CANONICAL_MANIFEST_v1.json")
    with open(out_manifest, "wb") as f:
        f.write(blob)

    lines = []
    L = lines.append
    L("ENGINE-008 -- PERIMETRE CANONIQUE TASK v1")
    L("=" * 74)
    L("SCHEMA=ENGINE008_TASK_SCOPE_v1")
    L("DEFINITION=NOUVELLE_PAR_CETTE_PASSE (l ancienne n a jamais ete materialisee)")
    L("SERIALISATION=json, cles triees, separateurs compacts, ensure_ascii=false, sans saut de ligne final")
    L("ORDRE=chemin logique, ordre des octets")
    L("ENTRY_COUNT=%d" % len(entries))
    L("MANIFEST_BYTES=%d" % len(blob))
    L("MANIFEST_SHA256=%s" % hashlib.sha256(blob).hexdigest())
    L("")
    L("TASK_EVAL_CONTENT_TREE_SHA256=%s" % tree)
    L("")
    L("=== ENTREES ===")
    for e in entries:
        L("  %-52s %10d  %s" % (e["path"], e["bytes"], e["sha256"]))
    L("")
    L("=== HASHES CONSERVES SEPAREMENT (perimetres DIFFERENTS) ===")
    L("TASK_EVAL_DATA_CONTENT_TREE_SHA256=c30d71d84712fae390be582111f0473c87ccaaad7411e1ef99b97689d69ab84a")
    L("  -> perimetre DATA amont (corpora/task_eval), historique, INCHANGE")
    L("TASK_EVAL_CONTENT_TREE_SHA256=%s" % tree)
    L("  -> perimetre TASK COMPLET (ce manifeste)")
    L("  Les deux ne sont PAS comparables : perimetres differents.")
    L("")
    L("=== EXCLUSIONS DECIDEES (§9) ===")
    for x in ["le manifeste lui-meme",
              "le rapport de gel qui contient son hash",
              "les resultats qualite futurs",
              "les fichiers temporaires et logs non normatifs",
              "les grands modeles",
              "le corpus long contexte non prepare"]:
        L("  - %s" % x)
    L("")
    L("=== MODELES ET DEPENDANCES RUNTIME : manifestes de provenance SEPARES ===")
    L("  ENGINE008_RENDERER_PROVENANCE_v1.json  (fork, libs, ABI, chemin resolu)")
    L("  ENGINE008_Q1_RECOVERY.txt               (identite Q1 et son autorite)")
    L("  qualite_baseline_model_sha256=31629f53165ab6a7dad8c9847dcfd1fdf55829dac1e6e748f4a68581b0033d34")
    L("  qualite_candidate_model_sha256=1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937")
    L("  libllama_sha256=87c42d5f5dd7ab3c10967b4d3eaac82636b6af11e2c18a7f2798bab2d01bb2b2")
    L("  TASK_EVAL_CONTENT_TREE_SHA256 ne couvre PAS les modeles : ils sont exclus")
    L("  par decision, et lies par des manifestes de provenance separes.")

    txt = "\n".join(lines) + "\n"
    with open(os.path.join(W, "out", "TASK_SCOPE_MANIFEST_REPORT.txt"), "w",
              encoding="utf-8", newline="\n") as f:
        f.write(txt)
    print(txt)
    return 0


if __name__ == "__main__":
    sys.exit(main())
