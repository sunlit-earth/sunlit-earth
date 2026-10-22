"use strict";

// ---------------------------------------------------------------- helpers
const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const clamp = (v, a, b) => Math.min(b, Math.max(a, v));
const wrap180 = a => ((a + 180) % 360 + 360) % 360 - 180;
const RAD = Math.PI / 180;
const DAY = 86400000;
const HOUR = 3600000;
const now = () => performance.now();

const sub = (a, b) => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const add = (a, b) => [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
const scale = (a, k) => [a[0] * k, a[1] * k, a[2] * k];
const dot = (a, b) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const cross = (a, b) => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
const norm = a => scale(a, 1 / Math.hypot(a[0], a[1], a[2]));

// ---------------------------------------------------------------- platform
let mac = false;
const KEY_LABELS = {
  pc: { Mod: "Ctrl", Shift: "Shift", Help: ["F1"] },
  mac: { Mod: "⌘", Shift: "⇧", Help: ["⌘", "?"] },
};
const labels = () => KEY_LABELS[mac ? "mac" : "pc"];
const keyLabel = k => labels()[k] || k;
const kbd = keys => keys.flatMap(k => k === "Help" ? labels().Help : [keyLabel(k)]).map(k => `<kbd>${k}</kbd>`).join(" ");

// ---------------------------------------------------------------- camera state (the app's model)
const DIST_MIN = 1.5, DIST_MAX = 80;
const zoomToDist = z => DIST_MIN * Math.pow(DIST_MAX / DIST_MIN, z);
const distToZoom = d => Math.log(d / DIST_MIN) / Math.log(DIST_MAX / DIST_MIN);
const DEF = { lon: 0, lat: 30, zoom: distToZoom(8), tilt: 0, yaw: 0, pitch: 0, fov: 20, ox: 0, oy: 0 };
const cam = { ...DEF, lon: 8, lat: 28 };
const LIMITS = { lat: [-89, 89], zoom: [0, 1], yaw: [-90, 90], pitch: [-90, 90], fov: [10, 170], ox: [-3, 3], oy: [-3, 3] };

function setCam(key, v) {
  if (key === "lon" || key === "tilt") v = wrap180(v);
  else if (LIMITS[key]) v = clamp(v, ...LIMITS[key]);
  cam[key] = v;
}

const PRESETS = [
  ["Africa", { lon: 20, lat: -10, zoom: .35 }],
  ["N. America", { lon: -102, lat: 32, zoom: .3, oy: .28 }],
  ["S. America", { lon: -60, lat: -20, zoom: .35 }],
  ["Asia", { lon: 90, lat: 24, zoom: .35 }],
  ["Europe", { lon: 11, lat: 24, zoom: .19, pitch: 30 }],
  ["Oceania", { lon: 147, lat: -5, zoom: .22, tilt: -155, pitch: -26 }],
  ["Pacific", { lon: -150, lat: -20, zoom: .35 }],
  ["Blue Marble", { lon: 37.4, lat: -26.3, zoom: .35, tilt: -176 }],
  ["Earthrise", { lon: -12, lat: 4, zoom: .75, tilt: -116, pitch: 45 }],
];

let tweens = [];
function tweenTo(target, ms = 450) {
  const t0 = now();
  for (const [k, to] of Object.entries(target)) {
    const from = cam[k];
    const end = (k === "lon" || k === "tilt") ? from + wrap180(to - from) : to;
    tweens = tweens.filter(t => t.k !== k);
    tweens.push({ k, from, to: end, t0, ms });
  }
}
function runTweens() {
  const t = now();
  tweens = tweens.filter(tw => {
    const x = clamp((t - tw.t0) / tw.ms, 0, 1);
    const e = 1 - Math.pow(1 - x, 3);
    setCam(tw.k, tw.from + (tw.to - tw.from) * e);
    return x < 1;
  });
}

const touched = {};
const touch = key => { touched[key] = now(); };

// ---------------------------------------------------------------- camera actions
function orbitByDrag(dx, dy, gain) {
  const t = cam.tilt * RAD, c = Math.cos(t), s = Math.sin(t);
  const d0 = dx * c + dy * s, d1 = -dx * s + dy * c;
  setCam("lon", cam.lon - d0 * gain);
  setCam("lat", cam.lat + d1 * gain);
  touch("orbit");
}
const coarseGain = () => 0.3 * zoomToDist(cam.zoom) / 8;
// joysticks move the camera the way they are pushed, so the picture slides the other way
function orbitRate(vx, vy, dt) {
  const degPerSec = Math.min(120, 70 * zoomToDist(cam.zoom) / 8);
  orbitByDrag(-vx, vy, degPerSec * dt);
}
function lookRate(vx, vy, dt) { setCam("yaw", cam.yaw + vx * 45 * dt); setCam("pitch", cam.pitch + vy * 45 * dt); touch("look"); }
function panRate(vx, vy, dt) { const k = 1.4 * dt; setCam("ox", cam.ox + vx * k); setCam("oy", cam.oy + vy * k); touch("pan"); }
function zoomBy(d) { setCam("zoom", cam.zoom + d); touch("zoom"); }
function rollBy(d) { setCam("tilt", cam.tilt + d); touch("roll"); }
const snap = (x, at, within) => Math.abs(x - at) < within ? at : x;

// ---------------------------------------------------------------- time state
const T = { live: true, anchor: Date.now(), wall: now(), playing: false, rate: 60, kind: "clock", zone: "local" };
const MIN_T = Date.UTC(1900, 0, 1), MAX_T = Date.UTC(2100, 11, 31);
const LADDER = { clock: [1, 5, 10, 30, 60, 120, 300, 600, 1800, 3600, 7200, 21600], daily: [1, 2, 7, 14, 30, 60, 90] };
const DEFAULT_RATE = { clock: 60, daily: 7 };
function sim() {
  if (T.live) return Date.now();
  if (!T.playing) return T.anchor;
  const s = (now() - T.wall) / 1000;
  const t = T.kind === "clock" ? T.anchor + T.rate * s * 1000 : T.anchor + Math.trunc(T.rate * s) * DAY;
  if (t < MIN_T || t > MAX_T) { T.anchor = clamp(t, MIN_T, MAX_T); T.playing = false; return T.anchor; }
  return t;
}
function rebase() { const t = sim(); T.anchor = t; T.wall = now(); return t; }
function setTime(ms) { T.live = false; T.anchor = clamp(ms, MIN_T, MAX_T); T.wall = now(); }
function goLive() { T.live = true; T.playing = false; }
function setPlaying(p) { const t = rebase(); T.live = false; T.anchor = t; T.playing = p; }
function setRate(r) { rebase(); T.live = false; T.rate = r; T.playing = true; }
const rateIndex = () => LADDER[T.kind].indexOf(Math.abs(T.rate));
const dirOf = () => Math.sign(T.rate) || 1;
function setMagnitude(i) {
  const lad = LADDER[T.kind];
  rebase();
  T.rate = dirOf() * lad[clamp(i, 0, lad.length - 1)];
}
const faster = () => setMagnitude(rateIndex() + 1);
const slower = () => setMagnitude(rateIndex() - 1);
function setDir(d) { rebase(); T.rate = d * Math.abs(T.rate); }
function setKind(k) {
  if (T.kind === k) return;
  rebase();
  const d = dirOf();
  T.kind = k;
  T.rate = d * DEFAULT_RATE[k];
}
// J and L: start in their direction from pause, speed up along it, slow down and stop against it
function shuttle(d) {
  if (!T.playing) { setDir(d); setPlaying(true); return; }
  if (dirOf() === d) faster();
  else if (rateIndex() === 0) setPlaying(false);
  else slower();
}
function step(dir, big) {
  const t = rebase();
  const size = T.kind === "clock" ? (big ? HOUR : 15 * 60000) : (big ? 7 : 1) * DAY;
  setTime(t + dir * size);
}
const CLOCK_NOTES = { 1: "real time", 5: "5 s/s", 10: "10 s/s", 30: "30 s/s", 60: "1 min/s", 120: "2 min/s", 300: "5 min/s", 600: "10 min/s", 1800: "30 min/s", 3600: "1 h/s", 7200: "2 h/s", 21600: "6 h/s" };
const DAILY_LABELS = { 1: "1 day/s", 2: "2 days/s", 7: "1 week/s", 14: "2 weeks/s", 30: "1 month/s", 60: "2 months/s", 90: "3 months/s" };
function rateLabel(r, kind = T.kind) {
  const s = r < 0 ? "−" : "";
  const a = Math.abs(r);
  return s + (kind === "daily" ? DAILY_LABELS[a] : a + "×");
}
const rateNote = (r, kind = T.kind) => kind === "clock" ? CLOCK_NOTES[Math.abs(r)] : "";

function zoneOff(t) { return T.zone === "local" ? -new Date(t).getTimezoneOffset() * 60000 : 0; }
function zoneName(zone, t) {
  if (zone === "utc") return "UTC";
  try {
    return new Intl.DateTimeFormat(undefined, { timeZoneName: "short" }).formatToParts(new Date(t)).find(p => p.type === "timeZoneName").value;
  } catch { return "local"; }
}
function parts(t, zone = T.zone) {
  const off = zone === "local" ? -new Date(t).getTimezoneOffset() * 60000 : 0;
  const d = new Date(t + off);
  return { y: d.getUTCFullYear(), mo: d.getUTCMonth(), da: d.getUTCDate(), wd: d.getUTCDay(), h: d.getUTCHours(), mi: d.getUTCMinutes(), s: d.getUTCSeconds(), off, msOfDay: ((t + off) % DAY + DAY) % DAY };
}
const MON = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const WD = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const pad2 = n => String(n).padStart(2, "0");
const fmtDate = (p, year = true) => `${WD[p.wd]} ${p.da} ${MON[p.mo]}${year ? " " + p.y : ""}`;
const fmtTime = (p, secs) => `${pad2(p.h)}:${pad2(p.mi)}${secs ? ":" + pad2(p.s) : ""}`;
const dayStart = t => t - parts(t).msOfDay;
const daysInYear = y => ((y % 4 === 0 && y % 100 !== 0) || y % 400 === 0) ? 366 : 365;
function fmtOffset(ms) {
  const sign = ms < 0 ? "−" : "+";
  let a = Math.abs(ms) / 60000;
  if (a < 1) return "now";
  const y = Math.floor(a / 525960); a -= y * 525960;
  const d = Math.floor(a / 1440); a -= d * 1440;
  const h = Math.floor(a / 60); const m = Math.floor(a - h * 60);
  if (y) return `${sign}${y} y ${d} d`;
  if (d) return `${sign}${d} d ${h} h`;
  if (h) return `${sign}${h} h ${m} min`;
  return `${sign}${m} min`;
}

// ---------------------------------------------------------------- the Sun
function sunEcl(ms) {
  const n = (ms - 946728000000) / DAY;
  const L = 280.460 + 0.9856474 * n, g = (357.528 + 0.9856003 * n) * RAD;
  return { n, lam: (L + 1.915 * Math.sin(g) + 0.020 * Math.sin(2 * g)) * RAD };
}
function sunDir(ms) {
  const { n, lam } = sunEcl(ms);
  const eps = (23.439 - 4e-7 * n) * RAD;
  const dec = Math.asin(Math.sin(eps) * Math.sin(lam));
  const ra = Math.atan2(Math.cos(eps) * Math.sin(lam), Math.cos(lam));
  const lon = ra - (280.46061837 + 360.98564736629 * n) * RAD;
  return [Math.cos(dec) * Math.sin(lon), Math.sin(dec), Math.cos(dec) * Math.cos(lon)];
}
const seasonCache = {};
function seasons(y) {
  if (seasonCache[y]) return seasonCache[y];
  const names = ["March equinox", "June solstice", "September equinox", "December solstice"];
  const out = [0, 90, 180, 270].map((target, i) => {
    let t = Date.UTC(y, 2 + 3 * i, 21);
    for (let k = 0; k < 6; k++) t += wrap180(target - sunEcl(t).lam / RAD) / 0.9856 * DAY;
    return { name: names[i], t };
  });
  return (seasonCache[y] = out);
}
const subCam = () => { const la = cam.lat * RAD, lo = cam.lon * RAD; return [Math.cos(la) * Math.sin(lo), Math.sin(la), Math.cos(la) * Math.cos(lo)]; };
const altAt = (p, t) => Math.asin(clamp(dot(p, sunDir(t)), -1, 1)) / RAD;
const ALT_COLORS = [[-18, [10, 18, 38]], [-12, [18, 29, 63]], [-6, [59, 47, 99]], [-2, [150, 74, 92]], [1, [224, 135, 74]], [6, [226, 178, 102]], [15, [93, 154, 214]], [40, [63, 134, 208]]];
function altColor(a) {
  if (a <= ALT_COLORS[0][0]) return `rgb(${ALT_COLORS[0][1]})`;
  for (let i = 1; i < ALT_COLORS.length; i++) {
    const [a1, c1] = ALT_COLORS[i];
    if (a <= a1) {
      const [a0, c0] = ALT_COLORS[i - 1];
      const x = (a - a0) / (a1 - a0);
      return `rgb(${c0.map((v, j) => Math.round(v + (c1[j] - v) * x))})`;
    }
  }
  return `rgb(${ALT_COLORS[ALT_COLORS.length - 1][1]})`;
}

// ---------------------------------------------------------------- WebGL globe
const canvas = $("#globe");
const gl = canvas.getContext("webgl2", { antialias: false });
const VS = `#version 300 es
in vec2 p; void main(){ gl_Position = vec4(p, 0.0, 1.0); }`;
const FS = `#version 300 es
precision highp float;
uniform vec2 uRes; uniform vec3 uEye, uS, uU, uF, uSun; uniform float uTan, uTilt; uniform vec2 uOff;
uniform sampler2D uDay, uNight, uClouds;
out vec4 o;
float hash(vec3 p){ p = fract(p * 0.3183099 + 0.1); p *= 17.0; return fract(p.x * p.y * p.z * (p.x + p.y + p.z)); }
vec3 stars(vec3 d){
  vec3 q = d * 260.0; vec3 id = floor(q); vec3 f = fract(q) - 0.5;
  float h = hash(id);
  if (h < 0.975) return vec3(0.0);
  float b = (h - 0.975) / 0.025;
  return vec3(0.85 + 0.15 * hash(id + 3.1), 0.88, 1.0) * b * b * smoothstep(0.32, 0.0, length(f)) * 1.4;
}
vec4 tex(sampler2D s, vec3 n){
  float lon = atan(n.x, n.z); float lat = asin(clamp(n.y, -1.0, 1.0));
  vec2 uv = vec2(lon / 6.2831853 + 0.5, 0.5 - lat / 3.14159265);
  vec2 dx = dFdx(uv), dy = dFdy(uv);
  if (abs(dx.x) > 0.5) dx.x -= sign(dx.x);
  if (abs(dy.x) > 0.5) dy.x -= sign(dy.x);
  return textureGrad(s, uv, dx, dy);
}
void main(){
  vec2 ndc = gl_FragCoord.xy / uRes * 2.0 - 1.0 + uOff;
  vec2 p = vec2(ndc.x * uTan * uRes.x / uRes.y, ndc.y * uTan);
  float c = cos(-uTilt), s = sin(-uTilt);
  p = vec2(c * p.x - s * p.y, s * p.x + c * p.y);
  vec3 rd = normalize(uS * p.x + uU * p.y + uF);
  float b = dot(uEye, rd); float cc = dot(uEye, uEye) - 1.0; float h = b * b - cc;
  float t = -b - sqrt(max(h, 0.0));
  bool hit = h > 0.0 && t > 0.0;
  vec3 n = normalize(uEye + rd * max(t, 0.0));
  vec3 day = tex(uDay, n).rgb; vec3 night = tex(uNight, n).rgb; float cl = tex(uClouds, n).r;
  float ndl = dot(n, uSun);
  vec3 col;
  if (hit) {
    float cloud = smoothstep(0.42, 0.95, cl);
    float lit = smoothstep(-0.10, 0.10, ndl);
    float shade = 0.5 + 0.5 * clamp(ndl * 1.5 + 0.15, 0.0, 1.0);
    vec3 dcol = mix(day * 1.08, vec3(0.95), cloud * 0.85) * shade;
    vec3 ncol = night * vec3(1.0, 0.86, 0.62) * 1.5 * (1.0 - cloud * 0.8) + vec3(0.010, 0.014, 0.028);
    col = mix(ncol, dcol, lit);
    col += vec3(1.0, 0.45, 0.15) * exp(-pow(ndl / 0.05, 2.0)) * 0.10 * (1.0 - cloud * 0.6);
    float rim = pow(1.0 - max(dot(n, -rd), 0.0), 3.0);
    col = mix(col, vec3(0.38, 0.62, 1.0), rim * 0.55 * smoothstep(-0.25, 0.25, ndl));
  } else {
    col = stars(rd);
    if (b < 0.0) {
      float dmin = sqrt(max(cc - b * b + 1.0, 0.0));
      vec3 cp = normalize(uEye + rd * (-b));
      col += vec3(0.30, 0.55, 1.0) * exp(-(dmin - 1.0) / 0.016) * smoothstep(-0.3, 0.3, dot(cp, uSun)) * 0.85;
    }
    float sd = dot(rd, uSun);
    col += vec3(1.0, 0.95, 0.85) * (smoothstep(0.999980, 0.999992, sd) * 5.0 + pow(max(sd, 0.0), 900.0) * 0.7);
  }
  o = vec4(col, 1.0);
}`;
function compile(type, src) {
  const sh = gl.createShader(type); gl.shaderSource(sh, src); gl.compileShader(sh);
  if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(sh));
  return sh;
}
const prog = gl.createProgram();
gl.attachShader(prog, compile(gl.VERTEX_SHADER, VS));
gl.attachShader(prog, compile(gl.FRAGMENT_SHADER, FS));
gl.linkProgram(prog);
if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(prog));
gl.useProgram(prog);
gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
const loc = gl.getAttribLocation(prog, "p");
gl.enableVertexAttribArray(loc);
gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
const U = {};
for (const n of ["uRes", "uEye", "uS", "uU", "uF", "uSun", "uTan", "uTilt", "uOff", "uDay", "uNight", "uClouds"]) U[n] = gl.getUniformLocation(prog, n);
function makeTex(unit, src) {
  const tex = gl.createTexture();
  gl.activeTexture(gl.TEXTURE0 + unit);
  gl.bindTexture(gl.TEXTURE_2D, tex);
  gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, 1, 1, 0, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array([10, 20, 40, 255]));
  const img = new Image();
  img.onload = () => {
    gl.activeTexture(gl.TEXTURE0 + unit);
    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, img);
    gl.generateMipmap(gl.TEXTURE_2D);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.REPEAT);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  };
  img.src = src;
}
const TX = window.MOCKUP_TEXTURES || {};
makeTex(0, TX.day); makeTex(1, TX.night); makeTex(2, TX.clouds);
gl.uniform1i(U.uDay, 0); gl.uniform1i(U.uNight, 1); gl.uniform1i(U.uClouds, 2);

function resize() {
  const dpr = Math.min(window.devicePixelRatio || 1, 1.5);
  const w = Math.round(canvas.clientWidth * dpr), h = Math.round(canvas.clientHeight * dpr);
  if (canvas.width !== w || canvas.height !== h) { canvas.width = w; canvas.height = h; }
}
function render(t) {
  resize();
  gl.viewport(0, 0, canvas.width, canvas.height);
  const d = zoomToDist(cam.zoom), lo = cam.lon * RAD, la = clamp(cam.lat, -89.9, 89.9) * RAD;
  const eye = [d * Math.cos(la) * Math.sin(lo), d * Math.sin(la), d * Math.cos(la) * Math.cos(lo)];
  const f0 = norm(scale(eye, -1)), r0 = norm(cross(f0, [0, 1, 0])), u0 = cross(r0, f0);
  const target = add(scale(r0, Math.sin(cam.yaw * RAD)), scale(u0, Math.sin(cam.pitch * RAD)));
  const f = norm(sub(target, eye)), s = norm(cross(f, [0, 1, 0])), u = cross(s, f);
  gl.uniform2f(U.uRes, canvas.width, canvas.height);
  gl.uniform3fv(U.uEye, eye); gl.uniform3fv(U.uS, s); gl.uniform3fv(U.uU, u); gl.uniform3fv(U.uF, f);
  gl.uniform3fv(U.uSun, sunDir(t));
  gl.uniform1f(U.uTan, Math.tan(cam.fov * RAD / 2));
  gl.uniform1f(U.uTilt, cam.tilt * RAD);
  gl.uniform2f(U.uOff, cam.ox, cam.oy);
  gl.drawArrays(gl.TRIANGLES, 0, 3);
}

// ---------------------------------------------------------------- mouse glyphs and tooltips
function mouseSVG(which, h = 18) {
  const on = "#4cc2ff", off = "rgba(255,255,255,.10)";
  const L = which === "left" || which === "both", R = which === "right" || which === "both";
  const W = which === "middle" || which === "wheel";
  const arrows = which === "wheel"
    ? `<path d="M14.2 3.5 15.6 1.6 17 3.5M14.2 12.5 15.6 14.4 17 12.5" fill="none" stroke="${on}" stroke-width="1.1"/>` : "";
  const w = which === "wheel" ? 18 : 12;
  return `<svg width="${h * w / 16}" height="${h}" viewBox="0 0 ${w} 16" style="flex:none"><path d="M6 .8A5.2 5.2 0 0 0 .8 6v.6H6z" fill="${L ? on : off}"/><path d="M6 .8A5.2 5.2 0 0 1 11.2 6v.6H6z" fill="${R ? on : off}"/><rect x=".8" y=".8" width="10.4" height="14.4" rx="5.2" fill="none" stroke="rgba(255,255,255,.75)" stroke-width="1"/><path d="M.8 6.6h10.4M6 .8v5.8" stroke="rgba(255,255,255,.55)" stroke-width=".8"/><rect x="4.9" y="2.4" width="2.2" height="3.2" rx="1.1" fill="${W ? on : "rgba(255,255,255,.55)"}"/>${arrows}</svg>`;
}
// "Mod" and "Shift" in the texts become the platform's names
const TIPS = {
  orbit: { t: "Orbit", d: "Push the knob and hold: the camera travels over the globe the way you push, faster the farther. The arrows step 10°.", m: "left", g: "Drag the globe with the left button", k: ["←", "↑", "↓", "→"] },
  roll: { t: "Roll", d: "Drag around the ring to turn the picture about its center.", m: "both", g: "Drag with left and right held", k: ["Q", "E"] },
  north: { t: "North up", d: "Turn the picture back so that north points up.", k: ["N"] },
  zoom: { t: "Zoom", d: "Near at the top, far at the bottom. The mark is the default, and the thumb snaps to it.", m: "wheel", g: "Scroll the mouse wheel", k: ["+", "−"] },
  zoomin: { t: "Zoom in", d: "Click for a step, hold to keep going.", m: "wheel", g: "Scroll the mouse wheel", k: ["+"] },
  zoomout: { t: "Zoom out", d: "Click for a step, hold to keep going.", m: "wheel", g: "Scroll the mouse wheel", k: ["−"] },
  look: { t: "Look", d: "Aim the camera away from the Earth's center. The view swings the way you push; the globe slides the other way.", m: "middle", g: "Drag with the middle button", k: ["Shift", "←↑↓→"] },
  lookreset: { t: "Look straight at the Earth", d: "Yaw and pitch back to 0°." },
  pan: { t: "Move the frame", d: "Moves the camera across the picture the way you push, without turning it, so the Earth slides the other way and ends up off center.", m: "right", g: "Drag with the right button (the globe follows the hand)", k: ["Mod", "←↑↓→"] },
  panreset: { t: "Center the frame", d: "Both offsets back to 0." },
  help: { t: "Mouse and keyboard", d: "Every gesture and shortcut on one sheet.", k: ["Help"] },
  hide: { t: "Hide controls", d: "Clear the preview to judge the composition.", k: ["H"] },
  show: { t: "Show controls", k: ["H"] },
  stepback: { t: "Step back", d: "15 minutes in Clock mode, a day in Daily mode. With Shift, an hour or a week.", k: [","] },
  stepfwd: { t: "Step forward", d: "15 minutes in Clock mode, a day in Daily mode. With Shift, an hour or a week.", k: ["."] },
  dir: { t: "Direction", d: "Run time backward or forward. Switching keeps the speed.", k: ["R"] },
  ruler: { t: "Speed", d: "Backward on the left, forward on the right, one dot per speed, the slowest next to play. Click a dot or drag along the ruler to run at that speed.", k: ["J", "K", "L"] },
  play: { t: "Play / pause", d: "Run time at the chosen speed. Sun, terminator, Moon and stars move; the clouds stay as observed.", k: ["Space"] },
  rate: { t: "Speed", d: "Click for every speed in both directions.", k: ["J", "K", "L"] },
  drivetime: { t: "Play runs the time", d: "The clock runs, from real time to 6 hours a second, and the date follows. Day and night sweep across the globe." },
  drivedate: { t: "Play runs the date", d: "The date runs, from a day to 3 months a second, and the time of day stays. The terminator swings through the seasons." },
  pickdate: { t: "Date", d: "Click to type an exact date and time." },
  picktime: { t: "Time", d: "Click to type an exact date and time." },
  zone: { t: "Time zone", d: "Switch between UTC and your local time. The time track follows." },
  now: { t: "Back to now", d: "Return to live time, which follows the clock.", k: ["0"] },
  pin: { t: "Keep open", d: "Keep the full bar open and at full strength, wherever the pointer goes." },
  daytrack: { t: "Time of day", d: "Drag to change the time and keep the date. Shaded for day, twilight and night at the point under the camera." },
  yeartrack: { t: "Date", d: "Drag to change the date and keep the time of day. Diamonds mark equinoxes and solstices, and the handle snaps to them." },
};
const platformText = s => s.replace(/\bMod\b/g, keyLabel("Mod")).replace(/\bShift\b/g, keyLabel("Shift"));
const tip = $("#tip");
const overlays = $("#overlays");
const preview = $("#preview");
let tipTimer = null, tipFor = null;
function showTip(el) {
  const d = TIPS[el.dataset.tip];
  if (!d) return;
  tip.innerHTML = `<div class="t">${d.t}</div>${d.d ? `<div class="d">${platformText(d.d)}</div>` : ""}` +
    (d.m || d.k ? `<div class="how">${d.m ? mouseSVG(d.m) + `<span>${platformText(d.g)}</span>` : ""}${d.k ? `<span style="margin-left:auto;white-space:nowrap">${kbd(d.k)}</span>` : ""}</div>` : "");
  const pr = preview.getBoundingClientRect(), r = el.getBoundingClientRect();
  tip.style.left = "0px"; tip.style.top = "0px";
  tip.classList.add("show");
  const tw = tip.offsetWidth, th = tip.offsetHeight;
  let x, y;
  if (r.top - pr.top > pr.height * 0.55) { x = r.left + r.width / 2 - tw / 2 - pr.left; y = r.top - pr.top - th - 10; }
  else if (r.left - pr.left > pr.width * 0.5) { x = r.left - pr.left - tw - 12; y = r.top + r.height / 2 - th / 2 - pr.top; }
  else { x = r.right - pr.left + 12; y = r.top + r.height / 2 - th / 2 - pr.top; }
  tip.style.left = clamp(x, 6, pr.width - tw - 6) + "px";
  tip.style.top = clamp(y, 6, pr.height - th - 6) + "px";
}
function hideTip() { clearTimeout(tipTimer); tip.classList.remove("show"); tipFor = null; }
overlays.addEventListener("mouseover", e => {
  const el = e.target.closest("[data-tip]");
  if (el === tipFor) return;
  hideTip();
  if (!el || drag.active) return;
  tipFor = el;
  tipTimer = setTimeout(() => showTip(el), 450);
});
overlays.addEventListener("mouseleave", hideTip);
overlays.addEventListener("pointerdown", hideTip, true);

// the first use of an overlay control names its mouse gesture
const TEACH = {
  orbit: ["left", "Faster: drag the globe with the left button"],
  zoom: ["wheel", "Or scroll the mouse wheel over the globe"],
  roll: ["both", "Or drag the globe with left and right held"],
  look: ["middle", "Or drag the globe with the middle button"],
  pan: ["right", "Or drag the globe with the right button"],
};
const taught = new Set();
let toastTimer = null;
function teach(key) {
  if (taught.has(key) || !TEACH[key]) return;
  taught.add(key);
  const [m, text] = TEACH[key];
  const el = $("#toast");
  el.innerHTML = `<span style="color:var(--ov-dim)">Tip</span>${mouseSVG(m)}<span>${platformText(text)}</span>`;
  el.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.remove("show"), 2800);
}

// ---------------------------------------------------------------- input primitives
const drag = { active: false };
const ticking = new Set();
function joystick(pad, onRate, key) {
  const knob = $(".knob", pad);
  let held = false, vx = 0, vy = 0;
  const lim = () => pad.clientWidth / 2 - 7;
  function setFrom(e) {
    const r = pad.getBoundingClientRect();
    let dx = e.clientX - (r.left + r.width / 2), dy = e.clientY - (r.top + r.height / 2);
    const m = Math.hypot(dx, dy), L = lim();
    if (m > L) { dx *= L / m; dy *= L / m; }
    knob.style.transform = `translate(${dx}px, ${dy}px)`;
    vx = dx / L; vy = -dy / L;
  }
  function start(e) {
    e.preventDefault(); e.stopPropagation();
    held = true; drag.active = true;
    pad.classList.add("held");
    knob.setPointerCapture(e.pointerId);
    setFrom(e);
    teach(key);
  }
  knob.addEventListener("pointerdown", start);
  pad.addEventListener("pointerdown", e => { if (e.target.closest(".chev") || e.target === knob) return; start(e); });
  knob.addEventListener("pointermove", e => { if (held) setFrom(e); });
  const end = () => {
    if (!held) return;
    held = false; drag.active = false;
    pad.classList.remove("held");
    knob.style.transform = "";
    vx = vy = 0;
  };
  knob.addEventListener("pointerup", end);
  knob.addEventListener("pointercancel", end);
  ticking.add(dt => { if (held && (vx || vy)) { const s = Math.hypot(vx, vy); onRate(vx * s, vy * s, dt); } });
}
function holdButton(el, onStep, onRate, key) {
  let held = false, t0 = 0;
  el.addEventListener("pointerdown", e => { e.preventDefault(); el.setPointerCapture(e.pointerId); held = true; t0 = now(); onStep(); teach(key); });
  const end = () => { held = false; };
  el.addEventListener("pointerup", end);
  el.addEventListener("pointercancel", end);
  ticking.add(dt => { if (held && now() - t0 > 350) onRate(dt); });
}
function slider(el, vertical, onValue, key) {
  let held = false;
  function setFrom(e) {
    const r = el.getBoundingClientRect();
    const x = vertical ? (e.clientY - r.top - 4) / (r.height - 8) : (e.clientX - r.left - 4) / (r.width - 8);
    onValue(clamp(x, 0, 1));
  }
  el.addEventListener("pointerdown", e => { e.preventDefault(); e.stopPropagation(); held = true; drag.active = true; el.setPointerCapture(e.pointerId); setFrom(e); teach(key); });
  el.addEventListener("pointermove", e => { if (held) setFrom(e); });
  const end = () => { held = false; drag.active = false; };
  el.addEventListener("pointerup", end);
  el.addEventListener("pointercancel", end);
}
function placeOnSlider(child, el, x, vertical) {
  if (vertical) child.style.top = (4 + x * (el.clientHeight - 8)) + "px";
  else child.style.left = (4 + x * (el.clientWidth - 8)) + "px";
}
function ring(el) {
  const track = $(".track", el);
  let held = false, startAng = 0, startTilt = 0;
  const ang = e => { const r = el.getBoundingClientRect(); return Math.atan2(e.clientX - (r.left + r.width / 2), -(e.clientY - (r.top + r.height / 2))) / RAD; };
  track.addEventListener("pointerdown", e => {
    e.preventDefault(); held = true; drag.active = true; track.setPointerCapture(e.pointerId);
    startAng = ang(e); startTilt = cam.tilt; teach("roll");
  });
  track.addEventListener("pointermove", e => { if (held) { setCam("tilt", startTilt - wrap180(ang(e) - startAng)); touch("roll"); } });
  const end = () => { held = false; drag.active = false; };
  track.addEventListener("pointerup", end);
  track.addEventListener("pointercancel", end);
}
function ringTicks(svg, size) {
  const c = size / 2, r1 = c - 8.5, r2 = c - 3;
  let s = `<g class="rot">`;
  for (let i = 1; i < 12; i++) {
    const a = i * 30 * RAD;
    const long = i % 3 === 0;
    const ra = long ? r1 - 1 : r1 + 1;
    s += `<line x1="${c + ra * Math.sin(a)}" y1="${c - ra * Math.cos(a)}" x2="${c + r2 * Math.sin(a)}" y2="${c - r2 * Math.cos(a)}" stroke="rgba(255,255,255,${long ? .45 : .22})" stroke-width="${long ? 1.4 : 1}"/>`;
  }
  svg.innerHTML = s + "</g>";
}
function placeNorth(el, size) {
  const c = size / 2, r = c - 5.5, a = -cam.tilt * RAD;
  el.style.left = (c + r * Math.sin(a)) + "px";
  el.style.top = (c - r * Math.cos(a)) + "px";
}

// ---------------------------------------------------------------- camera panel
const camA = $("#camA");
ringTicks($("#aTicks"), 78);
ring($("#aRing"));
joystick($("#aOrbit"), orbitRate, "orbit");
$$("#aOrbit .chev").forEach(ch => ch.addEventListener("pointerdown", e => {
  e.preventDefault(); e.stopPropagation();
  const [sx, sy] = ch.dataset.step.split(",").map(Number);
  const t = cam.tilt * RAD, c = Math.cos(t), s = Math.sin(t);
  const dx = -sx, dy = sy;
  tweenTo({ lon: cam.lon - (dx * c + dy * s) * 10, lat: clamp(cam.lat + (-dx * s + dy * c) * 10, -89, 89) }, 250);
  touch("orbit"); teach("orbit");
}));
$("#aNorth").addEventListener("click", () => { tweenTo({ tilt: 0 }); touch("roll"); });
holdButton($("#aZoomIn"), () => tweenTo({ zoom: clamp(cam.zoom - 0.05, 0, 1) }, 200), dt => zoomBy(-0.3 * dt), "zoom");
holdButton($("#aZoomOut"), () => tweenTo({ zoom: clamp(cam.zoom + 0.05, 0, 1) }, 200), dt => zoomBy(0.3 * dt), "zoom");
slider($("#aZoom"), true, x => { setCam("zoom", snap(x, DEF.zoom, 0.03)); touch("zoom"); }, "zoom");
joystick($("#aLook"), lookRate, "look");
joystick($("#aPan"), panRate, "pan");
$("#aLookReset").addEventListener("click", () => { tweenTo({ yaw: 0, pitch: 0 }); touch("look"); });
$("#aPanReset").addEventListener("click", () => { tweenTo({ ox: 0, oy: 0 }); touch("pan"); });
let camHover = false, camUsed = 0;
camA.addEventListener("pointerenter", () => { camHover = true; });
camA.addEventListener("pointerleave", () => { camHover = false; camUsed = now(); });
camA.addEventListener("pointerdown", () => { camUsed = now(); });
placeOnSlider($("#aZoom .mark"), $("#aZoom"), DEF.zoom, true);

// ---------------------------------------------------------------- gestures on the globe
const active = new Set();
const lastUse = {};
let gButtons = 0, gLast = null, wheelTimer = null;
function setGesture() {
  for (const k of ["orbit", "roll", "pan", "look"]) active.delete(k);
  if ((gButtons & 3) === 3) active.add("roll");
  else if (gButtons & 1) active.add("orbit");
  else if (gButtons & 2) active.add("pan");
  else if (gButtons & 4) active.add("look");
  for (const k of active) lastUse[k] = now();
}
preview.addEventListener("contextmenu", e => e.preventDefault());
canvas.addEventListener("mousedown", e => { if (e.button === 1) e.preventDefault(); });
canvas.addEventListener("pointerdown", e => {
  e.preventDefault();
  canvas.setPointerCapture(e.pointerId);
  gButtons = e.buttons; gLast = [e.clientX, e.clientY];
  canvas.classList.add("dragging");
  hideTip(); hidePops();
  setGesture();
});
canvas.addEventListener("pointermove", e => {
  if (!gLast) return;
  gButtons = e.buttons;
  const dx = e.clientX - gLast[0], dy = e.clientY - gLast[1];
  gLast = [e.clientX, e.clientY];
  setGesture();
  if (active.has("roll")) rollBy(dx * 0.5);
  else if (active.has("orbit")) orbitByDrag(dx, dy, coarseGain());
  else if (active.has("pan")) { const k = 0.002 * zoomToDist(cam.zoom) / 8 * 1.25; setCam("ox", cam.ox - dx * k); setCam("oy", cam.oy + dy * k); touch("pan"); }
  else if (active.has("look")) { setCam("yaw", cam.yaw + dx * 0.2); setCam("pitch", cam.pitch - dy * 0.2); touch("look"); }
});
const gEnd = e => {
  gButtons = e.buttons || 0;
  if (!gButtons) { gLast = null; canvas.classList.remove("dragging"); }
  setGesture();
};
canvas.addEventListener("pointerup", gEnd);
canvas.addEventListener("pointercancel", gEnd);
preview.addEventListener("wheel", e => {
  if (e.target !== canvas) return;
  e.preventDefault();
  zoomBy(e.deltaY * 0.0005);
  active.add("zoom"); lastUse.zoom = now();
  clearTimeout(wheelTimer);
  wheelTimer = setTimeout(() => active.delete("zoom"), 350);
}, { passive: false });

// ---------------------------------------------------------------- keyboard
const keys = new Set();
const isTyping = e => e.target.closest && e.target.closest("input, textarea, select");
const toggleSheet = () => { $("#sheet").classList.toggle("show"); hideTip(); };
document.addEventListener("keydown", e => {
  if (isTyping(e)) return;
  const k = e.key;
  const helpKey = mac ? (k === "?" && (e.metaKey || e.ctrlKey)) : k === "F1";
  if (helpKey) { e.preventDefault(); toggleSheet(); return; }
  if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", " "].includes(k)) e.preventDefault();
  if (k === "Escape") { $("#sheet").classList.remove("show"); hidePops(); return; }
  const held = k.length === 1 ? k.toLowerCase() : k;
  if (e.repeat) { keys.add(held); return; }
  if (!e.ctrlKey && !e.metaKey && !e.altKey) {
    switch (e.code) {
      case "Space": setPlaying(!T.playing); return;
      case "KeyJ": shuttle(-1); return;
      case "KeyK": setPlaying(false); return;
      case "KeyL": shuttle(1); return;
      case "KeyR": setDir(-dirOf()); return;
      case "Comma": step(-1, e.shiftKey); return;
      case "Period": step(1, e.shiftKey); return;
      case "KeyN": tweenTo({ tilt: 0 }); touch("roll"); return;
      case "KeyH": overlays.classList.toggle("hidden"); hideTip(); return;
    }
    if (k === "0") { goLive(); return; }
  }
  keys.add(held);
});
document.addEventListener("keyup", e => { keys.delete(e.key.length === 1 ? e.key.toLowerCase() : e.key); });
window.addEventListener("blur", () => keys.clear());
function keyTick(dt) {
  for (const k of [...active]) if (k.endsWith("-key")) active.delete(k);
  let vx = 0, vy = 0;
  if (keys.has("ArrowLeft")) vx -= 1;
  if (keys.has("ArrowRight")) vx += 1;
  if (keys.has("ArrowUp")) vy += 1;
  if (keys.has("ArrowDown")) vy -= 1;
  const mod = keys.has("Control") || (mac && keys.has("Meta")), shift = keys.has("Shift");
  if (vx || vy) {
    if (mod) { panRate(vx * 0.6, vy * 0.6, dt); active.add("pan-key"); }
    else if (shift) { lookRate(vx * 0.6, vy * 0.6, dt); active.add("look-key"); }
    else { orbitRate(vx * 0.6, vy * 0.6, dt); active.add("orbit-key"); }
  }
  if (keys.has("+") || keys.has("=")) { zoomBy(-0.3 * dt); active.add("zoom-key"); }
  if (keys.has("-")) { zoomBy(0.3 * dt); active.add("zoom-key"); }
  if (keys.has("q")) { rollBy(-60 * dt); active.add("roll-key"); }
  if (keys.has("e")) { rollBy(60 * dt); active.add("roll-key"); }
  for (const k of active) if (k.endsWith("-key")) lastUse[k.slice(0, -4)] = now();
}

// ---------------------------------------------------------------- time bar
const timeA = $("#timeA");
let timePinned = false, timeHover = false, timeLeaveTimer = null;
const popOpen = () => $("#ratePop").classList.contains("show") || $("#pickPop").classList.contains("show");
function syncTimeOpen() {
  timeA.classList.toggle("open", timePinned || timeHover || !!tDrag || popOpen());
  timeA.classList.toggle("pinned", timePinned);
}
timeA.addEventListener("pointerenter", () => { clearTimeout(timeLeaveTimer); timeHover = true; syncTimeOpen(); });
timeA.addEventListener("pointerleave", () => { timeLeaveTimer = setTimeout(() => { timeHover = false; syncTimeOpen(); }, 700); });
const PIN = on => `<svg width="14" height="14" viewBox="0 0 14 14"><path d="M5 1.5h4l-.6 4 2.1 2H3.5l2.1-2z" fill="${on ? "currentColor" : "none"}" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"/><path d="M7 7.5V13" stroke="currentColor" stroke-width="1.3"/></svg>`;
function playIcon() {
  return T.playing
    ? `<svg width="12" height="12" viewBox="0 0 12 12"><rect x="2" y="1.5" width="3" height="9" rx=".8" fill="currentColor"/><rect x="7" y="1.5" width="3" height="9" rx=".8" fill="currentColor"/></svg>`
    : dirOf() > 0
      ? `<svg width="12" height="12" viewBox="0 0 12 12"><path d="M3 1.5v9l7.5-4.5z" fill="currentColor"/></svg>`
      : `<svg width="12" height="12" viewBox="0 0 12 12"><path d="M9 1.5v9L1.5 6z" fill="currentColor"/></svg>`;
}
document.addEventListener("click", e => {
  const el = e.target.closest("[data-act]");
  if (!el) { if (!e.target.closest(".pop")) hidePops(); return; }
  const act = el.dataset.act;
  if (act !== "rate" && act !== "pickdate" && act !== "picktime") hidePops();
  switch (act) {
    case "play": setPlaying(!T.playing); break;
    case "stepback": step(-1, e.shiftKey); break;
    case "stepfwd": step(1, e.shiftKey); break;
    case "dir": setDir(Number(el.dataset.dir)); break;
    case "now": goLive(); break;
    case "pin": timePinned = !timePinned; syncTimeOpen(); break;
    case "rate": togglePop($("#ratePop"), el, buildLadder); break;
    case "pickdate": togglePop($("#pickPop"), el, fillPicker); break;
    case "picktime": togglePop($("#pickPop"), el, fillPicker); break;
    case "zone": T.zone = T.zone === "utc" ? "local" : "utc"; break;
    case "help": toggleSheet(); break;
    case "hide": overlays.classList.toggle("hidden"); hideTip(); break;
  }
  syncTimeOpen();
});
$$(".drive").forEach(b => b.addEventListener("click", e => { e.stopPropagation(); setKind(b.dataset.kind); }));
function hidePops() { $$(".pop").forEach(p => p.classList.remove("show")); }
function togglePop(pop, anchor, fill) {
  const open = pop.classList.contains("show");
  hidePops();
  if (open) return;
  fill();
  pop.classList.add("show");
  const pr = preview.getBoundingClientRect(), r = anchor.getBoundingClientRect();
  pop.style.left = clamp(r.left + r.width / 2 - pop.offsetWidth / 2 - pr.left, 6, pr.width - pop.offsetWidth - 6) + "px";
  pop.style.top = (r.top - pr.top - pop.offsetHeight - 8) + "px";
}
function buildLadder() {
  let s = `<span class="h">Backward</span><span class="h">Forward</span>`;
  for (const a of LADDER[T.kind]) {
    for (const sign of [-1, 1]) {
      const r = sign * a, on = T.playing && T.rate === r;
      s += `<button data-rate="${r}" class="${on ? "on" : ""}">${rateLabel(r)}${rateNote(r) ? `<small>${rateNote(r)}</small>` : ""}</button>`;
    }
  }
  const lad = $("#ladder");
  lad.innerHTML = s;
  $$("button", lad).forEach(b => b.addEventListener("click", e => { e.stopPropagation(); setRate(Number(b.dataset.rate)); hidePops(); syncTimeOpen(); }));
}
function fillPicker() {
  const p = parts(sim());
  $("#pickDate").value = `${p.y}-${pad2(p.mo + 1)}-${pad2(p.da)}`;
  $("#pickTime").value = `${pad2(p.h)}:${pad2(p.mi)}`;
}
function applyPicker() {
  const [y, mo, da] = $("#pickDate").value.split("-").map(Number);
  const [h, mi] = ($("#pickTime").value || "00:00").split(":").map(Number);
  if (!y) return;
  setTime(Date.UTC(y, mo - 1, da, h, mi) - zoneOff(sim()));
}
$("#pickDate").addEventListener("change", applyPicker);
$("#pickTime").addEventListener("change", applyPicker);

// ---------------------------------------------------------------- transport: skip, speed rulers, play
const ICON = {
  skipBack: `<svg width="14" height="12" viewBox="0 0 14 12"><path d="M1 1v10" stroke="currentColor" stroke-width="1.6"/><path d="M12 1 3.5 6l8.5 5z" fill="currentColor"/></svg>`,
  skipFwd: `<svg width="14" height="12" viewBox="0 0 14 12"><path d="M13 1v10" stroke="currentColor" stroke-width="1.6"/><path d="M2 1l8.5 5L2 11z" fill="currentColor"/></svg>`,
  left: `<svg width="9" height="9" viewBox="0 0 10 10"><path d="M8 1v8L2 5z" fill="currentColor"/></svg>`,
  right: `<svg width="9" height="9" viewBox="0 0 10 10"><path d="M2 1v8l6-4z" fill="currentColor"/></svg>`,
};
const skipBtn = d => `<button class="tbtn" data-act="${d < 0 ? "stepback" : "stepfwd"}" data-tip="${d < 0 ? "stepback" : "stepfwd"}">${d < 0 ? ICON.skipBack : ICON.skipFwd}</button>`;
const PLAY = `<button class="tbtn big" data-act="play" data-tip="play"></button>`;
function rulerHTML(side) {
  const lad = LADDER[T.kind];
  const order = side < 0 ? [...lad].reverse() : lad;
  const dots = order.map(a => {
    const i = lad.indexOf(a), px = 4 + i * (6 / Math.max(1, lad.length - 1));
    return `<button class="rdot" data-rate="${side * a}" title="${rateLabel(side * a)}"><i style="width:${px}px;height:${px}px"></i></button>`;
  }).join("");
  return `<div class="ruler" data-tip="ruler" data-side="${side}">${dots}</div>`;
}
let builtTransport = "";
function buildTransport() {
  if (T.kind === builtTransport) return;
  builtTransport = T.kind;
  $("#tCenter").innerHTML = `<div class="tbtns">${skipBtn(-1)}${rulerHTML(-1)}${PLAY}${rulerHTML(1)}${skipBtn(1)}</div>`;
}

// ---------------------------------------------------------------- the clock block beside the three rows
const CAL = `<svg width="12" height="12" viewBox="0 0 12 12"><rect x="1" y="2" width="10" height="9" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.1"/><path d="M1 5h10M4 1v2.5M8 1v2.5" stroke="currentColor" stroke-width="1.1"/></svg>`;
const SWAP = `<svg width="10" height="10" viewBox="0 0 10 10"><path d="M1 3h7M6 1l2 2-2 2M9 7H2M4 5 2 7l2 2" fill="none" stroke="currentColor" stroke-width="1.1"/></svg>`;
const BLOCK_LINES = {
  date: `<button class="cdate" data-act="pickdate" data-tip="pickdate">${CAL}<span data-role="dateyear"></span></button>`,
  clock: `<div class="ctime"><button class="cbig" data-act="picktime" data-tip="picktime"><span data-role="time"></span><small data-role="secs"></small></button><button class="zchip" data-act="zone" data-tip="zone"><span data-role="zone"></span>${SWAP}</button></div>`,
  live: `<div class="cnow"><button class="nowbtn" data-act="now" data-tip="now"><span class="dot"></span><span data-role="nowtext"></span></button><span class="offset" data-role="offset"></span></div>`,
};
$("#rBlock").innerHTML = ["live", "clock", "date"].map(k => BLOCK_LINES[k]).join("");
// dragging along a ruler picks the dot under the pointer
let rulerDrag = null;
$("#tCenter").addEventListener("pointerdown", e => {
  const r = e.target.closest(".ruler");
  if (!r) return;
  e.preventDefault();
  rulerDrag = r;
  r.setPointerCapture(e.pointerId);
  pickRuler(e);
});
$("#tCenter").addEventListener("pointermove", e => { if (rulerDrag) pickRuler(e); });
$("#tCenter").addEventListener("pointerup", () => { rulerDrag = null; });
function pickRuler(e) {
  let best = null, bd = 1e9;
  for (const d of $$(".rdot", rulerDrag)) {
    const r = d.getBoundingClientRect(), dist = Math.abs(e.clientX - (r.left + r.width / 2));
    if (dist < bd) { bd = dist; best = d; }
  }
  if (best && !(T.playing && T.rate === Number(best.dataset.rate))) setRate(Number(best.dataset.rate));
}

let tDrag = null;
function beginTimeDrag(e, el, kind) {
  e.preventDefault(); e.stopPropagation();
  hideTip(); hidePops();
  const wasPlaying = T.playing;
  const t = rebase();
  T.live = false; T.anchor = t; T.playing = false;
  tDrag = { el, kind, wasPlaying };
  el.setPointerCapture(e.pointerId);
  moveTimeDrag(e);
}
function endTimeDrag() {
  if (!tDrag) return;
  if (tDrag.wasPlaying) setPlaying(true);
  tDrag = null;
  syncTimeOpen();
}
function snapDay(y, idx) {
  for (const s of seasons(y)) {
    const p = parts(s.t);
    if (p.y !== y) continue;
    const sIdx = (Date.UTC(p.y, p.mo, p.da) - Date.UTC(y, 0, 1)) / DAY;
    if (Math.abs(idx - sIdx) <= 3) return sIdx;
  }
  return idx;
}
function setDayOfYear(y, idx) {
  const p = parts(sim());
  idx = clamp(Math.round(idx), 0, daysInYear(y) - 1);
  setTime(Date.UTC(y, 0, 1) + idx * DAY + p.msOfDay - p.off);
}
function moveTimeDrag(e) {
  if (!tDrag) return;
  const r = tDrag.el.getBoundingClientRect();
  const x = clamp((e.clientX - r.left) / r.width, 0, 0.99999);
  const t = sim(), p = parts(t);
  if (tDrag.kind === "day") setTime(dayStart(t) + Math.round(x * 24 * 60) * 60000);
  else setDayOfYear(p.y, snapDay(p.y, Math.floor(x * daysInYear(p.y))));
}
for (const [id, kind] of [["#aDay", "day"], ["#aYear", "year"]]) {
  const el = $(id);
  el.addEventListener("pointerdown", e => { if (e.target.classList.contains("season")) return; beginTimeDrag(e, el, kind); });
  el.addEventListener("pointermove", e => {
    moveTimeDrag(e);
    const r = el.getBoundingClientRect(), x = clamp((e.clientX - r.left) / r.width, 0, 0.99999);
    const hv = $(".hovertime", el);
    hv.style.display = "block";
    hv.style.left = (x * 100) + "%";
    const p = parts(sim());
    if (kind === "day") { const m = Math.round(x * 1440); hv.textContent = `${pad2(Math.floor(m / 60) % 24)}:${pad2(m % 60)}`; }
    else { const d = new Date(Date.UTC(p.y, 0, 1) + Math.floor(x * daysInYear(p.y)) * DAY); hv.textContent = `${d.getUTCDate()} ${MON[d.getUTCMonth()]}`; }
  });
  el.addEventListener("pointerleave", () => { $(".hovertime", el).style.display = "none"; });
  el.addEventListener("pointerup", endTimeDrag);
  el.addEventListener("pointercancel", endTimeDrag);
}

let builtYear = null;
function buildYear(y) {
  if (builtYear === y + T.zone) return;
  builtYear = y + T.zone;
  const diy = daysInYear(y);
  let ticks = "";
  for (let m = 0; m < 12; m++) {
    const x = (Date.UTC(y, m, 1) - Date.UTC(y, 0, 1)) / DAY / diy * 100;
    ticks += `<span style="left:${x}%"></span><b style="left:${x}%">${MON[m][0]}</b>`;
  }
  $("#aYearTicks").innerHTML = ticks;
  let sea = "";
  for (const s of seasons(y)) {
    const p = parts(s.t);
    const x = ((Date.UTC(p.y, p.mo, p.da) - Date.UTC(y, 0, 1)) / DAY + p.msOfDay / DAY) / diy * 100;
    sea += `<div class="season" style="left:${x}%" title="${s.name}: ${fmtDate(p)} ${fmtTime(p)} ${zoneName(T.zone, s.t)}" data-t="${s.t}"></div>`;
  }
  $("#aSeasons").innerHTML = sea;
  $$("#aSeasons .season").forEach(el => el.addEventListener("pointerdown", e => {
    e.stopPropagation(); e.preventDefault();
    const p = parts(Number(el.dataset.t));
    setDayOfYear(p.y, (Date.UTC(p.y, p.mo, p.da) - Date.UTC(p.y, 0, 1)) / DAY);
  }));
  let dt = "";
  for (let h = 0; h < 24; h += 3) dt += `<span style="left:${h / 24 * 100}%"></span>${h % 6 === 0 ? `<b style="left:${h / 24 * 100}%">${pad2(h)}</b>` : ""}`;
  $("#aDayTicks").innerHTML = dt;
}

let shadeKey = "";
function updateShading(t) {
  const ds = dayStart(t);
  const key = `${ds}|${Math.round(cam.lat)}|${Math.round(cam.lon)}|${T.zone}`;
  if (key === shadeKey) return;
  shadeKey = key;
  const p = subCam();
  const lin = [];
  for (let i = 0; i <= 48; i++) lin.push(`${altColor(altAt(p, ds + i * 30 * 60000))} ${(i / 48 * 100).toFixed(2)}%`);
  $("#aDay").style.background = `linear-gradient(90deg, ${lin.join(",")})`;
}

// ---------------------------------------------------------------- sidebar
const ROWS = [
  ["sg-pos", "lon", "Longitude", -180, 180, v => Math.round(v) + "°", "orbit"],
  ["sg-pos", "lat", "Latitude", -89, 89, v => Math.round(v) + "°", "orbit"],
  ["sg-pos", "zoom", "Zoom", 0, 1, v => zoomToDist(v).toFixed(1), "zoom"],
  ["sg-ori", "tilt", "Tilt", -180, 180, v => Math.round(v) + "°", "roll"],
  ["sg-ori", "yaw", "Yaw", -90, 90, v => Math.round(v) + "°", "look"],
  ["sg-ori", "pitch", "Pitch", -90, 90, v => Math.round(v) + "°", "look"],
  ["sg-frame", "fov", "Field of view", 10, 170, v => Math.round(v) + "°", "lens"],
  ["sg-frame", "ox", "Offset X", -3, 3, v => v.toFixed(2), "pan"],
  ["sg-frame", "oy", "Offset Y", -3, 3, v => v.toFixed(2), "pan"],
];
const rowEls = ROWS.map(([g, key, label, min, max, fmt, mirror]) => {
  const row = document.createElement("div");
  row.className = "srow";
  row.innerHTML = `<span>${label}</span><input type="range" min="${min}" max="${max}" step="${(max - min) / 1000}"><span class="v"></span>`;
  $("#" + g).appendChild(row);
  const input = $("input", row), v = $(".v", row);
  input.addEventListener("input", () => setCam(key, Number(input.value)));
  return { row, input, v, key, fmt, mirror };
});
PRESETS.forEach(([name, p]) => {
  const b = document.createElement("button");
  b.className = "btn"; b.textContent = name;
  b.addEventListener("click", () => tweenTo({ ...DEF, ...p }, 700));
  $("#presets").appendChild(b);
});

// ---------------------------------------------------------------- page toggles and cheat sheet
function seg(id, fn) {
  $$(`#${id} button`).forEach(b => b.addEventListener("click", () => {
    $$(`#${id} button`).forEach(x => x.classList.toggle("on", x === b));
    fn(b.dataset.v);
  }));
}
seg("segOs", v => { mac = v === "mac"; buildSheet(); });
preview.addEventListener("pointerenter", () => preview.classList.add("pointer-in"));
preview.addEventListener("pointerleave", () => preview.classList.remove("pointer-in"));

function buildSheet() {
  const mouse = [
    ["left", "Orbit the globe"], ["wheel", "Zoom"], ["middle", "Look around (yaw, pitch)"],
    ["both", "Roll the picture"], ["right", "Move the frame"],
  ];
  const kb = [
    [["←", "→", "↑", "↓"], "Orbit"], [["Shift", "←↑↓→"], "Look"], [["Mod", "←↑↓→"], "Move the frame"],
    [["+", "−"], "Zoom"], [["Q", "E"], "Roll"], [["N"], "North up"],
    [["Space"], "Play / pause"], [["J", "K", "L"], "Shuttle: back, pause, forward; again for faster"], [["R"], "Reverse direction"],
    [[",", "."], "Step back / forward (Shift: bigger)"], [["0"], "Back to now"], [["H"], "Hide controls"], [["Help"], "This sheet"],
  ];
  const m = mouse.map(([w, t, isNew, mod]) => `<div class="item"><span class="k">${mod ? `<kbd>${keyLabel("Mod")}</kbd>` : ""}${mouseSVG(w, 20)}</span><span>${t}${isNew ? '<span class="new">NEW</span>' : ""}</span></div>`).join("");
  const k = kb.map(([ks, t]) => `<div class="item"><span class="k">${kbd(ks)}</span><span>${platformText(t)}</span></div>`).join("");
  $("#sheetCols").innerHTML = `<div><h4>Mouse on the globe</h4>${m}</div><div><h4>Keyboard (all new)</h4>${k}</div>`;
  $("#sheetSub").textContent = mac ? "macOS" : "Windows and Linux";
  if (tipFor) showTip(tipFor);
}
buildSheet();
$("#sheet").addEventListener("click", e => { if (e.target.id === "sheet") $("#sheet").classList.remove("show"); });

// ---------------------------------------------------------------- per frame
const setHTML = (el, s) => { if (el.__s !== s) { el.innerHTML = s; el.__s = s; } };
const setText = (el, s) => { if (el.__s !== s) { el.textContent = s; el.__s = s; } };
const recent = k => active.has(k) || active.has(k + "-key") || now() - (lastUse[k] || 0) < 120;
function updateUI(t) {
  // camera panel
  placeNorth($("#aNorth"), 78);
  $("#aTicks .rot").setAttribute("transform", `rotate(${-cam.tilt} 39 39)`);
  placeOnSlider($("#aZoom .thumb"), $("#aZoom"), cam.zoom, true);
  $("#aLookReset").classList.toggle("show", Math.abs(cam.yaw) > .3 || Math.abs(cam.pitch) > .3);
  $("#aPanReset").classList.toggle("show", Math.abs(cam.ox) > .005 || Math.abs(cam.oy) > .005);
  for (const el of $$("[data-mirror]", camA)) el.classList.toggle("mirror-on", recent(el.dataset.mirror));
  camA.classList.toggle("idle", !(camHover || drag.active || now() - camUsed < 900));

  // sidebar
  for (const r of rowEls) {
    if (document.activeElement !== r.input) r.input.value = cam[r.key];
    setText(r.v, r.fmt(cam[r.key]));
    r.row.classList.toggle("hl", now() - (touched[r.mirror] || -1e9) < 600 && !r.row.matches(":hover"));
  }
  setText($("#sbMode"), T.live ? "Live" : T.playing ? `Playing ${rateLabel(T.rate)}` : "Fixed");
  setText($("#sbUtc"), new Date(t).toISOString().slice(0, 16).replace("T", " "));

  // time
  const p = parts(t), zn = zoneName(T.zone, t);
  buildYear(p.y);
  updateShading(t);
  const off = T.live ? "" : fmtOffset(t - Date.now());
  let state;
  if (T.live) state = `<span class="state"><span class="dot"></span>LIVE</span>`;
  else if (T.playing) state = `<span class="state" style="color:var(--accent)">${dirOf() > 0 ? "▶" : "◀"} ${rateLabel(Math.abs(T.rate))}</span>`;
  else state = `<span class="state" style="color:var(--sun)">${off}</span>`;
  setHTML($("#aPill"), `${state}<span>${fmtDate(p, !T.live)} · ${fmtTime(p, T.live)}</span><span class="zone">${zn}</span>`);
  buildTransport();
  const playBtn = $("#tCenter .tbtn.big");
  setHTML(playBtn, playIcon());
  playBtn.classList.toggle("on", T.playing);
  const speed = rateLabel(Math.abs(T.rate)), note = rateNote(T.rate);
  const side = $("#speedSide");
  side.classList.toggle("paused", !T.playing);
  setHTML(side, `<span class="v">${dirOf() < 0 ? ICON.left : ICON.right}${speed}</span><span class="n">${note || "same time each day"}</span>`);
  $$("#tCenter .rdot").forEach(d => {
    const r = Number(d.dataset.rate);
    d.classList.toggle("on", T.playing && T.rate === r);
    d.classList.toggle("lit", T.playing && Math.sign(r) === dirOf() && Math.abs(r) < Math.abs(T.rate));
  });
  $$(".drive").forEach(b => {
    b.classList.toggle("on", b.dataset.kind === T.kind);
    b.classList.toggle("running", b.dataset.kind === T.kind && T.playing);
  });
  const pin = $("#pinBtn");
  setHTML(pin, PIN(timePinned));
  pin.classList.toggle("on", timePinned);
  const bar = $("#aBar");
  const dateText = `${WD[p.wd]} ${p.da} ${MON[p.mo]}`;
  const roles = {
    time: fmtTime(p), secs: T.live ? ":" + pad2(p.s) : "", zone: zn,
    date: dateText, dateyear: `${dateText} ${p.y}`, year: String(p.y), offset: off,
    nowtext: T.live ? "Live" : "Now",
  };
  for (const el of $$("[data-role]", bar)) { const v = roles[el.dataset.role]; if (v !== undefined) setText(el, v); }
  $$(".nowbtn", bar).forEach(b => b.classList.toggle("islive", T.live));
  $("#aDay .handle").style.left = (p.msOfDay / DAY * 100) + "%";
  const doy = (Date.UTC(p.y, p.mo, p.da) - Date.UTC(p.y, 0, 1)) / DAY;
  $("#aYear .handle").style.left = ((doy + p.msOfDay / DAY) / daysInYear(p.y) * 100) + "%";
}

let lastFrame = now();
function frame() {
  const t1 = now(), dt = Math.min(0.1, (t1 - lastFrame) / 1000);
  lastFrame = t1;
  for (const f of ticking) f(dt);
  keyTick(dt);
  runTweens();
  const t = sim();
  render(t);
  updateUI(t);
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);
