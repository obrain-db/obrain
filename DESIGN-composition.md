# Composition native — point d'extension (audit 26/11)

## Primitive retenue
Le substrat simplicial stocke déjà : `SimplexView { kind, flags, members }` (hyperarête
typée, membres canonicalisés triés+dédup), `key_hash_members` (empreinte canonique),
incidence nœud→simplexes, tombstones, passes de compaction.

## Ce qui manque pour la composition
1. Relations typées ORIENTÉES entre membres (rôles) — le simplex est un ensemble,
   pas un graphe relationnel. → réutiliser les arêtes typées existantes (EdgeKind).
2. Référence entre connaissances (une composition référence un sous-simplexe AB)
   → nouvelle relation de composition (à versionner).
3. Identité structurelle ≠ empreinte : distinguer indexation par hash (rapide) de
   l'égalité structurelle effective (gère les collisions).

## Mapping cible
- AB  = un SimplexView (kind="structure", members={A,B}) + une arête typée orientée
       (A --calls--> B) pour la relation.
- ABC = un SimplexView (members={A,B,C}) + une RÉFÉRENCE vers AB (composition) +
       ses propres arêtes. L'identité d'ABC inclut la référence à AB.
- Contexte/état dynamique : séparé de la structure (projection ou propriétés).

## Premier test natif (à implémenter)
Stocker AB, ABC, ABD via les primitives substrate, les retrouver par incidence,
modifier un contexte sans contaminer l'autre, supprimer ABD en préservant AB,
réouvrir la base et reconstruire. Mesurer coût vs plat.
