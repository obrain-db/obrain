#!/usr/bin/env node
/**
 * Montage d'un nœud `selfd` avec LA bibliothèque MCP DU HARNESS.
 *
 * Ce n'est pas un test de conformité écrit à la main : on emploie le SDK MCP
 * dans la version exacte que `packages/mcp/mcp-client` utilise (v1.29.0 liée
 * dans son `node_modules/.pnpm`), avec le même transport
 * (`StdioClientTransport`). Si ce client-là monte le nœud, le harness le montera.
 *
 * Le seul écart avec la configuration réelle du harness, c'est `command: 'ssh'`
 * là où un serveur MCP local serait `command: 'node'`. C'est précisément la
 * démonstration : **le transport est un exécutable**, et `ssh` en est un.
 *
 * Usage : node mount-harness-client.mjs [hote] [chemin-du-binaire-distant]
 */

const SDK =
  process.env.MCP_SDK ??
  '/home/triviere/projects/harness-worktrees/graph-writer/node_modules/.pnpm/@modelcontextprotocol+sdk@1.29.0_zod@4.4.3/node_modules/@modelcontextprotocol/sdk/dist/esm'

const { Client } = await import(`${SDK}/client/index.js`)
const { StdioClientTransport } = await import(`${SDK}/client/stdio.js`)

const hote = process.argv[2] ?? 'localhost'
const binaire = process.argv[3] ?? '/tmp/selfd-node'

// Exactement ce qu'une ligne de `cordis.patch.yml` produirait.
const transport = new StdioClientTransport({
  command: 'ssh',
  args: [
    '-o', 'BatchMode=yes',
    hote,
    binaire, 'mcp',
    '--root', '/tmp',
    '--allow-exec',
    '--state-dir', '/tmp/selfd-node-state',
  ],
})

const client = new Client({ name: 'essai-montage-noeud', version: '1.0.0' })

try {
  await client.connect(transport)
  const v = client.getServerVersion()
  console.log(`monté : ${v?.name} ${v?.version} · protocole ${client.getServerCapabilities() ? 'oui' : 'non'}`)

  const { tools } = await client.listTools()
  console.log(`outils : ${tools.map((t) => t.name).join(', ')}`)

  const statut = await client.callTool({ name: 'node_status', arguments: {} })
  const s = JSON.parse(statut.content[0].text)
  console.log(`nœud   : ${s.node.nodeId} sur ${s.node.hostnameCourant} (${s.os}/${s.arch}) · exec=${s.capabilities.exec}`)

  const exec = await client.callTool({ name: 'exec', arguments: { cmd: 'hostname' } })
  const e = JSON.parse(exec.content[0].text)
  console.log(`exec   : ${JSON.stringify(e.stdout)} code=${e.exitCode} en ${e.durationMs} ms`)

  // Un refus doit être un RÉSULTAT (isError), pas une erreur de protocole : le
  // modèle doit pouvoir le lire et changer d'approche.
  const refus = await client.callTool({ name: 'read', arguments: { path: '/etc/passwd' } })
  const r = JSON.parse(refus.content[0].text)
  console.log(`refus  : isError=${refus.isError} → ${String(r.error).slice(0, 70)}…`)

  await client.close()
  console.log('\nVERDICT : le nœud Rust est monté par le client MCP du harness.')
} catch (e) {
  console.error(`ÉCHEC : ${e?.message ?? e}`)
  process.exit(1)
}
