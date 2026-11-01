import test from "node:test";
import assert from "node:assert/strict";
import {
  DEFAULT_PORT,
  canEnable,
  deviceLine,
  networkHint,
  parsePort,
  passwordProblem,
  portProblem,
} from "../logic/network.ts";
import { setTimeFormat } from "../logic/format.ts";

test("a port is a whole number from 1024 to 65535", () => {
  assert.equal(parsePort(String(DEFAULT_PORT)), 8796);
  assert.equal(parsePort(" 9000 "), 9000);
  assert.equal(parsePort("1024"), 1024);
  assert.equal(parsePort("65535"), 65535);
  for (const bad of ["", "80", "1023", "65536", "8796.5", "-1", "abc", "1e4", "0x2000"]) {
    assert.equal(parsePort(bad), null, bad);
    assert.ok(portProblem(bad), bad);
  }
  assert.equal(portProblem("8796"), null);
  assert.match(portProblem("80"), /1024 and 65535/);
  assert.match(portProblem("http"), /whole number/);
});

test("a password is long enough, typed twice the same", () => {
  assert.match(passwordProblem("short", "short"), /8 characters/);
  assert.match(passwordProblem("        ", "        "), /spaces/);
  assert.match(passwordProblem("correct horse", "correct horsE"), /differ/);
  assert.equal(passwordProblem("correct horse", "correct horse"), null);
  // Characters, not bytes: eight accented letters are enough.
  assert.equal(passwordProblem("éééééééé", "éééééééé"), null);
  assert.equal(passwordProblem("🔑🔑🔑🔑🔑🔑🔑", "🔑🔑🔑🔑🔑🔑🔑"), "At least 8 characters.");
  const huge = "x".repeat(1025);
  assert.match(passwordProblem(huge, huge), /too long/);
});

test("network access is turned on only with a password, and off always", () => {
  assert.equal(canEnable({ enabled: false, passwordSet: false }), false);
  assert.equal(canEnable({ enabled: false, passwordSet: true }), true);
  assert.equal(canEnable({ enabled: true, passwordSet: false }), true);
});

const net = (over) => ({
  enabled: false,
  port: 8796,
  https: true,
  passwordSet: true,
  addresses: [],
  devices: [],
  ...over,
});

test("the box's hint says how it is", () => {
  assert.equal(networkHint(net({})), "off");
  assert.equal(networkHint(net({ enabled: true })), "on · HTTPS");
  assert.equal(networkHint(net({ enabled: true, https: false })), "on · plain HTTP");
  assert.equal(networkHint(net({ enabled: true, error: "port 8796 is in use" })), "not working");
});

test("a device says where and when it was last seen", () => {
  setTimeFormat("24h");
  const created = Math.floor(new Date(2026, 9, 3, 18, 5).getTime() / 1000);
  const now = created + 10 * 86_400;
  const line = deviceLine(
    { id: "ab", name: "Phone", userAgent: "", created, lastUsed: now - 600, address: "192.168.1.30" },
    now,
  );
  assert.match(line, /^Last used 10m ago from 192\.168\.1\.30 · signed in .*18:05$/);
  assert.match(deviceLine({ id: "ab", name: "Phone", userAgent: "", created }, now), /^Signed in /);
  assert.match(
    deviceLine({ id: "ab", name: "Phone", userAgent: "", created, address: "10.0.0.2" }, now),
    /^From 10\.0\.0\.2 · signed in /,
  );
});
