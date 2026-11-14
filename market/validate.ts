import fs from 'node:fs'
import { pathToFileURL } from 'node:url'
import path from 'node:path'

const [dsh, file] = process.argv.slice(2)
const { parseMarketIndex, marketInstallTarget } = await import(pathToFileURL(path.join(dsh, 'packages/util/plugin-market-index/src/index.ts')).href)
const index = parseMarketIndex(JSON.parse(fs.readFileSync(file, 'utf8')), 'https://raw.githubusercontent.com/emmettlu/tracefs/main/market/index.json')
for (const p of index.plugins) for (const v of p.versions) console.log(`${p.name}@${v.version}: ${marketInstallTarget(p, v).spec}`)
