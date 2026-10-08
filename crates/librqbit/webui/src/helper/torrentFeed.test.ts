// Run with `npm test`. TorrentFeed against a fake WebSocket, fake timers and a fake
// poll endpoint, replaying the server's golden messages.
import messagesFile from "../../../src/list_feed/testdata/list_messages.json";
import { FeedMessage } from "./listFeed";
import {
  FeedEnv,
  TorrentFeed,
  WATCHDOG_MS,
  WebSocketLike,
} from "./torrentFeed";

let failures = 0;
let checks = 0;
function eq(actual: unknown, expected: unknown, what: string) {
  checks++;
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    failures++;
    console.error(
      `FAIL ${what}: got ${JSON.stringify(actual)}, want ${JSON.stringify(expected)}`,
    );
  }
}
const messages = (messagesFile as any).messages as FeedMessage[];
const flush = () => new Promise((r) => setTimeout(r, 0));

class FakeWS implements WebSocketLike {
  static all: FakeWS[] = [];
  binaryType = "blob";
  readyState = 0;
  onopen: ((ev: any) => void) | null = null;
  onmessage: ((ev: { data: any }) => void) | null = null;
  onclose: ((ev: any) => void) | null = null;
  onerror: ((ev: any) => void) | null = null;
  sent: any[] = [];
  closed = false;
  constructor(public url: string) {
    FakeWS.all.push(this);
  }
  send(d: string) {
    this.sent.push(JSON.parse(d));
  }
  close() {
    if (this.closed) return;
    this.closed = true;
    this.onclose?.({});
  }
  open() {
    this.readyState = 1;
    this.onopen?.({});
  }
  deliver(m: unknown) {
    this.onmessage?.({ data: JSON.stringify(m) });
  }
  fail() {
    this.onerror?.({});
    this.close();
  }
}

function harness(opts: { ws?: boolean } = {}) {
  FakeWS.all = [];
  let now = 0;
  let timers: { at: number; f: () => void; id: number }[] = [];
  let nextId = 1;
  const polls: (null | { seq: number; epoch: string })[] = [];
  const pollQueue: (FeedMessage | Error)[] = [];
  const env: FeedEnv = {
    wsUrl: opts.ws === false ? null : "ws://server/stream/torrents",
    WebSocket: FakeWS as any,
    makeInflater: null,
    poll: async (since) => {
      polls.push(since);
      const m = pollQueue.shift();
      if (!m) throw new Error("no poll answer queued");
      if (m instanceof Error) throw m;
      return m;
    },
    setTimeout: (f, ms) => {
      const id = nextId++;
      timers.push({ at: now + ms, f, id });
      return id;
    },
    clearTimeout: (id) => {
      timers = timers.filter((t) => t.id !== id);
    },
    now: () => now,
  };
  const seen: number[] = [];
  const errors: unknown[] = [];
  const transports: string[] = [];
  const feed = new TorrentFeed(env, {
    onTorrents: (t) => seen.push(t.length),
    onError: (e) => errors.push(e),
    onTransport: (t) => transports.push(t),
  });
  const advance = async (ms: number) => {
    const end = now + ms;
    for (;;) {
      timers.sort((a, b) => a.at - b.at);
      const t = timers[0];
      if (!t || t.at > end) break;
      timers.shift();
      now = t.at;
      t.f();
      await flush();
    }
    now = end;
    await flush();
  };
  return {
    feed,
    env,
    seen,
    errors,
    transports,
    polls,
    pollQueue,
    advance,
    timers: () => timers,
  };
}

// 1. WebSocket: snapshot, deltas, tick/refresh messages.
{
  const h = harness();
  h.feed.start();
  const ws = FakeWS.all[0];
  eq(ws.url, "ws://server/stream/torrents?tick_ms=1000", "url");
  ws.open();
  ws.deliver(messages[0]);
  ws.deliver(messages[1]);
  eq(h.seen, [23, 23], "snapshot + delta applied");
  eq(h.feed.transport, "websocket", "transport");
  h.feed.setTickMs(5000);
  h.feed.refresh();
  eq(
    ws.sent,
    [{ type: "tick", ms: 5000 }, { type: "refresh" }],
    "tick + refresh sent",
  );
  // 2. Gap → resync, then a fresh snapshot is accepted.
  ws.deliver(messages[3]);
  eq(ws.sent[2], { type: "resync" }, "resync on gap");
  eq(h.seen.length, 2, "gap not applied");
  ws.deliver({ ...messages[0], seq: 7 });
  ws.deliver({ ...messages[1], base: 7, seq: 8 });
  eq(h.seen.length, 4, "resynced");
  eq(h.polls.length, 0, "no polling while the socket works");
  h.feed.stop();
}

// 3. Dropped connection: one catch-up poll, reconnect after 1 s, snapshot again.
{
  const h = harness();
  h.feed.start();
  const ws = FakeWS.all[0];
  ws.open();
  ws.deliver(messages[0]);
  h.pollQueue.push(messages[1]);
  ws.close();
  await flush();
  eq(h.polls, [{ seq: 1, epoch: "test" }], "catch-up poll with since");
  eq(h.seen, [23, 23], "catch-up applied");
  await h.advance(999);
  eq(FakeWS.all.length, 1, "not yet reconnected");
  await h.advance(1);
  eq(FakeWS.all.length, 2, "reconnected after 1 s");
  FakeWS.all[1].open();
  FakeWS.all[1].deliver({ ...messages[0], seq: 9 });
  eq(h.seen.length, 3, "snapshot on the new socket");
  h.feed.stop();
}

// 4. WebSocket never connects → delta polling, WebSocket retried, polling stops once it works.
{
  const h = harness();
  h.feed.start();
  FakeWS.all[0].fail();
  await h.advance(1000);
  eq(FakeWS.all.length, 2, "second attempt after 1 s");
  h.pollQueue.push(messages[0], messages[1], messages[2]);
  FakeWS.all[1].fail();
  await flush();
  eq(h.polls[0], null, "first poll asks for a snapshot");
  eq(h.feed.transport, "polling", "polling");
  await h.advance(1000);
  eq(h.polls[1], { seq: 1, epoch: "test" }, "then deltas since the last seq");
  await h.advance(1000);
  eq(h.polls[2], { seq: 2, epoch: "test" }, "next delta");
  eq(h.seen, [23, 23, 23], "all applied");
  // Poll gap (server restarted: unknown epoch answered with a snapshot is fine; a
  // delta that doesn't fit → reset and ask for a snapshot).
  h.pollQueue.push(
    { ...messages[3], base: 99 },
    { ...messages[0], epoch: "new", seq: 1 },
  );
  await h.advance(1000);
  eq(
    h.polls.slice(3),
    [{ seq: 3, epoch: "test" }, null],
    "gap → snapshot poll",
  );
  eq(h.seen.length, 4, "resynced by polling");
  // WebSocket retried (10 s after falling back) while polling.
  h.pollQueue.push(
    ...Array(20).fill({
      type: "delta",
      proto: 1,
      epoch: "new",
      base: 1,
      seq: 1,
    }),
  );
  await h.advance(10_000);
  const retry = FakeWS.all[FakeWS.all.length - 1];
  eq(FakeWS.all.length >= 3, true, "WebSocket retried while polling");
  retry.open();
  retry.deliver({ ...messages[0], epoch: "new", seq: 2 });
  const pollsAfter = h.polls.length;
  await h.advance(5000);
  eq(h.polls.length, pollsAfter, "polling stopped once the socket works");
  eq(h.feed.transport, "websocket", "back on the socket");
  h.feed.stop();
}

// 5. Poll errors are reported and retried after 5 s; no socket at all.
{
  const h = harness({ ws: false });
  h.pollQueue.push(new Error("down"));
  h.feed.start();
  await flush();
  eq(
    h.errors.length === 1 && (h.errors[0] as Error).message === "down",
    true,
    "error reported",
  );
  h.pollQueue.push(messages[0]);
  await h.advance(5000);
  eq(h.seen, [23], "recovered");
  eq(h.errors[h.errors.length - 1], null, "error cleared");
  h.feed.stop();
  eq(h.timers().length, 0, "stop clears timers");
}

// 6. Watchdog: a silent socket is closed and replaced.
{
  const h = harness();
  h.feed.start();
  FakeWS.all[0].open();
  FakeWS.all[0].deliver(messages[0]);
  h.pollQueue.push({
    type: "delta",
    proto: 1,
    epoch: "test",
    base: 1,
    seq: 1,
  } as FeedMessage);
  await h.advance(WATCHDOG_MS);
  eq(FakeWS.all[0].closed, true, "silent socket closed");
  await h.advance(1000);
  eq(FakeWS.all.length, 2, "and replaced");
  h.feed.stop();
}

console.log(`torrentFeed: ${checks - failures}/${checks} checks passed`);
if (failures) throw new Error(`${failures} torrentFeed check(s) failed`);
