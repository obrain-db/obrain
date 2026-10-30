# PROTOCOLE DE VERSION — ce qui tourne doit être ce qui est validé

*Écrit le 2026-09-16, après avoir constaté une dérive réelle : l'interface promue
annonçait `hors_ligne 0e3367a4` alors que les sources étaient en `990a8911`.*

## 1. La règle, en une phrase

**Une seule référence — un tag — décrit la version validée, et une porte compare
le réel à cette référence.**

Le problème n'était pas l'absence de version : il y en avait une (une branche, un
binaire, une UI promue). Le problème était qu'**aucun moyen ne les comparait**.
Trois états vivants, rien pour dire s'ils étaient d'accord.

## 2. La topologie, dite une fois pour toutes

`/home/triviere/projects/banc-obrain/.git` est le dépôt commun. Les autres
répertoires en sont des **worktrees** :

| arbre | branche | rôle |
|---|---|---|
| `banc-obrain-tronc` | `harness/continuite-dans-contrat-2026-09-15` | **l'arbre de service** : le binaire de `obrain-viz.service` est bâti ici |
| `banc-obrain` | `consolidation/tronc-2026-09-15-plus-fusion` | arbre de travail voisin (d'autres chantiers y vivent) |

Le service est bâti depuis `banc-obrain-tronc`. Ce n'est pas un détail : c'est ce
qui rend une synchronisation manuelle entre les deux **inutile pour la version
servie** — et dangereuse quand on croit l'inverse.

**Ce qui dérive, ce sont des BRANCHES, pas des copies.** Un correctif appliqué
à `banc-obrain` ne change rien à ce qui tourne tant qu'il n'est pas dans la
branche de service.

## 3. Ce qui définit la version

```
VERSION.json          ← l'ancre : tag, branche, arbre, binaire, manifeste d'UI, portes
scripts/version-check.sh   ← la porte qui compare le réel à l'ancre
```

`VERSION.json` nomme :

* `tag_valide` — **la référence stable** (`version-2026-09-16`) ;
* `branche_de_service` et `arbre_de_service` — où la version vit ;
* `reference_ui` — le manifeste de l'UI promue, dont `hors_ligne` doit nommer le
  commit du tag ;
* `portes` — ce qui doit passer pour dire « validé » (moteur, acceptation, interface).

## 4. Pourquoi un TAG et pas un hash de commit

Un fichier ne peut pas figer le hash du commit qui le contient : l'amender change
ce hash. Mesuré trois fois d'affilée — la référence a nommé `990a8911`, puis
`2bf5d5d7`, puis `68e87663`, dont deux valeurs déjà périmées par le commit
suivant. Un tag n'a pas ce défaut : il est **posé sur** un commit et se vérifie.

## 5. Le geste de validation (nouvelle version)

1. les modifications sont **commitée** dans l'arbre de service — rien d'en cours ;
2. la référence est corrigée **dans le même commit** que ce qu'elle valide
   (jamais amendée après coup : on retombe dans la circularité) ;
3. `git tag -f -a version-<date> -m "…" <commit>` ;
4. l'UI est **re-promue** depuis cet arbre propre :
   `scripts/ui-deploy.sh --worktree <arbre-de-service> --id "version-<date>"` ;
5. `scripts/version-check.sh --complet` doit être **au vert**.

## 6. Ce que la porte attrape, et pourquoi ces contrôles

| contrôle | ce qu'il empêche |
|---|---|
| tag → commit courant | valider une version qui a bougé sans être revue |
| aucune modification non commitée | **la cause de la dérive** : l'UI est bâtie depuis l'arbre de travail, la référence vient de `git HEAD` |
| `hors_ligne` de l'UI = commit du tag | servir une page qui n'est pas la version validée |
| binaire du service = binaire attendu | faire tourner un binaire d'un autre arbre |
| empreinte de chaque actif servi | une substitution silencieuse de bundle (déjà arrivée le 14/09 : markdown perdu) |

Défaut réel attrapé au premier passage : une modification non commitée d'**une
autre session** dans `crates/obrain-substrate/src/replay.rs`. La porte l'a nommée
au lieu de laisser la version servie diverger en silence.

## 7. La règle à retenir

> Une version n'est pas un dossier, ni une branche : c'est **une référence qu'on
> peut comparer**. Sans la comparaison, « à jour » est une croyance.
