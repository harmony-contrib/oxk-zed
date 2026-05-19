#!/usr/bin/env node
"use strict";

const { fork } = require("node:child_process");

const serverPath = process.env.ARKTS_LANGUAGE_SERVER_PATH;
if (!serverPath) {
  console.error("ARKTS_LANGUAGE_SERVER_PATH is not set");
  process.exit(1);
}

const child = fork(serverPath, ["--node-ipc"], {
  env: process.env,
  stdio: ["pipe", "pipe", "pipe", "ipc"],
});

let input = Buffer.alloc(0);
let initializationOptions = null;
let nextInternalRequestId = -1;
const internalRequestIds = new Set();

child.stderr.on("data", (chunk) => process.stderr.write(chunk));

// Keep stdout reserved for the client-facing JSON-RPC stream.
child.stdout.on("data", (chunk) => process.stderr.write(chunk));

child.on("message", (message) => {
  if (message && internalRequestIds.delete(message.id)) {
    return;
  }
  writeMessage(message);
});

child.on("exit", (code, signal) => {
  if (signal) {
    process.exit(1);
  }
  process.exit(code ?? 0);
});

process.stdin.on("data", (chunk) => {
  input = Buffer.concat([input, chunk]);
  while (readMessage()) {}
});

process.stdin.on("end", () => {
  child.kill();
});

function readMessage() {
  const headerEnd = input.indexOf("\r\n\r\n");
  if (headerEnd === -1) {
    return false;
  }

  const header = input.subarray(0, headerEnd).toString("ascii");
  const match = /Content-Length:\s*(\d+)/i.exec(header);
  if (!match) {
    throw new Error(`Missing Content-Length header: ${header}`);
  }

  const length = Number(match[1]);
  const messageStart = headerEnd + 4;
  const messageEnd = messageStart + length;
  if (input.length < messageEnd) {
    return false;
  }

  const payload = input.subarray(messageStart, messageEnd).toString("utf8");
  input = input.subarray(messageEnd);

  handleMessage(JSON.parse(payload));
  return true;
}

function handleMessage(message) {
  if (message.method === "initialize") {
    initializationOptions = message.params?.initializationOptions ?? null;
    child.send(message);
    sendArktsConfiguration(initializationOptions);
    return;
  }

  if (message.method === "workspace/didChangeConfiguration") {
    const settings = message.params?.settings ?? null;
    child.send(message);
    sendArktsConfiguration(settings);
    return;
  }

  child.send(message);
}

function sendArktsConfiguration(options) {
  if (!options || typeof options !== "object") {
    return;
  }

  const id = nextInternalRequestId--;
  internalRequestIds.add(id);
  child.send({
    jsonrpc: "2.0",
    id,
    method: "ets/waitForEtsConfigurationChangedRequested",
    params: options,
  });
}

function writeMessage(message) {
  const payload = Buffer.from(JSON.stringify(message), "utf8");
  process.stdout.write(`Content-Length: ${payload.length}\r\n\r\n`);
  process.stdout.write(payload);
}
