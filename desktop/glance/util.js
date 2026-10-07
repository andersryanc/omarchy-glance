// Helpers shared by the widget controls: graph presentation per source (as in
// src/sources.rs), colours, and time formatting.
.pragma library

// Per graph id: default label, gradients (btop theme name and fallback stops,
// one per series), mirrored series, and sample value text for sizing.
const GREEN_RED = ["#77ca9b", "#cbc06c", "#dc4c4c"]
const BLUE_RED = ["#4897d4", "#a77fd4", "#dc4c4c"]
const DOWNLOAD = ["#291f75", "#4f43a3", "#b0a9de"]
const UPLOAD = ["#620665", "#7d4180", "#dcafde"]
const TEMP = ["#4897d4", "#5474e8", "#ff40b6"]
const RED_GREEN = ["#dc4c4c", "#cbc06c", "#77ca9b"]

function graphKind(id) { return id.startsWith("glance.") ? id.slice(7) : id }

// A battery shows a level meter unless it graphs charge or power history.
function isMeter(id, options) { return graphKind(id) === "battery" && (options.graph ?? "level") === "level" }

function isMirrored(id) { return ["network", "disk"].includes(graphKind(id)) }

function gradientSpecs(id, options) {
  switch (graphKind(id)) {
  case "cpu": case "gpu": return [["cpu", GREEN_RED]]
  case "memory": return [["used", BLUE_RED]]
  case "network": case "disk": return [["download", DOWNLOAD], ["upload", UPLOAD]]
  case "fan": return [["temp", TEMP]]
  case "battery":
    if (isMeter(id, options)) return [["battery", RED_GREEN]] // btop has none
    return options.graph === "power" ? [["temp", TEMP]] : [["cpu", GREEN_RED]]
  }
  return [["cpu", GREEN_RED]]
}

// One list of stops per series: the widget's `gradient`, else the btop theme
// (theme: name_start/_mid/_end -> colour), else the fallback.
function gradients(id, options, theme) {
  const custom = options.gradient
  const lists = Array.isArray(custom) && custom.length > 0
    ? (typeof custom[0] === "string" ? [custom] : custom) : null
  return gradientSpecs(id, options).map(([name, fallback], i) => {
    if (lists) {
      const stops = lists[Math.min(i, lists.length - 1)]
      if (Array.isArray(stops) && stops.length > 0) return stops
    }
    const themed = ["start", "mid", "end"].map(k => theme[name + "_" + k]).filter(c => !!c)
    return themed.length > 0 ? themed : fallback
  })
}

function defaultLabel(id) {
  return { cpu: "cpu", memory: "mem", gpu: "gpu", network: "net", disk: "disk", fan: "fan", battery: "" }[graphKind(id)] ?? ""
}

// Nerd Font battery glyph for a charge and status, as the Touch Bar picks it.
function batteryIcon(battery) {
  if (!battery) return String.fromCodePoint(0xF0091)
  if (battery.status === "Charging") return String.fromCodePoint(0xF0084)
  if (battery.charge >= 95) return String.fromCodePoint(0xF0079)
  const step = Math.min(8, Math.max(0, roundEven(battery.charge / 10) - 1))
  return String.fromCodePoint(0xF007A + step)
}

function graphLabel(id, options, state) {
  if (graphKind(id) === "battery" && options.label === undefined) return batteryIcon(state.battery)
  return options.label === undefined || options.label === null ? defaultLabel(id) : String(options.label)
}

// The longest value lines, so a graph doesn't change width as values do.
function widestValues(id, options) {
  const k = graphKind(id)
  if ((k === "cpu" || k === "gpu") && options.temperature) return ["100%", "100°"]
  if (k === "disk" && options.show === "usage") return ["100%"]
  if (k === "network") return ["↓999.9M"]
  if (k === "disk") return ["R 999.9M"]
  if (k === "battery" && (options.detail ?? "none") !== "none") return ["100%", "10:00"]
  if (k === "fan") return ["9999"]
  return ["100%"]
}

function roundEven(v) {
  const r = Math.round(v)
  return Math.abs(v % 1) === 0.5 && r % 2 !== 0 ? r - 1 : r
}

// "#rrggbb", or the "rgb(r,g,b)" that lerp() returns, as [r, g, b].
function parseColor(c) {
  const s = String(c)
  if (s.startsWith("rgb(")) return s.slice(4, -1).split(",").map(Number)
  const v = s.replace("#", "")
  return [parseInt(v.slice(0, 2), 16), parseInt(v.slice(2, 4), 16), parseInt(v.slice(4, 6), 16)]
}

// Colour at t (0-1) along a list of "#rrggbb" or rgb() stops, as a CSS rgb() string.
function lerp(stops, t) {
  const cs = stops.map(parseColor)
  if (cs.length === 1) return "rgb(" + cs[0].join(",") + ")"
  const pos = Math.min(1, Math.max(0, t)) * (cs.length - 1)
  const i = Math.min(Math.floor(pos), cs.length - 2)
  const f = pos - i
  const c = [0, 1, 2].map(k => Math.round(cs[i][k] + (cs[i + 1][k] - cs[i][k]) * f))
  return "rgb(" + c.join(",") + ")"
}

const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]
const LONG_DAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"]
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]

// The strftime codes the agents widget's formats use, in local time.
function strftime(d, fmt) {
  const pad = n => (n < 10 ? "0" : "") + n
  return fmt.replace(/%-?([a-zA-Z%])/g, (m, c) => {
    const nopad = m.length === 3
    const p = n => (nopad ? String(n) : pad(n))
    switch (c) {
    case "H": return p(d.getHours())
    case "I": return p(d.getHours() % 12 || 12)
    case "M": return p(d.getMinutes())
    case "S": return p(d.getSeconds())
    case "p": return d.getHours() < 12 ? "AM" : "PM"
    case "a": return DAYS[d.getDay()]
    case "A": return LONG_DAYS[d.getDay()]
    case "b": return MONTHS[d.getMonth()]
    case "d": return p(d.getDate())
    case "e": return String(d.getDate())
    case "m": return p(d.getMonth() + 1)
    case "y": return pad(d.getFullYear() % 100)
    case "Y": return String(d.getFullYear())
    case "%": return "%"
    }
    return m
  })
}

// When a limit resets: a clock time (with the weekday if more than a day
// away), or with resets "countdown" the time left.
function resetText(options, resetsAt, now) {
  const mode = options.resets ?? "time"
  if (mode === "none") return ""
  if (!resetsAt) return "—"
  const at = new Date(resetsAt)
  const left = (at - now) / 1000
  if (left <= 0) return "now"
  if (mode === "countdown") {
    const minutes = Math.floor(left / 60)
    const days = Math.floor(minutes / 1440), hours = Math.floor(minutes / 60) % 24, mins = minutes % 60
    if (days > 0) return days + "d " + hours + "h"
    if (hours > 0) return hours + "h " + (mins < 10 ? "0" : "") + mins + "m"
    return mins + "m"
  }
  return strftime(at, left < 86400 ? (options.timeFormat ?? "%H:%M") : (options.dayTimeFormat ?? "%a %H:%M"))
}

// A sample of the longest reset text, so the widget keeps its width.
function resetWidest(options) {
  const mode = options.resets ?? "time"
  if (mode === "none") return ""
  if (mode === "countdown") return "6d 23h"
  const sample = new Date(2026, 8, 30, 23, 59) // a Wednesday
  const day = strftime(sample, options.dayTimeFormat ?? "%a %H:%M")
  const time = strftime(sample, options.timeFormat ?? "%H:%M")
  return time.length > day.length ? time : day
}

function shortLabel(options, label) {
  const custom = options.shortLabels
  if (custom && custom[label] !== undefined) return String(custom[label])
  const known = { "Session": "5h", "5h window": "5h", "Weekly": "7d" }
  return known[label] ?? label.slice(0, 1).toLowerCase()
}

function limits(state) {
  const l = state.limits ?? []
  return l.length > 0 ? l : [{ label: "Session", fraction: null, resetsAt: null }, { label: "Weekly", fraction: null, resetsAt: null }]
}

function percent(fraction) { return fraction === null || fraction === undefined ? "—" : roundEven(fraction * 100) + "%" }
