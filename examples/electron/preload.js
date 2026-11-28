// The page's whole view of the editor: these calls and nothing of Node.
const { contextBridge, ipcRenderer } = require('electron')

const call = (channel) => (...args) => ipcRenderer.invoke(channel, ...args)
const listen = (channel) => (listener) =>
  ipcRenderer.on(channel, (_event, ...args) => listener(...args))

contextBridge.exposeInMainWorld('editor', {
  // Each resolves with JSON from the editor; `edit` is the page's number for
  // the change, which the frame showing it carries back.
  open: call('open'),
  set: call('set'),
  reset: call('reset'),
  curveInsert: call('curve-insert'),
  curveMove: call('curve-move'),
  curveRemove: call('curve-remove'),
  export: call('export'),
  setView: (maxEdge, edit) => ipcRenderer.send('view', maxEdge, edit),
  onRendered: listen('rendered'),
  onOpened: listen('opened'),
  // The scripted run that takes the example's measures and screenshots.
  onProbe: listen('probe'),
  probe: call('probe'),
})
