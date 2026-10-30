# La flotte, mesurée — et ce qu'un harness sait de lui-même

Relevé du 2026-09-15, depuis n3 (192.168.1.103). Aucune de ces lignes n'est déduite :
chaque cellule vient d'une sonde exécutée (`curl` sur le contrat du harness, `ssh`
non interactif, `/dev/tcp` pour la joignabilité).

## 1. Cartographie

| machine | SSH (22) | HTTP (80) | état du harness |
|---|---|---|---|
| **n1** `.101` | ouvert, **clé refusée** | fermé | inconnu — non inventorié |
| **n2** `.102` | **clé autorisée** ✓ | fermé | binaire présent (158 Mo, 9 sept.), **aucun store**, rien qui tourne |
| **n3** `.103` | ✓ (machine locale) | 308 → caddy | **en marche** : harness `:8090`, viz `:8432` |
| **dgx** `.107` | fermé | 200 → SPA (`<base href="/">`) | inconnu |
| **m4** `.54` | fermé | **502** | **amont tombé** — un proxy répond, la cible non |

Note : les ports 101/102/103 sont **fermés** sur les trois machines ; la joignabilité
de n1/n2 passe par le **22**, pas par un port qui porterait leur numéro.

## 2. Ce qu'un harness dit de lui-même — sur DEUX machines

n3, `GET /api/harness/health` (280 µs depuis le LAN) :

```json
{"mode":"ro","status":"ok","store":"/home/triviere/.local/share/obrain-harness/data/runtime-dev.obrain","version":"0.0.1"}
```

n2, `obrain version` :

```text
obrain 0.0.1
Build:
  rustc:    1.91.1
  target:   x86_64
  os:       linux
```

**Occurrences du nom de la machine : 0.** Et `version: "0.0.1"` est une constante :
les deux machines répondent la même chose. Ce qui les distingue est le **chemin d'un
store** — sur n2, il n'y en a même pas.

Autres routes du contrat (`harness_serve.rs:125-152`) : `/api/harness/sessions`
(rend `cwd` et `title` — un fragment de connaissance de soi), et
`/api/harness/context`. Cette dernière **sans session** rend
`{"engrams":[],"goals":[],"session_id":"","weave":[],"summaries":[],"status":"ok"}` :
un « ok » sur un vide intégral. Une réponse vide qui a la forme d'une bonne réponse —
le mode de défaillance à interdire dans un système de pilotage.

## 3. Le graphe global ne connaît aucune machine

Interrogation du store de n3 : les noms contenant `harness` ou `machine` ne remontent
que des **chemins de fichiers du code source** du projet harness. Les labels présents
sont `Message`, `ToolCall`, `ToolResult`, `Step`, `Function`, `SelfState` (24 478).
Les sessions, les appels d'outils, l'état du Self — mais **pas la flotte**.

## 4. Ce que cela impose pour « le chat pilote le harness du client »

On ne peut pas piloter une machine qui n'a pas de nom. Ni transport ni permissions ne
sont le verrou : il n'y a rien à adresser, rien à autoriser, rien à auditer **par
machine**. n2 en est la preuve — provisionnée il y a six jours, jamais reliée à quoi
que ce soit.

Ordre de travail proposé :

1. **La machine comme nœud de premier ordre**, écrite par le **harness lui-même** au
   démarrage (`machine:<hostname>` : os, store servi, mode offert, dernière présence).
   Lui seul sait où il tourne. Auto-déclaration, pas inventaire administré.
2. **La déclaration des capacités et de leurs limites** — sinon le modèle propose des
   gestes qu'il ne peut pas prendre et l'échec arrive après coup.
3. **Le chemin d'action** : identité × action × machine. La garde a déjà cette forme
   (`controler_action(session, ACTION_*, espace)`), et l'`espace` — aujourd'hui déduit
   d'un **nom de fichier** — est l'embryon de la machine. C'est ce dérivé qui a cassé
   le compte neuf le 2026-09-15, et c'est ce qui cassera la deuxième machine.
4. **La règle apprise du bug** : *annoncer est décider*. Une liste ne doit jamais
   contenir une ressource dont on n'a pas vérifié le droit — ni une machine qu'on ne
   joint pas.

## 5. Le test de bout en bout, à écrire

Deux harnesses sur deux stores (n3 en marche, **n2 à initialiser** — binaire présent,
aucun store, rien qui tourne : le sujet idéal), et **une seule question au serveur
global** : « quelles machines connais-tu, et que sait faire chacune ? ».

Aujourd'hui cette question **n'a aucune réponse** : elle échoue par le vide, pas par
l'erreur. Le jour où elle nomme les deux machines et leurs capacités, le pilotage
devient constructible — et le test devient le garde-fou qui empêche de le reperdre.

## 6. Ce qui reste hors de portée, dit franchement

- **n1** : clé SSH refusée — inventaire impossible sans accès.
- **m4** : amont en 502 — quelque chose est tombé là-bas, ce n'est pas une absence.
- **dgx** : répond en HTTP (SPA) mais rien n'indique un harness.
- Aucun identifiant pour le dashboard armé en IAM : les routes authentifiées de la
  flotte se mesurent sur instance isolée, jamais sur le déploiement réel.
