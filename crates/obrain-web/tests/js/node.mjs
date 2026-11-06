// Exécute la batterie sur le paquet node : `node tests/js/node.mjs <dossier pkg>`
import { createRequire } from "node:module";
import { resolve } from "node:path";
import { executer } from "./suite.mjs";

const pkg = resolve(process.argv[2] ?? "pkg", "obrain_web.js");
const { Base } = createRequire(import.meta.url)(pkg);
const rapport = await executer(Base, (l) => console.log(l));
console.log(`\n${rapport.ok}/${rapport.total} tests passés, ${rapport.ignores.length} ignorés, ${rapport.echecs.length} échecs, ${rapport.duree_ms} ms`);
process.exit(rapport.echecs.length === 0 ? 0 : 1);
