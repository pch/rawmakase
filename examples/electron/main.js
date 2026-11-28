// The Electron main process: the window, and the editor's Node addon, which
// lives here. The page asks for edits over IPC and gets frames back the same
// way; rendering itself runs on the addon's own thread.
const { app, BrowserWindow, dialog, ipcMain } = require('electron')
const fs = require('node:fs')
const path = require('node:path')

const addonPath = path.join(__dirname, 'native', 'rawmakase.node')
const { Addon } = require(addonPath)

const flag = (name) =>
  process.argv.find((arg) => arg.startsWith(`--${name}=`))?.slice(name.length + 3)
// `--photo=<file>` opens a photo at launch; `--probe=<folder>` runs the scripted
// measures and screenshots into it and quits.
const photoAtLaunch = flag('photo')
const probeFolder = flag('probe')

const now = () => performance.timeOrigin + performance.now()

let win
let editor

// Where this app keeps RAWmakase's data: values it chooses and hands to the
// library, not variables in the environment. A cache entry belongs to the build
// of the addon that made it.
function folders() {
  const home = app.getPath('userData')
  const built = fs.statSync(addonPath)
  return {
    dataDir: path.join(home, 'data'),
    cacheDir: path.join(home, 'cache'),
    buildId: `${built.size}-${Math.round(built.mtimeMs)}`,
  }
}

// When the newest edit reached this process, for the measures.
const received = new Map()

function onRendered({ pixels, ...frame }) {
  if (!win || win.isDestroyed()) return
  frame.receivedAt = received.get(frame.edit)
  frame.renderedAt = now()
  for (const edit of received.keys()) if (edit < frame.edit) received.delete(edit)
  win.webContents.send('rendered', frame, pixels)
}

async function open(file, edit) {
  const state = await editor.open(file, edit)
  win.setTitle(`${path.basename(file)} — RAWmakase Electron example`)
  return state
}

function handle(channel, handler) {
  ipcMain.handle(channel, (_event, ...args) => handler(...args))
}

function wire() {
  const edited = (edit) => received.set(edit, now())
  handle('open', async (edit) => {
    const { canceled, filePaths } = await dialog.showOpenDialog(win, {
      title: 'Open a photo',
      properties: ['openFile'],
    })
    return canceled ? null : open(filePaths[0], edit)
  })
  handle('set', (name, value, edit) => {
    edited(edit)
    return editor.setParameter(name, value, edit)
  })
  handle('reset', (name, edit) => {
    edited(edit)
    return editor.resetParameter(name, edit)
  })
  handle('curve-insert', (x, y, edit) => {
    edited(edit)
    return editor.curveInsert(x, y, edit)
  })
  handle('curve-move', (index, x, y, edit) => {
    edited(edit)
    return editor.curveMove(index, x, y, edit)
  })
  handle('curve-remove', (index, edit) => {
    edited(edit)
    return editor.curveRemove(index, edit)
  })
  handle('export', async () => {
    let file = probeFolder && path.resolve(probeFolder, 'export.jpg')
    if (!file) {
      const chosen = await dialog.showSaveDialog(win, {
        title: 'Export',
        defaultPath: 'export.jpg',
        filters: [
          { name: 'JPEG', extensions: ['jpg'] },
          { name: '16-bit TIFF', extensions: ['tif'] },
          { name: 'PNG', extensions: ['png'] },
        ],
      })
      if (chosen.canceled) return null
      file = chosen.filePath
    }
    const note = await editor.export(file)
    return { file, note }
  })
  ipcMain.on('view', (_event, maxEdge, edit) => editor.setView(maxEdge, edit))
  // The probe's steps that need this process: the photo named at launch, a
  // screenshot, its report.
  handle('probe', async (step, payload) => {
    if (!probeFolder) return null
    fs.mkdirSync(probeFolder, { recursive: true })
    if (step === 'open') {
      return open(path.resolve(photoAtLaunch), payload)
    } else if (step === 'shot') {
      const image = await win.webContents.capturePage()
      fs.writeFileSync(path.join(probeFolder, `${payload}.png`), image.toPNG())
    } else if (step === 'report') {
      fs.writeFileSync(path.join(probeFolder, 'measures.json'), JSON.stringify(payload, null, 2))
      app.quit()
    }
  })
}

app.whenReady().then(async () => {
  editor = new Addon(folders(), onRendered)
  wire()
  win = new BrowserWindow({
    width: 1280,
    height: 800,
    minWidth: 900,
    minHeight: 600,
    backgroundColor: '#1b1b1d',
    title: 'RAWmakase Electron example',
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  })
  win.removeMenu()
  // The page is this app's own files and stays that.
  win.webContents.setWindowOpenHandler(() => ({ action: 'deny' }))
  win.webContents.on('will-navigate', (event) => event.preventDefault())
  await win.loadFile(path.join(__dirname, 'renderer', 'index.html'))
  if (probeFolder) {
    win.webContents.send('probe')
  } else if (photoAtLaunch) {
    try {
      win.webContents.send('opened', await open(path.resolve(photoAtLaunch), 0))
    } catch (error) {
      dialog.showErrorBox('Could not open the photo', error.message)
    }
  }
})

app.on('window-all-closed', () => app.quit())
