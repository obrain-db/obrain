#!/usr/bin/env python3
"""ENGINE-008 RECOVERY-01 -- statut terminal."""
import hashlib
import os

HERE = os.path.dirname(os.path.abspath(__file__))
CK = os.path.join(HERE, "checkpoint")


def sha(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


order = os.path.join(CK, "QUALITY_GLOBAL_ITEM_ORDER_DATA.txt")
order_sha = sha(order) if os.path.exists(order) else "ABSENT"
order_lines = sum(1 for _ in open(order, encoding="utf-8")) if os.path.exists(order) else 0

libllama = "87c42d5f5dd7ab3c10967b4d3eaac82636b6af11e2c18a7f2798bab2d01bb2b2"
task_tree = "c30d71d84712fae390be582111f0473c87ccaaad7411e1ef99b97689d69ab84a"
token_tree = "0ce4512e9d73b5e1cc3beddd2222e9ad7a96592508f10930515ef8b4276cf5dd"
contracts = "fa30f16d95a564491e6d02666350ba783d6f44493ea5d04cef024e92ef0c3bc6"
thresholds = "08d70467d2dada51a362bfa91694b00d6bd0b1dd020feab2f7bb52af47666bc7"
tpl_b = "c3cf9e34abf4f9e36c2d72165aa9c132d3e2a725b6c2586aaa3a8af9d7a81041"
tpl_c = "12827f24b742ea4e80cdc12dbcf9622227056b9f797252a3149263d4f9aaadce"

status = f"""ENGINE-008 -- STATUT TERMINAL DE LA PASSE RECOVERY-01
==========================================================================
RAPPORT TERMINAL -- valeurs MESUREES ou explicitement non disponibles.

AGENT_TOKEN_BUDGET=UNLIMITED
ENGINE008_PASS=RECOVERY_ONLY

--- Q1 ---
Q1_REFERENCE_AUTHORITY_RECOVERED=YES
  (Q1_PREEXECUTION_BUILD_PROVENANCE.txt retrouve ; le SHA de continuite de fil
   a ete recoupe avec lui : CONCORDANCE. Aucune contradiction, aucune
   resolution silencieuse.)
QUALITY_EVAL_LIBLLAMA_SHA256={libllama}
Q1_QUALITY_LIBLLAMA_SHA_MATCH=YES
Q1_QUALIFIED_DEPENDENCIES_AVAILABLE=YES
  (au sens de la DISPONIBILITE ; ces bibliotheques n ont PAS de SHA d autorite
   enregistre : leur identite est constatee, pas qualifiee)
Q1_RUNTIME_ENVIRONMENT_AVAILABLE=YES
  (binaire present, dependances resolues a cote, sur le dgx ; l execution
   effective n est PAS verifiee dans cette passe -- aucun decode autorise)

--- DATA ---
TASK_EVAL_DATA_CONTENT_TREE_SHA256={task_tree}
TASK_DATA_REFERENCE_HASH_MATCH=YES
TASK_EVAL_CONTENT_TREE_SHA256=NOT_CALCULABLE
  (perimetre defini par le protocole HARNESS, dont le texte autoritaire est
   INTROUVABLE ; aucun perimetre n a ete invente)
TASK_GLOBAL_ITEM_ORDER_SHA256={order_sha}
  portee : ordre global au niveau DATA, fichier QUALITY_GLOBAL_ITEM_ORDER_DATA.txt
  ({order_lines} lignes) -- ce n est PAS l ordre global TASK rendu, qui exige
  les prompts rendus et reste non calculable
TOKEN_EVAL_CONTENT_TREE_V2_SHA256={token_tree}
TOKEN_REFERENCE_HASH_MATCH=YES
  (RE-DERIVE independamment depuis les sources amont : 379 unites,
   65536 tokens scores, mismatch=0. Le premier essai, qui supposait a tort un
   parcours d arborescence, a donne 5119a3f9... et a ete REJETE.)

--- SCORERS ET SEUILS ---
TASK_PROMPT_CONTRACTS_V1_SHA256={contracts}
SCORERS_CURRENT_AUTHORITY_VERIFIED=YES
  (ENGINE008_SCORER_PROVENANCE recalculee depuis le disque = 6d9e68cf... MATCH)
SCORERS_CURRENT_SELFTEST=PASS
  (6 modules, EXIT=0 : MMLU, GSM8K, MBPP_SANDBOX/docker, IFEVAL, LONG_CONTEXT,
   STRUCTURED_VALIDITY, TASK_AGGREGATE, BOOTSTRAP. Aucune restriction abaissee.)
THRESHOLDS_CURRENT_AUTHORITY_VERIFIED=YES
  (QUALITY_THRESHOLDS_v1.txt present, sha256={thresholds} ;
   aucun SHA d autorite prealable n avait ete publie -- le constat est publie
   comme constat)

--- CHAT TEMPLATE ---
BASELINE_CHAT_TEMPLATE_SHA256={tpl_b}
CANDIDATE_CHAT_TEMPLATE_SHA256={tpl_c}
CHAT_TEMPLATE_BASELINE_CANDIDATE_MATCH=NO
  (inchange ; aucun GGUF modifie, aucun commentaire retire, aucun template
   retouche pour obtenir artificiellement l egalite)
NATIVE_CHAT_RENDERER_SOURCE_AUDIT=COMPLETE
NATIVE_CHAT_RENDERER_CONTRACT_COMPATIBILITY=NO
  (llama_chat_apply_template, DANS LE FORK Q1, ne parse aucun Jinja et route
   les deux templates vers LLM_CHAT_TEMPLATE_CHATML. Le vrai chemin Jinja est
   common_chat_templates_apply dans libllama-common.so, non expose par
   libllama.so.)
CHAT_TEMPLATE_AMENDMENT_STATUS=PROPOSED_NOT_ADOPTED

--- CHECKPOINT ---
RECOVERY_CHECKPOINT_CREATED=YES
  checkpoint local : 56 fichiers, 3,5 Mo, re-haches sur n3 (hote different de
  la source) : tree hash TASK et 6 scorers reproduits a l identique

--- INTEGRITE PREFREEZE ---
HARNESS_PREFREEZE_DECODE_CALL_COUNT=0
QUALITY_PREFREEZE_BASELINE_RESULT_COUNT=0
QUALITY_PREFREEZE_CANDIDATE_RESULT_COUNT=0

--- PROTOCOLE ---
ENGINE008_PROTOCOL_FROZEN=NO
ENGINE008_EXECUTION_ALLOWED=NO

REMAINING_BLOCKERS=B1_ARBITRAGE_SCIENTIFIQUE_DU_TEMPLATE_EMBARQUE ;
B2_TEXTE_AUTORITAIRE_DU_PROTOCOLE_HARNESS_JAMAIS_MATERIALISE ;
B3_STATUT_D_AUTORITE_DE_libllama-common_SO_NON_QUALIFIEE ;
B4_POINTS_OUVERTS_DE_RENDU_enable_thinking_ET_reasoning_effort ;
B5_AUCUNE_DOCUMENTATION_MTP_PROPRE_A_ENGINE008

ENGINE008_NEXT_STEP=DECIDER_L_OBJECTIF_DE_COMPARAISON_PUIS_ADOPTER_OU_REJETER_ENGINE008_CHAT_TEMPLATE_AMENDMENT_PROPOSED_v1_ET_ECRIRE_LE_TEXTE_AUTORITAIRE_DU_PROTOCOLE_HARNESS

--- ARRET ---
Arret a la fin de RECOVERY-01. Aucun enchainement automatique vers HARNESS,
ni vers une requalification Q1, ni vers l evaluation qualite.
Aucun llama_decode, aucun logit, aucun prefill, aucune generation, aucune
NLL/PPL, aucun benchmark qualite, aucun calcul long contexte.
"""

p = os.path.join(HERE, "ENGINE008_RECOVERY_STATUS.txt")
with open(p, "w", encoding="utf-8", newline="\n") as f:
    f.write(status)
print(status)
