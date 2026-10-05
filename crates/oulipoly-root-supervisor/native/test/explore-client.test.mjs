// Deterministic controls of the parent's explorer client and the OpenCode
// `explore` tool against a fake root ingress (a Unix socket speaking the
// owner's child protocol). No owner, model, OpenCode or Claude runs here.
//   node --test --experimental-strip-types native/test/explore-client.test.mjs

import { test } from "node:test"
import assert from "node:assert/strict"
import { createServer } from "node:net"
import { cpSync, mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join, dirname } from "node:path"
import { fileURLToPath, pathToFileURL } from "node:url"
import { exploreRequest, renderChild, pickRoute, exploreDescription } from "../explore-client.mjs"

const here = dirname(fileURLToPath(import.meta.url))

// A fake ingress: records each request line and the client's close, then
// answers with `reply(request, socket)`.
async function ingress(reply) {
  const dir = mkdtempSync(join(tmpdir(), "explore-"))
  const path = join(dir, "bash.sock")
  const seen = { requests: [], closed: 0 }
  const server = createServer((socket) => {
    let buffer = ""
    socket.setEncoding("utf8")
    socket.on("data", (chunk) => {
      buffer += chunk
      const at = buffer.indexOf("\n")
      if (at >= 0) {
        const request = JSON.parse(buffer.slice(0, at))
        seen.requests.push(request)
        reply(request, socket)
      }
    })
    socket.on("close", () => seen.closed++)
    socket.on("error", () => {})
  })
  await new Promise((done) => server.listen(path, done))
  return { path, seen, dir, close: () => new Promise((done) => server.close(() => { rmSync(dir, { recursive: true, force: true }); done() })) }
}

const line = (socket, value) => socket.write(JSON.stringify(value) + "\n")
const accepted = { event: "accepted", child: "child-1", starts: { used: 1, max: 4 }, concurrent: { live: 1, max: 2 } }

test("answered with unproved end: content succeeds, lifecycle stays visible and unclaimed", async () => {
  const fake = await ingress((request, socket) => {
    line(socket, accepted)
    line(socket, { event: "agent-message", linked: true, text: "noise" })
    line(socket, { event: "turn-end", stop_reason: "end_turn" })
    line(socket, { event: "end-unknown", reason: "root-pid1-connection-lost" })
    line(socket, {
      event: "result", child: "child-1", route: request.route, outcome: "answered", answer: "it is in lib.rs",
      end: { event: "end-unknown" },
      lifecycle: { end: "unknown", bash_runs_open: 1, bash_run_end_unknown: true, budget: "still charged (an end is unknown: it may be running)" },
    })
  })
  try {
    const result = await exploreRequest("luna-max", "where?", { socketPath: fake.path })
    assert.deepEqual(fake.seen.requests, [{ v: 1, op: "child", route: "luna-max", prompt: "where?" }])
    assert.equal(result.isError, false)
    assert.match(result.text, /: answered\./)
    assert.match(result.text, /it is in lib\.rs/)
    assert.match(result.text, /Lifecycle status: end unknown; its Bash runs still open at this result: 1 \(one ended unknown\); root budget: still charged/)
    assert.match(result.text, /does not prove the child or its Bash ended or drained/)
    assert.match(result.text, /end-unknown\(root-pid1-connection-lost\)/)
    assert.doesNotMatch(result.text, /noise/)
  } finally {
    await fake.close()
  }
})

test("refusal is an error that says nothing started", async () => {
  const fake = await ingress((_, socket) => line(socket, { event: "refused", reason: "budget-concurrent: 2 of 2 (1 unknown-end charged)" }))
  try {
    const result = await exploreRequest("luna-max", "q", { socketPath: fake.path })
    assert.equal(result.isError, true)
    assert.match(result.text, /refused by this root \(budget-concurrent: 2 of 2 \(1 unknown-end charged\)\)\. No child was started/)
  } finally {
    await fake.close()
  }
})

test("admitted then lost: possibly running, no result claimed, no retry", async () => {
  const fake = await ingress((_, socket) => { line(socket, accepted); socket.end() })
  try {
    const result = await exploreRequest("luna-max", "q", { socketPath: fake.path })
    assert.equal(result.isError, true)
    assert.match(result.text, /was admitted, but its result was not received .*it may have run.*Do not retry/s)
  } finally {
    await fake.close()
  }
})

test("abort closes the connection (the owner's stop cue) and reports it", async () => {
  const fake = await ingress((_, socket) => line(socket, accepted))
  try {
    const controller = new AbortController()
    const pending = exploreRequest("luna-max", "q", { socketPath: fake.path, signal: controller.signal })
    await new Promise((done) => setTimeout(done, 100))
    controller.abort()
    const result = await pending
    assert.equal(result.isError, true)
    assert.match(result.text, /aborted by this agent/)
    await new Promise((done) => setTimeout(done, 100))
    assert.equal(fake.seen.closed, 1)
  } finally {
    await fake.close()
  }
})

test("no ingress or no route: nothing asked", async () => {
  const none = await exploreRequest("luna-max", "q", { socketPath: "" })
  assert.equal(none.isError, true)
  assert.match(none.text, /nothing was asked/)
  const unreachable = await exploreRequest("luna-max", "q", { socketPath: join(tmpdir(), "absent-explore.sock") })
  assert.equal(unreachable.isError, true)
  assert.match(unreachable.text, /not delivered/)
  assert.equal(pickRoute({ routes: ["a", "b"] }, undefined), undefined)
  assert.equal(pickRoute({ routes: ["a"] }, undefined), "a")
  assert.match(exploreDescription({ routes: ["luna-max"], max_starts: 4, max_concurrent: 2 }), /4 child starts in total, 2 at once/)
})

test("a result without lifecycle says end and drain are unknown", () => {
  const result = renderChild([accepted, { event: "result", child: "child-1", route: "r", outcome: "no-answer" }])
  assert.equal(result.isError, true)
  assert.match(result.text, /not reported by the root \(end and drain unknown to this tool\)/)
})

test("OpenCode explore tool, laid out as provisioning writes it, returns the rendered text", async () => {
  const config = mkdtempSync(join(tmpdir(), "explore-tool-"))
  try {
    mkdirSync(join(config, "tool"))
    cpSync(join(here, "../opencode/explore-tool.ts"), join(config, "tool/explore.ts"))
    cpSync(join(here, "../explore-client.mjs"), join(config, "explore-client.mjs"))
    writeFileSync(join(config, "explore.json"), JSON.stringify({ routes: ["luna-max"], max_starts: 4, max_concurrent: 2 }))
    // A stand-in for @opencode-ai/plugin: `tool` returns its definition.
    const plugin = join(config, "node_modules/@opencode-ai/plugin")
    mkdirSync(plugin, { recursive: true })
    writeFileSync(join(plugin, "package.json"), JSON.stringify({ name: "@opencode-ai/plugin", type: "module", main: "index.js" }))
    writeFileSync(join(plugin, "index.js"), `
      const chain = () => ({ describe() { return this }, optional() { return this } })
      export function tool(definition) { return definition }
      tool.schema = { string: chain }
    `)
    const fake = await ingress((request, socket) => {
      line(socket, accepted)
      line(socket, { event: "result", child: "child-1", route: request.route, outcome: "answered", answer: `asked ${request.prompt}`,
        lifecycle: { end: "observed", bash_runs_open: 0, budget: "released" } })
    })
    try {
      const { default: explore } = await import(pathToFileURL(join(config, "tool/explore.ts")).href)
      assert.match(explore.description, /Routes: luna-max/)
      process.env.OULIPOLY_ROOT_BASH_V1 = fake.path
      const text = await explore.execute({ question: "where is X" }, { abort: new AbortController().signal })
      assert.equal(typeof text, "string")
      assert.deepEqual(fake.seen.requests, [{ v: 1, op: "child", route: "luna-max", prompt: "where is X" }])
      assert.match(text, /answered\.\nAnswer .*\nasked where is X\nLifecycle status: end observed; its Bash runs still open at this result: 0; root budget: released/s)
    } finally {
      delete process.env.OULIPOLY_ROOT_BASH_V1
      await fake.close()
    }
  } finally {
    rmSync(config, { recursive: true, force: true })
  }
})
