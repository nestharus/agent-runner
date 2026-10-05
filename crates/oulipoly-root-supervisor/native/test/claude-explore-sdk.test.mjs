// Published Agent SDK + MCP client/server, actual receiver tool callback,
// fake root ingress. No Claude executable, account or model is invoked.
// Run in a B-owned native source overlay with published node_modules and
// OULIPOLY_CLAUDE_RECEIVER_NO_MAIN=1; see packaging/native-linux/README.md.
import { test } from "node:test"
import assert from "node:assert/strict"
import { createServer } from "node:net"
import { mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { createSdkMcpServer } from "@anthropic-ai/claude-agent-sdk"
import { Client } from "@modelcontextprotocol/sdk/client/index.js"
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js"
import { exploreTool } from "../claude/acp-v2-receiver.mjs"

const config = { routes: ["luna-max"], max_starts: 4, max_concurrent: 2 }
const bounded = (promise) => {
  let timer
  return Promise.race([promise, new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error("SDK explore watchdog")), 5000)
  })]).finally(() => clearTimeout(timer))
}

async function fixture(answer) {
  const dir = mkdtempSync(join(tmpdir(), "sdk-explore-"))
  const path = join(dir, "bash.sock")
  let admitted, disconnected
  const request = new Promise((done) => { admitted = done })
  const closed = new Promise((done) => { disconnected = done })
  const sockets = new Set()
  const server = createServer((socket) => {
    sockets.add(socket)
    let buffer = ""
    socket.on("data", (data) => {
      buffer += data
      if (!buffer.includes("\n")) return
      const value = JSON.parse(buffer.slice(0, buffer.indexOf("\n")))
      admitted(value)
      const line = (v) => socket.write(JSON.stringify(v) + "\n")
      line({ event: "accepted", child: "child-1", starts: { used: 1, max: 4 }, concurrent: { live: 1, max: 2 } })
      if (answer) line({ event: "result", child: "child-1", route: value.route, outcome: "answered", answer,
        lifecycle: { end: "observed", bash_runs_open: 0, budget: "still charged (release pending)" } })
    })
    socket.on("error", () => {})
    socket.on("close", () => { sockets.delete(socket); disconnected() })
  })
  await new Promise((done) => server.listen(path, done))
  process.env.OULIPOLY_ROOT_BASH_V1 = path
  const sdk = createSdkMcpServer({ name: "oulipoly", version: "1", alwaysLoad: true, tools: [exploreTool(config)] })
  const client = new Client({ name: "fake-claude-peer", version: "1" })
  const [near, far] = InMemoryTransport.createLinkedPair()
  await sdk.instance.connect(far)
  await client.connect(near)
  return { client, request, closed, async close() {
    await client.close()
    await sdk.instance.close()
    for (const socket of sockets) socket.destroy()
    await new Promise((done) => server.close(done))
    delete process.env.OULIPOLY_ROOT_BASH_V1
    rmSync(dir, { recursive: true, force: true })
  } }
}

test("published SDK registers and invokes the receiver explore callback", async () => {
  const f = await fixture("wired via native_root.rs")
  try {
    const tools = await f.client.listTools()
    assert.equal(tools.tools.length, 1)
    assert.equal(tools.tools[0].name, "explore")
    assert.equal(tools.tools[0]._meta["anthropic/alwaysLoad"], true)
    assert.match(tools.tools[0].description, /4 child starts in total, 2 at once/)
    const reply = await bounded(f.client.callTool({ name: "explore", arguments: { question: "where?" } }))
    assert.deepEqual(await f.request, { v: 1, op: "child", route: "luna-max", prompt: "where?" })
    assert.equal(reply.isError, false)
    assert.match(reply.content[0].text, /wired via native_root.rs/)
    assert.match(reply.content[0].text, /Lifecycle status: end observed.*release pending/)
    await bounded(f.closed)
  } finally { await f.close() }
})

test("published MCP cancellation propagates extra.signal and closes admitted ingress", async () => {
  const f = await fixture(null)
  try {
    const controller = new AbortController()
    const pending = f.client.callTool({ name: "explore", arguments: { question: "wait?" } }, undefined,
      { signal: controller.signal, timeout: 5000 })
    // Attach rejection before abort; cancellation need not return content.
    const rejected = assert.rejects(pending)
    assert.deepEqual(await bounded(f.request), { v: 1, op: "child", route: "luna-max", prompt: "wait?" })
    controller.abort(new Error("fake peer cancelled"))
    await bounded(rejected)
    await bounded(f.closed)
  } finally { await f.close() }
})
