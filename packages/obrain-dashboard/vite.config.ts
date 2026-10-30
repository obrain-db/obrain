import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";

/**
 * POLICES KaTeX EN LIGNE (data:), ET POURQUOI C'EST NÉCESSAIRE ICI.
 *
 * `obrain viz serve` sert `/app/*` en lisant le fichier puis en le convertissant
 * en chaîne UTF-8 (`static_file` → `String::from_utf8_lossy`, viz_serve.rs).
 * Pour un `.woff2` — un binaire — cette conversion DÉTRUIT le fichier : la
 * police arriverait corrompue et KaTeX retomberait sur une police système
 * (symboles manquants, espacements cassés : une formule fausse se lit comme une
 * formule juste).
 *
 * On inline donc les polices en base64 DANS la feuille de style : plus aucune
 * requête binaire. On ne garde que le `woff2` — tous les navigateurs visés le
 * lisent ; les variantes `woff`/`ttf` pèsent 1,5 Mo pour rien.
 *
 * Le correctif de fond est côté serveur (servir des OCTETS) ; il demande un
 * redémarrage du serveur, celui-ci n'en demande pas.
 */
function policesEnLigne(): Plugin {
  return {
    name: "obrain-polices-en-ligne",
    enforce: "pre",
    transform(code, id) {
      if (!id.includes("katex") || !id.includes(".css")) return null;
      // `bloc` est la correspondance ENTIÈRE (« src:url(…),url(…),url(…) ») :
      // on garde le préfixe tel quel, on ne refiltre que les sources.
      const nettoye = code.replace(/src:([^;}]+)/g, (bloc) => {
        const gardees = bloc.split(",").filter((p) => p.includes("woff2"));
        return gardees.length ? gardees.join(",") : bloc;
      });
      return { code: nettoye, map: null };
    },
  };
}

// Base '/app/' : le dashboard est servi par `obrain viz serve` sous /app
// (dev : `pnpm dev` ouvre /app avec proxy /api -> obrain viz serve).
export default defineConfig({
  base: "/app/",
  plugins: [policesEnLigne(), react()],
  server: {
    port: 5173,
    proxy: { "/api": "http://127.0.0.1:8432" },
  },
  build: {
    outDir: "dist",
    // ── ON NE VIDE PLUS `dist/` À CHAQUE RECONSTRUCTION ────────────────────
    // Défaut mesuré le 2026-09-13, signalé par l'utilisateur : « index absent,
    // ma page recharge et je ne peux pas avoir de réponse ». Vite vide `outDir`
    // par défaut, et le veilleur de dev reconstruit à chaque changement de
    // `src/` : pendant les ~25 s du build, `dist/index.html` n'existe plus, donc
    // le serveur répond « index absent » — et le rechargement du veilleur tombe
    // pile dans cette fenêtre. En gardant les fichiers précédents, la page
    // ancienne reste servie jusqu'à ce que la nouvelle soit écrite. Les bundles
    // hachés périmés s'accumulent (quelques mégaoctets en développement, sans
    // conséquence : leurs noms changent à chaque build, aucune confusion
    // possible) — c'est un prix dérisoire pour ne plus casser la page.
    emptyOutDir: false,
    sourcemap: true,
    // Les polices KaTeX doivent tenir en `data:` (voir `policesEnLigne`) : le
    // seuil par défaut (4 ko) en laisserait passer la plupart en fichiers, donc
    // corrompues à l'arrivée.
    assetsInlineLimit: (fichier: string) =>
      fichier.includes("katex") && fichier.endsWith(".woff2") ? true : undefined,
  },
});
