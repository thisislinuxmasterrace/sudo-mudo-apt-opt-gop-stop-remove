#!/usr/bin/env node
// launcher: runs the native binary shipped by the matching platform package. build.mjs copies this
// script to bin/sudo-mudo-apt-opt-gop-stop-remove.js of the launcher package.
"use strict";

const { spawn } = require("node:child_process");
const os = require("node:os");
const path = require("node:path");

const NAME = "sudo-mudo-apt-opt-gop-stop-remove";
const SCOPE = "@archhaze24";

// candidate packages per platform, in order of preference. the musl build is fully static, so on
// linux (x64 and arm64) it is also a valid fallback when the glibc package was not installed.
const PLATFORMS = {
  "linux-x64": [`${SCOPE}/${NAME}-linux-x64`, `${SCOPE}/${NAME}-linux-x64-musl`],
  "linux-arm64": [`${SCOPE}/${NAME}-linux-arm64`, `${SCOPE}/${NAME}-linux-arm64-musl`],
  "darwin-x64": [`${SCOPE}/${NAME}-darwin-x64`],
  "darwin-arm64": [`${SCOPE}/${NAME}-darwin-arm64`],
  "win32-x64": [`${SCOPE}/${NAME}-win32-x64`],
};

// true on a musl-based linux (alpine etc.): glibc reports its version, musl does not.
function isMusl() {
  if (process.platform !== "linux") return false;
  try {
    const report = process.report.getReport();
    const header = typeof report === "string" ? JSON.parse(report).header : report.header;
    if (header && "glibcVersionRuntime" in header) return !header.glibcVersionRuntime;
  } catch {
    // fall through to the file check
  }
  try {
    return require("node:fs").readdirSync("/lib").some((name) => name.startsWith("ld-musl-"));
  } catch {
    return false;
  }
}

function binaryPath() {
  const key = `${process.platform}-${process.arch}`;
  const candidates = PLATFORMS[key];
  if (!candidates) {
    throw new Error(
      `there is no prebuilt binary for ${key}. supported: ${Object.keys(PLATFORMS).join(", ")}. ` +
        `build from source instead: cargo install ${NAME}`,
    );
  }
  const ordered = key.startsWith("linux-") && isMusl() ? [...candidates].reverse() : candidates;
  const exe = process.platform === "win32" ? `${NAME}.exe` : NAME;
  for (const pkg of ordered) {
    try {
      const manifest = require.resolve(`${pkg}/package.json`);
      return path.join(path.dirname(manifest), "bin", exe);
    } catch {
      // not installed, try the next candidate
    }
  }
  throw new Error(
    `the platform package ${ordered[0]} is not installed (npm skipped the optional dependency). ` +
      `reinstall without --omit=optional, or run: cargo install ${NAME}`,
  );
}

let binary;
try {
  binary = binaryPath();
} catch (error) {
  console.error(`${NAME}: ${error.message}`);
  process.exit(1);
}

// the child keeps the terminal: the confirmation questions are read from the same stdin.
const child = spawn(binary, process.argv.slice(2), { stdio: "inherit" });

// forward termination signals so the native process does not outlive the launcher (timeout, ci
// cancel, closing terminal). windows only supports SIGINT/SIGBREAK; the others are skipped.
const FORWARDED = process.platform === "win32" ? ["SIGINT", "SIGBREAK"] : ["SIGINT", "SIGTERM", "SIGHUP"];
// on a posix terminal ctrl-c already reaches the child through the tty (same foreground process
// group), so SIGINT is only swallowed here (the handler keeps the launcher alive until the child
// exits) and not forwarded a second time: the erase treats a second signal as a hard stop.
const TTY_SIGINT = process.platform !== "win32" && process.stdin.isTTY === true;
for (const signal of FORWARDED) {
  process.on(signal, () => {
    if (signal === "SIGINT" && TTY_SIGINT) return;
    if (child.exitCode === null && child.signalCode === null) child.kill(signal);
  });
}

child.on("error", (error) => {
  console.error(`${NAME}: could not run ${binary}: ${error.message}`);
  process.exit(1);
});

child.on("exit", (code, signal) => {
  if (signal) {
    process.exit(128 + (os.constants.signals[signal] ?? 0));
  }
  process.exit(code ?? 1);
});
