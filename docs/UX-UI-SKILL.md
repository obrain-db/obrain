# UX / UI — Compétence de création et de réflexion (obrain-dashboard)

Ce document est la **compétence** à charger avant toute création ou critique
d'interface du Dashboard obrain. Il ne décrit pas *ce que* dessiner mais
*comment* réfléchir, auditer et intégrer.

## 1 · Cadre de pensée (dans l'ordre)
1. **Modèle mental d'abord** : que doit comprendre l'utilisateur en 3 s ? Une
   interface graphique = une carte. Chaque widget doit répondre « où suis-je,
   qu'est-ce que je vois, que puis-je faire ».
2. **Une action primaire par zone** ; le reste en secondaire (ghost), tertiaire
   (chips). Jamais 2 CTA identiques au même endroit.
3. **États complets** : chaque contrôle a 5 états (repos, hover, actif/on,
   disabled+raison, focus) ; chaque vue a 4 états (chargement, vide, erreur,
   données) — jamais d'écran muet.
4. **Perception (gestalt)** : proximité (grouper = même zone), contraste
   (hiérarchie par la couleur/graisse, pas par l'opacité), alignement, répétition
   (un seul langage de surface), fermeture.
5. **Densité vs lisibilité** : le niveau de détail suit l'échelle et l'intention
   (LOD) — jamais de couche affichée « parce qu'elle existe ».

## 2 · Checklist d'intégration (à cocher AVANT de committer de l'UI)
- [ ] Tokens SCSS uniquement (pas de hex inline, pas d'opacité pour la hiérarchie)
- [ ] Palette perceptive (contraste AA, daltonisme : teinte + luminance)
- [ ] Échelle typographique stricte (≤3 tailles) + graisses par rôle
- [ ] Zones de layout : 4 coins = widgets ; centre du canvas libre (jamais 2 panneaux superposés)
- [ ] Mobile : bottom-sheet dessinée (poignée, coins, hauteur bornée, scroll),
      cibles ≥44 px, `env(safe-area-inset-*)`, gestes (pinch) non bloqués
- [ ] A11y : `aria-pressed`, `aria-label`, focus-visible, clavier (Échap, /, ⌘K)
- [ ] Textes FR corrects, concis ; aucune séquence `\uXXXX` brute dans le JSX
- [ ] Motion utile (plongée 300-500 ms, fade), jamais décorative
- [ ] Messages d'état avec action de sortie (« élargir », « fermer », « réessayer »)
- [ ] Vérifié sur les 3 tailles d'écran + données réelles (kernel 1,5 M)

## 3 · Règles spécifiques graphe / data-viz
- **Couleur = sémantique** (CALLS, communautés, ∂, syn…) avec légende interactive.
- **Échelle continue** : Monde → module → fichier → fonction (un seul canvas si possible).
- **Sélection = focus visuel** (les voisins s'allument, le reste s'estompe) — jamais juste un contour.
- **Labels** : LOD piloté par le zoom ; hubs nommés d'abord.
- **Signaux absents = couche disabled avec raison** (✕ + libellé) — pas de gris trompeur.
- **Chaque overlay ajoute une info vraie** ; sinon il est retiré.

## 4 · Process de travail
1. **Brief** : besoins, utilisateur, données réelles dispo, contraintes de perf.
2. **Inventaire** de l'existant (fichiers, atomes, composants, endpoints).
3. **Wireframe** textuel des états (repos/ouvert/sélectionné/mobile).
4. **Review checklist §2** sur le wireframe.
5. **Intégration** : tokens → composant → états → responsive → perf.
6. **Vérification** : capture réelle (desktop + mobile) ; corriger au pixel.

## 5 · Critères de décision entre options
Favoriser l'option qui : respecte le modèle mental (§1.1), ajoute une vraie
information (§3), coûte le moins de surface, et reste vérifiable sans moi
(captures de l'utilisateur) — sinon demander la capture avant d'itérer.
