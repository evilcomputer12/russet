// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// A synthetic russetd for the W19 screenshot pass: a named-pipe JSON-RPC server
// that answers what the setup, sign-in and run-destination surfaces ask, with
// made-up devices and a made-up account (no real identifiers). Apple ID sign-in
// follows W22's wire shapes: begin → two-factor (code already pushed to the
// trusted devices) → requestCode for a text → submitCode → Account.

import * as net from "node:net";
import { createMessageConnection, SocketMessageReader, SocketMessageWriter, ResponseError } from "vscode-jsonrpc/node.js";

const ACCOUNT = {
  id: "acct-synthetic",
  kind: "apple-id",
  label: "synthetic.developer@example.invalid",
  teamId: "SYNTHTEAM1",
  isDefault: true,
};

export function startFakeDaemon(pipePath, projectRoot) {
  let signedIn = false;
  const handlers = {
    initialize: () => ({
      protocol: { major: 1, minor: 0 },
      server: {
        name: "russetd", version: "0.0.0-synthetic", buildId: "0.0.0+gsynthetic", pid: process.pid,
        executable: "fake-russetd", executableId: { dev: 0, ino: 0, mtimeNs: 0, size: 0 },
        startedAt: new Date().toISOString(), platform: process.platform,
      },
      connectionId: "c-1",
      grant: { profile: "ide", allow: [], confirm: [], approval: [], deny: [] },
      capabilities: { methods: [], notifications: [], features: { mirror: false, wifiDevices: false, mcpHttp: false } },
      endpoints: {},
    }),
    "daemon.ping": () => ({}),
    "device.watch": () => ({ subscriptionId: "sub-dev" }),
    "device.list": () => ({
      devices: [
        {
          udid: "00000000-0000000000SYNTH1", name: "Test iPhone", marketingName: "iPhone 15", productType: "iPhone15,4",
          osVersion: "26.1", build: "23B85", transport: "usb", codeSigningMonitor: "txm", state: "ready",
        },
      ],
    }),
    "project.info": () => ({
      stack: "swiftpm", root: projectRoot, name: "HelloProject",
      targets: [{ name: "HelloApp", bundleId: "com.example.HelloApp", kind: "app" }],
      configurations: ["debug", "release"], defaultTarget: "HelloApp",
    }),
    "account.list": () => ({ accounts: signedIn ? [ACCOUNT] : [] }),
    "toolchain.list": () => ({ tools: [{ component: "swift", version: "6.4", path: "/opt/swift", provenance: { kind: "managed", pin: "6.4" }, env: {}, ok: true, problems: [] }], sdks: [], pins: {} }),
    "account.anisette.status": () => ({ kind: "local", librariesPresent: true, apkVersion: "synthetic", provisioned: true, localSupported: true, pinned: true }),
    "account.appleId.begin": (p) => {
      if (!p || typeof p.password !== "string" || p.password.length === 0) {
        throw new ResponseError(5250, "bad credentials", { russetCode: "gsa.badCredentials" });
      }
      return {
        sessionId: "synthetic-session",
        step: "two-factor",
        twoFactor: { via: ["trusted-device", "sms"], codeSentTo: "trusted-device", phoneNumbers: [{ id: 1, number: "(•••) •••-••42" }], codeLength: 6 },
      };
    },
    "account.appleId.requestCode": (p) => {
      if (p?.via !== "trusted-device" && p?.via !== "sms") {
        throw new ResponseError(-32602, "unknown variant for via");
      }
      return {};
    },
    "account.appleId.submitCode": (p) => {
      if (p?.code !== "123456") {
        throw new ResponseError(5251, "bad code", { russetCode: "gsa.badCode" });
      }
      signedIn = true;
      return ACCOUNT;
    },
    "doctor.run": () => ({ checks: [] }),
  };
  const server = net.createServer((socket) => {
    const connection = createMessageConnection(new SocketMessageReader(socket), new SocketMessageWriter(socket));
    for (const [method, handler] of Object.entries(handlers)) {
      connection.onRequest(method, (params) => handler(params ?? {}));
    }
    connection.onRequest((method) => {
      throw new ResponseError(1005, `${method} is not implemented in the synthetic daemon`, { russetCode: "core.notImplemented" });
    });
    connection.listen();
    socket.on("error", () => {});
  });
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(pipePath, () => resolve(server));
  });
}
