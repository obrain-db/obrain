# Contrat natif d'identité de domaine (substrate)

Statut : **contrat documentaire + tests de comportement**. Aucune structure
persistée nouvelle, aucun champ ajouté, aucune identité logique `K` créée. Ce
document dit ce que le substrat garantit déjà, ce qu'il ne garantit pas, et ce
que le consommateur doit faire explicitement pour qu'une identité de domaine
existe.

## 1. Objet et portée

La question : quand deux bases (deux répertoires de stockage) existent, qu'est-ce
qui les distingue ? La réponse native du substrat est **négative sur le point
essentiel** : rien, dans la `ReferenceTentative`, ne nomme le *domaine de
stockage*. Ce contrat fixe les termes, éprouve les comportements, et conclut que
l'identité de domaine ne peut être **ni déduite ni auto-portée sans opération
explicite**.

## 2. Les cinq identités à distinguer

| # | Identité | Portée dans le substrat | Discrimine les bases ? |
|---|----------|-------------------------|------------------------|
| 1 | **Domaine de stockage** | la base = le répertoire. N'apparaît **NULLE PART** dans la référence ni dans aucun champ persisté. | (c'est lui qu'on voudrait nommer) |
| 2 | **Écrivain** (`writer.id`) | `ReferenceTentative.auteur()` — `writer.id` persiste dans le répertoire de la base, tiré une fois, relu à chaque ouverture. | **NON** : deux bases peuvent le partager par copie du fichier (`writer.id`). |
| 3 | **Incarnation de ressource** (`NodeId`) | `ReferenceTentative.cible()` — alloué monotone, **localement** à une base, jamais réutilisé. | **NON** : chaque base alloue son premier nœud à `NodeId(1)`. |
| 4 | **Opération logique** | l'intention du consommateur (la transformation voulue). **ABSENTE** de la référence : rien ne la nomme. | non portée |
| 5 | **Tentative physique** | l'entrée estampillée réellement posée dans la chaîne canonique d'UNE base. C'est ce que résout `resoudre_reference`. | oui, mais **localement** seulement |

Le cœur du contrat : une `ReferenceTentative` n'identifie qu'une estampille
`(auteur, horloge)` **locale à une base**. Cette localité n'est pas portée par la
référence. `resoudre_reference` rend `Engage` dès que l'entrée
`(auteur, horloge, cible, clé)` existe dans la chaîne canonique du store qui
résout — indépendamment de la base qui a émis la référence.

## 3. Ce que porte / ne porte pas la référence

Porte : `auteur` (writer.id), `horloge` (HLC packée), `cible` (NodeId), `effets`
(les clés écrites, dans l'ordre de préparation). Codec versionné `OBRT` v1 :
`magic(4) | version(1) | auteur(8 LE) | horloge(8 LE) | cible(8 LE) | n(4 LE) |
[len(4 LE) | octets]*n`.

Ne porte pas : le **chemin** du répertoire, le **contenu** écrit (aucune valeur),
aucune **identité de base/domaine**, aucune **intention** (opération logique).

Conséquence : la comparaison des octets de deux références dit « même
`(auteur, horloge, cible, effets)` », pas « même tentative physique ». Deux
tentatives physiques distinctes peuvent produire des références byte-identiques
(collision HLC contrôlée, `tests/collision_hlc_controlee.rs`).

## 4. Comportements attendus (répertoire)

Tous éprouvés par des tests d'intégration existants, réaffirmés ici.

### 4.1 Réouverture (même domaine)
`writer.id` est relu du même répertoire : l'auteur est stable, une référence
émise avant fermeture résout `Engage` après réouverture. L'`incarnation` (état
en mémoire) n'est pas persistée et ne survit pas — mais elle n'entre pas dans la
référence. *(référence : `reference_tentative_externe.rs`, scénario 1)*

### 4.2 Déplacement du répertoire
`writer.id` est un fichier DANS le répertoire : `rename` le déplace avec lui.
L'auteur est stable et la résolution survit au déplacement. **Le chemin
n'appartient pas à l'identité.** *(référence : `collision_hlc_controlee.rs`,
scénario 3)*

### 4.3 Sauvegarde + restauration
Une sauvegarde est une copie du répertoire (`writer.id` compris). Restaurer =
écraser la base par la copie : l'écriture antérieure à la sauvegarde résout,
celle postérieure est perdue (`Indetermine`). `writer.id` ne porte aucune notion
de fraîcheur. *(référence : `collision_hlc_controlee.rs`, scénario 4)*

### 4.4 Copie indépendante (fork)
Copier le répertoire produit une base **indistinguable de l'originale** : même
`writer.id`, mêmes données. Une référence de l'originale résout dans la copie.
`writer.id` + `horloge` identifient une **tentative**, pas une base. C'est ici
que le contrat exige une opération explicite (voir §6). *(référence : test `resolution_dans_copie_de_base` dans `src/store.rs`)*

### 4.5 Divergence
Deux bases issues d'une copie, qui écrivent ensuite des valeurs différentes,
divergent par **horloge** (HLC murale) : chaque divergence ne résout que chez
elle (`Engage` localement, `Indetermine` ailleurs), tant que les horloges ne se
rejoignent pas. *(référence : `identite_backend_incarnation.rs`, scénario 1)*

### 4.6 Rollback historique
La résolution est **historique** : elle consulte la chaîne canonique, pas
l'état courant. Une entrée engagée puis masquée reste `Engage` ; « jamais
engagé » et « compacté » sont tous deux `Indetermine`. La restauration d'une
sauvegarde est un cas de rollback : ce qui n'était pas dans la copie devient
`Indetermine`. *(référence : `resoudre_effet_historique`, `restauration_…`)*

### 4.7 Incarnation
Un `NodeId` est alloué monotone et **jamais réutilisé** : supprimer puis
recréer une cible produit un `NodeId` différent. Un `NodeId` vivant ne prouve
donc pas une incarnation identique — la référence de l'incarnation morte reste
`Indetermine`. *(référence : `identite_backend_incarnation.rs`, scénario 4)*

## 5. Fiabilité d'une identité de domaine auto-portée

**Conclusion : une identité auto-portée (un UUID copié AVEC les fichiers, comme
`writer.id`) ne peut PAS distinguer un fork d'un déplacement.**

La raison est structurelle : « déplacer » et « copier » produisent des
répertoires byte-identiques. La seule différence est l'intention de l'opérateur,
qui n'est pas observable par le substrat. Toute identité qui vit dans les
fichiers du domaine est donc, par construction, **indistinguable entre un
déplacement légitime (même domaine, identité conservée) et une copie (domaine
distinct, identité partagée à tort)**.

Le défaut démontré (`collision_hlc_controlee.rs`) en est le cas limite : deux
bases partageant `writer.id` produisent des références byte-identiques qui
résolvent `Engage` dans les deux. La contre-épreuve ajoutée
(`collision_hlc_contenu_identique_faux_positif_indetectable`) va plus loin :
même **contenu** écrit des deux côtés → le faux positif croisé devient
**totalement indétectable** (ni octets, ni valeur relue, ni auteur résolvant ne
séparent les deux tentatives). **La comparaison de valeurs ne se substitue pas à
l'identité** : c'est précisément là que l'ambiguïté est maximale.

Il faut donc, pour établir un domaine distinct, **l'une des deux choses** :
1. une **opération explicite de fork/réaffectation** (le consommateur demande une
   nouvelle identité de domaine, et le substrat la régénère), **ou**
2. une **autorité indépendante hors du domaine** (un annuaire/registre externe
   qui attribue et atteste les identités de domaine).

## 6. Contrat minimal — ce que le consommateur DOIT faire, ce que le substrat peut vérifier

**Le consommateur DOIT, explicitement :**
- conserver le **domaine de stockage** (répertoire) **hors-bande** : c'est lui
  qui donne à une référence son interprétation ;
- traiter `writer.id` comme une identité **d'écrivain**, jamais comme une
  identité de base ;
- pour **forker** un domaine, exécuter une opération explicite qui régénère
  l'identité (le contrat ne fixe pas le mécanisme — fichier d'identité dédié ou
  autorité externe — il fixe la **nécessité** de l'explicite) ;
- pour **déplacer** un domaine, déplacer le répertoire entier (l'identité suit).

**Le substrat peut vérifier (et vérifie déjà) :**
- qu'une référence forgée, altérée ou **venue d'une autre base** ne peut produire
  que `Indetermine` — un `Engage` exige de retrouver, dans la chaîne canonique de
  CE store, une entrée estampillée `(auteur, horloge)` égale à la référence ;
- que `auteur_resolvant` (le `writer_id` local) est **local** : le comparer à
  `reference.auteur()` écarte une référence étrangère, mais **ne discrimine pas
  deux bases qui partagent `writer.id`** (c'est la limite mesurée).

**Le substrat ne peut PAS vérifier (et ne prétend pas) :**
- l'unicité inter-bases de `writer.id` (une copie du fichier la viole) ;
- qu'un `Engage` correspond à l'intention du consommateur (l'opération logique
  n'est pas portée) ;
- que la valeur courante relue correspond à ce que la tentative écrivait
  (`valeur_courante` est l'état courant observé, pas une attestation de
  compatibilité).

## 7. Question ouverte — OBRT / V4 (coordination T6)

L'identité de domaine **n'est pas persistée aujourd'hui**. Si elle devait
l'être (un UUID de domaine dans un fichier du répertoire, à côté de `writer.id`),
elle devra être **versionnée en coordination avec T6** : le codec `OBRT`
(`REFERENCE_TENTATIVE_VERSION = 1`) et le format de répertoire
(`SUBSTRATE_FORMAT_VERSION = 3` dans `meta.rs`, migration `1 → 2 → 3`) suivent
chacun leur table de version, et un champ d'identité persistant ne peut être
ajouté sans arbitrer sa place dans ces deux mécanismes. **Non évalué ici** : le
présent contrat est documentaire et comportemental, il n'ajoute aucun champ ni
aucune identité logique `K`.
