/**
 * Local Bambu LAN queue. Credentials stay in .data/bambu.json (gitignored).
 * The printer is one job at a time; this process holds the queue.
 */
import { execFile } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import tls from "node:tls";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

const execFileAsync = promisify(execFile);

const here = path.dirname(fileURLToPath(import.meta.url));
const dataDir = path.join(here, ".data");
const confFile = path.join(dataDir, "bambu.json");
const printFileDefault = path.join(dataDir, "mini-hull.gcode.3mf");

/** @type {null | { connect: Function }} */
let mqttLib = null;
/** @type {null | { Client: Function }} */
let ftpLib = null;
try {
  mqttLib = await import("mqtt");
  ftpLib = await import("basic-ftp");
} catch {
  mqttLib = null;
  ftpLib = null;
}

function loadConf() {
  try {
    return JSON.parse(fs.readFileSync(confFile, "utf8"));
  } catch {
    return {};
  }
}

function saveConf(patch) {
  fs.mkdirSync(dataDir, { recursive: true, mode: 0o700 });
  const next = { ...loadConf(), ...patch };
  fs.writeFileSync(confFile, `${JSON.stringify(next, null, 2)}\n`, { mode: 0o600 });
  return next;
}

function printFile() {
  const conf = loadConf();
  const p = process.env.BAMBU_FILE || conf.file || printFileDefault;
  const abs = path.isAbsolute(p) ? p : path.join(here, p);
  if (fs.existsSync(abs)) return abs;
  if (fs.existsSync(printFileDefault)) return printFileDefault;
  return abs;
}

let lastReport = null;
let lastError = "";
let busy = false;
let seq = 1;
/** @type {Buffer | null} */
let lastJpeg = null;
let lastJpegAt = 0;
let cameraBusy = false;
/** @type {null | import("mqtt").MqttClient} */
let mqttClient = null;
let mqttHost = "";
let mqttSerial = "";

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function printBlock(report = lastReport) {
  return report?.print ?? report ?? {};
}

function gcodeState(report = lastReport) {
  return String(printBlock(report).gcode_state ?? "").toUpperCase();
}

function startedState(state) {
  const s = String(state || "").toUpperCase();
  return s === "PREPARE" || s === "RUNNING" || s === "PAUSE" || s === "SLICING";
}

function commandFail(report = lastReport) {
  const p = printBlock(report);
  const cmd = String(p.command ?? "");
  const result = String(p.result ?? "").toLowerCase();
  if ((cmd === "project_file" || cmd === "gcode_file") && (result === "fail" || result === "failed")) {
    return String(p.reason || p.fail_reason || p.err_code || "printer refused the job");
  }
  return null;
}

function printStats(report) {
  const p = report?.print ?? report ?? {};
  return {
    nozzle: typeof p.nozzle_temper === "number" ? Math.round(p.nozzle_temper) : null,
    bed: typeof p.bed_temper === "number" ? Math.round(p.bed_temper) : null,
    percent: typeof p.mc_percent === "number" ? p.mc_percent : null,
    layer: typeof p.layer_num === "number" ? p.layer_num : null,
    layers: typeof p.total_layer_num === "number" ? p.total_layer_num : null,
  };
}

export function printerView() {
  const conf = loadConf();
  const file = printFile();
  const state = lastReport?.print?.gcode_state ?? lastReport?.gcode_state ?? null;
  const stats = printStats(lastReport);
  return {
    configured: Boolean(conf.serial && conf.access_code),
    host: conf.host || null,
    serial_tail: conf.serial ? conf.serial.slice(-4) : null,
    model: conf.model || "Bambu Lab A1",
    file_ready: fs.existsSync(file),
    deps: Boolean(mqttLib && ftpLib),
    online: Boolean(conf.host && lastReport),
    gcode_state: state,
    camera: Boolean(lastJpeg),
    print_command: printBlock().command ?? null,
    print_result: printBlock().result ?? null,
    print_reason: printBlock().reason || printBlock().fail_reason || null,
    nozzle: stats.nozzle,
    bed: stats.bed,
    percent: stats.percent,
    layer: stats.layer,
    layers: stats.layers,
    error: lastError || null,
    auto_start: (process.env.BAMBU_AUTO_START ?? "true") === "true",
  };
}

function cameraAuth(conf) {
  const buf = Buffer.alloc(80);
  buf.writeUInt32LE(0x40, 0);
  buf.writeUInt32LE(0x3000, 4);
  buf.write("bblp", 16, "ascii");
  buf.write(String(conf.access_code), 48, "ascii");
  return buf;
}

/**
 * One JPEG from the A1 chamber camera on TLS :6000. Cached ~1.5s.
 * @returns {Promise<Buffer | null>}
 */
export async function cameraJpeg() {
  if (lastJpeg && Date.now() - lastJpegAt < 1500) return lastJpeg;
  if (cameraBusy) return lastJpeg;
  const conf = loadConf();
  if (!conf.host || !conf.access_code) return lastJpeg;
  cameraBusy = true;
  try {
    const jpeg = await new Promise((resolve) => {
      const sock = tls.connect(
        { host: conf.host, port: 6000, rejectUnauthorized: false, timeout: 8_000 },
        () => sock.write(cameraAuth(conf)),
      );
      let buf = Buffer.alloc(0);
      let payloadSize = 0;
      let headerDone = false;
      const finish = (img) => {
        clearTimeout(t);
        sock.destroy();
        resolve(img);
      };
      const t = setTimeout(() => finish(lastJpeg), 8_000);
      sock.on("data", (d) => {
        buf = Buffer.concat([buf, d]);
        if (!headerDone) {
          if (buf.length < 16) return;
          payloadSize = buf.readUInt32LE(0);
          buf = buf.subarray(16);
          headerDone = true;
          if (payloadSize < 128 || payloadSize > 2_000_000) {
            finish(lastJpeg);
            return;
          }
        }
        if (headerDone && buf.length >= payloadSize) {
          const img = buf.subarray(0, payloadSize);
          if (img[0] === 0xff && img[1] === 0xd8) finish(img);
          else finish(lastJpeg);
        }
      });
      sock.on("error", () => finish(lastJpeg));
      sock.on("timeout", () => finish(lastJpeg));
    });
    if (jpeg) {
      lastJpeg = jpeg;
      lastJpegAt = Date.now();
    }
    return jpeg ?? lastJpeg;
  } finally {
    cameraBusy = false;
  }
}

function idleEnough(state) {
  if (!state) return true;
  const s = String(state).toUpperCase();
  return s === "IDLE" || s === "FINISH" || s === "FAILED" || s === "UNKNOWN";
}

function probeMqtt(ip, ms = 500) {
  return new Promise((resolve) => {
    const s = net.connect({ host: ip, port: 8883, timeout: ms }, () => {
      s.destroy();
      resolve(true);
    });
    const fail = () => {
      s.destroy();
      resolve(false);
    };
    s.on("error", fail);
    s.on("timeout", fail);
  });
}

function wlanHosts() {
  const skip = new Set();
  const hosts = [];
  for (const addrs of Object.values(os.networkInterfaces())) {
    for (const a of addrs ?? []) {
      const v4 = a.family === "IPv4" || a.family === 4;
      if (!v4 || a.internal) continue;
      skip.add(a.address);
      const parts = a.address.split(".");
      if (parts.length !== 4) continue;
      const prefix = `${parts[0]}.${parts[1]}.${parts[2]}`;
      for (let i = 1; i < 255; i++) hosts.push(`${prefix}.${i}`);
    }
  }
  return hosts.filter((ip) => !skip.has(ip));
}

async function scanLan() {
  const hosts = wlanHosts();
  const concurrency = 32;
  for (let i = 0; i < hosts.length; i += concurrency) {
    const batch = hosts.slice(i, i + concurrency);
    const found = await Promise.all(batch.map(async (ip) => ((await probeMqtt(ip)) ? ip : null)));
    const hit = found.find(Boolean);
    if (hit) return hit;
  }
  return null;
}

async function scanHost() {
  if (process.env.BAMBU_HOST) {
    const host = process.env.BAMBU_HOST.trim();
    saveConf({ host });
    return host;
  }
  const conf = loadConf();
  if (conf.host && (await probeMqtt(conf.host, 1200))) return conf.host;
  const hit = await scanLan();
  if (hit) saveConf({ host: hit });
  else if (conf.host) saveConf({ host: "" });
  return hit;
}

function clientOpts(conf) {
  return {
    username: "bblp",
    password: String(conf.access_code),
    rejectUnauthorized: false,
    connectTimeout: 8_000,
    reconnectPeriod: 4_000,
    clientId: `harborline-${process.pid}`,
    protocolVersion: 4,
    clean: true,
  };
}

/** @type {null | { sequence_id?: string, result?: string, reason?: string, command?: string }} */
let lastAck = null;

function ingestReport(buf) {
  try {
    const msg = JSON.parse(buf.toString("utf8"));
    if (msg.print && typeof msg.print === "object") {
      const p = msg.print;
      if (p.result && p.msg == null && !p.gcode_state) {
        lastAck = p;
        return;
      }
      const prev = lastReport?.print ?? {};
      const next = { ...prev, ...p };
      if (p.command === "push_status" && p.result == null) {
        delete next.result;
        delete next.reason;
        delete next.fail_reason;
      }
      lastReport = { ...lastReport, print: next };
    } else if (msg.gcode_state) {
      lastReport = { ...lastReport, ...msg };
    }
  } catch {
    // ignore junk
  }
}

async function closeMqtt() {
  const client = mqttClient;
  mqttClient = null;
  mqttHost = "";
  mqttSerial = "";
  if (!client) return;
  try {
    await Promise.race([client.endAsync(), sleep(2_000)]);
  } catch {
    try {
      client.destroy();
    } catch {
      // already gone
    }
  }
}

async function ensureMqtt(conf) {
  if (!mqttLib) throw new Error("mqtt package missing");
  if (mqttClient?.connected && mqttHost === conf.host && mqttSerial === conf.serial) return mqttClient;
  await closeMqtt();
  const client = await mqttLib.connectAsync(`mqtts://${conf.host}:8883`, clientOpts(conf));
  mqttClient = client;
  mqttHost = conf.host;
  mqttSerial = conf.serial;
  client.on("message", (_topic, buf) => ingestReport(buf));
  client.on("close", () => {
    if (mqttClient === client) {
      mqttClient = null;
      mqttHost = "";
      mqttSerial = "";
    }
  });
  await client.subscribeAsync(`device/${conf.serial}/report`);
  return client;
}

function clearCommandState() {
  lastAck = null;
  if (lastReport?.print) {
    const p = { ...lastReport.print };
    delete p.result;
    delete p.reason;
    delete p.fail_reason;
    lastReport = { ...lastReport, print: p };
  }
}

async function waitForStart(ms, sequenceId) {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (sequenceId && lastAck && String(lastAck.sequence_id) === String(sequenceId)) {
      const result = String(lastAck.result || "").toLowerCase();
      if (result === "fail" || result === "failed") {
        return { ok: false, reason: String(lastAck.reason || lastAck.fail_reason || "printer refused the job") };
      }
    }
    const fail = commandFail();
    if (fail) return { ok: false, reason: fail };
    if (startedState(gcodeState())) return { ok: true, state: gcodeState() };
    await sleep(250);
  }
  return { ok: false, reason: `no start ack (state ${gcodeState() || "unknown"})` };
}

async function releaseStudio() {
  try {
    await execFileAsync("osascript", ["-e", 'tell application "BambuStudio" to quit'], { timeout: 8_000 });
    await sleep(2_500);
  } catch {
    // not running is fine
  }
}

async function publishProject(conf, remote, orderId, extra) {
  const client = await ensureMqtt(conf);
  clearCommandState();
  const sequence_id = String(seq++);
  const payload = {
    print: {
      sequence_id,
      command: "project_file",
      param: "Metadata/plate_1.gcode",
      url: extra.url,
      file: remote,
      md5: "",
      bed_type: "auto",
      timelapse: false,
      bed_leveling: true,
      bed_levelling: true,
      flow_cali: false,
      vibration_cali: false,
      layer_inspect: false,
      use_ams: extra.use_ams,
      ams_mapping: extra.ams_mapping,
      subtask_name: remote,
      profile_id: "0",
      project_id: "0",
      subtask_id: "0",
      task_id: "0",
    },
  };
  // qos 0: A1 LAN often never returns a PUBACK; qos 1 hangs until our timeout.
  client.publish(`device/${conf.serial}/request`, JSON.stringify(payload), { qos: 0 });
  return waitForStart(20_000, sequence_id);
}

async function uploadAndStart(orderId) {
  const conf = loadConf();
  if (!conf.host || !conf.serial || !conf.access_code) throw new Error("printer not configured");
  if (!ftpLib) throw new Error("basic-ftp package missing");
  const src = printFile();
  if (!fs.existsSync(src)) throw new Error("print file missing; put a sliced .gcode.3mf in .data/mini-hull.gcode.3mf");
  const remote = `harborline-${orderId}.gcode.3mf`;
  const ftp = new ftpLib.Client(12_000);
  ftp.ftp.verbose = false;
  try {
    const upload = (async () => {
      await ftp.access({
        host: conf.host,
        port: 990,
        user: "bblp",
        password: String(conf.access_code),
        secure: "implicit",
        secureOptions: { rejectUnauthorized: false },
      });
      await ftp.uploadFrom(src, remote);
    })();
    await Promise.race([
      upload,
      sleep(18_000).then(() => {
        throw new Error("ftps upload timeout");
      }),
    ]);
  } catch (e) {
    lastError = e instanceof Error ? e.message : "ftps failed";
    // A previous send may already have left the file in /cache.
  } finally {
    try {
      ftp.close();
    } catch {
      // ignore
    }
  }
  await ensureMqtt(conf);
  await snapshot();
  const attempts = [
    { url: `ftp:///${remote}`, use_ams: true, ams_mapping: [0, -1, -1, -1, -1] },
    { url: `ftp:///${remote}`, use_ams: false, ams_mapping: "" },
    { url: "ftp:///model/SpeedBoatRace_Bambu Pla Basic.gcode.3mf", use_ams: true, ams_mapping: [0, -1, -1, -1, -1] },
  ];
  let last = { ok: false, reason: "not sent" };
  for (const extra of attempts) {
    last = await publishProject(conf, extra.url.includes("/model/") ? "SpeedBoatRace_Bambu Pla Basic.gcode.3mf" : remote, orderId, extra);
    if (last.ok) return last;
  }
  throw new Error(last.reason || "printer did not start");
}

/** Stop the job on the bed. No-op if the printer is already idle. */
export async function stopPrint() {
  const conf = loadConf();
  if (!conf.host || !conf.serial || !conf.access_code) throw new Error("printer not configured");
  const client = await ensureMqtt(conf);
  await snapshot();
  const before = gcodeState();
  if (idleEnough(before) && before) return { ok: true, state: before, already_idle: true };
  clearCommandState();
  const sequence_id = String(seq++);
  await Promise.race([
    client.publishAsync(
      `device/${conf.serial}/request`,
      JSON.stringify({ print: { sequence_id, command: "stop", param: "" } }),
      { qos: 1 },
    ),
    sleep(5_000).then(() => {
      throw new Error("mqtt stop timeout");
    }),
  ]);
  const deadline = Date.now() + 15_000;
  while (Date.now() < deadline) {
    const state = gcodeState();
    if (idleEnough(state) || state === "PAUSE" || state === "STOPPED") return { ok: true, state };
    await sleep(250);
  }
  return { ok: false, state: gcodeState(), reason: "printer did not stop" };
}

async function snapshot() {
  const conf = loadConf();
  if (!conf.host || !conf.serial || !conf.access_code || !mqttLib) return null;
  const client = await ensureMqtt(conf);
  client.publish(
    `device/${conf.serial}/request`,
    JSON.stringify({ pushing: { sequence_id: String(seq++), command: "pushall" } }),
    { qos: 0 },
  );
  const deadline = Date.now() + 4_000;
  while (Date.now() < deadline) {
    if (printBlock().gcode_state) return lastReport;
    await sleep(150);
  }
  return lastReport;
}

/**
 * @param {object} order
 * @param {(patch: object) => object | null} upsert
 */
export function enqueueIfPaid(order, upsert) {
  if (!order || order.status !== "succeeded") return order;
  if (order.print?.status) return order;
  const next = upsert({
    merchant_order_id: order.merchant_order_id,
    print: {
      status: "queued",
      file: path.basename(printFile()),
      queued_at: new Date().toISOString(),
    },
  });
  return next;
}

/**
 * @param {() => object[]} listOrders
 * @param {(patch: object) => object | null} upsert
 */
export function startWorker(listOrders, upsert) {
  const tick = async () => {
    if (busy) return;
    busy = true;
    try {
      const conf = loadConf();
      if (!conf.serial || !conf.access_code) {
        lastError = "no printer credentials";
        return;
      }
      if (!mqttLib || !ftpLib) {
        lastError = "npm install mqtt basic-ftp in examples/merchant-demo";
        return;
      }
      const host = await scanHost();
      if (!host) {
        lastError = "printer not on LAN (need MQTT :8883)";
        lastReport = null;
        return;
      }
      try {
        await snapshot();
        lastError = "";
        void cameraJpeg();
      } catch (e) {
        lastError = e instanceof Error ? e.message : "mqtt failed";
        lastReport = null;
        return;
      }
      const state = gcodeState();
      const auto = (process.env.BAMBU_AUTO_START ?? "true") === "true";
      if (!auto) return;
      if (idleEnough(state)) {
        for (const o of listOrders()) {
          if (o.status !== "succeeded") continue;
          const stuck = o.print?.status === "sending" || (o.print?.status === "printing" && !o.print?.accepted_at);
          if (!stuck) continue;
          upsert({
            merchant_order_id: o.merchant_order_id,
            print: { ...o.print, status: "queued", error: "printer stayed idle; retrying" },
          });
        }
      }
      if (!idleEnough(state)) return;
      const queued = listOrders().filter((o) => o.status === "succeeded" && o.print?.status === "queued");
      queued.sort((a, b) => String(a.print?.queued_at ?? "").localeCompare(String(b.print?.queued_at ?? "")));
      const job = queued[0];
      if (!job) return;
      upsert({
        merchant_order_id: job.merchant_order_id,
        print: { ...job.print, status: "sending", started_at: new Date().toISOString(), error: null },
      });
      try {
        const started = await uploadAndStart(job.merchant_order_id);
        upsert({
          merchant_order_id: job.merchant_order_id,
          print: {
            ...job.print,
            status: "printing",
            started_at: new Date().toISOString(),
            accepted_at: new Date().toISOString(),
            gcode_state: started.state,
            error: null,
          },
        });
        lastError = "";
      } catch (e) {
        upsert({
          merchant_order_id: job.merchant_order_id,
          print: {
            ...job.print,
            status: "error",
            error: e instanceof Error ? e.message : "send failed",
          },
        });
        lastError = e instanceof Error ? e.message : "send failed";
      }
    } finally {
      busy = false;
    }
  };
  void tick();
  return setInterval(() => void tick(), 8_000);
}
