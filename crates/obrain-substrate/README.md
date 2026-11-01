# obrain-substrate

Substrat « topology-as-storage » d'Obrain : zones mmap, records Pod à taille fixe, WAL natif, index d'embeddings par paliers.

Ce crate fait partie d'[Obrain](https://github.com/obrain-db/obrain), base de données graphe embarquée en Rust. Il est publié pour que le crate `obrain` puisse l'être ; l'API à utiliser est celle d'`obrain`, sauf besoin explicite d'un niveau plus bas.

Licence : Apache-2.0 (voir `LICENSE`). Dérivé de GrafeoDB (voir `NOTICE` à la racine du dépôt).
