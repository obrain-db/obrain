# Audit des endpoints — 2026-09-15

Perimetre : viz :8432 (live), daemon harness :8090, chats :3082/:3083,
gateway :8144, DGX :9910. Inventaire par le code du tronc
`consolidation/tronc-release-2026-09-15`, sonde live sur le deploye.

## Verdict global

- **Zero route morte** : 27 endpoints sondes, tous repondent (200, ou
  400/401/503 attendus — parametre requis, auth armee, IAM non arme).
- **4 endpoints au-dessus de 100 ms** (voir ci-dessous).
- **2 constats d infrastructure** a traiter.

## Inventaire et mesures (sonde du 2026-09-15, store runtime-dev)

| Endpoint | Code | ms | Octets | Verdict |
|---|---|---|---|---|
| /api/viz/status | 200 | 15,6 | 221 | OK |
| /api/stores | 200 | 0,5 | 712 | OK |
| /api/info | 200 | 0,3 | 350 | OK |
| /api/query (count) | 200 | 0,6 | 77 | OK |
| /api/functions (q=session) | 200 | **758** | 5 336 | LENT — construction trigrammes a la demande dans l appel |
| /api/viz/subgraph (pire cas 584 aretes) | 200 | 70,8 | 181 221 | OK (payload) |
| /api/viz/overview | 200 | 0,5 | 587 | OK |
| /api/viz/modules | 200 | 0,3 | 14 192 | OK |
| /api/viz/signatures | 200 | 0,5 | 2 228 | OK |
| /api/viz/arcs | 200 | 0,5 | 3 413 | OK |
| /api/viz/space | 200 | **1 513** | 1 087 | LENT — projection calculee par appel |
| /api/viz/weave | 200 | **144** | 4 870 | LENT (limite) |
| /api/viz/activation | 200 | 4,7 | 6 782 | OK |
| /api/worlds, /status | 200 | 1,2 / 0,3 | — | OK |
| /api/engine/sante | 200 | 5,2 | 134 | OK |
| /api/engine/memoire | 200 | 0,6 | 154 | OK |
| /api/engine/ask, /chat | 400 sans params | 0,4 | — | vivants (LLM non declenche) |
| /api/entretien | 200 | 0,3 | 131 | OK (binaire consolide-pensee) |
| /api/auth/whoami | 200 | 0,2 | 285 | OK |
| /api/iam/moi | 503 | 0,3 | 163 | attendu : IAM non arme (:8432 sans auth, exige) |
| /app | 200 | 0,4 | 702 | OK |
| harness /api/harness/health | 200 | 7,6 | 122 | OK |
| harness /api/harness/sessions | 200 | 62,3 | 1 457 | OK |
| harness /api/harness/context | 200 | **110** | 482 | au-dessus du contrat < 50 ms (chantier T2/NS) |
| chats :3083 / :3082 | 200 | 3,5 / 4,4 | — | OK |
| gateway :8144 | 401 | 9,8 | — | attendu (IAM arme) |
| DGX :9910 /health | 200 | 4,7 | — | OK |

Routes du tronc non couvertes par la sonde parametree : /api/viz/subgraph/stream
(WS — contrat teste par ailleurs, handshake 0,9 ms), et les 16 autres routes
/api/iam/* (meme garde 503 tant que l IAM du viz est desarme).

## Constats d infrastructure

1. **Le viz :8432 tourne sur `sauvegardes-bascule-v3/obrain.consolide-pensee-20260915`**
   (deploye ce jour par la lignee selflink/consolidation), pas sur le binaire
   partage du harness. Le tronc `consolidation/tronc-release-2026-09-15`
   CONTIENT ce travail (0 commit manquant) et le supplante : la bascule du
   binaire doit pointer les unites vers le tronc.
2. **Processus errant** : un second `viz serve` sur 127.0.0.1:8439 sert le
   store PROD depuis un build /tmp/tpc-j6 (reste de banc). A eteindre ou a
   adopter dans une unite.

## Points chauds a traiter (par priorite)

1. `/api/viz/space` 1,5 s : projection par appel — memoire/cache a poser.
2. `/api/functions` 758 ms a froid : la construction de l index de
   trigrammes tombe dans l appel — le chantier NS (index persistants
   charges a l ouverture) la supprime ; en attendant, warm au demarrage.
3. `/api/harness/context` 110 ms vs contrat 50 ms : scan tc.session —
   correctif = index d egalite declare (`index create`) une fois le tronc
   deploye (le verbe existe dans le tronc).
4. `/api/viz/weave` 144 ms : rappel BM25 — meme famille que 2.
