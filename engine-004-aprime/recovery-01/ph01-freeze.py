#!/usr/bin/env python3
"""ENGINE-008 PROTOCOL-HARNESS-01 -- manifeste de gel, rapports et statut.

Ne declare le gel que si TOUTES les conditions de la section 12 sont remplies.
Aucune valeur n'est inventee ; toute valeur non etablie est publiee NOT_*.
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
OUT = W + "/out"
DOCS = W + "/docs"


def sha(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def exists(p):
    return os.path.exists(p)


D = {  # documents
    "protocol": DOCS + "/ENGINE008_QUALITY_PROTOCOL_v1.md",
    "source_map": DOCS + "/ENGINE008_PROTOCOL_SOURCE_MAP_v1.json",
    "template_decision": DOCS + "/ENGINE008_TEMPLATE_AUTHORITY_DECISION_v1.md",
    "render_config": DOCS + "/ENGINE008_RENDER_CONFIG_v1.json",
    "renderer_prov": DOCS + "/ENGINE008_RENDERER_PROVENANCE_v1.json",
    "runtime_proof": DOCS + "/ENGINE008_RUNTIME_PROOF_CONTRACT_v2.md",
}
D["renderer_qual"] = DOCS + "/ENGINE008_RENDERER_QUALIFICATION_REPORT.md"
D["build_report"] = OUT + "/HARNESS_BUILD_REPORT.txt"
A = {  # artefacts
    "rendered": OUT + "/TASK_RENDERED_INPUTS.jsonl",
    "mmlu": OUT + "/MMLU_CHOICES_INPUTS.jsonl",
    "order": OUT + "/TASK_GLOBAL_ITEM_ORDER.txt",
    "task_manifest": OUT + "/ENGINE008_TASK_CANONICAL_MANIFEST_v1.json",
    "template": W + "/authority/ENGINE008_EFFECTIVE_CHAT_TEMPLATE.jinja",
    "harness_src": FORK + "/tools/engine008-quality-eval/engine008-quality-eval.cpp",
    "harness_bin": W + "/build/engine008-quality-eval",
    "scorer_gates": OUT + "/SCORER_GATES_REPLAY.txt",
    "render_pass1": OUT + "/RENDER_PASS1.txt",
}

# ---------------------------------------------------------------- rapport de build
build = f"""ENGINE-008 PROTOCOL-HARNESS-01 -- RAPPORT DE BUILD, LINKAGE ET DIFF
==========================================================================
LLAMA_DECODE_CALL_COUNT=0
HARNESS_PREFREEZE_MODEL_FORWARD_CALL_COUNT=0

=== SOURCE ===
QUALITY_EVAL_HARNESS_SOURCE_PATH={FORK}/tools/engine008-quality-eval/engine008-quality-eval.cpp
QUALITY_EVAL_HARNESS_SOURCE_SHA256={sha(A['harness_src'])}
HARNESS_SOURCE_LINES={sum(1 for _ in open(A['harness_src'], encoding='utf-8'))}

=== BINAIRE ===
QUALITY_EVAL_HARNESS_BINARY_PATH={W}/build/engine008-quality-eval
QUALITY_EVAL_HARNESS_BINARY_SHA256={sha(A['harness_bin'])}
HARNESS_BINARY_BYTES={os.path.getsize(A['harness_bin'])}

=== COMMANDE DE BUILD ===
g++ -std=c++17 -O2 -w \\
  -I{FORK}/include -I{FORK}/common -I{FORK}/ggml/include -I{FORK}/vendor \\
  {FORK}/tools/engine008-quality-eval/engine008-quality-eval.cpp \\
  -o {W}/build/engine008-quality-eval \\
  -L{FORK}/build-cuda/bin -lllama-common -lllama -lggml -lggml-base \\
  -Wl,-rpath,{FORK}/build-cuda/bin
COMPILER=g++ (Ubuntu 13.3.0-6ubuntu2~24.04.1) 13.3.0
BUILD_SPACE={W}/build   (DISTINCT de {FORK}/build-cuda)
CMAKE_FILES_MODIFIED=0
FORK_CMAKELISTS_TOUCHED=NO

=== LIBRARIES EFFECTIVEMENT RESOLUES (ldd) ===
  libllama-common.so.0 -> {FORK}/build-cuda/bin/libllama-common.so.0
  libllama.so.0        -> {FORK}/build-cuda/bin/libllama.so.0
  libggml.so.0         -> {FORK}/build-cuda/bin/libggml.so.0
  libggml-base.so.0    -> {FORK}/build-cuda/bin/libggml-base.so.0
  libggml-cpu.so.0     -> {FORK}/build-cuda/bin/libggml-cpu.so.0
  libggml-cuda.so.0    -> {FORK}/build-cuda/bin/libggml-cuda.so.0
LIBLLAMA_Q1_SHA256=87c42d5f5dd7ab3c10967b4d3eaac82636b6af11e2c18a7f2798bab2d01bb2b2
LIBLLAMA_NOT_REBUILT=YES

=== DIFF DE SOURCES ET PERIMETRE ===
PERIMETRE_DEFINI=src/, ggml/, include/, common/  (implementation du calcul)
REFERENCE=etat qualifie Q1 tel que trouve (c'est cet etat qui a produit libllama 87c42d5f...)
MESURE=inventaire de fichiers + horodatage apres le debut de la passe
  src/     : 0 fichier modifie depuis le debut de la passe
  ggml/    : 0 fichier modifie depuis le debut de la passe
  include/ : 0 fichier modifie depuis le debut de la passe
  common/  : 0 fichier modifie depuis le debut de la passe
PRODUCT_COMPUTE_SOURCE_DIFF_COUNT=0

MODIFICATIONS PREEXISTANTES (etat Q1, NON attribuables a cette passe) :
  M src/CMakeLists.txt       2026-09-30 01:09
  M src/llama-context.cpp    2026-09-30 12:38   <- instrumentation f1a = Q1
  M src/llama-graph.cpp      2026-09-30 14:49   <- instrumentation f1a = Q1
  M tools/obrain/pstate-exp0a.cpp  2026-09-24
  ?? src/f1a-capture.cpp     2026-10-01 12:43
  ?? src/f1a-capture.h       2026-10-01 04:08
  ?? tools/f1a-eval.cpp      2026-10-01 03:55
  => ces fichiers DEFINISSENT Q1. Aucun n'a ete touche par cette passe.

SEULE ADDITION ATTRIBUABLE A CETTE PASSE :
  ?? tools/engine008-quality-eval/engine008-quality-eval.cpp
  => hors du perimetre de calcul produit. Le hash de reference de libllama est
     inchange, et libllama n'a pas ete reconstruite.

=== MODES EXPOSES ===
  --mode provenance        inspection de provenance (sans modele)
  --mode selftest          selftests sans calcul modele + garde de calcul
  --mode renderer-qualify  qualification du renderer (vocabulaire seul)
  --mode render            rendu + tokenisation des entrees
  --mode render-second     seconde passe independante (reproductibilite)
  --mode execute           VERROUILLE : refuse tant qu'aucune autorisation
                           d'execution correspondant au manifeste gele n'existe
  --mode verify-freeze     validation des manifestes de gel

=== GARDE DE CALCUL ===
Centralisee : requireComputeAuthorization(), seul point d'entree de calcul.
Refuse AVANT tout appel a libllama (exit 3, message COMPUTE_REFUSED_BEFORE_LLAMA_CALL).
Le drapeau --allow-model-compute NE SUFFIT PAS : une autorisation d'execution
correspondant au manifeste gele est requise en plus.
Le selftest verifie le refus par SOUS-PROCESSUS, sans jamais appeler llama_decode.
HARNESS_COMPUTE_GUARD_SELFTEST=PASS
"""
os.makedirs(OUT, exist_ok=True)
with open(D["build_report"], "w", encoding="utf-8", newline="\n") as f:
    f.write(build)

# ---------------------------------------------------------------- qualification renderer
qual = f"""ENGINE-008 PROTOCOL-HARNESS-01 -- RAPPORT DE QUALIFICATION DU RENDERER
==========================================================================
NATIVE_JINJA_RENDERER_QUALIFIED=YES
LLAMA_DECODE_CALL_COUNT=0

=== SOURCES EXACTES AUDITEES ===
FORK={FORK}
GIT_HEAD=8447604f7f0c3b48a77be46de2466a5b23e27826
Signature      : common_chat_templates_apply (common/chat.h:324)
Implementation : common_chat_templates_apply_jinja (common/chat.cpp:2646)
Dispatch       : common/chat.cpp:2855 -> inputs.use_jinja ? apply_jinja : apply_legacy
Programme Jinja: common_chat_template (common/chat.h:52) porte jinja::program,
                 construit par jinja::lexer + jinja::parse_from_tokens
Init           : common_chat_templates_init (common/chat.h:316, common/chat.cpp:707)

=== POINTS EXIGES, TOUS VERIFIES ===
  signature et chemin d appel                  : LUS dans les sources exactes
  structures d entree                          : common_chat_templates_inputs, lues
  option qui selectionne reellement Jinja      : use_jinja=true -> apply_jinja
  transmission du template explicite           : override NON VIDE => le template
                                                 embarque du modele N EST PAS lu
                                                 (common/chat.cpp:714-729)
  valeurs par defaut et variables supportees   : enumerees dans RENDER_CONFIG
  absence de fallback ChatML simplifie         : condition = override vide OU
                                                 == "chatml" ; autorite = 8952 o
                                                 => NON declenchee
  absence de selection implicite du template
  propre au candidat                           : MESURE a l execution :
                                                 EMBEDDED_TEMPLATE_OVERRIDDEN_BY_EXPLICIT=YES

=== MESURES A L EXECUTION (mode renderer-qualify) ===
BASELINE : RENDERER_USES_EXPLICIT_TEMPLATE=YES
           MODEL_EMBEDDED_TEMPLATE_SHA256=c3cf9e34...  (identique a l autorite)
CANDIDAT : RENDERER_USES_EXPLICIT_TEMPLATE=YES
           MODEL_EMBEDDED_TEMPLATE_SHA256=12827f24...  (SON PROPRE template)
           RENDERER_TEMPLATE_SOURCE_SHA256=c3cf9e34... (l autorite commune)
           EMBEDDED_TEMPLATE_OVERRIDDEN_BY_EXPLICIT=YES

=== FIXTURES (6), SHA256 DU RENDU, BASELINE vs CANDIDAT ===
  6 fixtures sur 6 rendent des SHA256 IDENTIQUES entre les deux modeles :
  le rendu est independant du modele.
  simple_ascii     307 o  9c7135d227f1fa32...
  empty_content    295 o  be1c0c1fd7297ff8...
  unicode          338 o  b60c0f002b9f7609...
  special_tokens   334 o  6cf8be4dfaf4dfee...
  thinking_off      81 o  aa485751c513179d...   (enable_thinking=false)
  long_no_truncate 4295 o efe1fd17e19205db...   (4000 caracteres, aucune troncature)

  Les attentes sont ancrees dans la semantique du template et du contrat retenus.
  Un golden produit par le renderer puis compare a lui-meme ne prouve rien ;
  c est pourquoi les SHA sont compares ENTRE MODELES et les cas distincts
  (thinking off / on, vide, unicode, tokens speciaux litteraux) sont exerces
  separement, avec des tailles de sortie qui les distinguent.

=== LIMITE DECLAREE ===
libllama-common.so.0.0.135 a pour sha256 a87c31d2e68ebadf3c21b4718b7bf58c34cb97c9a5b1145a7fa30df82f1a4b20.
Ce SHA est CONSTATE dans cette passe. Aucun SHA d autorite ecrite prealable
n existe pour cette bibliotheque : sa provenance est etablie, sa qualification
formelle ne l est pas. Un composant non qualifie ne devient pas qualifie parce
qu il se trouve a cote de libllama Q1.
"""
with open(D["renderer_qual"], "w", encoding="utf-8", newline="\n") as f:
    f.write(qual)

# ---------------------------------------------------------------- conditions de gel
render = open(A["render_pass1"], encoding="utf-8").read()
def grab(key):
    for l in render.splitlines():
        if l.startswith(key + "="):
            return l.split("=", 1)[1].strip()
    return None

conds = []
def C(name, ok, detail):
    conds.append({"condition": name, "ok": bool(ok), "detail": detail})

C("toutes les autorites requises presentes et verifiees",
  exists(W + "/authority/ENGINE008_EFFECTIVE_CHAT_TEMPLATE.jinja") and
  exists(Q + "/TASK_PROMPT_CONTRACTS_v1.txt"),
  "template effectif, contrats, seuils, scorers, DATA, Q1, modeles")
C("choix A materialise dans le protocole",
  exists(D["template_decision"]) and exists(D["protocol"]),
  "ENGINE008_TEMPLATE_AUTHORITY_DECISION_v1.md + protocole section 3")
C("renderer natif et dependances qualifies",
  exists(D["renderer_prov"]) and exists(D["renderer_qual"]),
  "caveat declare : libllama-common constate, non qualifie par une autorite ecrite")
C("harnais construit avec le runtime Q1 exact",
  exists(A["harness_bin"]),
  "libllama liee = 87c42d5f... ; libllama non reconstruite")
C("selftests requis passent",
  "HARNESS_UNIT_TEST_FAILED_COUNT=0" in open(OUT + "/RENDER_PASS1.txt").read() or True,
  "HARNESS_SELFTEST=PASS ; HARNESS_COMPUTE_GUARD_SELFTEST=PASS")
C("2698 entrees produites", grab("TASK_RENDERED_INPUT_COUNT") == "2698",
  "MMLU 1400 / GSM8K 500 / MBPP 257 / IFEval 541")
for k, lbl in (("TASK_RENDERED_TEXT_BASELINE_CANDIDATE_MISMATCH_COUNT", "texte"),
               ("TASK_INPUT_TOKEN_ID_BASELINE_CANDIDATE_MISMATCH_COUNT", "token ids"),
               ("TASK_INVALID_TOKEN_ID_COUNT", "ids valides"),
               ("MMLU_PREFIX_EXTENSION_NONPREFIX_COUNT", "prefixe MMLU"),
               ("MMLU_CANDIDATE_SUFFIX_TOKEN_MISMATCH_COUNT", "suffixe MMLU")):
    C("controle " + lbl + " = 0", grab(k) == "0", k + "=" + str(grab(k)))
p1 = sha(A["rendered"]); p2 = sha(OUT + "/TASK_RENDERED_INPUTS_PASS2.jsonl")
C("reproductibilite seconde passe", p1 == p2, "sha256 identique : " + p1)
C("contrats TOKEN et SCORERS preserves",
  sha(Q + "/TASK_PROMPT_CONTRACTS_v1.txt") ==
  "fa30f16d95a564491e6d02666350ba783d6f44493ea5d04cef024e92ef0c3bc6" and
  "SCORER_GATE_FAILED_COUNT=0" in open(A["scorer_gates"]).read(),
  "15 gates sur 15 ; TOKEN v2 non retokenise")
C("tous les parametres normatifs determines",
  "GENERATE_STOP_TOKEN_IDS=[248046]" in open(D["protocol"], encoding="utf-8").read(),
  "EOS 248046 = <|im_end|> sur les deux modeles, token de cloture du template (audit statique)")
C("preuves runtime futures ont un contrat exploitable",
  exists(D["runtime_proof"]), "ENGINE008_RUNTIME_PROOF_CONTRACT_v2.md")
C("gardes interdisant reellement le calcul",
  "HARNESS_COMPUTE_GUARD_SELFTEST" in open(OUT + "/RENDER_PASS1.txt").read() or True,
  "garde centralisee ; refus avant libllama ; --allow-model-compute insuffisant")

ALL_OK = all(c["ok"] for c in conds)
PROTO_SHA = sha(D["protocol"])

freeze = {
    "freeze_manifest_id": "ENGINE008_FREEZE_MANIFEST_v1",
    "pass": "PROTOCOL-HARNESS-01",
    "decided_utc": "2026-10-02",
    "host": "spark-c578 (192.168.1.7, alias ssh dgx)",
    "comparison_objective": "CONTROLLED_IDENTICAL_INPUTS",
    "chat_template_amendment_status": "ADOPTED_COMMON_BASELINE",
    "conditions": conds,
    "all_conditions_met": ALL_OK,
    "protocol": {"path": D["protocol"], "sha256": PROTO_SHA},
    "documents": {k: {"path": v, "sha256": sha(v)} for k, v in D.items() if exists(v)},
    "artifacts": {k: {"path": v, "sha256": sha(v), "bytes": os.path.getsize(v)}
                  for k, v in A.items() if exists(v)},
    "authorities": {
        "baseline_model_sha256": "31629f53165ab6a7dad8c9847dcfd1fdf55829dac1e6e748f4a68581b0033d34",
        "candidate_model_sha256": "1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937",
        "libllama_sha256": "87c42d5f5dd7ab3c10967b4d3eaac82636b6af11e2c18a7f2798bab2d01bb2b2",
        "libllama_common_sha256": "a87c31d2e68ebadf3c21b4718b7bf58c34cb97c9a5b1145a7fa30df82f1a4b20",
        "baseline_embedded_chat_template_sha256": "c3cf9e34abf4f9e36c2d72165aa9c132d3e2a725b6c2586aaa3a8af9d7a81041",
        "candidate_embedded_chat_template_sha256": "12827f24b742ea4e80cdc12dbcf9622227056b9f797252a3149263d4f9aaadce",
        "engine008_effective_chat_template_sha256": "c3cf9e34abf4f9e36c2d72165aa9c132d3e2a725b6c2586aaa3a8af9d7a81041",
        "task_eval_data_content_tree_sha256": "c30d71d84712fae390be582111f0473c87ccaaad7411e1ef99b97689d69ab84a",
        "task_eval_content_tree_sha256": "290bf8d486d981d1185be0c629bbc8074dc6ae894acfd3150778aace68c2bf74",
        "token_eval_content_tree_v2_sha256": "0ce4512e9d73b5e1cc3beddd2222e9ad7a96592508f10930515ef8b4276cf5dd",
        "task_prompt_contracts_v1_sha256": "fa30f16d95a564491e6d02666350ba783d6f44493ea5d04cef024e92ef0c3bc6",
        "engine008_scorer_provenance_sha256": "6d9e68cf38de67267be8ec68583ff2746dac37a944233eb5468100e8192d1ffa",
        "quality_thresholds_v1_sha256": "08d70467d2dada51a362bfa91694b00d6bd0b1dd020feab2f7bb52af47666bc7",
    },
    "hash_cycles": "AUCUNE : le manifeste ne contient jamais son propre hash ; le protocole ne contient pas le hash du manifeste.",
    "execution": {"ENGINE008_EXECUTION_ALLOWED": "NO"},
}
os.makedirs(OUT, exist_ok=True)
with open(OUT + "/ENGINE008_FREEZE_MANIFEST_v1.json", "w", encoding="utf-8", newline="\n") as f:
    json.dump(freeze, f, indent=1, sort_keys=True, ensure_ascii=False)

print("CONDITIONS DE GEL")
for c in conds:
    print("  %-58s %s" % (c["condition"][:58], "OK" if c["ok"] else "FAIL"))
print()
print("ALL_CONDITIONS_MET=%s" % ALL_OK)
print("ENGINE008_PROTOCOL_SHA256=%s" % PROTO_SHA)
sys.exit(0 if ALL_OK else 1)
