# RFC — Isolation native : le principal descend dans le substrat

**Statut** : brouillon — conception seulement, aucune ligne de production touchée.
**Date** : 2026-09-12 — branche `docs/principal-natif` (worktree `banc-obrain-rfc`),
dérivée de HEAD `afbb5cfc`.
**Auteur** : harness RFC-PRINCIPAL (dispositif Ergaster), chantier `PRINCIPAL-NATIF`.

**Convention de lecture.** Toute affirmation sur le code existant est citée
`fichier:ligne` et provient d'un `grep`/`sed` lancé pendant la rédaction. Ce qui
est **PROPOSÉ** (la conception) est marqué comme tel. Là où le brief décrivait un
mécanisme que je n'ai pas retrouvé, je le dis explicitement — voir §0.

---

## 0. Ce que l'audit du code a CONFIRMÉ, INFIRMÉ, ou NUANCÉ dans le brief

Avant de trancher, le relevé honnête, parce qu'un RFC qui part d'un diagnostic
faux oriente l'architecture sur du vent.

- **CONFIRMÉ — le moteur n'a aucun *token* `tenant`/`principal`.** `grep -ni`
  sur `crates/obrain-engine`, `crates/obrain-substrate`, `crates/obrain-core`
  ne rend que du bruit (`maintenant`, `contenant`, `tenant un verrou`, un label
  de test `:Tenant`). Aucune notion nommée « tenant » ni « principal ».

- **NUANCÉ — mais le moteur a DÉJÀ une notion d'identité de lecteur, sous un
  AUTRE vocabulaire.** Elle existe à deux endroits :
  - `AccessContext` (`crates/obrain-core/src/execution/base_registry.rs:246`) :
    `{ reader: String, decide: Box<AccessDecider>, audit }`. Elle voyage dans la
    session (champ `access_context: Mutex<Option<Arc<AccessContext>>>`,
    `crates/obrain-engine/src/session/mod.rs:124`, commentaire « V3 … `None` = accès
    total »).
  - Le crate `obrain-iam` (`crates/obrain-iam/src/lib.rs`) : système **ORN**
    (`orn:obrain:{service}:{account_id}:{resource_type}/{path}`), `AuthProvider`,
    `Identity`, `PolicyDecision`, et une `AuthenticatedSession`
    (`crates/obrain-iam/src/session_ext.rs:38`).

  **Conséquence directe pour la conception : il ne faut PAS créer un troisième
  vocabulaire.** Le RFC réutilise `AccessContext.reader` et l'`account_id` d'ORN,
  et l'aligne sur ce que `this-rs` exprime déjà (§1).

- **INFIRMÉ — « le `NodeRecord` a de la place pour un `tenant_id u32 »`.** FAUX.
  Le `NodeRecord` fait **exactement 32 B, sans un octet libre**, assertions à la
  compilation à l'appui (`crates/obrain-substrate/src/record.rs:76-105`). Ajouter
  un `u32` est une **rupture de format** (32 B → 40 B avec l'alignement 8), pas un
  remplissage de trou. C'est la question §3, et elle change la réponse.

- **CONFIRMÉ — la garde vit AU-DESSUS du substrat, l'accès direct l'ignore.**
  - La viz `:8432` `/api/query` ouvre le store et exécute la requête **sans
    principal** (`crates/obrain-cli/src/viz_serve.rs:1037` → `cached_ro` →
    `db.session().execute(q)`, aucune identité en jeu).
  - `AuthenticatedSession` est un objet **compagnon**, pas un emballage : le
    commentaire de `session_ext.rs:1-22` le dit — le middleware crée un `Session`
    ET une `AuthenticatedSession`, puis doit *penser* à appeler
    `check_access(...)` **avant** `session.execute(...)`. Rien dans le type
    n'oblige cet appel. C'est la définition d'une convention, pas d'une garantie.

- **CONFIRMÉ — la faille « 4 segments » échoue OUVERT.** Détail en §5.

**Thèse du RFC, reformulée à la lumière du code :** obrain a une identité de
lecteur (`AccessContext`) mais elle n'est câblée QUE sur une frontière (la
résolution inter-bases `ref()`, §2) et à une granularité GROSSIÈRE (par *slug* de
store, pas par nœud). Toute lecture locale d'un store unique — le cas de la viz,
le cas dominant — ne la traverse jamais. **L'isolation doit descendre là où
passe *chaque* lecture de nœud : le prédicat de visibilité du substrat.**

---

## 1. La forme de l'identité (décision 1)

### Ce qui existe

- `this-rs` construit son identité depuis un JWT. `StsClaims`
  (`this-rs/src/core/auth/sts.rs:16`) porte : `sub: String` (le principal),
  `tenant_id: Uuid`, `roles: Vec<String>`. Le provider mappe ça vers
  `AuthContext` (`this-rs/src/core/auth/wami_provider.rs:211`) : variantes
  `User { user_id, tenant_id, roles }`, `Service { service_name, tenant_id }`,
  `Admin { admin_id }`, `Anonymous`. **Le tenant est un `Uuid` (128 bits).**
- `this-rs` matérialise la décision d'autorisation en **propriétés indexées**
  sur le nœud du principal — `acl_key`, `acl_scope`, `acl_digest`, `acl_perms`,
  label `Acl` (`this-rs/src/storage/iam.rs:108`, doc-module `:22-30`) — parce
  que « `SubstrateStore` n'expose aucune API blob publique », seuls
  `create_property_index` / `find_nodes_by_property` le sont.
- Côté moteur, `AccessContext` porte un `reader: String`
  (`base_registry.rs:246`) et vit sur la `Session` (per-session), pas sur la
  base — le store `Arc<dyn GraphStoreMut>` est partagé entre sessions
  (`session/mod.rs:82`).

### Décision

**Le principal vit sur la SESSION, pas sur `ObrainDB::open`.** On garde
`ObrainDB::open(path)` inchangé (`crates/obrain-engine/src/database/mod.rs:387`,
délègue à `open_substrate`) et on introduit un constructeur de session porteur
d'identité — PROPOSÉ :

```rust
// PROPOSÉ (illustration de signature, non implémenté)
impl ObrainDB {
    pub fn session_as(&self, principal: Principal) -> Session { /* … */ }
}

// PROPOSÉ — réutilise le vocabulaire déjà présent, n'en crée pas un troisième.
pub struct Principal {
    reader: String,          // = AccessContext.reader / ORN account_id / StsClaims.sub
    tenant: TenantId,        // interné (voir §3) depuis le Uuid de StsClaims.tenant_id
    roles: Vec<ArcStr>,      // = StsClaims.roles ; consommé par obrain-iam, PAS par le substrat
}
```

**Motivation.** La base est partagée entre lecteurs concurrents
(`session/mod.rs:82`, store `Arc`) ; l'identité est une propriété *de la
session*, pas *de la base*. Le champ `access_context` de `Session` EXISTE
déjà pour ça — on l'étend au lieu d'ajouter un canal parallèle. Un principal posé
« après ouverture, mutable par transaction » est rejeté : l'identité n'est pas
une donnée transactionnelle (elle ne *commit* ni ne *rollback*), c'est le
contexte immuable du lecteur pour toute la durée de la session.

**Alternatives écartées.**
- `open_as(path, principal)` : lierait un principal à *l'ouverture du fichier*.
  Faux niveau — un serveur multi-tenant ouvre le store une fois et sert N
  tenants. Rejeté.
- Principal mutable par transaction : brouille identité et unité d'atomicité ;
  ouvre la porte à une élévation de privilège intra-session. Rejeté.

**Ce que le substrat consomme vs ce que les couches consomment.** Seul `tenant`
descend dans le substrat (§2, §3). `roles`/permissions restent au-dessus, dans
`obrain-iam` et les ACL matérialisées de `this-rs` (`iam.rs`) — le natif fait du
**confinement de tenant**, pas du RBAC (voir §« ne fait pas »). On ne duplique
donc pas la logique de rôles dans le moteur.

---

## 2. Le point de filtrage (décision 2)

### Ce qui existe — le prédicat de visibilité unique

Le filtre MVCC se réduit à UNE fonction par nœud, `noeud_visible_pour`
(`crates/obrain-substrate/src/store.rs:3808`) :

```rust
// EXISTANT — crates/obrain-substrate/src/store.rs:3808 (extrait)
fn noeud_visible_pour(&self, id: NodeId, epoque: u64, tx: Option<u64>) -> bool {
    if let Some(proprietaire) = self.en_attente_noeuds.get(&id)
        && tx != Some(*proprietaire.value())
    {
        return false;                       // écriture d'une autre tx : invisible
    }
    self.noeud_vivant_a(id, epoque)         // né avant l'époque, pas encore mort
}
```

Tous les chemins de lecture y convergent : le filtre par lot
`filtrer_par_epoque_et_tx` (`store.rs:3779`) l'appelle par nœud, et le chemin
rapide `visibilite_triviale()` (`store.rs:3822`, une seule lecture atomique
`!mvcc_actif`) court-circuite vers `filtrer_noeuds_vivants` quand aucune carte
MVCC n'est peuplée — l'état permanent d'une base sans transactions explicites
(le SCAN l'a mesuré : se réduit à `!is_tombstoned()`).

Le trait `GraphStore` (`crates/obrain-core/src/graph/traits.rs`) expose les
surfaces à couvrir : `get_node:91`, `get_node_versioned`, `get_node_at_epoch`,
`node_ids:258`, `all_node_ids:265`, `find_nodes_by_property:385`,
`all_nodes:1120`, `nodes_with_label:1131`, et les traversees`edges_from` (§145+).

### Décision

**Le ET des deux filtres est évalué UNE SEULE FOIS, au même endroit :
`noeud_visible_pour`.** On l'étend — PROPOSÉ :

```rust
// PROPOSÉ — le prédicat devient un ET à trois facteurs.
fn noeud_visible_pour(&self, id: NodeId, epoque: u64, tx: Option<u64>,
                      principal: Option<&Principal>) -> bool {
    // (1) visibilité transactionnelle (inchangé)
    if let Some(p) = self.en_attente_noeuds.get(&id) && tx != Some(*p.value()) {
        return false;
    }
    // (2) vivant à l'époque (inchangé) ET (3) autorisé au principal (nouveau)
    self.noeud_vivant_a(id, epoque) && self.autorise_au_principal(id, principal)
}
```

Formellement : **un nœud est visible ⟺ (vivant à l'époque) ∧ (non en attente
d'autrui) ∧ (autorisé au principal).** Le troisième facteur, `autorise_au_
principal`, lit le tenant du nœud (§3) et le compare au tenant du principal.

**Où, une fois ou par méthode ?** Une seule fois, dans `noeud_visible_pour` —
c'est le point où *tout* le trait converge déjà. Les méthodes du trait
(`node_ids`, `find_nodes_by_property`, `neighbors`, …) ne réimplémentent PAS le
ET ; elles continuent d'appeler le filtre par lot, qui appelle
`noeud_visible_pour`. C'est exactement l'argument architectural du RFC : un seul
point de vérité, impossible à contourner par une nouvelle méthode d'accès.

**Composition avec le chemin rapide.** Piège identifié : `visibilite_triviale()`
court-circuite le filtre MVCC quand aucune tx n'est ouverte, et retombe sur
`filtrer_noeuds_vivants` — qui n'appelle PAS `noeud_visible_pour`. Si on ne fait
rien, **le filtre de principal serait sauté sur le chemin chaud**, c'est-à-dire
précisément le cas de la viz. Donc — PROPOSÉ : le court-circuit MVCC reste, mais
il est gardé par `principal.is_none()`. Quand un principal est posé, le lot passe
par un chemin qui applique (3) même en l'absence de MVCC. Le coût de (3) doit
donc être négligeable (§3) — sinon on retaxe le chemin chaud qu'on protège.

**Traversées.** `neighbors` / `edges_from` doivent filtrer la **cible** par le
même prédicat : une arête vers un nœud d'un autre tenant ne doit rendre ni le
voisin ni l'arête. Sinon l'isolation fuit par les jointures.

---

## 3. Le coût — colonne vs propriété (décision 3)

### Ce qui existe — le `NodeRecord` est PLEIN

`crates/obrain-substrate/src/record.rs:76-102` :

```rust
// EXISTANT — 32 B, aligné 8. AUCUN champ de padding, AUCUN réservé.
pub struct NodeRecord {
    pub label_bitset: u64,        //  8 B  (bit i ⇒ label i, « up to 64 labels »)
    pub first_edge_off: U48,      //  6 B
    pub first_prop_off: U48,      //  6 B
    pub community_id: u32,        //  4 B
    pub energy: u16,              //  2 B
    pub scar_util_affinity: u16,  //  2 B  (5b|5b|5b|1b — plein)
    pub centrality_cached: u16,   //  2 B
    pub flags: u16,               //  2 B  (bits 0..5 utilisés, record.rs:66-72)
}   // total = 32 B, assert size==32 && align==8 (record.rs:104-105)
```

**Espace libre chiffré :**
- **Octets libres : 0.** 8+6+6+4+2+2+2+2 = 32 = la taille exacte. Pas de trou.
- **Bits libres : les `flags`.** `node_flags` (`record.rs:66-72`) définit 6
  drapeaux (`TOMBSTONED`…`IDENTITY`, bits 0-5) sur un `u16` → **10 bits libres**.
- `label_bitset` est un `u64` (capacité 64 labels) — ses bits inutilisés sont
  RÉSERVÉS aux labels, pas disponibles pour un tenant.

### Le fait qui décide

Lire une **colonne** du `NodeRecord` est une lecture directe dans le mmap, même
classe que `label_bitset` ou `is_tombstoned()` : **~ns**. Un
`find_nodes_by_property` (le chemin des ACL de `this-rs`) est un lookup indexé :
**~µs**. Un prédicat de tenant s'exécute à *chaque* lecture de nœud (§2) — il
doit donc être une colonne, pas une propriété.

Mais §0 l'a établi : il n'y a **pas de place** pour un `u32`. Deux voies :

- **Voler des bits de `flags`.** 10 bits libres → jusqu'à 1024 tenants sans
  toucher au format 32 B. Séduisant (zéro rupture), mais : (a) plafonne le
  nombre de tenants à 1024 ; (b) mélange l'identité de tenant avec des drapeaux
  d'état (risque de corruption sémantique) ; (c) `this-rs` a des tenants `Uuid`
  — 1024 slots via interne, mais le plafond est bas pour du multi-tenant réel.
- **Bump de format 32 B → 40 B, colonne `tenant_id: u32` dédiée.** Rupture de
  format — mais **le précédent existe dans le même fichier** : `EdgeRecord` est
  passé de 32 B à 36 B en T17f (`record.rs`, doc de `first_prop_off` :
  « layout bump 32 B → 36 B », assert `size==36` `:309`). Le coût : +8 B/nœud
  (25 %) à cause de l'alignement 8 (36 arrondi à 40).

### Décision

**Colonne `tenant_id: u32` dédiée, bump 32 B → 40 B.** La lecture est directe
(~ns), le prédicat §2 est donc quasi-gratuit et le chemin chaud reste chaud.
On accepte la rupture de format (précédent `EdgeRecord`, plan de migration §4).

**Uuid → u32 : dictionnaire de tenants (PROPOSÉ).** `this-rs` a un tenant `Uuid`
(128 bits) ; il ne rentre pas dans une colonne u32. On interne le `Uuid` en un
`TenantId(u32)` via un dictionnaire `Uuid ↔ u32`, exactement le patron déjà
utilisé pour les chaînes (`substrate.dict`, `substrate.strings`) et les labels
(interning en id de bit). `tenant_id = 0` est **réservé** au sens « public /
sans tenant » (§4). La colonne stocke le `TenantId` interné, jamais le `Uuid`.

**Alternative écartée.** Le tenant en **propriété** (comme les ACL de `this-rs`) :
correct fonctionnellement mais µs par nœud sur le chemin chaud — c'est ce que
l'opérateur refuse explicitement. On le garde toutefois comme **jalon
intermédiaire** (§6, M0) : il protège *avant* le bump de format.

---

## 4. La compatibilité — millions de nœuds sans tenant (décision 4)

### La contrainte

Les stores actuels ont des millions de nœuds **sans** `tenant_id`. Le
`NodeRecord` est `Zeroable` (`record.rs`, dérive `Pod, Zeroable`) : une zone qui
grandit est **remplie de zéros**. Donc un `tenant_id` ajouté par croissance de
format vaut **0** par défaut, gratuitement, sans réécriture.

Le principe de durabilité de la semaine interdit « une migration qui réécrit tout
d'un coup sans WAL ».

### Décision

**`tenant_id = 0` signifie « sans tenant / public », visible de tous.** Et —
distinct — **un store ouvert SANS principal (`principal = None`) voit tout**
(comportement historique mono-base, cohérent avec `AccessContext` dont le
`None` = « accès total », `base_registry.rs:244`).

Règle de visibilité de `autorise_au_principal` — PROPOSÉ :

| principal | tenant du nœud | visible ? |
|---|---|---|
| `None` (accès total) | quelconque | oui |
| `Some(t)` | `0` (public) | oui |
| `Some(t)` | `t` | oui |
| `Some(t)` | `u ≠ t`, `u ≠ 0` | **non** |

**Migration : champ par défaut à l'ouverture, PAS de rescan.** Le `tenant_id`
apparaît via le mécanisme normal de montée de version de format (zone
zéro-remplie ⇒ tenant 0). Aucune réécriture des millions de nœuds au démarrage.
Le re-tenanting explicite (attribuer un tenant réel à des nœuds hérités) est une
passe de fond **journalisée par le WAL**, idempotente, jamais un stop-the-world.

**Fail-open assumé et borné.** Ce défaut à 0 = public est un fail-*semi*-open
délibéré, justifié : (a) c'est la seule valeur qu'un enregistrement hérité peut
porter sans réécriture ; (b) un défaut fail-closed ferait *disparaître* tout
store existant à la première ouverture avec principal. Le durcissement (basculer
le défaut `None`→fail-closed pour les serveurs exposés) est une décision séparée
(§6, M3) — pas un prérequis à la protection.

**Alternative écartée.** Fail-closed dès le défaut (nœud sans tenant = invisible)
: casse instantanément tous les stores et force le rescan interdit. Rejeté.

---

## 5. Le lien avec la faille des 4 segments (décision 5)

### La faille, vérifiée dans le code

`this-rs` route l'autorisation par **nombre de segments** d'URL.
`Authorization::requirement` (`this-rs/src/server/middleware/authorization.rs:543`)
ne reconnaît que les formes à **1, 2, 3 ou 5** segments (`:557-580`) ; toute autre
forme rend `Requirement::Abstain` (« forme non reconnue »). `decide` propage en
`Decision::Abstain`. Et le middleware — c'est le cœur :

```rust
// EXISTANT — this-rs/src/server/middleware/authorization.rs:724
match decision {
    Decision::Allow { .. } | Decision::Abstain { .. } => next.run(request).await,
    Decision::Deny { .. } => { /* … */ }
}
```

**`Abstain` échoue OUVERT** : la requête passe. Une route à 4 segments (ex.
`/pages/42/foo/bar`) tombe hors des formes reconnues → `Abstain` → **passe sans
contrôle**. (Le doc-comment `:607` « Fail-closed : illisible ⇒ refus » ne vaut
que pour une *politique illisible*, pas pour `Abstain`.)

### La démonstration — l'argument le plus fort du natif

**Établi, avec une nuance précise.**

Si l'isolation est native au substrat (§2, §3), alors même une requête qui
échappe au middleware par la faille de route atteint bien `session.execute(q)` —
mais la session **porte un principal**, et le store ne rend que les nœuds du
tenant de ce principal. **La fuite CROISÉE (voir les données d'un AUTRE tenant)
devient impossible quel que soit le chemin HTTP** : le store refuse la donnée,
la route n'a plus le dernier mot. C'est l'argument central du RFC, et il tient.

**La nuance qui empêche la surpromesse (réfutation partielle) :**
- Le natif neutralise la **confidentialité inter-tenant**, PAS l'autorisation
  d'opération. Une route à 4 segments qui échappe au middleware laisse toujours
  passer un `delete` non autorisé *à l'intérieur du tenant du principal* — le
  substrat ne sait rien des rôles (§1, ils restent au-dessus).
- Le natif ne corrige pas la sur-permission **intra-tenant** (un user qui voit
  des données de son propre tenant qu'un rôle devrait masquer). C'est le domaine
  des ACL matérialisées de `this-rs` (`iam.rs`), au-dessus du substrat.
- **Le cas viz reste ouvert tant que la viz n'a pas de principal.**
  `viz_serve.rs:1037` ouvre le store sans principal → `principal = None` → voit
  tout (§4). Le natif ne *ferme* ce trou que si la viz est ouverte AVEC un
  principal, ou si le défaut bascule fail-closed (M3). C'est la limite honnête :
  le natif rend la protection *possible et non-contournable par la route*, il ne
  l'*active* pas à la place de l'appelant qui choisit `None`.

**Bilan.** Faille de route rendue sans objet **pour la confidentialité
inter-tenant** — l'argument fort est vrai. Elle reste pertinente pour
l'autorisation d'opération et l'intra-tenant. Le RFC ne prétend pas la première
remplacer les secondes.

---

## 6. Impact par crate

- **`obrain-substrate`** (`record.rs`, `store.rs`) : cœur du RFC. Colonne
  `tenant_id` dans `NodeRecord` (bump 32→40 B, §3), dictionnaire `Uuid↔u32`,
  extension de `noeud_visible_pour` (§2), garde du chemin rapide
  `visibilite_triviale` par `principal.is_none()`.
- **`obrain-core`** (`graph/traits.rs`, `execution/base_registry.rs`) : le
  prédicat de principal doit être exprimable au niveau du trait `GraphStore`
  (le paramètre `principal` circule via le `SessionContext`, PAS en ajoutant
  un argument à chaque méthode — on le porte dans le contexte de session, comme
  `AccessContext` circule déjà). Réutilise le `reader` d'`AccessContext`.
- **`obrain-engine`** (`session/mod.rs`, `database/mod.rs`) : `session_as(principal)`
  (§1) ; câblage du principal dans `SessionContext` → jusqu'au store.
  `ObrainDB::open` inchangé.
- **`obrain-iam`** (`session_ext.rs`, `orn.rs`, `policy.rs`) : cesse d'être un
  compagnon facultatif pour le confinement de tenant — c'est lui qui construit le
  `Principal` (mappe `Identity`/ORN `account_id` → `TenantId` interné). Les rôles
  et `check_access` (RBAC) restent chez lui, au-dessus.
- **`obrain-cli`** (`viz_serve.rs`) : `api_query` doit ouvrir la session **avec**
  un principal (ou un principal explicite « lecteur viz » borné à un tenant).
  Sans ça, la viz reste `None` = accès total (§5).
- **`this-rs`** (`storage/obrain.rs`, `storage/iam.rs`, `storage/tenant_scoped.rs`,
  `core/auth/*`) : le `tenant_id: Uuid` de `StsClaims` devient la source du
  `TenantId` natif. La tenancy par graphe nommé (`space: RwLock<Option<String>>`,
  `obrain.rs:87` ; `TenantScopedLinkService`, `tenant_scoped.rs:54`) peut, à
  terme, s'appuyer sur le confinement natif plutôt que le réimplémenter — hors
  périmètre de CE RFC (voir « ne fait pas »).

---

## 7. Jalons (le premier protège le plus tôt)

- **M0 — protéger sans rupture de format (le plus tôt).** Threader un
  `Principal` optionnel dans `Session`→`SessionContext`→store, et appliquer le
  prédicat de tenant §2 en lisant une **propriété** `tenant` (µs, via le patron
  `find_nodes_by_property`/`get_node_property` déjà utilisé par les ACL de
  `this-rs`). Faire passer les lecteurs exposés (viz `api_query`, `this-rs`) par
  un principal. **Ce jalon ferme la fuite inter-tenant tout de suite**, plus lent
  mais correct, sans toucher au format du record.
- **M1 — colonne native (~ns).** Dictionnaire `Uuid↔u32`, bump `NodeRecord`
  32→40 B, colonne `tenant_id`, migration de la lecture du prédicat de la
  propriété (M0) vers la colonne. Montée de version de format gardée par le WAL.
- **M2 — re-tenanting des nœuds hérités.** Passe de fond journalisée (WAL,
  idempotente) qui attribue un `TenantId` réel aux nœuds actuellement à 0 qui
  appartiennent en fait à un tenant. Câblage de l'audit (réutilise
  `AccessContext.emit_audit`, `base_registry.rs:301`).
- **M3 — durcissement fail-closed.** Basculer le défaut `principal = None`
  vers fail-closed pour les ouvertures exposées au réseau ; garder l'accès total
  pour l'outillage local mono-base derrière un drapeau explicite.

---

## 8. Ce que ce RFC NE fait PAS (honnêtement)

- **Pas de RBAC natif.** Le substrat confine par tenant ; il ne connaît ni rôles
  ni permissions. `roles`/`check_access` restent dans `obrain-iam` et les ACL
  matérialisées de `this-rs` (`iam.rs`). Le natif ne remplace pas la §1 de leur
  logique d'autorisation, seulement la frontière de confidentialité.
- **Pas de filtrage au niveau propriété ou arête-attribut.** On rend visible/
  invisible un *nœud* (et, par cascade, ses arêtes). On ne masque pas une
  propriété individuelle d'un nœud visible.
- **Pas de chiffrement au repos.** Un tenant ayant accès brut au fichier du store
  lit tout. Le confinement est logique, pas cryptographique.
- **Ne ferme pas le trou viz à la place de l'appelant.** Tant que
  `viz_serve.rs:1037` choisit `None`, la viz voit tout (§5). Le RFC rend la
  fermeture *possible et non-contournable par la route* ; il ne force pas
  l'appelant à poser un principal (c'est M0/M3).
- **Ne remplace pas `AccessContext` inter-bases.** La garde par *slug* sur la
  résolution `ref()` (`store_for_reader`, `base_registry.rs:192`) reste : elle
  répond à une question différente (« ce lecteur peut-il atteindre CE store »)
  de celle du RFC (« ce lecteur peut-il voir CE nœud »). Les deux coexistent.
- **Ne migre pas la tenancy par graphe nommé de `this-rs`.** Remplacer
  `space`/`TenantScopedLinkService` par le confinement natif est un chantier
  ultérieur, pas celui-ci.
- **N'implémente rien.** Conception seule. Les extraits `PROPOSÉ` illustrent des
  signatures, aucun fichier de production n'est modifié.

---

## Annexe — table des citations (vérifiées par `grep`/`sed`, HEAD `afbb5cfc`)

| Affirmation | Fichier:ligne |
|---|---|
| `NodeRecord` 32 B, champs, asserts | `crates/obrain-substrate/src/record.rs:76-105` |
| `node_flags` bits 0-5 (6/16) | `crates/obrain-substrate/src/record.rs:66-72` |
| `EdgeRecord` bump 32→36 B (précédent) | `crates/obrain-substrate/src/record.rs` (doc `first_prop_off`, assert `:315`) |
| `noeud_visible_pour` (prédicat unique) | `crates/obrain-substrate/src/store.rs:3808` |
| `filtrer_par_epoque_et_tx` (lot) | `crates/obrain-substrate/src/store.rs:3779` |
| `visibilite_triviale` (chemin rapide) | `crates/obrain-substrate/src/store.rs:3822` |
| trait `GraphStore` (get_node/node_ids/find_nodes_by_property) | `crates/obrain-core/src/graph/traits.rs:91,258,265,385` |
| `AccessContext` `{reader, decide, audit}` | `crates/obrain-core/src/execution/base_registry.rs:246` |
| `store_for_reader` (garde par slug, inter-bases) | `crates/obrain-core/src/execution/base_registry.rs:192` |
| `Session.access_context` (V3, None=accès total) | `crates/obrain-engine/src/session/mod.rs:82` |
| `ObrainDB::open`→`open_substrate` | `crates/obrain-engine/src/database/mod.rs:387` |
| crate `obrain-iam` / ORN | `crates/obrain-iam/src/lib.rs` |
| `AuthenticatedSession` compagnon, pas wrapper | `crates/obrain-iam/src/session_ext.rs:1-38` |
| viz `:8432` `/api/query` sans principal | `crates/obrain-cli/src/viz_serve.rs:387,1037` |
| viz port 8432 | `crates/obrain-cli/src/commands/viz.rs:207` |
| `StsClaims {sub, tenant_id:Uuid, roles}` | `this-rs/src/core/auth/sts.rs:16` |
| `AuthContext` User/Service/Admin/Anonymous | `this-rs/src/core/auth/wami_provider.rs:211` |
| ACL matérialisées en propriétés indexées | `this-rs/src/storage/iam.rs:22-30,108` |
| tenancy `space: RwLock<Option<String>>` | `this-rs/src/storage/obrain.rs:87` |
| `TenantScopedLinkService` | `this-rs/src/storage/tenant_scoped.rs:54` |
| route 4 segments → `Abstain` | `this-rs/src/server/middleware/authorization.rs:543,557-580` |
| `Abstain` échoue ouvert | `this-rs/src/server/middleware/authorization.rs:724` |
