# Backlog Dashboard obrain — graphe de tâches

Édité : 2026-09-09 · Méthode : docs/UX-UI-SKILL.md · Suivi : todo + commits.

## Graphe (dépendances)
```
UX-UI-SKILL (fait)
   ├─ P1. Console soignée + aria ............ DONE 985cf2f5
   ├─ P1. Recherche paliers/ancrage/axes ..... DONE (74758757, 42356986)
   ├─ P1. RiccI provisionné partout .......... DONE a0fc2fdd (kernel 60,6%)
   ├─ P2. Couches présélectionnées/états ..... DONE f6af4cfb
   └─ P2. Monde : items+modules ............. DONE d716c2de
   ┌─ P2. CANVAS UNIFIÉ (Monde↔Graphe échelle) → bloc C (gros chantier)
   ├─ P2. Arcs CALLS inter-modules .......... AGENT A (serveur+overlay)
   ├─ P2. Couches/organique dans la vue Monde → AGENT B
   ├─ P3. Flux d'exécution entre 2 ancres .... PENDING
   ├─ P3. Inline restants + états vides ....... AGENT B (partiel)
   └─ P3. Vérif capture utilisateur ......... BLOQUÉ (besoin capture)
```

## État
| Tâche | Statut | Preuve |
|---|---|---|
| Arcs inter-modules | en cours (A) | à intégrer |
| Couches dans Monde | en cours (B) | à intégrer |
| Flux exécution 2 ancres | pending | — |
| Canvas unifié | bloc C | design agent C dispo |

## Règles
- À chaque livraison : build TS ✓ + garde 13/13 + commit + statut ici.
- Aucune couche affichée sans données réelles (disabled + raison).
- Vérification visuelle = capture utilisateur obligatoire avant « fini ».
