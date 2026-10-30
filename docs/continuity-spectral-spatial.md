# Continuité spectrale/spatiale (obrain ↔ aletheia ↔ harness) — design doc

Statut : annexe vérifiée (2026-09-04). Relie les analyses spectrale & spatiale
déjà implémentées à l'injection de continuité du harness (CH6), pour la
reconstruction de contexte au-delà de la limite de fenêtre.

## Thèse
La continuité longue ≠ rechargement du log linéaire ; c'est une RECONSTRUCTION
depuis le graphe :
    min|M|  s.t.  D(Reconstruct(M), Elun) < ε
où le rappel sélectionne M par **signature spectrale (forme)** + **encodage
spatial (localité Hilbert)** des ensembles co-activés (engrams), alimentés par
le weave `TOUCHES` (sessions↔fichiers).

## Preuves (fichier:ligne vérifiés ce jour)
- Spectrale graphe : `crates/obrain-adapters/src/plugins/algorithms/spectral.rs`
  (`spectral_embedding`, Laplacien normalisé, eigvecs, 4d) — lignes 20-25/74/329.
- Spectrale ensembles : `crates/obrain-cognitive/src/engram/spectral.rs`
  (`SpectralEncoder`, signature compacte d'ensembles de nœuds → rappel co-activation).
- Spatiale : `…/algorithms/hilbert.rs` + `hilbert_features.rs` (cascade
  multi-résolution 8 facettes × 8 niveaux = 64d ; facettes 0-1 = spectral).
- Chaîne runtime : bridge (`mcp_server.rs`) expose `memory_recall`,
  `context_build`, `import_events` (GraphWriter → engram_manager) — 28
  références vérifiées.
- Weave : arêtes `TOUCHES` sessions↔fichiers dans le runtime = **16 889**
  (mesuré, 2026-09-04 ; toute valeur antérieure ≠ à rapporter à cette date).
- Rappel mesuré : engrams actifs (rappel ~0,48 ms, confiance 0,7 ; persistance
  substrate `engram_members`/`engram_bitset`) — à re-mesurer sur la base full.

## Correspondance CH6 (harness)
- `context_build` (injection composée par session) = squelette existant ;
- chaînon à enrichir : fusion du rappel **spectral-ensembles** (`memory_recall`
  sur engrams) dans le payload injecté, cadré par la fenêtre maxTokens ;
- validation mécanique : doc 11 §9.9 (aletheia).

## Hooks d'implémentation candidats (à choisir)
1. context_build + memory_recall enrichi (injection par forme spectrale + localité) ;
2. persistance de la signature spectrale d'ensemble à l'import_events ;
3. validation numérique : facettes spectrales 0-1 de la cascade vs
   `spectral_embedding` (cohérence), rayon spectral vs calibrage aletheia.
