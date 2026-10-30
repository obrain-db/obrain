# Obrain Dashboard — Base Graph d'organisation

Document vivant qui structure l'avancement sur une **base graphe** : chaque changement
doit être relié à une contrainte, un besoin utilisateur ou un levier — et chaque
levier à une mesure de vérification. Les décisions y sont datées.

## Graphe de synthèse (mermaid)

```mermaid
graph LR
  R1[Besoin: big picture tissée] --> L3[Carte-mère persistante]
  R2[Besoin: tout instantané] --> L2[Index d'agrégats (trie)]
  R3[Besoin: comprendre couches/branches] --> L1[Moteur de trame sémantique]
  R4[Besoin: pas de bruit/doublons] --> L4[Contrat visuel unique]
  R5[Besoin: actions cohérentes] --> L5[Grammaire d'interactions unique]
  L2 --> L1 --> L3
  L4 --> L5
  L1 -->|serveur| E[space/children ~µs]
  E -->|client| G[Monde multi-échelles]
  G --> H[Graphe unitaire]
  F[Décision 2026-09-10] --> L4
  F --> L5
  F -->|ensuite| L2
```

## Cartographie actuelle (vérifiée, mesures du 2026-09-10)

> **Relief du périmètre :8432 (routes, caches, mesures, contrat MCP ~1 ms) : `perimeter-8432.md`**

- **Endpoint agrégats** : modules · arcs · space (ensembles ± arcs) · overview · query — caches **durables disque+mémoire** (mtime), < 1 ms aux revisites, warm au déploiement.
- **Space** : clé `(store,prefix,rank,links)` · hover/rameaux `links=0` · préchauffage auto des enfants (top-3) en arrière-plan.
- **Monde** : grammaires auto (projets+branches / tissu force / familles) · lignée persistante · branches auto 2 sauts · cascade 3 sauts au clic · survol sans doublon · épinglage multi · vue du dessus · pages `?store=&dive=` + titres.
- **Code concerné (base graph du dépôt)** : `StoreOverview.tsx` = monolithe (~1200 l) à fort centre de gravité — prochain candidat à découper une fois L4/L5 stabilisés ; `api.ts` client ; `viz_serve.rs` (agrégats+caches) ; `vizserve.sh` (warm).

## Décisions datées

| Date | Décision | Lié à |
|---|---|---|
| 2026-09-10 | Grammaire fantômes unifiée : `↰` ancêtre, `↳` fils (niveau 1), `·` petits-fils (niveau 2) — supprimer `▸`/`»`/`↓` ad hoc | L4/L5 |
| 2026-09-10 | Hover silencieux si les fils sont déjà déployés | R4 |
| 2026-09-10 | Agrégats persistés disque (pas seulement mémoire) + warm déploiement | R2 |
| 2026-09-10 | `links=0` pour hover/rameaux ; préchauffage auto niveau courant | R2 |
| 2026-09-10 | Étape A : grammaire fantômes unifiée `↰/↳/·` ; sélection sans doublon | L4/L5 |
| 2026-09-10 | Étape B : clavier (Échap/←/Entrée/R/T/+/-) | L5 |
| 2026-09-10 | Étape C-lite : préchauffage cascade petits-enfants (serveur) | L2/R2 |
| 2026-09-10 | Étape D : carte-mère big picture (mini-carte + repère de vue) | L3 |
| 2026-09-10 | Transitions fluides : pose caméra conservée entre niveaux, fondu canvas, étiquettes animées, chargement dans le canvas | L4/R2 |

## Backlog ordonné (petits pas vérifiés)

- [x] D4 : hover sans doublon · agrégats durables · warm · links=0 · préchauffage auto
- [x] D3 : branches auto 2 sauts (racine + niveaux projets) · cascade 3 sauts
- [ ] **Étape A (L4)** : grammaire unifiée des fantômes (nettoyage glyphs/CSS) — *en cours*
- [ ] Étape B (L5) : raccourcis clavier + zéro double affichage systématique
- [ ] Étape C (L2) : index d'agrégats précalculé côté moteur (trie) → space/children ~µs partout
- [ ] Étape D (L3) : carte-mère persistante / mini-carte big picture
- [ ] Étape E (L1) : moteur de trame sémantique (squelette contenance + densité)
- [ ] Étape F : découpage `StoreOverview.tsx` (layout / draw / interactions)

## Règle de travail
Chaque itération : 1) choix relié au graphe ci-dessus → 2) implémentation minimale → 3) build + vérif live (rendu/DOM/réseau/erreurs) → 4) mise à jour de ce document.

## Matrice d'état (audit 2026-09-10) — « toutes les bases sont-elles enrichies ? »

| Base | Contenu | État | Vérifié |
|---|---|---|---|
| Données (agrégats) | modules/arcs/space/overview/query + caches durables disque+mémoire, warm déploiement | ✅ enrichie | oui (<1 ms revisites) |
| Représentation | projets/branches, tissu, familles, cascade 3 sauts, branches auto | ✅ | oui (live) |
| Révélation progressive | liste complète (200) conservée → « ⤵ révéler +N » par paliers (0 troncature cachée) | ✅ | oui (48→51, 0 erreur) |
| Navigation | fil d'échelle, clavier, vue du dessus, carte-mère, épinglage | ✅ (+aide H) | oui |
| Transitions | chip de chargement canvas, fondu, pose caméra conservée, labels animés | ✅ | oui (39 rAF/s idle, 0 erreur) |
| Fluidité dense | étiquettes DOM projetées à 10 Hz (WebGL 60 FPS) — ~11 rAF/s en headless logiciel à 48-68 labels | ✅ | oui (E2E) |
| LOD étiquettes par distance | expérimenté puis RETIRÉ (rendu cassé en aveugle) — à reprendre avec validation visuelle | ⏳ | — |
| Traversée | historique réel (pushState/popstate) — précédent/suivant entre niveaux | ✅ | oui (E2E) |
| Pages | `?store=&dive=` + titre + URL réécrite | ✅ | oui |
| Monitoring | `GET /api/viz/status` (uptime, caches par type, stores) | ✅ | oui (arcs 5 · mod 6 · sp 49 · 6 stores) |
| Aide (bis) | overlay clavier (H/?) + status + smoke + e2e | ✅ | oui |
| Aide | overlay clavier (H/?) | ✅ | construit |
| Backend moteur (C-full) | index trie ~µs partout | ⛔ manque : API itération moteur | — |
| Sitemap/pages XML | `/sitemap.xml` (stores + projets, URLs encodées) | ✅ | oui (200, <url>) |
| Qualité code | `StoreOverview.tsx` monolithe ~1300 l | ⏳ Étape F (découpage) | — |
| Tests automatisés | `scripts/check-all.sh` → smoke (seuils réels) + e2e | ✅ | oui (SMOKE OK / E2E OK) |

→ Bases fonctionnelles **enrichies et raccord** ; restent : C-full (moteur), Étape E (trame), F (découpage), sitemap XML, tests.
