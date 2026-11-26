'use strict'
// The scripted run (`electron . --probe=<folder> --photo=<file>`): it drives the
// page through its own controls, as a hand would, to take the example's measures
// and screenshots, then has the main process write them and quit. Idle in a
// normal run.

editor.onProbe(async () => {
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))
  const nextPaint = () => new Promise((resolve) => requestAnimationFrame(() => resolve(clock())))
  const shot = async (name) => {
    await nextPaint()
    await sleep(150)
    await editor.probe('shot', name)
  }

  // Every frame drawn from here on, with the times of its journey.
  const frames = []
  let waiting = []
  app.drawn.push((drawn) => {
    const entry = { ...drawn, presentedAt: null }
    frames.push(entry)
    requestAnimationFrame(() => {
      entry.presentedAt = clock()
      waiting = waiting.filter((wait) => !wait(entry))
    })
  })
  // The first frame presented that `wanted` accepts.
  const frameWhere = (wanted) =>
    new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('No frame came within 20 s')), 20000)
      waiting.push((entry) => {
        if (!wanted(entry.frame)) return false
        clearTimeout(timer)
        resolve(entry)
        return true
      })
    })
  const frameOf = (edit, draft) =>
    frameWhere((frame) => frame.edit === edit && frame.draft === draft)
  // The newest edit's frame, which may be on screen already.
  const latestFrame = async () => {
    while (app.shown.edit !== app.edit || app.shown.draft) await sleep(20)
    await nextPaint()
  }

  // Moves a slider as dragging its thumb does.
  const slide = (id, value) => {
    const { control, range } = app.controls.get(id)
    range.value = toPosition(control, value)
    range.dispatchEvent(new Event('input', { bubbles: true }))
    return app.edit
  }
  const pointer = (target, type, x = 0, y = 0) =>
    target.dispatchEvent(
      new PointerEvent(type, { clientX: x, clientY: y, button: 0, pointerId: 7, bubbles: true }),
    )

  // One edit's times, in milliseconds from the slider's change.
  const journey = ({ frame, madeAt, arrivedAt, drawnAt, presentedAt }) => ({
    draft: frame.draft,
    width: frame.width,
    height: frame.height,
    // The render on the Rust side, recipe to RGBA.
    render: frame.renderMs,
    // Page to main process, to the edit's arrival there.
    toMain: frame.receivedAt - madeAt,
    // In the addon besides the render: the edit, the hand-over to the render
    // thread and the call back into JavaScript.
    aroundRender: frame.renderedAt - frame.receivedAt - frame.renderMs,
    // Main process to page: the pixels over IPC and into the page's world.
    toPage: arrivedAt - frame.renderedAt,
    // Into the canvas.
    draw: drawnAt - arrivedAt,
    // The change to pixels in the canvas, and to the next repaint.
    toPixels: drawnAt - madeAt,
    toPaint: presentedAt - madeAt,
  })
  const summary = (journeys) => {
    const stat = (key) => {
      const sorted = journeys.map((j) => j[key]).sort((a, b) => a - b)
      const at = (q) => sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))]
      return { median: at(0.5), p90: at(0.9), max: sorted[sorted.length - 1] }
    }
    const keys = ['render', 'toMain', 'aroundRender', 'toPage', 'draw', 'toPixels', 'toPaint']
    return {
      count: journeys.length,
      size: [journeys[0].width, journeys[0].height],
      ...Object.fromEntries(keys.map((key) => [key, stat(key)])),
    }
  }
  const exposureAt = (step) => -1 + (2 * (step % 21)) / 20

  const report = { devicePixelRatio, errors: [] }
  try {
    await sleep(400)
    await shot('1-empty')

    const openEdit = nextEdit()
    const first = frameWhere((frame) => !frame.draft)
    app.opened(JSON.parse(await editor.probe('open', openEdit)))
    await first
    await shot('2-photo-open')
    report.photo = app.photo
    report.viewEdge = app.viewEdge

    // Live editing: one slider change at a time, each waited for. Whether a
    // draft shows first is the library's rule (when the last frames were slow
    // and the photo is large); the first pixels of an edit are its draft's
    // when one came, else its frame's.
    const fullFrames = []
    const firstPixels = []
    for (let step = 1; step <= 40; step += 1) {
      const edit = slide('exposure', exposureAt(step))
      const first = frameWhere((frame) => frame.edit === edit)
      const full = frameOf(edit, false)
      firstPixels.push(journey(await first))
      fullFrames.push(journey(await full))
    }
    report.frame = summary(fullFrames.slice(5))
    report.firstPixels = summary(firstPixels.slice(5))
    report.draftsShown = firstPixels.slice(5).filter((j) => j.draft).length

    // A fast drag: far more changes than renders. Nothing queues: the frames
    // shown are fewer than the edits, in order, and the last edit's frame ends it.
    const drag = async () => {
      const before = frames.length
      const started = clock()
      let edit = 0
      const edits = 300
      for (let step = 0; step < edits; step += 1) {
        edit = slide('exposure', -1 + (2 * step) / (edits - 1))
        if (step % 4 === 3) await sleep(0)
      }
      const sent = clock() - started
      const lastFrame = await frameOf(edit, false)
      const shown = frames.slice(before).map((entry) => entry.frame)
      const inOrder = shown.every((frame, i) => i === 0 || frame.edit >= shown[i - 1].edit)
      return {
        edits,
        sentInMs: sent,
        framesShown: shown.filter((frame) => !frame.draft).length,
        draftsShown: shown.filter((frame) => frame.draft).length,
        shownInOrder: inOrder,
        lastEditShownMsAfterItWasMade: lastFrame.presentedAt - lastFrame.madeAt,
      }
    }
    report.fastDrag = await drag()

    // The edited state: sliders, then two points on the curve, one dragged.
    for (const [id, value] of [
      ['exposure', 0.35],
      ['contrast', 0.2],
      ['highlights', -0.4],
      ['shadows', 0.3],
      ['temperature', 5200],
    ]) {
      slide(id, value)
    }
    const curve = $('curve')
    const box = curve.getBoundingClientRect()
    const on = (x, y) => [box.left + 12 + x * (box.width - 24), box.top + 12 + (1 - y) * (box.height - 24)]
    const settled = () => sleep(120)
    pointer(curve, 'pointerdown', ...on(0.25, 0.25))
    await settled()
    pointer(curve, 'pointermove', ...on(0.25, 0.16))
    await settled()
    pointer(curve, 'pointerup', ...on(0.25, 0.16))
    await settled()
    pointer(curve, 'pointerdown', ...on(0.7, 0.7))
    await settled()
    pointer(curve, 'pointermove', ...on(0.72, 0.8))
    await settled()
    pointer(curve, 'pointerup', ...on(0.72, 0.8))
    await settled()
    await latestFrame()
    report.curveAfterEditing = app.curve.points
    await shot('3-edited')

    // A point removed again by a double-click, then Export.
    const before = app.curve.points.length
    curve.dispatchEvent(
      new MouseEvent('dblclick', { clientX: on(0.72, 0.8)[0], clientY: on(0.72, 0.8)[1], bubbles: true }),
    )
    await settled()
    report.pointsAfterDoubleClick = [before, app.curve.points.length]
    $('export').click()
    for (let waited = 0; waited < 200 && !$('message').textContent.startsWith('Exported'); waited += 1) {
      await sleep(50)
    }
    report.exportMessage = $('message').textContent
    await shot('4-exported')
  } catch (error) {
    report.errors.push(String(error.stack || error))
  }
  report.message = $('message').textContent
  await editor.probe('report', report)
})
