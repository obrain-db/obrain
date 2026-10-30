# LE CHAT ET L'ENGINE — document de reprise

> Deuxième volet du tissage : ce qui a été appris et corrigé sur **le chat, les
> conversations et la génération**, pour reprendre sans rejouer l'histoire.
> Premier volet : `docs/ECHELLE-MULTI-NIVEAUX-REPRISE-2026-09-16.md`.
> Résumé importable : `docs/ADR-2026-09-16-ECHELLE-MULTI-NIVEAUX.md`.

Écrit le 2026-09-16 · branche `consolidation/tronc-2026-09-15-plus-fusion`.

---

## 1. Le chat, tel qu'il est

| propriété | état | où |
|---|---|---|
| **Monté au niveau de l'App** (pas dans la scène WebGL) | fait — un chat n'a rien à faire dans une scène 3D : il payait la charge GPU et une scène qui échoue emportait la conversation | `app/App.tsx` (`ChatPleinEcran` + bouton flottant) |
| **Accessible depuis toutes les vues** | mesuré : porte présente et cliquable dans 🌌 Cosmos · 🧠 Monde · 🔺 Groupe · 🧬 Signaux (`elementFromPoint` désigne le bouton) | idem |
| **Accessible à la PREMIÈRE CONNEXION** | corrigé : le bouton était masqué quand il n'y avait **aucune base** (`aucuneBase ? null : …`) alors que le panneau restait monté « pour expliquer ce qui manque » — donc fermé ET sans bouton | `App.tsx` |
| **Fonctionne SANS graphe** | mesuré : sans paramètre `store`, `HTTP 200` + SSE → `contexte` → `think` → `texte`. Le serveur ancre alors sur l'espace par défaut, et le Self vient du **compte** (`compte_connecte` → `self_qn_pour`) ; le refus `if (!store) return` était donc **plus strict que le serveur** | `graph/ChatPleinEcran.tsx` (`lancerFlux`, `envoyer`) |
| **Titre honnête** | l'en-tête dit « l'engine sur le graphe » seulement quand un espace est visé ; sinon « l'engine (sans graphe visé) » et d'où vient la réponse | idem |

## 2. Structurer les conversations (`conversations.ts`, module PUR)

Ce qui existait : **une** conversation par store, dans `obrain.chat.v1.<store>` — poser une
question écrasait la précédente, aucun fil n'avait de nom, le seul rangement était de tout
oublier.

Ce qui est posé : plusieurs conversations (identifiant, **titre tiré de la première
question**, dossier libre, épinglage), tri **épinglées d'abord puis la plus récente** (ID
comme départage : deux rendus ne peuvent pas diverger), suppression qui **ne laisse jamais
la liste vide**, **migration idempotente** de l'ancien fil unique, bornage à 40 tours avec
retrait des tours « en cours ».

- Interface : `🗂 n` dans l'en-tête → liste, avec ouvrir · renommer · ranger · épingler ·
  supprimer, et **＋ nouvelle conversation**.
- Tests : `conversations.test.mjs`, **17 cas** (titre, tri, dossiers, suppression, migration,
  bornage, invariant « la liste vit »).
- **Ce test a trouvé un vrai défaut** : `"…\n".lines()` rend une **dernière ligne vide** — la
  règle « même ligne répétée » du détecteur de boucle ne voyait donc jamais le cas le plus
  visible (une ligne répétée six fois en fin de texte).

### Défaut trouvé en mesurant, et il PERDAIT des données

`useState(() => chargerEtat(store))` ne lit le disque qu'au **montage**, et au montage
`store` vaut souvent `""` (le sujet n'est pas encore résolu). L'effet de sauvegarde, lui,
dépendait de `store` : dès que le vrai espace arrivait, il **écrivait dans la clé du vrai
espace un état chargé depuis la clé vide**.

```
après « ＋ nouvelle » : obrain.chat.v2.<espace> → 271 o (2 conversations)
après RECHARGEMENT    :                          155 o (1 conversation)  ← écrasée
après correctif       :                          271 o · 2 lignes visibles
```

**Règle à retenir** : un cahier appartient à un espace — quand l'espace change, on **relit** ;
et on n'écrit que pour l'espace dont le cahier vient.

## 3. Le graphe vide / la première connexion

Il y a **deux « première fois »**, et une seule était traitée :

| cas | état |
|---|---|
| **aucun espace accordé** (`aucuneBase`) | expliqué (compte, espaces, « demandez à l'administrateur ») — et la porte du chat est maintenant ouverte |
| **espace accordé mais VIDE** (self `achaptal` : **2 nœuds, 1 arête**) | corrigé : l'App interroge la matière de l'espace actif (`/api/viz/overview`) et, en dessous de 10 nœuds, dit ce qu'il contient **et donne les deux sorties** |

Mesuré dans les deux sens : espace peuplé (`runtime-dev`) → **aucun bandeau** (il ne ment
pas) ; self `achaptal` → *« Cet espace est encore vide — 2 nœuds… ce n'est pas une panne…
[💬 poser une question] [＋ ajouter un monde] »*. Un échec de lecture ne conclut **jamais**
« vide » : sans chiffre, pas de bandeau.

## 4. L'engine : deux défauts, celui de la boucle et celui du flux

Signalés : « génération en boucle infinie » et « couper le flux ». **Vérifié dans le code** :

- la boucle d'outils est **bornée** (4 à 6 tours) et il n'y a **aucun réessai** ;
- la seule borne de longueur est `plafond_generation(demande, restant)` → un modèle qui se
  répète écrit **jusqu'au plafond**, ce qui se lit comme une génération infinie ;
- **rien** ne détectait la répétition ;
- le rappel de `http_min::post_sse` rendait `()` : **aucun appelant ne pouvait arrêter la
  lecture**, et l'écriture SSE ignorait ses erreurs (`let _ = …`) → un onglet fermé laissait
  l'engine générer, à **600 s de délai par appel de modèle** ;
- aucune question n'avait de durée maximale.

Correctifs :
1. `repetition.rs` (module pur, 7 cas) : détecte un motif dégénéré — **prudent par
   construction** (motif long, répétitions **consécutives**, texte déjà substantiel), parce
   qu'un détecteur qui crie au loup couperait des réponses **justes** ;
2. `post_sse` : le rappel rend « continuer » ou « arrêter », et `post_sse` rend `Ok(true)`
   quand l'arrêt vient de lui — **la fin d'un flux n'est plus confondue avec une réponse
   complète** ;
3. `engine.rs` : le texte émis (raisonnement **et** réponse) est accumulé ; un motif
   dégénéré arrête la lecture, **l'annonce**, et **n'exécute aucun outil** ;
4. un drapeau `client_parti` (posé par l'écrivain SSE à la première écriture refusée) arrête
   la génération — vérifié en tuant le client à 4 s :

```
WARN obrain::engine: engine : client parti — génération arrêtée au tour 1 (23.8 s de modèle cumulées)
```
   et l'instance **reste vivante** (`HTTP 200` après l'abandon) ;
5. `BUDGET_QUESTION_S = 300` borne la **somme** des appels au modèle, pas un appel.

**Un défaut de mon propre correctif, trouvé en le mesurant** : le journal était posé en tête
de tour — donc il ne pouvait rien dire, la boucle s'arrêtant au premier tour. Il est
maintenant **là où l'arrêt a lieu**, et au niveau `warn` (mesuré : en `info!`, le service ne
l'écrit pas).

**Reste** : la détection du départ du client survient tardivement (le temps que l'écriture
échoue) — à mesurer si l'on veut qu'elle soit immédiate.

## 5. Reprendre : les commandes

```bash
# Les portes
cd packages/obrain-dashboard && ./node_modules/.bin/tsc -b
cd ../.. && cargo test -p obrain-cli --bin obrain        # 228 tests
npx tsc src/graph/conversations.ts --outDir /tmp/conv-test --module esnext \
  --target es2020 --moduleResolution bundler --strict && node src/graph/conversations.test.mjs
npx tsc src/graph/densite.ts --outDir /tmp/dens-test --module esnext \
  --target es2020 --moduleResolution bundler --strict && node src/graph/densite.test.mjs
npx tsc src/graph/echelle.ts --outDir /tmp/ech-test --module esnext \
  --target es2020 --moduleResolution bundler --strict && node src/graph/echelle.test.mjs

# Le chat sans graphe (le chemin le plus court pour vérifier le découplage)
curl -s -N -m 90 "http://127.0.0.1:8455/api/engine/chat?q=bonjour&pensee=non" | head -20

# Un client qui part : l'arrêt doit s'écrire dans le journal du service
timeout 6 curl -s -N -m 4 "http://127.0.0.1:8455/api/engine/chat?q=explique%20le%20depot&pensee=non" > /dev/null
grep "client parti" /tmp/serve-*.log
```

## 6. Les commits de ce volet

| commit | ce qu'il pose |
|---|---|
| `b3a105c7` | la porte du chat ne se ferme plus quand rien n'est ouvert (première connexion) |
| `5c39e395` | le chat cesse d'exiger une base pour parler (il répond sans graphe) |
| `bdd32a8f` | structurer ses conversations (module pur + interface) |
| `e7c1f6cb` | un espace accordé mais vide s'explique (le mot qui manquait) |
| `8c76266c` | la génération qui boucle s'arrête (coupe-circuit de répétition) |
| `4cbbf229` | le client parti arrête la génération + budget par question |

## 7. En trois phrases, pour reprendre demain

1. **Le chat est découplé du graphe** : il vit au niveau de l'App, s'ouvre de partout, et
   répond même sans base (le serveur ancre sur son espace par défaut et le Self du compte).
2. **Les conversations se structurent** (titre, dossier, épingle, migration, persistance) —
   et un changement d'espace **relit** le cahier au lieu d'écrire celui d'un autre.
3. **L'engine sait s'arrêter** : répétition détectée, client parti, budget par question —
   chaque arrêt se dit, dans le texte pour l'utilisateur et dans le journal pour l'exploitant.
