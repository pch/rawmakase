// Builds the Node addon (the crate here with its `addon` feature) and puts it
// where main.js loads it from: native/rawmakase.node.
import { execFileSync } from 'node:child_process'
import { copyFileSync, mkdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
execFileSync('cargo', ['build', '--release', '--locked', '--lib', '--features', 'addon'], {
  cwd: root,
  stdio: 'inherit',
})
const library = {
  linux: 'librawmakase_electron.so',
  darwin: 'librawmakase_electron.dylib',
  win32: 'rawmakase_electron.dll',
}[process.platform]
if (!library) throw new Error(`No addon file name is known for ${process.platform}`)
mkdirSync(join(root, 'native'), { recursive: true })
copyFileSync(join(root, 'target', 'release', library), join(root, 'native', 'rawmakase.node'))
