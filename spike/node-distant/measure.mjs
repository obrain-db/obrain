#!/usr/bin/env node
/**
 * Mesure du DÉMARRAGE : temps entre le lancement du processus et la réponse à
 * `initialize`. C'est la seule mesure qui compte pour un nœud qu'on va lancer
 * des milliers de fois — un coût par appel, pas une impression.
 *
 * Usage : node measure.mjs <étiquette> <commande…>
 */

import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'

const etiquette = process.argv[2]
const cmd = process.argv[3]
const args = process.argv.slice(4)

const t0 = process.hrtime.bigint()
const enfant = spawn(cmd, args, { stdio: ['pipe', 'pipe', 'pipe'] })

let stderr = ''
enfant.stderr.on('data', (d) => (stderr += d))

const rl = createInterface({ input: enfant.stdout })
let fini = false

const minuteur = setTimeout(() => {
  if (!fini) {
    console.log(`${etiquette} : ÉCHEC — aucune réponse à initialize en 60 s`)
    if (stderr.trim()) {
      const l = stderr.trim().split('\n')
      console.log(`   dernière erreur : ${l[l.length - 1].slice(0, 160)}`)
    }
    enfant.kill('SIGKILL')
    process.exit(0)
  }
}, 60_000)

enfant.on('error', (e) => {
  console.log(`${etiquette} : ÉCHEC au lancement — ${e.message}`)
  process.exit(0)
})

enfant.on('exit', (code) => {
  if (!fini) {
    console.log(`${etiquette} : ÉCHEC — le processus est sorti (code ${code})`)
    if (stderr.trim()) {
      const l = stderr.trim().split('\n')
      console.log(`   dernière erreur : ${l[l.length - 1].slice(0, 160)}`)
    }
    process.exit(0)
  }
})

rl.on('line', (ligne) => {
  let m
  try {
    m = JSON.parse(ligne)
  } catch {
    return
  }
  if (m.id === 1) {
    fini = true
    clearTimeout(minuteur)
    const ms = Number(process.hrtime.bigint() - t0) / 1e6
    const nom = m.result?.serverInfo?.name ?? '?'
    console.log(`${etiquette} : ${ms.toFixed(0)} ms  (serverInfo.name=${nom})`)
    enfant.kill('SIGKILL')
    process.exit(0)
  }
})

// Ligne unique : on RELANCE le processus pour chaque mesure, jamais on ne
// réutilise un processus chaud — ce serait mesurer autre chose.
//
// Les paramètres sont FOURNIS : un `initialize` sans paramètres se fait refuser
// par le harness (il lui faut cwd/provider/model), et l'on chronométrerait
// alors une ERREUR — un chiffre flatteur et faux.
const params = process.env.INIT_PARAMS ? JSON.parse(process.env.INIT_PARAMS) : {}
const envoi = { jsonrpc: '2.0', id: 1, method: 'initialize' }
if (Object.keys(params).length) envoi.params = params
enfant.stdin.write(JSON.stringify(envoi) + '\n')
