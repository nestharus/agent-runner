// Proof only: an ACP v2 agent endpoint hosted INSIDE the OpenCode process as
// a server plugin. Not product code, not wired anywhere.
//
// - Wire: upstream @agentclientprotocol/sdk/experimental/v2 AgentApp
//   (schema-v2.0.0-alpha.7), ND-JSON over a Unix socket named by
//   OULIPOLY_ACP_V2_SOCKET. No extra process: the listener lives in the
//   OpenCode process that owns the conversation (serve or TUI worker).
// - Insertion: native POST /session/{id}/prompt_async with a minted native
//   messageID. Native SessionPrompt.prompt persists the user message
//   (createUserMessage -> updateMessage/updatePart) before loop().
// - Sessions: session/new creates a native session; session/resume attaches
//   to an existing native session (no replay, the native store has history).
// - ACK: session/prompt answers { messageId } only after the native bus
//   reported message.updated(role=user, id) and the text part, and a native
//   GET of that message returns it with exactly the submitted text body.
//   Never from the HTTP 204 alone.
// - Diagnostics go to OULIPOLY_ACP_V2_LOG, never stderr (TUI screen).
// - Proof knob: host env OULIPOLY_ACP_V2_PROOF_NO_REPLY=1 forwards native
//   noReply so no model loop starts. Production would omit it.
// - No dedup contract is advertised (memory here does not survive restart).

import net from "node:net"
import { appendFileSync } from "node:fs"
import { randomBytes } from "node:crypto"
import { Readable, Writable } from "node:stream"
import { agent, methods, ndJsonStream, PROTOCOL_VERSION } from "@agentclientprotocol/sdk/experimental/v2"

type Waiter = { message: boolean; text: boolean; done: () => void }
type Shared = {
  waiters: Map<string, Waiter>
  sessions: Map<string, { client: any; directory: string; notify: Set<(update: unknown) => void> }>
  log: (line: string) => void
}

const g = globalThis as any
const shared: Shared = (g.__oulipolyAcpV2 ??= {
  waiters: new Map(),
  sessions: new Map(),
  // Never write to stderr: inside the TUI host that is the user's screen.
  log: (line: string) => {
    const file = process.env.OULIPOLY_ACP_V2_LOG
    if (file) appendFileSync(file, `[acp-v2-proof] ${new Date().toISOString()} ${line}\n`)
  },
})

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
  let value = BigInt(now) * 0x1000n + BigInt(counter)
  const time = Buffer.alloc(6)
  for (let i = 0; i < 6; i++) time[i] = Number((value >> BigInt(40 - 8 * i)) & 0xffn)
  const chars = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"
  const bytes = randomBytes(14)
  let tail = ""
  for (const b of bytes) tail += chars[b % 62]
  return "msg_" + time.toString("hex") + tail
}

function settle(id: string, field: "message" | "text") {
  const waiter = shared.waiters.get(id)
  if (!waiter) return
  waiter[field] = true
  if (waiter.message && waiter.text) waiter.done()
}

function listen(socketPath: string, client: any, directory: string) {
  const app = agent({ name: "oulipoly-opencode-v2-proof" })
    .onRequest(methods.agent.initialize, () => ({
      protocolVersion: PROTOCOL_VERSION,
      info: { name: "opencode+oulipoly-v2-proof", version: "0" },
      capabilities: { session: { resume: {} } as any },
    }))
    // Attach to a conversation that already exists natively (e.g. the one the
    // TUI was started on). No replay; the native store keeps the history.
    .onRequest(methods.agent.session.resume, async ({ params, client: acp }) => {
      const existing = await client.session.get({ path: { id: params.sessionId }, throwOnError: true })
      const id = existing.data.id as string
      const notify = new Set<(update: unknown) => void>()
      notify.add((update) => void acp.notify(methods.client.session.update, { sessionId: id, update } as any))
      shared.sessions.set(id, { client, directory: params.cwd, notify })
      shared.log(`session/resume native=${id} title=${JSON.stringify(existing.data.title)} cwd=${params.cwd}`)
      return {}
    })
    .onRequest(methods.agent.session.new, async ({ params, client: acp }) => {
      const created = await client.session.create({ body: {}, throwOnError: true })
      const id = created.data.id as string
      const notify = new Set<(update: unknown) => void>()
      notify.add((update) => void acp.notify(methods.client.session.update, { sessionId: id, update } as any))
      shared.sessions.set(id, { client, directory: params.cwd, notify })
      shared.log(`session/new native=${id} cwd=${params.cwd}`)
      return { sessionId: id }
    })
    .onRequest(methods.agent.session.prompt, async ({ params }) => {
      const session = shared.sessions.get(params.sessionId)
      if (!session) throw new Error(`unknown session ${params.sessionId}`)
      const texts = params.prompt.map((block: any) => {
        if (block.type !== "text") throw new Error(`proof handles text blocks only, got ${block.type}`)
        return block.text as string
      })
      const meta = (params as any)._meta ?? {}
      const noReply = process.env.OULIPOLY_ACP_V2_PROOF_NO_REPLY === "1"
      const messageID = mintMessageID()
      const inserted = new Promise<void>((done) =>
        shared.waiters.set(messageID, { message: false, text: false, done }),
      )
      const res = await client.session.promptAsync({
        path: { id: params.sessionId },
        body: {
          messageID,
          parts: texts.map((text) => ({ type: "text", text })),
          noReply,
          model: { providerID: "proof-none", modelID: "none" },
        },
      })
      shared.log(`prompt_async http=${res.response?.status} messageID=${messageID} noReply=${noReply}`)
      await inserted
      shared.waiters.delete(messageID)
      const readback = await client.session.message({
        path: { id: params.sessionId, messageID },
        throwOnError: true,
      })
      const info = readback.data.info
      const body = readback.data.parts.filter((part: any) => part.type === "text").map((part: any) => part.text)
      if (info.id !== messageID || info.role !== "user" || JSON.stringify(body) !== JSON.stringify(texts))
        throw new Error("native readback mismatch")
      shared.log(`ACK messageId=${messageID} readback.parts=${readback.data.parts.length}`)
      for (const send of session.notify)
        send({ sessionUpdate: "user_message", messageId: messageID, content: texts.map((text) => ({ type: "text", text })) })
      const echo = typeof meta["oulipoly.ai/messageKey"] === "string" ? { "oulipoly.ai/messageKey": meta["oulipoly.ai/messageKey"] } : {}
      return { messageId: messageID, _meta: echo }
    })

  const server = net.createServer((socket) => {
    const output = Writable.toWeb(socket) as WritableStream<Uint8Array>
    const input = Readable.toWeb(socket) as ReadableStream<Uint8Array>
    app.connect(ndJsonStream(output, input))
  })
  server.listen(socketPath, () => shared.log(`listening ${socketPath} pid=${process.pid}`))
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
      if (event.type === "message.updated" && p.info?.role === "user") {
        shared.log(`bus message.updated user id=${p.info.id}`)
        settle(p.info.id, "message")
      }
      if (event.type === "message.part.updated" && p.part?.type === "text") settle(p.part.messageID, "text")
      if (event.type === "message.updated" && p.info?.role === "assistant")
        shared.log(`bus message.updated ASSISTANT id=${p.info.id}`)
      if (event.type === "session.status") {
        const session = shared.sessions.get(p.sessionID)
        const state = p.status?.type === "idle" ? "idle" : p.status?.type === "busy" ? "running" : undefined
        shared.log(`bus session.status ${p.sessionID} ${p.status?.type}`)
        if (session && state) for (const send of session.notify) send({ sessionUpdate: "state_update", state })
      }
    },
  }
}
