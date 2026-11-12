# obrain-bindings-common

Socle partagé des liaisons de langage d'Obrain : classification des erreurs
(`error`), extraction des nœuds et arêtes d'un `QueryResult` (`entity`) et
conversion `serde_json::Value` ⇄ `obrain Value` (`json`). Chaque liaison
(C, Python, Node.js…) en dépend et ne fait que projeter ces types sur son
mécanisme d'interface.

Ce crate fait partie d'[Obrain](https://github.com/obrain-db/obrain), base de
données graphe embarquée en Rust. Il est publié pour que les liaisons puissent
l'être ; il n'a pas d'API destinée à un usage direct.

Licence : Apache-2.0 (voir `LICENSE`). Dérivé de GrafeoDB (voir `NOTICE` à la
racine du dépôt).
