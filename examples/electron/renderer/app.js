'use strict'
// The page: sliders built from what the library says of each control, the photo
// on a canvas, and the tone curve. Everything it knows of a control's range,
// step and text comes from the editor; it only maps a slider's position to a
// value and sends the value.

const $ = (id) => document.getElementById(id)
const clock = () => performance.timeOrigin + performance.now()

const app = {
  // The number of the newest edit. Frames carry the number of the edit they show.
  edit: 0,
  // When each edit was made here, for the frame that shows it.
  stamps: new Map(),
  // Frames of edits before this one are another photo's.
  floor: 0,
  shown: { edit: -1, draft: false },
  photo: null,
  controls: new Map(),
  curve: null,
  viewEdge: null,
  // Called with every frame drawn; the probe listens here.
  drawn: [],
}
window.app = app

function nextEdit() {
  app.edit += 1
  app.stamps.set(app.edit, clock())
  return app.edit
}

function say(what) {
  const message = $('message')
  const failed = what instanceof Error
  // Electron prefixes an error from the main process with how it was called.
  message.textContent = failed
    ? what.message.replace(/^Error invoking remote method '[^']*': (Error: )?/, '')
    : what
  message.classList.toggle('error', failed)
}

// Sliders: 0 to STEPS positions, mapped to the control's range evenly in the
// value or, for a `reciprocal` control (Temp), evenly in its reciprocal.
const STEPS = 1000

function toPosition(control, value) {
  const { min, max } = control
  const along =
    control.scale === 'reciprocal'
      ? (1 / min - 1 / value) / (1 / min - 1 / max)
      : (value - min) / (max - min)
  return Math.round(Math.min(1, Math.max(0, along)) * STEPS)
}

function toValue(control, position) {
  const { min, max, step } = control
  const along = position / STEPS
  const value =
    control.scale === 'reciprocal'
      ? 1 / (1 / min - along * (1 / min - 1 / max))
      : min + along * (max - min)
  return Math.min(max, Math.max(min, Math.round(value / step) * step))
}

// Shows a control's state as the editor returned it. The thumb is left alone
// while it is being dragged.
async function changed(entry, pending, moveThumb) {
  try {
    const control = JSON.parse(await pending)
    entry.control = control
    entry.output.textContent = control.text
    if (moveThumb) entry.range.value = toPosition(control, control.value)
    say('')
  } catch (error) {
    say(error)
  }
}

function buildControls(controls) {
  const box = $('controls')
  box.replaceChildren()
  app.controls.clear()
  for (const control of controls) {
    const range = document.createElement('input')
    range.type = 'range'
    range.id = `control-${control.id}`
    range.min = 0
    range.max = STEPS
    range.step = 1
    range.value = toPosition(control, control.value)
    const label = document.createElement('label')
    label.htmlFor = range.id
    label.textContent = control.label
    label.title = 'Double-click to reset'
    const output = document.createElement('output')
    output.htmlFor = range.id
    output.textContent = control.text
    const row = document.createElement('div')
    row.className = 'control'
    row.append(label, range, output)
    box.append(row)

    const entry = { control, range, output }
    app.controls.set(control.id, entry)
    range.addEventListener('input', () => {
      const value = toValue(entry.control, Number(range.value))
      changed(entry, editor.set(control.id, value, nextEdit()), false)
    })
    label.addEventListener('dblclick', () => {
      changed(entry, editor.reset(control.id, nextEdit()), true)
    })
  }
}

// The photo's canvas is laid out at the size the photo fits the stage at, never
// above 100%, and the editor is told that size in device pixels. A draft's
// smaller frame is stretched over the same box.
let viewTimer = null

function layout() {
  if (!app.photo) return
  const viewport = $('viewport')
  const style = getComputedStyle(viewport)
  const width = viewport.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight)
  const height =
    viewport.clientHeight - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom)
  const scale = Math.min(
    width / app.photo.width,
    height / app.photo.height,
    1 / devicePixelRatio,
  )
  const shown = [app.photo.width * scale, app.photo.height * scale].map((v) =>
    Math.max(1, Math.round(v)),
  )
  const canvas = $('photo')
  canvas.style.width = `${shown[0]}px`
  canvas.style.height = `${shown[1]}px`
  const edge = Math.round(Math.max(...shown) * devicePixelRatio)
  if (edge === app.viewEdge) return
  // A window being resized settles before the editor renders for it.
  clearTimeout(viewTimer)
  viewTimer = setTimeout(
    () => {
      app.viewEdge = edge
      editor.setView(edge, app.edit)
    },
    app.viewEdge === null ? 0 : 80,
  )
}
new ResizeObserver(layout).observe($('viewport'))

editor.onRendered((frame, pixels) => {
  if (frame.error) {
    say(new Error(frame.error))
    return
  }
  const arrivedAt = clock()
  // The editor drops a frame older than the one it showed, and a draft after
  // its frame; what is left to drop here is a frame of the photo before.
  if (frame.edit < app.floor) return
  const canvas = $('photo')
  if (canvas.width !== frame.width || canvas.height !== frame.height) {
    canvas.width = frame.width
    canvas.height = frame.height
  }
  const data = new Uint8ClampedArray(pixels.buffer, pixels.byteOffset, pixels.byteLength)
  canvas.getContext('2d').putImageData(new ImageData(data, frame.width, frame.height), 0, 0)
  canvas.hidden = false
  app.shown = { edit: frame.edit, draft: frame.draft }

  const drawnAt = clock()
  const madeAt = app.stamps.get(frame.edit)
  for (const edit of app.stamps.keys()) if (edit < frame.edit) app.stamps.delete(edit)
  const parts = [`${frame.width} × ${frame.height}${frame.draft ? ' draft' : ''}`]
  parts.push(`rendered in ${frame.renderMs.toFixed(1)} ms`)
  if (madeAt !== undefined) parts.push(`${(drawnAt - madeAt).toFixed(0)} ms from the control`)
  $('frame-info').textContent = parts.join(' · ')
  for (const listener of app.drawn) listener({ frame, madeAt, arrivedAt, drawnAt })
})

// The tone curve. Points and the curve's shape come from the editor; this draws
// them and turns the pointer into points added, moved and removed.
const curveCanvas = $('curve')
const CURVE_PAD = 12
const POINT_REACH = 10
// The point being dragged: its index (unknown until the editor has added it),
// the newest position not sent yet, and whether a move is on its way.
let drag = null

function curveBox() {
  const rect = curveCanvas.getBoundingClientRect()
  return {
    rect,
    width: rect.width - 2 * CURVE_PAD,
    height: rect.height - 2 * CURVE_PAD,
  }
}

function curvePosition(event) {
  const { rect, width, height } = curveBox()
  const unit = (v) => Math.min(1, Math.max(0, v))
  return [
    unit((event.clientX - rect.left - CURVE_PAD) / width),
    unit(1 - (event.clientY - rect.top - CURVE_PAD) / height),
  ]
}

function pointAt(event) {
  const { rect, width, height } = curveBox()
  const index = app.curve.points.findIndex(([x, y]) => {
    const dx = rect.left + CURVE_PAD + x * width - event.clientX
    const dy = rect.top + CURVE_PAD + (1 - y) * height - event.clientY
    return Math.hypot(dx, dy) <= POINT_REACH
  })
  return index < 0 ? null : index
}

function setCurve(curve) {
  app.curve = curve
  drawCurve()
}

function drawCurve() {
  const { rect, width, height } = curveBox()
  if (!app.curve || rect.width === 0) return
  const ratio = devicePixelRatio
  curveCanvas.width = Math.round(rect.width * ratio)
  curveCanvas.height = Math.round(rect.height * ratio)
  const g = curveCanvas.getContext('2d')
  g.scale(ratio, ratio)
  const at = (x, y) => [CURVE_PAD + x * width, CURVE_PAD + (1 - y) * height]
  const line = (from, to, color) => {
    g.strokeStyle = color
    g.beginPath()
    g.moveTo(...from)
    g.lineTo(...to)
    g.stroke()
  }
  g.lineWidth = 1
  for (let i = 0; i <= 4; i += 1) {
    line(at(i / 4, 0), at(i / 4, 1), '#34343a')
    line(at(0, i / 4), at(1, i / 4), '#34343a')
  }
  line(at(0, 0), at(1, 1), '#44444a')
  const { samples, points } = app.curve
  g.strokeStyle = '#dcdcde'
  g.lineWidth = 1.5
  g.beginPath()
  samples.forEach(([x, y], i) => {
    const [px, py] = at(x, Math.min(1, Math.max(0, y)))
    if (i === 0) g.moveTo(px, py)
    else g.lineTo(px, py)
  })
  g.stroke()
  points.forEach(([x, y], index) => {
    g.beginPath()
    g.arc(...at(x, y), 4.5, 0, 2 * Math.PI)
    g.fillStyle = drag && drag.index === index ? '#d9a441' : '#dcdcde'
    g.fill()
    g.strokeStyle = '#1b1b1d'
    g.stroke()
  })
}

// One move is on its way at a time, and the newest position replaces the one
// waiting, so the curve drawn is never many moves behind the pointer.
function sendMove() {
  if (!drag || drag.index === null || drag.busy || !drag.next) return
  const mine = drag
  const [x, y] = mine.next
  mine.next = null
  mine.busy = true
  editor
    .curveMove(mine.index, x, y, nextEdit())
    .then((json) => setCurve(JSON.parse(json)), say)
    .finally(() => {
      mine.busy = false
      if (drag === mine) sendMove()
    })
}

curveCanvas.addEventListener('pointerdown', async (event) => {
  if (!app.curve || event.button !== 0) return
  try {
    curveCanvas.setPointerCapture(event.pointerId)
  } catch {
    // A pointer the browser does not know (a scripted one) cannot be captured.
  }
  const mine = { index: pointAt(event), next: null, busy: false }
  drag = mine
  if (mine.index !== null) {
    drawCurve()
    return
  }
  try {
    const added = JSON.parse(await editor.curveInsert(...curvePosition(event), nextEdit()))
    mine.index = added.index
    setCurve(added.curve)
    sendMove()
  } catch (error) {
    say(error)
  }
})

curveCanvas.addEventListener('pointermove', (event) => {
  if (!drag) return
  drag.next = curvePosition(event)
  sendMove()
})

function release() {
  if (!drag) return
  const { index, next } = drag
  drag = null
  // The last position, if it was still waiting for the move before it.
  if (index !== null && next) {
    editor.curveMove(index, ...next, nextEdit()).then((json) => setCurve(JSON.parse(json)), say)
  }
  drawCurve()
}
curveCanvas.addEventListener('pointerup', release)
curveCanvas.addEventListener('pointercancel', release)

async function removePoint(index) {
  try {
    setCurve(JSON.parse(await editor.curveRemove(index, nextEdit())))
  } catch (error) {
    say(error)
  }
}

curveCanvas.addEventListener('dblclick', (event) => {
  if (!app.curve) return
  const index = pointAt(event)
  if (index !== null) removePoint(index)
})

$('curve-reset').addEventListener('click', async () => {
  // The ends stay; every point between them goes, last first.
  for (let index = app.curve.points.length - 2; index >= 1; index -= 1) {
    await removePoint(index)
  }
})

function opened(state) {
  app.photo = state.photo
  // The editor numbers this photo's first frame after every edit of the last.
  app.floor = state.edit
  app.edit = Math.max(app.edit, state.edit)
  app.shown = { edit: -1, draft: false }
  // Its first frame took a decode, not the time from a control.
  app.stamps.clear()
  buildControls(state.controls)
  for (const id of ['basic', 'tone-curve']) $(id).hidden = false
  for (const id of ['empty', 'closed']) $(id).hidden = true
  $('export').disabled = false
  setCurve(state.curve)
  const { name, camera, width, height } = state.photo
  $('photo-info').textContent = [name, camera, `${width} × ${height}`].filter(Boolean).join(' · ')
  say('')
  layout()
}
app.opened = opened
editor.onOpened((json) => opened(JSON.parse(json)))

// Open and Export take a while and are off while either runs.
async function busy(work) {
  const buttons = ['open', 'open-first', 'export'].map($)
  const off = buttons.map((button) => button.disabled)
  for (const button of buttons) button.disabled = true
  try {
    await work()
  } catch (error) {
    say(error)
  } finally {
    buttons.forEach((button, i) => {
      button.disabled = off[i]
    })
    if (app.photo) $('export').disabled = false
  }
}

function open() {
  return busy(async () => {
    const json = await editor.open(nextEdit())
    if (json) opened(JSON.parse(json))
  })
}
$('open').addEventListener('click', open)
$('open-first').addEventListener('click', open)

$('export').addEventListener('click', () =>
  busy(async () => {
    say('Exporting…')
    const done = await editor.export()
    say(done ? [`Exported to ${done.file}`, done.note].filter(Boolean).join(' — ') : '')
  }),
)
