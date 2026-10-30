ENGINE-008 PROTOCOL-HARNESS-01 -- RAPPORT DE QUALIFICATION DU RENDERER
==========================================================================
NATIVE_JINJA_RENDERER_QUALIFIED=YES
LLAMA_DECODE_CALL_COUNT=0

=== SOURCES EXACTES AUDITEES ===
FORK=/home/triviere/projects/llama-obrain-cuda-f1a
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
