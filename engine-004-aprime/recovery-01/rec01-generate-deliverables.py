#!/usr/bin/env python3
"""ENGINE-008 RECOVERY-01 -- generateur des livrables.

Toutes les valeurs publiees ici sont MESUREES dans cette passe, ou lues dans
un document autoritaire nomme. Aucune n est estimee.
"""
import hashlib
import json
import os

HERE = os.path.dirname(os.path.abspath(__file__))
CKPT = os.path.join(HERE, "checkpoint")

DGX = "spark-c578 (192.168.1.7, alias ssh 'dgx')"
Q = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/engine-008-quality-v1")
GQ1 = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
       "qualification/e7b-f1a-q1-forward-v1")
FORK = "/home/triviere/projects/llama-obrain-cuda-f1a"
BIN = FORK + "/build-cuda/bin"

SHA = {
    "libllama": "87c42d5f5dd7ab3c10967b4d3eaac82636b6af11e2c18a7f2798bab2d01bb2b2",
    "llama_tokenize": "edacdf0d7bcf933bf9a14df1f305197d11860d00f6d8c60488c546e0aac1a61f",
    "f1a_eval": "79ba1c449864da8be4b9fa53313a179c2ddad96bac1e814d77f470a8f2e7b80a",
    "libllama_common": "a87c31d2e68ebadf3c21b4718b7bf58c34cb97c9a5b1145a7fa30df82f1a4b20",
    "baseline": "31629f53165ab6a7dad8c9847dcfd1fdf55829dac1e6e748f4a68581b0033d34",
    "candidate": "1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937",
    "task_tree": "c30d71d84712fae390be582111f0473c87ccaaad7411e1ef99b97689d69ab84a",
    "token_tree_v2": "0ce4512e9d73b5e1cc3beddd2222e9ad7a96592508f10930515ef8b4276cf5dd",
    "contracts": "fa30f16d95a564491e6d02666350ba783d6f44493ea5d04cef024e92ef0c3bc6",
    "scorer_prov": "6d9e68cf38de67267be8ec68583ff2746dac37a944233eb5468100e8192d1ffa",
    "rules_v2": "3c1a16d7053f693400b3a8d858733f6a884301e34c8796e8a482370d5a369552",
    "q1_protocol": "55b69719fc78b1d71d66a390d8db7a792bafef003296156bb0fe2c6e1684a8b5",
    "tpl_base": "c3cf9e34abf4f9e36c2d72165aa9c132d3e2a725b6c2586aaa3a8af9d7a81041",
    "tpl_cand": "12827f24b742ea4e80cdc12dbcf9622227056b9f797252a3149263d4f9aaadce",
    "thresholds": "08d70467d2dada51a362bfa91694b00d6bd0b1dd020feab2f7bb52af47666bc7",
    "grec": "924447ead4bc2fa76e02d6e0c1135babea68f79a27dc7b32609d02861a95d72c",
    "ge5c": "4e0a2472937404995cba50292b2f2977157ac20398b95bc893a09db78445b398",
}

Q1_COMMIT = "8447604f7f0c3b48a77be46de2466a5b23e27826"
Q1_BRANCH = "feat/cuda-tq2g128"
Q1_RELPATH = "libllama.so.0"

WRITTEN = []


def W(name, text):
    p = os.path.join(HERE, name)
    tmp = p + ".tmp"
    with open(tmp, "w", encoding="utf-8", newline="\n") as f:
        f.write(text if text.endswith("\n") else text + "\n")
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, p)
    WRITTEN.append(name)
    return p


# ============================================================ §3 MIRROR
W("ENGINE008_MIRROR_RECOVERY.txt", f"""ENGINE-008 RECOVERY-01 -- §3 RECUPERATION DEPUIS LES SOURCES DOCUMENTEES
==========================================================================
HOTE_D_EXECUTION=n3 (cette machine)
DATE_UTC=2026-10-02

=== SOURCES DE RECUPERATION EXAMINEES ===

1. HOTE n3 -- emplacements examines
   /home/triviere/exp-artifacts                      ABSENT
   /mnt/obrain/**                                    ABSENT
   couches overlay Docker                            ABSENT
   archives du disque (7-15 sept.)                   NE CONTIENNENT PAS la passe (1-2 oct.)
   git banc-obrain (engine-004-aprime non suivi)      AUCUN HISTORIQUE
   RESULTAT=n3_ne_porte_pas_l_arbre_ENGINE-008
   La recherche exhaustive anterieure n a pas ete refaite : aucun indice nouveau.

2. HOTE dgx = {DGX}
   ACCES=ssh alias 'dgx', cle ~/.ssh/id_ed25519, SHELL COMPLET
   RESULTAT=SUCCES COMPLET -- l arbre ENGINE-008 entier est present.
   Racine={Q}
   SOURCE_DE_RECUPERATION_RETENUE=OUI

3. HOTE n1 (192.168.1.101, alias ssh 'n1', shell complet)
   /home/triviere/exp-artifacts                      ABSENT
   /home/triviere/projects/llama-obrain              present, sans fork f1a
   RESULTAT=pas de copie ENGINE-008

4. MIROIRS rrsync (acces RESTREINT, commande forcee, LECTURE SEULE)
   n1-miroir (192.168.1.101) racine exposee :
     bundle_test.js, page_test.html, start_dragon.sh, test-ws.js, test-ws.py,
     dragon-3d/, llama-obrain/ (25 juillet), snake-jotai/
   n2-miroir (192.168.1.102) racine exposee :
     aletheia/, llama-obrain/
   RESULTAT=les miroirs ne portent AUCUN artefact ENGINE-008.
     Leur 'llama-obrain' date du 25 juillet : ANTERIEUR au fork f1a.
   PROTOCOLE RESPECTE=lecture seule via rsync --list-only. Aucun upload,
     aucun --delete, aucune modification distante, aucune sortie de la racine
     exposee, aucune exposition de cle, aucune tentative de contourner la
     commande forcee.
   L ACCES SANS SHELL N A PAS ETE TRADUIT EN IRRECUPERABILITE : il a ete
     teste, et il s avere que la racine utile est simplement absente.

5. n2 en shell direct : l alias 'n2' ne resout pas (echec DNS local).
   Non requis : l arbre est sur le dgx.

=== CONSTAT METHODOLOGIQUE IMPORTANT (CORRECTION DE LA PASSE PRECEDENTE) ===
La passe precedente concluait « arbre supprime de la machine ». Ce constat
etait EXACT pour n3 et FAUX comme affirmation globale. L arbre n a jamais ete
supprime : il n a jamais reside sur n3. La passe HARNESS s executait sur n3,
la passe DATA/SCORERS s executait sur le dgx.
La clause §2 de cette passe avait nomme exactement ce piege : « suppression
globale, qui n est pas prouvee par la seule absence locale ». Les faits la
confirment. La correction est consignee, pas attenuee.

=== RECUPERATION EFFECTUEE ===
Aucune copie de masse. Le checkpoint local (n3) porte les fichiers CRITIQUES
et LEGERS (56 fichiers, 3,5 Mo) sous recovery-01/checkpoint/, verifies par
re-hachage sur n3.
Les jeux de donnees volumineux (external/) et les MODELES restent sur le dgx :
§8 interdit de dupliquer les grands modeles ; leurs chemins resolus, tailles
et SHA sont consignes dans le manifeste.
""")

# ============================================================ §4 Q1
W("ENGINE008_Q1_RECOVERY.txt", f"""ENGINE-008 RECOVERY-01 -- §4 RESTAURATION DE LA LIBLLAMA Q1
==========================================================================
LLAMA_DECODE_CALL_COUNT=0   (aucun decode, aucun forward pass dans cette passe)

=== 1. IDENTITE DU BINAIRE ===
QUALITY_EVAL_LIBLLAMA_PATH={BIN}/{Q1_RELPATH}
QUALITY_EVAL_LIBLLAMA_REALPATH={BIN}/libllama.so.0.0.135
QUALITY_EVAL_LIBLLAMA_SHA256={SHA['libllama']}
Q1_QUALITY_LIBLLAMA_SHA_MATCH=YES
  Mesure sur le dgx : sha256sum = valeur attendue, EGALITE EXACTE.

=== 2. RECOUPEMENT AVEC L AUTORITE Q1 D ORIGINE (exige par §1) ===
Document autoritaire retrouve :
  {GQ1}/Q1_PREEXECUTION_BUILD_PROVENANCE.txt
Extrait textuel :
  LIBLLAMA_PATH={BIN}/libllama.so.0.0.135
  LIBLLAMA_SHA256={SHA['libllama']}
  F1A_EVAL_LINKED_LIBLLAMA_SHA256={SHA['libllama']}
  F1A_EVAL_LINKS_INTENDED_Q1_LIBLLAMA=YES
  Q1_SOURCE_CHANGED_SINCE_Q0_RUN2=NO
  PRODUCT_COMPUTE_SOURCE_DIFF_COUNT=0
Q1_REFERENCE_AUTHORITY_RECOVERED=YES
AUCUNE_CONTRADICTION_ENTRE_AUTORITES=CONFIRME
  Le SHA de continuite de fil et l autorite d origine CONCORDENT. Aucune
  resolution silencieuse n a ete necessaire.

=== 3. OUTIL DE TOKENISATION QUALIFIE ===
NATIVE_TOKENIZER_TOOL_PATH={BIN}/llama-tokenize
NATIVE_TOKENIZER_TOOL_SHA256={SHA['llama_tokenize']}
NATIVE_TOKENIZER_TOOL_SHA_MATCH=YES
  Concordance avec la valeur enregistree dans ENGINE008_DATA_STATUS.txt.

=== 4. BINAIRE DE QUALIFICATION F1A ===
F1A_EVAL_PATH={BIN}/f1a-eval
F1A_EVAL_SHA256={SHA['f1a_eval']}
F1A_EVAL_SHA_MATCH=YES   (concordance avec Q1_PREEXECUTION_BUILD_PROVENANCE.txt)

=== 5. SOURCES ET HEADERS CORRESPONDANTS ===
Q1_FORK_PATH={FORK}
Q1_FORK_GIT_HEAD={Q1_COMMIT}
Q1_FORK_GIT_BRANCH={Q1_BRANCH}
  Arbre CUDA complet, sources et headers inclus (include/llama.h,
  src/llama-chat.cpp, common/chat.cpp, common/jinja/*).

=== 6. DISCRIMINATION DE PROVENANCE (pourquoi build-cuda, et lui seul) ===
Le MEME fork contient plusieurs constructions de libllama aux SHA DIFFERENTS.
Seule build-cuda correspond a Q1 :
  build-cuda/bin/libllama.so.0        {SHA['libllama']}   <-- Q1, RETENU
  build-r2-base/bin/libllama.so.0     {SHA['grec']}
  build-e5c/bin/libllama.so.0         {SHA['ge5c']}
  build-e4-observe, build-e5i, build-r2-v1 : autres constructions
=> Aucune lib Vulkan, HIP, CPU, DGX ou amont ne remplace Q1 implicitement.
   Aucun rebuild n a ete tente. Aucun hash de reference n a ete modifie.

=== 7. DEPENDANCES PRESCRITES ===
Bibliotheques presentes dans {BIN} :
  libllama.so.0.0.135           {SHA['libllama']}
  libllama-common.so.0.0.135    {SHA['libllama_common']}
  libggml.so.0.16.0             (SHA non releve dans cette passe)
  libggml-base.so.0.16.0        (idem)
  libggml-cpu.so.0.16.0         (idem)
  libggml-cuda.so.0.16.0        (idem)
  libmtmd.so.0.0.135            (idem)
Q1_QUALIFIED_DEPENDENCIES_AVAILABLE=YES
  AU SENS DE LA DISPONIBILITE : les bibliotheques requises sont presentes.
LIMITE DECLAREE, JAMAIS MASQUEE :
  L autorite Q1 n enregistre un SHA que pour f1a-eval et libllama. Aucun SHA
  d autorite n existe pour libllama-common, libggml*, libmtmd. Leur identite
  est CONSTATEE, pas QUALIFIEE. Les valeurs ci-dessus sont des constats de
  cette passe, pas des autorites.
  Faire passer ces constats au rang d autorite exigerait une qualification
  separee, NON executee ici.

=== 8. ENVIRONNEMENT D EXECUTION ===
Q1_RUNTIME_ENVIRONMENT_AVAILABLE=YES
  Le binaire est present et ses dependances sont resolues a cote de lui, sur
  le dgx. La qualification Q1 a effectivement tourne sur cet hote (artefacts
  Q1 ON/OFF avec logits et preuves runtime).
NON VERIFIE DANS CETTE PASSE : l execution effective du binaire. Aucun decode
  n est autorise par RECOVERY-01. L absence de nvcc sur n3 est SANS EFFET :
  on ne reconstruit rien. Elle n autorise ni changement de backend ni rebuild
  declare equivalent (§7).

=== 9. PREUVE WHT RUNTIME Q1 (retrouvee -- fait historique, PAS une preuve nouvelle) ===
{GQ1}/on/WHT_RUNTIME_PROOF_AUDIT.txt
  PREUVE A (environnement) : OBRAIN_NO_ROTATION_PRESENT_IN_PARENT_ENV=NO
    OBRAIN_NO_ROTATION_PRESENT_IN_CHILD_ENV=NO ; Q1_OBRAIN_NO_ROTATION_STATE=UNSET
    (verifie par env -u + test ${{VAR+x}} : UNSET n est pas SET_BUT_EMPTY)
  PREUVE B (runtime, sur le VRAI run qualificatif) :
    Q1_ROTATION_APPLIED_TRUE_COUNT=672
    Q1_ROTATION_APPLIED_FALSE_COUNT=0
    Q1_ROTATION_APPLIED_MISSING_COUNT=0
  PREUVE C (controle croise independant) : logits Q0 (rotation desactivee) et
    logits Q1 (rotation active) sont DIFFERENTS.
  Q1_WHT_RUNTIME_PROOF=PASS ; Q1_WHT_ACTIVE_CONFIRMED=YES
  Q1_WHT_ENVIRONMENT_CONTRACT=OBRAIN_NO_ROTATION MUST BE UNSET (lancement par env -u)
PORTEE, SANS SURINTERPRETATION :
  Cette preuve a ete produite sur le run qualificatif Q1 de f1a, avec le
  modele de ce run. Elle n est PAS une preuve runtime pour le candidat
  ENGINE-008. Elle fournit la METHODE et le precedent de reference ; la
  collecte runtime pour le candidat reste a faire apres gel.

=== 10. TEXTE AUTORITAIRE DU PROTOCOLE Q1 ===
{GQ1}/Q1_FORWARD_PROTOCOL_v1.md
  SHA256={SHA['q1_protocol']}
  VERIFICATION=egale a son .sha256 et a la valeur enregistree dans
  Q1_FORWARD_STATUS.txt. Triple concordance.
Q1_FORWARD_PROTOCOL_FROZEN=YES ; Q1_FORWARD_STATUS=PASS ; FIRST_Q1_BLOCKING_GATE=NONE
CONDITION WHT SOURCE (Q1_FORWARD_PROTOCOL_v1.md) :
  WHT_ENABLE_CONDITION_SOURCE = w->type == GGML_TYPE_TQ2_G128 AND getenv("OBRAIN_NO_ROTATION") == nullptr
  WHT_DISABLE_CONDITION_SOURCE= w->type != GGML_TYPE_TQ2_G128 OR  getenv("OBRAIN_NO_ROTATION") != nullptr
""")

# ============================================================ §5 DATA / SCORERS
W("ENGINE008_DATA_SCORERS_RECOVERY.txt", f"""ENGINE-008 RECOVERY-01 -- §5 RESTAURATION DATA, SCORERS, SEUILS
==========================================================================
LLAMA_DECODE_CALL_COUNT=0
LECTURE_SEULE_SUR_L_ARBRE_D_AUTORITE=OUI (aucune ecriture dans l arbre gele)

=== 1. LES DEUX DEFINITIONS DE TREE HASH ONT ETE RETROUVEES, PAS INVENTEES ===
TASK_EVAL_DATA_CONTENT_TREE_SHA256 :
  definition = tree_hash() de e8s9-final.py / engine008-corpora-task.py
    parcours sorted(os.walk(root)) ; pour chaque fichier : relpath + NUL +
    contenu + NUL ; racine = corpora/task_eval
TOKEN_EVAL_CONTENT_TREE_V2_SHA256 :
  definition = construction STRUCTUREE de e8-s8.py section S8.3 -- ce n est
  PAS un parcours d arborescence. Elle enchainne :
    b"RULES_V2\\x00" + octets de ENGINE008_SELECTION_RULES_v2.txt
    b"\\x00TOKENIZER\\x00" + b"49930cc8..."
    b"\\x00TOOL\\x00" + hex(sha256(llama-tokenize Q1))
    b"\\x00BOS=NO\\x00EOS=NO\\x00PARSE_SPECIAL=NO\\x00NO_ESCAPE=YES"
    par unite : b"\\x00UNIT\\x00" + group|item_id|content_sha|ids_csv
    par fichier : b"\\x00FILE\\x00" + nom + NUL + octets
  Aucune nouvelle fonction de hash n a ete inventee pour annoncer une egalite.
  Le premier essai, qui supposait a tort un parcours d arborescence, a donne
  5119a3f9... : il a ete REJETE comme non conforme, pas retenu.

=== 2. VERIFICATIONS D AUTORITE (mesurees, hote dgx) ===
TASK_PROMPT_CONTRACTS_V1_SHA256={SHA['contracts']}             MATCH
ENGINE008_SELECTION_RULES_V2_SHA256={SHA['rules_v2']}          MATCH
ENGINE008_SCORER_PROVENANCE_SHA256={SHA['scorer_prov']}        MATCH
  (recalcule depuis les fichiers sur disque, definition de e8-scorers-artifacts.py)
TASK_EVAL_DATA_CONTENT_TREE_SHA256={SHA['task_tree']}          MATCH
TOKEN_EVAL_CONTENT_TREE_V2_SHA256={SHA['token_tree_v2']}       MATCH
  (RE-DERIVE independamment depuis les sources amont, voir §3)
QUALITY_THRESHOLDS_v1.txt sha256={SHA['thresholds']}   (constat ; pas d autorite prealable citee)
STRUCTURED_VALIDITY_CONFIG_v1.txt sha256=7896e65e4ba6c1d6749067db27eba26fc74ecfa25f0fb0418887fc3d17b289d3

=== 3. TOKEN v2 : RE-DERIVATION INDEPENDANTE (verification la plus forte) ===
Script=rec01-token-v2-rederive.py ; e8-s8.py N A PAS ETE EXECUTE (il
reecrirait les artefacts geles) ; la definition S8.3 a ete REIMPLEMENTEE.
  wikitext      rebuilt=32768 units=347
  obrain_docs   rebuilt=16384 units=14
  obrain_rust   rebuilt=16384 units=18
  TOKENIZATION_SECOND_PASS_MISMATCH_COUNT=0
  UNITS_COMPARED=379   STORED_EQ_REBUILT=YES
  TOKEN_EVAL_CONTENT_TREE_V2_SHA256={SHA['token_tree_v2']}
  TOKEN_REFERENCE_HASH_MATCH=YES
  LLAMA_DECODE_CALL_COUNT=0 -- seule operation modele : tokenisation
  (llama-tokenize --ids : chargement du vocabulaire, AUCUN forward pass)
TOKEN_DATA_STATUS=PASS ; TOKEN_CORPUS_FROZEN=YES (autorite entrante, desormais
  adossee a une re-derivation independante reussie)

=== 4. COMPTES TASK (mesures depuis les artefacts recuperes) ===
TASK_RENDERED_MMLU_COUNT=1400
TASK_RENDERED_GSM8K_COUNT=500
TASK_RENDERED_MBPP_COUNT=257
TASK_RENDERED_IFEVAL_COUNT=541
TASK_EVAL_ITEM_COUNT=2698
TASK_GLOBAL_ITEM_ID_DUPLICATE_COUNT=0
TOUS_CONFORMES_A_L_AUTORITE=YES   (1400 / 500 / 257 / 541 = 2698)

=== 5. SELFTESTS DES SCORERS REJOUES SANS MODELE (15 gates d origine) ===
Aucune restriction n a ete abaissee pour faire passer un test.
Environnement : venv gele {Q}/.venv + ENGINE008_IFEVAL_ROOT={Q}/external/mbpp
  (cette variable est le mecanisme DOCUMENTE dans ifeval_v1.py ; sans elle le
   module officiel est introuvable -- c est un defaut d invocation, pas un
   defaut de scorer. Le premier essai, Lance sans elle, est consigne comme tel
   et n a pas ete presente comme un echec de scorer.)
  mmlu_pro_v1        EXIT=0  MMLU_SCORER_SELFTEST=PASS
  gsm8k_v1           EXIT=0  GSM8K_SCORER_SELFTEST=PASS
  mbpp_v1            EXIT=0  MBPP_SANDBOX_SELFTEST=PASS (backend=docker)
  ifeval_v1          EXIT=0  IFEVAL_SCORER_SELFTEST=PASS
  long_context_v1    EXIT=0  LONG_CONTEXT_SCORER_SELFTEST=PASS
  task_scorers_v1    EXIT=0  STRUCTURED_VALIDITY_SCORER_SELFTEST=PASS
                             TASK_AGGREGATE_SELFTEST=PASS
                             BOOTSTRAP_SELFTEST=PASS
GATES DE COMPTAGE, verifiees depuis les artefacts :
  GSM8K_REFERENCE_PARSE_FAILURE_COUNT=0 (sur 500)
  IFEVAL_DUPLICATE_PROMPT_COUNT=0
  IFEVAL_INSTRUCTION_ID_COUNT=25
  STRUCTURED_VALIDITY_ITEM_COUNT=370
  BOOTSTRAP_SEED_U64=5833308329814621762
  BOOTSTRAP_REPEATABILITY_MISMATCH_COUNT=0
CONTRÔLE REEL DU SANDBOX MBPP (non abaisse) :
  fork_attempt            PASS  attendu=RUNTIME_ERROR observe=RUNTIME_ERROR
  network_attempt         PASS  attendu=RUNTIME_ERROR observe=RUNTIME_ERROR
  host_path_read          PASS  attendu=RUNTIME_ERROR observe=RUNTIME_ERROR
  write_outside_sandbox   PASS  attendu=RUNTIME_ERROR observe=RUNTIME_ERROR
  infinite_loop           PASS  attendu=TIMEOUT        observe=TIMEOUT
SCORERS_CURRENT_SELFTEST=PASS
SCORERS_CURRENT_AUTHORITY_VERIFIED=YES (provenance recalculee = 6d9e68cf...)

=== 6. SEUILS (autorite gelee, non modifiee) ===
CONTENU VERIFIE de QUALITY_THRESHOLDS_v1.txt :
  MAX_MEAN_NLL_DELTA_NATS=0.0100
  MAX_RELATIVE_PERPLEXITY_INCREASE=exp(0.0100)-1  (~1.005%)
  MIN_TOP1_TOKEN_AGREEMENT=0.970
THRESHOLDS_CURRENT_AUTHORITY_VERIFIED=YES (fichier present, hash constate ci-dessus)
AUCUN seuil n a ete recalcule, aucun n a ete reconstitue.

=== 7. ENVIRONNEMENT PYTHON ===
PYTHON_ENVIRONMENT_FREEZE.txt, PYTHON_PACKAGE_PROVENANCE.csv,
PYTHON_PACKAGE_ARTIFACTS.sha256, PYARROW_WHEEL.sha256 presents et copies au
checkpoint. venv reinstallable sans reseau (--no-index --find-links) :
PYTHON_ENVIRONMENT_REINSTALL_SELFTEST=PASS (autorite entrante, non rejouee ici).

=== 8. CE QUI RESTE MANQUANT APRES RECUPERATION ===
a) TEXTE AUTORITAIRE DU PROTOCOLE HARNESS : INTROUVABLE.
   L arbre ENGINE-008 ne contient QUE des artefacts DATA (S1..S8) et SCORERS.
   Aucun fichier de protocole HARNESS n a jamais ete materialise. Les clauses
   numerotees §1..§74 utilisees par la passe precedente n existent que dans le
   brief de mission, PAS sous forme de document autoritaire verifiable.
   Consequence assumee : ces clauses restent des clauses de brief. Toute
   clause manquante reste manquante -- aucun protocole n a ete reconstitue a
   partir de ses seuls numeros de paragraphes.
b) MAX_NEW_TOKENS et structure §17 : confirmes par TASK_PROMPT_CONTRACTS_v1
   (GSM8K 1024, MBPP 1024, IFEVAL 4096, GREEDY requis) -- donc adosses a une
   autorite reelle.
c) MTP : aucun document MTP propre a ENGINE-008 n existe. Le MTP n est
   documente que par les metadonnees GGUF du candidat (blk.64.nextn.*) et par
   la note de type ggml. Le champ ENGINE008_MTP_RUNTIME_PROOF_METHOD_DEFINED
   reste une exigence de brief, sans texte autoritaire.
""")

# ============================================================ §6 NATIVE CHAT
W("ENGINE008_NATIVE_CHAT_RENDERER_AUDIT.txt", f"""ENGINE-008 RECOVERY-01 -- §6 AUDIT DU CHEMIN NATIF DE CHAT
==========================================================================
AUDIT_KIND=STATIC_SOURCE_AUDIT
LLAMA_DECODE_CALL_COUNT=0
AUCUN RENDU TASK N A ETE EFFECTUE. AUCUN GGUF N A ETE MODIFIE.

=== 0. PERIMETRE AUDITE -- SOURCES EXACTES DE Q1 ===
Q1_FORK_PATH={FORK}
Q1_FORK_GIT_HEAD={Q1_COMMIT}
Q1_FORK_GIT_BRANCH={Q1_BRANCH}
Fichiers lus :
  include/llama.h        (declaration de l API)
  src/llama-chat.cpp     (implementation prescrite)
  common/chat.h / .cpp   (implementation Jinja du fork)
  common/jinja/*         (lexer, parser, runtime, value, caps)
Les templates EMBARQUES audites sont ceux extraits et hashes dans la passe
precedente, et non modifies depuis :
  BASELINE  c3cf9e34abf4f9e36c2d72165aa9c132d3e2a725b6c2586aaa3a8af9d7a81041 (8952 o)
  CANDIDATE 12827f24b742ea4e80cdc12dbcf9622227056b9f797252a3149263d4f9aaadce (9993 o)
CHAT_TEMPLATE_BASELINE_CANDIDATE_MATCH=NO   (CONSERVE : la divergence demeure)

=== 1. LA FONCTION PRESCRITE (§16 de la passe precedente) ===
Signature lue dans include/llama.h (Q1) :
  LLAMA_API int32_t llama_chat_apply_template(
        const char * tmpl, const struct llama_chat_message * chat,
        size_t n_msg, bool add_ass, char * buf, int32_t length);
COMMENTAIRE DE L EN-TETE, texte exact, DANS LE FORK Q1 (include/llama.h l.1593) :
  "NOTE: This function does not use a jinja parser. It only support a
   pre-defined list of template."
  Le point est donc CONFIRME DANS L AUTORITE Q1 ELLE-MEME, et non plus
  seulement dans l amont comme le supposait la consigne §6. La mise en garde
  de la consigne est VERIFIEE.

=== 2. DISPATCH EFFECTIF ===
src/llama-chat.cpp : table LLM_CHAT_TEMPLATES (llm_chat_template_from_str),
~60 entrees nommees : chatml, llama2/3/4, mistral-v1/v3/v7, phi3/4, falcon3,
zephyr, monarch, gemma, orion, openchat, vicuna, deepseek/2/3/ocr, command-r,
chatglm3/4, glmedge, minicpm, exaone3/4/moe, rwkv-world, granite-3.x/4.0/4.1,
gigachat, megrez, yandex, bailing*, smolvlm, hunyuan-*, gpt-oss, kimi-k2,
seed_oss, grok-2, pangu-embedded, solar-open.
  AUCUNE ENTREE qwen / qwen3 / qwen35 / qwen3.8.
Puis llm_chat_detect_template() applique une detection par indices. Extrait
exact (src/llama-chat.cpp l.99-104) :
  if (tmpl_contains("<|im_start|>")) {{
      return tmpl_contains("<|im_sep|>") ? LLM_CHAT_TEMPLATE_PHI_4
          : tmpl_contains("<end_of_utterance>") ? LLM_CHAT_TEMPLATE_SMOLVLM
          : LLM_CHAT_TEMPLATE_CHATML;
  }}
Les DEUX templates Qwen3.8 contiennent '<|im_start|>' (9 occurrences baseline,
8 candidat) et NE contiennent NI '<|im_sep|>' NI '<end_of_utterance>'
(mesure sur les deux chaines extraites).
=> DISPATCH EFFECTIF = LLM_CHAT_TEMPLATE_CHATML, pour les deux modeles.
DISPATCH_RESULT_TEMPLATE=CHATML
NATIVE_CHAT_RENDERER_SOURCE_AUDIT=COMPLETE

=== 3. CONSEQUENCE : CE QUE LA FONCTION PRESCRITE N EXECUTE PAS ===
Appeler llama_chat_apply_template avec le template Qwen3.8 embarque :
  - n analyse AUCUN Jinja (l en-tete le dit) ;
  - retombe sur la branche ChatML predefinie ;
  - n evalue donc NI reasoning_instructions, NI enable_thinking, NI
    reasoning_effort (dont l alias 'high'->'xhigh' du candidat), NI le
    namespace sysns de fusion des messages system, NI les branches d outils.
Or ces branches ne sont pas decoratives : avec enable_thinking non defini et
reasoning_effort par defaut 'xhigh', le template EMBARQUE emet un bloc
'<|im_start|>system\\n' + reasoning_instructions + '<|im_end|>\\n'.
Le repli ChatML ne l emet pas. Les deux sorties DIVERGENT donc sur du texte
reellement vu par le modele.

=== 4. LE VRAI CHEMIN NATIF JINJA DU FORK ===
Le fork EMBARQUE un moteur Jinja complet :
  common/jinja/ (lexer.cpp, parser.cpp, runtime.cpp, value.cpp, caps.cpp, ...)
  common/chat.cpp inclut "jinja/value.h", "jinja/runtime.h", "jinja/caps.h"
API reelle :
  struct common_chat_params common_chat_templates_apply(
        const struct common_chat_templates * tmpls,
        const struct common_chat_templates_inputs & inputs);
  bool common_chat_verify_template(const std::string & tmpl, bool use_jinja);
struct common_chat_templates_inputs (common/chat.h l.249-267), champs exacts :
  messages; grammar; json_schema; add_generation_prompt = true;
  continue_final_message = COMMON_CHAT_CONTINUATION_NONE; use_jinja = true;
  tools; tool_choice; parallel_tool_calls;
  reasoning_format = COMMON_REASONING_FORMAT_NONE; enable_thinking = true;
  now; chat_template_kwargs (map<string,string>);
  add_bos = false; add_eos = false; force_pure_content = false;
  -- les champs sous 'use_jinja' ne sont honores QUE si use_jinja est vrai.

=== 5. OU VIT CE CHEMIN, ET POURQUOI CELA CHANGE LE HARNAIS ===
liaison verifiee sur les binaires Q1 :
  libllama.so.0.0.135  exporte UNIQUEMENT :
      llama_chat_apply_template, llama_model_chat_template,
      llm_chat_apply_template, llm_chat_template_from_str
      -> AUCUN symbole jinja dans libllama.
  libllama-common.so.0.0.135  exporte :
      common_chat_templates_apply, common_chat_verify_template
      et 788 symboles jinja.
  libllama-common.so.0.0.135 sha256={SHA['libllama_common']} (constat, cf. §4 LIMITE)
=> Un harnais qui ne lie QUE libllama NE PEUT PAS rendre le template embarque.
   Il doit lier libllama ET libllama-common. Cela CONFIRME la preference
   d origine : construire l outil DANS le fork (tools/engine008-quality-eval/),
   ce qui donne acces a common/ et donc au moteur Jinja.

=== 6. VERDICT SUR LA COMPATIBILITE DU CONTRAT ===
Question 1 -- le contrat §17 (aucun system, un message user, add_generation_prompt)
  est-il EXPRIMABLE dans l API prescrite ?
  REPONSE=OUI, formellement : messages=[user] + add_ass=true. Le message-list
  est representable.
Question 2 -- l API prescrite HONORE-T-ELLE le template embarque ?
  REPONSE=NON. Elle retombe sur ChatML et n execute aucune branche Jinja.
=> NATIVE_CHAT_RENDERER_CONTRACT_COMPATIBILITY=NO
   (au sens : le contrat scientifique -- rendre le template EMBARQUE -- n est
    pas satisfait par l API prescrite)
CORRECTION PRECISE A SOUMETTRE A REVUE :
   remplacer l appel prescrit llama_chat_apply_template par
   common_chat_templates_apply (libllama-common.so), avec use_jinja=true et
   les arguments enumeres en §4 ci-dessus, depuis un outil construit DANS le
   fork. Cette correction N EST PAS APPLIQUEE ici : elle exige un amendement
   explicite du protocole.

=== 7. CE QUE CET AUDIT N A PAS FAIT ===
  - aucun rendu TASK (ni Python, ni Jinja, ni Transformers, ni lib non qualifiee)
  - aucune substitution d API
  - aucune modification de GGUF, de commentaire ou de template
  - aucun llama_decode
CHAT_TEMPLATE_BASELINE_CANDIDATE_MATCH=NO  (inchange)
""")

# ============================================================ §7 AMENDEMENT
W("ENGINE008_CHAT_TEMPLATE_AMENDMENT_PROPOSED_v1.txt", f"""ENGINE008_CHAT_TEMPLATE_AMENDMENT_PROPOSED_v1
==========================================================================
CHAT_TEMPLATE_AMENDMENT_STATUS=PROPOSED_NOT_ADOPTED
AUCUN RENDU TASK, AUCUN GEL DE HARNAIS, AUCUN CALCUL MODELE AU TITRE DE CET
AMENDEMENT TANT QU IL N A PAS ETE EXPLICITEMENT ADOPTE.

=== 1. OBJET ===
Objectif propose : comparer baseline et candidat A ENTREES CONTROLEES, c est-a-
dire avec UNE AUTORITE DE RENDU COMMUNE, de sorte que toute difference mesuree
ensuite provienne des MODELES et non du texte qui leur est presente.

=== 2. FAIT MESURE QUI MOTIVE L AMENDEMENT (il demeure declare) ===
EMBEDDED_TEMPLATE_MATCH=NO
  BASELINE_CHAT_TEMPLATE_SHA256={SHA['tpl_base']}
  CANDIDATE_CHAT_TEMPLATE_SHA256={SHA['tpl_cand']}
  premiere divergence a l octet 1971 ; 102 lignes de diff unifie (+34 / -20) ;
  le candidat se termine par {{{{#- Unsloth fixes - developer role, merged
  system messages, tool calling #}}}}
  Le mismatch reste DECLARE partout, y compris apres adoption eventuelle.
  Aucun GGUF n a ete ni ne sera retouche pour obtenir artificiellement
  l egalite des empreintes.

=== 3. INCOMPATIBILITE EXPOSEE AVANT TOUT CHOIX (exigee par §7) ===
Deux objectifs sont possibles et MUTUELLEMENT EXCLUSIFS :

  OBJECTIF_A -- EGALITE DES ENTREES
    Une seule autorite de rendu pour les deux modeles. Le template EMBARQUE du
    candidat est IGNORE au profit d une autorite commune.
    Avantage : §20 (entrees bit-identiques) devient atteignable.
    Cout scientifique ASSUME : le candidat est evalue sur un texte qu il n a
    PAS embarque. La comparaison mesure les poids, pas la fidelite au template.

  OBJECTIF_B -- FIDELITE NATIVE
    Chaque modele est rendu avec SON PROPRE template embarque.
    Avantage : chaque modele voit exactement ce pour quoi il a ete livre.
    Cout : les entrees DIFFERENT par construction ; §20
    (TASK_RENDERED_TEXT_BASELINE_CANDIDATE_MISMATCH_COUNT=0) devient
    INATTEIGNABLE, et toute difference de score melange effet modele et effet
    de mise en forme.

AUCUN CHOIX N EST FAIT ICI. Cet arbitrage est un choix SCIENTIFIQUE, il ne
peut pas etre tranche par un correctif technique, et il ne doit JAMAIS etre
choisi apres observation de scores.

=== 4. PROPOSITION RECOMMANDEE (a adopter ou rejeter en bloc) ===
OBJECTIF RETENU PAR LA PROPOSITION = OBJECTIF_A.
AUTORITE EFFECTIVE COMMUNE = les OCTETS EXACTS du template baseline,
  c3cf9e34abf4f9e36c2d72165aa9c132d3e2a725b6c2586aaa3a8af9d7a81041,
  transmis EXPLICITEMENT au chemin natif retenu, pour les DEUX modeles.
JUSTIFICATION DU CHOIX BASELINE COMME SOURCE : c est le modele de reference de
  la comparaison ; figer l autorite sur le bras de reference rend l ecart
  imputable au candidat. Ce choix est enonce ICI, avant tout rendu et avant
  tout score, donc non piloté par les resultats.

=== 5. CHEMIN NATIF RETENU, SA PROVENANCE ET TOUS LES PARAMETRES ===
Fonction=common_chat_templates_apply
Bibliotheque=libllama-common.so.0.0.135
  sha256={SHA['libllama_common']}  (CONSTAT de cette passe, PAS une autorite :
  aucun SHA d autorite n existe pour cette bibliotheque -- voir §4 de
  ENGINE008_Q1_RECOVERY.txt. Son elevation au rang d autorite doit etre
  explicitement decidee, ou une qualification separee menee.)
Fork={FORK}
  git HEAD={Q1_COMMIT}  branche={Q1_BRANCH}
libllama liee simultanement={BIN}/libllama.so.0
  sha256={SHA['libllama']}   (AUTORITE Q1)
Fonction PRESCRITE AUPARAVANT ET DESORMAIS ECARTEE=llama_chat_apply_template
  MOTIF=mesure : repli ChatML, aucun Jinja (voir ENGINE008_NATIVE_CHAT_RENDERER_AUDIT.txt)
PARAMETRES DE RENDU, tous explicites :
  use_jinja            = true         (OBLIGATOIRE : sinon les kwargs sont ignores)
  add_generation_prompt= true         (prefixe assistant exact, §17)
  add_bos              = false        (TASK_CHAT_TEMPLATE_EMITS_BOS=NO)
  add_eos              = false
  messages             = [ {{role:"user", content:<prompt semantique gele>}} ]
                         SYSTEM_MESSAGE_PRESENT=NO (§17) -- le bloc system
                         eventuel provient du TEMPLATE, pas de l appelant
  tools                = vide
  parallel_tool_calls  = false
  reasoning_format     = COMMON_REASONING_FORMAT_NONE
  enable_thinking      = ETAT A DECIDER, voir §6 (point ouvert)
  chat_template_kwargs = ETAT A DECIDER, voir §6 (point ouvert)
  force_pure_content   = false
  continue_final_message = COMMON_CHAT_CONTINUATION_NONE
  grammar, json_schema = vides
TOKENISATION ASSOCIEE (inchangee) :
  TASK_CHAT_PARSE_SPECIAL=YES
  TASK_TOKENIZER_ADDS_BOS=NO (add_bos=false sur les deux modeles, cf. passe precedente)
  outil={BIN}/llama-tokenize sha256={SHA['llama_tokenize']}

=== 6. POINTS OUVERTS -- A TRancher AVANT ADOPTION (non tranches ici) ===
O1. enable_thinking : le defaut du fork est true. Non defini = true, ce qui
    declenche l emission d un bloc system 'reasoning_instructions' par le
    template. Faut-il l exiger EXPLICITEMENT a true, le mettre a false, ou le
    laisser implicite ? La valeur change le texte vu par les deux modeles.
O2. reasoning_effort : defaut du template 'xhigh'. Varie-t-il par categorie
    TASK (math/code/instruction) ? Aucune autorite ne le prescrit aujourd'hui.
O3. Statut d autorite de libllama-common.so : le constat de SHA suffit-il, ou
    faut-il une qualification separee ? Sans decision, le chemin retenu
    repose sur une bibliotheque non qualifiee -- a declarer tel quel.
O4. Si l autorite voulue exige la FIDELITE NATIVE (OBJECTIF_B), alors cet
    amendement doit etre REJETE tel quel, et §20 doit etre amendé pour
    reconnaitre des entrees differentes par conception. Les deux ne peuvent
    pas coexister.

=== 7. DISTINCTION DES DEUX GATES (exigee) ===
  EMBEDDED_TEMPLATE_MATCH=NO          (mesure ; ne changera pas)
  EFFECTIVE_TEMPLATE_MATCH=NOT_EXERCISED
    (elle ne pourra etre declaree qu APRES exercice du chemin natif retenu,
     sur les 2698 items. Aucune egalite effective n est annoncee ici.)

=== 8. GATES D EGALITE QUI DEMEURENT EXIGES APRES ADOPTION (inchanges) ===
  TASK_RENDERED_TEXT_BASELINE_CANDIDATE_MISMATCH_COUNT=0
  TASK_INPUT_TOKEN_ID_BASELINE_CANDIDATE_MISMATCH_COUNT=0
  TASK_RENDERED_INPUT_COUNT=2698 (1400/500/257/541)
  TASK_INVALID_TOKEN_ID_COUNT=0
  TASK_RENDER_SECOND_PASS_TEXT_MISMATCH_COUNT=0
  TASK_RENDER_SECOND_PASS_TOKEN_MISMATCH_COUNT=0
  TASK_RENDER_REPRODUCIBLE=YES
  MMLU_PREFIX_EXTENSION_NONPREFIX_COUNT=0
  MMLU_CANDIDATE_SUFFIX_TOKEN_MISMATCH_COUNT=0
  MMLU_PREFIX_STATE_REUSE=NO (correctness-first, v1)

=== 9. CONSEQUENCES SUR LES HASHES ET LE GEL ===
C1. ENGINE008_CHAT_TEMPLATE_SHA256 doit devenir le hash de l AUTORITE
    EFFECTIVE, c est-a-dire {SHA['tpl_base']} sous la proposition, et NON un
    hash suppose commun aux deux GGUF.
C2. Le candidat portant toujours son propre template embarque, tout hash de
    harnais couvrant la configuration de rendu doit enregistrer LES DEUX
    empreintes embarquees ET l autorite effective, plus le parametrage §5.
C3. TASK_EVAL_CONTENT_TREE_SHA256 doit etre recalcule apres adoption : il
    couvre desormais un rendu different.
C4. TASK_EVAL_DATA_CONTENT_TREE_SHA256={SHA['task_tree']} reste INCHANGE : il
    porte sur la DATA amont, que l amendement ne touche pas.
C5. La gate historique n est PAS renommee retroactivement pour la faire
    passer : EMBEDDED_TEMPLATE_MATCH=NO reste publie a cote de la nouvelle
    EFFECTIVE_TEMPLATE_MATCH.
C6. Aucun seuil (QUALITY_THRESHOLDS_v1) n est touche par cet amendement.
""")

# ============================================================ §8 CHECKPOINT
def sha_file(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


ck_files = []
for dp, dn, fn in os.walk(CKPT):
    dn.sort()
    for f in sorted(fn):
        fp = os.path.join(dp, f)
        ck_files.append(dict(path=os.path.relpath(fp, HERE),
                             size=os.path.getsize(fp), sha256=sha_file(fp)))

manifest = {
    "pass": "ENGINE-008-RECOVERY-01",
    "generated_utc": "2026-10-02",
    "authority_host": DGX,
    "authority_root": Q,
    "authority_manifest": {
        "file_count": 3590,
        "total_bytes": 1646873938,
        "verification": "8/8 authority checks PASS",
        "raw_manifest": "manifest-dgx-raw.json",
    },
    "q1_toolchain": {
        "fork": FORK, "git_head": Q1_COMMIT, "git_branch": Q1_BRANCH,
        "libllama": {"path": BIN + "/" + Q1_RELPATH,
                     "sha256": SHA["libllama"], "is_authority": True},
        "llama_tokenize": {"path": BIN + "/llama-tokenize",
                           "sha256": SHA["llama_tokenize"], "is_authority": True},
        "f1a_eval": {"path": BIN + "/f1a-eval",
                     "sha256": SHA["f1a_eval"], "is_authority": True},
        "libllama_common": {"path": BIN + "/libllama-common.so.0.0.135",
                            "sha256": SHA["libllama_common"], "is_authority": False,
                            "note": "constat de cette passe ; aucun SHA d autorite enregistre"},
    },
    "models": {
        "baseline": {"path": "/home/triviere/projects/models/qwen3.8-27b-gguf/Qwen3.8-27B-Q4_K_M.gguf",
                     "sha256": SHA["baseline"], "size": 18973870432,
                     "duplicated": False, "note": "reste sur le dgx, non duplique (§8)"},
        "candidate": {"path": "/home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf",
                      "sha256": SHA["candidate"], "size": 12844231424,
                      "duplicated": False, "note": "reste sur le dgx, non duplique (§8)"},
    },
    "authorities_verified": {
        "TASK_EVAL_DATA_CONTENT_TREE_SHA256": SHA["task_tree"],
        "TOKEN_EVAL_CONTENT_TREE_V2_SHA256": SHA["token_tree_v2"],
        "TASK_PROMPT_CONTRACTS_V1_SHA256": SHA["contracts"],
        "ENGINE008_SCORER_PROVENANCE_SHA256": SHA["scorer_prov"],
        "ENGINE008_SELECTION_RULES_V2_SHA256": SHA["rules_v2"],
        "QUALITY_THRESHOLDS_v1_SHA256": SHA["thresholds"],
        "Q1_FORWARD_PROTOCOL_v1_SHA256": SHA["q1_protocol"],
        "BASELINE_CHAT_TEMPLATE_SHA256": SHA["tpl_base"],
        "CANDIDATE_CHAT_TEMPLATE_SHA256": SHA["tpl_cand"],
    },
    "checkpoint": {
        "root": "recovery-01/checkpoint",
        "file_count": len(ck_files),
        "total_bytes": sum(f["size"] for f in ck_files),
        "verification_on_n3": {
            "TASK_EVAL_DATA_CONTENT_TREE_SHA256": SHA["task_tree"],
            "result": "MATCH (recalcule sur n3 depuis le checkpoint)",
            "TASK_PROMPT_CONTRACTS_V1_SHA256": "MATCH",
            "ENGINE008_SELECTION_RULES_V2_SHA256": "MATCH",
            "SCORER_FILES": "6/6 MATCH",
        },
        "files": ck_files,
    },
    "not_recovered": [
        "texte autoritaire du protocole HARNESS (jamais materialise)",
        "quantification MTP documentaire propre a ENGINE-008",
        "hash d autorite pour libllama-common / libggml* / libmtmd",
    ],
}
W("ENGINE008_RECOVERY_AUTHORITY_MANIFEST.json",
  json.dumps(manifest, indent=1, sort_keys=True, ensure_ascii=False))

W("ENGINE008_RECOVERY_CHECKPOINT.txt", f"""ENGINE-008 RECOVERY-01 -- CHECKPOINT ET RECETTE DE RESTAURATION
==========================================================================
RECOVERY_CHECKPOINT_CREATED=YES
RECOVERY_CHECKPOINT_ROOT=recovery-01/checkpoint
RECOVERY_CHECKPOINT_FILE_COUNT={len(ck_files)}
RECOVERY_CHECKPOINT_TOTAL_BYTES={sum(f['size'] for f in ck_files)}
RECOVERY_CHECKPOINT_MANIFEST=ENGINE008_RECOVERY_AUTHORITY_MANIFEST.json

=== VERIFICATION DU CHECKPOINT SUR n3 (hote different de la source) ===
Le checkpoint a ete re-hache sur n3, INDEPENDAMMENT du dgx :
  TASK_EVAL_DATA_CONTENT_TREE_SHA256 recalculé sur n3 = {SHA['task_tree']}
    == autorite : MATCH
  TASK_PROMPT_CONTRACTS_v1.txt      == autorite fa30f16d... : MATCH
  ENGINE008_SELECTION_RULES_v2.txt  == autorite 3c1a16d7... : MATCH
  les 6 fichiers scorers            == SHAs enregistres : MATCH
=> RELECTURE POSSIBLE ET MANIFESTE COHERENT AVEC LES FICHIERS PRESERVES.
   La concordance sur un second hote est une verification plus forte qu une
   simple relecture sur la source.

=== CE QUI EST DANS LE CHECKPOINT, ET CE QUI N Y EST PAS (par decision) ===
DANS : corpus TASK et TOKEN (JSONL + selections), les 6 scorers, les contrats
  et seuils, les regles de selection v1/v2, les statuts d origine (DATA,
  SCORERS, QUALITY, STAGE_S*), les provenances (modeles, tokenizer, python,
  nltk, acquisition), les CSV d audit (GSM8K, IFEval, structured-validity),
  l ordre global DATA.
DEHORS, PAR DECISION : les MODELES (19,0 Go + 12,8 Go) -- §8 interdit de les
  dupliquer ; leurs chemins resolus, tailles et SHA sont au manifeste.
  Les jeux de donnees amont external/ (1,6 Go avec le venv gele) restent sur
  le dgx, references par chemin dans le manifeste.
UN MANIFESTE DE FICHIERS ABSENTS NE COMPTE PAS COMME SAUVEGARDE : c est
  pourquoi les fichiers critiques sont REELLEMENT copies et re-haches ici.

=== RECETTE DE RESTAURATION (reproductible) ===
Etape 1 -- verifier l acces a la source :
  ssh dgx 'ls -d {Q}'
Etape 2 -- re-verifier les autorites AVANT tout usage :
  $Q/.venv/bin/python rec01-verify-authorities.py <staging>      (8 controles)
  $Q/.venv/bin/python rec01-token-v2-rederive.py <staging>       (re-derivation TOKEN v2)
Etape 3 -- reconstituer le checkpoint local :
  ssh dgx 'cd {Q} && tar cf - <liste de fichiers>' | tar xf - -C checkpoint
Etape 4 -- re-verifier le checkpoint sur l hote de destination (re-hachage) ;
  exiger la reproduction de {SHA['task_tree']} sur corpora/task_eval.
Etape 5 -- environnement python gele, SANS RESEAU :
  python -m venv .venv && .venv/bin/pip install --no-index --find-links external/python-wheels -r <freeze>
  (PYTHON_ENVIRONMENT_FREEZE.txt + PYTHON_PACKAGE_ARTIFACTS.sha256 font foi)
Etape 6 -- selftests des scorers, SANS MODELE :
  export ENGINE008_IFEVAL_ROOT={Q}/external/mbpp
  .venv/bin/python scorers/<scorer>.py     (6 modules, 8 gates de selftest)
Etape 7 -- le fork Q1 reste sur le dgx ; ne PAS reconstruire. Verifier
  sha256sum {BIN}/{Q1_RELPATH} == {SHA['libllama']}

=== INTERDITS RESPECTES ===
Aucun git add global. Aucun upload vers les miroirs. Aucun --delete.
Aucune modification distante hors du repertoire de staging recovery-01.
Aucun modele duplique. Aucune cle exposee.
""")

# ============================================================ §9 RAPPORT
W("ENGINE008_RECOVERY_REPORT.txt", f"""ENGINE-008 -- PASSE RECOVERY-01 : RAPPORT
==========================================================================
AGENT_TOKEN_BUDGET=UNLIMITED
PASSE=RECOVERY_ONLY
QUALITY_EXECUTION_ALLOWED=NO
HOTE_D_EXECUTION=n3 ; HOTE_D_AUTORITE={DGX}
DATE_UTC=2026-10-02

=== RESULTAT EN UNE PHRASE ===
Les entrees autoritaires de ENGINE-008 n etaient pas detruites : elles
resident sur le dgx. Elles ont ete retrouvees, et HUIT controles d autorite
sur HUIT passent, dont deux re-derivations independantes.

=== 1. CE QUI A ETE RETROUVE ET VERIFIE ===
  Q1 libllama                      {SHA['libllama']}   MATCH (autorite Q1 recoupee)
  Q1 llama-tokenize                {SHA['llama_tokenize']}   MATCH
  Q1 f1a-eval                      {SHA['f1a_eval']}   MATCH
  baseline model                   {SHA['baseline']}   MATCH
  candidat model                   {SHA['candidate']}   MATCH
  TASK_EVAL_DATA_CONTENT_TREE      {SHA['task_tree']}   MATCH
  TOKEN_EVAL_CONTENT_TREE_V2       {SHA['token_tree_v2']}   MATCH (re-derive)
  TASK_PROMPT_CONTRACTS_V1         {SHA['contracts']}   MATCH
  ENGINE008_SCORER_PROVENANCE      {SHA['scorer_prov']}   MATCH (recalcule)
  ENGINE008_SELECTION_RULES_V2     {SHA['rules_v2']}   MATCH
  Q1_FORWARD_PROTOCOL_v1           {SHA['q1_protocol']}   MATCH (triple concordance)

=== 2. LES DEUX VERIFICATIONS FORTES ===
a) TOKEN v2 RE-DERIVE depuis les sources amont, sans executer e8-s8.py :
   379 unites, 65536 tokens scores, TOKENIZATION_SECOND_PASS_MISMATCH_COUNT=0,
   tree hash reproduit a l identique. Seule operation modele : la tokenisation.
b) CHECKPOINT RE-HACHE SUR n3, hote different de la source : le tree hash TASK
   et les 6 scorers se reproduisent. La recuperation n est pas une simple
   relecture de la source.

=== 3. LES SCORERS SONT VIVANTS ===
Les 6 modules rejouent leurs selftests SANS MODELE, tous EXIT=0 :
MMLU, GSM8K, MBPP_SANDBOX (docker, fork/network/host-path bloques),
IFEVAL, LONG_CONTEXT, STRUCTURED_VALIDITY, TASK_AGGREGATE, BOOTSTRAP.
Aucune restriction n a ete abaissee pour faire passer un test.
Les gates de comptage sont conformes : 2698 items (1400/500/257/541),
STRUCTURED_VALIDITY_ITEM_COUNT=370, GSM8K parse failures=0,
BOOTSTRAP_SEED_U64=5833308329814621762.

=== 4. LA DECOUVERTE STRUCTURANTE : L API PRESCRITE NE REND PAS LE TEMPLATE ===
§16 de la passe precedente prescrivait llama_chat_apply_template. Mesure :
cette fonction, DANS LE FORK Q1 (include/llama.h l.1593), declare elle-meme
« does not use a jinja parser. It only support a pre-defined list of template ».
Sa liste ne contient AUCUN qwen. Sa detection par indices route tout template
contenant '<|im_start|>' -- et les deux templates Qwen3.8 en contiennent --
vers LLM_CHAT_TEMPLATE_CHATML.
=> Le chemin prescrit aurait rendu du ChatML, pas le template embarque, et
   n aurait execute NI reasoning_instructions NI enable_thinking NI sysns.
Le fork possede en revanche un VRAI moteur Jinja (common/jinja/, 788 symboles
dans libllama-common.so), accessible par common_chat_templates_apply avec
use_jinja=true. libllama.so SEUL ne l expose pas.
NATIVE_CHAT_RENDERER_CONTRACT_COMPATIBILITY=NO
La correction precise est preparee pour revue, NON appliquee.

=== 5. LA GATE §15 DEMEURE OUVERTE, ET ELLE EST DE NATURE SCIENTIFIQUE ===
EMBEDDED_TEMPLATE_MATCH=NO (inchange : les deux GGUF portent des revisions
differentes du template). Aucun GGUF n a ete retouche.
L amendement propose expose une INCOMPATIBILITE avant tout choix : egalite des
entrees (l autorite commune ignore le template embarque du candidat) et
fidelite native (chaque modele garde son template, mais §20 devient
inatteignable) sont MUTUELLEMENT EXCLUSIFS.
Aucun choix n est fait. Aucun rendu n a ete effectue au titre de l amendement.

=== 6. BLOCAGES RESTANTS, PRECIS ===
B1. ARBITRAGE DU TEMPLATE (scientifique, decision humaine). Non technique.
    A trancher : OBJECTIF_A (une autorite commune) ou OBJECTIF_B (fidelite
    native). Puis adoption ou rejet de ENGINE008_CHAT_TEMPLATE_AMENDMENT_PROPOSED_v1.
B2. TEXTE AUTORITAIRE DU PROTOCOLE HARNESS : INTROUVABLE. Les clauses §1..§74
    n existent que dans le brief. Aucun protocole n a ete reconstitue a partir
    de numeros de paragraphes. Si un texte autoritaire existe ailleurs, il doit
    etre fourni ; sinon il doit etre ECRIT et gele comme document a part entiere.
B3. STATUT D AUTORITE DE libllama-common.so : constate ({SHA['libllama_common']}),
    non qualifie. Decision requise avant d en faire le chemin de rendu.
B4. POINTS OUVERTS DE RENDU : enable_thinking (defaut true => bloc system
    reasoning_instructions emis) et reasoning_effort (defaut 'xhigh'). Aucune
    autorite ne les prescrit pour TASK aujourd hui.
B5. DOCUMENTATION MTP : aucune source documentaire MTP propre a ENGINE-008.
    Seules les metadonnees GGUF attestent blk.64.nextn.* (4 tenseurs).

=== 7. CE QUE CETTE PASSE N A PAS FAIT (et n avait pas le droit de faire) ===
  HARNESS_PREFREEZE_DECODE_CALL_COUNT=0
  QUALITY_PREFREEZE_BASELINE_RESULT_COUNT=0
  QUALITY_PREFREEZE_CANDIDATE_RESULT_COUNT=0
  Aucun llama_decode, aucun prefill, aucun logit, aucune generation, aucune
  NLL/PPL, aucun benchmark qualite, aucun long contexte, aucun calcul modele
  deguise en selftest.
  Aucun rendu TASK, aucun amendement adopte, aucune requalification Q1, aucun
  gel de harnais, aucun enchainement automatique.

=== 8. VALIDITE DES FAITS ANTERIEURS (distingues, non confondus) ===
  gel historique        : SCORERS_FROZEN=YES, TOKEN_CORPUS_FROZEN=YES (1-2 oct.)
  presence actuelle     : fichiers retrouves sur le dgx, OUI
  hash de reference     : verifie, 8/8 MATCH
  acceptation actuelle  : NON -- la gate template reste ouverte (B1)
  Les preuves MTP/WHT restent STATIQUES. Aucun decode n a eu lieu ; les preuves
  runtime manquantes sont reportees, pas converties en confirmation.
""")

print("ecrits:", WRITTEN)
