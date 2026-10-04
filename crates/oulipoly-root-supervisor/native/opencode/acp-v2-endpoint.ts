// ACP v2 agent endpoint hosted INSIDE the OpenCode process as a server
// plugin, for a root supervisor harness with `"endpoint": "unix-socket"`.
// There is no bridge process: the listener lives in the OpenCode process
// that owns the native conversations.
//
// - Wire: upstream @agentclientprotocol/sdk/experimental/v2 AgentApp
//   (schema-v2.0.0-alpha.7), ND-JSON over the Unix socket the owner chose
//   and passed as OULIPOLY_ACP_V2_SOCKET. Without it nothing listens.
// - Scope: one OpenCode instance, its directory. session/new and
//   session/resume refuse any other cwd; resume also refuses a native
//   session recorded for another directory.
// - session/new creates a native session; session/resume attaches to an
//   existing native one without replay (the native store has the history).
// - session/prompt inserts through native POST /session/{id}/prompt_async
//   with a minted native messageID and the session's own configured model.
//   It answers { messageId } only after the native bus reported the user
//   message and its text part AND a native GET returns that message with
//   exactly the submitted text. Never from the HTTP status alone; a non-2xx
//   status is an error.
// - OULIPOLY_ACP_V2_NO_REPLY=1 makes the host insertion-only: native noReply,
//   so no model turn starts. Unset, the native loop runs whatever model the
//   session is configured with.
// - No dedup contract is advertised: nothing here survives a restart.
// - Diagnostics go to OULIPOLY_ACP_V2_LOG, never stderr (a TUI's screen).

import net from "node:net"
import { appendFileSync } from "node:fs"
import { randomBytes } from "node:crypto"
import { Readable, Writable } from "node:stream"
import { agent, methods, ndJsonStream, PROTOCOL_VERSION } from "@agentclientprotocol/sdk/experimental/v2"

type Notify = (update: unknown) => void
type Waiter = { message: boolean; text: boolean; done: () => void }

const g = globalThis as any
const shared: {
  waiters: Map<string, Waiter>
  // Native session id -> the update senders of connections attached to it.
  sessions: Map<string, Set<Notify>>
} = (g.__oulipolyAcpV2 ??= { waiters: new Map(), sessions: new Map() })

function log(line: string) {
  const file = process.env.OULIPOLY_ACP_V2_LOG
  if (file) appendFileSync(file, `[acp-v2] ${new Date().toISOString()} ${line}\n`)
}

// Same shape as OpenCode's Identifier.ascending("message"): msg_ + 12 hex + 14 base62.
let lastMs = 0
let counter = 0
function mintMessageID(): string {
  const now = Date.now()
  if (now !== lastMs) {
    lastMs = now
    counter = 0
  }
  counter++
  const value = BigInt(now) * 0x1000n + BigInt(counter)
  const time = Buffer.alloc(6)
  for (let i = 0; i < 6; i++) time[i] = Number((value >> BigInt(40 - 8 * i)) & 0xffn)
  const chars = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"
  let tail = ""
  for (const b of randomBytes(14)) tail += chars[b % 62]
  return "msg_" + time.toString("hex") + tail
}

function settle(id: string, field: "message" | "text") {
  const waiter = shared.waiters.get(id)
  if (!waiter) return
  waiter[field] = true
  if (waiter.message && waiter.text) waiter.done()
}

function listen(socketPath: string, client: any, directory: string) {
  // Connections, each with the sessions it attached to.
  const app = (attached: Map<string, Notify>) =>
    agent({ name: "oulipoly-opencode-acp-v2" })
      .onRequest(methods.agent.initialize, () => ({
        protocolVersion: PROTOCOL_VERSION,
        info: { name: "opencode+oulipoly-acp-v2", version: "0" },
        // `{}`: the baseline session surface, session/resume included.
        capabilities: { session: {} },
      }))
      .onRequest(methods.agent.session.resume, async ({ params, client: acp }) => {
        if (params.cwd !== directory) throw new Error(`cwd ${params.cwd} is not this host's ${directory}`)
        const existing = await client.session.get({ path: { id: params.sessionId }, throwOnError: true })
        const id = existing.data.id as string
        if (existing.data.directory !== directory)
          throw new Error(`session ${id} belongs to ${existing.data.directory}, not ${directory}`)
        attach(attached, id, acp)
        log(`session/resume native=${id} title=${JSON.stringify(existing.data.title)}`)
        return {}
      })
      .onRequest(methods.agent.session.new, async ({ params, client: acp }) => {
        if (params.cwd !== directory) throw new Error(`cwd ${params.cwd} is not this host's ${directory}`)
        const created = await client.session.create({ body: {}, throwOnError: true })
        const id = created.data.id as string
        attach(attached, id, acp)
        log(`session/new native=${id}`)
        return { sessionId: id }
      })
      .onRequest(methods.agent.session.prompt, async ({ params }) => {
        if (!attached.has(params.sessionId)) throw new Error(`session ${params.sessionId} is not open here`)
        const texts = params.prompt.map((block: any) => {
          if (block.type !== "text") throw new Error(`only text blocks are handled, got ${block.type}`)
          return block.text as string
        })
        const noReply = process.env.OULIPOLY_ACP_V2_NO_REPLY === "1"
        const messageID = mintMessageID()
        const inserted = new Promise<void>((done) => shared.waiters.set(messageID, { message: false, text: false, done }))
        try {
          const res = await client.session.promptAsync({
            path: { id: params.sessionId },
            body: { messageID, parts: texts.map((text) => ({ type: "text", text })), noReply },
          })
          const status = res.response?.status ?? 0
          log(`prompt_async http=${status} messageID=${messageID} noReply=${noReply}`)
          if (status < 200 || status > 299) throw new Error(`native prompt_async answered ${status}`)
          await inserted
        } finally {
          shared.waiters.delete(messageID)
        }
        const readback = await client.session.message({
          path: { id: params.sessionId, messageID },
          throwOnError: true,
        })
        const info = readback.data.info
        const body = readback.data.parts.filter((part: any) => part.type === "text").map((part: any) => part.text)
        if (info.id !== messageID || info.role !== "user" || JSON.stringify(body) !== JSON.stringify(texts))
          throw new Error("native readback mismatch")
        log(`ACK messageId=${messageID}`)
        for (const send of shared.sessions.get(params.sessionId) ?? [])
          send({ sessionUpdate: "user_message", messageId: messageID, content: texts.map((text) => ({ type: "text", text })) })
        return { messageId: messageID }
      })

  const server = net.createServer((socket) => {
    const attached = new Map<string, Notify>()
    socket.on("close", () => {
      for (const [id, send] of attached) shared.sessions.get(id)?.delete(send)
    })
    socket.on("error", (error) => log(`connection error ${error.message}`))
    const output = Writable.toWeb(socket) as WritableStream<Uint8Array>
    const input = Readable.toWeb(socket) as ReadableStream<Uint8Array>
    app(attached).connect(ndJsonStream(output, input))
  })
  server.on("error", (error) => log(`listener error ${error.message}`))
  server.listen(socketPath, () => log(`listening ${socketPath} pid=${process.pid} directory=${directory}`))
}

function attach(attached: Map<string, Notify>, id: string, acp: any) {
  const send: Notify = (update) => void acp.notify(methods.client.session.update, { sessionId: id, update } as any)
  const previous = attached.get(id)
  if (previous) shared.sessions.get(id)?.delete(previous)
  attached.set(id, send)
  let senders = shared.sessions.get(id)
  if (!senders) shared.sessions.set(id, (senders = new Set()))
  senders.add(send)
}

export const AcpV2Endpoint = async ({ client, directory }: any) => {
  const socketPath = process.env.OULIPOLY_ACP_V2_SOCKET
  if (socketPath && !g.__oulipolyAcpV2Listening) {
    g.__oulipolyAcpV2Listening = true
    listen(socketPath, client, directory)
  }
  return {
    event: async ({ event }: any) => {
      const p = event.properties ?? {}
      if (event.type === "message.updated" && p.info?.role === "user") settle(p.info.id, "message")
      if (event.type === "message.part.updated" && p.part?.type === "text") settle(p.part.messageID, "text")
      if (event.type === "session.status") {
        const state = p.status?.type === "idle" ? "idle" : p.status?.type === "busy" ? "running" : undefined
        if (state) for (const send of shared.sessions.get(p.sessionID) ?? []) send({ sessionUpdate: "state_update", state })
      }
    },
  }
}
