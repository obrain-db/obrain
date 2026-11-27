# Contrat natif d'identité de domaine (substrate)

Statut : **contrat documentaire aligné sur le code** (#334/#418). L'identité de
domaine est désormais **persistée** — protocole TROIS FICHIERS `domaine.id` +
`domaine.init` + `domaine.fork` — et **portée par la référence v2** (champ
`domaine`, u128). Ce document dit ce que le substrat garantit, ce qu'il ne
garantit pas, et ce que le consommateur doit faire explicitement. Source de
vérité : `crates/obrain-substrate/src/domaine_id.rs`, `store.rs`
(`ReferenceTentative` v2, `resoudre_reference`,
`publier_si_precondition_intacte`, `forker_domaine`, `migrer_legacy_domaine`),
`error.rs` (`DomaineForke`). Format exact des fichiers : `format-spec.md` §0.1.

## 1. Objet et portée

La question : quand deux bases (deux répertoires de stockage) existent, qu'est-ce
qui les distingue ? La réponse native du substrat est désormais **positive sur le
point essentiel** : chaque base porte, DANS son répertoire, un `domaine_id`
persistant (`domaine.id`, u128, UUID v4, marqué par `domaine.init`), et la
référence d'une tentative porte ce domaine (codec v2). Le `writer_id` (identité
d'écrivain, u64) reste distinct et ne discrimine PAS les bases à lui seul — c'est
le `domaine_id` qui les distingue, sauf copie intégrale NON déclarée (voir §5).

## 2. Les cinq identités à distinguer

| # | Identité | Portée dans le substrat | Discrimine les bases ? |
|---|----------|-------------------------|------------------------|
| 1 | **Domaine de stockage** | la base = le répertoire. Persisté dans `domaine.id` (u128, UUID v4), marqué par `domaine.init`, porté par la référence v2 (`ReferenceTentative.domaine`). | **OUI** (sauf copie intégrale non déclarée, §5) |
| 2 | **Écrivain** (`writer.id`) | `ReferenceTentative.auteur()` — `writer.id` (u64), identité d'ÉCRIVAIN, distincte du domaine (`writer_id.rs`). | **NON** : deux bases peuvent le partager par copie. |
| 3 | **Incarnation de ressource** (`NodeId`) | `ReferenceTentative.cible()` — alloué monotone, **localement** à une base, jamais réutilisé. | **NON** : chaque base alloue son premier nœud à `NodeId(1)`. |
| 4 | **Opération logique** | l'intention du consommateur (la transformation voulue). **ABSENTE** de la référence. | non portée |
| 5 | **Tentative physique** | l'entrée estampillée réellement posée dans la chaîne canonique d'UNE base. | oui, **localement** |

Le cœur du contrat : une `ReferenceTentative` v2 identifie une estampille
`(domaine, auteur, horloge, cible, effets)`. Le `domaine` rend la résolution
**sensible à la base émettrice** : `resoudre_reference` rend `Engage` seulement si
le domaine de la référence est nul (legacy v1) ou égal à celui du store qui
résout — une référence qui nomme un AUTRE domaine est intégralement
`Indetermine`, sans même consulter le contenu.

## 3. Ce que porte / ne porte pas la référence

Porte : `auteur` (writer.id, u64 LE), `horloge` (HLC packée, u64 LE), `cible`
(NodeId, u64 LE), `domaine` (u128 BE, UUID RFC 4122), `effets` (les clés écrites,
dans l'ordre de préparation). Codec versionné `OBRT` v2 :
`magic(4) | version(1) | auteur(8 LE) | horloge(8 LE) | cible(8 LE) |
domaine(16 BE) | n(4 LE) | [len(4 LE) | octets]*n` (`REFERENCE_TENTATIVE_VERSION
= 2`, `store.rs`). La v1 (SANS `domaine`) est encore LUE mais plus produite ; elle
décode `domaine == 0` (« domaine non spécifié, legacy »).

Ne porte pas : le **chemin** du répertoire, le **contenu** écrit (aucune valeur),
l'**intention** (opération logique).

Conséquence : deux références v2 byte-identiques portent le même `(domaine,
auteur, horloge, cible, effets)` — donc la même tentative dans le même domaine.
Une référence v1 reste ambiguë entre domaines (limite historique documentée,
jamais une confirmation fictive).

## 4. Comportements attendus (répertoire)

Tous éprouvés par des tests d'intégration existants, réaffirmés ici.

### 4.1 Réouverture (même domaine)
`domaine.id` (+ `domaine.init`) est relu du même répertoire : le domaine est
stable, une référence émise avant fermeture résout `Engage` après réouverture.
*(référence : `tests/identite_domaine.rs::reouverture_meme_base_meme_domaine`,
`tests/reference_tentative_externe.rs`, scénario 1)*

### 4.2 Déplacement du répertoire
`domaine.id` et `domaine.init` sont des fichiers DANS le répertoire : `rename` les
déplace avec lui. Le domaine est stable et la résolution survit au déplacement.
**Le chemin n'appartient pas à l'identité.** *(référence :
`tests/identite_domaine.rs::deplacement_repertoire_conserve_le_domaine`)*

### 4.3 Sauvegarde + restauration
Une sauvegarde est une copie du répertoire (`domaine.id`/`domaine.init` compris).
Restaurer = écraser la base par la copie : l'écriture antérieure à la sauvegarde
résout, celle postérieure est perdue (`Indetermine`). L'identité suit la copie.
*(référence : `store.rs` mod `collision_hlc_controlee` ; `tests/domaine_snapshot_fork.rs`)*

### 4.4 Copie indépendante (fork)
Copier le répertoire produit une base **indistinguable de l'originale** : même
`domaine.id`, mêmes données — une référence de l'originale résout dans la copie.
Distinguer la copie exige **`forker_domaine()`** : l'opération explicite qui tire
et persiste un NOUVEAU `domaine_id` (journal de récupération `domaine.fork`).
*(référence : `tests/identite_domaine.rs::copie_integrale_non_declaree_partage_le_domaine`,
`::fork_declare_regenerer_un_domaine_distinct` ; `store.rs::resolution_dans_copie_de_base`)*

### 4.5 Divergence
Deux bases issues d'un fork déclaré, qui écrivent ensuite des valeurs différentes,
divergent par **domaine** (u128 régénéré) ET par **horloge** (HLC murale) : chaque
divergence ne résout que chez elle (`Engage` localement, `Indetermine` ailleurs).
*(référence : `tests/identite_backend_incarnation.rs`, scénario 1)*

### 4.6 Rollback historique
La résolution est **historique** : elle consulte la chaîne canonique, pas l'état
courant. « jamais engagé » et « compacté » sont tous deux `Indetermine`. La
restauration d'une sauvegarde est un rollback : ce qui n'était pas dans la copie
devient `Indetermine`. *(référence : `store.rs::resoudre_effet_historique`,
`::restauration_depuis_sauvegarde_perd_les_ecritures_posterieures`)*

### 4.7 Incarnation
Un `NodeId` est alloué monotone et **jamais réutilisé** : supprimer puis recréer
une cible produit un `NodeId` différent. *(référence :
`tests/identite_backend_incarnation.rs`, scénario 4)*

## 5. Fiabilité d'une identité de domaine auto-portée

**Conclusion : une identité portée PAR les fichiers (comme `domaine.id`) ne peut
pas, à elle seule, distinguer un fork d'un déplacement — c'est l'opération
EXPLICITE qui les distingue.**

La raison est structurelle : « déplacer » et « copier » produisent des répertoires
byte-identiques ; la seule différence est l'intention de l'opérateur,
inobservable par le substrat. Le protocole (#334/#418) tranche donc par
**l'explicite** :
1. **déplacement** = déplacer le répertoire entier → `domaine.id` suit, même
   domaine (identité conservée) ;
2. **fork** = appeler **`forker_domaine()`** → nouveau `domaine.id` tiré et
   persisté (régénéré), avec journal `domaine.fork` pour la récupération.

**Ce que le domaine CHANGE (par rapport au contrat d'origine)** : le faux positif
croisé — deux bases qui partagent `writer.id` ET écrivent le même contenu —
devient **DÉTECTABLE** dès lors que le fork est déclaré. La référence v2 porte le
`domaine` de sa base ; `resoudre_reference` refuse intégralement une référence qui
nomme un autre domaine, même à contenu identique. *(référence : `store.rs` mod
`collision_hlc_controlee` — `collision_hlc_controlee_detectable_par_domaine`,
`collision_hlc_contenu_identique_detecte_par_domaine`)*

**La limite assumée (documentée dans `domaine_id.rs`)** reste : une copie
intégrale NON déclarée (`cp -a`) partage le MÊME `domaine_id` — seule
l'opération de fork la distingue. **La comparaison de valeurs ne se substitue pas
à l'identité**, et l'identité elle-même ne se substitue pas au geste de fork.

Il faut donc, pour établir un domaine distinct, **l'une des deux choses** :
1. l'**opération explicite de fork** `forker_domaine()` (implémentée), **ou**
2. une **autorité indépendante hors du domaine** (annuaire/registre externe).

## 6. Contrat minimal — ce que le consommateur DOIT faire, ce que le substrat peut vérifier

**Le consommateur DOIT, explicitement :**
- pour **forker** un domaine, appeler `forker_domaine()` (régénère `domaine.id`) —
  une simple copie ne change pas l'identité ;
- pour **déplacer** un domaine, déplacer le répertoire entier (l'identité suit) ;
- pour **identifier** une base legacy à l'historique inconnu (ouverte en
  réouverture, donc identité éphémère), appeler `migrer_legacy_domaine()` ;
- traiter `writer.id` comme une identité **d'écrivain**, jamais comme une
  identité de base.

**Le substrat vérifie (et vérifie déjà) :**
- qu'une référence v2 qui nomme un **autre domaine** (non nul, ≠ celui du store)
  est **intégralement** `Indetermine`, SANS consulter le contenu — la garde de
  domaine de `resoudre_reference` écarte le faux positif croisé ;
- qu'une référence forgée ou altérée ne peut produire que `Indetermine` ;
- qu'une publication préparée sous un domaine qui a été **forké entre-temps** est
  refusée `SubstrateError::DomaineForke` (`error.rs`) — la garde relit
  `domaine.id` du FICHIER, pas du cache mémoire, pour voir le fork d'un pair
  (`publier_si_precondition_intacte`).

**Le substrat ne peut PAS vérifier (et ne prétend pas) :**
- l'unicité inter-bases d'un `domaine.id` copié sans fork déclaré (`cp -a`) ;
- qu'un `Engage` correspond à l'intention du consommateur (l'opération logique
  n'est pas portée) ;
- que la valeur courante relue correspond à ce que la tentative écrivait
  (`valeur_courante` est l'état courant observé, pas une attestation de
  compatibilité).

## 7. Versionnement — OBRT v2 et la règle 8 → 16 octets

L'identité de domaine est **persistée et versionnée**, en coordination avec deux
mécanismes distincts :
- la référence `OBRT` est en **v2** (`REFERENCE_TENTATIVE_VERSION = 2`,
  `store.rs`) : ajout du champ `domaine` (u128 BE) après `cible`. La v1 (sans
  `domaine`) reste LUE (décode `domaine == 0`), plus jamais produite ;
- le format de répertoire substrat (`SUBSTRATE_FORMAT_VERSION = 3`, migration
  `1 → 2 → 3`) est **inchangé** : les fichiers d'identité (`domaine.id` /
  `domaine.init` / `domaine.fork`) sont un **sidecar versionné à part**, avec leur
  propre entête `OBRD1\n` / `OBRI1\n` / `OBRF1\n` (magic + version `1` + `\n`).

**Règle de compatibilité (8 octets → 16 octets)** : le même entête `OBRI1` ne
rend pas deux formats compatibles — l'entête encode magic + version mais **pas la
longueur de la charge utile**. Un `domaine.init` historique à 8 octets (préfixe
64 bits) porterait le même `OBRI1` que le format réel à 16 octets. Le code tranche
par une **lecture stricte** : `lire_init` exige exactement 22 octets (6 + 16) ;
toute autre longueur est un refus `InvalidData`, jamais une relecture ni une
régénération. Toute évolution future de ces fichiers doit **incrémenter la version
de l'entête** (`OBRI2\n`, …) — jamais réutiliser le même chiffre avec une charge
différente. Voir `format-spec.md` §0.1.6.
