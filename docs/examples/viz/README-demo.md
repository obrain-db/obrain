# obrain viz — sous-graphe de démonstration (consommateur web)

Sous-graphe **réel** extrait du store `engine_idx.obrain` (banc-obrain), centre
`substitute_in_operator`, 1 hop — même vérité que le banc de pertinence LLM :
`2404 -> 2404 [label="CALLS"]` (récursivité) est présent dans le DOT ci-dessous.

## Fichiers

| Fichier | Contenu |
|---|---|
| [`sub.json`](sub.json) | json-graph v1 (source de vérité, 5 nœuds / 5 arêtes) |
| [`sub.svg`](sub.svg) | rendu Graphviz réel (WASM `@viz-js/viz`) — à ouvrir dans le navigateur |
| [`viewer.html`](viewer.html) | vue interactive **sans dépendance** : clic sur un nœud → voisins en évidence |

## DOT (rendu natif par les artefacteurs markdown/GUI)

```dot
digraph obrain_viz {
    "2373" [label="__file__"] [tooltip="/CHEMIN/UTILISATEUR/projects/banc-obrain/crates/obrain-engine/src/query/processor.rs"];
    "2403" [label="substitute_params"] [tooltip="/CHEMIN/UTILISATEUR/projects/banc-obrain/crates/obrain-engine/src/query/processor.rs"];
    "2404" [label="substitute_in_operator"] [tooltip="/CHEMIN/UTILISATEUR/projects/banc-obrain/crates/obrain-engine/src/query/processor.rs"];
    "2405" [label="resolve_count_param"] [tooltip="/CHEMIN/UTILISATEUR/projects/banc-obrain/crates/obrain-engine/src/query/processor.rs"];
    "2406" [label="substitute_in_expression"] [tooltip="/CHEMIN/UTILISATEUR/projects/banc-obrain/crates/obrain-engine/src/query/processor.rs"];
    "2373" -> "2404" [label="DEFINES"];
    "2403" -> "2404" [label="CALLS"];
    "2404" -> "2404" [label="CALLS"];
    "2404" -> "2405" [label="CALLS"];
    "2404" -> "2406" [label="CALLS"];
}
```

## Mermaid (rendu natif dans n'importe quel markdown : GitHub, Obsidian, GUI)

```mermaid
flowchart LR
    nf598030b21eb96be["substitute_params"]
    nf598060b21eb9bd7["substitute_in_operator"]
    nf598050b21eb9a24["resolve_count_param"]
    nf598080b21eb9f3d["substitute_in_expression"]
    nf598030b21eb96be -->|CALLS| nf598060b21eb9bd7
    nf598060b21eb9bd7 -->|CALLS| nf598050b21eb9a24
    nf598060b21eb9bd7 -->|CALLS| nf598080b21eb9f3d
```

## Commandes de régénération

```bash
obrain viz subgraph <engine_idx.obrain> substitute_in_operator \
  --hops 1 --limit 12 --as json > sub.json
obrain viz subgraph <engine_idx.obrain> substitute_in_operator \
  --hops 1 --limit 12 --as dot  > sub.dot
# puis DOT -> SVG via @viz-js/viz (WASM) :
#   node --input-type=module -e "const {instance}=await import('@viz-js/viz');
#   const viz=await instance();process.stdout.write(viz.renderString(dot,{format:'svg'}));"
```
