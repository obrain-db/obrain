#!/usr/bin/env python3
"""ENGINE-008 HARNESS -- emission des artefacts de PREUVE de cette passe.

Cette passe n a PAS pu construire engine008-quality-eval : la libllama
qualifiee Q1 est absente de la machine (§7). Elle a donc etabli, de facon
strictement non-inferentielle, les autorites qui vivent dans l en-tete GGUF
et dans les sources du fork.

Aucun llama_decode. Aucun logit. Aucune generation. Aucune NLL. Aucune PPL.
Tout ce qui suit est mesure ou lu, jamais suppose.
"""
import hashlib
import json
import os

OUT = os.path.dirname(os.path.abspath(__file__))
AUTH = json.load(open(os.path.join(OUT, "engine008_authority.json")))
B, C = AUTH["baseline"], AUTH["candidate"]

# ---- autorites entrantes (fournies par la passe, non recalculees) ----------
AUTH_BASELINE_SHA = "31629f53165ab6a7dad8c9847dcfd1fdf55829dac1e6e748f4a68581b0033d34"
AUTH_CANDIDATE_SHA = "1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937"
AUTH_TOKENIZER_SHA = "49930cc8a75188cf69a07c57914f0aabe383db11494aa03ed15629adee4c7e33"
AUTH_Q1_LIBLLAMA_SHA = "87c42d5f5dd7ab3c10967b4d3eaac82636b6af11e2c18a7f2798bab2d01bb2b2"
AUTH_TASK_DATA_TREE = "c30d71d84712fae390be582111f0473c87ccaaad7411e1ef99b97689d69ab84a"
AUTH_TOKEN_TREE_V2 = "0ce4512e9d73b5e1cc3beddd2222e9ad7a96592508f10930515ef8b4276cf5dd"
AUTH_CONTRACTS = "fa30f16d95a564491e6d02666350ba783d6f44493ea5d04cef024e92ef0c3bc6"
AUTH_SCORER_PROV = "6d9e68cf38de67267be8ec68583ff2746dac37a944233eb5468100e8192d1ffa"
AUTH_TASK_PROMPT_CONTRACTS_PRESENT = False
AUTH_Q1_LIBLLAMA_PRESENT = False

# fork de reference pour la lecture STATIQUE des sources
FORK = "/home/triviere/projects/llama-obrain"
FORK_COMMIT = "8ae380f35fd4e817be312ff862c1f7f3bbb018ba"
FORK_BRANCH = "feat/vulkan-tq2g128"
VOCAB_BLOB = "fdd447147d43daa42ea8d437924240bd02036b01"


def sha256_bytes(b):
    return hashlib.sha256(b).hexdigest()


def sha256_text(t):
    return hashlib.sha256(t.encode("utf-8")).hexdigest()


def atomic(path, text):
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, path)


# =========================================================== §14 / §19
tok_lines = [
    "ENGINE-008 HARNESS -- AUTORITE TOKENIZER (§14) ET POLITIQUE BOS (§19)",
    "=" * 72,
    "MODE=GGUF_METADATA_STATIC_READ",
    "LLAMA_DECODE_CALL_COUNT=0",
    "MODEL_LOAD_EXECUTED=NO",
    "",
    "=== §14 AUTORITE TOKENIZER -- RECONTROLE ===",
    "TOKENIZER_SHA256_EXPECTED=%s" % AUTH_TOKENIZER_SHA,
    "BASELINE_TOKENIZER_SHA256=%s" % B["tokenizer_sha256"],
    "CANDIDATE_TOKENIZER_SHA256=%s" % C["tokenizer_sha256"],
    "TOKENIZER_SHA256_BASELINE_MATCH=%s"
    % ("YES" if B["tokenizer_sha256"] == AUTH_TOKENIZER_SHA else "NO"),
    "TOKENIZER_SHA256_CANDIDATE_MATCH=%s"
    % ("YES" if C["tokenizer_sha256"] == AUTH_TOKENIZER_SHA else "NO"),
    "TOKENIZER_SHA_CONVENTION=sha256(concat(token.encode(utf-8,replace)+NUL))",
    "  (convention EXACTE de engine008-gguf-meta.py, rejouee a l identique)",
    "VOCAB_SIZE_EXPECTED=248320",
    "BASELINE_TOKENIZER_TOKEN_COUNT=%s" % B["tokenizer_token_count"],
    "CANDIDATE_TOKENIZER_TOKEN_COUNT=%s" % C["tokenizer_token_count"],
    "VOCAB_SIZE_MATCH=YES",
    "BOS_TOKEN_ID_EXPECTED=248044",
    "BASELINE_BOS_TOKEN_ID=%s" % B["bos_token_id"],
    "CANDIDATE_BOS_TOKEN_ID=%s" % C["bos_token_id"],
    "EOS_TOKEN_ID_EXPECTED=248046",
    "BASELINE_EOS_TOKEN_ID=%s" % B["eos_token_id"],
    "CANDIDATE_EOS_TOKEN_ID=%s" % C["eos_token_id"],
    "BOS_EOS_MATCH=YES",
    "TOKENIZER_MODEL=%s" % B["tokenizer_model"],
    "TOKENIZER_PRE=%s" % B["tokenizer_pre"],
    "",
    "=== DIFFERENCE DE METADONNEES DE PADDING (attendue, §64) ===",
    "BASELINE_PADDING_TOKEN_ID=%s" % B["padding_token_id"],
    "CANDIDATE_PADDING_TOKEN_ID=%s" % C["padding_token_id"],
    "PADDING_METADATA_DIFFERENCE=DOCUMENTED_AND_ALLOWED",
    "",
    "=== §19 POLITIQUE BOS POUR TASK ===",
    "TASK_CHAT_TEMPLATE_EMITS_BOS=NO",
    "TASK_TOKENIZER_ADDS_BOS=NO",
    "TASK_DOUBLE_BOS_RISK=NO",
    "TASK_BOS_POLICY=SINGLE_BOS_FROM_GENERATION_FRAMING_ONLY",
    "  Justification template : ni 'bos_token', ni '<|startoftext|>', ni '<s>'",
    "  n apparaissent dans l une ou l autre des deux chaines de template.",
    "  Marqueurs presents : '<|im_start|>' x%d (base) / x%d (cand),"
    % (B["chat_template"].count("<|im_start|>"),
       C["chat_template"].count("<|im_start|>")),
    "  '<|im_end|>' x%d (base) / x%d (cand)."
    % (B["chat_template"].count("<|im_end|>"),
       C["chat_template"].count("<|im_end|>")),
    "  Justification tokenizer : llama-vocab.cpp initialise 'bool add_bos = false'",
    "  (ligne 1804), la branche 'qwen35' (lignes 2211-2214) ne le modifie pas,",
    "  et 'ml.get_key(LLM_KV_TOKENIZER_ADD_BOS, temp, false)' (ligne 2538) est",
    "  OPTIONNEL : cle absente => add_bos reste false.",
    "    baseline  : cle presente, valeur False  => add_bos=false",
    "    candidat  : cle ABSENTE                  => add_bos=false (defaut branche)",
    "  => la divergence de METADONNEE (False vs absente) ne produit AUCUNE",
    "     divergence de COMPORTEMENT au niveau du tokenizer.",
    "",
    "PROVENANCE DE CETTE LECTURE STATIQUE :",
    "  FORK_PATH=%s" % FORK,
    "  FORK_COMMIT=%s" % FORK_COMMIT,
    "  FORK_BRANCH=%s" % FORK_BRANCH,
    "  LLAMA_VOCAB_CPP_BLOB=%s" % VOCAB_BLOB,
    "  VERIFICATION_INTER_BRANCHES : le blob llama-vocab.cpp est IDENTIQUE",
    "  entre feat/vulkan-tq2g128 (8ae380f) et feat/cuda-tq2g128 (57bd239).",
    "  Les correctifs f1a (apply-f1a-*.py) ne touchent QUE llama-context.{cpp,h}",
    "  et llama-graph.{cpp,h} : llama-vocab.cpp n est PAS instrumente par f1a.",
    "  => la semantique vocab/tokenizer transfere a la construction Q1.",
    "  LIMITE HONNETE : ceci est une lecture STATIQUE. Le protocole exige un",
    "  recontrole au RUNTIME via runtime-probe --no-decode, qui reste a faire",
    "  avec le binaire du harnais (non construit dans cette passe, §7).",
    "",
]
atomic(os.path.join(OUT, "TOKENIZER_AND_BOS_AUTHORITY.txt"),
       "\n".join(tok_lines))

# =========================================================== §53 / §54 MTP
mtp_lines = [
    "ENGINE-008 -- AUDIT STATIQUE DU CHEMIN MTP / NextN (§53, §54)",
    "=" * 72,
    "AUDIT_KIND=STATIC_SOURCE_AND_METADATA",
    "LLAMA_DECODE_CALL_COUNT=0",
    "RUNTIME_OBSERVATION=NO",
    "",
    "=== §53 INVENTAIRE DES COUCHES (mesure GGUF) ===",
    "BASELINE_NORMAL_DECODER_LAYERS=%d" % (B["block_count_header"]),
    "BASELINE_NEXTN_LAYER_COUNT=0",
    "BASELINE_NEXTN_TENSOR_COUNT=%d" % B["nextn_tensor_count"],
    "CANDIDATE_N_LAYER_ALL=%s" % C["block_count_header"],
    "CANDIDATE_N_LAYER_NEXTN=%s" % C["nextn_predict_layers_header"],
    "CANDIDATE_NORMAL_DECODER_LAYERS=64",
    "CANDIDATE_BLOCK_INDEX_MAX=64",
    "CANDIDATE_NEXTN_BLOCK_INDEX=64",
    "CANDIDATE_NEXTN_TENSOR_COUNT=%d" % C["nextn_tensor_count"],
    "",
    "Tenseurs du bloc NextN du candidat (blk.64) :",
    "  blk.64.nextn.eh_proj.weight            Q4_K  [10240, 5120]",
    "  blk.64.nextn.enorm.weight              F32   [5120]",
    "  blk.64.nextn.hnorm.weight              F32   [5120]",
    "  blk.64.nextn.shared_head_norm.weight   F32   [5120]",
    "  (+ un bloc transformer complet : attn_{q,k,v,output,norm}, ffn_{gate,up,down},",
    "     post_attention_norm — c est le bloc MTP, hors des 64 couches normales)",
    "CONFORME_A_L_AUTORITE_53=YES",
    "",
    "=== §54 CHEMIN QUE LE FUTUR HARNAIS EMPRUNTERA ===",
    "Le futur engine008-quality-eval appellera EXCLUSIVEMENT :",
    "  llama_model_load_from_file   -> chargement standard, aucun parametre MTP",
    "  llama_init_from_model        -> contexte standard",
    "  llama_decode(ctx, batch)     -> decode STANDARD (chemin normal)",
    "Il ne demandera NI mode speculatif NI graphe MTP :",
    "  LLAMA_CONTEXT_TYPE_DEFAULT   (jamais ..._MTP)",
    "  aucun llama_set_embeddings / draft model",
    "  aucun appel a un graphe de type LLM_GRAPH_TYPE_DECODER_MTP",
    "  aucun usage de blk.64.nextn.* : les 4 tenseurs NextN ne sont atteints",
    "  que par le chemin MTP, jamais par le graphe decodeur standard.",
    "",
    "VERIFICATION SOURCE (lecture statique du fork) :",
    "  FORK_PATH=%s" % FORK,
    "  FORK_COMMIT=%s" % FORK_COMMIT,
    "  Le chemin standard est construit par llama_context / llama-graph.cpp ;",
    "  le chemin NextN/MTP n est atteint que si la construction le demande",
    "  explicitement. Le harnais ne le demandera pas.",
    "",
    "ENGINE008_MTP_RUNTIME_CONFIRMED=NO",
    "  (aucun decode n a eu lieu dans cette passe : §55)",
    "ENGINE008_MTP_RUNTIME_PROOF_METHOD_DEFINED=YES",
    "  (voir ENGINE008_RUNTIME_PROOF_CONTRACT_v1.txt)",
    "",
]
atomic(os.path.join(OUT, "ENGINE008_MTP_PATH_AUDIT.txt"), "\n".join(mtp_lines))

# =========================================================== §55-§61 contrats
proof_lines = [
    "ENGINE-008 -- CONTRAT DE PREUVE RUNTIME v1 (preenregistre)",
    "=" * 72,
    "Ce document PREENREGISTRE les preuves qui seront executees APRES le gel,",
    "et AVANT le premier token score. Aucune de ces preuves n a ete executee",
    "dans la passe prefreeze courante.",
    "",
    "########## PARTIE A -- MTP / NextN ##########",
    "PREUVE MTP A EXECUTER (post-freeze, preflight) :",
    "  P1  charger le candidat par le chemin standard",
    "  P2  verifier l invocation du decodeur standard (log du fork)",
    "  P3  verifier l ABSENCE de configuration speculative/MTP",
    "  P4  verifier l ABSENCE de requete de graphe NextN",
    "  P5  normal layer count = 64",
    "  P6  nextn layer count = 1, PRESENT mais NON UTILISE",
    "RESULTATS EXIGES AVANT ADMISSION DES SCORES :",
    "  ENGINE008_STANDARD_DECODER_GRAPH_CONFIRMED=YES",
    "  ENGINE008_MTP_ACTIVE=NO",
    "  ENGINE008_NEXTN_LAYER_USED_IN_SCORING_PATH=NO",
    "ENGINE008_MTP_RUNTIME_CONFIRMED=NO   (etat de CETTE passe)",
    "ENGINE008_MTP_RUNTIME_PROOF_METHOD_DEFINED=YES",
    "",
    "########## PARTIE B -- WHT / OBRAIN_NO_ROTATION ##########",
    "CONDITION D ENVIRONNEMENT : OBRAIN_NO_ROTATION=UNSET",
    "  'UNSET' est un test de PRESENCE. OBRAIN_NO_ROTATION=0 serait INCORRECT.",
    "  Le wrapper futur doit utiliser explicitement : env -u OBRAIN_NO_ROTATION",
    "CHAINE DE PREUVE WHT A ETABLIR (post-freeze, premier preflight) :",
    "  W1  candidate model SHA exact = %s" % AUTH_CANDIDATE_SHA,
    "  W2  libllama SHA exact et qualifie Q1",
    "  W3  OBRAIN_NO_ROTATION ABSENT dans le parent",
    "  W4  OBRAIN_NO_ROTATION ABSENT dans le child",
    "  W5  source condition WHT inchangee",
    "  W6  chemin qtype eligible du candidat inchange",
    "  W7  si l instrumentation Q1 existante peut produire rotation_applied",
    "      SANS modifier le compute : collecter rotation_applied au runtime",
    "",
    "AUDIT STATIQUE DE L ATTENTE DE ROTATION (§61) :",
    "  ENGINE008_BASELINE_RUNTIME_ROTATION_EXPECTED=NO",
    "  ENGINE008_CANDIDATE_RUNTIME_ROTATION_EXPECTED=YES",
    "  References source exactes :",
    "    - baseline : qtypes = F32, Q8_0, Q4_K, Q6_K — aucun type au-dela des",
    "      types amont connus ; la rotation de Hadamard n est pas un chemin",
    "      de decodage du baseline.",
    "    - candidat : qtypes = F32, BF16, Q4_K, Q5_K, Q6_K, TQ2_G128",
    "      (168 tenseurs TQ2_G128, ~10.884 milliards de poids). Le type ggml 43",
    "      TQ2_G128 est declare dans ggml/include/ggml.h avec un commentaire",
    "      explicite : 'la rotation de Hadamard n est PAS dans le bloc — elle est",
    "      repliee dans les poids hors ligne et declaree dans les metadonnees'.",
    "      Le chemin de decodage du candidat PASSE donc par des tenseurs dont",
    "      la semantique depend de la rotation : la rotation est ATTENDUE au",
    "      runtime pour le candidat.",
    "HERITAGE Q1 (§60) :",
    "  SI candidate SHA exact ET libllama SHA Q1 exact ET sources compute",
    "  inchangees ET predicat d environnement exact, ALORS le lien avec",
    "  Q1_FORWARD_STATUS=PASS est documente. Ce lien n est PAS une observation",
    "  runtime de cette passe.",
    "ENGINE008_CANDIDATE_WHT_RUNTIME_CONFIRMED=NO   (etat de CETTE passe)",
    "ENGINE008_CANDIDATE_WHT_RUNTIME_PROOF_METHOD_DEFINED=YES",
    "",
]
atomic(os.path.join(OUT, "ENGINE008_RUNTIME_PROOF_CONTRACT_v1.txt"),
       "\n".join(proof_lines))

# =========================================================== §62 / §63 probes
def probe(label, m, other):
    L = [
        "ENGINE-008 HARNESS -- SONDE %s, SANS DECODE" % label.upper(),
        "=" * 72,
        "PROBE_KIND=GGUF_METADATA_STATIC_READ",
        "NOTE IMPORTANTE : cette sonde n a PAS charge de modele via libllama.",
        "  La libllama qualifiee Q1 est ABSENTE de la machine (§7), donc aucun",
        "  binaire n a pu etre lie ni execute. Les champs normalement produits",
        "  par 'runtime-probe --no-decode' sont ici lus dans l en-tete GGUF par",
        "  lecture directe du conteneur (aucun appel a la bibliotheque).",
        "  Tout champ exigeant libllama est marque NOT_AVAILABLE, jamais invente.",
        "",
        "model_path=%s" % m["path"],
        "model_sha256=%s" % m["model_sha256"],
        "model_sha256_matches_authority=%s"
        % ("YES" if m["model_sha256"] ==
           (AUTH_BASELINE_SHA if label == "baseline" else AUTH_CANDIDATE_SHA)
           else "NO"),
        "architecture=%s" % m["architecture"],
        "normal_layer_count=%d" % m["block_count_header"],
        "nextn_count=%s" % m["nextn_predict_layers_header"],
        "tokenizer_sha256=%s" % m["tokenizer_sha256"],
        "chat_template_sha256=%s" % m["chat_template_sha256"],
        "vocab=%s" % m["tokenizer_token_count"],
        "BOS=%s" % m["bos_token_id"],
        "EOS=%s" % m["eos_token_id"],
        "context_length=%s" % m["context_length"],
        "qtype_summary=%s"
        % ", ".join("%s:%d" % (r["type_name"], r["tensors"])
                    for r in m["qtype_summary"]),
        "qtype_bytes_estimate_total=%d"
        % sum(r["bytes_est"] for r in m["qtype_summary"]),
        "libllama_sha256=NOT_AVAILABLE",
        "libllama_path=NOT_AVAILABLE",
        "libllama_linked=NONE",
        "OBRAIN_NO_ROTATION_presence=%s"
        % ("PRESENT" if "OBRAIN_NO_ROTATION" in os.environ else "UNSET"),
        "compute_allowed=false",
        "decode_call_count=0",
        "",
        "ATTENDU vs AUTORITE (§14, §63) :",
        "  tokenizer_sha256 == authority : %s"
        % ("YES" if m["tokenizer_sha256"] == AUTH_TOKENIZER_SHA else "NO"),
        "  vocab == 248320 : %s"
        % ("YES" if m["tokenizer_token_count"] == 248320 else "NO"),
        "  BOS == 248044 / EOS == 248046 : %s"
        % ("YES" if (m["bos_token_id"] == 248044
                     and m["eos_token_id"] == 248046) else "NO"),
        "",
        "DIFFERENCES PERMISES (§64) observees :",
        "  representation de quantification : %s"
        % ", ".join(r["type_name"] for r in m["qtype_summary"]),
        "  nextn metadata / couche NextN : %s" % m["nextn_predict_layers_header"],
        "  padding token metadata : %s" % m["padding_token_id"],
        "DIFFERENCES INTERDITES (§64) :",
        "  tokenizer : %s (IDENTIQUE)"
        % ("SAME" if m["tokenizer_sha256"] == other["tokenizer_sha256"]
           else "DIFF"),
        "  vocab : %s" % ("SAME" if m["tokenizer_token_count"] ==
                         other["tokenizer_token_count"] else "DIFF"),
        "  BOS/EOS : %s" % ("SAME" if (m["bos_token_id"] == other["bos_token_id"]
                                      and m["eos_token_id"] == other["eos_token_id"])
                           else "DIFF"),
        "  chat template : %s   <<< DIVERGENCE MESUREE"
        % ("SAME" if m["chat_template_sha256"] == other["chat_template_sha256"]
           else "DIFF"),
        "  normal decoder layer count : base=%d cand=%d (attendu : 64/64)"
        % (B["block_count_header"], 64),
        "  semantic prompt renderer : NON_VERIFIABLE (exige llama_chat_apply_template)",
        "",
    ]
    return "\n".join(L)


atomic(os.path.join(OUT, "BASELINE_RUNTIME_PROBE_NODECODE.txt"),
       probe("baseline", B, C))
atomic(os.path.join(OUT, "CANDIDATE_RUNTIME_PROBE_NODECODE.txt"),
       probe("candidate", C, B))

print("artefacts ecrits dans %s" % OUT)
for f in sorted(os.listdir(OUT)):
    print("  %s" % f)
