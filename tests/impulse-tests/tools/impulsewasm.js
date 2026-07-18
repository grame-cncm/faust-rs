#!/usr/bin/env node
"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const { spawnSync } = require("child_process");

const SAMPLE_RATE = 44100;
const BLOCK_SIZE = 64;
const DEFAULT_FRAMES = 15000;
const DSP_PTR = 0;
const SOUND_CHAN = 2;
const SOUND_LENGTH = 4096;
const SOUND_SR = 44100;
const SOUND_BUFFER_SIZE = 1024;
const MAX_CHAN = 64;
const MAX_SOUNDFILE_PARTS = 256;

function usage() {
  console.error("usage: impulsewasm.js <file.dsp> [-n <frames>] [-I <dir>]... [-single|-double] [-vec [-vs <n>] [-lv <n>]] [-ss <n>]");
}

function parseArgs(argv) {
  let input = null;
  let frames = DEFAULT_FRAMES;
  let doublePrecision = false;
  const importDirs = [];
  // Compiler-mode flags forwarded verbatim to faust-rs.
  const compilerArgs = [];
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "-n") {
      i += 1;
      if (i >= argv.length) throw new Error("-n requires a frame count");
      frames = Number.parseInt(argv[i], 10);
      if (!Number.isFinite(frames) || frames < 0) throw new Error(`invalid frame count: ${argv[i]}`);
    } else if (arg === "-I") {
      i += 1;
      if (i >= argv.length) throw new Error("-I requires a directory");
      importDirs.push(argv[i]);
    } else if (arg === "-double") {
      doublePrecision = true;
    } else if (arg === "-single") {
      doublePrecision = false;
    } else if (arg === "-vec") {
      compilerArgs.push("-vec");
    } else if (arg === "-vs" || arg === "-lv" || arg === "-ss" || arg === "--scheduling-strategy") {
      i += 1;
      if (i >= argv.length) throw new Error(`${arg} requires a value`);
      if ((arg === "-ss" || arg === "--scheduling-strategy") && !/^\d+$/.test(argv[i])) {
        throw new Error(`invalid scheduling strategy: ${argv[i]}`);
      }
      compilerArgs.push(arg, argv[i]);
    } else if (arg.startsWith("-")) {
      throw new Error(`unknown option: ${arg}`);
    } else if (input === null) {
      input = arg;
    } else {
      throw new Error(`unexpected argument: ${arg}`);
    }
  }
  if (input === null) throw new Error("missing DSP input");
  return { input, frames, doublePrecision, importDirs, compilerArgs };
}

function normalize(value) {
  if (Math.abs(value) < 1e-6) return 0;
  return value;
}

function formatHeader(inputs, outputs, frames) {
  return [
    `number_of_inputs  : ${String(inputs).padStart(3, " ")}`,
    `number_of_outputs : ${String(outputs).padStart(3, " ")}`,
    `number_of_frames  : ${String(frames).padStart(6, " ")}`,
  ];
}

function formatFrame(index, values) {
  return `${String(index).padStart(6, " ")} : ${values.map((value) => ` ${Number(value).toFixed(6)}`).join(" ")}`;
}

function align(value, alignment) {
  return Math.ceil(value / alignment) * alignment;
}

function collectItems(ui, out = []) {
  if (!Array.isArray(ui)) return out;
  for (const item of ui) {
    if (item && Array.isArray(item.items)) {
      collectItems(item.items, out);
    } else if (item) {
      out.push(item);
    }
  }
  return out;
}

function soundfilePartCount(url) {
  const trimmed = String(url || "").trim();
  const open = trimmed.indexOf("{");
  if (open < 0) return trimmed.length === 0 ? 1 : 1;
  const close = trimmed.indexOf("}", open + 1);
  if (close < 0) return 1;
  const body = trimmed.slice(open + 1, close);
  const count = body
    .split(";")
    .filter((part) => part.trim().replace(/^'+|'+$/g, "").length > 0)
    .length;
  return Math.max(1, count);
}

function ensureMemory(memory, requiredBytes) {
  if (requiredBytes <= memory.buffer.byteLength) return;
  const needPages = Math.ceil((requiredBytes - memory.buffer.byteLength) / 65536);
  memory.grow(needPages);
}

function installSoundfile(memory, dspPtr, zoneIndex, numRealParts, cursor) {
  const realParts = Math.min(numRealParts, MAX_SOUNDFILE_PARTS);
  cursor = align(cursor, 8);
  const rawPtr = cursor;
  cursor += 16;
  const channelPtrsPtr = align(cursor, 4);
  cursor = channelPtrsPtr + MAX_CHAN * 4;
  const lengthsPtr = align(cursor, 4);
  cursor = lengthsPtr + MAX_SOUNDFILE_PARTS * 4;
  const sampleRatesPtr = align(cursor, 4);
  cursor = sampleRatesPtr + MAX_SOUNDFILE_PARTS * 4;
  const offsetsPtr = align(cursor, 4);
  cursor = offsetsPtr + MAX_SOUNDFILE_PARTS * 4;

  const partOffsets = [];
  let totalFrames = 0;
  for (let part = 0; part < realParts; part += 1) {
    partOffsets.push(totalFrames);
    totalFrames += SOUND_LENGTH;
  }
  for (let part = realParts; part < MAX_SOUNDFILE_PARTS; part += 1) {
    partOffsets.push(totalFrames);
    totalFrames += SOUND_BUFFER_SIZE;
  }

  const buffersPtr = align(cursor, 8);
  const channelBytes = totalFrames * 8;
  cursor = buffersPtr + SOUND_CHAN * channelBytes;
  ensureMemory(memory, cursor);

  const data = new DataView(memory.buffer);
  data.setUint32(dspPtr + zoneIndex, rawPtr, true);
  data.setUint32(rawPtr, channelPtrsPtr, true);
  data.setUint32(rawPtr + 4, lengthsPtr, true);
  data.setUint32(rawPtr + 8, sampleRatesPtr, true);
  data.setUint32(rawPtr + 12, offsetsPtr, true);

  for (let channel = 0; channel < MAX_CHAN; channel += 1) {
    data.setUint32(channelPtrsPtr + channel * 4, buffersPtr + (channel % SOUND_CHAN) * channelBytes, true);
  }
  for (let part = 0; part < MAX_SOUNDFILE_PARTS; part += 1) {
    data.setInt32(lengthsPtr + part * 4, part < realParts ? SOUND_LENGTH : SOUND_BUFFER_SIZE, true);
    data.setInt32(sampleRatesPtr + part * 4, SOUND_SR, true);
    data.setInt32(offsetsPtr + part * 4, partOffsets[part], true);
  }
  for (let part = 0; part < realParts; part += 1) {
    const partOffset = partOffsets[part];
    for (let sample = 0; sample < SOUND_LENGTH; sample += 1) {
      const value = Math.sin(part + (2.0 * Math.PI * sample) / SOUND_LENGTH);
      for (let channel = 0; channel < SOUND_CHAN; channel += 1) {
        data.setFloat64(buffersPtr + channel * channelBytes + (partOffset + sample) * 8, value, true);
      }
    }
  }
  return cursor;
}

function mathImports() {
  const fmod = (a, b) => a % b;
  const remainder = (a, b) => a - Math.round(a / b) * b;
  const copysign = (x, y) => Math.abs(x) * (Object.is(y, -0) || y < 0 ? -1 : 1);
  return {
    _sinf: Math.sin,
    _cosf: Math.cos,
    _expf: Math.exp,
    _exp10f: (x) => 10 ** x,
    _logf: Math.log,
    _log10f: Math.log10,
    _tanf: Math.tan,
    _atanf: Math.atan,
    _asinf: Math.asin,
    _acosf: Math.acos,
    _roundf: Math.round,
    _powf: Math.pow,
    _atan2f: Math.atan2,
    _fmodf: fmod,
    _remainderf: remainder,
    _isnanf: (x) => Number.isNaN(x) ? 1 : 0,
    _isinff: (x) => !Number.isFinite(x) ? 1 : 0,
    _copysignf: copysign,
    _sin: Math.sin,
    _cos: Math.cos,
    _exp: Math.exp,
    _exp10: (x) => 10 ** x,
    _log: Math.log,
    _log10: Math.log10,
    _tan: Math.tan,
    _atan: Math.atan,
    _asin: Math.asin,
    _acos: Math.acos,
    _round: Math.round,
    _pow: Math.pow,
    _atan2: Math.atan2,
    _fmod: fmod,
    _remainder: remainder,
    _isnan: (x) => Number.isNaN(x) ? 1 : 0,
    _isinf: (x) => !Number.isFinite(x) ? 1 : 0,
    _copysign: copysign,
    _acosh: Math.acosh,
    _asinh: Math.asinh,
    _atanh: Math.atanh,
    _cosh: Math.cosh,
    _sinh: Math.sinh,
    _tanh: Math.tanh,
  };
}

function compileWasm(faustRs, input, importDirs, doublePrecision, compilerArgs, tmpDir) {
  const wasmPath = path.join(tmpDir, `${path.basename(input, ".dsp")}.wasm`);
  const timeout = process.env.FAUST_RS_TIMEOUT_SECONDS || "600";
  if (!/^[1-9]\d*$/.test(timeout)) throw new Error(`invalid FAUST_RS_TIMEOUT_SECONDS: ${timeout}`);
  const args = ["-lang", "wasm", doublePrecision ? "-double" : "-single", "--timeout", timeout, input, "-o", wasmPath];
  args.push(...compilerArgs);
  for (const dir of importDirs) args.push("-I", dir);
  const result = spawnSync(faustRs, args, { encoding: "utf8" });
  if (result.status !== 0) {
    const detail = (result.stderr || result.stdout || "").trim();
    throw new Error(detail || `failed to compile ${input} to WASM`);
  }
  return { wasmPath, jsonPath: wasmPath.replace(/\.wasm$/i, ".json") };
}

async function run() {
  const { input, frames, doublePrecision, importDirs, compilerArgs } = parseArgs(process.argv.slice(2));
  const faustRs = process.env.FAUST_RS || path.join("..", "..", "target", "release", "faust-rs");
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "faust-rs-impulse-wasm-"));
  try {
    const { wasmPath, jsonPath } = compileWasm(faustRs, input, importDirs, doublePrecision, compilerArgs, tmpDir);
    const json = JSON.parse(fs.readFileSync(jsonPath, "utf8"));
    const wasmBytes = fs.readFileSync(wasmPath);
    const module = await WebAssembly.compile(wasmBytes);
    const importObject = { env: mathImports() };
    if (WebAssembly.Module.imports(module).some((entry) => entry.module === "env" && entry.name === "memory")) {
      importObject.env.memory = new WebAssembly.Memory({ initial: 32 });
    }
    const instance = await WebAssembly.instantiate(module, importObject);
    const exp = instance.exports;
    const memory = exp.memory || importObject.env.memory;
    if (!memory) throw new Error("WASM module has no exported or imported memory");

    const sampleBytes = doublePrecision ? 8 : 4;
    const inputs = exp.getNumInputs(DSP_PTR);
    const outputs = exp.getNumOutputs(DSP_PTR);
    const jsonSize = Number(json.size || 0);
    const pointerTableBytes = (inputs + outputs) * 4;
    const pointersPtr = align(jsonSize, 4);
    const inputsPtr = pointersPtr;
    const outputsPtr = inputsPtr + inputs * 4;
    let audioPtr = align(pointersPtr + pointerTableBytes, sampleBytes);
    const channelBytes = BLOCK_SIZE * sampleBytes;
    const channelPtrs = [];
    for (let channel = 0; channel < inputs + outputs; channel += 1) {
      channelPtrs.push(audioPtr);
      audioPtr += channelBytes;
    }
    ensureMemory(memory, audioPtr);

    let u32 = new Uint32Array(memory.buffer);
    for (let i = 0; i < inputs; i += 1) u32[(inputsPtr >> 2) + i] = channelPtrs[i];
    for (let i = 0; i < outputs; i += 1) u32[(outputsPtr >> 2) + i] = channelPtrs[inputs + i];

    exp.init(DSP_PTR, SAMPLE_RATE);
    const uiItems = collectItems(json.ui);
    let cursor = audioPtr;
    for (const item of uiItems) {
      if (item.type === "soundfile" && Number.isInteger(item.index)) {
        cursor = installSoundfile(memory, DSP_PTR, item.index, soundfilePartCount(item.url), cursor);
      }
    }
    u32 = new Uint32Array(memory.buffer);
    for (let i = 0; i < inputs; i += 1) u32[(inputsPtr >> 2) + i] = channelPtrs[i];
    for (let i = 0; i < outputs; i += 1) u32[(outputsPtr >> 2) + i] = channelPtrs[inputs + i];

    const buttonIndices = uiItems
      .filter((item) => item.type === "button" && Number.isInteger(item.index))
      .map((item) => item.index);

    let written = 0;
    let cycle = 0;
    const lines = formatHeader(inputs, outputs, frames);
    while (written < frames) {
      const n = Math.min(BLOCK_SIZE, frames - written);
      const view = doublePrecision
        ? new Float64Array(memory.buffer)
        : new Float32Array(memory.buffer);
      for (const ptr of channelPtrs) {
        view.fill(0, ptr / sampleBytes, ptr / sampleBytes + BLOCK_SIZE);
      }
      if (written === 0) {
        for (let channel = 0; channel < inputs; channel += 1) {
          view[channelPtrs[channel] / sampleBytes] = 1.0;
        }
      }
      const buttonValue = cycle === 0 ? 1.0 : 0.0;
      for (const index of buttonIndices) {
        exp.setParamValue(DSP_PTR, index, buttonValue);
      }
      exp.compute(DSP_PTR, n, inputsPtr, outputsPtr);
      for (let frame = 0; frame < n; frame += 1) {
        const values = [];
        for (let channel = 0; channel < outputs; channel += 1) {
          const ptr = channelPtrs[inputs + channel] / sampleBytes;
          values.push(normalize(Number(view[ptr + frame])));
        }
        lines.push(formatFrame(written, values));
        written += 1;
      }
      cycle += 1;
    }
    process.stdout.write(lines.join("\n"));
    if (lines.length > 0) process.stdout.write("\n");
  } finally {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
}

run().catch((error) => {
  console.error(error && error.stack ? error.stack : String(error));
  process.exit(1);
});
