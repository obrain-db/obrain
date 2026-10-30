#!/usr/bin/env node
/**
 * Essai de transport — un nœud distant piloté en JSON-RPC sur stdio.
 *
 * CE QU'ON TESTE, ET RIEN D'AUTRE : le harness sait lancer un sous-processus et
 * lui parler en JSON-RPC délimité par des retours à la ligne. Si `command` est
 * un exécutable arbitraire, alors `ssh` doit suffire à porter ce même protocole
 * vers une AUTRE machine — sans écrire un octet de protocole nouveau.
 *
 * Le pari est structurel, pas magique : `ssh hote commande` relaie stdin/stdout
 * du processus local vers le processus distant. Le JSON-RPC ne voit pas la
 * différence.
 *
 * Usage : node drive.mjs <chemin/cordis.yml> [hote]
 */

import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'

const cfg = process.argv[2]
const hote = process.argv[3] ?? 'localhost'
if (!cfg) {
  console.error('usage: node drive.mjs <cordis.yml> [hote]')
  process.exit(2)
}

// C'est TOUT le « transport » : une commande ssh ordinaire.
// Le cwd distant est explicite — les plugins du harness se résolvent depuis le
// dépôt, pas depuis le répertoire par défaut de sshd.
const DEPOT = process.env.DSH_DEPOT ?? '/home/triviere/projects/harness-worktrees/graph-writer'
const AGENT = `${DEPOT}/packages/examples/jsonrpc-demo/lib/bin.js`

// ── L'ENVIRONNEMENT N'EST PAS LE MÊME SOUS SSH, et c'est un fait mesuré ──────
// Premier essai : le harness n'a même pas démarré. `ssh hote 'node …'` résout
// `/usr/bin/node` → v18.19.1, alors que le harness importe `node:util` /
// `parseEnv` (Node ≥ 20.12). Dans un shell local, nvm fournit v22.19.0.
//
// Ce n'est pas un détail de confort : un harness déployé sur N machines doit
// ÉPINGLER son interpréteur, pas hériter du PATH d'un shell non interactif.
// Le chemin est donc explicite ici — comme il devrait l'être dans un déploiement.
const NODE = process.env.DSH_NODE ?? '/home/triviere/.nvm/versions/node/v22.19.0/bin/node'

const enfant = spawn(
  'ssh',
  ['-o', 'BatchMode=yes', '-o', 'StrictHostKeyChecking=no', hote,
   `cd ${DEPOT} && exec ${NODE} ${AGENT} ${cfg}`],
  { stdio: ['pipe', 'pipe', 'pipe'] },
)

enfant.stderr.on('data', (d) => process.stderr.write(`[ssh stderr] ${d}`))

let compteur = 0
const enAttente = new Map()
const trames = []

function envoyer(methode, params) {
  const id = ++compteur
  const req = { jsonrpc: '2.0', id, method: methode }
  if (params !== undefined) req.params = params
  enfant.stdin.write(JSON.stringify(req) + '\n')
  return new Promise((res, rej) => {
    enAttente.set(id, { res, rej })
    setTimeout(() => {
      if (enAttente.has(id)) {
        enAttente.delete(id)
        rej(new Error(`délai dépassé sur ${methode}`))
      }
    }, 300_000)
  })
}

const rl = createInterface({ input: enfant.stdout })
rl.on('line', (ligne) => {
  if (!ligne.trim()) return
  let m
  try {
    m = JSON.parse(ligne)
  } catch {
    console.error(`[sortie non-JSON] ${ligne.slice(0, 200)}`)
    return
  }
  if (m.id !== undefined && enAttente.has(m.id)) {
    const { res, rej } = enAttente.get(m.id)
    enAttente.delete(m.id)
    m.error ? rej(new Error(JSON.stringify(m.error))) : res(m.result)
    return
  }
  // Notifications : c'est le flux d'événements du nœud distant.
  if (m.method) {
    trames.push(m)
    if (m.method === 'session.status') {
      console.log(`  · statut : ${m.params?.status}`)
    } else if (m.method === 'session.event') {
      const e = m.params?.event
      if (e?.type) console.log(`  · event : ${e.type}`)
    } else {
      console.log(`  · ${m.method}`)
    }
  }
})

/** Extrait le dernier texte assistant complet du flux d'événements. */
function dernierTexte() {
  let texte = null
  for (const t of trames) {
    const e = t.params?.event
    if (e?.type !== 'assistant/message') continue
    const blocs = e.message?.content ?? e.content ?? []
    const t2 = blocs
      .filter((b) => b?.type === 'text')
      .map((b) => b.text)
      .join('')
      .trim()
    if (t2) texte = t2
  }
  return texte
}

/** Attend que le nœud repasse `idle` après avoir été `running`. */
function attendreFin(delaiMs = 240_000) {
  return new Promise((res, rej) => {
    const debut = Date.now()
    let vuRunning = false
    const iv = setInterval(() => {
      const statuts = trames
        .filter((t) => t.method === 'session.status')
        .map((t) => t.params?.status)
      if (statuts.includes('running')) vuRunning = true
      if (vuRunning && statuts.at(-1) === 'idle') {
        clearInterval(iv)
        res()
      }
      if (Date.now() - debut > delaiMs) {
        clearInterval(iv)
        rej(new Error('le nœud distant ne s\'est jamais remis au repos'))
      }
    }, 500)
  })
}

const t0 = Date.now()
try {
  console.log(`1) poignée de main par ssh vers « ${hote} »`)
  const init = await envoyer('initialize', {
    cwd: DEPOT,
    provider: 'local',
    model: 'qwen-local',
    maxTokens: 2048,
  })
  console.log(`   serverInfo : ${JSON.stringify(init.serverInfo)}`)
  console.log(`   (${Date.now() - t0} ms)`)

  console.log('2) envoi d\'un tour qui exige une ACTION sur la machine distante')
  const tache =
    'Exécute la commande shell `uname -n` et `pwd`. Réponds ensuite en une seule ' +
    'phrase donnant le nom d\'hôte EXACT renvoyé par la commande, sans commentaire.'
  const { messageId } = await envoyer('session/prompt', {
    sessionId: 'essai-transport-1',
    contentBlocks: [{ type: 'text', text: tache }],
  })
  console.log(`   reçu (messageId=${messageId})`)

  console.log('3) attente de la fin du tour…')
  await attendreFin()

  const texte = dernierTexte()
  console.log('\n─── RÉSULTAT ───')
  console.log(texte ? texte : '(aucun texte assistant)')

  const appelBash = trames.some((t) => {
    const e = t.params?.event
    const blocs = e?.message?.content ?? []
    return e?.type === 'assistant/message' && blocs.some((b) => b?.type === 'tool-call')
  })
  console.log(`\naction shell déclenchée sur le nœud : ${appelBash ? 'OUI' : 'NON (à vérifier)'}`)
  console.log(`durée totale : ${Date.now() - t0} ms`)

  await envoyer('shutdown')
  enfant.kill('SIGTERM')
  process.exit(0)
} catch (e) {
  console.error(`\nÉCHEC : ${e.message}`)
  enfant.kill('SIGKILL')
  process.exit(1)
}
