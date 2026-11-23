# KV-outils — séparation RENDU / ANALYSE des outils (2026-10-05)

Dernière porte du chantier KV-outils du chat obrain. Branche `feat/kv-splice-chat`,
serveur `llama-obrain-cuda-f1a`.

## 1. Le problème (mesuré, pas supposé)

Le chemin KV-outils greffe un état KV (`state_ref`) qui contient **déjà** le message
système et les ~70 schémas d'outils ; il envoyait donc la requête **sans `tools`**
(sinon le gabarit les rendrait une seconde fois). Or le serveur ne construit le
parseur d'appels que **si `tools` est présent** :

- `tools/server/server-common.cpp` : `parse_tool_calls` n'est posé que si
  `!inputs.tools.empty()` ;
- `common/chat.cpp` : le parseur PEG est généré **à partir des outils**
  (`common_chat_tools_to_json_oaicompat(inputs.tools)` → `peg_generator`).

Résultat mesuré (9910, et reproduit ici sur une instance de test, requête B
ci-dessous) : sans `tools`, `finish_reason: "stop"`, `tool_calls: null`, et le
modèle écrit `<tool_call>` **en texte brut dans `content`**. Ajouter
`"parse_tool_calls": true` ne change rien : ce champ ne **crée** pas de parseur, il
autorise seulement celui que les outils feraient naître.

## 2. Le point de séparation choisi

Un outil sert à deux choses distinctes :

| Rôle | Qui le consomme |
|---|---|
| **RENDU** — insérer les schémas dans le prompt | le gabarit Jinja (variable `tools`) |
| **ANALYSE** — reconnaître les appels en sortie | le parseur PEG + la grammaire d'appels |

Le fork rendait déjà le **choix de gabarit**, le **parseur** et la **grammaire**
dépendants de la seule présence de `inputs.tools`. Il ne restait donc qu'un point à
séparer : **le passage des outils au contexte Jinja**, unique et localisé dans
`common_chat_template_direct_apply_impl` (`common/chat.cpp`) — tous les rendus
(prompt, generation prompt, gabarits spécialisés, analyse différentielle) passent
par lui.

Champ ajouté : **`render_tools`** (défaut `true`) dans
`common_chat_templates_inputs` (`common/chat.h`) et dans
`autoparser::generation_params` (`common/chat-auto-parser.h`). Quand il vaut
`false`, la variable `tools` **n'est pas posée** dans le contexte Jinja : le
gabarit rend alors **exactement** le prompt qu'il rendait quand la requête n'en
portait pas — pendant que le parseur, lui, est construit **avec** elles.

## 3. Comment la requête dit « analyse mais ne rends pas »

Nouveau champ de corps, sur `/v1/chat/completions` (et toute route qui passe par
`oaicompat_chat_params_parse`) :

```json
{
  "messages": [ { "role": "user", "content": "..." } ],
  "state_ref": "<ref de l'état système+outils>",
  "tools": [ /* les schémas RÉELS du compte */ ],
  "tool_choice": "auto",
  "tools_parse_only": true
}
```

- `tools` : **obligatoire** pour que le parseur existe (noms + schémas validés) ;
- `tools_parse_only: true` : ces outils ne sont **pas rendus** — le prompt les
  porte déjà, greffés en état KV ;
- le défaut est `false` : une requête qui ne pose pas ce champ garde le
  comportement d'avant, au bit près.

**Ce n'est volontairement PAS déduit de `state_ref`** : un état de **fait** (chemin
A.2) se combine très bien avec des outils rendus normalement. Déduire « ne rends
pas les outils » de la seule présence d'un `state_ref` casserait ce cas-là.

Diagnostic : `tools_parse_only` sans `tools` → avertissement journal (aucun outil à
analyser, la sortie restera du texte brut).

## 4. Preuve

- **Test unitaire** (`tests/test-chat.cpp`, `--kv-outils`), sur le gabarit du modèle
  **déployé** (`models/templates/Qwen3.8-27B-OBRAIN-v3.jinja`, copié octet pour
  octet depuis `/props`, sha256 `12827f24…`) :
  1. avec outils, le prompt contient `# Tools` ;
  2. avec `render_tools=false`, le prompt est **identique à l'octet près** au prompt
     sans outils (c'est la condition du splice : rien n'est rendu deux fois) ;
  3. le parseur existe dans les deux cas (il vient des outils) ;
  4. la **même** sortie brute est un appel **structuré** en analyse seule, et du
     **contenu brut** sans outils (le défaut mesuré, reproduit comme contrôle).
- **Bout en bout** (instance de test sur port libre, CPU, `state_ref` réel, 9910 non
  touché) : `cached_tokens == fact_tokens == 367` (l'état est bien greffé, pas
  re-décodé) et

  | requête | finish_reason | tool_calls | content | prompt_tokens |
  |---|---|---|---|---|
  | A `tools` + `tools_parse_only` + `state_ref` | `tool_calls` | `n3__write{path,content}` | vide | 413 |
  | B `state_ref` seul (contrôle) | `stop` | `null` | `<tool_call>` en texte | 413 |
  | C `tools` rendus + `state_ref` (doublon) | `tool_calls` | idem A | vide | **747** |

  La ligne C chiffre ce que le client évite : +334 jetons de schémas rendus une
  seconde fois, à chaque étape.
- **Recompilation** : `make -j16 llama-server` dans `build-cuda` → 0 erreur.
- **Non-régression** : `test-chat` complet (60+ gabarits) → `All tests passed!`.

## 5. Points d'incertitude

1. **Gabarits à variante `tool_use` distincte** : quand la requête porte des outils,
   la variante `tool_use` reste choisie (celle dont l'état a été extrait), mais elle
   est rendue **sans** le bloc d'outils. Sur le 27B de 9910 il n'y a qu'un gabarit
   (`template_tool_use` absent) : le cas est sans effet aujourd'hui. Un
   avertissement unique au journal le signale si un modèle à deux gabarits arrive.
2. **`--no-jinja`** : le chemin `common_chat_templates_apply_legacy` ne rend jamais
   les outils (il ignore `inputs.tools`) — `render_tools` y est donc sans objet.
3. **Client** : rien n'est modifié côté chat. Tant qu'il n'envoie pas `tools` +
   `tools_parse_only`, le comportement reste celui d'aujourd'hui (le défaut mesuré).
