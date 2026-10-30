# Obrain Dashboard — composant graph réutilisable (embarqué)

Le Dashboard est conçu comme une **surface de représentation du graphe obrain**
servie par `obrain viz serve`, réutilisable par n'importe quel hôte **connecté
via obrain** (API HTTP RO + URL). Deux modes d'intégration :

## 1) URL paramétrée (iframe / nouvelle fenêtre)
```
GET /?store=Linux complet (v6.6)&center=tcp_v4_rcv&hops=2
```
- `store` = label du preset (ou chemin absolu du store) ;
- `center` = une ou plusieurs ancres (virgule) — suffixe auto-résolu ;
- `hops` = 0 (complet) | 1 | 2 …

Exemple d'embarquement :
```html
<iframe src="http://HOST:8432/?store=Linux%20complet%20(v6.6)&center=tcp_v4_rcv"
        style="width:100%;height:600px;border:0"></iframe>
```

## 2) API pour les hôtes obrain (données, µs-ms)
- `GET /api/info` (taille store), `/api/viz/overview` (monde), `/api/viz/modules`
  (modules réels), `/api/viz/subgraph` (sous-graphe), `/api/functions` (recherche
  par paliers), `/api/query` (GQL/Cypher RO).
- Même origine = pas de CORS ; autre origine = reverse-proxy à ajouter.

## Contrat
- json-graph v1 (gelé, extensions additives optionnelles).
- Lecture seule stricte (RO), aucune écriture depuis la vue.
