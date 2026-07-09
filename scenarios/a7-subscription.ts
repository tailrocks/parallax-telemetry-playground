#!/usr/bin/env bun
// A7: minimal GraphQL-over-WebSocket subscription smoke using Bun's native
// WebSocket and the graphql-transport-ws protocol.

const url = process.env.CATALOG_WS_URL ?? "ws://localhost:8080/graphql"
const timeoutMs = Number(process.env.SUBSCRIPTION_TIMEOUT_MS ?? "10000")
const id = "price-changes"

let events = 0
let acknowledged = false

const socket = new WebSocket(url, "graphql-transport-ws")

const timer = setTimeout(() => {
  socket.close(1000, "timeout")
  if (events < 1) {
    console.error(`A7 failed: no subscription events received from ${url}`)
    process.exit(1)
  }
  console.log(`A7 done: received ${events} subscription event(s) from ${url}`)
  process.exit(0)
}, timeoutMs)

socket.addEventListener("open", () => {
  socket.send(JSON.stringify({ type: "connection_init", payload: {} }))
})

socket.addEventListener("message", (event) => {
  const message = JSON.parse(String(event.data)) as {
    type: string
    id?: string
    payload?: unknown
  }

  if (message.type === "connection_ack") {
    acknowledged = true
    socket.send(
      JSON.stringify({
        id,
        type: "subscribe",
        payload: {
          query: "subscription priceSmoke { priceChanges { id sku name priceMinor } }",
        },
      })
    )
    return
  }

  if (message.type === "next" && message.id === id) {
    events += 1
    console.log(JSON.stringify(message.payload))
    return
  }

  if (message.type === "error") {
    console.error(JSON.stringify(message.payload))
    clearTimeout(timer)
    socket.close(1011, "subscription error")
    process.exit(1)
  }
})

socket.addEventListener("close", () => {
  if (!acknowledged || events < 1) {
    clearTimeout(timer)
    console.error(
      `A7 failed: websocket closed before receiving an event from ${url}`
    )
    process.exit(1)
  }
})

socket.addEventListener("error", () => {
  clearTimeout(timer)
  console.error(`A7 failed: websocket error from ${url}`)
  process.exit(1)
})
